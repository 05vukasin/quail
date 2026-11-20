//! A thin streaming layer over quick-xml: one event loop for every part, with entity
//! resolution, deadline checks and the decompression budget folded in.
//!
//! Elements are matched by local name (`w:p` and `ns0:p` alike): OOXML producers do not all
//! use the conventional prefixes, and Strict OOXML changes the namespace URIs but not the
//! local names.

use std::borrow::Cow;
use std::io::{BufReader, Read};

use quail_core::Emitter;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::package::BUDGET_MSG;

/// How often (in events) the deadline is checked.
const DEADLINE_EVERY: u32 = 2048;

/// What the walk callback sees.
#[derive(Clone, Copy)]
pub(crate) enum Ev<'e> {
    /// Start tag; the callback may answer [`Flow::Skip`] to skip its whole subtree.
    Start(&'e BytesStart<'e>),
    /// Self-closing tag.
    Empty(&'e BytesStart<'e>),
    /// End tag, by local name.
    End(&'e str),
    /// Character data (text, CDATA, or one resolved entity).
    Text(&'e str),
}

/// What the walk does next.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Keep going.
    Go,
    /// Skip the subtree of the element just started (only meaningful for `Start`).
    Skip,
    /// Stop reading this part.
    Stop,
}

/// Why a walk ended early.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The XML is malformed (or not UTF-8) from this point on.
    Malformed(String),
    /// The part could not be read (a corrupt or lying zip entry).
    Read(String),
    /// The decompression budget ran out.
    Budget,
    /// The file's time budget ran out.
    Timeout,
}

impl Stop {
    /// One line for a warning.
    pub fn describe(&self, part: &str) -> String {
        match self {
            Self::Malformed(m) => format!("{part}: malformed XML, rest of part skipped: {m}"),
            Self::Read(m) => format!("{part}: read error, rest of part skipped: {m}"),
            Self::Budget => format!("{part}: decompression budget exceeded, rest skipped"),
            Self::Timeout => format!("{part}: deadline reached, rest skipped"),
        }
    }
}

/// Streams the events of one part to `f`. Returns `Ok` at end of input or when `f` stops.
pub(crate) fn walk(
    part: impl Read,
    deadline: &Emitter,
    mut f: impl FnMut(Ev<'_>) -> Flow,
) -> Result<(), Stop> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(32 << 10, part));
    {
        let c = reader.config_mut();
        c.check_end_names = false;
        c.allow_unmatched_ends = true;
        c.allow_dangling_amp = true;
        c.expand_empty_elements = false;
        c.trim_text(false);
    }
    let mut buf = Vec::with_capacity(4096);
    let mut skip_buf = Vec::new();
    let mut n: u32 = 0;
    loop {
        n = n.wrapping_add(1);
        if n.is_multiple_of(DEADLINE_EVERY) && deadline.check_deadline().is_err() {
            return Err(Stop::Timeout);
        }
        let ev = match reader.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(e) => return Err(classify(&e)),
        };
        let flow = match &ev {
            Event::Start(e) => {
                let flow = f(Ev::Start(e));
                if flow == Flow::Skip {
                    let name = e.name().as_ref().to_owned();
                    skip_buf.clear();
                    if let Err(e) =
                        reader.read_to_end_into(quick_xml::name::QName(&name), &mut skip_buf)
                    {
                        return Err(classify(&e));
                    }
                    Flow::Go
                } else {
                    flow
                }
            }
            Event::Empty(e) => f(Ev::Empty(e)),
            Event::End(e) => f(Ev::End(local(e.name().as_ref()))),
            Event::Text(t) => f(Ev::Text(&t.xml10_content())),
            Event::CData(t) => f(Ev::Text(&t.xml10_content())),
            Event::GeneralRef(r) => match r.resolve_char_ref() {
                Ok(Some(c)) => {
                    let mut tmp = [0u8; 4];
                    f(Ev::Text(c.encode_utf8(&mut tmp)))
                }
                _ => match quick_xml::escape::resolve_predefined_entity(r) {
                    Some(s) => f(Ev::Text(s)),
                    None => Flow::Go,
                },
            },
            Event::Eof => return Ok(()),
            _ => Flow::Go,
        };
        if flow == Flow::Stop {
            return Ok(());
        }
        buf.clear();
    }
}

fn classify(e: &quick_xml::Error) -> Stop {
    let mut msg = e.to_string();
    msg.truncate(200);
    match e {
        quick_xml::Error::Io(io) if io.to_string().contains(BUDGET_MSG) => Stop::Budget,
        quick_xml::Error::Io(_) => Stop::Read(msg),
        _ => Stop::Malformed(msg),
    }
}

/// The part of a qualified name after the prefix.
pub(crate) fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

/// Local name of a start tag.
pub(crate) fn name<'e>(e: &'e BytesStart<'_>) -> &'e str {
    local(e.name().0)
}

/// Value of the attribute with this local name, entities resolved.
pub(crate) fn attr<'e>(e: &'e BytesStart<'_>, local_name: &str) -> Option<Cow<'e, str>> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| local(a.key.as_ref()) == local_name)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
}

/// Attribute with this exact qualified name (for unprefixed attributes such as `Id`).
pub(crate) fn attr_exact<'e>(e: &'e BytesStart<'_>, qname: &str) -> Option<Cow<'e, str>> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == qname)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
}

/// `w:val`.
pub(crate) fn val<'e>(e: &'e BytesStart<'_>) -> Option<Cow<'e, str>> {
    attr(e, "val")
}

/// An on/off property: on unless `w:val` says `0`, `false`, `off` or `none`.
pub(crate) fn on(e: &BytesStart<'_>) -> bool {
    !matches!(
        val(e).as_deref().map(str::trim),
        Some("0" | "false" | "off" | "none")
    )
}

/// `w:val` as an integer.
pub(crate) fn val_int(e: &BytesStart<'_>) -> Option<i64> {
    val(e).and_then(|v| v.trim().parse().ok())
}

//! A thin streaming layer over quick-xml: one event loop for every part, with entity
//! resolution, deadline checks and the decompression budget folded in.
//!
//! Elements are matched by local name (`w:p` and `ns0:p` alike): OOXML producers do not all
//! use the conventional prefixes, and Strict OOXML changes the namespace URIs but not the
//! local names.

use std::borrow::Cow;
use std::cell::Cell;
use std::io::{BufReader, Read};
use std::rc::Rc;

use quail_core::Emitter;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::package::BUDGET_MSG;

/// How often (in events) the deadline is checked.
const DEADLINE_EVERY: u32 = 2048;

/// What the walk callback sees.
#[derive(Clone, Copy)]
pub enum Ev<'e> {
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
pub enum Flow {
    /// Keep going.
    Go,
    /// Skip the subtree of the element just started (only meaningful for `Start`).
    Skip,
    /// Stop reading this part.
    Stop,
}

/// Why a walk ended early.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    /// The XML is malformed (or not UTF-8) from this point on.
    Malformed(String),
    /// The part could not be read (a corrupt or lying zip entry).
    Read(String),
    /// The decompression budget ran out.
    Budget,
    /// The file's time budget ran out.
    Timeout,
    /// One XML event (a text node, a tag) was larger than this many bytes.
    Oversize(u64),
}

impl Stop {
    /// One line for a warning.
    pub fn describe(&self, part: &str) -> String {
        match self {
            Self::Malformed(m) => format!("{part}: malformed XML, rest of part skipped: {m}"),
            Self::Read(m) => format!("{part}: read error, rest of part skipped: {m}"),
            Self::Budget => format!("{part}: decompression budget exceeded, rest skipped"),
            Self::Timeout => format!("{part}: deadline reached, rest skipped"),
            Self::Oversize(n) => {
                format!("{part}: an XML event is larger than {n} bytes, rest of part skipped")
            }
        }
    }
}

/// Largest single XML event (one text node, one tag) read by default.
pub const MAX_EVENT: u64 = 16 << 20;
const EVENT_MSG: &str = "single XML event too large";

/// What a finished walk noticed besides the events.
#[derive(Debug, Default, Clone, Copy)]
pub struct WalkInfo {
    /// Entity references that are neither predefined nor character references (they come
    /// from a DOCTYPE, which is not expanded): dropped.
    pub unknown_entities: u64,
    /// The part has a DOCTYPE (Word never writes one).
    pub doctype: bool,
}

/// Counts the bytes pulled from the part since the last event: one event (a text node, a
/// tag) may not exceed `max` bytes, so a 300 MB text node cannot be buffered whole.
struct Guard<R> {
    inner: R,
    since: Rc<Cell<u64>>,
    max: u64,
}

impl<R: Read> Read for Guard<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.since.get() > self.max {
            return Err(std::io::Error::other(EVENT_MSG));
        }
        let n = self.inner.read(buf)?;
        self.since.set(self.since.get() + n as u64);
        Ok(n)
    }
}

/// Streams the events of one part to `f`, with the default event size cap.
pub fn walk(
    part: impl Read,
    deadline: &Emitter,
    f: impl FnMut(Ev<'_>) -> Flow,
) -> Result<WalkInfo, Stop> {
    walk_with(part, deadline, MAX_EVENT, f)
}

/// Streams the events of one part to `f`. Returns at end of input or when `f` stops.
/// Skipped subtrees are read event by event, so the deadline still applies inside them.
pub fn walk_with(
    part: impl Read,
    deadline: &Emitter,
    max_event: u64,
    mut f: impl FnMut(Ev<'_>) -> Flow,
) -> Result<WalkInfo, Stop> {
    let since = Rc::new(Cell::new(0u64));
    let guard = Guard {
        inner: part,
        since: Rc::clone(&since),
        max: max_event,
    };
    let mut reader = Reader::from_reader(BufReader::with_capacity(32 << 10, guard));
    {
        let c = reader.config_mut();
        c.check_end_names = false;
        c.allow_unmatched_ends = true;
        c.allow_dangling_amp = true;
        c.expand_empty_elements = false;
        c.trim_text(false);
    }
    let mut info = WalkInfo::default();
    let mut buf = Vec::with_capacity(4096);
    let mut skip_depth: usize = 0;
    let mut n: u32 = 0;
    loop {
        n = n.wrapping_add(1);
        if n.is_multiple_of(DEADLINE_EVERY) && deadline.check_deadline().is_err() {
            return Err(Stop::Timeout);
        }
        buf.clear();
        let ev = match reader.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(e) => return Err(classify(&e, max_event)),
        };
        since.set(0);
        if skip_depth > 0 {
            match ev {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                Event::Eof => return Ok(info),
                _ => {}
            }
            continue;
        }
        let flow = match &ev {
            Event::Start(e) => {
                let flow = f(Ev::Start(e));
                if flow == Flow::Skip {
                    skip_depth = 1;
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
                _ => {
                    if let Some(s) = quick_xml::escape::resolve_predefined_entity(r) {
                        f(Ev::Text(s))
                    } else {
                        info.unknown_entities += 1;
                        Flow::Go
                    }
                }
            },
            Event::DocType(_) => {
                info.doctype = true;
                Flow::Go
            }
            Event::Eof => return Ok(info),
            _ => Flow::Go,
        };
        if flow == Flow::Stop {
            return Ok(info);
        }
    }
}

fn classify(e: &quick_xml::Error, max_event: u64) -> Stop {
    let mut msg = e.to_string();
    msg.truncate(200);
    match e {
        quick_xml::Error::Io(io) if io.to_string().contains(BUDGET_MSG) => Stop::Budget,
        quick_xml::Error::Io(io) if io.to_string().contains(EVENT_MSG) => Stop::Oversize(max_event),
        quick_xml::Error::Io(_) => Stop::Read(msg),
        _ => Stop::Malformed(msg),
    }
}

/// The part of a qualified name after the prefix.
pub fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

/// Local name of a start tag.
pub fn name<'e>(e: &'e BytesStart<'_>) -> &'e str {
    local(e.name().0)
}

/// Value of the attribute with this local name, entities resolved.
pub fn attr<'e>(e: &'e BytesStart<'_>, local_name: &str) -> Option<Cow<'e, str>> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| local(a.key.as_ref()) == local_name)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
}

/// Attribute with this exact qualified name (for unprefixed attributes such as `Id`).
pub fn attr_exact<'e>(e: &'e BytesStart<'_>, qname: &str) -> Option<Cow<'e, str>> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == qname)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
}

/// `w:val`.
pub fn val<'e>(e: &'e BytesStart<'_>) -> Option<Cow<'e, str>> {
    attr(e, "val")
}

/// An on/off property: on unless `w:val` says `0`, `false`, `off` or `none`.
pub fn on(e: &BytesStart<'_>) -> bool {
    !matches!(
        val(e).as_deref().map(str::trim),
        Some("0" | "false" | "off" | "none")
    )
}

/// `w:val` as an integer.
pub fn val_int(e: &BytesStart<'_>) -> Option<i64> {
    val(e).and_then(|v| v.trim().parse().ok())
}

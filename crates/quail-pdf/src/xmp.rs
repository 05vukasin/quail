//! Document identity ids from the catalog's XMP packet (`/Metadata`): `xmpMM:DocumentID` and
//! `xmpMM:OriginalDocumentID`, in attribute or element form, on a top-level `rdf:Description`.
//!
//! Hostile input: the packet is decoded under the file's decode budget and refused above
//! [`MAX_PACKET`]; the XML is read event by event with a nesting cap, the deadline checked, a
//! DOCTYPE never expanded (unknown entities are dropped) and over-long values ignored. A
//! malformed packet gives a warning, never an error; ids read before the fault are kept.

use hayro_syntax::Pdf;
use hayro_syntax::object::{Dict, Stream};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use quick_xml::{NsReader, XmlVersion};

/// Largest XMP packet read, decoded (packets with thumbnails reach a few hundred KiB).
pub(crate) const MAX_PACKET: usize = 4 << 20;
/// Element nesting allowed in the packet (real packets nest under ten levels).
const MAX_DEPTH: usize = 64;
/// Longest id kept, in bytes (ids are UUIDs or GUIDs with a short scheme prefix).
const MAX_ID: usize = 256;
/// How often (in events) the deadline is checked.
const DEADLINE_EVERY: u32 = 1024;

const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_MM: &str = "http://ns.adobe.com/xap/1.0/mm/";

/// The ids found, each trimmed and as written otherwise.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Ids {
    pub document_id: Option<String>,
    pub original_document_id: Option<String>,
}

/// Reads the ids from the catalog's `/Metadata` stream. Warnings go to `warn`.
pub(crate) fn catalog_ids(pdf: &Pdf, late: &dyn Fn() -> bool, warn: &mut Vec<String>) -> Ids {
    let xref = pdf.xref();
    let Some(stream) = xref
        .get::<Dict<'_>>(xref.root_id())
        .and_then(|c| c.get::<Stream<'_>>(b"Metadata"))
    else {
        return Ids::default();
    };
    if stream.raw_data().len() > MAX_PACKET {
        warn.push(format!(
            "XMP packet larger than {MAX_PACKET} bytes, identity ids not read"
        ));
        return Ids::default();
    }
    let Ok(data) = stream.decoded() else {
        warn.push("XMP packet could not be decoded, identity ids not read".to_owned());
        return Ids::default();
    };
    if data.len() > MAX_PACKET {
        warn.push(format!(
            "XMP packet larger than {MAX_PACKET} bytes, identity ids not read"
        ));
        return Ids::default();
    }
    ids(&data, late, warn)
}

/// Which id an attribute or element names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Document,
    Original,
}

/// The id an `xmpMM:*` name stands for. A name is in `xmpMM` when its prefix is bound to the
/// XMP Media Management namespace, or (in a packet that forgot the declaration) when the
/// prefix is literally `xmpMM`.
fn which(res: &ResolveResult<'_>, name: QName<'_>) -> Option<Which> {
    let in_mm = match res {
        ResolveResult::Bound(ns) => ns.as_ref() == NS_MM,
        ResolveResult::Unknown(p) => p == "xmpMM",
        ResolveResult::Unbound => false,
    };
    if !in_mm {
        return None;
    }
    match name.local_name().as_ref() {
        "DocumentID" => Some(Which::Document),
        "OriginalDocumentID" => Some(Which::Original),
        _ => None,
    }
}

fn is_rdf(res: &ResolveResult<'_>, name: QName<'_>, local: &str) -> bool {
    let ns_ok = match res {
        ResolveResult::Bound(ns) => ns.as_ref() == NS_RDF,
        ResolveResult::Unknown(p) => p == "rdf",
        ResolveResult::Unbound => false,
    };
    ns_ok && name.local_name().as_ref() == local
}

/// What a start tag is, for the walk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Rdf,
    Description,
    Id(Which),
    Other,
}

fn tag(res: &ResolveResult<'_>, name: QName<'_>) -> Tag {
    if is_rdf(res, name, "RDF") {
        Tag::Rdf
    } else if is_rdf(res, name, "Description") {
        Tag::Description
    } else {
        which(res, name).map_or(Tag::Other, Tag::Id)
    }
}

/// Collects the ids of one packet.
struct Found<'w> {
    ids: Ids,
    too_long: bool,
    warn: &'w mut Vec<String>,
}

impl Found<'_> {
    /// Sets `which` to `raw` (trimmed) unless it is already set, empty or over-long.
    fn set(&mut self, which: Which, raw: &str) {
        let v = raw.trim();
        if v.is_empty() {
            return;
        }
        if v.len() > MAX_ID || v.chars().any(char::is_control) {
            self.too_long = true;
            return;
        }
        let slot = match which {
            Which::Document => &mut self.ids.document_id,
            Which::Original => &mut self.ids.original_document_id,
        };
        if slot.is_none() {
            *slot = Some(v.to_owned());
        }
    }

    /// The attribute form: `<rdf:Description xmpMM:DocumentID="..."/>`.
    fn attributes(&mut self, r: &NsReader<&[u8]>, e: &BytesStart<'_>) {
        for a in e.attributes().with_checks(false).flatten() {
            let (res, _) = r.resolver().resolve_attribute(a.key);
            if let Some(w) = which(&res, a.key)
                && let Ok(v) = a.normalized_value(XmlVersion::Implicit1_0)
            {
                self.set(w, &v);
            }
        }
    }
}

/// The packet as text: UTF-8, or UTF-16 with a byte order mark. `None` otherwise.
fn packet_text(data: &[u8]) -> Option<std::borrow::Cow<'_, str>> {
    let utf16 = |rest: &[u8], be: bool| {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok().map(std::borrow::Cow::Owned)
    };
    if let Some(rest) = data.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true);
    }
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false);
    }
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    std::str::from_utf8(data)
        .ok()
        .map(std::borrow::Cow::Borrowed)
}

/// Reads the ids from a decoded packet.
pub(crate) fn ids(data: &[u8], late: &dyn Fn() -> bool, warn: &mut Vec<String>) -> Ids {
    let Some(text) = packet_text(data) else {
        warn.push("XMP packet is not UTF-8 or UTF-16, identity ids not read".to_owned());
        return Ids::default();
    };
    let mut found = Found {
        ids: Ids::default(),
        too_long: false,
        warn,
    };
    if let Err(msg) = walk(text.as_bytes(), late, &mut found) {
        found.warn.push(format!("XMP packet: {msg}"));
    }
    if found.too_long {
        found.warn.push(format!(
            "XMP packet: an identity id longer than {MAX_ID} bytes or with control characters, ignored"
        ));
    }
    found.ids
}

/// The event loop. Only `rdf:Description` elements directly under `rdf:RDF` count: the
/// `xmpMM:Pantry` and `xmpMM:Ingredients` structures nest the ids of other documents.
fn walk(xml: &[u8], late: &dyn Fn() -> bool, found: &mut Found<'_>) -> Result<(), String> {
    let mut r = NsReader::from_reader(xml);
    {
        let c = r.config_mut();
        c.trim_text(false);
        c.expand_empty_elements = false;
    }
    let mut depth = 0usize;
    // Depth of the open `rdf:RDF`, of the open top-level `rdf:Description`, and the id
    // element being read (with its depth and text so far).
    let mut rdf_at: Option<usize> = None;
    let mut desc_at: Option<usize> = None;
    let mut reading: Option<(Which, usize, String)> = None;
    let mut n: u32 = 0;
    loop {
        n = n.wrapping_add(1);
        if n.is_multiple_of(DEADLINE_EVERY) && late() {
            return Err("deadline reached, rest skipped".to_owned());
        }
        let (kind, ev) = match r.read_resolved_event() {
            Ok((res, ev)) => {
                let kind = match &ev {
                    Event::Start(e) | Event::Empty(e) => tag(&res, e.name()),
                    _ => Tag::Other,
                };
                (kind, ev)
            }
            Err(e) => {
                let mut m = format!("malformed XML, rest skipped: {e}");
                m.truncate(200);
                return Err(m);
            }
        };
        match ev {
            Event::Start(e) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(format!("nested deeper than {MAX_DEPTH}, rest skipped"));
                }
                match kind {
                    Tag::Rdf if rdf_at.is_none() => rdf_at = Some(depth),
                    Tag::Description if rdf_at.is_some_and(|d| d + 1 == depth) => {
                        desc_at = Some(depth);
                        found.attributes(&r, &e);
                    }
                    Tag::Id(w) if desc_at.is_some_and(|d| d + 1 == depth) => {
                        reading = Some((w, depth, String::new()));
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                if depth + 1 > MAX_DEPTH {
                    return Err(format!("nested deeper than {MAX_DEPTH}, rest skipped"));
                }
                if kind == Tag::Description && rdf_at == Some(depth) {
                    found.attributes(&r, &e);
                }
            }
            Event::End(_) => {
                if let Some((w, d, text)) = reading.take() {
                    if d == depth {
                        found.set(w, &text);
                    } else {
                        reading = Some((w, d, text));
                    }
                }
                if desc_at == Some(depth) {
                    desc_at = None;
                }
                if rdf_at == Some(depth) {
                    rdf_at = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Text(t) => push_text(&mut reading, depth, &t.xml10_content()),
            Event::CData(t) => push_text(&mut reading, depth, &t.xml10_content()),
            Event::GeneralRef(g) => {
                let c = match g.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => quick_xml::escape::resolve_predefined_entity(&g).map(str::to_owned),
                };
                // Entities from a DOCTYPE are never expanded: dropped.
                if let Some(c) = c {
                    push_text(&mut reading, depth, &c);
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

/// Appends direct text of the id element being read, up to just over [`MAX_ID`] (enough to
/// tell an over-long value).
fn push_text(reading: &mut Option<(Which, usize, String)>, depth: usize, s: &str) {
    if let Some((_, d, text)) = reading
        && *d == depth
        && text.len() <= MAX_ID
    {
        let room = MAX_ID + 1 - text.len();
        let mut end = s.len().min(room);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&s[..end]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(xml: &str) -> (Ids, Vec<String>) {
        let mut w = Vec::new();
        let ids = ids(xml.as_bytes(), &|| false, &mut w);
        (ids, w)
    }

    const HEAD: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#;
    const TAIL: &str = r#"</rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;

    #[test]
    fn attribute_form() {
        let (ids, w) = read(&format!(
            r#"{HEAD}<rdf:Description rdf:about="" xmlns:mm="http://ns.adobe.com/xap/1.0/mm/" mm:DocumentID=" uuid:1111 " mm:OriginalDocumentID="xmp.did:2222" mm:InstanceID="uuid:3333"/>{TAIL}"#
        ));
        assert_eq!(ids.document_id.as_deref(), Some("uuid:1111"));
        assert_eq!(ids.original_document_id.as_deref(), Some("xmp.did:2222"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn element_form_and_nested_ids_ignored() {
        let (ids, w) = read(&format!(
            r#"{HEAD}<rdf:Description rdf:about="" xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmlns:stRef="http://ns.adobe.com/xap/1.0/sType/ResourceRef#">
<xmpMM:DerivedFrom rdf:parseType="Resource"><stRef:documentID>xmp.did:other</stRef:documentID></xmpMM:DerivedFrom>
<xmpMM:Pantry><rdf:Bag><rdf:li><rdf:Description xmpMM:DocumentID="uuid:embedded"/></rdf:li></rdf:Bag></xmpMM:Pantry>
<xmpMM:DocumentID>
  adobe:docid:indd:a&amp;b&#x31;
</xmpMM:DocumentID>
<xmpMM:OriginalDocumentID><![CDATA[uuid:ABCD]]></xmpMM:OriginalDocumentID>
</rdf:Description>{TAIL}"#
        ));
        assert_eq!(ids.document_id.as_deref(), Some("adobe:docid:indd:a&b1"));
        assert_eq!(ids.original_document_id.as_deref(), Some("uuid:ABCD"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn first_description_wins_and_other_namespaces_ignored() {
        let (ids, _) = read(&format!(
            r#"{HEAD}<rdf:Description xmlns:o="http://example.com/other/" o:DocumentID="no"/><rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmpMM:DocumentID="first"/><rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmpMM:DocumentID="second"/>{TAIL}"#
        ));
        assert_eq!(ids.document_id.as_deref(), Some("first"));
        assert_eq!(ids.original_document_id, None);
    }

    #[test]
    fn undeclared_xmpmm_prefix_is_accepted() {
        let (ids, _) = read(&format!(
            r#"{HEAD}<rdf:Description xmpMM:DocumentID="uuid:x"/>{TAIL}"#
        ));
        assert_eq!(ids.document_id.as_deref(), Some("uuid:x"));
    }

    #[test]
    fn doctype_entities_are_not_expanded() {
        let xml = format!(
            r#"<!DOCTYPE x [<!ENTITY a "AAAAAAAAAA"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]>{HEAD}<rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"><xmpMM:DocumentID>id&b;</xmpMM:DocumentID></rdf:Description>{TAIL}"#
        );
        let (ids, _) = read(&xml);
        assert_eq!(ids.document_id.as_deref(), Some("id"));
    }

    #[test]
    fn hostile_packets_warn() {
        // Malformed: the id read before the fault is kept.
        let (ids, w) = read(&format!(
            r#"{HEAD}<rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmpMM:DocumentID="kept"></rdf:Descr"#
        ));
        assert_eq!(ids.document_id.as_deref(), Some("kept"));
        assert!(w.iter().any(|m| m.contains("malformed")), "{w:?}");

        let deep = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
        let (ids, w) = read(&deep);
        assert_eq!(ids, Ids::default());
        assert!(w.iter().any(|m| m.contains("deeper")), "{w:?}");

        let long = "x".repeat(MAX_ID + 1);
        let (ids, w) = read(&format!(
            r#"{HEAD}<rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"><xmpMM:DocumentID>{long}</xmpMM:DocumentID></rdf:Description>{TAIL}"#
        ));
        assert_eq!(ids.document_id, None);
        assert!(w.iter().any(|m| m.contains("longer")), "{w:?}");

        let mut bad = Vec::new();
        let ids2 = super::ids(&[0xC3, 0x28, b'<'], &|| false, &mut bad);
        assert_eq!(ids2, Ids::default());
        assert!(bad.iter().any(|m| m.contains("UTF-8")), "{bad:?}");

        // The deadline stops the walk.
        let many = format!("{HEAD}{}{TAIL}", "<rdf:li/>".repeat(10_000));
        let mut w = Vec::new();
        let _ = super::ids(many.as_bytes(), &|| true, &mut w);
        assert!(w.iter().any(|m| m.contains("deadline")), "{w:?}");
    }

    #[test]
    fn utf16_packet() {
        let xml = format!(
            r#"{HEAD}<rdf:Description xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmpMM:DocumentID="uuid:16"/>{TAIL}"#
        );
        let mut data = vec![0xFE, 0xFF];
        for u in xml.encode_utf16() {
            data.extend_from_slice(&u.to_be_bytes());
        }
        let mut w = Vec::new();
        assert_eq!(
            super::ids(&data, &|| false, &mut w).document_id.as_deref(),
            Some("uuid:16")
        );
    }
}

//! File-level facts from the document catalog and the Info dictionary: forms, signatures,
//! JavaScript, embedded files, XMP, fonts. Every tree walk is bounded in depth and size.

use std::collections::BTreeSet;

use hayro_syntax::Pdf;
use hayro_syntax::object::{Array, DateTime, Dict, Name, Object, Stream};
use hayro_syntax::page::Page;
use quail_core::Date;

/// Nodes visited in one tree walk (fields, name trees).
const MAX_NODES: usize = 20_000;
/// Nesting depth of one tree walk.
const MAX_DEPTH: u32 = 32;
/// Distinct font names kept.
const MAX_FONTS: usize = 512;

/// What the catalog says about the document.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct CatalogFacts {
    pub has_xmp: bool,
    pub form_fields: usize,
    pub signature_fields: usize,
    pub signed_fields: usize,
    pub has_javascript: bool,
    pub embedded_files: usize,
}

pub(crate) fn catalog_facts(pdf: &Pdf, raw: &[u8]) -> CatalogFacts {
    let mut out = CatalogFacts::default();
    let xref = pdf.xref();
    let Some(catalog) = xref.get::<Dict<'_>>(xref.root_id()) else {
        return out;
    };
    out.has_xmp = catalog.get::<Stream<'_>>(b"Metadata").is_some();

    if let Some(form) = catalog.get::<Dict<'_>>(b"AcroForm")
        && let Some(fields) = form.get::<Array<'_>>(b"Fields")
    {
        let mut budget = MAX_NODES;
        for f in fields.iter::<Dict<'_>>() {
            walk_field(&f, None, 0, &mut budget, &mut out);
        }
        if form.contains_key(b"XFA") {
            // XFA forms keep their fields in XML; count the form itself as one field.
            out.form_fields = out.form_fields.max(1);
        }
    }

    let names = catalog.get::<Dict<'_>>(b"Names");
    if let Some(ef) = names
        .as_ref()
        .and_then(|n| n.get::<Dict<'_>>(b"EmbeddedFiles"))
    {
        let mut budget = MAX_NODES;
        out.embedded_files = name_tree_len(&ef, 0, &mut budget);
    }
    let js_tree = names
        .as_ref()
        .is_some_and(|n| n.contains_key(b"JavaScript"));
    let open_js = catalog
        .get::<Dict<'_>>(b"OpenAction")
        .is_some_and(|a| is_js_action(&a));
    // Actions can hide anywhere (annotations, fields, pages). Uncompressed files reveal them
    // to a byte scan; compressed object streams do not, hence the catalog checks above.
    let raw_js = crate::raw::find(raw, b"/JavaScript").is_some()
        || crate::raw::find(raw, b"/JS ").is_some()
        || crate::raw::find(raw, b"/JS(").is_some()
        || crate::raw::find(raw, b"/JS<").is_some();
    out.has_javascript = js_tree || open_js || raw_js || js_in_objects(pdf);
    out
}

/// Objects inspected by [`js_in_objects`].
const MAX_OBJECTS_SCANNED: usize = 200_000;

/// Any dictionary that is a JavaScript action (`/S /JavaScript`) or carries a `/JS` entry.
/// Walks every object, including those inside compressed object streams, which a byte scan
/// cannot see. Only runs when the cheaper checks found nothing.
fn js_in_objects(pdf: &Pdf) -> bool {
    pdf.objects()
        .into_iter()
        .take(MAX_OBJECTS_SCANNED)
        .any(|o| match o {
            Object::Dict(d) => d.contains_key(b"JS") || is_js_action(&d),
            _ => false,
        })
}

fn is_js_action(a: &Dict<'_>) -> bool {
    a.get::<Name<'_>>(b"S")
        .is_some_and(|s| s.as_str() == "JavaScript")
}

/// Counts terminal fields (those without `Kids`, or whose kids are only widgets) and
/// signature fields. `FT` is inheritable.
fn walk_field(
    f: &Dict<'_>,
    inherited_ft: Option<&str>,
    depth: u32,
    budget: &mut usize,
    out: &mut CatalogFacts,
) {
    if depth > MAX_DEPTH || *budget == 0 {
        return;
    }
    *budget -= 1;
    let own_ft = f.get::<Name<'_>>(b"FT").map(|n| n.as_str().to_owned());
    let ft = own_ft.as_deref().or(inherited_ft);
    let kids: Vec<Dict<'_>> = f
        .get::<Array<'_>>(b"Kids")
        .map(|k| k.iter::<Dict<'_>>().collect())
        .unwrap_or_default();
    // Kids that carry a field name (`T`) are fields; the rest are widget annotations.
    let field_kids: Vec<&Dict<'_>> = kids.iter().filter(|k| k.contains_key(b"T")).collect();
    if field_kids.is_empty() {
        out.form_fields += 1;
        if ft == Some("Sig") {
            out.signature_fields += 1;
            if f.contains_key(b"V") {
                out.signed_fields += 1;
            }
        }
        return;
    }
    for k in field_kids {
        walk_field(k, ft, depth + 1, budget, out);
    }
}

/// Number of entries in a name tree (`Names` arrays hold key/value pairs).
fn name_tree_len(node: &Dict<'_>, depth: u32, budget: &mut usize) -> usize {
    if depth > MAX_DEPTH || *budget == 0 {
        return 0;
    }
    *budget -= 1;
    let mut n = 0;
    if let Some(names) = node.get::<Array<'_>>(b"Names") {
        n += names.raw_iter().count() / 2;
    }
    if let Some(kids) = node.get::<Array<'_>>(b"Kids") {
        for k in kids.iter::<Dict<'_>>() {
            n += name_tree_len(&k, depth + 1, budget);
            if *budget == 0 {
                break;
            }
        }
    }
    n
}

/// Font names used by a page's resources (`BaseFont`, subset tag stripped).
pub(crate) fn page_fonts(page: &Page<'_>, out: &mut BTreeSet<String>) {
    let fonts = &page.resources().fonts;
    for key in fonts.keys() {
        if out.len() >= MAX_FONTS {
            return;
        }
        let Some(font) = fonts.get::<Dict<'_>>(&*key) else {
            continue;
        };
        if let Some(name) = font.get::<Name<'_>>(b"BaseFont") {
            let n = strip_subset(name.as_str());
            if !n.is_empty() {
                out.insert(n.to_owned());
            }
        }
    }
}

/// `ABCDEF+Arial-Bold` → `Arial-Bold`.
pub(crate) fn strip_subset(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}

/// A calendar date from a PDF date, when it is plausible.
pub(crate) fn to_date(d: &DateTime) -> Option<Date> {
    let y = i16::try_from(d.year).ok()?;
    let ok = (1970..=2200).contains(&y) && (1..=12).contains(&d.month) && (1..=31).contains(&d.day);
    ok.then_some(Date {
        y,
        m: d.month,
        d: d.day,
    })
}

/// Decodes a PDF text string: UTF-16BE or UTF-8 with a BOM, else `PDFDocEncoding`. Control
/// characters are dropped and whitespace trimmed; `None` when nothing is left.
pub(crate) fn text_string(bytes: &[u8]) -> Option<String> {
    let s = if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| pdf_doc_char(b)).collect()
    };
    let s: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_owned();
    (!s.is_empty()).then_some(s)
}

/// `PDFDocEncoding`: Latin-1 except for 0x18–0x1F and 0x80–0x9F.
fn pdf_doc_char(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}',
        '\u{2044}', '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}',
        '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}',
        '\u{0141}', '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}',
        '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}',
    ];
    const LOW: [char; 8] = [
        '\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}',
        '\u{02DC}',
    ];
    match b {
        0x18..=0x1F => LOW
            .get(usize::from(b - 0x18))
            .copied()
            .unwrap_or('\u{FFFD}'),
        0x80..=0x9F => HIGH
            .get(usize::from(b - 0x80))
            .copied()
            .unwrap_or('\u{FFFD}'),
        _ => char::from(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_prefix() {
        assert_eq!(strip_subset("ABCDEF+Arial-Bold"), "Arial-Bold");
        assert_eq!(strip_subset("Helvetica"), "Helvetica");
        assert_eq!(strip_subset("abcdef+X"), "abcdef+X");
        assert_eq!(strip_subset("AB+X"), "AB+X");
    }

    #[test]
    fn text_strings() {
        assert_eq!(text_string(b"  Hello\0 ").as_deref(), Some("Hello"));
        assert_eq!(
            text_string(&[0xFE, 0xFF, 0x00, 0x41, 0x04, 0x16]).as_deref(),
            Some("A\u{416}")
        );
        assert_eq!(
            text_string(&[0x8D, b'x', 0x8E]).as_deref(),
            Some("\u{201C}x\u{201D}")
        );
        assert_eq!(text_string(b"\0\0"), None);
        // Odd UTF-16 length does not panic.
        assert_eq!(text_string(&[0xFE, 0xFF, 0x00]), None);
    }
}

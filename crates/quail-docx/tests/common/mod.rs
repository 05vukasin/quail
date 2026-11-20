//! Builds DOCX files in code: a zip with hand-written minimal XML parts.

#![allow(
    dead_code,
    clippy::format_push_string,
    clippy::cast_possible_truncation
)]

use std::io::{Cursor, Write};

use quail_core::{Blob, Document, Emitter, Extractor, Fact, Limits, Value};
use quail_docx::DocxExtractor;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A paragraph with plain text.
pub fn p(text: &str) -> String {
    format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
}

/// A paragraph with a style.
pub fn ps(style: &str, text: &str) -> String {
    format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#)
}

/// A numbered paragraph.
pub fn pn(num: u32, ilvl: u32, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num}"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
    )
}

/// A part with the `WordprocessingML` namespaces declared on its root.
pub fn wpart(root: &str, inner: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} xmlns:w="{W}" xmlns:r="{R}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">{inner}</w:{root}>"#
    )
}

pub struct Docx {
    body: String,
    parts: Vec<(String, Vec<u8>)>,
    doc_rels: Vec<(String, String, String, bool)>,
    overrides: Vec<(String, String)>,
    pub store: bool,
}

impl Docx {
    /// A document whose `w:body` holds `body`.
    pub fn new(body: &str) -> Self {
        Self {
            body: body.to_owned(),
            parts: Vec::new(),
            doc_rels: Vec::new(),
            overrides: Vec::new(),
            store: false,
        }
    }

    /// Adds a part.
    pub fn part(mut self, name: &str, content: impl Into<Vec<u8>>) -> Self {
        self.parts.push((name.to_owned(), content.into()));
        self
    }

    /// Adds a part related from the main document with relationship type `kind`.
    pub fn related(mut self, id: &str, kind: &str, target: &str, content: &str) -> Self {
        self.doc_rels
            .push((id.to_owned(), kind.to_owned(), target.to_owned(), false));
        self.parts
            .push((format!("word/{target}"), content.as_bytes().to_vec()));
        self
    }

    /// Adds an external relationship of the main document.
    pub fn external(mut self, id: &str, kind: &str, target: &str) -> Self {
        self.doc_rels
            .push((id.to_owned(), kind.to_owned(), target.to_owned(), true));
        self
    }

    /// Overrides the content type of a part.
    pub fn content_type(mut self, part: &str, ct: &str) -> Self {
        self.overrides.push((part.to_owned(), ct.to_owned()));
        self
    }

    pub fn build(self) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(if self.store {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        });
        let mut add = |name: &str, data: &[u8]| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(data).unwrap();
        };
        let mut ct = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#,
        );
        let mut main_ct =
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
                .to_owned();
        for (part, t) in &self.overrides {
            if part == "/word/document.xml" {
                main_ct.clone_from(t);
            } else {
                ct.push_str(&format!(
                    r#"<Override PartName="{part}" ContentType="{t}"/>"#
                ));
            }
        }
        ct.push_str(&format!(
            r#"<Override PartName="/word/document.xml" ContentType="{main_ct}"/></Types>"#
        ));
        add("[Content_Types].xml", ct.as_bytes());
        let mut pkg_rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/>"#
        );
        let has = |n: &str| self.parts.iter().any(|(p, _)| p == n);
        if has("docProps/core.xml") {
            pkg_rels.push_str(r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>"#);
        }
        if has("docProps/app.xml") {
            pkg_rels.push_str(&format!(
                r#"<Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/>"#
            ));
        }
        if has("docProps/custom.xml") {
            pkg_rels.push_str(&format!(
                r#"<Relationship Id="rId4" Type="{REL}/custom-properties" Target="docProps/custom.xml"/>"#
            ));
        }
        pkg_rels.push_str("</Relationships>");
        add("_rels/.rels", pkg_rels.as_bytes());
        let mut rels = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (id, kind, target, external) in &self.doc_rels {
            let mode = if *external {
                r#" TargetMode="External""#
            } else {
                ""
            };
            rels.push_str(&format!(
                r#"<Relationship Id="{id}" Type="{REL}/{kind}" Target="{target}"{mode}/>"#
            ));
        }
        rels.push_str("</Relationships>");
        add("word/_rels/document.xml.rels", rels.as_bytes());
        let doc = wpart("document", &format!("<w:body>{}</w:body>", self.body));
        add("word/document.xml", doc.as_bytes());
        for (name, data) in &self.parts {
            add(name, data);
        }
        zip.finish().unwrap().into_inner()
    }
}

/// Runs the extractor with default limits.
pub fn extract(bytes: Vec<u8>) -> (Document, Emitter) {
    extract_with(bytes, &Limits::default())
}

/// Runs the extractor with `limits`.
pub fn extract_with(bytes: Vec<u8>, limits: &Limits) -> (Document, Emitter) {
    let mut out = Emitter::new(quail_docx::ID);
    let doc = DocxExtractor
        .extract(&Blob::from(bytes), limits, &mut out)
        .expect("extract");
    (doc, out)
}

/// The value of the first fact with this key.
pub fn fact<'a>(out: &'a Emitter, key: &str) -> Option<&'a Value> {
    out.facts()
        .iter()
        .find(|f: &&Fact| f.key.as_str() == key)
        .map(|f| &f.value)
}

/// Texts of a list-of-text fact.
pub fn texts(out: &Emitter, key: &str) -> Vec<String> {
    match fact(out, key) {
        Some(Value::List(v)) => v
            .iter()
            .filter_map(|x| match x {
                Value::Text(t) => Some(t.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Warnings of a finished emitter.
pub fn warnings(out: Emitter) -> Vec<String> {
    out.into_parts().2
}

/// `zip` with the entry `name` replaced by `data` (all entries stored).
pub fn replace_part(zip: &[u8], name: &str, data: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut a = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    let mut entries = Vec::new();
    for i in 0..a.len() {
        let mut f = a.by_index(i).unwrap();
        let n = f.name().unwrap().into_owned();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        entries.push((n, buf));
    }
    let list: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), if n == name { data } else { b.as_slice() }))
        .collect();
    raw_zip(&list)
}

/// A stored zip written byte by byte, so it can hold what zip writers refuse (repeated
/// names).
pub fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                h.extend_from_slice(&[0; 6]);
                h.extend_from_slice(&0u32.to_le_bytes());
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(data);
        central.extend(header(0x0201_4b50, true));
    }
    let cd_at = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend(central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

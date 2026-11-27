//! Builds PPTX files in code: a zip with hand-written minimal XML parts.

#![allow(dead_code, clippy::format_push_string, clippy::format_collect)]

use std::io::{Cursor, Write};

use quail_core::{Blob, BlockKind, Document, Emitter, Extractor, Fact, Limits, Value, Zone};
use quail_pptx::PptxExtractor;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// `a:p` paragraphs from lines; a line starting with `-` gets an explicit bullet, `~` an
/// explicit `a:buNone`.
pub fn paras(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|l| {
            let (ppr, t) = if let Some(t) = l.strip_prefix('-') {
                (r#"<a:pPr><a:buChar char="•"/></a:pPr>"#, t)
            } else if let Some(t) = l.strip_prefix('~') {
                ("<a:pPr><a:buNone/></a:pPr>", t)
            } else {
                ("", *l)
            };
            format!("<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"/><a:t>{t}</a:t></a:r></a:p>")
        })
        .collect()
}

/// A shape: an optional placeholder (`type`, `idx`), an optional offset in EMU, text.
pub fn sp(id: u32, ph: Option<(&str, Option<u32>)>, pos: Option<(i64, i64)>, body: &str) -> String {
    let ph = match ph {
        Some((t, idx)) => {
            let t = if t.is_empty() {
                String::new()
            } else {
                format!(r#" type="{t}""#)
            };
            let idx = idx.map(|i| format!(r#" idx="{i}""#)).unwrap_or_default();
            format!("<p:ph{t}{idx}/>")
        }
        None => String::new(),
    };
    let xfrm = pos
        .map(|(x, y)| {
            format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="100" cy="100"/></a:xfrm>"#)
        })
        .unwrap_or_default();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Shape {id}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{xfrm}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>"#
    )
}

/// A slide-like part (`sld`, `sldLayout`, `sldMaster`, `notes`) around shapes.
pub fn part(root: &str, attrs: &str, shapes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:{root} {NS}{attrs}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{shapes}</p:spTree></p:cSld></p:{root}>"#
    )
}

pub fn slide(shapes: &str) -> String {
    part("sld", "", shapes)
}

pub fn notes(text: &str) -> String {
    part(
        "notes",
        "",
        &(sp(2, Some(("sldImg", None)), None, "")
            + &sp(3, Some(("body", Some(1))), None, &paras(&[text]))
            + &sp(4, Some(("sldNum", Some(5))), None, &paras(&["7"]))),
    )
}

pub struct Slide {
    pub xml: String,
    pub notes: Option<String>,
}

pub struct Pptx {
    pub slides: Vec<Slide>,
    pub layout: String,
    pub master: String,
    pub extra: Vec<(String, Vec<u8>)>,
    pub core: Option<String>,
    /// Slide part numbers in `sldIdLst` order (default: 1..=n).
    pub order: Option<Vec<usize>>,
}

/// A layout whose title sits at the top and body below it; the master puts dates, footers
/// and slide numbers at the bottom.
pub fn default_layout() -> String {
    part(
        "sldLayout",
        "",
        &(sp(2, Some(("title", None)), Some((457_200, 274_638)), "")
            + &sp(3, Some(("", Some(1))), Some((457_200, 1_600_200)), "")),
    )
}

pub fn default_master() -> String {
    part(
        "sldMaster",
        "",
        &(sp(2, Some(("title", None)), Some((457_200, 274_638)), "")
            + &sp(3, Some(("body", Some(1))), Some((457_200, 1_600_200)), "")
            + &sp(4, Some(("ftr", Some(3))), Some((3_124_200, 6_356_350)), "")),
    )
}

impl Pptx {
    pub fn new(slides: Vec<Slide>) -> Self {
        Self {
            slides,
            layout: default_layout(),
            master: default_master(),
            extra: Vec::new(),
            core: None,
            order: None,
        }
    }

    pub fn of(slides: &[&str]) -> Self {
        Self::new(
            slides
                .iter()
                .map(|s| Slide {
                    xml: (*s).to_owned(),
                    notes: None,
                })
                .collect(),
        )
    }

    pub fn build(&self) -> Vec<u8> {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut ct = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>"#,
        );
        let n = self.slides.len();
        for i in 1..=n {
            ct.push_str(&format!(r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#));
        }
        ct.push_str("</Types>");
        files.push(("[Content_Types].xml".into(), ct.into_bytes()));
        files.push((
            "_rels/.rels".into(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#).into_bytes(),
        ));
        let order: Vec<usize> = self.order.clone().unwrap_or_else(|| (1..=n).collect());
        let ids: String = order
            .iter()
            .enumerate()
            .map(|(k, i)| format!(r#"<p:sldId id="{}" r:id="rIdS{i}"/>"#, 256 + k))
            .collect();
        files.push((
            "ppt/presentation.xml".into(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/></p:presentation>"#).into_bytes(),
        ));
        let mut prels = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#
        );
        for i in 1..=n {
            prels.push_str(&format!(
                r#"<Relationship Id="rIdS{i}" Type="{REL}/slide" Target="slides/slide{i}.xml"/>"#
            ));
        }
        prels.push_str("</Relationships>");
        files.push(("ppt/_rels/presentation.xml.rels".into(), prels.into_bytes()));
        for (k, s) in self.slides.iter().enumerate() {
            let i = k + 1;
            files.push((
                format!("ppt/slides/slide{i}.xml"),
                s.xml.clone().into_bytes(),
            ));
            let mut rels = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#
            );
            if let Some(nx) = &s.notes {
                rels.push_str(&format!(r#"<Relationship Id="rId2" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide{i}.xml"/>"#));
                files.push((
                    format!("ppt/notesSlides/notesSlide{i}.xml"),
                    nx.clone().into_bytes(),
                ));
            }
            rels.push_str("</Relationships>");
            files.push((
                format!("ppt/slides/_rels/slide{i}.xml.rels"),
                rels.into_bytes(),
            ));
        }
        files.push((
            "ppt/slideLayouts/slideLayout1.xml".into(),
            self.layout.clone().into_bytes(),
        ));
        files.push((
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).into_bytes(),
        ));
        files.push((
            "ppt/slideMasters/slideMaster1.xml".into(),
            self.master.clone().into_bytes(),
        ));
        if let Some(c) = &self.core {
            files.push(("docProps/core.xml".into(), c.clone().into_bytes()));
        }
        files.extend(self.extra.iter().cloned());
        zip_of(&files, CompressionMethod::Deflated)
    }
}

pub fn zip_of(files: &[(String, Vec<u8>)], method: CompressionMethod) -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(method);
    for (name, data) in files {
        w.start_file(name.as_str(), opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

pub struct Run {
    pub doc: Document,
    pub out: Emitter,
}

pub fn run(bytes: &[u8]) -> Run {
    run_with(bytes, &Limits::default())
}

pub fn run_with(bytes: &[u8], limits: &Limits) -> Run {
    let blob = Blob::new(bytes.to_vec());
    let mut out = Emitter::new(quail_pptx::ID);
    let doc = PptxExtractor::new()
        .extract(&blob, limits, &mut out)
        .expect("extract");
    Run { doc, out }
}

impl Run {
    pub fn blocks(&self) -> Vec<(u32, BlockKind, Zone, String)> {
        self.doc
            .pages
            .iter()
            .flat_map(|p| {
                p.blocks
                    .iter()
                    .map(move |b| (p.index, b.kind, b.zone, b.span.slice(&p.text).to_owned()))
            })
            .collect()
    }

    pub fn texts(&self) -> Vec<String> {
        self.blocks().into_iter().map(|b| b.3).collect()
    }

    pub fn fact(&self, key: &str) -> Option<&Value> {
        self.out
            .facts()
            .iter()
            .find(|f: &&Fact| f.key.as_str() == key)
            .map(|f| &f.value)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        match self.fact(key) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn problems(mut self) -> Vec<String> {
        self.out.take_problems()
    }

    pub fn warnings(mut self) -> Vec<String> {
        let mut all = self.out.take_problems();
        all.extend(self.out.into_parts().2);
        all
    }
}

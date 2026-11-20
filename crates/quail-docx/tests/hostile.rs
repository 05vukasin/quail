//! Regression tests for hostile inputs found in review: each one used to cost unbounded
//! memory or time, or to lose text silently. All inputs are generated here.

#![allow(clippy::cast_possible_truncation, clippy::format_push_string)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use quail_core::{Blob, BlockKind, Document, Emitter, Error, Extractor, Limits, Value};
use quail_docx::DocxExtractor;

fn texts_of(doc: &Document) -> Vec<String> {
    doc.pages
        .iter()
        .flat_map(|p| p.blocks.iter().map(|b| b.span.slice(&p.text).to_owned()))
        .collect()
}

fn run(bytes: Vec<u8>, limits: &Limits) -> (Document, Vec<String>, Emitter) {
    let (doc, out) = extract_with(bytes, limits);
    // Split off the warnings, keep the facts in an emitter for `fact()`.
    let (facts, _, warnings) = out.into_parts();
    let mut facts_only = Emitter::new(quail_docx::ID);
    for f in facts {
        facts_only.push(f.key, f.value, f.conf, f.evidence);
    }
    (doc, warnings, facts_only)
}

fn small_text(n: u64) -> Limits {
    Limits {
        max_text_bytes: n,
        ..Limits::default()
    }
}

#[test]
fn many_section_ends_stop_at_max_pages() {
    let body = "<w:sectPr/>".repeat(200_000);
    let limits = Limits {
        max_pages: 10,
        ..Limits::default()
    };
    let t = Instant::now();
    let (doc, w, out) = run(Docx::new(&body).build(), &limits);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(doc.pages.len(), 10);
    assert_eq!(fact(&out, "docx.sections"), Some(&Value::Int(10)));
    assert!(w.iter().any(|w| w.contains("max_pages")), "{w:?}");
}

#[test]
fn many_note_references_are_capped() {
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>{}",
        r#"<w:footnoteReference w:id="1"/>"#.repeat(400_000),
        p("after")
    );
    let (doc, w, _) = run(Docx::new(&body).build(), &Limits::default());
    assert!(doc.pages.iter().map(|p| p.blocks.len()).sum::<usize>() <= 250_000);
    assert!(
        w.iter().any(|w| w.contains("blocks and references")),
        "{w:?}"
    );
}

#[test]
fn repeated_level_overrides_keep_one_per_level() {
    let numbering = wpart(
        "numbering",
        &format!(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/>{}<w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num>"#,
            r#"<w:lvlOverride w:ilvl="0"></w:lvlOverride>"#.repeat(200_000)
        ),
    );
    let bytes = Docx::new(&[pn(1, 0, "a"), pn(1, 0, "b")].concat())
        .related("rId1", "numbering", "numbering.xml", &numbering)
        .build();
    let (doc, _, _) = run(bytes, &Limits::default());
    assert_eq!(texts_of(&doc), vec!["5. a", "6. b"]);
}

/// Numbering labels and the newlines joining cell paragraphs are page text too.
#[test]
fn labels_and_joins_are_charged_to_the_text_budget() {
    let numbering = wpart(
        "numbering",
        &format!(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="-999999999999999999"/><w:lvlText w:val="{}"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#,
            "%1".repeat(32)
        ),
    );
    let bytes = Docx::new(&pn(1, 0, "a").repeat(5_000))
        .related("rId1", "numbering", "numbering.xml", &numbering)
        .build();
    let (doc, w, _) = run(bytes, &small_text(1000));
    assert!(doc.full_text().len() <= 1001, "{}", doc.full_text().len());
    assert!(w.iter().any(|w| w.contains("text limit")));
    // Labels are short whatever the level text says.
    assert!(texts_of(&doc).iter().all(|t| t.len() <= 34));

    let cell = format!(
        "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>",
        p("x").repeat(5_000)
    );
    let (doc, _, _) = run(Docx::new(&cell).build(), &small_text(1000));
    assert!(doc.full_text().len() <= 1001, "{}", doc.full_text().len());
}

#[test]
fn unterminated_fields_do_not_hide_the_rest() {
    let body = format!(
        r#"{}<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:t>x</w:t></w:r></w:p>{}{}"#,
        p("before"),
        p("AFTER1"),
        p("AFTER2")
    );
    let (doc, w, _) = run(Docx::new(&body).build(), &Limits::default());
    assert_eq!(texts_of(&doc), vec!["before", "AFTER1", "AFTER2"]);
    assert!(w.iter().any(|w| w.contains("abandoned")), "{w:?}");

    let body = format!(
        r#"{}<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r></w:p>{}"#,
        p("before"),
        p("AFTER")
    );
    let (doc, w, _) = run(Docx::new(&body).build(), &Limits::default());
    assert_eq!(texts_of(&doc), vec!["before", "1", "AFTER"]);
    assert!(w.iter().any(|w| w.contains("still open")), "{w:?}");
}

#[test]
fn nesting_beyond_the_depth_caps_stays_in_sync() {
    let body = format!(
        "<w:p><w:r><w:t>OUTER-A</w:t></w:r>{}{}<w:r><w:t>OUTER-B</w:t></w:r></w:p>{}",
        "<w:p>".repeat(70),
        "</w:p>".repeat(70),
        p("NEXT")
    );
    let (doc, _, _) = run(Docx::new(&body).build(), &Limits::default());
    assert_eq!(texts_of(&doc), vec!["OUTER-AOUTER-B", "NEXT"]);

    let body = format!(
        "<w:tbl><w:tr><w:tc>{}{}{}</w:tc></w:tr></w:tbl>{}",
        p("cell"),
        "<w:tbl><w:tr><w:tc>".repeat(70),
        "</w:tc></w:tr></w:tbl>".repeat(70),
        p("after table")
    );
    let (doc, _, out) = run(Docx::new(&body).build(), &Limits::default());
    let blocks: Vec<_> = doc.pages[0].blocks.iter().map(|b| b.kind).collect();
    assert_eq!(texts_of(&doc), vec!["cell", "after table"]);
    assert_eq!(blocks[1], BlockKind::Paragraph);
    assert_eq!(fact(&out, "docx.tables"), Some(&Value::Int(71)));

    // Very deep paragraph nesting is linear, not a crash.
    let body = format!(
        "{}<w:r><w:t>deep</w:t></w:r>{}",
        "<w:p>".repeat(200_000),
        "</w:p>".repeat(200_000)
    );
    let (doc, _, _) = run(Docx::new(&body).build(), &Limits::default());
    assert_eq!(doc.full_text(), "deep");
}

#[test]
fn a_huge_text_node_is_not_buffered_whole() {
    let body = format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", "a".repeat(5 << 20));
    let (doc, w, _) = run(Docx::new(&body).build(), &small_text(1000));
    assert!(doc.full_text().len() <= 1000);
    assert!(w.iter().any(|w| w.contains("XML event is larger")), "{w:?}");
}

#[test]
fn doctype_entities_are_reported() {
    let doc = r#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY a "AAAA">]><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>x&a;y&amp;z</w:t></w:r></w:p></w:body></w:document>"#;
    let bytes = replace_part(
        &Docx::new(&p("x")).build(),
        "word/document.xml",
        doc.as_bytes(),
    );
    let (doc, w, _) = run(bytes, &Limits::default());
    assert_eq!(doc.full_text(), "xy&z");
    assert!(w.iter().any(|w| w.contains("DOCTYPE")), "{w:?}");
    assert!(w.iter().any(|w| w.contains("entities dropped")), "{w:?}");
}

#[test]
fn hidden_and_deleted_text_are_counted_and_adjacent_deletions_merged() {
    let body = r#"<w:p><w:r><w:t xml:space="preserve">Shown </w:t></w:r><w:r><w:rPr><w:vanish/></w:rPr><w:t>secret</w:t></w:r><w:r><w:rPr><w:vanish w:val="0"/></w:rPr><w:t>visible</w:t></w:r>
<w:del w:id="1" w:author="A"><w:r><w:delText>ab</w:delText></w:r></w:del><w:del w:id="2" w:author="A"><w:r><w:delText>cd</w:delText></w:r></w:del>
<w:r><w:t xml:space="preserve"> end</w:t></w:r><w:del w:id="3" w:author="A"><w:r><w:delText>zz</w:delText></w:r></w:del></w:p>
<w:p><w:pPr><w:rPr><w:vanish/></w:rPr></w:pPr><w:r><w:t>mark hidden only</w:t></w:r></w:p>"#;
    let (doc, _, out) = run(Docx::new(body).build(), &Limits::default());
    assert_eq!(
        texts_of(&doc),
        vec!["Shown visible end", "abcd", "zz", "mark hidden only"]
    );
    assert_eq!(fact(&out, "docx.hidden_text_chars"), Some(&Value::Int(6)));
    assert_eq!(fact(&out, "docx.deleted_text_chars"), Some(&Value::Int(6)));
}

#[test]
fn restarts_overrides_and_legal_numbering() {
    let numbering = wpart(
        "numbering",
        r#"<w:abstractNum w:abstractNumId="0">
  <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%1."/></w:lvl>
  <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1.%2"/><w:isLgl/></w:lvl>
  <w:lvl w:ilvl="2"><w:start w:val="1"/><w:lvlRestart w:val="0"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="(%3)"/></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>"#,
    );
    let body = [
        pn(1, 0, "one"),
        pn(1, 1, "legal"),
        pn(1, 2, "x"),
        pn(1, 0, "two"),
        pn(1, 2, "never restarts"),
        pn(2, 0, "restarted"),
        pn(2, 0, "again"),
        pn(1, 0, "shared counter"),
    ]
    .concat();
    let bytes = Docx::new(&body)
        .related("rId1", "numbering", "numbering.xml", &numbering)
        .build();
    let (doc, _, _) = run(bytes, &Limits::default());
    assert_eq!(
        texts_of(&doc),
        vec![
            "I. one",
            "1.1 legal",
            "(a) x",
            "II. two",
            "(b) never restarts",
            "I. restarted",
            "II. again",
            "III. shared counter",
        ]
    );
}

#[test]
fn grid_span_and_vertical_merge() {
    let body = r#"<w:tbl>
<w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>C</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>D</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr>
</w:tbl>"#;
    let (doc, _, _) = run(Docx::new(body).build(), &Limits::default());
    let tc = |row, col| BlockKind::TableCell { table: 0, row, col };
    let got: Vec<_> = doc.pages[0]
        .blocks
        .iter()
        .map(|b| (b.span.slice(&doc.pages[0].text).to_owned(), b.kind))
        .collect();
    assert_eq!(
        got,
        vec![
            ("A".to_owned(), tc(0, 0)),
            ("B".to_owned(), tc(0, 2)),
            ("C".to_owned(), tc(1, 0)),
            ("D".to_owned(), tc(1, 1)),
        ]
    );
}

#[test]
fn exact_duplicate_entries_are_counted() {
    let doc = |t: &str| wpart("document", &format!("<w:body>{}</w:body>", p(t)));
    let ct = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#;
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let first = doc("FIRST");
    let second = doc("SECOND");
    let bytes = raw_zip(&[
        ("[Content_Types].xml", ct.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("word/document.xml", first.as_bytes()),
        ("word/document.xml", second.as_bytes()),
    ]);
    let (doc, w, out) = run(bytes, &Limits::default());
    assert_eq!(doc.full_text(), "SECOND");
    assert_eq!(fact(&out, "docx.duplicate_entries"), Some(&Value::Int(1)));
    assert!(
        w.iter().any(|w| w.contains("repeat an earlier name")),
        "{w:?}"
    );
}

#[test]
fn forged_directory_counts_are_refused_before_parsing() {
    let base = Docx::new(&p("x")).build();
    let eocd = base
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    // A plain EOCD claiming 65 535 entries.
    let mut forged = base.clone();
    forged[eocd + 8..eocd + 10].copy_from_slice(&0xFFFFu16.to_le_bytes());
    forged[eocd + 10..eocd + 12].copy_from_slice(&0xFFFFu16.to_le_bytes());
    let mut out = Emitter::new(quail_docx::ID);
    let r = DocxExtractor.extract(&Blob::from(forged), &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::LimitExceeded(_))), "{r:?}");

    // A ZIP64 record claiming a billion entries.
    let mut z64 = base[..eocd].to_vec();
    let rec_at = z64.len() as u64;
    z64.extend_from_slice(&[0x50, 0x4b, 0x06, 0x06]);
    z64.extend_from_slice(&44u64.to_le_bytes());
    z64.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z64.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    z64.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    z64.extend_from_slice(&0u64.to_le_bytes());
    z64.extend_from_slice(&0u64.to_le_bytes());
    z64.extend_from_slice(&[0x50, 0x4b, 0x06, 0x07, 0, 0, 0, 0]);
    z64.extend_from_slice(&rec_at.to_le_bytes());
    z64.extend_from_slice(&1u32.to_le_bytes());
    z64.extend_from_slice(&base[eocd..]);
    let r = DocxExtractor.extract(&Blob::from(z64), &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::LimitExceeded(_))), "{r:?}");
}

#[test]
fn deadline_applies_inside_a_skipped_subtree() {
    let body = format!(
        "<w:p><w:r><mc:AlternateContent><mc:Fallback>{}</mc:Fallback></mc:AlternateContent></w:r></w:p>",
        "<w:p/>".repeat(5_000_000)
    );
    let bytes = Docx::new(&body).build();
    let mut out =
        Emitter::with_deadline(quail_docx::ID, Instant::now() + Duration::from_millis(30));
    let t = Instant::now();
    let _ = DocxExtractor.extract(&Blob::from(bytes), &Limits::default(), &mut out);
    let elapsed = t.elapsed();
    let w = out.into_parts().2;
    assert!(w.iter().any(|w| w.contains("deadline")), "{w:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

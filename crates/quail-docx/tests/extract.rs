//! End-to-end tests over DOCX files generated in code.

#![allow(
    clippy::format_push_string,
    clippy::format_in_format_args,
    clippy::useless_format,
    clippy::cast_possible_truncation,
    clippy::too_many_lines
)]

mod common;

use common::*;
use quail_core::{
    Blob, BlockKind, Document, Emitter, Error, Extractor, Limits, Page, PageKind, Value, Zone,
};
use quail_docx::DocxExtractor;

fn blocks(page: &Page) -> Vec<(String, BlockKind, Zone)> {
    page.blocks
        .iter()
        .map(|b| (b.span.slice(&page.text).to_owned(), b.kind, b.zone))
        .collect()
}

fn all_blocks(doc: &Document) -> Vec<(String, BlockKind, Zone)> {
    doc.pages.iter().flat_map(blocks).collect()
}

const STYLES: &str = r#"
<w:docDefaults/>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>
  <w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Clause"><w:name w:val="My Clause"/><w:basedOn w:val="Normal"/>
  <w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Sub"><w:name w:val="Sub"/><w:basedOn w:val="Clause"/></w:style>
<w:style w:type="table" w:styleId="Grid"><w:name w:val="Table Grid"/>
  <w:tblStylePr w:type="firstRow"><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:tblStylePr></w:style>
"#;

const NUMBERING: &str = r#"
<w:abstractNum w:abstractNumId="0">
  <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1"/></w:lvl>
  <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1.%2"/></w:lvl>
  <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="(%3)"/></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="1">
  <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
"#;

#[test]
fn headings_lists_and_numbering() {
    let body = [
        ps("Heading1", "Definitions"),
        pn(1, 1, "First clause"),
        pn(1, 2, "sub item"),
        pn(1, 2, "another"),
        pn(1, 1, "Second clause"),
        ps("Heading1", "Payment"),
        pn(1, 1, "Pay on time"),
        pn(2, 0, "bullet"),
        ps("Sub", "Outline via basedOn"),
        r#"<w:p><w:pPr><w:outlineLvl w:val="2"/></w:pPr><w:r><w:t>Direct outline</w:t></w:r></w:p>"#
            .to_owned(),
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:numPr><w:numId w:val="0"/></w:numPr></w:pPr><w:r><w:t>Unnumbered heading</w:t></w:r></w:p>"#.to_owned(),
        p("plain"),
        p(""),
    ]
    .concat();
    let bytes = Docx::new(&body)
        .related("rId1", "styles", "styles.xml", &wpart("styles", STYLES))
        .related(
            "rId2",
            "numbering",
            "numbering.xml",
            &wpart("numbering", NUMBERING),
        )
        .build();
    let (doc, out) = extract(bytes);
    assert_eq!(doc.pages.len(), 1);
    assert_eq!(doc.pages[0].kind, PageKind::Logical);
    let b = all_blocks(&doc);
    let h1 = BlockKind::Heading { level: 1 };
    let expected = vec![
        ("1 Definitions", h1, Zone::Title),
        ("1.1 First clause", BlockKind::ListItem, Zone::Body),
        ("(a) sub item", BlockKind::ListItem, Zone::Body),
        ("(b) another", BlockKind::ListItem, Zone::Body),
        ("1.2 Second clause", BlockKind::ListItem, Zone::Body),
        ("2 Payment", h1, Zone::Title),
        ("2.1 Pay on time", BlockKind::ListItem, Zone::Body),
        ("- bullet", BlockKind::ListItem, Zone::Body),
        (
            "Outline via basedOn",
            BlockKind::Heading { level: 2 },
            Zone::Title,
        ),
        (
            "Direct outline",
            BlockKind::Heading { level: 3 },
            Zone::Title,
        ),
        ("Unnumbered heading", h1, Zone::Title),
        ("plain", BlockKind::Paragraph, Zone::Body),
    ];
    let got: Vec<_> = b.iter().map(|(t, k, z)| (t.as_str(), *k, *z)).collect();
    assert_eq!(got, expected);
    assert_eq!(fact(&out, "docx.paragraphs"), Some(&Value::Int(13)));
    assert_eq!(fact(&out, "docx.sections"), Some(&Value::Int(1)));
    // Text is blocks joined by newlines.
    assert!(
        doc.pages[0]
            .text
            .starts_with("1 Definitions\n1.1 First clause\n")
    );
}

#[test]
fn tables_with_spans_and_nesting() {
    let cell = |inner: &str| format!("<w:tc>{inner}</w:tc>");
    let nested = format!(
        "<w:tbl><w:tr>{}{}</w:tr></w:tbl>",
        cell(&p("inner a")),
        cell(&p("inner b"))
    );
    let body = format!(
        "{}<w:tbl><w:tblGrid/><w:tr>{}{}</w:tr><w:tr><w:trPr><w:gridBefore w:val=\"1\"/></w:trPr>{}</w:tr></w:tbl>{}<w:tbl><w:tr>{}</w:tr></w:tbl>",
        p("before"),
        format!(
            "<w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr>{}{}</w:tc>",
            p("wide"),
            p("second para")
        ),
        cell(&p("right")),
        cell(&format!("{}{nested}", p("outer"))),
        p("after"),
        cell(&p("t2")),
    );
    let (doc, out) = extract(Docx::new(&body).build());
    let got = all_blocks(&doc);
    let tc = |table, row, col| BlockKind::TableCell { table, row, col };
    let expected = vec![
        ("before".to_owned(), BlockKind::Paragraph, Zone::Body),
        ("wide\nsecond para".to_owned(), tc(0, 0, 0), Zone::Table),
        ("right".to_owned(), tc(0, 0, 2), Zone::Table),
        (
            "outer\ninner a\ninner b".to_owned(),
            tc(0, 1, 1),
            Zone::Table,
        ),
        ("after".to_owned(), BlockKind::Paragraph, Zone::Body),
        ("t2".to_owned(), tc(1, 0, 0), Zone::Table),
    ];
    assert_eq!(got, expected);
    assert_eq!(fact(&out, "docx.tables"), Some(&Value::Int(3)));
    assert_eq!(fact(&out, "docx.nested_tables"), Some(&Value::Int(1)));
}

fn sect(header: Option<&str>, footer: Option<&str>) -> String {
    let mut s = String::from("<w:sectPr>");
    if let Some(h) = header {
        s.push_str(&format!(
            r#"<w:headerReference w:type="default" r:id="{h}"/>"#
        ));
    }
    if let Some(f) = footer {
        s.push_str(&format!(
            r#"<w:footerReference w:type="default" r:id="{f}"/>"#
        ));
    }
    s.push_str(r#"<w:pgSz w:w="12240" w:h="15840"/></w:sectPr>"#);
    s
}

#[test]
fn sections_headers_footers_deduped() {
    let body = format!(
        "{}<w:p><w:pPr>{}</w:pPr><w:r><w:t>end of one</w:t></w:r></w:p>{}{}{}",
        p("section one"),
        sect(Some("rIdH1"), Some("rIdF1")),
        p("section two"),
        p("more"),
        sect(Some("rIdH1"), Some("rIdF2")),
    );
    let header = wpart("hdr", &p("ACME Letterhead"));
    let footer = wpart("ftr", &p("Page footer"));
    let bytes = Docx::new(&body)
        .related("rIdH1", "header", "header1.xml", &header)
        .related("rIdF1", "footer", "footer1.xml", &footer)
        // A different part with identical text: deduped too.
        .related("rIdF2", "footer", "footer2.xml", &footer)
        .build();
    let (doc, out) = extract(bytes);
    assert_eq!(doc.pages.len(), 2);
    assert_eq!(
        blocks(&doc.pages[0]),
        vec![
            ("ACME Letterhead".into(), BlockKind::Paragraph, Zone::Header),
            ("section one".into(), BlockKind::Paragraph, Zone::Body),
            ("end of one".into(), BlockKind::Paragraph, Zone::Body),
            ("Page footer".into(), BlockKind::Paragraph, Zone::Footer),
        ]
    );
    assert_eq!(
        blocks(&doc.pages[1]),
        vec![
            ("section two".into(), BlockKind::Paragraph, Zone::Body),
            ("more".into(), BlockKind::Paragraph, Zone::Body),
        ]
    );
    assert_eq!(doc.pages[1].index, 1);
    assert_eq!(fact(&out, "docx.sections"), Some(&Value::Int(2)));
    assert_eq!(fact(&out, "docx.headers"), Some(&Value::Int(1)));
    assert_eq!(fact(&out, "docx.footers"), Some(&Value::Int(2)));
}

#[test]
fn tracked_changes_accepted_view() {
    let body = r#"
<w:p><w:r><w:t xml:space="preserve">The price is </w:t></w:r>
<w:del w:id="1" w:author="Alice" w:date="2024-01-01T00:00:00Z"><w:r><w:delText>100</w:delText></w:r></w:del>
<w:ins w:id="2" w:author="Bob" w:date="2024-01-02T00:00:00Z"><w:r><w:t>200</w:t></w:r></w:ins>
<w:r><w:t xml:space="preserve"> euro</w:t></w:r>
<w:moveFrom w:id="3" w:author="alice"><w:r><w:t xml:space="preserve"> moved</w:t></w:r></w:moveFrom>
<w:r><w:rPr><w:b/><w:rPrChange w:id="4" w:author="Carol"><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr><w:t>.</w:t></w:r>
</w:p>
<w:p><w:pPr><w:rPr><w:ins w:id="5" w:author="Bob"/></w:rPr></w:pPr><w:r><w:t>next</w:t></w:r></w:p>"#;
    let (doc, out) = extract(Docx::new(body).build());
    let got = all_blocks(&doc);
    assert_eq!(
        got,
        vec![
            (
                "The price is 200 euro.".into(),
                BlockKind::Paragraph,
                Zone::Body
            ),
            ("100".into(), BlockKind::Annotation, Zone::Body),
            ("moved".into(), BlockKind::Annotation, Zone::Body),
            ("next".into(), BlockKind::Paragraph, Zone::Body),
        ]
    );
    assert_eq!(fact(&out, "docx.tracked_insertions"), Some(&Value::Int(2)));
    assert_eq!(fact(&out, "docx.tracked_deletions"), Some(&Value::Int(2)));
    assert_eq!(
        fact(&out, "docx.tracked_format_changes"),
        Some(&Value::Int(1))
    );
    assert_eq!(
        texts(&out, "docx.revision_authors"),
        vec!["Alice", "Bob", "Carol"]
    );
}

#[test]
fn comments_and_notes() {
    let body = format!(
        r#"<w:p><w:commentRangeStart w:id="0"/><w:r><w:t>Clause text</w:t></w:r><w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>{}<w:p><w:r><w:t>Body two</w:t></w:r><w:r><w:endnoteReference w:id="2"/></w:r></w:p>"#,
        p("Plain")
    );
    let comments = wpart(
        "comments",
        &format!(
            r#"<w:comment w:id="0" w:author="Dana" w:initials="D">{}</w:comment><w:comment w:id="1" w:author="dana">{}</w:comment><w:comment w:id="2" w:author="Eve">{}</w:comment>"#,
            p("Please check"),
            p("Orphan"),
            p("Another")
        ),
    );
    let footnotes = wpart(
        "footnotes",
        &format!(
            r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="0" w:type="continuationSeparator"><w:p><w:r><w:t>sep</w:t></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> See annex.</w:t></w:r></w:p></w:footnote>"#
        ),
    );
    let endnotes = wpart(
        "endnotes",
        &format!(r#"<w:endnote w:id="2">{}</w:endnote>"#, p("End note")),
    );
    let bytes = Docx::new(&body)
        .related("rId1", "comments", "comments.xml", &comments)
        .related("rId2", "footnotes", "footnotes.xml", &footnotes)
        .related("rId3", "endnotes", "endnotes.xml", &endnotes)
        .build();
    let (doc, out) = extract(bytes);
    assert_eq!(
        all_blocks(&doc),
        vec![
            ("Clause text".into(), BlockKind::Paragraph, Zone::Body),
            ("Please check".into(), BlockKind::Annotation, Zone::Body),
            ("Plain".into(), BlockKind::Paragraph, Zone::Body),
            ("Body two".into(), BlockKind::Paragraph, Zone::Body),
            ("See annex.".into(), BlockKind::Note, Zone::Body),
            ("End note".into(), BlockKind::Note, Zone::Body),
        ]
    );
    assert_eq!(fact(&out, "docx.comments"), Some(&Value::Int(3)));
    assert_eq!(texts(&out, "docx.comment_authors"), vec!["Dana", "Eve"]);
    assert_eq!(fact(&out, "docx.footnotes"), Some(&Value::Int(1)));
    assert_eq!(fact(&out, "docx.endnotes"), Some(&Value::Int(1)));
}

#[test]
fn rsids_template_and_settings() {
    let settings = wpart(
        "settings",
        r#"<w:trackRevisions/><w:documentProtection w:edit="readOnly" w:enforcement="1"/><w:attachedTemplate r:id="rId1"/>
<w:rsids><w:rsidRoot w:val="00A1B2C3"/><w:rsid w:val="00000001"/><w:rsid w:val="00A1B2C3"/><w:rsid w:val="FFFFFFFF"/><w:rsid w:val="123"/></w:rsids>"#,
    );
    let settings_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/attachedTemplate" Target="file:///C:/Templates/Contract.dotm" TargetMode="External"/></Relationships>"#;
    let bytes = Docx::new(&p("x"))
        .related("rId9", "settings", "settings.xml", &settings)
        .part("word/_rels/settings.xml.rels", settings_rels)
        .build();
    let (_, out) = extract(bytes);
    assert_eq!(
        fact(&out, "docx.rsids"),
        Some(&Value::List(vec![
            Value::U64(1),
            Value::U64(0x00A1_B2C3),
            Value::U64(0xFFFF_FFFF)
        ]))
    );
    assert_eq!(fact(&out, "docx.rsid_root"), Some(&Value::U64(0x00A1_B2C3)));
    assert_eq!(fact(&out, "docx.rsid_count"), Some(&Value::Int(3)));
    assert_eq!(
        fact(&out, "docx.attached_template"),
        Some(&Value::text("file:///C:/Templates/Contract.dotm"))
    );
    assert_eq!(fact(&out, "docx.track_revisions"), Some(&Value::Bool(true)));
    assert_eq!(
        fact(&out, "docx.protection"),
        Some(&Value::text("readOnly"))
    );
    assert!(warnings(out).iter().any(|w| w.contains("rsid")));
}

#[test]
fn document_properties() {
    let core = r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:title>Master  Services &amp; Agreement</dc:title><dc:subject>Subj</dc:subject><dc:creator>Writer One</dc:creator><cp:keywords>k1, k2</cp:keywords>
<cp:lastModifiedBy>Writer Two</cp:lastModifiedBy><cp:revision>17</cp:revision><cp:category>Legal</cp:category>
<dcterms:created xsi:type="dcterms:W3CDTF">2021-05-04T08:00:00Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2022-01-02T03:04:05Z</dcterms:modified></cp:coreProperties>"#;
    let app = r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Template>Normal.dotm</Template><TotalTime>42</TotalTime><Pages>3</Pages><Words>512</Words><Application>Microsoft Office Word</Application><Company>Example Corp</Company><AppVersion>16.0000</AppVersion><TitlesOfParts><vt:vector size="1" baseType="lpstr"><vt:lpstr>ignored</vt:lpstr></vt:vector></TitlesOfParts></Properties>"#;
    let custom = r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="DocNumber"><vt:lpwstr>A-17</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="3" name="Approved"><vt:bool>true</vt:bool></property></Properties>"#;
    let bytes = Docx::new(&p("x"))
        .part("docProps/core.xml", core)
        .part("docProps/app.xml", app)
        .part("docProps/custom.xml", custom)
        .build();
    let (_, out) = extract(bytes);
    let t = |k: &str| fact(&out, k).cloned();
    assert_eq!(
        t("docx.title"),
        Some(Value::text("Master Services & Agreement"))
    );
    assert_eq!(t("docx.subject"), Some(Value::text("Subj")));
    assert_eq!(t("docx.creator"), Some(Value::text("Writer One")));
    assert_eq!(t("docx.last_modified_by"), Some(Value::text("Writer Two")));
    assert_eq!(t("docx.revision"), Some(Value::Int(17)));
    assert_eq!(t("docx.created"), Some(Value::text("2021-05-04T08:00:00Z")));
    assert_eq!(
        t("docx.modified"),
        Some(Value::text("2022-01-02T03:04:05Z"))
    );
    assert_eq!(t("docx.keywords"), Some(Value::text("k1, k2")));
    assert_eq!(t("docx.category"), Some(Value::text("Legal")));
    assert_eq!(
        t("docx.application"),
        Some(Value::text("Microsoft Office Word"))
    );
    assert_eq!(t("docx.app_version"), Some(Value::text("16.0000")));
    assert_eq!(t("docx.company"), Some(Value::text("Example Corp")));
    assert_eq!(t("docx.template"), Some(Value::text("Normal.dotm")));
    assert_eq!(t("docx.total_time"), Some(Value::Int(42)));
    assert_eq!(t("docx.pages"), Some(Value::Int(3)));
    assert_eq!(t("docx.words"), Some(Value::Int(512)));
    assert_eq!(t("docx.custom_property_count"), Some(Value::Int(2)));
    assert_eq!(
        texts(&out, "docx.custom_property_names"),
        vec!["DocNumber", "Approved"]
    );
    assert_eq!(
        texts(&out, "docx.custom_properties"),
        vec!["DocNumber=A-17", "Approved=true"]
    );
    assert_eq!(t("docx.has_macros"), Some(Value::Bool(false)));
}

#[test]
fn fields_hyperlinks_and_special_runs() {
    let body = r#"
<w:p><w:r><w:t xml:space="preserve">Visit </w:t></w:r>
<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK "https://Docs.Example.com/path" \o "tip" </w:instrText></w:r>
<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>our site</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>
<w:r><w:t xml:space="preserve"> or page </w:t></w:r>
<w:fldSimple w:instr=" PAGE "><w:r><w:t>7</w:t></w:r></w:fldSimple>
<w:r><w:tab/><w:t>a</w:t><w:br/><w:t>b</w:t><w:noBreakHyphen/><w:t>c</w:t></w:r>
<w:hyperlink r:id="rIdL"><w:r><w:t xml:space="preserve"> link</w:t></w:r></w:hyperlink>
<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> XE "index" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>
</w:p>
<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t>tabs &amp; &#x41;&#66;</w:t></w:r></w:p>
<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><w:txbxContent><w:p><w:r><w:t>In a box</w:t></w:r></w:p></w:txbxContent></w:drawing></mc:Choice><mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>In a box</w:t></w:r></w:p></w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>Anchor para</w:t></w:r></w:p>"#;
    let bytes = Docx::new(body)
        .external("rIdL", "hyperlink", "mailto:someone@Mail.Example.org")
        .external("rIdM", "hyperlink", "http://other.example.net:8080/x")
        .external("rIdN", "hyperlink", "https://docs.example.com/again")
        .build();
    let (doc, out) = extract(bytes);
    assert_eq!(
        all_blocks(&doc)
            .into_iter()
            .map(|b| b.0)
            .collect::<Vec<_>>(),
        vec![
            "Visit our site or page 7\ta\nb-c link",
            "tabs & AB",
            "In a box",
            "Anchor para"
        ]
    );
    assert_eq!(
        texts(&out, "docx.hyperlink_domains"),
        vec!["mail.example.org", "other.example.net", "docs.example.com"]
    );
}

#[test]
fn macros_media_and_content_type() {
    let bytes = Docx::new(&p("x"))
        .content_type(
            "/word/document.xml",
            "application/vnd.ms-word.document.macroEnabled.main+xml",
        )
        .part("word/vbaProject.bin", vec![0u8; 16])
        .part("word/media/image1.png", vec![1u8; 8])
        .part("word/media/image2.jpeg", vec![1u8; 8])
        .part("word/embeddings/oleObject1.bin", vec![2u8; 8])
        .build();
    let (_, out) = extract(bytes);
    assert_eq!(fact(&out, "docx.has_macros"), Some(&Value::Bool(true)));
    assert_eq!(fact(&out, "docx.images"), Some(&Value::Int(2)));
    assert_eq!(fact(&out, "docx.embedded_objects"), Some(&Value::Int(1)));
    assert_eq!(
        fact(&out, "docx.main_content_type"),
        Some(&Value::text(
            "application/vnd.ms-word.document.macroEnabled.main+xml"
        ))
    );
}

#[test]
fn clm_placeholder_parties() {
    let form = r#"<?xml version="1.0"?><sqph:contractplaceholders xmlns:sqph="urn:x-clm">
<sqph:FirstParty><sqph:PlaceholderLegalEntityName>[[ Client Name ]]</sqph:PlaceholderLegalEntityName></sqph:FirstParty>
<sqph:FirstParty><sqph:PlaceholderLegalEntityName> Client  Holding Ltd </sqph:PlaceholderLegalEntityName></sqph:FirstParty>
<sqph:SecondParty><sqph:PlaceholderLegalEntityName>Supplier GmbH</sqph:PlaceholderLegalEntityName><sqph:PlaceholderLegalEntityContractEmail>x@y.z</sqph:PlaceholderLegalEntityContractEmail></sqph:SecondParty>
</sqph:contractplaceholders>"#;
    let other = r#"<?xml version="1.0"?><root><FirstParty><PlaceholderLegalEntityName>Not a form</PlaceholderLegalEntityName></FirstParty></root>"#;
    let bytes = Docx::new(&p("x"))
        .part("customXml/item1.xml", other)
        .part("customXml/item2.xml", form)
        .build();
    let (_, out) = extract(bytes);
    assert_eq!(
        fact(&out, "docx.clm_first_party"),
        Some(&Value::text("Client Holding Ltd"))
    );
    assert_eq!(
        fact(&out, "docx.clm_second_party"),
        Some(&Value::text("Supplier GmbH"))
    );
}

/// customXml written by third-party tools: Latin-1 bytes and control characters.
#[test]
fn clm_form_with_invalid_bytes() {
    let mut form = br#"<?xml version="1.0"?><sqph:contractplaceholders xmlns:sqph="urn:x"><sqph:SecondParty><sqph:PlaceholderLegalEntityName>Caf"#.to_vec();
    form.extend_from_slice(&[0xE9, 0x01]);
    form.extend_from_slice(
        b" AG</sqph:PlaceholderLegalEntityName></sqph:SecondParty></sqph:contractplaceholders>",
    );
    let bytes = Docx::new(&p("x")).part("customXml/item1.xml", form).build();
    let (_, out) = extract(bytes);
    assert_eq!(
        fact(&out, "docx.clm_second_party"),
        Some(&Value::text("Caf\u{e9} AG"))
    );
    assert!(warnings(out).is_empty());
}

/// A main part that inflates far past the budget: reading stops at the budget.
#[test]
fn zip_bomb_stops_at_budget() {
    let para = p("all work and no play makes a dull document ");
    let body = para.repeat(200_000); // ~13 MB of XML, compresses to a few KB
    let bytes = Docx::new(&body).build();
    assert!(bytes.len() < 200_000, "test input should compress well");
    let limits = Limits {
        max_decompressed: 1 << 20,
        ..Limits::default()
    };
    let (doc, out) = extract_with(bytes, &limits);
    let text: usize = doc.pages.iter().map(|p| p.text.len()).sum();
    assert!(text > 0 && text < 1 << 20, "partial text, got {text}");
    let w = warnings(out);
    assert!(
        w.iter().any(|w| w.contains("decompression budget")),
        "{w:?}"
    );
}

/// Same bomb, but the zip lies about the inflated size: the real count still stops it.
#[test]
fn zip_bomb_with_lying_sizes() {
    let body = p("x ".repeat(1000).as_str()).repeat(5_000);
    let mut bytes = Docx::new(&body).build();
    let declared = lie_about_size(&mut bytes, "word/document.xml", 100);
    assert!(declared, "patched the size fields");
    let limits = Limits {
        max_decompressed: 1 << 20,
        ..Limits::default()
    };
    let mut out = Emitter::new(quail_docx::ID);
    // Either a partial document or a zip error: never more than the budget inflated.
    if let Ok(doc) = DocxExtractor.extract(&Blob::from(bytes), &limits, &mut out) {
        let text: usize = doc.pages.iter().map(|p| p.text.len()).sum();
        assert!(text <= 1 << 20);
        let w = warnings(out);
        eprintln!("{w:?} text={text}");
        assert!(!w.is_empty());
    }
}

/// Rewrites the uncompressed size of `name` in its local and central headers.
fn lie_about_size(zip: &mut [u8], name: &str, size: u32) -> bool {
    let mut patched = false;
    let n = name.as_bytes();
    let mut i = 0;
    while i + 46 <= zip.len() {
        let sig = &zip[i..i + 4];
        if sig == [0x50, 0x4b, 0x03, 0x04] {
            let nlen = u16::from_le_bytes([zip[i + 26], zip[i + 27]]) as usize;
            if zip.get(i + 30..i + 30 + nlen) == Some(n) {
                zip[i + 22..i + 26].copy_from_slice(&size.to_le_bytes());
                patched = true;
            }
        } else if sig == [0x50, 0x4b, 0x01, 0x02] {
            let nlen = u16::from_le_bytes([zip[i + 28], zip[i + 29]]) as usize;
            if zip.get(i + 46..i + 46 + nlen) == Some(n) {
                zip[i + 24..i + 28].copy_from_slice(&size.to_le_bytes());
                patched = true;
            }
        }
        i += 1;
    }
    patched
}

#[test]
fn zip_entry_with_path_trick_is_ignored() {
    let bytes = Docx::new(&p("safe"))
        .part("../evil.xml", "<x/>")
        .part("word/../../evil2.xml", "<x/>")
        .build();
    let (doc, out) = extract(bytes);
    assert_eq!(doc.pages[0].text, "safe");
    let w = warnings(out);
    assert!(w.iter().any(|w| w.contains("unsafe")), "{w:?}");
}

#[test]
fn malformed_xml_gives_partial_document() {
    let mut bytes = Docx::new(&format!("{}{}", p("first"), p("second"))).build();
    // Replace the main part with truncated XML.
    let doc = wpart(
        "document",
        &format!("<w:body>{}{}", p("kept"), "<w:p><w:r><w:t>lost"),
    );
    let truncated = &doc[..doc.len() - 30];
    bytes = rebuild_with(&bytes, "word/document.xml", truncated.as_bytes());
    let (doc, out) = extract(bytes);
    assert_eq!(doc.pages[0].blocks.len(), 1);
    assert!(doc.pages[0].text.starts_with("kept"));
    let _ = out;

    let broken = wpart(
        "document",
        "<w:body><w:p><w:r><w:t>ok</w:t></w:r></w:p><w:p><!-- never closed </w:body>",
    );
    let bytes = rebuild_with(
        &Docx::new(&p("x")).build(),
        "word/document.xml",
        broken.as_bytes(),
    );
    let (doc, out) = extract(bytes);
    assert_eq!(doc.pages[0].text, "ok");
    assert!(warnings(out).iter().any(|w| w.contains("malformed")));
}

fn rebuild_with(zip: &[u8], name: &str, data: &[u8]) -> Vec<u8> {
    use std::io::{Cursor, Read, Write};
    let mut a = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..a.len() {
        let mut f = a.by_index(i).unwrap();
        let n = f.name().unwrap().into_owned();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        w.start_file(n.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(if n == name { data } else { &buf }).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn not_a_zip_and_encrypted() {
    let mut out = Emitter::new(quail_docx::ID);
    let r = DocxExtractor.extract(&Blob::from(b"hello".to_vec()), &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::Malformed { .. })));
    let mut cfb = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    cfb.resize(4096, 0);
    let r = DocxExtractor.extract(&Blob::from(cfb), &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::Encrypted)));
    // A zip without a main document.
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("a.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    let z = w.finish().unwrap().into_inner();
    let r = DocxExtractor.extract(&Blob::from(z), &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::Malformed { .. })));
}

#[test]
fn page_and_text_limits() {
    let s = sect(None, None);
    let body = format!(
        "<w:p><w:pPr>{s}</w:pPr><w:r><w:t>one</w:t></w:r></w:p><w:p><w:pPr>{s}</w:pPr><w:r><w:t>two</w:t></w:r></w:p>{}{s}",
        p("three")
    );
    let limits = Limits {
        max_pages: 2,
        ..Limits::default()
    };
    let (doc, out) = extract_with(Docx::new(&body).build(), &limits);
    assert_eq!(doc.pages.len(), 2);
    assert_eq!(fact(&out, "docx.sections"), Some(&Value::Int(3)));
    assert!(warnings(out).iter().any(|w| w.contains("max_pages")));

    let limits = Limits {
        max_text_bytes: 10,
        ..Limits::default()
    };
    let (doc, out) = extract_with(
        Docx::new(&format!("{}{}", p("0123456789abcdef"), p("more"))).build(),
        &limits,
    );
    assert_eq!(doc.full_text(), "0123456789");
    assert!(warnings(out).iter().any(|w| w.contains("text limit")));
}

#[test]
fn deadline_in_the_past_gives_partial() {
    let bytes = Docx::new(&p("x")).build();
    let mut out = Emitter::with_deadline(quail_docx::ID, std::time::Instant::now());
    let doc = DocxExtractor
        .extract(&Blob::from(bytes), &Limits::default(), &mut out)
        .unwrap();
    assert!(doc.full_text().is_empty());
    assert!(warnings(out).iter().any(|w| w.contains("deadline")));
}

#[test]
fn ids_and_formats() {
    assert_eq!(DocxExtractor.id().to_string(), "docx.extract@1");
    assert_eq!(DocxExtractor.formats(), &[quail_core::Format::Docx]);
}

/// No input may panic: mutate a real-looking DOCX byte by byte.
#[test]
fn mutated_inputs_never_panic() {
    let body = [
        ps("Heading1", "Title"),
        pn(1, 1, "item"),
        "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>".to_owned(),
    ]
    .concat();
    let mut d = Docx::new(&body)
        .related("rId1", "styles", "styles.xml", &wpart("styles", STYLES))
        .related(
            "rId2",
            "numbering",
            "numbering.xml",
            &wpart("numbering", NUMBERING),
        );
    d.store = true; // stored entries: mutations hit the XML itself
    let base = d.build();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..1500 {
        let mut b = base.clone();
        for _ in 0..4 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let i = (seed as usize) % b.len();
            b[i] = (seed >> 32) as u8;
        }
        let mut out = Emitter::new(quail_docx::ID);
        let _ = DocxExtractor.extract(&Blob::from(b), &Limits::default(), &mut out);
    }
}

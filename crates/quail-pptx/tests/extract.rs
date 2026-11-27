//! What the PPTX extractor makes of presentations built here.

mod common;

use common::{Pptx, Slide, notes, paras, run, slide, sp};
use quail_core::{BlockKind, PageKind, TextSource, Value, Zone};

#[test]
fn reading_order_titles_bullets() {
    // File order is scrambled: a text box at the bottom, the body placeholder (position
    // from the layout), a text box at the top right, the title (position from the layout).
    let s = slide(
        &(sp(
            5,
            None,
            Some((457_200, 5_000_000)),
            &paras(&["Source: internal data"]),
        ) + &sp(
            3,
            Some(("", Some(1))),
            None,
            &paras(&["Revenue up 12%", "Costs flat", "~Plain line"]),
        ) + &sp(
            4,
            None,
            Some((6_000_000, 1_000_000)),
            &paras(&["Draft", "-Boxed bullet"]),
        ) + &sp(
            2,
            Some(("title", None)),
            None,
            &paras(&["Quarterly", "Review"]),
        )),
    );
    let r = run(&Pptx::of(&[&s]).build());
    let got: Vec<(BlockKind, Zone, String)> =
        r.blocks().into_iter().map(|b| (b.1, b.2, b.3)).collect();
    let p = |k, z, t: &str| (k, z, t.to_owned());
    assert_eq!(
        got,
        vec![
            p(
                BlockKind::Heading { level: 1 },
                Zone::Title,
                "Quarterly\nReview"
            ),
            p(BlockKind::Paragraph, Zone::Body, "Draft"),
            p(BlockKind::ListItem, Zone::Body, "Boxed bullet"),
            p(BlockKind::ListItem, Zone::Body, "Revenue up 12%"),
            p(BlockKind::ListItem, Zone::Body, "Costs flat"),
            p(BlockKind::Paragraph, Zone::Body, "Plain line"),
            p(BlockKind::Paragraph, Zone::Body, "Source: internal data"),
        ]
    );
    let page = &r.doc.pages[0];
    assert_eq!(page.kind, PageKind::Logical);
    assert_eq!(page.text_source, TextSource::Native);
}

#[test]
#[allow(clippy::many_single_char_names)] // slides a, b, c
fn one_page_per_slide_in_list_order_with_notes() {
    let a = slide(&sp(2, Some(("ctrTitle", None)), None, &paras(&["Opening"])));
    let b = slide(
        &(sp(2, Some(("title", None)), None, &paras(&["Second"]))
            + &sp(
                3,
                Some(("subTitle", Some(1))),
                None,
                &paras(&["the details"]),
            )),
    );
    let c = slide(&sp(2, None, Some((0, 0)), &paras(&["Third"])));
    let mut p = Pptx::new(vec![
        Slide {
            xml: a,
            notes: Some(notes("Say hello first.")),
        },
        Slide {
            xml: b,
            notes: None,
        },
        Slide {
            xml: c,
            notes: Some(notes("Close with the price.")),
        },
    ]);
    // The list order wins over part names.
    p.order = Some(vec![3, 1, 2]);
    let r = run(&p.build());
    let got: Vec<(u32, BlockKind, String)> =
        r.blocks().into_iter().map(|b| (b.0, b.1, b.3)).collect();
    assert_eq!(
        got,
        vec![
            (0, BlockKind::Paragraph, "Third".to_owned()),
            (0, BlockKind::Note, "Close with the price.".to_owned()),
            (1, BlockKind::Heading { level: 1 }, "Opening".to_owned()),
            (1, BlockKind::Note, "Say hello first.".to_owned()),
            (2, BlockKind::Heading { level: 1 }, "Second".to_owned()),
            (2, BlockKind::Heading { level: 2 }, "the details".to_owned()),
        ]
    );
    assert_eq!(r.doc.pages.len(), 3);
    assert_eq!(r.int("pptx.slides"), Some(3));
    assert_eq!(r.int("pptx.notes_slides"), Some(2));
}

#[test]
fn tables_with_spans_and_merges() {
    let tc = |t: &str, attrs: &str| {
        format!(
            r"<a:tc{attrs}><a:txBody><a:bodyPr/>{}</a:txBody></a:tc>",
            paras(&[t])
        )
    };
    let row = |cells: &[String]| format!("<a:tr h=\"370840\">{}</a:tr>", cells.concat());
    let tbl = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Table 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="457200" y="2000000"/><a:ext cx="8000000" cy="1000000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid><a:gridCol w="1"/><a:gridCol w="1"/><a:gridCol w="1"/></a:tblGrid>{}{}{}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        row(&[
            tc("Item", ""),
            tc("Q1", r#" gridSpan="2""#),
            tc("", r#" hMerge="1""#)
        ]),
        row(&[tc("Widgets", r#" rowSpan="2""#), tc("10", ""), tc("12", "")]),
        row(&[tc("", r#" vMerge="1""#), tc("11", ""), tc("", "")]),
    );
    let chart = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Chart"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="x" r:id="rId9"><a:t>not table text</a:t></c:chart></a:graphicData></a:graphic></p:graphicFrame>"#;
    let s = slide(&(sp(2, Some(("title", None)), None, &paras(&["Sales"])) + &tbl + chart));
    let r = run(&Pptx::of(&[&s]).build());
    let cell = |row, col, t: &str| {
        (
            BlockKind::TableCell { table: 0, row, col },
            Zone::Table,
            t.to_owned(),
        )
    };
    let got: Vec<(BlockKind, Zone, String)> = r
        .blocks()
        .into_iter()
        .skip(1)
        .map(|b| (b.1, b.2, b.3))
        .collect();
    assert_eq!(
        got,
        vec![
            cell(0, 0, "Item"),
            cell(0, 1, "Q1"),
            cell(1, 0, "Widgets"),
            cell(1, 1, "10"),
            cell(1, 2, "12"),
            cell(2, 1, "11"),
        ]
    );
    assert_eq!(r.int("pptx.tables"), Some(1));
}

#[test]
fn groups_hidden_shapes_footers_and_alternate_content() {
    let group = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="10" name="Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="100" y="3000000"/><a:ext cx="1" cy="1"/><a:chOff x="0" y="0"/><a:chExt cx="1" cy="1"/></a:xfrm></p:grpSpPr>{}{}</p:grpSp>"#,
        sp(
            11,
            None,
            Some((5_000_000, 3_000_000)),
            &paras(&["group right"])
        ),
        sp(12, None, Some((100, 3_000_000)), &paras(&["group left"])),
    );
    let hidden = r#"<p:sp><p:nvSpPr><p:cNvPr id="20" name="Secret" hidden="1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:p><a:r><a:t>hidden note</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let alt = format!(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="p14">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        sp(30, None, Some((100, 4_000_000)), &paras(&["choice"])),
        sp(31, None, Some((100, 4_000_000)), &paras(&["fallback"])),
    );
    let footer = sp(40, Some(("ftr", Some(3))), None, &paras(&["Confidential"]));
    let num = sp(
        41,
        Some(("sldNum", Some(4))),
        Some((0, 0)),
        "<a:p><a:fld id=\"{1}\" type=\"slidenum\"><a:t>3</a:t></a:fld></a:p>",
    );
    let br = sp(
        50,
        None,
        Some((100, 2_000_000)),
        "<a:p><a:r><a:t>line one</a:t></a:r><a:br/><a:r><a:t>line two</a:t></a:r></a:p>",
    );
    let s = part_sld_hidden(&(footer + &num + &group + hidden + &alt + &br));
    let r = run(&Pptx::of(&[&s]).build());
    let got: Vec<(Zone, String)> = r.blocks().into_iter().map(|b| (b.2, b.3)).collect();
    let b = |t: &str| (Zone::Body, t.to_owned());
    assert_eq!(
        got,
        vec![
            b("line one\nline two"),
            b("group left"),
            b("group right"),
            b("choice"),
            // Footers last, in their own reading order (this slide number sits at the top).
            (Zone::Footer, "3".to_owned()),
            (Zone::Footer, "Confidential".to_owned()),
        ]
    );
    assert_eq!(r.int("pptx.hidden_shapes"), Some(1));
    assert_eq!(r.int("pptx.hidden_slides"), Some(1));
}

fn part_sld_hidden(shapes: &str) -> String {
    common::part("sld", r#" show="0""#, shapes)
}

#[test]
fn properties_and_package_facts() {
    let mut p = Pptx::of(&[&slide(&sp(2, None, Some((0, 0)), &paras(&["x"])))]);
    p.core = Some(r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/"><dc:title>Board  deck</dc:title><dc:creator>Ana Example</dc:creator><cp:lastModifiedBy>Bob Sample</cp:lastModifiedBy><dcterms:created>2024-03-01T09:30:00Z</dcterms:created><dcterms:modified>never</dcterms:modified></cp:coreProperties>"#.to_owned());
    p.extra.push(("ppt/media/image1.png".into(), vec![0; 8]));
    p.extra.push(("ppt/vbaProject.bin".into(), vec![0; 8]));
    let r = run(&p.build());
    let text = |k: &str| match r.fact(k) {
        Some(Value::Text(t)) => t.to_string(),
        other => panic!("{k}: {other:?}"),
    };
    assert_eq!(text("pptx.title"), "Board deck");
    assert_eq!(text("pptx.creator"), "Ana Example");
    assert_eq!(text("pptx.last_modified_by"), "Bob Sample");
    assert!(
        matches!(r.fact("pptx.created"), Some(Value::Date(d)) if (d.y, d.m, d.d) == (2024, 3, 1))
    );
    assert!(r.fact("pptx.modified").is_none());
    assert_eq!(r.int("pptx.images"), Some(1));
    assert_eq!(r.fact("pptx.has_macros"), Some(&Value::Bool(true)));
    let w = r.warnings();
    assert!(w.iter().any(|w| w.contains("not a W3CDTF date")), "{w:?}");
}

#[test]
fn escaped_text_and_unicode() {
    let s = slide(&sp(
        2,
        None,
        Some((0, 0)),
        "<a:p><a:r><a:t>A &amp; B &lt;Ltd&gt; &#269;lan &#x4E2D;</a:t></a:r></a:p>",
    ));
    let r = run(&Pptx::of(&[&s]).build());
    assert_eq!(r.texts(), vec!["A & B <Ltd> član 中"]);
}

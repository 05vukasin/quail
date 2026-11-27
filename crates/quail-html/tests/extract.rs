//! What the HTML extractor makes of documents built here: blocks, tables, hidden content,
//! encodings, and synthetic files shaped like SEC EDGAR filings (no real filing is used).

mod common;

use common::run;
use quail_core::{BlockKind, PageKind, TextSource, Zone};

fn cell(table: u16, row: u16, col: u16) -> BlockKind {
    BlockKind::TableCell { table, row, col }
}

#[test]
fn headings_paragraphs_lists() {
    let r = run(r#"<!DOCTYPE html>
<html lang="en"><head><title> Annual
 Report </title><meta name="generator" content="Example Writer 2">
<style>p { color: red }</style><script>var x = "<p>not text</p>";</script></head>
<body>
<h1>Master   Services
Agreement</h1>
<p>This agreement is made between <b>Example Trading Ltd</b> and <i>Sample d.o.o.</i></p>
<p>First line<br>Second line<br/>
</p>
<ul><li>Payment within 30 days<li>Delivery DAP</ul>
<ol><li><p>Nested paragraph in item</p></li></ol>
<h3>3. Term</h3>
text without a paragraph
<pre>  keep   this
    indented</pre>
</body></html>"#);
    let b = r.blocks();
    let got: Vec<(BlockKind, Zone, &str)> =
        b.iter().map(|(_, k, z, t)| (*k, *z, t.as_str())).collect();
    assert_eq!(
        got,
        vec![
            (
                BlockKind::Heading { level: 1 },
                Zone::Title,
                "Master Services Agreement"
            ),
            (
                BlockKind::Paragraph,
                Zone::Body,
                "This agreement is made between Example Trading Ltd and Sample d.o.o."
            ),
            (BlockKind::Paragraph, Zone::Body, "First line\nSecond line"),
            (BlockKind::ListItem, Zone::Body, "Payment within 30 days"),
            (BlockKind::ListItem, Zone::Body, "Delivery DAP"),
            (BlockKind::ListItem, Zone::Body, "Nested paragraph in item"),
            (BlockKind::Heading { level: 3 }, Zone::Title, "3. Term"),
            (BlockKind::Paragraph, Zone::Body, "text without a paragraph"),
            (
                BlockKind::Paragraph,
                Zone::Body,
                "keep   this\n    indented"
            ),
        ]
    );
    assert_eq!(r.doc.pages.len(), 1);
    let p = &r.doc.pages[0];
    assert_eq!(p.kind, PageKind::Logical);
    assert_eq!(p.text_source, TextSource::Native);
    assert!(!r.text().contains("not text") && !r.text().contains("color"));
    assert_eq!(r.str("html.title").as_deref(), Some("Annual Report"));
    assert_eq!(r.str("html.lang").as_deref(), Some("en"));
    assert_eq!(r.str("html.generator").as_deref(), Some("Example Writer 2"));
    assert_eq!(r.int("html.scripts"), Some(1));
    assert_eq!(r.str("html.encoding").as_deref(), Some("UTF-8"));
    assert_eq!(r.str("html.encoding_source").as_deref(), Some("utf8"));
    r.assert_spans();
}

#[test]
fn entities_are_decoded() {
    let r = run(
        "<p>Fish &amp; Chips &lt;Ltd&gt; &copy; 2024 &#8212; &#x41;&eacute;&nbsp;x &bogus; &amp</p>",
    );
    assert_eq!(
        r.texts(),
        vec!["Fish & Chips <Ltd> © 2024 — Aé x &bogus; &"]
    );
}

#[test]
fn hidden_and_dropped_content() {
    let r = run(r#"<body>
<nav><a href="/">Home</a> | <a href="/about">About</a></nav>
<p>Visible one</p>
<div style="DISPLAY: none">hidden by style<p>and its children</p></div>
<p hidden>hidden attribute</p>
<span style="visibility : hidden">invisible span</span>
<template><p>template text</p></template>
<noscript>enable JavaScript</noscript>
<select><option>Option text</option></select>
<svg><text>svg label</text></svg>
<iframe src="https://example.com/x"><p>frame fallback</p></iframe>
<!-- a comment -->
<p>Visible two</p>
</body>"#);
    assert_eq!(r.texts(), vec!["Visible one", "Visible two"]);
    assert_eq!(r.int("html.hidden_elements"), Some(3));
    // Only the first title is the fact; no title is page text.
    let r = run("<title>First</title><p>body</p><title>Second</title>");
    assert_eq!(r.texts(), vec!["body"]);
    assert_eq!(r.str("html.title").as_deref(), Some("First"));
}

#[test]
fn implied_end_tags() {
    // Unclosed `<p>`, `<li>` and cells, the way old HTML writes them.
    let r = run(
        "<P>one<P>two<UL><LI>a<LI>b</UL><TABLE><TR><TD>c1<TD>c2<TR><TD>d1<TD>d2</TABLE><p>after",
    );
    assert_eq!(
        r.blocks()
            .iter()
            .map(|(_, k, _, t)| (*k, t.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (BlockKind::Paragraph, "one"),
            (BlockKind::Paragraph, "two"),
            (BlockKind::ListItem, "a"),
            (BlockKind::ListItem, "b"),
            (cell(0, 0, 0), "c1"),
            (cell(0, 0, 1), "c2"),
            (cell(0, 1, 0), "d1"),
            (cell(0, 1, 1), "d2"),
            (BlockKind::Paragraph, "after"),
        ]
    );
}

#[test]
fn table_spans() {
    let r = run(r#"<table>
<thead><tr><th rowspan="2">Item</th><th colspan="2">Amount</th></tr>
<tr><th>2024</th><th>2023</th></tr></thead>
<tbody><tr><td>Revenue</td><td>1,200</td><td>1,100</td></tr>
<tr><td colspan="3"><p>Note:</p><p>unaudited</p></td></tr></tbody>
</table>"#);
    let got: Vec<(BlockKind, Zone, String)> = r
        .blocks()
        .into_iter()
        .map(|(_, k, z, t)| (k, z, t))
        .collect();
    let t = |row, col, s: &str| (cell(0, row, col), Zone::Table, s.to_owned());
    assert_eq!(
        got,
        vec![
            t(0, 0, "Item"),
            t(0, 1, "Amount"),
            t(1, 1, "2024"),
            t(1, 2, "2023"),
            t(2, 0, "Revenue"),
            t(2, 1, "1,200"),
            t(2, 2, "1,100"),
            t(3, 0, "Note:\nunaudited"),
        ]
    );
    assert_eq!(r.int("html.tables"), Some(1));
    assert_eq!(r.int("html.layout_tables"), Some(0));
}

#[test]
fn header_and_footer_zones() {
    let r = run(
        "<header><p>Example Trading Ltd</p></header><main><p>Body</p></main><footer>Page 1</footer>",
    );
    let z: Vec<Zone> = r.blocks().iter().map(|b| b.2).collect();
    assert_eq!(z, vec![Zone::Header, Zone::Body, Zone::Footer]);
}

#[test]
fn page_breaks_split_pages() {
    let r = run(r#"<p>Page one</p>
<hr style="page-break-after: always">
<p>Page two</p>
<div style="page-break-before:always"><p>Page three</p></div>
<p style="break-after: page">end of three</p>
<div style="page-break-before: always"></div>
<p>Page four</p>"#);
    let pages: Vec<(u32, String)> = r.blocks().into_iter().map(|b| (b.0, b.3)).collect();
    assert_eq!(
        pages,
        vec![
            (0, "Page one".to_owned()),
            (1, "Page two".to_owned()),
            (2, "Page three".to_owned()),
            (2, "end of three".to_owned()),
            (3, "Page four".to_owned()),
        ]
    );
    assert_eq!(
        r.doc.pages.iter().map(|p| p.index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn declared_charsets_decode() {
    // windows-1250 (Serbian Latin) declared in a meta tag.
    let mut b = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1250\"></head><body><p>Ugovor o kupoprodaji, \x9Aifra \x8A-12, \xE8lan 3</p>".to_vec();
    b.extend_from_slice(b"</body></html>");
    let r = run(&b);
    assert_eq!(r.texts(), vec!["Ugovor o kupoprodaji, šifra Š-12, član 3"]);
    assert_eq!(r.str("html.encoding").as_deref(), Some("windows-1250"));
    assert_eq!(r.str("html.encoding_source").as_deref(), Some("meta"));

    // UTF-16LE with a BOM.
    let s = "<p>Уговор број 7</p>";
    let le: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(s.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    let r = run(&le);
    assert_eq!(r.texts(), vec!["Уговор број 7"]);
    assert_eq!(r.str("html.encoding_source").as_deref(), Some("bom"));

    // Undeclared and not UTF-8: guessed, never fatal.
    let r = run(b"<p>Caf\xE9 cr\xE8me br\xFBl\xE9e, d\xE9j\xE0 vu, na\xEFve fa\xE7ade</p>");
    assert_eq!(r.str("html.encoding_source").as_deref(), Some("guess"));
    assert_eq!(r.texts(), vec!["Café crème brûlée, déjà vu, naïve façade"]);
}

#[test]
fn xhtml_self_closing_and_html_mode() {
    // In XHTML `<div/>` is empty; in HTML it opens a div (and the hidden one hides what follows).
    let x = run(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><div style="display:none"/><p>shown</p></body></html>"#,
    );
    assert_eq!(x.texts(), vec!["shown"]);
    let h = run(
        r#"<body><div style="display:none"/><p>inside the hidden div</p></div><p>after</p></body>"#,
    );
    assert_eq!(h.texts(), vec!["after"]);
}

/// A synthetic 10-K shaped like an EDGAR inline XBRL filing: XML declaration, namespace
/// declarations, a hidden `ix:header` with `dei:` facts, a layout table around the whole
/// document, `<font>` everywhere, a financial table nested in the layout, inline XBRL facts
/// inside cells and paragraphs, and `<hr>` page breaks.
const TEN_K: &str = r#"<?xml version='1.0' encoding='ASCII'?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:ix="http://www.xbrl.org/2013/inlineXBRL" xmlns:dei="http://xbrl.sec.gov/dei/2023" xmlns:us-gaap="http://fasb.org/us-gaap/2023" xml:lang="en-US">
<head><title>exmp-20231231</title><meta http-equiv="Content-Type" content="text/html"/></head>
<body>
<div style="display:none"><ix:header><ix:hidden>
<ix:nonNumeric name="dei:AmendmentFlag" contextRef="c-1">false</ix:nonNumeric>
<ix:nonNumeric name="dei:EntityCentralIndexKey" contextRef="c-1">0000999999</ix:nonNumeric>
</ix:hidden><ix:references/><ix:resources><xbrli:context id="c-1">ctx</xbrli:context></ix:resources></ix:header></div>
<table style="width:100%"><tr><td>
<p style="text-align:center"><font style="font-family:Times New Roman;font-size:10pt;font-weight:bold">UNITED STATES<br/>SECURITIES AND EXCHANGE COMMISSION</font></p>
<p style="text-align:center"><font size="2">FORM <ix:nonNumeric name="dei:DocumentType" contextRef="c-1">10-K</ix:nonNumeric></font></p>
<p><font size="2">For the fiscal year ended <ix:nonNumeric name="dei:DocumentPeriodEndDate" contextRef="c-1" format="ixt:date-monthname-day-year-en">December&#160;31, 2023</ix:nonNumeric></font></p>
<p><font size="2"><ix:nonNumeric name="dei:EntityRegistrantName" contextRef="c-1">EXAMPLE HOLDINGS, INC.</ix:nonNumeric></font></p>
<hr style="page-break-after:always"/>
<h2><font size="3">Item&#160;7. Management&#8217;s Discussion</font></h2>
<p><font size="2">Revenue was $<ix:nonFraction name="us-gaap:Revenues" contextRef="c-1" unitRef="usd" decimals="-6" scale="6" format="ixt:num-dot-decimal">1,234</ix:nonFraction> million.</font></p>
<table cellpadding="0" cellspacing="0" style="border-collapse:collapse">
<tr><td style="width:60%"><font size="1">&#160;</font></td><td colspan="2"><font size="1"><b>2023</b></font></td><td colspan="2"><font size="1"><b>2022</b></font></td></tr>
<tr><td><font size="2">Net sales</font></td><td><font size="2">$</font></td><td style="text-align:right"><font size="2"><ix:nonFraction name="us-gaap:Revenues" contextRef="c-1" unitRef="usd" decimals="-6" scale="6">1,234</ix:nonFraction></font></td><td><font size="2">$</font></td><td style="text-align:right"><font size="2">1,100</font></td></tr>
<tr><td><font size="2">Net loss</font></td><td></td><td style="text-align:right"><font size="2">(<ix:nonFraction name="us-gaap:NetIncomeLoss" contextRef="c-1" unitRef="usd" sign="-" scale="6">56</ix:nonFraction>)</font></td><td></td><td style="text-align:right"><font size="2">(48</font></td></tr>
</table>
<p style="page-break-before:always"><font size="2">Signatures</font></p>
</td></tr></table>
</body></html>"#;

#[test]
fn edgar_inline_xbrl_10k() {
    let r = run(TEN_K);
    let b = r.blocks();
    let got: Vec<(u32, BlockKind, &str)> =
        b.iter().map(|(p, k, _, t)| (*p, *k, t.as_str())).collect();
    assert_eq!(
        got,
        vec![
            (
                0,
                BlockKind::Paragraph,
                "UNITED STATES\nSECURITIES AND EXCHANGE COMMISSION"
            ),
            (0, BlockKind::Paragraph, "FORM 10-K"),
            (
                0,
                BlockKind::Paragraph,
                "For the fiscal year ended December 31, 2023"
            ),
            (0, BlockKind::Paragraph, "EXAMPLE HOLDINGS, INC."),
            (
                1,
                BlockKind::Heading { level: 2 },
                "Item 7. Management\u{2019}s Discussion"
            ),
            (1, BlockKind::Paragraph, "Revenue was $1,234 million."),
            (1, cell(0, 0, 1), "2023"),
            (1, cell(0, 0, 3), "2022"),
            (1, cell(0, 1, 0), "Net sales"),
            (1, cell(0, 1, 1), "$"),
            (1, cell(0, 1, 2), "1,234"),
            (1, cell(0, 1, 3), "$"),
            (1, cell(0, 1, 4), "1,100"),
            (1, cell(0, 2, 0), "Net loss"),
            (1, cell(0, 2, 2), "(56)"),
            (1, cell(0, 2, 4), "(48"),
            (2, BlockKind::Paragraph, "Signatures"),
        ]
    );
    // The hidden header's text is not page text, but its cover facts are kept.
    assert!(!r.text().contains("0000999999") && !r.text().contains("ctx"));
    assert_eq!(
        r.list("html.xbrl_dei"),
        vec![
            "AmendmentFlag=false",
            "EntityCentralIndexKey=0000999999",
            "DocumentType=10-K",
            "DocumentPeriodEndDate=December 31, 2023",
            "EntityRegistrantName=EXAMPLE HOLDINGS, INC.",
        ]
    );
    assert_eq!(
        r.fact("html.inline_xbrl"),
        Some(&quail_core::Value::Bool(true))
    );
    assert_eq!(r.int("html.ix_facts"), Some(8));
    assert_eq!(r.int("html.tables"), Some(1));
    assert_eq!(r.int("html.layout_tables"), Some(1));
    assert_eq!(r.int("html.hidden_elements"), Some(1));
    assert_eq!(r.str("html.encoding").as_deref(), Some("windows-1252"));
    assert_eq!(r.str("html.encoding_source").as_deref(), Some("xml"));
    assert_eq!(r.str("html.lang").as_deref(), Some("en-US"));
    assert_eq!(r.str("html.title").as_deref(), Some("exmp-20231231"));
    assert!(r.doc.pages.iter().all(|p| p.kind == PageKind::Logical));
    r.assert_spans();
}

/// An older 8-K the way EDGAR filers wrote them around 2005: upper-case tags, no closing
/// `</P>` or `</TD>`, `<FONT>` and `<CENTER>`, layout tables nested three deep, `&nbsp;`
/// spacer cells, an ISO-8859-1 charset.
#[test]
fn edgar_legacy_8k() {
    let mut b = br##"<HTML><HEAD><TITLE>form8-k.htm</TITLE>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=iso-8859-1"></HEAD>
<BODY BGCOLOR="#FFFFFF">
<CENTER><FONT SIZE=4><B>FORM 8-K</B></FONT></CENTER>
<TABLE WIDTH="100%"><TR><TD>
 <TABLE WIDTH="100%"><TR><TD>
  <TABLE BORDER=0>
   <TR><TD><FONT SIZE=2>Delaware</FONT><TD>&nbsp;<TD><FONT SIZE=2>001-99999</FONT>
   <TR><TD><FONT SIZE=1>(State of incorporation)</FONT><TD>&nbsp;<TD><FONT SIZE=1>(Commission File No.)</FONT>
  </TABLE>
 </TD></TR></TABLE>
</TD></TR></TABLE>
<P><FONT SIZE=2><B>Item&nbsp;2.02</B>&nbsp;&nbsp;Results of Operations and Financial Condition.</FONT>
<P><FONT SIZE=2>On May&nbsp;1, the Company issued a press release (the &#147;Release&#148;). Caf"##
        .to_vec();
    b.extend_from_slice(b"\xE9 S.A.</FONT>\n</BODY></HTML>");
    let r = run(&b);
    let got: Vec<(BlockKind, String)> = r.blocks().into_iter().map(|b| (b.1, b.3)).collect();
    let p = |s: &str| (BlockKind::Paragraph, s.to_owned());
    assert_eq!(
        got,
        vec![
            p("FORM 8-K"),
            (cell(0, 0, 0), "Delaware".to_owned()),
            (cell(0, 0, 2), "001-99999".to_owned()),
            (cell(0, 1, 0), "(State of incorporation)".to_owned()),
            (cell(0, 1, 2), "(Commission File No.)".to_owned()),
            p("Item 2.02 Results of Operations and Financial Condition."),
            p(
                "On May 1, the Company issued a press release (the \u{201c}Release\u{201d}). Café S.A."
            ),
        ]
    );
    assert_eq!(r.int("html.layout_tables"), Some(2));
    assert_eq!(r.int("html.tables"), Some(1));
    assert_eq!(
        r.fact("html.inline_xbrl"),
        Some(&quail_core::Value::Bool(false))
    );
    assert_eq!(r.str("html.title").as_deref(), Some("form8-k.htm"));
    assert_eq!(r.str("html.encoding").as_deref(), Some("windows-1252"));
}

#[test]
fn empty_and_textless_inputs() {
    for b in [
        &b""[..],
        b"<html></html>",
        b"<!-- only a comment -->",
        b"<script>x</script>",
    ] {
        let r = run(b);
        assert_eq!(r.doc.pages.len(), 1);
        assert!(r.doc.pages[0].text.is_empty());
    }
}

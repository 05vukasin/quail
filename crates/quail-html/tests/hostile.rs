//! Hostile inputs built in code: deep nesting, endless tokens, huge spans, limits, the
//! deadline. Each must end quickly with bounded memory and a partial (never failed) result.

#![allow(clippy::cast_possible_truncation)] // a tiny test PRNG indexes small tables

mod common;

use std::time::{Duration, Instant};

use common::{run, run_with};
use quail_core::{Blob, Emitter, Extractor, Limits};
use quail_html::HtmlExtractor;

fn quick<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let v = f();
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "{what} took {:?}",
        t.elapsed()
    );
    v
}

#[test]
fn deeply_nested_elements() {
    let mut s = "<div>".repeat(300_000);
    s.push_str("deep text");
    let r = quick("divs", || run(&s));
    assert_eq!(r.texts(), vec!["deep text"]);
    let mut s = "<span>".repeat(300_000);
    s.push_str("<p>x</p>");
    s.push_str(&"</span>".repeat(300_000));
    s.push_str("<p>after</p>");
    let r = quick("spans", || run(&s));
    assert_eq!(r.texts(), vec!["x", "after"]);
}

#[test]
fn nested_tables_and_hidden_nesting() {
    let n = 100_000;
    let mut s = "<table><tr><td>".repeat(n);
    s.push_str("core");
    s.push_str(&"</td></tr></table>".repeat(n));
    s.push_str("<p>after</p>");
    let r = quick("tables", || run(&s));
    let t = r.texts();
    assert_eq!(t.first().map(String::as_str), Some("core"));
    assert_eq!(t.last().map(String::as_str), Some("after"));
    r.assert_spans();
    // Hidden content nested past the depth bound still ends where it should.
    let mut s = "<div hidden>".to_owned();
    s.push_str(&"<b>".repeat(5000));
    s.push_str("secret");
    s.push_str(&"</b>".repeat(5000));
    s.push_str("</div><p>shown</p>");
    let r = quick("hidden", || run(&s));
    assert_eq!(r.texts(), vec!["shown"]);
}

#[test]
fn endless_tokens_stop() {
    for start in ["<!--", "<p title=\"", "<p ", "<!DOCTYPE html PUBLIC \""] {
        let mut s = format!("<p>before</p>{start}");
        s.push_str(&"x".repeat(17 << 20));
        let r = quick(start, || run(&s));
        assert_eq!(r.texts(), vec!["before"], "{start}");
        let problems = r.problems();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("without a complete token")),
            "{start}: {problems:?}"
        );
    }
    // A long script is consumed, not kept.
    let mut s = "<script>".to_owned();
    s.push_str(&"var a = 1;\n".repeat(2_000_000));
    s.push_str("</script><p>after</p>");
    let r = quick("script", || run(&s));
    assert_eq!(r.texts(), vec!["after"]);
}

#[test]
fn text_limit_is_charged_and_cut_on_a_char_boundary() {
    let limits = Limits {
        max_text_bytes: 1000,
        ..Limits::default()
    };
    let s = "<p>ćirilica и латиница</p>".repeat(1000);
    let r = run_with(&s, &limits);
    let total: usize = r.doc.pages.iter().map(|p| p.text.len()).sum();
    assert!(total <= 1000, "{total}");
    assert!(total > 900, "{total}");
    r.assert_spans();
    let p = r.problems();
    assert!(p.iter().any(|p| p.contains("text limit")), "{p:?}");
    // Inside a table too.
    let s = format!(
        "<table>{}</table>",
        "<tr><td>cell text</td><td>more</td></tr>".repeat(10_000)
    );
    let r = run_with(&s, &limits);
    let total: usize = r.doc.pages.iter().map(|p| p.text.len()).sum();
    assert!(total <= 1000, "{total}");
}

#[test]
fn page_and_block_caps() {
    let limits = Limits {
        max_pages: 3,
        ..Limits::default()
    };
    let s = "<p style=\"page-break-after:always\">page</p>".repeat(100);
    let r = run_with(&s, &limits);
    assert_eq!(r.doc.pages.len(), 3);
    assert!(r.problems().iter().any(|p| p.contains("max_pages")));
    let s = "<p>x".repeat(300_000);
    let r = quick("blocks", || run(&s));
    assert_eq!(r.texts().len(), 250_000);
    assert!(r.problems().iter().any(|p| p.contains("blocks")));
}

#[test]
fn absurd_spans_and_wide_rows() {
    let s = r#"<table><tr><td colspan="4294967295" rowspan="99999999999">a</td><td colspan="-5">b</td></tr>
<tr><td>c</td></tr></table>"#;
    let r = run(s);
    assert_eq!(r.texts(), vec!["a", "b", "c"]);
    let s = format!("<table><tr>{}</tr></table>", "<td>v</td>".repeat(100_000));
    let r = quick("wide", || run(&s));
    assert_eq!(r.texts().len(), 100_000);
    r.assert_spans();
    // Rows of full-width spans: the span bookkeeping stays bounded.
    let s = format!(
        "<table>{}</table>",
        "<tr><td colspan=1000 rowspan=65534>w</td></tr>".repeat(20_000)
    );
    quick("spans", || run(&s));
}

#[test]
fn references_and_doctype_never_expand() {
    // HTML has no DOCTYPE entities: an XML-style internal subset is just a doctype token.
    let s = r#"<!DOCTYPE html [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]><p>&b;&a; &amp;&#0;&#xFFFFFFFF;&#55296;</p>"#;
    let r = run(s);
    let t = r.texts().join("|");
    assert!(t.contains("&b;&a; &"), "{t}");
    assert!(t.len() < 100, "{t}");
    // Many references decode in linear time.
    let s = format!("<p>{}</p>", "&amp;&eacute;&#8212;".repeat(100_000));
    let r = quick("refs", || run(&s));
    assert_eq!(r.texts()[0].chars().count(), 300_000);
}

#[test]
fn invalid_bytes_are_counted() {
    let r = run(b"<meta charset=utf-8><p>ok \xFF\xFE bad</p>");
    assert_eq!(r.int("html.decode_errors"), Some(2));
    assert_eq!(r.texts(), vec!["ok \u{fffd}\u{fffd} bad"]);
}

#[test]
fn past_deadline_is_partial() {
    let s = "<p>text</p>".repeat(100_000);
    let blob = Blob::new(s.into_bytes());
    let mut out = Emitter::with_deadline(quail_html::ID, Instant::now());
    let doc = HtmlExtractor::new()
        .extract(&blob, &Limits::default(), &mut out)
        .unwrap();
    assert!(doc.pages.len() <= 1);
    assert!(out.take_problems().iter().any(|p| p.contains("deadline")));
}

#[test]
fn random_and_cut_inputs_never_panic() {
    let doc = br#"<?xml version='1.0'?><html xmlns:ix="http://www.xbrl.org/2013/inlineXBRL"><body><div style="display:none"><ix:header><ix:nonNumeric name="dei:DocumentType">10-K</ix:nonNumeric></ix:header></div><table><tr><td rowspan=2>a<td colspan=3><table><tr><td>b</table><tr><td>c</table><h2 style="page-break-before:always">T</h2><ul><li>x<li><pre>y
 z</pre></ul><script>if (a<b) {}</script><title>t</title>&amp;&#x41;</body></html>"#;
    for cut in 0..doc.len() {
        let r = run(&doc[..cut]);
        r.assert_spans();
    }
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabet: &[&[u8]] = &[
        b"<",
        b">",
        b"/",
        b"<p>",
        b"</p>",
        b"<td>",
        b"<tr>",
        b"<table>",
        b"</table>",
        b"<li>",
        b"<div hidden>",
        b"</div>",
        b"<br>",
        b"&amp;",
        b"&#",
        b"x",
        b" ",
        b"\n",
        b"<!--",
        b"-->",
        b"<script>",
        b"</script>",
        b"<ix:nonNumeric name=dei:A>",
        b"</ix:nonNumeric>",
        b"\xFF",
        b"\0",
        b"<h1>",
        b"<pre>",
        b"<hr style=page-break-after:always>",
    ];
    for _ in 0..300 {
        let mut b = Vec::new();
        for _ in 0..(next() % 400) {
            b.extend_from_slice(alphabet[(next() % alphabet.len() as u64) as usize]);
        }
        run(&b).assert_spans();
    }
}

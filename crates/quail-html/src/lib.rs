//! HTML extractor: the WHATWG tokenizer of `html5ever` (no tree builder, no DOM) feeding an
//! element stack of our own, so every structure and its depth bound is ours.
//!
//! Namespace: `html.*` (every key is in [`keys`]). Extractor id: [`ID`] (`html.extract@1`).
//!
//! # Document model
//!
//! - [`PageKind::Logical`](quail_core::PageKind) pages split at CSS page breaks
//!   (`page-break-before:always`, `page-break-after:always`, `break-before:page`,
//!   `break-after:page`; what EDGAR filings put between printed pages), or one page.
//! - Blocks: paragraphs and other block elements ([`BlockKind::Paragraph`]); `h1`-`h6`
//!   ([`BlockKind::Heading`], zone `Title`); `li` ([`BlockKind::ListItem`]); table cells
//!   ([`BlockKind::TableCell`] with table, row and column, `colspan` and `rowspan`
//!   respected, zone `Table`). Blocks in `<header>` and `<footer>` are in zones `Header` and
//!   `Footer`. `<br>` is `\n` inside a block. Whitespace collapses as a browser shows it
//!   (kept in `<pre>`), and `&nbsp;` is a space.
//! - **Layout tables** (one cell in all, or a cell holding a heading, a list, a page break
//!   or another table: how EDGAR filings and old HTML lay out pages) are not data: their
//!   cells are read as ordinary blocks, and the tables inside them become tables of their
//!   own. A data table's cell is one block, its paragraphs joined by `\n`.
//! - Left out: `script`, `style`, `template`, `noscript`, `nav`, `iframe`, `object`, `svg`,
//!   `math`, `select`, `button`, `textarea`, media elements, the `<head>` (the `<title>` is
//!   the [`keys::TITLE`] fact), comments, and **hidden content**: the `hidden` attribute,
//!   `display:none`, `visibility:hidden` and `ix:header`.
//! - Inline XBRL: `ix:` elements are ordinary inline content, so the text of
//!   `ix:nonFraction` and `ix:nonNumeric` stays where it is. The `dei:` cover facts (hidden
//!   ones included) become [`keys::XBRL_DEI`].
//! - `<x/>` closes itself in XHTML (an XML declaration or the XHTML namespace) and for
//!   prefixed elements (`<ix:nonNumeric .../>`); in HTML it does not, as in a browser.
//!
//! # Encoding
//!
//! A BOM, then `<meta charset>` / `<meta http-equiv="Content-Type">` (a simplified WHATWG
//! prescan of the first 4 KiB), then the XML declaration's `encoding`, then UTF-8 when the
//! bytes are valid UTF-8, then a `chardetng` guess. Labels are WHATWG labels (`iso-8859-1`
//! and `ASCII` decode as windows-1252); a declared UTF-16 means UTF-8.
//!
//! # Limits
//!
//! The input is decoded and tokenized in 64 KiB chunks with the deadline checked between
//! them. Character references are the HTML table (no DOCTYPE entities exist in HTML, so
//! nothing expands). `max_text_bytes` is charged for every byte of page text, separators
//! included; `max_pages` stops at that many pages; at most 250 000 blocks; 512 open
//! elements (deeper ones are inline content); 32 nested tables (deeper ones are plain
//! blocks); 1024 columns, `colspan` 1000, `rowspan` 65 534; more than 16 MiB of input
//! without one token completing (an endless comment, tag or attribute value) stops the
//! read. Stopping short is reported with `Emitter::problem`. No URL is ever fetched.

mod build;
mod charset;
pub mod keys;

use quail_core::{AnalyzerId, Blob, Document, Emitter, Extractor, Format, Limits, Result, Value};

#[cfg(doc)]
use quail_core::BlockKind;

/// Name and version of the extractor.
pub const ID: AnalyzerId = AnalyzerId::new("html.extract", 1);

/// The HTML extractor. Stateless; share one instance between threads.
#[derive(Clone, Copy, Debug, Default)]
pub struct HtmlExtractor;

impl HtmlExtractor {
    /// New extractor.
    pub fn new() -> Self {
        Self
    }
}

fn int(n: i64) -> Value {
    Value::Int(n)
}

impl Extractor for HtmlExtractor {
    fn id(&self) -> AnalyzerId {
        ID
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Html]
    }

    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        let bytes = blob.as_bytes();
        let (enc, bom, source) = charset::detect(bytes);
        out.fact(keys::ENCODING, Value::text(enc.name()));
        out.fact(keys::ENCODING_SOURCE, Value::text(source.as_str()));
        let (pages, st, ended) = build::run(bytes.get(bom..).unwrap_or(&[]), enc, limits, out);
        if ended.decode_errors > 0 {
            out.fact(
                keys::DECODE_ERRORS,
                int(i64::try_from(ended.decode_errors).unwrap_or(i64::MAX)),
            );
            out.warn(format!(
                "{} bytes not valid in {} replaced by U+FFFD",
                ended.decode_errors,
                enc.name()
            ));
        }
        if let Some(t) = st.title {
            out.fact(keys::TITLE, Value::text(t));
        }
        if let Some(l) = st.lang {
            out.fact(keys::LANG, Value::text(l));
        }
        if let Some(g) = st.generator {
            out.fact(keys::GENERATOR, Value::text(g));
        }
        out.fact(keys::INLINE_XBRL, Value::Bool(st.inline_xbrl));
        if st.inline_xbrl {
            out.fact(keys::IX_FACTS, int(st.ix_facts));
        }
        if !st.dei.is_empty() {
            out.fact(
                keys::XBRL_DEI,
                Value::List(st.dei.into_iter().map(Value::text).collect()),
            );
        }
        out.fact(keys::TABLES, int(st.tables));
        out.fact(keys::LAYOUT_TABLES, int(st.layout_tables));
        out.fact(keys::HIDDEN_ELEMENTS, int(st.hidden));
        out.fact(keys::SCRIPTS, int(st.scripts));
        out.fact(keys::ELEMENTS, int(st.elements));
        if st.depth_capped {
            out.warn(format!(
                "elements nested deeper than {} read as inline content",
                build::MAX_DEPTH
            ));
        }
        if st.tables_capped {
            out.warn(format!(
                "tables nested deeper than {} read as plain blocks",
                build::MAX_TABLE_DEPTH
            ));
        }
        // Stopped short: the record becomes partial.
        if st.truncated {
            out.problem(format!(
                "text limit of {} bytes reached, rest of the text dropped",
                limits.max_text_bytes
            ));
        }
        if st.pages_capped {
            out.problem(format!(
                "max_pages ({}) reached, rest of the document dropped",
                limits.max_pages
            ));
        }
        if st.blocks_capped {
            out.problem(format!(
                "more than {} blocks: the rest dropped",
                build::MAX_BLOCKS
            ));
        }
        if ended.oversize_token {
            out.problem(format!(
                "more than {} bytes without a complete token (an unterminated comment, tag or \
                 attribute): rest of the file skipped",
                build::MAX_TOKEN
            ));
        }
        if ended.timed_out {
            out.problem("deadline reached: document is partial");
        }
        Ok(Document { pages })
    }
}

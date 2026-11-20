//! DOCX extractor: a streaming OOXML reader built on `zip` and `quick-xml` (no DOM).
//!
//! Namespace: `docx.*` (every key is in [`keys`]). Extractor id: [`ID`] (`docx.extract@1`).
//!
//! # Document model
//!
//! - One [`Page`](quail_core::Page) of [`PageKind::Logical`](quail_core::PageKind) per
//!   section (`w:sectPr`), in order. Each page's text is, in reading order: the section's
//!   header blocks ([`Zone::Header`](quail_core::Zone)), the body, the footnotes and endnotes
//!   referenced in the section ([`BlockKind::Note`](quail_core::BlockKind)), and the footer
//!   blocks ([`Zone::Footer`](quail_core::Zone)). Headers and footers are emitted once: a
//!   section whose header text was already emitted (the same part, or one with identical
//!   text) gets none.
//! - Blocks: paragraphs; headings (paragraph or style outline level, or the built-in
//!   `heading N` / `title` styles, down the `basedOn` chain), zone `Title`; list items
//!   (`w:numPr`, direct or from the style); table cells with table/row/column indices
//!   (`w:gridSpan` and `w:gridBefore` respected), zone `Table`. A table nested in a cell is
//!   flattened into that cell's text (counted in `docx.nested_tables`).
//! - Numbered paragraphs start with the label Word shows (`7.1`, `(a)`, `iv.`, `-` for
//!   bullets), computed from `numbering.xml` with Word's counting rules.
//! - Text is the **accepted-changes** view: tracked insertions are included, tracked
//!   deletions are not. Each deletion's text follows its paragraph as a
//!   [`BlockKind::Annotation`](quail_core::BlockKind) block, and so does each comment
//!   referenced from the paragraph.
//! - Fields: the result text is kept, the field code is not. `w:tab` is `\t`, `w:br` is `\n`.
//!   Text boxes are read once (`mc:Fallback` copies are skipped).
//!
//! # Limits
//!
//! Zip entry count (at most 20 000), unsafe entry names (absolute, `..`, drive letters,
//! backslashes: ignored), and the bytes every part *actually* inflates to, counted against
//! [`Limits::max_decompressed`](quail_core::Limits) for the whole file; declared sizes only
//! ever make it refuse earlier. `max_pages` caps sections, `max_text_bytes` caps text. The
//! deadline is checked every 2048 XML events. Malformed XML yields a partial document and a
//! warning.

mod body;
mod extract;
pub mod keys;
mod numbering;
mod package;
mod props;
mod rels;
mod styles;
mod util;
mod xml;

use quail_core::{AnalyzerId, Blob, Document, Emitter, Extractor, Format, Limits, Result};

/// Name and version of the extractor.
pub const ID: AnalyzerId = AnalyzerId::new("docx.extract", 1);

/// The DOCX extractor. Stateless; share one instance between threads.
#[derive(Clone, Copy, Debug, Default)]
pub struct DocxExtractor;

impl DocxExtractor {
    /// New extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Extractor for DocxExtractor {
    fn id(&self) -> AnalyzerId {
        ID
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Docx]
    }

    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        let mut warnings = Vec::new();
        let mut facts = Vec::new();
        let result = extract::run(blob.as_bytes(), limits, out, &mut warnings, &mut facts);
        for (k, v) in facts {
            out.fact(k, v);
        }
        for w in warnings {
            out.warn(w);
        }
        result
    }
}

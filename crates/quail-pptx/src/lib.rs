//! PPTX extractor: a streaming OOXML reader built on `quail-ooxml` (`zip` and `quick-xml`,
//! no DOM).
//!
//! Namespace: `pptx.*` (every key is in [`keys`]). Extractor id: [`ID`] (`pptx.extract@1`).
//!
//! # Document model
//!
//! - One [`Page`](quail_core::Page) of [`PageKind::Logical`](quail_core::PageKind) per
//!   slide, in the order of `p:sldIdLst` (hidden slides included, counted in
//!   [`keys::HIDDEN_SLIDES`]).
//! - **Reading order** on a slide: the title placeholders, then the shapes top to bottom
//!   and left to right (by the offset of their `xfrm`; a placeholder without one takes the
//!   position of its layout's placeholder with the same `idx` or type, else the master's),
//!   then the date, footer and slide number placeholders ([`Zone::Footer`]). A group
//!   is sorted inside and placed as one. Shapes without any position keep their file order
//!   after the placed ones.
//! - Blocks: the title (`title`, `ctrTitle`) is one [`BlockKind::Heading`] of level 1 and
//!   the subtitle one of level 2, zone `Title`, paragraphs joined by `\n`; every other
//!   paragraph is a block, a [`BlockKind::ListItem`] when it has a bullet (`a:buChar`,
//!   `a:buAutoNum`, or a body placeholder without `a:buNone`) and a
//!   [`BlockKind::Paragraph`] otherwise; `a:br` is `\n`. Table cells (`a:tbl`) are
//!   [`BlockKind::TableCell`] with table, row and column (zone `Table`, merged continuation
//!   cells left out, the paragraphs of a cell joined by `\n`).
//! - **Speaker notes**: the body placeholder of the slide's notes part, one
//!   [`BlockKind::Note`] per paragraph, after the slide's own blocks.
//! - Left out: hidden shapes (`cNvPr hidden="1"`, counted in [`keys::HIDDEN_SHAPES`]),
//!   pictures, connectors, charts and diagrams (their text lives in other parts), the
//!   `mc:Fallback` copy of alternate content, and comments.
//!
//! # Limits
//!
//! The package is read by `quail-ooxml` exactly as for DOCX: at most 20 000 zip entries
//! (from the end-of-central-directory record, before the directory is parsed), unsafe
//! names ignored, the bytes parts **actually** inflate to counted against
//! [`Limits::max_decompressed`](quail_core::Limits) for the whole file (declared sizes are
//! never trusted), DOCTYPEs never expanded, one XML event at most 16 MiB, the deadline
//! checked every 2048 XML events and per slide. Slides are read before the properties.
//! `max_pages` slides; `max_text_bytes` charged for every byte of page text, separators
//! included; at most 250 000 blocks per file, 20 000 slides listed, 100 000 shapes,
//! paragraphs and cells per part, groups 64 deep, 1 MiB per paragraph, 1000 layouts and
//! masters. Stopping short is reported with `Emitter::problem`; malformed XML ends only
//! its part.

mod extract;
pub mod keys;
mod slide;

use quail_core::{AnalyzerId, Blob, Document, Emitter, Extractor, Format, Limits, Result};
use quail_ooxml::Notes;

#[cfg(doc)]
use quail_core::{BlockKind, Zone};

/// Name and version of the extractor.
pub const ID: AnalyzerId = AnalyzerId::new("pptx.extract", 1);

/// The PPTX extractor. Stateless; share one instance between threads.
#[derive(Clone, Copy, Debug, Default)]
pub struct PptxExtractor;

impl PptxExtractor {
    /// New extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Extractor for PptxExtractor {
    fn id(&self) -> AnalyzerId {
        ID
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Pptx]
    }

    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        let mut notes = Notes::default();
        let mut facts = Vec::new();
        let result = extract::run(blob.as_bytes(), limits, out, &mut notes, &mut facts);
        for (k, v) in facts {
            out.fact(k, v);
        }
        for w in notes.warnings {
            out.warn(w);
        }
        // Stopped short (a limit, the deadline, a broken part): the record becomes partial.
        for p in notes.problems {
            out.problem(p);
        }
        result
    }
}

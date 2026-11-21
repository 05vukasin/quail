use serde::{Deserialize, Serialize};

use crate::artifact::Artifact;
use crate::fact::{BBox, Zone};

/// A byte range `[start, end)` in a page's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    /// First byte.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
}

impl Span {
    /// The text this span covers. Empty when out of range or not on a char boundary.
    pub fn slice(self, text: &str) -> &str {
        text.get(self.start as usize..self.end as usize)
            .unwrap_or("")
    }
}

/// Whether a page is a printed page or a logical unit of a flowing document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    /// A real page with geometry (PDF, image).
    Physical,
    /// A flowing document without fixed pages (DOCX): one logical page per section.
    Logical,
}

/// Where a page's text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSource {
    /// The file's own text (PDF text layer, DOCX XML).
    Native,
    /// Optical character recognition.
    Ocr,
    /// No text yet: the page is an image or its text layer was rejected.
    None,
}

/// What a block of text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BlockKind {
    /// A paragraph or a line group.
    Paragraph,
    /// A heading; `level` 1 is the highest.
    Heading {
        /// Heading level.
        level: u8,
    },
    /// An item of a numbered or bulleted list.
    ListItem,
    /// One cell of a table.
    TableCell {
        /// Table index on the page.
        table: u16,
        /// Row index.
        row: u16,
        /// Column index.
        col: u16,
    },
    /// Footnote or endnote.
    Note,
    /// Comment or tracked change (DOCX).
    Annotation,
}

/// A run of text on a page. Its text is `span` of [`Page::text`], so text is stored once and
/// every fact can point back into it. Build it with [`Block::new`] so that fields added later
/// do not break extractors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    /// Range in the page text.
    pub span: Span,
    /// What it is.
    pub kind: BlockKind,
    /// Where on the page it sits.
    pub zone: Zone,
    /// Its box, when the format has geometry.
    pub bbox: Option<BBox>,
    /// The measured visual lines inside the block, in reading order, each with its span
    /// (within the block's `span`) and its box. Empty when the format has no line geometry
    /// (and then left out of serialized output). A paragraph block keeps its own `span` and
    /// `bbox`; the lines refine it, they do not replace it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<LineBox>,
}

impl Block {
    /// A block without line geometry.
    pub fn new(span: Span, kind: BlockKind, zone: Zone, bbox: Option<BBox>) -> Self {
        Self {
            span,
            kind,
            zone,
            bbox,
            lines: Vec::new(),
        }
    }
}

/// One measured visual line of a [`Block`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineBox {
    /// Range in the page text (within the block's span).
    pub span: Span,
    /// Its box, in points from the top-left corner.
    pub bbox: BBox,
}

/// An image placed on a page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageImage {
    /// Where it is drawn, in points.
    pub bbox: BBox,
    /// Its pixel size as stored in the file.
    pub pixels: (u32, u32),
}

/// One page of a document. Build it with struct update syntax over [`Page::new`] so that
/// fields added later do not break extractors:
/// `Page { text, blocks, ..Page::new(index, PageKind::Physical) }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    /// Zero-based index.
    pub index: u32,
    /// Physical or logical.
    pub kind: PageKind,
    /// Width and height in points, for physical pages.
    pub size: Option<(f32, f32)>,
    /// Text in reading order. Blocks are separated by `\n`.
    pub text: String,
    /// Blocks pointing into `text`, in reading order.
    pub blocks: Vec<Block>,
    /// Where the text came from.
    pub text_source: TextSource,
    /// The text layer is missing or unusable and the page should be OCR'd.
    pub needs_ocr: bool,
    /// Images drawn on the page (scans, logos, stamps, signatures).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PageImage>,
    /// Share of the page area covered by images, in [0, 1].
    #[serde(default)]
    pub image_coverage: f32,
}

impl Page {
    /// An empty page with no text and no images.
    pub fn new(index: u32, kind: PageKind) -> Self {
        Self {
            index,
            kind,
            size: None,
            text: String::new(),
            blocks: Vec::new(),
            text_source: TextSource::None,
            needs_ocr: false,
            images: Vec::new(),
            image_coverage: 0.0,
        }
    }

    /// True when the page is a single image covering most of it (a scan): OCR can take the
    /// embedded image instead of rendering the page.
    pub fn is_scan(&self) -> bool {
        self.images.len() == 1 && self.image_coverage >= 0.85
    }
}

/// What an extractor makes of a file. File-level properties (producer, author, ...) are
/// facts, not fields: the extractor emits them under its own namespace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Pages in order.
    pub pages: Vec<Page>,
}

impl Document {
    /// All page texts joined with a form feed, for analyzers that do not care about pages.
    pub fn full_text(&self) -> String {
        let mut out = String::with_capacity(self.pages.iter().map(|p| p.text.len() + 1).sum());
        for (i, p) in self.pages.iter().enumerate() {
            if i > 0 {
                out.push('\u{c}');
            }
            out.push_str(&p.text);
        }
        out
    }
}

impl Artifact for Document {
    const NAME: &'static str = "document";
}

/// An improved version of the extracted [`Document`], produced by a later step (OCR filling
/// pages that had no usable text layer). Text consumers read it through
/// [`Ctx::document`](crate::Ctx::document), which prefers it over the original, and declare
/// `after(&["revised_document"])` so they run once it exists. Page indices match the original.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RevisedDocument(pub Document);

impl Artifact for RevisedDocument {
    const NAME: &'static str = "revised_document";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(y0: f32, y1: f32) -> BBox {
        BBox {
            x0: 10.0,
            y0,
            x1: 100.0,
            y1,
        }
    }

    #[test]
    fn blocks_without_lines_serialize_as_before() {
        let b = Block::new(
            Span { start: 0, end: 5 },
            BlockKind::Paragraph,
            Zone::Body,
            Some(bb(0.0, 10.0)),
        );
        let j = serde_json::to_string(&b).unwrap();
        // Byte-identical to the JSON of a block before `lines` existed.
        assert_eq!(
            j,
            r#"{"span":{"start":0,"end":5},"kind":{"kind":"paragraph"},"zone":"body","bbox":{"x0":10.0,"y0":0.0,"x1":100.0,"y1":10.0}}"#
        );
        // ... and such JSON still reads back.
        assert_eq!(serde_json::from_str::<Block>(&j).unwrap(), b);
    }

    #[test]
    fn lines_round_trip() {
        let mut b = Block::new(
            Span { start: 0, end: 11 },
            BlockKind::Paragraph,
            Zone::Body,
            Some(bb(0.0, 24.0)),
        );
        b.lines = vec![
            LineBox {
                span: Span { start: 0, end: 5 },
                bbox: bb(0.0, 12.0),
            },
            LineBox {
                span: Span { start: 6, end: 11 },
                bbox: bb(12.0, 24.0),
            },
        ];
        let j = serde_json::to_string(&b).unwrap();
        assert!(j.contains(r#""lines":[{"span":{"start":0,"end":5}"#), "{j}");
        assert_eq!(serde_json::from_str::<Block>(&j).unwrap(), b);
    }
}

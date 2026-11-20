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
/// every fact can point back into it.
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
}

/// One page of a document.
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

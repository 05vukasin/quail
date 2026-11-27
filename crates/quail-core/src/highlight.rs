//! Highlights: where a fact sits in the document text, in the block model that consumers show.
//!
//! [`Evidence`](crate::Evidence) cites a byte span of a page's text. That is exact, but a
//! reader that shows the document block by block (doewood's `GetDocumentText`, a UI that
//! highlights a passage, an agent that quotes it) needs the block and the characters inside
//! it. A [`Highlight`] carries both views of one passage:
//!
//! | field | meaning |
//! |---|---|
//! | `key` | the fact key it proves (`payment.amount`) |
//! | `page` | the page index, as [`Page::index`](crate::Page::index) |
//! | `block` | the block's index among all the document's blocks, from 0 (see below) |
//! | `start`, `end` | character range inside that block's text (Unicode scalar values, end exclusive) |
//! | `quote` | the quoted text, exactly the block text between `start` and `end` |
//! | `conf` | the fact's confidence |
//!
//! A step that emits highlights writes them as a Record fact `<namespace>.highlight` (fields
//! `key page block start end quote`, in that order; the confidence is the fact's `conf`, the
//! byte span is its evidence), one per highlighted fact value.
//!
//! # Block numbering
//!
//! [`BlockIndex`] numbers blocks the way doewood stores a document's text, so `block` is the
//! `TextBlock.index` that `GetDocumentText` returns for the same bytes and step versions:
//!
//! - pages in document order, blocks in reading order, one counter for the whole document;
//! - a block whose text is blank is skipped and takes no number;
//! - consecutive [`BlockKind::TableCell`] blocks of one table row become one block: each
//!   cell's text trimmed, after one tab per column since the row's first column (a cell out
//!   of column order still gets one tab).
//!
//! A passage that runs past its block is cut at the block's end: a highlight never spans two
//! blocks.

use crate::document::{BlockKind, Document, Span};
use crate::fact::FactKey;
use crate::value::Value;

/// One highlighted passage, see the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    /// The fact key the passage proves.
    pub key: FactKey,
    /// Page index ([`Page::index`](crate::Page::index)).
    pub page: u32,
    /// Index of the block among all the document's blocks.
    pub block: u32,
    /// First character of the passage in the block's text.
    pub start: u32,
    /// One past its last character.
    pub end: u32,
    /// The passage as written.
    pub quote: String,
    /// The fact's confidence.
    pub conf: f32,
}

impl Highlight {
    /// The record value of a `<namespace>.highlight` fact: `key page block start end quote`.
    pub fn to_value(&self) -> Value {
        Value::record([
            ("key", Value::text(self.key.as_str())),
            ("page", Value::Int(i64::from(self.page))),
            ("block", Value::Int(i64::from(self.block))),
            ("start", Value::Int(i64::from(self.start))),
            ("end", Value::Int(i64::from(self.end))),
            ("quote", Value::text(self.quote.as_str())),
        ])
    }
}

/// Where a byte span of a page lands in the block model: block index, character range and
/// the quoted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Index of the block among all the document's blocks.
    pub block: u32,
    /// First character in the block's text.
    pub start: u32,
    /// One past the last character.
    pub end: u32,
    /// The text between them.
    pub quote: String,
}

/// One numbered block: a plain block, or the merged cells of a table row.
#[derive(Clone, Debug)]
struct Unit {
    index: u32,
    /// Byte range in the page text (for a row: from the first cell to the last).
    span: Span,
    /// The cells of a merged row: each cell's trimmed byte range and the character offset
    /// where its text starts in the row text. Empty for a plain block.
    cells: Vec<(Span, u32)>,
}

/// The block numbering of one document (see the module documentation), for turning page
/// byte spans into [`Location`]s.
#[derive(Clone, Debug, Default)]
pub struct BlockIndex {
    /// Per page of the document, in order: its index and its units by start.
    pages: Vec<(u32, Vec<Unit>)>,
}

fn chars(s: &str) -> u32 {
    u32::try_from(s.chars().count()).unwrap_or(u32::MAX)
}

/// The byte range of `text[span]` without leading and trailing whitespace.
fn trimmed(text: &str, span: Span) -> Span {
    let s = span.slice(text);
    let lead = s.len() - s.trim_start().len();
    let body = s.trim();
    let start = span.start + u32::try_from(lead).unwrap_or(0);
    Span {
        start,
        end: start + u32::try_from(body.len()).unwrap_or(0),
    }
}

/// The text doewood stores for a merged row: each trimmed cell at its character offset,
/// tabs before it.
fn row_text(text: &str, cells: &[(Span, u32)]) -> String {
    let mut out = String::new();
    let mut len = 0u32;
    for &(c, offset) in cells {
        while len < offset {
            out.push('\t');
            len += 1;
        }
        let body = c.slice(text);
        out.push_str(body);
        len = len.saturating_add(chars(body));
    }
    out
}

/// A table row being merged.
struct OpenRow {
    table: u16,
    row: u16,
    span: Span,
    /// Characters of the row text so far.
    len: u32,
    blank: bool,
    next_col: Option<u16>,
    cells: Vec<(Span, u32)>,
}

impl OpenRow {
    fn push(&mut self, text: &str, span: Span, col: u16) {
        let tabs = match self.next_col {
            None => col,
            Some(next) => col.saturating_sub(next) + 1,
        };
        self.len = self.len.saturating_add(u32::from(tabs));
        let t = trimmed(text, span);
        let body = t.slice(text);
        if !body.is_empty() {
            self.blank = false;
        }
        self.cells.push((t, self.len));
        self.len = self.len.saturating_add(chars(body));
        self.span.end = self.span.end.max(span.end);
        let next = col.saturating_add(1);
        self.next_col = Some(self.next_col.map_or(next, |n| n.max(next)));
    }
}

impl BlockIndex {
    /// The numbering of `doc`'s blocks.
    pub fn new(doc: &Document) -> Self {
        let mut index: u32 = 0;
        let mut pages = Vec::with_capacity(doc.pages.len());
        let close = |row: Option<OpenRow>, units: &mut Vec<Unit>, index: &mut u32| {
            if let Some(r) = row
                && !r.blank
            {
                units.push(Unit {
                    index: *index,
                    span: r.span,
                    cells: r.cells,
                });
                *index = index.saturating_add(1);
            }
        };
        for page in &doc.pages {
            let mut units = Vec::new();
            let mut row: Option<OpenRow> = None;
            for b in &page.blocks {
                let text = b.span.slice(&page.text);
                if let BlockKind::TableCell { table, row: r, col } = b.kind {
                    match &mut row {
                        Some(open) if open.table == table && open.row == r => {
                            open.push(&page.text, b.span, col);
                        }
                        _ => {
                            close(row.take(), &mut units, &mut index);
                            let mut open = OpenRow {
                                table,
                                row: r,
                                span: b.span,
                                len: 0,
                                blank: true,
                                next_col: None,
                                cells: Vec::new(),
                            };
                            open.push(&page.text, b.span, col);
                            row = Some(open);
                        }
                    }
                    continue;
                }
                close(row.take(), &mut units, &mut index);
                if text.trim().is_empty() {
                    continue;
                }
                units.push(Unit {
                    index,
                    span: b.span,
                    cells: Vec::new(),
                });
                index = index.saturating_add(1);
            }
            close(row.take(), &mut units, &mut index);
            pages.push((page.index, units));
        }
        Self { pages }
    }

    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.pages.iter().map(|(_, u)| u.len()).sum()
    }

    /// Whether the document has no non-blank block.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where `span` of page `page` (its [`Page::index`](crate::Page::index)) lands: the
    /// block holding its first byte, the range cut to that block. In a merged table row the
    /// range may cover several cells (the quote then holds the tabs between them). `None`
    /// when the page is unknown or the span starts outside every block.
    pub fn locate(&self, doc: &Document, page: u32, span: Span) -> Option<Location> {
        let pos = self.pages.iter().position(|(i, _)| *i == page)?;
        let text = doc.pages.get(pos).map(|p| p.text.as_str())?;
        let unit = self.pages[pos]
            .1
            .iter()
            .find(|u| u.span.start <= span.start && span.start < u.span.end)?;
        if unit.cells.is_empty() {
            let s = span.start;
            let e = span.end.clamp(s, unit.span.end);
            let before = text.get(unit.span.start as usize..s as usize)?;
            let quote = text.get(s as usize..e as usize)?;
            let start = chars(before);
            return Some(Location {
                block: unit.index,
                start,
                end: start.saturating_add(chars(quote)),
                quote: quote.to_owned(),
            });
        }
        // A row: the character of a byte is the one of its cell (a byte between cells counts
        // as the start of the next cell, one past the last cell as the row's end).
        let at = |b: u32| -> Option<u32> {
            for &(cell, offset) in &unit.cells {
                if b <= cell.start {
                    return Some(offset);
                }
                if b <= cell.end {
                    let before = text.get(cell.start as usize..b as usize)?;
                    return Some(offset.saturating_add(chars(before)));
                }
            }
            unit.cells
                .last()
                .map(|&(c, o)| o.saturating_add(chars(c.slice(text))))
        };
        let start = at(span.start)?;
        let end = at(span.end.min(unit.span.end))?.max(start);
        let row = row_text(text, &unit.cells);
        let quote: String = row
            .chars()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect();
        Some(Location {
            block: unit.index,
            start,
            end,
            quote,
        })
    }

    /// A [`Highlight`] of `span` on page `page` for `key`, see [`BlockIndex::locate`].
    pub fn highlight(
        &self,
        doc: &Document,
        key: &FactKey,
        page: u32,
        span: Span,
        conf: f32,
    ) -> Option<Highlight> {
        let l = self.locate(doc, page, span)?;
        Some(Highlight {
            key: key.clone(),
            page,
            block: l.block,
            start: l.start,
            end: l.end,
            quote: l.quote,
            conf,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Block, Page, PageKind};
    use crate::fact::Zone;

    fn sp(s: u32, e: u32) -> Span {
        Span { start: s, end: e }
    }

    fn page(index: u32, text: &str, blocks: Vec<(Span, BlockKind)>) -> Page {
        let mut p = Page::new(index, PageKind::Logical);
        p.text = text.to_owned();
        p.blocks = blocks
            .into_iter()
            .map(|(s, k)| Block::new(s, k, Zone::Body, None))
            .collect();
        p
    }

    fn cell(table: u16, row: u16, col: u16) -> BlockKind {
        BlockKind::TableCell { table, row, col }
    }

    /// The text doewood stores for each numbered block, rebuilt from the units.
    #[test]
    fn numbering_skips_blank_blocks_and_merges_table_rows() {
        //          0         1         2         3
        //          0123456789012345678901234567890123456
        let text = "Title\n \nNaziv\n Cena \nUsluga\n1.000,00\nKraj";
        let p = page(
            0,
            text,
            vec![
                (sp(0, 5), BlockKind::Heading { level: 1 }),
                (sp(6, 7), BlockKind::Paragraph),
                (sp(8, 13), cell(0, 0, 0)),
                (sp(14, 20), cell(0, 0, 1)),
                (sp(21, 27), cell(0, 1, 0)),
                (sp(28, 36), cell(0, 1, 2)),
                (sp(37, 41), BlockKind::Paragraph),
            ],
        );
        let doc = Document { pages: vec![p] };
        let ix = BlockIndex::new(&doc);
        assert_eq!(ix.len(), 4);
        // "Title" is block 0, the blank paragraph takes no number.
        let l = ix.locate(&doc, 0, sp(0, 5)).unwrap();
        assert_eq!(
            (l.block, l.start, l.end, l.quote.as_str()),
            (0, 0, 5, "Title")
        );
        // Row 0 is "Naziv\tCena": the second cell starts at character 6.
        let l = ix.locate(&doc, 0, sp(15, 19)).unwrap();
        assert_eq!(
            (l.block, l.start, l.end, l.quote.as_str()),
            (1, 6, 10, "Cena")
        );
        // Row 1 is "Usluga\t\t1.000,00": column 2 after two tabs.
        let l = ix.locate(&doc, 0, sp(28, 36)).unwrap();
        assert_eq!((l.block, l.start, l.end), (2, 8, 16));
        assert_eq!(l.quote, "1.000,00");
        // A span over the whole row quotes it with its tabs.
        let l = ix.locate(&doc, 0, sp(21, 36)).unwrap();
        assert_eq!((l.block, l.start, l.end), (2, 0, 16));
        assert_eq!(l.quote, "Usluga\t\t1.000,00");
        let l = ix.locate(&doc, 0, sp(37, 41)).unwrap();
        assert_eq!(l.block, 3);
    }

    #[test]
    fn counts_characters_not_bytes_and_cuts_at_the_block_end() {
        let text = "Предмет уговора је\nодржавање";
        let first = u32::try_from("Предмет уговора је".len()).unwrap();
        let total = u32::try_from(text.len()).unwrap();
        let p0 = page(
            3,
            text,
            vec![
                (sp(0, first), BlockKind::Paragraph),
                (sp(first + 1, total), BlockKind::Paragraph),
            ],
        );
        let doc = Document {
            pages: vec![page(2, "x", vec![(sp(0, 1), BlockKind::Paragraph)]), p0],
        };
        let ix = BlockIndex::new(&doc);
        let start = u32::try_from("Предмет ".len()).unwrap();
        let l = ix.locate(&doc, 3, sp(start, total)).unwrap();
        assert_eq!((l.block, l.start, l.end), (1, 8, 18));
        assert_eq!(l.quote, "уговора је");
        let h = ix
            .highlight(&doc, &FactKey::new("payment.subject"), 3, sp(0, 4), 0.5)
            .unwrap();
        assert_eq!(
            h.to_value().field("key").and_then(Value::as_text),
            Some("payment.subject")
        );
        assert!(ix.locate(&doc, 9, sp(0, 1)).is_none());
        // A span that starts between blocks is in none.
        assert!(ix.locate(&doc, 3, sp(first, first + 1)).is_none());
    }
}

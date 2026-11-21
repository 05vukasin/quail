//! Layout and structure sketches: what a document looks like, independent of its words.
//!
//! # `sketch.layout` (geometry; stable definition)
//!
//! For the first [`LAYOUT_PAGES`] pages with a size, every block with a box and every placed
//! image becomes a feature, hashed with `quail_text::shingle::hash_bytes`:
//!
//! - page class: 0 for the first page, 1 for the second, 2 for any later one (more pages of
//!   the same form do not move the sketch much);
//! - block: `[b'B', page class, x0/8, x1/8, y0/12, height class, kind, zone]`, where `x/8`
//!   is the position in eighths of the page width (0..=7), `y0/12` in twelfths of its
//!   height, height class `0..=5` for < 10, < 20, < 40, < 80, < 160, ≥ 160 points;
//!   plus a column feature `[b'C', page class, x0/8, x1/8, kind, zone]`;
//! - image: `[b'I', page class, x0/8, y0/8, x1/8, y1/8]`, weight 2 (letterheads, logos,
//!   stamps are strong template signals);
//! - page: `[b'S', page class, orientation, size class]`, weight 4.
//!
//! The weighted features are signed with [`crate::simhash::simhash`]. Edited text moves the
//! blocks below the edit by a line or two, which changes few quantized features; a different
//! form changes most of them. Compare with [`crate::hamming`] ≤ [`LAYOUT_HAMMING_MAX`].
//! Emitted only with at least [`MIN_LAYOUT_BLOCKS`] boxed blocks.
//!
//! # `sketch.structure` (block-kind sequence; stable definition)
//!
//! Blocks in reading order become symbols: heading (with its level, 1..=6), list item,
//! paragraph, note, each with its zone; a run of cells of one table becomes one table symbol
//! with its row count class (1, 2, 3–4, 5–8, 9–16, > 16) and column count (≤ 8);
//! annotations (comments, tracked deletions) are skipped. Consecutive equal symbols collapse
//! into one with a run class (1, 2–3, 4–7, 8–15, ≥ 16). The features are the symbols and the
//! 3-grams of symbols, weighted by count, signed with SimHash. Compare with
//! [`crate::hamming`] ≤ [`STRUCTURE_HAMMING_MAX`]. Emitted only when the sequence has at
//! least [`MIN_STRUCTURE_KINDS`] distinct symbols of at least two block kinds: a PDF whose
//! blocks are all paragraphs (in header, body and footer zones) has no structure to compare;
//! there the geometry sketch speaks.
//!
//! # `sketch.headings`
//!
//! [`crate::compat::structural_fp`] over the heading blocks' texts: equal skeletons, equal
//! values. Emitted with at least two headings.

use quail_core::{Block, BlockKind, Document, Page, Zone};
use quail_text::shingle::hash_bytes;

use crate::simhash::{simhash, simhash_counted};

/// Pages the geometry sketch reads.
pub const LAYOUT_PAGES: usize = 8;
/// Fewest boxed blocks for a geometry sketch.
pub const MIN_LAYOUT_BLOCKS: usize = 3;
/// Fewest distinct symbols for a structure sketch (also needed: two block kinds).
pub const MIN_STRUCTURE_KINDS: usize = 3;
/// Blocks the structure sketch reads.
pub const MAX_STRUCTURE_BLOCKS: usize = 20_000;
/// Hamming distance (of 64) at or below which two `sketch.layout` values are one layout.
pub const LAYOUT_HAMMING_MAX: u32 = 6;
/// Hamming distance (of 64) at or below which two `sketch.structure` values are one
/// structure.
pub const STRUCTURE_HAMMING_MAX: u32 = 8;

fn zone_code(z: Zone) -> u8 {
    match z {
        Zone::Unknown => 0,
        Zone::Header => 1,
        Zone::Footer => 2,
        Zone::Body => 3,
        Zone::Title => 4,
        Zone::Table => 5,
        Zone::Signature => 6,
    }
}

fn kind_code(k: BlockKind) -> (u8, u8) {
    match k {
        BlockKind::Paragraph => (1, 0),
        BlockKind::Heading { level } => (2, level.clamp(1, 6)),
        BlockKind::ListItem => (3, 0),
        BlockKind::TableCell { .. } => (4, 0),
        BlockKind::Note => (5, 0),
        BlockKind::Annotation => (6, 0),
    }
}

/// Bucket `v / extent` into `0..n`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn bucket(v: f32, extent: f32, n: u8) -> u8 {
    if extent.is_nan() || extent <= 0.0 || !v.is_finite() {
        return 0;
    }
    let x = (v / extent).clamp(0.0, 1.0) * f32::from(n);
    (x as u8).min(n - 1)
}

fn height_class(h: f32) -> u8 {
    match h {
        h if h.is_nan() || h < 10.0 => 0,
        h if h < 20.0 => 1,
        h if h < 40.0 => 2,
        h if h < 80.0 => 3,
        h if h < 160.0 => 4,
        _ => 5,
    }
}

fn page_features(page: &Page, class: u8, out: &mut Vec<(u64, u32)>) -> usize {
    let Some((w, h)) = page.size.filter(|(w, h)| *w > 0.0 && *h > 0.0) else {
        return 0;
    };
    let orientation = u8::from(w > h);
    // A4 (595×842), Letter (612×792), anything else.
    let (short, long) = if w > h { (h, w) } else { (w, h) };
    let size = if (short - 595.0).abs() < 12.0 && (long - 842.0).abs() < 12.0 {
        1
    } else if (short - 612.0).abs() < 12.0 && (long - 792.0).abs() < 12.0 {
        2
    } else {
        0
    };
    out.push((hash_bytes(&[b'S', class, orientation, size]), 4));
    let mut blocks = 0;
    for b in &page.blocks {
        let Some(bb) = b.bbox else { continue };
        let (k, _) = kind_code(b.kind);
        let z = zone_code(b.zone);
        let x0 = bucket(bb.x0, w, 8);
        let x1 = bucket(bb.x1, w, 8);
        let y0 = bucket(bb.y0, h, 12);
        let hc = height_class(bb.y1 - bb.y0);
        out.push((hash_bytes(&[b'B', class, x0, x1, y0, hc, k, z]), 1));
        out.push((hash_bytes(&[b'C', class, x0, x1, k, z]), 1));
        blocks += 1;
    }
    for im in &page.images {
        let bb = im.bbox;
        let f = [
            b'I',
            class,
            bucket(bb.x0, w, 8),
            bucket(bb.y0, h, 8),
            bucket(bb.x1, w, 8),
            bucket(bb.y1, h, 8),
        ];
        out.push((hash_bytes(&f), 2));
    }
    blocks
}

/// `sketch.layout` of a document and the number of boxed blocks it used; `None` below
/// [`MIN_LAYOUT_BLOCKS`].
pub fn geometry_sketch(doc: &Document) -> Option<(u64, usize)> {
    let mut features = Vec::new();
    let mut blocks = 0;
    for (i, page) in doc.pages.iter().take(LAYOUT_PAGES).enumerate() {
        let class = u8::try_from(i.min(2)).unwrap_or(2);
        blocks += page_features(page, class, &mut features);
    }
    (blocks >= MIN_LAYOUT_BLOCKS).then(|| (simhash(features), blocks))
}

fn rows_class(rows: u16) -> u8 {
    match rows {
        0..=1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        _ => 5,
    }
}

fn run_class(n: usize) -> u8 {
    match n {
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        8..=15 => 3,
        _ => 4,
    }
}

/// One structure symbol: kind, parameter, zone.
type Symbol = [u8; 3];

/// The structure symbols of a document's blocks in reading order (before run collapsing).
pub fn structure_symbols(doc: &Document) -> Vec<Symbol> {
    let mut out: Vec<Symbol> = Vec::new();
    let mut seen = 0usize;
    for page in &doc.pages {
        // (table index, max row, max col) of the table being read.
        let mut table: Option<(u16, u16, u16, u8)> = None;
        let flush = |t: &mut Option<(u16, u16, u16, u8)>, out: &mut Vec<Symbol>| {
            if let Some((_, rows, cols, z)) = t.take() {
                let c = u8::try_from(cols.saturating_add(1).min(8)).unwrap_or(8);
                out.push([4, rows_class(rows.saturating_add(1)) << 4 | c, z]);
            }
        };
        for b in &page.blocks {
            seen += 1;
            if seen > MAX_STRUCTURE_BLOCKS {
                break;
            }
            let Block { kind, zone, .. } = *b;
            if let BlockKind::TableCell {
                table: t, row, col, ..
            } = kind
            {
                match &mut table {
                    Some((ti, r, c, _)) if *ti == t => {
                        *r = (*r).max(row);
                        *c = (*c).max(col);
                    }
                    _ => {
                        flush(&mut table, &mut out);
                        table = Some((t, row, col, zone_code(zone)));
                    }
                }
                continue;
            }
            flush(&mut table, &mut out);
            if matches!(kind, BlockKind::Annotation) {
                continue;
            }
            let (k, p) = kind_code(kind);
            out.push([k, p, zone_code(zone)]);
        }
        flush(&mut table, &mut out);
    }
    out
}

/// `sketch.structure` of a document and the number of collapsed symbols; `None` when the
/// sequence has fewer than [`MIN_STRUCTURE_KINDS`] distinct symbols.
pub fn structure_sketch(doc: &Document) -> Option<(u64, usize)> {
    let symbols = structure_symbols(doc);
    let mut distinct = symbols.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let mut kinds: Vec<u8> = distinct.iter().map(|s| s[0]).collect();
    kinds.dedup();
    if distinct.len() < MIN_STRUCTURE_KINDS || kinds.len() < 2 {
        return None;
    }
    let runs: Vec<[u8; 4]> = symbols
        .chunk_by(|a, b| a == b)
        .map(|run| [run[0][0], run[0][1], run[0][2], run_class(run.len())])
        .collect();
    let mut hashes: Vec<u64> = runs.iter().map(|r| hash_bytes(r)).collect();
    let mut buf = Vec::with_capacity(12);
    for w in runs.windows(3) {
        buf.clear();
        for r in w {
            buf.extend_from_slice(r);
        }
        hashes.push(hash_bytes(&buf));
    }
    Some((simhash_counted(&mut hashes), runs.len()))
}

/// Heading texts of a document in reading order.
pub fn headings(doc: &Document) -> Vec<&str> {
    doc.pages
        .iter()
        .flat_map(|p| {
            p.blocks
                .iter()
                .filter(|b| matches!(b.kind, BlockKind::Heading { .. }))
                .map(|b| b.span.slice(&p.text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_clamped() {
        assert_eq!(bucket(-5.0, 100.0, 8), 0);
        assert_eq!(bucket(100.0, 100.0, 8), 7);
        assert_eq!(bucket(f32::NAN, 100.0, 8), 0);
        assert_eq!(bucket(50.0, 0.0, 8), 0);
        assert_eq!(height_class(f32::NAN), 0);
        assert_eq!(height_class(500.0), 5);
    }
}

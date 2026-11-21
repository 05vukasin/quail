//! Tables rebuilt from PDF geometry.
//!
//! `quail-pdf` emits paragraph blocks (lines joined by `\n`) with one box per block and no
//! table structure. Table cells far apart on one baseline end up in separate blocks; a
//! column of cells stacks into one block. This module:
//!
//! 1. splits blocks into lines and estimates each line's box: the block's height divided
//!    evenly, the width in proportion to the line's length (numbers right-aligned, text
//!    left-aligned);
//! 2. groups lines into rows by vertical centre;
//! 3. finds table regions: a header row (two or more column roles) or a run of rows with
//!    several cells and amounts, plus the short text rows between them (wrapped
//!    descriptions, section titles) and the total rows after them;
//! 4. finds columns as x-ranges covered by cells in most rows (a projection profile, so one
//!    merged cell does not merge two columns), splitting a cell that spans several columns
//!    by the estimated position of its words;
//! 5. hands each region to [`crate::table::parse`] as a [`Grid`].
//!
//! Rows outside every region go to the text-line parser.

use quail_core::{BBox, Page, Span};

use crate::header;
use crate::item::{DocHints, Source, span_union, union};
use crate::num;
use crate::table::{GCell, Grid};

/// A line of text with its (estimated) box.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub text: String,
    pub span: Span,
    pub bbox: BBox,
}

/// A row: lines on one baseline, left to right.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub lines: Vec<Line>,
    pub y0: f32,
    pub y1: f32,
}

impl Row {
    fn h(&self) -> f32 {
        (self.y1 - self.y0).max(1.0)
    }

    /// The row as one text line: cells joined by two spaces.
    pub fn joined(&self) -> (String, Span, Option<BBox>) {
        let mut t = String::new();
        let mut span = Span { start: 0, end: 0 };
        let mut bbox = None;
        for l in &self.lines {
            if !t.is_empty() {
                t.push_str("  ");
            }
            t.push_str(&l.text);
            span = span_union(span, l.span);
            bbox = union(bbox, Some(l.bbox));
        }
        (t, span, bbox)
    }
}

/// Most rows, columns and regions handled per page (hostile layouts).
const MAX_LINES: usize = 20_000;
const MAX_BANDS: usize = 24;

fn numeric_line(t: &str) -> bool {
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let letters = t.chars().filter(|c| c.is_alphabetic()).count();
    digits > 0 && letters <= 4
}

/// Lines of a page with estimated boxes.
pub(crate) fn lines(page: &Page) -> Vec<Line> {
    let mut out = Vec::new();
    for b in &page.blocks {
        let Some(bb) = b.bbox else { continue };
        let text = b.span.slice(&page.text);
        let parts: Vec<(usize, &str)> = {
            let mut v = Vec::new();
            let mut off = 0;
            for l in text.split('\n') {
                v.push((off, l));
                off += l.len() + 1;
            }
            v
        };
        let n = parts.len().max(1);
        #[allow(clippy::cast_precision_loss)]
        let pitch = (bb.y1 - bb.y0) / n as f32;
        let longest = parts
            .iter()
            .map(|(_, l)| l.trim().chars().count())
            .max()
            .unwrap_or(1)
            .max(1);
        for (i, (off, l)) in parts.iter().enumerate() {
            let trimmed = l.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lead = l.len() - l.trim_start().len();
            let start = b.span.start as usize + off + lead;
            #[allow(clippy::cast_precision_loss)]
            let (y0, w) = (
                bb.y0 + pitch * i as f32,
                (bb.x1 - bb.x0) * trimmed.chars().count() as f32 / longest as f32,
            );
            let (x0, x1) = if n > 1 && numeric_line(trimmed) {
                (bb.x1 - w, bb.x1)
            } else if n > 1 {
                (bb.x0, bb.x0 + w)
            } else {
                (bb.x0, bb.x1)
            };
            out.push(Line {
                text: trimmed.to_owned(),
                span: Span {
                    start: u32::try_from(start).unwrap_or(u32::MAX),
                    end: u32::try_from(start + trimmed.len()).unwrap_or(u32::MAX),
                },
                bbox: BBox {
                    x0,
                    y0,
                    x1,
                    y1: y0 + pitch,
                },
            });
            if out.len() >= MAX_LINES {
                return out;
            }
        }
    }
    out
}

/// Lines grouped into rows, top to bottom.
pub(crate) fn rows(mut lines: Vec<Line>) -> Vec<Row> {
    let yc = |l: &Line| (l.bbox.y0 + l.bbox.y1) / 2.0;
    lines.sort_by(|a, b| yc(a).total_cmp(&yc(b)));
    let mut rows: Vec<Row> = Vec::new();
    for l in lines {
        let c = yc(&l);
        let h = (l.bbox.y1 - l.bbox.y0).max(1.0);
        if let Some(r) = rows.last_mut() {
            let rc = (r.y0 + r.y1) / 2.0;
            if (c - rc).abs() <= 0.5 * h.min(r.h()) {
                r.y0 = r.y0.min(l.bbox.y0);
                r.y1 = r.y1.max(l.bbox.y1);
                r.lines.push(l);
                continue;
            }
        }
        rows.push(Row {
            y0: l.bbox.y0,
            y1: l.bbox.y1,
            lines: vec![l],
        });
    }
    for r in &mut rows {
        r.lines.sort_by(|a, b| a.bbox.x0.total_cmp(&b.bbox.x0));
    }
    rows
}

fn is_header_row(r: &Row) -> bool {
    if r.lines.len() < 2 {
        return false;
    }
    let roles: Vec<_> = r.lines.iter().filter_map(|l| header::role(&l.text)).collect();
    let mut kinds: Vec<u8> = roles.iter().map(|h| h.role as u8).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds.len() >= 2
        && roles.iter().any(|h| {
            matches!(
                h.role,
                header::Role::UnitPrice | header::Role::Total
            )
        })
}

fn has_money(r: &Row, hints: &DocHints) -> bool {
    r.lines.iter().any(|l| {
        num::scan(&l.text, hints.dec, hints.order)
            .iter()
            .any(num::NumTok::moneylike)
    })
}

/// A short all-text row with several cells: the column labels of a table whose header
/// names no known role (`Title | Onshore | Offshore`).
fn label_row(r: &Row) -> bool {
    r.lines.len() >= 2
        && r.lines.iter().all(|l| {
            l.text.chars().count() <= 40
                && l.text.chars().any(char::is_alphabetic)
                && !l.text.chars().any(|c| c.is_ascii_digit())
        })
}

fn has_amount(r: &Row, hints: &DocHints) -> bool {
    let mut plain = 0;
    for l in &r.lines {
        for t in num::scan(&l.text, hints.dec, hints.order) {
            if t.moneylike() {
                return true;
            }
            if t.number().is_some() && !t.glued {
                plain += 1;
            }
        }
    }
    plain >= 2
}

fn prose(r: &Row) -> bool {
    r.lines.iter().any(|l| l.text.chars().count() > 150)
}

struct Region {
    rows: Vec<usize>,
    header: Option<usize>,
    data: usize,
}

/// Table regions of a page and the rows left over.
pub(crate) fn tables(page: &Page, hints: &DocHints) -> (Vec<Grid>, Vec<Row>) {
    let rows = rows(lines(page));
    let mut regions: Vec<Region> = Vec::new();
    let mut cur: Option<Region> = None;
    let mut quiet = 0usize;
    let close = |cur: &mut Option<Region>, regions: &mut Vec<Region>, rows: &[Row]| {
        if let Some(mut r) = cur.take() {
            // Drop trailing rows without amounts.
            while let Some(&last) = r.rows.last() {
                if Some(last) != r.header && !has_amount(&rows[last], hints) {
                    r.rows.pop();
                } else {
                    break;
                }
            }
            if (r.header.is_some() && r.data >= 1) || r.data >= 2 {
                regions.push(r);
            }
        }
    };
    for (i, r) in rows.iter().enumerate() {
        let gap_ok = cur.as_ref().is_some_and(|c| {
            c.rows
                .last()
                .and_then(|&l| rows.get(l))
                .is_some_and(|last| r.y0 - last.y1 <= 3.0 * last.h().max(r.h()))
        });
        if is_header_row(r) {
            close(&mut cur, &mut regions, &rows);
            cur = Some(Region {
                rows: vec![i],
                header: Some(i),
                data: 0,
            });
            quiet = 0;
            continue;
        }
        // Under a header plain numbers make a data row; without one, money must be there.
        let headed = cur.as_ref().is_some_and(|c| c.header.is_some()) && gap_ok;
        let amount = if headed { has_amount(r, hints) } else { has_money(r, hints) };
        let data = !prose(r) && amount && (r.lines.len() >= 2 || cur.is_some() && gap_ok);
        if data {
            if let Some(c) = cur.as_mut().filter(|_| gap_ok) {
                c.rows.push(i);
                c.data += 1;
            } else {
                close(&mut cur, &mut regions, &rows);
                // Column labels right above the first row.
                let above = i
                    .checked_sub(1)
                    .and_then(|k| rows.get(k).map(|a| (k, a)))
                    .filter(|(k, a)| {
                        label_row(a)
                            && r.y0 - a.y1 <= 2.0 * a.h().max(r.h())
                            && regions.last().is_none_or(|g| !g.rows.contains(k))
                    })
                    .map(|(k, _)| k);
                let mut first = Vec::with_capacity(2);
                first.extend(above);
                first.push(i);
                cur = Some(Region {
                    rows: first,
                    header: None,
                    data: 1,
                });
            }
            quiet = 0;
        } else if cur.is_some() && gap_ok && !prose(r) && quiet < 3 {
            if let Some(c) = cur.as_mut() {
                c.rows.push(i);
            }
            quiet += 1;
        } else {
            close(&mut cur, &mut regions, &rows);
            quiet = 0;
        }
    }
    close(&mut cur, &mut regions, &rows);

    let mut used = vec![false; rows.len()];
    let mut grids = Vec::new();
    for reg in &regions {
        if let Some(g) = grid_of(page, &rows, reg) {
            for &i in &reg.rows {
                used[i] = true;
            }
            grids.push(g);
        }
    }
    let rest = rows
        .into_iter()
        .zip(used)
        .filter(|(_, u)| !u)
        .map(|(r, _)| r)
        .collect();
    (grids, rest)
}

fn grid_of(page: &Page, rows: &[Row], reg: &Region) -> Option<Grid> {
    let width = page
        .size
        .map_or(0.0, |s| s.0)
        .max(rows.iter().flat_map(|r| r.lines.iter()).map(|l| l.bbox.x1).fold(0.0, f32::max));
    if !(width.is_finite() && width > 0.0) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bins = (width.ceil() as usize).clamp(1, 5_000);
    let mut cover = vec![0u32; bins + 1];
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bin = |x: f32| (x.max(0.0) as usize).min(bins);
    let multi: Vec<usize> = reg
        .rows
        .iter()
        .copied()
        .filter(|&i| Some(i) != reg.header && rows[i].lines.len() >= 2)
        .collect();
    for &i in &multi {
        for l in &rows[i].lines {
            for c in cover.iter_mut().take(bin(l.bbox.x1) + 1).skip(bin(l.bbox.x0)) {
                *c += 1;
            }
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    let threshold = (multi.len() as u32).div_ceil(4).max(1);
    if let Some(h) = reg.header {
        for l in &rows[h].lines {
            for c in cover.iter_mut().take(bin(l.bbox.x1) + 1).skip(bin(l.bbox.x0)) {
                *c += threshold;
            }
        }
    }
    let mut bands: Vec<(f32, f32)> = Vec::new();
    let mut open: Option<usize> = None;
    for (x, &c) in cover.iter().enumerate() {
        match (c >= threshold, open) {
            (true, None) => open = Some(x),
            (false, Some(s)) => {
                #[allow(clippy::cast_precision_loss)]
                bands.push((s as f32, x as f32));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(s) = open {
        #[allow(clippy::cast_precision_loss)]
        bands.push((s as f32, bins as f32));
    }
    if bands.len() < 2 || bands.len() > MAX_BANDS {
        return None;
    }
    let mut grid_rows = Vec::with_capacity(reg.rows.len());
    for &i in &reg.rows {
        let mut cells: Vec<Option<GCell>> = vec![None; bands.len()];
        for l in &rows[i].lines {
            place(l, &bands, &mut cells);
        }
        grid_rows.push(cells);
    }
    Some(Grid {
        page: page.index,
        ncols: bands.len(),
        rows: grid_rows,
        source: Source::Layout,
    })
}

fn overlap(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.1.min(b.1) - a.0.max(b.0)).max(0.0)
}

fn put(cells: &mut [Option<GCell>], band: usize, text: &str, span: Span, bbox: BBox) {
    let Some(slot) = cells.get_mut(band) else { return };
    match slot {
        Some(c) => {
            c.text.push(' ');
            c.text.push_str(text);
            c.span = span_union(c.span, span);
            c.bbox = union(c.bbox, Some(bbox));
        }
        None => {
            *slot = Some(GCell {
                text: text.to_owned(),
                span,
                bbox: Some(bbox),
            });
        }
    }
}

/// Puts a line into its column, or splits it over several columns by word position.
fn place(l: &Line, bands: &[(f32, f32)], cells: &mut [Option<GCell>]) {
    let lx = (l.bbox.x0, l.bbox.x1);
    let w = (lx.1 - lx.0).max(1.0);
    let hit: Vec<usize> = bands
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            let o = overlap(lx, **b);
            o > 0.25 * w || o > 0.5 * (b.1 - b.0)
        })
        .map(|(i, _)| i)
        .collect();
    if hit.len() <= 1 {
        let band = hit.first().copied().unwrap_or_else(|| nearest(bands, (lx.0 + lx.1) / 2.0));
        put(cells, band, &l.text, l.span, l.bbox);
        return;
    }
    // Several columns: place each word by its estimated centre.
    let n = l.text.chars().count().max(1);
    #[allow(clippy::cast_precision_loss)]
    let per = w / n as f32;
    let mut chars_before = 0usize;
    let mut offset = 0usize;
    for word in l.text.split(' ') {
        let wl = word.chars().count();
        if !word.is_empty() {
            #[allow(clippy::cast_precision_loss)]
            let cx = lx.0 + per * (chars_before as f32 + wl as f32 / 2.0);
            let band = nearest(bands, cx);
            let start = l.span.start as usize + offset;
            let span = Span {
                start: u32::try_from(start).unwrap_or(u32::MAX),
                end: u32::try_from(start + word.len()).unwrap_or(u32::MAX),
            };
            #[allow(clippy::cast_precision_loss)]
            let bbox = BBox {
                x0: lx.0 + per * chars_before as f32,
                x1: lx.0 + per * (chars_before + wl) as f32,
                ..l.bbox
            };
            put(cells, band, word, span, bbox);
        }
        chars_before += wl + 1;
        offset += word.len() + 1;
    }
}

fn nearest(bands: &[(f32, f32)], x: f32) -> usize {
    bands
        .iter()
        .enumerate()
        .min_by(|a, b| {
            let d = |b: &(f32, f32)| {
                if x < b.0 {
                    b.0 - x
                } else if x > b.1 {
                    x - b.1
                } else {
                    0.0
                }
            };
            d(a.1).total_cmp(&d(b.1))
        })
        .map_or(0, |(i, _)| i)
}

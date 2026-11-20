//! From positioned glyphs to lines, paragraph blocks and reading order.
//!
//! Coordinates are PDF points from the page's top-left corner, y growing downwards (the
//! orientation of [`BBox`]). The algorithm is deliberately simple and linear-ish:
//!
//! 1. Glyphs are bucketed into lines by baseline (tolerance relative to the font size).
//! 2. A line is split into segments at wide horizontal gaps (columns, table cells).
//! 3. Segments stack into paragraph blocks when they are close vertically and overlap
//!    horizontally.
//! 4. Blocks are ordered with a recursive XY-cut: split at the widest horizontal gap first
//!    (rows), else the widest vertical gap (columns), so two-column text reads column by column.

use quail_core::BBox;

/// One glyph with its Unicode text and box.
#[derive(Clone, Debug)]
pub(crate) struct Glyph {
    /// Unicode text (usually one char, ligatures give several).
    pub text: String,
    /// Left edge.
    pub x0: f32,
    /// Right edge (origin + advance).
    pub x1: f32,
    /// Baseline y.
    pub baseline: f32,
    /// Font size in points (height of the em square on the page).
    pub size: f32,
}

/// A line segment: glyphs on one baseline without a wide gap.
#[derive(Clone, Debug)]
pub(crate) struct Segment {
    pub text: String,
    pub bbox: BBox,
    pub size: f32,
}

/// A paragraph-like group of segments.
#[derive(Clone, Debug)]
pub(crate) struct LayoutBlock {
    /// Lines of the block, top to bottom.
    pub lines: Vec<Segment>,
    pub bbox: BBox,
}

impl LayoutBlock {
    /// Text of the block: its lines joined by `\n`.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&l.text);
        }
        s
    }
}

/// Space inserted when the gap to the previous glyph exceeds this many ems.
const SPACE_GAP_EM: f32 = 0.18;
/// A segment ends at a gap of this many ems.
const SEGMENT_GAP_EM: f32 = 2.2;
/// Baselines closer than this many ems belong to the same line.
const SAME_LINE_EM: f32 = 0.45;
/// Segments stack into one block when the vertical gap is below this many ems.
const PARA_GAP_EM: f32 = 0.9;
/// XY-cut recursion bound (a hostile page cannot blow the stack).
const MAX_CUT_DEPTH: u32 = 32;

/// Lays out a page's glyphs into blocks in reading order.
pub(crate) fn layout(mut glyphs: Vec<Glyph>) -> Vec<LayoutBlock> {
    glyphs.retain(|g| g.x0.is_finite() && g.x1.is_finite() && g.baseline.is_finite());
    for g in &mut glyphs {
        if !(g.size.is_finite() && g.size > 0.1) {
            g.size = 1.0;
        }
    }
    let segments = segments(glyphs);
    let blocks = blocks(segments);
    let mut order = Vec::with_capacity(blocks.len());
    let idx: Vec<usize> = (0..blocks.len()).collect();
    xy_cut(&blocks, idx, 0, &mut order);
    let mut slots: Vec<Option<LayoutBlock>> = blocks.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|i| slots.get_mut(i).and_then(Option::take))
        .collect()
}

fn segments(mut glyphs: Vec<Glyph>) -> Vec<Segment> {
    // Content streams usually emit text in a sensible order already; a stable sort by
    // baseline keeps that order inside a line when x ties.
    glyphs.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    for g in glyphs {
        if let Some(line) = lines.last_mut()
            && let Some(first) = line.first()
            && (g.baseline - first.baseline).abs() <= SAME_LINE_EM * first.size.max(g.size)
        {
            line.push(g);
            continue;
        }
        lines.push(vec![g]);
    }
    let mut out = Vec::new();
    for mut line in lines {
        line.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        let mut cur: Option<(String, BBox, f32, f32)> = None; // text, bbox, size, last x1
        for g in line {
            let top = g.baseline - 0.8 * g.size;
            let bottom = g.baseline + 0.2 * g.size;
            if let Some((text, bbox, size, last_x1)) = cur.as_mut() {
                let gap = g.x0 - *last_x1;
                let em = size.max(g.size);
                if gap > SEGMENT_GAP_EM * em {
                    if let Some((t, b, s, _)) = cur.take() {
                        push_segment(&mut out, &t, b, s);
                    }
                } else {
                    // Overlapping duplicates (fake bold: the same glyph drawn twice with a tiny
                    // offset) are dropped.
                    if gap < -0.3 * em && text.ends_with(g.text.as_str()) {
                        continue;
                    }
                    if gap > SPACE_GAP_EM * em
                        && !text.ends_with(char::is_whitespace)
                        && !g.text.starts_with(char::is_whitespace)
                    {
                        text.push(' ');
                    }
                    text.push_str(&g.text);
                    bbox.x1 = bbox.x1.max(g.x1);
                    bbox.y0 = bbox.y0.min(top);
                    bbox.y1 = bbox.y1.max(bottom);
                    *last_x1 = g.x1.max(*last_x1);
                    continue;
                }
            }
            let bbox = BBox {
                x0: g.x0,
                y0: top,
                x1: g.x1,
                y1: bottom,
            };
            cur = Some((g.text, bbox, g.size, g.x1));
        }
        if let Some((t, b, s, _)) = cur {
            push_segment(&mut out, &t, b, s);
        }
    }
    out
}

fn push_segment(out: &mut Vec<Segment>, text: &str, bbox: BBox, size: f32) {
    let text = text.trim().to_owned();
    if !text.is_empty() {
        out.push(Segment { text, bbox, size });
    }
}

fn blocks(segments: Vec<Segment>) -> Vec<LayoutBlock> {
    let mut blocks: Vec<LayoutBlock> = Vec::new();
    // Segments arrive top to bottom (line order) and left to right within a line. Each joins
    // the most recent open block it continues, so columns grow side by side.
    for s in segments {
        let target = blocks.iter().rposition(|b| continues(b, &s));
        match target.and_then(|i| blocks.get_mut(i)) {
            Some(b) => {
                b.bbox = union(b.bbox, s.bbox);
                b.lines.push(s);
            }
            None => blocks.push(LayoutBlock {
                bbox: s.bbox,
                lines: vec![s],
            }),
        }
    }
    blocks
}

fn continues(b: &LayoutBlock, s: &Segment) -> bool {
    let Some(last) = b.lines.last() else {
        return false;
    };
    let em = last.size.max(s.size);
    let gap = s.bbox.y0 - last.bbox.y1;
    let similar_size = (last.size - s.size).abs() <= 0.25 * em;
    let overlap = s.bbox.x0.max(b.bbox.x0) < s.bbox.x1.min(b.bbox.x1);
    gap > -0.3 * em && gap < PARA_GAP_EM * em && similar_size && overlap
}

fn union(a: BBox, b: BBox) -> BBox {
    BBox {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

/// Recursive XY-cut over block indices, appending them in reading order.
fn xy_cut(blocks: &[LayoutBlock], mut idx: Vec<usize>, depth: u32, out: &mut Vec<usize>) {
    let bbox_of = |i: usize| blocks.get(i).map(|b| b.bbox);
    if idx.len() <= 1 || depth >= MAX_CUT_DEPTH {
        idx.sort_by(|&a, &b| {
            let (Some(x), Some(y)) = (bbox_of(a), bbox_of(b)) else {
                return std::cmp::Ordering::Equal;
            };
            x.y0.total_cmp(&y.y0).then(x.x0.total_cmp(&y.x0))
        });
        out.extend(idx);
        return;
    }
    // Horizontal cut (rows) first: the reading order of a page is top to bottom.
    if let Some((a, b)) = split(&idx, |i| bbox_of(i).map(|r| (r.y0, r.y1)), 0.0) {
        xy_cut(blocks, a, depth + 1, out);
        xy_cut(blocks, b, depth + 1, out);
        return;
    }
    // Vertical cut (columns): require a real gutter, not a word gap.
    if let Some((a, b)) = split(&idx, |i| bbox_of(i).map(|r| (r.x0, r.x1)), 4.0) {
        xy_cut(blocks, a, depth + 1, out);
        xy_cut(blocks, b, depth + 1, out);
        return;
    }
    xy_cut(blocks, idx, MAX_CUT_DEPTH, out);
}

/// Splits `idx` at the widest gap in the projection given by `span`, if there is one wider
/// than `min_gap`. Returns (before, after).
fn split(
    idx: &[usize],
    span: impl Fn(usize) -> Option<(f32, f32)>,
    min_gap: f32,
) -> Option<(Vec<usize>, Vec<usize>)> {
    let mut items: Vec<(f32, f32, usize)> = idx
        .iter()
        .filter_map(|&i| span(i).map(|(a, b)| (a, b, i)))
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best: Option<(f32, f32)> = None; // (gap, cut position)
    let mut reach = f32::NEG_INFINITY;
    for w in items.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
            continue;
        };
        reach = reach.max(a.1);
        let gap = b.0 - reach;
        if gap > min_gap && best.is_none_or(|(g, _)| gap > g) {
            best = Some((gap, b.0));
        }
    }
    let (_, cut) = best?;
    let (before, after): (Vec<_>, Vec<_>) = items.into_iter().partition(|x| x.0 < cut);
    if before.is_empty() || after.is_empty() {
        return None;
    }
    Some((
        before.into_iter().map(|x| x.2).collect(),
        after.into_iter().map(|x| x.2).collect(),
    ))
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;

    fn word(s: &str, x: f32, baseline: f32, size: f32) -> Vec<Glyph> {
        let w = size * 0.5;
        s.chars()
            .enumerate()
            .map(|(i, c)| Glyph {
                text: c.to_string(),
                x0: x + i as f32 * w,
                x1: x + (i + 1) as f32 * w,
                baseline,
                size,
            })
            .collect()
    }

    #[test]
    fn words_and_spaces() {
        let mut g = word("Hello", 72.0, 100.0, 10.0);
        g.extend(word("world", 72.0 + 5.0 * 5.0 + 3.0, 100.0, 10.0));
        let b = layout(g);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].text(), "Hello world");
    }

    #[test]
    fn two_columns_read_column_by_column() {
        let mut g = Vec::new();
        for (i, y) in [100.0, 112.0, 124.0].iter().enumerate() {
            g.extend(word(&format!("left{i}"), 72.0, *y, 10.0));
            g.extend(word(&format!("right{i}"), 320.0, *y, 10.0));
        }
        let text: Vec<String> = layout(g).iter().map(LayoutBlock::text).collect();
        assert_eq!(text, ["left0\nleft1\nleft2", "right0\nright1\nright2"]);
    }

    #[test]
    fn paragraphs_split_on_big_gap() {
        let mut g = word("first", 72.0, 100.0, 10.0);
        g.extend(word("second", 72.0, 160.0, 10.0));
        assert_eq!(layout(g).len(), 2);
    }

    #[test]
    fn junk_coordinates_do_not_panic() {
        let g = vec![
            Glyph {
                text: "a".into(),
                x0: f32::NAN,
                x1: 1.0,
                baseline: 1.0,
                size: 1.0,
            },
            Glyph {
                text: "b".into(),
                x0: 0.0,
                x1: f32::INFINITY,
                baseline: -1e30,
                size: 0.0,
            },
        ];
        let _ = layout(g);
    }
}

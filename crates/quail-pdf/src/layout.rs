//! From positioned glyphs to lines, paragraph blocks and reading order.
//!
//! Page coordinates are PDF points from the top-left corner, y growing downwards (the
//! orientation of [`BBox`]). The algorithm is linear apart from sorting:
//!
//! 1. Glyphs are grouped by text direction (the nearest of the four axis directions of their
//!    advance vector), and each group is laid out in its own frame, where its text runs left
//!    to right. Rotated text (by `cm` or `Tm`) thus gets its own baselines.
//! 2. Glyphs are bucketed into lines by baseline (tolerance relative to the font size), and a
//!    line is split into segments at wide horizontal gaps (columns, table cells).
//! 3. Segments stack into paragraph blocks when they are close vertically and overlap
//!    horizontally. Only blocks still within reach of the current line are considered, so a
//!    page of 80 000 one-glyph lines stays linear.
//! 4. Blocks are ordered with a recursive XY-cut: a clear column gutter is cut first, then
//!    rows, then any remaining narrow vertical gap.

use quail_core::BBox;

/// A glyph's Unicode text: one char inline (the common case, no allocation), else a string
/// (ligatures, multi-char `ToUnicode` entries).
#[derive(Clone, Debug)]
pub(crate) enum GlyphText {
    /// One char, UTF-8 encoded, and its length in bytes.
    Char([u8; 4], u8),
    /// Anything longer.
    Str(String),
}

impl GlyphText {
    /// The text of one char.
    pub fn from_char(c: char) -> Self {
        let mut b = [0; 4];
        let n = c.encode_utf8(&mut b).len();
        Self::Char(b, u8::try_from(n).unwrap_or(4))
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        match self {
            // Always valid: written by `char::encode_utf8`.
            Self::Char(b, n) => {
                std::str::from_utf8(b.get(..usize::from(*n)).unwrap_or(&[])).unwrap_or_default()
            }
            Self::Str(s) => s,
        }
    }
}

impl From<String> for GlyphText {
    fn from(s: String) -> Self {
        let mut it = s.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => Self::from_char(c),
            _ => Self::Str(s),
        }
    }
}

impl From<&str> for GlyphText {
    fn from(s: &str) -> Self {
        Self::from(s.to_owned())
    }
}

/// One glyph with its Unicode text, origin and advance, in page coordinates.
#[derive(Clone, Debug)]
pub(crate) struct Glyph {
    /// Unicode text (usually one char, ligatures give several).
    pub text: GlyphText,
    /// Origin x.
    pub x: f32,
    /// Origin y (on the baseline).
    pub y: f32,
    /// Advance vector x.
    pub dx: f32,
    /// Advance vector y.
    pub dy: f32,
    /// Font size in points (height of the em square on the page).
    pub size: f32,
}

/// A glyph in its text frame: text runs along +x, lines go down along +y.
#[derive(Clone, Debug)]
struct FrameGlyph {
    text: GlyphText,
    x0: f32,
    x1: f32,
    baseline: f32,
    size: f32,
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
    /// Box in page coordinates.
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
/// A vertical gap at least this wide (points) between two groups of blocks that sit side by
/// side is a column gutter, cut before rows.
const GUTTER: f32 = 12.0;
/// Each side of a column cut spans at least this share of the combined width.
const MIN_COLUMN_SHARE: f32 = 0.3;
/// Each side of a column cut holds at least this many lines.
const MIN_COLUMN_LINES: usize = 3;

/// The four text directions, as unit vectors in page coordinates (y down).
const DIRECTIONS: [(f32, f32); 4] = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];

fn direction_of(g: &Glyph) -> usize {
    let len = g.dx.hypot(g.dy);
    if !(len.is_finite() && len > 1e-6) {
        return 0;
    }
    let (ux, uy) = (g.dx / len, g.dy / len);
    let mut best = 0;
    let mut best_dot = f32::NEG_INFINITY;
    for (i, (dx, dy)) in DIRECTIONS.iter().enumerate() {
        let dot = ux * dx + uy * dy;
        if dot > best_dot {
            best_dot = dot;
            best = i;
        }
    }
    best
}

/// Lays out a page's glyphs into blocks in reading order. The dominant direction comes first.
pub(crate) fn layout(glyphs: Vec<Glyph>) -> Vec<LayoutBlock> {
    let mut groups: [Vec<FrameGlyph>; 4] = Default::default();
    for g in glyphs {
        if !(g.x.is_finite() && g.y.is_finite() && g.dx.is_finite() && g.dy.is_finite()) {
            continue;
        }
        let dir = direction_of(&g);
        let (dx, dy) = DIRECTIONS[dir];
        // Frame axes: u along the text, v = u rotated +90° (down the page for upright text).
        let (nx, ny) = (-dy, dx);
        let u = g.x * dx + g.y * dy;
        let v = g.x * nx + g.y * ny;
        let adv = g.dx.hypot(g.dy);
        let size = if g.size.is_finite() && g.size > 0.1 {
            g.size
        } else {
            1.0
        };
        if let Some(group) = groups.get_mut(dir) {
            group.push(FrameGlyph {
                text: g.text,
                x0: u,
                x1: u + adv,
                baseline: v,
                size,
            });
        }
    }
    let mut order: Vec<usize> = (0..4).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(groups.get(i).map_or(0, Vec::len)));
    let mut out = Vec::new();
    for dir in order {
        let Some(group) = groups.get_mut(dir) else {
            continue;
        };
        if group.is_empty() {
            continue;
        }
        let (dx, dy) = DIRECTIONS[dir];
        let blocks = layout_frame(std::mem::take(group));
        out.extend(blocks.into_iter().map(|mut b| {
            b.bbox = to_page(b.bbox, dx, dy);
            for l in &mut b.lines {
                l.bbox = to_page(l.bbox, dx, dy);
            }
            b
        }));
    }
    out
}

/// Maps a frame box back to page coordinates (axis-aligned hull of the four corners).
fn to_page(b: BBox, dx: f32, dy: f32) -> BBox {
    let (nx, ny) = (-dy, dx);
    let corner = |u: f32, v: f32| (u * dx + v * nx, u * dy + v * ny);
    let pts = [
        corner(b.x0, b.y0),
        corner(b.x1, b.y0),
        corner(b.x0, b.y1),
        corner(b.x1, b.y1),
    ];
    let mut r = BBox {
        x0: f32::INFINITY,
        y0: f32::INFINITY,
        x1: f32::NEG_INFINITY,
        y1: f32::NEG_INFINITY,
    };
    for (x, y) in pts {
        r.x0 = r.x0.min(x);
        r.y0 = r.y0.min(y);
        r.x1 = r.x1.max(x);
        r.y1 = r.y1.max(y);
    }
    r
}

fn layout_frame(glyphs: Vec<FrameGlyph>) -> Vec<LayoutBlock> {
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

fn segments(mut glyphs: Vec<FrameGlyph>) -> Vec<Segment> {
    // Content streams usually emit text in a sensible order already; a stable sort by
    // baseline keeps that order inside a line when x ties.
    glyphs.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
    // Lines are runs of the sorted glyphs: a glyph joins the line when its baseline is close
    // to the line's first glyph. Each run is then sorted by x in place (stable).
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(first) = glyphs.get(start) {
        let (b0, s0) = (first.baseline, first.size);
        let rest = glyphs.get(start + 1..).unwrap_or(&[]);
        let len = rest
            .iter()
            .position(|g| (g.baseline - b0).abs() > SAME_LINE_EM * s0.max(g.size))
            .map_or(rest.len() + 1, |n| n + 1);
        if let Some(line) = glyphs.get_mut(start..start + len) {
            line.sort_by(|a, b| a.x0.total_cmp(&b.x0));
            line_segments(line, &mut out);
        }
        start += len;
    }
    out
}

/// Splits one line (sorted by x) into segments at wide gaps.
fn line_segments(line: &[FrameGlyph], out: &mut Vec<Segment>) {
    let mut cur: Option<(String, BBox, f32, f32)> = None; // text, bbox, size, last x1
    for g in line {
        let gt = g.text.as_str();
        let top = g.baseline - 0.8 * g.size;
        let bottom = g.baseline + 0.2 * g.size;
        let mut close = false;
        if let Some((text, bbox, size, last_x1)) = cur.as_mut() {
            let gap = g.x0 - *last_x1;
            let em = size.max(g.size);
            if gap > SEGMENT_GAP_EM * em {
                close = true;
            } else {
                // Overlapping duplicates (fake bold: the same glyph drawn twice with a tiny
                // offset) are dropped.
                if gap < -0.3 * em && text.ends_with(gt) {
                    continue;
                }
                if gap > SPACE_GAP_EM * em
                    && !text.ends_with(char::is_whitespace)
                    && !gt.starts_with(char::is_whitespace)
                {
                    text.push(' ');
                }
                text.push_str(gt);
                bbox.x1 = bbox.x1.max(g.x1);
                bbox.y0 = bbox.y0.min(top);
                bbox.y1 = bbox.y1.max(bottom);
                *last_x1 = g.x1.max(*last_x1);
                continue;
            }
        }
        if close && let Some((t, b, s, _)) = cur.take() {
            push_segment(out, &t, b, s);
        }
        let bbox = BBox {
            x0: g.x0,
            y0: top,
            x1: g.x1,
            y1: bottom,
        };
        let mut text = String::with_capacity(32);
        text.push_str(gt);
        cur = Some((text, bbox, g.size, g.x1));
    }
    if let Some((t, b, s, _)) = cur {
        push_segment(out, &t, b, s);
    }
}

fn push_segment(out: &mut Vec<Segment>, text: &str, bbox: BBox, size: f32) {
    let text = text.trim().to_owned();
    if !text.is_empty() {
        out.push(Segment { text, bbox, size });
    }
}

fn blocks(segments: Vec<Segment>) -> Vec<LayoutBlock> {
    let mut blocks: Vec<LayoutBlock> = Vec::new();
    // Blocks that may still continue. Segments arrive in line order (top to bottom), so a
    // block whose last line ends more than a paragraph gap above the current line can never
    // grow again and leaves this list: the search stays short (about the column count).
    let mut open: Vec<usize> = Vec::new();
    for s in segments {
        open.retain(|&i| {
            blocks
                .get(i)
                .and_then(|b| b.lines.last())
                .is_some_and(|last| {
                    let em = last.size.max(s.size);
                    s.bbox.y0 - last.bbox.y1 < PARA_GAP_EM * em
                })
        });
        let target = open
            .iter()
            .rposition(|&i| blocks.get(i).is_some_and(|b| continues(b, &s)))
            .and_then(|p| open.get(p).copied());
        if let Some(b) = target.and_then(|i| blocks.get_mut(i)) {
            b.bbox = union(b.bbox, s.bbox);
            b.lines.push(s);
        } else {
            open.push(blocks.len());
            blocks.push(LayoutBlock {
                bbox: s.bbox,
                lines: vec![s],
            });
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
    // A clear column gutter first: two groups side by side, each with several blocks, that
    // overlap vertically. Otherwise paragraph gaps at the same height in both columns would
    // cut rows across them and interleave the columns.
    if let Some((a, b)) = split(&idx, |i| bbox_of(i).map(|r| (r.x0, r.x1)), GUTTER)
        && is_column_split(blocks, &a, &b)
    {
        xy_cut(blocks, a, depth + 1, out);
        xy_cut(blocks, b, depth + 1, out);
        return;
    }
    // Rows: the reading order of a page is top to bottom.
    if let Some((a, b)) = split(&idx, |i| bbox_of(i).map(|r| (r.y0, r.y1)), 0.0) {
        xy_cut(blocks, a, depth + 1, out);
        xy_cut(blocks, b, depth + 1, out);
        return;
    }
    // Any remaining vertical gap: require a real gap, not a word gap.
    if let Some((a, b)) = split(&idx, |i| bbox_of(i).map(|r| (r.x0, r.x1)), 4.0) {
        xy_cut(blocks, a, depth + 1, out);
        xy_cut(blocks, b, depth + 1, out);
        return;
    }
    xy_cut(blocks, idx, MAX_CUT_DEPTH, out);
}

/// The two sides look like text columns, not table columns or label/value pairs: each is
/// at least [`MIN_COLUMN_SHARE`] of the combined width and holds at least
/// [`MIN_COLUMN_LINES`] lines, and their vertical extents overlap for at least half of the
/// shorter one.
fn is_column_split(blocks: &[LayoutBlock], a: &[usize], b: &[usize]) -> bool {
    let stats = |ids: &[usize]| {
        ids.iter().filter_map(|&i| blocks.get(i)).fold(
            (
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                0usize,
            ),
            |(x0, x1, y0, y1, n), blk| {
                (
                    x0.min(blk.bbox.x0),
                    x1.max(blk.bbox.x1),
                    y0.min(blk.bbox.y0),
                    y1.max(blk.bbox.y1),
                    n + blk.lines.len(),
                )
            },
        )
    };
    let (ax0, ax1, at, ab, an) = stats(a);
    let (bx0, bx1, bt, bb, bn) = stats(b);
    if an < MIN_COLUMN_LINES || bn < MIN_COLUMN_LINES {
        return false;
    }
    let total = ax1.max(bx1) - ax0.min(bx0);
    let wide = |w: f32| total > 0.0 && w >= MIN_COLUMN_SHARE * total;
    if !(wide(ax1 - ax0) && wide(bx1 - bx0)) {
        return false;
    }
    let overlap = ab.min(bb) - at.max(bt);
    let shorter = (ab - at).min(bb - bt);
    shorter > 0.0 && overlap >= 0.5 * shorter && !rows_aligned(blocks, a, b)
}

/// Most lines of the side with fewer lines have a line on the other side at the same
/// baseline: the two sides are cells of the same rows (a table, a label/value form), which
/// read row by row. Independent text columns drift apart as their paragraphs differ.
fn rows_aligned(blocks: &[LayoutBlock], a: &[usize], b: &[usize]) -> bool {
    let baselines = |ids: &[usize]| {
        let mut v: Vec<(f32, f32)> = ids
            .iter()
            .filter_map(|&i| blocks.get(i))
            .flat_map(|blk| blk.lines.iter().map(|l| (l.bbox.y1, l.size)))
            .collect();
        v.sort_by(|x, y| x.0.total_cmp(&y.0));
        v
    };
    let (la, lb) = (baselines(a), baselines(b));
    let (few, many) = if la.len() <= lb.len() {
        (la, lb)
    } else {
        (lb, la)
    };
    if few.is_empty() {
        return false;
    }
    let matched = few
        .iter()
        .filter(|(y, size)| {
            let tol = 0.25 * size.max(1.0);
            let k = many.partition_point(|(m, _)| *m < y - tol);
            many.get(k).is_some_and(|(m, _)| (m - y).abs() <= tol)
        })
        .count();
    matched * 5 >= few.len() * 4
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

    fn word_dir(s: &str, x: f32, y: f32, size: f32, (dx, dy): (f32, f32)) -> Vec<Glyph> {
        let w = size * 0.5;
        s.chars()
            .enumerate()
            .map(|(i, c)| Glyph {
                text: GlyphText::from_char(c),
                x: x + i as f32 * w * dx,
                y: y + i as f32 * w * dy,
                dx: w * dx,
                dy: w * dy,
                size,
            })
            .collect()
    }

    fn word(s: &str, x: f32, baseline: f32, size: f32) -> Vec<Glyph> {
        word_dir(s, x, baseline, size, (1.0, 0.0))
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

    /// A line of text about 220 pt wide.
    fn wide(tag: &str) -> String {
        format!("{tag} lorem ipsum dolor sit amet consectetur")
    }

    #[test]
    fn text_columns_with_aligned_paragraph_gaps_are_not_interleaved() {
        // Two wide text columns, each with two paragraphs, the paragraph gaps at about the
        // same height but the baselines offset (different leading), as in real columns:
        // rows-first would read L-para1, R-para1, L-para2, R-para2.
        let mut g = Vec::new();
        for (i, y) in [100.0, 112.0, 124.0, 170.0, 182.0, 194.0]
            .iter()
            .enumerate()
        {
            g.extend(word(&wide(&format!("left{i}")), 40.0, *y, 10.0));
            g.extend(word(&wide(&format!("right{i}")), 320.0, *y + 5.0, 10.0));
        }
        let text: Vec<String> = layout(g).iter().map(LayoutBlock::text).collect();
        assert_eq!(text.len(), 4, "{text:?}");
        assert!(
            text[0].starts_with("left0") && text[1].starts_with("left3"),
            "{text:?}"
        );
        assert!(
            text[2].starts_with("right0") && text[3].starts_with("right3"),
            "{text:?}"
        );
    }

    #[test]
    fn table_rows_read_row_by_row() {
        // Two wide cells per row on shared baselines: a table, read row by row.
        let mut g = Vec::new();
        for (i, y) in [100.0, 112.0, 124.0, 170.0, 182.0, 194.0]
            .iter()
            .enumerate()
        {
            g.extend(word(&wide(&format!("label{i}")), 40.0, *y, 10.0));
            g.extend(word(&wide(&format!("value{i}")), 320.0, *y, 10.0));
        }
        let text: Vec<String> = layout(g).iter().map(LayoutBlock::text).collect();
        assert!(
            text[0].starts_with("label0") && text[1].starts_with("value0"),
            "{text:?}"
        );
    }

    #[test]
    fn title_spanning_columns_comes_first() {
        let mut g = word(
            "A title that spans both columns of the page",
            72.0,
            60.0,
            14.0,
        );
        for (i, y) in [100.0, 112.0, 124.0].iter().enumerate() {
            g.extend(word(&format!("left{i}"), 72.0, *y, 10.0));
            g.extend(word(&format!("right{i}"), 320.0, *y, 10.0));
        }
        let text: Vec<String> = layout(g).iter().map(LayoutBlock::text).collect();
        assert!(text[0].starts_with("A title"), "{text:?}");
        assert_eq!(
            &text[1..],
            ["left0\nleft1\nleft2", "right0\nright1\nright2"]
        );
    }

    #[test]
    fn rotated_text_keeps_its_lines() {
        // Text running down the page (rotated 90°), two lines side by side.
        let mut g = word_dir("vertical", 100.0, 100.0, 10.0, (0.0, 1.0));
        g.extend(word_dir("second", 88.0, 100.0, 10.0, (0.0, 1.0)));
        let blocks = layout(g);
        let text: Vec<String> = blocks.iter().map(LayoutBlock::text).collect();
        assert_eq!(text, ["vertical\nsecond"]);
        // The box is tall and narrow, at the right place.
        let b = blocks[0].bbox;
        assert!(b.y1 - b.y0 > b.x1 - b.x0, "{b:?}");
        assert!(b.y0 >= 99.0 && b.x1 <= 110.0, "{b:?}");
    }

    #[test]
    fn paragraphs_split_on_big_gap() {
        let mut g = word("first", 72.0, 100.0, 10.0);
        g.extend(word("second", 72.0, 160.0, 10.0));
        assert_eq!(layout(g).len(), 2);
    }

    #[test]
    fn many_lines_are_linear() {
        // 80 000 one-glyph lines, each its own paragraph: must not be quadratic.
        let g: Vec<Glyph> = (0..80_000)
            .flat_map(|i| word("a", 10.0, 10.0 + i as f32 * 2.0, 0.5))
            .collect();
        let t = std::time::Instant::now();
        let blocks = layout(g);
        assert!(!blocks.is_empty());
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn max_glyphs_per_page_lay_out_quickly() {
        // The device's cap: 400 000 glyphs, here 4 000 lines of 100 glyphs.
        let line: String = "abcdefghij".repeat(10);
        let g: Vec<Glyph> = (0..4_000)
            .flat_map(|i| word(&line, 10.0, 10.0 + i as f32 * 1.2, 1.0))
            .collect();
        assert_eq!(g.len(), 400_000);
        let t = std::time::Instant::now();
        let blocks = layout(g);
        assert!(!blocks.is_empty());
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn junk_coordinates_do_not_panic() {
        let g = vec![
            Glyph {
                text: "a".into(),
                x: f32::NAN,
                y: 1.0,
                dx: 1.0,
                dy: 0.0,
                size: 1.0,
            },
            Glyph {
                text: "b".into(),
                x: 0.0,
                y: -1e30,
                dx: f32::INFINITY,
                dy: 0.0,
                size: 0.0,
            },
            Glyph {
                text: "c".into(),
                x: 0.0,
                y: 0.0,
                dx: 0.0,
                dy: 0.0,
                size: -3.0,
            },
        ];
        let _ = layout(g);
    }
}

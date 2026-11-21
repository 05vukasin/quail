//! The [`OcrText`] artifact: what OCR read on each page, laid out like a core [`Page`] (text in
//! reading order, blocks pointing into it, boxes in points) so text analyzers can use it.

use quail_core::{Artifact, BBox, Block, BlockKind, Document, Page, Span, TextSource, Zone};
use serde::{Deserialize, Serialize};

use crate::engine::{OcrPage, PxBox};
use crate::script::{Script, ScriptCounts};

/// Where the pixels of a page came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelSource {
    /// The page's single embedded scan, decoded.
    EmbeddedScan,
    /// The page rendered by the format's [`PageRaster`](quail_core::PageRaster).
    Render,
}

impl PixelSource {
    /// Lowercase name, as written into facts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmbeddedScan => "embedded_scan",
            Self::Render => "render",
        }
    }
}

/// A word of an OCR'd line, in page coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrTextWord {
    /// Byte range in [`OcrPageText::text`].
    pub span: Span,
    /// Box in points, top-left origin.
    pub bbox: BBox,
    /// Recognition confidence.
    pub conf: f32,
}

/// One OCR'd line, in page coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrTextLine {
    /// Byte range in [`OcrPageText::text`].
    pub span: Span,
    /// Box in points, top-left origin.
    pub bbox: BBox,
    /// Recognition confidence.
    pub conf: f32,
    /// Script of its letters.
    pub script: Script,
    /// Words, when the engine reports them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<OcrTextWord>,
}

/// What OCR read on one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrPageText {
    /// Zero-based page index.
    pub index: u32,
    /// Text in reading order: lines of one row joined by a space, rows by `\n`.
    pub text: String,
    /// One [`BlockKind::Paragraph`] block per row, pointing into `text`, boxes in points.
    pub blocks: Vec<Block>,
    /// Every kept line, in reading order.
    pub lines: Vec<OcrTextLine>,
    /// Lines dropped for a confidence below the threshold.
    pub dropped_lines: u32,
    /// Mean confidence of the kept lines, weighted by characters (`None`: nothing read).
    pub mean_conf: Option<f32>,
    /// Characters read (kept lines, spaces excluded).
    pub chars: u32,
    /// Dominant script.
    pub script: Script,
    /// Clockwise rotation of the page content found by the engine.
    pub rotation: u16,
    /// Resolution of the recognized image, pixels per inch.
    pub dpi: f32,
    /// Where the pixels came from.
    pub source: PixelSource,
}

/// OCR results of a file: one entry per OCR'd page, in page order. Pages that were not
/// OCR'd (they have a text layer) are absent. Provided by `ocr.pages@1` as `"ocr_text"`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OcrText {
    /// Short id of the engine (`name@fingerprint`).
    pub engine: String,
    /// OCR'd pages.
    pub pages: Vec<OcrPageText>,
}

impl Artifact for OcrText {
    const NAME: &'static str = "ocr_text";
}

impl OcrText {
    /// The OCR result of page `index`.
    pub fn page(&self, index: u32) -> Option<&OcrPageText> {
        self.pages.iter().find(|p| p.index == index)
    }

    /// A copy of `doc` in which every OCR'd page whose own text is not native gets the OCR
    /// text and blocks (`text_source = Ocr`, `needs_ocr = false`). Native text is never
    /// replaced. This is what a text analyzer should read once OCR has run; see the crate
    /// documentation for how it is meant to reach them.
    pub fn apply_to(&self, doc: &Document) -> Document {
        let mut out = doc.clone();
        for p in &mut out.pages {
            if p.text_source == TextSource::Native && !p.needs_ocr {
                continue;
            }
            if let Some(o) = self.page(p.index) {
                apply_page(p, o);
            }
        }
        out
    }
}

fn apply_page(p: &mut Page, o: &OcrPageText) {
    p.text.clone_from(&o.text);
    p.blocks.clone_from(&o.blocks);
    p.text_source = TextSource::Ocr;
    p.needs_ocr = false;
}

/// How page pixels map to points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxToPt {
    /// Points per pixel horizontally.
    pub sx: f32,
    /// Points per pixel vertically.
    pub sy: f32,
}

impl PxToPt {
    /// For an image of `px` pixels showing a page of `size_pt` points; falls back to `dpi`
    /// when the page size is unknown or unusable.
    pub fn new(px: (u32, u32), size_pt: Option<(f32, f32)>, dpi: f32) -> Self {
        let by_dpi = 72.0
            / if dpi.is_finite() && dpi > 0.0 {
                dpi
            } else {
                72.0
            };
        #[allow(clippy::cast_precision_loss)]
        match size_pt {
            Some((w, h))
                if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && px.0 > 0 && px.1 > 0 =>
            {
                Self {
                    sx: w / px.0 as f32,
                    sy: h / px.1 as f32,
                }
            }
            _ => Self {
                sx: by_dpi,
                sy: by_dpi,
            },
        }
    }

    /// A pixel box in points.
    pub fn bbox(&self, b: &PxBox) -> BBox {
        BBox {
            x0: b.x0 * self.sx,
            y0: b.y0 * self.sy,
            x1: b.x1 * self.sx,
            y1: b.y1 * self.sy,
        }
    }
}

/// Zone by position alone: the top and bottom 7% of the page are header and footer bands.
fn zone_of(b: &BBox, page_h: Option<f32>) -> Zone {
    match page_h {
        Some(h) if h > 0.0 => {
            if b.y1 <= h * 0.07 {
                Zone::Header
            } else if b.y0 >= h * 0.93 {
                Zone::Footer
            } else {
                Zone::Body
            }
        }
        _ => Zone::Unknown,
    }
}

/// Lays out an engine result as page text. Lines with `conf < min_conf` or no text are
/// dropped; the rest is grouped into rows (vertical overlap of at least half the smaller
/// line height), rows top to bottom, lines left to right. Stops adding lines once the text
/// would exceed `max_bytes`.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]
pub fn layout(
    index: u32,
    ocr: &OcrPage,
    map: PxToPt,
    page_h: Option<f32>,
    min_conf: f32,
    max_bytes: usize,
    dpi: f32,
    source: PixelSource,
) -> OcrPageText {
    let mut kept: Vec<usize> = Vec::new();
    let mut dropped = 0u32;
    for (i, l) in ocr.lines.iter().enumerate() {
        if l.text.trim().is_empty() || l.conf.is_nan() || l.conf < min_conf {
            dropped += u32::from(!l.text.trim().is_empty());
            continue;
        }
        kept.push(i);
    }
    kept.sort_by(|&a, &b| {
        let (la, lb) = (&ocr.lines[a].bbox, &ocr.lines[b].bbox);
        la.cy().total_cmp(&lb.cy()).then(la.x0.total_cmp(&lb.x0))
    });
    // Rows: consecutive lines (by centre) that overlap vertically.
    let mut rows: Vec<(PxBox, Vec<usize>)> = Vec::new();
    for i in kept {
        let b = ocr.lines[i].bbox;
        if let Some((rb, members)) = rows.last_mut() {
            let overlap = rb.y1.min(b.y1) - rb.y0.max(b.y0);
            if overlap >= 0.5 * rb.height().min(b.height()) && overlap > 0.0 {
                rb.x0 = rb.x0.min(b.x0);
                rb.x1 = rb.x1.max(b.x1);
                members.push(i);
                continue;
            }
        }
        rows.push((b, vec![i]));
    }
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut lines = Vec::new();
    let mut counts = ScriptCounts::default();
    let (mut n, mut sum) = (0u32, 0f64);
    'rows: for (_, mut members) in rows {
        members.sort_by(|&a, &b| ocr.lines[a].bbox.x0.total_cmp(&ocr.lines[b].bbox.x0));
        let row_start = text.len();
        // `row_start` points at the `\n` for every row but the first.
        let first = if row_start > 0 {
            row_start + 1
        } else {
            row_start
        };
        let mut row_box: Option<PxBox> = None;
        for (k, &i) in members.iter().enumerate() {
            let l = &ocr.lines[i];
            let t = l.text.trim();
            let sep = if k > 0 {
                " "
            } else if row_start > 0 {
                "\n"
            } else {
                ""
            };
            if text.len() + sep.len() + t.len() > max_bytes {
                if k > 0 {
                    push_block(&mut blocks, &text, first, row_box, map, page_h);
                }
                break 'rows;
            }
            text.push_str(sep);
            let start = text.len();
            text.push_str(t);
            let span = span(start, text.len());
            let words = l
                .words
                .iter()
                .filter_map(|w| {
                    let wt = w.text.trim();
                    let off = t.find(wt).filter(|_| !wt.is_empty())?;
                    Some(OcrTextWord {
                        span: span_at(start + off, wt.len()),
                        bbox: map.bbox(&w.bbox),
                        conf: w.conf,
                    })
                })
                .collect();
            lines.push(OcrTextLine {
                span,
                bbox: map.bbox(&l.bbox),
                conf: l.conf,
                script: l.script,
                words,
            });
            counts.add(t);
            let c =
                u32::try_from(t.chars().filter(|c| !c.is_whitespace()).count()).unwrap_or(u32::MAX);
            n = n.saturating_add(c);
            sum += f64::from(l.conf) * f64::from(c);
            row_box = Some(match row_box {
                None => l.bbox,
                Some(r) => PxBox::around([
                    (r.x0, r.y0),
                    (r.x1, r.y1),
                    (l.bbox.x0, l.bbox.y0),
                    (l.bbox.x1, l.bbox.y1),
                ]),
            });
        }
        push_block(&mut blocks, &text, first, row_box, map, page_h);
    }
    #[allow(clippy::cast_possible_truncation)]
    let mean_conf = (n > 0).then(|| (sum / f64::from(n)) as f32);
    OcrPageText {
        index,
        text,
        blocks,
        lines,
        dropped_lines: dropped,
        mean_conf,
        chars: n,
        script: counts.script(),
        rotation: ocr.rotation,
        dpi,
        source,
    }
}

fn push_block(
    blocks: &mut Vec<Block>,
    text: &str,
    start: usize,
    row: Option<PxBox>,
    map: PxToPt,
    page_h: Option<f32>,
) {
    let Some(row) = row else { return };
    if start >= text.len() {
        return;
    }
    let bbox = map.bbox(&row);
    blocks.push(Block {
        span: span(start, text.len()),
        kind: BlockKind::Paragraph,
        zone: zone_of(&bbox, page_h),
        bbox: Some(bbox),
    });
}

fn span(start: usize, end: usize) -> Span {
    Span {
        start: u32::try_from(start).unwrap_or(u32::MAX),
        end: u32::try_from(end).unwrap_or(u32::MAX),
    }
}

fn span_at(start: usize, len: usize) -> Span {
    span(start, start + len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{OcrLine, OcrWord};
    use crate::script::script_of;
    use quail_core::PageKind;

    fn line(t: &str, x0: f32, y0: f32, x1: f32, y1: f32, conf: f32) -> OcrLine {
        let bbox = PxBox { x0, y0, x1, y1 };
        OcrLine {
            text: t.into(),
            quad: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
            bbox,
            conf,
            script: script_of(t),
            words: vec![],
        }
    }

    fn page(lines: Vec<OcrLine>) -> OcrPage {
        OcrPage {
            width: 1000,
            height: 2000,
            rotation: 0,
            lines,
        }
    }

    #[test]
    fn rows_and_reading_order() {
        let p = page(vec![
            line("right", 600.0, 100.0, 900.0, 140.0, 0.9),
            line("second row", 100.0, 300.0, 500.0, 340.0, 0.8),
            line("left", 100.0, 105.0, 400.0, 142.0, 1.0),
            line("noise", 100.0, 500.0, 200.0, 540.0, 0.1),
            line("   ", 100.0, 600.0, 200.0, 640.0, 0.9),
        ]);
        let map = PxToPt::new((1000, 2000), Some((500.0, 1000.0)), 200.0);
        let t = layout(
            3,
            &p,
            map,
            Some(1000.0),
            0.5,
            1 << 20,
            200.0,
            PixelSource::Render,
        );
        assert_eq!(t.text, "left right\nsecond row");
        assert_eq!(t.blocks.len(), 2);
        assert_eq!(t.blocks[0].span.slice(&t.text), "left right");
        assert_eq!(t.blocks[1].span.slice(&t.text), "second row");
        assert_eq!(t.lines.len(), 3);
        assert_eq!(t.lines[1].span.slice(&t.text), "right");
        assert_eq!(t.dropped_lines, 1);
        assert_eq!(t.chars, 18);
        assert_eq!(t.script, Script::Latin);
        // Pixels to points: half scale.
        let b = t.blocks[0].bbox.unwrap_or(BBox {
            x0: 0.0,
            y0: 0.0,
            x1: 0.0,
            y1: 0.0,
        });
        assert!((b.x0 - 50.0).abs() < 1e-3 && (b.x1 - 450.0).abs() < 1e-3);
        assert!((b.y0 - 50.0).abs() < 1e-3 && (b.y1 - 71.0).abs() < 1e-3);
        assert_eq!(t.blocks[0].zone, Zone::Body);
        let m = t.mean_conf.unwrap_or(0.0);
        // (4*1.0 + 5*0.9 + 9*0.8) / 18
        assert!((m - (4.0 + 4.5 + 7.2) / 18.0).abs() < 1e-5, "{m}");
    }

    #[test]
    fn text_cap_is_respected() {
        let p = page(vec![
            line("aaaa", 0.0, 0.0, 10.0, 10.0, 1.0),
            line("bbbb", 0.0, 100.0, 10.0, 110.0, 1.0),
        ]);
        let map = PxToPt::new((1000, 2000), None, 144.0);
        let t = layout(0, &p, map, None, 0.0, 6, 144.0, PixelSource::Render);
        assert_eq!(t.text, "aaaa");
        assert_eq!(t.blocks.len(), 1);
        assert!((map.sx - 0.5).abs() < 1e-6);
        assert_eq!(t.blocks[0].zone, Zone::Unknown);
    }

    #[test]
    fn words_get_spans() {
        let mut l = line("Ugovor br. 5", 0.0, 900.0, 300.0, 940.0, 0.95);
        l.words = vec![OcrWord {
            text: "br.".into(),
            bbox: PxBox {
                x0: 120.0,
                y0: 900.0,
                x1: 160.0,
                y1: 940.0,
            },
            conf: 0.9,
        }];
        let p = page(vec![l]);
        let map = PxToPt::new((1000, 2000), Some((1000.0, 2000.0)), 72.0);
        let t = layout(
            0,
            &p,
            map,
            Some(2000.0),
            0.0,
            1 << 20,
            72.0,
            PixelSource::EmbeddedScan,
        );
        assert_eq!(t.lines[0].words[0].span.slice(&t.text), "br.");
    }

    #[test]
    fn apply_keeps_native_text() {
        let mut native = Page::new(0, PageKind::Physical);
        native.text = "native".into();
        native.text_source = TextSource::Native;
        let mut scan = Page::new(1, PageKind::Physical);
        scan.needs_ocr = true;
        let doc = Document {
            pages: vec![native, scan],
        };
        let p = page(vec![line("ocr text", 0.0, 900.0, 300.0, 940.0, 0.95)]);
        let map = PxToPt::new((1000, 2000), None, 200.0);
        let mut ocr = OcrText::default();
        for i in 0..2 {
            ocr.pages.push(layout(
                i,
                &p,
                map,
                None,
                0.0,
                1 << 20,
                200.0,
                PixelSource::Render,
            ));
        }
        let out = ocr.apply_to(&doc);
        assert_eq!(out.pages[0].text, "native");
        assert_eq!(out.pages[1].text, "ocr text");
        assert_eq!(out.pages[1].text_source, TextSource::Ocr);
        assert!(!out.pages[1].needs_ocr);
        assert_eq!(out.pages[1].blocks.len(), 1);
    }

    #[test]
    fn zones_by_position() {
        let h = Some(1000.0);
        let b = |y0, y1| BBox {
            x0: 0.0,
            y0,
            x1: 1.0,
            y1,
        };
        assert_eq!(zone_of(&b(10.0, 60.0), h), Zone::Header);
        assert_eq!(zone_of(&b(940.0, 990.0), h), Zone::Footer);
        assert_eq!(zone_of(&b(400.0, 420.0), h), Zone::Body);
    }
}

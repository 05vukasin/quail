//! A hayro [`Device`] that records text glyphs (Unicode, position, direction), image
//! placements and how much vector drawing a page does. Nothing is rasterized and no image is
//! decoded.
//!
//! The device is also the page's watchdog. hayro cannot be preempted, but every operator that
//! draws or clips (forms push a clip on every `Do`) calls back here, so the device counts the
//! calls and checks the deadline. When the page runs out of time or of draw calls, it unwinds
//! out of the interpreter with [`Abort`] through `resume_unwind`, which skips the panic hook,
//! and the caller's per-page `catch_unwind` turns it into a problem.

use std::panic::resume_unwind;

use hayro_interpret::font::{Glyph as HGlyph, GlyphRun};
use hayro_interpret::hayro_cmap::BfString;
use hayro_interpret::kurbo::{Affine, BezPath, Point, Rect};
use hayro_interpret::{
    BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask,
};
use quail_core::{BBox, Emitter, PageImage};

use crate::layout::{Glyph, GlyphText};

/// Glyphs kept per page; a hostile page drawing millions of glyphs is truncated.
pub(crate) const MAX_GLYPHS_PER_PAGE: usize = 400_000;
/// Device callbacks (draws, clips, groups) allowed per page. A form drawing itself twice per
/// level reaches 2^40 calls in 8 KB; real pages stay far below this.
pub(crate) const MAX_CALLS_PER_PAGE: u64 = 4_000_000;
/// Clips and transparency groups allowed per page. Every form `Do` pushes a clip, so this cuts
/// form fan-out early (hayro re-parses a form's content on each use, ~6 µs per call), while
/// plain drawing keeps the larger [`MAX_CALLS_PER_PAGE`] budget.
pub(crate) const MAX_NESTINGS_PER_PAGE: u64 = 250_000;
/// The deadline is checked every this many callbacks.
const DEADLINE_EVERY: u64 = 1024;
/// Image placements remembered per page (the count keeps going).
const MAX_IMAGES_KEPT: usize = 64;

/// Why a page interpretation was cut short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Abort {
    /// The file's time budget ran out.
    Deadline,
    /// More than [`MAX_CALLS_PER_PAGE`] draw calls.
    TooManyCalls,
}

/// Counts callbacks and enforces the deadline and the call cap.
pub(crate) struct Watchdog<'e> {
    calls: u64,
    nestings: u64,
    ops: u64,
    clock: Clock<'e>,
}

/// Where the deadline comes from.
pub(crate) enum Clock<'e> {
    /// The extractor's emitter (the file's budget).
    Emitter(&'e Emitter),
    /// A fixed instant (rendering).
    Until(std::time::Instant),
}

impl<'e> Watchdog<'e> {
    pub fn new(clock: Clock<'e>) -> Self {
        Self {
            calls: 0,
            nestings: 0,
            ops: 0,
            clock,
        }
    }

    fn check_deadline(&self) {
        let late = match &self.clock {
            Clock::Emitter(e) => e.check_deadline().is_err(),
            Clock::Until(t) => std::time::Instant::now() >= *t,
        };
        if late {
            resume_unwind(Box::new(Abort::Deadline));
        }
    }

    #[inline]
    pub(crate) fn tick(&mut self) {
        self.tick_n(1);
    }

    /// `n` draw calls at once (a glyph run counts each glyph).
    #[inline]
    pub(crate) fn tick_n(&mut self, n: u64) {
        let before = self.calls;
        self.calls = self.calls.saturating_add(n.max(1));
        if self.calls > MAX_CALLS_PER_PAGE {
            resume_unwind(Box::new(Abort::TooManyCalls));
        }
        if before / DEADLINE_EVERY != self.calls / DEADLINE_EVERY {
            self.check_deadline();
        }
    }

    /// A content-stream operator: some (`sh`, inline images) cost far more than a draw
    /// callback, so the deadline is also checked between operators.
    #[inline]
    pub(crate) fn op(&mut self) {
        self.ops = self.ops.saturating_add(1);
        if self.ops.is_multiple_of(DEADLINE_EVERY) {
            self.check_deadline();
        }
    }

    /// A clip or a transparency group: also counted against [`MAX_NESTINGS_PER_PAGE`].
    #[inline]
    pub(crate) fn nest(&mut self) {
        self.nestings += 1;
        if self.nestings > MAX_NESTINGS_PER_PAGE {
            resume_unwind(Box::new(Abort::TooManyCalls));
        }
        self.tick();
    }
}

/// What the device saw on one page, detached from the device.
pub(crate) struct Captured {
    pub glyphs: Vec<Glyph>,
    pub unmapped: usize,
    pub truncated: usize,
    pub images: usize,
    pub placements: Vec<PageImage>,
    pub paths: usize,
    pub image_coverage: f32,
}

/// What the device saw on one page.
pub(crate) struct PageCapture<'e> {
    pub glyphs: Vec<Glyph>,
    /// Glyphs drawn without any Unicode mapping (counted, not kept).
    pub unmapped: usize,
    /// Glyphs dropped by [`MAX_GLYPHS_PER_PAGE`].
    pub truncated: usize,
    /// Number of image draws (including stencil masks).
    pub images: usize,
    /// The first [`MAX_IMAGES_KEPT`] placements.
    pub placements: Vec<PageImage>,
    /// Number of filled or stroked paths.
    pub paths: usize,
    page: Rect,
    watchdog: Watchdog<'e>,
}

impl<'e> PageCapture<'e> {
    pub fn new(width: f64, height: f64, watchdog: Watchdog<'e>) -> Self {
        Self {
            glyphs: Vec::new(),
            unmapped: 0,
            truncated: 0,
            images: 0,
            placements: Vec::new(),
            paths: 0,
            page: Rect::new(0.0, 0.0, width.max(0.0), height.max(0.0)),
            watchdog,
        }
    }

    /// What was seen, without the watchdog (and its borrow of the emitter).
    pub fn finish(self) -> Captured {
        let image_coverage = self.image_coverage();
        Captured {
            glyphs: self.glyphs,
            unmapped: self.unmapped,
            truncated: self.truncated,
            images: self.images,
            placements: self.placements,
            paths: self.paths,
            image_coverage,
        }
    }

    /// Share of the page area covered by the bounding boxes of its images, summed and capped
    /// at 1. Overlaps count twice, which is fine for "is this page mostly a picture".
    fn image_coverage(&self) -> f32 {
        let area = self.page.area();
        if area <= 0.0 {
            return 0.0;
        }
        let covered: f64 = self
            .placements
            .iter()
            .map(|p| f64::from((p.bbox.x1 - p.bbox.x0) * (p.bbox.y1 - p.bbox.y0)).max(0.0))
            .sum();
        #[allow(clippy::cast_possible_truncation)]
        let r = (covered / area).min(1.0) as f32;
        r
    }
}

fn glyph_advance(g: &HGlyph<'_>) -> f64 {
    let adv = match g {
        HGlyph::Outline(o) => o.advance_width(),
        HGlyph::Type3(t) => t.advance_width(),
    };
    match adv {
        Some(a) if a.is_finite() && a > 0.0 => f64::from(a),
        _ => 500.0,
    }
}

#[allow(clippy::cast_possible_truncation)]
fn f(x: f64) -> f32 {
    x as f32
}

impl<'a> Device<'a> for PageCapture<'_> {
    fn on_operator(&mut self) {
        self.watchdog.op();
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {
        self.watchdog.tick();
        self.paths = self.paths.saturating_add(1);
    }

    fn push_clip_path(&mut self, _: &ClipPath) {
        self.watchdog.nest();
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {
        self.watchdog.nest();
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        self.watchdog.tick();
        // Invisible text (render mode 3) is included on purpose: it is the text layer of
        // OCR'd scans.
        for g in run.glyphs() {
            let text = match g.as_unicode() {
                Some(BfString::Char(c)) => GlyphText::from_char(c),
                Some(BfString::String(s)) => GlyphText::from(s),
                None => {
                    self.unmapped = self.unmapped.saturating_add(1);
                    continue;
                }
            };
            if self.glyphs.len() >= MAX_GLYPHS_PER_PAGE {
                self.truncated = self.truncated.saturating_add(1);
                continue;
            }
            // Glyph space is 1000 units per em; `t` maps it onto the page (top-left origin).
            // The advance vector gives the text direction, so rotated text (via `cm` or `Tm`)
            // keeps its own baseline.
            let t: Affine = props.transform * g.transform();
            let origin = t * Point::new(0.0, 0.0);
            let end = t * Point::new(glyph_advance(g), 0.0);
            let top = t * Point::new(0.0, 1000.0);
            let adv = end - origin;
            self.glyphs.push(Glyph {
                text,
                x: f(origin.x),
                y: f(origin.y),
                dx: f(adv.x),
                dy: f(adv.y),
                size: f((top - origin).hypot()),
            });
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        self.watchdog.tick();
        self.images = self.images.saturating_add(1);
        if self.placements.len() >= MAX_IMAGES_KEPT {
            return;
        }
        // hayro hands over a transform from image pixel space (0..width × 0..height) to the
        // page; map that rectangle and clip it to the page.
        let (w, h) = (image.width(), image.height());
        let r = props
            .transform
            .transform_rect_bbox(Rect::new(0.0, 0.0, f64::from(w), f64::from(h)))
            .intersect(self.page);
        if !(r.area().is_finite() && r.area() > 0.0) {
            return;
        }
        self.placements.push(PageImage {
            pixels: (w, h),
            bbox: BBox {
                x0: f(r.x0),
                y0: f(r.y0),
                x1: f(r.x1),
                y1: f(r.y1),
            },
        });
    }

    fn draw_rect(&mut self, _: &Rect, _: DrawProps<'a>, _: &DrawMode) {
        self.watchdog.tick();
        self.paths = self.paths.saturating_add(1);
    }

    fn pop_clip(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

/// A device that only runs the watchdog: used to vet a page before an expensive render.
pub(crate) struct DryRun<'e>(pub Watchdog<'e>);

impl<'a> Device<'a> for DryRun<'_> {
    fn on_operator(&mut self) {
        self.0.op();
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {
        self.0.tick();
    }
    fn push_clip_path(&mut self, _: &ClipPath) {
        self.0.nest();
    }
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {
        self.0.nest();
    }
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        // Every glyph is a fill for the renderer: count glyphs, not runs.
        self.0.tick_n(run.glyphs().len() as u64);
        // The renderer runs Type3 glyph procedures; so does the dry run, so that their
        // fan-out is counted too (hayro-interpret caps their nesting depth).
        for g in run.glyphs() {
            if let HGlyph::Type3(t3) = &**g {
                t3.interpret(self, props.transform, g.transform(), &props.paint);
            }
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {
        self.0.tick();
    }
    fn draw_rect(&mut self, _: &Rect, _: DrawProps<'a>, _: &DrawMode) {
        self.0.tick();
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

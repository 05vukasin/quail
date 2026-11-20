//! A hayro [`Device`] that records text glyphs (with Unicode and position), image placements
//! and how much vector drawing a page does. Nothing is rasterized and no image is decoded.

use hayro_interpret::font::{Glyph as HGlyph, GlyphRun};
use hayro_interpret::hayro_cmap::BfString;
use hayro_interpret::kurbo::{Affine, BezPath, Point, Rect};
use hayro_interpret::{
    BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask,
};

use quail_core::{BBox, PageImage};

use crate::layout::Glyph;

/// Glyphs kept per page; a hostile page drawing millions of glyphs is truncated.
pub(crate) const MAX_GLYPHS_PER_PAGE: usize = 400_000;
/// Image placements remembered per page (the count keeps going).
const MAX_IMAGES_KEPT: usize = 64;

/// What the device saw on one page.
#[derive(Default)]
pub(crate) struct PageCapture {
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
}

impl PageCapture {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            page: Rect::new(0.0, 0.0, width.max(0.0), height.max(0.0)),
            ..Self::default()
        }
    }

    /// Share of the page area covered by the union bounding box of each image, summed and
    /// capped at 1. Overlaps count twice, which is fine for "is this page mostly a picture".
    pub fn image_coverage(&self) -> f32 {
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

impl<'a> Device<'a> for PageCapture {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {
        self.paths = self.paths.saturating_add(1);
    }

    fn push_clip_path(&mut self, _: &ClipPath) {}

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        // Invisible text (render mode 3) is included on purpose: it is the text layer of
        // OCR'd scans.
        for g in run.glyphs() {
            let text = match g.as_unicode() {
                Some(BfString::Char(c)) => c.to_string(),
                Some(BfString::String(s)) => s,
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
            let t: Affine = props.transform * g.transform();
            let origin = t * Point::new(0.0, 0.0);
            let end = t * Point::new(glyph_advance(g), 0.0);
            let top = t * Point::new(0.0, 1000.0);
            let size = (top - origin).hypot();
            self.glyphs.push(Glyph {
                text,
                x0: f(origin.x.min(end.x)),
                x1: f(origin.x.max(end.x)),
                baseline: f(origin.y),
                size: f(size),
            });
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
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

    fn pop_clip(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

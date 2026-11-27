//! [`PageRaster`] for PDF, on hayro: renders pages, or decodes a page's single embedded scan.
//!
//! [`PdfRaster`] shares the file's [`Blob`] and keeps one parsed document behind a mutex, so
//! OCR over many pages parses the file once, not once per page. Every call gets a fresh decode
//! budget (`max_decompressed`, images up to `max_render_pixels`), and the parsed document is
//! dropped once the bytes decoded since it was opened pass [`REOPEN_AFTER_BYTES`], because
//! hayro caches each page's decoded content for the life of the parse. Memory held by the
//! artifact is thus the file itself plus that bounded cache.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderAbort, RenderCache, RenderLimits, RenderSettings};
use hayro_interpret::font::GlyphRun;
use hayro_interpret::kurbo::{BezPath, Rect};
use hayro_interpret::util::TransformExt;
use hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image as HImage, ImageData,
    ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro_syntax::Pdf;
use hayro_syntax::budget::DecodeBudget;
use hayro_syntax::page::Page as HPage;
use quail_core::{Blob, Error, Format, Image, Limits, PageRaster, Result};

use crate::device::{Abort, Clock, DryRun, MAX_CALLS_PER_PAGE, Watchdog};
use crate::extract::REOPEN_AFTER_BYTES;
use crate::open::{Caught, caught, open};

/// Highest DPI [`PdfRaster::render`] accepts.
pub const MAX_DPI: f32 = 1200.0;
/// Share of the page an image must cover to count as the page's scan.
pub const SCAN_COVERAGE: f64 = 0.98;

/// Renders the pages of one PDF. Built by [`PdfExtractor`](crate::PdfExtractor) and provided
/// as the [`Raster`](quail_core::Raster) artifact; usable directly too.
pub struct PdfRaster {
    blob: Blob,
    state: Mutex<State>,
    /// Pages known to hold no single embedded scan (see [`PdfRaster::with_no_scan`]).
    no_scan: Vec<bool>,
    /// Pages the extractor stopped at the per-page draw cap (see [`PdfRaster::over_cap`]).
    over_cap: Vec<bool>,
    /// The file's deadline: every call ends by it, whatever `limits.timeout` says, so
    /// extraction, page hashing and OCR of one file share one time budget.
    file_deadline: Option<Instant>,
}

struct State {
    budget: Arc<DecodeBudget>,
    pdf: Option<Pdf>,
    decoded_since_open: u64,
}

impl std::fmt::Debug for PdfRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfRaster")
            .field("len", &self.blob.len())
            .finish_non_exhaustive()
    }
}

impl PdfRaster {
    /// A raster over the PDF in `blob`. Nothing is parsed until the first call.
    pub fn new(blob: Blob) -> Self {
        Self::with_no_scan(blob, Vec::new())
    }

    /// A raster whose calls all end by `deadline` (the file's), in addition to each call's
    /// own `limits.timeout`.
    pub fn with_deadline(blob: Blob, deadline: Option<Instant>) -> Self {
        Self::new(blob).until(deadline)
    }

    /// Ends every call by `deadline` (the file's), in addition to each call's own
    /// `limits.timeout`. The extractor builds its `Raster` this way, so extraction, page
    /// hashing and OCR of one file share one time budget.
    #[must_use]
    pub fn until(mut self, deadline: Option<Instant>) -> Self {
        self.file_deadline = deadline;
        self
    }

    /// Like [`PdfRaster::new`], told which pages hold no single embedded scan: `no_scan[i]`
    /// means page `i` was interpreted completely and drew zero or several images, so
    /// [`PageRaster::embedded_scan`] answers `None` for it without parsing or decoding
    /// anything (a page with a scan and a stamp would otherwise decode the scan only to
    /// discard it). Pages past the end of the vector are examined as usual.
    pub fn with_no_scan(blob: Blob, no_scan: Vec<bool>) -> Self {
        Self {
            file_deadline: None,
            blob,
            state: Mutex::new(State {
                budget: Arc::new(DecodeBudget::new(0, 0)),
                pdf: None,
                decoded_since_open: 0,
            }),
            no_scan,
            over_cap: Vec::new(),
        }
    }

    /// Pages whose interpretation the extractor stopped at the per-page draw-call or nesting
    /// cap: `over_cap[i]` makes [`PageRaster::render`] and the scan lookups refuse page `i` at
    /// once (`LimitExceeded`), instead of interpreting it up to the same cap twice more (the
    /// scan lookup, then the render's dry run) only to refuse it then. A 2^n form fan-out page
    /// thus costs one capped interpretation per file, not three per caller.
    #[must_use]
    pub(crate) fn over_cap(mut self, over_cap: Vec<bool>) -> Self {
        self.over_cap = over_cap;
        self
    }

    /// Did the extractor stop page `page` at the draw cap?
    fn is_over_cap(&self, page: u32) -> bool {
        usize::try_from(page)
            .ok()
            .and_then(|i| self.over_cap.get(i))
            .copied()
            .unwrap_or(false)
    }

    /// Runs `f` on page `page` of the parsed document under a fresh budget for `limits`.
    fn with_page<T>(
        &self,
        page: u32,
        limits: &Limits,
        f: impl FnOnce(&HPage<'_>, &DecodeBudget) -> Result<T>,
    ) -> Result<T> {
        if page >= limits.max_pages {
            return Err(Error::LimitExceeded("max_pages"));
        }
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.budget
            .reset(limits.max_decompressed, limits.max_render_pixels);
        if st.pdf.is_none() {
            st.pdf = Some(open(&self.blob, &st.budget)?);
            st.decoded_since_open = 0;
        }
        let budget = st.budget.clone();
        let result = match st.pdf.as_ref() {
            Some(pdf) => {
                let pages = pdf.pages();
                match pages.get(usize::try_from(page).unwrap_or(usize::MAX)) {
                    Some(p) => f(p, &budget),
                    // A page tree cut short by the decode budget is a limit, not a bad index.
                    None if budget.limit_hit() => Err(Error::LimitExceeded("max_decompressed")),
                    None => Err(Error::Other(format!("page {page} does not exist"))),
                }
            }
            None => Err(Error::Other("document not open".to_owned())),
        };
        st.decoded_since_open = st
            .decoded_since_open
            .saturating_add(limits.max_decompressed.saturating_sub(budget.remaining()));
        // Start from a fresh parse after an unwind out of hayro, past the cache margin, or
        // after ANY limit hit: hayro caches each page's decoded content (refused or partial
        // ones included) for the life of the parse, so reusing it would silently return a
        // blank page next time.
        let unwound = matches!(
            &result,
            Err(Error::Panic(_) | Error::Timeout | Error::LimitExceeded(_))
        );
        if unwound || budget.limit_hit() || st.decoded_since_open > REOPEN_AFTER_BYTES {
            st.pdf = None;
        }
        result
    }
}

/// Bytes one render may allocate for decoded images and child pixmaps (pattern tiles, stencil
/// paints, soft masks): the decode budget plus two canvases' worth at the pixel cap (at 4
/// bytes per pixel). The main canvas itself is bounded by `max_render_pixels` up front.
fn render_byte_budget(limits: &Limits) -> u64 {
    limits
        .max_decompressed
        .saturating_add(limits.max_render_pixels.saturating_mul(4))
}

/// Pixel-composites vello may spend rasterizing one render: 256 full-canvas layers' worth.
const RENDER_WORK_PER_CANVAS_PIXEL: u64 = 256;

impl PdfRaster {
    /// When a call starting now must end: its own timeout or the file's deadline, whichever
    /// comes first. `Err(Timeout)` when the file's time is already spent.
    fn call_deadline(&self, limits: &Limits) -> Result<Instant> {
        let own = Instant::now()
            .checked_add(limits.timeout)
            .unwrap_or_else(Instant::now);
        let d = self.file_deadline.map_or(own, |f| f.min(own));
        if Instant::now() >= d {
            return Err(Error::Timeout);
        }
        Ok(d)
    }
}

/// Luma from premultiplied RGBA over an opaque white background.
fn to_gray(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .map(|p| {
            let (r, g, b) = (u32::from(p[0]), u32::from(p[1]), u32::from(p[2]));
            u8::try_from(((299 * r + 587 * g + 114 * b) / 1000).min(255)).unwrap_or(u8::MAX)
        })
        .collect()
}

fn unwind_error(p: &(dyn std::any::Any + Send), what: &str) -> Error {
    match caught(p) {
        Caught::Abort(Abort::Deadline) => Error::Timeout,
        Caught::Abort(Abort::TooManyCalls) => Error::LimitExceeded("draw operations per page"),
        Caught::Panic(m) => Error::Panic(format!("{what}: {m}")),
    }
}

/// Interprets the page with a device that only counts, so that a page whose drawing fans out
/// (forms, Type3 glyphs) or runs past the deadline is refused before the expensive render.
fn dry_run(p: &HPage<'_>, deadline: Instant) -> Result<()> {
    let (w, h) = p.render_dimensions();
    catch_unwind(AssertUnwindSafe(|| {
        let cache = InterpreterCache::new();
        let mut ctx = Context::new(
            p.initial_transform(true).to_kurbo(),
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            &cache,
            p.xref(),
            InterpreterSettings::default(),
        );
        let mut dev = DryRun(Watchdog::new(Clock::Until(deadline)));
        interpret_page(p, &mut ctx, &mut dev);
    }))
    .map_err(|e| unwind_error(&*e, "dry run"))
}

impl PageRaster for PdfRaster {
    /// Renders to 8-bit gray (`channels == 1`) on white. Refuses, before allocating:
    /// - a page index at or past `limits.max_pages` (`LimitExceeded("max_pages")`);
    /// - a canvas over `limits.max_render_pixels` or hayro's 65 535 px per side
    ///   (`LimitExceeded("max_render_pixels")`);
    /// - a page whose drawing exceeds the per-page draw-call cap or `limits.timeout`, found by a
    ///   counting dry run;
    /// - a page with a stream or image over `max_decompressed` / `max_render_pixels`
    ///   (`LimitExceeded("max_decompressed")`): a render with images missing would mislead OCR.
    fn render(&self, page: u32, dpi: f32, limits: &Limits) -> Result<Image> {
        if !(dpi.is_finite() && dpi > 0.0 && dpi <= MAX_DPI) {
            return Err(Error::Other(format!("dpi {dpi} outside (0, {MAX_DPI}]")));
        }
        if self.is_over_cap(page) {
            return Err(Error::LimitExceeded("draw operations per page"));
        }
        let deadline = self.call_deadline(limits)?;
        self.with_page(page, limits, |p, budget| {
            let (w, h) = p.render_dimensions();
            let scale = dpi / 72.0;
            let (pw, ph) = ((w * scale).ceil(), (h * scale).ceil());
            if !(pw.is_finite() && ph.is_finite() && pw >= 1.0 && ph >= 1.0) {
                return Err(Error::Malformed {
                    format: Format::Pdf,
                    msg: format!("page {page} has no area"),
                });
            }
            let max_side = f32::from(u16::MAX);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let pixels = u64::from(pw.min(max_side) as u32) * u64::from(ph.min(max_side) as u32);
            if pw > max_side || ph > max_side || pixels > limits.max_render_pixels {
                return Err(Error::LimitExceeded("max_render_pixels"));
            }
            dry_run(p, deadline)?;
            // The renderer charges every decoded image and child pixmap (pattern tiles,
            // stencil paints, soft masks) to this budget before allocating, ticks the deadline
            // in every callback and tile copy, and caps callbacks.
            let render_limits = Arc::new(RenderLimits::new(
                Some(deadline),
                render_byte_budget(limits),
                MAX_CALLS_PER_PAGE,
                pixels.saturating_mul(RENDER_WORK_PER_CANVAS_PIXEL),
            ));
            let pixmap = catch_unwind(AssertUnwindSafe(|| {
                let cache = RenderCache::new();
                hayro::render_limited(
                    p,
                    &cache,
                    &InterpreterSettings::default(),
                    &RenderSettings::default(),
                    &PixmapSettings {
                        x_scale: scale,
                        y_scale: scale,
                        bg_color: WHITE,
                    },
                    render_limits,
                )
            }))
            .map_err(|e| match e.downcast_ref::<RenderAbort>() {
                Some(RenderAbort::Deadline) => Error::Timeout,
                Some(RenderAbort::Memory) => Error::LimitExceeded("render memory"),
                Some(RenderAbort::Calls) => Error::LimitExceeded("draw operations per page"),
                Some(RenderAbort::Work) => Error::LimitExceeded("render work"),
                None => unwind_error(&*e, "renderer"),
            })?;
            if budget.limit_hit() {
                return Err(Error::LimitExceeded("max_decompressed"));
            }
            Ok(Image {
                width: u32::from(pixmap.width()),
                height: u32::from(pixmap.height()),
                channels: 1,
                data: to_gray(pixmap.data_as_u8_slice()),
            })
        })
    }

    /// The single image of a scanned page, decoded as stored (1 or 3 channels). `None` unless
    /// the page draws exactly one image, upright (a positive scale from pixels to the page, so
    /// OCR can map pixel boxes to page boxes by scaling), covering at least [`SCAN_COVERAGE`]
    /// of the page; rendering is the fallback for everything else. Image masks come back as
    /// gray, ink dark.
    fn embedded_scan(&self, page: u32, limits: &Limits) -> Option<Result<Image>> {
        self.scan(page, None, limits)
    }

    /// [`PageRaster::embedded_scan`] at a reduced resolution. JPEG 2000 scans are decoded at
    /// the smallest resolution level whose longer side is still at least `max_side`; JPEG (DCT)
    /// scans at the smallest IDCT scale (1/8, 1/4 or 1/2) whose output is still at least the
    /// requested size in both dimensions. Memory (and the work after entropy decoding) falls
    /// with the square of the scale. Every other filter (JBIG2, CCITT, Flate, ...) decodes at
    /// full size, as `embedded_scan` does, and so do palette JPEG 2000 images (hayro cannot
    /// reduce them) and CMYK, YCCK, 12-bit or lossless JPEGs.
    fn embedded_scan_reduced(
        &self,
        page: u32,
        max_side: u32,
        limits: &Limits,
    ) -> Option<Result<Image>> {
        self.scan(page, Some(max_side.max(1)), limits)
    }
}

impl PdfRaster {
    /// The page's single scan, decoded at full size (`max_side == None`) or reduced towards
    /// `max_side` px on its longer side.
    fn scan(&self, page: u32, max_side: Option<u32>, limits: &Limits) -> Option<Result<Image>> {
        if usize::try_from(page)
            .ok()
            .and_then(|i| self.no_scan.get(i))
            .copied()
            .unwrap_or(false)
            && page < limits.max_pages
        {
            return None;
        }
        if self.is_over_cap(page) {
            return Some(Err(Error::LimitExceeded("draw operations per page")));
        }
        let deadline = match self.call_deadline(limits) {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        let found = self.with_page(page, limits, |p, budget| {
            let (w, h) = p.render_dimensions();
            let dev = catch_unwind(AssertUnwindSafe(|| {
                let cache = InterpreterCache::new();
                let mut ctx = Context::new(
                    p.initial_transform(true).to_kurbo(),
                    Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                    &cache,
                    p.xref(),
                    InterpreterSettings::default(),
                );
                let mut dev = ScanDevice {
                    page: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                    images: 0,
                    max_side,
                    result: None,
                    watchdog: Watchdog::new(Clock::Until(deadline)),
                };
                interpret_page(p, &mut ctx, &mut dev);
                (dev.images, dev.result)
            }))
            .map_err(|e| unwind_error(&*e, "scan"))?;
            if budget.limit_hit() {
                return Err(Error::LimitExceeded("max_decompressed"));
            }
            Ok(dev)
        });
        match found {
            Err(e) => Some(Err(e)),
            Ok((1, Some(img))) => Some(Ok(img)),
            Ok(_) => None,
        }
    }
}

/// The target size hayro's decoders get for a reduced decode of a `w` × `h` image: the image
/// scaled so that its longer side is `max_side`, rounded up; `None` when it is no larger.
/// hayro's JPEG 2000 decoder skips `⌊log2(w / tw)⌋` or `⌊log2(h / th)⌋` resolution levels,
/// whichever is smaller, which keeps the longer side at or above `max_side`; its DCT decoder
/// takes the smallest IDCT scale whose output covers the target in both dimensions.
fn reduced_target(w: u32, h: u32, max_side: u32) -> Option<(u32, u32)> {
    let long = w.max(h);
    if max_side == 0 || long <= max_side {
        return None;
    }
    let scale = |v: u32| {
        u32::try_from((u64::from(v) * u64::from(max_side)).div_ceil(u64::from(long)))
            .unwrap_or(v)
            .max(1)
    };
    Some((scale(w), scale(h)))
}

/// Decodes the first image drawn when it is upright and covers the page; counts the rest.
struct ScanDevice<'e> {
    page: Rect,
    images: usize,
    /// Reduced decode: about this many px on the image's longer side.
    max_side: Option<u32>,
    result: Option<Image>,
    watchdog: Watchdog<'e>,
}

impl<'a> Device<'a> for ScanDevice<'_> {
    fn on_operator(&mut self) {
        self.watchdog.op();
    }

    fn on_x_object(&mut self, bytes: usize) {
        self.watchdog.x_object(bytes);
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {
        self.watchdog.tick();
    }
    fn push_clip_path(&mut self, _: &ClipPath) {
        self.watchdog.nest();
    }
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {
        self.watchdog.nest();
    }
    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {
        self.watchdog.tick();
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_image(&mut self, image: HImage<'a, '_>, props: ImageDrawProps<'a>) {
        self.watchdog.tick();
        self.images += 1;
        if self.images > 1 {
            self.result = None;
            return;
        }
        // Upright: a pure positive scale from image pixels to the page (top-left origin).
        let [sx, shy, shx, sy, _, _] = props.transform.as_coeffs();
        let eps = 1e-6 * sx.abs().max(sy.abs()).max(1e-9);
        if !(sx > 0.0 && sy > 0.0 && shy.abs() <= eps && shx.abs() <= eps) {
            return;
        }
        let (iw, ih) = (image.width(), image.height());
        let placed = props
            .transform
            .transform_rect_bbox(Rect::new(0.0, 0.0, f64::from(iw), f64::from(ih)))
            .intersect(self.page);
        let page_area = self.page.area();
        if !(page_area > 0.0 && placed.area() / page_area >= SCAN_COVERAGE) {
            return;
        }
        let target = self.max_side.and_then(|m| reduced_target(iw, ih, m));
        // Decoding goes through the file's budget: pixels and bytes are capped by the
        // vendored decoders before anything is allocated.
        let mut out: Option<Image> = None;
        match &image {
            HImage::Raster(raster) => raster.with_rgba(
                |data, _alpha| {
                    out = Some(match data {
                        ImageData::Luma(luma) => Image {
                            width: luma.width,
                            height: luma.height,
                            channels: 1,
                            data: luma.data,
                        },
                        ImageData::Rgb(rgb) => Image {
                            width: rgb.width,
                            height: rgb.height,
                            channels: 3,
                            data: rgb.data,
                        },
                    });
                },
                target,
            ),
            HImage::Stencil(stencil) => stencil.with_stencil(
                |mask, _paint| {
                    out = Some(Image {
                        width: mask.width,
                        height: mask.height,
                        channels: 1,
                        // Mask coverage 255 = painted (ink): invert to dark-on-white.
                        data: mask.data.iter().map(|v| 255 - v).collect(),
                    });
                },
                target,
            ),
        }
        self.result = out;
    }

    fn draw_rect(&mut self, _: &Rect, _: DrawProps<'a>, _: &DrawMode) {
        self.watchdog.tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_conversion() {
        assert_eq!(to_gray(&[255, 255, 255, 255, 0, 0, 0, 255]), [255, 0]);
        assert!(to_gray(&[1, 2]).is_empty());
    }

    #[test]
    fn reduced_targets_keep_the_longer_side() {
        assert_eq!(reduced_target(100, 50, 128), None);
        assert_eq!(reduced_target(128, 64, 128), None);
        assert_eq!(reduced_target(5, 5, 0), None);
        assert_eq!(reduced_target(2480, 3508, 128), Some((91, 128)));
        assert_eq!(reduced_target(10_000, 1, 128), Some((128, 1)));
        // The levels hayro skips keep the longer side at or above the request.
        for (w, h) in [
            (2480, 3508),
            (3508, 2480),
            (1700, 2200),
            (999, 333),
            (10_000, 1),
        ] {
            let (tw, th) = reduced_target(w, h, 128).unwrap_or((w, h));
            let k = (w / tw).ilog2().min((h / th).ilog2());
            assert!(w.max(h) >> k >= 128, "{w}x{h}");
        }
    }

    #[test]
    fn raster_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<PdfRaster>();
    }
}

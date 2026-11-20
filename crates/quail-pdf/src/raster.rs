//! [`PageRaster`] for PDF, on hayro: renders pages, or decodes a page's single embedded scan.
//!
//! [`PdfRaster`] holds only the file's [`Blob`] (shared, not copied) and re-parses the xref
//! per call. Parsing is cheap next to rendering, and it keeps the artifact's memory bounded by
//! the file itself for as long as the file's artifacts live.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use hayro_interpret::font::GlyphRun;
use hayro_interpret::kurbo::{BezPath, Rect};
use hayro_interpret::util::TransformExt;
use hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image as HImage, ImageData,
    ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro_syntax::{DecryptionError, LoadPdfError, Pdf, PdfData};
use quail_core::{Blob, Error, Format, Image, Limits, PageRaster, Result};

use crate::budget::Budget;
use crate::extract::{BlobBytes, panic_msg};

/// Highest DPI [`PdfRaster::render`] accepts.
pub const MAX_DPI: f32 = 1200.0;

/// Renders the pages of one PDF. Built by [`PdfExtractor`](crate::PdfExtractor) and provided
/// as the [`Raster`](quail_core::Raster) artifact; usable directly too.
#[derive(Clone, Debug)]
pub struct PdfRaster {
    blob: Blob,
}

impl PdfRaster {
    /// A raster over the PDF in `blob`.
    pub fn new(blob: Blob) -> Self {
        Self { blob }
    }

    fn open(&self) -> Result<Pdf> {
        let data = PdfData::from(Arc::new(BlobBytes(self.blob.clone())));
        match catch_unwind(AssertUnwindSafe(|| Pdf::new(data))) {
            Ok(Ok(pdf)) => Ok(pdf),
            Ok(Err(LoadPdfError::Decryption(DecryptionError::PasswordProtected))) => {
                Err(Error::Encrypted)
            }
            Ok(Err(_)) => Err(Error::Malformed {
                format: Format::Pdf,
                msg: "cannot open for rendering".to_owned(),
            }),
            Err(p) => Err(Error::Panic(format!("pdf parser: {}", panic_msg(&*p)))),
        }
    }
}

fn page_error(page: u32) -> Error {
    Error::Other(format!("page {page} does not exist"))
}

/// Luma from premultiplied RGBA over an opaque white background.
fn to_gray(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .map(|p| {
            let (r, g, b) = (u32::from(p[0]), u32::from(p[1]), u32::from(p[2]));
            // `min` keeps the cast lossless.
            u8::try_from(((299 * r + 587 * g + 114 * b) / 1000).min(255)).unwrap_or(u8::MAX)
        })
        .collect()
}

impl PageRaster for PdfRaster {
    /// Renders to 8-bit gray (`channels == 1`) on white. Fails with
    /// `LimitExceeded("max_render_pixels")` before allocating when the page at `dpi` is too
    /// big, and with `LimitExceeded("max_decompressed")` when its streams or images would
    /// inflate past the budget.
    fn render(&self, page: u32, dpi: f32, limits: &Limits) -> Result<Image> {
        if !(dpi.is_finite() && dpi > 0.0 && dpi <= MAX_DPI) {
            return Err(Error::Other(format!("dpi {dpi} outside (0, {MAX_DPI}]")));
        }
        let pdf = self.open()?;
        let pages = pdf.pages();
        let p = pages
            .get(usize::try_from(page).unwrap_or(usize::MAX))
            .ok_or_else(|| page_error(page))?;
        let (w, h) = p.render_dimensions();
        let scale = dpi / 72.0;
        let (pw, ph) = ((w * scale).ceil(), (h * scale).ceil());
        if !(pw.is_finite() && ph.is_finite() && pw >= 1.0 && ph >= 1.0) {
            return Err(Error::Malformed {
                format: Format::Pdf,
                msg: format!("page {page} has no area"),
            });
        }
        // hayro's canvas is u16 per side.
        let max_side = f32::from(u16::MAX);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pixels = u64::from(pw.min(max_side) as u32) * u64::from(ph.min(max_side) as u32);
        if pw > max_side || ph > max_side || pixels > limits.max_render_pixels {
            return Err(Error::LimitExceeded("max_render_pixels"));
        }
        let mut budget = Budget::new(limits.max_decompressed, self.blob.len());
        if !(budget.charge_page(p) && budget.charge_images(p)) {
            return Err(Error::LimitExceeded("max_decompressed"));
        }
        let rendered = catch_unwind(AssertUnwindSafe(|| {
            let cache = RenderCache::new();
            hayro::render(
                p,
                &cache,
                &InterpreterSettings::default(),
                &RenderSettings::default(),
                &PixmapSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: WHITE,
                },
            )
        }));
        let pixmap = rendered.map_err(|e| Error::Panic(format!("renderer: {}", panic_msg(&*e))))?;
        Ok(Image {
            width: u32::from(pixmap.width()),
            height: u32::from(pixmap.height()),
            channels: 1,
            data: to_gray(pixmap.data_as_u8_slice()),
        })
    }

    /// The single image of a scanned page, decoded as stored (1 or 3 channels). `None` when the
    /// page does not hold exactly one image, or when that image is rotated or mirrored on the
    /// page (then rendering gives the upright page). Image masks come back as gray, ink dark.
    fn embedded_scan(&self, page: u32, limits: &Limits) -> Option<Result<Image>> {
        let pdf = match self.open() {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let pages = pdf.pages();
        let Some(p) = pages.get(usize::try_from(page).unwrap_or(usize::MAX)) else {
            return Some(Err(page_error(page)));
        };
        let mut budget = Budget::new(limits.max_decompressed, self.blob.len());
        if !(budget.charge_page(p) && budget.charge_images(p)) {
            return Some(Err(Error::LimitExceeded("max_decompressed")));
        }
        let (w, h) = p.render_dimensions();
        let found = catch_unwind(AssertUnwindSafe(|| {
            let cache = InterpreterCache::new();
            let mut ctx = Context::new(
                p.initial_transform(true).to_kurbo(),
                Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                &cache,
                p.xref(),
                InterpreterSettings::default(),
            );
            let mut dev = ScanDevice {
                max_pixels: limits.max_render_pixels,
                ..ScanDevice::default()
            };
            interpret_page(p, &mut ctx, &mut dev);
            dev
        }));
        let dev = match found {
            Ok(d) => d,
            Err(e) => return Some(Err(Error::Panic(format!("scan: {}", panic_msg(&*e))))),
        };
        if dev.images != 1 {
            return None;
        }
        match dev.result {
            Some(Ok(img)) => Some(Ok(img)),
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    }
}

/// Decodes the first image drawn, if it is upright, and counts the rest.
#[derive(Default)]
struct ScanDevice {
    images: usize,
    max_pixels: u64,
    result: Option<Result<Image>>,
}

impl<'a> Device<'a> for ScanDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_image(&mut self, image: HImage<'a, '_>, props: ImageDrawProps<'a>) {
        self.images += 1;
        if self.images > 1 {
            self.result = None;
            return;
        }
        // Upright: a pure positive scale from image pixels to the page (top-left origin).
        let [a, b, c, d, _, _] = props.transform.as_coeffs();
        let eps = 1e-6 * a.abs().max(d.abs()).max(1e-9);
        if !(a > 0.0 && d > 0.0 && b.abs() <= eps && c.abs() <= eps) {
            return;
        }
        let (w, h) = (image.width(), image.height());
        if u64::from(w) * u64::from(h) > self.max_pixels {
            self.result = Some(Err(Error::LimitExceeded("max_render_pixels")));
            return;
        }
        let mut out: Option<Image> = None;
        match &image {
            HImage::Raster(r) => r.with_rgba(
                |data, _alpha| {
                    out = Some(match data {
                        ImageData::Luma(l) => Image {
                            width: l.width,
                            height: l.height,
                            channels: 1,
                            data: l.data,
                        },
                        ImageData::Rgb(c) => Image {
                            width: c.width,
                            height: c.height,
                            channels: 3,
                            data: c.data,
                        },
                    });
                },
                None,
            ),
            HImage::Stencil(s) => s.with_stencil(
                |l, _paint| {
                    out = Some(Image {
                        width: l.width,
                        height: l.height,
                        channels: 1,
                        // Mask coverage 255 = painted (ink): invert to dark-on-white.
                        data: l.data.iter().map(|v| 255 - v).collect(),
                    });
                },
                None,
            ),
        }
        self.result = out.map(Ok);
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
}

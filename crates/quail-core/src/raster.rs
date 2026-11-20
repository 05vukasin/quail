use std::fmt;
use std::sync::Arc;

use crate::artifact::Artifact;
use crate::error::Result;
use crate::limits::Limits;

/// A decoded raster image, 8 bits per channel, rows top to bottom.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// 1 (gray), 3 (RGB) or 4 (RGBA).
    pub channels: u8,
    /// `width * height * channels` bytes.
    pub data: Vec<u8>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}x{}x{})", self.width, self.height, self.channels)
    }
}

/// Turns pages of one document into pixels. Implemented by extractors whose format has
/// geometry (PDF); used by OCR and by inspection tools, which then never depend on the
/// extractor crate itself.
pub trait PageRaster: Send + Sync {
    /// Renders page `page` (zero-based) at `dpi`, refusing to exceed
    /// `limits.max_render_pixels`.
    fn render(&self, page: u32, dpi: f32, limits: &Limits) -> Result<Image>;

    /// The page's single embedded scan, decoded, when the page is one image (see
    /// [`Page::is_scan`](crate::Page::is_scan)); cheaper and sharper than rendering.
    fn embedded_scan(&self, page: u32, limits: &Limits) -> Option<Result<Image>> {
        let _ = (page, limits);
        None
    }
}

/// Artifact giving later steps a [`PageRaster`] for the current file.
#[derive(Clone)]
pub struct Raster(pub Arc<dyn PageRaster>);

impl Artifact for Raster {
    const NAME: &'static str = "raster";
}

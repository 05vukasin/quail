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

    /// Like [`PageRaster::embedded_scan`], for callers that need only about `max_side` pixels
    /// on the image's longer side (thumbnails, perceptual hashes): decoders that can decode
    /// at a lower resolution (JPEG 2000 resolution levels, scaled JPEG IDCT) skip the work
    /// and memory of the full one.
    ///
    /// The result's longer side is at least `max_side` and at most the full size, since
    /// decoders scale by powers of two; it is never smaller than requested unless the source
    /// is. Images whose decoder cannot reduce come back at full size. Same `None` cases as
    /// `embedded_scan`. The default calls `embedded_scan`.
    fn embedded_scan_reduced(
        &self,
        page: u32,
        max_side: u32,
        limits: &Limits,
    ) -> Option<Result<Image>> {
        let _ = max_side;
        self.embedded_scan(page, limits)
    }
}

/// Artifact giving later steps a [`PageRaster`] for the current file.
#[derive(Clone)]
pub struct Raster(pub Arc<dyn PageRaster>);

impl Artifact for Raster {
    const NAME: &'static str = "raster";
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Full;

    impl PageRaster for Full {
        fn render(&self, _: u32, _: f32, _: &Limits) -> Result<Image> {
            Err(crate::Error::Other("no render".to_owned()))
        }
        fn embedded_scan(&self, _: u32, _: &Limits) -> Option<Result<Image>> {
            Some(Ok(Image {
                width: 4,
                height: 2,
                channels: 1,
                data: vec![0; 8],
            }))
        }
    }

    #[test]
    fn reduced_scan_defaults_to_the_full_scan() {
        let limits = Limits::default();
        let full = Full.embedded_scan(0, &limits).map(Result::ok);
        assert_eq!(
            Full.embedded_scan_reduced(0, 1, &limits).map(Result::ok),
            full
        );
    }
}

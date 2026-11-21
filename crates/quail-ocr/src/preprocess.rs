//! Pixel work done before an engine sees a page: picking a DPI that fits the pixel budget and
//! shrinking oversized scans.

use quail_core::{Error, Image, Result};

/// Highest DPI that keeps a `w × h` point page within `max_pixels`, capped at `target`.
/// `None` when the page size is unusable.
pub fn fit_dpi(size_pt: (f32, f32), target: f32, max_pixels: u64) -> Option<f32> {
    let (w, h) = size_pt;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && target > 0.0) {
        return None;
    }
    let area_in2 = f64::from(w) / 72.0 * f64::from(h) / 72.0;
    #[allow(clippy::cast_precision_loss)]
    let max_dpi = (max_pixels as f64 / area_in2).sqrt();
    #[allow(clippy::cast_possible_truncation)]
    let dpi = f64::from(target).min(max_dpi) as f32;
    // Leave a pixel of slack for the renderer's rounding up.
    let dpi = dpi * 0.995;
    (dpi.is_finite() && dpi >= 1.0).then_some(dpi)
}

/// Checks that `image` is well-formed: known channel count, buffer of the stated size.
pub fn validate(image: &Image) -> Result<()> {
    let expected = u64::from(image.width) * u64::from(image.height) * u64::from(image.channels);
    if !matches!(image.channels, 1 | 3 | 4) || image.width == 0 || image.height == 0 {
        return Err(Error::Other(format!(
            "unusable image {}x{}x{}",
            image.width, image.height, image.channels
        )));
    }
    if image.data.len() as u64 != expected {
        return Err(Error::Other(format!(
            "image buffer is {} bytes, expected {expected}",
            image.data.len()
        )));
    }
    Ok(())
}

/// Shrinks `image` by area averaging so that it has at most `max_pixels` pixels and its long
/// side at most `max_side`. Returns it unchanged when it already fits. Never enlarges.
pub fn shrink_to_fit(image: Image, max_pixels: u64, max_side: u32) -> Image {
    if validate(&image).is_err() {
        return image;
    }
    let (w, h) = (u64::from(image.width), u64::from(image.height));
    let px = w * h;
    #[allow(clippy::cast_precision_loss)]
    let s_px = if px > max_pixels {
        (max_pixels as f64 / px as f64).sqrt()
    } else {
        1.0
    };
    let long = w.max(h);
    #[allow(clippy::cast_precision_loss)]
    let s_side = if long > u64::from(max_side) {
        f64::from(max_side) / long as f64
    } else {
        1.0
    };
    let s = s_px.min(s_side);
    if s >= 1.0 {
        return image;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (nw, nh) = (
        ((f64::from(image.width) * s).floor() as u32).max(1),
        ((f64::from(image.height) * s).floor() as u32).max(1),
    );
    resize_area(&image, nw, nh)
}

/// Area-average resize of a [`validate`]d image to `nw × nh` (a downscale; upscaling repeats
/// pixels).
pub(crate) fn resize_area(image: &Image, nw: u32, nh: u32) -> Image {
    let ch = usize::from(image.channels);
    let (w, h) = (image.width as usize, image.height as usize);
    let (out_w, out_h) = (nw as usize, nh as usize);
    let mut out = vec![0u8; out_w * out_h * ch];
    // Column spans are the same for every row.
    let spans_x: Vec<(usize, usize)> = (0..out_w).map(|x| span(x, out_w, w)).collect();
    let mut acc = vec![0u32; out_w * ch];
    for y in 0..out_h {
        let (y0, y1) = span(y, out_h, h);
        acc.fill(0);
        for sy in y0..y1 {
            let row = &image.data[sy * w * ch..(sy + 1) * w * ch];
            for (x, &(x0, x1)) in spans_x.iter().enumerate() {
                for sx in x0..x1 {
                    for c in 0..ch {
                        acc[x * ch + c] += u32::from(row[sx * ch + c]);
                    }
                }
            }
        }
        for (x, &(x0, x1)) in spans_x.iter().enumerate() {
            // At most (w/nw + 1) * (h/nh + 1) samples; fits u32 for any sane page.
            let n = u32::try_from((x1 - x0) * (y1 - y0))
                .unwrap_or(u32::MAX)
                .max(1);
            for c in 0..ch {
                let v = (acc[x * ch + c] + n / 2) / n;
                out[(y * out_w + x) * ch + c] = u8::try_from(v.min(255)).unwrap_or(u8::MAX);
            }
        }
    }
    Image {
        width: nw,
        height: nh,
        channels: image.channels,
        data: out,
    }
}

/// Source range `[a, b)` (non-empty) of output index `i` when `n_out` cells cover `n_in`.
fn span(i: usize, n_out: usize, n_in: usize) -> (usize, usize) {
    let a = i * n_in / n_out;
    let b = ((i + 1) * n_in).div_ceil(n_out).min(n_in);
    (a.min(n_in.saturating_sub(1)), b.max(a + 1).min(n_in.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_fits_budget() {
        let a4 = (595.0, 842.0);
        assert!((fit_dpi(a4, 200.0, 40_000_000).unwrap_or(0.0) - 199.0).abs() < 0.5);
        let d = fit_dpi(a4, 300.0, 4_000_000).unwrap_or(0.0);
        let px = f64::from(595.0 * d / 72.0) * f64::from(842.0 * d / 72.0);
        assert!(px <= 4_000_000.0 && px > 3_900_000.0, "{d} {px}");
        assert_eq!(fit_dpi((0.0, 842.0), 200.0, 1 << 30), None);
        assert_eq!(fit_dpi((f32::NAN, 842.0), 200.0, 1 << 30), None);
        assert_eq!(fit_dpi(a4, 200.0, 0), None);
    }

    #[test]
    fn shrink_averages() {
        let img = Image {
            width: 4,
            height: 2,
            channels: 1,
            data: vec![0, 255, 0, 255, 0, 255, 0, 255],
        };
        let s = shrink_to_fit(img.clone(), 2, 100);
        assert_eq!((s.width, s.height), (2, 1));
        assert_eq!(s.data, vec![128, 128]);
        assert_eq!(shrink_to_fit(img.clone(), 100, 100), img);
        let side = shrink_to_fit(img, 100, 2);
        assert_eq!((side.width, side.height), (2, 1));
    }

    #[test]
    fn resize_odd_sizes_and_rgb() {
        let img = Image {
            width: 7,
            height: 5,
            channels: 3,
            data: (0..105u32).map(|v| u8::try_from(v).unwrap_or(0)).collect(),
        };
        for (w, h) in [(3, 2), (1, 1), (7, 5), (6, 4)] {
            let r = resize_area(&img, w, h);
            assert_eq!(r.data.len(), (w * h * 3) as usize);
        }
        assert!(validate(&img).is_ok());
    }

    #[test]
    fn rejects_bad_images() {
        let bad = Image {
            width: 2,
            height: 2,
            channels: 2,
            data: vec![0; 8],
        };
        assert!(validate(&bad).is_err());
        let short = Image {
            width: 2,
            height: 2,
            channels: 1,
            data: vec![0; 3],
        };
        assert!(validate(&short).is_err());
    }
}

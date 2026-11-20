//! Perceptual hash of page images: two scans of one paper land a few bits apart.
//!
//! # Definition (stable)
//!
//! 1. Gray: 1 channel as is; RGB(A) as `(299 R + 587 G + 114 B) / 1000` (alpha ignored).
//! 2. Shrink to 32 × 32 by averaging each output cell's source pixels (box filter).
//! 3. 2-D DCT-II of the 32 × 32 block; keep the top-left 8 × 8 low frequencies (DC included).
//! 4. Bit `63 − (8·row + col)` is set when that coefficient is above the median of the 64.
//!
//! Compare page by page with [`crate::hamming`]: ≤ [`PAGE_HAMMING_MAX`] bits is the same page.
//! Deliberately all-or-nothing at the document level: two scans are the same document when
//! their page counts agree and every page pair is within the bar (a form reused with
//! different handwriting matches on some pages only).

/// Hamming distance (of 64) at or below which two page hashes show the same page.
pub const PAGE_HAMMING_MAX: u32 = 8;

const N: usize = 32;

/// Gray 32 × 32 box-filtered thumbnail of an 8-bit image buffer; `None` when the buffer
/// does not match its size or the image is empty.
#[allow(clippy::many_single_char_names)]
pub fn thumbnail(width: u32, height: u32, channels: u8, data: &[u8]) -> Option<[f32; N * N]> {
    let (w, h, c) = (width as usize, height as usize, usize::from(channels));
    if w == 0
        || h == 0
        || !matches!(c, 1 | 3 | 4)
        || data.len() != w.checked_mul(h)?.checked_mul(c)?
    {
        return None;
    }
    let mut sum = [0f64; N * N];
    let mut count = [0u32; N * N];
    for y in 0..h {
        let ty = y * N / h;
        let row = &data[y * w * c..(y + 1) * w * c];
        for x in 0..w {
            let tx = x * N / w;
            let p = &row[x * c..x * c + c];
            let g = if c == 1 {
                f64::from(p[0])
            } else {
                (299.0 * f64::from(p[0]) + 587.0 * f64::from(p[1]) + 114.0 * f64::from(p[2]))
                    / 1000.0
            };
            sum[ty * N + tx] += g;
            count[ty * N + tx] += 1;
        }
    }
    let mut out = [0f32; N * N];
    for ((o, s), n) in out.iter_mut().zip(sum).zip(count) {
        #[allow(clippy::cast_possible_truncation)] // a mean of bytes
        let mean = if n == 0 {
            255.0
        } else {
            (s / f64::from(n)) as f32
        };
        *o = mean;
    }
    Some(out)
}

/// pHash of a 32 × 32 gray thumbnail.
pub fn phash_of_thumbnail(t: &[f32; N * N]) -> u64 {
    // cos((2x + 1) u π / 2N), u < 8 only.
    let mut cos = [[0f64; N]; 8];
    for (u, row) in cos.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)] // small integers
            let arg = (2.0 * x as f64 + 1.0) * u as f64 * std::f64::consts::PI / (2.0 * N as f64);
            *v = arg.cos();
        }
    }
    // Rows first: r[y][u] = Σx t[y][x] cos[u][x].
    let mut r = [[0f64; 8]; N];
    for (y, ry) in r.iter_mut().enumerate() {
        for (u, v) in ry.iter_mut().enumerate() {
            *v = (0..N).map(|x| f64::from(t[y * N + x]) * cos[u][x]).sum();
        }
    }
    let mut coef = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            coef[v * 8 + u] = (0..N).map(|y| r[y][u] * cos[v][y]).sum();
        }
    }
    let mut sorted = coef;
    sorted.sort_unstable_by(f64::total_cmp);
    let median = f64::midpoint(sorted[31], sorted[32]);
    coef.iter()
        .enumerate()
        .filter(|(_, c)| **c > median)
        .fold(0u64, |acc, (i, _)| acc | 1 << (63 - i))
}

/// pHash of an 8-bit image buffer; `None` when the buffer does not match its size.
pub fn phash(width: u32, height: u32, channels: u8, data: &[u8]) -> Option<u64> {
    thumbnail(width, height, channels, data).map(|t| phash_of_thumbnail(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, flip: bool) -> Vec<u8> {
        let mut d = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = (x * 255 / w + y * 64 / h) % 256;
                let v = u8::try_from(v).unwrap();
                d.push(if flip { 255 - v } else { v });
            }
        }
        d
    }

    #[test]
    fn scale_invariant_and_contrast_sensitive() {
        let a = phash(320, 400, 1, &gradient(320, 400, false)).unwrap();
        let b = phash(160, 200, 1, &gradient(160, 200, false)).unwrap();
        let c = phash(320, 400, 1, &gradient(320, 400, true)).unwrap();
        assert!(crate::hamming(a, b) <= 4, "{}", crate::hamming(a, b));
        assert!(crate::hamming(a, c) > 20);
    }

    #[test]
    fn rejects_bad_buffers() {
        assert!(phash(0, 10, 1, &[]).is_none());
        assert!(phash(2, 2, 1, &[0; 3]).is_none());
        assert!(phash(2, 2, 2, &[0; 8]).is_none());
        assert!(phash(2, 2, 3, &[9; 12]).is_some());
    }
}

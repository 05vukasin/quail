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

/// First source index of each of the `N` output cells along an axis of `len` pixels, plus
/// `len` at the end: pixel `i` lands in cell `i · N / len` (integer division), so cell `t`
/// starts at `⌈t · len / N⌉`. A cell whose range is empty (`len < N`) gets no pixels.
fn cell_starts(len: usize) -> [usize; N + 1] {
    let mut s = [0; N + 1];
    for (t, v) in s.iter_mut().enumerate() {
        *v = (t * len).div_ceil(N);
    }
    s
}

/// Sum of bytes; `u32` lanes (vectorize well) over chunks short enough not to overflow.
fn sum_bytes(s: &[u8]) -> u64 {
    s.chunks(1 << 16)
        .map(|c| u64::from(c.iter().map(|&p| u32::from(p)).sum::<u32>()))
        .sum()
}

/// Gray 32 × 32 box-filtered thumbnail of an 8-bit image buffer; `None` when the buffer
/// does not match its size or the image is empty.
///
/// Runs cell by cell over precomputed pixel ranges instead of dividing per pixel. Gray
/// input is summed in integers (exact, so equal to the `f64` sum of the definition); color
/// input adds each pixel's luma to its cell in the same order as a plain row-major walk, so
/// the result is bit-for-bit the same.
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
    let xs = cell_starts(w);
    let ys = cell_starts(h);
    let mut sum = [0f64; N * N];
    if c == 1 {
        // Integer sums are exact; an `f64` holds them exactly too (below 2^53).
        let mut isum = [0u64; N * N];
        for (ty, rows) in ys.windows(2).enumerate() {
            let cells = &mut isum[ty * N..(ty + 1) * N];
            for row in data[rows[0] * w..rows[1] * w].chunks_exact(w) {
                for (cell, cols) in cells.iter_mut().zip(xs.windows(2)) {
                    *cell += sum_bytes(&row[cols[0]..cols[1]]);
                }
            }
        }
        for (s, i) in sum.iter_mut().zip(isum) {
            #[allow(clippy::cast_precision_loss)] // exact below 2^53
            let v = i as f64;
            *s = v;
        }
    } else {
        for (ty, rows) in ys.windows(2).enumerate() {
            let cells = &mut sum[ty * N..(ty + 1) * N];
            for row in data[rows[0] * w * c..rows[1] * w * c].chunks_exact(w * c) {
                for (cell, cols) in cells.iter_mut().zip(xs.windows(2)) {
                    let mut acc = *cell;
                    for p in row[cols[0] * c..cols[1] * c].chunks_exact(c) {
                        // 299 R + 587 G + 114 B is an integer below 2^18: the same `f64`
                        // as the sum of the three products.
                        let luma =
                            299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]);
                        acc += f64::from(luma) / 1000.0;
                    }
                    *cell = acc;
                }
            }
        }
    }
    let mut out = [0f32; N * N];
    for (ty, rows) in ys.windows(2).enumerate() {
        for (tx, cols) in xs.windows(2).enumerate() {
            let n = (rows[1] - rows[0]) * (cols[1] - cols[0]);
            let i = ty * N + tx;
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            // a mean of bytes; pixel counts are exact in f64
            let mean = if n == 0 {
                255.0
            } else {
                (sum[i] / n as f64) as f32
            };
            out[i] = mean;
        }
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

    /// The definition, literally: per-pixel cell index and `f64` sums in row-major order.
    #[allow(clippy::many_single_char_names)]
    fn thumbnail_reference(w: u32, h: u32, ch: u8, data: &[u8]) -> Option<[f32; N * N]> {
        let (w, h, c) = (w as usize, h as usize, usize::from(ch));
        if w == 0 || h == 0 || !matches!(c, 1 | 3 | 4) || data.len() != w * h * c {
            return None;
        }
        let mut sum = [0f64; N * N];
        let mut count = [0u32; N * N];
        for y in 0..h {
            let ty = y * N / h;
            for x in 0..w {
                let tx = x * N / w;
                let p = &data[(y * w + x) * c..(y * w + x) * c + c];
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
            #[allow(clippy::cast_possible_truncation)]
            let mean = if n == 0 {
                255.0
            } else {
                (s / f64::from(n)) as f32
            };
            *o = mean;
        }
        Some(out)
    }

    #[test]
    fn thumbnail_matches_the_definition_bit_for_bit() {
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let sizes = [
            (1, 1),
            (5, 3),
            (31, 33),
            (32, 32),
            (33, 31),
            (64, 1),
            (1, 70),
            (97, 61),
            (250, 333),
            (612, 792),
            (1000, 7),
        ];
        for (w, h) in sizes {
            for ch in [1u8, 3, 4] {
                for noisy in [false, true] {
                    let n = w * h * usize::from(ch);
                    let data: Vec<u8> = (0..n)
                        .map(|i| {
                            if noisy {
                                (next() >> 24).to_le_bytes()[0]
                            } else {
                                u8::try_from(i * 7 % 256).unwrap()
                            }
                        })
                        .collect();
                    let (w32, h32) = (u32::try_from(w).unwrap(), u32::try_from(h).unwrap());
                    let a = thumbnail(w32, h32, ch, &data).unwrap();
                    let b = thumbnail_reference(w32, h32, ch, &data).unwrap();
                    assert!(
                        a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()),
                        "{w}x{h}x{ch} noisy={noisy}"
                    );
                    assert_eq!(phash_of_thumbnail(&a), phash_of_thumbnail(&b));
                }
            }
        }
    }

    #[test]
    fn rejects_bad_buffers() {
        assert!(phash(0, 10, 1, &[]).is_none());
        assert!(phash(2, 2, 1, &[0; 3]).is_none());
        assert!(phash(2, 2, 2, &[0; 8]).is_none());
        assert!(phash(2, 2, 3, &[9; 12]).is_some());
    }
}

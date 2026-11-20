//! 64-bit SimHash.
//!
//! [`simhash`] is the classic construction: every feature is a 64-bit hash with a weight;
//! bit `i` of the result is set when the features whose bit `i` is set outweigh those whose
//! bit is clear (ties give 0). Similar feature multisets give sketches a few bits apart.
//!
//! # `sketch.simhash_text` (stable definition)
//!
//! Features are the word 3-shingles of [`crate::minhash`] (normalized text, folded tokens,
//! `quail-text` XXH3 hashes), each weighted `1 + floor(log2(count))`: repeats count, but a
//! boilerplate line repeated on every page does not drown the rest. Compare with
//! [`crate::hamming`]: at most [`TEXT_HAMMING_MAX`] bits for a near-duplicate.
//!
//! Measured against the MinHash estimate on 352 private business PDFs/DOCX (61 776 pairs):
//! every pair with J ≥ 0.9 is within 5 bits; at ≤ 6 bits 74 pairs are flagged, 72 of them
//! with J ≥ 0.6. The expected distance is about `64·θ/π` with `θ` the angle between the
//! shingle vectors, so ≤ 6 bits ≈ J ≥ 0.9 and ≤ 12 bits ≈ J ≥ 0.6 (with many more false
//! candidates). SimHash is cheaper to index than MinHash (one `u64`; split into 4 blocks of
//! 16 bits, a pair within 3 bits shares a block exactly) but coarser: confirm with MinHash.

use crate::minhash::shingle_hashes;

/// Hamming distance (of 64) at or below which two `sketch.simhash_text` values are
/// near-duplicate candidates.
pub const TEXT_HAMMING_MAX: u32 = 6;

/// SimHash of weighted feature hashes.
///
/// ```
/// use quail_sketch::simhash::simhash;
/// assert_eq!(simhash([(0b101, 1)]), 0b101);
/// assert_eq!(simhash(std::iter::empty()), 0);
/// ```
pub fn simhash<I: IntoIterator<Item = (u64, u32)>>(features: I) -> u64 {
    let mut cols = [0i64; 64];
    for (h, w) in features {
        let w = i64::from(w);
        for (bit, c) in cols.iter_mut().enumerate() {
            if h >> bit & 1 == 1 {
                *c += w;
            } else {
                *c -= w;
            }
        }
    }
    cols.iter()
        .enumerate()
        .filter(|(_, c)| **c > 0)
        .fold(0u64, |acc, (bit, _)| acc | 1 << bit)
}

/// SimHash of hashes weighted by their multiplicity; sorts `hashes` in place.
pub fn simhash_counted(hashes: &mut [u64]) -> u64 {
    hashes.sort_unstable();
    simhash(
        hashes
            .chunk_by(|a, b| a == b)
            .map(|run| (run[0], u32::try_from(run.len()).unwrap_or(u32::MAX))),
    )
}

/// SimHash of hashes weighted `1 + floor(log2(multiplicity))`; sorts `hashes` in place.
pub fn simhash_log_counted(hashes: &mut [u64]) -> u64 {
    hashes.sort_unstable();
    simhash(hashes.chunk_by(|a, b| a == b).map(|run| {
        let n = u32::try_from(run.len()).unwrap_or(u32::MAX).max(1);
        (run[0], 1 + n.ilog2())
    }))
}

/// `sketch.simhash_text` of already-normalized text, with the number of shingles; `None`
/// without any token.
pub fn text_simhash(normalized: &str) -> Option<(u64, usize)> {
    let mut hashes = Vec::new();
    shingle_hashes(normalized, &mut hashes);
    if hashes.is_empty() {
        return None;
    }
    let n = hashes.len();
    Some((simhash_log_counted(&mut hashes), n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majority_per_bit() {
        assert_eq!(simhash([(0b11, 1), (0b01, 1), (0b00, 1)]), 0b01);
        assert_eq!(simhash([(0b10, 3), (0b01, 1)]), 0b10);
        // A tie is 0.
        assert_eq!(simhash([(0b1, 1), (0b0, 1)]), 0);
    }

    #[test]
    fn counted_weights_repeats() {
        let mut h = vec![0b01, 0b10, 0b10];
        assert_eq!(simhash_counted(&mut h), 0b10);
        // log weights: 1 + log2(1) = 1 against 1 + log2(2) = 2.
        let mut h = vec![0b01, 0b10, 0b10, 0b01];
        assert_eq!(simhash_log_counted(&mut h), 0);
        let mut h = vec![0b01, 0b10, 0b10, 0b10, 0b10];
        assert_eq!(simhash_log_counted(&mut h), 0b10);
    }
}

//! Per-file similarity sketches: compact, stable values a service compares across millions of
//! files to find near-duplicates, versions and documents cut from one template.
//!
//! Namespace: `sketch.*`. This crate only **emits** sketches and LSH keys, one file at a time;
//! indexing and matching across files belong to the service. Every sketch documents the
//! comparison it is meant for (a Hamming distance or a Jaccard estimate) and the thresholds
//! measured for it; see the README for the numbers.
//!
//! | analyzer | profile | sketch | compare with |
//! |---|---|---|---|
//! | [`MinHashAnalyzer`] `sketch.minhash@2` | Fast | 128-row MinHash of word 3-shingles + 32 LSH band keys | [`jaccard_estimate`]: near-duplicate ≥ [`minhash::NEAR_DUP_JACCARD`], version ≥ [`minhash::VERSION_JACCARD`] |
//! | [`SimHashText`] `sketch.simhash_text@2` | Fast | 64-bit SimHash of weighted word 3-shingles | [`hamming`] ≤ [`simhash::TEXT_HAMMING_MAX`] |
//! | [`SimHashTemplate`] `sketch.simhash_template@2` | Fast | 64-bit SimHash of entity-masked words | [`hamming`] ≤ [`template::TEMPLATE_HAMMING_MAX`] |
//! | [`LayoutAnalyzer`] `sketch.layout@2` | Fast | geometry SimHash (PDF), block-structure SimHash, heading skeleton | [`hamming`] ≤ [`layout::LAYOUT_HAMMING_MAX`] / [`layout::STRUCTURE_HAMMING_MAX`]; headings: equality |
//! | [`InvoiceLayout`] `sketch.invoice_layout@2` | Fast | invoice labels + number-pattern digest | equality |
//! | [`TextKey`] `sketch.text_key@2` | Fast | 128-bit key of the folded words | equality |
//! | [`CompatMinHash`] `sketch.minhash_compat@3` | Standard | byte-compatible port of an existing deployment's sketches (compatibility mode) | as in that deployment, see [`compat`] |
//! | [`PagePHash`] `sketch.page_phash@4` | Standard | 64-bit DCT pHash of up to 4 scanned pages (first 3 + last; scans decoded at reduced resolution) | [`hamming`] ≤ [`phash::PAGE_HAMMING_MAX`] per page |
//!
//! Everything builds on `quail-text`: its normalization, tokens, folding and pinned XXH3
//! hashes. Golden tests (`tests/golden.rs`) pin every sketch; a golden diff means a new
//! analyzer version.
//!
//! The pure functions ([`minhash::text_minhash`], [`simhash::text_simhash`],
//! [`template::template_simhash`], [`layout::geometry_sketch`], ...) work without the analyzers.

#[allow(clippy::unreadable_literal)]
mod coeffs;

mod analyzers;
pub mod compat;
pub mod invoice;
pub mod layout;
pub mod minhash;
pub mod phash;
pub mod simhash;
pub mod template;
mod text;

pub use analyzers::{
    CompatMinHash, InvoiceLayout, LayoutAnalyzer, MAX_PHASH_PAGES, MinHashAnalyzer, PHASH_DPI,
    PHASH_SCAN_SIDE, PagePHash, SimHashTemplate, SimHashText, TextKey, phash_pages, text_key,
};

/// Number of differing bits of two 64-bit sketches (SimHash, pHash).
///
/// ```
/// assert_eq!(quail_sketch::hamming(0b1011, 0b0010), 2);
/// ```
#[inline]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Estimated Jaccard similarity of the shingle sets behind two MinHash signatures as stored
/// in `Value::Bytes`: the share of rows that agree. Works for both layouts this crate emits
/// ([`keys::MINHASH`], 128 rows of 4 bytes, and [`keys::COMPAT_MINHASH`], 128 rows of 8 bytes).
/// Returns 0 when the signatures differ in length or are not a whole number of 128 rows.
///
/// The estimate's standard error is `sqrt(J(1-J)/128)`: ±0.04 at J = 0.6, ±0.027 at 0.9.
///
/// ```
/// use quail_sketch::{jaccard_estimate, minhash::text_minhash};
/// let a = text_minhash("one two three four five six seven eight nine ten").unwrap();
/// assert_eq!(jaccard_estimate(&a.to_bytes(), &a.to_bytes()), 1.0);
/// ```
pub fn jaccard_estimate(a: &[u8], b: &[u8]) -> f64 {
    let rows = minhash::PERMUTATIONS;
    if a.is_empty() || a.len() != b.len() || !a.len().is_multiple_of(rows) {
        return 0.0;
    }
    let lane = a.len() / rows;
    let equal = a
        .chunks_exact(lane)
        .zip(b.chunks_exact(lane))
        .filter(|(x, y)| x == y)
        .count();
    #[allow(clippy::cast_precision_loss)] // at most 128
    let share = equal as f64 / rows as f64;
    share
}

/// Every fact key this crate emits.
pub mod keys {
    use quail_core::FactKey;

    /// MinHash signature of the word 3-shingles of the normalized, folded text
    /// (`Value::Bytes`, 128 rows × 4 bytes little-endian; see [`crate::minhash`]). Compare with
    /// [`crate::jaccard_estimate`]. Absent below [`crate::minhash::MIN_SHINGLES`] distinct shingles.
    pub const MINHASH: FactKey = FactKey::new("sketch.minhash");
    /// LSH band keys of [`MINHASH`] (`Value::List` of 32 `Value::U64`, band index mixed in, so
    /// all bands can share one index). Two files sharing any key are candidates.
    pub const MINHASH_BANDS: FactKey = FactKey::new("sketch.minhash_bands");
    /// Distinct word 3-shingles behind [`MINHASH`] (`Value::Int`).
    pub const MINHASH_SHINGLES: FactKey = FactKey::new("sketch.minhash_shingles");
    /// 64-bit SimHash of the weighted word 3-shingles (`Value::U64`). Compare with
    /// [`crate::hamming`].
    pub const SIMHASH_TEXT: FactKey = FactKey::new("sketch.simhash_text");
    /// 64-bit SimHash of the entity-masked words: numbers, dates, amounts, codes, names,
    /// URLs and e-mail addresses replaced by their class (`Value::U64`).
    pub const SIMHASH_TEMPLATE: FactKey = FactKey::new("sketch.simhash_template");
    /// 64-bit SimHash of the page geometry: quantized block boxes, kinds and zones of the
    /// first pages (`Value::U64`). Only for documents with block boxes (PDF).
    pub const LAYOUT: FactKey = FactKey::new("sketch.layout");
    /// Blocks with a box that went into [`LAYOUT`] (`Value::Int`).
    pub const LAYOUT_BLOCKS: FactKey = FactKey::new("sketch.layout_blocks");
    /// 64-bit SimHash of the block-kind sequence: headings and levels, lists, tables and
    /// their shape, paragraph runs (`Value::U64`). Any format with blocks.
    pub const STRUCTURE: FactKey = FactKey::new("sketch.structure");
    /// Digest of the heading skeleton (heading texts, numbering stripped, lowercased), the
    /// `structural_fp` of [`crate::compat`] as a big-endian integer (`Value::U64`). Compare
    /// for equality. Absent with fewer than two headings.
    pub const HEADINGS: FactKey = FactKey::new("sketch.headings");
    /// Invoice layout digest: which of 16 invoice labels occur, plus the shapes of
    /// invoice-number-like codes (`Value::U64`, see [`crate::invoice`]). Compare for equality.
    pub const INVOICE_LAYOUT: FactKey = FactKey::new("sketch.invoice_layout");
    /// Invoice labels found (`Value::Int`, 1..=16).
    pub const INVOICE_LABELS: FactKey = FactKey::new("sketch.invoice_labels");
    /// Distinct invoice-number shapes found (`Value::Int`).
    pub const INVOICE_NUMBER_PATTERNS: FactKey = FactKey::new("sketch.invoice_number_patterns");
    /// XXH3-128 of the folded words joined by one space (`Value::Bytes`, 16 bytes): the same
    /// words, whatever the bytes, spacing, case, diacritics or script. Absent below 400
    /// characters.
    pub const TEXT_KEY: FactKey = FactKey::new("sketch.text_key");
    /// Python-compatible version signature (`Value::Bytes`, 128 rows × 8 bytes little-endian),
    /// see [`crate::compat`].
    pub const COMPAT_MINHASH: FactKey = FactKey::new("sketch.compat_minhash");
    /// Python-compatible LSH bucket keys of [`COMPAT_MINHASH`] (`Value::List` of 32
    /// `Value::Text`, `"<band>:<16 hex>"`).
    pub const COMPAT_MINHASH_BANDS: FactKey = FactKey::new("sketch.compat_minhash_bands");
    /// Distinct shingles behind [`COMPAT_MINHASH`] (`Value::Int`).
    pub const COMPAT_SHINGLES: FactKey = FactKey::new("sketch.compat_shingles");
    /// Python-compatible template SimHash of the entity-masked words (`Value::U64`).
    pub const COMPAT_TEMPLATE: FactKey = FactKey::new("sketch.compat_template");
    /// 64-bit DCT pHash per scanned page (`Value::List` of `Value::U64`), aligned with
    /// [`PAGE_PHASH_PAGES`].
    pub const PAGE_PHASH: FactKey = FactKey::new("sketch.page_phash");
    /// Zero-based page indices of [`PAGE_PHASH`] (`Value::List` of `Value::Int`).
    pub const PAGE_PHASH_PAGES: FactKey = FactKey::new("sketch.page_phash_pages");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hamming_counts_bits() {
        assert_eq!(hamming(0, u64::MAX), 64);
        assert_eq!(hamming(7, 7), 0);
    }

    #[test]
    fn jaccard_rejects_mismatched_signatures() {
        assert!(jaccard_estimate(&[], &[]).abs() < f64::EPSILON);
        assert!(jaccard_estimate(&[0; 512], &[0; 1024]).abs() < f64::EPSILON);
        assert!(jaccard_estimate(&[0; 100], &[0; 100]).abs() < f64::EPSILON);
        assert!((jaccard_estimate(&[1; 1024], &[1; 1024]) - 1.0).abs() < f64::EPSILON);
        let mut b = vec![0u8; 512];
        b[0] = 1; // one row differs
        assert!((jaccard_estimate(&[0; 512], &b) - 127.0 / 128.0).abs() < 1e-9);
    }
}

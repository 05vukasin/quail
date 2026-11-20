use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How much work to spend per file. Ordered: `Fast < Standard < Deep`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Rules and statistics only, about a millisecond per file.
    Fast,
    /// Plus static embeddings and small linear models. Pure Rust. The default.
    #[default]
    Standard,
    /// Plus neural models (OCR, NER) through ONNX, run where cheaper layers are unsure.
    Deep,
}

/// Hard per-file resource limits. Exceeding one yields a partial record, never a crash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Largest file read at all; bigger files get only size and a streaming hash.
    pub max_bytes: u64,
    /// Pages extracted per document; later pages are skipped and flagged.
    pub max_pages: u32,
    /// Bytes any decompression may produce for one file (zip bombs, flate bombs).
    pub max_decompressed: u64,
    /// Bytes of text kept per document.
    pub max_text_bytes: u64,
    /// Pixels of one rendered page (width × height) for OCR.
    pub max_render_pixels: u64,
    /// Wall time for the whole file.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 512 << 20,
            max_pages: 2_000,
            max_decompressed: 1 << 30,
            max_text_bytes: 64 << 20,
            max_render_pixels: 40_000_000,
            timeout: Duration::from_mins(2),
        }
    }
}

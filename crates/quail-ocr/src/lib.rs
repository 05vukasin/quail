//! OCR for pages without a usable text layer. Namespace `ocr.*`.
//!
//! One analyzer, [`OcrPages`] (`ocr.pages@1`, profile `Deep`, requires `document` and
//! `raster`, provides [`OcrText`] as `"ocr_text"`). It reads only core types: the pages of the
//! [`Document`](quail_core::Document) that have `needs_ocr`, and their pixels from the
//! [`Raster`](quail_core::Raster) artifact (the page's embedded scan when
//! [`Page::is_scan`](quail_core::Page::is_scan), else a render at 200 DPI, again at 300 DPI
//! when the first reading is weak).
//!
//! # Pieces
//!
//! - [`OcrEngine`]: an image in, lines and words with pixel boxes and confidences out.
//!   Engines are `&mut` (an ONNX Runtime session runs through `&mut self`), so
//!   [`EngineFactory`] makes one per worker and [`EnginePool`] hands them out.
//! - [`OcrCache`] / [`MemoryCache`]: results keyed by BLAKE3 of the pixels and the engine
//!   fingerprint, so a scan seen twice is read once.
//! - [`OcrText`]: the result laid out like core pages (text in reading order, paragraph
//!   blocks with boxes in points, lines and words with confidences).
//!   [`OcrText::apply_to`] gives the document with OCR text filled into the pages that had
//!   none, for analyzers that read text.
//! - [`models`]: the manifest of model files (URL, version, size, SHA-256, license) and the
//!   model directory. Models are never part of the repository.
//!
//! # Backends
//!
//! - `paddle` (feature, off by default): PP-OCRv6 small detection and recognition through
//!   `oar-ocr` on ONNX Runtime, PP-LCNet page and line orientation, and the Cyrillic PP-OCRv5
//!   recognizer for Serbian Cyrillic, routed per page and per line (see [`paddle`]).
//! - Without a backend the crate still builds (pure Rust) and the analyzer reports the
//!   problem `ocr backend not compiled` for files that have a page needing OCR, and nothing
//!   for the others.
//!
//! # Facts
//!
//! | key | value | what |
//! |---|---|---|
//! | [`keys::PAGES`] | Int | pages OCR'd |
//! | [`keys::CHARS`] | Int | characters read (spaces excluded) |
//! | [`keys::MEAN_CONF`] | Float | mean recognition confidence, weighted by characters |
//! | [`keys::SCRIPT`] | Text | dominant script of the OCR'd text: `latin`, `cyrillic`, `mixed`, `other` |
//! | [`keys::PAGE`] | Record | one per OCR'd page: `page`, `script`, `mean_conf`, `chars`, `lines`, `dpi`, `source` (`embedded_scan`/`render`), `rotation`; `conf` = the page's mean confidence |

pub mod analyzer;
pub mod cache;
pub mod engine;
pub mod models;
pub mod pool;
pub mod preprocess;
pub mod script;
pub mod text;

#[cfg(feature = "paddle")]
pub mod paddle;

pub use analyzer::{OcrConfig, OcrPages, pages_needing_ocr};
pub use cache::{CacheKey, MemoryCache, OcrCache};
pub use engine::{
    EngineFactory, EngineId, OcrEngine, OcrHints, OcrLine, OcrPage, OcrWord, PxBox, ScriptHint,
};
pub use pool::EnginePool;
pub use script::{Script, ScriptCounts, script_of};
pub use text::{OcrPageText, OcrText, OcrTextLine, OcrTextWord, PixelSource, PxToPt};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// Pages OCR'd (Int).
    pub const PAGES: FactKey = FactKey::new("ocr.pages");
    /// Characters read, spaces excluded (Int).
    pub const CHARS: FactKey = FactKey::new("ocr.chars");
    /// Mean recognition confidence weighted by characters (Float).
    pub const MEAN_CONF: FactKey = FactKey::new("ocr.mean_conf");
    /// Dominant script of the OCR'd text (Text).
    pub const SCRIPT: FactKey = FactKey::new("ocr.script");
    /// One OCR'd page (Record: `page`, `script`, `mean_conf`, `chars`, `lines`, `dpi`,
    /// `source`, `rotation`).
    pub const PAGE: FactKey = FactKey::new("ocr.page");
}

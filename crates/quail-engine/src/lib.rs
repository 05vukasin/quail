//! The engine: sniffs the format, picks the extractor, orders analyzers, enforces limits,
//! catches panics and runs many files in parallel with bounded memory.
//!
//! Namespace: `engine.*`. It knows no concrete extractor or analyzer: the caller registers
//! them on an [`EngineBuilder`].
//!
//! ```
//! use quail_core::{Blob, SourceItem};
//! use quail_engine::EngineBuilder;
//!
//! let engine = EngineBuilder::new().build().unwrap();
//! let rec = engine.process(SourceItem {
//!     locator: "a.txt".into(),
//!     blob: Blob::from(b"hello".to_vec()),
//!     modified: None,
//! });
//! assert_eq!(rec.format, quail_core::Format::Text);
//! ```
//!
//! # Deadlines are cooperative
//!
//! Each file gets `now + Limits::timeout` as its deadline, passed to every step through
//! [`quail_core::Emitter::with_deadline`]. The engine checks it between steps and steps check
//! it in their loops. Nothing can preempt code that does not check: a native library that
//! hangs inside one call keeps its worker thread busy until it returns. Killing a stuck
//! worker needs process isolation, which is the job of the service around the library.

mod builder;
mod engine;
mod panic;
mod run;
pub mod sniff;

pub use builder::{BUILTIN_ARTIFACTS, BuildError, EngineBuilder};
pub use engine::{Engine, SNIFF_ID};
pub use panic::install_quiet_panic_hook;
pub use run::{RunOptions, RunStats};
pub use sniff::{Sniff, detect, sniff};

/// Fact keys emitted by the engine.
pub mod keys {
    use quail_core::FactKey;

    /// Detected format, as [`quail_core::Format::as_str`] (`pdf`, `docx`, `zip`, `image`,
    /// `text`, `unknown`).
    pub const FORMAT: FactKey = FactKey::new("engine.format");
    /// Most specific MIME type known from the bytes, e.g. `application/pdf`.
    pub const MIME: FactKey = FactKey::new("engine.mime");
}

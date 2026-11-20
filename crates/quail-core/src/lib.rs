//! Shared contract of quail.
//!
//! Every other crate depends on this one and on nothing of each other's, except
//! `quail-engine`, which wires them together. The pieces:
//!
//! - [`Blob`]: the bytes of one file, owned or memory-mapped.
//! - [`Format`]: what kind of file it is.
//! - [`Document`]: what an [`Extractor`] makes of a file: pages, text in reading order and
//!   blocks that point into that text, with zones and boxes.
//! - [`Fact`]: one piece of metadata with its confidence, its source and its evidence.
//! - [`Analyzer`]: turns what is known about a file into more facts and artifacts.
//! - [`Artifacts`]: typed, read-only intermediate results shared between analyzers.
//! - [`Source`] / [`RecordSink`]: where files come from and where [`FileRecord`]s go.
//!
//! Rules every implementation follows:
//!
//! 1. One file at a time, no shared mutable state: an analyzer sees only the file at hand.
//!    Decisions across files belong to the service, not to the library.
//! 2. Report candidates, not verdicts: a fact carries `conf`, `src` and evidence.
//! 3. A crate emits facts only under its own key namespace (see [`FactKey`]).
//! 4. Hostile input never panics on purpose and never exceeds [`Limits`].

mod analyzer;
mod artifact;
mod blob;
mod document;
mod error;
mod extractor;
mod fact;
mod format;
mod limits;
mod raster;
mod record;
mod source;
mod value;

pub use analyzer::{Analyzer, AnalyzerId, Ctx, Emitter};
pub use artifact::{Artifact, Artifacts};
pub use blob::Blob;
pub use document::{Block, BlockKind, Document, Page, PageImage, PageKind, Span, TextSource};
pub use error::{Error, Result};
pub use extractor::Extractor;
pub use fact::{BBox, Evidence, Fact, FactKey, Zone};
pub use format::Format;
pub use limits::{Limits, Profile};
pub use raster::{Image, PageRaster, Raster};
pub use record::{FileRecord, Status, Timing};
pub use source::{RecordSink, Source, SourceItem};
pub use value::{Date, Money, Value};

/// Version of the [`FileRecord`] schema. Bump on any breaking change to the output.
pub const SCHEMA_VERSION: u32 = 1;

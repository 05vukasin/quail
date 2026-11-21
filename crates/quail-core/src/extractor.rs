use crate::analyzer::{AnalyzerId, Emitter};
use crate::blob::Blob;
use crate::document::Document;
use crate::error::Result;
use crate::format::Format;
use crate::limits::Limits;

/// Turns the bytes of one format into a [`Document`]. File-level properties (producer,
/// author, signatures, revision ids...) go out as facts through `out`, under the extractor's
/// own namespace (`pdf.*`, `docx.*`).
///
/// The engine picks the extractor by [`Format`], runs it under `catch_unwind` and a
/// timeout, and stores the returned document as an artifact for the analyzers.
pub trait Extractor: Send + Sync {
    /// Name and output version.
    fn id(&self) -> AnalyzerId;

    /// Formats it handles.
    fn formats(&self) -> &'static [Format];

    /// Artifact names it may store besides the `document` (e.g. `"raster"`). Analyzers may
    /// require them; they are absent for files of other formats or when extraction fails, in
    /// which case such analyzers are skipped like for any other missing artifact.
    fn provides(&self) -> &'static [&'static str] {
        &[]
    }

    /// Extracts the document. Must respect `limits` (stop at `max_pages`, refuse to inflate
    /// past `max_decompressed`) and prefer a partial document plus warnings over an error.
    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document>;
}

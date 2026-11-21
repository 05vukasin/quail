//! PDF extractor for quail.
//!
//! [`PdfExtractor`] (`pdf.extract@2`) turns a PDF into a [`Document`](quail_core::Document):
//! one physical [`Page`](quail_core::Page) per page with its size, its text in reading order
//! and paragraph [`Block`](quail_core::Block)s carrying boxes (points, top-left origin) and
//! zones (header/footer by geometry and repetition, body otherwise), and the images it draws.
//! Each page says whether its text layer is usable or the page should be OCR'd. File-level
//! properties go out as `pdf.*` facts, declared in [`keys`].
//!
//! The extractor also provides a [`Raster`](quail_core::Raster) artifact backed by
//! [`PdfRaster`]: OCR renders pages, or takes a page's embedded scan, through the core
//! [`PageRaster`](quail_core::PageRaster) trait without depending on this crate.
//!
//! The stack is pure Rust: `hayro-syntax` parses (xref repair, object streams, RC4/AES with the
//! empty user password) and `hayro-interpret` runs content streams into a text-collecting
//! device. The same interpreter renders pages (crate `hayro`) when OCR needs pixels.
//!
//! Hostile input: every limit in [`Limits`](quail_core::Limits) is honoured, third-party calls
//! run under `catch_unwind` per document and per page, and problems become warnings with a
//! partial document whenever something could be read.

mod device;
mod extract;
mod layout;
mod meta;
mod open;
mod quality;
mod raster;
mod raw;
mod zones;

pub use extract::PdfExtractor;
pub use raster::PdfRaster;

/// Keys of the facts this crate emits. All live under `pdf.*`.
pub mod keys {
    use quail_core::FactKey;

    /// PDF version, e.g. `"1.7"` (catalog `Version` wins over the header). Text.
    pub const VERSION: FactKey = FactKey::new("pdf.version");
    /// Number of pages in the document (not only the extracted ones). Int.
    pub const PAGE_COUNT: FactKey = FactKey::new("pdf.page_count");
    /// The file has an `/Encrypt` dictionary (owner- or user-password protected). Bool.
    pub const ENCRYPTED: FactKey = FactKey::new("pdf.encrypted");
    /// Info `Producer`. Text.
    pub const PRODUCER: FactKey = FactKey::new("pdf.producer");
    /// Info `Creator`. Text.
    pub const CREATOR: FactKey = FactKey::new("pdf.creator");
    /// Info `Author`. Text.
    pub const AUTHOR: FactKey = FactKey::new("pdf.author");
    /// Info `Title`. Text.
    pub const TITLE: FactKey = FactKey::new("pdf.title");
    /// Info `Subject`. Text.
    pub const SUBJECT: FactKey = FactKey::new("pdf.subject");
    /// Info `Keywords`. Text.
    pub const KEYWORDS: FactKey = FactKey::new("pdf.keywords");
    /// Info `CreationDate`. Date.
    pub const CREATED: FactKey = FactKey::new("pdf.created");
    /// Info `ModDate`. Date.
    pub const MODIFIED: FactKey = FactKey::new("pdf.modified");
    /// UTC offset of `CreationDate`, e.g. `+02:00`, when it has a non-zero one. Text.
    pub const CREATED_UTC_OFFSET: FactKey = FactKey::new("pdf.created_utc_offset");
    /// UTC offset of `ModDate`, e.g. `-05:00`, when it has a non-zero one. Text.
    pub const MODIFIED_UTC_OFFSET: FactKey = FactKey::new("pdf.modified_utc_offset");
    /// Incremental updates appended after the first revision (`%%EOF` sections minus one,
    /// minus the extra section of a linearized file). Int.
    pub const INCREMENTAL_UPDATES: FactKey = FactKey::new("pdf.incremental_updates");
    /// The file is linearized ("fast web view"). Bool.
    pub const LINEARIZED: FactKey = FactKey::new("pdf.linearized");
    /// The catalog has an XMP metadata stream. Bool.
    pub const HAS_XMP: FactKey = FactKey::new("pdf.has_xmp");
    /// Signature form fields. Int.
    pub const SIGNATURE_FIELDS: FactKey = FactKey::new("pdf.signature_fields");
    /// At least one signature field carries a signature value. Bool.
    pub const SIGNED: FactKey = FactKey::new("pdf.signed");
    /// Terminal `AcroForm` fields (signatures included). Int.
    pub const FORM_FIELDS: FactKey = FactKey::new("pdf.form_fields");
    /// The document contains JavaScript actions. Bool.
    pub const HAS_JAVASCRIPT: FactKey = FactKey::new("pdf.has_javascript");
    /// Files in the `EmbeddedFiles` name tree. Int.
    pub const EMBEDDED_FILES: FactKey = FactKey::new("pdf.embedded_files");
    /// Font names used by the extracted pages, subset tag stripped, deduplicated, sorted.
    /// List of Text.
    pub const FONTS: FactKey = FactKey::new("pdf.fonts");
    /// Extracted pages that are an image with (almost) no text. Int.
    pub const IMAGE_ONLY_PAGES: FactKey = FactKey::new("pdf.image_only_pages");
    /// Extracted pages flagged `needs_ocr`. Int.
    pub const PAGES_NEEDING_OCR: FactKey = FactKey::new("pdf.pages_needing_ocr");
    /// Extracted pages that are scans needing OCR: image-only, or mostly an image whose text
    /// layer is too thin or garbage (a bad OCR layer, e.g. Cyrillic read as Latin). A scan
    /// with a usable OCR layer is not counted. Int.
    pub const SCANNED_PAGES: FactKey = FactKey::new("pdf.scanned_pages");
    /// More than half of the extracted pages are [`SCANNED_PAGES`]: the document is a scan
    /// that needs OCR. Emitted for every PDF with an extracted page. Bool.
    pub const IS_SCANNED: FactKey = FactKey::new("pdf.is_scanned");
}

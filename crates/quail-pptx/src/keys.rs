//! Every fact key the PPTX extractor emits. All live under `pptx.*`.

use quail_core::FactKey;

// docProps/core.xml and docProps/app.xml
/// `dc:title`. Text.
pub const TITLE: FactKey = FactKey::new("pptx.title");
/// `dc:subject`. Text.
pub const SUBJECT: FactKey = FactKey::new("pptx.subject");
/// `dc:creator`. Text.
pub const CREATOR: FactKey = FactKey::new("pptx.creator");
/// `cp:lastModifiedBy`. Text.
pub const LAST_MODIFIED_BY: FactKey = FactKey::new("pptx.last_modified_by");
/// `cp:keywords`. Text.
pub const KEYWORDS: FactKey = FactKey::new("pptx.keywords");
/// `dcterms:created`: the calendar date of the W3CDTF value. Date; an unparsable value is
/// reported in a warning instead.
pub const CREATED: FactKey = FactKey::new("pptx.created");
/// `dcterms:modified`, like [`CREATED`]. Date.
pub const MODIFIED: FactKey = FactKey::new("pptx.modified");
/// `Application`, e.g. `Microsoft Office PowerPoint`. Text.
pub const APPLICATION: FactKey = FactKey::new("pptx.application");
/// `Company`. Text.
pub const COMPANY: FactKey = FactKey::new("pptx.company");

// structure
/// Slides listed in `presentation.xml` (`p:sldIdLst`), read or not. Int.
pub const SLIDES: FactKey = FactKey::new("pptx.slides");
/// Slides marked hidden (`show="0"`); their text is still read. Int.
pub const HIDDEN_SLIDES: FactKey = FactKey::new("pptx.hidden_slides");
/// Slides whose speaker notes have text. Int.
pub const NOTES_SLIDES: FactKey = FactKey::new("pptx.notes_slides");
/// Tables (`a:tbl`) emitted as table cells. Int.
pub const TABLES: FactKey = FactKey::new("pptx.tables");
/// Shapes and groups with text left out because they are hidden (`cNvPr hidden="1"`). Int.
pub const HIDDEN_SHAPES: FactKey = FactKey::new("pptx.hidden_shapes");

// package
/// A `vbaProject.bin` part is present (`.pptm`). Bool.
pub const HAS_MACROS: FactKey = FactKey::new("pptx.has_macros");
/// Parts under `ppt/media/`. Int.
pub const IMAGES: FactKey = FactKey::new("pptx.images");
/// Parts under `ppt/embeddings/`. Int.
pub const EMBEDDED_OBJECTS: FactKey = FactKey::new("pptx.embedded_objects");
/// Entries in the zip directory. Int.
pub const ZIP_ENTRIES: FactKey = FactKey::new("pptx.zip_entries");
/// Entries whose name repeats an earlier one, exactly or by case. Int.
pub const DUPLICATE_ENTRIES: FactKey = FactKey::new("pptx.duplicate_entries");

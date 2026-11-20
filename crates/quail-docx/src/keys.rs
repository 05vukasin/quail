//! Every fact key the DOCX extractor emits. All live under `docx.*`.

use quail_core::FactKey;

// docProps/core.xml
/// `dc:title`. Text.
pub const TITLE: FactKey = FactKey::new("docx.title");
/// `dc:subject`. Text.
pub const SUBJECT: FactKey = FactKey::new("docx.subject");
/// `dc:creator`. Text.
pub const CREATOR: FactKey = FactKey::new("docx.creator");
/// `cp:lastModifiedBy`. Text.
pub const LAST_MODIFIED_BY: FactKey = FactKey::new("docx.last_modified_by");
/// `cp:revision`, the save counter. Int when numeric, else Text.
pub const REVISION: FactKey = FactKey::new("docx.revision");
/// `dcterms:created`: the calendar date of the W3CDTF value (`2024-03-01T09:30:00Z` gives
/// 2024-03-01). Date; an unparsable value is reported in a warning instead.
pub const CREATED: FactKey = FactKey::new("docx.created");
/// `dcterms:modified`, like [`CREATED`]. Date.
pub const MODIFIED: FactKey = FactKey::new("docx.modified");
/// `cp:lastPrinted`, like [`CREATED`]. Date.
pub const LAST_PRINTED: FactKey = FactKey::new("docx.last_printed");
/// `cp:keywords`. Text.
pub const KEYWORDS: FactKey = FactKey::new("docx.keywords");
/// `cp:category`. Text.
pub const CATEGORY: FactKey = FactKey::new("docx.category");
/// `dc:description` (the "comments" property). Text.
pub const DESCRIPTION: FactKey = FactKey::new("docx.description");

// docProps/app.xml
/// `Application`, e.g. `Microsoft Office Word`. Text.
pub const APPLICATION: FactKey = FactKey::new("docx.application");
/// `AppVersion`, e.g. `16.0000`. Text.
pub const APP_VERSION: FactKey = FactKey::new("docx.app_version");
/// `Company`. Text.
pub const COMPANY: FactKey = FactKey::new("docx.company");
/// `Manager`. Text.
pub const MANAGER: FactKey = FactKey::new("docx.manager");
/// `Template` (file name of the template the document was made from). Text.
pub const TEMPLATE: FactKey = FactKey::new("docx.template");
/// `TotalTime`, editing minutes. Int.
pub const TOTAL_TIME: FactKey = FactKey::new("docx.total_time");
/// `Pages` as last computed by the producing application. Int.
pub const PAGES: FactKey = FactKey::new("docx.pages");
/// `Words` as last computed by the producing application. Int.
pub const WORDS: FactKey = FactKey::new("docx.words");
/// `Characters` as last computed by the producing application. Int.
pub const CHARACTERS: FactKey = FactKey::new("docx.characters");
/// `Paragraphs` as last computed by the producing application. Int.
pub const APP_PARAGRAPHS: FactKey = FactKey::new("docx.app_paragraphs");

// docProps/custom.xml
/// Number of custom properties. Int.
pub const CUSTOM_PROPERTY_COUNT: FactKey = FactKey::new("docx.custom_property_count");
/// Custom property names, in file order. List of Text.
pub const CUSTOM_PROPERTY_NAMES: FactKey = FactKey::new("docx.custom_property_names");
/// Custom properties as `name=value` text, in file order. List of Text.
pub const CUSTOM_PROPERTIES: FactKey = FactKey::new("docx.custom_properties");

// word/settings.xml
/// Every `w:rsid` of `w:rsids`, in file order: the editing-session ids of the document's
/// history. Documents that share session ids share an ancestor. List of U64.
pub const RSIDS: FactKey = FactKey::new("docx.rsids");
/// `w:rsidRoot`: the session that created the document. U64.
pub const RSID_ROOT: FactKey = FactKey::new("docx.rsid_root");
/// Number of rsids. Int.
pub const RSID_COUNT: FactKey = FactKey::new("docx.rsid_count");
/// Target of `w:attachedTemplate` (often a path on the author's machine). Text.
pub const ATTACHED_TEMPLATE: FactKey = FactKey::new("docx.attached_template");
/// `w:trackRevisions` is on. Bool.
pub const TRACK_REVISIONS: FactKey = FactKey::new("docx.track_revisions");
/// `w:documentProtection` edit mode when enforced (`readOnly`, `comments`, ...). Text.
pub const PROTECTION: FactKey = FactKey::new("docx.protection");

// Revisions and comments
/// Tracked insertions (`w:ins`, `w:moveTo`) in all text parts. Int.
pub const TRACKED_INSERTIONS: FactKey = FactKey::new("docx.tracked_insertions");
/// Tracked deletions (`w:del`, `w:moveFrom`) in all text parts. Int.
pub const TRACKED_DELETIONS: FactKey = FactKey::new("docx.tracked_deletions");
/// Tracked formatting changes (`w:rPrChange`, `w:pPrChange`, ...). Int.
pub const TRACKED_FORMAT_CHANGES: FactKey = FactKey::new("docx.tracked_format_changes");
/// Distinct authors of tracked changes, first spelling kept. List of Text.
pub const REVISION_AUTHORS: FactKey = FactKey::new("docx.revision_authors");
/// Characters of tracked deletions (left out of the text). Int.
pub const DELETED_TEXT_CHARS: FactKey = FactKey::new("docx.deleted_text_chars");
/// Characters of hidden runs (`w:vanish` set on the run; left out of the text). Int.
pub const HIDDEN_TEXT_CHARS: FactKey = FactKey::new("docx.hidden_text_chars");
/// Number of comments. Int.
pub const COMMENTS: FactKey = FactKey::new("docx.comments");
/// Distinct comment authors, first spelling kept. List of Text.
pub const COMMENT_AUTHORS: FactKey = FactKey::new("docx.comment_authors");

// Package
/// A VBA project is present (`vbaProject.bin` or a macro-enabled main part). Bool.
pub const HAS_MACROS: FactKey = FactKey::new("docx.has_macros");
/// Content type of the main part (tells `.docx`, `.docm`, `.dotx`, `.dotm` apart). Text.
pub const MAIN_CONTENT_TYPE: FactKey = FactKey::new("docx.main_content_type");
/// Embedded objects (parts under `word/embeddings/`). Int.
pub const EMBEDDED_OBJECTS: FactKey = FactKey::new("docx.embedded_objects");
/// Image parts (under `word/media/`). Int.
pub const IMAGES: FactKey = FactKey::new("docx.images");
/// Distinct lowercase host names of external hyperlinks (and `mailto:` domains), in first
/// seen order. List of Text.
pub const HYPERLINK_DOMAINS: FactKey = FactKey::new("docx.hyperlink_domains");
/// Zip entries whose name repeats an earlier entry's, exactly or in another case. Of exact
/// repeats the last entry is read, of case variants the first. Int.
pub const DUPLICATE_ENTRIES: FactKey = FactKey::new("docx.duplicate_entries");
/// Number of zip entries. Int.
pub const ZIP_ENTRIES: FactKey = FactKey::new("docx.zip_entries");

// Structure
/// Sections (`w:sectPr`), i.e. logical pages. Int.
pub const SECTIONS: FactKey = FactKey::new("docx.sections");
/// Body paragraphs, empty ones included. Int.
pub const PARAGRAPHS: FactKey = FactKey::new("docx.paragraphs");
/// Body tables, nested ones included. Int.
pub const TABLES: FactKey = FactKey::new("docx.tables");
/// Tables nested inside a table cell; their text is flattened into the outer cell. Int.
pub const NESTED_TABLES: FactKey = FactKey::new("docx.nested_tables");
/// Footnotes (separators excluded). Int.
pub const FOOTNOTES: FactKey = FactKey::new("docx.footnotes");
/// Endnotes (separators excluded). Int.
pub const ENDNOTES: FactKey = FactKey::new("docx.endnotes");
/// Distinct header parts referenced by sections. Int.
pub const HEADERS: FactKey = FactKey::new("docx.headers");
/// Distinct footer parts referenced by sections. Int.
pub const FOOTERS: FactKey = FactKey::new("docx.footers");

// CLM placeholder form (customXml `contractplaceholders`)
/// Filled legal-entity name of the CLM form's First Party (usually the client). Text.
pub const CLM_FIRST_PARTY: FactKey = FactKey::new("docx.clm_first_party");
/// Filled legal-entity name of the CLM form's Second Party (usually the supplier). Text.
pub const CLM_SECOND_PARTY: FactKey = FactKey::new("docx.clm_second_party");

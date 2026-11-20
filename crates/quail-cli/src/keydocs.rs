//! One-line docs of every fact key, per crate, for `quail keys`.
//!
//! Generated from the doc comments of each crate's `keys` module (the doc block of each
//! `pub const`). The test `every_declared_key_is_documented` fails when a crate declares a key
//! that is missing here; entries name the constants, so a renamed or removed key fails to
//! compile.

/// A fact key and its documentation.
#[derive(Clone, Debug)]
pub struct KeyDoc {
    /// The key.
    pub key: quail_core::FactKey,
    /// What it means and its value type.
    pub doc: &'static str,
}

/// Keys of `quail_engine::keys`.
pub const ENGINE: &[KeyDoc] = &[
    KeyDoc {
        key: quail_engine::keys::FORMAT,
        doc: "Detected format, as `quail_core::Format::as_str` (`pdf`, `docx`, `zip`, `image`, `text`, `unknown`).",
    },
    KeyDoc {
        key: quail_engine::keys::MIME,
        doc: "Most specific MIME type known from the bytes, e.g. `application/pdf`.",
    },
];

/// Keys of `quail_hash::keys`.
pub const HASH: &[KeyDoc] = &[
    KeyDoc {
        key: quail_hash::keys::BLAKE3,
        doc: "BLAKE3 digest of the whole file (`Value::Bytes`, 32 bytes).",
    },
    KeyDoc {
        key: quail_hash::keys::XXH3_128,
        doc: "XXH3-128 of the whole file (`Value::Bytes`, 16 bytes, canonical big-endian). A fast lookup key, not collision-resistant against an adversary: use `BLAKE3` for that.",
    },
    KeyDoc {
        key: quail_hash::keys::CDC,
        doc: "Content-defined chunk hashes in file order (`Value::List` of `Value::U64`, each the XXH3-64 of one chunk). Two files sharing chunk hashes share those bytes.",
    },
    KeyDoc {
        key: quail_hash::keys::CDC_COUNT,
        doc: "Number of content-defined chunks (`Value::Int`).",
    },
];

/// Keys of `quail_docx::keys`.
#[cfg(feature = "docx")]
pub const DOCX: &[KeyDoc] = &[
    KeyDoc {
        key: quail_docx::keys::TITLE,
        doc: "`dc:title`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::SUBJECT,
        doc: "`dc:subject`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::CREATOR,
        doc: "`dc:creator`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::LAST_MODIFIED_BY,
        doc: "`cp:lastModifiedBy`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::REVISION,
        doc: "`cp:revision`, the save counter. Int when numeric, else Text.",
    },
    KeyDoc {
        key: quail_docx::keys::CREATED,
        doc: "`dcterms:created`: the calendar date of the W3CDTF value (`2024-03-01T09:30:00Z` gives 2024-03-01). Date; an unparsable value is reported in a warning instead.",
    },
    KeyDoc {
        key: quail_docx::keys::MODIFIED,
        doc: "`dcterms:modified`, like `CREATED`. Date.",
    },
    KeyDoc {
        key: quail_docx::keys::LAST_PRINTED,
        doc: "`cp:lastPrinted`, like `CREATED`. Date.",
    },
    KeyDoc {
        key: quail_docx::keys::KEYWORDS,
        doc: "`cp:keywords`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::CATEGORY,
        doc: "`cp:category`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::DESCRIPTION,
        doc: "`dc:description` (the \"comments\" property). Text.",
    },
    KeyDoc {
        key: quail_docx::keys::APPLICATION,
        doc: "`Application`, e.g. `Microsoft Office Word`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::APP_VERSION,
        doc: "`AppVersion`, e.g. `16.0000`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::COMPANY,
        doc: "`Company`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::MANAGER,
        doc: "`Manager`. Text.",
    },
    KeyDoc {
        key: quail_docx::keys::TEMPLATE,
        doc: "`Template` (file name of the template the document was made from). Text.",
    },
    KeyDoc {
        key: quail_docx::keys::TOTAL_TIME,
        doc: "`TotalTime`, editing minutes. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::PAGES,
        doc: "`Pages` as last computed by the producing application. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::WORDS,
        doc: "`Words` as last computed by the producing application. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::CHARACTERS,
        doc: "`Characters` as last computed by the producing application. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::APP_PARAGRAPHS,
        doc: "`Paragraphs` as last computed by the producing application. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::CUSTOM_PROPERTY_COUNT,
        doc: "Number of custom properties. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::CUSTOM_PROPERTY_NAMES,
        doc: "Custom property names, in file order. List of Text.",
    },
    KeyDoc {
        key: quail_docx::keys::CUSTOM_PROPERTIES,
        doc: "Custom properties as `name=value` text, in file order. List of Text.",
    },
    KeyDoc {
        key: quail_docx::keys::RSIDS,
        doc: "Every `w:rsid` of `w:rsids`, in file order: the editing-session ids of the document's history. Documents that share session ids share an ancestor. List of U64.",
    },
    KeyDoc {
        key: quail_docx::keys::RSID_ROOT,
        doc: "`w:rsidRoot`: the session that created the document. U64.",
    },
    KeyDoc {
        key: quail_docx::keys::RSID_COUNT,
        doc: "Number of rsids. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::ATTACHED_TEMPLATE,
        doc: "Target of `w:attachedTemplate` (often a path on the author's machine). Text.",
    },
    KeyDoc {
        key: quail_docx::keys::TRACK_REVISIONS,
        doc: "`w:trackRevisions` is on. Bool.",
    },
    KeyDoc {
        key: quail_docx::keys::PROTECTION,
        doc: "`w:documentProtection` edit mode when enforced (`readOnly`, `comments`, ...). Text.",
    },
    KeyDoc {
        key: quail_docx::keys::TRACKED_INSERTIONS,
        doc: "Tracked insertions (`w:ins`, `w:moveTo`) in all text parts. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::TRACKED_DELETIONS,
        doc: "Tracked deletions (`w:del`, `w:moveFrom`) in all text parts. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::TRACKED_FORMAT_CHANGES,
        doc: "Tracked formatting changes (`w:rPrChange`, `w:pPrChange`, ...). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::REVISION_AUTHORS,
        doc: "Distinct authors of tracked changes, first spelling kept. List of Text.",
    },
    KeyDoc {
        key: quail_docx::keys::DELETED_TEXT_CHARS,
        doc: "Characters of tracked deletions (left out of the text). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::HIDDEN_TEXT_CHARS,
        doc: "Characters of hidden runs (`w:vanish` set on the run; left out of the text). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::COMMENTS,
        doc: "Number of comments. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::COMMENT_AUTHORS,
        doc: "Distinct comment authors, first spelling kept. List of Text.",
    },
    KeyDoc {
        key: quail_docx::keys::HAS_MACROS,
        doc: "A VBA project is present (`vbaProject.bin` or a macro-enabled main part). Bool.",
    },
    KeyDoc {
        key: quail_docx::keys::MAIN_CONTENT_TYPE,
        doc: "Content type of the main part (tells `.docx`, `.docm`, `.dotx`, `.dotm` apart). Text.",
    },
    KeyDoc {
        key: quail_docx::keys::EMBEDDED_OBJECTS,
        doc: "Embedded objects (parts under `word/embeddings/`). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::IMAGES,
        doc: "Image parts (under `word/media/`). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::HYPERLINK_DOMAINS,
        doc: "Distinct lowercase host names of external hyperlinks (and `mailto:` domains), in first seen order. List of Text.",
    },
    KeyDoc {
        key: quail_docx::keys::DUPLICATE_ENTRIES,
        doc: "Zip entries whose name repeats an earlier entry's, exactly or in another case. Of exact repeats the last entry is read, of case variants the first. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::ZIP_ENTRIES,
        doc: "Number of zip entries. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::SECTIONS,
        doc: "Sections (`w:sectPr`), i.e. logical pages. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::PARAGRAPHS,
        doc: "Body paragraphs, empty ones included. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::TABLES,
        doc: "Body tables, nested ones included. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::NESTED_TABLES,
        doc: "Tables nested inside a table cell; their text is flattened into the outer cell. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::FOOTNOTES,
        doc: "Footnotes (separators excluded). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::ENDNOTES,
        doc: "Endnotes (separators excluded). Int.",
    },
    KeyDoc {
        key: quail_docx::keys::HEADERS,
        doc: "Distinct header parts referenced by sections. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::FOOTERS,
        doc: "Distinct footer parts referenced by sections. Int.",
    },
    KeyDoc {
        key: quail_docx::keys::CLM_FIRST_PARTY,
        doc: "Filled legal-entity name of the CLM form's First Party (usually the client). Text.",
    },
    KeyDoc {
        key: quail_docx::keys::CLM_SECOND_PARTY,
        doc: "Filled legal-entity name of the CLM form's Second Party (usually the supplier). Text.",
    },
];

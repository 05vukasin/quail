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
        doc: "Detected format, as `quail_core::Format::as_str` (`pdf`, `docx`, `pptx`, `html`, `zip`, `image`, `text`, `unknown`).",
    },
    KeyDoc {
        key: quail_engine::keys::MIME,
        doc: "Most specific MIME type known from the bytes, e.g. `application/pdf`.",
    },
    KeyDoc {
        key: quail_engine::keys::DUPLICATE_OF,
        doc: "On a record reused from an earlier file of the same run with the same bytes (`RunOptions::dedup_by_content`): the locator of that first file. Its record was not re-analyzed: the other facts are the first file's, and its only timing is the `engine.dedup` step.",
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

/// Keys of `quail_pptx::keys`.
#[cfg(feature = "pptx")]
pub const PPTX: &[KeyDoc] = &[
    KeyDoc {
        key: quail_pptx::keys::TITLE,
        doc: "`dc:title`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::SUBJECT,
        doc: "`dc:subject`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::CREATOR,
        doc: "`dc:creator`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::LAST_MODIFIED_BY,
        doc: "`cp:lastModifiedBy`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::KEYWORDS,
        doc: "`cp:keywords`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::CREATED,
        doc: "`dcterms:created`: the calendar date of the W3CDTF value. Date; an unparsable value is reported in a warning instead.",
    },
    KeyDoc {
        key: quail_pptx::keys::MODIFIED,
        doc: "`dcterms:modified`, like `pptx.created`. Date.",
    },
    KeyDoc {
        key: quail_pptx::keys::APPLICATION,
        doc: "`Application`, e.g. `Microsoft Office PowerPoint`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::COMPANY,
        doc: "`Company`. Text.",
    },
    KeyDoc {
        key: quail_pptx::keys::SLIDES,
        doc: "Slides listed in `presentation.xml` (`p:sldIdLst`), read or not. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::HIDDEN_SLIDES,
        doc: "Slides marked hidden (`show=\"0\"`); their text is still read. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::NOTES_SLIDES,
        doc: "Slides whose speaker notes have text. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::TABLES,
        doc: "Tables (`a:tbl`) emitted as table cells. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::HIDDEN_SHAPES,
        doc: "Shapes and groups with text left out because they are hidden (`cNvPr hidden=\"1\"`). Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::HAS_MACROS,
        doc: "A `vbaProject.bin` part is present (`.pptm`). Bool.",
    },
    KeyDoc {
        key: quail_pptx::keys::IMAGES,
        doc: "Parts under `ppt/media/`. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::EMBEDDED_OBJECTS,
        doc: "Parts under `ppt/embeddings/`. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::ZIP_ENTRIES,
        doc: "Entries in the zip directory. Int.",
    },
    KeyDoc {
        key: quail_pptx::keys::DUPLICATE_ENTRIES,
        doc: "Entries whose name repeats an earlier one, exactly or by case. Int.",
    },
];

/// Keys of `quail_html::keys`.
#[cfg(feature = "html")]
pub const HTML: &[KeyDoc] = &[
    KeyDoc {
        key: quail_html::keys::ENCODING,
        doc: "Encoding the text was decoded with (WHATWG name, e.g. `UTF-8`, `windows-1252`). Text.",
    },
    KeyDoc {
        key: quail_html::keys::ENCODING_SOURCE,
        doc: "Where `html.encoding` came from: `bom`, `meta` (`<meta charset>` or `http-equiv`), `xml` (the XML declaration), `utf8` (none declared, valid UTF-8) or `guess`. Text.",
    },
    KeyDoc {
        key: quail_html::keys::DECODE_ERRORS,
        doc: "Bytes that were not valid in `html.encoding` and became U+FFFD (only when there were any). Int.",
    },
    KeyDoc {
        key: quail_html::keys::TITLE,
        doc: "Text of the first `<title>`, whitespace collapsed. Text.",
    },
    KeyDoc {
        key: quail_html::keys::LANG,
        doc: "`lang` (or `xml:lang`) of the `<html>` element. Text.",
    },
    KeyDoc {
        key: quail_html::keys::GENERATOR,
        doc: "`content` of `<meta name=\"generator\">` (e.g. `Workiva`, `Microsoft Word 15`). Text.",
    },
    KeyDoc {
        key: quail_html::keys::INLINE_XBRL,
        doc: "The file is inline XBRL: it declares the inline XBRL namespace or uses `ix:` facts. Bool.",
    },
    KeyDoc {
        key: quail_html::keys::IX_FACTS,
        doc: "Inline XBRL facts (`ix:nonFraction` and `ix:nonNumeric`, hidden ones included). Int.",
    },
    KeyDoc {
        key: quail_html::keys::XBRL_DEI,
        doc: "The `dei:` cover facts of an inline XBRL filing, `name=value` in file order, hidden ones included (e.g. `DocumentType=10-K`, `EntityRegistrantName=...`, `EntityCentralIndexKey=0000123456`). At most 200, values at most 1 KiB. List of Text.",
    },
    KeyDoc {
        key: quail_html::keys::TABLES,
        doc: "Data tables emitted as table cells. Int.",
    },
    KeyDoc {
        key: quail_html::keys::LAYOUT_TABLES,
        doc: "Layout tables (one cell, or cells holding headings, lists, page breaks or other tables) whose cells were read as ordinary blocks. Int.",
    },
    KeyDoc {
        key: quail_html::keys::HIDDEN_ELEMENTS,
        doc: "Elements left out as hidden (`hidden`, `display:none`, `visibility:hidden`, `ix:header`), counted at the outermost one. Int.",
    },
    KeyDoc {
        key: quail_html::keys::SCRIPTS,
        doc: "`<script>` elements (their code is never run or kept). Int.",
    },
    KeyDoc {
        key: quail_html::keys::ELEMENTS,
        doc: "Start tags read. Int.",
    },
];

/// Keys of `quail_pdf::keys`.
#[cfg(feature = "pdf")]
pub const PDF: &[KeyDoc] = &[
    KeyDoc {
        key: quail_pdf::keys::VERSION,
        doc: "PDF version, e.g. `\"1.7\"` (catalog `Version` wins over the header). Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::PAGE_COUNT,
        doc: "Number of pages in the document (not only the extracted ones). Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::ENCRYPTED,
        doc: "The file has an `/Encrypt` dictionary (owner- or user-password protected). Bool.",
    },
    KeyDoc {
        key: quail_pdf::keys::PRODUCER,
        doc: "Info `Producer`. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::CREATOR,
        doc: "Info `Creator`. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::AUTHOR,
        doc: "Info `Author`. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::TITLE,
        doc: "Info `Title`. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::SUBJECT,
        doc: "Info `Subject`. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::KEYWORDS,
        doc: "Info `Keywords`. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::CREATED,
        doc: "Info `CreationDate`. Date.",
    },
    KeyDoc {
        key: quail_pdf::keys::MODIFIED,
        doc: "Info `ModDate`. Date.",
    },
    KeyDoc {
        key: quail_pdf::keys::CREATED_UTC_OFFSET,
        doc: "UTC offset of `CreationDate`, e.g. `+02:00`, when it has a non-zero one. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::MODIFIED_UTC_OFFSET,
        doc: "UTC offset of `ModDate`, e.g. `-05:00`, when it has a non-zero one. Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::INCREMENTAL_UPDATES,
        doc: "Incremental updates appended after the first revision (`%%EOF` sections minus one, minus the extra section of a linearized file). Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::LINEARIZED,
        doc: "The file is linearized (\"fast web view\"). Bool.",
    },
    KeyDoc {
        key: quail_pdf::keys::HAS_XMP,
        doc: "The catalog has an XMP metadata stream. Bool.",
    },
    KeyDoc {
        key: quail_pdf::keys::SIGNATURE_FIELDS,
        doc: "Signature form fields. Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::SIGNED,
        doc: "At least one signature field carries a signature value. Bool.",
    },
    KeyDoc {
        key: quail_pdf::keys::FORM_FIELDS,
        doc: "Terminal `AcroForm` fields (signatures included). Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::HAS_JAVASCRIPT,
        doc: "The document contains JavaScript actions. Bool.",
    },
    KeyDoc {
        key: quail_pdf::keys::EMBEDDED_FILES,
        doc: "Files in the `EmbeddedFiles` name tree. Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::FONTS,
        doc: "Font names used by the extracted pages, subset tag stripped, deduplicated, sorted. List of Text.",
    },
    KeyDoc {
        key: quail_pdf::keys::IMAGE_ONLY_PAGES,
        doc: "Extracted pages that are an image with (almost) no text. Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::PAGES_NEEDING_OCR,
        doc: "Extracted pages flagged `needs_ocr`. Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::SCANNED_PAGES,
        doc: "Extracted pages that are scans needing OCR: image-only, or mostly an image whose text layer is too thin or garbage (a bad OCR layer, e.g. Cyrillic read as Latin). A scan with a usable OCR layer is not counted. Int.",
    },
    KeyDoc {
        key: quail_pdf::keys::IS_SCANNED,
        doc: "More than half of the extracted pages are `SCANNED_PAGES`: the document is a scan that needs OCR. Emitted for every PDF with an extracted page. Bool.",
    },
];

/// Keys of `quail_text::keys`.
#[cfg(feature = "text")]
pub const TEXT: &[KeyDoc] = &[
    KeyDoc {
        key: quail_text::keys::CHARS,
        doc: "Characters of the document text, whitespace excluded (Int).",
    },
    KeyDoc {
        key: quail_text::keys::WORDS,
        doc: "Word-like tokens (words, numbers, codes) of the document text (Int).",
    },
    KeyDoc {
        key: quail_text::keys::PAGES_WITH_TEXT,
        doc: "Pages with at least one non-whitespace character (Int).",
    },
    KeyDoc {
        key: quail_text::keys::SCRIPT,
        doc: "Dominant script of the letters: `latin`, `cyrillic`, `greek`, `other`, or `mixed` when no script has 80 % of the letters (Text). Absent without letters.",
    },
    KeyDoc {
        key: quail_text::keys::SCRIPT_LATIN,
        doc: "Share of letters in Latin script (Float, 0..=1).",
    },
    KeyDoc {
        key: quail_text::keys::SCRIPT_CYRILLIC,
        doc: "Share of letters in Cyrillic script (Float, 0..=1).",
    },
    KeyDoc {
        key: quail_text::keys::SCRIPT_GREEK,
        doc: "Share of letters in Greek script (Float, 0..=1).",
    },
    KeyDoc {
        key: quail_text::keys::SCRIPT_OTHER,
        doc: "Share of letters in any other script (Float, 0..=1).",
    },
    KeyDoc {
        key: quail_text::keys::GARBAGE_RATIO,
        doc: "Document-level garbage ratio, see `crate::garbage` (Float, 0..=1).",
    },
    KeyDoc {
        key: quail_text::keys::GARBAGE_PAGES,
        doc: "Pages with enough text to judge whose garbage ratio exceeds `crate::garbage::NEEDS_OCR_RATIO` (Int).",
    },
    KeyDoc {
        key: quail_text::keys::LANG,
        doc: "Most likely language, ISO 639-1 (`sr`, `hr`, `en`, ...); `conf` is its score (Text).",
    },
    KeyDoc {
        key: quail_text::keys::LANG_TOP,
        doc: "Top candidates as a list of `[code, score]` pairs, best first (List).",
    },
    KeyDoc {
        key: quail_text::keys::PAGE_LANG,
        doc: "Language of one page, with the page as evidence; `conf` is its score (Text).",
    },
    KeyDoc {
        key: quail_text::keys::ENCODING,
        doc: "Plain-text files: lowercase WHATWG name of the detected encoding (`utf-8`, `utf-16le`, `windows-1250`, `iso-8859-5`, ...); `conf` is its confidence (Text).",
    },
    KeyDoc {
        key: quail_text::keys::ENCODING_CONFIDENCE,
        doc: "Plain-text files: confidence of the encoding, 1 for a BOM or valid UTF-8, a heuristic in `[0.3, 0.9]` for a guessed legacy encoding, see `crate::decode` (Float).",
    },
    KeyDoc {
        key: quail_text::keys::ENCODING_BOM,
        doc: "Plain-text files: the file starts with a byte-order mark (Bool).",
    },
    KeyDoc {
        key: quail_text::keys::DECODE_ERRORS,
        doc: "Plain-text files: byte sequences that could not be decoded and became U+FFFD; absent when there were none (Int).",
    },
    KeyDoc {
        key: quail_text::keys::LINE_ENDINGS,
        doc: "Plain-text files: line endings before normalization to LF: `lf`, `crlf`, `cr`, `mixed` or `none` (Text).",
    },
];

/// Keys of `quail_signals::keys`.
#[cfg(feature = "signals")]
pub const SIGNALS: &[KeyDoc] = &[
    KeyDoc {
        key: quail_signals::keys::IBAN,
        doc: "IBAN, compact and uppercase (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::BIC,
        doc: "BIC / SWIFT code (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::VAT,
        doc: "EU / UK VAT number with its country prefix, compact (`DE136695976`, `ATU04571784`) (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::RS_PIB,
        doc: "Serbian tax number PIB, 9 digits (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::RS_MB,
        doc: "Serbian registration number (matični broj) of a company, 8 digits (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::RS_ACCOUNT,
        doc: "Serbian bank account `BBB-NNNNNNNNNNNNN-KK`, middle part zero-padded (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::HR_OIB,
        doc: "Croatian OIB, 11 digits (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::LEI,
        doc: "LEI, 20 characters (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::DUNS,
        doc: "DUNS number, 9 digits (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::US_EIN,
        doc: "US EIN `12-3456789` (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::JMBG_COUNT,
        doc: "Number of valid JMBGs (personal IDs) in the file; the values are never emitted (Int).",
    },
    KeyDoc {
        key: quail_signals::keys::DATE,
        doc: "A calendar date (Date).",
    },
    KeyDoc {
        key: quail_signals::keys::DATE_EFFECTIVE,
        doc: "A date a keyword marks as the start / effective date (Date).",
    },
    KeyDoc {
        key: quail_signals::keys::DATE_EXPIRY,
        doc: "A date a keyword marks as the end / expiry date (Date).",
    },
    KeyDoc {
        key: quail_signals::keys::DATE_SIGNED,
        doc: "A date a keyword marks as the signing date (Date).",
    },
    KeyDoc {
        key: quail_signals::keys::DATE_INVOICE,
        doc: "A date a keyword marks as the invoice / issue date (Date).",
    },
    KeyDoc {
        key: quail_signals::keys::DATE_DUE,
        doc: "A date a keyword marks as the payment due date (Date).",
    },
    KeyDoc {
        key: quail_signals::keys::AMOUNT,
        doc: "An amount with its currency, exact decimal (Money).",
    },
    KeyDoc {
        key: quail_signals::keys::AMOUNT_TOTAL,
        doc: "An amount after a total keyword (`Ukupno`, `Total`, `Za uplatu`) (Money).",
    },
    KeyDoc {
        key: quail_signals::keys::AMOUNT_NET,
        doc: "An amount marked as without VAT (`bez PDV-a`, `excl. VAT`) (Money).",
    },
    KeyDoc {
        key: quail_signals::keys::AMOUNT_GROSS,
        doc: "An amount marked as with VAT (`sa PDV-om`, `incl. VAT`) (Money).",
    },
    KeyDoc {
        key: quail_signals::keys::EMAIL_DOMAIN,
        doc: "Domain of an e-mail address, free-mail hosts excluded (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::WEB_DOMAIN,
        doc: "Domain of a URL or web address (Text).",
    },
    KeyDoc {
        key: quail_signals::keys::PHONE,
        doc: "Phone number in E.164 (`+381111234567`) (Text).",
    },
];

/// Keys of `quail_refs::keys`.
#[cfg(feature = "refs")]
pub const REFS: &[KeyDoc] = &[
    KeyDoc {
        key: quail_refs::keys::DOC_NUMBER,
        doc: "An own number of this document (record: `kind`, `raw`, `key`, `base`, `date`).",
    },
    KeyDoc {
        key: quail_refs::keys::CITES,
        doc: "A document this one cites (record: `kind`, `relation`, `raw`, `key`, `base`, `ordinal`, `date`).",
    },
    KeyDoc {
        key: quail_refs::keys::TITLE,
        doc: "The document title (Text).",
    },
    KeyDoc {
        key: quail_refs::keys::INSTRUMENT_KIND,
        doc: "The instrument kind the title (or the first own label) names (Text, see `crate::Kind`).",
    },
    KeyDoc {
        key: quail_refs::keys::ORDINAL,
        doc: "The ordinal of the instrument: Amendment No. 3 → 3 (Int).",
    },
    KeyDoc {
        key: quail_refs::keys::PAGE_MARKER,
        doc: "A page-of-page marker, one per page at most (record: `n`, `of`).",
    },
    KeyDoc {
        key: quail_refs::keys::VERSION_HINT,
        doc: "A version marker (record: `kind`, `n`).",
    },
    KeyDoc {
        key: quail_refs::keys::STUDY_CODE,
        doc: "A clinical study code (`D\\d{3}[0-9A-Z]C\\d{5}`), upper-cased (Text).",
    },
    KeyDoc {
        key: quail_refs::keys::CLM_RECORD,
        doc: "A CLM record id, `con_<digits>` (Text).",
    },
];

/// Keys of `quail_parties::keys`.
#[cfg(feature = "parties")]
pub const PARTIES: &[KeyDoc] = &[
    KeyDoc {
        key: quail_parties::keys::COMPANY,
        doc: "A company mention (Record: `surface`, `key`, `norm`, `quality`, `legal_form`?, `jurisdiction`?, `jurisdiction_conf`?, `roles`, `role_conf`, `defined_term`?).",
    },
    KeyDoc {
        key: quail_parties::keys::ALIAS_OF,
        doc: "A name statement (Record: `current`, `other`, `kind` = `former_name`, `trading_name` or `dba`).",
    },
    KeyDoc {
        key: quail_parties::keys::CANDIDATES,
        doc: "Ranked candidates (List of Record: `key`, `surface`, `score`, `roles`).",
    },
    KeyDoc {
        key: quail_parties::keys::SIGNATORY_TITLE,
        doc: "A title a signatory signs under (Text), no person name.",
    },
    KeyDoc {
        key: quail_parties::keys::SIGNATORY_COUNT,
        doc: "Lines of signature blocks naming a signatory by title or label (Int).",
    },
];

/// Keys of `quail_clauses::keys`.
#[cfg(feature = "clauses")]
pub const CLAUSES: &[KeyDoc] = &[
    KeyDoc {
        key: quail_clauses::keys::GOVERNING_LAW,
        doc: "Governing law (Record: `jurisdiction`, `code`, `raw`, `basis`).",
    },
    KeyDoc {
        key: quail_clauses::keys::TERM,
        doc: "Term of the agreement (Record: `duration`, `text`, `raw`, `renewal`).",
    },
    KeyDoc {
        key: quail_clauses::keys::SURVIVAL,
        doc: "How long obligations survive (Record: `duration`, `text`, `raw`).",
    },
    KeyDoc {
        key: quail_clauses::keys::NOTICE_PERIOD,
        doc: "A notice period (Record: `duration`, `text`, `raw`).",
    },
    KeyDoc {
        key: quail_clauses::keys::TERMINATION,
        doc: "A termination right (Record: `kind`, `raw`).",
    },
];

/// Keys of `quail_sketch::keys`.
#[cfg(feature = "sketch")]
pub const SKETCH: &[KeyDoc] = &[
    KeyDoc {
        key: quail_sketch::keys::MINHASH,
        doc: "MinHash signature of the word 3-shingles of the normalized, folded text (`Value::Bytes`, 128 rows × 4 bytes little-endian; see `crate::minhash`). Compare with `crate::jaccard_estimate`. Absent below `crate::minhash::MIN_SHINGLES` distinct shingles.",
    },
    KeyDoc {
        key: quail_sketch::keys::MINHASH_BANDS,
        doc: "LSH band keys of `MINHASH` (`Value::List` of 32 `Value::U64`, band index mixed in, so all bands can share one index). Two files sharing any key are candidates.",
    },
    KeyDoc {
        key: quail_sketch::keys::MINHASH_SHINGLES,
        doc: "Distinct word 3-shingles behind `MINHASH` (`Value::Int`).",
    },
    KeyDoc {
        key: quail_sketch::keys::SIMHASH_TEXT,
        doc: "64-bit SimHash of the weighted word 3-shingles (`Value::U64`). Compare with `crate::hamming`.",
    },
    KeyDoc {
        key: quail_sketch::keys::SIMHASH_TEMPLATE,
        doc: "64-bit SimHash of the entity-masked words: numbers, dates, amounts, codes, names, URLs and e-mail addresses replaced by their class (`Value::U64`).",
    },
    KeyDoc {
        key: quail_sketch::keys::LAYOUT,
        doc: "64-bit SimHash of the page geometry: quantized block boxes, kinds and zones of the first pages (`Value::U64`). Only for documents with block boxes (PDF).",
    },
    KeyDoc {
        key: quail_sketch::keys::LAYOUT_BLOCKS,
        doc: "Blocks with a box that went into `LAYOUT` (`Value::Int`).",
    },
    KeyDoc {
        key: quail_sketch::keys::STRUCTURE,
        doc: "64-bit SimHash of the block-kind sequence: headings and levels, lists, tables and their shape, paragraph runs (`Value::U64`). Any format with blocks.",
    },
    KeyDoc {
        key: quail_sketch::keys::HEADINGS,
        doc: "Digest of the heading skeleton (heading texts, numbering stripped, lowercased), the `structural_fp` of `crate::compat` as a big-endian integer (`Value::U64`). Compare for equality. Absent with fewer than two headings.",
    },
    KeyDoc {
        key: quail_sketch::keys::INVOICE_LAYOUT,
        doc: "Invoice layout digest: which of 16 invoice labels occur, plus the shapes of invoice-number-like codes (`Value::U64`, see `crate::invoice`). Compare for equality.",
    },
    KeyDoc {
        key: quail_sketch::keys::INVOICE_LABELS,
        doc: "Invoice labels found (`Value::Int`, 1..=16).",
    },
    KeyDoc {
        key: quail_sketch::keys::INVOICE_NUMBER_PATTERNS,
        doc: "Distinct invoice-number shapes found (`Value::Int`).",
    },
    KeyDoc {
        key: quail_sketch::keys::TEXT_KEY,
        doc: "XXH3-128 of the folded words joined by one space (`Value::Bytes`, 16 bytes): the same words, whatever the bytes, spacing, case, diacritics or script. Absent below 400 characters.",
    },
    KeyDoc {
        key: quail_sketch::keys::COMPAT_MINHASH,
        doc: "Python-compatible version signature (`Value::Bytes`, 128 rows × 8 bytes little-endian), see `crate::compat`.",
    },
    KeyDoc {
        key: quail_sketch::keys::COMPAT_MINHASH_BANDS,
        doc: "Python-compatible LSH bucket keys of `COMPAT_MINHASH` (`Value::List` of 32 `Value::Text`, `\"<band>:<16 hex>\"`).",
    },
    KeyDoc {
        key: quail_sketch::keys::COMPAT_SHINGLES,
        doc: "Distinct shingles behind `COMPAT_MINHASH` (`Value::Int`).",
    },
    KeyDoc {
        key: quail_sketch::keys::COMPAT_TEMPLATE,
        doc: "Python-compatible template SimHash of the entity-masked words (`Value::U64`).",
    },
    KeyDoc {
        key: quail_sketch::keys::PAGE_PHASH,
        doc: "64-bit DCT pHash per scanned page (`Value::List` of `Value::U64`), aligned with `PAGE_PHASH_PAGES`.",
    },
    KeyDoc {
        key: quail_sketch::keys::PAGE_PHASH_PAGES,
        doc: "Zero-based page indices of `PAGE_PHASH` (`Value::List` of `Value::Int`).",
    },
];

/// Keys of `quail_items::keys`.
#[cfg(feature = "items")]
pub const ITEMS: &[KeyDoc] = &[
    KeyDoc {
        key: quail_items::keys::LINE,
        doc: "One price item (Record, see the crate documentation).",
    },
    KeyDoc {
        key: quail_items::keys::TABLE_TOTAL,
        doc: "A subtotal / VAT / total row of a price table (Record).",
    },
    KeyDoc {
        key: quail_items::keys::COUNT,
        doc: "Number of `item.line` facts (Int).",
    },
];

/// Keys of `quail_payment::keys`.
#[cfg(feature = "payment")]
pub const PAYMENT: &[KeyDoc] = &[
    KeyDoc {
        key: quail_payment::keys::SUBJECT,
        doc: "What is paid for, as the document states it (Record: `text`, `source`: `clause`, `after_heading`, `line_items` or `title`).",
    },
    KeyDoc {
        key: quail_payment::keys::CATEGORY,
        doc: "Category of what is paid for (Record: `code`, `label`, `keyword`). Codes: `it_software`, `telecom`, `legal`, `accounting_audit`, `consulting`, `marketing`, `rent_lease`, `utilities`, `fuel_energy`, `logistics_transport`, `cleaning_facility`, `maintenance_repair`, `insurance`, `goods_materials`, `financial_services`, `other`.",
    },
    KeyDoc {
        key: quail_payment::keys::AMOUNT,
        doc: "The price the document commits to (Money): the gross amount when stated, else the plain total, else the net amount. A monthly fee stays a monthly fee (see `payment.period`).",
    },
    KeyDoc {
        key: quail_payment::keys::AMOUNT_BASIS,
        doc: "What `payment.amount` is: `gross`, `net` or `unstated` (Text).",
    },
    KeyDoc {
        key: quail_payment::keys::CURRENCY,
        doc: "ISO 4217 code of the price (Text).",
    },
    KeyDoc {
        key: quail_payment::keys::NET,
        doc: "The price without VAT, where stated (Money).",
    },
    KeyDoc {
        key: quail_payment::keys::VAT,
        doc: "The VAT amount, where stated (Money).",
    },
    KeyDoc {
        key: quail_payment::keys::GROSS,
        doc: "The price with VAT, where stated (Money).",
    },
    KeyDoc {
        key: quail_payment::keys::PERIOD,
        doc: "How often the price is paid: `one_off`, `monthly`, `quarterly`, `yearly` (Text).",
    },
    KeyDoc {
        key: quail_payment::keys::LINES,
        doc: "Invoice and offer line items (List of Record: `description`, `quantity`, `unit`, `unit_price`, `total`, `currency`, `vat_rate`, fields where known; from `item.line`).",
    },
    KeyDoc {
        key: quail_payment::keys::HIGHLIGHT,
        doc: "Where a payment fact sits in the text, one per fact (one per line of `payment.lines`). Record: `key` (the fact key), `page`, `block` (the block's index among all the document's blocks, as doewood's `GetDocumentText` numbers them: blank blocks skipped, the cells of a table row merged), `start`, `end` (characters in the block's text, end exclusive), `quote` (that text); `conf` is the fact's. Shared shape, see `quail_core::highlight`.",
    },
];

/// Keys of `quail_summary::keys`.
#[cfg(feature = "summary")]
pub const SUMMARY: &[KeyDoc] = &[
    KeyDoc {
        key: quail_summary::keys::CARD,
        doc: "The file card (Record: `format kind ordinal title title_source lang script pages number cites cites_total parties parties_total dates amounts currencies ids personal_ids items sketch producer author company flags`, absent fields left out; see the README).",
    },
    KeyDoc {
        key: quail_summary::keys::CARD_TEXT,
        doc: "The card as compact `key: value` lines within the token budget (Text).",
    },
    KeyDoc {
        key: quail_summary::keys::CARD_TOKENS,
        doc: "Estimated tokens of `CARD_TEXT` (Int, see `crate::tokens`).",
    },
    KeyDoc {
        key: quail_summary::keys::EXTRACTIVE,
        doc: "1–3 sentence English summary without a model (Text).",
    },
    KeyDoc {
        key: quail_summary::keys::SUBJECT,
        doc: "The subject sentence, verbatim (Record: `text`, `lang`?, `method` = `clause`, `recital`, `operative` or `textrank`, `label`?), evidence: its page.",
    },
    KeyDoc {
        key: quail_summary::keys::OUTLINE,
        doc: "The document's headings with page ranges (Record: `method` = `headings` or `windows`, `total`, `pages`, `sections` = list of `title level page end_page source`, pages zero-based; `truncated`?).",
    },
    KeyDoc {
        key: quail_summary::keys::DIGEST,
        doc: "The bounded summary input of a document of any length (Text: `FACTS` = the card, then the whole `TEXT` when it fits, else `OUTLINE`, `START` and per-section `EXCERPTS` copied verbatim).",
    },
    KeyDoc {
        key: quail_summary::keys::DIGEST_TOKENS,
        doc: "Estimated tokens of `DIGEST` (Int, see `crate::tokens`).",
    },
    KeyDoc {
        key: quail_summary::keys::KEYWORDS,
        doc: "Document keywords and key phrases, best first (List of Record: `term`, `key` = the matching key, stems in ASCII, the same for Latin and Cyrillic, `score` in (0, 1], `count`, `first_page` zero-based, `entities`? = keys of the `party.candidates` it names).",
    },
    KeyDoc {
        key: quail_summary::keys::TOPICS,
        doc: "Topic map of a long document (Record: `method` = `headings` or `windows`, `threshold`, `sections` = list of `title page end_page keywords shift`, `segments` = list of `title first sections page end_page keywords shift` (`first`: index of its first section), pages zero-based; `truncated`?).",
    },
    KeyDoc {
        key: quail_summary::keys::LLM,
        doc: "The LLM summary (Record: `text`, `subject`?, `title_en`?, `method` = `llm` or `extractive_fallback`, `model`, `checked`, `cached`, `input` = `digest` or `map_reduce`, `calls`, `prompt_tokens`?, `output_tokens`?). Feature `llm` or `api`.",
    },
];

/// Keys of `quail_ocr::keys`.
#[cfg(feature = "ocr")]
pub const OCR: &[KeyDoc] = &[
    KeyDoc {
        key: quail_ocr::keys::PAGES,
        doc: "Pages OCR'd (Int).",
    },
    KeyDoc {
        key: quail_ocr::keys::CHARS,
        doc: "Characters read, spaces excluded (Int).",
    },
    KeyDoc {
        key: quail_ocr::keys::MEAN_CONF,
        doc: "Mean recognition confidence weighted by characters (Float).",
    },
    KeyDoc {
        key: quail_ocr::keys::SCRIPT,
        doc: "Dominant script of the OCR'd text (Text).",
    },
    KeyDoc {
        key: quail_ocr::keys::PAGE,
        doc: "One OCR'd page (Record: `page`, `script`, `mean_conf`, `chars`, `lines`, `dpi`, `source`, `rotation`).",
    },
];

//! Every fact key the HTML extractor emits. All live under `html.*`.

use quail_core::FactKey;

/// Encoding the text was decoded with (WHATWG name, e.g. `UTF-8`, `windows-1252`). Text.
pub const ENCODING: FactKey = FactKey::new("html.encoding");
/// Where [`ENCODING`] came from: `bom`, `meta` (`<meta charset>` or `http-equiv`), `xml`
/// (the XML declaration), `utf8` (none declared, valid UTF-8) or `guess`. Text.
pub const ENCODING_SOURCE: FactKey = FactKey::new("html.encoding_source");
/// Bytes that were not valid in [`ENCODING`] and became U+FFFD (only when there were
/// any). Int.
pub const DECODE_ERRORS: FactKey = FactKey::new("html.decode_errors");
/// Text of the first `<title>`, whitespace collapsed. Text.
pub const TITLE: FactKey = FactKey::new("html.title");
/// `lang` (or `xml:lang`) of the `<html>` element. Text.
pub const LANG: FactKey = FactKey::new("html.lang");
/// `content` of `<meta name="generator">` (e.g. `Workiva`, `Microsoft Word 15`). Text.
pub const GENERATOR: FactKey = FactKey::new("html.generator");
/// The file is inline XBRL: it declares the inline XBRL namespace or uses `ix:` facts. Bool.
pub const INLINE_XBRL: FactKey = FactKey::new("html.inline_xbrl");
/// Inline XBRL facts (`ix:nonFraction` and `ix:nonNumeric`, hidden ones included). Int.
pub const IX_FACTS: FactKey = FactKey::new("html.ix_facts");
/// The `dei:` cover facts of an inline XBRL filing, `name=value` in file order, hidden ones
/// included (e.g. `DocumentType=10-K`, `EntityRegistrantName=...`,
/// `EntityCentralIndexKey=0000123456`). At most 200, values at most 1 KiB. List of Text.
pub const XBRL_DEI: FactKey = FactKey::new("html.xbrl_dei");
/// Data tables emitted as table cells. Int.
pub const TABLES: FactKey = FactKey::new("html.tables");
/// Layout tables (one cell, or cells holding headings, lists, page breaks or other tables)
/// whose cells were read as ordinary blocks. Int.
pub const LAYOUT_TABLES: FactKey = FactKey::new("html.layout_tables");
/// Elements left out as hidden (`hidden`, `display:none`, `visibility:hidden`,
/// `ix:header`), counted at the outermost one. Int.
pub const HIDDEN_ELEMENTS: FactKey = FactKey::new("html.hidden_elements");
/// `<script>` elements (their code is never run or kept). Int.
pub const SCRIPTS: FactKey = FactKey::new("html.scripts");
/// Start tags read. Int.
pub const ELEMENTS: FactKey = FactKey::new("html.elements");

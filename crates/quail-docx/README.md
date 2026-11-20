# quail-docx

DOCX extractor for quail: a streaming OOXML reader on `zip` (inflate only) and `quick-xml`
(event reader, no DOM). Pure Rust, no `unsafe`.

- Extractor id: `docx.extract@1` (`quail_docx::ID`), formats `[Format::Docx]`.
- Public API: `DocxExtractor` (implements `quail_core::Extractor`), `ID`, `keys::*`.

## Document model

- One logical page per section (`w:sectPr`). Page text in reading order: section header,
  body, the footnotes/endnotes referenced in the section (`Note`), section footer. Headers and
  footers are emitted once per distinct text (`Zone::Header` / `Zone::Footer`).
- Blocks: `Paragraph`; `Heading { level }` (outline level, or `heading N` / `title` styles via
  the `basedOn` chain, zone `Title`); `ListItem` (`w:numPr`, direct or from the style);
  `TableCell { table, row, col }` (grid spans respected, zone `Table`; nested tables are
  flattened into the outer cell); `Note`; `Annotation`.
- Numbered paragraphs begin with the label Word shows (`7.1`, `(a)`, `iv`, `-` for bullets).
- **Accepted-changes text**: tracked insertions are in the text, deletions are not. Each
  deletion's text, and each comment referenced from a paragraph, follows that paragraph as an
  `Annotation` block.
- Fields keep their result, not their code. `w:tab` → `\t`, `w:br` → `\n`. Text boxes are read
  once (`mc:Fallback` skipped).

## Keys (`docx.*`)

| Source | Keys |
|---|---|
| `docProps/core.xml` | `title`, `subject`, `creator`, `last_modified_by`, `revision`, `created`, `modified`, `last_printed`, `keywords`, `category`, `description` |
| `docProps/app.xml` | `application`, `app_version`, `company`, `manager`, `template`, `total_time`, `pages`, `words`, `characters`, `app_paragraphs` |
| `docProps/custom.xml` | `custom_property_count`, `custom_property_names`, `custom_properties` (`name=value`) |
| `word/settings.xml` | `rsids` (List of U64, file order), `rsid_root` (U64), `rsid_count`, `attached_template`, `track_revisions`, `protection` |
| revisions, comments | `tracked_insertions`, `tracked_deletions`, `tracked_format_changes`, `revision_authors`, `comments`, `comment_authors` |
| package | `has_macros`, `main_content_type`, `images`, `embedded_objects`, `hyperlink_domains`, `zip_entries` |
| structure | `sections`, `paragraphs`, `tables`, `nested_tables`, `footnotes`, `endnotes`, `headers`, `footers` |
| `customXml` CLM form | `clm_first_party`, `clm_second_party` |

Dates are kept as written (W3CDTF text). rsids are accepted only as exactly 8 hex digits.

## Limits

- At most 20 000 zip entries; entries with absolute, `..`, drive-letter or backslash names are
  ignored (warning).
- The bytes each part **actually** inflates to are counted against `Limits::max_decompressed`
  for the whole file; reading stops at the first byte over it. Declared sizes are never trusted.
- `max_pages` caps sections, `max_text_bytes` caps text; the deadline is checked every 2048 XML
  events and before each part.
- Malformed XML or a broken entry ends that part only: partial document plus a warning.
  Encrypted (OLE-wrapped) files return `Error::Encrypted`; a non-zip returns `Error::Malformed`.

## Development

`cargo test -p quail-docx` (all inputs are generated in code), `cargo bench -p quail-docx`.
`QUAIL_TESTDATA=<dir> cargo test -p quail-docx --test corpus` extracts every DOCX under a local
corpus and fuzzes stored copies of it; `cargo run -p quail-docx --example corpus -- <dir>` prints
aggregate numbers only.

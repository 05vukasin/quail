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
| `docProps/core.xml` | `title`, `subject`, `creator`, `last_modified_by`, `revision`, `created`, `modified`, `last_printed` (Dates), `keywords`, `category`, `description` |
| `docProps/app.xml` | `application`, `app_version`, `company`, `manager`, `template`, `total_time`, `pages`, `words`, `characters`, `app_paragraphs` |
| `docProps/custom.xml` | `custom_property_count`, `custom_property_names`, `custom_properties` (`name=value`) |
| `word/settings.xml` | `rsids` (List of U64, file order), `rsid_root` (U64), `rsid_count`, `attached_template`, `track_revisions`, `protection` |
| revisions, comments | `tracked_insertions`, `tracked_deletions`, `tracked_format_changes`, `revision_authors`, `deleted_text_chars`, `hidden_text_chars`, `comments`, `comment_authors` |
| package | `has_macros`, `main_content_type`, `images`, `embedded_objects`, `hyperlink_domains`, `zip_entries`, `duplicate_entries` |
| structure | `sections`, `paragraphs`, `tables`, `nested_tables`, `footnotes`, `endnotes`, `headers`, `footers` |
| `customXml` CLM form | `clm_first_party`, `clm_second_party` |

Dates are the calendar date of the W3CDTF value (an unparsable one is a warning). rsids are
accepted only as exactly 8 hex digits. Hidden runs (`w:vanish` on the run itself) are left out
of the text and counted in `hidden_text_chars`.

## Limits

Stopping short at any limit is an `Emitter::problem` (the record becomes partial, the document
is kept); notes about the file are warnings.

| What | Limit |
|---|---|
| zip entries | 20 000, read from the EOCD / ZIP64 record before the directory is parsed (`LimitExceeded`) |
| entry names | absolute, `..`, drive letter, backslash, control characters: ignored (warning); exact repeats: the last is read, case variants: the first (counted in `duplicate_entries`) |
| inflated bytes | the bytes parts **actually** inflate to, against `max_decompressed` for the whole file; declared sizes never trusted. The body is read before footnotes, endnotes and comments, so they cannot starve it; every part left without budget is named in a problem |
| one XML event | remaining text budget + 1 MiB, at most 17 MiB (a huge text node is not buffered whole) |
| sections | `max_pages`: the body stops after that many section ends |
| text | `max_text_bytes`: every byte of page text is charged, separators and numbering labels included (labels at most 32 chars) |
| items | 250 000 blocks, references and section ends per file; then the part stops |
| nesting | 64 paragraphs, 64 tables, 64 fields deep (deeper ones flattened, ends kept in sync) |
| fields | a field still in its instruction part when its paragraph ends is abandoned (warning) |
| lists, styles, rels | 20 000 lists, 20 000 styles, 50 000 relationships per part, 1 override per list level, 100 000 rsids |
| deadline | checked every 2048 XML events (skipped subtrees included), before each part, per section and while opening the zip |

Malformed XML or a broken entry ends that part only. Encrypted (OLE-wrapped) files return
`Error::Encrypted`; a non-zip returns `Error::Malformed`.

## Development

`cargo test -p quail-docx` (all inputs are generated in code), `cargo bench -p quail-docx`.
`QUAIL_TESTDATA=<dir> cargo test -p quail-docx --test corpus` extracts every DOCX under a local
corpus and fuzzes stored copies of it; `cargo run -p quail-docx --example corpus -- <dir>` prints
aggregate numbers only.

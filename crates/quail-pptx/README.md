# quail-pptx

PPTX extractor for quail: a streaming PresentationML reader on `quail-ooxml` (the zip package
and `quick-xml` event walk it shares with `quail-docx`; no DOM). Pure Rust, no `unsafe`.

- Extractor id: `pptx.extract@1` (`quail_pptx::ID`), formats `[Format::Pptx]`.
- Public API: `PptxExtractor` (implements `quail_core::Extractor`), `ID`, `keys::*`.

## Document model

- One logical page per slide, in `p:sldIdLst` order (hidden slides included and counted).
- Reading order on a slide: title placeholders (`title`, `ctrTitle`, then `subTitle`), then the
  other shapes top to bottom and left to right by their `xfrm` offset, then the date, footer
  and slide-number placeholders (`Zone::Footer`). A placeholder without its own `xfrm` takes
  the position of the layout's placeholder with the same `idx` (else the same type), else the
  master's. A group (`p:grpSp`) is sorted inside and placed as one. Shapes with no position at
  all keep their file order after the placed ones.
- Blocks:
  - the title is one `Heading { level: 1 }` (subtitle: level 2), zone `Title`, its paragraphs
    joined by `\n`;
  - every other paragraph (`a:p`) is a block: `ListItem` when it has a bullet (`a:buChar`,
    `a:buAutoNum`, `a:buBlip`, or a body placeholder without `a:buNone`), else `Paragraph`;
    `a:br` is `\n`;
  - table cells (`a:tbl` in a `p:graphicFrame`): `TableCell { table, row, col }`, zone
    `Table`; `gridSpan` / `rowSpan` continuation cells (`hMerge`, `vMerge`) are left out; a
    cell's paragraphs are joined by `\n`;
  - speaker notes: the body placeholder of the slide's notes part, one `Note` per paragraph,
    after the slide's own blocks (the slide image, number, header and footer placeholders of
    the notes page are not notes).
- Left out: shapes with `cNvPr hidden="1"` (counted in `hidden_shapes`), pictures,
  connectors, charts and SmartArt (their text lives in parts of their own), the `mc:Fallback`
  copy of alternate content, `extLst` extensions, comments.

## Keys (`pptx.*`)

| Source | Keys |
|---|---|
| `docProps/core.xml` | `title`, `subject`, `creator`, `last_modified_by`, `keywords`, `created`, `modified` (Dates) |
| `docProps/app.xml` | `application`, `company` |
| structure | `slides`, `hidden_slides`, `notes_slides`, `tables`, `hidden_shapes` |
| package | `has_macros`, `images`, `embedded_objects`, `zip_entries`, `duplicate_entries` |

## Limits

Stopping short at any limit is an `Emitter::problem` (the record becomes partial, the document
is kept); notes about the file are warnings.

| What | Limit |
|---|---|
| zip entries | 20 000, read from the EOCD / ZIP64 record before the directory is parsed (`LimitExceeded`) |
| entry names | absolute, `..`, drive letter, backslash, control characters: ignored (warning) |
| inflated bytes | the bytes parts **actually** inflate to, against `max_decompressed` for the whole file; declared sizes never trusted. Slides are read before the properties |
| XML | DOCTYPE never expanded (no entity bombs, no external entities); one XML event at most 16 MiB; malformed XML ends only its part |
| slides | `max_pages` pages; at most 20 000 listed |
| text | `max_text_bytes`: every byte of page text is charged, separators included; at most 1 MiB per paragraph |
| items | 250 000 blocks per file; 100 000 shapes, paragraphs and cells per part; groups 64 deep (deeper ones flattened into their parent) |
| layouts | 1000 layouts and masters parsed for placeholder positions |
| deadline | checked every 2048 XML events (skipped subtrees included), per slide and while opening the zip |

Encrypted (OLE-wrapped) files return `Error::Encrypted`; a non-zip, or a zip without a
presentation part, returns `Error::Malformed`.

## Development

`cargo test -p quail-pptx`: every input is generated in code (`tests/common`).

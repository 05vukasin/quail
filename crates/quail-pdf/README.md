# quail-pdf

PDF extractor for quail: text with positions, page structure, and file-level PDF facts.
Pure Rust: [`hayro-syntax`](https://crates.io/crates/hayro-syntax) parses (xref repair, object
streams, RC4/AES with the empty user password), and
[`hayro-interpret`](https://crates.io/crates/hayro-interpret) runs content streams into a
text-collecting device. The same interpreter renders pages (crate `hayro`) when OCR needs pixels.

## Extractor

`PdfExtractor` has the id `pdf.extract@1` and handles `Format::Pdf`. It produces:

- `Document`: one `Page` (`PageKind::Physical`) per page up to `limits.max_pages`. Each page has:
  - `size` in points, after rotation.
  - `text` in reading order: blocks are ordered by a recursive XY-cut, so two columns read one column at a time.
  - `blocks`: paragraph groups of lines, each with a `Span` into the text and a `BBox` (points, top-left origin).
  - `zone`: `Header` or `Footer` by geometry (top or bottom 6 % band) or by repetition (the same text, digits ignored, in the top or bottom 15 % band on enough pages). Everything else is `Body`.
  - `text_source` (`Native` or `None`) and `needs_ocr`. A page needs OCR when it has no text but has images or heavy vector drawing, when it is mostly an image with little text, or when its text is garbage.
- Artifact `pdf.pages` (`PdfPages`): per-page image count, coverage, placements (pixel size and box), image-only flag and rotation. `quail-ocr` can use it to extract a page's scan directly instead of rendering the page.

## Keys (`pdf.*`)

| key | value |
|---|---|
| `pdf.version` | Text, e.g. `1.7` |
| `pdf.page_count` | Int (all pages, not only the extracted ones) |
| `pdf.encrypted` | Bool |
| `pdf.producer`, `pdf.creator`, `pdf.author`, `pdf.title`, `pdf.subject`, `pdf.keywords` | Text |
| `pdf.created`, `pdf.modified` | Date |
| `pdf.incremental_updates` | Int |
| `pdf.linearized` | Bool |
| `pdf.has_xmp` | Bool |
| `pdf.signature_fields` | Int |
| `pdf.signed` | Bool |
| `pdf.form_fields` | Int |
| `pdf.has_javascript` | Bool |
| `pdf.embedded_files` | Int |
| `pdf.fonts` | List of Text (deduplicated, subset tag stripped) |
| `pdf.image_only_pages` | Int |
| `pdf.pages_needing_ocr` | Int |

## Limits and robustness

- `max_bytes`: larger files fail with `LimitExceeded("max_bytes")` before any parsing.
- `max_pages`: later pages are skipped, with a warning.
- `max_decompressed`: before a page is interpreted, its content streams, form XObjects and font programs are inflated once by a bounded decoder and charged to the budget. Shared streams are charged once. Pages past the budget are skipped, with a warning.
- `max_text_bytes`: text is truncated at a block boundary, with a warning.
- `timeout`: `Emitter::check_deadline` runs before every page and gives a partial document.
- Encrypted with a user password: returns `Error::Encrypted` after emitting the facts it can read (version, encrypted, incremental updates, linearized).
- Panics in the parser or the interpreter are caught per document and per page. The page is then flagged `needs_ocr`, with a warning.

## Development

```sh
CARGO_BUILD_JOBS=2 cargo test -p quail-pdf
cargo bench -p quail-pdf --bench extract
# aggregate numbers only, safe on private corpora:
cargo run -p quail-pdf --example corpus --profile profiling -- <dir> [threads]
```

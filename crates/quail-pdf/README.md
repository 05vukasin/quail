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
  - `images` (core `PageImage`: page box and pixel size, first 64 per page) and `image_coverage`, so `Page::is_scan()` works.
- Artifact `raster` (core `Raster`), backed by `PdfRaster`, which implements `PageRaster` on hayro:
  - `render(page, dpi, limits)` gives 8-bit gray on white. It refuses pages over `max_render_pixels` (and over hayro's 65,535 px per side) before allocating, and charges content, fonts and images to `max_decompressed`.
  - `embedded_scan(page, limits)` decodes the page's single image as stored (gray or RGB; image masks come back as dark ink on white). It returns `None` when the page holds more than one image, or when the image is rotated or mirrored; rendering then gives the upright page.
  - `PdfRaster` holds only the file's shared `Blob` and re-parses per call, so it adds no memory beyond the file.

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

Early stops are reported with `Emitter::problem`, which keeps the partial document and makes the record Partial. Notes, such as a rejected garbage text layer, go through `warn`.

- `max_bytes`: larger files fail with `LimitExceeded("max_bytes")` before any parsing.
- `max_pages`: later pages are skipped (problem).
- `max_decompressed`:
  - Object streams are measured on the raw bytes before the file is opened, since hayro inflates them with no cap; a file over the budget fails with `LimitExceeded`.
  - Before a page is interpreted, its content streams, form XObjects and font programs are inflated once by a bounded decoder and charged to the budget. Shared streams are charged once. Pages past the budget are skipped (problem).
- `max_text_bytes`: text is truncated at a block boundary (problem).
- `timeout`: `Emitter::check_deadline` runs before every page and gives a partial document (problem).
- Encrypted with a user password: returns `Error::Encrypted` after emitting the facts it can read (version, encrypted, incremental updates, linearized).
- Panics in the parser, the interpreter or the renderer are caught per document, per page and per render. A failed page is flagged `needs_ocr` (problem).
- JavaScript is found through the catalog, a byte scan, and a walk over every object (object streams included).

## Development

```sh
CARGO_BUILD_JOBS=2 cargo test -p quail-pdf
cargo bench -p quail-pdf --bench extract
# aggregate numbers only, safe on private corpora:
cargo run -p quail-pdf --example corpus --profile profiling -- <dir> [threads]
```

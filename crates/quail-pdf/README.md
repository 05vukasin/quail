# quail-pdf

PDF extractor for quail: text with positions, page structure, and file-level PDF facts.
Pure Rust: [`hayro-syntax`](https://crates.io/crates/hayro-syntax) parses (xref repair, object
streams, RC4/AES with the empty user password), and
[`hayro-interpret`](https://crates.io/crates/hayro-interpret) runs content streams into a
text-collecting device. The same interpreter renders pages (crate `hayro`) when OCR needs pixels.

The hayro crates are vendored with hardening patches: a per-file decode budget, and image size
checks before any allocation. Allocation failure aborts the process, so blow-ups are refused
inside the decoders. See [`vendor/README.md`](../../vendor/README.md).

## Extractor

`PdfExtractor` has the id `pdf.extract@2` and handles `Format::Pdf`. It produces:

- `Document`: one `Page` (`PageKind::Physical`) per page up to `limits.max_pages`. Each page has:
  - `size` in points, after rotation.
  - `text` in reading order: blocks are ordered by a recursive XY-cut, so two columns read one column at a time.
  - `blocks`: paragraph groups of lines, each with a `Span` into the text and a `BBox` (points, top-left origin), and `lines`: the measured visual lines of the paragraph (`LineBox`: span within the block, box from the glyphs), in reading order.
  - `zone`: `Header` or `Footer` by geometry (top or bottom 6 % band) or by repetition (the same text, digits ignored, in the top or bottom 15 % band on enough pages). Everything else is `Body`.
  - `text_source` (`Native` or `None`) and `needs_ocr`. A page needs OCR when it has no text but has images or heavy vector drawing, when it is mostly an image with little text, or when `quail-text`'s `garbage_report` says `Garbage`. That covers unmapped and private-use glyphs, mojibake, implausible words, an unnatural vowel share, and Cyrillic read as Latin look-alikes (`OKBMPHM CnOPA3YM`).
    - A layer of unreadable glyphs (`broken_ratio()` or unmapped glyphs above 0.35) is dropped (`text_source = None`).
    - A look-alike layer keeps its `Native` text, whose digits and layout are right, with `needs_ocr = true`. `ocr.pages` then replaces it in the revised document (`OcrText::apply_to` replaces every page with `needs_ocr`). Without OCR, `text.lang` skips such pages, and the other text analyzers still read the layer.
  - `images` (core `PageImage`: page box and pixel size, first 64 per page) and `image_coverage`, so `Page::is_scan()` works.
- Artifact `raster` (core `Raster`), backed by `PdfRaster`, which implements `PageRaster` on hayro:
  - `render(page, dpi, limits)` gives 8-bit gray on white. Page indices match `Page::index`; indices at or past `max_pages` are refused. A render is also refused when:
    - the canvas is over `max_render_pixels` or hayro's 65,535 px per side;
    - a counting dry run hits the draw-call cap or `limits.timeout`;
    - a stream or image was refused by the decode budget (a render with missing images would mislead OCR).
    - The render itself runs under `hayro::render_limited`. Every callback and tile copy checks `limits.timeout` (`Timeout`). Decoded images (8 bytes per declared pixel), pattern tiles and soft-mask canvases are charged to a render byte budget of `max_decompressed` + 8 × `max_render_pixels` before they are allocated (`LimitExceeded("render memory")`).
    - After any limit hit or unwind the cached parse is dropped, because hayro caches refused streams.
    - The time budget is the file's: the extractor's `Raster` (`PdfRaster::with_no_scan(..).until(deadline)`) ends every call by the file's deadline, so extraction, page hashing and OCR of one file share one `timeout`.
    - vello's rasterization runs after recording and cannot be interrupted, so its work (fill areas, transparency layers) is charged while recording, against 256 full-canvas composites (`LimitExceeded("render work")`).
  - `embedded_scan(page, limits)` decodes the page's single image as stored (gray or RGB; image masks come back as dark ink on white). It applies only when the page draws exactly one image, upright, covering ≥ 98 % of the page, so OCR can map pixels to page points by scaling. Otherwise it returns `None` and rendering is the fallback. Pages the extractor interpreted completely and saw draw zero or several images answer `None` at once, without a parse or a decode (`PdfRaster::with_no_scan`).
  - `embedded_scan_reduced(page, max_side, limits)` is the same scan for thumbnails (page pHash). JPEG 2000 scans are decoded at the smallest resolution level whose longer side is still ≥ `max_side` (hayro's `target_dimension`), which cuts their decode time and memory by roughly the square of the scale. JPEG (DCT) scans are decoded with a scaled IDCT (1/8, 1/4 or 1/2; vendored `jpeg-decoder`) at the smallest scale whose output still covers the target in both dimensions: at `max_side` 128 a 300 dpi page decodes at 1/8, with about 1/25 of the heap and 35–50 % less time (entropy decoding cannot be skipped). Every other filter (JBIG2, CCITT, Flate), palette JPEG 2000, and CMYK/YCCK/12-bit/lossless JPEGs decode at full size, exactly as `embedded_scan`. The renderer passes its target size the same way, so JPEGs drawn at half their resolution or less also decode reduced when rendering.
  - `PdfRaster` shares the file's `Blob` and keeps one parsed document behind a mutex (OCR over N pages parses once). It drops that parse after 64 MiB decoded, and every call gets a fresh decode budget.

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
| `pdf.scanned_pages` | Int: scans needing OCR, meaning image-only pages and pages that are mostly an image whose text layer is too thin or garbage. A scan with a usable OCR layer is not counted |
| `pdf.is_scanned` | Bool: more than half of the extracted pages are `pdf.scanned_pages` (emitted for every PDF with a page) |

## Limits and robustness

Early stops are reported with `Emitter::problem`, which keeps the partial document and makes the record Partial. Notes, such as a rejected garbage text layer, go through `warn`.

- `max_bytes`: larger files fail with `LimitExceeded("max_bytes")` before any parsing.
- `max_pages`: later pages are skipped (problem). Page trees stop at 131,072 pages (problem).
- `max_decompressed` is one budget for every byte decoded from the file. That covers object and xref streams (also in the repair path, and encrypted ones after decryption), content (shared and repeated `/Contents` included), fonts, forms at any depth, Type3 procedures, patterns, soft masks, annotation appearances and images.
  - Every filter layer of every chain (Flate, LZW, ASCII85, ASCIIHex, RunLength) stops at what is left, while decoding.
  - Opening fails with `LimitExceeded` if the object graph alone exceeds the budget.
  - A refused stream on a page is a problem; extraction stops once less than 1 MiB is left.
- Memory: hayro caches each page's decoded content for the life of a parse, so the document is re-parsed once 64 MiB were decoded since it was opened. Peak memory stays near one page plus that margin, and never above the budget.
- `max_render_pixels` caps every decoded image: `/Width` × `/Height`, and the sizes in JPEG SOF, JPEG 2000 SIZ, JBIG2 page-information and CCITT headers. All of them are checked before the decoder allocates. JBIG2 bitmaps are additionally capped at 2^28 pixels.
- `max_text_bytes`: text is truncated at a block boundary (problem). Text rejected as garbage does not count against it.
  - Per page, glyph text lookups stop at `max_text_bytes` too: every `ToUnicode` string is charged in full (the lookup allocates it), and a glyph keeps at most 32 chars of it. Later glyphs are counted as dropped without being looked up, and so are glyphs past 400k per page.
- `timeout`: `Emitter::check_deadline` runs before every page, after interpretation, after layout and before zoning.
  - Inside a page, the device checks the deadline every 1,024 callbacks and caps a page at 4M draw calls or 250k clips/groups (every form `Do` pushes a clip, so form fan-out of 2^40 in 8 KB stops in about 1.5 s). Every glyph of a text run is one draw call, so one huge `Tj` meets the cap and the deadline too.
  - Either way it unwinds out of hayro with a sentinel caught per page (problem). Layout is linear apart from sorting (400k glyphs in well under a second); at most 64 blocks stay open for continuation, so hundreds of thousands of segments on one line stay linear too.
- Encrypted with a user password: returns `Error::Encrypted` after emitting the facts it can read (version, encrypted, incremental updates, linearized).
- Panics in the parser, the interpreter or the renderer are caught per document, per page and per render. A failed page is flagged `needs_ocr` (problem).
- JavaScript is found through the catalog, a byte scan, and a walk over every object (object streams included).
- Form-field and embedded-file name-tree walks have a work budget of 200k units (a node visited, a `/Kids` entry resolved, a name counted) and check the deadline every 1,024 units, so shared nodes with huge arrays cannot multiply the work.

## Development

```sh
CARGO_BUILD_JOBS=2 cargo test -p quail-pdf
cargo bench -p quail-pdf --bench extract
# aggregate numbers only, safe on private corpora:
cargo run -p quail-pdf --example corpus --profile profiling -- <dir> [threads]
```

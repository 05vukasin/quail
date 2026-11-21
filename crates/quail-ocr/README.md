# quail-ocr

OCR for pages that have no usable text layer. Namespace `ocr.*`.

One analyzer: **`ocr.pages@1`** (`OcrPages`), profile `Deep`. It requires the `document` and `raster`
artifacts and provides `ocr_text` (`OcrText`) and `revised_document`. It uses only core types. It reads the pages that have
`needs_ocr` and gets their pixels from the `Raster` artifact (`PageRaster`), so it never depends on
an extractor crate.

## Per page

1. **Pixels.** If `Page::is_scan()`, the embedded scan is decoded through `PageRaster::embedded_scan`
   and shrunk to at most 300 DPI. Otherwise the page is rendered at 200 DPI. The DPI is lowered when
   needed so the image fits `Limits::max_render_pixels` and has a long side of at most 4000 px.
2. **Recognition.** An engine from the pool reads the image. Results are cached by BLAKE3 of the
   pixels, the engine fingerprint and the hints.
3. **Retry.** If the mean confidence is below 0.80, the page is rendered again at 300 DPI. The
   better of the two readings is kept.
4. **Layout.** Lines with confidence below 0.5 are dropped. The rest are grouped into rows, and the
   boxes are mapped from pixels to page points. The result is laid out like a core `Page`: text in
   reading order, one `Paragraph` block per row, each with its zone (header and footer bands by
   position) and box, plus lines and words with their confidence and script.

`Emitter::check_deadline` runs before each page and before each retry. The caps are
`OcrConfig::max_pages` (200) and the file's `max_text_bytes`, which OCR text shares with the native
text. A page that fails is recorded as a problem, and the remaining pages still run.

## Keys

| key | value | what |
|---|---|---|
| `ocr.pages` | Int | pages OCR'd |
| `ocr.chars` | Int | characters read (spaces excluded) |
| `ocr.mean_conf` | Float | mean recognition confidence, weighted by characters |
| `ocr.script` | Text | dominant script: `latin`, `cyrillic`, `mixed`, `other` |
| `ocr.page` | Record | one per OCR'd page, with fields `page`, `script`, `mean_conf`, `chars`, `lines`, `dpi`, `source` (`embedded_scan` or `render`) and `rotation`. `conf` is the page's mean confidence; `Evidence.page` is set |

## Backends

- **`paddle`** (cargo feature, off by default): [PP-OCRv6](https://www.paddleocr.ai/) small,
  through [`oar-ocr`](https://crates.io/crates/oar-ocr) `=0.11.0` on ONNX Runtime 1.31
  (`ort =2.0.0-rc.15`, statically linked prebuilt binaries for Linux x64/arm64, macOS arm64 and
  Windows). It includes PP-LCNet page orientation (0/90/180/270) and text-line orientation (0/180).
  Deskew is optional (`PaddleConfig::deskew_degrees`). PP-OCRv6 cannot read Cyrillic, so the
  Cyrillic PP-OCRv5 recognizer re-reads a sample of the longest lines. A page where at least half
  of the sample reads as confident Cyrillic is read in both scripts, line by line. On a page where
  only some lines do, only the weak lines are re-read. Each engine owns its ONNX Runtime sessions
  (one intra-op thread, no spinning), and `EnginePool` keeps one engine per worker thread.
- **No backend** (the default build, pure Rust): for every file that has a page needing OCR, the
  analyzer reports the problem `ocr backend not compiled`. For other files it reports nothing.

`OcrEngine` is the engine trait. Tests drive the analyzer with a scripted engine and an
in-memory raster (`tests/analyzer.rs`), so CI covers everything except the native backend.

## Models

Models are not in the repository. `models::MANIFEST` lists each file with its URL, upstream
version, size, SHA-256 and license (all Apache-2.0, PaddleOCR). The backend checks every
SHA-256 before it loads anything. Models are looked up in `$QUAIL_OCR_MODELS`, or else in the
user cache directory under `quail/ocr-models`.

```sh
cargo run -p quail-ocr --features fetch --example fetch_models -- [DIR]
```

| file | MiB |
|---|---|
| `pp-ocrv6_small_det.onnx` | 9.4 |
| `pp-ocrv6_small_rec.onnx` | 20.2 |
| `cyrillic_pp-ocrv5_mobile_rec.onnx` | 7.7 |
| `pp-lcnet_x1_0_doc_ori.onnx` | 6.5 |
| `pp-lcnet_x0_25_textline_ori.onnx` | 1.0 |
| dictionaries (2) | 0.1 |

## Getting OCR text to text analyzers

The core `Document` artifact is write-once. `OcrText::apply_to(&Document)` returns the document with
the OCR text and blocks filled into the pages that had no native text (`text_source = Ocr`), and
the analyzer provides it as the core `RevisedDocument` artifact (`revised_document`) whenever at
least one page was read. Text analyzers (`text.*`, `signal.*`, `ref.*`, `party.*`, `clause.*`,
`item.*`, `sketch.*`, `summary.*`) declare `after(&["revised_document"])` and read
`Ctx::document()`, which prefers it over the extracted document.

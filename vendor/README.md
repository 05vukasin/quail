# Vendored, patched crates

These are copies of published crates with small patches: hardening for hostile PDFs (hayro),
speed (lingua) and a thread-free, unsafe-free JPEG decoder for reduced decodes (jpeg-decoder).
The root
`Cargo.toml` routes the crates.io versions here through `[patch.crates-io]`. They are not
workspace members (`exclude = ["vendor"]`), so workspace lints and `cargo fmt` do not touch
them.

Every change is marked with a `// quail:` comment. To see the full diff, compare against the
pristine sources in `~/.cargo/registry/src/*/<crate>-<version>/`.

| crate | version | upstream |
|---|---|---|
| `hayro-syntax` | 0.9.0 | https://github.com/LaurenzV/hayro |
| `hayro-interpret` | 0.9.0 | https://github.com/LaurenzV/hayro |
| `hayro-jbig2` | 0.4.0 | https://github.com/LaurenzV/hayro |
| `hayro` | 0.9.0 | https://github.com/LaurenzV/hayro |
| `lingua` | 1.8.0 | https://github.com/pemistahl/lingua-rs |
| `jpeg-decoder` | 0.3.2 | https://github.com/image-rs/jpeg-decoder |

`quail-pdf` pins `hayro`, `hayro-syntax` and `hayro-interpret` at `=0.9.0`, and `quail-text`
pins `lingua` at `=1.8.0`, and the vendored `hayro-syntax` pins `jpeg-decoder` at `=0.3.2`
(a path dependency, `../jpeg-decoder`, so it needs no `[patch]` entry), so that the patches always apply to the version they were written
for.

`lingua` is trimmed to the library: no accuracy reports, images, tests, benchmarks or binding
metadata (`Cargo.toml` builds an `rlib` only).

## Why

On allocation failure, Rust aborts the process: `catch_unwind` cannot catch it. A 747-byte
PDF could make hayro allocate 4 GiB, and an 8 KB one could object-stream-bomb or fan out
forever. Every blow-up therefore has to be refused before allocating, inside the decoders.
The crate using them (`quail-pdf`) cannot do that from outside.

## Patches

### hayro-syntax 0.9.0

- **`budget.rs` (new):** `DecodeBudget`, a per-file budget made of decoded bytes in total
  (`max_decompressed`) and pixels per image (`max_render_pixels`). It also has a `limit_hit`
  flag, so the caller can report what was refused.
  - `PdfData::with_budget` attaches it.
  - `XRef::decode_budget` and `Stream::max_image_pixels` / `Stream::note_limit_hit` read it.
  - Streams without document context fall back to fixed defaults: 64 MiB per stream and 64 M
    pixels. Inline images get the file's budget from the interpreter instead (see
    hayro-interpret).
  - `reserve_capped` / `read_capped`: decoders grow their output with `try_reserve_exact`,
    by half at a time but never past the cap. So a decode capped at the budget never holds
    2 × the budget (as `read_to_end` doubling did), and an allocation failure becomes a
    refused decode instead of an abort.
- **`object/stream.rs`:** every filter layer of every decode (object streams, xref streams,
  content, fonts, forms, Type3, patterns, soft masks, annotations, images) is capped by what
  is left of the budget and charged to it. `decoded_data_with_budget` passes the budget
  explicitly while the xref is still being built, because contexts are dummies then.
- **`filter/mod.rs`:** `Filter::apply` takes the limits.
  - Data filters (Flate, LZW, ASCIIHex, ASCII85, RunLength) stop just past the cap; a longer
    result is refused and flagged.
  - Image filters (DCT, CCITT, JBIG2, JPX) must fit the pixel and byte caps, as declared by
    the image dictionary, before their decoders run (JPX too since the codestream-box fix).
- **`filter/lzw_flate.rs`, `filter/run_length.rs`, `filter/ascii_85.rs`:** output caps and
  capped, fallible growth inside the decode loops: flate2 read in 64 KiB chunks, the pdf.js
  fallback, LZW, RunLength, and ASCII85 `z`.
- **`filter/dct.rs`:** `header_fits` checks the frame size in the JPEG SOF marker. A stream
  without an SOF is refused as corrupt (not as a limit).
- **`filter/dct.rs` (reduced decode):** when the caller passes a `target_dimension` at most
  half the JPEG frame, the image is decoded with jpeg-decoder's scaled IDCT at 1/8, 1/4 or
  1/2: the smallest scale whose output still covers the target in both dimensions
  (`reduced_scale`). zune-jpeg has no scaled IDCT. Only gray and three-component 8-bit
  DCT images take this path, with the color transform zune-jpeg's header reading and the PDF's
  `/ColorTransform` chose (YCbCr → RGB, RGB, or untransformed); CMYK, YCCK, multi-band,
  12-bit and lossless JPEGs, frames under 16 px on a side, and any jpeg-decoder error fall
  back to the unchanged full zune-jpeg decode. The decoding buffer is capped at the reduced
  output size. The pixel and byte caps from `header_fits` still run first, on the full frame.
  `Cargo.toml`: `jpeg-decoder` (`std` feature).
- **`filter/jpx.rs`:** `header_fits` checks the image size in the JPEG 2000 SIZ marker of
  every codestream the decoder will parse: a raw codestream (SOC + SIZ at the start), or every
  `jp2c` box of a JP2 file, walking the top-level boxes with the decoder's lenient length
  rules. A SIZ elsewhere (a decoy in a `uuid` or `xml` box) is not what gets decoded. Data that
  is neither format, or a `jp2c` box without SOC + SIZ, is refused as corrupt (not as a
  limit).
- **`filter/jbig2.rs`:** the JBIG2 page size (from the data, not the dictionary) must fit
  before the 8-bit output is allocated.
- **`filter/ccitt.rs`:** columns × rows must fit; when the row count is unknown, rows stop at
  the pixel cap, and dropping rows sets the limit flag.
- **`xref.rs`:** the budget is threaded through xref-stream parsing and through object-stream
  decoding in the repair path.
- **`xref.rs` (repair scan):** an object header is validated, and a dictionary probed, only up
  to the next `endobj` (`EndobjCursor`, found walking forward once). An unclosed dictionary
  repeated 80 000 times made every header skip the following objects as nested garbage, up
  to the nesting cap: quadratic, 10 s for 4 MB.
- **`object/stream.rs`:** `Stream::encoded_len`, the stored data length without a copy.
- **`content/mod.rs`:** an inline image's end is probed (`starts_with_content` on the content
  after a candidate `EI`) at most `MAX_CONTENT_PROBES` (64) times; then the candidate is taken
  as the end. 20 000 candidates inside one unclosed 900 KB string each parsed to the end of the
  stream: quadratic, 20 s.
- **`filter/lzw_flate.rs` (predictors):** row lengths are computed in checked `usize` (the `u8`
  product wrapped). `/Colors 0` or `/Columns 0` is refused (it divided by zero), and a row
  longer than the data decodes nothing instead of sizing a gigabyte zero row.
- **`page.rs`:**
  - `MAX_PAGES` (131 072). Page trees stop there. Inline kid dictionaries are not
    deduplicated, so a small file could otherwise create millions of `Page` structs.
  - `page_stream` charges the concatenation of a `/Contents` array (a second copy of every
    part) to the budget, grows it within what is left, and stops at the first part that does
    not fit.

### hayro-interpret 0.9.0

- **`x_object/decode/mod.rs`:** image and mask sample buffers are sized from `/Width` ×
  `/Height`. Images above the file's pixel cap are refused (and flagged) before anything is
  allocated, whatever filter they use or how small their data is.
- **`font/type3.rs`, `font/mod.rs`, `context.rs`:** Type3 glyph procedures nested deeper than
  4 are not drawn. A glyph showing text in its own font recursed until the stack overflowed (a
  process abort). The Type3 depth is tracked apart from form nesting.
- **`device.rs`, `interpret/mod.rs`, `font/type3.rs`, `pattern.rs`:** `Device::on_operator`, a
  default no-op hook called before every content-stream operator (forwarded by wrapper
  devices), so a device can check a deadline between operators.
- **`interpret/text.rs`, `interpret/mod.rs`:** a glyph run reaches the device in pieces of at
  most `MAX_RUN_GLYPHS` (16 384). One `Tj` of 8 M characters collected 8 M positioned glyphs
  (~180 bytes each) first: 1.4 GiB, an allocation-failure abort. The text state advances as
  before and clip-mode outlines still accumulate in it, so what is drawn does not change.
- **`device.rs`, `interpret/mod.rs`, `font/type3.rs`, `pattern.rs`:** `Device::on_x_object`,
  a default no-op hook called before every `Do` with the bytes hayro parses again for it (the
  XObject's dictionary and stored data), so a device can bound a page's form work.
- **`x_object/image.rs`, `interpret/mod.rs`, `x_object/decode/image.rs`:** an image carries the
  file's budget. Inline images (and soft masks) get it from the interpreter context, since
  their streams have none, so they obey `max_render_pixels` too.

### hayro-jbig2 0.4.0

- **`bitmap.rs`:** `MAX_PIXELS` (2^28, 32 MiB of bits) caps every bitmap, instead of
  `MAX_DIMENSION²` (65535², 512 MiB). `capacity_hint` clamps capacity hints taken from counts
  in the file.
- **`gray_scale.rs`, `decode/halftone.rs`:** caps before allocating gray-scale value arrays
  (counted at 4 bytes per pixel) and halftone skip arrays. A halftone grid with more cells
  than max(region pixels, 2^16) is refused; a 1 KB file asked for a 16384² grid (a 1 GiB array
  and 2^28-step loops).
- **`decode/{pattern,symbol,text}.rs`, `segment.rs`:** clamped `Vec::with_capacity` hints. A
  symbol count of 2^32 no longer reserves 2^32 elements up front.

### hayro 0.9.0 (renderer)

- **`limits.rs` (new):** `RenderLimits` (deadline, byte budget, callback cap, rasterization
  work budget) and the `RenderAbort` payload.
  - vello rasterizes the recording after hayro is done and cannot be interrupted. So its work
    is charged while recording: every fill by its device-space area, every transparency group
    by the canvas area. The deadline is checked every 1024 operators and once more before
    rasterization starts.
  - Recording memory is charged too: 1 KiB per glyph fill, 512 bytes per path fill, 64 KiB per
    gradient (a lookup table each), and the sampled texture of a non-native shading.
- **`lib.rs`:**
  - `render_limited` takes a `RenderLimits` (`render` and `render_into` are unchanged).
  - Every device callback ticks the limits.
  - `child_context` charges its canvas (8 bytes per pixel) before allocating it.
  - Over a limit, the render unwinds with `RenderAbort` through `resume_unwind`.
- **`tiling.rs`:** each tile copy ticks (nested patterns multiply copies), and the tile pixmap
  is charged.
- **`mask.rs`:** soft-mask canvases are charged (12 bytes per pixel) before allocating.
- **`image.rs`:** every image draw is charged at 8 bytes per declared pixel *before* decoding,
  so repeated draws of a big image, or many distinct images, run into the budget instead of
  memory. The charge saturates (it wrapped for `/Width`, `/Height` near `u32::MAX`).
- **`paint.rs`:** gradient and sampled-shading charges.
- **`glyph_cache.rs`:** glyph positions are summed in `f64` (the `i32` sum overflowed).

### jpeg-decoder 0.3.2 (scaled IDCT for reduced DCT decodes)

Used by hayro-syntax's `filter/dct.rs` only for reduced decodes (see above); full-size decodes
stay on zune-jpeg.

- **`worker/mod.rs`:** the decoder always uses its same-thread (`Immediate`) worker. Upstream
  spawns a `std::thread` per color component (or uses rayon) for every image above 128 × 128
  output pixels, from inside quail's own worker pool; `worker/multithreaded.rs` and
  `worker/rayon.rs` are removed.
- **`lib.rs`, `decoder.rs`, `idct.rs`:** `#![forbid(unsafe_code)]`. The SIMD `arch` module
  (SSSE3/NEON/wasm IDCT and color conversion, the crate's only unsafe code) is removed; the
  portable paths upstream builds with `platform_independent` are the only ones.
- **`Cargo.toml`:** library only, no features, no dependencies (no rayon); tests, benches and
  examples are not vendored.

### lingua 1.8.0 (speed; output unchanged)

quail's `text.lang` spent most of its time inside `compute_language_confidence_values`. The
patches keep every probability bit-for-bit and change only how it is looked up:

- **`fast.rs` (new):** `NgramTable`, every 1-, 2- and 3-gram of the detector's probability
  models in one hash map from the packed n-gram (21 bits per character) to a dense row with
  one slot per language: the model's exact `f64` bits, or empty. It is filled from the
  embedded fst models once, in `LanguageDetector::from`, walking only keys of at most three
  characters (an fst automaton prunes longer ones). Built only for detectors of at most 16
  languages; larger ones keep upstream's lookups.
- **`detector.rs`:**
  - `compute_language_probabilities` and `count_unigrams` use the table: one probe per
    n-gram (and backoff level) for all languages, instead of a `DashMap` probe plus an
    `fst::Map::get` per language. Per language, the first backoff level found is added, in
    n-gram order, exactly as upstream; 4- and 5-grams (texts under 120 characters) still go
    to the fst.
  - **Deterministic sums.** Upstream summed over a `HashSet` of n-grams and a `HashMap` of
    languages, whose iteration order is random per call, so the last bits of the `f64`
    confidences changed from call to call. The softmax denominator is now summed in language
    order, and the very-long-text fallback breaks ties in language order.
  - `filter_languages_by_rules` counts the words containing each character once, then adds
    those counts per mapping entry, instead of searching every word for every character of
    every entry. Same counts.
- **`model.rs`:** `create_lower_order_ngrams` returns the distinct n-grams in order of first
  occurrence (deterministic; see above) and slices each word once instead of walking it from
  the start per n-gram.
- **`alphabet.rs`:** `CharSet` is a sorted list of merged code point ranges with a binary
  search, instead of a `HashSet` of every code point (SipHash per character, ~100 000 entries
  for Han). Same characters match.
- **`Cargo.toml`:** `rlib` only, no binary, test or bench targets.

Checked: `quail scan` records (minus `timings`) identical before and after on the private
corpus and CUAD, at `-j1` and `-j4`.

## Updating

1. Copy the new upstream sources over the directory.
2. Re-apply every `// quail:` change.
3. Run `cargo test -p quail-pdf`. Its `tests/hostile.rs` regenerates the hostile inputs and
   fails if any of them gets through; `tests/dct_reduced.rs` checks the scaled JPEG decode.
4. For lingua, run `cargo test -p quail-text` and compare `quail scan` records (minus
   `timings`) on a corpus before and after: they must be identical.

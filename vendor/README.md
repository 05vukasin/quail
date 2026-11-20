# Vendored, patched crates

These are copies of published crates with small hardening patches for hostile PDFs. The root
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

`quail-pdf` pins `hayro`, `hayro-syntax` and `hayro-interpret` at `=0.9.0` so that the patches
always apply to the version they were written for.

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
  - Streams without document context (inline images) fall back to fixed defaults: 64 MiB per
    stream and 64 M pixels.
- **`object/stream.rs`:** every filter layer of every decode (object streams, xref streams,
  content, fonts, forms, Type3, patterns, soft masks, annotations, images) is capped by what
  is left of the budget and charged to it. `decoded_data_with_budget` passes the budget
  explicitly while the xref is still being built, because contexts are dummies then.
- **`filter/mod.rs`:** `Filter::apply` takes the limits.
  - Data filters (Flate, LZW, ASCIIHex, ASCII85, RunLength) stop just past the cap; a longer
    result is refused and flagged.
  - Image filters (DCT, CCITT, JBIG2, JPX) must fit the pixel and byte caps, as declared by
    the image dictionary, before their decoders run.
- **`filter/lzw_flate.rs`, `filter/run_length.rs`:** output caps inside the decode loops (the
  flate2 path via `Read::take`, plus the pdf.js fallback, LZW and RunLength).
- **`filter/dct.rs`:** `header_fits` checks the frame size in the JPEG SOF marker.
- **`filter/jpx.rs`:** `header_fits` checks the image size in the JPEG 2000 SIZ marker. A
  stream without one is refused.
- **`filter/jbig2.rs`:** the JBIG2 page size (from the data, not the dictionary) must fit
  before the 8-bit output is allocated.
- **`filter/ccitt.rs`:** columns × rows must fit; when the row count is unknown, rows stop at
  the pixel cap.
- **`xref.rs`:** the budget is threaded through xref-stream parsing and through object-stream
  decoding in the repair path.
- **`page.rs`:** `MAX_PAGES` (131 072). Page trees stop there. Inline kid dictionaries are not
  deduplicated, so a small file could otherwise create millions of `Page` structs.

### hayro-interpret 0.9.0

- **`x_object/decode/mod.rs`:** image and mask sample buffers are sized from `/Width` ×
  `/Height`. Images above the file's pixel cap are refused (and flagged) before anything is
  allocated, whatever filter they use or how small their data is.

### hayro-jbig2 0.4.0

- **`bitmap.rs`:** `MAX_PIXELS` (2^28, 32 MiB of bits) caps every bitmap, instead of
  `MAX_DIMENSION²` (65535², 512 MiB). `capacity_hint` clamps capacity hints taken from counts
  in the file.
- **`gray_scale.rs`, `decode/halftone.rs`:** the same cap before allocating gray-scale value
  arrays and halftone skip arrays.
- **`decode/{pattern,symbol,text}.rs`, `segment.rs`:** clamped `Vec::with_capacity` hints. A
  symbol count of 2^32 no longer reserves 2^32 elements up front.

## Updating

1. Copy the new upstream sources over the directory.
2. Re-apply every `// quail:` change.
3. Run `cargo test -p quail-pdf`. Its `tests/hostile.rs` regenerates the hostile inputs and
   fails if any of them gets through.

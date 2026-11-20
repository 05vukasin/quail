# quail-engine

Runs extractors and analyzers over files: format sniffing, analyzer ordering, limits, panic
containment and bounded-memory parallelism. It knows no concrete extractor or analyzer; the
caller registers them on an `EngineBuilder`.

## Keys emitted

| key             | value | meaning                                                       |
|-----------------|-------|---------------------------------------------------------------|
| `engine.format` | text  | `pdf`, `docx`, `zip`, `image`, `text`, `unknown`              |
| `engine.mime`   | text  | most specific MIME type known from the bytes                  |

Both come from the step `engine.sniff@1`, the first entry of every record's `steps`.

## Sniffing

Bytes only, never the file name; bounded work, no inflation.

- `%PDF-` within the first 1024 bytes: PDF.
- ZIP (`PK\x03\x04`): the end-of-central-directory record and at most 4096 directory entries
  (local headers for truncated archives) are scanned for `word/document.xml` (DOCX) and
  `[Content_Types].xml`. XLSX/PPTX get their MIME type but stay `zip`.
- PNG, JPEG, GIF, TIFF, WebP, JPEG 2000, HEIC/AVIF, BMP: image.
- First 8 KiB without NUL and with under 1% control bytes: text (`charset=utf-8` when valid
  UTF-8; UTF-16 by BOM).
- Everything else (OLE2, gzip, 7z, RAR, RTF, ...): unknown, with a MIME type when known.

## Building

`EngineBuilder::new().profile(..).limits(..).extractor(..).analyzer(..).build()` checks,
once: one extractor per format at the top priority (`extractor_with_priority`), unique
analyzer ids, every `requires` provided by an active analyzer or built in (`document`), no
cycles. The topological order (ties: registration order) is fixed at build time. Analyzers
above the profile are left out (`Engine::inactive`). `quiet_panics(true)` keeps contained
panics off stderr.

## Per file (`Engine::process`)

1. Sniff. 2. Over `max_bytes`: no extraction, record `partial`, analyzers that do not
require `document` still run. 3. The format's extractor under `catch_unwind`; its document
becomes the `document` artifact (an error or panic marks the record `failed`). 4. Analyzers
in plan order, under `catch_unwind`, if `applies` and their artifacts exist. A failing step
loses its facts and artifacts and keeps its warnings; warnings and errors become `partial`
problems. Each step that ran is in `steps` with its `timings`.

Deadlines (`now + Limits::timeout`) are cooperative: passed through
`Emitter::with_deadline`, checked between steps. A native library that hangs cannot be
preempted inside a thread; process isolation is the service's job.

## Many files (`Engine::run`)

`threads` workers (default: available parallelism minus one), one writer thread for the
sink, `crossbeam-channel` between them. At most `max_in_flight` files exist between the source
pull and the sink write, so memory is bounded by that many blobs. `ordered: true` writes in
source order through a reorder buffer bounded the same way. Returns `RunStats` (files, bytes,
ok/partial/failed, source and sink errors, elapsed, files/s, MB/s).

## Overhead

`cargo bench -p quail-engine`: about 0.26 µs per file for sniffing alone and 1.4 µs per file
with a no-op extractor and 8 no-op analyzers (x86_64 laptop, `profiling` profile).

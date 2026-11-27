# quail-engine

Runs extractors and analyzers over files: format sniffing, analyzer ordering, limits, panic
containment and bounded-memory parallelism. It knows no concrete extractor or analyzer; the
caller registers them on an `EngineBuilder`.

## Keys emitted

| key             | value | meaning                                                       |
|-----------------|-------|---------------------------------------------------------------|
| `engine.format` | text  | `pdf`, `docx`, `pptx`, `html`, `zip`, `image`, `text`, `unknown` |
| `engine.mime`   | text  | most specific MIME type known from the bytes                  |
| `engine.duplicate_of` | text | `dedup_by_content` only: locator of the earlier file with the same bytes whose record this one reuses |

`engine.format` and `engine.mime` come from the step `engine.sniff@3`, the first entry of
every record's `steps`; `engine.duplicate_of` from `engine.dedup@1`, the last.

## Sniffing

Bytes only, never the file name; bounded work, no inflation, never panics (and runs under
`catch_unwind` anyway).

- `%PDF-d.d` within the first 1024 bytes, preceded only by whitespace or binary junk (a
  prefix that reads as text, e.g. a note or source code quoting `%PDF-1.7`, means text).
- ZIP (`PK\x03\x04`): up to 8 end-of-central-directory candidates from the end (a comment
  can hold a fake one), then at most 20 000 directory entries (the DOCX extractor's entry
  cap), looking for `word/document.xml` (DOCX), `ppt/presentation.xml` (PPTX) and
  `[Content_Types].xml`, ignoring ASCII case. With `[Content_Types].xml` but a main part
  under another name, an XML part directly under `word/` means DOCX (the extractor finds the
  main part through `_rels/.rels`). A garbage or ZIP64 directory falls back to walking local
  headers. XLSX gets its MIME type but stays `zip`.
- PNG, JPEG, GIF, TIFF, WebP, JPEG 2000, HEIC/AVIF, BMP: image.
- First 8 KiB without NUL and with under 1% control bytes: text (`charset=utf-8` when valid
  UTF-8; UTF-16 by BOM). UTF-16 without a BOM is text too (`charset=utf-16le`/`be`): a third
  of the 2-byte units with a zero byte on one side, almost none on the other, and those
  units printable.
- Text is `html` (`text/html`, same charset rules) when its first 1024 bytes, after a BOM,
  whitespace, an XML declaration and comments, start with `<!DOCTYPE html` or an HTML tag:
  `html`, `head`, `body`, `title`, `meta`, `div`, `p`, `table`, `font`, `center`, `h1`-`h6`,
  `br`, `hr` (any case, namespace prefix allowed). EDGAR filings (`<HTML>`, inline XBRL after
  `<?xml ...?>`) match; other XML and prose that mentions tags stay `text`.
- Everything else (OLE2, gzip, 7z, RAR, RTF, ...): unknown, with a MIME type when known.

## Building

`EngineBuilder::new().profile(..).limits(..).extractor(..).analyzer(..).build()` checks,
once: one extractor per format at the top priority (`extractor_with_priority`), unique
analyzer names (whatever the version), every `requires` provided by an active analyzer or
built in (`document`), no cycles. Requirements match `Artifact::NAME`, so a name must belong
to one artifact type only. The topological order (ties: registration order) is fixed at
build time. Analyzers above the profile are left out (`Engine::inactive`).
`quiet_panics(true)` keeps contained panics off stderr.

## Status of a record (`Engine::process`)

| what happened                                                          | status    |
|------------------------------------------------------------------------|-----------|
| every step that applied succeeded (warnings or not)                    | `ok`      |
| file over `max_bytes`: no extraction, generic analyzers still run      | `partial` |
| extractor returned `Encrypted` or `LimitExceeded` (its facts kept)     | `partial` |
| extractor returned another error, panicked or timed out (facts kept)   | `failed`  |
| an analyzer errored, panicked or timed out (its facts dropped)         | `partial` |
| an applicable analyzer skipped because a provider of its input failed  | `partial` |
| the deadline passed before all analyzers ran                           | `partial` |

Warnings never change the status: they go to `FileRecord::warnings`, prefixed with the step
id. An analyzer whose input was simply never produced (no extractor for the format, the
provider did not apply) is skipped silently, so a text or PNG file with a
document-requiring analyzer is `ok`.

Deadlines (`now + Limits::timeout`) are cooperative: passed through
`Emitter::with_deadline`, checked between steps. A native library that hangs cannot be
preempted inside a thread; process isolation is the service's job.

## Many files (`Engine::run`)

`RunOptions`:

| field                 | default                 | effect                                              |
|-----------------------|-------------------------|-----------------------------------------------------|
| `threads`             | parallelism − 1 (≥ 1)   | workers (0 means 1)                                 |
| `max_in_flight`       | 2 × threads             | files between source pull and sink write (0 means 1) |
| `max_in_flight_bytes` | 1 GiB                   | no dispatch past this; one bigger file goes alone   |
| `ordered`             | false                   | source order, reorder buffer ≤ `max_in_flight`      |
| `stop_on_sink_error`  | true                    | first sink error stops pulling                      |
| `cancel`              | none                    | `Arc<AtomicBool>`: stop pulling, drain, return      |
| `stack_size`          | 32 MiB                  | worker stacks: deep recursion on hostile nesting would abort the process |
| `on_artifacts`        | none                    | `Arc<dyn Fn(&FileRecord, &Artifacts)>` per processed file, on the worker (see below) |
| `dedup_by_content`    | false                   | reuse the record of an earlier file with the same bytes (see below) |
| `dedup_max_records`   | 10 000                  | records kept for dedup (LRU)                        |
| `dedup_max_bytes`     | 64 MiB                  | approximate heap bytes kept for dedup (LRU)         |

Workers are std threads; one writer thread owns the sink; `crossbeam-channel` in between.
The reader learns of freed slots from the writer and wakes every 20 ms, so it notices
cancellation, a sink stop or a dead writer. A worker that dies still sends a `Failed`
record, so ordered mode never waits for a lost item. In ordered mode one slow file holds
back everything after it; waits over 2 × `limits.timeout` are counted in `RunStats::stalls`.

Source errors skip the item; an error with a locator (`Error::at`) gets a `Failed` record
(format `unknown`, size 0). A panicking source counts as one error item. Sources should cap
file size themselves (`quail-io`'s `max_file_bytes`, set from `limits.max_bytes`), since a
pulled file is in memory before the byte budget can refuse it.

`RunStats`: files, bytes, ok/partial/failed, source errors / error records / panics, sink
errors and the first one, unwritten, stopped_on_sink_error, cancelled, stalls,
writer_panicked, hook panics and the first one, duplicates and dedup evictions, elapsed,
`files_per_sec()`, `mb_per_sec()`.

### Content dedup (`dedup_by_content`, `quail scan --dedup`)

The reader hashes each file (BLAKE3, the `blake3` crate built like quail-hash's) in source
order before handing it to a worker, so the first file of a content is always the first in
the source, at any thread count. It is processed normally and its finished record kept
in a run-scoped LRU cache (`dedup_max_records`, `dedup_max_bytes`). A later file with the
same bytes gets a copy of that record (facts, steps, format, pages, status, warnings) with its
own locator, plus `engine.duplicate_of` and the step `engine.dedup@1`; its only timing is
`engine.dedup` (hash and lookup). No step runs for it and `on_artifacts` is not called.

- A copy that arrives while the first is still in flight waits for it (condition variable),
  at most 2 × `limits.timeout`, then is processed normally: no deadlock, whatever the
  thread count or ordering.
- A first copy that ends `Failed`, panics, or does not fit the cache is not reused; the next
  copy is processed (and may become the cached one).
- Ordered mode works unchanged: every file keeps its sequence number.
- Records are assumed to depend on the bytes only; an analyzer reading `Ctx::locator` would
  leak the first locator into the copies.
- `quail-worker`'s `IsolatedRunner` ignores it: with path jobs the supervisor never holds the
  bytes (`quail scan` refuses `--dedup` with `--isolate`).

## Getting artifacts out

A record carries facts only; the artifacts (the extracted `Document`, a page raster, ...) are
dropped when the file is done. To keep them without registering an analyzer (which would
appear in `steps`):

- one file: `Engine::process_with(item, |record, artifacts| ..)` returns the same record as
  `process` and calls the closure first, on the calling thread (a panic in it is the
  caller's and is not caught);
- a run: `RunOptions::on_artifacts`, called on the worker thread for every processed file,
  concurrently and in completion order, before the artifacts are dropped. It is not called
  for the `Failed` records of source errors. A panic in it is caught and counted in
  `RunStats::hook_panics` (first message in `first_hook_panic`); the record is still
  written. `quail scan --dump-documents` uses it.

## Overhead

`cargo bench -p quail-engine --profile profiling`: about 0.29 µs per file for sniffing
alone and 1.6 µs per file with a no-op extractor and 8 no-op analyzers (x86_64 laptop).

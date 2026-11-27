# Security

quail is built to read millions of files nobody has checked: mail attachments, scanned
archives, exports from unknown systems. This page describes what quail assumes about its
input, the defences it has, how they are tested, and how to report a problem.

## The input model

Every file is treated as hostile. A file may be:

- malformed by accident (truncated, a broken cross-reference table, a ZIP with a bad
  directory);
- crafted to exhaust a resource: a deflate or object-stream bomb, an image that declares
  100 000 x 100 000 pixels, a page tree with millions of pages, forms nested 2^40 times,
  XML nested a million levels deep;
- crafted to make a parser loop or recurse without end.

quail does not run anything from a file. It executes no JavaScript, macros or embedded
programs (it only reports that they exist: `pdf.has_javascript`, `docx.has_macros`), resolves
no external references and opens no network connections. The default build loads no native
code.

What quail promises for any input:

1. **One record per file.** A bad file never ends the run. It gets a `partial` or `failed`
   record that says what went wrong.
2. **Bounded resources per file.** Memory, decoded bytes, pages, text and time are capped by
   `Limits`; past a cap the step stops with a partial result.
3. **No silent wrong answers from a limit.** A step that stopped short records a *problem*,
   which makes the record `partial` ([concepts.md](concepts.md#7-status-problems-and-warnings)).

## Defences, layer by layer

### Limits

Every file runs under `Limits` (defaults in parentheses; `quail scan --help` lists the
flags):

| limit | default | flag | what happens past it |
|---|---|---|---|
| `max_bytes` | 512 MiB | `--max-bytes` | the file is not read at all; `failed` record naming the limit |
| `max_decompressed` | 256 MiB | `--max-decompressed` | decoding stops (ZIP entries, PDF streams); partial document |
| `max_pages` | 2000 | `--max-pages` | later pages are skipped; problem |
| `max_text_bytes` | 64 MiB | - | text is cut at a block boundary; problem |
| `max_render_pixels` | 40 M pixels | - | images larger than this are refused before decoding |
| `timeout` | 2 min | `--timeout` | the file's deadline (see below) |
| `max_timeout` | none | derived from the OCR budget under `--profile deep` | the latest a step (OCR, for its pages) may extend the deadline to |

The source refuses an oversized file before reading it, and the engine bounds the bytes in
flight across all workers (`--max-in-flight-bytes`, default 1 GiB).

### Deadlines

The engine cannot interrupt a thread, so deadlines are cooperative: every step receives the
file's deadline and checks it in its loops (per page, per chunk, every 1024 content-stream
operators inside a PDF page). A step past the deadline returns what it has and records a
problem. Rendering and OCR share the same deadline as extraction, so one file has one time
budget. Only OCR extends it, by what the pages it found need, never past `max_timeout`; a
file without pages to OCR has the plain `timeout`.

### Decode budgets inside the parsers

Rust aborts the process when an allocation fails, and that cannot be caught. So the
allocation has to be refused before it happens, inside the decoders. quail vendors the
pure-Rust hayro PDF crates with patches ([architecture.md](architecture.md#the-pdf-stack)):

- one per-file budget of decoded bytes, charged by every filter layer of every stream;
- declared image sizes checked against `max_render_pixels` before any buffer is allocated
  (`/Width` x `/Height`, JPEG, JPEG 2000, JBIG2 and CCITT headers);
- capped, fallible buffer growth (`try_reserve`), so a refused decode is an error, not an
  abort;
- caps on page trees (131 072 pages), Type3 glyph recursion (depth 4), JBIG2 bitmaps and
  halftone grids;
- a render budget (time, bytes, rasterization work) for page renders.

The DOCX and PPTX readers are streaming XML readers with no DOM, on one shared package
reader (`quail-ooxml`): at most 20 000 ZIP entries (counted from the end-of-central-directory
record before the directory is parsed), unsafe entry names ignored, and the bytes entries
*actually* inflate to charged against the same `max_decompressed` budget (declared sizes are
never trusted). A DOCTYPE is never expanded, so entity bombs and external entities (XXE) do
nothing; one XML event is at most 16 MiB; nesting depth is bounded.

The HTML reader runs the html5ever tokenizer without its tree builder, so there is no DOM and
the element stack is quail's own: 512 open elements, 32 nested tables, 1024 columns, 250 000
blocks. HTML has no user-defined entities (only the fixed table of character references), and
input that goes 16 MiB without completing a token (an endless comment, tag or attribute) stops
the read. Nothing is fetched: no stylesheet, script, frame or image.

### Panic containment

Every step runs under `catch_unwind`. A panic ends that step for that file only: its facts are
dropped, the record becomes `partial` (or `failed` for an extractor), and the run continues.
The release profile keeps `panic = "unwind"` for this reason. Worker threads have 32 MiB
stacks, so deep but finite recursion does not overflow.

### Process isolation

What `catch_unwind` cannot stop (an abort, an out-of-memory, a stack overflow, a hang in code
that never checks its deadline), a separate process can. With `quail scan --isolate`:

- every file is processed in a worker child process;
- each worker has an address-space limit (`--worker-mem`, `RLIMIT_AS`, default 1 GiB) and
  no core dumps;
- a watchdog kills a worker that does not answer within `2 x --timeout + 5 s`, plus twice
  any deadline extension the worker reports for the file (never more than `max_timeout`
  allows);
- a crashed file gets a `failed` record (`worker crashed: out of memory: ...`), is retried
  once in a fresh worker, and is recorded as poisoned if it crashes again.

Use `--isolate` for any corpus you do not control. Its cost is small for real documents
([performance.md](performance.md#process-isolation)). Linux enforces all limits; see
[architecture.md](architecture.md#worker-isolation) for macOS and Windows.

### Code rules

- `unsafe` is denied in the whole workspace (one opt-in exception: the `mmap` feature of
  `quail-io`).
- No `unwrap`, `expect` or indexing that can panic on file content.
- `cargo deny` checks licenses and advisories of every dependency in CI.

## How it is tested

- **Hostile test suites.** `crates/quail-pdf/tests/hostile.rs`,
  `crates/quail-docx/tests/hostile.rs`, `crates/quail-pptx/tests/hostile.rs` and
  `crates/quail-html/tests/hostile.rs` generate crafted inputs in code (bombs, huge images,
  deep nesting, recursion, cycles) and fail if any of them gets through a limit.
- **Fuzzing.** `quail-fuzz` is a mutation fuzzer for the whole pipeline that runs on stable
  Rust. Every case runs in a child process with memory, CPU and wall-clock limits, and the
  parent classifies how it ended: `ok`, `caught-panic`, `deadline`, `mem-high`, `slow`,
  `oom-abort`, `stack-overflow`, `timeout`, `cpu-limit`, `killed`, `abort`. Seeds are
  generated (DOCX, PPTX and PDF builders, HTML shaped like EDGAR filings, hostile Unicode
  text); the mutators know the formats (ZIP header lies, XML nesting storms, PDF filter
  chains, page-tree cycles, xref loops, incremental updates with hostile objects, HTML
  nesting floods, endless tokens and charset swaps). Each new crash signature is saved with a
  delta-debugged minimal copy. How to run it: [development.md](development.md#fuzzing).
- **Isolation tests.** `quail-cli`'s tests crash workers on purpose (a test-only build
  feature) and check that every file still gets exactly one record.

## Reporting a problem

If a file makes quail crash, hang, use far more memory than its limits allow, or produce a
fact that leaks something it should not:

1. Reduce it if you can. `quail-fuzz minimize FILE OUT` shrinks a crashing input while it
   keeps crashing the same way.
2. **Never attach a confidential document.** Attach the minimized file only if it contains
   nothing private, or describe how to build an equivalent file.
3. Open an issue at <https://github.com/05vukasin/quail/issues> with the `quail version`
   output, the command line, the record (or the stderr of the crash) and the platform.

For a problem that should not be public before it is fixed (for example a way to make quail
read or write outside the files it was given), open an issue that asks for a private contact
without the details.

# Architecture

How quail is put together: the crates and what may depend on what, how the engine plans and
runs the steps for one file, how a run over many files stays inside a memory budget, how a
crash is kept to one file, and why the PDF stack is vendored. The ideas used here (facts,
artifacts, steps) are explained in [concepts.md](concepts.md).

## Crates

quail is a Cargo workspace. Every crate lives in `crates/` and has its own README with the
exact keys, ids and limits.

| crate | role | namespace |
|---|---|---|
| `quail-core` | the contract: `Blob`, `Format`, `Document`/`Page`/`Block`, `Fact`/`Value`/`Evidence`, `Analyzer`/`Extractor`/`Emitter`/`Ctx`, `Artifacts`, `Limits`/`Profile`, `FileRecord`, `Source`/`RecordSink`. Types and traits only | - |
| `quail-engine` | format sniffing, step planning, limits and deadlines, panic containment, bounded parallel runs, content dedup | `engine.*` |
| `quail-hash` | BLAKE3, XXH3-128 and FastCDC chunk hashes of the raw bytes | `hash.*` |
| `quail-pdf` | PDF extractor on the vendored, hardened hayro crates; provides the page `raster` | `pdf.*` |
| `quail-docx` | streaming DOCX (OOXML) extractor | `docx.*` |
| `quail-text` | text utilities (normalization, scripts, money parsing, encoding detection), the plain-text extractor and the text analyzers | `text.*` |
| `quail-signals` | identifiers with check digits, dates with roles, amounts, contact domains | `signal.*` |
| `quail-refs` | own document numbers, cited documents, titles, instrument kind | `ref.*` |
| `quail-parties` | company mentions, roles, defined terms, ranked party candidates | `party.*` |
| `quail-clauses` | governing law, term and renewal, notice and survival periods, termination | `clause.*` |
| `quail-items` | price lines (description, quantity, unit, prices) with CPV categories | `item.*` |
| `quail-sketch` | similarity sketches: MinHash + LSH bands, SimHashes, layout, page pHash | `sketch.*` |
| `quail-summary` | the file card and extractive summary (finalizers); optional local LLM summary | `summary.*` |
| `quail-ocr` | OCR of pages without a usable text layer (native backend; in `quail` with `--features ocr`, profile `deep`) | `ocr.*` |
| `quail-io` | sources (filesystem walk, manifest) and sinks (JSONL, optional zstd) | - |
| `quail-worker` | process isolation: supervisor, worker children, rlimits, watchdog | - |
| `quail-cli` | the `quail` binary; the only place that names every concrete step | - |
| `quail-eval` | evaluation harness and the `quail-eval` binary | - |
| `quail-fuzz` | stable-Rust mutation fuzzer for the whole pipeline | - |

## Dependency rules

```
quail-cli ──▶ quail-worker ──▶ quail-engine ──▶ quail-core
    │                                               ▲
    ├──▶ quail-io ──────────────────────────────────┤
    │                                               │
    └──▶ leaf crates (one cargo feature each)       │
           pdf, docx, hash, ocr ────────────────────┤
           signals, refs, parties, clauses,         │
           items, sketch, summary ──▶ quail-text ───┘
```

`quail-eval` and `quail-fuzz` are tools on top: the harness reads records (with `quail-io`,
`quail-hash` and `quail-text`), the fuzzer wires an engine like `quail-cli` does.

- `quail-core` depends only on `serde` and `thiserror`.
- A leaf crate (pdf, docx, hash, signals, refs, parties, clauses, items, sketch, summary,
  ocr) depends on `quail-core` and, when it needs text utilities, on `quail-text`. Never on
  another leaf crate. A crate that reads another crate's facts (the summary card reads almost
  all of them) names the keys as plain strings, and a test checks that each one is declared by
  its owner.
- Only `quail-engine` (generically) and `quail-cli` (concretely) wire steps together.
  `quail-cli/src/registry.rs` is the one file that names every extractor and analyzer, each
  behind the cargo feature of its crate.
- A crate that links a native library (ONNX Runtime for OCR, llama.cpp for the LLM summary,
  the C zstd library) puts it behind a non-default cargo feature. The default build is pure
  Rust.
- One namespace per crate. A crate emits facts only under its own prefix and declares every
  key once, as a `pub const` in its `keys` module. `quail keys` and [keys.md](keys.md) are
  generated from those declarations.

## One file: the engine

`Engine::process(item)` turns one `SourceItem` (a locator and its bytes) into one
`FileRecord`:

```
bytes ──▶ sniff ──▶ extractor for the format ──▶ analyzers in plan order ──▶ finalizers ──▶ record
          engine.    pdf / docx / text.extract     hash, text, signal, ref,      summary.card,
          sniff@2    (Document, Raster)            party, clause, sketch, item   summary.extractive
```

1. **Sniff.** The format comes from the bytes only, never the file name: `%PDF-` near the
   start, a ZIP whose directory holds `word/document.xml`, image magic numbers, or text. The
   work is bounded and nothing is inflated.
2. **Extract.** The extractor registered for that format (one per format) builds the
   `Document` artifact and file-level facts. Formats with page geometry also provide a
   `Raster` artifact, so later steps can render a page without knowing the extractor.
3. **Analyze.** Analyzers run in the order fixed when the engine was built (see below). Each
   gets a read-only `Ctx` (the bytes, the format, the artifacts and the facts produced so far)
   and an `Emitter` for its output. The engine merges an analyzer's facts and artifacts only
   after it returns, so no step sees another one's half-finished output.
4. **Record.** Facts in production order, the ids of the steps that ran, per-step timings, a
   status and warnings.

### The plan

`EngineBuilder::build()` checks the registered steps once and computes a fixed order:

- **`requires`**: artifact names that must exist before a step runs (`document`, `raster`,
  `normalized_text`, ...). The builder fails if no active step provides one, naming the
  inactive provider when a profile left it out.
- **`provides`**: artifact names a step may produce. A requirement adds an edge from every
  provider.
- **`after`**: soft ordering. If an active step provides one of these names, run after it;
  if none does, run anyway. Text consumers declare `after(&["revised_document"])`, so with
  OCR active they read the OCR'd text, and without it they run as before.
- **`finalizer`**: runs after every non-finalizer step, whatever the registration order.
  The summary card and the extractive summary are finalizers, because they read the facts of
  all other steps. Finalizers are ordered among themselves by their requirements.
- **`profile`**: steps above the run's profile are left out of the plan
  (`Engine::inactive`).

Ties keep registration order, so the plan, and with it the record, is deterministic. Cycles
and unsatisfied requirements are build errors, not runtime surprises. `quail version` prints
the plan and a fingerprint over it.

### Status and containment

Every step runs under `catch_unwind` with the file's deadline. An analyzer that errors,
panics or times out loses its own facts and makes the record `partial`; the steps that do not
depend on it still run. An extractor failure makes the record `failed`, but the generic
analyzers (hashes) still run on the bytes. The full table is in
[concepts.md](concepts.md#7-status-problems-and-warnings).

## Many files: `Engine::run`

```
Source ──▶ reader ──▶ N worker threads ──▶ writer thread ──▶ RecordSink
(walk,       │         (Engine::process,       │              (JSONL file
 manifest)   │          32 MiB stacks)         │               or stdout)
             └── bounded: max_in_flight files, max_in_flight_bytes ──┘
```

- Workers are plain threads; one writer thread owns the sink; bounded channels in between.
- At most `max_in_flight` files (default 2 x threads) and `max_in_flight_bytes` (default
  1 GiB) are between the source and the sink. A single file larger than the byte bound runs
  alone.
- `--ordered` keeps source order with a reorder buffer no larger than `max_in_flight`.
- Worker stacks are 32 MiB, because deep recursion on hostile nesting would otherwise
  overflow the stack, which aborts the process.
- Sources cap the file size themselves (`--max-bytes`), so an oversized file is never read
  into memory.
- With `--dedup`, each worker hashes its file first; a file whose bytes were already
  processed in this run gets a copy of that record with its own locator and
  `engine.duplicate_of` ([concepts.md](concepts.md#13-content-dedup)).

## Worker isolation

In-process, a panic costs one file. But in Rust a failed allocation aborts the process, a
stack overflow kills it, and a parser stuck in a loop that never checks its deadline cannot
be preempted. `quail scan --isolate` (crate `quail-worker`) handles those:

- The `quail` binary starts `-j` copies of itself as worker processes. Each builds its own
  engine with the same profile and limits, so models are loaded once per worker.
- Workers read the files themselves; file bytes never pass through the supervisor.
- Each worker runs under `RLIMIT_AS` (`--worker-mem`, default 1 GiB), with core dumps
  disabled, and under a watchdog that kills it after `2 x --timeout + 5 s`.
- A dead worker gives its file a `failed` record (`worker crashed: out of memory`, `stack
  overflow`, `timed out`, ...). The file is retried once in a fresh worker; a second crash
  records it as poisoned. Every input gets exactly one record.
- Records are the same as in process apart from timings.

Linux enforces every limit. On macOS `RLIMIT_AS` is not reliably enforced; on Windows there
are no rlimits, but workers are still separate processes under the watchdog. The cost of
isolation is in [performance.md](performance.md#process-isolation).

## The PDF stack

PDF parsing, content-stream interpretation and page rendering use the pure-Rust
[hayro](https://github.com/LaurenzV/hayro) crates (`hayro-syntax`, `hayro-interpret`,
`hayro-jbig2`, `hayro`). quail vendors them in `vendor/` and routes the crates.io versions
there with `[patch.crates-io]`.

Why: Rust aborts the process on allocation failure, and `catch_unwind` cannot catch that. A
747-byte PDF could make the upstream crates allocate 4 GiB, and an 8 KB one could
object-stream-bomb or fan out forever. These blow-ups have to be refused before the
allocation, inside the decoders, which a caller cannot do from outside. The patches add:

- a per-file **decode budget**: every byte decoded from the file (object streams, content,
  fonts, forms, images, ...) is charged against `max_decompressed`, and every filter layer
  stops at what is left;
- **image size checks before allocation**: `/Width` x `/Height` and the sizes in the JPEG,
  JPEG 2000, JBIG2 and CCITT headers must fit `max_render_pixels`;
- caps on page-tree size, Type3 glyph recursion depth and JBIG2 bitmaps;
- a **render budget** for the renderer: deadline, bytes and rasterization work are charged
  while recording, because the rasterizer cannot be interrupted once it starts.

Every change is marked `// quail:` and listed in `vendor/README.md`. `quail-pdf` pins the
exact upstream versions (`=0.9.0`) so the patches always apply to the code they were written
for. The same directory holds a patched `lingua` (language detection), changed for speed with
bit-identical output ([performance.md](performance.md#what-was-optimized)).

The policy for vendored crates is in [development.md](development.md#vendored-crates).

## Portability

- **No `target-cpu=native`**, anywhere. A binary built on one x86_64 machine runs on any
  other.
- **SIMD only with runtime detection and a scalar fallback.** BLAKE3 (built with its `pure`
  feature: Rust intrinsics, no assembly, so no C toolchain is needed) picks SSE4.1, AVX2 or
  NEON at run time; `memchr` does the same for the PDF byte scans.
- **Pure-Rust default build.** `cargo build` needs only a Rust toolchain. Native libraries are
  opt-in features (`zstd` and `ocr` of `quail-cli`, the summary's `llm`).
- **`unsafe` is denied** workspace-wide. The one exception is the optional `mmap` feature of
  `quail-io`, isolated in one file.
- **Panics unwind** (`panic = "unwind"` in the release profile), so `catch_unwind` can
  contain them per step.
- CI runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` and
  `cargo test --workspace` on Linux x86_64, Linux ARM, macOS and Windows, plus `cargo deny`.

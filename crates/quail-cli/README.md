# quail-cli

The `quail` command. It wires the concrete extractors and analyzers into `quail-engine` and
runs them over files: `scan` many files to JSONL, `inspect` one file, list `keys`, print the
pipeline `version`. Emits no facts of its own.

```sh
cargo install --path crates/quail-cli          # or: cargo build --release -p quail-cli
```

## scan

```sh
# Every file under two roots, records to a file, progress and summary on stderr.
quail scan /data/contracts /data/inbox -o records.jsonl

# Only PDFs and DOCX, no dotfiles, reproducible order, records in walk order.
quail scan /data --ext pdf,docx --skip-hidden --sorted --ordered > records.jsonl

# A fixed split (`path` or `locator<TAB>path` per line), plus the extracted documents
# in the quail-eval sidecar format.
quail scan --manifest split.tsv -o recs.jsonl --dump-documents docs.jsonl

# Tighter limits, 4 workers, machine-readable stats.
quail scan /data -j 4 --max-bytes 64M --max-pages 200 --timeout 30s --json-stats -q

# Every file in a worker child process: a crash, OOM or hang costs one record.
quail scan /data -j 4 --isolate --worker-mem 1G -o records.jsonl

# First records only: exits quietly as soon as `head` closes the pipe.
quail scan /data | head -3
```

| option | effect |
|---|---|
| `PATH...` / `--manifest FILE` | roots to walk (files or directories) / a manifest (`-` = stdin) |
| `-o, --out FILE` | records here instead of stdout |
| `--dump-documents FILE` | one line per extracted document: `{"locator","blake3","document"}` (`quail-eval`'s `inspect::SidecarLine`) |
| `--zstd` | zstd-compressed records (build with `--features zstd`) |
| `--profile fast\|standard\|deep` | analyzers above the profile do not run (default `standard`) |
| `-j, --threads N` | workers (default CPUs − 1) |
| `--max-in-flight N`, `--max-in-flight-bytes SIZE` | memory bound between source and sink (default 2 × threads, 1G) |
| `--ordered` | records in source order |
| `--include GLOB`, `--exclude GLOB`, `--ext a,b`, `--skip-hidden`, `--sorted`, `--follow-symlinks` | walk filters (`quail-io`'s `FsSource`) |
| `--max-bytes SIZE`, `--max-decompressed SIZE`, `--max-pages N`, `--timeout DUR` | per-file `Limits` overrides |
| `-q, --quiet` / `--json-stats` | no stderr output / final stats as one JSON object |
| `--isolate` | process each file in a worker child process (`quail-worker`), see below |
| `--worker-mem SIZE` | address space per worker (`RLIMIT_AS`, default 1G) |
| `--retry-crashed` / `--no-retry` | retry a file whose worker crashed once in a fresh worker (default) / record the first crash |

Behaviour:

- **Oversized files are never read.** `--max-bytes` sets both `Limits::max_bytes` and the
  source's `max_file_bytes`: a bigger file becomes a `failed` record naming the cap.
- **Unreadable files are records.** A file that cannot be opened (or a manifest entry that is
  missing) gets a `failed` record under its locator, so a run accounts for every input.
- **Ctrl-C** stops pulling files, finishes the ones in flight, writes their records, prints
  the stats and exits with 130. A second Ctrl-C exits at once.
- **Broken pipe** (`| head`): the run stops, nothing is printed, exit code 0. Files in flight
  get 2 s to finish (so `--dump-documents` stays complete), then the process exits.
- **stderr**: a progress line when stderr is a terminal, then a summary: files, ok / partial /
  failed, MB, files/s, MB/s, source and sink errors, and the most frequent problems (digits
  folded to `#`, the file's own path to `<file>`, so they group by kind).
- Exit code: 0 (even when some files failed: that is in the records), 1 on a sink or dump
  error or an unusable input (bad glob, missing manifest), 2 on a usage error (unknown or
  malformed option), 130 after Ctrl-C.

`--dump-documents` uses the engine's artifacts hook (`RunOptions::on_artifacts`), so records
are byte-for-byte what a run without it writes (no extra step). Files without a document get
no line. A failed write or a panic while dumping is reported in the summary
(`hook_panics` in `--json-stats`) and makes the exit code 1.

### --isolate

In-process, a panic costs one file (the engine's `catch_unwind`), but an abort, a failed
allocation, a stack overflow or a parser that never checks its deadline ends the run. With
`--isolate` the `quail` binary re-runs itself as `-j` worker processes
(`quail_worker::IsolatedRunner`); each builds its own engine (same profile and limits) and
reads the files itself, so file bytes never pass through the supervisor. Records, the order
(`--ordered`), `--dump-documents` (the worker sends the document back), Ctrl-C, broken pipe
and exit codes are the same as in process; records are identical apart from timings.

- A dead worker gives its file a `failed` record, `worker crashed: <reason>` (aborted, out of
  memory, stack overflow, timed out, killed...), with the size and the sniffed format. The
  file is retried once in a fresh worker unless `--no-retry`; a second crash records it as
  poisoned. Hangs are killed after `2 x --timeout + 5 s` and not retried.
- `--worker-mem` is `RLIMIT_AS` per worker (Linux enforces it). It covers the models (the
  language models are the largest part), the file and its document: budget about 4x the
  largest file accepted on top of the models. Total memory is about `-j x` that.
- Size cap and unreadable files are checked in the supervisor, with the same records as in
  process.
- Stats gain an `isolation` object (`--json-stats`): `crashes`, `crash_kinds` (by kind:
  `abort`, `oom`, `stack_overflow`, `cpu_limit`, `timeout`, `exit`, `killed`, `signal`,
  `protocol`), `retries`, `retried_ok`, `poisoned`, `respawns`, `spawn_failures`; the human
  summary prints a `worker crashes:` line when there were any.

## inspect

```sh
quail inspect contract.docx           # record: header, status, steps, facts by namespace
quail inspect contract.docx --text    # plus every page, block by block: [zone/kind] text
quail inspect scan.pdf --json --text  # {"record": .., "document": ..}
```

Facts are grouped by namespace and show value, `conf` (when below 1), `src` and evidence
(`p3 header [10..42] box(72,40,300,58)`). Long text is cut at 160 characters, lists at 8
items. Records print as `{field=value ...}` (`party.company`, `ref.doc_number`, `ref.cites`);
a list of records (`party.candidates`) prints one record per line under its key. Floats show
at most four decimals. Exit code 1 for a `failed` record.

## keys

```sh
quail keys            # every step and every fact key of this build, one line each
quail keys docx.      # a prefix filter
quail keys --json
```

## version

```sh
quail version                 # step ids at the default profile and the pipeline fingerprint
quail version --profile deep --json
```

The fingerprint is BLAKE3 (first 8 bytes, hex) over the schema version, the profile and
every step id in run order (sniffing, the extractor of each format, the analyzer plan). It
changes exactly when a step's version changes or a step is added or removed, which is when
stored records may be stale. Limits are not part of it.

## OCR (`ocr` feature, not default)

Scanned pages and garbled text layers are read by `ocr.pages@2` from
[quail-ocr](../quail-ocr/README.md). It uses PP-OCRv6 on ONNX Runtime, and the Cyrillic recognizer
re-reads Serbian Cyrillic lines. The default build stays pure Rust. The `ocr` feature links
ONNX Runtime (prebuilt binaries for Linux x64/arm64, macOS arm64 and Windows, downloaded by the
`ort` build script) and an HTTPS client for the model download.

```sh
cargo build --release -p quail-cli --features ocr
quail models fetch                 # about 45 MiB into $QUAIL_OCR_MODELS or the user cache dir;
                                   # every file's SHA-256 is checked before it is moved into place
quail models list                  # each file: size, ok / missing / bad sha-256, license
quail scan --profile deep contracts/ -o records.jsonl
```

- OCR runs only under `--profile deep`. Under other profiles the step is inactive, and no model is
  loaded or checked.
- It reads the pages that quail-pdf flags `needs_ocr`:
  - image-only scans;
  - mostly-image pages with a thin text layer;
  - pages whose text layer is garbage, including Cyrillic OCR'd as Latin look-alikes. For these
    pages the OCR text replaces the layer.
- Text analyzers (`text.*`, `signal.*`, `ref.*`, `party.*`, `item.*`, the summary) run after it
  and read the revised document.
- Budgets are derived, not hand-picked (`registry::OcrSettings::resolve`, `scan::plan`):
  - `--ocr-jobs` pages are read at once in the process whatever `-j` (default: 1 GiB each
    within 40 % of the available and 20 % of the total memory, at most 4 and at most `-j`);
  - `--ocr-threads` ONNX Runtime threads per engine (default `CPUs / jobs`, at most 4);
  - per file at most `--ocr-max-pages` (40) pages and `--ocr-page-time` (45 s) of reading per
    page; past either the record is `partial`. The OCR text counts against `max_text_bytes`.
  - Every file starts with `--timeout` (2m). OCR extends the deadline of a file with pages
    to read by what they need and by its waits for a free job, up to `Limits::max_timeout`
    (`timeout + 40 x 45s x ceil(-j / jobs)`, at most 2 h), so the budget fits behind the
    other workers; a file without pages to OCR keeps the timeout. With `--timeout` and no
    `--ocr-page-time` there is no per-page budget and no extension.
  - With `--isolate`, one engine per worker process and `-j` defaults to the OCR jobs.
  - `scan` and `inspect` print the resulting line `quail: ocr: ...` on stderr (not with `-q`).
- Under `--profile deep` without models (or without the feature), `scan` and `inspect` print one
  warning. Every file with a page needing OCR then gets the problem `ocr.pages@2: ocr models: …`
  and is `partial`.

Models: `quail models fetch --dir DIR` or `QUAIL_OCR_MODELS=DIR`. They are the PaddleOCR ONNX
exports under Apache-2.0, listed with URL, size and SHA-256 in `quail_ocr::models::MANIFEST`.

## Registry

`src/registry.rs` is the only place that names concrete crates: `builder(profile, limits)`
returns the `EngineBuilder` with everything this build has, `STEPS` documents each step and
`key_tables()` lists each crate's keys. One cargo feature per crate:

| feature | crate | steps |
|---|---|---|
| (always) | quail-engine, quail-hash | `engine.sniff@2`, `hash.identity@1` |
| `docx` (default) | quail-docx | `docx.extract@1` |
| `pdf` (default) | quail-pdf | `pdf.extract@2` (provides the `raster` artifact) |
| `text` (default) | quail-text | `text.extract@1` (extractor for `Format::Text`: `.txt` and anything sniffed as text), `text.stats@3`, `text.lang@2` |
| `signals` (default) | quail-signals | `signal.scan@5` |
| `refs` (default) | quail-refs | `ref.references@3` |
| `parties` (default) | quail-parties | `party.candidates@4` |
| `clauses` (default) | quail-clauses | `clause.terms@1` |
| `sketch` (default) | quail-sketch | `sketch.minhash@2`, `sketch.simhash_text@2`, `sketch.simhash_template@2`, `sketch.layout@2`, `sketch.invoice_layout@2`, `sketch.text_key@2` (Fast); `sketch.minhash_compat@3` (Standard: not in `--profile fast`); with `pdf`: `sketch.page_phash@4` (Standard, runs only on documents with a scanned page or a page needing OCR) |
| `items` (default) | quail-items | `item.lines@5` (CPV categories from Standard) |
| `summary` (default) | quail-summary | `summary.card@4`, `summary.extractive@2`, `summary.outline@1`, `summary.keywords@1`, `summary.topics@1` (Standard, documents of 20+ pages) (finalizers); `summary.digest@3` is in the crate, not registered by default |
| `ocr` | quail-ocr | `ocr.pages@2` (Deep only; PP-OCRv6 on ONNX Runtime, native, not default) and `quail models` |
| `zstd` | (quail-io) | `--zstd` output |
| `test-hooks` | - | test-only: `QUAIL_TEST_CRASH_ON=<substring>` makes `--isolate` workers abort on matching locators. Never ship it |

Key docs (`src/keydocs.rs`) are generated from each crate's `keys` module by
`tools/genkeys.py`; a unit test fails when a crate declares a key that is not in the table.

## Numbers

See the pull request that introduced the crate: end-to-end files/s and peak RSS at 1, 4
and 8 threads on a private corpus of business documents (aggregates only).

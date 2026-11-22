# Command line

Every `quail` command, option and exit code. `quail --help` and `quail <command> --help` print
the same information for your build. The evaluation tool `quail-eval` is described in
[evaluation.md](evaluation.md#commands).

```
quail scan     PATH... | --manifest FILE     files to JSONL records, one per file
quail inspect  FILE                          one file, pretty-printed
quail keys     [PREFIX]                      every fact key and step of this build
quail version                                step versions and the pipeline fingerprint
quail models   fetch | list                  OCR model files (only in a build with --features ocr)
```

Sizes take `K`, `M`, `G` or `T` (powers of 1024, with or without `B`/`iB`; a bare number is
bytes). Durations take `ms`, `s`, `m` (or `min`) or `h`; a bare number is seconds.

## quail scan

Processes every file under the given paths (files or directories) and writes one JSON record
per file ([concepts.md](concepts.md#1-filerecord)) to stdout or `--out`. Progress and a summary
go to stderr.

```sh
# Every file under two roots, records to a file.
quail scan /data/contracts /data/inbox -o records.jsonl

# Only PDF and DOCX, no dotfiles, reproducible order, records in walk order.
quail scan /data --ext pdf,docx --skip-hidden --sorted --ordered > records.jsonl

# A fixed list of files, plus the extracted documents for quail-eval.
quail scan --manifest split.tsv -o recs.jsonl --dump-documents docs.jsonl

# Tighter limits, 4 workers, machine-readable stats.
quail scan /data -j 4 --max-bytes 64M --max-pages 200 --timeout 30s --json-stats -q -o records.jsonl

# Untrusted input: every file in a worker process.
quail scan /data -j 4 --isolate --worker-mem 1G -o records.jsonl

# Analyze each distinct content once.
quail scan /data --dedup -o records.jsonl
```

### Input

| option | effect |
|---|---|
| `PATH...` | files and directories to walk |
| `--manifest FILE` | read the files named in FILE instead: one `path`, or `locator<TAB>path`, per line; `#` starts a comment; `-` reads stdin. The locator is what the record's `locator` says |
| `--ext a,b` | only these extensions, comma-separated, case-insensitive |
| `--include GLOB` | only files whose path relative to its root matches one of these globs (`*` crosses `/`); repeatable |
| `--exclude GLOB` | skip files and directories matching these globs; an excluded directory is not entered; repeatable |
| `--skip-hidden` | skip files and directories whose name starts with `.` |
| `--sorted` | walk in sorted order (byte order of names), for reproducible runs |
| `--follow-symlinks` | follow symbolic links (by default they are skipped; a loop is one error) |

### Output

| option | effect |
|---|---|
| `-o, --out FILE` | write records here instead of stdout (`-` is stdout) |
| `--ordered` | write records in source order; a slow file holds back the ones after it |
| `--dump-documents FILE` | also write each extracted document as one JSON line `{"locator", "blake3", "document"}`, the sidecar format `quail-eval inspect --documents` reads. Records are unchanged by it |
| `--zstd` | zstd-compress the records; needs a build with `--features zstd` |
| `-q, --quiet` | no progress line and no summary on stderr |
| `--json-stats` | print the final stats as one JSON object on stderr instead of the summary |

### Work

| option | default | effect |
|---|---|---|
| `--profile fast\|standard\|deep` | `standard` | how much work per file ([concepts.md](concepts.md#9-profiles)); `deep` adds OCR in a build with `--features ocr` |
| `-j, --threads N` | CPUs - 1 | worker threads (or worker processes with `--isolate`) |
| `--max-in-flight N` | 2 x threads | files between the source and the sink |
| `--max-in-flight-bytes SIZE` | `1G` | bytes of files in flight; a single larger file runs alone |
| `--dedup` | off | a file with the same bytes (BLAKE3) as an earlier file of the run gets that file's facts plus `engine.duplicate_of`, without running any step. Such files are not in `--dump-documents`. Not with `--isolate` |

### Limits per file

| option | default | effect |
|---|---|---|
| `--max-bytes SIZE` | `512M` | larger files are not read; they get a `failed` record naming the limit |
| `--max-decompressed SIZE` | `256M` | bytes a document may inflate to while it is decoded (ZIP entries, PDF streams); past it the extractor stops with a partial document |
| `--max-pages N` | 2000 | pages extracted per document |
| `--timeout DURATION` | `2m` | wall time per file. Under `--profile deep` with OCR, raised to fit the OCR budget (below) unless given |

### OCR (build feature `ocr`, `--profile deep`)

Every default is derived from the machine and the other options, so a deep scan needs no
`-j` or `--timeout` tuning. `scan` and `inspect` print the result on stderr (not with `-q`):
`quail: ocr: 2 page(s) at a time, 4 thread(s) each; up to 40 pages per file, 45s per page;
timeout 120m per file`.

| option | default | effect |
|---|---|---|
| `--ocr-jobs N` | `min(-j, 40 % of available memory / 1 GiB, 4)`, at least 1 (2 off Linux) | pages OCR'd at the same time in the process, whatever `-j`; each takes about 0.6 to 1 GB. Other workers keep processing files that need no OCR, or wait for a free job (oldest file first) |
| `--ocr-threads N` | `CPUs / --ocr-jobs`, 1 to 4 | ONNX Runtime intra-op threads per OCR engine, so that jobs x threads fits the CPUs |
| `--ocr-max-pages N` | 40 | pages OCR'd per file; later pages are left out with a problem (`partial`) |
| `--ocr-page-time DURATION` | `45s`, none with `--timeout` | reading time per page needing OCR; a file's OCR stops with a problem (`partial`) once pages x this is spent. Waiting for a free job does not count |

Without `--timeout`, the per-file timeout becomes `2m + max-pages x page-time x
ceil(-j / ocr-jobs)`, at most 2 hours: enough for a file's whole OCR budget even with every other worker's file
ahead of it. With `--timeout`, that value is the file's wall time and there is no per-page budget
unless `--ocr-page-time` is also given. With `--isolate`, each worker process loads its own engine:
each reads one page at a time and `-j` defaults to the `--ocr-jobs` value.

### Isolation

| option | default | effect |
|---|---|---|
| `--isolate` | off | process every file in a worker child process: an abort, out-of-memory, stack overflow or hang costs that file a `failed` record (`worker crashed: ...`), not the run. Workers read the files themselves; each loads its own models |
| `--worker-mem SIZE` | `1G` | address space per worker (`RLIMIT_AS`). Budget the models plus about 4 x the largest file accepted |
| `--retry-crashed` | on | retry a file whose worker crashed once, in a fresh worker |
| `--no-retry` | | record a worker crash as final |

A hung worker is killed after `2 x --timeout + 5 s`; timeouts are not retried. With
`--json-stats`, the stats gain an `isolation` object (`crashes`, `crash_kinds`, `retries`,
`retried_ok`, `poisoned`, `respawns`, `spawn_failures`).

### Behaviour

- **Every input gets a record.** A file that cannot be opened, a manifest entry that is
  missing, or a file over `--max-bytes` gets a `failed` record under its locator.
- **stderr summary**: files, ok / partial / failed, MB, files/s, MB/s and the most frequent
  problems, with digits folded to `#` and the file's own path to `<file>` so that they group
  by kind:

  ```
  quail: 3 files (3 ok, 0 partial, 0 failed), 0.0 MB in 863.19ms: 3.5 files/s, 0.0 MB/s; 1 duplicates reused [15 threads]
    duplicates reused (--dedup): 1 (0 evicted from the cache)
  ```

- **Ctrl-C** stops pulling files, finishes the ones in flight, writes their records, prints
  the stats and exits with 130. A second Ctrl-C exits at once.
- **Broken pipe** (`quail scan /data | head -3`): the run stops quietly with exit code 0.

### Exit codes

| code | when |
|---|---|
| 0 | the run finished, even if some files are `failed` (that is in the records) |
| 1 | the sink or the document dump could not be written, or the input is unusable (bad glob, missing manifest) |
| 2 | usage error (unknown or malformed option) |
| 130 | interrupted with Ctrl-C |

## quail inspect

Processes one file and prints its record for a person: header, status, steps with their
times, then the facts grouped by namespace.

```sh
quail inspect contract.docx            # the record
quail inspect contract.docx --text     # plus every page, block by block: [zone/kind] text
quail inspect scan.pdf --json --text   # {"record": .., "document": ..}
```

| option | effect |
|---|---|
| `--text` | also print every page's text, block by block, with its zone |
| `--json` | print JSON instead: the record, or `{"record": .., "document": ..}` with `--text` |
| `--profile`, `--max-bytes`, `--max-decompressed`, `--max-pages`, `--timeout` | as for `scan` |

Each fact shows its value, `conf` (when below 1), `src` and evidence. Evidence prints as
`at p1 body [775..802] box(72,280,528,344)`: page (one-based here; zero-based in JSON), zone,
byte span in the page text, and box in points. Long text is cut at 160 characters and lists
at 8 items. Exit code 1 when the record is `failed`.

## quail keys

Lists every step and every fact key this build knows, with one-line docs. The generated
reference [keys.md](keys.md) is the same list for the default build.

```sh
quail keys            # everything
quail keys signal.    # only keys and steps starting with a prefix
quail keys --json     # {"steps": [{id, kind, formats, doc}], "keys": [{key, crate, doc}]}
```

## quail version

Prints the version, the schema version, the profile, the build features, every step in run
order and the **pipeline fingerprint**.

```sh
quail version
quail version --profile deep --json
```

The fingerprint is a BLAKE3 hash (first 8 bytes, hex) over the schema version, the profile and
every step id in run order. It changes exactly when a step's version changes or a step is
added or removed, which is when stored records may be stale
([concepts.md](concepts.md#6-steps-extractors-analyzers-ids-and-versions)). Limits are not
part of it. Steps that the profile leaves out are listed as `inactive at this profile`.

## quail models

Only in a build with `--features ocr`. Manages the OCR model files (7 files, about 45 MiB,
Apache-2.0, each listed with URL, size and SHA-256 in `quail_ocr::models::MANIFEST`).

```sh
quail models fetch               # download missing or corrupt files, checking each SHA-256
quail models list                # each file: size, ok / missing / bad sha-256, license
quail models fetch --dir DIR     # another directory
```

Without `--dir` the directory is `$QUAIL_OCR_MODELS`, else the user cache directory
(`quail/ocr-models`). Under `--profile deep`, `scan` and `inspect` print one warning when OCR
cannot run (no `ocr` feature, no model directory, missing models); each file with a page that
needs OCR then gets a problem and is `partial`. See [OCR options](#ocr-build-feature-ocr---profile-deep) and [quickstart.md](quickstart.md#7-ocr) for
memory and time budgets.

## Build features

`quail` is built from `crates/quail-cli`. Each analyzer crate is a cargo feature, all on by
default: `docx`, `pdf`, `text`, `signals`, `refs`, `parties`, `clauses`, `sketch`, `items`,
`summary`. Optional:

| feature | effect |
|---|---|
| `ocr` | OCR of pages that need it (`ocr.pages@2`, only under `--profile deep`) and the `quail models` command. Links ONNX Runtime (prebuilt binaries downloaded by the `ort` build script) and an HTTPS client for the model download |
| `zstd` | `scan --zstd`; links the C zstd library |

`quail version` lists the features of the binary at hand. The LLM summary (`quail-summary`'s
`llm` feature) is not wired into the `quail` binary; see
[quickstart.md](quickstart.md#8-llm-summary-opt-in).

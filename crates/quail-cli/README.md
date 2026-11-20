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

## Registry

`src/registry.rs` is the only place that names concrete crates: `builder(profile, limits)`
returns the `EngineBuilder` with everything this build has, `STEPS` documents each step and
`key_tables()` lists each crate's keys. One cargo feature per crate:

| feature | crate | steps |
|---|---|---|
| (always) | quail-engine, quail-hash | `engine.sniff@1`, `hash.identity@1` |
| `docx` (default) | quail-docx | `docx.extract@1` |
| `pdf` | quail-pdf | `pdf.extract@1` — to wire in when #7 merges |
| `text` (default) | quail-text | `text.stats@1`, `text.lang@1` |
| `signals` (default) | quail-signals | `signal.scan@1` |
| `refs` (default) | quail-refs | `ref.references@1` |
| `parties` (default) | quail-parties | `party.candidates@1` |
| `sketch` (default) | quail-sketch | `sketch.minhash@1`, `sketch.simhash_text@1`, `sketch.simhash_template@1`, `sketch.layout@1`, `sketch.invoice_layout@1`, `sketch.text_key@1` (Fast); `sketch.minhash_sada@1` (Standard: not in `--profile fast`); `sketch.page_phash@1` not wired: needs the `raster` artifact, which the engine cannot see an extractor provide yet |
| `zstd` | (quail-io) | `--zstd` output |

Key docs (`src/keydocs.rs`) are generated from each crate's `keys` module by
`tools/genkeys.py`; a unit test fails when a crate declares a key that is not in the table.

## Numbers

See the pull request that introduced the crate: end-to-end files/s and peak RSS at 1, 4
and 8 threads on a private corpus of business documents (aggregates only).

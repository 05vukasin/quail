# Quickstart

Build quail, scan a folder, look at one file in detail, and choose how much work to spend per
file. Takes about fifteen minutes, most of it the first build.

## 1. Build

quail is built from source with a Rust toolchain (1.97 or newer; `rustup` picks the pinned
version from `rust-toolchain.toml`). The default build is pure Rust: no C compiler, no system
libraries, no models to download.

```sh
git clone https://github.com/05vukasin/quail
cd quail
cargo build --release -p quail-cli
export PATH="$PWD/target/release:$PATH"
quail version
```

`quail version` prints the steps of the pipeline and its fingerprint:

```
quail 0.1.0
schema       1
profile      standard
fingerprint  7799f963965d3df0
features     docx, pdf, text, signals, refs, parties, clauses, sketch, items, summary
steps
  engine            engine.sniff@2
  extractor pdf     pdf.extract@2
  extractor docx    docx.extract@1
  analyzer          hash.identity@1
  ...
```

The release build uses fat LTO and takes several minutes. For trying things out,
`cargo build -p quail-cli` is faster to build (dependencies are still optimized) and puts the
binary in `target/debug/`. Install it instead with `cargo install --path crates/quail-cli`.

## 2. Scan a folder

Point `quail scan` at one or more files or directories. It writes one JSON record per file:

```console
$ quail scan contracts -o records.jsonl
quail: 3 files (3 ok, 0 partial, 0 failed), 0.0 MB in 824.54ms: 3.6 files/s, 0.1 MB/s [15 threads]
```

Every file gets a record, whatever it is: a broken PDF or an unreadable file gets a `failed`
record that says why. PDF, DOCX and plain-text files are read in full; a `.txt` file gets its
encoding detected (`text.encoding`, with a confidence) and then the same analyzers as a PDF.
Other files get their format, MIME type and hashes. Each line of `records.jsonl` looks like this (shortened):

```json
{"schema":1,"locator":"contracts/amendment-2.pdf","size":37135,"format":"pdf","pages":1,
 "facts":[{"key":"engine.format","value":{"t":"text","v":"pdf"},"conf":1.0,"src":{"name":"engine.sniff","version":1}},
          {"key":"signal.iban","value":{"t":"text","v":"DE89370400440532013000"},"conf":0.99,
           "src":{"name":"signal.scan","version":4},
           "evidence":{"page":0,"zone":"body","span":{"start":775,"end":802},"bbox":{"x0":72.1,"y0":279.9,"x1":527.7,"y1":344.1}}},
          ...],
 "steps":[...],"timings":[...],"status":{"state":"ok"}}
```

[concepts.md](concepts.md) explains every field. Useful options:

```sh
quail scan /data --ext pdf,docx --skip-hidden --sorted -o records.jsonl   # filters, stable order
quail scan /data -j 4 --timeout 30s --json-stats -q -o records.jsonl      # 4 threads, stats as JSON
quail scan --manifest files.tsv -o records.jsonl                           # a fixed list of files
```

All options: [cli.md](cli.md#quail-scan).

## 3. Inspect one file

`quail inspect` processes one file and prints the record for a person, facts grouped by
namespace, with confidence, source and evidence:

```console
$ quail inspect contracts/amendment-2.pdf
file     contracts/amendment-2.pdf
format   pdf (application/pdf)   size 37.1 kB   pages 1
status   ok
steps    engine.sniff@2 (10 µs), pdf.extract@2 (9.3 ms), hash.identity@1 (111 µs), ...

[party] 5 fact(s)
  party.candidates  2 record(s)  src party.candidates@4
                      - {key="northwind logistics" surface="Northwind Logistics Ltd" score=0.99 roles=["customer", "signatory"]}
                      - {key="blue river software" surface="Blue River Software GmbH" score=0.99 roles=["supplier", "signatory"]}

[signal] 9 fact(s)
  signal.iban         "DE89370400440532013000"  conf 0.99  src signal.scan@5  at p1 body [775..802] box(72,280,528,344)
  signal.amount       4200.00 EUR  conf 0.97  src signal.scan@5  at p1 body [661..673] box(72,280,528,344)
  signal.date_signed  2023-09-12  conf 0.82  src signal.scan@5  at p1 body [110..127] box(72,130,543,201)
  ...
```

`at p1 body [775..802] box(72,280,528,344)` is the evidence: page 1, body zone, bytes 775 to 802
of the page text, and the box on the page in points. Add `--text` to see the page text itself,
block by block, so you can check what the span points at:

```sh
quail inspect contracts/amendment-2.pdf --text
quail inspect contracts/amendment-2.pdf --json --text > amendment.json   # record + document
```

## 4. Untrusted files: `--isolate`

By default all files are processed in one process. A panic in a parser costs only that file,
but an out-of-memory abort, a stack overflow or an endless loop would end the run. For files
you do not control, process every file in a worker process:

```console
$ quail scan contracts -j 2 --isolate -o records.jsonl
quail: 3 files (3 ok, 0 partial, 0 failed), 0.0 MB in 1.23s: 2.4 files/s, 0.0 MB/s [2 isolated workers]
```

A crashed worker gives its file a `failed` record (`worker crashed: out of memory: ...`), the
file is retried once in a fresh worker, and the run goes on. Records are otherwise the same as
without `--isolate`. Each worker loads its own models, so memory grows with `-j`; the cap per
worker is `--worker-mem` (default `1G`). Details: [security.md](security.md#process-isolation).

## 5. Duplicates: `--dedup`

Large document stores hold many byte-identical copies. With `--dedup`, a file whose bytes were
already processed in this run reuses that record:

```console
$ quail scan contracts --dedup -o records.jsonl
quail: 3 files (3 ok, 0 partial, 0 failed), 0.0 MB in 863.19ms: 3.5 files/s, 0.0 MB/s; 1 duplicates reused [15 threads]
  duplicates reused (--dedup): 1 (0 evicted from the cache)
```

The copy's record has its own `locator`, the fact `engine.duplicate_of` with the first file's
locator, and the step `engine.dedup@1`. `--dedup` cannot be combined with `--isolate`.

## 6. Profiles

`--profile` sets how much work to spend per file:

| profile | use it for |
|---|---|
| `fast` | a first pass over a large store: everything rule-based, without CPV categories for price lines, the compatibility MinHash and the page pHash |
| `standard` | the default: every analyzer of the default build |
| `deep` | `standard` plus OCR of the pages that need it, in a build with the `ocr` feature (section 7). In the default build it runs the same steps as `standard` |

```sh
quail scan /data --profile fast -o records.jsonl
quail version --profile fast        # the exact steps of a profile
```

Each step's lowest profile is in [keys.md](keys.md#steps).

## 7. OCR

Scanned pages have no text layer, and some text layers are garbage (for example Cyrillic
stored as Latin look-alike letters). The PDF extractor marks such pages (`needs_ocr` on the
page, `pdf.pages_needing_ocr` and `pdf.is_scanned` in the record), so you can find them
without OCR.

OCR (`ocr.pages@1`, PP-OCRv6 on ONNX Runtime, with a second recognizer for Serbian Cyrillic)
is in the `quail` binary behind the non-default cargo feature `ocr`. ONNX Runtime is a native
library: the `ort` build script downloads prebuilt binaries (Linux x64/arm64, macOS arm64,
Windows). The models (7 files, about 45 MiB, Apache-2.0) are downloaded separately and every
SHA-256 is checked:

```sh
cargo build --release -p quail-cli --features ocr
quail models fetch                 # into $QUAIL_OCR_MODELS, else the user cache dir (quail/ocr-models)
quail models list                  # each file: size, ok / missing / bad sha-256, license
quail scan contracts --profile deep -j 2 --timeout 15m -o records.jsonl
```

The sample amendment printed to a 200 DPI gray image and wrapped in a PDF has no text layer; with
OCR the same facts come back (excerpt):

```console
$ quail inspect scans/scanned-amendment.pdf --profile deep --timeout 15m
status   ok
steps    engine.sniff@2 (12 µs), pdf.extract@2 (707 µs), hash.identity@1 (165 µs), sketch.page_phash@3 (50.6 ms), ocr.pages@1 (5.87 s), ...
  ocr.page       {page=0 script="latin" mean_conf=0.9931 chars=974 lines=22 dpi=200 source="embedded_scan" rotation=0}  conf 0.99  src ocr.pages@1  at p1
  pdf.is_scanned           true  src pdf.extract@2
  signal.iban         "DE89370400440532013000"  conf 0.99  src signal.scan@5  at p1 body [775..802] box(72,333,258,343)
```

- OCR runs only under `--profile deep`, and only on pages that need it. The text analyzers run
  after it and read the OCR'd pages
  ([concepts.md](concepts.md#11-documents-ocr-and-the-revised-document)); each OCR'd page also
  gets an `ocr.page` fact with its mean confidence.
- **Memory**: about 0.6 to 0.8 GB per OCR worker (ONNX Runtime sessions and page images at
  200 to 300 DPI). On a 4 GB machine keep `-j` at 3 or below in deep mode.
- **Time**: 5 to 15 s per page per worker on a laptop CPU. The default `--timeout` of 2 minutes
  covers about 10 scanned pages; raise it for long scans. A file that hits the timeout keeps
  the pages already read and is `partial`.
- Without the feature or without the models, `--profile deep` prints one warning
  (`quail: warning: no OCR: ...`), and every file with a page needing OCR gets a problem and is
  `partial`.

Measured speed, memory and quality: [performance.md](performance.md#ocr).

## 8. LLM summary (opt-in)

Every record has a deterministic card (`summary.card_text`) and an English summary
(`summary.extractive`) made without a model. A more fluent summary written by a small local
LLM is available as the step `summary.llm@2` in `quail-summary`. It is off unless you ask for
it, three times over:

1. it is behind the cargo feature `llm`, which builds llama.cpp (needs CMake and a C++
   compiler);
2. it runs only in profile `deep`;
3. it does nothing unless its config has `enabled: true`.

The models (Qwen3.5 GGUF, Apache-2.0, pinned and SHA-256 checked) are downloaded separately:
`small` (0.53 GB) or `default` (1.28 GB):

```sh
cargo run -p quail-summary --features fetch --example fetch_models -- small
```

Every generated summary is fact-checked: each date, number, identifier and name in it must
appear in the card or in the text the model saw; otherwise the extractive summary is kept
(`method = extractive_fallback`). Unlike OCR, this step is not in the `quail` binary yet; a
program registers `quail_summary::SummaryLlm` on its own engine. Measured speed:
[performance.md](performance.md#llm-summary).

## Next

- [concepts.md](concepts.md): what every part of a record means.
- [keys.md](keys.md): every fact quail can produce.
- [evaluation.md](evaluation.md): how good the facts are, measured on public datasets.

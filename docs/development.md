# Development

How quail is built, and the rules every change follows. They exist so that many people can
work on it in parallel without stepping on each other.

## Layout

```
crates/
  quail-core     the contract: types and traits only. Every crate depends on it.
  quail-engine   format sniffing, DAG, limits, panics, parallelism, dedup   namespace engine.*
  quail-hash     identity: digests, content-defined chunks                  namespace hash.*
  quail-pdf      PDF extractor (vendored, hardened hayro)                   namespace pdf.*
  quail-docx     DOCX extractor                                             namespace docx.*
  quail-pptx     PPTX extractor                                             namespace pptx.*
  quail-ooxml    OOXML package, XML walk, relationships (DOCX and PPTX)
  quail-html     HTML extractor (html5ever tokenizer, EDGAR / inline XBRL)  namespace html.*
  quail-text     text utilities + text-level analyzers                      namespace text.*
  quail-signals  identifiers, dates, amounts, contacts                      namespace signal.*
  quail-refs     own numbers, cited documents, titles, instrument kind      namespace ref.*
  quail-parties  company and role candidates                                namespace party.*
  quail-clauses  governing law, term, notice, survival, termination         namespace clause.*
  quail-items    price lines with CPV categories                            namespace item.*
  quail-payment  subject, category, price, period and lines of a payment     namespace payment.*
  quail-contract key contract facts: parties, term, obligations, law         namespace contract.*
  quail-card     the normalised key-facts card                              namespace card.*
  quail-sketch   similarity sketches                                        namespace sketch.*
  quail-summary  file card, extractive and optional LLM summary             namespace summary.*
  quail-ocr      OCR of pages without a text layer (optional backend)       namespace ocr.*
  quail-io       sources (filesystem, manifest) and sinks (JSONL)
  quail-worker   process isolation (supervisor, workers, rlimits, watchdog)
  quail-cli      the `quail` binary (wires everything)
  quail-eval     evaluation harness and the `quail-eval` binary
  quail-fuzz     stable-Rust mutation fuzzer
vendor/          patched third-party crates (see "Vendored crates" below)
assets/          logo
docs/            documentation
tools/           scripts (dataset fetchers, CPV index, key reference)
```

## Dependency rules

- `quail-core` depends on nothing but `serde` and `thiserror`.
- A leaf crate (hash, text, pdf, docx, pptx, html, io, ...) depends on `quail-core` and, when
  it needs text utilities, on `quail-text`. Never on another leaf crate; `quail-ooxml` is the
  shared OOXML library of `quail-docx` and `quail-pptx` (no steps, no keys).
- Only `quail-engine` and `quail-cli` know concrete extractors and analyzers.
- A crate that pulls a native library (ONNX Runtime, llama.cpp, the C zstd library) puts it
  behind a cargo feature, so the default build is pure Rust and builds anywhere `cargo build`
  does.

## Ownership rules

- **One crate, one owner per change.** A change touches one crate's directory. Changing
  `quail-core` is its own change, reviewed on its own.
- **One namespace per crate.** A crate emits facts only under its namespace and declares every
  key once, as a `pub const` in its `keys` module:

  ```rust
  pub mod keys {
      use quail_core::FactKey;
      /// BLAKE3 digest of the whole file.
      pub const BLAKE3: FactKey = FactKey::new("hash.blake3");
  }
  ```

- **Analyzer ids** are `namespace.name@version`. Bump the version whenever the output for the
  same input can change.
- **Third-party dependencies** go in the crate's own `Cargo.toml` with an exact minor version.
  Add a dependency to `[workspace.dependencies]` only when a second crate needs it.

## Code rules

- One file at a time, no global mutable state. Models and dictionaries are loaded at
  construction and shared read-only (`Arc`).
- Report candidates, not verdicts: every inferred fact has a calibrated `conf` and, when it
  comes from content, `Evidence` (page, zone, span, box).
- Hostile input: no `unwrap`/`expect`/indexing that can panic on file content; respect every
  `Limits` field; call `Emitter::check_deadline` in per-page and per-chunk loops; prefer a
  partial result over an error. `Emitter::warn` is a note that changes nothing;
  `Emitter::problem` says the step stopped short (limit, deadline, broken page) and makes the
  record `Partial` while keeping everything it produced.
- Build `Page` values as `Page { text, blocks, ..Page::new(i, kind) }` so new fields never
  break extractors. Formats with geometry provide a `Raster` artifact (`PageRaster`) so OCR
  and inspection can render pages without depending on the extractor crate.
- Memory: stream or work page by page; never hold more than one rendered page per worker.
- Portability: never `target-cpu=native`. SIMD only behind runtime feature detection with a
  scalar fallback, tested on x86_64 and aarch64. CI runs Linux, macOS, Windows and Linux ARM.
- `unsafe` is denied workspace-wide. A crate that truly needs it allows it locally with a
  `// SAFETY:` comment on each block and says why in its PR.
- Performance claims come with a benchmark (`criterion` in the crate's `benches/`) and numbers
  in the PR.

## Tests and data

- Unit tests build their inputs in code (generated PDFs/DOCX, synthetic text). No real
  documents in the repository, ever: corpora live outside it (`../testdata`).
- Tests that need a corpus read its path from `QUAIL_TESTDATA` and skip when it is unset.
- Public benchmark datasets are fetched with the scripts in `tools/datasets/` (see its
  README); they are never committed.

## Workflow

1. Branch from `master` in a git worktree: `git worktree add ../wt/<crate>-<topic> -b <crate>/<topic>`.
2. Keep `make check` green (fmt, clippy with `-D warnings`, tests).
3. Open a pull request: what changed, which keys and analyzer ids were added, measured numbers.
4. Review, then squash-merge. `Cargo.lock` conflicts are resolved by the merger.

On a shared or small machine, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0` keeps memory and the
size of `target/` down.

### Adding or changing a fact key

1. Declare it in the crate's `keys` module with a doc comment: the first paragraph is the
   one-line documentation `quail keys` prints.
2. Regenerate the CLI's key table and the reference page:

   ```sh
   python3 crates/quail-cli/tools/genkeys.py && cargo fmt -p quail-cli
   make keys
   ```

   `make keys` builds the CLI and rewrites [keys.md](keys.md) from `quail keys --json`;
   `make keys-check` fails when the page is out of date. A unit test in `quail-cli` fails when a
   crate declares a key that the table does not document.
3. Bump the analyzer's version if existing output changes.

## Evaluation gate

Any change that can move a quality number (rules, dictionaries, models, extraction) comes with
a before/after comparison on the public dev splits ([evaluation.md](evaluation.md)):

```sh
quail scan --manifest kleister-nda.dev.manifest --sorted -o before.jsonl   # on master
quail scan --manifest kleister-nda.dev.manifest --sorted -o after.jsonl    # on the branch
quail-eval score --records before.jsonl --gold ~/quail-gold --label before --quiet
quail-eval score --records after.jsonl  --gold ~/quail-gold --label after --quiet
quail-eval compare eval/runs/before/scoreboard.json eval/runs/after/scoreboard.json --max-f1-drop 1.0
```

- `compare` exits 1 when any task's F1 drops by more than the threshold or failed records
  rise, and 2 when the two scoreboards were scored on different gold. A PR with exit 1
  explains the drop or does not merge.
- Paste `report.md` and the `compare` output into the PR. Both contain aggregates and BLAKE3
  prefixes only, never document text, so they are safe to publish even for a private corpus.
- Score `dev` only. The holdout (`--allow-holdout`) is for releases; tuning against it makes
  it worthless.
- Look at the files: `quail-eval inspect --where mismatch` shows the page text with every
  fact's evidence marked, next to the gold.

## Fuzzing

`quail-fuzz` runs the whole pipeline on mutated inputs, each case in a child process with
memory, CPU and wall-clock limits ([security.md](security.md#how-it-is-tested)). Build it with
the `fuzz` profile (optimized, with debug assertions and overflow checks on):

```sh
cargo build --profile fuzz -p quail-fuzz
B=target/fuzz/quail-fuzz
$B fuzz --out ../fuzz-out --jobs 2 --time 3600 --seed 1    # or --iterations N
$B run ../fuzz-out/crashes/*-min.*                         # re-classify saved findings
$B exec FILE                                               # in-process, e.g. under gdb
$B minimize FILE OUT
$B seeds DIR                                               # write the generated seeds
```

- Each new crash signature (`class@where`, e.g. `stack-overflow@docx.extract@2`) is saved under
  `<out>/crashes/` with a minimized copy and a line in `crashes/summary.tsv`.
- A fix comes with a regression test that builds the minimized input in code (the
  `tests/hostile.rs` suites of `quail-pdf`, `quail-docx`, `quail-pptx` and `quail-html`), not
  with the file itself.
- Run the fuzzer for at least an hour on any change to an extractor, a decoder or the
  vendored crates, and report the iterations and findings in the PR.
- Child-process limits are Unix-only; CPU limits and peak-RSS measurement are Linux-only.

## Vendored crates

`vendor/` holds copies of published crates with small patches: the hayro PDF crates
(hardening against hostile PDFs) and lingua (speed, identical output). The root `Cargo.toml`
routes their crates.io versions there with `[patch.crates-io]`; they are not workspace members,
so workspace lints and `cargo fmt` do not apply to them.

- **Only when a caller cannot fix it from outside.** Vendor a crate only for a problem that must
  be solved inside it: an allocation that has to be refused before it happens, a recursion that
  overflows the stack, a hot loop. Prefer an upstream fix; send the patch upstream too.
- **Pin the exact version** in the crate that uses it (`hayro = "=0.9.0"`), so the patches
  always apply to the code they were written for.
- **Mark every change** with a `// quail:` comment and describe it in `vendor/README.md`,
  file by file.
- **No change in output unless it is the point.** A speed patch must leave records
  byte-identical: compare `quail scan` records, minus `timings`, before and after on a corpus,
  at `-j1` and `-j4`.
- **Updating to a new upstream version:** copy the new sources over the directory, re-apply
  every `// quail:` change, run `cargo test -p quail-pdf` (its `tests/hostile.rs` regenerates
  the hostile inputs and fails if any gets through) or `cargo test -p quail-text` for lingua,
  and compare records as above.

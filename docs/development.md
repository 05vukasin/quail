# Development

How quail is built, and the rules every change follows. They exist so that many people (and
coding agents) can work in parallel without stepping on each other.

## Layout

```
crates/
  quail-core     the contract: types and traits only. Every crate depends on it.
  quail-hash     identity: digests, content-defined chunks          namespace hash.*
  quail-text     text utilities + text-level analyzers              namespace text.*
  quail-pdf      PDF extractor                                      namespace pdf.*
  quail-docx     DOCX extractor                                     namespace docx.*
  quail-engine   format sniffing, DAG, limits, panics, parallelism  namespace engine.*
  quail-io       sources (filesystem) and sinks (JSONL)
  quail-cli      the `quail` binary (wires everything)
assets/          logo
docs/            documentation
tools/           scripts (dataset fetchers, model fetchers)
```

## Dependency rules

- `quail-core` depends on nothing but `serde` and `thiserror`.
- A leaf crate (hash, text, pdf, docx, io, ...) depends on `quail-core` and, when it needs
  text utilities, on `quail-text`. Never on another leaf crate.
- Only `quail-engine` and `quail-cli` know concrete extractors and analyzers.
- A crate that pulls a native library (PDFium, ONNX Runtime) puts it behind a cargo feature,
  so the default build is pure Rust and builds anywhere `cargo build` does.

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

## Workflow

1. Branch from `master` in a git worktree: `git worktree add ../wt/<crate>-<topic> -b <crate>/<topic>`.
2. Keep `make check` green (fmt, clippy with `-D warnings`, tests).
3. Open a pull request: what changed, which keys and analyzer ids were added, measured numbers.
4. Review, then squash-merge. `Cargo.lock` conflicts are resolved by the merger.

# Performance

How fast quail is, how much memory it uses, and what was done to get there. Every number is
measured; the setup is stated with each table.

## Throughput and memory

`quail scan --sorted -q --json-stats`, `profiling` build (the release profile with thin LTO and
debug info; the default `release` profile uses fat LTO and is at least as fast), `master` at
commit `ed43afc` (2026-11-21), warm page cache, profile `standard` unless noted. Machine: AMD
Ryzen 7 7735U laptop CPU (8 cores, 16 threads), 14 GiB RAM, shared with other work (load
average 4 to 6 during the runs), so treat files/s as indicative (about ±10 %). Peak RSS is
sampled every 50 ms from `/proc` and summed over the process and its workers.

| corpus | files | MB | run | files/s | MB/s | peak RSS | ok / partial / failed |
|---|---:|---:|---|---:|---:|---:|---|
| CUAD (contracts, PDF) | 510 | 101 | `-j 4` | **54.6** | 10.8 | 197 MB | 505 / 5 / 0 |
| | | | `-j 1` | 17.6 | 3.5 | 139 MB | |
| | | | `-j 4 --profile fast` | 77.8 | 15.4 | 163 MB | |
| | | | `-j 4 --isolate` | 55.1 | 10.9 | 446 MB (largest worker 131 MB) | |
| Kleister-NDA (NDAs, PDF) | 726 | 47 | `-j 4` | **153.7** | 10.0 | 104 MB | 726 / 0 / 0 |
| RealKIE FCC (invoices, PDF, many scans) | 370 | 102 | `-j 4` | **104.1** | 28.6 | 235 MB | 370 / 0 / 0 |
| private corpus of ~400 business documents (PDF, DOCX, scans) | 417 | 216 | `-j 4` | **129.6** | 67.0 | 261 MB | 416 / 1 / 0 |
| | | | `-j 1` | 41.1 | 21.3 | 182 MB | |
| | | | `-j 4 --profile fast` | 208.3 | 107.7 | 162 MB | |
| | | | `-j 4 --isolate` | 108.4 | 56.1 | 480 MB (largest worker 132 MB) | |
| | | | `-j 4 --dedup` | 151.5 | 78.4 | 276 MB | 32 duplicates reused |

Notes:

- **Memory is bounded per run, not per corpus.** At most `--max-in-flight` files (2 x threads)
  and `--max-in-flight-bytes` (1 GiB) are in flight; every limit in `Limits` caps one file.
  With 4 threads, a run stays well under the 4 GB of a small machine.
- **Scaling.** From 1 to 4 threads: 3.1x on CUAD and 3.2x on the private corpus.
- **Where the time goes** (sum of the records' `timings`, `-j 4`): on CUAD, `pdf.extract` 34 %,
  the compatibility MinHash 14 %, `sketch.invoice_layout` 8 %, `text.lang` 8 %. On the private
  corpus, `pdf.extract` 35 %, `sketch.page_phash` 17 %, the compatibility MinHash 9 %. On the FCC
  invoices, most of them scans, `sketch.page_phash` takes 53 %.
- **`--profile fast`** leaves out the CPV categories of price lines, the compatibility MinHash
  and the page pHash: 42 % more files/s on CUAD, 61 % more on the private corpus.
- The 5 `partial` CUAD records are contracts that name more than 32 candidate companies; the
  32 strongest are kept.

## What was optimized

Each change below was measured when it was made, with records compared before and after
(minus `timings`): they were identical unless the step's version was bumped.

- **SIMD with runtime detection.** BLAKE3 is built with its `pure` feature (Rust intrinsics,
  no assembly) and picks SSE4.1, AVX2 or NEON at run time; the full identity of a file
  (BLAKE3, XXH3-128 and FastCDC chunks) runs at about 1.45 GiB/s on one core. The PDF
  extractor's byte scans (`startxref`, `/Encrypt`, JavaScript probes) use `memchr::memmem`,
  also SIMD with runtime detection and a scalar fallback; together with fewer allocations per
  glyph that cut extraction time by 11 to 19 % end to end. Nothing is compiled with
  `target-cpu=native`, so one binary runs on every CPU of its architecture.
- **Shared normalization.** Each page used to be normalized about 8 times and tokenized 5 to 8
  times per file, once per analyzer. Now `text.stats` builds a `normalized_text` artifact
  (normalized text, offset map, tokens and their keys) and the language, reference, party and
  sketch analyzers reuse it: their summed step time fell by 40 % on the private corpus and
  49 % on CUAD.
- **Language detection.** `text.lang` spent most of its time in lingua's per-language n-gram
  lookups. The vendored, patched lingua looks up every n-gram once for all languages in one
  table and sums in a fixed order: 3.6 to 4.9 times faster, with bit-identical output.
- **Page pHash of scans.** The 32 x 32 thumbnail filter of `sketch.page_phash` walks
  precomputed cells in integer arithmetic (30 times faster, bit-identical), and the step
  hashes at most 4 pages per file (40 to 58 % less step time).
- **CPV index.** The index behind the price-line categories went from about 47 MB to 9 MB
  of RSS, built once per process and shared.
- **Content dedup.** `--dedup` hashes each file first and reuses the record of identical
  bytes: on the private corpus, with 32 duplicates among 417 files, 17 % more files/s in the
  table above.

## Process isolation

`--isolate` runs every file in a worker process ([architecture.md](architecture.md#worker-isolation)).
Its cost has three parts:

- **Per file**: two pipe round trips and the JSON round trip of the record. With a trivial
  engine on 2 KiB text files that is 34 to 81 µs per file (`cargo bench -p quail-worker`).
  Real documents take milliseconds each, so the cost disappears: on CUAD isolated and
  in-process runs had the same files/s (55.1 against 54.6); on the private corpus, with many
  small files, isolation cost 16 %.
- **Memory**: each worker loads its own models (language detection, CPV index), about 100 to
  130 MB per worker. At 4 workers that is 446 to 480 MB in total, against 197 to 261 MB in
  process.
- **Crashes**: a crashed worker is replaced, which costs a process start and the engine build.

Use `--isolate` for untrusted input; for a trusted corpus on a tight memory budget, run in
process.

## OCR

Measured when the OCR backend was added (`quail-ocr`, PP-OCRv6 on ONNX Runtime): on 32 scanned
pages from the private corpus, one worker read 0.175 pages/s (5.2 s per page) with a peak RSS
of about 850 MB; two workers read 0.294 pages/s. Mean recognition confidence was 0.976. OCR
is two orders of magnitude slower than everything else together, which is why it runs only
in profile `deep` and only on pages that need it.

## LLM summary

Measured when `summary.llm` was added: 30 English CUAD dev documents, 4 threads, about 1,000
prompt tokens and 70 output tokens per document. Qwen3.5-2B: 25 s per document (median), 2.1 GB
peak RSS, 28 of 30 summaries passed the fact check. Qwen3.5-0.8B: 14 s per document, 1.1 GB,
30 of 30. The deterministic card and extractive summary take 0.4 ms and 13 ms per file
(median, debug build) by comparison.

## Measuring

```sh
cargo build --profile profiling -p quail-cli
target/profiling/quail scan /data --sorted -j 4 -q --json-stats -o /dev/null
```

`--json-stats` prints files, bytes, files/s and MB/s. Per-step latency percentiles of a run are
in the `quail-eval score` report ([evaluation.md](evaluation.md#metrics)). Crates with hot paths
have `criterion` benchmarks (`cargo bench -p quail-hash`, `-p quail-pdf`, `-p quail-sketch`,
`-p quail-items`, `-p quail-worker`, `-p quail-engine`, `-p quail-io`). A pull request that
claims a speed-up includes before/after numbers from such a run.

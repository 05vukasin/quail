# Performance

How fast quail is, how much memory it uses, and what was done to get there. Every number is
measured; the setup is stated with each table.

## Throughput and memory

`quail scan --sorted -q --json-stats`, `profiling` build (the release profile with thin LTO and
debug info; the default `release` profile uses fat LTO and is at least as fast), `master` at
commit `51821a4` (2026-11-21), warm page cache, profile `standard` unless noted. Machine: AMD
Ryzen 7 7735U laptop CPU (8 cores, 16 threads), 14 GiB RAM, shared with other work (load
average about 6 to 7 during the runs), so treat files/s as indicative (about ±10 %). Peak RSS is
sampled every 50 ms from `/proc` and summed over the process and its workers.

| corpus | files | MB | run | files/s | MB/s | peak RSS | ok / partial / failed |
|---|---:|---:|---|---:|---:|---:|---|
| CUAD (contracts, PDF) | 510 | 101 | `-j 4` | **53.6** | 10.6 | 163 MB | 505 / 5 / 0 |
| | | | `-j 1` | 17.3 | 3.4 | 111 MB | |
| | | | `-j 4 --profile fast` | 72.5 | 14.4 | 161 MB | |
| | | | `-j 4 --isolate` | 53.8 | 10.7 | 439 MB (largest worker 127 MB) | |
| Kleister-NDA (NDAs, PDF) | 726 | 47 | `-j 4` | **156.2** | 10.2 | 105 MB | 726 / 0 / 0 |
| RealKIE FCC (invoices, PDF, many scans) | 370 | 102 | `-j 4` | **115.6** | 31.8 | 225 MB | 370 / 0 / 0 |
| private corpus of ~400 business documents (PDF, DOCX, scans) | 417 | 216 | `-j 4` | **128.6** | 66.5 | 248 MB | 416 / 1 / 0 |
| | | | `-j 1` | 43.4 | 22.5 | 172 MB | |
| | | | `-j 4 --profile fast` | 195.4 | 101.1 | 155 MB | |
| | | | `-j 4 --isolate` | 109.0 | 56.4 | 486 MB (largest worker 138 MB) | |
| | | | `-j 4 --dedup` | 142.3 | 73.6 | 271 MB | 32 duplicates reused |

Notes:

- **Memory is bounded per run, not per corpus.** At most `--max-in-flight` files (2 x threads)
  and `--max-in-flight-bytes` (1 GiB) are in flight; every limit in `Limits` caps one file.
  With 4 threads, a run stays well under the 4 GB of a small machine.
- **Scaling.** From 1 to 4 threads: 3.1x on CUAD and 3.0x on the private corpus.
- **Where the time goes** (sum of the records' `timings`, `-j 4`): on CUAD, `pdf.extract` 35 %,
  the compatibility MinHash 13 %, `sketch.invoice_layout` 8 %, `text.lang` 7 %. On the private
  corpus, `pdf.extract` 33 %, `sketch.page_phash` 16 %, the compatibility MinHash 9 %. On the FCC
  invoices, most of them scans, `sketch.page_phash` takes 54 %.
- **`--profile fast`** leaves out the CPV categories of price lines, the compatibility MinHash
  and the page pHash: 35 % more files/s on CUAD, 52 % more on the private corpus.
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
- **Text at the limit** (#142). A file whose text reaches `max_text_bytes` (64 MiB) used to
  push the text analyzers past 1 GiB (a 280 KB PPTX aborted on a 512 MiB allocation in
  `signal.scan`): per-token buffers of 16 to 48 bytes a token, several copies of the text, a
  `Vec` of every 3-word window. Now the `normalized_text` artifact holds at most 8 MiB of page
  text (about 40 MB) and no page over 4 MiB; `signal.scan` and `ref.references` read a longer
  page in 4 MiB pieces cut at blank lines or line breaks (`quail_text::piece`); the sketches
  normalize one page or piece at a time, hash the joined words as they come and keep only
  shingle digests; the compatibility sketches stream the text instead of joining it;
  `party.candidates`, `summary.keywords` and `summary.extractive` read the first 4 MiB of
  such a page. Peaks on one page of 64 MiB of generated text (optimized build): `signal.scan`
  711 → 49 MiB, `sketch.minhash_compat` 665 → 74, `ref.references` 579 → 83,
  `sketch.invoice_layout` 319 → 9, `text.stats` 315 → 8, `party.candidates` 252 → 63, and
  every other step under 100 MiB except the fixed-cap `contract.facts` (160, at most 8 MiB of
  text). The process peak fell from 1.10 GB to 0.46 GB of RSS. Records are identical on pages
  up to 4 MiB; `crates/quail-cli/tests/text_memory.rs` asserts the bounds.
- **Content dedup.** `--dedup` hashes each file first and reuses the record of identical
  bytes: on the private corpus, with 32 duplicates among 417 files, 11 % more files/s in the
  table above.

## Process isolation

`--isolate` runs every file in a worker process ([architecture.md](architecture.md#worker-isolation)).
Its cost has three parts:

- **Per file**: two pipe round trips and the JSON round trip of the record. With a trivial
  engine on 2 KiB text files that is 34 to 81 µs per file (`cargo bench -p quail-worker`).
  Real documents take milliseconds each, so the cost disappears: on CUAD isolated and
  in-process runs had the same files/s (53.8 against 53.6); on the private corpus, with many
  small files, isolation cost 15 %.
- **Memory**: each worker loads its own models (language detection, CPV index), about 100 to
  140 MB per worker. At 4 workers that is 439 to 486 MB in total, against 163 to 248 MB in
  process.
- **Crashes**: a crashed worker is replaced, which costs a process start and the engine build.

Use `--isolate` for untrusted input; for a trusted corpus on a tight memory budget, run in
process.

## OCR

`quail --features ocr --profile deep -j 2 --timeout 15m`, dev build (the OCR inference is native either way),
on the RealKIE FCC dev files, the same shared laptop. Measured when OCR was wired into the CLI,
with `item.lines@4`:

| | without OCR | with OCR |
|---|---:|---:|
| files / pages / pages OCR'd | 296 / 1,373 / 0 | 296 / 1,373 / 701 |
| wall time | 31.6 s | 3,618 s |
| per OCR'd page | | 10.2 s per worker, 5.1 s wall with 2 workers |
| peak RSS | 177 MB | 1,835 MB |
| `items` F1 (P / R) | 46.2 (60.6 / 37.4) | 49.7 (54.3 / 45.7) |
| `items` F1 on price and page only | 52.2 | 64.0 |
| documents with gold rows and no prediction | 49.3 % | 15.7 % |

- OCR is two orders of magnitude slower than everything else together, which is why it runs
  only in profile `deep` and only on pages that need it.

### Automatic budgets (`ocr.pages@2`)

The same dev build and laptop (16 logical CPUs, 14 GiB, shared with other work: load average
4 to 15 during the runs, so times are indicative). The earlier runs needed `-j 2 --timeout 15m`
by hand; with `ocr.pages@2` the command is just `quail scan --profile deep` (default `-j` = 15).
quail printed `2 page(s) at a time, 4 thread(s) each; up to 40 pages per file, 45s per page;
timeout 120m per file`. Peak RSS is summed over the process tree, sampled every 0.5 s.

| | RealKIE FCC dev, by hand | RealKIE FCC dev, defaults | Serbian dev, by hand | Serbian dev, defaults |
|---|---:|---:|---:|---:|
| files / pages OCR'd | 296 / 701 | 296 / 701 | 91 / 58 | 91 / 58 |
| wall time | 3,618 s | **2,311 s** | 481 s | **388 s** |
| files/s | 0.082 | 0.128 | 0.189 | 0.235 |
| wall per OCR'd page | 5.1 s | 3.3 s | 8.3 s | 6.7 s |
| peak RSS | 1,835 MB | 2,449 MB | 1,287 MB | 1,428 MB |
| partial / timeouts | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |

- OCR output is the same: `ocr.pages` and `ocr.chars` are identical on all 387 files, and so
  are the OCR-fed Serbian tasks (`tax_id` 99.0, `registration_number` 97.0, `bank_account`
  94.3 F1). `items` on RealKIE moved from 49.7 to 60.7 F1 and Serbian `party` from 32.1 to
  29.5 between the two runs, from `item.lines@5` and `party.candidates@4` on master, not from
  OCR.
- **Threads.** One 4-page FCC scan (`quail inspect --profile deep`, one job): 15.8 s per page
  with 1 intra-op thread, 7.5 s with 4 (the default for one job here), 6.3 s with 8. The output
  is identical across thread counts. Text lines are recognized one at a time
  (`PaddleConfig::region_batch` 1): batches are padded to their widest line.
- **Memory.** A job peaks at about 1 GB with 4 threads (3 jobs reached 3.3 to 3.5 GB on the
  same split, 2 jobs 2.4 GB), so quail plans 1 GiB per job within 40 % of the available and
  20 % of the total memory: 2 jobs on this 14 GiB laptop, 1 on a 4 GB machine.
- **Time.** 5 to 15 s per page on an idle CPU, 20 to 45 s on a loaded one. At 20 s per page 3
  of 91 Serbian files went partial under load; at 45 s none did. OCR keeps
  `max(10 s, 15 %)` of the file timeout for the analyzers after it, so a file that runs out of
  time is `partial` with all its text facts for the pages read (before, the 4-page scan above
  with the 2-minute default timeout on a loaded machine got OCR text and then
  `engine: timeout; 17 analyzer(s) not run`).

### Idle engines and the OCR thread budget (#115, #116)

`tests/paddle.rs` `measure` (ignored; how to run it is in its header), test profile
(dependencies at `opt-level = 2`), one process, a 4 vCPU AMD EPYC server with 7.6 GiB, single
runs. 12 warm pages of generated invoice text (Helvetica 11 pt, about 1500 characters) rendered
at 300 DPI, read by `jobs` engines side by side. "Cores busy" is the process's CPU time over the
wall time; "output" is a BLAKE3 of every page's `OcrPage` as JSON, the same in every row.

| code | jobs x threads | pages/s | CPU s a page | cores busy | RSS after the burst | RSS after the idle timeout |
|---|---|---|---|---|---|---|
| before (`62108d4`) | 1 x 4 | 0.39 | 5.85 | 2.30 | 655 MiB | 655 MiB (engine kept) |
| after | 1 x 4 | 0.39 | 5.78 | 2.27 | 630 MiB | 217 MiB (5 s timeout) |
| after, doewood's `GLIBC_TUNABLES` | 1 x 4 | 0.39 | 5.83 | 2.27 | 604 MiB | 155 MiB |
| before | 2 x 2 | 0.61 | 5.10 | 3.08 | 1147 MiB | 1147 MiB (engines kept) |
| after | 2 x 2 | 0.63 | 4.96 | 3.14 | 1154 MiB | 283 MiB |

The RSS before the engines was 120 to 127 MiB (the test's page images and the models' files).

- **Idle engines (#115).** Dropping an idle engine gives back 410 to 450 MiB per engine with
  glibc's defaults, and almost all of it (to 36 MiB above the start) with doewood's
  `arena_max=2:mmap_threshold=131072`. Before, the memory stayed for the life of the process.
  The default timeout is 5 minutes (`EnginePool::DEFAULT_IDLE_TIMEOUT`); a reload costs a
  model load (0.25 s) and a slower first page.
- **Thread budget (#116).** On an idle machine the speed is the same (the engine's own pool
  replaces the global one thread for thread). Before, the pre- and post-processing of every
  engine ran on rayon's global pool (one thread per CPU, shared with the host and the other
  engines); now it runs on the engine's pool of `threads` threads, so `jobs × threads` bounds
  OCR's threads even when the host's other workers are busy, and the global pool is never
  started (`engines_never_start_the_global_rayon_pool` fails on `62108d4`).

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

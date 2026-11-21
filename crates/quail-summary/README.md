# quail-summary

Per-file **cards** and **English summaries** for LLM agents. Namespace `summary.*`. A card is
what an agent reads first about a file (~100–300 tokens); the summary says in 1–3 sentences what
the document is. Both are built from the facts of the other crates and the start of the text,
deterministically and without a model. An optional local LLM (feature `llm`, opt-in) writes a
more fluent abstract, which is fact-checked before it is used.

Summaries are always in English. Names, numbers and identifiers are copied verbatim, in their
original script; a non-English subject sentence is quoted and marked with its language.

## Analyzers

| id | profile | requires / after | emits |
|---|---|---|---|
| `summary.card@1` (`SummaryCard`) | Fast | `document` / `revised_document ocr_text raster` | `summary.card`, `summary.card_text`, `summary.card_tokens` |
| `summary.extractive@1` (`SummaryExtractive`) | Fast | same | `summary.extractive`, `summary.subject` |
| `summary.llm@1` (`SummaryLlm`, feature `llm`) | Deep, opt-in | same | `summary.llm` |

**Ordering.** They read `ctx.facts`, so they must run after every fact producer. The engine
orders analyzers only by artifacts and keeps registration order among equals; the fact producers
provide no artifacts. The summary steps therefore declare `after` on every artifact a workspace
step provides (`AFTER`) and must be **registered last** (`tests/engine.rs` checks the plan). A
core "finalizer" flag that makes this explicit is requested in the PR.

## Keys

| key | value |
|---|---|
| `summary.card` | record, fields below (absent ones left out) |
| `summary.card_text` | Text: compact `key: value` lines, at most 300 estimated tokens |
| `summary.card_tokens` | Int: estimated tokens of `card_text` |
| `summary.extractive` | Text: template sentence + subject sentence |
| `summary.subject` | record `text lang? method label?` (`method`: `clause` / `textrank`), evidence: page |
| `summary.llm` | record `text subject? title_en? method model checked cached` (`method`: `llm` / `extractive_fallback`) |

`summary.card` fields, in this order:

| field | value | from |
|---|---|---|
| `format` | `pdf`, `docx`, ... | `engine.format` |
| `kind`, `ordinal` | instrument kind, Amendment No. → Int | `ref.instrument_kind`, `ref.ordinal` |
| `title`, `title_source` | as written; `text` or `metadata` | `ref.title`, else `pdf.title` / `docx.title` |
| `lang`, `script` | `en`, `sr-Latn`, `sr-Cyrl`, ...; `latin`, `cyrillic`, `mixed` | `text.lang`, `text.script` |
| `pages` | Int | `pdf.page_count`, `docx.pages`, document pages |
| `number` | record `raw kind?` | best `ref.doc_number` |
| `cites`, `cites_total` | list of `relation kind? raw ordinal? date?` (parent relations first, ≤ 6) | `ref.cites` |
| `parties`, `parties_total` | list of `name role? score` (score ≥ 0.3, ≤ 4) | `party.candidates`, else `party.company`, else the DOCX CLM fields |
| `dates` | list of `role date`: `signed effective expires issued due`, or `date` | `signal.date_*`, else best `signal.date` |
| `amounts` | list of `role amount`: `total net gross` (≤ 2 each), or `max` | `signal.amount_*`, else largest `signal.amount` in the main currency |
| `currencies` | list, most frequent first | `signal.amount` |
| `ids` | list of `kind values count` (`pib mb vat iban oib lei ein duns account bic`, ≤ 2 values) | `signal.*` |
| `personal_ids` | Int, count only | `signal.jmbg_count` |
| `items` | record `count categories` (top 3 CPV `code label lines`) | `item.count`, `item.line` |
| `sketch` | record `simhash_text simhash_template text_key` (first 8 bytes, hex) | `sketch.*` |
| `producer`, `author`, `company` | Text | `pdf.producer` / `docx.application` / `pdf.creator`; `pdf.author` / `docx.creator`; `docx.company` |
| `flags` | list: `scanned(n) ocr(n) needs_ocr(n) encrypted signed signature_fields(n) macros javascript hidden_text(n) tracked_changes(n) comments(n) embedded(n) updated(n) protected garbled_text` | `pdf.*`, `docx.*`, `ocr.pages`, `text.garbage_ratio` |

The input keys are plain strings in `src/input.rs` (leaf crates do not depend on each other);
`tests/keys.rs` checks that each one is declared in its owner crate.

## Card text

```
doc: amendment #2 no=114/23-A2 "Amendment No. 2 to the Master Services Agreement" lang=en pages=3 pdf
parties: customer=Northwind Logistics Ltd; supplier=Blue River Software GmbH
dates: effective=2023-09-12 expires=2026-12-31
amounts: total=EUR 4,200.00
refs: amends=msa 114/23; references=po PO-77812
ids: vat=DE136695976 iban=DE89370400440532013000,GB82WEST12345698765432 (+1)
flags: signed
meta: producer="Microsoft® Word 2016"
sketch: text=9f3a61c2b7d40e15 tmpl=11c07a3e5b92d468
```

Per-field limits (title 100 characters, names 60, labels 40, list lengths, `+N` for the rest),
then whole lines are dropped from the least useful up (`sketch`, `meta`, `items`, `flags`, `ids`,
`refs`, `amounts`, `dates`, `parties`) until the text fits the budget; `doc` is clipped last.

**Token estimate** (`tokens::estimate`): per whitespace-separated chunk, ASCII letters and digits
weigh 1, every other character 2, and a chunk costs `ceil(weight / 4)` tokens (at least 1). No
tokenizer is loaded.

## Extractive summary

- Template sentence from the card: kind (+ No. ordinal, own number; the title under the generic
  `contract` or when nothing else identifies the document), parent document ("to the master
  services agreement 114/23"), up to three parties with roles ("and others" beyond), dates by
  role, up to two amounts, `scanned`. It adds no number the card does not hold.
- Subject sentence: the first sentence under a subject heading or label (`Subject of the
  Agreement`, `Subject matter`, `Purpose`, `Scope of Work`, `Description of Services`,
  `Article 1` + title line, `Predmet ugovora`, `Predmet`, `Svrha`, `Član 1`, in Latin or Cyrillic,
  folded), at most 40 words; else the top sentence of a `TextRank` over the first 4 pages
  (≤ 150 sentences of 8–60 words; definitions, parties blocks, addresses and signature lines
  filtered out; clause numbers stripped; a position prior, tripled for sentences that say who
  provides what: "agrees to", "shall provide", "hereby", "grants", "whereas", ...).
- English documents: `… Subject: <sentence>`; others: `… Subject (sr, original): "<sentence>"`.
  Invoices, orders, quotes and price lists get the template sentence alone (their prose is terms
  and conditions); `summary.subject` is still emitted.
- Card tweaks that keep it useful: parties whose name words are all inside a stronger party's
  name are merged ("Cisco" into "Cisco Systems, Inc."); item categories need a top score ≥ 0.3;
  a role-less largest amount needs ≥ 100.

## LLM summary (feature `llm`)

- `LlmSummarizer` on `llama-cpp-2 =0.1.161` (`dynamic-backends`: one CPU backend per
  microarchitecture, chosen at load time; `QUAIL_LLM_BACKENDS` overrides the backend directory).
  Builds llama.cpp with CMake and a C++ compiler, so it is never in the default build.
- Models (`models::MANIFEST`, URLs pinned to a Hugging Face commit, SHA-256 checked before
  loading, Apache-2.0): `default` = Qwen3.5-2B `Q4_K_M` (1.28 GB), `small` = Qwen3.5-0.8B `Q4_K_M`
  (0.53 GB). `cargo run -p quail-summary --features fetch --example fetch_models -- [small|default|all] [DIR]`;
  the directory is `$QUAIL_LLM_MODELS` or the user cache (`quail/llm-models`).
- Prompt: fixed instructions first, then the card text and the first ~800 estimated tokens of
  text; greedy decoding, thinking off, output constrained by a grammar compiled from the JSON
  schema `{summary ≤ 400 chars, subject ≤ 120, title_en ≤ 120}`.
- **Fact check** (`factcheck::check`) of every output: each date, number, identifier and name
  word must be found in the card text or the text the model saw; otherwise the extractive
  summary is emitted with `method = extractive_fallback` and a warning. Timeouts (60 s), bad JSON
  and load errors fall back the same way; a summary never fails the file.
- Cache: `BLAKE3(card text ‖ text head ‖ summarizer id)`, the id carrying the model SHA-256, the
  prompt version and the decoding settings (`MemoryCache`, or any `SummaryCache`).
- Opt-in: `SummaryLlm` applies only with `LlmConfig::enabled`; invoices and purchase orders are
  skipped (their card says everything); one generation at a time per model.

## Measured

Dev build (dependencies at opt-level 2), shared 16-core laptop. Aggregates only.

| corpus | files | card tokens p10 / p50 / p90 / max | extractive fact check | card / extractive time p50 |
|---|---|---|---|---|
| CUAD dev | 407 | 114 / 140 / 178 / 248 | 407 / 407 | 62 µs / 1.7 ms |
| private business files | 380 | 110 / 185 / 230 / 275 | 380 / 380 | 59 µs / 0.3 ms |
| serbian-public | 362 | 34 / 147 / 253 / 299 | 361 / 362 (checker artefact on a copied date) | 63 µs / 0.6 ms |

LLM, 30 English CUAD dev documents, 4 threads, one model at a time, ~1,000 prompt tokens,
~70 output tokens: Qwen3.5-2B 25 s/doc p50 (max 33 s), 2.1 GB peak RSS, fact check 28/30 (one
real catch: a name rewritten, one false alarm); Qwen3.5-0.8B 14 s/doc p50, 1.1 GB, 30/30.
The fact check verifies that values are grounded, not that relations between them are right.

## Limits

- Reads the first 4 pages and at most 48 KiB of normalized text for the subject; the LLM sees at
  most `head_tokens` of it. Checks the deadline before the subject search and during generation.
- Card text: hard budget (default 300 estimated tokens, `SummaryCard::with_max_tokens`).
- No panics on any fact shape: unexpected value types are skipped.

# quail-summary

Per-file **cards** and **English summaries** for LLM agents. Namespace `summary.*`. A card is
what an agent reads first about a file (~100–300 tokens); the summary says in 1–3 sentences what
the document is. Both are built from the facts of the other crates and the start of the text,
deterministically and without a model. For documents of **any length** (2 or 2,000 pages) the
crate also builds the document's outline and a bounded **digest**: the input a small local model
needs to summarize it. An optional local LLM (feature `llm`, opt-in) writes a more fluent
abstract from the digest (or part by part, map-reduce), which is fact-checked before it is used.

Summaries are always in English. Names, numbers and identifiers are copied verbatim, in their
original script; a non-English subject sentence is quoted and marked with its language.

## Analyzers

| id | profile | requires / after | emits |
|---|---|---|---|
| `summary.card@3` (`SummaryCard`) | Fast | `document` / `revised_document ocr_text raster normalized_text summary_terms` | `summary.card`, `summary.card_text`, `summary.card_tokens` |
| `summary.extractive@2` (`SummaryExtractive`) | Fast | `document` / `revised_document ocr_text raster normalized_text` | `summary.extractive`, `summary.subject` |
| `summary.outline@1` (`SummaryOutline`) | Fast | same | `summary.outline` |
| `summary.keywords@1` (`SummaryKeywords`) | Fast | same; provides `summary_terms` | `summary.keywords` |
| `summary.topics@1` (`SummaryTopics`) | Standard | `document` / same + `summary_terms` | `summary.topics` |
| `summary.digest@2` (`SummaryDigest`) | Standard | `document` / same + `summary_terms` | `summary.digest`, `summary.digest_tokens` |
| `summary.llm@3` (`SummaryLlm`, feature `llm`) | Deep, opt-in | `document` / `revised_document ocr_text raster normalized_text` | `summary.keywords` | list of records `term key score count first_page entities?`, best first (below) |
| `summary.topics` | record `method threshold sections segments truncated?`: `sections` list of `title page end_page keywords shift`, `segments` list of `title first sections page end_page keywords shift` (pages zero-based, `first` = index of the segment's first section) |
| `summary.llm` |

**Ordering.** They read `ctx.facts`, so they must run after every fact producer. All of them are
finalizers (`Analyzer::finalizer`): the engine runs them after every non-finalizer step,
whatever the registration order. They also declare `after` on every artifact a workspace step
provides (`AFTER`), so they see OCR text when OCR is active (`tests/engine.rs` checks the plan).
The card, topic and digest steps also run after `summary.keywords` (`AFTER_TERMS`, the
`summary_terms` artifact): the card prints the keywords, the other two reuse its term index
(and build the same index themselves when it is absent).

`quail-cli` registers `summary.outline@1`, `summary.keywords@1` and `summary.topics@1` by
default (`summary.topics` is Standard: left out under `--profile fast`). `summary.digest@2`
is Standard and not in the CLI pipeline: it costs a few ms per file and adds ~6.5 KB to every
record; a service that feeds a model registers it (`SummaryDigest::with_config` sets the
budget and a minimum document size).

## Keys

| key | value |
|---|---|
| `summary.card` | record, fields below (absent ones left out) |
| `summary.card_text` | Text: compact `key: value` lines, at most 300 estimated tokens |
| `summary.card_tokens` | Int: estimated tokens of `card_text` |
| `summary.extractive` | Text: template sentence + subject sentence |
| `summary.subject` | record `text lang? method label?` (`method`: `clause` / `recital` / `operative` / `textrank`), evidence: page |
| `summary.outline` | record `method total pages sections truncated?`: `method` `headings` / `windows`, `total` headings found, `sections` list of `title level page end_page source` (pages zero-based; `source` `block` / `keyword` / `numbered` / `capitals` / `size` / `window`) |
| `summary.digest` | Text: the bounded model input, at most 2,000 estimated tokens by default (below) |
| `summary.digest_tokens` | Int: estimated tokens of `summary.digest` |
| `summary.llm` | record `text subject? title_en? method model checked cached input calls` (`method`: `llm` / `extractive_fallback`; `input`: `digest` / `map_reduce`) |

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
| `keywords` | list of terms, best first (≤ 8) | `summary.keywords` |
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
keywords: hosted platform, service levels, support tickets, uptime credits
ids: vat=DE136695976 iban=DE89370400440532013000,GB82WEST12345698765432 (+1)
flags: signed
meta: producer="Microsoft® Word 2016"
sketch: text=9f3a61c2b7d40e15 tmpl=11c07a3e5b92d468
```

Per-field limits (title 100 characters, names 60, labels 40, list lengths, `+N` for the rest),
then whole lines are dropped from the least useful up (`sketch`, `meta`, `items`, `flags`, `ids`, `keywords`,
`refs`, `amounts`, `dates`, `parties`) until the text fits the budget; `doc` is clipped last.

**Token estimate** (`tokens::estimate`): per whitespace-separated chunk, ASCII letters and digits
weigh 1, every other character 2, and a chunk costs `ceil(weight / 4)` tokens (at least 1). No
tokenizer is loaded.

## Extractive summary

- Template sentence from the card: kind (+ No. ordinal, own number; the title under the generic
  `contract` or when nothing else identifies the document), parent document ("to the master
  services agreement 114/23"), up to three parties with roles ("and others" beyond), dates by
  role, up to two amounts, `scanned`. It adds no number the card does not hold.
- Subject sentence, in this order (`summary.subject.method`):
  1. `clause`: the first sentence under a subject, purpose or scope heading or label
     (`Subject of the Agreement`, `Subject matter`, `Purpose`, `Scope of Work`, `Description of
     Services`, `Appointment`, `Grant of License`, `Services`, `Predmet ugovora`, `Predmet`,
     `Svrha`, in Latin or Cyrillic, folded), at most 40 words; a boilerplate sentence under it is
     skipped;
  2. `recital`: the last `WHEREAS` recital before the operative part that states an intent
     (`desires to`, `wishes to`, `intends`, `agree`), its first sentence, recital letter
     dropped; or a `The Company wishes to …` sentence in prose;
  3. `operative`: the first sentence after `NOW, THEREFORE … agree as follows:` (or `In
     consideration of …`, `It is agreed`) that is no heading, definition, list fragment or
     boilerplate, run-in heading dropped (`2. SUPPLY. Supplier shall …`);
  4. `clause` under a weak heading (`Background`, `Article 1` + title line, `Član 1`);
  5. `textrank`: the top sentence of a `TextRank` over the first 4 pages (≤ 150 sentences of
     8–60 words; definitions, parties blocks, addresses and signature lines filtered out; clause
     numbers stripped; a position prior, tripled for sentences that say who provides what,
     and boilerplate (notices, counterparts, governing law, severability, entire agreement,
     redaction notes, waiver, assignment, indemnity) and list fragments ranked down to 5 %;
     the best sentence that is neither wins when there is one).
- The first pages come from the shared `normalized_text` artifact of `text.stats` when it holds
  the whole page (same text; normalized here otherwise).
- English documents: `… Subject: <sentence>`; others: `… Subject (sr, original): "<sentence>"`.
  Invoices, orders, quotes and price lists get the template sentence alone (their prose is terms
  and conditions); `summary.subject` is still emitted.
- Card tweaks that keep it useful: parties whose name words are all inside a stronger party's
  name are merged ("Cisco" into "Cisco Systems, Inc."); a name cut at its start (`II, LLC`,
  `GP, LLC`, `, Inc.`) is left out; an expiry date before the effective (else signed) date is
  left out; item categories need a top score ≥ 0.3; a role-less largest amount needs ≥ 100.

## Outline (`summary.outline@1`)

One pass over every line of every page (no model, linear time). A line is a heading when it is:

- a DOCX heading block (`BlockKind::Heading`, its level);
- a keyword heading: `PART II`, `CHAPTER 3`, `Article 5`, `Section 2.1`, `SCHEDULE A`, `Annex 1`,
  `Appendix B`, `Exhibit C`, `Član 4` (capitalized; a bare one takes the short line after it as
  its title: `CHAPTER 3` + `Storage and Retrieval`; `Part III.` ending a wrapped sentence is not);
- a numbered heading: `1. Definitions`, `2.3 Payment Terms`, `IV. RESULTS`, or run in
  (`7. TERM. This Agreement …` → `7. TERM`); a full sentence after a number is a list item;
- a short line in capitals (not `IN WITNESS WHEREOF`, signature labels, code, or capitals set
  smaller than the body);
- larger type (PDF line boxes): at least 1.2× the median line height, in up to three size
  classes that occur often enough (a cover page's one huge line is no class); only the largest
  class when numbered headings dominate (a contract's larger lines are mostly emphasis).

Left out: running headers and footers (the same line on ≥ 3 pages and a tenth of the document;
the first occurrence of a numbered or keyword one stays, it marks where the chapter starts),
`24 | Chapter 1: …` page furniture, table-of-contents pages (≥ 5 lines and 30 % ending in a page
number or dot leaders). Kinds are ranked part > chapter / schedule > article > numbered by depth >
capitals > type size by size; the ranks found become levels 1, 2, 3, …. At most 64 entries,
whole levels first (all level 1, then level 2, …; a level that does not fit is sampled evenly).
Each entry's pages run to the next heading of the same or a higher level. Fewer than 3
headings: fixed page windows (`Pages 1-9`, at most 16, emitted from 4 pages on).

## Keywords (`summary.keywords@1`)

Document keywords and key phrases for matching and clustering documents by subject and company,
no model, one linear pass over the shared `normalized_text` tokens (`terms`):

- **Candidates**: runs of 1–3 content words; stop words (English and Serbian, legal boilerplate
  such as `hereinafter`, `hereto`, `u daljem tekstu`, months, Roman numerals, legal forms),
  punctuation, numbers, codes, URLs and a capitalized word that starts a line (a heading ends
  there) cut them. Dates, amounts and identifiers are therefore never keywords (they are
  signals already). Content words have ≥ 3 letters (2 for an upper-case acronym).
- **One key for both scripts**: tokens are keyed by `FoldOptions::KEY` (lowercase, no
  diacritics, Serbian Cyrillic transliterated), so `Уговор`, `UGOVOR` and `ugovor` are one
  word. Lemmatization-lite: Serbian / Croatian / Bosnian case endings folded on words of ≥ 6
  letters keeping ≥ 4 (`ugovora`, `ugovorom`, `ugovorima` → `ugovor`; a multi-letter ending
  only after a consonant), English plurals (`services` → `service`, `parties` → `party`, not
  `-ss`, `-us`, `-is`); other languages are not stemmed.
- **Furniture**: a line among the first or last three of a page, of ≤ 12 words, that repeats
  (digits ignored) on ≥ 3 pages and a tenth of the document is a running header or footer and
  is skipped.
- **Score** (YAKE, Campos et al. 2020, per stem): casing (capitalized not at a sentence
  start; acronyms outside runs of capitals), median sentence position, frequency, relatedness
  (distinct neighbours per occurrence: a word next to anything is stop-word-like), spread over
  sentences; `S(w) = T_rel · T_pos / (T_case + (T_freq + T_sent) / T_rel)`, ×3 for generic
  document words (`agreement`, `section`, `party`, `ugovor`, `ponuda`, `nabavka`, ...). A
  phrase scores `Π S(w) / (tf · (1 + Σ S(w)))`, ÷4 when all its words are in the document title
  (`ref.title`: the title names the subject). Lower is better. Phrases of 2–3 words need ≥ 2
  occurrences (single words too from 300 content words), unless they are title words. A
  generic word alone, or a phrase of generic words only, is never a keyword.
- **Pick**: the best 16 (`KeywordConfig::max`), a phrase that occurs mostly inside a picked one
  (≤ 1.3× its count) skipped and a picked word replaced by the phrase it mostly occurs in; at
  most 3 company names (`max_names`; a party's defined term such as `Licensee` does not count).
  `score` = `S(best) / S(k)`: 1 for the first, falling.
- **Fields**: `term` (first occurrence as written, lowercased; a 2–6 letter word that is mostly
  an acronym keeps its capitals), `key` (stems in ASCII: the same for Latin and Cyrillic, use it
  to match documents), `count`, `first_page` (zero-based), `entities` (the `party.candidates`
  keys whose name or defined term holds every word of the keyword; helps company matching).

## Topics (`summary.topics@1`)

A map of a long document (≥ 20 pages, `TopicConfig::min_pages`): where the subject changes and
what each part is about, so a 200–2,000-page file can be navigated and summarized by subject.

- **Sections**: the outline's headings at the deepest level that gives ≤ 64 sections (headings
  that start on the same page are one), plus `Opening pages` before the first heading; without
  3 headings, ≤ 32 fixed page windows. A section's text is its pages.
- **Vectors**: per section, the 1–2 word phrases of its pages (the term index keeps per-page
  counts). Keyword weight `(1 + ln pages) · (1 + ½ ln tf) · ln((N + 1) / df)` over the `N`
  sections (spread across the section's pages beats repetition on one page, a phrase in every
  section weighs almost nothing), single generic words ×0.3, two-word phrases ×1.3. Section
  keywords: top 5; segment keywords: top 8.
- **Shift**: cosine distance (1 − cos) of a section's single-word vector to the previous one.
- **Segments**: a section whose distance to the running segment's summed vector reaches the
  threshold `max(0.6, median + ½ (p90 − median))` of those distances starts a new segment; at
  most 64 (the largest distances win). The record keeps the threshold.
- `topics::range_keywords` gives the distinctive keywords of any page ranges: the digest uses it.

## Digest (`summary.digest@2`)

The model input for a document of any length, at most `DigestConfig::max_tokens` (default
2,000) estimated tokens, deterministic:

```
FACTS:
doc: contract "Master Supply Agreement" lang=en pages=85 pdf
parties: …
OUTLINE (48 of 120 headings, 85 pages):
- Article I. DEFINITIONS (pp. 1-19)
- Article II. GOVERNANCE (pp. 19-26)
…
START:
<the first ~200 tokens of the text: title, parties, recitals>
EXCERPTS (sentences copied from each section):
## Article II. GOVERNANCE (pp. 19-26)
<2-3 salient sentences, verbatim>
## Article III. LICENSE (pp. 26-31)
…
```

- A document whose whole text fits gets `FACTS` + `TEXT` (the whole normalized text).
- With a term index (`summary.keywords` ran, or the digest builds one), each section title is
  followed by `keywords: a, b, c` (5 per section, distinctive against the other sections, see
  Topics); their tokens come out of the excerpt budget.
- Budget: card (≤ 300), outline (15 %, deepest levels dropped first), start (10 %), the rest
  for excerpts. Sections are the outline entries at the deepest level that gives ≤ 16 of them
  (one per 125 tokens of budget; one level deeper when that gives fewer than a third, merged
  by size into `Title (+N more)`), plus `Opening pages` before the first heading. Each section
  gets a floor of 40 tokens and a share of the rest proportional to its size; unused budget
  passes to the next section.
- Per section: at most 48 paragraphs and 12 KiB (its first paragraphs and an even sample of the
  rest), sentences of 8–64 words that end with `.`, `!` or `?` and do not start in lower case
  (a paragraph cut by a page break), no dot leaders, `|` furniture or signature lines. Score:
  keyword coverage (the section's recurring words × their inverse section frequency in the
  document, title words boosted: a linear stand-in for `TextRank`'s centrality) × position prior
  (first paragraphs of a section) × cue bonus (who provides what) × penalties (boilerplate
  clauses 0.2, definitions, list fragments, contact lines 0.3). Pick: maximal marginal
  relevance (λ 0.7) against the sentences picked so far in the document; a sentence with Jaccard
  ≥ 0.6 to a picked one is a repeat and skipped. Printed in document order.
- **Verbatim**: every excerpt is a substring of its page's normalized text (whitespace
  collapsed), so the digest adds no fact; `Digest::verbatim` checks it.
- `digest::map_parts` splits the same selection into ≤ `MapReduceConfig::max_parts` (default
  8) contiguous parts of similar size with `part_tokens` (1,500) each, for map-reduce.

## LLM summary (feature `llm`)

- `LlmSummarizer` on `llama-cpp-2 =0.1.161` (`dynamic-backends`: one CPU backend per
  microarchitecture, chosen at load time; `QUAIL_LLM_BACKENDS` overrides the backend directory).
  Builds llama.cpp with CMake and a C++ compiler, so it is never in the default build.
- Models (`models::MANIFEST`, URLs pinned to a Hugging Face commit, SHA-256 checked before
  loading, Apache-2.0): `default` = Qwen3.5-2B `Q4_K_M` (1.28 GB), `small` = Qwen3.5-0.8B `Q4_K_M`
  (0.53 GB). `cargo run -p quail-summary --features fetch --example fetch_models -- [small|default|all] [DIR]`;
  the directory is `$QUAIL_LLM_MODELS` or the user cache (`quail/llm-models`).
- Prompt (version 2): fixed instructions first, then the card text and the digest body
  (`LlmConfig::digest_tokens`, default 1,600 with the card): one call per document of any
  length. Greedy decoding, thinking off, output constrained by a grammar compiled from the JSON
  schema `{summary ≤ 400 chars, subject ≤ 120, title_en ≤ 120}`.
- Map-reduce (`LlmConfig::map_reduce = Some(MapReduceConfig)`, off by default): a document of
  ≥ 60 pages whose text does not fit the digest is summarized in ≤ 8 map calls (one per part,
  schema `{summary ≤ 360}`, ≤ 120 output tokens) and 1 reduce call over the card, the outline and
  the part summaries. Each part summary is fact-checked against the card and that part's
  excerpts and replaced by the part's first excerpt when it fails; the final summary is checked
  against everything the map steps read (`mapreduce`, model-free and tested with a scripted
  generator). `Summarizer::summarize_long` is the hook; its default is one call.
- **Fact check** (`factcheck::check`) of every output: each date, number, identifier and name
  word must be found in the card text or the text the model saw; otherwise the extractive
  summary is emitted with `method = extractive_fallback` and a warning. Timeouts (60 s), bad JSON
  and load errors fall back the same way; a summary never fails the file.
- Cache: `BLAKE3(card text ‖ digest body (‖ map-reduce marker) ‖ summarizer id)`, the id carrying
  the model SHA-256, the prompt version, the budgets and the map-reduce settings (`MemoryCache`,
  or any `SummaryCache`).
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

**Outline and digest** (no model; dev build at opt-level 2, one thread, best of 3; default
2,000-token budget):

| corpus | files | pages p50 / max | digest tokens p50 / max | sections with an excerpt p10 / p50 | excerpts verbatim | outline / digest ms p50 (p90) |
|---|---|---|---|---|---|---|
| long documents (12 Kleister-Charity reports, 11 longest CUAD contracts, one 613-page book) | 24 | 104 / 613 | 1,962 / 1,976 | 0.94 / 1.00 | 638 / 638 | 3.3 (4.2) / 3.0 (8.5) |
| CUAD, part I | 198 | 12 / 111 | 1,919 / 1,982 | 0.93 / 1.00 | 4,433 / 4,433 | 0.29 (1.2) / 1.5 (3.5) |

Outline + digest cost 6.3 ms per 100 pages p50 (p90 12.6) on the long documents; the digest
reads at most 16 × 12 KiB, so its cost stops growing with length (the 613-page book: 11 ms
outline, 14 ms digest). `map_parts` (8 parts): 4 ms p50, 17 ms max. In the CLI pipeline on all
510 CUAD files (3 threads): `summary.outline` 0.6 ms p50, 2.5 ms p90; `summary.digest` 4.6 ms
p50, 10 ms p90 and 6.9 KB per record, hence Standard and not in the CLI defaults. The book's
outline finds its 3 parts, 12 chapters and their sections (227 headings, 64 kept).

**Long documents with the model** (Qwen3.5-2B `Q4_K_M`, 4 threads, one model at a time; 3
documents of 85, 112 and 613 pages): one call over the digest takes 32–35 s (~1,600 prompt
tokens, ~23 s prefill); map-reduce (8 map + 1 reduce calls, 9–12.5 k prompt tokens) takes
174–263 s. Peak RSS 2.07 GB either way. Fact check 6 / 6, no map fallback. Quality (1–5, own
rating): single call 4 / 4 / 3, map-reduce 4 / 4 / 2.5. The map steps' part summaries are good
and specific; the reduce step sometimes drifts to card details (producer, an adviser named in
the card) or a garbled title. Default stays the single call; map-reduce is for callers who want
per-part summaries and can spend ~3–4 min per long document.

## Limits

- Reads the first 4 pages and at most 48 KiB of normalized text for the subject. The outline
  reads every line once and checks the deadline every 16 pages (a cut outline is marked
  `truncated` and the record partial). The digest reads at most 16 sections × 12 KiB of
  paragraphs, whatever the length, and checks the deadline per section; its output is bounded
  by its token budget, the outline by 64 entries. The LLM sees at most `digest_tokens` per call
  and at most `max_parts + 1` calls per document. Checks the deadline during generation.
- Keywords: one pass over every token (deadline checked every 16 pages; a cut index is marked
  and the record partial), at most 150,000 distinct words and 300,000 distinct phrases (later
  new ones are not counted), 32 sentence positions per word; per-page counts of 1–2 word
  phrases only. Output: at most 16 keywords. Topics: at most 64 sections and 64 segments, 5
  and 8 keywords each; documents of fewer than 20 pages get none.
- Card text: hard budget (default 300 estimated tokens, `SummaryCard::with_max_tokens`).
- No panics on any fact shape: unexpected value types are skipped.

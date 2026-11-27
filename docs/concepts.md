# Concepts

The ideas behind quail's output, one at a time: what a record holds, what a fact and its
confidence mean, how steps are named and versioned, and what happens when a file is too big,
too slow or broken. [architecture.md](architecture.md) shows how the pieces fit together;
[keys.md](keys.md) lists every fact.

## 1. FileRecord

quail writes one **record** per input file, as one line of JSON (JSONL). A record is
everything quail knows about that file:

```json
{
  "schema": 1,
  "locator": "contracts/amendment-2.pdf",
  "size": 36921,
  "format": "pdf",
  "pages": 1,
  "facts": [ ... ],
  "steps": [ {"name": "engine.sniff", "version": 1}, {"name": "hash.identity", "version": 1}, ... ],
  "timings": [ {"src": {"name": "pdf.extract", "version": 1}, "micros": 2210}, ... ],
  "status": { "state": "ok" },
  "warnings": [ ]
}
```

| field | meaning |
|---|---|
| `schema` | version of the record format (currently 1); bumped on any breaking change |
| `locator` | where the file came from: the path as walked, or the locator given in a manifest |
| `size` | bytes |
| `format` | sniffed from the bytes: `pdf`, `docx`, `pptx`, `html`, `zip`, `image`, `text`, `unknown` |
| `pages` | pages extracted, when the file is a document |
| `facts` | every fact, in the order the steps produced them |
| `steps` | every step that ran, with its version |
| `timings` | microseconds per step |
| `status` | `ok`, `partial` (with `problems`) or `failed` (with `error`); see section 7 |
| `warnings` | notes that changed nothing, each prefixed with the step id |

quail describes one file at a time. It never looks at two files together (the one exception,
`--dedup`, only skips repeated work for identical bytes). Grouping files into contracts,
companies or cases is left to the system that reads the records; the sketches and identifiers
in the record are what it needs for that.

## 2. Facts

A **fact** is one piece of metadata:

```json
{
  "key": "signal.iban",
  "value": {"t": "text", "v": "DE89370400440532013000"},
  "conf": 0.98,
  "src": {"name": "signal.scan", "version": 4},
  "evidence": {"page": 0, "zone": "body", "span": {"start": 775, "end": 802},
               "bbox": {"x0": 72.1, "y0": 279.9, "x1": 527.7, "y1": 344.1}}
}
```

- **`key`**: what it is, `namespace.name`. Each crate owns one namespace (`pdf.`, `signal.`,
  `party.`, ...) and declares its keys once. A key can appear several times in one record
  (every IBAN found, every party candidate).
- **`value`**: a tagged value; see the next section.
- **`conf`**: confidence in [0, 1]; see section 4.
- **`src`**: the step (and its version) that produced it.
- **`evidence`**: where in the file it was found, when it came from the content.

quail reports **candidates, not verdicts**. A contract may name four companies; quail emits
all of them as `party.candidates` with roles and scores, and leaves the decision to whoever
reads the record. A fact is never silently dropped because a competing one looked better.

## 3. Values

`value` is always `{"t": <type>, "v": <value>}`, from a closed set of shapes, so any consumer
(JSON, a column store, a search index) can store any fact without knowing its key:

| `t` | `v` | used for |
|---|---|---|
| `bool` | `true` / `false` | `pdf.encrypted`, `docx.has_macros` |
| `int` | integer | counts, sizes, page numbers |
| `float` | number | ratios, scores |
| `text` | string | names, identifiers as written, languages, MIME types |
| `u64` | integer | 64-bit hashes and sketches (SimHash, XXH3) |
| `bytes` | lowercase hex string | BLAKE3 digests, MinHash signatures |
| `date` | `{"y": 2023, "m": 9, "d": 12}` | calendar dates |
| `money` | `{"units": 420000, "scale": 2, "currency": [69, 85, 82]}` | exact decimal amounts: 4200.00 EUR; the currency is the ISO 4217 code as three byte values (`EUR`), left out when unknown; never a float |
| `list` | array of values | several values of one fact (chunk hashes, fonts) |
| `record` | object of named values | one structured fact (a party with its role, a cited document with kind, number and date) |

**Records** carry fields in a fixed order, and each field is itself a tagged value. Field
names are documented next to the key in [keys.md](keys.md). A cited document (`ref.cites`):

```json
{"t": "record", "v": {
  "kind":     {"t": "text", "v": "msa"},
  "relation": {"t": "text", "v": "amends"},
  "raw":      {"t": "text", "v": "114/23"},
  "key":      {"t": "text", "v": "11423"},
  "date":     {"t": "date", "v": {"y": 2023, "m": 3, "d": 3}}
}}
```

Fields that were not found are left out, so a consumer checks for presence, not for null.

## 4. Confidence

- **Measured facts have `conf` 1.** A hash, a page count, a property read from the file's
  metadata: nothing was guessed.
- **Inferred facts have `conf` below 1**, and it is meant to be **calibrated**: among facts
  with `conf` around 0.8, about 80 % should be right. A consumer can then pick its own
  threshold: high for automatic actions, low for a search index that a person reviews.

Calibration is measured, not assumed. `quail-eval score` bins every scored prediction by
`conf` and reports the expected calibration error (ECE) per task
([evaluation.md](evaluation.md#metrics)). When an analyzer changes its scoring, its ECE on
the dev splits is part of the review.

A confidence is a property of one fact, not of the file. The same IBAN found once in a
signature block and once in body text gives two facts, each with its own `conf` and
evidence.

## 5. Evidence and zones

`evidence` says where a fact was found, precisely enough to cite it and to draw it on the
rendered page:

| field | meaning |
|---|---|
| `page` | zero-based page index (DOCX: section index; PPTX: slide index; see below) |
| `zone` | `header`, `footer`, `body`, `title`, `table`, `signature` (left out when unknown) |
| `span` | byte range `[start, end)` in that page's extracted text |
| `bbox` | box on the page, in PDF points (1/72 inch) from the top-left corner |

**Zones matter.** A company named in the letterhead or the signature block is worth more than
one mentioned in passing; a date in a running footer is usually a print date. Analyzers use
the zone in their scores, and consumers can too.

PDF pages are physical pages with geometry. DOCX has no fixed pages, so the extractor makes
one *logical* page per section; such pages have no boxes. A PPTX gives one logical page per
slide (its speaker notes are `note` blocks at the end of the page). An HTML file is split
into logical pages at CSS page breaks (`page-break-before/after: always`, what SEC EDGAR
filings put between printed pages), one page without them. Plain text files are split into
logical pages at form feeds (one page without them), also without boxes. `quail inspect --text` prints the
extracted page text, so a `span` can be checked by eye.

## 6. Steps: extractors, analyzers, ids and versions

Everything in a record is produced by a **step**:

- the engine's own steps: `engine.sniff` (format from the bytes) and, with `--dedup`,
  `engine.dedup`;
- an **extractor** per format, turning bytes into a `Document`: `pdf.extract`,
  `docx.extract`, `pptx.extract`, `html.extract` (with the declared charset),
  `text.extract` (plain text, with encoding detection);
- **analyzers**, turning what is known so far into more facts: `hash.identity`, `text.stats`,
  `signal.scan`, `party.candidates`, ...

Each step has an id `namespace.name@version`. **The version is bumped whenever the output for
the same input can change.** That is what makes re-indexing cheap and safe:

- `steps` in every record lists exactly which step versions produced it.
- `quail version` prints the step ids of the current build and a **pipeline fingerprint**, a
  hash over the schema version, the profile and every step id in run order.
- When the fingerprint changes, compare a stored record's `steps` with `quail version --json`:
  a record is stale exactly when one of its steps has a different version, or a step was
  added or removed. Re-run only those files (or only those whose facts you use).

Limits are not part of the fingerprint: changing `--timeout` does not mark records stale.

## 7. Status, problems and warnings

| status | meaning |
|---|---|
| `ok` | every step that applied succeeded |
| `partial` | something stopped short, but every fact present is valid; `problems` lists what |
| `failed` | the file could not be read as a document (not a valid PDF/DOCX, too large, unreadable); generic facts such as hashes may still be there |

A step reports two kinds of trouble:

- a **warning** is a note that changes nothing: "an unparsable date in the properties", "a
  garbage text layer was rejected". The record stays `ok`.
- a **problem** says the step did not finish its job: it hit a limit, ran out of time, skipped
  a broken page. Everything it produced is kept, and the record becomes `partial`.

An analyzer that errors, panics or times out loses its own facts and makes the record
`partial`; analyzers that do not depend on it still run. An extractor that fails makes the
record `failed`. An encrypted PDF is `partial`: the facts readable without the password are
kept.

## 8. Limits and deadlines

Every file runs under `Limits`. Exceeding one gives a partial record, never a crash:

| limit | default | effect |
|---|---|---|
| `max_bytes` | 512 MiB | bigger files are not read; `failed` record naming the limit |
| `max_pages` | 2000 | later pages are skipped (problem) |
| `max_decompressed` | 256 MiB | total bytes any decompression may produce for the file |
| `max_text_bytes` | 64 MiB | text kept per document |
| `max_render_pixels` | 40 M | pixels of one decoded image or rendered page |
| `timeout` | 2 min | wall time for the whole file |
| `max_timeout` | 0 (no extension) | the most a step may extend the file's wall time to |

The **deadline** is the file's start time plus `timeout`. A step that finds more work than
that covers may extend it, up to the start plus `max(timeout, max_timeout)`
(`Emitter::extend_deadline`): OCR does, for the pages it has to read, so a file without such
pages keeps the plain `timeout`. Steps check the deadline in their loops and stop with a
problem when it passes; rendering and OCR use the same deadline, so a file has
one time budget. The engine cannot preempt a thread that never checks; that is what
`--isolate` is for ([architecture.md](architecture.md#worker-isolation)).

## 9. Profiles

A **profile** says how much work to spend per file. Each analyzer declares the lowest profile
it runs in; steps above the chosen profile are left out of the plan.

| profile | what runs |
|---|---|
| `fast` | rules and statistics: extraction, hashes, text statistics, document language, identifiers, dates and amounts, references, parties, clauses, price lines, most sketches, the card and summary |
| `standard` (default) | `fast`, plus per-page languages, CPV categories for price lines, the MinHash of the compatibility mode with an existing deployment (`sketch.minhash_compat`) and the page pHash of scanned pages |
| `deep` | `standard`, plus OCR (`ocr.pages`) of the pages that need it, in a `quail` built with `--features ocr`. Programs that build their own engine can also add the LLM summary (`summary.llm`). In the default build, `deep` runs the same steps as `standard` |

`quail version --profile fast` shows the exact plan of each profile for your build.

## 10. Artifacts

Steps pass data to each other through **artifacts**: typed intermediate results, produced
once, read by many, never changed after they are stored. They are not part of the record.

| artifact | provided by | read by |
|---|---|---|
| `document` | the extractor | every text analyzer |
| `raster` | `pdf.extract` | page pHash, OCR |
| `normalized_text` | `text.stats` | `text.lang`, refs, parties, sketches (one normalization per file, shared) |
| `revised_document` | `ocr.pages` | every text analyzer, through `Ctx::document()` |

A step declares the artifacts it `requires` (it does not run without them), the ones it
`provides`, and the ones it runs `after` if they exist. The engine orders steps by these
declarations when it is built ([architecture.md](architecture.md#the-plan)).

To get the extracted `Document` out of a run, for evaluation or inspection, use
`quail scan --dump-documents FILE`: one JSON line per file with its locator, BLAKE3 and
document. Records are unchanged by it.

## 11. Documents, OCR and the revised document

A `Document` is a list of pages. Each page has its text in reading order, **blocks**
(paragraphs, headings, list items, table cells, notes) that point into that text with a span,
a zone and a box, and for PDF the measured lines of each block. A page also says where its
text came from (`native`, `ocr`, `none`) and whether it **needs OCR**: no text layer but
images, mostly an image with little text, or a text layer that is garbage.

When OCR is active (profile `deep`, in a build with the `ocr` feature and its models), the
flow is:

```
pdf.extract ──▶ document (page 3: needs_ocr) + raster
ocr.pages   ──▶ renders or decodes page 3, reads it, provides revised_document
                (page 3 now has text_source = ocr, with blocks and boxes)
text.*, signal.*, ref.*, party.*, ...  ──▶ read ctx.document(): the revised one
```

The original `document` is write-once and stays as extracted. Text analyzers declare
`after(["revised_document"])` and read `Ctx::document()`, which returns the revised document
when one exists and the original otherwise. Without OCR they run exactly as before.

## 12. Finalizers

Some steps need the facts of every other step: the file **card** (`summary.card`), the
**extractive summary** (`summary.extractive`), the keywords and the topic map. They are
**finalizers**: the engine runs them after all non-finalizer steps, whatever the registration
order (and among themselves by what they read: the card after the keywords).

The card is what an LLM agent should read first about a file: a compact, deterministic text
of 100 to 300 tokens with the document kind, title, parties, dates, amounts, identifiers,
references, keywords and flags. The extractive summary is one to three English sentences built
from the card and the document's subject clause, without a model.

**Keywords** (`summary.keywords`) are the document's 10–16 key phrases, ranked by YAKE-style
statistics (no model, one pass over the text). Stop words, numbers, dates, amounts and running
headers never become keywords, and a phrase in the title ranks higher. Each keyword has a
`key` that is the same for Latin and Cyrillic spellings and for Serbian case forms (`ugovora`,
`уговором` → `ugovor`), so documents can be matched and clustered by subject; a keyword that
names a party links to its `party.candidates` key (`entities`), so they can be matched by
company too.

For long documents (20 pages or more) the **topic map** (`summary.topics`) says where the
subject changes: every outline section (or page window) gets its distinctive keywords and a
*shift*, the cosine distance of its words to the previous section's, and consecutive sections
on one subject are grouped into topic segments with their page ranges and keywords. A
2,000-page file can then be navigated, and summarized part by part, by subject. The digest
(`summary.digest`, the bounded input for a model) prints the same kind of keywords under each
of its sections.

## 13. Content dedup

`quail scan --dedup` hashes each file before processing it. A file whose bytes were already
processed in the same run gets a copy of that record with its own locator, the fact
`engine.duplicate_of` (the first file's locator) and the step `engine.dedup@1`; no other step
runs for it. The cache is bounded (10 000 records, 64 MiB) and is per run. Records are assumed
to depend on the bytes only. `--dedup` cannot be combined with `--isolate`.

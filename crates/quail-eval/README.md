# quail-eval

The evaluation harness: decides, with numbers, whether a rule or model change in quail is
better or worse, and puts the raw metadata next to the actual file so a person (or an AI
agent) can check it. Library plus a `quail-eval` binary. Emits no facts; no analyzers.

Discipline (as in hedgehog's `docs/evaluation.md`):

- **Only measured numbers.** Every claim in a PR comes from a scoreboard.
- **Before and after on the same gold.** `compare` refuses scoreboards whose gold sets
  differ (gold digest, split or dataset filter).
- **A frozen holdout.** Each file's split is fixed by its BLAKE3; the lists are committed.
  `score` scores dev only unless `--allow-holdout` is given: holdout is for releases.
- **No regressions.** `compare` exits 1 when any task's F1 drops more than the threshold or
  failed records rise.
- **Look at the files.** `inspect` writes packs with the page text and every fact's
  evidence marked, gold vs predicted side by side.

## Workflow

```sh
# 0. gold, once per dataset (written outside the repo)
quail-eval convert --adapter kleister-nda --input $QUAIL_TESTDATA/kleister-nda --out ~/quail-gold
quail-eval splits --gold ~/quail-gold --lists crates/quail-eval/split --check   # frozen lists

# 1. run: process the corpus with quail and write JSONL records (quail-io's JsonlSink;
#    the quail CLI once it lands). Records must carry hash.blake3 (quail-hash).

# 2. score -> eval/runs/<label>/{scoreboard.json,report.md}
quail-eval score --records before.jsonl --gold ~/quail-gold --label before
quail-eval score --records after.jsonl  --gold ~/quail-gold --label after

# 3. compare (CI gate: exit 0 ok, 1 regression, 2 not comparable / error)
quail-eval compare eval/runs/before/scoreboard.json eval/runs/after/scoreboard.json --max-f1-drop 1.0

# 4. inspect what changed or looks wrong -> eval/inspect/<run>/
quail-eval inspect --records after.jsonl --gold ~/quail-gold --sample 20 --where mismatch,low_conf --seed 7
quail-eval inspect --records after.jsonl --gold ~/quail-gold --blake3 f7c2d392e0b2adc1
```

`eval/runs/` and `eval/inspect/` are gitignored: scoreboard JSON and packs contain values and
text of the documents. `report.md` and the compare output hold aggregates and hashes only,
so they can go into a PR, even for a private corpus.

## Gold format

One JSON file per document, in a dataset directory outside the repository:

```json
{
  "file": { "blake3": "<64 lowercase hex>", "locator_hint": "documents/0001.pdf" },
  "dataset": "kleister-nda",
  "split": "dev",
  "source": "kleister-nda/train",
  "labels": [
    { "task": "party", "value": "Example Corp", "match": "normalized" },
    { "task": "effective_date", "value": "2001-02-03", "match": "normalized", "page": 0 },
    { "task": "amount_total", "value": 4200.5, "match": "numeric_tol", "tol": 0.01 },
    { "task": "language", "value": ["sr", "hr"], "match": "any_of", "note": "mixed" }
  ],
  "absent": ["iban"]
}
```

- Files are matched to records by BLAKE3 (the `hash.blake3` fact), else by locator (exact,
  or a unique locator ending with the hint at a path boundary).
- A task in `labels` has those values; a task in `absent` has none (any prediction is an
  FP); any other task is not annotated for that document and not scored on it.
- Match modes (after the task's normalizer, except `exact`): `exact` (trimmed text),
  `normalized` (equal), `contains` (token run of one inside the other, shorter ≥ 3
  chars), `any_of` (list of alternatives), `numeric_tol` (|gold − pred| ≤ `tol`, default 0.01),
  `row` (an object value: one gold row of a rows task, see *Rows tasks*).
- Gold names neutral **tasks**, never fact keys, so it survives key renames.

## Task map (`tasks.toml`)

Committed in this crate and compiled in (override with `--tasks`). Per task:

- `keys`: fact keys pooled as predictions.
- `normalize`: `none`, `text`, `company`, `date`, `identifier`, `bank_account`, `number`,
  `bool`, `language`. `company`, `date` and `identifier` fold Serbian Cyrillic to Latin
  (`quail-text`): `company` also drops `д.о.о.`/`доо`/`а.д.`/`d.0.0.`/`preduzetnik` at the end and
  `JP`/`JKP` at the start; `date` reads Serbian month names (nominative and genitive, `1. јула
  2026. године`); `identifier` keeps letters and digits and maps Cyrillic look-alikes to Latin
  (`ОС 90` = `OC 90`); `bank_account` reads `BBB-N-KK` as 18 digits (`840-26650-09` =
  `840-0000000026650-09`).
- `multi` (set semantics), `min_conf`.
- `field`: for record values, the field that is the prediction (`surface` of `party.company`,
  `raw` of `ref.doc_number`; lists of records are flattened).
- `only` / `except`: for record values, keep / drop records by a field's value
  (`only = { kind = ["registry"] }`).
- `aliases`: predicted value → the gold vocabulary's value (`{ amendment = "annex" }`).
- `absent_if_equals`: the gold leaves this task out when it equals the named task's value (CUAD
  drops `effective_date` when it is the agreement date). On such documents a prediction equal
  to a gold label of the named task is neither a TP nor an FP; any other prediction is an FP as
  usual.
- `rows`: gold labels are rows and predictions are whole records (see *Rows tasks*);
  `normalize`/`field`/`only`/`except`/`aliases` do not apply.
- `pending`: why the task has no producer yet; its keys are the planned names, it is scored as
  abstentions and flagged in the report.

Tasks: `doc_type`, `doc_title`, `party`, `effective_date`, `agreement_date`,
`expiration_date`, `jurisdiction`, `term`, `contract_number`, `document_number`,
`parent_contract_number`, `procurement_number`, `tax_id`, `registration_number`,
`bank_account`, `iban`, `amount_total`, `language`, `script`, `is_scanned`,
`near_duplicate_of`, `items` (rows).

## Rows tasks (`items`)

`items` scores the price lines of `quail-items` (`item.line` records). Gold rows:

```json
{ "task": "items", "match": "row", "page": 0,
  "value": { "description": "Senior consultant", "quantity": 10, "unit": "h",
             "unit_price": 85.0, "total": 850.0, "currency": "EUR", "cpv": ["79411000"] } }
```

Every field is optional. A line and a row match when they are on the same page (if both are
known), their prices agree within 0.5 % (relative; the unit price, else the total, else one
against the other), and their folded descriptions have a token Jaccard ≥ 0.3 (when both
have one). Pairs are taken one to one, best first. This gives the task's P/R/F1. The
scoreboard's `rows.items` adds:

- `lines_price_only`: the same matching without the description test;
- per field (`quantity`, `unit`, `unit_price`, `total`, `currency`), over matched pairs whose
  row has the field: `n`, `filled` (the line has it), `correct` (numbers within 0.5 %,
  quantity exact, text case-insensitive), `accuracy`;
- categories: over matched pairs with `cpv` gold, `hit@3` (a gold code among the top 3
  `category_candidates`, first 8 digits) and `division@3` (same two-digit division).

## Metrics

Per task, pooled over scored documents and broken down per dataset and per split:

- **single-valued**: the most confident prediction is a TP if it matches any label, else
  one FP and one FN; **multi-valued**: predictions (deduplicated after normalization, most
  confident first) are matched one to one with labels; unmatched predictions are FPs,
  unmatched labels FNs.
- P = TP/(TP+FP), R = TP/(TP+FN), F1 = 2TP/(2TP+FP+FN); `null` when undefined.
- **abstain rate**: labelled documents with no prediction (after `min_conf`).
- **pred. on absent**: documents marked absent that still got a prediction.
- **calibration**: every scored prediction (correct = TP) in ten `conf` bins; ECE =
  Σ (n_bin/N)·|accuracy − mean conf|.
- **status**: ok / partial / failed / with warnings, over the run and over scored documents.
- **latency**: p50/p95/p99/max per step (`timings`) and per file (`total`), µs,
  nearest-rank. Machine-dependent: not gated unless `--max-p95-ratio` is given.

## Inspection packs

`<out>/<run>/<blake3 prefix>/inspect.md` + `record.json`, and `<out>/<run>/index.md`:
file identity (and whether the file on disk still has the recorded hash), gold vs
predicted per task with the result, all facts, then each page's text with evidence spans
marked `⟦n|…|n⟧` (n = fact number) and zone changes `«header»`.

Selection: `--blake3 <prefix>`, `--locator <substring>`, or `--sample N --where
mismatch,low_conf,failed,partial --seed S` (a seeded hash order, so a seed always gives the
same sample).

Page text is not in records, so it comes from a `DocumentSource`: plain-text files are read
from their locator (`TextFiles`); `--documents dump.jsonl` reads a dump of
`{"locator", "blake3", "document": <quail_core::Document>}` lines (`Sidecar`); library users
can re-run any quail `Extractor` (`Extractors`). Page images: the `PageRenderer` trait is the
hook; no renderer ships yet (`NoRenderer`), the pack says so on every page.

## Splits

`assign(id) = holdout` iff `blake3("<seed>:<id>")` as a fraction of 2^64 is below the share
(default 20 %, seed `quail-eval-holdout-2026-11-20`). Per file, so adding files never moves
others. `splits` writes `<dataset>.dev.txt` / `<dataset>.holdout.txt` (BLAKE3 ids only:
committed in `crates/quail-eval/split/`: Kleister-NDA 253 dev / 84 holdout, CUAD 409 / 100, RealKIE FCC 296 / 74); files already listed keep
their side even if seed or share change. `--check` fails if lists are stale or a gold file's
`split` disagrees; `--apply` fixes the gold files. `convert` uses the same assignment.

## Dataset adapters (`GoldAdapter`)

| adapter | reads | tasks | status |
|---|---|---|---|
| `kleister-nda` | `{train,dev-0}/expected.tsv` + `in.tsv` (decompress `in.tsv.xz` first), `documents/*.pdf` | `party`, `effective_date`, `jurisdiction`, `term` (missing key → absent) | run on the real data: 337 documents (train + dev-0), 1315 labels |
| `cuad` | `CUAD_v1.json` (SQuAD form), contract PDFs/TXTs by title | `doc_title`, `party` (contains), `agreement_date`, `effective_date`, `expiration_date` (dates parsed to ISO; non-date answers skipped and counted; no answer → absent) | run on the real data: 510 documents (1 without a file), 3675 labels, 773 non-date answers skipped |
| `realkie-fcc` | RealKIE `fcc_invoices`: `{train,val,test}.csv` (span labels in the OCR text), `ocr/*.json.gz` (page offsets), `files/*.pdf` | `items`: `Line Item - *` spans grouped into rows in text order, one gold row per `Rate` (`description`, `unit_price`, `currency` = USD when a `$` precedes the rate) on the rate's page; no rows → absent | run on the real data: 370 documents, 19,991 rows (33 documents without rows) |

CUAD's party spans include defined-term aliases ("Distributor"), so party precision against
CUAD is low by construction; recall is the meaningful number there.

## Limits

Records and gold are loaded into memory (fine for gold sets of thousands; records of a
million-file run are about the JSONL size). No network, no models; depends only on clap,
serde, serde_json, toml, flate2 (pure Rust, for RealKIE's `.json.gz`) and quail crates.

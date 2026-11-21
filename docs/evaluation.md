# Evaluation

How good quail's facts are, measured on public datasets, and how to run the measurements
yourself. Every number on this page comes from a `quail-eval` scoreboard; nothing is
estimated.

Contents: [Datasets](#datasets) · [Current scores](#current-scores) · [Metrics](#metrics) ·
[Running the evaluation](#running-the-evaluation) · [Gold format](#gold-format) ·
[Tasks](#tasks) · [Commands](#commands) · [Rules](#rules) · [What is not measured yet](#what-is-not-measured-yet)

## Datasets

| dataset | what it is | gold tasks | dev / holdout files |
|---|---|---|---|
| [Kleister-NDA](https://github.com/applicaai/kleister-nda) | non-disclosure agreements filed with the US SEC (PDF) | `party`, `effective_date`, `jurisdiction`, `term` | 253 / 84 |
| [CUAD](https://zenodo.org/records/4595826) v1 | commercial contracts from SEC filings, clause-level annotations by lawyers (PDF) | `doc_title`, `party`, `agreement_date`, `effective_date`, `expiration_date` | 409 / 100 |
| [RealKIE](https://zenodo.org/records/13327077) FCC invoices | broadcast-advertising invoices filed with the US FCC (PDF, many scanned) | `items` (price lines) | 296 / 74 |

Each file's split is fixed by its BLAKE3 hash: 20 % of each dataset is a frozen **holdout**
that is not used while quail is developed. The lists of hashes are committed in
`crates/quail-eval/split/`. Every number below is on the **dev** split.

## Current scores

quail `0.1.0`, default build, profile `standard`, `master` of 2026-11-21. 958 dev documents;
953 records `ok`, 4 `partial`, 1 `failed`.

| task | dataset | what is predicted | support | P | R | **F1** | ECE |
|---|---|---|---:|---:|---:|---:|---:|
| `jurisdiction` | Kleister-NDA | governing law (`clause.governing_law`) | 249 | 99.6 | 98.0 | **98.8** | 0.046 |
| `effective_date` | Kleister-NDA | `signal.date_effective`, `signal.date_signed` | 176 | 87.6 | 88.6 | **88.1** | |
| `party` | Kleister-NDA | `party.company` | 475 | 69.6 | 73.3 | **71.4** | |
| `term` | Kleister-NDA | `clause.term`, `clause.survival` | 89 | 70.2 | 74.2 | **72.1** | 0.064 |
| `doc_title` | CUAD | `ref.title`, then the file's title property | 417 | 94.5 | 92.4 | **93.4** | 0.177 |
| `agreement_date` | CUAD | `signal.date_signed`, `signal.date_effective` | 292 | 84.7 | 83.6 | **84.1** | 0.006 |
| `effective_date` | CUAD | `signal.date_effective`, `signal.date_signed` | 187 | 83.6 | 87.2 | **85.3** | |
| `party` | CUAD | `party.company` (name or defined term) | 2045 | 85.2 | 69.4 | **76.5** | |
| `expiration_date` | CUAD | `signal.date_expiry` | 2 | 22.2 | 100.0 | 36.4 | 0.664 |
| `items` | RealKIE FCC | `item.line` rows (description, price, page) | 15461 | 60.6 | 37.4 | **46.2** | 0.118 |

P, R and F1 are percentages; *support* is the number of gold labels. ECE is the calibration
error of `conf` ([Metrics](#metrics)), pooled over all datasets of the task (blank where the
task spans two datasets; pooled values: `effective_date` 0.020, `party` 0.062).

Notes on reading the table:

- **`expiration_date`** has only 2 positive labels in the dev split: most CUAD expiration
  answers are a term ("five years from the effective date"), not a date, and the adapter skips
  them. Its F1 says little. The 7 false positives are predictions on documents whose gold marks
  the date as absent.
- **CUAD `party`**: CUAD's party spans include defined-term aliases ("Distributor") as well
  as names. quail predicts each party once, with its name and its defined term; recall is the
  more meaningful number.
- **`items`**: a predicted line matches a gold row when it is on the same page, its price
  agrees within 0.5 % and its description shares at least 30 % of its words. Matched lines
  have the right unit price in 96.3 % of cases and the right currency in 100 %. Ignoring the
  description test, F1 is 52.2. 124 of the 296 dev invoices have no text layer on any page;
  without OCR their lines cannot be found, and 49.3 % of the documents with gold rows get no
  prediction.
- **Status**: the 1 `failed` record is a CUAD file that is missing from the dataset download
  (its gold exists; the scan records it as unreadable). The 4 `partial` records are CUAD
  contracts naming more than 32 candidate companies; the 32 strongest are kept.

Per-step latency from the same run (debug build of quail with optimized dependencies,
4 threads, a shared machine) is in the scoreboard; throughput of a release build is in
[performance.md](performance.md).

## Metrics

Per task, pooled over the scored documents, with a breakdown per dataset and per split:

- **Single-valued** tasks (a date, a title): the most confident prediction is a true positive
  if it matches any gold label, otherwise one false positive and one false negative.
- **Multi-valued** tasks (parties, price lines): predictions, deduplicated after
  normalization and taken most confident first, are matched one to one with the labels;
  unmatched predictions are false positives, unmatched labels false negatives.
- P = TP / (TP + FP), R = TP / (TP + FN), F1 = 2TP / (2TP + FP + FN).
- **Abstain rate**: labelled documents without any prediction.
- **Predicted on absent**: documents whose gold says the value is absent but that got a
  prediction.
- **Calibration**: every scored prediction is put in one of ten `conf` bins. ECE (expected
  calibration error) = sum over bins of (n_bin / N) x |accuracy - mean conf|. 0 means that
  `conf` 0.8 is right 80 % of the time; the report also prints each bin.
- **Status** counts (ok / partial / failed / with warnings) and **latency** per step (p50,
  p95, p99, max).

Values are compared after a per-task normalization: dates as dates (`3 March 2023` =
`2023-03-03`), company names without legal-form noise and with `&` read as `and`, identifiers
with letters and digits only, numbers within a tolerance.

## Running the evaluation

`quail-eval` is a binary in the workspace:

```sh
cargo build --release -p quail-cli -p quail-eval
export PATH="$PWD/target/release:$PATH"
```

### 1. Get the datasets and convert them to gold

The fetch scripts are in `tools/datasets/` (see its README). Gold files are written outside the
repository:

```sh
export QUAIL_TESTDATA=~/testdata
quail-eval convert --adapter kleister-nda --input $QUAIL_TESTDATA/kleister-nda --out ~/quail-gold --lists crates/quail-eval/split
quail-eval convert --adapter cuad --input $QUAIL_TESTDATA/cuad --out ~/quail-gold --lists crates/quail-eval/split
quail-eval convert --adapter realkie-fcc --input $QUAIL_TESTDATA/realkie/fcc_invoices --out ~/quail-gold --lists crates/quail-eval/split
quail-eval splits --gold ~/quail-gold --lists crates/quail-eval/split --check
```

The Kleister-NDA adapter reads `in.tsv` next to `expected.tsv` in `train/` and `dev-0/`;
decompress the shipped `in.tsv.xz` first (`xz -dk train/in.tsv.xz dev-0/in.tsv.xz`).
`splits --check` exits 1 if a gold file's split disagrees with the committed lists.

### 2. Scan the dev files

Score matches records to gold by the `hash.blake3` fact, so any run that includes the dev files
works. To scan only them, build a manifest (`locator<TAB>path` per line) from the gold files:

```sh
python3 - ~/quail-gold kleister-nda $QUAIL_TESTDATA/kleister-nda > kleister-nda.dev.manifest <<'EOF'
import glob, json, os, sys
gold, dataset, root = sys.argv[1:4]
for f in sorted(glob.glob(os.path.join(gold, dataset, "*.json"))):
    g = json.load(open(f, encoding="utf-8"))
    if g["split"] == "dev":
        hint = g["file"]["locator_hint"]
        print(f"{hint}\t{os.path.join(root, hint)}")
EOF
quail scan --manifest kleister-nda.dev.manifest -j 4 -o kleister-nda.dev.jsonl
```

Repeat for `cuad` (root `$QUAIL_TESTDATA/cuad`) and `realkie-fcc` (root
`$QUAIL_TESTDATA/realkie/fcc_invoices`), then put the records in one file:

```sh
cat kleister-nda.dev.jsonl cuad.dev.jsonl realkie-fcc.dev.jsonl > all.dev.jsonl
```

### 3. Score

```sh
quail-eval score --records all.dev.jsonl --gold ~/quail-gold --label master
```

This prints the report and writes `eval/runs/master/scoreboard.json` and `report.md`.
`eval/` is gitignored: the scoreboard contains values from the documents. `report.md` holds
aggregates and hashes only.

### 4. Compare before and after

```sh
quail-eval compare eval/runs/master/scoreboard.json eval/runs/branch/scoreboard.json --max-f1-drop 1.0
```

Exit code 0: no regression; 1: some task's F1 dropped by more than `--max-f1-drop` points, or
failed records rose; 2: the scoreboards are not comparable (different gold, split or dataset
filter). This is the gate every quality change passes
([development.md](development.md#evaluation-gate)).

### 5. Look at the files

```sh
quail-eval inspect --records all.dev.jsonl --gold ~/quail-gold --sample 20 --where mismatch,low_conf --seed 7
quail-eval inspect --records all.dev.jsonl --gold ~/quail-gold --blake3 0071d378
```

Each pack (`eval/inspect/<run>/<blake3 prefix>/inspect.md`) shows the file's identity, gold
against predicted per task, every fact, and each page's text with every fact's evidence span
marked `⟦n|…|n⟧`. Page text is not in records: pass `--documents` with the output of
`quail scan --dump-documents` to include it.

## Gold format

One JSON file per document:

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
    { "task": "language", "value": ["sr", "hr"], "match": "any_of" }
  ],
  "absent": ["iban"]
}
```

- Files are matched to records by BLAKE3, else by locator.
- A task in `labels` has those values; a task in `absent` has none (any prediction is a false
  positive); any other task is not annotated for that document and not scored on it.
- Match modes: `exact`, `normalized`, `contains`, `any_of`, `numeric_tol`, and `row` for
  rows tasks such as `items`.
- Gold names neutral **tasks**, never fact keys, so it survives key renames. Your own corpus
  can be scored the same way: write gold files in this format.

## Tasks

`crates/quail-eval/tasks.toml` maps each task to the fact keys whose values are its
predictions, with the normalizer and options. It is compiled into `quail-eval`; `--tasks FILE`
overrides it. For example:

```toml
[task.jurisdiction]
description = "Governing law"
keys = ["clause.governing_law"]
```

Options per task: `keys`, `normalize` (`text`, `company`, `date`, `identifier`,
`bank_account`, `number`, `bool`, `language`), `multi` (set semantics), `min_conf`, `field`
and `alt_fields` (which fields of a record value are the prediction), `only` / `except`
(filter records by a field), `aliases` (map predicted values to the gold vocabulary),
`absent_if_equals`, `rows`, and `pending` (a task with no producer yet, scored as abstentions).
The full description is in `crates/quail-eval/README.md`.

## Commands

| command | does |
|---|---|
| `quail-eval convert --adapter A --out DIR` | converts a public dataset (`kleister-nda`, `cuad`, `realkie-fcc`) into gold files; `--input` defaults to `$QUAIL_TESTDATA/<adapter>`; `--lists` keeps the committed split sides |
| `quail-eval splits --gold DIR --lists DIR` | assigns dev / holdout by BLAKE3 and writes the lists; `--check` only verifies, `--apply` fixes gold files that disagree |
| `quail-eval score --records R --gold DIR` | scores a run; `--split dev\|holdout\|all` (holdout needs `--allow-holdout`), `--dataset`, `--min-conf`, `--label`, `--out`, `--quiet` |
| `quail-eval compare A B` | compares two scoreboards; `--max-f1-drop` (default 1), `--max-failed-increase` (default 0), `--max-p95-ratio`, `--allow-gold-change`, `--json` |
| `quail-eval inspect --records R` | writes inspection packs; select with `--blake3`, `--locator`, or `--sample N --where mismatch,low_conf,failed,partial --seed S`; `--documents` adds page text |

## Rules

- **Only measured numbers.** A claim about quality in a pull request or on this page comes
  from a scoreboard.
- **Before and after on the same gold.** `compare` refuses scoreboards whose gold sets differ.
- **The holdout is frozen.** `score` scores dev unless `--allow-holdout` is given; the holdout
  is for releases. Adding files to a dataset never moves existing files between splits.
- **No regressions.** `compare` exits 1 when any task's F1 drops by more than the threshold.
- **Look at the files.** A number that moved is explained with inspection packs.

## What is not measured yet

- **Identifiers, amounts, document numbers, references, document type.** quail emits them
  (`signal.*`, `ref.*`), and `tasks.toml` defines the tasks, but no public dataset with
  matching gold is converted yet. Development uses a private corpus of ~400 business
  documents, scored the same way; only aggregates from it are published.
- **Layout and reading order.** Datasets such as OmniDocBench and DocLayNet are fetched by
  `tools/datasets/` but have no adapter yet.
- **OCR quality.** OCR is not in the default build; FCC `items` recall shows what it costs.
- **Near-duplicates.** The `near_duplicate_of` task is defined but pending: sketches are
  per file, and pairing them across files is the consumer's job.

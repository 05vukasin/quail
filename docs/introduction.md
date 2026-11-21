# What quail is

quail reads business documents, one file at a time, and writes down what it finds in them as
facts that a program, a search index or an AI agent can use directly. This page explains the
problem it solves, what you get back, and what quail deliberately does not do. After it,
[quickstart.md](quickstart.md) gets you running and [concepts.md](concepts.md) explains the
output in detail.

## The problem

An organization's documents pile up for decades: contracts and their amendments, invoices,
offers, orders, letters, scans of signed copies, the same file saved five times under five
names. Millions of PDF and DOCX files, most of them never opened again, all of them holding
facts somebody will one day need: who signed with whom, what was agreed, until when, for how
much, which document amends which.

Getting at those facts is hard:

- **The files are not data.** A PDF is drawing instructions for a page; the words come in any
  order, columns run into each other, headers and footers repeat on every page. A DOCX hides
  tracked changes, comments and hidden text. Many pages are scans with no text at all.
- **The volume rules out expensive tools.** Sending millions of files to a cloud API or
  through a large model costs a lot, takes weeks, and sends confidential documents out of the
  building.
- **Some files are hostile or broken.** In millions of files there are always some that make
  a parser allocate gigabytes, recurse until the stack overflows, or never finish.
- **A wrong answer is worse than none.** A system that says "the supplier is X" without saying
  how sure it is, or where it read that, cannot be trusted with decisions.

## What quail does

quail runs on a plain CPU, offline, with no external service. For each file it produces one
**record**: a list of **facts**, each with a **confidence**, the **step** that produced it
and the **evidence** (page, zone, text span, box on the page) where it was found.

For the amendment in the [quickstart](quickstart.md), quail finds, among about 80 facts:

| fact | value | conf | where |
|---|---|---:|---|
| `ref.instrument_kind` | `amendment` | 0.80 | title, page 1 |
| `ref.cites` | amends `msa` `114/23` dated 2023-03-03 | 0.75 | page 1, body |
| `party.candidates` | Northwind Logistics Ltd (customer, signatory), Blue River Software GmbH (supplier, signatory) | | |
| `signal.date_signed` | 2023-09-12 | 0.82 | page 1, body |
| `signal.date_expiry` | 2026-12-31 | 0.78 | page 1, body |
| `signal.amount` | 4,200.00 EUR | 0.97 | page 1, body |
| `signal.iban` | `DE89370400440532013000` (check digits verified) | 0.99 | page 1, body |
| `clause.governing_law` | England and Wales (`GB-EAW`) | 0.97 | page 1 |
| `hash.blake3`, `sketch.simhash_text`, ... | exact and near-duplicate fingerprints | 1 | |
| `summary.card_text` | a 207-token card an LLM agent can read first | | |

The facts come in layers, each built on the ones before:

1. **Identity**: exact hashes of the bytes and content-defined chunk hashes, so identical and
   partly identical files can be found.
2. **Structure**: the format, file properties (producer, author, dates, signatures, macros,
   tracked changes) and the text of every page in reading order, split into blocks with zones
   (header, footer, body, title, table, signature) and boxes.
3. **Text signals**: language and script, identifiers with check digits (IBAN, VAT and tax
   numbers, LEI, ...), dates with their role (signed, effective, expiry), amounts with their
   currency, contact domains.
4. **Business meaning**: the document's own number and title, the kind of instrument
   (contract, amendment, invoice, order, ...), the documents it cites, the companies it names
   with their roles, contract clauses (governing law, term, notice periods), price lines.
5. **Similarity**: MinHash and SimHash sketches of the text, of its template and of the
   layout, so near-duplicates and documents made from the same template can be grouped.
6. **Summary**: a compact card and a one-to-three-sentence summary, deterministic and
   without a model.

## Principles

- **Candidates, not verdicts.** When a contract names four companies, quail reports four
  candidates with roles and scores. It does not throw information away because one candidate
  looked better.
- **Calibrated confidence.** `conf` 0.8 should be right about 80 % of the time; that is
  measured on public datasets ([evaluation.md](evaluation.md)).
- **Evidence for everything inferred.** Every fact read from the content says where, so a
  person or an agent can check it against the page.
- **Versioned steps.** Every fact names the step that made it, with a version. When a step
  changes, you know exactly which stored records are stale ([concepts.md](concepts.md#6-steps-extractors-analyzers-ids-and-versions)).
- **Hostile input is normal.** Limits, deadlines, decode budgets inside the parsers and
  optional process isolation mean one bad file costs one record, not the run
  ([security.md](security.md)).
- **Measured, not claimed.** Quality numbers come from scoreboards on public datasets, speed
  numbers from runs with stated hardware ([performance.md](performance.md)).

## What quail does not do

- **It does not look across files.** quail describes one file at a time. Deciding that two
  files are versions of the same contract, or that "Blue River Software GmbH" in one file and
  "Blue River" in another are the same company, belongs to the system that reads the records.
  quail gives it the keys to do that: normalized names, identifiers, cited numbers, sketches.
- **It does not change your files.** It reads them and writes records.
- **It does not need a model or the internet by default.** OCR and an LLM-written summary
  exist as optional steps with their own native libraries and downloaded models; the default
  build is pure Rust and runs rules, statistics and small built-in dictionaries.
- **It does not read every format.** PDF and DOCX are the formats it extracts; other files get
  their format, MIME type and hashes.

## Where to go next

1. [quickstart.md](quickstart.md): build quail, scan a folder, inspect a file.
2. [concepts.md](concepts.md): records, facts, values, confidence, evidence, steps.
3. [keys.md](keys.md): every fact quail can produce.

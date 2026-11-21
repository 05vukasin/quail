<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/logo-dark.png">
    <img src="assets/logo/logo.png" alt="quail logo: a quail whose wing is a fingerprint" width="160">
  </picture>
</p>

<h1 align="center">quail</h1>

<p align="center">
  <b>Give it millions of documents. Get back rich, searchable metadata for every file, offline, on a plain CPU.</b>
</p>

<p align="center">
  <a href="https://github.com/05vukasin/quail/actions/workflows/ci.yml"><img src="https://github.com/05vukasin/quail/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

---

quail reads business documents (PDF and DOCX), one file at a time, and writes one JSON record
per file: the facts it found, each with a **confidence**, the **step** that produced it and the
**evidence** (page, zone, text span, box on the page) where it was found. A person or an AI
agent can check every fact against the page.

- **Offline and CPU-only.** No external API, no GPU. The default build is pure Rust and needs
  no models.
- **Built for millions of files.** 55 to 155 PDF files per second on 4 threads of a laptop CPU,
  under 300 MB of memory, content dedup, and process isolation so a hostile file costs one
  record, not the run.
- **Candidates, not verdicts.** Every party, date and identifier found is reported with its
  score; nothing is silently dropped.
- **Per file.** Grouping files into contracts, companies or cases is left to the system that
  reads the records; quail gives it identifiers, normalized names and similarity sketches.

## What it finds

| layer | facts | namespace |
|---|---|---|
| Identity | BLAKE3, XXH3-128, content-defined chunk hashes | `hash.*` |
| Structure | format and MIME type; PDF and DOCX properties (producer, author, dates, signatures, forms, JavaScript, macros, tracked changes, comments, hidden text); page text in reading order with blocks, zones and boxes; pages that need OCR | `engine.*` `pdf.*` `docx.*` |
| Text | characters, words, scripts, garbage ratio; document and page language | `text.*` |
| Signals | IBAN, BIC, VAT, LEI, DUNS, EIN and national tax and registry numbers with check digits; dates with roles (signed, effective, expiry, invoice, due); amounts with currency and role; e-mail and web domains | `signal.*` |
| References | own document number, title, instrument kind (contract, amendment, invoice, order, ...), cited documents with relation and date | `ref.*` |
| Parties | company mentions with legal form, roles and defined terms; ranked party candidates | `party.*` |
| Clauses | governing law, term and renewal, notice and survival periods, termination rights | `clause.*` |
| Items | price lines (description, quantity, unit, prices) with CPV category candidates | `item.*` |
| Similarity | MinHash with LSH bands, SimHash of text and of template, layout hashes, page pHash of scans | `sketch.*` |
| Summary | a file card of 100 to 300 tokens for LLM agents and a 1 to 3 sentence English summary, without a model | `summary.*` |

Optional, behind cargo features with native libraries and downloaded models: OCR of scanned
pages (`ocr.*`) and a fact-checked summary from a small local LLM. Every key:
[docs/keys.md](docs/keys.md).

## Build

```sh
git clone https://github.com/05vukasin/quail
cd quail
cargo build --release -p quail-cli        # Rust 1.97+, pure Rust, no system libraries
export PATH="$PWD/target/release:$PATH"
```

## Use

```sh
quail scan /data/contracts -o records.jsonl              # every file under a folder, one JSON line each
quail scan /data --isolate -j 4 -o records.jsonl         # each file in a worker process: crashes cost one record
quail scan /data --dedup --profile fast -o records.jsonl # identical files analyzed once; a cheaper pass
quail inspect contracts/amendment-2.pdf                  # one file, for a person
quail keys signal.                                       # what each fact means
```

`quail inspect` on a synthetic two-party amendment (excerpt):

```text
file     contracts/amendment-2.pdf
format   pdf (application/pdf)   size 37.1 kB   pages 1
status   ok

[ref] 5 fact(s)
  ref.instrument_kind  "amendment"  conf 0.80  src ref.references@2  at p1 body [0..38] box(72,86,444,121)
  ref.cites            {kind="msa" relation="amends" raw="114/23" key="11423" date=2023-03-03}  conf 0.75  src ref.references@2  at p1 body [328..383] box(72,130,543,201)

[party] 5 fact(s)
  party.candidates  2 record(s)  src party.candidates@3
                      - {key="northwind logistics" surface="Northwind Logistics Ltd" score=0.99 roles=["customer", "signatory"]}
                      - {key="blue river software" surface="Blue River Software GmbH" score=0.99 roles=["supplier", "signatory"]}

[signal] 9 fact(s)
  signal.iban         "DE89370400440532013000"  conf 0.99  src signal.scan@3  at p1 body [775..802] box(72,280,528,344)
  signal.amount       4200.00 EUR  conf 0.97  src signal.scan@3  at p1 body [661..673] box(72,280,528,344)
  signal.date_signed  2023-09-12  conf 0.82  src signal.scan@3  at p1 body [110..127] box(72,130,543,201)
  signal.date_expiry  2026-12-31  conf 0.78  src signal.scan@3  at p1 body [884..900] box(72,359,531,408)

[clause] 1 fact(s)
  clause.governing_law  {jurisdiction="England and Wales" code="GB-EAW" raw="laws of England and Wales" basis="laws_of"}  conf 0.97  src clause.terms@1  at p1 [1013..1038]
```

`scan` writes the same record as JSON, one line per file. Walkthrough:
[docs/quickstart.md](docs/quickstart.md).

## How good is it?

Scored with `quail-eval` against hand-labelled public datasets, on their dev splits (a frozen
20 % holdout is kept for releases). F1 from 0 to 100:

| task | dataset | P | R | **F1** |
|---|---|---:|---:|---:|
| governing law | Kleister-NDA | 99.6 | 98.0 | **98.8** |
| effective date | Kleister-NDA | 87.6 | 88.6 | **88.1** |
| term | Kleister-NDA | 70.2 | 74.2 | **72.1** |
| parties | Kleister-NDA | 69.6 | 73.3 | **71.4** |
| document title | CUAD | 94.5 | 92.4 | **93.4** |
| agreement date | CUAD | 84.7 | 83.6 | **84.1** |
| effective date | CUAD | 83.6 | 87.2 | **85.3** |
| parties | CUAD | 85.2 | 69.4 | **76.5** |
| price lines | RealKIE FCC invoices | 60.6 | 37.4 | **46.2** |

958 dev documents, `master` of 2026-11-21. Confidence is calibrated: the expected calibration
error is below 0.07 for every task above except document title (0.18) and price lines (0.12).
Many FCC invoices are scans, which the default build (no OCR) cannot read. Details, the other
metrics and how to reproduce: [docs/evaluation.md](docs/evaluation.md).

## How fast?

`quail scan -j 4`, profile `standard`, on a laptop CPU (AMD Ryzen 7 7735U), shared with other
work:

| corpus | files | files/s | MB/s | peak RSS |
|---|---:|---:|---:|---:|
| CUAD contracts (PDF) | 510 | 54.6 | 10.8 | 197 MB |
| Kleister-NDA (PDF) | 726 | 153.7 | 10.0 | 104 MB |
| RealKIE FCC invoices (PDF, many scans) | 370 | 104.1 | 28.6 | 235 MB |
| a private corpus of ~400 business documents (PDF, DOCX, scans) | 417 | 129.6 | 67.0 | 261 MB |

`--profile fast` adds 42 to 61 % files/s; `--isolate` costs 0 to 16 % files/s and about twice
the memory (each worker loads its own models). One thread does 17.6 files/s on CUAD. Setup,
per-step shares and what was optimized: [docs/performance.md](docs/performance.md).

## Documentation

Start with [what quail is](docs/introduction.md), then the [quickstart](docs/quickstart.md) and
the [concepts](docs/concepts.md). The [documentation index](docs/README.md) lists every page:
the [command line](docs/cli.md), every [fact key](docs/keys.md), the
[architecture](docs/architecture.md), [evaluation](docs/evaluation.md),
[performance](docs/performance.md), [security](docs/security.md) and
[development](docs/development.md).

## Contributing

Issues and pull requests are welcome. Please:

- when a file is read wrongly, include a small synthetic document or the code that builds one;
  never attach a real or confidential document;
- keep `make check` green (`cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`);
- for changes that can move a quality number, include `quail-eval compare` before and after.

Setup, rules and workflow: [docs/development.md](docs/development.md).

## License

MIT, see [LICENSE](LICENSE).

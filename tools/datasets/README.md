# tools/datasets

Reproducible fetch scripts for the public benchmark datasets quail is evaluated
on. Data lands **outside the repo** in `$QUAIL_TESTDATA` (default
`<repo>/../testdata`). Nothing under `testdata/` is ever committed.

```sh
tools/datasets/run_all.sh            # everything, smallest first, then manifest
tools/datasets/cuad.sh               # a single dataset
QUAIL_TESTDATA=/data/quail tools/datasets/run_all.sh
```

- **Idempotent and resumable.** `hf download --include`, `curl -C -` and
  sha256 checks (Kleister-Charity). If you re-run a script it skips what is
  already on disk.
- **Disk guard.** Each script checks `df` first and aborts if free space would
  drop below `QUAIL_MIN_FREE_GB` (default 30).
- **Requirements.** Needs `bash`, `curl`, `unzip`, `tar`, `python3`, plus the
  Hugging Face CLI (`uv tool install huggingface_hub`). `manifest.py` uses
  pyarrow through `uv run --with pyarrow` to count documents stored inside
  parquet files.
- **No identity sent.** Requests use a generic User-Agent and need no account,
  token or e-mail. Setting `HF_TOKEN` only raises the Hugging Face rate limit.
- **Manifest.** `manifest.py` writes `$QUAIL_TESTDATA/MANIFEST.md` and
  `manifest.json`. These hold the measured sizes, file counts by extension and
  real PDF/DOCX counts, detected from file content (magic bytes).

## License rule

**eval-only** means the data may be used to measure quail. Do not train or
tune shipped models or rules on it, and do not redistribute it. This applies
to every dataset whose license is non-commercial or unclear.

## Datasets

The size column is measured on disk after download (2026-11-21). The PDF and
DOCX columns count real files by magic bytes. "Embedded" counts payloads inside
parquet rows.

| script | dataset | what quail evaluates with it | license | eval-only | size | real files | source |
|---|---|---|---|---|---|---|---|
| `cuad.sh` | CUAD v1 | contract clause facts (41 types) on real EDGAR contract PDFs | CC-BY-4.0 | no | 280 MB | 510 PDF + 510 TXT | https://zenodo.org/records/4595826 |
| `kleister_nda.sh` | Kleister-NDA | key info (parties, dates, term, jurisdiction) from NDA PDFs | unclear (no license file) | **yes** | 95 MB | 726 PDF | https://github.com/applicaai/kleister-nda |
| `kleister_charity.sh` | Kleister-Charity (dev-0 + test-A) | key info from long, partly scanned annual-report PDFs | unclear (no license file) | **yes** | 4.16 GB | 1049 PDF | https://github.com/applicaai/kleister-charity |
| `realkie.sh` | RealKIE (fcc_invoices, nda, resource_contracts test+val) | enterprise KIE on invoices/contracts, OCR robustness | CC-BY-NC-4.0 | **yes** | 841 MB | 890 PDF | https://zenodo.org/records/13327077 (s3://project-fruitfly) |
| `rvl_cdip_n_mp.sh` | RVL-CDIP-N-MP | document-type classification on multi-page PDFs | CC-BY-NC-4.0 | **yes** | 2.61 GB | 997 PDF | https://huggingface.co/datasets/jordyvl/rvl_cdip_n_mp |
| `doc_split.sh` | DocSplit (CSVs + test GT) | packet splitting / document boundaries (on RVL-CDIP-N-MP PDFs) | CC-BY-NC-4.0 | **yes** | 521 MB | 4163 JSON GT | https://huggingface.co/datasets/amazon/doc_split |
| `rvl_cdip_n.sh` | RVL-CDIP-N | doc-type classification on scanned page images | CC-BY-3.0 tag, source terms unclear | **yes** | 545 MB | 1002 TIFF embedded | https://huggingface.co/datasets/jordyvl/RVL-CDIP-N |
| `tabmepp.sh` | TABME++ (test + val) | page-stream segmentation (where one document ends) | MIT | no | 1.12 GB | 12326 JPEG pages embedded | https://huggingface.co/datasets/bevaya/TABMEpp |
| `omnidocbench.sh` | OmniDocBench | layout, reading order, tables, formulas on page images | research only, non-commercial | **yes** | 1.55 GB | 1657 page images | https://huggingface.co/datasets/opendatalab/OmniDocBench |
| `doclaynet.sh` | DocLayNet v1.2 (test) | layout block classes + PDF text cells | CDLA-Permissive-1.0 | no | 2.42 GB | 4999 single-page PDF + PNG embedded | https://huggingface.co/datasets/docling-project/DocLayNet-v1.2 |
| `fintabnet_otsl.sh` | FinTabNet-OTSL (test) | financial table structure | "other" (CDLA-Permissive-1.0 upstream) | **yes** | 288 MB | 10397 PNG embedded | https://huggingface.co/datasets/docling-project/FinTabNet_OTSL |
| `sec_material_contracts.sh` | SEC Exhibit-10 contracts (2 of 304 shards) | contract-type / party / date facts on real filing text | CC-BY-SA-4.0 | no | 255 MB | 7512 parquet rows | https://huggingface.co/datasets/chenghao/sec-material-contracts |
| `acord.sh` | ACORD | clause retrieval / clause-type labels | CC-BY-4.0 | no | 17 MB | 1 DOCX, TSV/JSONL | https://huggingface.co/datasets/theatticusproject/acord |
| `maud.sh` | MAUD v1 | merger-agreement deal-point facts | CC-BY-4.0 | no | 158 MB | 100 contract TXT | https://huggingface.co/datasets/theatticusproject/maud |
| `ledgar.sh` | LEDGAR (LexGLUE) | clause-type classification of provisions | CC-BY-4.0 (LexGLUE), CC-BY-SA-4.0 upstream | no | 28 MB | 80000 parquet rows | https://huggingface.co/datasets/coastalcph/lex_glue |
| `postocr_sr.sh` | postocr-sr | Serbian OCR-noise normalisation / text quality | CC-BY-4.0 | no | 93 MB | JSONL pairs | https://huggingface.co/datasets/Sagicc/postocr-sr |
| `setimes_sr.sh` | SETimes.SR | Serbian NER (persons, orgs, locations) | CC-BY-SA-4.0 | no | 14 MB | CoNLL-U | https://huggingface.co/datasets/classla/setimes_sr |
| `gleif_lei.sh` | GLEIF LEI Golden Copy (CSV, zipped) | organisation-name / LEI resolution gazetteer | CC0-1.0 | no | 511 MB | 1 CSV zip (~3.5 M records) | https://goldencopy.gleif.org/api/v2/golden-copies/publishes/lei2/latest |

Total measured: about 15.5 GB.

## Deliberately not fetched

- **DocILE**: needs registration.
- **Live SEC EDGAR crawling**: EDGAR requires a contact e-mail in the User-Agent.
- **Subsets left out to save space**:
  - Kleister-Charity: train documents.
  - RealKIE: page PNGs and the resource_contracts train split (~40 GB).
  - sec-material-contracts: shards 2..303 (~40 GB).
  - DocLayNet and FinTabNet: train and validation splits.
  - DocSplit: train and validation JSON (~48k small files; anonymous HF requests are rate-limited).

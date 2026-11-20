# quail-refs

Per-file candidates for linking documents into contract families (MSA → SOW / annex / amendment
→ PO → invoice) and multi-part documents. Namespace `ref.*`. Serbian (Latin and Cyrillic) and
English, a few French kind words.

- Analyzer: `ref.references@1` (`RefAnalyzer`), `Profile::Fast`, requires `document`.
- Public API: `RefAnalyzer`, `keys::*`, `Kind`, `Relation`, `field` (wrapper over `Value::field`),
  `number::*` (the number rules, usable by the linking service), the `MAX_*` caps.

Not here: IBAN, VAT, PIB, dates, money, e-mails (`quail-signals`); company names, parties and
roles (`quail-parties`).

## Keys

| key | value | what |
|---|---|---|
| `ref.doc_number` | record `kind raw key base date` | an own number of this document |
| `ref.cites` | record `kind relation raw key base ordinal date` | a document this one cites |
| `ref.title` | Text | title: heading/title-zone block, else a title line, else the largest line |
| `ref.instrument_kind` | Text | `msa contract nda sow amendment amended_restated annex change_order side_letter po order invoice quote` |
| `ref.ordinal` | Int | Amendment No. 3 → 3, Third Amendment → 3, Aneks II → 2, AM9 → 9 |
| `ref.page_marker` | record `n of` | "Strana 2 od 5", "Page 2 of 5", "Стр. 2 од 5"; one per page |
| `ref.version_hint` | record `kind n` | `version revision draft final execution redline clean` |
| `ref.study_code` | Text | clinical study code `D\d{3}[0-9A-Z]C\d{5}` |
| `ref.clm_record` | Text | CLM record id `con_<digits>` |

Records are `Value::Record` (an ordered JSON object, snake_case part names); read a part with
`Value::field` (or `quail_refs::field`, a thin wrapper).
`raw` is the number as written, `key` its matching key, `base` the key without a version tail
(`(rev|ver|v|r)\d{1,3}$`), `date` a `Value::Date`. Relations: `amends annex_to issued_under
billed_against references`. Every fact carries evidence (page, zone, span into the original page
text, the block's box when it has one).

## Number rules

A faithful port of a production contract-audit service's rules (character for character, the
Python lookarounds written out by hand):

- `normalize_contract_no`: alphanumeric core, upper-cased, at least 5 characters.
- `is_identifying_contract_key`: a run of ≥ 3 significant digits that is neither a date (`10.27.22`,
  `2025-04-18`, `Amendment_20221130`) nor a year alone or beside a month word (`Agreement 2021`,
  `Oct2021`); a year joined to a code segment stays (`QD-2047`); `SOW-001`, `No. 001` are ordinals
  (label words), `QRX-001` is a code.
- `is_code_prefixed_number`: a code word of ≥ 4 letters welded to ≥ 2 significant digits (`QXPSO_78`).
- `normalize_contract_number`: the key, zero padding after a letter folded (`QXPSO_078` = `QXPSO_78`),
  `""` for a label without a code (`Amendment No. 3`).
- `invoice_number_key` / `invoice_identity_key`: 3-character floor, re-issue letter (`SI-731204A`)
  and run padding folded for identity. Invoice numbers use the identity key, every other kind the
  contract key.
- `without_version_tail`, `unpadded`, `study_codes`, `clm_record_ids`.

## Own or cited

A labelled number ("Ugovor br.", "Broj ugovora", "Уговор бр.", "Contract No.", "Agreement Number",
"PO Number", "Invoice No", "Faktura broj", "Račun br.", "Broj: …" on a head line) is the document's
own when its kind fits the instrument kind (from the title, else from the first labelled number at
the head of page 1). The "PO Number" on an invoice is cited (`billed_against`); on a purchase order
it is its own. A trigger before the kind ("pursuant to", "under", "to the", "uz", "na osnovu",
"po") makes a citation and names the relation; otherwise the relation follows from the two kinds.
"This Agreement" / "ovaj ugovor" is the document itself. A generic "Contract ID: 6619/AMD/QX"
takes the kind its type code names (`AMD`, `CR`, `SOW`, `PO`, `INV`, `MSA`). Serbian *aneks* is an
amendment, *prilog* an annex. Bank accounts ("tekući račun"), "P.O. Box", "Executive Order",
"NDA No." (drug applications) and securities-filing exhibit indexes ("EXHIBIT 10.12") are skipped.

## How

Per page: `quail_text::normalize::normalize_mapped` (offset map back to the page text), then
`quail_text::token::keyed_tokens` folded into a *key line* (lowercase, no diacritics, Cyrillic →
Latin, punctuation dropped, line breaks kept), so one pattern covers "UGOVOR", "Уговор" and
"ugovor". Patterns are `regex` automata over the key line (linear time); values are read from the
normalized text. Pages 1–2 are prepared first (title, instrument kind), the rest one at a time.

## Limits

- Reads at most `Limits::max_text_bytes` of text (problem when it stops), checks the deadline per
  page, skips pages flagged `needs_ocr`.
- Caps per document: 16 distinct own numbers, 64 cites, 16 version hints, 32 study codes and 32 CLM
  ids; one page marker per page. A cap hit is an `Emitter::problem`.
- Numbers longer than 48 characters are ignored; the title reads at most 25 lines of page 1 (or 2).

## Measured

Throughput, end to end (normalize + keyed tokens + matching), release build on a shared 16-core
laptop (load average 9–12 during the runs): 7–10 MB/s of page text (CUAD 510 PDFs via pdftotext:
25.9 MB in 2.5–3.8 s; 369 private business PDFs + 11 DOCX: 5.2 MB in 0.7 s, under 2 ms per file).
quail-text's `normalize_mapped` + `keyed_tokens` alone run at 12–14 MB/s on the same text under
the same load, so most of the time is there. Criterion (`cargo bench -p quail-refs --profile
profiling`, 32 pages × 4 KiB of reference-dense generated text): sr-Latn 7.1 MiB/s, sr-Cyrl
5.7 MiB/s.

Title on CUAD (510 PDFs, gold = the annotated document name, folded comparison; equal or one
containing the other): 460 / 495 documents with a gold name (92.9 %), 96.0 % of titles with
`conf ≥ 0.7`. `amendment` on CUAD: 20 documents, all with an amendment/addendum gold name (20 of
28 such documents found). Instrument kind on 380 private business files against their folder
(weak truth): 205 of 209 invoices with text in the invoice folder read `invoice`. Hand-checked
random samples (precision): own numbers 29/30 (private) and 23/24 (CUAD), cites about 85–90 %
(40-fact samples), page markers 15/15, ordinals 25/25, version hints about 22/25.

## Known gaps

- Fonts are not in the document model: "largest line" uses the box height of single-line blocks.
- Titles that are a party name in a heading lose only to a later kind line.
- An identifier labelled "Contract ID" in an amendment or change order is cited (`amends`) unless
  its type code says otherwise; some CLMs print the document's own id that way.
- No file-name signals (`ctx.locator`): everything comes from the text.

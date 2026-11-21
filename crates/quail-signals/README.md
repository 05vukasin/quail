# quail-signals

Rule-based **candidates** (not verdicts) for formatted business data: identifiers with check digits,
dates, money and contacts. Namespace `signal.*`. Contract and document numbers, cross-document
references and titles belong to `quail-refs`; company names, legal forms and parties to `quail-parties`.

One analyzer: **`signal.scan@1`** (`SignalScan`), profile `Fast`, requires the `document` artifact.

Every fact carries a calibrated `conf` and `Evidence { page, zone, span, bbox }`: the span is a byte
range of the **original** page text (scanning runs on normalized text and maps back through
`quail_text::normalize::OffsetMap`); zone and box come from the block that contains it.

## Keys

| key | value | what |
|---|---|---|
| `signal.iban` | Text | IBAN, compact uppercase; every SWIFT-registry country length, MOD 97-10 |
| `signal.bic` | Text | BIC/SWIFT, 8 or 11 characters, ISO country; only after a `BIC`/`SWIFT` label or next to an IBAN |
| `signal.vat` | Text | EU/UK VAT number with prefix (`DE…`, `ATU…`, `NL…B01`, `GB…`, `EL…`); check digits for AT BE DE ES FR GB HR IT NL PL SI, format only for the other member states |
| `signal.rs_pib` | Text | Serbian PIB, 9 digits, ISO 7064 MOD 11,10 |
| `signal.rs_mb` | Text | Serbian matični broj, 8 digits, MOD 11 (weights 2 7 6 5 4 3 2) |
| `signal.rs_account` | Text | Serbian bank account `BBB-NNNNNNNNNNNNN-KK`, MOD 97-10, middle part zero-padded |
| `signal.hr_oib` | Text | Croatian OIB, 11 digits, ISO 7064 MOD 11,10 |
| `signal.lei` | Text | LEI (ISO 17442), MOD 97-10 |
| `signal.duns` | Text | DUNS, 9 digits, label required (no checksum) |
| `signal.us_ein` | Text | US EIN `12-3456789`, IRS prefix check, label required before or after |
| `signal.jmbg_count` | Int | number of **valid** JMBGs in the file; values are personal data and are never emitted, no evidence |
| `signal.date` | Date | every date |
| `signal.date_effective` / `_expiry` / `_signed` / `_invoice` / `_due` | Date | the same date again when a keyword before it gives its role |
| `signal.amount` | Money | amount with currency, exact decimal |
| `signal.amount_total` | Money | amount after a total keyword (`Ukupno`, `UKUPNO ZA UPLATU`, `Total`, `Amount due`) |
| `signal.amount_net` / `signal.amount_gross` | Money | amount marked without / with VAT (`bez PDV-a`, `excl. VAT`, `Net Total`; `sa PDV-om`, `incl. VAT`, `Gross Total`) |
| `signal.email_domain` | Text | domain of an e-mail address; free-mail domains (sada's `GENERIC_EMAIL_DOMAINS`) dropped |
| `signal.web_domain` | Text | domain of a URL, `www.` host or bare domain with a common TLD |
| `signal.phone` | Text | phone number in E.164 |

One fact per distinct (key, value) per file, with the evidence of its most confident occurrence.

## Design decisions

- **Invalid check digits**: a well-formed identifier with wrong check digits is kept **only right after
  its label** (`PIB: 123456780`), at `conf` 0.2 (a typo or OCR error is still worth a look); without a
  label it is dropped.
- **Bare digit runs** stay low or absent: MB, OIB, DUNS, EIN and unprefixed VAT numbers need their label;
  a bare 9-digit number with a valid PIB checksum is kept (conf 0.3) only on pages that read as Serbian
  business text and only when it starts with 1. Bare IBAN, LEI and dashed Serbian accounts are kept
  because their MOD 97 check is strong.
- **Labels** are folded phrases (`quail_text::fold`), so `PIB`, `ПИБ`, `Matični broj`, `Матични број`,
  `Mat. br.` match one list. A label counts within four words on the same line, or on the line above when
  the value starts its line (label cells, labels above values); another number in between ends the search.
  `PIB/MB: 101234567/07005466` is split and each part takes the label its length fits.
- **Dates**: numeric `d.m.yyyy`, `d. m. yyyy.`, ISO, `d/m/y` and `d-m-y`; month names in English (full and
  abbreviated), Serbian Latin and Cyrillic (nominative and genitive), Croatian, German; `12th day of
  September, 2023`; a month-name date may wrap over one line break (`January` / `22, 2013`), and
  `December 11,2014` is read. Years 1900–2099, real calendar days only; two-digit years are not read. Slash dates
  with both parts ≤ 12 are ambiguous: read month-first only when the file has more month-first evidence
  (`09/25/2023`, `September 12, 2023`) than day-first, else day-first; without any evidence at conf 0.6.
- **Date roles** are separate keys with `Value::Date` (one query gets "the due dates"), rather than one
  role fact that has to be joined to its date by span. `conf` of a role fact = date conf × role
  precision. `date_expiry` means "end of a term or period" (`until`, `through`, `ending on`, `expires`).
  Where a role comes from, in order:
  1. a role phrase up to five words before the date, also across one wrapped line (`made as of`,
     `entered into on`, `dated as of`, `effective as of`, `commencing on`, `with effect from`, `stupa na
     snagu`, `važi od`, `zaključen(o) (je) dana`, `zaključuju`, `Datum zaključenja`, `potpisan`; Cyrillic
     folds to Latin). `dated as of`, `made`, `made on` and `entered into on` give `date_signed`; `as of`,
     `effective`, `entered into`, `made this` give `date_effective`;
  2. a bare label right before the date: `Date:`, `Datum:`, and `dana` at the start of a line or after a
     comma (`U Beogradu, dana ...`) → `date_signed`;
  3. a date alone on its line in the first 25 lines of the first page (a letter or cover-page date, a
     place before it and `godine` after it allowed) → `date_signed`.

  **Cited agreements** get no role: for `date_effective` / `date_signed` phrases the sentence before the
  phrase is searched for the nearest agreement noun (`agreement`, `contract`, `plan`, `letter`, `ugovor`,
  `sporazum`, ...; defined terms in quotes skipped) and its determiner. `this` / `ovaj` (and a bare `the
  Agreement`, `This is an Agreement`) mean this document; `the license agreement`, `a ... Agreement`, `both
  Agreements`, `that certain ...` mean another one. Serbian participles of another document
  (`Ugovora zaključenog dana`, `potpisanog`) stop the search.
- **Money**: with both `.` and `,` the later one is decimal; one separator followed by 1–2 digits is
  decimal, by exactly 3 digits groups thousands; spaces group thousands in threes; `million`/`milion`
  etc. multiply. ISO codes only when written in capitals (`TRY`, `ALL` are words), currency words
  (`dinara`, `din.`, `дин.`, `evra`, `dollars`) and symbols (`€ $ £ ¥ ₹`) in any case. An amount without a
  currency is kept only directly after a total keyword and with two decimals.
- **Phones**: international (`+`, `00`) always; domestic only after a phone label as Serbian
  (`011/123-4567`, on a Serbian page or written with a slash) or North American; `(212) 555-1234` alone
  is distinctive enough. A trunk `(0)` and a leading national `0` after the country code are dropped.

## Limits

Reads at most `Limits::max_text_bytes`, checks the deadline per page, keeps at most 200 distinct values
per key (`MAX_PER_KEY`) and 4 000 dates per file before resolving their order; each limit records a
`problem` (record becomes `Partial`) and keeps what was found. No regular expressions: every rule looks
a bounded number of tokens around a candidate, so the scan is linear in the text.

## Calibration (conf ≈ measured precision)

Measured on 1 430 real files: CUAD (510 contracts, 26.8 MB of text), Kleister-NDA (540, 9.6 MB), a
private business corpus (369 PDFs through `pdftotext`, 11 DOCX through `quail-docx`; 5.2 MB). Precision
is hand-checked on random samples of the emitted facts (pooled over the corpora). Date roles are
measured against gold: on the documents of the Kleister-NDA and CUAD dev splits (and serbian-public
silver dev) that carry an effective or agreement date, a role fact is correct when its date is one of
them ("any date of this agreement"). The `conf` column is the emitted conf of a usual date (0.97 ×
role conf). A date alone at the head of the page is set below its precision (0.9) because
Kleister-NDA leaves about half of its letter agreements without an effective date.

| key / rule | facts | sample | precision | conf |
|---|---|---|---|---|
| `date` (unambiguous) | 3 940 | 40 | 1.00 | 0.95–0.97 |
| `date` ambiguous, by evidence / default | — | — | prior | 0.85 / 0.6 |
| `date_effective` (phrase) | 353 on dev gold docs | Kleister-NDA + CUAD dev gold | 0.77 (0.90 / 0.72) | 0.76 |
| `date_expiry` (end of term/period) | 208 | 30 | 0.93 | 0.78 |
| `date_signed` (phrase) | 209 on dev gold docs | Kleister-NDA + CUAD dev gold | 0.65 | 0.63 |
| `date_signed` (`Date:` / `dana` label) | 140 on dev gold docs | Kleister-NDA + CUAD + serbian-public silver dev | 0.48 | 0.49 |
| `date_signed` (date alone at the head of page 1) | 47 on dev gold docs | Kleister-NDA + CUAD dev gold | 0.98 | 0.87 |
| `date_invoice` | 105 | 30 | 1.00 | 0.92 |
| `date_due` (after the `Discount Due Date` fix) | 140 | 30 | 1.00 | 0.92 |
| `amount` | 3 688 | 40 | 1.00 | 0.97 |
| `amount_total` (with currency) | 275 | 30 + 25 | 0.97 invoices, ~0.85 contracts | 0.87 |
| `amount_total` without currency | few | — | prior | 0.54 |
| `amount_net` / `amount_gross` | 29 / 29 | all | 1.00 | 0.87 |
| `email_domain` | 610 | 25 + 10 distinct | 1.00 | 0.99 |
| `web_domain` URL / `www.` | 692 | 40 | 1.00 | 0.97 |
| `web_domain` bare (after the `Ref.No` fix) | ~250 | 25 | 1.00 | 0.9 |
| `phone` international | 779 | 40 + 25 + 15 | 0.99 | 0.95–0.97 |
| `phone` `(AAA) BBB-CCCC` without label | — | ~15 in the samples | 1.00 | 0.9 |
| `vat` | 120 (4 distinct) | all distinct | 1.00 | 0.97–0.98 |
| `iban` | 28 (1 distinct) | all | 1.00 | 0.97 |
| `bic` | 79 (≈10 distinct) | all distinct | 1.00 | 0.95 |
| `us_ein` | 74 (3 distinct) | all distinct | 1.00 | 0.9 |
| `rs_pib`, `rs_mb`, `hr_oib`, `rs_account`, `lei`, `duns`, `jmbg_count` | 0 | — | prior | see `conf` |

The corpora are English: Serbian and Croatian identifiers, Serbian dates and amounts are covered by
tests only and their confidences are priors from the check strength and the label (documented in
`conf`). The private invoices come from few issuers, so their samples are repetitive.

All checksums are cross-checked against python-stdnum 2.2 on ~45 000 random and generated-valid numbers
(PIB, OIB, IBAN, LEI, VAT of AT BE DE ES FR GB IT NL PL SI): 0 mismatches. The matični broj algorithm
is not in stdnum; it was confirmed on published registration numbers.

## Throughput

`cargo bench -p quail-signals --bench scan` (generated, entity-dense text). On real text (release, no
LTO, one thread, shared loaded machine): 11–13 MB/s end to end. About 60 % of that time is
`quail-text` normalization, tokenization and folding (16–17 MB/s alone on the same input).

# quail-payment

What a contract, offer or invoice is paid for and how much, as per-file **candidates**, on
native and OCR text alike. Namespace `payment.*`. Serbian (Latin and Cyrillic) and English.
Rules only; no model, no network.

- Analyzer: `payment.extract@1` (`PaymentExtract`), profile `Fast`, requires `document`, runs
  after `revised_document` (OCR text when a step provides it) and as a **finalizer**, so it
  reads the facts of the other steps: `item.line` and `item.table_total` (`quail-items`) and
  `ref.instrument_kind` (`quail-refs`). Registered before the summary finalizers.
- Public API: `PaymentExtract`, `keys::*`, and the rule modules `lex`, `cues`, `amount`,
  `subject`, `category` (`CATEGORIES`, `categorize`).

## Keys

| key | value |
|---|---|
| `payment.subject` | Record `{text, source}`: what is paid for, as written; `source` is `clause`, `after_heading`, `line_items` or `title` |
| `payment.category` | Record `{code, label, keyword}`: see the taxonomy below |
| `payment.amount` | Money: the price the document commits to |
| `payment.amount_basis` | Text: `gross`, `net` or `unstated` |
| `payment.currency` | Text: ISO 4217 code of the price |
| `payment.net`, `payment.vat`, `payment.gross` | Money, each where the document states it |
| `payment.period` | Text: `one_off`, `monthly`, `quarterly`, `yearly` |
| `payment.lines` | List of Record `{description, quantity, unit, unit_price, total, currency, vat_rate}` |
| `payment.highlight` | Record `{key, page, block, start, end, quote}`, one per fact above (one per line) |

Every fact has a `conf` and evidence (page and byte span). A fact with no cue in the text is
**absent**: nothing is guessed.

## Highlights

`payment.highlight` uses the shared shape of `quail_core::highlight`: the fact key, the page,
the block's index among all the document's blocks, the character range in that block's text
and the quote. Blocks are numbered as doewood's `GetDocumentText` returns them (blank blocks
skipped, the cells of a table row merged with tabs), so a reader of the stored text can mark
the passage directly. Later steps that highlight their facts emit `<namespace>.highlight`
with the same record.

## How it works

The text is read as lines of tokens with folded keys (`quail_text::fold`: lowercase, no
diacritics, Cyrillic → Latin), so one cue list covers `Predmet ugovora`, `PREDMET UGOVORA`
and `Предмет уговора`.

- **Price.** An amount after a price cue on the same line, or alone at the start of the next
  line (a label cell and its value cell):
  - Serbian: `Ukupna vrednost (ugovora)`, `Vrednost ugovora`, `Ugovorena cena`,
    `Ukupno za uplatu`, `Za uplatu`, `Ukupno`, `Cena`, `Naknada`, `Mesečna naknada`,
    `Zakupnina`, `Iznos`;
  - English: `Contract Value`, `Total Price`, `Total fee`, `Amount Due`, `Grand Total`,
    `Total`, `Subtotal`, `Fee`, `Monthly fee`, `Price`, `Rent`, `Amount`.

  Numbers and currencies are read by `quail_text::money` (shared with `quail-signals` and
  `quail-items`): `1.234.567,89`, `1,234,567.89`, `1 234 567,89`, `RSD`, `din.`, `дин.`,
  `dinara`, `€`, `EUR`, `evra`, `$`, `USD`. An amount without a currency needs decimals or
  thousand groups and a cue stronger than `Iznos` / `Amount`.

  VAT: `bez PDV-a`, `plus PDV`, `excl. VAT`, `net` after or before the amount make it net;
  `sa PDV-om`, `uključujući PDV`, `incl. VAT`, `gross` make it gross; a line that opens with
  `PDV`, `VAT` or `Tax` gives the VAT amount. Penalties, guarantees, deposits, interest,
  advances, damages and share capital on the line are not the price (unless the cue is a
  strong one); rates (`po satu`, `per hour`, `unit price`, `/h`) never are.

  Per role the strongest cue wins, then the larger amount; two different amounts under
  equally strong cues lower the confidence. `payment.amount` is the gross amount when stated,
  else the plain total (gross when it equals net + VAT), else the net amount. Net + VAT =
  gross (within 0.5 %) raises the confidence of all three, a mismatch lowers it. Table
  totals (`item.table_total`) fill a role the text left empty.
- **Currency.** The price's own; else the one currency every price candidate names.
- **Period.** A period word on the price's line (`mesečna naknada`, `mesečno`, `per month`,
  `kvartalno`, `godišnje`, `annually`, `jednokratno`, `one-time`), else on another line about
  paying (`Naknada se plaća mesečno`), else `one_off` for an invoice (`Faktura`, `Račun`,
  `Invoice`, or `ref.instrument_kind` = `invoice`). A monthly fee is reported as it is, never
  multiplied into a total.
- **Subject.** The text after a label to the end of its sentence (wrapped lines included):
  `Predmet ovog ugovora je`, `Predmet ugovora:`, `Predmet nabavke`, `Opis usluge`,
  `Subject of the Agreement`, `Scope of Services`, `Scope of Work`, `Services:`,
  `Description:`. A label alone on its line (a heading) takes the paragraph under it. Else the
  first three line descriptions, else a title that names something specific
  (`UGOVOR O ODRŽAVANJU SOFTVERA`, not `UGOVOR O PRUŽANJU USLUGA`).
- **Category.** Keyword weights over the subject's words, from one table
  (`category::CATEGORIES`): `it_software`, `telecom`, `legal`, `accounting_audit`,
  `consulting`, `marketing`, `rent_lease`, `utilities`, `fuel_energy`, `logistics_transport`,
  `cleaning_facility`, `maintenance_repair`, `insurance`, `goods_materials`,
  `financial_services`, and `other` when a subject matches no keyword. Specific words weigh 3
  (`softver`, `osiguranje`), generic ones 1 (`održavanje`, `oprema`), so `održavanje softvera`
  is IT. A tie keeps the first category of the table at a low confidence. To extend it, add
  keywords or a category to the table.
- **Lines.** The `item.line` facts with a confidence of at least 0.4 (`MIN_LINE_CONF`), at
  most 200.

## Limits

Reads at most `Limits::max_text_bytes` (and `MAX_SCAN_BYTES`, 8 MiB) and 200 000 lines,
400 tokens per line, skips pages that still need OCR, checks the deadline per page. Every rule
looks a bounded number of tokens around a cue, so the scan is linear in the text.

## Tests

`tests/payment.rs` builds contracts and invoices in code (Latin, Cyrillic, English, DOCX-style
tables). The CLI tests (`crates/quail-cli/tests/payment.rs`) run generated PDFs and a Cyrillic
text file through `quail scan`; `payment_ocr.rs` (feature `ocr`) renders the same PDFs to
200 dpi image-only scans, reads them with `--profile deep` and checks that the same payment
facts come out.

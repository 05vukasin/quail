# quail-items

Price items per file: one record per offered or billed service or product line, so that a
later service can answer "who is cheapest for X" across suppliers. Namespace `item.*`.

The crate reports each line as written plus normalized fields and top-3 category candidates
(EU CPV 2008). It never decides that two suppliers' lines are the same service.

- Analyzer: `item.lines@5` (`ItemLines`), requires `document`, runs after `revised_document`,
  reads `Ctx::document()`. Line extraction is `Profile::Fast`; category candidates are added
  from `Profile::Standard` on. The CPV index is built lazily on the first file that needs
  categories (about 0.16 s, about 9 MB RSS) and shared by every clone of the analyzer.
- Public API: `ItemLines` (`new`, `with_index`, `index`, `index_built`), `CpvIndex`
  (`embedded`, `from_tsv`, `top`, `top_with`, `categorize`, `entry_of_code`, `heap_bytes`),
  `cpv::{ALIAS_SCORE, ALIAS_STEP, MIN_SCORE}`, `desc_key`, `keys::*`, `MAX_ITEMS`, `TOP_K`.

## Keys

| key | value | what |
|---|---|---|
| `item.line` | record | one price item, fields below |
| `item.table_total` | record `label kind amount currency table_index row_index` | a subtotal / VAT / total row; `kind` is `subtotal net vat gross total discount` |
| `item.count` | Int | number of `item.line` facts |

`item.line` fields (optional ones only when known), in this order:

| field | value |
|---|---|
| `description` | Text, as written |
| `desc_key` | Text, folded (lowercase, no diacritics, Latin script, single spaces) |
| `code` | Text, article / item code column |
| `section` | Text, the last section row above the line in its table |
| `quantity` | Float |
| `unit` | Text: `pcs h day week month year min m m2 m3 km kg g t l kwh lump_sum set pack pair person night trip page spot item service sqft` |
| `unit_raw` | Text, the unit as written |
| `unit_price`, `unit_price_gross` | Money (net and gross when both columns exist) |
| `total`, `total_gross` | Money |
| `currency` | Text, ISO 4217 (cell, column header, table, or the document's dominant currency) |
| `vat_rate` | Float, percent |
| `vat_included` | Bool; absent = unknown |
| `valid_from`, `valid_to` | Date, when written on the line |
| `category_candidates` | List of record `code label label_sr score`, best first, at most 3 (Standard+) |
| `source` | `table` (DOCX cells), `layout` (table rebuilt from PDF geometry), `text` (one text line) |
| `table_index`, `row_index` | Int, for table lines |

Evidence: page, zone (`table`, or `body` for text lines), span of the description cell (or
line) in the page text, the row's box when the format has geometry. `conf` reflects the source
(DOCX table with header 0.85, PDF layout with header 0.75, guessed roles 0.5–0.6, text lines
0.35–0.7), +0.1 when quantity × unit price = total, −0.1 when it does not, +0.03 with a unit.

## How lines are found

1. **DOCX tables** (`TableCell` blocks): header detection in English and Serbian (Latin and
   Cyrillic, folded): `Naziv/Opis/Usluga`, `Jedinica mere/JM`, `Količina/Kol.`,
   `Cena/Jedinična cena`, `Iznos/Ukupno`, `PDV`, `Description/Item`, `Qty`, `Unit`,
   `Unit price/Rate`, `Amount/Total`, `VAT`. Two-row headers (`Cena` over `bez PDV`|`sa PDV`),
   the column-numbering row of tender forms (`1 | 2 | 3 | 6=4x5`), tables continued on the next
   page (previous header reused), and roles guessed from content when there is no header
   (text column, unit column, numeric columns checked by `qty × price = amount` and by
   net/gross VAT ratios; rate matrices such as `Role | Onshore | Offshore` give one item per
   cell with the column label in the description). Total rows go to `item.table_total`.
2. **PDF**: `quail-pdf` gives paragraph blocks with their measured lines (`Block.lines`). Lines
   keep their measured boxes; a block without them, or with a line holding several cells
   (a run of two or more spaces, whose box spans the columns between them), is split into
   lines with estimated boxes (height divided evenly, width by length). Lines are grouped
   into rows by vertical centre, table regions found from
   a header row or runs of rows with money, columns found by an x-projection profile (one
   merged cell does not merge two columns; a cell spanning several columns is split by word
   position), then parsed as above. Without a header only amounts written as money (currency,
   two decimals, thousand groups) count.
3. **Text lines**: `description …… 2 x 3.500,00 = 7.000,00 RSD`, merged table rows checked by
   arithmetic, rate cards (`Senior consultant – 85 EUR/h`). Sentences (verbs, `iznosi`,
   `shall not exceed`, long tails) are rejected.
4. **Categories**: `CpvIndex::categorize`. First a small curated alias layer (`src/alias.rs`,
   about 60 English and Serbian phrases): `consulting`, `project management`, `medical
   writing`, `Phase III`, `TFLs`, `SDTM`, `SaaS subscription`, `software licence`, `legal`,
   `travel`, `T&E`, role titles (`Senior Consultant`, `Principal – Onshore`), and others, each
   mapped to the CPV service codes a buyer would pick (score 0.8, then 0.78, 0.76). Then
   TF-IDF cosine over folded word stems and character trigrams of the CPV labels (English
   and Serbian), the better label per code, top 3 above 0.12. Lines billed by time or
   reading as a role or service prefer business, IT, research and health services
   (divisions 70–85, ×1.15; other services ×1.05; goods and works ×0.85), and their query
   leaves out billing words (`onshore`, `senior`, `fees`, months, `Phase`).
   The index keeps postings in compressed sparse rows with terms packed into integers.

Numbers and currencies are read by `quail_text::money` (shared with `quail-signals`): `1,200.50`,
`1.200,50`, `1 200,50` (also no-break spaces), `1'200.50`, up to four decimals resolved by the
document's decimal separator; a minus sign, and an amount standing alone in parentheses
(`(1,200.00)`, a credit) are negative.

The CPV index (`data/cpv2008.tsv`, 620 kB, EU Regulation 213/2008 English labels + the Serbian
UJN translation) is embedded; see `data/README.md` for sources and licence, and
`tools/cpv/fetch.sh` to rebuild it.

## Limits

At most `MAX_ITEMS` (5 000) items per file (then a problem), tables up to 5 000 rows × 64
columns, 24 rebuilt columns per PDF table, 20 000 lines per page; the deadline is checked per
page and per table. Scanned pages without a text layer produce nothing unless OCR filled a
`RevisedDocument`.

## Not here

Document-level amounts, dates and VAT numbers (`quail-signals`), parties (`quail-parties`),
cross-supplier matching of services (the service, at query time).

## Summary and credit rows

Not offered lines, so they go to `item.table_total`, not `item.line`: subtotal/VAT/total rows,
formula rows (`Operations (E+F+G)`, `(C = A - B)`), the value before or after a change
(`Original SOW`, `Revised Professional Fees`), and credits and discounts (`SalesIQ Credit`,
`3% partnership discount`, any line with a negative amount; kind `discount`). Descriptions
holding two or more amounts (`Duns #: 8,690.72 GBP 0.00 1,738.14 GBP`) are dropped. In
documents that write `1,200.50`, a plain space never groups thousands (`88 131 131` are
numbers of adjacent cells).

## Versions

- `item.lines@5`: descriptions by token shape (`src/desc.rs`). A layout or text description
  holding an air time is split at its dates, times, weekdays and spot codes, and the run of
  words that reads most like a description is kept (`3 WBTV M 04/21/14 11:24 PM WBTV News @ 11p`
  becomes `WBTV News @ 11p`; the code goes to `code`). Without a time only dates split it, so
  product names keep their codes. Dayparts (`M-F`) are kept, make-good notes (`® MG for …`)
  dropped. Without a header the most text-like column is the description (program over
  station, network and code columns); a station or code alone under a program row takes that
  program. A header role over an empty column moves to the unnamed column its values sit in;
  a price header over a text column without an amount is dropped; `Total Spots` is a
  quantity; period labels (`2024`, `Year 2`) are column labels.
  RealKIE FCC dev: F1 46.2 to 51.7; text-layer documents without a line 16 to 4.
- `item.lines@4`: PDF tables from the measured line boxes of `Block.lines` (estimated boxes
  only for blocks without them or with merged cells). RealKIE FCC dev: F1 42.6 to 46.2.

- `item.lines@3`: summary, formula, before/after-change and credit rows go to
  `item.table_total`; multi-amount descriptions dropped; CPV alias layer and stronger service
  preference; compact, lazily built index (same TF-IDF scores).

- `item.lines@2`: numbers via `quail_text::money`; credits in parentheses are negative;
  amounts grouped with no-break spaces are read; `km.` is never a currency.
- `item.lines@1`: first version.

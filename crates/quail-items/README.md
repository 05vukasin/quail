# quail-items

Price items per file: one record per offered or billed service or product line, so that a
later service can answer "who is cheapest for X" across suppliers. Namespace `item.*`.

The crate reports each line as written plus normalized fields and top-3 category candidates
(EU CPV 2008). It never decides that two suppliers' lines are the same service.

- Analyzer: `item.lines@1` (`ItemLines`), requires `document`, runs after `revised_document`,
  reads `Ctx::document()`. Line extraction is `Profile::Fast`; category candidates are added
  from `Profile::Standard` on.
- Public API: `ItemLines` (`new`, `with_index`), `CpvIndex` (`embedded`, `from_tsv`, `top`,
  `top_with`), `desc_key`, `keys::*`, `MAX_ITEMS`, `TOP_K`.

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
2. **PDF**: `quail-pdf` gives paragraph blocks with one box each. Blocks are split into lines
   with estimated boxes, lines grouped into rows by vertical centre, table regions found from
   a header row or runs of rows with money, columns found by an x-projection profile (one
   merged cell does not merge two columns; a cell spanning several columns is split by word
   position), then parsed as above. Without a header only amounts written as money (currency,
   two decimals, thousand groups) count.
3. **Text lines**: `description …… 2 x 3.500,00 = 7.000,00 RSD`, merged table rows checked by
   arithmetic, rate cards (`Senior consultant – 85 EUR/h`). Sentences (verbs, `iznosi`,
   `shall not exceed`, long tails) are rejected.
4. **Categories**: `CpvIndex`, TF-IDF cosine over folded word stems and character trigrams of
   the CPV labels (English and Serbian), the better label per code, top 3 above 0.12; lines
   billed by time or reading as a role/service prefer service divisions (50–98).

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

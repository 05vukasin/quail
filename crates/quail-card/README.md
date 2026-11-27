# quail-card

The normalised **key-facts card** of a contract or invoice. Namespace `card.*`.

- Analyzer: `card.facts@1` (`KeyFactsCard`), profile `Fast`, requires `document`, a
  finalizer registered after `contract.facts`. It reads no text of its own: per field it
  takes the most confident fact of the other steps and writes it in one canonical shape, so
  documents can be searched and filtered without reading them.
- Shapes: dates are `Date` (`YYYY-MM-DD` in `card.text`), amounts `Money` (exact decimal and
  ISO 4217 code), durations `{iso, months?, days?}` (years as 12 months, weeks as 7 days),
  codes lowercase, lists of text for multi-valued fields.
- Every card fact keeps the evidence of the fact it came from and has one `card.highlight`
  per source fact (shape of `quail_core::highlight`). A field with no source is absent.

| key | value | from |
|---|---|---|
| `card.kind` | Text | `ref.instrument_kind` |
| `card.number` | Text | `ref.doc_number` |
| `card.supplier`, `card.customer` | Record `{name, pib?, mb?, vat?, reg?, account?, address?, representative?}` | `contract.party` and its details; else `party.company` with the role |
| `card.signed`, `card.start`, `card.end`, `card.issued`, `card.due` | Date | `signal.date_signed`, `_effective`, `_expiry`, `_invoice`, `_due` |
| `card.term` | Record `{iso?, months?, days?, indefinite?}` | `contract.term` |
| `card.auto_renewal` | Bool | `contract.renewal` |
| `card.notice` | Record `{iso, months?, days?}` | `contract.notice` |
| `card.payment_days` | Int | `contract.payment_days` |
| `card.payment_method` | Text | `contract.payment_method` |
| `card.amount`, `card.amount_basis`, `card.currency`, `card.period` | Money, Text | `payment.*` |
| `card.subject`, `card.category` | Text | `payment.subject`, `payment.category` |
| `card.accounts` | List of Text | `signal.iban`, `signal.rs_account` |
| `card.advance` | Record `{percent?, amount?}` | `contract.advance` |
| `card.indexation` | Text | `contract.indexation` |
| `card.penalty` | Record `{percent?, per?, amount?, cap_percent?}` | `contract.penalty` |
| `card.guarantees` | List of Text | `contract.guarantee` |
| `card.liability_cap` | Record `{amount?, percent?, basis?, months?}` | `contract.liability_cap` |
| `card.confidentiality` | Bool | `contract.confidentiality`, or an NDA |
| `card.sla` | Record `{availability?, response_hours?, resolution_hours?}` | `contract.sla` |
| `card.warranty` | Record `{iso, months?, days?}` | `contract.warranty` |
| `card.governing_law` | Text (ISO 3166) | `contract.governing_law` |
| `card.court` | Text | `contract.court` |
| `card.arbitration` | Text | `contract.arbitration` |
| `card.languages` | List of Text (ISO 639-1) | `contract.language` |
| `card.annexes` | List of Text | `contract.annex` |
| `card.amends` | Text | `ref.cites` with relation `amends` |
| `card.text` | Text | the card as `field: value` lines |
| `card.highlight` | Record `{key, page, block, start, end, quote}` | one per source fact |

`card.text` is meant for a prompt or a search index: one line per field, in the order above,
values in their normalised text form (`signed: 2026-01-15`, `amount: 1200000.00 RSD`,
`notice: iso=P30D; days=30`).

## Tests

`tests/card.rs` runs the contracts of `quail-contract`'s tests through every step the engine
runs before the card and checks each field and its highlight.

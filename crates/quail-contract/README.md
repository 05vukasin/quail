# quail-contract

The key facts of a contract or invoice, each with a **highlight**, as per-file candidates on
native and OCR text alike. Namespace `contract.*`. Serbian (Latin and Cyrillic) and English.
Rules only; no model, no network.

- Analyzer: `contract.facts@1` (`ContractFacts`), profile `Fast`, requires `document`, runs
  after `revised_document` (OCR text when a step provides it) and as a **finalizer**,
  registered after `payment.extract` and before `card.facts`.
- It reuses what other steps found instead of finding it again: company names
  (`party.company`), identifiers checked by their check digits (`signal.rs_pib`,
  `signal.rs_mb`, `signal.vat`, `signal.iban`, `signal.rs_account`, `signal.bic`), dates
  (`signal.date`), the English term, notice, renewal, termination and governing-law clauses
  (`clause.*`) and the instrument kind (`ref.instrument_kind`). Document numbers and cited
  documents stay `ref.*`; dates with roles stay `signal.date_*`; the normalised one-value
  view of everything is `card.*` (`quail-card`).
- Public API: `ContractFacts`, `keys::*`, the sentence and reader helpers (`text`) and the
  duration reader (`duration`).

## Keys

| key | value |
|---|---|
| `contract.party` | Record `{role, label, name}`: `role` is `supplier` or `customer`, `label` the role word as written |
| `contract.party_id` | Record `{role, party?, kind, value, checked}`: `kind` is `pib`, `mb`, `vat`, `reg`, `iban`, `account`, `bic`; `checked` when `quail-signals` confirmed the check digits |
| `contract.address` | Record `{role, party?, text}` |
| `contract.representative` | Record `{role, party?, name?, title?}` |
| `contract.signatory` | Record `{role, label, name?, title?}` |
| `contract.term` | Record `{iso?, months?, days?, indefinite?, text}` |
| `contract.renewal` | Record `{auto, iso?, same_period?, text}` |
| `contract.notice` | Record `{iso, days?, months?, text}` |
| `contract.termination` | Record `{kind, text}`: `breach`, `convenience`, `insolvency`, `mutual`, `force_majeure`, `notice`, `change_of_control` |
| `contract.payment_days` | Record `{days, business?, from?, text}`: `from` is `invoice_receipt`, `invoice_date`, `delivery`, `signing` |
| `contract.payment_method` | Text: `bank_transfer`, `cash`, `card`, `compensation` |
| `contract.advance` | Record `{percent?, amount?, text}` |
| `contract.indexation` | Record `{basis, text}`: `cpi`, `inflation`, `exchange_rate`, `fixed`, `other` |
| `contract.late_interest` | Record `{statutory, percent?, text}` |
| `contract.penalty` | Record `{percent?, per?, amount?, cap_percent?, text}` |
| `contract.guarantee` | Record `{kind, purpose?, percent?, amount?, text}` |
| `contract.liability_cap` | Record `{amount?, percent?, basis?, months?, text}` |
| `contract.confidentiality` | Record `{iso?, text}` |
| `contract.sla` | Record `{metric, percent?, iso?, hours?, text}`: `availability`, `response`, `resolution` |
| `contract.deadline` | Record `{iso?, days?, date?, text}` |
| `contract.warranty` | Record `{iso, months?, days?, text}` |
| `contract.governing_law` | Record `{code, jurisdiction, text}` |
| `contract.court` | Record `{court, city?, text}` |
| `contract.arbitration` | Record `{institution?, rules?, seat?, text}` |
| `contract.language` | Record `{languages, prevails?, text}` |
| `contract.annex` | Record `{label, title?}` |
| `contract.amendment` | Record `{action, article?, text}`: `change`, `delete`, `add` |
| `contract.highlight` | Record `{key, page, block, start, end, quote}`, one per fact above |

Every fact has a `conf`, evidence (page and byte span: the cue and the value it read) and a
highlight in the shared shape of `quail_core::highlight`. `text` fields quote the sentence
(at most 300 characters). A fact with no cue is **absent**: nothing is guessed. Keys of which
a document has one (term, renewal, notice, payment days and method, indexation, late
interest, liability cap, confidentiality, warranty, governing law, court, arbitration,
language) keep the most confident; the others keep one fact per kind (per role, per
identifier, per metric, per annex label), at most 16 per key.

Names of representatives and signatories are emitted as the contract writes them (they are
who signs it); `quail-parties` keeps emitting titles only.

## How it works

The text is read as lines of folded tokens (`quail_payment::lex`: lowercase, no
diacritics, Cyrillic → Latin) and joined into sentences across line breaks (a sentence ends at
`.`/`!`/`?` before a capital or a number, not after `br.`, `d.o.o.` or an initial; at an
enumeration `2.`, a heading `Član 3`, `Article`, `Prilog`, a title in capitals, a signature
label). Every rule finds a cue phrase and reads a bounded number of tokens around it.

- **Parties.** A role mark is a label at the start of a line (`Naručilac:`, `Supplier:`) or
  a defined term (`(u daljem tekstu: Izvođač)`, `(hereinafter the "Customer")`,
  `(the "Supplier")`). The text before the mark (back to the previous mark of the sentence),
  or the label line and the detail lines under it (`PIB:`, `MB:`, `Adresa:`, ...), is the
  party's: its `party.company` mention is the name, and in it `PIB`, `MB`, `matični broj`,
  `VAT No.`, `Registration No.`, `tekući račun`, `IBAN`, `SWIFT` give identifiers (shape
  checked; `checked` and a higher confidence when `quail-signals` found the same value with
  valid check digits), `sa sedištem u`, `ul.`, `registered office at` the address, and
  `koga zastupa`, `zastupnik`, `represented by` the representative (title words such as
  `direktor`, `generalni direktor`, `CEO`, `Managing Director` and a capitalised name of two
  or three words, either order). `, represented by ...` right after an English mark belongs
  to that party. Signature blocks: a line `Za Naručioca`, `U ime Izvođača`, `For the
  Supplier`, `On behalf of the Customer` (one label per line; two columns are skipped), then
  the name and title on it or on the next three lines, `Name:` / `Title:` labels included.
- **Term.** `zaključuje se na period od`, `u trajanju od`, `važi`, `for a term of`,
  `for a period of`, `remain in force for` with a duration (`12 (dvanaest) meseci`,
  `godinu dana`, `two (2) years`) in a sentence about the contract and not about a warranty,
  confidentiality, notice, payment or delivery; `na neodređeno vreme`, `indefinite term`.
  Renewal: `automatski se produžava`, `prećutno se produžava`, `automatically renew`,
  negated forms give `auto: false`. Notice: `otkazni rok od`, `pismeno obaveštenje od`,
  `notice period of`, `30 days' notice` in a termination sentence. Termination kinds from
  `kršenje`, `povreda`, `breach`, `bez navođenja razloga`, `for convenience`, `stečaj`,
  `insolvency`, `sporazumno`, `mutual agreement`, `viša sila`, `force majeure`,
  `change of control` in a sentence about ending the contract (`raskid`, `otkaz`,
  `terminate`).
- **Payment terms.** `u roku od 15 dana`, `rok plaćanja`, `valuta`, `within 30 days`,
  `net 30` near a pay word, with what the days run from (`od dana prijema fakture`,
  `of receipt of the invoice`, `od dana isporuke`); `virmanski`, `bank transfer`, `u gotovini`;
  `avans`, `advance payment` with a percentage or amount; `indeks potrošačkih cena`, `CPI`,
  `inflacija`, `srednji kurs NBS`, `fiksna cena`, `prices shall remain fixed`;
  `zakonska zatezna kamata`, `late payment interest`.
- **Obligations.** `ugovorna kazna`, `penal`, `liquidated damages` with a rate, `za svaki
  dan` / `per week` and a cap (`najviše 10%`, `up to a maximum of`); `bankarska garancija`,
  `menica`, `performance bond`, `letter of credit`, `depozit` with a purpose (`za dobro
  izvršenje posla`, `povraćaj avansa`, `garantni rok`) and a negation check; liability limited
  (`ograničena je na`, `shall not exceed`) to an amount, a percentage, the contract value or
  the fees paid; `poverljive informacije`, `poslovna tajna`, `confidential` in a sentence
  that obliges, with how long; `dostupnost 99,5%`, `availability`, `vreme odziva`,
  `respond within`, `rok za otklanjanje`, `resolve within`; `rok isporuke`, `isporučiti u
  roku od`, `deliver within`, a date of `signal.date`; `garantni rok`, `warranty period`.
- **Law.** `merodavno je pravo`, `primenjuje se pravo / zakon` with `Republike Srbije`,
  `Zakona o obligacionim odnosima` (Serbia) and neighbouring jurisdictions; the English clause
  of `quail-clauses` otherwise. Court: `nadležan je Privredni sud u Beogradu`, `courts of
  London` in a sentence about jurisdiction or disputes, normalised to one script
  (`Privredni sud u Nišu`) and the city in the nominative. Arbitration: the institution
  (`Spoljnotrgovinska arbitraža pri Privrednoj komori Srbije`), the rules (`ICC`, `LCIA`,
  `UNCITRAL`, `VIAC`), the seat; negated forms are skipped. Language: `sačinjen na srpskom
  jeziku`, `in the English language`, and the version that prevails.
- **Document.** Lines that start with `Prilog 1:`, `Annex A -`, `Schedule 1:`, `Exhibit B`,
  and the items under `Prilozi:` / `Annexes:`. In an amendment (`ref.instrument_kind`, or a
  title `ANEKS`, `AMENDMENT`): `menja se član 2`, `briše se`, `dodaje se`, `is hereby
  amended`, `is deleted`, with the article.

## Limits

Reads at most `Limits::max_text_bytes` (and 8 MiB) and 200 000 lines, 400 tokens per line,
sentences of at most 160 tokens, skips pages that still need OCR and checks the deadline per
page. Every rule looks a bounded number of tokens around a cue, so the scan is linear.

## Tests

`tests/contract.rs` builds contracts in code (Serbian Latin and Cyrillic, English, an
amendment, the same contract as OCR text) and checks every fact, that each fact has one
highlight whose quote is the block text, and negatives that must not match. The CLI tests
(`crates/quail-cli/tests/key_facts.rs`) run generated PDFs and a Cyrillic text file through
`quail scan`; `key_facts_ocr.rs` (feature `ocr`) renders the PDFs to 200 dpi image-only scans,
reads them with `--profile deep` and compares the card field by field.

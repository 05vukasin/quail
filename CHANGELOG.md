# Changelog

## Unreleased

### Key contract facts and the key-facts card (#129)

- New step `contract.facts@1` (`quail-contract`, feature `contract`, in the default build): the
  key facts of a contract or invoice, each with a `contract.highlight`, on native and OCR text,
  in Serbian (Latin and Cyrillic) and English: parties with their role, identifiers (PIB, MB,
  VAT, registration number, accounts, IBAN, BIC), address, representative and signatories;
  term, renewal, notice period and termination rights; days to pay, payment method, advance,
  indexation and late-payment interest; penalties, guarantees, liability caps,
  confidentiality, service levels, deadlines and the warranty period; governing law, court,
  arbitration and language; annexes and what an amendment changes. It reuses the facts of
  the other steps (`party.company`, `signal.*` identifiers and dates, `clause.*`, `ref.*`)
  instead of finding them again. Nothing is guessed: a fact without a cue is absent.
- New step `card.facts@1` (`quail-card`, feature `card`, in the default build): the
  normalised key-facts card `card.*`, one canonical value per field (ISO dates, money,
  durations in months or days, codes), with `card.text` and a `card.highlight` per source
  fact.
- `quail-fuzz` runs both steps.

### Payment facts (#128)

- New step `payment.extract@1` (`quail-payment`, feature `payment`, in the default build): what
  a contract, offer or invoice is paid for and how much, on native and OCR text alike, in
  Serbian (Latin and Cyrillic) and English. Keys: `payment.subject`, `payment.category` (a
  keyword taxonomy in one table), `payment.amount` with `payment.amount_basis`,
  `payment.currency`, `payment.net`, `payment.vat`, `payment.gross`, `payment.period`,
  `payment.lines` (from `item.line`) and `payment.highlight`. Nothing is guessed: a fact
  without a cue is absent.
- `quail_core::highlight`: one shared highlight shape (`key page block start end quote`, the
  fact's `conf`) and `BlockIndex`, which numbers blocks as doewood's `GetDocumentText` does,
  for every step that highlights its facts. Documented in `docs/keys.md`.
- `quail-fuzz` runs `item.lines` and `payment.extract`, and forwards `after` and `finalizer`
  of the wrapped steps.

### PPTX and HTML (#120)

- New extractors `pptx.extract@1` (`quail-pptx`) and `html.extract@1` (`quail-html`), in
  the default `quail` build (features `pptx` and `html`). PPTX: one page per slide, reading
  order, titles, bullets, tables, speaker notes. HTML: headings, paragraphs, lists and tables
  as blocks, layout tables read as blocks, scripts, styles, navigation and hidden content
  dropped, CSS page breaks as pages, the declared charset, inline XBRL text kept and its
  `dei:` cover facts in `html.xbrl_dei`. Both have the same hostile-input bounds as DOCX.
- `quail_core::Format` gains `Pptx` and `Html` (`"pptx"`, `"html"`). `engine.sniff@3`: a PPTX
  is now `pptx` (it was `zip` with the PPTX MIME type, and is `pptx` without
  `[Content_Types].xml` too), and text that starts like HTML is `html` (`text/html`; it was
  `text`).
- `quail-ooxml`: the zip package, XML walk and relationships of `quail-docx`, moved unchanged
  so `quail-pptx` uses the same code. `docx.extract@1` output is unchanged.

### quail-ocr

- Idle OCR engines are dropped after an idle timeout, so a long-running host gets the ONNX
  Runtime arenas of an engine back after its last scanned page (#115). The default is
  `EnginePool::DEFAULT_IDLE_TIMEOUT`, 5 minutes; set it with
  `OcrPages::with_idle_timeout(Some(d))` (`None` keeps idle engines for the life of the
  analyzer, as before) or `EnginePool::with_idle_timeout`, and drop the idle engines at once
  with `OcrPages::trim` / `EnginePool::trim`. The next page loads a new engine; the output is
  the same.
- Every PP-OCR engine runs its page on a rayon pool of its own with `intra_threads` threads, so
  `oar-ocr`'s pre- and post-processing (resize, normalization, decoding, DB post-processing)
  stays within the OCR thread budget (`jobs × threads`) and never starts rayon's global pool
  (#116). The output is byte-identical for every thread count.

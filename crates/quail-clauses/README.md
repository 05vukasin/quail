# quail-clauses

Contract clauses as per-file **candidates**: governing law, the term and whether it renews, how
long obligations survive, notice periods and termination rights. Namespace `clause.*`. Pure
Rust, no dependencies beyond `quail-core`, English rules.

- Analyzer: `clause.terms@1` (`ClauseTerms`), profile `Fast`, requires `document`, runs after
  `revised_document` (`ctx.document()`, so OCR text is used when a step provides it).
- Public API: `ClauseTerms`, `keys::*`, and the rule modules `law` (`find`, `Basis`), `term`
  (`find`, `renewal`, `termination`), `duration` (`duration_at`), `places` (`PLACES`,
  `place_at`), `tokens` (`Stream`).

## Keys

| key | value |
|---|---|
| `clause.governing_law` | Record `{jurisdiction, code, raw, basis}`: `Delaware`, `US-DE` (ISO 3166-2, or ISO 3166-1 for a country), the phrase as written, `laws_of` / `place_law` / `bare_laws_of` / `forum` |
| `clause.term` | Record `{duration, text, raw, renewal}`: `P2Y`, `2 years` (units as written: `12 months` stays months), the sentence up to the duration, `auto` / `none` / `unknown` |
| `clause.survival` | Record `{duration, text, raw}`: how long obligations survive (`obligations of confidentiality shall survive for five years`) |
| `clause.notice_period` | Record `{duration, text, raw}`: `30 days` before `prior written notice` |
| `clause.termination` | Record `{kind, raw}`: `convenience`, `breach`, `insolvency`, `change_of_control` |

One fact per distinct value (the best occurrence's conf and evidence), at most 4 per key, most
confident first. Evidence: page and byte span of the phrase in the page text.

## How it works

The text of all pages is one token stream (words and single punctuation characters, with page
and byte span), so clauses wrapped over line and page breaks read as one.

- **Governing law**:
  - Anchor: `law`/`laws` with a governing word (`governed`, `governing`, `construed`, `interpreted`, `enforced`, `applicable`) at most 14 tokens before it.
  - Then `of (the) State / Commonwealth / Province / Republic of` and a place from `places::PLACES`: the 50 US states, DC and Puerto Rico, Canadian provinces, Australian states, UK nations, about 50 countries. The longest name wins (`New South Wales`, `England and Wales`).
  - Or a place right before `law` (`governed by Delaware law`, `Texas law shall govern`).
  - `the laws of the United States and the State of X` gives X.
  - Without a governing word, `the laws of X` is an incorporation statement (`bare_laws_of`, 0.5). The forum (`courts of the State of X`) gives 0.6. Both are halved when a governing-law clause names a place.
- **Durations**: `two (2) years`, `2 (two) years` (the digits win when the two disagree), `twenty-four (24) months`, `a two-year period`, `thirty (30) calendar days`, `5.5 months`.
- **Term**, read from the sentence before the duration (24 tokens at most):
  - an end verb (`terminate`, `expire`, `remain`, `continue`, `survive`, `in effect`, `in force`) with `this Agreement`: **0.92** with a start anchor after it (`from the Effective Date`, `date hereof`, `date of this ...`), 0.55 without one;
  - `term` with `this Agreement`: 0.9 / 0.7;
  - an end verb with obligations, duties, restrictions or confidentiality: `clause.survival`, 0.7 / 0.35;
  - a bare end verb or `term`: only with the anchor, 0.75.
- **What is not a term**:
  - covenant, payment and employment periods (`agrees that, for a period of`, `will not`, `solicit`, `hire`, `employment`, `standstill`, `COBRA`, `installments`);
  - tails after the term (`during the term ... and for a period of`);
  - renewal or extension periods (`successive one-year periods`);
  - deadlines (`within`, `at least`, `per`, `every`);
  - days and weeks score 0.3 lower.
- **Notice periods**: a duration before `(prior) (written) notice`, or after `notice of`.
- **Renewal**: from the words around the term.
  - `auto`: `automatically renew(ed)`, `successive`, `renewal term`.
  - `none`: `shall not be renewed`, `non-renewable`.
  - `unknown`: neither.
- **Termination**: in a sentence with `terminate`:
  - `convenience`: `for convenience`, `without cause`, `at any time`;
  - `breach`: `material breach`;
  - `insolvency`: `bankruptcy`, `insolvency`, `receiver`;
  - `change_of_control`: `change of control`.

## Measured (Kleister-NDA dev, 253 NDAs, `quail-eval`)

| task | support | P | R | F1 | ECE |
|---|---:|---:|---:|---:|---:|
| jurisdiction (`clause.governing_law`, field `jurisdiction`) | 249 | 99.6 | 98.0 | 98.8 | 0.046 |
| term (`clause.term` + `clause.survival`, field `text`) | 89 | 70.2 | 74.2 | 72.1 | 0.064 |

- The Kleister gold leaves the term out of 164 documents, and many of those still state one (`This letter agreement shall expire two years ...`). Those predictions count as FPs, so precision is a lower bound.
- Cost: 2.7 ms per file in a debug build on Kleister (CUAD: 7.5 ms), linear in the text.

## Limits

- Reads at most `Limits::max_text_bytes` (and 16 MiB) of text, and records a `problem` when the limit cuts the text.
- Skips pages that need OCR.
- Checks the deadline before every page and keeps what it read.
- Every rule looks a bounded number of tokens around an anchor.
- At most 4 values per key.
- `raw` is cut at 300 characters.

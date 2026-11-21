# quail-parties

Per-file **candidates** for the companies a document is about, and the roles it gives them.
Namespace `party.*`. It never decides which company owns a file and never compares files: a
resolution service joins the keys across files (union-find on the `key` field of `party.company`, plus
the `party.alias_of` pairs) and assigns documents to suppliers.

Out of scope (sibling crates): identifiers, dates, money and e-mail domains (`quail-signals`);
document numbers, references and titles (`quail-refs`).

- Analyzer: `party.candidates@4` (`PartyCandidates`), profile `Fast`, requires `document`.
- Public API: `PartyCandidates`, `keys::*`, and the pure helpers in `names`
  (`normalize_name`, `normalized_key`, `core_key`, `squashed_key`, `meaningful_words`,
  `core_words`, `score_name_match`, `are_names_similar`, `acronym_key`,
  `lone_word_across_jurisdictions`, `contains_aligned`), `quality` (`classify`, `verdict`,
  `is_role_term`, `is_name_fragment`, `names_a_product`, `is_person_shaped`), `forms::FORMS`,
  `roles::role_of_word`, `scan::domain_labels`.

## How it works

Per page, on the normalized text (`quail_text::normalize_mapped`, spans mapped back to the page
text), tokens with folded keys (`quail_text` fold: Serbian Cyrillic → Latin, no diacritics).
Both come from `text.stats`'s `normalized_text` artifact when it holds the page and the page is
read whole (soft `after(["normalized_text"])`), else they are computed here, with the same result:

1. **Legal forms** anchor a name: `d.o.o.`/`doo`/`DOO`/`д.о.о.`, `a.d.`, `d.d.`, `DOOEL`, `JP`,
   `JKP`, `PR`/`preduzetnik`, `Javno preduzeće`, `Akcionarsko društvo`, `LLC`, `Inc.`, `Corp.`,
   `Ltd`, `Limited`, `plc`, `LLP`, `GmbH`, `AG`, `KG`, `S.A.`, `SAS`, `SARL`, `S.r.l.`, `S.p.A.`,
   `B.V.`, `N.V.`, `AB`, `A/S`, `ApS`, `Oy`, `Kft.`, `Zrt.`, `s.r.o.`, `Sp. z o.o.`, `Pty`,
   `OOO`, ... (dotted, spaced, any script). Weak spellings that are also words (`AS`, `AD`, `SA`,
   `Co.`, `Company`, `Group`) must be written like a form and score lower. The name is the run
   of capitalised words on the same line before (suffix forms) or after (prefix forms) the form,
   at most 8 words, one interior comma allowed ("Brenwick, Dalton and Company"); it stops at
   document words, prepositions, labels, role words, clause references ("Article 12") and
   lowercase prose. It also stops at recital and heading words (`WHEREAS`, `JOINDER OF`), signature-block
   titles (`President`, `CEO`, `VP`, `Director(s)`, `Board`, `Employee`, ...), and where a
   Titlecase word precedes a name set in capitals (`Mark G. Parker NIKE, Inc.` → `NIKE, Inc.`). A
   name that starts its line inside a wrapped paragraph (a line above of eight tokens or more ending
   in a name word) takes the words at the end of that line (`between Vertex` / `Pharmaceuticals
   Incorporated`). A weak form after `of` or after only generic nouns (`Return of Company`, `HOLDING
   CO.`, `Parent Company`) is no name. A seat city after the form ("ACME d.o.o. Beograd") stays in the surface and
   leaves the key. A sole trader (`PR`) needs a two- or three-word personal name before the form
   and no digits. English name shapes the walk keeps (`party.candidates@3`):
   - a capitalised preposition that opens a name mid-sentence (`by At Home Corporation`, `and ON
     Semiconductor`), `&` inside a word (`AT&T Corp.`);
   - a parenthesised part (`GALEN (CHEMICALS) LIMITED`, `Bunker One (USA) Inc.`);
   - a registration number as the first word before `Limited`/`Ltd`/`Inc.`/`Corp.` (`2205925
     Ontario Limited`), a digit-led word before `Company` (`3M Company`);
   - brands mixing cases (`ChipMOS TECHNOLOGIES INC.` is not cut at `ChipMOS`);
   - names wrapped over a line: a strong form that starts its line (`Sibannac,` / `Inc.`), `&` or
     `and` at the break (`Leggett &` / `Platt, Incorporated`), a short line that opens with
     `between`/`and`/`by` (`between Aspect Medical` / `Systems, Inc.`).

   And where it stops: a connector does not reset the capitals rule (`Yu Zhou and GENEXOSOME
   TECHNOLOGIES INC.` → `GENEXOSOME TECHNOLOGIES INC.`), the same first word on both sides of
   `and` starts two names (`Velto ASP and Velto AB`), a column gap of six spaces or a tab
   (`Snap Sample      Ultra Airlines, Inc.` in a two-column signature block), `X and Company`
   when the document defines `"Company"`, a possessive or an initialled person before a weak form
   (`Licensee's Company`, `Brian J. Sample Company`), and runs of column letters before a weak
   form (`V W X Y Z AA AB`).
2. **Role labels** (`Dobavljač:`, `Izvršilac:`, `Naručilac:`, `Kupac:`, `Supplier:`, `Vendor:`,
   `Customer:`, `Bill to:`, `Ship to:`, `Remit to:`, `Consignee`, ...) give the next name its role;
   without a legal form, the name must fill the line, have at least two words and no person's
   shape in any letter case. Side-by-side label columns ("Remit To: Ship To:") assign nothing.
3. **Brands spelled by a domain**: one to three capitalised words that spell a second-level label
   of an e-mail or web domain in the same document ("KESTRELON" + `www.kestrelon.com`).
   Free-mail and platform hosts are ignored. (The domains themselves are `quail-signals`' facts.)
4. **Name statements**: `formerly`, `previously known as`, `f/k/a`, `now known as`, `n/k/a`,
   `renamed`, `trading as`, `t/a`, `d/b/a`, `doing business as`, `prethodno`, `ranije`,
   `bivše` between a found name and the next → `party.alias_of`.
5. **Context** per mention: parties clause (an opener such as `između`, `between`, `entered
   into` before it on the opening page — the first page, or the first of the first three with 60
   words or more when a short cover or exhibit page comes first — or registration details such as `sa sedištem`, `PIB`,
   `matični broj`, `registered office`, `incorporated`, `represented by` after it); defined terms
   (`(u daljem tekstu: Naručilac)`, `(hereinafter the "Supplier")`, `("Company")`); letterhead
   (header zone, or the first 6 lines of page 1 when the extractor gives no zones); footer;
   signature block (`For and on behalf of X`, also with X on the next line, `Za X`, `Za i u ime
   X`, a name alone on its line over a `By:` line, or after a signature cue on the last two
   pages); bank in payment details (lower confidence, no issuer role); the EDGAR filer on a
   `Source: X, 10-K, ...` line (no signatory role); counsel after `with a copy to`.

Every surface passes the reference name-quality gates (ported from the owner's
`name_quality.py`): length (2–120 chars, ≤ 10 words), no line break, e-mail/URL, addressee label,
postal code (five-digit codes, so Serbian addresses too), digits plus an address word, personal
title, "Name, Role", prose fragments, a bare role, a product tail, and no distinctive word
("Republika Srbija", "Group Ltd", "a Florida Corporation").

## Keys

One `party.company` fact per mention: a record on the mention's `Evidence` (page, zone, source
span), `conf` = mention confidence. Fields, in this order:

| field | value |
|---|---|
| `surface` | surface form as written (whitespace, quotes and dashes normalized) |
| `key` | core key: legal forms, connectors and seat city dropped, folded (Cyrillic → Latin, no diacritics, `đ → dj`) |
| `norm` | reference `normalized_key` (NFKD, marks and punctuation dropped; `sym-` + BLAKE2s-64 fallback) |
| `quality` | `ok` or `suspect` (two bare Titlecase words: a person or a brand) |
| `legal_form` | canonical form (`d.o.o.`, `LLC`, `GmbH`, ...); absent without one |
| `jurisdiction`, `jurisdiction_conf` | ISO 3166 alpha-2, lowercase, several joined by `\|`, and its confidence; absent when unknown |
| `roles`, `role_conf` | `supplier`, `customer`, `issuer`, `bill_to`, `ship_to`, `signatory`, `party`, `mentioned`, and a parallel list of confidences (mention conf × role weight) |
| `defined_term` | the term a clause defines for it (`Naručilac`, `Supplier`); absent without one |

Document level: `party.candidates` (top 10 keys by noisy-or of their mentions' confidence, a
list of records `{key, surface, score, roles}`), `party.alias_of` (record `{current, other,
kind}`, kind `former_name` / `trading_name` / `dba`, evidence spans both names), `party.signatory_title`
(titles such as `Direktor`, `Managing Director`, with evidence), `party.signatory_count` (lines of
signature blocks naming a signatory by title or by a `Name:` / `By:` / `Ime i prezime` label).

**Person names** are never emitted on their own: signature blocks give titles and a count, a
formless name with a person's shape is dropped. The exception is a sole trader's registered
business name (`Ime Prezime PR Naziv`), which is the legal name of the business.

## Confidence

`conf` estimates the probability that the mention is a real company that is a party to, or the
issuer of, the document:

| evidence | conf |
|---|---|
| base: strong legal form / weak form / formless (domain brand, name statement, label) | 0.75 / 0.40 / 0.65, 0.60, 0.55 |
| parties clause | +0.12 |
| defined term stating a role / any other defined term | +0.10 / +0.05 |
| letterhead or header / footer | +0.06 / +0.03 |
| signature block | +0.15 |
| after a role label (with a legal form) | +0.30 |
| heading | +0.05 |
| bank in payment details | −0.30 |
| clamp | 0.05 … 0.97 |
| then, when the only role is `mentioned`: × 0.7 on the opening page with a defined term, × 0.3 on it without one, × 0.25 on later pages | |
| then, when the only role is `party` (a clause) on a later page and there is no defined term | × 0.3 |
| then: the EDGAR filer (`Source:` line) at most 0.5; after `with a copy to` × 0.4 | |

The `mentioned` factors are the measured precision of such mentions on the Kleister-NDA and CUAD
dev splits (about 50 %, 15 % and 10 % against about 60–85 % for mentions with any other role), so a
`min_conf` of 0.3 keeps the parties and drops most affiliates, advisers and third parties named in
the body. In `party.candidates`, a key's score is the noisy-or of its mentions that have a role and
of its single strongest mentioned-only mention: a company named twenty times in the body is no more
a party than one named once.

Role confidences (`role_conf`) are `conf × weight` (0.95 for a defined term or a Bill/Ship-to label, 0.9 supplier /
customer / party, 0.85 signatory, 0.8 issuer from a letterhead, 0.6 issuer from a footer).

## Measured

Aggregates only; the private corpus never leaves the machine.

**`quail-eval` party task, dev splits** (`min_conf` 0.3, committed task map, `surface` field
only; `party.candidates@2` → `@3`):

| dataset | support | P | R | F1 | ECE |
|---|---:|---|---|---|---|
| Kleister-NDA | 475 | 63.0 → 67.7 | 67.4 → 71.4 | 65.1 → 69.5 | 0.128 → 0.143 |
| CUAD | 2040 | 75.4 → 79.1 | 40.7 → 44.8 | 52.8 → 57.2 | 0.100 → 0.034 |

About 470 of CUAD's misses are aliases and defined terms (`Licensor`, `the "Company"`), which
`party.company` carries in `defined_term`; scored with that field too, CUAD is F1 76.6.

**Private business corpus** (380 PDF/DOCX files, PDFs through `pdftotext`, DOCX through
`quail-docx`; gold = the supplier names the owner's system recorded per file, 354 files, 330 of
them with a text layer; the tenant's own entities removed from the candidates before scoring):

| metric | value |
|---|---|
| true supplier at rank 1 / in top 3 / anywhere | 0.833 / 0.862 / 0.862 (files with text: 0.894 / 0.924) |
| top-1 precision (files with a non-tenant candidate, n = 307) | 0.961 |
| files with at least one candidate | 335 / 380 (misses are scans without text, brand-only mentions) |

Mention precision per confidence band, counting a mention correct only when it is the gold
supplier or the tenant (a lower bound: other real companies count as wrong; a hand-checked
random sample of the "wrong" ones was about 85 % real company names):

| conf | mentions | party precision |
|---|---|---|
| 0.4–0.5 (weak form mid-text, banks) | 231 | 0.28 |
| 0.5–0.6 | 58 | 0.90 |
| 0.6–0.7 | 260 | 0.84 |
| 0.7–0.8 | 948 | 0.86 |
| 0.8–0.9 | 403 | 0.86 |
| 0.9–1.0 | 202 | 0.96 |

**Kleister-NDA** (train + dev, 337 public NDAs, gold `party=`; scored on the 468 gold parties
with a legal form, persons are never emitted): recall 0.968 among all mentions, 0.902 in the top
2, 0.959 in the top 5; precision of the top-1 candidate 0.903, top-2 0.801.

**CUAD v1** (499 contracts, gold "Parties" answers; 1 118 answers with a legal form): recall
0.860 among all mentions, 0.662 in the top 2, 0.769 in the top 5; top-1 precision 0.889, top-2
0.838.

**Speed** (`cargo bench -p quail-parties --bench parties`, thin LTO, shared laptop; generated
pages dense with parties clauses): whole analyzer 5.2 MiB/s sr-Latn, 4.9 MiB/s en, 2.9 MiB/s
sr-Cyrl (normalization + tokens 8–17 MiB/s, the scan itself 10–15 MiB/s). On the private
corpus: median 0.8 ms per file, p95 22 ms. Robustness: 0 errors, 0 panics on 380 + 836 files;
one file partial (mention cap).

## Limits and cost

- Linear per page: every rule looks a bounded number of tokens around an anchor (walks ≤ 8 words,
  defined terms ≤ 80 tokens, clause windows 25/15 words).
- At most 200 mentions per page (then a `problem`), 3 mentions per key and 64 keys per document
  (the strongest), 16 name statements, 8 titles, 10 ranked candidates. Keys over 64 are dropped
  with a `warn` when every dropped key is mentioned-only with a score under 0.3 (`WEAK_DROP`), and
  with a `problem` when a stronger one is among them or the document has more than 512 distinct
  keys (`MAX_CANDIDATE_KEYS`, a list of names rather than a contract). Distinct keys per document
  on the dev splits: CUAD p99 32, max 61; Kleister-NDA p99 15, max 32 (the cap was 32 before
  `party.candidates@4` and made 4 of 408 CUAD dev records partial).
- Reads at most `Limits::max_text_bytes` (and 16 MiB) of text; a `problem` when it stops early.
  Checks the deadline per page and keeps what it found. Skips `needs_ocr` pages.
- Vocabulary tables are built once (read-only).

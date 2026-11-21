# quail-text

Text utilities every text analyzer builds on, the plain-text extractor and two text-level analyzers.
Namespace `text.*`.

## Utilities

| module | what |
|---|---|
| `normalize` | NFKC, invisible/control characters removed, quotes and dashes folded, whitespace collapsed, `ugo-\nvor → ugovor` (real hyphens only, never dashes); `Cow` result, `normalize_into` buffer reuse, `normalize_mapped` + `OffsetMap` in both directions (normalized ⇄ source spans) |
| `fold` | matching keys: lowercase, diacritics off (`đ → dj` or `d`), Serbian Cyrillic → Latin; `fold_into` never allocates beyond the caller's buffer |
| `translit` | exact Serbian Cyrillic ⇄ Latin, linear; documented Latin→Cyrillic policy (greedy digraphs + exception list; foreign words and Roman numerals kept Latin, acronyms converted (`PDV → ПДВ`) unless `keep_acronyms`); `cyrillic_to_latin_chars` streams |
| `script` | Latin / Cyrillic / Greek / other letter counts and ratios |
| `token` | UAX #29 tokens with byte spans, linear time; kinds `Word Number Alnum Url Email Punct Symbol`; `114/23`, `2023-05-12`, `1.200,50`, `1 200 000,00`, spaced IBANs, `+381`, `C++`, URLs and e-mail addresses kept whole; `keyed_tokens` adds a folded key per token from one reused buffer |
| `shingle` | word shingles and char n-grams as **stable** XXH3-64 hashes (fixed seed, pinned by tests) |
| `garbage` | `garbage_report` / `garbage_ratio`: share of broken-text-layer characters, including Cyrillic read as Latin look-alikes (`OKBMPHM CnOPA3YM`, whole page judged; `is_lookalike()`, `broken_ratio()` without that class); `verdict()` → `Clean`, `Garbage` (ratio > `NEEDS_OCR_RATIO` = 0.10) or `TooShort` (< 20 characters, never an OCR reason by itself) |
| `money` | exact-decimal numbers and amounts: `parse_number` with a `NumberFormat` (`MONEY`: two decimals, no sign; `QUANTITY`: four decimals, sign, `1'200`) and a `DecimalHint` for `1,500`; `parse_money` with currency (symbol, word in any case incl. Cyrillic `дин.`, ISO code in capitals), multiplier (`5 million`, `2 miliona`) and sign (`-1,200.00`, `1.200,00-`, `(1,200.00)`); `Decimal` with `Display`; the currency and multiplier tables. Shared by `quail-signals` and `quail-items` |
| `lang` | `LanguageDetector`: Lingua (high accuracy, 11 languages) + an sr/hr/bs marker layer, scored candidates |
| `decode` | bytes of a plain-text file to text: BOM (UTF-8, UTF-16LE/BE), UTF-16 without BOM (NUL positions), valid or almost-valid UTF-8, else a legacy encoding guessed by `chardetng` and decoded by `encoding_rs`; a confidence and a count of replaced sequences |
| `normalized` | the `normalized_text` artifact (`NormalizedText`): per page the default-normalized text, its `OffsetMap`, every token (`tokens_with_symbols`) with its `FoldOptions::KEY` key, and the source length |

## Stability of stored sketches

Tokens, normalization, keys and hashes feed sketches that are stored and compared across machines and
releases. The crates that decide them (`unicode-segmentation`, `unicode-normalization`, `xxhash-rust`,
`lingua`) are pinned with `=` versions, and `tests/golden.rs` pins the tokens, normalized text, key,
shingles and n-grams of a diverse Unicode sample. **A golden diff means stored sketches change: bump the
version of every analyzer that stores sketches or keys built on these functions** (`name@N+1`), then
regenerate the vectors (`QUAIL_GOLDEN_PRINT=1 cargo test -p quail-text --test golden -- --nocapture`).

## Threads

`LanguageDetector::new` loads the models (Latin and Cyrillic) on the calling thread. quail never calls
Lingua's preload/unload, which run on rayon's global pool, so building and using the detector starts no
threads (measured: 1 thread in the process after construction and detection). The detector is `Clone`
(an `Arc`) and safe to use from many threads.

## Extractor: `text.extract@1`

`TextExtractor` turns `Format::Text` files into a `Document`, so plain text gets the same pipeline as
PDF and DOCX.

- **Encoding** (`decode`): emits `text.encoding` (lowercase WHATWG name, `conf` = confidence),
  `text.encoding_confidence` (1 for a BOM or valid UTF-8; a heuristic in `[0.3, 0.9]` for a legacy
  guess: more non-ASCII characters and a larger share of letters among them raise it),
  `text.encoding_bom`, and `text.decode_errors` + a warning when bytes were replaced by U+FFFD. A
  confidence below 0.6 adds a warning.
- **Line endings**: CRLF and CR become LF before pages and blocks are built, so spans index the stored
  page text; `text.line_endings` = `lf|crlf|cr|mixed|none` (before normalization).
- **Pages**: split on form feed into `Logical` pages (a trailing form feed opens no empty page), else
  one page. `text_source = Native`, `needs_ocr = false`.
- **Blocks**: paragraphs separated by blank lines, trimmed; a page with no blank line but several lines
  gets a block per line. A one-line block of 3-80 characters with 2+ letters, not ending in `.,;`, is a
  `Heading` level 1 when all its letters are upper case, level 2 when it starts with a capital or digit,
  has at most 8 words, no trailing `:` and every word of 4+ letters is capitalized. Zone `Body` (no
  header/footer inference).
- **Limits**: reads at most `max_bytes` (and at most 2 × `max_text_bytes` + 4), keeps at most
  `max_text_bytes` (cut on a char boundary), at most `max_pages` pages: each is a `problem`. Deadline
  checked per page and every 4096 blocks.

## Analyzers

Both require the `document` artifact and run in `Fast`.

- `text.stats@3`: `text.chars`, `text.words`, `text.pages_with_text`, `text.script`
  (`latin|cyrillic|greek|other|mixed`), `text.script.{latin,cyrillic,greek,other}`,
  `text.garbage_ratio`, `text.garbage_pages`. Characters, scripts and garbage are measured on the text
  as extracted, words on the normalized (de-hyphenated) text; a garbage page is one whose own verdict is
  `Garbage` (too-short pages never are).
- `text.lang@2`: `text.lang` (best, `conf` = score), `text.lang_top` (`[[code, score], ...]`, top 3);
  from `Standard` on also `text.page_lang` per page (page evidence). Skips `needs_ocr` pages.

## Shared normalization: `normalized_text`

`text.stats` normalizes every page anyway, so it provides what it built as the `NormalizedText`
artifact (`provides = ["normalized_text"]`), from `Ctx::document()` (OCR text when OCR ran). Per
page: the text normalized with `NormalizeOptions::default()`, its `OffsetMap` (both directions),
all tokens with spans, kinds and `FoldOptions::KEY` keys, and the length of the page text it came
from. Pages cut by `Limits::max_text_bytes` (and the ones after) are absent.

Consumers declare `after(&["normalized_text"])` (soft) and call `NormalizedText::page(pos, &page.text)`
only when they would normalize that whole page with the defaults themselves; otherwise (no
`text.stats`, page cut by their own budget, other options) they run their own pass. The result is the
same either way, so no output depends on whether `text.stats` is active. `text.lang` uses it for the
pages it reads whole.

Built eagerly (tokens and keys included): `text.stats` tokenized every page already to count words,
so the extra cost is folding the keys, +10–25 % of `text.stats` time (one fold per token instead of one
per token in each consumer). Heap: 16 bytes per token, the normalized text, the keys that differ from
their token, 16 bytes per offset-map checkpoint: median 4.4–4.5 bytes per byte of page text, 63 KB per
file on average on 363 private PDFs/DOCX (max 1.0 MB), 259 KB on CUAD's 510 contracts (max 1.6 MB).
It lives until the file is done.

## Limits and cost

- `text.stats` reads at most `Limits::max_text_bytes` (records a problem when it stops), checks the deadline
  per page.
- `text.lang` samples 3 000 characters for the model (start, middle, end) and scans at most 20 000
  for sr/hr/bs markers: about 1 ms per document. The detector holds about 60 MB: the models
  (process-wide, read-only, file-backed pages of the binary, ~54 MB once touched) plus a 7 MB
  table of their 1- to 3-grams that the vendored Lingua builds at construction (about 0.1 s,
  once per process; see `vendor/README.md`). Build it once and share it.

## Measured

Language identification, 13 test sets (sr Latin, sr Cyrillic, hr, bs, en, de, fr, it, es; author-written
sentences, 14 per set + 8 per set for sr/hr/bs in a second register), accuracy of the best candidate:

| backend | short (3 words) | sentence | 5 sentences | RSS | speed |
|---|---|---|---|---|---|
| whatlang (allowlist) | 64 % | 72 % | 72 % | 4 MB | 20 MB/s |
| Lingua low accuracy, 11 langs¹ | 66 % | 83 % | 87 % | 54 MB | 0.9 MB/s |
| Lingua high accuracy, 11 langs | 72 % | 80 % | 82 % | 56 MB | 1.9 MB/s |
| **quail (Lingua high + sr/hr/bs layer)** | **74 %** | **93 %** | **99.4 %** | 55 MB | 1.8 MB/s |

¹ measured before the second sr/hr/bs set was added. RSS is the process high-water mark;
speed is on 2 425 corpus pages (≤ 2 000 characters each).

Neither library knows Latin-script Serbian (0 % for both); the marker layer brings it to 100 % on
5-sentence texts. The sr/hr/bs sets are small and the second set was used while choosing markers, so
treat the cluster numbers as optimistic.

Garbage ratio on the text layers of 369 private business PDFs (pdftotext, 2 582 pages with ≥ 20
characters): clean pages max 0.039 (p99 0.017), 0 pages above 0.10. Broken layers simulated from the same
pages, detected at 0.10: private-use glyphs 100 %, control glyphs 100 %, letter substitution 97 %, glyph
ids shifted by 29 93 %, Latin-1-shifted 95 % (misses are pages with < 100 letters).

Look-alike class (Cyrillic shown as Latin), on the text layers quail-pdf extracts (pages with ≥ 20
characters):

| corpus | pages | look-alike pages | `Garbage` pages before → after |
|---|---:|---:|---:|
| serbian-public (332 PDFs) | 2 937 | 217 (28 files) | 177 → 242 |
| CUAD (510 PDFs) | 9 286 | 0 | 2 → 0 |
| private (369 PDFs) | 2 582 | 0 | 0 → 0 |

Of the 20 serbian-public silver-gold documents whose notes say the layer is garbled, 15 are found;
the 5 misses are a Latin layer turned Cyrillic look-alike (the other direction), two low-quality Latin
OCR layers and a garbled stamp or field labels on an otherwise clean page. On clean pages the suspect
share stays at or below 0.10 (0.08 with four or more suspects); look-alike pages sit at 0.3–0.6.
The two CUAD pages that were `Garbage` before (a camel-case brand line, a part-number table) are clean
now: camel case is not case chaos, and chunks with digits are left out of the vowel share. The highest
non-look-alike ratio on CUAD is 0.058, on the private corpus 0.039.

Throughput (`cargo bench -p quail-text --bench throughput`, thin LTO, shared 16-core laptop, MiB/s on
256 KiB of generated text, Serbian Latin / Serbian Cyrillic / English):

| function | sr-Latn | sr-Cyrl | en |
|---|---|---|---|
| `normalize` (Cow) | 68 | 18–22 | 77 |
| `normalize_into` | 87 | 19–23 | 105 |
| `fold` (KEY) | 416 | 93–120 | 671 |
| `tokens` | 57 | 21–25 | 55 |
| `text_shingles` k=5 | 42 | 16–17 | 39 |
| `char_ngrams` n=5 | 134 | 100–115 | 122 |
| `ScriptCounts` | 837 | 502 | 1 439 |
| `garbage_report` | 91 | 20 | 119 |

Cyrillic is slower per byte because every character takes the Unicode-table path (two bytes per
letter, no ASCII fast path). Language detection: 0.8–1.4 ms per 256 KiB document (mostly sampling
and the marker scan), 0.08–0.15 ms for a 120-character snippet. Over CUAD's 510 contracts
(`Standard`: per document and per page), the analyzer takes 2.3 s instead of 9.0 s with upstream
Lingua 1.8.0.

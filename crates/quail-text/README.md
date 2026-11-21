# quail-text

Text utilities every text analyzer builds on, plus two text-level analyzers. Namespace `text.*`.

## Utilities

| module | what |
|---|---|
| `normalize` | NFKC, invisible/control characters removed, quotes and dashes folded, whitespace collapsed, `ugo-\nvor → ugovor` (real hyphens only, never dashes); `Cow` result, `normalize_into` buffer reuse, `normalize_mapped` + `OffsetMap` in both directions (normalized ⇄ source spans) |
| `fold` | matching keys: lowercase, diacritics off (`đ → dj` or `d`), Serbian Cyrillic → Latin; `fold_into` never allocates beyond the caller's buffer |
| `translit` | exact Serbian Cyrillic ⇄ Latin, linear; documented Latin→Cyrillic policy (greedy digraphs + exception list; foreign words and Roman numerals kept Latin, acronyms converted (`PDV → ПДВ`) unless `keep_acronyms`); `cyrillic_to_latin_chars` streams |
| `script` | Latin / Cyrillic / Greek / other letter counts and ratios |
| `token` | UAX #29 tokens with byte spans, linear time; kinds `Word Number Alnum Url Email Punct Symbol`; `114/23`, `2023-05-12`, `1.200,50`, `1 200 000,00`, spaced IBANs, `+381`, `C++`, URLs and e-mail addresses kept whole; `keyed_tokens` adds a folded key per token from one reused buffer |
| `shingle` | word shingles and char n-grams as **stable** XXH3-64 hashes (fixed seed, pinned by tests) |
| `garbage` | `garbage_report` / `garbage_ratio`: share of broken-text-layer characters; `verdict()` → `Clean`, `Garbage` (ratio > `NEEDS_OCR_RATIO` = 0.10) or `TooShort` (< 20 characters, never an OCR reason by itself) |
| `money` | exact-decimal numbers and amounts: `parse_number` with a `NumberFormat` (`MONEY`: two decimals, no sign; `QUANTITY`: four decimals, sign, `1'200`) and a `DecimalHint` for `1,500`; `parse_money` with currency (symbol, word in any case incl. Cyrillic `дин.`, ISO code in capitals), multiplier (`5 million`, `2 miliona`) and sign (`-1,200.00`, `1.200,00-`, `(1,200.00)`); `Decimal` with `Display`; the currency and multiplier tables. Shared by `quail-signals` and `quail-items` |
| `lang` | `LanguageDetector`: Lingua (high accuracy, 11 languages) + an sr/hr/bs marker layer, scored candidates |

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

## Analyzers

Both require the `document` artifact and run in `Fast`.

- `text.stats@1`: `text.chars`, `text.words`, `text.pages_with_text`, `text.script`
  (`latin|cyrillic|greek|other|mixed`), `text.script.{latin,cyrillic,greek,other}`,
  `text.garbage_ratio`, `text.garbage_pages`. Characters, scripts and garbage are measured on the text
  as extracted, words on the normalized (de-hyphenated) text; a garbage page is one whose own verdict is
  `Garbage` (too-short pages never are).
- `text.lang@1`: `text.lang` (best, `conf` = score), `text.lang_top` (`[[code, score], ...]`, top 3);
  from `Standard` on also `text.page_lang` per page (page evidence). Skips `needs_ocr` pages.

## Limits and cost

- `text.stats` reads at most `Limits::max_text_bytes` (records a problem when it stops), checks the deadline
  per page.
- `text.lang` samples 3 000 characters for the model (start, middle, end) and scans at most 20 000
  for sr/hr/bs markers: about 1 ms per document. The detector holds about 50 MB of models
  (process-wide, read-only once loaded; about 28 MB after construction, 55 MB once every language has been
  seen); build it once and share it.

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
letter, no ASCII fast path). Language detection: 1.3–1.8 ms per 256 KiB document, 0.17–0.31 ms for a
120-character snippet.

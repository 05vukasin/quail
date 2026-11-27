# quail-html

HTML extractor for quail: the WHATWG tokenizer of `html5ever` without its tree builder (no
DOM), feeding an element stack of quail's own, so every structure and its bound is ours.
Pure Rust, no `unsafe`. Nothing is ever fetched: no stylesheet, script, frame or image.

- Extractor id: `html.extract@1` (`quail_html::ID`), formats `[Format::Html]` (HTML and XHTML,
  inline XBRL included; `engine.sniff` decides from the first bytes).
- Public API: `HtmlExtractor` (implements `quail_core::Extractor`), `ID`, `keys::*`.

## Document model

- Logical pages split at CSS page breaks (`page-break-before:always`,
  `page-break-after:always`, `break-before:page`, `break-after:page`, in a `style`
  attribute), or one page. SEC EDGAR filings mark their printed pages this way.
- Blocks: block elements (`p`, `div`, `blockquote`, `center`, `dt`, `dd`, ...) are
  `Paragraph`; `h1`-`h6` are `Heading { level }` (zone `Title`); `li` is `ListItem`; table
  cells are `TableCell { table, row, col }` (zone `Table`, `colspan` and `rowspan` respected).
  Blocks inside `<header>` and `<footer>` get zones `Header` and `Footer`. `<br>` is `\n`
  inside a block. Whitespace collapses the way a browser shows it (kept in `<pre>`), and
  `&nbsp;` is a space. Missing end tags are implied as HTML does (`<p>`, `<li>`, `<td>`,
  `<tr>`, ...).
- **Layout tables**: a table with one cell in all, or with a cell holding a heading, a list,
  a page break or another table, is layout, not data. Its cells are read as ordinary blocks
  and the tables inside become tables of their own, so an EDGAR filing wrapped in a
  full-page table keeps its headings and its financial tables. A data cell is one block,
  its paragraphs joined by `\n`; empty (spacer) cells give no block.
- **Left out**: `script`, `style`, `template`, `noscript`, `nav`, `iframe`, `object`,
  `applet`, `svg`, `math`, `select`, `datalist`, `button`, `textarea`, `audio`, `video`,
  `canvas`, `map`, `frameset`; the `<head>` (its `<title>` is the `title` fact); comments;
  **hidden content**: the `hidden` attribute, `display:none` and `visibility:hidden` in a
  `style` attribute, and `ix:header`. Stylesheets are not applied.
- **Inline XBRL**: `ix:` elements (any prefix bound to the inline XBRL namespace) are inline
  content, so the text of `ix:nonFraction` and `ix:nonNumeric` stays where it is. The `dei:`
  cover facts, hidden ones included, become `xbrl_dei` (`DocumentType=10-K`, ...).
- `<x/>` closes itself in XHTML (an XML declaration or the XHTML namespace) and for prefixed
  elements; in HTML it does not, as in a browser.

## Encoding

In order: a BOM; `<meta charset>` or `<meta http-equiv="Content-Type" content="...;
charset=...">` (a simplified WHATWG prescan of the first 4 KiB, comments skipped); the XML
declaration's `encoding`; UTF-8 when the bytes are valid UTF-8; UTF-16 by its zero bytes;
a `chardetng` guess. Labels are WHATWG labels: `iso-8859-1` and `ASCII` decode as
windows-1252, a declared UTF-16 means UTF-8. Invalid bytes become U+FFFD and are counted.

## Keys (`html.*`)

| Keys | |
|---|---|
| `encoding`, `encoding_source`, `decode_errors` | the charset and where it came from (`bom`, `meta`, `xml`, `utf8`, `guess`) |
| `title`, `lang`, `generator` | `<title>`, `<html lang>`, `<meta name="generator">` |
| `inline_xbrl`, `ix_facts`, `xbrl_dei` | inline XBRL: present, number of facts, `dei:` cover facts |
| `tables`, `layout_tables`, `hidden_elements`, `scripts`, `elements` | structure counts |

## Limits

Stopping short is an `Emitter::problem` (the record becomes partial).

| What | Limit |
|---|---|
| input | decoded and tokenized in 64 KiB chunks; the deadline is checked between chunks |
| one token | more than 16 MiB of input without a token completing (an endless comment, tag or attribute value) stops the read |
| text | `max_text_bytes`: every byte of page text is charged, separators included, and cut on a char boundary |
| pages, blocks | `max_pages`; 250 000 blocks |
| nesting | 512 open elements (deeper ones are inline content), 32 nested tables (deeper ones plain blocks) |
| tables | 1024 columns, `colspan` 1000, `rowspan` 65 534 |
| references | the fixed HTML table only: HTML has no DOCTYPE entities, so nothing expands |
| facts | title and generator 4 KiB, 200 `dei:` facts of 1 KiB |

## Development

`cargo test -p quail-html`: every input is built in code, including synthetic files shaped
like EDGAR 10-K (inline XBRL) and 8-K filings (`tests/extract.rs`) and hostile ones
(`tests/hostile.rs`).

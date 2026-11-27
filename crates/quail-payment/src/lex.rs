//! The document as lines of keyed tokens: every line of every page, each token with its byte
//! span in the page text and its folded key (`quail_text::fold`, Cyrillic → Latin, no
//! diacritics, lowercase), so one cue list covers `Predmet ugovora`, `PREDMET UGOVORA` and
//! `Предмет уговора`.

use quail_core::{Document, Span, TextSource};
use quail_text::fold::{FoldOptions, fold_into};
use quail_text::token::{TokenKind, tokens_with_symbols};

/// Tokens read per line; the rest of a longer line is ignored (no business line is that long).
pub const MAX_LINE_TOKENS: usize = 400;

/// One token.
#[derive(Clone, Debug)]
pub struct Tok {
    /// Byte range in the page text.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
    /// What it is.
    pub kind: TokenKind,
    /// Folded key (for punctuation and symbols, the character itself).
    pub key: String,
}

impl Tok {
    /// Whether it is a word, number or code.
    pub fn wordlike(&self) -> bool {
        !matches!(self.kind, TokenKind::Punct | TokenKind::Symbol)
    }
}

/// One line of a page.
#[derive(Clone, Debug)]
pub struct Line {
    /// Position of the page in `Document::pages`.
    pub pos: usize,
    /// The page's index (`Page::index`).
    pub page: u32,
    /// Byte range of the line in the page text.
    pub span: Span,
    /// Its tokens, at most [`MAX_LINE_TOKENS`].
    pub toks: Vec<Tok>,
    /// The page's text came from OCR.
    pub ocr: bool,
}

impl Line {
    /// The line's text.
    pub fn text<'d>(&self, doc: &'d Document) -> &'d str {
        doc.pages
            .get(self.pos)
            .map_or("", |p| self.span.slice(&p.text))
    }

    /// The text between two byte offsets of the page.
    pub fn slice<'d>(&self, doc: &'d Document, start: u32, end: u32) -> &'d str {
        doc.pages
            .get(self.pos)
            .map_or("", |p| Span { start, end }.slice(&p.text))
    }

    /// The text between the token before `i` and token `i` (`" "` for one space).
    pub fn gap_before<'d>(&self, doc: &'d Document, i: usize) -> &'d str {
        if i == 0 || i >= self.toks.len() {
            return "";
        }
        self.slice(doc, self.toks[i - 1].end, self.toks[i].start)
    }
}

/// The lines read, and whether reading stopped early.
pub struct Lines {
    /// Every non-blank line, in document order.
    pub lines: Vec<Line>,
    /// The byte budget ended the read at this page index.
    pub cut_at: Option<u32>,
    /// The deadline ended the read.
    pub stopped: bool,
}

/// Reads at most `budget` bytes of page text into lines, skipping pages that still need OCR
/// (their text is not usable). `stop` is asked once per page.
pub fn read(doc: &Document, budget: usize, max_lines: usize, mut stop: impl FnMut() -> bool) -> Lines {
    let mut out = Lines {
        lines: Vec::new(),
        cut_at: None,
        stopped: false,
    };
    let mut used = 0usize;
    let mut key = String::new();
    'pages: for (pos, page) in doc.pages.iter().enumerate() {
        if stop() {
            out.stopped = true;
            break;
        }
        if page.needs_ocr {
            continue;
        }
        let ocr = page.text_source == TextSource::Ocr;
        let mut offset = 0usize;
        for raw in page.text.split('\n') {
            let start = offset;
            offset += raw.len() + 1;
            if raw.trim().is_empty() {
                continue;
            }
            used += raw.len();
            if used > budget || out.lines.len() >= max_lines {
                out.cut_at = Some(page.index);
                break 'pages;
            }
            let mut toks = Vec::new();
            for t in tokens_with_symbols(raw).take(MAX_LINE_TOKENS) {
                key.clear();
                fold_into(t.text, &FoldOptions::KEY, &mut key);
                toks.push(Tok {
                    start: u32::try_from(start + t.start).unwrap_or(u32::MAX),
                    end: u32::try_from(start + t.end).unwrap_or(u32::MAX),
                    kind: t.kind,
                    key: key.clone(),
                });
            }
            if toks.is_empty() {
                continue;
            }
            out.lines.push(Line {
                pos,
                page: page.index,
                span: Span {
                    start: u32::try_from(start).unwrap_or(u32::MAX),
                    end: u32::try_from(start + raw.len()).unwrap_or(u32::MAX),
                },
                toks,
                ocr,
            });
        }
    }
    out
}

/// Whether `word` (a cue word: a folded key, `*` at the end for a prefix) matches `key`.
pub fn word_matches(word: &str, key: &str) -> bool {
    match word.strip_suffix('*') {
        Some(stem) => key.starts_with(stem),
        None => key == word,
    }
}

/// The token index one past `phrase` when it starts at token `i` of `toks`, skipping
/// punctuation between its words (`PDV-a`, `excl.`). `None` when it does not match.
pub fn match_at(toks: &[Tok], i: usize, phrase: &[&str]) -> Option<usize> {
    let mut j = i;
    for (n, w) in phrase.iter().enumerate() {
        if n > 0 {
            while j < toks.len() && toks[j].kind == TokenKind::Punct && !word_matches(w, &toks[j].key) {
                j += 1;
            }
        }
        let t = toks.get(j)?;
        if !word_matches(w, &t.key) {
            return None;
        }
        j += 1;
    }
    Some(j)
}

/// The first phrase of `table` that starts at token `i`, the longest when several do:
/// `(entry, end)`.
pub fn longest_at<'t, T>(toks: &[Tok], i: usize, table: &'t [(&'t [&'t str], T)]) -> Option<(&'t T, usize)> {
    let mut best: Option<(&T, usize)> = None;
    for (phrase, v) in table {
        if let Some(end) = match_at(toks, i, phrase)
            && best.is_none_or(|(_, e)| end > e)
        {
            best = Some((v, end));
        }
    }
    best
}

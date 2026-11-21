//! One page prepared for scanning: normalized text, its offset map, tokens with folded keys.

use quail_core::Span;
use quail_text::fold::{FoldOptions, fold_into};
use quail_text::normalize::{NormalizeOptions, OffsetMap, Whitespace, normalize_mapped};
use quail_text::token::{TokenKind, tokens_with_symbols};

/// A token of the normalized page text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
    key: (u32, u32),
    /// Line number (count of `\n` before it).
    pub line: u32,
    /// The original text has a blank line (a paragraph break) right before it; the
    /// normalized text keeps only one `\n` of it.
    pub para: bool,
}

/// Normalized text of one page and its tokens. Buffers are reused across pages.
#[derive(Default)]
pub(crate) struct PageText {
    pub norm: String,
    pub map: OffsetMap,
    pub toks: Vec<Tok>,
    keys: String,
}

/// Normalization used before scanning: NFKC (NBSP and other spaces become `' '`,
/// fullwidth digits ASCII), invisible characters out, dashes and quotes folded, lines kept.
/// No de-hyphenation: it would glue `123-\n456`-like codes only after letters anyway, and
/// identifiers should be read as written.
const NORM: NormalizeOptions = NormalizeOptions {
    nfkc: true,
    remove_invisible: true,
    fold_typography: true,
    whitespace: Whitespace::CollapseKeepLines,
    dehyphenate: false,
};

impl PageText {
    /// Prepares `text` (cleared state first).
    pub fn load(&mut self, text: &str) {
        normalize_mapped(text, &NORM, &mut self.norm, &mut self.map);
        self.toks.clear();
        self.keys.clear();
        let mut line = 0u32;
        let mut last = 0usize;
        let norm = &self.norm;
        let mut prev_end = 0usize;
        for t in tokens_with_symbols(norm) {
            let breaks = memchr_count(&norm.as_bytes()[last..t.start]);
            line += u32::try_from(breaks).unwrap_or(0);
            last = t.start;
            let para = breaks > 0 && {
                let a = self.map.to_source(prev_end);
                let b = self.map.to_source(t.start);
                text.get(a..b)
                    .is_some_and(|g| memchr_count(g.as_bytes()) >= 2)
            };
            prev_end = t.end;
            let k0 = self.keys.len();
            fold_into(t.text, &FoldOptions::KEY, &mut self.keys);
            let k1 = self.keys.len();
            self.toks.push(Tok {
                start: t.start,
                end: t.end,
                kind: t.kind,
                key: (to_u32(k0), to_u32(k1)),
                line,
                para,
            });
        }
    }

    pub fn len(&self) -> usize {
        self.toks.len()
    }

    /// Text of token `i` as written (normalized).
    pub fn text(&self, i: usize) -> &str {
        self.toks
            .get(i)
            .and_then(|t| self.norm.get(t.start..t.end))
            .unwrap_or("")
    }

    /// Folded key of token `i`.
    pub fn key(&self, i: usize) -> &str {
        self.toks
            .get(i)
            .and_then(|t| self.keys.get(t.key.0 as usize..t.key.1 as usize))
            .unwrap_or("")
    }

    pub fn kind(&self, i: usize) -> Option<TokenKind> {
        self.toks.get(i).map(|t| t.kind)
    }

    /// Text between token `i - 1` and token `i` (whitespace); `None` for the first token.
    pub fn gap_before(&self, i: usize) -> Option<&str> {
        let prev = self.toks.get(i.checked_sub(1)?)?;
        let cur = self.toks.get(i)?;
        self.norm.get(prev.end..cur.start)
    }

    /// Tokens `i - 1` and `i` touch or are separated by one space, on one line.
    pub fn tight(&self, i: usize) -> bool {
        matches!(self.gap_before(i), Some("" | " "))
    }

    /// Tokens `i - 1` and `i` touch.
    pub fn touch(&self, i: usize) -> bool {
        matches!(self.gap_before(i), Some(""))
    }

    /// Is there a blank line in the original text right before token `i`?
    pub fn paragraph_before(&self, i: usize) -> bool {
        self.toks.get(i).is_some_and(|t| t.para)
    }

    pub fn line(&self, i: usize) -> u32 {
        self.toks.get(i).map_or(0, |t| t.line)
    }

    /// Is token `i` a word, number, code (not punctuation or a symbol)?
    pub fn wordlike(&self, i: usize) -> bool {
        !matches!(
            self.kind(i),
            Some(TokenKind::Punct | TokenKind::Symbol) | None
        )
    }

    /// Normalized span from the start of token `a` to the end of token `b`.
    pub fn span(&self, a: usize, b: usize) -> Span {
        let s = self.toks.get(a).map_or(0, |t| t.start);
        let e = self.toks.get(b).map_or(s, |t| t.end);
        Span {
            start: to_u32(s),
            end: to_u32(e.max(s)),
        }
    }
}

fn memchr_count(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count()
}

pub(crate) fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The byte span `s..e` as a [`Span`].
pub(crate) fn span(s: usize, e: usize) -> Span {
    Span {
        start: to_u32(s),
        end: to_u32(e.max(s)),
    }
}

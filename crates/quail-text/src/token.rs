//! Tokenization with byte spans, on Unicode word boundaries (UAX #29), number-aware.
//!
//! [`tokens`] yields words, numbers and alphanumeric codes; [`tokens_with_symbols`] also
//! yields punctuation and symbols. Every [`Token`] borrows its text and carries its byte
//! span in the input, so a fact found on a token can cite it as
//! [`Evidence`](quail_core::Evidence) directly. Tokenize the page text itself (not a
//! normalized copy) when spans must point into the page, or map them back with
//! [`OffsetMap`](crate::normalize::OffsetMap).
//!
//! # Rules on top of UAX #29
//!
//! UAX #29 already keeps `1.200,50`, `3,14`, `don't`, `RS35` and `foo_bar` whole. quail
//! additionally merges, without whitespace in between:
//!
//! 1. **Number chains**: a token ending in a digit, then one of `/ - : . ,`, then a digit
//!    starts a continuation: `114/23`, `12/05/2023`, `2023-05-12`, `12:30`,
//!    `160-0000000123456-78`, `011/123-456`, `1.200,50`. The continuation runs to the end of
//!    the following alphanumeric run (`114/23a`). A separator followed by a non-digit ends
//!    the token (`12.` + `maj`, `2023-` + `II`).
//! 2. **Spaced IBANs**: two uppercase letters and two digits, then groups of four
//!    uppercase-alphanumerics separated by single spaces (the last group 1–4 long),
//!    15–34 characters without the spaces: `RS35 2600 0560 1001 6113 79`. An unspaced IBAN is
//!    already one UAX #29 token.
//! 3. **Grouped thousands**: 1–3 digits followed by groups of exactly three digits after a
//!    space, merged only when the separators are non-breaking or thin spaces (U+00A0,
//!    U+202F, U+2009), or when the number ends in a decimal part of one or two digits
//!    (`1 200 000,00`). So `1 200,50` is one token but `strana 5 100 primeraka` is not.
//!
//! Token kinds: [`TokenKind::Word`] (letters only), [`TokenKind::Number`] (digits and
//! separators only), [`TokenKind::Alnum`] (letters and digits), [`TokenKind::Symbol`].

use std::iter::Peekable;

use quail_core::Span;
use unicode_segmentation::{UWordBoundIndices, UnicodeSegmentation};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Letters (and marks, apostrophes, connectors) only.
    Word,
    /// Digits, possibly with separators (`1.200,50`, `114/23`).
    Number,
    /// Letters and digits (`RS35...`, `A4`, `114/23a`).
    Alnum,
    /// A punctuation or symbol character (only from [`tokens_with_symbols`]).
    Symbol,
}

/// One token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token<'a> {
    /// The token's text, borrowed from the input.
    pub text: &'a str,
    /// First byte in the input.
    pub start: usize,
    /// One past the last byte in the input.
    pub end: usize,
    /// Its kind.
    pub kind: TokenKind,
}

impl Token<'_> {
    /// The byte span, for [`Evidence`](quail_core::Evidence) (saturates past 4 GiB).
    pub fn span(&self) -> Span {
        Span {
            start: u32::try_from(self.start).unwrap_or(u32::MAX),
            end: u32::try_from(self.end).unwrap_or(u32::MAX),
        }
    }

    /// Whether it is a word, number or code (not a symbol).
    pub fn is_wordlike(&self) -> bool {
        self.kind != TokenKind::Symbol
    }
}

/// Words, numbers and codes of `text`, in order.
///
/// ```
/// use quail_text::token::{tokens, TokenKind};
/// let t: Vec<_> = tokens("Rešenje br. 114/23 od 1.200,50 RSD").map(|t| t.text).collect();
/// assert_eq!(t, ["Rešenje", "br", "114/23", "od", "1.200,50", "RSD"]);
/// ```
pub fn tokens(text: &str) -> Tokens<'_> {
    Tokens::new(text, false)
}

/// Like [`tokens`], plus one [`TokenKind::Symbol`] token per punctuation or symbol
/// character (whitespace is never a token).
pub fn tokens_with_symbols(text: &str) -> Tokens<'_> {
    Tokens::new(text, true)
}

/// Iterator returned by [`tokens`] and [`tokens_with_symbols`].
pub struct Tokens<'a> {
    text: &'a str,
    segs: Peekable<UWordBoundIndices<'a>>,
    symbols: bool,
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str, symbols: bool) -> Self {
        Self {
            text,
            segs: text.split_word_bound_indices().peekable(),
            symbols,
        }
    }

    /// Extends `end` with the rules of the module docs; returns the new end.
    fn extend(&self, start: usize, end: usize) -> usize {
        let bytes = self.text.as_bytes();
        let seg = &self.text[start..end];
        if let Some(e) = self.iban(start, end) {
            return e;
        }
        let mut end = end;
        if seg.bytes().all(|b| b.is_ascii_digit())
            && seg.len() <= 3
            && let Some(e) = self.grouped_thousands(end)
        {
            end = e;
        }
        // Number chains.
        while end > start
            && bytes.get(end - 1).is_some_and(u8::is_ascii_digit)
            && bytes.get(end).is_some_and(|b| b"/-:.,".contains(b))
            && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
        {
            end += 1;
            end += self.text[end..]
                .char_indices()
                .find(|&(_, c)| !c.is_alphanumeric())
                .map_or(self.text.len() - end, |(i, _)| i);
        }
        end
    }

    /// End of a spaced IBAN starting at `start`, if there is one.
    fn iban(&self, start: usize, end: usize) -> Option<usize> {
        let b = self.text.as_bytes();
        let head = b.get(start..end)?;
        let ok_head = head.len() >= 4
            && head[..2].iter().all(u8::is_ascii_uppercase)
            && head[2..4].iter().all(u8::is_ascii_digit)
            && head
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        // A head of 4 is the classic "RS35 ...", a full unspaced IBAN needs no merging.
        if !ok_head || head.len() != 4 || b.get(end) != Some(&b' ') {
            return None;
        }
        let mut count = 4;
        let mut pos = end;
        let mut last_group = 4;
        while b.get(pos) == Some(&b' ') && last_group == 4 {
            let g = b
                .get(pos + 1..)?
                .iter()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                .count();
            if g == 0 || g > 4 || count + g > 34 {
                break;
            }
            count += g;
            pos += 1 + g;
            last_group = g;
        }
        let next_alnum = self.text[pos..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric);
        (count >= 15 && !next_alnum).then_some(pos)
    }

    /// End of `d{1,3}( ddd)+([.,]d{1,2})?` after the first group, when the rule allows it.
    fn grouped_thousands(&self, end: usize) -> Option<usize> {
        let text = self.text;
        let mut pos = end;
        let mut special_sep = true;
        let mut groups = 0;
        loop {
            let rest = &text[pos..];
            let Some(sep) = rest.chars().next() else {
                break;
            };
            let thin = matches!(sep, '\u{A0}' | '\u{202F}' | '\u{2009}');
            if sep != ' ' && !thin {
                break;
            }
            let after = &rest[sep.len_utf8()..];
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            if digits != 3 {
                break;
            }
            special_sep &= thin;
            groups += 1;
            pos += sep.len_utf8() + 3;
        }
        if groups == 0 {
            return None;
        }
        let b = text.as_bytes();
        let dec = match b.get(pos) {
            Some(b'.' | b',') => {
                let d = b
                    .get(pos + 1..)
                    .map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count());
                (1..=2).contains(&d).then_some(pos + 1 + d)
            }
            _ => None,
        };
        match dec {
            Some(e) => Some(e),
            None if special_sep => Some(pos),
            None => None,
        }
    }
}

fn kind_of(s: &str) -> TokenKind {
    let (mut alpha, mut digit) = (false, false);
    for c in s.chars() {
        alpha |= c.is_alphabetic();
        digit |= c.is_numeric();
    }
    match (alpha, digit) {
        (true, true) => TokenKind::Alnum,
        (false, true) => TokenKind::Number,
        (true | false, false) => TokenKind::Word,
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let (start, seg) = self.segs.next()?;
            let wordlike = seg.chars().any(char::is_alphanumeric);
            if !wordlike {
                if self.symbols && !seg.chars().all(char::is_whitespace) {
                    return Some(Token {
                        text: seg,
                        start,
                        end: start + seg.len(),
                        kind: TokenKind::Symbol,
                    });
                }
                continue;
            }
            let seg_end = start + seg.len();
            let mut end = if seg.bytes().any(|b| b.is_ascii_digit()) {
                self.extend(start, seg_end)
            } else {
                seg_end
            };
            // Swallow the segments the merge covered; extend over a straddled one.
            while let Some(&(s, sg)) = self.segs.peek() {
                if s >= end {
                    break;
                }
                end = end.max(s + sg.len());
                self.segs.next();
            }
            let text = &self.text[start..end];
            return Some(Token {
                text,
                start,
                end,
                kind: kind_of(text),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(s: &str) -> Vec<&str> {
        tokens(s).map(|t| t.text).collect()
    }

    #[test]
    fn words_and_spans() {
        let s = "Ugovor o zakupu, Beograd.";
        let t: Vec<_> = tokens(s).collect();
        assert_eq!(t.len(), 4);
        for tok in &t {
            assert_eq!(&s[tok.start..tok.end], tok.text);
            assert_eq!(tok.span().slice(s), tok.text);
            assert_eq!(tok.kind, TokenKind::Word);
        }
        assert_eq!(texts("Уговор о закупу"), ["Уговор", "о", "закупу"]);
    }

    #[test]
    fn number_chains() {
        assert_eq!(
            texts("broj 114/23 od 12.05.2023. godine"),
            ["broj", "114/23", "od", "12.05.2023", "godine"]
        );
        assert_eq!(texts("2023-05-12 u 12:30"), ["2023-05-12", "u", "12:30"]);
        assert_eq!(
            texts("račun 160-0000000123456-78"),
            ["račun", "160-0000000123456-78"]
        );
        assert_eq!(texts("iznos 1.200,50 din"), ["iznos", "1.200,50", "din"]);
        assert_eq!(texts("2023-II"), ["2023", "II"]);
        assert_eq!(texts("114/23a"), ["114/23a"]);
        let k: Vec<_> = tokens("114/23 x 114/23a").map(|t| t.kind).collect();
        assert_eq!(k, [TokenKind::Number, TokenKind::Word, TokenKind::Alnum]);
    }

    #[test]
    fn ibans() {
        assert_eq!(
            texts("IBAN: RS35 2600 0560 1001 6113 79 hvala"),
            ["IBAN", "RS35 2600 0560 1001 6113 79", "hvala"]
        );
        assert_eq!(texts("RS35260005601001611379"), ["RS35260005601001611379"]);
        // Too short to be an IBAN.
        assert_eq!(texts("AB12 3456"), ["AB12", "3456"]);
        assert_eq!(
            texts("DE89 3704 0044 0532 0130 00."),
            ["DE89 3704 0044 0532 0130 00"]
        );
    }

    #[test]
    fn grouped_thousands() {
        assert_eq!(texts("1 200 000,00 RSD"), ["1 200 000,00", "RSD"]);
        assert_eq!(texts("1\u{a0}200 EUR"), ["1\u{a0}200", "EUR"]);
        assert_eq!(
            texts("strana 5 100 primeraka"),
            ["strana", "5", "100", "primeraka"]
        );
    }

    #[test]
    fn symbols() {
        let t: Vec<_> = tokens_with_symbols("Član 5. (§ 3) — 20%")
            .map(|t| (t.text, t.kind))
            .collect();
        assert!(t.contains(&("§", TokenKind::Symbol)));
        assert!(t.contains(&("%", TokenKind::Symbol)));
        assert!(t.contains(&("Član", TokenKind::Word)));
    }

    #[test]
    fn hostile() {
        let s: String = "1/".repeat(1000);
        assert_eq!(tokens(&s).count(), 1);
        let s = "RS35 ".repeat(50);
        let _ = tokens(&s).count();
        let _ = tokens("9\u{a0}").count();
        let _ = tokens("1,").count();
        let _ = tokens("").count();
    }
}

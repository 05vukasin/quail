//! Tokenization with byte spans, on Unicode word boundaries (UAX #29), number-aware.
//!
//! [`tokens`] yields words, numbers, codes, URLs and e-mail addresses;
//! [`tokens_with_symbols`] also yields punctuation and symbols. Every [`Token`] borrows its
//! text and carries its byte span in the input, so a fact found on a token can cite it as
//! [`Evidence`](quail_core::Evidence) directly. Tokenize the page text itself (not a
//! normalized copy) when spans must point into the page, or map them back with
//! [`OffsetMap`](crate::normalize::OffsetMap). [`keyed_tokens`] adds a folded matching key
//! per token, written into one reused buffer.
//!
//! Every rule below looks ahead a bounded distance, so tokenizing is linear in the input.
//!
//! # Rules on top of UAX #29
//!
//! UAX #29 already keeps `1.200,50`, `3,14`, `don't`, `U.S.A`, `RS35` and `foo_bar` whole.
//! quail additionally merges, without whitespace in between:
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
//! 3. **Grouped thousands**: 1–3 digits followed by 1–6 groups of exactly three digits after
//!    a space, merged only when the separators are non-breaking or thin spaces (U+00A0,
//!    U+202F, U+2009), or when the number ends in a decimal part of one or two digits
//!    (`1 200 000,00`). So `1 200,50` is one token but `strana 5 100 primeraka` is not, and a
//!    longer run of groups is a table of numbers, not one number.
//! 4. **Signed international numbers**: a `+` directly before a digit run that does not
//!    follow a letter or digit joins it: `+381`. The rest of a phone number stays separate
//!    tokens (`+381`, `11`, `123-4567`): spaced digit groups are too ambiguous to merge.
//! 5. **URLs**: `http://`, `https://`, `ftp://` or `www.` up to the next whitespace (at most
//!    2 048 bytes), without trailing `.,;:!?)]}>"'`: one [`TokenKind::Url`].
//! 6. **E-mail addresses**: a word directly followed by `@` and a domain with a dot
//!    (`ime.prezime@firma.rs`): one [`TokenKind::Email`]. A local part with `-` or `+` keeps
//!    only its last word.
//! 7. **Language names**: `C++`, `C#`, `F#`: a short word followed by `++` or `#` and no
//!    letter or digit.
//!
//! Superscript and fullwidth digits are numbers as UAX #29 sees them (`x²` gives `x` and
//! `²`; `５０` is a number); fold or normalize first to get ASCII digits.

use std::iter::Peekable;

use quail_core::Span;
use unicode_segmentation::{UWordBoundIndices, UnicodeSegmentation};

use crate::fold::{FoldOptions, fold_into};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Letters (and marks, apostrophes, connectors) only.
    Word,
    /// Digits, possibly with separators and a leading `+` (`1.200,50`, `114/23`, `+381`).
    Number,
    /// Letters and digits (`RS35...`, `A4`, `114/23a`).
    Alnum,
    /// A URL (`https://...`, `www....`).
    Url,
    /// An e-mail address.
    Email,
    /// A punctuation character (only from [`tokens_with_symbols`]).
    Punct,
    /// Any other non-letter, non-digit character: currency, math, emoji (only from
    /// [`tokens_with_symbols`]).
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

    /// Whether it is a word, number, code, URL or address (not punctuation or a symbol).
    pub fn is_wordlike(&self) -> bool {
        !matches!(self.kind, TokenKind::Punct | TokenKind::Symbol)
    }
}

/// Words, numbers, codes, URLs and e-mail addresses of `text`, in order.
///
/// ```
/// use quail_text::token::{tokens, TokenKind};
/// let t: Vec<_> = tokens("Rešenje br. 114/23 od 1.200,50 RSD").map(|t| t.text).collect();
/// assert_eq!(t, ["Rešenje", "br", "114/23", "od", "1.200,50", "RSD"]);
/// ```
pub fn tokens(text: &str) -> Tokens<'_> {
    Tokens::new(text, false)
}

/// Like [`tokens`], plus one [`TokenKind::Punct`] or [`TokenKind::Symbol`] token per
/// punctuation or symbol segment (whitespace is never a token).
pub fn tokens_with_symbols(text: &str) -> Tokens<'_> {
    Tokens::new(text, true)
}

/// Iterator returned by [`tokens`] and [`tokens_with_symbols`].
pub struct Tokens<'a> {
    text: &'a str,
    segs: Peekable<UWordBoundIndices<'a>>,
    symbols: bool,
}

/// Longest URL kept as one token.
const MAX_URL: usize = 2_048;
/// Longest e-mail domain scanned.
const MAX_DOMAIN: usize = 255;
/// Most digit groups in one grouped number (10^21).
const MAX_GROUPS: usize = 6;

/// Invisible format characters: never a token on their own.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
}

fn is_punct(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_punctuation()
            && !matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~');
    }
    matches!(c,
        '\u{A1}' | '\u{A7}' | '\u{AB}' | '\u{B6}' | '\u{B7}' | '\u{BB}' | '\u{BF}'
        | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205E}' | '\u{3001}'..='\u{3003}'
        | '\u{300C}'..='\u{300F}')
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str, symbols: bool) -> Self {
        Self {
            text,
            segs: text.split_word_bound_indices().peekable(),
            symbols,
        }
    }

    /// Kind and end of a special token starting at word segment `[start, end)`, if any.
    fn special(&self, start: usize, end: usize) -> Option<(TokenKind, usize)> {
        let b = self.text.as_bytes();
        let seg = &self.text[start..end];
        // URL.
        let lower_is = |s: &str| seg.eq_ignore_ascii_case(s);
        let scheme = (lower_is("http") || lower_is("https") || lower_is("ftp"))
            && b.get(end..end + 3) == Some(b"://");
        let www = lower_is("www") && b.get(end) == Some(&b'.');
        if scheme || www {
            let limit = (start + MAX_URL).min(b.len());
            let mut e = b[start..limit]
                .iter()
                .position(u8::is_ascii_whitespace)
                .map_or(limit, |p| start + p);
            while e > end && b"., ;:!?)]}>\"'".contains(&b[e - 1]) {
                e -= 1;
            }
            while !self.text.is_char_boundary(e) {
                e -= 1;
            }
            return Some((TokenKind::Url, e));
        }
        // E-mail: word '@' domain-with-dot.
        if b.get(end) == Some(&b'@') && b.get(end + 1).is_some_and(u8::is_ascii_alphanumeric) {
            let d0 = end + 1;
            let limit = (d0 + MAX_DOMAIN).min(b.len());
            let mut e = d0
                + b[d0..limit]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == b'.' || **c == b'-')
                    .count();
            while e > d0 && matches!(b[e - 1], b'.' | b'-') {
                e -= 1;
            }
            let domain = &b[d0..e];
            let dotted = domain
                .iter()
                .position(|&c| c == b'.')
                .is_some_and(|p| p > 0 && p + 1 < domain.len());
            if dotted {
                return Some((TokenKind::Email, e));
            }
        }
        // C++, C#, F#.
        if seg.len() <= 2 && seg.bytes().all(|c| c.is_ascii_alphabetic()) {
            let tail = if b.get(end..end + 2) == Some(b"++") {
                2
            } else {
                usize::from(b.get(end) == Some(&b'#'))
            };
            if tail > 0 {
                let next = self.text[end + tail..].chars().next();
                if !next.is_some_and(char::is_alphanumeric) {
                    return Some((TokenKind::Word, end + tail));
                }
            }
        }
        None
    }

    /// Extends a numeric segment with the rules of the module docs; returns the new end.
    fn extend_number(&self, start: usize, end: usize) -> usize {
        let bytes = self.text.as_bytes();
        let seg = &self.text[start..end];
        if let Some(e) = self.iban(start, end) {
            return e;
        }
        let mut end = end;
        // A group inside a longer run (" 200" after "1") never starts a grouped number:
        // the run was judged from its first group.
        let in_run = self.text[..start].chars().next_back().is_some_and(|c| {
            matches!(c, ' ' | '\u{A0}' | '\u{202F}' | '\u{2009}')
                && bytes
                    .get(start.saturating_sub(c.len_utf8() + 1))
                    .is_some_and(u8::is_ascii_digit)
        });
        if !in_run
            && seg.bytes().all(|b| b.is_ascii_digit())
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
        let ok_head = head.len() == 4
            && head[..2].iter().all(u8::is_ascii_uppercase)
            && head[2..].iter().all(u8::is_ascii_digit);
        if !ok_head || b.get(end) != Some(&b' ') {
            return None;
        }
        let mut count = 4;
        let mut pos = end;
        let mut last_group = 4;
        while b.get(pos) == Some(&b' ') && last_group == 4 {
            let g = b
                .get(pos + 1..)?
                .iter()
                .take(5)
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

    /// End of `d{1,3}( ddd){1,6}([.,]d{1,2})?` after the first group, when the rule allows.
    fn grouped_thousands(&self, end: usize) -> Option<usize> {
        let text = self.text;
        let mut pos = end;
        let mut special_sep = true;
        let mut groups = 0;
        // One group more than allowed, to recognise (and reject) a table of numbers.
        while groups <= MAX_GROUPS {
            let rest = &text[pos..];
            let Some(sep) = rest.chars().next() else {
                break;
            };
            let thin = matches!(sep, '\u{A0}' | '\u{202F}' | '\u{2009}');
            if sep != ' ' && !thin {
                break;
            }
            let after = &rest.as_bytes()[sep.len_utf8()..];
            let digits = after
                .iter()
                .take(4)
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits != 3 {
                break;
            }
            special_sep &= thin;
            groups += 1;
            pos += sep.len_utf8() + 3;
        }
        if groups == 0 || groups > MAX_GROUPS {
            return None;
        }
        let b = text.as_bytes();
        let dec = match b.get(pos) {
            Some(b'.' | b',') => {
                let d = b.get(pos + 1..).map_or(0, |r| {
                    r.iter().take(3).take_while(|c| c.is_ascii_digit()).count()
                });
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

    /// Swallows the segments a merged token covers; returns the final end.
    fn swallow(&mut self, mut end: usize) -> usize {
        while let Some(&(s, sg)) = self.segs.peek() {
            if s >= end {
                break;
            }
            end = end.max(s + sg.len());
            self.segs.next();
        }
        end
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
            let seg_end = start + seg.len();
            let wordlike = seg.chars().any(char::is_alphanumeric);
            if !wordlike {
                // "+381": a plus sign glued to a digit run that does not follow a word.
                if seg == "+"
                    && self.segs.peek().is_some_and(|&(s, n)| {
                        s == seg_end && n.bytes().next().is_some_and(|b| b.is_ascii_digit())
                    })
                    && !self.text[..start]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric)
                {
                    let (s, n) = self.segs.next()?;
                    let end = self.extend_number(s, s + n.len());
                    let end = self.swallow(end);
                    let text = &self.text[start..end];
                    return Some(Token {
                        text,
                        start,
                        end,
                        kind: kind_of(text),
                    });
                }
                if self.symbols && !seg.chars().all(|c| c.is_whitespace() || is_invisible(c)) {
                    let kind = if seg.chars().all(is_punct) {
                        TokenKind::Punct
                    } else {
                        TokenKind::Symbol
                    };
                    return Some(Token {
                        text: seg,
                        start,
                        end: seg_end,
                        kind,
                    });
                }
                continue;
            }
            let (kind, end) = if let Some((k, e)) = self.special(start, seg_end) {
                (Some(k), e)
            } else if seg.bytes().any(|b| b.is_ascii_digit()) {
                (None, self.extend_number(start, seg_end))
            } else {
                (None, seg_end)
            };
            let end = self.swallow(end);
            let text = &self.text[start..end];
            return Some(Token {
                text,
                start,
                end,
                kind: kind.unwrap_or_else(|| kind_of(text)),
            });
        }
    }
}

/// A token with its folded matching key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyedToken<'a, 'k> {
    /// The token (original text, span, kind).
    pub token: Token<'a>,
    /// The token folded with the iterator's [`FoldOptions`]; valid until the next call.
    pub key: &'k str,
}

/// Word-like tokens with a folded key each, see [`keyed_tokens`].
///
/// Not an [`Iterator`]: the key borrows one buffer that is reused for every token, so take
/// tokens with [`KeyedTokens::next_keyed`] (`while let Some(t) = it.next_keyed() { ... }`)
/// or [`KeyedTokens::for_each_keyed`]. Nothing is allocated per token, for Latin or
/// Cyrillic text.
pub struct KeyedTokens<'a> {
    tokens: Tokens<'a>,
    opts: FoldOptions,
    buf: String,
}

impl<'a> KeyedTokens<'a> {
    /// The next token and its key.
    pub fn next_keyed(&mut self) -> Option<KeyedToken<'a, '_>> {
        let token = self.tokens.next()?;
        self.buf.clear();
        fold_into(token.text, &self.opts, &mut self.buf);
        Some(KeyedToken {
            token,
            key: &self.buf,
        })
    }

    /// Calls `f` for every remaining token.
    pub fn for_each_keyed(mut self, mut f: impl FnMut(KeyedToken<'a, '_>)) {
        while let Some(t) = self.next_keyed() {
            f(t);
        }
    }
}

/// Word-like tokens of `text` (as [`tokens`]) with keys folded by `opts`.
///
/// ```
/// use quail_text::fold::FoldOptions;
/// use quail_text::token::keyed_tokens;
/// let mut it = keyed_tokens("Ђорђе ČAČAK", &FoldOptions::KEY);
/// let mut keys = Vec::new();
/// while let Some(t) = it.next_keyed() {
///     keys.push(t.key.to_owned());
/// }
/// assert_eq!(keys, ["djordje", "cacak"]);
/// ```
pub fn keyed_tokens<'a>(text: &'a str, opts: &FoldOptions) -> KeyedTokens<'a> {
    KeyedTokens {
        tokens: tokens(text),
        opts: *opts,
        buf: String::with_capacity(64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn texts(s: &str) -> Vec<&str> {
        tokens(s).map(|t| t.text).collect()
    }

    fn kinds(s: &str) -> Vec<(&str, TokenKind)> {
        tokens_with_symbols(s).map(|t| (t.text, t.kind)).collect()
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
            texts("1\u{202f}000\u{202f}000"),
            ["1\u{202f}000\u{202f}000"]
        );
        assert_eq!(
            texts("strana 5 100 primeraka"),
            ["strana", "5", "100", "primeraka"]
        );
        // A run of more than six groups is a table, not a number.
        let table = format!("1{},50", " 200".repeat(7));
        assert_eq!(tokens(&table).count(), 8);
    }

    #[test]
    fn urls_emails_phones_and_languages() {
        assert_eq!(
            texts("Vidi https://firma.rs/uslovi?x=1, ili www.firma.rs."),
            ["Vidi", "https://firma.rs/uslovi?x=1", "ili", "www.firma.rs"]
        );
        let k: Vec<_> = tokens("pišite na ime.prezime@firma.co.rs!")
            .map(|t| (t.text, t.kind))
            .collect();
        assert_eq!(
            k.last(),
            Some(&("ime.prezime@firma.co.rs", TokenKind::Email))
        );
        assert_eq!(texts("a@b nije adresa"), ["a", "b", "nije", "adresa"]);
        assert_eq!(
            texts("Tel: +381 11 123-4567"),
            ["Tel", "+381", "11", "123-4567"]
        );
        assert_eq!(texts("A+1"), ["A", "1"]);
        assert_eq!(
            texts("C++ i C# i F#, ali C+"),
            ["C++", "i", "C#", "i", "F#", "ali", "C"]
        );
        assert_eq!(
            texts("e-mail don't U.S.A."),
            ["e", "mail", "don't", "U.S.A"]
        );
    }

    #[test]
    fn punct_and_symbols() {
        let t = kinds("Član 5. (§ 3) — 20% € 5 + 2 🙂");
        for (s, k) in [
            (".", TokenKind::Punct),
            ("(", TokenKind::Punct),
            ("§", TokenKind::Punct),
            ("—", TokenKind::Punct),
            ("%", TokenKind::Punct),
            ("€", TokenKind::Symbol),
            ("+", TokenKind::Symbol),
            ("🙂", TokenKind::Symbol),
            ("Član", TokenKind::Word),
        ] {
            assert!(t.contains(&(s, k)), "{s}: {t:?}");
        }
        assert_eq!(texts("x² ５０ ٣٤٥"), ["x", "²", "５０", "٣٤٥"]);
    }

    #[test]
    fn keyed() {
        let mut it = keyed_tokens("Ђорђевић, ŠUMA 114/23", &FoldOptions::KEY);
        let mut out = Vec::new();
        while let Some(t) = it.next_keyed() {
            out.push((t.token.text.to_owned(), t.key.to_owned()));
        }
        assert_eq!(
            out,
            [
                ("Ђорђевић".to_owned(), "djordjevic".to_owned()),
                ("ŠUMA".to_owned(), "suma".to_owned()),
                ("114/23".to_owned(), "114/23".to_owned()),
            ]
        );
    }

    /// Time of tokenizing `s`.
    fn time(s: &str) -> Duration {
        let t = Instant::now();
        let n = tokens_with_symbols(s).count();
        assert!(n > 0);
        t.elapsed()
    }

    #[test]
    fn linear_time_on_adversarial_input() {
        // Each case: 1 MB should take about 4x the time of 256 KB, never 16x.
        let cases: [(&str, &str); 6] = [
            ("1", " 200"),
            ("", "1/"),
            ("", "RS35 "),
            ("", "(cid:"),
            ("", "a."),
            ("www.", "x"),
        ];
        for (head, unit) in cases {
            let small = format!("{head}{}", unit.repeat((256 << 10) / unit.len()));
            let big = format!("{head}{}", unit.repeat((1 << 20) / unit.len()));
            let (a, b) = (time(&small), time(&big));
            assert!(
                b < a * 10 + Duration::from_millis(50),
                "{unit:?}: 256K {a:?}, 1M {b:?}"
            );
        }
    }

    #[test]
    fn hostile() {
        let s: String = "1/".repeat(1000);
        assert_eq!(tokens(&s).count(), 1);
        for s in [
            "9\u{a0}", "1,", "", "+", "+1", "http://", "a@", "a@b.", "C+", "www.", "ŠŠ#",
        ] {
            let _ = tokens_with_symbols(s).count();
        }
    }
}

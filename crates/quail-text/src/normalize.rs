//! Text normalization for analysis: Unicode NFKC, invisible and control characters removed,
//! typographic variants folded, whitespace collapsed, words split across line breaks joined.
//!
//! The steps, in order, each switchable in [`NormalizeOptions`]:
//!
//! 1. **NFKC** compatibility normalization: ligatures (`ﬁ → fi`), fullwidth and superscript
//!    forms, Serbo-Croatian digraph letters (`ǉ → lj`), NBSP and other spaces (`→ ' '`),
//!    `… → ...`. It runs per *cluster* (a character plus the combining marks or composing
//!    characters that follow it), which equals whole-string NFKC for every script quail
//!    targets, keeps memory flat and lets the output be mapped back to the input.
//! 2. **Invisible characters** removed: C0/C1 controls other than whitespace, zero-width
//!    space/joiners, word joiner, BOM, bidi controls. The soft hyphen (U+00AD) is dropped,
//!    except that at a line end it marks a hyphenated word (step 5).
//! 3. **Typography** folded to ASCII: single quotes and primes `‘ ’ ‚ ‛ ′ → '`, double quotes
//!    and guillemets `“ ” „ ‟ ″ « » → "`, hyphens, dashes and minus `‐ ‑ ‒ – — ― − → -`.
//! 4. **Whitespace**: see [`Whitespace`]. With collapsing, leading and trailing whitespace
//!    is trimmed as well.
//! 5. **De-hyphenation**: a hyphen after a letter, followed by whitespace that contains a
//!    line break and then a *lowercase* letter, is removed with the whitespace:
//!    `"ugo-\nvor" → "ugovor"`. An uppercase letter after the break keeps the hyphen
//!    (`"Novi-\nSad"` stays two words), as does a hyphen without a line break (`crno-beli`).
//!
//! [`normalize`] borrows when the text is already normal (the common case for clean text
//! layers), [`normalize_into`] reuses a caller's buffer, and [`normalize_mapped`] also
//! records an [`OffsetMap`] so a span found in normalized text can be mapped back to the
//! original page text for [`Evidence`](quail_core::Evidence).

use std::borrow::Cow;

use quail_core::Span;
use unicode_normalization::char::{canonical_combining_class, compose};
use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfkc_quick};

/// How [`normalize`] treats whitespace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Whitespace {
    /// Leave every whitespace character as it is.
    Keep,
    /// A run of whitespace becomes one `'\n'` if it contains a line break (`\n`, `\r`, VT,
    /// FF, NEL, U+2028, U+2029), else one `' '`. Keeps the line structure. The default.
    #[default]
    CollapseKeepLines,
    /// Every run of whitespace becomes one `' '`.
    Collapse,
}

/// Options of [`normalize`]. `Default` turns everything on.
#[allow(clippy::struct_excessive_bools)] // independent switches, one per step
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalizeOptions {
    /// Apply NFKC (step 1).
    pub nfkc: bool,
    /// Remove control and invisible characters (step 2).
    pub remove_invisible: bool,
    /// Fold quotes and dashes to ASCII (step 3).
    pub fold_typography: bool,
    /// Whitespace handling (step 4).
    pub whitespace: Whitespace,
    /// Join words hyphenated across a line break (step 5).
    pub dehyphenate: bool,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            nfkc: true,
            remove_invisible: true,
            fold_typography: true,
            whitespace: Whitespace::CollapseKeepLines,
            dehyphenate: true,
        }
    }
}

/// Normalizes `text`. Returns the input (or a prefix of it, when only trailing whitespace
/// went away) without allocating when nothing else changes.
///
/// ```
/// use quail_text::normalize::{normalize, NormalizeOptions};
/// let o = NormalizeOptions::default();
/// assert_eq!(normalize("Ugo-\nvor  o  „zakupu“\u{200b} — ﬁnal", &o), "Ugovor o \"zakupu\" - final");
/// ```
pub fn normalize<'a>(text: &'a str, opts: &NormalizeOptions) -> Cow<'a, str> {
    let mut sink = CowSink {
        src: text,
        len: 0,
        owned: None,
    };
    run(text, *opts, &mut sink);
    match sink.owned {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed(&text[..sink.len]),
    }
}

/// Normalizes `text`, appending to `out`. Reuse `out` across pages to avoid allocations.
pub fn normalize_into(text: &str, opts: &NormalizeOptions, out: &mut String) {
    out.reserve(text.len());
    run(text, *opts, &mut StringSink(out));
}

/// Normalizes `text` into `out` (cleared first) and records in `map` where each output byte
/// came from, so spans in `out` can be mapped back with [`OffsetMap::span_to_source`].
pub fn normalize_mapped(
    text: &str,
    opts: &NormalizeOptions,
    out: &mut String,
    map: &mut OffsetMap,
) {
    out.clear();
    out.reserve(text.len());
    map.clear();
    let mut sink = MapSink { out, map };
    run(text, *opts, &mut sink);
}

/// One place where output and source stop running in lockstep: output char at `out`
/// (`out_len` bytes) came from source bytes `src..src_end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    out: usize,
    out_len: usize,
    src: usize,
    src_end: usize,
}

/// Sparse map from byte offsets in normalized text back to the source text.
///
/// Between checkpoints, output and source advance byte for byte; a checkpoint is stored
/// only where that stops (a removed character, a ligature expanded, whitespace collapsed),
/// so unchanged text costs nothing. A span that starts or ends inside an expansion widens
/// to the whole source character (`fi` from `ﬁ` maps to `ﬁ`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OffsetMap {
    points: Vec<Point>,
    /// Bytes written so far and source end of the last output char.
    out_len: usize,
    src_end: usize,
}

impl OffsetMap {
    /// Empty map.
    pub fn new() -> Self {
        Self::default()
    }

    fn clear(&mut self) {
        self.points.clear();
        self.out_len = 0;
        self.src_end = 0;
    }

    #[inline]
    fn record(&mut self, out_len: usize, src: usize, src_end: usize) {
        let lockstep = src == self.src_end && src_end - src == out_len;
        if !lockstep {
            self.points.push(Point {
                out: self.out_len,
                out_len,
                src,
                src_end,
            });
        }
        self.out_len += out_len;
        self.src_end = src_end;
    }

    /// Source offset for an output offset, as a start (`end == false`) or exclusive end.
    fn map(&self, x: usize, end: bool) -> usize {
        let x = x.min(self.out_len);
        // Last point that covers or precedes x (for an end: precedes x - 1).
        let key = if end { x.saturating_sub(1) } else { x };
        let i = self.points.partition_point(|p| p.out <= key);
        let Some(p) = i.checked_sub(1).and_then(|i| self.points.get(i)) else {
            return x;
        };
        if end && x == 0 {
            return 0;
        }
        let inside = if end {
            x <= p.out + p.out_len
        } else {
            x < p.out + p.out_len
        };
        if inside {
            if end { p.src_end } else { p.src }
        } else {
            p.src_end + (x - p.out - p.out_len)
        }
    }

    /// Source byte offset of the output byte offset `out` (as the start of a range).
    pub fn to_source(&self, out: usize) -> usize {
        self.map(out, false)
    }

    /// Maps a span of the normalized text back to the source text.
    pub fn span_to_source(&self, span: Span) -> Span {
        let start = self.map(span.start as usize, false);
        let end = self.map(span.end as usize, true).max(start);
        Span {
            start: u32::try_from(start).unwrap_or(u32::MAX),
            end: u32::try_from(end).unwrap_or(u32::MAX),
        }
    }
}

/// Source range `[start, end)` of the cluster an output character came from.
type Src = (usize, usize);

trait Sink {
    fn push(&mut self, c: char, src: Src);
}

struct StringSink<'a>(&'a mut String);

impl Sink for StringSink<'_> {
    #[inline]
    fn push(&mut self, c: char, _src: Src) {
        self.0.push(c);
    }
}

struct MapSink<'a> {
    out: &'a mut String,
    map: &'a mut OffsetMap,
}

impl Sink for MapSink<'_> {
    #[inline]
    fn push(&mut self, c: char, src: Src) {
        self.map.record(c.len_utf8(), src.0, src.1);
        self.out.push(c);
    }
}

/// Stays borrowed while the output equals a prefix of the input.
struct CowSink<'a> {
    src: &'a str,
    len: usize,
    owned: Option<String>,
}

impl Sink for CowSink<'_> {
    #[inline]
    fn push(&mut self, c: char, _src: Src) {
        if let Some(s) = &mut self.owned {
            s.push(c);
            return;
        }
        let rest = self.src.as_bytes().get(self.len..).unwrap_or_default();
        let mut buf = [0u8; 4];
        let enc = c.encode_utf8(&mut buf).as_bytes();
        if rest.starts_with(enc) {
            self.len += enc.len();
        } else {
            let mut s = String::with_capacity(self.src.len());
            s.push_str(&self.src[..self.len]);
            s.push(c);
            self.owned = Some(s);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WsKind {
    None,
    Space,
    Line,
}

/// Steps 2–5 on the NFKC output.
struct Pipeline<'s, S: Sink> {
    opts: NormalizeOptions,
    sink: &'s mut S,
    /// Pending whitespace: kind and source offset of its first char.
    ws: WsKind,
    ws_src: Src,
    /// Pending whitespace kept verbatim (only with `Whitespace::Keep`).
    ws_raw: Vec<(char, Src)>,
    /// Pending hyphen after a letter: (char, src, soft).
    hyphen: Option<(char, Src, bool)>,
    /// Last emitted character was a letter.
    after_letter: bool,
    /// Nothing emitted yet (for trimming leading whitespace).
    at_start: bool,
}

fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

fn is_invisible(c: char) -> bool {
    match c {
        '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' => false,
        c if c.is_control() => true,
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}'
        | '\u{180E}' => true,
        _ => false,
    }
}

fn fold_typo(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' | '\u{AB}' | '\u{BB}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FE63}' => '-',
        c => c,
    }
}

fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{2010}' | '\u{2011}')
}

impl<S: Sink> Pipeline<'_, S> {
    #[inline]
    fn emit(&mut self, c: char, src: Src) {
        self.sink.push(c, src);
        self.after_letter = c.is_alphabetic();
        self.at_start = false;
    }

    fn flush_ws(&mut self) {
        match self.opts.whitespace {
            Whitespace::Keep => {
                for i in 0..self.ws_raw.len() {
                    let (c, s) = self.ws_raw[i];
                    self.emit(c, s);
                }
                self.ws_raw.clear();
            }
            Whitespace::CollapseKeepLines | Whitespace::Collapse => {
                let c = match self.ws {
                    WsKind::None => return,
                    WsKind::Line if self.opts.whitespace == Whitespace::CollapseKeepLines => '\n',
                    _ => ' ',
                };
                if !self.at_start {
                    self.emit(c, self.ws_src);
                }
            }
        }
        self.ws = WsKind::None;
    }

    fn flush_hyphen(&mut self) {
        if let Some((h, s, soft)) = self.hyphen.take()
            && !soft
        {
            self.emit(h, s);
        }
    }

    #[inline]
    fn feed(&mut self, c: char, src: Src) {
        if c == '\u{AD}' {
            // Soft hyphen: only meaningful as a line-end hyphenation mark.
            if self.opts.dehyphenate
                && self.after_letter
                && self.ws == WsKind::None
                && self.hyphen.is_none()
            {
                self.hyphen = Some((c, src, true));
            } else if !self.opts.remove_invisible {
                self.flush_pending();
                self.emit(c, src);
            }
            return;
        }
        if self.opts.remove_invisible && is_invisible(c) {
            return;
        }
        let c = if self.opts.fold_typography {
            fold_typo(c)
        } else {
            c
        };
        if c.is_whitespace() {
            let line = is_line_break(c);
            if self.opts.whitespace == Whitespace::Keep {
                self.ws_raw.push((c, src));
            }
            match self.ws {
                WsKind::None => {
                    self.ws = if line { WsKind::Line } else { WsKind::Space };
                    self.ws_src = src;
                }
                WsKind::Space if line => self.ws = WsKind::Line,
                _ => {}
            }
            return;
        }
        if self.hyphen.is_some() {
            if self.ws == WsKind::Line && c.is_lowercase() {
                // ugo-\nvor: drop the hyphen and the line break.
                self.hyphen = None;
                self.ws = WsKind::None;
                self.ws_raw.clear();
                self.emit(c, src);
                return;
            }
            self.flush_pending();
        } else if self.opts.dehyphenate
            && is_hyphen(c)
            && self.after_letter
            && self.ws == WsKind::None
        {
            self.hyphen = Some((c, src, false));
            return;
        }
        self.flush_ws();
        self.emit(c, src);
    }

    fn flush_pending(&mut self) {
        self.flush_hyphen();
        self.flush_ws();
    }

    fn finish(&mut self) {
        self.flush_hyphen();
        if self.opts.whitespace == Whitespace::Keep {
            self.flush_ws();
        }
        // Collapsing modes drop trailing whitespace.
    }
}

/// Whether `d` continues the cluster that ends with `last` (NFKC may merge them).
#[inline]
fn continues_cluster(last: char, d: char) -> bool {
    // Starters that compose with a preceding starter (Hangul jamo, a few Indic vowel signs)
    // all sit at U+0900 and above, so most text skips the composition lookup.
    canonical_combining_class(d) != 0 || (d >= '\u{900}' && compose(last, d).is_some())
}

fn run<S: Sink>(text: &str, opts: NormalizeOptions, sink: &mut S) {
    let mut p = Pipeline {
        opts,
        sink,
        ws: WsKind::None,
        ws_src: (0, 0),
        ws_raw: Vec::new(),
        hyphen: None,
        after_letter: false,
        at_start: true,
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        // ASCII fast path: an ASCII char followed by ASCII (or the end) is its own cluster.
        if b < 0x80 && bytes.get(i + 1).is_none_or(|&n| n < 0x80) {
            p.feed(char::from(b), (i, i + 1));
            i += 1;
            continue;
        }
        let rest = &text[i..];
        let mut chars = rest.char_indices();
        let Some((_, first)) = chars.next() else {
            break;
        };
        let mut end = first.len_utf8();
        let mut last = first;
        if opts.nfkc {
            for (j, d) in chars {
                if !continues_cluster(last, d) {
                    break;
                }
                last = d;
                end = j + d.len_utf8();
            }
        }
        let cluster = &rest[..end];
        let single = end == first.len_utf8();
        if !opts.nfkc || (single && is_nfkc_quick(std::iter::once(first)) == IsNormalized::Yes) {
            p.feed(first, (i, i + end));
        } else {
            for c in cluster.nfkc() {
                p.feed(c, (i, i + end));
            }
        }
        i += end;
    }
    p.finish();
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // test offsets are tiny
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        normalize(s, &NormalizeOptions::default()).into_owned()
    }

    #[test]
    fn clean_text_is_borrowed() {
        let o = NormalizeOptions::default();
        assert!(matches!(
            normalize("Ugovor o zakupu.\nČlan 1.", &o),
            Cow::Borrowed(_)
        ));
        assert!(matches!(normalize("Уговор о закупу", &o), Cow::Borrowed(_)));
        // Trailing whitespace trimmed: still a borrow (a prefix).
        assert!(matches!(normalize("abc  \n", &o), Cow::Borrowed("abc")));
    }

    #[test]
    fn nfkc_ligatures_and_spaces() {
        assert_eq!(n("ﬁnansijski ﬂuks"), "finansijski fluks");
        assert_eq!(n("ǉubav ǋegoš"), "ljubav Njegoš");
        assert_eq!(n("1\u{a0}200,50"), "1 200,50");
        assert_eq!(n("c\u{30c}as"), "čas");
        assert_eq!(n("x²"), "x2");
    }

    #[test]
    fn invisible_and_controls() {
        assert_eq!(n("a\u{200b}b\u{feff}c\u{0}d\u{7}e"), "abcde");
        assert_eq!(n("le\u{ad}ft"), "left");
        assert_eq!(n("\u{202e}abc\u{202c}"), "abc");
    }

    #[test]
    fn typography() {
        assert_eq!(
            n("„Navodnici“ «guillemets» ‘single’"),
            "\"Navodnici\" \"guillemets\" 'single'"
        );
        assert_eq!(n("2020–2023 — test − 5"), "2020-2023 - test - 5");
    }

    #[test]
    fn whitespace_modes() {
        assert_eq!(n("  a \t b \r\n\r\n c  "), "a b\nc");
        let flat = NormalizeOptions {
            whitespace: Whitespace::Collapse,
            ..NormalizeOptions::default()
        };
        assert_eq!(normalize("a\n\nb   c", &flat), "a b c");
        let keep = NormalizeOptions {
            whitespace: Whitespace::Keep,
            ..NormalizeOptions::default()
        };
        assert_eq!(normalize(" a  \n b ", &keep), " a  \n b ");
    }

    #[test]
    fn dehyphenation() {
        assert_eq!(n("ugo-\nvor"), "ugovor");
        assert_eq!(n("ugo- \n  vor je"), "ugovor je");
        assert_eq!(n("ugo\u{ad}\nvor"), "ugovor");
        assert_eq!(n("Novi-\nSad"), "Novi-\nSad");
        assert_eq!(n("crno-beli"), "crno-beli");
        assert_eq!(n("a - b"), "a - b");
        assert_eq!(n("kraj-"), "kraj-");
        assert_eq!(n("12-\nmaj"), "12-\nmaj");
        assert_eq!(n("уго-\nвор"), "уговор");
        let keep = NormalizeOptions {
            whitespace: Whitespace::Keep,
            ..NormalizeOptions::default()
        };
        assert_eq!(normalize("ugo-\nvor x-\nY", &keep), "ugovor x-\nY");
        let off = NormalizeOptions {
            dehyphenate: false,
            ..NormalizeOptions::default()
        };
        assert_eq!(normalize("ugo-\nvor", &off), "ugo-\nvor");
    }

    #[test]
    fn into_appends_and_matches() {
        let o = NormalizeOptions::default();
        let mut buf = String::from(">");
        normalize_into("a  ﬁ", &o, &mut buf);
        assert_eq!(buf, ">a fi");
    }

    #[test]
    fn offsets_map_back() {
        let src = "  Ugo-\nvor ﬁrme „X“ —  kraj";
        let mut out = String::new();
        let mut map = OffsetMap::new();
        normalize_mapped(src, &NormalizeOptions::default(), &mut out, &mut map);
        assert_eq!(out, "Ugovor firme \"X\" - kraj");
        for word in ["Ugo", "firme", "X", "kraj"] {
            let s = out.find(word).unwrap();
            let span = Span {
                start: s as u32,
                end: (s + word.len()) as u32,
            };
            let back = map.span_to_source(span);
            let orig = back.slice(src);
            match word {
                "firme" => assert_eq!(orig, "ﬁrme"),
                w => assert_eq!(orig, w),
            }
        }
        // "vor" sits after the removed "-\n"; "fi" inside the ligature widens to it.
        let s = out.find("vor").unwrap();
        assert_eq!(&src[map.to_source(s)..map.to_source(s + 3)], "vor");
        let f = out.find("firme").unwrap();
        let one = Span {
            start: f as u32,
            end: f as u32 + 1,
        };
        assert_eq!(map.span_to_source(one).slice(src), "ﬁ");
        let whole = Span {
            start: 0,
            end: out.len() as u32,
        };
        assert_eq!(map.span_to_source(whole).slice(src), src.trim());
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let o = NormalizeOptions::default();
        let weird: String = (0u32..0x3000).filter_map(char::from_u32).collect();
        let _ = normalize(&weird, &o);
        let _ = normalize("\u{301}\u{301}-\n\u{ad}", &o);
        let _ = normalize("", &o);
    }
}

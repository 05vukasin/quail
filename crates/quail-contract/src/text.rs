//! Sentences over the keyed lines of `quail_payment::lex`, and the small readers every rule
//! shares: cue search, percentages, amounts, capitalised name runs.
//!
//! A sentence is a [`Line`] whose tokens may come from several lines of one page (a clause
//! wrapped over line breaks). It ends at `.`, `!` or `?` before an uppercase word, a number
//! or the end of a line (not after an abbreviation such as `br.`, `d.o.o.` or a single
//! letter), before a line that starts an enumeration (`2.`, `b)`), a heading word (`Član`,
//! `Article`, `Prilog`) or a signature label (`Za Naručioca`), after a title line in
//! capitals, and after [`MAX_SENT_TOKENS`] tokens.

use quail_core::{Document, Span};
use quail_payment::amount::{Amount, amount_at};
use quail_payment::lex::{Line, Tok, match_at, word_matches};
use quail_text::money::{Decimal, parse_amount};
use quail_text::token::TokenKind;

/// Tokens of one sentence at most; a longer run is cut.
pub const MAX_SENT_TOKENS: usize = 160;

/// Tokens before a `.` that do not end a sentence.
const ABBREV: &[&str] = &[
    "br", "st", "cl", "tj", "npr", "sl", "ul", "bb", "no", "nr", "inc", "ltd", "co", "corp", "art",
    "str", "tel", "mr", "dr", "sv", "itd", "odn", "vs", "approx", "d.o.o", "a.d", "o.d", "k.d",
    "j.p", "s.a", "e.g", "i.e", "mat", "reg", "ziro", "tek", "posl", "ing", "dipl", "prof",
];

/// First words of a line that start a new sentence.
const HEADING_START: &[&str] = &[
    "clan",
    "article",
    "section",
    "prilog",
    "prilozi",
    "annex",
    "schedule",
    "exhibit",
    "appendix",
    "attachment",
    "aneks",
];

/// Whether token `i` of `line` (a `.`, `!` or `?`) ends a sentence.
fn ends_at(doc: &Document, line: &Line, i: usize) -> bool {
    let t = &line.toks[i];
    if t.kind != TokenKind::Punct || !matches!(t.key.as_str(), "." | "!" | "?") {
        return false;
    }
    if t.key == "." && i > 0 {
        let p = &line.toks[i - 1];
        // `PDV-a.` ends a sentence; `J.` and `d.` do not.
        let suffix = i >= 2 && line.toks[i - 2].key == "-";
        if p.kind == TokenKind::Word
            && ((p.key.chars().count() == 1 && !suffix) || ABBREV.contains(&p.key.as_str()))
        {
            return false;
        }
    }
    match line.toks.get(i + 1) {
        None => true,
        Some(n) => match n.kind {
            TokenKind::Number => true,
            TokenKind::Word | TokenKind::Alnum => upper_start(raw(doc, line, n)),
            _ => false,
        },
    }
}

/// The text of a token as written.
pub fn raw<'d>(doc: &'d Document, line: &Line, t: &Tok) -> &'d str {
    line.slice(doc, t.start, t.end)
}

/// Whether `s` starts with an uppercase letter.
pub fn upper_start(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_uppercase)
}

/// Whether every letter of the line's words is uppercase (a title such as `UGOVOR O ...`).
fn title_line(doc: &Document, l: &Line) -> bool {
    let mut words = 0;
    for t in &l.toks {
        if t.kind == TokenKind::Word {
            let s = raw(doc, l, t);
            if s.chars().any(char::is_lowercase) {
                return false;
            }
            words += 1;
        }
    }
    words >= 2
        && !l
            .toks
            .last()
            .is_some_and(|t| matches!(t.key.as_str(), "." | ","))
}

/// Whether a line starts with an enumeration (`2.`, `b)`).
fn p_enum(l: &Line) -> bool {
    let first = &l.toks[0];
    l.toks.len() > 2
        && matches!(l.toks[1].key.as_str(), "." | ")")
        && ((first.kind == TokenKind::Number && first.key.len() <= 2)
            || (first.kind == TokenKind::Word && first.key.chars().count() == 1))
}

/// Whether a sentence break sits between `prev` and `next` (two consecutive lines).
fn breaks(doc: &Document, prev: &Line, next: &Line) -> bool {
    if prev.pos != next.pos {
        return true;
    }
    let first = &next.toks[0];
    // `2. Beta d.o.o.`, `b) ...`
    if p_enum(next) {
        return true;
    }
    if HEADING_START.contains(&first.key.as_str()) && upper_start(raw(doc, next, first)) {
        return true;
    }
    if first.key == "za" && upper_start(raw(doc, next, first)) && next.toks.len() <= 4 {
        return true;
    }
    // `1. Term`, `2) Payment`: a short numbered heading without a full stop.
    if prev.toks.len() <= 7
        && prev.toks.len() >= 3
        && p_enum(prev)
        && !prev
            .toks
            .last()
            .is_some_and(|t| matches!(t.key.as_str(), "." | "," | ":" | ";"))
    {
        return true;
    }
    // `Član 2. Cena i plaćanje`: a heading line is a sentence of its own.
    let p0 = &prev.toks[0];
    if HEADING_START.contains(&p0.key.as_str())
        && upper_start(raw(doc, prev, p0))
        && prev.toks.len() <= 10
        && prev.toks.last().is_none_or(|t| t.key != ",")
    {
        return true;
    }
    let lone = |l: &Line| l.toks.len() == 1 && matches!(l.toks[0].key.as_str(), "i" | "and");
    lone(prev) || lone(next) || title_line(doc, prev) || title_line(doc, next)
}

/// The sentences of `lines`, in document order.
pub fn sentences(doc: &Document, lines: &[Line]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut cur: Option<Line> = None;
    for (li, l) in lines.iter().enumerate() {
        if li > 0 && breaks(doc, &lines[li - 1], l) {
            out.extend(cur.take());
        }
        for (k, t) in l.toks.iter().enumerate() {
            let c = cur.get_or_insert_with(|| Line {
                pos: l.pos,
                page: l.page,
                span: Span {
                    start: t.start,
                    end: t.end,
                },
                toks: Vec::new(),
                ocr: l.ocr,
            });
            c.toks.push(t.clone());
            c.span.end = t.end;
            if ends_at(doc, l, k) || c.toks.len() >= MAX_SENT_TOKENS {
                out.extend(cur.take());
            }
        }
    }
    out.extend(cur.take());
    out
}

/// One cue found: its entry and its token range `[start, end)`.
pub struct Hit<'t, T> {
    /// The table entry.
    pub what: &'t T,
    /// First token.
    pub start: usize,
    /// One past the last token.
    pub end: usize,
}

/// Every cue of `table` in `toks`, longest first at each position, not overlapping.
pub fn find<'t, T>(toks: &[Tok], table: &'t [(&'t [&'t str], T)]) -> Vec<Hit<'t, T>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let mut best: Option<(&T, usize)> = None;
        for (phrase, v) in table {
            if let Some(end) = match_at(toks, i, phrase)
                && best.is_none_or(|(_, e)| end > e)
            {
                best = Some((v, end));
            }
        }
        if let Some((what, end)) = best {
            out.push(Hit {
                what,
                start: i,
                end,
            });
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Whether any cue of `words` (single cue words) is among `toks`.
pub fn has(toks: &[Tok], words: &[&str]) -> bool {
    toks.iter()
        .any(|t| words.iter().any(|w| word_matches(w, &t.key)))
}

/// Whether any phrase of `phrases` occurs in `toks`.
pub fn has_phrase(toks: &[Tok], phrases: &[&[&str]]) -> bool {
    (0..toks.len()).any(|i| phrases.iter().any(|p| match_at(toks, i, p).is_some()))
}

/// A percentage at token `i`: a number followed by `%`, `posto`, `procen*` or `percent`.
/// Returns the value and its last token.
pub fn percent_at(doc: &Document, line: &Line, i: usize) -> Option<(Decimal, usize)> {
    let t = line.toks.get(i)?;
    if t.kind != TokenKind::Number {
        return None;
    }
    let next = line.toks.get(i + 1)?;
    let unit = matches!(next.key.as_str(), "%" | "posto" | "percent" | "pct")
        || next.key.starts_with("procen")
        || (next.key == "per" && line.toks.get(i + 2).is_some_and(|t| t.key == "cent"));
    if !unit {
        return None;
    }
    let v = parse_amount(raw(doc, line, t))?;
    let last = if next.key == "per" { i + 2 } else { i + 1 };
    Some((v, last))
}

/// The first percentage in tokens `from..to`.
pub fn first_percent(
    doc: &Document,
    line: &Line,
    from: usize,
    to: usize,
) -> Option<(Decimal, usize, usize)> {
    (from..to.min(line.toks.len()))
        .find_map(|i| percent_at(doc, line, i).map(|(v, last)| (v, i, last)))
}

/// The first amount with a currency in tokens `from..to`.
pub fn first_money(doc: &Document, line: &Line, from: usize, to: usize) -> Option<Amount> {
    (from..to.min(line.toks.len())).find_map(|i| {
        let a = amount_at(doc, line, i)?;
        a.currency.is_some().then_some(a)
    })
}

/// A run of 2 or 3 capitalised words starting at token `i` that reads as a person's name:
/// letters only, not a cue word of `stop`. Returns its last token.
pub fn name_at(doc: &Document, line: &Line, i: usize, stop: &[&str]) -> Option<usize> {
    let mut last = None;
    let mut n = 0;
    let mut k = i;
    while k < line.toks.len() && n < 3 {
        let t = &line.toks[k];
        let s = raw(doc, line, t);
        if t.kind != TokenKind::Word
            || !upper_start(s)
            || s.chars().count() < 2
            || stop.iter().any(|w| word_matches(w, &t.key))
        {
            break;
        }
        if n > 0 && line.gap_before(doc, k) != " " {
            break;
        }
        last = Some(k);
        n += 1;
        k += 1;
    }
    if n >= 2 { last } else { None }
}

/// The text of tokens `a..=b`, as written (from the first byte of `a` to the last of `b`).
pub fn text_of(doc: &Document, line: &Line, a: usize, b: usize) -> String {
    let s = line.slice(doc, line.toks[a].start, line.toks[b].end);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The sentence text, collapsed and cut to `max` characters at a word boundary.
pub fn clip(doc: &Document, line: &Line, max: usize) -> String {
    let s = text_of(doc, line, 0, line.toks.len() - 1);
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max).collect();
    if let Some(p) = out.rfind(' ') {
        out.truncate(p);
    }
    out.push('…');
    out
}

/// The byte span of tokens `a..=b`.
pub fn span_of(line: &Line, a: usize, b: usize) -> Span {
    Span {
        start: line.toks[a].start,
        end: line.toks[b.min(line.toks.len() - 1)].end,
    }
}

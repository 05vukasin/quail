//! Price candidates: an amount after a price cue on the same line (or alone on the next
//! line, as in a two-column table), with the VAT role and period the words around it give.
//!
//! Numbers, currencies and the Serbian and English separator rules are read by
//! `quail_text::money` (shared with `quail-signals` and `quail-items`): `1.234.567,89`,
//! `1,234,567.89`, `1 234 567,89`, `RSD`, `din.`, `дин.`, `€`, `EUR`, `$`, `USD`.

use quail_core::Document;
use quail_text::money::{self, Decimal};
use quail_text::token::TokenKind;

use crate::cues::{self, Period, Role, Strength};
use crate::lex::{Line, Tok, longest_at, word_matches};

/// Most amounts read after one cue.
const PER_CUE: usize = 3;
/// Tokens after an amount searched for a VAT marker.
const MARK_AFTER: usize = 6;

/// An amount as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Amount {
    /// Its value.
    pub value: Decimal,
    /// Its currency, when written next to it.
    pub currency: Option<[u8; 3]>,
    /// First token (the currency when it comes first).
    pub first: usize,
    /// Last token (the currency when it comes last).
    pub last: usize,
}

/// A price candidate.
#[derive(Clone, Debug)]
pub struct Cand {
    /// The amount.
    pub value: Decimal,
    /// Its currency, when written.
    pub currency: Option<[u8; 3]>,
    /// What it is.
    pub role: Role,
    /// How strong its cue is.
    pub strength: Strength,
    /// The period its line gives.
    pub period: Option<Period>,
    /// Page index (`Page::index`).
    pub page: u32,
    /// The page's text came from OCR.
    pub ocr: bool,
    /// Byte range on the page: from the cue to the end of the amount (and its VAT marker).
    pub start: u32,
    /// End of that range.
    pub end: u32,
    /// The amount was alone on the line after its cue.
    pub next_line: bool,
}

fn currency_of(doc: &Document, line: &Line, t: &Tok) -> Option<[u8; 3]> {
    match t.kind {
        TokenKind::Symbol => {
            let mut c = t.key.chars();
            match (c.next(), c.next()) {
                (Some(ch), None) => money::currency_symbol(ch),
                _ => None,
            }
        }
        TokenKind::Word | TokenKind::Alnum => money::currency_word(&t.key)
            .or_else(|| money::currency_code(line.slice(doc, t.start, t.end))),
        _ => None,
    }
}

fn pure(t: &str, n: std::ops::RangeInclusive<usize>) -> bool {
    n.contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit())
}

/// Three digits and a decimal part: the last group of `1 234 567,89` when the tokenizer
/// did not merge it.
fn last_group(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() >= 5
        && b[..3].iter().all(u8::is_ascii_digit)
        && matches!(b[3], b',' | b'.')
        && (1..=2).contains(&(b.len() - 4))
        && b[4..].iter().all(u8::is_ascii_digit)
}

/// Units that make a number something other than money (`30 dana`, `20 %`, `12 months`).
const NOT_MONEY_AFTER: &[&str] = &[
    "%", "dan*", "days", "day", "mesec*", "meseci", "month*", "godin*", "year*", "sat*", "hour*",
    "kom*", "pcs", "h", "km", "kg", "m2", "m", "procen*", "percent",
];

/// The amount whose number (or currency) is token `i`.
pub fn amount_at(doc: &Document, line: &Line, i: usize) -> Option<Amount> {
    let toks = &line.toks;
    let first = i;
    let mut n = i;
    let mut currency = None;
    if toks.get(i)?.kind != TokenKind::Number {
        currency = Some(currency_of(doc, line, &toks[i])?);
        n = i + 1;
        // `RSD: 1.000,00`, `EUR -`.
        while toks
            .get(n)
            .is_some_and(|t| t.kind == TokenKind::Punct && t.key == ":")
        {
            n += 1;
        }
        if toks.get(n)?.kind != TokenKind::Number {
            return None;
        }
    }
    let text = |k: usize| line.slice(doc, toks[k].start, toks[k].end);
    // Space-grouped thousands the tokenizer left apart: `1 234 567`.
    let mut last = n;
    let mut num = text(n).to_owned();
    if pure(text(n), 1..=3) {
        let mut k = n + 1;
        while k < toks.len() && k <= n + 6 && line.gap_before(doc, k) == " " {
            let g = text(k);
            if pure(g, 3..=3) {
                num.push(' ');
                num.push_str(g);
                last = k;
                k += 1;
            } else if last_group(g) {
                num.push(' ');
                num.push_str(g);
                last = k;
                break;
            } else {
                break;
            }
        }
    }
    let value = money::parse_amount(&num)?;
    // `1.000,- RSD`, `1.000,00 din.`
    let mut k = last + 1;
    if toks.get(k).is_some_and(|t| t.key == ",") && toks.get(k + 1).is_some_and(|t| t.key == "-") {
        k += 2;
    }
    if let Some(t) = toks.get(k) {
        if NOT_MONEY_AFTER.iter().any(|w| word_matches(w, &t.key)) && currency.is_none() {
            return None;
        }
        if currency.is_none()
            && let Some(c) = currency_of(doc, line, t)
        {
            currency = Some(c);
            last = k;
            if toks.get(k + 1).is_some_and(|t| t.key == ".") && t.key == "din" {
                last = k + 1;
            }
        }
    }
    Some(Amount {
        value,
        currency,
        first,
        last,
    })
}

/// Whether an amount without a currency still reads as money: decimals or thousand groups,
/// and at least 100.
fn money_like(doc: &Document, line: &Line, a: &Amount) -> bool {
    if a.currency.is_some() {
        return true;
    }
    let t = line.slice(doc, line.toks[a.first].start, line.toks[a.last].end);
    let grouped = t.contains(['.', ',', ' ', '\u{a0}']);
    let big = a.value.to_f64() >= 100.0;
    (a.value.scale > 0 || grouped) && big
}

/// The end of token `i` and the tokens glued to it (`PDV` + `-a`, `-om`).
fn glued_end(toks: &[Tok], i: usize) -> u32 {
    let mut j = i;
    while j + 1 < toks.len() && toks[j + 1].start == toks[j].end && toks[j + 1].key != "." {
        j += 1;
    }
    toks[j].end
}

/// One past the last token of the first money amount at or after token `from`.
fn next_money(doc: &Document, line: &Line, from: usize) -> Option<usize> {
    (from..line.toks.len()).find_map(|k| {
        amount_at(doc, line, k)
            .filter(|a| money_like(doc, line, a))
            .map(|a| a.last + 1)
    })
}

/// A cue found on a line.
#[derive(Clone, Copy, Debug)]
struct CueAt {
    start: usize,
    end: usize,
    strength: Strength,
    role: Role,
    period: Option<Period>,
}

/// Index of the first word token of the line, skipping numbering (`3.`, `a)`, `-`).
fn first_word(toks: &[Tok]) -> Option<usize> {
    toks.iter()
        .position(|t| t.kind == TokenKind::Word)
        .filter(|&i| i <= 4)
}

fn cues_on(doc: &Document, line: &Line) -> Vec<CueAt> {
    let toks = &line.toks;
    let mut out = Vec::new();
    if let Some(f) = first_word(toks)
        && let Some(((), end)) = longest_at(toks, f, cues::VAT_LABEL).map(|(u, e)| (*u, e))
        && longest_at(toks, f, cues::PRICE).is_none()
    {
        out.push(CueAt {
            start: f,
            end,
            strength: Strength::Fair,
            role: Role::Vat,
            period: None,
        });
    }
    let mut i = match out.last() {
        // `PDV (20%) iznosi 246.913,58 RSD`: the words up to the VAT amount are no cue.
        Some(c) => match next_money(doc, line, c.end) {
            Some(next) => next,
            None => return out,
        },
        None => 0,
    };
    while i < toks.len() {
        if let Some((&(strength, role, period), end)) = longest_at(toks, i, cues::PRICE) {
            out.push(CueAt {
                start: i,
                end,
                strength,
                role,
                period,
            });
            // Words between a cue and its amount are not cues of their own (`Ukupna vrednost
            // ugovora iznosi 1.000 RSD`): the next cue comes after the next number.
            match next_money(doc, line, end) {
                Some(next) => i = next,
                None => break,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The first VAT marker in tokens `a..b`.
fn mark_in(toks: &[Tok], a: usize, b: usize, last: bool) -> Option<Role> {
    let mut found = None;
    let mut i = a;
    while i < b.min(toks.len()) {
        if let Some((&r, end)) = longest_at(toks, i, cues::VAT_MARK) {
            if !last {
                return Some(r);
            }
            found = Some(r);
            i = end;
        } else {
            i += 1;
        }
    }
    found
}

/// The period words of a line say, the first found.
pub fn period_on(toks: &[Tok]) -> Option<Period> {
    (0..toks.len()).find_map(|i| longest_at(toks, i, cues::PERIOD).map(|(&p, _)| p))
}

fn not_price(toks: &[Tok], before: usize) -> bool {
    toks[..before.min(toks.len())]
        .iter()
        .any(|t| cues::NOT_PRICE.iter().any(|w| word_matches(w, &t.key)))
}

/// The price candidates of line `li` of `lines`.
pub fn scan(doc: &Document, lines: &[Line], li: usize, out: &mut Vec<Cand>) {
    let line = &lines[li];
    let toks = &line.toks;
    let cues = cues_on(doc, line);
    if cues.is_empty() {
        return;
    }
    let line_period = period_on(toks);
    for (c, cue) in cues.iter().enumerate() {
        let seg_end = cues.get(c + 1).map_or(toks.len(), |n| n.start);
        let mut found = 0;
        let mut k = cue.end;
        let mut prev_end = cue.start;
        while k < seg_end && found < PER_CUE {
            let Some(a) = amount_at(doc, line, k) else {
                k += 1;
                continue;
            };
            k = a.last + 1;
            if !money_like(doc, line, &a)
                || (cue.strength == Strength::Weak && a.currency.is_none())
            {
                continue;
            }
            found += 1;
            let after = mark_in(
                toks,
                a.last + 1,
                (a.last + 1 + MARK_AFTER).min(seg_end),
                false,
            );
            let before = mark_in(toks, prev_end, a.first, true);
            let role = if cue.role == Role::Vat {
                Role::Vat
            } else {
                after.or(before).unwrap_or(cue.role)
            };
            let mut end = toks[a.last].end;
            if after.is_some() {
                // Reach to the end of the marker.
                let mut j = a.last + 1;
                while j < (a.last + 1 + MARK_AFTER).min(seg_end) {
                    if let Some((_, e)) = longest_at(toks, j, cues::VAT_MARK) {
                        end = glued_end(toks, e - 1);
                        break;
                    }
                    j += 1;
                }
            }
            prev_end = a.last + 1;
            if cue.strength < Strength::Strong && not_price(toks, a.first) {
                continue;
            }
            if a.value.units <= 0 && role != Role::Vat {
                continue;
            }
            out.push(Cand {
                value: a.value,
                currency: a.currency,
                role,
                strength: cue.strength,
                period: cue.period.or(line_period),
                page: line.page,
                ocr: line.ocr,
                start: toks[cue.start].start,
                end,
                next_line: false,
            });
        }
        // A label cell or row with its amount on the next line: `Ukupno za uplatu:` /
        // `120.000,00 RSD`.
        let rest_empty = toks[cue.end..seg_end].iter().all(|t| !t.wordlike());
        if found == 0
            && rest_empty
            && c + 1 == cues.len()
            && let Some(next) = lines.get(li + 1)
            && next.pos == line.pos
        {
            let start = next.toks.iter().position(Tok::wordlike).filter(|&s| s <= 1);
            if let Some(s) = start
                && let Some(a) = amount_at(doc, next, s)
                && money_like(doc, next, &a)
                && (cue.strength > Strength::Weak || a.currency.is_some())
                && (a.value.units > 0 || cue.role == Role::Vat)
                && !(cue.strength < Strength::Strong && not_price(toks, toks.len()))
            {
                let after = mark_in(&next.toks, a.last + 1, a.last + 1 + MARK_AFTER, false);
                let before = mark_in(toks, cue.start, toks.len(), true);
                let role = if cue.role == Role::Vat {
                    Role::Vat
                } else {
                    after.or(before).unwrap_or(cue.role)
                };
                out.push(Cand {
                    value: a.value,
                    currency: a.currency,
                    role,
                    strength: cue.strength,
                    period: cue.period.or(line_period).or_else(|| period_on(&next.toks)),
                    page: next.page,
                    ocr: next.ocr,
                    start: next.toks[a.first].start,
                    end: next.toks[a.last].end,
                    next_line: true,
                });
            }
        }
    }
}

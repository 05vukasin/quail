//! Amounts of money: a number next to a currency (`1.200,50 RSD`, `RSD 1 200,50`,
//! `€1,200.50`, `USD 5 million`, `1.000,00 дин.`), read as an exact decimal, plus total and
//! VAT hints from keywords around it.
//!
//! # Reading the number
//!
//! Numbers, currency words, symbols, codes and multiplier words are read by
//! `quail_text::money` (`parse_amount`, i.e. `NumberFormat::MONEY`), shared with
//! `quail-items`. The rules, in short:
//!
//! - With both `.` and `,`, the later one is the decimal separator (`1.200,50`, `1,200.50`).
//! - With one kind only: one separator followed by one or two digits is decimal (`1200,5`,
//!   `99.90`); followed by exactly three digits, or repeated, it groups thousands
//!   (`1.200`, `1,200`, `1,200,000`). Three decimals are never read: money is written with
//!   two.
//! - Spaces group thousands (`1 200 000,00`), only in groups of exactly three digits.
//! - Thousand groups must be well-formed (`1.20.000` is no amount).
//! - `million`, `milion(a)`, `mln`, `billion`, `milijard(a|i|e)`, `thousand`, `hiljad(a|e|u)`
//!   after the number multiply it.
//!
//! An amount without a currency is kept only after a total keyword and with a decimal part
//! (`Ukupno: 12.345,00`), at lower confidence.

use quail_core::{Money, Span, Value};
use quail_text::money;
use quail_text::token::TokenKind;

use crate::conf;
use crate::keys;
use crate::page::PageText;
use crate::phrase::{Reach, find_after, find_before};
use crate::scan::Cand;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Vat {
    Gross,
    Net,
}

const VAT: &[(&[&str], Vat)] = &[
    (&["sa", "pdv"], Vat::Gross),
    (&["s", "pdv"], Vat::Gross),
    (&["ukljucujuci", "pdv"], Vat::Gross),
    (&["ukljucen", "pdv"], Vat::Gross),
    (&["sa", "uracunatim", "pdv"], Vat::Gross),
    (&["uracunat", "pdv"], Vat::Gross),
    (&["incl", "vat"], Vat::Gross),
    (&["inc", "vat"], Vat::Gross),
    (&["including", "vat"], Vat::Gross),
    (&["inclusive", "of", "vat"], Vat::Gross),
    (&["vat", "included"], Vat::Gross),
    (&["vat", "inclusive"], Vat::Gross),
    (&["bez", "pdv"], Vat::Net),
    (&["bez", "pdv", "a"], Vat::Net),
    (&["plus", "pdv"], Vat::Net),
    (&["excl", "vat"], Vat::Net),
    (&["ex", "vat"], Vat::Net),
    (&["excluding", "vat"], Vat::Net),
    (&["exclusive", "of", "vat"], Vat::Net),
    (&["plus", "vat"], Vat::Net),
    (&["plus", "applicable", "vat"], Vat::Net),
    (&["net", "of", "vat"], Vat::Net),
    (&["vat", "excluded"], Vat::Net),
    (&["net", "total"], Vat::Net),
    (&["total", "before", "vat"], Vat::Net),
    (&["total", "excl", "vat"], Vat::Net),
    (&["gross", "total"], Vat::Gross),
    (&["total", "incl", "vat"], Vat::Gross),
    (&["vat", "exclusive"], Vat::Net),
];

const TOTALS: &[(&[&str], ())] = &[
    (&["ukupno"], ()),
    (&["sveukupno"], ()),
    (&["ukupan", "iznos"], ()),
    (&["ukupni", "iznos"], ()),
    (&["ukupna", "cena"], ()),
    (&["ukupna", "vrednost"], ()),
    (&["ukupna", "vrijednost"], ()),
    (&["za", "uplatu"], ()),
    (&["za", "placanje"], ()),
    (&["svega"], ()),
    (&["total"], ()),
    (&["amount", "due"], ()),
    (&["balance", "due"], ()),
];

const NEAR: Reach = Reach { words: 4, lines: 1 };
const ADJACENT: Reach = Reach { words: 0, lines: 1 };
const SAME_LINE: Reach = Reach { words: 3, lines: 0 };
const BEFORE_LINE: Reach = Reach { words: 4, lines: 1 };

/// Currency of token `i`, if it is one: a word in any case, a symbol, or an ISO code in
/// capitals (`quail_text::money`).
fn currency(p: &PageText, i: usize) -> Option<[u8; 3]> {
    let key = p.key(i);
    let mut chars = key.chars();
    let symbol = match (chars.next(), chars.next()) {
        (Some(c), None) => money::currency_symbol(c),
        _ => None,
    };
    money::currency_word(key)
        .or(symbol)
        .or_else(|| money::currency_code(p.text(i)))
}

/// Units and scale of an amount as written (`quail_text::money::parse_amount`).
fn parse_amount(s: &str) -> Option<(i64, u8)> {
    money::parse_amount(s).map(|d| (d.units, d.scale))
}

fn pure(t: &str, n: std::ops::RangeInclusive<usize>) -> bool {
    n.contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit())
}

/// Three digits plus a decimal part: the last group of `1 200 000,00`.
fn last_group(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() >= 5
        && b[..3].iter().all(u8::is_ascii_digit)
        && matches!(b[3], b',' | b'.')
        && (2..=3).contains(&(b.len() - 4))
        && b[4..].iter().all(u8::is_ascii_digit)
}

/// The amount whose first token is `s`: the token itself, or `1 200 000` spread over
/// space-separated groups. Returns the text and the last token.
fn amount_from(p: &PageText, s: usize) -> Option<(String, usize)> {
    if p.kind(s)? != TokenKind::Number {
        return None;
    }
    let t = p.text(s);
    let mut text = t.to_owned();
    let mut e = s;
    if pure(t, 1..=3) {
        let mut k = s + 1;
        while k < p.len() && k <= s + 6 && p.gap_before(k) == Some(" ") {
            let g = p.text(k);
            if pure(g, 3..=3) {
                text.push(' ');
                text.push_str(g);
                e = k;
                k += 1;
            } else if last_group(g) {
                text.push(' ');
                text.push_str(g);
                e = k;
                break;
            } else {
                break;
            }
        }
    }
    Some((text, e))
}

/// The amount whose last token is `e`, walking left over space-separated thousand groups.
fn amount_to(p: &PageText, e: usize) -> Option<(String, usize)> {
    if p.kind(e)? != TokenKind::Number {
        return None;
    }
    let t = p.text(e);
    let mut s = e;
    if pure(t, 3..=3) || last_group(t) {
        let mut k = e;
        while k > 0 && e - k < 6 && p.gap_before(k) == Some(" ") {
            let prev = p.text(k - 1);
            if pure(prev, 3..=3) {
                k -= 1;
            } else if pure(prev, 1..=2) {
                k -= 1;
                break;
            } else {
                break;
            }
        }
        // Only a run that ends on a short leading group is one number (`1 200 000`); a run of
        // three-digit groups alone is a table column.
        if k < e && pure(p.text(k), 1..=3) {
            s = k;
        }
    }
    let mut text = String::new();
    for j in s..=e {
        if j > s {
            text.push(' ');
        }
        text.push_str(p.text(j));
    }
    Some((text, s))
}

/// Applies a multiplier word after token `e` (`5 million`).
fn multiplied(p: &PageText, e: usize, units: i64, scale: u8) -> Option<(i64, u8, usize)> {
    let k = e + 1;
    if k < p.len()
        && p.tight(k)
        && let Some(pow) = money::multiplier(p.key(k))
    {
        let u = units.checked_mul(10i64.checked_pow(pow)?)?;
        return Some((u, scale, k));
    }
    Some((units, scale, e))
}

/// Scans a page for amounts.
pub(crate) fn scan(p: &PageText, out: &mut Vec<Cand>) {
    let mut used_until = 0usize;
    let mut i = 0;
    while i < p.len() {
        let Some(cur) = currency(p, i) else {
            if let Some(last) = bare_total(p, i, used_until, out) {
                used_until = last + 1;
                i = last + 1;
            } else {
                i += 1;
            }
            continue;
        };
        let symbol = p.kind(i) == Some(TokenKind::Symbol);
        let after = || {
            let s = i + 1;
            (s < p.len() && p.tight(s))
                .then(|| amount_from(p, s).map(|(t, e)| (t, s, e)))
                .flatten()
        };
        let before = || {
            let e = i.checked_sub(1)?;
            (e >= used_until && p.tight(i))
                .then(|| amount_to(p, e).map(|(t, s)| (t, s, e)))
                .flatten()
        };
        let found = if symbol {
            after().or_else(before)
        } else {
            before().or_else(after)
        };
        let Some((text, s, e)) = found else {
            i += 1;
            continue;
        };
        let Some((units, scale)) = parse_amount(&text) else {
            i += 1;
            continue;
        };
        let Some((units, scale, e)) = multiplied(p, e, units, scale) else {
            i += 1;
            continue;
        };
        let (a, b) = (s.min(i), e.max(i));
        let conf = if symbol {
            conf::MONEY_SYMBOL
        } else {
            conf::MONEY_CODE
        };
        emit(p, s, e, p.span(a, b), units, scale, Some(cur), conf, out);
        used_until = b + 1;
        i = b + 1;
    }
}

/// An amount without a currency after a total keyword: `Ukupno: 12.345,00`.
fn bare_total(p: &PageText, i: usize, used_until: usize, out: &mut Vec<Cand>) -> Option<usize> {
    if i < used_until || p.kind(i)? != TokenKind::Number {
        return None;
    }
    let (text, e) = amount_from(p, i)?;
    let (units, scale) = parse_amount(&text)?;
    if scale != 2 {
        return None;
    }
    // A currency right after belongs to the main path.
    if e + 1 < p.len() && p.tight(e + 1) && currency(p, e + 1).is_some() {
        return None;
    }
    // The keyword right before the number (`Ukupno: 12.345,00`): `total of section 4.10`
    // style references are not amounts.
    find_before(p, i, TOTALS, ADJACENT)?;
    emit(
        p,
        i,
        e,
        p.span(i, e),
        units,
        scale,
        None,
        conf::MONEY_BARE_TOTAL,
        out,
    );
    Some(e)
}

#[allow(clippy::too_many_arguments)]
fn emit(
    p: &PageText,
    s: usize,
    e: usize,
    span: Span,
    units: i64,
    scale: u8,
    currency: Option<[u8; 3]>,
    conf: f32,
    out: &mut Vec<Cand>,
) {
    let value = Value::Money(Money {
        units,
        scale,
        currency,
    });
    out.push(Cand::new(keys::AMOUNT, value.clone(), conf, span));
    if find_before(p, s.min(e), TOTALS, NEAR).is_some() {
        out.push(Cand::new(
            keys::AMOUNT_TOTAL,
            value.clone(),
            conf * conf::MONEY_TOTAL_ROLE,
            span,
        ));
    }
    let vat = find_after(p, e, VAT, SAME_LINE).or_else(|| find_before(p, s, VAT, BEFORE_LINE));
    match vat.map(|(v, _)| v) {
        Some(Vat::Gross) => out.push(Cand::new(
            keys::AMOUNT_GROSS,
            value,
            conf * conf::MONEY_VAT_ROLE,
            span,
        )),
        Some(Vat::Net) => out.push(Cand::new(
            keys::AMOUNT_NET,
            value,
            conf * conf::MONEY_VAT_ROLE,
            span,
        )),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_amount;

    #[test]
    fn number_formats() {
        // The rules live in `quail_text::money`; these pin what signals relies on.
        assert_eq!(parse_amount("1.200,50"), Some((120_050, 2)));
        assert_eq!(parse_amount("1,200.50"), Some((120_050, 2)));
        assert_eq!(parse_amount("1 200 000"), Some((1_200_000, 0)));
        assert_eq!(parse_amount("1.200"), Some((1200, 0)));
        assert_eq!(parse_amount("99.9"), Some((999, 1)));
        assert_eq!(parse_amount("1.20.000"), None);
        assert_eq!(parse_amount("12345.678"), None);
        assert_eq!(parse_amount("9999999999999999"), None);
    }
}

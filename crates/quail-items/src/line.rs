//! Items written as one line of text, outside any table:
//!
//! - `Servis klima uređaja ...... 2 x 3.500,00 = 7.000,00 RSD` (quantity × price = amount);
//! - `Usluga čišćenja  10  sat  1.200,00  12.000,00` (a table row whose cells the layout
//!   merged; accepted when quantity × price matches the amount);
//! - `Senior consultant – 85 EUR/h`, `Paušal održavanja: 15.000,00 din mesečno` (rate card:
//!   one price, with a currency, a unit or a separator before it).
//!
//! A line that reads as a sentence (long, a verb-like word before the amount, more words after
//! it) is not an item: contracts state their total value in prose and that is not a price
//! line.

use quail_core::{BBox, Span};

use crate::header;
use crate::item::{DocHints, Item, Source};
use crate::num::{self, Num, NumKind, NumTok};
use crate::table::{self, unit_after, unit_in_desc};
use crate::unit;

/// Words that, right before an amount, make the line a sentence, not an item.
const SENTENCE_TAIL: &[&str] = &[
    "iznosi", "iznos", "iznosu", "od", "do", "is", "of", "be", "are", "was", "amount", "amounts",
    "total", "ukupno", "visini", "vrednosti", "vrednost", "cena", "price", "sum", "uplatu", "approximately",
    "exceed", "exceeding", "than", "up", "to", "na", "za", "sa", "with", "pursuant", "within", "clan", "article",
    "section", "tacka", "stav", "broj", "no", "br", "page", "strana", "year", "godine", "godina", "dana", "days",
    "from", "de", "au", "by", "at", "about", "around", "under", "over", "than", "least", "most", "maximum",
    "minimum", "max", "min", "capital", "kapital", "fund", "budget", "budzet", "limit", "cap", "worth",
];

/// Words that make a description a sentence (a verb or a relative pronoun).
const SENTENCE_WORDS: &[&str] = &[
    "will", "shall", "must", "may", "is", "are", "be", "been", "was", "were", "has", "have", "had",
    "which", "that", "who", "whom", "whose", "if", "when", "where", "pay", "paid", "agrees", "agree",
    "se", "je", "su", "ce", "biti", "bice", "koji", "koja", "koje", "ako", "kada", "placa", "placanje",
    "iznosi", "obavezuje", "dostavlja",
];

/// Words allowed after the last amount.
fn tail_ok(rest: &str) -> bool {
    let w = header::words(rest);
    let mut extra = 0;
    for x in &w {
        let allowed = num::currency_word(x, true).is_some()
            || unit::normalize(x).is_some()
            || matches!(
                x.as_str(),
                "bez" | "sa" | "s" | "pdv" | "pdva" | "pdvom" | "vat" | "excl" | "incl" | "plus" | "po"
                    | "per" | "neto" | "bruto" | "net" | "gross" | "tax" | "taxes" | "mesecno" | "godisnje"
                    | "monthly" | "%" | "$" | "€" | "a" | "e" | "om"
            );
        if !allowed {
            extra += 1;
        }
    }
    extra <= 1
}

fn clean_desc(s: &str) -> &str {
    s.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(c, '.' | '…' | '-' | '–' | '—' | ':' | '|' | '_' | '=' | '*' | '•' | '·' | ',' | ';' | '$' | '€' | '£' | '(')
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= (0.011f64).max(0.01 * b.abs())
}

/// Parses one text line. `span` and `bbox` locate it on the page.
#[allow(clippy::too_many_lines)]
pub(crate) fn parse(
    text: &str,
    page: u32,
    span: Span,
    bbox: Option<BBox>,
    hints: &DocHints,
) -> Option<Item> {
    let t = text.trim();
    if t.len() < 4 || t.len() > 220 {
        return None;
    }
    let toks = num::scan(t, hints.dec, hints.order);
    let values: Vec<&NumTok> = toks
        .iter()
        .filter(|x| matches!(x.kind, NumKind::Number(_)) && !x.glued)
        .collect();
    let first_money = values.iter().position(|v| v.moneylike())?;
    // The description: text before the first money value, minus a leading ordinal and dates.
    let money = values[first_money];
    let mut head = &t[..money.start];
    let mut dates = Vec::new();
    for x in &toks {
        if x.end > money.start {
            break;
        }
        if let NumKind::Date(d) = x.kind {
            dates.push(d);
        }
    }
    // Leading ordinal: `1.`, `12)`, `1`.
    let lead: usize = head
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || matches!(c, '.' | ')' | ' '))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    if lead > 0 && head[..lead].chars().any(|c| c.is_ascii_digit()) {
        head = &head[lead..];
    }
    let mut desc = clean_desc(head).to_owned();
    let mut qty: Option<(Num, Option<(&'static str, String)>)> = None;
    // `Usluga 2 kom 1.200,00`: quantity and unit at the end of the description.
    {
        let owned = desc.clone();
        let mut w: Vec<&str> = owned.split_whitespace().collect();
        // A currency word right before the price (`RSD 1.200,00`) is not description.
        if w.len() >= 2 && w.last().is_some_and(|x| num::currency_word(&unit::key(x), false).is_some()) {
            w.pop();
            desc = clean_desc(&w.join(" ")).to_owned();
        }
        // `2 x 3.500,00`: quantity times price.
        if w.len() >= 3 && matches!(w[w.len() - 1], "x" | "X" | "×" | "*") {
            if let Some(q) = num::parse_number(w[w.len() - 2], hints.dec) {
                qty = Some((q, None));
                w.truncate(w.len() - 2);
                desc = clean_desc(&w.join(" ")).to_owned();
            }
        } else if w.len() >= 3 {
            let (n, u) = (w[w.len() - 2], w[w.len() - 1]);
            if let (Some(q), Some(un)) = (num::parse_number(n, hints.dec), unit::of_phrase(u)) {
                qty = Some((q, Some((un, u.to_owned()))));
                desc = clean_desc(&w[..w.len() - 2].join(" ")).to_owned();
            }
        }
    }
    let letters = desc.chars().filter(|c| c.is_alphabetic()).count();
    let words = desc.split_whitespace().count();
    if letters < 3 || words > 14 || desc.chars().count() > 120 {
        return None;
    }
    if table::total_kind(&desc).is_some() || table::is_date_text(&desc) {
        return None;
    }
    let desc_words = header::words(&desc);
    let last_word = desc_words.last().cloned().unwrap_or_default();
    if SENTENCE_TAIL.contains(&last_word.as_str())
        || desc_words.iter().any(|w| SENTENCE_WORDS.contains(&w.as_str()))
        || desc.starts_with(|c: char| c.is_lowercase())
    {
        return None;
    }
    // Values from the first money value on.
    let after: Vec<&NumTok> = values[first_money..].to_vec();
    let last = after.last()?;
    if !tail_ok(&t[last.end..]) {
        return None;
    }
    // Text between values must be short (units, `x`, `=`, currency).
    for w in after.windows(2) {
        let between = &t[w[0].end..w[1].start];
        if header::words(between).len() > 3 {
            return None;
        }
    }
    let mut it = Item::new(page, Source::Text, desc);
    it.span = span;
    it.bbox = bbox;
    it.currency = after.iter().find_map(|v| v.currency).or(hints.currency);
    it.vat_included = hints.vat;
    let explicit_vat = header::words(&t[last.end..]);
    if explicit_vat.iter().any(|w| w == "bez" || w == "excl" || w == "plus" || w == "neto" || w == "net") {
        it.vat_included = Some(false);
    } else if explicit_vat.iter().any(|w| w == "sa" || w == "incl" || w == "bruto" || w == "gross") {
        it.vat_included = Some(true);
    }
    it.vat_rate = toks.iter().find_map(|x| match x.kind {
        NumKind::Percent(v) => Some(v),
        _ => None,
    });
    let nums: Vec<Num> = after.iter().filter_map(|v| v.number()).collect();
    let mul = t[money.start..].contains(['x', '×', '*']) && t[money.start..].contains('=');
    match nums.as_slice() {
        [p] => {
            // A rate card line: one price.
            let unit = unit_after(t, money.end).or_else(|| unit_in_desc(&it.description));
            let separated = head.trim_end().ends_with(['–', '-', ':', '.', '…', '|', '—']);
            if !(money.currency.is_some() || unit.is_some() || (separated && hints.commercial)) {
                return None;
            }
            it.unit_price = Some(*p);
            if let Some((u, raw)) = unit {
                it.unit = Some(u);
                it.unit_raw = Some(raw);
            }
            if let Some((q, u)) = qty {
                it.quantity = Some(q);
                if let Some((u, raw)) = u {
                    it.unit = Some(u);
                    it.unit_raw = Some(raw);
                }
                // With a quantity, a single amount is the line total.
                it.total = it.unit_price.take();
            }
            it.conf = 0.4 + if it.unit.is_some() { 0.1 } else { 0.0 } + if money.currency.is_some() { 0.05 } else { 0.0 };
        }
        [a, b] => {
            if let Some((q, u)) = &qty {
                // `2 kom 1.200,00 2.400,00`
                if close(q.to_f64() * a.to_f64(), b.to_f64()) {
                    it.quantity = Some(*q);
                    if let Some((u, raw)) = u {
                        it.unit = Some(u);
                        it.unit_raw = Some(raw.clone());
                    }
                    it.unit_price = Some(*a);
                    it.total = Some(*b);
                    it.conf = 0.65;
                } else {
                    return None;
                }
            } else if a.units != 0 && [1.2, 1.1, 1.18, 1.19, 1.25, 1.21].iter().any(|r| close(a.to_f64() * r, b.to_f64())) {
                it.unit_price = Some(*a);
                it.unit_price_gross = Some(*b);
                it.vat_included = Some(false);
                it.conf = 0.5;
            } else if mul {
                return None;
            } else {
                // Price and amount of an unknown quantity, or two prices: keep the first.
                it.unit_price = Some(*a);
                it.conf = 0.35;
            }
        }
        many => {
            let n = many.len();
            let (q, p, tot) = (many[n - 3], many[n - 2], many[n - 1]);
            if close(q.to_f64() * p.to_f64(), tot.to_f64()) {
                it.quantity = Some(q);
                it.unit_price = Some(p);
                it.total = Some(tot);
                it.conf = if mul { 0.7 } else { 0.6 };
                if let Some((u, raw)) = qty.and_then(|q| q.1).or_else(|| unit_after(t, after[n - 3].end)) {
                    it.unit = Some(u);
                    it.unit_raw = Some(raw);
                }
            } else {
                return None;
            }
        }
    }
    if it.unit.is_none()
        && let Some((u, raw)) = unit_in_desc(&it.description)
    {
        it.unit = Some(u);
        it.unit_raw = Some(raw);
    }
    if let Some(d) = dates.first() {
        it.valid_from = Some(*d);
        it.valid_to = dates.get(1).copied();
    }
    Some(it)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::num::Dec;

    fn p(s: &str) -> Option<Item> {
        let h = DocHints {
            dec: Dec::Comma,
            ..DocHints::default()
        };
        parse(s, 0, Span { start: 0, end: 0 }, None, &h)
    }

    #[test]
    fn rate_cards() {
        let a = p("Senior consultant – 85 EUR/h").expect("rate");
        assert_eq!(a.description, "Senior consultant");
        assert_eq!(a.unit, Some("h"));
        assert_eq!(a.currency, Some(*b"EUR"));
        assert_eq!(a.unit_price.map(|n| n.units), Some(85));
        let b = p("Paušal održavanja: 15.000,00 din mesečno").expect("lump");
        assert_eq!(b.unit, Some("month"));
        assert_eq!(b.unit_price.map(|n| n.units), Some(1_500_000));
    }

    #[test]
    fn multiplication_line() {
        let a = p("Servis klima uređaja ...... 2 x 3.500,00 = 7.000,00 RSD").expect("mul");
        assert_eq!(a.description, "Servis klima uređaja");
        assert_eq!(a.quantity.map(|n| n.units), Some(2));
        assert_eq!(a.total.map(|n| n.units), Some(700_000));
        assert_eq!(a.currency, Some(*b"RSD"));
    }

    #[test]
    fn merged_row() {
        let a = p("1. Usluga čišćenja 10 sat 1.200,00 12.000,00").expect("row");
        assert_eq!(a.description, "Usluga čišćenja");
        assert_eq!(a.unit, Some("h"));
        assert_eq!(a.quantity.map(|n| n.units), Some(10));
    }

    #[test]
    fn prose_is_not_an_item() {
        assert!(p("Ukupna vrednost ugovora iznosi 1.200.000,00 dinara bez PDV-a, a plaćanje se vrši u roku od 45 dana.").is_none());
        assert!(p("The total price shall not exceed $50,000.00").is_none());
        assert!(p("Ukupno: 12.000,00").is_none());
        assert!(p("Član 5").is_none());
    }
}

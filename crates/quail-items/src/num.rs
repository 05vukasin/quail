//! Numbers, amounts, percentages and dates inside one cell or line.
//!
//! The scanner is a single pass over the characters, no regular expressions. Separator rules
//! follow `quail-signals` (later of `.`/`,` is decimal; a single separator followed by one or
//! two digits is decimal, by three digits it groups thousands), with two differences that
//! line items need: quantities and unit prices may have three or four decimals (`0,0450` per
//! kWh, `1,500` kg), resolved by the document's decimal separator ([`Dec`]); and a leading
//! minus sign is kept (credits, discounts).

use quail_core::{Date, Money};

/// The document's decimal separator, learned from its unambiguous amounts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Dec {
    /// Not known.
    #[default]
    Unknown,
    /// `1.200,50`.
    Comma,
    /// `1,200.50`.
    Dot,
}

/// An exact decimal `units × 10^-scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Num {
    pub units: i64,
    pub scale: u8,
}

impl Num {
    #[allow(clippy::cast_precision_loss)]
    pub fn to_f64(self) -> f64 {
        self.units as f64 / 10f64.powi(i32::from(self.scale))
    }

    pub fn money(self, currency: Option<[u8; 3]>) -> Money {
        Money {
            units: self.units,
            scale: self.scale,
            currency,
        }
    }
}

/// Parses a number as written: digits with `.`, `,`, `'` or space groups, optional sign.
pub(crate) fn parse_number(s: &str, dec: Dec) -> Option<Num> {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix(['-', '−']) {
        Some(rest) => (true, rest.trim_start()),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let b = body.as_bytes();
    let (first, last) = (b.first()?, b.last()?);
    if !first.is_ascii_digit() || !last.is_ascii_digit() {
        return None;
    }
    if !b
        .iter()
        .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b',' | b' ' | b'\''))
    {
        return None;
    }
    let last_dot = body.rfind('.');
    let last_comma = body.rfind(',');
    let decimal = match (last_dot, last_comma) {
        (Some(d), Some(c)) => Some(d.max(c)),
        (Some(x), None) | (None, Some(x)) => {
            let sep = b[x];
            let count = b.iter().filter(|&&c| c == sep).count();
            let after = b.len() - x - 1;
            let hinted = matches!((sep, dec), (b',', Dec::Comma) | (b'.', Dec::Dot));
            let other_hinted = matches!((sep, dec), (b',', Dec::Dot) | (b'.', Dec::Comma));
            if count > 1 {
                None
            } else if (1..=2).contains(&after) {
                Some(x)
            } else if after == 3 {
                // `1.200` / `1,200`: thousands unless the document writes decimals this way.
                if hinted { Some(x) } else { None }
            } else if after == 4 && !other_hinted {
                Some(x)
            } else {
                return None;
            }
        }
        (None, None) => None,
    };
    let (int, frac) = match decimal {
        Some(x) => (&body[..x], &body[x + 1..]),
        None => (body, ""),
    };
    if frac.len() > 4 || !frac.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let groups: Vec<&str> = int.split([' ', '.', ',', '\'']).collect();
    if groups
        .iter()
        .any(|g| g.is_empty() || !g.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    if groups.len() > 1 && (groups[0].len() > 3 || groups.iter().skip(1).any(|g| g.len() != 3)) {
        return None;
    }
    let digits: usize = groups.iter().map(|g| g.len()).sum::<usize>() + frac.len();
    if digits > 15 {
        return None;
    }
    let mut units: i64 = 0;
    for c in groups.iter().flat_map(|g| g.bytes()).chain(frac.bytes()) {
        units = units.checked_mul(10)?.checked_add(i64::from(c - b'0'))?;
    }
    if neg {
        units = -units;
    }
    Some(Num {
        units,
        scale: u8::try_from(frac.len()).ok()?,
    })
}

/// What a numeric run turned out to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NumKind {
    /// A plain number (amount, quantity).
    Number(Num),
    /// A percentage (`20%`, `20 %`).
    Percent(Num),
    /// A calendar date (`12.09.2023`, `11/04/16`, `2023-09-12`).
    Date(Date),
}

/// One numeric run in a text, with its byte range and what surrounds it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NumTok {
    pub start: usize,
    pub end: usize,
    pub kind: NumKind,
    /// Currency written right before or after it.
    pub currency: Option<[u8; 3]>,
    /// Directly attached to a letter, `/` or `-` (`A4`, `m2`, `114/23`, `PDV-20`): a code,
    /// not an amount.
    pub glued: bool,
    /// Has thousand groups or decimals as written (`1.200`, `12,00`).
    pub formatted: bool,
}

impl NumTok {
    pub fn number(&self) -> Option<Num> {
        match self.kind {
            NumKind::Number(n) => Some(n),
            _ => None,
        }
    }

    /// Money-like: a currency, decimals or thousand groups, and not glued to a code.
    pub fn moneylike(&self) -> bool {
        !self.glued && self.number().is_some() && (self.currency.is_some() || self.formatted)
    }
}

/// How slash dates are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DateOrder {
    /// Day first (`12/09/2023` is 12 September), the European default.
    #[default]
    Dmy,
    /// Month first (`11/04/16` is 4 November), US documents.
    Mdy,
}

/// Currency words and symbols, folded.
const CURRENCY_WORDS: &[(&str, [u8; 3])] = &[
    ("rsd", *b"RSD"),
    ("din", *b"RSD"),
    ("dinar", *b"RSD"),
    ("dinara", *b"RSD"),
    ("dinari", *b"RSD"),
    ("eur", *b"EUR"),
    ("euro", *b"EUR"),
    ("euros", *b"EUR"),
    ("evra", *b"EUR"),
    ("eura", *b"EUR"),
    ("evro", *b"EUR"),
    ("usd", *b"USD"),
    ("us$", *b"USD"),
    ("dollar", *b"USD"),
    ("dollars", *b"USD"),
    ("dolara", *b"USD"),
    ("gbp", *b"GBP"),
    ("chf", *b"CHF"),
    ("km", *b"BAM"),
    ("bam", *b"BAM"),
    ("kn", *b"HRK"),
    ("hrk", *b"HRK"),
    ("cad", *b"CAD"),
    ("aud", *b"AUD"),
    ("sek", *b"SEK"),
    ("nok", *b"NOK"),
    ("dkk", *b"DKK"),
    ("pln", *b"PLN"),
    ("czk", *b"CZK"),
    ("huf", *b"HUF"),
    ("ron", *b"RON"),
    ("bgn", *b"BGN"),
    ("mkd", *b"MKD"),
    ("jpy", *b"JPY"),
    ("cny", *b"CNY"),
    ("inr", *b"INR"),
];

const CURRENCY_SYMBOLS: &[(char, [u8; 3])] = &[
    ('€', *b"EUR"),
    ('$', *b"USD"),
    ('£', *b"GBP"),
    ('¥', *b"JPY"),
    ('₹', *b"INR"),
];

/// Currency of a folded word (`km` only where the caller allows it: it is also kilometres).
pub(crate) fn currency_word(key: &str, allow_km: bool) -> Option<[u8; 3]> {
    if key == "km" && !allow_km {
        return None;
    }
    let key = key.trim_end_matches('.');
    CURRENCY_WORDS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, c)| *c)
}

fn currency_symbol(c: char) -> Option<[u8; 3]> {
    CURRENCY_SYMBOLS
        .iter()
        .find(|(s, _)| *s == c)
        .map(|(_, cur)| *cur)
}

fn is_sep(c: char) -> bool {
    matches!(c, '.' | ',' | '\'')
}

/// Space-like characters that may group thousands.
fn is_group_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}')
}

/// The word (letters only, folded to ASCII lowercase when simple) right after byte `i`,
/// skipping spaces; `None` when the next thing is not a letter.
fn word_after(text: &str, i: usize) -> Option<(String, usize)> {
    let rest = text.get(i..)?;
    let skip = rest.len() - rest.trim_start().len();
    let rest = &rest[skip..];
    let mut w = String::new();
    let mut end = 0;
    for (k, c) in rest.char_indices() {
        if c.is_alphabetic() || (c == '.' && !w.is_empty() && w.len() <= 3) || c == '$' {
            w.extend(c.to_lowercase());
            end = k + c.len_utf8();
            if c == '.' {
                break;
            }
        } else {
            break;
        }
    }
    if w.is_empty() {
        None
    } else {
        Some((fold_simple(&w), i + skip + end))
    }
}

fn word_before(text: &str, i: usize) -> Option<String> {
    let head = text.get(..i)?.trim_end();
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphabetic() || *c == '$' || *c == '.')
        .last()
        .map(|(k, _)| k)?;
    let w = head[start..].trim_start_matches('.');
    if w.is_empty() {
        None
    } else {
        Some(fold_simple(&w.to_lowercase()))
    }
}

/// Folds Serbian Cyrillic currency words (`дин`, `евра`) the cheap way.
fn fold_simple(w: &str) -> String {
    if w.is_ascii() {
        return w.to_owned();
    }
    quail_text::fold::fold(w, &quail_text::fold::FoldOptions::KEY).into_owned()
}

/// All numeric runs of `text`, left to right.
#[allow(clippy::too_many_lines)]
pub(crate) fn scan(text: &str, dec: Dec, order: DateOrder) -> Vec<NumTok> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let (start_b, c) = chars[i];
        if !c.is_ascii_digit() {
            i += 1;
            continue;
        }
        // Extend over digits and separators; a space continues only before exactly three
        // digits that end the group.
        let mut j = i;
        let mut last_digit = i;
        while j < n {
            let (_, cj) = chars[j];
            if cj.is_ascii_digit() {
                last_digit = j;
                j += 1;
            } else if (is_sep(cj) || matches!(cj, '/' | '-'))
                && chars.get(j + 1).is_some_and(|(_, d)| d.is_ascii_digit())
            {
                j += 1;
            } else if is_group_space(cj) {
                let run = chars.get(j + 1..).map_or(0, |r| {
                    r.iter().take_while(|(_, d)| d.is_ascii_digit()).count()
                });
                let after = chars.get(j + 1 + run).map(|(_, d)| *d);
                let group_ok =
                    run == 3 && after.is_none_or(|d| !d.is_ascii_digit() && d != '/' && d != '-');
                // The run before the space is a group too (1-3 digits since the last
                // separator or space).
                let before = chars[i..j]
                    .iter()
                    .rev()
                    .take_while(|(_, d)| d.is_ascii_digit())
                    .count();
                if group_ok && (1..=3).contains(&before) {
                    j += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        let end_idx = last_digit + 1;
        let end_b = chars.get(end_idx).map_or(text.len(), |(b, _)| *b);
        let raw = &text[start_b..end_b];
        let prev = i.checked_sub(1).and_then(|k| chars.get(k)).map(|(_, c)| *c);
        let next = chars.get(end_idx).map(|(_, c)| *c);
        let neg = prev == Some('-')
            && i.checked_sub(2)
                .and_then(|k| chars.get(k))
                .is_none_or(|(_, c)| c.is_whitespace() || *c == '(');
        let glued_prev = prev.is_some_and(|p| p.is_alphabetic() || p == '/' || p == '_')
            || (prev == Some('-') && !neg);
        let after_next = chars.get(end_idx + 1).map(|(_, c)| *c);
        let glued_next = next.is_some_and(|x| {
            x.is_alphabetic()
                || (x == '/' && !unit_slash(text, end_b))
                || (x == '-' && after_next.is_some_and(char::is_alphanumeric))
        });
        let kind = if raw.contains(['/', '-']) {
            parse_date(raw, order).map(NumKind::Date)
        } else {
            let percent = text
                .get(end_b..)
                .is_some_and(|r| r.trim_start_matches(is_group_space).starts_with('%'));
            let date = parse_date(raw, order);
            if let Some(d) = date.filter(|_| raw.matches('.').count() >= 2) {
                Some(NumKind::Date(d))
            } else {
                parse_number(raw, dec).map(|mut v| {
                    if neg {
                        v.units = -v.units;
                    }
                    if percent {
                        NumKind::Percent(v)
                    } else {
                        NumKind::Number(v)
                    }
                })
            }
        };
        if let Some(kind) = kind {
            // Currency: a symbol or word right before, or right after.
            let before_sym = text
                .get(..start_b)
                .map(|h| h.trim_end_matches(is_group_space))
                .and_then(|h| h.trim_end_matches('-').chars().next_back())
                .and_then(currency_symbol);
            let after_text = text.get(end_b..).unwrap_or("");
            let after_sym = after_text
                .trim_start_matches(is_group_space)
                .chars()
                .next()
                .and_then(currency_symbol);
            let after_word = word_after(text, end_b).and_then(|(w, _)| currency_word(&w, false));
            let before_word = if glued_prev {
                None
            } else {
                word_before(text, if neg { start_b - 1 } else { start_b })
                    .and_then(|w| currency_word(&w, false))
            };
            let currency = before_sym.or(after_sym).or(after_word).or(before_word);
            let glued_by_letter = glued_next && after_word.is_none();
            // `No. 11129984`, `#4521`, `br. 12`, `ID: 773882`: an identifier.
            let after_label = text.get(..start_b).is_some_and(|h| {
                let h = h.trim_end_matches([' ', ':', '\u{a0}']);
                // Only the last few characters matter (the text before can be long).
                let tail_start = h.char_indices().rev().nth(12).map_or(0, |(k, _)| k);
                let h = &h[tail_start..];
                let lower = h.to_lowercase();
                h.ends_with('#')
                    || [
                        "no.", "no", "nr.", "nr", "br.", "br", "broj", "id", "number", "num",
                        "sec.", "sec", "section", "clause", "article", "art.", "cl.", "page", "p.",
                        "pp.", "str.", "clan", "član", "tacka", "tačka", "stav", "annex",
                        "schedule", "exhibit", "appendix", "version", "v", "rev", "box", "suite",
                        "floor", "room",
                    ]
                    .iter()
                    .any(|l| {
                        lower.ends_with(l)
                            && lower[..lower.len() - l.len()]
                                .chars()
                                .next_back()
                                .is_none_or(|c| !c.is_alphanumeric())
                    })
            });
            // `7, 8, 10,11,12, 13,14 i 15`: a list of numbers, not an amount.
            let listed = {
                let before = text.get(..start_b).unwrap_or("").trim_end_matches(' ');
                let after = text.get(end_b..).unwrap_or("");
                (before.ends_with(',')
                    && before[..before.len() - 1].ends_with(|c: char| c.is_ascii_digit()))
                    || (after.starts_with(',')
                        && after[1..]
                            .trim_start_matches(' ')
                            .starts_with(|c: char| c.is_ascii_digit())
                        && !after[1..].starts_with(|c: char| c.is_ascii_digit()))
            };
            let glued = (glued_prev && before_sym.is_none())
                || glued_by_letter
                || (after_label && currency.is_none())
                || (listed && currency.is_none());
            // Money is written with two decimals or with thousand groups (`12,00`, `1.200`),
            // not `10.1` (a clause) or `0.3`.
            let formatted = match kind {
                NumKind::Number(v) => {
                    v.scale == 2 || (v.scale == 0 && raw.contains([',', '.', ' ', '\u{a0}', '\'']))
                }
                _ => false,
            };
            out.push(NumTok {
                start: if neg { start_b - 1 } else { start_b },
                end: end_b,
                kind,
                currency,
                glued,
                formatted,
            });
        }
        i = end_idx.max(i + 1);
    }
    out
}

/// `85/h`, `85/sat`: a slash that introduces a unit, not a code.
fn unit_slash(text: &str, at: usize) -> bool {
    let Some(rest) = text.get(at + 1..) else {
        return false;
    };
    let w: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    crate::unit::normalize(&fold_simple(&w)).is_some()
}

/// `12.09.2023`, `12.9.23`, `11/04/16`, `2023-09-12`.
pub(crate) fn parse_date(raw: &str, order: DateOrder) -> Option<Date> {
    let raw = raw.trim_end_matches('.');
    let sep = raw.chars().find(|c| matches!(c, '.' | '/' | '-'))?;
    let parts: Vec<&str> = raw.split(sep).collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let v: Vec<u32> = parts.iter().map(|p| p.parse().unwrap_or(0)).collect();
    let (y, m, d) = if parts[0].len() == 4 {
        (v[0], v[1], v[2])
    } else {
        if parts[0].len() > 2 || parts[1].len() > 2 || !matches!(parts[2].len(), 2 | 4) {
            return None;
        }
        let y = if parts[2].len() == 2 {
            2000 + v[2]
        } else {
            v[2]
        };
        let (a, b) = (v[0], v[1]);
        let mdy =
            sep == '/' && (order == DateOrder::Mdy && b <= 31 && a <= 12 || a <= 12 && b > 12);
        if mdy { (y, a, b) } else { (y, b, a) }
    };
    if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(Date {
        y: i16::try_from(y).ok()?,
        m: u8::try_from(m).ok()?,
        d: u8::try_from(d).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str, dec: Dec) -> Option<(i64, u8)> {
        parse_number(s, dec).map(|v| (v.units, v.scale))
    }

    #[test]
    fn separators() {
        assert_eq!(n("1.200,50", Dec::Unknown), Some((120_050, 2)));
        assert_eq!(n("1,200.50", Dec::Unknown), Some((120_050, 2)));
        assert_eq!(n("1 200 000,00", Dec::Unknown), Some((120_000_000, 2)));
        assert_eq!(n("1.200", Dec::Unknown), Some((1200, 0)));
        assert_eq!(n("1,200", Dec::Comma), Some((1200, 3)));
        assert_eq!(n("1,200", Dec::Dot), Some((1200, 0)));
        assert_eq!(n("0,0450", Dec::Comma), Some((450, 4)));
        assert_eq!(n("12,5", Dec::Unknown), Some((125, 1)));
        assert_eq!(n("-15,00", Dec::Unknown), Some((-1500, 2)));
        assert_eq!(n("1.20.000", Dec::Unknown), None);
        assert_eq!(n("1'200.00", Dec::Unknown), Some((120_000, 2)));
    }

    #[test]
    fn scan_finds_amounts_currency_and_codes() {
        let t = scan(
            "Usluga A4 kom 2 x 1.200,00 RSD = 2.400,00 din.",
            Dec::Comma,
            DateOrder::Dmy,
        );
        let v: Vec<_> = t
            .iter()
            .map(|t| (t.number().map(|n| n.units), t.currency, t.glued))
            .collect();
        assert_eq!(
            v,
            [
                (Some(4), None, true),
                (Some(2), None, false),
                (Some(120_000), Some(*b"RSD"), false),
                (Some(240_000), Some(*b"RSD"), false),
            ]
        );
    }

    #[test]
    fn scan_dates_percent_and_symbols() {
        let t = scan(
            "$275.00 11/04/16 20% €1,200.50 85 EUR/h",
            Dec::Dot,
            DateOrder::Mdy,
        );
        assert_eq!(t[0].currency, Some(*b"USD"));
        assert_eq!(
            t[1].kind,
            NumKind::Date(Date {
                y: 2016,
                m: 11,
                d: 4
            })
        );
        assert!(matches!(t[2].kind, NumKind::Percent(_)));
        assert_eq!(t[3].currency, Some(*b"EUR"));
        assert_eq!(t[3].number().map(|n| n.units), Some(120_050));
        assert_eq!(t[4].currency, Some(*b"EUR"));
        let d = scan("od 12.09.2023. do 31.12.2023.", Dec::Comma, DateOrder::Dmy);
        assert_eq!(d.len(), 2);
        assert_eq!(
            d[0].kind,
            NumKind::Date(Date {
                y: 2023,
                m: 9,
                d: 12
            })
        );
    }

    #[test]
    fn unit_slash_is_not_a_code() {
        let t = scan("Senior consultant 85/h", Dec::Unknown, DateOrder::Dmy);
        assert!(!t[0].glued);
        let t = scan("broj 114/23", Dec::Unknown, DateOrder::Dmy);
        assert!(t.iter().all(|t| t.glued || t.number().is_none()));
    }
}

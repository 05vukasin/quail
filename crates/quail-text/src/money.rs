//! Numbers and amounts of money as business documents write them, read as exact decimals.
//!
//! One implementation of the separator rules for every crate that reads prices, totals or
//! quantities, so `1,200.50`, `1.200,50`, `1 200,50` and `1'200.50` mean the same thing
//! everywhere.
//!
//! ```
//! use quail_text::money::{self, Decimal, NumberFormat};
//!
//! assert_eq!(money::parse_amount("1.200,50"), Some(Decimal::new(120_050, 2)));
//! assert_eq!(money::parse_amount("1,200.50"), Some(Decimal::new(120_050, 2)));
//! assert_eq!(money::parse_amount("1 200 000"), Some(Decimal::new(1_200_000, 0)));
//!
//! let m = money::parse_money("(€1,200.50)").unwrap();
//! assert_eq!((m.units, m.scale, m.currency), (-120_050, 2, Some(*b"EUR")));
//! let m = money::parse_money("1.000,00 дин.").unwrap();
//! assert_eq!((m.units, m.currency), (100_000, Some(*b"RSD")));
//! let m = money::parse_money("USD 5 million").unwrap();
//! assert_eq!(m.units, 5_000_000);
//!
//! // Quantities and unit prices may carry three or four decimals; the document's decimal
//! // separator decides `1,500`.
//! let f = NumberFormat::QUANTITY.with_decimal(money::DecimalHint::Comma);
//! assert_eq!(money::parse_number("1,500", &f), Some(Decimal::new(1500, 3)));
//! assert_eq!(money::parse_number("1,500", &NumberFormat::QUANTITY), Some(Decimal::new(1500, 0)));
//! ```
//!
//! # Separator rules ([`parse_number`])
//!
//! - With both `.` and `,`, the later one is the decimal separator and the other one groups
//!   thousands (`1.200,50`, `1,200.50`). Mixing them inside the integer part is no number
//!   (`1,200.300,50`).
//! - With one kind only: once, followed by one or two digits, it is decimal (`1200,5`,
//!   `99.90`). Followed by exactly three digits, or repeated, it groups thousands (`1.200`,
//!   `1,200`, `1,200,000`), unless the [`DecimalHint`] says the document writes decimals with
//!   it and [`NumberFormat::max_decimals`] allows three (`1,500` kg). Followed by four
//!   digits it is decimal when the format allows four and the hint does not say the other
//!   separator is the decimal one (`0,0450` per kWh).
//! - Spaces (also no-break, narrow no-break and thin spaces) group thousands, and so does
//!   `'` when [`NumberFormat::apostrophe`] is on (`1'200.50`).
//! - Thousand groups must be well-formed: a leading group of one to three digits, then
//!   groups of exactly three (`1.20.000` is no number).
//! - At most [`MAX_INT_DIGITS`] digits before the decimal separator.
//!
//! # Amounts ([`parse_money`])
//!
//! A number with an optional currency before or after it, as a symbol (`€`, `$`, `£`, `¥`,
//! `₹`), a word in any case (`RSD`, `din.`, `дин.`, `dinara`, `euro`, `evra`, `dollars`,
//! `US$`, ...) or an ISO 4217 code in capitals ([`currency_code`]); an optional multiplier
//! word after it (`5 million`, `2 miliona`, `3 mlrd`); negative when written `-1.200,00`,
//! `1.200,00-`, `(1,200.00)` or `-$5`.

// Counting a separator in a short number; not worth a dependency.
#![allow(clippy::naive_bytecount)]

use quail_core::Money;

use crate::fold::{FoldOptions, fold};

/// Most digits accepted before the decimal separator.
pub const MAX_INT_DIGITS: usize = 15;

/// Which separator a document writes decimals with, learned from its unambiguous numbers
/// (see [`DecimalHint::of`]). It only decides cases the rules leave open (`1,500`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DecimalHint {
    /// Not known: `1.200` and `1,200` group thousands.
    #[default]
    Unknown,
    /// Decimal comma, `1.200,50`.
    Comma,
    /// Decimal point, `1,200.50`.
    Dot,
}

impl DecimalHint {
    /// The decimal separator `s` shows without doubt, if any: both separators
    /// (`1.200,50` → `Comma`), or one separator once followed by one or two digits
    /// (`12,5` → `Comma`, `99.90` → `Dot`).
    pub fn of(s: &str) -> Self {
        let b = s.trim().as_bytes();
        let (dot, comma) = (
            b.iter().rposition(|&c| c == b'.'),
            b.iter().rposition(|&c| c == b','),
        );
        let sep = match (dot, comma) {
            (Some(d), Some(c)) => Some(d.max(c)),
            (Some(x), None) | (None, Some(x)) => {
                let once = b.iter().filter(|&&c| c == b[x]).count() == 1;
                let after = b.len() - x - 1;
                (once && (1..=2).contains(&after)).then_some(x)
            }
            (None, None) => None,
        };
        match sep.map(|x| b[x]) {
            Some(b',') => Self::Comma,
            Some(b'.') => Self::Dot,
            _ => Self::Unknown,
        }
    }
}

/// How [`parse_number`] reads a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NumberFormat {
    /// The document's decimal separator, when known.
    pub decimal: DecimalHint,
    /// Most decimals accepted (`0..=4`; more are clamped to 4).
    pub max_decimals: u8,
    /// Accept a leading `-`, `−` (U+2212) or `+`.
    pub signed: bool,
    /// Accept `'` as a thousands separator (`1'200.50`).
    pub apostrophe: bool,
}

impl NumberFormat {
    /// Amounts of money: at most two decimals, no sign, no apostrophes, decimal separator
    /// unknown. This is how `quail-signals` reads amounts.
    pub const MONEY: Self = Self {
        decimal: DecimalHint::Unknown,
        max_decimals: 2,
        signed: false,
        apostrophe: false,
    };

    /// Quantities and unit prices: up to four decimals (`0,0450` per kWh), a sign
    /// (credits, discounts) and apostrophe groups.
    pub const QUANTITY: Self = Self {
        decimal: DecimalHint::Unknown,
        max_decimals: 4,
        signed: true,
        apostrophe: true,
    };

    /// The same format with the document's decimal separator.
    #[must_use]
    pub const fn with_decimal(self, decimal: DecimalHint) -> Self {
        Self { decimal, ..self }
    }
}

impl Default for NumberFormat {
    fn default() -> Self {
        Self::MONEY
    }
}

/// An exact decimal: `units × 10^-scale` (`Decimal::new(120_050, 2)` is 1200.50).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Decimal {
    /// The digits as an integer, sign included.
    pub units: i64,
    /// Number of decimal places in `units`.
    pub scale: u8,
}

impl Decimal {
    /// `units × 10^-scale`.
    pub const fn new(units: i64, scale: u8) -> Self {
        Self { units, scale }
    }

    /// The nearest `f64` (exact up to 2^53 units).
    #[allow(clippy::cast_precision_loss)]
    pub fn to_f64(self) -> f64 {
        self.units as f64 / 10f64.powi(i32::from(self.scale))
    }

    /// The same value with `currency`.
    pub const fn money(self, currency: Option<[u8; 3]>) -> Money {
        Money {
            units: self.units,
            scale: self.scale,
            currency,
        }
    }

    /// Whether it is below zero.
    pub const fn is_negative(self) -> bool {
        self.units < 0
    }

    /// The value multiplied by `10^pow` (`5 million`), `None` on overflow.
    pub fn scaled_up(self, pow: u32) -> Option<Self> {
        let units = self.units.checked_mul(10i64.checked_pow(pow)?)?;
        Some(Self { units, ..self })
    }
}

impl From<Money> for Decimal {
    fn from(m: Money) -> Self {
        Self::new(m.units, m.scale)
    }
}

impl std::fmt::Display for Decimal {
    /// Plain decimal with a point and every stored decimal: `1200.50`, `-0.0450`, `12`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let digits = self.units.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        let sign = if self.units < 0 { "-" } else { "" };
        if scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let (int, frac) = padded.split_at(padded.len() - scale);
        write!(f, "{sign}{int}.{frac}")
    }
}

/// Space-like characters that group thousands.
pub fn is_group_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}')
}

/// Reads a number as written (see the module rules). `None` for anything that is not one
/// number: empty text, letters, malformed groups, too many decimals or digits.
pub fn parse_number(s: &str, format: &NumberFormat) -> Option<Decimal> {
    let s = s.trim();
    let (neg, body) = if format.signed {
        match s.strip_prefix(['-', '\u{2212}']) {
            Some(rest) => (true, rest.trim_start()),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        }
    } else {
        (false, s)
    };
    let first = body.chars().next()?;
    let last = body.chars().next_back()?;
    if !first.is_ascii_digit() || !last.is_ascii_digit() {
        return None;
    }
    let grouping = |c: char| is_group_space(c) || (format.apostrophe && c == '\'');
    if !body
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || grouping(c))
    {
        return None;
    }
    let b = body.as_bytes();
    let max_dec = usize::from(format.max_decimals.min(4));
    let (decimal, group) = match (body.rfind('.'), body.rfind(',')) {
        (Some(d), Some(c)) => {
            if d > c {
                (Some(d), Some(','))
            } else {
                (Some(c), Some('.'))
            }
        }
        (Some(x), None) | (None, Some(x)) => {
            let sep = b[x];
            let count = b.iter().filter(|&&c| c == sep).count();
            let after = b.len() - x - 1;
            let hinted = matches!(
                (sep, format.decimal),
                (b',', DecimalHint::Comma) | (b'.', DecimalHint::Dot)
            );
            let other_hinted = matches!(
                (sep, format.decimal),
                (b',', DecimalHint::Dot) | (b'.', DecimalHint::Comma)
            );
            let decimal = count == 1
                && ((1..=2).contains(&after)
                    || (after == 3 && hinted && max_dec >= 3)
                    || (after == 4 && !other_hinted && max_dec >= 4));
            if decimal {
                (Some(x), None)
            } else {
                (None, Some(char::from(sep)))
            }
        }
        (None, None) => (None, None),
    };
    let (int, frac) = match decimal {
        Some(x) => (&body[..x], &body[x + 1..]),
        None => (body, ""),
    };
    if frac.len() > max_dec || !frac.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // The integer part: one to three digits, then groups of exactly three.
    let mut groups = int.split(|c: char| grouping(c) || Some(c) == group);
    let lead = groups.next()?;
    if lead.is_empty() || !lead.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut int_digits = lead.len();
    let mut grouped = false;
    for g in groups {
        if g.len() != 3 || !g.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        grouped = true;
        int_digits += 3;
    }
    if (grouped && lead.len() > 3) || int_digits > MAX_INT_DIGITS {
        return None;
    }
    let mut units: i64 = 0;
    for c in int.bytes().filter(u8::is_ascii_digit).chain(frac.bytes()) {
        units = units.checked_mul(10)?.checked_add(i64::from(c - b'0'))?;
    }
    Some(Decimal {
        units: if neg { -units } else { units },
        scale: u8::try_from(frac.len()).ok()?,
    })
}

/// [`parse_number`] with [`NumberFormat::MONEY`]: `1.200,50`, `1,200.50`, `1 200,50`,
/// `1200`, never three decimals or a sign.
pub fn parse_amount(s: &str) -> Option<Decimal> {
    parse_number(s, &NumberFormat::MONEY)
}

/// Reads an amount with its currency, multiplier and sign (see the module docs) using
/// [`NumberFormat::MONEY`] for the number.
pub fn parse_money(s: &str) -> Option<Money> {
    parse_money_with(s, &NumberFormat::MONEY)
}

/// [`parse_money`] with a number format (its `signed` flag is ignored: signs and
/// parentheses are always read).
pub fn parse_money_with(s: &str, format: &NumberFormat) -> Option<Money> {
    let format = NumberFormat {
        signed: false,
        ..*format
    };
    let mut s = s.trim();
    let mut neg = false;
    if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        neg = true;
        s = inner.trim();
    }
    let mut currency = None;
    let mut take_sign = |s: &mut &str| {
        if let Some(rest) = s.strip_prefix(['-', '\u{2212}']) {
            neg = !neg;
            *s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix('+') {
            *s = rest.trim_start();
        }
    };
    take_sign(&mut s);
    // A currency before the number.
    let head_len = s.find(|c: char| c.is_ascii_digit() || c == '-' || c == '\u{2212}')?;
    let head = s[..head_len].trim();
    if !head.is_empty() {
        currency = Some(currency_token(head)?);
    }
    s = &s[head_len..];
    take_sign(&mut s);
    // A trailing minus, multiplier and currency after it.
    let last_digit = s.rfind(|c: char| c.is_ascii_digit())?;
    let (num, mut tail) = s.split_at(last_digit + 1);
    if let Some(rest) = tail.strip_prefix(['-', '\u{2212}']) {
        neg = !neg;
        tail = rest;
    }
    let mut pow = 0;
    for word in tail.split_whitespace() {
        if let Some(p) = (pow == 0).then(|| multiplier(&key(word))).flatten() {
            pow = p;
        } else if currency.is_none() {
            currency = Some(currency_token(word)?);
        } else {
            return None;
        }
    }
    let mut value = parse_number(num, &format)?;
    if pow > 0 {
        value = value.scaled_up(pow)?;
    }
    if neg {
        value.units = -value.units;
    }
    Some(value.money(currency))
}

fn key(word: &str) -> String {
    fold(word, &FoldOptions::KEY).into_owned()
}

/// A currency written as one token: a symbol, a word (`din.`, `US$`) or a code in capitals.
fn currency_token(t: &str) -> Option<[u8; 3]> {
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && let Some(cur) = currency_symbol(c)
    {
        return Some(cur);
    }
    currency_word(key(t).trim_end_matches('.')).or_else(|| currency_code(t))
}

/// Currency words (folded keys) that may be written in any case.
const WORDS: &[(&str, [u8; 3])] = &[
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
    ("kn", *b"HRK"),
    ("kuna", *b"HRK"),
];

/// ISO 4217 codes recognised only as written in capitals, because some are words too
/// (`TRY`, `RUB`).
const CODES: &[&str] = &[
    "AUD", "BAM", "BGN", "CAD", "CNY", "CZK", "DKK", "HKD", "HRK", "HUF", "INR", "JPY", "MKD",
    "NOK", "NZD", "PLN", "RON", "RUB", "SEK", "SGD", "TRY", "UAH", "ZAR",
];

/// Codes in [`CODES`] that are also English words, never read in lowercase.
const WORD_CODES: &[&str] = &["RUB", "TRY"];

const SYMBOLS: &[(char, [u8; 3])] = &[
    ('€', *b"EUR"),
    ('$', *b"USD"),
    ('£', *b"GBP"),
    ('¥', *b"JPY"),
    ('₹', *b"INR"),
];

/// Multiplier words (folded) and their power of ten.
const MULTIPLIERS: &[(&str, u32)] = &[
    ("thousand", 3),
    ("hiljada", 3),
    ("hiljade", 3),
    ("hiljadu", 3),
    ("million", 6),
    ("millions", 6),
    ("milion", 6),
    ("miliona", 6),
    ("mln", 6),
    ("billion", 9),
    ("milijarda", 9),
    ("milijardi", 9),
    ("milijarde", 9),
];

/// The currency of a folded word written in any case (`rsd`, `din`, `dinara`, `evra`,
/// `euro`, `us$`, `dollars`, `kn`, ...). Fold with [`FoldOptions::KEY`] first, so `дин`
/// and `RSD` are found; a trailing `.` is not stripped (`din.` is the caller's to trim).
pub fn currency_word(key: &str) -> Option<[u8; 3]> {
    WORDS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c)
}

/// The currency of an ISO 4217 code written in capitals (`AUD`, `CZK`, `TRY`, ...): the
/// codes that are not already [`currency_word`]s. Lowercase is not accepted (`try`).
pub fn currency_code(text: &str) -> Option<[u8; 3]> {
    CODES.iter().find(|c| **c == text).map(|c| {
        let b = c.as_bytes();
        [b[0], b[1], b[2]]
    })
}

/// [`currency_code`] for a code in any case, except codes that are also English words
/// (`rub`, `try`).
pub fn currency_code_any_case(text: &str) -> Option<[u8; 3]> {
    if text.len() != 3 {
        return None;
    }
    let upper = text.to_ascii_uppercase();
    if WORD_CODES.contains(&upper.as_str()) {
        return None;
    }
    currency_code(&upper)
}

/// The currency of a symbol (`€ $ £ ¥ ₹`).
pub fn currency_symbol(c: char) -> Option<[u8; 3]> {
    SYMBOLS.iter().find(|(s, _)| *s == c).map(|(_, cur)| *cur)
}

/// The power of ten of a folded multiplier word (`thousand` 3, `million`/`miliona`/`mln`
/// 6, `billion`/`milijardi` 9).
pub fn multiplier(key: &str) -> Option<u32> {
    MULTIPLIERS.iter().find(|(w, _)| *w == key).map(|(_, p)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Option<(i64, u8)> {
        parse_amount(s).map(|d| (d.units, d.scale))
    }

    fn q(s: &str, hint: DecimalHint) -> Option<(i64, u8)> {
        parse_number(s, &NumberFormat::QUANTITY.with_decimal(hint)).map(|d| (d.units, d.scale))
    }

    fn m(s: &str) -> Option<(i64, u8, Option<[u8; 3]>)> {
        parse_money(s).map(|m| (m.units, m.scale, m.currency))
    }

    #[test]
    fn money_format_matches_the_signals_rules() {
        assert_eq!(a("1.200,50"), Some((120_050, 2)));
        assert_eq!(a("1,200.50"), Some((120_050, 2)));
        assert_eq!(a("1 200,50"), Some((120_050, 2)));
        assert_eq!(a("1 200 000"), Some((1_200_000, 0)));
        assert_eq!(a("1.200"), Some((1200, 0)));
        assert_eq!(a("1,200"), Some((1200, 0)));
        assert_eq!(a("1,200,000"), Some((1_200_000, 0)));
        assert_eq!(a("1.200.300,50"), Some((120_030_050, 2)));
        assert_eq!(a("99.9"), Some((999, 1)));
        assert_eq!(a("1200,5"), Some((12005, 1)));
        assert_eq!(a("1.20.000"), None);
        assert_eq!(a("1.200.50"), None, "three-digit groups only");
        assert_eq!(a("1,200.300,50"), None, "mixed group separators");
        assert_eq!(a("12345.678"), None);
        assert_eq!(a("1.2345"), None, "four decimals");
        assert_eq!(a("-5"), None, "no sign");
        assert_eq!(a("1'200"), None, "no apostrophes");
        assert_eq!(a("9999999999999999"), None);
        assert_eq!(a("999999999999999.99"), Some((99_999_999_999_999_999, 2)));
        assert_eq!(a(""), None);
        assert_eq!(a("1,"), None);
        assert_eq!(a(",5"), None);
        assert_eq!(a("1 20"), None);
        assert_eq!(a("1\u{a0}200,00"), Some((120_000, 2)));
    }

    #[test]
    fn quantity_format_uses_the_hint() {
        assert_eq!(q("1,200", DecimalHint::Comma), Some((1200, 3)));
        assert_eq!(q("1,200", DecimalHint::Dot), Some((1200, 0)));
        assert_eq!(q("1,200", DecimalHint::Unknown), Some((1200, 0)));
        assert_eq!(q("0,0450", DecimalHint::Comma), Some((450, 4)));
        assert_eq!(q("0,0450", DecimalHint::Dot), None);
        assert_eq!(q("-15,00", DecimalHint::Unknown), Some((-1500, 2)));
        assert_eq!(q("\u{2212} 15", DecimalHint::Unknown), Some((-15, 0)));
        assert_eq!(q("+15", DecimalHint::Unknown), Some((15, 0)));
        assert_eq!(q("1'200.00", DecimalHint::Unknown), Some((120_000, 2)));
        assert_eq!(q("1.23456", DecimalHint::Unknown), None);
    }

    #[test]
    fn hint_of_unambiguous_numbers() {
        assert_eq!(DecimalHint::of("1.200,50"), DecimalHint::Comma);
        assert_eq!(DecimalHint::of("1,200.50"), DecimalHint::Dot);
        assert_eq!(DecimalHint::of("12,5"), DecimalHint::Comma);
        assert_eq!(DecimalHint::of("1.200"), DecimalHint::Unknown);
        assert_eq!(DecimalHint::of("1,200,000"), DecimalHint::Unknown);
        assert_eq!(DecimalHint::of("12"), DecimalHint::Unknown);
    }

    #[test]
    fn amounts_with_currency_sign_and_multiplier() {
        assert_eq!(m("€1,200.50"), Some((120_050, 2, Some(*b"EUR"))));
        assert_eq!(m("€ 1.200,50"), Some((120_050, 2, Some(*b"EUR"))));
        assert_eq!(m("RSD 1 200,50"), Some((120_050, 2, Some(*b"RSD"))));
        assert_eq!(m("1.200,50 rsd"), Some((120_050, 2, Some(*b"RSD"))));
        assert_eq!(m("1.000,00 дин."), Some((100_000, 2, Some(*b"RSD"))));
        assert_eq!(m("1.000,00 динара"), Some((100_000, 2, Some(*b"RSD"))));
        assert_eq!(m("US$ 25"), Some((25, 0, Some(*b"USD"))));
        assert_eq!(m("CZK 300"), Some((300, 0, Some(*b"CZK"))));
        assert_eq!(m("300 try"), None);
        assert_eq!(m("USD 5 million"), Some((5_000_000, 0, Some(*b"USD"))));
        assert_eq!(m("2,5 miliona evra"), Some((25_000_000, 1, Some(*b"EUR"))));
        assert_eq!(m("(1,200.00)"), Some((-120_000, 2, None)));
        assert_eq!(m("($1,200.00)"), Some((-120_000, 2, Some(*b"USD"))));
        assert_eq!(m("-$5"), Some((-5, 0, Some(*b"USD"))));
        assert_eq!(m("$-5"), Some((-5, 0, Some(*b"USD"))));
        assert_eq!(m("1.200,00-"), Some((-120_000, 2, None)));
        assert_eq!(m("−15,00 EUR"), Some((-1500, 2, Some(*b"EUR"))));
        assert_eq!(m("1200"), Some((1200, 0, None)));
        assert_eq!(m("EUR"), None);
        assert_eq!(m("about 1200"), None);
        assert_eq!(m("1200 EUR USD"), None);
        assert_eq!(m("12 apples"), None);
        assert_eq!(m("9999999999999 billion"), None, "overflow");
    }

    #[test]
    fn currencies() {
        assert_eq!(currency_word("dinara"), Some(*b"RSD"));
        assert_eq!(currency_word("RSD"), None, "keys are folded");
        assert_eq!(currency_code("CAD"), Some(*b"CAD"));
        assert_eq!(currency_code("cad"), None);
        assert_eq!(currency_code_any_case("cad"), Some(*b"CAD"));
        assert_eq!(currency_code_any_case("try"), None);
        assert_eq!(currency_code_any_case("TRY"), None);
        assert_eq!(currency_symbol('£'), Some(*b"GBP"));
        assert_eq!(multiplier("miliona"), Some(6));
    }

    #[test]
    fn decimal_display_and_f64() {
        assert_eq!(Decimal::new(120_050, 2).to_string(), "1200.50");
        assert_eq!(Decimal::new(-450, 4).to_string(), "-0.0450");
        assert_eq!(Decimal::new(12, 0).to_string(), "12");
        assert_eq!(Decimal::new(5, 2).to_string(), "0.05");
        assert!((Decimal::new(120_050, 2).to_f64() - 1200.5).abs() < 1e-9);
        assert_eq!(
            Decimal::new(5, 0).scaled_up(6),
            Some(Decimal::new(5_000_000, 0))
        );
        assert_eq!(Decimal::new(i64::MAX, 0).scaled_up(1), None);
    }

    #[test]
    fn hostile_input_never_panics() {
        for s in [
            "",
            " ",
            "(",
            ")",
            "()",
            "-",
            "$",
            "€€",
            "1..2",
            ",,",
            "1,,2",
            "١٢٣",
            "1\u{a0}",
            "(-)",
            "--5",
            "5--",
            "$ $ 5",
            "ДИН",
            "1 2 3 4",
            "999,999,999,999,999,999",
        ] {
            let _ = parse_money(s);
            let _ = parse_number(s, &NumberFormat::QUANTITY);
            let _ = DecimalHint::of(s);
        }
    }
}

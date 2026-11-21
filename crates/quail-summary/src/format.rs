//! Language-neutral renderings of values: ISO dates, `EUR 4,200.00`, short hex, clipped text.

use std::fmt::Write as _;

use quail_core::{Date, Money};

/// `YYYY-MM-DD`.
pub fn date(d: Date) -> String {
    format!("{:04}-{:02}-{:02}", d.y, d.m, d.d)
}

/// `EUR 4,200.00`: ISO code (or `?` when unknown), thousands grouped by commas, the written
/// number of decimals.
pub fn money(m: Money) -> String {
    let cur = m
        .currency
        .and_then(|c| std::str::from_utf8(&c).ok().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned());
    format!("{cur} {}", amount(m))
}

/// The number of a [`Money`] without currency: `4,200.00`.
pub fn amount(m: Money) -> String {
    let neg = m.units < 0;
    let digits = m.units.unsigned_abs().to_string();
    let scale = usize::from(m.scale);
    let (int, frac) = if digits.len() > scale {
        digits.split_at(digits.len() - scale)
    } else {
        ("0", digits.as_str())
    };
    let mut out = String::with_capacity(digits.len() + 8);
    if neg {
        out.push('-');
    }
    let n = int.len();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if scale > 0 {
        out.push('.');
        for _ in frac.len()..scale {
            out.push('0');
        }
        out.push_str(frac);
    }
    out
}

/// The amount as a float, for ordering only.
#[allow(clippy::cast_precision_loss)] // ordering of amounts, not arithmetic
pub fn money_value(m: Money) -> f64 {
    m.units as f64 / 10f64.powi(i32::from(m.scale))
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// One line: control characters and line breaks become spaces, runs of whitespace one space.
pub fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// At most `max` characters of `s` (on one line), cut at a word boundary when one is near,
/// with `…` when cut.
pub fn clip(s: &str, max: usize) -> String {
    let s = one_line(s);
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i * 3 >= cut.len() * 2 => &cut[..i],
        _ => &cut,
    };
    format!("{}…", cut.trim_end_matches([' ', ',', ';', ':']))
}

/// Upper-case first letter.
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_and_dates() {
        let m = Money {
            units: 420_050,
            scale: 2,
            currency: Some(*b"EUR"),
        };
        assert_eq!(money(m), "EUR 4,200.50");
        let m = Money {
            units: -5,
            scale: 2,
            currency: None,
        };
        assert_eq!(money(m), "? -0.05");
        let m = Money {
            units: 1_234_567,
            scale: 0,
            currency: Some(*b"RSD"),
        };
        assert_eq!(money(m), "RSD 1,234,567");
        assert_eq!(
            date(Date {
                y: 2023,
                m: 9,
                d: 2
            }),
            "2023-09-02"
        );
    }

    #[test]
    fn clipping() {
        assert_eq!(clip("a\n b\tc", 10), "a b c");
        assert_eq!(clip("Master Services Agreement", 17), "Master Services…");
        assert_eq!(clip("Master Services Agreement", 15), "Master Service…");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
        assert_eq!(capitalize("ugovor"), "Ugovor");
    }
}

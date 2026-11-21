//! Durations as contracts write them: `two (2) years`, `2 (two) years`, `thirty (30) days`,
//! `24 months`, `a two-year period`, `one year`, `ninety (90) calendar days`, `5.5 months`.

use crate::tokens::Stream;

/// Unit of a duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Calendar or business days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
    /// Years.
    Year,
}

impl Unit {
    fn of(key: &str) -> Option<Self> {
        Some(match key {
            "day" | "days" => Self::Day,
            "week" | "weeks" => Self::Week,
            "month" | "months" => Self::Month,
            "year" | "years" => Self::Year,
            _ => return None,
        })
    }

    /// English name, singular or plural.
    pub fn name(self, one: bool) -> &'static str {
        match (self, one) {
            (Self::Day, true) => "day",
            (Self::Day, false) => "days",
            (Self::Week, true) => "week",
            (Self::Week, false) => "weeks",
            (Self::Month, true) => "month",
            (Self::Month, false) => "months",
            (Self::Year, true) => "year",
            (Self::Year, false) => "years",
        }
    }

    /// ISO 8601 designator.
    pub fn iso(self) -> char {
        match self {
            Self::Day => 'D',
            Self::Week => 'W',
            Self::Month => 'M',
            Self::Year => 'Y',
        }
    }
}

/// A duration found at tokens `a..b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Duration {
    /// Amount, in tenths (`55.5` months is 555).
    pub tenths: u32,
    /// Unit.
    pub unit: Unit,
    /// First token.
    pub a: usize,
    /// One past the unit token.
    pub b: usize,
}

impl Duration {
    fn amount(&self) -> String {
        if self.tenths.is_multiple_of(10) {
            (self.tenths / 10).to_string()
        } else {
            format!("{}.{}", self.tenths / 10, self.tenths % 10)
        }
    }

    /// `2 years`, `18 months`, `1 year`.
    pub fn text(&self) -> String {
        format!("{} {}", self.amount(), self.unit.name(self.tenths == 10))
    }

    /// ISO 8601 duration: `P2Y`, `P18M`, `P30D`, `P2W` (a fraction keeps its decimal).
    pub fn iso(&self) -> String {
        format!("P{}{}", self.amount(), self.unit.iso())
    }
}

/// Value of a number word (`two`, `twenty`, `eighteen`).
fn word_number(key: &str) -> Option<u32> {
    Some(match key {
        "one" | "a" | "an" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "thirteen" => 13,
        "fourteen" => 14,
        "fifteen" => 15,
        "sixteen" => 16,
        "seventeen" => 17,
        "eighteen" => 18,
        "nineteen" => 19,
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        "hundred" => 100,
        _ => return None,
    })
}

fn digits(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 4 || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse().ok()
}

/// A number at token `i` in tenths and one past its last token: digits (`2`, `5.5`), a
/// number word (`two`, `twenty-four`, `twenty four`) or `a`/`an`/`one`.
fn number_at(s: &Stream, i: usize) -> Option<(u32, usize)> {
    let k = s.key(i);
    if let Some(n) = digits(k) {
        // "5.5": a dot between digits, no spaces.
        if s.is_punct(i + 1, '.')
            && let Some(f) = digits(s.key(i + 2))
            && s.key(i + 2).len() == 1
            && tight(s, i, i + 2)
        {
            return Some((n * 10 + f, i + 3));
        }
        return Some((n * 10, i + 1));
    }
    let n = word_number(k)?;
    if (20..100).contains(&n) && n % 10 == 0 {
        // "twenty-four", "twenty four".
        let j = if s.is_punct(i + 1, '-') { i + 2 } else { i + 1 };
        if let Some(u) = word_number(s.key(j)).filter(|u| (1..10).contains(u)) {
            return Some(((n + u) * 10, j + 1));
        }
    }
    Some((n * 10, i + 1))
}

/// Tokens `a..=b` follow one another without spaces.
fn tight(s: &Stream, a: usize, b: usize) -> bool {
    (a..b).all(|k| match (s.toks.get(k), s.toks.get(k + 1)) {
        (Some(x), Some(y)) => x.page == y.page && x.span.end == y.span.start,
        _ => false,
    })
}

/// A parenthesised number after the first one: `(2)`, `(two)`. One past `)`.
fn echo_at(s: &Stream, i: usize) -> Option<usize> {
    if !s.is_punct(i, '(') {
        return None;
    }
    let (_, j) = number_at(s, i + 1)?;
    s.is_punct(j, ')').then_some(j + 1)
}

/// A duration starting at token `i`.
pub fn duration_at(s: &Stream, i: usize) -> Option<Duration> {
    if !s.is_word(i) {
        return None;
    }
    let (mut n, mut j) = number_at(s, i)?;
    // "a" and "an" count only before a unit: "a period of" is no duration.
    let article = matches!(s.key(i), "a" | "an");
    if let Some(e) = echo_at(s, j) {
        // "two (2)": the digits win when the two disagree.
        if let Some((m, _)) = number_at(s, j + 1)
            && digits(s.key(j + 1)).is_some()
        {
            n = m;
        }
        j = e;
    }
    // "two-year", "2-month".
    if s.is_punct(j, '-') {
        j += 1;
    }
    if matches!(
        s.key(j),
        "calendar" | "business" | "working" | "consecutive" | "full"
    ) {
        j += 1;
    }
    let unit = Unit::of(s.key(j))?;
    if n == 0 || article && j != i + 1 {
        return None;
    }
    Some(Duration {
        tenths: n,
        unit,
        a: i,
        b: j + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Document, Page, PageKind};

    fn first(text: &str) -> Option<(String, String)> {
        let d = Document {
            pages: vec![Page {
                text: text.to_string(),
                ..Page::new(0, PageKind::Physical)
            }],
        };
        let (s, _) = Stream::read(&d, 1 << 20, || false);
        (0..s.len()).find_map(|i| duration_at(&s, i).map(|d| (d.text(), d.iso())))
    }

    #[test]
    fn spelled_and_echoed() {
        let t = |x: &str| first(x).map(|p| p.0);
        assert_eq!(t("for two (2) years from"), Some("2 years".into()));
        assert_eq!(t("for 2 (two) years"), Some("2 years".into()));
        assert_eq!(t("thirty (30) calendar days"), Some("30 days".into()));
        assert_eq!(t("a two-year period"), Some("2 years".into()));
        assert_eq!(t("twenty-four (24) months"), Some("24 months".into()));
        assert_eq!(t("one year"), Some("1 year".into()));
        assert_eq!(t("a year"), Some("1 year".into()));
        assert_eq!(t("55.5 months"), Some("55.5 months".into()));
        assert_eq!(t("three (5) years"), Some("5 years".into()));
        assert_eq!(t("a period of three years"), Some("3 years".into()));
        assert_eq!(t("in 2019 the years"), None);
        assert_eq!(first("18 months").unwrap().1, "P18M");
        assert_eq!(first("5.5 months").unwrap().1, "P5.5M");
    }
}

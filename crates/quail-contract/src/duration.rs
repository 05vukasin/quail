//! Durations in Serbian and English, on folded tokens: `30 dana`, `trideset (30) dana`,
//! `12 (dvanaest) meseci`, `godinu dana`, `2 godine`, `30 radnih dana`, `two (2) years`,
//! `a two-year`, `24 months`, `4 hours`, `30 calendar days`.

use quail_payment::lex::Tok;
use quail_text::token::TokenKind;

/// Unit of a duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days (calendar or business).
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
            "minut" | "minuta" | "minute" | "minuti" | "minutes" | "min" => Self::Minute,
            "sat" | "sata" | "sati" | "satu" | "casova" | "hour" | "hours" | "h" => Self::Hour,
            "dan" | "dana" | "danu" | "dani" | "day" | "days" => Self::Day,
            "nedelja" | "nedelje" | "nedelju" | "sedmica" | "sedmice" | "sedmicu" | "week"
            | "weeks" => Self::Week,
            "mesec" | "meseca" | "meseci" | "mesecu" | "mjesec" | "mjeseca" | "mjeseci"
            | "month" | "months" => Self::Month,
            "godina" | "godine" | "godinu" | "godini" | "godinom" | "year" | "years" => Self::Year,
            _ => return None,
        })
    }
}

/// A duration found at tokens `first..=last`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dur {
    /// How many units.
    pub n: u32,
    /// The unit.
    pub unit: Unit,
    /// Business (working) days.
    pub business: bool,
    /// First token.
    pub first: usize,
    /// Last token.
    pub last: usize,
}

impl Dur {
    /// ISO 8601 duration: `P30D`, `P2W`, `P12M`, `P2Y`, `PT4H`, `PT30M`.
    pub fn iso(&self) -> String {
        let n = self.n;
        match self.unit {
            Unit::Minute => format!("PT{n}M"),
            Unit::Hour => format!("PT{n}H"),
            Unit::Day => format!("P{n}D"),
            Unit::Week => format!("P{n}W"),
            Unit::Month => format!("P{n}M"),
            Unit::Year => format!("P{n}Y"),
        }
    }

    /// Days, for day and week units.
    pub fn days(&self) -> Option<u32> {
        match self.unit {
            Unit::Day => Some(self.n),
            Unit::Week => self.n.checked_mul(7),
            _ => None,
        }
    }

    /// Months, for month and year units.
    pub fn months(&self) -> Option<u32> {
        match self.unit {
            Unit::Month => Some(self.n),
            Unit::Year => self.n.checked_mul(12),
            _ => None,
        }
    }

    /// Hours, for minute (rounded up), hour and day units.
    pub fn hours(&self) -> Option<u32> {
        match self.unit {
            Unit::Minute => Some(self.n.div_ceil(60)),
            Unit::Hour => Some(self.n),
            Unit::Day => self.n.checked_mul(24),
            _ => None,
        }
    }
}

/// Value of a number word (folded): `dva` 2, `trideset` 30, `twelve` 12.
fn word_value(key: &str) -> Option<u32> {
    Some(match key {
        "jedan" | "jedna" | "jednu" | "jedne" | "jednog" | "jednom" | "one" => 1,
        "dva" | "dve" | "dvije" | "two" => 2,
        "tri" | "three" => 3,
        "cetiri" | "four" => 4,
        "pet" | "five" => 5,
        "sest" | "six" => 6,
        "sedam" | "seven" => 7,
        "osam" | "eight" => 8,
        "devet" | "nine" => 9,
        "deset" | "ten" => 10,
        "jedanaest" | "eleven" => 11,
        "dvanaest" | "twelve" => 12,
        "petnaest" | "fifteen" => 15,
        "osamnaest" | "eighteen" => 18,
        "dvadeset" | "twenty" => 20,
        "trideset" | "thirty" => 30,
        "cetrdeset" | "forty" => 40,
        "pedeset" | "fifty" => 50,
        "sezdeset" | "sixty" => 60,
        "devedeset" | "ninety" => 90,
        _ => return None,
    })
}

/// A number at token `i` (digits or words, `twenty-four`, `dvadeset cetiri`): its value
/// and the token after it.
fn number_at(toks: &[Tok], i: usize) -> Option<(u32, usize)> {
    let t = toks.get(i)?;
    if t.kind == TokenKind::Number {
        if t.key.len() > 4 || !t.key.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return Some((t.key.parse().ok()?, i + 1));
    }
    if t.kind != TokenKind::Word {
        return None;
    }
    let v = word_value(&t.key)?;
    if v >= 20 && v % 10 == 0 {
        // `twenty-four`, `twenty four`, `dvadeset cetiri`.
        let mut j = i + 1;
        if toks.get(j).is_some_and(|t| t.key == "-") {
            j += 1;
        }
        if let Some(u) = toks.get(j).and_then(|t| word_value(&t.key))
            && u < 10
        {
            return Some((v + u, j + 1));
        }
    }
    Some((v, i + 1))
}

/// Adjectives between the number and the unit.
fn adjective(key: &str) -> Option<bool> {
    match key {
        "radnih" | "radna" | "radni" | "business" | "working" => Some(true),
        "kalendarskih" | "kalendarska" | "kalendarski" | "calendar" | "consecutive"
        | "uzastopnih" | "punih" | "full" => Some(false),
        _ => None,
    }
}

/// The duration that starts at token `i`.
pub fn duration_at(toks: &[Tok], i: usize) -> Option<Dur> {
    // `godinu dana`, `mesec dana`: one year, one month.
    if let Some(u) = toks.get(i).and_then(|t| Unit::of(&t.key))
        && matches!(u, Unit::Year | Unit::Month)
        && toks.get(i + 1).is_some_and(|t| t.key == "dana")
        && matches!(toks[i].key.as_str(), "godinu" | "godina" | "mesec" | "mjesec")
    {
        return Some(Dur {
            n: 1,
            unit: u,
            business: false,
            first: i,
            last: i + 1,
        });
    }
    let (n, mut j) = number_at(toks, i)?;
    // `(30)`, `(trideset)`.
    if toks.get(j).is_some_and(|t| t.key == "(") {
        let mut k = j + 1;
        if number_at(toks, k).is_some() {
            k = number_at(toks, k)?.1;
            if toks.get(k).is_some_and(|t| t.key == ")") {
                j = k + 1;
            }
        }
    }
    if toks.get(j).is_some_and(|t| t.key == "-") {
        j += 1;
    }
    let mut business = false;
    if let Some(b) = toks.get(j).and_then(|t| adjective(&t.key)) {
        business = b;
        j += 1;
    }
    let unit = Unit::of(&toks.get(j)?.key)?;
    if n == 0 {
        return None;
    }
    let mut last = j;
    // `godinu dana` after a number word: `jednu godinu dana`.
    if matches!(unit, Unit::Year | Unit::Month) && toks.get(j + 1).is_some_and(|t| t.key == "dana")
    {
        last = j + 1;
    }
    Some(Dur {
        n,
        unit,
        business,
        first: i,
        last,
    })
}

/// The first duration in tokens `from..to`, of one of `units` (all when empty).
pub fn first_duration(toks: &[Tok], from: usize, to: usize, units: &[Unit]) -> Option<Dur> {
    (from..to.min(toks.len()))
        .filter_map(|i| duration_at(toks, i))
        .find(|d| units.is_empty() || units.contains(&d.unit))
}

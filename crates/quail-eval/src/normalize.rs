//! Value normalizers and the comparison of a gold label with a predicted value.

use std::borrow::Cow;

use quail_text::fold::{FoldOptions, fold};
use serde::{Deserialize, Serialize};

use crate::gold::{Label, MatchMode};

/// How values of a task are brought to a canonical form before comparing them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalizer {
    /// No task-specific normalization (`normalized` matching then uses `text`).
    #[default]
    None,
    /// Lowercase; every run of non-alphanumeric characters becomes one space; trimmed.
    Text,
    /// Folded (Serbian Cyrillic → Latin, no diacritics), `text`, then a leading `the` and
    /// leading forms (`jp`, `jkp`) and trailing legal-form tokens (`inc`, `llc`, `ltd`,
    /// `gmbh`, `d o o`, `доо`, `а д`, ...) are dropped.
    Company,
    /// A date in ISO form `YYYY-MM-DD` when it parses (ISO, `D.M.YYYY`, US `M/D/YYYY`,
    /// `Month D, YYYY`, `D Month YYYY`, `Dth day of Month, YYYY`; English and Serbian month
    /// names, Latin and Cyrillic, nominative and genitive: `1. јула 2026. године`), else
    /// `text`.
    Date,
    /// Letters and digits only, uppercase; Cyrillic letters that look like Latin ones become
    /// those, the rest is transliterated (IBANs, contract numbers, hashes; `ОС 90` = `OC 90`).
    Identifier,
    /// A Serbian bank account `BBB-N-KK` as its 18 digits, the middle part zero-padded to 13
    /// (`840-26650-09` = `840-0000000026650-09`); anything else as `identifier`.
    BankAccount,
    /// A decimal number (thousands separators removed, `,` or `.` as decimal mark), else `text`.
    Number,
    /// `true` / `false` (`yes`, `1`, `da`, ... accepted), else `text`.
    Bool,
    /// Primary language subtag, lowercase (`en-US` → `en`); a few English language names
    /// map to their code (`Serbian` → `sr`).
    Language,
}

impl Normalizer {
    /// Canonical form of `s`.
    pub fn apply(self, s: &str) -> String {
        match self {
            Self::None => s.trim().to_string(),
            Self::Text => text(s),
            Self::Company => company(s),
            Self::Date => parse_date(s).unwrap_or_else(|| text(s)),
            Self::Identifier => identifier(s),
            Self::BankAccount => bank_account(s).unwrap_or_else(|| identifier(s)),
            Self::Number => parse_number(s).map_or_else(|| text(s), |n| format!("{n}")),
            Self::Bool => parse_bool(s).map_or_else(|| text(s), |b| b.to_string()),
            Self::Language => language(s),
        }
    }

    /// The normalizer used by `normalized`, `contains` and `any_of` matching.
    fn effective(self) -> Self {
        if self == Self::None { Self::Text } else { self }
    }
}

/// Whether `pred` matches `label` under the task normalizer `norm`.
pub fn label_matches(label: &Label, norm: Normalizer, pred: &str) -> bool {
    let eff = norm.effective();
    label.alternatives().iter().any(|gold| match label.mode {
        MatchMode::Exact => gold.trim() == pred.trim(),
        MatchMode::Normalized | MatchMode::AnyOf => {
            let g = eff.apply(gold);
            !g.is_empty() && g == eff.apply(pred)
        }
        MatchMode::Contains => contains(&eff.apply(gold), &eff.apply(pred)),
        MatchMode::NumericTol => match (parse_number(gold), parse_number(pred)) {
            (Some(g), Some(p)) => (g - p).abs() <= label.tol.unwrap_or(0.01) + 1e-9,
            _ => false,
        },
    })
}

/// Serbian Cyrillic folded to Latin, diacritics removed, lowercase.
fn folded(s: &str) -> Cow<'_, str> {
    fold(s, &FoldOptions::KEY)
}

/// Cyrillic capitals that look like Latin ones, as those (`ОС` → `OC`): identifiers are
/// typed in either script with the same glyphs.
fn latin_lookalike(c: char) -> char {
    match c {
        'А' | 'а' => 'A',
        'В' | 'в' => 'B',
        'Е' | 'е' => 'E',
        'К' | 'к' => 'K',
        'М' | 'м' => 'M',
        'Н' | 'н' => 'H',
        'О' | 'о' => 'O',
        'Р' | 'р' => 'P',
        'С' | 'с' => 'C',
        'Т' | 'т' => 'T',
        'Х' | 'х' => 'X',
        'Ј' | 'ј' => 'J',
        _ => c,
    }
}

fn identifier(s: &str) -> String {
    let lookalikes: String = s.chars().map(latin_lookalike).collect();
    folded(&lookalikes)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

/// `BBB-N-KK` (or 18 bare digits) as 18 digits.
fn bank_account(s: &str) -> Option<String> {
    let t = s.trim();
    let parts: Vec<&str> = t.split('-').map(str::trim).collect();
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    match parts.as_slice() {
        [bank, num, check]
            if bank.len() == 3
                && check.len() == 2
                && num.len() <= 13
                && parts.iter().all(|x| digits(x)) =>
        {
            Some(format!("{bank}{num:0>13}{check}"))
        }
        [x] if x.len() == 18 && digits(x) => Some((*x).to_owned()),
        _ => None,
    }
}

/// Token containment either way round; the shorter side needs 3+ characters.
fn contains(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.chars().count() < 3 {
        return !short.is_empty() && short == long;
    }
    let s: Vec<&str> = short.split(' ').collect();
    let l: Vec<&str> = long.split(' ').collect();
    l.windows(s.len()).any(|w| w == s.as_slice())
}

/// Lowercase alphanumeric tokens separated by single spaces.
pub fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

const LEGAL_FORMS: &[&[&str]] = &[
    &["inc"],
    &["incorporated"],
    &["corp"],
    &["corporation"],
    &["co"],
    &["company"],
    &["ltd"],
    &["limited"],
    &["llc"],
    &["l", "l", "c"],
    &["plc"],
    &["lp"],
    &["llp"],
    &["l", "p"],
    &["gmbh"],
    &["ag"],
    &["sa"],
    &["s", "a"],
    &["nv"],
    &["bv"],
    &["doo"],
    &["d", "o", "o"],
    &["ad"],
    &["a", "d"],
    &["sas"],
    &["srl"],
    &["spa"],
    &["s", "p", "a"],
    // Serbian (after folding Cyrillic to Latin): d.o.o. as OCR'd with zeros, a.d., sole
    // traders, one-person companies.
    &["d", "0", "0"],
    &["dooel"],
    &["pr"],
    &["preduzetnik"],
    &["akcionarsko", "drustvo"],
    &["drustvo", "sa", "ogranicenom", "odgovornoscu"],
];

/// Legal forms written before the name (`JP ЕПС`, `JKP Beogradski vodovod`).
const LEADING_FORMS: &[&[&str]] = &[
    &["jp"],
    &["jkp"],
    &["javno", "preduzece"],
    &["javno", "komunalno", "preduzece"],
];

fn company(s: &str) -> String {
    let t = text(&folded(s));
    let mut toks: Vec<&str> = t.split(' ').filter(|x| !x.is_empty()).collect();
    if toks.len() > 1 && toks.first() == Some(&"the") {
        toks.remove(0);
    }
    for form in LEADING_FORMS {
        if toks.len() > form.len() && toks.starts_with(form) {
            toks.drain(..form.len());
            break;
        }
    }
    loop {
        let before = toks.len();
        for form in LEGAL_FORMS {
            if toks.len() > form.len() && toks.ends_with(form) {
                toks.truncate(toks.len() - form.len());
                break;
            }
        }
        if toks.len() == before {
            break;
        }
    }
    toks.join(" ")
}

const MONTHS: &[(&str, u8)] = &[
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("sept", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
    // Serbian, Latin and Cyrillic (folded), nominative and genitive.
    ("januar", 1),
    ("januara", 1),
    ("februar", 2),
    ("februara", 2),
    ("mart", 3),
    ("marta", 3),
    ("aprila", 4),
    ("maj", 5),
    ("maja", 5),
    ("juna", 6),
    ("jula", 7),
    ("avgust", 8),
    ("avgusta", 8),
    ("septembar", 9),
    ("septembra", 9),
    ("oktobar", 10),
    ("oktobra", 10),
    ("novembar", 11),
    ("novembra", 11),
    ("decembar", 12),
    ("decembra", 12),
];

fn month(tok: &str) -> Option<u8> {
    MONTHS.iter().find(|(n, _)| *n == tok).map(|(_, m)| *m)
}

fn year(tok: &str) -> Option<i32> {
    let n: i32 = tok.parse().ok()?;
    match tok.len() {
        4 => Some(n),
        2 => Some(if n < 50 { 2000 + n } else { 1900 + n }),
        _ => None,
    }
}

fn day(tok: &str) -> Option<u8> {
    let digits = tok.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &tok[digits.len()..];
    if !matches!(suffix, "" | "st" | "nd" | "rd" | "th") {
        return None;
    }
    let d: u8 = digits.parse().ok()?;
    (1..=31).contains(&d).then_some(d)
}

fn iso(y: i32, m: u8, d: u8) -> Option<String> {
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => return None,
    };
    ((1..=max).contains(&d) && (1000..=9999).contains(&y)).then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// Parses a date into `YYYY-MM-DD`. Slash dates are read as US `M/D/Y` (the public English
/// corpora), dotted dates as European `D.M.Y`.
#[allow(clippy::many_single_char_names)]
pub fn parse_date(s: &str) -> Option<String> {
    let s = s.trim();
    // Numeric forms.
    for (sep, order) in [('-', "ymd"), ('/', "mdy"), ('.', "dmy")] {
        let parts: Vec<&str> = s.trim_end_matches('.').split(sep).map(str::trim).collect();
        if parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        {
            if parts[0].len() == 4 {
                let y = year(parts[0])?;
                return iso(y, parts[1].parse().ok()?, parts[2].parse().ok()?);
            }
            return match order {
                "mdy" => iso(
                    year(parts[2])?,
                    parts[0].parse().ok()?,
                    parts[1].parse().ok()?,
                ),
                "dmy" => iso(
                    year(parts[2])?,
                    parts[1].parse().ok()?,
                    parts[0].parse().ok()?,
                ),
                _ => None,
            };
        }
    }
    // Worded forms: tokens of `text` (Cyrillic folded), ignoring filler words.
    let t = text(&folded(s));
    let toks: Vec<&str> = t
        .split(' ')
        .filter(|w| {
            !matches!(
                *w,
                "the"
                    | "day"
                    | "of"
                    | "this"
                    | "dated"
                    | "as"
                    | "on"
                    | "dana"
                    | "godine"
                    | "god"
                    | "g"
            )
        })
        .collect();
    match toks.as_slice() {
        [m, d, y] if month(m).is_some() => iso(year(y)?, month(m)?, day(d)?),
        [d, m, y] if month(m).is_some() => iso(year(y)?, month(m)?, day(d)?),
        _ => None,
    }
}

/// Parses a decimal number written with thousands separators and `.` or `,` as the decimal
/// mark; ignores currency symbols and codes around it.
pub fn parse_number(s: &str) -> Option<f64> {
    let core: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '\''))
        .filter(|c| *c != '\'')
        .collect();
    let neg = core.starts_with('-');
    let core = core.trim_start_matches('-');
    if core.is_empty() || !core.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let last_dot = core.rfind('.');
    let last_comma = core.rfind(',');
    let decimal = match (last_dot, last_comma) {
        (Some(d), Some(c)) => Some(d.max(c)),
        (Some(d), None) => {
            // `1.234.567` or `1.234` with three digits after: thousands; else decimal.
            let after = core.len() - d - 1;
            (core.matches('.').count() == 1 && after != 3).then_some(d)
        }
        (None, Some(c)) => {
            let after = core.len() - c - 1;
            (core.matches(',').count() == 1 && after != 3).then_some(c)
        }
        (None, None) => None,
    };
    let mut plain = String::with_capacity(core.len());
    for (i, ch) in core.char_indices() {
        if ch.is_ascii_digit() {
            plain.push(ch);
        } else if Some(i) == decimal {
            plain.push('.');
        }
    }
    let v: f64 = plain.parse().ok()?;
    Some(if neg { -v } else { v })
}

fn parse_bool(s: &str) -> Option<bool> {
    match text(s).as_str() {
        "true" | "yes" | "y" | "1" | "da" => Some(true),
        "false" | "no" | "n" | "0" | "ne" => Some(false),
        _ => None,
    }
}

fn language(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    let primary = lower.split(['-', '_']).next().unwrap_or("").to_string();
    let named = match primary.as_str() {
        "english" => "en",
        "serbian" => "sr",
        "croatian" => "hr",
        "bosnian" => "bs",
        "german" => "de",
        "french" => "fr",
        "spanish" => "es",
        "italian" => "it",
        "russian" => "ru",
        "slovenian" => "sl",
        "macedonian" => "mk",
        "hungarian" => "hu",
        _ => return primary,
    };
    named.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_company() {
        assert_eq!(text("  New_York, U.S.A. "), "new york u s a");
        assert_eq!(company("The Alpha Holdings, Inc."), "alpha holdings");
        assert_eq!(company("Beta d.o.o."), "beta");
        assert_eq!(company("Gamma Co., Ltd."), "gamma");
        // Never strips the whole name.
        assert_eq!(company("Inc"), "inc");
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("2001-02-03").as_deref(), Some("2001-02-03"));
        assert_eq!(parse_date("2/3/2001").as_deref(), Some("2001-02-03"));
        assert_eq!(parse_date("3.2.2001.").as_deref(), Some("2001-02-03"));
        assert_eq!(
            parse_date("February 3, 2001").as_deref(),
            Some("2001-02-03")
        );
        assert_eq!(
            parse_date("3rd day of February, 2001").as_deref(),
            Some("2001-02-03")
        );
        assert_eq!(parse_date("3 Feb 01").as_deref(), Some("2001-02-03"));
        assert_eq!(parse_date("2001-02-30"), None);
        assert_eq!(parse_date("sometime next year"), None);
    }

    #[test]
    fn serbian_company_forms_and_scripts() {
        assert_eq!(
            company("Bimed d.o.o., Beograd"),
            company("Bimed д.о.о., Београд")
        );
        assert_eq!(company("„Зекоз“ д.о.о"), "zekoz");
        assert_eq!(company("ФАРМИКС ДОО"), "farmiks");
        assert_eq!(company("Alfa a.d."), "alfa");
        assert_eq!(company("Бета а.д."), "beta");
        assert_eq!(company("ЈП ЕПС"), "eps");
        assert_eq!(company("JKP Vodovod"), "vodovod");
        assert_eq!(company("Gama d.0.0."), "gama");
        assert_eq!(
            company("Petar Petrović PR Delta"),
            "petar petrovic pr delta"
        );
        assert_eq!(company("Delta Petrović preduzetnik"), "delta petrovic");
    }

    #[test]
    fn serbian_dates() {
        assert_eq!(parse_date("1. јула 2026").as_deref(), Some("2026-07-01"));
        assert_eq!(
            parse_date("26. септембра 2024. године").as_deref(),
            Some("2024-09-26")
        );
        assert_eq!(parse_date("1. aprila 2026").as_deref(), Some("2026-04-01"));
        assert_eq!(parse_date("12. mart 2023").as_deref(), Some("2023-03-12"));
        assert_eq!(
            parse_date("dana 3. decembra 2019. god.").as_deref(),
            Some("2019-12-03")
        );
    }

    #[test]
    fn identifiers_and_accounts() {
        assert_eq!(
            Normalizer::Identifier.apply("66-73-224-15/ОС 90"),
            "667322415OC90"
        );
        assert_eq!(
            Normalizer::Identifier.apply("66-73-224-15/OC90"),
            Normalizer::Identifier.apply("66-73-224-15/ОС 90")
        );
        assert_eq!(Normalizer::Identifier.apply("Бр. 404-1/20"), "BP404120");
        let acc = Normalizer::BankAccount;
        assert_eq!(acc.apply("840-26650-09"), acc.apply("840-0000000026650-09"));
        assert_eq!(acc.apply("840-26650-09"), "840000000002665009");
        assert_eq!(acc.apply("840000000002665009"), "840000000002665009");
        assert_eq!(acc.apply("RS35 2600"), "RS352600");
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("EUR 1.234,56"), Some(1234.56));
        assert_eq!(parse_number("$1,234.56"), Some(1234.56));
        assert_eq!(parse_number("1,234"), Some(1234.0));
        assert_eq!(parse_number("12,5"), Some(12.5));
        assert_eq!(parse_number("-7"), Some(-7.0));
        assert_eq!(parse_number("n/a"), None);
    }

    #[test]
    fn matching_modes() {
        let l = Label::text("party", "Alpha Holdings, Inc.", MatchMode::Normalized);
        assert!(label_matches(&l, Normalizer::Company, "ALPHA HOLDINGS"));
        assert!(!label_matches(&l, Normalizer::Company, "Alpha"));
        let l = Label::text("party", "Alpha Holdings", MatchMode::Contains);
        assert!(label_matches(
            &l,
            Normalizer::Text,
            "Alpha Holdings Group of Companies"
        ));
        assert!(!label_matches(&l, Normalizer::Text, "Alphabet Holdings"));
        let l = Label::text("x", "A-1", MatchMode::Exact);
        assert!(label_matches(&l, Normalizer::Identifier, " A-1 "));
        assert!(!label_matches(&l, Normalizer::Identifier, "A1"));
        let mut l = Label::text("amount_total", "4200.50", MatchMode::NumericTol);
        assert!(label_matches(&l, Normalizer::Number, "4.200,49 EUR"));
        l.tol = Some(0.001);
        assert!(!label_matches(&l, Normalizer::Number, "4200.49"));
        let mut l = Label::text("language", "", MatchMode::AnyOf);
        l.value = serde_json::json!(["sr", "hr"]);
        assert!(label_matches(&l, Normalizer::Language, "hr-HR"));
        assert!(!label_matches(&l, Normalizer::Language, "en"));
        let l = Label::text("d", "February 3, 2001", MatchMode::Normalized);
        assert!(label_matches(&l, Normalizer::Date, "2001-02-03"));
    }
}

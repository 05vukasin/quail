//! Descriptions by token shape: what in a cell is the description and what is a date, a
//! time, a weekday or a code that a layout glued to it, and how much a cell reads as a
//! description at all.
//!
//! Broadcast and logistics invoices put air date, air time, weekday, station and spot code
//! next to the program name, and a PDF layout often reads them as one cell
//! (`3 WBTV M 04/21/14 11:24 PM WBTV News @ 11p`). [`clean`] splits such a description at
//! those tokens and keeps the run of words that reads most like a description
//! (`WBTV News @ 11p`); a code it drops (`DJTTV091216H`) is returned for the `code` field.
//! [`text_score`] ranks cells (and so columns) by how much they read as a description, so
//! a code column (`KM16103H`) or a station column (`ESPN` in every row) loses to the program
//! column next to it.

/// What a whitespace-separated token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `04/21/14`, `07-20-16`, `10/10/2013-10/10/2013`, `12.05.2023`.
    Date,
    /// `11:24`, `5:30-6:00`, `19:00-24:00`, `8-9p`, `1135p-1205a`, `:30`, `AM`.
    Time,
    /// `M`, `Tu`, `WED`, `M-F`, `Sa-Su`, `MTWTF--`.
    Weekday,
    /// `DJTTV091216H`, `2013CCFG014H`: a run of capitals and digits.
    Code,
    /// `3`, `30`, `1/0`: a bare count or ordinal.
    Int,
    /// Anything else.
    Word,
}

impl Kind {
    /// A token that separates description runs.
    fn noise(self) -> bool {
        matches!(self, Self::Date | Self::Time | Self::Weekday | Self::Code)
    }
}

fn digits_and(s: &str, seps: &[char]) -> bool {
    !s.is_empty()
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || seps.contains(&c))
}

/// `04/21/14`, `07-20-16`, `12.05.2023`, `10/2024`; a range of two of them.
fn is_date(t: &str) -> bool {
    let one = |s: &str| {
        if !digits_and(s, &['/', '-', '.']) {
            return false;
        }
        let sep = if s.contains('/') {
            '/'
        } else if s.contains('-') {
            '-'
        } else {
            '.'
        };
        let parts: Vec<&str> = s.trim_end_matches('.').split(sep).collect();
        let ok_len = parts.iter().all(|p| (1..=4).contains(&p.len()));
        let mixed = s
            .chars()
            .filter(|c| matches!(c, '/' | '-' | '.'))
            .any(|c| c != sep);
        if mixed || !ok_len {
            return false;
        }
        match parts.len() {
            3 => parts[0].len() <= 4 && parts[1].len() <= 2,
            // `10/2024`, `05/16`: month and year (not `3/4`, not `13-16`).
            2 => sep == '/' && parts[0].len() <= 2 && parts[1].len() >= 2,
            _ => false,
        }
    };
    if one(t) {
        return true;
    }
    // `10/10/2013-10/10/2013`
    t.split_once('-').is_some_and(|(a, b)| one(a) && one(b))
}

/// `11:24`, `20:31:05`, `6a`, `1135p`, `500A` (with `bare` also `11p`).
fn clock(s: &str, bare: bool) -> bool {
    let s = s.trim_end_matches(['.', ',']);
    let lower = s.to_ascii_lowercase();
    let body = lower
        .strip_suffix("am")
        .or_else(|| lower.strip_suffix("pm"))
        .or_else(|| lower.strip_suffix('a'))
        .or_else(|| lower.strip_suffix('p'))
        .or_else(|| lower.strip_suffix('m'));
    let suffixed = body.is_some();
    let body = body.unwrap_or(&lower);
    if body.is_empty() || body.len() > 8 {
        return false;
    }
    if body.contains(':') {
        let parts: Vec<&str> = body.split(':').collect();
        return (2..=3).contains(&parts.len())
            && parts[0].len() <= 2
            && parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty())
            && parts[1..].iter().all(|p| p.len() == 2);
    }
    body.chars().all(|c| c.is_ascii_digit()) && body.len() <= 4 && suffixed && bare
}

/// `11:24`, `5:30-6:00`, `8-9p`, `10a-3p`, `:30`, `00:00:30`.
fn is_time(t: &str) -> bool {
    let t = t.trim_end_matches([',', ';']);
    if let Some(rest) = t.strip_prefix(':') {
        return rest.len() == 2 && rest.chars().all(|c| c.is_ascii_digit());
    }
    if clock(t, false) {
        return true;
    }
    // A range: both ends clocks, at least one with a colon or a suffix (`8-9p`, `7-9A`).
    if let Some((a, b)) = t.split_once('-') {
        let num = |s: &str| !s.is_empty() && s.len() <= 4 && s.chars().all(|c| c.is_ascii_digit());
        let (ca, cb) = (clock(a, true), clock(b, true));
        return ((ca || num(a)) && cb) || (ca && num(b) && a.contains(':'));
    }
    false
}

const WEEKDAYS: &[&str] = &[
    "mo", "tu", "we", "th", "fr", "sa", "su", "mon", "tue", "tues", "wed", "thu", "thur", "thurs",
    "fri", "sat", "sun",
];

/// `M-F`, `Sa-Su`, `M-Su`: a daypart, which names the program slot (kept as a word).
fn is_daypart(t: &str) -> bool {
    let lower = t.trim_end_matches(['.', ',']).to_ascii_lowercase();
    lower.split_once('-').is_some_and(|(a, b)| {
        !b.contains('-')
            && [a, b]
                .iter()
                .all(|d| d.len() == 1 && "mtwfs".contains(*d) || WEEKDAYS.contains(d))
    })
}

/// `M`, `Tu`, `WED`, `MTWTF--`, `-T-----`, `TW`.
fn is_weekday(t: &str) -> bool {
    let bare = t.trim_end_matches(['.', ',']);
    if bare.len() == 1 {
        return matches!(bare, "M" | "T" | "W" | "F" | "S");
    }
    let lower = bare.to_ascii_lowercase();
    if WEEKDAYS.contains(&lower.as_str()) {
        return true;
    }
    // A day grid: seven slots of day letters and dashes (`MTWTF--`, `-T-----`, `MTUWThF`).
    let grid = bare.chars().all(|c| c == '-' || c.is_ascii_alphabetic())
        && bare.contains('-')
        && (5..=9).contains(&bare.len());
    // Pieces of a day grid split at its blanks: `TW`, `TF-`, `-WTF-`.
    let letters = bare.chars().all(|c| "MTWFS-".contains(c)) && bare.len() <= 7;
    grid || letters || bare == "MTUWThF" || bare == "MTWTFSS"
}

/// `DJTTV091216H`, `2013CCFG014H`, `KM16103H`: capitals and digits, six or more, both kinds.
pub(crate) fn is_code(t: &str) -> bool {
    let t = t.trim_matches(|c: char| matches!(c, ',' | ';' | '(' | ')'));
    let n = t.chars().count();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let upper = t.chars().filter(char::is_ascii_uppercase).count();
    n >= 6
        && digits >= 1
        && upper >= 2
        && t.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn is_meridiem(t: &str) -> bool {
    matches!(
        t.trim_end_matches(['.', ',']),
        "AM" | "PM" | "am" | "pm" | "a.m" | "p.m"
    )
}

fn kind(t: &str) -> Kind {
    if is_date(t) {
        Kind::Date
    } else if is_time(t) {
        Kind::Time
    } else if is_meridiem(t) {
        // A time only after a clock (`11:24 PM`); `PM Drive` is a program.
        Kind::Word
    } else if is_daypart(t) {
        Kind::Word
    } else if is_weekday(t) {
        Kind::Weekday
    } else if is_code(t) {
        Kind::Code
    } else if digits_and(t, &['/', '-']) && t.len() <= 5 {
        Kind::Int
    } else {
        Kind::Word
    }
}

/// Tokens of a description: whitespace-separated, with a clock glued to a word split off
/// (`Football22:11:48` is `Football` `22:11:48`).
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for t in s.split_whitespace() {
        let split = t
            .char_indices()
            .find(|&(i, c)| {
                c.is_ascii_digit()
                    && i > 0
                    && t[..i].chars().last().is_some_and(char::is_alphabetic)
                    && t[..i].chars().filter(|c| c.is_alphabetic()).count() >= 3
                    && is_time(&t[i..])
            })
            .map(|(i, _)| i);
        match split {
            Some(i) => {
                out.push(&t[..i]);
                out.push(&t[i..]);
            }
            None => out.push(t),
        }
    }
    out
}

/// A token that reads as a word of a description: two letters or more, not a code.
fn wordish(t: &str) -> bool {
    t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && kind(t) == Kind::Word
}

/// A description split at its dates, times, weekdays and codes: the run of words that reads
/// most like a description, and the first code dropped (for the `code` field). `None` when
/// there is nothing to split or no run has a word.
pub(crate) fn clean(desc: &str) -> Option<(String, Option<String>)> {
    let mut toks = tokens(desc);
    // `® MG for 6.2, 24.1`: a make-good note on the program, not part of it.
    let note = toks.iter().enumerate().position(|(i, t)| {
        *t == "®"
            || t.starts_with("MGfor")
            || (*t == "MG" && toks.get(i + 1).is_some_and(|n| *n == "for"))
    });
    if let Some(i) = note {
        toks.truncate(i);
    }
    let mut kinds: Vec<Kind> = toks.iter().map(|t| kind(t)).collect();
    for i in 1..kinds.len() {
        if is_meridiem(toks[i]) && kinds[i - 1] == Kind::Time {
            kinds[i] = Kind::Time;
        }
    }
    // Without an air time this is no broadcast line: product names hold codes and capitals
    // (`HYPAK TSCF1MLL`, `Programming / MW`), so only dates split it, and a weekday only
    // next to a date (`07-13-16 WED`).
    if !kinds.contains(&Kind::Time) {
        let date_at = |i: usize| kinds.get(i).is_some_and(|k| *k == Kind::Date);
        let near: Vec<bool> = (0..kinds.len())
            .map(|i| date_at(i.wrapping_sub(1)) || date_at(i + 1))
            .collect();
        for (i, k) in kinds.iter_mut().enumerate() {
            let day_word = *k == Kind::Weekday
                && toks[i].len() >= 2
                && WEEKDAYS.contains(
                    &toks[i]
                        .trim_end_matches(['.', ','])
                        .to_ascii_lowercase()
                        .as_str(),
                );
            if *k == Kind::Code || (*k == Kind::Weekday && !(day_word && near[i])) {
                *k = Kind::Word;
            }
        }
    }
    if note.is_none() && !kinds.iter().any(|k| k.noise()) {
        return None;
    }
    let code = toks
        .iter()
        .zip(&kinds)
        .find(|(_, k)| **k == Kind::Code)
        .map(|(t, _)| (*t).to_owned());
    // Runs between noise tokens, without the counts and punctuation at their ends.
    let mut runs: Vec<&[&str]> = Vec::new();
    let mut start = 0;
    for i in 0..=toks.len() {
        if i == toks.len() || kinds[i].noise() {
            let mut a = start;
            let mut b = i;
            let edge = |t: &str, k: Kind| {
                k == Kind::Int || !t.chars().any(char::is_alphanumeric) || t == "to"
            };
            while a < b && edge(toks[a], kinds[a]) {
                a += 1;
            }
            while b > a && edge(toks[b - 1], kinds[b - 1]) {
                b -= 1;
            }
            if a < b {
                runs.push(&toks[a..b]);
            }
            start = i + 1;
        }
    }
    // Most words wins; on a tie the later run (the program comes after the air date).
    let best = runs
        .iter()
        .enumerate()
        .max_by_key(|(i, r)| (r.iter().filter(|t| wordish(t)).count(), *i))
        .map(|(_, r)| *r)?;
    if !best.iter().any(|t| wordish(t)) {
        return None;
    }
    let text = best.join(" ");
    let text = text.trim_end_matches([':', '-', '–', ',', ';']).trim();
    if text.chars().filter(|c| c.is_alphabetic()).count() < 3 {
        return None;
    }
    Some((text.to_owned(), code))
}

/// How much a cell reads as a description: its words (two letters or more, not dates, times,
/// weekdays or codes), a lone short all-capitals word (a station or network: `ESPN`) counting
/// half.
pub(crate) fn text_score(cell: &str) -> f32 {
    let words: Vec<&str> = tokens(cell).into_iter().filter(|t| wordish(t)).collect();
    match words.as_slice() {
        [] => 0.0,
        [w] if w.chars().count() <= 5
            && w.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
        {
            0.5
        }
        ws => {
            #[allow(clippy::cast_precision_loss)]
            let n = ws.len().min(8) as f32;
            n
        }
    }
}

/// A description that names nothing by itself: a station or code alone (`WTVD`, `KM16103H`).
pub(crate) fn weak(desc: &str) -> bool {
    text_score(desc) <= 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Option<String> {
        clean(s).map(|(t, _)| t)
    }

    #[test]
    fn token_shapes() {
        for d in [
            "04/21/14",
            "07-20-16",
            "10/10/2013-10/10/2013",
            "12.05.2023",
            "10/2024",
        ] {
            assert_eq!(kind(d), Kind::Date, "{d}");
        }
        for t in [
            "11:24",
            "20:31:05",
            "5:30-6:00",
            "19:00-24:00",
            "8-9p",
            "1135p-1205a",
            ":30",
            "10a-3p",
            "500A-700A",
            "7-9A",
        ] {
            assert_eq!(kind(t), Kind::Time, "{t}");
        }
        for w in ["M", "Tu", "WED", "MTWTF--", "-T-----", "TW", "TF-"] {
            assert_eq!(kind(w), Kind::Weekday, "{w}");
        }
        for k in ["DJTTV091216H", "2013CCFG014H", "KM16103H"] {
            assert_eq!(kind(k), Kind::Code, "{k}");
        }
        for w in [
            "3/4", "13-16", "11p", "6a", "Monday", "News", "2,000", "ISO", "M-F", "Sa-Su", "PM",
        ] {
            assert!(!kind(w).noise(), "{w}");
        }
    }

    #[test]
    fn keeps_the_program_name() {
        assert_eq!(
            c("3 WBTV M 04/21/14 11:24 PM WBTV News @ 11p").as_deref(),
            Some("WBTV News @ 11p")
        );
        assert_eq!(c("FXNC 07-13-16 WED Hannity").as_deref(), Some("Hannity"));
        assert_eq!(
            c("Tu 10/11/16 6:27 AM M-F Local News @ 6a ® MG for 6.2, 24.1").as_deref(),
            Some("M-F Local News @ 6a")
        );
        assert_eq!(
            c("2 WEDR M 10/03/16 3:53 PM M-F PM Drive").as_deref(),
            Some("M-F PM Drive")
        );
        assert_eq!(
            c("1 08/06/12 09/02/12 Good Morning America 7-9A").as_deref(),
            Some("Good Morning America")
        );
        assert_eq!(c("SportsCenter 20:31:05").as_deref(), Some("SportsCenter"));
        assert_eq!(
            c("College Football22:11:48").as_deref(),
            Some("College Football")
        );
        assert_eq!(c("30 AMC DT").as_deref(), None);
        let (t, code) = clean(
            "13-16 DISC PHILLY 7:44PM DJTTV091216H DEPLORABLES H Deadliest Catch: On Deck 30 7",
        )
        .unwrap();
        assert_eq!(t, "DEPLORABLES H Deadliest Catch: On Deck");
        assert_eq!(code.as_deref(), Some("DJTTV091216H"));
        // Nothing to split: unchanged.
        assert_eq!(c("Consulting services, senior"), None);
        // Product names keep their codes and capitals when no air time is written.
        assert_eq!(c("47533130 ULTRASAFE X225PLUS RFS"), None);
        assert_eq!(c("Team Leader for Programming / MW"), None);
        assert_eq!(c("PRODUCTION Feb 24"), None);
        assert_eq!(
            c("4/12/20 HYPAK TSCF1MLL 4023FLUR").as_deref(),
            Some("HYPAK TSCF1MLL 4023FLUR")
        );
        // Only noise: unchanged.
        assert_eq!(c("6:30-7A"), None);
        assert_eq!(
            c("Usluga održavanja 01.03.2024-31.03.2024").as_deref(),
            Some("Usluga održavanja")
        );
    }

    #[test]
    fn scores_text_over_codes_and_stations() {
        assert!(text_score("America's Newsroom") > text_score("ESPN"));
        assert!(text_score("ESPN") > text_score("LGPAC071816AH"));
        assert!(text_score("SportsCenter 20:31:05") >= 1.0);
        assert!(weak("WTVD"));
        assert!(weak("KM16103H"));
        assert!(!weak("Judge Faith"));
    }
}

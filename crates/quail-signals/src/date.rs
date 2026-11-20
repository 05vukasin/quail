//! Dates: numeric (`12.09.2023.`, `12. 09. 2023.`, `2023-09-12`, `12/09/2023`), with
//! month names in Serbian (Latin and Cyrillic, nominative and genitive), Croatian, English
//! and German (`12. septembra 2023.`, `12. септембра 2023.`, `September 12, 2023`,
//! `12th day of September, 2023`), and the role a keyword before them gives.
//!
//! Slash and dash dates whose day and month are both 12 or less (`05/06/2023`) are
//! ambiguous. They are read day-first unless the file shows month-first evidence: more
//! unambiguous month-first dates (`09/25/2023`, `September 12, 2023`) than day-first ones
//! (`25/09/2023`, `12.09.2023`, `12 September 2023`). See `resolve`.

use quail_core::{Date, FactKey, Span};
use quail_text::token::TokenKind;

use crate::checksum::valid_ymd;
use crate::conf;
use crate::keys;
use crate::page::PageText;
use crate::phrase::{Reach, find_before};

/// What a keyword before a date says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Effective,
    Expiry,
    Signed,
    Invoice,
    Due,
}

impl Role {
    pub fn key(self) -> FactKey {
        match self {
            Self::Effective => keys::DATE_EFFECTIVE,
            Self::Expiry => keys::DATE_EXPIRY,
            Self::Signed => keys::DATE_SIGNED,
            Self::Invoice => keys::DATE_INVOICE,
            Self::Due => keys::DATE_DUE,
        }
    }
}

/// How a date was written, for the day/month order vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// Day first for sure (`12.09.2023`, `25/09/2023`, `12 September 2023`).
    Dmy,
    /// Month first for sure (`09/25/2023`, `September 12, 2023`).
    Mdy,
    /// Year first (ISO): says nothing about the order.
    Ymd,
    /// `05/06/2023`: day-first reading in `date`, month-first in `alt`.
    Ambiguous,
}

/// A date found on a page.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DateCand {
    pub span: Span,
    pub date: Date,
    /// Month-first reading of an ambiguous date.
    pub alt: Option<Date>,
    pub order: Order,
    pub conf: f32,
    /// Role key and the role's own precision.
    pub role: Option<(Role, f32)>,
}

/// Month of a folded month name: English (full and abbreviated), Serbian nominative and
/// genitive (Cyrillic folds to Latin), Croatian, and the German forms that differ.
fn month(key: &str) -> Option<u8> {
    Some(match key {
        "january" | "jan" | "januar" | "januara" | "sijecanj" | "sijecnja" => 1,
        "february" | "feb" | "februar" | "februara" | "veljaca" | "veljace" => 2,
        "march" | "mar" | "mart" | "marta" | "ozujak" | "ozujka" | "marz" | "maerz" => 3,
        "april" | "apr" | "aprila" | "travanj" | "travnja" => 4,
        "may" | "maj" | "maja" | "svibanj" | "svibnja" | "mai" => 5,
        "june" | "jun" | "juna" | "juni" | "lipanj" | "lipnja" => 6,
        "july" | "jul" | "jula" | "juli" | "srpanj" | "srpnja" => 7,
        "august" | "aug" | "avgust" | "avgusta" | "augusta" | "kolovoz" | "kolovoza" => 8,
        "september" | "sep" | "sept" | "septembar" | "septembra" | "rujan" | "rujna" => 9,
        "october" | "oct" | "oktobar" | "oktobra" | "listopad" | "listopada" | "oktober" => 10,
        "november" | "nov" | "novembar" | "novembra" | "studeni" | "studenoga" | "studenog" => 11,
        "december" | "dec" | "decembar" | "decembra" | "prosinac" | "prosinca" | "dezember" => 12,
        _ => return None,
    })
}

/// Role keywords before a date (folded phrases).
const ROLES: &[(&[&str], Role)] = &[
    (&["effective", "date"], Role::Effective),
    (&["effective"], Role::Effective),
    (&["commencement", "date"], Role::Effective),
    (&["start", "date"], Role::Effective),
    (&["with", "effect", "from"], Role::Effective),
    (&["stupa", "na", "snagu"], Role::Effective),
    (&["stupanja", "na", "snagu"], Role::Effective),
    (&["vazi", "od"], Role::Effective),
    (&["datum", "pocetka"], Role::Effective),
    (&["expiry", "date"], Role::Expiry),
    (&["expiration", "date"], Role::Expiry),
    (&["expires"], Role::Expiry),
    (&["expire"], Role::Expiry),
    (&["termination", "date"], Role::Expiry),
    (&["end", "date"], Role::Expiry),
    (&["valid", "until"], Role::Expiry),
    (&["valid", "through"], Role::Expiry),
    (&["through"], Role::Expiry),
    (&["ending", "on"], Role::Expiry),
    (&["end", "on"], Role::Expiry),
    (&["terminate", "on"], Role::Expiry),
    (&["terminates", "on"], Role::Expiry),
    (&["concluding"], Role::Expiry),
    (&["commencing", "on"], Role::Effective),
    (&["as", "of"], Role::Effective),
    (&["entered", "into"], Role::Effective),
    (&["made", "this"], Role::Effective),
    (&["into", "this"], Role::Effective),
    (&["commences", "on"], Role::Effective),
    (&["until"], Role::Expiry),
    (&["vazi", "do"], Role::Expiry),
    (&["do", "dana"], Role::Expiry),
    (&["istice"], Role::Expiry),
    (&["rok", "vazenja"], Role::Expiry),
    (&["datum", "isteka"], Role::Expiry),
    (&["zakljucno", "sa"], Role::Expiry),
    (&["signed"], Role::Signed),
    (&["dated"], Role::Signed),
    (&["executed"], Role::Signed),
    (&["date", "of", "signature"], Role::Signed),
    (&["signature", "date"], Role::Signed),
    (&["datum", "potpisivanja"], Role::Signed),
    (&["potpisan"], Role::Signed),
    (&["potpisano"], Role::Signed),
    (&["zakljucen"], Role::Signed),
    (&["zakljucen", "dana"], Role::Signed),
    (&["zakljucen", "je", "dana"], Role::Signed),
    (&["zakljucuju", "dana"], Role::Signed),
    (&["invoice", "date"], Role::Invoice),
    (&["date", "of", "invoice"], Role::Invoice),
    (&["datum", "fakture"], Role::Invoice),
    (&["datum", "racuna"], Role::Invoice),
    (&["datum", "izdavanja"], Role::Invoice),
    (&["issue", "date"], Role::Invoice),
    (&["date", "of", "issue"], Role::Invoice),
    (&["due", "date"], Role::Due),
    (&["payment", "due"], Role::Due),
    (&["due"], Role::Due),
    (&["pay", "by"], Role::Due),
    (&["valuta"], Role::Due),
    (&["rok", "placanja"], Role::Due),
    (&["datum", "dospeca"], Role::Due),
    (&["dospece"], Role::Due),
];

const ROLE_REACH: Reach = Reach { words: 5, lines: 1 };

/// Words that turn a role keyword into another date (`Discount Due Date`).
const ROLE_BLOCKERS: &[&str] = &["discount", "early"];

/// Is the word before the role phrase starting at `first` a blocker?
fn blocked(p: &PageText, first: usize) -> bool {
    (first.saturating_sub(3)..first)
        .rev()
        .find(|&j| p.wordlike(j))
        .is_some_and(|j| p.line(j) == p.line(first) && ROLE_BLOCKERS.contains(&p.key(j)))
}

/// Measured precision of each role (see the README).
fn role_conf(r: Role) -> f32 {
    match r {
        Role::Effective => conf::DATE_ROLE_EFFECTIVE,
        Role::Expiry => conf::DATE_ROLE_EXPIRY,
        Role::Signed => conf::DATE_ROLE_SIGNED,
        Role::Invoice => conf::DATE_ROLE_INVOICE,
        Role::Due => conf::DATE_ROLE_DUE,
    }
}

/// Scans a page for dates.
pub(crate) fn scan(p: &PageText, out: &mut Vec<DateCand>) {
    let mut i = 0;
    while i < p.len() {
        match at(p, i) {
            Some((mut c, last)) => {
                c.role = find_before(p, i, ROLES, ROLE_REACH)
                    .filter(|(_, first)| !blocked(p, *first))
                    .map(|(k, _)| (k, role_conf(k)));
                out.push(c);
                i = last + 1;
            }
            None => i += 1,
        }
    }
}

fn mk(y: u32, m: u32, d: u32) -> Option<Date> {
    let yi = i32::try_from(y).ok()?;
    if !(1900..=2099).contains(&yi) || !valid_ymd(yi, m, d) {
        return None;
    }
    Some(Date {
        y: i16::try_from(y).ok()?,
        m: u8::try_from(m).ok()?,
        d: u8::try_from(d).ok()?,
    })
}

/// Day number of a token: `12`, `12th`, `1st`, `2nd`, `3rd`.
fn day_of(p: &PageText, i: usize) -> Option<u32> {
    let t = p.text(i);
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let rest = &t[digits..];
    match p.kind(i)? {
        TokenKind::Number if rest.is_empty() => {}
        TokenKind::Alnum
            if matches!(
                rest.to_ascii_lowercase().as_str(),
                "st" | "nd" | "rd" | "th"
            ) => {}
        _ => return None,
    }
    t[..digits].parse().ok()
}

fn year_of(p: &PageText, i: usize) -> Option<u32> {
    let t = p.text(i);
    (p.kind(i)? == TokenKind::Number && t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()))
        .then(|| t.parse().ok())
        .flatten()
}

/// Next token index if it follows `j` tightly (same line, at most one space).
fn next(p: &PageText, j: usize) -> Option<usize> {
    (j + 1 < p.len() && p.tight(j + 1)).then_some(j + 1)
}

/// Skips one punctuation token `.` or `,` after `j` if present.
fn skip_punct(p: &PageText, j: usize, allowed: &[&str]) -> usize {
    match next(p, j) {
        Some(k) if p.kind(k) == Some(TokenKind::Punct) && allowed.contains(&p.text(k)) => k,
        _ => j,
    }
}

/// A date starting at token `i`, with its last token.
fn at(p: &PageText, i: usize) -> Option<(DateCand, usize)> {
    match p.kind(i)? {
        TokenKind::Number => numeric(p, i).or_else(|| day_first(p, i)),
        TokenKind::Alnum => day_first(p, i),
        TokenKind::Word => month_first(p, i),
        _ => None,
    }
}

fn cand(
    p: &PageText,
    a: usize,
    b: usize,
    date: Date,
    order: Order,
    conf: f32,
) -> (DateCand, usize) {
    (
        DateCand {
            span: p.span(a, b),
            date,
            alt: None,
            order,
            conf,
            role: None,
        },
        b,
    )
}

/// `12.09.2023`, `2023-09-12`, `12/09/2023`, and `12. 09. 2023` spread over tokens.
fn numeric(p: &PageText, i: usize) -> Option<(DateCand, usize)> {
    let t = p.text(i);
    let parts: Vec<&str> = t.split(['.', '/', '-']).collect();
    let seps: Vec<char> = t.chars().filter(|c| matches!(c, '.' | '/' | '-')).collect();
    if parts.len() == 3 && seps[0] == seps[1] {
        let n: Vec<u32> = parts
            .iter()
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        let l: Vec<usize> = parts.iter().map(|s| s.len()).collect();
        if l[0] == 4 && (1..=2).contains(&l[1]) && (1..=2).contains(&l[2]) {
            let d = mk(n[0], n[1], n[2])?;
            return Some(cand(p, i, i, d, Order::Ymd, conf::DATE_ISO));
        }
        if (1..=2).contains(&l[0]) && (1..=2).contains(&l[1]) && l[2] == 4 {
            if seps[0] == '.' {
                let d = mk(n[2], n[1], n[0])?;
                return Some(cand(p, i, i, d, Order::Dmy, conf::DATE_DOTTED));
            }
            let dmy = mk(n[2], n[1], n[0]);
            let mdy = mk(n[2], n[0], n[1]);
            return match (dmy, mdy) {
                (Some(d), None) => Some(cand(p, i, i, d, Order::Dmy, conf::DATE_SLASH)),
                (None, Some(d)) => Some(cand(p, i, i, d, Order::Mdy, conf::DATE_SLASH)),
                (Some(d), Some(alt)) if d == alt => {
                    Some(cand(p, i, i, d, Order::Dmy, conf::DATE_SLASH))
                }
                (Some(d), Some(alt)) => {
                    let (mut c, last) =
                        cand(p, i, i, d, Order::Ambiguous, conf::DATE_AMBIGUOUS_DEFAULT);
                    c.alt = Some(alt);
                    Some((c, last))
                }
                (None, None) => None,
            };
        }
        return None;
    }
    // `12. 09. 2023`: day, dot, month, dot, year as separate tokens.
    if parts.len() == 1 && (1..=2).contains(&t.len()) {
        let d1 = next(p, i)?;
        let m = next(p, d1)?;
        let d2 = next(p, m)?;
        let y = next(p, d2)?;
        let dots = p.text(d1) == "." && p.text(d2) == "." && p.touch(d1) && p.touch(d2);
        let mt = p.text(m);
        if dots && p.kind(m) == Some(TokenKind::Number) && (1..=2).contains(&mt.len()) {
            let year = year_of(p, y)?;
            let d = mk(year, mt.parse().ok()?, t.parse().ok()?)?;
            return Some(cand(p, i, y, d, Order::Dmy, conf::DATE_DOTTED));
        }
    }
    None
}

/// `12. septembra 2023`, `12 September 2023`, `12th of September, 2023`,
/// `12th day of September, 2023`.
fn day_first(p: &PageText, i: usize) -> Option<(DateCand, usize)> {
    let day = day_of(p, i)?;
    let mut j = skip_punct(p, i, &["."]);
    let mut k = next(p, j)?;
    if p.key(k) == "day" {
        k = next(p, k)?;
        if p.key(k) != "of" {
            return None;
        }
        k = next(p, k)?;
    } else if p.key(k) == "of" {
        k = next(p, k)?;
    }
    let m = month(p.key(k))?;
    j = skip_punct(p, k, &[".", ","]);
    let y = next(p, j)?;
    let year = year_of(p, y)?;
    let d = mk(year, u32::from(m), day)?;
    Some(cand(p, i, y, d, Order::Dmy, conf::DATE_MONTH_NAME))
}

/// `September 12, 2023`, `Sept. 12th, 2023`.
fn month_first(p: &PageText, i: usize) -> Option<(DateCand, usize)> {
    let m = month(p.key(i))?;
    let j = skip_punct(p, i, &["."]);
    let dk = next(p, j)?;
    let day = day_of(p, dk)?;
    let j = skip_punct(p, dk, &[","]);
    let y = next(p, j)?;
    let year = year_of(p, y)?;
    let d = mk(year, u32::from(m), day)?;
    Some(cand(p, i, y, d, Order::Mdy, conf::DATE_MONTH_NAME))
}

/// Reads the file's ambiguous dates once every page is scanned: month-first when the file
/// has more month-first than day-first evidence, else day-first. Confidence is lower when
/// there is no evidence either way.
pub(crate) fn resolve(dates: &mut [DateCand]) {
    let mdy = dates.iter().filter(|d| d.order == Order::Mdy).count();
    let dmy = dates.iter().filter(|d| d.order == Order::Dmy).count();
    for d in dates.iter_mut().filter(|d| d.order == Order::Ambiguous) {
        if mdy > dmy {
            if let Some(alt) = d.alt {
                d.date = alt;
            }
            d.conf = conf::DATE_AMBIGUOUS_EVIDENCE;
        } else if dmy > mdy {
            d.conf = conf::DATE_AMBIGUOUS_EVIDENCE;
        }
    }
}

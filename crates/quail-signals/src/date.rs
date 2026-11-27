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
use crate::phrase::{MAX_SKIPPED, Reach, find_before_wrapped};

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

/// Role keywords before a date (folded phrases). `None` marks a phrase that says the date
/// belongs to another document (`Ugovora zaključenog dana ...`): the search stops there and
/// the date gets no role. When several phrases end on the same word the first listed wins,
/// so longer phrases come before their tails.
const ROLES: &[(&[&str], Option<Role>)] = &[
    (&["effective", "date"], Some(Role::Effective)),
    (&["dated", "effective", "as", "of"], Some(Role::Effective)),
    (&["effective"], Some(Role::Effective)),
    (&["commencement", "date"], Some(Role::Effective)),
    (&["start", "date"], Some(Role::Effective)),
    (&["with", "effect", "from"], Some(Role::Effective)),
    (&["with", "effect", "on"], Some(Role::Effective)),
    (&["stupa", "na", "snagu"], Some(Role::Effective)),
    (&["stupio", "na", "snagu"], Some(Role::Effective)),
    (&["stupanja", "na", "snagu"], Some(Role::Effective)),
    (&["vazi", "od", "dana"], Some(Role::Effective)),
    (&["vazi", "od"], Some(Role::Effective)),
    (&["primenjuje", "se", "od"], Some(Role::Effective)),
    (&["datum", "pocetka"], Some(Role::Effective)),
    (&["expiry", "date"], Some(Role::Expiry)),
    (&["expiration", "date"], Some(Role::Expiry)),
    (&["expires"], Some(Role::Expiry)),
    (&["expire"], Some(Role::Expiry)),
    (&["termination", "date"], Some(Role::Expiry)),
    (&["end", "date"], Some(Role::Expiry)),
    (&["valid", "until"], Some(Role::Expiry)),
    (&["valid", "through"], Some(Role::Expiry)),
    (&["through"], Some(Role::Expiry)),
    (&["ending", "on"], Some(Role::Expiry)),
    (&["end", "on"], Some(Role::Expiry)),
    (&["terminate", "on"], Some(Role::Expiry)),
    (&["terminates", "on"], Some(Role::Expiry)),
    (&["concluding"], Some(Role::Expiry)),
    (&["commencing", "on"], Some(Role::Effective)),
    (&["commencing"], Some(Role::Effective)),
    (&["commence", "on"], Some(Role::Effective)),
    (&["commences", "on"], Some(Role::Effective)),
    (&["dated", "as", "of"], Some(Role::Signed)),
    (&["as", "of"], Some(Role::Effective)),
    (&["entered", "into", "on"], Some(Role::Signed)),
    (&["entered", "into"], Some(Role::Effective)),
    (&["made", "this"], Some(Role::Effective)),
    (&["into", "this"], Some(Role::Effective)),
    (&["made", "on"], Some(Role::Signed)),
    (&["made"], Some(Role::Signed)),
    (&["until"], Some(Role::Expiry)),
    (&["vazi", "do"], Some(Role::Expiry)),
    (&["do", "dana"], Some(Role::Expiry)),
    (&["istice"], Some(Role::Expiry)),
    (&["rok", "vazenja"], Some(Role::Expiry)),
    (&["datum", "isteka"], Some(Role::Expiry)),
    (&["zakljucno", "sa"], Some(Role::Expiry)),
    (&["signed"], Some(Role::Signed)),
    (&["dated"], Some(Role::Signed)),
    (&["executed"], Some(Role::Signed)),
    (&["date", "of", "signature"], Some(Role::Signed)),
    (&["signature", "date"], Some(Role::Signed)),
    (&["datum", "potpisivanja"], Some(Role::Signed)),
    (&["datum", "zakljucenja"], Some(Role::Signed)),
    (&["datum", "zakljucivanja"], Some(Role::Signed)),
    (&["potpisan"], Some(Role::Signed)),
    (&["potpisano"], Some(Role::Signed)),
    (&["potpisali"], Some(Role::Signed)),
    (&["zakljucen", "je", "dana"], Some(Role::Signed)),
    (&["zakljuceno", "je", "dana"], Some(Role::Signed)),
    (&["zakljucen", "dana"], Some(Role::Signed)),
    (&["zakljuceno", "dana"], Some(Role::Signed)),
    (&["zakljucuju", "dana"], Some(Role::Signed)),
    (&["zakljucen"], Some(Role::Signed)),
    (&["zakljuceno"], Some(Role::Signed)),
    (&["zakljucuju"], Some(Role::Signed)),
    (&["zakljucili"], Some(Role::Signed)),
    // A participle of another document: "Ugovora zaključenog dana ...".
    (&["zakljucenog"], None),
    (&["zakljucenom"], None),
    (&["potpisanog"], None),
    (&["potpisanom"], None),
    (&["invoice", "date"], Some(Role::Invoice)),
    (&["date", "of", "invoice"], Some(Role::Invoice)),
    (&["datum", "fakture"], Some(Role::Invoice)),
    (&["datum", "racuna"], Some(Role::Invoice)),
    (&["datum", "izdavanja"], Some(Role::Invoice)),
    (&["issue", "date"], Some(Role::Invoice)),
    (&["date", "of", "issue"], Some(Role::Invoice)),
    (&["due", "date"], Some(Role::Due)),
    (&["payment", "due"], Some(Role::Due)),
    (&["due"], Some(Role::Due)),
    (&["pay", "by"], Some(Role::Due)),
    (&["valuta"], Some(Role::Due)),
    (&["rok", "placanja"], Some(Role::Due)),
    (&["datum", "dospeca"], Some(Role::Due)),
    (&["dospece"], Some(Role::Due)),
];

/// Words and one wrapped line break may sit between a role phrase and its date.
const ROLE_REACH: Reach = Reach { words: 5, lines: 1 };

/// Words that turn a role keyword into another date (`Discount Due Date`).
const ROLE_BLOCKERS: &[&str] = &["discount", "early"];

/// A bare label right before a date (`Date: 7/21/2015`, `Datum: ...`, `U Beogradu, dana
/// 12.03.2024.`): the date of signing when nothing else names a role.
const DATE_LABELS: &[&str] = &["date", "datum", "dana"];

/// Words before a [`DATE_LABELS`] label that make it something else (`od dana`, `do dana`,
/// `the date`).
const LABEL_BLOCKERS: &[&str] = &[
    "od", "do", "roku", "nakon", "pre", "posle", "dana", "the", "such", "that", "any", "which",
    "of", "a", "this",
];

/// Nouns naming an agreement, for telling this agreement's date from a cited one's.
const INSTRUMENTS: &[&str] = &[
    "agreement",
    "agreements",
    "contract",
    "contracts",
    "amendment",
    "letter",
    "lease",
    "license",
    "plan",
    "indenture",
    "memorandum",
    "work",
    "ugovor",
    "ugovora",
    "ugovorom",
    "ugovoru",
    "sporazum",
    "sporazuma",
    "sporazumom",
    "aneks",
    "aneksa",
];

/// Determiners that make the instrument this one.
const THIS: &[&str] = &[
    "this", "ovaj", "ovog", "ovoga", "ovim", "ovom", "ovo", "ova", "ove", "present",
];

/// Determiners that make the instrument another one (`the license agreement dated`, `a
/// Covenant ... Agreement dated`, `both Agreements executed`). `the` right before the noun
/// (`the Agreement`) still means this agreement.
const OTHER: &[&str] = &[
    "the", "that", "a", "an", "such", "its", "his", "her", "their", "any", "each", "both", "said",
    "certain", "prior", "existing", "original", "previous", "attached", "our", "your", "my",
    "other", "separate",
];

/// Words before the instrument noun that make it an earlier one whatever its determiner (`have
/// previously entered into the Agreement dated ...`).
const EARLIER: &[&str] = &["previously", "heretofore", "originally", "formerly"];

/// Words a period may follow inside a sentence: legal forms and other abbreviations of
/// company names and references (`Inc.`, `N.A.`, `S.A.`, `ref.`, `No.`).
const ABBREVIATIONS: &[&str] = &[
    "inc", "ltd", "co", "corp", "llc", "plc", "lp", "llp", "bv", "nv", "ag", "sa", "no", "nos",
    "ref", "jr", "sr", "st", "mr", "ms", "dr", "vs",
];

/// Does the token at `j` end a sentence? A `.` after an abbreviation or a single letter does
/// not (`Bar Trading S.A.`, `Quux Bank, N. A.`); nor does one after an initialism (`U.S.`).
fn sentence_end(p: &PageText, j: usize) -> bool {
    match p.text(j) {
        ";" => true,
        "." => {
            let before = j.checked_sub(1).filter(|&b| p.touch(j) && p.wordlike(b));
            !before.is_some_and(|b| {
                let w = p.key(b);
                w.chars().count() == 1 || w.contains('.') || ABBREVIATIONS.contains(&w)
            })
        }
        _ => false,
    }
}

/// Words searched back from a role phrase for the instrument noun (a parties clause between
/// them: `the Supply Agreement entered into between X Inc. and Y S.A. (a subsidiary of Z)
/// dated ...`).
const CITED_WORDS: usize = 24;

/// Is token `i` a word inside quotes (a defined term: `(the "Agreement")`)?
fn quoted(p: &PageText, i: usize) -> bool {
    let q = |j: usize| matches!(p.text(j), "\"" | "'");
    (i > 0 && q(i - 1)) || q(i + 1)
}

/// Does the role phrase starting at `first` date another instrument than this one? Looks
/// back, within the sentence, for the nearest instrument noun and the determiner before it.
fn cited(p: &PageText, first: usize) -> bool {
    let line = p.line(first);
    let mut words = 0;
    // A verb between the noun and the phrase (`This Amendment to the Statement of Work dated
    // X, is made effective as of Y`): the noun sits in a phrase of the subject, so another
    // instrument there does not decide; the subject's own noun further back does.
    let mut verb = false;
    let mut skipped = 0;
    let mut j = first;
    while j > 0 && words < CITED_WORDS {
        j -= 1;
        if line - p.line(j).min(line) > 3 || sentence_end(p, j) {
            return false;
        }
        if !p.wordlike(j) {
            skipped += 1;
            if skipped > MAX_SKIPPED {
                return false;
            }
            continue;
        }
        words += 1;
        verb = verb || matches!(p.key(j), "is" | "are");
        if !INSTRUMENTS.contains(&p.key(j)) || quoted(p, j) {
            continue;
        }
        match determiner(p, j, line) {
            Some(true) if verb => {}
            Some(other) => return other,
            None => return false,
        }
    }
    false
}

/// What the determiner of the instrument noun at `j` says: `Some(true)` another instrument,
/// `Some(false)` this one, `None` no determiner in reach.
fn determiner(p: &PageText, j: usize, line: u32) -> Option<bool> {
    let mut k = j;
    let mut between = 0;
    let mut skipped = 0;
    while k > 0 && between < 14 {
        k -= 1;
        if line - p.line(k).min(line) > 4 || sentence_end(p, k) || matches!(p.text(k), ":" | ")") {
            return None;
        }
        if !p.wordlike(k) {
            skipped += 1;
            if skipped > MAX_SKIPPED {
                return None;
            }
            continue;
        }
        let w = p.key(k);
        if THIS.contains(&w) {
            return Some(false);
        }
        if EARLIER.contains(&w) {
            return Some(true);
        }
        if OTHER.contains(&w) {
            // `the Agreement` and `This is an Agreement` are this one, unless it was made
            // earlier (`have previously entered into the Agreement`).
            let is_before = prev_word(p, k).is_some_and(|b| p.key(b) == "is");
            let this = (w == "the" && between == 0) || (matches!(w, "a" | "an") && is_before);
            return Some(!this || earlier_before(p, k));
        }
        between += 1;
    }
    None
}

/// An [`EARLIER`] word within the four words before token `k`, in the sentence.
fn earlier_before(p: &PageText, k: usize) -> bool {
    let mut seen = 0;
    let mut j = k;
    // Punctuation is walked over too, at most `MAX_SKIPPED` tokens of it.
    while j > 0 && seen < 4 && k - j <= MAX_SKIPPED {
        j -= 1;
        if sentence_end(p, j) {
            return false;
        }
        if p.wordlike(j) {
            seen += 1;
            if EARLIER.contains(&p.key(j)) {
                return true;
            }
        }
    }
    false
}

/// Is the word before the role phrase starting at `first` a blocker?
fn blocked(p: &PageText, first: usize) -> bool {
    (first.saturating_sub(3)..first)
        .rev()
        .find(|&j| p.wordlike(j))
        .is_some_and(|j| p.line(j) == p.line(first) && ROLE_BLOCKERS.contains(&p.key(j)))
}

/// The word-like token before `i`, over at most three punctuation tokens and one line break.
fn prev_word(p: &PageText, i: usize) -> Option<usize> {
    let line = p.line(i);
    (i.saturating_sub(4)..i)
        .rev()
        .find(|&j| p.wordlike(j))
        .filter(|&j| line - p.line(j).min(line) <= 1)
}

/// `Date: <date>`, `Datum: <date>`, `dana <date>`: the label right before the date.
fn labelled(p: &PageText, i: usize) -> bool {
    let Some(l) = prev_word(p, i) else {
        return false;
    };
    if !DATE_LABELS.contains(&p.key(l)) {
        return false;
    }
    let before = prev_word(p, l).filter(|&b| p.line(b) == p.line(l));
    if p.key(l) == "dana" {
        // `U Beogradu, dana ...` or `Dana ...` at the start of a line; never inside a
        // sentence (`konstatuju da su dana ... zaključili`, `objavljen dana ...`).
        return before.is_none() || (l > 0 && p.text(l - 1) == ",");
    }
    !before.is_some_and(|b| LABEL_BLOCKERS.contains(&p.key(b)))
}

/// Words that may follow a date on its own line (`12.12.2017. god.`).
const YEAR_WORDS: &[&str] = &["godine", "god", "g"];

/// A date standing alone on its line (a letter or cover-page date), with at most a place
/// before it (`Rome, 2 April 2020`) and nothing but punctuation after it.
fn alone_on_line(p: &PageText, a: usize, b: usize) -> bool {
    let line = p.line(a);
    let mut before = 0;
    let mut j = a;
    // At most a two-word place before it: stop at the third word, or a long line made of
    // dates walks back to its start from every date (quadratic; a 3 MB line ran minutes).
    while j > 0 && p.line(j - 1) == line {
        j -= 1;
        if p.wordlike(j) {
            before += 1;
            if before > 2 {
                return false;
            }
        } else if a - j > MAX_SKIPPED {
            return false;
        }
    }
    let place = before == 0 || (before <= 2 && a > 0 && p.text(a - 1) == ",");
    let mut k = b + 1;
    while k < p.len() && p.line(k) == p.line(b) {
        if (p.wordlike(k) && !YEAR_WORDS.contains(&p.key(k))) || k - b > MAX_SKIPPED {
            return false;
        }
        k += 1;
    }
    place
}

/// Lines from the top of the first page where a date alone on its line is the document's.
const HEAD_LINES: u32 = 25;

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

/// The role of the date at tokens `a..=b` and its precision. `first_page` enables the
/// letter-date rule.
fn role_of(p: &PageText, a: usize, b: usize, first_page: bool) -> Option<(Role, f32)> {
    if let Some((role, first)) = find_before_wrapped(p, a, ROLES, ROLE_REACH) {
        let role = role.filter(|_| !blocked(p, first))?;
        // `DATED: ...`, `Effective Date: ...`: a field of this document's head.
        let field = a > 0 && p.text(a - 1) == ":";
        if matches!(role, Role::Effective | Role::Signed) && !field && cited(p, first) {
            return None;
        }
        return Some((role, role_conf(role)));
    }
    if labelled(p, a) {
        return Some((Role::Signed, conf::DATE_ROLE_LABEL));
    }
    if first_page && p.line(a) < HEAD_LINES && alone_on_line(p, a, b) {
        return Some((Role::Signed, conf::DATE_ROLE_HEAD));
    }
    None
}

/// Scans a page for dates; `first_page` is true on the first page with text.
pub(crate) fn scan(p: &PageText, first_page: bool, out: &mut Vec<DateCand>) {
    let mut i = 0;
    while i < p.len() {
        match at(p, i) {
            Some((mut c, last)) => {
                c.role = role_of(p, i, last, first_page);
                out.push(c);
                i = last + 1;
            }
            None => i += 1,
        }
    }
}

/// Next token index if it follows `j` tightly or starts the next line (a date wrapped over
/// a line break: `January` / `22, 2013`).
fn next_wrapped(p: &PageText, j: usize) -> Option<usize> {
    let k = j + 1;
    if k >= p.len() {
        return None;
    }
    if p.tight(k) {
        return Some(k);
    }
    let gap = p.gap_before(k)?;
    (p.line(k) == p.line(j) + 1 && gap.len() <= 3 && gap.trim().is_empty()).then_some(k)
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
    let mut k = next_wrapped(p, j)?;
    if p.key(k) == "day" {
        k = next_wrapped(p, k)?;
        if p.key(k) != "of" {
            return None;
        }
        k = next_wrapped(p, k)?;
    } else if p.key(k) == "of" {
        k = next_wrapped(p, k)?;
    }
    let m = month(p.key(k))?;
    j = skip_punct(p, k, &[".", ","]);
    let y = next_wrapped(p, j)?;
    let year = year_of(p, y)?;
    let d = mk(year, u32::from(m), day)?;
    Some(cand(p, i, y, d, Order::Dmy, conf::DATE_MONTH_NAME))
}

/// `11,2014` read as one number token: day and year glued by a comma.
fn glued_day_year(t: &str) -> Option<(u32, u32)> {
    let (d, y) = t.split_once(',')?;
    let ok = (1..=2).contains(&d.len())
        && y.len() == 4
        && d.bytes().chain(y.bytes()).all(|b| b.is_ascii_digit());
    ok.then(|| Some((d.parse().ok()?, y.parse().ok()?)))?
}

/// `September 12, 2023`, `Sept. 12th, 2023`, `December 11,2014`.
fn month_first(p: &PageText, i: usize) -> Option<(DateCand, usize)> {
    let m = month(p.key(i))?;
    let j = skip_punct(p, i, &["."]);
    let dk = next_wrapped(p, j)?;
    if let Some((day, year)) = glued_day_year(p.text(dk)) {
        let d = mk(year, u32::from(m), day)?;
        return Some(cand(p, i, dk, d, Order::Mdy, conf::DATE_MONTH_NAME));
    }
    let day = day_of(p, dk)?;
    let j = skip_punct(p, dk, &[","]);
    let y = next_wrapped(p, j)?;
    let year = year_of(p, y)?;
    let d = mk(year, u32::from(m), day)?;
    Some(cand(p, i, y, d, Order::Mdy, conf::DATE_MONTH_NAME))
}

/// Months from `a` to `b` (negative when `b` is earlier), ignoring days.
fn months_between(a: Date, b: Date) -> i32 {
    (i32::from(b.y) * 12 + i32::from(b.m)) - (i32::from(a.y) * 12 + i32::from(a.m))
}

/// Document-level role confidence, once every page is scanned and the ambiguous dates are
/// resolved. `early[i]` says date `i` is on one of the first two pages.
///
/// - The first effective-role date on the first two pages is the agreement's own (it is the
///   one picked about 90 % right on the Kleister-NDA and CUAD dev splits), and so is the first
///   signing phrase there: both get the role precision measured for them
///   ([`conf::DATE_ROLE_EFFECTIVE_FIRST`], [`conf::DATE_ROLE_SIGNED_FIRST`]).
/// - Sanity against the most confident effective date: an expiry before it and a signing date
///   two years or more before it are unlikely to be this agreement's (a cited agreement, a
///   prior term, an OCR slip) and keep only a fraction of their role confidence.
pub(crate) fn calibrate(dates: &mut [DateCand], early: &[bool]) {
    let mut effective_seen = false;
    let mut signed_seen = false;
    for (d, &e) in dates.iter_mut().zip(early) {
        let Some((role, rc)) = d.role.as_mut() else {
            continue;
        };
        if !e {
            continue;
        }
        match role {
            Role::Effective if !effective_seen => {
                *rc = conf::DATE_ROLE_EFFECTIVE_FIRST;
                effective_seen = true;
            }
            // Only the phrase rule (`made on`, `dated as of`): labels and head dates keep
            // their own precision.
            Role::Signed if !signed_seen && (*rc - conf::DATE_ROLE_SIGNED).abs() < 1e-6 => {
                *rc = conf::DATE_ROLE_SIGNED_FIRST;
                signed_seen = true;
            }
            _ => {}
        }
    }
    let mut effective: Option<(Date, f32)> = None;
    for d in dates.iter() {
        if let Some((Role::Effective, rc)) = d.role {
            let c = d.conf * rc;
            if effective.is_none_or(|(_, best)| c > best) {
                effective = Some((d.date, c));
            }
        }
    }
    let Some((eff, _)) = effective else {
        return;
    };
    for d in dates.iter_mut() {
        let Some((role, rc)) = d.role.as_mut() else {
            continue;
        };
        match role {
            Role::Expiry if d.date < eff => *rc *= conf::DATE_EXPIRY_BEFORE_EFFECTIVE,
            Role::Signed if months_between(d.date, eff) >= 24 => {
                *rc *= conf::DATE_SIGNED_YEARS_BEFORE;
            }
            _ => {}
        }
    }
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

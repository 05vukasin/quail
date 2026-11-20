//! Keys for document numbers: when a number names one agreement, and how two spellings of
//! it compare.
//!
//! A faithful port of a production contract-audit service's rules (contract numbers,
//! parent-link resolution, version-identity guards). Python's lookarounds are written out by
//! hand so the results are the same character for character; the regex crate has none.
//!
//! - [`normalize_contract_no`]: alphanumeric core, upper-cased, at least 5 characters.
//! - [`is_identifying_contract_key`]: carries a run of three or more significant digits that
//!   is neither a date nor a year standing alone ("Amendment No. 4", "Agreement 2021" do not).
//! - [`is_code_prefixed_number`]: a code word of four letters or more welded to two
//!   significant digits (`QXPSO_78`).
//! - [`fold_zero_padding`]: "QXPSO078" and "QXPSO78" are one key; opening zeros stay.
//! - [`normalize_contract_number`]: the matching key, `""` for a label without a code.
//! - [`invoice_number_key`] / [`invoice_identity_key`]: invoice numbers (3-character floor,
//!   re-issue letter and run padding folded for identity).
//! - [`without_version_tail`], [`unpadded`]: relaxed comparison of a cited number.
//! - [`study_codes`], [`clm_record_ids`]: scoping identifiers.
//!
//! Python `\d` and `str.isdigit` are read as [`char::is_numeric`], `str.isalnum` as
//! [`char::is_alphanumeric`]: identical on the NFKC-normalized text quail feeds in.

use std::sync::LazyLock;

use regex::Regex;

/// A core shorter than this identifies nothing ("A1", "SOW").
pub const MIN_CONTRACT_NO_LENGTH: usize = 5;
/// Significant digits an identifying number carries.
pub const MIN_IDENTIFYING_DIGITS: usize = 3;
const MIN_CODE_LETTERS: usize = 4;
const MIN_CODE_DIGITS: usize = 2;
const YEAR_DIGITS: usize = 4;
const COMPACT_DATE_DIGITS: usize = 8;
/// Invoice counters can be four digits ("5821"): a lower floor than agreements.
pub const MIN_INVOICE_NUMBER_CHARS: usize = 3;

/// A year beside one of these is a date label ("Oct2021", "FY2023"), not a code.
const CALENDAR_WORDS: &[&str] = &[
    "jan",
    "january",
    "feb",
    "february",
    "mar",
    "march",
    "apr",
    "april",
    "may",
    "jun",
    "june",
    "jul",
    "july",
    "aug",
    "august",
    "sep",
    "sept",
    "september",
    "oct",
    "october",
    "nov",
    "november",
    "dec",
    "december",
    "fy",
    "cy",
];

/// A digit run joined to one of these is the ordinal of a document type ("SOW-001").
/// "po" is deliberately absent: a purchase-order number is a real order's number.
const LABEL_WORDS: &[&str] = &[
    "sow",
    "annex",
    "amendment",
    "addendum",
    "appendix",
    "attachment",
    "exhibit",
    "schedule",
    "change",
    "order",
    "wo",
    "no",
    "rev",
    "revision",
    "version",
];

fn is_digit(c: char) -> bool {
    c.is_numeric()
}

fn all(s: &[char], f: impl Fn(char) -> bool) -> bool {
    !s.is_empty() && s.iter().all(|&c| f(c))
}

fn lower(s: &[char]) -> String {
    s.iter().flat_map(|c| c.to_lowercase()).collect()
}

fn is_calendar(s: &[char]) -> bool {
    let l = lower(s);
    CALENDAR_WORDS.contains(&l.as_str())
}

fn is_non_code(s: &[char]) -> bool {
    let l = lower(s);
    CALENDAR_WORDS.contains(&l.as_str()) || LABEL_WORDS.contains(&l.as_str())
}

/// Collision-safe key: the alphanumeric core, upper-cased, so "KW 482917", "kw-482917" and
/// "KW482917" share one key. `""` when the core is shorter than [`MIN_CONTRACT_NO_LENGTH`].
pub fn normalize_contract_no(value: &str) -> String {
    let core = alnum_upper(value);
    if core.chars().count() < MIN_CONTRACT_NO_LENGTH {
        return String::new();
    }
    core
}

fn alnum_upper(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Collapses the zero padding of a digit run written straight after an upper-case ASCII
/// letter (`(?<=[A-Z])0+(\d)` → `\1`): "QXPSO078" → "QXPSO78". Opening zeros stay ("007123").
pub fn fold_zero_padding(key: &str) -> String {
    let c: Vec<char> = key.chars().collect();
    let mut out = String::with_capacity(key.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '0' && i > 0 && c[i - 1].is_ascii_uppercase() {
            let mut j = i;
            while j < c.len() && c[j] == '0' {
                j += 1;
            }
            if j < c.len() && is_digit(c[j]) {
                out.push(c[j]);
                i = j + 1;
                continue;
            }
            if j - i >= 2 {
                // `0+` backs off one zero so `(\d)` takes the last one.
                out.push('0');
                i = j;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Can `value` name ONE agreement? It needs a run of three or more significant digits that
/// is neither a date nor a year standing on its own.
pub fn is_identifying_contract_key(value: &str) -> bool {
    let toks = number_tokens(&strip_dates(value));
    (0..toks.len()).any(|i| all(&toks[i], is_digit) && identifies(&toks, i))
}

/// Is `value` a code word (four letters or more, not a label) welded to a run of at least two
/// significant digits (`QXPSO_78`)?
pub fn is_code_prefixed_number(value: &str) -> bool {
    let toks = number_tokens(&strip_dates(value));
    (0..toks.len()).any(|i| all(&toks[i], is_digit) && is_short_code(&toks, i))
}

/// The matching key of a contract number: alphanumerics, zero padding inside the code
/// folded, lower-cased. `""` for a label without an identifying code ("Amendment No. 3").
pub fn normalize_contract_number(value: &str) -> String {
    if !is_identifying_contract_key(value) && !is_code_prefixed_number(value) {
        return String::new();
    }
    fold_zero_padding(&alnum_upper(value)).to_lowercase()
}

/// Letters and digits upper-cased; `""` without a digit or with fewer than
/// [`MIN_INVOICE_NUMBER_CHARS`] characters.
pub fn invoice_number_key(value: &str) -> String {
    let core = alnum_upper(value);
    if core.chars().count() < MIN_INVOICE_NUMBER_CHARS || !core.chars().any(is_digit) {
        return String::new();
    }
    core
}

/// [`invoice_number_key`] behind the agreement guard, with a trailing re-issue letter
/// ("SI-731204A") and the leading zeros of every digit run folded away.
pub fn invoice_identity_key(value: &str) -> String {
    if !is_identifying_contract_key(value) {
        return String::new();
    }
    let core = invoice_number_key(value);
    if core.is_empty() {
        return core;
    }
    let c: Vec<char> = core.chars().collect();
    let reissue = c.len() >= 2 && c[c.len() - 1].is_alphabetic() && is_digit(c[c.len() - 2]);
    let folded = if reissue { &c[..c.len() - 1] } else { &c[..] };
    fold_run_leading_zeros(folded)
}

/// `(?<!\d)0+(\d)` → `\1`: the leading zeros of a digit run, never zeros inside one.
fn fold_run_leading_zeros(c: &[char]) -> String {
    let mut out = String::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '0' && (i == 0 || !is_digit(c[i - 1])) {
            let mut j = i;
            while j < c.len() && c[j] == '0' {
                j += 1;
            }
            if j < c.len() && is_digit(c[j]) {
                out.push(c[j]);
                i = j + 1;
                continue;
            }
            if j - i >= 2 {
                out.push('0');
                i = j;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

static VERSION_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:rev|ver|v|r)\d{1,3}$").unwrap_or_else(|_| unreachable!()));

/// The key without the tail an amendment adds for the revision it creates
/// (`(?:rev|ver|v|r)\d{1,3}$`): "kw482917v14" → "kw482917".
pub fn without_version_tail(key: &str) -> String {
    VERSION_TAIL.replace(key, "").into_owned()
}

/// The key without the zeros a filing pads its front with (`^0+(?=\w)`).
pub fn unpadded(key: &str) -> &str {
    let zeros = key.bytes().take_while(|&b| b == b'0').count();
    if zeros == 0 {
        return key;
    }
    if zeros == key.len() {
        return &key[zeros - 1..];
    }
    let rest = &key[zeros..];
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        rest
    } else {
        &key[zeros - 1..]
    }
}

static STUDY_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)D[0-9]{3}[0-9A-Z]C[0-9]{5}").unwrap_or_else(|_| unreachable!())
});
static CLM_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)con_([0-9]{3,7})").unwrap_or_else(|_| unreachable!()));

/// Clinical study codes (`(?<![A-Z0-9])D\d{3}[0-9A-Z]C\d{5}(?!\d)` on upper-cased text) with
/// their byte ranges, upper-cased.
pub fn study_codes(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(m) = STUDY_CODE.find_at(text, at) {
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if before.is_some_and(|c| c.is_ascii_alphanumeric())
            || after.is_some_and(|c| c.is_ascii_digit())
        {
            at = m.start() + 1;
            continue;
        }
        out.push((m.start(), m.end(), m.as_str().to_ascii_uppercase()));
        at = m.end();
    }
    out
}

/// CLM record ids (`(?<![A-Za-z0-9])con_(\d{3,7})(?!\d)`, any case) with their byte ranges,
/// written `con_<digits>`.
pub fn clm_record_ids(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(c) = CLM_ID.captures_at(text, at) {
        let (Some(m), Some(d)) = (c.get(0), c.get(1)) else {
            break;
        };
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if before.is_some_and(|c| c.is_ascii_alphanumeric())
            || after.is_some_and(|c| c.is_ascii_digit())
        {
            at = m.start() + 1;
            continue;
        }
        out.push((m.start(), m.end(), format!("con_{}", d.as_str())));
        at = m.end();
    }
    out
}

/// `[a-z]+|\d+|[^a-z\d]` (case-insensitive): ASCII letter runs, digit runs, single others.
fn number_tokens(value: &str) -> Vec<Vec<char>> {
    let mut out: Vec<Vec<char>> = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut cls = 0u8; // 1 letters, 2 digits
    for ch in value.chars() {
        let k = if ch.is_ascii_alphabetic() {
            1
        } else if is_digit(ch) {
            2
        } else {
            0
        };
        if k != 0 && k == cls {
            cur.push(ch);
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cls = k;
        if k == 0 {
            out.push(vec![ch]);
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Replaces every calendar date of the raw value by a space, as `_DATE.sub(" ", value)`:
/// `(?<![a-z\d-])(?:\d{1,2}[./-]\d{1,2}[./-](?:\d{2}|(?:19|20)\d{2})|(?:19|20)\d{2}[./-]\d{1,2}[./-]\d{1,2})(?!\d)`.
fn strip_dates(value: &str) -> String {
    let c: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < c.len() {
        let behind_ok =
            i == 0 || !(c[i - 1].is_ascii_alphabetic() || is_digit(c[i - 1]) || c[i - 1] == '-');
        if behind_ok && let Some(end) = date_at(&c, i) {
            out.push(' ');
            i = end;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// End of a date starting at `i`, trying the alternatives in the order Python's backtracking
/// does.
fn date_at(c: &[char], i: usize) -> Option<usize> {
    let digits = |from: usize, n: usize| -> bool {
        from + n <= c.len() && c[from..from + n].iter().all(|&x| is_digit(x))
    };
    let sep = |at: usize| at < c.len() && matches!(c[at], '.' | '/' | '-');
    let free = |at: usize| at >= c.len() || !is_digit(c[at]);
    let century =
        |at: usize| digits(at, 4) && matches!((c[at], c[at + 1]), ('1', '9') | ('2', '0'));
    // d{1,2} sep d{1,2} sep (d{2} | (19|20)d{2})
    for a in [2, 1] {
        if !digits(i, a) || !sep(i + a) {
            continue;
        }
        let j = i + a + 1;
        for b in [2, 1] {
            if !digits(j, b) || !sep(j + b) {
                continue;
            }
            let k = j + b + 1;
            if digits(k, 2) && free(k + 2) {
                return Some(k + 2);
            }
            if century(k) && free(k + 4) {
                return Some(k + 4);
            }
        }
    }
    // (19|20)d{2} sep d{1,2} sep d{1,2}
    if century(i) && sep(i + 4) {
        let j = i + 5;
        for b in [2, 1] {
            if !digits(j, b) || !sep(j + b) {
                continue;
            }
            let k = j + b + 1;
            for d in [2, 1] {
                if digits(k, d) && free(k + d) {
                    return Some(k + d);
                }
            }
        }
    }
    None
}

fn is_alnum_tok(t: &[char]) -> bool {
    all(t, char::is_alphanumeric)
}

/// Segments a digit run touches directly or across one joint character. A space is never a
/// joint ("SOW 001" stays an ordinal).
fn joined(toks: &[Vec<char>], pos: usize, code_joints: bool) -> Vec<&[char]> {
    let mut segs: Vec<&[char]> = Vec::new();
    let is_joint =
        |t: &[char]| t.len() == 1 && (matches!(t[0], '-' | '/') || (code_joints && t[0] == '_'));
    for step in [-1isize, 1] {
        let Some(near) = pos.checked_add_signed(step).filter(|&n| n < toks.len()) else {
            continue;
        };
        if is_alnum_tok(&toks[near]) {
            segs.push(&toks[near]);
        } else if is_joint(&toks[near])
            && let Some(beyond) = near.checked_add_signed(step).filter(|&b| b < toks.len())
            && is_alnum_tok(&toks[beyond])
        {
            segs.push(&toks[beyond]);
        }
    }
    segs
}

fn is_code_word(seg: &[char]) -> bool {
    all(seg, char::is_alphabetic) && !is_non_code(seg)
}

fn is_code_segment(seg: &[char]) -> bool {
    if all(seg, is_digit) {
        return seg.len() >= MIN_IDENTIFYING_DIGITS;
    }
    !is_calendar(seg)
}

fn is_year(run: &[char]) -> bool {
    run.len() == YEAR_DIGITS && matches!((run[0], run[1]), ('1', '9') | ('2', '0'))
}

fn is_compact_date(run: &[char]) -> bool {
    if run.len() != COMPACT_DATE_DIGITS || !is_year(&run[..YEAR_DIGITS]) {
        return false;
    }
    let two = |a: char, b: char| Some(a.to_digit(10)? * 10 + b.to_digit(10)?);
    let (Some(month), Some(day)) = (two(run[4], run[5]), two(run[6], run[7])) else {
        return false;
    };
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

fn identifies(toks: &[Vec<char>], pos: usize) -> bool {
    let run = &toks[pos][..];
    let glued = joined(toks, pos, true).into_iter().any(is_code_word);
    let significant = if glued {
        run.len()
    } else {
        run.len() - run.iter().take_while(|&&c| c == '0').count()
    };
    if significant < MIN_IDENTIFYING_DIGITS {
        return false;
    }
    if is_year(run) || is_compact_date(run) {
        return joined(toks, pos, false).into_iter().any(is_code_segment);
    }
    true
}

fn is_short_code(toks: &[Vec<char>], pos: usize) -> bool {
    let run = &toks[pos];
    if run.len() - run.iter().take_while(|&&c| c == '0').count() < MIN_CODE_DIGITS {
        return false;
    }
    joined(toks, pos, true)
        .into_iter()
        .any(|seg| seg.len() >= MIN_CODE_LETTERS && is_code_word(seg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_and_floor() {
        assert_eq!(normalize_contract_no("KW 482917"), "KW482917");
        assert_eq!(normalize_contract_no("kw-482917"), "KW482917");
        assert_eq!(normalize_contract_no("A1"), "");
        assert_eq!(normalize_contract_no("SOW"), "");
    }

    #[test]
    fn identifying_rules() {
        assert!(is_identifying_contract_key("KW482917"));
        assert!(is_identifying_contract_key("114/23-1"));
        assert!(!is_identifying_contract_key("Amendment No. 4"));
        assert!(!is_identifying_contract_key("Annex 5"));
        assert!(!is_identifying_contract_key("Agreement 2021"));
        assert!(!is_identifying_contract_key("Oct2021"));
        assert!(!is_identifying_contract_key("2025-04-18"));
        assert!(!is_identifying_contract_key("10.27.22"));
        assert!(!is_identifying_contract_key("Amendment_20221130"));
        assert!(!is_identifying_contract_key("No. 001"));
        assert!(!is_identifying_contract_key("SOW-001"));
        assert!(is_identifying_contract_key("QRX001"));
        assert!(is_identifying_contract_key("QRX-001"));
        assert!(is_identifying_contract_key("QD-2047"));
        assert!(is_identifying_contract_key("YY-RS-20240611"));
        assert!(is_identifying_contract_key("SR-2019-12-345"));
        assert!(is_identifying_contract_key("PO/007"));
    }

    #[test]
    fn short_codes_and_padding() {
        assert!(is_code_prefixed_number("QXPSO_78"));
        assert!(!is_code_prefixed_number("Amendment 12"));
        assert_eq!(fold_zero_padding("QXPSO078"), "QXPSO78");
        assert_eq!(fold_zero_padding("007123"), "007123");
        assert_eq!(fold_zero_padding("A000"), "A0");
        assert_eq!(normalize_contract_number("QXPSO_078"), "qxpso78");
        assert_eq!(normalize_contract_number("QXPSO_78"), "qxpso78");
        assert_eq!(normalize_contract_number("KW-482917 "), "kw482917");
        assert_eq!(normalize_contract_number("Amendment No. 3"), "");
    }

    #[test]
    fn invoices() {
        assert_eq!(invoice_number_key("5821"), "5821");
        assert_eq!(invoice_number_key("12"), "");
        assert_eq!(invoice_number_key("ABCD"), "");
        assert_eq!(invoice_identity_key("SI-731204A"), "SI731204");
        assert_eq!(invoice_identity_key("INVXY001533"), "INVXY1533");
        assert_eq!(invoice_identity_key("1002"), "1002");
        assert_eq!(invoice_identity_key("2021"), "");
    }

    #[test]
    fn relaxed_forms() {
        assert_eq!(without_version_tail("kw482917v14"), "kw482917");
        assert_eq!(without_version_tail("abc123rev2"), "abc123");
        assert_eq!(without_version_tail("ver1234"), "ver1234");
        assert_eq!(unpadded("0071234560"), "71234560");
        assert_eq!(unpadded("000"), "0");
    }

    #[test]
    fn scoping_ids() {
        let s = study_codes("Study D123AC45678 and xD4567C12345 and D890XC123456");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].2, "D123AC45678");
        let c = clm_record_ids("see CON_4821, acon_1234 and con_12345678");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].2, "con_4821");
    }
}

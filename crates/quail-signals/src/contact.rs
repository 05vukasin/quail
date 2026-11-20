//! Contacts: e-mail domains, web domains, phone numbers (E.164).
//!
//! E-mail addresses and URLs come from `quail-text` tokens ([`TokenKind::Email`],
//! [`TokenKind::Url`]); only their domain is kept, so no personal address leaves the file.
//! Free-mail domains (`gmail.com`, ...) identify a person, not a company, and are dropped
//! (the list of sada's `GENERIC_EMAIL_DOMAINS`).
//!
//! Phone numbers are read from a bounded run of digits, spaces and `+ ( ) - . /`:
//! international numbers (`+381 11 123 4567`, `00381 ...`, `+44 (0)20 7946 0958`) always;
//! domestic ones only after a phone label, as Serbian (`011/123-4567` on a Serbian page or
//! with a slash) or North American (`(212) 555-1234`, `212-555-1234`), whose parenthesised
//! form is distinctive enough to stand without a label. Other domestic numbers are dropped:
//! their country is unknown.

use quail_core::Value;
use quail_text::token::TokenKind;

use crate::conf;
use crate::keys;
use crate::page::{PageText, span};
use crate::phrase::{Reach, find_before};
use crate::scan::Cand;

/// Free-mail hosts, verbatim from sada `GENERIC_EMAIL_DOMAINS`.
pub const GENERIC_EMAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "hotmail.co.uk",
    "live.com",
    "msn.com",
    "yahoo.com",
    "yahoo.co.uk",
    "ymail.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "protonmail.com",
    "proton.me",
    "aol.com",
    "gmx.com",
    "gmx.net",
    "mail.com",
    "zoho.com",
    "yandex.com",
    "fastmail.com",
];

/// Top-level domains accepted for a bare domain without `www.` or a scheme (`firma.rs`).
const BARE_TLDS: &[&str] = &[
    "com", "net", "org", "info", "biz", "eu", "io", "rs", "hr", "ba", "me", "mk", "si", "de", "at",
    "ch", "fr", "it", "es", "nl", "be", "pl", "uk", "us", "ca", "cz", "sk", "hu", "ro", "bg", "gr",
    "se", "dk", "fi", "ie", "pt", "lu", "gov", "edu", "int",
];

const PHONE_LABELS: &[(&[&str], ())] = &[
    (&["tel"], ()),
    (&["telefon"], ()),
    (&["telephone"], ()),
    (&["phone"], ()),
    (&["ph"], ()),
    (&["fax"], ()),
    (&["faks"], ()),
    (&["telefaks"], ()),
    (&["mob"], ()),
    (&["mobile"], ()),
    (&["mobilni"], ()),
    (&["cell"], ()),
    (&["gsm"], ()),
];

const PHONE_REACH: Reach = Reach { words: 2, lines: 0 };

/// Normalized domain key: lowercase, no `www.`, no trailing dot. `None` if it does not look
/// like a domain (needs a dot and an alphabetic TLD of two or more letters).
pub(crate) fn domain_key(host: &str) -> Option<String> {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    let h = h.strip_prefix("www.").unwrap_or(&h);
    let (_, tld) = h.rsplit_once('.')?;
    let ok_labels = h.split('.').all(|l| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_alphanumeric() || c == '-')
    });
    (ok_labels && tld.len() >= 2 && tld.chars().all(char::is_alphabetic)).then(|| h.to_owned())
}

/// A domain written without `www.` or a scheme (`firma.rs`, `amish-handscraped.com`):
/// ASCII, one case throughout (`Ref.No`, `S.NO` are abbreviations), a common TLD, a name
/// of two or more characters. Words joined by touching hyphens before it belong to it.
fn bare_domain(p: &PageText, i: usize) -> Option<(String, usize)> {
    let t = p.text(i);
    let uniform =
        t.bytes().all(|b| !b.is_ascii_uppercase()) || t.bytes().all(|b| !b.is_ascii_lowercase());
    if !t.is_ascii() || !uniform {
        return None;
    }
    let mut start = i;
    while start >= 2
        && i - start < 8
        && p.text(start - 1) == "-"
        && p.touch(start)
        && p.touch(start - 1)
        && p.kind(start - 2)
            .is_some_and(|k| matches!(k, TokenKind::Word | TokenKind::Alnum | TokenKind::Number))
    {
        start -= 2;
    }
    let mut full = String::new();
    for j in start..=i {
        full.push_str(p.text(j));
    }
    let d = domain_key(&full)?;
    let (name, tld) = d.rsplit_once('.')?;
    let first = name.rsplit('.').next().unwrap_or("");
    (BARE_TLDS.contains(&tld) && first.len() >= 2).then_some((d, start))
}

/// Host of a URL: scheme, user info, port, path dropped.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    host.split(':').next().unwrap_or("")
}

/// Scans a page for e-mail domains, web domains and phone numbers.
pub(crate) fn scan(p: &PageText, serbian: bool, out: &mut Vec<Cand>) {
    let mut i = 0;
    while i < p.len() {
        let sp = p.span(i, i);
        let t = p.text(i);
        match p.kind(i) {
            Some(TokenKind::Email) => {
                let host = t.rsplit_once('@').map_or("", |(_, h)| h);
                if let Some(d) = domain_key(host)
                    && !GENERIC_EMAIL_DOMAINS.contains(&d.as_str())
                {
                    out.push(Cand::new(
                        keys::EMAIL_DOMAIN,
                        Value::text(d),
                        conf::EMAIL,
                        sp,
                    ));
                }
            }
            Some(TokenKind::Url) => {
                if let Some(d) = domain_key(url_host(t)) {
                    out.push(Cand::new(keys::WEB_DOMAIN, Value::text(d), conf::URL, sp));
                }
            }
            Some(TokenKind::Word) if t.contains('.') => {
                let lower = t.to_ascii_lowercase();
                if lower.starts_with("www.") {
                    if let Some(d) = domain_key(&lower) {
                        out.push(Cand::new(keys::WEB_DOMAIN, Value::text(d), conf::URL, sp));
                    }
                } else if let Some((d, s)) = bare_domain(p, i) {
                    out.push(Cand::new(
                        keys::WEB_DOMAIN,
                        Value::text(d),
                        conf::WEB_BARE,
                        p.span(s, i),
                    ));
                }
            }
            _ => {}
        }
        if let Some(end) = phone(p, i, serbian, out) {
            // Skip the tokens the number covered.
            while i + 1 < p.len() && p.toks[i + 1].start < end {
                i += 1;
            }
        }
        i += 1;
    }
}

/// Two-digit country calling codes (ITU-T E.164); `1` and `7` are one digit, the rest three.
const CC2: &[u32] = &[
    20, 27, 30, 31, 32, 33, 34, 36, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 51, 52, 53, 54, 55, 56,
    57, 58, 60, 61, 62, 63, 64, 65, 66, 81, 82, 84, 86, 90, 91, 92, 93, 94, 95, 98,
];

fn cc_len(d: &[u8]) -> Option<usize> {
    let first = *d.first()?;
    if first == b'0' {
        return None;
    }
    if matches!(first, b'1' | b'7') {
        return Some(1);
    }
    let two = u32::from(d.get(1)?.checked_sub(b'0')?) + u32::from(first - b'0') * 10;
    if CC2.contains(&two) {
        Some(2)
    } else {
        // Three-digit codes; zone 0 and codes starting 28/80/83/89/99x are unassigned-ish but
        // rare enough not to matter here.
        Some(3)
    }
}

/// A phone number starting at token `i`: pushes it and returns the byte end.
fn phone(p: &PageText, i: usize, serbian: bool, out: &mut Vec<Cand>) -> Option<usize> {
    let t = p.text(i);
    let kind = p.kind(i)?;
    let intl_plus = kind == TokenKind::Number && t.starts_with('+');
    let intl_00 = kind == TokenKind::Number && t.starts_with("00") && t.len() >= 4;
    let paren = kind == TokenKind::Punct && t == "(";
    let domestic = kind == TokenKind::Number && t.starts_with('0') && !intl_00;
    let nanp_plain = kind == TokenKind::Number && t.len() >= 12 && t.as_bytes()[0] != b'0';
    if !(intl_plus || intl_00 || paren || domestic || nanp_plain) {
        return None;
    }
    let start = p.toks[i].start;
    let bytes = p.norm.as_bytes();
    // Bounded run of phone characters on this line, no double spaces.
    let mut end = start;
    let mut k = start;
    while k < bytes.len() && k - start < 28 {
        let c = bytes[k];
        let ok = c.is_ascii_digit()
            || matches!(c, b'-' | b'.' | b'/' | b'(' | b')')
            || (c == b'+' && k == start)
            || (c == b' ' && bytes.get(k + 1).is_some_and(|n| *n != b' '));
        if !ok {
            break;
        }
        if c.is_ascii_digit() {
            end = k + 1;
        }
        k += 1;
    }
    // Glued to a letter or digit beyond the bound: not a phone number.
    if bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let raw = p.norm.get(start..end)?;
    let cleaned = raw.replace("(0)", "");
    let digits: Vec<u8> = cleaned.bytes().filter(u8::is_ascii_digit).collect();
    let labeled = find_before(p, i, PHONE_LABELS, PHONE_REACH).is_some();
    let e164 = |d: &[u8]| {
        let mut s = String::with_capacity(d.len() + 1);
        s.push('+');
        s.extend(d.iter().map(|&b| char::from(b)));
        s
    };
    let (value, conf) = if intl_plus || intl_00 {
        let d = if intl_00 { &digits[2..] } else { &digits[..] };
        let cl = cc_len(d)?;
        let mut national: Vec<u8> = d.get(cl..)?.to_vec();
        if national.first() == Some(&b'0') {
            national.remove(0);
        }
        let total = cl + national.len();
        if !(8..=15).contains(&total) || national.len() < 6 {
            return None;
        }
        let mut all = d[..cl].to_vec();
        all.extend(national);
        let c = if labeled {
            conf::PHONE_INTL_LABELED
        } else {
            conf::PHONE_INTL
        };
        (e164(&all), c)
    } else if let Some(v) = nanp(&cleaned, &digits) {
        let paren_form = cleaned.starts_with('(');
        let c = if labeled {
            conf::PHONE_DOMESTIC_LABELED
        } else if paren_form {
            conf::PHONE_NANP_PAREN
        } else {
            return None;
        };
        (e164(&v), c)
    } else if domestic && labeled && (serbian || raw.contains('/')) {
        // Serbian: 0 + area/operator + subscriber, 9 or 10 digits in all.
        if !(9..=10).contains(&digits.len()) || digits.get(1) == Some(&b'0') {
            return None;
        }
        let mut all = b"381".to_vec();
        all.extend(&digits[1..]);
        (e164(&all), conf::PHONE_DOMESTIC_LABELED)
    } else {
        return None;
    };
    out.push(Cand::new(
        keys::PHONE,
        Value::text(value),
        conf,
        span(start, end),
    ));
    Some(end)
}

/// North American number: `(AAA) BBB-CCCC`, `AAA-BBB-CCCC`, `AAA.BBB.CCCC`, optional
/// leading `1-`. Returns `1` + the ten digits.
fn nanp(text: &str, digits: &[u8]) -> Option<Vec<u8>> {
    let d = match digits.len() {
        10 => digits,
        11 if digits[0] == b'1' => &digits[1..],
        _ => return None,
    };
    // Shape: three groups 3-3-4 separated by one of `) - .` and optional spaces.
    let groups: Vec<&str> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .collect();
    let shape: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    let ok_shape = matches!(shape.as_slice(), [3, 3, 4] | [1, 3, 3, 4]);
    if !ok_shape || !(b'2'..=b'9').contains(&d[0]) || !(b'2'..=b'9').contains(&d[3]) {
        return None;
    }
    if text.contains('/') {
        return None;
    }
    let mut v = vec![b'1'];
    v.extend_from_slice(d);
    Some(v)
}

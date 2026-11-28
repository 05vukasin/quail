//! Kind mentions on a page: a kind phrase ("Ugovor", "Purchase Order", "MSA") with what
//! surrounds it on its line: a trigger before it ("pursuant to", "uz"), a number label and a
//! number after it ("br. 114/23", "No. 3", "#12"), a date ("dated 1 March 2021").

use quail_core::{Date, Zone};

use crate::kind::{Kind, Relation};
use crate::number::{invoice_identity_key, is_identifying_contract_key, normalize_contract_number};
use crate::patterns::{Patterns, roman};
use crate::prep::Prepared;

/// Lines at the top of the first page that count as its head (title, number fields).
pub(crate) const HEAD_LINES: u32 = 15;
/// Lines at the bottom of a page that count as its footer when the extractor gives no zones.
pub(crate) const FOOT_LINES: u32 = 3;
const MAX_RAW: usize = 48;
const LOOKBACK: usize = 6;

const LABELS: &[&str] = &[
    "no",
    "nr",
    "number",
    "num",
    "br",
    "broj",
    "id",
    "ref",
    "reference",
    "nos",
];
const SELF_WORDS: &[&str] = &[
    "this",
    "ovaj",
    "ovog",
    "ovim",
    "ovom",
    "ovoga",
    "ova",
    "ove",
    "ovu",
    "ovo",
    "ovih",
    "present",
    "predmetni",
    "predmetnog",
    "predmetnom",
    "predmetnim",
    "hereby",
    "herein",
];
const BANK_WORDS: &[&str] = &[
    "tekuci",
    "ziro",
    "bankovni",
    "devizni",
    "namenski",
    "poslovni",
    "tekuceg",
    "ziro-racun",
    "bank",
    "na",
];
const NOT_ORDERS: &[&str] = &[
    "executive",
    "court",
    "administrative",
    "protective",
    "consent",
    "restraining",
    "standing",
    "work",
    "in",
];
const NOT_VALUES: &[&str] = &[
    "od", "of", "dated", "from", "the", "i", "and", "za", "dana", "date", "to", "uz", "na",
];

/// A number read after a label.
#[derive(Clone, Debug)]
pub(crate) struct Number {
    /// As written (normalized text).
    pub raw: String,
    /// Matching key (`""` when the number identifies nothing).
    pub key: String,
    /// Normalized byte range.
    pub ns: usize,
    pub ne: usize,
}

/// One kind mention.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // independent facts about one mention
pub(crate) struct Mention {
    pub kind: Kind,
    pub trigger: Option<Relation>,
    pub self_ref: bool,
    pub label: bool,
    pub number: Option<Number>,
    pub ordinal: Option<i64>,
    pub date: Option<Date>,
    /// In the head of the first page, a header/title zone, or a footer.
    pub own_region: bool,
    /// In the head of the first page or a header/title zone.
    pub head: bool,
    pub first_page: bool,
    /// The article right before the kind ("the Agreement").
    pub definite: bool,
    /// Normalized byte range of the whole mention.
    pub ns: usize,
    pub ne: usize,
}

pub(crate) fn is_label(k: &str) -> bool {
    LABELS.contains(&k)
}

fn is_alpha_word(k: &str) -> bool {
    !k.is_empty() && k.chars().all(char::is_alphabetic)
}

/// Where a token sits: (own region, head).
pub(crate) fn region(pp: &Prepared<'_>, i: usize) -> (bool, bool) {
    let Some(t) = pp.toks.get(i) else {
        return (false, false);
    };
    let zone = pp.zone_at(t.ns);
    let head = (pp.page.index == 0 && pp.top && t.line < HEAD_LINES)
        || matches!(zone, Zone::Header | Zone::Title);
    let foot = zone == Zone::Footer
        || (zone == Zone::Unknown
            && pp.bottom
            && t.line + FOOT_LINES >= pp.lines
            && pp.lines > FOOT_LINES * 3);
    (head || foot, head)
}

/// Mentions on one page, in order.
pub(crate) fn mentions(p: &Patterns, pp: &Prepared<'_>) -> Vec<Mention> {
    let mut out = Vec::new();
    for m in p.kind.find_iter(&pp.keys) {
        let (Some(a), Some(b)) = (pp.tok_starting(m.start()), pp.tok_ending(m.end())) else {
            continue;
        };
        if let Some(x) = mention(p, pp, a, b, m.as_str()) {
            out.push(x);
        }
    }
    out
}

fn mention(p: &Patterns, pp: &Prepared<'_>, a: usize, b: usize, phrase: &str) -> Option<Mention> {
    let kind = Kind::of_phrase(phrase)?;
    // "po" is the Serbian "per" unless written in capitals.
    if (phrase == "po" || phrase == "p.o")
        && (pp.text(a).chars().any(char::is_lowercase) || pp.key(b + 1) == "box")
    {
        return None;
    }
    let prev = (a > 0 && pp.same_line(a - 1, a)).then(|| pp.key(a - 1));
    if kind == Kind::Invoice
        && phrase.starts_with("racun")
        && prev.is_some_and(|k| BANK_WORDS.contains(&k))
    {
        return None;
    }
    // "Executive Order 13224", "court order": not a commercial order.
    if kind == Kind::Order && prev.is_some_and(|k| NOT_ORDERS.contains(&k)) {
        return None;
    }
    let self_ref = prev.is_some_and(|k| SELF_WORDS.contains(&k));
    let label_before = prev.is_some_and(is_label);
    let definite = prev == Some("the");
    let (line_a, _) = pp.line_bounds(a);
    let lb = line_a.max(a.saturating_sub(LOOKBACK));
    let trigger = if lb < a {
        let ks = pp.toks[lb].ks;
        let ke = pp.toks[a - 1].ke;
        pp.keys.get(ks..ke).and_then(|s| p.trigger(s))
    } else {
        None
    };

    let (number, ordinal, label, end_tok) = read_number(p, pp, kind, a, b, label_before);
    if let Some(n) = &number
        && kind == Kind::Invoice
        && p.bank_account.is_match(&n.raw)
    {
        return None;
    }
    if let Some(n) = &number {
        // "NDA No. 21-748" is a (new drug) application; "EXHIBIT 10.12" an exhibit index of
        // a securities filing.
        if phrase == "nda" {
            return None;
        }
        if kind == Kind::Annex && is_filing_exhibit(&n.raw) {
            return None;
        }
    }
    let ord_end = end_tok.filter(|_| number.is_none()).map(|t| pp.toks[t].ne);
    let after = number
        .as_ref()
        .map_or(ord_end.unwrap_or(pp.toks[b].ne), |n| n.ne);
    let date = date_after(p, pp, end_tok.unwrap_or(b), after);
    let (own_region, head) = region(pp, a);
    let ns = pp.toks[if label_before { a - 1 } else { a }].ns;
    Some(Mention {
        kind,
        trigger,
        self_ref,
        label,
        number,
        ordinal,
        date: date.map(|d| d.0),
        own_region,
        head,
        first_page: pp.page.index == 0,
        definite,
        ns,
        ne: date.map_or(after, |d| d.1).max(after),
    })
}

type Read = (Option<Number>, Option<i64>, bool, Option<usize>);

/// The number after a kind phrase (tokens `a..=b`), its ordinal, whether a label was seen and
/// the last token used.
fn read_number(
    p: &Patterns,
    pp: &Prepared<'_>,
    kind: Kind,
    a: usize,
    b: usize,
    label_before: bool,
) -> Read {
    let n = pp.toks.len();
    let mut i = b + 1;
    let mut label = label_before;
    if i >= n {
        return (None, None, label, None);
    }
    // A code glued to its kind: "PO-4500012345", "INV-2023-001", "SOW-001".
    let glue = pp.norm.get(pp.toks[b].ne..pp.toks[i].ns).unwrap_or(" ");
    if a == b
        && pp.key(a).len() <= 4
        && !glue.is_empty()
        && glue.chars().all(|c| matches!(c, '-' | '_' | '/'))
    {
        return value_at(p, pp, kind, a, pp.toks[a].ns, false)
            .map_or((None, None, label, None), |(num, ord, last)| {
                (Some(num), ord, false, Some(last))
            });
    }
    if !label {
        let mut j = i;
        let mut gap = 0;
        while j < n
            && pp.same_line(b, j)
            && gap < 4
            && is_alpha_word(pp.key(j))
            && !is_label(pp.key(j))
        {
            j += 1;
            gap += 1;
        }
        if j < n && pp.same_line(b, j) && is_label(pp.key(j)) {
            label = true;
            i = j + 1;
        }
    }
    if i >= n {
        return (None, None, label, None);
    }
    let prev_end = pp.toks[i - 1].ne;
    let gap = pp.norm.get(prev_end..pp.toks[i].ns).unwrap_or("");
    let gap_ok = if label {
        gap.len() <= 8
            && gap.matches('\n').count() <= 1
            && gap
                .chars()
                .all(|c| c.is_whitespace() || matches!(c, '.' | ':' | '#' | '-' | '°' | 'º' | '='))
    } else {
        gap.len() <= 3 && !gap.contains('\n') && gap.chars().all(|c| c == ' ' || c == '#')
    };
    if !gap_ok || NOT_VALUES.contains(&pp.key(i)) {
        return (None, None, label, None);
    }
    if let Some((num, ord, last)) = value_at(p, pp, kind, i, pp.toks[i].ns, label) {
        return (Some(num), ord, label, Some(last));
    }
    // Roman ordinals: "Aneks II", "Amendment No. III".
    let t = pp.text(i);
    let ord = (t.chars().all(|c| c.is_ascii_uppercase())
        && matches!(
            kind,
            Kind::Amendment | Kind::Annex | Kind::Sow | Kind::ChangeOrder
        ))
    .then(|| roman(t))
    .flatten();
    (None, ord, label, ord.map(|_| i))
}

/// The number starting at normalized byte `ns` (token `i`): raw, key, ordinal, last token.
fn value_at(
    p: &Patterns,
    pp: &Prepared<'_>,
    kind: Kind,
    i: usize,
    ns: usize,
    label: bool,
) -> Option<(Number, Option<i64>, usize)> {
    let rest = pp.norm.get(ns..)?;
    let m = label
        .then(|| p.spaced_value.find(rest))
        .flatten()
        .or_else(|| p.value.find(rest))?;
    let raw = m.as_str();
    if raw.chars().count() > MAX_RAW || !raw.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let ne = ns + m.end();
    let key = if kind == Kind::Invoice {
        invoice_identity_key(raw)
    } else {
        normalize_contract_number(raw)
    };
    let ordinal = if key.is_empty() {
        ordinal_of(raw)
    } else {
        None
    };
    let last = pp.toks[i..]
        .iter()
        .take_while(|t| t.ns < ne)
        .count()
        .saturating_sub(1)
        + i;
    Some((
        Number {
            raw: raw.to_owned(),
            key,
            ns,
            ne,
        },
        ordinal,
        last,
    ))
}

/// The ordinal of a non-identifying number: "3", "003", "SOW-001" → 1. Not a year.
fn ordinal_of(raw: &str) -> Option<i64> {
    if is_identifying_contract_key(raw) {
        return None;
    }
    let digits =
        raw.trim_start_matches(|c: char| c.is_ascii_alphabetic() || matches!(c, '-' | '_' | '/'));
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: i64 = digits.parse().ok()?;
    (1..=999).contains(&v).then_some(v)
}

/// A date right after token `last` / normalized byte `after`.
fn date_after(p: &Patterns, pp: &Prepared<'_>, last: usize, after: usize) -> Option<(Date, usize)> {
    let j = last
        + pp.toks
            .get(last + 1..)?
            .iter()
            .take_while(|t| t.ns < after)
            .count()
        + 1;
    let t = pp.toks.get(j)?;
    if !pp.same_line(last, j) && pp.line(j) != pp.line(last) + 1 {
        return None;
    }
    let mut end = (t.ks + 64).min(pp.keys.len());
    while !pp.keys.is_char_boundary(end) {
        end -= 1;
    }
    let slice = pp.keys.get(t.ks..end)?.replace('\n', " ");
    let (date, used) = p.date_at(&slice)?;
    let k_end = t.ks + used;
    let last_tok = pp.tok_ending(k_end)?;
    Some((date, pp.toks[last_tok].ne))
}

/// Serbian registry labels: "Broj:", "Br.:" (with the colon), "Delov. br.", "Број акта".
fn is_registry_label(lab: &str, gap: &str) -> bool {
    let strong = lab
        .split(' ')
        .any(|w| matches!(w, "delovodni" | "delov" | "del" | "akta" | "predmeta"));
    strong || (gap.contains(':') && lab.split(' ').any(|w| matches!(w, "br" | "broj")))
}

/// A number after a generic label at the start of a head line ("Broj: 01-123/2023",
/// "Ref. No.: KW-482917"), without a kind word; `true` for a Serbian registry label.
pub(crate) fn generic_numbers(p: &Patterns, pp: &Prepared<'_>) -> Vec<(Number, bool)> {
    let mut out = Vec::new();
    let n = pp.toks.len();
    let mut i = 0;
    while i < n {
        let (a, b) = pp.line_bounds(i);
        i = b + 1;
        let (own, head) = region(pp, a);
        if !(own && head) {
            if pp.page.index == 0 && pp.top && pp.line(a) >= HEAD_LINES {
                break;
            }
            continue;
        }
        for len in [3usize, 2, 1] {
            let last = a + len - 1;
            if last >= b {
                continue;
            }
            let ks = pp.toks[a].ks;
            let ke = pp.toks[last].ke;
            let Some(lab) = pp.keys.get(ks..ke) else {
                break;
            };
            if !p.generic_label.is_match(lab) {
                continue;
            }
            let v = last + 1;
            let gap = pp.norm.get(pp.toks[last].ne..pp.toks[v].ns).unwrap_or("");
            if gap.len() > 6
                || !gap
                    .chars()
                    .all(|c| c == ' ' || matches!(c, '.' | ':' | '#'))
            {
                break;
            }
            if let Some(rest) = pp.norm.get(pp.toks[v].ns..)
                && let Some(m) = p.value.find(rest)
            {
                let raw = m.as_str();
                let registry = is_registry_label(lab, gap);
                let mut key = normalize_contract_number(raw);
                // A registry label carries a registry number even when it is short
                // ("Број акта: 92/16"): any three digits will do.
                if key.is_empty() && registry && raw.bytes().filter(u8::is_ascii_digit).count() >= 3
                {
                    key = raw
                        .chars()
                        .filter(|c| c.is_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect();
                }
                if !key.is_empty() && raw.chars().count() <= MAX_RAW {
                    out.push((
                        Number {
                            raw: raw.to_owned(),
                            key,
                            ns: pp.toks[v].ns,
                            ne: pp.toks[v].ns + m.end(),
                        },
                        registry,
                    ));
                }
            }
            break;
        }
    }
    out
}

/// "10.12", "99.3", "10.1A": the exhibit index of a securities filing.
fn is_filing_exhibit(raw: &str) -> bool {
    let Some((a, b)) = raw.split_once('.') else {
        return false;
    };
    let b = b.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    (1..=3).contains(&a.len())
        && (1..=3).contains(&b.len())
        && a.bytes().all(|c| c.is_ascii_digit())
        && b.bytes().all(|c| c.is_ascii_digit())
}

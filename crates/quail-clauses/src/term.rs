//! The term of an agreement (`shall remain in effect for a period of two (2) years from the
//! Effective Date`, `The term of this Agreement is 3 years`), whether it renews, notice
//! periods (`thirty (30) days' prior written notice`) and termination hints.

use crate::duration::{Duration, Unit, duration_at};
use crate::tokens::Stream;

/// Tokens before a duration searched for term words.
const BEFORE: usize = 24;
/// Tokens after a duration searched for an anchor (`from the Effective Date`).
const AFTER: usize = 10;
/// Tokens around a term searched for renewal words.
const RENEWAL_WINDOW: usize = 60;

/// What a duration was read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The duration of the agreement.
    Term,
    /// How long confidentiality or other obligations survive.
    Survival,
    /// A notice period.
    Notice,
}

/// One duration with what it was read as and a score in `0..1`.
#[derive(Clone, Copy, Debug)]
pub struct DurationHit {
    /// The duration.
    pub d: Duration,
    /// Term, survival or notice.
    pub kind: Kind,
    /// Strength of the evidence.
    pub score: f32,
    /// First token of the phrase (for evidence).
    pub a: usize,
    /// One past its last token.
    pub b: usize,
}

/// Renewal of the term.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renewal {
    /// Renews by itself (`automatically renewed for successive one-year periods`).
    Auto,
    /// Says it does not renew.
    None,
    /// Not stated near the term.
    Unknown,
}

impl Renewal {
    /// `auto`, `none`, `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Unknown => "unknown",
        }
    }
}

fn any_in(s: &Stream, from: usize, to: usize, set: &[&str]) -> bool {
    (from..to.min(s.len())).any(|k| set.contains(&s.key(k)))
}

fn seq_in(s: &Stream, from: usize, to: usize, seq: &[&str]) -> bool {
    (from..to.min(s.len())).any(|k| s.seq_at(k, seq))
}

/// First token of the sentence holding token `i`, at most `max` tokens back.
fn sentence_start(s: &Stream, i: usize, max: usize) -> usize {
    let from = i.saturating_sub(max);
    (from..i)
        .rev()
        .find(|&k| s.sentence_end(k))
        .map_or(from, |k| k + 1)
}

/// One past the sentence holding token `i`, at most `max` tokens on.
fn sentence_stop(s: &Stream, i: usize, max: usize) -> usize {
    let to = (i + max).min(s.len());
    (i..to).find(|&k| s.sentence_end(k)).unwrap_or(to)
}

/// Every duration with a term, survival or notice reading, in text order.
pub fn find(s: &Stream) -> Vec<DurationHit> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let Some(d) = duration_at(s, i) else {
            i += 1;
            continue;
        };
        if let Some(h) = classify(s, d) {
            out.push(h);
        }
        i = d.b;
    }
    out
}

/// Verbs before a duration that say how long something lasts (`shall terminate two years`,
/// `shall remain in effect for three years`, `shall survive for five years`).
const END_WORDS: &[&str] = &[
    "terminate",
    "terminates",
    "expire",
    "expires",
    "end",
    "ends",
    "remain",
    "remains",
    "continue",
    "continues",
    "survive",
    "survives",
    "effect",
    "force",
    "govern",
    "governs",
];

/// Nouns before a duration that make it the agreement's own.
const AGREEMENT_WORDS: &[&str] = &["agreement", "nda", "letter"];

/// Nouns before a duration that make it how long obligations last.
const OBLIGATION_WORDS: &[&str] = &[
    "obligations",
    "obligation",
    "duty",
    "duties",
    "provisions",
    "restrictions",
    "confidentiality",
    "undertakings",
    "covenants",
];

/// Words before a duration that make it a covenant's period (non-solicit, non-compete,
/// standstill), a payment schedule or an employment tail, not the term.
const COVENANT_WORDS: &[&str] = &[
    "agree",
    "agrees",
    "solicit",
    "solicitation",
    "hire",
    "employ",
    "employment",
    "employed",
    "compete",
    "competition",
    "standstill",
    "acquire",
    "purchase",
    "payroll",
    "installments",
    "salary",
    "severance",
    "month",
    "cobra",
    "coverage",
    "benefits",
    "premiums",
    "options",
    "exercisable",
    "release",
];

/// Words right before a duration that make it a renewal or extension period.
const RENEWAL_WORDS: &[&str] = &[
    "renew",
    "renewed",
    "renewal",
    "successive",
    "additional",
    "extended",
    "extension",
    "further",
];

/// `thirty (30) days' prior written notice`, `upon 60 days notice`, `notice of 30 days`.
fn is_notice(s: &Stream, d: Duration) -> bool {
    let after = (d.b..(d.b + 5).min(s.len()))
        .take_while(|&k| {
            matches!(
                s.key(k),
                "'" | "prior" | "written" | "advance" | "notice" | "s" | "of" | "the"
            )
        })
        .any(|k| s.key(k) == "notice");
    after || d.a >= 2 && s.key(d.a - 1) == "of" && s.key(d.a - 2) == "notice"
}

/// A covenant, payment or employment period, or a tail after the term (`during the term of
/// this Agreement and for a period of one year`, `you agree that for a period of 18 months`).
fn is_covenant(s: &Stream, from: usize, a: usize) -> bool {
    any_in(s, from, a, COVENANT_WORDS)
        || seq_in(s, from, a, &["will", "not"])
        || seq_in(s, from, a, &["shall", "not"])
        || seq_in(s, from, a, &["not", "to"])
        || seq_in(s, from, a, &["during", "the", "term"])
        || seq_in(s, from, a, &["following", "the"])
        || seq_in(s, from, a, &["after", "the"])
}

fn classify(s: &Stream, d: Duration) -> Option<DurationHit> {
    let from = sentence_start(s, d.a, BEFORE);
    let to = sentence_stop(s, d.b, AFTER);
    if is_notice(s, d) {
        return Some(DurationHit {
            d,
            kind: Kind::Notice,
            score: 0.7,
            a: d.a,
            b: d.b,
        });
    }
    // Deadlines, ages and amounts of time to act are not terms: "within thirty days",
    // "no later than", "at least", "per year".
    let prev = if d.a > 0 { s.key(d.a - 1) } else { "" };
    if matches!(
        prev,
        "within" | "least" | "than" | "per" | "every" | "each" | "next" | "last"
    ) || any_in(s, d.a.saturating_sub(3), d.a, RENEWAL_WORDS)
        || is_covenant(s, from, d.a)
    {
        return None;
    }
    let agreement = any_in(s, from, d.a, AGREEMENT_WORDS);
    let obligations = any_in(s, from, d.a, OBLIGATION_WORDS);
    let ends = any_in(s, from, d.a, END_WORDS);
    let term_word = any_in(s, from, d.a, &["term"]);
    // "... from the Effective Date", "... after the date hereof": the term's start, the
    // strongest single sign (Kleister-NDA dev: 35 of 37 right with it, 14 of 29 without).
    let anchor = seq_in(s, d.b, to, &["effective", "date"])
        || seq_in(s, d.b, to, &["date", "hereof"])
        || seq_in(s, d.b, to, &["date", "of", "this"]);
    // Scores are the measured precision of each reading as the term (Kleister-NDA dev,
    // absent-gold documents counted wrong).
    let (kind, mut score): (Kind, f32) = match (ends, agreement, obligations, term_word) {
        // "This Agreement shall terminate two years ...", "remain in effect for ...".
        (true, true, _, _) => (Kind::Term, if anchor { 0.92 } else { 0.55 }),
        // "The obligations of confidentiality shall survive for five years".
        (true, false, true, _) => (Kind::Survival, if anchor { 0.7 } else { 0.35 }),
        // "The term of this Agreement shall be three years".
        (false, true, _, true) => (Kind::Term, if anchor { 0.9 } else { 0.7 }),
        // A bare term word or end verb counts only with its start.
        (true, _, _, _) | (_, _, _, true) if anchor => (Kind::Term, 0.75),
        _ => return None,
    };
    // Days and weeks are rarely the length of an agreement.
    if d.unit == Unit::Day || d.unit == Unit::Week {
        score -= 0.3;
    }
    if score < 0.2 {
        return None;
    }
    Some(DurationHit {
        d,
        kind,
        score: score.min(1.0),
        a: from,
        b: d.b,
    })
}

/// Renewal words around tokens `a..b`.
pub fn renewal(s: &Stream, a: usize, b: usize) -> Renewal {
    let from = a.saturating_sub(RENEWAL_WINDOW / 2);
    let to = b + RENEWAL_WINDOW;
    let none = seq_in(s, from, to, &["not", "be", "renewed"])
        || seq_in(s, from, to, &["no", "renewal"])
        || seq_in(s, from, to, &["not", "renew"])
        || any_in(s, from, to, &["nonrenewable"])
        || seq_in(s, from, to, &["non", "-", "renewable"]);
    if none {
        return Renewal::None;
    }
    let auto = seq_in(s, from, to, &["automatically", "renew"])
        || seq_in(s, from, to, &["automatically", "renewed"])
        || seq_in(s, from, to, &["automatically", "extended"])
        || seq_in(s, from, to, &["auto", "-", "renew"])
        || seq_in(s, from, to, &["shall", "renew"])
        || any_in(s, from, to, &["successive"])
        || seq_in(s, from, to, &["renewal", "term"])
        || seq_in(s, from, to, &["renewal", "period"]);
    if auto {
        Renewal::Auto
    } else {
        Renewal::Unknown
    }
}

/// A termination right named in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminationHit {
    /// `convenience`, `breach`, `insolvency`, `change_of_control`.
    pub kind: &'static str,
    /// First token.
    pub a: usize,
    /// One past the last token.
    pub b: usize,
}

/// Termination rights: `terminate ... for convenience` / `without cause` / `at any time`,
/// `material breach`, `bankruptcy` / `insolvency`, `change of control`, each within a
/// sentence that holds `terminate`. The first one of each kind.
pub fn termination(s: &Stream) -> Vec<TerminationHit> {
    let mut out: Vec<TerminationHit> = Vec::new();
    for i in 0..s.len() {
        if !matches!(s.key(i), "terminate" | "terminated" | "termination") {
            continue;
        }
        let from = sentence_start(s, i, 30);
        let to = sentence_stop(s, i, 40);
        let kinds: [(&str, bool); 4] = [
            (
                "convenience",
                seq_in(s, from, to, &["for", "convenience"])
                    || seq_in(s, from, to, &["without", "cause"])
                    || seq_in(s, i, to, &["at", "any", "time"]),
            ),
            (
                "breach",
                seq_in(s, from, to, &["material", "breach"])
                    || seq_in(s, from, to, &["materially", "breaches"]),
            ),
            (
                "insolvency",
                any_in(
                    s,
                    from,
                    to,
                    &[
                        "bankruptcy",
                        "insolvency",
                        "insolvent",
                        "receiver",
                        "liquidation",
                    ],
                ),
            ),
            (
                "change_of_control",
                seq_in(s, from, to, &["change", "of", "control"]),
            ),
        ];
        for (kind, hit) in kinds {
            if hit && !out.iter().any(|h| h.kind == kind) {
                out.push(TerminationHit {
                    kind,
                    a: from,
                    b: to,
                });
            }
        }
        if out.len() == kinds.len() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Document, Page, PageKind};

    fn stream(text: &str) -> (Document, Stream) {
        let d = Document {
            pages: vec![Page {
                text: text.to_string(),
                ..Page::new(0, PageKind::Physical)
            }],
        };
        let (s, _) = Stream::read(&d, 1 << 20, || false);
        (d, s)
    }

    fn kinds(text: &str) -> Vec<(String, Kind)> {
        let (_, s) = stream(text);
        find(&s).into_iter().map(|h| (h.d.text(), h.kind)).collect()
    }

    #[test]
    fn terms() {
        assert_eq!(
            kinds(
                "This Agreement shall remain in effect for a period of two (2) years from the Effective Date."
            ),
            [("2 years".to_string(), Kind::Term)]
        );
        assert_eq!(
            kinds("The term of this Agreement shall be three years."),
            [("3 years".to_string(), Kind::Term)]
        );
        assert_eq!(
            kinds(
                "The Recipient's obligations of confidentiality shall survive for five (5) years after disclosure."
            ),
            [("5 years".to_string(), Kind::Survival)]
        );
        assert!(kinds("He has 20 years of experience.").is_empty());
        assert!(kinds("Payment is due within thirty (30) days of the invoice.").is_empty());
    }

    #[test]
    fn notice_periods() {
        assert_eq!(
            kinds(
                "Either party may terminate this Agreement upon thirty (30) days' prior written notice."
            ),
            [("30 days".to_string(), Kind::Notice)]
        );
    }

    #[test]
    fn renewal_and_termination() {
        let (_, s) = stream(
            "The term is one year and shall be automatically renewed for successive one-year periods. \
             Either party may terminate for convenience. Termination for material breach.",
        );
        assert_eq!(renewal(&s, 0, 4), Renewal::Auto);
        let t: Vec<&str> = termination(&s).iter().map(|h| h.kind).collect();
        assert_eq!(t, ["convenience", "breach"]);
        let (_, s) = stream("The term is one year. This Agreement shall not be renewed.");
        assert_eq!(renewal(&s, 0, 4), Renewal::None);
        let (_, s) = stream("The term is one year.");
        assert_eq!(renewal(&s, 0, 4), Renewal::Unknown);
    }
}

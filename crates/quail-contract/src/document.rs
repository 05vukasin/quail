//! The document's annexes and, in an amendment, what it changes. Its own number and the
//! documents it cites are `quail-refs`' (`ref.doc_number`, `ref.cites`).

use quail_core::Value;
use quail_payment::lex::{Line, match_at};
use quail_text::token::TokenKind;

use crate::analyzer::{Found, Input, conf, rec, t};
use crate::keys;
use crate::terms::MAX_TEXT;
use crate::text::{clip, find, has, has_phrase, raw, span_of, text_of, upper_start};

/// Annex words in the nominative, as a line or a list item starts with them.
const ANNEX: &[&str] = &[
    "prilog", "annex", "schedule", "exhibit", "appendix", "attachment",
];

/// Headings over a list of annexes (`Prilozi:`, `Annexes:`).
const ANNEX_LIST: &[&[&str]] = &[
    &["prilozi"],
    &["prilozi", "ugovora"],
    &["annexes"],
    &["schedules"],
    &["exhibits"],
    &["attachments"],
    &["appendices"],
];

/// An annex id after the annex word: a number, a capital letter or a roman numeral.
fn annex_id(doc: &quail_core::Document, l: &Line, k: usize) -> Option<usize> {
    let mut k = k;
    if l.toks.get(k).is_some_and(|t| matches!(t.key.as_str(), "br" | "no" | "broj" | "number")) {
        k += 1;
        while l.toks.get(k).is_some_and(|t| t.key == ".") {
            k += 1;
        }
    }
    let tk = l.toks.get(k)?;
    let s = raw(doc, l, tk);
    let ok = match tk.kind {
        TokenKind::Number => s.len() <= 3,
        TokenKind::Word | TokenKind::Alnum => {
            (s.len() == 1 && s.chars().all(|c| c.is_ascii_uppercase()))
                || (s.len() <= 4 && s.chars().all(|c| matches!(c, 'I' | 'V' | 'X')))
        }
        _ => false,
    };
    ok.then_some(k)
}

/// The title after an annex label: after `:`, `-`, `–`, `—` or `.`, to the line's end
/// (at most 14 tokens).
fn annex_title(doc: &quail_core::Document, l: &Line, k: usize) -> Option<(String, usize)> {
    let mut k = k;
    let mut sep = false;
    while l
        .toks
        .get(k)
        .is_some_and(|t| matches!(t.key.as_str(), ":" | "-" | "–" | "—" | "." | ")"))
    {
        sep = true;
        k += 1;
    }
    if !sep && k < l.toks.len() && !upper_start(raw(doc, l, &l.toks[k])) {
        return None;
    }
    let mut last = None;
    for j in k..l.toks.len().min(k + 14) {
        if matches!(l.toks[j].key.as_str(), ";" | "(") {
            break;
        }
        last = Some(j);
    }
    let mut last = last?;
    while last > k && l.toks[last].kind == TokenKind::Punct {
        last -= 1;
    }
    if l.toks[last].kind == TokenKind::Punct {
        return None;
    }
    Some((text_of(doc, l, k, last), last))
}

fn annexes(inp: &Input<'_>, out: &mut Vec<Found>) {
    let doc = inp.doc;
    let lines = inp.lines;
    let mut list_label: Option<(String, u32, usize)> = None;
    for (li, l) in lines.iter().enumerate() {
        let first = &l.toks[0];
        // Items of an annex list: `1. Tehnička specifikacija`.
        if let Some((word, pos, from)) = &list_label {
            if l.pos != *pos as usize || li > from + 10 {
                list_label = None;
            } else if l.toks.len() >= 3
                && first.kind == TokenKind::Number
                && first.key.len() <= 2
                && matches!(l.toks[1].key.as_str(), "." | ")")
                && let Some((title, last)) = annex_title(doc, l, 2)
            {
                let label = format!("{word} {}", raw(doc, l, first));
                out.push(Found {
                    key: keys::ANNEX,
                    value: rec(vec![("label", t(label)), ("title", t(title))]),
                    conf: conf(l, 0.7),
                    page: l.page,
                    span: span_of(l, 0, last),
                });
                continue;
            } else {
                list_label = None;
            }
        }
        if ANNEX_LIST.iter().any(|p| match_at(&l.toks, 0, p).is_some_and(|e| l.toks.get(e).is_some_and(|t| t.key == ":") || e == l.toks.len())) {
            let word = if first.key.starts_with("prilo") { "Prilog" } else {
                match first.key.as_str() {
                    "schedules" => "Schedule",
                    "exhibits" => "Exhibit",
                    "attachments" => "Attachment",
                    "appendices" => "Appendix",
                    _ => "Annex",
                }
            };
            list_label = Some((word.to_owned(), u32::try_from(l.pos).unwrap_or(u32::MAX), li));
            continue;
        }
        // `Prilog 1: Tehnička specifikacija`, `Annex A – Pricing`, `PRILOG 2`.
        if !ANNEX.contains(&first.key.as_str()) || !upper_start(raw(doc, l, first)) {
            continue;
        }
        let Some(id) = annex_id(doc, l, 1) else { continue };
        let word = raw(doc, l, first);
        let mut w: String = word.chars().take(1).collect();
        w.extend(word.chars().skip(1).flat_map(char::to_lowercase));
        let label = format!("{w} {}", raw(doc, l, &l.toks[id]));
        let title = annex_title(doc, l, id + 1);
        let last = title.as_ref().map_or(id, |x| x.1);
        out.push(Found {
            key: keys::ANNEX,
            value: rec(vec![("label", t(label)), ("title", title.map(|x| Value::text(x.0)))]),
            conf: conf(l, 0.75),
            page: l.page,
            span: span_of(l, 0, last),
        });
    }
}

const ACTIONS: &[(&[&str], &str)] = &[
    (&["menja", "se"], "change"),
    (&["se", "menja"], "change"),
    (&["menjaju", "se"], "change"),
    (&["se", "menjaju"], "change"),
    (&["izmenjen*"], "change"),
    (&["glasi"], "change"),
    (&["brise", "se"], "delete"),
    (&["se", "brise"], "delete"),
    (&["brisu", "se"], "delete"),
    (&["dodaje", "se"], "add"),
    (&["se", "dodaje"], "add"),
    (&["dodaju", "se"], "add"),
    (&["is", "amended"], "change"),
    (&["is", "hereby", "amended"], "change"),
    (&["are", "hereby", "amended"], "change"),
    (&["shall", "be", "amended"], "change"),
    (&["is", "replaced"], "change"),
    (&["is", "hereby", "replaced"], "change"),
    (&["shall", "read"], "change"),
    (&["is", "deleted"], "delete"),
    (&["is", "hereby", "deleted"], "delete"),
    (&["shall", "be", "deleted"], "delete"),
    (&["is", "added"], "add"),
    (&["is", "hereby", "added"], "add"),
    (&["is", "inserted"], "add"),
    (&["shall", "be", "added"], "add"),
];

const ARTICLE: &[&str] = &[
    "clan", "clana", "clanu", "cl", "article", "clause", "section", "tacka", "tacke", "stav",
    "stava",
];

fn amendments(inp: &Input<'_>, out: &mut Vec<Found>) {
    let doc = inp.doc;
    let amendment_doc = inp.facts.iter().any(|f| {
        f.key.as_str() == "ref.instrument_kind"
            && matches!(f.value.as_text(), Some("amendment" | "amended_restated" | "change_order"))
    }) || inp.lines.iter().take(6).any(|l| {
        matches!(l.toks[0].key.as_str(), "aneks" | "amendment" | "addendum" | "izmena")
            && upper_start(raw(doc, l, &l.toks[0]))
    });
    for s in inp.sents {
        let toks = &s.toks;
        if !amendment_doc
            && !has(toks, &["aneks*", "amendment"])
            && !has_phrase(toks, &[&["ovim", "izmenama"]])
        {
            continue;
        }
        let Some(h) = find(toks, ACTIONS).into_iter().next() else {
            continue;
        };
        let art = (0..toks.len()).find_map(|i| {
            if !ARTICLE.contains(&toks[i].key.as_str()) {
                return None;
            }
            let mut j = i + 1;
            while toks.get(j).is_some_and(|t| t.key == ".") {
                j += 1;
            }
            let n = toks.get(j)?;
            matches!(n.kind, TokenKind::Number | TokenKind::Alnum).then_some((i, j))
        });
        let (a, b) = art.map_or((h.start, h.end - 1), |(i, j)| (i.min(h.start), j.max(h.end - 1)));
        out.push(Found {
            key: keys::AMENDMENT,
            value: rec(vec![
                ("action", t(*h.what)),
                ("article", art.map(|(i, j)| Value::text(text_of(doc, s, i, j)))),
                ("text", t(clip(doc, s, MAX_TEXT))),
            ]),
            conf: conf(s, if art.is_some() { 0.8 } else { 0.6 }),
            page: s.page,
            span: span_of(s, a, b),
        });
    }
}

/// Annexes and amendments.
pub(crate) fn scan(inp: &Input<'_>, out: &mut Vec<Found>) {
    annexes(inp, out);
    amendments(inp, out);
}

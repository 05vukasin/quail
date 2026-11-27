//! Law and disputes: governing law, the court, arbitration and the contract's languages.
//!
//! English governing-law clauses are those of `quail-clauses` (`clause.governing_law`); the
//! rules here read the Serbian ones (`merodavno je pravo Republike Srbije`, `primenjuju se
//! odredbe Zakona o obligacionim odnosima`).

use quail_core::Value;
use quail_payment::lex::{Line, word_matches};
use quail_text::token::TokenKind;

use crate::analyzer::{Found, Input, conf, rec, t};
use crate::keys;
use crate::terms::MAX_TEXT;
use crate::text::{clip, find, has, raw, span_of, text_of, upper_start};

fn found(key: quail_core::FactKey, value: Value, s: &Line, base: f32, a: usize, b: usize) -> Found {
    Found {
        key,
        value,
        conf: conf(s, base),
        page: s.page,
        span: span_of(s, a, b),
    }
}

/// Jurisdictions as Serbian contracts name them (genitive) and English ones.
const PLACES: &[(&[&str], (&str, &str))] = &[
    (&["republike", "srbije"], ("RS", "Serbia")),
    (&["srbije"], ("RS", "Serbia")),
    (&["srpsko"], ("RS", "Serbia")),
    (
        &["zakona", "o", "obligacionim", "odnosima"],
        ("RS", "Serbia"),
    ),
    (
        &["zakon", "o", "obligacionim", "odnosima"],
        ("RS", "Serbia"),
    ),
    (
        &["zakonom", "o", "obligacionim", "odnosima"],
        ("RS", "Serbia"),
    ),
    (&["republike", "crne", "gore"], ("ME", "Montenegro")),
    (&["crne", "gore"], ("ME", "Montenegro")),
    (
        &["bosne", "i", "hercegovine"],
        ("BA", "Bosnia and Herzegovina"),
    ),
    (&["republike", "hrvatske"], ("HR", "Croatia")),
    (&["hrvatske"], ("HR", "Croatia")),
    (&["republike", "slovenije"], ("SI", "Slovenia")),
    (&["slovenije"], ("SI", "Slovenia")),
    (&["severne", "makedonije"], ("MK", "North Macedonia")),
    (&["nemacke"], ("DE", "Germany")),
    (&["austrije"], ("AT", "Austria")),
    (&["svajcarske"], ("CH", "Switzerland")),
    (&["engleske"], ("GB-ENG", "England")),
];

const LAW_CUES: &[(&[&str], ())] = &[
    (&["merodavn*"], ()),
    (&["primenjuj*"], ()),
    (&["primenjuje"], ()),
    (&["primenice", "se"], ()),
    (&["primjenjuj*"], ()),
    (&["tumaci", "se"], ()),
    (&["tumacice", "se"], ()),
];

fn governing(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    let Some(cue) = find(toks, LAW_CUES).into_iter().next() else {
        return;
    };
    if !has(toks, &["pravo", "prava", "zakon*", "propis*", "odredb*"]) {
        return;
    }
    let Some(p) = find(toks, PLACES).into_iter().next() else {
        return;
    };
    let (code, name) = *p.what;
    let base = if word_matches("merodavn*", &toks[cue.start].key) {
        0.85
    } else {
        0.75
    };
    out.push(found(
        keys::GOVERNING_LAW,
        rec(vec![
            ("code", t(code)),
            ("jurisdiction", t(name)),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        base,
        cue.start.min(p.start),
        cue.end.max(p.end) - 1,
    ));
}

/// The English clause of `quail-clauses`, when no Serbian one was found.
fn governing_from_clauses(inp: &Input<'_>, out: &mut Vec<Found>) {
    if out.iter().any(|f| f.key == keys::GOVERNING_LAW) {
        return;
    }
    let best = inp
        .located("clause.governing_law")
        .filter(|(f, _, _)| f.value.field("basis").and_then(Value::as_text) != Some("forum"))
        .max_by(|a, b| a.0.conf.total_cmp(&b.0.conf));
    if let Some((f, page, span)) = best {
        let v = &f.value;
        out.push(Found {
            key: keys::GOVERNING_LAW,
            value: rec(vec![
                ("code", v.field("code").cloned()),
                ("jurisdiction", v.field("jurisdiction").cloned()),
                ("text", v.field("raw").cloned()),
            ]),
            conf: (f.conf * 100.0).round() / 100.0,
            page,
            span,
        });
    }
}

/// Court adjectives, folded stem → nominative.
const COURT_ADJ: &[(&str, &str)] = &[
    ("privredn", "Privredni"),
    ("osnovn", "Osnovni"),
    ("vis", "Viši"),
    ("apelacion", "Apelacioni"),
    ("vrhovn", "Vrhovni"),
];

/// Cities in the locative (`u Beogradu`) → nominative.
const CITIES: &[(&str, &str)] = &[
    ("beogradu", "Beograd"),
    ("novom", "Novi Sad"),
    ("nisu", "Niš"),
    ("kragujevcu", "Kragujevac"),
    ("subotici", "Subotica"),
    ("pancevu", "Pančevo"),
    ("cacku", "Čačak"),
    ("kraljevu", "Kraljevo"),
    ("leskovcu", "Leskovac"),
    ("valjevu", "Valjevo"),
    ("zrenjaninu", "Zrenjanin"),
    ("sapcu", "Šabac"),
    ("smederevu", "Smederevo"),
    ("uzicu", "Užice"),
    ("somboru", "Sombor"),
    ("podgorici", "Podgorica"),
    ("zagrebu", "Zagreb"),
    ("sarajevu", "Sarajevo"),
    ("ljubljani", "Ljubljana"),
];

/// Capitalised words from token `k` (at most `n`, `and`/`i` between them allowed).
fn caps(inp: &Input<'_>, s: &Line, k: usize, n: usize) -> Option<usize> {
    let mut last = None;
    let mut j = k;
    while j < s.toks.len() && j < k + n {
        let tk = &s.toks[j];
        let w = raw(inp.doc, s, tk);
        if tk.kind == TokenKind::Word && upper_start(w) {
            last = Some(j);
        } else if !(last.is_some() && matches!(tk.key.as_str(), "and" | "i")) {
            break;
        }
        j += 1;
    }
    last
}

fn court(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(
        toks,
        &[
            "nadlez*",
            "jurisdiction",
            "spor*",
            "disput*",
            "venue",
            "competent",
        ],
    ) {
        return;
    }
    let Some(k) = toks.iter().position(|t| {
        matches!(
            t.key.as_str(),
            "sud" | "suda" | "sudu" | "sudom" | "sudovi" | "sudova" | "court" | "courts"
        )
    }) else {
        return;
    };
    let english = toks[k].key.starts_with("court");
    let doc = inp.doc;
    let mut first = k;
    let mut name = String::new();
    if english {
        name = text_of(doc, s, k, k);
    } else {
        if k > 0
            && let Some((_, nom)) = COURT_ADJ
                .iter()
                .find(|(stem, _)| toks[k - 1].key.starts_with(stem))
            && upper_start(raw(doc, s, &toks[k - 1]))
        {
            first = k - 1;
            name.push_str(nom);
            name.push(' ');
        }
        name.push_str("sud");
    }
    // `u Beogradu`, `in Belgrade`, `of England and Wales`.
    let mut last = k;
    let mut city = None;
    if let Some(p) = toks.get(k + 1)
        && matches!(p.key.as_str(), "u" | "in" | "of")
        && let Some(e) = caps(inp, s, k + 2, 4)
    {
        last = e;
        let place = text_of(doc, s, k + 2, e);
        name.push(' ');
        if english {
            name.push_str(&text_of(doc, s, k + 1, e));
        } else {
            // One script for the normalised name: `Privredni sud u Nišu`.
            name.push_str(&quail_text::translit::cyrillic_to_latin(&text_of(
                doc,
                s,
                k + 1,
                e,
            )));
        }
        city = Some(
            CITIES
                .iter()
                .find(|(loc, _)| toks[k + 2].key == *loc)
                .map_or(place, |(_, nom)| (*nom).to_owned()),
        );
    }
    if city.is_none() && first == k {
        // `nadležan sud` alone names no court.
        return;
    }
    if english {
        name = text_of(doc, s, first, last);
    }
    out.push(found(
        keys::COURT,
        rec(vec![
            ("court", t(name)),
            ("city", city.map(Value::text)),
            ("text", t(clip(doc, s, MAX_TEXT))),
        ]),
        s,
        0.8,
        first,
        last,
    ));
}

const RULES: &[(&[&str], &str)] = &[
    (&["icc"], "ICC"),
    (&["international", "chamber", "of", "commerce"], "ICC"),
    (&["lcia"], "LCIA"),
    (&["uncitral"], "UNCITRAL"),
    (&["viac"], "VIAC"),
    (&["siac"], "SIAC"),
    (&["swiss", "rules"], "Swiss Rules"),
    (&["dis"], "DIS"),
];

fn arbitration(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    let Some(k) = toks.iter().position(|t| t.key.starts_with("arbitra")) else {
        return;
    };
    // `neće se rešavati arbitražom`, `not be referred to arbitration`.
    if toks[k.saturating_sub(4)..k]
        .iter()
        .any(|t| matches!(t.key.as_str(), "ne" | "nece" | "not" | "no"))
    {
        return;
    }
    let doc = inp.doc;
    // `Spoljnotrgovinska arbitraža pri Privrednoj komori Srbije`.
    let mut first = k;
    if k > 0 && upper_start(raw(doc, s, &toks[k - 1])) && toks[k - 1].kind == TokenKind::Word {
        first = k - 1;
    }
    let mut last = k;
    if let Some(p) = toks.get(k + 1)
        && matches!(p.key.as_str(), "pri" | "of" | "kod")
    {
        // `pri Privrednoj komori Srbije`: words to the last capitalised one.
        let mut j = k + 2;
        while j < toks.len() && j < k + 8 && toks[j].kind == TokenKind::Word {
            if upper_start(raw(doc, s, &toks[j])) {
                last = j;
            }
            j += 1;
        }
    }
    let institution = (last > k || first < k).then(|| text_of(doc, s, first, last));
    let rules = find(toks, RULES).into_iter().next().map(|h| *h.what);
    let seat = ["seat", "sediste", "mesto"]
        .iter()
        .find_map(|w| toks.iter().position(|t| t.key == *w))
        .and_then(|i| {
            let j = (i + 1..(i + 6).min(toks.len()))
                .find(|&j| upper_start(raw(doc, s, &toks[j])) && toks[j].kind == TokenKind::Word)?;
            Some(text_of(doc, s, j, caps(inp, s, j, 3)?))
        });
    out.push(found(
        keys::ARBITRATION,
        rec(vec![
            ("institution", institution.map(Value::text)),
            ("rules", rules.and_then(t)),
            ("seat", seat.map(Value::text)),
            ("text", t(clip(doc, s, MAX_TEXT))),
        ]),
        s,
        0.8,
        first,
        last,
    ));
}

const LANGUAGES: &[(&str, &str)] = &[
    ("srpsk", "sr"),
    ("engl", "en"),
    ("nemack", "de"),
    ("german", "de"),
    ("francusk", "fr"),
    ("french", "fr"),
    ("rusk", "ru"),
    ("russian", "ru"),
    ("hrvatsk", "hr"),
    ("croatian", "hr"),
    ("italijansk", "it"),
    ("italian", "it"),
    ("serbian", "sr"),
];

fn lang_of(key: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(stem, _)| key.starts_with(stem))
        .map(|&(_, c)| c)
}

fn language(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(toks, &["jezik*", "language*", "verzij*", "version*"]) {
        return;
    }
    let mut langs: Vec<&str> = Vec::new();
    let mut first = None;
    let mut last = 0;
    let mut prevails = None;
    let prevail_at = toks.iter().position(|t| {
        word_matches("merodavn*", &t.key)
            || word_matches("prednost", &t.key)
            || word_matches("prevail*", &t.key)
            || word_matches("control*", &t.key)
    });
    for (i, tk) in toks.iter().enumerate() {
        let Some(c) = lang_of(&tk.key) else { continue };
        // Next to `jezik` / `language` / `verzija` / `version` / `tekst` (or `i` and another).
        let near = toks[i + 1..(i + 4).min(toks.len())].iter().any(|t| {
            [
                "jezik*",
                "language*",
                "verzij*",
                "version*",
                "tekst*",
                "text",
            ]
            .iter()
            .any(|w| word_matches(w, &t.key))
        });
        if !near {
            continue;
        }
        if !langs.contains(&c) {
            langs.push(c);
        }
        first.get_or_insert(i);
        last = i;
        if let Some(p) = prevail_at
            && i.abs_diff(p) <= 8
            && toks[i.saturating_sub(3)..(i + 3).min(toks.len())]
                .iter()
                .any(|t| {
                    word_matches("verzij*", &t.key)
                        || word_matches("version*", &t.key)
                        || word_matches("tekst*", &t.key)
                        || t.key == "text"
                })
        {
            prevails.get_or_insert(c);
        }
    }
    let Some(first) = first else { return };
    if prevails.is_none()
        && langs.len() == 1
        && !has(
            toks,
            &[
                "sacinjen*",
                "zakljucen*",
                "napisan*",
                "drawn",
                "executed",
                "written",
                "made",
                "prepared",
            ],
        )
    {
        return;
    }
    out.push(found(
        keys::LANGUAGE,
        rec(vec![
            (
                "languages",
                Some(Value::List(langs.iter().map(|c| Value::text(*c)).collect())),
            ),
            ("prevails", prevails.and_then(t)),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        if prevails.is_some() { 0.8 } else { 0.7 },
        first,
        last + 1,
    ));
}

/// Every law and dispute fact of the document.
pub(crate) fn scan(inp: &Input<'_>, out: &mut Vec<Found>) {
    for s in inp.sents {
        governing(inp, s, out);
        court(inp, s, out);
        arbitration(inp, s, out);
        language(inp, s, out);
    }
    governing_from_clauses(inp, out);
}

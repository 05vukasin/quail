//! Term and payment terms: duration, renewal, notice period, termination rights, days to
//! pay, payment method, advance, price indexation and late-payment interest.
//!
//! English term, notice and termination clauses found by `quail-clauses` (`clause.term`,
//! `clause.notice_period`, `clause.termination`) are taken as they are; the rules here add
//! Serbian and the cases those miss.

use quail_core::{Span, Value};
use quail_payment::lex::{Line, match_at};

use crate::analyzer::{Found, Input, conf, float, int, rec, t};
use crate::duration::{Dur, Unit, first_duration};
use crate::keys;
use crate::text::{clip, find, first_money, first_percent, has, has_phrase, span_of};

/// Characters of a sentence quoted in a `text` field.
pub(crate) const MAX_TEXT: usize = 300;

/// Words that put a sentence about the contract itself.
const CONTRACT_WORDS: &[&str] = &[
    "ugovor*", "agreement", "contract", "term", "sporazum*", "zakup*", "lease",
];

/// Words of sentences whose durations are not the contract's term.
const NOT_TERM: &[&str] = &[
    "garanc*", "garantn*", "warrant*", "poverljiv*", "confidential*", "otkaz*", "notice",
    "surviv*", "zastarel*", "plac*", "uplat*", "pay*", "faktur*", "invoice*", "isporuk*",
    "deliver*", "kazn*", "penal*", "reklamac*", "odziv*", "response",
];

const TERM_CUES: &[(&[&str], ())] = &[
    (&["zakljucuje", "se", "na", "period", "od"], ()),
    (&["zakljucuje", "se", "na", "rok", "od"], ()),
    (&["zakljucuje", "se", "na", "vreme", "od"], ()),
    (&["zakljucuje", "se", "na", "odredjeno", "vreme", "od"], ()),
    (&["zakljucuje", "se", "na"], ()),
    (&["zakljucen", "je", "na"], ()),
    (&["na", "period", "od"], ()),
    (&["u", "trajanju", "od"], ()),
    (&["na", "rok", "od"], ()),
    (&["na", "odredjeno", "vreme", "od"], ()),
    (&["rok", "trajanja"], ()),
    (&["period", "vazenja"], ()),
    (&["rok", "vazenja"], ()),
    (&["vazi"], ()),
    (&["vazice"], ()),
    (&["traje"], ()),
    (&["for", "a", "term", "of"], ()),
    (&["for", "a", "period", "of"], ()),
    (&["initial", "term", "of"], ()),
    (&["term", "of"], ()),
    (&["remain", "in", "force", "for"], ()),
    (&["remain", "in", "full", "force", "and", "effect", "for"], ()),
    (&["remain", "in", "effect", "for"], ()),
];

const INDEFINITE: &[&[&str]] = &[
    &["na", "neodredjeno", "vreme"],
    &["neodredjeno", "vreme"],
    &["indefinite", "period"],
    &["indefinite", "term"],
    &["indefinitely"],
    &["for", "an", "indefinite"],
];

/// Fields `iso`, `months`, `days` of a duration.
fn dur_fields(d: &Dur) -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("iso", t(d.iso())),
        ("months", d.months().and_then(int)),
        ("days", d.days().and_then(int)),
    ]
}

/// `iso`, `months`, `days` of an ISO duration written by `quail-clauses` (`P2Y`, `P30D`).
fn iso_fields(iso: &str) -> Vec<(&'static str, Option<Value>)> {
    let body = iso.strip_prefix('P').unwrap_or("");
    let (num, unit) = body.split_at(body.len().saturating_sub(1));
    let n: Option<u32> = num.parse().ok();
    let (months, days) = match (unit, n) {
        ("Y", Some(n)) => (n.checked_mul(12), None),
        ("M", Some(n)) => (Some(n), None),
        ("W", Some(n)) => (None, n.checked_mul(7)),
        ("D", Some(n)) => (None, Some(n)),
        _ => (None, None),
    };
    vec![
        ("iso", t(iso)),
        ("months", months.and_then(int)),
        ("days", days.and_then(int)),
    ]
}

fn found(key: quail_core::FactKey, value: Value, s: &Line, base: f32, span: Span) -> Found {
    Found {
        key,
        value,
        conf: conf(s, base),
        page: s.page,
        span,
    }
}

/// The contract's duration.
fn term(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(toks, CONTRACT_WORDS) {
        return;
    }
    let text = clip(inp.doc, s, MAX_TEXT);
    for i in 0..toks.len() {
        if let Some(e) = INDEFINITE.iter().find_map(|p| match_at(toks, i, p)) {
            out.push(found(
                keys::TERM,
                rec(vec![
                    ("indefinite", Some(Value::Bool(true))),
                    ("text", t(text.as_str())),
                ]),
                s,
                0.75,
                span_of(s, i, e - 1),
            ));
            return;
        }
    }
    if has(toks, NOT_TERM) {
        return;
    }
    for h in find(toks, TERM_CUES) {
        let Some(d) = first_duration(toks, h.end, h.end + 4, &[]) else {
            continue;
        };
        let base = match d.unit {
            Unit::Month | Unit::Year => 0.8,
            Unit::Day | Unit::Week => 0.6,
            _ => continue,
        };
        let mut f = dur_fields(&d);
        f.push(("text", t(text.as_str())));
        out.push(found(keys::TERM, rec(f), s, base, span_of(s, h.start, d.last)));
        return;
    }
}

const RENEW_YES: &[(&[&str], ())] = &[
    (&["automatski", "se", "produz*"], ()),
    (&["automatski", "produz*"], ()),
    (&["produzava", "se", "automatski"], ()),
    (&["produzice", "se", "automatski"], ()),
    (&["precutno", "se", "produz*"], ()),
    (&["precutno", "produz*"], ()),
    (&["smatra", "se", "produzen*"], ()),
    (&["automatically", "renew*"], ()),
    (&["automatically", "be", "renewed"], ()),
    (&["automatically", "extend*"], ()),
    (&["automatically", "be", "extended"], ()),
    (&["renew*", "automatically"], ()),
    (&["auto", "renew*"], ()),
    (&["successive", "renewal"], ()),
    (&["tacitly", "renewed"], ()),
];

const RENEW_NO: &[&[&str]] = &[
    &["ne", "produzava", "se", "automatski"],
    &["nece", "se", "automatski"],
    &["ne", "moze", "se", "precutno"],
    &["ne", "produzava", "se", "precutno"],
    &["not", "be", "automatically"],
    &["not", "automatically"],
    &["shall", "not", "renew"],
    &["will", "not", "renew"],
    &["shall", "not", "be", "renewed"],
    &["no", "automatic", "renewal"],
    &["bez", "automatskog", "produzenja"],
];

fn renewal(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    let text = clip(inp.doc, s, MAX_TEXT);
    for i in 0..toks.len() {
        if let Some(e) = RENEW_NO.iter().find_map(|p| match_at(toks, i, p)) {
            out.push(found(
                keys::RENEWAL,
                rec(vec![("auto", Some(Value::Bool(false))), ("text", t(text.as_str()))]),
                s,
                0.75,
                span_of(s, i, e - 1),
            ));
            return;
        }
    }
    let Some(h) = find(toks, RENEW_YES).into_iter().next() else {
        return;
    };
    let d = first_duration(toks, 0, toks.len(), &[Unit::Month, Unit::Year]);
    let same = has_phrase(
        toks,
        &[
            &["isti", "period"],
            &["isti", "rok"],
            &["isto", "vreme"],
            &["same", "period"],
            &["same", "term"],
            &["equal", "period*"],
        ],
    );
    let end = d.map_or(h.end - 1, |d| d.last.max(h.end - 1));
    out.push(found(
        keys::RENEWAL,
        rec(vec![
            ("auto", Some(Value::Bool(true))),
            ("iso", d.map(|d| Value::text(d.iso()))),
            ("same_period", same.then_some(Value::Bool(true))),
            ("text", t(text)),
        ]),
        s,
        0.8,
        span_of(s, h.start, end),
    ));
}

const NOTICE_CUES: &[(&[&str], ())] = &[
    (&["otkazni", "rok"], ()),
    (&["otkaznim", "rokom"], ()),
    (&["otkaznog", "roka"], ()),
    (&["otkaznom", "roku"], ()),
    (&["prethodno", "pismeno", "obavestenje"], ()),
    (&["pismeno", "obavestenje"], ()),
    (&["pismenim", "obavestenjem"], ()),
    (&["pisanim", "obavestenjem"], ()),
    (&["obavestenje", "o", "otkazu"], ()),
    (&["notice", "period"], ()),
    (&["notice", "of"], ()),
    (&["written", "notice", "of"], ()),
    (&["prior", "written", "notice", "of"], ()),
];

const DAYS_ETC: &[Unit] = &[Unit::Day, Unit::Week, Unit::Month];

fn notice(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    let text = clip(inp.doc, s, MAX_TEXT);
    if has(toks, &["plac*", "uplat*", "faktur*", "invoice*", "pay*"]) {
        return;
    }
    for h in find(toks, NOTICE_CUES) {
        let Some(d) = first_duration(toks, h.end, h.end + 6, DAYS_ETC) else {
            continue;
        };
        let mut f = dur_fields(&d);
        f.push(("text", t(text.as_str())));
        out.push(found(keys::NOTICE, rec(f), s, 0.8, span_of(s, h.start, d.last)));
        return;
    }
    // English `thirty (30) days' prior written notice`.
    for i in 0..toks.len() {
        let Some(d) = crate::duration::duration_at(toks, i) else {
            continue;
        };
        if !DAYS_ETC.contains(&d.unit) {
            continue;
        }
        let near = (d.last + 1..(d.last + 5).min(toks.len())).find(|&k| toks[k].key == "notice");
        if let Some(k) = near
            && has(toks, &["terminat*", "otkaz*", "raskid*"])
        {
            let mut f = dur_fields(&d);
            f.push(("text", t(text.as_str())));
            out.push(found(keys::NOTICE, rec(f), s, 0.75, span_of(s, d.first, k)));
            return;
        }
    }
}

const END_WORDS: &[&str] = &[
    "raskid*", "raskin*", "otkaz*", "terminat*", "prestanak", "prestaje", "prestati",
];

const TERMINATION: &[(&[&str], &str)] = &[
    (&["krsenj*"], "breach"),
    (&["povred*"], "breach"),
    (&["ne", "izvrsava"], "breach"),
    (&["ne", "ispunjava"], "breach"),
    (&["neispunjenj*"], "breach"),
    (&["neizvrsenj*"], "breach"),
    (&["ne", "izvrsi"], "breach"),
    (&["breach*"], "breach"),
    (&["fails", "to", "perform"], "breach"),
    (&["bez", "navodjenja", "razloga"], "convenience"),
    (&["bez", "obrazlozenja"], "convenience"),
    (&["bez", "razloga"], "convenience"),
    (&["for", "convenience"], "convenience"),
    (&["without", "cause"], "convenience"),
    (&["for", "any", "reason"], "convenience"),
    (&["stecaj*"], "insolvency"),
    (&["likvidacij*"], "insolvency"),
    (&["insolven*"], "insolvency"),
    (&["bankrupt*"], "insolvency"),
    (&["sporazumn*"], "mutual"),
    (&["sporazumom"], "mutual"),
    (&["mutual", "agreement"], "mutual"),
    (&["mutual", "consent"], "mutual"),
    (&["mutually", "agree*"], "mutual"),
    (&["visa", "sila"], "force_majeure"),
    (&["vise", "sile"], "force_majeure"),
    (&["visom", "silom"], "force_majeure"),
    (&["visoj", "sili"], "force_majeure"),
    (&["force", "majeure"], "force_majeure"),
    (&["change", "of", "control"], "change_of_control"),
    (&["promen*", "vlasnick*", "strukture"], "change_of_control"),
    (&["otkaznim", "rokom"], "notice"),
    (&["otkazni", "rok"], "notice"),
    (&["notice"], "notice"),
];

fn termination(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if toks.len() < 6 || !has(toks, END_WORDS) {
        return;
    }
    let text = clip(inp.doc, s, MAX_TEXT);
    let mut seen: Vec<&str> = Vec::new();
    for h in find(toks, TERMINATION) {
        let kind = *h.what;
        if seen.contains(&kind) {
            continue;
        }
        // Notice only when nothing more specific is said.
        if kind == "notice" && !seen.is_empty() {
            continue;
        }
        seen.push(kind);
        out.push(found(
            keys::TERMINATION,
            rec(vec![("kind", t(kind)), ("text", t(text.as_str()))]),
            s,
            if kind == "notice" { 0.6 } else { 0.75 },
            s.span,
        ));
    }
}

const PAY_WORDS: &[&str] = &[
    "plac*", "plat*", "uplat*", "isplat*", "pay*", "paid", "faktur*", "racun*", "invoice*",
    "valut*",
];

const PAY_CUES: &[(&[&str], ())] = &[
    (&["u", "roku", "od"], ()),
    (&["roku", "od"], ()),
    (&["rok", "placanja"], ()),
    (&["rok", "placanja", "je"], ()),
    (&["rok", "placanja", "iznosi"], ()),
    (&["valuta", "placanja"], ()),
    (&["valuta"], ()),
    (&["najkasnije", "u", "roku", "od"], ()),
    (&["within"], ()),
    (&["payment", "terms"], ()),
    (&["no", "later", "than"], ()),
    (&["in"], ()),
];

const PAY_FROM: &[(&[&str], &str)] = &[
    (&["prijema", "fakture"], "invoice_receipt"),
    (&["prijema", "racuna"], "invoice_receipt"),
    (&["prijema", "ispravne", "fakture"], "invoice_receipt"),
    (&["prijema", "ispravnog", "racuna"], "invoice_receipt"),
    (&["receipt", "of", "invoice"], "invoice_receipt"),
    (&["receipt", "of", "the", "invoice"], "invoice_receipt"),
    (&["receipt", "of", "a", "valid", "invoice"], "invoice_receipt"),
    (&["receipt", "of", "an", "invoice"], "invoice_receipt"),
    (&["receipt", "of", "each", "invoice"], "invoice_receipt"),
    (&["izdavanja", "fakture"], "invoice_date"),
    (&["ispostavljanja", "fakture"], "invoice_date"),
    (&["izdavanja", "racuna"], "invoice_date"),
    (&["prometa"], "invoice_date"),
    (&["invoice", "date"], "invoice_date"),
    (&["date", "of", "invoice"], "invoice_date"),
    (&["date", "of", "the", "invoice"], "invoice_date"),
    (&["isporuke"], "delivery"),
    (&["delivery"], "delivery"),
    (&["potpisivanja", "ugovora"], "signing"),
    (&["zakljucenja", "ugovora"], "signing"),
    (&["signing"], "signing"),
];

fn payment_days(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(toks, PAY_WORDS)
        || has(toks, &["otkaz*", "notice", "reklamac*", "prigovor*", "complaint*", "dispute*"])
    {
        return;
    }
    let text = clip(inp.doc, s, MAX_TEXT);
    // `net 30`, `Net 30 days`.
    for i in 0..toks.len().saturating_sub(1) {
        if toks[i].key == "net"
            && toks[i + 1].kind == quail_text::token::TokenKind::Number
            && let Ok(n) = toks[i + 1].key.parse::<u32>()
            && (1..=180).contains(&n)
        {
            out.push(found(
                keys::PAYMENT_DAYS,
                rec(vec![("days", int(n)), ("text", t(text.as_str()))]),
                s,
                0.8,
                span_of(s, i, i + 1),
            ));
            return;
        }
    }
    for h in find(toks, PAY_CUES) {
        let Some(d) = first_duration(toks, h.end, h.end + 3, &[Unit::Day]) else {
            continue;
        };
        if d.n > 365 {
            continue;
        }
        // Only a deadline about paying: a pay word in the six tokens around it.
        let lo = h.start.saturating_sub(8);
        let hi = (d.last + 8).min(toks.len());
        if !has(&toks[lo..hi], PAY_WORDS) {
            continue;
        }
        let from = find(&toks[d.last + 1..hi], PAY_FROM).into_iter().next();
        let mut end = d.last;
        if let Some(f) = &from {
            end = d.last + f.end;
        }
        let base = if from.is_some() { 0.85 } else { 0.7 };
        out.push(found(
            keys::PAYMENT_DAYS,
            rec(vec![
                ("days", int(d.n)),
                ("business", d.business.then_some(Value::Bool(true))),
                ("from", from.map(|f| Value::text(*f.what))),
                ("text", t(text.as_str())),
            ]),
            s,
            base,
            span_of(s, h.start, end),
        ));
        return;
    }
}

const METHOD: &[(&[&str], &str)] = &[
    (&["virmanski"], "bank_transfer"),
    (&["virmanom"], "bank_transfer"),
    (&["virmansk*"], "bank_transfer"),
    (&["uplatom", "na", "tekuci", "racun"], "bank_transfer"),
    (&["uplatom", "na", "racun"], "bank_transfer"),
    (&["na", "tekuci", "racun"], "bank_transfer"),
    (&["bank", "transfer"], "bank_transfer"),
    (&["wire", "transfer"], "bank_transfer"),
    (&["u", "gotovini"], "cash"),
    (&["gotovinski"], "cash"),
    (&["in", "cash"], "cash"),
    (&["platnom", "karticom"], "card"),
    (&["karticom"], "card"),
    (&["credit", "card"], "card"),
    (&["kompenzacij*"], "compensation"),
];

fn method(_inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(toks, PAY_WORDS) {
        return;
    }
    if let Some(h) = find(toks, METHOD).into_iter().next() {
        out.push(found(
            keys::PAYMENT_METHOD,
            Value::text(*h.what),
            s,
            0.75,
            span_of(s, h.start, h.end - 1),
        ));
    }
}

const ADVANCE: &[(&[&str], ())] = &[
    (&["avans*"], ()),
    (&["avansn*"], ()),
    (&["advance", "payment*"], ()),
    (&["prepayment"], ()),
    (&["upfront", "payment"], ()),
    (&["down", "payment"], ()),
    (&["in", "advance"], ()),
];

fn advance(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if toks.len() < 5 || has(toks, &["garancij*", "guarantee*", "menic*"]) {
        return;
    }
    let Some(h) = find(toks, ADVANCE).into_iter().next() else {
        return;
    };
    let pct = first_percent(inp.doc, s, 0, toks.len());
    let amount = first_money(inp.doc, s, h.end, toks.len());
    if pct.is_none() && amount.is_none() && !has(toks, PAY_WORDS) {
        return;
    }
    let mut end = h.end - 1;
    let mut start = h.start;
    if let Some((_, a, b)) = pct {
        start = start.min(a);
        end = end.max(b);
    }
    if let Some(a) = &amount {
        end = end.max(a.last);
    }
    let base = if pct.is_some() || amount.is_some() { 0.75 } else { 0.55 };
    out.push(found(
        keys::ADVANCE,
        rec(vec![
            ("percent", pct.and_then(|p| float(p.0))),
            ("amount", amount.map(|a| Value::Money(a.value.money(a.currency)))),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        base,
        span_of(s, start, end),
    ));
}

const INDEX: &[(&[&str], &str)] = &[
    (&["indeks", "potrosackih", "cena"], "cpi"),
    (&["indeksom", "potrosackih", "cena"], "cpi"),
    (&["indeksu", "potrosackih", "cena"], "cpi"),
    (&["consumer", "price", "index"], "cpi"),
    (&["cpi"], "cpi"),
    (&["inflacij*"], "inflation"),
    (&["inflation"], "inflation"),
    (&["srednj*", "kurs*"], "exchange_rate"),
    (&["kurs*", "nbs"], "exchange_rate"),
    (&["exchange", "rate"], "exchange_rate"),
    (&["revalorizac*"], "other"),
    (&["indeksacij*"], "other"),
    (&["indexation"], "other"),
    (&["indexed"], "other"),
    (&["price", "adjustment"], "other"),
    (&["uskladjivanj*", "cen*"], "other"),
    (&["uskladjuje", "se"], "other"),
    (&["fiksn*"], "fixed"),
    (&["nepromenljiv*"], "fixed"),
    (&["fixed", "price*"], "fixed"),
    (&["prices", "shall", "remain", "fixed"], "fixed"),
    (&["remain", "fixed"], "fixed"),
];

fn indexation(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(toks, &["cen*", "price*", "fee*", "naknad*", "zakupnin*", "rent"]) {
        return;
    }
    let hits = find(toks, INDEX);
    // An index beats `fixed`; the first of a kind wins.
    let Some(h) = hits
        .iter()
        .find(|h| *h.what != "fixed")
        .or_else(|| hits.first())
    else {
        return;
    };
    let base = if *h.what == "other" { 0.6 } else { 0.75 };
    out.push(found(
        keys::INDEXATION,
        rec(vec![
            ("basis", t(*h.what)),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        base,
        span_of(s, h.start, h.end - 1),
    ));
}

const INTEREST: &[(&[&str], ())] = &[
    (&["zatezn*", "kamat*"], ()),
    (&["zakonsk*", "zatezn*", "kamat*"], ()),
    (&["late", "payment", "interest"], ()),
    (&["default", "interest"], ()),
    (&["interest", "on", "late", "payment*"], ()),
    (&["interest", "on", "overdue"], ()),
    (&["interest", "on", "any", "overdue"], ()),
];

fn late_interest(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    let Some(h) = find(toks, INTEREST).into_iter().next() else {
        return;
    };
    let statutory = has(toks, &["zakonsk*", "statutory"]);
    let pct = first_percent(inp.doc, s, h.end, toks.len());
    let end = pct.map_or(h.end - 1, |p| p.2);
    out.push(found(
        keys::LATE_INTEREST,
        rec(vec![
            ("statutory", Some(Value::Bool(statutory))),
            ("percent", pct.and_then(|p| float(p.0))),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        0.75,
        span_of(s, h.start, end),
    ));
}

/// English clauses of `quail-clauses`, taken as they are when the rules found nothing.
fn from_clauses(inp: &Input<'_>, out: &mut Vec<Found>) {
    let has_key = |out: &Vec<Found>, k: &quail_core::FactKey| out.iter().any(|f| &f.key == k);
    let ocr_line = |page: u32| inp.lines.iter().find(|l| l.page == page);
    let add = |out: &mut Vec<Found>, key: quail_core::FactKey, src: &str, value: &dyn Fn(&Value) -> Option<Value>| {
        if has_key(out, &key) {
            return;
        }
        let best = inp
            .located(src)
            .filter_map(|(f, page, span)| Some((value(&f.value)?, f.conf, page, span)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((v, c, page, span)) = best {
            let ocr = ocr_line(page).is_some_and(|l| l.ocr);
            let c = if ocr { c * 0.95 } else { c };
            out.push(Found {
                key,
                value: v,
                conf: (c * 100.0).round() / 100.0,
                page,
                span,
            });
        }
    };
    add(out, keys::TERM, "clause.term", &|v| {
        let iso = v.field("duration")?.as_text()?;
        let mut f = iso_fields(iso);
        f.push(("text", v.field("raw").cloned()));
        Some(rec(f))
    });
    add(out, keys::NOTICE, "clause.notice_period", &|v| {
        let iso = v.field("duration")?.as_text()?;
        let mut f = iso_fields(iso);
        f.push(("text", v.field("raw").cloned()));
        Some(rec(f))
    });
    add(out, keys::RENEWAL, "clause.term", &|v| {
        let auto = match v.field("renewal")?.as_text()? {
            "auto" => true,
            "none" => false,
            _ => return None,
        };
        Some(rec(vec![
            ("auto", Some(Value::Bool(auto))),
            ("text", v.field("raw").cloned()),
        ]))
    });
    let kinds: Vec<Value> = out
        .iter()
        .filter(|f| f.key == keys::TERMINATION)
        .filter_map(|f| f.value.field("kind").cloned())
        .collect();
    for (f, page, span) in inp.located("clause.termination") {
        let Some(kind) = f.value.field("kind") else { continue };
        if kinds.contains(kind) || out.iter().any(|o| o.key == keys::TERMINATION && o.value.field("kind") == Some(kind)) {
            continue;
        }
        out.push(Found {
            key: keys::TERMINATION,
            value: rec(vec![("kind", Some(kind.clone())), ("text", f.value.field("raw").cloned())]),
            conf: (f.conf * 100.0).round() / 100.0,
            page,
            span,
        });
    }
}

/// Every term and payment-term fact of the document.
pub(crate) fn scan(inp: &Input<'_>, out: &mut Vec<Found>) {
    for s in inp.sents {
        term(inp, s, out);
        renewal(inp, s, out);
        notice(inp, s, out);
        termination(inp, s, out);
        payment_days(inp, s, out);
        method(inp, s, out);
        advance(inp, s, out);
        indexation(inp, s, out);
        late_interest(inp, s, out);
    }
    from_clauses(inp, out);
}

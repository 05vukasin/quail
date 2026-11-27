//! Obligations and risk: penalties, guarantees, liability caps, confidentiality, service
//! levels, delivery deadlines and the warranty period.

use quail_core::{Date, Value};
use quail_payment::lex::Line;

use crate::analyzer::{Found, Input, conf, float, int, rec, t};
use crate::duration::{Unit, first_duration};
use crate::keys;
use crate::terms::MAX_TEXT;
use crate::text::{clip, find, first_money, first_percent, has, has_phrase, span_of};

fn found(key: quail_core::FactKey, value: Value, s: &Line, base: f32, a: usize, b: usize) -> Found {
    Found {
        key,
        value,
        conf: conf(s, base),
        page: s.page,
        span: span_of(s, a, b),
    }
}

fn money(a: &quail_payment::amount::Amount) -> Value {
    Value::Money(a.value.money(a.currency))
}

const PENALTY: &[(&[&str], ())] = &[
    (&["ugovorn*", "kazn*"], ()),
    (&["penal*"], ()),
    (&["liquidated", "damages"], ()),
    (&["contractual", "penalty"], ()),
    (&["penalty"], ()),
    (&["penalties"], ()),
];

const PER_DAY: &[&[&str]] = &[
    &["za", "svaki", "dan"],
    &["po", "danu"],
    &["dnevno"],
    &["per", "day"],
    &["for", "each", "day"],
    &["for", "every", "day"],
    &["each", "day"],
    &["per", "calendar", "day"],
    &["za", "svaki", "kalendarski", "dan"],
];

const PER_WEEK: &[&[&str]] = &[
    &["za", "svaku", "nedelju"],
    &["po", "nedelji"],
    &["nedeljno"],
    &["per", "week"],
    &["for", "each", "week"],
    &["for", "every", "week"],
    &["each", "week"],
];

const CAP: &[(&[&str], ())] = &[
    (&["najvise"], ()),
    (&["maksimalno"], ()),
    (&["max*"], ()),
    (&["ne", "moze", "preci"], ()),
    (&["ne", "moze", "biti", "veca", "od"], ()),
    (&["ne", "moze", "biti", "veci", "od"], ()),
    (&["up", "to", "a", "maximum", "of"], ()),
    (&["not", "exceed"], ()),
    (&["capped", "at"], ()),
    (&["limited", "to"], ()),
    (&["do", "iznosa", "od"], ()),
    (&["do", "visine", "od"], ()),
];

fn penalty(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if toks.len() < 5 {
        return;
    }
    let Some(h) = find(toks, PENALTY).into_iter().next() else {
        return;
    };
    let doc = inp.doc;
    let cap = find(toks, CAP).into_iter().find(|c| c.start > h.start);
    let cap_pct = cap.as_ref().and_then(|c| first_percent(doc, s, c.end, c.end + 4));
    let rate_end = cap.as_ref().map_or(toks.len(), |c| c.start);
    let pct = first_percent(doc, s, 0, rate_end);
    let amount = first_money(doc, s, h.end, rate_end);
    let per = if has_phrase(toks, PER_DAY) {
        Some("day")
    } else if has_phrase(toks, PER_WEEK) {
        Some("week")
    } else {
        None
    };
    let obliges = crate::text::has(toks, &["plat*", "duzan", "duzna", "obavez*", "pay*", "shall", "zarac*"]);
    if pct.is_none() && amount.is_none() && !obliges {
        return;
    }
    let mut end = h.end - 1;
    let mut start = h.start;
    for (a, b) in [pct.map(|p| (p.1, p.2)), cap_pct.map(|p| (p.1, p.2)), amount.map(|a| (a.first, a.last))]
        .into_iter()
        .flatten()
    {
        start = start.min(a);
        end = end.max(b);
    }
    let base = if pct.is_some() || amount.is_some() { 0.8 } else { 0.55 };
    out.push(found(
        keys::PENALTY,
        rec(vec![
            ("percent", pct.and_then(|p| float(p.0))),
            ("per", per.and_then(t)),
            ("amount", amount.as_ref().map(money)),
            ("cap_percent", cap_pct.and_then(|p| float(p.0))),
            ("text", t(clip(doc, s, MAX_TEXT))),
        ]),
        s,
        base,
        start,
        end,
    ));
}

const GUARANTEE: &[(&[&str], &str)] = &[
    (&["bankarsk*", "garancij*"], "bank_guarantee"),
    (&["bank", "guarantee*"], "bank_guarantee"),
    (&["garancij*", "banke"], "bank_guarantee"),
    (&["menic*"], "bill_of_exchange"),
    (&["menicn*"], "bill_of_exchange"),
    (&["bill", "of", "exchange"], "bill_of_exchange"),
    (&["promissory", "note"], "bill_of_exchange"),
    (&["performance", "bond"], "performance_bond"),
    (&["letter", "of", "credit"], "letter_of_credit"),
    (&["akreditiv*"], "letter_of_credit"),
    (&["parent", "company", "guarantee"], "parent_guarantee"),
    (&["security", "deposit"], "deposit"),
    (&["depozit*"], "deposit"),
];

const PURPOSE: &[(&[&str], &str)] = &[
    (&["dobro", "izvrsenje"], "performance"),
    (&["dobro", "izvrsenje", "posla"], "performance"),
    (&["izvrsenje", "ugovornih", "obaveza"], "performance"),
    (&["performance"], "performance"),
    (&["povracaj", "avans*"], "advance"),
    (&["avansn*"], "advance"),
    (&["advance", "payment"], "advance"),
    (&["garantnom", "roku"], "warranty"),
    (&["otklanjanje", "gresaka"], "warranty"),
    (&["warranty"], "warranty"),
    (&["ozbiljnost", "ponude"], "bid"),
    (&["bid"], "bid"),
];

fn guarantee(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    // `nije potrebna`, `is not required`.
    if toks.len() < 4 || has(toks, &["nije", "nisu", "nece", "not"]) {
        return;
    }
    let doc = inp.doc;
    let mut seen: Vec<&str> = Vec::new();
    for h in find(toks, GUARANTEE) {
        let kind = *h.what;
        if seen.contains(&kind) {
            continue;
        }
        seen.push(kind);
        let purpose = find(toks, PURPOSE).into_iter().next().map(|p| *p.what);
        let pct = first_percent(doc, s, h.end, toks.len());
        let amount = first_money(doc, s, h.end, toks.len());
        let mut end = h.end - 1;
        for b in [pct.map(|p| p.2), amount.map(|a| a.last)].into_iter().flatten() {
            end = end.max(b);
        }
        out.push(found(
            keys::GUARANTEE,
            rec(vec![
                ("kind", t(kind)),
                ("purpose", purpose.and_then(t)),
                ("percent", pct.and_then(|p| float(p.0))),
                ("amount", amount.as_ref().map(money)),
                ("text", t(clip(doc, s, MAX_TEXT))),
            ]),
            s,
            0.75,
            h.start,
            end,
        ));
    }
}

const LIMIT: &[(&[&str], ())] = &[
    (&["ogranicen*"], ()),
    (&["ogranicava"], ()),
    (&["ne", "moze", "preci"], ()),
    (&["ne", "moze", "biti", "veca"], ()),
    (&["ne", "moze", "biti", "veci"], ()),
    (&["najvise", "do"], ()),
    (&["do", "visine"], ()),
    (&["do", "iznosa"], ()),
    (&["shall", "not", "exceed"], ()),
    (&["not", "exceed"], ()),
    (&["limited", "to"], ()),
    (&["capped", "at"], ()),
];

const BASIS: &[(&[&str], &str)] = &[
    (&["ugovoren*", "cen*"], "contract_value"),
    (&["ugovoren*", "vrednost*"], "contract_value"),
    (&["vrednost*", "ugovora"], "contract_value"),
    (&["ukupn*", "vrednost*"], "contract_value"),
    (&["contract", "value"], "contract_value"),
    (&["contract", "price"], "contract_value"),
    (&["total", "contract"], "contract_value"),
    (&["fees", "paid"], "fees_paid"),
    (&["amounts", "paid"], "fees_paid"),
    (&["fees", "payable"], "fees_paid"),
    (&["total", "fees"], "fees_paid"),
    (&["placen*", "naknad*"], "fees_paid"),
    (&["isplacen*", "iznos*"], "fees_paid"),
];

fn liability_cap(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if !has(toks, &["odgovornost*", "odgovara*", "liability", "liable"])
        || has(toks, &["neogranicen*", "unlimited", "kazn*", "penal*"])
    {
        return;
    }
    let Some(h) = find(toks, LIMIT).into_iter().next() else {
        return;
    };
    let doc = inp.doc;
    let amount = first_money(doc, s, h.end, toks.len());
    let pct = first_percent(doc, s, h.end, toks.len());
    let basis = find(&toks[h.end..], BASIS).into_iter().next();
    if amount.is_none() && pct.is_none() && basis.is_none() {
        return;
    }
    let months = first_duration(toks, h.end, toks.len(), &[Unit::Month, Unit::Year]);
    let mut end = h.end - 1;
    for b in [
        amount.map(|a| a.last),
        pct.map(|p| p.2),
        basis.as_ref().map(|b| h.end + b.end - 1),
    ]
    .into_iter()
    .flatten()
    {
        end = end.max(b);
    }
    out.push(found(
        keys::LIABILITY_CAP,
        rec(vec![
            ("amount", amount.as_ref().map(money)),
            ("percent", pct.and_then(|p| float(p.0))),
            ("basis", basis.map(|b| Value::text(*b.what))),
            ("months", months.and_then(|d| d.months()).and_then(int)),
            ("text", t(clip(doc, s, MAX_TEXT))),
        ]),
        s,
        0.75,
        h.start,
        end,
    ));
}

const CONFIDENTIAL: &[(&[&str], ())] = &[
    (&["poverljiv*"], ()),
    (&["poslovn*", "tajn*"], ()),
    (&["confidential*"], ()),
    (&["non", "disclosure"], ()),
    (&["nda"], ()),
];

fn confidentiality(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if toks.len() < 6 {
        return;
    }
    let Some(h) = find(toks, CONFIDENTIAL).into_iter().next() else {
        return;
    };
    if !has(
        toks,
        &[
            "obavez*", "duzn*", "cuva*", "shall", "must", "agree*", "undertak*", "keep", "sme",
            "smeju", "otkri*", "disclose*", "cuvati",
        ],
    ) {
        return;
    }
    let d = first_duration(toks, 0, toks.len(), &[Unit::Month, Unit::Year]);
    let end = d.map_or(h.end - 1, |d| d.last.max(h.end - 1));
    let base = if d.is_some() { 0.8 } else { 0.7 };
    out.push(found(
        keys::CONFIDENTIALITY,
        rec(vec![
            ("iso", d.map(|d| Value::text(d.iso()))),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        base,
        h.start,
        end,
    ));
}

const AVAILABILITY: &[(&[&str], ())] = &[
    (&["dostupnost*"], ()),
    (&["raspolozivost*"], ()),
    (&["availability"], ()),
    (&["uptime"], ()),
];

const RESPONSE: &[(&[&str], &str)] = &[
    (&["vreme", "odziva"], "response"),
    (&["vreme", "reakcije"], "response"),
    (&["vreme", "odgovora"], "response"),
    (&["rok", "za", "odziv"], "response"),
    (&["odziv*"], "response"),
    (&["reaguje", "u", "roku", "od"], "response"),
    (&["reagovati", "u", "roku", "od"], "response"),
    (&["response", "time"], "response"),
    (&["respond", "within"], "response"),
    (&["reaction", "time"], "response"),
    (&["vreme", "otklanjanja"], "resolution"),
    (&["rok", "za", "otklanjanje"], "resolution"),
    (&["otkloni", "u", "roku", "od"], "resolution"),
    (&["otkloniti", "u", "roku", "od"], "resolution"),
    (&["resolution", "time"], "resolution"),
    (&["resolve", "within"], "resolution"),
    (&["resolved", "within"], "resolution"),
    (&["fix", "within"], "resolution"),
    (&["restore", "within"], "resolution"),
    (&["respond*"], "response"),
    (&["resolve*"], "resolution"),
    (&["restore*"], "resolution"),
    (&["fix"], "resolution"),
    (&["otklon*"], "resolution"),
];

fn sla(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    let doc = inp.doc;
    let text = clip(doc, s, MAX_TEXT);
    if let Some(h) = find(toks, AVAILABILITY).into_iter().next()
        && let Some((v, a, b)) = first_percent(doc, s, h.end, (h.end + 10).min(toks.len()))
        && (90.0..=100.0).contains(&v.to_f64())
    {
        out.push(found(
            keys::SLA,
            rec(vec![
                ("metric", t("availability")),
                ("percent", float(v)),
                ("text", t(text.as_str())),
            ]),
            s,
            0.8,
            h.start.min(a),
            b,
        ));
    }
    for h in find(toks, RESPONSE) {
        let Some(d) = first_duration(toks, h.end, h.end + 10, &[Unit::Minute, Unit::Hour, Unit::Day])
        else {
            continue;
        };
        // A single verb cue needs `within` / `u roku` before the duration.
        if h.end - h.start == 1
            && !toks[h.end..d.first]
                .iter()
                .any(|t| matches!(t.key.as_str(), "within" | "roku" | "in" | "je" | "is" | "od"))
        {
            continue;
        }
        out.push(found(
            keys::SLA,
            rec(vec![
                ("metric", t(*h.what)),
                ("iso", t(d.iso())),
                ("hours", d.hours().and_then(int)),
                ("text", t(text.as_str())),
            ]),
            s,
            0.75,
            h.start,
            d.last,
        ));
    }
}

const DEADLINE: &[(&[&str], ())] = &[
    (&["rok", "isporuke"], ()),
    (&["rok", "za", "isporuku"], ()),
    (&["rok", "izvrsenja"], ()),
    (&["rok", "za", "izvrsenje"], ()),
    (&["rok", "za", "zavrsetak"], ()),
    (&["rok", "zavrsetka"], ()),
    (&["rok", "izvodjenja"], ()),
    (&["rok", "za", "izvodjenje"], ()),
    (&["isporuci*"], ()),
    (&["isporuc*"], ()),
    (&["izvrsi", "usluge"], ()),
    (&["zavrsi*"], ()),
    (&["predati"], ()),
    (&["preda"], ()),
    (&["delivery", "date"], ()),
    (&["completion", "date"], ()),
    (&["deadline"], ()),
    (&["deliver*"], ()),
    (&["complete*"], ()),
];

/// A `signal.date` fact inside the sentence's byte range: its date and token.
fn date_in(inp: &Input<'_>, s: &Line, from: usize) -> Option<(Date, usize)> {
    let lo = s.toks.get(from)?.start;
    for (f, page, span) in inp.located("signal.date") {
        if page != s.page || span.start < lo || span.end > s.span.end {
            continue;
        }
        let Value::Date(d) = f.value else { continue };
        let k = s.toks.iter().rposition(|t| t.start < span.end)?;
        return Some((d, k));
    }
    None
}

fn deadline(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if toks.len() < 4
        || has(toks, &["plac*", "plat*", "uplat*", "pay*", "faktur*", "invoice*", "otkaz*", "notice", "garantn*", "warrant*", "reklamac*"])
    {
        return;
    }
    let Some(h) = find(toks, DEADLINE).into_iter().next() else {
        return;
    };
    let doc = inp.doc;
    let d = first_duration(toks, h.end, (h.end + 12).min(toks.len()), &[Unit::Day, Unit::Week, Unit::Month]);
    let date = date_in(inp, s, h.end);
    if d.is_none() && date.is_none() {
        return;
    }
    let mut end = h.end - 1;
    for b in [d.map(|d| d.last), date.map(|x| x.1)].into_iter().flatten() {
        end = end.max(b);
    }
    out.push(found(
        keys::DEADLINE,
        rec(vec![
            ("iso", d.map(|d| Value::text(d.iso()))),
            ("days", d.and_then(|d| d.days()).and_then(int)),
            ("date", date.map(|x| Value::Date(x.0))),
            ("text", t(clip(doc, s, MAX_TEXT))),
        ]),
        s,
        0.7,
        h.start,
        end,
    ));
}

const WARRANTY: &[(&[&str], ())] = &[
    (&["garantni", "rok"], ()),
    (&["garantnog", "roka"], ()),
    (&["garantni", "period"], ()),
    (&["garancij*", "za", "kvalitet"], ()),
    (&["warranty", "period"], ()),
    (&["warranty", "of"], ()),
    (&["warrants"], ()),
    (&["warranty"], ()),
];

fn warranty(inp: &Input<'_>, s: &Line, out: &mut Vec<Found>) {
    let toks = &s.toks;
    if has(toks, &["bankarsk*", "menic*", "bank"]) {
        return;
    }
    let Some(h) = find(toks, WARRANTY).into_iter().next() else {
        return;
    };
    let Some(d) = first_duration(toks, h.end, (h.end + 10).min(toks.len()), &[Unit::Month, Unit::Year, Unit::Day])
    else {
        return;
    };
    out.push(found(
        keys::WARRANTY,
        rec(vec![
            ("iso", t(d.iso())),
            ("months", d.months().and_then(int)),
            ("days", d.days().and_then(int)),
            ("text", t(clip(inp.doc, s, MAX_TEXT))),
        ]),
        s,
        0.75,
        h.start,
        d.last,
    ));
}

/// Every obligation and risk fact of the document.
pub(crate) fn scan(inp: &Input<'_>, out: &mut Vec<Found>) {
    for s in inp.sents {
        penalty(inp, s, out);
        guarantee(inp, s, out);
        liability_cap(inp, s, out);
        confidentiality(inp, s, out);
        sla(inp, s, out);
        deadline(inp, s, out);
        warranty(inp, s, out);
    }
}

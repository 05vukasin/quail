//! `contract.facts@1` over contracts built in code: every fact with its highlight, Serbian
//! Latin and Cyrillic and English, an amendment, OCR text, and negatives that must not match.
// One test per document checks all of its facts in a row.
#![allow(clippy::too_many_lines)]

mod common;

use common::{EN, NEGATIVE, SR_AMENDMENT, SR_CYRILLIC, SR_LATIN, doc, run};
use quail_clauses::ClauseTerms;
use quail_contract::ContractFacts;
use quail_core::{Fact, Value};
use quail_parties::PartyCandidates;
use quail_refs::RefAnalyzer;
use quail_signals::SignalScan;

fn facts(text: &str, ocr: bool) -> Vec<Fact> {
    let refs = RefAnalyzer::new();
    run(
        doc(text, ocr),
        &[
            &SignalScan,
            &refs,
            &PartyCandidates,
            &ClauseTerms,
            &ContractFacts,
        ],
    )
}

fn all<'a>(f: &'a [Fact], key: &str) -> Vec<&'a Fact> {
    f.iter().filter(|x| x.key.as_str() == key).collect()
}

fn one<'a>(f: &'a [Fact], key: &str) -> &'a Value {
    let v = all(f, key);
    assert_eq!(v.len(), 1, "{key}: {v:#?}");
    &v[0].value
}

fn s<'a>(v: &'a Value, field: &str) -> Option<&'a str> {
    v.field(field).and_then(Value::as_text)
}

fn i(v: &Value, field: &str) -> Option<i64> {
    match v.field(field)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

fn x(v: &Value, field: &str) -> Option<f64> {
    match v.field(field)? {
        Value::Float(n) => Some(*n),
        _ => None,
    }
}

fn b(v: &Value, field: &str) -> Option<bool> {
    match v.field(field)? {
        Value::Bool(n) => Some(*n),
        _ => None,
    }
}

/// The `role` record of `key`.
fn of_role<'a>(f: &'a [Fact], key: &str, role: &str) -> Vec<&'a Value> {
    all(f, key)
        .into_iter()
        .map(|x| &x.value)
        .filter(|v| s(v, "role") == Some(role))
        .collect()
}

/// Every `contract.*` fact has evidence and exactly one highlight whose quote is the page
/// text of its span; returns the quotes of `key`.
fn quotes(f: &[Fact], text: &str, key: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    let facts: Vec<&Fact> = f
        .iter()
        .filter(|x| x.key.namespace() == "contract" && x.key.as_str() != "contract.highlight")
        .collect();
    let hl = all(f, "contract.highlight");
    assert_eq!(facts.len(), hl.len(), "one highlight per fact");
    for h in &hl {
        let block = usize::try_from(i(&h.value, "block").unwrap()).unwrap();
        let (st, en) = (i(&h.value, "start").unwrap(), i(&h.value, "end").unwrap());
        let quote = s(&h.value, "quote").unwrap();
        let chars: String = lines[block]
            .chars()
            .skip(usize::try_from(st).unwrap())
            .take(usize::try_from(en - st).unwrap())
            .collect();
        assert_eq!(chars, quote, "highlight quote is the block text");
        assert!(!quote.is_empty());
        assert!(h.conf > 0.0 && h.conf <= 1.0);
    }
    hl.iter()
        .filter(|h| s(&h.value, "key") == Some(key))
        .map(|h| s(&h.value, "quote").unwrap().to_owned())
        .collect()
}

#[test]
fn serbian_latin_contract_has_every_fact() {
    let f = facts(SR_LATIN, false);
    // Parties.
    let sup = of_role(&f, "contract.party", "supplier");
    assert_eq!(s(sup[0], "name"), Some("Alfa Soft d.o.o."));
    assert_eq!(s(sup[0], "label"), Some("Izvođač"));
    let cus = of_role(&f, "contract.party", "customer");
    assert_eq!(s(cus[0], "name"), Some("Beta Trgovina d.o.o."));
    let ids: Vec<(String, String, String)> = all(&f, "contract.party_id")
        .iter()
        .map(|x| {
            (
                s(&x.value, "role").unwrap().to_owned(),
                s(&x.value, "kind").unwrap().to_owned(),
                s(&x.value, "value").unwrap().to_owned(),
            )
        })
        .collect();
    for want in [
        ("supplier", "pib", "100000024"),
        ("supplier", "mb", "07000012"),
        ("supplier", "account", "160-0000000123456-78"),
        ("customer", "pib", "100000032"),
        ("customer", "mb", "07000020"),
    ] {
        assert!(
            ids.contains(&(want.0.into(), want.1.into(), want.2.into())),
            "{want:?} in {ids:?}"
        );
    }
    assert!(
        all(&f, "contract.party_id")
            .iter()
            .all(|x| b(&x.value, "checked") == Some(true))
    );
    let addr = of_role(&f, "contract.address", "supplier");
    assert_eq!(s(addr[0], "text"), Some("Beogradu, ul. Knez Mihailova 10"));
    let rep = of_role(&f, "contract.representative", "customer");
    assert_eq!(s(rep[0], "name"), Some("Marko Marković"));
    assert_eq!(s(rep[0], "title"), Some("generalni direktor"));
    let sig = of_role(&f, "contract.signatory", "supplier");
    assert_eq!(s(sig[0], "name"), Some("Petar Petrović"));
    assert_eq!(s(sig[0], "title"), Some("direktor"));
    assert_eq!(
        s(of_role(&f, "contract.signatory", "customer")[0], "title"),
        Some("generalni direktor")
    );
    // Term.
    let term = one(&f, "contract.term");
    assert_eq!(
        (s(term, "iso"), i(term, "months")),
        (Some("P12M"), Some(12))
    );
    let ren = one(&f, "contract.renewal");
    assert_eq!(b(ren, "auto"), Some(true));
    assert_eq!(b(ren, "same_period"), Some(true));
    let notice = one(&f, "contract.notice");
    assert_eq!(
        (s(notice, "iso"), i(notice, "days")),
        (Some("P30D"), Some(30))
    );
    let kinds: Vec<&str> = all(&f, "contract.termination")
        .iter()
        .filter_map(|x| s(&x.value, "kind"))
        .collect();
    assert!(
        kinds.contains(&"breach") && kinds.contains(&"notice"),
        "{kinds:?}"
    );
    // Payment terms.
    let pd = one(&f, "contract.payment_days");
    assert_eq!(
        (i(pd, "days"), s(pd, "from")),
        (Some(15), Some("invoice_receipt"))
    );
    assert_eq!(
        one(&f, "contract.payment_method").as_text(),
        Some("bank_transfer")
    );
    assert_eq!(x(one(&f, "contract.advance"), "percent"), Some(30.0));
    assert_eq!(s(one(&f, "contract.indexation"), "basis"), Some("cpi"));
    assert_eq!(
        b(one(&f, "contract.late_interest"), "statutory"),
        Some(true)
    );
    // Obligations.
    let pen = &all(&f, "contract.penalty")[0].value;
    assert_eq!(
        (x(pen, "percent"), s(pen, "per"), x(pen, "cap_percent")),
        (Some(0.5), Some("day"), Some(10.0))
    );
    let g = &all(&f, "contract.guarantee")[0].value;
    assert_eq!(
        (s(g, "kind"), s(g, "purpose"), x(g, "percent")),
        (Some("bank_guarantee"), Some("performance"), Some(10.0))
    );
    assert_eq!(
        s(one(&f, "contract.liability_cap"), "basis"),
        Some("contract_value")
    );
    assert_eq!(s(one(&f, "contract.confidentiality"), "iso"), Some("P3Y"));
    let sla: Vec<&Value> = all(&f, "contract.sla").iter().map(|x| &x.value).collect();
    assert!(
        sla.iter()
            .any(|v| s(v, "metric") == Some("availability") && x(v, "percent") == Some(99.5))
    );
    assert!(
        sla.iter()
            .any(|v| s(v, "metric") == Some("response") && i(v, "hours") == Some(4))
    );
    assert_eq!(i(&all(&f, "contract.deadline")[0].value, "days"), Some(10));
    assert_eq!(i(one(&f, "contract.warranty"), "months"), Some(12));
    // Law.
    assert_eq!(s(one(&f, "contract.governing_law"), "code"), Some("RS"));
    let court = one(&f, "contract.court");
    assert_eq!(
        (s(court, "court"), s(court, "city")),
        (Some("Privredni sud u Beogradu"), Some("Beograd"))
    );
    let lang = one(&f, "contract.language");
    assert_eq!(
        lang.field("languages"),
        Some(&Value::List(vec![Value::text("sr")]))
    );
    // Document.
    let annexes: Vec<(&str, Option<&str>)> = all(&f, "contract.annex")
        .iter()
        .map(|x| (s(&x.value, "label").unwrap(), s(&x.value, "title")))
        .collect();
    assert_eq!(
        annexes,
        [
            ("Prilog 1", Some("Tehnička specifikacija")),
            ("Prilog 2", Some("Cenovnik"))
        ]
    );
    // Highlights.
    assert_eq!(
        quotes(&f, SR_LATIN, "contract.notice"),
        ["otkazni rok od 30 dana"]
    );
    assert_eq!(
        quotes(&f, SR_LATIN, "contract.court"),
        ["Privredni sud u Beogradu"]
    );
    assert_eq!(
        quotes(&f, SR_LATIN, "contract.payment_days"),
        ["u roku od 15 dana od dana prijema fakture"]
    );
    assert_eq!(quotes(&f, SR_LATIN, "contract.party_id").len(), 5);
}

#[test]
fn serbian_cyrillic_contract() {
    let f = facts(SR_CYRILLIC, false);
    let sup = of_role(&f, "contract.party", "supplier");
    assert_eq!(s(sup[0], "label"), Some("Закуподавац"));
    assert_eq!(s(sup[0], "name"), Some("Гама Некретнине д.о.о."));
    assert_eq!(
        s(of_role(&f, "contract.party", "customer")[0], "label"),
        Some("Закупац")
    );
    assert!(
        of_role(&f, "contract.party_id", "customer")
            .iter()
            .any(|v| s(v, "kind") == Some("pib") && s(v, "value") == Some("100000032"))
    );
    let rep = of_role(&f, "contract.representative", "supplier");
    assert_eq!(s(rep[0], "name"), Some("Јован Јовановић"));
    assert_eq!(i(one(&f, "contract.term"), "months"), Some(24));
    assert_eq!(b(one(&f, "contract.renewal"), "auto"), Some(true));
    assert_eq!(i(one(&f, "contract.notice"), "days"), Some(60));
    assert_eq!(i(one(&f, "contract.payment_days"), "days"), Some(8));
    let pen = &all(&f, "contract.penalty")[0].value;
    assert_eq!((x(pen, "percent"), s(pen, "per")), (Some(0.1), Some("day")));
    assert_eq!(
        s(&all(&f, "contract.guarantee")[0].value, "kind"),
        Some("bill_of_exchange")
    );
    assert_eq!(s(one(&f, "contract.governing_law"), "code"), Some("RS"));
    let court = one(&f, "contract.court");
    assert_eq!(s(court, "city"), Some("Niš"));
    assert_eq!(s(court, "court"), Some("Privredni sud u Nišu"));
    assert_eq!(
        s(&all(&f, "contract.annex")[0].value, "label"),
        Some("Прилог 1")
    );
    assert_eq!(
        quotes(&f, SR_CYRILLIC, "contract.notice"),
        ["отказни рок од 60 дана"]
    );
}

#[test]
fn english_agreement_has_every_fact() {
    let f = facts(EN, false);
    let sup = of_role(&f, "contract.party", "supplier");
    assert_eq!(s(sup[0], "name"), Some("Alpha Widgets LLC"));
    let rep = of_role(&f, "contract.representative", "supplier");
    assert_eq!(
        (s(rep[0], "name"), s(rep[0], "title")),
        (Some("John Smith"), Some("Chief Executive Officer"))
    );
    assert_eq!(
        s(of_role(&f, "contract.address", "supplier")[0], "text"),
        Some("1 Main Street, Dover, Delaware")
    );
    let ids: Vec<(Option<&str>, Option<&str>)> = of_role(&f, "contract.party_id", "customer")
        .iter()
        .map(|v| (s(v, "kind"), s(v, "value")))
        .collect();
    assert!(ids.contains(&(Some("vat"), Some("DE136695976"))), "{ids:?}");
    assert!(ids.contains(&(Some("reg"), Some("HRB 12345"))), "{ids:?}");
    let sig = of_role(&f, "contract.signatory", "customer");
    assert_eq!(
        (s(sig[0], "name"), s(sig[0], "title")),
        (Some("Anna Weber"), Some("Managing Director"))
    );
    assert_eq!(
        s(of_role(&f, "contract.signatory", "supplier")[0], "name"),
        Some("John Smith")
    );
    assert_eq!(s(one(&f, "contract.term"), "iso"), Some("P2Y"));
    assert_eq!(b(one(&f, "contract.renewal"), "auto"), Some(true));
    assert_eq!(i(one(&f, "contract.notice"), "days"), Some(90));
    let kinds: Vec<&str> = all(&f, "contract.termination")
        .iter()
        .filter_map(|x| s(&x.value, "kind"))
        .collect();
    for k in ["convenience", "breach", "insolvency"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    let pd = one(&f, "contract.payment_days");
    assert_eq!(
        (i(pd, "days"), s(pd, "from")),
        (Some(30), Some("invoice_receipt"))
    );
    assert_eq!(
        one(&f, "contract.payment_method").as_text(),
        Some("bank_transfer")
    );
    assert_eq!(x(one(&f, "contract.advance"), "percent"), Some(20.0));
    assert_eq!(s(one(&f, "contract.indexation"), "basis"), Some("fixed"));
    let cap = one(&f, "contract.liability_cap");
    assert_eq!(
        (s(cap, "basis"), i(cap, "months")),
        (Some("fees_paid"), Some(12))
    );
    let pen = &all(&f, "contract.penalty")[0].value;
    assert_eq!(
        (x(pen, "percent"), s(pen, "per"), x(pen, "cap_percent")),
        (Some(1.0), Some("week"), Some(10.0))
    );
    assert_eq!(
        s(&all(&f, "contract.guarantee")[0].value, "kind"),
        Some("performance_bond")
    );
    let sla: Vec<&Value> = all(&f, "contract.sla").iter().map(|x| &x.value).collect();
    for (m, h) in [("response", 2), ("resolution", 8)] {
        assert!(
            sla.iter()
                .any(|v| s(v, "metric") == Some(m) && i(v, "hours") == Some(h)),
            "{m}: {sla:?}"
        );
    }
    assert!(sla.iter().any(|v| x(v, "percent") == Some(99.9)));
    assert_eq!(s(one(&f, "contract.confidentiality"), "iso"), Some("P5Y"));
    assert_eq!(s(one(&f, "contract.governing_law"), "code"), Some("GB-EAW"));
    assert_eq!(s(one(&f, "contract.court"), "city"), Some("London"));
    let arb = one(&f, "contract.arbitration");
    assert_eq!(
        (s(arb, "rules"), s(arb, "seat")),
        (Some("ICC"), Some("London"))
    );
    assert_eq!(s(one(&f, "contract.language"), "prevails"), Some("en"));
    let annexes: Vec<&str> = all(&f, "contract.annex")
        .iter()
        .filter_map(|x| s(&x.value, "label"))
        .collect();
    assert_eq!(annexes, ["Annex A", "Schedule 1"]);
    assert_eq!(
        quotes(&f, EN, "contract.penalty"),
        ["Liquidated damages of 1% per week of delay apply, up to a maximum of 10%"]
    );
}

#[test]
fn amendment_says_what_it_changes() {
    let f = facts(SR_AMENDMENT, false);
    let a: Vec<(Option<&str>, Option<&str>)> = all(&f, "contract.amendment")
        .iter()
        .map(|x| (s(&x.value, "action"), s(&x.value, "article")))
        .collect();
    assert_eq!(
        a,
        [
            (Some("change"), Some("član 2")),
            (Some("delete"), Some("član 7"))
        ]
    );
    quotes(&f, SR_AMENDMENT, "contract.amendment");
}

#[test]
fn ocr_text_gives_the_same_facts_at_a_lower_confidence() {
    let native = facts(SR_LATIN, false);
    let ocr = facts(SR_LATIN, true);
    let keys = |f: &[Fact]| {
        let mut k: Vec<String> = f
            .iter()
            .filter(|x| x.key.namespace() == "contract")
            .map(|x| x.key.as_str().to_owned())
            .collect();
        k.sort();
        k
    };
    assert_eq!(keys(&native), keys(&ocr));
    let n = one(&native, "contract.notice");
    assert_eq!(n, one(&ocr, "contract.notice"));
    let c = |f: &[Fact]| all(f, "contract.notice")[0].conf;
    assert!(c(&ocr) < c(&native));
}

#[test]
fn negatives_do_not_match() {
    let f = facts(NEGATIVE, false);
    let got: Vec<&str> = f
        .iter()
        .filter(|x| x.key.namespace() == "contract")
        .map(|x| x.key.as_str())
        .collect();
    assert!(got.is_empty(), "{got:?}");
    // Near misses one by one.
    for text in [
        // A deadline that is not about paying, a notice that is not a notice period.
        "Rok za odgovor na reklamaciju je 8 dana od prijema.",
        // `garantni rok` without a duration, a bank guarantee is not a warranty.
        "Garantni rok važi prema proizvođaču. Bankarska garancija nije potrebna za 12 meseci.",
        // Not a court clause: no court is named.
        "Za sporove je nadležan sud.",
        // Arbitration negated.
        "Sporovi se neće rešavati arbitražom.",
        // A role word without a defined term or a label.
        "Kupac je kupio robu u prodavnici.",
        // An annex mentioned in passing, inflected.
        "U skladu sa Prilogom 1 strane su se dogovorile.",
        // Durations of other things.
        "Projekat traje 12 meseci prema planu.",
    ] {
        let f = facts(text, false);
        let got: Vec<String> = f
            .iter()
            .filter(|x| x.key.namespace() == "contract")
            .map(|x| format!("{} {:?}", x.key, x.value))
            .collect();
        assert!(
            got.iter().all(|g| g.starts_with("contract.highlight")) || got.is_empty(),
            "{text}: {got:#?}"
        );
    }
}

#[test]
fn empty_and_tiny_inputs() {
    for text in [
        "",
        " ",
        ".",
        "PIB",
        "Naručilac:",
        "(u daljem tekstu: Kupac)",
        "Za Naručioca",
    ] {
        let f = facts(text, false);
        assert!(
            f.iter().all(|x| x.key.namespace() != "contract"),
            "{text}: {f:#?}"
        );
    }
}

#[test]
fn overlapping_and_cut_off_cues_do_not_panic() {
    for text in [
        "A d.o.o. (u daljem tekstu: Kupac), represented by John Smith (the Seller) (the Buyer)",
        "dalje Kupac, koga zastupa dalje Prodavac koga zastupa",
        "(the \"Supplier\"), represented by",
        "PIB PIB PIB 1 MB",
        "Za Naručioca\nZa Izvođača\nName:\nTitle:",
        "Prilozi:\n1.\n2. -",
        "u roku od 999999999999 dana od dana prijema fakture",
        "Ugovorna kazna 0,5% najviše",
        "nadležan je sud u",
        "arbitraža pri",
        "Član\nČlan 1.\n1.\ni\nand",
    ] {
        let _ = facts(text, false);
        let _ = facts(text, true);
    }
}

#[test]
fn more_forms() {
    let text = "UGOVOR O SARADNJI
Ugovor se zaključuje na neodređeno vreme.
Ugovor se ne produžava automatski.
Sporove rešava Spoljnotrgovinska arbitraža pri Privrednoj komori Srbije.
Ugovor je sačinjen na srpskom i engleskom jeziku, a u slučaju razlike merodavna je verzija na srpskom jeziku.
Payment terms: Net 30.
Prilozi:
1. Cenovnik
2. Plan isporuke";
    let f = facts(text, false);
    assert_eq!(b(one(&f, "contract.term"), "indefinite"), Some(true));
    assert_eq!(b(one(&f, "contract.renewal"), "auto"), Some(false));
    assert_eq!(
        s(one(&f, "contract.arbitration"), "institution"),
        Some("Spoljnotrgovinska arbitraža pri Privrednoj komori Srbije")
    );
    let lang = one(&f, "contract.language");
    assert_eq!(
        lang.field("languages"),
        Some(&Value::List(vec![Value::text("sr"), Value::text("en")]))
    );
    assert_eq!(s(lang, "prevails"), Some("sr"));
    assert_eq!(i(one(&f, "contract.payment_days"), "days"), Some(30));
    let annexes: Vec<(&str, Option<&str>)> = all(&f, "contract.annex")
        .iter()
        .map(|x| (s(&x.value, "label").unwrap(), s(&x.value, "title")))
        .collect();
    assert_eq!(
        annexes,
        [
            ("Prilog 1", Some("Cenovnik")),
            ("Prilog 2", Some("Plan isporuke"))
        ]
    );
    quotes(&f, text, "contract.annex");
}

/// A long document (the Latin contract many times over, about 1.5 MB): the scan stays
/// linear and every key keeps its cap.
#[test]
fn long_document_is_bounded() {
    let text = vec![SR_LATIN; 400].join("\n");
    let t0 = std::time::Instant::now();
    let f = run(doc(&text, false), &[&ContractFacts]);
    let took = t0.elapsed();
    eprintln!("{} bytes, {} facts in {took:?}", text.len(), f.len());
    assert!(took.as_secs() < 60, "{took:?}");
    let mut keys: Vec<&str> = f.iter().map(|x| x.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        if k != "contract.highlight" {
            assert!(
                f.iter().filter(|x| x.key.as_str() == k).count() <= quail_contract::MAX_PER_KEY
            );
        }
    }
}

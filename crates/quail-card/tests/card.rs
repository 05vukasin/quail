//! `card.facts@1` after the steps the engine runs before it, on the contracts of
//! `quail-contract`'s tests: one normalised value per field, each with a highlight.
// One test per document checks all of its facts in a row.
#![allow(clippy::too_many_lines)]

#[path = "../../quail-contract/tests/common/mod.rs"]
mod common;

use common::{EN, NEGATIVE, SR_CYRILLIC, SR_LATIN, doc, run};
use quail_card::KeyFactsCard;
use quail_clauses::ClauseTerms;
use quail_contract::ContractFacts;
use quail_core::{Date, Fact, Value};
use quail_items::ItemLines;
use quail_parties::PartyCandidates;
use quail_payment::PaymentExtract;
use quail_refs::RefAnalyzer;
use quail_signals::SignalScan;

fn card(text: &str) -> Vec<Fact> {
    let refs = RefAnalyzer::new();
    let items = ItemLines::new();
    run(
        doc(text, false),
        &[
            &SignalScan,
            &refs,
            &PartyCandidates,
            &ClauseTerms,
            &items,
            &PaymentExtract,
            &ContractFacts,
            &KeyFactsCard,
        ],
    )
    .into_iter()
    .filter(|f| f.key.namespace() == "card")
    .collect()
}

fn get<'a>(f: &'a [Fact], key: &str) -> Option<&'a Value> {
    let v: Vec<&Fact> = f.iter().filter(|x| x.key.as_str() == key).collect();
    assert!(v.len() <= 1, "{key}: {v:#?}");
    v.first().map(|x| &x.value)
}

fn field<'a>(f: &'a [Fact], key: &str, name: &str) -> Option<&'a Value> {
    get(f, key)?.field(name)
}

fn text<'a>(f: &'a [Fact], key: &str) -> Option<&'a str> {
    get(f, key)?.as_text()
}

fn list(f: &[Fact], key: &str) -> Vec<String> {
    match get(f, key) {
        Some(Value::List(v)) => v
            .iter()
            .filter_map(Value::as_text)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Every card field but the text form has at least one highlight under its own key.
fn highlighted(f: &[Fact]) {
    for x in f {
        let k = x.key.as_str();
        if matches!(k, "card.highlight" | "card.text") {
            continue;
        }
        assert!(
            f.iter().any(|h| h.key.as_str() == "card.highlight"
                && h.value.field("key").and_then(Value::as_text) == Some(k)),
            "{k} has no highlight"
        );
    }
}

#[test]
fn serbian_card() {
    let f = card(SR_LATIN);
    assert_eq!(text(&f, "card.kind"), Some("contract"));
    assert_eq!(text(&f, "card.number"), Some("114/2026"));
    let sup = get(&f, "card.supplier").unwrap();
    assert_eq!(
        sup.field("name").and_then(Value::as_text),
        Some("Alfa Soft d.o.o.")
    );
    assert_eq!(sup.field("pib").and_then(Value::as_text), Some("100000024"));
    assert_eq!(sup.field("mb").and_then(Value::as_text), Some("07000012"));
    assert_eq!(
        sup.field("representative").and_then(Value::as_text),
        Some("Petar Petrović, direktor")
    );
    assert_eq!(
        field(&f, "card.customer", "pib").and_then(Value::as_text),
        Some("100000032")
    );
    assert_eq!(
        get(&f, "card.signed"),
        Some(&Value::Date(Date {
            y: 2026,
            m: 1,
            d: 15
        }))
    );
    assert_eq!(field(&f, "card.term", "months"), Some(&Value::Int(12)));
    assert_eq!(get(&f, "card.auto_renewal"), Some(&Value::Bool(true)));
    assert_eq!(field(&f, "card.notice", "days"), Some(&Value::Int(30)));
    assert_eq!(get(&f, "card.payment_days"), Some(&Value::Int(15)));
    assert_eq!(text(&f, "card.payment_method"), Some("bank_transfer"));
    assert_eq!(text(&f, "card.currency"), Some("RSD"));
    assert!(
        matches!(get(&f, "card.amount"), Some(Value::Money(m)) if m.units == 120_000_000 && m.scale == 2)
    );
    assert_eq!(text(&f, "card.category"), Some("it_software"));
    assert_eq!(list(&f, "card.accounts"), ["160-0000000123456-78"]);
    assert_eq!(
        field(&f, "card.advance", "percent"),
        Some(&Value::Float(30.0))
    );
    assert_eq!(text(&f, "card.indexation"), Some("cpi"));
    assert_eq!(
        field(&f, "card.penalty", "per").and_then(Value::as_text),
        Some("day")
    );
    assert_eq!(list(&f, "card.guarantees"), ["bank_guarantee"]);
    assert_eq!(
        field(&f, "card.liability_cap", "basis").and_then(Value::as_text),
        Some("contract_value")
    );
    assert_eq!(get(&f, "card.confidentiality"), Some(&Value::Bool(true)));
    assert_eq!(
        field(&f, "card.sla", "availability"),
        Some(&Value::Float(99.5))
    );
    assert_eq!(
        field(&f, "card.sla", "response_hours"),
        Some(&Value::Int(4))
    );
    assert_eq!(field(&f, "card.warranty", "months"), Some(&Value::Int(12)));
    assert_eq!(text(&f, "card.governing_law"), Some("RS"));
    assert_eq!(text(&f, "card.court"), Some("Privredni sud u Beogradu"));
    assert_eq!(list(&f, "card.languages"), ["sr"]);
    assert_eq!(list(&f, "card.annexes"), ["Prilog 1", "Prilog 2"]);
    let t = text(&f, "card.text").unwrap();
    for line in [
        "signed: 2026-01-15",
        "amount: 1200000.00 RSD",
        "notice: iso=P30D; days=30",
        "governing_law: RS",
    ] {
        assert!(t.lines().any(|l| l == line), "{line} in\n{t}");
    }
    highlighted(&f);
}

#[test]
fn cyrillic_card() {
    let f = card(SR_CYRILLIC);
    assert_eq!(
        field(&f, "card.supplier", "name").and_then(Value::as_text),
        Some("Гама Некретнине д.о.о.")
    );
    assert_eq!(
        field(&f, "card.customer", "pib").and_then(Value::as_text),
        Some("100000032")
    );
    assert_eq!(field(&f, "card.term", "months"), Some(&Value::Int(24)));
    assert_eq!(get(&f, "card.payment_days"), Some(&Value::Int(8)));
    assert_eq!(list(&f, "card.guarantees"), ["bill_of_exchange"]);
    assert_eq!(text(&f, "card.court"), Some("Privredni sud u Nišu"));
    highlighted(&f);
}

#[test]
fn english_card() {
    let f = card(EN);
    assert_eq!(text(&f, "card.kind"), Some("msa"));
    assert_eq!(text(&f, "card.number"), Some("MSA-2026-07"));
    assert_eq!(
        field(&f, "card.customer", "vat").and_then(Value::as_text),
        Some("DE136695976")
    );
    assert_eq!(
        get(&f, "card.signed"),
        Some(&Value::Date(Date {
            y: 2026,
            m: 3,
            d: 1
        }))
    );
    assert_eq!(field(&f, "card.term", "months"), Some(&Value::Int(24)));
    assert_eq!(field(&f, "card.notice", "days"), Some(&Value::Int(90)));
    assert_eq!(get(&f, "card.payment_days"), Some(&Value::Int(30)));
    assert_eq!(list(&f, "card.accounts"), ["DE89370400440532013000"]);
    assert_eq!(text(&f, "card.indexation"), Some("fixed"));
    assert_eq!(list(&f, "card.guarantees"), ["performance_bond"]);
    assert_eq!(text(&f, "card.governing_law"), Some("GB-EAW"));
    assert_eq!(text(&f, "card.arbitration"), Some("ICC"));
    assert_eq!(list(&f, "card.annexes"), ["Annex A", "Schedule 1"]);
    highlighted(&f);
}

#[test]
fn nothing_to_say_gives_no_contract_fields() {
    let f = card(NEGATIVE);
    let keys: Vec<&str> = f
        .iter()
        .map(|x| x.key.as_str())
        .filter(|k| {
            !matches!(
                *k,
                "card.text" | "card.highlight" | "card.kind" | "card.subject" | "card.category"
            )
        })
        .collect();
    assert!(keys.is_empty(), "{keys:?}");
}

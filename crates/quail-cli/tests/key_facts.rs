//! `contract.facts@1` and `card.facts@1` end to end: generated contracts (PDF with a text
//! layer, plain text in Cyrillic) through `quail scan`. The scanned copies of the PDFs are
//! read through OCR in `key_facts_ocr.rs`.
#![cfg(all(
    feature = "contract",
    feature = "card",
    feature = "pdf",
    feature = "text"
))]

use std::fs;
use std::process::Command;

use quail_core::{FileRecord, Value};

mod common;
use common::{KEY_EN_CONTRACT, KEY_SR_CONTRACT, PdfPage, pdf};

fn scan(path: &std::path::Path) -> FileRecord {
    let o = Command::new(env!("CARGO_BIN_EXE_quail"))
        .args(["scan", path.to_str().unwrap(), "-q"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap()
}

fn get<'a>(r: &'a FileRecord, key: &'a str) -> Option<&'a Value> {
    r.facts_for(key).next().map(|f| &f.value)
}

fn field<'a>(r: &'a FileRecord, key: &'a str, name: &str) -> Option<&'a Value> {
    get(r, key)?.field(name)
}

fn text<'a>(r: &'a FileRecord, key: &'a str) -> Option<&'a str> {
    get(r, key)?.as_text()
}

/// Every `contract.*` and `card.*` fact has a highlight in its namespace.
fn highlighted(r: &FileRecord) {
    for f in &r.facts {
        let ns = f.key.namespace();
        if !matches!(ns, "contract" | "card") || f.key.as_str().ends_with(".highlight") {
            continue;
        }
        if f.key.as_str() == "card.text" {
            continue;
        }
        let hk = format!("{ns}.highlight");
        assert!(
            r.facts_for(&hk)
                .any(|h| h.value.field("key").and_then(Value::as_text) == Some(f.key.as_str())),
            "{} has no highlight",
            f.key
        );
    }
}

#[test]
fn serbian_contract_pdf() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("ugovor.pdf");
    fs::write(&p, pdf(&[PdfPage::Text(KEY_SR_CONTRACT)], "ugovor")).unwrap();
    let r = scan(&p);
    assert_eq!(text(&r, "card.number"), Some("114/2026"));
    assert_eq!(
        field(&r, "card.supplier", "name").and_then(Value::as_text),
        Some("Alfa Soft d.o.o.")
    );
    assert_eq!(
        field(&r, "card.supplier", "pib").and_then(Value::as_text),
        Some("100000024")
    );
    assert_eq!(
        field(&r, "card.supplier", "representative").and_then(Value::as_text),
        Some("Petar Petrovic, direktor")
    );
    assert_eq!(
        field(&r, "card.customer", "mb").and_then(Value::as_text),
        Some("07000020")
    );
    assert_eq!(field(&r, "card.term", "months"), Some(&Value::Int(12)));
    assert_eq!(get(&r, "card.auto_renewal"), Some(&Value::Bool(true)));
    assert_eq!(field(&r, "card.notice", "days"), Some(&Value::Int(30)));
    assert_eq!(get(&r, "card.payment_days"), Some(&Value::Int(15)));
    assert_eq!(
        field(&r, "card.penalty", "percent"),
        Some(&Value::Float(0.5))
    );
    assert_eq!(field(&r, "card.warranty", "months"), Some(&Value::Int(12)));
    assert_eq!(text(&r, "card.governing_law"), Some("RS"));
    assert_eq!(text(&r, "card.court"), Some("Privredni sud u Beogradu"));
    assert!(r.facts_for("contract.signatory").next().is_some());
    assert!(r.facts_for("contract.annex").next().is_some());
    highlighted(&r);
}

#[test]
fn english_contract_pdf() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("agreement.pdf");
    fs::write(&p, pdf(&[PdfPage::Text(KEY_EN_CONTRACT)], "agreement")).unwrap();
    let r = scan(&p);
    assert_eq!(
        field(&r, "card.customer", "vat").and_then(Value::as_text),
        Some("DE136695976")
    );
    assert_eq!(field(&r, "card.term", "months"), Some(&Value::Int(24)));
    assert_eq!(field(&r, "card.notice", "days"), Some(&Value::Int(90)));
    assert_eq!(get(&r, "card.payment_days"), Some(&Value::Int(30)));
    assert_eq!(get(&r, "card.confidentiality"), Some(&Value::Bool(true)));
    assert_eq!(text(&r, "card.governing_law"), Some("GB-EAW"));
    assert_eq!(text(&r, "card.currency"), Some("EUR"));
    highlighted(&r);
}

#[test]
fn cyrillic_text_file() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("ugovor.txt");
    fs::write(
        &p,
        "УГОВОР О ЗАКУПУ\nУговор бр. 7/2026\n1. Гама Некретнине д.о.о., ПИБ 100000024 (у даљем тексту: Закуподавац)\nи\n2. Делта Трговина д.о.о., ПИБ 100000032 (у даљем тексту: Закупац)\nУговор се закључује на период од 2 године.\nСвака страна може отказати уговор уз отказни рок од 60 дана.\nЗа све спорове надлежан је Привредни суд у Нишу.\n",
    )
    .unwrap();
    let r = scan(&p);
    assert_eq!(
        field(&r, "card.supplier", "pib").and_then(Value::as_text),
        Some("100000024")
    );
    assert_eq!(field(&r, "card.term", "months"), Some(&Value::Int(24)));
    assert_eq!(field(&r, "card.notice", "days"), Some(&Value::Int(60)));
    assert_eq!(text(&r, "card.court"), Some("Privredni sud u Nišu"));
    highlighted(&r);
}

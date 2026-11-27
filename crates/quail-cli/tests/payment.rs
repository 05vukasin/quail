//! `payment.extract@1` end to end: generated contracts and invoices (PDF with a text layer,
//! plain text in Cyrillic) through `quail scan`. The scanned copies of the same PDFs are
//! read through OCR in `payment_ocr.rs`.
#![cfg(all(feature = "payment", feature = "pdf", feature = "text"))]

use std::fs;
use std::process::Command;

use quail_core::{FileRecord, Value};

mod common;
use common::{
    PAY_EN_INVOICE, PAY_EN_MONTHLY, PAY_SR_CONTRACT, PAY_SR_INVOICE, PAY_SR_NO_PRICE, PdfPage, pdf,
};

fn scan(path: &std::path::Path) -> FileRecord {
    let o = Command::new(env!("CARGO_BIN_EXE_quail"))
        .args(["scan", path.to_str().unwrap(), "-q"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap()
}

fn money(r: &FileRecord, key: &str) -> Option<(f64, String)> {
    match &r.facts_for(key).next()?.value {
        Value::Money(m) => {
            #[allow(clippy::cast_precision_loss)]
            let v = m.units as f64 / 10f64.powi(i32::from(m.scale));
            let c = m
                .currency
                .map(|c| String::from_utf8_lossy(&c).into_owned())
                .unwrap_or_default();
            Some((v, c))
        }
        _ => None,
    }
}

fn text<'a>(r: &'a FileRecord, key: &'a str) -> Option<&'a str> {
    r.facts_for(key).next()?.value.as_text()
}

fn field<'a>(r: &'a FileRecord, key: &'a str, name: &str) -> Option<&'a str> {
    r.facts_for(key).next()?.value.field(name)?.as_text()
}

fn is(r: &FileRecord, key: &str, v: f64, c: &str) {
    let (x, cur) = money(r, key).unwrap_or_else(|| panic!("no {key}: {:#?}", r.facts));
    assert!((x - v).abs() < 0.005, "{key}: {x} != {v}");
    assert_eq!(cur, c, "{key}");
}

fn pdf_file(dir: &std::path::Path, name: &str, lines: &[&str]) -> std::path::PathBuf {
    let p = dir.join(name);
    fs::write(&p, pdf(&[PdfPage::Text(lines)], name)).unwrap();
    p
}

#[test]
fn serbian_contract_pdf() {
    let t = tempfile::tempdir().unwrap();
    let r = scan(&pdf_file(t.path(), "ugovor.pdf", PAY_SR_CONTRACT));
    is(&r, "payment.net", 1_234_567.89, "RSD");
    is(&r, "payment.vat", 246_913.58, "RSD");
    is(&r, "payment.gross", 1_481_481.47, "RSD");
    is(&r, "payment.amount", 1_481_481.47, "RSD");
    assert_eq!(text(&r, "payment.currency"), Some("RSD"));
    assert_eq!(
        field(&r, "payment.subject", "text"),
        Some("pruzanje usluga odrzavanja poslovnog softvera Narucioca")
    );
    assert_eq!(field(&r, "payment.category", "code"), Some("it_software"));
    assert!(r.facts_for("payment.highlight").count() >= 6);
}

#[test]
fn english_monthly_fee_pdf() {
    let t = tempfile::tempdir().unwrap();
    let r = scan(&pdf_file(t.path(), "subscription.pdf", PAY_EN_MONTHLY));
    is(&r, "payment.amount", 2500.0, "EUR");
    is(&r, "payment.net", 2500.0, "EUR");
    assert_eq!(text(&r, "payment.amount_basis"), Some("net"));
    assert_eq!(text(&r, "payment.period"), Some("monthly"));
    assert_eq!(field(&r, "payment.category", "code"), Some("it_software"));
}

#[test]
fn english_invoice_pdf() {
    let t = tempfile::tempdir().unwrap();
    let r = scan(&pdf_file(t.path(), "invoice.pdf", PAY_EN_INVOICE));
    is(&r, "payment.net", 1800.0, "USD");
    is(&r, "payment.vat", 144.0, "USD");
    is(&r, "payment.amount", 1944.0, "USD");
    assert_eq!(text(&r, "payment.amount_basis"), Some("gross"));
    assert_eq!(text(&r, "payment.period"), Some("one_off"));
    let Some(Value::List(lines)) = r.facts_for("payment.lines").next().map(|f| &f.value) else {
        panic!("no payment.lines: {:#?}", r.facts);
    };
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(
        lines[0].field("description").and_then(Value::as_text),
        Some("Consulting services")
    );
    assert!(matches!(lines[0].field("total"), Some(Value::Money(m)) if m.units == 150_000));
    assert_eq!(field(&r, "payment.subject", "source"), Some("line_items"));
    assert_eq!(field(&r, "payment.category", "code"), Some("consulting"));
}

#[test]
fn serbian_invoice_pdf() {
    let t = tempfile::tempdir().unwrap();
    let r = scan(&pdf_file(t.path(), "faktura.pdf", PAY_SR_INVOICE));
    is(&r, "payment.net", 40_000.0, "RSD");
    is(&r, "payment.vat", 8_000.0, "RSD");
    is(&r, "payment.amount", 48_000.0, "RSD");
    assert_eq!(text(&r, "payment.period"), Some("one_off"));
    let Some(Value::List(lines)) = r.facts_for("payment.lines").next().map(|f| &f.value) else {
        panic!("no payment.lines: {:#?}", r.facts);
    };
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(field(&r, "payment.category", "code"), Some("it_software"));
}

#[test]
fn contract_without_a_price_pdf() {
    let t = tempfile::tempdir().unwrap();
    let r = scan(&pdf_file(t.path(), "saradnja.pdf", PAY_SR_NO_PRICE));
    for k in ["payment.amount", "payment.net", "payment.gross", "payment.currency"] {
        assert!(r.facts_for(k).next().is_none(), "{k}: {:#?}", r.facts);
    }
    assert!(r.facts_for("payment.subject").next().is_some());
}

#[test]
fn serbian_cyrillic_text_file() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("ugovor.txt");
    fs::write(
        &p,
        "УГОВОР О ПРУЖАЊУ УСЛУГА ОДРЖАВАЊА\n\n\
         Предмет уговора је одржавање рачунарске мреже и сервера Наручиоца.\n\n\
         Месечна накнада износи 120.000,00 динара без ПДВ-а.\n",
    )
    .unwrap();
    let r = scan(&p);
    is(&r, "payment.amount", 120_000.0, "RSD");
    assert_eq!(text(&r, "payment.amount_basis"), Some("net"));
    assert_eq!(text(&r, "payment.period"), Some("monthly"));
    assert_eq!(
        field(&r, "payment.subject", "text"),
        Some("одржавање рачунарске мреже и сервера Наручиоца")
    );
    assert_eq!(field(&r, "payment.category", "code"), Some("it_software"));
}

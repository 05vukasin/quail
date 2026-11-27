//! `payment.extract@1` on scans: the generated contracts and invoices of `payment.rs`,
//! rendered at 200 dpi into image-only PDFs, read with `--profile deep` and the real OCR
//! backend (feature `ocr`), must give the same payment facts as the text-layer PDFs: amounts
//! within half a percent, the same currency, category, period and amount basis, and a subject
//! with most of the same words. Needs the models in `$QUAIL_OCR_MODELS`; without them every
//! test here skips.
#![cfg(feature = "ocr")]

use std::fs;
use std::path::Path;
use std::process::Command;

use quail_core::{FileRecord, Value};

mod common;
use common::{
    PAY_EN_INVOICE, PAY_EN_MONTHLY, PAY_SR_CONTRACT, PAY_SR_INVOICE, PAY_SR_NO_PRICE, PdfPage, pdf,
    scanned,
};

fn models() -> Option<String> {
    let dir = std::env::var("QUAIL_OCR_MODELS").ok()?;
    let o = Command::new(env!("CARGO_BIN_EXE_quail"))
        .args(["models", "list", "--dir", &dir])
        .output()
        .ok()?;
    let listing = String::from_utf8_lossy(&o.stdout).into_owned();
    if o.status.success() && !listing.contains("file(s) to fetch") {
        Some(dir)
    } else {
        eprintln!("OCR models not in {dir}; skipping");
        None
    }
}

fn scan(models: &str, path: &Path, deep: bool) -> FileRecord {
    let mut args = vec!["scan", path.to_str().unwrap(), "-q"];
    if deep {
        args.extend(["--profile", "deep"]);
    }
    let o = Command::new(env!("CARGO_BIN_EXE_quail"))
        .env("QUAIL_OCR_MODELS", models)
        .args(&args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap()
}

fn money(r: &FileRecord, key: &str) -> Option<(f64, Option<[u8; 3]>)> {
    match &r.facts_for(key).next()?.value {
        Value::Money(m) => {
            #[allow(clippy::cast_precision_loss)]
            let v = m.units as f64 / 10f64.powi(i32::from(m.scale));
            Some((v, m.currency))
        }
        _ => None,
    }
}

fn text(r: &FileRecord, key: &str, field: Option<&str>) -> Option<String> {
    let v = &r.facts_for(key).next()?.value;
    let v = match field {
        Some(f) => v.field(f)?,
        None => v,
    };
    v.as_text().map(str::to_owned)
}

fn words(s: &str) -> std::collections::BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_lowercase)
        .collect()
}

/// Compares the scan's payment facts with the native ones; prints one line per fact and
/// returns the facts that did not survive.
fn compare(name: &str, native: &FileRecord, scan: &FileRecord) -> Vec<String> {
    let mut lost = Vec::new();
    for key in [
        "payment.amount",
        "payment.net",
        "payment.vat",
        "payment.gross",
    ] {
        let (a, b) = (money(native, key), money(scan, key));
        let ok = match (a, b) {
            (None, None) => true,
            (Some((x, cx)), Some((y, cy))) => (x - y).abs() <= x.abs() * 0.005 && cx == cy,
            _ => false,
        };
        eprintln!(
            "{name} {key}: native {a:?} scan {b:?} {}",
            if ok { "ok" } else { "LOST" }
        );
        if !ok {
            lost.push(key.to_owned());
        }
    }
    for (key, field) in [
        ("payment.currency", None),
        ("payment.amount_basis", None),
        ("payment.period", None),
        ("payment.category", Some("code")),
    ] {
        let (a, b) = (text(native, key, field), text(scan, key, field));
        let ok = a == b;
        eprintln!(
            "{name} {key}: native {a:?} scan {b:?} {}",
            if ok { "ok" } else { "LOST" }
        );
        if !ok {
            lost.push(key.to_owned());
        }
    }
    let (a, b) = (
        text(native, "payment.subject", Some("text")),
        text(scan, "payment.subject", Some("text")),
    );
    let ok = match (&a, &b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            let (wx, wy) = (words(x), words(y));
            #[allow(clippy::cast_precision_loss)]
            let share = wx.intersection(&wy).count() as f64 / wx.len().max(1) as f64;
            share >= 0.7
        }
        _ => false,
    };
    eprintln!(
        "{name} payment.subject: native {a:?} scan {b:?} {}",
        if ok { "ok" } else { "LOST" }
    );
    if !ok {
        lost.push("payment.subject".to_owned());
    }
    let count = |r: &FileRecord| match r.facts_for("payment.lines").next().map(|f| &f.value) {
        Some(Value::List(l)) => l.len(),
        _ => 0,
    };
    let ok = count(native) == count(scan);
    eprintln!(
        "{name} payment.lines: native {} scan {} {}",
        count(native),
        count(scan),
        if ok { "ok" } else { "LOST" }
    );
    if !ok {
        lost.push("payment.lines".to_owned());
    }
    let highlights = scan.facts_for("payment.highlight").count();
    eprintln!(
        "{name} payment.highlight: native {} scan {highlights}",
        native.facts_for("payment.highlight").count()
    );
    lost
}

fn check(name: &str, lines: &[&str]) {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let text_pdf = pdf(&[PdfPage::Text(lines)], name);
    let native_path = t.path().join(format!("{name}.pdf"));
    fs::write(&native_path, &text_pdf).unwrap();
    let scan_path = t.path().join(format!("{name}-scan.pdf"));
    fs::write(&scan_path, scanned(&text_pdf, 1, 200.0)).unwrap();
    let native = scan(&m, &native_path, false);
    let scanned = scan(&m, &scan_path, true);
    assert!(
        scanned.facts_for("ocr.pages").next().is_some(),
        "{name}: the scan was not OCR'd: {:#?}",
        scanned.steps
    );
    let lost = compare(name, &native, &scanned);
    assert!(lost.is_empty(), "{name}: lost on the scan: {lost:?}");
}

#[test]
fn serbian_contract_scan() {
    check("ugovor", PAY_SR_CONTRACT);
}

#[test]
fn english_monthly_fee_scan() {
    check("subscription", PAY_EN_MONTHLY);
}

#[test]
fn english_invoice_scan() {
    check("invoice", PAY_EN_INVOICE);
}

#[test]
fn serbian_invoice_scan() {
    check("faktura", PAY_SR_INVOICE);
}

#[test]
fn contract_without_a_price_scan() {
    check("saradnja", PAY_SR_NO_PRICE);
}

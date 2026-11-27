//! `contract.facts@1` and `card.facts@1` on scans: the generated contracts of
//! `key_facts.rs`, rendered at 200 dpi into image-only PDFs, read with `--profile deep` and
//! the real OCR backend (feature `ocr`), must give the same card as the text-layer PDFs.
//! Needs the models in `$QUAIL_OCR_MODELS`; without them every test here skips.
#![cfg(all(feature = "ocr", feature = "contract", feature = "card"))]
#![allow(clippy::many_single_char_names)]

use std::fs;
use std::path::Path;
use std::process::Command;

use quail_core::{FileRecord, Value};

mod common;
use common::{KEY_EN_CONTRACT, KEY_SR_CONTRACT, PdfPage, pdf, scanned};

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

/// The card fields as comparable text (`card.text` without the price, which
/// `payment_ocr.rs` checks).
fn fields(r: &FileRecord) -> Vec<(String, String)> {
    let Some(Value::Text(t)) = r.facts_for("card.text").next().map(|f| &f.value) else {
        return Vec::new();
    };
    t.lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Compares the scan's card with the native one, field by field; returns the fields that
/// did not survive.
fn check(name: &str, lines: &[&str]) -> Vec<String> {
    let Some(m) = models() else { return Vec::new() };
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
        "{name}: the scan was not OCR'd"
    );
    let (a, b) = (fields(&native), fields(&scanned));
    let mut lost = Vec::new();
    for (k, v) in &a {
        let got = b.iter().find(|(x, _)| x == k).map(|(_, y)| y.as_str());
        let ok = got == Some(v.as_str());
        eprintln!(
            "{name} card.{k}: native {v:?} scan {got:?} {}",
            if ok { "ok" } else { "LOST" }
        );
        if !ok {
            lost.push(k.clone());
        }
    }
    for (k, v) in &b {
        if !a.iter().any(|(x, _)| x == k) {
            eprintln!("{name} card.{k}: native None scan {v:?} EXTRA");
            lost.push(format!("+{k}"));
        }
    }
    // The `contract.*` facts themselves, by key.
    let keys = |r: &FileRecord| {
        let mut k: Vec<String> = r
            .facts
            .iter()
            .filter(|f| f.key.namespace() == "contract" && !f.key.as_str().ends_with("highlight"))
            .map(|f| f.key.as_str().to_owned())
            .collect();
        k.sort();
        k
    };
    let (ka, kb) = (keys(&native), keys(&scanned));
    for k in &ka {
        if ka.iter().filter(|x| *x == k).count() != kb.iter().filter(|x| *x == k).count() {
            eprintln!(
                "{name} {k}: native {} scan {}",
                ka.iter().filter(|x| *x == k).count(),
                kb.iter().filter(|x| *x == k).count()
            );
        }
    }
    let hl = |r: &FileRecord, k: &str| r.facts_for(k).count();
    eprintln!(
        "{name} contract.highlight: native {} scan {}; card.highlight: native {} scan {}",
        hl(&native, "contract.highlight"),
        hl(&scanned, "contract.highlight"),
        hl(&native, "card.highlight"),
        hl(&scanned, "card.highlight"),
    );
    lost
}

#[test]
fn serbian_contract_scan() {
    let lost = check("ugovor", KEY_SR_CONTRACT);
    assert!(lost.is_empty(), "lost on the scan: {lost:?}");
}

#[test]
fn english_contract_scan() {
    let lost = check("agreement", KEY_EN_CONTRACT);
    assert!(lost.is_empty(), "lost on the scan: {lost:?}");
}

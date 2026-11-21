//! The PDF extractor wired into `quail scan`: pdf.* facts, text-level analyzers on the
//! extracted text, and `sketch.page_phash` on scanned pages only.
#![cfg(feature = "pdf")]

use std::fs;
use std::process::Command;

use quail_core::{FileRecord, Format, Status, Value};

mod common;
use common::{CONTRACT, PdfPage, pdf};

fn scan(dir: &std::path::Path) -> Vec<FileRecord> {
    let o = Command::new(env!("CARGO_BIN_EXE_quail"))
        .args(["scan", dir.to_str().unwrap(), "--sorted", "--ordered", "-q"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn keys(r: &FileRecord) -> Vec<&str> {
    r.facts.iter().map(|f| f.key.as_str()).collect()
}

fn ran(r: &FileRecord, step: &str) -> bool {
    r.steps.iter().any(|s| s.name == step)
}

#[test]
fn pdf_gets_pdf_text_signal_and_ref_facts() {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("contract.pdf"),
        pdf(&[PdfPage::Text(CONTRACT)], "Supply Agreement"),
    )
    .unwrap();
    let recs = scan(t.path());
    let r = &recs[0];
    assert_eq!(r.format, Format::Pdf);
    assert_eq!(r.status, Status::Ok, "{:?}", r.warnings);
    assert_eq!(r.pages, Some(1));
    assert!(ran(r, "pdf.extract"), "{:?}", r.steps);
    let k = keys(r);
    for key in ["pdf.version", "pdf.page_count", "pdf.producer", "pdf.title"] {
        assert!(k.contains(&key), "{key} missing in {k:?}");
    }
    assert_eq!(
        r.facts_for("pdf.title").next().unwrap().value,
        Value::text("Supply Agreement")
    );
    if cfg!(feature = "text") {
        assert!(
            k.contains(&"text.words") && k.contains(&"text.lang"),
            "{k:?}"
        );
        let lang = r.facts_for("text.lang").next().unwrap();
        assert_eq!(lang.value, Value::text("en"));
    }
    if cfg!(feature = "signals") {
        let iban = r.facts_for("signal.iban").next().expect("signal.iban");
        assert_eq!(iban.value, Value::text("DE89370400440532013000"));
    }
    if cfg!(feature = "refs") {
        assert!(k.iter().any(|k| k.starts_with("ref.")), "{k:?}");
    }
    if cfg!(feature = "sketch") {
        assert!(k.contains(&"sketch.minhash"), "{k:?}");
        // A text page is not a scan: no pHash step, no pHash fact.
        assert!(!ran(r, "sketch.page_phash"), "{:?}", r.steps);
        assert!(!k.contains(&"sketch.page_phash"));
    }
}

#[test]
#[cfg(feature = "sketch")]
fn page_phash_hashes_only_scanned_pages() {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("mixed.pdf"),
        pdf(
            &[
                PdfPage::Text(CONTRACT),
                PdfPage::Scan,
                PdfPage::Text(CONTRACT),
            ],
            "Mixed",
        ),
    )
    .unwrap();
    fs::write(t.path().join("scan.pdf"), pdf(&[PdfPage::Scan], "Scan")).unwrap();
    let recs = scan(t.path());
    let (mixed, scan) = (&recs[0], &recs[1]);
    assert!(mixed.locator.ends_with("mixed.pdf") && scan.locator.ends_with("scan.pdf"));
    for r in [mixed, scan] {
        assert!(ran(r, "sketch.page_phash"), "{}: {:?}", r.locator, r.steps);
        assert!(r.facts_for("pdf.pages_needing_ocr").next().is_some());
    }
    let pages = |r: &FileRecord| {
        r.facts_for("sketch.page_phash_pages")
            .next()
            .map(|f| f.value.clone())
    };
    assert_eq!(pages(mixed), Some(Value::List(vec![Value::Int(1)])));
    assert_eq!(pages(scan), Some(Value::List(vec![Value::Int(0)])));
    let hash = |r: &FileRecord| {
        r.facts_for("sketch.page_phash")
            .next()
            .unwrap()
            .value
            .clone()
    };
    // The same image on both: the same hash.
    assert_eq!(hash(mixed), hash(scan));
}

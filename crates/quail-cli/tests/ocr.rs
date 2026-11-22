//! `--profile deep` with the real OCR backend (feature `ocr`): the budgets the CLI derives and
//! the problems a file gets past them. Needs the models in `$QUAIL_OCR_MODELS` (CI fetches and
//! caches them); without them every test here skips.
#![cfg(feature = "ocr")]
#![allow(clippy::many_single_char_names)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use quail_core::{FileRecord, Status, Value};

mod common;
use common::{PdfPage, pdf};

/// The model directory when every model file is there.
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

fn quail(models: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quail"))
        .env("QUAIL_OCR_MODELS", models)
        .args(args)
        .output()
        .unwrap()
}

fn records(o: &Output) -> Vec<FileRecord> {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn three_scans(dir: &Path) -> String {
    let p = dir.join("scans.pdf");
    fs::write(
        &p,
        pdf(&[PdfPage::Scan, PdfPage::Scan, PdfPage::Scan], "Scans"),
    )
    .unwrap();
    p.to_str().unwrap().to_owned()
}

fn int(r: &FileRecord, key: &str) -> Option<i64> {
    r.facts_for(key).next().and_then(|f| match f.value {
        Value::Int(i) => Some(i),
        _ => None,
    })
}

fn problems(r: &FileRecord) -> Vec<String> {
    match &r.status {
        Status::Partial { problems } => problems.clone(),
        _ => Vec::new(),
    }
}

#[test]
fn deep_scan_reads_every_page_with_derived_budgets() {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let file = three_scans(t.path());
    let o = quail(&m, &["scan", &file, "--profile", "deep", "-j", "3"]);
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    // The note says how OCR runs and the raised per-file timeout.
    assert!(stderr.contains("quail: ocr: "), "{stderr}");
    assert!(stderr.contains("per page; timeout "), "{stderr}");
    let recs = records(&o);
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert!(
        r.steps.iter().any(|s| s.name == "ocr.pages"),
        "{:?}",
        r.steps
    );
    assert_eq!(r.status, Status::Ok, "{:?}", r.status);
    assert_eq!(int(r, "ocr.pages"), Some(3));
}

#[test]
fn pages_past_the_budgets_make_the_record_partial() {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let file = three_scans(t.path());
    let o = quail(
        &m,
        &[
            "scan",
            &file,
            "--profile",
            "deep",
            "-q",
            "--ocr-max-pages",
            "1",
        ],
    );
    let r = &records(&o)[0];
    assert_eq!(int(r, "ocr.pages"), Some(1));
    assert!(
        problems(r).iter().any(|p| p.contains("max_pages")),
        "{:?}",
        r.status
    );

    let o = quail(
        &m,
        &[
            "scan",
            &file,
            "--profile",
            "deep",
            "-q",
            "--ocr-page-time",
            "1ms",
        ],
    );
    let r = &records(&o)[0];
    assert!(int(r, "ocr.pages").is_some_and(|n| n < 3), "{:?}", r.facts);
    assert!(
        problems(r).iter().any(|p| p.contains("page_time")),
        "{:?}",
        r.status
    );
}

#[test]
fn explicit_jobs_threads_and_timeout_are_kept() {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let file = three_scans(t.path());
    let o = quail(
        &m,
        &[
            "scan",
            &file,
            "--profile",
            "deep",
            "--ocr-jobs",
            "1",
            "--ocr-threads",
            "2",
            "--timeout",
            "10m",
        ],
    );
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        stderr.contains("1 page(s) at a time, 2 thread(s) each"),
        "{stderr}"
    );
    assert!(
        stderr.contains("no per-page budget; timeout 10m"),
        "{stderr}"
    );
    assert_eq!(int(&records(&o)[0], "ocr.pages"), Some(3));
}

/// A scan longer than the file timeout allows: OCR stops early enough that the text
/// analyzers still run on the pages read, instead of the engine skipping them at the deadline.
#[test]
fn ocr_leaves_time_for_the_analyzers_after_it() {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("long.pdf");
    let pages: Vec<PdfPage<'_>> = (0..40).map(|_| PdfPage::Scan).collect();
    fs::write(&p, pdf(&pages, "Long scan")).unwrap();
    let file = p.to_str().unwrap();
    let o = quail(
        &m,
        &[
            "scan",
            file,
            "--profile",
            "deep",
            "-q",
            "--ocr-jobs",
            "1",
            "--ocr-threads",
            "1",
            "--timeout",
            "20s",
        ],
    );
    let r = &records(&o)[0];
    let probs = problems(r);
    assert!(
        !probs.iter().any(|p| p.contains("analyzer(s) not run")),
        "{probs:?}"
    );
    for step in ["ocr.pages", "signal.scan", "summary.card"] {
        assert!(
            r.steps.iter().any(|s| s.name == step),
            "{step}: {:?}",
            r.steps
        );
    }
    if int(r, "ocr.pages") != Some(40) {
        assert!(
            probs.iter().any(|p| p.contains("steps after it")),
            "{probs:?}"
        );
    }
}

/// `inspect` gets the same derived budgets as `scan` (one OCR job, raised timeout).
#[test]
fn inspect_gets_the_auto_budget() {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let file = three_scans(t.path());
    let o = quail(&m, &["inspect", &file, "--profile", "deep"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        stderr.contains("quail: ocr: 1 page(s) at a time"),
        "{stderr}"
    );
    assert!(
        stderr.contains("45s per page; timeout 32m per file"),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(stdout.contains("ocr.pages"), "{stdout}");
}

/// `--isolate`: one engine per worker process, and `-j` defaults to the OCR jobs.
#[test]
fn isolated_deep_scan_reads_pages() {
    let Some(m) = models() else { return };
    let t = tempfile::tempdir().unwrap();
    let file = three_scans(t.path());
    let o = quail(
        &m,
        &[
            "scan",
            &file,
            "--profile",
            "deep",
            "--isolate",
            "--worker-mem",
            "4G",
        ],
    );
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(stderr.contains("1 page(s) at a time"), "{stderr}");
    let r = &records(&o)[0];
    assert_eq!(r.status, Status::Ok, "{:?}", r.status);
    assert_eq!(int(r, "ocr.pages"), Some(3));
}

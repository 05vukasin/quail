//! `quail scan --isolate`: the same records as an in-process run, and a worker crash costs
//! one `Failed` record.
#![allow(clippy::similar_names)]

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use quail_core::{FileRecord, Status};
use serde_json::Value as Json;

mod common;
use common::{CONTRACT, PdfPage, docx, pdf};

fn quail() -> Command {
    Command::new(env!("CARGO_BIN_EXE_quail"))
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn write(dir: &Path, rel: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, data).unwrap();
    p
}

fn records(jsonl: &str) -> Vec<FileRecord> {
    jsonl
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad line {l}: {e}")))
        .collect()
}

fn json_stats(o: &Output) -> Json {
    let line = stderr(o)
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .map_or_else(|| panic!("no stats in {}", stderr(o)), str::to_owned);
    serde_json::from_str(&line).unwrap()
}

/// Every kind of input: DOCX, text and scanned PDFs, plain text, junk, a nested file, an
/// empty file and one over the size cap.
fn corpus() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    write(
        d,
        "in/a.docx",
        &docx("Service Agreement", "The parties agree."),
    );
    write(d, "in/b.txt", b"plain text\n");
    write(d, "in/c.bin", &[0u8, 159, 146, 150, 0, 1, 2, 255]);
    write(
        d,
        "in/contract.pdf",
        &pdf(&[PdfPage::Text(CONTRACT)], "Supply"),
    );
    write(
        d,
        "in/mixed.pdf",
        &pdf(&[PdfPage::Text(CONTRACT), PdfPage::Scan], "Mixed"),
    );
    write(d, "in/sub/d.txt", b"nested\n");
    write(d, "in/empty.txt", b"");
    write(d, "in/big.bin", &vec![7u8; 100_000]);
    t
}

/// One scan of `<dir>/in` into `<dir>/<tag>.jsonl` (+ document dump); returns the records
/// and the JSON stats.
fn scan(dir: &Path, tag: &str, extra: &[&str]) -> (Vec<FileRecord>, Json, PathBuf) {
    let out = dir.join(format!("{tag}.jsonl"));
    let dump = dir.join(format!("{tag}.docs.jsonl"));
    let input = dir.join("in");
    let mut args = vec![
        "scan",
        input.to_str().unwrap(),
        "--sorted",
        "--ordered",
        "-j",
        "2",
        "--max-bytes",
        "64K",
        "-o",
        out.to_str().unwrap(),
        "--dump-documents",
        dump.to_str().unwrap(),
        "--json-stats",
    ];
    args.extend_from_slice(extra);
    let o = quail().args(&args).output().unwrap();
    assert!(o.status.success(), "{tag}: {}", stderr(&o));
    (
        records(&fs::read_to_string(&out).unwrap()),
        json_stats(&o),
        dump,
    )
}

fn without_timings(mut recs: Vec<FileRecord>) -> Vec<FileRecord> {
    for r in &mut recs {
        r.timings.clear();
    }
    recs
}

fn lines(p: &Path) -> BTreeSet<String> {
    fs::read_to_string(p)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn isolated_run_equals_in_process_run() {
    let t = corpus();
    let (local, ls, ldump) = scan(t.path(), "local", &[]);
    let (iso, is, idump) = scan(t.path(), "iso", &["--isolate"]);
    assert_eq!(local.len(), 8);
    assert_eq!(without_timings(iso), without_timings(local.clone()));
    // Every count matches; the isolated run adds its worker stats.
    for k in [
        "files",
        "ok",
        "partial",
        "failed",
        "source_error_records",
        "bytes",
    ] {
        assert_eq!(is[k], ls[k], "{k}");
    }
    assert!(ls.get("isolation").is_none());
    assert_eq!(is["isolation"]["crashes"], 0, "{is}");
    assert_eq!(is["isolation"]["poisoned"], 0);
    // The documents come back from the workers: the same dump.
    assert_eq!(lines(&idump), lines(&ldump));
    let extracted = usize::from(cfg!(feature = "docx")) + 2 * usize::from(cfg!(feature = "pdf"));
    assert_eq!(lines(&idump).len(), extracted, "docx + 2 pdf dumped");
    // The oversized file failed in the supervisor, before any worker read it.
    let big = local
        .iter()
        .find(|r| r.locator.ends_with("big.bin"))
        .unwrap();
    let Status::Failed { error } = &big.status else {
        panic!("{:?}", big.status)
    };
    assert!(error.contains("source cap of 65536"), "{error}");
}

#[test]
fn isolated_manifest_keeps_locators_and_fails_missing_files_by_locator() {
    let t = corpus();
    let manifest = "# split\nid-a\tin/a.docx\nin/b.txt\nid-gone\tnope/missing.pdf\n";
    let m = write(t.path(), "split.tsv", manifest.as_bytes());
    let run = |extra: &[&str]| {
        let mut args = vec!["scan", "--manifest", m.to_str().unwrap(), "--ordered", "-q"];
        args.extend_from_slice(extra);
        let o = quail().args(&args).output().unwrap();
        assert!(o.status.success(), "{}", stderr(&o));
        without_timings(records(&String::from_utf8(o.stdout).unwrap()))
    };
    let iso = run(&["--isolate"]);
    let locs: Vec<_> = iso.iter().map(|r| r.locator.as_str()).collect();
    assert_eq!(locs, ["id-a", "in/b.txt", "id-gone"]);
    assert_eq!(iso, run(&[]));
}

#[test]
fn isolated_broken_pipe_exits_quietly() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..1500 {
        write(
            t.path(),
            &format!("f{i}.txt"),
            format!("file {i}\n").as_bytes(),
        );
    }
    let mut child = quail()
        .args(["scan", t.path().to_str().unwrap(), "-j", "2", "--isolate"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(first.starts_with('{'), "{first}");
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "no exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(status.success(), "{status:?} {err}");
    assert!(err.is_empty(), "stderr not quiet: {err}");
}

#[test]
fn isolate_options_need_isolate() {
    let o = quail()
        .args(["scan", ".", "--worker-mem", "2G"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--isolate"), "{}", stderr(&o));
}

/// A worker that aborts on one file (test hook) costs that file a `Failed` record; the
/// others succeed, the crash is retried once and counted.
#[test]
#[cfg(feature = "test-hooks")]
fn worker_crash_fails_one_file_and_the_run_goes_on() {
    let t = corpus();
    write(t.path(), "in/boom-1.txt", b"this one crashes\n");
    let run = |extra: &[&str]| {
        let input = t.path().join("in");
        let mut args = vec![
            "scan",
            input.to_str().unwrap(),
            "--sorted",
            "--ordered",
            "-j",
            "2",
            "--isolate",
            "--json-stats",
        ];
        args.extend_from_slice(extra);
        let o = quail()
            .args(&args)
            .env("QUAIL_TEST_CRASH_ON", "boom-")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", stderr(&o));
        (
            records(&String::from_utf8(o.stdout.clone()).unwrap()),
            json_stats(&o),
        )
    };

    let (recs, s) = run(&[]);
    assert_eq!(recs.len(), 9);
    for r in &recs {
        if r.locator.ends_with("boom-1.txt") {
            let Status::Failed { error } = &r.status else {
                panic!("{:?}", r.status)
            };
            assert!(error.contains("worker crashed"), "{error}");
            assert!(error.contains("abort"), "{error}");
            assert_eq!(r.size, 17);
        } else {
            assert!(!matches!(r.status, Status::Failed { .. }), "{r:?}");
        }
    }
    let i = &s["isolation"];
    assert_eq!(i["crashes"], 2, "{s}");
    assert_eq!(i["crash_kinds"]["abort"], 2);
    assert_eq!(
        (i["retries"].as_u64(), i["poisoned"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(s["files"], 9);

    let (recs, s) = run(&["--no-retry"]);
    assert_eq!(recs.len(), 9);
    assert_eq!(s["isolation"]["crashes"], 1, "{s}");
    assert_eq!(s["isolation"]["retries"], 0);
    assert_eq!(s["failed"], 1);
}

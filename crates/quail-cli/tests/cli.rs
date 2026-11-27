//! End-to-end tests: run the `quail` binary over files generated in a temp dir.

#![allow(clippy::many_single_char_names)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(feature = "docx")]
use quail_core::Value;
use quail_core::{FileRecord, Format, Status};
#[cfg(feature = "docx")]
use quail_eval::inspect::{DocumentSource, Sidecar};
use serde_json::Value as Json;

mod common;
use common::docx;

fn quail() -> Command {
    Command::new(env!("CARGO_BIN_EXE_quail"))
}

fn run(args: &[&str]) -> Output {
    quail().args(args).output().expect("run quail")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).expect("utf-8 stdout")
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

/// A docx, a text file, a junk file and a hidden text file.
fn corpus() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "a.docx",
        &docx("Service Agreement", "The parties agree."),
    );
    write(t.path(), "b.txt", b"plain text\n");
    write(t.path(), "c.bin", &[0u8, 159, 146, 150, 0, 1, 2, 255]);
    write(t.path(), "sub/.hidden.txt", b"hidden\n");
    t
}

fn records(jsonl: &str) -> Vec<FileRecord> {
    jsonl
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad line {l}: {e}")))
        .collect()
}

fn name(r: &FileRecord) -> &str {
    Path::new(&r.locator).file_name().unwrap().to_str().unwrap()
}

#[cfg(feature = "docx")]
fn blake3_hex(r: &FileRecord) -> String {
    match &r.facts_for("hash.blake3").next().expect("blake3").value {
        Value::Bytes(b) => quail_cli::dump::hex(b),
        v => panic!("{v:?}"),
    }
}

fn json_stats(o: &Output) -> Json {
    let line = stderr(o)
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .map_or_else(|| panic!("no stats in {}", stderr(o)), str::to_owned);
    serde_json::from_str(&line).unwrap()
}

#[test]
#[cfg(feature = "docx")]
fn scan_writes_one_valid_record_per_file_and_stats() {
    let t = corpus();
    let out = t.path().join("out.jsonl");
    let root = t.path().to_str().unwrap();
    let o = run(&[
        "scan",
        root,
        "--sorted",
        "--exclude",
        "out.jsonl",
        "-j",
        "2",
        "-o",
        out.to_str().unwrap(),
        "--json-stats",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(o.stdout.is_empty(), "records went to the file");
    let recs = records(&fs::read_to_string(&out).unwrap());
    let mut names: Vec<_> = recs.iter().map(name).collect();
    names.sort_unstable();
    assert_eq!(names, [".hidden.txt", "a.docx", "b.txt", "c.bin"]);
    for r in &recs {
        assert_eq!(r.status, Status::Ok, "{}", r.locator);
        assert_eq!(blake3_hex(r).len(), 64);
    }
    let d = recs.iter().find(|r| name(r) == "a.docx").unwrap();
    assert_eq!(d.format, Format::Docx);
    assert_eq!(d.pages, Some(1));
    assert!(
        d.facts_for("docx.title")
            .any(|f| f.value == Value::text("Service Agreement"))
    );
    assert!(d.steps.iter().any(|s| s.name == "docx.extract"));
    let junk = recs.iter().find(|r| name(r) == "c.bin").unwrap();
    assert_eq!(junk.format, Format::Unknown);

    let s = json_stats(&o);
    assert_eq!(s["files"], 4);
    assert_eq!(s["ok"], 4);
    assert_eq!(s["failed"], 0);
    assert_eq!(s["threads"], 2);
    assert!(s["files_per_sec"].as_f64().unwrap() > 0.0);
}

#[test]
fn filters_and_human_summary() {
    let t = corpus();
    let o = run(&[
        "scan",
        t.path().to_str().unwrap(),
        "--skip-hidden",
        "--ext",
        "docx,TXT",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&stdout(&o));
    let mut names: Vec<_> = recs.iter().map(name).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.docx", "b.txt"]);
    let e = stderr(&o);
    assert!(e.contains("2 files (2 ok, 0 partial, 0 failed)"), "{e}");
    assert!(e.contains("files/s"), "{e}");

    let q = run(&["scan", t.path().to_str().unwrap(), "--quiet"]);
    assert!(q.status.success());
    assert!(q.stderr.is_empty(), "{}", stderr(&q));
}

#[test]
fn dedup_reuses_records_of_identical_files() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "a.txt", b"the same agreement text\n");
    write(t.path(), "b.txt", b"another text\n");
    write(t.path(), "c/a-copy.txt", b"the same agreement text\n");
    let root = t.path().to_str().unwrap();
    let o = run(&[
        "scan",
        root,
        "--sorted",
        "--ordered",
        "-j",
        "1",
        "--dedup",
        "--json-stats",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&stdout(&o));
    assert_eq!(recs.len(), 3);
    let first = recs.iter().find(|r| name(r) == "a.txt").unwrap();
    let copy = recs.iter().find(|r| name(r) == "a-copy.txt").unwrap();
    let of = copy
        .facts_for("engine.duplicate_of")
        .next()
        .expect("duplicate_of");
    assert_eq!(of.value, Value::text(first.locator.as_str()));
    assert_eq!(copy.facts.len(), first.facts.len() + 1);
    assert!(copy.steps.iter().any(|s| s.name == "engine.dedup"));
    assert_eq!(copy.timings.len(), 1);
    assert_eq!(first.facts_for("engine.duplicate_of").count(), 0);
    assert_eq!(json_stats(&o)["duplicates"], 1);

    // Off without the flag.
    let o = run(&["scan", root, "--sorted", "--json-stats"]);
    assert!(
        records(&stdout(&o))
            .iter()
            .all(|r| r.facts_for("engine.duplicate_of").count() == 0)
    );
    assert_eq!(json_stats(&o)["duplicates"], 0);
}

#[test]
fn ordered_mode_keeps_source_order() {
    let t = tempfile::tempdir().unwrap();
    let mut expected = Vec::new();
    for i in 0..300 {
        let rel = format!("f{i:04}.txt");
        // Uneven sizes so workers finish out of order.
        let body = "x".repeat(1 + (i * 7919) % 20_000);
        expected.push(
            write(t.path(), &rel, body.as_bytes())
                .to_string_lossy()
                .into_owned(),
        );
    }
    let o = run(&[
        "scan",
        t.path().to_str().unwrap(),
        "--sorted",
        "--ordered",
        "-j",
        "4",
        "--max-in-flight",
        "16",
        "-q",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let got: Vec<String> = records(&stdout(&o))
        .into_iter()
        .map(|r| r.locator)
        .collect();
    assert_eq!(got, expected);
}

#[test]
fn manifest_mode_keeps_locators_and_fails_missing_files_by_locator() {
    let t = corpus();
    let manifest = "# split\nid-a\ta.docx\nb.txt\nid-gone\tnope/missing.pdf\n";
    write(t.path(), "split.tsv", manifest.as_bytes());
    let m = t.path().join("split.tsv");
    let o = run(&["scan", "--manifest", m.to_str().unwrap(), "--ordered", "-q"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&stdout(&o));
    let locs: Vec<_> = recs.iter().map(|r| r.locator.as_str()).collect();
    assert_eq!(locs, ["id-a", "b.txt", "id-gone"]);
    assert_eq!(recs[0].format, Format::Docx);
    let Status::Failed { error } = &recs[2].status else {
        panic!("{:?}", recs[2].status)
    };
    assert!(error.contains("nope/missing.pdf"), "{error}");

    // The same manifest from stdin; relative paths resolve against the working directory.
    let mut child = quail()
        .args(["scan", "--manifest", "-", "--ordered", "--json-stats"])
        .current_dir(t.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(manifest.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(records(&stdout(&o)).len(), 3);
    let s = json_stats(&o);
    assert_eq!((s["ok"].as_u64(), s["failed"].as_u64()), (Some(2), Some(1)));
    assert_eq!(s["source_error_records"], 1);
    assert!(
        s["top_problems"][0]["problem"]
            .as_str()
            .unwrap()
            .contains("<file>"),
        "{s}"
    );
}

#[test]
#[cfg(feature = "docx")]
fn dump_documents_writes_sidecar_lines() {
    let t = corpus();
    write(t.path(), "d2.docx", &docx("Second", "More text."));
    let dump = t.path().join("docs.jsonl");
    let out = t.path().join("recs.jsonl");
    let o = run(&[
        "scan",
        t.path().to_str().unwrap(),
        "--ext",
        "docx,txt,bin",
        "-o",
        out.to_str().unwrap(),
        "--dump-documents",
        dump.to_str().unwrap(),
        "-q",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&fs::read_to_string(&out).unwrap());
    let lines: Vec<Json> = fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // Only extracted documents are dumped: the two DOCX files, and with the text extractor
    // the two text files (not the junk).
    assert_eq!(lines.len(), if cfg!(feature = "text") { 4 } else { 2 });
    for l in &lines {
        let obj = l.as_object().unwrap();
        let mut keys: Vec<_> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["blake3", "document", "locator"]);
        let rec = recs
            .iter()
            .find(|r| r.locator == l["locator"].as_str().unwrap())
            .unwrap();
        assert_eq!(l["blake3"].as_str().unwrap(), blake3_hex(rec));
        // The document is quail-core's serde form.
        let doc: quail_core::Document = serde_json::from_value(l["document"].clone()).unwrap();
        assert_eq!(doc.pages.len(), 1);
        let t = &doc.pages[0].text;
        assert!(t.contains("text") || t.contains("agree") || t.contains("hidden"));
        assert_eq!(rec.status, Status::Ok);
    }
    // The dump leaves the records untouched: no extra step.
    for r in &recs {
        assert!(
            r.steps.iter().all(|s| !s.name.starts_with("cli.")),
            "{:?}",
            r.steps
        );
    }

    // quail-eval reads the dump back and finds each record's document.
    let sidecar = Sidecar::load(&dump, None).unwrap();
    for r in &recs {
        let doc = sidecar.document(r).unwrap();
        let extracted =
            r.format == Format::Docx || (cfg!(feature = "text") && r.format == Format::Text);
        assert_eq!(doc.is_some(), extracted, "{}", r.locator);
    }
}

#[test]
fn files_over_max_bytes_are_not_read_and_fail_by_locator() {
    let t = tempfile::tempdir().unwrap();
    let big = write(t.path(), "big.bin", &vec![7u8; 4096]);
    write(t.path(), "small.txt", b"small\n");
    let o = run(&[
        "scan",
        t.path().to_str().unwrap(),
        "--max-bytes",
        "1K",
        "--json-stats",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&stdout(&o));
    let b = recs.iter().find(|r| name(r) == "big.bin").unwrap();
    assert_eq!(b.locator, big.to_string_lossy());
    let Status::Failed { error } = &b.status else {
        panic!("{:?}", b.status)
    };
    assert!(error.contains("source cap of 1024"), "{error}");
    assert_eq!(json_stats(&o)["failed"], 1);
}

#[cfg(unix)]
#[test]
fn unreadable_file_gets_a_failed_record_with_its_path() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let p = write(t.path(), "locked.txt", b"secret");
    write(t.path(), "open.txt", b"open");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&p).is_ok() {
        eprintln!("privileges ignore file modes; skipping");
        return;
    }
    let o = run(&["scan", t.path().to_str().unwrap(), "-q"]);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&stdout(&o));
    assert_eq!(recs.len(), 2);
    let l = recs.iter().find(|r| name(r) == "locked.txt").unwrap();
    assert!(matches!(l.status, Status::Failed { .. }), "{:?}", l.status);
}

/// Waits for a child at most `limit`, killing it after that.
fn wait_timeout(child: &mut Child, limit: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            panic!("quail did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn broken_pipe_exits_quietly_and_promptly() {
    let t = tempfile::tempdir().unwrap();
    // Enough output to overflow the 256 KiB sink buffer many times.
    for i in 0..4000 {
        write(
            t.path(),
            &format!("d{}/f{i}.txt", i % 10),
            format!("file {i}\n").as_bytes(),
        );
    }
    let mut child = quail()
        .args(["scan", t.path().to_str().unwrap(), "-j", "2"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(first.starts_with('{'), "{first}");
    // Reader dropped: like `quail scan | head -1`.
    let status = wait_timeout(&mut child, Duration::from_secs(20));
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

/// Blocks until `pid` catches SIGINT (Linux: bit 2 of `SigCgt` in `/proc/<pid>/status`), so a
/// signal sent next reaches the handler instead of killing the process. Where there is no
/// `/proc`, waits half a second instead.
#[cfg(unix)]
fn wait_for_sigint_handler(pid: u32) {
    let path = format!("/proc/{pid}/status");
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(30) {
        let Some(mask) = fs::read_to_string(&path).ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("SigCgt:"))
                .and_then(|m| u64::from_str_radix(m.trim(), 16).ok())
        }) else {
            std::thread::sleep(Duration::from_millis(500));
            return;
        };
        if mask & (1 << (2 - 1)) != 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("quail did not install its Ctrl-C handler within 30 s");
}

#[cfg(unix)]
#[test]
fn ctrl_c_drains_prints_stats_and_exits_130() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..4 {
        write(t.path(), &format!("f{i}.txt"), b"x");
    }
    let mut child = quail()
        .args([
            "scan",
            "--manifest",
            "-",
            "--ordered",
            "-j",
            "1",
            "--max-in-flight",
            "1",
        ])
        .current_dir(t.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    // Stderr lines, as they come, so the test can wait for the handler's message.
    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    let err_reader = BufReader::new(child.stderr.take().unwrap());
    let err_thread = std::thread::spawn(move || {
        let mut all = String::new();
        for line in err_reader.lines().map_while(Result::ok) {
            all.push_str(&line);
            all.push('\n');
            let _ = line_tx.send(line);
        }
        all
    });
    stdin.write_all(b"f0.txt\nf1.txt\n").unwrap();
    stdin.flush().unwrap();
    // Ready once the handler is in (it goes in before anything slow): no sleep guesses.
    wait_for_sigint_handler(child.id());
    let pid = child.id().to_string();
    let killed = Command::new("kill").args(["-INT", &pid]).status().unwrap();
    assert!(killed.success());
    // The handler says so on stderr once the stop is set.
    let t0 = Instant::now();
    loop {
        let left = Duration::from_secs(30).saturating_sub(t0.elapsed());
        match line_rx.recv_timeout(left) {
            Ok(l) if l.contains("interrupted") => break,
            Ok(_) => {}
            Err(e) => panic!("no interrupt message: {e}"),
        }
    }
    // The source is blocked reading stdin: one more line lets it notice the stop.
    let _ = stdin.write_all(b"f2.txt\n");
    let _ = stdin.flush();
    let status = wait_timeout(&mut child, Duration::from_secs(30));
    drop(stdin);
    let mut recs = String::new();
    out.read_to_string(&mut recs).unwrap();
    let err = err_thread.join().unwrap();
    assert_eq!(status.code(), Some(130), "{err}");
    assert!(err.contains("interrupted"), "{err}");
    assert!(err.contains("cancelled"), "{err}");
    // Records of files already pulled are written in full. The interrupt can land while the
    // engine is still being built (language models, indexes): then nothing was pulled yet,
    // which is a clean cancel too.
    let n = records(&recs).len();
    assert!(n <= 3, "{n} records");
}

#[test]
#[cfg(feature = "docx")]
fn inspect_pretty_prints_facts_and_text() {
    let t = corpus();
    let a = t.path().join("a.docx");
    let o = run(&["inspect", a.to_str().unwrap(), "--text"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.contains("status   ok"), "{s}");
    assert!(
        s.contains("[docx]") && s.contains("[hash]") && s.contains("[engine]"),
        "{s}"
    );
    assert!(
        s.contains("docx.title") && s.contains("\"Service Agreement\""),
        "{s}"
    );
    assert!(s.contains("src docx.extract@2"), "{s}");
    assert!(s.contains("--- page 1"), "{s}");
    assert!(
        s.contains("/h1] Service Agreement") || s.contains("Service Agreement"),
        "{s}"
    );
    assert!(s.contains("The parties agree."), "{s}");
    assert!(
        !s.contains("cli.capture"),
        "the capture step is hidden: {s}"
    );

    let j = run(&["inspect", a.to_str().unwrap(), "--json", "--text"]);
    let v: Json = serde_json::from_slice(&j.stdout).unwrap();
    let rec: FileRecord = serde_json::from_value(v["record"].clone()).unwrap();
    assert_eq!(rec.format, Format::Docx);
    assert_eq!(v["document"]["pages"].as_array().unwrap().len(), 1);

    let missing = run(&["inspect", t.path().join("nope").to_str().unwrap()]);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("quail: error:"));
}

#[test]
#[cfg(feature = "docx")]
fn keys_and_version() {
    let o = run(&["keys"]);
    assert!(o.status.success());
    let s = stdout(&o);
    for k in [
        "engine.format",
        "hash.blake3",
        "docx.title",
        "docx.extract@2",
        "hash.identity@1",
    ] {
        assert!(s.contains(k), "{k} missing");
    }
    let j: Json = serde_json::from_slice(&run(&["keys", "docx.", "--json"]).stdout).unwrap();
    let keys = j["keys"].as_array().unwrap();
    assert!(keys.len() > 40);
    assert!(
        keys.iter()
            .all(|k| k["key"].as_str().unwrap().starts_with("docx."))
    );
    assert!(keys.iter().all(|k| !k["doc"].as_str().unwrap().is_empty()));

    let v: Json = serde_json::from_slice(&run(&["version", "--json"]).stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["fingerprint"].as_str().unwrap().len(), 16);
    let ids: Vec<_> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    // Extractors in format order (PDF, DOCX, PPTX, HTML, text), then the analyzers in
    // registration order.
    let mut expected = vec!["engine.sniff@3"];
    if cfg!(feature = "pdf") {
        expected.push("pdf.extract@4");
    }
    expected.push("docx.extract@2");
    if cfg!(feature = "pptx") {
        expected.push("pptx.extract@1");
    }
    if cfg!(feature = "html") {
        expected.push("html.extract@1");
    }
    if cfg!(feature = "text") {
        expected.push("text.extract@1");
    }
    expected.push("hash.identity@1");
    if cfg!(feature = "text") {
        expected.extend(["text.stats@3", "text.lang@2"]);
    }
    if cfg!(feature = "signals") {
        expected.push("signal.scan@5");
    }
    if cfg!(feature = "refs") {
        expected.push("ref.references@3");
    }
    if cfg!(feature = "parties") {
        expected.push("party.candidates@6");
    }
    if cfg!(feature = "clauses") {
        expected.push("clause.terms@1");
    }
    if cfg!(feature = "sketch") {
        expected.extend([
            "sketch.minhash@2",
            "sketch.simhash_text@2",
            "sketch.simhash_template@2",
            "sketch.layout@2",
            "sketch.invoice_layout@2",
            "sketch.text_key@2",
            "sketch.minhash_compat@3",
        ]);
    }
    if cfg!(all(feature = "sketch", feature = "pdf")) {
        expected.push("sketch.page_phash@4");
    }
    if cfg!(feature = "items") {
        expected.push("item.lines@6");
    }
    if cfg!(feature = "summary") {
        expected.extend([
            // Plan order: the card and the topic map run after the keywords they read.
            "summary.extractive@2",
            "summary.outline@1",
            "summary.keywords@1",
            "summary.card@4",
            "summary.topics@1",
        ]);
    }
    assert_eq!(ids, expected);
    let features: Vec<_> = v["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert_eq!(features.contains(&"clauses"), cfg!(feature = "clauses"));
    // Deterministic.
    let again: Json = serde_json::from_slice(&run(&["version", "--json"]).stdout).unwrap();
    assert_eq!(again["fingerprint"], v["fingerprint"]);
}

#[test]
fn usage_errors() {
    let o = run(&["scan"]);
    assert!(!o.status.success());
    let o = run(&["scan", "x", "--manifest", "m"]);
    assert!(!o.status.success());
    let o = run(&["scan", ".", "--max-bytes", "12Q"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("invalid size unit"));
    #[cfg(not(feature = "zstd"))]
    {
        let o = run(&["scan", ".", "--zstd"]);
        assert!(stderr(&o).contains("zstd"));
        assert!(!o.status.success());
    }
}

/// With the text feature, an extracted DOCX also gets text statistics and a language.
#[test]
#[cfg(all(feature = "docx", feature = "text"))]
fn docx_gets_text_facts() {
    let t = tempfile::tempdir().unwrap();
    let body = "The supplier shall deliver the services described in this agreement \
                within thirty days of the effective date, and the customer shall pay \
                every invoice within forty five days of receipt.";
    let a = write(t.path(), "a.docx", &docx("Service Agreement", body));
    let out = t.path().join("out.jsonl");
    let o = run(&[
        "scan",
        a.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--quiet",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&fs::read_to_string(&out).unwrap());
    let keys: Vec<&str> = recs[0].facts.iter().map(|f| f.key.as_str()).collect();
    for k in ["text.chars", "text.words", "text.lang"] {
        assert!(keys.contains(&k), "{k} missing in {keys:?}");
    }
    let lang = recs[0].facts_for("text.lang").next().unwrap();
    assert_eq!(lang.value, quail_core::Value::text("en"));
}

/// A plain-text contract (Windows-1252 with CRLF line endings) goes through the whole pipeline
/// like a DOCX: text, signal, ref, party and summary facts.
#[test]
#[cfg(all(
    feature = "text",
    feature = "signals",
    feature = "refs",
    feature = "parties",
    feature = "summary"
))]
fn text_file_gets_the_full_pipeline() {
    let t = tempfile::tempdir().unwrap();
    let body = "SUPPLY AGREEMENT\r\n\r\n\
                This Agreement No. 114/23 is made between Alpha Widgets LLC (the \"Supplier\")\r\n\
                and Beta Components GmbH (the \"Customer\").\r\n\r\n\
                Effective Date: March 1, 2021. Payments go to IBAN DE89 3704 0044 0532 0130 00.\r\n\
                Total: EUR 4,200.50 for the delivery of widgets to the M\u{fc}nchen site.\r\n\
                \r\n\
                The Supplier shall deliver the goods within thirty days of the Effective Date.\r\n";
    // Windows-1252: every char of the body is Latin-1, one byte each.
    let bytes: Vec<u8> = body
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap())
        .collect();
    assert!(std::str::from_utf8(&bytes).is_err());
    let a = write(t.path(), "contract.txt", &bytes);
    let out = t.path().join("out.jsonl");
    let o = run(&[
        "scan",
        a.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--quiet",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&fs::read_to_string(&out).unwrap());
    let r = &recs[0];
    assert_eq!(r.format, Format::Text);
    assert_eq!(r.status, Status::Ok, "{:?}", r.warnings);
    assert_eq!(r.pages, Some(1));
    assert!(r.steps.iter().any(|s| s.name == "text.extract"));
    let keys: Vec<&str> = r.facts.iter().map(|f| f.key.as_str()).collect();
    for k in [
        "text.encoding",
        "text.line_endings",
        "text.chars",
        "text.lang",
        "signal.iban",
        "signal.date_effective",
        "ref.instrument_kind",
        "party.company",
    ] {
        assert!(keys.contains(&k), "{k} missing in {keys:?}");
    }
    assert!(
        keys.iter().any(|k| k.starts_with("summary.")),
        "no summary.* in {keys:?}"
    );
    let enc = r.facts_for("text.encoding").next().unwrap();
    assert_eq!(enc.value, quail_core::Value::text("windows-1252"));
    let le = r.facts_for("text.line_endings").next().unwrap();
    assert_eq!(le.value, quail_core::Value::text("crlf"));
    let iban = r.facts_for("signal.iban").next().unwrap();
    assert_eq!(
        iban.value,
        quail_core::Value::text("DE89370400440532013000")
    );
}

/// With the signals, refs and parties features, a contract-like DOCX gets their facts, and
/// `inspect` prints record values field by field.
#[test]
#[cfg(all(
    feature = "docx",
    feature = "signals",
    feature = "refs",
    feature = "parties"
))]
fn docx_gets_signal_ref_and_party_facts() {
    let t = tempfile::tempdir().unwrap();
    let body = "This Agreement No. 114/23 is made between Alpha Widgets LLC (the \"Supplier\") \
                and Beta Components GmbH (the \"Customer\"). Effective Date: March 1, 2021. \
                Payments go to IBAN DE89 3704 0044 0532 0130 00. Total: EUR 4,200.50.";
    let a = write(t.path(), "a.docx", &docx("Supply Agreement", body));
    let out = t.path().join("out.jsonl");
    let o = run(&[
        "scan",
        a.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--quiet",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let recs = records(&fs::read_to_string(&out).unwrap());
    let r = &recs[0];
    let keys: Vec<&str> = r.facts.iter().map(|f| f.key.as_str()).collect();
    for k in [
        "signal.iban",
        "signal.date_effective",
        "ref.instrument_kind",
        "party.company",
    ] {
        assert!(keys.contains(&k), "{k} missing in {keys:?}");
    }
    let iban = r.facts_for("signal.iban").next().unwrap();
    assert_eq!(
        iban.value,
        quail_core::Value::text("DE89370400440532013000")
    );
    let surfaces: Vec<String> = r
        .facts_for("party.company")
        .filter_map(|f| match f.value.field("surface") {
            Some(quail_core::Value::Text(s)) => Some(s.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        surfaces.iter().any(|s| s.contains("Alpha Widgets")),
        "{surfaces:?}"
    );

    let o = run(&["inspect", a.to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(
        s.contains("[party]") && s.contains("[ref]") && s.contains("[signal]"),
        "{s}"
    );
    assert!(s.contains("surface=\"Alpha Widgets LLC\""), "{s}");
    // Ranked candidates: one line per record.
    assert!(s.contains("record(s)") && s.contains("    - {key="), "{s}");
}

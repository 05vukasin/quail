//! End-to-end tests: run the `quail` binary over files generated in a temp dir.

#![allow(clippy::many_single_char_names)]

use std::fs;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use quail_core::{FileRecord, Format, Status, Value};
use serde_json::Value as Json;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

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

/// A minimal DOCX: one section with a title paragraph, a body paragraph and core properties.
fn docx(title: &str, body: &str) -> Vec<u8> {
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_owned(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#.to_owned(),
        ),
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="{W}"><w:body><w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>{title}</w:t></w:r></w:p><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"#
            ),
        ),
        (
            "docProps/core.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></cp:coreProperties>"#
            ),
        ),
    ];
    for (name, data) in parts {
        z.start_file(name, opts).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
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

fn blake3_hex(r: &FileRecord) -> String {
    match &r.facts_for("hash.blake3").next().expect("blake3").value {
        Value::Bytes(b) => quail_cli::tap::hex(b),
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
    // Only extracted documents are dumped: the two DOCX files.
    assert_eq!(lines.len(), 2);
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
        assert!(doc.pages[0].text.contains("text") || doc.pages[0].text.contains("agree"));
        assert!(rec.steps.iter().any(|s| s.name == "cli.dump_documents"));
        assert_eq!(rec.status, Status::Ok);
    }
    let txt = recs.iter().find(|r| name(r) == "b.txt").unwrap();
    assert_eq!(
        txt.status,
        Status::Ok,
        "the tap adds no problem without a document"
    );
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
    stdin.write_all(b"f0.txt\nf1.txt\n").unwrap();
    stdin.flush().unwrap();
    // Wait until the first file went through, so the run is under way; the sink buffers,
    // so watch for the source being blocked on stdin instead: give it a moment.
    std::thread::sleep(Duration::from_millis(500));
    let pid = child.id().to_string();
    let killed = Command::new("kill").args(["-INT", &pid]).status().unwrap();
    assert!(killed.success());
    std::thread::sleep(Duration::from_millis(200));
    // The source is blocked reading stdin: one more line lets it notice the stop.
    let _ = stdin.write_all(b"f2.txt\n");
    let _ = stdin.flush();
    let status = wait_timeout(&mut child, Duration::from_secs(20));
    drop(stdin);
    let mut recs = String::new();
    out.read_to_string(&mut recs).unwrap();
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert_eq!(status.code(), Some(130), "{err}");
    assert!(err.contains("interrupted"), "{err}");
    assert!(err.contains("cancelled"), "{err}");
    // Records of files already pulled are written in full.
    let n = records(&recs).len();
    assert!((2..=3).contains(&n), "{n} records");
}

#[test]
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
    assert!(s.contains("src docx.extract@1"), "{s}");
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
fn keys_and_version() {
    let o = run(&["keys"]);
    assert!(o.status.success());
    let s = stdout(&o);
    for k in [
        "engine.format",
        "hash.blake3",
        "docx.title",
        "docx.extract@1",
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
    assert_eq!(ids, ["engine.sniff@1", "docx.extract@1", "hash.identity@1"]);
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

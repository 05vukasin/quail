//! Failure-matrix tests: an engine whose analyzer misbehaves on command (abort, allocate
//! until the address-space limit, overflow the stack, hang, spin, panic, exit, print to
//! stdout) runs in isolated workers, and every other file must still come out right.
//!
//! The test binary is its own worker executable: `main` calls `maybe_run_worker` first, as
//! a real binary does, so the harness is custom (`harness = false`). Run one test with
//! `cargo test -p quail-worker --test isolation -- <name substring>`.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Error, Extractor, FactKey, FileRecord,
    Format, Limits, Page, PageKind, RecordSink, Result, Source, SourceItem, Status, TextSource,
    Value,
};
use quail_engine::{Engine, EngineBuilder, RunOptions};
use quail_worker::{
    CrashKind, IsolatedRunner, IsolatedStats, IsolationOptions, Job, JobInput, PathJobs,
    StderrMode, WorkerConfig, WorkerError, WorkerProcess, maybe_run_worker,
};

// ---------------------------------------------------------------------------------------
// The misbehaving engine (runs in the workers)

/// Lines starting with `!` are commands.
struct Trap;

#[inline(never)]
fn recurse(depth: u64) -> u64 {
    let pad = std::hint::black_box([depth.to_le_bytes()[0]; 4096]);
    if depth == u64::MAX {
        return u64::from(pad[0]);
    }
    recurse(depth + 1) + u64::from(pad[1])
}

impl Analyzer for Trap {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("test.trap", 1)
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let text = String::from_utf8_lossy(ctx.blob.as_bytes());
        for line in text.lines() {
            let Some(cmd) = line.strip_prefix('!') else {
                continue;
            };
            let (cmd, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
            match cmd {
                "ABORT" => std::process::abort(),
                "OOM" => {
                    // Up to 512 MiB in 16 MiB pieces, each touched: twice the test's
                    // 256 MiB RLIMIT_AS, and bounded so that an OS without RLIMIT_AS just
                    // finishes without pressuring the machine.
                    let mut keep = Vec::new();
                    for _ in 0..32 {
                        keep.push(std::hint::black_box(vec![0x5au8; 16 << 20]));
                    }
                    out.fact(FactKey::new("test.oom_survived"), Value::Bool(true));
                    drop(keep);
                }
                "STACK" => {
                    std::hint::black_box(recurse(0));
                }
                "HANG" => loop {
                    std::thread::sleep(Duration::from_secs(1));
                },
                "SPIN" => {
                    let t = Instant::now();
                    let mut x = 0u64;
                    while t.elapsed() < Duration::from_mins(1) {
                        x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
                    }
                }
                "PANIC" => panic!("trap panic"),
                "EXIT" => std::process::exit(3),
                "PRINT" => println!("this line must not reach the protocol channel"),
                "SLEEP" => std::thread::sleep(Duration::from_millis(arg.parse().unwrap_or(0))),
                "FLAKY" => {
                    let marker = PathBuf::from(arg);
                    if !marker.exists() {
                        let _ = std::fs::write(&marker, b"x");
                        std::process::abort();
                    }
                }
                _ => {}
            }
        }
        out.fact(
            FactKey::new("test.len"),
            Value::Int(i64::try_from(ctx.blob.len()).unwrap_or(0)),
        );
        Ok(())
    }
}

/// Plain text becomes a one-page document.
struct TextExtract;

impl Extractor for TextExtract {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("test.extract", 1)
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::Text]
    }
    fn extract(&self, blob: &Blob, _: &Limits, _: &mut Emitter) -> Result<Document> {
        Ok(Document {
            pages: vec![Page {
                text: String::from_utf8_lossy(blob.as_bytes()).into_owned(),
                text_source: TextSource::Native,
                ..Page::new(0, PageKind::Logical)
            }],
        })
    }
}

fn test_engine() -> Engine {
    EngineBuilder::new()
        .quiet_panics(true)
        .limits(Limits {
            timeout: Duration::from_secs(1),
            ..Limits::default()
        })
        .extractor(Box::new(TextExtract))
        .analyzer(Box::new(Trap))
        .build()
        .expect("test engine")
}

// ---------------------------------------------------------------------------------------
// Supervisor-side helpers

fn isolation() -> IsolationOptions {
    IsolationOptions {
        worker: WorkerConfig {
            rlimit_as: Some(256 << 20),
            stderr: StderrMode::Discard,
            ..WorkerConfig::default()
        },
        grace: Duration::from_millis(500),
        ..IsolationOptions::default()
    }
}

fn run_opts(threads: usize) -> RunOptions {
    RunOptions {
        threads,
        max_in_flight: threads * 2,
        ..RunOptions::default()
    }
}

struct VecSource {
    items: std::vec::IntoIter<SourceItem>,
    pulled: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
    max_gap: Arc<AtomicU64>,
    cancel_after: Option<(u64, Arc<AtomicBool>)>,
}

impl VecSource {
    fn new(items: Vec<SourceItem>, written: &Arc<AtomicU64>) -> Self {
        Self {
            items: items.into_iter(),
            pulled: Arc::default(),
            written: Arc::clone(written),
            max_gap: Arc::default(),
            cancel_after: None,
        }
    }
}

impl Source for VecSource {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        let item = self.items.next()?;
        let pulled = self.pulled.fetch_add(1, Ordering::SeqCst) + 1;
        let gap = pulled - self.written.load(Ordering::SeqCst);
        self.max_gap.fetch_max(gap, Ordering::SeqCst);
        if let Some((n, flag)) = &self.cancel_after
            && pulled >= *n
        {
            flag.store(true, Ordering::SeqCst);
        }
        Some(Ok(item))
    }
}

#[derive(Default)]
struct Collect {
    records: Arc<Mutex<Vec<FileRecord>>>,
    written: Arc<AtomicU64>,
    fail_at: Option<u64>,
}

impl RecordSink for Collect {
    fn write(&mut self, record: &FileRecord) -> Result<()> {
        let n = self.written.load(Ordering::SeqCst);
        if self.fail_at.is_some_and(|f| n + 1 >= f) {
            return Err(Error::Other("disk full".into()));
        }
        self.records.lock().unwrap().push(record.clone());
        self.written.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn item(locator: &str, body: &str) -> SourceItem {
    SourceItem {
        locator: locator.to_owned(),
        blob: Blob::from(body.as_bytes().to_vec()),
        modified: None,
    }
}

fn error_of(r: &FileRecord) -> &str {
    match &r.status {
        Status::Failed { error } => error,
        other => panic!("{} expected Failed, got {other:?}", r.locator),
    }
}

fn by_locator(records: &[FileRecord]) -> HashMap<String, FileRecord> {
    records
        .iter()
        .map(|r| (r.locator.clone(), r.clone()))
        .collect()
}

fn tmp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("quail-worker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn kind(stats: &IsolatedStats, k: &str) -> u64 {
    stats.crash_kinds.get(k).copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------------------
// Tests

fn failure_matrix() {
    let bad = [
        ("bad/abort", "!ABORT"),
        ("bad/oom", "!OOM"),
        ("bad/stack", "!STACK"),
        ("bad/hang", "!HANG"),
        ("bad/panic", "!PANIC"),
        ("bad/exit", "!EXIT"),
        ("bad/print", "!PRINT"),
    ];
    let mut items = Vec::new();
    for i in 0..40 {
        items.push(item(&format!("good/{i}"), &format!("good file {i}\n")));
        if i % 5 == 2 {
            let (l, b) = bad[(i / 5) % bad.len()];
            items.push(item(l, b));
        }
    }
    let total = items.len() as u64;
    let sink = Collect::default();
    let records = Arc::clone(&sink.records);
    let mut src = VecSource::new(items, &sink.written);
    let max_gap = Arc::clone(&src.max_gap);
    let mut sink = sink;
    let mut runner = IsolatedRunner::new(isolation());
    let t = Instant::now();
    let stats = runner
        .run(&mut src, &mut sink, &run_opts(4))
        .expect("workers start");
    let elapsed = t.elapsed();
    eprintln!("    {stats}");
    let recs = records.lock().unwrap();
    assert_eq!(stats.run.files, total);
    assert_eq!(recs.len() as u64, total, "one record per file");
    let m = by_locator(&recs);
    for i in 0..40 {
        let r = &m[&format!("good/{i}")];
        assert_eq!(r.status, Status::Ok, "{r:?}");
        assert!(r.facts_for("test.len").next().is_some());
    }
    let abort = error_of(&m["bad/abort"]);
    assert!(abort.starts_with("worker crashed: "), "{abort}");
    assert!(abort.contains("aborted"), "{abort}");
    assert!(abort.contains("twice; skipped"), "{abort}");
    #[cfg(unix)]
    assert!(abort.contains("SIGABRT"), "{abort}");
    let stack = error_of(&m["bad/stack"]);
    assert!(stack.contains("stack overflow"), "{stack}");
    let hang = error_of(&m["bad/hang"]);
    assert!(hang.contains("timed out"), "{hang}");
    let exit = error_of(&m["bad/exit"]);
    assert!(exit.contains("exited with code 3"), "{exit}");
    match &m["bad/panic"].status {
        Status::Partial { problems } => {
            assert!(
                problems.iter().any(|p| p.contains("trap panic")),
                "{problems:?}"
            );
        }
        s => panic!("panic must stay in-process (Partial), got {s:?}"),
    }
    if cfg!(target_os = "linux") {
        let oom = error_of(&m["bad/oom"]);
        assert!(oom.contains("out of memory"), "{oom}");
        assert_eq!(kind(&stats, "oom"), 2);
    }
    if cfg!(unix) {
        assert_eq!(m["bad/print"].status, Status::Ok, "stdout is redirected");
    }
    // The crashed record keeps size and a sniffed format.
    assert_eq!(m["bad/abort"].size, 6);
    assert_eq!(m["bad/abort"].format, Format::Text);
    assert_eq!(kind(&stats, "abort"), 2);
    assert_eq!(kind(&stats, "stack_overflow"), 2);
    assert_eq!(kind(&stats, "exit"), 2);
    assert_eq!(
        kind(&stats, "timeout"),
        1,
        "timeouts are not retried by default"
    );
    assert!(stats.poisoned >= 3);
    assert!(stats.respawns >= stats.crashes - 4, "{stats:?}");
    assert_eq!(stats.run.failed, stats.poisoned + 1, "poisoned + the hang");
    // Backpressure: never more than max_in_flight files between source and sink (+1 being
    // pulled).
    assert!(max_gap.load(Ordering::SeqCst) <= 8 + 1, "gap {max_gap:?}");
    // One hang costs ~2.5 s; crashes cost a respawn each.
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    // The pool is alive and reusable after all that.
    assert_eq!(runner.pids().len(), 4);
}

fn ordered_output_with_crashes() {
    let mut items = Vec::new();
    for i in 0..30 {
        let body = match i % 7 {
            0 => format!("!SLEEP {}\n", 30 + (i * 13) % 50),
            3 => "!ABORT\n".to_owned(),
            _ => format!("file {i}\n"),
        };
        items.push(item(&format!("f{i:02}"), &body));
    }
    let expected: Vec<String> = items.iter().map(|i| i.locator.clone()).collect();
    let mut sink = Collect::default();
    let records = Arc::clone(&sink.records);
    let mut src = VecSource::new(items, &sink.written);
    let opts = RunOptions {
        ordered: true,
        ..run_opts(3)
    };
    let mut runner = IsolatedRunner::new(isolation());
    let stats = runner.run(&mut src, &mut sink, &opts).unwrap();
    let got: Vec<String> = records
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.locator.clone())
        .collect();
    assert_eq!(got, expected);
    assert_eq!(stats.run.failed, 4); // f03, f10, f17, f24
    assert_eq!(stats.poisoned, 4);
}

fn retry_recovers_a_flaky_file() {
    let dir = tmp_dir("flaky");
    let marker = dir.join("marker");
    let mut sink = Collect::default();
    let records = Arc::clone(&sink.records);
    let mut src = VecSource::new(
        vec![
            item("ok", "fine"),
            item("flaky", &format!("!FLAKY {}", marker.display())),
        ],
        &sink.written,
    );
    let mut runner = IsolatedRunner::new(isolation());
    let stats = runner.run(&mut src, &mut sink, &run_opts(1)).unwrap();
    let m = by_locator(&records.lock().unwrap());
    assert_eq!(m["flaky"].status, Status::Ok);
    assert!(
        m["flaky"]
            .warnings
            .iter()
            .any(|w| w.contains("first attempt crashed")),
        "{:?}",
        m["flaky"].warnings
    );
    assert_eq!(
        (
            stats.crashes,
            stats.retries,
            stats.retried_ok,
            stats.poisoned
        ),
        (1, 1, 1, 0)
    );

    // Without retry the first crash is final.
    std::fs::remove_file(&marker).unwrap();
    let mut sink = Collect::default();
    let records = Arc::clone(&sink.records);
    let mut src = VecSource::new(
        vec![item("flaky", &format!("!FLAKY {}", marker.display()))],
        &sink.written,
    );
    let mut runner = IsolatedRunner::new(IsolationOptions {
        retry_crashed: false,
        ..isolation()
    });
    let stats = runner.run(&mut src, &mut sink, &run_opts(1)).unwrap();
    let e = error_of(&records.lock().unwrap()[0]).to_owned();
    assert!(!e.contains("twice"), "{e}");
    assert_eq!((stats.crashes, stats.retries, stats.poisoned), (1, 0, 0));
    let _ = std::fs::remove_dir_all(dir);
}

fn cancel_stops_pulling() {
    let items: Vec<_> = (0..200)
        .map(|i| item(&format!("c{i}"), "!SLEEP 5\n"))
        .collect();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut sink = Collect::default();
    let records = Arc::clone(&sink.records);
    let mut src = VecSource::new(items, &sink.written);
    src.cancel_after = Some((10, Arc::clone(&cancel)));
    let pulled = Arc::clone(&src.pulled);
    let opts = RunOptions {
        cancel: Some(cancel),
        ..run_opts(2)
    };
    let mut runner = IsolatedRunner::new(isolation());
    let stats = runner.run(&mut src, &mut sink, &opts).unwrap();
    assert!(stats.run.cancelled);
    let n = records.lock().unwrap().len() as u64;
    assert_eq!(
        n,
        pulled.load(Ordering::SeqCst),
        "every pulled file is finished"
    );
    assert!(n < 20, "{n}");
}

fn path_mode_reads_in_the_worker() {
    let dir = tmp_dir("paths");
    let good = dir.join("good.txt");
    let pdf = dir.join("bad.pdf");
    std::fs::write(&good, "hello from disk").unwrap();
    std::fs::write(&pdf, "%PDF-1.7\n!ABORT\n").unwrap();
    let missing = dir.join("missing.txt");
    let mut jobs = PathJobs::new(vec![Ok(good.clone()), Ok(pdf.clone()), Ok(missing)].into_iter());
    let mut sink = Collect::default();
    let records = Arc::clone(&sink.records);
    let mut runner = IsolatedRunner::new(isolation());
    let stats = runner.run_jobs(&mut jobs, &mut sink, &run_opts(2)).unwrap();
    assert_eq!(stats.run.files, 3);
    let m = by_locator(&records.lock().unwrap());
    let g = &m[&good.to_string_lossy().into_owned()];
    assert_eq!(g.status, Status::Ok);
    assert_eq!(g.size, 15);
    let p = &m[&pdf.to_string_lossy().into_owned()];
    assert!(error_of(p).contains("worker crashed"));
    assert_eq!(p.format, Format::Pdf, "sniffed in the supervisor");
    assert_eq!(p.size, 16);
    let miss = m
        .values()
        .find(|r| r.locator.ends_with("missing.txt"))
        .unwrap();
    assert!(error_of(miss).contains("worker: io"), "{miss:?}");
    let _ = std::fs::remove_dir_all(dir);
}

fn documents_reach_the_artifacts_hook() {
    let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let s = Arc::clone(&seen);
    let opts = RunOptions {
        on_artifacts: Some(Arc::new(
            move |r: &FileRecord, a: &quail_core::Artifacts| {
                assert!(r.locator != "boom", "hook panic");
                let doc = a.get::<Document>().expect("document artifact");
                s.lock()
                    .unwrap()
                    .push((r.locator.clone(), doc.pages[0].text.clone()));
            },
        )),
        ..run_opts(2)
    };
    let mut sink = Collect::default();
    let mut src = VecSource::new(
        vec![
            item("a", "alpha text"),
            item("boom", "beta"),
            item("c", "!ABORT"),
        ],
        &sink.written,
    );
    let mut runner = IsolatedRunner::new(isolation());
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let stats = runner.run(&mut src, &mut sink, &opts).unwrap();
    panic::set_hook(prev);
    let seen = seen.lock().unwrap();
    assert_eq!(*seen, vec![("a".to_owned(), "alpha text".to_owned())]);
    assert_eq!(stats.run.hook_panics, 1);
    assert_eq!(stats.run.files, 3);

    // The single-file API returns the document too.
    let (rec, doc) = runner.process(&Job::from(item("one", "single")), true);
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(doc.unwrap().pages[0].text, "single");
}

fn sink_error_stops_the_run() {
    let items: Vec<_> = (0..100).map(|i| item(&format!("s{i}"), "x")).collect();
    let mut sink = Collect {
        fail_at: Some(3),
        ..Collect::default()
    };
    let mut src = VecSource::new(items, &sink.written);
    let pulled = Arc::clone(&src.pulled);
    let mut runner = IsolatedRunner::new(isolation());
    let stats = runner.run(&mut src, &mut sink, &run_opts(2)).unwrap();
    assert!(stats.run.stopped_on_sink_error);
    assert_eq!(stats.run.sink_errors, 1);
    assert_eq!(stats.run.first_sink_error.as_deref(), Some("disk full"));
    assert!(pulled.load(Ordering::SeqCst) < 100);
    assert_eq!(stats.run.files, pulled.load(Ordering::SeqCst));
}

fn cpu_limit_kills_a_spinning_worker() {
    if !cfg!(unix) {
        return;
    }
    let mut runner = IsolatedRunner::new(IsolationOptions {
        worker: WorkerConfig {
            rlimit_cpu: Some(Duration::from_secs(1)),
            ..isolation().worker
        },
        kill_after: Some(Duration::from_secs(30)),
        retry_crashed: false,
        ..isolation()
    });
    let t = Instant::now();
    let (rec, _) = runner.process(&Job::from(item("spin", "!SPIN")), false);
    let e = error_of(&rec);
    assert!(e.contains("CPU time limit"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    // The limit is per file: a second file in the fresh worker is fine.
    let (rec, _) = runner.process(&Job::from(item("ok", "!SLEEP 10")), false);
    assert_eq!(rec.status, Status::Ok);
}

fn single_worker_crash_and_respawn() {
    let cfg = isolation().worker;
    let mut w = WorkerProcess::spawn(&cfg).unwrap();
    assert_eq!(w.engine_timeout(), Duration::from_secs(1));
    let ok = w
        .process(&Job::from(item("a", "hi")), false, Duration::from_secs(5))
        .unwrap();
    assert_eq!(ok.record.status, Status::Ok);
    let crash = w
        .process(
            &Job::from(item("b", "!EXIT")),
            false,
            Duration::from_secs(5),
        )
        .unwrap_err();
    assert_eq!(crash.kind, CrashKind::Exit(3));
    assert!(!w.is_alive());
    let pid = w.pid();
    drop(w);
    let mut w = WorkerProcess::spawn(&cfg).unwrap();
    assert_ne!(w.pid(), pid);
    let hang = w
        .process(
            &Job::from(item("h", "!HANG")),
            false,
            Duration::from_millis(300),
        )
        .unwrap_err();
    assert_eq!(hang.kind, CrashKind::Timeout);
    // Bytes vs path inputs are both accepted by the same worker type.
    assert!(matches!(Job::path("x").input, JobInput::Path(_)));
}

fn startup_failures_are_errors() {
    let mut runner = IsolatedRunner::new(IsolationOptions {
        worker: WorkerConfig {
            program: Some("/nonexistent/quail-worker".into()),
            ..WorkerConfig::default()
        },
        ..IsolationOptions::default()
    });
    assert!(matches!(runner.warm_up(1), Err(WorkerError::Spawn(_))));
    #[cfg(unix)]
    {
        // An executable that is not a worker: exits at once.
        let mut runner = IsolatedRunner::new(IsolationOptions {
            worker: WorkerConfig {
                program: Some("true".into()),
                ..WorkerConfig::default()
            },
            ..IsolationOptions::default()
        });
        let mut sink = Collect::default();
        let mut src = VecSource::new(vec![item("a", "x")], &sink.written);
        match runner.run(&mut src, &mut sink, &run_opts(1)) {
            Err(WorkerError::Startup(m)) => assert!(m.contains("maybe_run_worker"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Harness

type Test = (&'static str, fn());

const TESTS: &[Test] = &[
    ("failure_matrix", failure_matrix),
    ("ordered_output_with_crashes", ordered_output_with_crashes),
    ("retry_recovers_a_flaky_file", retry_recovers_a_flaky_file),
    ("cancel_stops_pulling", cancel_stops_pulling),
    (
        "path_mode_reads_in_the_worker",
        path_mode_reads_in_the_worker,
    ),
    (
        "documents_reach_the_artifacts_hook",
        documents_reach_the_artifacts_hook,
    ),
    ("sink_error_stops_the_run", sink_error_stops_the_run),
    (
        "cpu_limit_kills_a_spinning_worker",
        cpu_limit_kills_a_spinning_worker,
    ),
    (
        "single_worker_crash_and_respawn",
        single_worker_crash_and_respawn,
    ),
    ("startup_failures_are_errors", startup_failures_are_errors),
];

fn main() -> ExitCode {
    maybe_run_worker(test_engine);

    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    // No test may hang: a deadlock fails the whole binary.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_mins(5));
        eprintln!("isolation tests: global timeout, a test is stuck");
        std::process::exit(1);
    });
    let mut failed = Vec::new();
    let mut ran = 0;
    for (name, test) in TESTS {
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        ran += 1;
        let t = Instant::now();
        let r = panic::catch_unwind(AssertUnwindSafe(test));
        let status = if r.is_ok() { "ok" } else { "FAILED" };
        eprintln!("test {name} ... {status} ({:.2?})", t.elapsed());
        if r.is_err() {
            failed.push(*name);
        }
    }
    eprintln!(
        "\nisolation: {} passed, {} failed",
        ran - failed.len(),
        failed.len()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!("failed: {failed:?}");
        ExitCode::FAILURE
    }
}

//! Per-file cost of isolation: the same small text files through `Engine::run` in process
//! and through `IsolatedRunner` with 1, 4 and 8 workers (bytes over the pipe, paths read by
//! the worker, and with the document sent back).
//!
//! A throughput benchmark, not a criterion micro-benchmark: the binary is its own worker
//! executable, so `main` must call `maybe_run_worker` first. Run with
//! `cargo bench -p quail-worker` (`QUAIL_BENCH_FILES` sets the file count, default 20000).
#![allow(missing_docs, clippy::too_many_lines)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, FactKey, FileRecord, Format,
    Limits, Page, PageKind, RecordSink, Result, Source, SourceItem, TextSource, Value,
};
use quail_engine::{Engine, EngineBuilder, RunOptions};
use quail_worker::{IsolatedRunner, IsolationOptions, PathJobs, maybe_run_worker};

struct TextExtract;
impl Extractor for TextExtract {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("bench.extract", 1)
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

/// A cheap analyzer that emits a few facts, like a real one would.
struct Words(&'static str);
impl Analyzer for Words {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new(self.0, 1)
    }
    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let doc = ctx.get::<Document>().map_or(0, |d| {
            d.pages
                .iter()
                .map(|p| p.text.split_whitespace().count())
                .sum()
        });
        out.fact(
            FactKey::new("bench.words"),
            Value::Int(i64::try_from(doc).unwrap_or(0)),
        );
        out.fact(FactKey::new("bench.lang"), Value::text("en"));
        Ok(())
    }
}

const NAMES: [&str; 4] = ["bench.a", "bench.b", "bench.c", "bench.d"];

fn engine() -> Engine {
    let mut b = EngineBuilder::new().extractor(Box::new(TextExtract));
    for n in NAMES {
        b = b.analyzer(Box::new(Words(n)));
    }
    b.build().expect("bench engine")
}

fn body(i: usize) -> Vec<u8> {
    let mut s = format!("Document {i}\n");
    while s.len() < 2048 {
        s.push_str("the quick brown fox jumps over the lazy dog; invoice terms apply. ");
    }
    s.into_bytes()
}

struct Items {
    n: usize,
    i: usize,
}
impl Source for Items {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        if self.i == self.n {
            return None;
        }
        self.i += 1;
        Some(Ok(SourceItem {
            locator: format!("bench/{}.txt", self.i),
            blob: Blob::from(body(self.i)),
            modified: None,
        }))
    }
}

/// Counts and drops, the way a fast sink would.
struct Count(u64);
impl RecordSink for Count {
    fn write(&mut self, _: &FileRecord) -> Result<()> {
        self.0 += 1;
        Ok(())
    }
}

fn opts(threads: usize) -> RunOptions {
    RunOptions {
        threads,
        max_in_flight: threads * 4,
        ..RunOptions::default()
    }
}

fn report(label: &str, files: u64, elapsed: Duration, base: Option<Duration>, n: usize) {
    #[allow(clippy::cast_precision_loss)]
    let f = files as f64;
    let us = elapsed.as_secs_f64() * 1e6 / f;
    let extra = base.map_or(String::new(), |b| {
        #[allow(clippy::cast_precision_loss)]
        let per_worker = (elapsed.as_secs_f64() - b.as_secs_f64()) * 1e6 * n as f64 / f;
        format!("  overhead {per_worker:>7.1} µs/file per worker")
    });
    println!(
        "{label:<34} {:>9.0} files/s  {us:>7.2} µs/file wall{extra}",
        f / elapsed.as_secs_f64()
    );
}

fn main() {
    maybe_run_worker(engine);
    let n: usize = std::env::var("QUAIL_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    // Criterion-style flags from `cargo bench` are ignored.
    let eng = engine();

    // Serialization alone: encode in the child, decode in the supervisor.
    let rec = eng.process(SourceItem {
        locator: "bench/x.txt".into(),
        blob: Blob::from(body(1)),
        modified: None,
    });
    let reps = 20_000u32;
    let t = Instant::now();
    let mut bytes = 0;
    for _ in 0..reps {
        let j = serde_json::to_vec(&rec).unwrap();
        bytes = j.len();
        let back: FileRecord = serde_json::from_slice(&j).unwrap();
        std::hint::black_box(back);
    }
    println!(
        "record JSON round trip: {:.2} µs ({bytes} bytes, {} facts)",
        t.elapsed().as_secs_f64() * 1e6 / f64::from(reps),
        rec.facts.len()
    );

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("quail-worker-bench");
    let _ = std::fs::create_dir_all(&dir);

    for threads in [1usize, 4, 8] {
        println!("-- {n} files of 2 KiB, N={threads}");
        let mut sink = Count(0);
        let t = Instant::now();
        let s = eng.run(&mut Items { n, i: 0 }, &mut sink, &opts(threads));
        let base = t.elapsed();
        report("in-process Engine::run", s.files, base, None, threads);

        let mut runner = IsolatedRunner::new(IsolationOptions::default());
        runner.warm_up(threads).expect("workers");
        let mut sink = Count(0);
        let t = Instant::now();
        let s = runner
            .run(&mut Items { n, i: 0 }, &mut sink, &opts(threads))
            .expect("run");
        report(
            "isolated, bytes",
            s.run.files,
            t.elapsed(),
            Some(base),
            threads,
        );
        assert_eq!(s.run.files, n as u64);

        let with_docs = RunOptions {
            on_artifacts: Some(std::sync::Arc::new(
                |_: &FileRecord, a: &quail_core::Artifacts| {
                    std::hint::black_box(a.get::<Document>());
                },
            )),
            ..opts(threads)
        };
        let mut sink = Count(0);
        let t = Instant::now();
        let s = runner
            .run(&mut Items { n, i: 0 }, &mut sink, &with_docs)
            .expect("run");
        report(
            "isolated, bytes + document",
            s.run.files,
            t.elapsed(),
            Some(base),
            threads,
        );

        if threads == 4 {
            let files = n.min(5_000);
            let paths: Vec<PathBuf> = (0..files)
                .map(|i| {
                    let p = dir.join(format!("{i}.txt"));
                    std::fs::write(&p, body(i)).unwrap();
                    p
                })
                .collect();
            let mut sink = Count(0);
            let t = Instant::now();
            let s = eng.run(
                &mut PathItems(paths.clone().into_iter()),
                &mut sink,
                &opts(threads),
            );
            let base = t.elapsed();
            report(
                &format!("in-process, {files} paths"),
                s.files,
                base,
                None,
                threads,
            );
            let mut jobs = PathJobs::new(paths.into_iter().map(Ok));
            let mut sink = Count(0);
            let t = Instant::now();
            let s = runner
                .run_jobs(&mut jobs, &mut sink, &opts(threads))
                .expect("run");
            report(
                "isolated, paths",
                s.run.files,
                t.elapsed(),
                Some(base),
                threads,
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads files in the supervisor, as an in-process run would.
struct PathItems(std::vec::IntoIter<PathBuf>);
impl Source for PathItems {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        let p = self.0.next()?;
        Some(std::fs::read(&p).map_err(Into::into).map(|b| SourceItem {
            locator: p.to_string_lossy().into_owned(),
            blob: Blob::from(b),
            modified: None,
        }))
    }
}

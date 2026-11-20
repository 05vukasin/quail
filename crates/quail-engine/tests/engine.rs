//! Engine behaviour with mock extractors and analyzers.
#![allow(missing_docs, clippy::unnecessary_box_returns, clippy::manual_assert)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quail_core::{
    Analyzer, AnalyzerId, Artifact, Blob, Ctx, Document, Emitter, Error, Extractor, FactKey,
    FileRecord, Format, Limits, Page, PageKind, Profile, RecordSink, Result, Source, SourceItem,
    Status, TextSource, Value,
};
use quail_engine::{BuildError, EngineBuilder, RunOptions, SNIFF_ID, keys};

// ---------- mocks ----------

fn page(text: &str) -> Page {
    Page {
        index: 0,
        kind: PageKind::Logical,
        size: None,
        text: text.to_owned(),
        blocks: Vec::new(),
        text_source: TextSource::Native,
        needs_ocr: false,
    }
}

#[derive(Clone, Copy)]
enum ExtractMode {
    Pages(usize),
    Fail,
    Panic,
}

struct MockExtractor {
    name: &'static str,
    formats: &'static [Format],
    mode: ExtractMode,
}

impl MockExtractor {
    fn text(mode: ExtractMode) -> Box<Self> {
        Box::new(Self {
            name: "mock.text",
            formats: &[Format::Text],
            mode,
        })
    }
}

impl Extractor for MockExtractor {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new(self.name, 1)
    }
    fn formats(&self) -> &'static [Format] {
        self.formats
    }
    fn extract(&self, blob: &Blob, _: &Limits, out: &mut Emitter) -> Result<Document> {
        match self.mode {
            ExtractMode::Pages(n) => {
                out.fact(FactKey::new("mock.extractor"), Value::text(self.name));
                let text = String::from_utf8_lossy(blob.as_bytes());
                Ok(Document {
                    pages: (0..n).map(|_| page(&text)).collect(),
                })
            }
            ExtractMode::Fail => Err(Error::Malformed {
                format: Format::Text,
                msg: "bad".into(),
            }),
            ExtractMode::Panic => panic!("extractor exploded"),
        }
    }
}

struct Shingles;
impl Artifact for Shingles {
    const NAME: &'static str = "shingles";
}

type RunFn = dyn Fn(&Ctx<'_>, &mut Emitter) -> Result<()> + Send + Sync;

/// A configurable analyzer.
struct Mock {
    name: &'static str,
    requires: &'static [&'static str],
    provides: &'static [&'static str],
    profile: Profile,
    applies: bool,
    run: Box<RunFn>,
}

impl Mock {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            requires: &[],
            provides: &[],
            profile: Profile::Fast,
            applies: true,
            run: Box::new(move |_, out| {
                out.fact(FactKey::new("mock.ran"), Value::text(name));
                Ok(())
            }),
        }
    }
    fn requires(mut self, r: &'static [&'static str]) -> Self {
        self.requires = r;
        self
    }
    fn provides(mut self, p: &'static [&'static str]) -> Self {
        self.provides = p;
        self
    }
    fn profile(mut self, p: Profile) -> Self {
        self.profile = p;
        self
    }
    fn applies(mut self, a: bool) -> Self {
        self.applies = a;
        self
    }
    fn run(
        mut self,
        f: impl Fn(&Ctx<'_>, &mut Emitter) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.run = Box::new(f);
        self
    }
    fn boxed(self) -> Box<Self> {
        Box::new(self)
    }
}

impl Analyzer for Mock {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new(self.name, 1)
    }
    fn requires(&self) -> &'static [&'static str] {
        self.requires
    }
    fn provides(&self) -> &'static [&'static str] {
        self.provides
    }
    fn profile(&self) -> Profile {
        self.profile
    }
    fn applies(&self, _: &Ctx<'_>) -> bool {
        self.applies
    }
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        (self.run)(ctx, out)
    }
}

fn item(locator: &str, bytes: &[u8]) -> SourceItem {
    SourceItem {
        locator: locator.to_owned(),
        blob: Blob::from(bytes.to_vec()),
        modified: None,
    }
}

fn ran(rec: &FileRecord) -> Vec<String> {
    rec.facts_for("mock.ran")
        .filter_map(|f| match &f.value {
            Value::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect()
}

fn problems(rec: &FileRecord) -> &[String] {
    match &rec.status {
        Status::Partial { problems } => problems,
        other => panic!("expected partial, got {other:?}"),
    }
}

// ---------- process ----------

#[test]
fn emits_format_and_mime_and_runs_everything() {
    let engine = EngineBuilder::new()
        .extractor(MockExtractor::text(ExtractMode::Pages(3)))
        .analyzer(Mock::new("mock.a").requires(&["document"]).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hello"));
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(rec.format, Format::Text);
    assert_eq!(rec.pages, Some(3));
    assert_eq!(rec.size, 5);
    assert_eq!(keys::FORMAT.as_str(), "engine.format");
    assert_eq!(keys::MIME.as_str(), "engine.mime");
    let fmt: Vec<_> = rec.facts_for("engine.format").collect();
    assert_eq!(fmt.len(), 1);
    assert_eq!(fmt[0].value, Value::text("text"));
    assert_eq!(fmt[0].src, SNIFF_ID);
    assert_eq!(
        rec.facts_for("engine.mime").next().unwrap().value,
        Value::text("text/plain; charset=utf-8")
    );
    assert_eq!(rec.facts_for("mock.extractor").count(), 1);
    assert_eq!(ran(&rec), ["mock.a"]);
    let steps: Vec<_> = rec.steps.iter().map(ToString::to_string).collect();
    assert_eq!(steps, ["engine.sniff@1", "mock.text@1", "mock.a@1"]);
    assert_eq!(rec.timings.len(), 3);
}

#[test]
fn pdf_and_docx_detected_in_records() {
    let engine = EngineBuilder::new().build().unwrap();
    let rec = engine.process(item("a.pdf", b"%PDF-1.7\n1 0 obj\n"));
    assert_eq!(rec.format, Format::Pdf);
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(rec.pages, None);
}

#[test]
fn artifacts_flow_in_topological_order() {
    // Registered consumer-first; the plan must still run the provider first.
    let engine = EngineBuilder::new()
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(
            Mock::new("mock.consumer")
                .requires(&["shingles"])
                .run(|ctx, out| {
                    assert!(ctx.get::<Shingles>().is_some());
                    out.fact(FactKey::new("mock.ran"), Value::text("mock.consumer"));
                    Ok(())
                })
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.provider")
                .requires(&["document"])
                .provides(&["shingles"])
                .run(|_, out| {
                    out.provide(Shingles);
                    out.fact(FactKey::new("mock.ran"), Value::text("mock.provider"));
                    Ok(())
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.free").boxed())
        .build()
        .unwrap();
    let plan: Vec<_> = engine.plan().map(|a| a.name).collect();
    assert_eq!(plan, ["mock.provider", "mock.consumer", "mock.free"]);
    let rec = engine.process(item("x.txt", b"hi"));
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(ran(&rec), ["mock.provider", "mock.consumer", "mock.free"]);
}

#[test]
fn analyzer_panic_is_contained_and_others_run() {
    let engine = EngineBuilder::new()
        .quiet_panics(true)
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(Mock::new("mock.before").boxed())
        .analyzer(
            Mock::new("mock.bomb")
                .run(|_, out| {
                    out.fact(FactKey::new("mock.ran"), Value::text("mock.bomb"));
                    out.warn("about to fail");
                    panic!("hostile input {}", 42)
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.after").boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hi"));
    // The panicking analyzer's facts are dropped, its warnings kept.
    assert_eq!(ran(&rec), ["mock.before", "mock.after"]);
    let p = problems(&rec);
    assert!(
        p.iter()
            .any(|m| m == "mock.bomb@1: panic: hostile input 42"),
        "{p:?}"
    );
    assert!(p.iter().any(|m| m == "mock.bomb@1: about to fail"), "{p:?}");
    assert!(rec.steps.iter().any(|s| s.name == "mock.bomb"));
}

#[test]
fn analyzer_error_marks_partial_and_dependents_are_skipped() {
    let engine = EngineBuilder::new()
        .analyzer(
            Mock::new("mock.provider")
                .provides(&["shingles"])
                .run(|_, out| {
                    out.provide(Shingles);
                    Err(Error::Other("nope".into()))
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.consumer").requires(&["shingles"]).boxed())
        .analyzer(
            Mock::new("mock.quiet_consumer")
                .requires(&["shingles"])
                .applies(false)
                .boxed(),
        )
        .build()
        .unwrap();
    let rec = engine.process(item("x.bin", &[0, 1, 2]));
    assert!(ran(&rec).is_empty());
    let p = problems(&rec);
    assert_eq!(
        p,
        [
            "mock.provider@1: nope",
            "mock.consumer@1: skipped, missing artifact `shingles`"
        ]
    );
}

#[test]
fn extractor_failure_and_panic_fail_the_file_but_generic_analyzers_run() {
    for mode in [ExtractMode::Fail, ExtractMode::Panic] {
        let engine = EngineBuilder::new()
            .quiet_panics(true)
            .extractor(MockExtractor::text(mode))
            .analyzer(Mock::new("mock.generic").boxed())
            .analyzer(Mock::new("mock.doc").requires(&["document"]).boxed())
            .build()
            .unwrap();
        let rec = engine.process(item("x.txt", b"hello"));
        assert_eq!(ran(&rec), ["mock.generic"]);
        assert_eq!(rec.pages, None);
        let Status::Failed { error } = &rec.status else {
            panic!("expected failed, got {:?}", rec.status);
        };
        assert!(error.starts_with("mock.text@1: "), "{error}");
        assert!(
            error.contains("mock.doc@1: skipped, missing artifact `document`"),
            "{error}"
        );
    }
}

#[test]
fn too_big_skips_extraction_but_runs_generic_analyzers() {
    let limits = Limits {
        max_bytes: 4,
        ..Limits::default()
    };
    let engine = EngineBuilder::new()
        .limits(limits)
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(Mock::new("mock.generic").boxed())
        .analyzer(Mock::new("mock.doc").requires(&["document"]).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hello world"));
    assert_eq!(ran(&rec), ["mock.generic"]);
    assert_eq!(rec.pages, None);
    assert_eq!(rec.facts_for("mock.extractor").count(), 0);
    let p = problems(&rec);
    assert!(p[0].contains("over max_bytes 4"), "{p:?}");
    assert!(p[1].contains("missing artifact `document`"), "{p:?}");
    // Sniffing still happened.
    assert_eq!(rec.format, Format::Text);
}

#[test]
fn deadline_exceeded_stops_the_plan() {
    let limits = Limits {
        timeout: Duration::from_millis(30),
        ..Limits::default()
    };
    let engine = EngineBuilder::new()
        .limits(limits)
        .analyzer(Mock::new("mock.first").boxed())
        .analyzer(
            Mock::new("mock.slow")
                .run(|_, out| {
                    loop {
                        out.check_deadline()?;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.never").boxed())
        .analyzer(Mock::new("mock.never2").boxed())
        .build()
        .unwrap();
    let t = std::time::Instant::now();
    let rec = engine.process(item("x.txt", b"hi"));
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(ran(&rec), ["mock.first"]);
    let p = problems(&rec);
    assert_eq!(p[0], "mock.slow@1: timeout");
    assert_eq!(p[1], "engine: timeout; 2 analyzer(s) not run");
}

#[test]
fn profile_filters_analyzers() {
    let build = |p| {
        EngineBuilder::new()
            .profile(p)
            .analyzer(Mock::new("mock.fast").boxed())
            .analyzer(Mock::new("mock.std").profile(Profile::Standard).boxed())
            .analyzer(Mock::new("mock.deep").profile(Profile::Deep).boxed())
            .build()
            .unwrap()
    };
    let names = |p| -> Vec<&str> { build(p).plan().map(|a| a.name).collect() };
    assert_eq!(names(Profile::Fast), ["mock.fast"]);
    assert_eq!(names(Profile::Standard), ["mock.fast", "mock.std"]);
    assert_eq!(names(Profile::Deep), ["mock.fast", "mock.std", "mock.deep"]);
    let e = build(Profile::Standard);
    assert_eq!(e.inactive(), [AnalyzerId::new("mock.deep", 1)]);
    let rec = e.process(item("x", b"x"));
    assert_eq!(ran(&rec), ["mock.fast", "mock.std"]);
}

#[test]
fn not_applicable_is_not_a_step() {
    let engine = EngineBuilder::new()
        .analyzer(Mock::new("mock.no").applies(false).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x", b"x"));
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(rec.steps, [SNIFF_ID]);
}

// ---------- build validation ----------

fn err(b: EngineBuilder) -> BuildError {
    match b.build() {
        Ok(_) => panic!("expected a build error"),
        Err(e) => e,
    }
}

#[test]
fn unsatisfied_requirement() {
    let e = err(EngineBuilder::new().analyzer(Mock::new("mock.a").requires(&["ghost"]).boxed()));
    assert_eq!(
        e.to_string(),
        "analyzer mock.a@1 requires artifact `ghost`, which no active analyzer provides"
    );
    let e = err(EngineBuilder::new()
        .profile(Profile::Fast)
        .analyzer(Mock::new("mock.a").requires(&["shingles"]).boxed())
        .analyzer(
            Mock::new("mock.p")
                .provides(&["shingles"])
                .profile(Profile::Deep)
                .boxed(),
        ));
    assert!(
        e.to_string()
            .contains("(mock.p@1 provides it but is above profile Fast)"),
        "{e}"
    );
}

#[test]
fn cycles_are_rejected_with_the_path() {
    let e = err(EngineBuilder::new()
        .analyzer(Mock::new("mock.free").boxed())
        .analyzer(
            Mock::new("mock.a")
                .requires(&["b"])
                .provides(&["a"])
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.b")
                .requires(&["c"])
                .provides(&["b"])
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.c")
                .requires(&["a"])
                .provides(&["c"])
                .boxed(),
        ));
    let BuildError::Cycle(c) = &e else {
        panic!("{e}")
    };
    assert_eq!(c.len(), 4);
    assert_eq!(c.first(), c.last());
    assert_eq!(
        e.to_string(),
        "analyzer dependency cycle: mock.c@1 -> mock.b@1 -> mock.a@1 -> mock.c@1"
    );
    let e = err(EngineBuilder::new().analyzer(
        Mock::new("mock.self")
            .requires(&["x"])
            .provides(&["x"])
            .boxed(),
    ));
    assert!(matches!(e, BuildError::Cycle(_)), "{e}");
}

#[test]
fn duplicate_analyzer() {
    let e = err(EngineBuilder::new()
        .analyzer(Mock::new("mock.a").boxed())
        .analyzer(Mock::new("mock.a").boxed()));
    assert_eq!(
        e,
        BuildError::DuplicateAnalyzer(AnalyzerId::new("mock.a", 1))
    );
}

#[test]
fn extractor_priority() {
    let a = || {
        Box::new(MockExtractor {
            name: "mock.a",
            formats: &[Format::Text, Format::Pdf],
            mode: ExtractMode::Pages(1),
        })
    };
    let b = || {
        Box::new(MockExtractor {
            name: "mock.b",
            formats: &[Format::Text],
            mode: ExtractMode::Pages(2),
        })
    };
    let e = err(EngineBuilder::new().extractor(a()).extractor(b()));
    assert!(matches!(
        e,
        BuildError::AmbiguousExtractor {
            format: Format::Text,
            ..
        }
    ));
    assert!(e.to_string().contains("mock.a@1 and mock.b@1"), "{e}");

    let engine = EngineBuilder::new()
        .extractor(a())
        .extractor_with_priority(b(), 10)
        .build()
        .unwrap();
    assert_eq!(engine.extractor_for(Format::Text).unwrap().name, "mock.b");
    assert_eq!(engine.extractor_for(Format::Pdf).unwrap().name, "mock.a");
    assert_eq!(engine.extractor_for(Format::Docx), None);
    assert_eq!(engine.process(item("x", b"text")).pages, Some(2));

    // A tie below the winner is fine.
    let c = Box::new(MockExtractor {
        name: "mock.c",
        formats: &[Format::Text],
        mode: ExtractMode::Pages(3),
    });
    let engine = EngineBuilder::new()
        .extractor(a())
        .extractor(c)
        .extractor_with_priority(b(), 1)
        .build()
        .unwrap();
    assert_eq!(engine.extractor_for(Format::Text).unwrap().name, "mock.b");
}

// ---------- run ----------

#[derive(Default)]
struct Live {
    now: AtomicUsize,
    max: AtomicUsize,
}

/// Bytes that count how many of their kind are alive.
struct Counted {
    bytes: Vec<u8>,
    live: Arc<Live>,
}

impl Counted {
    fn new(bytes: Vec<u8>, live: &Arc<Live>) -> Self {
        let n = live.now.fetch_add(1, Ordering::SeqCst) + 1;
        live.max.fetch_max(n, Ordering::SeqCst);
        Self {
            bytes,
            live: Arc::clone(live),
        }
    }
}

impl AsRef<[u8]> for Counted {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.live.now.fetch_sub(1, Ordering::SeqCst);
    }
}

struct MockSource {
    next: usize,
    total: usize,
    live: Arc<Live>,
    error_every: usize,
}

impl Source for MockSource {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        if self.next >= self.total {
            return None;
        }
        let i = self.next;
        self.next += 1;
        if self.error_every > 0 && i % self.error_every == self.error_every - 1 {
            return Some(Err(Error::Other("unreadable".into())));
        }
        let bytes = format!("file number {i}\n").into_bytes();
        Some(Ok(SourceItem {
            locator: format!("mem/{i}"),
            blob: Blob::new(Counted::new(bytes, &self.live)),
            modified: None,
        }))
    }
}

#[derive(Default)]
struct VecSink {
    locators: Vec<String>,
    finished: bool,
}

impl RecordSink for VecSink {
    fn write(&mut self, record: &FileRecord) -> Result<()> {
        self.locators.push(record.locator.clone());
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.finished = true;
        Ok(())
    }
}

fn jittery_engine() -> quail_engine::Engine {
    // Variable work per file so completion order differs from source order.
    EngineBuilder::new()
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(
            Mock::new("mock.jitter")
                .run(|ctx, out| {
                    let n = ctx
                        .blob
                        .as_bytes()
                        .iter()
                        .map(|&b| u64::from(b))
                        .sum::<u64>();
                    std::thread::sleep(Duration::from_micros(n % 7 * 50));
                    out.fact(FactKey::new("mock.ran"), Value::Int(1));
                    Ok(())
                })
                .boxed(),
        )
        .build()
        .unwrap()
}

#[test]
fn run_bounds_in_flight_blobs() {
    let engine = jittery_engine();
    let live = Arc::new(Live::default());
    let mut src = MockSource {
        next: 0,
        total: 1000,
        live: Arc::clone(&live),
        error_every: 0,
    };
    let mut sink = VecSink::default();
    let opts = RunOptions {
        threads: 4,
        max_in_flight: 6,
        ordered: false,
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    assert_eq!(stats.files, 1000);
    assert_eq!(stats.ok, 1000);
    assert_eq!(stats.partial + stats.failed, 0);
    assert_eq!(sink.locators.len(), 1000);
    assert!(sink.finished);
    let max = live.max.load(Ordering::SeqCst);
    assert!(max <= 6, "max live blobs {max} > 6");
    assert!(max >= 2, "no parallelism observed: {max}");
    assert_eq!(live.now.load(Ordering::SeqCst), 0);
    assert!(stats.bytes > 0 && stats.files_per_sec() > 0.0);
}

#[test]
fn run_ordered_preserves_source_order_and_skips_source_errors() {
    let engine = jittery_engine();
    let live = Arc::new(Live::default());
    let mut src = MockSource {
        next: 0,
        total: 300,
        live: Arc::clone(&live),
        error_every: 10,
    };
    let mut sink = VecSink::default();
    let opts = RunOptions {
        threads: 4,
        max_in_flight: 8,
        ordered: true,
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    assert_eq!(stats.source_errors, 30);
    assert_eq!(stats.files, 270);
    let expected: Vec<String> = (0..300)
        .filter(|i| i % 10 != 9)
        .map(|i| format!("mem/{i}"))
        .collect();
    assert_eq!(sink.locators, expected);
    assert!(live.max.load(Ordering::SeqCst) <= 8);
}

#[test]
fn run_counts_sink_errors_and_panics() {
    struct Bad(Arc<Mutex<u32>>);
    impl RecordSink for Bad {
        fn write(&mut self, r: &FileRecord) -> Result<()> {
            *self.0.lock().unwrap() += 1;
            if r.locator.ends_with('3') {
                panic!("sink bug");
            }
            Err(Error::Other("disk full".into()))
        }
    }
    let engine = EngineBuilder::new().quiet_panics(true).build().unwrap();
    let mut src = MockSource {
        next: 0,
        total: 20,
        live: Arc::new(Live::default()),
        error_every: 0,
    };
    let calls = Arc::new(Mutex::new(0));
    let mut sink = Bad(Arc::clone(&calls));
    let stats = engine.run(&mut src, &mut sink, &RunOptions::default());
    assert_eq!(*calls.lock().unwrap(), 20);
    assert_eq!(stats.sink_errors, 20);
    assert_eq!(stats.files, 20);
    assert!(stats.first_sink_error.is_some());
}

#[test]
fn default_options_are_sane() {
    let o = RunOptions::default();
    assert!(o.threads >= 1);
    assert!(o.max_in_flight >= o.threads);
    assert!(!o.ordered);
}

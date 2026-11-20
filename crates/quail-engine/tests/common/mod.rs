//! Mock extractors, analyzers, sources and sinks shared by the integration tests.
#![allow(dead_code, clippy::unnecessary_box_returns)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use quail_core::{
    Analyzer, AnalyzerId, Artifact, Blob, Ctx, Document, Emitter, Error, Extractor, FactKey,
    FileRecord, Format, Limits, Page, PageKind, Profile, RecordSink, Result, Source, SourceItem,
    Status, TextSource, Value,
};

pub fn page(text: &str) -> Page {
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
pub enum ExtractMode {
    Pages(usize),
    Fail,
    Panic,
    Encrypted,
    Limit,
    /// Loops on `check_deadline` until it fails.
    Timeout,
    /// Sleeps this long without checking the deadline, then succeeds.
    Sleep(Duration),
}

pub struct MockExtractor {
    pub name: &'static str,
    pub formats: &'static [Format],
    pub mode: ExtractMode,
}

impl MockExtractor {
    pub fn text(mode: ExtractMode) -> Box<Self> {
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
        out.fact(FactKey::new("mock.extractor"), Value::text(self.name));
        out.warn("extractor note");
        let doc = |n| {
            let text = String::from_utf8_lossy(blob.as_bytes());
            Document {
                pages: (0..n).map(|_| page(&text)).collect(),
            }
        };
        match self.mode {
            ExtractMode::Pages(n) => Ok(doc(n)),
            ExtractMode::Fail => Err(Error::Malformed {
                format: Format::Text,
                msg: "bad".into(),
            }),
            ExtractMode::Panic => panic!("extractor exploded"),
            ExtractMode::Encrypted => Err(Error::Encrypted),
            ExtractMode::Limit => Err(Error::LimitExceeded("max_decompressed")),
            ExtractMode::Timeout => loop {
                out.check_deadline()?;
                std::thread::sleep(Duration::from_millis(1));
            },
            ExtractMode::Sleep(d) => {
                std::thread::sleep(d);
                Ok(doc(1))
            }
        }
    }
}

pub struct Shingles;
impl Artifact for Shingles {
    const NAME: &'static str = "shingles";
}

type RunFn = dyn Fn(&Ctx<'_>, &mut Emitter) -> Result<()> + Send + Sync;

/// A configurable analyzer. By default it emits `mock.ran = <name>`.
pub struct Mock {
    pub name: &'static str,
    pub version: u32,
    requires: &'static [&'static str],
    provides: &'static [&'static str],
    profile: Profile,
    applies: Option<fn(&Ctx<'_>) -> bool>,
    run: Box<RunFn>,
}

impl Mock {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            version: 1,
            requires: &[],
            provides: &[],
            profile: Profile::Fast,
            applies: None,
            run: Box::new(move |_, out| {
                out.fact(FactKey::new("mock.ran"), Value::text(name));
                Ok(())
            }),
        }
    }
    pub fn version(mut self, v: u32) -> Self {
        self.version = v;
        self
    }
    pub fn requires(mut self, r: &'static [&'static str]) -> Self {
        self.requires = r;
        self
    }
    pub fn provides(mut self, p: &'static [&'static str]) -> Self {
        self.provides = p;
        self
    }
    pub fn profile(mut self, p: Profile) -> Self {
        self.profile = p;
        self
    }
    pub fn applies(mut self, a: bool) -> Self {
        self.applies = Some(if a { |_| true } else { |_| false });
        self
    }
    pub fn applies_if(mut self, f: fn(&Ctx<'_>) -> bool) -> Self {
        self.applies = Some(f);
        self
    }
    pub fn run(
        mut self,
        f: impl Fn(&Ctx<'_>, &mut Emitter) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.run = Box::new(f);
        self
    }
    pub fn boxed(self) -> Box<Self> {
        Box::new(self)
    }
}

impl Analyzer for Mock {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new(self.name, self.version)
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
    fn applies(&self, ctx: &Ctx<'_>) -> bool {
        self.applies.is_none_or(|f| f(ctx))
    }
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        (self.run)(ctx, out)
    }
}

pub fn item(locator: &str, bytes: &[u8]) -> SourceItem {
    SourceItem {
        locator: locator.to_owned(),
        blob: Blob::from(bytes.to_vec()),
        modified: None,
    }
}

pub fn ran(rec: &FileRecord) -> Vec<String> {
    rec.facts_for("mock.ran")
        .filter_map(|f| match &f.value {
            Value::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect()
}

pub fn problems(rec: &FileRecord) -> &[String] {
    match &rec.status {
        Status::Partial { problems } => problems,
        other => panic!("expected partial, got {other:?}"),
    }
}

pub fn failed(rec: &FileRecord) -> &str {
    match &rec.status {
        Status::Failed { error } => error,
        other => panic!("expected failed, got {other:?}"),
    }
}

// ---------- run() fixtures ----------

#[derive(Default)]
pub struct Live {
    pub now: AtomicUsize,
    pub max: AtomicUsize,
}

impl Live {
    pub fn max(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }
    pub fn now(&self) -> usize {
        self.now.load(Ordering::SeqCst)
    }
}

/// Bytes that count how many of their kind are alive.
pub struct Counted {
    bytes: Vec<u8>,
    live: Arc<Live>,
}

impl Counted {
    pub fn new(bytes: Vec<u8>, live: &Arc<Live>) -> Self {
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

/// What the source does with item `i`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    Error,
    LocatedError,
    Panic,
}

pub struct MockSource {
    pub next: usize,
    /// `usize::MAX` for an endless source.
    pub total: usize,
    pub live: Arc<Live>,
    /// Size of each blob; `0` means a short text line.
    pub size: usize,
    pub fault: fn(usize) -> Fault,
}

impl MockSource {
    pub fn new(total: usize) -> Self {
        Self {
            next: 0,
            total,
            live: Arc::new(Live::default()),
            size: 0,
            fault: |_| Fault::None,
        }
    }
}

impl Source for MockSource {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        if self.next >= self.total {
            return None;
        }
        let i = self.next;
        self.next += 1;
        let locator = format!("mem/{i}");
        match (self.fault)(i) {
            Fault::None => {}
            Fault::Error => return Some(Err(Error::Other("unreadable".into()))),
            Fault::LocatedError => {
                return Some(Err(Error::at(
                    locator,
                    Error::Other("permission denied".into()),
                )));
            }
            Fault::Panic => panic!("source bug"),
        }
        let bytes = if self.size == 0 {
            format!("file number {i}\n").into_bytes()
        } else {
            vec![b'a'; self.size]
        };
        Some(Ok(SourceItem {
            locator,
            blob: Blob::new(Counted::new(bytes, &self.live)),
            modified: None,
        }))
    }
}

#[derive(Default)]
pub struct VecSink {
    pub records: Vec<FileRecord>,
    pub finished: bool,
}

impl VecSink {
    pub fn locators(&self) -> Vec<&str> {
        self.records.iter().map(|r| r.locator.as_str()).collect()
    }
}

impl RecordSink for VecSink {
    fn write(&mut self, record: &FileRecord) -> Result<()> {
        self.records.push(record.clone());
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.finished = true;
        Ok(())
    }
}

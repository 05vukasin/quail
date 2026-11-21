//! The fuzz target: the whole engine (every extractor and analyzer quail ships, as in
//! `quail-cli`'s registry) plus a page render when the extractor provides a raster.
//!
//! Every step is wrapped so it announces itself on stderr (`QF-STEP <id>`) before it runs:
//! when the process dies (allocation failure, stack overflow, a kill on timeout), the last
//! line names the step that killed it.

use std::collections::BTreeSet;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, AnalyzerId, Blob, Block, BlockKind, Ctx, Document, Emitter, Extractor, FileRecord,
    Format, Limits, Page, PageKind, Profile, Raster, Result, SourceItem, Span, Status, Zone,
};
use quail_engine::{DEFAULT_STACK_SIZE, Engine, EngineBuilder};

/// Panic locations seen while running the current case.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A source path without the machine-specific prefix: `hayro-0.9.0/src/x.rs` for a
/// registry crate, `crates/...` or `vendor/...` for workspace files.
pub fn short_path(p: &str) -> &str {
    if let Some(i) = p.find("/registry/src/") {
        let rest = &p[i + 14..];
        return rest.find('/').map_or(rest, |j| &rest[j + 1..]);
    }
    for marker in ["/crates/", "/vendor/", "/rustlib/src/rust/"] {
        if let Some(i) = p.rfind(marker) {
            return &p[i + 1..];
        }
    }
    p
}

/// Installs a panic hook that records `file:line` of every panic (caught or not) and prints
/// one line per panic to stderr, so even a panic that turns into an abort leaves its location.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map_or_else(
            || "?".to_owned(),
            |l| format!("{}:{}", short_path(l.file()), l.line()),
        );
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let msg: String = msg.chars().take(200).collect();
        let _ = writeln!(std::io::stderr(), "QF-PANIC {loc} {msg}");
        if std::env::var_os("QUAIL_FUZZ_BACKTRACE").is_some() {
            let _ = writeln!(
                std::io::stderr(),
                "{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        if let Ok(mut p) = PANICS.lock() {
            p.push(loc);
        }
    }));
}

fn announce(what: &str) {
    let _ = writeln!(std::io::stderr(), "QF-STEP {what}");
}

/// An extractor that announces itself.
struct TracedExtractor<E>(E);

impl<E: Extractor> Extractor for TracedExtractor<E> {
    fn id(&self) -> AnalyzerId {
        self.0.id()
    }
    fn formats(&self) -> &'static [Format] {
        self.0.formats()
    }
    fn provides(&self) -> &'static [&'static str] {
        self.0.provides()
    }
    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        announce(&self.0.id().to_string());
        self.0.extract(blob, limits, out)
    }
}

/// An analyzer that announces itself.
struct Traced<A>(A);

impl<A: Analyzer> Analyzer for Traced<A> {
    fn id(&self) -> AnalyzerId {
        self.0.id()
    }
    fn requires(&self) -> &'static [&'static str] {
        self.0.requires()
    }
    fn provides(&self) -> &'static [&'static str] {
        self.0.provides()
    }
    fn profile(&self) -> Profile {
        self.0.profile()
    }
    fn applies(&self, ctx: &Ctx<'_>) -> bool {
        self.0.applies(ctx)
    }
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        announce(&self.0.id().to_string());
        self.0.run(ctx, out)
    }
}

/// Plain text as a document, so the text analyzers see the tricky-Unicode inputs (quail has
/// no text extractor of its own yet). Pages split at form feeds, paragraphs at blank lines,
/// `|` lines become table cells and `#` lines headings.
pub struct PlainText;

impl PlainText {
    /// Extractor id (fuzz-only).
    pub const ID: AnalyzerId = AnalyzerId::new("fuzz.text", 1);
}

fn decode(bytes: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

impl Extractor for PlainText {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::Text]
    }
    #[allow(clippy::cast_possible_truncation)]
    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        let mut text = decode(blob.as_bytes());
        let max = usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX);
        if text.len() > max {
            let mut cut = max;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            out.problem("text over max_text_bytes");
        }
        let mut pages = Vec::new();
        for (i, chunk) in text.split('\u{c}').enumerate() {
            out.check_deadline()?;
            if i >= limits.max_pages as usize {
                out.problem("pages over max_pages");
                break;
            }
            let mut page = Page {
                text: chunk.to_owned(),
                ..Page::new(i as u32, PageKind::Logical)
            };
            let mut start = 0usize;
            let mut table = 0u16;
            for line in chunk.split_inclusive('\n') {
                let end = start + line.len();
                let t = line.trim();
                if !t.is_empty() {
                    let kind = if t.starts_with('#') {
                        BlockKind::Heading {
                            level: t.chars().take_while(|&c| c == '#').count().min(9) as u8,
                        }
                    } else if t.starts_with('|') {
                        table = table.saturating_add(1);
                        BlockKind::TableCell {
                            table: 0,
                            row: table,
                            col: 0,
                        }
                    } else {
                        BlockKind::Paragraph
                    };
                    page.blocks.push(Block::new(
                        Span {
                            start: start as u32,
                            end: end as u32,
                        },
                        kind,
                        if i == 0 && start == 0 {
                            Zone::Title
                        } else {
                            Zone::Body
                        },
                        None,
                    ));
                }
                start = end;
            }
            pages.push(page);
        }
        Ok(Document { pages })
    }
}

/// What one case did, as the worker reports it.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// `ok`, `partial` or `failed`.
    pub status: String,
    /// The step that hit the cooperative deadline, if one did.
    pub deadline: Option<String>,
    /// Locations of caught panics.
    pub panics: Vec<String>,
    /// Peak resident memory of the process during the case, KiB (0 when unknown).
    pub peak_kb: u64,
    /// Wall time, milliseconds.
    pub ms: u64,
    /// The slowest step.
    pub slowest: String,
    /// Small hashes of what the case produced (warnings, problems, keys), for corpus feedback.
    pub features: Vec<u32>,
}

impl Report {
    /// One tab-separated line (no tabs or newlines inside fields).
    pub fn to_line(&self) -> String {
        let feats: Vec<String> = self.features.iter().map(|f| format!("{f:x}")).collect();
        format!(
            "R\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.status,
            self.deadline.as_deref().unwrap_or("-"),
            if self.panics.is_empty() {
                "-".to_owned()
            } else {
                self.panics.join(";")
            },
            self.peak_kb,
            self.ms,
            if self.slowest.is_empty() {
                "-"
            } else {
                &self.slowest
            },
            feats.join(",")
        )
    }

    /// Parses [`to_line`](Self::to_line).
    pub fn parse(line: &str) -> Option<Self> {
        let f: Vec<&str> = line.trim_end_matches(['\n', '\r']).split('\t').collect();
        if f.len() < 8 || f[0] != "R" {
            return None;
        }
        Some(Self {
            status: f[1].to_owned(),
            deadline: (f[2] != "-").then(|| f[2].to_owned()),
            panics: if f[3] == "-" {
                Vec::new()
            } else {
                f[3].split(';').map(str::to_owned).collect()
            },
            peak_kb: f[4].parse().ok()?,
            ms: f[5].parse().ok()?,
            slowest: f[6].to_owned(),
            features: f[7]
                .split(',')
                .filter_map(|x| u32::from_str_radix(x, 16).ok())
                .collect(),
        })
    }
}

fn h32(s: &str) -> u32 {
    // FNV-1a
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A message with digits and quoted parts squashed, so the same kind of note hashes the same.
fn squash(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(80));
    let mut last_digit = false;
    for c in s.chars().take(80) {
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('#');
            }
            last_digit = true;
        } else {
            out.push(c);
            last_digit = false;
        }
    }
    out
}

/// Clears the peak-RSS counter of this process (Linux; a no-op elsewhere).
pub fn reset_peak() {
    #[cfg(target_os = "linux")]
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// Peak resident memory since the last [`reset_peak`], KiB (Linux; 0 elsewhere).
pub fn peak_kb() -> u64 {
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        for l in s.lines() {
            if let Some(v) = l.strip_prefix("VmHWM:") {
                return v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

/// The engine as the fuzzer runs it.
pub struct Target {
    engine: Engine,
    render: bool,
}

impl Target {
    /// Every extractor and analyzer of the build, `Deep` profile, `deadline` per file.
    pub fn new(deadline: Duration, render: bool) -> Self {
        let limits = Limits {
            timeout: deadline,
            ..Limits::default()
        };
        let b = EngineBuilder::new()
            .profile(Profile::Deep)
            .limits(limits)
            .quiet_panics(true)
            .analyzer(Box::new(Traced(quail_hash::IdentityAnalyzer)))
            .extractor(Box::new(TracedExtractor(quail_docx::DocxExtractor::new())))
            .extractor(Box::new(TracedExtractor(PlainText)))
            .analyzer(Box::new(Traced(quail_text::TextStats)))
            .analyzer(Box::new(Traced(quail_text::TextLang::new())))
            .analyzer(Box::new(Traced(quail_signals::SignalScan)))
            .analyzer(Box::new(Traced(quail_refs::RefAnalyzer::new())))
            .analyzer(Box::new(Traced(quail_parties::PartyCandidates)))
            .extractor(Box::new(TracedExtractor(quail_pdf::PdfExtractor::new())))
            .analyzer(Box::new(Traced(quail_sketch::MinHashAnalyzer)))
            .analyzer(Box::new(Traced(quail_sketch::SimHashText)))
            .analyzer(Box::new(Traced(quail_sketch::SimHashTemplate)))
            .analyzer(Box::new(Traced(quail_sketch::LayoutAnalyzer)))
            .analyzer(Box::new(Traced(quail_sketch::InvoiceLayout)))
            .analyzer(Box::new(Traced(quail_sketch::TextKey)))
            .analyzer(Box::new(Traced(quail_sketch::CompatMinHash)))
            .analyzer(Box::new(Traced(quail_sketch::PagePHash)));
        // quail's own panic hook would hide panic locations; ours is installed afterwards.
        let engine = match b.build() {
            Ok(e) => e,
            Err(e) => panic!("engine does not build: {e}"),
        };
        install_panic_hook();
        Self { engine, render }
    }

    /// Runs one input on a worker-sized stack (as `Engine::run` does) and reports.
    pub fn run(&self, data: Vec<u8>) -> Report {
        if let Ok(mut p) = PANICS.lock() {
            p.clear();
        }
        reset_peak();
        let start = Instant::now();
        let record = std::thread::scope(|s| {
            let h = std::thread::Builder::new()
                .stack_size(DEFAULT_STACK_SIZE)
                .spawn_scoped(s, || self.process(data));
            match h {
                Ok(h) => h.join().ok(),
                Err(_) => None,
            }
        });
        let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut report = record.map_or_else(
            || Report {
                status: "harness-panic".to_owned(),
                ..Report::default()
            },
            |r| describe(&r),
        );
        report.ms = ms;
        report.peak_kb = peak_kb();
        if let Ok(p) = PANICS.lock() {
            report.panics.clone_from(&p);
        }
        report
    }

    fn process(&self, data: Vec<u8>) -> FileRecord {
        let item = SourceItem {
            locator: "case".into(),
            blob: Blob::from(data),
            modified: None,
        };
        let render = self.render;
        let limits = self.engine.limits().clone();
        self.engine.process_with(item, |_, artifacts| {
            if !render {
                return;
            }
            if let Some(Raster(r)) = artifacts.get::<Raster>() {
                announce("render");
                let r = r.clone();
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    for page in 0..2 {
                        let _ = r.render(page, 36.0, &limits);
                        let _ = r.embedded_scan(page, &limits);
                    }
                }));
            }
        })
    }
}

/// The report of a record (without timing and memory, filled in by the caller).
pub fn describe(r: &FileRecord) -> Report {
    let mut feats = BTreeSet::new();
    feats.insert(h32(r.format.as_str()));
    let mut deadline = None;
    let (status, msgs): (&str, Vec<&String>) = match &r.status {
        Status::Ok => ("ok", Vec::new()),
        Status::Partial { problems } => ("partial", problems.iter().collect()),
        Status::Failed { error } => ("failed", vec![error]),
    };
    feats.insert(h32(status));
    for m in msgs.iter().copied().chain(r.warnings.iter()) {
        feats.insert(h32(&squash(m)));
        if deadline.is_none() && m.contains("timeout") {
            deadline = Some(m.split(':').next().unwrap_or("?").trim().to_owned());
        }
    }
    for f in &r.facts {
        feats.insert(h32(f.key.as_str()));
    }
    if let Some(p) = r.pages {
        feats.insert(h32(&format!("pages{}", u32::BITS - p.leading_zeros())));
    }
    let slowest = r
        .timings
        .iter()
        .max_by_key(|t| t.micros)
        .map_or_else(|| "-".to_owned(), |t| t.src.to_string());
    Report {
        status: status.to_owned(),
        deadline,
        slowest,
        features: feats.into_iter().collect(),
        ..Report::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_round_trips() {
        let r = Report {
            status: "partial".into(),
            deadline: Some("docx.extract@1".into()),
            panics: vec!["a.rs:1".into(), "b.rs:2".into()],
            peak_kb: 12,
            ms: 3,
            slowest: "text.lang@1".into(),
            features: vec![1, 0xffff_ffff],
        };
        let back = Report::parse(&r.to_line()).unwrap();
        assert_eq!(back.status, r.status);
        assert_eq!(back.deadline, r.deadline);
        assert_eq!(back.panics, r.panics);
        assert_eq!(back.features, r.features);
        assert_eq!(squash("page 12 of 345"), "page # of #");
        assert_eq!(
            short_path("/home/u/.cargo/registry/src/index.crates.io-abc/hayro-0.9.0/src/a.rs"),
            "hayro-0.9.0/src/a.rs"
        );
        assert_eq!(
            short_path("/w/q/crates/quail-pdf/src/b.rs"),
            "crates/quail-pdf/src/b.rs"
        );
        assert_eq!(short_path("vendor/x/src/c.rs"), "vendor/x/src/c.rs");
    }

    #[test]
    fn plain_text_blocks_cover_the_lines() {
        let blob = Blob::from("# T\nline one\n\n| a |\n\u{c}page two".as_bytes().to_vec());
        let mut out = Emitter::new(PlainText::ID);
        let doc = PlainText
            .extract(&blob, &Limits::default(), &mut out)
            .unwrap();
        assert_eq!(doc.pages.len(), 2);
        let p = &doc.pages[0];
        let texts: Vec<&str> = p
            .blocks
            .iter()
            .map(|b| b.span.slice(&p.text).trim())
            .collect();
        assert_eq!(texts, ["# T", "line one", "| a |"]);
        assert_eq!(decode(&[0xff, 0xfe, b'h', 0, b'i', 0]), "hi");
    }
}

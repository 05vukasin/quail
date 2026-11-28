//! Measurement harness (#142): peak memory of every step on 64 MiB of generated text.
#![allow(unsafe_code, clippy::cast_precision_loss, clippy::missing_panics_doc)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, Format, Limits, Profile, Result,
    SourceItem,
};
use quail_engine::EngineBuilder;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

struct Counting;

// SAFETY: forwards to `System` unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            track(layout.size().cast_signed());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            track(layout.size().cast_signed());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-layout.size().cast_signed());
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            track(new_size.cast_signed() - layout.size().cast_signed());
        }
        p
    }
}

fn track(delta: isize) {
    let now = LIVE.fetch_add(delta, Ordering::Relaxed) + delta;
    PEAK.fetch_max(now, Ordering::Relaxed);
    let t = BT.load(Ordering::Relaxed);
    if t > 0 && delta >= t {
        let _ = INSIDE.try_with(|i| {
            if !i.get() {
                i.set(true);
                let bt = std::backtrace::Backtrace::force_capture().to_string();
                let mut lines = vec![format!("BIG {} MiB live {} MiB", delta >> 20, now >> 20)];
                let mut prev = String::new();
                for l in bt.lines() {
                    let l = l.trim();
                    if l.starts_with("at ") && l.contains("/crates/") && !l.contains("text_memory") {
                        if prev.len() > 0 { lines.push(format!("   {prev} {l}")); }
                    }
                    prev = l.to_string();
                }
                eprintln!("{}", lines.join("\n"));
                i.set(false);
            }
        });
    }
}

static BT: AtomicIsize = AtomicIsize::new(0);
thread_local! { static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[global_allocator]
static ALLOC: Counting = Counting;

static RESULTS: Mutex<Vec<(String, f64, f64)>> = Mutex::new(Vec::new());

fn measure<T>(name: String, f: impl FnOnce() -> T) -> T {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let r = f();
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let secs = t.elapsed().as_secs_f64();
    RESULTS
        .lock()
        .unwrap()
        .push((name, peak as f64 / f64::from(1 << 20), secs));
    r
}

struct Peak<A>(A);

impl<A: Analyzer> Analyzer for Peak<A> {
    fn id(&self) -> AnalyzerId {
        self.0.id()
    }
    fn requires(&self) -> &'static [&'static str] {
        self.0.requires()
    }
    fn provides(&self) -> &'static [&'static str] {
        self.0.provides()
    }
    fn after(&self) -> &'static [&'static str] {
        self.0.after()
    }
    fn finalizer(&self) -> bool {
        self.0.finalizer()
    }
    fn profile(&self) -> Profile {
        self.0.profile()
    }
    fn applies(&self, ctx: &Ctx<'_>) -> bool {
        self.0.applies(ctx)
    }
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        measure(self.0.id().to_string(), || self.0.run(ctx, out))
    }
}

struct PeakX<E>(E);

impl<E: Extractor> Extractor for PeakX<E> {
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
        measure(self.0.id().to_string(), || self.0.extract(blob, limits, out))
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// About `bytes` of contract-like text where nothing repeats: pseudo-words, distinct dates,
/// amounts, ids and names, in lines and paragraphs.
fn text(bytes: usize, seed: u64) -> String {
    const SYL: &[&str] = &[
        "ka", "lo", "vi", "ne", "ra", "go", "ze", "mi", "tu", "pre", "do", "sta", "ni", "ju",
        "ugo", "vor", "ba", "še", "ći", "đa", "ž", "ко", "на", "ру",
    ];
    let mut r = Rng(seed | 1);
    let mut s = String::with_capacity(bytes + 256);
    let mut line = 0usize;
    while s.len() < bytes {
        match r.below(20) {
            0 => {
                let n = r.below(1 << 40);
                s.push_str(&format!("{}.{:02},{:02} RSD", n % 1_000_000, n % 97, n % 89));
            }
            1 => {
                let n = r.below(1 << 30);
                s.push_str(&format!(
                    "{:02}.{:02}.{}.",
                    1 + n % 28,
                    1 + (n >> 5) % 12,
                    1990 + (n >> 9) % 40
                ));
            }
            2 => s.push_str(&format!("PIB {}", 100_000_000 + r.below(899_999_999))),
            3 => {
                let n = r.next();
                s.push_str(&format!("RS35{:018}", n % 1_000_000_000_000_000_000));
            }
            4 => s.push_str(&format!("{}%", r.below(10_000) as f64 / 100.0)),
            5 => {
                let n = r.below(100_000);
                s.push_str(&format!("član {n}."));
            }
            _ => {
                let k = 1 + r.below(4);
                let cap = r.below(6) == 0;
                for i in 0..k {
                    let syl = SYL[usize::try_from(r.below(SYL.len() as u64)).unwrap()];
                    if cap && i == 0 {
                        let mut c = syl.chars();
                        let f = c.next().unwrap();
                        s.extend(f.to_uppercase());
                        s.push_str(c.as_str());
                    } else {
                        s.push_str(syl);
                    }
                }
                if r.below(8) == 0 {
                    s.push_str(&format!("{}", r.below(1000)));
                }
            }
        }
        line += 1;
        match r.below(14) {
            0 => s.push_str(". "),
            1 => s.push_str(", "),
            _ => s.push(' '),
        }
        if line > 8 + usize::try_from(r.below(8)).unwrap() {
            line = 0;
            s.push('\n');
            if r.below(6) == 0 {
                s.push('\n');
            }
        }
    }
    s
}

fn engine(profile: Profile) -> quail_engine::Engine {
    EngineBuilder::new()
        .profile(profile)
        .limits(Limits {
            timeout: std::time::Duration::from_hours(1),
            max_timeout: std::time::Duration::from_hours(1),
            ..Limits::default()
        })
        .quiet_panics(true)
        .extractor(Box::new(PeakX(quail_text::TextExtractor::new())))
        .analyzer(Box::new(Peak(quail_hash::IdentityAnalyzer)))
        .analyzer(Box::new(Peak(quail_text::TextStats)))
        .analyzer(Box::new(Peak(quail_text::TextLang::new())))
        .analyzer(Box::new(Peak(quail_signals::SignalScan)))
        .analyzer(Box::new(Peak(quail_refs::RefAnalyzer::new())))
        .analyzer(Box::new(Peak(quail_parties::PartyCandidates)))
        .analyzer(Box::new(Peak(quail_clauses::ClauseTerms)))
        .analyzer(Box::new(Peak(quail_sketch::MinHashAnalyzer)))
        .analyzer(Box::new(Peak(quail_sketch::SimHashText)))
        .analyzer(Box::new(Peak(quail_sketch::SimHashTemplate)))
        .analyzer(Box::new(Peak(quail_sketch::LayoutAnalyzer)))
        .analyzer(Box::new(Peak(quail_sketch::InvoiceLayout)))
        .analyzer(Box::new(Peak(quail_sketch::TextKey)))
        .analyzer(Box::new(Peak(quail_sketch::CompatMinHash)))
        .analyzer(Box::new(Peak(quail_items::ItemLines::new())))
        .analyzer(Box::new(Peak(quail_payment::PaymentExtract)))
        .analyzer(Box::new(Peak(quail_contract::ContractFacts)))
        .analyzer(Box::new(Peak(quail_card::KeyFactsCard)))
        .analyzer(Box::new(Peak(quail_summary::SummaryCard::new())))
        .analyzer(Box::new(Peak(quail_summary::SummaryExtractive)))
        .analyzer(Box::new(Peak(quail_summary::SummaryOutline::new())))
        .analyzer(Box::new(Peak(quail_summary::SummaryKeywords::new())))
        .analyzer(Box::new(Peak(quail_summary::SummaryTopics::new())))
        .build()
        .unwrap()
}

#[test]
#[ignore = "measurement"]
fn measure_steps_on_64_mib() {
    let mib: usize = std::env::var("MEASURE_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let e = engine(Profile::Deep);
    if let Some(t) = std::env::var("MEASURE_BT").ok().and_then(|v| v.parse::<isize>().ok()) {
        BT.store(t << 20, Ordering::Relaxed);
    }
    let body = text(mib << 20, 142);
    let rec = e.process(SourceItem {
        locator: "big.txt".into(),
        blob: Blob::from(body.into_bytes()),
        modified: None,
    });
    for (name, mib, secs) in RESULTS.lock().unwrap().iter() {
        eprintln!("PEAK {name:32} {mib:9.1} MiB {secs:7.2} s");
    }
    if let Ok(path) = std::env::var("MEASURE_OUT") {
        std::fs::write(path, serde_json::to_string_pretty(&rec).unwrap()).unwrap();
    }
    eprintln!("steps {:?}", rec.steps.len());
}

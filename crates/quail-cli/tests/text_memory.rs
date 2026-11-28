//! Peak memory of every text step on a page at the `max_text_bytes` limit (quail #142).
//!
//! A file whose text reaches the 64 MiB limit, with nothing repeated, used to push the text
//! analyzers past 1 GiB (`oom-abort@signal.scan@5` on a 280 KB PPTX): per-token buffers of
//! 16 to 48 bytes a token, several copies of the text, a `Vec` of every 3-word window. Each
//! step is now measured on such a text, generated here, with a thread-local allocation
//! counter, and must stay within its bound.
//!
//! The counting allocator is the only `unsafe` here: it is test-only and forwards to the
//! system allocator unchanged.
#![allow(unsafe_code, clippy::cast_precision_loss, clippy::missing_panics_doc)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::time::Duration;

use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, Format, Limits, Profile, Result,
    SourceItem,
};
use quail_engine::EngineBuilder;

// ---------------------------------------------------------------------------------------------
// Peak-allocation measurement.

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
    static PEAKS: RefCell<Vec<(String, usize)>> = const { RefCell::new(Vec::new()) };
}

struct Counting;

// SAFETY: every method forwards to `System` with the caller's arguments unchanged; the only
// extra work is updating const-initialized thread-local counters, which never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `GlobalAlloc::alloc`'s contract.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            track(layout.size().cast_signed());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds the contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            track(layout.size().cast_signed());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-layout.size().cast_signed());
        // SAFETY: `ptr` was allocated by `System` with this layout (all allocations go
        // through this type).
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `GlobalAlloc::realloc`'s contract.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            track(new_size.cast_signed() - layout.size().cast_signed());
        }
        p
    }
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|l| {
        let now = l.get() + delta;
        l.set(now);
        let _ = PEAK.try_with(|p| {
            if now > p.get() {
                p.set(now);
            }
        });
    });
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f` and returns its result with the peak extra bytes this thread held meanwhile.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.with(Cell::get);
    let outer = PEAK.with(Cell::get);
    PEAK.with(|p| p.set(base));
    let r = f();
    let peak = PEAK.with(Cell::get);
    // An enclosing measurement still sees this peak.
    PEAK.with(|p| p.set(outer.max(peak)));
    (r, usize::try_from(peak - base).unwrap_or(0))
}

const MIB: usize = 1 << 20;

// ---------------------------------------------------------------------------------------------
// Steps that record their peak.

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
        let (r, peak) = peak_of(|| self.0.run(ctx, out));
        PEAKS.with(|p| p.borrow_mut().push((self.0.id().name.to_owned(), peak)));
        r
    }
}

struct PeakExtractor<E>(E);

impl<E: Extractor> Extractor for PeakExtractor<E> {
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
        let (r, peak) = peak_of(|| self.0.extract(blob, limits, out));
        PEAKS.with(|p| p.borrow_mut().push((self.0.id().name.to_owned(), peak)));
        r
    }
}

/// Every text step of the default build, each recording its peak.
fn engine() -> quail_engine::Engine {
    EngineBuilder::new()
        .profile(Profile::Deep)
        .limits(Limits {
            // Debug builds are slow; the deadline is not what is tested.
            timeout: Duration::from_hours(1),
            max_timeout: Duration::from_hours(1),
            ..Limits::default()
        })
        .quiet_panics(true)
        .extractor(Box::new(PeakExtractor(quail_text::TextExtractor::new())))
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

// ---------------------------------------------------------------------------------------------
// Input.

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
}

/// About `bytes` of contract-like text in which almost nothing repeats: pseudo-words in
/// Latin and Cyrillic, distinct amounts, dates, tax ids, IBANs, percentages and article
/// numbers, in lines of 9 to 16 items and paragraphs. One page (no form feed).
fn text(bytes: usize, seed: u64) -> String {
    const SYL: &[&str] = &[
        "ka", "lo", "vi", "ne", "ra", "go", "ze", "mi", "tu", "pre", "do", "sta", "ni", "ju",
        "ugo", "vor", "ba", "še", "ći", "đa", "ž", "ко", "на", "ру",
    ];
    let mut r = Rng(seed | 1);
    let mut s = String::with_capacity(bytes + 256);
    let mut items = 0;
    while s.len() < bytes {
        let n = r.next();
        let _ = match r.below(20) {
            0 => write!(s, "{}.{:02},{:02} RSD", n % 1_000_000, n % 97, n % 89),
            1 => write!(
                s,
                "{:02}.{:02}.{}.",
                1 + n % 28,
                1 + (n >> 5) % 12,
                1990 + (n >> 9) % 40
            ),
            2 => write!(s, "PIB {}", 100_000_000 + n % 899_999_999),
            3 => write!(s, "RS35{:018}", n % 1_000_000_000_000_000_000),
            4 => write!(s, "{}%", (n % 10_000) as f64 / 100.0),
            5 => write!(s, "član {}.", n % 100_000),
            _ => {
                let cap = r.below(6) == 0;
                for i in 0..=r.below(4) {
                    let syl = SYL[r.below(SYL.len())];
                    if cap && i == 0 {
                        let mut c = syl.chars();
                        s.extend(c.next().into_iter().flat_map(char::to_uppercase));
                        s.push_str(c.as_str());
                    } else {
                        s.push_str(syl);
                    }
                }
                if r.below(8) == 0 {
                    let _ = write!(s, "{}", n % 1000);
                }
                Ok(())
            }
        };
        s.push_str(match r.below(14) {
            0 => ". ",
            1 => ", ",
            _ => " ",
        });
        items += 1;
        if items > 8 + r.below(8) {
            items = 0;
            s.push_str(if r.below(6) == 0 { "\n\n" } else { "\n" });
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------

/// Peak bound of each text step on one page of 64 MiB of text, measured in an optimized
/// build before and after #142 (MiB): `signal.scan` 711 → 49, `sketch.minhash_compat`
/// 665 → 74, `ref.references` 579 → 83, `sketch.invoice_layout` 319 → 9, `text.stats`
/// 315 → 8, `party.candidates` 252 → 63, `clause.terms` 174 → 96, `summary.keywords`
/// 170 → 63, `sketch.minhash` and `sketch.simhash_text` 128 → 84, `item.lines` 119 → 56.
/// `payment.extract` (84) and `contract.facts` (160) read at most 8 MiB and were bounded
/// already. Every bound has headroom over the measured peak.
const BOUNDS: &[(&str, usize)] = &[
    ("text.extract", 128),
    ("hash.identity", 16),
    ("text.stats", 32),
    ("text.lang", 16),
    ("signal.scan", 96),
    ("ref.references", 128),
    ("party.candidates", 96),
    ("clause.terms", 128),
    ("sketch.minhash", 128),
    ("sketch.simhash_text", 128),
    ("sketch.simhash_template", 64),
    ("sketch.layout", 16),
    ("sketch.invoice_layout", 32),
    ("sketch.text_key", 48),
    ("sketch.minhash_compat", 128),
    ("item.lines", 96),
    ("payment.extract", 128),
    ("contract.facts", 192),
    ("card.facts", 16),
    ("summary.card", 16),
    ("summary.extractive", 16),
    ("summary.outline", 64),
    ("summary.keywords", 96),
    ("summary.topics", 16),
];

#[test]
fn every_text_step_is_bounded_at_the_text_limit() {
    let limit = usize::try_from(Limits::default().max_text_bytes).unwrap();
    let body = text(limit, 142);
    assert!(body.len() >= limit);
    let e = engine();
    PEAKS.with(|p| p.borrow_mut().clear());
    let (rec, total) = peak_of(|| {
        e.process(SourceItem {
            locator: "big.txt".into(),
            blob: Blob::from(body.into_bytes()),
            modified: None,
        })
    });
    let peaks = PEAKS.with(|p| p.borrow().clone());
    let mut report = String::new();
    for (name, peak) in &peaks {
        let _ = writeln!(report, "{name:28} {:7.1} MiB", *peak as f64 / MIB as f64);
    }
    eprintln!("{report}whole file {:.1} MiB", total as f64 / MIB as f64);
    for (name, bound) in BOUNDS {
        let Some((_, peak)) = peaks.iter().find(|(n, _)| n == name) else {
            panic!("{name} did not run\n{report}");
        };
        assert!(
            *peak < bound * MIB,
            "{name}: peak {} MiB, bound {bound} MiB\n{report}",
            peak / MIB
        );
    }
    // The text (64 MiB) and its document are held all along; the steps add little to them.
    assert!(total < 384 * MIB, "whole file {} MiB", total / MIB);
    // The 64 MiB page was read, not skipped: the sketches and signals found something.
    let has = |key: &str| rec.facts.iter().any(|f| f.key.as_str() == key);
    for key in [
        "signal.date",
        "sketch.minhash",
        "sketch.compat_minhash",
        "sketch.compat_template",
        "sketch.text_key",
    ] {
        assert!(has(key), "no {key}");
    }
}

//! The `signal.scan@4` analyzer: runs every family over each page and collects candidates.

use std::collections::HashMap;

use quail_core::{
    Analyzer, AnalyzerId, Block, Ctx, Document, Emitter, Evidence, FactKey, Profile, Result, Span,
    Value,
};

use crate::conf;
use crate::date::{self, DateCand};
use crate::ident::IdentScan;
use crate::keys;
use crate::page::PageText;
use crate::{contact, money};

/// Distinct values kept per key and file; more are dropped with a problem.
pub const MAX_PER_KEY: usize = 200;

/// Dates buffered per file before the day/month order is resolved.
const MAX_DATES: usize = 20 * MAX_PER_KEY;

/// One candidate found on a page; `span` is in the normalized page text.
#[derive(Clone, Debug)]
pub(crate) struct Cand {
    pub key: FactKey,
    pub value: Value,
    pub conf: f32,
    pub span: Span,
}

impl Cand {
    pub fn new(key: FactKey, value: Value, conf: f32, span: Span) -> Self {
        Self {
            key,
            value,
            conf,
            span,
        }
    }
}

/// Words (folded) that mark a page as Serbian business text. They let a bare 9-digit
/// number with a valid PIB checksum through (at low confidence) and decide how a domestic
/// phone number is read.
const SERBIAN_MARKERS: &[&str] = &[
    "pib",
    "maticni",
    "pdv",
    "rsd",
    "dinara",
    "ugovor",
    "ugovora",
    "jmbg",
    "tekuci",
    "ziro",
    "ukupno",
    "racun",
    "racuna",
    "dobavljac",
    "kupac",
    "narucilac",
];

/// Rule-based candidates for formatted business data: identifiers with check digits,
/// dates, amounts, contacts. See the crate documentation for keys and confidences.
///
/// Requires the `document` artifact; runs in `Fast`. Reads at most
/// `Limits::max_text_bytes` of text, keeps at most [`MAX_PER_KEY`] distinct values per key
/// (one fact per distinct value, with the evidence of its most confident occurrence) and
/// records a problem when either limit or the deadline stops it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SignalScan;

impl SignalScan {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("signal.scan", 4);
}

impl Analyzer for SignalScan {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let budget = usize::try_from(ctx.limits.max_text_bytes).unwrap_or(usize::MAX);
        scan_document(doc, budget, out);
        Ok(())
    }
}

/// Collects facts: one per distinct (key, value), best confidence wins, capped per key.
#[derive(Default)]
struct Collector {
    /// `key \0 value` → index in `facts`.
    index: HashMap<String, usize>,
    facts: Vec<(FactKey, Value, f32, Evidence)>,
    per_key: HashMap<FactKey, usize>,
    capped: Vec<FactKey>,
    /// Reused buffer for the lookup id, so repeated values allocate nothing.
    id: String,
}

impl Collector {
    fn add(&mut self, key: FactKey, value: Value, conf: f32, ev: Evidence) {
        use std::fmt::Write as _;
        self.id.clear();
        let _ = write!(self.id, "{key}\0{value:?}");
        if let Some(&i) = self.index.get(self.id.as_str()) {
            let f = &mut self.facts[i];
            if conf > f.2 {
                f.2 = conf;
                f.3 = ev;
            }
            return;
        }
        let n = self.per_key.entry(key.clone()).or_insert(0);
        if *n >= MAX_PER_KEY {
            if !self.capped.contains(&key) {
                self.capped.push(key);
            }
            return;
        }
        *n += 1;
        self.index.insert(self.id.clone(), self.facts.len());
        self.facts.push((key, value, conf, ev));
    }

    fn emit(self, out: &mut Emitter) {
        for key in &self.capped {
            out.problem(format!(
                "signal.scan: more than {MAX_PER_KEY} distinct values of {key}, the rest dropped"
            ));
        }
        for (key, value, conf, ev) in self.facts {
            let ev = (ev != Evidence::default()).then_some(ev);
            out.push(key, value, conf, ev);
        }
    }
}

/// The longest prefix of `text` within `budget` bytes, on a char boundary.
fn within(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The block that contains source byte `at`.
fn block_at(blocks: &[Block], sorted: bool, at: u32) -> Option<&Block> {
    if sorted {
        let i = blocks.partition_point(|b| b.span.start <= at);
        let b = blocks.get(i.checked_sub(1)?)?;
        (at < b.span.end || b.span.start == at).then_some(b)
    } else {
        blocks
            .iter()
            .find(|b| b.span.start <= at && at < b.span.end)
    }
}

fn scan_document(doc: &Document, mut budget: usize, out: &mut Emitter) {
    let mut pt = PageText::default();
    let mut col = Collector::default();
    let mut dates: Vec<(DateCand, Evidence)> = Vec::new();
    let mut dates_capped = false;
    let mut jmbg = 0u32;
    let mut serbian = false;
    let mut cands = Vec::new();
    let mut page_dates = Vec::new();
    let mut first_page = true;
    for page in &doc.pages {
        if out.check_deadline().is_err() {
            out.problem(format!(
                "signal.scan: deadline reached, pages from {} not scanned",
                page.index
            ));
            break;
        }
        if budget == 0 {
            out.problem(format!(
                "signal.scan: text limit reached, pages from {} not scanned",
                page.index
            ));
            break;
        }
        let text = within(&page.text, budget);
        budget -= text.len();
        if text.trim().is_empty() {
            continue;
        }
        pt.load(text);
        serbian = serbian || (0..pt.len()).any(|i| SERBIAN_MARKERS.contains(&pt.key(i)));
        cands.clear();
        let mut ids = IdentScan::new(&pt, serbian);
        ids.run(&mut cands);
        jmbg = jmbg.saturating_add(ids.jmbg);
        money::scan(&pt, &mut cands);
        contact::scan(&pt, serbian, &mut cands);
        page_dates.clear();
        date::scan(&pt, first_page, &mut page_dates);
        first_page = false;

        let sorted = page
            .blocks
            .windows(2)
            .all(|w| w[0].span.start <= w[1].span.start);
        let evidence = |sp: Span| {
            let src = pt.map.span_to_source(sp);
            let block = block_at(&page.blocks, sorted, src.start);
            Evidence {
                page: Some(page.index),
                zone: block.map(|b| b.zone).unwrap_or_default(),
                span: Some(src),
                bbox: block.and_then(|b| b.bbox),
            }
        };
        for c in cands.drain(..) {
            let ev = evidence(c.span);
            col.add(c.key, c.value, c.conf, ev);
        }
        for d in page_dates.drain(..) {
            if dates.len() >= MAX_DATES {
                dates_capped = true;
                break;
            }
            let ev = evidence(d.span);
            dates.push((d, ev));
        }
    }
    if dates_capped {
        out.problem(format!(
            "signal.scan: more than {MAX_DATES} dates, the rest not read"
        ));
    }
    let mut ds: Vec<DateCand> = dates.iter().map(|(d, _)| *d).collect();
    date::resolve(&mut ds);
    let early: Vec<bool> = dates
        .iter()
        .map(|(_, ev)| ev.page.is_some_and(|p| p <= 1))
        .collect();
    date::calibrate(&mut ds, &early);
    for (d, (_, ev)) in ds.iter().zip(dates) {
        if let Some((role, rc)) = d.role {
            col.add(role.key(), Value::Date(d.date), d.conf * rc, ev.clone());
        }
        col.add(keys::DATE, Value::Date(d.date), d.conf, ev);
    }
    if jmbg > 0 {
        col.add(
            keys::JMBG_COUNT,
            Value::Int(i64::from(jmbg)),
            conf::JMBG,
            Evidence::default(),
        );
    }
    col.emit(out);
}

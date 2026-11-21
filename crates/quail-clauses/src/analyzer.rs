//! `clause.terms@1`: the rules of [`crate::law`] and [`crate::term`] turned into facts.

use quail_core::{Analyzer, AnalyzerId, Ctx, Document, Emitter, Evidence, Profile, Result, Value};

use crate::keys;
use crate::law::{self, Basis};
use crate::term::{self, Kind};
use crate::tokens::Stream;

/// Values emitted per key (the most confident ones).
pub const MAX_PER_KEY: usize = 4;
/// Text read per document, whatever `Limits::max_text_bytes` allows.
pub const MAX_SCAN_BYTES: usize = 16 << 20;
/// Longest `raw` field, in characters.
pub const MAX_RAW_CHARS: usize = 300;

/// Governing law, term, survival, notice periods and termination rights of a contract. See
/// the crate documentation for the keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClauseTerms;

impl ClauseTerms {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("clause.terms", 1);
}

impl Analyzer for ClauseTerms {
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
        let budget = usize::try_from(ctx.limits.max_text_bytes)
            .unwrap_or(usize::MAX)
            .min(MAX_SCAN_BYTES);
        let (s, stopped) = Stream::read(doc, budget, || out.check_deadline().is_err());
        if stopped {
            out.problem("clause.terms: deadline reached, later pages not read");
        }
        if let Some(p) = s.cut_at {
            out.problem(format!("clause.terms: text limit reached at page {p}"));
        }
        analyze(doc, &s, out);
        Ok(())
    }
}

/// Confidence of a governing-law candidate by what found it.
fn law_conf(b: Basis) -> f32 {
    match b {
        Basis::LawsOf => 0.97,
        Basis::PlaceLaw => 0.9,
        Basis::Forum => 0.6,
        Basis::BareLawsOf => 0.5,
    }
}

fn basis_name(b: Basis) -> &'static str {
    match b {
        Basis::LawsOf => "laws_of",
        Basis::PlaceLaw => "place_law",
        Basis::Forum => "forum",
        Basis::BareLawsOf => "bare_laws_of",
    }
}

/// Facts in the making: one per distinct value, best confidence (and its evidence) wins.
struct Pending {
    /// Dedup id (key and value).
    id: String,
    value: Value,
    conf: f32,
    ev: Option<Evidence>,
}

fn add(list: &mut Vec<Pending>, id: String, value: Value, conf: f32, ev: Option<Evidence>) {
    if let Some(p) = list.iter_mut().find(|p| p.id == id) {
        if conf > p.conf {
            p.conf = conf;
            p.value = value;
            p.ev = ev;
        }
        return;
    }
    list.push(Pending {
        id,
        value,
        conf,
        ev,
    });
}

fn flush(list: Vec<Pending>, key: &quail_core::FactKey, out: &mut Emitter) {
    let mut list = list;
    // Stable: equal confidences keep text order.
    list.sort_by(|a, b| b.conf.total_cmp(&a.conf));
    for p in list.into_iter().take(MAX_PER_KEY) {
        out.push(key.clone(), p.value, p.conf, p.ev);
    }
}

fn raw(doc: &Document, s: &Stream, a: usize, b: usize) -> String {
    let text = s.slice(doc, a, b);
    let mut r: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if r.chars().count() > MAX_RAW_CHARS {
        r = r.chars().take(MAX_RAW_CHARS).collect();
    }
    r
}

fn evidence(s: &Stream, a: usize, b: usize) -> Option<Evidence> {
    let (page, span) = s.span(a, b)?;
    Some(Evidence {
        page: Some(page),
        span: Some(span),
        ..Evidence::default()
    })
}

/// Runs every rule over the stream and emits the facts.
pub(crate) fn analyze(doc: &Document, s: &Stream, out: &mut Emitter) {
    if s.is_empty() {
        return;
    }
    // Governing law. A governing-law clause makes the forum and incorporation statements of
    // other places much less likely to be the answer.
    let hits = law::find(s);
    let governed = hits
        .iter()
        .any(|h| matches!(h.basis, Basis::LawsOf | Basis::PlaceLaw));
    let mut laws = Vec::new();
    for h in &hits {
        let mut c = law_conf(h.basis);
        if governed && matches!(h.basis, Basis::Forum | Basis::BareLawsOf) {
            c *= 0.5;
        }
        let v = Value::record([
            ("jurisdiction", Value::text(h.place.name)),
            ("code", Value::text(h.place.code)),
            ("raw", Value::text(raw(doc, s, h.a, h.b))),
            ("basis", Value::text(basis_name(h.basis))),
        ]);
        add(
            &mut laws,
            h.place.code.to_string(),
            v,
            c,
            evidence(s, h.a, h.b),
        );
    }
    flush(laws, &keys::GOVERNING_LAW, out);

    // Durations.
    let mut terms = Vec::new();
    let mut survival = Vec::new();
    let mut notices = Vec::new();
    for h in term::find(s) {
        let text = h.d.text();
        let ev = evidence(s, h.a, h.b);
        let raw = raw(doc, s, h.a, h.b);
        match h.kind {
            Kind::Term => {
                let renewal = term::renewal(s, h.a, h.b);
                let v = Value::record([
                    ("duration", Value::text(h.d.iso())),
                    ("text", Value::text(text.as_str())),
                    ("raw", Value::text(raw)),
                    ("renewal", Value::text(renewal.as_str())),
                ]);
                add(&mut terms, text, v, h.score, ev);
            }
            Kind::Survival => {
                let v = Value::record([
                    ("duration", Value::text(h.d.iso())),
                    ("text", Value::text(text.as_str())),
                    ("raw", Value::text(raw)),
                ]);
                add(&mut survival, text, v, h.score, ev);
            }
            Kind::Notice => {
                let v = Value::record([
                    ("duration", Value::text(h.d.iso())),
                    ("text", Value::text(text.as_str())),
                    ("raw", Value::text(raw)),
                ]);
                add(&mut notices, text, v, h.score, ev);
            }
        }
    }
    flush(terms, &keys::TERM, out);
    flush(survival, &keys::SURVIVAL, out);
    flush(notices, &keys::NOTICE_PERIOD, out);

    let mut rights = Vec::new();
    for h in term::termination(s) {
        let v = Value::record([
            ("kind", Value::text(h.kind)),
            ("raw", Value::text(raw(doc, s, h.a, h.b))),
        ]);
        add(
            &mut rights,
            h.kind.to_string(),
            v,
            0.6,
            evidence(s, h.a, h.b),
        );
    }
    flush(rights, &keys::TERMINATION, out);
}

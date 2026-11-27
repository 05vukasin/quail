//! `contract.facts@1`: runs the rule modules over the sentences and lines of a document and
//! emits their facts, at most [`MAX_PER_KEY`] per key, each with a highlight.

use quail_core::highlight::BlockIndex;
use quail_core::{
    Analyzer, AnalyzerId, Ctx, Document, Emitter, Evidence, Fact, FactKey, Profile, Result, Span,
    Value,
};
use quail_payment::lex::{self, Line};

use crate::{document, keys, law, parties, risk, terms, text};

/// Text read per document, whatever `Limits::max_text_bytes` allows.
pub const MAX_SCAN_BYTES: usize = 8 << 20;
/// Lines read per document.
pub const MAX_SCAN_LINES: usize = 200_000;
/// Most facts per key.
pub const MAX_PER_KEY: usize = 16;

/// Key contract and invoice facts with highlights. See the crate documentation for the keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContractFacts;

impl ContractFacts {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("contract.facts", 1);
}

impl Analyzer for ContractFacts {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    /// Runs after the other steps: it reads the companies, identifiers, dates, clauses and
    /// references they found.
    fn finalizer(&self) -> bool {
        true
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
        let read = lex::read(doc, budget, MAX_SCAN_LINES, || {
            out.check_deadline().is_err()
        });
        if read.stopped {
            out.problem("contract.facts: deadline reached, later pages not read");
        }
        if let Some(p) = read.cut_at {
            out.problem(format!("contract.facts: text limit reached at page {p}"));
        }
        analyze(doc, &read.lines, ctx.facts, out);
        Ok(())
    }
}

/// One fact on its way out.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub key: FactKey,
    pub value: Value,
    pub conf: f32,
    pub page: u32,
    pub span: Span,
}

/// What the rules read: the document, its lines and sentences, the other steps' facts.
pub(crate) struct Input<'a> {
    pub doc: &'a Document,
    pub lines: &'a [Line],
    pub sents: &'a [Line],
    pub facts: &'a [Fact],
}

impl Input<'_> {
    /// Facts of `key` with a page and a span.
    pub fn located<'f>(&'f self, key: &'f str) -> impl Iterator<Item = (&'f Fact, u32, Span)> + 'f {
        self.facts.iter().filter_map(move |f| {
            if f.key.as_str() != key {
                return None;
            }
            let e = f.evidence.as_ref()?;
            Some((f, e.page?, e.span?))
        })
    }
}

/// Confidence of a fact found on `line`: `base`, lowered on OCR text, rounded.
pub(crate) fn conf(line: &Line, base: f32) -> f32 {
    let c = if line.ocr { base * 0.95 } else { base };
    (c.clamp(0.0, 1.0) * 100.0).round() / 100.0
}

/// A record from fields, the absent ones left out.
pub(crate) fn rec(fields: Vec<(&str, Option<Value>)>) -> Value {
    Value::record(fields.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))))
}

/// Text value.
#[allow(clippy::unnecessary_wraps)] // fills `rec` fields, which are optional
pub(crate) fn t(s: impl Into<Box<str>>) -> Option<Value> {
    Some(Value::text(s))
}

/// Int value.
#[allow(clippy::unnecessary_wraps)] // fills `rec` fields, which are optional
pub(crate) fn int(n: u32) -> Option<Value> {
    Some(Value::Int(i64::from(n)))
}

/// Float of a decimal.
#[allow(clippy::unnecessary_wraps)] // fills `rec` fields, which are optional
pub(crate) fn float(d: quail_text::money::Decimal) -> Option<Value> {
    Some(Value::Float((d.to_f64() * 10_000.0).round() / 10_000.0))
}

/// Keys of which a document has one fact: the most confident (then the first) is kept.
const SINGLE: &[&str] = &[
    "contract.term",
    "contract.renewal",
    "contract.notice",
    "contract.payment_days",
    "contract.payment_method",
    "contract.indexation",
    "contract.late_interest",
    "contract.liability_cap",
    "contract.confidentiality",
    "contract.warranty",
    "contract.governing_law",
    "contract.court",
    "contract.arbitration",
    "contract.language",
];

/// Facts that say the same thing: one key and one distinguishing value.
fn same(a: &Found, b: &Found) -> bool {
    if a.key != b.key {
        return false;
    }
    if SINGLE.contains(&a.key.as_str()) {
        return true;
    }
    let f = |v: &Value, n: &str| v.field(n).cloned();
    match a.key.as_str() {
        "contract.termination" => f(&a.value, "kind") == f(&b.value, "kind"),
        "contract.sla" => f(&a.value, "metric") == f(&b.value, "metric"),
        "contract.guarantee" => {
            f(&a.value, "kind") == f(&b.value, "kind")
                && f(&a.value, "purpose") == f(&b.value, "purpose")
        }
        "contract.annex" => {
            let k = |v: &Value| {
                v.field("label").and_then(Value::as_text).map(|s| {
                    quail_text::fold::fold(s, &quail_text::fold::FoldOptions::KEY).into_owned()
                })
            };
            k(&a.value) == k(&b.value)
        }
        "contract.party"
        | "contract.representative"
        | "contract.signatory"
        | "contract.address" => f(&a.value, "role") == f(&b.value, "role"),
        "contract.party_id" => {
            f(&a.value, "kind") == f(&b.value, "kind")
                && f(&a.value, "value") == f(&b.value, "value")
        }
        _ => a.value == b.value,
    }
}

/// Keeps one fact per [`same`] group (the most confident, ties to the first) and at most
/// [`MAX_PER_KEY`] per key, in document order.
fn dedup(found: Vec<Found>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for f in found {
        if let Some(o) = out.iter_mut().find(|o| same(o, &f)) {
            if f.conf > o.conf {
                *o = f;
            }
            continue;
        }
        if out.iter().filter(|o| o.key == f.key).count() < MAX_PER_KEY {
            out.push(f);
        }
    }
    out
}

/// The facts of one document.
pub(crate) fn analyze(doc: &Document, lines: &[Line], facts: &[Fact], out: &mut Emitter) {
    let sents = text::sentences(doc, lines);
    let input = Input {
        doc,
        lines,
        sents: &sents,
        facts,
    };
    let mut found = Vec::new();
    parties::scan(&input, &mut found);
    terms::scan(&input, &mut found);
    risk::scan(&input, &mut found);
    law::scan(&input, &mut found);
    document::scan(&input, &mut found);
    let found = dedup(found);
    let index = BlockIndex::new(doc);
    for f in &found {
        out.push(
            f.key.clone(),
            f.value.clone(),
            f.conf,
            Some(Evidence {
                page: Some(f.page),
                span: Some(f.span),
                ..Evidence::default()
            }),
        );
    }
    for f in &found {
        if let Some(h) = index.highlight(doc, &f.key, f.page, f.span, f.conf) {
            out.push(
                keys::HIGHLIGHT,
                h.to_value(),
                f.conf,
                Some(Evidence {
                    page: Some(f.page),
                    span: Some(f.span),
                    ..Evidence::default()
                }),
            );
        }
    }
}

//! `payment.extract@1`: the rules of [`crate::amount`], [`crate::subject`] and
//! [`crate::category`] turned into facts, with a highlight per fact.

use quail_core::highlight::BlockIndex;
use quail_core::{
    Analyzer, AnalyzerId, Ctx, Document, Emitter, Evidence, Fact, FactKey, Money, Profile, Result,
    Span, Value,
};
use quail_text::fold::{FoldOptions, fold};
use quail_text::money::Decimal;
use quail_text::token::tokens;

use crate::amount::{self, Cand};
use crate::category::{self, CATEGORIES};
use crate::cues::{self, Period, Role, Strength};
use crate::keys;
use crate::lex::{self, Line, word_matches};
use crate::subject::{self, Source, Subject};

/// Text read per document, whatever `Limits::max_text_bytes` allows.
pub const MAX_SCAN_BYTES: usize = 8 << 20;
/// Lines read per document.
pub const MAX_SCAN_LINES: usize = 200_000;
/// Most entries of `payment.lines`.
pub const MAX_LINES: usize = 200;
/// Lowest `item.line` confidence taken into `payment.lines` (below it the line parser only
/// guessed which number is which).
pub const MIN_LINE_CONF: f32 = 0.4;
/// Line descriptions joined into a subject when no subject clause is found.
const SUBJECT_FROM_LINES: usize = 3;

/// What is paid for and how much: subject, category, price (net, VAT, gross), currency,
/// period and line items of a contract, offer or invoice, native or OCR text alike. See the
/// crate documentation for the keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaymentExtract;

impl PaymentExtract {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("payment.extract", 1);
}

impl Analyzer for PaymentExtract {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    /// Runs after every other step, so the price lines (`item.line`), table totals and the
    /// instrument kind (`ref.instrument_kind`) are known.
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
        let read = lex::read(doc, budget, MAX_SCAN_LINES, || out.check_deadline().is_err());
        if read.stopped {
            out.problem("payment.extract: deadline reached, later pages not read");
        }
        if let Some(p) = read.cut_at {
            out.problem(format!("payment.extract: text limit reached at page {p}"));
        }
        analyze(doc, &read.lines, ctx.facts, out);
        Ok(())
    }
}

/// One fact on its way out, with the byte span its highlight quotes.
struct Out {
    key: FactKey,
    value: Value,
    conf: f32,
    at: Option<(u32, Span)>,
}

fn ev(at: Option<(u32, Span)>) -> Option<Evidence> {
    at.map(|(page, span)| Evidence {
        page: Some(page),
        span: Some(span),
        ..Evidence::default()
    })
}

fn sp(start: u32, end: u32) -> Span {
    Span { start, end }
}

fn round(c: f32) -> f32 {
    (c.clamp(0.0, 1.0) * 100.0).round() / 100.0
}

fn code(c: [u8; 3]) -> String {
    String::from_utf8_lossy(&c).into_owned()
}

fn money(d: Decimal, currency: Option<[u8; 3]>) -> Value {
    Value::Money(Money {
        units: d.units,
        scale: d.scale,
        currency,
    })
}

/// Whether `a + b` is `c` within half a percent (or one cent).
fn sums(a: Decimal, b: Decimal, c: Decimal) -> bool {
    let (a, b, c) = (a.to_f64(), b.to_f64(), c.to_f64());
    (a + b - c).abs() <= (c.abs() * 0.005).max(0.011)
}

fn same(a: Decimal, b: Decimal) -> bool {
    (a.to_f64() - b.to_f64()).abs() <= (b.to_f64().abs() * 0.005).max(0.011)
}

fn base_conf(c: &Cand) -> f32 {
    let mut conf = match c.strength {
        Strength::Strong => 0.85,
        Strength::Fair => 0.72,
        Strength::Weak => 0.55,
    };
    if c.currency.is_none() {
        conf -= 0.1;
    }
    if c.next_line {
        conf -= 0.05;
    }
    if c.ocr {
        conf *= 0.95;
    }
    conf
}

/// The best candidate of `role`: strongest cue, then largest amount. The flag says another
/// candidate as strong has another value.
fn best(cands: &[Cand], role: Role) -> Option<(&Cand, bool)> {
    let mut it = cands.iter().filter(|c| c.role == role);
    let mut top = it.next()?;
    for c in it {
        if c.strength > top.strength
            || (c.strength == top.strength && c.value.to_f64() > top.value.to_f64())
        {
            top = c;
        }
    }
    let ambiguous = cands
        .iter()
        .any(|c| c.role == role && c.strength == top.strength && !same(c.value, top.value));
    Some((top, ambiguous))
}

/// `item.table_total` facts as candidates (`label kind amount currency`), for totals in
/// table cells the text scan did not pair with a label.
fn table_totals(facts: &[Fact], ocr_pages: &[u32]) -> Vec<Cand> {
    let mut out = Vec::new();
    for f in facts.iter().filter(|f| f.key.as_str() == "item.table_total") {
        let role = match f.value.field("kind").and_then(Value::as_text) {
            Some("net" | "subtotal") => Role::Net,
            Some("vat") => Role::Vat,
            Some("gross") => Role::Gross,
            Some("total") => Role::Plain,
            _ => continue,
        };
        let Some(Value::Money(m)) = f.value.field("amount") else {
            continue;
        };
        let (Some(page), Some(span)) = (
            f.evidence.as_ref().and_then(|e| e.page),
            f.evidence.as_ref().and_then(|e| e.span),
        ) else {
            continue;
        };
        out.push(Cand {
            value: Decimal::new(m.units, m.scale),
            currency: m.currency,
            role,
            strength: Strength::Fair,
            period: None,
            page,
            ocr: ocr_pages.contains(&page),
            start: span.start,
            end: span.end,
            next_line: false,
        });
    }
    out
}

/// The facts of one document.
pub(crate) fn analyze(doc: &Document, lines: &[Line], facts: &[Fact], out: &mut Emitter) {
    let mut outs: Vec<Out> = Vec::new();
    let mut highlights: Vec<(FactKey, u32, Span, f32)> = Vec::new();

    let invoice = facts.iter().any(|f| {
        f.key.as_str() == "ref.instrument_kind" && f.value.as_text() == Some("invoice")
    }) || invoice_title(lines);

    // Price candidates; rates (per hour, per unit) are never the price.
    let mut cands = Vec::new();
    for li in 0..lines.len() {
        amount::scan(doc, lines, li, &mut cands);
    }
    cands.retain(|c| c.period != Some(Period::Rate));
    let ocr_pages: Vec<u32> = lines.iter().filter(|l| l.ocr).map(|l| l.page).collect();
    for t in table_totals(facts, &ocr_pages) {
        if !cands.iter().any(|c| c.role == t.role) {
            cands.push(t);
        }
    }
    price(&cands, invoice, lines, &mut outs);

    // Line items.
    let items: Vec<&Fact> = facts
        .iter()
        .filter(|f| f.key.as_str() == "item.line" && f.conf >= MIN_LINE_CONF)
        .take(MAX_LINES)
        .collect();
    if !items.is_empty() {
        let list: Vec<Value> = items.iter().map(|f| line_value(&f.value)).collect();
        #[allow(clippy::cast_precision_loss)]
        let conf = items.iter().map(|f| f.conf).sum::<f32>() / items.len() as f32;
        let first = items.iter().find_map(|f| at_of(f.evidence.as_ref()));
        outs.push(Out {
            key: keys::LINES,
            value: Value::List(list),
            conf: round(conf),
            at: first,
        });
        for f in &items {
            if let Some((page, span)) = at_of(f.evidence.as_ref()) {
                highlights.push((keys::LINES, page, span, round(f.conf)));
            }
        }
    }

    // Subject and category.
    let mut subjects = Vec::new();
    for li in 0..lines.len() {
        subject::scan(doc, lines, li, &mut subjects);
    }
    let mut chosen: Option<Subject> = None;
    for s in subjects {
        if chosen.as_ref().is_none_or(|c| s.conf > c.conf) {
            chosen = Some(s);
        }
    }
    let mut subject_at = None;
    if let Some(s) = &chosen {
        subject_at = Some((lines[s.line].page, sp(s.start, s.end)));
    }
    let mut subject_keys: Vec<String> = Vec::new();
    let mut subject_conf = 0.0;
    if let Some(s) = chosen.take() {
        subject_conf = s.conf * if lines[s.line].ocr { 0.95 } else { 1.0 };
        outs.push(subject_out(&s.text, s.source, subject_conf, subject_at));
        subject_keys = s.keys;
    } else if !items.is_empty() {
        let descs: Vec<String> = items
            .iter()
            .filter_map(|f| f.value.field("description").and_then(Value::as_text))
            .take(SUBJECT_FROM_LINES)
            .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        if !descs.is_empty() {
            let text = descs.join("; ");
            subject_conf = 0.6;
            subject_at = items.iter().find_map(|f| at_of(f.evidence.as_ref()));
            outs.push(subject_out(&text, Source::Lines, subject_conf, subject_at));
            for f in &items {
                if let Some(d) = f.value.field("description").and_then(Value::as_text) {
                    subject_keys.extend(tokens(d).map(|t| fold(t.text, &FoldOptions::KEY).into_owned()));
                }
            }
        }
    } else if let Some(s) = subject::title(doc, lines) {
        subject_conf = s.conf;
        subject_at = Some((lines[s.line].page, sp(s.start, s.end)));
        outs.push(subject_out(&s.text, s.source, subject_conf, subject_at));
        subject_keys = s.keys;
    }
    if subject_conf > 0.0 {
        let (cat, keyword, conf) = match category::categorize(subject_keys.iter().map(String::as_str)) {
            Some(p) => {
                let c = match p.score {
                    0..=1 => 0.55,
                    2 => 0.7,
                    _ => 0.8,
                };
                let c = if p.tied { 0.45 } else { c };
                let keyword = subject_keys.get(p.token).cloned().unwrap_or_default();
                (p.category, keyword, c)
            }
            None => (category::other(), String::new(), 0.3),
        };
        debug_assert!(CATEGORIES.iter().any(|c| c.code == cat.code));
        let mut fields = vec![
            ("code", Value::text(cat.code)),
            ("label", Value::text(cat.label)),
        ];
        if !keyword.is_empty() {
            fields.push(("keyword", Value::text(keyword)));
        }
        outs.push(Out {
            key: keys::CATEGORY,
            value: Value::record(fields),
            conf: round(conf * (subject_conf + 0.15).min(1.0)),
            at: subject_at,
        });
    }

    // Emit, with one highlight per fact that has a place in the text.
    let index = BlockIndex::new(doc);
    for o in &outs {
        if let Some((page, span)) = o.at
            && o.key != keys::LINES
        {
            highlights.push((o.key.clone(), page, span, o.conf));
        }
    }
    for o in outs {
        out.push(o.key, o.value, o.conf, ev(o.at));
    }
    for (key, page, span, conf) in highlights {
        if let Some(h) = index.highlight(doc, &key, page, span, conf) {
            out.push(keys::HIGHLIGHT, h.to_value(), conf, ev(Some((page, span))));
        }
    }
}

fn at_of(e: Option<&Evidence>) -> Option<(u32, Span)> {
    let e = e?;
    Some((e.page?, e.span?))
}

fn subject_out(text: &str, source: Source, conf: f32, at: Option<(u32, Span)>) -> Out {
    Out {
        key: keys::SUBJECT,
        value: Value::record([
            ("text", Value::text(text)),
            ("source", Value::text(source.as_str())),
        ]),
        conf: round(conf),
        at,
    }
}

/// The `payment.lines` entry of an `item.line` record: the fields a reader of the price needs.
fn line_value(v: &Value) -> Value {
    const FIELDS: &[&str] = &[
        "description",
        "quantity",
        "unit",
        "unit_price",
        "total",
        "currency",
        "vat_rate",
    ];
    Value::record(
        FIELDS
            .iter()
            .filter_map(|&name| v.field(name).map(|x| (name, x.clone()))),
    )
}

/// Whether the first lines of the first page carry an invoice title (`FAKTURA br. 12/2026`,
/// `Račun`, `Invoice`).
fn invoice_title(lines: &[Line]) -> bool {
    let Some(first) = lines.first() else {
        return false;
    };
    lines.iter().take(8).filter(|l| l.pos == first.pos).any(|l| {
        let words: Vec<_> = l.toks.iter().filter(|t| t.wordlike()).collect();
        words.len() <= 6
            && words
                .first()
                .is_some_and(|w| cues::INVOICE_TITLE.contains(&w.key.as_str()))
    })
}

/// The price facts: amount, basis, currency, net, VAT, gross, period.
fn price(cands: &[Cand], invoice: bool, lines: &[Line], outs: &mut Vec<Out>) {
    let net = best(cands, Role::Net);
    let vat = best(cands, Role::Vat);
    let mut gross = best(cands, Role::Gross);
    let plain = best(cands, Role::Plain);

    // The document's currency, when every candidate that names one names the same.
    let mut doc_currency = None;
    let mut mixed = false;
    for c in cands.iter().filter_map(|c| c.currency) {
        match doc_currency {
            None => doc_currency = Some(c),
            Some(d) if d != c => mixed = true,
            Some(_) => {}
        }
    }
    let doc_currency = if mixed { None } else { doc_currency };

    let mut conf_of = |c: &(&Cand, bool)| {
        let mut x = base_conf(c.0);
        if c.1 {
            x -= 0.15;
        }
        x
    };
    let mut net_c = net.as_ref().map(&mut conf_of);
    let mut vat_c = vat.as_ref().map(&mut conf_of);
    let mut gross_c = gross.as_ref().map(&mut conf_of);
    let plain_c = plain.as_ref().map(&mut conf_of);

    // A plain total that is net + VAT is the gross amount.
    let mut basis = "unstated";
    if gross.is_none()
        && let (Some(p), Some(n), Some(v)) = (&plain, &net, &vat)
        && sums(n.0.value, v.0.value, p.0.value)
    {
        gross = Some((p.0, p.1));
        gross_c = plain_c;
    }
    if let (Some(n), Some(v), Some(g)) = (&net, &vat, &gross) {
        let d = if sums(n.0.value, v.0.value, g.0.value) {
            0.08
        } else {
            -0.15
        };
        for c in [&mut net_c, &mut vat_c, &mut gross_c].into_iter().flatten() {
            *c += d;
        }
    }

    let amount = if let Some(g) = &gross {
        basis = "gross";
        Some((g.0, gross_c.unwrap_or(0.5)))
    } else if let Some(p) = &plain {
        if net.as_ref().is_some_and(|n| same(n.0.value, p.0.value)) {
            basis = "net";
        }
        Some((p.0, plain_c.unwrap_or(0.5)))
    } else if let Some(n) = &net {
        basis = "net";
        Some((n.0, net_c.unwrap_or(0.5)))
    } else {
        None
    };

    let cur = |c: &Cand| c.currency.or(doc_currency);
    let at = |c: &Cand| Some((c.page, sp(c.start, c.end)));
    let lowered = |c: &Cand, x: f32| if c.currency.is_none() { x - 0.05 } else { x };

    if let Some((a, conf)) = amount {
        outs.push(Out {
            key: keys::AMOUNT,
            value: money(a.value, cur(a)),
            conf: round(lowered(a, conf)),
            at: at(a),
        });
        outs.push(Out {
            key: keys::AMOUNT_BASIS,
            value: Value::text(basis),
            conf: round(conf),
            at: at(a),
        });
        if let Some(c) = cur(a) {
            let cc = if a.currency.is_some() { conf } else { 0.6 };
            outs.push(Out {
                key: keys::CURRENCY,
                value: Value::text(code(c)),
                conf: round(cc),
                at: at(a),
            });
        }
        // Period: on the price's line, else on a line about paying, else one-off for an
        // invoice.
        let period = a
            .period
            .map(|p| (p, 0.85 * if a.ocr { 0.95 } else { 1.0 }, at(a)))
            .or_else(|| {
                lines.iter().find_map(|l| {
                    let pays = l
                        .toks
                        .iter()
                        .any(|t| cues::PAY_WORDS.iter().any(|w| word_matches(w, &t.key)));
                    if !pays {
                        return None;
                    }
                    let p = amount::period_on(&l.toks).filter(|p| *p != Period::Rate)?;
                    Some((p, 0.65, Some((l.page, l.span))))
                })
            })
            .or_else(|| invoice.then(|| (Period::OneOff, 0.6, at(a))));
        if let Some((p, c, pat)) = period {
            outs.push(Out {
                key: keys::PERIOD,
                value: Value::text(p.as_str()),
                conf: round(c),
                at: pat,
            });
        }
    }
    for (key, cand, conf) in [
        (keys::NET, &net, net_c),
        (keys::VAT, &vat, vat_c),
        (keys::GROSS, &gross, gross_c),
    ] {
        if let (Some(c), Some(conf)) = (cand, conf) {
            outs.push(Out {
                key,
                value: money(c.0.value, cur(c.0)),
                conf: round(lowered(c.0, conf)),
                at: at(c.0),
            });
        }
    }
}

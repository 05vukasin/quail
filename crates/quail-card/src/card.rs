//! `card.facts@1`: one canonical value per card field, from the facts of the other steps.

use std::fmt::Write as _;

use quail_core::highlight::BlockIndex;
use quail_core::{
    Analyzer, AnalyzerId, Ctx, Emitter, Evidence, Fact, FactKey, Profile, Result, Span, Value,
};

use crate::keys;

/// The key-facts card. See the crate documentation for the keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyFactsCard;

impl KeyFactsCard {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("card.facts", 1);
}

impl Analyzer for KeyFactsCard {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    /// Runs after every other step (registered after `contract.facts`): it reads their facts.
    fn finalizer(&self) -> bool {
        true
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let entries = build(ctx.facts);
        if entries.is_empty() {
            return Ok(());
        }
        let text = card_text(&entries);
        let index = ctx.document().map(BlockIndex::new);
        for e in &entries {
            out.push(
                e.key.clone(),
                e.value.clone(),
                round(e.conf),
                e.sources.first().map(|s| ev(s.0, s.1)),
            );
        }
        out.push(keys::TEXT, Value::text(text), 1.0, None);
        if let (Some(doc), Some(index)) = (ctx.document(), index.as_ref()) {
            for e in &entries {
                for &(page, span, conf) in &e.sources {
                    let conf = round(conf);
                    if let Some(h) = index.highlight(doc, &e.key, page, span, conf) {
                        out.push(keys::HIGHLIGHT, h.to_value(), conf, Some(ev(page, span)));
                    }
                }
            }
        }
        Ok(())
    }
}

fn round(c: f32) -> f32 {
    (c.clamp(0.0, 1.0) * 100.0).round() / 100.0
}

fn ev(page: u32, span: Span) -> Evidence {
    Evidence {
        page: Some(page),
        span: Some(span),
        ..Evidence::default()
    }
}

/// One card field: its value, confidence and the places of its source facts.
struct Entry {
    key: FactKey,
    value: Value,
    conf: f32,
    sources: Vec<(u32, Span, f32)>,
}

fn at(f: &Fact) -> Option<(u32, Span, f32)> {
    let e = f.evidence.as_ref()?;
    Some((e.page?, e.span?, f.conf))
}

/// The most confident fact of `key` (the first of equals).
fn best<'f>(facts: &'f [Fact], key: &str) -> Option<&'f Fact> {
    let mut top: Option<&Fact> = None;
    for f in facts.iter().filter(|f| f.key.as_str() == key) {
        if top.is_none_or(|t| f.conf > t.conf) {
            top = Some(f);
        }
    }
    top
}

/// A record with only `fields` of `v`, in that order; `None` when none is there.
fn pick(v: &Value, fields: &[&str]) -> Option<Value> {
    let kept: Vec<(&str, Value)> = fields
        .iter()
        .filter_map(|&n| v.field(n).map(|x| (n, x.clone())))
        .collect();
    (!kept.is_empty()).then(|| Value::record(kept))
}

struct Card<'f> {
    facts: &'f [Fact],
    out: Vec<Entry>,
}

impl Card<'_> {
    fn add(&mut self, key: FactKey, value: Value, from: &Fact) {
        self.out.push(Entry {
            key,
            value,
            conf: from.conf,
            sources: at(from).into_iter().collect(),
        });
    }

    /// The best `src` fact, mapped.
    fn one(&mut self, key: FactKey, src: &str, map: impl Fn(&Value) -> Option<Value>) {
        if let Some(f) = best(self.facts, src)
            && let Some(v) = map(&f.value)
        {
            self.add(key, v, f);
        }
    }

    /// The distinct values of `src` facts, mapped, as a list.
    fn list(&mut self, key: FactKey, srcs: &[&str], map: impl Fn(&Value) -> Option<Value>) {
        let mut items: Vec<Value> = Vec::new();
        let mut sources = Vec::new();
        let mut conf: f32 = 0.0;
        for f in self.facts.iter().filter(|f| srcs.contains(&f.key.as_str())) {
            let Some(v) = map(&f.value) else { continue };
            if items.contains(&v) || items.len() >= 16 {
                continue;
            }
            items.push(v);
            sources.extend(at(f));
            conf = conf.max(f.conf);
        }
        if !items.is_empty() {
            self.out.push(Entry {
                key,
                value: Value::List(items),
                conf,
                sources,
            });
        }
    }

    /// A party of `role`: `contract.party` and its details, else `party.company`.
    fn party(&mut self, key: FactKey, role: &str) {
        let facts = self.facts;
        let of_role = |k: &'static str| {
            facts.iter().filter(move |f| {
                f.key.as_str() == k && f.value.field("role").and_then(Value::as_text) == Some(role)
            })
        };
        let top = of_role("contract.party").max_by(|a, b| a.conf.total_cmp(&b.conf));
        let mut fields: Vec<(String, Value)> = Vec::new();
        let mut sources = Vec::new();
        let mut conf = 0.0;
        let name = if let Some(p) = top {
            sources.extend(at(p));
            conf = p.conf;
            p.value.field("name").and_then(Value::as_text).map(str::to_owned)
        } else {
            // `party.company` mentions with this role, weighted by the role's confidence.
            let mut best: Option<(&Fact, f32)> = None;
            for f in facts.iter().filter(|f| f.key.as_str() == "party.company") {
                let (Some(Value::List(roles)), Some(Value::List(rc))) =
                    (f.value.field("roles"), f.value.field("role_conf"))
                else {
                    continue;
                };
                for (r, c) in roles.iter().zip(rc) {
                    let c = match c {
                        Value::Float(x) => *x as f32,
                        _ => continue,
                    };
                    if r.as_text() == Some(role) && best.is_none_or(|b| c > b.1) {
                        best = Some((f, c));
                    }
                }
            }
            best.and_then(|(f, c)| {
                sources.extend(at(f));
                conf = c.clamp(0.0, 1.0);
                f.value.field("surface").and_then(Value::as_text).map(str::to_owned)
            })
        };
        let Some(name) = name else { return };
        fields.push(("name".into(), Value::text(name)));
        for kind in ["pib", "mb", "vat", "reg", "iban", "account"] {
            let id = of_role("contract.party_id")
                .filter(|f| f.value.field("kind").and_then(Value::as_text) == Some(kind))
                .max_by(|a, b| a.conf.total_cmp(&b.conf));
            if let Some(f) = id
                && let Some(v) = f.value.field("value")
            {
                let field = if kind == "iban" { "account" } else { kind };
                if !fields.iter().any(|(n, _)| n == field) {
                    fields.push((field.into(), v.clone()));
                    sources.extend(at(f));
                }
            }
        }
        if let Some(f) = of_role("contract.address").next()
            && let Some(v) = f.value.field("text")
        {
            fields.push(("address".into(), v.clone()));
            sources.extend(at(f));
        }
        if let Some(f) = of_role("contract.representative").next() {
            let who = [f.value.field("name"), f.value.field("title")]
                .into_iter()
                .flatten()
                .filter_map(Value::as_text)
                .collect::<Vec<_>>()
                .join(", ");
            if !who.is_empty() {
                fields.push(("representative".into(), Value::text(who)));
                sources.extend(at(f));
            }
        }
        self.out.push(Entry {
            key,
            value: Value::Record(fields.into_iter().map(|(k, v)| (k.into_boxed_str(), v)).collect()),
            conf,
            sources,
        });
    }
}

const DURATION: &[&str] = &["iso", "months", "days"];

/// The card fields of `facts`.
fn build(facts: &[Fact]) -> Vec<Entry> {
    let mut c = Card {
        facts,
        out: Vec::new(),
    };
    let text = |v: &Value| v.as_text().map(Value::text);
    let field = |n: &'static str| move |v: &Value| v.field(n).cloned();
    c.one(keys::KIND, "ref.instrument_kind", text);
    c.one(keys::NUMBER, "ref.doc_number", field("raw"));
    c.party(keys::SUPPLIER, "supplier");
    c.party(keys::CUSTOMER, "customer");
    let date = |v: &Value| matches!(v, Value::Date(_)).then(|| v.clone());
    c.one(keys::SIGNED, "signal.date_signed", date);
    c.one(keys::START, "signal.date_effective", date);
    c.one(keys::END, "signal.date_expiry", date);
    c.one(keys::ISSUED, "signal.date_invoice", date);
    c.one(keys::DUE, "signal.date_due", date);
    c.one(keys::TERM, "contract.term", |v| {
        pick(v, &["iso", "months", "days", "indefinite"])
    });
    c.one(keys::AUTO_RENEWAL, "contract.renewal", field("auto"));
    c.one(keys::NOTICE, "contract.notice", |v| pick(v, DURATION));
    c.one(keys::PAYMENT_DAYS, "contract.payment_days", field("days"));
    c.one(keys::PAYMENT_METHOD, "contract.payment_method", text);
    c.one(keys::AMOUNT, "payment.amount", |v| {
        matches!(v, Value::Money(_)).then(|| v.clone())
    });
    c.one(keys::AMOUNT_BASIS, "payment.amount_basis", text);
    c.one(keys::CURRENCY, "payment.currency", text);
    c.one(keys::PERIOD, "payment.period", text);
    c.one(keys::SUBJECT, "payment.subject", field("text"));
    c.one(keys::CATEGORY, "payment.category", field("code"));
    c.list(keys::ACCOUNTS, &["signal.iban", "signal.rs_account"], text);
    c.one(keys::ADVANCE, "contract.advance", |v| pick(v, &["percent", "amount"]));
    c.one(keys::INDEXATION, "contract.indexation", field("basis"));
    c.one(keys::PENALTY, "contract.penalty", |v| {
        pick(v, &["percent", "per", "amount", "cap_percent"])
    });
    c.list(keys::GUARANTEES, &["contract.guarantee"], field("kind"));
    c.one(keys::LIABILITY_CAP, "contract.liability_cap", |v| {
        pick(v, &["amount", "percent", "basis", "months"])
    });
    if let Some(f) = best(facts, "contract.confidentiality") {
        c.add(keys::CONFIDENTIALITY, Value::Bool(true), f);
    } else if let Some(f) = facts
        .iter()
        .find(|f| f.key.as_str() == "ref.instrument_kind" && f.value.as_text() == Some("nda"))
    {
        c.add(keys::CONFIDENTIALITY, Value::Bool(true), f);
    }
    sla(&mut c);
    c.one(keys::WARRANTY, "contract.warranty", |v| pick(v, DURATION));
    c.one(keys::GOVERNING_LAW, "contract.governing_law", field("code"));
    c.one(keys::COURT, "contract.court", field("court"));
    c.one(keys::ARBITRATION, "contract.arbitration", |v| {
        Some(
            v.field("institution")
                .or_else(|| v.field("rules"))
                .cloned()
                .unwrap_or_else(|| Value::text("arbitration")),
        )
    });
    c.one(keys::LANGUAGES, "contract.language", field("languages"));
    c.list(keys::ANNEXES, &["contract.annex"], field("label"));
    if let Some(f) = facts
        .iter()
        .filter(|f| {
            f.key.as_str() == "ref.cites"
                && f.value.field("relation").and_then(Value::as_text) == Some("amends")
        })
        .max_by(|a, b| a.conf.total_cmp(&b.conf))
        && let Some(v) = f.value.field("raw")
    {
        c.add(keys::AMENDS, v.clone(), f);
    }
    c.out
}

/// `card.sla` from the `contract.sla` facts, one field per metric.
fn sla(c: &mut Card<'_>) {
    let mut fields: Vec<(&str, Value)> = Vec::new();
    let mut sources = Vec::new();
    let mut conf: f32 = 0.0;
    for f in c.facts.iter().filter(|f| f.key.as_str() == "contract.sla") {
        let (name, v) = match f.value.field("metric").and_then(Value::as_text) {
            Some("availability") => ("availability", f.value.field("percent")),
            Some("response") => ("response_hours", f.value.field("hours")),
            Some("resolution") => ("resolution_hours", f.value.field("hours")),
            _ => continue,
        };
        let Some(v) = v else { continue };
        if fields.iter().any(|(n, _)| *n == name) {
            continue;
        }
        fields.push((name, v.clone()));
        sources.extend(at(f));
        conf = conf.max(f.conf);
    }
    if !fields.is_empty() {
        c.out.push(Entry {
            key: keys::SLA,
            value: Value::record(fields),
            conf,
            sources,
        });
    }
}

/// A value as card text: dates ISO, money as a decimal and its code, records `k=v`.
fn show(v: &Value, out: &mut String) {
    match v {
        Value::Bool(b) => out.push_str(if *b { "yes" } else { "no" }),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Float(x) => {
            let _ = write!(out, "{x}");
        }
        Value::Text(s) => out.push_str(s),
        Value::Date(d) => {
            let _ = write!(out, "{:04}-{:02}-{:02}", d.y, d.m, d.d);
        }
        Value::Money(m) => {
            let digits = m.units.unsigned_abs().to_string();
            let scale = usize::from(m.scale);
            if m.units < 0 {
                out.push('-');
            }
            if scale == 0 {
                out.push_str(&digits);
            } else {
                let padded = format!("{digits:0>width$}", width = scale + 1);
                let (int, frac) = padded.split_at(padded.len() - scale);
                let _ = write!(out, "{int}.{frac}");
            }
            if let Some(c) = m.currency {
                out.push(' ');
                out.push_str(&String::from_utf8_lossy(&c));
            }
        }
        Value::List(items) => {
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                show(x, out);
            }
        }
        Value::Record(fields) => {
            for (i, (k, x)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str("; ");
                }
                let _ = write!(out, "{k}=");
                show(x, out);
            }
        }
        Value::U64(_) | Value::Bytes(_) => {}
    }
}

/// The card as `field: value` lines.
fn card_text(entries: &[Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        let name = e.key.as_str().strip_prefix("card.").unwrap_or(e.key.as_str());
        let _ = write!(out, "{name}: ");
        show(&e.value, &mut out);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Date, Money};

    #[test]
    fn text_form_is_normalised() {
        let mut s = String::new();
        show(&Value::Date(Date { y: 2026, m: 1, d: 5 }), &mut s);
        s.push('|');
        show(
            &Value::Money(Money {
                units: 148_148_147,
                scale: 2,
                currency: Some(*b"RSD"),
            }),
            &mut s,
        );
        s.push('|');
        show(&Value::record([("iso", Value::text("P12M")), ("months", Value::Int(12))]), &mut s);
        assert_eq!(s, "2026-01-05|1481481.47 RSD|iso=P12M; months=12");
    }

    #[test]
    fn nothing_in_gives_no_card() {
        assert!(build(&[]).is_empty());
    }
}

//! The `items` task: price lines (`item.line` records of `quail-items`) against gold rows.
//!
//! Gold rows are labels of a task whose spec has `rows = true` (the committed map calls it
//! `items`), with `match = "row"` and an object value; every field is optional:
//!
//! ```json
//! { "task": "items", "match": "row", "page": 0,
//!   "value": { "description": "Senior consultant", "quantity": 10, "unit": "h",
//!              "unit_price": 85.0, "total": 850.0, "currency": "EUR",
//!              "cpv": ["79411000"] } }
//! ```
//!
//! # Matching
//!
//! A predicted line and a gold row match when
//!
//! - they are on the same page (when both pages are known),
//! - their prices agree within [`PRICE_TOL`] (0.5 %, relative): the unit price when both
//!   have one, else the total, else the unit price of one against the total of the other
//!   (a line with no quantity is often written as one amount),
//! - and, when both have a description, the token Jaccard of the two folded descriptions is
//!   at least [`MIN_JACCARD`].
//!
//! Pairs are taken one to one, best first (higher Jaccard, then closer price, then reading
//! order). Unmatched predictions are FPs, unmatched gold rows FNs: the task's
//! P / R / F1. The same matching without the description test gives `lines_price_only`.
//!
//! # Field accuracy (matched pairs)
//!
//! For each of `quantity`, `unit`, `unit_price`, `total`, `currency`, over the matched pairs
//! whose gold row has the field: the share whose prediction is equal (numbers within
//! [`PRICE_TOL`], quantity within 1e-6; unit and currency case-insensitive). A missing
//! prediction is wrong; `filled` counts the pairs where the line has the field at all.
//!
//! # Categories
//!
//! Over matched pairs whose gold row lists `cpv` codes: `hit@3` is the share whose top-3
//! `category_candidates` contain one of them (compared on the first 8 digits, so the check
//! digit is optional), `division@3` the share with one in the same CPV division (first two
//! digits).

use std::collections::{BTreeMap, HashSet};

use quail_core::{FileRecord, Value};
use serde::{Deserialize, Serialize};

use crate::gold::{GoldDoc, Label};
use crate::score::{Counts, DocTask, PredOutcome};
use crate::tasks::TaskSpec;

/// Relative tolerance on prices and totals.
pub const PRICE_TOL: f64 = 0.005;
/// Least token Jaccard of two descriptions for a match.
pub const MIN_JACCARD: f64 = 0.3;
/// Fields scored on matched pairs.
pub const FIELDS: &[&str] = &["quantity", "unit", "unit_price", "total", "currency"];

/// One gold price row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldRow {
    /// Description as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Quantity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<f64>,
    /// Unit in `quail-items`' vocabulary (`h`, `pcs`, `month`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Unit price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_price: Option<f64>,
    /// Line total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// ISO 4217 code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Acceptable CPV codes (`79411000` or `79411000-8`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpv: Vec<String>,
}

impl GoldRow {
    /// Reads the row of a `match = "row"` label.
    pub fn of(label: &Label) -> Option<Self> {
        serde_json::from_value(label.value.clone()).ok()
    }

    /// Short display form: `description @ price`.
    pub fn display(&self) -> String {
        display(
            self.description.as_deref(),
            self.unit_price.or(self.total),
            self.currency.as_deref(),
        )
    }
}

fn display(desc: Option<&str>, price: Option<f64>, cur: Option<&str>) -> String {
    let d: String = desc.unwrap_or("?").chars().take(60).collect();
    match price {
        Some(p) => format!(
            "{d} @ {p}{}",
            cur.map(|c| format!(" {c}")).unwrap_or_default()
        ),
        None => d,
    }
}

/// One predicted line read back from a record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PredRow {
    /// Index of the fact in the record.
    pub fact: usize,
    /// Fact key.
    pub key: String,
    /// Confidence.
    pub conf: f32,
    /// Page of the evidence.
    pub page: Option<u32>,
    /// Description.
    pub description: Option<String>,
    /// Quantity.
    pub quantity: Option<f64>,
    /// Normalized unit.
    pub unit: Option<String>,
    /// Unit price.
    pub unit_price: Option<f64>,
    /// Total.
    pub total: Option<f64>,
    /// Currency.
    pub currency: Option<String>,
    /// Category codes, best first.
    pub cpv: Vec<String>,
}

impl PredRow {
    /// Display form, as [`GoldRow::display`].
    pub fn display(&self) -> String {
        display(
            self.description.as_deref(),
            self.unit_price.or(self.total),
            self.currency.as_deref(),
        )
    }
}

#[allow(clippy::cast_precision_loss)] // money far below 2^52 units
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Float(x) => Some(*x),
        Value::Int(n) => Some(*n as f64),
        Value::Money(m) => Some(m.units as f64 / 10f64.powi(i32::from(m.scale))),
        _ => None,
    }
}

/// The predicted lines of `rec`: every record value of the spec's keys.
pub fn pred_rows(rec: &FileRecord, spec: &TaskSpec, min_conf: Option<f32>) -> (Vec<PredRow>, u64) {
    let threshold = match (spec.min_conf, min_conf) {
        (Some(a), Some(b)) => a.max(b),
        (a, b) => a.or(b).unwrap_or(0.0),
    };
    let mut out = Vec::new();
    let mut dropped = 0;
    for (i, f) in rec.facts.iter().enumerate() {
        if !spec.keys.iter().any(|k| k == f.key.as_str()) || !matches!(f.value, Value::Record(_)) {
            continue;
        }
        if f.conf < threshold {
            dropped += 1;
            continue;
        }
        let v = &f.value;
        let text = |name: &str| v.field(name).and_then(Value::as_text).map(str::to_owned);
        let cpv = match v.field("category_candidates") {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(|c| c.field("code").and_then(Value::as_text))
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        out.push(PredRow {
            fact: i,
            key: f.key.to_string(),
            conf: f.conf,
            page: f.evidence.as_ref().and_then(|e| e.page),
            description: text("description"),
            quantity: v.field("quantity").and_then(number),
            unit: text("unit"),
            unit_price: v.field("unit_price").and_then(number),
            total: v.field("total").and_then(number),
            currency: text("currency"),
            cpv,
        });
    }
    // Most confident first, reading order within a confidence.
    out.sort_by(|a, b| b.conf.total_cmp(&a.conf).then(a.fact.cmp(&b.fact)));
    (out, dropped)
}

/// Lowercase alphanumeric tokens (diacritics and Cyrillic folded).
fn tokens(s: &str) -> HashSet<String> {
    let f = quail_text::fold::fold(s, &quail_text::fold::FoldOptions::KEY);
    f.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Token Jaccard of two descriptions.
pub fn jaccard(a: &str, b: &str) -> f64 {
    let (left, right) = (tokens(a), tokens(b));
    let union = left.union(&right).count();
    if union == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let share = left.intersection(&right).count() as f64 / union as f64;
    share
}

/// Relative difference within [`PRICE_TOL`].
pub fn price_close(a: f64, b: f64) -> bool {
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        return true;
    }
    (a - b).abs() / scale <= PRICE_TOL
}

/// The price difference of a pair (relative), when their prices agree.
fn price_gap(g: &GoldRow, p: &PredRow) -> Option<f64> {
    let pairs = [
        (g.unit_price, p.unit_price),
        (g.total, p.total),
        (g.unit_price, p.total),
        (g.total, p.unit_price),
    ];
    // The first pair where both sides are known decides; the cross pairs only when the
    // direct ones are not both known.
    let direct_known = (g.unit_price.is_some() && p.unit_price.is_some())
        || (g.total.is_some() && p.total.is_some());
    for (i, (a, b)) in pairs.into_iter().enumerate() {
        if i >= 2 && direct_known {
            break;
        }
        if let (Some(a), Some(b)) = (a, b)
            && price_close(a, b)
        {
            let scale = a.abs().max(b.abs()).max(1e-9);
            return Some((a - b).abs() / scale);
        }
    }
    None
}

/// One-to-one matching of gold rows and predicted lines; returns `(gold, pred)` pairs.
pub fn match_rows(
    gold: &[(Option<u32>, GoldRow)],
    pred: &[PredRow],
    use_description: bool,
) -> Vec<(usize, usize)> {
    let mut cands: Vec<(f64, f64, usize, usize)> = Vec::new();
    for (gi, (gpage, g)) in gold.iter().enumerate() {
        for (pi, p) in pred.iter().enumerate() {
            if let (Some(a), Some(b)) = (gpage, p.page)
                && *a != b
            {
                continue;
            }
            let Some(gap) = price_gap(g, p) else {
                continue;
            };
            let j = match (&g.description, &p.description) {
                (Some(a), Some(b)) if use_description => {
                    let j = jaccard(a, b);
                    if j < MIN_JACCARD {
                        continue;
                    }
                    j
                }
                _ => 0.0,
            };
            cands.push((j, gap, gi, pi));
        }
    }
    cands.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    let (mut gu, mut pu) = (vec![false; gold.len()], vec![false; pred.len()]);
    let mut out = Vec::new();
    for (_, _, gi, pi) in cands {
        if !gu[gi] && !pu[pi] {
            gu[gi] = true;
            pu[pi] = true;
            out.push((gi, pi));
        }
    }
    out.sort_unstable();
    out
}

/// Accuracy of one field over matched pairs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldScore {
    /// Matched pairs whose gold row has the field.
    pub n: u64,
    /// Of those, the line has the field.
    pub filled: u64,
    /// Of those, the line's value is right.
    pub correct: u64,
    /// `correct / n`.
    pub accuracy: Option<f64>,
}

/// Category hits over matched pairs with CPV gold.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CategoryScore {
    /// Matched pairs whose gold row lists codes.
    pub n: u64,
    /// A gold code among the top 3.
    pub hit3: u64,
    /// A gold code's division among the top 3.
    pub division3: u64,
    /// `hit3 / n`.
    pub hit_at_3: Option<f64>,
    /// `division3 / n`.
    pub division_at_3: Option<f64>,
}

/// The items scores of a run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemsScore {
    /// Matching without the description test (price and page only).
    pub lines_price_only: Counts,
    /// Per field.
    pub fields: BTreeMap<String, FieldScore>,
    /// Categories.
    pub category: CategoryScore,
}

#[allow(clippy::cast_precision_loss)]
fn ratio(a: u64, b: u64) -> Option<f64> {
    (b > 0).then(|| a as f64 / b as f64)
}

impl ItemsScore {
    /// Recomputes the rates.
    pub fn finish(&mut self) {
        for f in self.fields.values_mut() {
            f.accuracy = ratio(f.correct, f.n);
        }
        let c = &mut self.category;
        c.hit_at_3 = ratio(c.hit3, c.n);
        c.division_at_3 = ratio(c.division3, c.n);
    }

    /// Adds another run's counts (rates are recomputed by [`ItemsScore::finish`]).
    fn add(&mut self, o: &ItemsScore) {
        self.lines_price_only.add(
            o.lines_price_only.tp,
            o.lines_price_only.fp,
            o.lines_price_only.fn_,
        );
        for (k, f) in &o.fields {
            let e = self.fields.entry(k.clone()).or_default();
            e.n += f.n;
            e.filled += f.filled;
            e.correct += f.correct;
        }
        self.category.n += o.category.n;
        self.category.hit3 += o.category.hit3;
        self.category.division3 += o.category.division3;
    }
}

fn code8(c: &str) -> String {
    c.chars().filter(char::is_ascii_digit).take(8).collect()
}

fn field_check(name: &str, g: &GoldRow, p: &PredRow) -> Option<(bool, bool)> {
    let num = |a: Option<f64>, b: Option<f64>, close: &dyn Fn(f64, f64) -> bool| {
        a.map(|a| (b.is_some(), b.is_some_and(|b| close(a, b))))
    };
    let text = |a: &Option<String>, b: &Option<String>| {
        a.as_ref().map(|a| {
            (
                b.is_some(),
                b.as_ref().is_some_and(|b| a.eq_ignore_ascii_case(b)),
            )
        })
    };
    match name {
        "quantity" => num(g.quantity, p.quantity, &|a, b| {
            (a - b).abs() <= 1e-6 * a.abs().max(1.0)
        }),
        "unit" => text(&g.unit, &p.unit),
        "unit_price" => num(g.unit_price, p.unit_price, &price_close),
        "total" => num(g.total, p.total, &price_close),
        "currency" => text(&g.currency, &p.currency),
        _ => None,
    }
}

/// Scores the rows task `task` of one gold document against `rec`, or `None` when the
/// document has no rows of it and does not mark it absent.
pub fn score_doc(
    doc: &GoldDoc,
    task: &str,
    spec: &TaskSpec,
    rec: &FileRecord,
    min_conf: Option<f32>,
    acc: &mut ItemsScore,
) -> Option<DocTask> {
    let labels: Vec<&Label> = doc.labels_for(task).collect();
    let absent = doc.is_absent(task);
    if labels.is_empty() && !absent {
        return None;
    }
    let gold: Vec<(Option<u32>, GoldRow)> = labels
        .iter()
        .filter_map(|l| GoldRow::of(l).map(|r| (l.page, r)))
        .collect();
    let (pred, dropped) = pred_rows(rec, spec, min_conf);
    let pairs = match_rows(&gold, &pred, true);
    let loose = match_rows(&gold, &pred, false);
    let mut one = ItemsScore::default();
    let n = |x: usize| x as u64;
    one.lines_price_only.add(
        n(loose.len()),
        n(pred.len() - loose.len()),
        n(gold.len() - loose.len()),
    );
    for &(gi, pi) in &pairs {
        let (g, p) = (&gold[gi].1, &pred[pi]);
        for name in FIELDS {
            if let Some((filled, ok)) = field_check(name, g, p) {
                let f = one.fields.entry((*name).to_string()).or_default();
                f.n += 1;
                f.filled += u64::from(filled);
                f.correct += u64::from(ok);
            }
        }
        if !g.cpv.is_empty() {
            let gold8: Vec<String> = g.cpv.iter().map(|c| code8(c)).collect();
            let top: Vec<String> = p.cpv.iter().take(3).map(|c| code8(c)).collect();
            one.category.n += 1;
            one.category.hit3 += u64::from(top.iter().any(|t| gold8.contains(t)));
            one.category.division3 += u64::from(
                top.iter()
                    .any(|t| gold8.iter().any(|g| g.get(..2) == t.get(..2))),
            );
        }
    }
    acc.add(&one);
    let matched_pred: HashSet<usize> = pairs.iter().map(|&(_, p)| p).collect();
    let matched_gold: HashSet<usize> = pairs.iter().map(|&(g, _)| g).collect();
    let dt = DocTask {
        gold: gold.iter().map(|(_, g)| g.display()).collect(),
        absent,
        pred: pred
            .iter()
            .enumerate()
            .map(|(i, p)| PredOutcome {
                value: p.display(),
                conf: p.conf,
                key: p.key.clone(),
                fact: p.fact,
                correct: matched_pred.contains(&i),
            })
            .collect(),
        missed: gold
            .iter()
            .enumerate()
            .filter(|(i, _)| !matched_gold.contains(i))
            .map(|(_, (_, g))| g.display())
            .collect(),
        tp: n(pairs.len()),
        fp: n(pred.len() - pairs.len()),
        fn_: n(gold.len() - pairs.len()),
        below_min_conf: dropped,
    };
    Some(dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(desc: &str, price: f64) -> (Option<u32>, GoldRow) {
        (
            Some(0),
            GoldRow {
                description: Some(desc.into()),
                unit_price: Some(price),
                ..GoldRow::default()
            },
        )
    }

    fn p(desc: &str, price: f64, page: u32) -> PredRow {
        PredRow {
            page: Some(page),
            description: Some(desc.into()),
            unit_price: Some(price),
            ..PredRow::default()
        }
    }

    #[test]
    fn price_and_description_and_page() {
        let gold = vec![g("Senior consultant", 85.0), g("Junior consultant", 60.0)];
        let pred = vec![
            p("Senior Consultant (h)", 85.2, 0),
            p("Junior consultant", 60.0, 1),
            p("Travel", 85.0, 0),
        ];
        // 85.2 vs 85 is 0.23 %: a match; the junior line is on another page; travel shares
        // no word with the gold.
        assert_eq!(match_rows(&gold, &pred, true), vec![(0, 0)]);
        // Without descriptions the travel line could take the senior row, but the closer
        // price wins.
        assert_eq!(match_rows(&gold, &pred, false), vec![(0, 2)]);
        assert!(price_close(100.0, 100.5));
        assert!(!price_close(100.0, 100.6));
    }

    #[test]
    fn total_against_unit_price_when_direct_unknown() {
        let gold = vec![(
            None,
            GoldRow {
                description: Some("Licence".into()),
                total: Some(1200.0),
                ..GoldRow::default()
            },
        )];
        let pred = vec![p("Licence", 1200.0, 3)];
        assert_eq!(match_rows(&gold, &pred, true), vec![(0, 0)]);
    }

    #[test]
    fn jaccard_folds() {
        assert!((jaccard("Održavanje opreme", "odrzavanje OPREME") - 1.0).abs() < 1e-9);
        assert!(jaccard("a b", "c d") < 1e-9);
    }
}

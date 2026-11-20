//! Before/after comparison of two scoreboards, for regression gating.
//!
//! A comparison is a **regression** when any task's F1 drops by more than
//! [`CompareOptions::max_f1_drop`], a task scored before is missing after, or failed records
//! rise by more than [`CompareOptions::max_failed_increase`] (as a rate when the runs have
//! different sizes). It is **not comparable** when the gold sets differ (different digest,
//! split or dataset filter), unless that is explicitly allowed.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::report::{micros, pct};
use crate::score::Scoreboard;

/// Thresholds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompareOptions {
    /// Largest tolerated F1 drop per task, as a fraction (0.01 = one point).
    pub max_f1_drop: f64,
    /// Largest tolerated rise in failed records.
    pub max_failed_increase: u64,
    /// Compare even when the gold sets differ.
    pub allow_gold_change: bool,
    /// When set, a rise of the `total` p95 latency by more than this factor (1.5 = +50 %)
    /// is a regression. Off by default: latency depends on the machine.
    pub max_p95_ratio: Option<f64>,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            max_f1_drop: 0.01,
            max_failed_increase: 0,
            allow_gold_change: false,
            max_p95_ratio: None,
        }
    }
}

/// One task before and after.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskDelta {
    /// Task.
    pub task: String,
    /// F1 before.
    pub f1_a: Option<f64>,
    /// F1 after.
    pub f1_b: Option<f64>,
    /// Precision before / after.
    pub p: (Option<f64>, Option<f64>),
    /// Recall before / after.
    pub r: (Option<f64>, Option<f64>),
    /// Documents whose TP count fell or FP/FN count rose (ids).
    pub worse_docs: Vec<String>,
    /// Documents that improved (ids).
    pub better_docs: Vec<String>,
    /// This task regressed.
    pub regressed: bool,
}

/// The comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// Label of A.
    pub a: String,
    /// Label of B.
    pub b: String,
    /// The gold sets are the same.
    pub comparable: bool,
    /// Why not comparable.
    pub not_comparable: Vec<String>,
    /// Per task.
    pub tasks: Vec<TaskDelta>,
    /// Failed records before / after.
    pub failed: (u64, u64),
    /// Records before / after.
    pub records: (u64, u64),
    /// `total` p95 latency before / after, microseconds.
    pub p95_total: (u64, u64),
    /// Everything that counts as a regression.
    pub regressions: Vec<String>,
}

impl Comparison {
    /// Exit code for CI: 0 fine, 1 regression, 2 not comparable.
    pub fn exit_code(&self) -> i32 {
        if self.comparable {
            i32::from(!self.regressions.is_empty())
        } else {
            2
        }
    }
}

/// Compares `a` (before) with `b` (after).
#[allow(clippy::cast_precision_loss)]
pub fn compare(a: &Scoreboard, b: &Scoreboard, opts: &CompareOptions) -> Comparison {
    let mut not_comparable = Vec::new();
    if a.corpus.gold_digest != b.corpus.gold_digest {
        not_comparable.push("gold sets differ (gold digest)".to_string());
    }
    if a.split != b.split {
        not_comparable.push(format!(
            "splits differ: {} vs {}",
            a.split.as_str(),
            b.split.as_str()
        ));
    }
    if a.dataset != b.dataset {
        not_comparable.push("dataset filters differ".to_string());
    }
    let comparable = not_comparable.is_empty() || opts.allow_gold_change;

    let mut regressions = Vec::new();
    let names: BTreeSet<&String> = a.tasks.keys().chain(b.tasks.keys()).collect();
    let mut tasks = Vec::new();
    for name in names {
        let ta = a.tasks.get(name);
        let tb = b.tasks.get(name);
        let f1_a = ta.and_then(|t| t.counts.f1);
        let f1_b = tb.and_then(|t| t.counts.f1);
        let mut regressed = false;
        match (f1_a, f1_b) {
            (Some(x), Some(y)) if x - y > opts.max_f1_drop + 1e-12 => {
                regressed = true;
                regressions.push(format!(
                    "{name}: F1 {} → {} ({:+.1} points)",
                    pct(Some(x)),
                    pct(Some(y)),
                    (y - x) * 100.0
                ));
            }
            (Some(x), None) => {
                regressed = true;
                regressions.push(format!(
                    "{name}: scored before (F1 {}), not after",
                    pct(Some(x))
                ));
            }
            _ => {}
        }
        let (worse_docs, better_docs) = doc_flips(a, b, name);
        tasks.push(TaskDelta {
            task: name.clone(),
            f1_a,
            f1_b,
            p: (
                ta.and_then(|t| t.counts.precision),
                tb.and_then(|t| t.counts.precision),
            ),
            r: (
                ta.and_then(|t| t.counts.recall),
                tb.and_then(|t| t.counts.recall),
            ),
            worse_docs,
            better_docs,
            regressed,
        });
    }

    let (fa, fb) = (a.status.failed, b.status.failed);
    let (na, nb) = (a.status.total, b.status.total);
    if na == nb {
        if fb > fa + opts.max_failed_increase {
            regressions.push(format!("failed records {fa} → {fb}"));
        }
    } else if na > 0 && nb > 0 {
        let (ra, rb) = (fa as f64 / na as f64, fb as f64 / nb as f64);
        if rb > ra + 1e-12 && fb > fa + opts.max_failed_increase {
            regressions.push(format!(
                "failed rate {:.2} % → {:.2} % ({fa}/{na} → {fb}/{nb})",
                ra * 100.0,
                rb * 100.0
            ));
        }
    }

    let p95 = |s: &Scoreboard| s.latency.get("total").map_or(0, |l| l.p95);
    let (la, lb) = (p95(a), p95(b));
    if let Some(ratio) = opts.max_p95_ratio
        && la > 0
        && lb as f64 > la as f64 * ratio
    {
        regressions.push(format!("p95 per file {} → {}", micros(la), micros(lb)));
    }

    Comparison {
        a: a.label.clone(),
        b: b.label.clone(),
        comparable,
        not_comparable,
        tasks,
        failed: (fa, fb),
        records: (na, nb),
        p95_total: (la, lb),
        regressions,
    }
}

fn doc_flips(a: &Scoreboard, b: &Scoreboard, task: &str) -> (Vec<String>, Vec<String>) {
    let before: std::collections::HashMap<&str, (u64, u64, u64)> = a
        .docs
        .iter()
        .filter_map(|d| {
            d.tasks
                .get(task)
                .map(|t| (d.id.as_str(), (t.tp, t.fp, t.fn_)))
        })
        .collect();
    let mut worse = Vec::new();
    let mut better = Vec::new();
    for d in &b.docs {
        let (Some(t), Some(&(tp, fp, fn_))) = (d.tasks.get(task), before.get(d.id.as_str())) else {
            continue;
        };
        let errs_a = fp + fn_;
        let errs_b = t.fp + t.fn_;
        if t.tp < tp || errs_b > errs_a {
            worse.push(d.id.clone());
        } else if t.tp > tp || errs_b < errs_a {
            better.push(d.id.clone());
        }
    }
    (worse, better)
}

fn delta(a: Option<f64>, b: Option<f64>) -> String {
    match (a, b) {
        (Some(x), Some(y)) => format!("{:+.1}", (y - x) * 100.0),
        _ => "—".into(),
    }
}

/// Markdown summary: aggregates and document ids (hashes) only.
pub fn markdown(c: &Comparison) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# quail-eval compare · {} → {}\n", c.a, c.b);
    let verdict = match c.exit_code() {
        0 => "OK: no regression",
        1 => "REGRESSION",
        _ => "NOT COMPARABLE",
    };
    let _ = writeln!(s, "**{verdict}**\n");
    for n in &c.not_comparable {
        let _ = writeln!(s, "- {n}{}", if c.comparable { " (allowed)" } else { "" });
    }
    for r in &c.regressions {
        let _ = writeln!(s, "- regression: {r}");
    }
    let _ = writeln!(
        s,
        "\n| task | P | R | F1 before | F1 after | ΔF1 | docs worse | docs better |\n|---|---|---|---:|---:|---:|---:|---:|"
    );
    for t in &c.tasks {
        let _ = writeln!(
            s,
            "| {}{} | {} → {} | {} → {} | {} | {} | {} | {} | {} |",
            t.task,
            if t.regressed { " ⚠" } else { "" },
            pct(t.p.0),
            pct(t.p.1),
            pct(t.r.0),
            pct(t.r.1),
            pct(t.f1_a),
            pct(t.f1_b),
            delta(t.f1_a, t.f1_b),
            t.worse_docs.len(),
            t.better_docs.len()
        );
    }
    let _ = writeln!(
        s,
        "\nrecords {} → {} · failed {} → {} · p95 per file {} → {}",
        c.records.0,
        c.records.1,
        c.failed.0,
        c.failed.1,
        micros(c.p95_total.0),
        micros(c.p95_total.1)
    );
    let worse: Vec<String> = c
        .tasks
        .iter()
        .filter(|t| !t.worse_docs.is_empty())
        .map(|t| {
            let ids: Vec<&str> = t
                .worse_docs
                .iter()
                .take(10)
                .map(|id| id.get(..16).unwrap_or(id))
                .collect();
            format!(
                "- {}: {}{}",
                t.task,
                ids.join(" "),
                if t.worse_docs.len() > 10 { " …" } else { "" }
            )
        })
        .collect();
    if !worse.is_empty() {
        let _ = writeln!(
            s,
            "\nDocuments that got worse (inspect with `quail-eval inspect --blake3 <id>`):\n\n{}",
            worse.join("\n")
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{Counts, TaskScore};

    fn sb(f1: f64, failed: u64) -> Scoreboard {
        let mut s = Scoreboard {
            label: "x".into(),
            ..Scoreboard::default()
        };
        s.status.total = 100;
        s.status.failed = failed;
        s.tasks.insert(
            "party".into(),
            TaskScore {
                counts: Counts {
                    f1: Some(f1),
                    ..Counts::default()
                },
                ..TaskScore::default()
            },
        );
        s
    }

    #[test]
    fn gates_on_f1_and_failures() {
        let o = CompareOptions::default();
        assert_eq!(compare(&sb(0.80, 1), &sb(0.795, 1), &o).exit_code(), 0);
        assert_eq!(compare(&sb(0.80, 1), &sb(0.78, 1), &o).exit_code(), 1);
        assert_eq!(compare(&sb(0.80, 1), &sb(0.90, 2), &o).exit_code(), 1);
        let mut b = sb(0.8, 1);
        b.tasks.clear();
        assert_eq!(compare(&sb(0.80, 1), &b, &o).exit_code(), 1);
        let mut b = sb(0.8, 1);
        b.corpus.gold_digest = "other".into();
        assert_eq!(compare(&sb(0.80, 1), &b, &o).exit_code(), 2);
        let allow = CompareOptions {
            allow_gold_change: true,
            ..o
        };
        assert_eq!(compare(&sb(0.80, 1), &b, &allow).exit_code(), 0);
        assert!(markdown(&compare(&sb(0.80, 1), &sb(0.5, 1), &allow)).contains("REGRESSION"));
    }
}

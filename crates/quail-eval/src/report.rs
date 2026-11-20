//! Markdown report of a scoreboard. Aggregates only: no values, no locators, so a report on
//! a private corpus can be pasted into a public PR.

use std::fmt::Write as _;

use crate::score::{Counts, Scoreboard};

/// Percent with one decimal, `—` when undefined.
pub fn pct(x: Option<f64>) -> String {
    x.map_or_else(|| "—".into(), |v| format!("{:.1}", v * 100.0))
}

fn prf(c: &Counts) -> String {
    format!("{} | {} | {}", pct(c.precision), pct(c.recall), pct(c.f1))
}

/// Microseconds as a short duration.
#[allow(clippy::cast_precision_loss)]
pub fn micros(us: u64) -> String {
    if us >= 1_000_000 {
        format!("{:.2} s", us as f64 / 1e6)
    } else if us >= 1_000 {
        format!("{:.2} ms", us as f64 / 1e3)
    } else {
        format!("{us} µs")
    }
}

/// The report.
#[allow(clippy::too_many_lines)]
pub fn markdown(sb: &Scoreboard) -> String {
    let mut s = String::new();
    let c = &sb.corpus;
    let _ = writeln!(
        s,
        "# quail-eval · {} · split={}",
        sb.label,
        sb.split.as_str()
    );
    if let Some(d) = &sb.dataset {
        let _ = writeln!(s, "\ndataset: {d}");
    }
    let _ = writeln!(
        s,
        "\ncorpus: {} gold documents, {} scored ({} by locator), {} unmatched · run: {} records, {} bad lines",
        c.gold_docs,
        c.scored_docs,
        c.matched_by_locator,
        c.unmatched.len(),
        c.records,
        c.bad_record_lines
    );
    if let Some(m) = sb.min_conf {
        let _ = writeln!(s, "\nmin_conf: {m}");
    }
    if !c.unmapped_tasks.is_empty() {
        let list: Vec<String> = c
            .unmapped_tasks
            .iter()
            .map(|(t, n)| format!("{t} ({n})"))
            .collect();
        let _ = writeln!(
            s,
            "\n**unmapped gold tasks** (not in tasks.toml): {}",
            list.join(", ")
        );
    }
    if !c.unmatched.is_empty() {
        let _ = writeln!(
            s,
            "\n**{} gold documents have no record**: wrong data root or an incomplete run.",
            c.unmatched.len()
        );
    }
    let _ = writeln!(
        s,
        "\ngold digest: `{}`",
        c.gold_digest.get(..16).unwrap_or("")
    );

    let _ = writeln!(
        s,
        "\n## Status\n\n| | records | ok | partial | failed | with warnings |\n|---|---:|---:|---:|---:|---:|"
    );
    for (name, st) in [("run", &sb.status), ("scored", &sb.status_scored)] {
        let _ = writeln!(
            s,
            "| {name} | {} | {} | {} | {} | {} |",
            st.total, st.ok, st.partial, st.failed, st.with_warnings
        );
    }

    let _ = writeln!(
        s,
        "\n## Tasks\n\n| task | support | docs | absent docs | TP | FP | FN | P | R | F1 | abstain | pred. on absent | ECE |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
    for (name, t) in &sb.tasks {
        let _ = writeln!(
            s,
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            t.support,
            t.docs_labelled,
            t.docs_absent,
            t.counts.tp,
            t.counts.fp,
            t.counts.fn_,
            prf(&t.counts),
            pct(t.abstain_rate),
            t.predicted_on_absent,
            t.calibration
                .ece
                .map_or_else(|| "—".into(), |e| format!("{e:.3}")),
        );
    }

    let multi_dataset = sb
        .tasks
        .values()
        .any(|t| t.by_dataset.len() > 1 || t.by_split.len() > 1);
    if multi_dataset {
        let _ = writeln!(
            s,
            "\n## Breakdown\n\n| task | group | TP | FP | FN | P | R | F1 |\n|---|---|---:|---:|---:|---:|---:|---:|"
        );
        for (name, t) in &sb.tasks {
            for (g, c) in t
                .by_dataset
                .iter()
                .map(|(k, v)| (format!("dataset {k}"), v))
                .chain(t.by_split.iter().map(|(k, v)| (format!("split {k}"), v)))
            {
                let _ = writeln!(
                    s,
                    "| {name} | {g} | {} | {} | {} | {} |",
                    c.tp,
                    c.fp,
                    c.fn_,
                    prf(c)
                );
            }
        }
    }

    let with_cal: Vec<_> = sb
        .tasks
        .iter()
        .filter(|(_, t)| t.calibration.n > 0)
        .collect();
    if !with_cal.is_empty() {
        let _ = writeln!(
            s,
            "\n## Calibration\n\nPer `conf` bin: predictions / accuracy %.\n\n| task | n | 0–.1 | .1–.2 | .2–.3 | .3–.4 | .4–.5 | .5–.6 | .6–.7 | .7–.8 | .8–.9 | .9–1 |\n|---|---:|---|---|---|---|---|---|---|---|---|---|"
        );
        for (name, t) in with_cal {
            let bins: Vec<String> = t
                .calibration
                .bins
                .iter()
                .map(|b| {
                    if b.n == 0 {
                        "·".into()
                    } else {
                        format!("{} / {}", b.n, pct(b.accuracy))
                    }
                })
                .collect();
            let _ = writeln!(s, "| {name} | {} | {} |", t.calibration.n, bins.join(" | "));
        }
    }

    if !sb.latency.is_empty() {
        let _ = writeln!(
            s,
            "\n## Latency per file\n\n| step | n | mean | p50 | p95 | p99 | max |\n|---|---:|---:|---:|---:|---:|---:|"
        );
        for (step, l) in &sb.latency {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let mean = l.mean.round() as u64;
            let _ = writeln!(
                s,
                "| {step} | {} | {} | {} | {} | {} | {} |",
                l.n,
                micros(mean),
                micros(l.p50),
                micros(l.p95),
                micros(l.p99),
                micros(l.max)
            );
        }
    }
    s
}

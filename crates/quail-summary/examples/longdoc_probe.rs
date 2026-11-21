//! Measures `summary.outline` and `summary.digest` on extracted documents: digest tokens,
//! sections covered, verbatim check and build time per document. No model.
//!
//! ```text
//! quail scan DIR --dump-documents docs.jsonl -o records.jsonl
//! cargo run -p quail-summary --example longdoc_probe -- docs.jsonl [records.jsonl] \
//!     [--budget 2000] [--print SUBSTRING] [--parts 8]
//! ```
//!
//! `records.jsonl` gives each document its card text (`summary.card_text`); without it the
//! card is empty. `--print` prints the digest (and the map parts) of the documents whose
//! locator contains SUBSTRING. Output: one line per document, then aggregates.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use quail_core::{Document, FileRecord};
use quail_summary::digest::{self, DigestConfig, MapReduceConfig};
use quail_summary::outline::{self, OutlineConfig};

#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // a measurement script
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut files = Vec::new();
    let mut budget = 2000;
    let mut print: Option<String> = None;
    let mut parts = 8;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--budget" => budget = it.next().and_then(|v| v.parse().ok()).unwrap_or(budget),
            "--parts" => parts = it.next().and_then(|v| v.parse().ok()).unwrap_or(parts),
            "--print" => print = it.next().cloned(),
            _ => files.push(a.clone()),
        }
    }
    let Some(docs) = files.first() else {
        eprintln!("usage: longdoc_probe docs.jsonl [records.jsonl] [--budget N] [--print S]");
        return ExitCode::FAILURE;
    };
    let mut cards: HashMap<String, String> = HashMap::new();
    if let Some(r) = files.get(1) {
        let Ok(f) = std::fs::File::open(r) else {
            eprintln!("{r}: cannot open");
            return ExitCode::FAILURE;
        };
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(rec) = serde_json::from_str::<FileRecord>(&line) else {
                continue;
            };
            if let Some(t) = rec
                .facts
                .iter()
                .find(|f| f.key.as_str() == "summary.card_text")
                .and_then(|f| f.value.as_text())
            {
                cards.insert(rec.locator.clone(), t.to_owned());
            }
        }
    }
    let Ok(f) = std::fs::File::open(docs) else {
        eprintln!("{docs}: cannot open");
        return ExitCode::FAILURE;
    };
    let ocfg = OutlineConfig::default();
    let dcfg = DigestConfig {
        max_tokens: budget,
        ..DigestConfig::default()
    };
    let mr = MapReduceConfig {
        max_parts: parts,
        ..MapReduceConfig::default()
    };
    let mut rows: Vec<Row> = Vec::new();
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let locator = v["locator"].as_str().unwrap_or("").to_owned();
        let Ok(doc) = serde_json::from_value::<Document>(v["document"].clone()) else {
            continue;
        };
        let card = cards.get(&locator).cloned().unwrap_or_default();
        // Best of 3 runs for the time.
        let mut t_outline = Duration::MAX;
        let mut t_digest = Duration::MAX;
        let mut o = outline::outline(&doc, &ocfg, None);
        let mut d = digest::build(&doc, &card, &o, &dcfg, None);
        for _ in 0..3 {
            let t = Instant::now();
            o = outline::outline(&doc, &ocfg, None);
            t_outline = t_outline.min(t.elapsed());
            let t = Instant::now();
            d = digest::build(&doc, &card, &o, &dcfg, None);
            t_digest = t_digest.min(t.elapsed());
        }
        let t = Instant::now();
        let ps = digest::map_parts(&doc, &o, &dcfg, &mr, None);
        let t_parts = t.elapsed();
        let bad = d.verbatim(&doc).len();
        let excerpts: usize = d.sections.iter().map(|s| s.excerpts.len()).sum();
        let row = Row {
            pages: doc.pages.len(),
            doc_tokens: d.doc_tokens,
            method: o.method.as_str(),
            entries: o.entries.len(),
            total: o.total,
            digest: d.method.as_str(),
            tokens: d.tokens,
            sections: d.sections.len(),
            coverage: d.coverage(),
            excerpts,
            bad,
            outline_ms: ms(t_outline),
            digest_ms: ms(t_digest),
            parts: ps.len(),
            parts_tokens: ps.iter().map(|p| p.tokens).max().unwrap_or(0),
            parts_ms: ms(t_parts),
        };
        println!(
            "{:>5}p {:>8}tok outline={} {}/{} digest={} {}tok sections={} cov={:.2} excerpts={} \
             not_verbatim={} outline={:.2}ms digest={:.2}ms parts={} max_part={}tok {:.1}ms",
            row.pages,
            row.doc_tokens,
            row.method,
            row.entries,
            row.total,
            row.digest,
            row.tokens,
            row.sections,
            row.coverage,
            row.excerpts,
            row.bad,
            row.outline_ms,
            row.digest_ms,
            row.parts,
            row.parts_tokens,
            row.parts_ms
        );
        if print.as_deref().is_some_and(|p| locator.contains(p)) {
            println!("==== {locator}\n{}\n====", d.text);
            for (i, p) in ps.iter().enumerate() {
                println!(
                    "---- part {} of {}: {} ({} tok)\n{}",
                    i + 1,
                    ps.len(),
                    p.title,
                    p.tokens,
                    p.body
                );
            }
        }
        rows.push(row);
    }
    summarize(&rows);
    ExitCode::SUCCESS
}

struct Row {
    pages: usize,
    doc_tokens: usize,
    method: &'static str,
    entries: usize,
    total: usize,
    digest: &'static str,
    tokens: usize,
    sections: usize,
    coverage: f64,
    excerpts: usize,
    bad: usize,
    outline_ms: f64,
    digest_ms: f64,
    parts: usize,
    parts_tokens: usize,
    parts_ms: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn pct(mut v: Vec<f64>, q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    v[i]
}

#[allow(clippy::cast_precision_loss)]
fn summarize(rows: &[Row]) {
    let n = rows.len();
    let col = |f: &dyn Fn(&Row) -> f64| -> String {
        let v: Vec<f64> = rows.iter().map(f).collect();
        format!(
            "p10 {:.2} / p50 {:.2} / p90 {:.2} / max {:.2}",
            pct(v.clone(), 0.1),
            pct(v.clone(), 0.5),
            pct(v.clone(), 0.9),
            pct(v, 1.0)
        )
    };
    println!("documents: {n}");
    println!("pages:          {}", col(&|r| r.pages as f64));
    println!("digest tokens:  {}", col(&|r| r.tokens as f64));
    println!("coverage:       {}", col(&|r| r.coverage));
    println!("outline ms:     {}", col(&|r| r.outline_ms));
    println!("digest ms:      {}", col(&|r| r.digest_ms));
    println!(
        "ms per 100 pages (outline+digest): {}",
        col(&|r| (r.outline_ms + r.digest_ms) * 100.0 / r.pages.max(1) as f64)
    );
    println!("map parts ms:   {}", col(&|r| r.parts_ms));
    println!(
        "outline methods: headings {} / windows {}",
        rows.iter().filter(|r| r.method == "headings").count(),
        rows.iter().filter(|r| r.method == "windows").count()
    );
    println!(
        "digest methods: full {} / excerpts {}",
        rows.iter().filter(|r| r.digest == "full").count(),
        rows.iter().filter(|r| r.digest == "excerpts").count()
    );
    println!(
        "excerpts not verbatim: {} of {}",
        rows.iter().map(|r| r.bad).sum::<usize>(),
        rows.iter().map(|r| r.excerpts).sum::<usize>()
    );
}

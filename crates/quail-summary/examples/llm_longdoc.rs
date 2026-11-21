//! Summarizes long documents with the local model, two ways: one call over the digest, and
//! map-reduce over the parts. Prints time, peak RSS, fact check and the texts.
//!
//! ```text
//! quail scan DIR --dump-documents docs.jsonl -o records.jsonl
//! cargo run -p quail-summary --features llm --example llm_longdoc -- docs.jsonl records.jsonl \
//!     SUBSTRING... [--tier small|default] [--threads 4] [--parts 8]
//! ```
//!
//! Each SUBSTRING selects the documents whose locator contains it. The model must be in
//! `quail_summary::models::model_dir()` (see the `fetch_models` example).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use quail_core::{Document, FileRecord};
use quail_summary::digest::{self, DigestConfig, MapReduceConfig};
use quail_summary::mapreduce::{self, Step};
use quail_summary::models::{self, Tier};
use quail_summary::outline::{self, OutlineConfig};
use quail_summary::{FileCard, LlmConfig, LlmSummarizer, SummaryInput, prompt};

fn peak_rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // a measurement script
fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(docs), Some(recs)) = (args.next(), args.next()) else {
        eprintln!("usage: llm_longdoc docs.jsonl records.jsonl SUBSTRING... [--tier T]");
        return ExitCode::FAILURE;
    };
    let mut wanted = Vec::new();
    let mut tier = Tier::Default;
    let mut threads = 4u16;
    let mut parts = 8usize;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tier" => tier = args.next().and_then(|t| Tier::parse(&t)).unwrap_or(tier),
            "--threads" => threads = args.next().and_then(|t| t.parse().ok()).unwrap_or(threads),
            "--parts" => parts = args.next().and_then(|t| t.parse().ok()).unwrap_or(parts),
            _ => wanted.push(a),
        }
    }
    let mut cards: HashMap<String, (String, Option<String>)> = HashMap::new();
    if let Ok(f) = std::fs::File::open(&recs) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(rec) = serde_json::from_str::<FileRecord>(&line) else {
                continue;
            };
            let get = |k: &str| {
                rec.facts
                    .iter()
                    .find(|f| f.key.as_str() == k)
                    .and_then(|f| f.value.as_text())
                    .map(str::to_owned)
            };
            if let Some(t) = get("summary.card_text") {
                cards.insert(rec.locator.clone(), (t, get("text.lang")));
            }
        }
    }
    let Some(dir) = models::model_dir() else {
        eprintln!("no model directory; set {}", models::MODELS_ENV);
        return ExitCode::FAILURE;
    };
    let mr = MapReduceConfig {
        max_parts: parts,
        ..MapReduceConfig::default()
    };
    let cfg = LlmConfig {
        enabled: true,
        threads,
        map_reduce: Some(mr),
        timeout: Duration::from_secs(240),
        ..LlmConfig::default()
    };
    let t = Instant::now();
    let llm = match LlmSummarizer::load(&dir, models::file(tier), cfg.clone()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("load: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "loaded {} in {:.1?}, rss {:.0} MB",
        models::file(tier).name,
        t.elapsed(),
        peak_rss_mb()
    );
    let Ok(f) = std::fs::File::open(&docs) else {
        eprintln!("{docs}: cannot open");
        return ExitCode::FAILURE;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let locator = v["locator"].as_str().unwrap_or("").to_owned();
        if !wanted.iter().any(|w| locator.contains(w.as_str())) {
            continue;
        }
        let Ok(doc) = serde_json::from_value::<Document>(v["document"].clone()) else {
            continue;
        };
        let (card_text, lang) = cards.get(&locator).cloned().unwrap_or_default();
        let lang = lang.or_else(|| Some("en".to_owned()));
        let card = FileCard {
            lang: lang.clone(),
            ..FileCard::default()
        };
        let o = outline::outline(&doc, &OutlineConfig::default(), None);
        let dcfg = DigestConfig {
            max_tokens: cfg.digest_tokens,
            card_tokens: cfg.card_tokens,
            ..DigestConfig::default()
        };
        let d = digest::build(&doc, &card_text, &o, &dcfg, None);
        println!(
            "\n==== {locator}: {} pages, ~{} tokens; digest {} tokens ({})",
            doc.pages.len(),
            d.doc_tokens,
            d.tokens,
            d.method.as_str()
        );
        // 1. One call over the digest.
        let input = SummaryInput {
            card: &card,
            card_text: &card_text,
            subject: None,
            text_head: &d.body,
            lang: lang.as_deref(),
            deadline: None,
        };
        let t = Instant::now();
        match llm.summarize_with_stats(&input) {
            Ok((s, st)) => println!(
                "single: {:.1} s (prompt {} tok, prefill {:.1?}, {} out tok), rss {:.0} MB, checked={} {:?}\n  summary: {}\n  subject: {}\n  title_en: {}",
                t.elapsed().as_secs_f64(),
                st.prompt_tokens,
                st.prefill,
                st.output_tokens,
                peak_rss_mb(),
                s.checked,
                s.warnings,
                s.text,
                s.subject.unwrap_or_default(),
                s.title_en.unwrap_or_default()
            ),
            Err(e) => println!("single: error {e}"),
        }
        // 2. Map-reduce.
        let ps = digest::map_parts(&doc, &o, &dcfg, &mr, None);
        let outline_text = o.render(dcfg.max_tokens / 4);
        let t = Instant::now();
        let mut prompt_tokens = 0usize;
        let mut generate = |step: Step, user: &str| -> quail_core::Result<String> {
            let p = prompt::chatml(prompt::SYSTEM, user);
            let (raw, st) = llm.generate_with(&p, step, Instant::now() + cfg.timeout)?;
            prompt_tokens += st.prompt_tokens;
            Ok(raw)
        };
        match mapreduce::map_reduce(
            &card_text,
            &outline_text,
            &ps,
            lang.as_deref(),
            &mut generate,
        ) {
            Ok(r) => {
                println!(
                    "map_reduce: {:.1} s, {} calls ({} parts, {} prompt tok), {} map fallbacks, rss {:.0} MB, checked={} {:?}\n  summary: {}\n  subject: {}\n  title_en: {}",
                    t.elapsed().as_secs_f64(),
                    r.calls,
                    ps.len(),
                    prompt_tokens,
                    r.map_fallbacks,
                    peak_rss_mb(),
                    r.summary.checked,
                    r.summary.warnings,
                    r.summary.text,
                    r.summary.subject.clone().unwrap_or_default(),
                    r.summary.title_en.clone().unwrap_or_default()
                );
                for (i, (title, s)) in r.parts.iter().enumerate() {
                    println!("  [{}] {title}: {s}", i + 1);
                }
            }
            Err(e) => println!("map_reduce: error {e}"),
        }
    }
    ExitCode::SUCCESS
}

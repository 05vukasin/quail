//! Summarizes one text file with the local model and prints the result, the fact check and the
//! timings. A smoke test for the `llm` feature.
//!
//! ```text
//! cargo run -p quail-summary --features llm --release --example llm_probe -- FILE.txt [small|default]
//! ```
//!
//! The model must be in `quail_summary::models::model_dir()` (see the `fetch_models` example).
//! The card is minimal (no other analyzers run here): this checks the runtime, the prompt, the
//! grammar and the fact check, not the card.

use std::process::ExitCode;
use std::time::Instant;

use quail_summary::models::{self, Tier};
use quail_summary::{FileCard, LlmConfig, LlmSummarizer, SummaryInput, extractive, tokens};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: llm_probe FILE.txt [small|default]");
        return ExitCode::FAILURE;
    };
    let tier = args
        .next()
        .and_then(|t| Tier::parse(&t))
        .unwrap_or(Tier::Default);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(dir) = models::model_dir() else {
        eprintln!("no model directory; set {}", models::MODELS_ENV);
        return ExitCode::FAILURE;
    };
    let cfg = LlmConfig {
        enabled: true,
        ..LlmConfig::default()
    };
    let t = Instant::now();
    let llm = match LlmSummarizer::load(&dir, models::file(tier), cfg) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("load: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("loaded {} in {:.1?}", models::file(tier).name, t.elapsed());
    let card = FileCard {
        lang: Some("en".to_owned()),
        ..FileCard::default()
    };
    let card_text = card.text().text;
    let pages = vec![(0, text.clone())];
    let subject = extractive::subject(&pages);
    let head = tokens::head(&text, llm.config().head_tokens);
    let input = SummaryInput {
        card: &card,
        card_text: &card_text,
        subject: subject.as_ref(),
        text_head: head,
        lang: Some("en"),
        deadline: None,
    };
    match llm.summarize_with_stats(&input) {
        Ok((s, st)) => {
            println!("summary:  {}", s.text);
            println!("subject:  {}", s.subject.unwrap_or_default());
            println!("title_en: {}", s.title_en.unwrap_or_default());
            println!("checked:  {} {:?}", s.checked, s.warnings);
            println!(
                "prompt {} tok in {:.1?}, output {} tok in {:.1?}; head estimate {} vs {} real",
                st.prompt_tokens,
                st.prefill,
                st.output_tokens,
                st.generate,
                tokens::estimate(head),
                llm.count_tokens(head)
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("summarize: {e}");
            ExitCode::FAILURE
        }
    }
}

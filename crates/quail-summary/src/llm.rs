//! `summary.llm@1`: a local GGUF model (llama.cpp through `llama-cpp-2`) writes
//! `{summary, subject, title_en}`; every output is fact-checked and falls back to the
//! extractive summary when it fails. Feature `llm`, `Profile::Deep`, and opt-in: the analyzer
//! applies only when [`LlmConfig::enabled`] is set.
//!
//! - Input: the card text plus the first [`LlmConfig::head_tokens`] estimated tokens of the
//!   document; the fixed instructions come first ([`crate::prompt`]).
//! - Decoding: greedy (temperature 0), thinking off, output constrained by a grammar compiled
//!   from [`crate::prompt::SCHEMA`].
//! - Memory: one model (mmapped weights) per [`LlmSummarizer`], one context per call, and one
//!   call at a time per summarizer, so the resident set stays at one model plus one KV cache.
//! - Cache: [`CacheKey`] of card text, text head and [`SummarizerId`].
//!
//! llama.cpp has one process-wide backend; it is initialized once, on first use, and never
//! freed (a `OnceLock`, the only global state of the crate).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, Error, Profile, Result, Value};

use crate::analyzer::{AFTER, card_of};
use crate::cache::{CacheKey, SummaryCache};
use crate::extractive;
use crate::factcheck;
use crate::keys;
use crate::models::{self, ModelFile};
use crate::prompt;
use crate::summarizer::{
    Method, Summarizer, SummarizerId, Summary, SummaryInput, checked_or_extractive,
};
use crate::tokens;

/// Settings of the LLM step.
#[derive(Clone, Debug)]
pub struct LlmConfig {
    /// Opt-in switch: the analyzer does nothing unless this is set.
    pub enabled: bool,
    /// Threads for prefill and generation.
    pub threads: u16,
    /// Context size in tokens.
    pub n_ctx: u32,
    /// Estimated tokens of document text in the prompt.
    pub head_tokens: usize,
    /// Token budget of the card in the prompt.
    pub card_tokens: usize,
    /// Generated tokens at most.
    pub max_output_tokens: u32,
    /// Wall time per file at most (prefill and generation).
    pub timeout: Duration,
    /// Instrument kinds whose card already says everything (no model call).
    pub skip_kinds: Vec<String>,
}

impl Default for LlmConfig {
    /// Disabled; 4 threads, 4,096-token context, 800 text tokens, 300 card tokens, 200 output
    /// tokens, 60 s, invoices and purchase orders skipped.
    fn default() -> Self {
        Self {
            enabled: false,
            threads: 4,
            n_ctx: 4096,
            head_tokens: 800,
            card_tokens: 300,
            max_output_tokens: 200,
            timeout: Duration::from_mins(1),
            skip_kinds: vec!["invoice".to_owned(), "po".to_owned()],
        }
    }
}

/// Token counts and times of one generation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenStats {
    /// Prompt tokens.
    pub prompt_tokens: usize,
    /// Generated tokens.
    pub output_tokens: usize,
    /// Prefill time.
    pub prefill: Duration,
    /// Generation time.
    pub generate: Duration,
}

static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();

/// The process-wide llama.cpp backend. With `dynamic-backends` the CPU backend libraries are
/// loaded first: from `$QUAIL_LLM_BACKENDS` when set, else from the build's backend directory.
fn backend() -> Result<&'static LlamaBackend> {
    BACKEND
        .get_or_init(|| {
            llama_cpp_2::send_logs_to_tracing(
                llama_cpp_2::LogOptions::default().with_logs_enabled(false),
            );
            match std::env::var_os("QUAIL_LLM_BACKENDS") {
                Some(dir) if !dir.is_empty() => {
                    llama_cpp_2::llama_backend::load_backends_from_path(Path::new(&dir));
                }
                _ => llama_cpp_2::llama_backend::load_backends(),
            }
            LlamaBackend::init().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| Error::Resource(format!("llama.cpp backend: {e}")))
}

/// A GGUF model behind the [`Summarizer`] interface.
pub struct LlmSummarizer {
    model: LlamaModel,
    file: &'static ModelFile,
    grammar: String,
    cfg: LlmConfig,
    lock: Mutex<()>,
}

impl LlmSummarizer {
    /// Loads the manifest model `file` from `dir` after checking its SHA-256.
    pub fn load(dir: &Path, file: &'static ModelFile, cfg: LlmConfig) -> Result<Self> {
        let path = models::verify(dir, file)?;
        Self::load_unverified(&path, file, cfg)
    }

    /// Loads `path` as the manifest model `file` without hashing it (tests, benchmarks).
    pub fn load_unverified(path: &Path, file: &'static ModelFile, cfg: LlmConfig) -> Result<Self> {
        let backend = backend()?;
        let model = LlamaModel::load_from_file(backend, path, &LlamaModelParams::default())
            .map_err(|e| Error::Resource(format!("{}: {e}", path.display())))?;
        let grammar = llama_cpp_2::json_schema_to_grammar(prompt::SCHEMA)
            .map_err(|e| Error::Resource(format!("output grammar: {e}")))?;
        Ok(Self {
            model,
            file,
            grammar,
            cfg,
            lock: Mutex::new(()),
        })
    }

    /// The settings.
    pub fn config(&self) -> &LlmConfig {
        &self.cfg
    }

    /// Tokens of `text` under this model's tokenizer (to calibrate [`crate::tokens`]).
    pub fn count_tokens(&self, text: &str) -> usize {
        self.model
            .vocab()
            .tokenize(text.as_bytes(), false, false)
            .len()
    }

    /// Runs `prompt` (special tokens parsed) under the output grammar until end of generation,
    /// `max_output_tokens` or `deadline`; returns the text and its stats.
    pub fn generate(&self, prompt: &str, deadline: Instant) -> Result<(String, GenStats)> {
        let _one_at_a_time = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let res = |e: String| Error::Resource(e);
        let backend = backend()?;
        let vocab = self.model.vocab();
        let toks = vocab.tokenize(prompt.as_bytes(), false, true);
        let max_out = self.cfg.max_output_tokens as usize;
        if toks.len() + max_out >= self.cfg.n_ctx as usize {
            return Err(res(format!(
                "prompt of {} tokens does not fit a context of {}",
                toks.len(),
                self.cfg.n_ctx
            )));
        }
        let threads = i32::from(self.cfg.threads.max(1));
        let n_batch = 512u32;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.cfg.n_ctx))
            .with_n_batch(n_batch)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = self
            .model
            .new_context(backend, params)
            .map_err(|e| res(format!("context: {e}")))?;
        let mut stats = GenStats {
            prompt_tokens: toks.len(),
            ..GenStats::default()
        };

        let t0 = Instant::now();
        let mut batch = LlamaBatch::new(n_batch as usize, 1);
        let mut pos: i32 = 0;
        let mut done = 0usize;
        for chunk in toks.chunks(n_batch as usize) {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            batch.clear();
            done += chunk.len();
            let last_chunk = done == toks.len();
            for (i, &t) in chunk.iter().enumerate() {
                let is_last = last_chunk && i + 1 == chunk.len();
                batch
                    .add(t, pos, &[0], is_last)
                    .map_err(|e| res(format!("batch: {e}")))?;
                pos += 1;
            }
            ctx.decode(&mut batch)
                .map_err(|e| res(format!("prefill: {e}")))?;
        }
        stats.prefill = t0.elapsed();

        let t1 = Instant::now();
        let grammar = LlamaSampler::grammar(&self.model, &self.grammar, "root")
            .map_err(|e| res(format!("grammar: {e}")))?;
        let mut sampler = LlamaSampler::chain_simple([grammar, LlamaSampler::greedy()]);
        let mut out: Vec<u8> = Vec::new();
        let mut last = batch.n_tokens() - 1;
        for _ in 0..max_out {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            let tok = sampler.sample(&ctx, last);
            if vocab.is_eog(tok) {
                break;
            }
            out.extend(vocab.token_to_piece(tok, false, None));
            stats.output_tokens += 1;
            batch.clear();
            batch
                .add(tok, pos, &[0], true)
                .map_err(|e| res(format!("batch: {e}")))?;
            pos += 1;
            ctx.decode(&mut batch)
                .map_err(|e| res(format!("generate: {e}")))?;
            last = 0;
        }
        stats.generate = t1.elapsed();
        Ok((String::from_utf8_lossy(&out).into_owned(), stats))
    }

    /// Summarizes `input` and reports the generation stats too.
    pub fn summarize_with_stats(&self, input: &SummaryInput<'_>) -> Result<(Summary, GenStats)> {
        let user = prompt::user_message(input.card_text, input.text_head, input.lang);
        let p = prompt::chatml(prompt::SYSTEM, &user);
        let deadline = Instant::now() + self.cfg.timeout;
        let deadline = input.deadline.map_or(deadline, |d| d.min(deadline));
        let (raw, stats) = self.generate(&p, deadline)?;
        let out = prompt::parse(&raw).map_err(|e| Error::Resource(format!("model output: {e}")))?;
        let subject_src = input.subject.map_or("", |s| s.text.as_str());
        let sources = [input.card_text, input.text_head, subject_src];
        let mut fc = factcheck::check(&format!("{} {}", out.summary, out.subject), &sources, true);
        let title = factcheck::check(&out.title_en, &sources, false);
        fc.claims += title.claims;
        fc.unsupported.extend(title.unsupported);
        let checked = fc.passed();
        let warnings = if checked {
            Vec::new()
        } else {
            vec![format!(
                "fact check failed: {} of {} claims not in the card or text ({})",
                fc.unsupported.len(),
                fc.claims,
                fc.unsupported.join(", ")
            )]
        };
        let nonempty = |s: String| (!s.is_empty()).then_some(s);
        Ok((
            Summary {
                text: out.summary,
                subject: nonempty(out.subject),
                title_en: nonempty(out.title_en),
                method: Method::Llm,
                checked,
                warnings,
            },
            stats,
        ))
    }
}

impl Summarizer for LlmSummarizer {
    fn id(&self) -> SummarizerId {
        SummarizerId {
            name: self.file.id.to_owned(),
            version: 1,
            fingerprint: format!(
                "sha256={} prompt={} greedy ctx={} head={} card={} out={}",
                self.file.sha256,
                prompt::PROMPT_VERSION,
                self.cfg.n_ctx,
                self.cfg.head_tokens,
                self.cfg.card_tokens,
                self.cfg.max_output_tokens
            ),
        }
    }

    fn summarize(&self, input: &SummaryInput<'_>) -> Result<Summary> {
        self.summarize_with_stats(input).map(|(s, _)| s)
    }
}

/// `summary.llm@1`: the [`LlmSummarizer`] (or any [`Summarizer`]) as an analyzer. Deep and
/// opt-in ([`LlmConfig::enabled`]); never part of the default pipeline.
pub struct SummaryLlm {
    summarizer: Arc<dyn Summarizer>,
    cfg: LlmConfig,
    cache: Option<Arc<dyn SummaryCache>>,
}

impl SummaryLlm {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.llm", 1);

    /// The step for `summarizer` with `cfg` (which must have `enabled` set to do anything)
    /// and an optional result cache.
    pub fn new(
        summarizer: Arc<dyn Summarizer>,
        cfg: LlmConfig,
        cache: Option<Arc<dyn SummaryCache>>,
    ) -> Self {
        Self {
            summarizer,
            cfg,
            cache,
        }
    }
}

impl Analyzer for SummaryLlm {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        AFTER
    }

    fn profile(&self) -> Profile {
        Profile::Deep
    }

    fn applies(&self, _: &Ctx<'_>) -> bool {
        self.cfg.enabled
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let card = card_of(ctx);
        if card
            .kind
            .as_deref()
            .is_some_and(|k| self.cfg.skip_kinds.iter().any(|s| s == k))
        {
            return Ok(());
        }
        let card_text = card.render(self.cfg.card_tokens).text;
        let pages = ctx
            .document()
            .map(extractive::head_pages)
            .unwrap_or_default();
        let subject = extractive::subject(&pages);
        let joined: String = pages
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let text_head = tokens::head(&joined, self.cfg.head_tokens);
        if text_head.split_whitespace().count() < 20 {
            return Ok(()); // nothing for a model to read; the extractive summary stands
        }
        out.check_deadline()?;
        let input = SummaryInput {
            card: &card,
            card_text: &card_text,
            subject: subject.as_ref(),
            text_head,
            lang: card.lang.as_deref(),
            deadline: None,
        };
        let id = self.summarizer.id();
        let key = CacheKey::new(&card_text, text_head, &id);
        let (summary, cached) = if let Some(s) = self.cache.as_ref().and_then(|c| c.get(&key)) {
            ((*s).clone(), true)
        } else {
            let s = checked_or_extractive(self.summarizer.as_ref(), &input);
            if let Some(c) = &self.cache {
                c.put(key, Arc::new(s.clone()));
            }
            (s, false)
        };
        for w in &summary.warnings {
            out.warn(format!("{}: {w}", Self::ID));
        }
        let mut f = vec![("text", Value::text(summary.text.as_str()))];
        if let Some(s) = &summary.subject {
            f.push(("subject", Value::text(s.as_str())));
        }
        if let Some(t) = &summary.title_en {
            f.push(("title_en", Value::text(t.as_str())));
        }
        f.push(("method", Value::text(summary.method.as_str())));
        f.push(("model", Value::text(id.name)));
        f.push(("checked", Value::Bool(summary.checked)));
        f.push(("cached", Value::Bool(cached)));
        let conf = if summary.method == Method::Llm {
            0.8
        } else {
            0.6
        };
        out.push(keys::LLM, Value::record(f), conf, None);
        Ok(())
    }
}

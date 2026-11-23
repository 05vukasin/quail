//! The local backend of `summary.llm` ([`crate::llm`]): a GGUF model (llama.cpp through
//! `llama-cpp-2`) behind the [`Summarizer`] interface. Feature `llm`.
//!
//! - Decoding: greedy (temperature 0), thinking off, output constrained by a grammar compiled
//!   from [`crate::prompt::SCHEMA`].
//! - Memory: one model (mmapped weights) per [`LlmSummarizer`], one context per call, and one
//!   call at a time per summarizer, so the resident set stays at one model plus one KV cache.
//!
//! llama.cpp has one process-wide backend; it is initialized once, on first use, and never
//! freed (a `OnceLock`).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use quail_core::{Error, Result};

use crate::factcheck;
use crate::llm::{LlmConfig, checked_output};
use crate::mapreduce::{self, Step};
use crate::models::{self, ModelFile};
use crate::prompt;
use crate::summarizer::{LongInput, Summarizer, SummarizerId, Summary, SummaryInput, Usage};

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
    map_grammar: String,
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
        let map_grammar = llama_cpp_2::json_schema_to_grammar(prompt::MAP_SCHEMA)
            .map_err(|e| Error::Resource(format!("map grammar: {e}")))?;
        Ok(Self {
            model,
            file,
            grammar,
            map_grammar,
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
        self.generate_with(prompt, Step::Reduce, deadline)
    }

    /// [`Self::generate`] under the grammar and output budget of `step`: the summary schema
    /// for [`Step::Reduce`] (and single calls), the part schema for [`Step::Map`].
    pub fn generate_with(
        &self,
        prompt: &str,
        step: Step,
        deadline: Instant,
    ) -> Result<(String, GenStats)> {
        let _one_at_a_time = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let res = |e: String| Error::Resource(e);
        let backend = backend()?;
        let vocab = self.model.vocab();
        let toks = vocab.tokenize(prompt.as_bytes(), false, true);
        let (grammar_src, max_out) = match step {
            Step::Reduce => (&self.grammar, self.cfg.max_output_tokens as usize),
            Step::Map => (&self.map_grammar, self.cfg.map_output_tokens as usize),
        };
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
        let grammar = LlamaSampler::grammar(&self.model, grammar_src, "root")
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
        let user = prompt::user_message_for(
            input.card_text,
            input.text_head,
            input.lang,
            input.card.kind.as_deref(),
        );
        let p = prompt::chatml(prompt::SYSTEM, &user);
        let deadline = Instant::now() + self.cfg.timeout;
        let deadline = input.deadline.map_or(deadline, |d| d.min(deadline));
        let (raw, stats) = self.generate(&p, deadline)?;
        let usage = Usage {
            prompt_tokens: u64::try_from(stats.prompt_tokens).unwrap_or(u64::MAX),
            output_tokens: u64::try_from(stats.output_tokens).unwrap_or(u64::MAX),
        };
        Ok((checked_output(&raw, input, usage)?, stats))
    }
}

impl Summarizer for LlmSummarizer {
    fn id(&self) -> SummarizerId {
        let mr = self.cfg.map_reduce.map_or_else(String::new, |m| {
            format!(
                " mr={}x{}@{} map_out={}",
                m.max_parts, m.part_tokens, m.min_pages, self.cfg.map_output_tokens
            )
        });
        SummarizerId {
            name: self.file.id.to_owned(),
            version: 3,
            fingerprint: format!(
                "sha256={} prompt={} greedy ctx={} digest={} card={} out={}{mr}",
                self.file.sha256,
                prompt::PROMPT_VERSION,
                self.cfg.n_ctx,
                self.cfg.digest_tokens,
                self.cfg.card_tokens,
                self.cfg.max_output_tokens
            ),
        }
    }

    fn summarize(&self, input: &SummaryInput<'_>) -> Result<Summary> {
        self.summarize_with_stats(input).map(|(s, _)| s)
    }

    fn summarize_long(
        &self,
        input: &SummaryInput<'_>,
        long: &LongInput<'_>,
    ) -> Result<(Summary, usize)> {
        if long.parts.len() < 2 {
            return self.summarize(input).map(|s| (s, 1));
        }
        let calls = u32::try_from(long.parts.len() + 1).unwrap_or(u32::MAX);
        let deadline = Instant::now() + self.cfg.timeout * calls;
        let deadline = input.deadline.map_or(deadline, |d| d.min(deadline));
        let mut usage = Usage::default();
        let mut generate = |step: Step, user: &str| -> Result<String> {
            let p = prompt::chatml(prompt::SYSTEM, user);
            let per_call = (Instant::now() + self.cfg.timeout).min(deadline);
            self.generate_with(&p, step, per_call).map(|(raw, st)| {
                usage.add(Usage {
                    prompt_tokens: u64::try_from(st.prompt_tokens).unwrap_or(u64::MAX),
                    output_tokens: u64::try_from(st.output_tokens).unwrap_or(u64::MAX),
                });
                raw
            })
        };
        let doc_sources = input.document.map_or_else(Vec::new, |d| {
            factcheck::document_sources(d, factcheck::DOCUMENT_SOURCE_BYTES)
        });
        let r = mapreduce::map_reduce(
            input.card_text,
            long.outline,
            long.parts,
            &doc_sources,
            input.lang,
            &mut generate,
        )?;
        let mut summary = r.summary;
        summary.usage = usage;
        Ok((summary, r.calls))
    }
}

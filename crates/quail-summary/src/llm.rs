//! `summary.llm@5`: a language model writes `{summary, subject, title_en}` from the document's
//! digest; every output is fact-checked and falls back to the extractive summary when it
//! fails. `Profile::Deep`, and opt-in: the analyzer applies only when [`LlmConfig::enabled`]
//! is set. Two backends implement [`Summarizer`] for it:
//!
//! - feature `llm`: `LlmSummarizer`, a local GGUF model through llama.cpp (`crate::llama`);
//! - feature `api`: `ApiSummarizer`, a model behind an HTTP API (`OpenAI` Chat Completions and
//!   compatible servers, Anthropic Messages, AWS Bedrock Converse; `crate::api`).
//!
//! Both read the same prompt ([`crate::prompt`]), parse the same JSON and run the same fact
//! check ([`checked_output`]), so a summary means the same thing whichever model wrote it.
//!
//! - Input: the document's [digest](crate::digest) within [`LlmConfig::digest_tokens`]
//!   estimated tokens (the card, then the whole text of a short document or the outline,
//!   start and per-section excerpts of a long one); the fixed instructions come first. One
//!   call per document, whatever its length.
//! - Very long documents, when [`LlmConfig::map_reduce`] is set: one call per part (at most
//!   [`MapReduceConfig::max_parts`]) and one over the part summaries ([`crate::mapreduce`]),
//!   each fact-checked.
//! - Cache: [`CacheKey`] of card text, digest and [`SummarizerId`].
//! - Record: `prompt_tokens` and `output_tokens` say what the call cost (summed over every
//!   call of a map-reduce, as the backend reported them; absent for a cached summary).

use std::sync::Arc;
use std::time::Duration;

use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, Profile, Result, Value};

use crate::analyzer::{AFTER, card_of};
use crate::cache::{CacheKey, SummaryCache};
use crate::digest::{self, DigestConfig, DigestMethod, MapReduceConfig};
use crate::extractive;
use crate::factcheck;
use crate::keys;
use crate::outline::{self, OutlineConfig};
use crate::prompt;
use crate::summarizer::{
    LongInput, Method, Summarizer, Summary, SummaryInput, Usage, cacheable, checked_or_fallback,
};

#[cfg(feature = "llm")]
pub use crate::llama::{GenStats, LlmSummarizer};

/// Settings of the LLM step.
#[derive(Clone, Debug)]
pub struct LlmConfig {
    /// Opt-in switch: the analyzer does nothing unless this is set.
    pub enabled: bool,
    /// Threads for prefill and generation (local model).
    pub threads: u16,
    /// Context size in tokens (local model).
    pub n_ctx: u32,
    /// Estimated tokens of the digest in the prompt (card included).
    pub digest_tokens: usize,
    /// Token budget of the card in the prompt.
    pub card_tokens: usize,
    /// Generated tokens at most (local model).
    pub max_output_tokens: u32,
    /// Map-reduce for very long documents (`None`: one call over the digest).
    pub map_reduce: Option<MapReduceConfig>,
    /// Generated tokens at most per map call (local model).
    pub map_output_tokens: u32,
    /// Wall time per file at most (prefill and generation; local model).
    pub timeout: Duration,
    /// Instrument kinds whose card already says everything (no model call).
    pub skip_kinds: Vec<String>,
}

impl Default for LlmConfig {
    /// Disabled; 4 threads, 4,096-token context, a 1,600-token digest, 300 card tokens, 200
    /// output tokens, no map-reduce (120 tokens per map call when set), 60 s per call,
    /// invoices and purchase orders skipped.
    fn default() -> Self {
        Self {
            enabled: false,
            threads: 4,
            n_ctx: 4096,
            digest_tokens: 1600,
            card_tokens: 300,
            max_output_tokens: 200,
            map_reduce: None,
            map_output_tokens: 120,
            timeout: Duration::from_mins(1),
            skip_kinds: vec!["invoice".to_owned(), "po".to_owned()],
        }
    }
}

/// The summary in a model's raw output `raw` for `input`, fact-checked against the card, the
/// text the model saw, the subject sentence and the document: `checked` is set when every
/// claim was found, else a warning names the claims that were not. Fails when `raw` holds no
/// valid output JSON ([`prompt::parse`]).
pub fn checked_output(raw: &str, input: &SummaryInput<'_>, usage: Usage) -> Result<Summary> {
    let out = prompt::parse(raw)
        .map_err(|e| quail_core::Error::Resource(format!("model output: {e}")))?;
    let subject_src = input.subject.map_or("", |s| s.text.as_str());
    let mut sources = vec![input.card_text, input.text_head, subject_src];
    if let Some(doc) = input.document {
        sources.extend(factcheck::document_sources(
            doc,
            factcheck::DOCUMENT_SOURCE_BYTES,
        ));
    }
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
    Ok(Summary {
        text: out.summary,
        subject: nonempty(out.subject),
        title_en: nonempty(out.title_en),
        method: Method::Llm,
        checked,
        warnings,
        usage,
    })
}

/// `summary.llm@5`: any [`Summarizer`] (a local `LlmSummarizer`, an `ApiSummarizer`) as an
/// analyzer. Deep and opt-in ([`LlmConfig::enabled`]); never part of the default pipeline.
/// [`Self::with_profile`] lets a caller that registers it on explicit request (the CLI's
/// `--llm`) run it under a lower profile.
///
/// Reads the digest of the document ([`crate::digest`], [`LlmConfig::digest_tokens`]); with
/// [`LlmConfig::map_reduce`] set, a document of at least
/// [`MapReduceConfig::min_pages`] pages whose text does not fit the digest is summarized part
/// by part ([`Summarizer::summarize_long`]).
pub struct SummaryLlm {
    summarizer: Arc<dyn Summarizer>,
    cfg: LlmConfig,
    cache: Option<Arc<dyn SummaryCache>>,
    profile: Profile,
}

impl SummaryLlm {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.llm", 5);

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
            profile: Profile::Deep,
        }
    }

    /// The lowest profile the step runs in (default [`Profile::Deep`]).
    #[must_use]
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }
}

impl Analyzer for SummaryLlm {
    fn finalizer(&self) -> bool {
        true
    }

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
        self.profile
    }

    fn applies(&self, _: &Ctx<'_>) -> bool {
        self.cfg.enabled
    }

    #[allow(clippy::too_many_lines)] // one step, in order: input, cache, call, record
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
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let pages = extractive::head_pages(doc);
        let subject = extractive::subject(&pages);
        out.check_deadline()?;
        let o = outline::outline(doc, &OutlineConfig::default(), out.deadline());
        let dcfg = DigestConfig {
            max_tokens: self.cfg.digest_tokens,
            card_tokens: self.cfg.card_tokens,
            ..DigestConfig::default()
        };
        let d = digest::build(doc, &card_text, &o, &dcfg, out.deadline());
        if d.body.split_whitespace().count() < 20 {
            return Ok(()); // nothing for a model to read; the extractive summary stands
        }
        out.check_deadline()?;
        let parts = match self.cfg.map_reduce {
            Some(mr) if d.method == DigestMethod::Excerpts => {
                digest::map_parts(doc, &o, &dcfg, &mr, out.deadline())
            }
            _ => Vec::new(),
        };
        let truncated = o.truncated || d.truncated || parts.iter().any(|p| p.truncated);
        if truncated {
            out.problem(format!(
                "{}: deadline reached, the model read the first sections only",
                Self::ID
            ));
        }
        let outline_text = o.render(dcfg.max_tokens / 4);
        let input = SummaryInput {
            card: &card,
            card_text: &card_text,
            subject: subject.as_ref(),
            text_head: &d.body,
            lang: card.lang.as_deref(),
            deadline: out.deadline(),
            document: Some(doc),
        };
        let id = self.summarizer.id();
        let long = parts.len() >= 2;
        let cache_text = if long {
            format!("{}\u{0}map_reduce:{}", d.body, parts.len())
        } else {
            d.body.clone()
        };
        let key = CacheKey::new(&card_text, &cache_text, &id);
        let mut calls = 0usize;
        let (summary, cached) = if let Some(s) = self.cache.as_ref().and_then(|c| c.get(&key)) {
            ((*s).clone(), true)
        } else {
            let got = if long {
                let li = LongInput {
                    outline: &outline_text,
                    parts: &parts,
                };
                self.summarizer.summarize_long(&input, &li).map(|(s, n)| {
                    calls = n;
                    s
                })
            } else {
                calls = 1;
                self.summarizer.summarize(&input)
            };
            let s = checked_or_fallback(self.summarizer.as_ref(), &input, got);
            if let Some(c) = self.cache.as_ref().filter(|_| cacheable(&s, truncated)) {
                c.put(key, Arc::new(s.clone()));
            }
            (s, false)
        };
        for w in &summary.warnings {
            out.warn(format!("{}: {w}", Self::ID));
        }
        let int = |n: u64| Value::Int(i64::try_from(n).unwrap_or(i64::MAX));
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
        f.push((
            "input",
            Value::text(if long { "map_reduce" } else { "digest" }),
        ));
        f.push(("calls", int(u64::try_from(calls).unwrap_or(u64::MAX))));
        if !cached && !summary.usage.is_empty() {
            f.push(("prompt_tokens", int(summary.usage.prompt_tokens)));
            f.push(("output_tokens", int(summary.usage.output_tokens)));
        }
        let conf = if summary.method == Method::Llm {
            0.8
        } else {
            0.6
        };
        out.push(keys::LLM, Value::record(f), conf, None);
        Ok(())
    }
}

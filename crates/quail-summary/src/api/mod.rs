//! A model behind an HTTP API as a [`Summarizer`]: [`ApiSummarizer`]. Feature `api`, never in
//! the default build. **It sends document text** (the card and the digest) to the provider
//! the caller configures.
//!
//! - **Providers** ([`endpoint`]): `OpenAI` Chat Completions, which also covers
//!   `openai_compatible`, `OpenRouter`, Groq, vLLM, Ollama, LM Studio and llama.cpp's
//!   `llama-server` (so a self-hosted fine-tuned model works the same way); Anthropic
//!   Messages; AWS Bedrock Converse with a bearer token. Presets and the environment variable
//!   names are hedgehog's.
//! - **Same guarantees as the local model**: the same prompt ([`crate::prompt`]), the same
//!   JSON parsing and the same fact check ([`crate::llm::checked_output`],
//!   [`crate::mapreduce`]). An output that fails the check, an error, a refusal, a timeout or
//!   malformed JSON makes [`SummaryLlm`](crate::SummaryLlm) fall back to the extractive summary
//!   with a warning; it never fails the file.
//! - **Decoding**: temperature 0 (not sent to Anthropic, whose current models reject sampling
//!   parameters), a `max_tokens` cap, JSON mode where the protocol has one, thinking off
//!   (`chat_template_kwargs.enable_thinking = false` for vLLM-style servers,
//!   `reasoning.enabled = false` for `OpenRouter`, `thinking = disabled` and
//!   `output_config.effort = low` for Anthropic). A gateway that rejects one of these optional
//!   fields (400/422) gets the request again without them, once per summarizer, with a
//!   warning.
//! - **Robustness** ([`http`]): per-call timeouts within the file's deadline, bounded retries
//!   with exponential backoff that honour `retry-after`, a process-wide concurrency cap
//!   (`AI_MAX_CONCURRENT_LLM`, default 4).
//! - **Cache**: the [`SummarizerId`] fingerprint holds the protocol, the base URL's host, the
//!   model, the prompt versions and the decoding settings, never the key.
//! - **Keys** ([`Secret`]) are never logged, never in `Debug` output, never in errors.
//! - **Training log** ([`log`], opt-in): one JSONL row per call with the text the model read
//!   and wrote. It contains document text and must stay private.

pub mod endpoint;
pub mod env;
pub mod http;
pub mod log;
pub mod wire;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use quail_core::{Error, Result};

pub use endpoint::{Connect, Endpoint, Kind, Secret};
pub use env::Env;
pub use http::{Limiter, RetryPolicy};
pub use log::TrainingLog;

use crate::factcheck;
use crate::llm::checked_output;
use crate::mapreduce::{self, Step};
use crate::prompt;
use crate::summarizer::{LongInput, Summarizer, SummarizerId, Summary, SummaryInput, Usage};
use http::{Client, Failure};
use wire::Decoding;

/// Bump when [`API_JSON`] or [`API_MAP_JSON`] change: part of the fingerprint.
pub const API_PROMPT_VERSION: u32 = 1;

/// Appended to [`prompt::SYSTEM`]: an API model has no grammar, so the reply format is
/// spelled out.
pub const API_JSON: &str = " Reply with exactly one JSON object with the string keys summary, \
subject and title_en, and nothing before or after it.";

/// Appended to [`prompt::SYSTEM`] for a map step.
pub const API_MAP_JSON: &str = " Reply with exactly one JSON object with the string key summary, and nothing before or after it.";

/// Settings of an [`ApiSummarizer`].
#[derive(Clone, Debug)]
pub struct ApiConfig {
    /// Output tokens at most for a summary (single call and reduce step).
    pub max_tokens: u32,
    /// Output tokens at most for a map step.
    pub map_max_tokens: u32,
    /// Sampling temperature (`None`: not sent). Never sent to Anthropic.
    pub temperature: Option<f64>,
    /// Ask for a JSON object where the protocol can (`OpenAI` `response_format`).
    pub json_mode: bool,
    /// Switch thinking off where the provider has a switch.
    pub thinking_off: bool,
    /// Anthropic `output_config.effort` (`None`: not sent).
    pub effort: Option<String>,
    /// Time per HTTP attempt at most.
    pub call_timeout: Duration,
    /// Time per summary at most, retries included (per call for map-reduce); the file's
    /// deadline ([`SummaryInput::deadline`]) is never exceeded.
    pub timeout: Duration,
    /// Retries.
    pub retry: RetryPolicy,
    /// Concurrency limiter (`None`: the process-wide one, [`Limiter::global`], capped at the
    /// endpoint's `max_concurrent`).
    pub limiter: Option<Arc<Limiter>>,
    /// Opt-in training log ([`log`]).
    pub log: Option<Arc<TrainingLog>>,
}

impl Default for ApiConfig {
    /// 1,024 output tokens (512 per map step), temperature 0, JSON mode, thinking off,
    /// Anthropic effort `low`, 60 s per attempt, 120 s per summary, 3 retries from 1 s, the
    /// global limiter, no log.
    fn default() -> Self {
        Self {
            max_tokens: 1024,
            map_max_tokens: 512,
            temperature: Some(0.0),
            json_mode: true,
            thinking_off: true,
            effort: Some("low".to_owned()),
            call_timeout: Duration::from_mins(1),
            timeout: Duration::from_mins(2),
            retry: RetryPolicy::default(),
            limiter: None,
            log: None,
        }
    }
}

/// A model behind an HTTP API (see the module docs).
pub struct ApiSummarizer {
    ep: Endpoint,
    cfg: ApiConfig,
    client: Client,
    limiter: Arc<Limiter>,
    /// Set once the provider rejected an optional field: later requests leave them out.
    plain: AtomicBool,
}

impl std::fmt::Debug for ApiSummarizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiSummarizer")
            .field("endpoint", &self.ep)
            .field("plain", &self.plain.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl ApiSummarizer {
    /// A summarizer for `ep` with `cfg`.
    pub fn new(ep: Endpoint, cfg: ApiConfig) -> Self {
        let limiter = cfg.limiter.clone().unwrap_or_else(|| {
            let g = Limiter::global();
            g.set_cap(ep.max_concurrent);
            g
        });
        Self {
            ep,
            cfg,
            client: Client::new(),
            limiter,
            plain: AtomicBool::new(false),
        }
    }

    /// Resolves `req` against `env` ([`Endpoint::resolve`]) and builds the summarizer.
    pub fn connect(req: &Connect, env: &Env, cfg: ApiConfig) -> Result<Self> {
        Ok(Self::new(Endpoint::resolve(req, env)?, cfg))
    }

    /// The endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.ep
    }

    /// The settings.
    pub fn config(&self) -> &ApiConfig {
        &self.cfg
    }

    fn decoding(&self, step: Step) -> Decoding {
        let anthropic = self.ep.kind == Kind::Anthropic;
        Decoding {
            max_tokens: match step {
                Step::Reduce => self.cfg.max_tokens,
                Step::Map => self.cfg.map_max_tokens,
            },
            temperature: if anthropic {
                None
            } else {
                self.cfg.temperature
            },
            json_mode: self.cfg.json_mode,
            thinking_off: self.cfg.thinking_off,
            effort: if anthropic {
                self.cfg.effort.clone()
            } else {
                None
            },
        }
    }

    fn system(step: Step) -> String {
        match step {
            Step::Reduce => format!("{}{API_JSON}", prompt::SYSTEM),
            Step::Map => format!("{}{API_MAP_JSON}", prompt::SYSTEM),
        }
    }

    /// `s` with the key replaced by `***` (an error body that echoes it stays safe).
    fn redact(&self, s: String) -> String {
        match &self.ep.key {
            Some(k) if k.expose().len() >= 4 => s.replace(k.expose(), "***"),
            _ => s,
        }
    }

    /// One model call: the raw text, its usage and notes (an optional field was dropped).
    pub fn call(
        &self,
        step: Step,
        system: &str,
        user: &str,
        deadline: Instant,
    ) -> Result<(String, Usage, Vec<String>)> {
        let _permit = self.limiter.acquire(Some(deadline))?;
        let d = self.decoding(step);
        let url = self.ep.url();
        let mut notes = Vec::new();
        loop {
            let plain = self.plain.load(Ordering::Relaxed);
            let req = wire::request(&self.ep, system, user, &d, plain);
            let got = self.client.post(
                &url,
                &req.headers,
                &req.body,
                self.cfg.call_timeout,
                deadline,
                self.cfg.retry,
                &std::thread::sleep,
            );
            match got {
                Ok(resp) => {
                    let (text, usage) = wire::parse(self.ep.kind, &resp.body).map_err(|e| {
                        Error::Resource(self.redact(format!("{}: {e}", self.ep.preset)))
                    })?;
                    return Ok((text, usage, notes));
                }
                Err(Failure::Status(r))
                    if matches!(r.status, 400 | 422)
                        && !plain
                        && wire::has_optional(&self.ep, &d) =>
                {
                    self.plain.store(true, Ordering::Relaxed);
                    notes.push(self.redact(format!(
                        "{} rejected an optional setting (HTTP {}: {}); sending requests \
                         without temperature, JSON mode and thinking switches",
                        self.ep.preset,
                        r.status,
                        wire::error_message(&r.body)
                    )));
                }
                Err(Failure::Status(r)) => {
                    let hint = if r.status == 404 {
                        format!(
                            " (is model {:?} served at {}?)",
                            self.ep.model,
                            self.ep.host()
                        )
                    } else {
                        String::new()
                    };
                    return Err(Error::Resource(self.redact(format!(
                        "{}: HTTP {}: {}{hint}",
                        self.ep.preset,
                        r.status,
                        wire::error_message(&r.body)
                    ))));
                }
                Err(Failure::Timeout) => return Err(Error::Timeout),
                Err(Failure::Connection(e)) => {
                    return Err(Error::Resource(
                        self.redact(format!("{}: {e}", self.ep.preset)),
                    ));
                }
            }
        }
    }

    fn deadline(&self, input: &SummaryInput<'_>, calls: u32) -> Instant {
        let own = Instant::now() + self.cfg.timeout.saturating_mul(calls.max(1));
        input.deadline.map_or(own, |d| d.min(own))
    }

    fn log_row(&self, row: &log::Row<'_>) {
        if let Some(l) = &self.cfg.log {
            // A failed log write loses one training row, never the summary.
            let _ = l.write(row);
        }
    }

    fn row<'a>(
        &'a self,
        fp: &'a str,
        step: &'a str,
        system: &'a str,
        user: &'a str,
    ) -> log::Row<'a> {
        log::Row {
            summarizer: self.id().to_string(),
            fingerprint: fp,
            provider: self.ep.preset,
            model: &self.ep.model,
            step,
            system,
            user,
            ..log::Row::default()
        }
    }
}

impl Summarizer for ApiSummarizer {
    fn id(&self) -> SummarizerId {
        let d = self.decoding(Step::Reduce);
        SummarizerId {
            name: format!("{}:{}", self.ep.preset, self.ep.model),
            version: 1,
            fingerprint: format!(
                "api={} host={} model={} prompt={} api_prompt={} temp={} max_tokens={} \
                 map_tokens={} json={} thinking_off={} effort={}",
                self.ep.kind.as_str(),
                self.ep.host(),
                self.ep.model,
                prompt::PROMPT_VERSION,
                API_PROMPT_VERSION,
                d.temperature
                    .map_or_else(|| "none".to_owned(), |t| t.to_string()),
                self.cfg.max_tokens,
                self.cfg.map_max_tokens,
                self.cfg.json_mode,
                self.cfg.thinking_off,
                d.effort.as_deref().unwrap_or("none"),
            ),
        }
    }

    fn summarize(&self, input: &SummaryInput<'_>) -> Result<Summary> {
        let user = prompt::user_message_for(
            input.card_text,
            input.text_head,
            input.lang,
            input.card.kind.as_deref(),
        );
        let system = Self::system(Step::Reduce);
        let deadline = self.deadline(input, 1);
        let got = self.call(Step::Reduce, &system, &user, deadline);
        let fp = if self.cfg.log.is_some() {
            self.id().fingerprint
        } else {
            String::new()
        };
        let base = || log::Row {
            card_text: Some(input.card_text),
            digest: Some(input.text_head),
            ..self.row(&fp, "single", &system, &user)
        };
        let (raw, usage, notes) = match got {
            Ok(r) => r,
            Err(e) => {
                self.log_row(&log::Row {
                    error: Some(e.to_string()),
                    ..base()
                });
                return Err(e);
            }
        };
        let s = checked_output(&raw, input, usage);
        let findings: Vec<String> = match &s {
            Ok(s) if !s.checked => s.warnings.clone(),
            _ => Vec::new(),
        };
        self.log_row(&log::Row {
            output: Some(&raw),
            error: s.as_ref().err().map(ToString::to_string),
            checked: s.as_ref().ok().map(|s| s.checked),
            unsupported: &findings,
            prompt_tokens: usage.prompt_tokens,
            output_tokens: usage.output_tokens,
            ..base()
        });
        let mut s = s?;
        s.warnings.extend(notes);
        Ok(s)
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
        let deadline = self.deadline(input, calls);
        let fp = if self.cfg.log.is_some() {
            self.id().fingerprint
        } else {
            String::new()
        };
        let mut usage = Usage::default();
        let mut notes: Vec<String> = Vec::new();
        let mut generate = |step: Step, user: &str| -> Result<String> {
            let system = Self::system(step);
            let per_call = (Instant::now() + self.cfg.timeout).min(deadline);
            let name = match step {
                Step::Map => "map",
                Step::Reduce => "reduce",
            };
            match self.call(step, &system, user, per_call) {
                Ok((raw, u, n)) => {
                    usage.add(u);
                    notes.extend(n);
                    self.log_row(&log::Row {
                        output: Some(&raw),
                        prompt_tokens: u.prompt_tokens,
                        output_tokens: u.output_tokens,
                        ..self.row(&fp, name, &system, user)
                    });
                    Ok(raw)
                }
                Err(e) => {
                    self.log_row(&log::Row {
                        error: Some(e.to_string()),
                        ..self.row(&fp, name, &system, user)
                    });
                    Err(e)
                }
            }
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
        summary.warnings.extend(notes);
        Ok((summary, r.calls))
    }
}

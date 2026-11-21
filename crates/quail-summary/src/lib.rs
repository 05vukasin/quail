//! Per-file cards and English summaries for LLM agents. Namespace `summary.*`.
//!
//! - [`SummaryCard`] (`summary.card@2`, Fast): a deterministic **file card** built only from
//!   the facts of the other steps (type, title, language, pages, own number, cited documents,
//!   parties with roles, dates by role, amounts, identifiers, price items, sketches, producer,
//!   flags), stored as a record and rendered as compact `key: value` text under a hard token
//!   budget ([`tokens`]).
//! - [`SummaryExtractive`] (`summary.extractive@2`, Fast): 1–3 English sentences without a
//!   model: a template sentence from the card plus the document's subject sentence
//!   ([`extractive`]).
//! - [`SummaryOutline`] (`summary.outline@1`, Fast): the document's headings (DOCX heading
//!   blocks, `CHAPTER 3`, `Article 5`, `2.1 Term`, capitals, larger type) with page ranges,
//!   bounded, or fixed page windows ([`outline`]).
//! - [`SummaryDigest`] (`summary.digest@1`, Standard): the bounded **summary input** of a document
//!   of any length, 2 or 2,000 pages: card, outline, start and the salient sentences of every
//!   section, copied verbatim, within a token budget ([`digest`]). Any model can read it;
//!   [`digest::map_parts`] splits a very long document for a map-reduce summary.
//! - `SummaryLlm` (`summary.llm@2`, Deep, feature `llm`, opt-in): a local GGUF model reads the
//!   digest (or, for very long documents and when configured, summarizes parts first and then
//!   combines them) and writes `{summary, subject, title_en}` under a JSON grammar; every
//!   output is [fact-checked](factcheck) against the card and the text it saw, and falls back
//!   to the extractive summary when a claim is not found.
//!
//! Summaries are always in English. Names, numbers and identifiers are copied verbatim in their
//! original script; a non-English subject sentence is quoted and marked with its language.
//!
//! # Ordering
//!
//! The summary steps read `ctx.facts`, so they must run after every fact producer: they are
//! finalizers ([`quail_core::Analyzer::finalizer`]), which the engine runs after every other
//! step. They also declare `after` on every artifact a workspace step provides ([`AFTER`]).

pub mod analyzer;
pub mod cache;
pub mod card;
pub mod digest;
pub mod extractive;
pub mod factcheck;
pub mod format;
pub mod input;
#[cfg(feature = "llm")]
pub mod llm;
pub mod mapreduce;
pub mod models;
pub mod outline;
pub mod prompt;
pub mod summarizer;
pub mod tokens;

pub use analyzer::{AFTER, SummaryCard, SummaryDigest, SummaryExtractive, SummaryOutline, card_of};
pub use card::{CardText, FileCard};
pub use digest::{Digest, DigestConfig, DigestMethod, MapPart, MapReduceConfig};
pub use extractive::Subject;
#[cfg(feature = "llm")]
pub use llm::{LlmConfig, LlmSummarizer, SummaryLlm};
pub use outline::{Outline, OutlineConfig, OutlineEntry, OutlineMethod};
pub use summarizer::{
    ExtractiveSummarizer, LongInput, Method, Summarizer, SummarizerId, Summary, SummaryInput,
};

/// The keys this crate emits.
pub mod keys {
    use quail_core::FactKey;

    /// The file card (Record: `format kind ordinal title title_source lang script pages number
    /// cites cites_total parties parties_total dates amounts currencies ids personal_ids items
    /// sketch producer author company flags`, absent fields left out; see the README).
    pub const CARD: FactKey = FactKey::new("summary.card");
    /// The card as compact `key: value` lines within the token budget (Text).
    pub const CARD_TEXT: FactKey = FactKey::new("summary.card_text");
    /// Estimated tokens of [`CARD_TEXT`] (Int, see [`crate::tokens`]).
    pub const CARD_TOKENS: FactKey = FactKey::new("summary.card_tokens");
    /// 1–3 sentence English summary without a model (Text).
    pub const EXTRACTIVE: FactKey = FactKey::new("summary.extractive");
    /// The subject sentence, verbatim (Record: `text`, `lang`?, `method` = `clause`,
    /// `recital`, `operative` or `textrank`, `label`?), evidence: its page.
    pub const SUBJECT: FactKey = FactKey::new("summary.subject");
    /// The document's headings with page ranges (Record: `method` = `headings` or `windows`,
    /// `total`, `pages`, `sections` = list of `title level page end_page source`, pages
    /// zero-based; `truncated`?).
    pub const OUTLINE: FactKey = FactKey::new("summary.outline");
    /// The bounded summary input of a document of any length (Text: `FACTS` = the card, then
    /// the whole `TEXT` when it fits, else `OUTLINE`, `START` and per-section `EXCERPTS`
    /// copied verbatim).
    pub const DIGEST: FactKey = FactKey::new("summary.digest");
    /// Estimated tokens of [`DIGEST`] (Int, see [`crate::tokens`]).
    pub const DIGEST_TOKENS: FactKey = FactKey::new("summary.digest_tokens");
    /// The LLM summary (Record: `text`, `subject`?, `title_en`?, `method` = `llm` or
    /// `extractive_fallback`, `model`, `checked`, `cached`, `input` = `digest` or
    /// `map_reduce`, `calls`). Feature `llm`.
    pub const LLM: FactKey = FactKey::new("summary.llm");
}

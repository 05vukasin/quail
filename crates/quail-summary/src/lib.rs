//! Per-file cards and English summaries for LLM agents. Namespace `summary.*`.
//!
//! - [`SummaryCard`] (`summary.card@1`, Fast): a deterministic **file card** built only from
//!   the facts of the other steps (type, title, language, pages, own number, cited documents,
//!   parties with roles, dates by role, amounts, identifiers, price items, sketches, producer,
//!   flags), stored as a record and rendered as compact `key: value` text under a hard token
//!   budget ([`tokens`]).
//! - [`SummaryExtractive`] (`summary.extractive@1`, Fast): 1–3 English sentences without a
//!   model: a template sentence from the card plus the document's subject sentence
//!   ([`extractive`]).
//! - `SummaryLlm` (`summary.llm@1`, Deep, feature `llm`, opt-in): a local GGUF model writes
//!   `{summary, subject, title_en}` under a JSON grammar; every output is
//!   [fact-checked](factcheck) against the card and the text it saw, and falls back to the
//!   extractive summary when a claim is not found.
//!
//! Summaries are always in English. Names, numbers and identifiers are copied verbatim in their
//! original script; a non-English subject sentence is quoted and marked with its language.
//!
//! # Ordering
//!
//! The summary steps must run after every fact producer. The engine orders analyzers only by
//! artifacts (`requires` / `after`) and keeps registration order among equals, and the fact
//! producers provide no artifacts. So the summary steps declare `after` on every artifact a
//! workspace step provides ([`AFTER`]: `revised_document`, `ocr_text`, `raster`) and must be
//! **registered last**: then every analyzer that is ready when they are comes first. A core
//! "finalizer" flag that makes this explicit is requested in the PR.

pub mod analyzer;
pub mod cache;
pub mod card;
pub mod extractive;
pub mod factcheck;
pub mod format;
pub mod input;
#[cfg(feature = "llm")]
pub mod llm;
pub mod models;
pub mod prompt;
pub mod summarizer;
pub mod tokens;

pub use analyzer::{AFTER, SummaryCard, SummaryExtractive, card_of};
pub use card::{CardText, FileCard};
pub use extractive::Subject;
#[cfg(feature = "llm")]
pub use llm::{LlmConfig, LlmSummarizer, SummaryLlm};
pub use summarizer::{
    ExtractiveSummarizer, Method, Summarizer, SummarizerId, Summary, SummaryInput,
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
    /// The subject sentence, verbatim (Record: `text`, `lang`?, `method` = `clause` or
    /// `textrank`, `label`?), evidence: its page.
    pub const SUBJECT: FactKey = FactKey::new("summary.subject");
    /// The LLM summary (Record: `text`, `subject`?, `title_en`?, `method` = `llm` or
    /// `extractive_fallback`, `model`, `checked`, `cached`). Feature `llm`.
    pub const LLM: FactKey = FactKey::new("summary.llm");
}

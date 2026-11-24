//! The [`Summarizer`] interface: one English summary from a card and the start of the text.

use std::fmt;
use std::time::Instant;

use quail_core::{Document, Result};

use crate::card::FileCard;
use crate::digest::MapPart;
use crate::extractive::{self, Subject};

/// Name, version and fingerprint of a summarizer. Part of the cache key, so a new model file
/// or prompt never reuses old results.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SummarizerId {
    /// Short name, e.g. `extractive` or `qwen3.5-2b-q4_k_m`.
    pub name: String,
    /// Output version; bump it whenever the output for the same input can change.
    pub version: u32,
    /// What else determines the output: model SHA-256, prompt version, decoding settings.
    pub fingerprint: String,
}

impl fmt::Display for SummarizerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

/// What a summarizer reads.
#[derive(Clone, Copy, Debug)]
pub struct SummaryInput<'a> {
    /// The card, built from the facts.
    pub card: &'a FileCard,
    /// The card rendered as text (what a model sees as FACTS).
    pub card_text: &'a str,
    /// The subject sentence found in the text, if any.
    pub subject: Option<&'a Subject>,
    /// What the summarizer reads of the document: the body of its
    /// [digest](crate::digest::Digest::body) (the whole text of a short document; outline,
    /// start and per-section excerpts of a long one), within a token budget.
    pub text_head: &'a str,
    /// Document language, ISO 639-1.
    pub lang: Option<&'a str>,
    /// When the work must be done (the file's time budget).
    pub deadline: Option<Instant>,
    /// The whole document, when there is one: the fact check also accepts a claim its text
    /// makes outside the digest ([`crate::factcheck::document_sources`]).
    pub document: Option<&'a Document>,
}

/// How a summary was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Template sentence and subject sentence, no model.
    Extractive,
    /// A language model, fact-checked.
    Llm,
    /// A language model was asked, but its output failed (fact check, JSON, timeout) and the
    /// extractive summary stands in.
    ExtractiveFallback,
}

impl Method {
    /// `extractive`, `llm` or `extractive_fallback`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extractive => "extractive",
            Self::Llm => "llm",
            Self::ExtractiveFallback => "extractive_fallback",
        }
    }
}

/// Tokens a summary cost, as the model runtime or the API reported them (zero when unknown).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Prompt (input) tokens, every call summed.
    pub prompt_tokens: u64,
    /// Generated (output) tokens, every call summed.
    pub output_tokens: u64,
}

impl Usage {
    /// Adds the tokens of another call.
    pub fn add(&mut self, other: Usage) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
    }

    /// Whether nothing was counted.
    pub fn is_empty(&self) -> bool {
        self.prompt_tokens == 0 && self.output_tokens == 0
    }
}

/// One summary, always in English (quoted source sentences excepted).
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// One to three sentences.
    pub text: String,
    /// What the document is about, in a few words (LLM) or the subject sentence (extractive).
    pub subject: Option<String>,
    /// English title (LLM only).
    pub title_en: Option<String>,
    /// How it was made.
    pub method: Method,
    /// Every number, date, identifier and name in it was found in the card or the text.
    pub checked: bool,
    /// Why a model's output was not used, when it was not.
    pub warnings: Vec<String>,
    /// Tokens the model calls cost (also when their output was not used).
    pub usage: Usage,
}

/// The parts of a very long document for a hierarchical summary
/// ([`Summarizer::summarize_long`]).
#[derive(Clone, Copy, Debug)]
pub struct LongInput<'a> {
    /// The outline, rendered ([`crate::outline::Outline::render`]).
    pub outline: &'a str,
    /// Contiguous parts with their excerpts ([`crate::digest::map_parts`]).
    pub parts: &'a [MapPart],
}

/// Writes one English summary per file.
pub trait Summarizer: Send + Sync {
    /// Name, version, fingerprint.
    fn id(&self) -> SummarizerId;

    /// Summarizes one file. An error means "no summary": callers fall back to the extractive
    /// one and record the error as a warning.
    fn summarize(&self, input: &SummaryInput<'_>) -> Result<Summary>;

    /// Summarizes a very long file part by part, then the parts together (map-reduce,
    /// [`crate::mapreduce`]); returns the summary and the number of model calls. The default
    /// is one [`Self::summarize`] call over `input`.
    fn summarize_long(
        &self,
        input: &SummaryInput<'_>,
        long: &LongInput<'_>,
    ) -> Result<(Summary, usize)> {
        let _ = long;
        self.summarize(input).map(|s| (s, 1))
    }
}

/// The model-free summarizer: [`extractive::summary`]. Checked by construction (it copies
/// card values and a source sentence).
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractiveSummarizer;

impl Summarizer for ExtractiveSummarizer {
    fn id(&self) -> SummarizerId {
        SummarizerId {
            name: "extractive".to_owned(),
            version: 1,
            fingerprint: String::new(),
        }
    }

    fn summarize(&self, input: &SummaryInput<'_>) -> Result<Summary> {
        Ok(Summary {
            text: extractive::summary(input.card, input.subject),
            subject: input.subject.map(|s| s.text.clone()),
            title_en: None,
            method: Method::Extractive,
            checked: true,
            warnings: Vec::new(),
            usage: Usage::default(),
        })
    }
}

/// The checked summary of `summarizer`, or the extractive one with a warning when it failed
/// or its output did not pass the fact check.
pub fn checked_or_extractive(summarizer: &dyn Summarizer, input: &SummaryInput<'_>) -> Summary {
    checked_or_fallback(summarizer, input, summarizer.summarize(input))
}

/// [`checked_or_extractive`] of an attempt `got` made for `input` (for example by
/// [`Summarizer::summarize_long`]).
pub fn checked_or_fallback(
    summarizer: &dyn Summarizer,
    input: &SummaryInput<'_>,
    got: Result<Summary>,
) -> Summary {
    let fallback = |why: String, usage: Usage| {
        let mut s = ExtractiveSummarizer
            .summarize(input)
            .unwrap_or_else(|_| Summary {
                text: extractive::summary(input.card, input.subject),
                subject: None,
                title_en: None,
                method: Method::Extractive,
                checked: true,
                warnings: Vec::new(),
                usage: Usage::default(),
            });
        s.method = Method::ExtractiveFallback;
        s.warnings.push(why);
        s.usage = usage;
        s
    };
    match got {
        Ok(s) if s.checked => s,
        Ok(s) => fallback(s.warnings.join("; "), s.usage),
        Err(e) => fallback(format!("{}: {e}", summarizer.id()), Usage::default()),
    }
}

/// Whether `summary` may go into a [`SummaryCache`](crate::cache::SummaryCache): only a
/// fact-checked model output made from complete input. A fallback (timeout, failed check,
/// load error) or a summary of input the deadline cut short would otherwise be replayed on
/// every later run, even when the model would now succeed.
pub fn cacheable(summary: &Summary, input_truncated: bool) -> bool {
    summary.method == Method::Llm && summary.checked && !input_truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a fixed text, checked or not, or fails.
    struct Fixed(Option<(&'static str, bool)>);

    impl Summarizer for Fixed {
        fn id(&self) -> SummarizerId {
            SummarizerId {
                name: "fixed".into(),
                version: 1,
                fingerprint: String::new(),
            }
        }
        fn summarize(&self, _: &SummaryInput<'_>) -> Result<Summary> {
            let (text, checked) = self.0.ok_or(quail_core::Error::Timeout)?;
            Ok(Summary {
                text: text.into(),
                subject: None,
                title_en: None,
                method: Method::Llm,
                checked,
                warnings: if checked {
                    Vec::new()
                } else {
                    vec!["fact check failed: number:9".into()]
                },
                usage: Usage::default(),
            })
        }
    }

    #[test]
    fn only_complete_checked_model_output_is_cacheable() {
        let card = FileCard::default();
        let input = SummaryInput {
            card: &card,
            card_text: "doc: report",
            subject: None,
            text_head: "",
            lang: Some("en"),
            deadline: None,
            document: None,
        };
        let ok = checked_or_extractive(&Fixed(Some(("A report.", true))), &input);
        assert!(cacheable(&ok, false));
        // A deadline cut the input: the next run may read all of it.
        assert!(!cacheable(&ok, true));
        // A timed-out call and a failed fact check fall back; neither is cached.
        let timeout = checked_or_extractive(&Fixed(None), &input);
        assert_eq!(timeout.method, Method::ExtractiveFallback);
        assert!(!cacheable(&timeout, false));
        let failed = checked_or_extractive(&Fixed(Some(("A report of 9.", false))), &input);
        assert!(!cacheable(&failed, false));
    }

    #[test]
    fn falls_back_to_extractive() {
        let card = FileCard {
            kind: Some("invoice".into()),
            ..FileCard::default()
        };
        let input = SummaryInput {
            card: &card,
            card_text: "doc: invoice",
            subject: None,
            text_head: "",
            lang: Some("en"),
            deadline: None,
            document: None,
        };
        let ok = checked_or_extractive(&Fixed(Some(("An invoice.", true))), &input);
        assert_eq!((ok.method, ok.text.as_str()), (Method::Llm, "An invoice."));
        let bad = checked_or_extractive(&Fixed(Some(("An invoice for 9.", false))), &input);
        assert_eq!(bad.method, Method::ExtractiveFallback);
        assert_eq!(bad.text, "Invoice.");
        assert_eq!(bad.warnings, vec!["fact check failed: number:9"]);
        let err = checked_or_extractive(&Fixed(None), &input);
        assert_eq!(err.method, Method::ExtractiveFallback);
        assert!(
            err.warnings[0].starts_with("fixed@1: "),
            "{:?}",
            err.warnings
        );
    }
}

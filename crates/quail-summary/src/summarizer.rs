//! The [`Summarizer`] interface: one English summary from a card and the start of the text.

use std::fmt;
use std::time::Instant;

use quail_core::Result;

use crate::card::FileCard;
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
    /// The start of the document text, normalized, within a token budget.
    pub text_head: &'a str,
    /// Document language, ISO 639-1.
    pub lang: Option<&'a str>,
    /// When the work must be done (the file's time budget).
    pub deadline: Option<Instant>,
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
}

/// Writes one English summary per file.
pub trait Summarizer: Send + Sync {
    /// Name, version, fingerprint.
    fn id(&self) -> SummarizerId;

    /// Summarizes one file. An error means "no summary": callers fall back to the extractive
    /// one and record the error as a warning.
    fn summarize(&self, input: &SummaryInput<'_>) -> Result<Summary>;
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
        })
    }
}

/// The checked summary of `summarizer`, or the extractive one with a warning when it failed
/// or its output did not pass the fact check.
pub fn checked_or_extractive(summarizer: &dyn Summarizer, input: &SummaryInput<'_>) -> Summary {
    let fallback = |why: String| {
        let mut s = ExtractiveSummarizer
            .summarize(input)
            .unwrap_or_else(|_| Summary {
                text: extractive::summary(input.card, input.subject),
                subject: None,
                title_en: None,
                method: Method::Extractive,
                checked: true,
                warnings: Vec::new(),
            });
        s.method = Method::ExtractiveFallback;
        s.warnings.push(why);
        s
    };
    match summarizer.summarize(input) {
        Ok(s) if s.checked => s,
        Ok(s) => fallback(s.warnings.join("; ")),
        Err(e) => fallback(format!("{}: {e}", summarizer.id())),
    }
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
            })
        }
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

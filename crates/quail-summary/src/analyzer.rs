//! The analyzers: `summary.card@1` and `summary.extractive@1`.

use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, Evidence, Profile, Result, Value};

use crate::card::{DEFAULT_MAX_TOKENS, FileCard};
use crate::extractive;
use crate::keys;

/// Artifacts every fact-producing step of the workspace may provide. Declared as `after` so
/// the summary steps run once those steps are done (see the crate docs on ordering).
pub const AFTER: &[&str] = &["revised_document", "ocr_text", "raster"];

/// `summary.card@1`: the file card from every fact produced before it.
///
/// Emits [`keys::CARD`] (record), [`keys::CARD_TEXT`] (text, at most `max_tokens` estimated
/// tokens) and [`keys::CARD_TOKENS`].
#[derive(Clone, Copy, Debug)]
pub struct SummaryCard {
    max_tokens: usize,
}

impl SummaryCard {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.card", 1);

    /// With the default budget, [`DEFAULT_MAX_TOKENS`].
    pub const fn new() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// With a budget of `max_tokens` estimated tokens for the card text (at least 32).
    pub const fn with_max_tokens(max_tokens: usize) -> Self {
        Self {
            max_tokens: if max_tokens < 32 { 32 } else { max_tokens },
        }
    }
}

impl Default for SummaryCard {
    fn default() -> Self {
        Self::new()
    }
}

/// The card of the file in `ctx`.
pub fn card_of(ctx: &Ctx<'_>) -> FileCard {
    FileCard::from_facts(ctx.facts, ctx.document().map(|d| d.pages.len()))
}

impl Analyzer for SummaryCard {
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
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let card = card_of(ctx);
        let text = card.render(self.max_tokens);
        out.fact(keys::CARD, card.to_value());
        out.fact(keys::CARD_TEXT, Value::text(text.text));
        out.fact(
            keys::CARD_TOKENS,
            Value::Int(i64::try_from(text.tokens).unwrap_or(i64::MAX)),
        );
        Ok(())
    }
}

/// `summary.extractive@1`: a 1–3 sentence English summary without a model.
///
/// Emits [`keys::EXTRACTIVE`] (text) and, when a subject sentence is found,
/// [`keys::SUBJECT`] (record `text lang? method label?`, evidence: its page).
#[derive(Clone, Copy, Debug, Default)]
pub struct SummaryExtractive;

impl SummaryExtractive {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.extractive", 1);
}

impl Analyzer for SummaryExtractive {
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
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let card = card_of(ctx);
        let pages = ctx
            .document()
            .map(extractive::head_pages)
            .unwrap_or_default();
        out.check_deadline()?;
        let subject = extractive::subject(&pages);
        let text = extractive::summary(&card, subject.as_ref());
        let conf = match subject.as_ref().map(|s| s.method) {
            Some(extractive::SubjectMethod::Clause) => 0.9,
            Some(extractive::SubjectMethod::TextRank) => 0.6,
            None => 0.7,
        };
        out.push(keys::EXTRACTIVE, Value::text(text), conf, None);
        if let Some(s) = subject {
            let mut f = vec![("text", Value::text(s.text))];
            if let Some(l) = &card.lang {
                f.push(("lang", Value::text(l.as_str())));
            }
            f.push(("method", Value::text(s.method.as_str())));
            if let Some(l) = s.label {
                f.push(("label", Value::text(l)));
            }
            let conf = if s.method == extractive::SubjectMethod::Clause {
                0.85
            } else {
                0.5
            };
            out.push(
                keys::SUBJECT,
                Value::record(f),
                conf,
                Some(Evidence {
                    page: Some(s.page),
                    ..Evidence::default()
                }),
            );
        }
        Ok(())
    }
}

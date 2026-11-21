//! The analyzers: `summary.card@2`, `summary.extractive@2`, `summary.outline@1` and
//! `summary.digest@1`.

use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, Evidence, Profile, Result, Value};
use quail_text::NormalizedText;

use crate::card::{DEFAULT_MAX_TOKENS, FileCard};
use crate::digest::{self, DigestConfig};
use crate::extractive;
use crate::keys;
use crate::outline::{self, OutlineConfig, OutlineMethod};

/// Artifacts every fact-producing step of the workspace may provide. Declared as `after` so
/// the summary steps run once those steps are done (see the crate docs on ordering).
pub const AFTER: &[&str] = &["revised_document", "ocr_text", "raster", "normalized_text"];

/// `summary.card@2`: the file card from every fact produced before it.
///
/// Emits [`keys::CARD`] (record), [`keys::CARD_TEXT`] (text, at most `max_tokens` estimated
/// tokens) and [`keys::CARD_TOKENS`].
#[derive(Clone, Copy, Debug)]
pub struct SummaryCard {
    max_tokens: usize,
}

impl SummaryCard {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.card", 2);

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

/// `summary.extractive@2`: a 1–3 sentence English summary without a model.
///
/// Emits [`keys::EXTRACTIVE`] (text) and, when a subject sentence is found,
/// [`keys::SUBJECT`] (record `text lang? method label?`, evidence: its page).
#[derive(Clone, Copy, Debug, Default)]
pub struct SummaryExtractive;

impl SummaryExtractive {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.extractive", 2);
}

impl Analyzer for SummaryExtractive {
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
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let card = card_of(ctx);
        let shared = ctx.get::<NormalizedText>();
        let pages = ctx
            .document()
            .map(|d| extractive::head_pages_with(d, shared))
            .unwrap_or_default();
        out.check_deadline()?;
        let subject = extractive::subject(&pages);
        let text = extractive::summary(&card, subject.as_ref());
        let conf = match subject.as_ref().map(|s| s.method) {
            Some(extractive::SubjectMethod::Clause) => 0.9,
            Some(extractive::SubjectMethod::Recital) => 0.85,
            Some(extractive::SubjectMethod::Operative) => 0.8,
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
            let conf = match s.method {
                extractive::SubjectMethod::Clause => 0.85,
                extractive::SubjectMethod::Recital => 0.75,
                extractive::SubjectMethod::Operative => 0.7,
                extractive::SubjectMethod::TextRank => 0.5,
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

/// `summary.outline@1`: the document's headings with page ranges ([`crate::outline`]).
///
/// Emits [`keys::OUTLINE`] (record `method total pages sections [truncated]`, `sections` a
/// list of `title level page end_page source`) when headings are found, or fixed page
/// windows for a document of at least [`SummaryOutline::MIN_WINDOW_PAGES`] pages without them.
#[derive(Clone, Copy, Debug, Default)]
pub struct SummaryOutline {
    cfg: OutlineConfig,
}

impl SummaryOutline {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.outline", 1);
    /// Pages from which a document without headings gets page windows.
    pub const MIN_WINDOW_PAGES: u32 = 4;

    /// With the default [`OutlineConfig`].
    pub fn new() -> Self {
        Self::default()
    }

    /// With `cfg`.
    pub fn with_config(cfg: OutlineConfig) -> Self {
        Self { cfg }
    }
}

impl Analyzer for SummaryOutline {
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
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let o = outline::outline(doc, &self.cfg, out.deadline());
        if o.truncated {
            out.problem(format!(
                "{}: deadline reached, outline of the first pages only",
                Self::ID
            ));
        }
        let useful = match o.method {
            OutlineMethod::Headings => true,
            OutlineMethod::Windows => o.pages >= Self::MIN_WINDOW_PAGES,
        };
        if useful && !o.entries.is_empty() {
            let conf = if o.method == OutlineMethod::Headings {
                0.7
            } else {
                1.0
            };
            out.push(keys::OUTLINE, o.to_value(), conf, None);
        }
        Ok(())
    }
}

/// `summary.digest@1` (Standard): the bounded summary input for a document of any length
/// ([`crate::digest`]): card, outline, start and per-section excerpts copied verbatim.
///
/// Emits [`keys::DIGEST`] (text, at most [`DigestConfig::max_tokens`] estimated tokens) and
/// [`keys::DIGEST_TOKENS`], for documents of at least `min_doc_tokens` estimated tokens.
#[derive(Clone, Copy, Debug, Default)]
pub struct SummaryDigest {
    outline: OutlineConfig,
    digest: DigestConfig,
    min_doc_tokens: usize,
}

impl SummaryDigest {
    /// Id of this analyzer.
    pub const ID: AnalyzerId = AnalyzerId::new("summary.digest", 1);

    /// With the default configs, for every document with text.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `outline` and `digest` settings, for documents of at least `min_doc_tokens`
    /// estimated tokens (0: all; e.g. 2,000 leaves short documents to the card and the
    /// extractive summary and keeps records small).
    pub fn with_config(
        outline: OutlineConfig,
        digest: DigestConfig,
        min_doc_tokens: usize,
    ) -> Self {
        Self {
            outline,
            digest,
            min_doc_tokens,
        }
    }
}

impl Analyzer for SummaryDigest {
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
        // A few ms per file and up to ~8 KB of text per record: for services that feed a
        // model, not for every scan.
        Profile::Standard
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let bytes: usize = doc.pages.iter().map(|p| p.text.len()).sum();
        if bytes / 4 < self.min_doc_tokens {
            return Ok(());
        }
        let card = card_of(ctx);
        let card_text = card.render(self.digest.card_tokens).text;
        let o = outline::outline(doc, &self.outline, out.deadline());
        let d = digest::build(doc, &card_text, &o, &self.digest, out.deadline());
        if o.truncated || d.truncated {
            out.problem(format!(
                "{}: deadline reached, later sections have no excerpts",
                Self::ID
            ));
        }
        if d.body.split_whitespace().count() < 20 {
            return Ok(()); // no text to digest; the card says it all
        }
        out.fact(keys::DIGEST, Value::text(d.text));
        out.fact(
            keys::DIGEST_TOKENS,
            Value::Int(i64::try_from(d.tokens).unwrap_or(i64::MAX)),
        );
        Ok(())
    }
}

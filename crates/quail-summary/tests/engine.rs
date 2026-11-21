//! Ordering in a real engine: registered last, the summary steps see every fact, also when an
//! OCR-like step provides `revised_document` and the fact producers wait for it.

use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, FactKey, Format, Limits, Page,
    PageKind, Profile, RevisedDocument, SourceItem, TextSource, Value,
};
use quail_engine::EngineBuilder;
use quail_summary::{SummaryCard, SummaryExtractive, keys};

struct TextExtractor;

impl Extractor for TextExtractor {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("test.text", 1)
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::Text]
    }
    fn extract(&self, blob: &Blob, _: &Limits, _: &mut Emitter) -> quail_core::Result<Document> {
        Ok(Document {
            pages: vec![Page {
                text: String::from_utf8_lossy(blob.as_bytes()).into_owned(),
                text_source: TextSource::Native,
                ..Page::new(0, PageKind::Logical)
            }],
        })
    }
}

/// Emits one fact, after `revised_document` like the real text consumers.
struct Producer(&'static str, Value);

impl Analyzer for Producer {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new(self.0, 1)
    }
    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }
    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }
    fn run(&self, _: &Ctx<'_>, out: &mut Emitter) -> quail_core::Result<()> {
        out.fact(FactKey::new(self.0), self.1.clone());
        Ok(())
    }
}

/// Provides `revised_document`, registered after everything it unblocks.
struct Reviser;

impl Analyzer for Reviser {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("test.revise", 1)
    }
    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }
    fn provides(&self) -> &'static [&'static str] {
        &["revised_document"]
    }
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> quail_core::Result<()> {
        if let Some(d) = ctx.document() {
            out.provide(RevisedDocument(d.clone()));
        }
        Ok(())
    }
}

#[test]
fn summary_steps_registered_last_run_last() {
    let engine = EngineBuilder::new()
        .profile(Profile::Fast)
        .quiet_panics(true)
        .extractor(Box::new(TextExtractor))
        .analyzer(Box::new(Producer(
            "ref.instrument_kind",
            Value::text("invoice"),
        )))
        .analyzer(Box::new(Producer("text.lang", Value::text("en"))))
        .analyzer(Box::new(Reviser))
        .analyzer(Box::new(Producer("ref.ordinal", Value::Int(3))))
        .analyzer(Box::new(SummaryCard::new()))
        .analyzer(Box::new(SummaryExtractive))
        .build()
        .unwrap();
    let plan: Vec<AnalyzerId> = engine.plan().collect();
    let n = plan.len();
    assert_eq!(plan[n - 2], SummaryCard::ID, "{plan:?}");
    assert_eq!(plan[n - 1], SummaryExtractive::ID, "{plan:?}");

    let rec = engine.process(SourceItem {
        locator: "a.txt".into(),
        blob: Blob::new(b"Invoice for consulting services delivered in March.".to_vec()),
        modified: None,
    });
    let card = rec
        .facts_for(keys::CARD_TEXT.as_str())
        .next()
        .and_then(|f| f.value.as_text())
        .unwrap()
        .to_owned();
    assert!(card.starts_with("doc: invoice #3 lang=en"), "{card}");
    assert!(rec.facts_for(keys::EXTRACTIVE.as_str()).next().is_some());
}

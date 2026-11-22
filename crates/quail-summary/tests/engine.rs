//! Ordering in a real engine: registered last, the summary steps see every fact, also when an
//! OCR-like step provides `revised_document` and the fact producers wait for it.

use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, FactKey, Format, Limits, Page,
    PageKind, Profile, RevisedDocument, SourceItem, TextSource, Value,
};
use quail_engine::EngineBuilder;
use std::fmt::Write as _;

use quail_summary::{
    SummaryCard, SummaryDigest, SummaryExtractive, SummaryKeywords, SummaryOutline, SummaryTopics,
    keys,
};

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

#[test]
fn outline_and_digest_of_a_long_text() {
    let engine = EngineBuilder::new()
        .profile(Profile::Standard)
        .quiet_panics(true)
        .extractor(Box::new(TextExtractor))
        .analyzer(Box::new(Producer("text.lang", Value::text("en"))))
        .analyzer(Box::new(SummaryCard::new()))
        .analyzer(Box::new(SummaryOutline::new()))
        .analyzer(Box::new(SummaryDigest::new()))
        .build()
        .unwrap();
    let mut text = String::new();
    for c in 1..=30 {
        let _ = writeln!(text, "ARTICLE {c}\nTOPIC NUMBER {c}");
        for s in 0..40 {
            let _ = writeln!(
                text,
                "Clause {s} of article {c} makes the depot number {c} keep its ledger {s} \
                 current for the auditors every quarter."
            );
        }
    }
    let rec = engine.process(SourceItem {
        locator: "long.txt".into(),
        blob: Blob::new(text.into_bytes()),
        modified: None,
    });
    let method = rec
        .facts_for(keys::OUTLINE.as_str())
        .next()
        .and_then(|f| f.value.field("method").and_then(Value::as_text))
        .map(str::to_owned);
    assert_eq!(method.as_deref(), Some("headings"));
    let digest = rec
        .facts_for(keys::DIGEST.as_str())
        .next()
        .and_then(|f| f.value.as_text())
        .unwrap()
        .to_owned();
    assert!(digest.starts_with("FACTS:\n"), "{digest}");
    assert!(digest.contains("## ARTICLE 30 TOPIC NUMBER 30"), "{digest}");
    let tokens = rec
        .facts_for(keys::DIGEST_TOKENS.as_str())
        .next()
        .map(|f| f.value.clone());
    assert!(
        matches!(tokens, Some(Value::Int(n)) if n > 500 && n <= 2000),
        "{tokens:?}"
    );
}

#[test]
fn keywords_reach_the_card_and_topics_reuse_the_term_index() {
    let engine = EngineBuilder::new()
        .profile(Profile::Standard)
        .quiet_panics(true)
        .extractor(Box::new(TextExtractor))
        .analyzer(Box::new(Producer("text.lang", Value::text("en"))))
        // Registered before the step they read from: the plan still orders them after it.
        .analyzer(Box::new(SummaryCard::new()))
        .analyzer(Box::new(SummaryTopics::new()))
        .analyzer(Box::new(SummaryKeywords::new()))
        .build()
        .unwrap();
    let plan: Vec<AnalyzerId> = engine.plan().collect();
    let pos = |id: AnalyzerId| plan.iter().position(|&x| x == id).unwrap();
    assert!(pos(SummaryKeywords::ID) < pos(SummaryCard::ID), "{plan:?}");
    assert!(
        pos(SummaryKeywords::ID) < pos(SummaryTopics::ID),
        "{plan:?}"
    );

    let mut text = String::new();
    for (c, w) in ["glacier", "volcano", "orchard"].iter().enumerate() {
        let _ = writeln!(text, "CHAPTER {}\nAbout the {w}", c + 1);
        for s in 0..60 {
            let _ = writeln!(
                text,
                "The {w} survey team logs {w} samples near marker {s} before the {w} season ends."
            );
        }
    }
    let rec = engine.process(SourceItem {
        locator: "long.txt".into(),
        blob: Blob::new(text.into_bytes()),
        modified: None,
    });
    let key = keys::KEYWORDS;
    let kws = rec.facts_for(key.as_str()).next().unwrap();
    let Value::List(list) = &kws.value else {
        panic!("{kws:?}")
    };
    assert!(!list.is_empty() && list.len() <= 16);
    let card = rec
        .facts_for(keys::CARD_TEXT.as_str())
        .next()
        .and_then(|f| f.value.as_text())
        .unwrap()
        .to_owned();
    assert!(card.contains("\nkeywords: "), "{card}");
    // One logical page: no topic map (fewer than 20 pages).
    assert!(rec.facts_for(keys::TOPICS.as_str()).next().is_none());
}

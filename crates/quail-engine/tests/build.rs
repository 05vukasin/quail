//! `EngineBuilder::build` validation.
#![allow(missing_docs)]

mod common;

use common::*;
use quail_core::{AnalyzerId, Format, Profile};
use quail_engine::{BuildError, EngineBuilder};

fn err(b: EngineBuilder) -> BuildError {
    match b.build() {
        Ok(_) => panic!("expected a build error"),
        Err(e) => e,
    }
}

#[test]
fn unsatisfied_requirement() {
    let e = err(EngineBuilder::new().analyzer(Mock::new("mock.a").requires(&["ghost"]).boxed()));
    assert_eq!(
        e.to_string(),
        "analyzer mock.a@1 requires artifact `ghost`, which no active analyzer provides"
    );
    let e = err(EngineBuilder::new()
        .profile(Profile::Fast)
        .analyzer(Mock::new("mock.a").requires(&["shingles"]).boxed())
        .analyzer(
            Mock::new("mock.p")
                .provides(&["shingles"])
                .profile(Profile::Deep)
                .boxed(),
        ));
    assert!(
        e.to_string()
            .contains("(mock.p@1 provides it but is above profile Fast)"),
        "{e}"
    );
}

#[test]
fn cycles_are_rejected_with_the_path() {
    let e = err(EngineBuilder::new()
        .analyzer(Mock::new("mock.free").boxed())
        .analyzer(
            Mock::new("mock.a")
                .requires(&["b"])
                .provides(&["a"])
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.b")
                .requires(&["c"])
                .provides(&["b"])
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.c")
                .requires(&["a"])
                .provides(&["c"])
                .boxed(),
        ));
    let BuildError::Cycle(c) = &e else {
        panic!("{e}")
    };
    assert_eq!(c.len(), 4);
    assert_eq!(c.first(), c.last());
    assert_eq!(
        e.to_string(),
        "analyzer dependency cycle: mock.c@1 -> mock.b@1 -> mock.a@1 -> mock.c@1"
    );
}

#[test]
fn self_requiring_analyzer_is_a_cycle() {
    let e = err(EngineBuilder::new().analyzer(
        Mock::new("mock.self")
            .requires(&["x"])
            .provides(&["x"])
            .boxed(),
    ));
    assert_eq!(
        e,
        BuildError::Cycle(vec![
            AnalyzerId::new("mock.self", 1),
            AnalyzerId::new("mock.self", 1)
        ])
    );
}

#[test]
fn cycle_through_an_analyzer_provided_document() {
    // An analyzer that provides `document` becomes its provider (besides the extractor), so
    // requiring `document` orders after it, and a loop back is a cycle.
    let e = err(EngineBuilder::new()
        .analyzer(
            Mock::new("mock.ocr")
                .requires(&["layout"])
                .provides(&["document"])
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.layout")
                .requires(&["document"])
                .provides(&["layout"])
                .boxed(),
        ));
    assert!(matches!(e, BuildError::Cycle(ref c) if c.len() == 3), "{e}");
}

#[test]
fn duplicate_analyzer_name_whatever_the_version() {
    let e = err(EngineBuilder::new()
        .analyzer(Mock::new("mock.a").boxed())
        .analyzer(Mock::new("mock.a").version(2).boxed()));
    assert_eq!(
        e,
        BuildError::DuplicateAnalyzer(AnalyzerId::new("mock.a", 2))
    );
    assert_eq!(
        e.to_string(),
        "analyzer name `mock.a` is registered twice (mock.a@2)"
    );
}

#[test]
fn extractor_priority() {
    let a = || {
        Box::new(MockExtractor {
            name: "mock.a",
            formats: &[Format::Text, Format::Pdf],
            mode: ExtractMode::Pages(1),
        })
    };
    let b = || {
        Box::new(MockExtractor {
            name: "mock.b",
            formats: &[Format::Text],
            mode: ExtractMode::Pages(2),
        })
    };
    let e = err(EngineBuilder::new().extractor(a()).extractor(b()));
    assert!(matches!(
        e,
        BuildError::AmbiguousExtractor {
            format: Format::Text,
            ..
        }
    ));
    assert!(e.to_string().contains("mock.a@1 and mock.b@1"), "{e}");

    let engine = EngineBuilder::new()
        .extractor(a())
        .extractor_with_priority(b(), 10)
        .build()
        .unwrap();
    assert_eq!(engine.extractor_for(Format::Text).unwrap().name, "mock.b");
    assert_eq!(engine.extractor_for(Format::Pdf).unwrap().name, "mock.a");
    assert_eq!(engine.extractor_for(Format::Docx), None);
    assert_eq!(engine.process(item("x", b"text")).pages, Some(2));

    // A tie below the winner is fine.
    let c = Box::new(MockExtractor {
        name: "mock.c",
        formats: &[Format::Text],
        mode: ExtractMode::Pages(3),
    });
    let engine = EngineBuilder::new()
        .extractor(a())
        .extractor(c)
        .extractor_with_priority(b(), 1)
        .build()
        .unwrap();
    assert_eq!(engine.extractor_for(Format::Text).unwrap().name, "mock.b");
}

/// An artifact an extractor declares in `provides` satisfies analyzers that require it.
#[test]
fn extractor_provided_artifact_satisfies_requirements() {
    use quail_core::{
        Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, Format, Limits, Result,
    };
    struct WithRaster;
    impl Extractor for WithRaster {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("raster.extract", 1)
        }
        fn formats(&self) -> &'static [Format] {
            &[Format::Pdf]
        }
        fn provides(&self) -> &'static [&'static str] {
            &["raster"]
        }
        fn extract(&self, _: &Blob, _: &Limits, _: &mut Emitter) -> Result<Document> {
            Ok(Document::default())
        }
    }
    struct NeedsRaster;
    impl Analyzer for NeedsRaster {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("needs.raster", 1)
        }
        fn requires(&self) -> &'static [&'static str] {
            &["raster"]
        }
        fn run(&self, _: &Ctx<'_>, _: &mut Emitter) -> Result<()> {
            Ok(())
        }
    }
    let err = quail_engine::EngineBuilder::new()
        .analyzer(Box::new(NeedsRaster))
        .build()
        .err()
        .expect("no provider yet");
    assert!(
        matches!(err, quail_engine::BuildError::Unsatisfied { .. }),
        "{err:?}"
    );
    quail_engine::EngineBuilder::new()
        .extractor(Box::new(WithRaster))
        .analyzer(Box::new(NeedsRaster))
        .build()
        .expect("raster provided by the extractor");
}

/// `after` orders a consumer behind an active provider, and never blocks it when the provider is
/// absent; `Ctx::document` prefers the revised document.
#[test]
fn after_is_soft_ordering_and_ctx_prefers_revised_document() {
    use std::sync::{Arc, Mutex};

    use quail_core::{
        Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, Format, Limits, Page,
        PageKind, Result, RevisedDocument, SourceItem, TextSource,
    };

    struct Ext;
    impl Extractor for Ext {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("t.extract", 1)
        }
        fn formats(&self) -> &'static [Format] {
            &[Format::Text]
        }
        fn extract(&self, _: &Blob, _: &Limits, _: &mut Emitter) -> Result<Document> {
            Ok(Document {
                pages: vec![Page {
                    text: "native".into(),
                    text_source: TextSource::Native,
                    ..Page::new(0, PageKind::Logical)
                }],
            })
        }
    }
    struct Reviser;
    impl Analyzer for Reviser {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("t.revise", 1)
        }
        fn requires(&self) -> &'static [&'static str] {
            &["document"]
        }
        fn provides(&self) -> &'static [&'static str] {
            &["revised_document"]
        }
        fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
            let mut d = ctx.get::<Document>().cloned().unwrap_or_default();
            d.pages[0].text = "revised".into();
            out.provide(RevisedDocument(d));
            Ok(())
        }
    }
    struct Reader(Arc<Mutex<Vec<String>>>);
    impl Analyzer for Reader {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("t.read", 1)
        }
        fn requires(&self) -> &'static [&'static str] {
            &["document"]
        }
        fn after(&self) -> &'static [&'static str] {
            &["revised_document"]
        }
        fn run(&self, ctx: &Ctx<'_>, _: &mut Emitter) -> Result<()> {
            let t = ctx
                .document()
                .map(|d| d.pages[0].text.clone())
                .unwrap_or_default();
            self.0.lock().unwrap().push(t);
            Ok(())
        }
    }
    let item = || SourceItem {
        locator: "a.txt".into(),
        blob: Blob::from(b"plain text\n".to_vec()),
        modified: None,
    };
    // Reader registered first, but must run after the reviser.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let engine = quail_engine::EngineBuilder::new()
        .extractor(Box::new(Ext))
        .analyzer(Box::new(Reader(Arc::clone(&seen))))
        .analyzer(Box::new(Reviser))
        .build()
        .unwrap();
    engine.process(item());
    // Without a reviser the reader still runs and sees the original.
    let engine = quail_engine::EngineBuilder::new()
        .extractor(Box::new(Ext))
        .analyzer(Box::new(Reader(Arc::clone(&seen))))
        .build()
        .unwrap();
    engine.process(item());
    assert_eq!(*seen.lock().unwrap(), ["revised", "native"]);
}

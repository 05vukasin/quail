//! `Engine::process` with mock extractors and analyzers.
#![allow(missing_docs, clippy::manual_assert)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use quail_core::{Error, FactKey, Format, Limits, Profile, Status, Value};
use quail_engine::{EngineBuilder, SNIFF_ID, keys};

#[test]
fn emits_format_and_mime_and_runs_everything() {
    let engine = EngineBuilder::new()
        .extractor(MockExtractor::text(ExtractMode::Pages(3)))
        .analyzer(Mock::new("mock.a").requires(&["document"]).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hello"));
    // A warning does not make the record partial.
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(rec.warnings, ["mock.text@1: extractor note"]);
    assert_eq!(rec.format, Format::Text);
    assert_eq!(rec.pages, Some(3));
    assert_eq!(rec.size, 5);
    assert_eq!(keys::FORMAT.as_str(), "engine.format");
    assert_eq!(keys::MIME.as_str(), "engine.mime");
    let fmt: Vec<_> = rec.facts_for("engine.format").collect();
    assert_eq!(fmt.len(), 1);
    assert_eq!(fmt[0].value, Value::text("text"));
    assert_eq!(fmt[0].src, SNIFF_ID);
    assert_eq!(
        rec.facts_for("engine.mime").next().unwrap().value,
        Value::text("text/plain; charset=utf-8")
    );
    assert_eq!(rec.facts_for("mock.extractor").count(), 1);
    assert_eq!(ran(&rec), ["mock.a"]);
    let steps: Vec<_> = rec.steps.iter().map(ToString::to_string).collect();
    assert_eq!(steps, ["engine.sniff@1", "mock.text@1", "mock.a@1"]);
    assert_eq!(rec.timings.len(), 3);
}

#[test]
fn document_analyzer_on_formats_without_extractor_is_ok() {
    // Only a PDF extractor: text, PNG and unknown files never get a document, and that is
    // not a problem.
    let engine = EngineBuilder::new()
        .extractor(Box::new(MockExtractor {
            name: "mock.pdf",
            formats: &[Format::Pdf],
            mode: ExtractMode::Pages(1),
        }))
        .analyzer(Mock::new("mock.doc").requires(&["document"]).boxed())
        .analyzer(Mock::new("mock.generic").boxed())
        .build()
        .unwrap();
    for bytes in [
        &b"just text"[..],
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR",
        &[0, 1, 2, 3],
    ] {
        let rec = engine.process(item("x", bytes));
        assert_eq!(rec.status, Status::Ok, "{:?}", rec.format);
        assert_eq!(ran(&rec), ["mock.generic"]);
    }
    let rec = engine.process(item("a.pdf", b"%PDF-1.7\n1 0 obj\n"));
    assert_eq!(rec.format, Format::Pdf);
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(ran(&rec), ["mock.doc", "mock.generic"]);
}

#[test]
fn artifacts_flow_in_topological_order() {
    // Registered consumer-first; the plan must still run the provider first.
    let engine = EngineBuilder::new()
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(
            Mock::new("mock.consumer")
                .requires(&["shingles"])
                .run(|ctx, out| {
                    assert!(ctx.get::<Shingles>().is_some());
                    out.fact(FactKey::new("mock.ran"), Value::text("mock.consumer"));
                    Ok(())
                })
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.provider")
                .requires(&["document"])
                .provides(&["shingles"])
                .run(|_, out| {
                    out.provide(Shingles);
                    out.fact(FactKey::new("mock.ran"), Value::text("mock.provider"));
                    Ok(())
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.free").boxed())
        .build()
        .unwrap();
    let plan: Vec<_> = engine.plan().map(|a| a.name).collect();
    assert_eq!(plan, ["mock.provider", "mock.consumer", "mock.free"]);
    let rec = engine.process(item("x.txt", b"hi"));
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(ran(&rec), ["mock.provider", "mock.consumer", "mock.free"]);
}

#[test]
fn provider_not_applying_is_a_silent_skip() {
    let engine = EngineBuilder::new()
        .analyzer(
            Mock::new("mock.provider")
                .provides(&["shingles"])
                .applies(false)
                .boxed(),
        )
        .analyzer(Mock::new("mock.consumer").requires(&["shingles"]).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x", b"x"));
    assert_eq!(rec.status, Status::Ok);
    assert!(ran(&rec).is_empty());
}

#[test]
fn analyzer_panic_is_contained_and_others_run() {
    let engine = EngineBuilder::new()
        .quiet_panics(true)
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(Mock::new("mock.before").boxed())
        .analyzer(
            Mock::new("mock.bomb")
                .run(|_, out| {
                    out.fact(FactKey::new("mock.ran"), Value::text("mock.bomb"));
                    out.warn("about to fail");
                    panic!("hostile input {}", 42)
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.after").boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hi"));
    // The panicking analyzer's facts are dropped, its warnings kept.
    assert_eq!(ran(&rec), ["mock.before", "mock.after"]);
    assert_eq!(problems(&rec), ["mock.bomb@1: panic: hostile input 42"]);
    assert!(
        rec.warnings
            .iter()
            .any(|w| w == "mock.bomb@1: about to fail")
    );
    assert!(rec.steps.iter().any(|s| s.name == "mock.bomb"));
}

#[test]
fn failed_provider_makes_applicable_consumers_partial() {
    let engine = EngineBuilder::new()
        .analyzer(
            Mock::new("mock.provider")
                .provides(&["shingles"])
                .run(|_, out| {
                    out.provide(Shingles);
                    Err(Error::Other("nope".into()))
                })
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.consumer")
                .requires(&["shingles"])
                .provides(&["second"])
                .boxed(),
        )
        .analyzer(
            Mock::new("mock.quiet_consumer")
                .requires(&["shingles"])
                .applies(false)
                .boxed(),
        )
        // Transitively: its input was not produced because the consumer was skipped.
        .analyzer(Mock::new("mock.third").requires(&["second"]).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.bin", &[0, 1, 2]));
    assert!(ran(&rec).is_empty());
    let p = problems(&rec);
    assert_eq!(p.len(), 3, "{p:?}");
    assert_eq!(p[0], "mock.provider@1: nope");
    assert!(p[1].starts_with("mock.consumer@1: skipped, input `shingles` unavailable"));
    assert!(p[2].starts_with("mock.third@1: skipped, input `second` unavailable"));
}

#[test]
fn extractor_failure_and_panic_fail_the_file_but_keep_facts() {
    for mode in [ExtractMode::Fail, ExtractMode::Panic, ExtractMode::Timeout] {
        let engine = EngineBuilder::new()
            .quiet_panics(true)
            .limits(Limits {
                timeout: Duration::from_millis(20),
                ..Limits::default()
            })
            .extractor(MockExtractor::text(mode))
            .analyzer(Mock::new("mock.generic").boxed())
            .analyzer(Mock::new("mock.doc").requires(&["document"]).boxed())
            .build()
            .unwrap();
        let rec = engine.process(item("x.txt", b"hello"));
        assert_eq!(rec.pages, None);
        assert_eq!(rec.facts_for("mock.extractor").count(), 1);
        assert_eq!(rec.warnings, ["mock.text@1: extractor note"]);
        let error = failed(&rec);
        assert!(error.starts_with("mock.text@1: "), "{error}");
        if matches!(mode, ExtractMode::Timeout) {
            assert!(error.starts_with("mock.text@1: timeout"), "{error}");
        } else {
            // Generic analyzers still run, the document one is reported.
            assert_eq!(ran(&rec), ["mock.generic"]);
            assert!(
                error.contains("mock.doc@1: skipped, input `document`"),
                "{error}"
            );
        }
    }
}

#[test]
fn encrypted_or_limited_extraction_is_partial() {
    for (mode, msg) in [
        (ExtractMode::Encrypted, "mock.text@1: encrypted"),
        (
            ExtractMode::Limit,
            "mock.text@1: limit exceeded: max_decompressed",
        ),
    ] {
        let engine = EngineBuilder::new()
            .extractor(MockExtractor::text(mode))
            .analyzer(Mock::new("mock.generic").boxed())
            .build()
            .unwrap();
        let rec = engine.process(item("x.txt", b"hello"));
        assert_eq!(problems(&rec), [msg]);
        assert_eq!(rec.facts_for("mock.extractor").count(), 1);
        assert_eq!(ran(&rec), ["mock.generic"]);
    }
}

#[test]
fn too_big_skips_extraction_but_runs_generic_analyzers() {
    let limits = Limits {
        max_bytes: 4,
        ..Limits::default()
    };
    let engine = EngineBuilder::new()
        .limits(limits)
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(Mock::new("mock.generic").boxed())
        .analyzer(Mock::new("mock.doc").requires(&["document"]).boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hello world"));
    assert_eq!(ran(&rec), ["mock.generic"]);
    assert_eq!(rec.pages, None);
    assert_eq!(rec.facts_for("mock.extractor").count(), 0);
    let p = problems(&rec);
    assert!(p[0].contains("over max_bytes 4"), "{p:?}");
    assert!(p[1].contains("input `document` unavailable"), "{p:?}");
    assert_eq!(rec.format, Format::Text);
}

#[test]
fn deadline_exceeded_stops_the_plan() {
    let limits = Limits {
        timeout: Duration::from_millis(30),
        ..Limits::default()
    };
    let engine = EngineBuilder::new()
        .limits(limits)
        .analyzer(Mock::new("mock.first").boxed())
        .analyzer(
            Mock::new("mock.slow")
                .run(|_, out| {
                    loop {
                        out.check_deadline()?;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                })
                .boxed(),
        )
        .analyzer(Mock::new("mock.never").boxed())
        .analyzer(Mock::new("mock.never2").boxed())
        .build()
        .unwrap();
    let t = Instant::now();
    let rec = engine.process(item("x.txt", b"hi"));
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(ran(&rec), ["mock.first"]);
    let p = problems(&rec);
    assert_eq!(p[0], "mock.slow@1: timeout");
    assert_eq!(p[1], "engine: timeout; 2 analyzer(s) not run");
}

#[test]
fn deadline_expiring_during_extraction() {
    // The extractor never checks the deadline and overruns it: its document is kept, the
    // analyzers are not started.
    let engine = EngineBuilder::new()
        .limits(Limits {
            timeout: Duration::from_millis(10),
            ..Limits::default()
        })
        .extractor(MockExtractor::text(ExtractMode::Sleep(
            Duration::from_millis(30),
        )))
        .analyzer(Mock::new("mock.a").boxed())
        .build()
        .unwrap();
    let rec = engine.process(item("x.txt", b"hi"));
    assert_eq!(rec.pages, Some(1));
    assert!(ran(&rec).is_empty());
    assert_eq!(problems(&rec), ["engine: timeout; 1 analyzer(s) not run"]);
}

#[test]
fn profile_filters_analyzers() {
    let build = |p| {
        EngineBuilder::new()
            .profile(p)
            .analyzer(Mock::new("mock.fast").boxed())
            .analyzer(Mock::new("mock.std").profile(Profile::Standard).boxed())
            .analyzer(Mock::new("mock.deep").profile(Profile::Deep).boxed())
            .build()
            .unwrap()
    };
    let names = |p| -> Vec<&str> { build(p).plan().map(|a| a.name).collect() };
    assert_eq!(names(Profile::Fast), ["mock.fast"]);
    assert_eq!(names(Profile::Standard), ["mock.fast", "mock.std"]);
    assert_eq!(names(Profile::Deep), ["mock.fast", "mock.std", "mock.deep"]);
    let e = build(Profile::Standard);
    assert_eq!(e.inactive(), [quail_core::AnalyzerId::new("mock.deep", 1)]);
    let rec = e.process(item("x", b"x"));
    assert_eq!(ran(&rec), ["mock.fast", "mock.std"]);
}

#[test]
fn not_applicable_is_not_a_step() {
    let engine = EngineBuilder::new()
        .analyzer(Mock::new("mock.no").applies(false).boxed())
        .analyzer(
            Mock::new("mock.pdf_only")
                .applies_if(|ctx| ctx.format == Format::Pdf)
                .boxed(),
        )
        .build()
        .unwrap();
    let rec = engine.process(item("x", b"x"));
    assert_eq!(rec.status, Status::Ok);
    assert_eq!(rec.steps, [SNIFF_ID]);
}

/// An extractor that stops early reports a problem: the record is Partial and keeps its pages.
#[test]
fn extractor_problem_makes_record_partial_and_keeps_document() {
    use quail_core::{AnalyzerId, Blob, Document, Emitter, Extractor, Format, Limits, Result};
    struct Stops;
    impl Extractor for Stops {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("stops.extract", 1)
        }
        fn formats(&self) -> &'static [Format] {
            &[Format::Text]
        }
        fn extract(&self, _: &Blob, _: &Limits, out: &mut Emitter) -> Result<Document> {
            out.problem("stopped after page 1: deadline");
            Ok(Document {
                pages: vec![common::page("first page")],
            })
        }
    }
    let engine = quail_engine::EngineBuilder::new()
        .extractor(Box::new(Stops))
        .build()
        .unwrap();
    let rec = engine.process(quail_core::SourceItem {
        locator: "a.txt".into(),
        blob: Blob::from(b"plain text file\n".to_vec()),
        modified: None,
    });
    assert_eq!(rec.pages, Some(1));
    match rec.status {
        quail_core::Status::Partial { problems } => {
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains("stops.extract@1: stopped"))
            );
        }
        other => panic!("expected Partial, got {other:?}"),
    }
}

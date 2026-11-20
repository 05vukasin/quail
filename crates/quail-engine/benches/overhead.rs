//! Per-file engine overhead with no-op steps.
#![allow(missing_docs)]

use criterion::{Criterion, criterion_group, criterion_main};
use quail_core::{
    Analyzer, AnalyzerId, Blob, Ctx, Document, Emitter, Extractor, Format, Limits, Page, PageKind,
    Result, SourceItem, TextSource,
};
use quail_engine::EngineBuilder;

struct NoopExtractor;
impl Extractor for NoopExtractor {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("bench.extract", 1)
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::Text]
    }
    fn extract(&self, _: &Blob, _: &Limits, _: &mut Emitter) -> Result<Document> {
        Ok(Document {
            pages: vec![Page {
                index: 0,
                kind: PageKind::Logical,
                size: None,
                text: String::new(),
                blocks: Vec::new(),
                text_source: TextSource::Native,
                needs_ocr: false,
            }],
        })
    }
}

struct Noop(&'static str);
impl Analyzer for Noop {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new(self.0, 1)
    }
    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }
    fn run(&self, _: &Ctx<'_>, _: &mut Emitter) -> Result<()> {
        Ok(())
    }
}

const NAMES: [&str; 8] = [
    "bench.a", "bench.b", "bench.c", "bench.d", "bench.e", "bench.f", "bench.g", "bench.h",
];

fn overhead(c: &mut Criterion) {
    let blob = Blob::from(b"plain text file body, a few dozen bytes long\n".to_vec());
    let bare = EngineBuilder::new().build().unwrap();
    let mut b = EngineBuilder::new().extractor(Box::new(NoopExtractor));
    for n in NAMES {
        b = b.analyzer(Box::new(Noop(n)));
    }
    let full = b.build().unwrap();
    let item = || SourceItem {
        locator: "bench/file.txt".to_owned(),
        blob: blob.clone(),
        modified: None,
    };
    c.bench_function("process/sniff_only", |bch| {
        bch.iter(|| bare.process(item()));
    });
    c.bench_function("process/extractor+8_noop_analyzers", |bch| {
        bch.iter(|| full.process(item()));
    });
}

criterion_group!(benches, overhead);
criterion_main!(benches);

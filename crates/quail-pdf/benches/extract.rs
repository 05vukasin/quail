//! Extraction throughput on generated documents.
#![allow(missing_docs)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_core::{Blob, Emitter, Extractor, Limits};
use quail_pdf::PdfExtractor;

fn bench(c: &mut Criterion) {
    let extractor = PdfExtractor::new();
    let limits = Limits::default();
    let mut g = c.benchmark_group("pdf.extract");
    for (name, pages, lines) in [("1p_text", 1, 40), ("20p_text", 20, 40)] {
        let specs: Vec<common::PageSpec> =
            (1..=pages).map(|n| common::text_page(n, lines)).collect();
        let blob = Blob::from(common::pdf(
            &specs,
            &common::Extras {
                compress: true,
                ..common::Extras::default()
            },
        ));
        g.throughput(Throughput::Elements(pages as u64));
        g.bench_function(name, |b| {
            b.iter(|| {
                let mut out = Emitter::new(PdfExtractor::ID);
                extractor.extract(&blob, &limits, &mut out)
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

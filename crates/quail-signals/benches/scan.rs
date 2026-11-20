//! Throughput (MB/s) of `signal.scan@1` on generated text.
//!
//! `cargo bench -p quail-signals --bench scan`. Inputs are built in code: Serbian (Latin and
//! Cyrillic) and English contract/invoice-like text dense in identifiers, dates and amounts,
//! split into pages of ~4 KiB, about 512 KiB per input.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use quail_core::{
    Analyzer, Artifacts, Blob, Ctx, Document, Emitter, Format, Limits, Page, PageKind, Profile,
    TextSource,
};
use quail_signals::SignalScan;

const SR: &str = "Član 5. Zakupac je dužan da plaća zakupninu u iznosu od 1.200,50 RSD sa \
PDV-om do 15. septembra 2023. godine na tekući račun 160-0000000123456-78. PIB: 100000001, \
MB: 10000009. Tel: 011/123-4567, e-mail: kontakt@primer.rs. Ugovor stupa na snagu \
12.05.2023. i važi do 31.12.2025. UKUPNO ZA UPLATU: 12.345,00 RSD.\n";
const SR_CYR: &str = "Члан 5. Закупац је дужан да плаћа закупнину у износу од 1.200,50 дин. \
до 15. септембра 2023. године. ПИБ: 100000001, Матични број: 10000009. Укупно: 1.000,00 \
RSD.\n";
const EN: &str = "Section 5. The tenant shall pay USD 1,200.50 excl. VAT by September 15, \
2023 to IBAN RS35 2600 0560 1001 6113 79, SWIFT AAAARSBG. VAT No: GB 717 2961 78. Phone \
+44 20 7946 0958, (212) 555-1234. Effective Date: 05/06/2023. Total: $12,345.00. Visit \
www.example.com.\n";

fn doc(s: &str, bytes: usize) -> Document {
    let mut pages = Vec::new();
    let mut text = String::new();
    let mut total = 0;
    while total < bytes {
        text.push_str(s);
        total += s.len();
        if text.len() >= 4096 {
            pages.push(std::mem::take(&mut text));
        }
    }
    pages.push(text);
    Document {
        pages: pages
            .into_iter()
            .enumerate()
            .map(|(i, text)| Page {
                text,
                text_source: TextSource::Native,
                ..Page::new(u32::try_from(i).unwrap_or(u32::MAX), PageKind::Physical)
            })
            .collect(),
    }
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("signal.scan");
    g.measurement_time(Duration::from_secs(5));
    for (name, s) in [("sr", SR), ("sr_cyr", SR_CYR), ("en", EN)] {
        let d = doc(s, 512 << 10);
        let bytes: usize = d.pages.iter().map(|p| p.text.len()).sum();
        let mut artifacts = Artifacts::new();
        artifacts.insert(d);
        let blob = Blob::from(Vec::new());
        let limits = Limits::default();
        g.throughput(Throughput::Bytes(bytes as u64));
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                let ctx = Ctx {
                    blob: &blob,
                    format: Format::Text,
                    locator: "bench",
                    artifacts: &artifacts,
                    facts: &[],
                    limits: &limits,
                    profile: Profile::Fast,
                };
                let mut out = Emitter::new(SignalScan::ID);
                SignalScan.run(&ctx, &mut out).unwrap();
                black_box(out.facts().len())
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

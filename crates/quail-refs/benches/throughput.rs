//! Throughput of `ref.references` on generated documents (Serbian Latin, Serbian Cyrillic,
//! English), 32 pages of about 4 KiB each.
#![allow(missing_docs)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_core::{
    Analyzer, Artifacts, Blob, Ctx, Document, Emitter, Format, Limits, Page, PageKind, Profile,
};
use quail_refs::RefAnalyzer;

const SR: &str = "UGOVOR O PRUŽANJU USLUGA\nUgovor br. 114/23 od 12.03.2023. godine\nNa osnovu Okvirnog ugovora br. 55/2021 strane su se sporazumele o sledećem.\nČlan 1. Predmet ovog ugovora je pružanje usluga održavanja opreme prema specifikaciji iz Priloga 1.\nIsporučilac se obavezuje da usluge pruža stručno i u rokovima utvrđenim ovim ugovorom.\nStrana 1 od 32\n";
const CYR: &str = "УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА\nУговор бр. 1145/2023 од 12. марта 2023. године\nАнекс бр. 2 уз Уговор бр. 514/2022 мења члан 5.\nЗакупац се обавезује да закупнину плаћа месечно на текући рачун закуподавца.\nСтрана 1 од 32\n";
const EN: &str = "AMENDMENT NO. 2 TO THE MASTER SERVICES AGREEMENT DATED 1 MARCH 2021\nThis Amendment is issued pursuant to the Master Services Agreement No. KW-482917 and Statement of Work No. 3.\nInvoices shall reference Purchase Order 4500012345 and the Contract ID: 6619/AMD/QX.\nThe parties agree that all other terms of the Agreement remain in full force and effect.\nPage 1 of 32\n";

fn document(unit: &str) -> Document {
    let text = unit.repeat(4096 / unit.len() + 1);
    Document {
        pages: (0..32)
            .map(|i| Page {
                text: text.clone(),
                ..Page::new(i, PageKind::Physical)
            })
            .collect(),
    }
}

fn bench(c: &mut Criterion) {
    let a = RefAnalyzer::new();
    let mut g = c.benchmark_group("ref.references");
    for (name, unit) in [("sr-Latn", SR), ("sr-Cyrl", CYR), ("en", EN)] {
        let doc = document(unit);
        let bytes: usize = doc.pages.iter().map(|p| p.text.len()).sum();
        let mut artifacts = Artifacts::new();
        artifacts.insert(doc);
        let blob = Blob::new(Vec::<u8>::new());
        let limits = Limits::default();
        g.throughput(Throughput::Bytes(bytes as u64));
        g.bench_function(name, |b| {
            b.iter(|| {
                let ctx = Ctx {
                    blob: &blob,
                    format: Format::Pdf,
                    locator: "mem://bench",
                    artifacts: &artifacts,
                    facts: &[],
                    limits: &limits,
                    profile: Profile::Fast,
                };
                let mut out = Emitter::new(RefAnalyzer::ID);
                a.run(&ctx, &mut out).ok();
                out.into_parts().0.len()
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

//! Throughput of `party.candidates` on generated contract-like text.
//!
//! `cargo bench -p quail-parties --bench parties`

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_core::{
    Analyzer, Artifacts, Blob, Ctx, Document, Emitter, Format, Limits, Page, PageKind, Profile,
};
use quail_parties::PartyCandidates;
use quail_parties::scan::{PagePos, scan};
use quail_parties::words::PageWords;

const SR: &str = "Ugovor je zaključen između ALFATEK d.o.o. Beograd, sa sedištem u Beogradu, PIB 100200300, koga zastupa direktor (u daljem tekstu: Naručilac) i Betasoft DOO, Novi Sad (u daljem tekstu: Dobavljač). Dobavljač se obavezuje da Naručiocu pruži usluge održavanja softvera u skladu sa ponudom broj 12/2024 i opštim uslovima poslovanja. Cena usluga iznosi 1.200.000,00 dinara bez PDV-a i plaća se u roku od 15 dana od dana prijema fakture.\n";
const EN: &str = "This Master Services Agreement is entered into by and between Northwind Analytics Inc., a Delaware corporation (the \"Customer\"), and Bluefin Data Ltd (\"Supplier\"). The Supplier shall provide the Services described in each Statement of Work in accordance with the terms of this Agreement and the applicable Service Levels.\n";
const CYR: &str = "Уговор је закључен између ГАМА ПРОЈЕКТ д.о.о. Београд (у даљем тексту: Наручилац) и ДЕЛТА ИНЖЕЊЕРИНГ ДОО (у даљем тексту: Извршилац). Извршилац се обавезује да изведе радове у року.\n";

fn doc(paragraph: &str) -> Document {
    let page = paragraph.repeat(4_096 / paragraph.len() + 1);
    Document {
        pages: (0..64)
            .map(|i| Page {
                text: page.clone(),
                ..Page::new(i, PageKind::Physical)
            })
            .collect(),
    }
}

fn bench(c: &mut Criterion) {
    let limits = Limits::default();
    let blob = Blob::from(Vec::new());
    let mut g = c.benchmark_group("party");
    for (name, p) in [("sr-Latn", SR), ("en", EN), ("sr-Cyrl", CYR)] {
        let d = doc(p);
        let bytes: usize = d.pages.iter().map(|p| p.text.len()).sum();
        g.throughput(Throughput::Bytes(bytes as u64));
        let mut artifacts = Artifacts::new();
        artifacts.insert(d.clone());
        g.bench_function(format!("candidates/{name}"), |b| {
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
                let mut out = Emitter::new(PartyCandidates::ID);
                PartyCandidates.run(&ctx, &mut out).unwrap();
                black_box(out.facts().len())
            });
        });
        g.bench_function(format!("tokens/{name}"), |b| {
            b.iter(|| {
                d.pages
                    .iter()
                    .map(|p| PageWords::new(p, usize::MAX).len())
                    .sum::<usize>()
            });
        });
        let pos = PagePos {
            first: true,
            near_end: true,
            has_zones: false,
        };
        let words: Vec<PageWords> = d
            .pages
            .iter()
            .map(|p| PageWords::new(p, usize::MAX))
            .collect();
        g.bench_function(format!("scan/{name}"), |b| {
            b.iter(|| {
                words
                    .iter()
                    .map(|w| scan(w, pos, &[]).mentions.len())
                    .sum::<usize>()
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

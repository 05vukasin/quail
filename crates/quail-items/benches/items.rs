//! Throughput of `item.lines@2` on generated documents, and of CPV categorization.
//!
//! `cargo bench -p quail-items --bench items`. Inputs are built in code: a DOCX-style offer
//! (one table of 40 priced rows per page) and a PDF-style invoice page (header blocks on one
//! baseline, one block per column), 20 pages each; `cpv_top3` matches English and Serbian
//! descriptions against the embedded CPV 2008 index.

#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_core::{
    Analyzer, Artifacts, BBox, Blob, Block, BlockKind, Ctx, Document, Emitter, Format, Limits,
    Page, PageKind, Profile, Span, TextSource, Zone,
};
use quail_items::{CpvIndex, ItemLines};

const SERVICES: &[&str] = &[
    "Čišćenje poslovnog prostora",
    "Servis klima uređaja",
    "Software development services",
    "Project management",
    "Održavanje zelenih površina",
    "Network audit",
    "Translation services",
    "Security guard services",
];

fn push(text: &mut String, blocks: &mut Vec<Block>, t: &str, kind: BlockKind, bbox: Option<BBox>) {
    if !text.is_empty() {
        text.push('\n');
    }
    let start = u32::try_from(text.len()).unwrap_or(u32::MAX);
    text.push_str(t);
    let end = u32::try_from(text.len()).unwrap_or(u32::MAX);
    blocks.push(Block {
        span: Span { start, end },
        kind,
        zone: Zone::Unknown,
        bbox,
    });
}

fn docx_doc(pages: u32) -> Document {
    let mut out = Vec::new();
    for p in 0..pages {
        let (mut text, mut blocks) = (String::new(), Vec::new());
        let header = ["R.br.", "Naziv usluge", "JM", "Količina", "Cena", "Iznos"];
        for (c, h) in header.iter().enumerate() {
            let kind = BlockKind::TableCell {
                table: 0,
                row: 0,
                col: c as u16,
            };
            push(&mut text, &mut blocks, h, kind, None);
        }
        for r in 1..=40u16 {
            let s = SERVICES[usize::from(r) % SERVICES.len()];
            let q = u32::from(r) % 9 + 1;
            let cells = [
                format!("{r}."),
                s.to_owned(),
                "sat".to_owned(),
                q.to_string(),
                "1.250,00".to_owned(),
                format!("{},00", q * 1250),
            ];
            for (c, t) in cells.iter().enumerate() {
                let kind = BlockKind::TableCell {
                    table: 0,
                    row: r,
                    col: c as u16,
                };
                push(&mut text, &mut blocks, t, kind, None);
            }
        }
        out.push(Page {
            text,
            blocks,
            text_source: TextSource::Native,
            ..Page::new(p, PageKind::Logical)
        });
    }
    Document { pages: out }
}

fn pdf_doc(pages: u32) -> Document {
    let mut out = Vec::new();
    for p in 0..pages {
        let (mut text, mut blocks) = (String::new(), Vec::new());
        let bb = |x0: f32, x1: f32, y0: f32, n: usize| {
            Some(BBox {
                x0,
                y0,
                x1,
                y1: y0 + 12.0 * n as f32,
            })
        };
        for (t, x0, x1) in [
            ("Description", 50.0, 120.0),
            ("Qty", 300.0, 320.0),
            ("Unit price", 360.0, 420.0),
            ("Amount", 470.0, 520.0),
        ] {
            push(
                &mut text,
                &mut blocks,
                t,
                BlockKind::Paragraph,
                bb(x0, x1, 100.0, 1),
            );
        }
        let n = 40;
        let desc: Vec<&str> = (0..n).map(|i| SERVICES[i % SERVICES.len()]).collect();
        let qty: Vec<String> = (0..n).map(|i| (i % 9 + 1).to_string()).collect();
        let price: Vec<String> = (0..n).map(|_| "1,250.00".to_owned()).collect();
        let amount: Vec<String> = (0..n)
            .map(|i| format!("{}.00", (i % 9 + 1) * 1250))
            .collect();
        push(
            &mut text,
            &mut blocks,
            &desc.join("\n"),
            BlockKind::Paragraph,
            bb(50.0, 220.0, 120.0, n),
        );
        push(
            &mut text,
            &mut blocks,
            &qty.join("\n"),
            BlockKind::Paragraph,
            bb(308.0, 320.0, 120.0, n),
        );
        push(
            &mut text,
            &mut blocks,
            &price.join("\n"),
            BlockKind::Paragraph,
            bb(370.0, 420.0, 120.0, n),
        );
        push(
            &mut text,
            &mut blocks,
            &amount.join("\n"),
            BlockKind::Paragraph,
            bb(470.0, 520.0, 120.0, n),
        );
        out.push(Page {
            size: Some((595.0, 842.0)),
            text,
            blocks,
            text_source: TextSource::Native,
            ..Page::new(p, PageKind::Physical)
        });
    }
    Document { pages: out }
}

fn run(an: &ItemLines, doc: &Document, profile: Profile) -> usize {
    let blob = Blob::from(Vec::new());
    let mut arts = Artifacts::new();
    arts.insert(doc.clone());
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "bench",
        artifacts: &arts,
        facts: &[],
        limits: &limits,
        profile,
    };
    let mut out = Emitter::new(ItemLines::ID);
    let _ = an.run(&ctx, &mut out);
    out.facts().len()
}

fn bench(c: &mut Criterion) {
    let an = ItemLines::new();
    let mut g = c.benchmark_group("item_lines");
    g.measurement_time(Duration::from_secs(5));
    for (name, doc) in [("docx_20p", docx_doc(20)), ("pdf_20p", pdf_doc(20))] {
        g.throughput(Throughput::Elements(20 * 40));
        for profile in [Profile::Fast, Profile::Standard] {
            g.bench_function(format!("{name}_{profile:?}"), |b| {
                b.iter(|| run(&an, black_box(&doc), profile));
            });
        }
    }
    g.finish();
    let ix = CpvIndex::embedded();
    let mut g = c.benchmark_group("cpv");
    g.throughput(Throughput::Elements(SERVICES.len() as u64));
    g.bench_function("cpv_top3", |b| {
        b.iter(|| {
            SERVICES
                .iter()
                .map(|s| ix.top(black_box(s), 3).len())
                .sum::<usize>()
        });
    });
    g.bench_function("cpv_index_build", |b| b.iter(CpvIndex::embedded));
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

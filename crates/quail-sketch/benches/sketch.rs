//! Throughput of the text sketches on generated text (`cargo bench -p quail-sketch`).

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_sketch::{invoice, minhash, sada, simhash, template};
use quail_text::normalize::{NormalizeOptions, normalize};

fn text(words: usize) -> String {
    let syl = [
        "ka", "lo", "mi", "ne", "su", "ra", "to", "vi", "de", "pa", "zo", "li",
    ];
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut s = String::new();
    for i in 0..words {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let n = 1 + (x % 3) as usize;
        for k in 0..n {
            s.push_str(syl[((x >> (8 * k)) % 12) as usize]);
        }
        s.push_str(match i % 15 {
            7 => " 1.200,50 ",
            14 => ".\n",
            _ => " ",
        });
    }
    s
}

fn bench(c: &mut Criterion) {
    let raw = text(40_000);
    let norm = normalize(&raw, &NormalizeOptions::default()).into_owned();
    let mut g = c.benchmark_group("sketch");
    g.throughput(Throughput::Bytes(raw.len() as u64));
    g.sample_size(20);
    g.bench_function("minhash (normalize + shingle + 128 rows)", |b| {
        b.iter(|| minhash::text_minhash(black_box(&raw)));
    });
    g.bench_function("simhash_text", |b| {
        b.iter(|| simhash::text_simhash(black_box(&norm)))
    });
    g.bench_function("simhash_template", |b| {
        b.iter(|| template::template_simhash(black_box(&norm)));
    });
    g.bench_function("invoice_layout", |b| {
        b.iter(|| invoice::features(black_box(&raw)))
    });
    g.bench_function("sada raw_minhash", |b| {
        b.iter(|| sada::raw_minhash(black_box(&raw)))
    });
    g.bench_function("sada template_fp", |b| {
        b.iter(|| sada::template_fp(black_box(&raw)))
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

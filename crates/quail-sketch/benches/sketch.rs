//! Throughput of the text sketches on generated text (`cargo bench -p quail-sketch`).

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_sketch::{invoice, minhash, phash, sada, simhash, template};
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
        b.iter(|| simhash::text_simhash(black_box(&norm)));
    });
    g.bench_function("simhash_template", |b| {
        b.iter(|| template::template_simhash(black_box(&norm)));
    });
    g.bench_function("invoice_layout", |b| {
        b.iter(|| invoice::features(black_box(&raw)));
    });
    g.bench_function("sada raw_minhash", |b| {
        b.iter(|| sada::raw_minhash(black_box(&raw)));
    });
    g.bench_function("sada template_fp", |b| {
        b.iter(|| sada::template_fp(black_box(&raw)));
    });
    g.finish();
}

/// A gray (or RGB) scan-like page: noise plus dark text-like bars.
fn page(w: usize, h: usize, channels: usize) -> Vec<u8> {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut d = Vec::with_capacity(w * h * channels);
    for y in 0..h {
        for i in 0..w * channels {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let ink = (y / 40) % 2 == 0 && (i / 90) % 3 != 0;
            d.push(if ink { 20 } else { 230 } + (x % 16) as u8);
        }
    }
    d
}

fn bench_phash(c: &mut Criterion) {
    let mut g = c.benchmark_group("page_phash");
    g.sample_size(20);
    // A4 at 300 dpi (gray JBIG2/JPEG scan) and at 150 dpi in color.
    for (name, w, h, ch) in [
        ("gray A4 300dpi", 2480, 3508, 1u8),
        ("rgb A4 150dpi", 1240, 1754, 3),
    ] {
        let data = page(w, h, usize::from(ch));
        let (w32, h32) = (u32::try_from(w).unwrap(), u32::try_from(h).unwrap());
        g.throughput(Throughput::Elements((w * h) as u64));
        g.bench_function(name, |b| {
            b.iter(|| phash::phash(w32, h32, ch, black_box(&data)));
        });
    }
    g.finish();
}

criterion_group!(benches, bench, bench_phash);
criterion_main!(benches);

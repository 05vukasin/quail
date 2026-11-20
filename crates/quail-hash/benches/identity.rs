//! Throughput of the identity layer on 100 KB, 1 MB and 50 MB synthetic inputs.
//!
//! `fused` is what the analyzer runs (one pass, chunk by chunk); `separate` hashes the whole
//! buffer three times (BLAKE3, XXH3-128, then chunks). The single primitives are listed for
//! reference.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use quail_hash::{Chunk, ChunkParams, blake3, chunks, identity, xxh3_128};

/// Text-like bytes mixed with incompressible blocks, like a document with streams.
fn input(len: usize) -> Vec<u8> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let b = s.to_le_bytes();
        if (out.len() / 16_384) % 2 == 0 {
            out.extend(b.iter().map(|x| b'a' + x % 26));
        } else {
            out.extend(b);
        }
    }
    out.truncate(len);
    out
}

fn bench(c: &mut Criterion) {
    let p = ChunkParams::DOCUMENT;
    for (name, len) in [("100KB", 100_000), ("1MB", 1_000_000), ("50MB", 50_000_000)] {
        let data = input(len);
        let mut g = c.benchmark_group(format!("identity/{name}"));
        g.throughput(Throughput::Bytes(len as u64));
        if len > 10_000_000 {
            g.sample_size(10).measurement_time(Duration::from_secs(8));
        }
        g.bench_function(BenchmarkId::from_parameter("fused"), |b| {
            b.iter(|| identity(black_box(&data), p));
        });
        g.bench_function(BenchmarkId::from_parameter("separate"), |b| {
            b.iter(|| {
                let d = black_box(&data);
                let cs: Vec<Chunk> = chunks(d, p).collect();
                (blake3(d), xxh3_128(d), cs)
            });
        });
        g.bench_function(BenchmarkId::from_parameter("blake3"), |b| {
            b.iter(|| blake3(black_box(&data)));
        });
        g.bench_function(BenchmarkId::from_parameter("xxh3_128"), |b| {
            b.iter(|| xxh3_128(black_box(&data)));
        });
        g.bench_function(BenchmarkId::from_parameter("cdc"), |b| {
            b.iter(|| chunks(black_box(&data), p).count());
        });
        g.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);

//! Walk + read throughput over a generated tree of 10 000 small files.
//!
//! `cargo bench -p quail-io` (add `--features mmap` for the memory-mapping comparison).
//! The tree lives in a temp dir and is warm in the page cache after the first iteration, so
//! these numbers measure the walker and the read path, not the disk.

use std::fs;
use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_core::Source;
use quail_io::FsSource;
#[cfg(feature = "mmap")]
use quail_io::ReadOptions;

const DIRS: usize = 100;
const PER_DIR: usize = 100;
const FILE_BYTES: usize = 2048;

fn make_tree(root: &Path) {
    let body: Vec<u8> = (0..FILE_BYTES)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    for d in 0..DIRS {
        let dir = root.join(format!("d{d:03}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..PER_DIR {
            fs::write(dir.join(format!("f{f:03}.bin")), &body).unwrap();
        }
    }
}

fn drain(mut src: FsSource) -> (usize, usize) {
    let (mut n, mut bytes) = (0, 0);
    while let Some(item) = src.next_item() {
        let item = item.unwrap();
        n += 1;
        bytes += black_box(item.blob.as_bytes()).len();
    }
    (n, bytes)
}

fn bench_small(c: &mut Criterion) {
    let tmp = tempfile::tempdir().unwrap();
    make_tree(tmp.path());
    let root = tmp.path().to_path_buf();

    let mut g = c.benchmark_group("small_10k");
    g.throughput(Throughput::Elements((DIRS * PER_DIR) as u64));
    g.sample_size(20);

    g.bench_function("walk_only", |b| {
        b.iter(|| {
            let mut src = FsSource::builder().root(&root).build().unwrap();
            let mut n = 0;
            while let Some(p) = src.next_path() {
                black_box(p.unwrap());
                n += 1;
            }
            assert_eq!(n, DIRS * PER_DIR);
        });
    });
    g.bench_function("walk_read", |b| {
        b.iter(|| {
            let (n, _) = drain(FsSource::builder().root(&root).build().unwrap());
            assert_eq!(n, DIRS * PER_DIR);
        });
    });
    g.bench_function("walk_read_sorted", |b| {
        b.iter(|| {
            let (n, _) = drain(
                FsSource::builder()
                    .root(&root)
                    .sorted(true)
                    .build()
                    .unwrap(),
            );
            assert_eq!(n, DIRS * PER_DIR);
        });
    });
    g.bench_function("walk_read_filtered", |b| {
        b.iter(|| {
            let src = FsSource::builder()
                .root(&root)
                .extension("bin")
                .exclude("**/d00*")
                .build()
                .unwrap();
            let (n, _) = drain(src);
            assert_eq!(n, (DIRS - 10) * PER_DIR);
        });
    });
    #[cfg(feature = "mmap")]
    g.bench_function("walk_mmap_all", |b| {
        b.iter(|| {
            let src = FsSource::builder()
                .root(&root)
                .read_options(ReadOptions::new().mmap_min_bytes(Some(0)))
                .build()
                .unwrap();
            let (n, _) = drain(src);
            assert_eq!(n, DIRS * PER_DIR);
        });
    });
    g.finish();
}

/// Read vs map for larger files: 32 files of 4 MiB, every byte touched.
fn bench_large(c: &mut Criterion) {
    const N: usize = 32;
    const SIZE: usize = 4 << 20;
    let tmp = tempfile::tempdir().unwrap();
    let body: Vec<u8> = (0..SIZE).map(|i| u8::try_from(i % 253).unwrap()).collect();
    for i in 0..N {
        fs::write(tmp.path().join(format!("big{i:02}.bin")), &body).unwrap();
    }
    let root = tmp.path().to_path_buf();

    let touch = |src: FsSource| {
        let mut src = src;
        let mut sum = 0u64;
        while let Some(item) = src.next_item() {
            let item = item.unwrap();
            sum += item
                .blob
                .as_bytes()
                .iter()
                .step_by(64)
                .map(|&b| u64::from(b))
                .sum::<u64>();
        }
        black_box(sum)
    };

    let mut g = c.benchmark_group("large_32x4MiB");
    g.throughput(Throughput::Bytes((N * SIZE) as u64));
    g.sample_size(10);
    g.bench_function("read", |b| {
        b.iter(|| touch(FsSource::builder().root(&root).build().unwrap()));
    });
    #[cfg(feature = "mmap")]
    g.bench_function("mmap", |b| {
        b.iter(|| {
            touch(
                FsSource::builder()
                    .root(&root)
                    .read_options(ReadOptions::new().mmap_min_bytes(Some(1 << 20)))
                    .build()
                    .unwrap(),
            )
        });
    });
    g.finish();
}

criterion_group!(benches, bench_small, bench_large);
criterion_main!(benches);

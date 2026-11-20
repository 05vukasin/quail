//! Measures FastCDC parameter candidates on synthetic documents: how many chunks (and bytes)
//! a small edit changes, and how many chunk hashes a file costs.
//!
//! ```sh
//! cargo run --release -p quail-hash --example chunk_params [DIR]
//! ```
//!
//! With `DIR`, it also reports, per candidate, the share of bytes in chunks that occur in more
//! than one file of that directory tree (aggregate numbers only).
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use quail_hash::{Chunk, ChunkParams, chunks};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn text(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let r = rng.next();
        for i in 0..=((r % 9) as usize) {
            out.push(b'a' + ((r >> (8 + i * 5)) % 26) as u8);
        }
        out.push(if r.is_multiple_of(13) { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

fn random(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next() as u8).collect()
}

/// Text interleaved with incompressible blocks, like a PDF with streams or a DOCX with media.
fn mixed(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let n = 1000 + rng.below(30_000);
        if rng.next().is_multiple_of(2) {
            out.extend(text(rng, n));
        } else {
            out.extend(random(rng, n));
        }
    }
    out.truncate(len);
    out
}

fn edit(rng: &mut Rng, data: &[u8], kind: usize) -> Vec<u8> {
    let mut new = data.to_vec();
    let at = rng.below(data.len().max(1));
    match kind {
        0 => drop(new.splice(at..at, text(rng, 10))),
        1 => drop(new.splice(at..at, text(rng, 1000))),
        2 => drop(new.drain(at..(at + 100).min(new.len()))),
        _ => {
            for b in new.iter_mut().skip(at).take(10) {
                *b = b'x';
            }
        }
    }
    new
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

fn corpus(dir: &Path, candidates: &[(&str, Option<ChunkParams>)]) {
    let mut files = Vec::new();
    walk(dir, &mut files);
    let blobs: Vec<Vec<u8>> = files.iter().filter_map(|f| std::fs::read(f).ok()).collect();
    let total: u64 = blobs.iter().map(|b| b.len() as u64).sum();
    println!(
        "\ncorpus: {} files, {:.1} MB",
        blobs.len(),
        total as f64 / 1e6
    );
    println!(
        "{:<12} {:>12} {:>16}",
        "params", "chunks/file", "shared bytes %"
    );
    for (name, p) in candidates {
        let p = p.expect("valid params");
        let per_file: Vec<Vec<Chunk>> = blobs.iter().map(|b| chunks(b, p).collect()).collect();
        let mut files_with: HashMap<u64, usize> = HashMap::new();
        for cs in &per_file {
            let uniq: HashSet<u64> = cs.iter().map(|c| c.hash).collect();
            for h in uniq {
                *files_with.entry(h).or_default() += 1;
            }
        }
        let shared: u64 = per_file
            .iter()
            .flatten()
            .filter(|c| files_with[&c.hash] > 1)
            .map(|c| u64::from(c.len))
            .sum();
        let n: usize = per_file.iter().map(Vec::len).sum();
        println!(
            "{name:<12} {:>12.1} {:>16.2}",
            n as f64 / blobs.len().max(1) as f64,
            100.0 * shared as f64 / total.max(1) as f64
        );
    }
}

fn main() {
    let candidates = [
        ("1K/4K/32K", ChunkParams::new(1 << 10, 4 << 10, 32 << 10)),
        ("2K/8K/64K", ChunkParams::new(2 << 10, 8 << 10, 64 << 10)),
        (
            "4K/16K/128K",
            ChunkParams::new(4 << 10, 16 << 10, 128 << 10),
        ),
        (
            "8K/32K/256K",
            ChunkParams::new(8 << 10, 32 << 10, 256 << 10),
        ),
    ];
    let trials = 200;
    println!(
        "{:<12} {:>8} {:>7} {:>9} {:>12} {:>12}",
        "params", "size", "kind", "chunks", "chg chunks", "chg bytes %"
    );
    for (name, p) in candidates {
        let p = p.expect("valid params");
        for size in [10_000usize, 100_000, 1_000_000] {
            for (kind, generate) in [
                ("text", text as fn(&mut Rng, usize) -> Vec<u8>),
                ("random", random),
                ("mixed", mixed),
            ] {
                let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
                let (mut n_chunks, mut changed, mut changed_bytes) = (0usize, 0usize, 0u64);
                for t in 0..trials {
                    let old = generate(&mut rng, size);
                    let new = edit(&mut rng, &old, t % 4);
                    let a: Vec<Chunk> = chunks(&old, p).collect();
                    let b: Vec<Chunk> = chunks(&new, p).collect();
                    let known: HashSet<u64> = a.iter().map(|c| c.hash).collect();
                    for c in b.iter().filter(|c| !known.contains(&c.hash)) {
                        changed += 1;
                        changed_bytes += u64::from(c.len);
                    }
                    n_chunks += a.len();
                }
                let t = trials as f64;
                println!(
                    "{name:<12} {size:>8} {kind:>7} {:>9.1} {:>12.2} {:>12.2}",
                    n_chunks as f64 / t,
                    changed as f64 / t,
                    100.0 * changed_bytes as f64 / t / size as f64,
                );
            }
        }
    }
    if let Some(dir) = std::env::args().nth(1) {
        corpus(Path::new(&dir), &candidates);
    }
}

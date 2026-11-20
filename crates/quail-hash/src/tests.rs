#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]

use std::collections::HashSet;
use std::fmt::Write as _;

use quail_core::{Analyzer, Artifacts, Blob, Ctx, Emitter, Format, Limits, Profile, Value};

use super::*;

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Deterministic text-like bytes: words of lowercase letters separated by spaces/newlines.
fn text_like(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let r = next();
        let word = 1 + (r % 9) as usize;
        for i in 0..word {
            out.push(b'a' + ((r >> (8 + i * 5)) % 26) as u8);
        }
        out.push(if r.is_multiple_of(13) { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn new_hashes(old: &[Chunk], new: &[Chunk]) -> usize {
    let old: HashSet<u64> = old.iter().map(|c| c.hash).collect();
    new.iter().filter(|c| !old.contains(&c.hash)).count()
}

#[test]
fn blake3_known_answers() {
    assert_eq!(
        hex(&blake3(b"")),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    assert_eq!(
        hex(&blake3(&[0])),
        "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"
    );
    assert_eq!(
        hex(&blake3(b"abc")),
        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
    );
}

#[test]
fn xxh3_known_answers() {
    // Reference values from `xxhsum -H2`.
    assert_eq!(hex(&xxh3_128(b"")), "99aa06d3014798d86001c324468d497f");
    assert_eq!(hex(&xxh3_128(b"abc")), "06b05ab6733a618578af5f94892f3950");
    assert_eq!(
        hex(&xxh3_128(&pattern(100_000))),
        "54182c58bbb1337c42c23aeead96750d"
    );
}

#[test]
fn fused_pass_matches_one_shot_digests() {
    for len in [0, 1, 1023, 1024, 1025, 70_000, 1 << 20] {
        let data = text_like(len, 7);
        let id = identity(&data, ChunkParams::DOCUMENT);
        assert_eq!(id.digests.blake3, blake3(&data), "len {len}");
        assert_eq!(id.digests.xxh3_128, xxh3_128(&data), "len {len}");
        let streamed: Vec<Chunk> = chunks(&data, ChunkParams::DOCUMENT).collect();
        assert_eq!(id.chunks, streamed);
    }
}

#[test]
fn chunks_tile_the_input_within_bounds() {
    let p = ChunkParams::DOCUMENT;
    let data = text_like(3 << 20, 11);
    let cs: Vec<Chunk> = chunks(&data, p).collect();
    let mut at = 0;
    for (i, c) in cs.iter().enumerate() {
        assert_eq!(c.offset, at);
        assert!(c.len <= p.max());
        if i + 1 < cs.len() {
            assert!(c.len >= p.min());
        }
        at += u64::from(c.len);
    }
    assert_eq!(at, data.len() as u64);
}

#[test]
fn empty_input() {
    let id = identity(b"", ChunkParams::DOCUMENT);
    assert!(id.chunks.is_empty());
    assert_eq!(id.digests.blake3, blake3(b""));
}

#[test]
fn insertion_in_the_middle_changes_few_chunks() {
    for seed in 1..=8 {
        let old = text_like(1 << 20, seed);
        let mid = old.len() / 2;
        for insert in [1usize, 17, 100, 1000] {
            let mut new = old.clone();
            new.splice(mid..mid, text_like(insert, seed + 100));
            let a: Vec<Chunk> = chunks(&old, ChunkParams::DOCUMENT).collect();
            let b: Vec<Chunk> = chunks(&new, ChunkParams::DOCUMENT).collect();
            let changed = new_hashes(&a, &b);
            assert!(
                changed <= 3,
                "seed {seed}, +{insert} B: {changed} chunks changed"
            );
        }
    }
}

#[test]
fn deletion_in_the_middle_changes_few_chunks() {
    let old = text_like(1 << 20, 3);
    let mut new = old.clone();
    new.drain(400_000..400_050);
    let a: Vec<Chunk> = chunks(&old, ChunkParams::DOCUMENT).collect();
    let b: Vec<Chunk> = chunks(&new, ChunkParams::DOCUMENT).collect();
    assert!(new_hashes(&a, &b) <= 3);
}

#[test]
fn params_validation() {
    assert_eq!(
        ChunkParams::new(2048, 8192, 65536),
        Some(ChunkParams::DOCUMENT)
    );
    assert!(ChunkParams::new(8192, 2048, 65536).is_none());
    assert!(ChunkParams::new(2047, 8192, 65536).is_none());
    assert!(ChunkParams::new(32, 8192, 65536).is_none());
    assert!(ChunkParams::new(2048, 8192, 1 << 30).is_none());
}

#[test]
fn checked_stops_on_error() {
    let data = text_like(4 << 20, 5);
    let r = identity_checked(&data, ChunkParams::DOCUMENT, || Err("stop"));
    assert_eq!(r, Err("stop"));
    // Small inputs never reach a check.
    assert!(identity_checked(b"x", ChunkParams::DOCUMENT, || Err("stop")).is_ok());
}

fn run(bytes: Vec<u8>, limits: &Limits) -> Emitter {
    let blob = Blob::from(bytes);
    let artifacts = Artifacts::new();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Unknown,
        locator: "mem://test",
        artifacts: &artifacts,
        facts: &[],
        limits,
        profile: Profile::Fast,
    };
    let a = IdentityAnalyzer;
    assert!(a.applies(&ctx));
    let mut out = Emitter::new(a.id());
    a.run(&ctx, &mut out).unwrap();
    out
}

#[test]
fn analyzer_emits_all_keys() {
    let data = text_like(200_000, 9);
    let out = run(data.clone(), &Limits::default());
    let facts = out.facts();
    let get = |k: &quail_core::FactKey| {
        facts
            .iter()
            .find(|f| f.key == *k)
            .map(|f| f.value.clone())
            .unwrap()
    };
    assert_eq!(get(&keys::BLAKE3), Value::Bytes(blake3(&data).into()));
    assert_eq!(get(&keys::XXH3_128), Value::Bytes(xxh3_128(&data).into()));
    let Value::List(list) = get(&keys::CDC) else {
        panic!("cdc is not a list")
    };
    let expect: Vec<Value> = chunks(&data, ChunkParams::DOCUMENT)
        .map(|c| Value::U64(c.hash))
        .collect();
    assert_eq!(list, expect);
    assert_eq!(get(&keys::CDC_COUNT), Value::Int(expect.len() as i64));
    assert!(facts.iter().all(|f| f.key.namespace() == "hash"));
    assert_eq!(IdentityAnalyzer::ID.to_string(), "hash.identity@1");
}

#[test]
fn analyzer_on_empty_and_oversized_files() {
    let out = run(Vec::new(), &Limits::default());
    assert_eq!(out.facts().len(), 4);
    assert!(out.facts().iter().any(|f| f.value == Value::Int(0)));

    let limits = Limits {
        max_bytes: 10,
        ..Limits::default()
    };
    let out = run(vec![1; 1000], &limits);
    assert_eq!(out.facts().len(), 4);
}

#[test]
fn analyzer_honours_deadline() {
    let blob = Blob::from(text_like(8 << 20, 2));
    let artifacts = Artifacts::new();
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://big",
        artifacts: &artifacts,
        facts: &[],
        limits: &limits,
        profile: Profile::Fast,
    };
    let mut out = Emitter::with_deadline(IdentityAnalyzer::ID, std::time::Instant::now());
    assert!(matches!(
        IdentityAnalyzer.run(&ctx, &mut out),
        Err(quail_core::Error::Timeout)
    ));
    assert!(out.facts().is_empty());
}

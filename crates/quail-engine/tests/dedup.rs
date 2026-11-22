//! `RunOptions::dedup_by_content`: duplicates reuse the first record.
#![allow(missing_docs)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use common::*;
use quail_core::{FactKey, FileRecord, Result, Source, SourceItem, Value};
use quail_engine::{DEDUP_ID, Engine, EngineBuilder, RunOptions, keys};

/// An engine whose one analyzer counts its runs, sleeps `sleep` and emits the file's bytes.
fn counting_engine(runs: &Arc<AtomicU64>, sleep: Duration) -> Engine {
    let runs = Arc::clone(runs);
    EngineBuilder::new()
        .quiet_panics(true)
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(
            Mock::new("mock.count")
                .run(move |ctx, out| {
                    runs.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(sleep);
                    let text = String::from_utf8_lossy(ctx.blob.as_bytes()).into_owned();
                    out.fact(FactKey::new("mock.bytes"), Value::text(text));
                    Ok(())
                })
                .boxed(),
        )
        .build()
        .unwrap()
}

struct Items(std::vec::IntoIter<SourceItem>);

impl Items {
    fn new(items: &[(&str, &str)]) -> Self {
        let v: Vec<SourceItem> = items.iter().map(|(l, b)| item(l, b.as_bytes())).collect();
        Self(v.into_iter())
    }
}

impl Source for Items {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        self.0.next().map(Ok)
    }
}

fn dedup(threads: usize, ordered: bool) -> RunOptions {
    RunOptions {
        threads,
        max_in_flight: threads * 2,
        ordered,
        dedup_by_content: true,
        ..RunOptions::default()
    }
}

fn duplicate_of(r: &FileRecord) -> Option<String> {
    r.facts_for(keys::DUPLICATE_OF.as_str())
        .find_map(|f| match &f.value {
            Value::Text(t) => Some(t.to_string()),
            _ => None,
        })
}

fn by_locator<'a>(sink: &'a VecSink, locator: &str) -> &'a FileRecord {
    sink.records.iter().find(|r| r.locator == locator).unwrap()
}

#[test]
fn duplicates_reuse_the_first_record() {
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::ZERO);
    let mut src = Items::new(&[("a", "same"), ("b", "other"), ("c", "same"), ("d", "same")]);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &dedup(1, true));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(stats.files, 4);
    assert_eq!(stats.duplicates, 2);
    assert_eq!(sink.locators(), ["a", "b", "c", "d"]);

    let first = by_locator(&sink, "a");
    assert_eq!(duplicate_of(first), None);
    assert!(!first.steps.contains(&DEDUP_ID));
    for l in ["c", "d"] {
        let dup = by_locator(&sink, l);
        assert_eq!(duplicate_of(dup).as_deref(), Some("a"));
        // Same facts, then the marker; same steps, then the dedup step.
        assert_eq!(dup.facts[..dup.facts.len() - 1], first.facts[..]);
        assert_eq!(dup.facts.last().unwrap().src, DEDUP_ID);
        assert_eq!(dup.steps[..dup.steps.len() - 1], first.steps[..]);
        assert_eq!(*dup.steps.last().unwrap(), DEDUP_ID);
        assert_eq!(dup.timings.len(), 1);
        assert_eq!(dup.timings[0].src, DEDUP_ID);
        assert_eq!(
            (dup.size, dup.format, dup.pages),
            (first.size, first.format, first.pages)
        );
        assert_eq!(dup.status, first.status);
    }
    assert_eq!(duplicate_of(by_locator(&sink, "b")), None);
}

#[test]
fn off_by_default() {
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::ZERO);
    let mut src = Items::new(&[("a", "same"), ("b", "same")]);
    let mut sink = VecSink::default();
    let opts = RunOptions {
        threads: 1,
        ..RunOptions::default()
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(stats.duplicates, 0);
    assert!(sink.records.iter().all(|r| duplicate_of(r).is_none()));
}

#[test]
fn in_flight_duplicates_wait_for_the_first() {
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::from_millis(150));
    let mut src = Items::new(&[("a", "x"), ("b", "x"), ("c", "x"), ("d", "x")]);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &dedup(4, false));
    // All four are in flight at once; only one is analyzed.
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(stats.duplicates, 3);
    let originals: Vec<_> = sink
        .records
        .iter()
        .filter(|r| duplicate_of(r).is_none())
        .collect();
    assert_eq!(originals.len(), 1);
    let first = originals[0].locator.clone();
    assert!(
        sink.records
            .iter()
            .filter(|r| r.locator != first)
            .all(|r| duplicate_of(r).as_deref() == Some(first.as_str()))
    );
}

#[test]
fn ordered_mode_with_many_duplicates() {
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::from_millis(2));
    let names: Vec<String> = (0..60).map(|i| format!("f{i:02}")).collect();
    let contents: Vec<String> = (0..60).map(|i| format!("content {}", i % 7)).collect();
    let items: Vec<(&str, &str)> = names
        .iter()
        .zip(&contents)
        .map(|(n, c)| (n.as_str(), c.as_str()))
        .collect();
    let mut sink = VecSink::default();
    let stats = engine.run(&mut Items::new(&items), &mut sink, &dedup(4, true));
    assert_eq!(stats.files, 60);
    assert_eq!(
        sink.locators(),
        names.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(runs.load(Ordering::SeqCst), 7);
    assert_eq!(stats.duplicates, 53);
    // Every record carries its own content's facts.
    for (r, c) in sink.records.iter().zip(&contents) {
        let bytes = r.facts_for("mock.bytes").next().unwrap();
        assert_eq!(bytes.value, Value::text(c.as_str()));
    }
}

#[test]
fn evicted_records_are_processed_again() {
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::ZERO);
    let mut src = Items::new(&[("a", "one"), ("b", "two"), ("c", "one"), ("d", "two")]);
    let mut sink = VecSink::default();
    let opts = RunOptions {
        dedup_max_records: 1,
        ..dedup(1, true)
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    // Room for one record: "b" evicts "a", so "c" is processed again and evicts "b", and so
    // is "d".
    assert_eq!(runs.load(Ordering::SeqCst), 4);
    assert_eq!(stats.duplicates, 0);
    assert_eq!(stats.dedup_evicted, 3);

    // With room for two, the second pair is reused.
    let runs2 = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs2, Duration::ZERO);
    let mut src = Items::new(&[("a", "one"), ("b", "two"), ("c", "one"), ("d", "two")]);
    let mut sink = VecSink::default();
    let opts = RunOptions {
        dedup_max_records: 2,
        ..dedup(1, true)
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    assert_eq!(runs2.load(Ordering::SeqCst), 2);
    assert_eq!(stats.duplicates, 2);
    assert_eq!(stats.dedup_evicted, 0);
}

#[test]
fn byte_cap_keeps_nothing_too_big() {
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::ZERO);
    let mut src = Items::new(&[("a", "same"), ("b", "same")]);
    let mut sink = VecSink::default();
    let opts = RunOptions {
        dedup_max_bytes: 16,
        ..dedup(1, true)
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(stats.duplicates, 0);
}

#[test]
fn a_failed_first_copy_is_not_reused() {
    // The extractor panics on every file: `Failed` records, each copy processed.
    let engine = EngineBuilder::new()
        .quiet_panics(true)
        .extractor(MockExtractor::text(ExtractMode::Panic))
        .build()
        .unwrap();
    let mut src = Items::new(&[("a", "same"), ("b", "same"), ("c", "same")]);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &dedup(2, true));
    assert_eq!(stats.failed, 3);
    assert_eq!(stats.duplicates, 0);
    assert!(sink.records.iter().all(|r| duplicate_of(r).is_none()));
}

#[test]
fn the_first_copy_in_the_source_owns_its_content() {
    // Eight identical files of 4 MiB: hashing takes long enough that, if workers claimed the
    // content as they finish hashing, a later copy would often win. Ownership must follow
    // the source order at any thread count, in every run.
    let runs = Arc::new(AtomicU64::new(0));
    let engine = counting_engine(&runs, Duration::ZERO);
    let body = "invented identical content ".repeat(4 << 20 >> 5);
    for round in 0..20 {
        let names: Vec<String> = (0..8).map(|i| format!("c{i}")).collect();
        let items: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), body.as_str())).collect();
        let mut src = Items::new(&items);
        let mut sink = VecSink::default();
        let stats = engine.run(&mut src, &mut sink, &dedup(4, false));
        assert_eq!(stats.duplicates, 7, "round {round}");
        assert_eq!(duplicate_of(by_locator(&sink, "c0")), None, "round {round}");
        for n in &names[1..] {
            assert_eq!(
                duplicate_of(by_locator(&sink, n)).as_deref(),
                Some("c0"),
                "round {round}: {n}"
            );
        }
    }
}

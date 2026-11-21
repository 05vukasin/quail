//! `Engine::run`: backpressure, ordering, errors, cancellation.
#![allow(missing_docs, clippy::manual_assert)]

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use quail_core::{
    Artifacts, Error, FactKey, FileRecord, Limits, RecordSink, Result, Status, Value,
};
use quail_engine::{ArtifactsHook, Engine, EngineBuilder, RunOptions};

fn jittery_engine() -> Engine {
    // Variable work per file so completion order differs from source order.
    EngineBuilder::new()
        .quiet_panics(true)
        .extractor(MockExtractor::text(ExtractMode::Pages(1)))
        .analyzer(
            Mock::new("mock.jitter")
                .run(|ctx, out| {
                    let n = ctx.locator.bytes().map(u64::from).sum::<u64>();
                    std::thread::sleep(Duration::from_micros(n % 7 * 50));
                    out.fact(FactKey::new("mock.ran"), Value::Int(1));
                    Ok(())
                })
                .boxed(),
        )
        .build()
        .unwrap()
}

fn opts(threads: usize, max_in_flight: usize, ordered: bool) -> RunOptions {
    RunOptions {
        threads,
        max_in_flight,
        ordered,
        ..RunOptions::default()
    }
}

#[test]
fn run_bounds_in_flight_blobs() {
    let engine = jittery_engine();
    let mut src = MockSource::new(1000);
    let live = Arc::clone(&src.live);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &opts(4, 6, false));
    assert_eq!(stats.files, 1000);
    assert_eq!(stats.ok, 1000);
    assert_eq!(stats.partial + stats.failed, 0);
    assert_eq!(sink.records.len(), 1000);
    assert!(sink.finished);
    assert!(live.max() <= 6, "max live blobs {} > 6", live.max());
    assert!(live.max() >= 2, "no parallelism observed: {}", live.max());
    assert_eq!(live.now(), 0);
    assert!(stats.bytes > 0 && stats.files_per_sec() > 0.0);
    assert!(!stats.cancelled && !stats.stopped_on_sink_error && !stats.writer_panicked);
}

#[test]
fn fewer_slots_than_threads() {
    let engine = jittery_engine();
    let mut src = MockSource::new(200);
    let live = Arc::clone(&src.live);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &opts(8, 2, false));
    assert_eq!(stats.files, 200);
    assert!(live.max() <= 2, "{}", live.max());
}

#[test]
fn zero_threads_and_zero_slots_mean_one() {
    let engine = jittery_engine();
    let mut src = MockSource::new(50);
    let live = Arc::clone(&src.live);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &opts(0, 0, true));
    assert_eq!(stats.files, 50);
    assert_eq!(live.max(), 1);
    let expected: Vec<String> = (0..50).map(|i| format!("mem/{i}")).collect();
    assert_eq!(sink.locators(), expected);
}

#[test]
fn ordered_output_with_source_errors() {
    let engine = jittery_engine();
    let mut src = MockSource::new(300);
    src.fault = |i| match i % 10 {
        9 => Fault::Error,
        4 => Fault::LocatedError,
        _ => Fault::None,
    };
    let live = Arc::clone(&src.live);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &opts(4, 8, true));
    assert_eq!(stats.source_errors, 60);
    assert_eq!(stats.source_error_records, 30);
    assert_eq!(stats.files, 270);
    assert_eq!(stats.failed, 30);
    let expected: Vec<String> = (0..300)
        .filter(|i| i % 10 != 9)
        .map(|i| format!("mem/{i}"))
        .collect();
    assert_eq!(sink.locators(), expected);
    let located = &sink.records[4];
    assert_eq!(located.size, 0);
    assert!(
        matches!(&located.status, Status::Failed { error } if error.contains("permission denied")),
        "{:?}",
        located.status
    );
    assert!(live.max() <= 8);
}

#[test]
fn panicking_source_is_one_error_item() {
    let engine = jittery_engine();
    let mut src = MockSource::new(40);
    src.fault = |i| if i == 5 { Fault::Panic } else { Fault::None };
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &opts(2, 4, false));
    assert_eq!(stats.source_errors, 1);
    assert_eq!(stats.source_panics, 1);
    assert_eq!(stats.files, 39);
}

struct FailingSink {
    calls: Arc<Mutex<u64>>,
}

impl RecordSink for FailingSink {
    fn write(&mut self, r: &FileRecord) -> Result<()> {
        *self.calls.lock().unwrap() += 1;
        if r.locator.ends_with('3') {
            panic!("sink bug");
        }
        Err(Error::Other("broken pipe".into()))
    }
}

#[test]
fn sink_error_stops_an_endless_run() {
    let engine = jittery_engine();
    let mut src = MockSource::new(usize::MAX);
    let calls = Arc::new(Mutex::new(0));
    let mut sink = FailingSink {
        calls: Arc::clone(&calls),
    };
    let t = Instant::now();
    // Ordered, so the first write is `mem/0` (an error, not the panic of `mem/3`).
    let stats = engine.run(&mut src, &mut sink, &opts(4, 8, true));
    assert!(t.elapsed() < Duration::from_secs(5));
    assert!(stats.stopped_on_sink_error);
    assert_eq!(*calls.lock().unwrap(), 1);
    assert_eq!(stats.sink_errors, 1);
    assert_eq!(stats.first_sink_error.as_deref(), Some("broken pipe"));
    assert!(stats.files <= 10, "{}", stats.files);
    assert_eq!(stats.unwritten, stats.files - 1);
}

#[test]
fn sink_errors_and_panics_counted_when_not_stopping() {
    let engine = jittery_engine();
    let mut src = MockSource::new(20);
    let calls = Arc::new(Mutex::new(0));
    let mut sink = FailingSink {
        calls: Arc::clone(&calls),
    };
    let o = RunOptions {
        stop_on_sink_error: false,
        ..opts(2, 4, false)
    };
    let stats = engine.run(&mut src, &mut sink, &o);
    assert_eq!(*calls.lock().unwrap(), 20);
    assert_eq!(stats.sink_errors, 20);
    assert_eq!(stats.files, 20);
    assert!(!stats.stopped_on_sink_error);
}

struct CancellingSink {
    written: u64,
    cancel: Arc<AtomicBool>,
}

impl RecordSink for CancellingSink {
    fn write(&mut self, _: &FileRecord) -> Result<()> {
        self.written += 1;
        if self.written == 50 {
            self.cancel.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[test]
fn cancel_drains_in_flight_and_returns() {
    let engine = jittery_engine();
    let mut src = MockSource::new(usize::MAX);
    let live = Arc::clone(&src.live);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut sink = CancellingSink {
        written: 0,
        cancel: Arc::clone(&cancel),
    };
    let o = RunOptions {
        cancel: Some(cancel),
        ..opts(4, 8, true)
    };
    let stats = engine.run(&mut src, &mut sink, &o);
    assert!(stats.cancelled);
    assert!(
        stats.files >= 50 && stats.files <= 50 + 8,
        "{}",
        stats.files
    );
    // Everything pulled was written.
    assert_eq!(sink.written, stats.files);
    assert_eq!(live.now(), 0);
}

#[test]
fn byte_budget_limits_dispatch() {
    let engine = jittery_engine();
    let mut src = MockSource::new(200);
    src.size = 1000;
    let live = Arc::clone(&src.live);
    let mut sink = VecSink::default();
    let o = RunOptions {
        max_in_flight_bytes: 2500,
        ..opts(4, 100, false)
    };
    let stats = engine.run(&mut src, &mut sink, &o);
    assert_eq!(stats.files, 200);
    // Two dispatched (2000 <= 2500) plus the one the reader holds waiting for room.
    assert!(live.max() <= 3, "{}", live.max());

    // Every file is over the budget: each goes alone.
    let mut src = MockSource::new(50);
    src.size = 1000;
    let live = Arc::clone(&src.live);
    let o = RunOptions {
        max_in_flight_bytes: 10,
        ..opts(4, 100, false)
    };
    let stats = engine.run(&mut src, &mut VecSink::default(), &o);
    assert_eq!(stats.files, 50);
    assert!(live.max() <= 2, "{}", live.max());
}

#[test]
fn stalled_head_is_counted_in_ordered_mode() {
    let engine = EngineBuilder::new()
        .limits(Limits {
            timeout: Duration::from_millis(50),
            ..Limits::default()
        })
        .analyzer(
            Mock::new("mock.slow_head")
                .run(|ctx, _| {
                    if ctx.locator == "mem/0" {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    Ok(())
                })
                .boxed(),
        )
        .build()
        .unwrap();
    let mut src = MockSource::new(20);
    let mut sink = VecSink::default();
    let stats = engine.run(&mut src, &mut sink, &opts(2, 8, true));
    assert_eq!(stats.files, 20);
    // The slow head always stalls; on a loaded runner another file may miss the deadline too.
    assert!(stats.stalls >= 1, "{}", stats.stalls);
    assert_eq!(sink.locators()[0], "mem/0");
}

#[test]
fn default_options_are_sane() {
    let o = RunOptions::default();
    assert!(o.threads >= 1);
    assert!(o.max_in_flight >= o.threads);
    assert!(!o.ordered);
    assert!(o.stop_on_sink_error);
    assert_eq!(o.stack_size, 32 << 20);
    assert_eq!(o.max_in_flight_bytes, 1 << 30);
}

#[test]
fn on_artifacts_runs_per_file_and_its_panics_are_counted() {
    let engine = jittery_engine();
    let calls = Arc::new(AtomicU64::new(0));
    let docs = Arc::new(AtomicU64::new(0));
    let (c, d) = (Arc::clone(&calls), Arc::clone(&docs));
    let hook: ArtifactsHook = Arc::new(move |rec: &FileRecord, a: &Artifacts| {
        c.fetch_add(1, Ordering::Relaxed);
        if a.get::<quail_core::Document>().is_some() {
            d.fetch_add(1, Ordering::Relaxed);
        }
        if rec.locator.ends_with('7') {
            panic!("hook bug");
        }
    });
    let mut src = MockSource::new(100);
    let mut sink = VecSink::default();
    let o = RunOptions {
        on_artifacts: Some(hook),
        ..opts(4, 8, true)
    };
    let stats = engine.run(&mut src, &mut sink, &o);
    assert_eq!(calls.load(Ordering::Relaxed), 100);
    assert_eq!(docs.load(Ordering::Relaxed), 100);
    // mem/7, mem/17, ..., mem/97: ten panics, every record still written and ok.
    assert_eq!(stats.hook_panics, 10);
    assert!(
        stats
            .first_hook_panic
            .as_deref()
            .unwrap()
            .contains("hook bug")
    );
    assert_eq!((stats.files, stats.ok), (100, 100));
    assert_eq!(sink.records.len(), 100);
    assert!(stats.to_string().contains("10 artifact hook panics"));
    assert!(format!("{o:?}").contains("on_artifacts: Some"));
}

#[test]
fn on_artifacts_is_not_called_for_source_error_records() {
    let engine = jittery_engine();
    let calls = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&calls);
    let mut src = MockSource::new(10);
    src.fault = |i| {
        if i % 2 == 0 {
            Fault::LocatedError
        } else {
            Fault::None
        }
    };
    let mut sink = VecSink::default();
    let o = RunOptions {
        on_artifacts: Some(Arc::new(move |_: &FileRecord, _: &Artifacts| {
            c.fetch_add(1, Ordering::Relaxed);
        })),
        ..opts(2, 4, false)
    };
    let stats = engine.run(&mut src, &mut sink, &o);
    assert_eq!(sink.records.len(), 10);
    assert_eq!(stats.source_error_records, 5);
    assert_eq!(calls.load(Ordering::Relaxed), 5);
    assert_eq!(stats.hook_panics, 0);
}

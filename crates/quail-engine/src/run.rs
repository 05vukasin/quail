//! [`Engine::run`]: many files in parallel with bounded memory and a single writer.

use std::collections::BTreeMap;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use quail_core::{FileRecord, RecordSink, SCHEMA_VERSION, Source, SourceItem, Status};

use crate::engine::Engine;
use crate::panic::contain;

/// How [`Engine::run`] schedules work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOptions {
    /// Worker threads. `0` is treated as 1.
    pub threads: usize,
    /// Files pulled from the source but not yet written to the sink, at most. Bounds memory:
    /// at most this many blobs (and records waiting for the writer) exist at once. `0` is
    /// treated as 1.
    pub max_in_flight: usize,
    /// Write records in source order. Records that finish early wait in a reorder buffer,
    /// which `max_in_flight` bounds too.
    pub ordered: bool,
}

impl Default for RunOptions {
    /// `available_parallelism - 1` workers (at least 1), two files in flight per worker,
    /// unordered.
    fn default() -> Self {
        let threads = thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .max(1);
        Self {
            threads,
            max_in_flight: threads * 2,
            ordered: false,
        }
    }
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    /// Records written (or attempted) to the sink.
    pub files: u64,
    /// Sum of their sizes.
    pub bytes: u64,
    /// Records with [`Status::Ok`].
    pub ok: u64,
    /// Records with [`Status::Partial`].
    pub partial: u64,
    /// Records with [`Status::Failed`].
    pub failed: u64,
    /// Items the source could not produce (skipped, no record).
    pub source_errors: u64,
    /// Failed sink writes (including `finish`).
    pub sink_errors: u64,
    /// The first sink error, for the log.
    pub first_sink_error: Option<String>,
    /// Wall time of the run.
    pub elapsed: Duration,
}

impl RunStats {
    /// Files per second.
    pub fn files_per_sec(&self) -> f64 {
        per_sec(as_f64(self.files), self.elapsed)
    }

    /// Megabytes (10^6 bytes) per second.
    pub fn mb_per_sec(&self) -> f64 {
        per_sec(as_f64(self.bytes) / 1e6, self.elapsed)
    }
}

#[allow(clippy::cast_precision_loss)] // rates for display; 2^52 files is far away
fn as_f64(x: u64) -> f64 {
    x as f64
}

fn per_sec(x: f64, d: Duration) -> f64 {
    let s = d.as_secs_f64();
    if s > 0.0 { x / s } else { 0.0 }
}

impl fmt::Display for RunStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} files ({} ok, {} partial, {} failed), {:.1} MB in {:.2?}: {:.1} files/s, {:.1} MB/s",
            self.files,
            self.ok,
            self.partial,
            self.failed,
            as_f64(self.bytes) / 1e6,
            self.elapsed,
            self.files_per_sec(),
            self.mb_per_sec()
        )?;
        if self.source_errors > 0 || self.sink_errors > 0 {
            write!(
                f,
                "; {} source errors, {} sink errors",
                self.source_errors, self.sink_errors
            )?;
        }
        Ok(())
    }
}

impl Engine {
    /// Processes every item of `source` on `opts.threads` workers and writes each record to
    /// `sink` from a single writer thread, then calls [`RecordSink::finish`].
    ///
    /// Backpressure: the source is pulled only when one of `opts.max_in_flight` slots is
    /// free, and a slot is freed only when its record has been written. So at most
    /// `max_in_flight` blobs or pending records exist at any time, however fast the source.
    /// Source errors skip the item; sink errors are counted and the run continues.
    pub fn run(
        &self,
        source: &mut dyn Source,
        sink: &mut dyn RecordSink,
        opts: &RunOptions,
    ) -> RunStats {
        let started = Instant::now();
        let threads = opts.threads.max(1);
        let slots = opts.max_in_flight.max(1);
        let ordered = opts.ordered;

        let (slot_tx, slot_rx) = bounded::<()>(slots);
        for _ in 0..slots {
            let _ = slot_tx.send(());
        }
        let (work_tx, work_rx) = bounded::<(u64, SourceItem)>(slots);
        let (done_tx, done_rx) = bounded::<(u64, FileRecord)>(slots);

        let mut stats = thread::scope(|s| {
            for _ in 0..threads {
                let rx = work_rx.clone();
                let tx = done_tx.clone();
                s.spawn(move || {
                    for (seq, item) in rx {
                        if tx.send((seq, self.process_contained(item))).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(work_rx);
            drop(done_tx);
            let writer_slots = slot_tx.clone();
            let quiet = self.is_quiet();
            let writer = s.spawn(move || write_all(sink, &done_rx, &writer_slots, ordered, quiet));

            let mut seq = 0u64;
            let mut source_errors = 0u64;
            while slot_rx.recv().is_ok() {
                match source.next_item() {
                    None => break,
                    Some(Err(_)) => {
                        source_errors += 1;
                        let _ = slot_tx.send(());
                    }
                    Some(Ok(item)) => {
                        if work_tx.send((seq, item)).is_err() {
                            break;
                        }
                        seq += 1;
                    }
                }
            }
            drop(work_tx);
            let mut stats = writer.join().unwrap_or_default();
            stats.source_errors = source_errors;
            stats
        });
        stats.elapsed = started.elapsed();
        stats
    }

    /// `process`, plus a last-resort net for a bug in the engine itself: the file still
    /// gets a (failed) record and the run goes on.
    fn process_contained(&self, item: SourceItem) -> FileRecord {
        let locator = item.locator.clone();
        let size = item.blob.len() as u64;
        contain(self.is_quiet(), || self.process(item)).unwrap_or_else(|e| FileRecord {
            schema: SCHEMA_VERSION,
            locator,
            size,
            format: quail_core::Format::Unknown,
            pages: None,
            facts: Vec::new(),
            steps: Vec::new(),
            timings: Vec::new(),
            status: Status::Failed {
                error: format!("engine: {e}"),
            },
        })
    }
}

/// The single writer: reorders if asked, writes, frees a slot per record.
fn write_all(
    sink: &mut dyn RecordSink,
    done: &Receiver<(u64, FileRecord)>,
    slots: &Sender<()>,
    ordered: bool,
    quiet: bool,
) -> RunStats {
    let mut stats = RunStats::default();
    let mut pending: BTreeMap<u64, FileRecord> = BTreeMap::new();
    let mut next = 0u64;
    let mut write = |r: &FileRecord, stats: &mut RunStats| {
        stats.files += 1;
        stats.bytes += r.size;
        match r.status {
            Status::Ok => stats.ok += 1,
            Status::Partial { .. } => stats.partial += 1,
            Status::Failed { .. } => stats.failed += 1,
        }
        let res = contain(quiet, || sink.write(r)).and_then(|r| r);
        if let Err(e) = res {
            stats.sink_errors += 1;
            stats.first_sink_error.get_or_insert_with(|| e.to_string());
        }
        let _ = slots.send(());
    };
    for (seq, rec) in done {
        if !ordered {
            write(&rec, &mut stats);
            continue;
        }
        pending.insert(seq, rec);
        while let Some(rec) = pending.remove(&next) {
            write(&rec, &mut stats);
            next += 1;
        }
    }
    // Only reachable with gaps if a worker died; write what is left in order.
    for rec in pending.into_values() {
        write(&rec, &mut stats);
    }
    if let Err(e) = contain(quiet, || sink.finish()).and_then(|r| r) {
        stats.sink_errors += 1;
        stats.first_sink_error.get_or_insert_with(|| e.to_string());
    }
    stats
}

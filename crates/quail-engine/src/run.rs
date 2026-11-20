//! [`Engine::run`]: many files in parallel with bounded memory and a single writer.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use quail_core::{FileRecord, RecordSink, Source, SourceItem, Status};

use crate::engine::{Engine, failed_record};
use crate::panic::contain;

/// How often blocked loops wake up to look at cancellation, stop and liveness.
const TICK: Duration = Duration::from_millis(20);

/// Default worker stack: 32 MiB.
pub const DEFAULT_STACK_SIZE: usize = 32 << 20;

/// Default byte budget of files in flight: 1 GiB.
pub const DEFAULT_MAX_IN_FLIGHT_BYTES: u64 = 1 << 30;

/// How [`Engine::run`] schedules work.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Worker threads. `0` is treated as 1.
    pub threads: usize,
    /// Files pulled from the source but not yet written to the sink, at most. Bounds memory:
    /// at most this many blobs (and records waiting for the writer) exist at once. `0` is
    /// treated as 1.
    pub max_in_flight: usize,
    /// Bytes of files in flight, at most: no new file is dispatched while the files in
    /// flight plus it would exceed this, except that one file bigger than the budget may go
    /// alone. The source still reads one file ahead, so it should cap file size itself
    /// (`quail-io`'s `max_file_bytes`, set from [`Limits::max_bytes`](quail_core::Limits)).
    pub max_in_flight_bytes: u64,
    /// Write records in source order. Records that finish early wait in a reorder buffer,
    /// bounded by `max_in_flight`. One slow file holds back every record after it (and,
    /// once the buffer is full, the whole run) until it finishes; such waits longer than
    /// twice [`Limits::timeout`](quail_core::Limits) are counted in [`RunStats::stalls`].
    pub ordered: bool,
    /// Stop pulling new files after the first sink error (broken pipe, full disk). Records
    /// already in flight are dropped and counted in [`RunStats::unwritten`].
    pub stop_on_sink_error: bool,
    /// Set to `true` from anywhere to stop: no new file is pulled, files in flight are
    /// finished and written, and the run returns with [`RunStats::cancelled`].
    pub cancel: Option<Arc<AtomicBool>>,
    /// Stack size of each worker thread. Parsers of nested formats (PDF objects, XML) recurse,
    /// and a hostile file can nest deeply; a stack overflow aborts the whole process and
    /// cannot be caught like a panic, so workers get a large stack. It is virtual memory,
    /// committed only as it is used.
    pub stack_size: usize,
}

impl Default for RunOptions {
    /// `available_parallelism - 1` workers (at least 1), two files and at most
    /// [`DEFAULT_MAX_IN_FLIGHT_BYTES`] in flight, unordered, stop on sink error,
    /// [`DEFAULT_STACK_SIZE`] stacks.
    fn default() -> Self {
        let threads = thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .max(1);
        Self {
            threads,
            max_in_flight: threads * 2,
            max_in_flight_bytes: DEFAULT_MAX_IN_FLIGHT_BYTES,
            ordered: false,
            stop_on_sink_error: true,
            cancel: None,
            stack_size: DEFAULT_STACK_SIZE,
        }
    }
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    /// Records produced (written, failed to write, or unwritten after a stop).
    pub files: u64,
    /// Sum of their sizes.
    pub bytes: u64,
    /// Records with [`Status::Ok`].
    pub ok: u64,
    /// Records with [`Status::Partial`].
    pub partial: u64,
    /// Records with [`Status::Failed`] (including those for source errors).
    pub failed: u64,
    /// Items the source could not produce (errors and panics).
    pub source_errors: u64,
    /// Source errors that carried a locator and got a `Failed` record.
    pub source_error_records: u64,
    /// Source panics (also counted in `source_errors`).
    pub source_panics: u64,
    /// Failed sink writes (including `finish`).
    pub sink_errors: u64,
    /// The first sink error, for the log.
    pub first_sink_error: Option<String>,
    /// Records not written because the run stopped on a sink error.
    pub unwritten: u64,
    /// The run stopped early because of a sink error.
    pub stopped_on_sink_error: bool,
    /// The run stopped early because [`RunOptions::cancel`] was set.
    pub cancelled: bool,
    /// Ordered mode: times the head record held back finished ones for over twice the
    /// file timeout.
    pub stalls: u64,
    /// The writer thread died (a bug); counts above are those it reached.
    pub writer_panicked: bool,
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
        if self.cancelled {
            f.write_str("; cancelled")?;
        }
        if self.stopped_on_sink_error {
            f.write_str("; stopped on sink error")?;
        }
        Ok(())
    }
}

/// A finished file on its way to the writer.
struct Done {
    seq: u64,
    bytes: u64,
    record: FileRecord,
}

/// Sends a `Failed` record for its item if the worker unwinds before sending the real one,
/// so ordered mode never waits for a sequence number that will not come.
struct Guard<'a> {
    seq: u64,
    size: u64,
    locator: Option<String>,
    tx: &'a Sender<Done>,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Some(locator) = self.locator.take() {
            let _ = self.tx.send(Done {
                seq: self.seq,
                bytes: self.size,
                record: failed_record(locator, self.size, "engine: worker died".to_owned()),
            });
        }
    }
}

/// The reader's view of what is in flight, fed by the writer's "freed" messages.
struct Gate<'a> {
    freed: &'a Receiver<u64>,
    items: usize,
    bytes: u64,
}

enum Wait {
    Ready,
    Stopped,
    WriterGone,
}

impl Gate<'_> {
    fn free(&mut self, bytes: u64) {
        self.items = self.items.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    fn wait(&mut self, ready: impl Fn(usize, u64) -> bool, stop: &dyn Fn() -> bool) -> Wait {
        loop {
            while let Ok(b) = self.freed.try_recv() {
                self.free(b);
            }
            if ready(self.items, self.bytes) {
                return Wait::Ready;
            }
            if stop() {
                return Wait::Stopped;
            }
            match self.freed.recv_timeout(TICK) {
                Ok(b) => self.free(b),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Wait::WriterGone,
            }
        }
    }
}

#[derive(Clone, Copy)]
struct WriterCfg {
    ordered: bool,
    stop_on_sink_error: bool,
    quiet: bool,
    stall_after: Duration,
}

#[derive(Default)]
struct Shared {
    stats: Mutex<RunStats>,
    /// Set by the writer on a sink error when `stop_on_sink_error`.
    stop: AtomicBool,
}

impl Engine {
    /// Processes every item of `source` on `opts.threads` workers and writes each record to
    /// `sink` from a single writer thread, then calls [`RecordSink::finish`].
    ///
    /// Backpressure: the source is pulled only when fewer than `opts.max_in_flight` files
    /// (and fewer than `opts.max_in_flight_bytes` bytes) are between the source and the
    /// sink; a file stops counting once its record is written. So memory is bounded
    /// however fast the source is.
    ///
    /// Source errors skip the item; one that carries a locator ([`quail_core::Error::at`])
    /// still gets a `Failed` record. A panicking source counts as an error item. Sink errors
    /// are counted and, with `stop_on_sink_error`, end the run early. Nothing here blocks
    /// forever if a thread dies: the reader notices a dead writer, and a worker that dies
    /// still delivers a `Failed` record for its file.
    ///
    /// # Panics
    ///
    /// If the operating system cannot create the worker or writer threads.
    pub fn run(
        &self,
        source: &mut dyn Source,
        sink: &mut dyn RecordSink,
        opts: &RunOptions,
    ) -> RunStats {
        let started = Instant::now();
        let threads = opts.threads.max(1);
        let slots = opts.max_in_flight.max(1);
        let budget = opts.max_in_flight_bytes.max(1);
        let quiet = self.is_quiet();
        let cfg = WriterCfg {
            ordered: opts.ordered,
            stop_on_sink_error: opts.stop_on_sink_error,
            quiet,
            stall_after: self.limits().timeout.saturating_mul(2),
        };
        let shared = Shared::default();
        let cancel = opts.cancel.as_deref();
        let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));

        let (work_tx, work_rx) = bounded::<(u64, SourceItem)>(slots);
        let (done_tx, done_rx) = bounded::<Done>(slots);
        let (freed_tx, freed_rx) = unbounded::<u64>();

        let mut counts = SourceCounts::default();
        let mut was_cancelled = false;

        let writer_ok = thread::scope(|s| {
            for i in 0..threads {
                let rx = work_rx.clone();
                let tx = done_tx.clone();
                thread::Builder::new()
                    .name(format!("quail-worker-{i}"))
                    .stack_size(opts.stack_size)
                    .spawn_scoped(s, move || self.work(&rx, &tx))
                    .expect("spawn worker thread");
            }
            drop(work_rx);
            let shared = &shared;
            // The writer owns its channel ends, so if it dies the reader sees `freed`
            // disconnect and the workers see `done` disconnect.
            let writer = thread::Builder::new()
                .name("quail-writer".to_owned())
                .spawn_scoped(s, move || write_all(sink, &done_rx, &freed_tx, shared, cfg))
                .expect("spawn writer thread");

            let stop_now = || cancelled() || shared.stop.load(Ordering::Relaxed);
            let mut gate = Gate {
                freed: &freed_rx,
                items: 0,
                bytes: 0,
            };
            let reader = Reader {
                work: &work_tx,
                done: &done_tx,
                slots,
                budget,
                quiet,
            };
            counts = reader.read_all(source, &mut gate, &stop_now);
            was_cancelled = cancelled();
            drop(work_tx);
            drop(done_tx);
            writer.join().is_ok()
        });

        let mut stats = shared
            .stats
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        stats.writer_panicked = !writer_ok;
        stats.source_errors = counts.errors;
        stats.source_error_records = counts.error_records;
        stats.source_panics = counts.panics;
        stats.cancelled = was_cancelled;
        stats.elapsed = started.elapsed();
        stats
    }

    fn work(&self, rx: &Receiver<(u64, SourceItem)>, tx: &Sender<Done>) {
        for (seq, item) in rx {
            let size = item.blob.len() as u64;
            let mut guard = Guard {
                seq,
                size,
                locator: Some(item.locator.clone()),
                tx,
            };
            // Last-resort net for a bug in the engine itself: the file still gets a record.
            let record = match contain(self.is_quiet(), || self.process(item)) {
                Ok(r) => r,
                Err(e) => failed_record(
                    guard.locator.take().unwrap_or_default(),
                    size,
                    format!("engine: {e}"),
                ),
            };
            guard.locator = None;
            if tx
                .send(Done {
                    seq,
                    bytes: size,
                    record,
                })
                .is_err()
            {
                break;
            }
        }
    }
}

#[derive(Default)]
struct SourceCounts {
    errors: u64,
    error_records: u64,
    panics: u64,
}

/// The pulling side of a run (on the caller's thread, since the source is `&mut`).
struct Reader<'a> {
    work: &'a Sender<(u64, SourceItem)>,
    done: &'a Sender<Done>,
    slots: usize,
    budget: u64,
    quiet: bool,
}

impl Reader<'_> {
    fn read_all(
        &self,
        source: &mut dyn Source,
        gate: &mut Gate<'_>,
        stop_now: &dyn Fn() -> bool,
    ) -> SourceCounts {
        let mut counts = SourceCounts::default();
        let mut seq = 0u64;
        loop {
            if stop_now() {
                break;
            }
            match gate.wait(|items, _| items < self.slots, stop_now) {
                Wait::Ready => {}
                Wait::Stopped | Wait::WriterGone => break,
            }
            let item = match contain(self.quiet, || source.next_item()) {
                Ok(None) => break,
                Ok(Some(Ok(item))) => item,
                Ok(Some(Err(e))) => {
                    counts.errors += 1;
                    if let Some(locator) = e.locator() {
                        let record = failed_record(locator.to_owned(), 0, format!("source: {e}"));
                        gate.items += 1;
                        let done = Done {
                            seq,
                            bytes: 0,
                            record,
                        };
                        if self.done.send(done).is_err() {
                            break;
                        }
                        seq += 1;
                        counts.error_records += 1;
                    }
                    continue;
                }
                Err(_) => {
                    counts.errors += 1;
                    counts.panics += 1;
                    continue;
                }
            };
            let size = item.blob.len() as u64;
            // Room for its bytes; a file bigger than the budget goes alone. Cancellation does
            // not interrupt this wait: the file is already read, so it is processed.
            let budget = self.budget;
            let fits =
                |items: usize, bytes: u64| items == 0 || bytes.saturating_add(size) <= budget;
            if let Wait::WriterGone = gate.wait(fits, &|| false) {
                break;
            }
            gate.items += 1;
            gate.bytes += size;
            if self.work.send((seq, item)).is_err() {
                break;
            }
            seq += 1;
        }
        counts
    }
}

/// The single writer: reorders if asked, writes, reports each freed slot to the reader.
fn write_all(
    sink: &mut dyn RecordSink,
    done: &Receiver<Done>,
    freed: &Sender<u64>,
    shared: &Shared,
    cfg: WriterCfg,
) {
    let lock = || shared.stats.lock().unwrap_or_else(PoisonError::into_inner);
    let mut emit = |d: Done| {
        {
            let mut st = lock();
            let r = &d.record;
            st.files += 1;
            st.bytes += r.size;
            match r.status {
                Status::Ok => st.ok += 1,
                Status::Partial { .. } => st.partial += 1,
                Status::Failed { .. } => st.failed += 1,
            }
            if st.stopped_on_sink_error {
                st.unwritten += 1;
            } else if let Err(e) = contain(cfg.quiet, || sink.write(r)).and_then(|x| x) {
                st.sink_errors += 1;
                st.first_sink_error.get_or_insert_with(|| e.to_string());
                if cfg.stop_on_sink_error {
                    st.stopped_on_sink_error = true;
                    shared.stop.store(true, Ordering::Relaxed);
                }
            }
        }
        let _ = freed.send(d.bytes);
    };

    let mut pending: BTreeMap<u64, Done> = BTreeMap::new();
    let mut next = 0u64;
    let mut head_since = Instant::now();
    let mut head_flagged = false;
    loop {
        let msg = if cfg.ordered {
            match done.recv_timeout(TICK) {
                Ok(d) => Some(d),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match done.recv() {
                Ok(d) => Some(d),
                Err(_) => break,
            }
        };
        let Some(d) = msg else {
            if !head_flagged && !pending.is_empty() && head_since.elapsed() > cfg.stall_after {
                lock().stalls += 1;
                head_flagged = true;
            }
            continue;
        };
        if !cfg.ordered {
            emit(d);
            continue;
        }
        pending.insert(d.seq, d);
        while let Some(d) = pending.remove(&next) {
            emit(d);
            next += 1;
            head_since = Instant::now();
            head_flagged = false;
        }
        if !head_flagged && !pending.is_empty() && head_since.elapsed() > cfg.stall_after {
            lock().stalls += 1;
            head_flagged = true;
        }
    }
    // Gaps are impossible (every dispatched item yields a record), but never lose records.
    for d in pending.into_values() {
        emit(d);
    }
    if let Err(e) = contain(cfg.quiet, || sink.finish()).and_then(|r| r) {
        let mut st = lock();
        st.sink_errors += 1;
        st.first_sink_error.get_or_insert_with(|| e.to_string());
    }
}

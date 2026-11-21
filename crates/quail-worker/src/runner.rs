//! [`IsolatedRunner`]: [`Engine::run`](quail_engine::Engine::run) with every file processed
//! in a worker child process.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use quail_core::{
    Artifacts, Document, FileRecord, Format, RecordSink, SCHEMA_VERSION, Source, Status,
};
use quail_engine::{RunOptions, RunStats};

use crate::job::{FromSource, Job, JobInput, JobSource};
use crate::worker::{Crash, CrashKind, WorkerConfig, WorkerError, WorkerProcess};

/// How often blocked loops wake up to look at cancellation, stop and liveness.
const TICK: Duration = Duration::from_millis(20);
/// Files up to this size are read whole to sniff the format of a crashed path job; bigger
/// ones only by their first [`SNIFF_PREFIX`] bytes.
const SNIFF_WHOLE: u64 = 16 << 20;
/// See [`SNIFF_WHOLE`].
const SNIFF_PREFIX: u64 = 64 << 10;

/// How the supervisor treats its workers.
#[derive(Clone, Debug)]
pub struct IsolationOptions {
    /// How to start a worker and its limits.
    pub worker: WorkerConfig,
    /// A file whose worker crashed is tried once more in a fresh worker; if that crashes
    /// too the file is recorded as `Failed` and skipped (a poison file). With `false` the
    /// first crash is final.
    pub retry_crashed: bool,
    /// Also retry a file whose worker was killed by the watchdog. Off by default: a hang
    /// usually repeats, and a retry costs the whole wall-clock budget again.
    pub retry_timeouts: bool,
    /// Wall-clock budget of one file from the moment it is sent to the worker; past it the
    /// worker is killed. `None`: twice the engine's `Limits::timeout` plus [`grace`]
    /// (the engine's own cooperative deadline gets the first chance).
    ///
    /// [`grace`]: Self::grace
    pub kill_after: Option<Duration>,
    /// Added to twice the engine timeout when [`kill_after`](Self::kill_after) is `None`.
    pub grace: Duration,
    /// Attempts to (re)start a worker before the file at hand is recorded as `Failed`.
    pub spawn_attempts: u32,
}

impl Default for IsolationOptions {
    /// Default [`WorkerConfig`], retry crashes once, do not retry timeouts, kill after
    /// `2 × timeout + 5 s`, three spawn attempts.
    fn default() -> Self {
        Self {
            worker: WorkerConfig::default(),
            retry_crashed: true,
            retry_timeouts: false,
            kill_after: None,
            grace: Duration::from_secs(5),
            spawn_attempts: 3,
        }
    }
}

/// What an isolated run did: everything [`RunStats`] counts, plus worker crashes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IsolatedStats {
    /// The same counts as an in-process run.
    pub run: RunStats,
    /// Worker deaths while holding a file (every attempt counts).
    pub crashes: u64,
    /// Crashes by [`CrashKind::label`].
    pub crash_kinds: BTreeMap<&'static str, u64>,
    /// The first crash, `locator: message`, for the log.
    pub first_crash: Option<String>,
    /// Files tried again in a fresh worker after a crash.
    pub retries: u64,
    /// Retried files that then succeeded.
    pub retried_ok: u64,
    /// Files that crashed on the retry too, recorded as `Failed` and skipped.
    pub poisoned: u64,
    /// Workers started to replace a dead one.
    pub respawns: u64,
    /// Failed attempts to start a worker.
    pub spawn_failures: u64,
}

impl IsolatedStats {
    fn crash(&mut self, crash: &Crash, locator: &str) {
        self.crashes += 1;
        *self.crash_kinds.entry(crash.kind.label()).or_default() += 1;
        self.first_crash
            .get_or_insert_with(|| format!("{locator}: {crash}"));
    }
}

impl fmt::Display for IsolatedStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.run, f)?;
        if self.crashes > 0 {
            write!(f, "; {} worker crashes (", self.crashes)?;
            for (i, (k, n)) in self.crash_kinds.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{n} {k}")?;
            }
            write!(
                f,
                "), {} retried ({} ok), {} poisoned, {} respawns",
                self.retries, self.retried_ok, self.poisoned, self.respawns
            )?;
        }
        if self.spawn_failures > 0 {
            write!(f, "; {} worker spawn failures", self.spawn_failures)?;
        }
        Ok(())
    }
}

/// The function behind [`RunOptions::on_artifacts`].
type HookFn = dyn Fn(&FileRecord, &Artifacts) + Send + Sync;

/// One worker seat of the pool.
#[derive(Default)]
struct Slot {
    worker: Option<WorkerProcess>,
    /// Workers started in this seat so far.
    spawned: u64,
}

/// Runs files through a pool of worker child processes: a crash, an abort, an out-of-memory
/// kill or a hang costs one file, never the run.
///
/// Each worker is the current executable (or [`WorkerConfig::program`]) started in worker
/// mode, so the executable's `main` must call [`maybe_run_worker`](crate::maybe_run_worker)
/// first. Workers are started on first use and kept across runs; dropping the runner shuts
/// them down.
///
/// ```no_run
/// use quail_engine::{EngineBuilder, RunOptions};
/// use quail_worker::{IsolatedRunner, IsolationOptions, maybe_run_worker};
///
/// fn engine() -> quail_engine::Engine {
///     EngineBuilder::new().build().expect("engine")
/// }
///
/// fn main() {
///     maybe_run_worker(engine); // returns at once unless this process is a worker
///     # let mut source: Box<dyn quail_core::Source> = todo!();
///     # let mut sink: Box<dyn quail_core::RecordSink> = todo!();
///     let mut runner = IsolatedRunner::new(IsolationOptions::default());
///     let stats = runner.run(&mut *source, &mut *sink, &RunOptions::default()).unwrap();
///     eprintln!("{stats}");
/// }
/// ```
pub struct IsolatedRunner {
    opts: IsolationOptions,
    slots: Vec<Slot>,
}

impl fmt::Debug for IsolatedRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IsolatedRunner")
            .field("opts", &self.opts)
            .field("workers", &self.pids())
            .finish_non_exhaustive()
    }
}

impl IsolatedRunner {
    /// A runner with no worker started yet.
    pub fn new(opts: IsolationOptions) -> Self {
        Self {
            opts,
            slots: Vec::new(),
        }
    }

    /// Its options.
    pub fn options(&self) -> &IsolationOptions {
        &self.opts
    }

    /// Process ids of the live workers.
    pub fn pids(&self) -> Vec<u32> {
        self.slots
            .iter()
            .filter_map(|s| s.worker.as_ref().map(WorkerProcess::pid))
            .collect()
    }

    /// Makes sure `n` workers are running (at least one). Call it before a run to fail fast
    /// when workers cannot start at all (wrong executable, engine that does not build).
    pub fn warm_up(&mut self, n: usize) -> Result<(), WorkerError> {
        let n = n.max(1);
        if self.slots.len() < n {
            self.slots.resize_with(n, Slot::default);
        }
        for slot in &mut self.slots[..n] {
            if slot.worker.as_ref().is_none_or(|w| !w.is_alive()) {
                slot.worker = None;
                slot.worker = Some(WorkerProcess::spawn(&self.opts.worker)?);
                slot.spawned += 1;
            }
        }
        Ok(())
    }

    /// Limits the workers reported they could not apply (deduplicated).
    pub fn notes(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .slots
            .iter()
            .filter_map(|s| s.worker.as_ref())
            .flat_map(|w| w.notes().iter().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Processes one file in a worker, with the same crash handling (retry, respawn) as a
    /// run. Returns the record and, if `want_document`, the extracted document.
    pub fn process(&mut self, job: &Job, want_document: bool) -> (FileRecord, Option<Document>) {
        if self.slots.is_empty() {
            self.slots.push(Slot::default());
        }
        let stats = Mutex::new(IsolatedStats::default());
        let ctx = SlotCtx {
            opts: &self.opts,
            want_document,
            stats: &stats,
        };
        let size = job.size();
        ctx.handle(&mut self.slots[0], job, size)
    }

    /// [`Engine::run`](quail_engine::Engine::run), isolated: processes every item of
    /// `source` on `opts.threads` worker processes and writes each record to `sink` from a
    /// single writer thread, then calls [`RecordSink::finish`]. File bytes travel to the
    /// workers through their stdin; use [`run_jobs`](Self::run_jobs) with
    /// [`PathJobs`](crate::PathJobs) to let workers read files themselves.
    ///
    /// Honours `max_in_flight`, `max_in_flight_bytes`, `ordered`, `stop_on_sink_error` and
    /// `cancel` exactly as the in-process run does. `stack_size` is the worker's
    /// ([`WorkerConfig::stack_size`]). `on_artifacts` runs in the supervisor and sees only
    /// the `document` artifact (other artifacts do not leave the worker); setting it makes
    /// every worker send its document back.
    ///
    /// Err only when the first `opts.threads` workers cannot be started; later deaths are
    /// handled per file.
    ///
    /// # Panics
    ///
    /// If the operating system cannot create the supervisor or writer threads.
    pub fn run(
        &mut self,
        source: &mut dyn Source,
        sink: &mut dyn RecordSink,
        opts: &RunOptions,
    ) -> Result<IsolatedStats, WorkerError> {
        self.run_jobs(&mut FromSource(source), sink, opts)
    }

    /// [`run`](Self::run) over [`Job`]s, which may name a path the worker reads itself.
    ///
    /// # Panics
    ///
    /// If the operating system cannot create the supervisor or writer threads.
    pub fn run_jobs(
        &mut self,
        jobs: &mut dyn JobSource,
        sink: &mut dyn RecordSink,
        opts: &RunOptions,
    ) -> Result<IsolatedStats, WorkerError> {
        let started = Instant::now();
        let threads = opts.threads.max(1);
        self.warm_up(threads)?;
        let slots_n = opts.max_in_flight.max(1);
        let budget = opts.max_in_flight_bytes.max(1);
        let stall_after = self
            .slots
            .iter()
            .filter_map(|s| s.worker.as_ref())
            .map(|w| w.engine_timeout().saturating_mul(2))
            .max()
            .unwrap_or(Duration::MAX);
        let cfg = WriterCfg {
            ordered: opts.ordered,
            stop_on_sink_error: opts.stop_on_sink_error,
            stall_after,
        };
        let stats = Mutex::new(IsolatedStats::default());
        let stop = AtomicBool::new(false);
        let cancel = opts.cancel.as_deref();
        let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
        let hook = opts.on_artifacts.as_deref();

        let (work_tx, work_rx) = bounded::<(u64, u64, Job)>(slots_n);
        let (done_tx, done_rx) = bounded::<Done>(slots_n);
        let (freed_tx, freed_rx) = unbounded::<u64>();
        let mut counts = SourceCounts::default();
        let mut was_cancelled = false;
        let isolation = &self.opts;

        let writer_ok = thread::scope(|s| {
            let stats = &stats;
            let stop = &stop;
            for (i, slot) in self.slots[..threads].iter_mut().enumerate() {
                let rx = work_rx.clone();
                let tx = done_tx.clone();
                let ctx = SlotCtx {
                    opts: isolation,
                    want_document: hook.is_some(),
                    stats,
                };
                thread::Builder::new()
                    .name(format!("quail-supervisor-{i}"))
                    .spawn_scoped(s, move || ctx.serve(slot, &rx, &tx, hook))
                    .expect("spawn supervisor thread");
            }
            drop(work_rx);
            let writer = thread::Builder::new()
                .name("quail-writer".to_owned())
                .spawn_scoped(s, move || {
                    write_all(sink, &done_rx, &freed_tx, stats, stop, cfg)
                })
                .expect("spawn writer thread");
            let stop_now = || cancelled() || stop.load(Ordering::Relaxed);
            let mut gate = Gate {
                freed: &freed_rx,
                items: 0,
                bytes: 0,
            };
            counts = read_all(
                jobs, &work_tx, &done_tx, &mut gate, slots_n, budget, &stop_now,
            );
            was_cancelled = cancelled();
            drop(work_tx);
            drop(done_tx);
            writer.join().is_ok()
        });

        let mut stats = stats.into_inner().unwrap_or_else(PoisonError::into_inner);
        stats.run.writer_panicked = !writer_ok;
        stats.run.source_errors = counts.errors;
        stats.run.source_error_records = counts.error_records;
        stats.run.source_panics = counts.panics;
        stats.run.cancelled = was_cancelled;
        stats.run.elapsed = started.elapsed();
        Ok(stats)
    }
}

impl Drop for IsolatedRunner {
    fn drop(&mut self) {
        // Let every child see end of input at once, then reap them one by one.
        for s in &mut self.slots {
            if let Some(w) = s.worker.as_mut() {
                w.close_input();
            }
        }
        self.slots.clear();
    }
}

/// What a supervisor thread needs besides its seat.
struct SlotCtx<'a> {
    opts: &'a IsolationOptions,
    want_document: bool,
    stats: &'a Mutex<IsolatedStats>,
}

impl SlotCtx<'_> {
    fn lock(&self) -> std::sync::MutexGuard<'_, IsolatedStats> {
        self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn serve(
        &self,
        slot: &mut Slot,
        rx: &Receiver<(u64, u64, Job)>,
        tx: &Sender<Done>,
        hook: Option<&HookFn>,
    ) {
        for (seq, size, job) in rx {
            let (record, document) = self.handle(slot, &job, size);
            drop(job);
            if let Some(f) = hook
                && let Some(doc) = document
            {
                let mut artifacts = Artifacts::new();
                artifacts.insert(doc);
                if let Err(e) = panic::catch_unwind(AssertUnwindSafe(|| f(&record, &artifacts))) {
                    let mut st = self.lock();
                    st.run.hook_panics += 1;
                    st.run
                        .first_hook_panic
                        .get_or_insert_with(|| format!("{}: {}", record.locator, panic_text(&*e)));
                }
            }
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

    /// One file, with retry and poison handling.
    fn handle(&self, slot: &mut Slot, job: &Job, size: u64) -> (FileRecord, Option<Document>) {
        let mut first: Option<Crash> = None;
        loop {
            let worker = match self.ensure(slot) {
                Ok(w) => w,
                Err(e) => {
                    let error = match &first {
                        Some(c) => format!("worker crashed: {c}; no worker to retry in: {e}"),
                        None => format!("worker unavailable: {e}"),
                    };
                    return (failed(job, size, error), None);
                }
            };
            let kill_after = self.opts.kill_after.unwrap_or_else(|| {
                worker
                    .engine_timeout()
                    .saturating_mul(2)
                    .saturating_add(self.opts.grace)
            });
            match worker.process(job, self.want_document, kill_after) {
                Ok(mut p) => {
                    if !worker.is_alive() {
                        slot.worker = None;
                    }
                    if let Some(c) = &first {
                        self.lock().retried_ok += 1;
                        p.record.warnings.push(format!(
                            "worker: first attempt crashed ({c}); retried in a fresh worker"
                        ));
                    }
                    return (p.record, p.document);
                }
                Err(crash) => {
                    slot.worker = None;
                    let mut st = self.lock();
                    st.crash(&crash, &job.locator);
                    let retry = first.is_none()
                        && self.opts.retry_crashed
                        && (crash.kind != CrashKind::Timeout || self.opts.retry_timeouts);
                    if retry {
                        st.retries += 1;
                        first = Some(crash);
                        continue;
                    }
                    let error = match first {
                        Some(f) => {
                            st.poisoned += 1;
                            if f.kind == crash.kind {
                                format!("worker crashed: {crash} (twice; skipped)")
                            } else {
                                format!(
                                    "worker crashed: {f}; crashed again in a fresh worker: {crash}"
                                )
                            }
                        }
                        None => format!("worker crashed: {crash}"),
                    };
                    drop(st);
                    return (failed(job, size, error), None);
                }
            }
        }
    }

    /// The seat's live worker, starting one if needed.
    fn ensure<'s>(&self, slot: &'s mut Slot) -> Result<&'s mut WorkerProcess, WorkerError> {
        if slot.worker.as_ref().is_some_and(WorkerProcess::is_alive) {
            return slot.worker.as_mut().ok_or_else(unreachable_spawn);
        }
        slot.worker = None;
        let attempts = self.opts.spawn_attempts.max(1);
        let mut last = None;
        for i in 0..attempts {
            if i > 0 {
                thread::sleep(Duration::from_millis(50) * i);
            }
            match WorkerProcess::spawn(&self.opts.worker) {
                Ok(w) => {
                    if slot.spawned > 0 {
                        self.lock().respawns += 1;
                    }
                    slot.spawned += 1;
                    return Ok(slot.worker.insert(w));
                }
                Err(e) => {
                    self.lock().spawn_failures += 1;
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(unreachable_spawn))
    }
}

fn unreachable_spawn() -> WorkerError {
    WorkerError::Startup("no worker".to_owned())
}

fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned())
}

/// A `Failed` record for a file whose worker died, with the size and a cheap sniff of the
/// format done in the supervisor.
fn failed(job: &Job, size: u64, error: String) -> FileRecord {
    FileRecord {
        schema: SCHEMA_VERSION,
        locator: job.locator.clone(),
        size,
        format: sniff_job(job),
        pages: None,
        facts: Vec::new(),
        steps: Vec::new(),
        timings: Vec::new(),
        status: Status::Failed { error },
        warnings: Vec::new(),
    }
}

fn sniff_job(job: &Job) -> Format {
    match &job.input {
        JobInput::Bytes(b) => quail_engine::sniff(b.as_bytes()),
        JobInput::Path(p) => {
            let Ok(f) = File::open(p) else {
                return Format::Unknown;
            };
            let len = f.metadata().map_or(0, |m| m.len());
            let take = if len <= SNIFF_WHOLE {
                len
            } else {
                SNIFF_PREFIX
            };
            let mut buf = Vec::with_capacity(usize::try_from(take).unwrap_or(0));
            match f.take(take).read_to_end(&mut buf) {
                Ok(_) => quail_engine::sniff(&buf),
                Err(_) => Format::Unknown,
            }
        }
    }
}

/// A finished file on its way to the writer.
struct Done {
    seq: u64,
    bytes: u64,
    record: FileRecord,
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

#[derive(Default)]
struct SourceCounts {
    errors: u64,
    error_records: u64,
    panics: u64,
}

/// The pulling side of a run, on the caller's thread.
fn read_all(
    jobs: &mut dyn JobSource,
    work: &Sender<(u64, u64, Job)>,
    done: &Sender<Done>,
    gate: &mut Gate<'_>,
    slots: usize,
    budget: u64,
    stop_now: &dyn Fn() -> bool,
) -> SourceCounts {
    let mut counts = SourceCounts::default();
    let mut seq = 0u64;
    loop {
        if stop_now() {
            break;
        }
        match gate.wait(|items, _| items < slots, stop_now) {
            Wait::Ready => {}
            Wait::Stopped | Wait::WriterGone => break,
        }
        let job = match panic::catch_unwind(AssertUnwindSafe(|| jobs.next_job())) {
            Ok(None) => break,
            Ok(Some(Ok(job))) => job,
            Ok(Some(Err(e))) => {
                counts.errors += 1;
                if let Some(locator) = e.locator() {
                    let record = FileRecord {
                        schema: SCHEMA_VERSION,
                        locator: locator.to_owned(),
                        size: 0,
                        format: Format::Unknown,
                        pages: None,
                        facts: Vec::new(),
                        steps: Vec::new(),
                        timings: Vec::new(),
                        status: Status::Failed {
                            error: format!("source: {e}"),
                        },
                        warnings: Vec::new(),
                    };
                    gate.items += 1;
                    if done
                        .send(Done {
                            seq,
                            bytes: 0,
                            record,
                        })
                        .is_err()
                    {
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
        let size = job.size();
        let fits = |items: usize, bytes: u64| items == 0 || bytes.saturating_add(size) <= budget;
        if let Wait::WriterGone = gate.wait(fits, &|| false) {
            break;
        }
        gate.items += 1;
        gate.bytes += size;
        if work.send((seq, size, job)).is_err() {
            break;
        }
        seq += 1;
    }
    counts
}

#[derive(Clone, Copy)]
struct WriterCfg {
    ordered: bool,
    stop_on_sink_error: bool,
    stall_after: Duration,
}

/// The single writer: reorders if asked, writes, reports each freed slot to the reader.
fn write_all(
    sink: &mut dyn RecordSink,
    done: &Receiver<Done>,
    freed: &Sender<u64>,
    stats: &Mutex<IsolatedStats>,
    stop: &AtomicBool,
    cfg: WriterCfg,
) {
    let lock = || stats.lock().unwrap_or_else(PoisonError::into_inner);
    let mut emit = |d: Done| {
        {
            let mut guard = lock();
            let st = &mut guard.run;
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
            } else if let Err(e) = panic::catch_unwind(AssertUnwindSafe(|| sink.write(r)))
                .unwrap_or_else(|p| Err(quail_core::Error::Panic(panic_text(&*p))))
            {
                st.sink_errors += 1;
                st.first_sink_error.get_or_insert_with(|| e.to_string());
                if cfg.stop_on_sink_error {
                    st.stopped_on_sink_error = true;
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }
        let _ = freed.send(d.bytes);
    };

    let mut pending: HashMap<u64, Done> = HashMap::new();
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
        if let Some(d) = msg {
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
        }
        if !head_flagged && !pending.is_empty() && head_since.elapsed() > cfg.stall_after {
            lock().run.stalls += 1;
            head_flagged = true;
        }
    }
    // Gaps are impossible (every dispatched job yields a record), but never lose records.
    let mut rest: Vec<Done> = pending.into_values().collect();
    rest.sort_by_key(|d| d.seq);
    for d in rest {
        emit(d);
    }
    if let Err(e) = panic::catch_unwind(AssertUnwindSafe(|| sink.finish()))
        .unwrap_or_else(|p| Err(quail_core::Error::Panic(panic_text(&*p))))
    {
        let mut st = lock();
        st.run.sink_errors += 1;
        st.run.first_sink_error.get_or_insert_with(|| e.to_string());
    }
}

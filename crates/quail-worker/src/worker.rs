//! [`WorkerProcess`]: the supervisor's handle on one child, with its watchdog and the
//! classification of how it died.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, bounded};
use quail_core::{Document, FileRecord};

use crate::child::WORKER_ARG;
use crate::frame::{
    self, DATA, DOC, EXTEND, FATAL, HELLO, Hello, JOB, JobMsg, READY, RESULT, Ready, read_frame,
    split_extend, split_result, write_frame,
};
use crate::job::{Job, JobInput};
use crate::sys::{self, Death, nt, sig};

/// Default `RLIMIT_AS` of a worker: 1 GiB.
pub const DEFAULT_RLIMIT_AS: u64 = 1 << 30;
/// Default stack of the worker's processing thread: 32 MiB, as in-process
/// ([`quail_engine::DEFAULT_STACK_SIZE`]).
pub const DEFAULT_WORKER_STACK: usize = quail_engine::DEFAULT_STACK_SIZE;
/// Bytes of a child's stderr kept to explain its death.
const TAIL_BYTES: usize = 8 << 10;
/// How long a child that closed its output gets to finish exiting before it is killed.
const REAP_WAIT: Duration = Duration::from_secs(2);
/// How long the supervisor waits for a dead child's last stderr bytes.
const STDERR_DRAIN: Duration = Duration::from_secs(1);

/// What happens to a worker's stderr.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StderrMode {
    /// Copied to the supervisor's stderr (panic messages, crash reports). The default.
    #[default]
    Forward,
    /// Dropped. The last few kilobytes are still kept to explain a crash.
    Discard,
}

/// How to start a worker child and what limits it runs under.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Executable to start; `None` is the current executable
    /// ([`std::env::current_exe`]), which must call
    /// [`maybe_run_worker`](crate::maybe_run_worker) first thing in `main`.
    pub program: Option<PathBuf>,
    /// Extra arguments after [`WORKER_ARG`](crate::WORKER_ARG).
    pub args: Vec<OsString>,
    /// Address-space limit of the child (`RLIMIT_AS`, Unix). An allocation past it fails,
    /// which aborts the child: the file being processed fails, the run goes on. Covers
    /// everything the child maps, models included. `None`: no limit.
    pub rlimit_as: Option<u64>,
    /// CPU time per file (`RLIMIT_CPU`, re-armed before each file, Unix, whole seconds,
    /// rounded up). The kernel stops a child that spins past it with `SIGXCPU`. `None`: only
    /// the wall-clock watchdog.
    pub rlimit_cpu: Option<Duration>,
    /// Stack of the child's processing thread.
    pub stack_size: usize,
    /// Where the child's stderr goes.
    pub stderr: StderrMode,
    /// Time a new child gets to build its engine (load models) and report ready.
    pub startup_timeout: Duration,
    /// Largest message accepted from a child (a record plus, when asked, its document).
    pub max_response_bytes: u64,
}

impl Default for WorkerConfig {
    /// Current executable, [`DEFAULT_RLIMIT_AS`], no CPU limit, [`DEFAULT_WORKER_STACK`],
    /// stderr forwarded, 60 s startup, 1 GiB responses.
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            rlimit_as: Some(DEFAULT_RLIMIT_AS),
            rlimit_cpu: None,
            stack_size: DEFAULT_WORKER_STACK,
            stderr: StderrMode::Forward,
            startup_timeout: Duration::from_mins(1),
            max_response_bytes: 1 << 30,
        }
    }
}

/// A worker could not be started.
#[derive(Debug)]
pub enum WorkerError {
    /// The process could not be spawned (missing executable, no resources).
    Spawn(io::Error),
    /// It started but did not become ready: it died, timed out, or reported an error (the
    /// executable does not call [`maybe_run_worker`](crate::maybe_run_worker), or its engine
    /// failed to build).
    Startup(String),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot spawn worker: {e}"),
            Self::Startup(m) => write!(f, "worker did not start: {m}"),
        }
    }
}

impl std::error::Error for WorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(e) => Some(e),
            Self::Startup(_) => None,
        }
    }
}

/// How a worker died while processing a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CrashKind {
    /// The process aborted (`SIGABRT`, Windows fail-fast), for no reason it reported.
    Abort,
    /// An allocation failed, almost always against the `RLIMIT_AS` budget.
    OutOfMemory,
    /// The processing thread overflowed its stack.
    StackOverflow,
    /// The per-file CPU limit was hit (`SIGXCPU`).
    CpuLimit,
    /// No result within the wall-clock budget: the watchdog killed it.
    Timeout,
    /// Killed by `SIGKILL` from outside (the kernel's OOM killer, an operator).
    Killed,
    /// Killed by another signal.
    Signal(i32),
    /// Exited with a code (a panic outside the engine's containment exits with 101).
    Exit(i32),
    /// It sent something that is not the protocol, or reported a fatal error.
    Protocol,
}

impl CrashKind {
    /// Short stable name, used as the key of [`IsolatedStats::crash_kinds`](crate::IsolatedStats).
    pub fn label(self) -> &'static str {
        match self {
            Self::Abort => "abort",
            Self::OutOfMemory => "oom",
            Self::StackOverflow => "stack_overflow",
            Self::CpuLimit => "cpu_limit",
            Self::Timeout => "timeout",
            Self::Killed => "killed",
            Self::Signal(_) => "signal",
            Self::Exit(_) => "exit",
            Self::Protocol => "protocol",
        }
    }
}

/// A worker's death while it held a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crash {
    /// What kind of death.
    pub kind: CrashKind,
    /// Human description, e.g. `out of memory (SIGABRT); stderr: memory allocation of
    /// 33554432 bytes failed`.
    pub message: String,
}

impl fmt::Display for Crash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// What a worker returns for one file.
#[derive(Clone, Debug)]
pub struct Processed {
    /// The file's record, exactly as [`Engine::process`](quail_engine::Engine::process)
    /// made it in the child.
    pub record: FileRecord,
    /// The extracted document, when it was asked for and the extractor produced one.
    pub document: Option<Document>,
}

/// Why one exchange with the child failed.
enum Fail {
    /// The pipe broke or closed: the child is dying.
    Io(String),
    /// The child said something that is not the protocol; it may still be alive.
    Protocol(String),
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::InvalidData {
            Self::Protocol(e.to_string())
        } else {
            Self::Io(e.to_string())
        }
    }
}

/// One worker child process, owned by the supervisor: send it a [`Job`], get a
/// [`Processed`] back, or a [`Crash`] if it died on the way. After a crash the process is
/// gone; drop the handle and [`spawn`](Self::spawn) a new one.
///
/// Dropping the handle closes the child's stdin (it exits when its job loop sees the end of
/// input) and reaps it, killing it after a short wait.
pub struct WorkerProcess {
    child: Arc<Mutex<Child>>,
    stdin: Option<BufWriter<ChildStdin>>,
    stdout: BufReader<ChildStdout>,
    watchdog: Watchdog,
    tail: Arc<Mutex<Vec<u8>>>,
    stderr_done: Receiver<()>,
    stderr_thread: Option<JoinHandle<()>>,
    next_id: u64,
    max_response: u64,
    pid: u32,
    timeout: Duration,
    timeout_cap: Duration,
    max_bytes: u64,
    notes: Vec<String>,
    alive: bool,
}

impl fmt::Debug for WorkerProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerProcess")
            .field("pid", &self.pid)
            .field("alive", &self.alive)
            .finish_non_exhaustive()
    }
}

impl WorkerProcess {
    /// Starts a child and waits (at most [`WorkerConfig::startup_timeout`]) until its engine
    /// is built.
    pub fn spawn(cfg: &WorkerConfig) -> Result<Self, WorkerError> {
        let program = match &cfg.program {
            Some(p) => p.clone(),
            None => std::env::current_exe().map_err(WorkerError::Spawn)?,
        };
        let mut child = Command::new(program)
            .arg(WORKER_ARG)
            .args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(WorkerError::Spawn)?;
        let pid = child.id();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorkerError::Startup("child pipes missing".to_owned()));
        };
        let child = Arc::new(Mutex::new(child));
        let tail = Arc::new(Mutex::new(Vec::with_capacity(TAIL_BYTES)));
        let (done_tx, stderr_done) = bounded(1);
        let stderr_thread = {
            let tail = Arc::clone(&tail);
            let forward = cfg.stderr == StderrMode::Forward;
            thread::Builder::new()
                .name(format!("quail-worker-{pid}-stderr"))
                .spawn(move || {
                    pump_stderr(stderr, &tail, forward);
                    let _ = done_tx.send(());
                })
                .map_err(WorkerError::Spawn)?
        };
        let watchdog = Watchdog::start(Arc::clone(&child), pid).map_err(WorkerError::Spawn)?;
        let mut w = Self {
            child,
            stdin: Some(BufWriter::with_capacity(64 << 10, stdin)),
            stdout: BufReader::with_capacity(64 << 10, stdout),
            watchdog,
            tail,
            stderr_done,
            stderr_thread: Some(stderr_thread),
            next_id: 0,
            max_response: cfg.max_response_bytes,
            pid,
            timeout: Duration::ZERO,
            timeout_cap: Duration::ZERO,
            max_bytes: 0,
            notes: Vec::new(),
            alive: true,
        };
        match w.handshake(cfg) {
            Ok(ready) => {
                w.timeout = Duration::from_millis(ready.timeout_ms);
                w.timeout_cap = Duration::from_millis(ready.timeout_cap_ms).max(w.timeout);
                w.max_bytes = ready.max_bytes;
                w.notes = ready.notes;
                Ok(w)
            }
            Err(fail) => {
                let (protocol, msg) = match fail {
                    Fail::Io(m) => (false, m),
                    Fail::Protocol(m) => (true, m),
                };
                let crash = w.crash(protocol, &msg, cfg.startup_timeout);
                Err(WorkerError::Startup(if protocol {
                    crash.message
                } else {
                    format!("{msg}: {}", crash.message)
                }))
            }
        }
    }

    fn handshake(&mut self, cfg: &WorkerConfig) -> Result<Ready, Fail> {
        self.watchdog.arm(Instant::now() + cfg.startup_timeout);
        let hello = Hello {
            rlimit_as: cfg.rlimit_as,
            rlimit_cpu_secs: cfg.rlimit_cpu.map(|d| {
                d.as_secs()
                    .saturating_add(u64::from(d.subsec_nanos() > 0))
                    .max(1)
            }),
            stack_size: cfg.stack_size,
        };
        let hello = serde_json::to_vec(&hello).map_err(|e| Fail::Protocol(e.to_string()))?;
        write_frame(self.input()?, HELLO, &[&hello])?;
        let ready = match read_frame(&mut self.stdout, 1 << 20)? {
            Some(f) if f.tag == READY => serde_json::from_slice::<Ready>(&f.payload)
                .map_err(|e| Fail::Protocol(format!("bad ready message: {e}")))?,
            Some(f) if f.tag == FATAL => {
                return Err(Fail::Protocol(
                    String::from_utf8_lossy(&f.payload).into_owned(),
                ));
            }
            Some(f) => return Err(Fail::Protocol(format!("unexpected tag {}", f.tag))),
            None => {
                return Err(Fail::Io(
                    "exited before it was ready (does the executable call \
                     quail_worker::maybe_run_worker first in main?)"
                        .to_owned(),
                ));
            }
        };
        if self.watchdog.disarm() {
            return Err(Fail::Protocol(
                "ready only after the startup timeout".to_owned(),
            ));
        }
        Ok(ready)
    }

    fn input(&mut self) -> Result<&mut BufWriter<ChildStdin>, Fail> {
        self.stdin
            .as_mut()
            .ok_or_else(|| Fail::Io("input closed".to_owned()))
    }

    /// The child's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The child engine's [`Limits::timeout`](quail_core::Limits).
    pub fn engine_timeout(&self) -> Duration {
        self.timeout
    }

    /// The child engine's [`Limits::timeout_cap`](quail_core::Limits::timeout_cap): the
    /// longest a file's budget can become when a step extends its deadline.
    pub fn engine_timeout_cap(&self) -> Duration {
        self.timeout_cap
    }

    /// The child engine's [`Limits::max_bytes`](quail_core::Limits).
    pub fn engine_max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Limits the child could not apply (e.g. `RLIMIT_AS` on an OS without it).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// False once the child is known dead: after a [`Crash`], or when the watchdog fired just
    /// as a result arrived. Spawn a new one then.
    pub fn is_alive(&self) -> bool {
        self.alive
    }

    /// Processes one file in the child. Kills the child if no result arrives within
    /// `kill_after` (from the moment the job is sent), moved later by twice every extension
    /// of the file's deadline the child reports
    /// ([`Emitter::extend_deadline`](quail_core::Emitter::extend_deadline)), in all at most
    /// twice the difference between [`engine_timeout_cap`](Self::engine_timeout_cap) and
    /// [`engine_timeout`](Self::engine_timeout).
    ///
    /// `want_document` asks for the extracted [`Document`] too (it costs a JSON round trip of
    /// the whole text).
    pub fn process(
        &mut self,
        job: &Job,
        want_document: bool,
        kill_after: Duration,
    ) -> Result<Processed, Crash> {
        if !self.alive {
            return Err(Crash {
                kind: CrashKind::Killed,
                message: "worker already dead".to_owned(),
            });
        }
        self.tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let id = self.next_id;
        self.next_id += 1;
        let sent = Instant::now();
        self.watchdog.arm(sent + kill_after);
        match self.exchange(id, job, want_document, sent + kill_after) {
            Ok(p) => {
                if self.watchdog.disarm() {
                    // Killed just after it answered: the record is good, the process is not.
                    self.alive = false;
                }
                Ok(p)
            }
            Err(Fail::Io(m)) => Err(self.crash(false, &m, kill_after)),
            Err(Fail::Protocol(m)) => Err(self.crash(true, &m, kill_after)),
        }
    }

    fn exchange(
        &mut self,
        id: u64,
        job: &Job,
        want_document: bool,
        kill_at: Instant,
    ) -> Result<Processed, Fail> {
        let msg = JobMsg {
            id,
            locator: job.locator.clone(),
            modified: job.modified,
            want_document,
            path: match &job.input {
                JobInput::Path(p) => Some(frame::path_to_bytes(p)),
                JobInput::Bytes(_) => None,
            },
        };
        let msg = serde_json::to_vec(&msg).map_err(|e| Fail::Protocol(e.to_string()))?;
        let input = self.input()?;
        write_frame(input, JOB, &[&msg])?;
        if let JobInput::Bytes(blob) = &job.input {
            write_frame(input, DATA, &[blob.as_bytes()])?;
        }
        let f = loop {
            let f = self.next_frame()?;
            if f.tag != EXTEND {
                break f;
            }
            let (got, budget_ms) = split_extend(&f.payload)
                .ok_or_else(|| Fail::Protocol("bad extend frame".to_owned()))?;
            if got != id {
                return Err(Fail::Protocol(format!(
                    "extend for job {got}, expected {id}"
                )));
            }
            // Never more than the engine's cap allows, whatever the child says.
            let extra = Duration::from_millis(budget_ms)
                .min(self.timeout_cap)
                .saturating_sub(self.timeout);
            if let Some(at) = kill_at.checked_add(extra.saturating_mul(2)) {
                self.watchdog.extend(at);
            }
        };
        if f.tag != RESULT {
            return Err(unexpected(&f));
        }
        let (got, has_doc, json) =
            split_result(&f.payload).ok_or_else(|| Fail::Protocol("short result".to_owned()))?;
        if got != id {
            return Err(Fail::Protocol(format!(
                "result for job {got}, expected {id}"
            )));
        }
        let record: FileRecord =
            serde_json::from_slice(json).map_err(|e| Fail::Protocol(format!("bad record: {e}")))?;
        let document = if has_doc {
            let f = self.next_frame()?;
            if f.tag != DOC {
                return Err(unexpected(&f));
            }
            Some(
                serde_json::from_slice(&f.payload)
                    .map_err(|e| Fail::Protocol(format!("bad document: {e}")))?,
            )
        } else {
            None
        };
        Ok(Processed { record, document })
    }

    fn next_frame(&mut self) -> Result<frame::Frame, Fail> {
        read_frame(&mut self.stdout, self.max_response)?
            .ok_or_else(|| Fail::Io("worker closed its output".to_owned()))
    }

    /// The child failed an exchange: make sure it is dead, reap it and say why it died.
    fn crash(&mut self, protocol: bool, msg: &str, budget: Duration) -> Crash {
        self.alive = false;
        // Disarm first: once the child is reaped, a late firing would be a false timeout.
        let fired = self.watchdog.disarm();
        if protocol {
            self.kill();
        }
        let status = self.reap();
        let _ = self.stderr_done.recv_timeout(STDERR_DRAIN);
        let tail = {
            let t = self.tail.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&t).into_owned()
        };
        classify(fired, protocol.then_some(msg), status, &tail, budget)
    }

    fn kill(&self) {
        let mut c = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = c.kill();
    }

    /// Waits for the child to exit, killing it after [`REAP_WAIT`].
    fn reap(&mut self) -> Option<ExitStatus> {
        self.stdin = None;
        let until = Instant::now() + REAP_WAIT;
        loop {
            {
                let mut c = self.child.lock().unwrap_or_else(PoisonError::into_inner);
                match c.try_wait() {
                    Ok(Some(s)) => return Some(s),
                    Ok(None) if Instant::now() >= until => {
                        let _ = c.kill();
                        return c.wait().ok();
                    }
                    Ok(None) => {}
                    Err(_) => return None,
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Closes the child's input so it starts exiting; [`Drop`] then reaps it. Lets a pool
    /// shut down all its children in parallel.
    pub(crate) fn close_input(&mut self) {
        self.stdin = None;
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        let _ = self.reap();
        self.watchdog.stop();
        let _ = self.stderr_done.recv_timeout(STDERR_DRAIN);
        if let Some(t) = self.stderr_thread.take()
            && t.is_finished()
        {
            let _ = t.join();
        }
    }
}

fn unexpected(f: &frame::Frame) -> Fail {
    if f.tag == FATAL {
        Fail::Protocol(format!(
            "worker reported: {}",
            String::from_utf8_lossy(&f.payload)
        ))
    } else {
        Fail::Protocol(format!("unexpected frame tag {}", f.tag))
    }
}

/// Copies the child's stderr, keeping the last [`TAIL_BYTES`].
fn pump_stderr(mut r: impl Read, tail: &Mutex<Vec<u8>>, forward: bool) {
    let mut buf = [0u8; 4096];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if forward {
            let _ = io::stderr().write_all(&buf[..n]);
        }
        let mut t = tail.lock().unwrap_or_else(PoisonError::into_inner);
        t.extend_from_slice(&buf[..n]);
        if t.len() > TAIL_BYTES {
            let cut = t.len() - TAIL_BYTES;
            t.drain(..cut);
        }
    }
}

/// Names a death from what is known about it.
fn classify(
    timed_out: bool,
    protocol: Option<&str>,
    status: Option<ExitStatus>,
    stderr: &str,
    budget: Duration,
) -> Crash {
    let oom = stderr.contains("memory allocation of") || stderr.contains("out of memory");
    let stack = stderr.contains("overflowed its stack") || stderr.contains("stack overflow");
    let death = status.map_or(Death::Unknown, sys::death);
    let how = match death {
        Death::Signal(s) => format!("{} ({s})", sys::signal_name(s)),
        Death::Exit(c) => format!("exit code {c}"),
        Death::Unknown => "unknown exit".to_owned(),
    };
    let (kind, what) = if timed_out {
        (
            CrashKind::Timeout,
            format!("timed out: no result after {budget:.1?}, killed by the watchdog"),
        )
    } else if let Some(m) = protocol {
        (CrashKind::Protocol, format!("protocol error: {m}"))
    } else {
        match death {
            _ if stack => (CrashKind::StackOverflow, format!("stack overflow ({how})")),
            Death::Signal(s) if s == sig::ABRT && oom => (
                CrashKind::OutOfMemory,
                format!(
                    "out of memory: an allocation failed under the worker's memory limit ({how})"
                ),
            ),
            Death::Exit(c) if c == nt::FAST_FAIL && oom => (
                CrashKind::OutOfMemory,
                format!("out of memory: an allocation failed ({how})"),
            ),
            Death::Signal(s) if s == sig::ABRT => (CrashKind::Abort, format!("aborted ({how})")),
            Death::Exit(c) if c == nt::FAST_FAIL => (CrashKind::Abort, format!("aborted ({how})")),
            Death::Exit(c) if c == nt::STACK_OVERFLOW => {
                (CrashKind::StackOverflow, format!("stack overflow ({how})"))
            }
            Death::Signal(s) if s == sig::XCPU => (
                CrashKind::CpuLimit,
                format!("CPU time limit exceeded ({how})"),
            ),
            Death::Signal(s) if s == sig::KILL => (
                CrashKind::Killed,
                format!("killed from outside, e.g. by the kernel OOM killer ({how})"),
            ),
            Death::Signal(s) if s == sig::SEGV || s == sig::BUS => {
                (CrashKind::Signal(s), format!("memory fault ({how})"))
            }
            Death::Exit(c) if c == nt::ACCESS_VIOLATION => {
                (CrashKind::Exit(c), format!("memory fault ({how})"))
            }
            Death::Signal(s) => (CrashKind::Signal(s), format!("killed by {how}")),
            Death::Exit(c) => (CrashKind::Exit(c), format!("exited with code {c}")),
            Death::Unknown => (CrashKind::Exit(-1), "died (exit status unknown)".to_owned()),
        }
    };
    let message = match last_line(stderr) {
        Some(l) => format!("{what}; stderr: {l}"),
        None => what,
    };
    Crash { kind, message }
}

/// The stderr line that best explains a death (a Rust runtime message if any, else the
/// last non-empty line, skipping backtrace hints), shortened.
fn last_line(s: &str) -> Option<String> {
    const TELLING: [&str; 4] = [
        "memory allocation of",
        "overflowed its stack",
        "panicked at",
        "fatal runtime error",
    ];
    let lines = || s.lines().rev().map(str::trim).filter(|l| !l.is_empty());
    let l = TELLING
        .iter()
        .find_map(|m| lines().find(|l| l.contains(m)))
        .or_else(|| lines().find(|l| !l.starts_with("note: run with")))?;
    let mut end = l.len().min(240);
    while !l.is_char_boundary(end) {
        end -= 1;
    }
    Some(l[..end].to_owned())
}

/// Kills the child when an armed deadline passes. One small thread per child.
struct Watchdog {
    shared: Arc<(Mutex<WatchState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct WatchState {
    deadline: Option<Instant>,
    fired: bool,
    stop: bool,
}

impl Watchdog {
    fn start(child: Arc<Mutex<Child>>, pid: u32) -> io::Result<Self> {
        let shared = Arc::new((Mutex::new(WatchState::default()), Condvar::new()));
        let s = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name(format!("quail-worker-{pid}-watchdog"))
            .spawn(move || {
                let (lock, cv) = &*s;
                let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    if st.stop {
                        return;
                    }
                    match st.deadline {
                        None => st = cv.wait(st).unwrap_or_else(PoisonError::into_inner),
                        Some(d) => {
                            let now = Instant::now();
                            if now >= d {
                                st.fired = true;
                                st.deadline = None;
                                let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
                            } else {
                                st = cv
                                    .wait_timeout(st, d - now)
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .0;
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Moves an armed deadline later (never earlier); does nothing once it fired or when
    /// disarmed.
    fn extend(&self, deadline: Instant) {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(d) = st.deadline
            && deadline > d
            && !st.fired
        {
            st.deadline = Some(deadline);
            cv.notify_one();
        }
    }

    fn arm(&self, deadline: Instant) {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
        st.deadline = Some(deadline);
        st.fired = false;
        cv.notify_one();
    }

    /// Disarms; true if it fired since it was armed.
    fn disarm(&self) -> bool {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
        st.deadline = None;
        cv.notify_one();
        std::mem::take(&mut st.fired)
    }

    fn stop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
            st.stop = true;
            cv.notify_one();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_line_is_trimmed_and_bounded() {
        assert_eq!(last_line("a\n  b  \n\n"), Some("b".to_owned()));
        assert_eq!(last_line(""), None);
        let long = "é".repeat(500);
        assert!(last_line(&long).unwrap().len() <= 240);
    }

    #[test]
    fn classify_prefers_timeout_and_protocol() {
        let c = classify(true, None, None, "", Duration::from_secs(1));
        assert_eq!(c.kind, CrashKind::Timeout);
        let c = classify(false, Some("bad magic"), None, "", Duration::from_secs(1));
        assert_eq!(c.kind, CrashKind::Protocol);
        let c = classify(
            false,
            None,
            None,
            "thread 'x' has overflowed its stack",
            Duration::ZERO,
        );
        assert_eq!(c.kind, CrashKind::StackOverflow);
        assert!(
            c.message
                .contains("stderr: thread 'x' has overflowed its stack")
        );
    }
}

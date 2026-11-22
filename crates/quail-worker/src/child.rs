//! The worker side: [`maybe_run_worker`] and the job loop of a child process.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use quail_core::{Blob, Document, FileRecord, Format, SCHEMA_VERSION, SourceItem, Status};
use quail_engine::Engine;

use crate::frame::{
    self, DATA, DOC, EXTEND, FATAL, HELLO, Hello, JOB, JobMsg, READY, RESULT, Ready, read_frame,
    write_frame,
};
use crate::sys;

/// The hidden first argument that puts an executable in worker mode.
pub const WORKER_ARG: &str = "__quail-worker";

/// Largest control frame (HELLO, JOB) a child accepts.
const MAX_CONTROL: u64 = 16 << 20;

/// Turns this process into a worker child if it was started as one, and otherwise returns
/// at once.
///
/// Call it first thing in `main`, before parsing arguments or printing anything: the
/// supervisor ([`IsolatedRunner`](crate::IsolatedRunner)) re-runs the current executable
/// with [`WORKER_ARG`] as its first argument and talks to it over stdin and stdout. In worker
/// mode this applies the resource limits, calls `build_engine` once (so models load once per
/// child), serves jobs until the supervisor closes stdin, and then exits the process; it
/// never returns.
///
/// In a worker, anything the engine prints to stdout goes to stderr instead (on Unix), so
/// a stray `println!` cannot corrupt the protocol.
pub fn maybe_run_worker(build_engine: impl FnOnce() -> Engine + Send + 'static) {
    if std::env::args_os().nth(1).is_some_and(|a| a == WORKER_ARG) {
        std::process::exit(run_worker(build_engine));
    }
}

/// Worker mode proper. Returns the exit code.
fn run_worker(build_engine: impl FnOnce() -> Engine + Send + 'static) -> i32 {
    let out = match protocol_stdout() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("quail-worker: cannot set up the protocol channel: {e}");
            return 2;
        }
    };
    let mut out = BufWriter::with_capacity(64 << 10, out);
    let mut input = BufReader::with_capacity(64 << 10, io::stdin());

    let hello: Hello = match read_frame(&mut input, MAX_CONTROL) {
        Ok(Some(f)) if f.tag == HELLO => match serde_json::from_slice(&f.payload) {
            Ok(h) => h,
            Err(e) => return fatal(&mut out, &format!("bad hello: {e}")),
        },
        Ok(None) => return 0,
        Ok(Some(f)) => return fatal(&mut out, &format!("expected hello, got tag {}", f.tag)),
        Err(e) => return fatal(&mut out, &format!("reading hello: {e}")),
    };
    let notes = sys::apply_child_limits(hello.rlimit_as);
    let cpu = hello.rlimit_cpu_secs;
    // Parsers recurse: process on a thread with the stack the supervisor asked for. `input`
    // and `out` move in; this thread only waits.
    let worker = thread::Builder::new()
        .name("quail-worker".to_owned())
        .stack_size(hello.stack_size.max(1 << 20))
        .spawn(move || {
            let engine = build_engine();
            let ms = |d: std::time::Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
            let ready = Ready {
                pid: std::process::id(),
                timeout_ms: ms(engine.limits().timeout),
                timeout_cap_ms: ms(engine.limits().timeout_cap()),
                max_bytes: engine.limits().max_bytes,
                notes,
            };
            let ready = serde_json::to_vec(&ready).unwrap_or_default();
            if write_frame(&mut out, READY, &[&ready]).is_err() {
                return 0;
            }
            serve(&engine, &mut input, &Arc::new(Mutex::new(out)), cpu)
        });
    match worker.map(thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        // A panic outside the engine's own containment (in `build_engine`, say): the
        // supervisor reads the exit code and the message on stderr.
        Ok(Err(_)) => 101,
        Err(e) => {
            eprintln!("quail-worker: cannot spawn the processing thread: {e}");
            3
        }
    }
}

fn fatal(out: &mut impl Write, msg: &str) -> i32 {
    let _ = write_frame(out, FATAL, &[msg.as_bytes()]);
    eprintln!("quail-worker: {msg}");
    4
}

/// The job loop. Returns the exit code.
///
/// `out` is shared with the deadline hook, which writes an [`EXTEND`] frame from inside the
/// engine whenever a step extends the file's deadline, so that the supervisor's watchdog
/// moves along while the step still runs.
fn serve<W: Write + Send + 'static>(
    engine: &Engine,
    input: &mut impl Read,
    out: &Arc<Mutex<W>>,
    cpu: Option<u64>,
) -> i32 {
    let lock = || out.lock().unwrap_or_else(PoisonError::into_inner);
    // A DATA frame may be as big as the engine reads at all, plus slack for oversized files
    // that are only hashed.
    let max_data = engine.limits().max_bytes.saturating_mul(2).max(1 << 30);
    loop {
        let job: JobMsg = match read_frame(input, MAX_CONTROL) {
            Ok(None) => return 0,
            Ok(Some(f)) if f.tag == JOB => match serde_json::from_slice(&f.payload) {
                Ok(j) => j,
                Err(e) => return fatal(&mut *lock(), &format!("bad job: {e}")),
            },
            Ok(Some(f)) => return fatal(&mut *lock(), &format!("expected job, got tag {}", f.tag)),
            Err(e) => return fatal(&mut *lock(), &format!("reading job: {e}")),
        };
        let blob = match job.path {
            None => match read_frame(input, max_data) {
                Ok(Some(f)) if f.tag == DATA => Ok(Blob::from(f.payload)),
                Ok(_) => return fatal(&mut *lock(), "expected data frame"),
                Err(e) => return fatal(&mut *lock(), &format!("reading data: {e}")),
            },
            Some(p) => {
                read_capped(&frame::path_from_bytes(p), engine.limits().max_bytes).map(Blob::from)
            }
        };
        if let Some(secs) = cpu {
            sys::arm_cpu_limit(secs);
        }
        let mut doc_json = None;
        let record = match blob {
            Ok(blob) => {
                let item = SourceItem {
                    locator: job.locator,
                    blob,
                    modified: job.modified,
                };
                let on_budget = {
                    let (out, id) = (Arc::clone(out), job.id);
                    Arc::new(move |budget: std::time::Duration| {
                        let ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX);
                        let mut w = out.lock().unwrap_or_else(PoisonError::into_inner);
                        // A dead supervisor shows up when the result is written.
                        let _ = write_frame(&mut *w, EXTEND, &[&frame::extend_payload(id, ms)]);
                    })
                };
                let want = job.want_document;
                engine.process_watched(item, on_budget, |_, artifacts| {
                    if want {
                        doc_json = artifacts
                            .get::<Document>()
                            .and_then(|d| serde_json::to_vec(d).ok());
                    }
                })
            }
            Err(e) => read_failed(job.locator, &e),
        };
        let json = match serde_json::to_vec(&record) {
            Ok(j) => j,
            Err(e) => return fatal(&mut *lock(), &format!("cannot encode record: {e}")),
        };
        let mut head = [0u8; 9];
        head[..8].copy_from_slice(&job.id.to_le_bytes());
        head[8] = u8::from(doc_json.is_some());
        let mut w = lock();
        if write_frame(&mut *w, RESULT, &[&head, &json]).is_err() {
            return 0; // supervisor gone
        }
        if let Some(d) = doc_json
            && write_frame(&mut *w, DOC, &[&d]).is_err()
        {
            return 0;
        }
    }
}

/// The record of a file the child could not read (path mode).
/// Reads a file as `quail-io` does with `max_file_bytes = cap` (what `quail scan` sets from
/// `max_bytes`): a file over `cap` is refused, and one that grows while being read stops at
/// `cap + 1` bytes, so memory stays bounded and the record matches an in-process run.
fn read_capped(path: &std::path::Path, cap: u64) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > cap {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("{len} bytes exceeds the source cap of {cap} bytes"),
        ));
    }
    // The length is a hint only: never pre-allocate more than the cap or 1 GiB.
    let hint = usize::try_from(len.min(cap).min(1 << 30)).unwrap_or(0);
    let mut buf = Vec::with_capacity(hint);
    file.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("grew past the source cap of {cap} bytes while being read"),
        ));
    }
    Ok(buf)
}

fn read_failed(locator: String, e: &io::Error) -> FileRecord {
    FileRecord {
        schema: SCHEMA_VERSION,
        locator,
        size: 0,
        format: Format::Unknown,
        pages: None,
        facts: Vec::new(),
        steps: Vec::new(),
        timings: Vec::new(),
        status: Status::Failed {
            error: format!("worker: io: {e}"),
        },
        warnings: Vec::new(),
    }
}

/// The channel to write frames on: a duplicate of stdout, after which stdout itself is
/// pointed at stderr (Unix), so prints from analyzers cannot interleave with frames.
#[cfg(unix)]
fn protocol_stdout() -> io::Result<File> {
    use std::os::fd::AsFd;
    let proto = io::stdout().as_fd().try_clone_to_owned()?;
    nix::unistd::dup2_stdout(io::stderr()).map_err(io::Error::from)?;
    Ok(File::from(proto))
}

/// The channel to write frames on: a duplicate of stdout. A stray print to stdout would
/// break the protocol here; the supervisor detects it (bad magic) and records a crash.
#[cfg(windows)]
fn protocol_stdout() -> io::Result<File> {
    use std::os::windows::io::AsHandle;
    Ok(File::from(io::stdout().as_handle().try_clone_to_owned()?))
}

#[cfg(not(any(unix, windows)))]
fn protocol_stdout() -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "worker mode needs Unix or Windows",
    ))
}

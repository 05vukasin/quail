//! `quail scan --isolate`: every file is processed in a worker child process
//! ([`quail_worker`]), so an abort, an out-of-memory kill, a stack overflow or a hang costs
//! one `Failed` record instead of the run.
//!
//! - [`maybe_serve`] runs first thing in `main`: when the supervisor started this executable
//!   as a worker, it builds the engine from [`WorkerSpec`] (profile and limits, passed as one
//!   argument) and serves jobs until the supervisor closes its stdin.
//! - [`jobs`] turns the scan's source (a directory walk or a manifest) into path jobs: the
//!   worker reads each file itself, so file bytes never cross the pipe. The size cap and the
//!   "not a regular file" check run in the supervisor, with the same `Failed` records an
//!   in-process run writes.
//! - [`options`] maps `--worker-mem` and `--retry-crashed`/`--no-retry`.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use quail_core::{Error, Limits, Profile, Result};
use quail_engine::Engine;
use quail_io::{FsSource, ListSource};
use quail_worker::{IsolationOptions, Job, JobInput, JobSource, WORKER_ARG, WorkerConfig};
use serde::{Deserialize, Serialize};

use crate::registry;

/// What a worker needs to build the same engine as the supervisor would. Travels as one JSON
/// argument after [`WORKER_ARG`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerSpec {
    /// Work profile.
    pub profile: Profile,
    /// Per-file limits.
    pub limits: Limits,
    /// OCR settings of each worker (`None`: its defaults).
    #[serde(default)]
    pub ocr: Option<registry::OcrSettings>,
}

impl WorkerSpec {
    /// The worker arguments carrying this spec.
    pub fn args(&self) -> Vec<OsString> {
        vec![serde_json::to_string(self).unwrap_or_default().into()]
    }

    /// The spec from a worker's arguments (`[program, WORKER_ARG, spec]`).
    pub fn from_args(
        mut args: impl Iterator<Item = OsString>,
    ) -> std::result::Result<Self, String> {
        let spec = args
            .find(|a| a == WORKER_ARG)
            .and_then(|_| args.next())
            .ok_or("missing worker spec argument")?;
        let spec = spec.to_str().ok_or("worker spec is not UTF-8")?;
        serde_json::from_str(spec).map_err(|e| format!("bad worker spec: {e}"))
    }
}

/// Turns this process into a worker if the supervisor started it as one; returns at once
/// otherwise. Call it before parsing arguments.
pub fn maybe_serve() {
    quail_worker::maybe_run_worker(worker_engine);
}

/// The worker's engine. A bad spec or an engine that does not build ends the child with a
/// message on stderr; the supervisor reports it as a startup failure.
fn worker_engine() -> Engine {
    let spec = WorkerSpec::from_args(std::env::args_os()).unwrap_or_else(|e| die(&e));
    let b = registry::builder_with(spec.profile, spec.limits, spec.ocr.as_ref());
    #[cfg(feature = "test-hooks")]
    let b = test_hooks::install(b);
    b.build().unwrap_or_else(|e| die(&e.to_string()))
}

fn die(msg: &str) -> ! {
    eprintln!("quail: worker: {msg}");
    std::process::exit(2)
}

/// Supervisor options from the scan arguments.
pub fn options(spec: &WorkerSpec, worker_mem: Option<u64>, retry: bool) -> IsolationOptions {
    IsolationOptions {
        worker: WorkerConfig {
            args: spec.args(),
            rlimit_as: worker_mem.or(Some(quail_worker::DEFAULT_RLIMIT_AS)),
            ..WorkerConfig::default()
        },
        retry_crashed: retry,
        ..IsolationOptions::default()
    }
}

/// Where the paths of an isolated scan come from.
pub enum PathSource {
    /// A directory walk; the locator is the path.
    Walk(Box<FsSource>),
    /// A manifest: `(locator, path)` entries.
    Manifest(Box<dyn ManifestEntries>),
}

/// [`ListSource::next_entry`] over any reader.
pub trait ManifestEntries: Send {
    /// The next `(locator, path)`.
    fn next_entry(&mut self) -> Option<Result<(String, PathBuf)>>;
}

impl<R: BufRead + Send> ManifestEntries for ListSource<R> {
    fn next_entry(&mut self) -> Option<Result<(String, PathBuf)>> {
        ListSource::next_entry(self)
    }
}

/// Path jobs of a scan, checked against the size cap the in-process source applies.
pub struct ScanJobs {
    paths: PathSource,
    cap: u64,
}

/// Jobs for `paths`, refusing files over `cap` bytes before any worker sees them.
pub fn jobs(paths: PathSource, cap: u64) -> ScanJobs {
    ScanJobs { paths, cap }
}

impl JobSource for ScanJobs {
    fn next_job(&mut self) -> Option<Result<Job>> {
        Some(match &mut self.paths {
            PathSource::Walk(fs) => match fs.next_path()? {
                Ok(p) => {
                    let locator = p.to_string_lossy().into_owned();
                    check(&p, self.cap)
                        .map(|modified| job(locator.clone(), p, modified))
                        .map_err(|e| Error::at(locator, Error::Io(e)))
                }
                Err(e) => Err(e),
            },
            PathSource::Manifest(m) => match m.next_entry()? {
                Ok((locator, p)) => match check(&p, self.cap) {
                    Ok(modified) => Ok(job(locator, p, modified)),
                    // As the manifest source does: the locator may be an id, keep the path.
                    Err(e) => {
                        let e = io::Error::new(e.kind(), format!("{}: {e}", p.display()));
                        Err(Error::at(locator, Error::Io(e)))
                    }
                },
                Err(e) => Err(e),
            },
        })
    }
}

fn job(locator: String, path: PathBuf, modified: Option<i64>) -> Job {
    Job {
        locator,
        input: JobInput::Path(path),
        modified,
    }
}

/// What `quail-io` checks before reading a file: it is a regular file, opens and is within
/// the cap. Returns its modification time (seconds since the epoch). The file type is checked
/// before opening, so a directory is refused the same way on every OS (Windows cannot open
/// one: `PermissionDenied`).
fn check(path: &Path, cap: u64) -> io::Result<Option<i64>> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    // Opened to refuse unreadable files here, as an in-process run does.
    let meta = File::open(path)?.metadata()?;
    let len = meta.len();
    if len > cap {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("{len} bytes exceeds the source cap of {cap} bytes"),
        ));
    }
    Ok(meta.modified().ok().and_then(unix_seconds))
}

fn unix_seconds(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// Test-only hooks, compiled with the `test-hooks` feature:
///
/// - a worker whose `QUAIL_TEST_CRASH_ON` environment variable is set aborts on every file
///   whose locator contains it, standing in for a parser bug that kills the process;
/// - with `QUAIL_TEST_STALL` set (worker or in process), a last step runs until the file's
///   deadline, so a test can read the file's effective timeout from its timings.
#[cfg(feature = "test-hooks")]
pub(crate) mod test_hooks {
    use std::time::Duration;

    use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, Result};
    use quail_engine::EngineBuilder;

    /// The environment variable naming the locator substring to crash on.
    pub const ENV: &str = "QUAIL_TEST_CRASH_ON";
    /// Set (not empty): add [`Stall`].
    pub const STALL_ENV: &str = "QUAIL_TEST_STALL";

    /// A finalizer that waits for the file's deadline, then times out.
    struct Stall;

    impl Analyzer for Stall {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("test.stall", 1)
        }

        fn finalizer(&self) -> bool {
            true
        }

        fn run(&self, _: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
            loop {
                out.check_deadline()?;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Adds the stall step when its variable is set.
    pub fn install_stall(b: EngineBuilder) -> EngineBuilder {
        match std::env::var(STALL_ENV) {
            Ok(s) if !s.is_empty() => b.analyzer(Box::new(Stall)),
            _ => b,
        }
    }

    struct CrashTrap(String);

    impl Analyzer for CrashTrap {
        fn id(&self) -> AnalyzerId {
            AnalyzerId::new("test.crash_trap", 1)
        }

        fn run(&self, ctx: &Ctx<'_>, _: &mut Emitter) -> Result<()> {
            if ctx.locator.contains(&self.0) {
                std::process::abort();
            }
            Ok(())
        }
    }

    /// Adds the trap when the variable is set and not empty (and the stall step when its is).
    pub fn install(b: EngineBuilder) -> EngineBuilder {
        let b = install_stall(b);
        match std::env::var(ENV) {
            Ok(s) if !s.is_empty() => b.analyzer(Box::new(CrashTrap(s))),
            _ => b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips_through_worker_args() {
        let spec = WorkerSpec {
            profile: Profile::Fast,
            limits: Limits {
                max_pages: 7,
                ..Limits::default()
            },
            ocr: Some(registry::OcrSettings {
                jobs: 1,
                threads: 3,
                max_pages: 9,
                page_time: Some(std::time::Duration::from_millis(1500)),
            }),
        };
        let mut argv = vec![OsString::from("quail"), OsString::from(WORKER_ARG)];
        argv.extend(spec.args());
        assert_eq!(WorkerSpec::from_args(argv.into_iter()), Ok(spec));
        assert!(WorkerSpec::from_args(vec![OsString::from("quail")].into_iter()).is_err());
    }

    /// Records come back from workers as JSON: floats must parse back to the same bits.
    #[test]
    fn floats_survive_the_worker_pipe() {
        for x in [
            0.999_893_605_709_075_9_f64,
            0.998_699_605_464_935_3,
            1.0 / 3.0,
            1e-300,
        ] {
            let back: f64 = serde_json::from_str(&serde_json::to_string(&x).unwrap()).unwrap();
            assert_eq!(back.to_bits(), x.to_bits(), "{x}");
        }
    }

    #[test]
    fn oversized_and_special_files_are_refused_before_a_worker() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f");
        std::fs::write(&p, [0u8; 100]).unwrap();
        assert!(check(&p, 100).unwrap().is_some());
        let e = check(&p, 99).unwrap_err();
        assert!(e.to_string().contains("source cap of 99"), "{e}");
        assert_eq!(
            check(t.path(), 1 << 20).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

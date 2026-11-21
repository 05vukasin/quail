//! `quail scan`: many files to JSONL.

use std::io::{self, BufReader, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use quail_core::{Limits, Source};
use quail_engine::{Engine, RunOptions};
use quail_io::{FsSource, JsonlSink, ListSource, ReadOptions};

use crate::cli::ScanArgs;
use quail_worker::IsolatedRunner;

use crate::dump::DocumentDump;
use crate::isolate::{self, PathSource, ScanJobs, WorkerSpec};
use crate::registry;
use crate::stats::{self, CountingSink, Live};

/// Exit code after Ctrl-C.
pub const EXIT_INTERRUPTED: u8 = 130;

/// How long a closed stdout may wait for files in flight before the process just exits.
const BROKEN_PIPE_GRACE: Duration = Duration::from_secs(2);

/// Runs the command.
pub fn run(args: &ScanArgs) -> Result<ExitCode, String> {
    // The handler goes in before anything slow (engine construction loads language models):
    // a Ctrl-C during startup must cancel cleanly instead of killing the process.
    let cancel = Arc::new(AtomicBool::new(false));
    let interrupted = install_ctrlc(&cancel);
    let limits = args.limits.limits();
    // Oversized files are refused by the source, before they are read.
    let read = ReadOptions::new().max_file_bytes(Some(limits.max_bytes));

    let dump = match &args.dump_documents {
        Some(p) => Some(
            DocumentDump::create(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?,
        ),
        None => None,
    };
    let mut mode = mode(args, limits, read)?;

    let sink = sink(args)?;
    let broken_pipe = Arc::new(AtomicBool::new(false));
    let live = Arc::new(Live::default());
    let mut sink = CountingSink::new(
        sink,
        Arc::clone(&live),
        Arc::clone(&broken_pipe),
        Arc::clone(&cancel),
    );

    let mut opts = RunOptions::default();
    if let Some(t) = args.threads {
        opts.threads = t.max(1);
        opts.max_in_flight = opts.threads * 2;
    }
    if let Some(n) = args.max_in_flight {
        opts.max_in_flight = n.max(1);
    }
    if let Some(b) = args.max_in_flight_bytes {
        opts.max_in_flight_bytes = b.max(1);
    }
    opts.ordered = args.ordered;
    opts.dedup_by_content = args.dedup;
    opts.cancel = Some(Arc::clone(&cancel));
    opts.on_artifacts = dump.as_ref().map(DocumentDump::hook);

    let done = Arc::new(AtomicBool::new(false));
    let progress = (!args.quiet && io::stderr().is_terminal()).then(|| {
        stats::spawn_progress(
            Arc::clone(&live),
            Arc::clone(&done),
            Duration::from_millis(500),
        )
    });
    spawn_pipe_watchdog(Arc::clone(&broken_pipe), Arc::clone(&done), dump.clone());

    let (run_stats, isolated) = match &mut mode {
        Mode::InProcess(source, engine) => (engine.run(&mut **source, &mut sink, &opts), None),
        Mode::Isolated(iso) => match iso.1.run_jobs(&mut iso.0, &mut sink, &opts) {
            Ok(s) => (s.run.clone(), Some(s)),
            Err(e) => {
                done.store(true, Ordering::Relaxed);
                return Err(e.to_string());
            }
        },
    };

    done.store(true, Ordering::Relaxed);
    if let Some(p) = progress {
        let _ = p.join();
    }
    let dump_err = dump
        .as_ref()
        .and_then(|d| d.finish().err())
        .map(|e| format!("document dump: {e}"));
    if broken_pipe.load(Ordering::Relaxed) {
        // `quail scan | head`: the reader is gone, which is not an error.
        return Ok(ExitCode::SUCCESS);
    }

    let mut err = io::stderr().lock();
    if args.json_stats {
        let _ = writeln!(
            err,
            "{}",
            stats::json(&run_stats, &sink.tally, opts.threads, isolated.as_ref())
        );
    } else if !args.quiet {
        let _ = write!(
            err,
            "{}",
            stats::summary(&run_stats, &sink.tally, opts.threads, isolated.as_ref())
        );
    }
    if let Some(e) = &dump_err {
        let _ = writeln!(err, "quail: error: {e}");
    }
    if run_stats.cancelled && interrupted.load(Ordering::Relaxed) > 0 {
        return Ok(ExitCode::from(EXIT_INTERRUPTED));
    }
    if run_stats.sink_errors > 0
        || run_stats.writer_panicked
        || run_stats.hook_panics > 0
        || dump_err.is_some()
    {
        if args.quiet
            && !args.json_stats
            && let Some(e) = &run_stats.first_sink_error
        {
            let _ = writeln!(err, "quail: error: {e}");
        }
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// Under `--profile deep`, says once why pages needing OCR will not be read.
pub(crate) fn warn_without_ocr(profile: quail_core::Profile) {
    if profile == quail_core::Profile::Deep
        && let Some(why) = registry::ocr_unavailable()
    {
        eprintln!("quail: warning: no OCR: {why}");
    }
}

/// How the files are processed.
enum Mode {
    /// Engine threads in this process.
    InProcess(Box<dyn Source>, Box<Engine>),
    /// Worker child processes reading paths.
    Isolated(Box<(ScanJobs, IsolatedRunner)>),
}

/// In process: one engine shared by the threads. Isolated: each worker builds its own and
/// the supervisor only walks paths (no models are loaded here).
fn mode(args: &ScanArgs, limits: Limits, read: ReadOptions) -> Result<Mode, String> {
    let spec = WorkerSpec {
        profile: args.profile.into(),
        limits,
    };
    warn_without_ocr(spec.profile);
    if args.isolate {
        let cap = spec.limits.max_bytes;
        let paths = path_source(args, read)?;
        let opts = isolate::options(&spec, args.worker_mem, !args.no_retry);
        let jobs = isolate::jobs(paths, cap);
        return Ok(Mode::Isolated(Box::new((jobs, IsolatedRunner::new(opts)))));
    }
    let engine = registry::builder(spec.profile, spec.limits)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Mode::InProcess(source(args, read)?, Box::new(engine)))
}

/// The paths of the scan without reading the files, for workers that read them.
fn path_source(args: &ScanArgs, read: ReadOptions) -> Result<PathSource, String> {
    if let Some(m) = &args.manifest {
        if m == Path::new("-") {
            let r = BufReader::new(io::stdin());
            let src = ListSource::from_reader(r, None).read_options(read);
            return Ok(PathSource::Manifest(Box::new(src)));
        }
        let src = ListSource::open(m).map_err(|e| format!("manifest: {e}"))?;
        return Ok(PathSource::Manifest(Box::new(src.read_options(read))));
    }
    Ok(PathSource::Walk(Box::new(fs_source(args, read)?)))
}

fn source(args: &ScanArgs, read: ReadOptions) -> Result<Box<dyn Source>, String> {
    if let Some(m) = &args.manifest {
        if m == Path::new("-") {
            let r = BufReader::new(io::stdin());
            return Ok(Box::new(
                ListSource::from_reader(r, None).read_options(read),
            ));
        }
        let src = ListSource::open(m).map_err(|e| format!("manifest: {e}"))?;
        return Ok(Box::new(src.read_options(read)));
    }
    Ok(Box::new(fs_source(args, read)?))
}

fn fs_source(args: &ScanArgs, read: ReadOptions) -> Result<FsSource, String> {
    let mut b = FsSource::builder()
        .roots(args.paths.iter().cloned())
        .skip_hidden(args.skip_hidden)
        .sorted(args.sorted)
        .follow_symlinks(args.follow_symlinks)
        .read_options(read);
    for g in &args.include {
        b = b.include(g);
    }
    for g in &args.exclude {
        b = b.exclude(g);
    }
    for e in args.ext.iter().filter(|e| !e.trim().is_empty()) {
        b = b.extension(e.trim());
    }
    b.build().map_err(|e| e.to_string())
}

fn sink(args: &ScanArgs) -> Result<JsonlSink, String> {
    let path = args.out.as_deref().filter(|p| *p != Path::new("-"));
    if args.zstd {
        return zstd_sink(path);
    }
    match path {
        Some(p) => JsonlSink::create(p).map_err(|e| e.to_string()),
        None => Ok(JsonlSink::stdout()),
    }
}

#[cfg(feature = "zstd")]
fn zstd_sink(path: Option<&Path>) -> Result<JsonlSink, String> {
    const LEVEL: i32 = 3;
    match path {
        Some(p) => JsonlSink::create_zstd(p, LEVEL),
        None => JsonlSink::zstd(io::stdout(), LEVEL),
    }
    .map_err(|e| e.to_string())
}

#[cfg(not(feature = "zstd"))]
fn zstd_sink(_: Option<&Path>) -> Result<JsonlSink, String> {
    Err("--zstd needs a quail built with the `zstd` feature".to_owned())
}

/// First Ctrl-C: stop pulling files, finish the ones in flight, print the stats. Second:
/// exit at once. Returns the number of interrupts seen.
fn install_ctrlc(cancel: &Arc<AtomicBool>) -> Arc<AtomicU8> {
    let hits = Arc::new(AtomicU8::new(0));
    let (c, h) = (Arc::clone(cancel), Arc::clone(&hits));
    let installed = ctrlc::set_handler(move || {
        if h.fetch_add(1, Ordering::SeqCst) == 0 {
            c.store(true, Ordering::SeqCst);
            let _ = writeln!(
                io::stderr(),
                "\nquail: interrupted, finishing files in flight (Ctrl-C again to abort)"
            );
        } else {
            std::process::exit(i32::from(EXIT_INTERRUPTED));
        }
    });
    if let Err(e) = installed {
        let _ = writeln!(io::stderr(), "quail: warning: no Ctrl-C handler: {e}");
    }
    hits
}

/// Once stdout is closed, files still in flight get [`BROKEN_PIPE_GRACE`] to finish (so the
/// document dump is complete), then the process exits quietly.
fn spawn_pipe_watchdog(broken: Arc<AtomicBool>, done: Arc<AtomicBool>, dump: Option<DocumentDump>) {
    std::thread::spawn(move || {
        while !done.load(Ordering::Relaxed) {
            if broken.load(Ordering::Relaxed) {
                std::thread::sleep(BROKEN_PIPE_GRACE);
                if !done.load(Ordering::Relaxed) {
                    if let Some(d) = &dump {
                        let _ = d.finish();
                    }
                    std::process::exit(0);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

//! `quail-worker-demo FILE...`: processes files in isolated workers and prints one JSON
//! record per line.
//!
//! The engine only sniffs formats, plus an analyzer that aborts the process on any file
//! containing `QUAIL-DEMO-ABORT`, to show that such a file costs one `Failed` record and
//! nothing else:
//!
//! ```text
//! printf 'hello' > a.txt; printf 'QUAIL-DEMO-ABORT' > b.txt
//! quail-worker-demo a.txt b.txt
//! ```

use std::io::{self, Write};
use std::process::ExitCode;

use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, FileRecord, RecordSink, Result};
use quail_engine::{Engine, EngineBuilder, RunOptions};
use quail_worker::{IsolatedRunner, IsolationOptions, PathJobs, maybe_run_worker};

/// Aborts the process on a marker, standing in for a parser bug.
struct DemoTrap;

impl Analyzer for DemoTrap {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("worker.demo_trap", 1)
    }

    fn run(&self, ctx: &Ctx<'_>, _: &mut Emitter) -> Result<()> {
        let marker = b"QUAIL-DEMO-ABORT";
        if ctx
            .blob
            .as_bytes()
            .windows(marker.len())
            .any(|w| w == marker)
        {
            std::process::abort();
        }
        Ok(())
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .quiet_panics(true)
        .analyzer(Box::new(DemoTrap))
        .build()
        .expect("demo engine builds")
}

/// JSON lines on stdout.
struct Stdout(io::BufWriter<io::Stdout>);

impl RecordSink for Stdout {
    fn write(&mut self, record: &FileRecord) -> Result<()> {
        serde_json::to_writer(&mut self.0, record).map_err(io::Error::from)?;
        self.0.write_all(b"\n")?;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        Ok(self.0.flush()?)
    }
}

fn main() -> ExitCode {
    maybe_run_worker(engine);

    let paths: Vec<_> = std::env::args_os().skip(1).map(Into::into).collect();
    if paths.is_empty() {
        eprintln!("usage: quail-worker-demo FILE...");
        return ExitCode::from(2);
    }
    let mut jobs = PathJobs::new(paths.into_iter().map(Ok));
    let mut sink = Stdout(io::BufWriter::new(io::stdout()));
    let opts = RunOptions {
        threads: 2,
        ordered: true,
        ..RunOptions::default()
    };
    let mut runner = IsolatedRunner::new(IsolationOptions::default());
    match runner.run_jobs(&mut jobs, &mut sink, &opts) {
        Ok(stats) => {
            eprintln!("{stats}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("quail-worker-demo: {e}");
            ExitCode::FAILURE
        }
    }
}

//! The `quail` command-line tool, as a library so the wiring can be tested and reused.
//!
//! - [`registry`]: the one place where concrete extractors and analyzers are registered,
//!   behind one cargo feature per crate.
//! - [`scan`]: `quail scan`, many files to JSONL with progress, stats, Ctrl-C and broken
//!   pipe handling.
//! - [`inspect`]: `quail inspect`, one file pretty-printed.
//! - [`info`]: `quail keys` and `quail version`.
//! - [`dump`]: `--dump-documents`, fed by the engine's artifacts hook.

pub mod cli;
pub mod dump;
pub mod info;
pub mod inspect;
pub mod keydocs;
pub mod registry;
pub mod scan;
pub mod stats;

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};

/// Parses the arguments and runs the command.
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Scan(a) => scan::run(a),
        Command::Inspect(a) => inspect::run(a).map(|(out, code)| print(&out).unwrap_or(code)),
        Command::Keys(a) => Ok(print(&info::keys(a)).unwrap_or(ExitCode::SUCCESS)),
        Command::Version(a) => info::version(a).map(|out| print(&out).unwrap_or(ExitCode::SUCCESS)),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io::stderr(), "quail: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Writes to stdout. `Some(code)` when that failed: a closed pipe (`| head`) is a quiet
/// success, anything else an error.
fn print(s: &str) -> Option<ExitCode> {
    let mut out = io::stdout().lock();
    match out.write_all(s.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => None,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Some(ExitCode::SUCCESS),
        Err(e) => {
            let _ = writeln!(io::stderr(), "quail: error: stdout: {e}");
            Some(ExitCode::FAILURE)
        }
    }
}

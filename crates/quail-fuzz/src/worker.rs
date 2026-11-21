//! The child side: a long-lived worker process that runs cases one by one under resource
//! limits. Protocol on stdin/stdout, one line each way per case:
//!
//! ```text
//! parent -> worker   <path of the case file>
//! worker -> parent   R <status> <deadline step> <panics> <peak KiB> <ms> <slowest step> <features>
//! ```
//!
//! The worker prints `READY` once the engine is built. When it dies instead of answering, the
//! parent classifies the exit (signal, abort message on stderr) and starts a new one.

use std::io::{BufRead, Write};
use std::time::Duration;

use crate::target::Target;

/// Limits and options of a worker.
#[derive(Clone, Debug)]
pub struct WorkerOpts {
    /// Address-space limit, MiB (0: none).
    pub mem_mb: u64,
    /// CPU seconds per case (0: none).
    pub cpu_secs: u64,
    /// The engine's cooperative per-file deadline.
    pub deadline: Duration,
    /// Render pages when the extractor offers a raster.
    pub render: bool,
}

impl WorkerOpts {
    /// Command-line arguments that recreate these options in a child.
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![
            "--mem-mb".to_owned(),
            self.mem_mb.to_string(),
            "--cpu-secs".to_owned(),
            self.cpu_secs.to_string(),
            "--deadline".to_owned(),
            self.deadline.as_secs().to_string(),
        ];
        if !self.render {
            a.push("--no-render".to_owned());
        }
        a
    }
}

/// CPU seconds this process has used (Linux), rounded up.
#[cfg(target_os = "linux")]
fn cpu_used() -> u64 {
    if let Ok(s) = std::fs::read_to_string("/proc/self/stat") {
        // fields after the command name in parentheses; utime and stime are 14 and 15
        if let Some(rest) = s.rsplit_once(')').map(|x| x.1) {
            let f: Vec<&str> = rest.split_whitespace().collect();
            let ticks: u64 = f.get(11).and_then(|x| x.parse().ok()).unwrap_or(0)
                + f.get(12).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
            return ticks.div_ceil(100);
        }
    }
    0
}

/// Applies the address-space limit (Unix).
fn limit_memory(mb: u64) {
    #[cfg(unix)]
    if mb > 0 {
        let b = mb << 20;
        if let Err(e) = rlimit::setrlimit(rlimit::Resource::AS, b, b) {
            eprintln!("quail-fuzz worker: RLIMIT_AS not set: {e}");
        }
    }
    #[cfg(not(unix))]
    let _ = mb;
}

/// Lets the next case use `secs` more CPU seconds: the soft `RLIMIT_CPU` moves to the
/// current usage plus `secs` (SIGXCPU when exceeded). Linux only (needs the usage).
fn limit_cpu(secs: u64) {
    #[cfg(all(unix, target_os = "linux"))]
    if secs > 0 {
        let soft = cpu_used() + secs;
        let hard = rlimit::getrlimit(rlimit::Resource::CPU).map_or(rlimit::INFINITY, |x| x.1);
        let _ = rlimit::setrlimit(rlimit::Resource::CPU, soft.min(hard), hard);
    }
    #[cfg(not(all(unix, target_os = "linux")))]
    let _ = secs;
}

/// The worker loop; returns when stdin closes.
pub fn main(opts: &WorkerOpts) {
    let target = Target::new(opts.deadline, opts.render);
    limit_memory(opts.mem_mb);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "READY");
    let _ = out.flush();
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(path) = line else { break };
        let data = match std::fs::read(path.trim()) {
            Ok(d) => d,
            Err(e) => {
                let _ = writeln!(out, "E\t{e}");
                let _ = out.flush();
                continue;
            }
        };
        limit_cpu(opts.cpu_secs);
        let report = target.run(data);
        let _ = writeln!(out, "{}", report.to_line());
        let _ = out.flush();
    }
}

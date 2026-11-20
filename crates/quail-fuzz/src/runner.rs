//! The parent side: runs a case in a worker process (Unix) or in-process (elsewhere) and
//! classifies what happened.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use crate::target::{Report, Target};
use crate::worker::WorkerOpts;

/// How a case ended.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// Processed; nothing remarkable.
    Ok,
    /// Processed, but a step panicked (the engine caught it).
    CaughtPanic,
    /// Processed, but a step hit the cooperative per-file deadline.
    Deadline,
    /// Processed, but resident memory peaked above the threshold.
    MemHigh,
    /// Processed, but slower than the threshold.
    Slow,
    /// Aborted on an allocation failure (`memory allocation of N bytes failed`).
    Oom,
    /// Aborted on a stack overflow.
    StackOverflow,
    /// No answer within the wall-time limit; killed.
    Timeout,
    /// Killed by `RLIMIT_CPU` (SIGXCPU).
    CpuLimit,
    /// Killed by SIGKILL that the harness did not send (kernel OOM killer).
    Killed,
    /// Any other abort (a panic that could not unwind, a failed runtime assertion).
    Abort,
    /// Another signal (SIGSEGV, SIGBUS, SIGILL ...).
    Signal(i32),
    /// The worker exited with a code.
    Exit(i32),
}

impl Class {
    /// True for outcomes that kill the process: the ones that matter most.
    pub fn is_fatal(&self) -> bool {
        !matches!(
            self,
            Self::Ok | Self::CaughtPanic | Self::Deadline | Self::MemHigh | Self::Slow
        )
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => f.write_str("ok"),
            Self::CaughtPanic => f.write_str("caught-panic"),
            Self::Deadline => f.write_str("deadline"),
            Self::MemHigh => f.write_str("mem-high"),
            Self::Slow => f.write_str("slow"),
            Self::Oom => f.write_str("oom-abort"),
            Self::StackOverflow => f.write_str("stack-overflow"),
            Self::Timeout => f.write_str("timeout"),
            Self::CpuLimit => f.write_str("cpu-limit"),
            Self::Killed => f.write_str("killed"),
            Self::Abort => f.write_str("abort"),
            Self::Signal(n) => write!(f, "signal-{n}"),
            Self::Exit(n) => write!(f, "exit-{n}"),
        }
    }
}

/// Classification of one case.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// What happened.
    pub class: Class,
    /// Where: the panic location, the step running when the process died, the slowest step.
    pub detail: String,
    /// The worker's report, when it answered.
    pub report: Option<Report>,
    /// Last lines the worker wrote to stderr (crashes only).
    pub stderr: String,
}

impl Outcome {
    /// Identity of a finding: class and where.
    pub fn signature(&self) -> String {
        format!("{}@{}", self.class, self.detail)
    }
}

/// Thresholds that turn a normal run into a soft finding.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    /// Peak RSS above this is `mem-high`, KiB.
    pub mem_high_kb: u64,
    /// Wall time above this is `slow`, ms.
    pub slow_ms: u64,
}

/// Classifies a report.
pub fn classify_report(r: Report, t: Thresholds) -> Outcome {
    let (class, detail) = if let Some(p) = r.panics.first() {
        (Class::CaughtPanic, p.clone())
    } else if r.status == "harness-panic" {
        (Class::Abort, "harness".to_owned())
    } else if let Some(d) = &r.deadline {
        (Class::Deadline, d.clone())
    } else if r.peak_kb > t.mem_high_kb {
        (Class::MemHigh, r.slowest.clone())
    } else if r.ms > t.slow_ms {
        (Class::Slow, r.slowest.clone())
    } else {
        (Class::Ok, String::new())
    };
    Outcome {
        class,
        detail,
        report: Some(r),
        stderr: String::new(),
    }
}

/// The step named by the last `QF-STEP` line, and the last few other lines.
fn parse_stderr(s: &str) -> (String, String) {
    let step = s
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("QF-STEP "))
        .unwrap_or("-")
        .trim()
        .to_owned();
    let tail: Vec<&str> = s.lines().filter(|l| !l.starts_with("QF-STEP")).collect();
    let tail = tail[tail.len().saturating_sub(12)..].join("\n");
    (step, tail)
}

/// Classifies a worker that died.
fn classify_death(status: Option<ExitStatus>, stderr: &str, killed_by_us: bool) -> Outcome {
    let (step, tail) = parse_stderr(stderr);
    let class = if killed_by_us {
        Class::Timeout
    } else if stderr.contains("has overflowed its stack") {
        Class::StackOverflow
    } else if stderr.contains("memory allocation of") {
        Class::Oom
    } else {
        signal_class(status)
    };
    let detail = if class == Class::Abort {
        // a panic that turned into an abort names its location
        stderr
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("QF-PANIC "))
            .and_then(|l| l.split_whitespace().next())
            .map_or(step.clone(), |loc| format!("{step}:{loc}"))
    } else {
        step
    };
    Outcome {
        class,
        detail,
        report: None,
        stderr: tail,
    }
}

#[cfg(unix)]
fn signal_class(status: Option<ExitStatus>) -> Class {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Some(s) => match (s.signal(), s.code()) {
            (Some(6), _) => Class::Abort,
            (Some(9), _) => Class::Killed,
            (Some(24), _) => Class::CpuLimit,
            (Some(n), _) => Class::Signal(n),
            (None, Some(c)) => Class::Exit(c),
            (None, None) => Class::Exit(-1),
        },
        None => Class::Exit(-1),
    }
}

#[cfg(not(unix))]
fn signal_class(status: Option<ExitStatus>) -> Class {
    Class::Exit(status.and_then(|s| s.code()).unwrap_or(-1))
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

/// Runs cases, in a worker process or in-process.
pub struct Runner {
    mode: Mode,
    thresholds: Thresholds,
}

enum Mode {
    Child {
        exe: PathBuf,
        opts: WorkerOpts,
        stderr: PathBuf,
        timeout: Duration,
        proc: Option<Proc>,
        /// Cases since the worker started; it is restarted every `recycle` cases.
        done: u32,
        recycle: u32,
    },
    InProcess(Box<Target>),
}

impl Runner {
    /// A runner whose worker writes its stderr to `<dir>/stderr`. On non-Unix systems the
    /// cases run in this process, without limits.
    pub fn new(dir: &Path, opts: WorkerOpts, timeout: Duration, thresholds: Thresholds) -> Self {
        let mode = if cfg!(unix) {
            Mode::Child {
                exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("quail-fuzz")),
                opts,
                stderr: dir.join("stderr"),
                timeout,
                proc: None,
                done: 0,
                recycle: 2000,
            }
        } else {
            eprintln!(
                "quail-fuzz: warning: no child-process isolation on this OS; cases run in-process \
                 without memory/CPU limits, and an abort ends the fuzzer"
            );
            Mode::InProcess(Box::new(Target::new(opts.deadline, opts.render)))
        };
        Self { mode, thresholds }
    }

    /// Runs the case stored at `path`.
    pub fn run(&mut self, path: &Path) -> Outcome {
        let t = self.thresholds;
        match &mut self.mode {
            Mode::InProcess(target) => match std::fs::read(path) {
                Ok(d) => classify_report(target.run(d), t),
                Err(e) => Outcome {
                    class: Class::Exit(-1),
                    detail: format!("read: {e}"),
                    report: None,
                    stderr: String::new(),
                },
            },
            Mode::Child {
                exe,
                opts,
                stderr,
                timeout,
                proc,
                done,
                recycle,
            } => {
                if *done >= *recycle
                    && let Some(mut p) = proc.take()
                {
                    drop(p.stdin);
                    let _ = p.child.wait();
                }
                if proc.is_none() {
                    match spawn(exe, opts, stderr) {
                        Ok(p) => {
                            *proc = Some(p);
                            *done = 0;
                        }
                        Err(e) => {
                            return Outcome {
                                class: Class::Exit(-1),
                                detail: format!("spawn: {e}"),
                                report: None,
                                stderr: String::new(),
                            };
                        }
                    }
                }
                let Some(p) = proc.as_mut() else {
                    unreachable!("worker spawned above")
                };
                *done += 1;
                let _ = OpenOptions::new()
                    .write(true)
                    .open(&*stderr)
                    .and_then(|f| f.set_len(0));
                let sent = writeln!(p.stdin, "{}", path.display()).and_then(|()| p.stdin.flush());
                let answer = if sent.is_ok() {
                    p.rx.recv_timeout(*timeout)
                } else {
                    Err(RecvTimeoutError::Disconnected)
                };
                match answer {
                    Ok(line) => {
                        if let Some(r) = Report::parse(&line) {
                            classify_report(r, t)
                        } else {
                            Outcome {
                                class: Class::Exit(-2),
                                detail: line.chars().take(80).collect(),
                                report: None,
                                stderr: String::new(),
                            }
                        }
                    }
                    Err(e) => {
                        let mut p = proc.take().unwrap_or_else(|| unreachable!());
                        let killed = e == RecvTimeoutError::Timeout;
                        if killed {
                            let _ = p.child.kill();
                        }
                        let status = p.child.wait().ok();
                        classify_death(status, &read_tail(stderr), killed)
                    }
                }
            }
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Mode::Child { proc, .. } = &mut self.mode
            && let Some(mut p) = proc.take()
        {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }
}

fn read_tail(path: &Path) -> String {
    let Ok(mut f) = File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(16 << 10)));
    let mut s = Vec::new();
    let _ = f.read_to_end(&mut s);
    String::from_utf8_lossy(&s).into_owned()
}

fn spawn(exe: &Path, opts: &WorkerOpts, stderr: &Path) -> std::io::Result<Proc> {
    let err = OpenOptions::new().create(true).append(true).open(stderr)?;
    let mut child = Command::new(exe)
        .arg("worker")
        .args(opts.args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(err))
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    match rx.recv_timeout(Duration::from_mins(3)) {
        Ok(l) if l == "READY" => Ok(Proc { child, stdin, rx }),
        other => {
            let _ = child.kill();
            let _ = child.wait();
            Err(std::io::Error::other(format!(
                "worker did not start: {other:?}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deaths_are_classified_from_stderr() {
        let o = classify_death(
            None,
            "QF-STEP docx.extract@1\nthread 'x' has overflowed its stack\n",
            false,
        );
        assert_eq!(o.class, Class::StackOverflow);
        assert_eq!(o.detail, "docx.extract@1");
        let o = classify_death(
            None,
            "QF-STEP a@1\nQF-STEP b@1\nmemory allocation of 8 bytes failed\n",
            false,
        );
        assert_eq!((o.class, o.detail.as_str()), (Class::Oom, "b@1"));
        let o = classify_death(None, "QF-STEP a@1\n", true);
        assert_eq!(o.class, Class::Timeout);
        assert!(Class::Timeout.is_fatal() && !Class::CaughtPanic.is_fatal());
    }
}

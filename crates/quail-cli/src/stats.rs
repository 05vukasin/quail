//! Live counters, the problem tally and the final summary of `quail scan`.

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use quail_core::{Error, FileRecord, RecordSink, Result, Status};
use quail_engine::RunStats;
use quail_worker::IsolatedStats;

/// Counters shared with the progress thread.
#[derive(Debug, Default)]
pub struct Live {
    /// Records written.
    pub files: AtomicU64,
    /// Bytes of the files written.
    pub bytes: AtomicU64,
    /// `ok` records.
    pub ok: AtomicU64,
    /// `partial` records.
    pub partial: AtomicU64,
    /// `failed` records.
    pub failed: AtomicU64,
}

/// Distinct problem lines kept; past this, new kinds count as "other".
const MAX_KINDS: usize = 4096;

/// Counts problems by kind: the record's locator becomes `<file>`, digit runs `#`, so
/// "page 7 ..." and "page 9 ..." are one kind.
#[derive(Debug, Default)]
pub struct Tally {
    kinds: HashMap<String, u64>,
    other: u64,
}

impl Tally {
    fn add(&mut self, locator: &str, msg: &str) {
        let k = kind(locator, msg);
        if let Some(n) = self.kinds.get_mut(&k) {
            *n += 1;
        } else if self.kinds.len() < MAX_KINDS {
            self.kinds.insert(k, 1);
        } else {
            self.other += 1;
        }
    }

    /// The `n` most frequent kinds, most frequent first (ties by text).
    pub fn top(&self, n: usize) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.kinds.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        if self.other > 0 && v.len() < n {
            v.push(("(other kinds)".to_owned(), self.other));
        }
        v
    }
}

/// Normalizes one problem line into its kind.
pub fn kind(locator: &str, msg: &str) -> String {
    let msg = if locator.is_empty() {
        msg.to_owned()
    } else {
        msg.replace(locator, "<file>")
    };
    let mut out = String::with_capacity(msg.len().min(160));
    let mut in_digits = false;
    // Digits right after `@` are a step version (`pdf.extract@1`): kept.
    let mut version = false;
    for c in msg.chars() {
        if c.is_ascii_digit() && !version {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
            continue;
        }
        version = c == '@' || (version && c.is_ascii_digit());
        in_digits = false;
        if out.len() >= 140 {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

/// Wraps the record sink: counts what is written, tallies problems and turns a closed
/// stdout (`quail scan | head`) into a quiet stop.
pub struct CountingSink<S> {
    inner: S,
    live: Arc<Live>,
    /// Problem kinds seen.
    pub tally: Tally,
    broken_pipe: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl<S: RecordSink> CountingSink<S> {
    /// Wraps `inner`. A broken pipe sets `broken_pipe` and `cancel`.
    pub fn new(
        inner: S,
        live: Arc<Live>,
        broken_pipe: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner,
            live,
            tally: Tally::default(),
            broken_pipe,
            cancel,
        }
    }

    fn check(&self, r: Result<()>) -> Result<()> {
        if let Err(Error::Io(e)) = &r
            && e.kind() == io::ErrorKind::BrokenPipe
        {
            self.broken_pipe.store(true, Ordering::Relaxed);
            self.cancel.store(true, Ordering::Relaxed);
        }
        r
    }
}

impl<S: RecordSink> RecordSink for CountingSink<S> {
    fn write(&mut self, record: &FileRecord) -> Result<()> {
        if self.broken_pipe.load(Ordering::Relaxed) {
            return Err(Error::Io(io::ErrorKind::BrokenPipe.into()));
        }
        let r = self.inner.write(record);
        self.check(r)?;
        let l = &self.live;
        l.files.fetch_add(1, Ordering::Relaxed);
        l.bytes.fetch_add(record.size, Ordering::Relaxed);
        match &record.status {
            Status::Ok => l.ok.fetch_add(1, Ordering::Relaxed),
            Status::Partial { problems } => {
                for p in problems {
                    self.tally.add(&record.locator, p);
                }
                l.partial.fetch_add(1, Ordering::Relaxed)
            }
            Status::Failed { error } => {
                self.tally.add(&record.locator, error);
                l.failed.fetch_add(1, Ordering::Relaxed)
            }
        };
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if self.broken_pipe.load(Ordering::Relaxed) {
            return Ok(());
        }
        let r = self.inner.finish();
        let r = self.check(r);
        if self.broken_pipe.load(Ordering::Relaxed) {
            return Ok(());
        }
        r
    }
}

#[allow(clippy::cast_precision_loss)] // display only
fn f(x: u64) -> f64 {
    x as f64
}

/// One progress line, rewritten in place (`\r`), for a terminal.
pub fn progress_line(live: &Live, started: Instant) -> String {
    let secs = started.elapsed().as_secs_f64().max(1e-9);
    let files = live.files.load(Ordering::Relaxed);
    let bytes = live.bytes.load(Ordering::Relaxed);
    format!(
        "\r{files} files  {:.1} files/s  {:.1} MB/s  ok {} partial {} failed {}  ",
        f(files) / secs,
        f(bytes) / 1e6 / secs,
        live.ok.load(Ordering::Relaxed),
        live.partial.load(Ordering::Relaxed),
        live.failed.load(Ordering::Relaxed),
    )
}

/// Prints progress to stderr every `every` until `stop` is set.
pub fn spawn_progress(
    live: Arc<Live>,
    stop: Arc<AtomicBool>,
    every: Duration,
) -> std::thread::JoinHandle<()> {
    let started = Instant::now();
    std::thread::spawn(move || {
        let tick = Duration::from_millis(50);
        let mut last = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(tick);
            if last.elapsed() >= every {
                last = Instant::now();
                let mut e = io::stderr().lock();
                let _ = e.write_all(progress_line(&live, started).as_bytes());
                let _ = e.flush();
            }
        }
        let _ = io::stderr().write_all(b"\r\x1b[K");
    })
}

/// How many problem kinds the summary lists.
pub const TOP: usize = 8;

/// The human summary. `isolated`: the worker stats of an `--isolate` run.
pub fn summary(
    stats: &RunStats,
    tally: &Tally,
    threads: usize,
    isolated: Option<&IsolatedStats>,
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let unit = if isolated.is_some() {
        "isolated workers"
    } else {
        "threads"
    };
    let _ = writeln!(s, "quail: {stats} [{threads} {unit}]");
    if let Some(i) = isolated
        && (i.crashes > 0 || i.spawn_failures > 0)
    {
        let kinds: Vec<String> = i
            .crash_kinds
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        let _ = writeln!(
            s,
            "  worker crashes: {} ({}); {} retried ({} ok), {} poisoned, {} respawns, {} spawn failures",
            i.crashes,
            kinds.join(", "),
            i.retries,
            i.retried_ok,
            i.poisoned,
            i.respawns,
            i.spawn_failures
        );
        if let Some(c) = &i.first_crash {
            let _ = writeln!(s, "  first crash: {c}");
        }
    }
    if stats.source_errors > 0 {
        let _ = writeln!(
            s,
            "  source errors: {} ({} written as failed records)",
            stats.source_errors, stats.source_error_records
        );
    }
    if let Some(e) = &stats.first_sink_error {
        let _ = writeln!(s, "  sink error: {e} ({} unwritten)", stats.unwritten);
    }
    if let Some(e) = &stats.first_hook_panic {
        let _ = writeln!(
            s,
            "  document dump panics: {} (first: {e})",
            stats.hook_panics
        );
    }
    if stats.stalls > 0 {
        let _ = writeln!(s, "  ordered-mode stalls: {}", stats.stalls);
    }
    if stats.duplicates > 0 {
        let _ = writeln!(
            s,
            "  duplicates reused (--dedup): {} ({} evicted from the cache)",
            stats.duplicates, stats.dedup_evicted
        );
    }
    let top = tally.top(TOP);
    if !top.is_empty() {
        let _ = writeln!(s, "  top problems:");
        for (k, n) in top {
            let _ = writeln!(s, "  {n:>8}  {k}");
        }
    }
    s
}

/// The machine summary (`--json-stats`), one JSON object. An `--isolate` run adds
/// `"isolation"`: crashes (total and by kind), retries, poisoned files, respawns.
pub fn json(
    stats: &RunStats,
    tally: &Tally,
    threads: usize,
    isolated: Option<&IsolatedStats>,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "files": stats.files,
        "bytes": stats.bytes,
        "ok": stats.ok,
        "partial": stats.partial,
        "failed": stats.failed,
        "source_errors": stats.source_errors,
        "source_error_records": stats.source_error_records,
        "source_panics": stats.source_panics,
        "sink_errors": stats.sink_errors,
        "first_sink_error": stats.first_sink_error,
        "unwritten": stats.unwritten,
        "cancelled": stats.cancelled,
        "stalls": stats.stalls,
        "writer_panicked": stats.writer_panicked,
        "hook_panics": stats.hook_panics,
        "duplicates": stats.duplicates,
        "dedup_evicted": stats.dedup_evicted,
        "first_hook_panic": stats.first_hook_panic,
        "elapsed_secs": stats.elapsed.as_secs_f64(),
        "files_per_sec": stats.files_per_sec(),
        "mb_per_sec": stats.mb_per_sec(),
        "threads": threads,
        "top_problems": tally.top(TOP).into_iter()
            .map(|(problem, count)| serde_json::json!({"problem": problem, "count": count}))
            .collect::<Vec<_>>(),
    });
    if let Some(i) = isolated {
        v["isolation"] = serde_json::json!({
            "crashes": i.crashes,
            "crash_kinds": i.crash_kinds,
            "first_crash": i.first_crash,
            "retries": i.retries,
            "retried_ok": i.retried_ok,
            "poisoned": i.poisoned,
            "respawns": i.respawns,
            "spawn_failures": i.spawn_failures,
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_fold_numbers_and_locators() {
        assert_eq!(
            kind("/a/b.pdf", "source: /a/b.pdf: io: denied (os error 13)"),
            "source: <file>: io: denied (os error #)"
        );
        assert_eq!(
            kind("", "docx.extract@12: page 3 of 40"),
            "docx.extract@12: page # of #"
        );
        let mut t = Tally::default();
        t.add("x", "page 7 broken");
        t.add("y", "page 12 broken");
        t.add("y", "timeout");
        assert_eq!(
            t.top(5),
            [("page # broken".to_owned(), 2), ("timeout".to_owned(), 1)]
        );
    }
}

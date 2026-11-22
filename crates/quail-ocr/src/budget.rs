//! How much OCR runs at once, with how many threads, and for how long.
//!
//! OCR is two orders of magnitude slower than every other step and holds about
//! [`ENGINE_BYTES`] per page being read, so it gets limits of its own, separate from the
//! number of workers that run files:
//!
//! - [`OcrPermits`]: at most `jobs` pages are read at the same time in the process, whatever
//!   the number of workers. A worker waits for a permit before it decodes or renders the page,
//!   so page images are bounded too. Permits go to the oldest file first, so a file that
//!   started OCR finishes before newer ones start.
//! - [`OcrResources`]: `jobs` from the memory ([`ocr_memory`]: 40 % of the available, at most
//!   20 % of the total) and `threads` (ONNX Runtime intra-op
//!   threads per engine) so that `jobs × threads` stays within the logical CPUs.
//! - [`OcrConfig::page_time`](crate::OcrConfig::page_time): a time allowance per page that
//!   needs OCR. A file's OCR stops with a problem (record `partial`) once
//!   `pages × page_time` of reading is spent; time spent waiting for a permit is not counted.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use quail_core::{Error, Result};

/// Memory one engine needs while it reads a page with up to 4 intra-op threads: ONNX Runtime
/// sessions and arenas plus the page images at 200 to 300 DPI, and a share of the files the
/// workers hold meanwhile (measured: 3 engines x 4 threads peaked at 3.3 GB on the `RealKIE` FCC set).
pub const ENGINE_BYTES: u64 = 1 << 30;

/// Share of the available memory that OCR engines may use ([`ocr_memory`]).
pub const MEMORY_SHARE: f64 = 0.4;

/// Share of the total memory that OCR engines may use ([`ocr_memory`]), so that a moment
/// with much free memory does not plan for more than the machine can keep.
pub const TOTAL_SHARE: f64 = 0.2;

/// Most pages read at once by default; more is possible with an explicit `jobs`.
pub const MAX_AUTO_JOBS: usize = 4;

/// Most intra-op threads per engine by default.
pub const MAX_AUTO_THREADS: usize = 4;

/// Pages read at once when the available memory is unknown (not Linux).
pub const FALLBACK_JOBS: usize = 2;

/// How many pages are read at once and with how many threads each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OcrResources {
    /// Pages read at the same time in the process (engines in use).
    pub jobs: usize,
    /// ONNX Runtime intra-op threads per engine.
    pub threads: usize,
}

impl OcrResources {
    /// The plan for `workers` file workers on `cpus` logical CPUs with `memory` bytes for
    /// OCR (`None`: unknown; [`ocr_memory`] on this machine).
    ///
    /// `jobs` = `min(workers, memory / ENGINE_BYTES, MAX_AUTO_JOBS)`, at least 1
    /// (`min(workers, FALLBACK_JOBS)` when the memory is unknown). `threads` = `cpus / jobs`,
    /// between 1 and [`MAX_AUTO_THREADS`], so `jobs × threads ≤ cpus`.
    pub fn plan(workers: usize, cpus: usize, memory: Option<u64>) -> Self {
        let by_memory = memory.map_or(FALLBACK_JOBS, |b| {
            usize::try_from(b / ENGINE_BYTES).unwrap_or(usize::MAX)
        });
        let jobs = workers.min(by_memory).clamp(1, MAX_AUTO_JOBS);
        Self {
            jobs,
            threads: Self::threads_for(jobs, cpus),
        }
    }

    /// The plan for this machine: [`plan`](Self::plan) with the logical CPUs and
    /// [`ocr_memory`].
    pub fn auto(workers: usize) -> Self {
        Self::plan(workers, cpus(), ocr_memory())
    }

    /// The plan with explicit values where given. An explicit `jobs` also sets the default
    /// `threads` (`cpus / jobs`).
    #[must_use]
    pub fn with_overrides(self, jobs: Option<usize>, threads: Option<usize>) -> Self {
        let jobs = jobs.map_or(self.jobs, |j| j.max(1));
        let threads = match threads {
            Some(t) => t.max(1),
            None if jobs == self.jobs => self.threads,
            None => Self::threads_for(jobs, cpus()),
        };
        Self { jobs, threads }
    }

    fn threads_for(jobs: usize, cpus: usize) -> usize {
        (cpus / jobs.max(1)).clamp(1, MAX_AUTO_THREADS)
    }
}

/// Logical CPUs (1 when unknown).
pub fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// Bytes OCR engines may use on this machine: the smaller of [`MEMORY_SHARE`] of
/// [`available_memory`] and [`TOTAL_SHARE`] of [`total_memory`]. `None` where that is
/// unknown (other systems than Linux).
pub fn ocr_memory() -> Option<u64> {
    Some(share(available_memory()?, MEMORY_SHARE).min(share(total_memory()?, TOTAL_SHARE)))
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn share(bytes: u64, s: f64) -> u64 {
    (bytes as f64 * s) as u64
}

/// Memory available to this process in bytes: `MemAvailable` of `/proc/meminfo`, lowered to
/// the room left under a cgroup v2 limit (`memory.max - memory.current`) in a container.
/// `None` where that is unknown (other systems).
pub fn available_memory() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut avail = meminfo_field(&meminfo, "MemAvailable")?;
    if let (Some(max), Some(cur)) = (cgroup("memory.max"), cgroup("memory.current")) {
        avail = avail.min(max.saturating_sub(cur));
    }
    Some(avail)
}

/// Memory of the machine in bytes: `MemTotal` of `/proc/meminfo`, lowered to a cgroup v2
/// limit (`memory.max`). `None` where that is unknown.
pub fn total_memory() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let total = meminfo_field(&meminfo, "MemTotal")?;
    Some(cgroup("memory.max").map_or(total, |m| total.min(m)))
}

/// A number in `/sys/fs/cgroup/<name>` (`None` for `max` or no cgroup v2).
fn cgroup(name: &str) -> Option<u64> {
    std::fs::read_to_string(format!("/sys/fs/cgroup/{name}"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// A `kB` field of `/proc/meminfo`, in bytes.
fn meminfo_field(s: &str, field: &str) -> Option<u64> {
    let line = s.lines().find(|l| l.split(':').next() == Some(field))?;
    let kib: u64 = line
        .split_once(':')?
        .1
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    kib.checked_mul(1024)
}

/// At most `total` pages read at once, shared by every worker of the process. Waiters are
/// served by ticket ([`ticket`](Self::ticket), taken once per file): the oldest file first.
pub struct OcrPermits {
    total: usize,
    state: Mutex<State>,
    freed: Condvar,
    next: AtomicU64,
}

struct State {
    free: usize,
    waiting: BTreeSet<u64>,
}

impl OcrPermits {
    /// `total` permits (at least 1).
    pub fn new(total: usize) -> Self {
        let total = total.max(1);
        Self {
            total,
            state: Mutex::new(State {
                free: total,
                waiting: BTreeSet::new(),
            }),
            freed: Condvar::new(),
            next: AtomicU64::new(0),
        }
    }

    /// Permits in all.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Permits held right now.
    pub fn in_use(&self) -> usize {
        self.total - self.lock().free
    }

    /// A new ticket: later tickets wait behind earlier ones.
    pub fn ticket(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Waits for a permit until `deadline` (`Err(Error::Timeout)` then). Among waiters, the
    /// smallest `ticket` goes first. Returns the permit and the time spent waiting.
    pub fn acquire(
        &self,
        ticket: u64,
        deadline: Option<Instant>,
    ) -> Result<(OcrPermit<'_>, Duration)> {
        let start = Instant::now();
        let mut st = self.lock();
        st.waiting.insert(ticket);
        loop {
            if st.free > 0 && st.waiting.first() == Some(&ticket) {
                st.free -= 1;
                st.waiting.remove(&ticket);
                drop(st);
                // The next waiter may be able to go too.
                self.freed.notify_all();
                return Ok((OcrPermit { permits: self }, start.elapsed()));
            }
            let now = Instant::now();
            let wait = match deadline {
                Some(d) if now >= d => {
                    st.waiting.remove(&ticket);
                    drop(st);
                    self.freed.notify_all();
                    return Err(Error::Timeout);
                }
                Some(d) => d - now,
                // Woken on every release; the timeout only guards against a missed wake-up.
                None => Duration::from_secs(1),
            };
            st = self
                .freed
                .wait_timeout(st, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl std::fmt::Debug for OcrPermits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OcrPermits")
            .field("total", &self.total)
            .field("in_use", &self.in_use())
            .finish_non_exhaustive()
    }
}

/// One page's right to read; given back when dropped.
#[must_use = "the permit is given back when dropped"]
pub struct OcrPermit<'a> {
    permits: &'a OcrPermits,
}

impl Drop for OcrPermit<'_> {
    fn drop(&mut self) {
        self.permits.lock().free += 1;
        self.permits.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    const GIB: u64 = 1 << 30;

    #[test]
    fn plan_follows_memory_workers_and_cpus() {
        // 2.8 GiB for OCR (e.g. 20 % of 14 GiB): two engines.
        let p = OcrResources::plan(15, 16, Some(28 * GIB / 10));
        assert_eq!(
            p,
            OcrResources {
                jobs: 2,
                threads: 4
            }
        );
        // Never more than the workers, never more than MAX_AUTO_JOBS.
        assert_eq!(OcrResources::plan(2, 16, Some(64 * GIB)).jobs, 2);
        assert_eq!(
            OcrResources::plan(64, 64, Some(256 * GIB)).jobs,
            MAX_AUTO_JOBS
        );
        // Little memory: still one.
        assert_eq!(OcrResources::plan(8, 8, Some(GIB / 2)).jobs, 1);
        assert_eq!(
            OcrResources::plan(0, 0, Some(0)),
            OcrResources {
                jobs: 1,
                threads: 1
            }
        );
        // Unknown memory.
        assert_eq!(OcrResources::plan(8, 8, None).jobs, FALLBACK_JOBS);
        // jobs x threads within the CPUs.
        for cpus in 1..40 {
            for w in 1..20 {
                let p = OcrResources::plan(w, cpus, Some(32 * GIB));
                assert!(p.jobs * p.threads <= cpus.max(p.jobs), "{w} {cpus} {p:?}");
            }
        }
    }

    #[test]
    fn overrides() {
        let p = OcrResources {
            jobs: 3,
            threads: 4,
        };
        assert_eq!(p.with_overrides(None, None), p);
        assert_eq!(p.with_overrides(None, Some(1)).threads, 1);
        assert_eq!(
            p.with_overrides(Some(0), Some(0)),
            OcrResources {
                jobs: 1,
                threads: 1
            }
        );
        assert_eq!(
            p.with_overrides(Some(5), Some(2)),
            OcrResources {
                jobs: 5,
                threads: 2
            }
        );
    }

    #[test]
    fn meminfo() {
        let s = "MemTotal:       14680064 kB\nMemFree:  100 kB\nMemAvailable:    7340032 kB\n";
        assert_eq!(meminfo_field(s, "MemAvailable"), Some(7 * GIB));
        assert_eq!(meminfo_field(s, "MemTotal"), Some(14 * GIB));
        assert_eq!(meminfo_field("MemTotal: 1 kB\n", "MemAvailable"), None);
        assert_eq!(meminfo_field("MemAvailable: lots\n", "MemAvailable"), None);
        assert_eq!(share(10 * GIB, 0.2), 2 * GIB);
        // On Linux both are known and OCR gets at most a fifth of the machine.
        if let (Some(m), Some(t)) = (ocr_memory(), total_memory()) {
            assert!(m <= share(t, TOTAL_SHARE));
        }
    }

    #[test]
    fn permits_bound_concurrency() {
        let permits = Arc::new(OcrPermits::new(2));
        let busy = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let (p, b, m) = (permits.clone(), busy.clone(), peak.clone());
                std::thread::spawn(move || {
                    let t = p.ticket();
                    for _ in 0..5 {
                        let Ok((_permit, _)) = p.acquire(t, None) else {
                            return;
                        };
                        let now = b.fetch_add(1, Ordering::SeqCst) + 1;
                        m.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(1));
                        b.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap_or(());
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(permits.in_use(), 0);
    }

    #[test]
    fn acquire_times_out_and_leaves_the_queue() {
        let permits = OcrPermits::new(1);
        let (held, waited) = permits.acquire(permits.ticket(), None).unwrap();
        assert!(waited < Duration::from_secs(1));
        let t = permits.ticket();
        let r = permits.acquire(t, Some(Instant::now() + Duration::from_millis(20)));
        assert!(matches!(r, Err(Error::Timeout)));
        drop(held);
        // The timed-out ticket no longer blocks later ones.
        let late = permits.ticket();
        assert!(permits.acquire(late, Some(Instant::now())).is_ok());
    }

    #[test]
    fn oldest_ticket_goes_first() {
        let permits = Arc::new(OcrPermits::new(1));
        let held = permits.acquire(permits.ticket(), None).unwrap();
        let old = permits.ticket();
        let young = permits.ticket();
        let order = Arc::new(Mutex::new(Vec::new()));
        // The young file starts waiting first; the old one must still go first.
        let spawn = |t: u64| {
            let (p, o) = (permits.clone(), order.clone());
            std::thread::spawn(move || {
                if let Ok((_permit, _)) = p.acquire(t, None) {
                    o.lock().unwrap().push(t);
                }
            })
        };
        let a = spawn(young);
        std::thread::sleep(Duration::from_millis(20));
        let b = spawn(old);
        std::thread::sleep(Duration::from_millis(20));
        drop(held);
        a.join().unwrap_or(());
        b.join().unwrap_or(());
        assert_eq!(*order.lock().unwrap(), vec![old, young]);
    }
}

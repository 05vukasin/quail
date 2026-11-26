//! One engine per worker thread: engines are checked out for a page and returned after.
//!
//! # Idle engines
//!
//! An engine holds its models and the ONNX Runtime arenas it has grown (about 0.4 to 0.55 GiB
//! for PP-OCR), and those arenas never shrink while the engine lives. A pool therefore drops
//! an engine that has been idle for its [`idle timeout`](EnginePool::with_idle_timeout)
//! ([`EnginePool::DEFAULT_IDLE_TIMEOUT`] by default), and [`EnginePool::trim`] drops every
//! idle engine at once. The next page makes a new engine from the factory, so this is
//! transparent: an engine id stands for its output, and loading one again costs a model
//! load (a quarter of a second for PP-OCR) and a slower first page.
//!
//! The timeout runs on a short-lived thread (`quail-ocr-idle`) that exists only while idle
//! engines wait to expire, and holds no reference that keeps the pool alive.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use quail_core::Result;

use crate::engine::{EngineFactory, EngineId, OcrEngine};

/// Engines made by one factory, reused across files. A thread takes an idle engine (or makes
/// a new one) for each page, so there are never more engines than threads running OCR at
/// once, and no engine is used by two threads at the same time. At most `max_idle` idle
/// engines are kept, each for at most the idle timeout; extra or expired ones are dropped
/// (with their model memory).
pub struct EnginePool {
    inner: Arc<Inner>,
}

struct Inner {
    factory: Arc<dyn EngineFactory>,
    id: EngineId,
    max_idle: usize,
    state: Mutex<State>,
}

struct State {
    /// Idle engines and when each was returned, oldest first (engines are taken from the
    /// end, so the ones nobody needed expire).
    idle: Vec<(Box<dyn OcrEngine>, Instant)>,
    timeout: Option<Duration>,
    /// A `quail-ocr-idle` thread is waiting for the oldest idle engine to expire.
    reaper: bool,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EnginePool {
    /// Idle time after which an engine is dropped, unless the host sets another
    /// ([`with_idle_timeout`](Self::with_idle_timeout)): 5 minutes. Long enough that a
    /// stream of files, or a scan with gaps of a few minutes, keeps its engines; short enough
    /// that a long-running host gives the memory back soon after its last scanned page.
    pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

    /// Pool over `factory`, keeping up to `max_idle` idle engines for
    /// [`DEFAULT_IDLE_TIMEOUT`](Self::DEFAULT_IDLE_TIMEOUT) each.
    pub fn new(factory: Arc<dyn EngineFactory>, max_idle: usize) -> Self {
        let id = factory.id();
        Self {
            inner: Arc::new(Inner {
                factory,
                id,
                max_idle,
                state: Mutex::new(State {
                    idle: Vec::new(),
                    timeout: Some(Self::DEFAULT_IDLE_TIMEOUT),
                    reaper: false,
                }),
            }),
        }
    }

    /// Drops an engine once it has been idle for `timeout` (`None`: keeps idle engines as
    /// long as the pool lives; `Some(Duration::ZERO)`: never keeps one). Set it before the
    /// pool is used: an engine already waiting keeps the timeout it was returned under.
    #[must_use]
    pub fn with_idle_timeout(self, timeout: Option<Duration>) -> Self {
        self.inner.state().timeout = timeout;
        self
    }

    /// The idle timeout.
    pub fn idle_timeout(&self) -> Option<Duration> {
        self.inner.state().timeout
    }

    /// The id of the engines.
    pub fn id(&self) -> &EngineId {
        &self.inner.id
    }

    /// Idle engines right now.
    pub fn idle(&self) -> usize {
        self.inner.state().idle.len()
    }

    /// Drops every idle engine now and returns how many. Engines in use are not touched; the
    /// next page makes a new engine.
    pub fn trim(&self) -> usize {
        let dropped: Vec<_> = self.inner.state().idle.drain(..).collect();
        dropped.len()
    }

    /// Runs `f` with an engine of its own. An engine whose run panics is dropped, not
    /// returned.
    pub fn with<R>(&self, f: impl FnOnce(&mut dyn OcrEngine) -> R) -> Result<R> {
        let taken = self.inner.state().idle.pop().map(|(e, _)| e);
        let mut engine = match taken {
            Some(e) => e,
            None => self.inner.factory.create()?,
        };
        let r = f(engine.as_mut());
        // Dropped here (after the lock), unless the pool keeps it.
        let _dropped = self.give_back(engine);
        Ok(r)
    }

    /// Keeps `engine` as idle if there is room, else hands it back to be dropped.
    fn give_back(&self, engine: Box<dyn OcrEngine>) -> Option<Box<dyn OcrEngine>> {
        let mut s = self.inner.state();
        if s.idle.len() >= self.inner.max_idle || s.timeout.is_some_and(|t| t.is_zero()) {
            return Some(engine);
        }
        s.idle.push((engine, Instant::now()));
        if let Some(t) = s.timeout
            && !s.reaper
        {
            let pool = Arc::downgrade(&self.inner);
            s.reaper = std::thread::Builder::new()
                .name("quail-ocr-idle".into())
                .spawn(move || reap(&pool, t))
                .is_ok();
        }
        None
    }
}

/// Drops the engines of `pool` once they have been idle for `timeout`; returns when no idle
/// engine is left or the pool is gone. Holds the pool only while it looks.
fn reap(pool: &Weak<Inner>, timeout: Duration) {
    loop {
        let Some(inner) = pool.upgrade() else {
            return;
        };
        let mut s = inner.state();
        let now = Instant::now();
        let fresh = s
            .idle
            .iter()
            .position(|(_, at)| now.saturating_duration_since(*at) < timeout)
            .unwrap_or(s.idle.len());
        let expired: Vec<_> = s.idle.drain(..fresh).collect();
        let next = s.idle.first().map(|(_, at)| *at + timeout);
        if next.is_none() {
            s.reaper = false;
        }
        drop(s);
        drop(expired);
        drop(inner);
        match next {
            Some(at) => std::thread::sleep(at.saturating_duration_since(Instant::now())),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{OcrHints, OcrPage};
    use quail_core::Image;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts the engines alive.
    struct E(EngineId, Arc<AtomicUsize>);
    impl OcrEngine for E {
        fn id(&self) -> &EngineId {
            &self.0
        }
        fn recognize(&mut self, _: &Image, _: &OcrHints) -> Result<OcrPage> {
            Ok(OcrPage::default())
        }
    }
    impl Drop for E {
        fn drop(&mut self) {
            self.1.fetch_sub(1, Ordering::SeqCst);
        }
    }
    /// Counts the engines made and alive.
    struct F(AtomicUsize, Arc<AtomicUsize>);
    impl F {
        fn new() -> Arc<Self> {
            Arc::new(Self(AtomicUsize::new(0), Arc::new(AtomicUsize::new(0))))
        }
        fn made(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
        fn alive(&self) -> usize {
            self.1.load(Ordering::SeqCst)
        }
    }
    impl EngineFactory for F {
        fn id(&self) -> EngineId {
            EngineId::new("t", b"")
        }
        fn create(&self) -> Result<Box<dyn OcrEngine>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(E(self.id(), self.1.clone())))
        }
    }

    /// Waits up to 10 s for `cond`.
    fn eventually(cond: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn reuses_and_bounds_idle_engines() {
        let f = F::new();
        let pool = EnginePool::new(f.clone(), 1);
        assert_eq!(pool.idle_timeout(), Some(EnginePool::DEFAULT_IDLE_TIMEOUT));
        for _ in 0..5 {
            pool.with(|_| ()).unwrap_or(());
        }
        assert_eq!(f.made(), 1);
        // Nested use needs a second engine; only one is kept afterwards.
        pool.with(|_| pool.with(|_| ()).unwrap_or(())).unwrap_or(());
        assert_eq!(f.made(), 2);
        assert_eq!(pool.idle(), 1);
        assert_eq!(f.alive(), 1);
    }

    #[test]
    fn threads_get_their_own_engine() {
        let f = F::new();
        let pool = Arc::new(EnginePool::new(f.clone(), 8));
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let (p, b) = (pool.clone(), barrier.clone());
                std::thread::spawn(move || {
                    p.with(|_| {
                        b.wait();
                    })
                    .unwrap_or(());
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap_or(());
        }
        assert_eq!(f.made(), 4);
        assert_eq!(pool.idle(), 4);
    }

    #[test]
    fn idle_engines_are_dropped_after_the_timeout_and_made_again() {
        let f = F::new();
        let pool = EnginePool::new(f.clone(), 4).with_idle_timeout(Some(Duration::from_millis(50)));
        pool.with(|_| ()).unwrap_or(());
        assert_eq!((pool.idle(), f.alive()), (1, 1));
        assert!(eventually(|| f.alive() == 0), "idle engine not dropped");
        assert_eq!(pool.idle(), 0);
        // The next page gets a new engine, without the caller noticing.
        let img = Image {
            width: 1,
            height: 1,
            channels: 1,
            data: vec![0],
        };
        let hints = OcrHints {
            script: crate::engine::ScriptHint::Auto,
            dpi: 300.0,
        };
        let page = pool.with(|e| e.recognize(&img, &hints));
        assert!(matches!(page, Ok(Ok(_))));
        assert_eq!(f.made(), 2);
        assert!(eventually(|| f.alive() == 0));
        assert!(
            eventually(|| !pool.inner.state().reaper),
            "reaper left running"
        );
    }

    #[test]
    fn an_engine_in_use_is_kept_while_older_ones_expire() {
        let f = F::new();
        let pool =
            EnginePool::new(f.clone(), 4).with_idle_timeout(Some(Duration::from_millis(100)));
        // Two engines idle, then one taken again and returned later: only the other expires
        // on the first deadline.
        pool.with(|_| pool.with(|_| ()).unwrap_or(())).unwrap_or(());
        assert_eq!(pool.idle(), 2);
        std::thread::sleep(Duration::from_millis(60));
        pool.with(|_| ()).unwrap_or(());
        assert!(eventually(|| f.alive() == 1));
        assert!(eventually(|| f.alive() == 0));
        assert_eq!(f.made(), 2);
    }

    #[test]
    fn trim_and_no_timeout() {
        let f = F::new();
        let pool = EnginePool::new(f.clone(), 4).with_idle_timeout(None);
        pool.with(|_| pool.with(|_| ()).unwrap_or(())).unwrap_or(());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(f.alive(), 2);
        assert!(!pool.inner.state().reaper);
        assert_eq!(pool.trim(), 2);
        assert_eq!((pool.idle(), f.alive()), (0, 0));

        let zero = EnginePool::new(f.clone(), 4).with_idle_timeout(Some(Duration::ZERO));
        zero.with(|_| ()).unwrap_or(());
        assert_eq!((zero.idle(), f.alive()), (0, 0));
    }

    #[test]
    fn dropping_the_pool_drops_its_engines() {
        let f = F::new();
        let pool = EnginePool::new(f.clone(), 4);
        pool.with(|_| ()).unwrap_or(());
        assert_eq!(f.alive(), 1);
        drop(pool);
        assert_eq!(f.alive(), 0);
    }
}

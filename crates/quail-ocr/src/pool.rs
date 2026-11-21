//! One engine per worker thread: engines are checked out for a page and returned after.

use std::sync::{Arc, Mutex, PoisonError};

use quail_core::Result;

use crate::engine::{EngineFactory, EngineId, OcrEngine};

/// Engines made by one factory, reused across files. A thread takes an idle engine (or makes
/// a new one) for each page, so there are never more engines than threads running OCR at
/// once, and no engine is used by two threads at the same time. At most `max_idle` idle
/// engines are kept; extra ones are dropped (with their model memory) when returned.
pub struct EnginePool {
    factory: Arc<dyn EngineFactory>,
    id: EngineId,
    idle: Mutex<Vec<Box<dyn OcrEngine>>>,
    max_idle: usize,
}

impl EnginePool {
    /// Pool over `factory`, keeping up to `max_idle` idle engines.
    pub fn new(factory: Arc<dyn EngineFactory>, max_idle: usize) -> Self {
        let id = factory.id();
        Self {
            factory,
            id,
            idle: Mutex::new(Vec::new()),
            max_idle,
        }
    }

    /// The id of the engines.
    pub fn id(&self) -> &EngineId {
        &self.id
    }

    /// Idle engines right now.
    pub fn idle(&self) -> usize {
        self.idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Runs `f` with an engine of its own. An engine whose run panics is dropped, not
    /// returned.
    pub fn with<R>(&self, f: impl FnOnce(&mut dyn OcrEngine) -> R) -> Result<R> {
        let taken = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let mut engine = match taken {
            Some(e) => e,
            None => self.factory.create()?,
        };
        let r = f(engine.as_mut());
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if idle.len() < self.max_idle {
            idle.push(engine);
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{OcrHints, OcrPage};
    use quail_core::Image;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct E(EngineId);
    impl OcrEngine for E {
        fn id(&self) -> &EngineId {
            &self.0
        }
        fn recognize(&mut self, _: &Image, _: &OcrHints) -> Result<OcrPage> {
            Ok(OcrPage::default())
        }
    }
    struct F(AtomicUsize);
    impl EngineFactory for F {
        fn id(&self) -> EngineId {
            EngineId::new("t", b"")
        }
        fn create(&self) -> Result<Box<dyn OcrEngine>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(E(self.id())))
        }
    }

    #[test]
    fn reuses_and_bounds_idle_engines() {
        let f = Arc::new(F(AtomicUsize::new(0)));
        let pool = EnginePool::new(f.clone(), 1);
        for _ in 0..5 {
            pool.with(|_| ()).unwrap_or(());
        }
        assert_eq!(f.0.load(Ordering::SeqCst), 1);
        // Nested use needs a second engine; only one is kept afterwards.
        pool.with(|_| pool.with(|_| ()).unwrap_or(())).unwrap_or(());
        assert_eq!(f.0.load(Ordering::SeqCst), 2);
        assert_eq!(pool.idle(), 1);
    }

    #[test]
    fn threads_get_their_own_engine() {
        let f = Arc::new(F(AtomicUsize::new(0)));
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
        assert_eq!(f.0.load(Ordering::SeqCst), 4);
        assert_eq!(pool.idle(), 4);
    }
}

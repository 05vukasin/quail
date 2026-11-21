//! Results cache keyed by the page pixels and the engine configuration.
//!
//! The same scan shows up again and again in a business archive (a signed copy mailed twice,
//! the same annex attached to every contract), and OCR is by far the most expensive step, so
//! results are cached by `BLAKE3(engine fingerprint ‖ hints ‖ width ‖ height ‖ channels ‖
//! pixels)`. [`MemoryCache`] is a bounded in-process LRU; a disk cache implements the same
//! [`OcrCache`] trait.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use quail_core::Image;

use crate::engine::{EngineId, OcrHints, OcrPage, ScriptHint};

/// Key of one cached result.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CacheKey(pub [u8; 32]);

impl CacheKey {
    /// Key for recognizing `image` with the engine `engine` under `hints`.
    pub fn new(engine: &EngineId, hints: &OcrHints, image: &Image) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"quail-ocr/cache/1\0");
        h.update(&engine.fingerprint);
        h.update(&[match hints.script {
            ScriptHint::Auto => 0,
            ScriptHint::Latin => 1,
            ScriptHint::Cyrillic => 2,
        }]);
        h.update(&hints.dpi.to_bits().to_le_bytes());
        h.update(&image.width.to_le_bytes());
        h.update(&image.height.to_le_bytes());
        h.update(&[image.channels]);
        h.update(&image.data);
        Self(*h.finalize().as_bytes())
    }
}

/// Where OCR results are kept between pages and files. Shared by all workers.
pub trait OcrCache: Send + Sync {
    /// The result stored under `key`.
    fn get(&self, key: &CacheKey) -> Option<Arc<OcrPage>>;

    /// Stores a result.
    fn put(&self, key: CacheKey, page: Arc<OcrPage>);
}

/// Bounded least-recently-used cache in memory, by entry count and by an estimate of the
/// bytes held (text plus boxes).
pub struct MemoryCache {
    inner: Mutex<Lru>,
}

struct Lru {
    map: HashMap<CacheKey, (Arc<OcrPage>, u64, usize)>,
    tick: u64,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl MemoryCache {
    /// Holds at most `max_entries` pages and about `max_bytes` of results.
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Lru {
                map: HashMap::new(),
                tick: 0,
                bytes: 0,
                max_entries: max_entries.max(1),
                max_bytes: max_bytes.max(1),
            }),
        }
    }

    /// Number of cached pages.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map
            .len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryCache {
    /// 512 pages, 64 MiB.
    fn default() -> Self {
        Self::new(512, 64 << 20)
    }
}

/// Rough heap size of a result.
fn weight(p: &OcrPage) -> usize {
    let mut n = std::mem::size_of::<OcrPage>();
    for l in &p.lines {
        n += std::mem::size_of_val(l) + l.text.len();
        for w in &l.words {
            n += std::mem::size_of_val(w) + w.text.len();
        }
    }
    n
}

impl OcrCache for MemoryCache {
    fn get(&self, key: &CacheKey) -> Option<Arc<OcrPage>> {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.tick += 1;
        let tick = g.tick;
        g.map.get_mut(key).map(|e| {
            e.1 = tick;
            Arc::clone(&e.0)
        })
    }

    fn put(&self, key: CacheKey, page: Arc<OcrPage>) {
        let w = weight(&page);
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if w > g.max_bytes {
            return;
        }
        g.tick += 1;
        let tick = g.tick;
        if let Some(old) = g.map.insert(key, (page, tick, w)) {
            g.bytes -= old.2;
        }
        g.bytes += w;
        // Evicting scans the map: entries are few (hundreds) and a put follows seconds of OCR.
        while g.map.len() > g.max_entries || g.bytes > g.max_bytes {
            let Some(oldest) = g.map.iter().min_by_key(|(_, e)| e.1).map(|(k, _)| *k) else {
                break;
            };
            if let Some(e) = g.map.remove(&oldest) {
                g.bytes -= e.2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{OcrLine, PxBox};
    use crate::script::Script;

    fn img(v: u8) -> Image {
        Image {
            width: 2,
            height: 1,
            channels: 1,
            data: vec![v, 0],
        }
    }

    fn page(t: &str) -> Arc<OcrPage> {
        Arc::new(OcrPage {
            lines: vec![OcrLine {
                text: t.into(),
                quad: [(0.0, 0.0); 4],
                bbox: PxBox::default(),
                conf: 1.0,
                script: Script::Latin,
                words: vec![],
            }],
            ..OcrPage::default()
        })
    }

    #[test]
    fn key_covers_pixels_engine_and_hints() {
        let e = EngineId::new("e", b"");
        let h = OcrHints::default();
        let k = CacheKey::new(&e, &h, &img(1));
        assert_eq!(k, CacheKey::new(&e, &h, &img(1)));
        assert_ne!(k, CacheKey::new(&e, &h, &img(2)));
        assert_ne!(k, CacheKey::new(&EngineId::new("e", b"x"), &h, &img(1)));
        let h2 = OcrHints { dpi: 300.0, ..h };
        assert_ne!(k, CacheKey::new(&e, &h2, &img(1)));
    }

    #[test]
    fn evicts_least_recently_used() {
        let c = MemoryCache::new(2, 1 << 20);
        let (a, b, d) = (CacheKey([1; 32]), CacheKey([2; 32]), CacheKey([3; 32]));
        c.put(a, page("a"));
        c.put(b, page("b"));
        assert!(c.get(&a).is_some()); // a is now newer than b
        c.put(d, page("d"));
        assert_eq!(c.len(), 2);
        assert!(c.get(&b).is_none());
        assert!(c.get(&a).is_some());
        assert!(c.get(&d).is_some());
    }

    #[test]
    fn respects_byte_budget() {
        let one = weight(&page("x"));
        let c = MemoryCache::new(100, one * 2);
        for i in 0..10u8 {
            c.put(CacheKey([i; 32]), page("x"));
        }
        assert!(c.len() <= 2);
        // A single result bigger than the budget is not stored.
        let tiny = MemoryCache::new(10, 1);
        tiny.put(CacheKey([0; 32]), page("x"));
        assert!(tiny.is_empty());
    }
}

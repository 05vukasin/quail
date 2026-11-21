//! Summary cache keyed by `BLAKE3(card text ‖ text head ‖ summarizer id)`.
//!
//! LLM summaries cost seconds per file and exact duplicates are common in business archives
//! (the same annex attached to every contract), so results are cached by what the model sees.
//! A new model file, prompt or decoding setting changes the [`SummarizerId`] and so the key.
//! [`MemoryCache`] is a bounded in-process LRU; a persistent store implements
//! [`SummaryCache`] the same way.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::summarizer::{SummarizerId, Summary};

/// Key of one cached summary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CacheKey(pub [u8; 32]);

impl CacheKey {
    /// Key for summarizing `card_text` and `text_head` with `id`.
    pub fn new(card_text: &str, text_head: &str, id: &SummarizerId) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"quail-summary/cache/1\0");
        for part in [
            card_text.as_bytes(),
            text_head.as_bytes(),
            id.name.as_bytes(),
            &id.version.to_le_bytes(),
            id.fingerprint.as_bytes(),
        ] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part);
        }
        Self(*h.finalize().as_bytes())
    }
}

/// Where summaries are kept between files. Shared by all workers.
pub trait SummaryCache: Send + Sync {
    /// The summary stored under `key`.
    fn get(&self, key: &CacheKey) -> Option<Arc<Summary>>;
    /// Stores a summary.
    fn put(&self, key: CacheKey, summary: Arc<Summary>);
}

/// Bounded least-recently-used cache in memory.
pub struct MemoryCache {
    inner: Mutex<Lru>,
}

struct Lru {
    map: HashMap<CacheKey, (Arc<Summary>, u64)>,
    tick: u64,
    max: usize,
}

impl MemoryCache {
    /// Holds at most `max_entries` summaries.
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(Lru {
                map: HashMap::new(),
                tick: 0,
                max: max_entries.max(1),
            }),
        }
    }

    /// Number of cached summaries.
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

impl SummaryCache for MemoryCache {
    fn get(&self, key: &CacheKey) -> Option<Arc<Summary>> {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.tick += 1;
        let t = g.tick;
        g.map.get_mut(key).map(|(s, used)| {
            *used = t;
            Arc::clone(s)
        })
    }

    fn put(&self, key: CacheKey, summary: Arc<Summary>) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.tick += 1;
        let t = g.tick;
        if !g.map.contains_key(&key)
            && g.map.len() >= g.max
            && let Some(old) = g.map.iter().min_by_key(|(_, (_, u))| *u).map(|(k, _)| *k)
        {
            g.map.remove(&old);
        }
        g.map.insert(key, (summary, t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::Method;

    fn id(fp: &str) -> SummarizerId {
        SummarizerId {
            name: "m".into(),
            version: 1,
            fingerprint: fp.into(),
        }
    }

    fn summary(t: &str) -> Arc<Summary> {
        Arc::new(Summary {
            text: t.into(),
            subject: None,
            title_en: None,
            method: Method::Llm,
            checked: true,
            warnings: Vec::new(),
        })
    }

    #[test]
    fn keys_change_with_every_input() {
        let a = CacheKey::new("card", "head", &id("x"));
        assert_eq!(a, CacheKey::new("card", "head", &id("x")));
        assert_ne!(a, CacheKey::new("card", "head", &id("y")));
        assert_ne!(a, CacheKey::new("cardh", "ead", &id("x")));
    }

    #[test]
    fn evicts_the_least_recently_used() {
        let c = MemoryCache::new(2);
        let (k1, k2, k3) = (
            CacheKey::new("1", "", &id("")),
            CacheKey::new("2", "", &id("")),
            CacheKey::new("3", "", &id("")),
        );
        c.put(k1, summary("one"));
        c.put(k2, summary("two"));
        assert!(c.get(&k1).is_some());
        c.put(k3, summary("three"));
        assert_eq!(c.len(), 2);
        assert!(c.get(&k2).is_none());
        assert_eq!(c.get(&k1).map(|s| s.text.clone()).as_deref(), Some("one"));
    }
}

//! Run-level content dedup ([`RunOptions::dedup_by_content`](crate::RunOptions)).
//!
//! Every file is hashed (BLAKE3) by the reader, in source order, before it is handed to a
//! worker ([`Dedup::register`]): so the first copy of a content is always the one first in
//! the source (lowest sequence number), whatever the number of threads. It is processed as
//! usual and its finished record kept in a bounded LRU cache; a later copy gets that record
//! back with its own locator, an `engine.duplicate_of` fact and the `engine.dedup` step,
//! without running any step. A copy that arrives while the first is still being processed
//! waits for it, at most [`Dedup::wait_for`]; after that (or when the first copy's record
//! could not be kept) it is processed normally.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use quail_core::{AnalyzerId, Fact, FileRecord, Status, Timing, Value};

use crate::keys;

/// The engine's dedup step: its id is in the `steps` and `timings` of every duplicate.
pub const DEDUP_ID: AnalyzerId = AnalyzerId::new("engine.dedup", 1);

/// A BLAKE3 digest.
pub(crate) type Digest = [u8; 32];

/// The content digest the dedup cache keys on.
pub(crate) fn digest(bytes: &[u8]) -> Digest {
    *blake3::hash(bytes).as_bytes()
}

/// What a worker does with a file, decided by [`Dedup::claim`].
pub(crate) enum Claim {
    /// First copy: process it, then [`Dedup::finish`] (through [`Owner`]).
    Owner,
    /// Already processed in this run: reuse this record.
    Hit(Arc<FileRecord>),
    /// The first copy is still in flight after the wait: process this one too, but do not
    /// publish it.
    Busy,
}

struct Entry {
    record: Arc<FileRecord>,
    tick: u64,
    bytes: u64,
}

#[derive(Default)]
struct State {
    done: HashMap<Digest, Entry>,
    /// `tick → digest`, oldest first.
    lru: BTreeMap<u64, Digest>,
    tick: u64,
    bytes: u64,
    in_flight: HashSet<Digest>,
    evicted: u64,
}

/// The run's dedup cache, shared by the workers.
pub(crate) struct Dedup {
    state: Mutex<State>,
    cond: Condvar,
    max_records: usize,
    max_bytes: u64,
    /// Longest a duplicate waits for its first copy to finish.
    wait_for: Duration,
}

impl Dedup {
    pub(crate) fn new(max_records: usize, max_bytes: u64, wait_for: Duration) -> Self {
        Self {
            state: Mutex::new(State::default()),
            cond: Condvar::new(),
            max_records,
            max_bytes,
            wait_for,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records evicted so far.
    pub(crate) fn evicted(&self) -> u64 {
        self.lock().evicted
    }

    /// Records held now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().done.len()
    }

    /// Called by the reader, in source order: `true` when this is the first copy of
    /// `digest` (nothing cached, nothing in flight), which then counts as in flight until its
    /// [`Owner`] finishes. A later copy calls [`Self::claim`] on its worker.
    pub(crate) fn register(&self, digest: &Digest) -> bool {
        let mut st = self.lock();
        if st.done.contains_key(digest) {
            return false;
        }
        st.in_flight.insert(*digest)
    }

    /// Looks `digest` up; waits while its first copy is in flight.
    pub(crate) fn claim(&self, digest: &Digest) -> Claim {
        // `None`: a timeout too large to add, so wait without one.
        let until = Instant::now().checked_add(self.wait_for);
        let mut st = self.lock();
        loop {
            st.tick += 1;
            let tick = st.tick;
            if let Some(e) = st.done.get_mut(digest) {
                let old = std::mem::replace(&mut e.tick, tick);
                let record = Arc::clone(&e.record);
                st.lru.remove(&old);
                st.lru.insert(tick, *digest);
                return Claim::Hit(record);
            }
            if st.in_flight.insert(*digest) {
                return Claim::Owner;
            }
            st = match until {
                Some(until) => {
                    let now = Instant::now();
                    if now >= until {
                        return Claim::Busy;
                    }
                    self.cond
                        .wait_timeout(st, until - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self.cond.wait(st).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// Ends the first copy of `digest`: keeps its record (unless it failed or is too big for
    /// the cache) and wakes the copies waiting for it.
    fn finish(&self, digest: &Digest, record: Option<&FileRecord>) {
        let mut st = self.lock();
        st.in_flight.remove(digest);
        if let Some(r) = record.filter(|r| !matches!(r.status, Status::Failed { .. })) {
            let bytes = approx_bytes(r);
            if self.max_records > 0 && bytes <= self.max_bytes {
                st.tick += 1;
                let tick = st.tick;
                st.bytes += bytes;
                st.lru.insert(tick, *digest);
                let entry = Entry {
                    record: Arc::new(r.clone()),
                    tick,
                    bytes,
                };
                if let Some(old) = st.done.insert(*digest, entry) {
                    st.lru.remove(&old.tick);
                    st.bytes -= old.bytes;
                }
                while st.done.len() > self.max_records || st.bytes > self.max_bytes {
                    let Some((_, oldest)) = st.lru.pop_first() else {
                        break;
                    };
                    if let Some(e) = st.done.remove(&oldest) {
                        st.bytes -= e.bytes;
                        st.evicted += 1;
                    }
                }
            }
        }
        drop(st);
        self.cond.notify_all();
    }
}

/// The first copy of a content while it is processed. Dropping it without
/// [`Owner::publish`] (an error, a panic) still releases the copies waiting for it; they
/// are then processed normally.
pub(crate) struct Owner<'a> {
    dedup: &'a Dedup,
    digest: Digest,
    done: bool,
}

impl<'a> Owner<'a> {
    pub(crate) fn new(dedup: &'a Dedup, digest: Digest) -> Self {
        Self {
            dedup,
            digest,
            done: false,
        }
    }

    /// Keeps `record` for the copies to come.
    pub(crate) fn publish(mut self, record: &FileRecord) {
        self.done = true;
        self.dedup.finish(&self.digest, Some(record));
    }
}

impl Drop for Owner<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.dedup.finish(&self.digest, None);
        }
    }
}

/// The record of a duplicate: `first`'s facts, steps, format, pages, status and warnings
/// under `locator`, plus `engine.duplicate_of` (the first copy's locator) and the
/// `engine.dedup` step, whose time (hashing and lookup) is the only timing.
pub(crate) fn duplicate_record(first: &FileRecord, locator: String, micros: u64) -> FileRecord {
    let mut facts = first.facts.clone();
    facts.push(Fact {
        key: keys::DUPLICATE_OF,
        value: Value::text(first.locator.as_str()),
        conf: 1.0,
        src: DEDUP_ID,
        evidence: None,
    });
    let mut steps = first.steps.clone();
    steps.push(DEDUP_ID);
    FileRecord {
        schema: first.schema,
        locator,
        size: first.size,
        format: first.format,
        pages: first.pages,
        facts,
        steps,
        timings: vec![Timing {
            src: DEDUP_ID,
            micros,
        }],
        status: first.status.clone(),
        warnings: first.warnings.clone(),
    }
}

/// Rough heap size of a record, for the cache's byte cap.
fn approx_bytes(r: &FileRecord) -> u64 {
    let facts: usize = r
        .facts
        .iter()
        .map(|f| std::mem::size_of::<Fact>() + f.key.as_str().len() + value_bytes(&f.value))
        .sum();
    let warnings: usize = r.warnings.iter().map(|w| w.len() + 24).sum();
    let status = match &r.status {
        Status::Ok => 0,
        Status::Partial { problems } => problems.iter().map(|p| p.len() + 24).sum(),
        Status::Failed { error } => error.len(),
    };
    let total = std::mem::size_of::<FileRecord>()
        + r.locator.len()
        + facts
        + r.steps.len() * std::mem::size_of::<AnalyzerId>()
        + warnings
        + status;
    total as u64
}

fn value_bytes(v: &Value) -> usize {
    match v {
        Value::Text(s) => s.len(),
        Value::Bytes(b) => b.len(),
        Value::List(items) => items
            .iter()
            .map(|x| std::mem::size_of::<Value>() + value_bytes(x))
            .sum(),
        Value::Record(fields) => fields
            .iter()
            .map(|(k, x)| std::mem::size_of::<(Box<str>, Value)>() + k.len() + value_bytes(x))
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::failed_record;

    fn ok(locator: &str) -> FileRecord {
        FileRecord {
            status: Status::Ok,
            ..failed_record(locator.to_owned(), 1, String::new())
        }
    }

    fn put(d: &Dedup, key: u8) {
        let digest = [key; 32];
        assert!(matches!(d.claim(&digest), Claim::Owner));
        Owner::new(d, digest).publish(&ok(&key.to_string()));
    }

    #[test]
    fn a_hit_refreshes_recency() {
        let d = Dedup::new(2, u64::MAX, Duration::ZERO);
        put(&d, 1);
        put(&d, 2);
        assert!(matches!(d.claim(&[1; 32]), Claim::Hit(_)));
        put(&d, 3); // evicts 2, the least recently used
        assert_eq!(d.len(), 2);
        assert_eq!(d.evicted(), 1);
        assert!(matches!(d.claim(&[1; 32]), Claim::Hit(_)));
        assert!(matches!(d.claim(&[2; 32]), Claim::Owner));
    }

    #[test]
    fn a_dropped_owner_releases_the_waiters() {
        let d = Dedup::new(8, u64::MAX, Duration::from_secs(30));
        let digest = [7; 32];
        assert!(matches!(d.claim(&digest), Claim::Owner));
        let owner = Owner::new(&d, digest);
        std::thread::scope(|s| {
            let waiter = s.spawn(|| matches!(d.claim(&digest), Claim::Owner));
            std::thread::sleep(Duration::from_millis(50));
            drop(owner); // no record: the waiter becomes the owner
            assert!(waiter.join().unwrap());
        });
        assert_eq!(d.len(), 0);
    }

    #[test]
    fn a_busy_first_copy_times_out() {
        let d = Dedup::new(8, u64::MAX, Duration::from_millis(20));
        let digest = [9; 32];
        assert!(matches!(d.claim(&digest), Claim::Owner));
        assert!(matches!(d.claim(&digest), Claim::Busy));
    }
}

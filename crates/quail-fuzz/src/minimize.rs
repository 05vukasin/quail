//! Delta debugging: the smallest input (within a budget) that still ends with the same
//! signature. Zip packages are reduced entry by entry first (re-zipped, stored), then inside
//! each entry; everything else byte-wise.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::runner::Runner;
use crate::seeds::Kind;
use crate::zipfmt::{self, Entry, Eocd, Method};

/// Budget of one minimization.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Wall time.
    pub time: Duration,
    /// Runs of the target.
    pub trials: u32,
}

struct Oracle<'a> {
    runner: &'a mut Runner,
    path: &'a Path,
    signature: &'a str,
    start: Instant,
    budget: Budget,
    trials: u32,
}

impl Oracle<'_> {
    fn exhausted(&self) -> bool {
        self.trials >= self.budget.trials || self.start.elapsed() >= self.budget.time
    }

    fn test(&mut self, data: &[u8]) -> bool {
        if self.exhausted() {
            return false;
        }
        self.trials += 1;
        if std::fs::write(self.path, data).is_err() {
            return false;
        }
        self.runner.run(self.path).signature() == self.signature
    }
}

/// Classic ddmin over the units of `items`, keeping those `keep` needs.
fn ddmin<T: Clone>(items: Vec<T>, o: &mut Oracle<'_>, build: &dyn Fn(&[T]) -> Vec<u8>) -> Vec<T> {
    let mut cur = items;
    let mut n = 2usize;
    while cur.len() >= 2 && !o.exhausted() {
        let chunk = cur.len().div_ceil(n);
        let found = (0..n).find_map(|i| {
            let (s, e) = (i * chunk, ((i + 1) * chunk).min(cur.len()));
            if s >= e {
                return None;
            }
            let cand: Vec<T> = cur[..s].iter().chain(&cur[e..]).cloned().collect();
            o.test(&build(&cand)).then_some(cand)
        });
        if let Some(cand) = found {
            cur = cand;
            n = (n - 1).max(2);
        } else {
            if n >= cur.len() {
                break;
            }
            n = (n * 2).min(cur.len());
        }
    }
    cur
}

fn rezip(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let es: Vec<Entry> = entries
        .iter()
        .map(|(n, d)| Entry {
            method: Method::Store,
            ..Entry::new(n.clone(), d.clone())
        })
        .collect();
    zipfmt::write(&es, Eocd::default(), b"")
}

/// Minimizes `data` so that running it still gives `signature`. `path` is a scratch file.
pub fn minimize(
    runner: &mut Runner,
    kind: Kind,
    data: &[u8],
    signature: &str,
    path: &Path,
    budget: Budget,
) -> Vec<u8> {
    let mut o = Oracle {
        runner,
        path,
        signature,
        start: Instant::now(),
        budget,
        trials: 0,
    };
    if kind == Kind::Docx
        && let Some(entries) = zipfmt::read(data, 64 << 20)
        && o.test(&rezip(&entries))
    {
        let mut entries = ddmin(entries, &mut o, &|es| rezip(es));
        for i in 0..entries.len() {
            if o.exhausted() {
                break;
            }
            if entries[i].1.len() < 16 {
                continue;
            }
            let before = entries.clone();
            let bytes = entries[i].1.clone();
            let kept = ddmin(bytes, &mut o, &|d| {
                let mut es = before.clone();
                es[i].1 = d.to_vec();
                rezip(&es)
            });
            entries[i].1 = kept;
        }
        return rezip(&entries);
    }
    ddmin(data.to_vec(), &mut o, &|d| d.to_vec())
}

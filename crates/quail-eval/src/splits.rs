//! Deterministic dev / holdout assignment and committable split lists.
//!
//! A file goes to holdout when `blake3("<seed>:<id>")`, read as a fraction of 2^64, is below
//! the holdout share. The decision depends on the file alone, so adding files to a dataset
//! never moves existing ones. Once written, the lists (`<dataset>.dev.txt`,
//! `<dataset>.holdout.txt`: one BLAKE3 id per line, nothing else) are the frozen truth:
//! files already listed keep their side even if the seed or share changes; only new files
//! are assigned by hash. They hold hashes only, so they can be committed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::gold::{GoldDoc, Split};
use crate::{Error, Result};

/// Seed of quail's frozen holdout. Changing it re-freezes a new holdout: only at a release,
/// with both holdouts reported.
pub const DEFAULT_SEED: &str = "quail-eval-holdout-2026-11-20";

/// Default share of files in holdout.
pub const DEFAULT_HOLDOUT_SHARE: f64 = 0.2;

/// Side of `id` by hash.
pub fn assign(id: &str, seed: &str, holdout_share: f64) -> Split {
    let h = quail_hash::blake3(format!("{seed}:{id}").as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&h[..8]);
    #[allow(clippy::cast_precision_loss)]
    let x = u64::from_be_bytes(head) as f64 / 18_446_744_073_709_551_616.0;
    if x < holdout_share {
        Split::Holdout
    } else {
        Split::Dev
    }
}

/// Frozen lists: id → side, per dataset.
pub type Lists = BTreeMap<String, BTreeMap<String, Split>>;

/// Reads every `<dataset>.{dev,holdout}.txt` in `dir` (missing dir = no lists). An id on
/// both sides is an error.
pub fn load_lists(dir: &Path) -> Result<Lists> {
    let mut out: Lists = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(out);
    };
    for entry in rd {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let (dataset, side) = if let Some(d) = name.strip_suffix(".dev.txt") {
            (d, Split::Dev)
        } else if let Some(d) = name.strip_suffix(".holdout.txt") {
            (d, Split::Holdout)
        } else {
            continue;
        };
        let text = crate::read_to_string(&path)?;
        let map = out.entry(dataset.to_string()).or_default();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            if let Some(prev) = map.insert(line.to_string(), side)
                && prev != side
            {
                return Err(Error::parse(
                    &path,
                    format!("{line} is in both dev and holdout"),
                ));
            }
        }
    }
    Ok(out)
}

/// The side of each gold document: the frozen list when it has the id, the hash otherwise.
pub fn resolve(docs: &[GoldDoc], lists: &Lists, seed: &str, share: f64) -> Vec<Split> {
    docs.iter()
        .map(|d| {
            let id = d.id();
            lists
                .get(&d.dataset)
                .and_then(|m| m.get(&id))
                .copied()
                .unwrap_or_else(|| assign(&id, seed, share))
        })
        .collect()
}

/// Lists for `docs` with sides `sides`, merged into `lists` (existing entries kept).
pub fn merged(docs: &[GoldDoc], sides: &[Split], lists: &Lists) -> Lists {
    let mut out = lists.clone();
    for (d, s) in docs.iter().zip(sides) {
        out.entry(d.dataset.clone())
            .or_default()
            .entry(d.id())
            .or_insert(*s);
    }
    out
}

/// Writes the lists to `dir`, sorted, one id per line.
pub fn write_lists(dir: &Path, lists: &Lists) -> Result<()> {
    for (dataset, map) in lists {
        for side in [Split::Dev, Split::Holdout] {
            let ids: BTreeSet<&str> = map
                .iter()
                .filter(|(_, s)| **s == side)
                .map(|(id, _)| id.as_str())
                .collect();
            let mut text = String::new();
            for id in ids {
                text.push_str(id);
                text.push('\n');
            }
            crate::write_file(&dir.join(format!("{dataset}.{}.txt", side.as_str())), text)?;
        }
    }
    Ok(())
}

/// Counts per dataset: (dev, holdout).
pub fn counts(lists: &Lists) -> BTreeMap<String, (usize, usize)> {
    lists
        .iter()
        .map(|(d, m)| {
            let h = m.values().filter(|s| **s == Split::Holdout).count();
            (d.clone(), (m.len() - h, h))
        })
        .collect()
}

/// Gold documents whose `split` field disagrees with `sides`: (id, field, expected).
pub fn disagreements(docs: &[GoldDoc], sides: &[Split]) -> Vec<(String, Split, Split)> {
    docs.iter()
        .zip(sides)
        .filter(|(d, s)| d.split != **s)
        .map(|(d, s)| (d.id(), d.split, *s))
        .collect()
}

/// Id → side, across datasets (for quick lookups).
pub fn flatten(lists: &Lists) -> HashMap<String, Split> {
    lists
        .values()
        .flat_map(|m| m.iter().map(|(k, v)| (k.clone(), *v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gold::GoldFile;

    fn doc(i: u32) -> GoldDoc {
        GoldDoc {
            file: GoldFile {
                blake3: Some(crate::blake3_hex(&i.to_le_bytes())),
                locator_hint: None,
            },
            dataset: "syn".into(),
            split: Split::Dev,
            source: None,
            labels: vec![],
            absent: vec![],
        }
    }

    #[test]
    fn assignment_is_deterministic_and_near_share() {
        let ids: Vec<String> = (0..5000u32)
            .map(|i| crate::blake3_hex(&i.to_le_bytes()))
            .collect();
        let hold = ids
            .iter()
            .filter(|id| assign(id, DEFAULT_SEED, 0.2) == Split::Holdout)
            .count();
        assert!((900..1100).contains(&hold), "{hold}");
        assert_eq!(
            assign(&ids[7], DEFAULT_SEED, 0.2),
            assign(&ids[7], DEFAULT_SEED, 0.2)
        );
        assert_eq!(assign(&ids[7], DEFAULT_SEED, 0.0), Split::Dev);
        assert_eq!(assign(&ids[7], DEFAULT_SEED, 1.0), Split::Holdout);
    }

    #[test]
    fn lists_are_frozen() {
        let dir = tempfile::tempdir().unwrap();
        let docs: Vec<GoldDoc> = (0..50).map(doc).collect();
        let sides = resolve(&docs, &Lists::new(), DEFAULT_SEED, 0.2);
        write_lists(dir.path(), &merged(&docs, &sides, &Lists::new())).unwrap();
        let lists = load_lists(dir.path()).unwrap();
        // A different seed changes nothing for listed files.
        let again = resolve(&docs, &lists, "other-seed", 0.9);
        assert_eq!(again, sides);
        assert_eq!(
            disagreements(&docs, &sides).len(),
            sides.iter().filter(|s| **s == Split::Holdout).count()
        );
        let (dev, hold) = counts(&lists)["syn"];
        assert_eq!(dev + hold, 50);
    }
}

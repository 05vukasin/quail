//! Dataset adapters: public datasets (under `$QUAIL_TESTDATA`) converted into gold files.
//!
//! Each adapter reads the dataset's own annotation format, maps its fields to quail's
//! neutral task names, finds the document file to hash it (BLAKE3 is the join key) and
//! assigns dev/holdout with [`crate::splits`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::gold::{GoldDoc, GoldFile};
use crate::splits;
use crate::{Error, Result};

pub mod cuad;
pub mod kleister;
pub mod realkie;

pub use cuad::Cuad;
pub use kleister::KleisterNda;
pub use realkie::RealkieFcc;

/// Options shared by adapters.
#[derive(Clone, Debug)]
pub struct ConvertOptions {
    /// Split seed.
    pub seed: String,
    /// Share of files in holdout.
    pub holdout_share: f64,
    /// Frozen split lists (take precedence over the hash).
    pub lists: splits::Lists,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            seed: splits::DEFAULT_SEED.into(),
            holdout_share: splits::DEFAULT_HOLDOUT_SHARE,
            lists: splits::Lists::new(),
        }
    }
}

/// What an adapter produced.
#[derive(Debug, Default)]
pub struct Converted {
    /// Gold documents.
    pub docs: Vec<GoldDoc>,
    /// Counters (documents read, files not found, values skipped, ...).
    pub stats: BTreeMap<String, u64>,
}

impl Converted {
    pub(crate) fn bump(&mut self, key: &str) {
        *self.stats.entry(key.to_string()).or_default() += 1;
    }
}

/// Converts one public dataset into gold documents.
pub trait GoldAdapter {
    /// Dataset name, used as `dataset` in gold and as the default directory under
    /// `$QUAIL_TESTDATA`.
    fn name(&self) -> &'static str;

    /// Reads the dataset rooted at `root`.
    fn convert(&self, root: &Path, opts: &ConvertOptions) -> Result<Converted>;
}

/// Every built-in adapter.
pub fn all() -> Vec<Box<dyn GoldAdapter>> {
    vec![Box::new(KleisterNda), Box::new(Cuad), Box::new(RealkieFcc)]
}

/// The adapter called `name`.
pub fn by_name(name: &str) -> Option<Box<dyn GoldAdapter>> {
    all().into_iter().find(|a| a.name() == name)
}

/// The file's identity for gold: BLAKE3 when the file is readable, plus a locator hint
/// relative to `root`.
pub(crate) fn gold_file(root: &Path, file: Option<&Path>, fallback_hint: &str) -> GoldFile {
    match file {
        Some(p) => GoldFile {
            blake3: std::fs::read(p).ok().map(|b| crate::blake3_hex(&b)),
            locator_hint: Some(
                p.strip_prefix(root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/"),
            ),
        },
        None => GoldFile {
            blake3: None,
            locator_hint: Some(fallback_hint.to_string()),
        },
    }
}

/// Assigns the split of `doc` from the frozen lists or the hash.
pub(crate) fn assign_split(doc: &mut GoldDoc, opts: &ConvertOptions) {
    let sides = splits::resolve(
        std::slice::from_ref(doc),
        &opts.lists,
        &opts.seed,
        opts.holdout_share,
    );
    if let Some(s) = sides.first() {
        doc.split = *s;
    }
}

/// Every regular file under `root` (recursively), sorted.
pub(crate) fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    fn go(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for e in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
            let e = e.map_err(|e| Error::io(dir, e))?;
            let p = e.path();
            let ft = e.file_type().map_err(|e| Error::io(&p, e))?;
            if ft.is_dir() {
                go(&p, out)?;
            } else if ft.is_file() {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(root, &mut out)?;
    out.sort();
    Ok(out)
}

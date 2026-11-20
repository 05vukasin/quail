//! Evaluation harness of quail: decides, with numbers, whether a rule or model change is
//! better or worse, and lays the metadata next to the file so a person can check it.
//!
//! The workflow (see the crate README):
//!
//! 1. **run**: process a corpus with quail and write JSONL records ([`quail_io::JsonlSink`]).
//! 2. **score** ([`score::score`]): records + gold files + the task map ([`tasks::TaskMap`])
//!    give a [`score::Scoreboard`] (JSON) and a Markdown report ([`report::markdown`]).
//! 3. **compare** ([`compare::compare`]): two scoreboards, before and after a change; a drop
//!    in any task's F1 beyond a threshold, or more failed files, is a regression.
//! 4. **inspect** ([`inspect::write_packs`]): for chosen files, a pack with the record, gold
//!    vs predicted per task and the page text with every fact's evidence marked.
//!
//! Gold ([`gold::GoldDoc`]) names neutral *tasks* (`party`, `effective_date`, ...), not fact
//! keys, so it survives key renames; `tasks.toml` maps each task to the keys that answer it.
//! Files are matched to records by BLAKE3 (the `hash.blake3` fact), then by locator.
//! Dev and holdout are assigned per file from its hash ([`splits`]); holdout is scored only
//! when asked for explicitly.

pub mod adapters;
pub mod compare;
pub mod gold;
pub mod inspect;
pub mod normalize;
pub mod records;
pub mod report;
pub mod score;
pub mod splits;
pub mod tasks;

use std::fmt;
use std::path::{Path, PathBuf};

/// Result with the harness's error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What can go wrong while evaluating. Every variant names the file involved when there is one.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing a file failed.
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// A file could not be parsed (gold JSON, tasks TOML, scoreboard, dataset file).
    Parse {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        msg: String,
    },
    /// Input that parses but is not valid (a bad gold label, an unknown task, ...).
    Invalid(String),
    /// An error from a quail crate.
    Quail(quail_core::Error),
}

impl Error {
    /// I/O error on `path`.
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    /// Parse error in `path`.
    pub fn parse(path: impl AsRef<Path>, msg: impl fmt::Display) -> Self {
        Self::Parse {
            path: path.as_ref().to_path_buf(),
            msg: msg.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { path, msg } => write!(f, "{}: {msg}", path.display()),
            Self::Invalid(msg) => f.write_str(msg),
            Self::Quail(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Quail(e) => Some(e),
            _ => None,
        }
    }
}

impl From<quail_core::Error> for Error {
    fn from(e: quail_core::Error) -> Self {
        Self::Quail(e)
    }
}

/// Lowercase hex of bytes.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Lowercase hex BLAKE3 of bytes.
pub fn blake3_hex(bytes: &[u8]) -> String {
    hex(&quail_hash::blake3(bytes))
}

/// Writes `contents` to `path`, creating parent directories.
pub(crate) fn write_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(path, contents).map_err(|e| Error::io(path, e))
}

/// Reads a whole file as UTF-8.
pub(crate) fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

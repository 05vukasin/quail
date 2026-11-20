//! Turning a path into a [`SourceItem`].

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use quail_core::{Blob, Error, Result, SourceItem};

/// How a source reads a file into a [`Blob`]. The default reads every file whole into memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    max_file_bytes: Option<u64>,
    #[cfg(feature = "mmap")]
    mmap_min_bytes: Option<u64>,
}

impl ReadOptions {
    /// Default options: read everything, no cap, no memory mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Files larger than `cap` bytes are not read; they become an error item of kind
    /// [`io::ErrorKind::FileTooLarge`]. `None` (the default) reads files of any size and lets
    /// the engine apply `Limits::max_bytes`.
    #[must_use]
    pub fn max_file_bytes(mut self, cap: Option<u64>) -> Self {
        self.max_file_bytes = cap;
        self
    }

    /// The configured size cap.
    pub fn file_cap(&self) -> Option<u64> {
        self.max_file_bytes
    }

    /// Memory-maps files of at least `min` bytes instead of reading them; smaller files are
    /// still read (a mapping costs more than a read for small files). `None` (the default)
    /// never maps. See the crate README for when this helps and the risk it carries.
    #[cfg(feature = "mmap")]
    #[must_use]
    pub fn mmap_min_bytes(mut self, min: Option<u64>) -> Self {
        self.mmap_min_bytes = min;
        self
    }
}

/// Wraps an I/O error so its message names the path, keeping its kind.
pub(crate) fn io_error(path: &Path, e: &io::Error) -> Error {
    Error::Io(io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// Seconds since the Unix epoch, negative before it.
fn unix_seconds(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// Reads one file into a [`SourceItem`] whose locator is the path (lossily converted to
/// UTF-8: a non-UTF-8 name gets U+FFFD replacement characters).
///
/// Fails, with the path in the message, when the file cannot be opened or read, is not a
/// regular file, or is larger than [`ReadOptions::max_file_bytes`] (also when it grows past
/// the cap while being read).
pub fn read_file(path: &Path, opts: &ReadOptions) -> Result<SourceItem> {
    let file = File::open(path).map_err(|e| io_error(path, &e))?;
    let meta = file.metadata().map_err(|e| io_error(path, &e))?;
    if !meta.is_file() {
        return Err(io_error(
            path,
            &io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"),
        ));
    }
    let len = meta.len();
    if let Some(cap) = opts.max_file_bytes
        && len > cap
    {
        return Err(too_large(path, len, cap));
    }
    let modified = meta.modified().ok().and_then(unix_seconds);

    #[cfg(feature = "mmap")]
    if let Some(min) = opts.mmap_min_bytes
        && len >= min
        && len > 0
    {
        let map = crate::mmap::map(&file).map_err(|e| io_error(path, &e))?;
        return Ok(item(path, Blob::new(map), modified));
    }

    let blob = read_all(file, len, opts.max_file_bytes).map_err(|e| io_error(path, &e))?;
    Ok(item(path, blob, modified))
}

fn item(path: &Path, blob: Blob, modified: Option<i64>) -> SourceItem {
    SourceItem {
        locator: path.to_string_lossy().into_owned(),
        blob,
        modified,
    }
}

fn too_large(path: &Path, len: u64, cap: u64) -> Error {
    io_error(
        path,
        &io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("{len} bytes exceeds the source cap of {cap} bytes"),
        ),
    )
}

/// Reads the whole file, pre-sizing the buffer from the metadata length. With a cap, reads
/// at most `cap + 1` bytes so a file that grows while being read cannot exhaust memory.
fn read_all(file: File, len: u64, cap: Option<u64>) -> io::Result<Blob> {
    // The length is a hint only: the file may change under us. Never pre-allocate more than
    // the cap, and cap the hint itself so a lying size cannot reserve absurd memory.
    let hint = cap.map_or(len, |c| len.min(c));
    let hint = usize::try_from(hint.min(1 << 30)).unwrap_or(0);
    let mut buf = Vec::with_capacity(hint);
    // Without a cap, `take(u64::MAX)` never limits.
    let limit = cap.map_or(u64::MAX, |c| c.saturating_add(1));
    file.take(limit).read_to_end(&mut buf)?;
    if let Some(c) = cap
        && buf.len() as u64 > c
    {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("grew past the source cap of {c} bytes while being read"),
        ));
    }
    Ok(Blob::new(buf))
}

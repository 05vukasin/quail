//! Jobs: what the supervisor hands a worker, and where they come from.

use std::path::PathBuf;

use quail_core::{Blob, Result, Source, SourceItem};

/// How a worker gets a file's bytes.
#[derive(Clone, Debug)]
pub enum JobInput {
    /// The supervisor already has the bytes and sends them through the pipe.
    Bytes(Blob),
    /// The worker reads the file itself: nothing big crosses the pipe, and the supervisor
    /// never holds the file in memory.
    Path(PathBuf),
}

/// One file to process in a worker.
#[derive(Clone, Debug)]
pub struct Job {
    /// Stable name of the file, copied into its record.
    pub locator: String,
    /// Its bytes, or where to read them.
    pub input: JobInput,
    /// Last modification time, seconds since the Unix epoch, when known.
    pub modified: Option<i64>,
}

impl Job {
    /// A job for a file the worker reads itself; the locator is the path as text.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            locator: path.to_string_lossy().into_owned(),
            input: JobInput::Path(path),
            modified: None,
        }
    }

    /// Size in bytes: the blob's length, or the file's size on disk (0 if it cannot be
    /// read; the worker then reports the error).
    pub fn size(&self) -> u64 {
        match &self.input {
            JobInput::Bytes(b) => b.len() as u64,
            JobInput::Path(p) => std::fs::metadata(p).map_or(0, |m| m.len()),
        }
    }
}

impl From<SourceItem> for Job {
    fn from(item: SourceItem) -> Self {
        Self {
            locator: item.locator,
            input: JobInput::Bytes(item.blob),
            modified: item.modified,
        }
    }
}

/// Where jobs come from. Like [`Source`], an error skips one item and, when it carries a
/// locator ([`quail_core::Error::at`]), still gets a `Failed` record.
pub trait JobSource: Send {
    /// The next job, `None` when done.
    fn next_job(&mut self) -> Option<Result<Job>>;
}

/// A [`Source`] as a [`JobSource`]: every item travels as bytes.
pub(crate) struct FromSource<'a>(pub &'a mut dyn Source);

impl JobSource for FromSource<'_> {
    fn next_job(&mut self) -> Option<Result<Job>> {
        self.0.next_item().map(|r| r.map(Job::from))
    }
}

/// Paths as a [`JobSource`]: each file is read by the worker itself ([`JobInput::Path`]).
///
/// Wrap any iterator of paths, e.g. a directory walk that yields paths without reading the
/// files (`quail-io`'s `FsSource::next_path`):
///
/// ```
/// use quail_worker::PathJobs;
/// let jobs = PathJobs::new(vec![Ok::<_, quail_core::Error>("a.pdf".into())].into_iter());
/// # let _ = jobs;
/// ```
pub struct PathJobs<I> {
    paths: I,
}

impl<I> PathJobs<I> {
    /// Wraps an iterator of paths.
    pub fn new(paths: I) -> Self {
        Self { paths }
    }
}

impl<I> JobSource for PathJobs<I>
where
    I: Iterator<Item = Result<PathBuf>> + Send,
{
    fn next_job(&mut self) -> Option<Result<Job>> {
        self.paths.next().map(|r| r.map(Job::path))
    }
}

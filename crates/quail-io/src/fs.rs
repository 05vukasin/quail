//! [`FsSource`]: a lazy walk over directory trees.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use quail_core::{Error, Result, Source, SourceItem};
use walkdir::{DirEntry, WalkDir};

use crate::read::{ReadOptions, read_file};

/// Configures an [`FsSource`]. Start with [`FsSource::builder`].
#[derive(Clone, Debug, Default)]
pub struct FsSourceBuilder {
    roots: Vec<PathBuf>,
    follow_symlinks: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    extensions: Vec<String>,
    skip_hidden: bool,
    max_depth: Option<usize>,
    sorted: bool,
    read: ReadOptions,
}

impl FsSourceBuilder {
    /// Adds a root: a directory to walk, or a single file.
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.roots.push(root.into());
        self
    }

    /// Adds several roots.
    #[must_use]
    pub fn roots<P: Into<PathBuf>>(mut self, roots: impl IntoIterator<Item = P>) -> Self {
        self.roots.extend(roots.into_iter().map(Into::into));
        self
    }

    /// Follows symbolic links (default: off). Off, a symlink is neither yielded nor
    /// descended into; on, links are followed and a link back to an ancestor directory
    /// yields one error item instead of looping. Roots themselves are always followed.
    #[must_use]
    pub fn follow_symlinks(mut self, yes: bool) -> Self {
        self.follow_symlinks = yes;
        self
    }

    /// Yields only files whose path matches at least one of these globs (default: all files).
    ///
    /// Globs are matched against the path relative to its root, with `/` separators; `*`
    /// also matches `/`, so `*.pdf` selects PDFs at any depth and `invoices/**` one subtree.
    /// A root that is a file is matched by its file name.
    #[must_use]
    pub fn include(mut self, glob: impl Into<String>) -> Self {
        self.include.push(glob.into());
        self
    }

    /// Skips files matching any of these globs, and does not descend into directories whose
    /// relative path matches (`**/node_modules`, `archive`). Same syntax as [`Self::include`].
    #[must_use]
    pub fn exclude(mut self, glob: impl Into<String>) -> Self {
        self.exclude.push(glob.into());
        self
    }

    /// Yields only files with one of these extensions, compared case-insensitively, with or
    /// without the leading dot (`"pdf"`, `".DOCX"`). Default: any extension, or none.
    #[must_use]
    pub fn extension(mut self, ext: impl AsRef<str>) -> Self {
        let ext = ext.as_ref().trim_start_matches('.').to_ascii_lowercase();
        self.extensions.push(ext);
        self
    }

    /// Skips files and directories whose name starts with `.` (default: off). Roots are never
    /// skipped.
    #[must_use]
    pub fn skip_hidden(mut self, yes: bool) -> Self {
        self.skip_hidden = yes;
        self
    }

    /// Descends at most `depth` levels below each root (0: the root only; default: no limit).
    #[must_use]
    pub fn max_depth(mut self, depth: Option<usize>) -> Self {
        self.max_depth = depth;
        self
    }

    /// Deterministic order (default: off, directory order as the OS returns it). On, roots
    /// are walked in sorted order and every directory's entries are sorted by file name
    /// (byte order), so two runs over the same tree yield the same sequence.
    ///
    /// Memory: still lazy. Sorting needs a directory's whole listing before its first entry,
    /// so the walk holds the listings of the directories on the current path (one per depth
    /// level), roughly 100 bytes per entry: a flat directory of a million files costs about
    /// 100 MB while it is being walked, a deep tree of small directories almost nothing.
    #[must_use]
    pub fn sorted(mut self, yes: bool) -> Self {
        self.sorted = yes;
        self
    }

    /// How files are read (size cap, memory mapping).
    #[must_use]
    pub fn read_options(mut self, read: ReadOptions) -> Self {
        self.read = read;
        self
    }

    /// Builds the source. Fails only on an invalid glob.
    pub fn build(self) -> Result<FsSource> {
        let include = glob_set(&self.include)?;
        let exclude = glob_set(&self.exclude)?;
        let mut roots = self.roots;
        if self.sorted {
            roots.sort();
            roots.dedup();
        }
        Ok(FsSource {
            roots: roots.into_iter(),
            current: None,
            filter: Filter {
                include,
                exclude,
                extensions: self.extensions,
                skip_hidden: self.skip_hidden,
            },
            follow_symlinks: self.follow_symlinks,
            max_depth: self.max_depth,
            sorted: self.sorted,
            read: self.read,
        })
    }
}

fn glob_set(globs: &[String]) -> Result<Option<GlobSet>> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for g in globs {
        b.add(Glob::new(g).map_err(|e| Error::Other(format!("invalid glob {g:?}: {e}")))?);
    }
    b.build()
        .map(Some)
        .map_err(|e| Error::Other(format!("invalid globs: {e}")))
}

#[derive(Debug)]
struct Filter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    extensions: Vec<String>,
    skip_hidden: bool,
}

impl Filter {
    fn excluded(&self, rel: &Path) -> bool {
        self.exclude.as_ref().is_some_and(|g| g.is_match(rel))
    }

    fn wants_file(&self, rel: &Path, path: &Path) -> bool {
        if self.excluded(rel) {
            return false;
        }
        if let Some(inc) = &self.include
            && !inc.is_match(rel)
        {
            return false;
        }
        if !self.extensions.is_empty() {
            let Some(ext) = path.extension().and_then(OsStr::to_str) else {
                return false;
            };
            if !self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                return false;
            }
        }
        true
    }
}

struct Walk {
    root: PathBuf,
    it: walkdir::IntoIter,
}

/// Walks one or more roots and yields every regular file that passes the filters, reading
/// it with [`ReadOptions`]. Lazy: paths are discovered as the engine pulls, never collected
/// up front (see [`FsSourceBuilder::sorted`] for the memory cost of sorted mode).
///
/// Unreadable files, directories that cannot be listed, missing roots and symlink loops each
/// yield one error item; the walk continues. Sockets, FIFOs and devices are skipped silently,
/// as are symlinks unless [`FsSourceBuilder::follow_symlinks`] is on.
///
/// ```no_run
/// use quail_core::Source;
/// use quail_io::FsSource;
///
/// let mut src = FsSource::builder()
///     .root("/data/contracts")
///     .extension("pdf")
///     .extension("docx")
///     .exclude("**/.git")
///     .sorted(true)
///     .build()?;
/// while let Some(item) = src.next_item() {
///     match item {
///         Ok(item) => println!("{} {}", item.locator, item.blob.len()),
///         Err(e) => eprintln!("skipped: {e}"),
///     }
/// }
/// # Ok::<(), quail_core::Error>(())
/// ```
pub struct FsSource {
    roots: std::vec::IntoIter<PathBuf>,
    current: Option<Walk>,
    filter: Filter,
    follow_symlinks: bool,
    max_depth: Option<usize>,
    sorted: bool,
    read: ReadOptions,
}

impl std::fmt::Debug for FsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsSource")
            .field("current_root", &self.current.as_ref().map(|w| &w.root))
            .field("pending_roots", &self.roots.as_slice())
            .field("sorted", &self.sorted)
            .finish_non_exhaustive()
    }
}

impl FsSource {
    /// A builder with defaults: no roots, no filters, symlinks not followed, OS order.
    pub fn builder() -> FsSourceBuilder {
        FsSourceBuilder::default()
    }

    /// Walks the roots and yields the path of every file that passes the filters, without
    /// reading it. Useful to count or list a corpus. Errors are as for [`Source::next_item`].
    pub fn next_path(&mut self) -> Option<Result<PathBuf>> {
        loop {
            let Some(walk) = &mut self.current else {
                let root = self.roots.next()?;
                self.current = Some(self.start(root));
                continue;
            };
            let entry = match walk.it.next() {
                None => {
                    self.current = None;
                    continue;
                }
                Some(Err(e)) => return Some(Err(walk_error(&e))),
                Some(Ok(entry)) => entry,
            };
            if let Some(path) = self.accept(entry) {
                return Some(Ok(path));
            }
        }
    }

    fn start(&self, root: PathBuf) -> Walk {
        let mut wd = WalkDir::new(&root).follow_links(self.follow_symlinks);
        if let Some(d) = self.max_depth {
            wd = wd.max_depth(d);
        }
        if self.sorted {
            wd = wd.sort_by_file_name();
        }
        Walk {
            it: wd.into_iter(),
            root,
        }
    }

    /// Decides what to do with one entry: `Some(path)` to yield it; prunes directories.
    fn accept(&mut self, entry: DirEntry) -> Option<PathBuf> {
        let walk = self.current.as_mut()?;
        let depth = entry.depth();
        let ft = entry.file_type();
        if depth > 0
            && self.filter.skip_hidden
            && entry.file_name().as_encoded_bytes().first() == Some(&b'.')
        {
            if ft.is_dir() {
                walk.it.skip_current_dir();
            }
            return None;
        }
        let rel: &Path = if depth == 0 {
            entry.path().file_name().map_or(entry.path(), Path::new)
        } else {
            entry
                .path()
                .strip_prefix(&walk.root)
                .unwrap_or(entry.path())
        };
        if ft.is_dir() {
            if depth > 0 && self.filter.excluded(rel) {
                walk.it.skip_current_dir();
            }
            return None;
        }
        // With follow_links off a symlink reports its own type and is skipped here; on, it
        // reports the target's type.
        if !ft.is_file() || !self.filter.wants_file(rel, entry.path()) {
            return None;
        }
        Some(entry.into_path())
    }
}

impl Source for FsSource {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        Some(match self.next_path()? {
            Ok(path) => read_file(&path, &self.read),
            Err(e) => Err(e),
        })
    }
}

fn walk_error(e: &walkdir::Error) -> Error {
    // walkdir's message already names the path (and the loop's ancestor for a loop).
    let kind = e.io_error().map_or(io::ErrorKind::Other, io::Error::kind);
    Error::Io(io::Error::new(kind, e.to_string()))
}

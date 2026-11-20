//! Sources and sinks: where files come from and where records go.
//!
//! - [`FsSource`]: walks one or more directory trees lazily and yields each regular file.
//! - [`ListSource`]: reads a manifest (one path per line, or `locator<TAB>path`), for fixed
//!   evaluation splits.
//! - [`JsonlSink`]: writes one [`FileRecord`](quail_core::FileRecord) per line, buffered, to a
//!   file or stdout; [`JsonlReader`] reads such a file back.
//!
//! Both sources share [`ReadOptions`]: how a path becomes a [`Blob`](quail_core::Blob).
//!
//! # Errors never end a source
//!
//! An unreadable file, a directory that cannot be listed or a symlink loop becomes one
//! `Some(Err(..))` item and the walk goes on. The error is an [`Error::Io`](quail_core::Error)
//! whose message starts with the offending path, because the core `Source` contract has no
//! locator on error items.
//!
//! # Large files
//!
//! By default every file is read whole, whatever its size: the engine compares the blob
//! against `Limits::max_bytes` and decides what to do with an oversized file (size and a
//! streaming hash only). Memory is bounded by the engine pulling at its own pace. Set
//! [`ReadOptions::max_file_bytes`] to make the source refuse bigger files itself: they are
//! not opened for reading and become an error item of kind
//! [`std::io::ErrorKind::FileTooLarge`]. With the `mmap` feature, files above a threshold can
//! be memory-mapped instead of read ([`ReadOptions::mmap_min_bytes`]).

mod fs;
mod jsonl;
mod list;
#[cfg(feature = "mmap")]
#[allow(unsafe_code)] // memmap2::Mmap::map is unsafe; see the SAFETY notes in the module
mod mmap;
mod read;

pub use fs::{FsSource, FsSourceBuilder};
pub use jsonl::{JsonlReader, JsonlSink};
pub use list::ListSource;
pub use read::{ReadOptions, read_file};

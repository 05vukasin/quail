//! Memory-mapped reading (feature `mmap`). The only module of the crate that uses `unsafe`.

use std::fs::File;
use std::io;

use memmap2::Mmap;

/// Maps the whole file read-only.
///
/// The mapping is private to the returned value and lives as long as the [`Blob`] that wraps
/// it (and every clone of it); the `File` may be closed right after this call.
///
/// [`Blob`]: quail_core::Blob
pub(crate) fn map(file: &File) -> io::Result<Mmap> {
    // SAFETY: `Mmap::map` is unsafe because the mapped bytes are not owned by this process:
    // if another process truncates the file while it is mapped, touching the lost pages
    // raises SIGBUS (the process dies, no `catch_unwind` can stop it), and if another
    // process writes to it, the bytes change under a `&[u8]` that Rust assumes immutable
    // (analyzers may see torn, inconsistent content). Neither can be prevented from here: a
    // shared lock is advisory only. We accept it because the feature is opt-in, used for
    // corpora that are not being modified during a run (documented in the README and on
    // `ReadOptions::mmap_min_bytes`), and only for files above a size threshold; the default
    // build reads files into owned memory and has no such risk.
    unsafe { Mmap::map(file) }
}

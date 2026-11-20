# quail-io

Where files come from and where records go. Implements quail-core's `Source` and `RecordSink`.
Emits no facts and has no analyzers or extractors.

| Type | Role |
| --- | --- |
| `FsSource` | Lazy walk over one or more roots (files or directories), yields every regular file. |
| `ListSource` | Files named by a manifest: `path` or `locator<TAB>path` per line, `#` comments. For fixed eval splits. |
| `ReadOptions` / `read_file` | How a path becomes a `Blob`: whole read (default), size cap, optional memory map. |
| `JsonlSink` | One `FileRecord` per line, 256 KiB buffer, to a file, stdout or any writer. `finish()` flushes. |
| `JsonlReader` | Reads JSONL records back (eval, tests). |

## FsSource options

`follow_symlinks` (off: symlinks skipped; on: loops yield one error), `include` / `exclude` globs
(matched against the path relative to the root, `*` crosses `/`; an excluded directory is not
descended into), `extension` (case-insensitive), `skip_hidden`, `max_depth`, `sorted`.

`sorted(true)` gives a reproducible order (roots sorted and deduplicated, entries sorted by
file name, byte order). It stays lazy: the walk holds the listing of each directory on the
current path, about 100 bytes per entry (a flat directory of 1M files: ~100 MB while it is
walked). Unsorted mode holds even less.

## Errors and limits

Nothing ends a source except running out of input. An unreadable file, a directory that cannot
be listed, a missing root, a symlink loop or a bad manifest line is one `Err` item: an
`Error::Io` whose message starts with the path (or manifest line), keeping the I/O error kind.

Large files: by default every file is read whole and the engine applies `Limits::max_bytes`
(memory is bounded by the engine's in-flight limit). `ReadOptions::max_file_bytes(Some(cap))`
makes the source refuse bigger files itself, without reading them, as an error of kind
`FileTooLarge`; a file that grows past the cap while being read is refused too.

## Features

- `mmap`: `ReadOptions::mmap_min_bytes(Some(n))` maps files of at least `n` bytes instead of
  reading them. Uses `unsafe` (memmap2) in `src/mmap.rs` only: if another process truncates a
  mapped file the process gets SIGBUS, and if it writes to it the bytes change under the
  analyzers. Use only on corpora that are not modified during a run.
- `zstd`: `JsonlSink::zstd` / `create_zstd` write a compressed stream; `JsonlReader::open`
  detects it. Links the C zstd library, hence not default.

## Numbers

`cargo bench -p quail-io --features mmap` (`benches/walk.rs`), warm page cache, laptop CPU:

| Case | Time | Throughput |
| --- | --- | --- |
| 10k files × 2 KiB, walk only (`next_path`) | 8.2 ms | 1.22 M files/s |
| 10k files, walk + read | 70.6 ms | 142 k files/s |
| 10k files, walk + read, sorted | 76.7 ms | 130 k files/s |
| 10k files, walk + read, extension + exclude filters | 65.2 ms | 153 k files/s (9k yielded) |
| 10k files, walk + mmap every file | 106.8 ms | 94 k files/s |
| 32 × 4 MiB, read + touch | 32.5 ms | 3.8 GiB/s |
| 32 × 4 MiB, mmap + touch | 24.6 ms | 5.1 GiB/s |

Mapping small files is 1.5× slower than reading them; mapping multi-MiB files is ~25% faster
and avoids a heap copy. A threshold around 1 MiB is a reasonable `mmap_min_bytes`.

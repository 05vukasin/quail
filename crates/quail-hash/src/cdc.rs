//! Content-defined chunking (FastCDC 2020) with a content hash per chunk.

use fastcdc::v2020::{self, FastCDC, Normalization};

/// Chunk size bounds for FastCDC. Changing them changes every chunk hash, so they are part
/// of the analyzer version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkParams {
    min: u32,
    avg: u32,
    max: u32,
}

impl ChunkParams {
    /// Parameters for business documents (10 KB–2 MB): min 2 KiB, avg 8 KiB, max 64 KiB.
    ///
    /// Measured with `examples/chunk_params.rs` (synthetic text, random and mixed files, 200
    /// random small edits each): every candidate from 4 KiB to 32 KiB average changes 1.0–1.2
    /// chunks per edit, so the choice trades the bytes those chunks cover against the hashes
    /// a record carries. Per 1 MB file, 4 KiB avg: 200 hashes, 0.7 % of bytes changed;
    /// 8 KiB: 100 hashes, 1.3 %; 16 KiB: 50 hashes, 2.4 %; 32 KiB: 26 hashes, 4.8 %. On a
    /// real document corpus the share of bytes in chunks shared between files barely moves
    /// (36.9 / 34.8 / 32.5 / 30.7 %), so 8 KiB keeps most of the dedup signal at half the
    /// hashes of 4 KiB, and a 100 KB file still splits into about ten chunks.
    pub const DOCUMENT: Self = Self {
        min: 2 << 10,
        avg: 8 << 10,
        max: 64 << 10,
    };

    /// Custom bounds. `None` unless `min <= avg <= max`, all are even and within FastCDC's
    /// ranges (min 64 B–1 MiB, avg 256 B–4 MiB, max 1 KiB–16 MiB).
    pub const fn new(min: u32, avg: u32, max: u32) -> Option<Self> {
        let (lo, mid, hi) = (min as usize, avg as usize, max as usize);
        let ok = min <= avg
            && avg <= max
            && min.is_multiple_of(2)
            && avg.is_multiple_of(2)
            && max.is_multiple_of(2)
            && lo >= v2020::MINIMUM_MIN
            && lo <= v2020::MINIMUM_MAX
            && mid >= v2020::AVERAGE_MIN
            && mid <= v2020::AVERAGE_MAX
            && hi >= v2020::MAXIMUM_MIN
            && hi <= v2020::MAXIMUM_MAX;
        if ok {
            Some(Self { min, avg, max })
        } else {
            None
        }
    }

    /// Smallest chunk (except the last).
    pub const fn min(self) -> u32 {
        self.min
    }

    /// Target average chunk size.
    pub const fn avg(self) -> u32 {
        self.avg
    }

    /// Largest chunk.
    pub const fn max(self) -> u32 {
        self.max
    }

    pub(crate) fn chunker(self, bytes: &[u8]) -> FastCDC<'_> {
        FastCDC::with_level(
            bytes,
            self.min as usize,
            self.avg as usize,
            self.max as usize,
            Normalization::Level1,
        )
    }
}

impl Default for ChunkParams {
    fn default() -> Self {
        Self::DOCUMENT
    }
}

/// One content-defined chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chunk {
    /// First byte in the input.
    pub offset: u64,
    /// Length in bytes.
    pub len: u32,
    /// XXH3-64 of the chunk's bytes. Unlike FastCDC's own gear value (which only reflects
    /// the last bytes before the cut), it depends on the whole chunk.
    pub hash: u64,
}

impl Chunk {
    pub(crate) fn of(offset: usize, data: &[u8]) -> Self {
        Self {
            offset: offset as u64,
            // A chunk is at most `max` (<= 16 MiB) bytes.
            len: u32::try_from(data.len()).unwrap_or(u32::MAX),
            hash: xxhash_rust::xxh3::xxh3_64(data),
        }
    }
}

/// Content-defined chunks of `bytes`, in order. Empty input yields no chunks.
pub fn chunks(bytes: &[u8], params: ChunkParams) -> impl Iterator<Item = Chunk> + '_ {
    params.chunker(bytes).map(move |c| {
        let data = bytes.get(c.offset..c.offset + c.length).unwrap_or_default();
        Chunk::of(c.offset, data)
    })
}

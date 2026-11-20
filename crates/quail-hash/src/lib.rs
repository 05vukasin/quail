//! Identity layer: exact digests and content-defined chunks of the raw bytes.
//!
//! Namespace: `hash.*`. Works on every file regardless of format.
//!
//! The pure API ([`identity`], [`Digester`], [`chunks`]) is usable without the analyzer:
//!
//! ```
//! use quail_hash::{identity, ChunkParams};
//!
//! let id = identity(b"hello world", ChunkParams::DOCUMENT);
//! assert_eq!(id.chunks.len(), 1);
//! assert_eq!(id.digests.blake3.len(), 32);
//! ```
//!
//! [`IdentityAnalyzer`] (`hash.identity@1`) emits the keys in [`keys`].

mod analyzer;
mod cdc;
mod digest;

pub use analyzer::IdentityAnalyzer;
pub use cdc::{Chunk, ChunkParams, chunks};
pub use digest::{Digester, Digests, blake3, xxh3_128};

/// Every fact key this crate emits.
pub mod keys {
    use quail_core::FactKey;

    /// BLAKE3 digest of the whole file (`Value::Bytes`, 32 bytes).
    pub const BLAKE3: FactKey = FactKey::new("hash.blake3");
    /// XXH3-128 of the whole file (`Value::Bytes`, 16 bytes, canonical big-endian). A fast
    /// lookup key, not collision-resistant against an adversary: use [`BLAKE3`] for that.
    pub const XXH3_128: FactKey = FactKey::new("hash.xxh3_128");
    /// Content-defined chunk hashes in file order (`Value::List` of `Value::U64`, each the
    /// XXH3-64 of one chunk). Two files sharing chunk hashes share those bytes.
    pub const CDC: FactKey = FactKey::new("hash.cdc");
    /// Number of content-defined chunks (`Value::Int`).
    pub const CDC_COUNT: FactKey = FactKey::new("hash.cdc_count");
}

/// Digests and chunks of one byte string, computed in one pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Whole-file digests.
    pub digests: Digests,
    /// Content-defined chunks in order; empty for empty input.
    pub chunks: Vec<Chunk>,
}

/// Digests and chunks of `bytes` in a single pass: the chunker walks the input, each chunk is
/// hashed right after it is found, and every 64 KiB the bytes just walked go to both
/// whole-file hashers while they are still in cache. This beats three separate passes on
/// inputs larger than the cache (+10–20 % at 50 MB) and ties them below; feeding BLAKE3 one
/// chunk at a time instead is ~2x slower (see `benches/identity.rs`).
pub fn identity(bytes: &[u8], params: ChunkParams) -> Identity {
    match identity_checked(bytes, params, || Ok::<(), std::convert::Infallible>(())) {
        Ok(id) => id,
        Err(never) => match never {},
    }
}

/// Like [`identity`], but calls `check` every [`CHECK_EVERY`] chunks and stops with its
/// error (used to honour a deadline on huge inputs).
pub fn identity_checked<E>(
    bytes: &[u8],
    params: ChunkParams,
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<Identity, E> {
    let mut digester = Digester::new();
    let mut out = Vec::with_capacity(bytes.len() / params.avg() as usize + 1);
    // Bytes before `fed` went to the whole-file hashers. They get windows of `WINDOW` bytes
    // that the chunker has just read (so still in cache), never single chunks: BLAKE3 is
    // only fast on long, 1 KiB-aligned updates.
    let mut fed = 0;
    for (i, chunk) in params.chunker(bytes).enumerate() {
        if i % CHECK_EVERY == CHECK_EVERY - 1 {
            check()?;
        }
        let end = chunk.offset + chunk.length;
        let data = bytes.get(chunk.offset..end).unwrap_or_default();
        out.push(Chunk::of(chunk.offset, data));
        if end - fed >= WINDOW {
            let upto = fed + (end - fed) / WINDOW * WINDOW;
            digester.update(bytes.get(fed..upto).unwrap_or_default());
            fed = upto;
        }
    }
    digester.update(bytes.get(fed..).unwrap_or_default());
    Ok(Identity {
        digests: digester.finalize(),
        chunks: out,
    })
}

/// Window of the fused pass: large enough for BLAKE3's SIMD, small enough to stay in L2.
const WINDOW: usize = 64 << 10;

/// How many chunks [`identity_checked`] processes between two `check` calls (about 2 MiB
/// with [`ChunkParams::DOCUMENT`]).
pub const CHECK_EVERY: usize = 256;

#[cfg(test)]
mod tests;

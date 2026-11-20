//! Whole-file digests: BLAKE3 (identity) and XXH3-128 (fast key).

use xxhash_rust::xxh3::Xxh3;

/// BLAKE3-256 of `bytes`. SIMD is picked at runtime (SSE4.1/AVX2 on `x86_64`, NEON on `aarch64`).
pub fn blake3(bytes: &[u8]) -> [u8; 32] {
    *::blake3::hash(bytes).as_bytes()
}

/// XXH3-128 of `bytes` (seed 0) in canonical big-endian byte order, as `xxhsum -H2` prints it.
pub fn xxh3_128(bytes: &[u8]) -> [u8; 16] {
    xxhash_rust::xxh3::xxh3_128(bytes).to_be_bytes()
}

/// Both whole-file digests of one byte string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Digests {
    /// BLAKE3-256.
    pub blake3: [u8; 32],
    /// XXH3-128, canonical big-endian.
    pub xxh3_128: [u8; 16],
}

/// Streaming BLAKE3 + XXH3-128: feed the bytes in any number of pieces, get the same digests
/// as hashing them at once.
#[derive(Clone)]
pub struct Digester {
    blake3: ::blake3::Hasher,
    xxh3: Box<Xxh3>,
}

impl Digester {
    /// Fresh state.
    pub fn new() -> Self {
        Self {
            blake3: ::blake3::Hasher::new(),
            xxh3: Box::new(Xxh3::new()),
        }
    }

    /// Absorbs the next piece of input.
    pub fn update(&mut self, bytes: &[u8]) {
        self.blake3.update(bytes);
        self.xxh3.update(bytes);
    }

    /// Digests of everything absorbed so far.
    pub fn finalize(&self) -> Digests {
        Digests {
            blake3: *self.blake3.finalize().as_bytes(),
            xxh3_128: self.xxh3.digest128().to_be_bytes(),
        }
    }
}

impl Default for Digester {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Digester {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Digester").finish_non_exhaustive()
    }
}

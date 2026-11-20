//! `hash.identity@1`: digests and chunks of every file.

use quail_core::{Analyzer, AnalyzerId, Ctx, Emitter, Profile, Result, Value};

use crate::{ChunkParams, identity_checked, keys};

/// Emits [`keys::BLAKE3`], [`keys::XXH3_128`], [`keys::CDC`] and [`keys::CDC_COUNT`] for every
/// file, whatever its format.
///
/// Files larger than `Limits::max_bytes` are hashed too: the engine only hands over a blob it
/// has already read, hashing runs at memory speed with constant extra memory, and the
/// identity of an oversized file is exactly what deduplication still needs. A deadline is
/// checked every [`CHECK_EVERY`](crate::CHECK_EVERY) chunks.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityAnalyzer;

impl IdentityAnalyzer {
    /// Analyzer id. Version 1 = BLAKE3, XXH3-128 (seed 0), FastCDC 2020 level 1 with
    /// [`ChunkParams::DOCUMENT`] and XXH3-64 chunk hashes. Any change bumps it.
    pub const ID: AnalyzerId = AnalyzerId::new("hash.identity", 1);

    /// Chunking parameters of this version.
    pub const PARAMS: ChunkParams = ChunkParams::DOCUMENT;
}

impl Analyzer for IdentityAnalyzer {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let id = identity_checked(ctx.blob.as_bytes(), Self::PARAMS, || out.check_deadline())?;
        out.fact(keys::BLAKE3, Value::Bytes(id.digests.blake3.into()));
        out.fact(keys::XXH3_128, Value::Bytes(id.digests.xxh3_128.into()));
        let count = i64::try_from(id.chunks.len()).unwrap_or(i64::MAX);
        out.fact(
            keys::CDC,
            Value::List(id.chunks.iter().map(|c| Value::U64(c.hash)).collect()),
        );
        out.fact(keys::CDC_COUNT, Value::Int(count));
        Ok(())
    }
}

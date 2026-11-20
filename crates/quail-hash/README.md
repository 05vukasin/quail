# quail-hash

Identity layer of quail: exact digests and content-defined chunks of a file's raw bytes.
Works on every file, whatever its format. Namespace `hash.*`.

## Analyzer

`hash.identity@1` (`IdentityAnalyzer`, profile `Fast`, applies to all formats):

| key              | value                     | meaning                                         |
|------------------|---------------------------|-------------------------------------------------|
| `hash.blake3`    | `Bytes` (32)              | BLAKE3-256 of the file                          |
| `hash.xxh3_128`  | `Bytes` (16)              | XXH3-128, seed 0, canonical big-endian; fast key |
| `hash.cdc`       | `List` of `U64`           | XXH3-64 of each FastCDC chunk, in file order    |
| `hash.cdc_count` | `Int`                     | number of chunks (0 for an empty file)          |

Chunking: FastCDC 2020, normalization level 1, min 2 KiB / avg 8 KiB / max 64 KiB
(`ChunkParams::DOCUMENT`; measurement in `examples/chunk_params.rs`). The parameters are
part of the analyzer version: changing them bumps `@1`.

## Limits

Files larger than `Limits::max_bytes` are still hashed when the engine hands over their
blob: hashing is memory-speed with constant extra memory, and the identity of an oversized
file is what deduplication needs most. The deadline is checked every 256 chunks (~2 MiB);
on timeout no facts are emitted.

## Pure API

`identity` / `identity_checked` (one fused pass), `Digester` (streaming BLAKE3 + XXH3-128),
`blake3`, `xxh3_128`, `chunks`, `ChunkParams`.

## Speed

`cargo bench -p quail-hash` (criterion, synthetic text+binary input). On a laptop CPU
(AVX2, no AVX-512), single thread: ~1.45 GiB/s for the full identity at 100 KB and 1 MB,
~1.3 GiB/s at 50 MB. CDC is the bottleneck (~2.4 GiB/s); BLAKE3 ~3.5 GiB/s; XXH3 ~25 GiB/s.

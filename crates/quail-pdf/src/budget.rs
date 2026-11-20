//! The `max_decompressed` guard.
//!
//! hayro inflates streams without a size cap, so before a page is interpreted the streams it
//! will decode (content streams, form `XObjects` one level down, embedded font programs) are
//! inflated once with a bounded decoder and charged to the file's budget. A page whose
//! streams would exceed the remaining budget is not interpreted.
//!
//! Deflate expands at most ~1032:1, so when the whole file times that ratio fits in the
//! budget no stream can exceed it and the check is skipped (the common case for files below
//! ~1 MB with the default 1 GiB budget). Images are never decoded by the text device, so they
//! are not charged.

use std::collections::HashSet;

use hayro_syntax::Filter;
use hayro_syntax::object::{Array, Dict, Name, ObjectIdentifier, Stream};
use hayro_syntax::page::Page;
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// Worst-case deflate expansion ratio.
const MAX_DEFLATE_RATIO: u64 = 1032;

/// Bytes of decompression a file may still spend.
pub(crate) struct Budget {
    remaining: u64,
    /// Streams already charged: shared fonts and forms are decoded once per file in spirit,
    /// and charging them per page would exhaust the budget on long documents.
    seen: HashSet<ObjectIdentifier>,
    /// The budget cannot be exceeded by single-layer deflate on this file at all.
    trivially_safe: bool,
}

impl Budget {
    pub fn new(max_decompressed: u64, file_len: usize) -> Self {
        let worst = (file_len as u64).saturating_mul(MAX_DEFLATE_RATIO);
        Self {
            remaining: max_decompressed,
            seen: HashSet::new(),
            trivially_safe: worst <= max_decompressed,
        }
    }

    /// Charges the streams `page` will decode. `false` when the budget is exhausted.
    pub fn charge_page(&mut self, page: &Page<'_>) -> bool {
        let mut streams: Vec<Stream<'_>> = Vec::new();
        let raw = page.raw();
        if let Some(s) = raw.get::<Stream<'_>>(b"Contents") {
            streams.push(s);
        } else if let Some(a) = raw.get::<Array<'_>>(b"Contents") {
            streams.extend(a.iter::<Stream<'_>>());
        }
        let res = page.resources();
        for key in res.x_objects.keys() {
            if let Some(x) = res.x_objects.get::<Stream<'_>>(&*key)
                && x.dict()
                    .get::<Name<'_>>(b"Subtype")
                    .is_some_and(|n| n.as_str() == "Form")
            {
                streams.push(x);
            }
        }
        for key in res.fonts.keys() {
            if let Some(font) = res.fonts.get::<Dict<'_>>(&*key) {
                font_programs(&font, &mut streams);
            }
        }
        streams.iter().all(|s| self.charge(s))
    }

    fn charge(&mut self, s: &Stream<'_>) -> bool {
        let filters = s.filters();
        let deflates = filters
            .iter()
            .filter(|f| matches!(f, Filter::FlateDecode | Filter::LzwDecode))
            .count();
        if self.trivially_safe && deflates <= 1 {
            return true;
        }
        if !self.seen.insert(s.obj_id()) {
            return true;
        }
        let raw = s.raw_data();
        let produced = match filters.as_slice() {
            // Only a lone FlateDecode is measured exactly; other chains are charged at the
            // worst-case ratio, which is conservative.
            [Filter::FlateDecode] => bounded_inflate(&raw, self.remaining.saturating_add(1)),
            [] => raw.len() as u64,
            _ => (raw.len() as u64).saturating_mul(
                MAX_DEFLATE_RATIO.pow(u32::try_from(deflates.max(1)).unwrap_or(u32::MAX).min(3)),
            ),
        };
        if produced > self.remaining {
            self.remaining = 0;
            return false;
        }
        self.remaining -= produced;
        true
    }
}

fn font_programs<'a>(font: &Dict<'a>, out: &mut Vec<Stream<'a>>) {
    let descriptor = font.get::<Dict<'_>>(b"FontDescriptor").or_else(|| {
        // Type0: the descriptor sits on the descendant CID font.
        font.get::<Array<'_>>(b"DescendantFonts")
            .and_then(|a| a.iter::<Dict<'_>>().next())
            .and_then(|d| d.get::<Dict<'_>>(b"FontDescriptor"))
    });
    if let Some(d) = descriptor {
        for k in [&b"FontFile"[..], b"FontFile2", b"FontFile3"] {
            if let Some(s) = d.get::<Stream<'_>>(k) {
                out.push(s);
            }
        }
    }
}

/// Inflates zlib (or raw deflate) data, stopping once `cap` bytes were produced. Returns the
/// number of bytes produced (at most `cap`).
pub(crate) fn bounded_inflate(data: &[u8], cap: u64) -> u64 {
    let zlib = data.len() >= 2
        && data.first().is_some_and(|b| b & 0x0F == 8)
        && data
            .get(..2)
            .is_some_and(|h| (u16::from(h[0]) << 8 | u16::from(h[1])) % 31 == 0);
    // All input is present (no HAS_MORE_INPUT) and the output buffer wraps.
    let flags = if zlib {
        inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
    } else {
        0
    };
    let mut state = DecompressorOxide::new();
    // Wrapping 32 KiB window: we only count bytes, we never keep them.
    let mut window = vec![0u8; 32 * 1024];
    let mut pos = 0usize;
    let mut input = data;
    let mut produced: u64 = 0;
    loop {
        let (status, used_in, written) = decompress(&mut state, input, &mut window, pos, flags);
        input = input.get(used_in..).unwrap_or(&[]);
        produced = produced.saturating_add(written as u64);
        pos = (pos + written) & (window.len() - 1);
        if produced >= cap {
            return cap;
        }
        match status {
            TINFLStatus::HasMoreOutput => {}
            // Done, truncated or corrupt: hayro will see the same bytes, count what came out.
            _ => return produced,
        }
        if used_in == 0 && written == 0 {
            return produced;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    #[test]
    fn inflate_counts_and_caps() {
        let big = vec![b'a'; 3_000_000];
        let z = compress_to_vec_zlib(&big, 9);
        assert!(z.len() < 20_000);
        assert_eq!(bounded_inflate(&z, u64::MAX), 3_000_000);
        assert_eq!(bounded_inflate(&z, 1_000), 1_000);
    }

    #[test]
    fn garbage_does_not_hang() {
        assert!(bounded_inflate(&[0x78, 0x9c, 0xff, 0xff, 0xff], 1 << 20) < 1 << 20);
        assert_eq!(bounded_inflate(&[], 10), 0);
    }
}

//! Zip packages taken apart and written back by hand, so the writer can lie: wrong sizes and
//! CRCs, duplicate names, unknown methods, a directory that disagrees with the local headers.

use std::io::{Cursor, Read};

/// How an entry is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Method 0.
    Store,
    /// Method 8 (raw deflate).
    Deflate,
    /// Data deflated but the header says this method number.
    Claim(u16),
}

/// A header lie for one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lie {
    /// Honest headers.
    #[default]
    None,
    /// Uncompressed size in both headers replaced by this value.
    Size(u32),
    /// Compressed size replaced by this value.
    CompSize(u32),
    /// CRC-32 flipped.
    Crc,
    /// The central directory names the entry differently from its local header.
    CentralName,
}

/// One entry to write.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Name as stored (any bytes).
    pub name: Vec<u8>,
    /// Uncompressed data.
    pub data: Vec<u8>,
    /// Storage method.
    pub method: Method,
    /// Header lie.
    pub lie: Lie,
    /// Already-compressed bytes to store verbatim (a precomputed bomb); `data` is then only
    /// used for the CRC and the declared size.
    pub raw: Option<(Vec<u8>, u32, u32)>,
}

impl Entry {
    /// An honest deflated entry.
    pub fn new(name: impl Into<Vec<u8>>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            data: data.into(),
            method: Method::Deflate,
            lie: Lie::None,
            raw: None,
        }
    }
}

/// Lies about the end-of-central-directory record.
#[derive(Clone, Copy, Debug, Default)]
pub struct Eocd {
    /// Entry count replaced by this value.
    pub count: Option<u16>,
    /// Directory offset shifted by this many bytes.
    pub offset_delta: i64,
}

/// Entries of a zip package, decompressed, at most `cap` bytes each (larger entries are cut).
/// `None` when the bytes are not a readable zip.
pub fn read(bytes: &[u8], cap: u64) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len().min(4096) {
        let Ok(mut f) = z.by_index(i) else { continue };
        let name = f.name_raw().to_vec();
        let mut data = Vec::new();
        if (&mut f).take(cap).read_to_end(&mut data).is_err() {
            continue;
        }
        out.push((name, data));
    }
    if out.is_empty() { None } else { Some(out) }
}

/// CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// Raw deflate of `data`.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 6)
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// Writes a zip of `entries` with the given lies. Sizes over 4 GiB are not representable and
/// are truncated in the headers (which is just one more lie).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
pub fn write(entries: &[Entry], eocd: Eocd, prefix: &[u8]) -> Vec<u8> {
    let mut out = prefix.to_vec();
    let mut central = Vec::new();
    for e in entries {
        let (body, crc, size) = if let Some((raw, crc, size)) = &e.raw {
            (raw.clone(), *crc, *size)
        } else {
            let body = match e.method {
                Method::Store => e.data.clone(),
                Method::Deflate | Method::Claim(_) => deflate(&e.data),
            };
            (body, crc32(&e.data), e.data.len() as u32)
        };
        let method = match e.method {
            Method::Store => 0,
            Method::Deflate => 8,
            Method::Claim(m) => m,
        };
        let (mut crc, mut usize_, mut csize) = (crc, size, body.len() as u32);
        match e.lie {
            Lie::None | Lie::CentralName => {}
            Lie::Size(s) => usize_ = s,
            Lie::CompSize(s) => csize = s,
            Lie::Crc => crc = !crc,
        }
        let offset = out.len() as u32;
        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, 20);
        u16le(&mut out, 0);
        u16le(&mut out, method);
        u16le(&mut out, 0);
        u16le(&mut out, 0x21);
        u32le(&mut out, crc);
        u32le(&mut out, csize);
        u32le(&mut out, usize_);
        u16le(&mut out, e.name.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&body);

        let cname: Vec<u8> = if e.lie == Lie::CentralName {
            let mut n = e.name.clone();
            n.extend_from_slice(b".x");
            n
        } else {
            e.name.clone()
        };
        u32le(&mut central, 0x0201_4b50);
        u16le(&mut central, 20);
        u16le(&mut central, 20);
        u16le(&mut central, 0);
        u16le(&mut central, method);
        u16le(&mut central, 0);
        u16le(&mut central, 0x21);
        u32le(&mut central, crc);
        u32le(&mut central, csize);
        u32le(&mut central, usize_);
        u16le(&mut central, cname.len() as u16);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u32le(&mut central, 0);
        u32le(&mut central, offset);
        central.extend_from_slice(&cname);
    }
    let cd_offset = out.len() as i64 + eocd.offset_delta;
    let count = eocd.count.unwrap_or(entries.len() as u16);
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, count);
    u16le(&mut out, count);
    u32le(&mut out, cd_len);
    u32le(&mut out, cd_offset.clamp(0, i64::from(u32::MAX)) as u32);
    u16le(&mut out, 0);
    out
}

/// A deflate stream of `n` zero bytes, built in 1 MiB steps (a zip / flate bomb body).
pub fn zero_bomb(n: usize) -> Vec<u8> {
    use miniz_oxide::deflate::core::{
        CompressorOxide, TDEFLFlush, TDEFLStatus, compress, create_comp_flags_from_zip_params,
    };
    let mut c = CompressorOxide::new(create_comp_flags_from_zip_params(6, -15, 0));
    let chunk = vec![0u8; 1 << 20];
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut left = n;
    loop {
        let take = left.min(chunk.len());
        left -= take;
        let flush = if left == 0 {
            TDEFLFlush::Finish
        } else {
            TDEFLFlush::None
        };
        let mut input = &chunk[..take];
        loop {
            let (status, used, written) = compress(&mut c, input, &mut buf, flush);
            out.extend_from_slice(&buf[..written]);
            input = &input[used..];
            match status {
                TDEFLStatus::Okay if input.is_empty() && written < buf.len() => break,
                TDEFLStatus::Okay => {}
                _ => return out,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honest_zip_round_trips() {
        let es = vec![
            Entry::new("a.xml", b"<a/>".to_vec()),
            Entry {
                method: Method::Store,
                ..Entry::new("b/c.txt", b"hello".to_vec())
            },
        ];
        let z = write(&es, Eocd::default(), b"");
        let back = read(&z, 1 << 20).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], (b"a.xml".to_vec(), b"<a/>".to_vec()));
        assert_eq!(back[1], (b"b/c.txt".to_vec(), b"hello".to_vec()));
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn bomb_inflates_to_zeros() {
        let b = zero_bomb((3 << 20) + 17);
        assert!(b.len() < 20_000);
        let back = miniz_oxide::inflate::decompress_to_vec(&b).unwrap();
        assert_eq!(back.len(), (3 << 20) + 17);
        assert!(back.iter().all(|&x| x == 0));
    }
}

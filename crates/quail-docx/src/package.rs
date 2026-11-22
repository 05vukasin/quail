//! The OPC package (the zip), read defensively.
//!
//! Nothing the archive declares is trusted: entry names are checked for path tricks, and the
//! bytes every part actually inflates to are counted against one budget per file
//! ([`Limits::max_decompressed`]). Reading stops at the first byte over the budget, whatever
//! the entry's declared size said.

use std::collections::HashMap;
use std::io::{self, Cursor, Read};

use quail_core::{Emitter, Error, Format, Limits, Result};
use zip::ZipArchive;

/// Most zip entries accepted. Real DOCX files have tens to a few hundred.
pub(crate) const MAX_ENTRIES: usize = 20_000;

/// Message of the I/O error a part reader returns once the budget is spent.
pub(crate) const BUDGET_MSG: &str = "decompression budget exceeded";

/// Magic of an OLE compound file: what an encrypted (password protected) OOXML file is.
const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The zip of one DOCX file plus the decompression budget shared by all its parts.
pub(crate) struct Package<'a> {
    zip: ZipArchive<Cursor<&'a [u8]>>,
    /// Lowercased part name (no leading `/`) to entry index. OPC part names are
    /// case-insensitive.
    index: HashMap<String, usize>,
    /// Part names as written, in archive order (accepted entries only).
    names: Vec<String>,
    budget: u64,
    used: u64,
    exhausted: bool,
    /// Entries whose name repeats an earlier one (exactly or by case).
    pub duplicates: usize,
    /// Problems found while reading the package.
    pub warnings: Vec<String>,
}

impl<'a> Package<'a> {
    /// Opens the archive and vets its directory.
    pub fn open(bytes: &'a [u8], limits: &Limits, deadline: &Emitter) -> Result<Self> {
        if bytes.starts_with(&CFB_MAGIC) {
            // Encrypted OOXML is an OLE container holding the encrypted package.
            return Err(Error::Encrypted);
        }
        // Refuse a huge directory before the zip crate parses (and allocates) all of it.
        let raw_count = match declared_entries(bytes) {
            Some((count, cd_size)) => {
                if count > MAX_ENTRIES as u64 || cd_size / 46 > MAX_ENTRIES as u64 {
                    return Err(Error::LimitExceeded("zip entries"));
                }
                usize::try_from(count).unwrap_or(0)
            }
            None => 0,
        };
        let zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Malformed {
            format: Format::Docx,
            msg: format!("zip: {e}"),
        })?;
        if zip.len() > MAX_ENTRIES {
            return Err(Error::LimitExceeded("zip entries"));
        }
        let meta = zip.metadata();
        let mut index = HashMap::with_capacity(zip.len());
        let mut names = Vec::with_capacity(zip.len());
        let mut warnings = Vec::new();
        let mut rejected = 0usize;
        // The zip crate keys entries by name: of exactly repeated names it keeps the LAST
        // entry. Only the count survives, from the directory's own entry count.
        let exact_dupes = raw_count.saturating_sub(zip.len());
        let mut duplicates = 0usize;
        let mut declared: u128 = 0;
        for i in 0..zip.len() {
            if i % 1024 == 1023 {
                deadline.check_deadline()?;
            }
            let Ok(entry) = meta.entry(i) else {
                rejected += 1;
                continue;
            };
            let Ok(name) = entry.name() else {
                rejected += 1;
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            if !safe_name(&name) || entry.enclosed_name().is_none() {
                rejected += 1;
                continue;
            }
            declared += u128::from(entry.size());
            let key = name.to_ascii_lowercase();
            if index.contains_key(&key) {
                duplicates += 1;
                continue;
            }
            index.insert(key, i);
            names.push(name.into_owned());
        }
        if rejected > 0 {
            warnings.push(format!(
                "{rejected} zip entries with unsafe or unreadable names ignored"
            ));
        }
        if exact_dupes > 0 {
            warnings.push(format!(
                "{exact_dupes} zip entries repeat an earlier name exactly (the last one is read)"
            ));
        }
        if duplicates > 0 {
            warnings.push(format!(
                "{duplicates} zip entries repeat an earlier name in another case (the first one \
                 is read)"
            ));
        }
        let duplicates = duplicates + exact_dupes;
        if declared > u128::from(limits.max_decompressed) {
            warnings.push(format!(
                "zip declares {declared} uncompressed bytes, over the {} byte budget",
                limits.max_decompressed
            ));
        }
        Ok(Self {
            zip,
            index,
            names,
            budget: limits.max_decompressed,
            used: 0,
            exhausted: false,
            duplicates,
            warnings,
        })
    }

    /// The decompression budget is spent: no part opens any more.
    pub fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// Number of entries in the zip directory (all of them, rejected ones included).
    pub fn entry_count(&self) -> usize {
        self.zip.len()
    }

    /// Accepted part names, in archive order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether a part exists (case-insensitive, leading `/` ignored).
    pub fn has(&self, name: &str) -> bool {
        self.index.contains_key(&normalize_key(name))
    }

    /// A reader over the inflated bytes of a part, or `None` when it is missing, encrypted,
    /// unreadable, or would not fit in what is left of the budget.
    pub fn open_part(&mut self, name: &str) -> Option<PartReader<'_, 'a>> {
        if self.exhausted {
            return None;
        }
        let &idx = self.index.get(&normalize_key(name))?;
        let Self {
            zip,
            used,
            exhausted,
            budget,
            warnings,
            ..
        } = self;
        let file = match zip.by_index(idx) {
            Ok(f) => f,
            Err(e) => {
                warnings.push(format!("part {name}: {e}"));
                return None;
            }
        };
        if file.encrypted() {
            warnings.push(format!("part {name} is encrypted, skipped"));
            return None;
        }
        // The declared size is only reported, never trusted: the reader counts real bytes.
        if file.size() > budget.saturating_sub(*used) {
            warnings.push(format!(
                "part {name} declares {} bytes, over the remaining decompression budget",
                file.size()
            ));
        }
        Some(PartReader {
            file,
            used,
            cap: *budget,
            exhausted,
        })
    }
}

/// Reads one part, counting the bytes it really inflates to against the file's budget.
pub(crate) struct PartReader<'p, 'a> {
    file: zip::read::ZipFile<'p, Cursor<&'a [u8]>>,
    used: &'p mut u64,
    cap: u64,
    exhausted: &'p mut bool,
}

impl Read for PartReader<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.cap.saturating_sub(*self.used);
        if remaining == 0 {
            // Budget spent: any further byte is one too many.
            let mut probe = [0u8; 1];
            return if self.file.read(&mut probe)? == 0 {
                Ok(0)
            } else {
                *self.exhausted = true;
                Err(io::Error::other(BUDGET_MSG))
            };
        }
        let n = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let got = self.file.read(&mut buf[..n])?;
        *self.used += got as u64;
        Ok(got)
    }
}

/// Entry count and central directory size declared by the end-of-central-directory record
/// (and the ZIP64 record when present), read without parsing the directory.
fn declared_entries(b: &[u8]) -> Option<(u64, u64)> {
    // Offsets come from the file (the ZIP64 locator holds a u64): never add unchecked.
    let at = |i: usize, n: usize| i.checked_add(n).and_then(|end| b.get(i..end));
    let u16_at = |i: usize| {
        at(i, 2)
            .and_then(|x| <[u8; 2]>::try_from(x).ok())
            .map(|x| u64::from(u16::from_le_bytes(x)))
    };
    let u32_at = |i: usize| {
        at(i, 4)
            .and_then(|x| <[u8; 4]>::try_from(x).ok())
            .map(|x| u64::from(u32::from_le_bytes(x)))
    };
    let u64_at = |i: usize| {
        at(i, 8)
            .and_then(|x| <[u8; 8]>::try_from(x).ok())
            .map(u64::from_le_bytes)
    };
    let lowest = b.len().saturating_sub(22 + 0xFFFF);
    let mut i = b.len().checked_sub(22)?;
    while at(i, 4)? != [0x50, 0x4b, 0x05, 0x06] {
        if i <= lowest {
            return None;
        }
        i -= 1;
    }
    let mut count = u16_at(i + 10)?;
    let mut size = u32_at(i + 12)?;
    if let Some(loc) = i.checked_sub(20)
        && at(loc, 4) == Some(&[0x50, 0x4b, 0x06, 0x07][..])
        && let Some(off) = u64_at(loc + 8).and_then(|o| usize::try_from(o).ok())
        && at(off, 4) == Some(&[0x50, 0x4b, 0x06, 0x06][..])
    {
        count = count.max(u64_at(off.checked_add(32)?)?);
        size = size.max(u64_at(off.checked_add(40)?)?);
    }
    Some((count, size))
}

/// A part name is usable when it is relative, has no `..` or empty components, no
/// backslashes, drive letters or control characters.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.chars().any(char::is_control)
        && name
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
}

fn normalize_key(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

/// Resolves a relationship target against the folder of its source part, the way OPC does:
/// `../media/a.png` from `word/document.xml` is `word/../media/a.png` → `media/a.png`.
/// Returns `None` for targets that climb above the package root.
pub(crate) fn resolve_target(source_part: &str, target: &str) -> Option<String> {
    let target = percent_decode(target);
    let mut parts: Vec<&str> = Vec::new();
    if !target.starts_with('/')
        && let Some((dir, _)) = source_part.rsplit_once('/')
    {
        parts.extend(dir.split('/').filter(|c| !c.is_empty()));
    }
    for c in target.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Decodes `%XX` escapes (part names in relationships are URIs). Invalid escapes and
/// non-UTF-8 results are left as written.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(safe_name("word/document.xml"));
        assert!(!safe_name("../evil"));
        assert!(!safe_name("word/../../evil"));
        assert!(!safe_name("/abs"));
        assert!(!safe_name("C:/x"));
        assert!(!safe_name("a\\b"));
        assert!(!safe_name("a//b"));
    }

    #[test]
    fn targets() {
        assert_eq!(
            resolve_target("word/document.xml", "media/image1.png").as_deref(),
            Some("word/media/image1.png")
        );
        assert_eq!(
            resolve_target("word/document.xml", "../customXml/item1.xml").as_deref(),
            Some("customXml/item1.xml")
        );
        assert_eq!(
            resolve_target("word/document.xml", "/word/styles.xml").as_deref(),
            Some("word/styles.xml")
        );
        assert_eq!(resolve_target("word/document.xml", "../../x"), None);
        assert_eq!(
            resolve_target("", "word/my%20doc.xml").as_deref(),
            Some("word/my doc.xml")
        );
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%4"), "%4");
    }

    #[test]
    fn a_zip64_locator_pointing_past_the_address_space_is_ignored() {
        for off in [u64::MAX, u64::MAX - 3, u64::MAX - 35] {
            // ZIP64 end-of-central-directory locator, then an empty end-of-central-directory.
            let mut b = vec![0x50, 0x4b, 0x06, 0x07, 0, 0, 0, 0];
            b.extend_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
            b.extend_from_slice(&[0; 18]);
            assert_eq!(declared_entries(&b), Some((0, 0)));
        }
    }
}

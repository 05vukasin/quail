//! Format sniffing from magic numbers and a bounded look inside ZIP containers.
//!
//! Everything here reads at most a few kilobytes from the start of the file plus, for ZIP,
//! the end-of-central-directory record and up to [`MAX_ZIP_ENTRIES`] central-directory
//! entries. It never inflates anything and never panics on any input.

use quail_core::Format;

/// How far into the file `%PDF-` may start (Acrobat accepts junk before the header).
pub const PDF_HEADER_WINDOW: usize = 1024;
/// Bytes inspected by the text heuristic.
pub const TEXT_WINDOW: usize = 8 * 1024;
/// Central-directory (or local-header) entries looked at, at most, to tell DOCX from ZIP.
pub const MAX_ZIP_ENTRIES: usize = 4096;

/// Result of sniffing: the format and the most specific MIME type known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sniff {
    /// Detected format.
    pub format: Format,
    /// MIME type, e.g. `application/pdf`.
    pub mime: &'static str,
}

impl Sniff {
    const fn new(format: Format, mime: &'static str) -> Self {
        Self { format, mime }
    }
}

const PDF: Sniff = Sniff::new(Format::Pdf, "application/pdf");
const DOCX: Sniff = Sniff::new(
    Format::Docx,
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
);
const XLSX: Sniff = Sniff::new(
    Format::Zip,
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
);
const PPTX: Sniff = Sniff::new(
    Format::Zip,
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
);
const ZIP: Sniff = Sniff::new(Format::Zip, "application/zip");
const OCTET: Sniff = Sniff::new(Format::Unknown, "application/octet-stream");

/// Detects the format of `bytes`. Shortcut for `detect(bytes).format`.
pub fn sniff(bytes: &[u8]) -> Format {
    detect(bytes).format
}

/// Detects format and MIME type of `bytes`.
pub fn detect(bytes: &[u8]) -> Sniff {
    if bytes.is_empty() {
        return Sniff::new(Format::Unknown, "application/x-empty");
    }
    if bytes.starts_with(b"%PDF-") && pdf_version(bytes.get(5..8)) {
        return PDF;
    }
    if let Some(s) = image(bytes) {
        return s;
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return zip(bytes);
    }
    if let Some(s) = other_binary(bytes) {
        return s;
    }
    if pdf_header(&bytes[..bytes.len().min(PDF_HEADER_WINDOW)]) {
        return PDF;
    }
    if bytes.starts_with(b"{\\rtf") {
        // RTF is ASCII but markup, not prose: keep text analyzers off it.
        return Sniff::new(Format::Unknown, "application/rtf");
    }
    text(bytes).unwrap_or(OCTET)
}

fn image(b: &[u8]) -> Option<Sniff> {
    let img = |mime| Some(Sniff::new(Format::Image, mime));
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return img("image/png");
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return img("image/jpeg");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return img("image/gif");
    }
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return img("image/tiff");
    }
    if b.len() >= 12 && b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return img("image/webp");
    }
    if b.starts_with(b"\0\0\0\x0cjP  \r\n\x87\n") {
        return img("image/jp2");
    }
    if b.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return img("image/jp2");
    }
    if b.get(4..8) == Some(b"ftyp") {
        match b.get(8..12) {
            Some(b"avif" | b"avis") => return img("image/avif"),
            Some(b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1") => {
                return img("image/heic");
            }
            _ => {}
        }
    }
    // BMP: "BM", then reserved fields (bytes 6..10) that are zero in every real file.
    if b.len() >= 26 && b.starts_with(b"BM") && b.get(6..10) == Some(&[0, 0, 0, 0]) {
        return img("image/bmp");
    }
    None
}

fn other_binary(b: &[u8]) -> Option<Sniff> {
    let unk = |mime| Some(Sniff::new(Format::Unknown, mime));
    if b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return unk("application/x-ole-storage");
    }
    if b.starts_with(&[0x1F, 0x8B]) {
        return unk("application/gzip");
    }
    if b.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return unk("application/x-7z-compressed");
    }
    if b.starts_with(b"Rar!\x1A\x07") {
        return unk("application/vnd.rar");
    }
    None
}

/// `d.d` after `%PDF-`.
fn pdf_version(v: Option<&[u8]>) -> bool {
    matches!(v, Some([a, b'.', b]) if a.is_ascii_digit() && b.is_ascii_digit())
}

/// `%PDF-d.d` in `head`, preceded only by whitespace or binary junk. A prefix that reads as
/// text (a Markdown note, source code mentioning `%PDF-`) means this is a text file.
fn pdf_header(head: &[u8]) -> bool {
    let mut from = 0;
    while let Some(p) = head.get(from..).and_then(|h| find(h, b"%PDF-")) {
        let at = from + p;
        if pdf_version(head.get(at + 5..at + 8)) {
            return junk_prefix(&head[..at]);
        }
        from = at + 1;
    }
    false
}

fn junk_prefix(prefix: &[u8]) -> bool {
    if prefix.iter().all(u8::is_ascii_whitespace) {
        return true;
    }
    let printable = prefix
        .iter()
        .filter(|&&c| c.is_ascii_graphic() || c.is_ascii_whitespace())
        .count();
    let control = prefix
        .iter()
        .any(|&c| c < 0x20 && !c.is_ascii_whitespace() || c == 0x7F);
    let text = printable * 2 > prefix.len() || (!control && std::str::from_utf8(prefix).is_ok());
    !text
}

/// `memmem` for short needles.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(usize::from(u16::from_le_bytes([s[0], s[1]])))
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    let s = b.get(at..at.checked_add(4)?)?;
    usize::try_from(u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok()
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // independent presence flags, not a state
struct ZipNames {
    content_types: bool,
    word: bool,
    xl: bool,
    ppt: bool,
}

impl ZipNames {
    fn see(&mut self, name: &[u8]) {
        match name {
            b"[Content_Types].xml" => self.content_types = true,
            b"word/document.xml" => self.word = true,
            b"xl/workbook.xml" => self.xl = true,
            b"ppt/presentation.xml" => self.ppt = true,
            _ => {}
        }
    }

    fn done(&self) -> bool {
        self.content_types && (self.word || self.xl || self.ppt)
    }

    fn verdict(&self) -> Sniff {
        // `word/document.xml` is what a DOCX extractor needs; a missing content-types part
        // only means a sloppy writer, so it does not veto DOCX.
        if self.word {
            DOCX
        } else if self.content_types && self.xl {
            XLSX
        } else if self.content_types && self.ppt {
            PPTX
        } else {
            ZIP
        }
    }
}

fn zip(b: &[u8]) -> Sniff {
    let mut names = ZipNames::default();
    if !central_directory(b, &mut names) {
        local_headers(b, &mut names);
    }
    names.verdict()
}

/// Walks the central directory found through the end-of-central-directory record. Returns
/// `false` when there is no usable directory (truncated or ZIP64 file).
fn central_directory(b: &[u8], names: &mut ZipNames) -> bool {
    const EOCD: &[u8] = b"PK\x05\x06";
    const EOCD_LEN: usize = 22;
    /// Signature matches tried, from the end: a comment may contain a fake EOCD.
    const MAX_CANDIDATES: usize = 8;
    if b.len() < EOCD_LEN {
        return false;
    }
    // The EOCD is 22 bytes plus a comment of at most 65535 bytes, at the very end.
    let window_start = b.len().saturating_sub(EOCD_LEN + 0xFFFF);
    let mut end = b.len();
    for _ in 0..MAX_CANDIDATES {
        let Some(rel) = b
            .get(window_start..end)
            .and_then(|w| w.windows(4).rposition(|x| x == EOCD))
        else {
            return false;
        };
        let eocd = window_start + rel;
        end = eocd + 3; // next search strictly before this match
        if let Some((cd, entries)) = directory_at(b, eocd) {
            walk_directory(cd, entries, names);
            return true;
        }
    }
    false
}

/// The central directory an EOCD at `eocd` points to, if it is plausible: in bounds and
/// starting with a directory entry signature. ZIP64 markers are not followed.
fn directory_at(b: &[u8], eocd: usize) -> Option<(&[u8], usize)> {
    let entries = u16_at(b, eocd + 10)?;
    let cd_size = u32_at(b, eocd + 12)?;
    let cd_off = u32_at(b, eocd + 16)?;
    u16_at(b, eocd + 20)?; // the record is complete
    if cd_off == 0xFFFF_FFFF || entries == 0xFFFF {
        return None; // ZIP64: fall back to local headers
    }
    let cd = b.get(cd_off..cd_off.checked_add(cd_size)?)?;
    (entries == 0 || cd.starts_with(b"PK\x01\x02")).then_some((cd, entries))
}

fn walk_directory(cd: &[u8], entries: usize, names: &mut ZipNames) {
    let mut at = 0usize;
    for _ in 0..entries.min(MAX_ZIP_ENTRIES) {
        if cd.get(at..at + 4) != Some(b"PK\x01\x02") {
            break;
        }
        let (Some(n), Some(x), Some(c)) = (
            u16_at(cd, at + 28),
            u16_at(cd, at + 30),
            u16_at(cd, at + 32),
        ) else {
            break;
        };
        let Some(name) = cd.get(at + 46..at + 46 + n) else {
            break;
        };
        names.see(name);
        if names.done() {
            break;
        }
        at += 46 + n + x + c;
    }
}

/// Fallback for truncated archives: follow local file headers from the start. Stops at an
/// entry whose size is only known from a trailing data descriptor.
fn local_headers(b: &[u8], names: &mut ZipNames) {
    let mut at = 0usize;
    for _ in 0..MAX_ZIP_ENTRIES {
        if b.get(at..at + 4) != Some(b"PK\x03\x04") {
            break;
        }
        let (Some(flags), Some(csize), Some(n), Some(x)) = (
            u16_at(b, at + 6),
            u32_at(b, at + 18),
            u16_at(b, at + 26),
            u16_at(b, at + 28),
        ) else {
            break;
        };
        let Some(name) = b.get(at + 30..at + 30 + n) else {
            break;
        };
        names.see(name);
        if names.done() || flags & 0x0008 != 0 || csize == 0xFFFF_FFFF {
            break;
        }
        at = at.saturating_add(30 + n + x).saturating_add(csize);
    }
}

/// Text heuristic over the first [`TEXT_WINDOW`] bytes: no NUL, few control characters, and
/// valid UTF-8 (a multi-byte sequence cut by the window is fine). Single-byte legacy
/// encodings (Windows-125x, Latin-n) pass as text without a charset.
fn text(bytes: &[u8]) -> Option<Sniff> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Some(Sniff::new(Format::Text, "text/plain; charset=utf-16"));
    }
    let w = &bytes[..bytes.len().min(TEXT_WINDOW)];
    let mut control = 0usize;
    for &c in w {
        match c {
            0 => return None,
            b'\t' | b'\n' | b'\r' | 0x0C | 0x1B => {}
            0x01..=0x1F | 0x7F => control += 1,
            _ => {}
        }
    }
    // More than 1 control byte in 100 is binary.
    if control * 100 > w.len() {
        return None;
    }
    let utf8 = match std::str::from_utf8(w) {
        Ok(_) => true,
        // Only a sequence cut at the window end, and only when the window is not the file.
        Err(e) => e.error_len().is_none() && w.len() < bytes.len(),
    };
    Some(if utf8 {
        Sniff::new(Format::Text, "text/plain; charset=utf-8")
    } else {
        Sniff::new(Format::Text, "text/plain")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal stored ZIP with the given entry names (empty contents), with a proper
    /// central directory unless `with_cd` is false.
    pub(crate) fn zip_with(names: &[&str], with_cd: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for name in names {
            let off = u32::try_from(out.len()).unwrap();
            let n = u16::try_from(name.len()).unwrap();
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // ver, flags, method, time, date
            out.extend_from_slice(&[0; 12]); // crc, csize, usize
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());

            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&n.to_le_bytes());
            cd.extend_from_slice(&[0; 12]); // extra, comment, disk, int attr, ext attr
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        if with_cd {
            let cd_off = u32::try_from(out.len()).unwrap();
            let cd_len = u32::try_from(cd.len()).unwrap();
            let count = u16::try_from(names.len()).unwrap();
            out.extend_from_slice(&cd);
            out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&cd_len.to_le_bytes());
            out.extend_from_slice(&cd_off.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    #[test]
    fn pdf_at_start_and_after_junk() {
        assert_eq!(detect(b"%PDF-1.7\n%\xE2\xE3\n1 0 obj"), PDF);
        let mut junk = vec![b' '; 900];
        junk.extend_from_slice(b"%PDF-1.4");
        assert_eq!(sniff(&junk), Format::Pdf);
        let mut far = vec![b'x'; 1100];
        far.extend_from_slice(b"%PDF-1.4");
        assert_eq!(sniff(&far), Format::Text);
        // Binary junk before the header (e.g. a mail-gateway prefix with control bytes).
        let mut bin = vec![0x01, 0xFF, 0x00, 0x13, 0x88, 0x02];
        bin.extend_from_slice(b"%PDF-1.5\n");
        assert_eq!(sniff(&bin), Format::Pdf);
    }

    #[test]
    fn text_mentioning_pdf_header_is_text() {
        let md = b"# Notes\n\nA PDF file starts with `%PDF-1.7` and ends with `%%EOF`.\n";
        assert_eq!(sniff(md), Format::Text);
        let rs = b"fn is_pdf(b: &[u8]) -> bool {\n    b.starts_with(b\"%PDF-1.4\")\n}\n";
        assert_eq!(sniff(rs), Format::Text);
        let cyr = "Документ почиње са %PDF-1.4 заглављем".as_bytes();
        assert_eq!(sniff(cyr), Format::Text);
        // No version digits: not a header, even at offset 0.
        assert_eq!(sniff(b"%PDF-x is how it starts"), Format::Text);
        assert_eq!(sniff(b"%PDF-2.0\n"), Format::Pdf);
    }

    #[test]
    fn fake_eocd_in_comment_falls_back_to_the_real_one() {
        let mut z = zip_with(&["[Content_Types].xml", "word/document.xml"], true);
        // Archive comment holding a bogus EOCD signature pointing nowhere.
        let n = z.len();
        let fake: Vec<u8> = b"PK\x05\x06".iter().copied().chain([0xAB; 18]).collect();
        z[n - 2..n].copy_from_slice(&u16::try_from(fake.len()).unwrap().to_le_bytes());
        z.extend_from_slice(&fake);
        assert_eq!(sniff(&z), Format::Docx);
    }

    #[test]
    fn garbage_central_directory_falls_back_to_local_headers() {
        let mut z = zip_with(&["[Content_Types].xml", "word/document.xml"], true);
        let n = z.len();
        // Point the directory at offset 0 (a local header, not a directory entry).
        z[n - 6..n - 2].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(sniff(&z), Format::Docx);
    }

    /// xorshift64*: deterministic random bytes without a dependency.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed >> 12;
        *seed ^= *seed << 25;
        *seed ^= *seed >> 27;
        seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn rand_index(seed: &mut u64, n: usize) -> usize {
        usize::try_from(rng(seed) % u64::try_from(n).unwrap()).unwrap()
    }

    #[test]
    fn random_bytes_never_panic_and_stay_fast() {
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let prefixes: [&[u8]; 4] = [b"PK\x03\x04", b"PK\x05\x06", b"%PDF-1.", b"RIFF"];
        let docx = zip_with(&["[Content_Types].xml", "word/document.xml", "a"], true);
        let start = std::time::Instant::now();
        let mut slowest = std::time::Duration::ZERO;
        for i in 0..20_000 {
            let len = rand_index(&mut seed, 2048);
            let mut b: Vec<u8> = (0..len).map(|_| rng(&mut seed).to_le_bytes()[0]).collect();
            match i % 4 {
                0 => {}
                1 => {
                    let p = prefixes[i / 4 % prefixes.len()];
                    b.splice(0..0, p.iter().copied());
                }
                2 => {
                    // Sprinkle EOCD / directory signatures.
                    for _ in 0..4 {
                        let at = rand_index(&mut seed, b.len() + 1);
                        b.splice(at..at, b"PK\x05\x06PK\x01\x02".iter().copied());
                    }
                    b.splice(0..0, b"PK\x03\x04".iter().copied());
                }
                _ => {
                    // Mutate a valid DOCX.
                    b.clone_from(&docx);
                    for _ in 0..3 {
                        let at = rand_index(&mut seed, b.len());
                        b[at] = rng(&mut seed).to_le_bytes()[0];
                    }
                }
            }
            let t = std::time::Instant::now();
            let _ = detect(&b);
            slowest = slowest.max(t.elapsed());
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20));
        // One scheduler preemption on a shared CI runner can cost tens of milliseconds, so the
        // per-input bound only catches pathological (unbounded) work, not normal jitter.
        assert!(
            slowest < std::time::Duration::from_secs(1),
            "{slowest:?}"
        );
    }

    #[test]
    fn many_fake_eocds_are_bounded() {
        // 64 KiB of EOCD signatures in the tail: only a few candidates are tried.
        let mut z = zip_with(&["word/document.xml"], false);
        for _ in 0..16_000 {
            z.extend_from_slice(b"PK\x05\x06");
        }
        let t = std::time::Instant::now();
        let _ = sniff(&z);
        assert!(t.elapsed() < std::time::Duration::from_millis(200));
    }

    #[test]
    fn docx_vs_zip() {
        let docx = zip_with(
            &["[Content_Types].xml", "_rels/.rels", "word/document.xml"],
            true,
        );
        assert_eq!(detect(&docx), DOCX);
        let plain = zip_with(&["a.txt", "b/c.txt"], true);
        assert_eq!(detect(&plain), ZIP);
        let xlsx = zip_with(&["[Content_Types].xml", "xl/workbook.xml"], true);
        assert_eq!(detect(&xlsx), XLSX);
        // Content types alone is OOXML of an unknown kind: stays Zip.
        assert_eq!(
            sniff(&zip_with(&["[Content_Types].xml"], true)),
            Format::Zip
        );
    }

    #[test]
    fn truncated_docx_found_through_local_headers() {
        let docx = zip_with(&["[Content_Types].xml", "word/document.xml"], false);
        assert_eq!(sniff(&docx), Format::Docx);
    }

    #[test]
    fn hostile_zip_does_not_panic() {
        let mut z = zip_with(&["word/document.xml"], true);
        // Corrupt the central-directory offset and size.
        let n = z.len();
        z[n - 6..n - 2].copy_from_slice(&u32::MAX.to_le_bytes());
        z[n - 10..n - 6].copy_from_slice(&u32::MAX.to_le_bytes());
        let _ = sniff(&z);
        for cut in 0..z.len() {
            let _ = sniff(&z[..cut]);
        }
        assert_eq!(sniff(b"PK\x05\x06"), Format::Zip);
    }

    #[test]
    fn images() {
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n\0\0").mime, "image/png");
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]).mime, "image/jpeg");
        assert_eq!(detect(b"GIF89a....").mime, "image/gif");
        assert_eq!(detect(b"II*\0....").mime, "image/tiff");
        assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 ").mime, "image/webp");
        assert_eq!(detect(b"\0\0\0\x18ftypheic\0\0").mime, "image/heic");
        for b in [&b"\x89PNG\r\n\x1a\n"[..], b"GIF87a", b"MM\0*"] {
            assert_eq!(sniff(b), Format::Image);
        }
    }

    #[test]
    fn text_heuristics() {
        assert_eq!(detect(b"hello world\n").mime, "text/plain; charset=utf-8");
        assert_eq!(sniff("Ugovor o zakupu: čćžšđ".as_bytes()), Format::Text);
        assert_eq!(detect(b"caf\xE9 cr\xE8me").mime, "text/plain");
        assert_eq!(sniff(b"\xFF\xFEh\0i\0"), Format::Text);
        assert_eq!(sniff(b"bin\0ary"), Format::Unknown);
        assert_eq!(sniff(&[1, 2, 3, 4, 5, 6, 7, 8]), Format::Unknown);
        assert_eq!(detect(b"").mime, "application/x-empty");
        assert_eq!(sniff(b"{\\rtf1\\ansi"), Format::Unknown);
        // A UTF-8 sequence cut by the window still counts as UTF-8.
        let mut long = vec![b'a'; TEXT_WINDOW - 1];
        long.extend_from_slice("ž".as_bytes());
        assert_eq!(detect(&long).mime, "text/plain; charset=utf-8");
    }

    #[test]
    fn ole_is_unknown() {
        let s = detect(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]);
        assert_eq!(s.format, Format::Unknown);
        assert_eq!(s.mime, "application/x-ole-storage");
    }
}

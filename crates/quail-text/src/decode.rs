//! Bytes of a plain-text file to a `String`: encoding detection and decoding.
//!
//! The order of decisions, first match wins:
//!
//! 1. A byte-order mark: UTF-8 (`EF BB BF`), UTF-16LE (`FF FE`), UTF-16BE (`FE FF`). The BOM
//!    is dropped from the text.
//! 2. No BOM, but NUL bytes in at least a third of the even (or odd) positions and almost
//!    none in the others: UTF-16BE (or LE) without a BOM (checked first: ASCII-only UTF-16
//!    is also valid UTF-8).
//! 3. Valid UTF-8 (pure ASCII included): UTF-8.
//! 4. Almost valid UTF-8: at least four valid multi-byte sequences per invalid one. Decoded
//!    as UTF-8, invalid sequences become U+FFFD (a truncated or slightly damaged UTF-8 file
//!    is far more common than a legacy file that happens to contain valid UTF-8).
//! 5. Otherwise a legacy encoding guessed by [`chardetng`] (Windows-125x, ISO-8859-x, KOI8,
//!    IBM866, `Shift_JIS`, GBK, Big5, EUC-x...) over the first [`DETECT_WINDOW`] bytes, decoded
//!    with [`encoding_rs`].
//!
//! [`Decoded::confidence`] is 1 for a BOM and for valid UTF-8. For a guessed legacy encoding
//! `chardetng` gives no score, so it is a heuristic in `[0.3, 0.9]`: higher with more
//! non-ASCII characters to judge from (32 or more count fully) and with a larger share of
//! them decoding to letters (a wrong code page tends to yield symbols and controls). Every
//! replaced (malformed) sequence lowers it.

use std::borrow::Cow;

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// Bytes fed to the legacy-encoding detector at most (1 MiB): enough for a stable guess,
/// bounded time on a huge file.
pub const DETECT_WINDOW: usize = 1 << 20;

/// The text of a file and how it was decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded<'a> {
    /// The text, without the byte-order mark. Malformed sequences are U+FFFD.
    pub text: Cow<'a, str>,
    /// The encoding used; [`Decoded::name`] is its lowercase name.
    pub encoding: &'static Encoding,
    /// How sure the choice is, in `[0, 1]` (see the module docs).
    pub confidence: f32,
    /// The file started with a byte-order mark.
    pub bom: bool,
    /// Malformed sequences replaced by U+FFFD (an approximate count for legacy decoders:
    /// replacement characters in the output that were not in the input).
    pub replaced: usize,
}

impl Decoded<'_> {
    /// Lowercase WHATWG name of the encoding: `utf-8`, `utf-16le`, `windows-1250`,
    /// `iso-8859-5`, `shift_jis`, ...
    pub fn name(&self) -> String {
        self.encoding.name().to_ascii_lowercase()
    }

    /// True when some bytes could not be decoded.
    pub fn lossy(&self) -> bool {
        self.replaced > 0
    }
}

/// Replacement characters in `s`.
fn replacements(s: &str) -> usize {
    s.chars()
        .filter(|&c| c == char::REPLACEMENT_CHARACTER)
        .count()
}

/// Decodes `bytes` with `enc` (no BOM sniffing), counting the sequences it had to replace.
fn decode_with<'a>(bytes: &'a [u8], enc: &'static Encoding) -> (Cow<'a, str>, usize) {
    let (text, errors) = enc.decode_without_bom_handling(bytes);
    if !errors {
        return (text, 0);
    }
    // At least one; a U+FFFD already in the input is counted too (rare, hence approximate).
    let n = replacements(&text).max(1);
    (text, n)
}

/// Valid multi-byte sequences and invalid sequences of `bytes` read as UTF-8.
fn utf8_profile(bytes: &[u8]) -> (usize, usize) {
    let mut valid_multi = 0usize;
    let mut invalid = 0usize;
    for chunk in bytes.utf8_chunks() {
        valid_multi += chunk.valid().chars().filter(|c| c.len_utf8() > 1).count();
        if !chunk.invalid().is_empty() {
            invalid += 1;
        }
    }
    (valid_multi, invalid)
}

/// `Some(true)` for UTF-16BE, `Some(false)` for UTF-16LE, judged from NUL positions in the
/// first [`DETECT_WINDOW`] bytes (text in a Latin or Cyrillic script has a zero high byte in
/// most UTF-16 code units).
fn utf16_without_bom(bytes: &[u8]) -> Option<bool> {
    let w = &bytes[..bytes.len().min(DETECT_WINDOW)];
    let pairs = w.len() / 2;
    if pairs < 2 || !w.contains(&0) {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for p in w.chunks_exact(2) {
        even += usize::from(p[0] == 0);
        odd += usize::from(p[1] == 0);
    }
    if even * 3 >= pairs && odd * 50 <= pairs {
        Some(true)
    } else if odd * 3 >= pairs && even * 50 <= pairs {
        Some(false)
    } else {
        None
    }
}

/// Heuristic confidence of a legacy guess, see the module docs.
#[allow(clippy::cast_precision_loss)]
fn legacy_confidence(text: &str, replaced: usize) -> f32 {
    let mut non_ascii = 0usize;
    let mut letters = 0usize;
    for c in text.chars().filter(|c| !c.is_ascii()) {
        non_ascii += 1;
        letters += usize::from(c.is_alphabetic());
    }
    if non_ascii == 0 {
        return 0.9;
    }
    let share = letters as f32 / non_ascii as f32;
    let evidence = (non_ascii as f32 / 32.0).min(1.0);
    let penalty = (replaced as f32 / non_ascii as f32).min(1.0) * 0.4;
    (0.5 + 0.4 * share * evidence - penalty).clamp(0.3, 0.9)
}

/// `rest` (after a BOM) decoded with `enc`.
fn with_bom<'a>(rest: &'a [u8], enc: &'static Encoding) -> Decoded<'a> {
    let (text, replaced) = decode_with(rest, enc);
    Decoded {
        text,
        encoding: enc,
        confidence: 1.0,
        bom: true,
        replaced,
    }
}

/// Detects the encoding of `bytes` and decodes them (see the module docs).
#[allow(clippy::cast_precision_loss)]
pub fn decode(bytes: &[u8]) -> Decoded<'_> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return with_bom(rest, UTF_8);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return with_bom(rest, UTF_16LE);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return with_bom(rest, UTF_16BE);
    }
    if let Some(be) = utf16_without_bom(bytes) {
        let enc = if be { UTF_16BE } else { UTF_16LE };
        let (text, replaced) = decode_with(bytes, enc);
        let units = (bytes.len() / 2).max(1) as f32;
        return Decoded {
            text,
            encoding: enc,
            confidence: (0.8 - replaced as f32 / units).clamp(0.3, 0.8),
            bom: false,
            replaced,
        };
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Decoded {
            text: Cow::Borrowed(s),
            encoding: UTF_8,
            confidence: 1.0,
            bom: false,
            replaced: 0,
        };
    }
    let (valid_multi, invalid) = utf8_profile(bytes);
    if valid_multi > 0 && valid_multi >= 4 * invalid {
        let text = String::from_utf8_lossy(bytes);
        let share = invalid as f32 / (valid_multi + invalid) as f32;
        return Decoded {
            text,
            encoding: UTF_8,
            confidence: (0.95 - share).clamp(0.5, 0.95),
            bom: false,
            replaced: invalid,
        };
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    let window = &bytes[..bytes.len().min(DETECT_WINDOW)];
    det.feed(window, window.len() == bytes.len());
    let enc = det.guess(None, chardetng::Utf8Detection::Deny);
    let (text, replaced) = decode_with(bytes, enc);
    let confidence = legacy_confidence(&text, replaced);
    Decoded {
        text,
        encoding: enc,
        confidence,
        bom: false,
        replaced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str, be: bool) -> Vec<u8> {
        s.encode_utf16()
            .flat_map(|u| if be { u.to_be_bytes() } else { u.to_le_bytes() })
            .collect()
    }

    #[test]
    fn utf8_with_and_without_bom() {
        let d = decode("Ugovor o zakupu: čćžšđ".as_bytes());
        assert_eq!(
            (d.name().as_str(), d.bom, d.confidence, d.replaced),
            ("utf-8", false, 1.0, 0)
        );
        assert!(matches!(d.text, Cow::Borrowed(_)));
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice("Član 1".as_bytes());
        let d = decode(&b);
        assert_eq!(
            (d.name().as_str(), d.bom, &*d.text),
            ("utf-8", true, "Član 1")
        );
    }

    #[test]
    fn utf16_with_bom() {
        let s = "Ugovor — Уговор";
        let mut le = vec![0xFF, 0xFE];
        le.extend(utf16(s, false));
        let d = decode(&le);
        assert_eq!((d.name().as_str(), d.bom, &*d.text), ("utf-16le", true, s));
        let mut be = vec![0xFE, 0xFF];
        be.extend(utf16(s, true));
        let d = decode(&be);
        assert_eq!((d.name().as_str(), d.bom, &*d.text), ("utf-16be", true, s));
        // An odd trailing byte is one replaced sequence.
        le.push(0x41);
        let d = decode(&le);
        assert!(d.lossy());
        assert!(d.text.starts_with(s));
    }

    #[test]
    fn utf16_without_bom_from_nul_positions() {
        let s = "Plain text in UTF-16 without a byte order mark.";
        let le = utf16(s, false);
        let d = decode(&le);
        assert_eq!((d.name().as_str(), d.bom, &*d.text), ("utf-16le", false, s));
        let be = utf16(s, true);
        let d = decode(&be);
        assert_eq!((d.name().as_str(), &*d.text), ("utf-16be", s));
        assert!(d.confidence < 1.0);
    }

    #[test]
    fn nearly_utf8_is_lossy_utf8() {
        let mut b = "Ugovor čćžšđ i još ćirilica: Уговор о закупу"
            .as_bytes()
            .to_vec();
        b.push(0xC4); // a sequence cut short
        let d = decode(&b);
        assert_eq!(d.name().as_str(), "utf-8");
        assert_eq!(d.replaced, 1);
        assert!(d.confidence < 1.0 && d.confidence >= 0.5);
        assert!(d.text.ends_with('\u{FFFD}'));
    }

    #[test]
    fn empty_and_ascii() {
        let d = decode(b"");
        assert_eq!((d.name().as_str(), &*d.text), ("utf-8", ""));
        let d = decode(b"just ascii");
        assert_eq!((d.name().as_str(), d.confidence), ("utf-8", 1.0));
    }
}

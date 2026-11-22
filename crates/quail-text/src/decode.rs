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
//! [`decode_prefix`] decodes only as much as fits a text budget, so a file of a
//! multi-byte legacy encoding never needs its whole decoded text in memory.
//!
//! [`Decoded::confidence`] is 1 for a BOM and for valid UTF-8. For a guessed legacy encoding
//! `chardetng` gives no score, so it is a heuristic in `[0.3, 0.9]`: higher with more
//! non-ASCII characters to judge from (32 or more count fully) and with a larger share of
//! them decoding to letters (a wrong code page tends to yield symbols and controls). Every
//! replaced (malformed) sequence lowers it.

use std::borrow::Cow;

use encoding_rs::{CoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE};

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

/// Input bytes decoded per call of the streaming decoder.
const CHUNK: usize = 64 << 10;

/// The longest prefix of `s` of at most `max` bytes, on a char boundary, in place.
fn truncate_at_char(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// Room for `need` more bytes in `out`, growing geometrically but never past `max + need`.
fn reserve_bounded(out: &mut String, need: usize, max: usize) {
    if out.capacity() - out.len() >= need {
        return;
    }
    let want = out.len().saturating_add(need);
    let target = out
        .capacity()
        .saturating_mul(2)
        .min(max.saturating_add(need))
        .max(want);
    out.reserve_exact(target - out.len());
}

/// Decodes `bytes` with `enc` (no BOM sniffing) into at most `max` bytes of text (cut at a
/// char boundary), counting the sequences it had to replace. The flag is true when the
/// whole input fit. The output buffer never grows much past `max`, whatever the input.
fn decode_with<'a>(
    bytes: &'a [u8],
    enc: &'static Encoding,
    max: usize,
) -> (Cow<'a, str>, usize, bool) {
    if bytes.len() <= max && enc == UTF_8 {
        // Small UTF-8: borrow when valid, as encoding_rs does.
        let (text, errors) = enc.decode_without_bom_handling(bytes);
        if text.len() <= max {
            let n = if errors {
                replacements(&text).max(1)
            } else {
                0
            };
            return (text, n, true);
        }
    }
    let mut dec = enc.new_decoder_without_bom_handling();
    let mut out = String::new();
    let mut errors = false;
    let mut pos = 0usize;
    let complete = loop {
        let end = bytes.len().min(pos.saturating_add(CHUNK));
        let chunk = bytes.get(pos..end).unwrap_or_default();
        let last = end == bytes.len();
        let need = dec
            .max_utf8_buffer_length(chunk.len())
            .unwrap_or(chunk.len().saturating_mul(3).saturating_add(16));
        reserve_bounded(&mut out, need, max);
        let (res, read, had) = dec.decode_to_string(chunk, &mut out, last);
        errors |= had;
        pos += read;
        if out.len() > max {
            break false;
        }
        if res == CoderResult::InputEmpty && last && pos == bytes.len() {
            break true;
        }
    };
    let complete = complete && out.len() <= max;
    truncate_at_char(&mut out, max);
    // At least one; a U+FFFD already in the input is counted too (rare, hence approximate).
    let n = if errors { replacements(&out).max(1) } else { 0 };
    (Cow::Owned(out), n, complete)
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
fn with_bom<'a>(rest: &'a [u8], enc: &'static Encoding, max: usize) -> (Decoded<'a>, bool) {
    let (text, replaced, complete) = decode_with(rest, enc, max);
    let d = Decoded {
        text,
        encoding: enc,
        confidence: 1.0,
        bom: true,
        replaced,
    };
    (d, complete)
}

/// Detects the encoding of `bytes` and decodes them (see the module docs).
pub fn decode(bytes: &[u8]) -> Decoded<'_> {
    decode_prefix(bytes, usize::MAX).0
}

/// [`decode`], keeping at most `max_text` bytes of text (cut at a char boundary): only that
/// much is ever decoded, so memory stays near `max_text` whatever the encoding expands to.
/// The flag is true when the whole input fit. Detection reads the input as [`decode`] does;
/// the replacement count and the legacy confidence are those of the kept text.
#[allow(clippy::cast_precision_loss)]
pub fn decode_prefix(bytes: &[u8], max_text: usize) -> (Decoded<'_>, bool) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return with_bom(rest, UTF_8, max_text);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return with_bom(rest, UTF_16LE, max_text);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return with_bom(rest, UTF_16BE, max_text);
    }
    if let Some(be) = utf16_without_bom(bytes) {
        let enc = if be { UTF_16BE } else { UTF_16LE };
        let (text, replaced, complete) = decode_with(bytes, enc, max_text);
        let units = (bytes.len() / 2).max(1) as f32;
        let d = Decoded {
            text,
            encoding: enc,
            confidence: (0.8 - replaced as f32 / units).clamp(0.3, 0.8),
            bom: false,
            replaced,
        };
        return (d, complete);
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        let kept = cut_str(s, max_text);
        let d = Decoded {
            text: Cow::Borrowed(kept),
            encoding: UTF_8,
            confidence: 1.0,
            bom: false,
            replaced: 0,
        };
        return (d, kept.len() == s.len());
    }
    let (valid_multi, invalid) = utf8_profile(bytes);
    if valid_multi > 0 && valid_multi >= 4 * invalid {
        // A U+FFFD (3 bytes) stands for 1 to 3 input bytes, so the decoded text is at least
        // as long as its input: `max_text` input bytes are enough, minus a char cut in two.
        let mut end = bytes.len().min(max_text);
        if end < bytes.len() {
            let floor = end.saturating_sub(3);
            while end > floor && bytes.get(end).is_some_and(|b| b & 0xC0 == 0x80) {
                end -= 1;
            }
        }
        let head = bytes.get(..end).unwrap_or(bytes);
        let mut text = String::from_utf8_lossy(head);
        let complete = end == bytes.len() && text.len() <= max_text;
        if text.len() > max_text {
            let mut t = text.into_owned();
            truncate_at_char(&mut t, max_text);
            text = Cow::Owned(t);
        }
        let invalid = if end == bytes.len() {
            invalid
        } else {
            utf8_profile(head).1
        };
        let share = invalid as f32 / (valid_multi + invalid).max(1) as f32;
        let d = Decoded {
            text,
            encoding: UTF_8,
            confidence: (0.95 - share).clamp(0.5, 0.95),
            bom: false,
            replaced: invalid,
        };
        return (d, complete);
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    let window = &bytes[..bytes.len().min(DETECT_WINDOW)];
    det.feed(window, window.len() == bytes.len());
    let enc = det.guess(None, chardetng::Utf8Detection::Deny);
    let (text, replaced, complete) = decode_with(bytes, enc, max_text);
    let confidence = legacy_confidence(&text, replaced);
    let d = Decoded {
        text,
        encoding: enc,
        confidence,
        bom: false,
        replaced,
    };
    (d, complete)
}

/// The longest prefix of `s` of at most `max` bytes, on a char boundary.
fn cut_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s` in Windows-1251.
    fn cp1251(s: &str) -> Vec<u8> {
        encoding_rs::WINDOWS_1251.encode(s).0.into_owned()
    }

    #[test]
    fn a_prefix_decodes_only_what_it_keeps() {
        // Cyrillic in Windows-1251: one byte in, two bytes out.
        let bytes = cp1251("Испорука измишљене робе купцу у року. ").repeat(4000);
        let full = decode(&bytes);
        assert_eq!(full.name(), "windows-1251");
        assert!(full.text.len() > 2 * 100_000);
        let (d, complete) = decode_prefix(&bytes, 100_000);
        assert!(!complete);
        assert!(
            d.text.len() <= 100_000 && d.text.len() >= 99_998,
            "{}",
            d.text.len()
        );
        assert!(full.text.starts_with(&*d.text));
        assert_eq!(d.name(), full.name());
        // Everything fits: exactly what `decode` gives.
        let (all, complete) = decode_prefix(&bytes, usize::MAX);
        assert!(complete);
        assert_eq!(all, full);
        let (exact, complete) = decode_prefix(&bytes, full.text.len());
        assert!(complete);
        assert_eq!(exact, full);
    }

    #[test]
    fn prefixes_of_every_kind_match_decode_cut() {
        let s = "Ugovor — Уговор čćž ".repeat(500);
        let mut le = vec![0xFF, 0xFE];
        le.extend(utf16(&s, false));
        let mut lossy = s.as_bytes().to_vec();
        lossy[7] = 0xFF;
        let inputs: Vec<Vec<u8>> = vec![
            s.as_bytes().to_vec(),
            le,
            utf16(&s, true),
            lossy,
            cp1251(&"Купац плаћа робу. ".repeat(500)),
        ];
        for bytes in &inputs {
            let full = decode(bytes);
            for max in [0, 1, 7, 1000, 4097, full.text.len(), full.text.len() + 9] {
                let (d, complete) = decode_prefix(bytes, max);
                let mut end = max.min(full.text.len());
                while !full.text.is_char_boundary(end) {
                    end -= 1;
                }
                assert_eq!(&*d.text, &full.text[..end], "{} max {max}", full.name());
                assert_eq!(
                    complete,
                    end == full.text.len(),
                    "{} max {max}",
                    full.name()
                );
                assert_eq!(d.encoding, full.encoding);
            }
        }
    }

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

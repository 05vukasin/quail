//! Which encoding the bytes are in: a BOM, then the declared charset (`<meta charset>`,
//! `<meta http-equiv="Content-Type">`, the XML declaration of XHTML and inline XBRL), then
//! UTF-8 when the bytes are valid UTF-8, then a `chardetng` guess.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

/// Bytes searched for a declared charset. WHATWG prescans 1024; inline XBRL files can put a
/// long list of namespace declarations before the `<head>`, so look a little further.
pub(crate) const PRESCAN: usize = 4096;
/// Bytes fed to the legacy-encoding guess.
const GUESS_BYTES: usize = 1 << 20;

/// Where the encoding came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// A byte order mark.
    Bom,
    /// `<meta charset>` or `<meta http-equiv="Content-Type" content="...; charset=...">`.
    Meta,
    /// `<?xml ... encoding="..."?>`.
    XmlDecl,
    /// Nothing declared, and the bytes are valid UTF-8.
    Utf8,
    /// Nothing declared, not UTF-8: guessed from the bytes (UTF-16 by its zero bytes, else
    /// `chardetng`).
    Guess,
}

impl Source {
    /// Stable name, as written in `html.encoding_source`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bom => "bom",
            Self::Meta => "meta",
            Self::XmlDecl => "xml",
            Self::Utf8 => "utf8",
            Self::Guess => "guess",
        }
    }
}

/// The encoding of `bytes`, how many leading bytes are its BOM, and where it came from.
pub(crate) fn detect(bytes: &[u8]) -> (&'static Encoding, usize, Source) {
    if let Some((enc, len)) = Encoding::for_bom(bytes) {
        return (enc, len, Source::Bom);
    }
    let head = &bytes[..bytes.len().min(PRESCAN)];
    if let Some(be) = utf16_without_bom(head) {
        return (if be { UTF_16BE } else { UTF_16LE }, 0, Source::Guess);
    }
    if let Some(enc) = meta_charset(head).and_then(html_label) {
        return (enc, 0, Source::Meta);
    }
    if let Some(enc) = xml_encoding(head).and_then(html_label) {
        return (enc, 0, Source::XmlDecl);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (UTF_8, 0, Source::Utf8);
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(&bytes[..bytes.len().min(GUESS_BYTES)], true);
    (
        det.guess(None, chardetng::Utf8Detection::Deny),
        0,
        Source::Guess,
    )
}

/// An encoding label as HTML reads it: a declared UTF-16 means UTF-8 (the declaration was
/// readable as ASCII, so the bytes are not UTF-16), `x-user-defined` means windows-1252.
fn html_label(label: &[u8]) -> Option<&'static Encoding> {
    let enc = Encoding::for_label(label)?;
    Some(if enc == UTF_16LE || enc == UTF_16BE {
        UTF_8
    } else if enc == encoding_rs::X_USER_DEFINED {
        WINDOWS_1252
    } else {
        enc
    })
}

/// UTF-16 without a BOM: at least a third of the 2-byte units have a zero byte on one side
/// and at most 2% on the other (`Some(true)`: big endian).
fn utf16_without_bom(w: &[u8]) -> Option<bool> {
    let pairs = w.len() / 2;
    if pairs < 16 {
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

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn starts_ci(b: &[u8], p: &[u8]) -> bool {
    b.len() >= p.len() && b[..p.len()].eq_ignore_ascii_case(p)
}

/// The `encoding` pseudo-attribute of an XML declaration at the very start.
fn xml_encoding(head: &[u8]) -> Option<&[u8]> {
    let decl = head.strip_prefix(b"<?xml")?;
    let decl = &decl[..find(decl, b"?>")?];
    let at = find(decl, b"encoding")?;
    let rest = decl[at + 8..].trim_ascii_start().strip_prefix(b"=")?;
    let rest = rest.trim_ascii_start();
    let quote = *rest.first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.iter().position(|&c| c == quote)?;
    Some(rest[..end].trim_ascii())
}

/// A simplified WHATWG prescan: the charset of the first `<meta>` that declares one,
/// skipping comments and other tags.
fn meta_charset(head: &[u8]) -> Option<&[u8]> {
    let mut i = 0usize;
    while i < head.len() {
        let rest = &head[i..];
        if rest.starts_with(b"<!--") {
            i += 4 + find(&rest[4..], b"-->")? + 3;
        } else if starts_ci(rest, b"<meta")
            && rest
                .get(5)
                .is_some_and(|c| c.is_ascii_whitespace() || *c == b'/')
        {
            let (attrs, used) = tag_attributes(&rest[5..]);
            i += 5 + used;
            let get = |n: &str| {
                attrs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(n.as_bytes()))
            };
            if let Some((_, v)) = get("charset") {
                return Some(v.trim_ascii());
            }
            if get("http-equiv").is_some_and(|(_, v)| v.eq_ignore_ascii_case(b"content-type"))
                && let Some((_, c)) = get("content")
                && let Some(cs) = content_charset(c)
            {
                return Some(cs);
            }
        } else if rest.first() == Some(&b'<')
            && rest
                .get(1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/' || *c == b'!' || *c == b'?')
        {
            i += rest.iter().position(|&c| c == b'>')? + 1;
        } else {
            i += 1;
        }
    }
    None
}

/// `charset=...` in a `content` attribute value.
fn content_charset(c: &[u8]) -> Option<&[u8]> {
    let lower: Vec<u8> = c.to_ascii_lowercase();
    let at = find(&lower, b"charset")?;
    let rest = c[at + 7..].trim_ascii_start().strip_prefix(b"=")?;
    let rest = rest.trim_ascii_start();
    match rest.first() {
        Some(&q @ (b'"' | b'\'')) => {
            let r = &rest[1..];
            Some(&r[..r.iter().position(|&x| x == q)?])
        }
        Some(_) => {
            let end = rest
                .iter()
                .position(|&x| x == b';' || x.is_ascii_whitespace())
                .unwrap_or(rest.len());
            Some(&rest[..end])
        }
        None => None,
    }
}

/// Attributes of a tag whose name was already read, up to its `>`, and the bytes used.
/// At most 64 attributes.
#[allow(clippy::many_single_char_names, clippy::type_complexity)] // a byte scanner
fn tag_attributes(b: &[u8]) -> (Vec<(&[u8], &[u8])>, usize) {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() && out.len() < 64 {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' {
            return (out, (i + 1).min(b.len()));
        }
        let start = i;
        while i < b.len() && !matches!(b[i], b'=' | b'>' | b'/') && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = &b[start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            out.push((name, &b[0..0]));
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if let Some(&q @ (b'"' | b'\'')) = b.get(i) {
            let s = i + 1;
            let e = b[s..]
                .iter()
                .position(|&c| c == q)
                .map_or(b.len(), |p| s + p);
            i = (e + 1).min(b.len());
            &b[s..e]
        } else {
            let s = i;
            while i < b.len() && b[i] != b'>' && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            &b[s..i]
        };
        out.push((name, value));
    }
    (out, i.min(b.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_charsets() {
        let d = |b: &[u8]| {
            let (e, n, s) = detect(b);
            (e.name(), n, s)
        };
        assert_eq!(
            d(b"<html><head><meta charset=\"windows-1251\"><title>x</title>"),
            ("windows-1251", 0, Source::Meta)
        );
        assert_eq!(
            d(b"<HTML><HEAD><META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=iso-8859-1\">"),
            ("windows-1252", 0, Source::Meta)
        );
        assert_eq!(
            d(b"<meta content='text/html;charset=UTF-8' http-equiv='content-type'>"),
            ("UTF-8", 0, Source::Meta)
        );
        // Comments are skipped; a declared UTF-16 is UTF-8.
        assert_eq!(
            d(b"<!-- <meta charset=koi8-r> --><meta charset=utf-16>"),
            ("UTF-8", 0, Source::Meta)
        );
        // Inline XBRL: the XML declaration.
        assert_eq!(
            d(b"<?xml version='1.0' encoding='ASCII'?><html>"),
            ("windows-1252", 0, Source::XmlDecl)
        );
        assert_eq!(d(b"\xEF\xBB\xBF<p>x"), ("UTF-8", 3, Source::Bom));
        assert_eq!(d(b"<p>caf\xC3\xA9</p>"), ("UTF-8", 0, Source::Utf8));
        assert_eq!(d(b"<p>caf\xE9 cr\xE8me br\xFBl\xE9e</p>").2, Source::Guess);
        // Unknown labels fall through.
        assert_eq!(d(b"<meta charset=bogus><p>x").2, Source::Utf8);
        // Truncated tags and comments never panic.
        for b in [
            &b"<meta charset="[..],
            b"<meta",
            b"<!--",
            b"<?xml encoding=",
            b"<a",
        ] {
            let _ = detect(b);
        }
    }
}

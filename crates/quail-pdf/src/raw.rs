//! Facts read straight from the file bytes, without parsing objects. They work even when the
//! object graph is broken or encrypted, which is exactly when they are most useful.

/// What a byte-level scan of the file shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RawScan {
    /// Version from the `%PDF-x.y` header, e.g. `"1.7"`.
    pub header_version: Option<String>,
    /// Number of `%%EOF` markers: one per revision of the file.
    pub eof_markers: usize,
    /// Number of `startxref` keywords: one per xref section written.
    pub startxref: usize,
    /// A `/Linearized` dictionary appears near the start of the file.
    pub linearized: bool,
    /// An `/Encrypt` entry appears in a trailer or xref stream dictionary.
    pub encrypt_entry: bool,
}

/// How far into the file the header and the linearization dictionary may sit.
const HEAD_WINDOW: usize = 1024;

pub(crate) fn scan(bytes: &[u8]) -> RawScan {
    let head = bytes.get(..bytes.len().min(HEAD_WINDOW)).unwrap_or(bytes);
    let header_version = find(head, b"%PDF-").and_then(|i| {
        let v = head.get(i + 5..)?;
        let n = v
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b'.')
            .count()
            .min(4);
        let s = std::str::from_utf8(v.get(..n)?).ok()?;
        (!s.is_empty()).then(|| s.to_owned())
    });
    // `/Linearized` must be in the first object, which sits inside the first kilobyte.
    let linearized = find(head, b"/Linearized").is_some();
    RawScan {
        header_version,
        eof_markers: count(bytes, b"%%EOF"),
        startxref: count(bytes, b"startxref"),
        linearized,
        encrypt_entry: has_encrypt_entry(bytes),
    }
}

/// Number of incremental updates: revisions after the first. A linearized file carries one
/// extra `%%EOF`/`startxref` pair for its first-page section, which is not an update.
pub(crate) fn incremental_updates(s: &RawScan) -> usize {
    let sections = s.eof_markers.max(s.startxref);
    let base = if s.linearized { 2 } else { 1 };
    sections.saturating_sub(base)
}

/// `/Encrypt` inside a trailer (or an xref stream dictionary, which is how PDF 1.5+ writes its
/// trailer). Looking at every occurrence would also match a literal string, so only those
/// followed by an indirect reference or a dictionary count.
fn has_encrypt_entry(bytes: &[u8]) -> bool {
    let mut from = 0;
    while let Some(i) = bytes.get(from..).and_then(|b| find(b, b"/Encrypt")) {
        let at = from + i + b"/Encrypt".len();
        let rest = bytes
            .get(at..at.saturating_add(32).min(bytes.len()))
            .unwrap_or(&[]);
        let rest = trim_start(rest);
        let is_ref = rest.first().is_some_and(u8::is_ascii_digit)
            && rest.iter().take(32).any(|b| *b == b'R');
        if is_ref || rest.starts_with(b"<<") {
            return true;
        }
        from = at;
    }
    false
}

fn trim_start(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|c| c.is_ascii_whitespace()).count();
    b.get(n..).unwrap_or(&[])
}

pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

pub(crate) fn count(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    let first = needle[0];
    let mut n = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i] == first && hay.get(i..i + needle.len()) == Some(needle) {
            n += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_revisions() {
        let f = b"%PDF-1.4\n1 0 obj<<>>endobj\nstartxref\n9\n%%EOF\nmore\nstartxref\n10\n%%EOF\n";
        let s = scan(f);
        assert_eq!(s.header_version.as_deref(), Some("1.4"));
        assert_eq!(s.eof_markers, 2);
        assert_eq!(incremental_updates(&s), 1);
        assert!(!s.linearized);
        assert!(!s.encrypt_entry);
    }

    #[test]
    fn encrypt_needs_ref_or_dict() {
        assert!(scan(b"trailer<</Encrypt 5 0 R>>").encrypt_entry);
        assert!(!scan(b"(see /Encrypt docs)").encrypt_entry);
    }

    #[test]
    fn empty_and_tiny_inputs() {
        assert_eq!(scan(b""), RawScan::default());
        assert_eq!(count(b"%%", b"%%EOF"), 0);
    }
}

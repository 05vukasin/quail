//! The document text the text sketches read, within the file's limits.

use std::borrow::Cow;

use quail_core::{Document, Emitter, Limits, Result};
use quail_text::normalize::{NormalizeOptions, normalize_into};
use quail_text::piece::{PIECE_BYTES, pieces};
use quail_text::shingle::hash_str;
use quail_text::token::Token;
use quail_text::{NormalizedPage, NormalizedText};

/// The longest prefix of `text` within `budget` bytes, on a char boundary.
pub(crate) fn within(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Calls `f(position, page text part)` for every page the text sketches read: pages that
/// need OCR or have no text are skipped, at most `Limits::max_text_bytes` of page text is
/// read. Checks the deadline per page and records a problem when the limit stops it.
fn for_each_part<'d>(
    doc: &'d Document,
    limits: &Limits,
    out: &mut Emitter,
    mut f: impl FnMut(usize, &'d str),
) -> Result<()> {
    let mut budget = usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX);
    for (pos, page) in doc.pages.iter().enumerate() {
        out.check_deadline()?;
        if page.needs_ocr || page.text.is_empty() {
            continue;
        }
        if budget == 0 {
            out.problem(format!(
                "{}: text limit reached, pages from {} not sketched",
                out.src(),
                page.index
            ));
            break;
        }
        let part = within(&page.text, budget);
        budget -= part.len();
        f(pos, part);
    }
    Ok(())
}

/// The page texts of [`for_each_part`] as extracted, as consecutive segments of their
/// `'\n'`-joined text: page parts, `"\n"` between them, and a part longer than
/// [`PIECE_BYTES`] cut into pieces. Joined, they are that text; the compat sketches stream
/// them instead of holding the joined copy.
pub(crate) fn raw_segments<'d>(
    doc: &'d Document,
    limits: &Limits,
    out: &mut Emitter,
) -> Result<Vec<&'d str>> {
    let mut segs = Vec::new();
    for_each_part(doc, limits, out, |_, part| {
        if !segs.is_empty() {
            segs.push("\n");
        }
        segs.extend(pieces(part, PIECE_BYTES).map(|(_, p)| p));
    })?;
    Ok(segs)
}

/// Page texts joined by `'\n'`, each normalized with the `quail-text` defaults (see
/// [`for_each_part`]). For tests and inspection; the analyzers read [`DocText`].
#[allow(dead_code)]
pub(crate) fn normalized_text(
    doc: &Document,
    limits: &Limits,
    out: &mut Emitter,
) -> Result<String> {
    let opts = NormalizeOptions::default();
    let mut text = String::new();
    let mut first = true;
    for_each_part(doc, limits, out, |_, part| {
        if !first {
            text.push('\n');
        }
        first = false;
        normalize_into(part, &opts, &mut text);
    })?;
    Ok(text)
}

/// One part of a [`DocText`]: a page of the `normalized_text` artifact, or page text to
/// normalize when it is read.
enum Part<'a> {
    Shared(&'a NormalizedPage),
    Raw(&'a str),
}

impl Part<'_> {
    fn load(&self) -> Cow<'_, NormalizedPage> {
        match self {
            Part::Shared(p) => Cow::Borrowed(*p),
            Part::Raw(t) => Cow::Owned(NormalizedPage::new(t)),
        }
    }
}

/// The normalized document text the text sketches read: the pages of [`for_each_part`], each
/// normalized with the `quail-text` defaults and tokenized, as if joined by `'\n'`.
///
/// A page comes from the `normalized_text` artifact when the artifact holds it and the whole
/// page is read; otherwise it is normalized here, one page at a time while it is read, so at
/// most one page is held. Both give the same page, and the tokens of the pages are exactly
/// the tokens of their joined text (a line break always ends a token), so the sketches are
/// the same either way. A page longer than [`PIECE_BYTES`] is read as its pieces, each
/// normalized like a page.
pub(crate) struct DocText<'a> {
    parts: Vec<Part<'a>>,
}

impl<'a> DocText<'a> {
    /// Reads `doc` within `limits`, taking pages from `shared` where it can.
    pub fn read(
        doc: &'a Document,
        shared: Option<&'a NormalizedText>,
        limits: &Limits,
        out: &mut Emitter,
    ) -> Result<Self> {
        let mut parts = Vec::new();
        for_each_part(doc, limits, out, |pos, part| {
            let full = &doc.pages[pos].text;
            match shared.and_then(|n| n.page(pos, full)) {
                Some(p) if part.len() == full.len() => parts.push(Part::Shared(p)),
                _ => parts.extend(pieces(part, PIECE_BYTES).map(|(_, p)| Part::Raw(p))),
            }
        })?;
        Ok(Self { parts })
    }

    /// Calls `f(key)` with the folded key of every word-like token, in order.
    pub fn for_each_key(&self, mut f: impl FnMut(&str)) {
        for part in &self.parts {
            part.load().for_each_keyed(|_, key| f(key));
        }
    }

    /// Hashes ([`hash_str`]) of the folded keys, in order: `token_hashes` of the joined text.
    /// Computed one page at a time, as the iterator advances.
    pub fn key_hashes(&self) -> impl Iterator<Item = u64> + '_ {
        self.parts.iter().flat_map(|part| {
            let mut v = Vec::new();
            part.load().for_each_keyed(|_, key| v.push(hash_str(key)));
            v
        })
    }

    /// Calls `f(token, key, gap)` for every word-like token of the joined text, in order:
    /// what `keyed_tokens` gives over it. `gap` is the joined text since the previous token
    /// (the whole prefix for the first); token offsets are in the joined text.
    pub fn for_each_keyed(&self, mut f: impl FnMut(&Token<'_>, &str, &str)) {
        // Joined text since the last token, while that token is on an earlier page.
        let mut tail = String::new();
        let mut base = 0usize;
        for (n, part) in self.parts.iter().enumerate() {
            if n > 0 {
                tail.push('\n');
                base += 1;
            }
            let p = part.load();
            let text = p.text();
            let mut last: Option<usize> = None;
            for (i, t) in p.tokens().iter().enumerate() {
                if !t.is_wordlike() {
                    continue;
                }
                let gap = if let Some(e) = last {
                    text.get(e..t.start()).unwrap_or("")
                } else {
                    tail.push_str(text.get(..t.start()).unwrap_or(""));
                    tail.as_str()
                };
                let token = Token {
                    text: p.token_text(i),
                    start: base + t.start(),
                    end: base + t.end(),
                    kind: t.kind,
                };
                f(&token, p.key(i), gap);
                last = Some(t.end());
            }
            if let Some(e) = last {
                tail.clear();
                tail.push_str(text.get(e..).unwrap_or(""));
            } else {
                tail.push_str(text);
            }
            base += text.len();
        }
    }
}

/// Appends `values` to `set`, a list of `u64` to be sorted and deduplicated by its user:
/// whenever it would grow past a million entries it is sorted and deduplicated first, so a
/// text that repeats itself holds about its distinct values, not one per occurrence.
pub(crate) fn extend_distinct(set: &mut Vec<u64>, values: impl IntoIterator<Item = u64>) {
    for v in values {
        if set.len() == set.capacity() && set.len() >= 1 << 20 {
            set.sort_unstable();
            set.dedup();
            if set.len() > set.capacity() / 2 {
                set.reserve_exact(set.len() / 2);
            }
        }
        set.push(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{AnalyzerId, Page, PageKind};
    use quail_text::fold::FoldOptions;
    use quail_text::token::keyed_tokens;

    fn doc(texts: &[&str]) -> Document {
        Document {
            pages: texts
                .iter()
                .enumerate()
                .map(|(i, t)| Page {
                    text: (*t).to_owned(),
                    ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
                })
                .collect(),
        }
    }

    type Seen = Vec<(String, usize, usize, String, String)>;

    fn seen(t: &DocText<'_>) -> Seen {
        let mut v = Vec::new();
        t.for_each_keyed(|tok, key, gap| {
            v.push((
                tok.text.to_owned(),
                tok.start,
                tok.end,
                key.to_owned(),
                gap.to_owned(),
            ));
        });
        v
    }

    fn direct(text: &str) -> Seen {
        let mut v = Vec::new();
        let mut prev = 0;
        let mut it = keyed_tokens(text, &FoldOptions::KEY);
        while let Some(t) = it.next_keyed() {
            v.push((
                t.token.text.to_owned(),
                t.token.start,
                t.token.end,
                t.key.to_owned(),
                text[prev..t.token.start].to_owned(),
            ));
            prev = t.token.end;
        }
        v
    }

    #[test]
    fn pages_tokenize_like_the_joined_text() {
        let d = doc(&[
            "  Ugo-\nvor o „zakupu“ — Đorđe, 1\u{a0}200,50 RSD.  ",
            ". , ;",
            "",
            "www.firma.rs i ime@firma.rs: 12.05.2023. IBAN RS35 2600 0560 1001 6113 79",
            "\u{200b}",
            "Kraj.",
        ]);
        let limits = Limits::default();
        let mut out = Emitter::new(AnalyzerId::new("sketch.test", 1));
        let joined = normalized_text(&d, &limits, &mut out).unwrap();
        let want = direct(&joined);
        assert!(want.len() > 10);
        let alone = DocText::read(&d, None, &limits, &mut out).unwrap();
        assert_eq!(seen(&alone), want);
        let shared = NormalizedText::of(&d);
        let with = DocText::read(&d, Some(&shared), &limits, &mut out).unwrap();
        assert!(with.parts.iter().all(|p| matches!(p, Part::Shared(_))));
        assert_eq!(seen(&with), want);
        let keys: Vec<String> = want.iter().map(|w| w.3.clone()).collect();
        let mut got = Vec::new();
        with.for_each_key(|k| got.push(k.to_owned()));
        assert_eq!(got, keys);
        assert_eq!(
            with.key_hashes().collect::<Vec<_>>(),
            keys.iter().map(|k| hash_str(k)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cut_pages_are_normalized_here() {
        let d = doc(&["abc def", "ghi jkl mno"]);
        let limits = Limits {
            max_text_bytes: 10,
            ..Limits::default()
        };
        let mut out = Emitter::new(AnalyzerId::new("sketch.test", 1));
        let shared = NormalizedText::of(&d);
        let t = DocText::read(&d, Some(&shared), &limits, &mut out).unwrap();
        assert!(matches!(t.parts[0], Part::Shared(_)));
        assert!(matches!(t.parts[1], Part::Raw(_)));
        let mut keys = Vec::new();
        t.for_each_key(|k| keys.push(k.to_owned()));
        assert_eq!(keys, ["abc", "def", "ghi"]);
    }

    #[test]
    fn distinct_values_are_kept_once_as_the_list_fills() {
        let mut set = Vec::new();
        // Three passes over 700 000 values: a million and more are deduplicated on the way.
        extend_distinct(&mut set, (0..2_100_000u64).map(|i| (i % 700_000) * 7919));
        assert!(set.capacity() < 2_100_000, "{}", set.capacity());
        set.sort_unstable();
        set.dedup();
        let want: Vec<u64> = (0..700_000u64).map(|i| i * 7919).collect();
        assert_eq!(set, want);
    }
}

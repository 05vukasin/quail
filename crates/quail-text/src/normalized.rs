//! The `normalized_text` artifact: every page of the document normalized once, with its
//! offset map and its keyed tokens, shared by every text analyzer that would otherwise redo
//! the same work.
//!
//! [`TextStats`](crate::TextStats) builds it from [`Ctx::document`](quail_core::Ctx::document)
//! (so it holds OCR text when OCR revised the document) and provides it as
//! [`NormalizedText`]. Per page it keeps:
//!
//! - the page text normalized with [`NormalizeOptions::default`] (exactly what
//!   [`normalize_mapped`] returns for the whole page text);
//! - its [`OffsetMap`], which maps spans both ways between normalized and page text;
//! - every token of [`tokens_with_symbols`] over the normalized text (spans in the normalized
//!   text, kind) with its key folded by [`FoldOptions::KEY`] (what [`keyed_tokens`] gives for
//!   the word-like ones);
//! - the length of the page text it was built from.
//!
//! A page is only present when it was normalized as a whole: pages cut by
//! `Limits::max_text_bytes` (and every page after) are `None`, and so are the pages after
//! the artifact holds [`MAX_SHARED_BYTES`] of page text.
//!
//! # Using it
//!
//! Consumers declare `after(&["normalized_text"])` (soft: without `text.stats` they still run)
//! and take a page with [`NormalizedText::page`] only when they would have normalized that
//! whole page text with the default options themselves. When it returns `None` (artifact
//! absent, page cut, or a different text), they run their own pass, which produces the same
//! result. That way their output never depends on whether `text.stats` ran.
//!
//! # Cost
//!
//! 16 bytes per token (about one token per 5–6 bytes of text), plus the normalized text,
//! the keys that differ from their token, and 16 bytes per map checkpoint: about 4–5 bytes
//! of heap per byte of page text, held until the file is done. [`MAX_SHARED_BYTES`] bounds it
//! (about 40 MB): the pages past it are normalized by each consumer, one page or piece at a
//! time.
//!
//! [`keyed_tokens`]: crate::token::keyed_tokens

use quail_core::{Artifact, Document};

use crate::fold::{FoldOptions, fold_into};
use crate::normalize::{NormalizeOptions, OffsetMap, normalize_mapped};
use crate::token::{TokenKind, tokens_with_symbols};

/// Page text the artifact holds at most: pages are added in order while their total stays
/// within it. A page that does not fit, or is longer than
/// [`PIECE_BYTES`](crate::piece::PIECE_BYTES), is `None` (and its consumers normalize it
/// themselves).
pub const MAX_SHARED_BYTES: usize = 8 << 20;

/// One token of a [`NormalizedPage`]: byte range in the normalized page text and kind. Its
/// key is the token text itself when folding does not change it (most lowercase words and
/// every ASCII symbol), else a range of the page's key buffer that ends at `key_end` and
/// starts where the previous token's range ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageToken {
    /// First byte in the normalized text.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
    key_end: u32,
    /// Its kind ([`TokenKind::Punct`] and [`TokenKind::Symbol`] included).
    pub kind: TokenKind,
    /// The key differs from the text and is stored in the key buffer.
    own_key: bool,
}

impl PageToken {
    /// Whether it is a word, number, code, URL or address: the tokens
    /// [`tokens`](crate::token::tokens) yields.
    pub fn is_wordlike(&self) -> bool {
        !matches!(self.kind, TokenKind::Punct | TokenKind::Symbol)
    }

    /// Start as `usize`.
    pub fn start(&self) -> usize {
        self.start as usize
    }

    /// End as `usize`.
    pub fn end(&self) -> usize {
        self.end as usize
    }
}

/// One page normalized with the default options, see the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NormalizedPage {
    text: String,
    map: OffsetMap,
    toks: Vec<PageToken>,
    keys: String,
    source_len: usize,
}

impl NormalizedPage {
    /// Normalizes `source` (a whole page text) and tokenizes the result.
    pub fn new(source: &str) -> Self {
        let mut page = Self::default();
        page.rebuild(source);
        page
    }

    fn rebuild(&mut self, source: &str) {
        normalize_mapped(
            source,
            &NormalizeOptions::default(),
            &mut self.text,
            &mut self.map,
        );
        self.toks.clear();
        self.keys.clear();
        self.toks.reserve(self.text.len() / 5);
        for t in tokens_with_symbols(&self.text) {
            let k0 = self.keys.len();
            fold_into(t.text, &FoldOptions::KEY, &mut self.keys);
            let own_key = self.keys.get(k0..) != Some(t.text);
            if !own_key {
                self.keys.truncate(k0);
            }
            self.toks.push(PageToken {
                start: to_u32(t.start),
                end: to_u32(t.end),
                key_end: to_u32(self.keys.len()),
                kind: t.kind,
                own_key,
            });
        }
        self.source_len = source.len();
        // Kept for the rest of the file: no spare capacity.
        self.text.shrink_to_fit();
        self.keys.shrink_to_fit();
        self.toks.shrink_to_fit();
        self.map.shrink_to_fit();
    }

    /// Key of token `i` (`t`), whose stored key starts at `key_start`.
    #[inline]
    fn key_of(&self, t: &PageToken, key_start: usize) -> &str {
        if t.own_key {
            self.keys.get(key_start..t.key_end as usize).unwrap_or("")
        } else {
            self.text.get(t.start()..t.end()).unwrap_or("")
        }
    }

    /// The normalized text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Map between normalized and page text offsets.
    pub fn map(&self) -> &OffsetMap {
        &self.map
    }

    /// Every token, in order, punctuation and symbols included.
    pub fn tokens(&self) -> &[PageToken] {
        &self.toks
    }

    /// Folded key ([`FoldOptions::KEY`]) of token `i`; empty out of range.
    pub fn key(&self, i: usize) -> &str {
        let Some(t) = self.toks.get(i) else {
            return "";
        };
        let start = match i.checked_sub(1) {
            Some(p) => self.toks[p].key_end as usize,
            None => 0,
        };
        self.key_of(t, start)
    }

    /// Normalized text of token `i`; empty out of range.
    pub fn token_text(&self, i: usize) -> &str {
        self.toks
            .get(i)
            .and_then(|t| self.text.get(t.start()..t.end()))
            .unwrap_or("")
    }

    /// Calls `f(token, key)` for every word-like token, in order: what
    /// [`keyed_tokens`](crate::token::keyed_tokens) over [`Self::text`] gives.
    pub fn for_each_keyed(&self, mut f: impl FnMut(&PageToken, &str)) {
        let mut key_start = 0usize;
        for t in &self.toks {
            if t.is_wordlike() {
                f(t, self.key_of(t, key_start));
            }
            key_start = t.key_end as usize;
        }
    }

    /// Number of word-like tokens.
    pub fn words(&self) -> usize {
        self.toks.iter().filter(|t| t.is_wordlike()).count()
    }

    /// Length in bytes of the page text it was built from.
    pub fn source_len(&self) -> usize {
        self.source_len
    }

    /// Heap bytes held (capacities).
    pub fn heap_bytes(&self) -> usize {
        self.text.capacity()
            + self.keys.capacity()
            + self.toks.capacity() * std::mem::size_of::<PageToken>()
            + self.map.heap_bytes()
    }
}

/// The `normalized_text` artifact: one optional [`NormalizedPage`] per page of the document,
/// by position in [`Document::pages`]. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NormalizedText {
    pages: Vec<Option<NormalizedPage>>,
}

impl Artifact for NormalizedText {
    const NAME: &'static str = "normalized_text";
}

impl NormalizedText {
    /// Every page normalized as a whole (no text limit).
    pub fn of(doc: &Document) -> Self {
        Self {
            pages: doc
                .pages
                .iter()
                .map(|p| Some(NormalizedPage::new(&p.text)))
                .collect(),
        }
    }

    /// From pages built by the caller, by position (`None`: not normalized as a whole).
    pub fn from_pages(pages: Vec<Option<NormalizedPage>>) -> Self {
        Self { pages }
    }

    /// The page at position `pos` of the document, when it was normalized from a text of
    /// `source.len()` bytes, i.e. from the whole page text the caller holds. `None` sends
    /// the caller to its own pass.
    pub fn page(&self, pos: usize, source: &str) -> Option<&NormalizedPage> {
        self.pages
            .get(pos)?
            .as_ref()
            .filter(|p| p.source_len == source.len())
    }

    /// Number of page slots.
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// No page slots.
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Heap bytes held by all pages.
    pub fn heap_bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<Option<NormalizedPage>>()
            + self
                .pages
                .iter()
                .flatten()
                .map(NormalizedPage::heap_bytes)
                .sum::<usize>()
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize::normalize;
    use crate::token::{keyed_tokens, tokens};
    use quail_core::{Page, PageKind, Span};

    const PAGES: &[&str] = &[
        "Član 5. Ugo-\nvor o zakupu „ﬁrme“ — Đorđe Ǆep i ЉУБАВ Џаја, Ćirić.\n",
        "  Iznos: 1\u{a0}200,50 RSD, broj 114/23 od 12.05.2023.   IBAN RS35 2600 0560 1001 6113 79.",
        "",
        " \u{200b} \n",
        "Ｔｅｌ ＋３８１ １１, x², e\u{301}te\u{301}, so\u{ad}ft, https://firma.rs/a?b=1 \
         ime.prezime@firma.rs C++ The Tenant's „Agreement“.",
    ];

    fn doc() -> Document {
        Document {
            pages: PAGES
                .iter()
                .enumerate()
                .map(|(i, t)| Page {
                    text: (*t).to_owned(),
                    ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
                })
                .collect(),
        }
    }

    #[test]
    fn pages_match_the_direct_passes() {
        let d = doc();
        let n = NormalizedText::of(&d);
        assert_eq!(n.len(), PAGES.len());
        for (i, src) in PAGES.iter().enumerate() {
            let p = n.page(i, src).unwrap();
            assert_eq!(p.text(), normalize(src, &NormalizeOptions::default()));
            let mut text = String::new();
            let mut map = OffsetMap::new();
            normalize_mapped(src, &NormalizeOptions::default(), &mut text, &mut map);
            assert_eq!(p.map(), &map);
            let all: Vec<_> = tokens_with_symbols(&text)
                .map(|t| (t.start, t.end, t.kind))
                .collect();
            let got: Vec<_> = p
                .tokens()
                .iter()
                .map(|t| (t.start(), t.end(), t.kind))
                .collect();
            assert_eq!(got, all);
            for (j, t) in tokens_with_symbols(&text).enumerate() {
                let mut k = String::new();
                fold_into(t.text, &FoldOptions::KEY, &mut k);
                assert_eq!(p.key(j), k);
                assert_eq!(p.token_text(j), t.text);
            }
            let mut want = Vec::new();
            let mut it = keyed_tokens(&text, &FoldOptions::KEY);
            while let Some(t) = it.next_keyed() {
                want.push((t.token.start, t.token.end, t.key.to_owned()));
            }
            let mut got = Vec::new();
            p.for_each_keyed(|t, k| got.push((t.start(), t.end(), k.to_owned())));
            assert_eq!(got, want);
            assert_eq!(p.words(), tokens(&text).count());
            assert_eq!(p.source_len(), src.len());
        }
        assert!(n.heap_bytes() > 0);
    }

    #[test]
    fn map_works_both_ways() {
        let n = NormalizedText::of(&doc());
        let src = PAGES[0];
        let p = n.page(0, src).unwrap();
        let at = p.text().find("Ugovor").unwrap();
        let s = p.map().span_to_source(Span {
            start: u32::try_from(at).unwrap(),
            end: u32::try_from(at + 6).unwrap(),
        });
        assert_eq!(s.slice(src), "Ugo-\nvor");
        let back = p.map().span_to_normalized(s);
        assert_eq!(back.slice(p.text()), "Ugovor");
    }

    #[test]
    fn page_checks_the_source_length() {
        let n = NormalizedText::of(&doc());
        assert!(n.page(0, PAGES[0]).is_some());
        assert!(n.page(0, &PAGES[0][..4]).is_none());
        assert!(n.page(99, "").is_none());
        let partial = NormalizedText::from_pages(vec![None, Some(NormalizedPage::new("ab"))]);
        assert!(partial.page(0, "").is_none());
        assert!(partial.page(1, "ab").is_some());
    }

    #[test]
    fn joined_pages_tokenize_like_the_joined_text() {
        // Sketches join pages with '\n' and tokenize the result: the per-page tokens,
        // shifted, must be exactly the tokens of the joined text.
        let n = NormalizedText::of(&doc());
        let mut joined = String::new();
        let mut want = Vec::new();
        for (i, src) in PAGES.iter().enumerate() {
            if i > 0 {
                joined.push('\n');
            }
            let base = joined.len();
            let p = n.page(i, src).unwrap();
            joined.push_str(p.text());
            p.for_each_keyed(|t, k| want.push((base + t.start(), base + t.end(), k.to_owned())));
        }
        let mut got = Vec::new();
        let mut it = keyed_tokens(&joined, &FoldOptions::KEY);
        while let Some(t) = it.next_keyed() {
            got.push((t.token.start, t.token.end, t.key.to_owned()));
        }
        assert_eq!(got, want);
    }
}

//! A page prepared for matching: normalized text with its offset map, and a *key line*: the
//! folded keys of its tokens joined by `' '` (same line) or `'\n'` (line break), with each
//! token's position in both texts. Patterns run over the key line (lowercase, Latin, no
//! diacritics, no punctuation), values are read from the normalized text, and evidence is
//! mapped back to the original page text.
//!
//! The normalized text, its map and the keyed tokens come from the `normalized_text`
//! artifact when it holds the page, else from an own pass with the same options.

use std::borrow::Cow;

use quail_core::{BBox, Block, Evidence, Page, Span, Zone};
use quail_text::NormalizedPage;
use quail_text::fold::FoldOptions;
use quail_text::normalize::{NormalizeOptions, OffsetMap, normalize_mapped};
use quail_text::token::keyed_tokens;

/// One token: its range in the key line and in the normalized text, and its line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok {
    pub ks: usize,
    pub ke: usize,
    pub ns: usize,
    pub ne: usize,
    pub line: u32,
}

/// A prepared page.
pub(crate) struct Prepared<'p> {
    pub page: &'p Page,
    pub norm: Cow<'p, str>,
    pub map: Cow<'p, OffsetMap>,
    pub keys: String,
    pub toks: Vec<Tok>,
    /// Number of lines with at least one token.
    pub lines: u32,
    /// Index of the first token of each line.
    line_first: Vec<usize>,
}

/// Builds the key line token by token.
struct KeyLine {
    keys: String,
    toks: Vec<Tok>,
    line: u32,
    prev_end: Option<usize>,
    line_first: Vec<usize>,
}

impl KeyLine {
    fn push(&mut self, norm: &str, ns: usize, ne: usize, key: &str) {
        if let Some(pe) = self.prev_end {
            let gap = norm.get(pe..ns).unwrap_or("");
            if gap.contains('\n') {
                self.line += 1;
                self.keys.push('\n');
            } else {
                self.keys.push(' ');
            }
        }
        if self.line_first.len() <= self.line as usize {
            self.line_first.push(self.toks.len());
        }
        let ks = self.keys.len();
        // Keys never hold separators: a stray space would split one token in two.
        self.keys
            .extend(key.chars().map(|c| if c.is_whitespace() { '_' } else { c }));
        self.toks.push(Tok {
            ks,
            ke: self.keys.len(),
            ns,
            ne,
            line: self.line,
        });
        self.prev_end = Some(ne);
    }
}

impl<'p> Prepared<'p> {
    /// Prepares `text` (a prefix of `page.text`, or all of it). `shared` is the page from
    /// the `normalized_text` artifact, when it holds exactly this text normalized.
    pub fn new(page: &'p Page, text: &str, shared: Option<&'p NormalizedPage>) -> Self {
        let (norm, map) = if let Some(s) = shared {
            (Cow::Borrowed(s.text()), Cow::Borrowed(s.map()))
        } else {
            let mut norm = String::new();
            let mut map = OffsetMap::new();
            normalize_mapped(text, &NormalizeOptions::default(), &mut norm, &mut map);
            (Cow::Owned(norm), Cow::Owned(map))
        };
        let mut b = KeyLine {
            keys: String::with_capacity(norm.len()),
            toks: Vec::with_capacity(norm.len() / 5),
            line: 0,
            prev_end: None,
            line_first: Vec::new(),
        };
        if let Some(s) = shared {
            s.for_each_keyed(|t, key| b.push(&norm, t.start(), t.end(), key));
        } else {
            let mut it = keyed_tokens(&norm, &FoldOptions::KEY);
            while let Some(t) = it.next_keyed() {
                b.push(&norm, t.token.start, t.token.end, t.key);
            }
        }
        let lines = if b.toks.is_empty() { 0 } else { b.line + 1 };
        Self {
            page,
            norm,
            map,
            keys: b.keys,
            toks: b.toks,
            lines,
            line_first: b.line_first,
        }
    }

    /// Index of the token starting exactly at key offset `k`.
    pub fn tok_starting(&self, k: usize) -> Option<usize> {
        let i = self.toks.partition_point(|t| t.ks < k);
        (self.toks.get(i)?.ks == k).then_some(i)
    }

    /// Index of the token ending exactly at key offset `k`.
    pub fn tok_ending(&self, k: usize) -> Option<usize> {
        let i = self.toks.partition_point(|t| t.ke < k);
        (self.toks.get(i)?.ke == k).then_some(i)
    }

    /// Key of token `i`.
    pub fn key(&self, i: usize) -> &str {
        self.toks
            .get(i)
            .and_then(|t| self.keys.get(t.ks..t.ke))
            .unwrap_or("")
    }

    /// Normalized text of token `i`.
    pub fn text(&self, i: usize) -> &str {
        self.toks
            .get(i)
            .and_then(|t| self.norm.get(t.ns..t.ne))
            .unwrap_or("")
    }

    /// Whether tokens `a` and `b` are on the same line.
    pub fn same_line(&self, a: usize, b: usize) -> bool {
        match (self.toks.get(a), self.toks.get(b)) {
            (Some(x), Some(y)) => x.line == y.line,
            _ => false,
        }
    }

    /// Line of token `i`.
    pub fn line(&self, i: usize) -> u32 {
        self.toks.get(i).map_or(u32::MAX, |t| t.line)
    }

    /// Tokens `[first, last]` of line `line` (when it has any), from a token on it.
    pub fn line_bounds(&self, i: usize) -> (usize, usize) {
        let line = self.line(i) as usize;
        let Some(&a) = self.line_first.get(line) else {
            return (i, i);
        };
        let b = self
            .line_first
            .get(line + 1)
            .map_or(self.toks.len(), |&n| n)
            .saturating_sub(1)
            .max(a);
        (a, b)
    }

    /// Span in the original page text of normalized bytes `ns..ne`.
    pub fn source_span(&self, ns: usize, ne: usize) -> Span {
        let to = |x: usize| u32::try_from(x).unwrap_or(u32::MAX);
        self.map.span_to_source(Span {
            start: to(ns),
            end: to(ne),
        })
    }

    /// The block holding original byte `at`.
    pub fn block_at(&self, at: u32) -> Option<&Block> {
        let blocks = &self.page.blocks;
        let i = blocks.partition_point(|b| b.span.start <= at);
        let b = blocks.get(i.checked_sub(1)?)?;
        (at < b.span.end.max(b.span.start + 1)).then_some(b)
    }

    /// Zone of normalized offset `ns` (from its block; `Unknown` without blocks).
    pub fn zone_at(&self, ns: usize) -> Zone {
        let src = self.source_span(ns, ns + 1);
        self.block_at(src.start).map_or(Zone::Unknown, |b| b.zone)
    }

    /// Evidence for normalized bytes `ns..ne`: page, zone and box of the block, span.
    pub fn evidence(&self, ns: usize, ne: usize) -> Evidence {
        let span = self.source_span(ns, ne);
        let block = self.block_at(span.start);
        Evidence {
            page: Some(self.page.index),
            zone: block.map_or(Zone::Unknown, |b| b.zone),
            span: Some(span),
            bbox: block.and_then(|b| b.bbox).map(|b: BBox| b),
        }
    }

    /// Evidence for tokens `a..=b`.
    pub fn evidence_toks(&self, a: usize, b: usize) -> Evidence {
        match (self.toks.get(a), self.toks.get(b)) {
            (Some(x), Some(y)) => self.evidence(x.ns, y.ne),
            _ => Evidence {
                page: Some(self.page.index),
                ..Evidence::default()
            },
        }
    }
}

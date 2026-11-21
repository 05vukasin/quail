//! One page as a token stream: normalized text, tokens with folded keys, line numbers and
//! zones, and the way back to source spans.
//!
//! The normalized text, its map and the folded tokens come from `text.stats`'s
//! `normalized_text` artifact when the caller passes the page from there
//! ([`PageWords::with_shared`]); else they are computed here, with the same result.

use std::borrow::Cow;

use quail_core::{Page, Span, Zone};
use quail_text::NormalizedPage;
use quail_text::fold::{FoldOptions, fold_into};
use quail_text::normalize::{NormalizeOptions, OffsetMap, normalize_mapped};
use quail_text::token::{TokenKind, tokens_with_symbols};

/// One token of the normalized page text.
#[derive(Clone, Debug)]
pub struct Tok {
    /// First byte in the normalized text.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
    /// Kind.
    pub kind: TokenKind,
    /// Folded key, `start..end` in [`PageWords::keys`].
    key: (u32, u32),
    /// Zero-based line in the normalized text.
    pub line: u32,
    /// Whitespace separates it from the previous token.
    pub space_before: bool,
    /// A column gap separates it from the previous token on the same line: six spaces or
    /// more, or a tab, in the page text (`Snap Technologies      United Airlines, Inc.`).
    pub column_gap: bool,
    /// Zone of the block it sits in (`Unknown` without blocks).
    pub zone: Zone,
}

/// A page ready for scanning.
pub struct PageWords<'a> {
    /// Normalized page text.
    pub text: Cow<'a, str>,
    /// Map from normalized offsets back to the page text.
    pub map: Cow<'a, OffsetMap>,
    /// Tokens in order.
    pub toks: Vec<Tok>,
    /// All folded keys, concatenated.
    keys: String,
    /// Lines of the page.
    pub lines: u32,
}

/// Whitespace characters (in the page text) that make a column gap between two tokens.
const COLUMN_GAP: usize = 6;

impl PageWords<'static> {
    /// Tokenizes `page.text` (at most `budget` bytes of it, on a char boundary).
    pub fn new(page: &Page, budget: usize) -> Self {
        PageWords::with_shared(page, budget, None)
    }
}

impl<'a> PageWords<'a> {
    /// [`PageWords::new`], taking the normalized page from `shared` (the page's entry of the
    /// `normalized_text` artifact) when it is given and the whole page fits in `budget`.
    pub fn with_shared(page: &Page, budget: usize, shared: Option<&'a NormalizedPage>) -> Self {
        let mut src_end = budget.min(page.text.len());
        while !page.text.is_char_boundary(src_end) {
            src_end -= 1;
        }
        let src = &page.text[..src_end];
        let shared = shared.filter(|s| src.len() == page.text.len() && s.source_len() == src.len());
        let (text, map) = if let Some(s) = shared {
            (Cow::Borrowed(s.text()), Cow::Borrowed(s.map()))
        } else {
            let mut text = String::new();
            let mut map = OffsetMap::new();
            normalize_mapped(src, &NormalizeOptions::default(), &mut text, &mut map);
            (Cow::Owned(text), Cow::Owned(map))
        };
        let zones = ZoneIndex::new(page);
        let mut toks = Vec::new();
        let mut keys = String::with_capacity(text.len());
        let mut line = 0u32;
        let mut prev_end = 0usize;
        let mut push =
            |start: usize, end: usize, kind: TokenKind, key: &mut dyn FnMut(&mut String)| {
                let gap = text.get(prev_end..start).unwrap_or("");
                line += u32::try_from(gap.bytes().filter(|&b| b == b'\n').count()).unwrap_or(0);
                let k0 = keys.len();
                key(&mut keys);
                let column_gap = prev_end > 0 && !gap.contains('\n') && !gap.is_empty() && {
                    let (a, b) = (map.to_source(prev_end), map.to_source(start));
                    src.get(a..b).is_some_and(|g| {
                        g.contains('\t')
                            || g.chars().filter(|c| c.is_whitespace()).count() >= COLUMN_GAP
                    })
                };
                let zone = if zones.spans.is_empty() {
                    Zone::Unknown
                } else {
                    zones.zone_at(map.to_source(start))
                };
                toks.push(Tok {
                    start,
                    end,
                    kind,
                    key: (to_u32(k0), to_u32(keys.len())),
                    line,
                    space_before: !gap.is_empty() || prev_end == 0,
                    column_gap,
                    zone,
                });
                prev_end = end;
            };
        if let Some(s) = shared {
            for (i, t) in s.tokens().iter().enumerate() {
                push(t.start(), t.end(), t.kind, &mut |k| k.push_str(s.key(i)));
            }
        } else {
            for t in tokens_with_symbols(&text) {
                push(t.start, t.end, t.kind, &mut |k| {
                    fold_into(t.text, &FoldOptions::KEY, k);
                });
            }
        }
        Self {
            text,
            map,
            toks,
            keys,
            lines: line + 1,
        }
    }

    /// Number of tokens.
    pub fn len(&self) -> usize {
        self.toks.len()
    }

    /// No tokens.
    pub fn is_empty(&self) -> bool {
        self.toks.is_empty()
    }

    /// Folded key of token `i` (empty out of range).
    pub fn key(&self, i: usize) -> &str {
        self.toks.get(i).map_or("", |t| {
            self.keys
                .get(t.key.0 as usize..t.key.1 as usize)
                .unwrap_or("")
        })
    }

    /// Normalized text of token `i`.
    pub fn text(&self, i: usize) -> &str {
        self.toks
            .get(i)
            .map_or("", |t| self.text.get(t.start..t.end).unwrap_or(""))
    }

    /// Whether token `i` is a word, number or code.
    pub fn is_word(&self, i: usize) -> bool {
        self.toks.get(i).is_some_and(|t| {
            matches!(
                t.kind,
                TokenKind::Word | TokenKind::Alnum | TokenKind::Number
            )
        })
    }

    /// Whether token `i` is the punctuation character `c`.
    pub fn is_punct(&self, i: usize, c: char) -> bool {
        self.toks.get(i).is_some_and(|t| {
            matches!(t.kind, TokenKind::Punct | TokenKind::Symbol) && self.text(i).starts_with(c)
        })
    }

    /// Whether a line break separates tokens `i - 1` and `i`.
    pub fn breaks_before(&self, i: usize) -> bool {
        i == 0 || i >= self.toks.len() || self.toks[i].line != self.toks[i - 1].line
    }

    /// Normalized text of tokens `a..b`.
    pub fn slice(&self, a: usize, b: usize) -> &str {
        if a >= b || b > self.toks.len() {
            return "";
        }
        self.text
            .get(self.toks[a].start..self.toks[b - 1].end)
            .unwrap_or("")
    }

    /// Source span (in the page text) of tokens `a..b`.
    pub fn source_span(&self, a: usize, b: usize) -> Option<Span> {
        if a >= b || b > self.toks.len() {
            return None;
        }
        let span = Span {
            start: to_u32(self.toks[a].start),
            end: to_u32(self.toks[b - 1].end),
        };
        Some(self.map.span_to_source(span))
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Block zones by source offset.
struct ZoneIndex {
    spans: Vec<(u32, u32, Zone)>,
}

impl ZoneIndex {
    fn new(page: &Page) -> Self {
        let mut spans: Vec<(u32, u32, Zone)> = page
            .blocks
            .iter()
            .map(|b| (b.span.start, b.span.end, b.zone))
            .collect();
        spans.sort_by_key(|s| s.0);
        Self { spans }
    }

    fn zone_at(&self, src: usize) -> Zone {
        let pos = to_u32(src);
        let i = self.spans.partition_point(|s| s.0 <= pos);
        if i == 0 {
            return Zone::Unknown;
        }
        let (start, end, zone) = self.spans[i - 1];
        if pos >= start && pos < end {
            zone
        } else {
            Zone::Unknown
        }
    }
}

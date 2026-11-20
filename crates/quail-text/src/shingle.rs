//! Word shingles and character n-grams as streams of stable 64-bit hashes, the input of
//! MinHash/SimHash sketches.
//!
//! # Hash stability
//!
//! Stored sketches are compared across machines and releases, so every hash here is
//! **XXH3-64** (the xxHash 0.8 specification, frozen since 2020, identical on every platform
//! and endianness) with the fixed seed [`SEED`]:
//!
//! - a token hash is `XXH3-64(seed, UTF-8 bytes of the token)` ([`hash_str`]);
//! - a word shingle of `k` tokens is `XXH3-64(seed, h1 ‖ h2 ‖ … ‖ hk)` where `hi` are the
//!   token hashes as 8 little-endian bytes each;
//! - a char n-gram is `XXH3-64(seed, UTF-8 bytes of the n characters)`.
//!
//! The values are pinned by tests; changing any of this is a breaking change that needs a
//! new analyzer version for every sketch built on it.
//!
//! A sequence shorter than `k` (or `n`) yields exactly one gram covering all of it, so short
//! texts still get a non-empty sketch; an empty sequence yields nothing.

use std::collections::VecDeque;

use xxhash_rust::xxh3::xxh3_64_with_seed;

use crate::fold::{FoldOptions, fold_into};
use crate::token::{Tokens, tokens};

/// Seed of every hash in this module (`"quail\0\0\x01"`). Never change it.
pub const SEED: u64 = 0x7175_6169_6c00_0001;

/// Stable 64-bit hash of bytes (XXH3-64 with [`SEED`]).
#[inline]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    xxh3_64_with_seed(bytes, SEED)
}

/// Stable 64-bit hash of a string's UTF-8 bytes.
#[inline]
pub fn hash_str(s: &str) -> u64 {
    hash_bytes(s.as_bytes())
}

/// Hashes of `k`-token windows over a stream of token hashes. See [`word_shingles`].
pub struct Shingles<I> {
    inner: I,
    k: usize,
    window: VecDeque<u64>,
    buf: Vec<u8>,
    emitted: bool,
    done: bool,
}

impl<I: Iterator<Item = u64>> Shingles<I> {
    /// Shingles over token hashes (already computed with [`hash_str`]). `k` of 0 is treated
    /// as 1.
    pub fn from_hashes(inner: I, k: usize) -> Self {
        let k = k.max(1);
        Self {
            inner,
            k,
            window: VecDeque::with_capacity(k),
            buf: Vec::with_capacity(k * 8),
            emitted: false,
            done: false,
        }
    }

    fn hash_window(&mut self) -> u64 {
        self.buf.clear();
        for h in &self.window {
            self.buf.extend_from_slice(&h.to_le_bytes());
        }
        hash_bytes(&self.buf)
    }
}

impl<I: Iterator<Item = u64>> Iterator for Shingles<I> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        if self.done {
            return None;
        }
        for h in self.inner.by_ref() {
            if self.window.len() == self.k {
                self.window.pop_front();
            }
            self.window.push_back(h);
            if self.window.len() == self.k {
                self.emitted = true;
                return Some(self.hash_window());
            }
        }
        self.done = true;
        if !self.emitted && !self.window.is_empty() {
            self.emitted = true;
            return Some(self.hash_window());
        }
        None
    }
}

/// Word shingles of `k` consecutive tokens, hashed. The tokens are used as given: fold
/// them first (or use [`text_shingles`]) when case and diacritics must not matter.
///
/// ```
/// use quail_text::shingle::word_shingles;
/// let a: Vec<u64> = word_shingles(["ugovor", "o", "zakupu", "stana"], 3).collect();
/// assert_eq!(a.len(), 2);
/// ```
pub fn word_shingles<I, S>(tokens: I, k: usize) -> Shingles<impl Iterator<Item = u64>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    Shingles::from_hashes(tokens.into_iter().map(|t| hash_str(t.as_ref())), k)
}

/// Token hashes of a text: [`tokens`] (words, numbers, codes), each folded with `opts` into
/// a reused buffer, then hashed. No allocation per token.
pub struct FoldedTokenHashes<'a> {
    tokens: Tokens<'a>,
    opts: FoldOptions,
    buf: String,
}

impl Iterator for FoldedTokenHashes<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        let t = self.tokens.next()?;
        self.buf.clear();
        fold_into(t.text, &self.opts, &mut self.buf);
        Some(hash_str(&self.buf))
    }
}

/// Folded token hashes of `text`, see [`FoldedTokenHashes`].
pub fn token_hashes<'a>(text: &'a str, opts: &FoldOptions) -> FoldedTokenHashes<'a> {
    FoldedTokenHashes {
        tokens: tokens(text),
        opts: *opts,
        buf: String::with_capacity(32),
    }
}

/// Word shingles of a text: tokenize, fold each token with `opts`, shingle with `k`.
/// With [`FoldOptions::KEY`], Latin and Cyrillic spellings of the same Serbian text give
/// the same shingles.
pub fn text_shingles<'a>(
    text: &'a str,
    k: usize,
    opts: &FoldOptions,
) -> Shingles<FoldedTokenHashes<'a>> {
    Shingles::from_hashes(token_hashes(text, opts), k)
}

/// Hashes of every window of `n` consecutive characters of a text. See [`char_ngrams`].
pub struct CharNgrams<'a> {
    text: &'a str,
    chars: std::str::CharIndices<'a>,
    n: usize,
    starts: VecDeque<usize>,
    emitted: bool,
    done: bool,
}

impl Iterator for CharNgrams<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        if self.done {
            return None;
        }
        for (i, c) in self.chars.by_ref() {
            if self.starts.len() == self.n {
                self.starts.pop_front();
            }
            self.starts.push_back(i);
            if self.starts.len() == self.n {
                let s = self.starts.front().copied().unwrap_or(0);
                self.emitted = true;
                return Some(hash_str(&self.text[s..i + c.len_utf8()]));
            }
        }
        self.done = true;
        if !self.emitted && !self.text.is_empty() {
            self.emitted = true;
            return Some(hash_str(self.text));
        }
        None
    }
}

/// Character n-grams of `text` (Unicode scalar values, not bytes), hashed. The window
/// slides over the text as given, whitespace included: normalize and fold first when that
/// matters. `n` of 0 is treated as 1. No allocation per gram.
///
/// ```
/// use quail_text::shingle::char_ngrams;
/// assert_eq!(char_ngrams("čačak", 3).count(), 3);
/// ```
pub fn char_ngrams(text: &str, n: usize) -> CharNgrams<'_> {
    let n = n.max(1);
    CharNgrams {
        text,
        chars: text.char_indices(),
        n,
        starts: VecDeque::with_capacity(n),
        emitted: false,
        done: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_values() {
        // These values are part of the stored-sketch format. Do not update them casually.
        assert_eq!(hash_str("ugovor"), PINNED_TOKEN);
        let s: Vec<u64> = word_shingles(["ugovor", "o", "zakupu"], 3).collect();
        assert_eq!(s, [PINNED_SHINGLE]);
        let g: Vec<u64> = char_ngrams("abc", 3).collect();
        assert_eq!(g, [PINNED_NGRAM]);
    }

    const PINNED_TOKEN: u64 = 9_236_387_530_261_943_639;
    const PINNED_SHINGLE: u64 = 441_870_768_933_042_675;
    const PINNED_NGRAM: u64 = 11_421_930_272_970_557_661;

    #[test]
    fn xxh3_matches_the_specification() {
        // Reference vector of XXH3-64 (empty input, seed 0) from the xxHash test suite.
        assert_eq!(xxh3_64_with_seed(b"", 0), 0x2D06_8005_38D3_94C2);
    }

    #[test]
    fn shingle_counts() {
        let toks = ["a", "b", "c", "d", "e"];
        assert_eq!(word_shingles(toks, 3).count(), 3);
        assert_eq!(word_shingles(toks, 5).count(), 1);
        assert_eq!(word_shingles(toks, 9).count(), 1);
        assert_eq!(word_shingles(toks, 0).count(), 5);
        assert_eq!(word_shingles(Vec::<&str>::new(), 3).count(), 0);
        // A short sequence's gram is the hash of all of it.
        let short: Vec<u64> = word_shingles(["a", "b"], 3).collect();
        let full: Vec<u64> = word_shingles(["a", "b"], 2).collect();
        assert_eq!(short, full);
    }

    #[test]
    fn order_matters() {
        let a: Vec<u64> = word_shingles(["a", "b"], 2).collect();
        let b: Vec<u64> = word_shingles(["b", "a"], 2).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn scripts_agree_after_folding() {
        let lat: Vec<u64> =
            text_shingles("Ugovor o zakupu poslovnog prostora", 3, &FoldOptions::KEY).collect();
        let cyr: Vec<u64> =
            text_shingles("УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА", 3, &FoldOptions::KEY).collect();
        assert_eq!(lat, cyr);
        assert_eq!(lat.len(), 3);
    }

    #[test]
    fn ngrams() {
        assert_eq!(char_ngrams("abcd", 2).count(), 3);
        assert_eq!(char_ngrams("ab", 5).count(), 1);
        assert_eq!(char_ngrams("", 3).count(), 0);
        let a: Vec<u64> = char_ngrams("abab", 2).collect();
        assert_eq!(a[0], a[2]);
        assert_eq!(char_ngrams("ab", 5).next(), Some(hash_str("ab")));
    }
}

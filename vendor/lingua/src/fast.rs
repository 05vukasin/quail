// quail: new file. Hash-table lookups for the probability models, built once per detector.
//
// Upstream looks up every n-gram of a text once per language: a `DashMap` probe (shard
// read lock, SipHash) for the model, then an `fst::Map::get`. With about 600 distinct
// trigrams per 3 000-character text, each with up to three backoff levels, and 11
// languages, that is tens of thousands of fst walks per call and 94 % of quail's
// `text.lang` time.
//
// `NgramTable` holds every 1-, 2- and 3-gram of the detector's probability models in one
// hash map from the packed n-gram to a dense row with one slot per language (the exact
// `u64` bits of the model's `f64`, or `ABSENT`). One probe per n-gram serves all languages.
// 4- and 5-grams (used only for texts under 120 characters) still go to the fst.
//
// The values are the model's own bits, so every per-language sum is computed from the same
// numbers in the same order as before: output is unchanged.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use fst::{Automaton, IntoStreamer, Streamer};

use crate::file::read_probability_model_data_file;
use crate::language::Language;
use crate::model::NGRAM_PROBABILITY_MODEL_FILE_NAME;

/// Longest n-gram (in characters) kept in the table.
pub(crate) const MAX_TABLE_CHARS: usize = 3;

/// Detectors with more languages than this keep upstream's lookups (the dense rows would
/// grow with the language count).
pub(crate) const MAX_TABLE_LANGUAGES: usize = 16;

/// Empty slot. A model value is `ln` of a relative frequency in (0, 1], never NaN.
const ABSENT: u64 = u64::MAX;

/// Packs an n-gram of 1 to 5 characters into a `u128`: 21 bits per character (code point
/// plus one, so no character packs to zero), first character lowest. Injective on strings
/// of at most five characters. `None` for longer strings.
pub(crate) fn pack(ngram: &str) -> Option<u128> {
    let mut key = 0u128;
    for (i, c) in ngram.chars().enumerate() {
        if i == 5 {
            return None;
        }
        key |= (u128::from(c) + 1) << (21 * i);
    }
    Some(key)
}

/// Multiply-fold hasher for already packed integer keys. Not DoS-resistant: the table's
/// keys come from the shipped models, and per-call sets are bounded by the text sample.
#[derive(Default, Clone, Copy)]
pub(crate) struct FoldHasher(u64);

impl Hasher for FoldHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(b));
        }
    }

    fn write_u64(&mut self, x: u64) {
        let p = u128::from(self.0 ^ x) * 0x9E37_79B9_7F4A_7C15_u128;
        self.0 = (p as u64) ^ ((p >> 64) as u64);
    }

    fn write_u128(&mut self, x: u128) {
        self.write_u64(x as u64);
        self.write_u64((x >> 64) as u64);
    }
}

pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FoldHasher>>;
pub(crate) type FastSet<K> = HashSet<K, BuildHasherDefault<FoldHasher>>;

/// fst automaton accepting keys of 1 to `MAX_TABLE_CHARS` UTF-8 characters; it prunes the
/// traversal at the first byte of a longer key.
struct ShortKeys;

impl Automaton for ShortKeys {
    type State = usize;

    fn start(&self) -> usize {
        0
    }

    fn is_match(&self, chars: &usize) -> bool {
        (1..=MAX_TABLE_CHARS).contains(chars)
    }

    fn can_match(&self, chars: &usize) -> bool {
        *chars <= MAX_TABLE_CHARS
    }

    fn accept(&self, chars: &usize, byte: u8) -> usize {
        // A byte that is not a UTF-8 continuation byte starts a new character.
        if byte & 0xC0 == 0x80 {
            *chars
        } else {
            chars + 1
        }
    }
}

/// Every short n-gram of a detector's probability models, one dense row per n-gram.
pub(crate) struct NgramTable {
    columns: Vec<Language>,
    index: FastMap<u64, u32>,
    rows: Vec<u64>,
}

impl NgramTable {
    /// Builds the table for `languages` (in the given order). Languages without a model
    /// get an empty column, as upstream finds no probability for them.
    pub(crate) fn build(languages: &[Language]) -> Self {
        let width = languages.len();
        let mut index: FastMap<u64, u32> = FastMap::default();
        let mut rows: Vec<u64> = Vec::new();
        for (col, language) in languages.iter().enumerate() {
            let Ok(model) =
                read_probability_model_data_file(*language, NGRAM_PROBABILITY_MODEL_FILE_NAME)
            else {
                continue;
            };
            let mut stream = model.search(ShortKeys).into_stream();
            while let Some((key, value)) = stream.next() {
                let Some(packed) = std::str::from_utf8(key).ok().and_then(pack) else {
                    continue;
                };
                // At most three characters: 63 bits.
                let packed = packed as u64;
                let next = (rows.len() / width) as u32;
                let row = *index.entry(packed).or_insert_with(|| {
                    rows.resize(rows.len() + width, ABSENT);
                    next
                });
                rows[row as usize * width + col] = value;
            }
        }
        index.shrink_to_fit();
        rows.shrink_to_fit();
        Self {
            columns: languages.to_vec(),
            index,
            rows,
        }
    }

    /// Column of `language`, if the table has one.
    pub(crate) fn column(&self, language: Language) -> Option<usize> {
        self.columns.iter().position(|l| *l == language)
    }

    /// The row of an n-gram of at most `MAX_TABLE_CHARS` characters (given packed).
    #[inline]
    pub(crate) fn row(&self, packed: u128) -> Option<&[u64]> {
        let width = self.columns.len();
        self.index.get(&(packed as u64)).map(|&r| {
            let start = r as usize * width;
            &self.rows[start..start + width]
        })
    }

    /// Value of one slot: the model's `f64`, or `None`.
    #[inline]
    pub(crate) fn value(row: &[u64], col: usize) -> Option<f64> {
        let bits = row[col];
        (bits != ABSENT).then(|| f64::from_bits(bits))
    }
}

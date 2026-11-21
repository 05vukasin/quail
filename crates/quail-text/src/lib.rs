//! Text utilities shared by every text analyzer, plus text-level analyzers.
//!
//! Namespace: `text.*`.
//!
//! # Utilities
//!
//! | module | what |
//! |---|---|
//! | [`normalize`] | NFKC, invisible characters, quotes/dashes, whitespace, de-hyphenation; offset map back to the source |
//! | [`fold`] | matching keys: lowercase, diacritics, Serbian Cyrillic → Latin |
//! | [`translit`] | exact Serbian Cyrillic ⇄ Latin |
//! | [`script`] | Latin / Cyrillic / Greek / other letter ratios |
//! | [`token`] | UAX #29 tokens with byte spans and kinds, number-, IBAN-, URL- and e-mail-aware; keyed tokens |
//! | [`shingle`] | word shingles and char n-grams as stable XXH3-64 hashes |
//! | [`garbage`] | how broken a text layer looks: `Clean`, `Garbage` or `TooShort` |
//! | [`lang`] | language identification with top candidates |
//! | [`money`] | numbers and amounts as written (`1,200.50`, `1.200,50`, `1 200,50`, `(€1,200.50)`, `5 miliona din.`) as exact decimals; currency symbols, words and codes |
//!
//! Everything borrows where it can ([`std::borrow::Cow`] results, iterators over `&str`,
//! `*_into` variants that append to a caller's buffer) and nothing keeps global state.
//!
//! # Analyzers
//!
//! - [`TextStats`] (`text.stats@1`, Fast): characters, words, pages with text, script
//!   ratios, garbage ratio.
//! - [`TextLang`] (`text.lang@1`, Fast): document language with top-3 candidates.
//!
//! Both require the `document` artifact. Keys are in [`keys`].

pub mod fold;
pub mod garbage;
pub mod lang;
pub mod money;
pub mod normalize;
pub mod script;
pub mod shingle;
pub mod token;
pub mod translit;

mod analyzers;

pub use analyzers::{TextLang, TextStats};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// Characters of the document text, whitespace excluded (Int).
    pub const CHARS: FactKey = FactKey::new("text.chars");
    /// Word-like tokens (words, numbers, codes) of the document text (Int).
    pub const WORDS: FactKey = FactKey::new("text.words");
    /// Pages with at least one non-whitespace character (Int).
    pub const PAGES_WITH_TEXT: FactKey = FactKey::new("text.pages_with_text");
    /// Dominant script of the letters: `latin`, `cyrillic`, `greek`, `other`, or `mixed`
    /// when no script has 80 % of the letters (Text). Absent without letters.
    pub const SCRIPT: FactKey = FactKey::new("text.script");
    /// Share of letters in Latin script (Float, 0..=1).
    pub const SCRIPT_LATIN: FactKey = FactKey::new("text.script.latin");
    /// Share of letters in Cyrillic script (Float, 0..=1).
    pub const SCRIPT_CYRILLIC: FactKey = FactKey::new("text.script.cyrillic");
    /// Share of letters in Greek script (Float, 0..=1).
    pub const SCRIPT_GREEK: FactKey = FactKey::new("text.script.greek");
    /// Share of letters in any other script (Float, 0..=1).
    pub const SCRIPT_OTHER: FactKey = FactKey::new("text.script.other");
    /// Document-level garbage ratio, see [`crate::garbage`] (Float, 0..=1).
    pub const GARBAGE_RATIO: FactKey = FactKey::new("text.garbage_ratio");
    /// Pages with enough text to judge whose garbage ratio exceeds
    /// [`crate::garbage::NEEDS_OCR_RATIO`] (Int).
    pub const GARBAGE_PAGES: FactKey = FactKey::new("text.garbage_pages");
    /// Most likely language, ISO 639-1 (`sr`, `hr`, `en`, ...); `conf` is its score (Text).
    pub const LANG: FactKey = FactKey::new("text.lang");
    /// Top candidates as a list of `[code, score]` pairs, best first (List).
    pub const LANG_TOP: FactKey = FactKey::new("text.lang_top");
    /// Language of one page, with the page as evidence; `conf` is its score (Text).
    pub const PAGE_LANG: FactKey = FactKey::new("text.page_lang");
}

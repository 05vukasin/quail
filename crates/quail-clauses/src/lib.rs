//! Contract clauses. Namespace `clause.*`.
//!
//! One analyzer, [`ClauseTerms`] (`clause.terms@1`, `Fast`, requires `document`, runs after
//! `revised_document`), reads the document's text as one token stream (clauses wrap over line
//! and page breaks) and reports *candidates*: each fact has a `conf` and
//! [`Evidence`](quail_core::Evidence) with the page and the byte span of the phrase.
//!
//! | key | value |
//! |---|---|
//! | [`keys::GOVERNING_LAW`] | Record `{jurisdiction, code, raw, basis}`: `Delaware`, `US-DE`, the phrase as written, `laws_of` / `place_law` / `bare_laws_of` / `forum` |
//! | [`keys::TERM`] | Record `{duration, text, raw, renewal}`: `P2Y`, `2 years`, the phrase, `auto` / `none` / `unknown` |
//! | [`keys::SURVIVAL`] | Record `{duration, text, raw}`: how long obligations survive (`5 years`) |
//! | [`keys::NOTICE_PERIOD`] | Record `{duration, text, raw}`: `30 days' prior written notice` |
//! | [`keys::TERMINATION`] | Record `{kind, raw}`: `convenience`, `breach`, `insolvency`, `change_of_control` |
//!
//! # How it works
//!
//! - **Governing law** ([`law`]): `law`/`laws` with a governing word (`governed`,
//!   `construed`, `interpreted`, `Governing Law`) at most 14 tokens before it, then `of the
//!   State / Commonwealth / Province of` and a place from [`places::PLACES`] (US states, DC,
//!   Canadian provinces, Australian states, UK nations, countries), longest name first; or a
//!   place right before `law` (`governed by Delaware law`). `the laws of the United States and
//!   the State of X` gives X. `the laws of X` without a governing word (an incorporation
//!   statement) and the forum (`courts of the State of X`) are kept at a low confidence.
//! - **Durations** ([`duration`]): `two (2) years`, `2 (two) years`, `twenty-four (24)
//!   months`, `a two-year period`, `thirty (30) calendar days`, `5.5 months`.
//! - **Term** ([`term`]): a duration in a sentence with `term`, `period`, `in effect` /
//!   `in full force`, `expire` / `terminate` / `remain`, `this Agreement`, or followed by
//!   `from the Effective Date`. Deadlines (`within 30 days`, `at least`, `per year`) are not
//!   terms; days and weeks score lower. Durations of surviving obligations (`obligations of
//!   confidentiality shall survive for five years`) are [`keys::SURVIVAL`], and durations
//!   before `notice` are [`keys::NOTICE_PERIOD`]. Renewal comes from the words around the
//!   term (`automatically renewed`, `successive`, `renewal term`; `shall not be renewed`).
//!
//! # Limits
//!
//! Reads at most `Limits::max_text_bytes` (and [`MAX_SCAN_BYTES`]) of text, skips pages that
//! need OCR, checks the deadline per page and keeps what it read. Every rule looks a bounded
//! number of tokens around an anchor, so the scan is linear. At most [`MAX_PER_KEY`] values
//! per key.

// Rules walk token indices (`s` stream, `i`/`j`/`k` tokens, `a`/`b` ranges, `d` a duration):
// short names read better there than long ones.
#![allow(clippy::many_single_char_names)]

pub mod duration;
pub mod law;
pub mod places;
pub mod term;
pub mod tokens;

mod analyzer;

pub use analyzer::{ClauseTerms, MAX_PER_KEY, MAX_RAW_CHARS, MAX_SCAN_BYTES};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// Governing law (Record: `jurisdiction`, `code`, `raw`, `basis`).
    pub const GOVERNING_LAW: FactKey = FactKey::new("clause.governing_law");
    /// Term of the agreement (Record: `duration`, `text`, `raw`, `renewal`).
    pub const TERM: FactKey = FactKey::new("clause.term");
    /// How long obligations survive (Record: `duration`, `text`, `raw`).
    pub const SURVIVAL: FactKey = FactKey::new("clause.survival");
    /// A notice period (Record: `duration`, `text`, `raw`).
    pub const NOTICE_PERIOD: FactKey = FactKey::new("clause.notice_period");
    /// A termination right (Record: `kind`, `raw`).
    pub const TERMINATION: FactKey = FactKey::new("clause.termination");
}

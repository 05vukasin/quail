//! Document references: the candidates a service needs to link documents into contract
//! families (MSA → SOW / annex / amendment → PO → invoice) and multi-part documents.
//!
//! Namespace: `ref.*`. One analyzer, [`RefAnalyzer`] (`ref.references@3`, `Fast`, requires
//! `document`). Keys are in [`keys`]; the number rules are in [`number`].
//!
//! # What it finds
//!
//! - **Own numbers** (`ref.doc_number`): the contract, agreement, SOW, PO, invoice or order
//!   number of this document, from labels ("Ugovor br.", "Broj ugovora", "Уговор бр.",
//!   "Contract No.", "Agreement Number", "PO Number", "Invoice No", "Faktura broj",
//!   "Račun br."), title lines, and head lines ("Broj: ...", "Ref. No.: ...").
//! - **Cited documents** (`ref.cites`): "Aneks br. 2 uz Ugovor br. 114/23", "Amendment No. 2
//!   to the Master Services Agreement dated 1 March 2021", "pursuant to SOW #12", "issued under
//!   Purchase Order 4500012345", with the cited kind, number, ordinal, date and relation.
//! - **Self-description**: `ref.title`, `ref.instrument_kind`, `ref.ordinal`, and per page
//!   `ref.page_marker` ("Strana 2 od 5", "Page 2 of 5").
//! - **Version markers** (`ref.version_hint`): "v2", "rev. 3", "Draft", "Final", "Execution
//!   version", "Redline".
//! - **Scoping ids**: `ref.study_code` (clinical study codes) and `ref.clm_record` (`con_<n>`).
//!
//! # Own or cited
//!
//! A labelled number is the document's own when its kind fits the document's instrument kind
//! (from the title; else from the first labelled number at the head of the first page): the
//! "PO Number" on an invoice is cited (`billed_against`), on a purchase order it is its own.
//! A trigger before the kind phrase ("pursuant to", "under", "uz", "to the", "na osnovu")
//! always makes a citation and names the relation; without one, the relation follows from
//! the two kinds (an amendment `amends` its contract, an invoice is `billed_against` its order,
//! a SOW is `issued_under` its MSA, else `references`). "This Agreement" / "ovaj Ugovor" is
//! the document itself.
//!
//! # Record values
//!
//! Structured facts carry a [`Value::Record`](quail_core::Value) (an ordered JSON object of
//! `snake_case` parts), read with [`Value::field`](quail_core::Value::field) or [`field`].
//! Absent parts are left out.
//!
//! | key | parts |
//! |---|---|
//! | `ref.doc_number` | `kind`, `raw` (as written), `key` (matching key), `base` (key without a version tail), `date` |
//! | `ref.cites` | `kind`, `relation`, `raw`, `key`, `base`, `ordinal` (Int), `date` (Date) |
//! | `ref.page_marker` | `n`, `of` (Int) |
//! | `ref.version_hint` | `kind` (`version`, `revision`, `draft`, `final`, `execution`, `redline`, `clean`), `n` (Text) |
//!
//! Kinds: `msa contract nda sow amendment amended_restated annex change_order side_letter po
//! order invoice quote framework_agreement award_decision decision price_list notice
//! tender_documentation`; own numbers may also be `registry` (a Serbian head "Broj:" number)
//! and cites `procurement` (a public procurement procedure), and `unknown` marks an own number
//! found without a kind. Serbian *aneks*
//! is an amendment (izmena ugovora), *prilog* an annex. Relations: `amends annex_to
//! issued_under billed_against references`.
//!
//! # How
//!
//! Per page: [`quail_text::normalize`] with its offset map, then [`quail_text::token`] keyed
//! tokens folded to a *key line* (lowercase, no diacritics, Cyrillic → Latin), so one pattern
//! covers "UGOVOR", "Ugovor", "Уговор" and "ugovor". Patterns are `regex` automata over the key
//! line (linear time); values are read from the normalized text, evidence is mapped back to
//! the original page text (with the block's zone and box).
//!
//! The normalized page, its offset map and its keyed tokens are taken from `text.stats`'s
//! `normalized_text` artifact ([`quail_text::NormalizedText`]) when it holds the page and the
//! page is read whole (the analyzer declares a soft `after(&["normalized_text"])`); else they
//! are computed here. The output is the same either way.

// Token indices are `a`/`b` (first/last token) and `i`/`j` throughout, by design.
#![allow(clippy::many_single_char_names)]

mod analyzer;
mod kind;
mod markers;
pub mod number;
mod patterns;
mod prep;
mod scan;
mod title;

pub use analyzer::{MAX_CITES, MAX_DOC_NUMBERS, MAX_IDS, MAX_VERSION_HINTS, RefAnalyzer};
pub use kind::{Kind, Relation, field};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// An own number of this document (record: `kind`, `raw`, `key`, `base`, `date`).
    pub const DOC_NUMBER: FactKey = FactKey::new("ref.doc_number");
    /// A document this one cites (record: `kind`, `relation`, `raw`, `key`, `base`,
    /// `ordinal`, `date`).
    pub const CITES: FactKey = FactKey::new("ref.cites");
    /// The document title (Text).
    pub const TITLE: FactKey = FactKey::new("ref.title");
    /// The instrument kind the title (or the first own label) names (Text, see [`crate::Kind`]).
    pub const INSTRUMENT_KIND: FactKey = FactKey::new("ref.instrument_kind");
    /// The ordinal of the instrument: Amendment No. 3 → 3 (Int).
    pub const ORDINAL: FactKey = FactKey::new("ref.ordinal");
    /// A page-of-page marker, one per page at most (record: `n`, `of`).
    pub const PAGE_MARKER: FactKey = FactKey::new("ref.page_marker");
    /// A version marker (record: `kind`, `n`).
    pub const VERSION_HINT: FactKey = FactKey::new("ref.version_hint");
    /// A clinical study code (`D\d{3}[0-9A-Z]C\d{5}`), upper-cased (Text).
    pub const STUDY_CODE: FactKey = FactKey::new("ref.study_code");
    /// A CLM record id, `con_<digits>` (Text).
    pub const CLM_RECORD: FactKey = FactKey::new("ref.clm_record");
}

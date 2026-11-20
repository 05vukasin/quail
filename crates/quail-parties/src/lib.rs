//! Per-file candidates for the companies a document is about, and the roles it gives them.
//!
//! Namespace: `party.*`. One analyzer, [`PartyCandidates`] (`party.candidates@1`, `Fast`,
//! requires `document`). It reports candidates, never verdicts: it does not say which company
//! owns a file and it never compares files. A resolution service joins the keys across files
//! (union-find on the `key` field of [`keys::COMPANY`], [`keys::ALIAS_OF`] pairs) and decides.
//!
//! Identifiers (tax and registration numbers, IBANs, e-mail domains), dates and amounts are
//! `quail-signals`' scope; document numbers, references and titles are `quail-refs'`.
//!
//! # How a company is found
//!
//! 1. **Legal form anchors** ([`forms::FORMS`]): `d.o.o.`, `a.d.`, `DOO`, `PR`, `JP`,
//!    `LLC`, `Inc.`, `Ltd`, `GmbH`, `S.A.`, `S.p.A.`, `B.V.`, `plc`, `Kft.`, `s.r.o.`,
//!    `Sp. z o.o.`, ... in Latin or Cyrillic, dotted, spaced or not. The name is walked from
//!    the form over capitalised words on the same line (at most 8), stopping at document words,
//!    prepositions, labels, role words and lowercase prose; a seat city right after the form
//!    (`ACME d.o.o. Beograd`) is kept in the surface and left out of the key.
//! 2. **Role labels** (`Dobavljač:`, `Naručilac:`, `Supplier:`, `Customer:`, `Vendor:`,
//!    `Bill to:`, `Ship to:`, ...): the name after the label, on its line or the next; without
//!    a legal form only when it is a whole line value and passes every gate, and never when it
//!    has a person's shape.
//! 3. **Name statements**: the other side of `formerly`, `f/k/a`, `now known as`, `d/b/a`,
//!    `trading as`, `t/a`, `prethodno`, `ranije`, `bivše` next to a found name.
//!
//! Every surface then passes the reference name-quality gates ([`quality::verdict`]):
//! length and word count, addresses and postal codes, e-mail/URL, addressee labels, personal
//! titles, "Name, Role", prose fragments, role words alone, product tails, and names with no
//! distinctive word ("Republika Srbija", "Group Ltd").
//!
//! # Keys
//!
//! One [`keys::COMPANY`] fact per mention: a record on the mention's
//! [`Evidence`](quail_core::Evidence) (page, zone, source span), `conf` = mention confidence.
//! Fields, in this order:
//!
//! | field | value |
//! |---|---|
//! | `surface` | surface form as written (whitespace, quotes and dashes normalized) |
//! | `key` | core key: legal forms, connectors and seat city dropped, folded (Cyrillic → Latin, no diacritics, `đ → dj`) |
//! | `norm` | reference `normalized_key` (NFKD, marks and punctuation dropped; `sym-` + BLAKE2s-64 fallback) |
//! | `quality` | `ok` or `suspect` (two bare Titlecase words: a person or a brand) |
//! | `legal_form` | canonical form (`d.o.o.`, `LLC`, `GmbH`, ...); absent without one |
//! | `jurisdiction`, `jurisdiction_conf` | ISO 3166 alpha-2, lowercase, several joined by `\|`, and its confidence; absent when unknown |
//! | `roles`, `role_conf` | `supplier`, `customer`, `issuer`, `bill_to`, `ship_to`, `signatory`, `party`, `mentioned`, and a parallel list of confidences (mention conf × role weight) |
//! | `defined_term` | the term a clause defines for it (`Naručilac`, `Supplier`); absent without one |
//!
//! Document level: [`keys::CANDIDATES`] (top 10 keys by noisy-or score, a list of records
//! `{key, surface, score, roles}`), [`keys::ALIAS_OF`] (record `{current, other, kind}`, evidence
//! spans both names), [`keys::SIGNATORY_TITLE`] (Text) and [`keys::SIGNATORY_COUNT`] (Int).
//!
//! # Person names
//!
//! Personal names are never emitted on their own: signature blocks yield only titles
//! (`Direktor`, `Managing Director`) and a count of signatory lines, and a formless name with a
//! person's shape is dropped. The one exception is a sole trader's registered business name
//! (`Ime Prezime PR Agencija X`), which is the legal name of the business and is needed to
//! identify the supplier.
//!
//! # Confidence
//!
//! `conf` estimates the probability that the mention is a real company that is a party to,
//! or the issuer of, the document. Base 0.75 with a strong legal form, 0.40 with a weak one
//! (`AS`, `AD`, `Co.`, `Company`, `Group`), 0.65 / 0.60 / 0.55 without one (a brand spelled by
//! a domain of the document / across a name statement / after a role label); plus 0.12 in a
//! parties clause, 0.10 with a defined term stating a role (0.05 without), 0.06 in a
//! letterhead or header (0.03 footer), 0.15 in a signature block, 0.30 after a role label
//! (formed only), 0.05 in a heading, −0.30 for a bank in payment details; clamped to
//! 0.05..=0.97. See the README for the measured precision per band.

pub mod forms;
pub mod names;
pub mod quality;
pub mod roles;
pub mod scan;
pub mod vocab;
pub mod words;

mod analyzer;

pub use analyzer::{
    AliasPair, Candidate, Found, MAX_KEYS, MAX_MENTIONS_PER_KEY, MAX_RANKED, MAX_SCAN_BYTES,
    MAX_STATEMENTS, MAX_TITLES, PartyCandidates, candidate_record, collect, mention_conf, rank,
};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// A company mention (Record: `surface`, `key`, `norm`, `quality`, `legal_form`?,
    /// `jurisdiction`?, `jurisdiction_conf`?, `roles`, `role_conf`, `defined_term`?).
    pub const COMPANY: FactKey = FactKey::new("party.company");
    /// A name statement (Record: `current`, `other`, `kind` = `former_name`, `trading_name` or
    /// `dba`).
    pub const ALIAS_OF: FactKey = FactKey::new("party.alias_of");
    /// Ranked candidates (List of Record: `key`, `surface`, `score`, `roles`).
    pub const CANDIDATES: FactKey = FactKey::new("party.candidates");
    /// A title a signatory signs under (Text), no person name.
    pub const SIGNATORY_TITLE: FactKey = FactKey::new("party.signatory_title");
    /// Lines of signature blocks naming a signatory by title or label (Int).
    pub const SIGNATORY_COUNT: FactKey = FactKey::new("party.signatory_count");
}

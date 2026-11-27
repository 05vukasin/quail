//! Key contract and invoice facts, each with a highlight. Namespace `contract.*`.
//!
//! One analyzer, [`ContractFacts`] (`contract.facts@1`, `Fast`, requires `document`, runs
//! after `revised_document` and as a finalizer, so it reads OCR text and the facts of the
//! other steps). Serbian (Latin and Cyrillic) and English. Rules only; no model, no network.
//!
//! It adds what the other steps do not say, and reuses what they do: the company names
//! (`party.company`), validated identifiers (`signal.rs_pib`, `signal.rs_mb`, `signal.iban`,
//! `signal.rs_account`, `signal.vat`), the dates with roles (`signal.date_*`), the English
//! term, notice and governing-law clauses (`clause.*`) and the document numbers and cited
//! documents (`ref.*`). The canonical one-per-document view of all of them is the card of
//! `quail-card` (`card.*`).
//!
//! | key | value |
//! |---|---|
//! | [`keys::PARTY`] | Record `{role, label, name}`: a contracting party and its role (`supplier` or `customer`), with the role word as written |
//! | [`keys::PARTY_ID`] | Record `{role, party?, kind, value, checked}`: an identifier of a party: `pib`, `mb`, `vat`, `reg`, `iban`, `account`, `bic` |
//! | [`keys::ADDRESS`] | Record `{role, party?, text}` |
//! | [`keys::REPRESENTATIVE`] | Record `{role, party?, name?, title?}`: who represents a party (`koga zastupa`, `represented by`) |
//! | [`keys::SIGNATORY`] | Record `{role, label, name?, title?}`: a signature block (`Za Naručioca`, `For the Supplier`) |
//! | [`keys::TERM`] | Record `{iso?, months?, days?, indefinite?, text}`: the contract's duration |
//! | [`keys::RENEWAL`] | Record `{auto, iso?, same_period?, text}`: whether it renews by itself |
//! | [`keys::NOTICE`] | Record `{iso, days?, months?, text}`: the notice period |
//! | [`keys::TERMINATION`] | Record `{kind, text}`: a termination right: `breach`, `convenience`, `insolvency`, `mutual`, `force_majeure`, `notice`, `change_of_control` |
//! | [`keys::PAYMENT_DAYS`] | Record `{days, business?, from?, text}`: days to pay; `from` is `invoice_receipt`, `invoice_date`, `delivery`, `signing` |
//! | [`keys::PAYMENT_METHOD`] | Text: `bank_transfer`, `cash`, `card`, `compensation` |
//! | [`keys::ADVANCE`] | Record `{percent?, amount?, text}`: an advance payment |
//! | [`keys::INDEXATION`] | Record `{basis, text}`: price changes: `cpi`, `inflation`, `exchange_rate`, `fixed`, `other` |
//! | [`keys::LATE_INTEREST`] | Record `{statutory, percent?, text}`: interest on late payment |
//! | [`keys::PENALTY`] | Record `{percent?, per?, amount?, cap_percent?, text}`: a contractual penalty or liquidated damages |
//! | [`keys::GUARANTEE`] | Record `{kind, purpose?, percent?, amount?, text}`: `bank_guarantee`, `bill_of_exchange`, `performance_bond`, `letter_of_credit`, `parent_guarantee`, `deposit` |
//! | [`keys::LIABILITY_CAP`] | Record `{amount?, percent?, basis?, months?, text}`: a limit of liability |
//! | [`keys::CONFIDENTIALITY`] | Record `{iso?, text}`: a confidentiality obligation and how long it lasts |
//! | [`keys::SLA`] | Record `{metric, percent?, iso?, hours?, text}`: `availability`, `response` or `resolution` |
//! | [`keys::DEADLINE`] | Record `{iso?, days?, date?, text}`: a delivery or completion deadline |
//! | [`keys::WARRANTY`] | Record `{iso, months?, days?, text}`: the warranty period |
//! | [`keys::GOVERNING_LAW`] | Record `{code, jurisdiction, text}`: `RS`, `Serbia` |
//! | [`keys::COURT`] | Record `{court, city?, text}`: the court for disputes |
//! | [`keys::ARBITRATION`] | Record `{institution?, rules?, seat?, text}` |
//! | [`keys::LANGUAGE`] | Record `{languages, prevails?, text}`: ISO 639-1 codes |
//! | [`keys::ANNEX`] | Record `{label, title?}`: an annex that is part of the document |
//! | [`keys::AMENDMENT`] | Record `{action, article?, text}`: what an amendment changes: `change`, `delete`, `add` |
//! | [`keys::HIGHLIGHT`] | Record `{key, page, block, start, end, quote}`: where each fact above sits (see `quail_core::highlight`) |
//!
//! Every fact has a `conf`, an [`Evidence`](quail_core::Evidence) (page and byte span) and a
//! highlight. A fact without a cue in the text is absent: nothing is guessed. `role` is the
//! side a party is on: `supplier` (dobavljač, izvođač, prodavac, zakupodavac, supplier,
//! contractor, seller, lessor, licensor) or `customer` (naručilac, kupac, korisnik, zakupac,
//! customer, client, buyer, lessee, licensee). Names of representatives and signatories are
//! emitted as the contract writes them: they are what the document is signed by.
//!
//! # How it works
//!
//! The text is read as lines of tokens with folded keys (`quail_payment::lex`: lowercase,
//! no diacritics, Cyrillic → Latin) and joined into sentences ([`text`]). Every rule looks
//! for a cue phrase and reads a bounded number of tokens around it: durations ([`duration`]),
//! percentages, amounts (`quail_payment::amount`), dates (the `signal.date` facts inside the
//! sentence), capitalised names. See the README for the cue lists.
//!
//! # Limits
//!
//! Reads at most `Limits::max_text_bytes` (and [`MAX_SCAN_BYTES`]) and [`MAX_SCAN_LINES`]
//! lines, skips pages that still need OCR, checks the deadline per page. At most
//! [`MAX_PER_KEY`] facts per key. The scan is linear in the text.

// Rules walk token indices (`i`/`j`/`k`, `a`/`b` ranges, `s` a sentence).
#![allow(clippy::many_single_char_names)]
// Each rule is an ordered list of small checks; splitting them would scatter the order.
#![allow(clippy::too_many_lines)]

pub mod duration;
pub mod text;

mod analyzer;
mod document;
mod law;
mod parties;
mod risk;
mod terms;

pub use analyzer::{ContractFacts, MAX_PER_KEY, MAX_SCAN_BYTES, MAX_SCAN_LINES};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// A contracting party and its role (Record: `role` = `supplier` or `customer`, `label`
    /// = the role word as written, `name`).
    pub const PARTY: FactKey = FactKey::new("contract.party");
    /// An identifier of a party (Record: `role`, `party`?, `kind` = `pib`, `mb`, `vat`,
    /// `reg`, `iban`, `account` or `bic`, `value` compact, `checked` = confirmed by the
    /// check-digit scan of `quail-signals`).
    pub const PARTY_ID: FactKey = FactKey::new("contract.party_id");
    /// The address of a party (Record: `role`, `party`?, `text`).
    pub const ADDRESS: FactKey = FactKey::new("contract.address");
    /// Who represents a party (Record: `role`, `party`?, `name`?, `title`?).
    pub const REPRESENTATIVE: FactKey = FactKey::new("contract.representative");
    /// A signature block (Record: `role`, `label`, `name`?, `title`?).
    pub const SIGNATORY: FactKey = FactKey::new("contract.signatory");
    /// The duration of the contract (Record: `iso`?, `months`?, `days`?, `indefinite`?,
    /// `text`).
    pub const TERM: FactKey = FactKey::new("contract.term");
    /// Whether the contract renews by itself (Record: `auto` Bool, `iso`?, `same_period`?,
    /// `text`).
    pub const RENEWAL: FactKey = FactKey::new("contract.renewal");
    /// The notice period (Record: `iso`, `days`?, `months`?, `text`).
    pub const NOTICE: FactKey = FactKey::new("contract.notice");
    /// A termination right (Record: `kind` = `breach`, `convenience`, `insolvency`,
    /// `mutual`, `force_majeure`, `notice` or `change_of_control`, `text`).
    pub const TERMINATION: FactKey = FactKey::new("contract.termination");
    /// Days to pay (Record: `days` Int, `business`?, `from`? = `invoice_receipt`,
    /// `invoice_date`, `delivery` or `signing`, `text`).
    pub const PAYMENT_DAYS: FactKey = FactKey::new("contract.payment_days");
    /// How payment is made (Text: `bank_transfer`, `cash`, `card`, `compensation`).
    pub const PAYMENT_METHOD: FactKey = FactKey::new("contract.payment_method");
    /// An advance payment (Record: `percent`? Float, `amount`? Money, `text`).
    pub const ADVANCE: FactKey = FactKey::new("contract.advance");
    /// How the price may change (Record: `basis` = `cpi`, `inflation`, `exchange_rate`,
    /// `fixed` or `other`, `text`).
    pub const INDEXATION: FactKey = FactKey::new("contract.indexation");
    /// Interest on late payment (Record: `statutory` Bool, `percent`? Float, `text`).
    pub const LATE_INTEREST: FactKey = FactKey::new("contract.late_interest");
    /// A contractual penalty or liquidated damages (Record: `percent`? Float, `per`? =
    /// `day` or `week`, `amount`? Money, `cap_percent`? Float, `text`).
    pub const PENALTY: FactKey = FactKey::new("contract.penalty");
    /// A guarantee or security (Record: `kind` = `bank_guarantee`, `bill_of_exchange`,
    /// `performance_bond`, `letter_of_credit`, `parent_guarantee` or `deposit`, `purpose`? =
    /// `performance`, `advance`, `warranty` or `bid`, `percent`?, `amount`?, `text`).
    pub const GUARANTEE: FactKey = FactKey::new("contract.guarantee");
    /// A limit of liability (Record: `amount`? Money, `percent`? Float, `basis`? =
    /// `contract_value` or `fees_paid`, `months`? Int, `text`).
    pub const LIABILITY_CAP: FactKey = FactKey::new("contract.liability_cap");
    /// A confidentiality obligation (Record: `iso`? = how long it lasts, `text`).
    pub const CONFIDENTIALITY: FactKey = FactKey::new("contract.confidentiality");
    /// A service level (Record: `metric` = `availability`, `response` or `resolution`,
    /// `percent`? Float, `iso`?, `hours`? Int, `text`).
    pub const SLA: FactKey = FactKey::new("contract.sla");
    /// A delivery or completion deadline (Record: `iso`?, `days`? Int, `date`? Date, `text`).
    pub const DEADLINE: FactKey = FactKey::new("contract.deadline");
    /// The warranty period (Record: `iso`, `months`?, `days`?, `text`).
    pub const WARRANTY: FactKey = FactKey::new("contract.warranty");
    /// The governing law (Record: `code` ISO 3166, `jurisdiction`, `text`).
    pub const GOVERNING_LAW: FactKey = FactKey::new("contract.governing_law");
    /// The court for disputes (Record: `court`, `city`?, `text`).
    pub const COURT: FactKey = FactKey::new("contract.court");
    /// Arbitration of disputes (Record: `institution`?, `rules`?, `seat`?, `text`).
    pub const ARBITRATION: FactKey = FactKey::new("contract.arbitration");
    /// The contract's languages (Record: `languages` List of ISO 639-1 codes, `prevails`?,
    /// `text`).
    pub const LANGUAGE: FactKey = FactKey::new("contract.language");
    /// An annex that is part of the document (Record: `label`, `title`?).
    pub const ANNEX: FactKey = FactKey::new("contract.annex");
    /// What an amendment changes (Record: `action` = `change`, `delete` or `add`,
    /// `article`?, `text`).
    pub const AMENDMENT: FactKey = FactKey::new("contract.amendment");
    /// Where a contract fact sits in the text, one per fact (Record: `key`, `page`, `block`,
    /// `start`, `end`, `quote`; see `quail_core::highlight`).
    pub const HIGHLIGHT: FactKey = FactKey::new("contract.highlight");
}

//! The key-facts card of a contract or invoice. Namespace `card.*`.
//!
//! One analyzer, [`KeyFactsCard`] (`card.facts@1`, `Fast`, requires `document`, a finalizer
//! registered after `contract.facts`). It reads no text of its own: it picks, per field, the
//! most confident fact of the other steps and writes it in one canonical, normalised shape,
//! so a document can be searched and filtered without reading it. Dates are `Date` values
//! (ISO `YYYY-MM-DD` in the text form), amounts `Money` (exact decimal and ISO 4217 code),
//! durations ISO 8601 with whole `months` or `days`, codes lowercase. Every card fact keeps
//! the evidence of the fact it came from and gets a [`keys::HIGHLIGHT`]. A field with no
//! source fact is absent.
//!
//! | key | value | from |
//! |---|---|---|
//! | [`keys::KIND`] | Text | `ref.instrument_kind` |
//! | [`keys::NUMBER`] | Text | `ref.doc_number` (`raw`) |
//! | [`keys::SUPPLIER`], [`keys::CUSTOMER`] | Record `{name, pib?, mb?, vat?, reg?, account?, address?, representative?}` | `contract.party`, `contract.party_id`, `contract.address`, `contract.representative`; else `party.company` with that role |
//! | [`keys::SIGNED`], [`keys::START`], [`keys::END`], [`keys::ISSUED`], [`keys::DUE`] | Date | `signal.date_signed`, `_effective`, `_expiry`, `_invoice`, `_due` |
//! | [`keys::TERM`] | Record `{iso?, months?, days?, indefinite?}` | `contract.term` |
//! | [`keys::AUTO_RENEWAL`] | Bool | `contract.renewal` |
//! | [`keys::NOTICE`] | Record `{iso, months?, days?}` | `contract.notice` |
//! | [`keys::PAYMENT_DAYS`] | Int | `contract.payment_days` |
//! | [`keys::PAYMENT_METHOD`] | Text | `contract.payment_method` |
//! | [`keys::AMOUNT`], [`keys::AMOUNT_BASIS`], [`keys::CURRENCY`], [`keys::PERIOD`] | Money, Text | `payment.*` |
//! | [`keys::SUBJECT`], [`keys::CATEGORY`] | Text | `payment.subject` (`text`), `payment.category` (`code`) |
//! | [`keys::ACCOUNTS`] | List of Text | `signal.iban`, `signal.rs_account` |
//! | [`keys::ADVANCE`] | Record `{percent?, amount?}` | `contract.advance` |
//! | [`keys::INDEXATION`] | Text | `contract.indexation` (`basis`) |
//! | [`keys::PENALTY`] | Record `{percent?, per?, amount?, cap_percent?}` | `contract.penalty` |
//! | [`keys::GUARANTEES`] | List of Text | `contract.guarantee` (`kind`) |
//! | [`keys::LIABILITY_CAP`] | Record `{amount?, percent?, basis?, months?}` | `contract.liability_cap` |
//! | [`keys::CONFIDENTIALITY`] | Bool | `contract.confidentiality`, or an NDA |
//! | [`keys::SLA`] | Record `{availability?, response_hours?, resolution_hours?}` | `contract.sla` |
//! | [`keys::WARRANTY`] | Record `{iso, months?, days?}` | `contract.warranty` |
//! | [`keys::GOVERNING_LAW`] | Text | `contract.governing_law` (`code`) |
//! | [`keys::COURT`] | Text | `contract.court` (`court`) |
//! | [`keys::ARBITRATION`] | Text | `contract.arbitration`: institution, else rules, else `arbitration` |
//! | [`keys::LANGUAGES`] | List of Text | `contract.language` |
//! | [`keys::ANNEXES`] | List of Text | `contract.annex` (`label`) |
//! | [`keys::AMENDS`] | Text | `ref.cites` with relation `amends` (`raw`) |
//! | [`keys::TEXT`] | Text | the card as `field: value` lines, for a prompt or a search index |
//! | [`keys::HIGHLIGHT`] | Record `{key, page, block, start, end, quote}` | one per source fact |

mod card;

pub use card::KeyFactsCard;

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// The instrument kind (Text, from `ref.instrument_kind`: `contract`, `invoice`, ...).
    pub const KIND: FactKey = FactKey::new("card.kind");
    /// The document's own number as written (Text, from `ref.doc_number`).
    pub const NUMBER: FactKey = FactKey::new("card.number");
    /// The supplier (Record: `name`, `pib`?, `mb`?, `vat`?, `reg`?, `account`?, `address`?,
    /// `representative`?).
    pub const SUPPLIER: FactKey = FactKey::new("card.supplier");
    /// The customer (Record: same fields as `card.supplier`).
    pub const CUSTOMER: FactKey = FactKey::new("card.customer");
    /// Signing date (Date).
    pub const SIGNED: FactKey = FactKey::new("card.signed");
    /// Start or effective date (Date).
    pub const START: FactKey = FactKey::new("card.start");
    /// End or expiry date (Date).
    pub const END: FactKey = FactKey::new("card.end");
    /// Invoice issue date (Date).
    pub const ISSUED: FactKey = FactKey::new("card.issued");
    /// Payment due date (Date).
    pub const DUE: FactKey = FactKey::new("card.due");
    /// Duration of the contract (Record: `iso`?, `months`?, `days`?, `indefinite`?).
    pub const TERM: FactKey = FactKey::new("card.term");
    /// Whether the contract renews by itself (Bool).
    pub const AUTO_RENEWAL: FactKey = FactKey::new("card.auto_renewal");
    /// Notice period (Record: `iso`, `months`?, `days`?).
    pub const NOTICE: FactKey = FactKey::new("card.notice");
    /// Days to pay (Int).
    pub const PAYMENT_DAYS: FactKey = FactKey::new("card.payment_days");
    /// Payment method (Text: `bank_transfer`, `cash`, `card`, `compensation`).
    pub const PAYMENT_METHOD: FactKey = FactKey::new("card.payment_method");
    /// The price (Money, from `payment.amount`).
    pub const AMOUNT: FactKey = FactKey::new("card.amount");
    /// What the price is: `gross`, `net` or `unstated` (Text).
    pub const AMOUNT_BASIS: FactKey = FactKey::new("card.amount_basis");
    /// ISO 4217 code of the price (Text).
    pub const CURRENCY: FactKey = FactKey::new("card.currency");
    /// How often the price is paid (Text: `one_off`, `monthly`, `quarterly`, `yearly`).
    pub const PERIOD: FactKey = FactKey::new("card.period");
    /// What is paid for (Text).
    pub const SUBJECT: FactKey = FactKey::new("card.subject");
    /// Category code of what is paid for (Text, see `payment.category`).
    pub const CATEGORY: FactKey = FactKey::new("card.category");
    /// Bank accounts and IBANs the document names (List of Text).
    pub const ACCOUNTS: FactKey = FactKey::new("card.accounts");
    /// Advance payment (Record: `percent`?, `amount`?).
    pub const ADVANCE: FactKey = FactKey::new("card.advance");
    /// How the price may change (Text: `cpi`, `inflation`, `exchange_rate`, `fixed`,
    /// `other`).
    pub const INDEXATION: FactKey = FactKey::new("card.indexation");
    /// Contractual penalty (Record: `percent`?, `per`?, `amount`?, `cap_percent`?).
    pub const PENALTY: FactKey = FactKey::new("card.penalty");
    /// Guarantee kinds (List of Text, see `contract.guarantee`).
    pub const GUARANTEES: FactKey = FactKey::new("card.guarantees");
    /// Limit of liability (Record: `amount`?, `percent`?, `basis`?, `months`?).
    pub const LIABILITY_CAP: FactKey = FactKey::new("card.liability_cap");
    /// Whether the document binds to confidentiality (Bool).
    pub const CONFIDENTIALITY: FactKey = FactKey::new("card.confidentiality");
    /// Service levels (Record: `availability`? percent, `response_hours`?,
    /// `resolution_hours`?).
    pub const SLA: FactKey = FactKey::new("card.sla");
    /// Warranty period (Record: `iso`, `months`?, `days`?).
    pub const WARRANTY: FactKey = FactKey::new("card.warranty");
    /// Governing law, ISO 3166 code (Text).
    pub const GOVERNING_LAW: FactKey = FactKey::new("card.governing_law");
    /// The court for disputes (Text).
    pub const COURT: FactKey = FactKey::new("card.court");
    /// Arbitration: the institution, else the rules, else `arbitration` (Text).
    pub const ARBITRATION: FactKey = FactKey::new("card.arbitration");
    /// The document's languages, ISO 639-1 (List of Text).
    pub const LANGUAGES: FactKey = FactKey::new("card.languages");
    /// Annex labels (List of Text).
    pub const ANNEXES: FactKey = FactKey::new("card.annexes");
    /// The number of the document an amendment amends (Text, from `ref.cites`).
    pub const AMENDS: FactKey = FactKey::new("card.amends");
    /// The card as `field: value` lines (Text), for a prompt or a search index.
    pub const TEXT: FactKey = FactKey::new("card.text");
    /// Where a card field's source fact sits in the text, one per source fact (Record: `key`,
    /// `page`, `block`, `start`, `end`, `quote`; see `quail_core::highlight`).
    pub const HIGHLIGHT: FactKey = FactKey::new("card.highlight");
}

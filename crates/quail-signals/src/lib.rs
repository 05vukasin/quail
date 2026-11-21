//! Rule-based candidates for formatted business data. Namespace `signal.*`.
//!
//! One analyzer, [`SignalScan`] (`signal.scan@4`, `Fast`, requires `document`), reads every
//! page and reports *candidates*, not verdicts: each fact has a calibrated `conf` (see
//! [`conf`]) and [`Evidence`](quail_core::Evidence) with the page, the zone and box of the
//! block it sits in, and its byte span in the original page text.
//!
//! | family | keys |
//! |---|---|
//! | identifiers | [`keys::IBAN`], [`keys::BIC`], [`keys::VAT`], [`keys::RS_PIB`], [`keys::RS_MB`], [`keys::RS_ACCOUNT`], [`keys::HR_OIB`], [`keys::LEI`], [`keys::DUNS`], [`keys::US_EIN`], [`keys::JMBG_COUNT`] |
//! | dates | [`keys::DATE`] and the role keys [`keys::DATE_EFFECTIVE`], [`keys::DATE_EXPIRY`], [`keys::DATE_SIGNED`], [`keys::DATE_INVOICE`], [`keys::DATE_DUE`] |
//! | money | [`keys::AMOUNT`], [`keys::AMOUNT_TOTAL`], [`keys::AMOUNT_NET`], [`keys::AMOUNT_GROSS`] |
//! | contacts | [`keys::EMAIL_DOMAIN`], [`keys::WEB_DOMAIN`], [`keys::PHONE`] |
//!
//! # How it works
//!
//! Each page is normalized with `quail_text::normalize::normalize_mapped` (NFKC, invisible
//! characters out, dashes folded; the offset map takes spans back to the page text) and
//! tokenized with `quail_text::token::tokens_with_symbols`; every token gets its folded key
//! (`quail_text::fold`, Cyrillic → Latin, no diacritics), so one keyword list covers
//! `PIB`, `ПИБ`, `Matični broj`, `Матични број`, `septembra`, `септембра`. Every family
//! then walks the tokens once, looking a bounded number of tokens around each candidate:
//! the scan is linear in the text, with no regular expressions.
//!
//! # Rules worth knowing
//!
//! - **Check digits first.** IBAN, LEI, VAT numbers (where an algorithm is public), PIB, MB,
//!   OIB, Serbian accounts and JMBG are checked ([`checksum`]). A well-formed value with
//!   wrong check digits is kept only right after its label, at
//!   [`conf::INVALID_LABELED`]; without a label it is dropped.
//! - **Labels.** Bare digit runs are ambiguous (a PIB, a phone number, an invoice number all
//!   have nine digits). MB, OIB, DUNS, EIN and unprefixed VAT numbers need their label; a
//!   bare valid PIB is kept only on pages that read as Serbian business text, at
//!   [`conf::PIB_UNLABELED`].
//! - **JMBG is personal data.** Its value is never emitted, only [`keys::JMBG_COUNT`], the
//!   number of valid JMBGs in the file, so files with personal IDs can be flagged.
//! - **One fact per distinct value** and key, with the evidence of its most confident
//!   occurrence; at most [`MAX_PER_KEY`] values per key (then a problem is recorded).
//! - **Dates** with a role keyword before them (`Effective Date`, `Datum dospeća`,
//!   `Valuta`, `expires on`) are reported twice: as [`keys::DATE`] and under the role key,
//!   both `Value::Date`, with the role key's `conf` lowered by the role's precision.
//!   Ambiguous slash dates are resolved per file (see `date`).

// Scanners walk token indices (`p` page, `i`/`j`/`k` tokens, `t` text, `d` digits): short
// names read better there than long ones.
#![allow(clippy::many_single_char_names)]
// Counting a byte in a short slice; not worth a dependency.
#![allow(clippy::naive_bytecount)]

mod contact;
mod date;
mod ident;
mod money;
mod page;
mod phrase;
mod scan;

pub mod checksum;
pub mod conf;

pub use contact::GENERIC_EMAIL_DOMAINS;
pub use scan::{MAX_PER_KEY, SignalScan};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// IBAN, compact and uppercase (Text).
    pub const IBAN: FactKey = FactKey::new("signal.iban");
    /// BIC / SWIFT code (Text).
    pub const BIC: FactKey = FactKey::new("signal.bic");
    /// EU / UK VAT number with its country prefix, compact (`DE136695976`, `ATU04571784`)
    /// (Text).
    pub const VAT: FactKey = FactKey::new("signal.vat");
    /// Serbian tax number PIB, 9 digits (Text).
    pub const RS_PIB: FactKey = FactKey::new("signal.rs_pib");
    /// Serbian registration number (matični broj) of a company, 8 digits (Text).
    pub const RS_MB: FactKey = FactKey::new("signal.rs_mb");
    /// Serbian bank account `BBB-NNNNNNNNNNNNN-KK`, middle part zero-padded (Text).
    pub const RS_ACCOUNT: FactKey = FactKey::new("signal.rs_account");
    /// Croatian OIB, 11 digits (Text).
    pub const HR_OIB: FactKey = FactKey::new("signal.hr_oib");
    /// LEI, 20 characters (Text).
    pub const LEI: FactKey = FactKey::new("signal.lei");
    /// DUNS number, 9 digits (Text).
    pub const DUNS: FactKey = FactKey::new("signal.duns");
    /// US EIN `12-3456789` (Text).
    pub const US_EIN: FactKey = FactKey::new("signal.us_ein");
    /// Number of valid JMBGs (personal IDs) in the file; the values are never emitted (Int).
    pub const JMBG_COUNT: FactKey = FactKey::new("signal.jmbg_count");

    /// A calendar date (Date).
    pub const DATE: FactKey = FactKey::new("signal.date");
    /// A date a keyword marks as the start / effective date (Date).
    pub const DATE_EFFECTIVE: FactKey = FactKey::new("signal.date_effective");
    /// A date a keyword marks as the end / expiry date (Date).
    pub const DATE_EXPIRY: FactKey = FactKey::new("signal.date_expiry");
    /// A date a keyword marks as the signing date (Date).
    pub const DATE_SIGNED: FactKey = FactKey::new("signal.date_signed");
    /// A date a keyword marks as the invoice / issue date (Date).
    pub const DATE_INVOICE: FactKey = FactKey::new("signal.date_invoice");
    /// A date a keyword marks as the payment due date (Date).
    pub const DATE_DUE: FactKey = FactKey::new("signal.date_due");

    /// An amount with its currency, exact decimal (Money).
    pub const AMOUNT: FactKey = FactKey::new("signal.amount");
    /// An amount after a total keyword (`Ukupno`, `Total`, `Za uplatu`) (Money).
    pub const AMOUNT_TOTAL: FactKey = FactKey::new("signal.amount_total");
    /// An amount marked as without VAT (`bez PDV-a`, `excl. VAT`) (Money).
    pub const AMOUNT_NET: FactKey = FactKey::new("signal.amount_net");
    /// An amount marked as with VAT (`sa PDV-om`, `incl. VAT`) (Money).
    pub const AMOUNT_GROSS: FactKey = FactKey::new("signal.amount_gross");

    /// Domain of an e-mail address, free-mail hosts excluded (Text).
    pub const EMAIL_DOMAIN: FactKey = FactKey::new("signal.email_domain");
    /// Domain of a URL or web address (Text).
    pub const WEB_DOMAIN: FactKey = FactKey::new("signal.web_domain");
    /// Phone number in E.164 (`+381111234567`) (Text).
    pub const PHONE: FactKey = FactKey::new("signal.phone");
}

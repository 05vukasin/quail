//! What is paid for and how much. Namespace `payment.*`.
//!
//! One analyzer, [`PaymentExtract`] (`payment.extract@1`, `Fast`, requires `document`, runs
//! after `revised_document` and as a finalizer, so it reads OCR text and the facts of the
//! other steps). It labels every contract, offer and invoice with its subject, a category,
//! the price (net, VAT and gross apart), the currency, the period and the line items, in
//! Serbian (Latin and Cyrillic) and English. Each fact carries a `conf`, an
//! [`Evidence`](quail_core::Evidence) (page and byte span) and a [`keys::HIGHLIGHT`] fact
//! (block, character range and quote, see [`quail_core::highlight`]).
//!
//! | key | value |
//! |---|---|
//! | [`keys::SUBJECT`] | Record `{text, source}`: what is paid for, as written; `source` is `clause`, `after_heading`, `line_items` or `title` |
//! | [`keys::CATEGORY`] | Record `{code, label, keyword}`: one of [`category::CATEGORIES`] and the keyword that decided it (absent for `other`) |
//! | [`keys::AMOUNT`] | Money: the price the document commits to |
//! | [`keys::AMOUNT_BASIS`] | Text: `gross`, `net` or `unstated`, what [`keys::AMOUNT`] is |
//! | [`keys::CURRENCY`] | Text: ISO 4217 code of the price |
//! | [`keys::NET`], [`keys::VAT`], [`keys::GROSS`] | Money: without VAT, the VAT, with VAT, each where stated |
//! | [`keys::PERIOD`] | Text: `one_off`, `monthly`, `quarterly` or `yearly` |
//! | [`keys::LINES`] | List of Record `{description, quantity, unit, unit_price, total, currency, vat_rate}`: the `item.line` facts, fields where known |
//! | [`keys::HIGHLIGHT`] | Record `{key, page, block, start, end, quote}`: where each fact above sits, one per fact (one per line for `payment.lines`) |
//!
//! When nothing is found a fact is absent: nothing is guessed.
//!
//! # How it works
//!
//! The text is read as lines of tokens with folded keys ([`lex`]), so one cue list covers
//! `Ukupna vrednost`, `UKUPNA VREDNOST` and `Укупна вредност`.
//!
//! - **Price** ([`amount`]): an amount after a price cue on its line ([`cues::PRICE`]:
//!   `Ukupna vrednost ugovora`, `Vrednost ugovora`, `Cena`, `Naknada`, `Ukupno za uplatu`,
//!   `Iznos`, `Contract Value`, `Total`, `Fee`, `Amount Due`), or alone on the next line.
//!   Numbers and currencies are read by `quail_text::money` (`1.234.567,89`, `1,234,567.89`,
//!   `1 234 567,89`, `RSD`, `din.`, `дин.`, `€`, `EUR`, `$`, `USD`). A VAT marker next to
//!   the amount (`bez PDV-a`, `sa PDV-om`, `excl. VAT`, `incl. VAT`, [`cues::VAT_MARK`]) or a
//!   line that opens with `PDV` / `VAT` makes it net, gross or VAT. Penalties, guarantees,
//!   deposits, interest and advances ([`cues::NOT_PRICE`]) and rates (`po satu`, `per hour`,
//!   `unit price`) are not the price. The strongest cue wins, then the larger amount.
//!   `payment.amount` is the gross amount when stated, else the plain total (gross when it
//!   is net + VAT), else the net amount. Net + VAT = gross raises the confidence of all three;
//!   a mismatch lowers it. Table totals of `item.table_total` fill a role the text left empty.
//! - **Period**: a period word on the price's line (`mesečna naknada`, `mesečno`,
//!   `per month`, `godišnje`, `annually`, `jednokratno`), else on a line about paying, else
//!   `one_off` for an invoice. A monthly fee is reported as it is, never multiplied.
//! - **Subject** ([`subject`]): the text after a label ([`cues::SUBJECT`]: `Predmet ugovora`,
//!   `Predmet ovog ugovora je`, `Opis usluge`, `Subject of the Agreement`, `Scope of
//!   Services`, `Services:`, `Description:`) to the end of its sentence, or the paragraph
//!   under such a label when it is a heading; else the first line descriptions; else a
//!   specific title (`UGOVOR O ODRŽAVANJU SOFTVERA`).
//! - **Category** ([`category`]): keyword weights over the subject's words.
//! - **Lines**: the `item.line` facts of `quail-items` (tables, PDF layout, aligned OCR rows).
//!
//! # Limits
//!
//! Reads at most `Limits::max_text_bytes` (and [`MAX_SCAN_BYTES`]) and [`MAX_SCAN_LINES`]
//! lines of text, [`lex::MAX_LINE_TOKENS`] tokens per line, skips pages that still need OCR
//! and checks the deadline per page. Every rule looks a bounded number of tokens around a
//! cue, so the scan is linear. At most [`MAX_LINES`] line items.

// Rules walk token indices (`i`/`j`/`k` tokens, `a` an amount, `c` a candidate).
#![allow(clippy::many_single_char_names)]
// The scan and the fact assembly are ordered sequences of small rules; splitting them would
// scatter the order that matters.
#![allow(clippy::too_many_lines)]

pub mod amount;
pub mod category;
pub mod cues;
pub mod lex;
pub mod subject;

mod analyzer;

pub use analyzer::{MAX_LINES, MAX_SCAN_BYTES, MAX_SCAN_LINES, PaymentExtract};

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// What is paid for, as the document states it (Record: `text`, `source`).
    pub const SUBJECT: FactKey = FactKey::new("payment.subject");
    /// Category of what is paid for (Record: `code`, `label`, `keyword`).
    pub const CATEGORY: FactKey = FactKey::new("payment.category");
    /// The price the document commits to (Money).
    pub const AMOUNT: FactKey = FactKey::new("payment.amount");
    /// What `payment.amount` is: `gross`, `net` or `unstated` (Text).
    pub const AMOUNT_BASIS: FactKey = FactKey::new("payment.amount_basis");
    /// ISO 4217 code of the price (Text).
    pub const CURRENCY: FactKey = FactKey::new("payment.currency");
    /// The price without VAT, where stated (Money).
    pub const NET: FactKey = FactKey::new("payment.net");
    /// The VAT amount, where stated (Money).
    pub const VAT: FactKey = FactKey::new("payment.vat");
    /// The price with VAT, where stated (Money).
    pub const GROSS: FactKey = FactKey::new("payment.gross");
    /// How often the price is paid: `one_off`, `monthly`, `quarterly`, `yearly` (Text).
    pub const PERIOD: FactKey = FactKey::new("payment.period");
    /// Invoice and offer line items (List of Record).
    pub const LINES: FactKey = FactKey::new("payment.lines");
    /// Where a payment fact sits in the text (Record: `key`, `page`, `block`, `start`, `end`,
    /// `quote`; see `quail_core::highlight`).
    pub const HIGHLIGHT: FactKey = FactKey::new("payment.highlight");
}

//! The facts this crate reads, by key, and small helpers to pick them out of `ctx.facts`.
//!
//! Leaf crates never depend on each other, so the keys of `quail-refs`, `quail-parties`,
//! `quail-signals`, `quail-items`, `quail-sketch`, `quail-text`, `quail-pdf`, `quail-docx`,
//! `quail-ocr` and `quail-engine` are repeated here as plain strings. `tests/keys.rs` checks
//! that each one is declared (`FactKey::new("...")`) in its owner crate's sources, so a renamed
//! key fails the build of this crate's tests instead of silently emptying a card field.

use quail_core::{Fact, Value};

/// `engine.format`.
pub const ENGINE_FORMAT: &str = "engine.format";

/// `text.lang`.
pub const TEXT_LANG: &str = "text.lang";
/// `text.script`.
pub const TEXT_SCRIPT: &str = "text.script";
/// `text.garbage_ratio`.
pub const TEXT_GARBAGE: &str = "text.garbage_ratio";
/// `text.words`.
pub const TEXT_WORDS: &str = "text.words";

/// `ref.doc_number`.
pub const REF_DOC_NUMBER: &str = "ref.doc_number";
/// `ref.cites`.
pub const REF_CITES: &str = "ref.cites";
/// `ref.title`.
pub const REF_TITLE: &str = "ref.title";
/// `ref.instrument_kind`.
pub const REF_KIND: &str = "ref.instrument_kind";
/// `ref.ordinal`.
pub const REF_ORDINAL: &str = "ref.ordinal";

/// `party.candidates`.
pub const PARTY_CANDIDATES: &str = "party.candidates";
/// `party.company`.
pub const PARTY_COMPANY: &str = "party.company";

/// Dated roles of `quail-signals`, in card order, with the card's role name.
pub const SIGNAL_DATES: &[(&str, &str)] = &[
    ("signal.date_signed", "signed"),
    ("signal.date_effective", "effective"),
    ("signal.date_expiry", "expires"),
    ("signal.date_invoice", "issued"),
    ("signal.date_due", "due"),
];
/// `signal.date`.
pub const SIGNAL_DATE: &str = "signal.date";
/// `signal.amount`.
pub const SIGNAL_AMOUNT: &str = "signal.amount";
/// Amount roles of `quail-signals`, in card order, with the card's role name.
pub const SIGNAL_AMOUNTS: &[(&str, &str)] = &[
    ("signal.amount_total", "total"),
    ("signal.amount_net", "net"),
    ("signal.amount_gross", "gross"),
];
/// Identifiers of `quail-signals`, in card order, with the card's short name.
pub const SIGNAL_IDS: &[(&str, &str)] = &[
    ("signal.rs_pib", "pib"),
    ("signal.rs_mb", "mb"),
    ("signal.vat", "vat"),
    ("signal.iban", "iban"),
    ("signal.hr_oib", "oib"),
    ("signal.lei", "lei"),
    ("signal.us_ein", "ein"),
    ("signal.duns", "duns"),
    ("signal.rs_account", "account"),
    ("signal.bic", "bic"),
];
/// `signal.jmbg_count`.
pub const SIGNAL_JMBG_COUNT: &str = "signal.jmbg_count";

/// `item.line`.
pub const ITEM_LINE: &str = "item.line";
/// `item.count`.
pub const ITEM_COUNT: &str = "item.count";

/// `sketch.simhash_text`.
pub const SKETCH_SIMHASH_TEXT: &str = "sketch.simhash_text";
/// `sketch.simhash_template`.
pub const SKETCH_SIMHASH_TEMPLATE: &str = "sketch.simhash_template";
/// `sketch.text_key`.
pub const SKETCH_TEXT_KEY: &str = "sketch.text_key";

/// `ocr.pages`.
pub const OCR_PAGES: &str = "ocr.pages";

/// `pdf.page_count`.
pub const PDF_PAGE_COUNT: &str = "pdf.page_count";
/// `pdf.encrypted`.
pub const PDF_ENCRYPTED: &str = "pdf.encrypted";
/// `pdf.producer`.
pub const PDF_PRODUCER: &str = "pdf.producer";
/// `pdf.creator`.
pub const PDF_CREATOR: &str = "pdf.creator";
/// `pdf.author`.
pub const PDF_AUTHOR: &str = "pdf.author";
/// `pdf.title`.
pub const PDF_TITLE: &str = "pdf.title";
/// `pdf.signed`.
pub const PDF_SIGNED: &str = "pdf.signed";
/// `pdf.signature_fields`.
pub const PDF_SIGNATURE_FIELDS: &str = "pdf.signature_fields";
/// `pdf.has_javascript`.
pub const PDF_JAVASCRIPT: &str = "pdf.has_javascript";
/// `pdf.embedded_files`.
pub const PDF_EMBEDDED: &str = "pdf.embedded_files";
/// `pdf.image_only_pages`.
pub const PDF_IMAGE_ONLY_PAGES: &str = "pdf.image_only_pages";
/// `pdf.pages_needing_ocr`.
pub const PDF_NEEDS_OCR: &str = "pdf.pages_needing_ocr";
/// `pdf.incremental_updates`.
pub const PDF_UPDATES: &str = "pdf.incremental_updates";

/// `docx.title`.
pub const DOCX_TITLE: &str = "docx.title";
/// `docx.creator`.
pub const DOCX_CREATOR: &str = "docx.creator";
/// `docx.application`.
pub const DOCX_APPLICATION: &str = "docx.application";
/// `docx.company`.
pub const DOCX_COMPANY: &str = "docx.company";
/// `docx.pages`.
pub const DOCX_PAGES: &str = "docx.pages";
/// `docx.has_macros`.
pub const DOCX_MACROS: &str = "docx.has_macros";
/// `docx.hidden_text_chars`.
pub const DOCX_HIDDEN: &str = "docx.hidden_text_chars";
/// `docx.tracked_insertions`.
pub const DOCX_INS: &str = "docx.tracked_insertions";
/// `docx.tracked_deletions`.
pub const DOCX_DEL: &str = "docx.tracked_deletions";
/// `docx.tracked_format_changes`.
pub const DOCX_FMT: &str = "docx.tracked_format_changes";
/// `docx.comments`.
pub const DOCX_COMMENTS: &str = "docx.comments";
/// `docx.embedded_objects`.
pub const DOCX_EMBEDDED: &str = "docx.embedded_objects";
/// `docx.protection`.
pub const DOCX_PROTECTION: &str = "docx.protection";
/// `docx.clm_first_party`.
pub const DOCX_CLM_FIRST: &str = "docx.clm_first_party";
/// `docx.clm_second_party`.
pub const DOCX_CLM_SECOND: &str = "docx.clm_second_party";

/// Read-only view over the facts produced so far.
#[derive(Clone, Copy)]
pub struct Facts<'a>(pub &'a [Fact]);

impl<'a> Facts<'a> {
    /// Every fact with key `key`, in production order.
    pub fn all(self, key: &'a str) -> impl Iterator<Item = &'a Fact> + 'a {
        self.0.iter().filter(move |f| f.key.as_str() == key)
    }

    /// The most confident fact with key `key` (the first one on ties).
    pub fn best(self, key: &'a str) -> Option<&'a Fact> {
        let mut best: Option<&Fact> = None;
        for f in self.all(key) {
            if best.is_none_or(|b| f.conf > b.conf) {
                best = Some(f);
            }
        }
        best
    }

    /// Text of the most confident `key` fact, trimmed, when not empty.
    pub fn text(self, key: &'a str) -> Option<&'a str> {
        self.best(key)
            .and_then(|f| f.value.as_text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// Integer of the first `key` fact.
    pub fn int(self, key: &'a str) -> Option<i64> {
        self.all(key).find_map(|f| match f.value {
            Value::Int(i) => Some(i),
            _ => None,
        })
    }

    /// Boolean of the first `key` fact.
    pub fn bool(self, key: &'a str) -> Option<bool> {
        self.all(key).find_map(|f| match f.value {
            Value::Bool(b) => Some(b),
            _ => None,
        })
    }

    /// Float of the first `key` fact.
    pub fn float(self, key: &'a str) -> Option<f64> {
        self.all(key).find_map(|f| match f.value {
            Value::Float(x) => Some(x),
            _ => None,
        })
    }

    /// `U64` of the first `key` fact.
    pub fn u64(self, key: &'a str) -> Option<u64> {
        self.all(key).find_map(|f| match f.value {
            Value::U64(x) => Some(x),
            _ => None,
        })
    }
}

/// Text of the field `name` of a record, trimmed, when not empty.
pub fn field_text<'v>(v: &'v Value, name: &str) -> Option<&'v str> {
    v.field(name)
        .and_then(Value::as_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

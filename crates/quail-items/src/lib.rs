//! Price items: one record per offered or billed service or product line. Namespace `item.*`.
//!
//! The goal is supplier comparison: a later service answers "who is cheapest for X" from
//! these records. This crate does not decide that two lines from two suppliers are the same
//! service; it reports each line as written, normalized (quantity, unit, unit price, total,
//! currency, VAT), with top-3 category candidates from the EU CPV 2008 vocabulary and the
//! evidence to check it.
//!
//! One analyzer, [`ItemLines`] (`item.lines@6`): requires `document`, runs after
//! `revised_document`, `Profile::Fast` for the lines, categories from `Profile::Standard`.
//!
//! | key | value |
//! |---|---|
//! | [`keys::LINE`] | record, see below |
//! | [`keys::TABLE_TOTAL`] | record `label kind amount currency table_index row_index`; `kind` is `subtotal net vat gross total discount` |
//! | [`keys::COUNT`] | Int, number of `item.line` facts |
//!
//! `item.line` fields, in this order, optional ones only when known:
//! `description` (as written), `desc_key` (folded, see [`desc_key`]), `code`, `section`
//! (the table's last section row above the line), `quantity` (Float), `unit` (normalized:
//! `pcs h day week month year min m m2 m3 km kg g t l kwh lump_sum set pack pair person
//! night trip page spot item service`), `unit_raw`, `unit_price`, `unit_price_gross`,
//! `total`, `total_gross` (Money), `currency` (ISO 4217), `vat_rate` (Float, percent),
//! `vat_included` (Bool; absent when unknown), `valid_from`, `valid_to` (Date, when on the
//! line), `category_candidates` (List of record `code label label_sr score`, best first,
//! Standard profile and up), `source` (`table` DOCX table, `layout` table rebuilt from PDF
//! geometry, `text` one line of text), `table_index`, `row_index`. Evidence: page, zone
//! (`table` or `body`), span of the row in the page text, box when the format has one.
//!
//! # Sources
//!
//! 1. **Tables with structure** (DOCX `TableCell` blocks): header detection in English and
//!    Serbian Latin/Cyrillic (`Naziv/Opis/Usluga`, `Jedinica mere/JM`, `Količina/Kol.`,
//!    `Cena/Jedinična cena`, `Iznos/Ukupno`, `PDV`, `Description/Item`, `Qty`, `Unit`,
//!    `Unit price/Rate`, `Amount/Total`, `VAT`), two-row headers (`Cena` over `bez PDV` /
//!    `sa PDV`), the column-numbering row of tender forms, continuation tables that reuse the
//!    previous header, and roles guessed from content when there is no header (text column,
//!    unit column, numeric columns checked by `quantity × price = amount` and by net/gross
//!    VAT ratios). Subtotal, VAT and total rows become `item.table_total`.
//! 2. **PDF** (paragraph blocks with boxes): tables rebuilt from geometry, see `geom`.
//!    Descriptions read by token shape (`desc`): dates, air times, weekdays and spot codes
//!    glued to a layout description are split off, codes kept in `code`; the most text-like
//!    column is the description column.
//! 3. **Text lines**: `description …… 2 x 3.500,00 = 7.000,00`, merged table rows, rate
//!    cards (`Senior consultant – 85 EUR/h`).
//! 4. **Categories**: [`CpvIndex::categorize`]: a curated alias layer for common English and
//!    Serbian service phrases (`consulting`, `project management`, `medical writing`,
//!    `Phase III`, `SaaS subscription`, role titles) first, then TF-IDF over folded word
//!    stems and character trigrams of the CPV labels (English and Serbian), service lines
//!    preferring divisions 70–85.
//!
//! Summary rows (`Original SOW`, `Operations (E+F+G)`), credits and discounts (negative
//! amounts, `credit`/`discount` lines) become `item.table_total`, not `item.line`.
//!
//! # Limits
//!
//! At most [`MAX_ITEMS`] items per file (then a problem), tables up to 5 000 rows × 64
//! columns, 24 rebuilt columns per PDF table, 20 000 lines per page; the deadline is checked
//! per page and per table.

// Table and line parsers walk rows (`r`), columns (`c`) and tokens (`t`).
#![allow(clippy::many_single_char_names)]
#![allow(clippy::similar_names)]
// Row and line parsers are long sequences of small, ordered rules; splitting them would scatter
// the order that matters.
#![allow(clippy::too_many_lines)]
// `DocHints` is passed by reference everywhere so it can grow without touching every call.
#![allow(clippy::trivially_copy_pass_by_ref)]
// Counting a byte in a short slice; not worth a dependency.
#![allow(clippy::naive_bytecount)]

mod alias;
mod analyzer;
mod desc;
mod geom;
mod header;
mod item;
mod line;
mod num;
mod table;
mod unit;

pub mod cpv;

pub use analyzer::{ItemLines, MAX_ITEMS, TOP_K, desc_key};
pub use cpv::CpvIndex;

/// Fact keys emitted by this crate.
pub mod keys {
    use quail_core::FactKey;

    /// One price item (Record, see the crate documentation).
    pub const LINE: FactKey = FactKey::new("item.line");
    /// A subtotal / VAT / total row of a price table (Record).
    pub const TABLE_TOTAL: FactKey = FactKey::new("item.table_total");
    /// Number of `item.line` facts (Int).
    pub const COUNT: FactKey = FactKey::new("item.count");
}

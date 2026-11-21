//! What the extractors produce before it becomes facts.

use quail_core::{BBox, Date, Span};

use crate::num::{DateOrder, Dec, Num};

/// How a line was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// A table with structure (DOCX cells).
    Table,
    /// A table rebuilt from geometry (PDF lines with boxes).
    Layout,
    /// One line of text (`Senior consultant – 85 EUR/h`).
    Text,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Layout => "layout",
            Self::Text => "text",
        }
    }
}

/// One offered or billed line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub page: u32,
    pub description: String,
    pub code: Option<String>,
    pub section: Option<String>,
    pub quantity: Option<Num>,
    pub unit_raw: Option<String>,
    pub unit: Option<&'static str>,
    pub unit_price: Option<Num>,
    pub unit_price_gross: Option<Num>,
    pub total: Option<Num>,
    pub total_gross: Option<Num>,
    pub currency: Option<[u8; 3]>,
    pub vat_rate: Option<Num>,
    pub vat_included: Option<bool>,
    pub valid_from: Option<Date>,
    pub valid_to: Option<Date>,
    pub table_index: Option<u32>,
    pub row_index: Option<u32>,
    pub source: Source,
    pub conf: f32,
    pub span: Span,
    pub bbox: Option<BBox>,
}

impl Item {
    pub fn new(page: u32, source: Source, description: String) -> Self {
        Self {
            page,
            description,
            code: None,
            section: None,
            quantity: None,
            unit_raw: None,
            unit: None,
            unit_price: None,
            unit_price_gross: None,
            total: None,
            total_gross: None,
            currency: None,
            vat_rate: None,
            vat_included: None,
            valid_from: None,
            valid_to: None,
            table_index: None,
            row_index: None,
            source,
            conf: 0.5,
            span: Span { start: 0, end: 0 },
            bbox: None,
        }
    }
}

/// A subtotal, VAT or total row of a table.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TableTotal {
    pub page: u32,
    pub label: String,
    pub kind: &'static str,
    pub amount: Num,
    pub currency: Option<[u8; 3]>,
    pub table_index: u32,
    pub row_index: u32,
    pub conf: f32,
    pub span: Span,
    pub bbox: Option<BBox>,
}

/// What the whole document says that every line needs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DocHints {
    pub dec: Dec,
    pub order: DateOrder,
    /// The currency written most often, when one is written at all.
    pub currency: Option<[u8; 3]>,
    /// The document states prices with (true) or without (false) VAT, and never the other.
    pub vat: Option<bool>,
    /// Price list, offer, invoice or rate-card vocabulary is present.
    pub commercial: bool,
}

/// Union of two boxes.
pub(crate) fn union(a: Option<BBox>, b: Option<BBox>) -> Option<BBox> {
    match (a, b) {
        (Some(a), Some(b)) => Some(BBox {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        }),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Union of two spans (empty spans are ignored).
pub(crate) fn span_union(a: Span, b: Span) -> Span {
    if a.start == a.end {
        return b;
    }
    if b.start == b.end {
        return a;
    }
    Span {
        start: a.start.min(b.start),
        end: a.end.max(b.end),
    }
}

//! The `item.lines@6` analyzer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use quail_core::{
    Analyzer, AnalyzerId, BlockKind, Ctx, Document, Emitter, Evidence, Page, PageKind, Profile,
    Result, Value, Zone,
};
use quail_text::fold::{FoldOptions, fold};

use crate::cpv::CpvIndex;
use crate::desc;
use crate::geom;
use crate::item::{DocHints, Item, Source, TableTotal, span_union, union};
use crate::keys;
use crate::line;
use crate::num::{self, DateOrder, Dec, Num};
use crate::table::{self, GCell, Grid, Roles};

/// Most items reported per file; the rest is cut and a problem recorded.
pub const MAX_ITEMS: usize = 5_000;
/// Category candidates per item.
pub const TOP_K: usize = 3;
/// Text scanned for document-level hints (decimal separator, currency, VAT wording).
const HINT_BYTES: usize = 256 * 1024;
/// Largest table handled (rows × columns), against hostile DOCX tables.
const MAX_ROWS: usize = 5_000;
const MAX_COLS: usize = 64;
/// Tables read per page at most (a DOCX "page" is a whole section).
const MAX_TABLES_PER_PAGE: usize = 2_000;
/// Cell slots (used rows × columns) of all the tables of one page at most: a full
/// 5,000 × 64 table fits, hostile pages of thousands of wide tables do not.
const MAX_PAGE_SLOTS: usize = 1 << 19;

/// Price items per file: one `item.line` per offered or billed line, `item.table_total` per
/// subtotal / VAT / total row, `item.count`.
///
/// Requires `document`, runs after `revised_document` (so it sees OCR text when OCR ran),
/// reads [`Ctx::document`]. Line extraction runs in [`Profile::Fast`]; category candidates
/// (CPV top-3) are added from [`Profile::Standard`] on.
#[derive(Clone)]
pub struct ItemLines {
    /// Built on first use (only Standard and up categorize), shared by every clone.
    cpv: Arc<OnceLock<Arc<CpvIndex>>>,
}

impl Default for ItemLines {
    fn default() -> Self {
        Self::new()
    }
}

impl ItemLines {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("item.lines", 6);

    /// With the embedded CPV 2008 index. The index is built lazily on the first file that
    /// needs categories (about 0.2 s, about 15 MB), once for this value and all its clones; a
    /// `Profile::Fast` run never builds it.
    pub fn new() -> Self {
        Self {
            cpv: Arc::new(OnceLock::new()),
        }
    }

    /// With a given category index (shared).
    pub fn with_index(cpv: Arc<CpvIndex>) -> Self {
        Self {
            cpv: Arc::new(OnceLock::from(cpv)),
        }
    }

    /// The category index (built now if it was not yet).
    pub fn index(&self) -> &CpvIndex {
        self.cpv.get_or_init(|| Arc::new(CpvIndex::embedded()))
    }

    /// Whether the category index has been built.
    pub fn index_built(&self) -> bool {
        self.cpv.get().is_some()
    }
}
impl Analyzer for ItemLines {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let found = extract(doc, out);
        let categorize = ctx.profile >= Profile::Standard;
        emit(&found, categorize.then(|| self.index()), out);
        Ok(())
    }
}

/// Everything found in one document.
#[derive(Default)]
pub(crate) struct Found {
    pub items: Vec<Item>,
    pub totals: Vec<TableTotal>,
}

pub(crate) fn hints(doc: &Document) -> DocHints {
    let mut text = String::new();
    for p in &doc.pages {
        if text.len() >= HINT_BYTES {
            break;
        }
        let room = HINT_BYTES - text.len();
        let mut end = p.text.len().min(room);
        while !p.text.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&p.text[..end]);
        text.push('\n');
    }
    let (mut comma, mut dot) = (0u32, 0u32);
    let mut cur: HashMap<[u8; 3], u32> = HashMap::new();
    let (mut dmy, mut mdy) = (0u32, 0u32);
    for t in num::scan(&text, Dec::Unknown, DateOrder::Dmy) {
        let raw = &text[t.start..t.end];
        let (d, c) = (raw.rfind('.'), raw.rfind(','));
        let dec = match (d, c) {
            (Some(d), Some(c)) => Some(if d > c { '.' } else { ',' }),
            (Some(x), None) | (None, Some(x))
                if raw.len() - x == 3 && raw.matches(['.', ',']).count() == 1 =>
            {
                raw.as_bytes().get(x).map(|b| char::from(*b))
            }
            _ => None,
        };
        match dec {
            Some(',') => comma += 1,
            Some('.') => dot += 1,
            _ => {}
        }
        if let Some(c) = t.currency {
            *cur.entry(c).or_default() += 1;
        }
        if raw.contains('/') {
            let parts: Vec<u32> = raw.split('/').map(|p| p.parse().unwrap_or(0)).collect();
            if parts.len() == 3 {
                dmy += u32::from(parts[0] > 12 && parts[1] <= 12);
                mdy += u32::from(parts[1] > 12 && parts[0] <= 12);
            }
        }
    }
    let dec = match comma.cmp(&dot) {
        std::cmp::Ordering::Greater => Dec::Comma,
        std::cmp::Ordering::Less => Dec::Dot,
        std::cmp::Ordering::Equal => Dec::Unknown,
    };
    let currency = cur
        .iter()
        .max_by_key(|(c, n)| (**n, std::cmp::Reverse(**c)))
        .map(|(c, _)| *c);
    let order = if mdy > dmy || (mdy == dmy && currency == Some(*b"USD")) {
        DateOrder::Mdy
    } else {
        DateOrder::Dmy
    };
    let folded = fold(&text, &FoldOptions::KEY);
    let has = |needles: &[&str]| needles.iter().any(|n| folded.contains(n));
    // VAT wording outside total rows (`Ukupno sa PDV-om` says nothing about line prices).
    let prose: String = folded
        .lines()
        .filter(|l| {
            table::total_kind(
                l.split(['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'])
                    .next()
                    .unwrap_or(""),
            )
            .is_none()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let in_prose = |needles: &[&str]| needles.iter().any(|n| prose.contains(n));
    let net = in_prose(&[
        "bez pdv",
        "+ pdv",
        "plus pdv",
        "excl. vat",
        "excl vat",
        "excluding vat",
        "exclusive of vat",
        "plus vat",
        "+ vat",
        "net of vat",
        "pdv nije uracunat",
        "vat not included",
    ]);
    let gross = in_prose(&[
        "sa pdv",
        "s pdv",
        "ukljucujuci pdv",
        "uracunat pdv",
        "uracunatim pdv",
        "incl. vat",
        "incl vat",
        "including vat",
        "inclusive of vat",
        "vat included",
        "pdv uracunat",
        "sa uracunatim",
    ]);
    // `Sve cene su u dinarima`, `prices in EUR`: a currency named without an amount.
    let currency = currency.or_else(|| {
        if has(&["u dinarima", "dinara", " rsd", "in rsd"]) {
            Some(*b"RSD")
        } else if has(&["u evrima", "u eurima", "in eur", "in euro", " evra"]) {
            Some(*b"EUR")
        } else if has(&["in usd", "us dollars", "u dolarima", "in dollars"]) {
            Some(*b"USD")
        } else if has(&["in gbp", "pounds sterling"]) {
            Some(*b"GBP")
        } else {
            None
        }
    });
    let vat = match (net, gross) {
        (true, false) => Some(false),
        (false, true) => Some(true),
        _ => None,
    };
    let commercial = has(&[
        "cenovnik",
        "cjenovnik",
        "price list",
        "pricelist",
        "tarif",
        "rate card",
        "fee schedule",
        "ponuda",
        "predracun",
        "profaktura",
        "faktura",
        "racun br",
        "invoice",
        "quotation",
        "quote",
        "specifikacija",
        "schedule of rates",
        "pricing",
        "price schedule",
        "cene usluga",
        "cena usluga",
    ]);
    DocHints {
        dec,
        order,
        currency,
        vat,
        commercial,
    }
}

/// A cell with its row and column.
type CellAt = (u16, u16, GCell);

/// Why [`cell_grids`] stopped before the page's last table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridStop {
    /// More than [`MAX_TABLES_PER_PAGE`] tables.
    Tables,
    /// The tables need more than [`MAX_PAGE_SLOTS`] cell slots.
    Cells,
    /// The file's deadline.
    Deadline,
}

/// DOCX-style tables of a page: `TableCell` blocks grouped by table index, in order of first
/// appearance. A table holds only the rows that have a cell (row numbers may run to 5,000
/// with nothing in between). Stops at [`MAX_TABLES_PER_PAGE`], [`MAX_PAGE_SLOTS`] or
/// `deadline`, keeping the tables built so far.
fn cell_grids(page: &Page, deadline: Option<Instant>) -> (Vec<Grid>, Option<GridStop>) {
    let mut index: HashMap<u16, usize> = HashMap::new();
    let mut tables: Vec<Vec<CellAt>> = Vec::new();
    let mut stop = None;
    for b in &page.blocks {
        let BlockKind::TableCell { table, row, col } = b.kind else {
            continue;
        };
        if usize::from(row) >= MAX_ROWS || usize::from(col) >= MAX_COLS {
            continue;
        }
        let t = if let Some(&t) = index.get(&table) {
            t
        } else {
            if tables.len() >= MAX_TABLES_PER_PAGE {
                stop = Some(GridStop::Tables);
                continue;
            }
            index.insert(table, tables.len());
            tables.push(Vec::new());
            tables.len() - 1
        };
        let cell = GCell {
            text: b.span.slice(&page.text).to_owned(),
            span: b.span,
            bbox: b.bbox,
        };
        if let Some(v) = tables.get_mut(t) {
            v.push((row, col, cell));
        }
    }
    let mut grids = Vec::with_capacity(tables.len());
    let mut slots_left = MAX_PAGE_SLOTS;
    for mut cells in tables {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            stop = Some(GridStop::Deadline);
            break;
        }
        // Stable: the paragraphs of one cell keep their order.
        cells.sort_by_key(|c| c.0);
        let ncols = cells
            .iter()
            .map(|c| usize::from(c.1) + 1)
            .max()
            .unwrap_or(0);
        let used_rows = 1 + cells.windows(2).filter(|w| w[0].0 != w[1].0).count();
        let need = used_rows.saturating_mul(ncols);
        if need > slots_left {
            stop = Some(GridStop::Cells);
            break;
        }
        slots_left -= need;
        let mut rows: Vec<Vec<Option<GCell>>> = Vec::with_capacity(used_rows);
        let mut last = None;
        for (r, c, cell) in cells {
            if last != Some(r) {
                rows.push(vec![None; ncols]);
                last = Some(r);
            }
            let Some(slot) = rows.last_mut().and_then(|row| row.get_mut(usize::from(c))) else {
                continue;
            };
            match slot {
                Some(prev) => {
                    prev.text.push('\n');
                    prev.text.push_str(&cell.text);
                    prev.span = span_union(prev.span, cell.span);
                    prev.bbox = union(prev.bbox, cell.bbox);
                }
                None => *slot = Some(cell),
            }
        }
        // A cell's paragraphs are joined by newlines; table logic wants one line.
        for c in rows.iter_mut().flatten().flatten() {
            if c.text.contains('\n') {
                c.text = c
                    .text
                    .split('\n')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
        grids.push(Grid {
            page: page.index,
            ncols,
            rows,
            source: Source::Table,
        });
    }
    (grids, stop)
}

fn joined_row(row: &[Option<GCell>]) -> (String, quail_core::Span, Option<quail_core::BBox>) {
    let mut t = String::new();
    let mut span = quail_core::Span { start: 0, end: 0 };
    let mut bbox = None;
    for c in row.iter().flatten() {
        let s = c.text.trim();
        if s.is_empty() {
            continue;
        }
        if !t.is_empty() {
            t.push_str("  ");
        }
        t.push_str(s);
        span = span_union(span, c.span);
        bbox = union(bbox, c.bbox);
    }
    (t, span, bbox)
}

/// Finds items and totals in a document.
pub(crate) fn extract(doc: &Document, out: &mut Emitter) -> Found {
    let h = hints(doc);
    let mut found = Found::default();
    let mut prev: Option<Roles> = None;
    let mut table_index = 0u32;
    let mut full = false;
    for page in &doc.pages {
        if out.check_deadline().is_err() {
            out.problem(format!(
                "item.lines: deadline reached at page {}",
                page.index
            ));
            break;
        }
        if page.text.is_empty() {
            continue;
        }
        let (mut grids, stop) = cell_grids(page, out.deadline());
        match stop {
            Some(GridStop::Tables) => out.problem(format!(
                "item.lines: more than {MAX_TABLES_PER_PAGE} tables on page {}, rest skipped",
                page.index
            )),
            Some(GridStop::Cells) => out.problem(format!(
                "item.lines: tables on page {} need more than {MAX_PAGE_SLOTS} cells, rest \
                 skipped",
                page.index
            )),
            Some(GridStop::Deadline) => {
                out.problem(format!(
                    "item.lines: deadline reached in a table on page {}",
                    page.index
                ));
                break;
            }
            None => {}
        }
        // Lines of the page text are borrowed: a page of many megabytes is not copied.
        let mut text_lines: Vec<(Cow<'_, str>, quail_core::Span, Option<quail_core::BBox>)> =
            Vec::new();
        let geometric = page.kind == PageKind::Physical
            && page
                .blocks
                .iter()
                .any(|b| b.bbox.is_some() && !matches!(b.kind, BlockKind::TableCell { .. }));
        if geometric {
            let (g, rest) = geom::tables(page, &h);
            grids.extend(g);
            text_lines.extend(rest.iter().map(|r| {
                let (t, span, bbox) = r.joined();
                (Cow::Owned(t), span, bbox)
            }));
        } else {
            for b in &page.blocks {
                if matches!(b.kind, BlockKind::TableCell { .. })
                    || matches!(b.zone, Zone::Header | Zone::Footer)
                {
                    continue;
                }
                let text = b.span.slice(&page.text);
                let mut off = 0usize;
                for l in text.split('\n') {
                    let start = b.span.start as usize + off;
                    off += l.len() + 1;
                    let span = quail_core::Span {
                        start: u32::try_from(start).unwrap_or(u32::MAX),
                        end: u32::try_from(start + l.len()).unwrap_or(u32::MAX),
                    };
                    text_lines.push((Cow::Borrowed(l), span, b.bbox));
                }
            }
        }
        for g in &grids {
            if out.check_deadline().is_err() {
                out.problem(format!(
                    "item.lines: deadline reached in a table on page {}",
                    page.index
                ));
                break;
            }
            let parsed = table::parse(g, table_index, prev.as_ref(), &h);
            if parsed.roles.is_none() {
                for &ri in &parsed.unused_rows {
                    if let Some(row) = g.rows.get(ri) {
                        let (t, span, bbox) = joined_row(row);
                        text_lines.push((Cow::Owned(t), span, bbox));
                    }
                }
            }
            if parsed.roles.as_ref().is_some_and(|r| r.from_header) {
                prev.clone_from(&parsed.roles);
            }
            if !parsed.items.is_empty() || !parsed.totals.is_empty() {
                table_index += 1;
            }
            found.items.extend(parsed.items);
            found.totals.extend(parsed.totals);
        }
        for (t, span, bbox) in &text_lines {
            if let Some(it) = line::parse(t, page.index, *span, *bbox, &h) {
                found.items.push(it);
            }
        }
        if found.items.len() > MAX_ITEMS {
            found.items.truncate(MAX_ITEMS);
            full = true;
            break;
        }
    }
    for it in &mut found.items {
        if it.source != Source::Table
            && let Some((text, code)) = desc::clean(&it.description)
        {
            it.description = text;
            if it.code.is_none() {
                it.code = code;
            }
        }
    }
    credits_to_totals(&mut found);
    if full {
        out.problem(format!(
            "item.lines: more than {MAX_ITEMS} items, rest skipped"
        ));
    }
    found
}

/// Words that make a line a credit or a discount on other lines.
const CREDIT_WORDS: &[&str] = &[
    "credit",
    "credits",
    "discount",
    "rebate",
    "refund",
    "popust",
    "rabat",
    "odobrenje",
];

/// Credits and discounts are not offered lines: a line with a negative price or total
/// (`(1,200.00)`, `-15,00`), or a description with a credit word and no quantity
/// (`Licenses 3% partnership discount`), becomes an `item.table_total` of kind `discount`.
fn credits_to_totals(found: &mut Found) {
    let items = std::mem::take(&mut found.items);
    for it in items {
        let amount = it.total.or(it.unit_price);
        let negative = amount.is_some_and(Num::is_negative);
        let credit_word = it.quantity.is_none()
            && desc_key(&it.description)
                .split(' ')
                .any(|w| CREDIT_WORDS.contains(&w));
        match amount {
            Some(amount) if negative || credit_word => found.totals.push(TableTotal {
                page: it.page,
                label: it.description,
                kind: "discount",
                amount,
                currency: it.currency,
                table_index: it.table_index.unwrap_or(0),
                row_index: it.row_index.unwrap_or(0),
                conf: it.conf,
                span: it.span,
                bbox: it.bbox,
            }),
            _ => found.items.push(it),
        }
    }
}

/// The folded matching key of a description: lowercase, no diacritics, Latin script,
/// words separated by one space.
pub fn desc_key(description: &str) -> String {
    let f = fold(description, &FoldOptions::KEY);
    let mut out = String::with_capacity(f.len());
    for w in f
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

fn money(n: Num, c: Option<[u8; 3]>) -> Value {
    Value::Money(n.money(c))
}

fn cur_text(c: [u8; 3]) -> Value {
    Value::text(String::from_utf8_lossy(&c).into_owned())
}

fn int(v: u32) -> Value {
    Value::Int(i64::from(v))
}

pub(crate) fn emit(found: &Found, cpv: Option<&CpvIndex>, out: &mut Emitter) {
    let mut cache: HashMap<String, Value> = HashMap::new();
    for it in &found.items {
        let key = desc_key(&it.description);
        let mut f: Vec<(&str, Value)> = vec![
            ("description", Value::text(it.description.as_str())),
            ("desc_key", Value::text(key.as_str())),
        ];
        if let Some(c) = &it.code {
            f.push(("code", Value::text(c.as_str())));
        }
        if let Some(s) = &it.section {
            f.push(("section", Value::text(s.as_str())));
        }
        if let Some(q) = it.quantity {
            f.push(("quantity", Value::Float(q.to_f64())));
        }
        if let Some(u) = it.unit {
            f.push(("unit", Value::text(u)));
        }
        if let Some(u) = &it.unit_raw {
            f.push(("unit_raw", Value::text(u.as_str())));
        }
        if let Some(p) = it.unit_price {
            f.push(("unit_price", money(p, it.currency)));
        }
        if let Some(p) = it.unit_price_gross {
            f.push(("unit_price_gross", money(p, it.currency)));
        }
        if let Some(t) = it.total {
            f.push(("total", money(t, it.currency)));
        }
        if let Some(t) = it.total_gross {
            f.push(("total_gross", money(t, it.currency)));
        }
        if let Some(c) = it.currency {
            f.push(("currency", cur_text(c)));
        }
        if let Some(v) = it.vat_rate {
            f.push(("vat_rate", Value::Float(v.to_f64())));
        }
        if let Some(v) = it.vat_included {
            f.push(("vat_included", Value::Bool(v)));
        }
        if let Some(d) = it.valid_from {
            f.push(("valid_from", Value::Date(d)));
        }
        if let Some(d) = it.valid_to {
            f.push(("valid_to", Value::Date(d)));
        }
        if let Some(ix) = cpv {
            let cands = cache
                .entry(key.clone())
                .or_insert_with(|| {
                    Value::List(
                        ix.categorize(&it.description, it.section.as_deref(), it.unit, TOP_K)
                            .into_iter()
                            .filter_map(|c| {
                                let e = ix.entries().get(c.entry)?;
                                let mut r = vec![
                                    ("code", Value::text(&*e.code)),
                                    ("label", Value::text(&*e.en)),
                                ];
                                if !e.sr.is_empty() {
                                    r.push(("label_sr", Value::text(&*e.sr)));
                                }
                                r.push((
                                    "score",
                                    Value::Float(f64::from((c.score * 1000.0).round() / 1000.0)),
                                ));
                                Some(Value::record(r))
                            })
                            .collect(),
                    )
                })
                .clone();
            f.push(("category_candidates", cands));
        }
        f.push(("source", Value::text(it.source.as_str())));
        if let Some(t) = it.table_index {
            f.push(("table_index", int(t)));
        }
        if let Some(r) = it.row_index {
            f.push(("row_index", int(r)));
        }
        let zone = if it.source == Source::Text {
            Zone::Body
        } else {
            Zone::Table
        };
        out.push(
            keys::LINE,
            Value::record(f),
            it.conf,
            Some(Evidence {
                page: Some(it.page),
                zone,
                span: (it.span.end > it.span.start).then_some(it.span),
                bbox: it.bbox,
            }),
        );
    }
    for t in &found.totals {
        let mut f: Vec<(&str, Value)> = vec![
            ("label", Value::text(t.label.as_str())),
            ("kind", Value::text(t.kind)),
            ("amount", money(t.amount, t.currency)),
        ];
        if let Some(c) = t.currency {
            f.push(("currency", cur_text(c)));
        }
        f.push(("table_index", int(t.table_index)));
        f.push(("row_index", int(t.row_index)));
        out.push(
            keys::TABLE_TOTAL,
            Value::record(f),
            t.conf,
            Some(Evidence {
                page: Some(t.page),
                zone: Zone::Table,
                span: (t.span.end > t.span.start).then_some(t.span),
                bbox: t.bbox,
            }),
        );
    }
    out.fact(
        keys::COUNT,
        Value::Int(i64::try_from(found.items.len()).unwrap_or(i64::MAX)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Block, Span};

    /// A DOCX-like page: `tables` tables, each with one cell at (`row`, `col`).
    fn page(tables: u16, row: u16, col: u16) -> Page {
        let text = "Invented cell 12.50".to_owned();
        let span = Span {
            start: 0,
            end: u32::try_from(text.len()).unwrap_or(0),
        };
        Page {
            blocks: (0..tables)
                .map(|table| {
                    Block::new(
                        span,
                        BlockKind::TableCell { table, row, col },
                        Zone::Body,
                        None,
                    )
                })
                .collect(),
            text,
            ..Page::new(0, PageKind::Logical)
        }
    }

    #[test]
    fn a_table_holds_only_the_rows_it_uses() {
        // 4,999 empty rows before the only cell, which spans to the last column.
        let (grids, stop) = cell_grids(&page(10, 4_999, 63), None);
        assert!(stop.is_none());
        assert_eq!(grids.len(), 10);
        for g in &grids {
            assert_eq!((g.rows.len(), g.ncols), (1, 64));
            assert!(g.rows.capacity() <= 1, "{}", g.rows.capacity());
        }
    }

    #[test]
    fn cells_keep_their_order_and_merge() {
        let mut p = page(0, 0, 0);
        p.text = "a b c d".to_owned();
        let cell = |start: u32, table: u16, row: u16, col: u16| {
            Block::new(
                Span {
                    start,
                    end: start + 1,
                },
                BlockKind::TableCell { table, row, col },
                Zone::Body,
                None,
            )
        };
        // Table 1 seen first; two paragraphs in one cell; an empty row 1 in table 1.
        p.blocks = vec![
            cell(0, 1, 2, 0),
            cell(2, 0, 0, 1),
            cell(4, 1, 0, 0),
            cell(6, 1, 2, 0),
        ];
        let (grids, _) = cell_grids(&p, None);
        let texts: Vec<Vec<Vec<Option<String>>>> = grids
            .iter()
            .map(|g| {
                g.rows
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|c| c.as_ref().map(|c| c.text.clone()))
                            .collect()
                    })
                    .collect()
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                vec![vec![Some("c".to_owned())], vec![Some("a d".to_owned())]],
                vec![vec![None, Some("b".to_owned())]],
            ]
        );
    }

    #[test]
    fn too_many_tables_on_a_page_is_a_problem() {
        let tables = u16::try_from(MAX_TABLES_PER_PAGE + 5).unwrap_or(u16::MAX);
        let (grids, stop) = cell_grids(&page(tables, 0, 0), None);
        assert_eq!(grids.len(), MAX_TABLES_PER_PAGE);
        assert!(matches!(stop, Some(GridStop::Tables)));
        let doc = Document {
            pages: vec![page(tables, 0, 0)],
        };
        let mut out = Emitter::new(ItemLines::ID);
        let _ = extract(&doc, &mut out);
        let problems = out.take_problems();
        assert!(
            problems.iter().any(|p| p.contains("tables")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_cells_of_a_page_are_capped() {
        // Two tables of 5,000 rows, each row one cell in the last column: 320,000 slots each,
        // more than the page budget together.
        let mut p = page(0, 0, 0);
        let span = p
            .blocks
            .first()
            .map_or(Span { start: 0, end: 1 }, |b| b.span);
        p.blocks = (0..2u16)
            .flat_map(|table| {
                (0..u16::try_from(MAX_ROWS).unwrap_or(u16::MAX)).map(move |row| {
                    Block::new(
                        span,
                        BlockKind::TableCell {
                            table,
                            row,
                            col: 63,
                        },
                        Zone::Body,
                        None,
                    )
                })
            })
            .collect();
        let (grids, stop) = cell_grids(&p, None);
        assert!(matches!(stop, Some(GridStop::Cells)));
        assert_eq!(grids.len(), 1);
        let used: usize = grids.iter().map(|g| g.rows.len() * g.ncols).sum();
        assert!(used <= MAX_PAGE_SLOTS, "{used}");
    }

    #[test]
    fn a_passed_deadline_stops_building_tables() {
        let (grids, stop) = cell_grids(&page(3, 0, 0), Some(std::time::Instant::now()));
        assert!(grids.is_empty());
        assert!(matches!(stop, Some(GridStop::Deadline)));
    }
}

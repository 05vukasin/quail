//! From a grid of cells (a DOCX table, or a table rebuilt from PDF geometry) to line items:
//! header detection, column roles (from the header, from the previous table when a table
//! continues on the next page, or from the content), row parsing, total rows.

use quail_core::{BBox, Span};

use crate::header::{self, HeaderCell, Role};
use crate::item::{DocHints, Item, Source, TableTotal, span_union, union};
use crate::num::{self, Num, NumKind, NumTok};
use crate::unit;

/// One cell.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GCell {
    pub text: String,
    pub span: Span,
    pub bbox: Option<BBox>,
}

/// A table: rows of `ncols` optional cells.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grid {
    pub page: u32,
    pub ncols: usize,
    pub rows: Vec<Vec<Option<GCell>>>,
    pub source: Source,
}

/// Roles of a table's columns, kept to read a continuation of the table.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Roles {
    pub cols: Vec<Option<HeaderCell>>,
    /// Came from a header row (not guessed from content).
    pub from_header: bool,
    /// Price columns of a rate matrix (`Role | Onshore | Offshore`): one item per row and
    /// column, the column label added to the description.
    pub matrix: Vec<usize>,
}

impl Roles {
    fn first(&self, role: Role) -> Option<usize> {
        self.cols
            .iter()
            .position(|c| c.is_some_and(|c| c.role == role))
    }

    fn all(&self, role: Role) -> impl Iterator<Item = usize> + '_ {
        self.cols
            .iter()
            .enumerate()
            .filter(move |(_, c)| c.is_some_and(|c| c.role == role))
            .map(|(i, _)| i)
    }

    fn get(&self, i: usize) -> Option<HeaderCell> {
        self.cols.get(i).copied().flatten()
    }

    fn priced(&self) -> bool {
        self.first(Role::UnitPrice).is_some() || self.first(Role::Total).is_some()
    }
}

/// What came out of one grid.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub items: Vec<Item>,
    pub totals: Vec<TableTotal>,
    pub roles: Option<Roles>,
    /// Rows that produced nothing (for the text fallback).
    pub unused_rows: Vec<usize>,
}

fn cell_text(row: &[Option<GCell>], i: usize) -> &str {
    row.get(i)
        .and_then(Option::as_ref)
        .map_or("", |c| c.text.trim())
}

fn letters(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphabetic()).count()
}

/// Keywords that open a subtotal / total row, folded, with the kind they report.
const TOTAL_KEYS: &[(&[&str], &str)] = &[
    (&["ukupno", "bez"], "net"),
    (&["ukupno", "sa"], "gross"),
    (&["ukupno", "pdv"], "vat"),
    (&["ukupan", "iznos", "pdv"], "vat"),
    (&["iznos", "pdv"], "vat"),
    (&["pdv"], "vat"),
    (&["porez"], "vat"),
    (&["vat"], "vat"),
    (&["tax"], "vat"),
    (&["sales", "tax"], "vat"),
    (&["osnovica"], "net"),
    (&["poreska", "osnovica"], "net"),
    (&["net", "total"], "net"),
    (&["net", "amount"], "net"),
    (&["total", "net"], "net"),
    (&["gross", "total"], "gross"),
    (&["total", "gross"], "gross"),
    (&["grand", "total"], "total"),
    (&["sub", "total"], "subtotal"),
    (&["subtotal"], "subtotal"),
    (&["sub-total"], "subtotal"),
    (&["medjuzbir"], "subtotal"),
    (&["zbir"], "subtotal"),
    (&["ukupno"], "total"),
    (&["sveukupno"], "total"),
    (&["svega"], "total"),
    (&["ukupna", "cena"], "total"),
    (&["ukupna", "vrednost"], "total"),
    (&["ukupan", "iznos"], "total"),
    (&["za", "uplatu"], "total"),
    (&["za", "placanje"], "total"),
    (&["total"], "total"),
    (&["totals"], "total"),
    (&["amount", "due"], "total"),
    (&["balance", "due"], "total"),
    (&["net", "amount", "due"], "total"),
    (&["rabat"], "discount"),
    (&["popust"], "discount"),
    (&["discount"], "discount"),
    (&["agency", "commission"], "discount"),
];

/// The total kind a row label opens with, if it is a total row label.
pub(crate) fn total_kind(label: &str) -> Option<&'static str> {
    let w = header::words(label);
    if w.is_empty() || w.len() > 12 {
        return None;
    }
    let mut best: Option<(usize, &str)> = None;
    for (keys, kind) in TOTAL_KEYS {
        if w.len() >= keys.len() && w.iter().zip(keys.iter()).all(|(a, b)| a == b) {
            if best.is_none_or(|(n, _)| keys.len() > n) {
                best = Some((keys.len(), kind));
            }
        }
    }
    let kind = match best {
        Some((_, k)) => k,
        // `AIRTIME TOTAL :`, `Invoice total`, `Rep. Firm Commission`: a short label with a
        // total word anywhere.
        None if w.len() <= 5 => {
            let has = |set: &[&str]| w.iter().any(|x| set.contains(&x.as_str()));
            if has(&[
                "total", "totals", "subtotal", "ukupno", "svega", "payable", "balance",
            ]) {
                "total"
            } else if has(&["taxable", "osnovica"]) {
                "net"
            } else if has(&["amount", "iznos"])
                && has(&["tax", "vat", "pdv", "due", "excluding", "excl", "net"])
            {
                if has(&["excluding", "excl", "net"]) {
                    "net"
                } else {
                    "total"
                }
            } else if has(&["tax", "vat", "pdv"]) && w.len() <= 3 {
                "vat"
            } else if w.iter().any(|x| x == "commission" || x == "provizija") {
                "discount"
            } else if matches!(w.first().map(String::as_str), Some("gross" | "bruto")) {
                "gross"
            } else if matches!(w.first().map(String::as_str), Some("net" | "neto")) {
                "net"
            } else {
                return None;
            }
        }
        // `4,420.43 USD at TAX (0.0%) Totals`: a long label ending in a total word.
        None if matches!(
            w.last().map(String::as_str),
            Some("total" | "totals" | "subtotal" | "ukupno")
        ) =>
        {
            "total"
        }
        None => return None,
    };
    // `ukupno sa PDV-om` is gross, `ukupno bez PDV-a` net, `Total incl. VAT` gross.
    if kind == "total" {
        if w.iter()
            .any(|x| x == "incl" || x == "including" || x == "inclusive")
        {
            return Some("gross");
        }
        if w.iter()
            .any(|x| x == "excl" || x == "excluding" || x == "before")
        {
            return Some("net");
        }
    }
    Some(kind)
}

/// The first amount in a cell: number, its currency, and a unit after it (`85 EUR/h`).
pub(crate) struct CellAmount {
    pub num: Num,
    /// Written as money (currency, two decimals or thousand groups).
    pub money: bool,
    pub currency: Option<[u8; 3]>,
    pub unit: Option<(&'static str, String)>,
}

pub(crate) fn amount_in(text: &str, hints: &DocHints) -> Option<CellAmount> {
    amount_of(text, hints, false)
}

/// The amount of a price or total cell: its last money-like number (`4 Charged 38.00` is
/// 38.00), else its first number.
pub(crate) fn money_in(text: &str, hints: &DocHints) -> Option<CellAmount> {
    amount_of(text, hints, true)
}

fn amount_of(text: &str, hints: &DocHints, money: bool) -> Option<CellAmount> {
    let toks = num::scan(text, hints.dec, hints.order);
    let pick = if money {
        toks.iter().rposition(NumTok::moneylike)
    } else {
        None
    };
    let tok = match pick {
        Some(i) => toks.into_iter().nth(i)?,
        None => toks
            .into_iter()
            .find(|t| t.number().is_some() && !t.glued)?,
    };
    let num = tok.number()?;
    let unit = unit_after(text, tok.end);
    Some(CellAmount {
        num,
        money: tok.moneylike(),
        currency: tok.currency,
        unit,
    })
}

/// A unit right after an amount: `kom`, `/h`, `EUR/h`, `po satu`, `per hour`, `mesečno`.
pub(crate) fn unit_after(text: &str, at: usize) -> Option<(&'static str, String)> {
    let rest = text.get(at..)?;
    let mut words = rest
        .split(|c: char| c.is_whitespace() || c == '/' || c == '(' || c == ')' || c == ',')
        .filter(|w| !w.is_empty());
    let mut w = words.next()?;
    let mut key = unit::key(w);
    // Skip a currency word (`85 EUR/h`).
    if num::currency_word(&key, false).is_some() || key.len() == 1 && "€$£".contains(key.as_str())
    {
        w = words.next()?;
        key = unit::key(w);
    }
    if matches!(key.as_str(), "po" | "per" | "a" | "za") {
        let next = words.next()?;
        let k2 = unit::key(next);
        return unit::normalize(&k2).map(|u| (u, format!("{w} {next}")));
    }
    unit::normalize(&key).map(|u| (u, w.to_owned()))
}

/// A unit written in a description: `(h)`, `/sat`, `po satu`, `per hour`, `mesečno`.
pub(crate) fn unit_in_desc(desc: &str) -> Option<(&'static str, String)> {
    let w = header::words(desc);
    for (i, x) in w.iter().enumerate() {
        if matches!(x.as_str(), "po" | "per" | "za") {
            if let Some(n) = w.get(i + 1) {
                if let Some(u) = unit::normalize(n) {
                    if !matches!(u, "pcs" | "m" | "g" | "t" | "l" | "item" | "service") {
                        return Some((u, format!("{x} {n}")));
                    }
                }
            }
        }
        if matches!(
            x.as_str(),
            "mesecno"
                | "godisnje"
                | "monthly"
                | "annually"
                | "yearly"
                | "hourly"
                | "daily"
                | "weekly"
                | "nedeljno"
                | "pausal"
                | "pausalno"
        ) {
            return unit::normalize(x).map(|u| (u, x.clone()));
        }
    }
    None
}

/// `1 | 2 | 3 | 4 | 5=3x4`: the column-numbering row of a tender form.
fn is_numbering_row(row: &[Option<GCell>]) -> bool {
    let cells: Vec<&str> = row
        .iter()
        .filter_map(|c| c.as_ref().map(|c| c.text.trim()))
        .filter(|t| !t.is_empty())
        .collect();
    if cells.len() < 2 {
        return false;
    }
    let mut last = 0u32;
    for c in &cells {
        if c.len() > 14
            || !c.chars().all(|ch| {
                ch.is_ascii_digit() || " ()=xX×*+./-:".contains(ch) || ch == 'х' || ch == 'Х'
            })
        {
            return false;
        }
        let lead: String = c
            .trim_start_matches(['(', ' '])
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let Ok(n) = lead.parse::<u32>() else {
            return false;
        };
        if n <= last || n > 30 {
            return false;
        }
        last = n;
    }
    true
}

const MONTHS: &[&str] = &[
    "jan",
    "january",
    "feb",
    "february",
    "mar",
    "march",
    "apr",
    "april",
    "may",
    "jun",
    "june",
    "jul",
    "july",
    "aug",
    "august",
    "sep",
    "sept",
    "september",
    "oct",
    "october",
    "nov",
    "november",
    "dec",
    "december",
    "januar",
    "februar",
    "mart",
    "maj",
    "juni",
    "jul",
    "avgust",
    "septembar",
    "oktobar",
    "novembar",
    "decembar",
    "jun",
    "januara",
    "februara",
    "marta",
    "aprila",
    "maja",
    "juna",
    "jula",
    "avgusta",
    "septembra",
    "oktobra",
    "novembra",
    "decembra",
];

/// Only a date in words (`March 01, 2024`, `15. maj 2024`).
pub(crate) fn is_date_text(s: &str) -> bool {
    let w = header::words(s);
    let letters: Vec<&String> = w
        .iter()
        .filter(|x| x.chars().any(char::is_alphabetic))
        .collect();
    !letters.is_empty()
        && letters.len() <= 2
        && letters.iter().all(|x| MONTHS.contains(&x.as_str()))
}

/// A first row of short labels over the columns, with no numbers.
fn is_label_row(row: &[Option<GCell>]) -> bool {
    let cells: Vec<&str> = row
        .iter()
        .flatten()
        .map(|c| c.text.trim())
        .filter(|t| !t.is_empty())
        .collect();
    cells.len() >= 2
        && cells.iter().all(|t| {
            t.chars().count() <= 40
                && t.chars().any(char::is_alphabetic)
                && !t.chars().any(|c| c.is_ascii_digit())
        })
}

fn header_roles(row: &[Option<GCell>], ncols: usize) -> Vec<Option<HeaderCell>> {
    (0..ncols)
        .map(|i| {
            let t = cell_text(row, i);
            if t.is_empty() { None } else { header::role(t) }
        })
        .collect()
}

fn is_header(roles: &[Option<HeaderCell>]) -> bool {
    let mut seen: Vec<Role> = roles.iter().flatten().map(|h| h.role).collect();
    seen.sort_by_key(|r| *r as u8);
    seen.dedup();
    let priced = seen
        .iter()
        .any(|r| matches!(r, Role::UnitPrice | Role::Total));
    let described = seen
        .iter()
        .any(|r| matches!(r, Role::Desc | Role::Qty | Role::Unit | Role::Code));
    seen.len() >= 2 && priced && described
}

/// A row that reads as a header continuation: short texts, no amounts.
fn is_subheader(row: &[Option<GCell>], hints: &DocHints) -> bool {
    let mut any = false;
    for c in row.iter().flatten() {
        let t = c.text.trim();
        if t.is_empty() {
            continue;
        }
        if t.len() > 40
            || num::scan(t, hints.dec, hints.order)
                .iter()
                .any(|n| n.number().is_some() && !n.glued)
        {
            return false;
        }
        let w = header::words(t);
        let qualifier = w.iter().any(|x| {
            matches!(
                x.as_str(),
                "bez"
                    | "sa"
                    | "pdv"
                    | "vat"
                    | "rsd"
                    | "eur"
                    | "usd"
                    | "din"
                    | "excl"
                    | "incl"
                    | "net"
                    | "gross"
                    | "neto"
                    | "bruto"
                    | "%"
            )
        });
        if !qualifier && header::role(t).is_none() {
            return false;
        }
        any = true;
    }
    any
}

/// Header roles of row `h`, merged with a sub-header row `h + 1`.
fn merge_subheader(grid: &Grid, h: usize, sub: usize) -> Vec<Option<HeaderCell>> {
    let (top, bottom) = (&grid.rows[h], &grid.rows[sub]);
    let mut out = Vec::with_capacity(grid.ncols);
    for i in 0..grid.ncols {
        let (a, b) = (cell_text(top, i), cell_text(bottom, i));
        let joined = match (a.is_empty(), b.is_empty()) {
            (false, false) => format!("{a} {b}"),
            (false, true) => a.to_owned(),
            (true, false) => b.to_owned(),
            (true, true) => String::new(),
        };
        let mut r = if joined.is_empty() {
            None
        } else {
            header::role(&joined)
        };
        if a.is_empty() && !b.is_empty() {
            // A spanning top cell (`Cena`) over `bez PDV` | `sa PDV`: inherit its role.
            let left_cell: Option<HeaderCell> =
                out.iter().rev().find_map(|c: &Option<HeaderCell>| *c);
            let inherited = left_cell.map(|left: HeaderCell| {
                let q = header::role(&format!("cena {b}"));
                HeaderCell {
                    role: left.role,
                    vat: q.and_then(|q| q.vat),
                    currency: q.and_then(|q| q.currency).or(left.currency),
                }
            });
            if r.is_none_or(|r| r.role == Role::VatAmount) {
                r = inherited;
            }
        }
        out.push(r);
    }
    out
}

/// Kind of a cell's content, for guessing roles without a header.
#[derive(Default, Clone, Copy)]
struct ColStats {
    nonempty: usize,
    numeric: usize,
    money: usize,
    integer: usize,
    text: usize,
    unit: usize,
    percent: usize,
}

fn cell_numeric(t: &str, hints: &DocHints) -> Option<NumTok> {
    let toks = num::scan(t, hints.dec, hints.order);
    let first = toks.first()?.clone();
    // Mostly a number: few letters left besides a currency or unit word.
    let rest_letters = letters(t) - letters(t.get(first.start..first.end).unwrap_or(""));
    if first.glued || rest_letters > 6 {
        return None;
    }
    Some(first)
}

fn infer_roles(grid: &Grid, start: usize, hints: &DocHints) -> Option<Roles> {
    let mut stats = vec![ColStats::default(); grid.ncols];
    let mut rows = 0usize;
    for row in grid.rows.iter().skip(start) {
        let filled = row
            .iter()
            .flatten()
            .filter(|c| !c.text.trim().is_empty())
            .count();
        if filled < 2 {
            continue;
        }
        rows += 1;
        for (i, s) in stats.iter_mut().enumerate() {
            let t = cell_text(row, i);
            if t.is_empty() {
                continue;
            }
            s.nonempty += 1;
            if unit::of_phrase(t).is_some() {
                s.unit += 1;
            } else if let Some(n) = cell_numeric(t, hints) {
                match n.kind {
                    NumKind::Number(v) => {
                        s.numeric += 1;
                        s.money += usize::from(n.moneylike());
                        s.integer += usize::from(!n.formatted && v.scale == 0);
                    }
                    NumKind::Percent(_) => s.percent += 1,
                    NumKind::Date(_) => {}
                }
            } else if letters(t) >= 3 {
                s.text += 1;
            }
        }
    }
    if rows == 0 {
        return None;
    }
    let half = rows.div_ceil(2);
    let desc = (0..grid.ncols)
        .filter(|&i| stats[i].text >= half.max(1))
        .max_by_key(|&i| (stats[i].text, std::cmp::Reverse(i)))?;
    let mut cols: Vec<Option<HeaderCell>> = vec![None; grid.ncols];
    let mk = |role| {
        Some(HeaderCell {
            role,
            vat: None,
            currency: None,
        })
    };
    cols[desc] = mk(Role::Desc);
    let mut matrix = Vec::new();
    let is_money = |i: usize| stats[i].money * 2 >= stats[i].numeric.max(1);
    for i in 0..grid.ncols {
        if i != desc && stats[i].unit >= half {
            cols[i] = mk(Role::Unit);
        } else if i != desc && stats[i].percent >= half {
            cols[i] = mk(Role::VatRate);
        }
    }
    let numeric: Vec<usize> = (desc + 1..grid.ncols)
        .filter(|&i| {
            cols[i].is_none()
                && stats[i].numeric >= half
                && stats[i].numeric * 2 >= stats[i].nonempty
        })
        .collect();
    match numeric.as_slice() {
        [] => return None,
        [p] => cols[*p] = mk(Role::UnitPrice),
        [a, b] => {
            if stats[*a].integer * 2 >= stats[*a].numeric && stats[*b].money > stats[*a].money {
                cols[*a] = mk(Role::Qty);
                cols[*b] = mk(Role::UnitPrice);
            } else {
                match gross_ratio(grid, start, *a, *b, hints) {
                    Some(_) => {
                        cols[*a] = Some(HeaderCell {
                            role: Role::UnitPrice,
                            vat: Some(false),
                            currency: None,
                        });
                        cols[*b] = Some(HeaderCell {
                            role: Role::UnitPrice,
                            vat: Some(true),
                            currency: None,
                        });
                    }
                    None if is_money(*a)
                        && is_money(*b)
                        && !ascending(grid, start, *a, *b, hints) =>
                    {
                        cols[*a] = mk(Role::UnitPrice);
                        cols[*b] = mk(Role::UnitPrice);
                        matrix = vec![*a, *b];
                    }
                    None => {
                        cols[*a] = mk(Role::UnitPrice);
                        cols[*b] = mk(Role::Total);
                    }
                }
            }
        }
        many => {
            if let Some((q, p, t)) = best_triple(grid, start, many, hints) {
                cols[q] = mk(Role::Qty);
                cols[p] = mk(Role::UnitPrice);
                cols[t] = mk(Role::Total);
            } else {
                // Without arithmetic: prices are the money-formatted columns; integer-only
                // columns (lengths, zones, counts) are not prices.
                let money: Vec<usize> = many
                    .iter()
                    .copied()
                    .filter(|&i| stats[i].money * 2 >= stats[i].numeric)
                    .collect();
                let pool = if money.is_empty() {
                    many.to_vec()
                } else {
                    money
                };
                let n = pool.len();
                if n >= 2
                    && pool.iter().all(|&i| is_money(i))
                    && !ascending(grid, start, pool[n - 2], pool[n - 1], hints)
                {
                    for &c in &pool {
                        cols[c] = mk(Role::UnitPrice);
                    }
                    matrix.clone_from(&pool);
                } else if n >= 2 {
                    cols[pool[n - 1]] = mk(Role::Total);
                    cols[pool[n - 2]] = mk(Role::UnitPrice);
                } else {
                    cols[pool[0]] = mk(Role::UnitPrice);
                }
                let first_price = pool.first().copied().unwrap_or(grid.ncols);
                if let Some(&q) = many.iter().rev().find(|&&i| {
                    i < first_price && cols[i].is_none() && stats[i].integer * 2 >= stats[i].numeric
                }) {
                    cols[q] = mk(Role::Qty);
                }
            }
        }
    }
    // An ordinal column left of the description.
    if desc > 0 && stats[0].integer >= half && stats[0].text == 0 && cols[0].is_none() {
        cols[0] = mk(Role::Ordinal);
    }
    Some(Roles {
        cols,
        from_header: false,
        matrix,
    })
}

/// Share of rows where column `b` is at least column `a` (an amount is at least its price).
fn ascending(grid: &Grid, start: usize, a: usize, b: usize, hints: &DocHints) -> bool {
    let (mut n, mut up) = (0, 0);
    for row in grid.rows.iter().skip(start) {
        if let (Some(x), Some(y)) = (num_at(row, a, hints), num_at(row, b, hints)) {
            n += 1;
            up += usize::from(y.to_f64() >= x.to_f64());
        }
    }
    n > 0 && up * 5 >= n * 4
}

fn num_at(row: &[Option<GCell>], i: usize, hints: &DocHints) -> Option<Num> {
    money_in(cell_text(row, i), hints).map(|a| a.num)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= (0.011f64).max(0.01 * b.abs())
}

fn gross_ratio(grid: &Grid, start: usize, a: usize, b: usize, hints: &DocHints) -> Option<f64> {
    let mut hits = 0usize;
    let mut tried = 0usize;
    let mut ratio = None;
    for row in grid.rows.iter().skip(start) {
        let (Some(x), Some(y)) = (num_at(row, a, hints), num_at(row, b, hints)) else {
            continue;
        };
        if x.units == 0 {
            continue;
        }
        tried += 1;
        for r in [1.2, 1.1, 1.18, 1.19, 1.25, 1.21, 1.22, 1.17, 1.08, 1.05] {
            if close(x.to_f64() * r, y.to_f64()) {
                hits += 1;
                ratio = Some(r);
                break;
            }
        }
    }
    (tried > 0 && hits * 3 >= tried * 2)
        .then_some(ratio)
        .flatten()
}

fn best_triple(
    grid: &Grid,
    start: usize,
    cols: &[usize],
    hints: &DocHints,
) -> Option<(usize, usize, usize)> {
    // Each cell is read once: the triple search is cubic in the columns.
    let vals: Vec<Vec<Option<f64>>> = grid
        .rows
        .iter()
        .skip(start)
        .map(|row| {
            cols.iter()
                .map(|&c| num_at(row, c, hints).map(Num::to_f64))
                .collect()
        })
        .collect();
    let mut best = None;
    let mut best_hits = 0usize;
    for ia in 0..cols.len() {
        for ib in ia + 1..cols.len() {
            for ic in ib + 1..cols.len() {
                let mut hits = 0;
                let mut tried = 0;
                for v in &vals {
                    let (Some(x), Some(y), Some(z)) = (v[ia], v[ib], v[ic]) else {
                        continue;
                    };
                    tried += 1;
                    if close(x * y, z) {
                        hits += 1;
                    }
                }
                if tried > 0 && hits * 2 >= tried && hits > best_hits {
                    best_hits = hits;
                    best = Some((cols[ia], cols[ib], cols[ic]));
                }
            }
        }
    }
    best
}

/// Parses a grid. `prev` are the roles of the document's previous table (a table that goes
/// on over a page break repeats no header).
#[allow(clippy::too_many_lines)]
pub(crate) fn parse(
    grid: &Grid,
    table_index: u32,
    prev: Option<&Roles>,
    hints: &DocHints,
) -> Parsed {
    let mut out = Parsed::default();
    if grid.rows.is_empty() || grid.ncols == 0 {
        return out;
    }
    // 1. Header.
    let mut roles = None;
    let mut start = 0;
    let mut header_at = 0;
    let mut label_row: Option<usize> = None;
    for h in 0..grid.rows.len().min(8) {
        let r = header_roles(&grid.rows[h], grid.ncols);
        if is_header(&r) {
            let sub = h + 1;
            let r = if sub < grid.rows.len() && is_subheader(&grid.rows[sub], hints) {
                start = sub + 1;
                merge_subheader(grid, h, sub)
            } else {
                start = sub;
                r
            };
            roles = Some(Roles {
                cols: r,
                from_header: true,
                matrix: Vec::new(),
            });
            header_at = h;
            break;
        }
    }
    let roles = match roles {
        Some(r) => r,
        None => match prev {
            Some(p) if p.cols.len() == grid.ncols && p.from_header => p.clone(),
            _ => match {
                if grid.rows.len() > 1 && is_label_row(&grid.rows[0]) {
                    label_row = Some(0);
                    start = 1;
                }
                infer_roles(grid, start, hints)
            } {
                Some(r) => r,
                None => {
                    out.unused_rows = (0..grid.rows.len()).collect();
                    return out;
                }
            },
        },
    };
    if !roles.priced() {
        out.unused_rows = (0..grid.rows.len()).collect();
        return out;
    }
    out.unused_rows.extend(0..header_at);

    // Columns of each role.
    let mut desc_cols: Vec<usize> = roles.all(Role::Desc).collect();
    let mut code_col = roles.first(Role::Code);
    if desc_cols.len() > 1 {
        // `Item | Description`: a description column holding codes is the code column.
        let codes = |c: usize| {
            let (mut n, mut coded) = (0, 0);
            for row in grid.rows.iter().skip(start) {
                let t = cell_text(row, c);
                if t.is_empty() {
                    continue;
                }
                n += 1;
                coded += usize::from(
                    t.len() <= 20 && !t.contains(' ') && t.chars().any(|ch| ch.is_ascii_digit()),
                );
            }
            n > 0 && coded * 2 >= n
        };
        if let Some(pos) = desc_cols.iter().position(|&c| codes(c)) {
            let c = desc_cols.remove(pos);
            code_col = code_col.or(Some(c));
        }
    }
    let unit_col = roles.first(Role::Unit);
    let qty_col = roles.first(Role::Qty);
    let vat_col = roles.first(Role::VatRate);
    let cur_col = roles.first(Role::Currency);
    let start_col = roles.first(Role::StartDate);
    let end_col = roles.first(Role::EndDate);
    let (price_col, price_gross_col) = pick_pair(&roles, Role::UnitPrice);
    let (total_col, total_gross_col) = pick_pair(&roles, Role::Total);
    let first_value_col = [price_col, total_col, qty_col, unit_col]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(grid.ncols);
    let table_currency = roles.cols.iter().flatten().find_map(|h| h.currency);
    let col_vat = |c: Option<usize>| c.and_then(|c| roles.get(c)).and_then(|h| h.vat);
    let col_cur = |c: Option<usize>| c.and_then(|c| roles.get(c)).and_then(|h| h.currency);

    // A price column written as money in most rows does not take a bare `3` in another.
    let money_cols = [price_col, total_col].into_iter().flatten().any(|c| {
        let (mut n, mut m) = (0, 0);
        for row in grid.rows.iter().skip(start) {
            if let Some(a) = money_in(cell_text(row, c), hints) {
                n += 1;
                m += usize::from(a.money);
            }
        }
        n >= 2 && m * 2 >= n
    });
    let base_conf = match (grid.source, roles.from_header) {
        (Source::Table, true) => 0.85,
        (Source::Layout, true) => 0.75,
        (Source::Table, false) => 0.6,
        _ => 0.5,
    };
    let mut section: Option<String> = None;
    let mut last_item_row: Option<usize> = None;

    for (ri, row) in grid.rows.iter().enumerate().skip(start) {
        if is_numbering_row(row) {
            continue;
        }
        let row_span = row
            .iter()
            .flatten()
            .fold(Span { start: 0, end: 0 }, |s, c| span_union(s, c.span));
        let row_box = row.iter().flatten().fold(None, |b, c| union(b, c.bbox));
        // Description.
        let mut desc = String::new();
        for &c in &desc_cols {
            let t = cell_text(row, c);
            if !t.is_empty() {
                if !desc.is_empty() {
                    desc.push(' ');
                }
                desc.push_str(t);
            }
        }
        if desc_cols.is_empty() {
            for c in 0..first_value_col {
                let t = cell_text(row, c);
                if letters(t) >= 3 && Some(c) != code_col {
                    desc = t.to_owned();
                    break;
                }
            }
        }
        // Label for a total row: the description, or the first text cell of the row.
        let label = if letters(&desc) > 0 {
            desc.clone()
        } else {
            (0..grid.ncols)
                .map(|c| cell_text(row, c))
                .find(|t| letters(t) >= 2)
                .unwrap_or("")
                .to_owned()
        };
        // Without a header only amounts written as money count (`26`, `2011`, `4` in a
        // key-value layout are not prices).
        let trusted = |a: &CellAmount| (roles.from_header && !money_cols) || a.money;
        let price = price_col
            .and_then(|c| money_in(cell_text(row, c), hints))
            .filter(trusted);
        let total = total_col
            .and_then(|c| money_in(cell_text(row, c), hints))
            .filter(trusted);
        let desc_span = desc_cols
            .iter()
            .find_map(|&c| row.get(c).and_then(Option::as_ref).map(|c| c.span))
            .unwrap_or(row_span);
        if let Some(kind) = total_kind(&label) {
            let amount = total
                .as_ref()
                .or(price.as_ref())
                .map(|a| (a.num, a.currency))
                .or_else(|| last_amount(row, hints));
            if let Some((amount, cur)) = amount {
                out.totals.push(TableTotal {
                    page: grid.page,
                    label: label.clone(),
                    kind,
                    amount,
                    currency: cur
                        .or(col_cur(total_col))
                        .or(table_currency)
                        .or(hints.currency),
                    table_index,
                    row_index: u32::try_from(ri).unwrap_or(u32::MAX),
                    conf: base_conf,
                    span: row_span,
                    bbox: row_box,
                });
                last_item_row = None;
                continue;
            }
        }
        if price.is_none() && total.is_none() {
            let only_desc = row.iter().enumerate().all(|(i, c)| {
                desc_cols.contains(&i) || c.as_ref().is_none_or(|c| c.text.trim().is_empty())
            });
            if letters(&desc) >= 2 && only_desc {
                if grid.source == Source::Layout
                    && ri.checked_sub(1).is_some_and(|p| last_item_row == Some(p))
                    && let Some(it) = out.items.last_mut()
                {
                    // A wrapped description line.
                    it.description.push(' ');
                    it.description.push_str(desc.trim());
                    it.span = span_union(it.span, row_span);
                    it.bbox = union(it.bbox, row_box);
                    last_item_row = Some(ri);
                } else if desc.len() <= 120 {
                    section = Some(desc.trim().to_owned());
                    last_item_row = None;
                }
            } else {
                out.unused_rows.push(ri);
            }
            continue;
        }
        // `Contract No:` / `Due Date:` in a key-value layout; `March 01, 2024`.
        if letters(&desc) < 2
            || (!roles.from_header && desc.trim_end().ends_with(':'))
            || is_date_text(&desc)
        {
            out.unused_rows.push(ri);
            continue;
        }
        if !roles.matrix.is_empty() {
            for &c in &roles.matrix {
                let Some(a) = money_in(cell_text(row, c), hints).filter(|a| a.money) else {
                    continue;
                };
                let label = label_row
                    .and_then(|l| grid.rows.get(l))
                    .map(|l| cell_text(l, c))
                    .filter(|t| !t.is_empty());
                let d = match label {
                    Some(l) => format!("{} – {l}", desc.trim()),
                    None => desc.trim().to_owned(),
                };
                let mut it = Item::new(grid.page, grid.source, d);
                it.section.clone_from(&section);
                it.table_index = Some(table_index);
                it.row_index = u32::try_from(ri).ok();
                it.span = desc_span;
                it.bbox = row_box;
                it.unit_price = Some(a.num);
                it.currency = a.currency.or(table_currency).or(hints.currency);
                it.vat_included = hints.vat;
                if let Some((u, raw)) = a.unit.clone().or_else(|| unit_in_desc(&desc)) {
                    it.unit = Some(u);
                    it.unit_raw = Some(raw);
                }
                it.conf = (base_conf - 0.05).max(0.05);
                out.items.push(it);
            }
            last_item_row = Some(ri);
            continue;
        }
        let mut it = Item::new(grid.page, grid.source, desc.trim().to_owned());
        it.section.clone_from(&section);
        it.table_index = Some(table_index);
        it.row_index = u32::try_from(ri).ok();
        it.span = desc_span;
        it.bbox = row_box;
        it.code = code_col
            .map(|c| cell_text(row, c).to_owned())
            .filter(|s| !s.is_empty() && s.len() <= 40);
        // Quantity and unit.
        if let Some(q) = qty_col {
            let t = cell_text(row, q);
            if let Some(a) = amount_in(t, hints) {
                it.quantity = Some(a.num);
                if let Some((u, raw)) = a.unit {
                    it.unit = Some(u);
                    it.unit_raw = Some(raw);
                }
            }
        }
        if let Some(u) = unit_col {
            let t = cell_text(row, u);
            if !t.is_empty() {
                it.unit_raw = Some(t.to_owned());
                it.unit = unit::of_phrase(t);
            }
        }
        if it.unit.is_none()
            && let Some((u, raw)) = price
                .as_ref()
                .and_then(|p| p.unit.clone())
                .or_else(|| unit_in_desc(&it.description))
        {
            it.unit = Some(u);
            it.unit_raw = Some(raw);
        }
        it.unit_price = price.as_ref().map(|a| a.num);
        it.total = total.as_ref().map(|a| a.num);
        it.unit_price_gross = price_gross_col.and_then(|c| num_at(row, c, hints));
        it.total_gross = total_gross_col.and_then(|c| num_at(row, c, hints));
        if it.unit_price.is_none() && it.unit_price_gross.is_some() {
            std::mem::swap(&mut it.unit_price, &mut it.unit_price_gross);
        }
        it.currency = price
            .as_ref()
            .and_then(|a| a.currency)
            .or(total.as_ref().and_then(|a| a.currency))
            .or(cur_col.and_then(|c| num::currency_word(&unit::key(cell_text(row, c)), true)))
            .or(col_cur(price_col))
            .or(col_cur(total_col))
            .or(table_currency)
            .or(hints.currency);
        it.vat_rate = vat_col.and_then(|c| {
            num::scan(cell_text(row, c), hints.dec, hints.order)
                .into_iter()
                .find_map(|t| match t.kind {
                    NumKind::Percent(v) | NumKind::Number(v) => Some(v),
                    NumKind::Date(_) => None,
                })
        });
        it.vat_included = col_vat(price_col.or(total_col))
            .or_else(|| price_gross_col.map(|_| false))
            .or(hints.vat);
        // Dates on the line.
        let date_of = |c: Option<usize>| {
            c.and_then(|c| {
                num::scan(cell_text(row, c), hints.dec, hints.order)
                    .into_iter()
                    .find_map(|t| match t.kind {
                        NumKind::Date(d) => Some(d),
                        _ => None,
                    })
            })
        };
        it.valid_from = date_of(start_col);
        it.valid_to = date_of(end_col);
        // Confidence: the header, the arithmetic, the unit.
        let mut conf: f32 = base_conf;
        if let (Some(q), Some(p), Some(t)) = (it.quantity, it.unit_price, it.total) {
            if close(q.to_f64() * p.to_f64(), t.to_f64()) {
                conf += 0.1;
            } else {
                conf -= 0.1;
            }
        }
        if it.unit.is_some() {
            conf += 0.03;
        }
        it.conf = conf.clamp(0.05, 0.97);
        last_item_row = Some(ri);
        out.items.push(it);
    }
    out.roles = Some(roles);
    out
}

/// The net and gross columns of a role: a qualified pair, or the first column alone.
fn pick_pair(roles: &Roles, role: Role) -> (Option<usize>, Option<usize>) {
    let cols: Vec<usize> = roles.all(role).collect();
    let net = cols
        .iter()
        .copied()
        .find(|&c| roles.get(c).is_some_and(|h| h.vat == Some(false)));
    let gross = cols
        .iter()
        .copied()
        .find(|&c| roles.get(c).is_some_and(|h| h.vat == Some(true)));
    match (net, gross) {
        (Some(n), Some(g)) => (Some(n), Some(g)),
        (Some(n), None) => (Some(n), None),
        (None, Some(g)) => {
            let other = cols.iter().copied().find(|&c| c != g);
            match other {
                Some(o) => (Some(o), Some(g)),
                None => (Some(g), None),
            }
        }
        (None, None) => (cols.first().copied(), None),
    }
}

fn last_amount(row: &[Option<GCell>], hints: &DocHints) -> Option<(Num, Option<[u8; 3]>)> {
    row.iter().rev().flatten().find_map(|c| {
        num::scan(&c.text, hints.dec, hints.order)
            .into_iter()
            .rev()
            .find(NumTok::moneylike)
            .and_then(|t| t.number().map(|n| (n, t.currency)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&[&str]]) -> Grid {
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        Grid {
            page: 0,
            ncols,
            source: Source::Table,
            rows: rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|t| {
                            Some(GCell {
                                text: (*t).to_owned(),
                                span: Span { start: 0, end: 0 },
                                bbox: None,
                            })
                        })
                        .collect()
                })
                .collect(),
        }
    }

    fn hints() -> DocHints {
        DocHints {
            dec: num::Dec::Comma,
            ..DocHints::default()
        }
    }

    #[test]
    fn serbian_offer_table() {
        let g = grid(&[
            &[
                "R.br.",
                "Naziv usluge",
                "Jedinica mere",
                "Količina",
                "Jedinična cena bez PDV-a",
                "Ukupno bez PDV-a",
            ],
            &["1", "2", "3", "4", "5", "6=4x5"],
            &[
                "1.",
                "Servis klima uređaja",
                "sat",
                "10",
                "3.500,00",
                "35.000,00",
            ],
            &["2.", "Zamena filtera", "kom", "4", "1.200,00", "4.800,00"],
            &["", "Ukupno bez PDV-a", "", "", "", "39.800,00"],
            &["", "PDV 20%", "", "", "", "7.960,00"],
            &["", "Ukupno sa PDV-om", "", "", "", "47.760,00"],
        ]);
        let p = parse(&g, 0, None, &hints());
        assert_eq!(p.items.len(), 2, "{:?}", p.items);
        let a = &p.items[0];
        assert_eq!(a.description, "Servis klima uređaja");
        assert_eq!(a.unit, Some("h"));
        assert_eq!(a.quantity.map(|q| q.units), Some(10));
        assert_eq!(a.unit_price.map(|q| (q.units, q.scale)), Some((350_000, 2)));
        assert_eq!(a.total.map(|q| q.units), Some(3_500_000));
        assert_eq!(a.vat_included, Some(false));
        assert!(a.conf > 0.9);
        let kinds: Vec<_> = p.totals.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, ["net", "vat", "gross"]);
    }

    #[test]
    fn english_invoice_with_currency_column_header() {
        let g = grid(&[
            &["Item", "Description", "Qty", "Unit Price (USD)", "Amount"],
            &["A-100", "Cloud hosting, monthly", "3", "120.00", "360.00"],
            &["B-200", "Support hours", "5", "85.00", "425.00"],
            &["", "Subtotal", "", "", "785.00"],
        ]);
        let p = parse(&g, 0, None, &DocHints::default());
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].currency, Some(*b"USD"));
        assert_eq!(p.items[0].unit, Some("month"));
        assert_eq!(p.items[1].description, "Support hours");
        assert_eq!(p.totals[0].kind, "subtotal");
    }

    #[test]
    fn headerless_rate_card_is_inferred() {
        let g = grid(&[
            &["Senior consultant", "85,00 EUR"],
            &["Junior consultant", "45,00 EUR"],
            &["Project manager", "95,00 EUR"],
        ]);
        let p = parse(&g, 0, None, &hints());
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items[2].unit_price.map(|n| n.units), Some(9500));
        assert_eq!(p.items[2].currency, Some(*b"EUR"));
    }

    #[test]
    fn subheader_net_gross() {
        let g = grid(&[
            &["Opis", "Cena", ""],
            &["", "bez PDV", "sa PDV"],
            &["Parking 1h", "100,00", "120,00"],
        ]);
        let p = parse(&g, 0, None, &hints());
        assert_eq!(p.items.len(), 1, "{p:?}");
        assert_eq!(p.items[0].unit_price.map(|n| n.units), Some(10_000));
        assert_eq!(p.items[0].unit_price_gross.map(|n| n.units), Some(12_000));
        assert_eq!(p.items[0].vat_included, Some(false));
    }

    #[test]
    fn continuation_reuses_previous_header() {
        let g1 = grid(&[
            &["Opis", "Kol.", "Cena", "Iznos"],
            &["A usluga", "1", "10,00", "10,00"],
        ]);
        let p1 = parse(&g1, 0, None, &hints());
        let g2 = grid(&[&["B usluga", "2", "5,00", "10,00"]]);
        let p2 = parse(&g2, 1, p1.roles.as_ref(), &hints());
        assert_eq!(p2.items.len(), 1);
        assert_eq!(p2.items[0].quantity.map(|n| n.units), Some(2));
    }

    #[test]
    fn total_kinds() {
        assert_eq!(total_kind("Ukupno bez PDV-a"), Some("net"));
        assert_eq!(total_kind("УКУПНО СА ПДВ"), Some("gross"));
        assert_eq!(total_kind("Total incl. VAT"), Some("gross"));
        assert_eq!(total_kind("Grand Total"), Some("total"));
        assert_eq!(total_kind("Servis klima"), None);
    }
}

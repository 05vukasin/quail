//! `RealKIE` `fcc_invoices` (Townsend et al., 2024, <https://github.com/IndicoDataSolutions/RealKIE>,
//! CC BY-NC 4.0): 370 US broadcast (FCC political ad) invoices with span labels, as price
//! rows for the `items` task.
//!
//! Layout read (the dataset as published):
//!
//! ```text
//! <root>/{train,val,test}.csv     columns: document_path, labels (JSON), ocr, text, ...
//! <root>/files/<id>.pdf           the invoice
//! <root>/ocr/<id>.json.gz         OCR per page: [{"pages": [{"doc_offset": {start, end}}]}, ...]
//! ```
//!
//! `labels` are character spans in `text` (the OCR text of all pages joined), with labels
//! `Line Item - Start Date / End Date / Description / Rate / Days` and document fields. Line
//! item spans are grouped into rows in text order: a row ends when a label it already has
//! comes again. A row with a `Rate` becomes a gold row `{description, unit_price,
//! currency}` on the page the rate is on (from the OCR page offsets); `currency` is `USD`
//! when a `$` is written right before the rate. Rows without a rate are skipped (counted).
//! The gold labels per-spot rates and the flight summary lines alike, so a document often has
//! the same rate many times; the task's matching is one to one.
//!
//! Status: run on the real dataset (370 documents, all three CSV files).

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::Path;

use serde::Deserialize;

use super::{ConvertOptions, Converted, GoldAdapter, assign_split, gold_file};
use crate::gold::{GoldDoc, Label, MatchMode, Split};
use crate::items::GoldRow;
use crate::{Error, Result};

/// The `RealKIE` FCC invoices adapter (`realkie-fcc`).
#[derive(Clone, Copy, Debug, Default)]
pub struct RealkieFcc;

/// The task its rows are written under.
pub const TASK: &str = "items";

#[derive(Deserialize)]
pub(crate) struct Span {
    label: String,
    start: usize,
    text: String,
}

#[derive(Deserialize)]
struct OcrPage {
    pages: Vec<PageInfo>,
}

#[derive(Deserialize)]
struct PageInfo {
    doc_offset: Offset,
}

#[derive(Deserialize)]
struct Offset {
    start: usize,
    end: usize,
}

/// Splits CSV text (RFC 4180: `,` separated, `"` quoted, `""` escapes, newlines inside
/// quotes) into records of fields.
pub(crate) fn csv_records(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut rec: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => rec.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                rec.push(std::mem::take(&mut field));
                out.push(std::mem::take(&mut rec));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !rec.is_empty() {
        rec.push(field);
        out.push(rec);
    }
    out
}

/// `$1,234.50` / `275.00` as a number.
fn rate(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    t.parse().ok()
}

/// Page of a character offset, from the OCR page offsets.
fn page_of(pages: &[(usize, usize)], at: usize) -> Option<u32> {
    let i = pages
        .iter()
        .position(|&(s, e)| (s..=e).contains(&at))
        .or_else(|| pages.iter().rposition(|&(s, _)| s <= at))?;
    u32::try_from(i).ok()
}

/// Gold rows of one document from its span labels.
pub(crate) fn rows(labels: &[Span], text: &str, pages: &[(usize, usize)]) -> (Vec<Label>, u64) {
    let mut spans: Vec<&Span> = labels
        .iter()
        .filter(|s| s.label.starts_with("Line Item - "))
        .collect();
    spans.sort_by_key(|s| s.start);
    let mut groups: Vec<Vec<&Span>> = Vec::new();
    let mut cur: Vec<&Span> = Vec::new();
    for s in spans {
        // A new row starts at a label the current row already has, or at a start date after
        // a rate (the next row's first column).
        let repeats = cur.iter().any(|c| c.label == s.label);
        let next_row =
            s.label.ends_with("Start Date") && cur.iter().any(|c| c.label.ends_with("Rate"));
        if repeats || next_row {
            groups.push(std::mem::take(&mut cur));
        }
        cur.push(s);
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    // Span offsets count characters (Python indices); `$` is looked up by byte.
    let byte_at: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let mut out = Vec::new();
    let mut no_rate = 0;
    for g in groups {
        let Some(r) = g.iter().find(|s| s.label.ends_with("Rate")) else {
            no_rate += 1;
            continue;
        };
        let Some(price) = rate(&r.text) else {
            no_rate += 1;
            continue;
        };
        let at = byte_at.get(r.start).copied().unwrap_or(text.len());
        let dollar =
            text.get(..at).is_some_and(|h| h.trim_end().ends_with('$')) || r.text.contains('$');
        let row = GoldRow {
            description: g
                .iter()
                .find(|s| s.label.ends_with("Description"))
                .map(|s| s.text.trim().to_string()),
            unit_price: Some(price),
            currency: dollar.then(|| "USD".to_string()),
            ..GoldRow::default()
        };
        let Ok(value) = serde_json::to_value(&row) else {
            continue;
        };
        out.push(Label {
            task: TASK.into(),
            value,
            mode: MatchMode::Row,
            tol: None,
            page: page_of(pages, r.start),
            note: None,
        });
    }
    (out, no_rate)
}

fn read_pages(path: &Path) -> Result<Vec<(usize, usize)>> {
    let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut json = String::new();
    flate2::read::GzDecoder::new(f)
        .read_to_string(&mut json)
        .map_err(|e| Error::io(path, e))?;
    let pages: Vec<OcrPage> = serde_json::from_str(&json).map_err(|e| Error::parse(path, e))?;
    Ok(pages
        .iter()
        .filter_map(|p| p.pages.first())
        .map(|p| (p.doc_offset.start, p.doc_offset.end))
        .collect())
}

fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

impl GoldAdapter for RealkieFcc {
    fn name(&self) -> &'static str {
        "realkie-fcc"
    }

    fn convert(&self, root: &Path, opts: &ConvertOptions) -> Result<Converted> {
        let mut out = Converted::default();
        let mut seen = BTreeSet::new();
        for split in ["train", "val", "test"] {
            let csv = root.join(format!("{split}.csv"));
            if !csv.is_file() {
                continue;
            }
            let records = csv_records(&crate::read_to_string(&csv)?);
            let Some((head, body)) = records.split_first() else {
                continue;
            };
            let col = |name: &str| {
                head.iter()
                    .position(|h| h == name)
                    .ok_or_else(|| Error::parse(&csv, format!("no column {name}")))
            };
            let (c_doc, c_labels, c_ocr, c_text) = (
                col("document_path")?,
                col("labels")?,
                col("ocr")?,
                col("text")?,
            );
            for rec in body {
                let get = |i: usize| rec.get(i).map_or("", String::as_str);
                let name = base_name(get(c_doc));
                if name.is_empty() || !seen.insert(name.to_string()) {
                    continue;
                }
                out.bump("documents");
                let labels: Vec<Span> = serde_json::from_str(get(c_labels))
                    .map_err(|e| Error::parse(&csv, format!("{name}: labels: {e}")))?;
                let ocr = root.join("ocr").join(base_name(get(c_ocr)));
                let pages = if ocr.is_file() {
                    read_pages(&ocr)?
                } else {
                    out.bump("ocr_not_found");
                    Vec::new()
                };
                let (rows, no_rate) = rows(&labels, get(c_text), &pages);
                for _ in 0..no_rate {
                    out.bump("rows_without_rate");
                }
                if rows.is_empty() {
                    out.bump("documents_without_rows");
                }
                for _ in &rows {
                    out.bump("rows");
                }
                let pdf = root.join("files").join(name);
                let file = if pdf.is_file() {
                    Some(pdf.as_path())
                } else {
                    out.bump("file_not_found");
                    None
                };
                let mut doc = GoldDoc {
                    file: gold_file(root, file, &format!("files/{name}")),
                    dataset: self.name().into(),
                    split: Split::Dev,
                    source: Some(format!("realkie/fcc_invoices/{split}")),
                    labels: rows,
                    absent: Vec::new(),
                };
                if doc.labels.is_empty() {
                    doc.absent.push(TASK.into());
                }
                assign_split(&mut doc, opts);
                out.docs.push(doc);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_and_newlines() {
        let r = csv_records("a,b,c\n1,\"x, \"\"y\"\"\nz\",3\n");
        assert_eq!(r.len(), 2);
        assert_eq!(r[1], ["1", "x, \"y\"\nz", "3"]);
    }

    fn span(label: &str, start: usize, text: &str) -> Span {
        Span {
            label: format!("Line Item - {label}"),
            start,
            text: text.into(),
        }
    }

    #[test]
    fn rows_from_spans() {
        let text = "xx 11/04/16 ELLEN $275.00 | 11/07/16 ELLEN EF 300.00 | 11/08/16 NEWS";
        let labels = vec![
            span("Start Date", 3, "11/04/16"),
            span("Description", 12, "ELLEN"),
            span("Rate", 19, "275.00"),
            span("Start Date", 28, "11/07/16"),
            span("Description", 37, "ELLEN EF"),
            span("Rate", 46, "300.00"),
            span("Start Date", 55, "11/08/16"),
            span("Description", 64, "NEWS"),
        ];
        let (rows, no_rate) = rows(&labels, text, &[(0, 30), (31, 80)]);
        assert_eq!(no_rate, 1);
        assert_eq!(rows.len(), 2);
        let r0 = GoldRow::of(&rows[0]).unwrap();
        assert_eq!(r0.description.as_deref(), Some("ELLEN"));
        assert_eq!(r0.unit_price, Some(275.0));
        assert_eq!(r0.currency.as_deref(), Some("USD"));
        assert_eq!(rows[0].page, Some(0));
        let r1 = GoldRow::of(&rows[1]).unwrap();
        assert_eq!(r1.currency, None);
        assert_eq!(rows[1].page, Some(1));
    }
}

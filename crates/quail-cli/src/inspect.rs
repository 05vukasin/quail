//! `quail inspect`: one file, printed for a human (or an agent) to read.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

use quail_core::{BlockKind, Document, Evidence, Fact, FileRecord, Status, Value, Zone};
use quail_io::{ReadOptions, read_file};

use crate::cli::InspectArgs;
use crate::dump::hex;
use crate::registry;

/// Text values longer than this are cut in the fact listing.
const MAX_TEXT: usize = 160;
/// List items shown per fact.
const MAX_ITEMS: usize = 8;

/// Runs the command; returns what to print.
pub fn run(args: &InspectArgs) -> Result<(String, ExitCode), String> {
    #[cfg(feature = "ocr")]
    let ocr_args = &args.ocr;
    #[cfg(not(feature = "ocr"))]
    let ocr_args = &crate::cli::OcrArgs::default();
    // One file: one OCR job, with all the threads it can use.
    let plan = crate::scan::plan(args.profile.into(), &args.limits, ocr_args, Some(1), false);
    let read = ReadOptions::new().max_file_bytes(Some(plan.limits.max_bytes));
    let item = read_file(&args.file, &read).map_err(|e| e.to_string())?;
    crate::scan::warn_without_ocr(args.profile.into());
    let engine = registry::builder_with(args.profile.into(), plan.limits, plan.ocr.as_ref())
        .build()
        .map_err(|e| e.to_string())?;
    let mut doc = None;
    let rec = engine.process_with(item, |_, artifacts| {
        if args.text {
            doc = artifacts.get::<Document>().cloned();
        }
    });

    let code = match rec.status {
        Status::Failed { .. } => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    };
    let out = if args.json {
        let v = if args.text {
            serde_json::json!({ "record": rec, "document": doc })
        } else {
            serde_json::to_value(&rec).map_err(|e| e.to_string())?
        };
        let mut s = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        s.push('\n');
        s
    } else {
        let mut s = render_record(&rec);
        if args.text {
            match &doc {
                Some(d) => s.push_str(&render_document(d)),
                None => s.push_str("\n(no document: no extractor for this format, or it failed)\n"),
            }
        }
        s
    };
    Ok((out, code))
}

/// The record as text: header, status, steps, then facts grouped by namespace.
pub fn render_record(rec: &FileRecord) -> String {
    let mut s = String::new();
    let mime_key = quail_engine::keys::MIME;
    let mime = rec
        .facts_for(mime_key.as_str())
        .find_map(|f| match &f.value {
            Value::Text(t) => Some(&**t),
            _ => None,
        })
        .unwrap_or("");
    let _ = writeln!(s, "file     {}", rec.locator);
    let _ = write!(
        s,
        "format   {} ({})   size {}",
        rec.format.as_str(),
        mime,
        human_bytes(rec.size)
    );
    if let Some(p) = rec.pages {
        let _ = write!(s, "   pages {p}");
    }
    s.push('\n');
    match &rec.status {
        Status::Ok => s.push_str("status   ok\n"),
        Status::Partial { problems } => {
            let _ = writeln!(s, "status   partial");
            for p in problems {
                let _ = writeln!(s, "  problem  {p}");
            }
        }
        Status::Failed { error } => {
            let _ = writeln!(s, "status   failed: {error}");
        }
    }
    let steps: Vec<String> = rec
        .steps
        .iter()
        .map(|id| match rec.timings.iter().find(|t| t.src == *id) {
            Some(t) => format!("{id} ({})", human_micros(t.micros)),
            None => id.to_string(),
        })
        .collect();
    let _ = writeln!(s, "steps    {}", steps.join(", "));
    for w in &rec.warnings {
        let _ = writeln!(s, "  warning  {w}");
    }

    let mut groups: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
    for f in &rec.facts {
        groups.entry(f.key.namespace()).or_default().push(f);
    }
    for (ns, facts) in groups {
        let _ = writeln!(s, "\n[{ns}] {} fact(s)", facts.len());
        let width = facts
            .iter()
            .map(|f| f.key.as_str().len())
            .max()
            .unwrap_or(0);
        for f in facts {
            // A list of records (ranked candidates) gets one line per item below the fact.
            let items = match &f.value {
                Value::List(items)
                    if !items.is_empty() && items.iter().all(|x| matches!(x, Value::Record(_))) =>
                {
                    Some(items)
                }
                _ => None,
            };
            let shown = match items {
                Some(items) => format!("{} record(s)", items.len()),
                None => fmt_value(&f.value, MAX_TEXT),
            };
            let _ = write!(s, "  {:width$}  {shown}", f.key.as_str());
            if f.conf < 1.0 {
                let _ = write!(s, "  conf {:.2}", f.conf);
            }
            let _ = write!(s, "  src {}", f.src);
            if let Some(e) = &f.evidence {
                let _ = write!(s, "  at {}", fmt_evidence(e));
            }
            s.push('\n');
            for item in items.into_iter().flatten() {
                let _ = writeln!(s, "  {:width$}    - {}", "", fmt_value(item, MAX_TEXT));
            }
        }
    }
    s
}

/// Every page: a header line, then each block with its zone and kind.
pub fn render_document(doc: &Document) -> String {
    let mut s = String::new();
    for p in &doc.pages {
        let _ = write!(
            s,
            "\n--- page {} ({}, text {}, {} block(s)",
            p.index + 1,
            format!("{:?}", p.kind).to_lowercase(),
            format!("{:?}", p.text_source).to_lowercase(),
            p.blocks.len()
        );
        if let Some((w, h)) = p.size {
            let _ = write!(s, ", {w:.0}x{h:.0} pt");
        }
        if !p.images.is_empty() {
            let _ = write!(
                s,
                ", {} image(s) covering {:.0}%",
                p.images.len(),
                p.image_coverage * 100.0
            );
        }
        if p.needs_ocr {
            s.push_str(", needs OCR");
        }
        s.push_str(") ---\n");
        if p.blocks.is_empty() {
            if !p.text.is_empty() {
                let _ = writeln!(s, "{}", p.text);
            }
            continue;
        }
        for b in &p.blocks {
            let label = format!("{}/{}", zone_name(b.zone), kind_name(b.kind));
            let text = b.span.slice(&p.text);
            let mut lines = text.lines();
            let first = lines.next().unwrap_or("");
            let _ = writeln!(s, "[{label:<16}] {first}");
            for l in lines {
                let _ = writeln!(s, "{:19}{l}", "");
            }
        }
    }
    s
}

fn zone_name(z: Zone) -> &'static str {
    match z {
        Zone::Unknown => "unknown",
        Zone::Header => "header",
        Zone::Footer => "footer",
        Zone::Body => "body",
        Zone::Title => "title",
        Zone::Table => "table",
        Zone::Signature => "signature",
    }
}

fn kind_name(k: BlockKind) -> String {
    match k {
        BlockKind::Paragraph => "para".to_owned(),
        BlockKind::Heading { level } => format!("h{level}"),
        BlockKind::ListItem => "item".to_owned(),
        BlockKind::TableCell { table, row, col } => format!("t{table}r{row}c{col}"),
        BlockKind::Note => "note".to_owned(),
        BlockKind::Annotation => "annot".to_owned(),
    }
}

fn fmt_evidence(e: &Evidence) -> String {
    let mut parts = Vec::new();
    if let Some(p) = e.page {
        parts.push(format!("p{}", p + 1));
    }
    if e.zone != Zone::Unknown {
        parts.push(zone_name(e.zone).to_owned());
    }
    if let Some(sp) = e.span {
        parts.push(format!("[{}..{}]", sp.start, sp.end));
    }
    if let Some(b) = e.bbox {
        parts.push(format!(
            "box({:.0},{:.0},{:.0},{:.0})",
            b.x0, b.y0, b.x1, b.y1
        ));
    }
    parts.join(" ")
}

/// A value on one line; text cut at `max` chars.
pub fn fmt_value(v: &Value, max: usize) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(x) => fmt_float(*x),
        Value::Text(t) => {
            let one_line: String = t
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let n = one_line.chars().count();
            if n > max {
                let cut: String = one_line.chars().take(max).collect();
                format!("{cut:?}… ({n} chars)")
            } else {
                format!("{one_line:?}")
            }
        }
        Value::U64(x) => format!("{x:#018x}"),
        Value::Bytes(b) => {
            if b.len() > 32 {
                format!("{}… ({} bytes)", hex(&b[..32]), b.len())
            } else {
                hex(b)
            }
        }
        Value::Date(d) => format!("{:04}-{:02}-{:02}", d.y, d.m, d.d),
        Value::Money(m) => {
            let mut s = decimal(m.units, m.scale);
            if let Some(c) = m.currency {
                s.push(' ');
                s.push_str(&String::from_utf8_lossy(&c));
            }
            s
        }
        Value::List(items) => {
            let shown: Vec<String> = items
                .iter()
                .take(MAX_ITEMS)
                .map(|x| fmt_value(x, 40))
                .collect();
            let more = if items.len() > MAX_ITEMS {
                format!(", … ({} items)", items.len())
            } else {
                String::new()
            };
            format!("[{}{more}]", shown.join(", "))
        }
        Value::Record(fields) => {
            let shown: Vec<String> = fields
                .iter()
                .map(|(k, x)| format!("{k}={}", fmt_value(x, 40)))
                .collect();
            format!("{{{}}}", shown.join(" "))
        }
    }
}

/// A float for reading: four decimals at most (trailing zeros dropped), scientific below
/// 0.001. Scores and confidences are `f32` widened to `f64`; their full digits are noise.
fn fmt_float(x: f64) -> String {
    if x == 0.0 || !x.is_finite() || (x.fract() == 0.0 && x.abs() < 1e15) {
        return format!("{x}");
    }
    if x.abs() < 1e-3 {
        return format!("{x:.1e}");
    }
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn decimal(units: i64, scale: u8) -> String {
    let neg = units < 0;
    let digits = units.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let s = if scale == 0 {
        digits
    } else if digits.len() > scale {
        let (a, b) = digits.split_at(digits.len() - scale);
        format!("{a}.{b}")
    } else {
        format!("0.{digits:0>scale$}")
    };
    if neg { format!("-{s}") } else { s }
}

#[allow(clippy::cast_precision_loss)] // display only
fn human_bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} kB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}

#[allow(clippy::cast_precision_loss)] // display only
fn human_micros(us: u64) -> String {
    if us < 1_000 {
        format!("{us} µs")
    } else if us < 1_000_000 {
        format!("{:.1} ms", us as f64 / 1e3)
    } else {
        format!("{:.2} s", us as f64 / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use quail_core::Money;

    use super::*;

    #[test]
    fn values() {
        assert_eq!(fmt_value(&Value::text("a\nb"), 10), "\"a b\"");
        assert!(fmt_value(&Value::text("x".repeat(50)), 10).ends_with("(50 chars)"));
        let m = Money {
            units: -5,
            scale: 2,
            currency: Some(*b"EUR"),
        };
        assert_eq!(fmt_value(&Value::Money(m), 10), "-0.05 EUR");
        assert_eq!(decimal(420_050, 2), "4200.50");
        let l = Value::List((0..10).map(Value::Int).collect());
        assert_eq!(fmt_value(&l, 10), "[0, 1, 2, 3, 4, 5, 6, 7, … (10 items)]");
        assert_eq!(fmt_value(&Value::Float(f64::from(0.8_f32)), 10), "0.8");
        assert_eq!(fmt_value(&Value::Float(1.4e-11), 10), "1.4e-11");
        assert_eq!(fmt_value(&Value::Float(2.0), 10), "2");
        let r = Value::record([
            ("surface", Value::text("Alpha LLC")),
            ("roles", Value::List(vec![Value::text("supplier")])),
            ("score", Value::Float(f64::from(0.97_f32))),
        ]);
        assert_eq!(
            fmt_value(&r, 40),
            "{surface=\"Alpha LLC\" roles=[\"supplier\"] score=0.97}"
        );
    }
}

//! Reading quail records back and finding the record of a gold document.

use std::collections::HashMap;
use std::path::Path;

use quail_core::{FileRecord, Value};
use quail_io::JsonlReader;

use crate::Result;
use crate::gold::GoldFile;

/// The key of the BLAKE3 fact (`quail-hash`), used to match files to gold.
pub const BLAKE3_KEY: &str = "hash.blake3";

/// Records of one run.
#[derive(Debug, Default)]
pub struct Records {
    /// Records in file order.
    pub records: Vec<FileRecord>,
    /// Lines that could not be parsed (reported, not fatal).
    pub bad_lines: u64,
}

/// Reads a JSONL file written by [`quail_io::JsonlSink`]. Unparsable lines are counted and
/// skipped; failing to open the file is an error.
pub fn load(path: &Path) -> Result<Records> {
    let reader = JsonlReader::open(path)?;
    let mut out = Records::default();
    for item in reader {
        match item {
            Ok(r) => out.records.push(r),
            Err(_) => out.bad_lines += 1,
        }
    }
    Ok(out)
}

/// Lowercase hex of the record's `hash.blake3` fact.
pub fn blake3_of(rec: &FileRecord) -> Option<String> {
    rec.facts_for(BLAKE3_KEY).find_map(|f| match &f.value {
        Value::Bytes(b) => Some(crate::hex(b)),
        Value::Text(t) => Some(t.to_lowercase()),
        _ => None,
    })
}

/// A fact value as one or more strings: lists are flattened, dates are ISO, money is a plain
/// decimal (`4200.50`), bytes are hex.
pub fn value_strings(v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => items.iter().flat_map(value_strings).collect(),
        other => vec![value_string(other)],
    }
}

/// The strings of `v` for a task reading record field `field`: records give the strings of
/// that field (none when it is missing), lists are flattened, other values are as in
/// [`value_strings`]. Without a field this is [`value_strings`].
pub fn field_strings(v: &Value, field: Option<&str>) -> Vec<String> {
    let Some(name) = field else {
        return value_strings(v);
    };
    match v {
        Value::Record(_) => v.field(name).map(value_strings).unwrap_or_default(),
        Value::List(items) => items
            .iter()
            .flat_map(|x| field_strings(x, Some(name)))
            .collect(),
        other => vec![value_string(other)],
    }
}

/// A fact value as one display string.
pub fn value_string(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Text(t) => t.to_string(),
        Value::U64(n) => n.to_string(),
        Value::Bytes(b) => crate::hex(b),
        Value::Date(d) => format!("{:04}-{:02}-{:02}", d.y, d.m, d.d),
        Value::Money(m) => {
            let neg = m.units < 0;
            let abs = m.units.unsigned_abs().to_string();
            let scale = usize::from(m.scale);
            let padded = format!("{abs:0>width$}", width = scale + 1);
            let (int, frac) = padded.split_at(padded.len() - scale);
            let mut s = String::new();
            if neg {
                s.push('-');
            }
            s.push_str(int);
            if scale > 0 {
                s.push('.');
                s.push_str(frac);
            }
            s
        }
        Value::List(items) => items
            .iter()
            .map(value_string)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Record(fields) => fields
            .iter()
            .map(|(k, x)| format!("{k}={}", value_string(x)))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Looks records up by BLAKE3, then by locator.
#[derive(Debug, Default)]
#[allow(clippy::struct_field_names)]
pub struct RecordIndex {
    by_blake3: HashMap<String, usize>,
    by_locator: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
}

/// How a gold document was matched to a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchedBy {
    /// Same BLAKE3.
    Blake3,
    /// Same locator, or a locator ending with the hint at a path boundary (unique).
    Locator,
}

fn file_name(loc: &str) -> &str {
    loc.rsplit(['/', '\\']).next().unwrap_or(loc)
}

impl RecordIndex {
    /// Indexes `records`. With duplicates, the first record wins.
    pub fn new(records: &[FileRecord]) -> Self {
        let mut ix = Self::default();
        for (i, r) in records.iter().enumerate() {
            if let Some(h) = blake3_of(r) {
                ix.by_blake3.entry(h).or_insert(i);
            }
            ix.by_locator.entry(r.locator.clone()).or_insert(i);
            ix.by_name
                .entry(file_name(&r.locator).to_string())
                .or_default()
                .push(i);
        }
        ix
    }

    /// The record of a gold file.
    pub fn find(&self, records: &[FileRecord], file: &GoldFile) -> Option<(usize, MatchedBy)> {
        if let Some(h) = &file.blake3
            && let Some(&i) = self.by_blake3.get(h)
        {
            return Some((i, MatchedBy::Blake3));
        }
        let hint = file.locator_hint.as_deref()?;
        if let Some(&i) = self.by_locator.get(hint) {
            return Some((i, MatchedBy::Locator));
        }
        let hint_norm = hint.replace('\\', "/");
        let hint_norm = hint_norm.trim_start_matches("./");
        let cands: Vec<usize> = self
            .by_name
            .get(file_name(hint_norm))?
            .iter()
            .copied()
            .filter(|&i| {
                let loc = records[i].locator.replace('\\', "/");
                loc == hint_norm || loc.ends_with(&format!("/{hint_norm}"))
            })
            .collect();
        match cands.as_slice() {
            [i] => Some((*i, MatchedBy::Locator)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Money, Status};

    fn rec(locator: &str) -> FileRecord {
        FileRecord {
            schema: 1,
            locator: locator.into(),
            size: 0,
            format: quail_core::Format::Text,
            pages: None,
            facts: vec![],
            steps: vec![],
            timings: vec![],
            status: Status::Ok,
            warnings: vec![],
        }
    }

    #[test]
    fn money_and_lists() {
        let m = |units, scale| {
            value_string(&Value::Money(Money {
                units,
                scale,
                currency: None,
            }))
        };
        assert_eq!(m(420_050, 2), "4200.50");
        assert_eq!(m(-5, 2), "-0.05");
        assert_eq!(m(7, 0), "7");
        let l = Value::List(vec![Value::text("a"), Value::List(vec![Value::Int(1)])]);
        assert_eq!(value_strings(&l), vec!["a", "1"]);
    }

    #[test]
    fn record_fields() {
        let r = |s: &str| Value::record([("surface", Value::text(s)), ("key", Value::text("k"))]);
        assert_eq!(
            field_strings(&r("Alpha LLC"), Some("surface")),
            ["Alpha LLC"]
        );
        assert!(field_strings(&r("x"), Some("missing")).is_empty());
        let l = Value::List(vec![
            r("A"),
            Value::record([("other", Value::Int(1))]),
            r("B"),
        ]);
        assert_eq!(field_strings(&l, Some("surface")), ["A", "B"]);
        assert_eq!(
            field_strings(&Value::text("plain"), Some("surface")),
            ["plain"]
        );
        assert_eq!(field_strings(&r("A"), None), ["surface=A, key=k"]);
    }

    #[test]
    fn locator_fallback_is_unique_suffix() {
        let rs = vec![
            rec("/data/a/x.pdf"),
            rec("/data/b/x.pdf"),
            rec("/data/b/y.pdf"),
        ];
        let ix = RecordIndex::new(&rs);
        let f = |h: &str| GoldFile {
            blake3: None,
            locator_hint: Some(h.into()),
        };
        assert_eq!(ix.find(&rs, &f("b/x.pdf")), Some((1, MatchedBy::Locator)));
        assert_eq!(ix.find(&rs, &f("y.pdf")), Some((2, MatchedBy::Locator)));
        assert_eq!(ix.find(&rs, &f("x.pdf")), None, "ambiguous");
        assert_eq!(ix.find(&rs, &f("ab/x.pdf")), None, "not at a path boundary");
    }
}

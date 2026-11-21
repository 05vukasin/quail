//! Gold files: what a document is known to contain, one JSON file per document.
//!
//! Gold lives outside the repository (it describes real documents), in a dataset directory:
//!
//! ```json
//! {
//!   "file": { "blake3": "<64 hex>", "locator_hint": "nda/0001.pdf" },
//!   "dataset": "kleister-nda",
//!   "split": "dev",
//!   "labels": [
//!     { "task": "party", "value": "Example Corp", "match": "normalized" },
//!     { "task": "effective_date", "value": "2001-02-03", "match": "normalized", "page": 0 },
//!     { "task": "amount_total", "value": 4200.5, "match": "numeric_tol", "tol": 0.01 },
//!     { "task": "language", "value": ["sr", "hr"], "match": "any_of" }
//!   ],
//!   "absent": ["iban"]
//! }
//! ```
//!
//! A task listed in `labels` has those values; a task in `absent` is known to have none (any
//! prediction is a false positive); any other task is not annotated for this document and is
//! not scored on it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Which side of the frozen dev/holdout split a document is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Looked at while developing.
    Dev,
    /// Never looked at while developing; scored only for releases.
    Holdout,
}

impl Split {
    /// `dev` or `holdout`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Holdout => "holdout",
        }
    }
}

/// How a gold value is compared with a predicted one. The task's normalizer (from
/// `tasks.toml`) applies first in every mode except `exact`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Same text after trimming.
    Exact,
    /// Same text after normalization (the task's normalizer, or `text` when it has none).
    #[default]
    Normalized,
    /// After normalization, the tokens of one value are a contiguous run in the other's
    /// (either way round), and the shorter has at least 3 characters.
    Contains,
    /// `value` is a list of acceptable alternatives; any one matching (normalized) counts.
    AnyOf,
    /// Both parse as numbers and differ by at most `tol` (default 0.01).
    NumericTol,
    /// `value` is an object, one gold row of a rows task (`items`): scored by
    /// [`crate::items`], never by value comparison.
    Row,
}

/// One known value of one task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Label {
    /// Neutral task name, e.g. `party`.
    pub task: String,
    /// The value: a string, number or boolean, or a list of alternatives for `any_of`.
    pub value: serde_json::Value,
    /// How to compare it.
    #[serde(rename = "match", default)]
    pub mode: MatchMode,
    /// Absolute tolerance for `numeric_tol`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol: Option<f64>,
    /// Zero-based page where it appears, when known (shown in inspection packs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// Free note for annotators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Label {
    /// A label with a text value and the default match mode.
    pub fn text(task: impl Into<String>, value: impl Into<String>, mode: MatchMode) -> Self {
        Self {
            task: task.into(),
            value: serde_json::Value::String(value.into()),
            mode,
            tol: None,
            page: None,
            note: None,
        }
    }

    /// The value as one or more strings: a list gives its items, a scalar gives itself, a
    /// row its display form (`description @ price`).
    pub fn alternatives(&self) -> Vec<String> {
        match &self.value {
            serde_json::Value::Object(_) => crate::items::GoldRow::of(self)
                .map(|r| r.display())
                .into_iter()
                .collect(),
            serde_json::Value::Array(items) => items.iter().filter_map(scalar_string).collect(),
            v => scalar_string(v).into_iter().collect(),
        }
    }

    /// The value as one display string.
    pub fn display(&self) -> String {
        let alts = self.alternatives();
        if alts.len() == 1 {
            alts.into_iter().next().unwrap_or_default()
        } else {
            alts.join(" | ")
        }
    }
}

fn scalar_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Which file a gold document describes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldFile {
    /// Lowercase hex BLAKE3 of the file's bytes: the primary key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
    /// Path (or its tail) of the file, used when no record has the hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator_hint: Option<String>,
}

/// Everything known about one document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldDoc {
    /// The file.
    pub file: GoldFile,
    /// Dataset name, e.g. `kleister-nda`.
    pub dataset: String,
    /// Dev or holdout.
    pub split: Split,
    /// Where the labels came from (e.g. `kleister-nda/train`), for bookkeeping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Known values.
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Tasks known to have no value in this document.
    #[serde(default)]
    pub absent: Vec<String>,
}

impl GoldDoc {
    /// Stable id: the BLAKE3 hex, or `loc:<hint>` when the hash is unknown.
    pub fn id(&self) -> String {
        match (&self.file.blake3, &self.file.locator_hint) {
            (Some(h), _) => h.clone(),
            (None, Some(l)) => format!("loc:{l}"),
            (None, None) => String::from("loc:?"),
        }
    }

    /// Tasks with at least one label.
    pub fn labelled_tasks(&self) -> BTreeSet<&str> {
        self.labels.iter().map(|l| l.task.as_str()).collect()
    }

    /// Labels of one task.
    pub fn labels_for<'a>(&'a self, task: &'a str) -> impl Iterator<Item = &'a Label> + 'a {
        self.labels.iter().filter(move |l| l.task == task)
    }

    /// Whether `task` is known to be absent.
    pub fn is_absent(&self, task: &str) -> bool {
        self.absent.iter().any(|t| t == task)
    }

    /// Checks the document is well-formed: a file key, a valid hash, list values only for
    /// `any_of`, no task both labelled and absent.
    pub fn validate(&self) -> Result<()> {
        let id = self.id();
        if self.file.blake3.is_none() && self.file.locator_hint.is_none() {
            return Err(Error::Invalid(format!(
                "gold document in {}: needs file.blake3 or file.locator_hint",
                self.dataset
            )));
        }
        if let Some(h) = &self.file.blake3
            && (h.len() != 64
                || !h
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(Error::Invalid(format!(
                "{id}: file.blake3 must be 64 lowercase hex characters"
            )));
        }
        for l in &self.labels {
            if l.task.is_empty() {
                return Err(Error::Invalid(format!("{id}: label without task")));
            }
            let is_list = l.value.is_array();
            if is_list != (l.mode == MatchMode::AnyOf) {
                return Err(Error::Invalid(format!(
                    "{id}: task {}: a list value goes with match any_of and only with it",
                    l.task
                )));
            }
            if l.value.is_object() != (l.mode == MatchMode::Row) {
                return Err(Error::Invalid(format!(
                    "{id}: task {}: an object value goes with match row and only with it",
                    l.task
                )));
            }
            if l.mode == MatchMode::Row && crate::items::GoldRow::of(l).is_none() {
                return Err(Error::Invalid(format!(
                    "{id}: task {}: a row has unknown fields or wrong types",
                    l.task
                )));
            }
            if l.alternatives().is_empty() {
                return Err(Error::Invalid(format!(
                    "{id}: task {}: value must be a string, number, boolean, a list of them or a row",
                    l.task
                )));
            }
            if self.is_absent(&l.task) {
                return Err(Error::Invalid(format!(
                    "{id}: task {} is both labelled and absent",
                    l.task
                )));
            }
        }
        Ok(())
    }

    /// File name inside a dataset directory: `<dataset>/<first 16 hex of the id hash>.json`.
    pub fn file_name(&self) -> PathBuf {
        let id = self.id();
        let stem = if self.file.blake3.is_some() {
            id.chars().take(16).collect::<String>()
        } else {
            crate::blake3_hex(id.as_bytes()).chars().take(16).collect()
        };
        PathBuf::from(sanitize(&self.dataset)).join(format!("{stem}.json"))
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Reads every `*.json` file under `dir` (recursively, sorted by path) as a [`GoldDoc`] and
/// validates it. Any bad file is an error: gold must be clean. Two documents with the same id
/// are an error too.
pub fn load_dir(dir: &Path) -> Result<Vec<(PathBuf, GoldDoc)>> {
    let mut paths = Vec::new();
    collect_json(dir, &mut paths)?;
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    let mut seen = std::collections::HashMap::new();
    for p in paths {
        let text = crate::read_to_string(&p)?;
        let doc: GoldDoc = serde_json::from_str(&text).map_err(|e| Error::parse(&p, e))?;
        doc.validate()
            .map_err(|e| Error::parse(&p, e.to_string()))?;
        if let Some(prev) = seen.insert(doc.id(), p.clone()) {
            return Err(Error::Invalid(format!(
                "duplicate gold document {}: {} and {}",
                doc.id(),
                prev.display(),
                p.display()
            )));
        }
        out.push((p, doc));
    }
    Ok(out)
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let path = entry.path();
        let ft = entry.file_type().map_err(|e| Error::io(&path, e))?;
        if ft.is_dir() {
            collect_json(&path, out)?;
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    Ok(())
}

/// Writes `doc` as pretty JSON under `dir` at [`GoldDoc::file_name`]; returns the path.
pub fn write_doc(dir: &Path, doc: &GoldDoc) -> Result<PathBuf> {
    doc.validate()?;
    let path = dir.join(doc.file_name());
    let mut json = serde_json::to_string_pretty(doc)
        .map_err(|e| Error::Invalid(format!("serializing gold: {e}")))?;
    json.push('\n');
    crate::write_file(&path, json)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> GoldDoc {
        GoldDoc {
            file: GoldFile {
                blake3: Some("ab".repeat(32)),
                locator_hint: None,
            },
            dataset: "synthetic".into(),
            split: Split::Dev,
            source: None,
            labels: vec![Label::text("party", "Alpha Corp", MatchMode::Normalized)],
            absent: vec!["iban".into()],
        }
    }

    #[test]
    fn round_trips_and_validates() {
        let d = doc();
        let j = serde_json::to_string(&d).unwrap();
        assert!(j.contains(r#""match":"normalized""#));
        let back: GoldDoc = serde_json::from_str(&j).unwrap();
        assert_eq!(back, d);
        back.validate().unwrap();
    }

    #[test]
    fn parses_documented_example() {
        let j = r#"{ "file": { "blake3": "0000000000000000000000000000000000000000000000000000000000000000" },
          "dataset": "x", "split": "holdout",
          "labels": [ { "task": "language", "value": ["sr", "hr"], "match": "any_of" },
                      { "task": "amount_total", "value": 4200.5, "match": "numeric_tol", "tol": 0.01 } ],
          "absent": ["iban"] }"#;
        let d: GoldDoc = serde_json::from_str(j).unwrap();
        d.validate().unwrap();
        assert_eq!(d.labels[0].alternatives(), vec!["sr", "hr"]);
        assert_eq!(d.labels[1].alternatives(), vec!["4200.5"]);
    }

    #[test]
    fn rejects_bad_documents() {
        let mut d = doc();
        d.file.blake3 = Some("XYZ".into());
        assert!(d.validate().is_err());
        let mut d = doc();
        d.absent.push("party".into());
        assert!(d.validate().is_err());
        let mut d = doc();
        d.labels[0].value = serde_json::json!(["a", "b"]);
        assert!(d.validate().is_err());
        let mut d = doc();
        d.file = GoldFile::default();
        assert!(d.validate().is_err());
    }

    #[test]
    fn writes_and_loads_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let d = doc();
        let p = write_doc(dir.path(), &d).unwrap();
        assert!(p.ends_with("synthetic/abababababababab.json"));
        let loaded = load_dir(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, d);
        // A second copy under another name is a duplicate.
        std::fs::copy(&p, dir.path().join("dup.json")).unwrap();
        assert!(load_dir(dir.path()).is_err());
    }
}

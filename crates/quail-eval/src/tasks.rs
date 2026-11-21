//! The task map: which fact keys answer which gold task, and how their values are normalized.
//!
//! ```toml
//! [task.party]
//! keys = ["party.company"]
//! field = "surface"
//! normalize = "company"
//! multi = true
//! min_conf = 0.3
//! description = "Contracting parties"
//!
//! [task.term]
//! keys = ["clause.term"]
//! pending = "no producer yet"
//! ```
//!
//! The committed default is `tasks.toml` in this crate ([`TaskMap::builtin`]).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::normalize::Normalizer;
use crate::{Error, Result};

/// How one task is predicted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    /// Fact keys whose values are predictions for the task (all of them are pooled).
    pub keys: Vec<String>,
    /// Normalizer applied to gold and predicted values.
    #[serde(default)]
    pub normalize: Normalizer,
    /// Multi-valued (set semantics: every value is scored) or single-valued (only the most
    /// confident prediction counts).
    #[serde(default)]
    pub multi: bool,
    /// Predictions below this confidence are treated as abstentions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_conf: Option<f32>,
    /// For record values (`Value::Record`, also inside lists): the field whose value is the
    /// prediction (`surface` of `party.company`, `raw` of `ref.doc_number`). Records without
    /// the field give no prediction; non-record values are used as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// For record values: more fields whose values also answer the task (`defined_term` of
    /// `party.company`, when the gold labels a party's alias as well as its name). The record
    /// stays one prediction: it claims at most one gold label with `field` and one more with
    /// each alternative field, every claimed label is a TP, and it is one FP only when it
    /// claims none. Precision is then claimed labels over claimed labels plus unmatched
    /// predictions. Needs `field`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alt_fields: Vec<String>,
    /// For record values: keep only records whose field has one of the listed values
    /// (`only = { kind = ["registry"] }`). Records without the field are dropped.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub only: BTreeMap<String, Vec<String>>,
    /// For record values: drop records whose field has one of the listed values
    /// (`except = { kind = ["registry", "procurement"] }`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub except: BTreeMap<String, Vec<String>>,
    /// Predicted value → the gold vocabulary's value, compared after normalizing both
    /// (`aliases = { amendment = "annex" }` when a gold set calls Serbian *aneks* `annex`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
    /// When the gold marks this task absent *because* it equals another task's value (CUAD
    /// leaves `effective_date` out when it is the agreement date): a prediction equal to a
    /// gold label of the named task is neither a TP nor an FP on such documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absent_if_equals: Option<String>,
    /// Why the task has no producer yet (`"quail-pdf #7"`); the keys are the planned names.
    /// Pending tasks are still scored (as abstentions) and flagged in the report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
    /// Gold labels are rows (`match = "row"`, an object value) and predictions are the
    /// record values of `keys`, matched one to one by [`crate::items`] (price, page and
    /// description); `normalize`, `field`, `only`, `except` and `aliases` do not apply.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rows: bool,
    /// What the task is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TasksFile {
    #[serde(default)]
    task: BTreeMap<String, TaskSpec>,
}

/// All tasks, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskMap {
    /// Task name → spec.
    pub tasks: BTreeMap<String, TaskSpec>,
}

/// The committed default task map.
pub const BUILTIN_TOML: &str = include_str!("../tasks.toml");

impl TaskMap {
    /// Parses a TOML task map. `origin` names it in errors.
    pub fn from_toml(text: &str, origin: &Path) -> Result<Self> {
        let f: TasksFile = toml::from_str(text).map_err(|e| Error::parse(origin, e))?;
        for (name, spec) in &f.task {
            if spec.keys.is_empty() {
                return Err(Error::parse(origin, format!("task {name}: keys is empty")));
            }
            if spec.field.as_deref() == Some("") {
                return Err(Error::parse(origin, format!("task {name}: field is empty")));
            }
            if spec.absent_if_equals.as_deref() == Some(name.as_str()) {
                return Err(Error::parse(
                    origin,
                    format!("task {name}: absent_if_equals names the task itself"),
                ));
            }
            if !spec.alt_fields.is_empty() && spec.field.is_none() {
                return Err(Error::parse(
                    origin,
                    format!("task {name}: alt_fields need a field"),
                ));
            }
            if (!spec.only.is_empty() || !spec.except.is_empty()) && spec.field.is_none() {
                return Err(Error::parse(
                    origin,
                    format!("task {name}: only / except need a field"),
                ));
            }
            if let Some(c) = spec.min_conf
                && !(0.0..=1.0).contains(&c)
            {
                return Err(Error::parse(
                    origin,
                    format!("task {name}: min_conf outside [0, 1]"),
                ));
            }
        }
        Ok(Self { tasks: f.task })
    }

    /// Reads a TOML task map from a file.
    pub fn load(path: &Path) -> Result<Self> {
        Self::from_toml(&crate::read_to_string(path)?, path)
    }

    /// The committed `tasks.toml`.
    pub fn builtin() -> Self {
        // The file is tested below; a broken one fails the build's tests, not users.
        Self::from_toml(BUILTIN_TOML, Path::new("tasks.toml")).unwrap_or_default()
    }

    /// The spec of a task.
    pub fn get(&self, task: &str) -> Option<&TaskSpec> {
        self.tasks.get(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses() {
        let m = TaskMap::from_toml(BUILTIN_TOML, Path::new("tasks.toml")).unwrap();
        for t in [
            "doc_type",
            "party",
            "effective_date",
            "iban",
            "language",
            "is_scanned",
        ] {
            assert!(m.get(t).is_some(), "{t}");
        }
        assert!(m.get("party").unwrap().multi);
        assert_eq!(m.get("party").unwrap().normalize, Normalizer::Company);
        assert_eq!(m.get("party").unwrap().field.as_deref(), Some("surface"));
        assert_eq!(m.get("party").unwrap().alt_fields, ["defined_term"]);
        assert!(
            TaskMap::from_toml(
                "[task.a]\nkeys = [\"x.y\"]\nalt_fields = [\"t\"]\n",
                Path::new("t")
            )
            .is_err()
        );
        assert!(m.get("party").unwrap().pending.is_none());
        assert!(m.get("term").unwrap().pending.is_some());
        assert!(m.get("items").unwrap().rows);
        assert!(!m.get("party").unwrap().rows);
    }

    #[test]
    fn reads_field_and_pending() {
        let p = Path::new("t.toml");
        let m = TaskMap::from_toml(
            "[task.a]\nkeys = [\"x.y\"]\nfield = \"raw\"\npending = \"later\"\n",
            p,
        )
        .unwrap();
        let a = m.get("a").unwrap();
        assert_eq!(a.field.as_deref(), Some("raw"));
        assert_eq!(a.pending.as_deref(), Some("later"));
        assert!(TaskMap::from_toml("[task.a]\nkeys = [\"x.y\"]\nfield = \"\"\n", p).is_err());
    }

    #[test]
    fn reads_filters_aliases_and_absent_if_equals() {
        let p = Path::new("t.toml");
        let m = TaskMap::from_toml(
            "[task.a]\nkeys = [\"x.y\"]\nfield = \"raw\"\nonly = { kind = [\"registry\"] }\n\
             except = { kind = [\"procurement\"] }\naliases = { amendment = \"annex\" }\n\
             absent_if_equals = \"b\"\n",
            p,
        )
        .unwrap();
        let a = m.get("a").unwrap();
        assert_eq!(a.only["kind"], ["registry"]);
        assert_eq!(a.except["kind"], ["procurement"]);
        assert_eq!(a.aliases["amendment"], "annex");
        assert_eq!(a.absent_if_equals.as_deref(), Some("b"));
        assert!(
            TaskMap::from_toml("[task.a]\nkeys = [\"x.y\"]\nabsent_if_equals = \"a\"\n", p)
                .is_err()
        );
        assert!(
            TaskMap::from_toml("[task.a]\nkeys = [\"x.y\"]\nonly = { kind = [\"k\"] }\n", p)
                .is_err()
        );
    }

    #[test]
    fn rejects_bad_maps() {
        let p = Path::new("t.toml");
        assert!(TaskMap::from_toml("[task.a]\nkeys = []\n", p).is_err());
        assert!(
            TaskMap::from_toml("[task.a]\nkeys = [\"x.y\"]\nnormalize = \"nope\"\n", p).is_err()
        );
        assert!(TaskMap::from_toml("[task.a]\nkeys = [\"x.y\"]\nmin_conf = 2.0\n", p).is_err());
        assert!(TaskMap::from_toml("[task.a]\nkeys = [\"x.y\"]\ntypo = 1\n", p).is_err());
    }
}

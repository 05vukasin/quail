use serde::{Deserialize, Serialize};

use crate::analyzer::AnalyzerId;
use crate::fact::Fact;
use crate::format::Format;

/// How processing of one file ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Status {
    /// Every applicable step ran.
    Ok,
    /// A step failed, was skipped for a missing requirement, or hit a limit; the facts present
    /// are still valid. Warnings alone never make a record partial.
    Partial {
        /// One line per problem.
        problems: Vec<String>,
    },
    /// Nothing beyond the generic layer could be done.
    Failed {
        /// Why.
        error: String,
    },
}

/// Time spent in one step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    /// The step.
    pub src: AnalyzerId,
    /// Microseconds.
    pub micros: u64,
}

/// Everything quail knows about one file: the library's output and the contract with every
/// consumer. Versioned by [`SCHEMA_VERSION`](crate::SCHEMA_VERSION).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Schema version.
    pub schema: u32,
    /// Where the file came from.
    pub locator: String,
    /// Size in bytes.
    pub size: u64,
    /// Detected format.
    pub format: Format,
    /// Number of pages extracted, if it is a document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    /// All facts, in production order.
    pub facts: Vec<Fact>,
    /// Every step that ran, with its version.
    pub steps: Vec<AnalyzerId>,
    /// Time per step.
    pub timings: Vec<Timing>,
    /// Outcome.
    pub status: Status,
    /// Non-fatal notes from the steps, prefixed with the step id (`pdf.extract@1: ...`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl FileRecord {
    /// Facts with this key.
    pub fn facts_for<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Fact> + 'a {
        self.facts.iter().filter(move |f| f.key.as_str() == key)
    }
}

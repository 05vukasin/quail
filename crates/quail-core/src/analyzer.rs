use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::artifact::{Artifact, Artifacts};
use crate::blob::Blob;
use crate::error::{Error, Result};
use crate::fact::{Evidence, Fact, FactKey};
use crate::format::Format;
use crate::limits::{Limits, Profile};
use crate::value::Value;

/// Name and version of an analyzer or extractor, e.g. `simhash.text@1`. Bump the version
/// whenever its output can change for the same input: that is how re-indexing knows which
/// files to redo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct AnalyzerId {
    /// Dotted lowercase name; the part before the first dot is the crate's namespace.
    pub name: &'static str,
    /// Output version.
    pub version: u32,
}

impl AnalyzerId {
    /// New id.
    pub const fn new(name: &'static str, version: u32) -> Self {
        Self { name, version }
    }
}

impl<'de> Deserialize<'de> for AnalyzerId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            name: String,
            version: u32,
        }
        let raw = Raw::deserialize(d)?;
        // Records are read back only by eval and tests, and analyzer names are few, so
        // interning by leaking is bounded in practice.
        Ok(Self {
            name: Box::leak(raw.name.into_boxed_str()),
            version: raw.version,
        })
    }
}

impl fmt::Debug for AnalyzerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl fmt::Display for AnalyzerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// What an analyzer may look at: the file and everything produced before it. Read-only.
pub struct Ctx<'a> {
    /// The file's bytes.
    pub blob: &'a Blob,
    /// Its detected format.
    pub format: Format,
    /// Where it came from (path, `s3://` key, ...), for path-based signals.
    pub locator: &'a str,
    /// Artifacts produced so far.
    pub artifacts: &'a Artifacts,
    /// Facts produced so far, in production order.
    pub facts: &'a [Fact],
    /// Resource limits for this file.
    pub limits: &'a Limits,
    /// Active profile.
    pub profile: Profile,
}

impl Ctx<'_> {
    /// Shortcut for `self.artifacts.get::<T>()`.
    pub fn get<T: Artifact>(&self) -> Option<&T> {
        self.artifacts.get::<T>()
    }
}

/// Collects what one analyzer run produces. The engine merges it into the file's state after
/// the run, so analyzers never see each other's half-finished output.
pub struct Emitter {
    src: AnalyzerId,
    facts: Vec<Fact>,
    artifacts: Artifacts,
    warnings: Vec<String>,
    problems: Vec<String>,
    deadline: Option<Instant>,
}

impl Emitter {
    /// New emitter for the analyzer `src`, without a deadline.
    pub fn new(src: AnalyzerId) -> Self {
        Self {
            src,
            facts: Vec::new(),
            artifacts: Artifacts::new(),
            warnings: Vec::new(),
            problems: Vec::new(),
            deadline: None,
        }
    }

    /// New emitter whose work must finish by `deadline` (the file's time budget).
    pub fn with_deadline(src: AnalyzerId, deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
            ..Self::new(src)
        }
    }

    /// `Err(Error::Timeout)` once the file's time budget is spent. Long loops (per page, per
    /// chunk) call this so a hostile file cannot hold a worker: the engine cannot preempt.
    pub fn check_deadline(&self) -> Result<()> {
        match self.deadline {
            Some(d) if Instant::now() >= d => Err(Error::Timeout),
            _ => Ok(()),
        }
    }

    /// Emits a measured fact (confidence 1, no evidence).
    pub fn fact(&mut self, key: FactKey, value: Value) {
        self.push(key, value, 1.0, None);
    }

    /// Emits a fact with confidence and optional evidence.
    pub fn push(&mut self, key: FactKey, value: Value, conf: f32, evidence: Option<Evidence>) {
        debug_assert!((0.0..=1.0).contains(&conf), "conf out of range: {conf}");
        self.facts.push(Fact {
            key,
            value,
            conf: conf.clamp(0.0, 1.0),
            src: self.src,
            evidence,
        });
    }

    /// Stores an artifact for later analyzers.
    pub fn provide<T: Artifact>(&mut self, artifact: T) {
        self.artifacts.insert(artifact);
    }

    /// Records a non-fatal problem (e.g. "page 7 has a broken font, text skipped").
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Records that the step did not finish its job (stopped at a limit or the deadline, skipped
    /// a broken page...) while still returning a valid partial result. Unlike a warning this
    /// makes the file's record `Partial`.
    pub fn problem(&mut self, msg: impl Into<String>) {
        self.problems.push(msg.into());
    }

    /// Takes the problems recorded so far (used by the engine).
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// Who is emitting.
    pub fn src(&self) -> AnalyzerId {
        self.src
    }

    /// Facts emitted so far (for tests).
    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    /// Splits the emitter into its facts, artifacts and warnings (used by the engine).
    pub fn into_parts(self) -> (Vec<Fact>, Artifacts, Vec<String>) {
        (self.facts, self.artifacts, self.warnings)
    }
}

/// Produces facts and artifacts about one file. Stateless across files: everything it needs
/// beyond the file (a model, a dictionary) is loaded when it is constructed and shared
/// read-only between threads.
pub trait Analyzer: Send + Sync {
    /// Name and output version.
    fn id(&self) -> AnalyzerId;

    /// Artifact names that must exist before it runs. The engine orders analyzers by these
    /// and skips an analyzer whose requirements were never produced.
    fn requires(&self) -> &'static [&'static str] {
        &[]
    }

    /// Artifact names it may produce.
    fn provides(&self) -> &'static [&'static str] {
        &[]
    }

    /// Lowest profile it runs in. A `Deep` analyzer is skipped under `Standard`.
    fn profile(&self) -> Profile {
        Profile::Fast
    }

    /// Cheap check whether it applies to this file at all (format, a fact seen earlier).
    fn applies(&self, ctx: &Ctx<'_>) -> bool {
        let _ = ctx;
        true
    }

    /// Does the work. An error marks the file partial; it never stops other analyzers that do
    /// not depend on this one.
    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()>;
}

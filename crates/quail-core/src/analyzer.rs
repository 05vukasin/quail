use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::artifact::{Artifact, Artifacts};
use crate::blob::Blob;
use crate::document::{Document, RevisedDocument};
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

    /// The best available document: the [`RevisedDocument`] (e.g. with OCR text) when a step
    /// produced one, otherwise the extracted [`Document`].
    pub fn document(&self) -> Option<&Document> {
        self.get::<RevisedDocument>()
            .map(|r| &r.0)
            .or_else(|| self.get::<Document>())
    }
}

/// Called with the file's new deadline each time a step extends it
/// ([`Emitter::extend_deadline`]), while the step runs. A supervisor that enforces wall time
/// from outside (a watchdog) uses it to move its own limit along.
pub type DeadlineHook = Arc<dyn Fn(Instant) + Send + Sync>;

/// Collects what one analyzer run produces. The engine merges it into the file's state after
/// the run, so analyzers never see each other's half-finished output.
pub struct Emitter {
    src: AnalyzerId,
    facts: Vec<Fact>,
    artifacts: Artifacts,
    warnings: Vec<String>,
    problems: Vec<String>,
    deadline: Option<Instant>,
    /// Latest the deadline may be moved to; never before `deadline`.
    cap: Option<Instant>,
    on_extend: Option<DeadlineHook>,
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
            cap: None,
            on_extend: None,
        }
    }

    /// New emitter whose work must finish by `deadline` (the file's time budget). The
    /// deadline cannot be extended.
    pub fn with_deadline(src: AnalyzerId, deadline: Instant) -> Self {
        Self::with_extensible_deadline(src, deadline, deadline)
    }

    /// New emitter whose work must finish by `deadline`, which
    /// [`extend_deadline`](Self::extend_deadline) may move up to `cap` (the file's start plus
    /// [`Limits::timeout_cap`]). A `cap` before `deadline` counts as `deadline`.
    pub fn with_extensible_deadline(src: AnalyzerId, deadline: Instant, cap: Instant) -> Self {
        Self {
            deadline: Some(deadline),
            cap: Some(cap.max(deadline)),
            ..Self::new(src)
        }
    }

    /// Calls `hook` with the new deadline whenever [`extend_deadline`](Self::extend_deadline)
    /// moves it.
    #[must_use]
    pub fn on_extend(mut self, hook: DeadlineHook) -> Self {
        self.on_extend = Some(hook);
        self
    }

    /// The file's deadline, when the engine set one. Steps that hand work to other components
    /// (a renderer, an OCR engine) pass it on instead of starting a fresh time budget.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// The latest the deadline can be extended to (the deadline itself when it cannot be).
    pub fn deadline_cap(&self) -> Option<Instant> {
        self.cap
    }

    /// Moves the file's deadline `by` later, never past [`deadline_cap`](Self::deadline_cap),
    /// and returns how much was granted (zero without a deadline, at the cap, or for a zero
    /// `by`). The new deadline holds for this step and every step after it in the same file.
    ///
    /// For a step whose work is known only once it looked at the file, e.g. OCR asking for
    /// its pages' time: a file that needs none keeps the normal timeout.
    pub fn extend_deadline(&mut self, by: Duration) -> Duration {
        let (Some(d), Some(cap)) = (self.deadline, self.cap) else {
            return Duration::ZERO;
        };
        let new = d.checked_add(by).map_or(cap, |n| n.min(cap));
        if new <= d {
            return Duration::ZERO;
        }
        self.deadline = Some(new);
        if let Some(hook) = &self.on_extend {
            hook(new);
        }
        new - d
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

    /// Soft ordering: if an active step provides one of these artifacts, run after it; if none
    /// does (or it produced nothing for this file), run anyway. Text consumers use
    /// `&["revised_document"]` so they see OCR text when OCR is active.
    fn after(&self) -> &'static [&'static str] {
        &[]
    }

    /// A finalizer runs after every non-finalizer step (e.g. a summary built from all facts),
    /// whatever the registration order. Finalizers are ordered among themselves by their
    /// requirements and registration order.
    fn finalizer(&self) -> bool {
        false
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const ID: AnalyzerId = AnalyzerId::new("test.step", 1);

    #[test]
    fn extension_is_granted_up_to_the_cap() {
        let now = Instant::now();
        let base = now + Duration::from_secs(10);
        let cap = now + Duration::from_secs(25);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hook: DeadlineHook = {
            let seen = Arc::clone(&seen);
            Arc::new(move |d| seen.lock().unwrap().push(d))
        };
        let mut out = Emitter::with_extensible_deadline(ID, base, cap).on_extend(hook);
        assert_eq!(out.deadline_cap(), Some(cap));
        assert_eq!(
            out.extend_deadline(Duration::from_secs(5)),
            Duration::from_secs(5)
        );
        assert_eq!(out.deadline(), Some(base + Duration::from_secs(5)));
        // Capped: only the 10 s left are granted, then nothing.
        assert_eq!(
            out.extend_deadline(Duration::from_mins(1)),
            Duration::from_secs(10)
        );
        assert_eq!(out.deadline(), Some(cap));
        assert_eq!(out.extend_deadline(Duration::from_secs(1)), Duration::ZERO);
        assert_eq!(out.extend_deadline(Duration::MAX), Duration::ZERO);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![base + Duration::from_secs(5), cap]
        );
    }

    #[test]
    fn plain_deadlines_do_not_move() {
        let d = Instant::now() + Duration::from_secs(10);
        let mut out = Emitter::with_deadline(ID, d);
        assert_eq!(out.extend_deadline(Duration::from_secs(5)), Duration::ZERO);
        assert_eq!(out.deadline(), Some(d));
        let mut out = Emitter::new(ID);
        assert_eq!(out.extend_deadline(Duration::from_secs(5)), Duration::ZERO);
        assert_eq!(out.deadline(), None);
        // A cap before the deadline is the deadline.
        let mut out = Emitter::with_extensible_deadline(
            ID,
            d,
            d.checked_sub(Duration::from_secs(1)).unwrap(),
        );
        assert_eq!(out.extend_deadline(Duration::from_secs(5)), Duration::ZERO);
        assert_eq!(out.deadline(), Some(d));
    }

    #[test]
    fn timeout_cap_is_the_larger_timeout() {
        let l = Limits::default();
        assert_eq!(l.max_timeout, Duration::ZERO);
        assert_eq!(l.timeout_cap(), l.timeout);
        let l = Limits {
            max_timeout: Duration::from_mins(30),
            ..Limits::default()
        };
        assert_eq!(l.timeout_cap(), Duration::from_mins(30));
        // Limits serialized before `max_timeout` existed still parse, without extension.
        let mut v = serde_json::to_value(Limits::default()).unwrap();
        v.as_object_mut().unwrap().remove("max_timeout");
        let back: Limits = serde_json::from_value(v).unwrap();
        assert_eq!(back, Limits::default());
    }
}

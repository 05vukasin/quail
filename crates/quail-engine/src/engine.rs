//! [`Engine::process`]: one file through sniffing, extraction and the analyzer plan.

use std::time::Instant;

use quail_core::{
    Analyzer, AnalyzerId, Artifacts, Blob, Ctx, Emitter, Error, Extractor, Fact, FileRecord,
    Format, Limits, Profile, SCHEMA_VERSION, SourceItem, Status, Timing, Value,
};

use crate::builder::ExtractorTable;
use crate::keys;
use crate::panic::contain;
use crate::sniff::detect;

/// Id of the sniffing step, the source of `engine.format` and `engine.mime`.
pub const SNIFF_ID: AnalyzerId = AnalyzerId::new("engine.sniff", 1);

/// Runs extractors and analyzers over files. Built by
/// [`EngineBuilder`](crate::EngineBuilder); immutable and shared between worker threads.
///
/// Deadlines are cooperative: the engine hands every step the file's deadline through
/// [`Emitter::with_deadline`] and stops starting new steps once it has passed, but it cannot
/// preempt a step that never calls [`Emitter::check_deadline`]. A native library that hangs
/// inside one call holds its worker thread until it returns; only a separate process can be
/// killed, so process isolation (a watchdog that restarts workers) is the service's job.
pub struct Engine {
    profile: Profile,
    limits: Limits,
    quiet: bool,
    extractors: ExtractorTable,
    plan: Vec<Box<dyn Analyzer>>,
    inactive: Vec<AnalyzerId>,
}

/// Mutable state of one file while it moves through the plan.
struct FileState {
    facts: Vec<Fact>,
    artifacts: Artifacts,
    steps: Vec<AnalyzerId>,
    timings: Vec<Timing>,
    problems: Vec<String>,
    fatal: Option<String>,
}

impl FileState {
    fn step(&mut self, id: AnalyzerId, started: Instant) {
        self.steps.push(id);
        self.timings.push(Timing {
            src: id,
            micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        });
    }

    fn warnings(&mut self, id: AnalyzerId, warnings: Vec<String>) {
        self.problems
            .extend(warnings.into_iter().map(|w| format!("{id}: {w}")));
    }
}

impl Engine {
    pub(crate) fn from_parts(
        profile: Profile,
        limits: Limits,
        quiet: bool,
        extractors: ExtractorTable,
        plan: Vec<Box<dyn Analyzer>>,
        inactive: Vec<AnalyzerId>,
    ) -> Self {
        Self {
            profile,
            limits,
            quiet,
            extractors,
            plan,
            inactive,
        }
    }

    /// The active profile.
    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub(crate) fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// The per-file limits.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Analyzer ids in execution order (only those active under the profile).
    pub fn plan(&self) -> impl Iterator<Item = AnalyzerId> + '_ {
        self.plan.iter().map(|a| a.id())
    }

    /// Analyzers registered but left out because their profile is above the engine's.
    pub fn inactive(&self) -> &[AnalyzerId] {
        &self.inactive
    }

    /// The extractor chosen for `format`, if any.
    pub fn extractor_for(&self, format: Format) -> Option<AnalyzerId> {
        self.extractor(format).map(Extractor::id)
    }

    fn extractor(&self, format: Format) -> Option<&dyn Extractor> {
        self.extractors
            .iter()
            .find(|(f, _)| *f == format)
            .map(|(_, e)| &**e)
    }

    /// Processes one file. Never panics on file content and never returns an error: every
    /// problem ends up in [`FileRecord::status`].
    ///
    /// Steps: sniff (`engine.format`, `engine.mime`), size check against
    /// [`Limits::max_bytes`] (too big: no extraction, the record is partial, analyzers that
    /// do not require `document` still run), the format's extractor (its document becomes
    /// the `document` artifact), then every analyzer of the plan whose requirements exist
    /// and that [`applies`](Analyzer::applies). Extractor and analyzers run under
    /// `catch_unwind`; a step that errs or panics loses its facts and artifacts, keeps its
    /// warnings, and marks the file partial. A failed extractor marks it failed.
    pub fn process(&self, item: SourceItem) -> FileRecord {
        let SourceItem { locator, blob, .. } = item;
        self.process_blob(locator, &blob)
    }

    /// [`process`](Self::process) for a blob held elsewhere.
    pub fn process_blob(&self, locator: String, blob: &Blob) -> FileRecord {
        let start = Instant::now();
        let deadline = start.checked_add(self.limits.timeout);
        let size = blob.len() as u64;
        let cap = self.plan.len() + 2;
        let mut st = FileState {
            facts: Vec::with_capacity(cap * 2),
            artifacts: Artifacts::new(),
            steps: Vec::with_capacity(cap),
            timings: Vec::with_capacity(cap),
            problems: Vec::new(),
            fatal: None,
        };
        let emitter = |id| match deadline {
            Some(d) => Emitter::with_deadline(id, d),
            None => Emitter::new(id),
        };
        let expired = || deadline.is_some_and(|d| Instant::now() >= d);

        let sniff = detect(blob.as_bytes());
        for (key, value) in [
            (keys::FORMAT, Value::text(sniff.format.as_str())),
            (keys::MIME, Value::text(sniff.mime)),
        ] {
            st.facts.push(Fact {
                key,
                value,
                conf: 1.0,
                src: SNIFF_ID,
                evidence: None,
            });
        }
        st.step(SNIFF_ID, start);

        let mut pages = None;
        if size > self.limits.max_bytes {
            st.problems.push(format!(
                "engine: file is {size} bytes, over max_bytes {}; extraction skipped",
                self.limits.max_bytes
            ));
        } else if let Some(ext) = self.extractor(sniff.format) {
            let t = Instant::now();
            let id = ext.id();
            let mut out = emitter(id);
            let r =
                contain(self.quiet, || ext.extract(blob, &self.limits, &mut out)).and_then(|r| r);
            st.step(id, t);
            let (facts, artifacts, warnings) = out.into_parts();
            st.warnings(id, warnings);
            match r {
                Ok(doc) => {
                    pages = Some(u32::try_from(doc.pages.len()).unwrap_or(u32::MAX));
                    st.facts.extend(facts);
                    st.artifacts.absorb(artifacts);
                    st.artifacts.insert(doc);
                }
                Err(e) => st.fatal = Some(format!("{id}: {e}")),
            }
        }

        for (i, a) in self.plan.iter().enumerate() {
            if expired() {
                st.problems.push(format!(
                    "engine: {}; {} analyzer(s) not run",
                    Error::Timeout,
                    self.plan.len() - i
                ));
                break;
            }
            self.run_analyzer(&mut st, &**a, blob, sniff.format, &locator, &emitter);
        }

        let status = match st.fatal {
            Some(mut error) => {
                for p in &st.problems {
                    error.push_str("; ");
                    error.push_str(p);
                }
                Status::Failed { error }
            }
            None if st.problems.is_empty() => Status::Ok,
            None => Status::Partial {
                problems: st.problems,
            },
        };
        FileRecord {
            schema: SCHEMA_VERSION,
            locator,
            size,
            format: sniff.format,
            pages,
            facts: st.facts,
            steps: st.steps,
            timings: st.timings,
            status,
        }
    }

    fn run_analyzer(
        &self,
        st: &mut FileState,
        a: &dyn Analyzer,
        blob: &Blob,
        format: Format,
        locator: &str,
        emitter: &dyn Fn(AnalyzerId) -> Emitter,
    ) {
        let id = a.id();
        let t = Instant::now();
        let mut out = emitter(id);
        let missing = a
            .requires()
            .iter()
            .copied()
            .find(|r| !st.artifacts.has_name(r));
        let ran = {
            let ctx = Ctx {
                blob,
                format,
                locator,
                artifacts: &st.artifacts,
                facts: &st.facts,
                limits: &self.limits,
                profile: self.profile,
            };
            contain(self.quiet, || {
                if !a.applies(&ctx) {
                    None
                } else if missing.is_some() {
                    Some(Ok(false))
                } else {
                    Some(a.run(&ctx, &mut out).map(|()| true))
                }
            })
        };
        match ran {
            Ok(None) => {}
            Ok(Some(Ok(false))) => st.problems.push(format!(
                "{id}: skipped, missing artifact `{}`",
                missing.unwrap_or_default()
            )),
            Ok(Some(Ok(true))) => {
                st.step(id, t);
                let (facts, artifacts, warnings) = out.into_parts();
                st.facts.extend(facts);
                st.artifacts.absorb(artifacts);
                st.warnings(id, warnings);
            }
            Ok(Some(Err(e))) | Err(e) => {
                st.step(id, t);
                let (_, _, warnings) = out.into_parts();
                st.warnings(id, warnings);
                st.problems.push(format!("{id}: {e}"));
            }
        }
    }
}

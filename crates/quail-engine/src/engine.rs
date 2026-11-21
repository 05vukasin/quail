//! [`Engine::process`]: one file through sniffing, extraction and the analyzer plan.

use std::time::Instant;

use quail_core::{
    Analyzer, AnalyzerId, Artifacts, Blob, Ctx, Emitter, Error, Extractor, Fact, FileRecord,
    Format, Limits, Profile, SCHEMA_VERSION, SourceItem, Status, Timing, Value,
};

use crate::builder::ExtractorTable;
use crate::keys;
use crate::panic::contain;
use crate::sniff::{Sniff, detect};

/// Id of the sniffing step, the source of `engine.format` and `engine.mime`.
pub const SNIFF_ID: AnalyzerId = AnalyzerId::new("engine.sniff", 1);

/// Runs extractors and analyzers over files. Built by
/// [`EngineBuilder`](crate::EngineBuilder); immutable and shared between worker threads.
///
/// # Status of a record
///
/// | what happened                                                        | status    |
/// |----------------------------------------------------------------------|-----------|
/// | every step that applied succeeded (warnings or not)                  | `Ok`      |
/// | file over `max_bytes` (no extraction, generic analyzers still run)   | `Partial` |
/// | extractor returned `Encrypted` or `LimitExceeded` (its facts kept)   | `Partial` |
/// | extractor returned any other error, panicked or timed out (facts kept) | `Failed` |
/// | an analyzer errored, panicked or timed out (its facts dropped)       | `Partial` |
/// | an applicable analyzer skipped because a provider of its input failed | `Partial` |
/// | the deadline passed before all analyzers ran                         | `Partial` |
///
/// Warnings never change the status; they go to [`FileRecord::warnings`] prefixed with the
/// step id. An analyzer skipped because its input was never produced (its provider did not
/// apply, e.g. no extractor for the format) is silently skipped. `Failed` wins over
/// `Partial`: its message lists the other problems after the extractor's.
///
/// # Deadlines are cooperative
///
/// The engine hands every step the file's deadline through [`Emitter::with_deadline`] and
/// stops starting new steps once it has passed, but it cannot preempt a step that never calls
/// [`Emitter::check_deadline`]. A native library that hangs inside one call holds its worker
/// thread until it returns; only a separate process can be killed, so process isolation (a
/// watchdog that restarts workers) is the service's job.
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
    warnings: Vec<String>,
    /// Artifact names whose producer applied but failed (or was itself skipped for that
    /// reason): a consumer missing one of these is a problem, not a silent skip.
    failed: Vec<&'static str>,
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
        self.warnings
            .extend(warnings.into_iter().map(|w| format!("{id}: {w}")));
    }

    fn fail_provides(&mut self, names: &[&'static str]) {
        for n in names {
            if !self.failed.contains(n) {
                self.failed.push(n);
            }
        }
    }
}

/// A `Failed` record without any step, for a file the engine could not even start on.
pub(crate) fn failed_record(locator: String, size: u64, error: String) -> FileRecord {
    FileRecord {
        schema: SCHEMA_VERSION,
        locator,
        size,
        format: Format::Unknown,
        pages: None,
        facts: Vec::new(),
        steps: Vec::new(),
        timings: Vec::new(),
        status: Status::Failed { error },
        warnings: Vec::new(),
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
    /// problem ends up in [`FileRecord::status`] (see the table on [`Engine`]).
    ///
    /// Steps: sniff (`engine.format`, `engine.mime`), size check against
    /// [`Limits::max_bytes`], the format's extractor (its document becomes the `document`
    /// artifact), then every analyzer of the plan that [`applies`](Analyzer::applies) and
    /// whose required artifacts exist. Every step runs under `catch_unwind`.
    pub fn process(&self, item: SourceItem) -> FileRecord {
        let SourceItem { locator, blob, .. } = item;
        self.process_blob(locator, &blob)
    }

    /// [`process`](Self::process) for a blob held elsewhere.
    pub fn process_blob(&self, locator: String, blob: &Blob) -> FileRecord {
        self.process_parts(locator, blob).0
    }

    /// [`process`](Self::process), then `f` with the record and every artifact the steps
    /// produced (the extracted [`Document`](quail_core::Document) under `document`, a page
    /// raster, ...) before they are dropped. Use it to keep what a record does not carry, e.g.
    /// to dump documents, without registering an analyzer (which would show up in `steps`).
    ///
    /// `f` runs on the calling thread after the record is complete and cannot change it. It is
    /// the caller's own code, so a panic in it is not contained here; in [`Engine::run`] the
    /// same hook ([`RunOptions::on_artifacts`](crate::RunOptions::on_artifacts)) is.
    pub fn process_with(
        &self,
        item: SourceItem,
        f: impl FnOnce(&FileRecord, &Artifacts),
    ) -> FileRecord {
        let SourceItem { locator, blob, .. } = item;
        let (record, artifacts) = self.process_parts(locator, &blob);
        f(&record, &artifacts);
        record
    }

    /// The record of one file and the artifacts its steps produced.
    pub(crate) fn process_parts(&self, locator: String, blob: &Blob) -> (FileRecord, Artifacts) {
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
            warnings: Vec::new(),
            failed: Vec::new(),
            fatal: None,
        };
        let emitter = |id| match deadline {
            Some(d) => Emitter::with_deadline(id, d),
            None => Emitter::new(id),
        };
        let expired = || deadline.is_some_and(|d| Instant::now() >= d);

        let sniff = contain(self.quiet, || detect(blob.as_bytes())).unwrap_or_else(|e| {
            st.problems.push(format!("{SNIFF_ID}: {e}"));
            Sniff {
                format: Format::Unknown,
                mime: "application/octet-stream",
            }
        });
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
            st.fail_provides(&["document"]);
        } else if let Some(ext) = self.extractor(sniff.format) {
            pages = self.extract(&mut st, ext, blob, emitter(ext.id()));
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
        let record = FileRecord {
            schema: SCHEMA_VERSION,
            locator,
            size,
            format: sniff.format,
            pages,
            facts: st.facts,
            steps: st.steps,
            timings: st.timings,
            status,
            warnings: st.warnings,
        };
        (record, st.artifacts)
    }

    /// Runs the extractor; returns the page count when it produced a document.
    fn extract(
        &self,
        st: &mut FileState,
        ext: &dyn Extractor,
        blob: &Blob,
        mut out: Emitter,
    ) -> Option<u32> {
        let t = Instant::now();
        let id = ext.id();
        let r = contain(self.quiet, || ext.extract(blob, &self.limits, &mut out)).and_then(|r| r);
        st.step(id, t);
        // The emitter survives an error: what the extractor said before failing is kept.
        st.problems.extend(
            out.take_problems()
                .into_iter()
                .map(|p| format!("{id}: {p}")),
        );
        let (facts, artifacts, warnings) = out.into_parts();
        st.facts.extend(facts);
        st.artifacts.absorb(artifacts);
        st.warnings(id, warnings);
        match r {
            Ok(doc) => {
                let pages = u32::try_from(doc.pages.len()).unwrap_or(u32::MAX);
                st.artifacts.insert(doc);
                Some(pages)
            }
            Err(e @ (Error::Encrypted | Error::LimitExceeded(_))) => {
                st.problems.push(format!("{id}: {e}"));
                st.fail_provides(&["document"]);
                st.fail_provides(ext.provides());
                None
            }
            Err(e) => {
                st.fatal = Some(format!("{id}: {e}"));
                st.fail_provides(&["document"]);
                st.fail_provides(ext.provides());
                None
            }
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
                if missing.is_some() || !a.applies(&ctx) {
                    // Not applicable, or input never produced: `applies` only matters when
                    // the input failed, so it is asked below in that case alone.
                    None
                } else {
                    Some(a.run(&ctx, &mut out))
                }
            })
        };
        match ran {
            Ok(None) => {
                let Some(m) = missing else { return };
                if !st.failed.contains(&m) {
                    return; // provider did not apply: a silent skip
                }
                let applies = {
                    let ctx = Ctx {
                        blob,
                        format,
                        locator,
                        artifacts: &st.artifacts,
                        facts: &st.facts,
                        limits: &self.limits,
                        profile: self.profile,
                    };
                    contain(self.quiet, || a.applies(&ctx)).unwrap_or(false)
                };
                if applies {
                    st.problems
                        .push(format!("{id}: skipped, input `{m}` unavailable: an earlier step failed or hit a limit"));
                    st.fail_provides(a.provides());
                }
            }
            Ok(Some(Ok(()))) => {
                st.step(id, t);
                st.problems.extend(
                    out.take_problems()
                        .into_iter()
                        .map(|p| format!("{id}: {p}")),
                );
                let (facts, artifacts, warnings) = out.into_parts();
                st.facts.extend(facts);
                st.artifacts.absorb(artifacts);
                st.warnings(id, warnings);
            }
            Ok(Some(Err(e))) | Err(e) => {
                st.step(id, t);
                st.problems.extend(
                    out.take_problems()
                        .into_iter()
                        .map(|p| format!("{id}: {p}")),
                );
                let (_, _, warnings) = out.into_parts();
                st.warnings(id, warnings);
                st.problems.push(format!("{id}: {e}"));
                st.fail_provides(a.provides());
            }
        }
    }
}

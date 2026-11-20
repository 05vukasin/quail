//! [`EngineBuilder`]: registers extractors and analyzers and validates the plan once.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

use quail_core::{Analyzer, AnalyzerId, Extractor, Format, Limits, Profile};

use crate::engine::Engine;
use crate::panic::install_quiet_panic_hook;

/// Artifact names the engine itself provides, usable in [`Analyzer::requires`] without any
/// analyzer providing them.
pub const BUILTIN_ARTIFACTS: &[&str] = &["document"];

/// Why [`EngineBuilder::build`] refused a configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// Two extractors claim the same format with the same priority.
    #[error(
        "extractors {first} and {second} both handle format `{}` with priority {priority}; \
         register one of them with a different priority",
        format.as_str()
    )]
    AmbiguousExtractor {
        /// The contested format.
        format: Format,
        /// The extractor registered first.
        first: AnalyzerId,
        /// The extractor registered second.
        second: AnalyzerId,
        /// Their common priority.
        priority: i32,
    },
    /// The same analyzer id is registered twice.
    #[error("analyzer {0} is registered twice")]
    DuplicateAnalyzer(AnalyzerId),
    /// An analyzer requires an artifact nobody active provides.
    #[error(
        "analyzer {analyzer} requires artifact `{artifact}`, which no active analyzer provides{}",
        match inactive_provider {
            Some(p) => format!(" ({p} provides it but is above profile {profile:?})"),
            None => String::new(),
        }
    )]
    Unsatisfied {
        /// The analyzer whose requirement is missing.
        analyzer: AnalyzerId,
        /// The missing artifact name.
        artifact: &'static str,
        /// An analyzer that provides it but is filtered out by the profile.
        inactive_provider: Option<AnalyzerId>,
        /// The engine's profile.
        profile: Profile,
    },
    /// Requirements form a cycle; the ids are listed in dependency order, the first
    /// repeated at the end.
    #[error("analyzer dependency cycle: {}", fmt_cycle(.0))]
    Cycle(Vec<AnalyzerId>),
}

fn fmt_cycle(c: &[AnalyzerId]) -> String {
    c.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Configures an [`Engine`]. Validation and the analyzer order are computed once, in
/// [`build`](Self::build), never per file.
#[must_use]
pub struct EngineBuilder {
    profile: Profile,
    limits: Limits,
    quiet_panics: bool,
    extractors: Vec<(Box<dyn Extractor>, i32)>,
    analyzers: Vec<Box<dyn Analyzer>>,
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineBuilder {
    /// Default profile and limits, no extractors or analyzers, panics printed as usual.
    pub fn new() -> Self {
        Self {
            profile: Profile::default(),
            limits: Limits::default(),
            quiet_panics: false,
            extractors: Vec::new(),
            analyzers: Vec::new(),
        }
    }

    /// Analyzers whose [`Analyzer::profile`] is above this one are left out.
    pub fn profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }

    /// Per-file limits.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// When true, [`build`](Self::build) installs a process-wide panic hook that keeps panics
    /// caught inside extractors and analyzers off stderr (see
    /// [`install_quiet_panic_hook`](crate::install_quiet_panic_hook)). Panics elsewhere
    /// still print. The panic is recorded in the file's status either way.
    pub fn quiet_panics(mut self, quiet: bool) -> Self {
        self.quiet_panics = quiet;
        self
    }

    /// Registers an extractor with priority 0.
    pub fn extractor(self, extractor: Box<dyn Extractor>) -> Self {
        self.extractor_with_priority(extractor, 0)
    }

    /// Registers an extractor. For each format the one with the highest priority wins; equal
    /// priorities for one format are a [`BuildError::AmbiguousExtractor`].
    pub fn extractor_with_priority(mut self, extractor: Box<dyn Extractor>, priority: i32) -> Self {
        self.extractors.push((extractor, priority));
        self
    }

    /// Registers an analyzer.
    pub fn analyzer(mut self, analyzer: Box<dyn Analyzer>) -> Self {
        self.analyzers.push(analyzer);
        self
    }

    /// Validates the configuration and fixes the analyzer order.
    pub fn build(self) -> Result<Engine, BuildError> {
        let extractors = pick_extractors(self.extractors)?;

        let mut seen = HashSet::new();
        for a in &self.analyzers {
            if !seen.insert(a.id()) {
                return Err(BuildError::DuplicateAnalyzer(a.id()));
            }
        }
        let (active, inactive): (Vec<_>, Vec<_>) = self
            .analyzers
            .into_iter()
            .partition(|a| a.profile() <= self.profile);
        let plan = order(active, &inactive, self.profile)?;

        if self.quiet_panics {
            install_quiet_panic_hook();
        }
        Ok(Engine::from_parts(
            self.profile,
            self.limits,
            self.quiet_panics,
            extractors,
            plan,
            inactive.iter().map(|a| a.id()).collect(),
        ))
    }
}

/// The extractor chosen for each format.
pub(crate) type ExtractorTable = Vec<(Format, Arc<dyn Extractor>)>;

/// Per format, the highest-priority extractor; a tie at the top is an error.
fn pick_extractors(all: Vec<(Box<dyn Extractor>, i32)>) -> Result<ExtractorTable, BuildError> {
    let all: Vec<(Arc<dyn Extractor>, i32)> = all.into_iter().map(|(e, p)| (e.into(), p)).collect();
    let mut formats: Vec<Format> = Vec::new();
    for (e, _) in &all {
        for &f in e.formats() {
            if !formats.contains(&f) {
                formats.push(f);
            }
        }
    }
    let mut table = Vec::with_capacity(formats.len());
    for f in formats {
        let mut best: Option<usize> = None;
        let mut tie: Option<usize> = None;
        for (i, (e, p)) in all.iter().enumerate() {
            if !e.formats().contains(&f) {
                continue;
            }
            match best {
                Some(b) if all[b].1 > *p => {}
                Some(b) if all[b].1 == *p => tie = tie.or(Some(i)),
                _ => {
                    best = Some(i);
                    tie = None;
                }
            }
        }
        let Some(b) = best else { continue };
        if let Some(t) = tie {
            return Err(BuildError::AmbiguousExtractor {
                format: f,
                first: all[b].0.id(),
                second: all[t].0.id(),
                priority: all[b].1,
            });
        }
        table.push((f, Arc::clone(&all[b].0)));
    }
    Ok(table)
}

/// Topological order of `active` by requires/provides. Ties keep registration order, so the
/// plan is deterministic.
fn order(
    active: Vec<Box<dyn Analyzer>>,
    inactive: &[Box<dyn Analyzer>],
    profile: Profile,
) -> Result<Vec<Box<dyn Analyzer>>, BuildError> {
    let n = active.len();
    let mut providers: HashMap<&'static str, Vec<usize>> = HashMap::new();
    for (i, a) in active.iter().enumerate() {
        for &p in a.provides() {
            providers.entry(p).or_default().push(i);
        }
    }
    // deps[i]: analyzers that must run before i.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, a) in active.iter().enumerate() {
        for &r in a.requires() {
            match providers.get(r) {
                Some(ps) => deps[i].extend(ps.iter().copied()),
                None if BUILTIN_ARTIFACTS.contains(&r) => {}
                None => {
                    return Err(BuildError::Unsatisfied {
                        analyzer: a.id(),
                        artifact: r,
                        inactive_provider: inactive
                            .iter()
                            .find(|x| x.provides().contains(&r))
                            .map(|x| x.id()),
                        profile,
                    });
                }
            }
        }
        deps[i].sort_unstable();
        deps[i].dedup();
    }

    let mut indegree: Vec<usize> = deps.iter().map(Vec::len).collect();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, ds) in deps.iter().enumerate() {
        for &d in ds {
            dependents[d].push(i);
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> =
        (0..n).filter(|&i| indegree[i] == 0).map(Reverse).collect();
    let mut sorted = Vec::with_capacity(n);
    while let Some(Reverse(i)) = ready.pop() {
        sorted.push(i);
        for &j in &dependents[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.push(Reverse(j));
            }
        }
    }
    if sorted.len() < n {
        return Err(BuildError::Cycle(find_cycle(&deps, &indegree, &active)));
    }

    let mut slots: Vec<Option<Box<dyn Analyzer>>> = active.into_iter().map(Some).collect();
    Ok(sorted.into_iter().filter_map(|i| slots[i].take()).collect())
}

/// Walks unresolved dependencies from an unsorted node until one repeats.
fn find_cycle(
    deps: &[Vec<usize>],
    indegree: &[usize],
    active: &[Box<dyn Analyzer>],
) -> Vec<AnalyzerId> {
    let Some(start) = (0..deps.len()).find(|&i| indegree[i] > 0) else {
        return Vec::new();
    };
    let mut path = vec![start];
    let mut cur = start;
    // Every unsorted node has at least one unsorted dependency.
    while let Some(&next) = deps[cur].iter().find(|&&d| indegree[d] > 0) {
        if let Some(pos) = path.iter().position(|&p| p == next) {
            let mut cycle: Vec<usize> = path[pos..].to_vec();
            // Dependency order: providers first.
            cycle.reverse();
            cycle.push(cycle[0]);
            return cycle.into_iter().map(|i| active[i].id()).collect();
        }
        path.push(next);
        cur = next;
    }
    path.into_iter().map(|i| active[i].id()).collect()
}

//! The one place that knows which extractors and analyzers exist.
//!
//! Every concrete step is wired here, behind the cargo feature of its crate. To add a crate:
//! add an optional dependency and a feature in `Cargo.toml`, register its steps in
//! [`builder`], describe them in [`STEPS`] and its keys in [`key_tables`] (regenerate
//! `keydocs.rs` with `tools/genkeys.py`).

use std::time::Duration;

use quail_core::{AnalyzerId, Format, Limits, Profile};
use quail_engine::{EngineBuilder, SNIFF_ID};
use serde::{Deserialize, Serialize};

use crate::keydocs::{self, KeyDoc};

/// What kind of step an id belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// Built into the engine (sniffing).
    Engine,
    /// Turns one format into a `Document`.
    Extractor,
    /// Adds facts and artifacts.
    Analyzer,
}

impl StepKind {
    /// Lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Extractor => "extractor",
            Self::Analyzer => "analyzer",
        }
    }
}

/// Documentation of one registered step.
#[derive(Clone, Copy, Debug)]
pub struct StepDoc {
    /// Name and version.
    pub id: AnalyzerId,
    /// Extractor, analyzer or engine.
    pub kind: StepKind,
    /// Formats it handles (extractors) or applies to (empty: any).
    pub formats: &'static [Format],
    /// One line.
    pub doc: &'static str,
}

/// Every step this build can run, in registration order (sniffing first).
pub const STEPS: &[StepDoc] = &[
    StepDoc {
        id: SNIFF_ID,
        kind: StepKind::Engine,
        formats: &[],
        doc: "Format and MIME type from the bytes (magic numbers, ZIP directory).",
    },
    StepDoc {
        id: quail_engine::DEDUP_ID,
        kind: StepKind::Engine,
        formats: &[],
        doc: "`scan --dedup` only: a file whose bytes were already analyzed in the run reuses that record (`engine.duplicate_of`).",
    },
    StepDoc {
        id: quail_hash::IdentityAnalyzer::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "BLAKE3 and XXH3-128 of the whole file, FastCDC chunk hashes.",
    },
    #[cfg(feature = "docx")]
    StepDoc {
        id: quail_docx::ID,
        kind: StepKind::Extractor,
        formats: &[Format::Docx],
        doc: "Streaming OOXML reader: sections as logical pages, zones, properties, rsids.",
    },
    #[cfg(feature = "pdf")]
    StepDoc {
        id: quail_pdf::PdfExtractor::ID,
        kind: StepKind::Extractor,
        formats: &[Format::Pdf],
        doc: "Pure-Rust PDF reader: positioned text in reading order, zones, images, OCR flags, raster.",
    },
    #[cfg(feature = "text")]
    StepDoc {
        id: quail_text::TextExtractor::ID,
        kind: StepKind::Extractor,
        formats: &[Format::Text],
        doc: "Plain text: encoding (BOM, UTF-8/16, chardetng legacy guess), LF line endings, form-feed pages, paragraphs and headings.",
    },
    #[cfg(feature = "text")]
    StepDoc {
        id: quail_text::TextStats::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Characters, words, scripts and garbage ratio of the extracted text.",
    },
    #[cfg(feature = "text")]
    StepDoc {
        id: quail_text::TextLang::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Document language with top candidates (lingua + sr/hr/bs layer).",
    },
    #[cfg(feature = "signals")]
    StepDoc {
        id: quail_signals::SignalScan::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Identifiers with check digits, dates with roles, amounts, contact domains.",
    },
    #[cfg(feature = "refs")]
    StepDoc {
        id: quail_refs::RefAnalyzer::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Own document numbers, cited documents, title, instrument kind, page/version markers.",
    },
    #[cfg(feature = "parties")]
    StepDoc {
        id: quail_parties::PartyCandidates::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Company mentions with roles and defined terms, ranked candidates, name statements.",
    },
    #[cfg(feature = "clauses")]
    StepDoc {
        id: quail_clauses::ClauseTerms::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Contract clauses: governing law, term and renewal, survival and notice periods, termination rights.",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::MinHashAnalyzer::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "MinHash of word 3-shingles with 32 LSH band keys.",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::SimHashText::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "64-bit SimHash of weighted word 3-shingles.",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::SimHashTemplate::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "64-bit SimHash of entity-masked words (same template, other values).",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::LayoutAnalyzer::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Page-geometry and block-structure SimHashes, heading skeleton digest.",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::InvoiceLayout::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Invoice labels and number-pattern digest.",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::TextKey::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "128-bit key of the folded words (same words whatever the bytes).",
    },
    #[cfg(feature = "sketch")]
    StepDoc {
        id: quail_sketch::CompatMinHash::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Compatibility mode: byte-compatible port of an existing deployment's MinHash and template sketches.",
    },
    #[cfg(all(feature = "sketch", feature = "pdf"))]
    StepDoc {
        id: quail_sketch::PagePHash::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "64-bit DCT pHash of each scanned page (embedded scan or 36 dpi render; needs `raster`).",
    },
    #[cfg(feature = "ocr")]
    StepDoc {
        id: quail_ocr::OcrPages::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "OCR of pages that need it (PP-OCRv6 + Cyrillic re-read, Deep only); text replaces those pages for later steps.",
    },
    #[cfg(feature = "items")]
    StepDoc {
        id: quail_items::ItemLines::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Price items from tables and rate lines: description, quantity, unit, prices, CPV top-3.",
    },
    #[cfg(feature = "summary")]
    StepDoc {
        id: quail_summary::SummaryCard::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "File card from all facts (finalizer): kind, parties, dates, amounts, ids, items, flags.",
    },
    #[cfg(feature = "summary")]
    StepDoc {
        id: quail_summary::SummaryExtractive::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "1-3 sentence English summary without a model: card template + subject clause.",
    },
    #[cfg(feature = "summary")]
    StepDoc {
        id: quail_summary::SummaryOutline::ID,
        kind: StepKind::Analyzer,
        formats: &[],
        doc: "Document outline: headings (numbered, keyword, capitals, type size) with page ranges, bounded.",
    },
];

/// How OCR runs under `--profile deep`, resolved from the options and the machine
/// ([`OcrSettings::resolve`]). Travels to `--isolate` workers inside their spec.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrSettings {
    /// Pages OCR'd at the same time in one process.
    pub jobs: usize,
    /// ONNX Runtime intra-op threads per engine.
    pub threads: usize,
    /// Pages OCR'd per file.
    pub max_pages: u32,
    /// Reading time per page needing OCR (`None`: only the file's timeout).
    pub page_time: Option<Duration>,
}

impl OcrSettings {
    /// The settings for `workers` file workers on this machine with the overrides of
    /// `args`. `explicit_timeout`: the user set `--timeout`, so there is no per-page budget
    /// unless `--ocr-page-time` asks for one.
    #[cfg(feature = "ocr")]
    pub fn resolve(args: &crate::cli::OcrArgs, workers: usize, explicit_timeout: bool) -> Self {
        let r =
            quail_ocr::OcrResources::auto(workers).with_overrides(args.ocr_jobs, args.ocr_threads);
        let d = quail_ocr::OcrConfig::default();
        Self {
            jobs: r.jobs,
            threads: r.threads,
            max_pages: args.ocr_max_pages.unwrap_or(d.max_pages),
            page_time: match args.ocr_page_time {
                Some(t) => Some(t),
                None if explicit_timeout => None,
                None => d.page_time,
            },
        }
    }

    /// Longest wall time per file (`Limits::max_timeout`) that fits the OCR budget of a file
    /// after `base` for the rest, with the other `workers` queued for the OCR jobs ahead of
    /// it. Only a file with pages to OCR is extended towards it.
    #[cfg(feature = "ocr")]
    pub fn file_timeout(&self, base: Duration, workers: usize) -> Duration {
        let ahead = workers.max(1).div_ceil(self.jobs.max(1)).saturating_sub(1);
        self.config()
            .file_timeout(base, u32::try_from(ahead).unwrap_or(u32::MAX))
    }

    #[cfg(feature = "ocr")]
    fn config(&self) -> quail_ocr::OcrConfig {
        quail_ocr::OcrConfig {
            max_pages: self.max_pages,
            page_time: self.page_time,
            ..quail_ocr::OcrConfig::default()
        }
    }
}

/// An engine builder with every step of this build registered, for `profile` and `limits`.
/// Callers may register more (e.g. an analyzer of its own) before building. OCR, under
/// `Profile::Deep`, gets [`OcrSettings::resolve`] defaults for all CPUs.
pub fn builder(profile: Profile, limits: Limits) -> EngineBuilder {
    builder_with(profile, limits, None)
}

/// [`builder`] with the OCR settings of the run (`None`: defaults for this machine).
pub fn builder_with(profile: Profile, limits: Limits, ocr: Option<&OcrSettings>) -> EngineBuilder {
    #[cfg(not(feature = "ocr"))]
    let _ = ocr;
    let b = EngineBuilder::new()
        .profile(profile)
        .limits(limits)
        .quiet_panics(true)
        .analyzer(Box::new(quail_hash::IdentityAnalyzer));
    #[cfg(feature = "docx")]
    let b = b.extractor(Box::new(quail_docx::DocxExtractor::new()));
    #[cfg(feature = "pdf")]
    let b = b.extractor(Box::new(quail_pdf::PdfExtractor::new()));
    #[cfg(feature = "text")]
    let b = b
        .extractor(Box::new(quail_text::TextExtractor::new()))
        .analyzer(Box::new(quail_text::TextStats))
        .analyzer(Box::new(quail_text::TextLang::new()));
    #[cfg(feature = "signals")]
    let b = b.analyzer(Box::new(quail_signals::SignalScan));
    // Compiles its patterns once; shared by every worker.
    #[cfg(feature = "refs")]
    let b = b.analyzer(Box::new(quail_refs::RefAnalyzer::new()));
    #[cfg(feature = "parties")]
    let b = b.analyzer(Box::new(quail_parties::PartyCandidates));
    #[cfg(feature = "clauses")]
    let b = b.analyzer(Box::new(quail_clauses::ClauseTerms));
    // `sketch.minhash_compat@3` is Standard: the engine leaves it out under `Profile::Fast`.
    #[cfg(feature = "sketch")]
    let b = b
        .analyzer(Box::new(quail_sketch::MinHashAnalyzer))
        .analyzer(Box::new(quail_sketch::SimHashText))
        .analyzer(Box::new(quail_sketch::SimHashTemplate))
        .analyzer(Box::new(quail_sketch::LayoutAnalyzer))
        .analyzer(Box::new(quail_sketch::InvoiceLayout))
        .analyzer(Box::new(quail_sketch::TextKey))
        .analyzer(Box::new(quail_sketch::CompatMinHash));
    // Needs the `raster` artifact, which only the PDF extractor provides.
    #[cfg(all(feature = "sketch", feature = "pdf"))]
    let b = b.analyzer(Box::new(quail_sketch::PagePHash));
    // OCR runs in `Profile::Deep` only. The backend (model check, SHA-256 of every file) is
    // built only then; under other profiles the step is inactive and never loads anything.
    #[cfg(feature = "ocr")]
    let b = b.analyzer(Box::new(ocr_analyzer(profile, ocr)));
    // Builds its CPV index once; shared by every worker.
    #[cfg(feature = "items")]
    let b = b.analyzer(Box::new(quail_items::ItemLines::new()));
    // Finalizers: the engine runs them after every other step.
    #[cfg(feature = "summary")]
    let b = b
        .analyzer(Box::new(quail_summary::SummaryCard::new()))
        .analyzer(Box::new(quail_summary::SummaryExtractive))
        .analyzer(Box::new(quail_summary::SummaryOutline::new()));
    b
}

/// Fact key tables of this build: `(crate, keys)`.
pub fn key_tables() -> Vec<(&'static str, &'static [KeyDoc])> {
    #[allow(unused_mut)] // without optional crates nothing is pushed
    let mut v: Vec<(&'static str, &'static [KeyDoc])> = vec![
        ("quail-engine", keydocs::ENGINE),
        ("quail-hash", keydocs::HASH),
    ];
    #[cfg(feature = "docx")]
    v.push(("quail-docx", keydocs::DOCX));
    #[cfg(feature = "pdf")]
    v.push(("quail-pdf", keydocs::PDF));
    #[cfg(feature = "text")]
    v.push(("quail-text", keydocs::TEXT));
    #[cfg(feature = "signals")]
    v.push(("quail-signals", keydocs::SIGNALS));
    #[cfg(feature = "refs")]
    v.push(("quail-refs", keydocs::REFS));
    #[cfg(feature = "parties")]
    v.push(("quail-parties", keydocs::PARTIES));
    #[cfg(feature = "clauses")]
    v.push(("quail-clauses", keydocs::CLAUSES));
    #[cfg(feature = "sketch")]
    v.push(("quail-sketch", keydocs::SKETCH));
    #[cfg(feature = "items")]
    v.push(("quail-items", keydocs::ITEMS));
    #[cfg(feature = "summary")]
    v.push(("quail-summary", keydocs::SUMMARY));
    #[cfg(feature = "ocr")]
    v.push(("quail-ocr", keydocs::OCR));
    v
}

/// The OCR step for `profile`: with the PP-OCRv6 backend under `Deep`, else a stand-in that
/// is never planned.
#[cfg(feature = "ocr")]
fn ocr_analyzer(profile: Profile, ocr: Option<&OcrSettings>) -> quail_ocr::OcrPages {
    if profile < Profile::Deep {
        return quail_ocr::OcrPages::unavailable("ocr runs under --profile deep only");
    }
    let s = ocr.cloned().unwrap_or_else(|| {
        OcrSettings::resolve(
            &crate::cli::OcrArgs::default(),
            quail_ocr::budget::cpus(),
            false,
        )
    });
    let resources = quail_ocr::OcrResources {
        jobs: s.jobs,
        threads: s.threads,
    };
    quail_ocr::OcrPages::with_resources(resources, s.config())
}

/// Why OCR cannot run in this build, if it cannot: no `ocr` feature, no model directory, or
/// missing models. `scan` and `inspect` print it once under `--profile deep`.
pub fn ocr_unavailable() -> Option<String> {
    #[cfg(feature = "ocr")]
    {
        let Some(dir) = quail_ocr::models::model_dir() else {
            return Some(format!(
                "no OCR model directory (set {})",
                quail_ocr::models::MODELS_ENV
            ));
        };
        let missing = quail_ocr::models::missing(&dir);
        if missing.is_empty() {
            None
        } else {
            Some(format!(
                "{} OCR model files missing in {} (run `quail models fetch`)",
                missing.len(),
                dir.display()
            ))
        }
    }
    #[cfg(not(feature = "ocr"))]
    {
        Some("built without the `ocr` feature".to_owned())
    }
}

/// The documentation of a step, if it is registered here.
pub fn step_doc(id: AnalyzerId) -> Option<&'static StepDoc> {
    STEPS.iter().find(|s| s.id.name == id.name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn builds_for_every_profile_and_documents_every_step() {
        for p in [Profile::Fast, Profile::Standard, Profile::Deep] {
            let e = builder(p, Limits::default()).build().unwrap();
            for id in e.plan().chain(e.inactive().iter().copied()) {
                let doc = step_doc(id).unwrap_or_else(|| panic!("{id} has no StepDoc"));
                assert_eq!(doc.id, id, "StepDoc version of {id} is stale");
            }
            for s in STEPS.iter().filter(|s| s.kind == StepKind::Extractor) {
                for &f in s.formats {
                    assert_eq!(e.extractor_for(f), Some(s.id));
                }
            }
        }
    }

    /// The Standard sketch runs under the default profile and is inactive under Fast.
    #[test]
    #[cfg(feature = "sketch")]
    fn standard_sketches_follow_the_profile() {
        let standard = [quail_sketch::CompatMinHash::ID];
        let fast = builder(Profile::Fast, Limits::default()).build().unwrap();
        let std_ = builder(Profile::Standard, Limits::default())
            .build()
            .unwrap();
        for id in standard {
            assert!(fast.inactive().contains(&id), "{id} active under fast");
            assert!(!fast.plan().any(|x| x == id));
            assert!(
                std_.plan().any(|x| x == id),
                "{id} not planned under standard"
            );
        }
        assert!(fast.plan().any(|x| x == quail_sketch::MinHashAnalyzer::ID));
    }

    /// OCR is planned under Deep only (with or without models: without, it reports why).
    #[test]
    #[cfg(feature = "ocr")]
    fn ocr_runs_only_under_deep() {
        let id = quail_ocr::OcrPages::ID;
        for p in [Profile::Fast, Profile::Standard] {
            let e = builder(p, Limits::default()).build().unwrap();
            assert!(e.inactive().contains(&id), "{id} active under {p:?}");
        }
        let deep = builder(Profile::Deep, Limits::default()).build().unwrap();
        assert!(deep.plan().any(|x| x == id));
    }

    /// Every key the evaluation task map reads exists in this build, unless the task is
    /// pending or the key's namespace belongs to a crate this build does not have (`pdf.*`
    /// without the `pdf` feature). Catches a task pointing at a misspelled or renamed key.
    #[test]
    fn eval_tasks_read_real_keys() {
        let tables = key_tables();
        let known: BTreeSet<&str> = tables
            .iter()
            .flat_map(|(_, t)| t.iter().map(|k| k.key.as_str()))
            .collect();
        let namespaces: BTreeSet<&str> = tables
            .iter()
            .flat_map(|(_, t)| t.iter().map(|k| k.key.namespace()))
            .collect();
        let map = quail_eval::tasks::TaskMap::builtin();
        assert!(!map.tasks.is_empty());
        for (name, spec) in &map.tasks {
            if spec.pending.is_some() {
                continue;
            }
            for key in &spec.keys {
                let ns = key.split('.').next().unwrap_or("");
                if namespaces.contains(ns) {
                    assert!(known.contains(key.as_str()), "task {name}: no key {key}");
                }
            }
        }
    }

    /// Every `FactKey::new("...")` in a crate's sources appears in its table: the tables
    /// are generated, this catches a key added without regenerating.
    #[test]
    fn every_declared_key_is_documented() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        let sources = [
            ("quail-engine", "quail-engine/src/lib.rs"),
            ("quail-hash", "quail-hash/src/lib.rs"),
            #[cfg(feature = "docx")]
            ("quail-docx", "quail-docx/src/keys.rs"),
            #[cfg(feature = "pdf")]
            ("quail-pdf", "quail-pdf/src/lib.rs"),
            #[cfg(feature = "text")]
            ("quail-text", "quail-text/src/lib.rs"),
            #[cfg(feature = "signals")]
            ("quail-signals", "quail-signals/src/lib.rs"),
            #[cfg(feature = "refs")]
            ("quail-refs", "quail-refs/src/lib.rs"),
            #[cfg(feature = "parties")]
            ("quail-parties", "quail-parties/src/lib.rs"),
            #[cfg(feature = "clauses")]
            ("quail-clauses", "quail-clauses/src/lib.rs"),
            #[cfg(feature = "sketch")]
            ("quail-sketch", "quail-sketch/src/lib.rs"),
            #[cfg(feature = "items")]
            ("quail-items", "quail-items/src/lib.rs"),
            #[cfg(feature = "summary")]
            ("quail-summary", "quail-summary/src/lib.rs"),
            #[cfg(feature = "ocr")]
            ("quail-ocr", "quail-ocr/src/lib.rs"),
        ];
        let tables = key_tables();
        for (krate, file) in sources {
            let Ok(src) = std::fs::read_to_string(format!("{root}/{file}")) else {
                eprintln!("{file} not found (not in a workspace checkout); skipping");
                continue;
            };
            let declared: BTreeSet<&str> = src
                .split("FactKey::new(\"")
                .skip(1)
                .filter_map(|s| s.split_once('"').map(|(k, _)| k))
                .collect();
            let (_, table) = tables.iter().find(|(c, _)| *c == krate).unwrap();
            let documented: BTreeSet<&str> = table.iter().map(|k| k.key.as_str()).collect();
            assert_eq!(declared, documented, "{krate}: run tools/genkeys.py");
            assert!(
                table.iter().all(|k| !k.doc.is_empty()),
                "{krate}: empty doc"
            );
        }
    }
}

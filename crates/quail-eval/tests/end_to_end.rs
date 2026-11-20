//! End to end: engine (quail-hash + a mock analyzer) over generated files → JSONL → score
//! against generated gold → compare a worse run → inspection packs. All inputs are synthetic.

use std::path::{Path, PathBuf};
use std::process::Command;

use quail_core::{
    Analyzer, AnalyzerId, Ctx, Emitter, Error, Evidence, FactKey, RecordSink, Span, Value, Zone,
};
use quail_engine::{EngineBuilder, RunOptions};
use quail_eval::compare::{CompareOptions, compare};
use quail_eval::gold::{GoldDoc, GoldFile, Label, MatchMode, Split};
use quail_eval::inspect::{self, NoRenderer, PackOptions, Selection, TextFiles, Where};
use quail_eval::score::{ScoreOptions, score, write_scoreboard};
use quail_eval::tasks::TaskMap;
use quail_eval::{blake3_hex, gold, records};
use quail_hash::IdentityAnalyzer;
use quail_io::{FsSource, JsonlSink};

const PARTY: FactKey = FactKey::new("mock.party");
const DATE: FactKey = FactKey::new("mock.date");

/// Reads `PARTY: x`, `DATE: x` and `NOISE: x` lines. Version 1 misses names containing
/// "Gamma"; version 2 also misses "Beta". A file containing `FAIL` makes it error.
struct Mock {
    version: u32,
}

impl Analyzer for Mock {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("mock.extract", self.version)
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> quail_core::Result<()> {
        let text = std::str::from_utf8(ctx.blob.as_bytes()).unwrap_or("");
        if text.contains("FAIL") {
            return Err(Error::Other("mock failure".into()));
        }
        let mut pos = 0usize;
        for line in text.split_inclusive('\n') {
            let start = pos;
            pos += line.len();
            let body = line.trim_end();
            let Some((tag, value)) = body.split_once(": ") else {
                continue;
            };
            let vstart = start + tag.len() + 2;
            let ev = Some(Evidence {
                page: Some(0),
                zone: Zone::Body,
                span: Some(Span {
                    start: u32::try_from(vstart).unwrap(),
                    end: u32::try_from(vstart + value.len()).unwrap(),
                }),
                bbox: None,
            });
            match tag {
                "PARTY" if value.contains("Gamma") => {}
                "PARTY" if self.version >= 2 && value.contains("Beta") => {}
                "PARTY" => out.push(PARTY, Value::text(value), 0.8, ev),
                "DATE" => out.push(DATE, Value::text(value), 0.9, ev),
                "NOISE" => out.push(DATE, Value::text(value), 0.3, ev),
                _ => {}
            }
        }
        Ok(())
    }
}

const TASKS: &str = r#"
[task.party]
keys = ["mock.party"]
normalize = "company"
multi = true

[task.effective_date]
keys = ["mock.date"]
normalize = "date"
"#;

const FILES: &[&str] = &[
    "AGREEMENT 0\nPARTY: Alpha Corp\nPARTY: Beta LLC\nDATE: 2024-01-01\n",
    "AGREEMENT 1\nPARTY: Gamma Ltd\nPARTY: Delta Inc\nDATE: 2024-02-02\n",
    "AGREEMENT 2\nPARTY: Alpha Corp\nNOISE: 2020-02-02\n",
    "AGREEMENT 3\nPARTY: Epsilon GmbH\nDATE: 2024-03-03\n",
    "AGREEMENT 4\nPARTY: Zeta AG\nFAIL\n",
    "AGREEMENT 5\nPARTY: Eta SA\n",
];

fn label(task: &str, v: &str) -> Label {
    Label::text(task, v, MatchMode::Normalized)
}

fn gold_docs() -> Vec<GoldDoc> {
    let doc = |i: usize, labels: Vec<Label>, absent: &[&str]| GoldDoc {
        file: GoldFile {
            blake3: Some(blake3_hex(FILES[i].as_bytes())),
            locator_hint: None,
        },
        dataset: "synthetic".into(),
        split: Split::Dev,
        source: None,
        labels,
        absent: absent.iter().map(|s| (*s).to_string()).collect(),
    };
    let mut d3 = doc(
        3,
        vec![
            label("party", "Epsilon GmbH"),
            label("effective_date", "March 4, 2024"),
        ],
        &[],
    );
    // Matched by locator instead of hash.
    d3.file = GoldFile {
        blake3: None,
        locator_hint: Some("doc3.txt".into()),
    };
    vec![
        doc(
            0,
            vec![
                label("party", "Alpha Corporation"),
                label("party", "Beta, L.L.C."),
                label("effective_date", "January 1, 2024"),
            ],
            &[],
        ),
        doc(
            1,
            vec![
                label("party", "Gamma Ltd"),
                label("party", "Delta"),
                label("effective_date", "2/2/2024"),
            ],
            &[],
        ),
        doc(2, vec![label("party", "Alpha Corp")], &["effective_date"]),
        d3,
        doc(4, vec![label("party", "Zeta AG")], &[]),
    ]
}

fn run_engine(files: &Path, out: &Path, version: u32) {
    let engine = EngineBuilder::new()
        .analyzer(Box::new(IdentityAnalyzer))
        .analyzer(Box::new(Mock { version }))
        .quiet_panics(true)
        .build()
        .unwrap();
    let mut src = FsSource::builder()
        .root(files)
        .sorted(true)
        .build()
        .unwrap();
    let mut sink = JsonlSink::create(out).unwrap();
    let opts = RunOptions {
        threads: 2,
        ordered: true,
        ..RunOptions::default()
    };
    let stats = engine.run(&mut src, &mut sink, &opts);
    sink.finish().unwrap();
    assert_eq!(stats.files, 6);
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    for (i, text) in FILES.iter().enumerate() {
        std::fs::write(files.join(format!("doc{i}.txt")), text).unwrap();
    }
    (dir, files)
}

#[test]
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one scenario, step by step
fn run_score_compare_inspect() {
    let (dir, files) = setup();
    let run_a = dir.path().join("a.jsonl");
    let run_b = dir.path().join("b.jsonl");
    run_engine(&files, &run_a, 1);
    run_engine(&files, &run_b, 2);

    let gold_dir = dir.path().join("gold");
    for d in gold_docs() {
        gold::write_doc(&gold_dir, &d).unwrap();
    }
    let gold: Vec<GoldDoc> = gold::load_dir(&gold_dir)
        .unwrap()
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    let tasks = TaskMap::from_toml(TASKS, Path::new("tasks.toml")).unwrap();
    let opts = ScoreOptions {
        label: "a".into(),
        ..ScoreOptions::default()
    };

    let recs_a = records::load(&run_a).unwrap();
    let a = score(&recs_a, &gold, &tasks, &opts).unwrap();
    assert_eq!(a.corpus.gold_docs, 5);
    assert_eq!(a.corpus.scored_docs, 5);
    assert_eq!(a.corpus.matched_by_locator, 1);
    assert!(a.corpus.unmatched.is_empty());
    assert_eq!(
        (
            a.status.total,
            a.status.ok,
            a.status.partial,
            a.status.failed
        ),
        (6, 5, 1, 0)
    );

    let party = &a.tasks["party"];
    assert_eq!(
        (party.counts.tp, party.counts.fp, party.counts.fn_),
        (5, 0, 2)
    );
    assert_eq!(party.support, 7);
    assert_eq!(party.abstained, 1); // the failing file
    assert_eq!(party.counts.precision, Some(1.0));
    assert!((party.counts.f1.unwrap() - 10.0 / 12.0).abs() < 1e-9);

    let date = &a.tasks["effective_date"];
    assert_eq!((date.counts.tp, date.counts.fp, date.counts.fn_), (2, 2, 1));
    assert_eq!(
        (
            date.docs_labelled,
            date.docs_absent,
            date.predicted_on_absent
        ),
        (3, 1, 1)
    );
    assert_eq!(date.calibration.n, 4);
    assert_eq!(date.calibration.bins[9].n, 3);
    assert_eq!(date.calibration.bins[3].accuracy, Some(0.0));
    assert!(a.latency.contains_key("mock.extract@1"));
    assert!(a.latency.contains_key("hash.identity@1"));
    assert_eq!(a.latency["total"].n, 6);

    // Run B misses "Beta": party F1 drops, the comparison says so.
    let b = score(
        &records::load(&run_b).unwrap(),
        &gold,
        &tasks,
        &ScoreOptions {
            label: "b".into(),
            ..ScoreOptions::default()
        },
    )
    .unwrap();
    assert_eq!(b.tasks["party"].counts.tp, 4);
    let c = compare(&a, &b, &CompareOptions::default());
    assert_eq!(c.exit_code(), 1);
    let party_delta = c.tasks.iter().find(|t| t.task == "party").unwrap();
    assert!(party_delta.regressed);
    assert_eq!(
        party_delta.worse_docs,
        vec![blake3_hex(FILES[0].as_bytes())]
    );
    assert_eq!(compare(&a, &a, &CompareOptions::default()).exit_code(), 0);

    // The binary gates the same way.
    let sa = dir.path().join("a.json");
    let sb = dir.path().join("b.json");
    write_scoreboard(&sa, &a).unwrap();
    write_scoreboard(&sb, &b).unwrap();
    let bin = env!("CARGO_BIN_EXE_quail-eval");
    let st = Command::new(bin)
        .arg("compare")
        .arg(&sa)
        .arg(&sb)
        .output()
        .unwrap();
    assert_eq!(
        st.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&st.stdout)
    );
    let st = Command::new(bin)
        .arg("compare")
        .arg(&sa)
        .arg(&sa)
        .output()
        .unwrap();
    assert_eq!(st.status.code(), Some(0));

    // Inspection packs for every mismatch.
    let inputs = inspect::Inputs::new(&recs_a, &gold, &tasks, None);
    let sel = Selection::Sample {
        n: 10,
        wheres: vec![Where::Mismatch],
        seed: 1,
    };
    let chosen = inspect::select(&inputs, &sel, 0.5);
    assert_eq!(chosen.len(), 4); // docs 1, 2, 3 and the failing 4
    assert_eq!(inspect::select(&inputs, &sel, 0.5), chosen, "seeded");
    let pack_opts = PackOptions {
        out_dir: dir.path().join("inspect"),
        run: "a".into(),
        check_file: true,
    };
    let summary = inspect::write_packs(
        &inputs,
        &chosen,
        &TextFiles::default(),
        &NoRenderer,
        &pack_opts,
    )
    .unwrap();
    assert_eq!(summary.packs.len(), 4);
    assert!(summary.dir.join(".gitignore").is_file());
    assert!(summary.dir.join("index.md").is_file());
    let id3: String = blake3_hex(FILES[3].as_bytes()).chars().take(16).collect();
    let md = std::fs::read_to_string(summary.dir.join(&id3).join("inspect.md")).unwrap();
    assert!(md.contains("same bytes as recorded"), "{md}");
    assert!(
        md.contains("| effective_date | March 4, 2024 | ✗ 2024-03-03"),
        "{md}"
    );
    // Markers carry the fact's number in the record.
    let rec3 = recs_a
        .records
        .iter()
        .find(|r| r.locator.ends_with("doc3.txt"))
        .unwrap();
    let n = |key: &str| {
        rec3.facts
            .iter()
            .position(|f| f.key.as_str() == key)
            .unwrap()
            + 1
    };
    let (p, d) = (n("mock.party"), n("mock.date"));
    assert!(
        md.contains(&format!("PARTY: ⟦{p}|Epsilon GmbH|{p}⟧")),
        "{md}"
    );
    assert!(md.contains(&format!("DATE: ⟦{d}|2024-03-03|{d}⟧")), "{md}");
    assert!(summary.dir.join(&id3).join("record.json").is_file());
}

#[test]
fn binary_score_writes_scoreboard_and_report() {
    let (dir, files) = setup();
    let run = dir.path().join("run.jsonl");
    run_engine(&files, &run, 1);
    let gold_dir = dir.path().join("gold");
    for d in gold_docs() {
        gold::write_doc(&gold_dir, &d).unwrap();
    }
    let tasks = dir.path().join("tasks.toml");
    std::fs::write(&tasks, TASKS).unwrap();
    let out = dir.path().join("out");
    let bin = env!("CARGO_BIN_EXE_quail-eval");
    let st = Command::new(bin)
        .args(["score", "--quiet", "--records"])
        .arg(&run)
        .arg("--gold")
        .arg(&gold_dir)
        .arg("--tasks")
        .arg(&tasks)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let report = std::fs::read_to_string(out.join("report.md")).unwrap();
    assert!(
        report.contains("| party | 7 | 5 | 0 | 5 | 0 | 2 | 100.0 | 71.4 | 83.3 |"),
        "{report}"
    );
    // The report never carries values or locators.
    assert!(!report.contains("Alpha"));
    assert!(!report.contains("doc0"));
    // Holdout is refused without the flag.
    let st = Command::new(bin)
        .args(["score", "--quiet", "--split", "holdout", "--records"])
        .arg(&run)
        .arg("--gold")
        .arg(&gold_dir)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(st.status.code(), Some(2));
}

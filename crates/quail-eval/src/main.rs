//! `quail-eval`: score, compare, inspect, split, convert. See the crate README.

// Doc comments here are clap help text, printed as is: backticks would show.
#![allow(clippy::doc_markdown)]

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use quail_eval::adapters::{self, ConvertOptions};
use quail_eval::compare::{self, CompareOptions};
use quail_eval::gold::{self, GoldDoc};
use quail_eval::inspect::{self, Chain, DocumentSource, NoRenderer, PackOptions, Selection, Where};
use quail_eval::score::{self, ScoreOptions, SplitFilter};
use quail_eval::tasks::TaskMap;
use quail_eval::{Error, Result, records, report, splits};

#[derive(Parser)]
#[command(name = "quail-eval", version, about = "Evaluation harness for quail")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Score a run (JSONL records) against gold: writes scoreboard.json and report.md.
    Score {
        /// Records written by quail (JSONL).
        #[arg(long)]
        records: PathBuf,
        /// Gold directory (one JSON file per document, searched recursively).
        #[arg(long)]
        gold: PathBuf,
        /// Task map (default: the committed tasks.toml).
        #[arg(long)]
        tasks: Option<PathBuf>,
        /// Split to score.
        #[arg(long, value_enum, default_value = "dev")]
        split: SplitArg,
        /// Allow scoring the frozen holdout (release time only).
        #[arg(long)]
        allow_holdout: bool,
        /// Only this dataset.
        #[arg(long)]
        dataset: Option<String>,
        /// Predictions below this confidence count as abstentions.
        #[arg(long)]
        min_conf: Option<f32>,
        /// Run name (default: the records file stem).
        #[arg(long)]
        label: Option<String>,
        /// Output directory (default: eval/runs/<label>).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Do not print the report to stdout.
        #[arg(long)]
        quiet: bool,
    },
    /// Compare two scoreboards (A = before, B = after). Exit 1 on regression, 2 when not comparable.
    Compare {
        /// Scoreboard before.
        a: PathBuf,
        /// Scoreboard after.
        b: PathBuf,
        /// Largest tolerated F1 drop per task, in points (1.0 = one percentage point).
        #[arg(long, default_value_t = 1.0)]
        max_f1_drop: f64,
        /// Largest tolerated rise in failed records.
        #[arg(long, default_value_t = 0)]
        max_failed_increase: u64,
        /// Compare even when the gold sets differ.
        #[arg(long)]
        allow_gold_change: bool,
        /// Gate on per-file p95 latency growing by more than this factor (e.g. 1.5).
        #[arg(long)]
        max_p95_ratio: Option<f64>,
        /// Also write the comparison as JSON here.
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Write inspection packs (record, gold vs predicted, page text with evidence marked).
    Inspect {
        /// Records (JSONL).
        #[arg(long)]
        records: PathBuf,
        /// Gold directory (optional; enables gold vs predicted and --where mismatch).
        #[arg(long)]
        gold: Option<PathBuf>,
        /// Task map (default: the committed tasks.toml).
        #[arg(long)]
        tasks: Option<PathBuf>,
        /// Records whose BLAKE3 starts with this (repeatable).
        #[arg(long)]
        blake3: Vec<String>,
        /// Records whose locator contains this (repeatable).
        #[arg(long)]
        locator: Vec<String>,
        /// Sample this many records.
        #[arg(long)]
        sample: Option<usize>,
        /// Sampling filter, comma-separated, OR-ed: mismatch, low_conf, failed, partial.
        #[arg(long = "where", value_delimiter = ',')]
        wheres: Vec<String>,
        /// Sampling seed.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Confidence below which a fact is "low_conf".
        #[arg(long, default_value_t = 0.5)]
        low_conf: f32,
        /// Predictions below this confidence count as abstentions (as in score).
        #[arg(long)]
        min_conf: Option<f32>,
        /// JSONL dump of extracted documents ({"locator","blake3","document"} per line).
        #[arg(long)]
        documents: Option<PathBuf>,
        /// Pack root (gitignored).
        #[arg(long, default_value = "eval/inspect")]
        out: PathBuf,
        /// Run name (default: the records file stem).
        #[arg(long)]
        run: Option<String>,
        /// Do not re-hash the files on disk.
        #[arg(long)]
        no_file_check: bool,
    },
    /// Assign dev/holdout by BLAKE3 and write committable split lists (hashes only).
    Splits {
        /// Gold directory.
        #[arg(long)]
        gold: PathBuf,
        /// Directory of `<dataset>.{dev,holdout}.txt` lists (read first: listed files keep their side).
        #[arg(long)]
        lists: PathBuf,
        /// Split seed.
        #[arg(long, default_value = splits::DEFAULT_SEED)]
        seed: String,
        /// Share of files in holdout.
        #[arg(long, default_value_t = splits::DEFAULT_HOLDOUT_SHARE)]
        holdout_share: f64,
        /// Verify only: exit 1 if the lists would change or a gold file disagrees.
        #[arg(long)]
        check: bool,
        /// Rewrite the `split` field of gold files that disagree.
        #[arg(long)]
        apply: bool,
    },
    /// Convert a public dataset into gold files.
    Convert {
        /// Adapter: kleister-nda, cuad or realkie-fcc.
        #[arg(long)]
        adapter: String,
        /// Dataset root (default: $QUAIL_TESTDATA/<adapter>).
        #[arg(long)]
        input: Option<PathBuf>,
        /// Gold output directory (outside the repository).
        #[arg(long)]
        out: PathBuf,
        /// Split lists to respect (frozen sides).
        #[arg(long)]
        lists: Option<PathBuf>,
        /// Split seed.
        #[arg(long, default_value = splits::DEFAULT_SEED)]
        seed: String,
        /// Share of files in holdout.
        #[arg(long, default_value_t = splits::DEFAULT_HOLDOUT_SHARE)]
        holdout_share: f64,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SplitArg {
    Dev,
    Holdout,
    All,
}

impl From<SplitArg> for SplitFilter {
    fn from(s: SplitArg) -> Self {
        match s {
            SplitArg::Dev => Self::Dev,
            SplitArg::Holdout => Self::Holdout,
            SplitArg::All => Self::All,
        }
    }
}

fn load_tasks(p: Option<&Path>) -> Result<TaskMap> {
    match p {
        Some(p) => TaskMap::load(p),
        None => Ok(TaskMap::builtin()),
    }
}

fn load_gold(dir: &Path) -> Result<Vec<GoldDoc>> {
    Ok(gold::load_dir(dir)?.into_iter().map(|(_, d)| d).collect())
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("run")
        .trim_end_matches(".jsonl")
        .to_string()
}

#[allow(clippy::too_many_lines)] // one arm per subcommand
fn run(cli: Cli) -> Result<ExitCode> {
    match cli.cmd {
        Cmd::Score {
            records,
            gold,
            tasks,
            split,
            allow_holdout,
            dataset,
            min_conf,
            label,
            out,
            quiet,
        } => {
            let label = label.unwrap_or_else(|| stem(&records));
            let task_map = load_tasks(tasks.as_deref())?;
            let gold_docs = load_gold(&gold)?;
            let recs = records::load(&records)?;
            let mut inputs = BTreeMap::new();
            inputs.insert("records".into(), records.display().to_string());
            inputs.insert(
                "tasks".into(),
                tasks
                    .as_ref()
                    .map_or_else(|| "builtin".into(), |t| t.display().to_string()),
            );
            let opts = ScoreOptions {
                label: label.clone(),
                split: split.into(),
                allow_holdout,
                dataset,
                min_conf,
                inputs,
            };
            let sb = score::score(&recs, &gold_docs, &task_map, &opts)?;
            let out = out.unwrap_or_else(|| PathBuf::from("eval/runs").join(&label));
            score::write_scoreboard(&out.join("scoreboard.json"), &sb)?;
            let md = report::markdown(&sb);
            std::fs::create_dir_all(&out).map_err(|e| Error::io(&out, e))?;
            std::fs::write(out.join("report.md"), &md).map_err(|e| Error::io(&out, e))?;
            if !quiet {
                println!("{md}");
            }
            eprintln!("wrote {}/scoreboard.json and report.md", out.display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Compare {
            a,
            b,
            max_f1_drop,
            max_failed_increase,
            allow_gold_change,
            max_p95_ratio,
            json,
        } => {
            let sa = score::load_scoreboard(&a)?;
            let sb = score::load_scoreboard(&b)?;
            let opts = CompareOptions {
                max_f1_drop: max_f1_drop / 100.0,
                max_failed_increase,
                allow_gold_change,
                max_p95_ratio,
            };
            let c = compare::compare(&sa, &sb, &opts);
            println!("{}", compare::markdown(&c));
            if let Some(j) = json {
                let text =
                    serde_json::to_string_pretty(&c).map_err(|e| Error::Invalid(e.to_string()))?;
                std::fs::write(&j, text + "\n").map_err(|e| Error::io(&j, e))?;
            }
            Ok(ExitCode::from(u8::try_from(c.exit_code()).unwrap_or(2)))
        }
        Cmd::Inspect {
            records,
            gold,
            tasks,
            blake3,
            locator,
            sample,
            wheres,
            seed,
            low_conf,
            min_conf,
            documents,
            out,
            run,
            no_file_check,
        } => {
            let task_map = load_tasks(tasks.as_deref())?;
            let gold_docs = match &gold {
                Some(g) => load_gold(g)?,
                None => vec![],
            };
            let recs = records::load(&records)?;
            let sel = if !blake3.is_empty() {
                Selection::Blake3(blake3)
            } else if !locator.is_empty() {
                Selection::Locator(locator)
            } else if let Some(n) = sample {
                let wheres = wheres
                    .iter()
                    .map(|w| w.parse::<Where>())
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(Error::Invalid)?;
                if wheres.contains(&Where::Mismatch) && gold_docs.is_empty() {
                    return Err(Error::Invalid("--where mismatch needs --gold".into()));
                }
                Selection::Sample { n, wheres, seed }
            } else {
                return Err(Error::Invalid(
                    "choose files with --blake3, --locator or --sample".into(),
                ));
            };
            let inputs = inspect::Inputs::new(&recs, &gold_docs, &task_map, min_conf);
            let chosen = inspect::select(&inputs, &sel, low_conf);
            let mut chain: Vec<Box<dyn DocumentSource>> = Vec::new();
            if let Some(d) = &documents {
                let wanted: HashSet<String> = chosen
                    .iter()
                    .filter_map(|&i| recs.records.get(i))
                    .flat_map(|r| {
                        let mut v = vec![r.locator.clone()];
                        v.extend(records::blake3_of(r));
                        v
                    })
                    .collect();
                chain.push(Box::new(inspect::Sidecar::load(d, Some(&wanted))?));
            }
            chain.push(Box::new(inspect::TextFiles::default()));
            let opts = PackOptions {
                out_dir: out,
                run: run.unwrap_or_else(|| stem(&records)),
                check_file: !no_file_check,
            };
            let summary =
                inspect::write_packs(&inputs, &chosen, &Chain(chain), &NoRenderer, &opts)?;
            println!(
                "{} packs in {} (index.md lists them)",
                summary.packs.len(),
                summary.dir.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Splits {
            gold,
            lists,
            seed,
            holdout_share,
            check,
            apply,
        } => {
            let loaded = gold::load_dir(&gold)?;
            let docs: Vec<GoldDoc> = loaded.iter().map(|(_, d)| d.clone()).collect();
            let frozen = splits::load_lists(&lists)?;
            let sides = splits::resolve(&docs, &frozen, &seed, holdout_share);
            let new_lists = splits::merged(&docs, &sides, &frozen);
            let bad = splits::disagreements(&docs, &sides);
            for (dataset, (dev, hold)) in splits::counts(&new_lists) {
                println!("{dataset}: dev {dev}, holdout {hold}");
            }
            if check {
                let changed = new_lists != frozen;
                if changed {
                    println!("split lists in {} are out of date", lists.display());
                }
                for (id, field, want) in &bad {
                    println!(
                        "{id}: gold says {}, lists say {}",
                        field.as_str(),
                        want.as_str()
                    );
                }
                return Ok(if changed || !bad.is_empty() {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                });
            }
            splits::write_lists(&lists, &new_lists)?;
            if apply {
                for ((path, doc), side) in loaded.iter().zip(&sides) {
                    if doc.split != *side {
                        let mut d = doc.clone();
                        d.split = *side;
                        let text = serde_json::to_string_pretty(&d)
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                        std::fs::write(path, text + "\n").map_err(|e| Error::io(path, e))?;
                    }
                }
                println!("{} gold files updated", bad.len());
            } else if !bad.is_empty() {
                println!(
                    "{} gold files disagree with the lists (use --apply)",
                    bad.len()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Convert {
            adapter,
            input,
            out,
            lists,
            seed,
            holdout_share,
        } => {
            let a = adapters::by_name(&adapter).ok_or_else(|| {
                let names: Vec<&str> = adapters::all().iter().map(|a| a.name()).collect();
                Error::Invalid(format!(
                    "unknown adapter {adapter:?} (known: {})",
                    names.join(", ")
                ))
            })?;
            let input = match input {
                Some(p) => p,
                None => std::env::var_os("QUAIL_TESTDATA")
                    .map(|d| PathBuf::from(d).join(a.name()))
                    .ok_or_else(|| Error::Invalid("give --input or set QUAIL_TESTDATA".into()))?,
            };
            let opts = ConvertOptions {
                seed,
                holdout_share,
                lists: match &lists {
                    Some(l) => splits::load_lists(l)?,
                    None => splits::Lists::new(),
                },
            };
            let conv = a.convert(&input, &opts)?;
            for d in &conv.docs {
                gold::write_doc(&out, d)?;
            }
            println!(
                "{}: {} gold documents written to {}",
                a.name(),
                conv.docs.len(),
                out.display()
            );
            for (k, v) in &conv.stats {
                println!("  {k}: {v}");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("quail-eval: {e}");
            ExitCode::from(2)
        }
    }
}

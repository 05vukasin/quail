//! `quail-fuzz`: a stable-Rust mutation fuzzer for the whole quail pipeline (no nightly, no
//! cargo-fuzz).
//!
//! Panics are not the worst failure in quail: the engine catches them. What kills a scan is an
//! abort on allocation failure, a stack overflow, a hang or a memory blowup, so every case runs
//! in a child worker process under `RLIMIT_AS`, a per-case CPU limit and a wall timeout, and the
//! parent classifies how it ended. See `README.md`.
//!
//! ```text
//! quail-fuzz fuzz --out DIR [--jobs N] [--iterations N] [--time SECS] [--seed N] [--seeds DIR]
//!                 [--mem-mb 1024] [--cpu-secs 30] [--timeout 60] [--deadline 20]
//!                 [--mem-high-mb 512] [--slow-ms 5000] [--kinds docx,pdf,txt]
//!                 [--no-minimize] [--min-secs 120] [--no-render]
//! quail-fuzz run FILE...           classify each file (child process, same limits)
//! quail-fuzz exec FILE             run one file in this process and print the report
//! quail-fuzz minimize FILE OUT     minimize FILE keeping its signature
//! quail-fuzz seeds DIR             write the generated seeds
//! ```

// Generators and mutators are long tables of cases; splitting them would not make them clearer.
#![allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::format_push_string,
    clippy::single_match_else
)]

mod fuzz;
mod minimize;
mod mutate;
mod pdfgen;
mod rng;
mod runner;
mod seeds;
mod target;
mod worker;
mod zipfmt;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use runner::{Runner, Thresholds};
use seeds::Kind;
use worker::WorkerOpts;

struct Args {
    rest: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

const SWITCHES: &[&str] = &["--no-minimize", "--no-render"];

impl Args {
    fn parse(v: impl Iterator<Item = String>) -> Self {
        let mut rest = Vec::new();
        let mut flags = Vec::new();
        let mut it = v.peekable();
        while let Some(a) = it.next() {
            if a.starts_with("--") {
                if SWITCHES.contains(&a.as_str()) {
                    flags.push((a, None));
                } else {
                    let val = it.next();
                    flags.push((a, val));
                }
            } else {
                rest.push(a);
            }
        }
        Self { rest, flags }
    }

    fn has(&self, f: &str) -> bool {
        self.flags.iter().any(|(k, _)| k == f)
    }

    fn get(&self, f: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(k, _)| k == f)
            .and_then(|(_, v)| v.as_deref())
    }

    fn num(&self, f: &str, default: u64) -> Result<u64, String> {
        self.get(f).map_or(Ok(default), |v| {
            v.parse().map_err(|_| format!("{f}: not a number: {v}"))
        })
    }
}

fn worker_opts(a: &Args) -> Result<WorkerOpts, String> {
    Ok(WorkerOpts {
        mem_mb: a.num("--mem-mb", 1024)?,
        cpu_secs: a.num("--cpu-secs", 30)?,
        deadline: Duration::from_secs(a.num("--deadline", 20)?),
        render: !a.has("--no-render"),
    })
}

fn thresholds(a: &Args) -> Result<Thresholds, String> {
    Ok(Thresholds {
        mem_high_kb: a.num("--mem-high-mb", 512)? << 10,
        slow_ms: a.num("--slow-ms", 5000)?,
    })
}

fn scratch_runner(a: &Args) -> Result<(Runner, PathBuf), String> {
    let dir = PathBuf::from(a.get("--out").unwrap_or("quail-fuzz-out"))
        .join("work")
        .join("run");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let timeout = Duration::from_secs(a.num("--timeout", 60)?);
    Ok((
        Runner::new(&dir, worker_opts(a)?, timeout, thresholds(a)?),
        dir,
    ))
}

fn real_main() -> Result<(), String> {
    let mut argv = std::env::args().skip(1);
    let cmd = argv.next().unwrap_or_default();
    let a = Args::parse(argv);
    match cmd.as_str() {
        "worker" => {
            worker::main(&worker_opts(&a)?);
            Ok(())
        }
        "fuzz" => {
            let kinds = a.get("--kinds").map_or_else(Vec::new, |k| {
                k.split(',')
                    .filter_map(|x| match x {
                        "docx" => Some(Kind::Docx),
                        "pdf" => Some(Kind::Pdf),
                        "txt" | "text" => Some(Kind::Text),
                        _ => None,
                    })
                    .collect()
            });
            let cfg = fuzz::Config {
                out: PathBuf::from(a.get("--out").ok_or("--out DIR is required")?),
                seed_dir: a.get("--seeds").map(PathBuf::from),
                iterations: a
                    .get("--iterations")
                    .map(str::parse)
                    .transpose()
                    .map_err(|_| "--iterations: not a number")?,
                jobs: usize::try_from(a.num("--jobs", 1)?).unwrap_or(1),
                time: a
                    .get("--time")
                    .map(str::parse)
                    .transpose()
                    .map_err(|_| "--time: not a number")?
                    .map(Duration::from_secs),
                seed: a.num("--seed", 1)?,
                worker: worker_opts(&a)?,
                timeout: Duration::from_secs(a.num("--timeout", 60)?),
                thresholds: thresholds(&a)?,
                minimize: !a.has("--no-minimize"),
                min_budget: minimize::Budget {
                    time: Duration::from_secs(a.num("--min-secs", 120)?),
                    trials: 400,
                },
                kinds,
            };
            let s = fuzz::run(&cfg).map_err(|e| e.to_string())?;
            print!("{s}");
            Ok(())
        }
        "run" => {
            let (mut r, _) = scratch_runner(&a)?;
            for f in &a.rest {
                let o = r.run(std::path::Path::new(f));
                let rep = o.report.as_ref().map_or_else(String::new, |r| {
                    format!(
                        " status={} peak={}KiB ms={} slowest={}",
                        r.status, r.peak_kb, r.ms, r.slowest
                    )
                });
                println!("{f}\t{}{rep}", o.signature());
                if !o.stderr.is_empty() {
                    println!("{}", o.stderr);
                }
            }
            Ok(())
        }
        "exec" => {
            let opts = worker_opts(&a)?;
            let t = target::Target::new(opts.deadline, opts.render);
            for f in &a.rest {
                let d = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
                println!("{}", t.run(d).to_line());
            }
            Ok(())
        }
        "minimize" => {
            let [input, output] = a.rest.as_slice() else {
                return Err("usage: quail-fuzz minimize FILE OUT".into());
            };
            let (mut r, dir) = scratch_runner(&a)?;
            let data = std::fs::read(input).map_err(|e| e.to_string())?;
            let kind = Kind::of(&data);
            let sig = r.run(std::path::Path::new(input)).signature();
            let budget = minimize::Budget {
                time: Duration::from_secs(a.num("--min-secs", 300)?),
                trials: u32::try_from(a.num("--trials", 2000)?).unwrap_or(2000),
            };
            let m = minimize::minimize(
                &mut r,
                kind,
                &data,
                &sig,
                &dir.join(format!("min.{}", kind.ext())),
                budget,
            );
            std::fs::write(output, &m).map_err(|e| e.to_string())?;
            println!("{sig}: {} -> {} bytes", data.len(), m.len());
            Ok(())
        }
        "seeds" => {
            let dir = PathBuf::from(a.rest.first().ok_or("usage: quail-fuzz seeds DIR")?);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            for (i, (k, b)) in seeds::all().iter().enumerate() {
                std::fs::write(dir.join(format!("seed{i:02}.{}", k.ext())), b)
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        _ => {
            Err("usage: quail-fuzz fuzz|run|exec|minimize|seeds ... (see the crate README)".into())
        }
    }
}

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("quail-fuzz: {e}");
            ExitCode::from(2)
        }
    }
}

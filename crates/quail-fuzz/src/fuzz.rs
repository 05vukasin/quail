//! The fuzzing loop: `jobs` threads, each with its own PRNG stream, corpus and worker
//! process. Findings are de-duplicated by signature; the first input of each signature is
//! saved, minimized and summarized under `<out>/crashes/`.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::minimize::{self, Budget};
use crate::mutate;
use crate::rng::Rng;
use crate::runner::{Class, Outcome, Runner, Thresholds};
use crate::seeds::{self, Kind};
use crate::worker::WorkerOpts;

/// Fuzzing configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// Output directory (`crashes/`, `work/`, `stats.txt`).
    pub out: PathBuf,
    /// Extra seed files.
    pub seed_dir: Option<PathBuf>,
    /// Total cases over all jobs (none: unbounded).
    pub iterations: Option<u64>,
    /// Parallel jobs (one worker process each).
    pub jobs: usize,
    /// Wall-time budget.
    pub time: Option<Duration>,
    /// PRNG seed.
    pub seed: u64,
    /// Worker limits.
    pub worker: WorkerOpts,
    /// Wall-time limit per case.
    pub timeout: Duration,
    /// Soft-finding thresholds.
    pub thresholds: Thresholds,
    /// Minimize new findings.
    pub minimize: bool,
    /// Minimization budget per finding.
    pub min_budget: Budget,
    /// Only these kinds of seeds (empty: all).
    pub kinds: Vec<Kind>,
}

/// One de-duplicated finding.
#[derive(Clone, Debug)]
struct Finding {
    class: Class,
    count: u64,
    kind: Kind,
    mutator: &'static str,
    file: String,
}

#[derive(Default)]
struct Shared {
    findings: BTreeMap<String, Finding>,
    classes: BTreeMap<String, u64>,
    by_kind: BTreeMap<(Kind, String), u64>,
}

fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{:08x}", h >> 32)
}

/// Seeds: generated ones plus every file of `dir` (kind guessed from the bytes).
fn load_seeds(cfg: &Config) -> Vec<(Kind, Vec<u8>)> {
    let mut v = seeds::all();
    if let Some(dir) = &cfg.seed_dir
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            if let Ok(b) = std::fs::read(&p)
                && b.len() <= mutate::MAX_LEN
            {
                v.push((Kind::of(&b), b));
            }
        }
    }
    if !cfg.kinds.is_empty() {
        v.retain(|(k, _)| cfg.kinds.contains(k));
    }
    v
}

/// Runs the fuzzer; returns the summary text (also written to `<out>/stats.txt`).
pub fn run(cfg: &Config) -> std::io::Result<String> {
    let crashes = cfg.out.join("crashes");
    std::fs::create_dir_all(&crashes)?;
    let seeds = Arc::new(load_seeds(cfg));
    if seeds.is_empty() {
        return Err(std::io::Error::other("no seeds"));
    }
    let shared = Arc::new(Mutex::new(Shared::default()));
    let execs = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let jobs = cfg.jobs.max(1);
    let mut handles = Vec::new();
    for j in 0..jobs {
        let per = cfg
            .iterations
            .map(|n| n / jobs as u64 + u64::from((j as u64) < n % jobs as u64));
        let (cfg, seeds, shared, execs, stop) = (
            cfg.clone(),
            seeds.clone(),
            shared.clone(),
            execs.clone(),
            stop.clone(),
        );
        handles.push(std::thread::spawn(move || {
            job(j, per, &cfg, &seeds, &shared, &execs, &stop, start)
        }));
    }
    // progress
    let mut last = Instant::now();
    while handles.iter().any(|h| !h.is_finished()) {
        std::thread::sleep(Duration::from_millis(200));
        if cfg.time.is_some_and(|t| start.elapsed() >= t) {
            stop.store(true, Ordering::Relaxed);
        }
        if last.elapsed() >= Duration::from_secs(30) {
            last = Instant::now();
            let n = execs.load(Ordering::Relaxed);
            let s = shared
                .lock()
                .map(|s| format!("{:?}", s.classes))
                .unwrap_or_default();
            #[allow(clippy::cast_precision_loss)]
            let rate = n as f64 / start.elapsed().as_secs_f64();
            eprintln!(
                "[{:>6}s] {n} execs ({rate:.1}/s) {s}",
                start.elapsed().as_secs()
            );
        }
    }
    for h in handles {
        let _ = h.join();
    }
    let summary = summarize(&shared, execs.load(Ordering::Relaxed), start.elapsed(), cfg);
    std::fs::write(cfg.out.join("stats.txt"), &summary)?;
    Ok(summary)
}

fn summarize(shared: &Mutex<Shared>, execs: u64, took: Duration, cfg: &Config) -> String {
    let Ok(s) = shared.lock() else {
        return String::new();
    };
    let mut out = String::new();
    #[allow(clippy::cast_precision_loss)]
    let rate = execs as f64 / took.as_secs_f64().max(0.001);
    let _ = writeln!(
        out,
        "execs {execs} in {}s ({rate:.1}/s), jobs {}, seed {}, mem {} MiB, cpu {} s, timeout {} s, deadline {} s",
        took.as_secs(),
        cfg.jobs,
        cfg.seed,
        cfg.worker.mem_mb,
        cfg.worker.cpu_secs,
        cfg.timeout.as_secs(),
        cfg.worker.deadline.as_secs()
    );
    let _ = writeln!(out, "\noutcomes (cases):");
    for (c, n) in &s.classes {
        let _ = writeln!(out, "  {c:<16} {n}");
    }
    let _ = writeln!(out, "\noutcomes by input kind:");
    for ((k, c), n) in &s.by_kind {
        let _ = writeln!(out, "  {:<5} {c:<16} {n}", k.as_str());
    }
    let _ = writeln!(
        out,
        "\nfindings (distinct signatures): {}",
        s.findings.len()
    );
    let mut v: Vec<(&String, &Finding)> = s.findings.iter().collect();
    v.sort_by_key(|(sig, f)| {
        (
            !f.class.is_fatal(),
            std::cmp::Reverse(f.count),
            (*sig).clone(),
        )
    });
    for (sig, f) in v {
        let _ = writeln!(
            out,
            "  {:>6}x {sig}  [{} via {}] {}",
            f.count,
            f.kind.as_str(),
            f.mutator,
            f.file
        );
    }
    out
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn job(
    j: usize,
    iterations: Option<u64>,
    cfg: &Config,
    seeds: &[(Kind, Vec<u8>)],
    shared: &Mutex<Shared>,
    execs: &AtomicU64,
    stop: &AtomicBool,
    start: Instant,
) {
    let work = cfg.out.join("work").join(format!("j{j}"));
    if std::fs::create_dir_all(&work).is_err() {
        return;
    }
    let mut runner = Runner::new(&work, cfg.worker.clone(), cfg.timeout, cfg.thresholds);
    let mut rng = Rng::new(cfg.seed, j as u64);
    let mut corpus: Vec<(Kind, Vec<u8>)> = seeds.to_vec();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut i = 0u64;
    while !stop.load(Ordering::Relaxed) && iterations.is_none_or(|n| i < n) {
        if cfg.time.is_some_and(|t| start.elapsed() >= t) {
            break;
        }
        i += 1;
        let (kind, base) = &corpus[rng.below(corpus.len())];
        let kind = *kind;
        let other = &corpus[rng.below(corpus.len())].1;
        let (data, mutator) = mutate::mutate(&mut rng, kind, base, other);
        let path = work.join(format!("cur.{}", kind.ext()));
        if std::fs::write(&path, &data).is_err() {
            continue;
        }
        let o = runner.run(&path);
        execs.fetch_add(1, Ordering::Relaxed);
        let class = o.class.to_string();
        let new = {
            let Ok(mut s) = shared.lock() else { return };
            *s.classes.entry(class.clone()).or_default() += 1;
            *s.by_kind.entry((kind, class)).or_default() += 1;
            if o.class == Class::Ok {
                false
            } else {
                let sig = o.signature();
                if let Some(f) = s.findings.get_mut(&sig) {
                    f.count += 1;
                    false
                } else {
                    s.findings.insert(
                        sig,
                        Finding {
                            class: o.class.clone(),
                            count: 1,
                            kind,
                            mutator,
                            file: String::new(),
                        },
                    );
                    true
                }
            }
        };
        if new {
            let file = save(cfg, &mut runner, &work, kind, mutator, &data, &o, i);
            if let Ok(mut s) = shared.lock()
                && let Some(f) = s.findings.get_mut(&o.signature())
            {
                f.file = file;
            }
        }
        // corpus feedback: keep inputs that produced something never seen
        if let Some(r) = &o.report
            && !o.class.is_fatal()
            && data.len() <= 1 << 20
        {
            let fresh = r.features.iter().filter(|f| seen.insert(**f)).count();
            if fresh > 0 {
                if corpus.len() < 4000 {
                    corpus.push((kind, data));
                } else {
                    let at = seeds.len() + rng.below(corpus.len() - seeds.len());
                    corpus[at] = (kind, data);
                }
            }
        }
    }
}

/// Saves a new finding (raw and minimized) and appends its summary line; returns the file name.
#[allow(clippy::too_many_arguments)]
fn save(
    cfg: &Config,
    runner: &mut Runner,
    work: &Path,
    kind: Kind,
    mutator: &str,
    data: &[u8],
    o: &Outcome,
    iteration: u64,
) -> String {
    let sig = o.signature();
    let base = format!("{}-{}", o.class, short_hash(&sig));
    let dir = cfg.out.join("crashes");
    let raw = dir.join(format!("{base}.{}", kind.ext()));
    let _ = std::fs::write(&raw, data);
    let soft = matches!(o.class, Class::Slow | Class::MemHigh | Class::Deadline);
    let min = if cfg.minimize && !soft {
        let scratch = work.join(format!("min.{}", kind.ext()));
        let m = minimize::minimize(runner, kind, data, &sig, &scratch, cfg.min_budget);
        let _ = std::fs::write(dir.join(format!("{base}-min.{}", kind.ext())), &m);
        Some(m.len())
    } else {
        None
    };
    let mut txt = format!(
        "signature: {sig}\nclass: {}\ndetail: {}\nkind: {}\nmutator: {mutator}\nsize: {}\nminimized: {}\nfirst seen: job iteration {iteration}\n",
        o.class,
        o.detail,
        kind.as_str(),
        data.len(),
        min.map_or_else(|| "-".to_owned(), |m| m.to_string()),
    );
    if let Some(r) = &o.report {
        let _ = writeln!(
            txt,
            "status: {}, peak {} KiB, {} ms, slowest {}",
            r.status, r.peak_kb, r.ms, r.slowest
        );
    }
    if !o.stderr.is_empty() {
        let _ = writeln!(txt, "stderr tail:\n{}", o.stderr);
    }
    let _ = std::fs::write(dir.join(format!("{base}.txt")), txt);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("summary.tsv"))
    {
        let _ = writeln!(
            f,
            "{}\t{sig}\t{}\t{mutator}\t{}\t{}\t{base}.{}",
            o.class,
            kind.as_str(),
            data.len(),
            min.map_or_else(|| "-".to_owned(), |m| m.to_string()),
            kind.ext()
        );
    }
    eprintln!("NEW {sig} ({} bytes, {mutator}) -> {base}", data.len());
    format!("{base}.{}", kind.ext())
}

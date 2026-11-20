//! Runs the extractor over every `.docx`/`.docm`/`.dotx`/`.dotm` under a directory and
//! prints aggregate numbers only (no file names: corpora may be private).
//!
//! ```text
//! cargo run --release -p quail-docx --example corpus -- <dir> [--dump <out-dir>]
//! ```
//!
//! `--dump` writes each document's `full_text()` as `<n>.txt` (numbered in walk order) for
//! comparison against another extractor.

#![allow(missing_docs, clippy::cast_precision_loss, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quail_core::{Blob, Emitter, Extractor, Limits};
use quail_docx::DocxExtractor;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
            matches!(
                x.to_ascii_lowercase().as_str(),
                "docx" | "docm" | "dotx" | "dotm"
            )
        }) {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1) else {
        eprintln!("usage: corpus <dir> [--dump <out-dir>]");
        std::process::exit(2);
    };
    let dump = args
        .iter()
        .position(|a| a == "--dump")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let mut files = Vec::new();
    walk(Path::new(dir), &mut files);
    files.sort();
    let limits = Limits::default();
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut warning_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut key_files: BTreeMap<String, usize> = BTreeMap::new();
    let mut partial = 0usize;
    let (mut ok, mut bytes, mut blocks, mut pages, mut text, mut warned, mut facts) =
        (0usize, 0u64, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut total = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    for (i, f) in files.iter().enumerate() {
        let Ok(data) = std::fs::read(f) else {
            *errors.entry("read".into()).or_default() += 1;
            continue;
        };
        bytes += data.len() as u64;
        let blob = Blob::from(data);
        let mut out = Emitter::new(quail_docx::ID);
        let t = Instant::now();
        let r = DocxExtractor.extract(&blob, &limits, &mut out);
        let dt = t.elapsed();
        total += dt;
        slowest = slowest.max(dt);
        match r {
            Ok(doc) => {
                ok += 1;
                pages += doc.pages.len();
                blocks += doc.pages.iter().map(|p| p.blocks.len()).sum::<usize>();
                let full = doc.full_text();
                text += full.len();
                if let Some(d) = &dump {
                    let _ = std::fs::write(d.join(format!("{i:04}.txt")), full);
                }
            }
            Err(e) => {
                let kind = format!("{e}");
                let kind = kind.split(':').next().unwrap_or("").to_owned();
                *errors.entry(kind).or_default() += 1;
            }
        }
        facts += out.facts().len();
        let mut seen = std::collections::BTreeSet::new();
        for f in out.facts() {
            if seen.insert(f.key.as_str().to_owned()) {
                *key_files.entry(f.key.as_str().to_owned()).or_default() += 1;
            }
        }
        if !out.take_problems().is_empty() {
            partial += 1;
        }
        let w = out.into_parts().2;
        if !w.is_empty() {
            warned += 1;
        }
        for m in w {
            // Message shape only: digits folded, cut before any quoted detail.
            let shape: String = m
                .split(": ")
                .take(2)
                .collect::<Vec<_>>()
                .join(": ")
                .chars()
                .map(|c| if c.is_ascii_digit() { 'N' } else { c })
                .collect();
            *warning_kinds.entry(shape).or_default() += 1;
        }
    }
    let n = files.len().max(1);
    println!("files            {}", files.len());
    println!("ok               {ok}");
    println!("errors           {errors:?}");
    println!("partial          {partial}");
    println!("with warnings    {warned}");
    for (k, v) in &warning_kinds {
        println!("  {v:>5}  {k}");
    }
    println!("files per key:");
    for (k, v) in &key_files {
        println!("  {v:>5}  {k}");
    }
    println!("bytes            {bytes}");
    println!("pages (sections) {pages}");
    println!("avg blocks       {:.1}", blocks as f64 / ok.max(1) as f64);
    println!("avg facts        {:.1}", facts as f64 / n as f64);
    println!("avg text bytes   {:.0}", text as f64 / ok.max(1) as f64);
    println!("total time       {:.1} ms", total.as_secs_f64() * 1e3);
    println!("slowest file     {:.1} ms", slowest.as_secs_f64() * 1e3);
    println!(
        "files/s          {:.0}",
        files.len() as f64 / total.as_secs_f64().max(1e-9)
    );
    println!(
        "MB/s (zipped)    {:.1}",
        bytes as f64 / 1e6 / total.as_secs_f64().max(1e-9)
    );
}

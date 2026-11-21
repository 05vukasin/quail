//! `quail models`: the OCR model files (feature `ocr`).
//!
//! The files, their URLs, sizes, SHA-256 and licenses come from `quail_ocr::models::MANIFEST`.
//! `fetch` downloads the missing or corrupt ones and checks each SHA-256 before moving it into
//! place; `list` shows what the model directory holds.

use std::fmt::Write as _;
use std::path::PathBuf;

use quail_ocr::models;

use crate::cli::{ModelsAction, ModelsArgs};

/// Runs `quail models`; returns what to print.
pub fn run(args: &ModelsArgs) -> Result<String, String> {
    match &args.action {
        ModelsAction::Fetch { dir } => fetch(dir.clone()),
        ModelsAction::List { dir } => list(dir.clone()),
    }
}

fn dir_or_default(dir: Option<PathBuf>) -> Result<PathBuf, String> {
    dir.or_else(models::model_dir).ok_or_else(|| {
        format!(
            "no model directory: pass --dir or set {}",
            models::MODELS_ENV
        )
    })
}

#[allow(clippy::cast_precision_loss)] // sizes in MiB for display
fn mib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1u32 << 20)
}

fn fetch(dir: Option<PathBuf>) -> Result<String, String> {
    let dir = dir_or_default(dir)?;
    let total: u64 = models::MANIFEST.iter().map(|m| m.size).sum();
    eprintln!(
        "quail: {} model files, {:.1} MiB, into {}",
        models::MANIFEST.len(),
        mib(total),
        dir.display()
    );
    let got = models::fetch(&dir).map_err(|e| e.to_string())?;
    let mut s = String::new();
    for p in &got {
        let _ = writeln!(s, "fetched {}", p.display());
    }
    let _ = writeln!(
        s,
        "ok: {} downloaded, {} already present, SHA-256 checked",
        got.len(),
        models::MANIFEST.len() - got.len()
    );
    Ok(s)
}

fn list(dir: Option<PathBuf>) -> Result<String, String> {
    let dir = dir_or_default(dir)?;
    let mut s = String::new();
    let _ = writeln!(s, "model directory: {}", dir.display());
    let w = models::MANIFEST
        .iter()
        .map(|m| m.name.len())
        .max()
        .unwrap_or(0);
    let mut bad = 0usize;
    for m in models::MANIFEST {
        let status = match models::verify(&dir, m) {
            Ok(_) => "ok",
            Err(_) if !dir.join(m.name).exists() => {
                bad += 1;
                "missing"
            }
            Err(_) => {
                bad += 1;
                "bad sha-256"
            }
        };
        let _ = writeln!(
            s,
            "{:<w$}  {:>6.1} MiB  {:<11}  {}",
            m.name,
            mib(m.size),
            status,
            m.license
        );
    }
    if bad > 0 {
        let _ = writeln!(s, "{bad} file(s) to fetch: run `quail models fetch`");
    }
    Ok(s)
}

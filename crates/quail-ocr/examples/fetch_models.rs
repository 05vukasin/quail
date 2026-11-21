//! Downloads the OCR models of the manifest, checking each SHA-256.
//!
//! ```text
//! cargo run -p quail-ocr --features fetch --example fetch_models -- [DIR]
//! ```
//!
//! Without `DIR`, models go to `quail_ocr::models::model_dir()` (`$QUAIL_OCR_MODELS`, else the
//! user cache directory).

use std::path::PathBuf;
use std::process::ExitCode;

use quail_ocr::models;

fn main() -> ExitCode {
    let dir = match std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(models::model_dir)
    {
        Some(d) => d,
        None => {
            eprintln!("no model directory: pass one or set {}", models::MODELS_ENV);
            return ExitCode::FAILURE;
        }
    };
    let total: u64 = models::MANIFEST.iter().map(|m| m.size).sum();
    eprintln!(
        "{} files, {:.1} MiB, into {}",
        models::MANIFEST.len(),
        total as f64 / f64::from(1u32 << 20),
        dir.display()
    );
    match models::fetch(&dir) {
        Ok(got) => {
            for p in &got {
                eprintln!("fetched {}", p.display());
            }
            eprintln!(
                "ok: {} downloaded, {} already present",
                got.len(),
                models::MANIFEST.len() - got.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

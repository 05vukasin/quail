//! Downloads the GGUF models of the manifest, checking each SHA-256.
//!
//! ```text
//! cargo run -p quail-summary --features fetch --example fetch_models -- [small|default|all] [DIR]
//! ```
//!
//! Without a tier, only the default model (Qwen3.5-2B, 1.28 GB) is fetched. Without `DIR`,
//! models go to `quail_summary::models::model_dir()` (`$QUAIL_LLM_MODELS`, else the user cache
//! directory). The license of each file is printed before it is downloaded.

use std::path::PathBuf;
use std::process::ExitCode;

use quail_summary::models::{self, MANIFEST, Tier};

#[allow(clippy::cast_precision_loss)] // sizes in MiB for display
fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let which = args.next().unwrap_or_else(|| "default".to_owned());
    let files: Vec<_> = match which.as_str() {
        "all" => MANIFEST.iter().collect(),
        t => {
            let Some(t) = Tier::parse(t) else {
                eprintln!("unknown tier `{t}`: small, default or all");
                return ExitCode::FAILURE;
            };
            vec![models::file(t)]
        }
    };
    let Some(dir) = args.next().map(PathBuf::from).or_else(models::model_dir) else {
        eprintln!("no model directory: pass one or set {}", models::MODELS_ENV);
        return ExitCode::FAILURE;
    };
    for m in files {
        eprintln!(
            "{} ({}, {:.0} MiB, license {}) into {}",
            m.name,
            m.version,
            m.size as f64 / f64::from(1u32 << 20),
            m.license,
            dir.display()
        );
        match models::fetch(&dir, m) {
            Ok(true) => eprintln!("fetched {}", m.name),
            Ok(false) => eprintln!("already present {}", m.name),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

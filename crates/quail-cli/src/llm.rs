//! `--llm`: registers `summary.llm` with a model behind an API (build feature `llm-api`).
//!
//! Without `--llm` the builder is returned unchanged. With it, the endpoint is resolved from
//! the flags, the environment and the `--env-file` (in that order; see
//! `quail_summary::api::endpoint`), one line `quail: llm: provider=… model=… host=…` goes to
//! stderr (never the key), and the step is registered to run in the run's profile. A build
//! without the feature fails with a clear message instead.

use quail_core::Profile;
use quail_engine::EngineBuilder;

use crate::cli::LlmArgs;

/// The builder with the API summary step when `--llm` is given.
pub fn register(
    b: EngineBuilder,
    args: &LlmArgs,
    profile: Profile,
) -> Result<EngineBuilder, String> {
    let Some(preset) = args.llm.as_deref() else {
        return Ok(b);
    };
    imp::register(b, args, preset, profile)
}

#[cfg(feature = "llm-api")]
mod imp {
    use std::fmt::Write as _;
    use std::sync::Arc;

    use quail_core::Profile;
    use quail_engine::EngineBuilder;
    use quail_summary::api::{ApiConfig, ApiSummarizer, Connect, Env, TrainingLog};
    use quail_summary::cache::MemoryCache;
    use quail_summary::{LlmConfig, SummaryLlm};

    use crate::cli::LlmArgs;

    /// Summaries kept per run: identical documents (the same card and digest) are asked once.
    const CACHE_ENTRIES: usize = 4096;

    pub fn register(
        b: EngineBuilder,
        args: &LlmArgs,
        preset: &str,
        profile: Profile,
    ) -> Result<EngineBuilder, String> {
        let env = match &args.env_file {
            Some(p) => Env::with_file(p).map_err(|e| e.to_string())?,
            None => Env::process(),
        };
        let req = Connect {
            provider: (!preset.trim().is_empty()).then(|| preset.trim().to_owned()),
            model: args.llm_model.clone(),
            ..Connect::default()
        };
        let log = match &args.llm_log {
            Some(p) => Some(Arc::new(TrainingLog::open(p).map_err(|e| e.to_string())?)),
            None => None,
        };
        let api = ApiSummarizer::connect(
            &req,
            &env,
            ApiConfig {
                log,
                ..ApiConfig::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let mut line = format!("quail: llm: {}", api.endpoint().describe());
        if let Some(p) = &args.llm_log {
            let _ = write!(line, " training-log={}", p.display());
        }
        eprintln!("{line}");
        let step = SummaryLlm::new(
            Arc::new(api),
            LlmConfig {
                enabled: true,
                ..LlmConfig::default()
            },
            Some(Arc::new(MemoryCache::new(CACHE_ENTRIES))),
        )
        .with_profile(profile);
        Ok(b.analyzer(Box::new(step)))
    }
}

#[cfg(not(feature = "llm-api"))]
mod imp {
    use quail_core::Profile;
    use quail_engine::EngineBuilder;

    use crate::cli::LlmArgs;

    pub fn register(
        _: EngineBuilder,
        _: &LlmArgs,
        _: &str,
        _: Profile,
    ) -> Result<EngineBuilder, String> {
        Err("--llm needs a quail built with the `llm-api` feature \
             (cargo build --release -p quail-cli --features llm-api)"
            .to_owned())
    }
}

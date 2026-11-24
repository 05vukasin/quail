//! Opt-in training log: one JSONL row per model call, for building a fine-tuning set later.
//!
//! **It holds document text** (the card, the digest the model read, the model's output) and
//! must stay private: never commit it, never share it, delete it when done. It is off unless
//! the caller names a file ([`TrainingLog::open`]). On Unix a new file is created readable by
//! its owner only (`0600`). API keys are never written.
//!
//! Row fields: `ts` (Unix seconds), `summarizer` (`name@version`), `fingerprint`, `provider`,
//! `model`, `step` (`single`, `map` or `reduce`), `system`, `user` (the whole message the
//! model read), `card_text` and `digest` (for `single`), `output` (the raw model text, or
//! `null` when the call failed), `error`?, `checked` (the fact-check verdict; `null` when not
//! checked here), `unsupported` (the claims it did not find), `prompt_tokens`,
//! `output_tokens`.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value as Json, json};

use quail_core::{Error, Result};

/// One call, as logged.
#[derive(Clone, Debug, Default)]
pub struct Row<'a> {
    /// `name@version` of the summarizer.
    pub summarizer: String,
    /// Its fingerprint.
    pub fingerprint: &'a str,
    /// Preset.
    pub provider: &'a str,
    /// Model id.
    pub model: &'a str,
    /// `single`, `map` or `reduce`.
    pub step: &'a str,
    /// System message.
    pub system: &'a str,
    /// User message.
    pub user: &'a str,
    /// The card text (single calls).
    pub card_text: Option<&'a str>,
    /// The digest body (single calls).
    pub digest: Option<&'a str>,
    /// Raw output, `None` when the call failed.
    pub output: Option<&'a str>,
    /// Why the call failed.
    pub error: Option<String>,
    /// Fact-check verdict, when checked here.
    pub checked: Option<bool>,
    /// Claims not found.
    pub unsupported: &'a [String],
    /// Prompt tokens.
    pub prompt_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
}

/// An append-only JSONL file shared by every worker.
#[derive(Debug)]
pub struct TrainingLog {
    file: Mutex<File>,
}

impl TrainingLog {
    /// Opens (or creates) `path` for appending.
    pub fn open(path: &Path) -> Result<Self> {
        let mut o = OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let file = o
            .open(path)
            .map_err(|e| Error::Resource(format!("training log {}: {e}", path.display())))?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    /// Appends one row (one line, written whole). A failed write is returned, never a panic.
    pub fn write(&self, r: &Row<'_>) -> Result<()> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let opt = |s: Option<&str>| s.map_or(Json::Null, |s| json!(s));
        let row = json!({
            "ts": ts,
            "summarizer": r.summarizer,
            "fingerprint": r.fingerprint,
            "provider": r.provider,
            "model": r.model,
            "step": r.step,
            "system": r.system,
            "user": r.user,
            "card_text": opt(r.card_text),
            "digest": opt(r.digest),
            "output": opt(r.output),
            "error": r.error.as_deref().map_or(Json::Null, |e| json!(e)),
            "checked": r.checked.map_or(Json::Null, Json::Bool),
            "unsupported": r.unsupported,
            "prompt_tokens": r.prompt_tokens,
            "output_tokens": r.output_tokens,
        });
        let mut line = row.to_string();
        line.push('\n');
        let mut f = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        f.write_all(line.as_bytes())
            .map_err(|e| Error::Resource(format!("training log: {e}")))
    }
}

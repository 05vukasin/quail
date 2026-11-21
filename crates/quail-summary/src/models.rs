//! The model manifest and the model directory of the `llm` backend.
//!
//! Weights are not part of the repository or the crate. [`MANIFEST`] names each GGUF file with
//! its Hugging Face URL pinned to a repository **commit** (not `main`), its size, SHA-256 and
//! SPDX license; files live in [`model_dir`] and are fetched with the `fetch_models` example
//! (feature `fetch`) or by hand. The backend checks the SHA-256 before loading a file. Only
//! permissively licensed weights (Apache-2.0, MIT) are listed.

use std::path::{Path, PathBuf};

/// Size and quality tier of a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Qwen3.5-0.8B `Q4_K_M`: ~0.9 GB resident, fastest, wordier and less exact.
    Small,
    /// Qwen3.5-2B `Q4_K_M`: ~2.0 GB resident; the default.
    Default,
}

impl Tier {
    /// `small` or `default`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Default => "default",
        }
    }

    /// The tier named `s`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "small" => Some(Self::Small),
            "default" => Some(Self::Default),
            _ => None,
        }
    }
}

/// One file of the manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// Its tier.
    pub tier: Tier,
    /// Short model id, used in summaries and cache keys.
    pub id: &'static str,
    /// File name in the model directory.
    pub name: &'static str,
    /// Upstream model and quantization.
    pub version: &'static str,
    /// Download URL, pinned to a repository commit.
    pub url: &'static str,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256.
    pub sha256: &'static str,
    /// SPDX license of the weights.
    pub license: &'static str,
}

/// Every model the `llm` backend may load. Sizes and hashes are the Hugging Face LFS metadata
/// of the pinned commits, read on 2026-11-21.
pub const MANIFEST: &[ModelFile] = &[
    ModelFile {
        tier: Tier::Default,
        id: "qwen3.5-2b-q4_k_m",
        name: "Qwen3.5-2B-Q4_K_M.gguf",
        version: "Qwen3.5-2B (2026-02-28), unsloth GGUF Q4_K_M",
        url: "https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/resolve/f6d5376be1edb4d416d56da11e5397a961aca8ae/Qwen3.5-2B-Q4_K_M.gguf",
        size: 1_280_835_840,
        sha256: "aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223",
        license: "Apache-2.0",
    },
    ModelFile {
        tier: Tier::Small,
        id: "qwen3.5-0.8b-q4_k_m",
        name: "Qwen3.5-0.8B-Q4_K_M.gguf",
        version: "Qwen3.5-0.8B (2026-02-28), unsloth GGUF Q4_K_M",
        url: "https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/resolve/6ab461498e2023f6e3c1baea90a8f0fe38ab64d0/Qwen3.5-0.8B-Q4_K_M.gguf",
        size: 532_517_120,
        sha256: "bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517",
        license: "Apache-2.0",
    },
];

/// The manifest entry of `tier`.
pub fn file(tier: Tier) -> &'static ModelFile {
    // Every tier has exactly one entry (tested).
    MANIFEST
        .iter()
        .find(|m| m.tier == tier)
        .unwrap_or(&MANIFEST[0])
}

/// Environment variable naming the model directory; it wins over the platform default.
pub const MODELS_ENV: &str = "QUAIL_LLM_MODELS";

/// Where models are looked up: `$QUAIL_LLM_MODELS`, else the user cache directory
/// (`$XDG_CACHE_HOME` or `~/.cache` on Linux, `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on
/// Windows) plus `quail/llm-models`. `None` when no home directory is known.
pub fn model_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os(MODELS_ENV).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    base.map(|b| b.join("quail").join("llm-models"))
}

/// Whether `m` is present in `dir` with the right size.
pub fn present(dir: &Path, m: &ModelFile) -> bool {
    std::fs::metadata(dir.join(m.name)).is_ok_and(|md| md.is_file() && md.len() == m.size)
}

/// Lowercase hex SHA-256 of a file.
#[cfg(any(feature = "llm", feature = "fetch"))]
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(crate::format::hex(&h.finalize()))
}

/// Checks the SHA-256 of `m` in `dir` and returns its path.
#[cfg(any(feature = "llm", feature = "fetch"))]
pub fn verify(dir: &Path, m: &ModelFile) -> quail_core::Result<PathBuf> {
    let path = dir.join(m.name);
    let got = sha256_file(&path)
        .map_err(|e| quail_core::Error::Resource(format!("{}: {e}", path.display())))?;
    if got == m.sha256 {
        Ok(path)
    } else {
        Err(quail_core::Error::Resource(format!(
            "{}: SHA-256 {got}, expected {}",
            path.display(),
            m.sha256
        )))
    }
}

/// Downloads `m` into `dir` unless it is already there with the right hash; returns whether
/// it downloaded. Writes to a `.part` file and renames it only after the hash matched.
#[cfg(feature = "fetch")]
pub fn fetch(dir: &Path, m: &ModelFile) -> quail_core::Result<bool> {
    use std::io::{Read as _, Write as _};
    let res = |e: String| quail_core::Error::Resource(e);
    std::fs::create_dir_all(dir).map_err(|e| res(format!("{}: {e}", dir.display())))?;
    if present(dir, m) && verify(dir, m).is_ok() {
        return Ok(false);
    }
    let part = dir.join(format!("{}.part", m.name));
    let resp = ureq::get(m.url)
        .call()
        .map_err(|e| res(format!("{}: {e}", m.url)))?;
    let mut body = resp.into_body();
    let mut reader = body.as_reader();
    let mut out =
        std::fs::File::create(&part).map_err(|e| res(format!("{}: {e}", part.display())))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| res(format!("{}: {e}", m.url)))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > m.size {
            let _ = std::fs::remove_file(&part);
            return Err(res(format!("{}: larger than expected", m.name)));
        }
        out.write_all(&buf[..n])
            .map_err(|e| res(format!("{}: {e}", part.display())))?;
    }
    out.flush()
        .map_err(|e| res(format!("{}: {e}", part.display())))?;
    drop(out);
    let got = sha256_file(&part).map_err(|e| res(format!("{}: {e}", part.display())))?;
    if got != m.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(res(format!(
            "{}: SHA-256 {got}, expected {}",
            m.url, m.sha256
        )));
    }
    std::fs::rename(&part, dir.join(m.name)).map_err(|e| res(format!("{}: {e}", m.name)))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_pinned_and_permissive() {
        for t in [Tier::Small, Tier::Default] {
            assert_eq!(file(t).tier, t);
            assert_eq!(MANIFEST.iter().filter(|m| m.tier == t).count(), 1);
            assert_eq!(Tier::parse(t.as_str()), Some(t));
        }
        for m in MANIFEST {
            assert!(m.url.contains("/resolve/"), "{}", m.url);
            assert!(!m.url.contains("/resolve/main/"), "{} is not pinned", m.url);
            assert!(m.url.ends_with(m.name));
            assert_eq!(m.sha256.len(), 64);
            assert!(matches!(m.license, "Apache-2.0" | "MIT"));
        }
    }
}

//! The model manifest and the model directory.
//!
//! Models are not part of the repository or the crate. [`MANIFEST`] names every file the
//! `paddle` backend uses with its source URL, size, SHA-256 and license; the files live in
//! [`model_dir`] and are fetched with the `fetch_models` example (feature `fetch`) or by hand.
//! The backend checks every file's SHA-256 before loading it.

use std::path::{Path, PathBuf};

/// What a model file is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Text line detection (DB).
    Detector,
    /// Primary recognizer: Latin (with `č ć š ž đ`), English, Chinese, Japanese.
    Recognizer,
    /// Dictionary of the primary recognizer.
    RecognizerDict,
    /// Cyrillic recognizer (with Serbian `ђ ј љ њ ћ џ`).
    CyrillicRecognizer,
    /// Dictionary of the Cyrillic recognizer.
    CyrillicDict,
    /// Whole-page orientation classifier (0/90/180/270).
    DocOrientation,
    /// Per-line orientation classifier (0/180).
    LineOrientation,
}

/// One file of the manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// What it is for.
    pub role: Role,
    /// File name in the model directory.
    pub name: &'static str,
    /// Upstream model and release.
    pub version: &'static str,
    /// Where it is downloaded from (the `oar-ocr` mirror of the `PaddleOCR` ONNX exports).
    pub url: &'static str,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256.
    pub sha256: &'static str,
    /// SPDX license of the weights.
    pub license: &'static str,
}

macro_rules! mirror {
    ($name:literal) => {
        concat!(
            "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=",
            $name
        )
    };
}

/// Every file the `paddle` backend may load. The hashes are those of the `oar-ocr` 0.11.0
/// registry, checked against downloads on 2026-11-21.
pub const MANIFEST: &[ModelFile] = &[
    ModelFile {
        role: Role::Detector,
        name: "pp-ocrv6_small_det.onnx",
        version: "PP-OCRv6_small_det (PaddleOCR 3.7)",
        url: mirror!("pp-ocrv6_small_det.onnx"),
        size: 9_880_512,
        sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
        license: "Apache-2.0",
    },
    ModelFile {
        role: Role::Recognizer,
        name: "pp-ocrv6_small_rec.onnx",
        version: "PP-OCRv6_small_rec (PaddleOCR 3.7)",
        url: mirror!("pp-ocrv6_small_rec.onnx"),
        size: 21_159_378,
        sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
        license: "Apache-2.0",
    },
    ModelFile {
        role: Role::RecognizerDict,
        name: "ppocrv6_dict.txt",
        version: "PP-OCRv6 dictionary, 18,708 entries",
        url: mirror!("ppocrv6_dict.txt"),
        size: 74_947,
        sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
        license: "Apache-2.0",
    },
    ModelFile {
        role: Role::CyrillicRecognizer,
        name: "cyrillic_pp-ocrv5_mobile_rec.onnx",
        version: "cyrillic_PP-OCRv5_mobile_rec (PaddleOCR 3.x)",
        url: mirror!("cyrillic_pp-ocrv5_mobile_rec.onnx"),
        size: 8_076_390,
        sha256: "a18d96d7c8d73d90f2ed056549caa1de3a8e6cb744cccba16cd593ea8cd2d569",
        license: "Apache-2.0",
    },
    ModelFile {
        role: Role::CyrillicDict,
        name: "ppocrv5_cyrillic_dict.txt",
        version: "PP-OCRv5 Cyrillic dictionary",
        url: mirror!("ppocrv5_cyrillic_dict.txt"),
        size: 2_781,
        sha256: "db40aa52ceb112055be80c694afdf655d5d2c4f7873704524cc16a447ca913ba",
        license: "Apache-2.0",
    },
    ModelFile {
        role: Role::DocOrientation,
        name: "pp-lcnet_x1_0_doc_ori.onnx",
        version: "PP-LCNet_x1_0_doc_ori",
        url: mirror!("pp-lcnet_x1_0_doc_ori.onnx"),
        size: 6_787_248,
        sha256: "bbcd6c2b43ab15d2e605455aef2cd280ab87b570824522d24a69e9298875a1ac",
        license: "Apache-2.0",
    },
    ModelFile {
        role: Role::LineOrientation,
        name: "pp-lcnet_x0_25_textline_ori.onnx",
        version: "PP-LCNet_x0_25_textline_ori",
        url: mirror!("pp-lcnet_x0_25_textline_ori.onnx"),
        size: 1_018_629,
        sha256: "fb402220f39b183d64a68cd48d4bd53267a21354b6fc39370c2a83fbdad85b10",
        license: "Apache-2.0",
    },
];

/// The manifest entry for `role`.
pub fn file(role: Role) -> &'static ModelFile {
    // Every role has exactly one entry (tested).
    MANIFEST
        .iter()
        .find(|m| m.role == role)
        .unwrap_or(&MANIFEST[0])
}

/// Environment variable naming the model directory; it wins over the platform default.
pub const MODELS_ENV: &str = "QUAIL_OCR_MODELS";

/// Where models are looked up: `$QUAIL_OCR_MODELS`, else the user cache directory
/// (`$XDG_CACHE_HOME` or `~/.cache` on Linux, `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on
/// Windows) plus `quail/ocr-models`. `None` when no home directory is known.
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
    base.map(|b| b.join("quail").join("ocr-models"))
}

/// Manifest files that are absent from `dir` or have the wrong size.
pub fn missing(dir: &Path) -> Vec<&'static ModelFile> {
    MANIFEST
        .iter()
        .filter(|m| {
            std::fs::metadata(dir.join(m.name))
                .map_or(true, |md| !md.is_file() || md.len() != m.size)
        })
        .collect()
}

/// Lowercase hex SHA-256 of a file.
#[cfg(any(feature = "paddle", feature = "fetch"))]
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::fmt::Write as _;
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
    let mut s = String::with_capacity(64);
    for b in h.finalize() {
        let _ = write!(s, "{b:02x}");
    }
    Ok(s)
}

/// Checks the SHA-256 of `m` in `dir`.
#[cfg(any(feature = "paddle", feature = "fetch"))]
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

/// Downloads every missing or corrupt manifest file into `dir` (created if needed), checking
/// each SHA-256 before moving it into place. Returns the files it downloaded.
#[cfg(feature = "fetch")]
pub fn fetch(dir: &Path) -> quail_core::Result<Vec<PathBuf>> {
    use quail_core::Error;
    use std::io::{Read as _, Write as _};
    std::fs::create_dir_all(dir)?;
    let mut got = Vec::new();
    for m in MANIFEST {
        if verify(dir, m).is_ok() {
            continue;
        }
        let part = dir.join(format!("{}.part", m.name));
        let resp = ureq::get(m.url)
            .call()
            .map_err(|e| Error::Resource(format!("{}: {e}", m.url)))?;
        let mut body = resp.into_body();
        let mut reader = body.as_reader();
        let mut out = std::fs::File::create(&part)?;
        let mut buf = vec![0u8; 1 << 16];
        let mut n_total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            n_total += n as u64;
            if n_total > m.size {
                return Err(Error::Resource(format!("{}: larger than expected", m.name)));
            }
            out.write_all(&buf[..n])?;
        }
        out.sync_all()?;
        drop(out);
        let sha = sha256_file(&part)?;
        if sha != m.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Resource(format!(
                "{}: SHA-256 {sha}, expected {}",
                m.name, m.sha256
            )));
        }
        let path = dir.join(m.name);
        std::fs::rename(&part, &path)?;
        got.push(path);
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_complete_and_well_formed() {
        for role in [
            Role::Detector,
            Role::Recognizer,
            Role::RecognizerDict,
            Role::CyrillicRecognizer,
            Role::CyrillicDict,
            Role::DocOrientation,
            Role::LineOrientation,
        ] {
            assert_eq!(MANIFEST.iter().filter(|m| m.role == role).count(), 1);
            assert_eq!(file(role).role, role);
        }
        for m in MANIFEST {
            assert_eq!(m.sha256.len(), 64);
            assert!(
                m.sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
            assert!(m.url.starts_with("https://") && m.url.ends_with(m.name));
            assert_eq!(m.license, "Apache-2.0");
            assert!(m.size > 0);
        }
    }

    #[test]
    fn missing_lists_everything_in_an_empty_dir() {
        let dir = std::env::temp_dir().join("quail-ocr-test-no-such-dir-7f3a");
        assert_eq!(missing(&dir).len(), MANIFEST.len());
    }
}

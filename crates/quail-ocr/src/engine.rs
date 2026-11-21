//! The engine contract: an image in, lines and words with pixel boxes and confidences out.

use std::fmt;
use std::sync::Arc;

use quail_core::{Image, Result};
use serde::{Deserialize, Serialize};

use crate::script::Script;

/// Who recognized a page: a backend name plus a fingerprint of everything that changes its
/// output (model hashes, thresholds, routing). Part of the cache key, so two engines with the
/// same id must give the same result for the same pixels.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineId {
    /// Backend and model family, e.g. `paddle/pp-ocrv6-small+cyrillic-v5`.
    pub name: String,
    /// BLAKE3 over the models and the configuration.
    pub fingerprint: [u8; 32],
}

impl EngineId {
    /// Id with a fingerprint computed from `config` (any bytes that identify the setup).
    pub fn new(name: impl Into<String>, config: &[u8]) -> Self {
        let name = name.into();
        let mut h = blake3::Hasher::new();
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(config);
        Self {
            name,
            fingerprint: *h.finalize().as_bytes(),
        }
    }

    /// `name@` plus the first 8 hex digits of the fingerprint, for provenance.
    pub fn short(&self) -> String {
        use fmt::Write as _;
        let mut s = format!("{}@", self.name);
        for b in &self.fingerprint[..4] {
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}

impl fmt::Debug for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Which scripts to expect on a page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptHint {
    /// Decide per page from a sample of lines (the default).
    #[default]
    Auto,
    /// Latin only (skips the Cyrillic recognizer).
    Latin,
    /// Cyrillic first; Latin lines are still read when they are more confident.
    Cyrillic,
}

/// What the caller knows about the image.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrHints {
    /// Expected scripts.
    pub script: ScriptHint,
    /// Pixels per inch of the image (rendered DPI, or the scan's effective resolution).
    pub dpi: f32,
}

impl Default for OcrHints {
    fn default() -> Self {
        Self {
            script: ScriptHint::Auto,
            dpi: 200.0,
        }
    }
}

/// An axis-aligned box in image pixels, origin top-left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PxBox {
    /// Left.
    pub x0: f32,
    /// Top.
    pub y0: f32,
    /// Right.
    pub x1: f32,
    /// Bottom.
    pub y1: f32,
}

impl PxBox {
    /// The smallest box around `points`; an empty box for no points. Non-finite coordinates
    /// are ignored.
    pub fn around(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let mut b: Option<Self> = None;
        for (x, y) in points {
            if !(x.is_finite() && y.is_finite()) {
                continue;
            }
            b = Some(match b {
                None => Self {
                    x0: x,
                    y0: y,
                    x1: x,
                    y1: y,
                },
                Some(b) => Self {
                    x0: b.x0.min(x),
                    y0: b.y0.min(y),
                    x1: b.x1.max(x),
                    y1: b.y1.max(y),
                },
            });
        }
        b.unwrap_or_default()
    }

    /// Width (never negative).
    pub fn width(&self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }

    /// Height (never negative).
    pub fn height(&self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }

    /// Vertical centre.
    pub fn cy(&self) -> f32 {
        f32::midpoint(self.y0, self.y1)
    }
}

/// A word inside a line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrWord {
    /// Its text.
    pub text: String,
    /// Its box in pixels.
    pub bbox: PxBox,
    /// Recognition confidence in [0, 1] (the line's when the engine has no per-word score).
    pub conf: f32,
}

/// One recognized text line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    /// Its text.
    pub text: String,
    /// The detected quadrilateral, clockwise from top-left, in pixels of the input image.
    pub quad: [(f32, f32); 4],
    /// Its axis-aligned box in pixels.
    pub bbox: PxBox,
    /// Recognition confidence in [0, 1].
    pub conf: f32,
    /// Script of its letters.
    pub script: Script,
    /// Words, when the engine reports them (possibly empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<OcrWord>,
}

/// What an engine read on one image. Coordinates are pixels of the image it was given,
/// whatever rotation it applied internally.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OcrPage {
    /// Width of the input image.
    pub width: u32,
    /// Height of the input image.
    pub height: u32,
    /// Clockwise rotation (0, 90, 180, 270) the engine found the page content to have.
    pub rotation: u16,
    /// Lines in the engine's order (not necessarily reading order).
    pub lines: Vec<OcrLine>,
}

impl OcrPage {
    /// Characters (not counting spaces) over all lines.
    pub fn chars(&self) -> usize {
        self.lines
            .iter()
            .map(|l| l.text.chars().filter(|c| !c.is_whitespace()).count())
            .sum()
    }

    /// Mean confidence weighted by characters; `None` when nothing was read.
    pub fn mean_conf(&self) -> Option<f32> {
        let mut n = 0usize;
        let mut sum = 0f64;
        for l in &self.lines {
            let c = l.text.chars().filter(|c| !c.is_whitespace()).count();
            n += c;
            #[allow(clippy::cast_precision_loss)]
            {
                sum += f64::from(l.conf) * c as f64;
            }
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        (n > 0).then(|| (sum / n as f64) as f32)
    }
}

/// An OCR backend. Engines hold mutable inference state (an ONNX Runtime session is run
/// through `&mut`), so each one serves one thread at a time; [`EngineFactory`] makes one per
/// worker.
pub trait OcrEngine: Send {
    /// Identity and configuration fingerprint.
    fn id(&self) -> &EngineId;

    /// Reads `image` (1, 3 or 4 channels). Boxes come back in its pixels.
    fn recognize(&mut self, image: &Image, hints: &OcrHints) -> Result<OcrPage>;
}

/// Makes engines, one per worker thread. Model files are loaded (and checked) when an engine
/// is created; the factory itself is cheap and shared.
pub trait EngineFactory: Send + Sync {
    /// The id every engine it makes reports.
    fn id(&self) -> EngineId;

    /// A fresh engine.
    fn create(&self) -> Result<Box<dyn OcrEngine>>;
}

impl<F: EngineFactory + ?Sized> EngineFactory for Arc<F> {
    fn id(&self) -> EngineId {
        (**self).id()
    }

    fn create(&self) -> Result<Box<dyn OcrEngine>> {
        (**self).create()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_around_ignores_nan() {
        let b = PxBox::around([(1.0, 5.0), (f32::NAN, 0.0), (3.0, 2.0)]);
        assert_eq!(
            b,
            PxBox {
                x0: 1.0,
                y0: 2.0,
                x1: 3.0,
                y1: 5.0
            }
        );
        assert_eq!(PxBox::around([]), PxBox::default());
    }

    #[test]
    fn mean_conf_is_char_weighted() {
        let line = |t: &str, c| OcrLine {
            text: t.into(),
            quad: [(0.0, 0.0); 4],
            bbox: PxBox::default(),
            conf: c,
            script: Script::Latin,
            words: vec![],
        };
        let p = OcrPage {
            lines: vec![line("abc", 1.0), line("a", 0.0)],
            ..OcrPage::default()
        };
        assert_eq!(p.chars(), 4);
        assert!((p.mean_conf().unwrap_or(0.0) - 0.75).abs() < 1e-6);
        assert_eq!(OcrPage::default().mean_conf(), None);
    }

    #[test]
    fn engine_id_depends_on_config() {
        let a = EngineId::new("x", b"1");
        assert_eq!(a, EngineId::new("x", b"1"));
        assert_ne!(a, EngineId::new("x", b"2"));
        assert!(a.short().starts_with("x@"));
    }
}

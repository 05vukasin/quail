//! PP-OCRv6 on ONNX Runtime through `oar-ocr` (feature `paddle`).
//!
//! Per page:
//!
//! 1. **Orientation.** PP-LCNet doc orientation on a thumbnail; the page is turned upright
//!    when the classifier is sure ([`PaddleConfig::orientation_min_score`]). Boxes are mapped
//!    back to the input image afterwards.
//! 2. **Detection and recognition.** PP-OCRv6 small (DB detector, long side capped at 1600 px by default,
//!    [`PaddleConfig::det_max_side`]), PP-LCNet text-line orientation (0/180) per line, the
//!    PP-OCRv6 small recognizer (Latin with `č ć š ž đ`, English, Chinese, Japanese).
//!    Optional deskew ([`PaddleConfig::deskew_degrees`]).
//! 3. **Cyrillic routing.** PP-OCRv6 has no Cyrillic. The longest lines whose Latin reading
//!    is below [`PaddleConfig::latin_sure`] (up to [`PaddleConfig::script_sample`]; none on a
//!    clean Latin page, so it costs nothing there) are read again with the Cyrillic PP-OCRv5
//!    recognizer.
//!    When at least half of them read as confident Cyrillic, the page is Cyrillic and every
//!    line is read with both, keeping the Cyrillic reading unless the Latin one is clearly
//!    better; when only some do, lines below [`PaddleConfig::mixed_reread_below`] are read
//!    with both and the more confident Cyrillic reading wins; otherwise the page stays Latin.
//!
//! # Threads
//!
//! Every engine owns its ONNX Runtime sessions with [`PaddleConfig::intra_threads`] intra-op
//! threads and no spinning, and a rayon pool of the same number of threads (`quail-ocr-N`)
//! on which its whole page runs: `oar-ocr` resizes, normalizes and decodes on rayon, and
//! inside the engine's pool that work never reaches rayon's global pool (one thread per CPU,
//! shared with the host). A page alternates between the two (pre-processing, inference,
//! post-processing), so an engine keeps at most `intra_threads` threads busy, and
//! [`OcrPermits`](crate::OcrPermits) bound how many engines read at once: `jobs × threads`
//! is the whole OCR budget. The parallel steps are order-preserving maps (results collected
//! in input order, sums taken afterwards in order), so the output does not depend on the
//! thread count.

use std::path::{Path, PathBuf};

use image::{GrayImage, RgbImage, RgbaImage, imageops};
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::domain::tasks::{TextDetectionConfig, TextRecognitionConfig};
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::predictors::{DocumentOrientationPredictor, TextRecognitionPredictor};
use oar_ocr::processors::LimitType;
use quail_core::{Error, Image, Result};

use crate::engine::{
    EngineFactory, EngineId, OcrEngine, OcrHints, OcrLine, OcrPage, OcrWord, PxBox, ScriptHint,
};
use crate::models::{self, Role};
use crate::script::{ScriptCounts, script_of};

/// Settings of the PP-OCR backend. Everything here except `intra_threads` is part of the
/// engine fingerprint.
// Independent switches, not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub struct PaddleConfig {
    /// Longest side the detector sees (images are scaled down to it, never up).
    pub det_max_side: u32,
    /// Classify and correct page orientation.
    pub doc_orientation: bool,
    /// Minimum classifier score to rotate a page.
    pub orientation_min_score: f32,
    /// Classify and correct 180° text lines.
    pub line_orientation: bool,
    /// Use the Cyrillic recognizer.
    pub cyrillic: bool,
    /// Lines sampled to decide a page's script.
    pub script_sample: usize,
    /// Latin readings at or above this confidence are not re-read in Cyrillic (`Auto`).
    pub latin_sure: f32,
    /// On mixed pages, Latin readings below this confidence are tried in Cyrillic too.
    pub mixed_reread_below: f32,
    /// Deskew pages whose text is tilted by at least this many degrees (`None`: off).
    pub deskew_degrees: Option<f64>,
    /// Report word boxes.
    pub word_boxes: bool,
    /// ONNX Runtime intra-op threads per session, and the threads of the engine's rayon pool
    /// for pre- and post-processing (1 by default; [`OcrResources`](crate::OcrResources)
    /// picks it so that `jobs × threads` fits the CPUs).
    pub intra_threads: usize,
    /// Text lines recognized per batch. 1 by default: a batch is padded to its widest line,
    /// which costs both time and (kept) arena memory.
    pub region_batch: usize,
    /// ONNX Runtime memory patterns (pre-planned buffers per input shape). Off by default:
    /// page and line sizes vary, and the plans only grow memory.
    pub memory_pattern: bool,
}

impl Default for PaddleConfig {
    fn default() -> Self {
        Self {
            det_max_side: 1600,
            doc_orientation: true,
            orientation_min_score: 0.85,
            line_orientation: true,
            cyrillic: true,
            script_sample: 8,
            latin_sure: 0.95,
            mixed_reread_below: 0.90,
            deskew_degrees: None,
            word_boxes: true,
            intra_threads: 1,
            region_batch: 1,
            memory_pattern: false,
        }
    }
}

/// Makes [`PaddleEngine`]s from the models in one directory. [`PaddleFactory::new`] checks
/// every model's SHA-256 once.
#[derive(Debug)]
pub struct PaddleFactory {
    dir: PathBuf,
    cfg: PaddleConfig,
    id: EngineId,
}

impl PaddleFactory {
    /// Factory over the models in `dir` (see [`models::MANIFEST`]). Fails when a file is
    /// missing or its SHA-256 does not match.
    pub fn new(dir: &Path, cfg: PaddleConfig) -> Result<Self> {
        let missing = models::missing(dir);
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|m| m.name).collect();
            return Err(Error::Resource(format!(
                "missing in {}: {} (run the fetch_models example)",
                dir.display(),
                names.join(", ")
            )));
        }
        let mut fp = String::new();
        for m in models::MANIFEST {
            models::verify(dir, m)?;
            fp.push_str(m.sha256);
        }
        // Threads change speed, not output: not part of the fingerprint (the cache key).
        let same_output = PaddleConfig {
            intra_threads: 1,
            ..cfg.clone()
        };
        let _ = std::fmt::Write::write_fmt(&mut fp, format_args!("{same_output:?}"));
        let name = if cfg.cyrillic {
            "paddle/pp-ocrv6-small+cyrillic-v5"
        } else {
            "paddle/pp-ocrv6-small"
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            id: EngineId::new(name, fp.as_bytes()),
            cfg,
        })
    }

    /// Builds an engine (loads the models).
    pub fn engine(&self) -> Result<PaddleEngine> {
        PaddleEngine::load(&self.dir, &self.cfg, self.id.clone())
    }
}

impl EngineFactory for PaddleFactory {
    fn id(&self) -> EngineId {
        self.id.clone()
    }

    fn create(&self) -> Result<Box<dyn OcrEngine>> {
        Ok(Box::new(self.engine()?))
    }
}

fn ort(cfg: &PaddleConfig) -> OrtSessionConfig {
    OrtSessionConfig::new()
        .with_intra_threads(cfg.intra_threads.max(1))
        .with_inter_threads(1)
        .with_memory_pattern(cfg.memory_pattern)
        .add_config_entry("session.intra_op.allow_spinning", "0")
        .add_config_entry("session.inter_op.allow_spinning", "0")
}

fn oar_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("{what}: {e}"))
}

fn res_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Resource(format!("{what}: {e}"))
}

/// One PP-OCR pipeline: its own ONNX Runtime sessions and rayon pool, used by one thread at
/// a time.
pub struct PaddleEngine {
    id: EngineId,
    cfg: PaddleConfig,
    /// Where [`recognize`](OcrEngine::recognize) runs (see [Threads](self#threads)).
    threads: std::sync::Arc<rayon::ThreadPool>,
    ocr: OAROCR,
    orientation: Option<DocumentOrientationPredictor>,
    cyrillic: Option<TextRecognitionPredictor>,
}

impl std::fmt::Debug for PaddleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaddleEngine")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl PaddleEngine {
    fn load(dir: &Path, cfg: &PaddleConfig, id: EngineId) -> Result<Self> {
        let path = |r: Role| dir.join(models::file(r).name);
        let det = TextDetectionConfig {
            score_threshold: 0.2,
            box_threshold: 0.45,
            unclip_ratio: 1.4,
            max_candidates: 3000,
            limit_side_len: Some(cfg.det_max_side),
            limit_type: Some(LimitType::Max),
            max_side_len: Some(cfg.det_max_side.max(32)),
        };
        let mut b = OAROCRBuilder::new(
            path(Role::Detector),
            path(Role::Recognizer),
            path(Role::RecognizerDict),
        )
        .text_detection_config(det)
        .text_recognition_config(TextRecognitionConfig {
            score_threshold: 0.0,
        })
        .return_word_box(cfg.word_boxes)
        .ort_session(ort(cfg))
        .region_batch_size(cfg.region_batch.max(1))
        .deskew_threshold(cfg.deskew_degrees);
        if cfg.line_orientation {
            b = b.with_text_line_orientation_classification(path(Role::LineOrientation));
        }
        let ocr = b.build().map_err(|e| res_err("pp-ocrv6 pipeline", e))?;
        let orientation = if cfg.doc_orientation {
            Some(
                DocumentOrientationPredictor::builder()
                    .with_ort_config(ort(cfg))
                    .build(path(Role::DocOrientation))
                    .map_err(|e| res_err("doc orientation", e))?,
            )
        } else {
            None
        };
        let cyrillic = if cfg.cyrillic {
            Some(
                TextRecognitionPredictor::builder()
                    .score_threshold(0.0)
                    .dict_path(path(Role::CyrillicDict))
                    .with_ort_config(ort(cfg))
                    .build(path(Role::CyrillicRecognizer))
                    .map_err(|e| res_err("cyrillic recognizer", e))?,
            )
        } else {
            None
        };
        let threads = rayon::ThreadPoolBuilder::new()
            .num_threads(cfg.intra_threads.max(1))
            .thread_name(|i| format!("quail-ocr-{i}"))
            .build()
            .map_err(|e| res_err("ocr thread pool", e))?;
        Ok(Self {
            id,
            cfg: cfg.clone(),
            threads: std::sync::Arc::new(threads),
            ocr,
            orientation,
            cyrillic,
        })
    }

    /// Clockwise rotation of the page content (0, 90, 180, 270).
    fn orientation(&self, rgb: &RgbImage) -> Result<u16> {
        let Some(p) = &self.orientation else {
            return Ok(0);
        };
        let thumb = if rgb.width().max(rgb.height()) > 640 {
            imageops::thumbnail(
                rgb,
                rgb.width() * 640 / rgb.width().max(rgb.height()),
                rgb.height() * 640 / rgb.width().max(rgb.height()),
            )
        } else {
            rgb.clone()
        };
        let r = p
            .predict(vec![thumb])
            .map_err(|e| oar_err("doc orientation", e))?;
        let Some(top) = r.orientations.first().and_then(|c| c.first()) else {
            return Ok(0);
        };
        if top.score < self.cfg.orientation_min_score {
            return Ok(0);
        }
        Ok(match top.class_id {
            1 => 90,
            2 => 180,
            3 => 270,
            _ => 0,
        })
    }
}

fn to_rgb(img: &Image) -> Result<RgbImage> {
    let bad = || Error::Other(format!("image {img:?} does not match its buffer"));
    match img.channels {
        3 => RgbImage::from_raw(img.width, img.height, img.data.clone()).ok_or_else(bad),
        1 => {
            let g = GrayImage::from_raw(img.width, img.height, img.data.clone()).ok_or_else(bad)?;
            Ok(image::DynamicImage::ImageLuma8(g).to_rgb8())
        }
        4 => {
            let a = RgbaImage::from_raw(img.width, img.height, img.data.clone()).ok_or_else(bad)?;
            Ok(image::DynamicImage::ImageRgba8(a).to_rgb8())
        }
        n => Err(Error::Other(format!("{n} channels"))),
    }
}

/// Turns the content upright: content rotated clockwise by `rot` is rotated back.
fn upright(rgb: RgbImage, rot: u16) -> RgbImage {
    match rot {
        90 => imageops::rotate270(&rgb),
        180 => imageops::rotate180(&rgb),
        270 => imageops::rotate90(&rgb),
        _ => rgb,
    }
}

/// Maps a point of the upright image back to the input image (`w × h`) whose content was
/// rotated clockwise by `rot`.
#[allow(clippy::many_single_char_names)]
pub fn unrotate(p: (f32, f32), rot: u16, w: f32, h: f32) -> (f32, f32) {
    let (u, v) = p;
    match rot {
        // Upright = rotate270(input): input (x, y) -> (y, w - x).
        90 => (w - v, u),
        180 => (w - u, h - v),
        // Upright = rotate90(input): input (x, y) -> (h - y, x).
        270 => (v, h - u),
        _ => (u, v),
    }
}

/// A recognized region before routing.
struct Region {
    text: String,
    conf: f32,
    quad: [(f32, f32); 4],
    bbox: PxBox,
    flipped: bool,
    words: Vec<OcrWord>,
}

fn quad_of(points: &[oar_ocr::processors::Point]) -> [(f32, f32); 4] {
    let b = PxBox::around(points.iter().map(|p| (p.x, p.y)));
    if points.len() == 4 {
        [
            (points[0].x, points[0].y),
            (points[1].x, points[1].y),
            (points[2].x, points[2].y),
            (points[3].x, points[3].y),
        ]
    } else {
        [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)]
    }
}

/// Words from per-character boxes: consecutive non-space characters form a word.
fn words_from_chars(
    text: &str,
    boxes: &[oar_ocr::processors::BoundingBox],
    conf: f32,
) -> Vec<OcrWord> {
    let chars: Vec<char> = text.chars().collect();
    if boxes.len() != chars.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut pts: Vec<(f32, f32)> = Vec::new();
    for (c, b) in chars.iter().zip(boxes) {
        if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(OcrWord {
                    text: std::mem::take(&mut cur),
                    bbox: PxBox::around(pts.drain(..)),
                    conf,
                });
            }
            continue;
        }
        cur.push(*c);
        pts.extend(b.points.iter().map(|p| (p.x, p.y)));
    }
    if !cur.is_empty() {
        out.push(OcrWord {
            text: cur,
            bbox: PxBox::around(pts),
            conf,
        });
    }
    out
}

/// The upright crop of a region for re-recognition; `None` for vertical or degenerate ones.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn crop(img: &RgbImage, r: &Region) -> Option<RgbImage> {
    let b = r.bbox;
    if b.width() < 4.0 || b.height() < 4.0 || b.height() > 1.5 * b.width() {
        return None;
    }
    let pad = (b.height() * 0.08).max(1.0);
    let x0 = (b.x0 - pad).max(0.0) as u32;
    let y0 = (b.y0 - pad).max(0.0) as u32;
    let x1 = ((b.x1 + pad).ceil() as u32).min(img.width());
    let y1 = ((b.y1 + pad).ceil() as u32).min(img.height());
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return None;
    }
    let c = imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
    Some(if r.flipped {
        imageops::rotate180(&c)
    } else {
        c
    })
}

/// Which lines of a page to read with the Cyrillic recognizer, and how to choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Routing {
    /// Keep the Latin readings.
    Latin,
    /// Some lines are Cyrillic: re-read weak Latin lines.
    Mixed,
    /// The page is Cyrillic: re-read every line.
    Cyrillic,
}

/// Whether a Cyrillic reading should replace the Latin one.
fn prefer_cyrillic(route: Routing, lat: (&str, f32), cyr: (&str, f32)) -> bool {
    let has_cyr = ScriptCounts::of(cyr.0).cyrillic_share() >= 0.3;
    match route {
        Routing::Latin => false,
        // The Cyrillic dictionary has ASCII letters but not č ć š ž đ: on a Cyrillic page a
        // Latin line still keeps the Latin reading when that is more confident.
        Routing::Cyrillic => (has_cyr && cyr.1 >= lat.1 - 0.05) || cyr.1 > lat.1 + 0.02,
        Routing::Mixed => has_cyr && cyr.1 > lat.1 + 0.02,
    }
}

impl PaddleEngine {
    fn reread(
        &self,
        img: &RgbImage,
        regions: &[Region],
        idx: &[usize],
    ) -> Result<Vec<(usize, String, f32)>> {
        let Some(cyr) = &self.cyrillic else {
            return Ok(Vec::new());
        };
        let mut which = Vec::new();
        let mut crops = Vec::new();
        for &i in idx {
            if let Some(c) = regions.get(i).and_then(|r| crop(img, r)) {
                which.push(i);
                crops.push(c);
            }
        }
        if crops.is_empty() {
            return Ok(Vec::new());
        }
        // One line per call: batches are padded to their widest line, and ONNX Runtime keeps
        // the largest buffers it ever needed. The line-orientation classifier is trained on
        // the primary scripts and often calls Cyrillic lines upside down, so a flipped line
        // is read both ways and the more confident reading wins.
        let mut out = Vec::with_capacity(crops.len());
        let read = |c: RgbImage| -> Result<Option<(String, f32)>> {
            let r = cyr
                .predict(vec![c])
                .map_err(|e| oar_err("cyrillic recognizer", e))?;
            Ok(r.texts.into_iter().next().zip(r.scores.first().copied()))
        };
        for (i, c) in which.into_iter().zip(crops) {
            let flipped = regions.get(i).is_some_and(|r| r.flipped);
            let other = flipped.then(|| imageops::rotate180(&c));
            let mut best = read(c)?;
            if let Some(o) = other
                && let Some(b) = read(o)?
                && best.as_ref().is_none_or(|a| b.1 > a.1)
            {
                best = Some(b);
            }
            if let Some((t, s)) = best {
                out.push((i, t, s));
            }
        }
        Ok(out)
    }

    fn route(&self, img: &RgbImage, regions: &mut [Region], hint: ScriptHint) -> Result<()> {
        if self.cyrillic.is_none() || hint == ScriptHint::Latin || regions.is_empty() {
            return Ok(());
        }
        // Widest boxes first: the Latin text of a Cyrillic line may be empty.
        let mut by_len: Vec<usize> = (0..regions.len()).collect();
        by_len.sort_by(|&a, &b| regions[b].bbox.width().total_cmp(&regions[a].bbox.width()));
        // A confident Latin reading is never Cyrillic text in disguise: only the weaker lines
        // (unless the caller expects Cyrillic) are worth a second recognizer.
        let sample: Vec<usize> = by_len
            .iter()
            .copied()
            .filter(|&i| hint == ScriptHint::Cyrillic || regions[i].conf < self.cfg.latin_sure)
            .take(self.cfg.script_sample.max(1))
            .collect();
        if sample.is_empty() {
            return Ok(());
        }
        let first = self.reread(img, regions, &sample)?;
        let votes = first
            .iter()
            .filter(|(i, t, s)| {
                ScriptCounts::of(t).cyrillic_share() >= 0.5 && *s >= regions[*i].conf - 0.02
            })
            .count();
        let route = if hint == ScriptHint::Cyrillic || (votes > 0 && votes * 2 >= first.len()) {
            Routing::Cyrillic
        } else if votes > 0 {
            Routing::Mixed
        } else {
            Routing::Latin
        };
        if route == Routing::Latin {
            return Ok(());
        }
        let done: Vec<usize> = first.iter().map(|x| x.0).collect();
        let rest: Vec<usize> = (0..regions.len())
            .filter(|i| !done.contains(i))
            .filter(|&i| {
                route == Routing::Cyrillic || regions[i].conf < self.cfg.mixed_reread_below
            })
            .collect();
        let second = self.reread(img, regions, &rest)?;
        for (i, t, s) in first.into_iter().chain(second) {
            let r = &mut regions[i];
            if prefer_cyrillic(route, (&r.text, r.conf), (&t, s)) {
                // Character boxes belong to the Latin reading; keep the line box only.
                r.words.clear();
                if !t.trim().is_empty() {
                    r.words.extend(t.split_whitespace().map(|w| OcrWord {
                        text: w.to_owned(),
                        bbox: r.bbox,
                        conf: s,
                    }));
                }
                r.text = t;
                r.conf = s;
            }
        }
        Ok(())
    }
}

impl OcrEngine for PaddleEngine {
    fn id(&self) -> &EngineId {
        &self.id
    }

    fn recognize(&mut self, image: &Image, hints: &OcrHints) -> Result<OcrPage> {
        // Every rayon call of the page (here and in oar-ocr) goes to the engine's own pool.
        let threads = self.threads.clone();
        threads.install(|| self.read(image, *hints))
    }
}

impl PaddleEngine {
    fn read(&mut self, image: &Image, hints: OcrHints) -> Result<OcrPage> {
        crate::preprocess::validate(image)?;
        let rgb = to_rgb(image)?;
        let rot = self.orientation(&rgb)?;
        let up = upright(rgb, rot);
        let result = self
            .ocr
            .predict(vec![up])
            .map_err(|e| oar_err("pp-ocrv6", e))?;
        let Some(res) = result.into_iter().next() else {
            return Ok(OcrPage {
                width: image.width,
                height: image.height,
                rotation: rot,
                lines: Vec::new(),
            });
        };
        let up_img = res.input_img.clone();
        let mut regions: Vec<Region> = res
            .text_regions
            .into_iter()
            .map(|r| {
                // A detected line the Latin recognizer reads as nothing (it turns Cyrillic
                // into blanks) stays, with confidence 0, for the Cyrillic pass.
                let text = r.text.as_deref().unwrap_or("").trim().to_owned();
                let conf = if text.is_empty() {
                    0.0
                } else {
                    r.confidence.unwrap_or(0.0).clamp(0.0, 1.0)
                };
                let quad = quad_of(&r.bounding_box.points);
                let bbox = PxBox::around(quad);
                let words = r
                    .word_boxes
                    .as_deref()
                    .map(|wb| words_from_chars(&text, wb, conf))
                    .unwrap_or_default();
                Region {
                    text,
                    conf,
                    quad,
                    bbox,
                    flipped: r.orientation_angle.is_some_and(|a| (a - 180.0).abs() < 1.0),
                    words,
                }
            })
            .collect();
        self.route(&up_img, &mut regions, hints.script)?;
        regions.retain(|r| !r.text.trim().is_empty());
        #[allow(clippy::cast_precision_loss)]
        let (w, h) = (image.width as f32, image.height as f32);
        let back = |p: (f32, f32)| unrotate(p, rot, w, h);
        let lines = regions
            .into_iter()
            .map(|r| {
                let quad = r.quad.map(back);
                let map_box = |b: PxBox| PxBox::around([back((b.x0, b.y0)), back((b.x1, b.y1))]);
                OcrLine {
                    script: script_of(&r.text),
                    bbox: PxBox::around(quad),
                    quad,
                    conf: r.conf,
                    words: r
                        .words
                        .into_iter()
                        .map(|w| OcrWord {
                            bbox: map_box(w.bbox),
                            ..w
                        })
                        .collect(),
                    text: r.text,
                }
            })
            .collect();
        Ok(OcrPage {
            width: image.width,
            height: image.height,
            rotation: rot,
            lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrotate_inverts_upright() {
        // A 4x2 input with a marked pixel at (3, 0).
        let mut img = RgbImage::new(4, 2);
        img.put_pixel(3, 0, image::Rgb([255, 0, 0]));
        for rot in [0u16, 90, 180, 270] {
            let up = upright(img.clone(), rot);
            let (mut ux, mut uy) = (0, 0);
            for (x, y, p) in up.enumerate_pixels() {
                if p.0[0] == 255 {
                    (ux, uy) = (x, y);
                }
            }
            // Pixel centres map to pixel centres.
            #[allow(clippy::cast_precision_loss)]
            let (x, y) = unrotate((ux as f32 + 0.5, uy as f32 + 0.5), rot, 4.0, 2.0);
            assert!(
                (x - 3.5).abs() < 1e-4 && (y - 0.5).abs() < 1e-4,
                "rot {rot}: {x},{y}"
            );
        }
    }

    #[test]
    fn cyrillic_preference() {
        assert!(!prefer_cyrillic(Routing::Latin, ("x", 0.1), ("ж", 1.0)));
        assert!(prefer_cyrillic(
            Routing::Cyrillic,
            ("YroBop", 0.90),
            ("Уговор", 0.88)
        ));
        assert!(!prefer_cyrillic(
            Routing::Cyrillic,
            ("Ugovor", 0.99),
            ("Ugovor", 0.95)
        ));
        assert!(!prefer_cyrillic(
            Routing::Mixed,
            ("član", 0.95),
            ("clan", 0.96)
        ));
        assert!(prefer_cyrillic(
            Routing::Mixed,
            ("Yrobop", 0.6),
            ("Уговор", 0.95)
        ));
    }

    #[test]
    fn words_group_characters() {
        use oar_ocr::processors::{BoundingBox, Point};
        let bx = |x: f32| BoundingBox {
            points: vec![Point { x, y: 0.0 }, Point { x: x + 1.0, y: 2.0 }],
        };
        let w = words_from_chars("ab c", &[bx(0.0), bx(1.0), bx(2.0), bx(3.0)], 0.5);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].text, "ab");
        assert!((w[0].bbox.x1 - 2.0).abs() < 1e-6);
        assert_eq!(w[1].text, "c");
        assert!(words_from_chars("ab", &[bx(0.0)], 0.5).is_empty());
    }
}

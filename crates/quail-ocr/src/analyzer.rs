//! The `ocr.pages@1` analyzer.

use std::sync::Arc;

use quail_core::{
    Analyzer, AnalyzerId, Ctx, Document, Emitter, Error, Evidence, Image, Limits, Page, PageKind,
    PageRaster, Profile, Raster, Result, Value,
};

use crate::cache::{CacheKey, OcrCache};
use crate::engine::{EngineFactory, OcrHints, OcrPage, ScriptHint};
use crate::keys;
use crate::pool::EnginePool;
use crate::preprocess::{fit_dpi, shrink_to_fit, validate};
use crate::script::ScriptCounts;
use crate::text::{OcrPageText, OcrText, PixelSource, PxToPt, layout};

/// Knobs of [`OcrPages`].
#[derive(Clone, Debug, PartialEq)]
pub struct OcrConfig {
    /// DPI of the first render.
    pub dpi: f32,
    /// DPI of the second render when the first reads badly.
    pub retry_dpi: f32,
    /// Mean confidence below which a page is read again at `retry_dpi`.
    pub retry_below: f32,
    /// Lines with a lower confidence are left out of the text.
    pub min_line_conf: f32,
    /// Pages OCR'd per file; later ones are skipped with a problem.
    pub max_pages: u32,
    /// Embedded scans above this resolution are shrunk to it before OCR.
    pub max_scan_dpi: f32,
    /// Longest side of any image given to the engine.
    pub max_side: u32,
    /// Scripts to expect.
    pub script: ScriptHint,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            dpi: 200.0,
            retry_dpi: 300.0,
            retry_below: 0.80,
            min_line_conf: 0.5,
            max_pages: 200,
            max_scan_dpi: 300.0,
            max_side: 4000,
            script: ScriptHint::Auto,
        }
    }
}

enum Backend {
    Ready(EnginePool),
    Unavailable(String),
}

/// OCR of the pages that need it (`ocr.pages@1`, profile `Deep`, requires `document` and
/// `raster`, provides [`OcrText`]).
///
/// For each physical page with `needs_ocr`: takes the page's embedded scan when the page is
/// one ([`Page::is_scan`](quail_core::Page::is_scan)), else renders it at
/// [`OcrConfig::dpi`] (lowered to fit `Limits::max_render_pixels`); reads it with an engine
/// of the pool (results cached by pixels and engine); when the mean confidence is below
/// [`OcrConfig::retry_below`], renders again at [`OcrConfig::retry_dpi`] and keeps the better
/// reading. Boxes are mapped from pixels to page points.
///
/// Without a backend (crate built without `paddle`, or no models) it reports the problem
/// `ocr backend not compiled` (or why the models could not be used), and only for files that
/// have a page needing OCR.
pub struct OcrPages {
    backend: Backend,
    cache: Option<Arc<dyn OcrCache>>,
    config: OcrConfig,
}

impl OcrPages {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("ocr.pages", 1);

    /// Problem reported when the crate was built without a backend.
    pub const NOT_COMPILED: &'static str = "ocr backend not compiled";

    /// OCR with engines from `factory`, one per worker thread, keeping up to `max_idle`
    /// idle engines between files.
    pub fn new(factory: Arc<dyn EngineFactory>, max_idle: usize, config: OcrConfig) -> Self {
        Self {
            backend: Backend::Ready(EnginePool::new(factory, max_idle)),
            cache: None,
            config,
        }
    }

    /// An analyzer that cannot OCR: it reports `reason` as a problem for every file that has
    /// a page needing OCR.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            backend: Backend::Unavailable(reason.into()),
            cache: None,
            config: OcrConfig::default(),
        }
    }

    /// The best backend compiled in: `paddle` with the models of
    /// [`model_dir`](crate::models::model_dir) when the feature is on and the models are
    /// there, else [`unavailable`](Self::unavailable) with the reason. `workers` is the number
    /// of threads that will run files (engines kept idle).
    pub fn with_default_backend(workers: usize) -> Self {
        #[cfg(feature = "paddle")]
        {
            let Some(dir) = crate::models::model_dir() else {
                return Self::unavailable("ocr models: no model directory (set QUAIL_OCR_MODELS)");
            };
            match crate::paddle::PaddleFactory::new(&dir, crate::paddle::PaddleConfig::default()) {
                Ok(f) => Self::new(Arc::new(f), workers.max(1), OcrConfig::default()),
                Err(e) => Self::unavailable(format!("ocr models: {e}")),
            }
        }
        #[cfg(not(feature = "paddle"))]
        {
            let _ = workers;
            Self::unavailable(Self::NOT_COMPILED)
        }
    }

    /// Caches results in `cache` (shared by every file this analyzer sees).
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<dyn OcrCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Replaces the configuration.
    #[must_use]
    pub fn with_config(mut self, config: OcrConfig) -> Self {
        self.config = config;
        self
    }

    /// Whether a backend is ready.
    pub fn is_available(&self) -> bool {
        matches!(self.backend, Backend::Ready(_))
    }
}

/// Pages this analyzer would OCR.
pub fn pages_needing_ocr(doc: &Document) -> impl Iterator<Item = &Page> {
    doc.pages
        .iter()
        .filter(|p| p.needs_ocr && p.kind == PageKind::Physical)
}

impl Analyzer for OcrPages {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document", "raster"]
    }

    fn provides(&self) -> &'static [&'static str] {
        &["ocr_text"]
    }

    fn profile(&self) -> Profile {
        Profile::Deep
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let (Some(doc), Some(raster)) = (ctx.get::<Document>(), ctx.get::<Raster>()) else {
            return Ok(());
        };
        let todo: Vec<&Page> = pages_needing_ocr(doc).collect();
        if todo.is_empty() {
            return Ok(());
        }
        let pool = match &self.backend {
            Backend::Ready(p) => p,
            Backend::Unavailable(why) => {
                out.problem(why.clone());
                return Ok(());
            }
        };
        let native: u64 = doc.pages.iter().map(|p| p.text.len() as u64).sum();
        let mut budget =
            usize::try_from(ctx.limits.max_text_bytes.saturating_sub(native)).unwrap_or(usize::MAX);
        let run = Run {
            cfg: &self.config,
            pool,
            cache: self.cache.as_deref(),
            raster: &*raster.0,
            limits: ctx.limits,
        };
        let mut pages = Vec::new();
        let cap = usize::try_from(self.config.max_pages).unwrap_or(usize::MAX);
        if todo.len() > cap {
            out.problem(format!(
                "ocr: {} pages need OCR, only the first {cap} were read (max_pages)",
                todo.len()
            ));
        }
        for page in todo.into_iter().take(cap) {
            if out.check_deadline().is_err() {
                out.problem(format!("ocr: deadline reached after {} pages", pages.len()));
                break;
            }
            match run.page(page, budget, out) {
                Ok(t) => {
                    budget = budget.saturating_sub(t.text.len());
                    pages.push(t);
                }
                Err(Error::Timeout) => {
                    out.problem(format!("ocr: deadline reached on page {}", page.index));
                    break;
                }
                Err(e @ Error::Resource(_)) => {
                    out.problem(format!("ocr: engine unavailable: {e}"));
                    break;
                }
                Err(e) => out.problem(format!("ocr: page {} not read: {e}", page.index)),
            }
        }
        emit(&pages, out);
        out.provide(OcrText {
            engine: pool.id().short(),
            pages,
        });
        Ok(())
    }
}

fn emit(pages: &[OcrPageText], out: &mut Emitter) {
    let mut chars = 0u64;
    let mut sum = 0f64;
    let mut counts = ScriptCounts::default();
    for p in pages {
        chars += u64::from(p.chars);
        sum += f64::from(p.mean_conf.unwrap_or(0.0)) * f64::from(p.chars);
        counts.add(&p.text);
        out.push(
            keys::PAGE,
            Value::record([
                ("page", Value::Int(i64::from(p.index))),
                ("script", Value::text(p.script.as_str())),
                (
                    "mean_conf",
                    Value::Float(f64::from(p.mean_conf.unwrap_or(0.0))),
                ),
                ("chars", Value::Int(i64::from(p.chars))),
                (
                    "lines",
                    Value::Int(i64::try_from(p.lines.len()).unwrap_or(i64::MAX)),
                ),
                ("dpi", Value::Float(f64::from(p.dpi.round()))),
                ("source", Value::text(p.source.as_str())),
                ("rotation", Value::Int(i64::from(p.rotation))),
            ]),
            p.mean_conf.unwrap_or(0.0).clamp(0.0, 1.0),
            Some(Evidence {
                page: Some(p.index),
                ..Evidence::default()
            }),
        );
    }
    out.fact(
        keys::PAGES,
        Value::Int(i64::try_from(pages.len()).unwrap_or(i64::MAX)),
    );
    out.fact(
        keys::CHARS,
        Value::Int(i64::try_from(chars).unwrap_or(i64::MAX)),
    );
    if chars > 0 {
        #[allow(clippy::cast_precision_loss)]
        let mean = sum / chars as f64;
        out.fact(keys::MEAN_CONF, Value::Float((mean * 1e4).round() / 1e4));
        out.fact(keys::SCRIPT, Value::text(counts.script().as_str()));
    }
}

struct Run<'a> {
    cfg: &'a OcrConfig,
    pool: &'a EnginePool,
    cache: Option<&'a dyn OcrCache>,
    raster: &'a dyn PageRaster,
    limits: &'a Limits,
}

impl Run<'_> {
    fn page(&self, page: &Page, budget: usize, out: &mut Emitter) -> Result<OcrPageText> {
        let (img, source, dpi) = self.pixels(page, self.cfg.dpi, out)?;
        let first = self.recognize(&img, dpi)?;
        let mut best = (first, (img.width, img.height), source, dpi);
        let weak = best.0.mean_conf().is_some_and(|c| c < self.cfg.retry_below);
        if weak && dpi < self.cfg.retry_dpi * 0.9 && page.size.is_some() {
            out.check_deadline()?;
            match self.render(page, self.cfg.retry_dpi) {
                Ok((img2, dpi2)) if dpi2 > dpi * 1.1 => {
                    let second = self.recognize(&img2, dpi2)?;
                    if score(&second) > score(&best.0) {
                        best = (second, (img2.width, img2.height), PixelSource::Render, dpi2);
                    }
                }
                Ok(_) => {}
                Err(e) => out.warn(format!("ocr: page {} not re-rendered: {e}", page.index)),
            }
        }
        let (ocr, px, source, dpi) = best;
        let map = PxToPt::new(px, page.size, dpi);
        Ok(layout(
            page.index,
            &ocr,
            map,
            page.size.map(|s| s.1),
            self.cfg.min_line_conf,
            budget,
            dpi,
            source,
        ))
    }

    /// The image to read: the embedded scan when the page is one, else a render.
    fn pixels(
        &self,
        page: &Page,
        dpi: f32,
        out: &mut Emitter,
    ) -> Result<(Image, PixelSource, f32)> {
        if page.is_scan() {
            match self.raster.embedded_scan(page.index, self.limits) {
                Some(Ok(img)) if validate(&img).is_ok() => {
                    let eff = effective_dpi(&img, page.size).unwrap_or(dpi);
                    let max_px_dpi = self.limits.max_render_pixels;
                    let img = if eff > self.cfg.max_scan_dpi {
                        let side = scaled_side(&img, self.cfg.max_scan_dpi / eff);
                        shrink_to_fit(img, max_px_dpi, side.min(self.cfg.max_side))
                    } else {
                        shrink_to_fit(img, max_px_dpi, self.cfg.max_side)
                    };
                    let eff = effective_dpi(&img, page.size).unwrap_or(dpi);
                    return Ok((img, PixelSource::EmbeddedScan, eff));
                }
                Some(Ok(_)) => out.warn(format!("ocr: page {} scan is malformed", page.index)),
                Some(Err(e)) => out.warn(format!("ocr: page {} scan not decoded: {e}", page.index)),
                None => {}
            }
        }
        let (img, dpi) = self.render(page, dpi)?;
        Ok((img, PixelSource::Render, dpi))
    }

    fn render(&self, page: &Page, target: f32) -> Result<(Image, f32)> {
        let mut dpi = match page.size {
            Some(s) => fit_dpi(s, target, self.limits.max_render_pixels)
                .ok_or(Error::LimitExceeded("max_render_pixels"))?,
            None => target,
        };
        if let Some((w, h)) = page.size {
            // Keep the long side within `max_side` too.
            let long_in = w.max(h) / 72.0;
            #[allow(clippy::cast_precision_loss)]
            let side_dpi = self.cfg.max_side as f32 / long_in;
            if side_dpi.is_finite() && side_dpi > 0.0 {
                dpi = dpi.min(side_dpi * 0.995);
            }
        }
        let img = self.raster.render(page.index, dpi, self.limits)?;
        validate(&img)?;
        let img = shrink_to_fit(img, self.limits.max_render_pixels, self.cfg.max_side);
        let dpi = effective_dpi(&img, page.size).unwrap_or(dpi);
        Ok((img, dpi))
    }

    fn recognize(&self, img: &Image, dpi: f32) -> Result<OcrPage> {
        let hints = OcrHints {
            script: self.cfg.script,
            dpi: dpi.round(),
        };
        let key = self
            .cache
            .map(|_| CacheKey::new(self.pool.id(), &hints, img));
        if let (Some(c), Some(k)) = (self.cache, key.as_ref())
            && let Some(hit) = c.get(k)
        {
            return Ok((*hit).clone());
        }
        let page = self.pool.with(|e| e.recognize(img, &hints))??;
        if let (Some(c), Some(k)) = (self.cache, key) {
            c.put(k, Arc::new(page.clone()));
        }
        Ok(page)
    }
}

/// Mean confidence, then characters, to pick the better of two readings.
fn score(p: &OcrPage) -> (f32, usize) {
    (p.mean_conf().unwrap_or(0.0), p.chars())
}

/// Pixels per inch of `img` shown at `size_pt` (by width).
#[allow(clippy::cast_precision_loss)]
fn effective_dpi(img: &Image, size_pt: Option<(f32, f32)>) -> Option<f32> {
    let (w, _) = size_pt?;
    let d = img.width as f32 * 72.0 / w;
    (d.is_finite() && d > 0.0).then_some(d)
}

/// Long side of `img` scaled by `s`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn scaled_side(img: &Image, s: f32) -> u32 {
    ((img.width.max(img.height) as f32 * s).floor() as u32).max(1)
}

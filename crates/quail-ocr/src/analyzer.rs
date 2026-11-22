//! The `ocr.pages@2` analyzer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, AnalyzerId, Ctx, Document, Emitter, Error, Evidence, Image, Limits, Page, PageKind,
    PageRaster, Profile, Raster, Result, RevisedDocument, Value,
};

use crate::budget::{OcrPermits, OcrResources};
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
    /// Pages OCR'd per file; later ones are skipped with a problem (record `partial`).
    pub max_pages: u32,
    /// Reading time allowed per page that needs OCR (up to `max_pages`). A file's OCR stops
    /// with a problem once `pages × page_time` of reading is spent; time spent waiting for a
    /// permit ([`OcrPermits`]) is not counted. The file's own deadline
    /// ([`Emitter::deadline`]) always applies too. `None`: only the file's deadline.
    pub page_time: Option<Duration>,
    /// Embedded scans above this resolution are shrunk to it before OCR.
    pub max_scan_dpi: f32,
    /// Longest side of any image given to the engine.
    pub max_side: u32,
    /// Scripts to expect.
    pub script: ScriptHint,
}

impl OcrConfig {
    /// Default [`page_time`](Self::page_time): 20 s, about twice the measured 5 to 15 s per
    /// page of one engine on a laptop CPU, so a 300 DPI retry still fits.
    pub const PAGE_TIME: Duration = Duration::from_secs(20);

    /// Wall time to give a whole file so that its OCR (at most `max_pages` pages at
    /// `page_time` each) fits after `base` for everything else, with `queue` files ahead of
    /// it for the OCR permits (in a scan, `ceil(workers / jobs) - 1`). Without a
    /// `page_time`: `base`.
    pub fn file_timeout(&self, base: Duration, queue: u32) -> Duration {
        let Some(t) = self.page_time else {
            return base;
        };
        let ocr = t.saturating_mul(self.max_pages);
        base.saturating_add(ocr.saturating_mul(queue.saturating_add(1)))
    }
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            dpi: 200.0,
            retry_dpi: 300.0,
            retry_below: 0.80,
            min_line_conf: 0.5,
            max_pages: 40,
            page_time: Some(Self::PAGE_TIME),
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

/// OCR of the pages that need it (`ocr.pages@2`, profile `Deep`, requires `document` and
/// `raster`, provides [`OcrText`]).
///
/// For each physical page with `needs_ocr`: takes the page's embedded scan when the page is
/// one ([`Page::is_scan`](quail_core::Page::is_scan)), else renders it at
/// [`OcrConfig::dpi`] (lowered to fit `Limits::max_render_pixels`); reads it with an engine
/// of the pool (results cached by pixels and engine); when the mean confidence is below
/// [`OcrConfig::retry_below`], renders again at [`OcrConfig::retry_dpi`] and keeps the better
/// reading. Boxes are mapped from pixels to page points.
///
/// With [`OcrPermits`] (as built by [`with_resources`](Self::with_resources)), a page is
/// decoded, rendered and read only while its worker holds a permit, so at most `jobs` pages
/// are in work in the process however many workers run files. A file's OCR may take
/// [`OcrConfig::page_time`] per page plus the time it waited for permits, within the file's
/// deadline; pages past that or past [`OcrConfig::max_pages`] are left out with a problem.
///
/// Without a backend (crate built without `paddle`, or no models) it reports the problem
/// `ocr backend not compiled` (or why the models could not be used), and only for files that
/// have a page needing OCR.
pub struct OcrPages {
    backend: Backend,
    cache: Option<Arc<dyn OcrCache>>,
    config: OcrConfig,
    permits: Option<Arc<OcrPermits>>,
    resources: Option<OcrResources>,
}

impl OcrPages {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("ocr.pages", 2);

    /// Problem reported when the crate was built without a backend.
    pub const NOT_COMPILED: &'static str = "ocr backend not compiled";

    /// OCR with engines from `factory`, one per worker thread, keeping up to `max_idle`
    /// idle engines between files.
    pub fn new(factory: Arc<dyn EngineFactory>, max_idle: usize, config: OcrConfig) -> Self {
        Self {
            backend: Backend::Ready(EnginePool::new(factory, max_idle)),
            cache: None,
            config,
            permits: None,
            resources: None,
        }
    }

    /// An analyzer that cannot OCR: it reports `reason` as a problem for every file that has
    /// a page needing OCR.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            backend: Backend::Unavailable(reason.into()),
            cache: None,
            config: OcrConfig::default(),
            permits: None,
            resources: None,
        }
    }

    /// The best backend compiled in, sized for this machine:
    /// [`with_resources`](Self::with_resources) with [`OcrResources::auto`] for `workers`
    /// file workers and the default configuration.
    pub fn with_default_backend(workers: usize) -> Self {
        Self::with_resources(OcrResources::auto(workers), OcrConfig::default())
    }

    /// The best backend compiled in: `paddle` with the models of
    /// [`model_dir`](crate::models::model_dir) when the feature is on and the models are
    /// there, else [`unavailable`](Self::unavailable) with the reason. At most
    /// `resources.jobs` pages are read at once (shared [`OcrPermits`]), each by an engine with
    /// `resources.threads` ONNX Runtime intra-op threads.
    pub fn with_resources(resources: OcrResources, config: OcrConfig) -> Self {
        #[cfg(feature = "paddle")]
        {
            let Some(dir) = crate::models::model_dir() else {
                return Self::unavailable("ocr models: no model directory (set QUAIL_OCR_MODELS)");
            };
            let paddle = crate::paddle::PaddleConfig {
                intra_threads: resources.threads.max(1),
                ..crate::paddle::PaddleConfig::default()
            };
            match crate::paddle::PaddleFactory::new(&dir, paddle) {
                Ok(f) => {
                    let mut a = Self::new(Arc::new(f), resources.jobs.max(1), config)
                        .with_permits(Arc::new(OcrPermits::new(resources.jobs)));
                    a.resources = Some(resources);
                    a
                }
                Err(e) => Self::unavailable(format!("ocr models: {e}")),
            }
        }
        #[cfg(not(feature = "paddle"))]
        {
            let _ = (resources, config);
            Self::unavailable(Self::NOT_COMPILED)
        }
    }

    /// Reads at most `permits.total()` pages at once, sharing `permits` with whatever else
    /// holds them.
    #[must_use]
    pub fn with_permits(mut self, permits: Arc<OcrPermits>) -> Self {
        self.permits = Some(permits);
        self
    }

    /// The permits pages are read under, if any.
    pub fn permits(&self) -> Option<&Arc<OcrPermits>> {
        self.permits.as_ref()
    }

    /// Pages at once and threads per engine, when built by
    /// [`with_resources`](Self::with_resources) with a backend.
    pub fn resources(&self) -> Option<OcrResources> {
        self.resources
    }

    /// The configuration.
    pub fn config(&self) -> &OcrConfig {
        &self.config
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
        &["ocr_text", "revised_document"]
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
        let mut clock = Clock::new(&self.config, todo.len().min(cap), out.deadline());
        let ticket = self.permits.as_ref().map(|p| p.ticket());
        for page in todo.into_iter().take(cap) {
            if out.check_deadline().is_err() {
                out.problem(format!("ocr: deadline reached after {} pages", pages.len()));
                break;
            }
            if clock.spent() {
                out.problem(clock.spent_problem(pages.len()));
                break;
            }
            // Held while the page is decoded, rendered and read.
            let _permit = match (&self.permits, ticket) {
                (Some(p), Some(t)) => {
                    if let Ok((permit, waited)) = p.acquire(t, out.deadline()) {
                        clock.waited(waited);
                        Some(permit)
                    } else {
                        out.problem(format!(
                            "ocr: deadline reached waiting for an OCR slot after {} pages",
                            pages.len()
                        ));
                        break;
                    }
                }
                _ => None,
            };
            match run.page(page, budget, clock.deadline(), out) {
                Ok(t) => {
                    budget = budget.saturating_sub(t.text.len());
                    pages.push(t);
                }
                Err(Error::Timeout) if clock.spent() && out.check_deadline().is_ok() => {
                    out.problem(clock.spent_problem(pages.len()));
                    break;
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
        let ocr = OcrText {
            engine: pool.id().short(),
            pages,
        };
        if !ocr.pages.is_empty() {
            // Text consumers read it through `Ctx::document()` (they run `after` it).
            out.provide(RevisedDocument(ocr.apply_to(doc)));
        }
        out.provide(ocr);
        Ok(())
    }
}

/// The OCR time budget of one file: `pages × page_time` from the start, moved later by the
/// time spent waiting for permits, never past the file's deadline.
struct Clock {
    /// End of the budget; `None` without a `page_time`.
    until: Option<Instant>,
    file: Option<Instant>,
    allowed: Option<Duration>,
    pages: usize,
}

impl Clock {
    fn new(cfg: &OcrConfig, pages: usize, file: Option<Instant>) -> Self {
        let allowed = cfg
            .page_time
            .map(|t| t.saturating_mul(u32::try_from(pages).unwrap_or(u32::MAX)));
        Self {
            until: allowed.and_then(|a| Instant::now().checked_add(a)),
            file,
            allowed,
            pages,
        }
    }

    fn waited(&mut self, d: Duration) {
        self.until = self.until.and_then(|u| u.checked_add(d));
    }

    /// The earlier of the budget's end and the file's deadline.
    fn deadline(&self) -> Option<Instant> {
        match (self.until, self.file) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn spent(&self) -> bool {
        self.until.is_some_and(|u| Instant::now() >= u)
    }

    fn spent_problem(&self, read: usize) -> String {
        let secs = self.allowed.map_or(0, |a| a.as_secs());
        format!(
            "ocr: time budget of {secs} s for {} pages spent after {read} pages (page_time)",
            self.pages
        )
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
    fn page(
        &self,
        page: &Page,
        budget: usize,
        deadline: Option<Instant>,
        out: &mut Emitter,
    ) -> Result<OcrPageText> {
        let (img, source, dpi) = self.pixels(page, self.cfg.dpi, out)?;
        let first = self.recognize(&img, dpi)?;
        let mut best = (first, (img.width, img.height), source, dpi);
        let weak = best.0.mean_conf().is_some_and(|c| c < self.cfg.retry_below);
        if weak && dpi < self.cfg.retry_dpi * 0.9 && page.size.is_some() {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(Error::Timeout);
            }
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

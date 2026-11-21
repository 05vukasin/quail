//! `ocr.pages@1` end to end with a scripted engine and an in-memory raster: no models, no
//! native code, so it runs everywhere CI does.

#![allow(clippy::many_single_char_names)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, Artifacts, BBox, Blob, Ctx, Document, Emitter, Error, Format, Image, Limits, Page,
    PageImage, PageKind, PageRaster, Profile, Raster, Result, TextSource, Value,
};
use quail_ocr::{
    EngineFactory, EngineId, MemoryCache, OcrConfig, OcrEngine, OcrHints, OcrLine, OcrPage,
    OcrPages, OcrText, PixelSource, PxBox, Script, keys, script_of,
};

const A4: (f32, f32) = (595.0, 842.0);

/// Renders blank gray pages of the right size; scan pages also have an embedded 300 DPI
/// scan. Records every render DPI.
struct FakeRaster {
    renders: Mutex<Vec<(u32, f32)>>,
    scans: AtomicUsize,
}

impl FakeRaster {
    fn new() -> Self {
        Self {
            renders: Mutex::new(Vec::new()),
            scans: AtomicUsize::new(0),
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn blank(w: u32, h: u32, seed: u8) -> Image {
    let mut data = vec![255u8; (w * h) as usize];
    // Make pages of different index differ so the cache can tell them apart.
    data[0] = seed;
    Image {
        width: w,
        height: h,
        channels: 1,
        data,
    }
}

impl PageRaster for FakeRaster {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn render(&self, page: u32, dpi: f32, limits: &Limits) -> Result<Image> {
        let (w, h) = (
            (A4.0 * dpi / 72.0).ceil() as u32,
            (A4.1 * dpi / 72.0).ceil() as u32,
        );
        if u64::from(w) * u64::from(h) > limits.max_render_pixels {
            return Err(Error::LimitExceeded("max_render_pixels"));
        }
        self.renders.lock().map(|mut r| r.push((page, dpi))).ok();
        Ok(blank(w, h, u8::try_from(page % 200).unwrap_or(0)))
    }

    fn embedded_scan(&self, page: u32, _: &Limits) -> Option<Result<Image>> {
        if page != 0 {
            return None;
        }
        self.scans.fetch_add(1, Ordering::SeqCst);
        // 300 DPI A4.
        Some(Ok(blank(2480, 3508, 7)))
    }
}

/// Reads one line "Ugovor br. 7" at 10% / 10% of the image, confidence by DPI.
struct Scripted {
    id: EngineId,
    calls: Arc<AtomicUsize>,
    weak_below_dpi: f32,
}

impl OcrEngine for Scripted {
    fn id(&self) -> &EngineId {
        &self.id
    }

    #[allow(clippy::cast_precision_loss)]
    fn recognize(&mut self, image: &Image, hints: &OcrHints) -> Result<OcrPage> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (w, h) = (image.width as f32, image.height as f32);
        let bbox = PxBox {
            x0: 0.1 * w,
            y0: 0.1 * h,
            x1: 0.5 * w,
            y1: 0.12 * h,
        };
        let text = "Ugovor br. 7";
        Ok(OcrPage {
            width: image.width,
            height: image.height,
            rotation: 0,
            lines: vec![OcrLine {
                text: text.into(),
                quad: [
                    (bbox.x0, bbox.y0),
                    (bbox.x1, bbox.y0),
                    (bbox.x1, bbox.y1),
                    (bbox.x0, bbox.y1),
                ],
                bbox,
                conf: if hints.dpi < self.weak_below_dpi {
                    0.6
                } else {
                    0.95
                },
                script: script_of(text),
                words: vec![],
            }],
        })
    }
}

struct Factory {
    calls: Arc<AtomicUsize>,
    created: AtomicUsize,
    weak_below_dpi: f32,
    fail: bool,
}

impl Factory {
    fn new(weak_below_dpi: f32) -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(AtomicUsize::new(0)),
            created: AtomicUsize::new(0),
            weak_below_dpi,
            fail: false,
        })
    }
}

impl EngineFactory for Factory {
    fn id(&self) -> EngineId {
        EngineId::new("scripted", b"1")
    }

    fn create(&self) -> Result<Box<dyn OcrEngine>> {
        if self.fail {
            return Err(Error::Resource("no models".into()));
        }
        self.created.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Scripted {
            id: self.id(),
            calls: self.calls.clone(),
            weak_below_dpi: self.weak_below_dpi,
        }))
    }
}

/// Page 0 is a scan, page 1 needs OCR without a single image, page 2 has native text.
fn doc() -> Document {
    let scan = Page {
        size: Some(A4),
        needs_ocr: true,
        images: vec![PageImage {
            bbox: BBox {
                x0: 0.0,
                y0: 0.0,
                x1: A4.0,
                y1: A4.1,
            },
            pixels: (2480, 3508),
        }],
        image_coverage: 1.0,
        ..Page::new(0, PageKind::Physical)
    };
    let bad_layer = Page {
        size: Some(A4),
        needs_ocr: true,
        ..Page::new(1, PageKind::Physical)
    };
    let native = Page {
        size: Some(A4),
        text: "native text".into(),
        text_source: TextSource::Native,
        ..Page::new(2, PageKind::Physical)
    };
    Document {
        pages: vec![scan, bad_layer, native],
    }
}

struct Out {
    facts: Vec<quail_core::Fact>,
    artifacts: Artifacts,
    problems: Vec<String>,
}

fn run_with(
    a: &OcrPages,
    doc: Document,
    raster: Arc<FakeRaster>,
    limits: &Limits,
    deadline: Option<Instant>,
) -> Out {
    let blob = Blob::new(Vec::<u8>::new());
    let mut arts = Artifacts::new();
    arts.insert(doc);
    arts.insert(Raster(raster));
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://x.pdf",
        artifacts: &arts,
        facts: &[],
        limits,
        profile: Profile::Deep,
    };
    let mut out = match deadline {
        Some(d) => Emitter::with_deadline(OcrPages::ID, d),
        None => Emitter::new(OcrPages::ID),
    };
    a.run(&ctx, &mut out).unwrap_or_else(|e| panic!("run: {e}"));
    let problems = out.take_problems();
    let (facts, artifacts, _) = out.into_parts();
    Out {
        facts,
        artifacts,
        problems,
    }
}

fn run(a: &OcrPages, raster: Arc<FakeRaster>) -> Out {
    run_with(a, doc(), raster, &Limits::default(), None)
}

fn int(out: &Out, key: &quail_core::FactKey) -> Option<i64> {
    out.facts
        .iter()
        .find(|f| &f.key == key)
        .and_then(|f| match f.value {
            Value::Int(i) => Some(i),
            _ => None,
        })
}

#[test]
fn contract() {
    let a = OcrPages::unavailable("x");
    assert_eq!(a.id().to_string(), "ocr.pages@1");
    assert_eq!(a.requires(), &["document", "raster"]);
    assert_eq!(a.provides(), &["ocr_text", "revised_document"]);
    assert_eq!(a.profile(), Profile::Deep);
    assert!(!a.is_available());
}

#[test]
fn without_backend_problem_only_when_ocr_needed() {
    let a = OcrPages::unavailable(OcrPages::NOT_COMPILED);
    let out = run(&a, Arc::new(FakeRaster::new()));
    assert_eq!(out.problems, vec![OcrPages::NOT_COMPILED.to_owned()]);
    assert!(out.facts.is_empty());
    let native_only = Document {
        pages: vec![doc().pages.remove(2)],
    };
    let out = run_with(
        &a,
        native_only,
        Arc::new(FakeRaster::new()),
        &Limits::default(),
        None,
    );
    assert!(out.problems.is_empty());
    assert!(out.facts.is_empty());
}

#[cfg(not(feature = "paddle"))]
#[test]
fn default_backend_is_not_compiled() {
    let a = OcrPages::with_default_backend(2);
    assert!(!a.is_available());
    let out = run(&a, Arc::new(FakeRaster::new()));
    assert_eq!(out.problems, vec![OcrPages::NOT_COMPILED.to_owned()]);
}

#[test]
fn scan_uses_embedded_image_and_render_retries_at_300() {
    let f = Factory::new(250.0);
    let a = OcrPages::new(f.clone(), 2, OcrConfig::default());
    let raster = Arc::new(FakeRaster::new());
    let out = run(&a, raster.clone());
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(raster.scans.load(Ordering::SeqCst), 1);
    // Page 1 rendered at 200, weak, then again at 300. Page 0 (a 300 DPI scan) is never
    // rendered; page 2 has native text.
    let renders = raster.renders.lock().map(|r| r.clone()).unwrap_or_default();
    assert_eq!(renders.len(), 2, "{renders:?}");
    assert!(renders.iter().all(|r| r.0 == 1));
    assert!((renders[0].1 - 199.0).abs() < 1.5 && (renders[1].1 - 298.5).abs() < 1.5);

    let t = out
        .artifacts
        .get::<OcrText>()
        .unwrap_or_else(|| panic!("no ocr_text"));
    assert_eq!(t.pages.len(), 2);
    // The revised document carries the OCR text for consumers (`Ctx::document`).
    let revised = out
        .artifacts
        .get::<quail_core::RevisedDocument>()
        .unwrap_or_else(|| panic!("no revised_document"));
    assert!(
        revised
            .0
            .pages
            .iter()
            .any(|p| p.text.contains("Ugovor br. 7"))
    );
    let p0 = &t.pages[0];
    assert_eq!(p0.source, PixelSource::EmbeddedScan);
    assert!((p0.dpi - 300.0).abs() < 1.0);
    assert_eq!(p0.text, "Ugovor br. 7");
    assert_eq!(p0.script, Script::Latin);
    // 10% / 10% of the page, in points.
    let b = p0.blocks[0].bbox.unwrap_or(BBox {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    });
    assert!(
        (b.x0 - 59.5).abs() < 0.5 && (b.y0 - 84.2).abs() < 0.5,
        "{b:?}"
    );
    let p1 = &t.pages[1];
    assert_eq!(p1.source, PixelSource::Render);
    assert!(p1.dpi > 290.0);
    assert!((p1.mean_conf.unwrap_or(0.0) - 0.95).abs() < 1e-6);

    assert_eq!(int(&out, &keys::PAGES), Some(2));
    assert_eq!(int(&out, &keys::CHARS), Some(20));
    let pages: Vec<_> = out.facts.iter().filter(|f| f.key == keys::PAGE).collect();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].evidence.as_ref().and_then(|e| e.page), Some(0));
    assert_eq!(
        pages[1].value.field("source").and_then(Value::as_text),
        Some("render")
    );
    assert!(
        out.facts
            .iter()
            .any(|f| f.key == keys::SCRIPT && f.value.as_text() == Some("latin"))
    );
    assert!(out.facts.iter().any(|f| f.key == keys::MEAN_CONF));

    // The OCR text flows into a document for text analyzers; native text stays.
    let d = t.apply_to(&doc());
    assert_eq!(d.pages[0].text, "Ugovor br. 7");
    assert_eq!(d.pages[0].text_source, TextSource::Ocr);
    assert_eq!(d.pages[2].text, "native text");
}

#[test]
fn pixel_budget_lowers_dpi() {
    let f = Factory::new(0.0);
    let a = OcrPages::new(f, 1, OcrConfig::default());
    let raster = Arc::new(FakeRaster::new());
    let limits = Limits {
        max_render_pixels: 2_000_000,
        ..Limits::default()
    };
    let out = run_with(&a, doc(), raster.clone(), &limits, None);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let renders = raster.renders.lock().map(|r| r.clone()).unwrap_or_default();
    assert_eq!(renders.len(), 1);
    assert!(renders[0].1 < 150.0 && renders[0].1 > 140.0, "{renders:?}");
    // The 8.7 MP scan was shrunk too.
    let t = out
        .artifacts
        .get::<OcrText>()
        .unwrap_or_else(|| panic!("no ocr_text"));
    assert!(t.pages[0].dpi < 150.0);
}

#[test]
fn cache_skips_known_pixels() {
    let f = Factory::new(0.0);
    let cache = Arc::new(MemoryCache::default());
    let a = OcrPages::new(f.clone(), 1, OcrConfig::default()).with_cache(cache.clone());
    let first = run(&a, Arc::new(FakeRaster::new()));
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    let second = run(&a, Arc::new(FakeRaster::new()));
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        2,
        "second file fully cached"
    );
    assert_eq!(cache.len(), 2);
    assert_eq!(first.facts, second.facts);
    // Engines are made once and reused.
    assert_eq!(f.created.load(Ordering::SeqCst), 1);
}

#[test]
fn deadline_and_page_cap_give_partial_results() {
    let f = Factory::new(0.0);
    let a = OcrPages::new(f.clone(), 1, OcrConfig::default());
    let past = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let out = run_with(
        &a,
        doc(),
        Arc::new(FakeRaster::new()),
        &Limits::default(),
        Some(past),
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    assert!(
        out.problems.iter().any(|p| p.contains("deadline")),
        "{:?}",
        out.problems
    );
    assert_eq!(int(&out, &keys::PAGES), Some(0));
    assert!(out.artifacts.get::<OcrText>().is_some());

    let capped = OcrPages::new(
        f,
        1,
        OcrConfig {
            max_pages: 1,
            ..OcrConfig::default()
        },
    );
    let out = run(&capped, Arc::new(FakeRaster::new()));
    assert_eq!(int(&out, &keys::PAGES), Some(1));
    assert!(out.problems.iter().any(|p| p.contains("max_pages")));
}

#[test]
fn engine_failure_is_a_problem_not_an_error() {
    let f = Arc::new(Factory {
        calls: Arc::new(AtomicUsize::new(0)),
        created: AtomicUsize::new(0),
        weak_below_dpi: 0.0,
        fail: true,
    });
    let a = OcrPages::new(f, 1, OcrConfig::default());
    let out = run(&a, Arc::new(FakeRaster::new()));
    assert_eq!(out.problems.len(), 1);
    assert!(out.problems[0].contains("engine unavailable"));
}

#[test]
fn text_budget_is_shared_with_native_text() {
    let f = Factory::new(0.0);
    let a = OcrPages::new(f, 1, OcrConfig::default());
    let limits = Limits {
        // "native text" (11 bytes) + one OCR line (12 bytes) fits, a second does not.
        max_text_bytes: 11 + 12 + 5,
        ..Limits::default()
    };
    let out = run_with(&a, doc(), Arc::new(FakeRaster::new()), &limits, None);
    let t = out
        .artifacts
        .get::<OcrText>()
        .unwrap_or_else(|| panic!("no ocr_text"));
    let total: usize = t.pages.iter().map(|p| p.text.len()).sum();
    assert_eq!(total, 12);
}

#[test]
fn ocr_text_serializes() {
    let f = Factory::new(0.0);
    let a = OcrPages::new(f, 1, OcrConfig::default());
    let out = run(&a, Arc::new(FakeRaster::new()));
    let t = out
        .artifacts
        .get::<OcrText>()
        .unwrap_or_else(|| panic!("no ocr_text"));
    let json = serde_json::to_string(t).unwrap_or_default();
    let back: OcrText = serde_json::from_str(&json).unwrap_or_default();
    assert_eq!(&back, t);
}

#[test]
fn hostile_raster_output_is_rejected() {
    struct Liar;
    impl PageRaster for Liar {
        fn render(&self, _: u32, _: f32, _: &Limits) -> Result<Image> {
            Ok(Image {
                width: 100,
                height: 100,
                channels: 1,
                data: vec![0; 10],
            })
        }
        fn embedded_scan(&self, _: u32, _: &Limits) -> Option<Result<Image>> {
            Some(Ok(Image {
                width: 0,
                height: 5,
                channels: 9,
                data: vec![],
            }))
        }
    }
    let f = Factory::new(0.0);
    let a = OcrPages::new(f.clone(), 1, OcrConfig::default());
    let blob = Blob::new(Vec::<u8>::new());
    let mut arts = Artifacts::new();
    arts.insert(doc());
    arts.insert(Raster(Arc::new(Liar)));
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://x.pdf",
        artifacts: &arts,
        facts: &[],
        limits: &limits,
        profile: Profile::Deep,
    };
    let mut out = Emitter::new(OcrPages::ID);
    assert!(a.run(&ctx, &mut out).is_ok());
    assert_eq!(out.take_problems().len(), 2);
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

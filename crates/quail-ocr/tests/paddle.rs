//! The PP-OCR backend on generated pages (feature `paddle`): the output does not depend on the
//! thread count or on an engine being dropped after its idle timeout and loaded again, and the
//! engines never start rayon's global pool. Needs the models in `$QUAIL_OCR_MODELS` (CI
//! fetches and caches them); without them every test here skips.
//!
//! `measure` (ignored) prints what one setting gives on this machine: pages/s, cores busy,
//! threads that did work, and the RSS after a burst and after the idle timeout. One run per
//! setting, in the foreground:
//!
//! ```sh
//! QUAIL_OCR_MODELS=/path/to/models OCR_JOBS=1 OCR_THREADS=4 OCR_IDLE_MS=5000 \
//!   cargo test -p quail-ocr --features paddle --test paddle -- --ignored --nocapture measure
//! ```
#![cfg(feature = "paddle")]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use quail_core::{Blob, Image, Limits, PageRaster};
use quail_ocr::paddle::{PaddleConfig, PaddleFactory};
use quail_ocr::{EnginePool, OcrHints, OcrPage, ScriptHint};
use quail_pdf::PdfRaster;

/// US Letter in points.
const W: f32 = 612.0;
const H: f32 = 792.0;

fn models() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("QUAIL_OCR_MODELS")?);
    if quail_ocr::models::missing(&dir).is_empty() {
        Some(dir)
    } else {
        eprintln!("OCR models not in {}; skipping", dir.display());
        None
    }
}

/// Invented invoice text: a header and 24 lines of words and amounts (about 1500 characters).
fn page_lines(seed: u64) -> Vec<String> {
    const WORDS: &[&str] = &[
        "delivery",
        "of",
        "office",
        "supplies",
        "and",
        "paper",
        "for",
        "the",
        "second",
        "quarter",
        "service",
        "fee",
        "maintenance",
        "contract",
        "per",
        "unit",
        "transport",
        "costs",
        "included",
        "discount",
        "net",
        "amount",
        "payable",
        "within",
        "days",
        "printer",
        "toner",
        "cartridges",
        "boxes",
        "folders",
        "storage",
        "labels",
    ];
    let mut x = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut lines = vec![
        format!("INVOICE No. 2026-{:04}", next() % 10_000),
        format!(
            "Date: 2026-{:02}-{:02}   Customer ID: {:06}",
            next() % 12 + 1,
            next() % 28 + 1,
            next() % 1_000_000
        ),
        String::new(),
    ];
    for i in 0..24 {
        let mut line = String::new();
        while line.len() < 52 {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(WORDS[(next() % WORDS.len() as u64) as usize]);
        }
        if i % 2 == 0 {
            let (units, cents) = (next() % 9000 + 10, next() % 100);
            line = format!("{line}  {units}.{cents:02} EUR");
        }
        lines.push(line);
    }
    lines
}

/// A one-page text PDF (Helvetica 11 pt).
fn text_pdf(lines: &[String]) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let mut ops = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let top = 72.0 + 16.0 * i as f32;
        ops.push(Operation::new("BT", vec![]));
        ops.push(Operation::new("Tf", vec!["F1".into(), 11.into()]));
        ops.push(Operation::new("Td", vec![72.into(), (H - top).into()]));
        ops.push(Operation::new(
            "Tj",
            vec![Object::string_literal(l.as_bytes().to_vec())],
        ));
        ops.push(Operation::new("ET", vec![]));
    }
    let content = Content { operations: ops };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
            "MediaBox" => vec![0.into(), 0.into(), W.into(), H.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// The page of `seed` rendered at `dpi` (8-bit gray), as a scan would be.
fn page(seed: u64, dpi: f32) -> Image {
    PdfRaster::new(Blob::from(text_pdf(&page_lines(seed))))
        .render(0, dpi, &Limits::default())
        .expect("render the text page")
}

fn hints(dpi: f32) -> OcrHints {
    OcrHints {
        script: ScriptHint::Auto,
        dpi,
    }
}

fn factory(dir: &std::path::Path, threads: usize) -> Arc<PaddleFactory> {
    let cfg = PaddleConfig {
        intra_threads: threads,
        ..PaddleConfig::default()
    };
    Arc::new(PaddleFactory::new(dir, cfg).unwrap())
}

/// Text, confidences and boxes of a page, as JSON.
fn json(p: &OcrPage) -> String {
    serde_json::to_string(p).unwrap()
}

fn read(pool: &EnginePool, img: &Image, dpi: f32) -> OcrPage {
    pool.with(|e| e.recognize(img, &hints(dpi)))
        .unwrap()
        .unwrap()
}

#[test]
fn output_is_the_same_for_every_thread_count_and_after_an_idle_reload() {
    let Some(dir) = models() else { return };
    let dpi = 200.0;
    let pages: Vec<Image> = (1..=2).map(|s| page(s, dpi)).collect();
    let mut reference: Option<Vec<String>> = None;
    for threads in [1, 2, 4] {
        let pool = EnginePool::new(factory(&dir, threads), 1);
        let got: Vec<String> = pages.iter().map(|p| json(&read(&pool, p, dpi))).collect();
        match &reference {
            None => {
                let first: OcrPage = serde_json::from_str(&got[0]).unwrap();
                let text: String = first.lines.iter().map(|l| l.text.as_str()).collect();
                assert!(text.contains("INVOICE"), "page not read: {text}");
                reference = Some(got);
            }
            Some(r) => assert_eq!(&got, r, "output differs with {threads} thread(s)"),
        }
    }
    // An engine dropped after its idle timeout is loaded again on the next page, and reads it
    // the same.
    let pool =
        EnginePool::new(factory(&dir, 2), 1).with_idle_timeout(Some(Duration::from_millis(200)));
    let before = json(&read(&pool, &pages[0], dpi));
    let until = Instant::now() + Duration::from_secs(30);
    while pool.idle() > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(pool.idle(), 0, "idle engine not dropped");
    let after = json(&read(&pool, &pages[0], dpi));
    assert_eq!(before, after);
    assert_eq!(Some(&before), reference.as_ref().map(|r| &r[0]));
}

#[test]
fn engines_never_start_the_global_rayon_pool() {
    let Some(dir) = models() else { return };
    let pool = EnginePool::new(factory(&dir, 2), 1);
    let p = read(&pool, &page(3, 200.0), 200.0);
    assert!(!p.lines.is_empty());
    // Succeeds only while nothing in the process has used rayon's global pool.
    assert!(
        rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build_global()
            .is_ok(),
        "OCR ran on rayon's global pool"
    );
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}

/// `utime + stime` of a `/proc/.../stat` line, in clock ticks.
fn ticks(stat: &str) -> u64 {
    let rest = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = rest.split_whitespace().collect();
    f.get(11).and_then(|s| s.parse().ok()).unwrap_or(0)
        + f.get(12).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn process_ticks() -> u64 {
    ticks(&std::fs::read_to_string("/proc/self/stat").unwrap_or_default())
}

/// CPU ticks of every thread of the process, by thread id.
fn thread_ticks() -> std::collections::HashMap<String, u64> {
    let mut m = std::collections::HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if let Ok(s) = std::fs::read_to_string(e.path().join("stat")) {
                m.insert(id, ticks(&s));
            }
        }
    }
    m
}

/// A `kB` field of `/proc/self/status` in MiB.
fn status_mib(field: &str) -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Pages/s, cores busy, threads that did work and RSS for `OCR_JOBS` engines (default 1) with
/// `OCR_THREADS` threads each (default 4) reading `OCR_PAGES` warm pages (default 12) side by
/// side, then the RSS after `OCR_IDLE_MS` (default: no idle timeout) and 3 s more.
#[test]
#[ignore = "measurement: needs the models and minutes of CPU"]
fn measure() {
    let Some(dir) = models() else { return };
    let jobs = env_usize("OCR_JOBS").unwrap_or(1);
    let threads = env_usize("OCR_THREADS").unwrap_or(4);
    let n = env_usize("OCR_PAGES").unwrap_or(12);
    let idle = env_usize("OCR_IDLE_MS").map(|ms| Duration::from_millis(ms as u64));
    let dpi = 300.0;
    let pages: Vec<Image> = (0..n as u64).map(|s| page(100 + s, dpi)).collect();
    let warm: Vec<Image> = (0..jobs as u64).map(|s| page(10 + s, dpi)).collect();
    let rss_start = status_mib("VmRSS:");
    let pool = Arc::new(make_pool(factory(&dir, threads), jobs, idle));
    // One cold page per engine, at the same time, so every engine is made.
    std::thread::scope(|s| {
        for w in &warm {
            let pool = &pool;
            s.spawn(move || read(pool, w, dpi));
        }
    });
    let next = AtomicUsize::new(0);
    let (t0, c0, th0) = (Instant::now(), process_ticks(), thread_ticks());
    let outputs: Vec<(usize, String)> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..jobs)
            .map(|_| {
                let (pool, next, pages) = (&pool, &next, &pages);
                s.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(p) = pages.get(i) else { break };
                        out.push((i, json(&read(pool, p, dpi))));
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let wall = t0.elapsed().as_secs_f64();
    let cpu = (process_ticks() - c0) as f64 / 100.0;
    let busy = thread_ticks()
        .iter()
        .filter(|(id, t)| {
            let d = **t - th0.get(*id).copied().unwrap_or(0);
            d as f64 / 100.0 >= 0.05 * wall
        })
        .count();
    let rss_burst = status_mib("VmRSS:");
    let mut sorted = outputs;
    sorted.sort();
    let digest = blake3::hash(
        sorted
            .iter()
            .map(|(_, j)| j.as_str())
            .collect::<String>()
            .as_bytes(),
    );
    println!(
        "jobs {jobs} x threads {threads}, {n} warm pages at {dpi} DPI: {:.2} pages/s, {:.2} CPU s a page, \
         {:.2} cores busy, {busy} threads did work; RSS start {rss_start:.0} MiB, after the burst \
         {rss_burst:.0} MiB, peak {:.0} MiB; output {}",
        n as f64 / wall,
        cpu / n as f64,
        cpu / wall,
        status_mib("VmHWM:"),
        &digest.to_hex()[..16],
    );
    let wait = idle.unwrap_or_default() + Duration::from_secs(3);
    std::thread::sleep(wait);
    println!(
        "after {:.1} s idle (timeout {idle:?}): {} idle engine(s), RSS {:.0} MiB",
        wait.as_secs_f64(),
        pool.idle(),
        status_mib("VmRSS:"),
    );
}

/// The pool `measure` reads with.
fn make_pool(f: Arc<PaddleFactory>, jobs: usize, idle: Option<Duration>) -> EnginePool {
    EnginePool::new(f, jobs).with_idle_timeout(idle)
}

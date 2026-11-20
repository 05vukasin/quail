//! Runs the extractor over every `.pdf` under a directory and prints aggregate numbers only
//! (no file names, no text), so it is safe on private corpora:
//!
//! ```text
//! cargo run -p quail-pdf --example corpus --profile profiling -- <dir|list.txt> [threads] [dump_dir]
//! ```
//!
//! With a list file (one path per line) and `dump_dir`, page texts are written to
//! `dump_dir/<line index>.txt` (pages joined by form feeds) for comparison with other tools.
#![allow(missing_docs, clippy::cast_precision_loss, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use quail_core::{Blob, Emitter, Error, Extractor, Limits, Raster, TextSource};
use quail_pdf::PdfExtractor;

#[derive(Default)]
struct Agg {
    files: usize,
    bytes: u64,
    ok: usize,
    errors: BTreeMap<String, usize>,
    panics: usize,
    pages: usize,
    pages_native: usize,
    pages_ocr: usize,
    files_with_text: usize,
    files_needing_ocr: usize,
    files_with_warnings: usize,
    files_partial: usize,
    scan_pages: usize,
    scans_decoded: usize,
    extract_ms: f64,
    scan_ms: f64,
    slowest_ms: f64,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            out.push(p);
        }
    }
}

fn kind(e: &Error) -> String {
    match e {
        Error::Encrypted => "encrypted".into(),
        Error::Malformed { .. } => "malformed".into(),
        Error::Unsupported(_) => "unsupported".into(),
        Error::LimitExceeded(l) => format!("limit:{l}"),
        Error::Timeout => "timeout".into(),
        Error::Panic(_) => "panic".into(),
        _ => "other".into(),
    }
}

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb / 1024)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: corpus <dir> [threads]");
        return;
    };
    let threads: usize = args.next().and_then(|t| t.parse().ok()).unwrap_or(1);
    let dump: Option<PathBuf> = args.next().map(PathBuf::from);
    let mut files = Vec::new();
    if Path::new(&dir).is_file() {
        files = std::fs::read_to_string(&dir)
            .unwrap_or_default()
            .lines()
            .map(PathBuf::from)
            .collect();
    } else {
        walk(Path::new(&dir), &mut files);
        files.sort();
    }
    let queue = Mutex::new(files.into_iter().enumerate());
    let agg = Mutex::new(Agg::default());
    let limits = Limits::default();
    let extractor = PdfExtractor::new();
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let Some((index, path)) = queue.lock().ok().and_then(|mut q| q.next()) else {
                        return;
                    };
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    let len = bytes.len() as u64;
                    let blob = Blob::from(bytes);
                    let deadline = Instant::now() + limits.timeout;
                    let mut out = Emitter::with_deadline(PdfExtractor::ID, deadline);
                    let t = Instant::now();
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        extractor.extract(&blob, &limits, &mut out)
                    }));
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    let problems = out.take_problems();
                    let (_, artifacts, warnings) = out.into_parts();
                    // Decode embedded scans of scan pages through the core trait, as OCR will.
                    let mut decoded = 0usize;
                    let ts = Instant::now();
                    if let (Ok(Ok(doc)), Some(raster)) = (&r, artifacts.get::<Raster>()) {
                        for p in doc.pages.iter().filter(|p| p.is_scan()) {
                            if let Some(Ok(_)) = raster.0.embedded_scan(p.index, &limits) {
                                decoded += 1;
                            }
                        }
                    }
                    let Ok(mut a) = agg.lock() else { return };
                    a.files += 1;
                    a.bytes += len;
                    a.slowest_ms = a.slowest_ms.max(ms);
                    if !warnings.is_empty() {
                        a.files_with_warnings += 1;
                    }
                    a.files_partial += usize::from(!problems.is_empty());
                    a.scans_decoded += decoded;
                    a.extract_ms += ms;
                    a.scan_ms += ts.elapsed().as_secs_f64() * 1e3;
                    match r {
                        Ok(Ok(doc)) => {
                            if let Some(d) = &dump {
                                let _ =
                                    std::fs::write(d.join(format!("{index}.txt")), doc.full_text());
                            }
                            a.ok += 1;
                            a.pages += doc.pages.len();
                            let native = doc
                                .pages
                                .iter()
                                .filter(|p| p.text_source == TextSource::Native)
                                .count();
                            let ocr = doc.pages.iter().filter(|p| p.needs_ocr).count();
                            a.pages_native += native;
                            a.pages_ocr += ocr;
                            a.scan_pages += doc.pages.iter().filter(|p| p.is_scan()).count();
                            a.files_with_text += usize::from(native > 0);
                            a.files_needing_ocr += usize::from(ocr > 0);
                        }
                        Ok(Err(e)) => *a.errors.entry(kind(&e)).or_default() += 1,
                        Err(_) => a.panics += 1,
                    }
                }
            });
        }
    });
    let dt = t0.elapsed().as_secs_f64();
    let Ok(a) = agg.into_inner() else { return };
    let pct = |n: usize| 100.0 * n as f64 / a.files.max(1) as f64;
    println!(
        "files={} MB={:.1} threads={threads} secs={dt:.2} files/s={:.1} pages/s={:.0} slowest_ms={:.0} peak_rss_mb={}",
        a.files,
        a.bytes as f64 / 1e6,
        a.files as f64 / dt,
        a.pages as f64 / dt,
        a.slowest_ms,
        peak_rss_mb()
    );
    println!(
        "ok={} errors={:?} escaped_panics={} warnings_files={} partial_files={}",
        a.ok, a.errors, a.panics, a.files_with_warnings, a.files_partial
    );
    println!(
        "extract_cpu_s={:.2} (files/s per thread={:.1}) scan_pages={} embedded_scans_decoded={} scan_decode_cpu_s={:.2}",
        a.extract_ms / 1e3,
        a.files as f64 / (a.extract_ms / 1e3).max(1e-9),
        a.scan_pages,
        a.scans_decoded,
        a.scan_ms / 1e3
    );
    println!(
        "pages={} native_pages={} ocr_pages={} files_with_text={:.1}% files_needing_ocr={:.1}%",
        a.pages,
        a.pages_native,
        a.pages_ocr,
        pct(a.files_with_text),
        pct(a.files_needing_ocr)
    );
}

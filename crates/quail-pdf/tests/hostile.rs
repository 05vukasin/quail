//! Regression tests for hostile PDFs that used to kill the process (allocation failure aborts;
//! `catch_unwind` cannot help) or hold a worker forever. Every input is generated here; each
//! run is measured with a thread-local allocation counter and must stay under a memory bound.
//!
//! The counting allocator is the only `unsafe` in the crate: it is test-only and forwards to
//! the system allocator unchanged.
#![allow(
    unsafe_code,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::missing_panics_doc
)]

mod repro;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Write as _;
use std::time::{Duration, Instant};

use lopdf::dictionary;
use quail_core::{
    Blob, Document, Emitter, Error, Extractor, Fact, FactKey, Limits, PageRaster, Raster, Value,
};
use quail_pdf::{PdfExtractor, PdfRaster, keys};

// ---------------------------------------------------------------------------------------------
// Peak-allocation measurement.

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: every method forwards to `System` with the caller's arguments unchanged; the only
// extra work is updating const-initialized thread-local counters, which never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `GlobalAlloc::alloc`'s contract.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            track(layout.size().cast_signed());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds the contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            track(layout.size().cast_signed());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-layout.size().cast_signed());
        // SAFETY: `ptr` was allocated by `System` with this layout (all allocations go
        // through this type).
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `GlobalAlloc::realloc`'s contract.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            track(new_size.cast_signed() - layout.size().cast_signed());
        }
        p
    }
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|l| {
        let now = l.get() + delta;
        l.set(now);
        let _ = PEAK.try_with(|p| {
            if now > p.get() {
                p.set(now);
            }
        });
    });
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f` and returns its result with the peak extra bytes this thread held meanwhile.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(base));
    let r = f();
    let peak = PEAK.with(Cell::get) - base;
    (r, usize::try_from(peak).unwrap_or(0))
}

const MIB: usize = 1 << 20;

// ---------------------------------------------------------------------------------------------
// A raw PDF writer, so that tests control every byte (filters, object streams, broken trees).

fn build(objs: &[(u32, Vec<u8>)], root: u32, with_xref: bool) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offs = HashMap::new();
    let mut sorted: Vec<&(u32, Vec<u8>)> = objs.iter().collect();
    sorted.sort_by_key(|o| o.0);
    for (n, body) in sorted {
        offs.insert(*n, out.len());
        writeln!(out, "{n} 0 obj").unwrap();
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let size = objs.iter().map(|o| o.0).max().unwrap_or(0) + 1;
    if with_xref {
        let x = out.len();
        write!(out, "xref\n0 {size}\n0000000000 65535 f \n").unwrap();
        for n in 1..size {
            match offs.get(&n) {
                Some(o) => writeln!(out, "{o:010} 00000 n ").unwrap(),
                None => out.extend_from_slice(b"0000000000 65535 f \n"),
            }
        }
        write!(
            out,
            "trailer\n<< /Size {size} /Root {root} 0 R >>\nstartxref\n{x}\n%%EOF\n"
        )
        .unwrap();
    } else {
        write!(out, "trailer\n<< /Root {root} 0 R >>\n%%EOF\n").unwrap();
    }
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 9)
}

/// A stream object with `dict` entries and already-encoded `body`.
fn stream_raw(dict: &str, body: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

fn stream_flate(dict: &str, data: &[u8]) -> Vec<u8> {
    stream_raw(&format!("/Filter /FlateDecode {dict}"), &zlib(data))
}

const FONT: &str = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>";

/// Catalog (1), pages (2), one page (3) with `content` as object 4, plus `extra` objects.
fn page_pdf(content: &[u8], res: &str, extra: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
    let mut objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << {res} >> /Contents 4 0 R >>"
            )
            .into_bytes(),
        ),
        (4, stream_flate("", content)),
    ];
    objs.extend(extra);
    build(&objs, 1, true)
}

/// PDF LZW (`EarlyChange` 1), matching hayro's decoder: MSB-first codes of 9 to 12 bits, a
/// clear code before the table fills.
fn lzw(data: &[u8]) -> Vec<u8> {
    struct Bits {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }
    impl Bits {
        fn put(&mut self, code: u16, width: u32) {
            self.acc = (self.acc << width) | u64::from(code);
            self.n += width;
            while self.n >= 8 {
                self.n -= 8;
                self.out.push((self.acc >> self.n) as u8);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push((self.acc << (8 - self.n)) as u8);
            }
            self.out
        }
    }
    // Width the decoder uses for the next code, given how many it read since the last clear.
    let width = |emitted: usize| {
        let table = 258 + emitted.saturating_sub(1);
        let adjusted = table + 1;
        if adjusted >= 2048 {
            12
        } else if adjusted >= 1024 {
            11
        } else if adjusted >= 512 {
            10
        } else {
            9
        }
    };
    let mut bits = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    let mut dict: HashMap<(u16, u8), u16> = HashMap::new();
    let mut next: u16 = 258;
    let mut emitted = 0usize;
    bits.put(256, 9);
    let mut w: Option<u16> = None;
    for &b in data {
        let Some(cur) = w else {
            w = Some(u16::from(b));
            continue;
        };
        if let Some(&code) = dict.get(&(cur, b)) {
            w = Some(code);
            continue;
        }
        bits.put(cur, width(emitted));
        emitted += 1;
        dict.insert((cur, b), next);
        next += 1;
        w = Some(u16::from(b));
        if next >= 3800 {
            bits.put(256, width(emitted));
            dict.clear();
            next = 258;
            emitted = 0;
        }
    }
    if let Some(cur) = w {
        bits.put(cur, width(emitted));
        emitted += 1;
    }
    bits.put(257, width(emitted));
    bits.finish()
}

// ---------------------------------------------------------------------------------------------
// Running the extractor.

struct Run {
    doc: Result<Document, Error>,
    facts: Vec<Fact>,
    problems: Vec<String>,
    raster: Option<Raster>,
}

fn run_with(bytes: Vec<u8>, limits: &Limits, out: Emitter) -> Run {
    let mut out = out;
    let doc = PdfExtractor::new().extract(&Blob::from(bytes), limits, &mut out);
    let problems = out.take_problems();
    let (facts, artifacts, _) = out.into_parts();
    Run {
        doc,
        facts,
        problems,
        raster: artifacts.get::<Raster>().cloned(),
    }
}

fn run(bytes: Vec<u8>, limits: &Limits) -> Run {
    run_with(bytes, limits, Emitter::new(PdfExtractor::ID))
}

fn fact<'a>(r: &'a Run, k: &FactKey) -> Option<&'a Value> {
    r.facts.iter().find(|f| &f.key == k).map(|f| &f.value)
}

fn small_budget(mib: u64) -> Limits {
    Limits {
        max_decompressed: mib << 20,
        ..Limits::default()
    }
}

fn has_problem(r: &Run, needle: &str) -> bool {
    r.problems.iter().any(|p| p.contains(needle))
}

/// The file was refused for its decode budget: an error, or a partial record saying so.
fn refused(r: &Run) -> bool {
    matches!(r.doc, Err(Error::LimitExceeded("max_decompressed")))
        || (r.doc.is_ok() && has_problem(r, "max_decompressed"))
}

// ---------------------------------------------------------------------------------------------
// Object-stream bombs: refused while opening, whatever the filter chain.

fn objstm_pdf(filter: &str, body: &[u8]) -> Vec<u8> {
    build(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (
                5,
                stream_raw(
                    &format!("/Type /ObjStm /N 1 /First 4 /Filter {filter}"),
                    body,
                ),
            ),
        ],
        1,
        false,
    )
}

fn objstm_payload(pad_mib: usize) -> Vec<u8> {
    let mut v = b"3 0 << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >> ".to_vec();
    v.resize(v.len() + pad_mib * MIB, b' ');
    v
}

#[test]
fn objstm_chained_flate_bomb_is_bounded() {
    let layer1 = zlib(&objstm_payload(256));
    let layer2 = zlib(&layer1);
    assert!(layer2.len() < 64 * 1024, "bomb is small: {}", layer2.len());
    let bytes = objstm_pdf("[/FlateDecode /FlateDecode]", &layer2);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(16)));
    assert!(peak < 96 * MIB, "peak {} MiB", peak / MIB);
    assert!(refused(&r), "{:?} {:?}", r.doc, r.problems);
}

#[test]
fn objstm_lzw_bomb_is_bounded() {
    let body = lzw(&objstm_payload(128));
    assert!(body.len() < 1_000_000, "{}", body.len());
    let bytes = objstm_pdf("/LZWDecode", &body);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(16)));
    assert!(peak < 96 * MIB, "peak {} MiB", peak / MIB);
    assert!(refused(&r), "{:?} {:?}", r.doc, r.problems);
}

#[test]
fn objstm_ascii_then_flate_bomb_is_bounded() {
    // ASCIIHex wrapping a flate bomb: every layer is measured.
    let inner = zlib(&objstm_payload(256));
    let mut hex = String::with_capacity(inner.len() * 2 + 1);
    for b in &inner {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{b:02x}"));
    }
    hex.push('>');
    let bytes = objstm_pdf("[/ASCIIHexDecode /FlateDecode]", hex.as_bytes());
    let (r, peak) = peak_of(|| run(bytes, &small_budget(16)));
    assert!(peak < 96 * MIB, "peak {} MiB", peak / MIB);
    assert!(refused(&r), "{:?} {:?}", r.doc, r.problems);
}

#[test]
fn lzw_encoder_matches_decoder() {
    // Sanity: a small LZW content stream decodes to real text.
    let content = b"BT /F1 12 Tf 72 700 Td (hello lzw) Tj ET".to_vec();
    let bytes = build(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (
                3,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << {FONT} >> /Contents 4 0 R >>"
                )
                .into_bytes(),
            ),
            (4, stream_raw("/Filter /LZWDecode", &lzw(&content))),
        ],
        1,
        true,
    );
    let r = run(bytes, &Limits::default());
    let doc = r.doc.expect("doc");
    assert!(
        doc.pages[0].text.contains("hello lzw"),
        "{:?}",
        doc.pages[0].text
    );
}

#[test]
fn encrypted_content_bomb_is_bounded() {
    // Owner-password-only RC4: streams are decrypted, then inflated under the same budget.
    let mut doc = lopdf::Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut s = lopdf::Stream::new(lopdf::Dictionary::new(), vec![b' '; 256 * MIB]);
    s.compress().expect("compress");
    let content = doc.add_object(s);
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(
        pages_id,
        lopdf::Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
        }),
    );
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    doc.trailer.set(
        "ID",
        vec![
            lopdf::Object::String(
                b"0123456789abcdef".to_vec(),
                lopdf::StringFormat::Hexadecimal,
            ),
            lopdf::Object::String(
                b"0123456789abcdef".to_vec(),
                lopdf::StringFormat::Hexadecimal,
            ),
        ],
    );
    let v = lopdf::EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: lopdf::Permissions::all(),
    };
    let state = lopdf::EncryptionState::try_from(v).expect("state");
    doc.encrypt(&state).expect("encrypt");
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("save");
    drop(doc);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(16)));
    assert!(peak < 96 * MIB, "peak {} MiB", peak / MIB);
    assert_eq!(fact(&r, &keys::ENCRYPTED), Some(&Value::Bool(true)));
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
}

// ---------------------------------------------------------------------------------------------
// Shared and repeated content streams.

fn shared_pdf(npages: u32, mib: usize, array_on_one_page: bool) -> Vec<u8> {
    let mut content = b"BT /F1 9 Tf 72 700 Td (shared) Tj ET ".to_vec();
    content.resize(mib * MIB, b' ');
    let mut objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (4, stream_flate("", &content)),
    ];
    if array_on_one_page {
        let refs = "4 0 R ".repeat(npages as usize);
        objs.push((2, b"<< /Type /Pages /Kids [10 0 R] /Count 1 >>".to_vec()));
        objs.push((
            10,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << {FONT} >> /Contents [{refs}] >>"
            )
            .into_bytes(),
        ));
    } else {
        let kids: Vec<String> = (0..npages).map(|i| format!("{} 0 R", 10 + i)).collect();
        objs.push((
            2,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {npages} >>",
                kids.join(" ")
            )
            .into_bytes(),
        ));
        for i in 0..npages {
            objs.push((
                10 + i,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << {FONT} >> /Contents 4 0 R >>"
                )
                .into_bytes(),
            ));
        }
    }
    build(&objs, 1, true)
}

#[test]
fn shared_contents_memory_is_bounded_by_a_page() {
    // 12 pages share one 16 MiB content stream: 192 MiB decoded in total, under the default
    // 256 MiB budget, but never more than about one parse's cache (64 MiB) held at once.
    let bytes = shared_pdf(12, 16, false);
    let (r, peak) = peak_of(|| run(bytes, &Limits::default()));
    let doc = r.doc.expect("doc");
    assert_eq!(doc.pages.len(), 12);
    assert!(doc.pages.iter().all(|p| p.text.contains("shared")));
    assert!(peak < 128 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn shared_contents_stop_at_the_budget() {
    let bytes = shared_pdf(200, 16, false);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(128)));
    let doc = r.doc.as_ref().expect("doc");
    assert!(doc.pages.len() < 200);
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
    assert!(peak < 192 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn repeated_contents_array_is_bounded() {
    // One page whose /Contents lists the same 16 MiB stream 100 times.
    let bytes = shared_pdf(100, 16, true);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(128)));
    assert!(r.doc.is_ok());
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
    assert!(peak < 320 * MIB, "peak {} MiB", peak / MIB);
}

// ---------------------------------------------------------------------------------------------
// Unbounded drawing time.

fn fanout_pdf(levels: u32) -> Vec<u8> {
    let mut extra = Vec::new();
    for k in 0..levels {
        let n = 10 + k;
        let (content, res) = if k + 1 < levels {
            (
                b"/X Do /X Do".to_vec(),
                format!("/XObject << /X {} 0 R >>", n + 1),
            )
        } else {
            (b"BT /F1 1 Tf (a) Tj ET".to_vec(), FONT.to_owned())
        };
        extra.push((
            n,
            stream_flate(
                &format!(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << {res} >>"
                ),
                &content,
            ),
        ));
    }
    page_pdf(b"/X Do", "/XObject << /X 10 0 R >>", extra)
}

#[test]
fn form_fanout_is_cut_by_the_draw_cap_or_deadline() {
    let bytes = fanout_pdf(40);
    let deadline = Instant::now() + Duration::from_secs(3);
    let t = Instant::now();
    let r = run_with(
        bytes.clone(),
        &Limits::default(),
        Emitter::with_deadline(PdfExtractor::ID, deadline),
    );
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    assert!(r.doc.is_ok());
    assert!(
        has_problem(&r, "draw operations") || has_problem(&r, "timeout"),
        "{:?}",
        r.problems
    );
    // The raster refuses the page through its dry run instead of rendering forever.
    let raster = PdfRaster::new(Blob::from(bytes));
    let limits = Limits {
        timeout: Duration::from_secs(3),
        ..Limits::default()
    };
    let t = Instant::now();
    assert!(raster.render(0, 72.0, &limits).is_err());
    assert!(raster.embedded_scan(0, &limits).is_none_or(|r| r.is_err()));
    assert!(t.elapsed() < Duration::from_secs(40), "{:?}", t.elapsed());
}

#[test]
fn a_page_over_the_draw_cap_is_refused_by_the_raster_at_once() {
    // Fuzz finding (quail #131): the extractor stops a 2^n form fan-out at the draw cap and
    // marks the page as needing OCR; page hashing and OCR then interpreted it up to the same
    // cap twice more (the scan lookup, then the render's dry run). The extractor's raster now
    // refuses such a page without interpreting it.
    let r = run(fanout_pdf(40), &Limits::default());
    assert!(has_problem(&r, "draw operations"), "{:?}", r.problems);
    let raster = r.raster.expect("raster");
    let limits = Limits::default();
    let t = Instant::now();
    assert!(matches!(
        raster.0.render(0, 36.0, &limits),
        Err(Error::LimitExceeded("draw operations per page"))
    ));
    assert!(matches!(
        raster.0.embedded_scan_reduced(0, 128, &limits),
        Some(Err(Error::LimitExceeded("draw operations per page")))
    ));
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn a_page_over_the_decode_budget_is_refused_by_the_raster_at_once() {
    // Fuzz finding (quail #136, `slow@pdf.extract@4`, `slow@sketch.page_phash@4`): a page
    // whose `/Contents` lists one small stream of operators thousands of times, so its
    // content inflates past the 256 MiB `max_decompressed` cap. The extractor stops at the
    // cap; every scan lookup and render then inflated and interpreted it up to the cap again
    // only to refuse it. They now refuse it at once.
    let part = b"0 0 0 0 0 0 cm ".repeat(32 * 1024 / 15);
    let refs = "5 0 R ".repeat(10_000);
    let bytes = build(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (
                3,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << {FONT} >> /Contents [4 0 R {refs}] >>"
                )
                .into_bytes(),
            ),
            (4, stream_flate("", b"BT /F1 9 Tf 72 700 Td (bomb) Tj ET")),
            (5, stream_flate("", &part)),
        ],
        1,
        true,
    );
    let limits = Limits::default();
    let t = Instant::now();
    let r = run(bytes, &limits);
    let extract = t.elapsed();
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
    let raster = r.raster.expect("raster");
    // Page hashing and OCR ask for the page's scan and renders, at several sizes.
    let calls = Instant::now();
    for dpi in [36.0, 72.0, 100.0, 150.0, 200.0, 300.0, 400.0, 600.0] {
        assert!(matches!(
            raster.0.embedded_scan(0, &limits),
            Some(Err(Error::LimitExceeded("max_decompressed")))
        ));
        assert!(matches!(
            raster.0.embedded_scan_reduced(0, 128, &limits),
            Some(Err(Error::LimitExceeded("max_decompressed")))
        ));
        assert!(matches!(
            raster.0.render(0, dpi, &limits),
            Err(Error::LimitExceeded("max_decompressed"))
        ));
    }
    // All of them together cost less than the one capped pass of the extractor.
    assert!(
        calls.elapsed() < extract,
        "{:?} vs {extract:?}",
        calls.elapsed()
    );
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
}

/// Whether a render shows some ink.
fn inked(img: &quail_core::Image) -> bool {
    img.data.iter().any(|&p| p < 128)
}

#[test]
fn large_pages_under_the_decode_budget_still_render() {
    // A big content stream under the cap is extracted and rendered as before.
    let mut content = b"BT /F1 24 Tf 72 700 Td (large) Tj ET ".to_vec();
    content.resize(128 * MIB, b' ');
    let bytes = page_pdf(&content, FONT, vec![]);
    drop(content);
    let limits = Limits::default();
    let r = run(bytes, &limits);
    assert!(r.doc.expect("doc").pages[0].text.contains("large"));
    let img = r.raster.expect("raster").0.render(0, 36.0, &limits);
    assert!(inked(&img.expect("render")));

    // A page the extractor refused only because earlier pages spent the file's budget fits
    // the raster's fresh per-call budget, so it still renders.
    let limits = small_budget(64);
    let r = run(shared_pdf(3, 24, false), &limits);
    assert!(has_problem(&r, "page 2: a stream"), "{:?}", r.problems);
    let raster = r.raster.expect("raster");
    for _ in 0..2 {
        assert!(inked(&raster.0.render(2, 36.0, &limits).expect("render")));
    }
}

#[test]
fn one_huge_text_show_is_bounded_in_memory() {
    // Fuzz finding (quail #131, `oom-abort@pdf.extract@3`): one `Tj` of a few million
    // characters (a 20 KB file inflating to 8 MB of content). hayro collected every glyph of
    // a run (~180 bytes each) before handing it to the device: 1.4 GiB, then the allocation
    // failure aborted the process. Runs now reach the device in bounded pieces.
    // 3 M glyphs per page, below the 4 M draw cap, so the page still gives text.
    let text = "15.03.2024. ".repeat(250_000);
    let half = "15.03.2024. ".repeat(125_000);
    for content in [
        format!("BT /F1 11 Tf 72 740 Td ({text}) Tj ET"),
        format!("BT /F1 11 Tf 72 740 Td [({half}) -250 ({half})] TJ ET"),
    ] {
        let bytes = page_pdf(content.as_bytes(), FONT, vec![]);
        let (r, peak) = peak_of(|| run(bytes.clone(), &Limits::default()));
        assert!(peak < 192 * MIB, "peak {} MiB", peak / MIB);
        let doc = r.doc.expect("doc");
        assert!(
            doc.pages[0].text.starts_with("15.03.2024. 15.03.2024."),
            "{:?}",
            doc.pages[0].text.get(..40)
        );
        // The render path shows the same runs.
        let raster = PdfRaster::new(Blob::from(bytes));
        let (_, peak) = peak_of(|| raster.render(0, 36.0, &Limits::default()));
        assert!(peak < 192 * MIB, "render peak {} MiB", peak / MIB);
    }
}

/// [`fanout_pdf`] with form `10 + heavy` made costly to draw: `junk` bytes of garbage in its
/// dictionary, or (`junk == 0`) content that claims `/FlateDecode` but is stored plain, so
/// every draw fails to inflate it again.
fn heavy_fanout_pdf(levels: u32, heavy: u32, junk: usize) -> Vec<u8> {
    let mut extra = Vec::new();
    for k in 0..levels {
        let n = 10 + k;
        let (content, res) = if k + 1 < levels {
            (
                b"/X Do /X Do".to_vec(),
                format!("/XObject << /X {} 0 R >>", n + 1),
            )
        } else {
            (b"BT /F1 1 Tf (a) Tj ET".to_vec(), FONT.to_owned())
        };
        let dict =
            format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << {res} >>");
        let body = if k != heavy {
            stream_flate(&dict, &content)
        } else if junk > 0 {
            stream_flate(&format!("{} {dict}", "e ".repeat(junk / 2)), &content)
        } else {
            let mut plain = b"x\x9c".to_vec();
            for _ in 0..40 {
                plain.extend_from_slice(
                    b"BT /F1 12 Tf 72 700 Td (UGOVOR O KUPOPRODAJI br. 12) Tj ET ",
                );
            }
            plain.extend_from_slice(&content);
            stream_raw(&format!("/Filter /FlateDecode {dict}"), &plain)
        };
        extra.push((n, body));
    }
    page_pdf(b"/X Do", "/XObject << /X 10 0 R >>", extra)
}

#[test]
fn costly_forms_in_a_fanout_are_cut_by_the_form_data_cap() {
    // Fuzz findings (quail #131, `deadline@pdf.extract@3`, `slow@pdf.extract`): a 2^n form
    // fan-out with one costly form deep inside. hayro re-parses a form on every `Do`, so a
    // 2.8 MB junk dictionary, or a stream that fails to inflate, drawn thousands of times took
    // the whole file deadline while the nesting cap was still far.
    for (levels, heavy, junk) in [(30, 26, 2 * MIB), (27, 14, 0)] {
        let bytes = heavy_fanout_pdf(levels, heavy, junk);
        let t = Instant::now();
        let r = run(bytes, &Limits::default());
        // Generous for slow CI runners (Windows debug: ~5 s); before the fix this took ~100 s.
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "{heavy}: {:?}",
            t.elapsed()
        );
        assert!(r.doc.is_ok());
        assert!(has_problem(&r, "draw operations"), "{:?}", r.problems);
    }
}

#[test]
fn unclosed_dictionaries_repair_in_linear_time() {
    // Fuzz finding (quail #131, `slow@pdf.extract`): 4 MB of `5 0 obj << /Type /eam endobj`
    // and no xref. The repair scan validates every object header by skipping the object after
    // it; an unclosed dictionary swallowed the following objects as nested garbage, up to the
    // nesting cap of 512, from every header. Validation now stops at the next `endobj`.
    let mut bytes = page_pdf(b"BT /F1 12 Tf 72 700 Td (repaired) Tj ET", FONT, vec![]);
    let cut = bytes.windows(4).position(|w| w == b"xref").expect("xref");
    bytes.truncate(cut);
    for _ in 0..80_000 {
        bytes.extend_from_slice(b"5 0 obj\n<< /Filter /FlateDecode /Type /eam endobj\n");
    }
    bytes.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    let doc = r.doc.expect("doc");
    assert!(
        doc.pages[0].text.contains("repaired"),
        "{:?}",
        doc.pages[0].text
    );
}

#[test]
fn inline_image_end_search_is_bounded() {
    // Fuzz finding (quail #131, `slow@pdf.extract@4`): an inline image followed by one string
    // that never closes, with 20 000 `EI`s inside it. hayro tried every `EI` as the image's end
    // by parsing the rest of the stream as content, and every parse ran to the end of the
    // unclosed string: 20 s for a 3 KB file.
    let mut c = b"q 10 0 0 10 100 100 cm BI /W 4 /H 4 /CS /G /BPC 8 ID ".to_vec();
    c.extend_from_slice(&[0x80; 16]);
    c.extend_from_slice(b" EI Q BT /F1 12 Tf 72 700 Td (inlin");
    for _ in 0..20_000 {
        c.extend_from_slice(&[0x80; 10]);
        c.extend_from_slice(b" EI Q BT /F1 12 Tf 72 700 Td (inlin");
    }
    c.extend_from_slice(b"(inline image page) Tj ET");
    let bytes = page_pdf(&c, FONT, vec![]);
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(r.doc.is_ok());
}

#[test]
fn a_missing_font_selected_often_is_cheap() {
    // Fuzz finding (quail #134, `slow@sketch.page_phash@4`): a page whose `/Contents` lists one
    // stream 60 000 times, each selecting a font the page does not have. hayro built a new
    // fallback Helvetica (a 256-entry glyph table) on every such `Tf`, in extraction and again
    // in the render's dry run and the render: 7 s. Here 1 M selections in one stream.
    let mut c = b"BT ".to_vec();
    for _ in 0..1_000_000 {
        c.extend_from_slice(b"/F9 12 Tf ");
    }
    c.extend_from_slice(b"72 700 Td (fallback text) Tj ET");
    let bytes = page_pdf(&c, FONT, vec![]);
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    let raster = r.raster.expect("raster");
    let image = raster.0.render(0, 36.0, &Limits::default());
    // Generous for slow CI runners (Windows debug); before the fix this took ~50 s.
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    let doc = r.doc.expect("doc");
    assert!(
        doc.pages[0].text.contains("fallback text"),
        "{:?}",
        doc.pages[0].text
    );
    assert!(image.is_ok(), "{image:?}");
}

#[test]
fn unclosed_nested_dictionaries_repair_in_linear_time() {
    // Fuzz finding (quail #134, `slow@pdf.extract@4`): `11 0 obj << /Roo` repeated 264 000
    // times and no usable xref. The repair scan validates every object header and probes
    // every `<<`; each probe of the never-closed chain of dictionaries ran to the nesting cap
    // of 512 levels. The repair scan now allows 32 levels.
    let mut bytes = page_pdf(b"BT /F1 12 Tf 72 700 Td (repaired) Tj ET", FONT, vec![]);
    let cut = bytes.windows(4).position(|w| w == b"xref").expect("xref");
    bytes.truncate(cut);
    for _ in 0..1_600_000 {
        bytes.extend_from_slice(b"11 0 obj\n<</Roo");
    }
    bytes.extend_from_slice(b"t 1 0 R >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    // The raster parses the file again (a second repair).
    let raster = r.raster.expect("raster");
    let image = raster.0.render(0, 36.0, &Limits::default());
    // Generous for slow CI runners (Windows debug); before the fix this took ~43 s.
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    let doc = r.doc.expect("doc");
    assert!(
        doc.pages[0].text.contains("repaired"),
        "{:?}",
        doc.pages[0].text
    );
    assert!(image.is_ok(), "{image:?}");
}

/// A page PDF without its xref, then object 9 holding `junk`, then a trailer: the repair scan
/// has to walk `junk` and still find the page.
fn repair_pdf_with(junk: &[u8]) -> Vec<u8> {
    let mut bytes = page_pdf(b"BT /F1 12 Tf 72 700 Td (repaired) Tj ET", FONT, vec![]);
    let cut = bytes.windows(4).position(|w| w == b"xref").expect("xref");
    bytes.truncate(cut);
    bytes.extend_from_slice(b"9 0 obj\n<< /Length 5 >>\nstream\n");
    bytes.extend_from_slice(junk);
    bytes.extend_from_slice(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    bytes
}

/// Opens `bytes` for text and raster (each parse repairs the file) within 20 s, and checks
/// that the page was found.
fn assert_repaired_in_time(bytes: Vec<u8>) {
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    let raster = r.raster.expect("raster");
    let image = raster.0.render(0, 36.0, &Limits::default());
    // Generous for slow CI runners (Windows debug).
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    let doc = r.doc.expect("doc");
    assert!(
        doc.pages[0].text.contains("repaired"),
        "{:?}",
        doc.pages[0].text
    );
    assert!(image.is_ok(), "{image:?}");
}

#[test]
fn dictionaries_inside_an_unclosed_string_repair_in_linear_time() {
    // Fuzz finding (quail #139, `slow@pdf.extract@4`): a 2.5 MB stream with 16 000 `<<` and
    // one literal string that never closes (every repetition opens one more `(` than it
    // closes), no usable xref. The repair scan probes every `<<`, and every probe parsed into
    // the string and walked to the next `endobj`: 20 GB, 16 s. Here 100 000 of them in 900 KB;
    // before the fix this took about 90 s. The scan now has a work budget per `endobj` window.
    assert_repaired_in_time(repair_pdf_with(&b"(x << /A\n".repeat(100_000)));
}

#[test]
fn probes_hidden_by_comments_repair_in_linear_time() {
    // Same scan, quail #139: `<< /A %` repeated on one line, then a long unclosed array. Every
    // `<<` is inside the comment of the one before it, so each probe is a fresh start; each
    // read its comment to the end of the line, then the whole array. Before the fix this took
    // about 45 s.
    let mut junk = b"<< /A % ".repeat(4_000);
    junk.extend_from_slice(b"\n/B [ ");
    junk.extend_from_slice(&b"1 ".repeat(500_000));
    assert_repaired_in_time(repair_pdf_with(&junk));
}

#[test]
fn type3_recursion_is_bounded() {
    // A Type3 glyph whose procedure shows the same glyph twice: recursion through fonts.
    let charproc = stream_flate("", b"1000 0 0 0 1000 1000 d1 BT /T3 1 Tf (a) Tj (a) Tj ET");
    let font = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << /Font << /T3 5 0 R >> >> >>".to_vec();
    let bytes = page_pdf(
        b"BT /T3 12 Tf 72 700 Td (aaaa) Tj ET",
        "/Font << /T3 5 0 R >>",
        vec![(5, font), (6, charproc)],
    );
    let t = Instant::now();
    let r = run(bytes.clone(), &Limits::default());
    assert!(r.doc.is_ok());
    let raster = PdfRaster::new(Blob::from(bytes));
    let limits = Limits {
        timeout: Duration::from_secs(3),
        ..Limits::default()
    };
    let _ = raster.render(0, 36.0, &limits);
    assert!(t.elapsed() < Duration::from_secs(40), "{:?}", t.elapsed());
}

#[test]
fn many_tiny_lines_lay_out_in_linear_time() {
    // 80 000 lines of one glyph each (the reviewer's quadratic-layout file).
    let n = 80_000;
    let mut c = b"BT /F1 0.5 Tf 10 780 Td ".to_vec();
    for _ in 0..n {
        c.extend_from_slice(b"0 -1 Td (a) Tj ");
    }
    c.extend_from_slice(b"ET");
    let bytes = page_pdf(&c, FONT, vec![]);
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    let doc = r.doc.expect("doc");
    assert!(doc.pages[0].text.len() >= n);
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
}

// ---------------------------------------------------------------------------------------------
// Images whose real size differs from their dictionary.

fn jbig2_pdf(w: u32, h: u32) -> Vec<u8> {
    // Embedded JBIG2: a page-information segment claiming w × h, then end of page.
    let seg = |num: u32, typ: u8, data: &[u8]| {
        let mut s = num.to_be_bytes().to_vec();
        s.extend_from_slice(&[typ, 0, 1]);
        s.extend_from_slice(&(data.len() as u32).to_be_bytes());
        s.extend_from_slice(data);
        s
    };
    let mut pinfo = w.to_be_bytes().to_vec();
    pinfo.extend_from_slice(&h.to_be_bytes());
    pinfo.extend_from_slice(&[0; 8]);
    pinfo.extend_from_slice(&[0, 0, 0]);
    let mut data = seg(0, 48, &pinfo);
    data.extend(seg(1, 49, b""));
    let img = stream_raw(
        "/Type /XObject /Subtype /Image /Width 100 /Height 100 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode",
        &data,
    );
    page_pdf(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        vec![(5, img)],
    )
}

#[test]
fn jbig2_page_larger_than_its_dictionary_is_refused() {
    let bytes = jbig2_pdf(65_536, 65_536);
    let ((), peak) = peak_of(|| {
        let r = run(bytes.clone(), &Limits::default());
        assert!(r.doc.is_ok());
        let raster = r.raster.expect("raster");
        let _ = raster.0.render(0, 72.0, &Limits::default());
        let scan = raster.0.embedded_scan(0, &Limits::default());
        assert!(
            scan.is_none_or(|s| s.is_err()),
            "must not decode a 4 Gpx page"
        );
    });
    assert!(peak < 128 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn jpx_header_larger_than_the_cap_is_refused() {
    // A JPEG 2000 codestream whose SIZ marker claims 60 000 × 60 000 × 3.
    let mut cs = vec![0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x2F, 0x00, 0x00];
    for v in [60_000u32, 60_000, 0, 0, 60_000, 60_000, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&3u16.to_be_bytes());
    for _ in 0..3 {
        cs.extend_from_slice(&[7, 1, 1]);
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    let img = stream_raw(
        "/Type /XObject /Subtype /Image /Width 100 /Height 100 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Filter /JPXDecode",
        &cs,
    );
    let bytes = page_pdf(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        vec![(5, img)],
    );
    let ((), peak) = peak_of(|| {
        let raster = PdfRaster::new(Blob::from(bytes));
        let _ = raster.render(0, 72.0, &Limits::default());
        assert!(
            raster
                .embedded_scan(0, &Limits::default())
                .is_none_or(|s| s.is_err())
        );
    });
    assert!(peak < 128 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn oversized_images_in_forms_inline_and_patterns_are_refused_when_rendering() {
    let huge = "/Width 100000 /Height 100000 /BitsPerComponent 8 /ColorSpace /DeviceRGB";
    // Image inside a form.
    let img = stream_flate(
        &format!("/Type /XObject /Subtype /Image {huge}"),
        &[0; 1000],
    );
    let form = stream_flate(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Im0 6 0 R >> >>",
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
    );
    let in_form = page_pdf(
        b"/F Do",
        "/XObject << /F 5 0 R >>",
        vec![(5, form), (6, img.clone())],
    );
    // Inline image.
    let inline = page_pdf(
        b"q 612 0 0 792 0 0 cm BI /W 100000 /H 100000 /BPC 8 /CS /RGB ID \x00\x00\x00 EI Q",
        "",
        vec![],
    );
    // Image inside a tiling pattern.
    let pattern = stream_flate(
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 612 792] /XStep 612 /YStep 792 /Resources << /XObject << /Im0 6 0 R >> >>",
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
    );
    let in_pattern = page_pdf(
        b"/Pattern cs /P0 scn 0 0 612 792 re f",
        "/Pattern << /P0 5 0 R >>",
        vec![(5, pattern), (6, img)],
    );
    // Inline images get the file's budget from the interpreter, so their refusal is reported
    // like any other.
    for (bytes, reported) in [(in_form, true), (inline, true), (in_pattern, true)] {
        let ((), peak) = peak_of(|| {
            let r = run(bytes.clone(), &Limits::default());
            assert!(r.doc.is_ok());
            let raster = PdfRaster::new(Blob::from(bytes));
            let res = raster.render(0, 72.0, &Limits::default());
            if reported {
                assert!(
                    matches!(res, Err(Error::LimitExceeded(_))),
                    "{:?}",
                    res.map(|i| (i.width, i.height))
                );
            }
        });
        assert!(peak < 128 * MIB, "peak {} MiB", peak / MIB);
    }
}

// ---------------------------------------------------------------------------------------------
// Page trees.

#[test]
fn huge_kids_array_is_capped() {
    // 200 000 inline page dictionaries (not deduplicated by reference).
    let kids = "<< /Type /Page /MediaBox [0 0 1 1] >> ".repeat(200_000);
    let bytes = build(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (
                2,
                format!("<< /Type /Pages /Kids [{kids}] /Count 200000 >>").into_bytes(),
            ),
        ],
        1,
        true,
    );
    let t = Instant::now();
    let (r, peak) = peak_of(|| run(bytes, &Limits::default()));
    assert!(r.doc.is_ok());
    assert!(has_problem(&r, "page tree truncated"), "{:?}", r.problems);
    assert!(peak < 512 * MIB, "peak {} MiB", peak / MIB);
    assert!(t.elapsed() < Duration::from_mins(1), "{:?}", t.elapsed());
}

#[test]
fn page_tree_cycle_terminates() {
    let bytes = build(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (
                2,
                b"<< /Type /Pages /Kids [3 0 R 2 0 R] /Count 2 >>".to_vec(),
            ),
            (
                3,
                b"<< /Type /Pages /Kids [2 0 R 4 0 R] /Count 1 >>".to_vec(),
            ),
            (4, b"<< /Type /Page /MediaBox [0 0 612 792] >>".to_vec()),
        ],
        1,
        true,
    );
    let r = run(bytes, &Limits::default());
    assert_eq!(r.doc.expect("doc").pages.len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Layout and scans.

#[test]
fn rotated_text_is_grouped_along_its_baseline() {
    // Two lines rotated 90° by `cm`.
    let bytes = page_pdf(
        b"q 0 1 -1 0 300 100 cm BT /F1 12 Tf 0 0 Td (rotated first line) Tj 0 -14 Td (rotated second line) Tj ET Q",
        FONT,
        vec![],
    );
    let r = run(bytes, &Limits::default());
    let doc = r.doc.expect("doc");
    let t = &doc.pages[0].text;
    assert!(t.contains("rotated first line"), "{t:?}");
    assert!(t.contains("rotated second line"), "{t:?}");
    let first = t.find("first").expect("first");
    let second = t.find("second").expect("second");
    assert!(first < second, "{t:?}");
}

#[test]
fn two_column_page_reads_by_column() {
    // Two text columns, each with two paragraphs; the right column's baselines are offset
    // (different leading), as in real columns.
    let mut c = b"BT /F1 10 Tf".to_vec();
    for (i, y) in [700, 688, 676, 628, 616, 604].iter().enumerate() {
        c.extend_from_slice(
            format!(" 1 0 0 1 40 {y} Tm (left {i} lorem ipsum dolor sit amet consectetur) Tj")
                .as_bytes(),
        );
        c.extend_from_slice(
            format!(
                " 1 0 0 1 320 {} Tm (right {i} lorem ipsum dolor sit amet consectetur) Tj",
                y - 5
            )
            .as_bytes(),
        );
    }
    c.extend_from_slice(b" ET");
    let r = run(page_pdf(&c, FONT, vec![]), &Limits::default());
    let doc = r.doc.expect("doc");
    let t = &doc.pages[0].text;
    let pos = |s: &str| t.find(s).expect(s);
    assert!(pos("left 5") < pos("right 0"), "{t:?}");
}

#[test]
fn two_column_table_reads_by_row() {
    // The same geometry on shared baselines is a table: its row groups (split at the gap)
    // are read in order, not the whole left column first. (Stacked cells still merge into a
    // paragraph block per column inside a group; table detection is not done yet.)
    let mut c = b"BT /F1 10 Tf".to_vec();
    for (i, y) in [700, 688, 676, 628, 616, 604].iter().enumerate() {
        c.extend_from_slice(
            format!(" 1 0 0 1 40 {y} Tm (label {i} lorem ipsum dolor sit amet) Tj").as_bytes(),
        );
        c.extend_from_slice(
            format!(" 1 0 0 1 320 {y} Tm (value {i} lorem ipsum dolor sit amet) Tj").as_bytes(),
        );
    }
    c.extend_from_slice(b" ET");
    let r = run(page_pdf(&c, FONT, vec![]), &Limits::default());
    let doc = r.doc.expect("doc");
    let t = &doc.pages[0].text;
    let pos = |s: &str| t.find(s).expect(s);
    assert!(pos("value 2") < pos("label 3"), "{t:?}");
}

fn scan_pdf(cm: &str) -> Vec<u8> {
    let img = stream_raw(
        "/Type /XObject /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 8 /ColorSpace /DeviceGray",
        &[0x80; 16],
    );
    page_pdf(
        format!("q {cm} cm /Im0 Do Q").as_bytes(),
        "/XObject << /Im0 5 0 R >>",
        vec![(5, img)],
    )
}

#[test]
fn embedded_scan_requires_full_upright_coverage() {
    let full = PdfRaster::new(Blob::from(scan_pdf("612 0 0 792 0 0")));
    assert!(matches!(
        full.embedded_scan(0, &Limits::default()),
        Some(Ok(_))
    ));
    // Half the page: render instead.
    let half = PdfRaster::new(Blob::from(scan_pdf("306 0 0 792 0 0")));
    assert!(half.embedded_scan(0, &Limits::default()).is_none());
    // Rotated 90°: render instead.
    let rotated = PdfRaster::new(Blob::from(scan_pdf("0 792 -612 0 612 0")));
    assert!(rotated.embedded_scan(0, &Limits::default()).is_none());
    // Mirrored: render instead.
    let mirrored = PdfRaster::new(Blob::from(scan_pdf("-612 0 0 792 612 0")));
    assert!(mirrored.embedded_scan(0, &Limits::default()).is_none());
}

#[test]
fn raster_page_indices_follow_max_pages() {
    let mut objs = vec![(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec())];
    let kids: Vec<String> = (0..5).map(|i| format!("{} 0 R", 10 + i)).collect();
    objs.push((
        2,
        format!("<< /Type /Pages /Kids [{}] /Count 5 >>", kids.join(" ")).into_bytes(),
    ));
    for i in 0..5 {
        objs.push((
            10 + i,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".to_vec(),
        ));
    }
    let bytes = build(&objs, 1, true);
    let limits = Limits {
        max_pages: 2,
        ..Limits::default()
    };
    let r = run(bytes, &limits);
    let doc = r.doc.as_ref().expect("doc");
    assert_eq!(doc.pages.len(), 2);
    let raster = r.raster.expect("raster");
    // Same indices as `Page::index`; pages the extractor skipped are refused too.
    for p in &doc.pages {
        assert!(raster.0.render(p.index, 36.0, &limits).is_ok());
    }
    assert!(matches!(
        raster.0.render(3, 36.0, &limits),
        Err(Error::LimitExceeded("max_pages"))
    ));
    assert!(raster.0.render(3, 36.0, &Limits::default()).is_ok());
    assert!(raster.0.render(5, 36.0, &Limits::default()).is_err());
}

#[test]
fn raster_parses_once_for_many_pages() {
    // 200 renders of a 200-page file must not re-parse per call (quadratic in pages).
    let mut objs = vec![(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec())];
    let kids: Vec<String> = (0..200).map(|i| format!("{} 0 R", 10 + i)).collect();
    objs.push((
        2,
        format!("<< /Type /Pages /Kids [{}] /Count 200 >>", kids.join(" ")).into_bytes(),
    ));
    for i in 0..200 {
        objs.push((
            10 + i,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 50 50] >>".to_vec(),
        ));
    }
    let raster = PdfRaster::new(Blob::from(build(&objs, 1, true)));
    let t = Instant::now();
    for i in 0..200 {
        raster.render(i, 18.0, &Limits::default()).expect("render");
    }
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
}

// ---------------------------------------------------------------------------------------------
// Second review: allocation discipline, render limits, stale caches.

/// A page whose content is `prefix` followed by `mib` MiB of spaces, flate-compressed.
fn big_content_pdf(mib: usize, layers: usize) -> Vec<u8> {
    let mut content = b"BT /F1 24 Tf 72 700 Td (visible) Tj ET ".to_vec();
    content.resize(content.len() + mib * MIB, b' ');
    let mut body = content;
    for _ in 0..layers {
        body = zlib(&body);
    }
    let filters = vec!["/FlateDecode"; layers].join(" ");
    page_pdf(
        b"",
        FONT,
        vec![(4, stream_raw(&format!("/Filter [{filters}]"), &body))],
    )
}

#[test]
fn capped_inflate_does_not_double_its_buffer() {
    // 300 MiB behind a 64 MiB budget: `read_to_end` would grow the buffer to 128 MiB before
    // the cap is seen; chunked capped growth stays near 64 MiB.
    let bytes = big_content_pdf(300, 1);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(64)));
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
    assert!(peak < 100 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn chained_layers_are_charged_together() {
    // Two flate layers: the first expands to ~1 MiB, the second to 300 MiB; both layers'
    // outputs count against the same 64 MiB budget.
    let bytes = big_content_pdf(300, 2);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(64)));
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
    assert!(peak < 100 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn contents_concatenation_is_charged_and_capped() {
    // 100 references to one 16 MiB stream: the concatenated copy is charged too.
    let bytes = shared_pdf(100, 16, true);
    let (r, peak) = peak_of(|| run(bytes, &small_budget(64)));
    assert!(has_problem(&r, "max_decompressed"), "{:?}", r.problems);
    assert!(peak < 120 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn many_distinct_images_hit_the_render_budget() {
    // Ten distinct 2000 x 2000 1-bit images: tiny on disk, 8 bytes per pixel once drawn.
    let img = stream_flate(
        "/Type /XObject /Subtype /Image /Width 2000 /Height 2000 /BitsPerComponent 1 /ColorSpace /DeviceGray",
        &vec![0x55; 250 * 2000],
    );
    let mut content = Vec::new();
    let mut res = String::from("/XObject << ");
    let mut extra = Vec::new();
    for i in 0..10u32 {
        content.extend_from_slice(format!("q 612 0 0 792 0 0 cm /I{i} Do Q ").as_bytes());
        let _ = std::fmt::Write::write_fmt(&mut res, format_args!("/I{i} {} 0 R ", 10 + i));
        extra.push((10 + i, img.clone()));
    }
    res.push_str(">>");
    let bytes = page_pdf(&content, &res, extra);
    let limits = Limits {
        max_decompressed: 16 << 20,
        max_render_pixels: 4_000_000,
        ..Limits::default()
    };
    let (res, peak) = peak_of(|| PdfRaster::new(Blob::from(bytes)).render(0, 72.0, &limits));
    assert!(
        matches!(res, Err(Error::LimitExceeded(_))),
        "{:?}",
        res.map(|i| i.width)
    );
    assert!(peak < 160 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn inline_images_obey_max_render_pixels() {
    // A 3000 x 3000 inline image (9 M px) on a 4 M px budget: refused and reported.
    let data = zlib(&vec![0x80; 3000 * 3000]);
    let mut c = b"q 612 0 0 792 0 0 cm BI /W 3000 /H 3000 /BPC 8 /CS /G /F /Fl ID ".to_vec();
    c.extend_from_slice(&data);
    c.extend_from_slice(b"\nEI Q");
    let bytes = page_pdf(&c, "", vec![]);
    let limits = Limits {
        max_render_pixels: 4_000_000,
        ..Limits::default()
    };
    let raster = PdfRaster::new(Blob::from(bytes));
    assert!(matches!(
        raster.render(0, 36.0, &limits),
        Err(Error::LimitExceeded(_))
    ));
    assert!(raster.embedded_scan(0, &limits).is_none_or(|r| r.is_err()));
}

#[test]
fn nested_tiling_patterns_stop_at_the_deadline() {
    // Six nested patterns with tiny steps: copies multiply per level.
    let mut extra = Vec::new();
    let levels = 6;
    for k in 0..levels {
        let n = 10 + k;
        let (c, r) = if k + 1 < levels {
            (
                b"/Pattern cs /P0 scn 0 0 10 10 re f".to_vec(),
                format!("/Pattern << /P0 {} 0 R >>", n + 1),
            )
        } else {
            (b"0 0 1 rg 0 0 5 5 re f".to_vec(), String::new())
        };
        extra.push((
            n,
            stream_flate(
                &format!("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 1 /YStep 1 /Resources << {r} >>"),
                &c,
            ),
        ));
    }
    let bytes = page_pdf(
        b"/Pattern cs /P0 scn 0 0 612 792 re f",
        "/Pattern << /P0 10 0 R >>",
        extra,
    );
    let limits = Limits {
        timeout: Duration::from_secs(2),
        ..Limits::default()
    };
    let t = Instant::now();
    let res = PdfRaster::new(Blob::from(bytes)).render(0, 72.0, &limits);
    assert!(
        matches!(res, Err(Error::Timeout | Error::LimitExceeded(_))),
        "{:?}",
        res.map(|i| i.width)
    );
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
}

#[test]
fn a_refused_render_does_not_poison_the_next_one() {
    // The first render hits a tiny budget; hayro caches the (refused) content stream. The next
    // render, with room to spare, must re-parse and draw the real page, not a blank one.
    let bytes = big_content_pdf(8, 1);
    let raster = PdfRaster::new(Blob::from(bytes));
    let tight = Limits {
        max_decompressed: 1 << 20,
        ..Limits::default()
    };
    assert!(matches!(
        raster.render(0, 72.0, &tight),
        Err(Error::LimitExceeded(_))
    ));
    let img = raster.render(0, 72.0, &Limits::default()).expect("render");
    assert!(
        img.data.iter().any(|&v| v < 128),
        "blank page after a refused render"
    );
}

#[test]
fn jbig2_halftone_grid_far_larger_than_its_region_is_refused() {
    // A 64 x 64 region with a 16384 x 16384 halftone grid: a 1 GiB value array before.
    let seg = |num: u32, typ: u8, data: &[u8], refs: &[u8]| {
        let mut s = num.to_be_bytes().to_vec();
        s.push(typ);
        s.push((refs.len() as u8) << 5);
        s.extend_from_slice(refs);
        s.push(1);
        s.extend_from_slice(&(data.len() as u32).to_be_bytes());
        s.extend_from_slice(data);
        s
    };
    let mut pinfo = 64u32.to_be_bytes().to_vec();
    pinfo.extend_from_slice(&64u32.to_be_bytes());
    pinfo.extend_from_slice(&[0; 11]);
    let mut pdict = vec![0x00, 1, 1];
    pdict.extend_from_slice(&1u32.to_be_bytes());
    pdict.extend_from_slice(&[0; 64]);
    let mut ht = Vec::new();
    for v in [64u32, 64, 0, 0] {
        ht.extend_from_slice(&v.to_be_bytes());
    }
    ht.extend_from_slice(&[0, 0]);
    ht.extend_from_slice(&16_384u32.to_be_bytes());
    ht.extend_from_slice(&16_384u32.to_be_bytes());
    ht.extend_from_slice(&[0; 8]);
    ht.extend_from_slice(&256u16.to_be_bytes());
    ht.extend_from_slice(&0u16.to_be_bytes());
    ht.extend_from_slice(&[0; 256]);
    let mut data = seg(0, 48, &pinfo, &[]);
    data.extend(seg(1, 16, &pdict, &[]));
    data.extend(seg(2, 22, &ht, &[1]));
    data.extend(seg(3, 49, b"", &[]));
    let img = stream_raw(
        "/Type /XObject /Subtype /Image /Width 64 /Height 64 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode",
        &data,
    );
    let bytes = page_pdf(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        vec![(5, img)],
    );
    let t = Instant::now();
    let ((), peak) = peak_of(|| {
        let raster = PdfRaster::new(Blob::from(bytes));
        let _ = raster.render(0, 72.0, &Limits::default());
        let _ = raster.embedded_scan(0, &Limits::default());
    });
    assert!(peak < 96 * MIB, "peak {} MiB", peak / MIB);
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
}

#[test]
fn raster_reports_an_object_stream_bomb_as_a_limit() {
    let layer1 = zlib(&objstm_payload(256));
    let bytes = objstm_pdf("[/FlateDecode]", &layer1);
    let raster = PdfRaster::new(Blob::from(bytes));
    let res = raster.render(0, 72.0, &small_budget(16));
    assert!(
        matches!(res, Err(Error::LimitExceeded(_))),
        "{:?}",
        res.map(|i| i.width)
    );
}

// ---------------------------------------------------------------------------------------------
// Fuzzer findings (issues #35-#38): the minimized reproducers, as filed.

/// No path may end in a caught panic: the test profile has overflow checks on, so an
/// arithmetic overflow in a patched path panics here instead of wrapping silently.
fn assert_no_panic_anywhere(bytes: &[u8], limits: &Limits) {
    let r = run(bytes.to_vec(), limits);
    assert!(!matches!(r.doc, Err(Error::Panic(_))), "{:?}", r.doc);
    assert!(
        !r.problems
            .iter()
            .any(|p| p.contains("failed") || p.contains("panic")),
        "{:?}",
        r.problems
    );
    let raster = PdfRaster::new(Blob::from(bytes.to_vec()));
    for dpi in [36.0, 150.0] {
        let res = raster.render(0, dpi, limits);
        assert!(
            !matches!(res, Err(Error::Panic(_))),
            "{:?}",
            res.map(|i| i.width)
        );
    }
    let scan = raster.embedded_scan(0, limits);
    assert!(!matches!(scan, Some(Err(Error::Panic(_)))));
}

/// Runs `f` on a thread with the engine's worker stack (32 MiB): a stack overflow aborts the
/// whole test process, so reaching the join is the assertion.
fn on_worker_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(32 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("join")
}

#[test]
fn issue_35_recursive_type3_glyphs_do_not_overflow_the_stack() {
    let bytes = repro::pdf(repro::TYPE3_RECURSION);
    on_worker_stack(move || {
        let limits = Limits {
            timeout: Duration::from_secs(5),
            ..Limits::default()
        };
        let raster = PdfRaster::new(Blob::from(bytes.clone()));
        let t = Instant::now();
        let res = raster.render(0, 36.0, &limits);
        assert!(
            !matches!(res, Err(Error::Panic(_))),
            "{:?}",
            res.map(|i| i.width)
        );
        assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
        assert_no_panic_anywhere(&bytes, &limits);
    });
}

fn small_render_limits() -> Limits {
    Limits {
        max_decompressed: 64 << 20,
        max_render_pixels: 4_000_000,
        timeout: Duration::from_secs(10),
        ..Limits::default()
    }
}

#[test]
fn issue_36_long_text_runs_are_charged_to_the_render_budget() {
    let bytes = repro::pdf(repro::LONG_TJ_GZ);
    let limits = small_render_limits();
    let (res, peak) = peak_of(|| PdfRaster::new(Blob::from(bytes)).render(0, 150.0, &limits));
    assert!(
        matches!(res, Ok(_) | Err(Error::LimitExceeded(_) | Error::Timeout)),
        "{:?}",
        res.map(|i| i.width)
    );
    assert!(peak < 320 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn issue_36_many_shading_fills_are_charged_to_the_render_budget() {
    let bytes = repro::pdf(repro::MANY_SH_GZ);
    let limits = small_render_limits();
    let (res, peak) = peak_of(|| PdfRaster::new(Blob::from(bytes)).render(0, 36.0, &limits));
    assert!(
        matches!(res, Err(Error::LimitExceeded(_) | Error::Timeout)),
        "{:?}",
        res.map(|i| i.width)
    );
    assert!(peak < 320 * MIB, "peak {} MiB", peak / MIB);
}

#[test]
fn issue_37_nested_groups_are_bounded_before_rasterization() {
    let bytes = repro::pdf(repro::NESTED_GROUPS);
    let limits = Limits {
        timeout: Duration::from_secs(3),
        ..Limits::default()
    };
    let raster = PdfRaster::new(Blob::from(bytes));
    let t = Instant::now();
    let res = raster.render(0, 36.0, &limits);
    // Refused by the work budget or the deadline, or rendered, but never long past it.
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    assert!(!matches!(res, Err(Error::Panic(_))));
}

#[test]
fn issue_37_extraction_checks_the_deadline_between_operators() {
    // 59 000 `sh` operators: the deadline is checked between operators, not only on draws.
    let bytes = repro::pdf(repro::MANY_SH_GZ);
    let deadline = Instant::now() + Duration::from_secs(2);
    let t = Instant::now();
    let r = run_with(
        bytes,
        &Limits::default(),
        Emitter::with_deadline(PdfExtractor::ID, deadline),
    );
    assert!(r.doc.is_ok());
    assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
}

#[test]
fn issue_37_raster_calls_share_the_file_deadline() {
    // The file's time is spent: every call fails fast, whatever `limits.timeout` says.
    let bytes = repro::pdf(repro::NESTED_GROUPS);
    let past = Instant::now().checked_sub(Duration::from_secs(1));
    let raster = PdfRaster::with_deadline(Blob::from(bytes.clone()), past);
    let t = Instant::now();
    assert!(matches!(
        raster.render(0, 36.0, &Limits::default()),
        Err(Error::Timeout)
    ));
    assert!(matches!(
        raster.embedded_scan(0, &Limits::default()),
        Some(Err(Error::Timeout))
    ));
    // Failing fast, not a timing claim: generous for loaded debug runs.
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    // The extractor hands out a raster bound to the file's deadline.
    let limits = Limits {
        timeout: Duration::from_millis(1),
        ..Limits::default()
    };
    let r = run(bytes, &limits);
    std::thread::sleep(Duration::from_millis(5));
    let raster = r.raster.expect("raster");
    assert!(matches!(
        raster.0.render(0, 36.0, &Limits::default()),
        Err(Error::Timeout)
    ));
}

#[test]
fn issue_38_predictor_divide_by_zero() {
    assert_no_panic_anywhere(&repro::pdf(repro::PREDICTOR_DIV_ZERO), &Limits::default());
}

#[test]
fn issue_38_predictor_row_length_overflow() {
    assert_no_panic_anywhere(
        &repro::pdf(repro::PREDICTOR_MUL_OVERFLOW),
        &Limits::default(),
    );
}

#[test]
fn issue_38_image_budget_charge_overflow() {
    let bytes = repro::pdf(repro::IMAGE_CHARGE_OVERFLOW);
    assert_no_panic_anywhere(&bytes, &Limits::default());
    // The charge saturates instead of wrapping, so the image is refused, not drawn.
    let res = PdfRaster::new(Blob::from(bytes)).render(0, 36.0, &Limits::default());
    assert!(!matches!(res, Err(Error::Panic(_))));
}

#[test]
fn issue_38_glyph_position_overflow() {
    assert_no_panic_anywhere(&repro::pdf(repro::GLYPH_POS_OVERFLOW), &Limits::default());
}

#[test]
fn predictor_with_huge_columns_does_not_size_a_zero_row() {
    // `/Columns 2^31` with 16 colors of 16 bits: the zero row alone would be gigabytes.
    let img = stream_raw(
        "/Type /XObject /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 2147483647 /Colors 16 /BitsPerComponent 16 >>",
        &zlib(&[0u8; 64]),
    );
    let bytes = page_pdf(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        vec![(5, img)],
    );
    let ((), peak) = peak_of(|| assert_no_panic_anywhere(&bytes, &Limits::default()));
    assert!(peak < 128 * MIB, "peak {} MiB", peak / MIB);
}

// ---------------------------------------------------------------------------------------------
// Review findings: text, catalog walks, layout and JPEG 2000 boxes.

#[test]
fn huge_tounicode_strings_are_bounded() {
    // One code maps to 512 Ki UTF-16 units; the page shows it 200 000 times. Every lookup
    // allocates the whole string: kept as is, that is about 100 GB.
    let units = 512 * 1024;
    let mut cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Q def 1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <61> <".to_vec();
    cmap.extend(b"0041".repeat(units));
    cmap.extend_from_slice(
        b"> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end",
    );
    let n = 200_000;
    let mut c = b"BT /F1 1 Tf 10 700 Td (".to_vec();
    c.extend(std::iter::repeat_n(b'a', n));
    c.extend_from_slice(b") Tj ET");
    let bytes = page_pdf(
        &c,
        "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 5 0 R >> >>",
        vec![(5, stream_flate("", &cmap))],
    );
    let t = Instant::now();
    let (r, peak) = peak_of(|| run(bytes, &Limits::default()));
    let doc = r.doc.expect("doc");
    // Each glyph keeps at most 32 chars, and the page looks up at most `max_text_bytes`.
    assert!(!doc.pages[0].text.is_empty());
    assert!(doc.pages[0].text.len() < 8 * MIB);
    assert!(peak < 512 * MIB, "peak {} MiB", peak / MIB);
    assert!(t.elapsed() < Duration::from_mins(1), "{:?}", t.elapsed());
}

#[test]
fn one_huge_text_run_meets_the_call_cap() {
    // A single `Tj` of 5 million glyphs is 5 million draws, over the per-page cap: the page
    // is skipped instead of being walked glyph by glyph past every check.
    let n = 5_000_000;
    let mut c = b"BT /F1 1 Tf 10 700 Td (".to_vec();
    c.extend(std::iter::repeat_n(b'a', n));
    c.extend_from_slice(b") Tj ET");
    let bytes = page_pdf(&c, FONT, vec![]);
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    assert!(r.doc.is_ok());
    assert!(has_problem(&r, "draw operations"), "{:?}", r.problems);
    assert!(t.elapsed() < Duration::from_mins(1), "{:?}", t.elapsed());
}

#[test]
fn shared_wide_field_kids_are_bounded() {
    // Two fields whose `/Kids` list each other 100 000 times: every visit used to resolve the
    // whole array again (20 000 visits × 100 000 kids).
    let kids = |n: u32| format!("{n} 0 R ").repeat(100_000);
    let bytes = build(
        &[
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".to_vec(),
            ),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (
                3,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
            ),
            (
                5,
                format!("<< /T (f) /FT /Tx /Kids [{}] >>", kids(6)).into_bytes(),
            ),
            (6, format!("<< /T (g) /Kids [{}] >>", kids(5)).into_bytes()),
        ],
        1,
        true,
    );
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    assert!(r.doc.is_ok());
    assert!(fact(&r, &keys::FORM_FIELDS).is_some());
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
}

#[test]
fn shared_wide_name_tree_leaves_are_bounded() {
    // 20 000 `/Kids` entries pointing at one leaf of 300 000 names: counting the leaf on
    // every visit was 6 × 10^9 entries.
    let kids = "6 0 R ".repeat(20_000);
    let names = "(k) null ".repeat(300_000);
    let bytes = build(
        &[
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 5 0 R >> >>".to_vec(),
            ),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (
                3,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
            ),
            (5, format!("<< /Kids [{kids}] >>").into_bytes()),
            (6, format!("<< /Names [{names}] >>").into_bytes()),
        ],
        1,
        true,
    );
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    assert!(r.doc.is_ok());
    assert!(fact(&r, &keys::EMBEDDED_FILES).is_some());
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
}

#[test]
fn many_segments_on_one_line_lay_out_in_linear_time() {
    // 200 000 glyphs on one baseline, each 3 em from the next: 200 000 segments of one line,
    // which never closed as blocks (the open list grew to all of them).
    let n = 200_000;
    let mut c = b"BT /F1 1 Tf 10 700 Td [".to_vec();
    for _ in 0..n {
        c.extend_from_slice(b"(a)-3000");
    }
    c.extend_from_slice(b"] TJ ET");
    let bytes = page_pdf(&c, FONT, vec![]);
    let t = Instant::now();
    let r = run(bytes, &Limits::default());
    let doc = r.doc.expect("doc");
    assert!(doc.pages[0].text.len() >= n);
    assert!(t.elapsed() < Duration::from_mins(1), "{:?}", t.elapsed());
}

/// A JP2 box: length, type, body.
fn jp2_box(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = u32::try_from(body.len() + 8)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    b.extend_from_slice(&kind);
    b.extend_from_slice(body);
    b
}

/// SOC + SIZ (+ EOC) of a `w` × `h` × `comps` codestream.
fn j2k_header(w: u32, h: u32, comps: u16) -> Vec<u8> {
    let mut cs = vec![0xFF, 0x4F, 0xFF, 0x51];
    cs.extend_from_slice(&(38 + 3 * comps).to_be_bytes());
    cs.extend_from_slice(&[0, 0]);
    for v in [w, h, 0, 0, w, h, 0, 0] {
        cs.extend_from_slice(&v.to_be_bytes());
    }
    cs.extend_from_slice(&comps.to_be_bytes());
    for _ in 0..comps {
        cs.extend_from_slice(&[7, 1, 1]);
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

#[test]
fn jpx_codestream_boxes_are_checked_not_the_first_siz_anywhere() {
    // A JP2 file with a small decoy codestream in a `uuid` box, then the real `jp2c` box
    // claiming 60 000 × 60 000 × 4: the decoder reads the `jp2c` box.
    let mut jp2 = jp2_box(*b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]);
    jp2.extend(jp2_box(*b"ftyp", b"jp2 \0\0\0\0jp2 "));
    let mut ihdr = Vec::new();
    for v in [16u32, 16] {
        ihdr.extend_from_slice(&v.to_be_bytes());
    }
    ihdr.extend_from_slice(&[0, 1, 7, 7, 0, 0]);
    let mut jp2h = jp2_box(*b"ihdr", &ihdr);
    jp2h.extend(jp2_box(*b"colr", &[1, 0, 0, 0, 0, 0, 17]));
    jp2.extend(jp2_box(*b"jp2h", &jp2h));
    let mut uuid = vec![0u8; 16];
    uuid.extend(j2k_header(16, 16, 1));
    jp2.extend(jp2_box(*b"uuid", &uuid));
    jp2.extend(jp2_box(*b"jp2c", &j2k_header(60_000, 60_000, 4)));
    let img = stream_raw(
        "/Type /XObject /Subtype /Image /Width 16 /Height 16 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /JPXDecode",
        &jp2,
    );
    let bytes = page_pdf(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        vec![(5, img)],
    );
    let ((), peak) = peak_of(|| {
        let raster = PdfRaster::new(Blob::from(bytes));
        // Refused for its size (a limit), not handed to the decoder.
        assert!(
            matches!(
                raster.embedded_scan(0, &Limits::default()),
                Some(Err(Error::LimitExceeded(_)))
            ),
            "the jp2c codestream must be checked"
        );
        let _ = raster.render(0, 72.0, &Limits::default());
    });
    assert!(peak < 128 * MIB, "peak {} MiB", peak / MIB);
}

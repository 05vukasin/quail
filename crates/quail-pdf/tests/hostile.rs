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
    // 40 pages share one 16 MiB content stream: 640 MiB decoded in total, fine under the
    // default budget, but never more than about one parse's cache held at once.
    let bytes = shared_pdf(40, 16, false);
    let (r, peak) = peak_of(|| run(bytes, &Limits::default()));
    let doc = r.doc.expect("doc");
    assert_eq!(doc.pages.len(), 40);
    assert!(doc.pages.iter().all(|p| p.text.contains("shared")));
    assert!(peak < 192 * MIB, "peak {} MiB", peak / MIB);
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
    // Inline images carry no document context in hayro, so their refusal (by the fixed
    // default cap) cannot be reported: the render succeeds without the image. Memory is
    // bounded either way.
    for (bytes, reported) in [(in_form, true), (inline, false), (in_pattern, true)] {
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
    let mut c = b"BT /F1 10 Tf".to_vec();
    for (i, y) in [700, 688, 640, 628].iter().enumerate() {
        c.extend_from_slice(format!(" 1 0 0 1 72 {y} Tm (left {i}) Tj").as_bytes());
        c.extend_from_slice(format!(" 1 0 0 1 330 {y} Tm (right {i}) Tj").as_bytes());
    }
    c.extend_from_slice(b" ET");
    let r = run(page_pdf(&c, FONT, vec![]), &Limits::default());
    let doc = r.doc.expect("doc");
    let t = &doc.pages[0].text;
    let pos = |s: &str| t.find(s).expect(s);
    assert!(pos("left 3") < pos("right 0"), "{t:?}");
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

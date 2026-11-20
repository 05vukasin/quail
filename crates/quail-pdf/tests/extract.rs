//! End-to-end tests on PDFs generated in code.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::missing_panics_doc
)]

mod common;

use std::time::{Duration, Instant};

use common::{Extras, H, PageSpec, W, line, pdf, text_page};
use quail_core::{
    Blob, Document, Emitter, Error, Extractor, Fact, FactKey, Format, Limits, TextSource, Value,
    Zone,
};
use quail_pdf::{PdfExtractor, PdfPages, keys};

struct Run {
    doc: Result<Document, Error>,
    facts: Vec<Fact>,
    warnings: Vec<String>,
    pages: Option<PdfPages>,
}

fn run_with(bytes: Vec<u8>, limits: &Limits, out: Emitter) -> Run {
    let mut out = out;
    let doc = PdfExtractor::new().extract(&Blob::from(bytes), limits, &mut out);
    let (facts, artifacts, warnings) = out.into_parts();
    Run {
        doc,
        facts,
        warnings,
        pages: artifacts.get::<PdfPages>().cloned(),
    }
}

fn run(bytes: Vec<u8>) -> Run {
    run_with(bytes, &Limits::default(), Emitter::new(PdfExtractor::ID))
}

fn fact<'a>(r: &'a Run, k: &FactKey) -> Option<&'a Value> {
    r.facts.iter().find(|f| &f.key == k).map(|f| &f.value)
}

fn int(r: &Run, k: &FactKey) -> i64 {
    match fact(r, k) {
        Some(Value::Int(n)) => *n,
        other => panic!("{k}: {other:?}"),
    }
}

fn boolean(r: &Run, k: &FactKey) -> bool {
    match fact(r, k) {
        Some(Value::Bool(b)) => *b,
        other => panic!("{k}: {other:?}"),
    }
}

#[test]
fn ids() {
    let e = PdfExtractor::new();
    assert_eq!(e.id().to_string(), "pdf.extract@1");
    assert_eq!(e.formats(), &[Format::Pdf]);
}

#[test]
fn multi_page_text_with_positions() {
    let bytes = pdf(
        &[text_page(1, 5), text_page(2, 5), text_page(3, 5)],
        &Extras {
            info: vec![
                ("Producer", "quail tests"),
                ("Title", "Synthetic"),
                ("CreationDate", "D:20240131120000Z"),
            ],
            compress: true,
            ..Extras::default()
        },
    );
    let r = run(bytes);
    let doc = r.doc.as_ref().expect("doc");
    assert_eq!(doc.pages.len(), 3);
    for (i, p) in doc.pages.iter().enumerate() {
        assert_eq!(p.size, Some((W, H)));
        assert_eq!(p.text_source, TextSource::Native);
        assert!(!p.needs_ocr);
        let first = p.text.lines().next().expect("line");
        assert_eq!(
            first,
            format!("Page {} body line 0 lorem ipsum dolor sit amet", i + 1)
        );
        // Lines in reading order.
        let order: Vec<usize> = (0..5)
            .map(|n| p.text.find(&format!("line {n} ")).expect("present"))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]));
        for b in &p.blocks {
            assert!(!b.span.slice(&p.text).is_empty());
            let bb = b.bbox.expect("bbox");
            assert!(bb.x0 >= 60.0 && bb.x1 <= W && bb.y0 >= 130.0 && bb.y1 <= H);
            assert_eq!(b.zone, Zone::Body);
        }
    }
    // First line baseline at 150 pt from the top: the box must sit just above it.
    let bb = doc.pages[0].blocks[0].bbox.expect("bbox");
    assert!((bb.y0 - (150.0 - 0.8 * 11.0)).abs() < 2.0, "{bb:?}");
    assert!((bb.x0 - 72.0).abs() < 0.5, "{bb:?}");

    assert_eq!(int(&r, &keys::PAGE_COUNT), 3);
    assert_eq!(fact(&r, &keys::VERSION), Some(&Value::text("1.7")));
    assert_eq!(fact(&r, &keys::PRODUCER), Some(&Value::text("quail tests")));
    assert_eq!(fact(&r, &keys::TITLE), Some(&Value::text("Synthetic")));
    assert_eq!(
        fact(&r, &keys::CREATED),
        Some(&Value::Date(quail_core::Date {
            y: 2024,
            m: 1,
            d: 31
        }))
    );
    assert_eq!(
        fact(&r, &keys::FONTS),
        Some(&Value::List(vec![Value::text("Helvetica")]))
    );
    assert!(!boolean(&r, &keys::ENCRYPTED));
    assert_eq!(int(&r, &keys::PAGES_NEEDING_OCR), 0);
    assert_eq!(int(&r, &keys::INCREMENTAL_UPDATES), 0);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn header_and_footer_by_repetition_and_geometry() {
    let pages: Vec<PageSpec> = (1..=4)
        .map(|n| {
            let mut p = text_page(n, 4);
            // 9 % from the top: header only because it repeats.
            p.lines
                .push(line(72.0, 75.0, "Synthetic Corp quarterly report"));
            // Inside the bottom band, repeats with a changing number.
            p.lines.push(line(260.0, 760.0, &format!("Page {n} of 4")));
            p
        })
        .collect();
    let r = run(pdf(&pages, &Extras::default()));
    let doc = r.doc.expect("doc");
    for p in &doc.pages {
        let zone_of = |needle: &str| {
            p.blocks
                .iter()
                .find(|b| b.span.slice(&p.text).contains(needle))
                .map(|b| b.zone)
        };
        assert_eq!(zone_of("quarterly report"), Some(Zone::Header));
        assert_eq!(zone_of(" of 4"), Some(Zone::Footer));
        assert_eq!(zone_of("body line 2"), Some(Zone::Body));
        // Reading order: header first, footer last.
        assert!(p.text.starts_with("Synthetic Corp"));
        assert!(p.text.ends_with("of 4"));
    }
}

#[test]
fn image_only_page_needs_ocr() {
    let scan = PageSpec {
        lines: vec![],
        full_page_image: true,
    };
    let r = run(pdf(&[text_page(1, 3), scan], &Extras::default()));
    let doc = r.doc.as_ref().expect("doc");
    assert!(!doc.pages[0].needs_ocr);
    let p = &doc.pages[1];
    assert!(p.needs_ocr);
    assert_eq!(p.text_source, TextSource::None);
    assert!(p.text.is_empty());
    assert_eq!(int(&r, &keys::IMAGE_ONLY_PAGES), 1);
    assert_eq!(int(&r, &keys::PAGES_NEEDING_OCR), 1);
    let info = r.pages.as_ref().expect("artifact");
    assert_eq!(info.pages.len(), 2);
    assert!(info.pages[1].image_only);
    assert_eq!(info.pages[1].image_count, 1);
    assert!(info.pages[1].image_coverage > 0.99);
    assert_eq!(info.pages[1].images[0].width, 4);
}

#[test]
fn ocrd_scan_with_text_layer_is_native() {
    let mut p = text_page(1, 30);
    p.full_page_image = true;
    let r = run(pdf(&[p], &Extras::default()));
    let doc = r.doc.expect("doc");
    assert!(!doc.pages[0].needs_ocr);
    assert_eq!(doc.pages[0].text_source, TextSource::Native);
}

#[test]
fn user_password_is_reported_as_encrypted() {
    let r = run(common::encrypted(&[text_page(1, 3)], "secret"));
    assert!(matches!(r.doc, Err(Error::Encrypted)), "{:?}", r.doc);
    assert!(boolean(&r, &keys::ENCRYPTED));
    // What is readable without a password is still there.
    assert!(fact(&r, &keys::VERSION).is_some());
    assert!(fact(&r, &keys::INCREMENTAL_UPDATES).is_some());
}

#[test]
fn owner_only_encryption_is_read() {
    let r = run(common::encrypted(&[text_page(1, 3)], ""));
    let doc = r.doc.as_ref().expect("doc");
    assert!(boolean(&r, &keys::ENCRYPTED));
    assert!(doc.pages[0].text.contains("body line 2"));
}

#[test]
fn broken_xref_is_repaired() {
    let mut bytes = pdf(&[text_page(1, 3), text_page(2, 3)], &Extras::default());
    // Point startxref into the void and smash the xref table offsets.
    let at = bytes
        .windows(9)
        .rposition(|w| w == b"startxref")
        .expect("startxref");
    let tail = b"startxref\n999999999\n%%EOF\n";
    bytes.truncate(at);
    if let Some(x) = bytes.windows(4).rposition(|w| w == b"xref") {
        for b in &mut bytes[x + 4..] {
            if b.is_ascii_digit() {
                *b = b'7';
            }
        }
    }
    bytes.extend_from_slice(tail);
    let r = run(bytes);
    let doc = r.doc.as_ref().expect("repaired");
    assert_eq!(doc.pages.len(), 2);
    assert!(doc.pages[1].text.contains("Page 2 body line 1"));
}

#[test]
fn incremental_update_is_counted() {
    let mut bytes = pdf(&[text_page(1, 2)], &Extras::default());
    let at = bytes
        .windows(9)
        .rposition(|w| w == b"startxref")
        .expect("startxref");
    let trailer = bytes[at..].to_vec();
    bytes.extend_from_slice(b"\n% appended revision\n");
    bytes.extend_from_slice(&trailer);
    let r = run(bytes);
    assert_eq!(int(&r, &keys::INCREMENTAL_UPDATES), 1);
}

#[test]
fn document_features() {
    let r = run(pdf(
        &[text_page(1, 2)],
        &Extras {
            forms: true,
            javascript: true,
            embedded_file: true,
            xmp: true,
            ..Extras::default()
        },
    ));
    assert!(r.doc.is_ok());
    assert_eq!(int(&r, &keys::FORM_FIELDS), 3);
    assert_eq!(int(&r, &keys::SIGNATURE_FIELDS), 2);
    assert!(boolean(&r, &keys::SIGNED));
    assert!(boolean(&r, &keys::HAS_JAVASCRIPT));
    assert_eq!(int(&r, &keys::EMBEDDED_FILES), 1);
    assert!(boolean(&r, &keys::HAS_XMP));

    let plain = run(pdf(&[text_page(1, 2)], &Extras::default()));
    assert_eq!(int(&plain, &keys::FORM_FIELDS), 0);
    assert!(!boolean(&plain, &keys::SIGNED));
    assert!(!boolean(&plain, &keys::HAS_JAVASCRIPT));
    assert!(!boolean(&plain, &keys::HAS_XMP));
}

#[test]
fn two_columns_read_column_by_column() {
    let mut p = PageSpec::default();
    for i in 0..6 {
        let top = 150.0 + 14.0 * i as f32;
        p.lines
            .push(line(72.0, top, &format!("left column row {i}")));
        p.lines
            .push(line(330.0, top, &format!("right column row {i}")));
    }
    let r = run(pdf(&[p], &Extras::default()));
    let doc = r.doc.expect("doc");
    let t = &doc.pages[0].text;
    let last_left = t.find("left column row 5").expect("left");
    let first_right = t.find("right column row 0").expect("right");
    assert!(last_left < first_right, "{t}");
}

#[test]
fn max_pages_is_respected() {
    let pages: Vec<PageSpec> = (1..=5).map(|n| text_page(n, 1)).collect();
    let limits = Limits {
        max_pages: 2,
        ..Limits::default()
    };
    let r = run_with(
        pdf(&pages, &Extras::default()),
        &limits,
        Emitter::new(PdfExtractor::ID),
    );
    assert_eq!(r.doc.as_ref().expect("doc").pages.len(), 2);
    assert_eq!(int(&r, &keys::PAGE_COUNT), 5);
    assert!(r.warnings.iter().any(|w| w.contains("max_pages")));
}

#[test]
fn max_bytes_is_respected() {
    let limits = Limits {
        max_bytes: 10,
        ..Limits::default()
    };
    let r = run_with(
        pdf(&[text_page(1, 1)], &Extras::default()),
        &limits,
        Emitter::new(PdfExtractor::ID),
    );
    assert!(matches!(r.doc, Err(Error::LimitExceeded("max_bytes"))));
}

#[test]
fn max_text_bytes_truncates() {
    let limits = Limits {
        max_text_bytes: 200,
        ..Limits::default()
    };
    let pages: Vec<PageSpec> = (1..=3).map(|n| text_page(n, 10)).collect();
    let r = run_with(
        pdf(&pages, &Extras::default()),
        &limits,
        Emitter::new(PdfExtractor::ID),
    );
    let doc = r.doc.expect("doc");
    let total: usize = doc.pages.iter().map(|p| p.text.len()).sum();
    assert!(total <= 200);
    assert!(r.warnings.iter().any(|w| w.contains("max_text_bytes")));
}

#[test]
fn deadline_gives_partial_result() {
    let past = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .expect("instant");
    let r = run_with(
        pdf(&[text_page(1, 1), text_page(2, 1)], &Extras::default()),
        &Limits::default(),
        Emitter::with_deadline(PdfExtractor::ID, past),
    );
    assert_eq!(r.doc.as_ref().expect("doc").pages.len(), 0);
    assert!(r.warnings.iter().any(|w| w.contains("timeout")));
    assert_eq!(int(&r, &keys::PAGE_COUNT), 2);
}

#[test]
fn decompression_bomb_is_stopped() {
    // A content stream that inflates to 64 MiB of spaces, behind a 1 MiB budget.
    let mut doc = common::document(&[text_page(1, 1)], &Extras::default());
    let bomb = vec![b' '; 64 << 20];
    let mut s = lopdf::Stream::new(lopdf::Dictionary::new(), bomb);
    s.compress().expect("compress");
    let bomb_id = doc.add_object(s);
    let page_id = *doc.get_pages().values().next().expect("page");
    if let Ok(lopdf::Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Contents", bomb_id);
    }
    let bytes = common::save(&mut doc);
    assert!(bytes.len() < 1 << 20);
    let limits = Limits {
        max_decompressed: 1 << 20,
        ..Limits::default()
    };
    let r = run_with(bytes, &limits, Emitter::new(PdfExtractor::ID));
    assert_eq!(r.doc.expect("doc").pages.len(), 0);
    assert!(r.warnings.iter().any(|w| w.contains("max_decompressed")));
}

#[test]
fn garbage_never_panics() {
    for bytes in [
        Vec::new(),
        b"%PDF-1.7".to_vec(),
        b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF"
            .to_vec(),
        vec![0xFF; 4096],
    ] {
        let r = run(bytes);
        assert!(r.doc.is_err() || r.doc.is_ok());
    }
}

#[test]
fn mutated_files_never_panic() {
    let base = pdf(
        &[text_page(1, 4), text_page(2, 4)],
        &Extras {
            forms: true,
            compress: true,
            ..Extras::default()
        },
    );
    // Deterministic xorshift: flip bytes and truncate at many places.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..300 {
        let mut b = base.clone();
        let flips = 1 + (next() % 8) as usize;
        for _ in 0..flips {
            let i = (next() as usize) % b.len();
            b[i] = (next() & 0xFF) as u8;
        }
        if round % 5 == 0 {
            let cut = (next() as usize) % b.len();
            b.truncate(cut);
        }
        let _ = run(b);
    }
}

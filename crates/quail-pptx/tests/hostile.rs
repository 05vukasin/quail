//! Hostile packages built in code: bombs, entities, deep nesting, lying directories,
//! limits and the deadline. Each must end quickly, never fail on a readable zip, and
//! report a partial result when it stopped short.
//!
//! The counting allocator is the only `unsafe` in the crate: it is test-only and forwards to
//! the system allocator unchanged.
#![allow(unsafe_code)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

use common::{Pptx, paras, run, run_with, slide, sp, zip_of};
use quail_core::{Blob, Emitter, Error, Extractor, Limits};
use quail_pptx::PptxExtractor;
use zip::CompressionMethod;

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
        // SAFETY: forwarded unchanged; the caller upholds `GlobalAlloc::alloc_zeroed`'s
        // contract.
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

fn quick<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let v = f();
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "{what} took {:?}",
        t.elapsed()
    );
    v
}

fn text_slide(t: &str) -> String {
    slide(&sp(2, None, Some((0, 0)), &paras(&[t])))
}

#[test]
fn a_deflate_bomb_stops_at_the_budget() {
    // 64 MiB of whitespace inside a slide deflates to a few KiB; the budget is 4 MiB.
    let first = text_slide("first slide");
    let mut bomb = slide(&sp(2, None, Some((0, 0)), &paras(&["bomb slide"])));
    bomb.insert_str(bomb.len() - "</p:sld>".len(), &" ".repeat(64 << 20));
    let p = Pptx::of(&[&first, &bomb, &text_slide("never read")]);
    let limits = Limits {
        max_decompressed: 4 << 20,
        ..Limits::default()
    };
    let bytes = p.build();
    assert!(bytes.len() < 1 << 20);
    let r = quick("bomb", || run_with(&bytes, &limits));
    let t = r.texts();
    assert_eq!(t.first().map(String::as_str), Some("first slide"));
    assert!(!t.iter().any(|s| s == "never read"));
    let p = r.problems();
    assert!(
        p.iter().any(|p| p.contains("decompression budget")),
        "{p:?}"
    );
}

#[test]
fn doctype_entities_are_not_expanded() {
    let lol = r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;"><!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;"><!ENTITY xxe SYSTEM "file:///etc/passwd">]>"#;
    let s = text_slide("a&lol3;b&xxe;c").replacen(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        lol,
        1,
    );
    let r = run(&Pptx::of(&[&s]).build());
    assert_eq!(r.texts(), vec!["abc"]);
}

#[test]
fn deep_nesting_stays_bounded() {
    // A million nested groups, then a shape.
    let open = "<p:grpSp><p:grpSpPr/>".repeat(1_000_000);
    let close = "</p:grpSp>".repeat(1_000_000);
    let s = slide(&format!(
        "{open}{}{close}",
        sp(2, None, Some((0, 0)), &paras(&["deep"]))
    ));
    let r = quick("groups", || run(&Pptx::of(&[&s]).build()));
    assert_eq!(r.texts(), vec!["deep"]);
    // A million nested unknown elements inside a paragraph.
    let s = slide(&sp(
        2,
        None,
        Some((0, 0)),
        &format!(
            "<a:p><a:r><a:t>x</a:t>{}{}</a:r></a:p>",
            "<a:x>".repeat(1_000_000),
            "</a:x>".repeat(1_000_000)
        ),
    ));
    let r = quick("elements", || run(&Pptx::of(&[&s]).build()));
    assert_eq!(r.texts(), vec!["x"]);
}

#[test]
fn many_shapes_and_paragraphs_are_capped() {
    let s = slide(&sp(
        2,
        None,
        Some((0, 0)),
        &"<a:p><a:r><a:t>p</a:t></a:r></a:p>".repeat(300_000),
    ));
    let r = quick("paragraphs", || run(&Pptx::of(&[&s]).build()));
    assert_eq!(r.texts().len(), 100_000 - 1);
    let w = r.warnings();
    assert!(
        w.iter().any(|w| w.contains("more than 100000 shapes")),
        "{w:?}"
    );
}

#[test]
fn text_and_page_limits() {
    let slides: Vec<String> = (0..50)
        .map(|i| text_slide(&format!("slide number {i} with some text")))
        .collect();
    let refs: Vec<&str> = slides.iter().map(String::as_str).collect();
    let bytes = Pptx::of(&refs).build();
    let r = run_with(
        &bytes,
        &Limits {
            max_pages: 5,
            ..Limits::default()
        },
    );
    assert_eq!(r.doc.pages.len(), 5);
    assert!(r.problems().iter().any(|p| p.contains("max_pages")));
    let r = run_with(
        &bytes,
        &Limits {
            max_text_bytes: 100,
            ..Limits::default()
        },
    );
    let total: usize = r.doc.pages.iter().map(|p| p.text.len()).sum();
    assert!(total <= 100, "{total}");
    assert!(r.problems().iter().any(|p| p.contains("text limit")));
    // A huge paragraph is cut, not buffered whole.
    let s = text_slide(&"w".repeat(3 << 20));
    let r = run(&Pptx::of(&[&s]).build());
    assert_eq!(r.texts()[0].len(), 1 << 20);
    assert!(r.problems().iter().any(|p| p.contains("over 1 MiB")));
}

#[test]
fn a_huge_directory_is_refused_before_parsing() {
    let files: Vec<(String, Vec<u8>)> =
        (0..20_001).map(|i| (format!("e{i}"), Vec::new())).collect();
    let bytes = zip_of(&files, CompressionMethod::Stored);
    let blob = Blob::new(bytes);
    let mut out = Emitter::new(quail_pptx::ID);
    let r = PptxExtractor::new().extract(&blob, &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::LimitExceeded(_))), "{r:?}");
}

#[test]
fn not_a_presentation() {
    let ex = |b: Vec<u8>| {
        let mut out = Emitter::new(quail_pptx::ID);
        PptxExtractor::new().extract(&Blob::new(b), &Limits::default(), &mut out)
    };
    assert!(matches!(
        ex(b"not a zip".to_vec()),
        Err(Error::Malformed { .. })
    ));
    let cfb = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0];
    assert!(matches!(ex(cfb.to_vec()), Err(Error::Encrypted)));
    let empty = zip_of(
        &[("a.txt".to_owned(), b"x".to_vec())],
        CompressionMethod::Stored,
    );
    assert!(matches!(ex(empty), Err(Error::Malformed { .. })));
}

#[test]
fn broken_parts_degrade() {
    // A slide listed twice, one missing, one malformed, a relationship that climbs out.
    let mut p = Pptx::of(&[
        &text_slide("good"),
        "<p:sld><p:cSld><unclosed",
        &text_slide("third"),
    ]);
    p.order = Some(vec![1, 1, 9, 2, 3]);
    let r = run(&p.build());
    assert_eq!(r.texts(), vec!["good", "third"]);
    let w = r.warnings();
    assert!(w.iter().any(|w| w.contains("missing")), "{w:?}");
    assert!(w.iter().any(|w| w.contains("listed again")), "{w:?}");
}

#[test]
fn a_slide_listed_many_times_is_read_once() {
    // Fuzz finding (quail #141, `oom-abort@sketch.minhash_compat@3`): `sldIdLst` naming one
    // 740 KB slide (3 KB deflated) 577 times. Each entry read the slide again, so an 8 KB file
    // filled the 64 MiB text limit, and the sketches over that text aborted on a 256 MiB
    // allocation. A slide part, and a notes part shared by slides, is now read once.
    let words = "Zaključen dana 15.03.2024. godine u Beogradu između ugovornih strana. ";
    let big = text_slide(&words.repeat(14_000)); // ~1 MiB of text
    let mut p = Pptx::new(vec![
        common::Slide {
            xml: big,
            notes: Some(common::notes(&words.repeat(14_000))),
        },
        common::Slide {
            xml: text_slide("second"),
            notes: Some(common::notes("unused")),
        },
    ]);
    p.order = Some((0..1000).map(|k| 1 + k % 2).collect());
    p.shared_notes = true;
    let bytes = p.build();
    assert!(bytes.len() < 64 << 10, "{}", bytes.len());
    let (r, peak) = peak_of(|| quick("repeats", || run(&bytes)));
    let text: usize = r.doc.pages.iter().map(|p| p.text.len()).sum();
    assert!(peak < 16 * MIB, "peak {} MiB", peak / MIB);
    assert!(text < 3 * MIB, "text {text}");
    assert_eq!(r.doc.pages.len(), 2);
    assert_eq!(r.int("pptx.slides"), Some(1000));
    assert_eq!(r.int("pptx.notes_slides"), Some(1));
    let w = r.warnings();
    assert!(
        w.iter().any(|w| w.contains("998 slides listed again")),
        "{w:?}"
    );
}

#[test]
fn past_deadline_is_partial() {
    let bytes = Pptx::of(&[&text_slide("x")]).build();
    let mut out = Emitter::with_deadline(quail_pptx::ID, Instant::now());
    let doc = PptxExtractor::new()
        .extract(&Blob::new(bytes), &Limits::default(), &mut out)
        .unwrap();
    assert!(doc.pages.iter().all(|p| p.text.is_empty()));
    assert!(out.take_problems().iter().any(|p| p.contains("deadline")));
}

#[test]
fn truncated_packages_never_panic() {
    let bytes = Pptx::of(&[&text_slide("alpha"), &text_slide("beta")]).build();
    for cut in (0..bytes.len()).step_by(7) {
        let mut out = Emitter::new(quail_pptx::ID);
        let _ = PptxExtractor::new().extract(
            &Blob::new(bytes[..cut].to_vec()),
            &Limits::default(),
            &mut out,
        );
    }
}

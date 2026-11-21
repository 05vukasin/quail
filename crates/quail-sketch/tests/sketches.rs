//! Behaviour of every sketch on generated documents: identical inputs give distance 0, small
//! edits small distances, unrelated inputs large ones; masking makes filled-in forms collide.

#![allow(clippy::many_single_char_names)]

mod common;

use std::sync::Arc;

use common::{boxed_page, form, join, kinds_page, prose, run, text_doc};
use quail_core::{
    Artifacts, BlockKind, Document, Image, Limits, PageRaster, Raster, Result, Value,
};
use quail_sketch::layout::{
    LAYOUT_HAMMING_MAX, STRUCTURE_HAMMING_MAX, geometry_sketch, structure_sketch,
};
use quail_sketch::minhash::{MIN_SHINGLES, text_minhash};
use quail_sketch::simhash::{TEXT_HAMMING_MAX, text_simhash};
use quail_sketch::template::template_simhash;
use quail_sketch::{
    CompatMinHash, InvoiceLayout, LayoutAnalyzer, MinHashAnalyzer, PagePHash, SimHashTemplate,
    SimHashText, TextKey, hamming, jaccard_estimate, keys,
};

fn edited(words: &[String], every: usize) -> Vec<String> {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i % every == every / 2 {
                format!("{w}x")
            } else {
                w.clone()
            }
        })
        .collect()
}

#[test]
fn minhash_identical_edited_unrelated() {
    let a = prose(1, 600);
    let base = text_minhash(&join(&a)).unwrap();
    let same = text_minhash(&join(&a)).unwrap();
    assert!((base.jaccard(&same) - 1.0).abs() < f64::EPSILON);
    assert_eq!(base.bands(), same.bands());

    // One word in 100 changed: about 3 % of the shingles.
    let small = text_minhash(&join(&edited(&a, 100))).unwrap();
    let j = base.jaccard(&small);
    assert!(j >= 0.85, "small edit J = {j}");
    assert!(base.bands().iter().zip(small.bands()).any(|(x, y)| *x == y));

    let other = text_minhash(&join(&prose(2, 600))).unwrap();
    let j = base.jaccard(&other);
    assert!(j < 0.1, "unrelated J = {j}");

    // Stored bytes compare the same way.
    assert!(
        (jaccard_estimate(&base.to_bytes(), &small.to_bytes()) - base.jaccard(&small)).abs()
            < 1e-12
    );
}

#[test]
fn minhash_ignores_case_diacritics_and_script() {
    let lat = "Ugovor o zakupu poslovnog prostora zaključen između zakupodavca i zakupca";
    let cyr = "УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА ЗАКЉУЧЕН ИЗМЕЂУ ЗАКУПОДАВЦА И ЗАКУПЦА";
    assert_eq!(text_minhash(lat), text_minhash(cyr));
}

#[test]
fn minhash_analyzer_gates_short_bodies() {
    let facts = run(
        &MinHashAnalyzer,
        text_doc("too short to judge anything"),
        None,
    );
    assert!(facts.is_empty());
    let facts = run(&MinHashAnalyzer, text_doc(&join(&prose(3, 300))), None);
    let keys_seen: Vec<&str> = facts.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        keys_seen,
        [
            "sketch.minhash",
            "sketch.minhash_bands",
            "sketch.minhash_shingles"
        ]
    );
    match &facts[0].value {
        Value::Bytes(b) => assert_eq!(b.len(), 512),
        v => panic!("{v:?}"),
    }
    match &facts[1].value {
        Value::List(l) => assert_eq!(l.len(), 32),
        v => panic!("{v:?}"),
    }
    match facts[2].value {
        Value::Int(n) => assert!(usize::try_from(n).unwrap() >= MIN_SHINGLES),
        ref v => panic!("{v:?}"),
    }
}

#[test]
fn simhash_text_distances() {
    let a = prose(11, 800);
    let norm = |w: &[String]| join(w);
    let (h, _) = text_simhash(&norm(&a)).unwrap();
    assert_eq!(text_simhash(&norm(&a)).unwrap().0, h);
    let (s, _) = text_simhash(&norm(&edited(&a, 100))).unwrap();
    assert!(
        hamming(h, s) <= TEXT_HAMMING_MAX,
        "small edit: {}",
        hamming(h, s)
    );
    let (u, _) = text_simhash(&norm(&prose(12, 800))).unwrap();
    assert!(hamming(h, u) > 16, "unrelated: {}", hamming(h, u));
    assert_eq!(run(&SimHashText, text_doc(&norm(&a)), None).len(), 1);
}

#[test]
fn template_masking_makes_forms_collide() {
    let (a, _) = template_simhash(&form(21, 1)).unwrap();
    let (b, _) = template_simhash(&form(21, 2)).unwrap();
    let (c, _) = template_simhash(&form(22, 1)).unwrap();
    // Same form, other particulars: one template.
    assert!(hamming(a, b) <= 2, "same form: {}", hamming(a, b));
    // Another form: far apart.
    assert!(hamming(a, c) > 16, "other form: {}", hamming(a, c));
    // While the content sketches tell the two fills apart.
    let ma = text_minhash(&form(21, 1)).unwrap();
    let mb = text_minhash(&form(21, 2)).unwrap();
    assert!(ma.jaccard(&mb) < 0.95);
    let (ta, _) = text_simhash(&form(21, 1)).unwrap();
    let (tb, _) = text_simhash(&form(21, 2)).unwrap();
    assert!(hamming(ta, tb) > hamming(a, b));
    assert_eq!(run(&SimHashTemplate, text_doc(&form(21, 1)), None).len(), 1);
}

#[test]
fn compat_template_masking_makes_forms_collide() {
    let a = quail_sketch::compat::template_fp(&form(21, 1)).unwrap();
    let b = quail_sketch::compat::template_fp(&form(21, 2)).unwrap();
    let c = quail_sketch::compat::template_fp(&form(22, 1)).unwrap();
    assert!(hamming(a, b) <= quail_sketch::compat::TEMPLATE_HAMMING_MAX);
    assert!(hamming(a, c) > 16);
}

fn grid(rows: usize, shift: f32) -> Vec<(f32, f32, f32, f32)> {
    let mut v = vec![(60.0, 30.0, 300.0, 50.0)];
    for r in 0..rows {
        #[allow(clippy::cast_precision_loss)]
        let y = 100.0 + r as f32 * 50.0 + shift;
        v.push((60.0, y, 530.0, y + 36.0));
    }
    v
}

#[test]
fn layout_ignores_text_and_follows_geometry() {
    let a = Document {
        pages: vec![
            boxed_page(0, &grid(12, 0.0), &["alpha beta"]),
            boxed_page(1, &grid(10, 0.0), &["x"]),
        ],
    };
    let b = Document {
        pages: vec![
            boxed_page(0, &grid(12, 0.0), &["completely different words here"]),
            boxed_page(1, &grid(10, 0.0), &["y z"]),
        ],
    };
    let (ha, n) = geometry_sketch(&a).unwrap();
    assert_eq!(n, 13 + 11);
    assert_eq!(geometry_sketch(&b).unwrap().0, ha);
    // Text grew by a line: blocks below move 3 points.
    let c = Document {
        pages: vec![
            boxed_page(0, &grid(12, 3.0), &["a"]),
            boxed_page(1, &grid(10, 0.0), &["x"]),
        ],
    };
    let hc = geometry_sketch(&c).unwrap().0;
    assert!(
        hamming(ha, hc) <= LAYOUT_HAMMING_MAX,
        "shifted: {}",
        hamming(ha, hc)
    );
    // Two columns instead of one.
    let cols: Vec<(f32, f32, f32, f32)> = (0..12)
        .flat_map(|r| {
            #[allow(clippy::cast_precision_loss)]
            let y = 80.0 + r as f32 * 60.0;
            [(40.0, y, 280.0, y + 20.0), (320.0, y, 560.0, y + 20.0)]
        })
        .collect();
    let d = Document {
        pages: vec![boxed_page(0, &cols, &["a"])],
    };
    let hd = geometry_sketch(&d).unwrap().0;
    assert!(hamming(ha, hd) > 16, "other layout: {}", hamming(ha, hd));
    let facts = run(&LayoutAnalyzer, a, None);
    assert!(facts.iter().any(|f| f.key == keys::LAYOUT));
}

fn contract_kinds(extra_paragraphs: usize) -> Vec<BlockKind> {
    let mut k = vec![BlockKind::Heading { level: 1 }];
    for s in 0..6u16 {
        k.push(BlockKind::Heading { level: 2 });
        k.extend(std::iter::repeat_n(
            BlockKind::Paragraph,
            3 + usize::from(s % 3),
        ));
        k.extend(std::iter::repeat_n(BlockKind::ListItem, 2));
        if s == 3 {
            for row in 0..4 {
                for col in 0..3 {
                    k.push(BlockKind::TableCell { table: 0, row, col });
                }
            }
        }
    }
    k.extend(std::iter::repeat_n(BlockKind::Paragraph, extra_paragraphs));
    k
}

#[test]
fn structure_ignores_text_and_follows_kinds() {
    let a = Document {
        pages: vec![kinds_page(&contract_kinds(0), 1)],
    };
    let b = Document {
        pages: vec![kinds_page(&contract_kinds(0), 2)],
    };
    let (ha, _) = structure_sketch(&a).unwrap();
    assert_eq!(structure_sketch(&b).unwrap().0, ha);
    let c = Document {
        pages: vec![kinds_page(&contract_kinds(1), 1)],
    };
    let hc = structure_sketch(&c).unwrap().0;
    assert!(
        hamming(ha, hc) <= STRUCTURE_HAMMING_MAX,
        "one more paragraph: {}",
        hamming(ha, hc)
    );
    let other: Vec<BlockKind> = (0..40u16)
        .map(|i| match i % 5 {
            0 => BlockKind::Heading { level: 3 },
            1 | 2 => BlockKind::Note,
            _ => BlockKind::ListItem,
        })
        .collect();
    let d = Document {
        pages: vec![kinds_page(&other, 1)],
    };
    let hd = structure_sketch(&d).unwrap().0;
    assert!(hamming(ha, hd) > 16, "other structure: {}", hamming(ha, hd));
    // All paragraphs: nothing to compare.
    let flat = Document {
        pages: vec![kinds_page(&[BlockKind::Paragraph; 30], 1)],
    };
    assert!(structure_sketch(&flat).is_none());
    let facts = run(&LayoutAnalyzer, a, None);
    let names: Vec<&str> = facts.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(names, ["sketch.structure", "sketch.headings"]);
}

#[test]
fn invoice_layout_needs_a_label() {
    assert!(run(&InvoiceLayout, text_doc(&join(&prose(5, 100))), None).is_empty());
    let inv = "INVOICE NO AB-2041\nInvoice date 12.05.2023\nBill to Somebody\nSubtotal 100\nTotal due 120";
    let other = "INVOICE NO AB-3999\nInvoice date 01.06.2023\nBill to Somebody Else\nSubtotal 900\nTotal due 999";
    let a = run(&InvoiceLayout, text_doc(inv), None);
    let b = run(&InvoiceLayout, text_doc(other), None);
    assert_eq!(a.len(), 3);
    assert_eq!(a[0].value, b[0].value);
    assert_eq!(a[1].value, Value::Int(5));
}

#[test]
fn text_key_same_words_any_bytes() {
    let words = join(&prose(7, 200));
    let a = quail_sketch::text_key(&words).unwrap();
    let b = quail_sketch::text_key(&words.to_uppercase().replace(' ', "   ")).unwrap();
    assert_eq!(a, b);
    assert!(quail_sketch::text_key("short").is_none());
    assert_eq!(run(&TextKey, text_doc(&words), None).len(), 1);
}

#[test]
fn compat_analyzer_emits_compatible_sketches() {
    let facts = run(&CompatMinHash, text_doc(&join(&prose(9, 300))), None);
    let names: Vec<&str> = facts.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        names,
        [
            "sketch.compat_minhash",
            "sketch.compat_minhash_bands",
            "sketch.compat_shingles",
            "sketch.compat_template"
        ]
    );
    match &facts[0].value {
        Value::Bytes(b) => assert_eq!(b.len(), 1024),
        v => panic!("{v:?}"),
    }
    let short = run(&CompatMinHash, text_doc("a few words"), None);
    assert_eq!(short.len(), 1, "template only");
}

struct Pictures;

impl PageRaster for Pictures {
    fn render(&self, page: u32, _dpi: f32, _limits: &Limits) -> Result<Image> {
        let (w, h) = (60u32, 80u32);
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if page == 0 { x * 4 } else { y * 3 };
                data.push(u8::try_from(v.min(255)).unwrap());
            }
        }
        Ok(Image {
            width: w,
            height: h,
            channels: 1,
            data,
        })
    }
}

#[test]
fn page_phash_hashes_pages_needing_ocr() {
    let mut p0 = common::text_page(0, "");
    p0.needs_ocr = true;
    let mut p1 = common::text_page(1, "");
    p1.needs_ocr = true;
    let p2 = common::text_page(2, "native text");
    let doc = Document {
        pages: vec![p0, p1, p2],
    };
    let mut arts = Artifacts::new();
    arts.insert(Raster(Arc::new(Pictures)));
    let facts = run(&PagePHash, doc, Some(arts));
    assert_eq!(facts.len(), 2);
    let Value::List(hashes) = &facts[0].value else {
        panic!()
    };
    let Value::List(pages) = &facts[1].value else {
        panic!()
    };
    assert_eq!(pages, &[Value::Int(0), Value::Int(1)]);
    let (Value::U64(a), Value::U64(b)) = (&hashes[0], &hashes[1]) else {
        panic!()
    };
    assert!(hamming(*a, *b) > 16);
    // Without the raster the analyzer is not even applicable.
    assert!(run(&PagePHash, text_doc("x"), None).is_empty());
}

#[test]
fn page_phash_hashes_the_first_three_and_the_last_page() {
    assert_eq!(quail_sketch::phash_pages(0).count(), 0);
    assert_eq!(quail_sketch::phash_pages(3).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(
        quail_sketch::phash_pages(4).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert_eq!(
        quail_sketch::phash_pages(5).collect::<Vec<_>>(),
        [0, 1, 2, 4]
    );
    let pages = (0..10)
        .map(|i| {
            let mut p = common::text_page(i, if i == 4 { "native" } else { "" });
            p.needs_ocr = i != 4;
            p
        })
        .collect();
    let mut arts = Artifacts::new();
    arts.insert(Raster(Arc::new(Pictures)));
    let facts = run(&PagePHash, Document { pages }, Some(arts));
    let Value::List(pages) = &facts[1].value else {
        panic!()
    };
    // Candidates are pages 0-3 and 5-9: the first three and the last.
    assert_eq!(
        pages,
        &[Value::Int(0), Value::Int(1), Value::Int(2), Value::Int(9)]
    );
}

/// A raster whose scans differ by call: the full scan is a horizontal ramp, the reduced one
/// a vertical ramp, and the reduced call records the side it was asked for.
struct Scans(std::sync::Mutex<Vec<u32>>);

fn ramp(w: u32, h: u32, vertical: bool) -> Image {
    let data = (0..h)
        .flat_map(|y| (0..w).map(move |x| if vertical { y } else { x }))
        .map(|v| u8::try_from(v.min(255)).unwrap())
        .collect();
    Image {
        width: w,
        height: h,
        channels: 1,
        data,
    }
}

impl PageRaster for Scans {
    fn render(&self, _: u32, _: f32, _: &Limits) -> Result<Image> {
        Err(quail_core::Error::Other("no render".to_owned()))
    }
    fn embedded_scan(&self, _: u32, _: &Limits) -> Option<Result<Image>> {
        Some(Ok(ramp(256, 256, false)))
    }
    fn embedded_scan_reduced(&self, _: u32, side: u32, _: &Limits) -> Option<Result<Image>> {
        self.0.lock().unwrap().push(side);
        Some(Ok(ramp(side, side, true)))
    }
}

#[test]
fn page_phash_hashes_the_reduced_scan() {
    let mut p0 = common::text_page(0, "");
    p0.needs_ocr = true;
    let scans = Arc::new(Scans(std::sync::Mutex::new(Vec::new())));
    let mut arts = Artifacts::new();
    arts.insert(Raster(scans.clone()));
    let facts = run(&PagePHash, Document { pages: vec![p0] }, Some(arts));
    let Value::List(hashes) = &facts[0].value else {
        panic!()
    };
    let side = quail_sketch::PHASH_SCAN_SIDE;
    assert_eq!(*scans.0.lock().unwrap(), [side]);
    let reduced = ramp(side, side, true);
    let want = quail_sketch::phash::phash(side, side, 1, &reduced.data).unwrap();
    assert_eq!(hashes, &[Value::U64(want)]);
}

#[test]
fn hostile_inputs_do_not_panic() {
    let nasty = [
        String::new(),
        "\u{0}\u{feff}\u{200b}".repeat(1000),
        "1,".repeat(50_000),
        "A ".repeat(30_000) + "Ltd",
        "ab-".repeat(20_000) + "1",
        "\u{301}".repeat(5000),
        "x".repeat(1 << 20),
    ];
    for t in &nasty {
        for a in [
            &MinHashAnalyzer as &dyn quail_core::Analyzer,
            &SimHashText,
            &SimHashTemplate,
            &LayoutAnalyzer,
            &InvoiceLayout,
            &TextKey,
            &CompatMinHash,
        ] {
            let _ = run(a, text_doc(t), None);
        }
    }
}

/// Pages for the shared-artifact tests: hyphenation, typography, numbers, symbols, an empty
/// page and one needing OCR.
fn shared_doc() -> Document {
    let mut pages = vec![
        common::text_page(
            0,
            &format!("Ugo-\nvor „{}“ — 1\u{a0}200,50 RSD.", join(&prose(7, 300))),
        ),
        common::text_page(1, ""),
        common::text_page(
            2,
            &format!("{}\n12.05.2023. Acme Doo, www.acme.rs", form(3, 4)),
        ),
        common::text_page(3, "garbage"),
        common::text_page(4, &join(&prose(8, 200))),
    ];
    pages[3].needs_ocr = true;
    Document { pages }
}

#[test]
fn text_sketches_are_the_same_with_the_shared_artifact() {
    let doc = shared_doc();
    let analyzers: [&dyn quail_core::Analyzer; 4] =
        [&MinHashAnalyzer, &SimHashText, &SimHashTemplate, &TextKey];
    for a in analyzers {
        let alone = run(a, doc.clone(), None);
        assert!(!alone.is_empty(), "{}", a.id());
        let mut extra = Artifacts::new();
        extra.insert(quail_text::NormalizedText::of(&doc));
        assert_eq!(run(a, doc.clone(), Some(extra)), alone, "{}", a.id());
        assert!(a.after().contains(&"normalized_text"));
    }
}

#[test]
fn text_sketches_read_the_revised_document() {
    // The extracted page needs OCR (nothing to sketch); OCR filled it in.
    let mut scanned = common::text_page(0, "");
    scanned.needs_ocr = true;
    let extracted = Document {
        pages: vec![scanned],
    };
    let revised = text_doc(&join(&prose(9, 400)));
    for a in [&MinHashAnalyzer as &dyn quail_core::Analyzer, &TextKey] {
        assert!(run(a, extracted.clone(), None).is_empty());
        let mut e = Artifacts::new();
        e.insert(quail_core::RevisedDocument(revised.clone()));
        let got = run(a, extracted.clone(), Some(e));
        assert_eq!(got, run(a, revised.clone(), None), "{}", a.id());
        assert!(!got.is_empty());
        assert!(a.after().contains(&"revised_document"));
    }
}

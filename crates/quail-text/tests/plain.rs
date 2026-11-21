//! `text.extract@1` over generated plain-text files in many encodings.

use std::time::{Duration, Instant};

use quail_core::{Blob, BlockKind, Document, Emitter, Extractor, Fact, Limits, Value};
use quail_text::{TextExtractor, keys};

/// Invented Serbian Latin contract text with every diacritic.
const SR_LATIN: &str = "UGOVOR O ZAKUPU POSLOVNOG PROSTORA\n\n\
    Zakupodavac i zakupac su se sporazumeli da zakupac plaća mesečnu zakupninu \
    do petog dana u mesecu. Đorđe Šćekić, ovlašćeno lice, potpisuje ugovor u ime \
    zakupodavca. Čuvanje ključeva i održavanje prostora je obaveza zakupca. \
    Žalbe se podnose pismeno, uz obrazloženje i dokaze.\n";

/// Invented Serbian Cyrillic contract text.
const SR_CYRILLIC: &str = "УГОВОР О ПРУЖАЊУ УСЛУГА\n\n\
    Наручилац и извршилац су се споразумели да извршилац пружа услуге одржавања \
    рачунарске опреме током године. Наручилац плаћа накнаду у року од петнаест \
    дана од пријема рачуна. Сви спорови решавају се споразумно, а ако то није \
    могуће, надлежан је суд према седишту наручиоца. Уговор ступа на снагу даном \
    потписивања обе уговорне стране.\n";

fn extract_with(bytes: Vec<u8>, limits: &Limits) -> (Document, Vec<Fact>, Emitter) {
    let mut out = Emitter::new(TextExtractor::ID);
    let doc = TextExtractor::new()
        .extract(&Blob::from(bytes), limits, &mut out)
        .expect("extract");
    let facts = out.facts().to_vec();
    (doc, facts, out)
}

fn extract(bytes: Vec<u8>) -> (Document, Vec<Fact>) {
    let (d, f, _) = extract_with(bytes, &Limits::default());
    (d, f)
}

fn fact<'a>(facts: &'a [Fact], key: &quail_core::FactKey) -> Option<&'a Fact> {
    facts.iter().find(|f| f.key == *key)
}

fn encoding(facts: &[Fact]) -> (String, f32) {
    let f = fact(facts, &keys::ENCODING).expect("text.encoding");
    (f.value.as_text().unwrap().to_owned(), f.conf)
}

/// Windows-1250 bytes of `s`, built by hand from the code page (Serbian Latin letters only).
fn windows_1250(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            'č' => 0xE8,
            'ć' => 0xE6,
            'ž' => 0x9E,
            'š' => 0x9A,
            'đ' => 0xF0,
            'Č' => 0xC8,
            'Ć' => 0xC6,
            'Ž' => 0x8E,
            'Š' => 0x8A,
            'Đ' => 0xD0,
            c if c.is_ascii() => c as u8,
            c => panic!("not in the test map: {c}"),
        })
        .collect()
}

fn utf16(s: &str, be: bool) -> Vec<u8> {
    let mut v = if be {
        vec![0xFE, 0xFF]
    } else {
        vec![0xFF, 0xFE]
    };
    v.extend(
        s.encode_utf16()
            .flat_map(|u| if be { u.to_be_bytes() } else { u.to_le_bytes() }),
    );
    v
}

fn assert_spans_valid(doc: &Document) {
    for p in &doc.pages {
        assert!(!p.text.contains('\r'));
        for b in &p.blocks {
            let s = b.span.slice(&p.text);
            assert!(!s.is_empty(), "empty or invalid span {:?}", b.span);
            assert_eq!(s.len(), (b.span.end - b.span.start) as usize);
            assert_eq!(s.trim(), s);
        }
    }
}

#[test]
fn utf8_document() {
    let (doc, facts) = extract(SR_LATIN.as_bytes().to_vec());
    assert_eq!(encoding(&facts), ("utf-8".to_owned(), 1.0));
    assert_eq!(
        fact(&facts, &keys::ENCODING_BOM).unwrap().value,
        Value::Bool(false)
    );
    assert!(fact(&facts, &keys::DECODE_ERRORS).is_none());
    assert_eq!(doc.pages.len(), 1);
    assert_eq!(doc.pages[0].text, SR_LATIN);
    let p = &doc.pages[0];
    assert_eq!(p.blocks.len(), 2);
    assert_eq!(p.blocks[0].kind, BlockKind::Heading { level: 1 });
    assert_spans_valid(&doc);
}

#[test]
fn utf8_with_bom() {
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend_from_slice(SR_LATIN.as_bytes());
    let (doc, facts) = extract(b);
    assert_eq!(encoding(&facts).0, "utf-8");
    assert_eq!(
        fact(&facts, &keys::ENCODING_BOM).unwrap().value,
        Value::Bool(true)
    );
    assert_eq!(doc.pages[0].text, SR_LATIN, "BOM dropped");
}

#[test]
fn utf16_le_and_be_with_bom() {
    for be in [false, true] {
        let (doc, facts) = extract(utf16(SR_CYRILLIC, be));
        let want = if be { "utf-16be" } else { "utf-16le" };
        assert_eq!(encoding(&facts), (want.to_owned(), 1.0));
        assert_eq!(doc.pages[0].text, SR_CYRILLIC);
        assert_spans_valid(&doc);
    }
}

#[test]
fn windows_1250_serbian_latin() {
    let bytes = windows_1250(SR_LATIN);
    // The hand-built bytes are the code page's own.
    let (enc, _, _) = encoding_rs::WINDOWS_1250.encode(SR_LATIN);
    assert_eq!(bytes, &*enc);
    assert!(std::str::from_utf8(&bytes).is_err());
    let (doc, facts) = extract(bytes);
    let (name, conf) = encoding(&facts);
    assert_eq!(name, "windows-1250");
    assert!((0.3..=0.9).contains(&conf), "{conf}");
    assert!(
        conf > 0.6,
        "letters decode cleanly, no low-confidence warning: {conf}"
    );
    assert_eq!(doc.pages[0].text, SR_LATIN);
}

#[test]
fn windows_1251_cyrillic() {
    let (bytes, _, unmappable) = encoding_rs::WINDOWS_1251.encode(SR_CYRILLIC);
    assert!(!unmappable);
    assert_eq!(bytes[0], 0xD3, "У in Windows-1251");
    let (doc, facts) = extract(bytes.into_owned());
    assert_eq!(encoding(&facts).0, "windows-1251");
    assert_eq!(doc.pages[0].text, SR_CYRILLIC);
}

#[test]
fn crlf_is_normalized_and_spans_index_the_stored_text() {
    let crlf = SR_LATIN.replace('\n', "\r\n");
    let (doc, facts) = extract(crlf.into_bytes());
    assert_eq!(
        fact(&facts, &keys::LINE_ENDINGS).unwrap().value,
        Value::text("crlf")
    );
    assert_eq!(doc.pages[0].text, SR_LATIN);
    assert_spans_valid(&doc);
    let p = &doc.pages[0];
    assert_eq!(
        p.blocks[0].span.slice(&p.text),
        "UGOVOR O ZAKUPU POSLOVNOG PROSTORA"
    );
}

#[test]
fn form_feeds_split_logical_pages() {
    let src = format!("{SR_LATIN}\u{c}{SR_LATIN}\u{c}last page\n\u{c}");
    let (doc, _) = extract(src.into_bytes());
    assert_eq!(doc.pages.len(), 3);
    for (i, p) in doc.pages.iter().enumerate() {
        assert_eq!(p.index as usize, i);
        assert_eq!(p.kind, quail_core::PageKind::Logical);
    }
    assert_eq!(doc.pages[2].text, "last page\n");
    assert_spans_valid(&doc);
}

#[test]
fn huge_single_line_is_bounded() {
    let line = "word ".repeat(4_000_000); // 20 MB, no newline
    let t = Instant::now();
    let (doc, _) = extract(line.into_bytes());
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    assert_eq!(doc.pages.len(), 1);
    assert_eq!(doc.pages[0].blocks.len(), 1);
    assert_spans_valid(&doc);
}

#[test]
fn binary_garbage_never_panics() {
    // A deterministic byte soup: every byte value, NULs, lone CRs, form feeds, broken UTF-8.
    let mut x: u32 = 0x1234_5678;
    for len in [0usize, 1, 2, 3, 7, 64, 1000, 65_537] {
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 24) as u8
            })
            .collect();
        let limits = Limits {
            max_text_bytes: 4096,
            max_pages: 5,
            ..Limits::default()
        };
        let (doc, facts, _) = extract_with(bytes, &limits);
        assert!(doc.pages.len() <= 5);
        assert!(fact(&facts, &keys::ENCODING).is_some());
        assert!(doc.pages.iter().map(|p| p.text.len()).sum::<usize>() <= 4096 + 8);
        assert_spans_valid(&doc);
    }
    for odd in [vec![0xFF, 0xFE, 0x41], vec![0xFE, 0xFF], vec![0xEF, 0xBB]] {
        let (doc, _) = extract(odd);
        assert_spans_valid(&doc);
    }
}

#[test]
fn lossy_input_warns_and_counts() {
    let mut b = SR_LATIN.as_bytes().to_vec();
    b.extend_from_slice(&[0xC4, b' ', 0xE2, 0x82]);
    let (_, facts, out) = extract_with(b, &Limits::default());
    assert_eq!(encoding(&facts).0, "utf-8");
    assert!(fact(&facts, &keys::DECODE_ERRORS).is_some());
    let (_, _, warnings) = out.into_parts();
    assert!(
        warnings.iter().any(|w| w.contains("U+FFFD")),
        "{warnings:?}"
    );
}

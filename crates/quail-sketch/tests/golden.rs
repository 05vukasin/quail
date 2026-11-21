//! Golden vectors: every sketch of a fixed, generated sample is pinned.
//!
//! Stored sketches are compared across machines and releases: a change in `quail-text`
//! (normalization, tokens, folding, XXH3), in `BLAKE2b`, in the regex engines, in the
//! coefficient table or in this crate's rules would silently make old and new sketches
//! incomparable. These tests make it loud.
//!
//! **If this test fails, do not just update the vectors**: bump the version of every analyzer
//! whose sketch changed (`sketch.<name>@N+1`), then regenerate with
//! `QUAIL_GOLDEN_PRINT=1 cargo test -p quail-sketch --test golden -- --nocapture`.
//!
//! The `compat` vectors were checked against the Python original (`QUAIL_GOLDEN_DUMP=<file>`
//! writes the sample text for that check).

mod common;

use common::{boxed_page, form, join, kinds_page, prose};
use quail_core::{BlockKind, Document};
use quail_sketch::{compat, invoice, layout, minhash, phash, simhash, template};
use quail_text::normalize::{NormalizeOptions, normalize};
use quail_text::shingle::hash_bytes;

fn sample() -> String {
    let mut s = String::from(
        "UGOVOR O ZAKUPU — Ugovor br. 114/23, zaključen 12.05.2023. između Đorđa Petrovića \
         i firme Acme d.o.o. (PIB 100200300), iznos 1.200,50 RSD.\n\
         Уговор о закупу пословног простора, члан 5.\n\
         INVOICE NO ABC-EU-20991234 Invoice date 5 March 2024 Bill to Widgets Holdings Ltd. \
         VAT 20% Subtotal 1,000.00 Total due 1,200.00 office@acme.example https://acme.example/x\n",
    );
    s.push_str(&join(&prose(42, 300)));
    s.push('\n');
    s.push_str(&form(7, 3));
    s
}

fn print() -> bool {
    std::env::var_os("QUAIL_GOLDEN_PRINT").is_some()
}

#[test]
fn golden_text_sketches() {
    let text = sample();
    if let Some(p) = std::env::var_os("QUAIL_GOLDEN_DUMP") {
        std::fs::write(p, &text).unwrap();
    }
    let norm = normalize(&text, &NormalizeOptions::default());
    let m = minhash::text_minhash(&text).unwrap();
    let bytes = m.to_bytes();
    let got = (
        m.shingles,
        hash_bytes(&bytes),
        m.rows[..4].to_vec(),
        m.bands()[..4].to_vec(),
        simhash::text_simhash(&norm).unwrap().0,
        template::template_simhash(&norm).unwrap(),
        quail_sketch::text_key(&norm).map(u128::from_be_bytes),
    );
    if print() {
        println!("TEXT {got:?}");
    }
    assert_eq!(got, golden_text());
}

#[test]
fn golden_compat() {
    let text = sample();
    let (sig, n) = compat::raw_minhash(&text).unwrap();
    let bytes: Vec<u8> = sig.iter().flat_map(|r| r.to_le_bytes()).collect();
    let buckets = compat::lsh_buckets(&sig);
    let inv = invoice::features(&text).unwrap();
    let got = (
        n,
        sig[..3].to_vec(),
        hash_bytes(&bytes),
        buckets[0].clone(),
        buckets[31].clone(),
        compat::template_fp(&text).unwrap(),
        format!("{:016x}", inv.fingerprint()),
        format!(
            "{:016x}",
            compat::structural_fp(["1. Scope of Work", "II) Fees", "a) Term"])
        ),
    );
    if print() {
        println!("COMPAT {got:?}");
    }
    assert_eq!(
        got,
        (
            GOLDEN_COMPAT.0,
            GOLDEN_COMPAT.1.to_vec(),
            GOLDEN_COMPAT.2,
            GOLDEN_COMPAT.3.to_owned(),
            GOLDEN_COMPAT.4.to_owned(),
            GOLDEN_COMPAT.5,
            GOLDEN_COMPAT.6.to_owned(),
            GOLDEN_COMPAT.7.to_owned(),
        )
    );
}

#[test]
fn golden_layout_structure_phash() {
    let boxes: Vec<(f32, f32, f32, f32)> = (0..14u8)
        .map(|i| {
            let y = 40.0 + f32::from(i) * 55.0;
            (
                50.0 + f32::from(i % 3) * 20.0,
                y,
                545.0 - f32::from(i % 2) * 200.0,
                y + 30.0,
            )
        })
        .collect();
    let doc = Document {
        pages: vec![
            boxed_page(0, &boxes, &["a"]),
            boxed_page(1, &boxes[..7], &["b"]),
        ],
    };
    let mut kinds = vec![BlockKind::Heading { level: 1 }];
    for s in 0..5u16 {
        kinds.push(BlockKind::Heading { level: 2 });
        kinds.extend(std::iter::repeat_n(
            BlockKind::Paragraph,
            2 + usize::from(s),
        ));
        kinds.push(BlockKind::ListItem);
        kinds.push(BlockKind::TableCell {
            table: s,
            row: 0,
            col: 0,
        });
        kinds.push(BlockKind::TableCell {
            table: s,
            row: 1,
            col: 2,
        });
    }
    let sdoc = Document {
        pages: vec![kinds_page(&kinds, 5)],
    };
    let mut img = Vec::new();
    for y in 0..90u32 {
        for x in 0..70u32 {
            img.push(u8::try_from((x * 3 + y * y / 40) % 256).unwrap());
        }
    }
    let got = (
        layout::geometry_sketch(&doc).unwrap(),
        layout::structure_sketch(&sdoc).unwrap(),
        phash::phash(70, 90, 1, &img).unwrap(),
    );
    if print() {
        println!("LAYOUT {got:?}");
    }
    assert_eq!(got, GOLDEN_LAYOUT);
}

type TextGolden = (
    usize,
    u64,
    Vec<u32>,
    Vec<u64>,
    u64,
    (u64, usize),
    Option<u128>,
);

fn golden_text() -> TextGolden {
    (
        841,
        17_928_132_125_807_181_990,
        vec![1_430_261_215, 2_498_494_647, 1_512_978_877, 3_089_986_366],
        vec![
            7_213_859_534_655_864_785,
            4_923_703_250_259_445_138,
            16_842_788_956_541_844_838,
            5_991_609_292_571_040_522,
        ],
        11_123_065_833_884_862_065,
        (6_118_365_879_420_934_463, 472),
        Some(134_289_710_822_771_776_910_033_679_769_374_455_278),
    )
}

/// Checked against the Python original on the same text.
const GOLDEN_COMPAT: (usize, [u64; 3], u64, &str, &str, u64, &str, &str) = (
    874,
    [
        9_747_362_861_646_355,
        9_291_192_768_311_785,
        9_624_019_534_776_891,
    ],
    2_573_227_598_727_804_936,
    "0:193fa3b1559d2d90",
    "31:96ddb59453963798",
    17_750_551_229_852_862_670,
    "fada76306389340e",
    "d114831a2d8729ca",
);

const GOLDEN_LAYOUT: ((u64, usize), (u64, usize), u64) = (
    (11_406_520_394_328_250_499, 21),
    (3_777_483_071_283_583_000, 21),
    9_241_222_465_152_750_277,
);

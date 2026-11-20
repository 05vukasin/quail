//! Golden vectors: tokens, normalization, keys and sketch hashes of a fixed, diverse sample.
//!
//! Stored sketches (MinHash/SimHash over shingles) are compared across machines and
//! releases. A change in the Unicode tables of `unicode-segmentation` or
//! `unicode-normalization`, in XXH3, in the toolchain's `char` tables or in quail's own
//! rules can change them silently; these tests make it loud.
//!
//! **If this test fails, do not just update the vectors**: every analyzer that stores
//! sketches or keys derived from these functions must bump its version (`name@N+1`), so
//! re-indexing knows to redo them. Then regenerate with
//! `QUAIL_GOLDEN_PRINT=1 cargo test -p quail-text --test golden -- --nocapture`.

use quail_text::fold::{FoldOptions, fold};
use quail_text::normalize::{NormalizeOptions, normalize};
use quail_text::shingle::{char_ngrams, text_shingles};
use quail_text::token::tokens_with_symbols;

/// Serbian Latin and Cyrillic with diacritics, digraph letters, ligatures, fullwidth and
/// superscript digits, combining marks, typographic quotes and dashes, NBSP, soft hyphen,
/// zero-width space, numbers, an IBAN, a URL, an e-mail address, emoji with ZWJ, Greek,
/// Arabic digits, CJK.
const SAMPLE: &str = "Član 5. Ugo-\nvor o zakupu „ﬁrme“ — Đorđe Ǆep i ЉУБАВ Џаја, Ћирић.\n\
Iznos: 1\u{a0}200,50 RSD, broj 114/23 od 12.05.2023. IBAN RS35 2600 0560 1001 6113 79.\n\
Ｔｅｌ ＋３８１ １１, x², e\u{301}te\u{301}, so\u{ad}ft, ze\u{200b}ro, https://firma.rs/a?b=1 \
ime.prezime@firma.rs 👨\u{200d}👩\u{200d}👧 Ωμέγα ٣٤٥ 漢字 C++";

fn print() -> bool {
    std::env::var_os("QUAIL_GOLDEN_PRINT").is_some()
}

#[test]
fn golden_normalize_and_fold() {
    let n = normalize(SAMPLE, &NormalizeOptions::default());
    let k = fold(&n, &FoldOptions::KEY);
    if print() {
        println!("NORMALIZED {n:?}\nKEY {k:?}");
    }
    assert_eq!(n, GOLDEN_NORMALIZED);
    assert_eq!(k, GOLDEN_KEY);
}

#[test]
fn golden_tokens() {
    let got: Vec<String> = tokens_with_symbols(SAMPLE)
        .map(|t| format!("{}..{} {:?} {:?}", t.start, t.end, t.kind, t.text))
        .collect();
    if print() {
        println!("TOKENS {got:#?}");
    }
    assert_eq!(got, GOLDEN_TOKENS);
}

#[test]
fn golden_shingles_and_ngrams() {
    let n = normalize(SAMPLE, &NormalizeOptions::default());
    let sh: Vec<u64> = text_shingles(&n, 4, &FoldOptions::KEY).collect();
    let key = fold(&n, &FoldOptions::KEY);
    let ng: Vec<u64> = char_ngrams(&key, 5).take(12).collect();
    if print() {
        println!("SHINGLES {sh:?}\nNGRAMS {ng:?}");
    }
    assert_eq!(sh, GOLDEN_SHINGLES);
    assert_eq!(ng, GOLDEN_NGRAMS);
}

const GOLDEN_NORMALIZED: &str = "Član 5. Ugovor o zakupu \"firme\" - Đorđe DŽep i ЉУБАВ Џаја, Ћирић.\nIznos: 1 200,50 RSD, broj 114/23 od 12.05.2023. IBAN RS35 2600 0560 1001 6113 79.\nTel +381 11, x2, été, soft, zero, https://firma.rs/a?b=1 ime.prezime@firma.rs 👨\u{200d}👩\u{200d}👧 Ωμέγα ٣٤٥ 漢字 C++";
const GOLDEN_KEY: &str = "clan 5. ugovor o zakupu \"firme\" - djordje dzep i ljubav dzaja, ciric.\niznos: 1 200,50 rsd, broj 114/23 od 12.05.2023. iban rs35 2600 0560 1001 6113 79.\ntel +381 11, x2, ete, soft, zero, https://firma.rs/a?b=1 ime.prezime@firma.rs 👨\u{200d}👩\u{200d}👧 ωμεγα ٣٤٥ 漢字 c++";
const GOLDEN_TOKENS: &[&str] = &[
    "0..5 Word \"Član\"",
    "6..7 Number \"5\"",
    "7..8 Punct \".\"",
    "9..12 Word \"Ugo\"",
    "12..13 Punct \"-\"",
    "14..17 Word \"vor\"",
    "18..19 Word \"o\"",
    "20..26 Word \"zakupu\"",
    "27..30 Punct \"„\"",
    "30..36 Word \"ﬁrme\"",
    "36..39 Punct \"“\"",
    "40..43 Punct \"—\"",
    "44..51 Word \"Đorđe\"",
    "52..56 Word \"Ǆep\"",
    "57..58 Word \"i\"",
    "59..69 Word \"ЉУБАВ\"",
    "70..78 Word \"Џаја\"",
    "78..79 Punct \",\"",
    "80..90 Word \"Ћирић\"",
    "90..91 Punct \".\"",
    "92..97 Word \"Iznos\"",
    "97..98 Punct \":\"",
    "99..108 Number \"1\\u{a0}200,50\"",
    "109..112 Word \"RSD\"",
    "112..113 Punct \",\"",
    "114..118 Word \"broj\"",
    "119..125 Number \"114/23\"",
    "126..128 Word \"od\"",
    "129..139 Number \"12.05.2023\"",
    "139..140 Punct \".\"",
    "141..145 Word \"IBAN\"",
    "146..173 Alnum \"RS35 2600 0560 1001 6113 79\"",
    "173..174 Punct \".\"",
    "175..184 Word \"Ｔｅｌ\"",
    "185..188 Symbol \"＋\"",
    "188..197 Number \"３８１\"",
    "198..204 Number \"１１\"",
    "204..205 Punct \",\"",
    "206..207 Word \"x\"",
    "207..209 Number \"²\"",
    "209..210 Punct \",\"",
    "211..218 Word \"e\\u{301}te\\u{301}\"",
    "218..219 Punct \",\"",
    "220..226 Word \"so\\u{ad}ft\"",
    "226..227 Punct \",\"",
    "228..230 Word \"ze\"",
    "233..235 Word \"ro\"",
    "235..236 Punct \",\"",
    "237..259 Url \"https://firma.rs/a?b=1\"",
    "260..280 Email \"ime.prezime@firma.rs\"",
    "281..299 Symbol \"👨\\u{200d}👩\\u{200d}👧\"",
    "300..310 Word \"Ωμέγα\"",
    "311..317 Number \"٣٤٥\"",
    "318..321 Word \"漢\"",
    "321..324 Word \"字\"",
    "325..328 Word \"C++\"",
];
const GOLDEN_SHINGLES: &[u64] = &[
    11_936_262_523_503_422_235,
    17_709_269_274_768_195_572,
    9_559_246_641_206_690_550,
    10_276_223_987_488_734_895,
    15_865_628_176_800_770_163,
    3_558_404_607_817_502_924,
    16_170_140_856_785_923_107,
    5_519_359_923_577_975_392,
    5_114_212_572_557_800_055,
    13_857_123_896_305_305_150,
    13_781_349_286_317_724_547,
    6_520_800_296_318_062_331,
    12_759_773_375_785_257_375,
    12_619_833_481_830_558_694,
    9_501_581_656_029_336_343,
    6_578_313_709_078_779_164,
    840_693_877_941_469_241,
    6_992_850_773_435_322_595,
    10_696_116_002_377_753_170,
    11_220_641_905_934_298_283,
    5_146_186_814_832_521_263,
    4_147_985_655_481_498_234,
    1_227_526_690_539_863_830,
    1_410_156_091_304_382_499,
    10_524_180_320_416_736_426,
    3_765_875_418_399_735_615,
    15_112_375_867_118_745_200,
    12_396_976_582_916_031_961,
    14_167_828_683_206_679_766,
    14_461_923_452_561_461_178,
    16_440_423_316_760_038_025,
    6_486_146_119_037_996_796,
];
const GOLDEN_NGRAMS: &[u64] = &[
    8_610_605_580_547_125_523,
    3_323_326_358_029_259_165,
    2_631_132_640_979_587_579,
    11_969_642_140_373_277_442,
    12_456_537_511_583_280_887,
    7_725_549_885_688_563_042,
    18_212_418_819_633_525_633,
    6_551_984_350_037_985_551,
    16_989_345_330_491_115_570,
    4_786_083_549_967_254_662,
    10_822_853_600_189_768_966,
    631_087_928_160_175_584,
];

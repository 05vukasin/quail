//! Byte-compatible port of the document fingerprints of an existing Python service, so
//! sketches quail stores can be compared with the ones that service already holds.
//!
//! Given the same text, every function here returns exactly what the Python original
//! returns (checked by a cross-check script on a corpus; see the README). Its rules, kept
//! as they are:
//!
//! - Tokens: `<\w+>|\w+` over `text.lower()` (Python's Unicode `\w`: letters, numbers and
//!   `_`, which is `[\p{L}\p{N}_]`; marks are *not* word characters).
//! - Shingles: the set of word 3-shingles joined by one space; fewer than 3 tokens give the
//!   set of the tokens themselves.
//! - Digest: `BLAKE2b`, 8-byte output, personalization `b"sada-fp-v1"`, read as a big-endian
//!   integer.
//! - MinHash: 128 permutations `(a·x + b) mod (2^61 − 1)` over the digests, with the
//!   coefficients `CPython`'s `random.Random(0x5ADAF19315)` draws (see
//!   `tools/quail-sketch/gen_minhash_coeffs.py`); 32 bands × 4 rows; bucket key
//!   `"{band}:{hex digest of "{band}:{r1},{r2},{r3},{r4}"}"`.
//! - The *version* signature ([`raw_minhash`]) is over unmasked shingles and is empty below
//!   [`MIN_SHINGLES`]. Compare with the share of equal rows: ≥ 0.90 is a version, ≥ 0.75
//!   when an identifier (same invoice number, same agreement) ties the pair, ≥ 0.60 a
//!   near-duplicate; < 0.40 with a shared invoice number is another rendering.
//! - Template ([`template_fp`]): SimHash-64 over the set of unigrams of the entity-masked
//!   text ([`mask_entities`]); one template when within 8 bits.
//! - Heading skeleton ([`structural_fp`]) and invoice layout ([`crate::invoice`]): digests,
//!   compared for equality.
//!
//! Python's `\b`, `\s` and `\d` are emulated exactly where Rust's differ (`\s` also matches
//! U+001C–U+001F in Python; `\b` uses Python's `\w`), and so is its case-insensitive `i`,
//! which also matches `İ` and `ı`.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::LazyLock;

use crate::coeffs::{A, B};
use crate::minhash::{BANDS, P, PERMUTATIONS, ROWS, permute};
use crate::simhash::simhash;

/// Fewest distinct shingles for a version signature.
pub const MIN_SHINGLES: usize = 40;
/// Hamming distance within which two template fingerprints are one template.
pub const TEMPLATE_HAMMING_MAX: u32 = 8;

/// Python's `\w` (str patterns).
const W: &str = r"[\p{L}\p{N}_]";
/// Python's `\s` (str patterns): Unicode white space plus the separators U+001C–U+001F.
const S: &str = r"[\s\x1C-\x1F]";

fn compile(pattern: &str) -> fancy_regex::Regex {
    // The patterns are constants, covered by the tests.
    fancy_regex::Regex::new(pattern).expect("valid constant pattern")
}

/// `\b` in front of a word character / after one.
fn bounded(body: &str) -> String {
    format!(r"(?<!{W}){body}(?!{W})")
}

static WORD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"<{W}+>|{W}+")).expect("valid constant pattern"));
static WHITESPACE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!("{S}+")).expect("valid constant pattern"));
static EMAIL: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| compile(r"[\p{L}\p{N}_.%+-]+@[\p{L}\p{N}_.-]+\.[A-Za-z]{2,}"));
static PARTY_NAME: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    compile(
        r"(?:[A-Z][\p{L}\p{N}_&.'-]*[\s\x1C-\x1F,]+){1,6}(?:Ltd|Limited|Inc|Incorporated|LLC|LLP|PLC|Corp|Corporation|Company|GmbH|AG|S\.?A\.?|B\.?V\.?|N\.?V\.?|S\.?r\.?l\.?|Pty|Oy|AB|A/S)\.?",
    )
});
static MONTH_DATE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    // Case-insensitive only inside the month names: under `(?i)` Rust would also fold the
    // `\w` class of the lookarounds (U+0345 folds to a Greek letter), Python does not.
    compile(&bounded(&format!(
        r"\p{{Nd}}{{1,2}}{S}+(?i:january|february|march|apr[iİı]l|may|june|july|august|september|october|november|december){S}+\p{{Nd}}{{2,4}}"
    )))
});
static DATE: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| compile(&bounded(r"\p{Nd}{1,4}[./-]\p{Nd}{1,2}[./-]\p{Nd}{2,4}")));
// `\b\d[\d,.]*\b`: the run may end on `,` or `.`, so the closing `\b` is the general one.
static NUMBER: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    compile(&format!(
        r"(?<!{W})\p{{Nd}}[\p{{Nd}},.]*(?:(?<={W})(?!{W})|(?<!{W})(?={W}))"
    ))
});
static HEADING_NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(&format!(
        r"(?i)\A{S}*(?:\p{{Nd}}+[.)]?|[ivxlcdmİı]+[.)]|[a-zİı][.)]){S}+"
    ))
    .expect("valid constant pattern")
});

/// The regex engine gave up on a pathological input (backtracking limit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexLimit;

impl std::fmt::Display for RegexLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("regex backtracking limit reached")
    }
}

impl std::error::Error for RegexLimit {}

pub(crate) fn replace_all<'t>(
    re: &fancy_regex::Regex,
    text: &'t str,
    with: &str,
) -> Result<Cow<'t, str>, RegexLimit> {
    re.try_replacen(text, 0, fancy_regex::NoExpand(with))
        .map_err(|_| RegexLimit)
}

/// BLAKE2b-64 with the personalization `b"sada-fp-v1"`.
pub fn digest(value: &str) -> [u8; 8] {
    let h = blake2b_simd::Params::new()
        .hash_length(8)
        .personal(b"sada-fp-v1")
        .hash(value.as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(h.as_bytes());
    out
}

/// [`digest`] as a big-endian integer (`_digest_int`).
pub fn digest_int(value: &str) -> u64 {
    u64::from_be_bytes(digest(value))
}

/// [`digest`] as 16 lowercase hex digits (`_hex_digest`).
pub fn hex_digest(value: &str) -> String {
    format!("{:016x}", digest_int(value))
}

/// Python `str.strip()`.
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `_tokens`: lowercase word / placeholder tokens in order.
pub fn tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    WORD.find_iter(&lower)
        .map(|m| m.as_str().to_owned())
        .collect()
}

/// `shingle(tokens, 3)`: the set of 3-word shingles, or of the tokens when fewer than 3.
pub fn shingles(tokens: &[String]) -> Vec<String> {
    let mut out: Vec<String> = if tokens.len() < 3 {
        tokens.to_vec()
    } else {
        tokens.windows(3).map(|w| w.join(" ")).collect()
    };
    out.sort_unstable();
    out.dedup();
    out
}

/// `minhash_of(features)`: all rows are `p` for an empty set.
pub fn minhash_of(features: &[String]) -> [u64; PERMUTATIONS] {
    if features.is_empty() {
        return [P; PERMUTATIONS];
    }
    let base: Vec<u64> = features.iter().map(|f| digest_int(f)).collect();
    let mut mins = [u64::MAX; PERMUTATIONS];
    for &x in &base {
        for ((m, &a), &b) in mins.iter_mut().zip(&A).zip(&B) {
            let h = permute(a, b, x);
            if h < *m {
                *m = h;
            }
        }
    }
    mins
}

/// `raw_minhash_of(text)`: the version signature over unmasked shingles, with the number of
/// shingles; `None` below [`MIN_SHINGLES`].
pub fn raw_minhash(text: &str) -> Option<([u64; PERMUTATIONS], usize)> {
    let features = shingles(&tokens(text));
    if features.len() < MIN_SHINGLES {
        return None;
    }
    Some((minhash_of(&features), features.len()))
}

/// `lsh_buckets(signature)`.
pub fn lsh_buckets(signature: &[u64]) -> Vec<String> {
    let mut out = Vec::with_capacity(BANDS);
    let mut payload = String::new();
    for band in 0..BANDS {
        let rows = signature
            .get(band * ROWS..band * ROWS + ROWS)
            .unwrap_or(&[]);
        payload.clear();
        let _ = write!(payload, "{band}:");
        for (i, r) in rows.iter().enumerate() {
            if i > 0 {
                payload.push(',');
            }
            let _ = write!(payload, "{r}");
        }
        out.push(format!("{band}:{}", hex_digest(&payload)));
    }
    out
}

/// `mask_entities(text)` without caller-supplied entities.
pub fn mask_entities(text: &str) -> Result<String, RegexLimit> {
    let t = replace_all(&EMAIL, text, "<email>")?;
    let t = replace_all(&PARTY_NAME, &t, "<party> ")?;
    let t = replace_all(&MONTH_DATE, &t, "<date>")?;
    let t = replace_all(&DATE, &t, "<date>")?;
    Ok(replace_all(&NUMBER, &t, "<num>")?.into_owned())
}

/// `template_fp(text)`: SimHash of the set of masked unigrams; 0 for no tokens.
pub fn template_fp(text: &str) -> Result<u64, RegexLimit> {
    let mut features = tokens(&mask_entities(text)?);
    features.sort_unstable();
    features.dedup();
    Ok(simhash(features.iter().map(|f| (digest_int(f), 1))))
}

/// `_normalize_heading`: numbering off, white space collapsed, stripped, lowercased.
pub fn normalize_heading(heading: &str) -> String {
    let without = HEADING_NUMBER.replace(heading, "");
    py_strip(&WHITESPACE.replace_all(&without, " ")).to_lowercase()
}

/// `structural_fp(headings)` as a big-endian integer (the original returns its hex).
pub fn structural_fp<'a>(headings: impl IntoIterator<Item = &'a str>) -> u64 {
    let skeleton: Vec<String> = headings
        .into_iter()
        .map(normalize_heading)
        .filter(|h| !h.is_empty())
        .collect();
    digest_int(&skeleton.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_word_class() {
        // Marks split words in Python; superscript digits are word characters.
        assert_eq!(
            tokens("Cafe\u{301} x² <num> A_b"),
            ["cafe", "x²", "<num>", "a_b"]
        );
    }

    #[test]
    fn short_bodies_shingle_to_tokens() {
        let t = tokens("b a");
        assert_eq!(shingles(&t), ["a", "b"]);
        assert!(raw_minhash("only a few words here").is_none());
    }

    #[test]
    fn masks_like_python() {
        assert_eq!(
            mask_entities("Paid 1,200.50 on 12/06/2024 to Acme Widgets Ltd. via a@b.com").unwrap(),
            "Paid <num> on <date> to <party>  via <email>"
        );
        assert_eq!(
            mask_entities("due 5 March 2024, ref 1,a").unwrap(),
            "due <date>, ref <num>a"
        );
        assert_eq!(mask_entities("x12 3.").unwrap(), "x12 <num>.");
    }

    #[test]
    fn headings() {
        assert_eq!(normalize_heading("  1.  Scope   of Work "), "scope of work");
        assert_eq!(normalize_heading("iv) Fees"), "fees");
        assert_eq!(normalize_heading("Definitions"), "definitions");
        // Python's IGNORECASE matches İ and ı against `i`.
        assert_eq!(normalize_heading("İİ) Fees"), "fees");
        assert_eq!(mask_entities("on 3 APRİL 2024").unwrap(), "on <date>");
        // U+0345 is no word character in Python, case-insensitive or not.
        assert_eq!(
            mask_entities("9\u{1f}april 12\u{345} x").unwrap(),
            "<date>\u{345} x"
        );
    }
}

//! Byte-compatible port of the document fingerprints of an existing deployment (compatibility mode), so
//! sketches quail stores can be compared with the ones that deployment already holds.
//!
//! Given the same text, every function here returns exactly what the Python original
//! returns (checked by a cross-check script on a corpus; see the README). Its rules, kept
//! as they are:
//!
//! - Tokens: `<\w+>|\w+` over `text.lower()` (Python's Unicode `\w`: letters, numbers and
//!   `_`, which is `[\p{L}\p{N}_]`; marks are *not* word characters).
//! - Shingles: the set of word 3-shingles joined by one space; fewer than 3 tokens give the
//!   set of the tokens themselves.
//! - Digest: `BLAKE2b`, 8-byte output, personalization [`PERSONALIZATION`], read as a big-endian
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
//!
//! The four patterns that need lookarounds never run as unanchored searches on
//! `fancy-regex`'s backtracking VM: candidates are found with the linear `regex` crate and
//! each is tried anchored, with a backtracking limit of [`backtrack_limit`] (linear in the
//! text). An attempt past it returns [`RegexLimit`], and the analyzers then warn and skip
//! that sketch.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::{LazyLock, OnceLock};

use crate::coeffs::{A, B};
use crate::minhash::{BANDS, P, PERMUTATIONS, ROWS, permute};
use crate::simhash::simhash;
use crate::text::extend_distinct;

/// Fewest distinct shingles for a version signature.
pub const MIN_SHINGLES: usize = 40;

/// `BLAKE2b` personalization: the personalization string of the compatible deployment.
/// Part of the byte-compatibility contract: changing these bytes changes every digest, so
/// they stay exactly as that deployment uses them.
pub const PERSONALIZATION: &[u8] = b"sada-fp-v1";
/// Hamming distance within which two template fingerprints are one template.
pub const TEMPLATE_HAMMING_MAX: u32 = 8;

/// Python's `\w` (str patterns).
const W: &str = r"[\p{L}\p{N}_]";
/// Python's `\s` (str patterns): Unicode white space plus the separators U+001C–U+001F.
const S: &str = r"[\s\x1C-\x1F]";

/// Fewest backtracking steps one anchored attempt of a `fancy-regex` pattern may take before
/// it gives up with [`RegexLimit`] (see the module docs and the README, "Limits").
pub const BACKTRACK_LIMIT: usize = 100_000;
/// Backtracking steps allowed per byte of the text after the attempt's start.
pub const BACKTRACK_PER_BYTE: usize = 4;

/// The backtracking limit of an attempt over `len` bytes: max([`BACKTRACK_LIMIT`],
/// [`BACKTRACK_PER_BYTE`] × `len` rounded up to a power of two).
pub fn backtrack_limit(len: usize) -> usize {
    let scaled = len
        .saturating_mul(BACKTRACK_PER_BYTE)
        .checked_next_power_of_two()
        .unwrap_or(usize::MAX);
    scaled.max(BACKTRACK_LIMIT)
}

/// A Python pattern of the form `\b<body>` whose body needs lookarounds (Python's `\b`), so it
/// runs on `fancy-regex`'s backtracking VM — but never as an unanchored search, which would
/// try every start position inside the VM.
///
/// Instead the scan is done here, as Python's `re` does it: a match can only start on a
/// character of the body's first class (`first`, found with the linear `regex` crate) whose
/// previous character is not a word character (the leading `\b`, checked directly). At each
/// such candidate the body's lookaround-free core is tried anchored with the linear `regex`
/// crate first (no core match, no match: most candidates end here), and only then the full
/// body **anchored** (`\A(?:body)`, compiled anchored by `fancy-regex`) on the rest of the
/// text. A body whose only lookaround is a trailing general `\b` (`\b\d[\d,.]*\b`) needs no
/// VM at all: its match is the longest prefix of the greedy core match that ends on a word
/// boundary, which is what backtracking finds. After a match the scan resumes at its end,
/// after a miss at the next character. That is exactly Python's leftmost, non-overlapping search, in
/// time linear in the text for these patterns, and each attempt carries an explicit
/// backtracking limit ([`backtrack_limit`] of the remaining length): an attempt past it is
/// [`RegexLimit`] and the caller skips the sketch with a warning.
pub(crate) struct Bounded {
    first: regex::Regex,
    core: regex::Regex,
    /// The body is `core` followed by a general `\b` (no VM needed).
    core_then_boundary: bool,
    pattern: String,
    tiers: [OnceLock<fancy_regex::Regex>; 64],
}

impl Bounded {
    /// `\b<body>` whose matches start on `first` and contain a match of `core` from their start.
    pub(crate) fn new(first: &str, core: &str, body: &str) -> Self {
        Self::build(first, core, body, false)
    }

    /// `\b<core>\b` with a general closing `\b` after a greedy core.
    fn core_then_boundary(first: &str, core: &str) -> Self {
        Self::build(
            first,
            core,
            &format!(r"{core}(?:(?<={W})(?!{W})|(?<!{W})(?={W}))"),
            true,
        )
    }

    fn build(first: &str, core: &str, body: &str, core_then_boundary: bool) -> Self {
        let b = Self {
            first: regex::Regex::new(first).expect("valid constant pattern"),
            core: regex::Regex::new(&format!(r"\A(?:{core})")).expect("valid constant pattern"),
            core_then_boundary,
            pattern: format!(r"\A(?:{body})"),
            tiers: [const { OnceLock::new() }; 64],
        };
        let _ = b.anchored(0); // validates the constant pattern eagerly
        b
    }

    fn anchored(&self, len: usize) -> &fancy_regex::Regex {
        let limit = backtrack_limit(len);
        let tier = (limit.ilog2() as usize).min(63);
        self.tiers[tier].get_or_init(|| {
            // The patterns are constants, covered by the tests.
            fancy_regex::RegexBuilder::new(&self.pattern)
                .backtrack_limit(limit)
                .build()
                .expect("valid constant pattern")
        })
    }

    /// Byte ranges of the non-overlapping matches, left to right.
    pub(crate) fn matches(&self, text: &str) -> Result<Vec<(usize, usize)>, RegexLimit> {
        let mut out = Vec::new();
        let mut pos = 0;
        while let Some(c) = self.first.find_at(text, pos) {
            let start = c.start();
            let after_word = text[..start].chars().next_back().is_some_and(is_word);
            if !after_word && let Some(end) = self.attempt(text, start)? {
                out.push((start, end));
                pos = end;
                continue;
            }
            pos = c.end();
        }
        Ok(out)
    }

    /// End of the match starting at `start` (a candidate), if any.
    fn attempt(&self, text: &str, start: usize) -> Result<Option<usize>, RegexLimit> {
        let rest = &text[start..];
        let Some(core) = self.core.find(rest) else {
            return Ok(None);
        };
        if self.core_then_boundary {
            // Longest prefix of the greedy core match ending on a boundary.
            let mut next = text[start + core.end()..].chars().next();
            for (i, c) in rest[..core.end()].char_indices().rev() {
                if is_word(c) != next.is_some_and(is_word) {
                    return Ok(Some(start + i + c.len_utf8()));
                }
                next = Some(c);
            }
            return Ok(None);
        }
        let found = self
            .anchored(rest.len())
            .find(rest)
            .map_err(|_| RegexLimit)?;
        Ok(found.filter(|m| m.end() > 0).map(|m| start + m.end()))
    }

    /// Python's `re.sub(pattern, with, text)`.
    pub(crate) fn replace_all<'t>(
        &self,
        text: &'t str,
        with: &str,
    ) -> Result<Cow<'t, str>, RegexLimit> {
        let found = self.matches(text)?;
        if found.is_empty() {
            return Ok(Cow::Borrowed(text));
        }
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (a, b) in found {
            out.push_str(&text[last..a]);
            out.push_str(with);
            last = b;
        }
        out.push_str(&text[last..]);
        Ok(Cow::Owned(out))
    }
}

/// Python's `\w` for one character.
pub(crate) fn is_word(c: char) -> bool {
    static WORD_CHAR: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(&format!(r"\A{W}\z")).expect("valid constant pattern"));
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    let mut buf = [0u8; 4];
    WORD_CHAR.is_match(c.encode_utf8(&mut buf))
}

/// The first character of the digit patterns.
const DIGIT: &str = r"\p{Nd}";
/// After a word character: `\b` at the end of a body that ends on one.
const END: &str = r"(?![\p{L}\p{N}_])";

static WORD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"<{W}+>|{W}+")).expect("valid constant pattern"));
static WHITESPACE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!("{S}+")).expect("valid constant pattern"));
// No lookarounds: the linear `regex` crate gives Python's leftmost-first matches directly.
static EMAIL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"[\p{L}\p{N}_.%+-]+@[\p{L}\p{N}_.-]+\.[A-Za-z]{2,}")
        .expect("valid constant pattern")
});
static PARTY_NAME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:[A-Z][\p{L}\p{N}_&.'-]*[\s\x1C-\x1F,]+){1,6}(?:Ltd|Limited|Inc|Incorporated|LLC|LLP|PLC|Corp|Corporation|Company|GmbH|AG|S\.?A\.?|B\.?V\.?|N\.?V\.?|S\.?r\.?l\.?|Pty|Oy|AB|A/S)\.?",
    )
    .expect("valid constant pattern")
});
static MONTH_DATE: LazyLock<Bounded> = LazyLock::new(|| {
    // Case-insensitive only inside the month names: under `(?i)` Rust would also fold the
    // `\w` class of the lookarounds (U+0345 folds to a Greek letter), Python does not.
    Bounded::new(
        DIGIT,
        &format!(
            r"\p{{Nd}}{{1,2}}{S}+(?i:january|february|march|apr[iİı]l|may|june|july|august|september|october|november|december){S}+\p{{Nd}}{{2,4}}"
        ),
        &format!(
            r"\p{{Nd}}{{1,2}}{S}+(?i:january|february|march|apr[iİı]l|may|june|july|august|september|october|november|december){S}+\p{{Nd}}{{2,4}}{END}"
        ),
    )
});
static DATE: LazyLock<Bounded> = LazyLock::new(|| {
    Bounded::new(
        DIGIT,
        r"\p{Nd}{1,4}[./-]\p{Nd}{1,2}[./-]\p{Nd}{2,4}",
        &format!(r"\p{{Nd}}{{1,4}}[./-]\p{{Nd}}{{1,2}}[./-]\p{{Nd}}{{2,4}}{END}"),
    )
});
// `\b\d[\d,.]*\b`: the run may end on `,` or `.`, so the closing `\b` is the general one.
static NUMBER: LazyLock<Bounded> =
    LazyLock::new(|| Bounded::core_then_boundary(DIGIT, r"\p{Nd}[\p{Nd},.]*"));
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

/// BLAKE2b-64 with the personalization [`PERSONALIZATION`].
pub fn digest(value: &str) -> [u8; 8] {
    let h = blake2b_simd::Params::new()
        .hash_length(8)
        .personal(PERSONALIZATION)
        .hash(value.as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(h.as_bytes());
    out
}

/// [`digest`] as a big-endian integer (`_digest_int`).
pub fn digest_int(value: &str) -> u64 {
    u64::from_be_bytes(digest(value))
}

/// [`digest_int`] of `words` joined by one space, without building the joined string.
fn digest_words<S: AsRef<str>>(words: &[S]) -> u64 {
    let mut st = blake2b_simd::Params::new()
        .hash_length(8)
        .personal(PERSONALIZATION)
        .to_state();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            st.update(b" ");
        }
        st.update(w.as_ref().as_bytes());
    }
    let mut out = [0u8; 8];
    out.copy_from_slice(st.finalize().as_bytes());
    u64::from_be_bytes(out)
}

/// Text the template mask reads at once: the segments of a longer text are masked in groups
/// of about this size (see [`template_fp_of`]).
const MASK_BYTES: usize = quail_text::piece::PIECE_BYTES;

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
    let base: Vec<u64> = features.iter().map(|f| digest_int(f)).collect();
    minhash_of_digests(&base)
}

/// [`minhash_of`] over the features' digests (duplicates do not matter).
fn minhash_of_digests(base: &[u64]) -> [u64; PERMUTATIONS] {
    if base.is_empty() {
        return [P; PERMUTATIONS];
    }
    let mut mins = [u64::MAX; PERMUTATIONS];
    for &x in base {
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
///
/// Same result as `minhash_of(&shingles(&tokens(text)))`, computed on borrowed tokens: each
/// shingle is digested without being joined.
pub fn raw_minhash(text: &str) -> Option<([u64; PERMUTATIONS], usize)> {
    raw_minhash_of(&[text])
}

/// [`raw_minhash`] of the text that `segments` make when joined, streamed: one segment is
/// lowercased at a time and only the digests of the 3-word windows are kept (8 bytes each,
/// duplicates dropped as they fill). Segments should be cut at white space (a word cut in
/// two would be two words). The shingle set is counted by digest: two different shingles of
/// one text with the same 64-bit digest count once (the minhash is the same either way).
pub(crate) fn raw_minhash_of(segments: &[&str]) -> Option<([u64; PERMUTATIONS], usize)> {
    let mut digests = Vec::new();
    // The last three words, oldest first.
    let mut window: Vec<String> = Vec::with_capacity(3);
    let mut words = 0usize;
    for seg in segments {
        let lower = seg.to_lowercase();
        for m in WORD.find_iter(&lower) {
            words += 1;
            let mut w = if window.len() == 3 {
                window.remove(0)
            } else {
                String::new()
            };
            w.clear();
            w.push_str(m.as_str());
            window.push(w);
            if window.len() == 3 {
                extend_distinct(&mut digests, [digest_words(&window)]);
            }
        }
    }
    if words < 3 {
        // Fewer than three words: the words themselves (at most two, never enough).
        digests = window.iter().map(|w| digest_words(&[w])).collect();
    }
    digests.sort_unstable();
    digests.dedup();
    if digests.len() < MIN_SHINGLES {
        return None;
    }
    Some((minhash_of_digests(&digests), digests.len()))
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
    let t = EMAIL.replace_all(text, regex::NoExpand("<email>"));
    let t = PARTY_NAME.replace_all(&t, regex::NoExpand("<party> "));
    let t = MONTH_DATE.replace_all(&t, "<date>")?;
    let t = DATE.replace_all(&t, "<date>")?;
    Ok(NUMBER.replace_all(&t, "<num>")?.into_owned())
}

/// `template_fp(text)`: SimHash of the set of masked unigrams; 0 for no tokens.
pub fn template_fp(text: &str) -> Result<u64, RegexLimit> {
    template_fp_of(&[text])
}

/// [`template_fp`] of the text that `segments` make when joined. Consecutive segments are
/// joined and masked in groups of about [`MASK_BYTES`] (a text up to that size is masked
/// whole, as [`template_fp`]); a group ends at a segment end, so a mask can only miss an
/// entity that spans two groups. The unigram set is kept as digests (two different words with
/// one 64-bit digest count once).
pub(crate) fn template_fp_of(segments: &[&str]) -> Result<u64, RegexLimit> {
    let mut digests = Vec::new();
    let mut group = String::new();
    let add = |group: &str, digests: &mut Vec<u64>| -> Result<(), RegexLimit> {
        let lower = mask_entities(group)?.to_lowercase();
        // Unigrams repeat a lot: dedup the borrowed tokens first, then digest each once.
        let mut words: Vec<&str> = WORD.find_iter(&lower).map(|m| m.as_str()).collect();
        words.sort_unstable();
        words.dedup();
        extend_distinct(digests, words.iter().map(|w| digest_int(w)));
        Ok(())
    };
    for seg in segments {
        if !group.is_empty() && group.len() + seg.len() > MASK_BYTES {
            add(&group, &mut digests)?;
            group.clear();
        }
        group.push_str(seg);
    }
    if !group.is_empty() {
        add(&group, &mut digests)?;
    }
    digests.sort_unstable();
    digests.dedup();
    Ok(simhash(digests.iter().map(|&h| (h, 1))))
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
    fn borrowed_paths_equal_the_string_paths() {
        // The definitions, spelled out with owned shingles as the Python original does.
        let reference_raw = |t: &str| {
            let f = shingles(&tokens(t));
            (f.len() >= MIN_SHINGLES).then(|| (minhash_of(&f), f.len()))
        };
        let reference_template = |t: &str| {
            let mut f = tokens(&mask_entities(t).unwrap());
            f.sort_unstable();
            f.dedup();
            simhash(f.iter().map(|x| (digest_int(x), 1)))
        };
        let words = [
            "Ugovor",
            "o",
            "zakupu",
            "the",
            "agreement",
            "<num>",
            "Članak",
            "1.200,50",
            "plaća",
            "ZAKUPAC",
            "info@example.com",
            "12.03.2024",
            "rok",
            "od",
            "dana",
        ];
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut texts = vec![String::new(), "a".into(), "a b".into(), "a b c".into()];
        for n in [5usize, 60, 400, 3000] {
            let mut t = String::new();
            for i in 0..n {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                t.push_str(words[usize::try_from(x % 15).unwrap()]);
                t.push_str(if i % 9 == 8 { ".\n" } else { " " });
            }
            texts.push(t);
        }
        for t in &texts {
            assert_eq!(raw_minhash(t), reference_raw(t), "{} bytes", t.len());
            assert_eq!(
                template_fp(t).unwrap(),
                reference_template(t),
                "{} bytes",
                t.len()
            );
        }
    }

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
    fn limits_scale_with_the_text() {
        assert_eq!(backtrack_limit(0), BACKTRACK_LIMIT);
        assert_eq!(backtrack_limit(1 << 20), 4 << 20);
        assert_eq!(backtrack_limit(usize::MAX), usize::MAX);
        // An attempt past its limit is an error, not a panic.
        let tiny = fancy_regex::RegexBuilder::new(r"\A(?:[a-z]+-[a-z]+(?![a-z]))")
            .backtrack_limit(10)
            .build()
            .unwrap();
        assert!(tiny.find(&"a".repeat(1000)).is_err());
    }

    #[test]
    fn scan_matches_python_leftmost_search() {
        // A candidate after a word character is no start (`\b`); a miss moves one character on.
        let m = NUMBER.matches("a12 3, x1 45a 6.").unwrap();
        let got: Vec<&str> = m.iter().map(|&(a, b)| &"a12 3, x1 45a 6."[a..b]).collect();
        assert_eq!(got, ["3", "6"]);
        assert!(is_word('\u{b2}') && !is_word('\u{301}') && is_word('_') && !is_word('-'));
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

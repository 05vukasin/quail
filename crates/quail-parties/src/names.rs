//! Name keys and the pure name-matching rules, ported from the reference system so that keys
//! written by quail and by it agree. Per-file only: nothing here holds state across files, but
//! a later resolution service can use the same functions to compare candidates of many files.
//!
//! | function | reference | what |
//! |---|---|---|
//! | [`normalize_name`] | `matching.normalize_name` | NFKD, combining marks dropped, case-folded, punctuation **deleted** (`U.K.` → `uk`), whitespace collapsed; non-Latin scripts kept |
//! | [`normalized_key`] | `matching.normalized_key` | [`normalize_name`], or `sym-` + BLAKE2s-64 of the case-folded name when nothing survives |
//! | [`core_key`] | `name_prefix.core_key` | legal forms and connectors dropped, then folded (Serbian Cyrillic → Latin, `đ → dj`) |
//! | [`meaningful_words`] / [`core_words`] | same names | tokens minus noise words (and minus jurisdictions) |
//! | [`score_name_match`] / [`are_names_similar`] | same names | subset rule, Jaccard ≥ 0.6, one-typo tolerance |
//! | [`acronym_key`] | `matching._acronym_keys` | initials of 2–5 tokens, legal forms and connectors excluded |
//! | [`lone_word_across_jurisdictions`] | same name | the "Bank of America" / "Bank of China" veto |
//! | [`contains_aligned`] | `own_entities.matcher` | token-aligned containment both ways, 6-character floor |

use std::collections::BTreeSet;
use std::fmt::Write as _;

use blake2::Blake2s;
use blake2::digest::Digest;
use blake2::digest::consts::U8;
use quail_text::fold::{FoldOptions, fold_into};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::vocab;

/// Prefix of the opaque key of a name with no letter or digit.
pub const OPAQUE_KEY_PREFIX: &str = "sym-";
/// Minimum Jaccard overlap of two core-token sets to name one company.
pub const JACCARD_SIMILARITY_MIN: f64 = 0.6;
/// Shortest squashed run that may match by containment ([`contains_aligned`]).
pub const MIN_RUN_CHARS: usize = 6;

const FUZZY_TOKEN_MIN_LENGTH: usize = 5;
const SUBSET_SURPLUS_MAX: usize = 1;
const SUBSET_MIN_COMMON: usize = 2;
const SUBSET_SURPLUS_PENALTY: f64 = 0.05;
const MIN_ACRONYM_TOKENS: usize = 2;
const MIN_ACRONYM_LENGTH: usize = 2;
const MAX_ACRONYM_LENGTH: usize = 5;
const MIN_SHARED_CORE_WORDS: usize = 2;

/// Case-folded, marks dropped, punctuation deleted, whitespace collapsed.
///
/// ```
/// use quail_parties::names::normalize_name;
/// assert_eq!(normalize_name("Brenwick, Dalton U.K. Limited"), "brenwick dalton uk limited");
/// assert_eq!(normalize_name("ĐORĐEVIĆ d.o.o."), "đorđevic doo");
/// assert_eq!(normalize_name("АЦМЕ д.о.о."), "ацме доо");
/// ```
pub fn normalize_name(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.nfkd() {
        if is_combining_mark(c) {
            continue;
        }
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        for l in c.to_lowercase() {
            // `casefold` turns ß into ss; `to_lowercase` keeps it.
            let push: &[char] = if l == 'ß' { &['s', 's'] } else { &[l] };
            for &p in push {
                if p.is_alphanumeric() {
                    if space {
                        out.push(' ');
                        space = false;
                    }
                    out.push(p);
                }
            }
        }
    }
    out
}

/// [`normalize_name`], or `sym-` + BLAKE2s-64 (hex) of the trimmed, lowercased name when the
/// name has no letter or digit; empty for blank input.
///
/// ```
/// use quail_parties::names::normalized_key;
/// assert_eq!(normalized_key("ACME, d.o.o."), "acme doo");
/// assert_eq!(normalized_key(" *** "), "sym-4f0efe524442d697");
/// assert_eq!(normalized_key("  "), "");
/// ```
pub fn normalized_key(text: &str) -> String {
    let n = normalize_name(text);
    if !n.is_empty() {
        return n;
    }
    let stripped = text.trim();
    if stripped.is_empty() {
        return String::new();
    }
    let lowered = stripped.to_lowercase();
    let digest = Blake2s::<U8>::digest(lowered.as_bytes());
    let mut out = String::from(OPAQUE_KEY_PREFIX);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Folded tokens of a name: [`normalize_name`] tokens folded with
/// [`FoldOptions::KEY`] (Serbian Cyrillic → Latin, `đ → dj`), parenthesised annotations
/// removed first.
pub fn key_tokens(name: &str) -> Vec<String> {
    let stripped = strip_annotations(name);
    let norm = normalize_name(&stripped);
    let mut buf = String::new();
    norm.split(' ')
        .filter(|t| !t.is_empty())
        .map(|t| {
            buf.clear();
            fold_into(t, &FoldOptions::KEY, &mut buf);
            buf.clone()
        })
        .collect()
}

/// Removes parenthesised annotations: `Acme Inc (Consultancy)` → `Acme Inc`.
pub fn strip_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => {
                depth -= 1;
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words a core key drops: legal forms except `holdings`, and connectors.
pub fn is_core_drop(key: &str) -> bool {
    (vocab::is_legal_form_word(key) && key != "holdings")
        || matches!(key, "and" | "the" | "of" | "i")
}

/// The identity of a name as one folded string: legal forms and connectors dropped, a seat
/// city written right after a legal form dropped too (`ACME d.o.o. Beograd` → `acme`). Empty
/// when the name states only a form.
///
/// ```
/// use quail_parties::names::core_key;
/// assert_eq!(core_key("ACME d.o.o. Beograd"), "acme");
/// assert_eq!(core_key("Brenwick, Dalton and Company"), "brenwick dalton");
/// assert_eq!(core_key("АЦМЕ ДОО"), "acme");
/// assert_eq!(core_key("Group Ltd"), "");
/// ```
pub fn core_key(name: &str) -> String {
    let tokens = key_tokens(name);
    let mut kept: Vec<&str> = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        if is_core_drop(t) {
            continue;
        }
        let after_form = i > 0 && vocab::is_legal_form_word(&tokens[i - 1]);
        if after_form && vocab::lookup(vocab::SEAT_CITIES, t).is_some() {
            continue;
        }
        let second = i > 1 && vocab::is_legal_form_word(&tokens[i - 2]);
        let pair = (
            tokens.get(i.wrapping_sub(1)).map_or("", String::as_str),
            t.as_str(),
        );
        if second && matches!(pair, ("novi", "sad" | "beograd") | ("banja", "luka")) {
            continue;
        }
        kept.push(t);
    }
    kept.join(" ")
}

/// [`core_key`] without spaces: two systems never agree on spacing.
pub fn squashed_key(name: &str) -> String {
    core_key(name).replace(' ', "")
}

/// Folded tokens minus noise words, at least two characters long.
pub fn meaningful_words(name: &str) -> BTreeSet<String> {
    key_tokens(name)
        .into_iter()
        .filter(|t| !vocab::is_noise_word(t) && t.chars().count() >= 2)
        .collect()
}

/// [`meaningful_words`] minus jurisdiction words: the identity of a name.
pub fn core_words(name: &str) -> BTreeSet<String> {
    meaningful_words(name)
        .into_iter()
        .filter(|t| !vocab::is_geo(t))
        .collect()
}

/// The jurisdiction words of a name.
pub fn geo_words(name: &str) -> BTreeSet<String> {
    meaningful_words(name)
        .into_iter()
        .filter(|t| vocab::is_geo(t))
        .collect()
}

/// Initials of a name minus connectors, legal forms and jurisdictions, when 2–5 long.
///
/// ```
/// use quail_parties::names::acronym_key;
/// assert_eq!(acronym_key("E Research Technology").as_deref(), Some("ert"));
/// assert_eq!(acronym_key("Brenwick, Dalton and Company").as_deref(), Some("bd"));
/// assert_eq!(acronym_key("Acme Ltd"), None);
/// ```
pub fn acronym_key(name: &str) -> Option<String> {
    let tokens: Vec<String> = key_tokens(name)
        .into_iter()
        .filter(|t| {
            !vocab::is_legal_form_word(t)
                && !matches!(t.as_str(), "a" | "an" | "and" | "for" | "of" | "the" | "i")
                && !vocab::is_geo(t)
        })
        .collect();
    if tokens.len() < MIN_ACRONYM_TOKENS {
        return None;
    }
    let key: String = tokens.iter().filter_map(|t| t.chars().next()).collect();
    let n = key.chars().count();
    (MIN_ACRONYM_LENGTH..=MAX_ACRONYM_LENGTH)
        .contains(&n)
        .then_some(key)
}

fn within_one_edit(a: &[char], b: &[char]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    if a.len() == b.len() {
        let diffs: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        return match diffs.as_slice() {
            [] | [_] => true,
            [f, s] => *s == f + 1 && a[*f] == b[*s] && a[*s] == b[*f],
            _ => false,
        };
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let mut prefix = 0;
    while prefix < short.len() && short[prefix] == long[prefix] {
        prefix += 1;
    }
    short[prefix..] == long[prefix + 1..]
}

/// Same token modulo one typo (substitution, insertion, deletion or transposition), for
/// tokens of at least five characters; never for ideographic scripts.
pub fn tokens_equal_fuzzy(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    if ca.len().min(cb.len()) < FUZZY_TOKEN_MIN_LENGTH {
        return false;
    }
    let ideographic =
        |c: &char| c.is_alphabetic() && !c.is_lowercase() && !c.is_uppercase() && !c.is_ascii();
    if ca.iter().chain(cb.iter()).any(ideographic) {
        return false;
    }
    within_one_edit(&ca, &cb)
}

/// (common, exactly equal, surplus of a, surplus of b) with one-to-one pairing.
fn fuzzy_overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> (usize, usize, usize, usize) {
    let mut unused: BTreeSet<&str> = b.iter().map(String::as_str).collect();
    let (mut common, mut exact) = (0, 0);
    for w in a {
        if unused.remove(w.as_str()) {
            common += 1;
            exact += 1;
            continue;
        }
        if let Some(p) = unused.iter().copied().find(|o| tokens_equal_fuzzy(w, o)) {
            unused.remove(p);
            common += 1;
        }
    }
    (common, exact, a.len() - common, b.len() - common)
}

/// How strongly two core-token sets name one company (the reference ranking rule), or
/// `None`: core equality 1.0, a container with one extra token (sharing at least two)
/// 0.95, else Jaccard when ≥ [`JACCARD_SIMILARITY_MIN`]. A purely typo-derived single overlap
/// never matches.
pub fn score_name_match(a: &BTreeSet<String>, b: &BTreeSet<String>) -> Option<f64> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let (common, exact, sa, sb) = fuzzy_overlap(a, b);
    if common == 0 || (exact == 0 && common < SUBSET_MIN_COMMON) {
        return None;
    }
    if !(sa > 0 && sb > 0) {
        let surplus = sa + sb;
        if surplus <= SUBSET_SURPLUS_MAX && (surplus == 0 || common >= SUBSET_MIN_COMMON) {
            #[allow(clippy::cast_precision_loss)] // surplus <= 1
            return Some(1.0 - SUBSET_SURPLUS_PENALTY * surplus as f64);
        }
    }
    #[allow(clippy::cast_precision_loss)] // token counts are tiny
    let jaccard = common as f64 / (a.len() + b.len() - common) as f64;
    (jaccard >= JACCARD_SIMILARITY_MIN).then_some(jaccard)
}

fn core_or_all(words: BTreeSet<String>) -> BTreeSet<String> {
    let core: BTreeSet<String> = words
        .iter()
        .filter(|w| !vocab::is_geo(w))
        .cloned()
        .collect();
    if core.is_empty() { words } else { core }
}

/// Two names treated as one company: equal normalized, equal squashed, or
/// [`score_name_match`] on their core tokens.
///
/// ```
/// use quail_parties::names::are_names_similar;
/// assert!(are_names_similar("ACME d.o.o.", "Acme DOO Beograd"));
/// assert!(are_names_similar("Orla Venta", "OrlaVenta"));
/// assert!(!are_names_similar("Alpha Solutions", "Beta Solutions"));
/// ```
pub fn are_names_similar(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize_name(a), normalize_name(b));
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    if na == nb {
        return true;
    }
    let sq = squashed_key(a);
    if !sq.is_empty() && sq == squashed_key(b) {
        return true;
    }
    let ca = core_or_all(meaningful_words(a));
    let cb = core_or_all(meaningful_words(b));
    score_name_match(&ca, &cb).is_some()
}

/// True when two names share at most one core token and state disjoint jurisdictions
/// ("Bank of America" vs "Bank of China"): no automatic merge may bridge them.
pub fn lone_word_across_jurisdictions(a: &str, b: &str) -> bool {
    let shared = core_words(a).intersection(&core_words(b)).count();
    if shared >= MIN_SHARED_CORE_WORDS {
        return false;
    }
    let (ga, gb) = (geo_words(a), geo_words(b));
    !ga.is_empty() && !gb.is_empty() && ga.is_disjoint(&gb)
}

/// Every contiguous run of tokens, squashed.
fn token_runs(tokens: &[String]) -> BTreeSet<String> {
    let mut runs = BTreeSet::new();
    for start in 0..tokens.len() {
        let mut buf = String::new();
        for t in &tokens[start..] {
            buf.push_str(t);
            runs.insert(buf.clone());
        }
    }
    runs
}

/// Token-aligned containment in either direction, the reference own-entity rule: equal
/// normalized or squashed names; `needle` as a whole run of `name`'s tokens (squashed, at
/// least [`MIN_RUN_CHARS`]); or `name`'s noise-free core as a whole run of `needle`'s
/// tokens. Whole tokens only: `AZ Group` never matches `Diaz Group Ltd`.
///
/// ```
/// use quail_parties::names::contains_aligned;
/// assert!(contains_aligned("OrlaVenta UK Ltd", "Orla Venta"));
/// assert!(!contains_aligned("Diaz Group Ltd", "AZ Group"));
/// ```
pub fn contains_aligned(name: &str, needle: &str) -> bool {
    let n = key_tokens(name);
    let k = key_tokens(needle);
    if n.is_empty() || k.is_empty() {
        return false;
    }
    if n == k || n.concat() == k.concat() {
        return true;
    }
    let k_sq = k.concat();
    let k_is_noise = k.len() == 1 && vocab::is_noise_word(&k[0]);
    if k_sq.chars().count() >= MIN_RUN_CHARS && !k_is_noise && token_runs(&n).contains(&k_sq) {
        return true;
    }
    let core: String = n
        .iter()
        .filter(|t| !vocab::is_noise_word(t))
        .map(String::as_str)
        .collect();
    core.chars().count() >= MIN_RUN_CHARS && token_runs(&k).contains(&core)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(s: &[&str]) -> BTreeSet<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn normalize_matches_reference() {
        assert_eq!(normalize_name("Straße_Co"), "strasseco");
        assert_eq!(normalize_name("ﬁne & Co."), "fine co");
        assert_eq!(normalize_name("  "), "");
    }

    #[test]
    fn opaque_key_is_reference_blake2s() {
        assert_eq!(normalized_key("★☆"), "sym-de828d53ab060d16");
    }

    #[test]
    fn scoring() {
        assert_eq!(
            score_name_match(&set(&["acme"]), &set(&["acme"])),
            Some(1.0)
        );
        assert_eq!(
            score_name_match(
                &set(&["brenwick", "dalton"]),
                &set(&["brenwick", "dalton", "x"])
            ),
            Some(0.95)
        );
        assert_eq!(score_name_match(&set(&["novaro"]), &set(&["novaor"])), None);
        assert!(tokens_equal_fuzzy("brenwick", "brenwik"));
        assert!(!tokens_equal_fuzzy("usa", "usb"));
    }

    #[test]
    fn jurisdiction_veto() {
        assert!(lone_word_across_jurisdictions(
            "Bank of America",
            "Bank of China"
        ));
        assert!(!lone_word_across_jurisdictions(
            "Brenwick Dalton UK",
            "Brenwick Dalton Austria"
        ));
    }

    #[test]
    fn annotations_stripped() {
        assert_eq!(strip_annotations("Acme Inc (Consultancy) X"), "Acme Inc X");
    }
}

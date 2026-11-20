//! Template SimHash: documents filled in from one form collide while their particulars
//! differ.
//!
//! # Masking (stable definition of `sketch.simhash_template`)
//!
//! The normalized text is tokenized with `quail_text::token::keyed_tokens` (key:
//! [`FoldOptions::KEY`]) and every token becomes either its folded key or a class
//! placeholder:
//!
//! | token | becomes |
//! |---|---|
//! | URL / e-mail address | `<url>` / `<email>` |
//! | number with two date separators (`12.05.2023`, `2023-05-12`, `12/5/23`) | `<date>` |
//! | number with a decimal or thousands separator (`1.200,50`, `3,14`) | `<amount>` |
//! | any other number (`114/23`, `+381`, `7`) | `<num>` |
//! | letters and digits (`RS35…`, `INV-2041` parts, `A4`) | `<code>` |
//! | a month name (English, Serbian Latin; folded) | `<month>` |
//! | a capitalized word (`Petrović`, `Acme`) that does not start a line or a sentence | `<name>` |
//! | anything else | its folded key |
//!
//! A sentence starts at the beginning of the text, after a line break and after `.`, `!`,
//! `?`, `:` or `;` (ignoring spaces, quotes and brackets in between). All-caps words are
//! kept: in business documents they are mostly headings and labels, which are the template.
//!
//! The features are the **set** of masked words (each counted once, hashed with
//! `quail_text::shingle::hash_str`), signed with [`crate::simhash::simhash`]. A set of
//! unigrams moves little when clauses are edited or reordered, and the masked particulars
//! all fall onto a few placeholders.
//!
//! # Comparison
//!
//! [`crate::hamming`] ≤ [`TEMPLATE_HAMMING_MAX`] (8 of 64): same template. Combine with an
//! issuer key when the service has one: unrelated documents of one language and genre
//! (two contracts) share most function words and can land close.

use std::borrow::Cow;

use quail_text::fold::FoldOptions;
use quail_text::shingle::hash_str;
use quail_text::token::{Token, TokenKind, keyed_tokens};

use crate::simhash::simhash;

/// Hamming distance (of 64) at or below which two `sketch.simhash_template` values are one
/// template.
pub const TEMPLATE_HAMMING_MAX: u32 = 8;

/// Month names, folded, in English and Serbian Latin (Cyrillic folds onto these). `may` is
/// left out: it is mostly the verb.
const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "januar",
    "februar",
    "mart",
    "maj",
    "jun",
    "jul",
    "avgust",
    "septembar",
    "oktobar",
    "novembar",
    "decembar",
];

fn number_class(text: &str) -> &'static str {
    let date_seps = text
        .chars()
        .filter(|c| matches!(c, '.' | '/' | '-'))
        .count();
    let groups = text
        .split(['.', '/', '-'])
        .filter(|g| !g.is_empty())
        .count();
    if date_seps == 2 && groups == 3 && !text.contains(',') {
        return "<date>";
    }
    if text.contains(',') || text.contains('.') || text.contains('\u{a0}') || text.contains(' ') {
        return "<amount>";
    }
    "<num>"
}

/// Whether the gap between two tokens ends a sentence or a line.
fn starts_sentence(gap: &str) -> bool {
    gap.chars()
        .any(|c| matches!(c, '\n' | '.' | '!' | '?' | ':' | ';' | '\u{c}'))
}

fn is_title_case(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(char::is_uppercase) && chars.any(char::is_lowercase)
}

/// The masked form of one token; `gap` is the text since the previous token (the whole
/// prefix for the first one).
fn mask<'k>(token: &Token<'_>, key: &'k str, gap: &str, first: bool) -> Cow<'k, str> {
    let class = match token.kind {
        TokenKind::Url => Some("<url>"),
        TokenKind::Email => Some("<email>"),
        TokenKind::Number => Some(number_class(token.text)),
        TokenKind::Alnum => Some("<code>"),
        TokenKind::Word if MONTHS.contains(&key) => Some("<month>"),
        TokenKind::Word if !first && !starts_sentence(gap) && is_title_case(token.text) => {
            Some("<name>")
        }
        _ => None,
    };
    class.map_or(Cow::Borrowed(key), Cow::Borrowed)
}

/// Calls `f` with every masked token of already-normalized `text`, in order.
pub fn for_each_masked(text: &str, mut f: impl FnMut(&str)) {
    let mut it = keyed_tokens(text, &FoldOptions::KEY);
    let mut prev_end = 0usize;
    let mut first = true;
    while let Some(t) = it.next_keyed() {
        let gap = text.get(prev_end..t.token.start).unwrap_or("");
        prev_end = t.token.end;
        let m = mask(&t.token, t.key, gap, first);
        first = false;
        f(&m);
    }
}

/// Masked tokens of already-normalized `text` (allocating; for inspection and tests).
pub fn masked_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for_each_masked(text, |m| out.push(m.to_owned()));
    out
}

/// `sketch.simhash_template` of already-normalized `text`, with the number of distinct
/// masked words; `None` without any token.
pub fn template_simhash(text: &str) -> Option<(u64, usize)> {
    let mut hashes = Vec::new();
    for_each_masked(text, |m| hashes.push(hash_str(m)));
    hashes.sort_unstable();
    hashes.dedup();
    if hashes.is_empty() {
        return None;
    }
    Some((simhash(hashes.iter().map(|&h| (h, 1))), hashes.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_particulars() {
        let m = masked_tokens(
            "Ugovor br. 114/23 zaključen 12.05.2023. sa firmom Acme, iznos 1.200,50 RSD, \
             kontakt office@acme.rs, račun RS35260005601001611379, 5. maj",
        );
        assert_eq!(
            m,
            [
                "ugovor",
                "br",
                "<num>",
                "zakljucen",
                "<date>",
                "sa",
                "firmom",
                "<name>",
                "iznos",
                "<amount>",
                "rsd",
                "kontakt",
                "<email>",
                "racun",
                "<code>",
                "<num>",
                "<month>"
            ]
        );
    }

    #[test]
    fn sentence_starts_are_kept() {
        let m = masked_tokens("Payment is due. Late fees apply\nTotal");
        assert_eq!(
            m,
            ["payment", "is", "due", "late", "fees", "apply", "total"]
        );
    }
}

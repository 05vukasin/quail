//! Folding text into *matching keys*: lowercase, optionally without diacritics and with
//! Serbian Cyrillic transliterated to Latin, so `"ĐORĐEVIĆ"`, `"Djordjevic"` and
//! `"Ђорђевић"` all fold to `"djordjevic"`.
//!
//! Folding is lossy on purpose; use it for lookup keys, shingles and dictionary matches,
//! never for text shown to people.
//!
//! # Rules
//!
//! - **Case**: Unicode lowercase (`char::to_lowercase`). Not full case folding: `ß` stays
//!   `ß` unless diacritic folding is on (then it becomes `ss`).
//! - **Diacritics**: canonical decomposition, then combining marks dropped
//!   (`č ć → c`, `š → s`, `ž → z`, `é → e`, `ü → u`). Letters that do not decompose are
//!   mapped explicitly: `đ → dj` (or `d`, see [`DjFold`]), `ø → o`, `ł → l`, `ß → ss`,
//!   `æ → ae`, `œ → oe`, `þ → th`, `ð → d`, `ı → i`, `ħ → h`, `ŧ → t`.
//! - **Cyrillic**: Serbian Cyrillic is transliterated first with
//!   [`cyrillic_to_latin`](crate::translit::cyrillic_to_latin); other Cyrillic letters
//!   are kept (and lowercased).
//!
//! With diacritic folding, `dž` (from `џ` or written) becomes `dz`, `đ` becomes `dj`.
//! The `dj` choice matches how Serbian is written without diacritics (`Djordje`) and keeps
//! `đ` apart from `d`. Use [`DjFold::D`] for keys that must also match sloppy ASCII like
//! `Dordevic`; to match both spellings, fold with both variants.

use std::borrow::Cow;

use unicode_normalization::char::{decompose_canonical, is_combining_mark};

use crate::translit::cyrillic_to_latin;

/// How diacritic folding writes `đ` / `Đ`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DjFold {
    /// `đ → dj`, the conventional ASCII spelling (`Đorđe → Djordje`). The default.
    #[default]
    Dj,
    /// `đ → d` (`Đorđe → Dorde`), matching text typed without diacritics.
    D,
}

/// Options of [`fold`]. `Default` is [`FoldOptions::KEY`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldOptions {
    /// Lowercase.
    pub lowercase: bool,
    /// Strip diacritics (see the module rules).
    pub diacritics: bool,
    /// Transliterate Serbian Cyrillic to Latin first.
    pub cyrillic: bool,
    /// Spelling of `đ` when stripping diacritics.
    pub dj: DjFold,
}

impl FoldOptions {
    /// Lowercase only.
    pub const CASE: Self = Self {
        lowercase: true,
        diacritics: false,
        cyrillic: false,
        dj: DjFold::Dj,
    };
    /// Lowercase, Cyrillic → Latin, no diacritics: the matching key.
    pub const KEY: Self = Self {
        lowercase: true,
        diacritics: true,
        cyrillic: true,
        dj: DjFold::Dj,
    };
}

impl Default for FoldOptions {
    fn default() -> Self {
        Self::KEY
    }
}

/// Folds `text`. Borrows when nothing changes (plain lowercase ASCII under the defaults).
///
/// ```
/// use quail_text::fold::{fold, FoldOptions};
/// assert_eq!(fold("ĐORĐEVIĆ Šućur", &FoldOptions::KEY), "djordjevic sucur");
/// assert_eq!(fold("Ђорђевић", &FoldOptions::KEY), "djordjevic");
/// assert_eq!(fold("Čačak", &FoldOptions::CASE), "čačak");
/// ```
pub fn fold<'a>(text: &'a str, opts: &FoldOptions) -> Cow<'a, str> {
    let unchanged = text
        .bytes()
        .all(|b| b.is_ascii() && !(opts.lowercase && b.is_ascii_uppercase()));
    if unchanged {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    fold_into(text, opts, &mut out);
    if out == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(out)
    }
}

/// Folds `text`, appending to `out`. Reuse `out` (e.g. per token) to avoid allocations.
pub fn fold_into(text: &str, opts: &FoldOptions, out: &mut String) {
    out.reserve(text.len());
    let text: Cow<'_, str> = if opts.cyrillic {
        cyrillic_to_latin(text)
    } else {
        Cow::Borrowed(text)
    };
    for c in text.chars() {
        if c.is_ascii() {
            out.push(if opts.lowercase {
                c.to_ascii_lowercase()
            } else {
                c
            });
            continue;
        }
        if opts.lowercase {
            for l in c.to_lowercase() {
                push_folded(l, *opts, out);
            }
        } else {
            push_folded(c, *opts, out);
        }
    }
}

fn push_folded(c: char, opts: FoldOptions, out: &mut String) {
    if !opts.diacritics {
        out.push(c);
        return;
    }
    if let Some(s) = special(c, opts.dj) {
        out.push_str(s);
        return;
    }
    if is_combining_mark(c) {
        return;
    }
    decompose_canonical(c, |d| {
        if !is_combining_mark(d) {
            out.push(d);
        }
    });
}

/// Letters without a canonical decomposition that still carry a "diacritic".
fn special(c: char, dj: DjFold) -> Option<&'static str> {
    Some(match (c, dj) {
        ('đ', DjFold::Dj) => "dj",
        ('Đ', DjFold::Dj) => "Dj",
        ('đ', DjFold::D) | ('ð', _) => "d",
        ('Đ', DjFold::D) | ('Ð', _) => "D",
        ('ø', _) => "o",
        ('Ø', _) => "O",
        ('ł', _) => "l",
        ('Ł', _) => "L",
        ('ß', _) => "ss",
        ('ẞ', _) => "SS",
        ('æ', _) => "ae",
        ('Æ', _) => "AE",
        ('œ', _) => "oe",
        ('Œ', _) => "OE",
        ('þ', _) => "th",
        ('Þ', _) => "TH",
        ('ı', _) => "i",
        ('ħ', _) => "h",
        ('Ħ', _) => "H",
        ('ŧ', _) => "t",
        ('Ŧ', _) => "T",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_folding() {
        let k = FoldOptions::KEY;
        assert_eq!(
            fold("Čačak ćevapi Šabac Žabalj", &k),
            "cacak cevapi sabac zabalj"
        );
        assert_eq!(fold("Đorđe", &k), "djordje");
        assert_eq!(fold("Џеп ЉИЉАНА", &k), "dzep ljiljana");
        assert_eq!(fold("Straße Øre Łódź", &k), "strasse ore lodz");
        assert_eq!(fold("e\u{301}te\u{301}", &k), "ete");
        assert!(matches!(fold("already ascii", &k), Cow::Borrowed(_)));
    }

    #[test]
    fn dj_variants() {
        let d = FoldOptions {
            dj: DjFold::D,
            ..FoldOptions::KEY
        };
        assert_eq!(fold("Đorđević", &d), "dordevic");
        assert_eq!(fold("Đorđević", &FoldOptions::KEY), "djordjevic");
    }

    #[test]
    fn case_only_keeps_marks() {
        assert_eq!(fold("ĐAK Čačak", &FoldOptions::CASE), "đak čačak");
        let none = FoldOptions {
            lowercase: false,
            ..FoldOptions::KEY
        };
        assert_eq!(fold("ĐAK Čačak", &none), "DjAK Cacak");
    }

    #[test]
    fn fold_into_appends() {
        let mut buf = String::from("x:");
        fold_into("ŠUMA", &FoldOptions::KEY, &mut buf);
        assert_eq!(buf, "x:suma");
    }
}

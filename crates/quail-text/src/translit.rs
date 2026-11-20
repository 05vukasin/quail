//! Exact Serbian transliteration between Cyrillic (ћирилица) and Latin (latinica).
//!
//! # Cyrillic → Latin
//!
//! Letter by letter, one to one except for the three letters that become digraphs:
//! `љ → lj`, `њ → nj`, `џ → dž` (and `ђ → đ`, `ћ → ć`, `ч → č`, `ж → ž`, `ш → š`, `ј → j`).
//! The case of an uppercase digraph follows its neighbours: `Љубав → Ljubav`, but
//! `ЉУБАВ → LJUBAV`, `ЊЊ → NJNJ`. An uppercase digraph letter is written all caps when the
//! next character is an uppercase letter, or when the next character is not a letter and
//! the previous one is an uppercase letter; otherwise it is title case.
//!
//! Cyrillic letters that are not in the Serbian alphabet (Russian `я ю щ ы э й ё ъ ь`,
//! Macedonian `ѓ ќ ѕ`, Ukrainian `і ї є ґ`, ...) are left unchanged: this is a Serbian
//! transliterator, not a general romanizer. Everything that is not Cyrillic is copied.
//!
//! # Latin → Cyrillic
//!
//! The reverse needs a policy, because Latin digraphs are ambiguous:
//!
//! 1. **Greedy digraphs.** `lj → љ`, `nj → њ`, `dž → џ`, in any case (`LJ`, `Lj`, `lJ`), and
//!    the precomposed digraph letters `ǅ ǆ Ǆ ǈ ǉ Ǉ ǋ ǌ Ǌ` too.
//! 2. **Exceptions** where a digraph spans a morpheme boundary and is two letters:
//!    `d|ž` after the prefixes `nad-`, `pod-`, `pred-`, `ispod-`, `iznad-` at the start of a
//!    word (`nadživeti → надживети`, `podžupan → поджупан`), and `n|j` inside the stems
//!    `injek-`, `konjug-`, `konjunk-`, `vanjez-`, `tanjug` (`injekcija → инјекција`). The
//!    list is short on purpose; a word outside it gets the greedy reading. `od-` + `ž` is not
//!    an exception because `оџак` (chimney) is far more common in business text than the
//!    prefixed forms.
//! 3. **Foreign words stay Latin** (option [`LatinToCyrillic::keep_foreign`], on by
//!    default): a word that contains a Latin letter outside the Serbian alphabet (`q w x y`,
//!    `ä ö ü ß é ...`) is copied unchanged, so `Windows`, `Exxon` or `XII` survive. A foreign
//!    word spelled only with Serbian letters (`Microsoft`) cannot be told apart and is
//!    converted.
//! 4. **ASCII-ized text is not repaired.** `dj`, `c`, `s`, `z` written for `đ`, `č/ć`, `š`,
//!    `ž` are converted as written (`dj → дј`); the transliterator does not guess.
//!
//! Input in decomposed form (`c` + U+030C) is composed (NFC) first.

use std::borrow::Cow;

use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};

/// Latin form of a lowercase Serbian Cyrillic letter, or `None` for anything else.
fn cyr_lower(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'ђ' => "đ",
        'е' => "e",
        'ж' => "ž",
        'з' => "z",
        'и' => "i",
        'ј' => "j",
        'к' => "k",
        'л' => "l",
        'љ' => "lj",
        'м' => "m",
        'н' => "n",
        'њ' => "nj",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'ћ' => "ć",
        'у' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "c",
        'ч' => "č",
        'џ' => "dž",
        'ш' => "š",
        _ => return None,
    })
}

/// Latin form of an uppercase Serbian Cyrillic letter. Digraph letters come back in title
/// case; the caller upgrades them to all caps by context.
fn cyr_upper(c: char) -> Option<&'static str> {
    Some(match c {
        'А' => "A",
        'Б' => "B",
        'В' => "V",
        'Г' => "G",
        'Д' => "D",
        'Ђ' => "Đ",
        'Е' => "E",
        'Ж' => "Ž",
        'З' => "Z",
        'И' => "I",
        'Ј' => "J",
        'К' => "K",
        'Л' => "L",
        'Љ' => "Lj",
        'М' => "M",
        'Н' => "N",
        'Њ' => "Nj",
        'О' => "O",
        'П' => "P",
        'Р' => "R",
        'С' => "S",
        'Т' => "T",
        'Ћ' => "Ć",
        'У' => "U",
        'Ф' => "F",
        'Х' => "H",
        'Ц' => "C",
        'Ч' => "Č",
        'Џ' => "Dž",
        'Ш' => "Š",
        _ => return None,
    })
}

fn all_caps_digraph(c: char) -> &'static str {
    match c {
        'Љ' => "LJ",
        'Њ' => "NJ",
        _ => "DŽ",
    }
}

fn has_cyrillic(text: &str) -> bool {
    // Every Cyrillic letter starts with a UTF-8 lead byte 0xD0..=0xD3 (U+0400..=U+04FF).
    text.as_bytes().iter().any(|b| (0xD0..=0xD3).contains(b))
}

/// Serbian Cyrillic → Latin. Borrows when the text has no Cyrillic letter.
///
/// ```
/// use quail_text::translit::cyrillic_to_latin;
/// assert_eq!(cyrillic_to_latin("Уговор о кредиту, Љубљана, ЏЕП"), "Ugovor o kreditu, Ljubljana, DŽEP");
/// ```
pub fn cyrillic_to_latin(text: &str) -> Cow<'_, str> {
    if !has_cyrillic(text) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    cyrillic_to_latin_into(text, &mut out);
    Cow::Owned(out)
}

/// Serbian Cyrillic → Latin, appended to `out` (reuse the buffer across calls).
pub fn cyrillic_to_latin_into(text: &str, out: &mut String) {
    out.reserve(text.len());
    let mut prev: Option<char> = None;
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if let Some(l) = cyr_lower(c) {
            out.push_str(l);
        } else if let Some(u) = cyr_upper(c) {
            if matches!(c, 'Љ' | 'Њ' | 'Џ') {
                let next = it.peek().copied();
                let caps = match next {
                    Some(n) if n.is_alphabetic() => n.is_uppercase(),
                    _ => prev.is_some_and(char::is_uppercase),
                };
                out.push_str(if caps { all_caps_digraph(c) } else { u });
            } else {
                out.push_str(u);
            }
        } else {
            out.push(c);
        }
        prev = Some(c);
    }
}

/// Options of [`latin_to_cyrillic_with`]. See the [module docs](self) for the policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatinToCyrillic {
    /// Copy words with letters outside the Serbian Latin alphabet unchanged (default `true`).
    pub keep_foreign: bool,
    /// Apply the morpheme-boundary exception list (default `true`).
    pub exceptions: bool,
}

impl Default for LatinToCyrillic {
    fn default() -> Self {
        Self {
            keep_foreign: true,
            exceptions: true,
        }
    }
}

/// Serbian Latin → Cyrillic with the default policy. Borrows when nothing changes.
///
/// ```
/// use quail_text::translit::latin_to_cyrillic;
/// assert_eq!(latin_to_cyrillic("Ljubljana, NJIVA, džep"), "Љубљана, ЊИВА, џеп");
/// assert_eq!(latin_to_cyrillic("nadživeti Windows"), "надживети Windows");
/// ```
pub fn latin_to_cyrillic(text: &str) -> Cow<'_, str> {
    latin_to_cyrillic_with(text, LatinToCyrillic::default())
}

/// Serbian Latin → Cyrillic with explicit options. Borrows when nothing changes.
pub fn latin_to_cyrillic_with(text: &str, opts: LatinToCyrillic) -> Cow<'_, str> {
    let composed: Cow<'_, str> = if is_nfc_quick(text.chars()) == IsNormalized::Yes {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.nfc().collect())
    };
    if !composed
        .chars()
        .any(|c| lat_single(c).is_some() || digraph_char(c).is_some())
    {
        return composed;
    }
    let mut out = String::with_capacity(composed.len() * 2);
    latin_to_cyrillic_into(&composed, opts, &mut out);
    Cow::Owned(out)
}

/// Serbian Latin → Cyrillic, appended to `out`. Expects NFC input (see
/// [`latin_to_cyrillic_with`], which composes for you).
pub fn latin_to_cyrillic_into(text: &str, opts: LatinToCyrillic, out: &mut String) {
    out.reserve(text.len() * 2);
    let mut rest = text;
    let mut lower = String::new();
    while !rest.is_empty() {
        // Copy the non-letter run, then convert one word (maximal run of letters).
        let word_start = rest
            .char_indices()
            .find(|&(_, c)| c.is_alphabetic())
            .map_or(rest.len(), |(i, _)| i);
        out.push_str(&rest[..word_start]);
        rest = &rest[word_start..];
        let word_end = rest
            .char_indices()
            .find(|&(_, c)| !c.is_alphabetic())
            .map_or(rest.len(), |(i, _)| i);
        let word = &rest[..word_end];
        rest = &rest[word_end..];
        if word.is_empty() {
            continue;
        }
        if opts.keep_foreign && is_foreign(word) {
            out.push_str(word);
            continue;
        }
        lower.clear();
        lower.extend(word.chars().flat_map(char::to_lowercase));
        convert_word(word, &lower, opts.exceptions, out);
    }
}

/// A word with a Latin letter that Serbian Latin does not have.
fn is_foreign(word: &str) -> bool {
    word.chars().any(|c| {
        c.is_alphabetic()
            && crate::script::script_of(c) == Some(crate::script::Script::Latin)
            && lat_single(c).is_none()
            && digraph_char(c).is_none()
    })
}

/// Cyrillic for one Serbian Latin letter (not a digraph).
fn lat_single(c: char) -> Option<char> {
    Some(match c {
        'a' => 'а',
        'b' => 'б',
        'c' => 'ц',
        'č' => 'ч',
        'ć' => 'ћ',
        'd' => 'д',
        'đ' => 'ђ',
        'e' => 'е',
        'f' => 'ф',
        'g' => 'г',
        'h' => 'х',
        'i' => 'и',
        'j' => 'ј',
        'k' => 'к',
        'l' => 'л',
        'm' => 'м',
        'n' => 'н',
        'o' => 'о',
        'p' => 'п',
        'r' => 'р',
        's' => 'с',
        'š' => 'ш',
        't' => 'т',
        'u' => 'у',
        'v' => 'в',
        'z' => 'з',
        'ž' => 'ж',
        'A' => 'А',
        'B' => 'Б',
        'C' => 'Ц',
        'Č' => 'Ч',
        'Ć' => 'Ћ',
        'D' => 'Д',
        'Đ' => 'Ђ',
        'E' => 'Е',
        'F' => 'Ф',
        'G' => 'Г',
        'H' => 'Х',
        'I' => 'И',
        'J' => 'Ј',
        'K' => 'К',
        'L' => 'Л',
        'M' => 'М',
        'N' => 'Н',
        'O' => 'О',
        'P' => 'П',
        'R' => 'Р',
        'S' => 'С',
        'Š' => 'Ш',
        'T' => 'Т',
        'U' => 'У',
        'V' => 'В',
        'Z' => 'З',
        'Ž' => 'Ж',
        _ => return None,
    })
}

/// Cyrillic for a precomposed Latin digraph letter (U+01C4..=U+01CC).
fn digraph_char(c: char) -> Option<char> {
    Some(match c {
        'Ǆ' | 'ǅ' => 'Џ',
        'ǆ' => 'џ',
        'Ǉ' | 'ǈ' => 'Љ',
        'ǉ' => 'љ',
        'Ǌ' | 'ǋ' => 'Њ',
        'ǌ' => 'њ',
        _ => return None,
    })
}

const DZ_PREFIXES: [&str; 5] = ["nad", "pod", "pred", "ispod", "iznad"];
const NJ_STEMS: [&str; 5] = ["injek", "konjug", "konjunk", "vanjez", "tanjug"];

/// Whether the digraph whose first letter is at char index `ci` of the lowercased word
/// is really two letters.
fn split_digraph(lower: &str, ci: usize, first: char) -> bool {
    match first {
        'd' => DZ_PREFIXES
            .iter()
            .any(|p| lower.starts_with(p) && p.chars().count() == ci + 1),
        'n' => NJ_STEMS.iter().any(|s| {
            lower.match_indices(s).any(|(bi, _)| {
                // char index of the 'n' of "nj" inside the stem
                let off = s.find("nj").map_or(0, |o| s[..o].chars().count());
                lower[..bi].chars().count() + off == ci
            })
        }),
        _ => false,
    }
}

fn convert_word(word: &str, lower: &str, exceptions: bool, out: &mut String) {
    let chars: Vec<char> = word.chars().collect();
    let lchars: Vec<char> = lower.chars().collect();
    // Lowercasing can change the char count (rare: İ); then skip the exception check.
    let aligned = chars.len() == lchars.len();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if let Some(d) = digraph_char(c) {
            out.push(d);
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        let pair = if aligned {
            (lchars.get(i).copied(), lchars.get(i + 1).copied())
        } else {
            (
                c.to_lowercase().next(),
                next.and_then(|n| n.to_lowercase().next()),
            )
        };
        let digraph = match pair {
            (Some('l'), Some('j')) => Some(('Љ', 'љ')),
            (Some('n'), Some('j')) => Some(('Њ', 'њ')),
            (Some('d'), Some('ž')) => Some(('Џ', 'џ')),
            _ => None,
        };
        if let Some((up, low)) = digraph {
            let split = exceptions && aligned && pair.0.is_some_and(|f| split_digraph(lower, i, f));
            if !split {
                out.push(if c.is_uppercase() { up } else { low });
                i += 2;
                continue;
            }
        }
        match lat_single(c) {
            Some(cy) => out.push(cy),
            None => out.push(c),
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyrillic_to_latin_exact() {
        let cyr = "Ђорђе Џаја, Љиљана Њего, Ћирић Чачак Жужа Шишмиш";
        assert_eq!(
            cyrillic_to_latin(cyr),
            "Đorđe Džaja, Ljiljana Njego, Ćirić Čačak Žuža Šišmiš"
        );
        assert_eq!(cyrillic_to_latin("ЉУБАВ ЊИВА ЏАК"), "LJUBAV NJIVA DŽAK");
        assert_eq!(cyrillic_to_latin("ПРЕДУЗЕЋЕЉ."), "PREDUZEĆELJ.");
        assert_eq!(cyrillic_to_latin("ПРЕДУЗЕЋЕ Љ."), "PREDUZEĆE Lj.");
        assert_eq!(cyrillic_to_latin("Љ."), "Lj.");
        assert_eq!(cyrillic_to_latin("plain ascii"), "plain ascii");
        assert!(matches!(cyrillic_to_latin("plain"), Cow::Borrowed(_)));
        // Non-Serbian Cyrillic letters are kept.
        assert_eq!(cyrillic_to_latin("щы"), "щы");
    }

    #[test]
    fn latin_to_cyrillic_digraphs_and_exceptions() {
        assert_eq!(
            latin_to_cyrillic("ljubav Ljubav LJUBAV"),
            "љубав Љубав ЉУБАВ"
        );
        assert_eq!(latin_to_cyrillic("Đorđe Džaja"), "Ђорђе Џаја");
        assert_eq!(
            latin_to_cyrillic("nadživeti podžupan"),
            "надживети поджупан"
        );
        assert_eq!(latin_to_cyrillic("odžak džep"), "оџак џеп");
        assert_eq!(
            latin_to_cyrillic("injekcija konjunkcija"),
            "инјекција конјункција"
        );
        assert_eq!(latin_to_cyrillic("konj njiva"), "коњ њива");
        assert_eq!(latin_to_cyrillic("ǅep ǉubav"), "Џеп љубав");
    }

    #[test]
    fn latin_to_cyrillic_keeps_foreign_and_numbers() {
        assert_eq!(
            latin_to_cyrillic("Windows, XII, broj 114/23, Exxon"),
            "Windows, XII, број 114/23, Exxon"
        );
        let all = LatinToCyrillic {
            keep_foreign: false,
            exceptions: true,
        };
        // Without the foreign-word rule, letters without a Cyrillic counterpart stay Latin.
        assert_eq!(latin_to_cyrillic_with("Word", all), "Wорд");
    }

    #[test]
    fn decomposed_input_is_composed() {
        assert_eq!(latin_to_cyrillic("c\u{30c}ac\u{30c}ak"), "чачак");
    }

    #[test]
    fn round_trip_serbian() {
        let lat = "Ugovor o zakupu poslovnog prostora zaključen je u Beogradu, đak, džep, ljiljan, njiva.";
        let cyr = latin_to_cyrillic(lat);
        assert_eq!(cyrillic_to_latin(&cyr), lat);
    }
}

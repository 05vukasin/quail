//! Exact Serbian transliteration between Cyrillic (ћирилица) and Latin (latinica).
//!
//! # Cyrillic → Latin
//!
//! Letter by letter, one to one except for the three letters that become digraphs:
//! `љ → lj`, `њ → nj`, `џ → dž` (and `ђ → đ`, `ћ → ć`, `ч → č`, `ж → ž`, `ш → š`, `ј → j`).
//! The case of an uppercase digraph follows its neighbours: `Љубав → Ljubav`, but
//! `ЉУБАВ → LJUBAV`, `ЊЊ → NJNJ`. An uppercase digraph letter is written all caps when the
//! next character is an uppercase letter, or when the next character is not a letter and
//! the previous one is an uppercase letter; otherwise it is title case. Combining marks on
//! a digraph letter go on its first Latin letter (`Њ́ива → Ńjiva`).
//!
//! Cyrillic letters that are not in the Serbian alphabet (Russian `я ю щ ы э й ё ъ ь`,
//! Macedonian `ѓ ќ ѕ`, Ukrainian `і ї є ґ`, ...) are left unchanged: this is a Serbian
//! transliterator, not a general romanizer. Everything that is not Cyrillic is copied.
//! [`cyrillic_to_latin_chars`] does it over a character stream without allocating.
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
//! 3. **Foreign words stay Latin** ([`LatinToCyrillic::keep_foreign`], on by default): a
//!    word with a Latin letter outside the Serbian alphabet (`q w x y`, `ä ö ü ß é ...`) is
//!    copied unchanged, so `Windows` or `Exxon` survive. A foreign word spelled only with
//!    Serbian letters (`Microsoft`) cannot be told apart and is converted.
//! 4. **Roman numerals stay Latin** ([`LatinToCyrillic::keep_roman`], on by default):
//!    all-caps words made of `I V X L C D M` (`IV`, `XII`; a lone `I` is the conjunction and
//!    converts). Acronyms convert, as standard Serbian Cyrillic writes them (`PDV → ПДВ`);
//!    [`LatinToCyrillic::keep_acronyms`] (off by default) keeps all-caps words of 2–6
//!    letters without a vowel (`PDV`, `JMBG`) Latin instead.
//! 5. **ASCII-ized text is not repaired.** `dj`, `c`, `s`, `z` written for `đ`, `č/ć`, `š`,
//!    `ž` are converted as written (`dj → дј`); the transliterator does not guess.
//!
//! Input in decomposed form (`c` + U+030C) is composed (NFC) first. Both directions are
//! linear in the input.

use std::borrow::Cow;

use unicode_normalization::char::is_combining_mark;
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

/// Serbian Cyrillic → Latin over a stream of characters, without allocating. See
/// [`cyrillic_to_latin_chars`].
pub struct CyrToLat<I: Iterator<Item = char>> {
    it: std::iter::Peekable<I>,
    prev: Option<char>,
    /// Second letter of a digraph, written after any combining marks of the first.
    deferred: Option<char>,
}

impl<I: Iterator<Item = char>> Iterator for CyrToLat<I> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if self.deferred.is_some() {
            if let Some(&m) = self.it.peek()
                && is_combining_mark(m)
            {
                self.it.next();
                return Some(m);
            }
            return self.deferred.take();
        }
        let c = self.it.next()?;
        let latin = if let Some(l) = cyr_lower(c) {
            l
        } else if let Some(u) = cyr_upper(c) {
            if matches!(c, 'Љ' | 'Њ' | 'Џ') {
                let caps = match self.it.peek().copied() {
                    Some(n) if n.is_alphabetic() => n.is_uppercase(),
                    _ => self.prev.is_some_and(char::is_uppercase),
                };
                if caps { all_caps_digraph(c) } else { u }
            } else {
                u
            }
        } else {
            self.prev = Some(c);
            return Some(c);
        };
        self.prev = Some(c);
        let mut chars = latin.chars();
        let first = chars.next();
        self.deferred = chars.next();
        first
    }
}

/// Serbian Cyrillic → Latin as a character iterator (no allocation;
/// [`fold`](crate::fold) builds keys with it).
pub fn cyrillic_to_latin_chars<I: IntoIterator<Item = char>>(chars: I) -> CyrToLat<I::IntoIter> {
    CyrToLat {
        it: chars.into_iter().peekable(),
        prev: None,
        deferred: None,
    }
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
    out.extend(cyrillic_to_latin_chars(text.chars()));
}

/// Options of [`latin_to_cyrillic_with`]. See the [module docs](self) for the policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent switches
pub struct LatinToCyrillic {
    /// Copy words with letters outside the Serbian Latin alphabet unchanged (default `true`).
    pub keep_foreign: bool,
    /// Apply the morpheme-boundary exception list (default `true`).
    pub exceptions: bool,
    /// Keep all-caps Roman numerals (`IV`, `XII`, `MCMXC`) Latin (default `true`).
    pub keep_roman: bool,
    /// Keep all-caps acronyms without a vowel (`PDV`, `JMBG`, `SFRJ`, 2–6 letters) Latin
    /// (default `false`: standard Serbian Cyrillic writes them in Cyrillic, `ПДВ`).
    pub keep_acronyms: bool,
}

impl Default for LatinToCyrillic {
    fn default() -> Self {
        Self {
            keep_foreign: true,
            exceptions: true,
            keep_roman: true,
            keep_acronyms: false,
        }
    }
}

/// Serbian Latin → Cyrillic with the default policy. Borrows when nothing changes.
///
/// ```
/// use quail_text::translit::latin_to_cyrillic;
/// assert_eq!(latin_to_cyrillic("Ljubljana, NJIVA, džep"), "Љубљана, ЊИВА, џеп");
/// assert_eq!(
///     latin_to_cyrillic("nadživeti Windows, član IV, PDV"),
///     "надживети Windows, члан IV, ПДВ"
/// );
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
    if out == *composed {
        return composed;
    }
    Cow::Owned(out)
}

/// Serbian Latin → Cyrillic, appended to `out`. Expects NFC input (see
/// [`latin_to_cyrillic_with`], which composes for you).
pub fn latin_to_cyrillic_into(text: &str, opts: LatinToCyrillic, out: &mut String) {
    out.reserve(text.len() * 2);
    let mut rest = text;
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
        let keep = (opts.keep_foreign && is_foreign(word))
            || (opts.keep_roman && is_roman(word))
            || (opts.keep_acronyms && is_acronym(word));
        if keep {
            out.push_str(word);
        } else {
            convert_word(word, opts.exceptions, out);
        }
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

/// An all-caps Roman numeral: two or more of `I V X L C D M`, or a single one other than
/// `I` (a lone `I` is too often the conjunction at a sentence start).
fn is_roman(word: &str) -> bool {
    let b = word.as_bytes();
    !b.is_empty()
        && b.len() <= 15
        && b.iter().all(|c| b"IVXLCDM".contains(c))
        && (b.len() >= 2 || b[0] != b'I')
}

/// An all-caps word of 2–6 ASCII letters without a vowel (`PDV`, `JMBG`).
fn is_acronym(word: &str) -> bool {
    let b = word.as_bytes();
    (2..=6).contains(&b.len())
        && b.iter().all(u8::is_ascii_uppercase)
        && !b.iter().any(|c| b"AEIOU".contains(c))
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
/// `(stem, byte offset of the "nj" in it)`.
const NJ_STEMS: [(&str, usize); 5] = [
    ("injek", 1),
    ("konjug", 2),
    ("konjunk", 2),
    ("vanjez", 2),
    ("tanjug", 2),
];

/// Whether the digraph starting at byte `at` of `word` is really two letters. Constant
/// time: the prefixes and stems are ASCII, so they are compared in place.
fn split_digraph(word: &str, at: usize, first: char) -> bool {
    let b = word.as_bytes();
    match first {
        'd' | 'D' => DZ_PREFIXES.iter().any(|p| {
            at + 1 == p.len()
                && b.get(..p.len())
                    .is_some_and(|w| w.eq_ignore_ascii_case(p.as_bytes()))
        }),
        'n' | 'N' => NJ_STEMS.iter().any(|&(s, off)| {
            at.checked_sub(off).is_some_and(|st| {
                b.get(st..st + s.len())
                    .is_some_and(|w| w.eq_ignore_ascii_case(s.as_bytes()))
            })
        }),
        _ => false,
    }
}

fn convert_word(word: &str, exceptions: bool, out: &mut String) {
    let mut it = word.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if let Some(d) = digraph_char(c) {
            out.push(d);
            continue;
        }
        let next = it.peek().map(|&(_, n)| n);
        let digraph = match (c, next) {
            ('l' | 'L', Some('j' | 'J')) => Some(('Љ', 'љ')),
            ('n' | 'N', Some('j' | 'J')) => Some(('Њ', 'њ')),
            ('d' | 'D', Some('ž' | 'Ž')) => Some(('Џ', 'џ')),
            _ => None,
        };
        if let Some((up, low)) = digraph
            && !(exceptions && split_digraph(word, i, c))
        {
            out.push(if c.is_uppercase() { up } else { low });
            it.next();
            continue;
        }
        out.push(lat_single(c).unwrap_or(c));
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
        assert_eq!(
            latin_to_cyrillic("NADŽIVETI Injekcija TANJUG"),
            "НАДЖИВЕТИ Инјекција ТАНЈУГ"
        );
        assert_eq!(latin_to_cyrillic("manjina Konjic"), "мањина Коњиц");
    }

    #[test]
    fn latin_to_cyrillic_keeps_foreign_roman_and_acronyms() {
        assert_eq!(
            latin_to_cyrillic("Windows, broj 114/23, Exxon"),
            "Windows, број 114/23, Exxon"
        );
        assert_eq!(
            latin_to_cyrillic("Član IV i VI, glava MCMXC, PDV i JMBG, UGOVOR"),
            "Члан IV и VI, глава MCMXC, ПДВ и ЈМБГ, УГОВОР"
        );
        assert_eq!(latin_to_cyrillic("I tako"), "И тако");
        let plain = LatinToCyrillic {
            keep_roman: false,
            ..LatinToCyrillic::default()
        };
        assert_eq!(latin_to_cyrillic_with("IV PDV", plain), "ИВ ПДВ");
        let acronyms = LatinToCyrillic {
            keep_acronyms: true,
            ..LatinToCyrillic::default()
        };
        assert_eq!(
            latin_to_cyrillic_with("IV PDV UGOVOR", acronyms),
            "IV PDV УГОВОР"
        );
        let all = LatinToCyrillic {
            keep_foreign: false,
            ..LatinToCyrillic::default()
        };
        // Without the foreign-word rule, letters without a Cyrillic counterpart stay Latin.
        assert_eq!(latin_to_cyrillic_with("Word", all), "Wорд");
        assert!(matches!(latin_to_cyrillic("Windows XII"), Cow::Borrowed(_)));
    }

    #[test]
    fn accents_on_digraphs() {
        assert_eq!(cyrillic_to_latin("Њ\u{301}ива"), "N\u{301}jiva");
        assert_eq!(cyrillic_to_latin("љ\u{300}"), "l\u{300}j");
    }

    #[test]
    fn linear_time_on_long_words() {
        use std::time::{Duration, Instant};
        for unit in ["nj", "dž", "lj", "a", "Њ"] {
            let time = |n: usize| {
                let s = unit.repeat(n);
                let t = Instant::now();
                let a = latin_to_cyrillic(&s).len();
                let b = cyrillic_to_latin(&s).len();
                assert!(a > 0 && b > 0);
                t.elapsed()
            };
            let (a, b) = (time(100_000), time(400_000));
            assert!(
                b < a * 10 + Duration::from_millis(50),
                "{unit}: {a:?} vs {b:?}"
            );
        }
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

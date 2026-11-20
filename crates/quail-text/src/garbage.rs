//! How much of a text looks like a broken text layer: the signal PDF extraction uses to
//! decide `needs_ocr`.
//!
//! [`garbage_report`] counts, over the non-whitespace characters of a text, the ones that
//! fall in one of these classes (each character is counted at most once):
//!
//! - **replacement**: U+FFFD, the decoder's "I could not map this";
//! - **private use**: U+E000..=U+F8FF and planes 15–16, glyph ids of fonts without a
//!   Unicode mapping;
//! - **control**: C0/C1 controls other than whitespace, noncharacters;
//! - **cid**: `(cid:123)` placeholders written by some extractors for unmapped glyphs;
//! - **mojibake**: UTF-8 read as Latin-1/Windows-1252: a lead character `Â Ã Ä Å Æ Ð Ñ Ø`
//!   followed by a continuation-range character (U+0080..=U+00BF or a Windows-1252
//!   punctuation like `€ ‚ „ … † ‡ ‰ Š ‹ Œ Ž ‘ ’ “ ” • – — ˜ ™ š › œ ž Ÿ`); both count;
//! - **isolated symbol**: a whitespace-delimited chunk that is a single character outside
//!   letters, digits and common punctuation (`.,;:!?-()[]"'/%&@#*+=§€$•·°…<>|_~^` and the
//!   typographic quotes and dashes);
//! - **implausible word**: a letter run (4+ letters) that mixes Latin and Cyrillic or Greek,
//!   switches from lower to upper case twice or more (`aBcDe`), or (5+ letters, not all
//!   caps) has no vowel at all (`r` counts as a vowel, as in Serbian `prst`); all of its
//!   letters count;
//! - **unnatural vowel share** (whole text, 100+ letters of a script): natural text in
//!   every supported language has 25–65 % vowels among its Latin (or Cyrillic) letters;
//!   a font whose glyphs map to the wrong letters does not. Then all letters of that
//!   script count. This catches substitution ciphers and shifted glyph ids that look like
//!   letters.
//!
//! [`garbage_ratio`] is the share of such characters, in `[0, 1]`. A clean text layer scores
//! near 0; [`NEEDS_OCR_RATIO`] is the threshold above which a page's text should be
//! distrusted. It is calibrated on the text layers of a private corpus of 369 business PDFs
//! (see the crate README for the numbers).

/// Garbage ratio above which a page's text layer should be treated as unusable (OCR it).
pub const NEEDS_OCR_RATIO: f32 = 0.10;

/// Fewer non-whitespace characters than this on a page are too little to judge; callers
/// usually treat such a page as image-only.
pub const MIN_JUDGED_CHARS: usize = 20;

/// Counts behind [`garbage_ratio`]. Counts are additive across pages via [`Self::merge`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GarbageReport {
    /// Non-whitespace characters.
    pub chars: usize,
    /// U+FFFD characters.
    pub replacement: usize,
    /// Private-use characters.
    pub private_use: usize,
    /// Control characters and noncharacters.
    pub control: usize,
    /// Characters of `(cid:N)` placeholders.
    pub cid: usize,
    /// Characters of mojibake pairs.
    pub mojibake: usize,
    /// Isolated odd symbols.
    pub isolated_symbol: usize,
    /// Letters of implausible words.
    pub implausible: usize,
    /// Letters of a script whose vowel share is unnatural (whole text judged at once).
    pub unnatural: usize,
}

impl GarbageReport {
    fn classified(&self) -> usize {
        self.replacement
            + self.private_use
            + self.control
            + self.cid
            + self.mojibake
            + self.isolated_symbol
            + self.implausible
    }

    /// Characters counted as garbage. The whole-text vowel check can overlap the
    /// per-character classes, so it enters as a max, not a sum.
    pub fn garbage(&self) -> usize {
        self.classified().max(self.unnatural).min(self.chars)
    }

    /// Garbage share of the non-whitespace characters, `0` for an empty text.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // a ratio
    pub fn ratio(&self) -> f32 {
        if self.chars == 0 {
            return 0.0;
        }
        (self.garbage() as f64 / self.chars as f64).min(1.0) as f32
    }

    /// Adds another report (e.g. of the next page).
    pub fn merge(&mut self, o: &Self) {
        self.chars += o.chars;
        self.replacement += o.replacement;
        self.private_use += o.private_use;
        self.control += o.control;
        self.cid += o.cid;
        self.mojibake += o.mojibake;
        self.isolated_symbol += o.isolated_symbol;
        self.implausible += o.implausible;
        self.unnatural += o.unnatural;
    }

    /// Whether this text should be treated as a broken text layer.
    pub fn needs_ocr(&self) -> bool {
        self.chars < MIN_JUDGED_CHARS || self.ratio() > NEEDS_OCR_RATIO
    }
}

/// Share of `text` that looks like broken text-layer output, see the module docs.
///
/// ```
/// use quail_text::garbage::garbage_ratio;
/// assert!(garbage_ratio("Ugovor o zakupu poslovnog prostora.") < 0.01);
/// assert!(garbage_ratio("\u{e001}\u{e002}\u{e003} \u{fffd}\u{fffd}") > 0.9);
/// ```
pub fn garbage_ratio(text: &str) -> f32 {
    garbage_report(text).ratio()
}

fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD)
}

fn is_noncharacter(c: char) -> bool {
    let u = c as u32;
    (0xFDD0..=0xFDEF).contains(&u) || (u & 0xFFFE) == 0xFFFE
}

fn is_mojibake_lead(c: char) -> bool {
    matches!(c, 'Â' | 'Ã' | 'Ä' | 'Å' | 'Æ' | 'Ð' | 'Ñ' | 'Ø')
}

fn is_mojibake_tail(c: char) -> bool {
    matches!(c as u32, 0x80..=0xBF)
        || matches!(
            c,
            '€' | '‚'
                | 'ƒ'
                | '„'
                | '…'
                | '†'
                | '‡'
                | 'ˆ'
                | '‰'
                | 'Š'
                | '‹'
                | 'Œ'
                | 'Ž'
                | '‘'
                | '’'
                | '“'
                | '”'
                | '•'
                | '–'
                | '—'
                | '˜'
                | '™'
                | 'š'
                | '›'
                | 'œ'
                | 'ž'
                | 'Ÿ'
        )
}

fn is_common_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '§' | '€'
                | '£'
                | '•'
                | '·'
                | '°'
                | '…'
                | '–'
                | '—'
                | '‐'
                | '‑'
                | '−'
                | '„'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '«'
                | '»'
                | '×'
                | '©'
                | '®'
                | '™'
                | '№'
                | '\u{25AA}'
                | '\u{25CF}'
                | '\u{F0B7}'
        )
}

fn is_vowel(c: char) -> bool {
    let c = c.to_lowercase().next().unwrap_or(c);
    matches!(
        c,
        'a' | 'e'
            | 'i'
            | 'o'
            | 'u'
            | 'y'
            | 'r'
            | 'á'
            | 'à'
            | 'â'
            | 'ä'
            | 'é'
            | 'è'
            | 'ê'
            | 'ë'
            | 'í'
            | 'ì'
            | 'î'
            | 'ï'
            | 'ó'
            | 'ò'
            | 'ô'
            | 'ö'
            | 'ú'
            | 'ù'
            | 'û'
            | 'ü'
            | 'а'
            | 'е'
            | 'и'
            | 'о'
            | 'у'
            | 'р'
            | 'я'
            | 'ю'
            | 'ы'
            | 'э'
            | 'ё'
            | 'і'
            | 'ї'
            | 'є'
    ) || (!c.is_ascii()
        && crate::script::script_of(c).is_some_and(|s| {
            s != crate::script::Script::Latin && s != crate::script::Script::Cyrillic
        }))
}

/// Scores one letter run; returns how many of its letters are implausible.
fn implausible_letters(word: &str) -> usize {
    use crate::script::{Script, script_of};
    let mut n = 0usize;
    let (mut latin, mut cyr, mut greek) = (false, false, false);
    let (mut upper, mut lower) = (0usize, 0usize);
    let mut case_flips = 0usize;
    let mut prev_lower = false;
    let mut vowel = false;
    for c in word.chars() {
        n += 1;
        match script_of(c) {
            Some(Script::Latin) => latin = true,
            Some(Script::Cyrillic) => cyr = true,
            Some(Script::Greek) => greek = true,
            _ => {}
        }
        if c.is_uppercase() {
            upper += 1;
            if prev_lower {
                case_flips += 1;
            }
            prev_lower = false;
        } else {
            lower += usize::from(c.is_lowercase());
            prev_lower = c.is_lowercase();
        }
        vowel |= is_vowel(c);
    }
    if n < 4 {
        return 0;
    }
    let mixed = u8::from(latin) + u8::from(cyr) + u8::from(greek) > 1;
    let chaos = case_flips >= 2;
    let no_vowel = n >= 5 && !vowel && lower > 0 && upper < n && (latin || cyr);
    if mixed || chaos || no_vowel { n } else { 0 }
}

/// Full counts for `text`, see the module docs.
pub fn garbage_report(text: &str) -> GarbageReport {
    let mut r = GarbageReport::default();
    let mut v = VowelCounts::default();
    for chunk in text.split_whitespace() {
        score_chunk(chunk, &mut r, &mut v);
    }
    r.unnatural = v.unnatural_letters();
    r
}

/// Letters and vowels per script, for the vowel-share check.
#[derive(Default)]
struct VowelCounts {
    latin: usize,
    latin_vowels: usize,
    cyrillic: usize,
    cyrillic_vowels: usize,
}

impl VowelCounts {
    fn add(&mut self, c: char) {
        use crate::script::{Script, script_of};
        match script_of(c) {
            Some(Script::Latin) => {
                self.latin += 1;
                let mut base = c;
                unicode_normalization::char::decompose_canonical(c, |d| {
                    if base == c {
                        base = d;
                    }
                });
                if matches!(base.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y') {
                    self.latin_vowels += 1;
                }
            }
            Some(Script::Cyrillic) => {
                self.cyrillic += 1;
                let l = c.to_lowercase().next().unwrap_or(c);
                if matches!(
                    l,
                    'а' | 'е' | 'и' | 'о' | 'у' | 'я' | 'ю' | 'ё' | 'ы' | 'э' | 'і' | 'ї' | 'є'
                ) {
                    self.cyrillic_vowels += 1;
                }
            }
            _ => {}
        }
    }

    /// Letters of a script whose vowel share is outside the natural range.
    fn unnatural_letters(&self) -> usize {
        let off = |letters: usize, vowels: usize| {
            #[allow(clippy::cast_precision_loss)] // a ratio
            let share = vowels as f64 / letters.max(1) as f64;
            letters >= VOWEL_MIN_LETTERS && !(VOWEL_SHARE.0..=VOWEL_SHARE.1).contains(&share)
        };
        let mut n = 0;
        if off(self.latin, self.latin_vowels) {
            n += self.latin;
        }
        if off(self.cyrillic, self.cyrillic_vowels) {
            n += self.cyrillic;
        }
        n
    }
}

/// Letters of one script a text needs before its vowel share is judged.
const VOWEL_MIN_LETTERS: usize = 100;

/// Natural vowel share of letters (`a e i o u y` and accented forms, Cyrillic vowels). On
/// the calibration corpus clean pages range 0.29–0.45 (0.1th to 99.9th percentile); glyph
/// substitutions land at 0.05–0.22 and Latin-1-shifted text above 0.9.
const VOWEL_SHARE: (f64, f64) = (0.25, 0.65);

fn score_chunk(chunk: &str, r: &mut GarbageReport, v: &mut VowelCounts) {
    let mut chars = chunk.char_indices().peekable();
    let mut count = 0usize;
    let mut first: Option<char> = None;
    let mut word_start: Option<usize> = None;
    while let Some((i, c)) = chars.next() {
        count += 1;
        first.get_or_insert(c);
        r.chars += 1;
        v.add(c);
        // Letter runs, scored when they end.
        if c.is_alphabetic() {
            word_start.get_or_insert(i);
        } else if let Some(ws) = word_start.take() {
            r.implausible += implausible_letters(&chunk[ws..i]);
        }
        if c == '\u{FFFD}' {
            r.replacement += 1;
        } else if is_private_use(c) {
            r.private_use += 1;
        } else if (c.is_control() && !c.is_whitespace()) || is_noncharacter(c) {
            r.control += 1;
        } else if c == '(' && chunk[i..].starts_with("(cid:") {
            let len = chunk[i..].find(')').map_or(chunk.len() - i, |e| e + 1);
            let cid_chars = chunk[i..i + len].chars().count();
            r.cid += cid_chars;
            r.chars += cid_chars - 1;
            count += cid_chars - 1;
            while chars.peek().is_some_and(|&(j, _)| j < i + len) {
                chars.next();
            }
            word_start = None;
        } else if is_mojibake_lead(c) && chars.peek().is_some_and(|&(_, d)| is_mojibake_tail(d)) {
            chars.next();
            r.chars += 1;
            count += 1;
            r.mojibake += 2;
            word_start = None;
        }
    }
    if let Some(ws) = word_start {
        r.implausible += implausible_letters(&chunk[ws..]);
    }
    if count == 1
        && let Some(c) = first
        && !c.is_alphanumeric()
        && !is_common_punct(c)
        && c != '\u{FFFD}'
        && !is_private_use(c)
        && !c.is_control()
    {
        r.isolated_symbol += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_texts_score_low() {
        for t in [
            "Ugovor o zakupu poslovnog prostora zaključen dana 12.05.2023. godine u Beogradu.",
            "Уговор о закупу пословног простора закључен је у Београду, члан 5. став 2.",
            "The parties agree that this Agreement shall be governed by the laws of Serbia.",
            "Iznos: 1.200,50 RSD — PDV 20% • Rok plaćanja: 15 dana (§ 3).",
            "Prst, trg, srp, crkva, vrt i krv: PDV JMBG SFRJ.",
        ] {
            assert!(garbage_ratio(t) < 0.02, "{t}: {:?}", garbage_report(t));
        }
    }

    #[test]
    fn broken_layers_score_high() {
        // Private-use glyph ids.
        let pua: String = (0xE000u32..0xE040).filter_map(char::from_u32).collect();
        assert!(garbage_ratio(&pua) > 0.95);
        // Mojibake of Serbian Latin and Cyrillic (UTF-8 bytes shown as Windows-1252).
        let moj = "UgovorÄ\u{8d} o zakupu Ä‡ Å¡ Å¾ Ð£Ð³Ð¾Ð²Ð¾Ñ€";
        assert!(garbage_ratio(moj) > 0.3, "{:?}", garbage_report(moj));
        // cid placeholders.
        let cid = "(cid:12)(cid:7)(cid:44) (cid:3)(cid:9)";
        assert!(garbage_ratio(cid) > 0.95);
        // Glyph soup.
        let soup = "ˇ ˘ ¨ ¸ ˝ ˛ ¦ ¤ xQzKpWm bRtVx";
        assert!(garbage_ratio(soup) > 0.5, "{:?}", garbage_report(soup));
        // Homoglyph-mixed words (Latin 'o' inside Cyrillic).
        let mixed = "Угoвoр закупa";
        assert!(garbage_ratio(mixed) > 0.4, "{:?}", garbage_report(mixed));
    }

    const CLEAN_SR: &str = "Ugovor o zakupu poslovnog prostora zaključen je između zakupodavca i \
        zakupca. Zakupac je dužan da plaća zakupninu do petog u mesecu za tekući mesec, a \
        u slučaju kašnjenja plaća zakonsku zateznu kamatu. Ugovor stupa na snagu danom \
        potpisivanja i važi godinu dana.";

    #[test]
    fn whole_text_vowel_share() {
        assert!(
            garbage_ratio(CLEAN_SR) < 0.02,
            "{:?}",
            garbage_report(CLEAN_SR)
        );
        let cyr = crate::translit::latin_to_cyrillic(CLEAN_SR);
        assert!(garbage_ratio(&cyr) < 0.02, "{:?}", garbage_report(&cyr));
        // A font whose glyphs map to the wrong letters (substitution).
        let key = b"qwrtzpsdfghjklxcvbnmaeiouy";
        let sub: String = CLEAN_SR
            .chars()
            .map(|c| match c {
                'a'..='z' => char::from(key[usize::from(c as u8 - b'a')]),
                c => c,
            })
            .collect();
        assert!(
            garbage_ratio(&sub) > NEEDS_OCR_RATIO,
            "{:?}",
            garbage_report(&sub)
        );
        // Glyph ids shifted by 29, a classic of subset fonts without a ToUnicode map.
        let shifted: String = CLEAN_SR
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() {
                    char::from(c as u8 - 29)
                } else {
                    c
                }
            })
            .collect();
        assert!(
            garbage_ratio(&shifted) > NEEDS_OCR_RATIO,
            "{:?}",
            garbage_report(&shifted)
        );
    }

    #[test]
    fn reports_merge_and_empty() {
        assert!(garbage_ratio("") <= 0.0);
        assert!(garbage_report("").needs_ocr());
        let mut a = garbage_report("abc \u{fffd}");
        let b = garbage_report("def");
        a.merge(&b);
        assert_eq!(a.chars, 7);
        assert_eq!(a.replacement, 1);
    }

    #[test]
    fn hostile() {
        let _ = garbage_report("(cid:");
        let _ = garbage_report("((cid:1");
        let _ = garbage_report("Ã");
        let _ = garbage_report("\u{ffff}\u{10fffe}");
    }
}

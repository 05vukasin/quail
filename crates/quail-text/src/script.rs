//! Writing-system (script) detection: how much of a text's letters are Latin, Cyrillic,
//! Greek or something else.
//!
//! Only letters count (`char::is_alphabetic`); digits, punctuation and spaces do not. Script
//! membership is by Unicode block, which is exact for the scripts we care about and cheap
//! (no tables): Latin = Basic Latin letters, Latin-1 Supplement letters, Latin Extended-A/B,
//! IPA, Latin Extended Additional/C/D/E and fullwidth Latin; Cyrillic = U+0400..=U+052F plus
//! Cyrillic Extended-A/B/C; Greek = Greek and Coptic plus Greek Extended.

/// A script family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Script {
    /// Latin alphabet (Serbian latinica, English, ...).
    Latin,
    /// Cyrillic alphabet (Serbian ćirilica, Russian, ...).
    Cyrillic,
    /// Greek alphabet.
    Greek,
    /// Any other letter (Arabic, CJK, Hebrew, ...).
    Other,
}

impl Script {
    /// Lowercase name, as emitted in facts: `latin`, `cyrillic`, `greek`, `other`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Latin => "latin",
            Self::Cyrillic => "cyrillic",
            Self::Greek => "greek",
            Self::Other => "other",
        }
    }
}

/// Script of a letter, `None` when `c` is not a letter.
#[inline]
pub fn script_of(c: char) -> Option<Script> {
    if c.is_ascii() {
        return c.is_ascii_alphabetic().then_some(Script::Latin);
    }
    // Fast paths for the blocks that dominate our text (no table lookup).
    match c as u32 {
        0x00C0..=0x00D6 | 0x00D8..=0x00F6 | 0x00F8..=0x024F => return Some(Script::Latin),
        0x0400..=0x0481 | 0x048A..=0x052F => return Some(Script::Cyrillic),
        _ => {}
    }
    if !c.is_alphabetic() {
        return None;
    }
    Some(match c as u32 {
        0x00C0..=0x024F
        | 0x0250..=0x02AF
        | 0x1E00..=0x1EFF
        | 0x2C60..=0x2C7F
        | 0xA720..=0xA7FF
        | 0xAB30..=0xAB6F
        | 0xFF21..=0xFF3A
        | 0xFF41..=0xFF5A
        | 0x00AA
        | 0x00BA => Script::Latin,
        0x0400..=0x052F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => Script::Cyrillic,
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
        _ => Script::Other,
    })
}

/// Letter counts per script. Add text with [`ScriptCounts::add`]; counts are additive, so
/// page counts sum to the document's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptCounts {
    /// Latin letters.
    pub latin: u64,
    /// Cyrillic letters.
    pub cyrillic: u64,
    /// Greek letters.
    pub greek: u64,
    /// Letters of any other script.
    pub other: u64,
}

impl ScriptCounts {
    /// Counts of one text.
    pub fn of(text: &str) -> Self {
        let mut s = Self::default();
        s.add(text);
        s
    }

    /// Adds the letters of `text`.
    pub fn add(&mut self, text: &str) {
        for c in text.chars() {
            match script_of(c) {
                Some(Script::Latin) => self.latin += 1,
                Some(Script::Cyrillic) => self.cyrillic += 1,
                Some(Script::Greek) => self.greek += 1,
                Some(Script::Other) => self.other += 1,
                None => {}
            }
        }
    }

    /// Adds another set of counts.
    pub fn merge(&mut self, other: &Self) {
        self.latin += other.latin;
        self.cyrillic += other.cyrillic;
        self.greek += other.greek;
        self.other += other.other;
    }

    /// Total letters.
    pub fn letters(&self) -> u64 {
        self.latin + self.cyrillic + self.greek + self.other
    }

    /// Shares of the letters, each in `[0, 1]`, summing to 1 (all 0 when there are none).
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // ratios
    pub fn ratios(&self) -> ScriptRatios {
        let n = self.letters();
        if n == 0 {
            return ScriptRatios::default();
        }
        let n = n as f64;
        let r = |x: u64| (x as f64 / n) as f32;
        ScriptRatios {
            latin: r(self.latin),
            cyrillic: r(self.cyrillic),
            greek: r(self.greek),
            other: r(self.other),
        }
    }

    /// The script with the most letters, `None` when there are no letters. Ties go to the
    /// earlier variant (Latin first).
    pub fn dominant(&self) -> Option<Script> {
        if self.letters() == 0 {
            return None;
        }
        let all = [
            (Script::Latin, self.latin),
            (Script::Cyrillic, self.cyrillic),
            (Script::Greek, self.greek),
            (Script::Other, self.other),
        ];
        all.iter()
            .fold(None, |best: Option<(Script, u64)>, &(s, n)| match best {
                Some((_, bn)) if bn >= n => best,
                _ => Some((s, n)),
            })
            .map(|(s, _)| s)
    }
}

/// Shares of letters per script, see [`ScriptCounts::ratios`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScriptRatios {
    /// Latin share.
    pub latin: f32,
    /// Cyrillic share.
    pub cyrillic: f32,
    /// Greek share.
    pub greek: f32,
    /// Other share.
    pub other: f32,
}

impl ScriptRatios {
    /// Share of the largest script.
    pub fn max(&self) -> f32 {
        self.latin
            .max(self.cyrillic)
            .max(self.greek)
            .max(self.other)
    }
}

/// Script counts of `text`; shortcut for [`ScriptCounts::of`].
pub fn detect_scripts(text: &str) -> ScriptCounts {
    ScriptCounts::of(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_letters_only() {
        let s = ScriptCounts::of("Ugovor 114/23 — Уговор; Ωμέγα 漢字!");
        assert_eq!(s.latin, 6);
        assert_eq!(s.cyrillic, 6);
        assert_eq!(s.greek, 5);
        assert_eq!(s.other, 2);
        assert_eq!(s.dominant(), Some(Script::Latin));
        let r = s.ratios();
        assert!((r.latin + r.cyrillic + r.greek + r.other - 1.0).abs() < 1e-6);
    }

    #[test]
    fn serbian_letters() {
        assert_eq!(script_of('đ'), Some(Script::Latin));
        assert_eq!(script_of('ž'), Some(Script::Latin));
        assert_eq!(script_of('ђ'), Some(Script::Cyrillic));
        assert_eq!(script_of('џ'), Some(Script::Cyrillic));
        assert_eq!(script_of('7'), None);
        assert_eq!(ScriptCounts::of("12 34").dominant(), None);
        assert_eq!(
            ScriptCounts::of("Београд").dominant(),
            Some(Script::Cyrillic)
        );
    }
}

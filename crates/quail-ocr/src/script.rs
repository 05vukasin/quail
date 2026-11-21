//! Script of a line or a page, from its letters.

use serde::{Deserialize, Serialize};

/// Writing system of a piece of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Script {
    /// No letters (digits and punctuation only, or nothing).
    #[default]
    Unknown,
    /// Latin letters, including Serbian/Croatian `č ć š ž đ`.
    Latin,
    /// Cyrillic letters, including Serbian `ђ ј љ њ ћ џ`.
    Cyrillic,
    /// Both, neither dominating (at least 20% of the letters are of the minority script).
    Mixed,
    /// Letters of another script dominate.
    Other,
}

impl Script {
    /// Lowercase name, as written into facts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Latin => "latin",
            Self::Cyrillic => "cyrillic",
            Self::Mixed => "mixed",
            Self::Other => "other",
        }
    }
}

/// Letter counts by script.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptCounts {
    /// Latin letters.
    pub latin: usize,
    /// Cyrillic letters.
    pub cyrillic: usize,
    /// Letters of any other script.
    pub other: usize,
}

impl ScriptCounts {
    /// Counts the letters of `text`.
    pub fn of(text: &str) -> Self {
        let mut c = Self::default();
        c.add(text);
        c
    }

    /// Adds the letters of `text`.
    pub fn add(&mut self, text: &str) {
        for ch in text.chars() {
            if !ch.is_alphabetic() {
                continue;
            }
            match ch {
                'A'..='Z' | 'a'..='z' | '\u{00C0}'..='\u{024F}' | '\u{1E00}'..='\u{1EFF}' => {
                    self.latin += 1;
                }
                '\u{0400}'..='\u{052F}' => self.cyrillic += 1,
                _ => self.other += 1,
            }
        }
    }

    /// Adds counts.
    pub fn merge(&mut self, o: Self) {
        self.latin += o.latin;
        self.cyrillic += o.cyrillic;
        self.other += o.other;
    }

    /// All letters.
    pub fn letters(&self) -> usize {
        self.latin + self.cyrillic + self.other
    }

    /// Share of Cyrillic among all letters (0 when there are none).
    pub fn cyrillic_share(&self) -> f32 {
        ratio(self.cyrillic, self.letters())
    }

    /// The dominant script.
    pub fn script(&self) -> Script {
        let n = self.letters();
        if n == 0 {
            return Script::Unknown;
        }
        if self.other * 2 > n {
            return Script::Other;
        }
        let (lat, cyr) = (self.latin, self.cyrillic);
        let minority = lat.min(cyr);
        if minority * 5 >= lat + cyr && minority > 0 {
            Script::Mixed
        } else if cyr > lat {
            Script::Cyrillic
        } else {
            Script::Latin
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn ratio(a: usize, b: usize) -> f32 {
    if b == 0 { 0.0 } else { a as f32 / b as f32 }
}

/// Script of `text`.
pub fn script_of(text: &str) -> Script {
    ScriptCounts::of(text).script()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serbian_both_scripts() {
        assert_eq!(
            script_of("Ugovor o kupoprodaji, član 5, đak"),
            Script::Latin
        );
        assert_eq!(
            script_of("Уговор о купопродаји ђ ј љ њ ћ џ"),
            Script::Cyrillic
        );
        assert_eq!(script_of("Уговор Ugovor"), Script::Mixed);
        assert_eq!(
            script_of("Уговор о купопродаји бр. 12 PDF"),
            Script::Cyrillic
        );
        assert_eq!(script_of("12.03.2024 - 1.500,00"), Script::Unknown);
        assert_eq!(script_of("契約書です"), Script::Other);
    }

    #[test]
    fn shares() {
        let c = ScriptCounts::of("ab вг");
        assert_eq!(c.letters(), 4);
        assert!((c.cyrillic_share() - 0.5).abs() < 1e-6);
        assert!((ScriptCounts::default().cyrillic_share()).abs() < 1e-6);
    }
}

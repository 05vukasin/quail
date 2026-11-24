//! Is a page's text layer usable, or should the page go to OCR?
//!
//! The text itself is judged by `quail-text`'s [`GarbageReport`]: its verdict `Garbage`
//! (replacement and private-use glyphs, `(cid:N)`, mojibake, implausible words, an unnatural
//! vowel share, Cyrillic read as Latin look-alikes) makes a page need OCR. Only a layer that
//! is unreadable glyphs ([`GarbageReport::broken_ratio`] or unmapped glyphs above
//! [`MAX_GARBAGE`]) is dropped; a look-alike layer keeps its text (its digits and layout are
//! right) until OCR replaces it (the OCR analyzer replaces every page with `needs_ocr`).
//!
//! A font whose `ToUnicode` maps two of its glyphs to the same letter is a second, font-level
//! signal: the text reads almost right (`мптпрних впзила` for `моторних возила`, Word 2010's
//! broken `bfrange`s) and passes every text check, but the page shows something else. When
//! such letters ([`Signals::ambiguous`]) make at least [`MIN_AMBIGUOUS`] glyphs and
//! [`AMBIGUOUS_SHARE`] of the page's characters, the page needs OCR and keeps its text. Pages
//! that are mostly an image are left out: their text is an OCR layer, and capture fonts
//! (Acrobat's "searchable image") draw each letter with several glyphs traced from the scan.

use quail_text::garbage::GarbageReport;

/// Fewer non-whitespace characters than this count as "no text".
const MIN_TEXT_CHARS: usize = 16;
/// Pages with less text than this, mostly covered by images, are scans with a stamp or a
/// page number on top.
const SCAN_MAX_TEXT_CHARS: usize = 200;
/// Image coverage above which a page is "mostly image".
const MOSTLY_IMAGE: f32 = 0.6;
/// A text layer with more unreadable glyphs than this is dropped.
const MAX_GARBAGE: f32 = 0.35;
/// Ambiguous letters a page needs before its font mapping is distrusted.
const MIN_AMBIGUOUS: usize = 40;
/// Share of a page's characters that ambiguous letters must reach. Broken Word 2010 mappings
/// land at 5-7 % (one letter, Cyrillic `о`, shown as another); on the dev corpora the only
/// clean page with ambiguous letters has 2 of 584 (0.3 %).
const AMBIGUOUS_SHARE: f32 = 0.03;
/// Paths above which a page without text likely has its text drawn as outlines.
const OUTLINED_TEXT_PATHS: usize = 200;
/// Images covering less than this share of a text-less page (logos, 1-px spacers) do not
/// make it an OCR candidate.
const MIN_SCAN_COVERAGE: f32 = 0.2;

/// The measurements the verdict is based on.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Signals {
    /// Non-whitespace characters of the extracted text.
    pub chars: usize,
    /// Glyphs drawn without a Unicode mapping.
    pub unmapped: usize,
    /// Letters drawn by visible glyphs whose letter another glyph of the same font also maps
    /// to (a broken `ToUnicode`, see the module docs).
    pub ambiguous: usize,
    /// `quail-text`'s report on the extracted text.
    pub report: GarbageReport,
    /// Share of the page covered by images.
    pub image_coverage: f32,
    /// Number of images drawn.
    pub images: usize,
    /// Number of vector paths drawn.
    pub paths: usize,
}

/// Verdict for one page.
#[allow(clippy::struct_excessive_bools)] // four independent flags of one page
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Verdict {
    /// Keep the native text.
    pub keep_text: bool,
    /// Send to OCR.
    pub needs_ocr: bool,
    /// The page is an image with (almost) no text: a scan.
    pub image_only: bool,
    /// The page is a scan that needs OCR: image-only, or mostly an image whose text layer
    /// is too thin or garbage (a bad OCR layer).
    pub scanned: bool,
}

pub(crate) fn judge(s: &Signals) -> Verdict {
    #[allow(clippy::cast_precision_loss)]
    let unmapped_share = if s.chars + s.unmapped == 0 {
        0.0
    } else {
        s.unmapped as f32 / (s.chars + s.unmapped) as f32
    };
    let broken = s.report.broken_ratio().max(unmapped_share);
    let has_text = s.chars >= MIN_TEXT_CHARS;
    let mostly_image = s.images > 0 && s.image_coverage >= MOSTLY_IMAGE;
    let image_only = s.images > 0 && !has_text && s.image_coverage >= MIN_SCAN_COVERAGE;

    if has_text && broken > MAX_GARBAGE {
        return Verdict {
            keep_text: false,
            needs_ocr: true,
            image_only: false,
            scanned: mostly_image,
        };
    }
    #[allow(clippy::cast_precision_loss)]
    let ambiguous_font = !mostly_image
        && s.ambiguous >= MIN_AMBIGUOUS
        && s.ambiguous as f32 >= AMBIGUOUS_SHARE * s.chars as f32;
    let needs_ocr = if has_text {
        s.report.needs_ocr() || ambiguous_font || (mostly_image && s.chars < SCAN_MAX_TEXT_CHARS)
    } else {
        (s.images > 0 && s.image_coverage >= MIN_SCAN_COVERAGE)
            || s.paths >= OUTLINED_TEXT_PATHS
            || s.unmapped >= MIN_TEXT_CHARS
    };
    let image_only = image_only || (mostly_image && s.chars < SCAN_MAX_TEXT_CHARS && needs_ocr);
    Verdict {
        keep_text: has_text || s.chars > 0,
        needs_ocr,
        image_only,
        scanned: image_only || (needs_ocr && mostly_image),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_text::garbage::garbage_report;

    const CLEAN: &str = "Ugovor o zakupu poslovnog prostora zaključen je između zakupodavca i \
        zakupca. Zakupac je dužan da plaća zakupninu do petog u mesecu za tekući mesec.";

    fn text(t: &str) -> Signals {
        Signals {
            chars: t.chars().filter(|c| !c.is_whitespace()).count(),
            report: garbage_report(t),
            ..Signals::default()
        }
    }

    #[test]
    fn verdicts() {
        assert_eq!(
            judge(&text(&CLEAN.repeat(10))),
            Verdict {
                keep_text: true,
                needs_ocr: false,
                image_only: false,
                scanned: false
            }
        );
        let scan = Signals {
            images: 1,
            image_coverage: 0.98,
            ..Signals::default()
        };
        let v = judge(&scan);
        assert!(v.needs_ocr && v.image_only && v.scanned);
        // An OCR'd scan: full-page image plus an invisible text layer.
        let ocrd = Signals {
            images: 1,
            image_coverage: 1.0,
            ..text(&CLEAN.repeat(20))
        };
        assert!(!judge(&ocrd).needs_ocr && !judge(&ocrd).scanned);
        let blank = Signals::default();
        assert!(!judge(&blank).needs_ocr);
        // A logo on an otherwise empty page is not a scan.
        let logo = Signals {
            images: 1,
            image_coverage: 0.02,
            ..Signals::default()
        };
        assert!(!judge(&logo).needs_ocr && !judge(&logo).image_only);
        // Private-use glyphs: dropped and OCR'd.
        let pua: String = (0xE000u32..0xE200).filter_map(char::from_u32).collect();
        let v = judge(&text(&pua));
        assert!(!v.keep_text && v.needs_ocr);
        // Glyphs without a Unicode mapping count like garbage.
        let unmapped = Signals {
            unmapped: 900,
            ..text(CLEAN)
        };
        assert!(!judge(&unmapped).keep_text);
    }

    #[test]
    fn ambiguous_font_mapping_is_kept_and_ocrd() {
        // A page whose font maps two glyphs to `п` (Word 2010's broken `bfrange`): the text
        // passes every text check, the font says it is wrong.
        let page = Signals {
            ambiguous: 120,
            ..text(&CLEAN.repeat(12))
        };
        assert!(!page.report.needs_ocr());
        let v = judge(&page);
        assert!(v.keep_text && v.needs_ocr && !v.scanned, "{v:?}");
        // A couple of duplicate glyphs (a font with two `d`s) is not enough.
        let few = Signals {
            ambiguous: 2,
            ..text(&CLEAN.repeat(4))
        };
        assert!(!judge(&few).needs_ocr);
        // Below the share: a long page with a few dozen ambiguous letters.
        let thin = Signals {
            ambiguous: 45,
            ..text(&CLEAN.repeat(20))
        };
        assert!(!judge(&thin).needs_ocr);
        // A scan with a capture font (several glyphs per letter, traced from the image).
        let capture = Signals {
            ambiguous: 300,
            images: 1,
            image_coverage: 1.0,
            ..text(&CLEAN.repeat(12))
        };
        assert!(!judge(&capture).needs_ocr);
    }

    #[test]
    fn lookalike_layer_is_kept_and_ocrd() {
        // Cyrillic read as Latin look-alikes (an OCR layer from a Latin-only engine).
        let garbled = "Ha ocHOBy 4naHa 146. cTaB 1. 3aKoHa o jaBHMM Ha6aBKaMa M M3BeuiTaja o \
            nocTynKy jaBHe Ha6aBKe ycnyra, AMpeKTop AreHuMje AOHocM OflnyKy o AOAenM yroBopa \
            3a Ha6aBKy ycnyra. YroBop ce AOAerbyje noHyhavy ca ceAMLUTeM y Eeorpafly.";
        let v = judge(&text(garbled));
        assert!(
            v.keep_text && v.needs_ocr && !v.image_only && !v.scanned,
            "{v:?}"
        );
        // The same under a full-page scan image.
        let scan = Signals {
            images: 1,
            image_coverage: 1.0,
            ..text(&garbled.repeat(4))
        };
        let v = judge(&scan);
        assert!(v.keep_text && v.needs_ocr && v.scanned, "{v:?}");
    }
}

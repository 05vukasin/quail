//! Is a page's text layer usable, or should the page go to OCR?
//!
//! TODO: replace `garbage_ratio` with `quail-text`'s `garbage_ratio` once it lands; this one
//! only looks at the character classes, not at words or language.

/// Fewer non-whitespace characters than this count as "no text".
const MIN_TEXT_CHARS: usize = 16;
/// Pages with less text than this, mostly covered by images, are scans with a stamp or a
/// page number on top.
const SCAN_MAX_TEXT_CHARS: usize = 200;
/// Image coverage above which a page is "mostly image".
const MOSTLY_IMAGE: f32 = 0.6;
/// A text layer with more garbage than this is rejected.
const MAX_GARBAGE: f32 = 0.35;
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
    /// Share of [`Self::chars`] that look like garbage.
    pub garbage: f32,
    /// Share of the page covered by images.
    pub image_coverage: f32,
    /// Number of images drawn.
    pub images: usize,
    /// Number of vector paths drawn.
    pub paths: usize,
}

/// Verdict for one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Verdict {
    /// Keep the native text.
    pub keep_text: bool,
    /// Send to OCR.
    pub needs_ocr: bool,
    /// The page is an image with (almost) no text: a scan.
    pub image_only: bool,
}

pub(crate) fn judge(s: &Signals) -> Verdict {
    #[allow(clippy::cast_precision_loss)]
    let unmapped_share = if s.chars + s.unmapped == 0 {
        0.0
    } else {
        s.unmapped as f32 / (s.chars + s.unmapped) as f32
    };
    let garbage = s.garbage.max(unmapped_share);
    let has_text = s.chars >= MIN_TEXT_CHARS;
    let mostly_image = s.images > 0 && s.image_coverage >= MOSTLY_IMAGE;
    let image_only = s.images > 0 && !has_text && s.image_coverage >= MIN_SCAN_COVERAGE;

    if has_text && garbage > MAX_GARBAGE {
        return Verdict {
            keep_text: false,
            needs_ocr: true,
            image_only: false,
        };
    }
    let needs_ocr = if has_text {
        mostly_image && s.chars < SCAN_MAX_TEXT_CHARS
    } else {
        (s.images > 0 && s.image_coverage >= MIN_SCAN_COVERAGE)
            || s.paths >= OUTLINED_TEXT_PATHS
            || s.unmapped >= MIN_TEXT_CHARS
    };
    Verdict {
        keep_text: has_text || s.chars > 0,
        needs_ocr,
        image_only: image_only || (mostly_image && s.chars < SCAN_MAX_TEXT_CHARS && needs_ocr),
    }
}

/// Share of non-whitespace characters that are replacement characters, private-use code
/// points or controls: the typical output of fonts without a usable `ToUnicode` map.
pub(crate) fn garbage_ratio(text: &str) -> f32 {
    let mut total = 0usize;
    let mut bad = 0usize;
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        total += 1;
        let pua = matches!(u32::from(c), 0xE000..=0xF8FF | 0x000F_0000..=0x0010_FFFF);
        if c == '\u{FFFD}' || pua || c.is_control() {
            bad += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    if total == 0 {
        0.0
    } else {
        bad as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        let text = Signals {
            chars: 2_000,
            ..Signals::default()
        };
        assert_eq!(
            judge(&text),
            Verdict {
                keep_text: true,
                needs_ocr: false,
                image_only: false
            }
        );
        let scan = Signals {
            images: 1,
            image_coverage: 0.98,
            ..Signals::default()
        };
        let v = judge(&scan);
        assert!(v.needs_ocr && v.image_only);
        // An OCR'd scan: full-page image plus an invisible text layer.
        let ocrd = Signals {
            chars: 3_000,
            images: 1,
            image_coverage: 1.0,
            ..Signals::default()
        };
        assert!(!judge(&ocrd).needs_ocr);
        let blank = Signals::default();
        assert!(!judge(&blank).needs_ocr);
        // A logo on an otherwise empty page is not a scan.
        let logo = Signals {
            images: 1,
            image_coverage: 0.02,
            ..Signals::default()
        };
        assert!(!judge(&logo).needs_ocr && !judge(&logo).image_only);
        let garbage = Signals {
            chars: 500,
            garbage: 0.9,
            ..Signals::default()
        };
        assert!(!judge(&garbage).keep_text && judge(&garbage).needs_ocr);
    }

    #[test]
    fn garbage() {
        assert!(garbage_ratio("normal text") < 0.01);
        assert!(garbage_ratio("\u{FFFD}\u{FFFD}\u{E001}a") > 0.7);
        assert!(garbage_ratio("").abs() < f32::EPSILON);
    }
}

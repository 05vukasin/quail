//! The document text the text sketches read, within the file's limits.

use quail_core::{Document, Emitter, Limits, Result};
use quail_text::normalize::{NormalizeOptions, normalize_into};

/// The longest prefix of `text` within `budget` bytes, on a char boundary.
pub(crate) fn within(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Page texts joined by `'\n'` (normalized with the `quail-text` defaults when `normalize`),
/// reading at most `Limits::max_text_bytes` of page text and skipping pages that need OCR.
/// Checks the deadline per page and records a problem when the limit stops it.
pub(crate) fn document_text(
    doc: &Document,
    limits: &Limits,
    normalize: bool,
    out: &mut Emitter,
) -> Result<String> {
    let mut budget = usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX);
    let opts = NormalizeOptions::default();
    let mut text = String::new();
    for page in &doc.pages {
        out.check_deadline()?;
        if page.needs_ocr || page.text.is_empty() {
            continue;
        }
        if budget == 0 {
            out.problem(format!(
                "{}: text limit reached, pages from {} not sketched",
                out.src(),
                page.index
            ));
            break;
        }
        let part = within(&page.text, budget);
        budget -= part.len();
        if !text.is_empty() {
            text.push('\n');
        }
        if normalize {
            normalize_into(part, &opts, &mut text);
        } else {
            text.push_str(part);
        }
    }
    Ok(text)
}

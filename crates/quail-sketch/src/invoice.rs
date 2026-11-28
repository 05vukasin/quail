//! Invoice layout fingerprint, ported as-is from the existing deployment of [`crate::compat`].
//!
//! Features: `label:<l>` for each of the 16 [`LABELS`] that occurs as whole words in the
//! lowercased, white-space-collapsed text, plus `num:<shape>` for each distinct
//! invoice-number-like code (`\b[A-Za-z]{2,}(?:[-/][A-Za-z0-9]+){1,4}\b` containing a digit)
//! with digit runs collapsed to `#` and lowercased (`ABC-US-1024` → `abc-us-#`). Both lists
//! sorted, labels first, joined by `'\n'` and digested with BLAKE2b-64 ([`crate::compat::PERSONALIZATION`]).
//!
//! Compare for **equality**: one issuer's invoices from one billing system share labels and
//! number shape. The value is the Python `invoice_layout_fp` hex read as a big-endian integer.

use std::sync::LazyLock;

use crate::compat::{Bounded, RegexLimit, digest_int};

/// Invoice field labels, lowercase and space-separated.
pub const LABELS: [&str; 16] = [
    "invoice number",
    "invoice no",
    "invoice date",
    "bill to",
    "sold to",
    "remit to",
    "purchase order",
    "po number",
    "payment terms",
    "due date",
    "tax id",
    "vat",
    "subtotal",
    "total due",
    "amount due",
    "balance due",
];

static INVOICE_NUMBER: LazyLock<Bounded> = LazyLock::new(|| {
    Bounded::new(
        "[A-Za-z]",
        r"[A-Za-z]{2,}(?:[-/][A-Za-z0-9]+){1,4}",
        r"[A-Za-z]{2,}(?:[-/][A-Za-z0-9]+){1,4}(?![\p{L}\p{N}_])",
    )
});
static DIGIT_RUN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\p{Nd}+").expect("valid constant pattern"));
static WHITESPACE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\s\x1C-\x1F]+").expect("valid constant pattern"));

/// What the fingerprint is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvoiceFeatures {
    /// Labels found, sorted, without the `label:` prefix.
    pub labels: Vec<&'static str>,
    /// Distinct number shapes (`abc-us-#`), in order of first occurrence.
    pub patterns: Vec<String>,
}

impl InvoiceFeatures {
    /// The feature lines, as the original joins them.
    pub fn lines(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.labels.iter().map(|l| format!("label:{l}")).collect();
        labels.sort_unstable();
        let mut nums: Vec<String> = self.patterns.iter().map(|p| format!("num:{p}")).collect();
        nums.sort_unstable();
        labels.extend(nums);
        labels
    }

    /// The fingerprint (`invoice_layout_fp`).
    pub fn fingerprint(&self) -> u64 {
        digest_int(&self.lines().join("\n"))
    }
}

/// `invoice_number_patterns(text)`.
pub fn number_patterns(text: &str) -> Result<Vec<String>, RegexLimit> {
    let mut out: Vec<String> = Vec::new();
    patterns_into(text, &mut out)?;
    Ok(out)
}

/// The number shapes of `text` not yet in `out`, appended in order.
fn patterns_into(text: &str, out: &mut Vec<String>) -> Result<(), RegexLimit> {
    for (a, b) in INVOICE_NUMBER.matches(text)? {
        let token = &text[a..b];
        if !DIGIT_RUN.is_match(token) {
            continue;
        }
        let pattern = DIGIT_RUN.replace_all(token, "#").to_lowercase();
        if !out.contains(&pattern) {
            out.push(pattern);
        }
    }
    Ok(())
}

/// `invoice_layout_features(text)`.
pub fn features(text: &str) -> Result<InvoiceFeatures, RegexLimit> {
    features_of(&[text])
}

/// [`features`] of the text that `segments` make when joined, one segment at a time: the
/// lowercased, white-space-collapsed text is built as a stream and only its last bytes are
/// kept, enough to find a label that spans two segments. Segments should be cut at white
/// space (as [`quail_text::piece`] cuts): then the result is that of the joined text.
pub(crate) fn features_of(segments: &[&str]) -> Result<InvoiceFeatures, RegexLimit> {
    // The longest needle, ` <label> `, is 16 bytes: keep 15 bytes of the stream before each
    // new segment.
    const KEEP: usize = 15;
    let needles: Vec<String> = LABELS.iter().map(|l| format!(" {l} ")).collect();
    let mut found = [false; LABELS.len()];
    let mut patterns = Vec::new();
    // The collapsed text since the kept tail; it starts with the leading space of the
    // original's ` {text} `.
    let mut stream = String::from(" ");
    // The stream ends with a collapsed run of the text (not the leading space).
    let mut space = false;
    let check = |stream: &str, found: &mut [bool; LABELS.len()]| {
        for (f, n) in found.iter_mut().zip(&needles) {
            *f = *f || stream.contains(n.as_str());
        }
    };
    for seg in segments {
        let lower = seg.to_lowercase();
        let mut last = 0;
        for m in WHITESPACE.find_iter(&lower) {
            stream.push_str(&lower[last..m.start()]);
            if m.start() > last || !space {
                stream.push(' ');
            }
            space = true;
            last = m.end();
        }
        if last < lower.len() {
            stream.push_str(&lower[last..]);
            space = false;
        }
        check(&stream, &mut found);
        let mut cut = stream.len().saturating_sub(KEEP);
        while !stream.is_char_boundary(cut) {
            cut -= 1;
        }
        stream.drain(..cut);
        patterns_into(seg, &mut patterns)?;
    }
    stream.push(' ');
    check(&stream, &mut found);
    let mut labels: Vec<&'static str> = LABELS
        .iter()
        .zip(found)
        .filter_map(|(l, f)| f.then_some(*l))
        .collect();
    labels.sort_unstable();
    Ok(InvoiceFeatures { labels, patterns })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_shapes() {
        let f =
            features("INVOICE NO ABC-US-1024\nBill  To x\nVAT 20% total due INV/2024/001 AB-CD")
                .unwrap();
        assert_eq!(f.labels, ["bill to", "invoice no", "total due", "vat"]);
        assert_eq!(f.patterns, ["abc-us-#", "inv/#/#"]);
        assert_eq!(
            f.lines(),
            [
                "label:bill to",
                "label:invoice no",
                "label:total due",
                "label:vat",
                "num:abc-us-#",
                "num:inv/#/#"
            ]
        );
    }
}

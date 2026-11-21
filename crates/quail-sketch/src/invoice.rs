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
    Ok(out)
}

/// `invoice_layout_features(text)`.
pub fn features(text: &str) -> Result<InvoiceFeatures, RegexLimit> {
    let normalized = format!(" {} ", WHITESPACE.replace_all(&text.to_lowercase(), " "));
    let mut labels: Vec<&'static str> = LABELS
        .iter()
        .copied()
        .filter(|l| normalized.contains(&format!(" {l} ")))
        .collect();
    labels.sort_unstable();
    Ok(InvoiceFeatures {
        labels,
        patterns: number_patterns(text)?,
    })
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

//! A cheap token estimate for budgets, without a tokenizer.
//!
//! The card and the LLM input are budgeted in tokens of a BPE vocabulary (fitted to Qwen3.5's).
//! Real tokenizers weigh megabytes; a budget only needs to be close and deterministic. Each
//! whitespace-separated chunk is split into runs, and:
//!
//! - a run of ASCII letters costs `ceil(len / 6)`;
//! - each ASCII digit costs 1 (Qwen splits numbers into single digits);
//! - a run of non-ASCII letters (diacritics, Cyrillic) costs `ceil(2 · len / 5)`;
//! - every other character (punctuation, symbols) costs 1.
//!
//! So `"Amendment"` is 2, `"2023-09-12"` is 10, `"Уговор"` is 3 and `"d.o.o."` is 6. Measured
//! against the Qwen3.5 tokenizer on 1,400 rendered cards (CUAD and business documents):
//! estimate / real p10 0.93, p50 0.98, p90 1.05; on 760 text heads of 800 estimated tokens:
//! p10 0.79, p50 0.96, p90 1.16.

/// Estimated tokens of `text`.
pub fn estimate(text: &str) -> usize {
    text.split_whitespace().map(chunk).sum()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Ascii,
    Digit,
    Other,
    Symbol,
}

fn class(c: char) -> Run {
    if c.is_ascii_alphabetic() {
        Run::Ascii
    } else if c.is_ascii_digit() {
        Run::Digit
    } else if c.is_alphabetic() {
        Run::Other
    } else {
        Run::Symbol
    }
}

fn run_cost(kind: Run, len: usize) -> usize {
    match kind {
        Run::Ascii => len.div_ceil(6),
        Run::Other => (2 * len).div_ceil(5),
        Run::Digit | Run::Symbol => len,
    }
}

fn chunk(c: &str) -> usize {
    let mut total = 0;
    let mut cur: Option<(Run, usize)> = None;
    for ch in c.chars() {
        let k = class(ch);
        cur = match cur {
            Some((kind, n)) if kind == k && k != Run::Symbol => Some((kind, n + 1)),
            Some((kind, n)) => {
                total += run_cost(kind, n);
                Some((k, 1))
            }
            None => Some((k, 1)),
        };
    }
    if let Some((kind, n)) = cur {
        total += run_cost(kind, n);
    }
    total.max(1)
}

/// The longest prefix of `text` that ends on a chunk boundary and costs at most `max` tokens.
pub fn head(text: &str, max: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    let mut in_chunk = false;
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if in_chunk {
                let cost = chunk(&text[start..i]);
                if used + cost > max {
                    return text[..end].trim_end();
                }
                used += cost;
                end = i;
                in_chunk = false;
            }
        } else if !in_chunk {
            in_chunk = true;
            start = i;
        }
    }
    if in_chunk && used + chunk(&text[start..]) > max {
        return text[..end].trim_end();
    }
    text.trim_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights() {
        assert_eq!(estimate("Amendment"), 2);
        assert_eq!(estimate("2023-09-12"), 10);
        assert_eq!(estimate("Уговор"), 3);
        assert_eq!(estimate("d.o.o."), 6);
        assert_eq!(estimate("a b c"), 3);
        assert_eq!(estimate("  "), 0);
    }

    #[test]
    fn head_respects_the_budget() {
        let t = "one two three four five";
        assert_eq!(head(t, 0), "");
        assert_eq!(head(t, 1), "one");
        assert_eq!(head(t, 3), "one two three");
        assert_eq!(head(t, 100), t);
        assert!(estimate(head(t, 4)) <= 4);
    }
}

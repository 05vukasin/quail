//! A cheap token estimate for budgets, without a tokenizer.
//!
//! The card and the LLM input are budgeted in "tokens" of a typical BPE vocabulary (Qwen,
//! Llama, GPT). Real tokenizers are tens of megabytes; the budget only needs to be safe and
//! deterministic, so the estimate is computed per whitespace-separated chunk:
//!
//! - ASCII letters and digits weigh 1, other ASCII characters (punctuation, symbols) weigh 2,
//!   non-ASCII characters (diacritics, Cyrillic) weigh 2;
//! - a chunk costs `ceil(weight / 4)` tokens, at least 1.
//!
//! So `"Amendment"` is 3, `"2023-09-12"` is 3, `"Уговор"` is 3 and `"d.o.o."` is 3. Against the
//! Qwen3.5 tokenizer on generated cards and English contract text the estimate lands within
//! about ±20 % and errs high on numbers and codes (measured in the PR); Serbian Cyrillic costs
//! about 1.6× English per word in both.

/// Estimated tokens of `text`.
pub fn estimate(text: &str) -> usize {
    text.split_whitespace().map(chunk).sum()
}

fn chunk(c: &str) -> usize {
    let w: usize = c
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { 1 } else { 2 })
        .sum();
    w.div_ceil(4).max(1)
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
        assert_eq!(estimate("Amendment"), 3);
        assert_eq!(estimate("2023-09-12"), 3);
        assert_eq!(estimate("Уговор"), 3);
        assert_eq!(estimate("d.o.o."), 3);
        assert_eq!(estimate("a b c"), 3);
        assert_eq!(estimate("  "), 0);
    }

    #[test]
    fn head_respects_the_budget() {
        let t = "one two three four five";
        assert_eq!(head(t, 0), "");
        assert_eq!(head(t, 1), "one");
        assert_eq!(head(t, 3), "one two");
        assert_eq!(head(t, 4), "one two three");
        assert_eq!(head(t, 100), t);
    }
}

//! Keyword phrases over folded tokens: labels before a value, hints after it.
//!
//! A phrase is a list of folded words (`["maticni", "broj"]`); punctuation between its
//! words is skipped (`Mat. br.`, `PDV-om`, `D-U-N-S`). Folding makes one Latin phrase match
//! Cyrillic (`Матични број`), diacritics and case variants. Every search looks a bounded
//! number of tokens away, so scanning stays linear.

use quail_text::token::TokenKind;

use crate::page::PageText;

/// Most punctuation tokens allowed between two words of a phrase.
const MAX_PUNCT: usize = 2;

/// Most punctuation and symbol tokens a search walks over between a value and its phrase
/// (dot leaders: `PIB: .......... 123`). Searches count words, not punctuation, so without
/// this a line of 300 000 `(` made every value on it walk the whole run: quadratic.
pub(crate) const MAX_SKIPPED: usize = 64;

/// Does `phrase` end at token `j`? Returns the index of its first token.
pub(crate) fn ends_at(p: &PageText, j: usize, phrase: &[&str]) -> Option<usize> {
    ends_at_within(p, j, phrase, 0)
}

/// [`ends_at`] whose words may sit up to `lines` lines above token `j` (a phrase wrapped
/// over a line break of running text: `dated as` / `of 1 May 2024`).
pub(crate) fn ends_at_within(p: &PageText, j: usize, phrase: &[&str], lines: u32) -> Option<usize> {
    let mut j = j;
    let line = p.line(j);
    for (n, w) in phrase.iter().rev().enumerate() {
        if n > 0 {
            // Step back to the previous word, over at most MAX_PUNCT punctuation tokens.
            let mut steps = 0;
            loop {
                j = j.checked_sub(1)?;
                if line - p.line(j).min(line) > lines {
                    return None;
                }
                if p.wordlike(j) {
                    break;
                }
                steps += 1;
                if steps > MAX_PUNCT {
                    return None;
                }
            }
        }
        if p.key(j) != *w {
            return None;
        }
    }
    Some(j)
}

/// Does `phrase` start at token `i`? Returns the index of its last token.
pub(crate) fn starts_at(p: &PageText, i: usize, phrase: &[&str]) -> Option<usize> {
    let mut i = i;
    let line = p.line(i);
    for (n, w) in phrase.iter().enumerate() {
        if n > 0 {
            let mut steps = 0;
            loop {
                i += 1;
                if i >= p.len() || p.line(i) != line {
                    return None;
                }
                if p.wordlike(i) {
                    break;
                }
                steps += 1;
                if steps > MAX_PUNCT {
                    return None;
                }
            }
        }
        if p.key(i) != *w {
            return None;
        }
    }
    Some(i)
}

/// How far a search may look.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reach {
    /// Words (word-like tokens) that may sit between the phrase and the value.
    pub words: usize,
    /// Lines the phrase may be away (0: same line; 1: also the previous / next line, for
    /// table cells and labels above their value).
    pub lines: u32,
}

/// Is token `j` another value (a number of four or more digits), which ends a search?
fn is_other_value(p: &PageText, j: usize) -> bool {
    matches!(p.kind(j), Some(TokenKind::Number | TokenKind::Alnum))
        && p.text(j).bytes().filter(u8::is_ascii_digit).count() >= 4
}

/// The nearest phrase of `table` that ends before token `i`, within `reach`.
pub(crate) fn find_before<C: Copy>(
    p: &PageText,
    i: usize,
    table: &[(&[&str], C)],
    reach: Reach,
) -> Option<(C, usize)> {
    let line = p.line(i);
    let mut words = 0;
    let mut skipped = 0;
    let mut j = i;
    while j > 0 {
        j -= 1;
        let up = line - p.line(j).min(line);
        // A label on an earlier line counts only for a value that starts its line (a
        // label cell or a label above the value), never across other words.
        if up > reach.lines || (up > 0 && words > 0) {
            return None;
        }
        if !p.wordlike(j) {
            skipped += 1;
            if skipped > MAX_SKIPPED {
                return None;
            }
            continue;
        }
        for (phrase, c) in table {
            if let Some(first) = ends_at(p, j, phrase) {
                return Some((*c, first));
            }
        }
        if is_other_value(p, j) {
            return None;
        }
        words += 1;
        if words > reach.words {
            return None;
        }
    }
    None
}

/// Like [`find_before`], but for running text whose lines wrap: words between the phrase
/// and the value, and the phrase itself, may span up to `reach.lines` line breaks.
pub(crate) fn find_before_wrapped<C: Copy>(
    p: &PageText,
    i: usize,
    table: &[(&[&str], C)],
    reach: Reach,
) -> Option<(C, usize)> {
    let line = p.line(i);
    let mut words = 0;
    let mut skipped = 0;
    let mut j = i;
    while j > 0 {
        j -= 1;
        if line - p.line(j).min(line) > reach.lines {
            return None;
        }
        if !p.wordlike(j) {
            skipped += 1;
            if skipped > MAX_SKIPPED {
                return None;
            }
            continue;
        }
        for (phrase, c) in table {
            if let Some(first) = ends_at_within(p, j, phrase, reach.lines) {
                return Some((*c, first));
            }
        }
        if is_other_value(p, j) {
            return None;
        }
        words += 1;
        if words > reach.words {
            return None;
        }
    }
    None
}

/// The nearest phrase of `table` that starts after token `i`, within `reach`.
pub(crate) fn find_after<C: Copy>(
    p: &PageText,
    i: usize,
    table: &[(&[&str], C)],
    reach: Reach,
) -> Option<(C, usize)> {
    let line = p.line(i);
    let mut words = 0;
    let mut skipped = 0;
    let mut j = i + 1;
    while j < p.len() {
        if p.line(j) - line > reach.lines {
            return None;
        }
        if p.wordlike(j) {
            for (phrase, c) in table {
                if let Some(last) = starts_at(p, j, phrase) {
                    return Some((*c, last));
                }
            }
            if is_other_value(p, j) {
                return None;
            }
            words += 1;
            if words > reach.words {
                return None;
            }
        } else {
            skipped += 1;
            if skipped > MAX_SKIPPED {
                return None;
            }
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_cross_scripts_and_punctuation() {
        let mut p = PageText::default();
        p.load("Матични број: 1 i Mat. br. 2 i D-U-N-S 3 i PDV-om");
        let table: &[(&[&str], u8)] = &[
            (&["maticni", "broj"], 1),
            (&["mat", "br"], 2),
            (&["d", "u", "n", "s"], 3),
        ];
        let at = |text: &str| (0..p.len()).find(|&i| p.text(i) == text).unwrap();
        let r = Reach { words: 2, lines: 0 };
        assert_eq!(find_before(&p, at("1"), table, r).map(|x| x.0), Some(1));
        assert_eq!(find_before(&p, at("2"), table, r).map(|x| x.0), Some(2));
        assert_eq!(find_before(&p, at("3"), table, r).map(|x| x.0), Some(3));
        assert!(starts_at(&p, at("PDV"), &["pdv", "om"]).is_some());
    }

    #[test]
    fn search_stops_at_lines_and_other_values() {
        let mut p = PageText::default();
        p.load("PIB\n\nx 5\nPIB 12345 7");
        let table: &[(&[&str], u8)] = &[(&["pib"], 1)];
        let r = Reach { words: 3, lines: 0 };
        let five = (0..p.len()).find(|&i| p.text(i) == "5").unwrap();
        assert!(find_before(&p, five, table, r).is_none());
        let seven = (0..p.len()).find(|&i| p.text(i) == "7").unwrap();
        assert!(
            find_before(&p, seven, table, r).is_none(),
            "12345 is in between"
        );
    }
}

//! Bounded pieces of a long text, so an analyzer can stream a page of many megabytes instead
//! of building per-token structures for all of it at once.
//!
//! [`pieces`] cuts a text into consecutive parts of at most about `max` bytes. A cut goes
//! after a blank line when the second half of the window has one, else after a line break,
//! else after white space, else on a char boundary; it then moves past the white space that
//! follows (at most `max` more bytes), so a piece never starts with white space unless the
//! text does. A text of at most `max` bytes is one piece: analyzers that read pieces give
//! the same result as before on every page up to [`PIECE_BYTES`].

/// Piece size the text analyzers use for long pages: no real page comes close (a 2000-page
/// book is about 4 MB of text), and per-token structures for this much text stay in the tens
/// of megabytes.
pub const PIECE_BYTES: usize = 4 << 20;

/// The longest prefix of `text` within `max` bytes, on a char boundary.
pub fn prefix(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Iterator over `(offset in text, piece)`; see the [module docs](self).
#[derive(Clone, Debug)]
pub struct Pieces<'a> {
    text: &'a str,
    pos: usize,
    max: usize,
}

/// The pieces of `text` of at most about `max` bytes (`max` ≥ 1), in order; together they
/// are exactly `text`. An empty text has no piece.
pub fn pieces(text: &str, max: usize) -> Pieces<'_> {
    Pieces {
        text,
        pos: 0,
        max: max.max(4),
    }
}

impl<'a> Iterator for Pieces<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.text[self.pos..];
        if rest.is_empty() {
            return None;
        }
        let start = self.pos;
        let len = cut(rest, self.max);
        self.pos += len;
        Some((start, &rest[..len]))
    }
}

/// Length of the next piece of `rest`.
fn cut(rest: &str, max: usize) -> usize {
    if rest.len() <= max {
        return rest.len();
    }
    let window = prefix(rest, max);
    let mut half = window.len() / 2;
    while !window.is_char_boundary(half) {
        half -= 1;
    }
    let tail = &window.as_bytes()[half..];
    let find = |needle: &[u8]| {
        tail.windows(needle.len())
            .rposition(|w| w == needle)
            .map(|i| half + i + needle.len())
    };
    let mut end = find(b"\n\n")
        .or_else(|| find(b"\n"))
        .or_else(|| {
            window[half..]
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map(|(i, c)| half + i + c.len_utf8())
        })
        .unwrap_or(window.len());
    if end == 0 {
        // A first char longer than `max` (only for tiny `max`).
        end = rest.chars().next().map_or(rest.len(), char::len_utf8);
    }
    // Past the white space that follows, within `max` more bytes.
    let limit = end + max;
    for (i, c) in rest[end..].char_indices() {
        if !c.is_whitespace() || end + i + c.len_utf8() > limit {
            return end + i;
        }
    }
    rest.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &str, max: usize) -> Vec<(usize, &str)> {
        pieces(text, max).collect()
    }

    #[test]
    fn short_text_is_one_piece() {
        assert_eq!(all("abc def", 7), [(0, "abc def")]);
        assert!(all("", 7).is_empty());
    }

    #[test]
    fn cuts_prefer_blank_lines_then_lines_then_spaces() {
        assert_eq!(
            all("aaaa bb\n\ncc dd ee", 14),
            [(0, "aaaa bb\n\n"), (9, "cc dd ee")]
        );
        assert_eq!(
            all("aaaa bb\ncc dd ee", 12),
            [(0, "aaaa bb\n"), (8, "cc dd ee")]
        );
        assert_eq!(
            all("aaaa bbbb cccc", 8),
            [(0, "aaaa "), (5, "bbbb "), (10, "cccc")]
        );
        // No white space: on a char boundary.
        assert_eq!(all("ččččč", 5), [(0, "čč"), (4, "čč"), (8, "č")]);
    }

    #[test]
    fn white_space_after_a_cut_stays_with_the_piece() {
        assert_eq!(
            all("aaaa\n\n\n\n  bbbb", 8),
            [(0, "aaaa\n\n\n\n  "), (10, "bbbb")]
        );
    }

    #[test]
    fn pieces_cover_the_text_within_bounds() {
        let text: String = (0..5000)
            .map(|i| match i % 11 {
                0 => "\n".to_owned(),
                3 => "\n\n".to_owned(),
                _ => format!("rečč{i} "),
            })
            .collect();
        for max in [4, 5, 17, 64, 1000] {
            let mut joined = String::new();
            let mut next = 0;
            for (at, p) in pieces(&text, max) {
                assert_eq!(at, next);
                assert!(p.len() <= 2 * max, "{max}: {}", p.len());
                assert!(!p.is_empty());
                joined.push_str(p);
                next = at + p.len();
            }
            assert_eq!(joined, text);
        }
    }
}

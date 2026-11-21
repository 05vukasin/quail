//! A flat token stream over the pages of a document: words (letters and digits) and single
//! punctuation characters, each with its page, its byte span in that page's text and a
//! lowercase key. Clauses wrap over line and page breaks, so the rules read one stream.

use quail_core::{Document, Span};

/// One token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tok {
    /// Page index.
    pub page: u32,
    /// Byte span in the page text.
    pub span: Span,
    /// Lowercase text; typographic quotes and apostrophes folded to ASCII.
    pub key: String,
    /// A run of letters and digits (otherwise one punctuation character).
    pub word: bool,
    /// The first character is an uppercase letter.
    pub upper: bool,
}

/// The tokens of a document and what reading them cost.
#[derive(Debug, Default)]
pub struct Stream {
    /// Tokens in reading order.
    pub toks: Vec<Tok>,
    /// Pages whose text was cut by the byte budget (index of the first one).
    pub cut_at: Option<u32>,
}

impl Stream {
    /// Tokenizes `doc` within `budget` bytes of text, skipping pages that need OCR.
    /// `stop` is asked before every page; when it says so, reading ends there.
    pub fn read(doc: &Document, budget: usize, mut stop: impl FnMut() -> bool) -> (Self, bool) {
        let mut s = Self::default();
        let mut left = budget;
        for page in &doc.pages {
            if stop() {
                return (s, true);
            }
            if page.needs_ocr {
                continue;
            }
            if left == 0 {
                s.cut_at.get_or_insert(page.index);
                break;
            }
            let mut end = left.min(page.text.len());
            while !page.text.is_char_boundary(end) {
                end -= 1;
            }
            if end < page.text.len() {
                s.cut_at.get_or_insert(page.index);
            }
            tokenize(&page.text[..end], page.index, &mut s.toks);
            left -= end;
        }
        (s, false)
    }

    /// Number of tokens.
    pub fn len(&self) -> usize {
        self.toks.len()
    }

    /// No tokens.
    pub fn is_empty(&self) -> bool {
        self.toks.is_empty()
    }

    /// Key of token `i` (`""` past the end).
    pub fn key(&self, i: usize) -> &str {
        self.toks.get(i).map_or("", |t| t.key.as_str())
    }

    /// Token `i` is a word.
    pub fn is_word(&self, i: usize) -> bool {
        self.toks.get(i).is_some_and(|t| t.word)
    }

    /// Token `i` is the punctuation `c`.
    pub fn is_punct(&self, i: usize, c: char) -> bool {
        self.toks
            .get(i)
            .is_some_and(|t| !t.word && t.key.starts_with(c))
    }

    /// The word positions `i..` spell `seq` (punctuation tokens in between are not allowed).
    pub fn seq_at(&self, i: usize, seq: &[&str]) -> bool {
        seq.iter().enumerate().all(|(k, w)| self.key(i + k) == *w)
    }

    /// Ends a sentence: `.`, `;`, `!`, `?` not between two digits.
    pub fn sentence_end(&self, i: usize) -> bool {
        let Some(t) = self.toks.get(i) else {
            return true;
        };
        if t.word {
            return false;
        }
        match t.key.as_str() {
            ";" | "!" | "?" => true,
            "." => {
                // "2.5", "Inc." and "U.S." do not end a sentence here: only a dot before an
                // uppercase word, or at the end of the text, does.
                let next = self.toks.get(i + 1);
                let prev_digit = i > 0 && self.key(i - 1).starts_with(|c: char| c.is_ascii_digit());
                let next_digit =
                    next.is_some_and(|n| n.key.starts_with(|c: char| c.is_ascii_digit()));
                if prev_digit && next_digit {
                    return false;
                }
                // An initial or an abbreviation before the dot: "U.S.", "Inc.", "No.".
                let abbrev = i > 0
                    && self.is_word(i - 1)
                    && (self.key(i - 1).chars().count() == 1
                        || ABBREVIATIONS.contains(&self.key(i - 1)));
                next.is_none_or(|n| n.upper && !abbrev)
            }
            _ => false,
        }
    }

    /// The text covered by tokens `a..b` when they lie on one page, else the text of the first
    /// page's part, rebuilt from the page text.
    pub fn slice<'d>(&self, doc: &'d Document, a: usize, b: usize) -> &'d str {
        let (Some(first), Some(last)) = (self.toks.get(a), self.toks.get(b.saturating_sub(1)))
        else {
            return "";
        };
        let Some(page) = doc.pages.iter().find(|p| p.index == first.page) else {
            return "";
        };
        let end = if last.page == first.page {
            last.span.end
        } else {
            u32::try_from(page.text.len()).unwrap_or(u32::MAX)
        };
        Span {
            start: first.span.start,
            end,
        }
        .slice(&page.text)
    }

    /// The span of tokens `a..b` on the first token's page (cut at the page end).
    pub fn span(&self, a: usize, b: usize) -> Option<(u32, Span)> {
        let first = self.toks.get(a)?;
        let mut end = first.span.end;
        for t in self.toks.get(a..b)? {
            if t.page != first.page {
                break;
            }
            end = t.span.end;
        }
        Some((
            first.page,
            Span {
                start: first.span.start,
                end,
            },
        ))
    }
}

/// Words a dot follows without ending a sentence.
const ABBREVIATIONS: &[&str] = &[
    "inc", "co", "corp", "ltd", "no", "nos", "mr", "mrs", "ms", "dr", "jr", "sr", "st", "vs",
    "etc", "seq", "art", "sec", "par", "para",
];

fn fold_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' | '`' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201F}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        _ => c,
    }
}

fn tokenize(text: &str, page: u32, out: &mut Vec<Tok>) {
    let mut word_start: Option<usize> = None;
    let push_word = |out: &mut Vec<Tok>, s: usize, e: usize| {
        let w = &text[s..e];
        out.push(Tok {
            page,
            span: span(s, e),
            key: w.to_lowercase(),
            word: true,
            upper: w.starts_with(char::is_uppercase),
        });
    };
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            word_start.get_or_insert(i);
            continue;
        }
        if let Some(s) = word_start.take() {
            push_word(out, s, i);
        }
        if c.is_whitespace() || c.is_control() {
            continue;
        }
        out.push(Tok {
            page,
            span: span(i, i + c.len_utf8()),
            key: fold_char(c).to_string(),
            word: false,
            upper: false,
        });
    }
    if let Some(s) = word_start {
        push_word(out, s, text.len());
    }
}

fn span(s: usize, e: usize) -> Span {
    Span {
        start: u32::try_from(s).unwrap_or(u32::MAX),
        end: u32::try_from(e).unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Page, PageKind};

    fn doc(texts: &[&str]) -> Document {
        Document {
            pages: texts
                .iter()
                .enumerate()
                .map(|(i, t)| Page {
                    text: (*t).to_string(),
                    ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
                })
                .collect(),
        }
    }

    #[test]
    fn words_punctuation_and_spans() {
        let d = doc(&["Two (2) years\u{2019} notice.", "Next"]);
        let (s, stopped) = Stream::read(&d, 1 << 20, || false);
        assert!(!stopped);
        let keys: Vec<&str> = s.toks.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(
            keys,
            ["two", "(", "2", ")", "years", "'", "notice", ".", "next"]
        );
        assert_eq!(s.toks[8].page, 1);
        assert_eq!(s.slice(&d, 0, 5), "Two (2) years");
        assert!(s.sentence_end(7));
    }

    #[test]
    fn budget_cuts_on_a_char_boundary() {
        let d = doc(&["ééé abc"]);
        let (s, _) = Stream::read(&d, 3, || false);
        assert_eq!(s.cut_at, Some(0));
        assert_eq!(s.key(0), "é");
    }

    #[test]
    fn dots_inside_numbers_and_abbreviations_do_not_end_sentences() {
        let d = doc(&["2.5 years under U.S. Law. Then"]);
        let (s, _) = Stream::read(&d, 1 << 20, || false);
        assert!(!s.sentence_end(1));
        let last_dot = s.toks.iter().rposition(|t| t.key == ".").unwrap();
        assert!(s.sentence_end(last_dot));
    }
}

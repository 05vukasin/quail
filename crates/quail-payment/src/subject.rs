//! What is paid for, as the document states it: the text after a subject label
//! (`Predmet ugovora je ...`, `Предмет уговора: ...`, `Subject of the Agreement: ...`,
//! `Scope of Services`), or the line after such a label when it is a heading.

use quail_core::Document;
use quail_text::token::TokenKind;

use crate::cues;
use crate::lex::{Line, Tok, longest_at, word_matches};

/// Longest subject text, in characters.
pub const MAX_SUBJECT_CHARS: usize = 300;
/// Most extra lines a subject sentence may wrap over.
const MAX_WRAP: usize = 3;

/// How a subject was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// After the label on its line (`Predmet ugovora je ...`).
    Clause,
    /// The line after a label that stands alone (a heading `Predmet ugovora`).
    AfterHeading,
    /// The descriptions of the invoice or offer lines.
    Lines,
    /// The document title.
    Title,
}

impl Source {
    /// The value written into the `source` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clause => "clause",
            Self::AfterHeading => "after_heading",
            Self::Lines => "line_items",
            Self::Title => "title",
        }
    }
}

/// A subject candidate: page text from `start` to `end` on line `line`.
#[derive(Clone, Debug)]
pub struct Subject {
    /// The text, whitespace collapsed, at most [`MAX_SUBJECT_CHARS`].
    pub text: String,
    /// How it was found.
    pub source: Source,
    /// Confidence.
    pub conf: f32,
    /// The line it starts on (index in the lines read).
    pub line: usize,
    /// Byte range on the page: the label and the text.
    pub start: u32,
    /// End of that range.
    pub end: u32,
    /// Folded keys of its words, for the category.
    pub keys: Vec<String>,
}

fn collapse(s: &str) -> String {
    let mut out = String::new();
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.chars().count() > MAX_SUBJECT_CHARS {
        out = out.chars().take(MAX_SUBJECT_CHARS).collect();
    }
    out
}

/// Whether token `i` ends a sentence: `.`, `;` or `!` (a number keeps its own dots).
fn ends_sentence(t: &Tok) -> bool {
    t.kind == TokenKind::Punct && matches!(t.key.as_str(), "." | ";" | "!" | "?")
}

/// Reads subject text from token `from` of line `li`, wrapping over the next lines while the
/// sentence has not ended and the next line goes on in lowercase. Returns the end line, the
/// end byte and the folded word keys.
fn read_text(doc: &Document, lines: &[Line], li: usize, from: usize) -> Option<(usize, u32, Vec<String>)> {
    let mut keys = Vec::new();
    let mut end = None;
    let mut l = li;
    let mut k = from;
    let mut chars = 0usize;
    loop {
        let line = &lines[l];
        let mut ended = false;
        while k < line.toks.len() {
            let t = &line.toks[k];
            if ends_sentence(t) && !keys.is_empty() {
                ended = true;
                break;
            }
            if t.wordlike() {
                keys.push(t.key.clone());
            }
            end = Some((l, t.end));
            chars += t.key.len() + 1;
            k += 1;
            if chars > MAX_SUBJECT_CHARS * 2 {
                ended = true;
                break;
            }
        }
        if ended || l + 1 >= lines.len() || l + 1 > li + MAX_WRAP {
            break;
        }
        let next = &lines[l + 1];
        let lower = next
            .text(doc)
            .trim_start()
            .chars()
            .next()
            .is_some_and(char::is_lowercase);
        if next.pos != line.pos || !lower {
            break;
        }
        l += 1;
        k = 0;
    }
    let (l, e) = end?;
    Some((l, e, keys))
}

/// Whether `keys` say something: at least two words and not only numbers or glue.
fn meaningful(keys: &[String]) -> bool {
    let words = keys
        .iter()
        .filter(|k| k.chars().any(char::is_alphabetic) && !cues::SUBJECT_GLUE.contains(&k.as_str()))
        .count();
    words >= 2
}

/// Subject candidates of line `li`.
pub fn scan(doc: &Document, lines: &[Line], li: usize, out: &mut Vec<Subject>) {
    let line = &lines[li];
    let toks = &line.toks;
    let first = toks.iter().position(|t| t.kind == TokenKind::Word);
    let mut i = 0;
    while i < toks.len() {
        let Some((&(needs_colon, opens), end)) = longest_at(toks, i, cues::SUBJECT) else {
            i += 1;
            continue;
        };
        // Single-word labels (`Predmet:`, `Services:`) must open the line, after numbering.
        if opens && first.is_none_or(|f| f != i || i > 3) {
            i = end;
            continue;
        }
        let mut k = end;
        let colon = toks
            .get(k)
            .is_some_and(|t| t.kind == TokenKind::Punct && matches!(t.key.as_str(), ":" | "-"));
        while toks
            .get(k)
            .is_some_and(|t| t.kind == TokenKind::Punct && matches!(t.key.as_str(), ":" | "-" | ","))
        {
            k += 1;
        }
        let mut verb = false;
        while toks
            .get(k)
            .is_some_and(|t| cues::SUBJECT_GLUE.iter().any(|g| word_matches(g, &t.key)))
        {
            verb = true;
            k += 1;
        }
        while toks
            .get(k)
            .is_some_and(|t| t.kind == TokenKind::Punct && t.key == ":")
        {
            k += 1;
        }
        if needs_colon && !colon {
            i = end;
            continue;
        }
        if k < toks.len() {
            if !(colon || verb) {
                // `Predmet ugovora o ...` is a title, not a subject clause.
                i = end;
                continue;
            }
            if let Some((_, e, keys)) = read_text(doc, lines, li, k)
                && meaningful(&keys)
            {
                let text = collapse(line.slice(doc, toks[k].start, e));
                let conf = if needs_colon { 0.6 } else if verb { 0.85 } else { 0.8 };
                out.push(Subject {
                    text,
                    source: Source::Clause,
                    conf,
                    line: li,
                    start: toks[i].start,
                    end: e,
                    keys,
                });
            }
        } else if let Some(next) = lines.get(li + 1)
            && next.pos == line.pos
            && let Some((_, e, keys)) = read_text(doc, lines, li + 1, 0)
            && meaningful(&keys)
        {
            // A heading: the subject is the paragraph under it.
            let text = collapse(next.slice(doc, next.toks[0].start, e));
            out.push(Subject {
                text,
                source: Source::AfterHeading,
                conf: if needs_colon { 0.5 } else { 0.6 },
                line: li + 1,
                start: next.toks[0].start,
                end: e,
                keys,
            });
        }
        return;
    }
}

/// A title that names what is paid for (`UGOVOR O ODRŽAVANJU SOFTVERA`), from the first lines
/// of the first page: it opens with a contract or invoice word and has a word that is not
/// generic.
pub fn title(doc: &Document, lines: &[Line]) -> Option<Subject> {
    let first_pos = lines.first()?.pos;
    for (li, line) in lines.iter().enumerate().take(6) {
        if line.pos != first_pos {
            break;
        }
        let words: Vec<&Tok> = line.toks.iter().filter(|t| t.wordlike()).collect();
        let Some(w0) = words.first() else { continue };
        let opens = ["ugovor", "agreement", "contract", "ponuda", "offer"]
            .contains(&w0.key.as_str())
            || words.get(1).is_some_and(|w| {
                ["agreement", "contract"].contains(&w.key.as_str())
            });
        if !opens || words.len() < 3 || words.len() > 16 {
            continue;
        }
        let specific = words
            .iter()
            .any(|w| w.kind == TokenKind::Word && !cues::GENERIC_TITLE.contains(&w.key.as_str()));
        if !specific {
            continue;
        }
        let text = collapse(line.text(doc));
        return Some(Subject {
            text,
            source: Source::Title,
            conf: 0.45,
            line: li,
            start: line.span.start,
            end: line.span.end,
            keys: words.iter().map(|w| w.key.clone()).collect(),
        });
    }
    None
}

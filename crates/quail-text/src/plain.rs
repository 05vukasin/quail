//! `text.extract@1`: plain-text files as a [`Document`].
//!
//! - **Encoding**: see [`crate::decode`] (BOM, UTF-8, UTF-16, then a `chardetng` guess).
//! - **Line endings**: CRLF and lone CR become LF before anything else, so the stored page
//!   text has only `\n` and every span indexes that stored text.
//! - **Pages**: one [`PageKind::Logical`] page per form feed (`\f`) separated part, or one
//!   page when there is none. A form feed at the very end does not open an empty last page.
//! - **Blocks**: paragraphs separated by blank lines (lines of only whitespace), each span
//!   trimmed of surrounding whitespace. A page without any blank line but with several lines
//!   (one paragraph per line) gets one block per line. All blocks are in [`Zone::Body`]:
//!   running headers and footers are not inferred.
//! - **Headings**: a block of a single line of 3 to 80 characters, with at least two letters,
//!   not ending in `.`, `,` or `;`, is a heading when its letters are all upper case
//!   (level 1, `GENERAL TERMS`) or when it starts with an upper-case letter or a digit, has
//!   at most 8 words and every word of 4 or more letters is capitalized (level 2,
//!   `3. Payment Terms`). Everything else is a [`BlockKind::Paragraph`].
//!
//! Limits: reads at most `max_bytes` (and never more than twice `max_text_bytes`, since a
//! decoded byte is at least half a source byte), keeps at most `max_text_bytes` of text (cut
//! at a char boundary), at most `max_pages` pages; each records a problem. Only the kept
//! text is decoded, and it is held about once (twice while a text with form feeds is split
//! into pages), so peak memory beyond the input stays near `max_text_bytes`. Checks the
//! deadline per page and every 4096 blocks. All pages are `TextSource::Native`, never
//! `needs_ocr`.

use quail_core::{
    AnalyzerId, Blob, Block, BlockKind, Document, Emitter, Extractor, Format, Limits, Page,
    PageKind, Result, Span, TextSource, Value, Zone,
};

use crate::decode::decode_prefix;
use crate::keys;

/// Longest heading, in characters.
const MAX_HEADING_CHARS: usize = 80;
/// Most words of a title-case (level 2) heading.
const MAX_HEADING_WORDS: usize = 8;
/// Below this confidence of the encoding guess a warning says the text may be mis-decoded.
const LOW_CONFIDENCE: f32 = 0.6;

/// Plain-text extractor for [`Format::Text`].
///
/// Emits [`keys::ENCODING`] (with the confidence as `conf`), [`keys::ENCODING_CONFIDENCE`],
/// [`keys::ENCODING_BOM`], [`keys::LINE_ENDINGS`] and, when bytes could not be decoded,
/// [`keys::DECODE_ERRORS`] plus a warning. See the [module docs](self) for pages and blocks.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextExtractor;

impl TextExtractor {
    /// Extractor id.
    pub const ID: AnalyzerId = AnalyzerId::new("text.extract", 1);

    /// New extractor.
    pub fn new() -> Self {
        Self
    }
}

/// Which line endings a text used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Endings {
    lf: usize,
    crlf: usize,
    cr: usize,
}

impl Endings {
    fn label(self) -> &'static str {
        match (self.lf > 0, self.crlf > 0, self.cr > 0) {
            (false, false, false) => "none",
            (true, false, false) => "lf",
            (false, true, false) => "crlf",
            (false, false, true) => "cr",
            _ => "mixed",
        }
    }
}

/// `text` with CRLF and lone CR as LF, and the endings it had. In place: the text only
/// shrinks, so no second copy is made.
fn normalize_endings(text: String) -> (String, Endings) {
    let mut e = Endings::default();
    // One pass counting both (clippy's `naive_bytecount` suggests a crate for one byte).
    let (crs, lfs) = text.bytes().fold((0usize, 0usize), |(r, n), c| {
        (r + usize::from(c == b'\r'), n + usize::from(c == b'\n'))
    });
    if crs == 0 {
        e.lf = lfs;
        return (text, e);
    }
    let mut bytes = text.into_bytes();
    let mut write = 0usize;
    let mut read = 0usize;
    while read < bytes.len() {
        let byte = bytes[read];
        if byte == b'\r' {
            bytes[write] = b'\n';
            if bytes.get(read + 1) == Some(&b'\n') {
                e.crlf += 1;
                read += 1;
            } else {
                e.cr += 1;
            }
        } else {
            bytes[write] = byte;
        }
        write += 1;
        read += 1;
    }
    bytes.truncate(write);
    e.lf = lfs - e.crlf;
    // Only ASCII bytes were replaced or dropped: still UTF-8.
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned());
    (text, e)
}

fn span(start: usize, end: usize) -> Span {
    // Page text is at most `max_text_bytes`; clamp rather than wrap past 4 GiB.
    let c = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    Span {
        start: c(start),
        end: c(end),
    }
}

/// Heading level of a one-line block, see the module docs.
fn heading_level(line: &str) -> Option<u8> {
    if line.len() > MAX_HEADING_CHARS * 4 || line.ends_with(['.', ',', ';']) {
        return None;
    }
    let chars = line.chars().count();
    if !(3..=MAX_HEADING_CHARS).contains(&chars) {
        return None;
    }
    let letters = line.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 {
        return None;
    }
    if line
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(char::is_uppercase)
    {
        return Some(1);
    }
    let first = line.chars().next()?;
    if !(first.is_uppercase() || first.is_ascii_digit()) {
        return None;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() > MAX_HEADING_WORDS || line.ends_with(':') {
        return None;
    }
    let capitalized = words.iter().all(|w| {
        let letters = w.chars().filter(|c| c.is_alphabetic()).count();
        letters < 4
            || w.chars()
                .find(|c| c.is_alphabetic())
                .is_some_and(char::is_uppercase)
    });
    capitalized.then_some(2)
}

/// One block per run of non-blank lines (or per line, see the module docs), trimmed.
fn blocks(text: &str, out: &Emitter) -> Result<Vec<Block>> {
    // Lines as (start, end) of their trimmed content; `None` for a blank line.
    let mut groups: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, lines)
    let mut open: Option<(usize, usize, usize)> = None;
    let mut blank_seen = false;
    let mut line_spans: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let body = line.trim_end_matches('\n');
        let lead = body.len() - body.trim_start().len();
        let content = body.trim();
        if content.is_empty() {
            blank_seen = true;
            if let Some(g) = open.take() {
                groups.push(g);
            }
            continue;
        }
        let (s, e) = (start + lead, start + lead + content.len());
        line_spans.push((s, e));
        open = Some(match open {
            Some((gs, _, n)) => (gs, e, n + 1),
            None => (s, e, 1),
        });
    }
    if let Some(g) = open {
        groups.push(g);
    }
    if !blank_seen && line_spans.len() > 1 {
        groups = line_spans.iter().map(|&(s, e)| (s, e, 1)).collect();
    }
    let mut v = Vec::with_capacity(groups.len());
    for (i, (s, e, lines)) in groups.into_iter().enumerate() {
        if i % 4096 == 4095 {
            out.check_deadline()?;
        }
        let kind = match (lines, text.get(s..e).and_then(heading_level)) {
            (1, Some(level)) => BlockKind::Heading { level },
            _ => BlockKind::Paragraph,
        };
        v.push(Block::new(span(s, e), kind, Zone::Body, None));
    }
    Ok(v)
}

impl Extractor for TextExtractor {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Text]
    }

    #[allow(clippy::cast_possible_truncation)]
    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        let all = blob.as_bytes();
        let max_text = usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX);
        let max_read = usize::try_from(limits.max_bytes)
            .unwrap_or(usize::MAX)
            .min(max_text.saturating_mul(2).saturating_add(4));
        let bytes = &all[..all.len().min(max_read)];
        let input_cut = bytes.len() < all.len();

        let (decoded, complete) = decode_prefix(bytes, max_text);
        out.push(
            keys::ENCODING,
            Value::text(decoded.name()),
            decoded.confidence,
            None,
        );
        out.fact(
            keys::ENCODING_CONFIDENCE,
            Value::Float(f64::from(decoded.confidence)),
        );
        out.fact(keys::ENCODING_BOM, Value::Bool(decoded.bom));
        if decoded.lossy() {
            out.fact(
                keys::DECODE_ERRORS,
                Value::Int(i64::try_from(decoded.replaced).unwrap_or(i64::MAX)),
            );
            out.warn(format!(
                "{} byte sequence(s) not valid {}, replaced by U+FFFD",
                decoded.replaced,
                decoded.name()
            ));
        }
        if decoded.confidence < LOW_CONFIDENCE {
            out.warn(format!(
                "encoding guessed as {} with low confidence {:.2}",
                decoded.name(),
                decoded.confidence
            ));
        }

        let kept_len = decoded.text.len();
        if input_cut || !complete {
            out.problem(format!(
                "text cut at {kept_len} bytes (max_bytes / max_text_bytes)"
            ));
        }
        let (mut text, endings) = normalize_endings(decoded.text.into_owned());
        out.fact(keys::LINE_ENDINGS, Value::text(endings.label()));

        if text.ends_with('\u{c}') {
            text.pop();
        }
        let page = |i: usize, text: String, blocks: Vec<Block>| Page {
            text,
            blocks,
            text_source: TextSource::Native,
            needs_ocr: false,
            ..Page::new(i as u32, PageKind::Logical)
        };
        if !text.contains('\u{c}') {
            // One page: the text moves into it, no copy.
            out.check_deadline()?;
            let blocks = blocks(&text, out)?;
            return Ok(Document {
                pages: vec![page(0, text, blocks)],
            });
        }
        let max_pages = limits.max_pages as usize;
        let mut pages = Vec::new();
        for (i, chunk) in text.split('\u{c}').enumerate() {
            out.check_deadline()?;
            if i >= max_pages {
                out.problem(format!("pages over max_pages ({max_pages}), rest skipped"));
                break;
            }
            let blocks = blocks(chunk, out)?;
            pages.push(page(i, chunk.to_owned(), blocks));
        }
        Ok(Document { pages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(bytes: &[u8], limits: &Limits) -> (Document, Emitter) {
        let mut out = Emitter::new(TextExtractor::ID);
        let doc = TextExtractor
            .extract(&Blob::from(bytes.to_vec()), limits, &mut out)
            .unwrap();
        (doc, out)
    }

    fn texts(page: &Page) -> Vec<&str> {
        page.blocks
            .iter()
            .map(|b| b.span.slice(&page.text))
            .collect()
    }

    #[test]
    fn endings_are_normalized_and_labelled() {
        let n = |s: &str| normalize_endings(s.to_owned());
        let (t, e) = n("a\r\nb\rc\nd");
        assert_eq!(t, "a\nb\nc\nd");
        assert_eq!(e.label(), "mixed");
        assert_eq!(n("a\r\nb\r\n").1.label(), "crlf");
        assert_eq!(n("a\rb").1.label(), "cr");
        assert_eq!(n("ab").1.label(), "none");
        assert_eq!(n("a\nb").1.label(), "lf");
        assert_eq!(n("č\r\nž\r").0, "č\nž\n");
    }

    #[test]
    fn headings_follow_the_documented_rule() {
        assert_eq!(heading_level("GENERAL TERMS"), Some(1));
        assert_eq!(heading_level("ČLAN 5 – PLAĆANJE"), Some(1));
        assert_eq!(heading_level("3. Payment Terms"), Some(2));
        assert_eq!(heading_level("Terms of the Lease"), Some(2));
        assert_eq!(heading_level("The tenant pays monthly."), None);
        assert_eq!(heading_level("the lease terms"), None);
        assert_eq!(heading_level("Payment is due within thirty days"), None);
        assert_eq!(heading_level("12"), None);
        assert_eq!(heading_level(&"A".repeat(81)), None);
    }

    #[test]
    fn paragraphs_split_on_blank_lines() {
        let src =
            "SERVICE AGREEMENT\n\n  First paragraph, line one\nline two.  \n \n\nSecond one.\n";
        let (doc, _) = run(src.as_bytes(), &Limits::default());
        assert_eq!(doc.pages.len(), 1);
        let p = &doc.pages[0];
        assert_eq!(p.kind, PageKind::Logical);
        assert_eq!(p.text_source, TextSource::Native);
        assert!(!p.needs_ocr);
        assert_eq!(
            texts(p),
            [
                "SERVICE AGREEMENT",
                "First paragraph, line one\nline two.",
                "Second one."
            ]
        );
        assert_eq!(p.blocks[0].kind, BlockKind::Heading { level: 1 });
        assert_eq!(p.blocks[1].kind, BlockKind::Paragraph);
        assert!(p.blocks.iter().all(|b| b.zone == Zone::Body));
    }

    #[test]
    fn lines_are_blocks_without_blank_lines() {
        let (doc, _) = run(b"one line.\nanother line.\nthird.", &Limits::default());
        assert_eq!(
            texts(&doc.pages[0]),
            ["one line.", "another line.", "third."]
        );
    }

    #[test]
    fn crlf_spans_index_the_stored_text() {
        let src = "Heading Line\r\n\r\nBody with čćž\r\nmore\r\n";
        let (doc, mut out) = run(src.as_bytes(), &Limits::default());
        let p = &doc.pages[0];
        assert!(!p.text.contains('\r'));
        assert_eq!(texts(p), ["Heading Line", "Body with čćž\nmore"]);
        assert!(
            out.facts()
                .iter()
                .any(|f| f.key == keys::LINE_ENDINGS && f.value == Value::text("crlf"))
        );
        assert!(out.take_problems().is_empty());
    }

    #[test]
    fn form_feeds_make_pages_and_max_pages_holds() {
        let src = "page one\n\u{c}page two\n\u{c}\u{c}page four\u{c}";
        let (doc, _) = run(src.as_bytes(), &Limits::default());
        assert_eq!(doc.pages.len(), 4);
        assert_eq!(doc.pages[1].text, "page two\n");
        assert!(doc.pages[2].blocks.is_empty());
        assert_eq!(doc.pages[3].index, 3);
        let limits = Limits {
            max_pages: 2,
            ..Limits::default()
        };
        let (doc, mut out) = run(src.as_bytes(), &limits);
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(out.take_problems().len(), 1);
    }

    #[test]
    fn max_text_bytes_cuts_at_a_char_boundary() {
        let src = "čćžšđ".repeat(100);
        let limits = Limits {
            max_text_bytes: 101,
            ..Limits::default()
        };
        let (doc, mut out) = run(src.as_bytes(), &limits);
        let t = &doc.pages[0].text;
        assert_eq!(t.len(), 100);
        assert_eq!(out.take_problems().len(), 1);
        let limits = Limits {
            max_bytes: 10,
            ..Limits::default()
        };
        let (doc, mut out) = run(b"0123456789abcdef", &limits);
        assert_eq!(doc.pages[0].text, "0123456789");
        assert_eq!(out.take_problems().len(), 1);
    }
}

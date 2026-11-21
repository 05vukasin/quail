//! The structure of a document: its headings with page ranges, for documents of any length.
//!
//! [`outline`] reads every line of every page once (linear time, no model) and keeps the lines
//! that look like headings:
//!
//! - **DOCX headings**: blocks of kind [`BlockKind::Heading`], with their level;
//! - **keyword headings**: `PART II`, `CHAPTER 3`, `Article 5`, `Section 2.1`, `SCHEDULE A`,
//!   `Annex 1`, `Appendix B`, `Exhibit C`, `Član 4` (a bare keyword line takes the short line
//!   after it as its title: `CHAPTER 3` + `Storage and Retrieval`);
//! - **numbered headings**: `1. Definitions`, `2.3 Payment Terms`, `IV. RESULTS`, also run in
//!   (`7. TERM. This Agreement …` gives `7. TERM`); the depth of the number is the level;
//! - **capitals**: a short line in capitals (`TRUSTEES' REPORT`);
//! - **type size** (PDF): a short line whose glyphs are at least 1.2× the body text height, in
//!   up to three size classes, for books and reports whose headings carry no number.
//!
//! Running headers and footers (the same line on many pages), table-of-contents pages (lines
//! ending in page numbers or dot leaders) and sentences (long lines, lines ending in `,` or
//! `;`, lower-case prose after a number) are left out. The kinds found are ranked (part >
//! chapter > article > numbered by depth > capitals > type size by size) and the ranks that
//! occur become levels 1, 2, 3, ….
//!
//! The result is bounded: at most [`OutlineConfig::max_entries`] entries, whole levels first
//! (all level-1 headings, then level 2, …; a level that does not fit is sampled evenly).
//! Each entry gets the page range up to the next heading of the same or a higher level. A
//! document with fewer than [`OutlineConfig::min_headings`] headings gets fixed page windows
//! instead ([`OutlineMethod::Windows`]).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::Instant;

use quail_core::{BlockKind, Document, Page, Value};

use crate::format::clip;
use crate::tokens;

/// Settings of [`outline`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutlineConfig {
    /// Entries kept at most (whole levels first).
    pub max_entries: usize,
    /// Fewer headings than this: fixed page windows instead.
    pub min_headings: usize,
    /// Windows at most for a document without headings.
    pub max_windows: usize,
    /// Characters of a title kept.
    pub max_title_chars: usize,
}

impl Default for OutlineConfig {
    /// 64 entries, 3 headings, 16 windows, 90 characters.
    fn default() -> Self {
        Self {
            max_entries: 64,
            min_headings: 3,
            max_windows: 16,
            max_title_chars: 90,
        }
    }
}

/// How an outline was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineMethod {
    /// From the document's headings.
    Headings,
    /// Fixed page windows (no usable headings).
    Windows,
}

impl OutlineMethod {
    /// `headings` or `windows`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Headings => "headings",
            Self::Windows => "windows",
        }
    }
}

/// What made a line a heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadingSource {
    /// A DOCX heading block.
    Block,
    /// `CHAPTER 3`, `Article 5`, `SCHEDULE A`.
    Keyword,
    /// `2.3 Payment Terms`, `IV. RESULTS`.
    Numbered,
    /// A short line in capitals.
    Capitals,
    /// Larger type than the body text.
    Size,
    /// A fixed page window.
    Window,
}

impl HeadingSource {
    /// `block`, `keyword`, `numbered`, `capitals`, `size` or `window`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Keyword => "keyword",
            Self::Numbered => "numbered",
            Self::Capitals => "capitals",
            Self::Size => "size",
            Self::Window => "window",
        }
    }
}

/// One heading of an [`Outline`].
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    /// The heading as written (one line, clipped).
    pub title: String,
    /// 1 is the highest.
    pub level: u8,
    /// Zero-based page it is on.
    pub page: u32,
    /// Zero-based last page of its section (up to the next heading of the same or a higher
    /// level kept in the outline).
    pub end_page: u32,
    /// Byte offset of the heading in its page's text (where its section starts).
    pub offset: u32,
    /// The heading opens its page (nothing but a running header above it), so the section
    /// before it ends on the page before.
    pub opens_page: bool,
    /// What made it a heading.
    pub source: HeadingSource,
}

/// The headings of a document, bounded.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    /// How it was made.
    pub method: OutlineMethod,
    /// Entries in document order.
    pub entries: Vec<OutlineEntry>,
    /// Headings found before the cap.
    pub total: usize,
    /// Pages of the document.
    pub pages: u32,
    /// The deadline stopped the scan: only the pages before it were read.
    pub truncated: bool,
}

impl Outline {
    /// The outline as a record: `method total pages sections`, `sections` a list of
    /// `title level page end_page source` (pages zero-based, like [`quail_core::Evidence`]).
    pub fn to_value(&self) -> Value {
        let sections = self
            .entries
            .iter()
            .map(|e| {
                Value::record([
                    ("title", Value::text(e.title.as_str())),
                    ("level", Value::Int(i64::from(e.level))),
                    ("page", Value::Int(i64::from(e.page))),
                    ("end_page", Value::Int(i64::from(e.end_page))),
                    ("source", Value::text(e.source.as_str())),
                ])
            })
            .collect();
        let mut f = vec![
            ("method", Value::text(self.method.as_str())),
            (
                "total",
                Value::Int(i64::try_from(self.total).unwrap_or(i64::MAX)),
            ),
            ("pages", Value::Int(i64::from(self.pages))),
            ("sections", Value::List(sections)),
        ];
        if self.truncated {
            f.push(("truncated", Value::Bool(true)));
        }
        Value::record(f)
    }

    /// The outline as indented lines `- Title (pp. 3-7)` (pages one-based) within `max_tokens`
    /// estimated tokens: the deepest levels are dropped first, then the last entries, with a
    /// `(+N more)` line.
    pub fn render(&self, max_tokens: usize) -> String {
        let max_level = self.entries.iter().map(|e| e.level).max().unwrap_or(1);
        let lines_at = |depth: u8| -> Vec<String> {
            self.entries
                .iter()
                .filter(|e| e.level <= depth)
                .map(entry_line)
                .collect()
        };
        for depth in (1..=max_level).rev() {
            let lines = lines_at(depth);
            if tokens::estimate(&lines.join("\n")) > max_tokens {
                continue;
            }
            // A few top-level lines say little: the next level, cut, says more.
            if lines.len() < 6 && depth < max_level {
                return cut_lines(&lines_at(depth + 1), max_tokens);
            }
            return lines.join("\n");
        }
        cut_lines(&lines_at(1), max_tokens)
    }
}

/// `- Title (pp. 3-7)`, indented two spaces per level below 1.
fn entry_line(e: &OutlineEntry) -> String {
    let mut s = "  ".repeat(usize::from(e.level.saturating_sub(1)));
    s.push_str("- ");
    s.push_str(&e.title);
    s.push(' ');
    s.push_str(&page_range(e.page, e.end_page));
    s
}

/// `(p. 4)` or `(pp. 4-9)`, one-based.
pub fn page_range(page: u32, end: u32) -> String {
    if end <= page {
        format!("(p. {})", page + 1)
    } else {
        format!("(pp. {}-{})", page + 1, end + 1)
    }
}

/// The first lines that fit `max_tokens` with a `(+N more)` line.
fn cut_lines(lines: &[String], max_tokens: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for (i, l) in lines.iter().enumerate() {
        let cost = tokens::estimate(l);
        // Room for the "(+N more)" line (about 5 tokens).
        if used + cost + 5 > max_tokens {
            let _ = write!(out, "(+{} more)", lines.len() - i);
            return out;
        }
        used += cost;
        out.push_str(l);
        out.push('\n');
    }
    out.trim_end().to_owned()
}

/// A heading candidate before ranking.
#[derive(Clone, Debug)]
struct Cand {
    title: String,
    /// Smaller is higher; becomes the level.
    rank: u16,
    page: u32,
    offset: u32,
    /// Share of the page's text before it (0 = top of the page).
    pos: f32,
    source: HeadingSource,
    /// Letters only, lower case, for running-header detection.
    key: String,
    /// Glyph height of the line, when known.
    height: Option<f32>,
}

/// Keyword words (lower case) and their rank.
const KEYWORDS: &[(&str, u16)] = &[
    ("part", 10),
    ("deo", 10),
    ("dio", 10),
    ("title", 12),
    ("book", 10),
    ("chapter", 20),
    ("glava", 20),
    ("poglavlje", 20),
    ("schedule", 20),
    ("annex", 20),
    ("annexure", 20),
    ("appendix", 20),
    ("exhibit", 20),
    ("attachment", 20),
    ("prilog", 20),
    ("article", 30),
    ("clan", 30),
    ("član", 30),
    ("section", 40),
    ("clause", 40),
    ("odeljak", 40),
];

/// Rank of a numbered heading of depth 1 (`1.`); each level of depth adds 10.
const NUMBERED_RANK: u16 = 30;
/// Rank of a roman-numbered heading (`IV. RESULTS`).
const ROMAN_RANK: u16 = 25;
/// Rank of a line in capitals.
const CAPS_RANK: u16 = 45;
/// Rank of the largest type size class; each smaller class adds 10.
const SIZE_RANK: u16 = 50;

/// Words that make a capitals line no heading.
const NOT_HEADINGS: &[&str] = &[
    "in witness whereof",
    "confidential",
    "execution copy",
    "execution version",
    "table of contents",
    "contents",
    "page",
    "draft",
    "by",
    "name",
    "title",
    "date",
    "signature",
];

/// The outline of `doc`. Stops at `deadline` (marking it [`Outline::truncated`]).
pub fn outline(doc: &Document, cfg: &OutlineConfig, deadline: Option<Instant>) -> Outline {
    let pages = u32::try_from(doc.pages.len()).unwrap_or(u32::MAX);
    let mut truncated = false;
    let body_height = body_height(doc);
    let mut cands: Vec<Cand> = Vec::new();
    for (n, p) in doc.pages.iter().enumerate() {
        if n % 16 == 0 && deadline.is_some_and(|d| Instant::now() >= d) {
            truncated = true;
            break;
        }
        page_candidates(p, body_height, cfg, &mut cands);
    }
    let cands = drop_running(cands, doc.pages.len());
    let cands = assign_size_ranks(cands, body_height);
    let total = cands.len();
    if total < cfg.min_headings.max(1) {
        return Outline {
            method: OutlineMethod::Windows,
            entries: windows(doc, cfg),
            total,
            pages,
            truncated,
        };
    }
    let mut entries = levels(cands);
    entries = cap(entries, cfg.max_entries);
    set_end_pages(&mut entries, pages);
    Outline {
        method: OutlineMethod::Headings,
        entries,
        total,
        pages,
        truncated,
    }
}

/// Median glyph height of the document's lines (PDF line geometry), when there is any.
fn body_height(doc: &Document) -> Option<f32> {
    let mut hs: Vec<f32> = Vec::new();
    for p in &doc.pages {
        for b in &p.blocks {
            for l in &b.lines {
                let h = l.bbox.y1 - l.bbox.y0;
                if h.is_finite() && h > 0.5 {
                    hs.push(h);
                }
            }
            if hs.len() >= 20_000 {
                break;
            }
        }
    }
    if hs.len() < 20 {
        return None;
    }
    let mid = hs.len() / 2;
    let (_, m, _) = hs.select_nth_unstable_by(mid, f32::total_cmp);
    Some(*m)
}

/// One line of a page: its text, byte offset and glyph height.
struct Line<'a> {
    text: &'a str,
    offset: usize,
    height: Option<f32>,
    heading_level: Option<u8>,
}

fn lines_of(p: &Page) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    if p.blocks.is_empty() {
        let mut off = 0;
        for l in p.text.split('\n') {
            out.push(Line {
                text: l,
                offset: off,
                height: None,
                heading_level: None,
            });
            off += l.len() + 1;
        }
        return out;
    }
    for b in &p.blocks {
        let level = match b.kind {
            BlockKind::Heading { level } => Some(level.max(1)),
            BlockKind::TableCell { .. } | BlockKind::Note | BlockKind::Annotation => continue,
            _ => None,
        };
        if level.is_some() || b.lines.is_empty() {
            let text = b.span.slice(&p.text);
            if level.is_some() {
                out.push(Line {
                    text,
                    offset: b.span.start as usize,
                    height: None,
                    heading_level: level,
                });
                continue;
            }
            let mut off = b.span.start as usize;
            for l in text.split('\n') {
                out.push(Line {
                    text: l,
                    offset: off,
                    height: None,
                    heading_level: None,
                });
                off += l.len() + 1;
            }
        } else {
            for l in &b.lines {
                out.push(Line {
                    text: l.span.slice(&p.text),
                    offset: l.span.start as usize,
                    height: Some(l.bbox.y1 - l.bbox.y0),
                    heading_level: None,
                });
            }
        }
    }
    out
}

/// A table-of-contents page: many lines end in a page number or carry dot leaders.
fn is_toc_page(lines: &[Line<'_>]) -> bool {
    let mut n = 0;
    let mut toc = 0;
    for l in lines {
        let t = l.text.trim();
        if t.is_empty() {
            continue;
        }
        n += 1;
        if toc_line(t) {
            toc += 1;
        }
    }
    toc >= 5 && toc * 10 >= n * 3
}

/// Dot leaders: `Introduction ........ 5`, `1. Scope . . . 12`.
fn leader_line(t: &str) -> bool {
    t.contains("....") || t.contains(". . .") || t.contains('…')
}

/// A table-of-contents line: dot leaders, or a title ending in a page number
/// (`Storage and Retrieval 69`). Judged per page ([`is_toc_page`]): a heading such as
/// `Article 5` ends in a number too.
fn toc_line(t: &str) -> bool {
    if leader_line(t) {
        return true;
    }
    let mut ws = t.split_whitespace().rev();
    let last = ws.next().unwrap_or("");
    let before = ws.next();
    before.is_some_and(|b| b.chars().any(char::is_alphabetic))
        && (1..=4).contains(&last.len())
        && (last.chars().all(|c| c.is_ascii_digit()) || roman_value(last).is_some())
}

/// Lower-case letters (and digits, with `digits`) of `t`: the running-header key.
fn letter_key(t: &str, digits: bool) -> String {
    t.chars()
        .filter(|c| c.is_alphabetic() || (digits && c.is_ascii_digit()))
        .flat_map(char::to_lowercase)
        .collect()
}

#[allow(clippy::many_single_char_names)] // line, text, title, index
fn page_candidates(p: &Page, body: Option<f32>, cfg: &OutlineConfig, out: &mut Vec<Cand>) {
    let lines = lines_of(p);
    if is_toc_page(&lines) {
        return;
    }
    #[allow(clippy::cast_precision_loss)] // page text lengths
    let len = p.text.len().max(1) as f32;
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        i += 1;
        let t = l.text.trim();
        if t.is_empty() {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let pos = l.offset as f32 / len;
        let offset = u32::try_from(l.offset).unwrap_or(u32::MAX);
        let mut push = |title: String, rank: u16, source: HeadingSource, height: Option<f32>| {
            let title = clip(&title, cfg.max_title_chars);
            // Numbered headings repeat their words ("1.1 Scope", "2.1 Scope"): their key
            // keeps the digits. Running headers in capitals carry page numbers: theirs not.
            let digits = !matches!(source, HeadingSource::Capitals | HeadingSource::Size);
            let key = letter_key(&title, digits);
            out.push(Cand {
                title,
                rank,
                page: p.index,
                offset,
                pos,
                source,
                key,
                height,
            });
        };
        if let Some(level) = l.heading_level {
            if t.chars().any(char::is_alphanumeric) && t.split_whitespace().count() <= 20 {
                push(
                    t.to_owned(),
                    u16::from(level) * 10,
                    HeadingSource::Block,
                    None,
                );
            }
            continue;
        }
        if t.len() > 200 || leader_line(t) || page_furniture(t) {
            continue;
        }
        // The next non-empty line, a title for a bare keyword or number line.
        let next = lines[i..]
            .iter()
            .position(|n| !n.text.trim().is_empty())
            .map(|k| (i + k, lines[i + k].text.trim()));
        if let Some((rank, bare)) = keyword(t) {
            let mut title = t.to_owned();
            if bare && let Some((k, n)) = next.filter(|(_, n)| title_line(n)) {
                title.push(' ');
                title.push_str(n);
                i = k + 1;
            } else if bare && t.ends_with('.') {
                continue; // "... as discussed in / Part III." : a sentence wrapped at its end
            }
            push(title, rank, HeadingSource::Keyword, l.height);
            continue;
        }
        if let Some((rank, title, bare)) = numbered(t) {
            let mut title = title;
            if bare && let Some((k, n)) = next.filter(|(_, n)| title_line(n)) {
                title.push(' ');
                title.push_str(n);
                i = k + 1;
            }
            push(title, rank, HeadingSource::Numbered, l.height);
            continue;
        }
        // Capitals in smaller type than the body are code, tables or fine print.
        let small = matches!((l.height, body), (Some(h), Some(b)) if h < b * 0.95);
        if !small && capitals(t) {
            push(t.to_owned(), CAPS_RANK, HeadingSource::Capitals, l.height);
            continue;
        }
        if let (Some(h), Some(b)) = (l.height, body)
            && h >= b * 1.2
            && title_line(t)
        {
            push(t.to_owned(), SIZE_RANK, HeadingSource::Size, Some(h));
        }
    }
}

/// A short title-like line: 1–14 words with letters, no sentence or list punctuation at its
/// end, not starting in lower case.
fn title_line(t: &str) -> bool {
    let n = t.split_whitespace().count();
    (1..=14).contains(&n)
        && t.chars().filter(|c| c.is_alphabetic()).count() >= 2
        && !t.ends_with([',', ';', '.', ':'])
        && !t.starts_with(|c: char| c.is_lowercase())
        && !leader_line(t)
        && !page_furniture(t)
}

/// A running header or footer with its page number: `24 | Chapter 1: Reliability`,
/// `Problems with Replication Lag | 167`, `Page 3 of 9`.
fn page_furniture(t: &str) -> bool {
    let number = |s: &str| {
        let s = s.trim();
        !s.is_empty()
            && s.len() <= 5
            && s.chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'i' | 'v' | 'x'))
    };
    if let Some((a, b)) = t.split_once('|')
        && (number(a) || number(b))
    {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    lower.starts_with("page ") && lower.contains(" of ") && t.len() < 24
}

/// Words in title case or capitals: most words of four letters or more start upper case.
fn titleish(rest: &str) -> bool {
    let mut long = 0;
    let mut upper = 0;
    for w in rest.split_whitespace() {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        if w.chars().count() >= 4 {
            long += 1;
            if w.starts_with(|c: char| c.is_uppercase()) {
                upper += 1;
            }
        }
    }
    rest.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit() || c == '"')
        && upper * 10 >= long * 6
}

/// A keyword heading: `(rank, bare)`, `bare` when nothing follows the number.
fn keyword(t: &str) -> Option<(u16, bool)> {
    let mut ws = t.split_whitespace();
    let first = ws.next()?;
    if first.len() > 12 || !first.starts_with(|c: char| c.is_uppercase()) {
        return None;
    }
    let word = first.trim_end_matches(['.', ':']).to_lowercase();
    let &(_, rank) = KEYWORDS.iter().find(|(k, _)| *k == word)?;
    // A keyword in prose is lower case ("this section 4 applies"): heading keywords are
    // capitalized.
    if !first.starts_with(|c: char| c.is_uppercase()) {
        return None;
    }
    let num = ws.next()?;
    // "Chapter 4)." is the end of a parenthesis in prose.
    let num = num.trim_end_matches(['.', ':', '-', '–', '—']);
    if !label_number(num) {
        return None;
    }
    let rest: Vec<&str> = ws.collect();
    let rest = rest.join(" ");
    let rest = rest.trim_start_matches(['-', '–', '—', ':', '.', ' ']);
    if rest.is_empty() {
        return Some((rank, true));
    }
    let (head, run_in) = run_in(rest);
    if t.split_whitespace().count() > 16 && !run_in {
        return None;
    }
    // "Section 4 of this Agreement shall …": prose, not a heading.
    if !titleish(head)
        || (!run_in && t.ends_with([',', ';', '.']) && head.split_whitespace().count() > 8)
    {
        return None;
    }
    Some((rank, false))
}

/// A heading number: `1`, `2.3`, `IV`, `A`, `One`.
fn label_number(n: &str) -> bool {
    if n.is_empty() || n.len() > 8 {
        return false;
    }
    let digits = n.chars().all(|c| c.is_ascii_digit() || c == '.')
        && n.starts_with(|c: char| c.is_ascii_digit());
    let letter = n.len() == 1 && n.chars().all(|c| c.is_ascii_uppercase());
    let roman = roman_value(n).is_some();
    let word = matches!(
        n.to_lowercase().as_str(),
        "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine" | "ten"
    );
    digits || letter || roman || word
}

/// The value of a roman numeral `I`–`L` (upper case only).
fn roman_value(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 6 || !s.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L')) {
        return None;
    }
    let val = |c: char| match c {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        _ => 50,
    };
    let cs: Vec<i64> = s.chars().map(val).collect();
    let mut total = 0i64;
    for (i, &v) in cs.iter().enumerate() {
        if cs.get(i + 1).is_some_and(|&n| n > v) {
            total -= v;
        } else {
            total += v;
        }
    }
    u32::try_from(total).ok().filter(|&v| v > 0)
}

/// Splits a run-in heading: `TERM. This Agreement …` → (`TERM`, true); otherwise the whole
/// text and false.
fn run_in(rest: &str) -> (&str, bool) {
    if let Some(i) = rest.find(". ") {
        let head = &rest[..i];
        let n = head.split_whitespace().count();
        let after = rest[i + 2..].trim_start();
        if (1..=8).contains(&n) && titleish(head) && after.starts_with(|c: char| c.is_uppercase()) {
            return (head, true);
        }
    }
    (rest, false)
}

/// A numbered heading: `(rank, title, bare)`.
fn numbered(t: &str) -> Option<(u16, String, bool)> {
    if !t.starts_with(|c: char| c.is_ascii_digit() || matches!(c, 'I' | 'V' | 'X' | 'L')) {
        return None;
    }
    let mut parts = t.splitn(2, char::is_whitespace);
    let num = parts.next()?;
    let rest = parts.next().unwrap_or("").trim();
    let core = num.trim_end_matches(['.', ')']);
    let (rank, depth_ok) = if core.chars().all(|c| c.is_ascii_digit() || c == '.')
        && core.starts_with(|c: char| c.is_ascii_digit())
    {
        let comps: Vec<&str> = core.split('.').collect();
        if comps.iter().any(|c| c.is_empty() || c.len() > 3) || comps.len() > 4 {
            return None;
        }
        // "1" alone needs its period or parenthesis ("1. Scope"), else it is a list number
        // or a quantity.
        if comps.len() == 1 && num.len() == core.len() {
            return None;
        }
        let depth = u16::try_from(comps.len()).unwrap_or(4);
        (NUMBERED_RANK + (depth - 1) * 10, true)
    } else if num.ends_with('.') && roman_value(core).is_some() {
        (ROMAN_RANK, true)
    } else {
        return None;
    };
    if !depth_ok {
        return None;
    }
    if rest.is_empty() {
        return Some((rank, num.to_owned(), true));
    }
    let (head, is_run_in) = run_in(rest);
    let words = head.split_whitespace().count();
    if is_run_in {
        return Some((rank, format!("{num} {head}"), false));
    }
    if words > 12 || t.ends_with([',', ';']) || !titleish(head) {
        return None;
    }
    // A full sentence after a number is a list item: "2. The Supplier shall deliver …".
    if t.ends_with('.') && words > 6 {
        return None;
    }
    Some((rank, t.to_owned(), false))
}

/// A short line in capitals: 1–10 words, at least 5 letters, no lower-case letter.
fn capitals(t: &str) -> bool {
    let (mut letters, mut digits) = (0, 0);
    for c in t.chars() {
        if c.is_lowercase() || matches!(c, '(' | ')' | '=' | '{' | '}' | '_' | '<' | '>') {
            return false; // prose, or code
        }
        if c.is_alphabetic() {
            letters += 1;
        } else if c.is_ascii_digit() {
            digits += 1;
        }
    }
    let n = t.split_whitespace().count();
    if !(1..=10).contains(&n) || letters < 5 || digits * 2 > letters {
        return false;
    }
    if n == 1 && letters < 6 {
        return false;
    }
    if t.ends_with([',', ';']) {
        return false;
    }
    let k = t.to_lowercase();
    let k = k.trim_matches(|c: char| !c.is_alphanumeric());
    !NOT_HEADINGS
        .iter()
        .any(|w| k == *w || k.starts_with(&format!("{w}:")))
}

/// Drops running headers and footers: a line whose letters repeat on at least 3 pages and
/// a tenth of the document. The first occurrence of a keyword or numbered one stays (a
/// chapter title repeated in the page header marks where the chapter starts).
fn drop_running(cands: Vec<Cand>, pages: usize) -> Vec<Cand> {
    let mut seen: HashMap<&str, (u32, usize)> = HashMap::new();
    for c in &cands {
        let e = seen.entry(c.key.as_str()).or_insert((u32::MAX, 0));
        if e.0 != c.page {
            e.0 = c.page;
            e.1 += 1;
        }
    }
    let threshold = (pages / 10).max(3);
    let running: HashMap<String, usize> = seen
        .into_iter()
        .filter(|(_, (_, n))| *n >= threshold)
        .map(|(k, (_, n))| (k.to_owned(), n))
        .collect();
    let mut kept_first: HashSet<String> = HashSet::new();
    cands
        .into_iter()
        .filter(|c| {
            if c.key.is_empty() {
                return true; // "1." or "IV." alone: numbering, judged elsewhere
            }
            if !running.contains_key(&c.key) {
                return true;
            }
            let structural = matches!(
                c.source,
                HeadingSource::Keyword | HeadingSource::Numbered | HeadingSource::Block
            );
            structural && kept_first.insert(c.key.clone())
        })
        .collect()
}

/// Type-size candidates get one of three ranks by glyph height (largest first). When
/// numbered or keyword headings outnumber them (a contract), only the largest class is kept:
/// the smaller larger-type lines there are mostly emphasis.
fn assign_size_ranks(mut cands: Vec<Cand>, body: Option<f32>) -> Vec<Cand> {
    let Some(body) = body else {
        return cands;
    };
    let mut sizes: Vec<f32> = cands
        .iter()
        .filter(|c| c.source == HeadingSource::Size)
        .filter_map(|c| c.height)
        .map(|h| (h * 2.0).round() / 2.0)
        .collect();
    if sizes.is_empty() {
        return cands;
    }
    let size_count = sizes.len();
    sizes.sort_by(|a, b| b.total_cmp(a));
    // Classes: sizes used often enough (a cover page's one huge line is no class), at least
    // 0.08 x body apart, largest first.
    let min_count = (size_count / 40).max(4);
    let mut classes: Vec<f32> = Vec::new();
    let mut i = 0;
    while i < sizes.len() {
        let s = sizes[i];
        let n = sizes[i..]
            .iter()
            .take_while(|&&x| x.total_cmp(&s).is_eq())
            .count();
        i += n;
        if n >= min_count && classes.last().is_none_or(|&l| l - s >= body * 0.08) {
            classes.push(s);
        }
    }
    let structural = cands
        .iter()
        .filter(|c| matches!(c.source, HeadingSource::Keyword | HeadingSource::Numbered))
        .count();
    // A numbered contract's larger lines are mostly its title and emphasis: one class; a
    // book's or report's unnumbered headings: three.
    let keep = if structural >= 3 && structural * 2 >= size_count {
        1
    } else {
        3
    };
    cands.retain_mut(|c| {
        if c.source != HeadingSource::Size {
            return true;
        }
        let h = c.height.map_or(0.0, |h| (h * 2.0).round() / 2.0);
        // The class of this height (within 0.06 x body), if any.
        let class = classes.iter().position(|&s| (h - s).abs() <= body * 0.06);
        match class {
            Some(k) if k < keep => {
                c.rank = SIZE_RANK + u16::try_from(k).unwrap_or(0) * 10;
                true
            }
            _ => false,
        }
    });
    cands
}

/// Ranks that occur become levels 1, 2, 3, … (at most 6).
fn levels(cands: Vec<Cand>) -> Vec<OutlineEntry> {
    let mut ranks: Vec<u16> = cands.iter().map(|c| c.rank).collect();
    ranks.sort_unstable();
    ranks.dedup();
    let mut entries: Vec<OutlineEntry> = Vec::with_capacity(cands.len());
    for c in cands {
        let level = ranks.iter().position(|&r| r == c.rank).unwrap_or(0) + 1;
        // The same heading twice in a row (a title split over two lines) is one entry.
        if entries
            .last()
            .is_some_and(|l: &OutlineEntry| l.title == c.title && l.page == c.page)
        {
            continue;
        }
        entries.push(OutlineEntry {
            title: c.title,
            level: u8::try_from(level.min(6)).unwrap_or(6),
            page: c.page,
            end_page: c.page,
            offset: c.offset,
            opens_page: c.pos < 0.12,
            source: c.source,
        });
    }
    entries
}

/// At most `max` entries: every level from the top while it fits, then an even sample of
/// the first level that does not fit, in document order.
fn cap(entries: Vec<OutlineEntry>, max: usize) -> Vec<OutlineEntry> {
    if entries.len() <= max {
        return entries;
    }
    let mut keep = vec![false; entries.len()];
    let mut used = 0;
    for level in 1..=6u8 {
        let idx: Vec<usize> = (0..entries.len())
            .filter(|&i| entries[i].level == level)
            .collect();
        if idx.is_empty() {
            continue;
        }
        if used + idx.len() <= max {
            for &i in &idx {
                keep[i] = true;
            }
            used += idx.len();
            continue;
        }
        let room = max - used;
        for k in 0..room {
            keep[idx[k * idx.len() / room]] = true;
        }
        break;
    }
    entries
        .into_iter()
        .zip(keep)
        .filter_map(|(e, k)| k.then_some(e))
        .collect()
}

/// Each entry's section ends where the next entry of the same or a higher level starts: on
/// the page before it when that heading opens its page, else on its page.
fn set_end_pages(entries: &mut [OutlineEntry], pages: u32) {
    let last = pages.saturating_sub(1);
    for i in 0..entries.len() {
        let level = entries[i].level;
        let next = entries[i + 1..].iter().find(|n| n.level <= level);
        let end = match next {
            Some(n) if n.opens_page && n.page > entries[i].page => n.page - 1,
            Some(n) => n.page,
            None => last,
        };
        entries[i].end_page = end.max(entries[i].page);
    }
}

/// Fixed page windows: at most `max_windows`, at least 3 pages each (one window for a
/// short document).
fn windows(doc: &Document, cfg: &OutlineConfig) -> Vec<OutlineEntry> {
    let n = doc.pages.len();
    if n == 0 {
        return Vec::new();
    }
    let size = n.div_ceil(cfg.max_windows.max(1)).max(3);
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let end = (start + size).min(n) - 1;
        let page = doc.pages.get(start).map_or(0, |p| p.index);
        let end_page = doc.pages.get(end).map_or(page, |p| p.index);
        let title = if end_page > page {
            format!("Pages {}-{}", page + 1, end_page + 1)
        } else {
            format!("Page {}", page + 1)
        };
        out.push(OutlineEntry {
            title,
            level: 1,
            page,
            end_page,
            offset: 0,
            opens_page: true,
            source: HeadingSource::Window,
        });
        start = end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Block, PageKind, Span, TextSource, Zone};

    fn doc(pages: &[&str]) -> Document {
        Document {
            pages: pages
                .iter()
                .enumerate()
                .map(|(i, t)| Page {
                    text: (*t).to_owned(),
                    text_source: TextSource::Native,
                    ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
                })
                .collect(),
        }
    }

    fn titles(o: &Outline) -> Vec<(String, u8, u32, u32)> {
        o.entries
            .iter()
            .map(|e| (e.title.clone(), e.level, e.page, e.end_page))
            .collect()
    }

    #[test]
    fn finds_keyword_and_numbered_headings() {
        let d = doc(&[
            "SERVICES AGREEMENT\nThis agreement is made by the parties named below.\n\
             ARTICLE 1\nDEFINITIONS\nIn this agreement the following words apply.\n\
             1.1 Services\nThe supplier provides the services.",
            "1.2 Fees and Expenses\nFees are payable monthly.\nARTICLE 2\nTERM\n\
             The term is two years from signature.\n2.1 Renewal\nIt renews once.",
            "SCHEDULE A\nList of services and prices for each service in detail.",
        ]);
        let o = outline(&d, &OutlineConfig::default(), None);
        assert_eq!(o.method, OutlineMethod::Headings);
        assert_eq!(
            titles(&o),
            vec![
                ("SERVICES AGREEMENT".into(), 4, 0, 0),
                ("ARTICLE 1 DEFINITIONS".into(), 2, 0, 1),
                ("1.1 Services".into(), 3, 0, 0),
                ("1.2 Fees and Expenses".into(), 3, 1, 1),
                ("ARTICLE 2 TERM".into(), 2, 1, 1),
                ("2.1 Renewal".into(), 3, 1, 1),
                ("SCHEDULE A".into(), 1, 2, 2),
            ]
        );
    }

    #[test]
    fn prose_and_list_items_are_no_headings() {
        assert!(
            keyword("Section 4 of this Agreement shall survive any termination of it.").is_none()
        );
        assert!(keyword("section 4 applies").is_none());
        assert!(
            numbered("2. The Supplier shall deliver the goods to the warehouse on time.").is_none()
        );
        assert!(numbered("3 apples").is_none());
        assert!(numbered("2023 Annual Report").is_none());
        assert_eq!(
            numbered("7. TERM. This Agreement starts today.").map(|n| n.1),
            Some("7. TERM".to_owned())
        );
        assert_eq!(numbered("IV. RESULTS").map(|n| n.0), Some(ROMAN_RANK));
        assert!(!capitals("IN WITNESS WHEREOF"));
        assert!(capitals("TRUSTEES REPORT"));
        assert!(toc_line("Storage and Retrieval 69"));
        assert!(toc_line("1. Scope .......... 3"));
    }

    #[test]
    fn running_headers_and_toc_pages_are_dropped() {
        let mut pages = vec![
            "CONTENTS\nIntroduction 1\nMethods 4\nResults 9\nDiscussion 12\nReferences 15\nIndex 17"
                .to_owned(),
        ];
        for i in 1..12 {
            pages.push(format!(
                "ANNUAL REVIEW {i}\nSome text about the year in review for this page here.\n\
                 {i}. Topic Number {i}\nMore text follows on the topic of this page."
            ));
        }
        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        let o = outline(&doc(&refs), &OutlineConfig::default(), None);
        let t = titles(&o);
        assert!(
            t.iter().all(|(s, ..)| !s.starts_with("ANNUAL REVIEW")),
            "{t:?}"
        );
        assert!(
            t.iter()
                .all(|(s, ..)| s != "CONTENTS" && !s.starts_with("Methods"))
        );
        assert_eq!(t.len(), 11);
        assert_eq!(t[0].0, "1. Topic Number 1");
    }

    #[test]
    fn docx_heading_blocks_and_windows() {
        let text = "Overview\nBody text.\nDetails\nMore body.";
        let mut p = Page {
            text: text.to_owned(),
            text_source: TextSource::Native,
            ..Page::new(0, PageKind::Logical)
        };
        let blk =
            |s: u32, e: u32, kind| Block::new(Span { start: s, end: e }, kind, Zone::Body, None);
        p.blocks = vec![
            blk(0, 8, BlockKind::Heading { level: 1 }),
            blk(9, 19, BlockKind::Paragraph),
            blk(20, 27, BlockKind::Heading { level: 2 }),
            blk(28, 38, BlockKind::Paragraph),
        ];
        let d = Document {
            pages: vec![p.clone(), Page { index: 1, ..p }],
        };
        let o = outline(&d, &OutlineConfig::default(), None);
        assert_eq!(o.entries.len(), 4);
        assert_eq!(o.entries[0].source, HeadingSource::Block);
        assert_eq!((o.entries[1].level, o.entries[0].end_page), (2, 0));

        let plain: Vec<String> = (0..40)
            .map(|i| format!("plain text of page {i}."))
            .collect();
        let refs: Vec<&str> = plain.iter().map(String::as_str).collect();
        let o = outline(&doc(&refs), &OutlineConfig::default(), None);
        assert_eq!(o.method, OutlineMethod::Windows);
        assert_eq!(o.entries.len(), 14);
        assert_eq!(o.entries[0].title, "Pages 1-3");
        assert_eq!(o.entries[13].end_page, 39);
    }

    #[test]
    fn caps_whole_levels_first() {
        let mut pages = Vec::new();
        for c in 1..=10 {
            let mut t = format!("CHAPTER {c}\nThe Title Of Chapter {c}\n");
            for s in 1..=10 {
                let _ = writeln!(t, "{c}.{s} Section Name\nBody text of the section.");
            }
            pages.push(t);
        }
        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        let cfg = OutlineConfig {
            max_entries: 30,
            ..OutlineConfig::default()
        };
        let o = outline(&doc(&refs), &cfg, None);
        assert_eq!(o.total, 110);
        assert_eq!(o.entries.len(), 30);
        assert_eq!(o.entries.iter().filter(|e| e.level == 1).count(), 10);
        assert_eq!(o.entries[0].title, "CHAPTER 1 The Title Of Chapter 1");
        assert_eq!(o.entries[0].end_page, 0);
        let r = o.render(60);
        assert!(tokens::estimate(&r) <= 60, "{r}");
        assert!(r.contains("(+"), "{r}");
        assert!(o.render(2000).contains("  - 1.1 Section Name (p. 1)"));
    }
}

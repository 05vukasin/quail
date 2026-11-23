//! A bounded summary input for documents of any length: the card, the outline, the start of
//! the text and the most salient sentences of every section, within a token budget.
//!
//! A small local model reads ~1,000–3,000 tokens in a reasonable time; a 2,000-page document
//! has a million. [`build`] turns any document into a [`Digest`] of at most
//! [`DigestConfig::max_tokens`] estimated tokens ([`crate::tokens`]), without a model:
//!
//! 1. **Short documents** whose whole text fits: the card and the whole text
//!    ([`DigestMethod::Full`]).
//! 2. Otherwise ([`DigestMethod::Excerpts`]):
//!    - the card (`FACTS`);
//!    - the [outline](crate::outline) (`OUTLINE`, at most [`DigestConfig::outline_share`] of
//!      the budget, deepest levels dropped first);
//!    - the start of the text (`START`, [`DigestConfig::lead_share`]): title, parties, recitals;
//!    - per section (`EXCERPTS`): the outline's sections, at the deepest level that gives at
//!      most [`DigestConfig::max_groups`] sections (consecutive sections merged when even the
//!      top level has more), each with sentences **copied verbatim** from it. The excerpt budget
//!      is split across sections in proportion to their size, with a floor, so every section is
//!      represented and long ones get more.
//!
//! **Sentence selection** per section, linear in the sampled text: at most
//! [`DigestConfig::units_per_group`] paragraphs per section (its first paragraphs plus an even
//! sample of the rest), split into sentences of 8–60 words; each sentence scores by keyword
//! coverage (the section's recurring words weighted by their inverse section frequency across
//! the document, plus the section title's words: a cheap centroid stand-in for `TextRank`),
//! a position prior (a section's first paragraphs state what it is about), a cue bonus (who
//! provides what) and penalties for boilerplate, definitions, list fragments and contact lines.
//! Sentences are then picked by maximal marginal relevance (`λ`
//! [`DigestConfig::mmr_lambda`]) against the sentences already picked in the document, so
//! repeated clauses are not picked twice, and printed in document order.
//!
//! Everything is deterministic. Every excerpt is a substring of the normalized page text
//! (`quail_text::normalize`, whitespace collapsed), so a digest holds no fact the document does
//! not ([`Digest::verbatim`] checks it).
//!
//! [`map_parts`] splits the same selection into at most `n` contiguous parts with a budget each,
//! for a hierarchical (map-reduce) summary of very long documents (feature `llm`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

use quail_core::{BlockKind, Document};
use quail_text::NormalizedText;
use quail_text::normalize::{NormalizeOptions, Whitespace, normalize};

use crate::extractive::{
    ABBREVIATIONS, Flags, PhraseIndex, STOP, folded, fragment, strip_enumerator,
};
use crate::outline::{Outline, OutlineMethod, page_range};
use crate::terms::TermIndex;
use crate::tokens;
use crate::topics;

/// Settings of [`build`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DigestConfig {
    /// Estimated tokens of the whole digest at most, card included (at least 300).
    pub max_tokens: usize,
    /// Token budget of the card in it.
    pub card_tokens: usize,
    /// Share of the budget for the outline.
    pub outline_share: f32,
    /// Share of the budget for the start of the text.
    pub lead_share: f32,
    /// Sections at most (also at most one per 125 tokens of budget, at least 4).
    pub max_groups: usize,
    /// Tokens every section gets before the rest is split by size.
    pub floor_tokens: usize,
    /// Paragraphs read per section at most.
    pub units_per_group: usize,
    /// Bytes of paragraphs read per section at most (fewer paragraphs when they are long).
    pub bytes_per_group: usize,
    /// Candidate sentences per section at most.
    pub candidates_per_group: usize,
    /// Relevance against redundancy in the sentence pick (1 = relevance only).
    pub mmr_lambda: f64,
    /// Keywords per section (0: none), when a term index is given.
    pub section_keywords: usize,
}

impl Default for DigestConfig {
    /// 2,000 tokens, 300 card tokens, 15 % outline, 10 % start, 16 sections, floor 40,
    /// 48 paragraphs and 120 sentences per section, λ 0.7.
    fn default() -> Self {
        Self {
            max_tokens: 2000,
            card_tokens: 300,
            outline_share: 0.15,
            lead_share: 0.10,
            max_groups: 16,
            floor_tokens: 40,
            units_per_group: 48,
            bytes_per_group: 12 * 1024,
            candidates_per_group: 120,
            mmr_lambda: 0.7,
            section_keywords: 5,
        }
    }
}

/// How a digest was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestMethod {
    /// The whole text fits.
    Full,
    /// Outline, start and per-section excerpts.
    Excerpts,
}

impl DigestMethod {
    /// `full` or `excerpts`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Excerpts => "excerpts",
        }
    }
}

/// A sentence copied from a section.
#[derive(Clone, Debug, PartialEq)]
pub struct Excerpt {
    /// The sentence, verbatim (normalized whitespace, leading clause number dropped).
    pub text: String,
    /// Zero-based page.
    pub page: u32,
}

/// One section of a digest.
#[derive(Clone, Debug, PartialEq)]
pub struct DigestSection {
    /// Its heading (or `Pages 4-9`, or the first heading `(+N more)` of merged sections).
    pub title: String,
    /// Zero-based first page.
    pub page: u32,
    /// Zero-based last page.
    pub end_page: u32,
    /// Estimated tokens of its text.
    pub size_tokens: usize,
    /// Tokens of excerpts it was given.
    pub budget: usize,
    /// Candidate sentences it had.
    pub candidates: usize,
    /// The picked sentences, in document order.
    pub excerpts: Vec<Excerpt>,
    /// Its distinctive keywords ([`crate::topics::range_keywords`]), printed under its title.
    pub keywords: Vec<String>,
}

/// The bounded summary input of one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Digest {
    /// How it was made.
    pub method: DigestMethod,
    /// Outline, start and excerpts (or the whole text): what a model reads as TEXT.
    pub body: String,
    /// `FACTS` (the card) and [`Self::body`]: the full model input.
    pub text: String,
    /// Estimated tokens of [`Self::text`].
    pub tokens: usize,
    /// Estimated tokens of the whole document text.
    pub doc_tokens: usize,
    /// The sections and their excerpts (empty for [`DigestMethod::Full`]).
    pub sections: Vec<DigestSection>,
    /// The deadline stopped the selection: later sections have no excerpts.
    pub truncated: bool,
}

impl Digest {
    /// Share of sections with at least one excerpt among those that had a candidate sentence
    /// (1 when there are none, or for [`DigestMethod::Full`]).
    pub fn coverage(&self) -> f64 {
        let with = self.sections.iter().filter(|s| s.candidates > 0).count();
        if with == 0 {
            return 1.0;
        }
        let hit = self
            .sections
            .iter()
            .filter(|s| s.candidates > 0 && !s.excerpts.is_empty())
            .count();
        #[allow(clippy::cast_precision_loss)] // section counts are small
        let r = hit as f64 / with as f64;
        r
    }

    /// Excerpts that are not a substring of their page's normalized text (should be none).
    pub fn verbatim(&self, doc: &Document) -> Vec<&Excerpt> {
        let opts = flat();
        let mut cache: HashMap<u32, String> = HashMap::new();
        let mut bad = Vec::new();
        for e in self.sections.iter().flat_map(|s| &s.excerpts) {
            let text = cache.entry(e.page).or_insert_with(|| {
                doc.pages
                    .iter()
                    .find(|p| p.index == e.page)
                    .map(|p| normalize(&p.text, &opts).into_owned())
                    .unwrap_or_default()
            });
            if !text.contains(e.text.as_str()) {
                bad.push(e);
            }
        }
        bad
    }
}

/// Normalization of the digest: everything on, whitespace collapsed to single spaces.
fn flat() -> NormalizeOptions {
    NormalizeOptions {
        whitespace: Whitespace::Collapse,
        ..NormalizeOptions::default()
    }
}

/// A paragraph of a section: page position, page index, its text.
struct Unit<'a> {
    page: u32,
    text: &'a str,
}

/// A contiguous range of the document: from `(pos, offset)` up to the next group's start.
#[derive(Clone, Debug)]
struct Group {
    title: String,
    page: u32,
    end_page: u32,
    /// Page position (in `doc.pages`) and byte offset where it starts.
    start: (usize, usize),
    /// Its heading opens its page.
    opens: bool,
    /// Bytes of text in it.
    bytes: usize,
}

/// The digest of `doc` for the card text `card_text` and its `outline`. Stops picking
/// sentences at `deadline` (marking [`Digest::truncated`]).
pub fn build(
    doc: &Document,
    card_text: &str,
    outline: &Outline,
    cfg: &DigestConfig,
    deadline: Option<Instant>,
) -> Digest {
    build_with_terms(doc, card_text, outline, cfg, None, deadline)
}

/// [`build`], with a `keywords: …` line under each section title from the document's term
/// index (and the shared normalized pages, for the keywords' surface forms).
pub fn build_with_terms(
    doc: &Document,
    card_text: &str,
    outline: &Outline,
    cfg: &DigestConfig,
    terms: Option<(&TermIndex, Option<&NormalizedText>)>,
    deadline: Option<Instant>,
) -> Digest {
    let max = cfg.max_tokens.max(300);
    let facts = format!("FACTS:\n{}\n\n", card_text.trim());
    let facts_tokens = tokens::estimate(&facts);
    let doc_bytes: usize = doc.pages.iter().map(|p| p.text.len()).sum();
    let doc_tokens = doc_bytes / 4;
    let room = max.saturating_sub(facts_tokens + 4);

    // 1. The whole text, when it fits.
    if doc_bytes <= room * 8 {
        let opts = flat();
        let full: Vec<String> = doc
            .pages
            .iter()
            .map(|p| normalize(&p.text, &opts).into_owned())
            .filter(|t| !t.is_empty())
            .collect();
        let full = full.join("\n");
        let est = tokens::estimate(&full);
        if est <= room {
            let body = if full.is_empty() {
                String::new()
            } else {
                format!("TEXT:\n{full}")
            };
            return finish(DigestMethod::Full, &facts, &body, est, Vec::new(), false);
        }
    }

    // 2. Outline, start, excerpts.
    let mut body = String::new();
    if outline.method == OutlineMethod::Headings && !outline.entries.is_empty() {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let budget = (max as f32 * cfg.outline_share) as usize;
        let o = outline.render(budget.saturating_sub(12));
        let _ = write!(
            body,
            "OUTLINE ({} of {} headings, {} pages):\n{o}\n\n",
            outline.entries.len(),
            outline.total,
            outline.pages
        );
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let lead_budget = (max as f32 * cfg.lead_share) as usize;
    let lead = lead_text(doc, lead_budget);
    if !lead.is_empty() {
        let _ = write!(body, "START:\n{lead}\n\n");
    }
    let mut back = back_matter(doc);
    let groups = groups(doc, outline, max_groups(cfg, max), &mut back);
    let header = "EXCERPTS (sentences copied from each section):\n";
    let fixed = facts_tokens + tokens::estimate(&body) + tokens::estimate(header);
    let kws: Vec<Vec<String>> = match terms {
        Some((t, shared)) if cfg.section_keywords > 0 => {
            let ranges: Vec<(u32, u32)> = groups.iter().map(|g| (g.page, g.end_page)).collect();
            topics::range_keywords(doc, shared, t, &ranges, cfg.section_keywords)
        }
        _ => Vec::new(),
    };
    let titles: usize = groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            tokens::estimate(&section_title(&g.title, g.page, g.end_page))
                + 1
                + kws.get(i).map_or(0, |k| {
                    keyword_line(k).map_or(0, |l| tokens::estimate(&l) + 1)
                })
        })
        .sum();
    let avail = max.saturating_sub(fixed + titles + 8);
    let scope = Scope {
        back: &back,
        fill: true,
    };
    let (mut sections, truncated) = select(doc, &groups, None, avail, &lead, cfg, scope, deadline);
    for (s, k) in sections.iter_mut().zip(kws) {
        s.keywords = k;
    }
    body.push_str(header);
    let mut digest = render(&facts, &body, &sections, doc_tokens, truncated);
    // The estimate of the parts and of the whole can differ by a few tokens: trim to fit.
    while digest.tokens > max {
        let Some(s) = sections
            .iter_mut()
            .filter(|s| !s.excerpts.is_empty())
            .max_by_key(|s| s.excerpts.len())
        else {
            break;
        };
        s.excerpts.pop();
        digest = render(&facts, &body, &sections, doc_tokens, truncated);
    }
    digest
}

fn max_groups(cfg: &DigestConfig, max: usize) -> usize {
    cfg.max_groups.min(max / 125).max(4)
}

fn finish(
    method: DigestMethod,
    facts: &str,
    body: &str,
    doc_tokens: usize,
    sections: Vec<DigestSection>,
    truncated: bool,
) -> Digest {
    let text = format!("{facts}{body}").trim_end().to_owned();
    Digest {
        method,
        tokens: tokens::estimate(&text),
        body: body.trim_end().to_owned(),
        text,
        doc_tokens,
        sections,
        truncated,
    }
}

/// `keywords: a, b, c`, or nothing without keywords.
fn keyword_line(k: &[String]) -> Option<String> {
    (!k.is_empty()).then(|| format!("keywords: {}", k.join(", ")))
}

fn section_title(title: &str, page: u32, end: u32) -> String {
    format!("## {title} {}", page_range(page, end))
}

fn render(
    facts: &str,
    head: &str,
    sections: &[DigestSection],
    doc_tokens: usize,
    truncated: bool,
) -> Digest {
    let mut body = head.to_owned();
    for s in sections {
        body.push_str(&section_title(&s.title, s.page, s.end_page));
        body.push('\n');
        if let Some(l) = keyword_line(&s.keywords) {
            body.push_str(&l);
            body.push('\n');
        }
        let joined: Vec<&str> = s.excerpts.iter().map(|e| e.text.as_str()).collect();
        if !joined.is_empty() {
            body.push_str(&joined.join(" "));
            body.push('\n');
        }
    }
    finish(
        DigestMethod::Excerpts,
        facts,
        &body,
        doc_tokens,
        sections.to_vec(),
        truncated,
    )
}

/// The first `budget` tokens of the first text pages, normalized.
fn lead_text(doc: &Document, budget: usize) -> String {
    let opts = flat();
    let mut s = String::new();
    for p in &doc.pages {
        let t = normalize(&p.text, &opts);
        if t.is_empty() {
            continue;
        }
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&t);
        if tokens::estimate(&s) >= budget {
            break;
        }
    }
    tokens::head(&s, budget).to_owned()
}

/// Documents shorter than this have no back matter worth skipping (a contract's page that
/// starts with "References" is a clause).
const MIN_BACK_MATTER_PAGES: usize = 20;

/// Titles of back matter, folded: what follows the body of a book or report and says nothing
/// new about it (English, Serbian, Croatian), up to the colophon.
const BACK_TITLES: &[&str] = &[
    "index",
    "subject index",
    "glossary",
    "references",
    "bibliography",
    "endnotes",
    "works cited",
    "indeks",
    "literatura",
    "bibliografija",
    "pojmovnik",
    "about the author",
    "about the authors",
    "colophon",
];

/// Back matter only in a running head (`Notes | 412`), where they are a book's endnotes; a
/// heading "Notes" alone is often the notes to financial statements.
const BACK_RUNNING: &[&str] = &["notes", "napomene"];

/// `line` is a back-matter title; `running` when it is a page's first or last line, where a
/// running head adds a page number and a bar (`Index | 211`, `212 | Index`).
fn back_title(line: &str, running: bool) -> bool {
    let t = line.trim();
    let core = t.trim_matches(|c: char| c.is_ascii_digit() || c.is_whitespace() || c == '|');
    if core.is_empty() || core.len() > 24 {
        return false;
    }
    let f = folded(core);
    let f = f.trim();
    let decorated = running && core.len() < t.len() && t.contains('|');
    BACK_TITLES.contains(&f) || (decorated && BACK_RUNNING.contains(&f))
}

/// An index line: words, then page numbers (`queue handling, 245, 327`,
/// `history of, 101-140`), or a cross-reference (`(see also message brokers)`).
fn index_line(l: &str) -> bool {
    if l.starts_with("(see ") && l.ends_with(')') {
        return true;
    }
    let Some((head, last)) = l.rsplit_once(", ") else {
        return false;
    };
    let page = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
    let last = last.trim_end();
    let pages = match last.split_once(['-', '\u{2013}']) {
        Some((a, b)) => page(a) && page(b),
        None => page(last),
    };
    pages && head.chars().any(char::is_alphabetic)
}

/// A reference-list line: `[12] Author: "Title," …`.
fn reference_line(l: &str) -> bool {
    l.strip_prefix('[')
        .and_then(|r| r.split_once(']'))
        .is_some_and(|(n, rest)| {
            (1..=4).contains(&n.len())
                && n.bytes().all(|b| b.is_ascii_digit())
                && rest.starts_with(' ')
        })
}

/// A page of back matter: its running head or first line names it, or it is an index (40 % of
/// its lines end in page numbers) or a reference list (20 % of its lines open with `[n]`).
fn back_matter_page(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if let (Some(first), Some(last)) = (lines.first(), lines.last())
        && (back_title(first, true) || back_title(last, true))
    {
        return true;
    }
    let n = lines.len();
    if n < 8 {
        return false;
    }
    let index = lines.iter().filter(|l| index_line(l)).count();
    let refs = lines.iter().filter(|l| reference_line(l)).count();
    index * 10 >= n * 4 || (refs >= 3 && refs * 10 >= n * 2)
}

/// Which pages of `doc` (by position) are back matter ([`back_matter_page`]); empty when the
/// document is short or when most of it would be (a bibliography is its own subject).
fn back_matter(doc: &Document) -> Vec<bool> {
    if doc.pages.len() < MIN_BACK_MATTER_PAGES {
        return Vec::new();
    }
    let back: Vec<bool> = doc
        .pages
        .iter()
        .map(|p| back_matter_page(&p.text))
        .collect();
    let with_text = doc
        .pages
        .iter()
        .filter(|p| !p.text.trim().is_empty())
        .count();
    let n_back = back.iter().filter(|&&b| b).count();
    if n_back == 0 || n_back * 2 > with_text {
        return Vec::new();
    }
    back
}

/// Position in `doc.pages` of the page with index `index`.
fn position(doc: &Document, index: u32) -> usize {
    match doc.pages.get(index as usize) {
        Some(p) if p.index == index => index as usize,
        _ => doc
            .pages
            .iter()
            .position(|p| p.index >= index)
            .unwrap_or(doc.pages.len()),
    }
}

/// The sections of the digest: the outline's entries at the deepest level that gives at most
/// `max` of them (one level deeper when that gives fewer than a third of `max`), an opening
/// section before the first heading when it starts after page 1,
/// consecutive sections merged by size when there are still too many.
fn groups(doc: &Document, outline: &Outline, max: usize, back: &mut [bool]) -> Vec<Group> {
    let entries = &outline.entries;
    let max_level = entries.iter().map(|e| e.level).max().unwrap_or(1);
    let count = |l: u8| entries.iter().filter(|e| e.level <= l).count();
    let mut level = 1;
    for l in 1..=max_level {
        if count(l) <= max {
            level = l;
        }
    }
    // Too coarse (a book's few parts): one level deeper, merged by size below.
    if count(level) * 3 < max && level < max_level {
        level += 1;
    }
    let mut gs: Vec<Group> = entries
        .iter()
        .filter(|e| e.level <= level)
        .map(|e| Group {
            title: e.title.clone(),
            page: e.page,
            end_page: e.end_page,
            start: (position(doc, e.page), e.offset as usize),
            opens: e.opens_page,
            bytes: 0,
        })
        .collect();
    let opening = gs.first().is_none_or(|g| g.start.0 > 0);
    if opening && !doc.pages.is_empty() {
        let first = doc.pages.first().map_or(0, |p| p.index);
        let end = gs.first().map_or_else(
            || doc.pages.last().map_or(0, |p| p.index),
            |g| g.page.saturating_sub(1).max(first),
        );
        gs.insert(
            0,
            Group {
                title: "Opening pages".to_owned(),
                page: first,
                end_page: end,
                start: (0, 0),
                opens: true,
                bytes: 0,
            },
        );
    }
    // The first section holds whatever precedes its heading on the first page.
    if let Some(g) = gs.first_mut() {
        g.start = (0, 0);
    }
    let n = doc.pages.len();
    mark_back_sections(&gs, back);
    // Sizes: bytes from each start to the next, back matter left out.
    let mut has_back = vec![false; gs.len()];
    for i in 0..gs.len() {
        let (p0, o0) = gs[i].start;
        let (p1, o1) = gs.get(i + 1).map_or((n, 0), |g| g.start);
        let mut bytes = 0;
        for pos in p0..n.min(p1 + 1) {
            if back.get(pos).copied().unwrap_or(false) {
                has_back[i] = true;
                continue;
            }
            let len = doc.pages[pos].text.len();
            let a = if pos == p0 { o0.min(len) } else { 0 };
            let b = if pos == p1 { o1.min(len) } else { len };
            bytes += b.saturating_sub(a);
        }
        gs[i].bytes = bytes;
    }
    // Sections of back matter only are dropped (the pages stay skipped).
    if has_back.iter().any(|&b| b) && gs.iter().zip(&has_back).any(|(g, &b)| !b || g.bytes > 0) {
        gs = gs
            .into_iter()
            .zip(has_back)
            .filter(|(g, b)| !*b || g.bytes > 0)
            .map(|(g, _)| g)
            .collect();
    }
    if gs.len() > max {
        gs = merge(gs, max);
    }
    // Contiguous page ranges: each section ends where the next one starts (the last one at
    // the last page that is not back matter).
    let last = doc
        .pages
        .iter()
        .enumerate()
        .rev()
        .find(|(pos, _)| !back.get(*pos).copied().unwrap_or(false))
        .or_else(|| doc.pages.last().map(|p| (0, p)))
        .map_or(0, |(_, p)| p.index);
    for i in 0..gs.len() {
        let end = match gs.get(i + 1) {
            Some(n) if n.opens && n.page > gs[i].page => n.page - 1,
            Some(n) => n.page,
            None => last,
        };
        gs[i].end_page = end.max(gs[i].page);
    }
    gs
}

/// A back-matter heading ("Index", "References") makes the pages of its section back matter
/// (when back matter is recognized at all, `back` not empty).
fn mark_back_sections(gs: &[Group], back: &mut [bool]) {
    let n = back.len();
    for (i, g) in gs.iter().enumerate() {
        if g.start != (0, 0) && back_title(&g.title, false) {
            let a = g.start.0 + usize::from(!g.opens);
            let b = gs.get(i + 1).map_or(n, |g| g.start.0);
            for k in back.iter_mut().take(b.min(n)).skip(a) {
                *k = true;
            }
        }
    }
}

/// Merges consecutive sections into `max` groups of similar size.
fn merge(gs: Vec<Group>, max: usize) -> Vec<Group> {
    // Merge consecutive sections into `max` groups of similar size.
    let total: usize = gs.iter().map(|g| g.bytes).sum::<usize>().max(1);
    // Each merged group: (group, its first title, sections merged into it).
    let mut merged: Vec<(Group, String, usize)> = Vec::with_capacity(max);
    let mut acc = 0usize;
    for g in gs {
        let bucket = (acc * max / total).min(max - 1);
        acc += g.bytes;
        if merged.len() > bucket
            && let Some((m, _, more)) = merged.last_mut()
        {
            m.end_page = m.end_page.max(g.end_page);
            m.bytes += g.bytes;
            *more += 1;
        } else {
            let first = g.title.clone();
            merged.push((g, first, 0));
        }
    }
    merged
        .into_iter()
        .map(|(mut g, first, more)| {
            if more > 0 {
                g.title = format!("{first} (+{more} more)");
            }
            g
        })
        .collect()
}

/// The paragraphs of `g` (up to `next`), sampled: at most `max` (the first ones and an even
/// sample of the rest).
#[allow(clippy::many_single_char_names)] // page / offset pairs of a range
fn units<'a>(
    doc: &'a Document,
    g: &Group,
    next: Option<&Group>,
    max: usize,
    max_bytes: usize,
    back: &[bool],
) -> Vec<Unit<'a>> {
    let (p0, o0) = g.start;
    let (p1, o1) = next.map_or((doc.pages.len(), 0), |n| n.start);
    let mut all: Vec<Unit<'a>> = Vec::new();
    for pos in p0..doc.pages.len().min(p1 + 1) {
        if back.get(pos).copied().unwrap_or(false) {
            continue;
        }
        let p = &doc.pages[pos];
        let lo = if pos == p0 { o0 } else { 0 };
        let hi = if pos == p1 { o1 } else { usize::MAX };
        if lo >= hi {
            continue;
        }
        let in_range = |start: usize| start >= lo && start < hi;
        let mut any = false;
        for b in &p.blocks {
            if !matches!(b.kind, BlockKind::Paragraph | BlockKind::ListItem) {
                continue;
            }
            any = true;
            if in_range(b.span.start as usize) {
                let t = b.span.slice(&p.text);
                if !t.trim().is_empty() {
                    all.push(Unit {
                        page: p.index,
                        text: t,
                    });
                }
            }
        }
        if !any {
            // No blocks: the page's text in range, split at blank lines (or as one unit).
            let len = p.text.len();
            let (mut a, mut b) = (lo.min(len), hi.min(len));
            while a < len && !p.text.is_char_boundary(a) {
                a += 1;
            }
            while b < len && !p.text.is_char_boundary(b) {
                b += 1;
            }
            let t = p.text.get(a..b).unwrap_or("");
            for chunk in t.split("\n\n") {
                if !chunk.trim().is_empty() {
                    all.push(Unit {
                        page: p.index,
                        text: chunk,
                    });
                }
            }
        }
    }
    // Long paragraphs: fewer of them, so a section costs about `max_bytes` at most.
    let bytes: usize = all.iter().map(|u| u.text.len()).sum();
    let avg = (bytes / all.len().max(1)).max(1);
    let max = max.min((max_bytes / avg).max(6));
    if all.len() <= max {
        return all;
    }
    let lead = (max / 6).max(1);
    let rest = max - lead;
    let n = all.len() - lead;
    let mut keep = vec![false; all.len()];
    for k in keep.iter_mut().take(lead) {
        *k = true;
    }
    for i in 0..rest {
        keep[lead + i * n / rest] = true;
    }
    all.into_iter()
        .zip(keep)
        .filter_map(|(u, k)| k.then_some(u))
        .collect()
}

/// The sentences of `t` (normalized, one line) as slices: a boundary is `.`, `!` or `?`
/// followed by a space and an upper-case letter, a digit, a quote or a parenthesis, except
/// after an abbreviation, a single letter or a number (as [`crate::extractive`] splits).
fn split_sentences(t: &str) -> Vec<&str> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..b.len() {
        if !matches!(b[i], b'.' | b'!' | b'?') || i < start {
            continue;
        }
        let boundary = match b.get(i + 1) {
            None => true,
            Some(b' ') => t
                .get(i + 2..)
                .and_then(|r| r.chars().next())
                .is_some_and(|a| a.is_uppercase() || a.is_ascii_digit() || a == '"' || a == '('),
            _ => false,
        };
        if !boundary {
            continue;
        }
        if b[i] == b'.' {
            let word = t[start..i]
                .rsplit(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_alphanumeric());
            let lower = word.to_ascii_lowercase();
            if word.chars().count() <= 1
                || word.chars().all(|c| c.is_ascii_digit() || c == '.')
                || ABBREVIATIONS.contains(&lower.as_str())
            {
                continue;
            }
        }
        let s = t[start..=i].trim();
        if !s.is_empty() {
            out.push(s);
        }
        start = i + 1;
    }
    let rest = t[start.min(t.len())..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// A sentence that may start with a clause number or label ([`strip_enumerator`]).
fn numbered_start(s: &str) -> bool {
    let first = s.split(' ').next().unwrap_or("");
    first.starts_with(|c: char| c.is_ascii_digit() || c == '(')
        || matches!(
            first.to_ascii_lowercase().as_str(),
            "exhibit" | "section" | "article" | "clause" | "schedule" | "annex" | "clan"
        )
        || first.eq_ignore_ascii_case("član")
}

/// A candidate sentence.
struct Cand {
    text: String,
    page: u32,
    /// Order in its section.
    order: usize,
    /// Paragraph number in its section.
    unit: usize,
    /// Content-word ids, sorted, unique.
    terms: Vec<u32>,
    tokens: usize,
    /// Multiplier from cues and penalties.
    prior: f64,
}

/// Word ids of one digest build.
#[derive(Default)]
struct Vocab(HashMap<String, u32>);

impl Vocab {
    fn id(&mut self, w: &str) -> u32 {
        if let Some(&i) = self.0.get(w) {
            return i;
        }
        let i = u32::try_from(self.0.len()).unwrap_or(u32::MAX);
        self.0.insert(w.to_owned(), i);
        i
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Content words of a [`folded`] text: at least 3 characters, no stop word, not a number.
fn content_words(f: &str) -> impl Iterator<Item = &str> {
    f.split(' ')
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
}

/// Ids of the content words of a [`folded`] text, sorted, unique.
fn term_ids(f: &str, vocab: &mut Vocab) -> Vec<u32> {
    let mut ts: Vec<u32> = content_words(f).map(|w| vocab.id(w)).collect();
    ts.sort_unstable();
    ts.dedup();
    ts
}

/// A sentence worth an excerpt: 8–60 words, mostly letters, not shouting, no signature line.
fn usable(s: &str) -> bool {
    let n = s.split_whitespace().count();
    if !(8..=60).contains(&n) {
        return false;
    }
    let (mut letters, mut upper, mut total) = (0, 0, 0);
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        total += 1;
        if c.is_alphabetic() {
            letters += 1;
            if c.is_uppercase() {
                upper += 1;
            }
        }
    }
    total > 0
        && letters * 10 >= total * 7
        && upper * 2 <= letters
        && !(s.contains("/s/") || s.contains("By:") || s.contains("Name:"))
        // Table-of-contents leaders and running headers caught inside a paragraph.
        && !(s.contains(". . .") || s.contains("....") || s.contains('|'))
}

/// Cue bonus and penalties of a sentence `s` with phrase flags `fl`.
fn prior(s: &str, fl: Flags) -> f64 {
    let mut p = if fl.cue { 1.3 } else { 1.0 };
    if fl.boilerplate {
        p *= 0.2;
    }
    if fl.definition {
        p *= 0.3;
    }
    if fragment(s) {
        p *= 0.3;
    }
    if fl.contact {
        p *= 0.3;
    }
    p
}

fn candidates(
    units: &[Unit<'_>],
    lead: &str,
    max: usize,
    vocab: &mut Vocab,
    phrases: &PhraseIndex,
) -> Vec<Cand> {
    let opts = flat();
    // Sentences first (cheap checks), then an even sample, then the costly folding.
    let mut found: Vec<(usize, u32, String)> = Vec::new();
    for (u, unit) in units.iter().enumerate() {
        let t = normalize(unit.text, &opts);
        for s in split_sentences(&t) {
            // Whole sentences only: a paragraph cut by a page break ends without a period.
            if !s
                .trim_end_matches(['"', '\'', ')', '\u{201d}'])
                .ends_with(['.', '!', '?'])
            {
                continue;
            }
            let n = s.split_whitespace().count();
            // A lower-case start: the paragraph was cut by a page or column break.
            if !(8..=64).contains(&n) || s.starts_with(char::is_lowercase) {
                continue;
            }
            // Clause numbers and labels ("1.1", "(v)", "Section 2.") are dropped; most
            // sentences have none, and the strip allocates.
            let s = if numbered_start(s) {
                strip_enumerator(s)
            } else {
                s.to_owned()
            };
            if usable(&s) && !lead.contains(s.as_str()) {
                found.push((u, unit.page, s));
            }
        }
    }
    if found.len() > max {
        let n = found.len();
        let mut keep = vec![false; n];
        for i in 0..max {
            keep[i * n / max] = true;
        }
        found = found
            .into_iter()
            .zip(keep)
            .filter_map(|(c, k)| k.then_some(c))
            .collect();
    }
    let mut out = Vec::with_capacity(found.len());
    for (u, page, s) in found {
        let f = folded(&s);
        let fl = phrases.flags(&f);
        if fl.filler {
            continue;
        }
        let terms = term_ids(&f, vocab);
        if terms.len() < 3 {
            continue;
        }
        out.push(Cand {
            tokens: tokens::estimate(&s),
            prior: prior(&s, fl),
            terms,
            text: s,
            page,
            order: out.len(),
            unit: u,
        });
    }
    out
}

/// `|a ∩ b| / |a ∪ b|` of two sorted id lists.
#[allow(clippy::cast_precision_loss)] // term counts are small
fn jaccard(a: &[u32], b: &[u32]) -> f64 {
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - common;
    if union == 0 {
        0.0
    } else {
        common as f64 / union as f64
    }
}

/// Sentences already picked in earlier sections that a new one is compared with.
const REDUNDANCY_WINDOW: usize = 64;
/// A sentence this similar to a picked one (Jaccard of content words) is a repeat.
const REPEAT: f64 = 0.6;

/// Where [`select`] reads and what it may add.
#[derive(Clone, Copy)]
struct Scope<'a> {
    /// Back-matter pages by position ([`back_matter`]), skipped.
    back: &'a [bool],
    /// Budget no sentence used goes to the paragraphs of sections that are mostly not prose
    /// ([`fill_structured`]).
    fill: bool,
}

/// Picks the sentences of every group within `avail` tokens; `after` is the group that
/// follows the last one (the end of its range), if any.
#[allow(clippy::too_many_lines)] // one pass: candidates, frequencies, budgets, picks
#[allow(clippy::too_many_arguments)]
fn select(
    doc: &Document,
    groups: &[Group],
    after: Option<&Group>,
    avail: usize,
    lead: &str,
    cfg: &DigestConfig,
    scope: Scope<'_>,
    deadline: Option<Instant>,
) -> (Vec<DigestSection>, bool) {
    let mut truncated = false;
    let mut vocab = Vocab::default();
    let phrases = PhraseIndex::new();
    // Candidates of every group.
    let mut cands: Vec<Vec<Cand>> = Vec::with_capacity(groups.len());
    let mut all_units: Vec<Vec<Unit<'_>>> = Vec::with_capacity(groups.len());
    for (i, g) in groups.iter().enumerate() {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            truncated = true;
            cands.push(Vec::new());
            all_units.push(Vec::new());
            continue;
        }
        let next = groups.get(i + 1).or(after);
        let us = units(
            doc,
            g,
            next,
            cfg.units_per_group.max(1),
            cfg.bytes_per_group,
            scope.back,
        );
        cands.push(candidates(
            &us,
            lead,
            cfg.candidates_per_group.max(1),
            &mut vocab,
            &phrases,
        ));
        all_units.push(us);
    }
    // Section frequency of every term.
    let mut df = vec![0u32; vocab.len()];
    let mut last_seen = vec![usize::MAX; vocab.len()];
    for (gi, cs) in cands.iter().enumerate() {
        for c in cs {
            for &t in &c.terms {
                let t = t as usize;
                if last_seen[t] != gi {
                    last_seen[t] = gi;
                    df[t] += 1;
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let n_groups = groups.len().max(1) as f64;
    let idf: Vec<f64> = df
        .iter()
        .map(|&d| (1.0 + n_groups / f64::from(d.max(1))).ln())
        .collect();
    // Budgets: a floor each, the rest by size.
    let g = groups.len().max(1);
    let floor = cfg.floor_tokens.min(avail / g);
    let rest = avail.saturating_sub(floor * g);
    let total_bytes: usize = groups.iter().map(|g| g.bytes).sum::<usize>().max(1);
    let budgets: Vec<usize> = groups
        .iter()
        .map(|gr| floor + rest * gr.bytes / total_bytes)
        .collect();

    let mut picked: Vec<Vec<u32>> = Vec::new();
    let mut tf = vec![0u32; vocab.len()];
    let mut sections: Vec<DigestSection> = Vec::with_capacity(groups.len());
    let mut chosen_units: Vec<Vec<usize>> = Vec::with_capacity(groups.len());
    let mut leftover = 0usize;
    for (gi, (gr, cs)) in groups.iter().zip(&cands).enumerate() {
        // Scores: keyword coverage x position prior x cue / penalties.
        for c in cs {
            for &t in &c.terms {
                tf[t as usize] += 1;
            }
        }
        let title = folded(&gr.title);
        let mut title_terms: Vec<u32> = content_words(&title)
            .filter_map(|w| vocab.0.get(w).copied())
            .collect();
        title_terms.sort_unstable();
        #[allow(clippy::cast_precision_loss)]
        let scores: Vec<f64> = cs
            .iter()
            .map(|c| {
                let mut s = 0.0;
                for &t in &c.terms {
                    let rec = (f64::from(tf[t as usize]) - 1.0).min(8.0);
                    let in_title = if title_terms.binary_search(&t).is_ok() {
                        2.0
                    } else {
                        0.0
                    };
                    s += (rec + in_title) * idf[t as usize];
                }
                let s = s / (c.terms.len() as f64).sqrt();
                let pos = 1.0 + 0.5 / (1.0 + c.unit as f64 / 3.0);
                s * pos * c.prior
            })
            .collect();
        for c in cs {
            for &t in &c.terms {
                tf[t as usize] = 0;
            }
        }
        let top = scores.iter().copied().fold(0.0f64, f64::max);
        let budget = budgets[gi] + std::mem::take(&mut leftover);
        // Redundancy against the recent picks of earlier sections, then kept up to date.
        let mut red: Vec<f64> = cs
            .iter()
            .map(|c| {
                picked
                    .iter()
                    .rev()
                    .take(REDUNDANCY_WINDOW)
                    .map(|p| jaccard(&c.terms, p))
                    .fold(0.0f64, f64::max)
            })
            .collect();
        let mut used = 0usize;
        let mut chosen: Vec<usize> = Vec::new();
        let mut taken = vec![false; cs.len()];
        loop {
            let mut best: Option<(usize, f64)> = None;
            for (i, c) in cs.iter().enumerate() {
                if taken[i] || used + c.tokens > budget || red[i] > REPEAT {
                    continue;
                }
                let rel = if top > 0.0 { scores[i] / top } else { 0.0 };
                let mmr = cfg.mmr_lambda * rel - (1.0 - cfg.mmr_lambda) * red[i];
                if best.is_none_or(|(_, b)| mmr > b) {
                    best = Some((i, mmr));
                }
            }
            let Some((i, _)) = best else {
                break;
            };
            taken[i] = true;
            used += cs[i].tokens;
            chosen.push(i);
            for (j, c) in cs.iter().enumerate() {
                if !taken[j] {
                    red[j] = red[j].max(jaccard(&c.terms, &cs[i].terms));
                }
            }
            picked.push(cs[i].terms.clone());
        }
        // Unused budget goes to the next section.
        leftover = budget.saturating_sub(used);
        chosen.sort_by_key(|&i| cs[i].order);
        chosen_units.push(chosen.iter().map(|&i| cs[i].unit).collect());
        sections.push(DigestSection {
            title: gr.title.clone(),
            page: gr.page,
            end_page: gr.end_page,
            size_tokens: gr.bytes / 4,
            budget,
            candidates: cs.len(),
            excerpts: chosen
                .into_iter()
                .map(|i| Excerpt {
                    text: cs[i].text.clone(),
                    page: cs[i].page,
                })
                .collect(),
            keywords: Vec::new(),
        });
    }
    if scope.fill && leftover > 0 {
        let ctx = FillInput {
            doc,
            groups,
            after,
            back: scope.back,
            units: &all_units,
            cands: &cands,
            chosen: &chosen_units,
            budgets: &budgets,
            lead,
        };
        fill_structured(&mut sections, &ctx, leftover);
    }
    (sections, truncated)
}

/// Sections whose candidate sentences hold less than this share of their sampled text are
/// mostly not prose: tables, forms, invoices.
const PROSE_SHARE: f64 = 0.3;
/// Fewest tokens worth a paragraph excerpt of [`fill_structured`].
const MIN_FILL_TOKENS: usize = 12;

/// What [`fill_structured`] reads, per section of [`select`].
struct FillInput<'a, 'd> {
    doc: &'d Document,
    groups: &'a [Group],
    after: Option<&'a Group>,
    back: &'a [bool],
    /// The sampled paragraphs the sentences came from.
    units: &'a [Vec<Unit<'d>>],
    cands: &'a [Vec<Cand>],
    /// Paragraph of each picked sentence, in excerpt order.
    chosen: &'a [Vec<usize>],
    budgets: &'a [usize],
    lead: &'a str,
}

/// Spends `spare` tokens (budget no sentence used) on the sections that are mostly not prose,
/// in proportion to their budgets: their paragraphs in order, copied verbatim (the last one
/// cut at a word), skipping what the start of the text or a picked sentence already shows. An
/// invoice has few sentences; without this its digest would be the start of its first page.
fn fill_structured(sections: &mut [DigestSection], inp: &FillInput<'_, '_>, spare: usize) {
    let opts = flat();
    #[allow(clippy::cast_precision_loss)] // byte counts of a sampled section
    let structured: Vec<bool> = inp
        .units
        .iter()
        .zip(inp.cands)
        .map(|(us, cs)| {
            let text: usize = us.iter().map(|u| u.text.len()).sum();
            let prose: usize = cs.iter().map(|c| c.text.len()).sum();
            text > 0 && (prose as f64) < PROSE_SHARE * text as f64
        })
        .collect();
    let weight: usize = inp
        .budgets
        .iter()
        .zip(&structured)
        .filter_map(|(b, s)| s.then_some(*b))
        .sum();
    if weight == 0 {
        return;
    }
    let mut carry = 0usize;
    for (i, s) in sections.iter_mut().enumerate() {
        if !structured.get(i).copied().unwrap_or(false) {
            continue;
        }
        let (Some(g), Some(sampled)) = (inp.groups.get(i), inp.units.get(i)) else {
            continue;
        };
        let given = spare * inp.budgets.get(i).copied().unwrap_or(0) / weight + carry;
        let mut left = given;
        // Sorted by where the paragraph starts: its page, then its address in the page text.
        let at = |u: &Unit<'_>| (u.page, u.text.as_ptr() as usize);
        let mut all: Vec<((u32, usize), Excerpt)> = inp
            .chosen
            .get(i)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&u| sampled.get(u).map_or((0, 0), at))
            .zip(s.excerpts.drain(..))
            .collect();
        let picked: Vec<String> = all.iter().map(|(_, e)| e.text.clone()).collect();
        // Every paragraph in order, not the sample: a table reads top to bottom.
        let next = inp.groups.get(i + 1).or(inp.after);
        let us = units(inp.doc, g, next, usize::MAX, usize::MAX, inp.back);
        for unit in &us {
            if left < MIN_FILL_TOKENS {
                break;
            }
            let norm = normalize(unit.text, &opts);
            let t = after_lead(norm.trim(), inp.lead);
            if t.is_empty() || inp.lead.contains(t) || picked.iter().any(|p| t.contains(p.as_str()))
            {
                continue;
            }
            let text = tokens::head(t, left);
            if text.is_empty() {
                break;
            }
            left = left.saturating_sub(tokens::estimate(text));
            all.push((
                at(unit),
                Excerpt {
                    text: text.to_owned(),
                    page: unit.page,
                },
            ));
        }
        all.sort_by_key(|(u, _)| *u);
        s.excerpts = all.into_iter().map(|(_, e)| e).collect();
        s.budget += given - left;
        carry = left;
    }
}

/// `t` without the part the start of the text (`lead`, a prefix of the document) already
/// shows: a paragraph that begins inside the lead continues after its end.
fn after_lead<'a>(t: &'a str, lead: &str) -> &'a str {
    let mut k = t.len().min(32);
    while k > 0 && !t.is_char_boundary(k) {
        k -= 1;
    }
    if k == 0 {
        return t;
    }
    let Some(pos) = lead.find(&t[..k]) else {
        return t;
    };
    let overlap = lead.len() - pos;
    match (t.get(..overlap), t.get(overlap..)) {
        (Some(head), Some(rest)) if head == &lead[pos..] => rest.trim_start(),
        _ => t,
    }
}

/// Settings of the hierarchical (map-reduce) summary of very long documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapReduceConfig {
    /// Map calls at most (one per part).
    pub max_parts: usize,
    /// Estimated tokens of each part's input (card excluded).
    pub part_tokens: usize,
    /// Documents with fewer pages get the single call over the digest.
    pub min_pages: u32,
}

impl Default for MapReduceConfig {
    /// 8 parts of 1,500 tokens, from 60 pages.
    fn default() -> Self {
        Self {
            max_parts: 8,
            part_tokens: 1500,
            min_pages: 60,
        }
    }
}

/// One part of a map-reduce summary: contiguous sections and their excerpts.
#[derive(Clone, Debug, PartialEq)]
pub struct MapPart {
    /// `Chapter 3 … Chapter 5` or the one section's title.
    pub title: String,
    /// Zero-based first page.
    pub page: u32,
    /// Zero-based last page.
    pub end_page: u32,
    /// Section titles and excerpts: what the map call reads.
    pub body: String,
    /// Estimated tokens of [`Self::body`].
    pub tokens: usize,
    /// The sections.
    pub sections: Vec<DigestSection>,
    /// The deadline cut its excerpt selection short.
    pub truncated: bool,
}

/// The parts of a map-reduce summary of `doc`: the outline's sections (up to four per part)
/// split into at most `mr.max_parts` contiguous parts of similar size, each with its own
/// excerpts within `mr.part_tokens`. Empty when the document is shorter than
/// `mr.min_pages` pages.
pub fn map_parts(
    doc: &Document,
    outline: &Outline,
    cfg: &DigestConfig,
    mr: &MapReduceConfig,
    deadline: Option<Instant>,
) -> Vec<MapPart> {
    let parts = mr.max_parts.max(1);
    if u32::try_from(doc.pages.len()).unwrap_or(u32::MAX) < mr.min_pages {
        return Vec::new();
    }
    let mut back = back_matter(doc);
    let gs = groups(doc, outline, parts * 4, &mut back);
    let scope = Scope {
        back: &back,
        fill: false,
    };
    if gs.is_empty() {
        return Vec::new();
    }
    // Contiguous buckets of similar size.
    let total: usize = gs.iter().map(|g| g.bytes).sum::<usize>().max(1);
    let mut buckets: Vec<Vec<usize>> = Vec::new();
    let mut acc = 0;
    for (i, g) in gs.iter().enumerate() {
        let b = (acc * parts / total).min(parts - 1);
        acc += g.bytes;
        if buckets.len() <= b {
            buckets.push(Vec::new());
        }
        if let Some(last) = buckets.last_mut() {
            last.push(i);
        }
    }
    let mut out = Vec::new();
    for idx in buckets {
        let (Some(&a), Some(&b)) = (idx.first(), idx.last()) else {
            continue;
        };
        let slice = &gs[a..=b];
        let titles: usize = slice
            .iter()
            .map(|g| tokens::estimate(&section_title(&g.title, g.page, g.end_page)) + 1)
            .sum();
        let avail = mr.part_tokens.saturating_sub(titles + 4);
        let (sections, truncated) =
            select(doc, slice, gs.get(b + 1), avail, "", cfg, scope, deadline);
        let mut body = String::new();
        for s in &sections {
            body.push_str(&section_title(&s.title, s.page, s.end_page));
            body.push('\n');
            let j: Vec<&str> = s.excerpts.iter().map(|e| e.text.as_str()).collect();
            if !j.is_empty() {
                body.push_str(&j.join(" "));
                body.push('\n');
            }
        }
        let first = &gs[a];
        let last = &gs[b];
        let title = if a == b {
            first.title.clone()
        } else {
            format!("{} … {}", first.title, last.title)
        };
        out.push(MapPart {
            title,
            page: first.page,
            end_page: last.end_page.max(first.page),
            tokens: tokens::estimate(&body),
            body: body.trim_end().to_owned(),
            sections,
            truncated,
        });
    }
    out
}

#[cfg(test)]
#[allow(clippy::many_single_char_names)] // short names for test fixtures
mod tests {
    use super::*;
    use crate::outline::{OutlineConfig, outline};
    use quail_core::{Page, PageKind, TextSource};

    /// A synthetic book: `chapters` chapters of `pages` pages, each page with distinct
    /// sentences about the chapter's topic.
    fn book(chapters: usize, pages: usize) -> Document {
        const TOPICS: &[&str] = &[
            "replication",
            "partitioning",
            "transactions",
            "consensus",
            "storage",
            "encoding",
            "batching",
            "streaming",
            "indexing",
            "caching",
            "scheduling",
            "compression",
        ];
        const WORDS: &[&str] = &[
            "amber", "basalt", "cedar", "delta", "ember", "fjord", "garnet", "harbor", "iris",
            "jasper", "kelp", "lagoon",
        ];
        let mut out = Vec::new();
        for c in 0..chapters {
            let topic = TOPICS[c % TOPICS.len()];
            for p in 0..pages {
                let mut t = String::new();
                if p == 0 {
                    let _ = writeln!(t, "CHAPTER {}\nAll About {}", c + 1, capital(topic));
                }
                for (s, w) in WORDS.iter().enumerate() {
                    let v = WORDS[(s + p) % WORDS.len()];
                    let _ = writeln!(
                        t,
                        "The {topic} layer keeps {topic}{w} records near the {v}{topic} gate of \
                         page {p} in a separate {w}{v} queue for later review."
                    );
                }
                let i = u32::try_from(out.len()).unwrap();
                out.push(Page {
                    text: t,
                    text_source: TextSource::Native,
                    ..Page::new(i, PageKind::Physical)
                });
            }
        }
        Document { pages: out }
    }

    fn capital(s: &str) -> String {
        crate::format::capitalize(s)
    }

    #[test]
    fn short_documents_are_whole() {
        let d = book(1, 1);
        let o = outline(&d, &OutlineConfig::default(), None);
        let g = build(&d, "doc: report", &o, &DigestConfig::default(), None);
        assert_eq!(g.method, DigestMethod::Full);
        assert!(
            g.text
                .starts_with("FACTS:\ndoc: report\n\nTEXT:\nCHAPTER 1 All About")
        );
        assert!(g.tokens <= 2000);
    }

    #[test]
    fn long_documents_are_bounded_covered_and_verbatim() {
        let d = book(12, 40); // 480 pages
        let o = outline(&d, &OutlineConfig::default(), None);
        assert_eq!(o.method, OutlineMethod::Headings);
        assert_eq!(o.entries.len(), 12);
        for max in [800, 2000, 3000] {
            let cfg = DigestConfig {
                max_tokens: max,
                ..DigestConfig::default()
            };
            let g = build(&d, "doc: book pages=480", &o, &cfg, None);
            assert_eq!(g.method, DigestMethod::Excerpts);
            assert!(g.tokens <= max, "{} > {max}", g.tokens);
            assert!(g.verbatim(&d).is_empty());
            assert!(g.text.contains("OUTLINE (12 of 12 headings, 480 pages):"));
            if max >= 2000 {
                assert!(
                    g.text
                        .contains("## CHAPTER 7 All About Batching (pp. 241-280)"),
                    "{}",
                    g.text
                );
                assert!((g.coverage() - 1.0).abs() < 1e-9, "{}", g.coverage());
            }
            // Deterministic.
            assert_eq!(g, build(&d, "doc: book pages=480", &o, &cfg, None));
        }
    }

    #[test]
    fn merges_sections_when_there_are_too_many() {
        let d = book(40, 2);
        let o = outline(&d, &OutlineConfig::default(), None);
        let gs = groups(&d, &o, 16, &mut back_matter(&d));
        assert!((12..=16).contains(&gs.len()), "{}", gs.len());
        assert!(gs[0].title.ends_with("more)"), "{}", gs[0].title);
        let total: usize = d.pages.iter().map(|p| p.text.len()).sum();
        assert_eq!(gs.iter().map(|g| g.bytes).sum::<usize>(), total);
    }

    #[test]
    fn sections_carry_keywords_within_the_budget() {
        let d = book(12, 40);
        let o = outline(&d, &OutlineConfig::default(), None);
        let t = crate::terms::build(&d, None, crate::terms::StemLang::English, None);
        let cfg = DigestConfig::default();
        let g = build_with_terms(&d, "doc: book pages=480", &o, &cfg, Some((&t, None)), None);
        assert!(g.tokens <= cfg.max_tokens, "{}", g.tokens);
        assert!(g.verbatim(&d).is_empty());
        let lines = g
            .text
            .lines()
            .filter(|l| l.starts_with("keywords: "))
            .count();
        assert_eq!(lines, g.sections.len(), "{}", g.text);
        // The batching chapter's keywords name its topic.
        let s = g
            .sections
            .iter()
            .find(|s| s.title.contains("Batching"))
            .unwrap();
        assert!(
            s.keywords.iter().any(|k| k.contains("batching")),
            "{:?}",
            s.keywords
        );
        // Without a term index: the digest of `build`.
        assert!(
            !build(&d, "doc: book pages=480", &o, &cfg, None)
                .text
                .contains("keywords: ")
        );
    }

    /// `book(chapters, pages)` followed by `index` pages of an index ("term, 12, 40-44")
    /// with a running foot, then a colophon.
    fn book_with_index(chapters: usize, pages: usize, index: usize) -> Document {
        let mut d = book(chapters, pages);
        for k in 0..index {
            let mut t = String::new();
            for j in 0..30 {
                let _ = writeln!(t, "term{k}x{j} handling, {}, {}-{}", j + 3, j + 40, j + 44);
            }
            let _ = writeln!(t, "Index | {}", 600 + k);
            let i = u32::try_from(d.pages.len()).unwrap();
            d.pages.push(Page {
                text: t,
                text_source: TextSource::Native,
                ..Page::new(i, PageKind::Physical)
            });
        }
        let i = u32::try_from(d.pages.len()).unwrap();
        d.pages.push(Page {
            text: "About the Author\nThe author writes about records and queues for a living."
                .to_owned(),
            text_source: TextSource::Native,
            ..Page::new(i, PageKind::Physical)
        });
        d
    }

    #[test]
    fn back_matter_pages_are_recognized() {
        assert!(back_matter_page("Glossary\nA term.\nAnother term."));
        assert!(back_matter_page("some text\nmore text\n212 | Glossary"));
        assert!(back_matter_page("endnote text\nmore\nNotes | 412"));
        // "Notes" alone heads the notes to financial statements: not back matter.
        assert!(!back_matter_page(
            "Notes\n1. Accounting policies\nThe accounts are prepared."
        ));
        let refs = "[1] A. Author: \"A Paper,\" 2015.\nvenue\n[2] B. Author: \"Another,\" 2016.\n\
                    venue\n[3] C. Author: \"Third,\" 2017.\nvenue\n[4] D. Author, 2018.\nvenue";
        assert!(back_matter_page(refs));
        // Accounts with thousands separators are no index.
        let mut accounts = String::new();
        for i in 0..12 {
            let _ = writeln!(accounts, "Cash at bank {i},234 5,{i}78");
        }
        assert!(!back_matter_page(&accounts));
        assert!(index_line("queue handling, 245, 327"));
        assert!(index_line("history of, 101-140"));
        assert!(index_line("(see also message brokers)"));
        assert!(!index_line("Belgrade, 11000"));
        // Short documents are left alone.
        assert!(back_matter(&book_with_index(2, 3, 2)).is_empty());
    }

    #[test]
    fn back_matter_is_skipped_in_digest_and_parts() {
        let d = book_with_index(12, 6, 8);
        let n = d.pages.len();
        let back = back_matter(&d);
        assert_eq!(back.iter().filter(|&&b| b).count(), 9, "{back:?}");
        let o = outline(&d, &OutlineConfig::default(), None);
        let cfg = DigestConfig::default();
        let g = build(&d, "doc: book", &o, &cfg, None);
        let body_end = u32::try_from(n - 10).unwrap();
        for s in &g.sections {
            assert!(s.end_page <= body_end, "{} {}", s.title, s.end_page);
            for e in &s.excerpts {
                assert!(e.page <= body_end && !e.text.contains("term"), "{e:?}");
            }
        }
        assert!(g.verbatim(&d).is_empty());
        let mr = MapReduceConfig {
            min_pages: 20,
            ..MapReduceConfig::default()
        };
        let parts = map_parts(&d, &o, &cfg, &mr, None);
        assert!(!parts.is_empty());
        assert!(parts.iter().all(|p| !p.body.contains("term")));
        assert_eq!(parts.last().map(|p| p.end_page), Some(body_end));
    }

    #[test]
    fn structured_text_fills_the_unused_budget() {
        // An invoice: a header block and table rows, no sentence a model could be given.
        let mut pages = Vec::new();
        for p in 0..6u32 {
            let mut t = format!(
                "INVOICE\nPage {} of 6\nAdvertiser Northwind Foods\nInvoice Period 03/27/15 - \
                 04/19/15\nStation KXYZ\n",
                p + 1
            );
            for r in 0..40 {
                let _ = writeln!(
                    t,
                    "{} KXYZ Evening News M-F 5p-6p 05/{:02}/14 :30 $1,{r}00.00",
                    p * 40 + r,
                    r % 28 + 1
                );
            }
            pages.push(Page {
                text: t,
                text_source: TextSource::Native,
                ..Page::new(p, PageKind::Physical)
            });
        }
        let d = Document { pages };
        let o = outline(&d, &OutlineConfig::default(), None);
        let cfg = DigestConfig {
            max_tokens: 1600,
            ..DigestConfig::default()
        };
        let g = build(&d, "doc: invoice", &o, &cfg, None);
        assert_eq!(g.method, DigestMethod::Excerpts);
        assert!(g.tokens > 1200 && g.tokens <= 1600, "{}", g.tokens);
        assert!(g.body.contains("Evening News"), "{}", g.body);
        assert!(g.verbatim(&d).is_empty());
        // Prose leaves its budget to sentences: the book's digest has no paragraph fill.
        let b = book(12, 10);
        let bo = outline(&b, &OutlineConfig::default(), None);
        let bg = build(&b, "doc: book", &bo, &cfg, None);
        assert!(
            bg.sections
                .iter()
                .flat_map(|s| &s.excerpts)
                .all(|e| e.text.ends_with('.'))
        );
    }

    #[test]
    fn map_parts_are_bounded() {
        let d = book(12, 10);
        let o = outline(&d, &OutlineConfig::default(), None);
        let mr = MapReduceConfig::default();
        let parts = map_parts(&d, &o, &DigestConfig::default(), &mr, None);
        assert!(!parts.is_empty() && parts.len() <= 8, "{}", parts.len());
        for p in &parts {
            assert!(p.tokens <= mr.part_tokens, "{}", p.tokens);
            assert!(!p.body.is_empty());
        }
        assert_eq!(parts[0].page, 0);
        assert_eq!(parts.last().map(|p| p.end_page), Some(119));
        let short = MapReduceConfig {
            min_pages: 500,
            ..mr
        };
        assert!(map_parts(&d, &o, &DigestConfig::default(), &short, None).is_empty());
    }

    #[test]
    fn map_parts_mark_a_deadline_cut() {
        let d = book(12, 10);
        let o = outline(&d, &OutlineConfig::default(), None);
        let mr = MapReduceConfig::default();
        let cfg = DigestConfig::default();
        assert!(
            map_parts(&d, &o, &cfg, &mr, None)
                .iter()
                .all(|p| !p.truncated)
        );
        let past = Instant::now().checked_sub(std::time::Duration::from_secs(1));
        let cut = map_parts(&d, &o, &cfg, &mr, past);
        assert!(!cut.is_empty() && cut.iter().any(|p| p.truncated));
    }

    #[test]
    fn redundant_sentences_are_picked_once() {
        // Every page repeats the same clause; one copy at most.
        let pages: Vec<Page> = (0..30)
            .map(|i| Page {
                text: format!(
                    "Section {} Topic\nThe parties keep this clause identical on every single page of \
                     the file. Page {i} adds its own remark about rivers lakes and mountains number {i}.",
                    i + 1
                ),
                text_source: TextSource::Native,
                ..Page::new(i, PageKind::Physical)
            })
            .collect();
        let d = Document { pages };
        let o = outline(&d, &OutlineConfig::default(), None);
        let cfg = DigestConfig {
            max_tokens: 600,
            ..DigestConfig::default()
        };
        let g = build(&d, "doc: x", &o, &cfg, None);
        let excerpts = g.text.split("EXCERPTS").nth(1).unwrap_or("");
        let n = excerpts.matches("keep this clause identical").count();
        assert!(n <= 1, "{n}\n{}", g.text);
    }
}

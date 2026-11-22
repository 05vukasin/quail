//! The topic map of a long document (`summary.topics@1`): keywords per section, where the
//! subject changes, and the sections grouped into topic segments. No model, linear time.
//!
//! # Sections
//!
//! The outline's headings at the deepest level that gives at most
//! [`TopicConfig::max_sections`] of them (headings that start on the same page are one
//! section), plus `Opening pages` before the first heading; without at least 3 headings,
//! fixed page windows (at most [`TopicConfig::windows`]). A section's text is its pages,
//! from its heading's page to the page before the next heading (that page too when the next
//! heading does not open it).
//!
//! # Keywords and shifts
//!
//! A section's term vector counts the 1–2 word phrases of its pages
//! ([`TermIndex::pages`]); weights are `(1 + ln tf) · ln((N + 1) / df)` over the `N` sections
//! (a phrase found in every section weighs almost nothing), generic words ×0.3, two-word phrases
//! ×1.3. Section keywords are the top [`TopicConfig::section_keywords`] (a word that occurs
//! mostly inside a picked phrase is skipped). The **shift** of a section is the cosine
//! distance (1 − cos) of its single-word vector to the previous section's: 0 same subject, 1
//! nothing in common.
//!
//! # Segments
//!
//! Consecutive sections are grouped while they stay close to the running segment: a section
//! whose distance to the segment's summed vector is at least the threshold starts a new
//! segment. The threshold is `max(min_shift, median + 0.5 · (p90 − median))` of those
//! distances, so a document that changes subject everywhere is not cut at every section.
//! At most [`TopicConfig::max_segments`] segments (the largest distances win). Segment
//! keywords: the top [`TopicConfig::segment_keywords`] of the segment's vector, weighted by
//! the section frequencies.

use quail_core::{Document, Value};
use quail_text::NormalizedText;

use crate::outline::{Outline, OutlineMethod, page_range};
use crate::terms::{IdMap, TermIndex};

/// Settings of [`map`].
#[derive(Clone, Copy, Debug)]
pub struct TopicConfig {
    /// Documents with fewer pages get no topic map.
    pub min_pages: u32,
    /// Sections at most.
    pub max_sections: usize,
    /// Segments at most.
    pub max_segments: usize,
    /// Page windows at most when the document has no headings.
    pub windows: usize,
    /// Keywords per section.
    pub section_keywords: usize,
    /// Keywords per segment.
    pub segment_keywords: usize,
    /// Lowest distance that starts a segment.
    pub min_shift: f64,
}

impl Default for TopicConfig {
    fn default() -> Self {
        Self {
            min_pages: 20,
            max_sections: 64,
            max_segments: 64,
            windows: 32,
            section_keywords: 5,
            segment_keywords: 8,
            min_shift: 0.6,
        }
    }
}

/// One section of the map.
#[derive(Clone, Debug, PartialEq)]
pub struct TopicSection {
    /// Its heading (or `Pages 4-9`, `Opening pages`).
    pub title: String,
    /// Zero-based first page.
    pub page: u32,
    /// Zero-based last page.
    pub end_page: u32,
    /// Its distinctive keywords, best first.
    pub keywords: Vec<String>,
    /// Cosine distance to the previous section (0 for the first).
    pub shift: f64,
}

/// A run of sections about one subject.
#[derive(Clone, Debug, PartialEq)]
pub struct TopicSegment {
    /// Titles of its first sections (at most 4).
    pub titles: Vec<String>,
    /// Sections in it.
    pub sections: usize,
    /// Zero-based first page.
    pub page: u32,
    /// Zero-based last page.
    pub end_page: u32,
    /// Its keywords, best first.
    pub keywords: Vec<String>,
    /// Distance of its first section to the previous segment (0 for the first).
    pub shift: f64,
}

/// The topic map of a document.
#[derive(Clone, Debug, PartialEq)]
pub struct TopicMap {
    /// `headings` or `windows`.
    pub method: OutlineMethod,
    /// The sections, in order.
    pub sections: Vec<TopicSection>,
    /// The segments, in order.
    pub segments: Vec<TopicSegment>,
    /// The distance that started a segment.
    pub threshold: f64,
    /// The term index or the outline was cut by the deadline.
    pub truncated: bool,
}

impl TopicMap {
    /// The record stored in `summary.topics`.
    pub fn to_value(&self) -> Value {
        let words = |k: &[String]| Value::List(k.iter().map(|s| Value::text(s.as_str())).collect());
        let sections = self
            .sections
            .iter()
            .map(|s| {
                Value::record([
                    ("title", Value::text(s.title.as_str())),
                    ("page", Value::Int(i64::from(s.page))),
                    ("end_page", Value::Int(i64::from(s.end_page))),
                    ("keywords", words(&s.keywords)),
                    ("shift", Value::Float(round(s.shift))),
                ])
            })
            .collect();
        let segments = self
            .segments
            .iter()
            .map(|s| {
                Value::record([
                    (
                        "title",
                        Value::text(s.titles.first().map_or("", String::as_str)),
                    ),
                    ("titles", words(&s.titles)),
                    (
                        "sections",
                        Value::Int(i64::try_from(s.sections).unwrap_or(i64::MAX)),
                    ),
                    ("page", Value::Int(i64::from(s.page))),
                    ("end_page", Value::Int(i64::from(s.end_page))),
                    ("keywords", words(&s.keywords)),
                    ("shift", Value::Float(round(s.shift))),
                ])
            })
            .collect();
        let mut f = vec![
            ("method", Value::text(self.method.as_str())),
            ("threshold", Value::Float(round(self.threshold))),
            ("sections", Value::List(sections)),
            ("segments", Value::List(segments)),
        ];
        if self.truncated {
            f.push(("truncated", Value::Bool(true)));
        }
        Value::record(f)
    }
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// A section before keywords: title and page range.
#[derive(Clone, Debug)]
struct Span {
    title: String,
    page: u32,
    end: u32,
}

/// The sections of `doc` from its `outline` (see the [module docs](self)).
fn spans(doc: &Document, outline: &Outline, cfg: &TopicConfig) -> (OutlineMethod, Vec<Span>) {
    let first = doc.pages.first().map_or(0, |p| p.index);
    let last = doc.pages.last().map_or(0, |p| p.index);
    if outline.method == OutlineMethod::Headings && !outline.entries.is_empty() {
        let es = &outline.entries;
        let max_level = es.iter().map(|e| e.level).max().unwrap_or(1);
        let count = |l: u8| {
            let mut pages: Vec<u32> = es.iter().filter(|e| e.level <= l).map(|e| e.page).collect();
            pages.dedup();
            pages.len()
        };
        let mut level = 1;
        for l in 1..=max_level {
            if count(l) <= cfg.max_sections {
                level = l;
            }
        }
        // (title, page, next opens its page)
        let mut heads: Vec<(String, u32, bool)> = Vec::new();
        for e in es.iter().filter(|e| e.level <= level) {
            if heads.last().is_some_and(|h| h.1 == e.page) {
                continue;
            }
            heads.push((e.title.clone(), e.page, e.opens_page));
        }
        if heads.len() >= 3 {
            let mut out = Vec::new();
            if heads[0].1 > first {
                let end = if heads[0].2 {
                    heads[0].1 - 1
                } else {
                    heads[0].1
                };
                out.push(Span {
                    title: "Opening pages".to_owned(),
                    page: first,
                    end: end.max(first),
                });
            }
            for (i, (title, page, _)) in heads.iter().enumerate() {
                let end = match heads.get(i + 1) {
                    Some((_, np, opens)) if *opens && *np > *page => np - 1,
                    Some((_, np, _)) => *np,
                    None => last,
                };
                out.push(Span {
                    title: title.clone(),
                    page: *page,
                    end: end.max(*page),
                });
            }
            out.truncate(cfg.max_sections);
            if let Some(l) = out.last_mut() {
                l.end = l.end.max(last);
            }
            return (OutlineMethod::Headings, out);
        }
    }
    let n = doc.pages.len();
    let size = n.div_ceil(cfg.windows.max(1)).max(1);
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
        out.push(Span {
            title,
            page,
            end: end_page,
        });
        start = end + 1;
    }
    (OutlineMethod::Windows, out)
}

/// Sparse vector: phrase id → (occurrences, pages it occurs on).
type Vector = IdMap<u32, (u32, u32)>;

/// Adds `b` to `a`.
fn add(a: &mut Vector, b: &Vector) {
    for (&id, &(c, p)) in b {
        let e = a.entry(id).or_insert((0, 0));
        e.0 += c;
        e.1 += p;
    }
}

/// The phrase counts of the pages `page..=end` of `t`.
fn vector(t: &TermIndex, page: u32, end: u32) -> Vector {
    let a = t.pages.partition_point(|(i, _)| *i < page);
    let mut v = Vector::default();
    for (i, counts) in t.pages.get(a..).unwrap_or(&[]) {
        if *i > end {
            break;
        }
        for &(id, c) in counts {
            let e = v.entry(id).or_insert((0, 0));
            e.0 += c;
            e.1 += 1;
        }
    }
    v
}

/// Phrase id → number of vectors it occurs in.
fn doc_freq(vs: &[Vector]) -> IdMap<u32, u32> {
    let mut df: IdMap<u32, u32> = IdMap::default();
    for v in vs {
        for &id in v.keys() {
            *df.entry(id).or_insert(0) += 1;
        }
    }
    df
}

fn idf(df: &IdMap<u32, u32>, n: usize, id: u32) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let n = n as f64;
    ((n + 1.0) / f64::from(df.get(&id).copied().unwrap_or(1).max(1))).ln()
}

/// The single-word part of `v`, weighted, for cosine distances.
fn weighted_words(t: &TermIndex, v: &Vector, df: &IdMap<u32, u32>, n: usize) -> Vec<(u32, f64)> {
    let mut out: Vec<(u32, f64)> = v
        .iter()
        .filter(|(id, _)| t.phrases.get(**id as usize).is_some_and(|p| p.n == 1))
        .map(|(&id, &(c, _))| (id, (1.0 + f64::from(c).ln()) * idf(df, n, id)))
        .collect();
    out.sort_unstable_by_key(|x| x.0);
    out
}

/// Cosine distance of two sorted weighted vectors (1 when either is empty).
fn distance(a: &[(u32, f64)], b: &[(u32, f64)]) -> f64 {
    let na: f64 = a.iter().map(|x| x.1 * x.1).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x.1 * x.1).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 1.0;
    }
    let (mut i, mut j, mut dot) = (0, 0, 0.0);
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                dot += a[i].1 * b[j].1;
                i += 1;
                j += 1;
            }
        }
    }
    (1.0 - dot / (na * nb)).clamp(0.0, 1.0)
}

/// The top `k` phrase ids of `v` (see the [module docs](self)).
fn top(t: &TermIndex, v: &Vector, df: &IdMap<u32, u32>, n: usize, k: usize) -> Vec<u32> {
    let total: u32 = v.values().map(|x| x.0).sum();
    let min_tf = if total >= 400 { 2 } else { 1 };
    let mut scored: Vec<(u32, f64)> = v
        .iter()
        .filter_map(|(&id, &(c, pf))| {
            let p = t.phrases.get(id as usize)?;
            if c < min_tf || p.count < 2 {
                return None;
            }
            let ws = p.word_ids();
            let generic = ws.iter().filter(|&&w| t.words[w as usize].generic).count();
            if generic == ws.len() && p.n >= 2 {
                return None;
            }
            // Spread over the section's pages counts more than repetition on one page (a
            // worked example repeats its words; the subject runs through the section).
            let mut w =
                (1.0 + f64::from(pf).ln()) * (1.0 + 0.5 * f64::from(c).ln()) * idf(df, n, id);
            if generic == ws.len() {
                w *= 0.3;
            }
            if p.n == 2 {
                w *= 1.3;
            }
            Some((id, w))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out: Vec<u32> = Vec::new();
    for (id, _) in scored {
        if out.len() >= k {
            break;
        }
        let p = &t.phrases[id as usize];
        let c = f64::from(v.get(&id).map_or(0, |x| x.0));
        let redundant = out.iter().any(|&o| {
            let q = &t.phrases[o as usize];
            let qc = f64::from(v.get(&o).map_or(0, |x| x.0));
            (p.n == 1 && q.word_ids().contains(&p.words[0]) && c <= 1.3 * qc)
                || (q.n == 1 && p.word_ids().contains(&q.words[0]) && qc <= 1.3 * c)
        });
        if !redundant {
            out.push(id);
        }
    }
    out
}

/// Keywords of each page range of `ranges` (zero-based, inclusive), distinctive against the
/// other ranges: what the digest prints under each section.
pub fn range_keywords(
    doc: &Document,
    shared: Option<&NormalizedText>,
    t: &TermIndex,
    ranges: &[(u32, u32)],
    k: usize,
) -> Vec<Vec<String>> {
    let vs: Vec<Vector> = ranges.iter().map(|&(a, b)| vector(t, a, b)).collect();
    let df = doc_freq(&vs);
    let ids: Vec<Vec<u32>> = vs.iter().map(|v| top(t, v, &df, vs.len(), k)).collect();
    let all: Vec<u32> = ids.iter().flatten().copied().collect();
    let names = t.surfaces(doc, shared, &all);
    ids.iter()
        .map(|l| l.iter().filter_map(|id| names.get(id).cloned()).collect())
        .collect()
}

/// The topic map of `doc` from its term index `t` and `outline`, or `None` for a document
/// of fewer than [`TopicConfig::min_pages`] pages or fewer than 2 sections.
pub fn map(
    doc: &Document,
    shared: Option<&NormalizedText>,
    t: &TermIndex,
    outline: &Outline,
    cfg: &TopicConfig,
) -> Option<TopicMap> {
    let pages = u32::try_from(doc.pages.len()).unwrap_or(u32::MAX);
    if pages < cfg.min_pages.max(2) {
        return None;
    }
    let (method, spans) = spans(doc, outline, cfg);
    if spans.len() < 2 {
        return None;
    }
    let vs: Vec<Vector> = spans.iter().map(|s| vector(t, s.page, s.end)).collect();
    let n = vs.len();
    let df = doc_freq(&vs);
    let wv: Vec<Vec<(u32, f64)>> = vs.iter().map(|v| weighted_words(t, v, &df, n)).collect();
    let shifts: Vec<f64> = (0..n)
        .map(|i| {
            if i == 0 {
                0.0
            } else {
                distance(&wv[i - 1], &wv[i])
            }
        })
        .collect();

    // Segments: distance of each section to the running segment.
    let mut dist = vec![0.0; n];
    let mut acc: Vector = vs[0].clone();
    let mut starts = vec![0usize];
    // First pass with the floor only, to learn the distances; then the adaptive threshold.
    for (i, v) in vs.iter().enumerate().skip(1) {
        let d = distance(&weighted_words(t, &acc, &df, n), &wv[i]);
        dist[i] = d;
        if d >= cfg.min_shift {
            acc.clear();
        }
        add(&mut acc, v);
    }
    let mut ds: Vec<f64> = dist[1..].to_vec();
    ds.sort_by(f64::total_cmp);
    let q = |f: f64| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let i = ((ds.len().saturating_sub(1)) as f64 * f).round() as usize;
        ds.get(i).copied().unwrap_or(1.0)
    };
    let median = q(0.5);
    let threshold = cfg.min_shift.max(median + 0.5 * (q(0.9) - median));
    let mut acc: Vector = vs[0].clone();
    for (i, v) in vs.iter().enumerate().skip(1) {
        let d = distance(&weighted_words(t, &acc, &df, n), &wv[i]);
        dist[i] = d;
        if d >= threshold {
            starts.push(i);
            acc.clear();
        }
        add(&mut acc, v);
    }
    if starts.len() > cfg.max_segments.max(1) {
        let mut by: Vec<usize> = starts[1..].to_vec();
        by.sort_by(|&a, &b| dist[b].total_cmp(&dist[a]).then(a.cmp(&b)));
        by.truncate(cfg.max_segments.max(1) - 1);
        by.sort_unstable();
        starts = std::iter::once(0).chain(by).collect();
    }

    let sec_ids: Vec<Vec<u32>> = vs
        .iter()
        .map(|v| top(t, v, &df, n, cfg.section_keywords))
        .collect();
    let mut seg_ids: Vec<Vec<u32>> = Vec::new();
    for (j, &a) in starts.iter().enumerate() {
        let b = starts.get(j + 1).copied().unwrap_or(n);
        let mut v = Vector::default();
        for x in &vs[a..b] {
            add(&mut v, x);
        }
        seg_ids.push(top(t, &v, &df, n, cfg.segment_keywords));
    }
    let all: Vec<u32> = sec_ids.iter().chain(&seg_ids).flatten().copied().collect();
    let names = t.surfaces(doc, shared, &all);
    let named = |l: &[u32]| {
        l.iter()
            .filter_map(|id| names.get(id).cloned())
            .collect::<Vec<_>>()
    };

    let sections: Vec<TopicSection> = spans
        .iter()
        .zip(&sec_ids)
        .zip(&shifts)
        .map(|((s, ids), &shift)| TopicSection {
            title: s.title.clone(),
            page: s.page,
            end_page: s.end,
            keywords: named(ids),
            shift,
        })
        .collect();
    let segments = starts
        .iter()
        .enumerate()
        .map(|(j, &a)| {
            let b = starts.get(j + 1).copied().unwrap_or(n);
            TopicSegment {
                titles: spans[a..b]
                    .iter()
                    .take(4)
                    .map(|s| s.title.clone())
                    .collect(),
                sections: b - a,
                page: spans[a].page,
                end_page: spans[b - 1].end,
                keywords: named(&seg_ids[j]),
                shift: if a == 0 { 0.0 } else { dist[a] },
            }
        })
        .collect();
    Some(TopicMap {
        method,
        sections,
        segments,
        threshold,
        truncated: t.truncated || outline.truncated,
    })
}

/// The map as text lines, one per segment: `- Title (pp. 3-40): kw, kw, kw`.
pub fn render(m: &TopicMap) -> String {
    m.segments
        .iter()
        .map(|s| {
            format!(
                "- {} {}: {}",
                s.titles.first().map_or("", String::as_str),
                page_range(s.page, s.end_page),
                s.keywords.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline::{OutlineConfig, outline};
    use crate::terms::{StemLang, build};
    use quail_core::{Page, PageKind, TextSource};
    use std::fmt::Write as _;

    /// A synthetic report: `parts` parts about distinct subjects, each of `chapters` chapters
    /// of `pages` pages; chapters of one part share its vocabulary.
    fn report(parts: &[&[&str]], chapters: usize, pages: usize) -> Document {
        let mut out = Vec::new();
        let mut ch = 0;
        for words in parts {
            for c in 0..chapters {
                ch += 1;
                for p in 0..pages {
                    let mut t = String::new();
                    if p == 0 {
                        let _ = writeln!(t, "CHAPTER {ch}\nNotes on {} {}", words[0], c + 1);
                    }
                    for s in 0..14 {
                        let a = words[(s + p) % words.len()];
                        let b = words[(s + c + 1) % words.len()];
                        let _ =
                            writeln!(t, "The {a} {b} keeps the {b} near every {a} of the site.");
                    }
                    let i = u32::try_from(out.len()).unwrap();
                    out.push(Page {
                        text: t,
                        text_source: TextSource::Native,
                        ..Page::new(i, PageKind::Physical)
                    });
                }
            }
        }
        Document { pages: out }
    }

    const REEF: &[&str] = &["coral", "reef", "lagoon", "algae", "sponge", "tide"];
    const MINE: &[&str] = &["ore", "shaft", "drill", "tunnel", "quarry", "basalt"];
    const BANK: &[&str] = &["ledger", "loan", "deposit", "credit", "teller", "vault"];

    fn map_of(d: &Document) -> TopicMap {
        let o = outline(d, &OutlineConfig::default(), None);
        let t = build(d, None, StemLang::English, None);
        map(d, None, &t, &o, &TopicConfig::default()).unwrap()
    }

    #[test]
    fn segments_follow_the_subjects() {
        let d = report(&[REEF, MINE, BANK], 3, 4); // 36 pages, 9 chapters
        let m = map_of(&d);
        assert_eq!(m.method, OutlineMethod::Headings);
        assert_eq!(m.sections.len(), 9);
        let starts: Vec<u32> = m.segments.iter().map(|s| s.page).collect();
        assert_eq!(starts, vec![0, 12, 24], "{m:#?}");
        assert!(
            m.segments[0]
                .keywords
                .iter()
                .any(|k| k.split(' ').any(|w| REEF.contains(&w))),
            "{m:#?}"
        );
        assert!(
            m.segments[1]
                .keywords
                .iter()
                .any(|k| k.split(' ').any(|w| MINE.contains(&w)))
        );
        // Within a subject the shift is small, across one large.
        assert!(m.sections[1].shift < m.sections[3].shift, "{m:#?}");
        assert_eq!(m.segments[1].sections, 3);
        assert_eq!(m.segments[2].end_page, 35);
        // Deterministic.
        assert_eq!(m, map_of(&d));
        let v = m.to_value();
        assert!(v.field("segments").is_some());
        assert!(render(&m).starts_with("- CHAPTER 1"), "{}", render(&m));
    }

    #[test]
    fn windows_without_headings_and_bounds() {
        let pages: Vec<Page> = (0..100u32)
            .map(|i| {
                let w = if i < 52 { REEF } else { BANK };
                let mut t = String::new();
                for s in 0..10 {
                    let _ = writeln!(
                        t,
                        "the {} sits by the {} today.",
                        w[s % 6],
                        w[(s + i as usize) % 6]
                    );
                }
                Page {
                    text: t,
                    text_source: TextSource::Native,
                    ..Page::new(i, PageKind::Physical)
                }
            })
            .collect();
        let d = Document { pages };
        let m = map_of(&d);
        assert_eq!(m.method, OutlineMethod::Windows);
        assert!(m.sections.len() <= 32);
        assert!(m.segments.len() >= 2 && m.segments.len() <= 64, "{m:#?}");
        assert!(m.segments.iter().any(|s| s.page == 52), "{m:#?}");
        // Short documents get no map.
        let short = report(&[REEF], 1, 3);
        let o = outline(&short, &OutlineConfig::default(), None);
        let t = build(&short, None, StemLang::English, None);
        assert!(map(&short, None, &t, &o, &TopicConfig::default()).is_none());
    }

    #[test]
    fn segment_cap_keeps_the_largest_shifts() {
        let d = report(&[REEF, MINE, BANK], 3, 4);
        let o = outline(&d, &OutlineConfig::default(), None);
        let t = build(&d, None, StemLang::English, None);
        let cfg = TopicConfig {
            max_segments: 2,
            ..TopicConfig::default()
        };
        let m = map(&d, None, &t, &o, &cfg).unwrap();
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.segments[0].page, 0);
        let rk = range_keywords(&d, None, &t, &[(0, 11), (12, 23)], 4);
        assert!(rk[0].iter().any(|k| k.split(' ').any(|w| REEF.contains(&w))), "{rk:?}");
        assert!(rk[1].iter().any(|k| k.split(' ').any(|w| MINE.contains(&w))), "{rk:?}");
    }
}

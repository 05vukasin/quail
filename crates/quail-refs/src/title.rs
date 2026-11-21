//! The document title: a heading or title-zone block, else a title line (a kind phrase near
//! its start, often in capitals), else the largest line, else a short upper-case line; then the
//! instrument kind and ordinal it names.

use quail_core::{BBox, BlockKind};

use crate::kind::Kind;
use crate::patterns::{Patterns, ordinal_word, roman};
use crate::prep::Prepared;
use crate::scan::is_label;

/// Lines of the first page looked at.
const TITLE_LINES: u32 = 25;
const MAX_TITLE_TOKENS: usize = 16;
const MAX_TITLE_CHARS: usize = 200;
/// A title line this good ends the search.
const GOOD: f32 = 0.7;

const PROSE_START: &[&str] = &[
    "this", "these", "ovaj", "ovim", "whereas", "pursuant", "the", "dated", "between", "by", "u",
    "na", "in", "if", "for", "and", "of", "to", "with", "from", "as", "it", "we", "our", "dear",
    "sa", "od", "po", "kao", "i",
];
const PROSE_WORDS: &[&str] = &[
    "is",
    "are",
    "made",
    "entered",
    "shall",
    "hereby",
    "will",
    "zakljucen",
    "zakljucuju",
    "zakljucili",
    "izmedju",
    "between",
    "sklopljen",
    "have",
    "has",
    "been",
    "omitted",
    "redacted",
    "requested",
    "agree",
    "agrees",
    "requiring",
    "must",
];
const NOT_TITLES: &[&str] = &[
    "table of contents",
    "contents",
    "sadrzaj",
    "confidential",
    "strictly confidential",
    "poverljivo",
    "execution version",
    "execution copy",
    "draft",
    "nacrt",
    "final",
];
const JOIN_AFTER: &[&str] = &["o", "of", "for", "za", "and", "i", "na", "to", "uz", "&"];
const JOIN_BEFORE: &[&str] = &["o", "of", "for", "za", "to", "uz", "na", "under"];

/// The title found.
#[derive(Clone, Debug)]
pub(crate) struct Title {
    pub text: String,
    /// Page index into the prepared pages and token range.
    pub page: usize,
    pub a: usize,
    pub b: usize,
    pub conf: f32,
    pub kind: Option<(Kind, f32)>,
    pub ordinal: Option<(i64, usize, usize)>,
}

#[derive(Clone, Copy)]
struct Cand {
    a: usize,
    b: usize,
    score: f32,
    kind_at: Option<usize>,
}

/// Finds the title on the first page with text (page 0, or 1 when page 0 is a near-empty
/// cover).
pub(crate) fn find(p: &Patterns, pages: &[Prepared<'_>]) -> Option<Title> {
    for (pi, pp) in pages.iter().enumerate().take(2) {
        if pp.toks.len() < 2 && pi == 0 && pages.len() > 1 {
            continue;
        }
        let t = find_on(p, pp, pi);
        if t.is_some() || pp.toks.len() >= 8 {
            return t;
        }
    }
    None
}

fn find_on(p: &Patterns, pp: &Prepared<'_>, pi: usize) -> Option<Title> {
    let lines = line_ranges(pp);
    let heights = line_heights(pp, &lines);
    let median = median(&heights);
    let mut best: Option<Cand> = None;
    for (li, &(a, b)) in lines.iter().enumerate() {
        let Some(c) = candidate(p, pp, a, b, heights.get(li).copied().flatten(), median) else {
            continue;
        };
        if best.is_none_or(|x| c.score > x.score) {
            best = Some(c);
        }
        // The first good title line wins: later ones are headings of the body or exhibits.
        if c.score >= GOOD {
            break;
        }
    }
    let c = best?;
    let (a, b) = join(pp, &lines, c.a, c.b);
    let (a, b) = join_split(p, pp, &lines, &heights, (a, b), c.b);
    let text = title_text(pp, a, b);
    if text.is_empty() {
        return None;
    }
    let kind = c
        .kind_at
        .or_else(|| kind_in(p, pp, a, b).map(|(_, s)| s))
        .and_then(|s| {
            let (k, _) = kind_in(p, pp, s, b)?;
            // A title naming a procurement ("JAVNA NABAVKA MALE VREDNOSTI ...") is the
            // cover of its tender documentation.
            let k = if k == Kind::Procurement {
                Kind::TenderDocumentation
            } else {
                k
            };
            let near_start = s.saturating_sub(c.a) <= 3;
            Some((
                k,
                if near_start {
                    c.score.min(0.9)
                } else {
                    c.score * 0.8
                },
            ))
        });
    let ordinal = kind.and_then(|_| ordinal(p, pp, a, b));
    Some(Title {
        text,
        page: pi,
        a,
        b,
        conf: c.score,
        kind,
        ordinal,
    })
}

fn line_ranges(pp: &Prepared<'_>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pp.toks.len() && pp.line(i) < TITLE_LINES {
        let (a, b) = pp.line_bounds(i);
        out.push((a, b));
        i = b + 1;
    }
    out
}

/// Height of a line whose block is a single line (a font-size proxy), when boxes exist.
fn line_heights(pp: &Prepared<'_>, lines: &[(usize, usize)]) -> Vec<Option<f32>> {
    lines
        .iter()
        .map(|&(a, _)| {
            let src = pp.source_span(pp.toks[a].ns, pp.toks[a].ne);
            let block = pp.block_at(src.start)?;
            let text = block.span.slice(&pp.page.text);
            if text.trim().contains('\n') {
                return None;
            }
            block.bbox.map(|bb| (bb.y1 - bb.y0).abs())
        })
        .collect()
}

fn median(h: &[Option<f32>]) -> Option<f32> {
    let mut v: Vec<f32> = h.iter().flatten().copied().filter(|x| *x > 0.0).collect();
    if v.len() < 3 {
        return None;
    }
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
}

fn candidate(
    p: &Patterns,
    pp: &Prepared<'_>,
    a: usize,
    b: usize,
    height: Option<f32>,
    median: Option<f32>,
) -> Option<Cand> {
    let src = pp.source_span(pp.toks[a].ns, pp.toks[a].ne);
    let block = pp.block_at(src.start);
    if block.is_some_and(|bl| bl.zone == quail_core::Zone::Footer) {
        return None;
    }
    // A header-zone line is a running header, unless it is an upper-case kind line at the
    // top of the first page (portal forms put "ОДЛУКА О ...", "ОБАВЕШТЕЊЕ О ..." there).
    let in_header = block.is_some_and(|bl| bl.zone == quail_core::Zone::Header);
    let n = b - a + 1;
    let keys = pp.keys.get(pp.toks[a].ks..pp.toks[b].ke)?;
    if NOT_TITLES.contains(&keys) || keys.starts_with("exhibit 10") || is_exhibit_tag(keys) {
        return None;
    }
    let first = pp.key(a);
    if first.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !pp.text(a).ends_with(char::is_alphabetic)
    {
        // "1. DEFINITIONS", "2/5": numbered headings and counters are not titles.
        return None;
    }
    let text = pp.norm.get(pp.toks[a].ns..pp.toks[b].ne)?;
    let line_end = pp.norm.get(pp.toks[b].ne..).and_then(|s| s.chars().next());
    // "Contract Request ID : X" is a field line, not a title.
    let prose = n > MAX_TITLE_TOKENS
        || text.trim_end_matches(':').contains(':')
        || text.starts_with(char::is_lowercase)
        || text.contains("\")")
        || PROSE_START.contains(&first)
        || (a..=b).any(|i| PROSE_WORDS.contains(&pp.key(i)))
        || matches!(line_end, Some(',' | ';'));
    let upper = upper_ratio(text);
    let heading = block.is_some_and(|bl| {
        matches!(bl.kind, BlockKind::Heading { .. }) || bl.zone == quail_core::Zone::Title
    });
    // "Napomena: Račun br. ..." is a field, not a title.
    let field_before = |s: usize| {
        pp.norm
            .get(pp.toks[a].ns..pp.toks[s].ns)
            .is_some_and(|x| x.contains(':'))
    };
    let kind_at = kind_in(p, pp, a, b).filter(|&(k, s)| k != Kind::Procurement || s == a);
    let kind_at = kind_at.map(|(_, s)| s).filter(|&s| {
        let pos = s - a;
        // "Prvobitna vrednost ugovora", "Уговорена вредност": a field label naming a
        // contract in an oblique case is no title unless the line is set in capitals.
        let oblique = pos > 0 && upper < 0.8 && !heading && oblique_case(pp.key(s));
        // "Garden Design, Planting and Upkeep Agreement": a short title-case line that
        // ends on its noun names the instrument however long its modifiers are.
        let ends_on_noun = n <= 8 && HEAD_NOUNS.contains(&pp.key(b)) && title_case(pp, a, b);
        !field_before(s)
            && !oblique
            && (pos <= 3 || (n <= 10 && (upper >= 0.8 || heading)) || ends_on_noun)
    });
    if in_header && !(pp.page.index == 0 && kind_at == Some(a) && upper >= 0.8) {
        return None;
    }
    let big = matches!((height, median), (Some(h), Some(m)) if h >= 1.25 * m);
    let mut score: f32 = if prose {
        0.0
    } else if heading && kind_at.is_some() {
        0.85
    } else if heading {
        // A heading without a kind word is often a letterhead name: a kind line beats it.
        0.6
    } else if kind_at.is_some() {
        if upper >= 0.8 { 0.8 } else { 0.7 }
    } else if big {
        0.65
    } else if upper >= 0.8
        && (2..=12).contains(&n)
        && !text.contains(':')
        && text.chars().filter(|c| c.is_alphabetic()).count() >= 6
    {
        0.4
    } else {
        0.0
    };
    if score > 0.0 && kind_at.is_some() && big {
        score = (score + 0.05).min(0.9);
    }
    // "EXHIBIT C", "Schedule 1 Reseller Information": an attachment's tag, the title of an
    // annex only when nothing better comes before it.
    if score >= GOOD && kind_at == Some(a) && is_attachment_tag(p, pp, a, b) {
        score = 0.5;
    }
    // "Agreement Number 12965": a number field, a fallback title only. "Aneks br. 2" names
    // an instrument by its ordinal and stays a title.
    let ordinal_like =
        |t: &str| (t.len() <= 3 && t.bytes().all(|c| c.is_ascii_digit())) || roman(t).is_some();
    if score >= GOOD
        && kind_at == Some(a)
        && b > a
        && b <= a + 2
        && is_label(pp.key(a + 1))
        && !(b == a + 2 && ordinal_like(pp.text(b)))
    {
        score = 0.6;
    }
    // Earlier lines win ties; a later candidate must be strictly better.
    (score > 0.0).then_some(Cand {
        a,
        b,
        score,
        kind_at,
    })
}

/// A Serbian contract word in an oblique case (`ugovora`, `ugovoru`, `ugovorom`,
/// `sporazuma`, ...): it names a contract inside a phrase rather than heading one.
fn oblique_case(key: &str) -> bool {
    ["ugovor", "sporazum"]
        .iter()
        .any(|stem| key.starts_with(stem) && key.len() > stem.len())
}

/// An annex kind at the line start, followed by its code and not by "to"/"uz"/"of".
fn is_attachment_tag(p: &Patterns, pp: &Prepared<'_>, a: usize, b: usize) -> bool {
    if kind_in(p, pp, a, b).map(|(k, _)| k) != Some(Kind::Annex) {
        return false;
    }
    let link = |i: usize| matches!(pp.key(i), "to" | "uz" | "of" | "za" | "na" | "under");
    !(a + 1..=b.min(a + 3)).any(link)
}

fn is_exhibit_tag(keys: &str) -> bool {
    let mut w = keys.split(' ');
    matches!(w.next(), Some("exhibit" | "ex"))
        && w.next()
            .is_some_and(|x| x.starts_with(|c: char| c.is_ascii_digit()))
        && w.next().is_none()
}

fn upper_ratio(text: &str) -> f32 {
    let (mut up, mut all) = (0u32, 0u32);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        all += 1;
        if c.is_uppercase() {
            up += 1;
        }
    }
    if all < 4 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let r = up as f32 / all as f32;
    r
}

/// Joins the next line(s) when the title runs on ("UGOVOR\nO PRUŽANJU USLUGA").
fn join(pp: &Prepared<'_>, lines: &[(usize, usize)], a: usize, b: usize) -> (usize, usize) {
    let mut end = b;
    for _ in 0..2 {
        let Some(&(na, nb)) = lines.iter().find(|(x, _)| *x == end + 1) else {
            break;
        };
        let runs_on = JOIN_AFTER.contains(&pp.key(end)) || JOIN_BEFORE.contains(&pp.key(na));
        // "... ЗА ЈАВНУ НАБАВКУ ..." names the procurement, not the instrument.
        let procurement = pp.key(na) == "za" && pp.key(na + 1).starts_with("javn");
        if !runs_on || procurement || nb - na + 1 > 10 {
            break;
        }
        end = nb;
    }
    (a, end)
}

/// Nouns a title ends on: the head of "MASTER SERVICES AGREEMENT", "SUPPLY CONTRACT".
const HEAD_NOUNS: &[&str] = &[
    "agreement",
    "agreements",
    "contract",
    "license",
    "licence",
    "lease",
    "plan",
    "addendum",
    "amendment",
    "memorandum",
    "understanding",
    "deed",
    "guarantee",
    "guaranty",
    "order",
    "ugovor",
    "sporazum",
];

/// Words of a company name's legal form: a line holding one is a party, not a title.
const LEGAL_FORMS: &[&str] = &[
    "inc",
    "incorporated",
    "corp",
    "corporation",
    "llc",
    "ltd",
    "limited",
    "plc",
    "gmbh",
    "company",
    "co",
    "lp",
    "llp",
    "doo",
    "d.o.o",
    "ad",
    "a.d",
    "s.a",
    "b.v",
    "n.v",
    "dba",
];

/// Words of a sentence running on into the title line ("... LISTED ON SCHEDULE A" /
/// "HERETO").
const RUN_ON_WORDS: &[&str] = &["hereto", "hereof", "herein", "hereunder", "thereto"];

/// Lines of a split title joined before the line that names the kind.
const MAX_LINES_BEFORE: usize = 2;
/// Tokens of one line of a split title.
const MAX_SPLIT_TOKENS: usize = 8;

/// Joins a title split over short lines in the same style: the lines before a tail line
/// that ends on its noun ("WIDGET POLISHING" / "OUTSOURCING AGREEMENT", "THIRD AMENDED AND
/// RESTATED" / "EXCLUSIVE BROKERAGE AND" / "LOGISTICS AGREEMENT"), and the line after a head
/// line that breaks after its link ("AMENDMENT NO. 4 TO THE NONEXCLUSIVE" / "GADGET
/// RESELLER AGREEMENT"). A party line (a legal form), a field, a label, an exhibit tag or prose never
/// joins. `first_end` is the end of the line the title was found on.
fn join_split(
    p: &Patterns,
    pp: &Prepared<'_>,
    lines: &[(usize, usize)],
    heights: &[Option<f32>],
    (a, b): (usize, usize),
    first_end: usize,
) -> (usize, usize) {
    let Some(li) = lines.iter().position(|&(x, _)| x == a) else {
        return (a, b);
    };
    let Some(style) = split_style(pp, lines[li], heights.get(li).copied().flatten()) else {
        return (a, b);
    };
    let mut start = a;
    // Before: the found line must end on its head noun.
    if HEAD_NOUNS.contains(&pp.key(first_end)) && kind_in(p, pp, a, first_end).is_some() {
        for k in 1..=MAX_LINES_BEFORE {
            let Some(&(pa, pb)) = li.checked_sub(k).and_then(|i| lines.get(i)) else {
                break;
            };
            let h = heights.get(li - k).copied().flatten();
            if !split_line(pp, pa, pb) || split_style(pp, (pa, pb), h) != Some(style) {
                break;
            }
            // A line naming its own instrument is a title of its own, not a modifier.
            if kind_in(p, pp, pa, pb).is_some_and(|(_, s)| s == pa) {
                break;
            }
            if !same_zone(pp, pa, a) || !stacked(pp, pa, start) {
                break;
            }
            start = pa;
        }
    }
    // After: the found line links on (`... TO THE NONEXCLUSIVE`) and the next ends on a noun.
    let mut end = b;
    let linked = (a..=b).any(|i| matches!(pp.key(i), "to" | "uz"));
    if start == a
        && b == first_end
        && linked
        && !HEAD_NOUNS.contains(&pp.key(b))
        && let Some(&(na, nb)) = lines.get(li + 1)
    {
        let h = heights.get(li + 1).copied().flatten();
        if split_line(pp, na, nb)
            && split_style(pp, (na, nb), h) == Some(style)
            && HEAD_NOUNS.contains(&pp.key(nb))
            && same_zone(pp, na, a)
            && stacked(pp, a, na)
        {
            end = nb;
        }
    }
    (start, end)
}

/// The style of a split-title line: `Some(true)` set in capitals, `Some(false)` large type
/// (its height when known), `None` neither (body text does not join).
fn split_style(pp: &Prepared<'_>, (a, b): (usize, usize), height: Option<f32>) -> Option<bool> {
    let text = pp.norm.get(pp.toks[a].ns..pp.toks[b].ne)?;
    if upper_ratio(text) >= 0.8 {
        return Some(true);
    }
    let src = pp.source_span(pp.toks[a].ns, pp.toks[a].ne);
    let heading = pp.block_at(src.start).is_some_and(|bl| {
        matches!(bl.kind, BlockKind::Heading { .. }) || bl.zone == quail_core::Zone::Title
    });
    (heading && height.is_some()).then_some(false)
}

/// Can line `a..=b` be one line of a split title? Short, words only (no field, label,
/// number, exhibit tag, party, prose or note).
fn split_line(pp: &Prepared<'_>, a: usize, b: usize) -> bool {
    if b - a + 1 > MAX_SPLIT_TOKENS {
        return false;
    }
    let Some(keys) = pp.keys.get(pp.toks[a].ks..pp.toks[b].ke) else {
        return false;
    };
    // The whole line, with the punctuation around its tokens (`DENOTED WITH "***"`).
    let start = pp
        .norm
        .get(..pp.toks[a].ns)
        .map_or(0, |s| s.rfind('\n').map_or(0, |i| i + 1));
    let end = pp.norm.get(pp.toks[b].ne..).map_or(pp.toks[b].ne, |s| {
        pp.toks[b].ne + s.find('\n').unwrap_or(s.len())
    });
    let Some(text) = pp.norm.get(start..end) else {
        return false;
    };
    let words_only = text.chars().all(|c| {
        c.is_alphanumeric()
            || c.is_whitespace()
            || matches!(c, ',' | '&' | '-' | '\'' | '’' | '®' | '/')
    });
    let alphabetic = text.chars().filter(|c| c.is_alphabetic()).count() >= 3;
    let first = pp.key(a);
    words_only
        && alphabetic
        && !NOT_TITLES.contains(&keys)
        && !keys.starts_with("exhibit")
        && !keys.starts_with("schedule")
        && !keys.starts_with("annex")
        && !keys.starts_with("appendix")
        && !first.starts_with(|c: char| c.is_ascii_digit())
        && !PROSE_START.contains(&first)
        && (a..=b).all(|i| {
            let k = pp.key(i);
            !PROSE_WORDS.contains(&k) && !LEGAL_FORMS.contains(&k) && !RUN_ON_WORDS.contains(&k)
        })
}

/// Small words a title-case line keeps in lower case.
const SMALL_WORDS: &[&str] = &[
    "and", "of", "for", "the", "to", "on", "in", "a", "an", "or", "with", "by", "i", "o", "za",
    "na", "u", "sa",
];

/// Is every word of tokens `a..=b` capitalised, small words aside (`Garden Design,
/// Planting and Upkeep Agreement`)?
fn title_case(pp: &Prepared<'_>, a: usize, b: usize) -> bool {
    let mut words = 0;
    for i in a..=b {
        let t = pp.text(i);
        let Some(c) = t.chars().next() else {
            continue;
        };
        if !c.is_alphabetic() {
            continue;
        }
        words += 1;
        if !c.is_uppercase() && !SMALL_WORDS.contains(&pp.key(i)) {
            return false;
        }
    }
    words >= 2
}

/// The box of the visual line holding token `i`: its measured line, else its block when
/// that is one line; `None` without geometry.
fn line_box(pp: &Prepared<'_>, i: usize) -> Option<BBox> {
    let src = pp.source_span(pp.toks[i].ns, pp.toks[i].ne);
    let block = pp.block_at(src.start)?;
    if let Some(l) = block
        .lines
        .iter()
        .find(|l| l.span.start <= src.start && src.start < l.span.end.max(l.span.start + 1))
    {
        return Some(l.bbox);
    }
    let text = block.span.slice(&pp.page.text);
    if text.trim().contains('\n') {
        return None;
    }
    block.bbox
}

/// Does the line of token `upper` sit right above the line of token `lower`, as one title
/// set in lines: no more than half a line of space between them, heights within a factor of
/// 1.6, and centred on the same axis or flush on the same left edge? A company name
/// over a gap, `EXECUTION VERSION` in the right margin and a party named far above do not
/// join. True when either line has no geometry.
fn stacked(pp: &Prepared<'_>, upper: usize, lower: usize) -> bool {
    let (Some(u), Some(l)) = (line_box(pp, upper), line_box(pp, lower)) else {
        return true;
    };
    let (hu, hl) = ((u.y1 - u.y0).abs(), (l.y1 - l.y0).abs());
    let h = hu.max(hl);
    if h <= 0.0 || hu.min(hl) < h / 1.6 {
        return false;
    }
    let gap = l.y0.min(l.y1) - u.y0.max(u.y1);
    let centre = |b: &BBox| f32::midpoint(b.x0, b.x1);
    let width = (u.x1 - u.x0).abs().max((l.x1 - l.x0).abs());
    let centred = (centre(&u) - centre(&l)).abs() <= (0.05 * width).max(6.0);
    let flush = (u.x0.min(u.x1) - l.x0.min(l.x1)).abs() <= 3.0;
    (-0.5 * h..=0.6 * h).contains(&gap) && (centred || flush)
}

/// Are tokens `x` and `y` in blocks of the same zone?
fn same_zone(pp: &Prepared<'_>, x: usize, y: usize) -> bool {
    let zone = |i: usize| {
        let src = pp.source_span(pp.toks[i].ns, pp.toks[i].ne);
        pp.block_at(src.start).map(|bl| bl.zone)
    };
    zone(x) == zone(y)
}

fn title_text(pp: &Prepared<'_>, a: usize, b: usize) -> String {
    let raw = pp.norm.get(pp.toks[a].ns..pp.toks[b].ne).unwrap_or("");
    let mut out = String::with_capacity(raw.len());
    for w in raw.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
        if out.chars().count() >= MAX_TITLE_CHARS {
            break;
        }
    }
    out
}

/// The first kind phrase in tokens `a..=b` and the token it starts at.
pub(crate) fn kind_in(
    p: &Patterns,
    pp: &Prepared<'_>,
    a: usize,
    b: usize,
) -> Option<(Kind, usize)> {
    let keys = pp.keys.get(pp.toks[a].ks..pp.toks[b].ke)?;
    for m in p.kind.find_iter(keys) {
        let s = pp.tok_starting(pp.toks[a].ks + m.start())?;
        let phrase = m.as_str();
        if (phrase == "po" || phrase == "p.o")
            && (pp.text(s).chars().any(char::is_lowercase) || pp.key(s + 1) == "box")
        {
            continue;
        }
        if let Some(k) = Kind::of_phrase(phrase) {
            return Some((k, s));
        }
    }
    None
}

/// The ordinal the title names: "Amendment No. 3", "Aneks br. 2", "Third Amendment",
/// "Aneks II", "SOW #12", or the tags "AM9" / "CO 2". With its token range.
fn ordinal(p: &Patterns, pp: &Prepared<'_>, a: usize, b: usize) -> Option<(i64, usize, usize)> {
    let (kind, s) = kind_in(p, pp, a, b)?;
    if s > a
        && let Some(v) = ordinal_word(pp.key(s - 1))
    {
        return Some((v, s - 1, s));
    }
    let m = p.kind.find(pp.keys.get(pp.toks[s].ks..pp.toks[b].ke)?)?;
    let e = pp.tok_ending(pp.toks[s].ks + m.end())?;
    let mut i = e + 1;
    if i <= b && is_label(pp.key(i)) {
        i += 1;
    }
    if i <= b {
        let t = pp.text(i);
        if t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 3 {
            let v: i64 = t.parse().ok()?;
            return (v > 0).then_some((v, s, i));
        }
        if t.chars().all(|c| c.is_ascii_uppercase())
            && let Some(v) = roman(t)
        {
            return Some((v, s, i));
        }
    }
    let text = pp.norm.get(pp.toks[a].ns..pp.toks[b].ne)?;
    let tag = match kind {
        Kind::Amendment => &p.amendment_tag,
        Kind::ChangeOrder => &p.change_order_tag,
        _ => return None,
    };
    let c = tag.captures(text)?;
    let v: i64 = c.get(1)?.as_str().parse().ok()?;
    Some((v, a, b))
}

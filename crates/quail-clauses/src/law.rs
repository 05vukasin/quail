//! Governing law: `governed by (and construed in accordance with) the (internal) laws of the
//! State of X`, `X law shall govern`, `under the laws of the Commonwealth of X`, a
//! `Governing Law` heading over `the laws of X`; and, with a much lower confidence, the forum
//! (`courts of the State of X`) when no governing-law clause names a place.

use crate::places::{Place, is_country, place_at};
use crate::tokens::Stream;

/// How far before `law(s)` a governing verb or heading may stand.
const VERB_WINDOW: usize = 14;
/// How far after `of` the place may start (`the State of`, `the Commonwealth of`).
const OF_WINDOW: usize = 5;

/// What found a jurisdiction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// `governed by ... the laws of X` (or a `Governing Law` heading before it).
    LawsOf,
    /// `X law` after a governing verb (`governed by Delaware law`).
    PlaceLaw,
    /// `the laws of X` with no governing verb in reach.
    BareLawsOf,
    /// The courts or venue of X (forum, not governing law).
    Forum,
}

/// One candidate.
#[derive(Clone, Copy, Debug)]
pub struct LawHit {
    /// The jurisdiction.
    pub place: &'static Place,
    /// What found it.
    pub basis: Basis,
    /// First token of the phrase.
    pub a: usize,
    /// One past the last token.
    pub b: usize,
}

/// Words that make a `law(s)` governing.
const VERBS: &[&str] = &[
    "governed",
    "governing",
    "govern",
    "governs",
    "construed",
    "interpreted",
    "enforced",
    "construction",
    "interpretation",
    "applicable",
];

/// Words of a place's title between `of` and its name.
const TITLES: &[&str] = &[
    "the",
    "state",
    "states",
    "commonwealth",
    "province",
    "provinces",
    "republic",
    "kingdom",
    "territory",
    "federal",
    "great",
];

fn verb_before(s: &Stream, i: usize) -> bool {
    // No sentence test: a `Governing Law.` heading ends in a dot and still governs.
    (i.saturating_sub(VERB_WINDOW)..i).any(|k| VERBS.contains(&s.key(k)))
}

/// The place after `of` at token `i` (`of the State of New York`): the place and one past it.
fn place_after_of(s: &Stream, i: usize) -> Option<(&'static Place, usize)> {
    let mut j = i + 1;
    let end = j + OF_WINDOW;
    while j < end {
        if let Some(hit) = place_at(|k| s.key(k), j) {
            return Some(hit);
        }
        if TITLES.contains(&s.key(j)) || s.key(j) == "of" {
            j += 1;
            continue;
        }
        break;
    }
    None
}

/// `the laws of the United States and the State of X`: prefer the state.
fn refine(s: &Stream, hit: (&'static Place, usize)) -> (&'static Place, usize) {
    let (p, b) = hit;
    if !is_country(p) || p.code != "US" {
        return hit;
    }
    let mut j = b;
    let end = b + 6;
    while j < end {
        if s.key(j) == "of"
            && let Some((q, e)) = place_after_of(s, j)
            && !is_country(q)
        {
            return (q, e);
        }
        if s.sentence_end(j) {
            break;
        }
        j += 1;
    }
    hit
}

/// Every governing-law and forum candidate, in text order.
pub fn find(s: &Stream) -> Vec<LawHit> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let k = s.key(i);
        if k == "law" || k == "laws" {
            let verb = verb_before(s, i);
            // "the laws of X".
            if s.key(i + 1) == "of"
                && let Some(hit) = place_after_of(s, i + 1)
            {
                let (place, b) = refine(s, hit);
                out.push(LawHit {
                    place,
                    basis: if verb {
                        Basis::LawsOf
                    } else {
                        Basis::BareLawsOf
                    },
                    a: i,
                    b,
                });
                i = b;
                continue;
            }
            // "Delaware law", "New York laws" after a governing verb (or before one:
            // "Delaware law shall govern").
            let after_verb = (i + 1..i + 4).any(|k| VERBS.contains(&s.key(k)));
            if (verb || after_verb)
                && let Some((place, a)) = place_ending_at(s, i)
            {
                out.push(LawHit {
                    place,
                    basis: Basis::PlaceLaw,
                    a,
                    b: i + 1,
                });
            }
        } else if matches!(k, "courts" | "court" | "venue" | "jurisdiction" | "forum")
            && let Some(hit) = forum_after(s, i)
        {
            out.push(hit);
            i = hit.b;
            continue;
        }
        i += 1;
    }
    out
}

/// A place whose last word is right before token `i`: the place and its first token.
fn place_ending_at(s: &Stream, i: usize) -> Option<(&'static Place, usize)> {
    (i.saturating_sub(4)..i).find_map(|a| {
        place_at(|k| s.key(k), a)
            .filter(|&(_, e)| e == i)
            .map(|(p, _)| (p, a))
    })
}

/// `courts of the State of X`, `courts located in X`, `jurisdiction of ... X`.
fn forum_after(s: &Stream, i: usize) -> Option<LawHit> {
    let mut j = i + 1;
    let end = i + 10;
    while j < end && !s.sentence_end(j) {
        if matches!(s.key(j), "of" | "in") {
            let hit = place_after_of(s, j);
            if let Some((place, b)) = hit {
                return Some(LawHit {
                    place,
                    basis: Basis::Forum,
                    a: i,
                    b,
                });
            }
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{Document, Page, PageKind};

    fn hits(text: &str) -> Vec<(&'static str, Basis)> {
        let d = Document {
            pages: vec![Page {
                text: text.to_string(),
                ..Page::new(0, PageKind::Physical)
            }],
        };
        let (s, _) = Stream::read(&d, 1 << 20, || false);
        find(&s)
            .into_iter()
            .map(|h| (h.place.name, h.basis))
            .collect()
    }

    #[test]
    fn governing_law_phrasings() {
        assert_eq!(
            hits(
                "This Agreement shall be governed by and construed in accordance with the internal laws of the State of New York, without regard to conflicts of law."
            ),
            [("New York", Basis::LawsOf)]
        );
        assert_eq!(
            hits("10. Governing Law. The laws of the Commonwealth of Massachusetts apply."),
            [("Massachusetts", Basis::LawsOf)]
        );
        assert_eq!(
            hits("This Agreement is governed by Delaware law."),
            [("Delaware", Basis::PlaceLaw)]
        );
        assert_eq!(
            hits("Texas law shall govern this Agreement."),
            [("Texas", Basis::PlaceLaw)]
        );
        assert_eq!(
            hits("governed by the laws of the United States and the State of California"),
            [("California", Basis::LawsOf)]
        );
        assert_eq!(
            hits("governed by the laws of England and Wales"),
            [("England and Wales", Basis::LawsOf)]
        );
    }

    #[test]
    fn bare_laws_and_forum() {
        assert_eq!(
            hits("a corporation organized under the laws of the State of Delaware."),
            [("Delaware", Basis::BareLawsOf)]
        );
        assert_eq!(
            hits("submit to the exclusive jurisdiction of the courts of the State of Illinois."),
            [("Illinois", Basis::Forum)]
        );
        assert!(hits("the laws of physics").is_empty());
    }
}

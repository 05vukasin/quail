//! Document keywords and key phrases (`summary.keywords@1`): YAKE-style statistics over the
//! candidate phrases of the [term index](crate::terms), no model, linear time.
//!
//! # Score
//!
//! Each word (stem) gets YAKE's features (Campos et al., 2020):
//!
//! - casing `T_case = max(upper, acronym) / (1 + ln tf)` (capitalized not at a sentence start;
//!   acronyms outside runs of capitals, so a shouted clause does not count);
//! - position `T_pos = ln(ln(3 + median sentence))` (over its first sentences);
//! - frequency `T_freq = tf / (mean + std)` (over content words);
//! - relatedness `T_rel = 1 + (WL + WR) · tf / max tf` (distinct neighbours per occurrence:
//!   a word next to anything behaves like a stop word);
//! - spread `T_sent = sentences / all sentences`;
//!
//! `S(w) = T_rel · T_pos / (T_case + (T_freq + T_sent) / T_rel)`, times 3 for a generic
//! document word (`agreement`, `section`, `ugovor`). A phrase scores
//! `S(k) = Π S(w) / (tf(k) · (1 + Σ S(w)))`, lower is better; phrases of 2–3 words need at
//! least 2 occurrences, single words too in a document of 300 content words or more.
//!
//! The best `max` phrases are kept, skipping a phrase that is part of (or contains) a picked
//! one and occurs at most 1.3 times as often (it adds nothing). The record's `score` is
//! `S(best) / S(k)`: 1 for the first keyword, falling.
//!
//! # Entities
//!
//! A keyword whose every word is a word of a party's name (or of its defined term,
//! `the "Distributor"`) links to that party: `entities` lists the `party.candidates` keys,
//! so documents can be matched by company as well as by subject.

use std::collections::HashMap;

use quail_core::{Document, Fact, Value};
use quail_text::NormalizedText;
use quail_text::fold::{FoldOptions, fold};

use crate::input::{self as k, Facts, field_text};
use crate::terms::{self, StemLang, TermIndex};

/// Settings of [`extract`].
#[derive(Clone, Copy, Debug)]
pub struct KeywordConfig {
    /// Keywords kept (at most).
    pub max: usize,
    /// Of them, at most this many that are a party's name (its defined term does not count).
    pub max_names: usize,
}

impl Default for KeywordConfig {
    fn default() -> Self {
        Self {
            max: 16,
            max_names: 3,
        }
    }
}

/// One keyword.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyword {
    /// As written at its first occurrence, lowercased (short acronyms keep their capitals).
    pub term: String,
    /// The matching key: stems, ASCII, the same for Latin and Cyrillic spellings.
    pub key: String,
    /// `S(best) / S(k)`, in (0, 1].
    pub score: f64,
    /// Occurrences.
    pub count: u32,
    /// Zero-based page of the first occurrence.
    pub first_page: u32,
    /// Keys of the parties it names (`party.candidates`).
    pub entities: Vec<String>,
}

impl Keyword {
    /// The record stored in `summary.keywords`.
    pub fn to_value(&self) -> Value {
        let mut f = vec![
            ("term", Value::text(self.term.as_str())),
            ("key", Value::text(self.key.as_str())),
            ("score", Value::Float(self.score)),
            ("count", Value::Int(i64::from(self.count))),
            ("first_page", Value::Int(i64::from(self.first_page))),
        ];
        if !self.entities.is_empty() {
            f.push((
                "entities",
                Value::List(
                    self.entities
                        .iter()
                        .map(|e| Value::text(e.as_str()))
                        .collect(),
                ),
            ));
        }
        Value::record(f)
    }
}

/// YAKE word scores `S(w)` by word id (`f64::INFINITY` for stop words).
pub fn word_scores(t: &TermIndex) -> Vec<f64> {
    let content: Vec<f64> = t
        .words
        .iter()
        .filter(|w| !w.stop && w.tf > 0)
        .map(|w| f64::from(w.tf))
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let n = content.len().max(1) as f64;
    let mean = content.iter().sum::<f64>() / n;
    let std = (content.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    let max_tf = content.iter().copied().fold(1.0, f64::max);
    let sentences = f64::from(t.sentences.max(1));
    t.words
        .iter()
        .map(|w| {
            if w.stop || w.tf == 0 {
                return f64::INFINITY;
            }
            let tf = f64::from(w.tf);
            let case = f64::from(w.upper.max(w.acronym)) / (1.0 + tf.ln());
            // Positions are appended in sentence order: already sorted.
            let median = f64::from(w.positions.get(w.positions.len() / 2).copied().unwrap_or(0));
            let t_pos = (3.0 + median).ln().ln();
            let t_freq = tf / (mean + std).max(1.0);
            let wl = f64::from(w.left) / f64::from(w.left_total.max(1));
            let wr = f64::from(w.right) / f64::from(w.right_total.max(1));
            let t_rel = 1.0 + (wl + wr) * tf / max_tf;
            let t_sent = f64::from(w.sentences) / sentences;
            let s = t_rel * t_pos / (case + (t_freq + t_sent) / t_rel);
            if w.generic { s * 3.0 } else { s }
        })
        .collect()
}

/// A phrase made only of title words scores this many times better: the title names the
/// subject (`Strategic Alliance Agreement`).
pub const TITLE_BOOST: f64 = 4.0;

/// Phrase ids ranked best first with their scores `S(k)` (lower is better), eligible ones
/// only. `title` holds the stems of the document title's words.
pub fn ranked(t: &TermIndex, ws: &[f64], title: &[String]) -> Vec<(u32, f64)> {
    let min_single = if t.content_words >= 300 { 2 } else { 1 };
    let in_title = |p: &crate::terms::Phrase| {
        !title.is_empty()
            && p.word_ids()
                .iter()
                .all(|&w| title.iter().any(|s| *s == t.words[w as usize].stem))
    };
    let mut v: Vec<(u32, f64)> = t
        .phrases
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let titled = in_title(p);
            let need = if titled {
                1
            } else if p.n >= 2 {
                2
            } else {
                min_single
            };
            if p.count < need {
                return None;
            }
            let mut ss = [0.0f64; 3];
            for (slot, &w) in ss.iter_mut().zip(p.word_ids()) {
                *slot = ws.get(w as usize).copied().unwrap_or(f64::INFINITY);
            }
            let ss = &ss[..usize::from(p.n)];
            if ss.iter().any(|s| !s.is_finite()) {
                return None;
            }
            // Generic words alone (`agreement`, `party`, `section`) say nothing about the
            // subject; inside a phrase (`license agreement`) they may.
            if p.word_ids().iter().all(|&w| t.words[w as usize].generic) {
                return None;
            }
            let prod: f64 = ss.iter().product();
            let sum: f64 = ss.iter().sum();
            let mut s = prod / (f64::from(p.count) * (1.0 + sum));
            if titled {
                s /= TITLE_BOOST;
            }
            Some((u32::try_from(i).ok()?, s))
        })
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    v
}

/// Whether phrase `a`'s words are a contiguous part of phrase `b`'s.
fn part_of(a: &[u32], b: &[u32]) -> bool {
    a.len() < b.len() && b.windows(a.len()).any(|w| w == a)
}

/// Picks the best `max` phrases of `ranked`, skipping redundant parts and wholes, and
/// keeping at most `max_named` phrases for which `named` holds (company names: the subject
/// words get the other places).
#[allow(clippy::many_single_char_names)] // phrase / id / score of one comparison
pub fn pick(
    t: &TermIndex,
    ranked: &[(u32, f64)],
    max: usize,
    max_named: usize,
    named: impl Fn(u32) -> bool,
) -> Vec<(u32, f64)> {
    let mut out: Vec<(u32, f64)> = Vec::new();
    let mut taken_names = 0usize;
    for &(id, s) in ranked {
        if out.len() >= max {
            break;
        }
        let p = &t.phrases[id as usize];
        let is_name = named(id);
        if is_name && taken_names >= max_named {
            continue;
        }
        // A picked phrase this one is part of, mostly occurring inside it: skip this one.
        let inside = out.iter().any(|&(o, _)| {
            let q = &t.phrases[o as usize];
            part_of(p.word_ids(), q.word_ids()) && f64::from(p.count) <= 1.3 * f64::from(q.count)
        });
        if inside {
            continue;
        }
        // A picked phrase that is mostly part of this one: this one takes its place.
        let whole = out.iter().position(|&(o, _)| {
            let q = &t.phrases[o as usize];
            part_of(q.word_ids(), p.word_ids()) && f64::from(q.count) <= 1.3 * f64::from(p.count)
        });
        match whole {
            Some(i) => {
                out[i].0 = id;
                // A second picked part of it goes too.
                let mut j = i + 1;
                while j < out.len() {
                    let q = &t.phrases[out[j].0 as usize];
                    if part_of(q.word_ids(), p.word_ids())
                        && f64::from(q.count) <= 1.3 * f64::from(p.count)
                    {
                        out.remove(j);
                    } else {
                        j += 1;
                    }
                }
            }
            None => out.push((id, s)),
        }
        taken_names += usize::from(is_name);
    }
    out
}

/// A party for keyword links: its key and the stems of its name and defined term.
#[derive(Clone, Debug, Default)]
pub struct PartyWords {
    /// `party.candidates` key.
    pub key: String,
    /// Stems of the name words.
    pub name: Vec<String>,
    /// Stems of its defined terms (`the "Distributor"`): role words, subject signals too.
    pub terms: Vec<String>,
}

impl PartyWords {
    /// Whether every stem of `stems` is a word of its name or defined term.
    pub fn names(&self, stems: &[&str]) -> bool {
        stems
            .iter()
            .all(|s| self.name.iter().chain(&self.terms).any(|x| x == s))
    }

    /// Whether `stems` name the company itself (not only its defined term).
    pub fn is_name(&self, stems: &[&str]) -> bool {
        stems.iter().all(|s| self.name.iter().any(|x| x == s))
            && !stems.iter().all(|s| self.terms.iter().any(|x| x == s))
    }
}

/// The parties of `facts` (`party.candidates`, plus the defined terms of `party.company`
/// mentions with the same key), their words stemmed for `lang`.
pub fn parties(facts: &[Fact], lang: StemLang) -> Vec<PartyWords> {
    let f = Facts(facts);
    let mut out: Vec<PartyWords> = Vec::new();
    let words = |s: &str| -> Vec<String> {
        let folded = fold(s, &FoldOptions::KEY);
        folded
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 2)
            .filter(|w| !terms::STOP_EN.contains(w) && !terms::STOP_HBS.contains(w))
            .map(|w| terms::stem(w, lang).into_owned())
            .collect()
    };
    if let Some(Value::List(cands)) = f.best(k::PARTY_CANDIDATES).map(|x| &x.value) {
        for c in cands.iter().take(16) {
            let (Some(key), Some(surface)) = (field_text(c, "key"), field_text(c, "surface"))
            else {
                continue;
            };
            out.push(PartyWords {
                key: key.to_owned(),
                name: words(surface),
                terms: Vec::new(),
            });
        }
    }
    let mut defined: HashMap<&str, Vec<String>> = HashMap::new();
    for m in f.all(k::PARTY_COMPANY).take(256) {
        if let (Some(key), Some(term)) = (
            field_text(&m.value, "key"),
            field_text(&m.value, "defined_term"),
        ) {
            defined.entry(key).or_default().extend(words(term));
        }
    }
    for p in &mut out {
        if let Some(d) = defined.get(p.key.as_str()) {
            for s in d {
                if !p.terms.contains(s) {
                    p.terms.push(s.clone());
                }
            }
        }
    }
    out
}

/// Stems of the content words of `title` (`ref.title`), for [`ranked`].
pub fn title_stems(title: &str, lang: StemLang) -> Vec<String> {
    let folded = fold(title, &FoldOptions::KEY);
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !w.bytes().all(|b| b.is_ascii_digit()))
        .filter(|w| !terms::STOP_EN.contains(w) && !terms::STOP_HBS.contains(w))
        .map(|w| terms::stem(w, lang).into_owned())
        .take(16)
        .collect()
}

/// The keywords of `doc` from its term index `t`, linked to `parties`; phrases of the
/// document `title` (stems, [`title_stems`]) ranked up.
pub fn extract(
    doc: &Document,
    shared: Option<&NormalizedText>,
    t: &TermIndex,
    parties: &[PartyWords],
    title: &[String],
    cfg: &KeywordConfig,
) -> Vec<Keyword> {
    let ws = word_scores(t);
    let r = ranked(t, &ws, title);
    let stems_of = |id: u32| -> Vec<&str> {
        t.phrases[id as usize]
            .word_ids()
            .iter()
            .map(|&w| t.words[w as usize].stem.as_str())
            .collect()
    };
    let picked = pick(t, &r, cfg.max, cfg.max_names, |id| {
        let st = stems_of(id);
        parties.iter().any(|q| q.is_name(&st))
    });
    let best = picked.first().map_or(1.0, |p| p.1);
    let ids: Vec<u32> = picked.iter().map(|p| p.0).collect();
    let mut names = t.surfaces(doc, shared, &ids);
    picked
        .into_iter()
        .map(|(id, s)| {
            let p = &t.phrases[id as usize];
            let stems = stems_of(id);
            let entities = parties
                .iter()
                .filter(|q| q.names(&stems))
                .map(|q| q.key.clone())
                .collect();
            Keyword {
                term: names.remove(&id).unwrap_or_default(),
                key: t.key(id),
                score: round(if s > 0.0 { (best / s).min(1.0) } else { 1.0 }),
                count: p.count,
                first_page: p.first_page,
                entities,
            }
        })
        .collect()
}

fn round(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
#[allow(clippy::many_single_char_names)] // short names for test fixtures
mod tests {
    use super::*;
    use crate::terms::build;
    use quail_core::{Page, PageKind, TextSource};
    use std::fmt::Write as _;

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

    fn keywords(d: &Document, lang: StemLang) -> Vec<Keyword> {
        let t = build(d, None, lang, None);
        extract(d, None, &t, &[], &[], &KeywordConfig::default())
    }

    const LEASE: &str = "EQUIPMENT LEASE AGREEMENT\n\
        This Equipment Lease Agreement is made between Lessor and Lessee. The Lessor leases \
        the forklift fleet to the Lessee for use in the Lessee's warehouse. The forklift fleet \
        includes twelve electric forklifts and their charging stations. The Lessee shall keep \
        the forklift fleet insured and shall return the forklift fleet at the end of the lease \
        term. Charging stations remain the property of the Lessor. Maintenance of the electric \
        forklifts is performed by the Lessor at the warehouse. The monthly rent for the \
        forklift fleet is due on the first day of each month.";

    #[test]
    fn finds_the_subject_phrases() {
        let d = doc(&[LEASE]);
        let k = keywords(&d, StemLang::English);
        let terms: Vec<&str> = k.iter().map(|k| k.term.as_str()).collect();
        assert!(
            terms.iter().take(6).any(|t| t.contains("forklift")),
            "{terms:?}"
        );
        assert!(terms.contains(&"charging stations"), "{terms:?}");
        assert!(terms.iter().any(|t| t.contains("lessee")), "{terms:?}");
        // No stop word, number, or generic-only phrase.
        assert!(
            !terms
                .iter()
                .any(|t| t.split(' ').any(|w| w == "the" || w == "shall")),
            "{terms:?}"
        );
        assert!((k[0].score - 1.0).abs() < 1e-9);
        assert!(k.windows(2).all(|w| w[0].score >= w[1].score));
        assert!(k.len() <= 16);
        // Deterministic.
        assert_eq!(k, keywords(&d, StemLang::English));
    }

    #[test]
    fn latin_and_cyrillic_give_one_keyword() {
        let lat = "Ugovor o isporuci solarnih panela. Isporuka solarnih panela obuhvata montažu \
                   solarnih panela na krovu magacina. Montaža solarnih panela traje deset dana. \
                   Garancija za solarne panele traje pet godina.";
        let cyr = "Уговор о испоруци соларних панела. Испорука соларних панела обухвата монтажу \
                   соларних панела на крову магацина. Монтажа соларних панела траје десет дана. \
                   Гаранција за соларне панеле траје пет година.";
        let a = keywords(&doc(&[lat]), StemLang::Hbs);
        let b = keywords(&doc(&[cyr]), StemLang::Hbs);
        let keys = |k: &[Keyword]| k.iter().map(|k| k.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&a), keys(&b));
        assert!(
            keys(&a).iter().any(|k| k == "solarn panel"),
            "{:?}",
            keys(&a)
        );
        // Mixed: both scripts count as one phrase.
        let m = keywords(&doc(&[lat, cyr]), StemLang::Hbs);
        let sp = m.iter().find(|k| k.key == "solarn panel").unwrap();
        assert_eq!(sp.count, 10, "{m:?}");
        assert_eq!(sp.term, "solarnih panela");
        let c = b.iter().find(|k| k.key == "solarn panel").unwrap();
        assert_eq!(c.term, "соларних панела");
    }

    #[test]
    fn links_party_names() {
        let text = "Bluefin Analytics licenses the tide model to Coralline Ports. Bluefin \
                    Analytics hosts the tide model. Coralline Ports pays Bluefin Analytics for \
                    the tide model each quarter. The tide model predicts harbor tides.";
        let d = doc(&[text]);
        let t = build(&d, None, StemLang::English, None);
        let parties = vec![PartyWords {
            key: "bluefin analytics".into(),
            name: vec!["bluefin".into(), "analytic".into()],
            terms: Vec::new(),
        }];
        let k = extract(&d, None, &t, &parties, &[], &KeywordConfig::default());
        let b = k.iter().find(|k| k.key == "bluefin analytic").unwrap();
        assert_eq!(b.entities, vec!["bluefin analytics"]);
        let m = k.iter().find(|k| k.key == "tide model").unwrap();
        assert!(m.entities.is_empty());
        let v = b.to_value();
        assert!(v.field("entities").is_some());
        assert!(m.to_value().field("entities").is_none());
    }

    #[test]
    fn title_words_rank_up_and_generic_words_alone_never_rank() {
        let text = "GLAZING SERVICES AGREEMENT\nThe Contractor installs triple glazing panes in \
                    the atrium. The Contractor measures the atrium frames before the panes are \
                    cut. Under this agreement the parties meet weekly. The agreement covers the \
                    atrium, the frames and the panes. Each party signs the agreement. Glazing \
                    work ends in spring.";
        let d = doc(&[text]);
        let t = build(&d, None, StemLang::English, None);
        let plain = extract(&d, None, &t, &[], &[], &KeywordConfig::default());
        let title = title_stems("Glazing Services Agreement", StemLang::English);
        assert_eq!(title, vec!["glazing", "service", "agreement"]);
        let titled = extract(&d, None, &t, &[], &title, &KeywordConfig::default());
        let rank = |k: &[Keyword], key: &str| k.iter().position(|x| x.key == key);
        let before = rank(&plain, "glazing");
        let after = rank(&titled, "glazing");
        assert!(
            after.is_some() && (before.is_none() || after < before),
            "{plain:?} {titled:?}"
        );
        for k in [&plain, &titled] {
            assert!(
                !k.iter().any(|x| x.key == "agreement" || x.key == "party"),
                "{k:?}"
            );
        }
    }

    #[test]
    fn empty_and_hostile_text() {
        assert!(keywords(&doc(&[]), StemLang::English).is_empty());
        assert!(keywords(&doc(&["", "   ", "12 34 56 / . ,"]), StemLang::English).is_empty());
        let noise = (0..5000).fold(String::new(), |mut s, i| {
            let _ = write!(s, "w{i}x ");
            s
        });
        assert!(keywords(&doc(&[&noise]), StemLang::None).is_empty());
    }
}

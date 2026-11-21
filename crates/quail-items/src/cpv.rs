//! Category candidates from the EU Common Procurement Vocabulary (CPV 2008).
//!
//! The index is built once from the embedded `data/cpv2008.tsv` (9 454 codes, English and
//! Serbian Latin labels, about 620 kB) and shared read-only. A description is matched
//! against every label in two vector spaces, each TF-IDF weighted and cosine-normalized:
//!
//! - **word stems**: folded words (lowercase, no diacritics, Cyrillic → Latin), stop words
//!   out, light English/Serbian suffix stripping, cut to 6 characters;
//! - **character trigrams** of each word with boundary marks, which absorb inflection
//!   (`održavanje` / `održavanja`) and small spelling differences.
//!
//! The score of a code is the mean of both cosines, taking the better of its English and
//! Serbian label. Scores are in `[0, 1]`; a candidate list holds the top 3 above
//! [`MIN_SCORE`].

use std::collections::HashMap;

use quail_text::fold::{FoldOptions, fold};

/// Candidates below this score are dropped.
pub const MIN_SCORE: f32 = 0.12;

const DATA: &str = include_str!("../data/cpv2008.tsv");

/// One CPV code with its labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpvEntry {
    /// Code with check digit, e.g. `90910000-9`.
    pub code: Box<str>,
    /// English label.
    pub en: Box<str>,
    /// Serbian label in Latin script (may be empty).
    pub sr: Box<str>,
}

/// A category candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Index into [`CpvIndex::entries`].
    pub entry: usize,
    /// Score in `[0, 1]`.
    pub score: f32,
}

/// The CPV label index. Build once with [`CpvIndex::embedded`], share with `Arc`.
pub struct CpvIndex {
    entries: Vec<CpvEntry>,
    /// Label document → entry.
    doc_entry: Vec<u32>,
    words: Space,
    grams: Space,
}

#[derive(Default)]
struct Space {
    ids: HashMap<String, u32>,
    idf: Vec<f32>,
    /// Term → (doc, normalized weight).
    postings: Vec<Vec<(u32, f32)>>,
}

impl Space {
    fn build(docs: &[Vec<String>]) -> Self {
        let mut s = Space::default();
        let mut df: Vec<u32> = Vec::new();
        let mut doc_terms: Vec<Vec<(u32, f32)>> = Vec::with_capacity(docs.len());
        for terms in docs {
            let mut tf: HashMap<u32, f32> = HashMap::new();
            for t in terms {
                let next = u32::try_from(s.ids.len()).unwrap_or(u32::MAX);
                let id = *s.ids.entry(t.clone()).or_insert(next);
                if id as usize >= df.len() {
                    df.push(0);
                }
                *tf.entry(id).or_default() += 1.0;
            }
            for id in tf.keys() {
                df[*id as usize] += 1;
            }
            let mut v: Vec<(u32, f32)> = tf.into_iter().collect();
            v.sort_unstable_by_key(|(id, _)| *id);
            doc_terms.push(v);
        }
        #[allow(clippy::cast_precision_loss)]
        let n = docs.len().max(1) as f32;
        #[allow(clippy::cast_precision_loss)]
        let idf: Vec<f32> = df
            .iter()
            .map(|&d| (n / d.max(1) as f32).ln() + 1.0)
            .collect();
        s.postings = vec![Vec::new(); idf.len()];
        for (doc, terms) in doc_terms.iter().enumerate() {
            let w: Vec<f32> = terms
                .iter()
                .map(|(id, tf)| (1.0 + tf.ln()) * idf[*id as usize])
                .collect();
            let norm = w.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
            for ((id, _), x) in terms.iter().zip(w) {
                s.postings[*id as usize].push((u32::try_from(doc).unwrap_or(u32::MAX), x / norm));
            }
        }
        s.idf = idf;
        s
    }

    /// Adds `weight × cosine` of the query to `acc` per document (`touched` lists the
    /// documents that got a non-zero score).
    fn score(&self, terms: &[String], weight: f32, acc: &mut [f32], touched: &mut Vec<u32>) {
        let mut tf: HashMap<u32, f32> = HashMap::new();
        for t in terms {
            if let Some(&id) = self.ids.get(t) {
                *tf.entry(id).or_default() += 1.0;
            }
        }
        // Query norm over all its terms; unknown terms count with the highest idf, so a
        // description that is mostly unknown words scores low.
        #[allow(clippy::cast_precision_loss)]
        let unknown = terms.len() as f32 - tf.values().sum::<f32>();
        let max_idf = self.idf.iter().copied().fold(1.0, f32::max);
        let qw: Vec<(u32, f32)> = tf
            .iter()
            .map(|(id, f)| (*id, (1.0 + f.ln()) * self.idf[*id as usize]))
            .collect();
        let norm = (qw.iter().map(|(_, x)| x * x).sum::<f32>() + unknown * max_idf * max_idf)
            .sqrt()
            .max(1e-6);
        for (id, x) in qw {
            let w = weight * x / norm;
            for &(doc, y) in &self.postings[id as usize] {
                if let Some(a) = acc.get_mut(doc as usize) {
                    if *a == 0.0 {
                        touched.push(doc);
                    }
                    *a += w * y;
                }
            }
        }
    }
}

const STOP: &[&str] = &[
    "and", "of", "the", "for", "or", "in", "with", "to", "on", "a", "an", "by", "per", "other",
    "i", "u", "za", "od", "na", "sa", "s", "po", "do", "ili", "se", "koji", "koje", "koja", "kao",
    "ostali", "ostale", "ostalo", "drugi", "druge", "srodni", "srodne", "related", "etc",
];

const EN_SUFFIX: &[&str] = &["ings", "ing", "ies", "es", "ed", "s"];
const SR_SUFFIX: &[&str] = &[
    "ovanja", "ovanje", "anja", "anje", "enja", "enje", "ima", "ama", "ova", "eva", "om", "em",
    "ih", "im", "og", "oj", "a", "e", "i", "o", "u",
];

fn stem(w: &str) -> String {
    let mut s = w;
    for suf in EN_SUFFIX.iter().chain(SR_SUFFIX) {
        if s.len() > suf.len() + 3 && s.ends_with(suf) {
            s = &s[..s.len() - suf.len()];
            break;
        }
    }
    s.chars().take(6).collect()
}

/// Folded words of a text (letters and digits), at least two characters, numbers out.
fn folded_words(text: &str) -> Vec<String> {
    let f = fold(text, &FoldOptions::KEY);
    f.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

fn word_terms(words: &[String]) -> Vec<String> {
    words
        .iter()
        .filter(|w| !STOP.contains(&w.as_str()))
        .map(|w| stem(w))
        .collect()
}

fn gram_terms(words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for w in words {
        if STOP.contains(&w.as_str()) {
            continue;
        }
        let chars: Vec<char> = std::iter::once('_')
            .chain(w.chars())
            .chain(std::iter::once('_'))
            .collect();
        for g in chars.windows(3) {
            out.push(g.iter().collect());
        }
    }
    out
}

impl CpvIndex {
    /// The index over the embedded CPV 2008 list.
    pub fn embedded() -> Self {
        Self::from_tsv(DATA)
    }

    /// An index over `code<TAB>en<TAB>sr` lines (for tests and custom taxonomies).
    pub fn from_tsv(tsv: &str) -> Self {
        let mut entries = Vec::new();
        let mut doc_entry = Vec::new();
        let mut word_docs = Vec::new();
        let mut gram_docs = Vec::new();
        for line in tsv.lines() {
            let mut parts = line.split('\t');
            let (Some(code), Some(en)) = (parts.next(), parts.next()) else {
                continue;
            };
            let sr = parts.next().unwrap_or("");
            let e = u32::try_from(entries.len()).unwrap_or(u32::MAX);
            for label in [en, sr] {
                if label.is_empty() {
                    continue;
                }
                let w = folded_words(label);
                word_docs.push(word_terms(&w));
                gram_docs.push(gram_terms(&w));
                doc_entry.push(e);
            }
            entries.push(CpvEntry {
                code: code.into(),
                en: en.into(),
                sr: sr.into(),
            });
        }
        Self {
            words: Space::build(&word_docs),
            grams: Space::build(&gram_docs),
            entries,
            doc_entry,
        }
    }

    /// All entries, in code order.
    pub fn entries(&self) -> &[CpvEntry] {
        &self.entries
    }

    /// The best `k` categories for a description, best first, scores ≥ [`MIN_SCORE`].
    pub fn top(&self, text: &str, k: usize) -> Vec<Candidate> {
        let w = folded_words(text);
        if w.is_empty() {
            return Vec::new();
        }
        let mut acc = vec![0f32; self.doc_entry.len()];
        let mut touched = Vec::new();
        self.words
            .score(&word_terms(&w), 0.5, &mut acc, &mut touched);
        self.grams
            .score(&gram_terms(&w), 0.5, &mut acc, &mut touched);
        let mut best: HashMap<u32, f32> = HashMap::with_capacity(touched.len());
        for doc in touched {
            let s = acc[doc as usize];
            if s < MIN_SCORE {
                continue;
            }
            if let Some(&e) = self.doc_entry.get(doc as usize) {
                let b = best.entry(e).or_default();
                *b = b.max(s);
            }
        }
        let mut v: Vec<(u32, f32)> = best.into_iter().filter(|(_, s)| *s >= MIN_SCORE).collect();
        // Ties go to the more general code (smaller code string sorts first).
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(k);
        v.into_iter()
            .map(|(e, s)| Candidate {
                entry: e as usize,
                score: s.clamp(0.0, 1.0),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(ix: &CpvIndex, q: &str) -> Vec<String> {
        ix.top(q, 3)
            .into_iter()
            .map(|c| ix.entries()[c.entry].code.to_string())
            .collect()
    }

    #[test]
    fn matches_english_and_serbian() {
        let ix = CpvIndex::from_tsv(
            "90910000-9\tCleaning services\tUsluge čišćenja\n\
             50730000-1\tRepair and maintenance services of cooler groups\tUsluge popravke i održavanja rashladnih grupa\n\
             72000000-5\tIT services: consulting, software development, Internet and support\tIT usluge: savetovanje, razvoj softvera, internet i podrška\n\
             79341000-6\tAdvertising services\tUsluge oglašavanja\n",
        );
        assert_eq!(codes(&ix, "Office cleaning")[0], "90910000-9");
        assert_eq!(codes(&ix, "Čišćenje poslovnog prostora")[0], "90910000-9");
        assert_eq!(codes(&ix, "Održavanje rashladnih uređaja")[0], "50730000-1");
        assert_eq!(codes(&ix, "TV advertising spot")[0], "79341000-6");
        assert!(ix.top("xyz qqq", 3).is_empty());
    }

    #[test]
    fn embedded_index_loads() {
        let ix = CpvIndex::embedded();
        assert_eq!(ix.entries().len(), 9454);
        let top = codes(&ix, "Cleaning services");
        assert!(top.iter().any(|c| c.starts_with("909")), "{top:?}");
    }
}

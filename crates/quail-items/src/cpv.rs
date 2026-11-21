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

/// A term packed into an integer: up to six characters of 21 bits each (stems are cut to
/// six characters, trigrams have three), so two different terms never share a key.
type TermKey = u128;

fn term_key(t: &str) -> TermKey {
    t.chars()
        .take(6)
        .fold(0u128, |k, c| (k << 21) | TermKey::from(u32::from(c) + 1))
}

/// One TF-IDF vector space in compressed sparse rows: the postings of term `t` are
/// `docs[offsets[t]..offsets[t + 1]]` with their weights, in document order. No string and
/// no per-term allocation is kept after the build.
#[derive(Default)]
struct Space {
    ids: HashMap<TermKey, u32>,
    idf: Vec<f32>,
    max_idf: f32,
    offsets: Vec<u32>,
    docs: Vec<u32>,
    weights: Vec<f32>,
}

impl Space {
    /// Builds the space from each document's terms, given as term keys.
    fn build(docs: impl Iterator<Item = Vec<TermKey>>) -> Self {
        let mut s = Space::default();
        // Per document: (term id, tf), sorted by id, flattened.
        let mut flat: Vec<(u32, f32)> = Vec::new();
        let mut doc_start: Vec<u32> = vec![0];
        let mut df: Vec<u32> = Vec::new();
        let mut ids_buf: Vec<u32> = Vec::new();
        for terms in docs {
            ids_buf.clear();
            for t in terms {
                let next = u32::try_from(s.ids.len()).unwrap_or(u32::MAX);
                let id = *s.ids.entry(t).or_insert(next);
                if id as usize >= df.len() {
                    df.push(0);
                }
                ids_buf.push(id);
            }
            ids_buf.sort_unstable();
            let mut i = 0;
            while i < ids_buf.len() {
                let id = ids_buf[i];
                let run = ids_buf[i..].iter().take_while(|&&x| x == id).count();
                df[id as usize] += 1;
                #[allow(clippy::cast_precision_loss)]
                flat.push((id, run as f32));
                i += run;
            }
            doc_start.push(u32::try_from(flat.len()).unwrap_or(u32::MAX));
        }
        #[allow(clippy::cast_precision_loss)]
        let n = (doc_start.len() - 1).max(1) as f32;
        #[allow(clippy::cast_precision_loss)]
        let idf: Vec<f32> = df
            .iter()
            .map(|&d| (n / d.max(1) as f32).ln() + 1.0)
            .collect();
        // Postings per term, then offsets.
        let mut offsets = vec![0u32; idf.len() + 1];
        for (id, _) in &flat {
            offsets[*id as usize + 1] += 1;
        }
        for t in 1..offsets.len() {
            offsets[t] += offsets[t - 1];
        }
        let mut fill: Vec<u32> = offsets[..offsets.len() - 1].to_vec();
        let mut post_docs = vec![0u32; flat.len()];
        let mut post_w = vec![0f32; flat.len()];
        for (doc, w) in doc_start.windows(2).enumerate() {
            let terms = &flat[w[0] as usize..w[1] as usize];
            let norm = terms
                .iter()
                .map(|(id, tf)| {
                    let x = (1.0 + tf.ln()) * idf[*id as usize];
                    x * x
                })
                .sum::<f32>()
                .sqrt()
                .max(1e-6);
            for (id, tf) in terms {
                let slot = fill[*id as usize] as usize;
                fill[*id as usize] += 1;
                post_docs[slot] = u32::try_from(doc).unwrap_or(u32::MAX);
                post_w[slot] = (1.0 + tf.ln()) * idf[*id as usize] / norm;
            }
        }
        s.max_idf = idf.iter().copied().fold(1.0, f32::max);
        s.idf = idf;
        s.offsets = offsets;
        s.docs = post_docs;
        s.weights = post_w;
        s.ids.shrink_to_fit();
        s
    }

    /// Heap bytes held by the space (the hash table approximately).
    fn heap_bytes(&self) -> usize {
        self.ids.capacity() * (std::mem::size_of::<(TermKey, u32)>() + 1)
            + self.idf.capacity() * 4
            + self.offsets.capacity() * 4
            + self.docs.capacity() * 4
            + self.weights.capacity() * 4
    }

    /// Adds `weight × cosine` of the query to `acc` per document (`touched` lists the
    /// documents that got a non-zero score).
    fn score(&self, terms: &[String], weight: f32, acc: &mut [f32], touched: &mut Vec<u32>) {
        let mut tf: Vec<(u32, f32)> = Vec::new();
        for t in terms {
            if let Some(&id) = self.ids.get(&term_key(t)) {
                match tf.iter_mut().find(|(i, _)| *i == id) {
                    Some((_, f)) => *f += 1.0,
                    None => tf.push((id, 1.0)),
                }
            }
        }
        // Query norm over all its terms; unknown terms count with the highest idf, so a
        // description that is mostly unknown words scores low.
        #[allow(clippy::cast_precision_loss)]
        let unknown = terms.len() as f32 - tf.iter().map(|(_, f)| f).sum::<f32>();
        let qw: Vec<(u32, f32)> = tf
            .iter()
            .map(|(id, f)| (*id, (1.0 + f.ln()) * self.idf[*id as usize]))
            .collect();
        let norm = (qw.iter().map(|(_, x)| x * x).sum::<f32>()
            + unknown * self.max_idf * self.max_idf)
            .sqrt()
            .max(1e-6);
        for (id, x) in qw {
            let w = weight * x / norm;
            let (Some(&a), Some(&b)) = (
                self.offsets.get(id as usize),
                self.offsets.get(id as usize + 1),
            ) else {
                continue;
            };
            let (Some(docs), Some(ws)) = (
                self.docs.get(a as usize..b as usize),
                self.weights.get(a as usize..b as usize),
            ) else {
                continue;
            };
            for (&doc, &y) in docs.iter().zip(ws) {
                if let Some(acc) = acc.get_mut(doc as usize) {
                    if *acc == 0.0 {
                        touched.push(doc);
                    }
                    *acc += w * y;
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
        let mut labels: Vec<&str> = Vec::new();
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
                labels.push(label);
                doc_entry.push(e);
            }
            entries.push(CpvEntry {
                code: code.into(),
                en: en.into(),
                sr: sr.into(),
            });
        }
        // Terms are computed per label and packed right away: no label's strings outlive
        // its turn.
        let keys = |f: fn(&[String]) -> Vec<String>| {
            labels
                .iter()
                .map(move |l| f(&folded_words(l)).iter().map(|t| term_key(t)).collect())
        };
        let words = Space::build(keys(word_terms));
        let grams = Space::build(keys(gram_terms));
        entries.shrink_to_fit();
        doc_entry.shrink_to_fit();
        Self {
            entries,
            doc_entry,
            words,
            grams,
        }
    }

    /// Approximate heap bytes held by the index (entries, postings, term tables).
    pub fn heap_bytes(&self) -> usize {
        let entries: usize = self
            .entries
            .iter()
            .map(|e| e.code.len() + e.en.len() + e.sr.len())
            .sum();
        entries
            + self.entries.capacity() * std::mem::size_of::<CpvEntry>()
            + self.doc_entry.capacity() * 4
            + self.words.heap_bytes()
            + self.grams.heap_bytes()
    }

    /// All entries, in code order.
    pub fn entries(&self) -> &[CpvEntry] {
        &self.entries
    }

    /// The best `k` categories for a description, best first, scores ≥ [`MIN_SCORE`].
    pub fn top(&self, text: &str, k: usize) -> Vec<Candidate> {
        self.top_with(text, k, false)
    }

    /// Like [`CpvIndex::top`]; with `service` (the line is billed by time, or reads as a role
    /// or a service), service codes (divisions 50–98) are preferred over goods and works.
    pub fn top_with(&self, text: &str, k: usize, service: bool) -> Vec<Candidate> {
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
                let s = if service {
                    let services = self
                        .entries
                        .get(e as usize)
                        .is_some_and(|x| x.code.as_bytes().first().is_some_and(|d| *d >= b'5'));
                    if services { s * 1.15 } else { s * 0.85 }
                } else {
                    s
                };
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

/// Words that make a line read as a service or a role billed by time.
const SERVICE_WORDS: &[&str] = &[
    "service",
    "services",
    "fee",
    "fees",
    "consulting",
    "consultancy",
    "consultant",
    "support",
    "management",
    "manager",
    "director",
    "lead",
    "analyst",
    "specialist",
    "engineer",
    "developer",
    "architect",
    "associate",
    "principal",
    "partner",
    "advisor",
    "maintenance",
    "repair",
    "installation",
    "training",
    "design",
    "audit",
    "development",
    "usluga",
    "usluge",
    "uslugu",
    "servis",
    "odrzavanje",
    "odrzavanja",
    "popravka",
    "konsultant",
    "savetovanje",
    "obuka",
    "projektovanje",
    "izrada",
    "ispitivanje",
    "kontrola",
    "nadzor",
    "prevoz",
    "dostava",
];

/// Whether a description (or its unit) says the line is a service.
pub(crate) fn reads_as_service(text: &str, unit: Option<&str>) -> bool {
    if matches!(
        unit,
        Some("h" | "day" | "week" | "month" | "year" | "person" | "trip" | "service" | "lump_sum")
    ) {
        return true;
    }
    folded_words(text)
        .iter()
        .any(|w| SERVICE_WORDS.contains(&w.as_str()))
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

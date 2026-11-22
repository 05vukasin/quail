//! The term index of one document: every candidate key phrase (1–3 content words between
//! stop words and punctuation) with its count, first page and the statistics of its words,
//! and every page's counts of its 1–2 word phrases. One linear pass over the shared
//! `normalized_text` tokens; [`crate::keywords`] ranks it for the document and
//! [`crate::topics`] for its sections.
//!
//! # Terms
//!
//! - Tokens are the word tokens of [`quail_text::NormalizedPage`] with their matching key
//!   ([`quail_text::fold::FoldOptions::KEY`]: lowercase, no diacritics, Serbian Cyrillic
//!   transliterated), so `Уговор`, `UGOVOR` and `ugovor` are one word.
//! - **Lemmatization-lite** ([`stem`]): Serbian (and Croatian, Bosnian) case endings are
//!   folded on words of at least 6 letters, keeping a stem of at least 4 (`ugovora`,
//!   `ugovorom`, `ugovorima` → `ugovor`); English plurals (`services` → `service`, `parties`
//!   → `party`); other languages are left as they are.
//! - A phrase is a run of 1–3 content words with no stop word, punctuation, number, code or
//!   line-initial capital (a heading line ending) inside. Content words have at least 3
//!   letters (2 for an upper-case acronym), and are no stop word (English and Serbian, legal
//!   boilerplate such as `hereinafter`, months, Roman numerals, legal forms).
//! - Running headers and footers (a line among the first or last three of a page that
//!   repeats, digits ignored, on at least 3 pages and a tenth of the document) are skipped.
//!
//! # Bounds
//!
//! At most [`MAX_WORDS`] distinct words and [`MAX_PHRASES`] distinct phrases (later new ones
//! are not counted), per-page phrase counts for 1–2 word phrases only. The pass checks the
//! deadline every 16 pages and marks the index [`TermIndex::truncated`].

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use quail_core::{Artifact, Document, TextSource};
use quail_text::fold::{FoldOptions, fold};
use quail_text::token::TokenKind;
use quail_text::{NormalizedPage, NormalizedText};

/// Distinct words counted at most.
pub const MAX_WORDS: usize = 150_000;
/// Distinct phrases counted at most.
pub const MAX_PHRASES: usize = 300_000;
/// Sentence positions kept per word for its median position.
const MAX_POSITIONS: usize = 32;
/// Unused slot of a phrase's word ids.
const NONE: u32 = u32::MAX;

/// Language family of the stemming rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StemLang {
    /// English plural folding.
    English,
    /// Serbian, Croatian, Bosnian, Montenegrin case endings (Latin and Cyrillic).
    Hbs,
    /// No stemming.
    None,
}

impl StemLang {
    /// The rules for an ISO 639-1 code (`text.lang`), Serbian for Cyrillic text without one.
    pub fn of(lang: Option<&str>, script: Option<&str>) -> Self {
        match lang {
            Some("en") => Self::English,
            Some("sr" | "hr" | "bs" | "sh" | "cnr") => Self::Hbs,
            None if script == Some("cyrillic") => Self::Hbs,
            _ => Self::None,
        }
    }
}

/// One word (a stem) of the document, stop words included: YAKE's statistics.
#[derive(Clone, Debug, Default)]
pub struct WordStats {
    /// The stem.
    pub stem: String,
    /// A stop word or no content word.
    pub stop: bool,
    /// A generic legal or document word (`agreement`, `section`, `ugovor`): ranked down.
    pub generic: bool,
    /// Occurrences.
    pub tf: u32,
    /// Capitalized occurrences not at a sentence start.
    pub upper: u32,
    /// Acronym occurrences (all capitals, not inside a line of capitals).
    pub acronym: u32,
    /// Sentences it occurs in.
    pub sentences: u32,
    last_sentence: u32,
    /// The first sentence indexes it occurs in.
    pub positions: Vec<u32>,
    /// Distinct words seen right before it.
    pub left: u32,
    /// Distinct words seen right after it.
    pub right: u32,
    /// Occurrences with a word right before it.
    pub left_total: u32,
    /// Occurrences with a word right after it.
    pub right_total: u32,
}

/// One candidate phrase.
#[derive(Clone, Debug)]
pub struct Phrase {
    /// Word ids (indexes into [`TermIndex::words`]), `n` of them.
    pub words: [u32; 3],
    /// Number of words, 1–3.
    pub n: u8,
    /// Occurrences.
    pub count: u32,
    /// Zero-based page index of the first occurrence.
    pub first_page: u32,
    /// Position in `doc.pages` and byte range of the first occurrence in that page's
    /// normalized text.
    first_at: (u32, u32, u32),
    /// Occurrences written entirely in capitals.
    pub caps: u32,
}

impl Phrase {
    /// Its word ids.
    pub fn word_ids(&self) -> &[u32] {
        &self.words[..usize::from(self.n)]
    }
}

/// The term index of one document, see the [module docs](self). Also an artifact
/// (`summary_terms`), so the keyword, topic and digest steps of one file build it once.
#[derive(Clone, Debug)]
pub struct TermIndex {
    /// Stemming rules used.
    pub lang: StemLang,
    /// Every word, by id.
    pub words: Vec<WordStats>,
    /// Every candidate phrase, by id.
    pub phrases: Vec<Phrase>,
    /// Per page (by position in `doc.pages`): its zero-based index and the counts of its
    /// 1–2 word phrases, sorted by phrase id.
    pub pages: Vec<(u32, Vec<(u32, u32)>)>,
    /// Sentences seen.
    pub sentences: u32,
    /// Content-word occurrences counted.
    pub content_words: u64,
    /// The deadline stopped the pass: later pages were not read.
    pub truncated: bool,
    /// Furniture lines skipped (running headers and footers).
    pub furniture_lines: u32,
}

impl Artifact for TermIndex {
    const NAME: &'static str = "summary_terms";
}

/// The key of a phrase: its word ids, unused slots [`NONE`].
type PhraseKey = [u32; 3];

/// A fast multiplicative hasher (`FxHash`) for maps keyed by ids this crate assigns in order.
/// Never used for text keys: a hostile file cannot choose the ids, so it cannot make them
/// collide.
#[derive(Clone, Copy, Default)]
pub(crate) struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

/// A map keyed by ids ([`IdHasher`]).
pub(crate) type IdMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<IdHasher>>;
/// A set of ids ([`IdHasher`]).
pub(crate) type IdSet<K> = HashSet<K, std::hash::BuildHasherDefault<IdHasher>>;

struct Builder {
    lang: StemLang,
    word_ids: HashMap<String, u32>,
    words: Vec<WordStats>,
    phrase_ids: IdMap<PhraseKey, u32>,
    phrases: Vec<Phrase>,
    pairs: IdSet<u64>,
    sentence: u32,
    content_words: u64,
}

/// One word token of a page, ready for the phrase pass.
struct Tok {
    word: u32,
    start: u32,
    end: u32,
    /// The run of content words is cut before this token.
    cut_before: bool,
    content: bool,
}

impl Builder {
    fn word(&mut self, stem: &str, key: &str) -> Option<u32> {
        if let Some(&i) = self.word_ids.get(stem) {
            return Some(i);
        }
        if self.words.len() >= MAX_WORDS {
            return None;
        }
        let i = u32::try_from(self.words.len()).ok()?;
        let stop = is_stop(key) || is_stop(stem);
        self.words.push(WordStats {
            stem: stem.to_owned(),
            stop,
            generic: GENERIC.contains(&stem) || GENERIC.contains(&key),
            last_sentence: u32::MAX,
            ..WordStats::default()
        });
        self.word_ids.insert(stem.to_owned(), i);
        Some(i)
    }

    fn phrase(
        &mut self,
        key: PhraseKey,
        n: u8,
        page: u32,
        at: (u32, u32, u32),
        caps: bool,
    ) -> Option<u32> {
        if let Some(&i) = self.phrase_ids.get(&key) {
            let p = &mut self.phrases[i as usize];
            p.count += 1;
            p.caps += u32::from(caps);
            return Some(i);
        }
        if self.phrases.len() >= MAX_PHRASES {
            return None;
        }
        let i = u32::try_from(self.phrases.len()).ok()?;
        self.phrases.push(Phrase {
            words: key,
            n,
            count: 1,
            first_page: page,
            first_at: at,
            caps: u32::from(caps),
        });
        self.phrase_ids.insert(key, i);
        Some(i)
    }

    /// Reads one normalized page: word statistics, phrases, the page's phrase counts.
    #[allow(clippy::too_many_lines)] // one pass, one place
    fn page(
        &mut self,
        pos: u32,
        index: u32,
        np: &NormalizedPage,
        skip: &[(usize, usize)],
    ) -> Vec<(u32, u32)> {
        let text = np.text();
        let toks = np.tokens();
        let mut seq: Vec<Tok> = Vec::with_capacity(toks.len() / 2);
        let mut cut = true;
        let mut sentence_start = true;
        let mut prev_word: Option<u32> = None;
        let mut skip_i = 0usize;
        let mut caps_flags: Vec<bool> = Vec::with_capacity(toks.len() / 2);
        for (i, t) in toks.iter().enumerate() {
            let s = t.start();
            while skip.get(skip_i).is_some_and(|&(_, e)| e <= s) {
                skip_i += 1;
            }
            if skip.get(skip_i).is_some_and(|&(a, e)| a <= s && s < e) {
                cut = true;
                prev_word = None;
                continue;
            }
            match t.kind {
                TokenKind::Punct => {
                    let p = np.token_text(i);
                    // A hyphen or apostrophe inside a word joins it to the next one.
                    let joined = matches!(p, "-" | "'")
                        && text
                            .as_bytes()
                            .get(s.wrapping_sub(1))
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && text
                            .as_bytes()
                            .get(t.end())
                            .is_some_and(u8::is_ascii_alphanumeric);
                    if !joined {
                        cut = true;
                        prev_word = None;
                    }
                    if matches!(p, "." | "!" | "?") {
                        self.sentence = self.sentence.saturating_add(1);
                        sentence_start = true;
                    }
                    continue;
                }
                TokenKind::Word => {}
                _ => {
                    // Numbers, codes, URLs, addresses: signals of their own, never keywords.
                    cut = true;
                    prev_word = None;
                    continue;
                }
            }
            let surface = np.token_text(i);
            let key = np.key(i);
            let stem = stem(key, self.lang);
            let Some(w) = self.word(&stem, key) else {
                cut = true;
                prev_word = None;
                continue;
            };
            // A capitalized word starting a line ends a heading or a table cell.
            let line_start = text
                .get(..s)
                .and_then(|b| b.chars().rev().find(|c| *c != ' '))
                .is_none_or(|c| c == '\n');
            let first = surface.chars().next();
            let capital = first.is_some_and(char::is_uppercase);
            let all_caps =
                surface.chars().count() >= 2 && surface.chars().all(|c| !c.is_lowercase());
            if line_start && capital && !sentence_start {
                cut = true;
                prev_word = None;
            }
            let content = !self.words[w as usize].stop
                && (key.chars().count() >= 3 || (all_caps && key.chars().count() == 2));
            {
                let st = &mut self.words[w as usize];
                st.tf += 1;
                if capital && !all_caps && !sentence_start {
                    st.upper += 1;
                }
                if st.last_sentence != self.sentence {
                    st.last_sentence = self.sentence;
                    st.sentences += 1;
                    if st.positions.len() < MAX_POSITIONS {
                        st.positions.push(self.sentence);
                    }
                }
            }
            if let Some(pw) = prev_word {
                self.words[pw as usize].right_total += 1;
                self.words[w as usize].left_total += 1;
                let pair = (u64::from(pw) << 32) | u64::from(w);
                if self.pairs.len() < MAX_PHRASES * 4 && self.pairs.insert(pair) {
                    self.words[pw as usize].right += 1;
                    self.words[w as usize].left += 1;
                }
            }
            prev_word = Some(w);
            sentence_start = false;
            if content {
                self.content_words += 1;
            }
            seq.push(Tok {
                word: w,
                start: t.start,
                end: t.end,
                cut_before: cut || !content,
                content,
            });
            caps_flags.push(all_caps);
            cut = !content;
        }
        // Acronyms: all capitals, not inside a run of capitals (a heading or a shouted clause).
        for (j, t) in seq.iter().enumerate() {
            if !caps_flags[j] || !t.content {
                continue;
            }
            let near =
                |k: Option<usize>| k.and_then(|k| caps_flags.get(k)).copied().unwrap_or(false);
            if !near(j.checked_sub(1)) && !near(Some(j + 1)) {
                self.words[t.word as usize].acronym += 1;
            }
        }
        // Phrases: every 1–3 word window inside a run of content words.
        let mut counts: IdMap<u32, u32> = IdMap::default();
        let mut run_start = 0usize;
        for j in 0..seq.len() {
            if seq[j].cut_before || !seq[j].content {
                run_start = j;
            }
            if !seq[j].content {
                run_start = j + 1;
                continue;
            }
            for n in 1..=3usize {
                if j + 1 < n || j + 1 - n < run_start {
                    break;
                }
                let a = j + 1 - n;
                let mut key = [NONE; 3];
                for (k, slot) in key.iter_mut().enumerate().take(n) {
                    *slot = seq[a + k].word;
                }
                let caps = caps_flags[a..=j].iter().all(|&c| c);
                #[allow(clippy::cast_possible_truncation)] // n ≤ 3
                let got = self.phrase(key, n as u8, index, (pos, seq[a].start, seq[j].end), caps);
                if let Some(id) = got.filter(|_| n <= 2) {
                    *counts.entry(id).or_insert(0) += 1;
                }
            }
        }
        let mut v: Vec<(u32, u32)> = counts.into_iter().collect();
        v.sort_unstable();
        v
    }
}

/// Builds the term index of `doc`, taking each page from `shared` when it holds that whole
/// page (else normalizing it here: same result). Stops at `deadline`.
pub fn build(
    doc: &Document,
    shared: Option<&NormalizedText>,
    lang: StemLang,
    deadline: Option<Instant>,
) -> TermIndex {
    let furniture = furniture(doc);
    let mut b = Builder {
        lang,
        word_ids: HashMap::new(),
        words: Vec::new(),
        phrase_ids: IdMap::default(),
        phrases: Vec::new(),
        pairs: IdSet::default(),
        sentence: 0,
        content_words: 0,
    };
    let mut pages = Vec::with_capacity(doc.pages.len());
    let mut truncated = false;
    let mut furniture_lines = 0u32;
    for (pos, p) in doc.pages.iter().enumerate() {
        if pos % 16 == 15 && deadline.is_some_and(|d| Instant::now() >= d) {
            truncated = true;
            break;
        }
        if p.text_source == TextSource::None && p.text.trim().is_empty() {
            pages.push((p.index, Vec::new()));
            continue;
        }
        let own;
        let np = if let Some(n) = shared.and_then(|n| n.page(pos, &p.text)) {
            n
        } else {
            own = NormalizedPage::new(&p.text);
            &own
        };
        let skip = furniture_ranges(np.text(), &furniture);
        furniture_lines += u32::try_from(skip.len()).unwrap_or(u32::MAX);
        let pos32 = u32::try_from(pos).unwrap_or(u32::MAX);
        let counts = b.page(pos32, p.index, np, &skip);
        // A sentence never runs across a page.
        b.sentence = b.sentence.saturating_add(1);
        pages.push((p.index, counts));
    }
    TermIndex {
        lang,
        words: b.words,
        phrases: b.phrases,
        pages,
        sentences: b.sentence.max(1),
        content_words: b.content_words,
        truncated,
        furniture_lines,
    }
}

impl TermIndex {
    /// The display form of phrase `id`: its first occurrence as written, lowercased (an
    /// acronym of at most 6 letters keeps its capitals), whitespace collapsed.
    pub fn surface(&self, doc: &Document, shared: Option<&NormalizedText>, id: u32) -> String {
        self.surfaces(doc, shared, &[id])
            .remove(&id)
            .unwrap_or_default()
    }

    /// [`Self::surface`] of every id in `ids`, normalizing each page needed once.
    pub fn surfaces(
        &self,
        doc: &Document,
        shared: Option<&NormalizedText>,
        ids: &[u32],
    ) -> HashMap<u32, String> {
        let mut by_page: Vec<(u32, u32)> = ids
            .iter()
            .filter_map(|&id| Some((self.phrases.get(id as usize)?.first_at.0, id)))
            .collect();
        by_page.sort_unstable();
        by_page.dedup();
        let mut out = HashMap::new();
        let mut i = 0;
        while i < by_page.len() {
            let pos = by_page[i].0;
            let Some(p) = doc.pages.get(pos as usize) else {
                i += 1;
                continue;
            };
            let own;
            let np = if let Some(n) = shared.and_then(|n| n.page(pos as usize, &p.text)) {
                n
            } else {
                own = NormalizedPage::new(&p.text);
                &own
            };
            while i < by_page.len() && by_page[i].0 == pos {
                let id = by_page[i].1;
                let ph = &self.phrases[id as usize];
                let (_, a, e) = ph.first_at;
                let raw = np.text().get(a as usize..e as usize).unwrap_or("");
                let acronym: Vec<bool> = ph
                    .word_ids()
                    .iter()
                    .map(|&w| {
                        self.words
                            .get(w as usize)
                            .is_some_and(|s| s.acronym * 2 >= s.tf)
                    })
                    .collect();
                out.insert(id, display(raw, &acronym));
                i += 1;
            }
        }
        out
    }

    /// The matching key of phrase `id`: its stems joined by spaces (ASCII for Latin and
    /// Serbian Cyrillic text alike).
    pub fn key(&self, id: u32) -> String {
        let Some(ph) = self.phrases.get(id as usize) else {
            return String::new();
        };
        ph.word_ids()
            .iter()
            .filter_map(|&w| self.words.get(w as usize).map(|s| s.stem.as_str()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Distinct pages with text.
    pub fn text_pages(&self) -> usize {
        self.pages.iter().filter(|(_, c)| !c.is_empty()).count()
    }
}

/// `raw` lowercased, whitespace collapsed; a word of 2–6 letters keeps its capitals when it
/// is mostly written as an acronym (`acronym[i]`, one flag per word of `raw`).
fn display(raw: &str, acronym: &[bool]) -> String {
    let words: Vec<&str> = raw
        .split(char::is_whitespace)
        .filter(|w| !w.is_empty())
        .collect();
    let aligned = words.len() == acronym.len();
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let letters = w.chars().filter(|c| c.is_alphabetic()).count();
            let caps = (2..=6).contains(&letters) && w.chars().all(|c| !c.is_lowercase());
            if caps && aligned && acronym[i] {
                (*w).to_owned()
            } else {
                w.to_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lines among the first or last three of each page, keyed by their folded letters.
fn edge_lines(text: &str) -> Vec<(usize, usize)> {
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut off = 0usize;
    for l in text.split('\n') {
        // A running header or footer is short; a repeated sentence is body text.
        if !l.trim().is_empty() && l.split_whitespace().count() <= 12 {
            lines.push((off, off + l.len()));
        }
        off += l.len() + 1;
    }
    if lines.len() <= 6 {
        return lines;
    }
    let n = lines.len();
    let mut out: Vec<(usize, usize)> = lines[..3].to_vec();
    out.extend_from_slice(&lines[n - 3..]);
    out
}

fn line_key(line: &str) -> String {
    let letters: String = line
        .chars()
        .filter(|c| c.is_alphabetic() || *c == ' ')
        .collect();
    let k = fold(letters.trim(), &FoldOptions::KEY);
    k.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keys of the running header and footer lines of `doc`.
fn furniture(doc: &Document) -> HashSet<String> {
    let mut seen: HashMap<String, (u32, usize)> = HashMap::new();
    let mut text_pages = 0usize;
    for (pos, p) in doc.pages.iter().enumerate() {
        if p.text.trim().is_empty() {
            continue;
        }
        text_pages += 1;
        for (a, e) in edge_lines(&p.text) {
            let k = line_key(p.text.get(a..e).unwrap_or(""));
            if k.len() < 3 {
                continue;
            }
            let entry = seen.entry(k).or_insert((0, usize::MAX));
            if entry.1 != pos {
                entry.0 += 1;
                entry.1 = pos;
            }
        }
    }
    let min = 3u32.max(u32::try_from(text_pages / 10).unwrap_or(u32::MAX));
    seen.into_iter()
        .filter(|(_, (n, _))| *n >= min)
        .map(|(k, _)| k)
        .collect()
}

/// Byte ranges of the normalized page `text` that are furniture lines, sorted.
fn furniture_ranges(text: &str, furniture: &HashSet<String>) -> Vec<(usize, usize)> {
    if furniture.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<(usize, usize)> = edge_lines(text)
        .into_iter()
        .filter(|&(a, e)| furniture.contains(&line_key(text.get(a..e).unwrap_or(""))))
        .collect();
    v.sort_unstable();
    v
}

/// Lemmatization-lite of a folded key (see the [module docs](self)). Borrows when nothing
/// changes.
pub fn stem(key: &str, lang: StemLang) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    let n = key.chars().count();
    match lang {
        StemLang::None => Cow::Borrowed(key),
        StemLang::English => {
            let key = key.strip_suffix("'s").unwrap_or(key);
            if n >= 5 && key.ends_with("ies") && !key.ends_with("eies") {
                return Cow::Owned(format!("{}y", &key[..key.len() - 3]));
            }
            if n >= 4
                && key.ends_with('s')
                && !key.ends_with("ss")
                && !key.ends_with("us")
                && !key.ends_with("is")
                && !key.ends_with("ous")
            {
                return Cow::Borrowed(&key[..key.len() - 1]);
            }
            Cow::Borrowed(key)
        }
        StemLang::Hbs => {
            if n < 6 || !key.is_ascii() {
                return Cow::Borrowed(key);
            }
            for suf in HBS_SUFFIXES {
                if let Some(s) = key.strip_suffix(suf)
                    && s.len() >= 4
                    && s.bytes()
                        .last()
                        .is_some_and(|b| !b"aeiou".contains(&b) || suf.len() == 1)
                {
                    return Cow::Borrowed(s);
                }
            }
            Cow::Borrowed(key)
        }
    }
}

/// Serbian / Croatian / Bosnian case endings (folded), longest first. Conservative: noun and
/// adjective endings only; verbs and derivations are left alone.
const HBS_SUFFIXES: &[&str] = &[
    "ijama", "ama", "ima", "ega", "emu", "oga", "omu", "ih", "im", "om", "em", "og", "oj", "a",
    "e", "i", "u", "o",
];

fn is_stop(k: &str) -> bool {
    STOP_EN.binary_search(&k).is_ok() || STOP_HBS.binary_search(&k).is_ok()
}

/// English stop words, legal boilerplate, months and days, Roman numerals, legal forms
/// (sorted, folded).
pub(crate) const STOP_EN: &[&str] = &[
    "a",
    "about",
    "above",
    "accordance",
    "according",
    "across",
    "actually",
    "after",
    "afterwards",
    "ag",
    "again",
    "against",
    "agree",
    "agreed",
    "agrees",
    "all",
    "almost",
    "along",
    "already",
    "also",
    "although",
    "always",
    "am",
    "among",
    "amongst",
    "an",
    "and",
    "another",
    "any",
    "anyone",
    "anything",
    "applicable",
    "april",
    "are",
    "aren't",
    "around",
    "as",
    "at",
    "august",
    "b",
    "be",
    "became",
    "because",
    "become",
    "becomes",
    "been",
    "before",
    "behalf",
    "being",
    "below",
    "besides",
    "between",
    "beyond",
    "both",
    "but",
    "by",
    "c",
    "can",
    "can't",
    "cannot",
    "chapter",
    "co",
    "corp",
    "could",
    "d",
    "december",
    "did",
    "didn't",
    "do",
    "does",
    "doesn't",
    "doing",
    "don't",
    "done",
    "down",
    "due",
    "during",
    "e",
    "each",
    "eg",
    "either",
    "else",
    "et",
    "etc",
    "even",
    "every",
    "everything",
    "example",
    "except",
    "f",
    "february",
    "few",
    "first",
    "following",
    "for",
    "forth",
    "friday",
    "from",
    "further",
    "furthermore",
    "g",
    "gmbh",
    "h",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hereafter",
    "hereby",
    "herein",
    "hereinabove",
    "hereinafter",
    "hereinbelow",
    "hereof",
    "hereto",
    "heretofore",
    "hereunder",
    "hereunto",
    "herewith",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "however",
    "i",
    "i'd",
    "i'm",
    "i've",
    "ie",
    "if",
    "ii",
    "iii",
    "in",
    "inc",
    "include",
    "includes",
    "including",
    "into",
    "is",
    "isn't",
    "it",
    "it'll",
    "it's",
    "its",
    "itself",
    "iv",
    "ix",
    "j",
    "january",
    "july",
    "june",
    "just",
    "k",
    "l",
    "less",
    "let's",
    "llc",
    "llp",
    "lp",
    "ltd",
    "m",
    "made",
    "make",
    "many",
    "march",
    "may",
    "maybe",
    "me",
    "means",
    "might",
    "mine",
    "monday",
    "more",
    "moreover",
    "most",
    "much",
    "must",
    "my",
    "n",
    "namely",
    "neither",
    "never",
    "nevertheless",
    "no",
    "non",
    "none",
    "nor",
    "not",
    "nothing",
    "notwithstanding",
    "november",
    "now",
    "o",
    "october",
    "of",
    "off",
    "often",
    "on",
    "once",
    "one",
    "only",
    "onto",
    "or",
    "other",
    "others",
    "otherwise",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "p",
    "page",
    "pages",
    "per",
    "perhaps",
    "plc",
    "probably",
    "provide",
    "provided",
    "provides",
    "providing",
    "pursuant",
    "q",
    "quite",
    "r",
    "rather",
    "really",
    "regarding",
    "respect",
    "respectively",
    "s",
    "said",
    "same",
    "saturday",
    "second",
    "see",
    "september",
    "set",
    "shall",
    "she",
    "should",
    "simply",
    "since",
    "so",
    "some",
    "someone",
    "something",
    "sometimes",
    "somewhat",
    "such",
    "sunday",
    "t",
    "than",
    "that",
    "that's",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "thence",
    "there",
    "there's",
    "thereafter",
    "thereby",
    "therefor",
    "therefore",
    "therein",
    "thereinafter",
    "thereof",
    "thereon",
    "thereto",
    "thereunder",
    "thereupon",
    "these",
    "they",
    "they're",
    "third",
    "this",
    "those",
    "though",
    "through",
    "throughout",
    "thursday",
    "thus",
    "to",
    "together",
    "too",
    "toward",
    "towards",
    "tuesday",
    "two",
    "u",
    "under",
    "unless",
    "until",
    "unto",
    "up",
    "upon",
    "us",
    "use",
    "used",
    "usually",
    "v",
    "very",
    "vi",
    "via",
    "vii",
    "viii",
    "w",
    "was",
    "wasn't",
    "we",
    "we'd",
    "we'll",
    "we're",
    "we've",
    "wednesday",
    "well",
    "were",
    "what",
    "whatever",
    "whatsoever",
    "when",
    "whenever",
    "where",
    "whereas",
    "whereby",
    "wherein",
    "whereof",
    "whether",
    "which",
    "while",
    "who",
    "whoever",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "within",
    "without",
    "witnesseth",
    "won't",
    "would",
    "x",
    "xi",
    "xii",
    "y",
    "yet",
    "you",
    "you'd",
    "you'll",
    "you're",
    "you've",
    "your",
    "yours",
    "z",
];

/// Serbian, Croatian, Bosnian stop words (folded: Latin, no diacritics), months, legal forms
/// (sorted).
pub(crate) const STOP_HBS: &[&str] = &[
    "a",
    "ad",
    "ako",
    "ali",
    "april",
    "aprila",
    "avgust",
    "avgusta",
    "bez",
    "bi",
    "bih",
    "bila",
    "bili",
    "bilo",
    "bio",
    "biti",
    "br",
    "broj",
    "bude",
    "budu",
    "ce",
    "cemo",
    "ces",
    "cete",
    "cu",
    "da",
    "dalje",
    "daljem",
    "dana",
    "decembar",
    "decembra",
    "din",
    "dinara",
    "do",
    "dok",
    "doo",
    "druga",
    "druge",
    "drugi",
    "drugom",
    "drugu",
    "februar",
    "februara",
    "ga",
    "gde",
    "gdje",
    "god",
    "godine",
    "i",
    "ih",
    "ili",
    "im",
    "itd",
    "iz",
    "izmedju",
    "ja",
    "januar",
    "januara",
    "je",
    "jedan",
    "jedna",
    "jedne",
    "jednog",
    "jer",
    "jesu",
    "joj",
    "jos",
    "jul",
    "jula",
    "jun",
    "juna",
    "kad",
    "kada",
    "kako",
    "kao",
    "kod",
    "koja",
    "koje",
    "kojeg",
    "kojem",
    "koji",
    "kojih",
    "kojim",
    "kojima",
    "kojoj",
    "koju",
    "kom",
    "kroz",
    "li",
    "maj",
    "maja",
    "mart",
    "marta",
    "me",
    "mi",
    "moci",
    "mogu",
    "mora",
    "moraju",
    "moze",
    "mu",
    "na",
    "nad",
    "nakon",
    "nam",
    "nas",
    "navedene",
    "navedeni",
    "navedenog",
    "ne",
    "nego",
    "neka",
    "neki",
    "nekog",
    "nema",
    "ni",
    "nije",
    "nisu",
    "njega",
    "njegov",
    "njegova",
    "njegove",
    "njemu",
    "njen",
    "njena",
    "njih",
    "njihov",
    "njihova",
    "njihove",
    "njima",
    "novembar",
    "novembra",
    "o",
    "od",
    "odnosno",
    "oktobar",
    "oktobra",
    "on",
    "ona",
    "one",
    "oni",
    "ono",
    "osim",
    "ova",
    "ovaj",
    "ove",
    "ovih",
    "ovim",
    "ovo",
    "ovog",
    "ovoj",
    "ovom",
    "pa",
    "pak",
    "pdv",
    "po",
    "pod",
    "potrebno",
    "pred",
    "preko",
    "prema",
    "pri",
    "protiv",
    "radi",
    "rsd",
    "s",
    "sa",
    "saglasno",
    "sam",
    "samo",
    "se",
    "septembar",
    "septembra",
    "shodno",
    "si",
    "sl",
    "smo",
    "st",
    "ste",
    "sto",
    "su",
    "svaka",
    "svake",
    "svaki",
    "sve",
    "svi",
    "svih",
    "svoj",
    "svoja",
    "svoje",
    "svojih",
    "svojim",
    "svom",
    "ta",
    "taj",
    "tako",
    "takodje",
    "te",
    "tek",
    "tekstu",
    "ti",
    "to",
    "tog",
    "toj",
    "tokom",
    "tom",
    "treba",
    "tu",
    "u",
    "ukoliko",
    "usled",
    "uz",
    "vam",
    "vas",
    "vec",
    "vi",
    "za",
    "zbog",
];

/// Generic words of documents and contracts (folded, stemmed): ranked down, never dropped.
pub(crate) const GENERIC: &[&str] = &[
    "agreement",
    "amendment",
    "amount",
    "annex",
    "applicable",
    "article",
    "basis",
    "case",
    "clan",
    "clause",
    "company",
    "condition",
    "contract",
    "cost",
    "dan",
    "date",
    "day",
    "detail",
    "document",
    "dokumentacij",
    "effect",
    "event",
    "exhibit",
    "extent",
    "form",
    "information",
    "item",
    "javn",
    "javna",
    "javne",
    "javni",
    "javnih",
    "javnoj",
    "javnu",
    "konkursn",
    "konkursna",
    "law",
    "lic",
    "lice",
    "manner",
    "matter",
    "member",
    "nabavk",
    "narucilac",
    "narucioc",
    "notice",
    "number",
    "obavez",
    "obaveza",
    "obligation",
    "obrasc",
    "obrazac",
    "odredb",
    "odredba",
    "part",
    "party",
    "period",
    "person",
    "place",
    "ponud",
    "ponudjac",
    "postupak",
    "postupk",
    "prav",
    "pravo",
    "predmet",
    "provision",
    "purpose",
    "reason",
    "request",
    "requirement",
    "right",
    "schedule",
    "section",
    "stav",
    "stran",
    "strana",
    "tack",
    "tacka",
    "term",
    "thing",
    "time",
    "ugovor",
    "ugovorn",
    "uslov",
    "way",
    "written",
    "year",
    "zakon",
];

#[cfg(test)]
#[allow(clippy::many_single_char_names)] // short names for test fixtures
mod tests {
    use super::*;
    use quail_core::{Page, PageKind};

    pub(crate) fn doc(pages: &[&str]) -> Document {
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

    #[test]
    fn stop_lists_are_sorted() {
        for l in [STOP_EN, STOP_HBS] {
            assert!(
                l.windows(2).all(|w| w[0] < w[1]),
                "{:?}",
                l.windows(2).find(|w| w[0] >= w[1])
            );
        }
    }

    #[test]
    fn stems_are_conservative() {
        let hbs = |k| stem(k, StemLang::Hbs).into_owned();
        assert_eq!(hbs("ugovora"), "ugovor");
        assert_eq!(hbs("ugovorom"), "ugovor");
        assert_eq!(hbs("ugovorima"), "ugovor");
        assert_eq!(hbs("ugovor"), "ugovor");
        assert_eq!(hbs("nabavke"), "nabavk");
        assert_eq!(hbs("nabavku"), "nabavk");
        assert_eq!(hbs("odgovornosti"), "odgovornost");
        // Short words and stems are kept.
        assert_eq!(hbs("voda"), "voda");
        assert_eq!(hbs("cena"), "cena");
        let en = |k| stem(k, StemLang::English).into_owned();
        assert_eq!(en("services"), "service");
        assert_eq!(en("parties"), "party");
        assert_eq!(en("business"), "business");
        assert_eq!(en("status"), "status");
        assert_eq!(en("analysis"), "analysis");
        assert_eq!(en("tenant's"), "tenant");
        assert_eq!(stem("services", StemLang::None), "services");
    }

    #[test]
    fn latin_and_cyrillic_are_one_word() {
        let d = doc(&[
            "Ugovor o nabavci opreme. Isporuka opreme je predmet ugovora.",
            "Уговор о набавци опреме. Испорука опреме је предмет уговора.",
        ]);
        let t = build(&d, None, StemLang::Hbs, None);
        let opreme: Vec<_> = t.words.iter().filter(|w| w.stem == "oprem").collect();
        assert_eq!(opreme.len(), 1);
        assert_eq!(opreme[0].tf, 4);
        assert!(t.words.iter().all(|w| w.stem.is_ascii()));
    }

    #[test]
    fn phrases_stop_at_stop_words_numbers_and_punctuation() {
        let d = doc(&["The software license covers 12 hosted servers, and support services."]);
        let t = build(&d, None, StemLang::English, None);
        let keys: Vec<String> = (0..t.phrases.len())
            .map(|i| t.key(u32::try_from(i).unwrap()))
            .collect();
        assert!(
            keys.contains(&"software license cover".to_owned()),
            "{keys:?}"
        );
        assert!(keys.contains(&"hosted server".to_owned()), "{keys:?}");
        assert!(keys.contains(&"support service".to_owned()), "{keys:?}");
        assert!(!keys.iter().any(|k| k.contains("cover hosted")), "{keys:?}");
        assert!(
            !keys.iter().any(|k| k.contains("server support")),
            "{keys:?}"
        );
        assert!(!keys.iter().any(|k| k.contains("12")), "{keys:?}");
    }

    #[test]
    fn running_headers_are_skipped() {
        let pages: Vec<String> = (0..10)
            .map(|i| {
                let w = ["north", "south", "east", "west", "inner"][i % 5];
                format!("Harborview Quarterly Bulletin\nTidal gauges measure {w} lagoon levels near station {i}.\nPage {i}")
            })
            .collect();
        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        let t = build(&doc(&refs), None, StemLang::English, None);
        assert!(t.furniture_lines >= 10);
        let bulletin = t.words.iter().find(|w| w.stem == "bulletin");
        assert!(bulletin.is_none_or(|w| w.tf == 0), "{bulletin:?}");
        assert!(t.words.iter().any(|w| w.stem == "lagoon" && w.tf == 10));
    }

    #[test]
    fn deadline_marks_truncated() {
        let pages: Vec<String> = (0..40)
            .map(|i| format!("Coral reef survey number {i} of the lagoon."))
            .collect();
        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        let past = Instant::now().checked_sub(std::time::Duration::from_secs(1));
        let t = build(&doc(&refs), None, StemLang::English, past);
        assert!(t.truncated);
        assert!(t.pages.len() < 40);
    }
}

//! Language identification with scored candidates.
//!
//! # Backend
//!
//! [Lingua](https://github.com/pemistahl/lingua-rs) in high-accuracy mode, restricted to
//! the languages in [`Lang`] (Serbian, Croatian, Bosnian, Slovene, Macedonian, Russian,
//! English, German, French, Italian, Spanish). Chosen over `whatlang` and over Lingua's
//! low-accuracy mode by a benchmark (numbers in the crate README): it is the most accurate
//! on short text, the only one that knows Bosnian, returns a full score distribution, and
//! with 11 languages its models take about 50 MB of RAM (all 75 Lingua languages in
//! high-accuracy mode take about 1 GB, which is why the set is restricted). Lingua keeps its
//! models in a process-wide, read-only-after-load cache shared by every detector.
//!
//! Text is sampled before detection ([`DetectorOptions::max_chars`], taken from the start,
//! middle and end of the text) so cost per document is bounded: about 1 ms for the default
//! 3 000 characters.
//!
//! # Serbian, Croatian, Bosnian
//!
//! The three standards share most of their vocabulary and grammar, and both Lingua and
//! whatlang only know *Cyrillic* Serbian: Latin-script Serbian always comes out as Croatian
//! or Bosnian. quail therefore treats {sr, hr, bs} as one cluster: the backend decides how
//! much probability the cluster gets, and a lexical layer ([`hbs_markers`]) splits it using
//! markers that differ between the standards:
//!
//! - **yat reflex**: ekavian `mesec, vreme, pre, posle, deo, rešenje, sledeći, primerak`
//!   (Serbian) vs ijekavian `mjesec, vrijeme, prije, poslije, dio, rješenje, sljedeći,
//!   primjerak` (Croatian and Bosnian, and also Serbian as written in Montenegro and the
//!   Republika Srpska, which is why yat alone never decides Croatian or Bosnian);
//! - **lexicon**: Croatian `tisuća, tjedan, tko, kruh, obveza, poduzeće, ravnatelj, tvrtka,
//!   natječaj, točno, posto` and the Croatian month names; Bosnian `historija, sedmica` +
//!   ijekavian, `hljeb, uposlenik, lahko, kahva`; Serbian `opština, opšti, takođe, istorija,
//!   poreski`; shared Serbian/Bosnian `hiljada, obaveza, preduzeće, tačno, procenat,
//!   evropski, fabrika` and agent nouns in `-lac` (`naručilac`) vs Croatian `-telj`
//!   (`naručitelj`);
//! - **syntax**: modal + `da` + present (`može da vrati`, Serbian and Bosnian) vs modal +
//!   infinitive (`može vratiti`, Croatian and Bosnian); fused future plurals
//!   (`uradićemo`, Serbian);
//! - **script**: Cyrillic letters make it Serbian (Lingua's Serbian model is Cyrillic).
//!
//! Without markers (short or formulaic text), the cluster's mass is split by
//! [`DetectorOptions::hbs_prior`] (uniform by default) and Lingua's own hr/bs preference, so
//! a caller sees three close candidates instead of a false verdict: report candidates,
//! not verdicts.

use std::collections::HashMap;
use std::sync::Arc;

use lingua::{Language, LanguageDetector as Lingua, LanguageDetectorBuilder};

use crate::fold::{FoldOptions, fold_into};
use crate::script::{Script, ScriptCounts};
use crate::token::{TokenKind, tokens};

/// A language quail can identify.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lang {
    /// Serbian.
    Sr,
    /// Croatian.
    Hr,
    /// Bosnian.
    Bs,
    /// Slovene.
    Sl,
    /// Macedonian.
    Mk,
    /// Russian.
    Ru,
    /// English.
    En,
    /// German.
    De,
    /// French.
    Fr,
    /// Italian.
    It,
    /// Spanish.
    Es,
}

impl Lang {
    /// Every supported language.
    pub const ALL: [Self; 11] = [
        Self::Sr,
        Self::Hr,
        Self::Bs,
        Self::Sl,
        Self::Mk,
        Self::Ru,
        Self::En,
        Self::De,
        Self::Fr,
        Self::It,
        Self::Es,
    ];

    /// ISO 639-1 code (`sr`, `hr`, ...).
    pub fn code(self) -> &'static str {
        match self {
            Self::Sr => "sr",
            Self::Hr => "hr",
            Self::Bs => "bs",
            Self::Sl => "sl",
            Self::Mk => "mk",
            Self::Ru => "ru",
            Self::En => "en",
            Self::De => "de",
            Self::Fr => "fr",
            Self::It => "it",
            Self::Es => "es",
        }
    }

    /// Whether it is one of Serbian, Croatian, Bosnian.
    pub fn is_hbs(self) -> bool {
        matches!(self, Self::Sr | Self::Hr | Self::Bs)
    }

    fn lingua(self) -> Language {
        match self {
            Self::Sr => Language::Serbian,
            Self::Hr => Language::Croatian,
            Self::Bs => Language::Bosnian,
            Self::Sl => Language::Slovene,
            Self::Mk => Language::Macedonian,
            Self::Ru => Language::Russian,
            Self::En => Language::English,
            Self::De => Language::German,
            Self::Fr => Language::French,
            Self::It => Language::Italian,
            Self::Es => Language::Spanish,
        }
    }

    fn from_lingua(l: Language) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.lingua() == l)
    }
}

/// A candidate language and its score (a probability: scores of one result sum to ≤ 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LangScore {
    /// The language.
    pub lang: Lang,
    /// Its probability.
    pub score: f32,
}

/// Result of [`LanguageDetector::detect`].
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Candidates, best first, every supported language with a non-zero score.
    pub candidates: Vec<LangScore>,
    /// Marker counts that split the sr/hr/bs cluster, when it was considered.
    pub hbs: Option<HbsMarkers>,
}

impl Detection {
    /// The best candidate.
    pub fn best(&self) -> Option<LangScore> {
        self.candidates.first().copied()
    }

    /// The `n` best candidates.
    pub fn top(&self, n: usize) -> &[LangScore] {
        &self.candidates[..n.min(self.candidates.len())]
    }
}

/// Options of [`LanguageDetector`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorOptions {
    /// Characters sampled for the backend (default 3 000).
    pub max_chars: usize,
    /// Characters scanned for sr/hr/bs markers (default 50 000).
    pub max_marker_chars: usize,
    /// Fewer letters than this: no detection (default 8).
    pub min_letters: u64,
    /// Prior weight of sr, hr, bs inside the cluster (default uniform). A deployment that
    /// knows its documents are mostly Serbian can tilt it, e.g. `[2.0, 1.0, 1.0]`.
    pub hbs_prior: [f32; 3],
}

impl Default for DetectorOptions {
    fn default() -> Self {
        Self {
            max_chars: 3_000,
            max_marker_chars: 50_000,
            min_letters: 8,
            hbs_prior: [1.0, 1.0, 1.0],
        }
    }
}

/// A language detector. Build once (it loads models), share with `Clone` (an `Arc`) or
/// across threads by reference.
#[derive(Clone)]
pub struct LanguageDetector {
    inner: Arc<Lingua>,
    markers: Arc<MarkerIndex>,
    opts: DetectorOptions,
}

impl std::fmt::Debug for LanguageDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanguageDetector")
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl Default for LanguageDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageDetector {
    /// Detector with default options; preloads the models (about 50 MB).
    pub fn new() -> Self {
        Self::with_options(DetectorOptions::default())
    }

    /// Detector with explicit options.
    pub fn with_options(opts: DetectorOptions) -> Self {
        let langs: Vec<Language> = Lang::ALL.iter().map(|l| l.lingua()).collect();
        let inner = LanguageDetectorBuilder::from_languages(&langs)
            .with_preloaded_language_models()
            .build();
        Self {
            inner: Arc::new(inner),
            markers: Arc::new(MarkerIndex::new()),
            opts,
        }
    }

    /// Options in use.
    pub fn options(&self) -> &DetectorOptions {
        &self.opts
    }

    /// Detects the language of `text`. `None` when it has too few letters.
    pub fn detect(&self, text: &str) -> Option<Detection> {
        self.detect_limited(text, self.opts.max_chars)
    }

    /// Like [`Self::detect`], sampling at most `max_chars` characters for the backend
    /// (e.g. a smaller sample per page).
    pub fn detect_limited(&self, text: &str, max_chars: usize) -> Option<Detection> {
        let scripts = ScriptCounts::of(text);
        if scripts.letters() < self.opts.min_letters {
            return None;
        }
        let sample = sample(text, max_chars);
        let mut scores: Vec<(Lang, f64)> = self
            .inner
            .compute_language_confidence_values(sample)
            .into_iter()
            .filter_map(|(l, s)| Lang::from_lingua(l).map(|l| (l, s)))
            .collect();
        let mut hbs = None;
        let mass: f64 = scores
            .iter()
            .filter(|(l, _)| l.is_hbs())
            .map(|(_, s)| s)
            .sum();
        if mass > 0.0 {
            let marker_text = prefix_chars(text, self.opts.max_marker_chars);
            let m = self.markers.scan(marker_text);
            let get = |l: Lang| scores.iter().find(|x| x.0 == l).map_or(0.0, |x| x.1);
            let split = split_cluster(
                [get(Lang::Sr), get(Lang::Hr), get(Lang::Bs)],
                &m,
                &scripts,
                self.opts.hbs_prior,
            );
            for (l, s) in [Lang::Sr, Lang::Hr, Lang::Bs].into_iter().zip(split) {
                let v = mass * s;
                match scores.iter_mut().find(|x| x.0 == l) {
                    Some(x) => x.1 = v,
                    None => scores.push((l, v)),
                }
            }
            hbs = Some(m);
        }
        scores.retain(|x| x.1 > 0.0);
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        #[allow(clippy::cast_possible_truncation)] // probabilities
        let candidates = scores
            .into_iter()
            .map(|(lang, s)| LangScore {
                lang,
                score: s as f32,
            })
            .collect();
        Some(Detection { candidates, hbs })
    }
}

/// The first `n` characters of `text` (on a char boundary).
fn prefix_chars(text: &str, n: usize) -> &str {
    text.char_indices().nth(n).map_or(text, |(i, _)| &text[..i])
}

/// Up to `max` characters: the whole text when short, else three windows (start, middle,
/// end) joined by spaces, each cut at whitespace.
fn sample(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let n_chars = text.chars().count();
    if n_chars <= max {
        return text.to_owned();
    }
    let w = (max / 3).max(1);
    let mut out = String::with_capacity(max * 2);
    for start_char in [0, n_chars / 2 - w / 2, n_chars - w] {
        let start = text
            .char_indices()
            .nth(start_char)
            .map_or(text.len(), |(i, _)| i);
        let window = prefix_chars(&text[start..], w);
        // Cut to whole words.
        let a = if start_char == 0 {
            0
        } else {
            window.find(char::is_whitespace).map_or(0, |i| i + 1)
        };
        let b = window
            .rfind(char::is_whitespace)
            .filter(|&b| b > a)
            .unwrap_or(window.len());
        out.push_str(&window[a..b]);
        out.push(' ');
    }
    out
}

/// Marker counts for the sr/hr/bs split, see the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HbsMarkers {
    /// Ekavian word forms (Serbian).
    pub ekavian: u32,
    /// Ijekavian word forms (Croatian, Bosnian, ijekavian Serbian).
    pub ijekavian: u32,
    /// Croatian-only words.
    pub hr: u32,
    /// Bosnian-only words.
    pub bs: u32,
    /// Serbian-only words.
    pub sr: u32,
    /// Words shared by Serbian and Bosnian but not Croatian.
    pub sr_bs: u32,
    /// Words shared by Croatian and Bosnian but not Serbian.
    pub hr_bs: u32,
    /// Cyrillic letters seen.
    pub cyrillic: bool,
}

#[derive(Clone, Copy, Debug)]
enum Class {
    Ek,
    Ij,
    Hr,
    Bs,
    Sr,
    SrBs,
    HrBs,
}

/// `(form, class, prefix)`: a token matches when it equals `form`, or starts with it when
/// `prefix` is true. Forms are lowercase with diacritics.
const MARKERS: &[(&str, Class, bool)] = &[
    // yat: ekavian
    ("mesec", Class::Ek, true),
    ("meseč", Class::Ek, true),
    ("mesto", Class::Ek, false),
    ("mesta", Class::Ek, false),
    ("mestu", Class::Ek, false),
    ("mestim", Class::Ek, true),
    ("mesn", Class::Ek, true),
    ("vreme", Class::Ek, false),
    ("pre", Class::Ek, false),
    ("posle", Class::Ek, false),
    ("ovde", Class::Ek, false),
    ("gde", Class::Ek, false),
    ("nigde", Class::Ek, false),
    ("deo", Class::Ek, false),
    ("delu", Class::Ek, false),
    ("cena", Class::Ek, false),
    ("cene", Class::Ek, false),
    ("cenu", Class::Ek, false),
    ("cenom", Class::Ek, false),
    ("rešen", Class::Ek, true),
    ("rešav", Class::Ek, true),
    ("reši", Class::Ek, true),
    ("sledeć", Class::Ek, true),
    ("sled", Class::Ek, false),
    ("sednic", Class::Ek, true),
    ("sedišt", Class::Ek, true),
    ("primer", Class::Ek, true),
    ("istovetn", Class::Ek, true),
    ("obe", Class::Ek, false),
    ("mleko", Class::Ek, true),
    ("mleč", Class::Ek, true),
    ("reka", Class::Ek, false),
    ("reke", Class::Ek, false),
    ("reku", Class::Ek, false),
    ("dete", Class::Ek, false),
    ("deca", Class::Ek, false),
    ("dece", Class::Ek, false),
    ("deci", Class::Ek, false),
    ("detet", Class::Ek, true),
    ("zahtev", Class::Ek, true),
    ("nedelj", Class::Ek, true),
    ("uvek", Class::Ek, false),
    ("celokup", Class::Ek, true),
    ("celin", Class::Ek, true),
    ("ceo", Class::Ek, false),
    ("promen", Class::Ek, true),
    ("odeljen", Class::Ek, true),
    ("svedo", Class::Ek, true),
    ("zamen", Class::Ek, true),
    ("izveštaj", Class::Ek, true),
    ("obavešt", Class::Ek, true),
    ("savet", Class::Ek, true),
    ("napred", Class::Ek, false),
    ("telo", Class::Ek, false),
    ("tela", Class::Ek, false),
    ("telu", Class::Ek, false),
    ("lekar", Class::Ek, true),
    ("belež", Class::Ek, true),
    ("beleš", Class::Ek, true),
    ("verovatn", Class::Ek, true),
    ("devojk", Class::Ek, true),
    ("videti", Class::Ek, false),
    ("vredn", Class::Ek, true),
    ("procena", Class::Ek, false),
    ("procene", Class::Ek, false),
    ("procenu", Class::Ek, false),
    ("smer", Class::Ek, true),
    ("pesm", Class::Ek, true),
    ("lepo", Class::Ek, false),
    ("hteo", Class::Ek, false),
    ("htela", Class::Ek, false),
    ("svetsk", Class::Ek, true),
    ("izmen", Class::Ek, true),
    ("predlog", Class::Ek, true),
    ("sreda", Class::Ek, false),
    ("sredu", Class::Ek, false),
    ("srede", Class::Ek, false),
    ("overen", Class::Ek, true),
    ("overa", Class::Ek, true),
    ("overi", Class::Ek, true),
    ("primedb", Class::Ek, true),
    ("delatn", Class::Ek, true),
    ("vek", Class::Ek, false),
    ("veka", Class::Ek, false),
    ("lekov", Class::Ek, true),
    ("vest", Class::Ek, false),
    ("vesti", Class::Ek, false),
    ("sneg", Class::Ek, true),
    ("seć", Class::Ek, true),
    ("uslov", Class::SrBs, true),
    // yat: ijekavian
    ("mjesec", Class::Ij, true),
    ("mjeseč", Class::Ij, true),
    ("mjest", Class::Ij, true),
    ("mjesn", Class::Ij, true),
    ("vrijeme", Class::Ij, false),
    ("prije", Class::Ij, false),
    ("poslije", Class::Ij, false),
    ("ovdje", Class::Ij, false),
    ("gdje", Class::Ij, false),
    ("nigdje", Class::Ij, false),
    ("dio", Class::Ij, false),
    ("dijel", Class::Ij, true),
    ("cijen", Class::Ij, true),
    ("rješ", Class::Ij, true),
    ("sljed", Class::Ij, true),
    ("sjednic", Class::Ij, true),
    ("sjedišt", Class::Ij, true),
    ("primjer", Class::Ij, true),
    ("istovjetn", Class::Ij, true),
    ("obje", Class::Ij, false),
    ("mlijek", Class::Ij, true),
    ("mliječ", Class::Ij, true),
    ("rijek", Class::Ij, true),
    ("dijete", Class::Ij, false),
    ("djec", Class::Ij, true),
    ("djetet", Class::Ij, true),
    ("zahtjev", Class::Ij, true),
    ("nedjelj", Class::Ij, true),
    ("uvijek", Class::Ij, false),
    ("cjelokup", Class::Ij, true),
    ("cjelin", Class::Ij, true),
    ("cijel", Class::Ij, true),
    ("promjen", Class::Ij, true),
    ("odjeljen", Class::Ij, true),
    ("svjedo", Class::Ij, true),
    ("zamjen", Class::Ij, true),
    ("izvještaj", Class::Ij, true),
    ("obavješt", Class::Ij, true),
    ("savjet", Class::Ij, true),
    ("naprijed", Class::Ij, false),
    ("tijel", Class::Ij, true),
    ("bilješ", Class::Ij, true),
    ("vjerojatn", Class::Hr, true),
    ("vjerovatn", Class::Ij, true),
    ("djevojk", Class::Ij, true),
    ("vidjeti", Class::Ij, false),
    ("vrijedn", Class::Ij, true),
    ("procjen", Class::Ij, true),
    ("smjer", Class::Ij, true),
    ("pjesm", Class::Ij, true),
    ("lijep", Class::Ij, true),
    ("htio", Class::Ij, false),
    ("htjela", Class::Ij, false),
    ("svijet", Class::Ij, true),
    ("svjetsk", Class::Ij, true),
    ("stoljeć", Class::Ij, true),
    ("djelatn", Class::Ij, true),
    ("posjed", Class::Ij, true),
    ("pobjed", Class::Ij, true),
    ("odjel", Class::Ij, true),
    ("vijeć", Class::Ij, true),
    ("sjever", Class::Ij, true),
    ("lijev", Class::Ij, true),
    ("bijel", Class::Ij, true),
    ("izmjen", Class::Ij, true),
    ("prijedlog", Class::Ij, true),
    ("srijed", Class::Ij, true),
    ("ovjer", Class::Ij, true),
    ("primjedb", Class::Ij, true),
    ("vijek", Class::Ij, true),
    ("lijek", Class::Ij, true),
    ("vijest", Class::Ij, true),
    ("snijeg", Class::Ij, true),
    ("sjeć", Class::Ij, true),
    ("ljekar", Class::Bs, true),
    // Croatian
    ("tisuć", Class::Hr, true),
    ("tjed", Class::Hr, true),
    ("tko", Class::Hr, false),
    ("kruh", Class::Hr, true),
    ("zrakoplov", Class::Hr, true),
    ("kolodvor", Class::Hr, true),
    ("ravnatelj", Class::Hr, true),
    ("tvrtk", Class::Hr, true),
    ("tvrtc", Class::Hr, true),
    ("natječaj", Class::Hr, true),
    ("primitk", Class::Hr, true),
    ("primitak", Class::Hr, true),
    ("obvez", Class::Hr, true),
    ("obiju", Class::Hr, false),
    ("uvjet", Class::Hr, true),
    ("posto", Class::Hr, false),
    ("postot", Class::Hr, true),
    ("glede", Class::Hr, false),
    ("unatoč", Class::Hr, false),
    ("poduzeć", Class::Hr, true),
    ("poduzetn", Class::Hr, true),
    ("tvornic", Class::Hr, true),
    ("povijest", Class::Hr, true),
    ("sveučilišt", Class::Hr, true),
    ("kazališt", Class::Hr, true),
    ("glazb", Class::Hr, true),
    ("nogomet", Class::Hr, true),
    ("točk", Class::Hr, true),
    ("točn", Class::Hr, true),
    ("europsk", Class::Hr, true),
    ("kemij", Class::Hr, true),
    ("siječ", Class::Hr, true),
    ("veljač", Class::Hr, true),
    ("ožuj", Class::Hr, true),
    ("travnj", Class::Hr, true),
    ("travanj", Class::Hr, false),
    ("svibanj", Class::Hr, false),
    ("svibnj", Class::Hr, true),
    ("lipanj", Class::Hr, false),
    ("lipnj", Class::Hr, true),
    ("srpanj", Class::Hr, false),
    ("srpnj", Class::Hr, true),
    ("kolovoz", Class::Hr, true),
    ("rujan", Class::Hr, false),
    ("rujn", Class::Hr, true),
    ("listopad", Class::Hr, true),
    ("prosinac", Class::Hr, false),
    ("prosinc", Class::Hr, true),
    ("naručitelj", Class::Hr, true),
    ("isporučitelj", Class::Hr, true),
    ("prodavatelj", Class::Hr, true),
    ("ponuditelj", Class::Hr, true),
    ("kuna", Class::Hr, false),
    ("porezn", Class::HrBs, true),
    ("opć", Class::HrBs, true),
    ("također", Class::HrBs, false),
    ("tijekom", Class::Hr, false),
    ("tijek", Class::Hr, true),
    ("milijun", Class::Hr, true),
    ("osobn", Class::Hr, true),
    ("iskaznic", Class::Hr, true),
    ("preslik", Class::Hr, true),
    // Bosnian
    ("historij", Class::Bs, true),
    ("hljeb", Class::Bs, true),
    ("uposlen", Class::Bs, true),
    ("lahk", Class::Bs, true),
    ("mehk", Class::Bs, true),
    ("kahv", Class::Bs, true),
    ("kanton", Class::Bs, true),
    ("federacij", Class::Bs, true),
    ("konvertibiln", Class::Bs, true),
    ("maraka", Class::Bs, false),
    ("bih", Class::Bs, false),
    ("sedmic", Class::SrBs, true),
    // Serbian
    ("opštin", Class::Sr, true),
    ("opšt", Class::Sr, true),
    ("takođe", Class::Sr, false),
    ("istorij", Class::Sr, true),
    ("poresk", Class::Sr, true),
    ("hleb", Class::Sr, true),
    ("dinar", Class::Sr, true),
    ("vaspitn", Class::Sr, true),
    ("železni", Class::Sr, true),
    // Serbian and Bosnian
    ("hiljad", Class::SrBs, true),
    ("obavez", Class::SrBs, true),
    ("preduzeć", Class::SrBs, true),
    ("preduzetn", Class::SrBs, true),
    ("tačk", Class::SrBs, true),
    ("tačn", Class::SrBs, true),
    ("procenat", Class::SrBs, false),
    ("procent", Class::SrBs, true),
    ("evropsk", Class::SrBs, true),
    ("fabrik", Class::SrBs, true),
    ("hemij", Class::SrBs, true),
    ("naručilac", Class::SrBs, false),
    ("naručioc", Class::SrBs, true),
    ("isporučilac", Class::SrBs, false),
    ("isporučioc", Class::SrBs, true),
    ("ponuđač", Class::SrBs, true),
    ("prodavac", Class::SrBs, false),
    ("zakupac", Class::SrBs, false),
    ("zakupc", Class::SrBs, true),
    ("prijem", Class::SrBs, true),
    ("prodavnic", Class::SrBs, true),
    ("milion", Class::SrBs, true),
    ("lična", Class::SrBs, false),
    ("lične", Class::SrBs, false),
    ("ličnu", Class::SrBs, false),
    ("lični", Class::SrBs, false),
    ("odsto", Class::Sr, false),
];

/// Modal words that take "da + present" in Serbian and an infinitive in Croatian.
const MODALS: &[&str] = &[
    "može",
    "mogu",
    "možemo",
    "mora",
    "moraju",
    "moramo",
    "treba",
    "trebaju",
    "želi",
    "žele",
    "dužan",
    "dužna",
    "dužni",
    "obavezan",
    "obavezna",
    "obvezan",
    "obvezna",
    "počinje",
    "nastavlja",
    "namerava",
    "namjerava",
];

/// Lookup tables over [`MARKERS`], built once per detector.
#[derive(Debug)]
struct MarkerIndex {
    exact: HashMap<&'static str, Class>,
    prefix: HashMap<&'static str, Class>,
    max_prefix: usize,
}

impl MarkerIndex {
    fn new() -> Self {
        let mut exact = HashMap::new();
        let mut prefix = HashMap::new();
        let mut max_prefix = 0;
        for &(form, class, is_prefix) in MARKERS {
            if is_prefix {
                prefix.insert(form, class);
                max_prefix = max_prefix.max(form.len());
            } else {
                exact.insert(form, class);
            }
        }
        Self {
            exact,
            prefix,
            max_prefix,
        }
    }

    /// Class of a folded word: an exact form, else its longest marker prefix.
    fn classify(&self, word: &str) -> Option<Class> {
        if let Some(&c) = self.exact.get(word) {
            return Some(c);
        }
        // Prefix ends from the longest down, on char boundaries, without allocating.
        std::iter::once(word.len())
            .chain(word.char_indices().rev().map(|(i, _)| i).filter(|&i| i > 0))
            .filter(|&e| e <= self.max_prefix)
            .find_map(|e| self.prefix.get(&word[..e]).copied())
    }

    fn scan(&self, text: &str) -> HbsMarkers {
        let mut m = HbsMarkers::default();
        let mut buf = String::with_capacity(32);
        let fold = FoldOptions {
            cyrillic: true,
            ..FoldOptions::CASE
        };
        let mut prev_modal = false;
        for t in tokens(text) {
            if t.kind != TokenKind::Word || t.text.len() < 2 {
                prev_modal = false;
                continue;
            }
            if !m.cyrillic
                && t.text
                    .chars()
                    .any(|c| crate::script::script_of(c) == Some(Script::Cyrillic))
            {
                m.cyrillic = true;
            }
            buf.clear();
            fold_into(t.text, &fold, &mut buf);
            // Modal + "da" + present (Serbian, Bosnian) vs modal + infinitive (Croatian,
            // Bosnian): "može da vrati" / "može vratiti".
            if prev_modal {
                if buf == "da" {
                    m.sr_bs += 1;
                } else if buf.ends_with("ti") || buf.ends_with("ći") {
                    m.hr_bs += 1;
                }
            }
            prev_modal = MODALS.contains(&buf.as_str());
            // Fused future (Serbian): "uradićemo", "održaće" is ambiguous with nouns, so
            // only the plural forms count.
            if buf.len() > 8
                && ["aćemo", "aćete", "ićemo", "ićete"]
                    .iter()
                    .any(|e| buf.ends_with(e))
            {
                m.sr += 1;
                continue;
            }
            match self.classify(&buf) {
                Some(Class::Ek) => m.ekavian += 1,
                Some(Class::Ij) => m.ijekavian += 1,
                Some(Class::Hr) => m.hr += 1,
                Some(Class::Bs) => m.bs += 1,
                Some(Class::Sr) => m.sr += 1,
                Some(Class::SrBs) => m.sr_bs += 1,
                Some(Class::HrBs) => m.hr_bs += 1,
                None => {}
            }
        }
        m
    }
}

/// Counts sr/hr/bs markers in `text` (Latin or Cyrillic; Cyrillic is transliterated).
/// Builds its lookup tables per call; [`LanguageDetector`] keeps them instead.
pub fn hbs_markers(text: &str) -> HbsMarkers {
    MarkerIndex::new().scan(text)
}

/// Log-score a marker class adds per log-count to the standards that use it.
const MARKER_WEIGHT: f64 = 1.6;

/// Splits the cluster mass among sr, hr, bs. Returns shares summing to 1.
fn split_cluster(
    backend: [f64; 3],
    markers: &HbsMarkers,
    scripts: &ScriptCounts,
    prior: [f32; 3],
) -> [f64; 3] {
    let lc = |n: u32| f64::from(n).ln_1p();
    let ek = lc(markers.ekavian);
    let ij = lc(markers.ijekavian);
    let (sr, hr, bs) = (lc(markers.sr), lc(markers.hr), lc(markers.bs));
    let (sr_bs, hr_bs) = (lc(markers.sr_bs), lc(markers.hr_bs));
    let mut score = [
        // Ijekavian Serbian exists: ijekavian evidence counts a little for sr too.
        MARKER_WEIGHT * (ek + sr + sr_bs + 0.3 * ij),
        MARKER_WEIGHT * (ij + hr + hr_bs),
        MARKER_WEIGHT * (ij + bs + sr_bs + hr_bs),
    ];
    // Cyrillic text: Serbian.
    #[allow(clippy::cast_precision_loss)] // a ratio
    let cyr_share = scripts.cyrillic as f64 / scripts.letters().max(1) as f64;
    if cyr_share > 0.3 {
        score[0] += 4.0;
    }
    // Backend preference: Lingua's sr is Cyrillic-only, so for Latin text only its hr/bs
    // ratio is informative.
    let total: f64 = backend.iter().sum();
    let hb = backend[1] + backend[2];
    let base = if total > 0.0 && cyr_share <= 0.3 && hb > 0.0 {
        [
            1.0 / 3.0,
            2.0 / 3.0 * backend[1] / hb,
            2.0 / 3.0 * backend[2] / hb,
        ]
    } else if total > 0.0 && cyr_share > 0.3 {
        [backend[0] / total, backend[1] / total, backend[2] / total]
    } else {
        [1.0 / 3.0; 3]
    };
    // Soften the backend (square root) so markers dominate when present.
    let weight: [f64; 3] = std::array::from_fn(|i| {
        f64::from(prior[i].max(0.0)) * (base[i] + 1e-3).sqrt() * score[i].exp()
    });
    let z: f64 = weight.iter().sum();
    if z > 0.0 && z.is_finite() {
        weight.map(|x| x / z)
    } else {
        [1.0 / 3.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    static DET: LazyLock<LanguageDetector> = LazyLock::new(LanguageDetector::new);

    fn best(t: &str) -> &'static str {
        DET.detect(t)
            .and_then(|d| d.best())
            .map_or("none", |b| b.lang.code())
    }

    #[test]
    fn major_languages() {
        assert_eq!(
            best("This agreement enters into force on the date of signature."),
            "en"
        );
        assert_eq!(
            best("Dieser Vertrag tritt mit dem Tag der Unterzeichnung in Kraft."),
            "de"
        );
        assert_eq!(
            best("Le présent contrat entre en vigueur à la date de sa signature."),
            "fr"
        );
        assert_eq!(
            best("Il presente contratto entra in vigore alla data della firma."),
            "it"
        );
        assert_eq!(
            best("El presente contrato entra en vigor en la fecha de su firma."),
            "es"
        );
    }

    #[test]
    fn serbian_both_scripts() {
        let lat = "Zakupac je dužan da plaća zakupninu do petog u mesecu, a rešenje o tome \
                   donosi opština posle sednice.";
        assert_eq!(best(lat), "sr");
        let cyr = crate::translit::latin_to_cyrillic(lat);
        assert_eq!(best(&cyr), "sr");
    }

    #[test]
    fn croatian_and_bosnian() {
        let hr = "Zakupnik je dužan plaćati zakupninu do petog u mjesecu, a tisuću kuna \
                  uplaćuje tvrtka svaki tjedan.";
        assert_eq!(best(hr), "hr");
        let bs = "Uposlenik ima pravo na godišnji odmor, a hiljadu maraka se uplaćuje \
                  svake sedmice u mjesecu.";
        assert_eq!(best(bs), "bs");
    }

    #[test]
    fn candidates_are_a_distribution() {
        let d = DET.detect("Ugovor o zakupu poslovnog prostora").unwrap();
        let sum: f32 = d.candidates.iter().map(|c| c.score).sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        assert!(
            d.top(3).iter().all(|c| c.lang.is_hbs()),
            "{:?}",
            d.candidates
        );
        assert!(d.candidates.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn too_short_or_no_letters() {
        assert!(DET.detect("12 / 34").is_none());
        assert!(DET.detect("").is_none());
    }

    #[test]
    fn sampling_bounds() {
        let long = "Ugovor o zakupu stana. ".repeat(5_000);
        let s = sample(&long, 3_000);
        assert!(s.chars().count() <= 3_000 + 3);
        assert!(!s.is_empty());
        assert_eq!(sample("kratko", 3_000), "kratko");
        let uni = "ž".repeat(10_000);
        assert!(!sample(&uni, 300).is_empty());
    }

    #[test]
    fn markers() {
        let m = hbs_markers("Rešenje o promeni mesta, tisuća, hiljada, historija");
        assert_eq!(m.ekavian, 3);
        assert_eq!(m.hr, 1);
        assert_eq!(m.sr_bs, 1);
        assert_eq!(m.bs, 1);
        let c = hbs_markers("Решење о промени");
        assert!(c.cyrillic);
        assert_eq!(c.ekavian, 2);
    }
}

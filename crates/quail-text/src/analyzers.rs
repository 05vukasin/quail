//! `text.stats@2` and `text.lang@2`.

use quail_core::{
    Analyzer, AnalyzerId, Artifact, Ctx, Emitter, Evidence, Page, Profile, Result, Value,
};

use crate::garbage::{GarbageReport, garbage_report};
use crate::keys;
use crate::lang::{Detection, LanguageDetector};
use crate::normalize::{NormalizeOptions, normalize_into};
use crate::normalized::{NormalizedPage, NormalizedText};
use crate::script::ScriptCounts;
use crate::token::tokens;

/// Share of letters the dominant script needs for `text.script` to name it (else `mixed`).
const DOMINANT_SCRIPT: f32 = 0.8;

/// The longest prefix of `text` within `budget` bytes, on a char boundary.
fn within(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Document statistics: characters, words, pages with text, script ratios, garbage ratio.
///
/// Emits [`keys::CHARS`], [`keys::WORDS`], [`keys::PAGES_WITH_TEXT`], [`keys::SCRIPT`],
/// [`keys::SCRIPT_LATIN`], [`keys::SCRIPT_CYRILLIC`], [`keys::SCRIPT_GREEK`],
/// [`keys::SCRIPT_OTHER`], [`keys::GARBAGE_RATIO`] and [`keys::GARBAGE_PAGES`]. Characters,
/// scripts and garbage are measured on the page text as extracted; words on the normalized
/// text (de-hyphenated). A page counts as garbage by the same rule as
/// [`GarbageReport::verdict`] (too-short pages never do). Reads at most
/// `Limits::max_text_bytes` of text and records a problem when it stops early.
///
/// Provides the [`NormalizedText`] artifact (`normalized_text`): every page it read whole,
/// normalized with the default options, with its offset map and keyed tokens.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextStats;

impl TextStats {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("text.stats", 2);
}

impl Analyzer for TextStats {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn provides(&self) -> &'static [&'static str] {
        &[NormalizedText::NAME]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let mut normalized: Vec<Option<NormalizedPage>> = Vec::with_capacity(doc.pages.len());
        let mut budget = usize::try_from(ctx.limits.max_text_bytes).unwrap_or(usize::MAX);
        let mut chars = 0usize;
        let mut words = 0usize;
        let mut pages_with_text = 0usize;
        let mut norm = String::new();
        let mut scripts = ScriptCounts::default();
        let mut garbage = GarbageReport::default();
        for page in &doc.pages {
            out.check_deadline()?;
            if budget == 0 {
                out.problem(format!(
                    "text.stats: text limit reached, pages from {} not counted",
                    page.index
                ));
                break;
            }
            let text = within(&page.text, budget);
            budget -= text.len();
            // A page read whole goes into the artifact, and its words come from there.
            let whole = (text.len() == page.text.len()).then(|| NormalizedPage::new(text));
            let g = garbage_report(text);
            if g.chars > 0 {
                pages_with_text += 1;
                chars += g.chars;
                garbage.merge(&g);
                scripts.add(text);
                // Words of the normalized text: "ugo-\nvor" is one word.
                words += if let Some(p) = &whole {
                    p.words()
                } else {
                    norm.clear();
                    normalize_into(text, &NormalizeOptions::default(), &mut norm);
                    tokens(&norm).count()
                };
            }
            normalized.push(whole);
        }
        out.provide(NormalizedText::from_pages(normalized));
        out.fact(keys::CHARS, Value::Int(to_i64(chars)));
        out.fact(keys::WORDS, Value::Int(to_i64(words)));
        out.fact(keys::PAGES_WITH_TEXT, Value::Int(to_i64(pages_with_text)));
        if chars > 0 {
            out.fact(
                keys::GARBAGE_RATIO,
                Value::Float(f64::from(garbage.ratio())),
            );
            out.fact(
                keys::GARBAGE_PAGES,
                Value::Int(to_i64(garbage.garbage_texts)),
            );
        }
        if let Some(dominant) = scripts.dominant() {
            let r = scripts.ratios();
            let name = if r.max() >= DOMINANT_SCRIPT {
                dominant.as_str()
            } else {
                "mixed"
            };
            out.fact(keys::SCRIPT, Value::text(name));
            out.fact(keys::SCRIPT_LATIN, Value::Float(f64::from(r.latin)));
            out.fact(keys::SCRIPT_CYRILLIC, Value::Float(f64::from(r.cyrillic)));
            out.fact(keys::SCRIPT_GREEK, Value::Float(f64::from(r.greek)));
            out.fact(keys::SCRIPT_OTHER, Value::Float(f64::from(r.other)));
        }
        Ok(())
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Document language with its top-3 candidates; per-page languages from `Standard` on.
///
/// Emits [`keys::LANG`] (best language, `conf` = its score), [`keys::LANG_TOP`] (up to three
/// `[code, score]` pairs) and, under `Standard` and `Deep`, one [`keys::PAGE_LANG`] per page
/// with enough text, with the page as evidence. Pages flagged `needs_ocr` are skipped:
/// their text layer is not trusted. Text is normalized (de-hyphenated, NFKC) first and at
/// most [`DetectorOptions::max_marker_chars`](crate::lang::DetectorOptions) characters of
/// the document are read. Pages that [`TextStats`] already normalized are taken from the
/// [`NormalizedText`] artifact when they are read whole (same result).
#[derive(Clone, Debug, Default)]
pub struct TextLang {
    detector: LanguageDetector,
}

impl TextLang {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("text.lang", 2);

    /// Letters a page needs for a per-page language.
    pub const MIN_PAGE_LETTERS: u64 = 40;

    /// Characters of a page sampled for its language.
    pub const PAGE_SAMPLE_CHARS: usize = 1_000;

    /// With a new detector (loads the language models, about 50 MB, once).
    pub fn new() -> Self {
        Self::default()
    }

    /// With a shared detector.
    pub fn with_detector(detector: LanguageDetector) -> Self {
        Self { detector }
    }
}

fn top3(d: &Detection) -> Value {
    Value::List(
        d.top(3)
            .iter()
            .map(|c| {
                Value::List(vec![
                    Value::text(c.lang.code()),
                    Value::Float(f64::from(c.score)),
                ])
            })
            .collect(),
    )
}

fn usable(p: &Page) -> bool {
    !p.needs_ocr && !p.text.is_empty()
}

impl Analyzer for TextLang {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document", NormalizedText::NAME]
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let shared = ctx.get::<NormalizedText>();
        // `text` normalized with the defaults, appended to `out`: from the artifact when the
        // whole page is read and it has the page, else normalized here.
        let append = |pos: usize, page: &Page, part: &str, out: &mut String| match shared
            .and_then(|n| n.page(pos, &page.text))
        {
            Some(p) if part.len() == page.text.len() => out.push_str(p.text()),
            _ => normalize_into(part, &NormalizeOptions::default(), out),
        };
        let cap_chars = self.detector.options().max_marker_chars;
        // Bytes, not chars: 4 bytes per char at most.
        let cap = cap_chars.saturating_mul(4);
        let mut text = String::new();
        for (pos, page) in doc.pages.iter().enumerate().filter(|(_, p)| usable(p)) {
            out.check_deadline()?;
            if text.len() >= cap {
                break;
            }
            append(pos, page, within(&page.text, cap - text.len()), &mut text);
            text.push('\n');
        }
        if let Some(d) = self.detector.detect(&text)
            && let Some(best) = d.best()
        {
            out.push(
                keys::LANG,
                Value::text(best.lang.code()),
                best.score.clamp(0.0, 1.0),
                None,
            );
            out.fact(keys::LANG_TOP, top3(&d));
        }
        if ctx.profile < Profile::Standard {
            return Ok(());
        }
        let mut buf = String::new();
        for (pos, page) in doc.pages.iter().enumerate().filter(|(_, p)| usable(p)) {
            out.check_deadline()?;
            buf.clear();
            append(
                pos,
                page,
                within(&page.text, Self::PAGE_SAMPLE_CHARS * 4),
                &mut buf,
            );
            if ScriptCounts::of(&buf).letters() < Self::MIN_PAGE_LETTERS {
                continue;
            }
            if let Some(best) = self
                .detector
                .detect_limited(&buf, Self::PAGE_SAMPLE_CHARS)
                .and_then(|d| d.best())
            {
                out.push(
                    keys::PAGE_LANG,
                    Value::text(best.lang.code()),
                    best.score.clamp(0.0, 1.0),
                    Some(Evidence {
                        page: Some(page.index),
                        ..Evidence::default()
                    }),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{
        Artifacts, Blob, Document, Fact, FactKey, Format, Limits, PageKind, TextSource,
    };

    fn page(index: u32, text: &str) -> Page {
        Page {
            text: text.to_owned(),
            text_source: TextSource::Native,
            ..Page::new(index, PageKind::Physical)
        }
    }

    fn run(a: &dyn Analyzer, doc: Document, profile: Profile, limits: &Limits) -> Vec<Fact> {
        let mut artifacts = Artifacts::new();
        artifacts.insert(doc);
        run_on(a, &artifacts, profile, limits).0
    }

    /// Runs `a` over `artifacts`; its facts and artifacts.
    fn run_on(
        a: &dyn Analyzer,
        artifacts: &Artifacts,
        profile: Profile,
        limits: &Limits,
    ) -> (Vec<Fact>, Artifacts) {
        let blob = Blob::from(Vec::new());
        let ctx = Ctx {
            blob: &blob,
            format: Format::Pdf,
            locator: "mem://test",
            artifacts,
            facts: &[],
            limits,
            profile,
        };
        let mut out = Emitter::new(a.id());
        a.run(&ctx, &mut out).unwrap();
        let (facts, artifacts, _) = out.into_parts();
        (facts, artifacts)
    }

    fn get<'a>(facts: &'a [Fact], key: &FactKey) -> Option<&'a Value> {
        facts.iter().find(|f| &f.key == key).map(|f| &f.value)
    }

    fn int(facts: &[Fact], key: &FactKey) -> i64 {
        match get(facts, key) {
            Some(Value::Int(n)) => *n,
            v => panic!("{key}: {v:?}"),
        }
    }

    fn float(facts: &[Fact], key: &FactKey) -> f64 {
        match get(facts, key) {
            Some(Value::Float(n)) => *n,
            v => panic!("{key}: {v:?}"),
        }
    }

    #[test]
    fn stats_on_mixed_document() {
        let doc = Document {
            pages: vec![
                page(0, "Ugovor o zakupu br. 114/23"),
                page(1, "   "),
                page(2, "Уговор о закупу"),
            ],
        };
        let f = run(&TextStats, doc, Profile::Fast, &Limits::default());
        assert_eq!(int(&f, &keys::PAGES_WITH_TEXT), 2);
        assert_eq!(int(&f, &keys::WORDS), 8);
        assert_eq!(int(&f, &keys::CHARS), 22 + 13);
        assert_eq!(get(&f, &keys::SCRIPT), Some(&Value::text("mixed")));
        let lat = float(&f, &keys::SCRIPT_LATIN);
        let cyr = float(&f, &keys::SCRIPT_CYRILLIC);
        assert!((lat + cyr - 1.0).abs() < 1e-6 && lat > cyr);
        assert!(float(&f, &keys::GARBAGE_RATIO) < 0.01);
        assert_eq!(int(&f, &keys::GARBAGE_PAGES), 0);
        assert!(f.iter().all(|x| x.key.namespace() == "text"));
    }

    #[test]
    fn words_after_dehyphenation_and_short_pages() {
        let doc = Document {
            pages: vec![
                page(0, "Ovaj ugo-\nvor je zaključen."),
                page(1, "Strana 2"),
                page(2, "\u{e000}\u{e001}"),
            ],
        };
        let f = run(&TextStats, doc, Profile::Fast, &Limits::default());
        assert_eq!(int(&f, &keys::WORDS), 4 + 2);
        // Short pages, even private-use ones, are not garbage pages.
        assert_eq!(int(&f, &keys::GARBAGE_PAGES), 0);
    }

    #[test]
    fn stats_flag_garbage_pages() {
        let pua: String = (0xE000u32..0xE100).filter_map(char::from_u32).collect();
        let doc = Document {
            pages: vec![page(0, &pua), page(1, "Normal text of a clean page here.")],
        };
        let f = run(&TextStats, doc, Profile::Fast, &Limits::default());
        assert_eq!(int(&f, &keys::GARBAGE_PAGES), 1);
        assert!(float(&f, &keys::GARBAGE_RATIO) > 0.5);
        assert_eq!(get(&f, &keys::SCRIPT), Some(&Value::text("latin")));
    }

    #[test]
    fn stats_respect_text_limit() {
        let doc = Document {
            pages: vec![page(0, "abcdef"), page(1, "ghijkl")],
        };
        let limits = Limits {
            max_text_bytes: 4,
            ..Limits::default()
        };
        let mut artifacts = Artifacts::new();
        artifacts.insert(doc);
        let blob = Blob::from(Vec::new());
        let ctx = Ctx {
            blob: &blob,
            format: Format::Pdf,
            locator: "mem://test",
            artifacts: &artifacts,
            facts: &[],
            limits: &limits,
            profile: Profile::Fast,
        };
        let mut out = Emitter::new(TextStats::ID);
        TextStats.run(&ctx, &mut out).unwrap();
        let problems = out.take_problems();
        let (facts, _, _) = out.into_parts();
        assert_eq!(int(&facts, &keys::CHARS), 4);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn empty_document() {
        let f = run(
            &TextStats,
            Document::default(),
            Profile::Fast,
            &Limits::default(),
        );
        assert_eq!(int(&f, &keys::CHARS), 0);
        assert!(get(&f, &keys::SCRIPT).is_none());
        assert!(get(&f, &keys::GARBAGE_RATIO).is_none());
        let f = run(
            &TextLang::new(),
            Document::default(),
            Profile::Deep,
            &Limits::default(),
        );
        assert!(f.is_empty());
    }

    #[test]
    fn lang_document_and_pages() {
        let sr = "Zakupac je dužan da plaća zakupninu do petog u mesecu za tekući mesec. \
                  Rešenje o tome donosi opština posle sednice veća.";
        let en = "The tenant shall pay the rent by the fifth day of each month to the \
                  landlord's bank account, as agreed between the parties.";
        let mut broken = page(2, "\u{e000}\u{e001}");
        broken.needs_ocr = true;
        let doc = Document {
            pages: vec![page(0, sr), page(1, en), broken],
        };
        let lang = TextLang::new();
        let fast = run(&lang, doc.clone(), Profile::Fast, &Limits::default());
        assert!(fast.iter().all(|f| f.key != keys::PAGE_LANG));
        let top = get(&fast, &keys::LANG_TOP).unwrap();
        assert!(matches!(top, Value::List(v) if !v.is_empty() && v.len() <= 3));

        let std = run(&lang, doc, Profile::Standard, &Limits::default());
        let pages: Vec<(u32, &Value)> = std
            .iter()
            .filter(|f| f.key == keys::PAGE_LANG)
            .map(|f| (f.evidence.as_ref().and_then(|e| e.page).unwrap(), &f.value))
            .collect();
        assert_eq!(pages, [(0, &Value::text("sr")), (1, &Value::text("en"))]);
        for f in &std {
            assert!((0.0..=1.0).contains(&f.conf));
        }
    }

    #[test]
    fn stats_provide_whole_pages_only() {
        let doc = Document {
            pages: vec![
                page(0, "Ugo-\nvor  o zakupu"),
                page(1, "abcdefgh"),
                page(2, "x"),
            ],
        };
        let mut artifacts = Artifacts::new();
        artifacts.insert(doc.clone());
        let limits = Limits {
            max_text_bytes: 22,
            ..Limits::default()
        };
        let (_, made) = run_on(&TextStats, &artifacts, Profile::Fast, &limits);
        let n = made.get::<NormalizedText>().unwrap();
        let p = n.page(0, &doc.pages[0].text).unwrap();
        assert_eq!(p.text(), "Ugovor o zakupu");
        assert_eq!(p.words(), 3);
        // Page 1 was cut by the limit, page 2 never read.
        assert!(n.page(1, &doc.pages[1].text).is_none());
        assert!(n.page(2, &doc.pages[2].text).is_none());
    }

    #[test]
    fn lang_is_the_same_with_the_artifact() {
        let sr = "Zakupac je du\u{17e}an da pla\u{107}a zakup-\nninu do petog u mesecu za teku\u{107}i \
                  mesec. Re\u{161}enje o tome donosi op\u{161}tina posle sednice ve\u{107}a. ";
        let en = "The tenant shall pay the rent by the fifth day of each month to the \
                  landlord\u{2019}s bank account, as agreed between the parties. ";
        // A long page (cut for the per-page sample) and short ones.
        let doc = Document {
            pages: vec![
                page(0, &sr.repeat(40)),
                page(1, en),
                page(2, sr),
                page(3, ""),
            ],
        };
        for profile in [Profile::Fast, Profile::Standard] {
            let mut alone = Artifacts::new();
            alone.insert(doc.clone());
            let (want, _) = run_on(&TextLang::new(), &alone, profile, &Limits::default());
            let mut shared = Artifacts::new();
            shared.insert(doc.clone());
            let (_, made) = run_on(&TextStats, &shared, profile, &Limits::default());
            shared.absorb(made);
            assert!(shared.get::<NormalizedText>().is_some());
            let (got, _) = run_on(&TextLang::new(), &shared, profile, &Limits::default());
            assert_eq!(got, want);
            assert!(!got.is_empty());
        }
    }
}

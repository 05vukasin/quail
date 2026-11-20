//! Generated inputs shared by the integration tests.
#![allow(dead_code)]

use quail_core::{
    Analyzer, Artifacts, BBox, Blob, Block, BlockKind, Ctx, Document, Emitter, Fact, Format,
    Limits, Page, PageKind, Profile, Span, TextSource, Zone,
};

/// Deterministic xorshift generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
}

const SYLLABLES: &[&str] = &[
    "ka", "lo", "mi", "ne", "su", "ra", "to", "vi", "de", "pa", "zo", "li", "go", "be", "ma", "nu",
    "si", "te", "ro", "ja",
];

/// A pseudo-word from a seeded generator: 1 to 3 syllables.
pub fn word(rng: &mut Rng) -> String {
    let n = 1 + rng.below(3);
    (0..n)
        .map(|_| SYLLABLES[rng.below(SYLLABLES.len())])
        .collect()
}

/// `n` pseudo-words in sentences of 12, lowercase.
pub fn prose(seed: u64, n: usize) -> Vec<String> {
    let mut rng = Rng(seed);
    (0..n).map(|_| word(&mut rng)).collect()
}

/// Words joined into text with a period every 12 words.
pub fn join(words: &[String]) -> String {
    let mut s = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            s.push_str(if i % 12 == 0 { ". " } else { " " });
        }
        s.push_str(w);
    }
    s.push('.');
    s
}

/// A filled-in form: fixed boilerplate (from `form_seed`) with particulars from `fill_seed`
/// (a party name, numbers, dates, amounts, a code) in fixed slots.
pub fn form(form_seed: u64, fill_seed: u64) -> String {
    let boiler = prose(form_seed, 400);
    let mut rng = Rng(fill_seed);
    let party = {
        let mut w = word(&mut rng);
        w[..1].make_ascii_uppercase();
        w
    };
    let mut s = String::new();
    for (i, chunk) in boiler.chunks(20).enumerate() {
        s.push_str(&join(chunk));
        let n = rng.below(900) + 100;
        let d = rng.below(28) + 1;
        let m = rng.below(12) + 1;
        let amount = rng.below(90_000) + 1_000;
        let slot = match i % 4 {
            0 => format!(" ugovor br. {n}/{} sa firmom {party} doo", d + 10),
            1 => format!(" dana {d:02}.{m:02}.2023 u iznosu od {amount},{d:02} RSD"),
            2 => format!(" racun RS{}{} za {party}", 35 + d, 1_000_000 + n * 7),
            _ => format!(" rok {n} dana"),
        };
        s.push_str(&slot);
        s.push_str(".\n");
    }
    s
}

pub fn text_page(index: u32, text: &str) -> Page {
    let len = u32::try_from(text.len()).unwrap();
    Page {
        text: text.to_owned(),
        text_source: TextSource::Native,
        blocks: vec![Block {
            span: Span { start: 0, end: len },
            kind: BlockKind::Paragraph,
            zone: Zone::Body,
            bbox: None,
        }],
        ..Page::new(index, PageKind::Logical)
    }
}

pub fn text_doc(text: &str) -> Document {
    Document {
        pages: vec![text_page(0, text)],
    }
}

/// Runs one analyzer over a document (and optional extra artifacts) and returns its facts,
/// warnings excluded.
pub fn run(a: &dyn Analyzer, doc: Document, extra: Option<Artifacts>) -> Vec<Fact> {
    let blob = Blob::new(Vec::<u8>::new());
    let mut arts = Artifacts::new();
    arts.insert(doc);
    if let Some(e) = extra {
        arts.absorb(e);
    }
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "generated",
        artifacts: &arts,
        facts: &[],
        limits: &limits,
        profile: Profile::Deep,
    };
    let mut out = Emitter::new(a.id());
    if a.applies(&ctx) {
        a.run(&ctx, &mut out).unwrap();
    }
    out.facts().to_vec()
}

/// A physical page with boxed paragraph blocks at the given `(x0, y0, x1, y1)` positions;
/// the text of block `i` is `texts[i % texts.len()]`.
pub fn boxed_page(index: u32, boxes: &[(f32, f32, f32, f32)], texts: &[&str]) -> Page {
    let mut text = String::new();
    let mut blocks = Vec::new();
    for (i, &(x0, y0, x1, y1)) in boxes.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        let start = u32::try_from(text.len()).unwrap();
        text.push_str(texts[i % texts.len()]);
        let end = u32::try_from(text.len()).unwrap();
        blocks.push(Block {
            span: Span { start, end },
            kind: BlockKind::Paragraph,
            zone: if y0 < 60.0 { Zone::Header } else { Zone::Body },
            bbox: Some(BBox { x0, y0, x1, y1 }),
        });
    }
    Page {
        size: Some((595.0, 842.0)),
        text,
        blocks,
        text_source: TextSource::Native,
        ..Page::new(index, PageKind::Physical)
    }
}

/// A logical page whose blocks have the given kinds; block texts are pseudo-words.
pub fn kinds_page(kinds: &[BlockKind], seed: u64) -> Page {
    let mut rng = Rng(seed);
    let mut text = String::new();
    let mut blocks = Vec::new();
    for &kind in kinds {
        if !text.is_empty() {
            text.push('\n');
        }
        let start = u32::try_from(text.len()).unwrap();
        let w = word(&mut rng);
        text.push_str(&w);
        let end = u32::try_from(text.len()).unwrap();
        let zone = match kind {
            BlockKind::Heading { .. } => Zone::Title,
            BlockKind::TableCell { .. } => Zone::Table,
            _ => Zone::Body,
        };
        blocks.push(Block {
            span: Span { start, end },
            kind,
            zone,
            bbox: None,
        });
    }
    Page {
        text,
        blocks,
        text_source: TextSource::Native,
        ..Page::new(0, PageKind::Logical)
    }
}

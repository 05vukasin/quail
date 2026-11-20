//! The `sketch.*` analyzers.

// The analyzers are unit structs; their helpers take `self` for uniformity.
#![allow(clippy::unused_self)]

use quail_core::{Analyzer, AnalyzerId, Ctx, Document, Emitter, Profile, Raster, Result, Value};
use quail_text::fold::FoldOptions;
use quail_text::token::keyed_tokens;

use crate::keys;
use crate::minhash::{MIN_SHINGLES, MinHash, shingle_hashes};
use crate::text::document_text;
use crate::{invoice, layout, phash, sada, simhash, template};

fn int(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

macro_rules! analyzer {
    ($ty:ident, $name:literal, $version:literal, $profile:expr, $requires:expr) => {
        impl $ty {
            /// Analyzer id.
            pub const ID: AnalyzerId = AnalyzerId::new($name, $version);
        }

        impl Analyzer for $ty {
            fn id(&self) -> AnalyzerId {
                Self::ID
            }

            fn requires(&self) -> &'static [&'static str] {
                $requires
            }

            fn profile(&self) -> Profile {
                $profile
            }

            fn applies(&self, ctx: &Ctx<'_>) -> bool {
                self.applies_to(ctx)
            }

            fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
                let Some(doc) = ctx.get::<Document>() else {
                    return Ok(());
                };
                self.sketch(ctx, doc, out)
            }
        }
    };
}

fn has_text(ctx: &Ctx<'_>) -> bool {
    ctx.get::<Document>()
        .is_some_and(|d| d.pages.iter().any(|p| !p.needs_ocr && !p.text.is_empty()))
}

/// `sketch.minhash@1`: MinHash signature, LSH band keys and shingle count of the word
/// 3-shingles (see [`crate::minhash`]). Emits [`keys::MINHASH`], [`keys::MINHASH_BANDS`],
/// [`keys::MINHASH_SHINGLES`]; nothing below [`MIN_SHINGLES`] distinct shingles.
#[derive(Clone, Copy, Debug, Default)]
pub struct MinHashAnalyzer;

analyzer!(
    MinHashAnalyzer,
    "sketch.minhash",
    1,
    Profile::Fast,
    &["document"]
);

impl MinHashAnalyzer {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = document_text(doc, ctx.limits, true, out)?;
        let mut hashes = Vec::new();
        shingle_hashes(&text, &mut hashes);
        out.check_deadline()?;
        let Some(m) = MinHash::from_hashes(&mut hashes) else {
            return Ok(());
        };
        if m.shingles < MIN_SHINGLES {
            return Ok(());
        }
        out.fact(keys::MINHASH, Value::Bytes(m.to_bytes().into()));
        out.fact(
            keys::MINHASH_BANDS,
            Value::List(m.bands().into_iter().map(Value::U64).collect()),
        );
        out.fact(keys::MINHASH_SHINGLES, int(m.shingles));
        Ok(())
    }
}

/// `sketch.simhash_text@1`: 64-bit SimHash of the weighted word 3-shingles
/// ([`keys::SIMHASH_TEXT`], see [`crate::simhash`]). Nothing below
/// [`MIN_SHINGLES`] shingles.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimHashText;

analyzer!(
    SimHashText,
    "sketch.simhash_text",
    1,
    Profile::Fast,
    &["document"]
);

impl SimHashText {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = document_text(doc, ctx.limits, true, out)?;
        if let Some((h, n)) = simhash::text_simhash(&text)
            && n >= MIN_SHINGLES
        {
            out.fact(keys::SIMHASH_TEXT, Value::U64(h));
        }
        Ok(())
    }
}

/// Fewest distinct masked words for a template sketch.
const MIN_TEMPLATE_WORDS: usize = 20;

/// `sketch.simhash_template@1`: SimHash of the entity-masked words
/// ([`keys::SIMHASH_TEMPLATE`], see [`crate::template`]). Nothing below 20 distinct masked
/// words.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimHashTemplate;

analyzer!(
    SimHashTemplate,
    "sketch.simhash_template",
    1,
    Profile::Fast,
    &["document"]
);

impl SimHashTemplate {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = document_text(doc, ctx.limits, true, out)?;
        if let Some((h, n)) = template::template_simhash(&text)
            && n >= MIN_TEMPLATE_WORDS
        {
            out.fact(keys::SIMHASH_TEMPLATE, Value::U64(h));
        }
        Ok(())
    }
}

/// `sketch.layout@1`: geometry sketch ([`keys::LAYOUT`], [`keys::LAYOUT_BLOCKS`]) for
/// documents with block boxes, block-structure sketch ([`keys::STRUCTURE`]) and heading
/// skeleton ([`keys::HEADINGS`]) for any document with blocks (see [`crate::layout`]). Reads
/// no text except headings.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutAnalyzer;

analyzer!(
    LayoutAnalyzer,
    "sketch.layout",
    1,
    Profile::Fast,
    &["document"]
);

impl LayoutAnalyzer {
    #[allow(clippy::unused_self)]
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        ctx.get::<Document>()
            .is_some_and(|d| d.pages.iter().any(|p| !p.blocks.is_empty()))
    }

    fn sketch(self, _ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        out.check_deadline()?;
        if let Some((h, n)) = layout::geometry_sketch(doc) {
            out.fact(keys::LAYOUT, Value::U64(h));
            out.fact(keys::LAYOUT_BLOCKS, int(n));
        }
        out.check_deadline()?;
        if let Some((h, _)) = layout::structure_sketch(doc) {
            out.fact(keys::STRUCTURE, Value::U64(h));
        }
        let heads = layout::headings(doc);
        if heads.len() >= 2 {
            out.fact(
                keys::HEADINGS,
                Value::U64(sada::structural_fp(heads.iter().copied())),
            );
        }
        Ok(())
    }
}

/// `sketch.invoice_layout@1`: the invoice layout fingerprint ([`keys::INVOICE_LAYOUT`],
/// [`keys::INVOICE_LABELS`], [`keys::INVOICE_NUMBER_PATTERNS`]; see [`crate::invoice`]),
/// emitted when at least one invoice label occurs. Use the label count to decide whether the
/// document is an invoice at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct InvoiceLayout;

analyzer!(
    InvoiceLayout,
    "sketch.invoice_layout",
    1,
    Profile::Fast,
    &["document"]
);

impl InvoiceLayout {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = document_text(doc, ctx.limits, false, out)?;
        match invoice::features(&text) {
            Ok(f) if !f.labels.is_empty() => {
                out.fact(keys::INVOICE_LAYOUT, Value::U64(f.fingerprint()));
                out.fact(keys::INVOICE_LABELS, int(f.labels.len()));
                out.fact(keys::INVOICE_NUMBER_PATTERNS, int(f.patterns.len()));
            }
            Ok(_) => {}
            Err(e) => out.warn(format!("{}: {e}, sketch skipped", Self::ID)),
        }
        Ok(())
    }
}

/// Fewest characters of folded words for a text key.
const MIN_TEXT_KEY_CHARS: usize = 400;

/// `sketch.text_key@1`: XXH3-128 of the folded words joined by one space
/// ([`keys::TEXT_KEY`]): equal keys, the same words. Nothing below 400 characters.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextKey;

analyzer!(TextKey, "sketch.text_key", 1, Profile::Fast, &["document"]);

impl TextKey {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = document_text(doc, ctx.limits, true, out)?;
        if let Some(k) = text_key(&text) {
            out.fact(keys::TEXT_KEY, Value::Bytes(k.to_vec().into()));
        }
        Ok(())
    }
}

/// `sketch.text_key` of already-normalized text (canonical big-endian XXH3-128 with the
/// `quail-text` seed), `None` below 400 characters of folded words.
pub fn text_key(normalized: &str) -> Option<[u8; 16]> {
    let mut joined = String::with_capacity(normalized.len());
    let mut it = keyed_tokens(normalized, &FoldOptions::KEY);
    while let Some(t) = it.next_keyed() {
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(t.key);
    }
    (joined.chars().count() >= MIN_TEXT_KEY_CHARS).then(|| {
        xxhash_rust::xxh3::xxh3_128_with_seed(joined.as_bytes(), quail_text::shingle::SEED)
            .to_be_bytes()
    })
}

/// `sketch.minhash_sada@1`: the Python-compatible sketches of [`crate::sada`]: version
/// signature [`keys::SADA_MINHASH`] with its buckets [`keys::SADA_MINHASH_BANDS`] and
/// [`keys::SADA_SHINGLES`] (nothing below 40 shingles), and the template fingerprint
/// [`keys::SADA_TEMPLATE`]. Reads the text as extracted (not normalized), as the original.
/// Standard only: `BLAKE2b` per shingle and backtracking (`fancy-regex`) masking cost several
/// times the native sketches; a pattern that hits `sada::backtrack_limit` is a warning and
/// that sketch is skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct SadaMinHash;

analyzer!(
    SadaMinHash,
    "sketch.minhash_sada",
    1,
    Profile::Standard,
    &["document"]
);

impl SadaMinHash {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = document_text(doc, ctx.limits, false, out)?;
        if let Some((sig, n)) = sada::raw_minhash(&text) {
            let bytes: Vec<u8> = sig.iter().flat_map(|r| r.to_le_bytes()).collect();
            out.fact(keys::SADA_MINHASH, Value::Bytes(bytes.into()));
            out.fact(
                keys::SADA_MINHASH_BANDS,
                Value::List(
                    sada::lsh_buckets(&sig)
                        .into_iter()
                        .map(Value::text)
                        .collect(),
                ),
            );
            out.fact(keys::SADA_SHINGLES, int(n));
        }
        out.check_deadline()?;
        if !text.trim().is_empty() {
            match sada::template_fp(&text) {
                Ok(fp) => out.fact(keys::SADA_TEMPLATE, Value::U64(fp)),
                Err(e) => out.warn(format!("{}: {e}, sketch skipped", Self::ID)),
            }
        }
        Ok(())
    }
}

/// Pages `sketch.page_phash` hashes per document.
pub const MAX_PHASH_PAGES: usize = 16;
/// Resolution pages are rendered at for the hash when they hold no single scan.
pub const PHASH_DPI: f32 = 36.0;

/// `sketch.page_phash@1`: 64-bit pHash ([`crate::phash`]) of every scanned page
/// ([`quail_core::Page::is_scan`]) or page needing OCR, at most 16 per document
/// ([`keys::PAGE_PHASH`], [`keys::PAGE_PHASH_PAGES`]). Takes the embedded scan when there is
/// one, else renders the page at 36 dpi. Requires the `raster` artifact.
#[derive(Clone, Copy, Debug, Default)]
pub struct PagePHash;

analyzer!(
    PagePHash,
    "sketch.page_phash",
    1,
    Profile::Standard,
    &["document", "raster"]
);

impl PagePHash {
    #[allow(clippy::unused_self)]
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        ctx.get::<Document>()
            .is_some_and(|d| d.pages.iter().any(|p| p.is_scan() || p.needs_ocr))
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let Some(raster) = ctx.get::<Raster>() else {
            return Ok(());
        };
        let mut hashes = Vec::new();
        let mut pages = Vec::new();
        let wanted = doc.pages.iter().filter(|p| p.is_scan() || p.needs_ocr);
        for (n, page) in wanted.enumerate() {
            if n >= MAX_PHASH_PAGES {
                out.warn(format!(
                    "{}: only the first {MAX_PHASH_PAGES} scanned pages hashed",
                    Self::ID
                ));
                break;
            }
            out.check_deadline()?;
            let image = match raster.0.embedded_scan(page.index, ctx.limits) {
                Some(r) => r,
                None => raster.0.render(page.index, PHASH_DPI, ctx.limits),
            };
            match image {
                Ok(im) => match phash::phash(im.width, im.height, im.channels, &im.data) {
                    Some(h) => {
                        hashes.push(Value::U64(h));
                        pages.push(Value::Int(i64::from(page.index)));
                    }
                    None => out.warn(format!("{}: page {} image unusable", Self::ID, page.index)),
                },
                Err(e) => out.warn(format!("{}: page {}: {e}", Self::ID, page.index)),
            }
        }
        if !hashes.is_empty() {
            out.fact(keys::PAGE_PHASH, Value::List(hashes));
            out.fact(keys::PAGE_PHASH_PAGES, Value::List(pages));
        }
        Ok(())
    }
}

//! The `sketch.*` analyzers.

// The analyzers are unit structs; their helpers take `self` for uniformity.
#![allow(clippy::unused_self)]

use quail_core::{
    Analyzer, AnalyzerId, Artifact, Ctx, Document, Emitter, Profile, Raster, Result, Value,
};
use quail_text::NormalizedText;
use quail_text::fold::FoldOptions;
use quail_text::shingle::hash_str;
use quail_text::token::keyed_tokens;

use crate::keys;
use crate::minhash::{MIN_SHINGLES, MinHash, shingle_hashes_of_keys};
use crate::text::{DocText, extend_distinct, raw_segments};
use crate::{compat, invoice, layout, phash, simhash, template};

fn int(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// Soft ordering of the analyzers that read the best document (OCR text when present).
const AFTER_REVISED: &[&str] = &["revised_document"];
/// Same, plus the shared normalized pages of `text.stats`.
const AFTER_NORMALIZED: &[&str] = &["revised_document", NormalizedText::NAME];

/// The best available document: the revised one (OCR) when present, else the extracted one.
fn revised<'c>(ctx: &'c Ctx<'_>) -> Option<&'c Document> {
    ctx.document()
}

/// The document as extracted (page images and the extractor's OCR flags).
fn extracted<'c>(ctx: &'c Ctx<'_>) -> Option<&'c Document> {
    ctx.get::<Document>()
}

macro_rules! analyzer {
    (
        $ty:ident, $name:literal, $version:literal, $profile:expr, $requires:expr,
        $after:expr, $source:ident
    ) => {
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

            fn after(&self) -> &'static [&'static str] {
                $after
            }

            fn profile(&self) -> Profile {
                $profile
            }

            fn applies(&self, ctx: &Ctx<'_>) -> bool {
                self.applies_to(ctx)
            }

            fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
                let Some(doc) = $source(ctx) else {
                    return Ok(());
                };
                self.sketch(ctx, doc, out)
            }
        }
    };
}

fn has_text(ctx: &Ctx<'_>) -> bool {
    ctx.document()
        .is_some_and(|d| d.pages.iter().any(|p| !p.needs_ocr && !p.text.is_empty()))
}

/// `sketch.minhash@2`: MinHash signature, LSH band keys and shingle count of the word
/// 3-shingles (see [`crate::minhash`]). Emits [`keys::MINHASH`], [`keys::MINHASH_BANDS`],
/// [`keys::MINHASH_SHINGLES`]; nothing below [`MIN_SHINGLES`] distinct shingles.
#[derive(Clone, Copy, Debug, Default)]
pub struct MinHashAnalyzer;

analyzer!(
    MinHashAnalyzer,
    "sketch.minhash",
    2,
    Profile::Fast,
    &["document"],
    AFTER_NORMALIZED,
    revised
);

impl MinHashAnalyzer {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = DocText::read(doc, ctx.get::<NormalizedText>(), ctx.limits, out)?;
        // A set: duplicates are dropped as it fills.
        let mut hashes = Vec::new();
        extend_distinct(&mut hashes, shingle_hashes_of_keys(text.key_hashes()));
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

/// `sketch.simhash_text@2`: 64-bit SimHash of the weighted word 3-shingles
/// ([`keys::SIMHASH_TEXT`], see [`crate::simhash`]). Nothing below
/// [`MIN_SHINGLES`] shingles.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimHashText;

analyzer!(
    SimHashText,
    "sketch.simhash_text",
    2,
    Profile::Fast,
    &["document"],
    AFTER_NORMALIZED,
    revised
);

impl SimHashText {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = DocText::read(doc, ctx.get::<NormalizedText>(), ctx.limits, out)?;
        let hashes: Vec<u64> = shingle_hashes_of_keys(text.key_hashes()).collect();
        if let Some((h, n)) = simhash::shingles_simhash(hashes)
            && n >= MIN_SHINGLES
        {
            out.fact(keys::SIMHASH_TEXT, Value::U64(h));
        }
        Ok(())
    }
}

/// Fewest distinct masked words for a template sketch.
const MIN_TEMPLATE_WORDS: usize = 20;

/// `sketch.simhash_template@2`: SimHash of the entity-masked words
/// ([`keys::SIMHASH_TEMPLATE`], see [`crate::template`]). Nothing below 20 distinct masked
/// words.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimHashTemplate;

analyzer!(
    SimHashTemplate,
    "sketch.simhash_template",
    2,
    Profile::Fast,
    &["document"],
    AFTER_NORMALIZED,
    revised
);

impl SimHashTemplate {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = DocText::read(doc, ctx.get::<NormalizedText>(), ctx.limits, out)?;
        let mut hashes = Vec::new();
        let mut masker = template::Masker::new();
        text.for_each_keyed(|token, key, gap| {
            // A set: duplicates are dropped as it fills.
            extend_distinct(&mut hashes, [hash_str(&masker.mask(token, key, gap))]);
        });
        if let Some((h, n)) = template::masked_simhash(hashes)
            && n >= MIN_TEMPLATE_WORDS
        {
            out.fact(keys::SIMHASH_TEMPLATE, Value::U64(h));
        }
        Ok(())
    }
}

/// `sketch.layout@2`: geometry sketch ([`keys::LAYOUT`], [`keys::LAYOUT_BLOCKS`]) for
/// documents with block boxes, block-structure sketch ([`keys::STRUCTURE`]) and heading
/// skeleton ([`keys::HEADINGS`]) for any document with blocks (see [`crate::layout`]). Reads
/// no text except headings.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutAnalyzer;

analyzer!(
    LayoutAnalyzer,
    "sketch.layout",
    2,
    Profile::Fast,
    &["document"],
    AFTER_REVISED,
    revised
);

impl LayoutAnalyzer {
    #[allow(clippy::unused_self)]
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        ctx.document()
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
                Value::U64(compat::structural_fp(heads.iter().copied())),
            );
        }
        Ok(())
    }
}

/// `sketch.invoice_layout@2`: the invoice layout fingerprint ([`keys::INVOICE_LAYOUT`],
/// [`keys::INVOICE_LABELS`], [`keys::INVOICE_NUMBER_PATTERNS`]; see [`crate::invoice`]),
/// emitted when at least one invoice label occurs. Use the label count to decide whether the
/// document is an invoice at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct InvoiceLayout;

analyzer!(
    InvoiceLayout,
    "sketch.invoice_layout",
    2,
    Profile::Fast,
    &["document"],
    AFTER_REVISED,
    revised
);

impl InvoiceLayout {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let segs = raw_segments(doc, ctx.limits, out)?;
        match invoice::features_of(&segs) {
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

/// `sketch.text_key@2`: XXH3-128 of the folded words joined by one space
/// ([`keys::TEXT_KEY`]): equal keys, the same words. Nothing below 400 characters.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextKey;

analyzer!(
    TextKey,
    "sketch.text_key",
    2,
    Profile::Fast,
    &["document"],
    AFTER_NORMALIZED,
    revised
);

impl TextKey {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let text = DocText::read(doc, ctx.get::<NormalizedText>(), ctx.limits, out)?;
        // The joined words are hashed as they come, never held.
        let mut joined = JoinedKey::new();
        text.for_each_key(|key| joined.push(key));
        if let Some(k) = joined.finish() {
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
        join_key(&mut joined, t.key);
    }
    text_key_of_joined(&joined)
}

/// Appends a folded key to the words joined by one space.
fn join_key(joined: &mut String, key: &str) {
    if !joined.is_empty() {
        joined.push(' ');
    }
    joined.push_str(key);
}

/// [`text_key_of_joined`] of words pushed one by one, streamed into the hash.
struct JoinedKey {
    hasher: xxhash_rust::xxh3::Xxh3,
    chars: usize,
    any: bool,
}

impl JoinedKey {
    fn new() -> Self {
        Self {
            hasher: xxhash_rust::xxh3::Xxh3::with_seed(quail_text::shingle::SEED),
            chars: 0,
            any: false,
        }
    }

    fn push(&mut self, key: &str) {
        // As `join_key`: a space before every key once the joined text is not empty.
        if self.any {
            self.hasher.update(b" ");
            self.chars += 1;
        }
        self.any = self.any || !key.is_empty();
        self.hasher.update(key.as_bytes());
        self.chars += key.chars().count();
    }

    fn finish(&self) -> Option<[u8; 16]> {
        (self.chars >= MIN_TEXT_KEY_CHARS).then(|| self.hasher.digest128().to_be_bytes())
    }
}

/// [`text_key`] of the folded words joined by one space.
fn text_key_of_joined(joined: &str) -> Option<[u8; 16]> {
    (joined.chars().count() >= MIN_TEXT_KEY_CHARS).then(|| {
        xxhash_rust::xxh3::xxh3_128_with_seed(joined.as_bytes(), quail_text::shingle::SEED)
            .to_be_bytes()
    })
}

/// `sketch.minhash_compat@3`: the Python-compatible sketches of [`crate::compat`]: version
/// signature [`keys::COMPAT_MINHASH`] with its buckets [`keys::COMPAT_MINHASH_BANDS`] and
/// [`keys::COMPAT_SHINGLES`] (nothing below 40 shingles), and the template fingerprint
/// [`keys::COMPAT_TEMPLATE`]. Reads the text as extracted (not normalized), as the original.
/// Standard only: `BLAKE2b` per shingle and backtracking (`fancy-regex`) masking cost several
/// times the native sketches; a pattern that hits `compat::backtrack_limit` is a warning and
/// that sketch is skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompatMinHash;

analyzer!(
    CompatMinHash,
    "sketch.minhash_compat",
    3,
    Profile::Standard,
    &["document"],
    AFTER_REVISED,
    revised
);

impl CompatMinHash {
    fn applies_to(self, ctx: &Ctx<'_>) -> bool {
        has_text(ctx)
    }

    fn sketch(self, ctx: &Ctx<'_>, doc: &Document, out: &mut Emitter) -> Result<()> {
        let segs = raw_segments(doc, ctx.limits, out)?;
        if let Some((sig, n)) = compat::raw_minhash_of(&segs) {
            let bytes: Vec<u8> = sig.iter().flat_map(|r| r.to_le_bytes()).collect();
            out.fact(keys::COMPAT_MINHASH, Value::Bytes(bytes.into()));
            out.fact(
                keys::COMPAT_MINHASH_BANDS,
                Value::List(
                    compat::lsh_buckets(&sig)
                        .into_iter()
                        .map(Value::text)
                        .collect(),
                ),
            );
            out.fact(keys::COMPAT_SHINGLES, int(n));
        }
        out.check_deadline()?;
        if segs.iter().any(|s| !s.trim().is_empty()) {
            match compat::template_fp_of(&segs) {
                Ok(fp) => out.fact(keys::COMPAT_TEMPLATE, Value::U64(fp)),
                Err(e) => out.warn(format!("{}: {e}, sketch skipped", Self::ID)),
            }
        }
        Ok(())
    }
}

/// Pages `sketch.page_phash` hashes per document: the first `MAX_PHASH_PAGES - 1` candidate
/// pages and the last one (see [`phash_pages`]).
pub const MAX_PHASH_PAGES: usize = 4;
/// Resolution pages are rendered at for the hash when they hold no single scan.
pub const PHASH_DPI: f32 = 36.0;
/// Pixels `sketch.page_phash` asks for on an embedded scan's longer side
/// ([`PageRaster::embedded_scan_reduced`](quail_core::PageRaster::embedded_scan_reduced)):
/// 4 × the 32-px thumbnail edge, so every thumbnail cell still averages at least 16 pixels.
pub const PHASH_SCAN_SIDE: u32 = 128;

/// Which of `n` candidate pages (scans or pages needing OCR, in order) `sketch.page_phash`
/// hashes: all of them up to [`MAX_PHASH_PAGES`], else the first `MAX_PHASH_PAGES - 1` and
/// the last. The last page is where signed and stamped copies of one form differ most.
pub fn phash_pages(n: usize) -> impl Iterator<Item = usize> {
    let head = if n > MAX_PHASH_PAGES {
        MAX_PHASH_PAGES - 1
    } else {
        n
    };
    (0..head).chain((n > MAX_PHASH_PAGES).then(|| n - 1))
}

/// `sketch.page_phash@4`: 64-bit pHash ([`crate::phash`]) of scanned pages
/// ([`quail_core::Page::is_scan`]) and pages needing OCR: all of them up to 4, else the first
/// 3 and the last ([`phash_pages`]) ([`keys::PAGE_PHASH`], [`keys::PAGE_PHASH_PAGES`]). Takes
/// the embedded scan when there is one, decoded at reduced resolution (at least
/// [`PHASH_SCAN_SIDE`] px on the longer side, see
/// [`PageRaster::embedded_scan_reduced`](quail_core::PageRaster::embedded_scan_reduced)),
/// else renders the page at 36 dpi. Requires the `raster` artifact. `@4` gets JPEG (DCT) scans
/// at reduced resolution too (scaled IDCT), so their hashes differ from `@3` by a few bits;
/// `@3` did so for JPEG 2000 scans, `@2` hashed every scan at full resolution. Do not compare
/// hashes across versions. `@2` to `@4` hash the same pages; `@1` the first 16.
#[derive(Clone, Copy, Debug, Default)]
pub struct PagePHash;

analyzer!(
    PagePHash,
    "sketch.page_phash",
    4,
    Profile::Standard,
    &["document", "raster"],
    &[],
    extracted
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
        let wanted: Vec<_> = doc
            .pages
            .iter()
            .filter(|p| p.is_scan() || p.needs_ocr)
            .collect();
        if wanted.len() > MAX_PHASH_PAGES {
            out.warn(format!(
                "{}: {MAX_PHASH_PAGES} of {} scanned pages hashed (the first {} and the last)",
                Self::ID,
                wanted.len(),
                MAX_PHASH_PAGES - 1
            ));
        }
        for page in phash_pages(wanted.len()).filter_map(|i| wanted.get(i).copied()) {
            out.check_deadline()?;
            let image =
                match raster
                    .0
                    .embedded_scan_reduced(page.index, PHASH_SCAN_SIDE, ctx.limits)
                {
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

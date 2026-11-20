//! The extractor: parse, emit file facts, walk pages, lay out text, judge text quality,
//! assign zones.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use hayro_interpret::kurbo::Rect;
use hayro_interpret::util::TransformExt;
use hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
use hayro_syntax::page::Page as HPage;
use hayro_syntax::{DecryptionError, LoadPdfError, Pdf, PdfData, PdfVersion};
use quail_core::{
    AnalyzerId, Blob, Block, BlockKind, Document, Emitter, Error, Extractor, Format, Limits, Page,
    PageKind, Raster, Result, Span, TextSource, Value, Zone,
};

use crate::budget::Budget;
use crate::device::PageCapture;
use crate::quality::{Signals, garbage_ratio, judge};
use crate::raster::PdfRaster;
use crate::{keys, layout, meta, raw, zones};

/// The PDF extractor (`pdf.extract@1`). Stateless; one instance serves every thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct PdfExtractor;

impl PdfExtractor {
    /// Its id.
    pub const ID: AnalyzerId = AnalyzerId::new("pdf.extract", 1);

    /// New extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Extractor for PdfExtractor {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Pdf]
    }

    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        extract(blob, limits, out)
    }
}

/// Lets hayro read the blob without copying it.
pub(crate) struct BlobBytes(pub(crate) Blob);

impl AsRef<[u8]> for BlobBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

pub(crate) fn panic_msg(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

fn malformed(msg: impl Into<String>) -> Error {
    Error::Malformed {
        format: Format::Pdf,
        msg: msg.into(),
    }
}

fn version_str(v: PdfVersion) -> &'static str {
    match v {
        PdfVersion::Pdf10 => "1.0",
        PdfVersion::Pdf11 => "1.1",
        PdfVersion::Pdf12 => "1.2",
        PdfVersion::Pdf13 => "1.3",
        PdfVersion::Pdf14 => "1.4",
        PdfVersion::Pdf15 => "1.5",
        PdfVersion::Pdf16 => "1.6",
        PdfVersion::Pdf17 => "1.7",
        PdfVersion::Pdf20 => "2.0",
    }
}

fn int(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn extract(blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
    let bytes = blob.as_bytes();
    if bytes.len() as u64 > limits.max_bytes {
        return Err(Error::LimitExceeded("max_bytes"));
    }

    // Byte-level facts first: they survive encryption and broken object graphs.
    let scan = raw::scan(bytes);
    out.fact(
        keys::INCREMENTAL_UPDATES,
        int(raw::incremental_updates(&scan)),
    );
    out.fact(keys::LINEARIZED, Value::Bool(scan.linearized));

    // hayro inflates object streams while opening, without a cap: measure them first.
    if !crate::budget::object_streams_fit(bytes, limits.max_decompressed) {
        if let Some(v) = &scan.header_version {
            out.fact(keys::VERSION, Value::text(v.as_str()));
        }
        out.fact(keys::ENCRYPTED, Value::Bool(scan.encrypt_entry));
        return Err(Error::LimitExceeded("max_decompressed"));
    }

    let data = PdfData::from(Arc::new(BlobBytes(blob.clone())));
    let pdf = match catch_unwind(AssertUnwindSafe(|| Pdf::new(data))) {
        Ok(Ok(pdf)) => pdf,
        Ok(Err(e)) => {
            if let Some(v) = &scan.header_version {
                out.fact(keys::VERSION, Value::text(v.as_str()));
            }
            return Err(match e {
                LoadPdfError::Decryption(d) => {
                    out.fact(keys::ENCRYPTED, Value::Bool(true));
                    match d {
                        DecryptionError::PasswordProtected => Error::Encrypted,
                        DecryptionError::UnsupportedAlgorithm => {
                            Error::Unsupported("PDF encryption algorithm".to_owned())
                        }
                        DecryptionError::MissingIDEntry | DecryptionError::InvalidEncryption => {
                            malformed("invalid encryption dictionary")
                        }
                    }
                }
                LoadPdfError::Invalid => {
                    out.fact(keys::ENCRYPTED, Value::Bool(scan.encrypt_entry));
                    malformed("no usable xref or page tree, even after repair")
                }
            });
        }
        Err(p) => return Err(Error::Panic(format!("pdf parser: {}", panic_msg(&*p)))),
    };

    out.fact(keys::VERSION, Value::text(version_str(pdf.version())));
    out.fact(keys::ENCRYPTED, Value::Bool(scan.encrypt_entry));
    match catch_unwind(AssertUnwindSafe(|| document_facts(&pdf, bytes, out))) {
        Ok(()) => {}
        Err(p) => out.warn(format!("document facts skipped: {}", panic_msg(&*p))),
    }

    let doc = pages(&pdf, bytes.len(), limits, out);
    // Later steps (OCR) get pixels through the core trait and never depend on this crate.
    // The raster shares the file's bytes; it holds no parsed state.
    out.provide(Raster(Arc::new(PdfRaster::new(blob.clone()))));
    Ok(doc)
}

fn document_facts(pdf: &Pdf, bytes: &[u8], out: &mut Emitter) {
    let m = pdf.metadata();
    let texts = [
        (keys::PRODUCER, &m.producer),
        (keys::CREATOR, &m.creator),
        (keys::AUTHOR, &m.author),
        (keys::TITLE, &m.title),
        (keys::SUBJECT, &m.subject),
        (keys::KEYWORDS, &m.keywords),
    ];
    for (k, v) in texts {
        if let Some(s) = v.as_deref().and_then(meta::text_string) {
            out.fact(k, Value::text(s));
        }
    }
    for (k, v) in [
        (keys::CREATED, &m.creation_date),
        (keys::MODIFIED, &m.modification_date),
    ] {
        if let Some(d) = v.as_ref().and_then(meta::to_date) {
            out.fact(k, Value::Date(d));
        }
    }
    let c = meta::catalog_facts(pdf, bytes);
    out.fact(keys::HAS_XMP, Value::Bool(c.has_xmp));
    out.fact(keys::FORM_FIELDS, int(c.form_fields));
    out.fact(keys::SIGNATURE_FIELDS, int(c.signature_fields));
    out.fact(keys::SIGNED, Value::Bool(c.signed_fields > 0));
    out.fact(keys::HAS_JAVASCRIPT, Value::Bool(c.has_javascript));
    out.fact(keys::EMBEDDED_FILES, int(c.embedded_files));
}

/// One page before zoning: blocks still need their zone.
struct Pending {
    page: Page,
    bboxes: Vec<quail_core::BBox>,
}

#[allow(clippy::too_many_lines)]
fn pages(pdf: &Pdf, file_len: usize, limits: &Limits, out: &mut Emitter) -> Document {
    let all = pdf.pages();
    out.fact(keys::PAGE_COUNT, int(all.len()));
    let max = usize::try_from(limits.max_pages).unwrap_or(usize::MAX);
    if all.len() > max {
        out.problem(format!(
            "{} pages, only the first {max} extracted (max_pages)",
            all.len()
        ));
    }

    let settings = InterpreterSettings::default();
    let mut cache = InterpreterCache::new();
    let mut budget = Budget::new(limits.max_decompressed, file_len);
    let mut text_left = usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX);
    let mut fonts = BTreeSet::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut image_only_pages = 0usize;

    for (i, page) in all.iter().take(max).enumerate() {
        let index = u32::try_from(i).unwrap_or(u32::MAX);
        if out.check_deadline().is_err() {
            out.problem(format!("timeout: stopped before page {index}"));
            break;
        }
        if !budget.charge_page(page) {
            out.problem(format!(
                "max_decompressed reached: stopped before page {index}"
            ));
            break;
        }
        let _ = catch_unwind(AssertUnwindSafe(|| meta::page_fonts(page, &mut fonts)));

        let (w, h) = page.render_dimensions();
        let captured = catch_unwind(AssertUnwindSafe(|| capture(page, &cache, &settings, w, h)));
        let cap = match captured {
            Ok(c) => c,
            Err(p) => {
                out.problem(format!(
                    "page {index}: interpreter failed: {}",
                    panic_msg(&*p)
                ));
                // The cache may hold half-built state now.
                cache = InterpreterCache::new();
                pending.push(Pending {
                    page: Page {
                        size: Some((w, h)),
                        needs_ocr: true,
                        ..Page::new(index, PageKind::Physical)
                    },
                    bboxes: Vec::new(),
                });
                continue;
            }
        };
        if cap.truncated > 0 {
            out.warn(format!(
                "page {index}: {} glyphs beyond the per-page cap dropped",
                cap.truncated
            ));
        }

        let coverage = cap.image_coverage();
        let (images, placements, unmapped, paths) =
            (cap.images, cap.placements, cap.unmapped, cap.paths);
        let blocks = layout::layout(cap.glyphs);

        let mut text = String::new();
        let mut kept: Vec<(Span, quail_core::BBox)> = Vec::with_capacity(blocks.len());
        let mut truncated_text = false;
        for b in &blocks {
            let t = b.text();
            if t.len() + 1 > text_left {
                truncated_text = true;
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(&t);
            text_left -= t.len() + 1;
            let span = Span {
                start: u32::try_from(start).unwrap_or(u32::MAX),
                end: u32::try_from(text.len()).unwrap_or(u32::MAX),
            };
            kept.push((span, b.bbox));
        }
        if truncated_text {
            out.problem(format!(
                "max_text_bytes reached on page {index}: text truncated"
            ));
        }

        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        let verdict = judge(&Signals {
            chars,
            unmapped,
            garbage: garbage_ratio(&text),
            image_coverage: coverage,
            images,
            paths,
        });
        if !verdict.keep_text && chars > 0 {
            out.warn(format!("page {index}: text layer rejected as garbage"));
            text.clear();
            kept.clear();
        }
        let text_source = if text.is_empty() {
            TextSource::None
        } else {
            TextSource::Native
        };
        let blocks_out = kept
            .iter()
            .map(|(span, bbox)| Block {
                span: *span,
                kind: BlockKind::Paragraph,
                zone: Zone::Unknown,
                bbox: Some(*bbox),
            })
            .collect();
        pending.push(Pending {
            page: Page {
                size: Some((w, h)),
                text,
                blocks: blocks_out,
                text_source,
                needs_ocr: verdict.needs_ocr,
                images: placements,
                image_coverage: coverage,
                ..Page::new(index, PageKind::Physical)
            },
            bboxes: kept.into_iter().map(|(_, b)| b).collect(),
        });
        image_only_pages += usize::from(verdict.image_only);
    }

    // Zones need every page: repetition is a cross-page signal.
    let zone_in: Vec<(f32, Vec<zones::ZoneInput<'_>>)> = pending
        .iter()
        .map(|p| {
            let h = p.page.size.map_or(0.0, |s| s.1);
            let blocks = p
                .page
                .blocks
                .iter()
                .zip(&p.bboxes)
                .map(|(b, bbox)| zones::ZoneInput {
                    text: b.span.slice(&p.page.text),
                    bbox: *bbox,
                })
                .collect();
            (h, blocks)
        })
        .collect();
    let assigned = zones::assign(&zone_in);
    drop(zone_in);
    let mut doc = Document::default();
    for (p, z) in pending.into_iter().zip(assigned) {
        let mut page = p.page;
        for (b, zone) in page.blocks.iter_mut().zip(z) {
            b.zone = zone;
        }
        doc.pages.push(page);
    }

    out.fact(
        keys::FONTS,
        Value::List(fonts.into_iter().map(Value::text).collect()),
    );
    out.fact(
        keys::IMAGE_ONLY_PAGES,
        int(image_only_pages),
    );
    out.fact(
        keys::PAGES_NEEDING_OCR,
        int(doc.pages.iter().filter(|p| p.needs_ocr).count()),
    );
    doc
}

pub(crate) fn capture<'a>(
    page: &HPage<'a>,
    cache: &InterpreterCache<'a>,
    settings: &InterpreterSettings,
    w: f32,
    h: f32,
) -> PageCapture {
    let (w, h) = (f64::from(w), f64::from(h));
    let mut ctx = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, w, h),
        cache,
        page.xref(),
        settings.clone(),
    );
    let mut dev = PageCapture::new(w, h);
    interpret_page(page, &mut ctx, &mut dev);
    dev
}

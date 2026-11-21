//! The extractor: parse, emit file facts, walk pages, lay out text, judge text quality,
//! assign zones.
//!
//! Memory: every stream decode is charged to one per-file budget (`max_decompressed`, in the
//! vendored hayro-syntax), and hayro keeps each page's decoded content cached for the life of
//! the parsed document. So the document is dropped and re-parsed whenever the bytes decoded
//! since it was opened pass [`REOPEN_AFTER_BYTES`]: what a long or hostile file holds at once
//! stays near one page's worth plus that margin.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use hayro_interpret::kurbo::Rect;
use hayro_interpret::util::TransformExt;
use hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
use hayro_syntax::budget::DecodeBudget;
use hayro_syntax::page::Page as HPage;
use hayro_syntax::{Pdf, PdfVersion};
use quail_core::{
    AnalyzerId, Blob, Block, BlockKind, Document, Emitter, Error, Extractor, Format, Limits,
    LineBox, Page, PageKind, Raster, Result, Span, TextSource, Value, Zone,
};

use crate::device::{
    Abort, Captured, Clock, MAX_CALLS_PER_PAGE, MAX_NESTINGS_PER_PAGE, PageCapture, Watchdog,
};
use crate::open::{Caught, caught, open};
use crate::quality::{Signals, garbage_ratio, judge};
use crate::raster::PdfRaster;
use crate::{keys, layout, meta, raw, zones};

/// Decoded bytes after which the parsed document is dropped and re-opened, releasing hayro's
/// per-page content caches.
pub(crate) const REOPEN_AFTER_BYTES: u64 = 64 << 20;
/// Below this much budget left after a refused decode, the file is treated as spent.
const BUDGET_SPENT: u64 = 1 << 20;

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

    /// The `raster` artifact (`PdfRaster`), for page renders and embedded scans.
    fn provides(&self) -> &'static [&'static str] {
        &["raster"]
    }

    fn extract(&self, blob: &Blob, limits: &Limits, out: &mut Emitter) -> Result<Document> {
        extract(blob, limits, out)
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
    // The file's time budget: the engine's deadline when it set one (so the raster's later
    // calls share it), otherwise one starting now.
    let file_deadline = out
        .deadline()
        .or_else(|| Instant::now().checked_add(limits.timeout));
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

    let budget = crate::open::budget(limits);
    let pdf = match open(blob, &budget) {
        Ok(pdf) => pdf,
        Err(e) => {
            if let Some(v) = &scan.header_version {
                out.fact(keys::VERSION, Value::text(v.as_str()));
            }
            out.fact(
                keys::ENCRYPTED,
                Value::Bool(e.encrypted() || scan.encrypt_entry),
            );
            return Err(e.into());
        }
    };

    out.fact(keys::VERSION, Value::text(version_str(pdf.version())));
    out.fact(keys::ENCRYPTED, Value::Bool(scan.encrypt_entry));
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| document_facts(&pdf, bytes, out))) {
        out.problem(format!(
            "document facts skipped: {}",
            crate::open::panic_msg(&*p)
        ));
    }
    if budget.take_limit_hit() {
        out.problem(
            "max_decompressed: some objects were not decoded; document facts may be incomplete",
        );
    }

    let (doc, no_scan) = pages(pdf, blob, &budget, limits, out);
    // Later steps (OCR) get pixels through the core trait and never depend on this crate.
    out.provide(Raster(Arc::new(
        PdfRaster::with_no_scan(blob.clone(), no_scan).until(file_deadline),
    )));
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
    for (k, tz, v) in [
        (keys::CREATED, keys::CREATED_UTC_OFFSET, &m.creation_date),
        (
            keys::MODIFIED,
            keys::MODIFIED_UTC_OFFSET,
            &m.modification_date,
        ),
    ] {
        if let Some(dt) = v.as_ref()
            && let Some(d) = meta::to_date(dt)
        {
            out.fact(k, Value::Date(d));
            if let Some(off) = meta::utc_offset(dt) {
                out.fact(tz, Value::text(off));
            }
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

/// State carried across pages (and across re-opens of the document).
struct Walk<'o> {
    out: &'o mut Emitter,
    budget: &'o DecodeBudget,
    text_left: usize,
    fonts: BTreeSet<String>,
    pending: Vec<Pending>,
    image_only_pages: usize,
    /// Per page: interpreted completely (no abort, no refused decode) and drew a number of
    /// images other than one, so it holds no single embedded scan.
    no_scan: Vec<bool>,
}

/// Whether to go on with the next page.
#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

fn pages(
    first: Pdf,
    blob: &Blob,
    budget: &Arc<DecodeBudget>,
    limits: &Limits,
    out: &mut Emitter,
) -> (Document, Vec<bool>) {
    let total = first.pages().len();
    out.fact(keys::PAGE_COUNT, int(total));
    let max = usize::try_from(limits.max_pages).unwrap_or(usize::MAX);
    if total > max {
        out.problem(format!(
            "{total} pages, only the first {max} extracted (max_pages)"
        ));
    }
    let n = total.min(max);
    if total >= hayro_syntax::page::MAX_PAGES {
        out.problem(format!(
            "page tree truncated at {} pages",
            hayro_syntax::page::MAX_PAGES
        ));
    }

    let mut walk = Walk {
        out,
        budget,
        text_left: usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX),
        fonts: BTreeSet::new(),
        pending: Vec::new(),
        image_only_pages: 0,
        no_scan: Vec::new(),
    };
    let settings = InterpreterSettings::default();
    let mut next = 0usize;
    let mut doc = Some(first);
    'docs: while next < n {
        let pdf = match doc.take() {
            Some(p) => p,
            None => match open(blob, budget) {
                Ok(p) => p,
                Err(e) => {
                    walk.out
                        .problem(format!("re-open before page {next} failed: {e:?}"));
                    break;
                }
            },
        };
        let mark = budget.remaining();
        let mut cache = InterpreterCache::new();
        let all = pdf.pages();
        if all.len() <= next {
            break;
        }
        let start = next;
        for i in start..n.min(all.len()) {
            let Some(page) = all.get(i) else {
                break 'docs;
            };
            next = i + 1;
            let index = u32::try_from(i).unwrap_or(u32::MAX);
            let (flow, reset) = walk.page(page, index, &cache, &settings);
            if reset {
                // The cache may hold half-built state after an unwind.
                cache = InterpreterCache::new();
            }
            if flow == Flow::Stop {
                break 'docs;
            }
            if mark.saturating_sub(budget.remaining()) > REOPEN_AFTER_BYTES {
                // Drop this parse (and its per-page caches); the next one starts at `next`.
                continue 'docs;
            }
        }
    }

    let Walk {
        out,
        fonts,
        mut pending,
        image_only_pages,
        no_scan,
        ..
    } = walk;

    // Zones need every page: repetition is a cross-page signal.
    if out.check_deadline().is_ok() {
        assign_zones(&mut pending);
    } else {
        out.problem("timeout: zones not assigned");
        for p in &mut pending {
            for b in &mut p.page.blocks {
                b.zone = Zone::Body;
            }
        }
    }
    let doc = Document {
        pages: pending.into_iter().map(|p| p.page).collect(),
    };

    out.fact(
        keys::FONTS,
        Value::List(fonts.into_iter().map(Value::text).collect()),
    );
    out.fact(keys::IMAGE_ONLY_PAGES, int(image_only_pages));
    out.fact(
        keys::PAGES_NEEDING_OCR,
        int(doc.pages.iter().filter(|p| p.needs_ocr).count()),
    );
    (doc, no_scan)
}

fn assign_zones(pending: &mut [Pending]) {
    let assigned = {
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
        zones::assign(&zone_in)
    };
    for (p, z) in pending.iter_mut().zip(assigned) {
        for (b, zone) in p.page.blocks.iter_mut().zip(z) {
            b.zone = zone;
        }
    }
}

impl Walk<'_> {
    fn mark_no_scan(&mut self, index: u32, no_scan: bool) {
        // `index` is below `max_pages` and the page count: the vector stays page-sized.
        if let (true, Ok(i)) = (no_scan, usize::try_from(index)) {
            self.no_scan.resize(self.no_scan.len().max(i + 1), false);
            self.no_scan[i] = true;
        }
    }

    fn failed_page(&mut self, index: u32, size: (f32, f32), needs_ocr: bool) {
        self.pending.push(Pending {
            page: Page {
                size: Some(size),
                needs_ocr,
                ..Page::new(index, PageKind::Physical)
            },
            bboxes: Vec::new(),
        });
    }

    /// Extracts one page. Returns whether to go on, and whether the interpreter cache must be
    /// rebuilt (after an unwind out of hayro).
    #[allow(clippy::too_many_lines)]
    fn page<'a>(
        &mut self,
        page: &HPage<'a>,
        index: u32,
        cache: &InterpreterCache<'a>,
        settings: &InterpreterSettings,
    ) -> (Flow, bool) {
        if self.out.check_deadline().is_err() {
            self.out
                .problem(format!("timeout: stopped before page {index}"));
            return (Flow::Stop, false);
        }
        let _ = catch_unwind(AssertUnwindSafe(|| meta::page_fonts(page, &mut self.fonts)));

        let (w, h) = page.render_dimensions();
        let captured = {
            let out: &Emitter = self.out;
            catch_unwind(AssertUnwindSafe(|| {
                capture(
                    page,
                    cache,
                    settings,
                    w,
                    h,
                    Watchdog::new(Clock::Emitter(out)),
                )
            }))
        };
        let cap = match captured {
            Ok(c) => c,
            Err(p) => {
                return match caught(&*p) {
                    Caught::Abort(Abort::Deadline) => {
                        self.out
                            .problem(format!("timeout on page {index}: interpretation stopped"));
                        self.failed_page(index, (w, h), false);
                        (Flow::Stop, true)
                    }
                    Caught::Abort(Abort::TooManyCalls) => {
                        self.out.problem(format!(
                            "page {index}: more than {MAX_CALLS_PER_PAGE} draw operations or {MAX_NESTINGS_PER_PAGE} nested forms/clips, skipped"
                        ));
                        self.failed_page(index, (w, h), true);
                        (Flow::Continue, true)
                    }
                    Caught::Panic(m) => {
                        self.out
                            .problem(format!("page {index}: interpreter failed: {m}"));
                        self.failed_page(index, (w, h), true);
                        (Flow::Continue, true)
                    }
                };
            }
        };
        let mut flow = Flow::Continue;
        let limit_hit = self.budget.take_limit_hit();
        self.mark_no_scan(index, !limit_hit && cap.images != 1);
        if limit_hit {
            self.out.problem(format!(
                "page {index}: a stream or image exceeded max_decompressed or max_render_pixels and was skipped"
            ));
            if self.budget.remaining() < BUDGET_SPENT {
                self.out.problem(format!(
                    "max_decompressed reached: stopped after page {index}"
                ));
                flow = Flow::Stop;
            }
        }
        if cap.truncated > 0 {
            self.out.warn(format!(
                "page {index}: {} glyphs beyond the per-page cap dropped",
                cap.truncated
            ));
        }
        if self.out.check_deadline().is_err() {
            self.out
                .problem(format!("timeout after interpreting page {index}"));
            flow = Flow::Stop;
        }

        let coverage = cap.image_coverage;
        let (images, placements, unmapped, paths) =
            (cap.images, cap.placements, cap.unmapped, cap.paths);
        let blocks = layout::layout(cap.glyphs);
        if self.out.check_deadline().is_err() {
            self.out
                .problem(format!("timeout after laying out page {index}"));
            flow = Flow::Stop;
        }

        let mut text = String::new();
        let mut kept: Vec<(Span, quail_core::BBox)> = Vec::with_capacity(blocks.len());
        let mut kept_lines: Vec<Vec<LineBox>> = Vec::with_capacity(blocks.len());
        let mut truncated_text = false;
        let text_left_before = self.text_left;
        for b in &blocks {
            let t = b.text();
            if t.len() + 1 > self.text_left {
                truncated_text = true;
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(&t);
            self.text_left -= t.len() + 1;
            let span = Span {
                start: u32::try_from(start).unwrap_or(u32::MAX),
                end: u32::try_from(text.len()).unwrap_or(u32::MAX),
            };
            kept.push((span, b.bbox));
            // The block text is its lines joined by `\n`: each line's span follows from the
            // line lengths, and its box is the one the layout measured.
            let mut at = start;
            let mut lines = Vec::with_capacity(b.lines.len());
            for l in &b.lines {
                let end = at.saturating_add(l.text.len());
                lines.push(LineBox {
                    span: Span {
                        start: u32::try_from(at).unwrap_or(u32::MAX),
                        end: u32::try_from(end).unwrap_or(u32::MAX),
                    },
                    bbox: l.bbox,
                });
                at = end.saturating_add(1);
            }
            kept_lines.push(lines);
        }
        if truncated_text {
            self.out.problem(format!(
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
            self.out
                .warn(format!("page {index}: text layer rejected as garbage"));
            text.clear();
            kept.clear();
            kept_lines.clear();
            // Rejected text does not count against max_text_bytes.
            self.text_left = text_left_before;
        }
        let text_source = if text.is_empty() {
            TextSource::None
        } else {
            TextSource::Native
        };
        let blocks_out = kept
            .iter()
            .zip(kept_lines)
            .map(|((span, bbox), lines)| {
                let mut b = Block::new(*span, BlockKind::Paragraph, Zone::Unknown, Some(*bbox));
                b.lines = lines;
                b
            })
            .collect();
        self.pending.push(Pending {
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
        self.image_only_pages += usize::from(verdict.image_only);
        (flow, false)
    }
}

pub(crate) fn capture<'a>(
    page: &HPage<'a>,
    cache: &InterpreterCache<'a>,
    settings: &InterpreterSettings,
    w: f32,
    h: f32,
    watchdog: Watchdog<'_>,
) -> Captured {
    let (w, h) = (f64::from(w), f64::from(h));
    let mut ctx = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, w, h),
        cache,
        page.xref(),
        settings.clone(),
    );
    let mut dev = PageCapture::new(w, h, watchdog);
    interpret_page(page, &mut ctx, &mut dev);
    dev.finish()
}

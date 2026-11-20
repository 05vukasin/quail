//! Inspection packs: the metadata next to the file, for a person or an AI agent to read.
//!
//! For each chosen record, `<out>/<run>/<id>/` gets:
//!
//! - `record.json`: the record, pretty-printed;
//! - `inspect.md`: file identity (and whether the file on disk still has the recorded hash),
//!   gold vs predicted per task with the match result, every fact, and the text of every
//!   page with each fact's evidence span marked `⟦n|…|n⟧` (n = fact number in the facts
//!   table) and zone changes marked `«zone»`;
//! - page images, once a [`PageRenderer`] exists (none ships yet: the hook is there).
//!
//! Page text is not part of a record, so it comes from a [`DocumentSource`]: the file itself
//! for plain-text files ([`TextFiles`]), a JSONL dump of documents ([`Sidecar`]), or any
//! quail [`Extractor`] re-run on the file ([`Extractors`]).
//!
//! Packs contain document text: they go to a gitignored directory (`eval/inspect/`), and
//! each run directory gets its own `.gitignore` as a second guard.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use quail_core::{
    Document, Emitter, Extractor, Fact, FileRecord, Format, Limits, Page, PageKind, Status,
    TextSource, Zone,
};
use serde::{Deserialize, Serialize};

use crate::gold::GoldDoc;
use crate::records::{RecordIndex, Records, blake3_of, value_string};
use crate::score::{DocTask, predictions, score_doc_task, status_name};
use crate::tasks::TaskMap;
use crate::{Error, Result};

/// Where the pages of a record come from.
pub trait DocumentSource {
    /// The document of `rec`, or `None` when this source cannot provide it.
    fn document(&self, rec: &FileRecord) -> Result<Option<Document>>;
}

/// Plain-text files: the file at the record's locator is page 0 (UTF-8 only, so evidence
/// byte spans stay valid).
#[derive(Clone, Copy, Debug)]
pub struct TextFiles {
    /// Larger files are skipped.
    pub max_bytes: u64,
}

impl Default for TextFiles {
    fn default() -> Self {
        Self {
            max_bytes: 16 << 20,
        }
    }
}

/// A one-page logical document holding `text`.
pub fn text_document(text: String) -> Document {
    Document {
        pages: vec![Page {
            index: 0,
            kind: PageKind::Logical,
            size: None,
            text,
            blocks: vec![],
            text_source: TextSource::Native,
            needs_ocr: false,
        }],
    }
}

impl DocumentSource for TextFiles {
    fn document(&self, rec: &FileRecord) -> Result<Option<Document>> {
        if rec.format != Format::Text {
            return Ok(None);
        }
        let path = Path::new(&rec.locator);
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() && m.len() <= self.max_bytes => {}
            _ => return Ok(None),
        }
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Ok(String::from_utf8(bytes).ok().map(text_document))
    }
}

/// One line of a document dump: `{"locator": ..., "blake3": ..., "document": <Document>}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidecarLine {
    /// Locator of the file.
    pub locator: String,
    /// BLAKE3 hex, when known.
    #[serde(default)]
    pub blake3: Option<String>,
    /// The extracted document (quail-core's serde form).
    pub document: Document,
}

/// Documents from a JSONL dump ([`SidecarLine`] per line), matched by BLAKE3 then locator.
#[derive(Debug, Default)]
pub struct Sidecar {
    docs: Vec<Document>,
    by_key: HashMap<String, usize>,
}

impl Sidecar {
    /// Reads a dump, keeping only documents whose hash or locator is in `wanted` (all when
    /// `wanted` is `None`).
    pub fn load(path: &Path, wanted: Option<&std::collections::HashSet<String>>) -> Result<Self> {
        use std::io::BufRead as _;
        let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let mut out = Self::default();
        for (n, line) in std::io::BufReader::new(f).lines().enumerate() {
            let line = line.map_err(|e| Error::io(path, e))?;
            if line.trim().is_empty() {
                continue;
            }
            let l: SidecarLine = serde_json::from_str(&line)
                .map_err(|e| Error::parse(path, format!("line {}: {e}", n + 1)))?;
            let keep = wanted.is_none_or(|w| {
                w.contains(&l.locator) || l.blake3.as_ref().is_some_and(|h| w.contains(h))
            });
            if !keep {
                continue;
            }
            let i = out.docs.len();
            out.docs.push(l.document);
            out.by_key.entry(l.locator).or_insert(i);
            if let Some(h) = l.blake3 {
                out.by_key.entry(h).or_insert(i);
            }
        }
        Ok(out)
    }
}

impl DocumentSource for Sidecar {
    fn document(&self, rec: &FileRecord) -> Result<Option<Document>> {
        let i = blake3_of(rec)
            .and_then(|h| self.by_key.get(&h).copied())
            .or_else(|| self.by_key.get(&rec.locator).copied());
        Ok(i.and_then(|i| self.docs.get(i).cloned()))
    }
}

/// Re-runs a quail extractor on the file at the record's locator.
pub struct Extractors {
    /// Extractors, first match by format wins.
    pub extractors: Vec<Box<dyn Extractor>>,
    /// Limits passed to them.
    pub limits: Limits,
}

impl DocumentSource for Extractors {
    fn document(&self, rec: &FileRecord) -> Result<Option<Document>> {
        let Some(x) = self
            .extractors
            .iter()
            .find(|x| x.formats().contains(&rec.format))
        else {
            return Ok(None);
        };
        let path = Path::new(&rec.locator);
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(None);
        };
        let mut em = Emitter::new(x.id());
        Ok(Some(x.extract(
            &quail_core::Blob::from(bytes),
            &self.limits,
            &mut em,
        )?))
    }
}

/// Tries each source in order.
#[derive(Default)]
pub struct Chain(pub Vec<Box<dyn DocumentSource>>);

impl DocumentSource for Chain {
    fn document(&self, rec: &FileRecord) -> Result<Option<Document>> {
        for s in &self.0 {
            if let Some(d) = s.document(rec)? {
                return Ok(Some(d));
            }
        }
        Ok(None)
    }
}

/// Extension point for page images: renders one page of the file into `out_dir` and returns
/// the image path. No renderer ships yet (it needs the PDF renderer / OCR stack).
pub trait PageRenderer {
    /// Renders page `page` of `rec`; `None` when it cannot.
    fn render(&self, rec: &FileRecord, page: &Page, out_dir: &Path) -> Result<Option<PathBuf>>;
}

/// Renders nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRenderer;

impl PageRenderer for NoRenderer {
    fn render(&self, _: &FileRecord, _: &Page, _: &Path) -> Result<Option<PathBuf>> {
        Ok(None)
    }
}

/// A sampling filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Some task has an FP or FN against gold.
    Mismatch,
    /// Some fact of a mapped task has `conf` below the low-confidence threshold.
    LowConf,
    /// Status `failed`.
    Failed,
    /// Status `partial`.
    Partial,
}

impl std::str::FromStr for Where {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "mismatch" => Ok(Self::Mismatch),
            "low_conf" => Ok(Self::LowConf),
            "failed" => Ok(Self::Failed),
            "partial" => Ok(Self::Partial),
            _ => Err(format!(
                "unknown filter {s:?} (mismatch, low_conf, failed, partial)"
            )),
        }
    }
}

/// Which records to inspect.
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    /// Records whose BLAKE3 starts with one of these prefixes.
    Blake3(Vec<String>),
    /// Records whose locator contains one of these strings.
    Locator(Vec<String>),
    /// `n` records matching any filter (all records when `wheres` is empty), chosen by a
    /// seeded hash so the same seed gives the same sample.
    Sample {
        /// How many.
        n: usize,
        /// Filters, OR-ed.
        wheres: Vec<Where>,
        /// Seed.
        seed: u64,
    },
}

/// Records, gold and task map, with the record ↔ gold matching done once.
pub struct Inputs<'a> {
    /// Records of the run.
    pub records: &'a Records,
    /// Gold documents (may be empty).
    pub gold: &'a [GoldDoc],
    /// Task map.
    pub tasks: &'a TaskMap,
    /// Global minimum confidence, as in scoring.
    pub min_conf: Option<f32>,
    gold_of: HashMap<usize, usize>,
}

impl<'a> Inputs<'a> {
    /// Matches gold to records.
    pub fn new(
        records: &'a Records,
        gold: &'a [GoldDoc],
        tasks: &'a TaskMap,
        min_conf: Option<f32>,
    ) -> Self {
        let ix = RecordIndex::new(&records.records);
        let gold_of = gold
            .iter()
            .enumerate()
            .filter_map(|(g, d)| ix.find(&records.records, &d.file).map(|(r, _)| (r, g)))
            .collect();
        Self {
            records,
            gold,
            tasks,
            min_conf,
            gold_of,
        }
    }

    /// The gold document of record `i`.
    pub fn gold_for(&self, i: usize) -> Option<&'a GoldDoc> {
        self.gold_of.get(&i).and_then(|&g| self.gold.get(g))
    }

    /// Scored tasks of record `i` against its gold (empty without gold).
    pub fn doc_tasks(&self, i: usize) -> Vec<(String, DocTask)> {
        let (Some(doc), Some(rec)) = (self.gold_for(i), self.records.records.get(i)) else {
            return vec![];
        };
        self.tasks
            .tasks
            .iter()
            .filter_map(|(name, spec)| {
                let (preds, _) = predictions(rec, spec, self.min_conf);
                score_doc_task(doc, name, spec, preds).map(|t| (name.clone(), t))
            })
            .collect()
    }
}

fn mapped_low_conf(rec: &FileRecord, tasks: &TaskMap, low: f32) -> bool {
    rec.facts.iter().any(|f| {
        f.conf < low
            && tasks
                .tasks
                .values()
                .any(|s| s.keys.iter().any(|k| k == f.key.as_str()))
    })
}

/// Indices of the chosen records, in a stable order.
pub fn select(inputs: &Inputs<'_>, sel: &Selection, low_conf: f32) -> Vec<usize> {
    let recs = &inputs.records.records;
    match sel {
        Selection::Blake3(prefixes) => (0..recs.len())
            .filter(|&i| {
                blake3_of(&recs[i])
                    .is_some_and(|h| prefixes.iter().any(|p| h.starts_with(&p.to_lowercase())))
            })
            .collect(),
        Selection::Locator(parts) => (0..recs.len())
            .filter(|&i| parts.iter().any(|p| recs[i].locator.contains(p.as_str())))
            .collect(),
        Selection::Sample { n, wheres, seed } => {
            let mut cands: Vec<(String, usize)> = (0..recs.len())
                .filter(|&i| {
                    let r = &recs[i];
                    wheres.is_empty()
                        || wheres.iter().any(|w| match w {
                            Where::Mismatch => {
                                inputs.doc_tasks(i).iter().any(|(_, t)| t.is_mismatch())
                            }
                            Where::LowConf => mapped_low_conf(r, inputs.tasks, low_conf),
                            Where::Failed => matches!(r.status, Status::Failed { .. }),
                            Where::Partial => matches!(r.status, Status::Partial { .. }),
                        })
                })
                .map(|i| {
                    let key = blake3_of(&recs[i]).unwrap_or_else(|| recs[i].locator.clone());
                    (crate::blake3_hex(format!("{seed}:{key}").as_bytes()), i)
                })
                .collect();
            cands.sort();
            cands.into_iter().take(*n).map(|(_, i)| i).collect()
        }
    }
}

/// Options of [`write_packs`].
#[derive(Clone, Debug)]
pub struct PackOptions {
    /// Root of all packs, e.g. `eval/inspect`.
    pub out_dir: PathBuf,
    /// Run name: packs go to `<out_dir>/<run>/`.
    pub run: String,
    /// Compare the file on disk with the recorded hash.
    pub check_file: bool,
}

/// What [`write_packs`] wrote.
#[derive(Clone, Debug, Default)]
pub struct PackSummary {
    /// `<out_dir>/<run>`.
    pub dir: PathBuf,
    /// One directory per record.
    pub packs: Vec<PathBuf>,
}

fn pack_id(rec: &FileRecord) -> String {
    let h = blake3_of(rec).unwrap_or_else(|| crate::blake3_hex(rec.locator.as_bytes()));
    h.chars().take(16).collect()
}

/// Writes one pack per chosen record and an `index.md`.
pub fn write_packs(
    inputs: &Inputs<'_>,
    chosen: &[usize],
    docs: &dyn DocumentSource,
    renderer: &dyn PageRenderer,
    opts: &PackOptions,
) -> Result<PackSummary> {
    let dir = opts.out_dir.join(&opts.run);
    crate::write_file(
        &dir.join(".gitignore"),
        "# inspection packs hold document text\n*\n",
    )?;
    let mut summary = PackSummary {
        dir: dir.clone(),
        packs: vec![],
    };
    let mut index = format!(
        "# Inspection packs · {}\n\n| pack | locator | status | mismatched tasks |\n|---|---|---|---|\n",
        opts.run
    );
    for &i in chosen {
        let Some(rec) = inputs.records.records.get(i) else {
            continue;
        };
        let id = pack_id(rec);
        let pdir = dir.join(&id);
        let json = serde_json::to_string_pretty(rec)
            .map_err(|e| Error::Invalid(format!("serializing record: {e}")))?;
        crate::write_file(&pdir.join("record.json"), json + "\n")?;
        let tasks = inputs.doc_tasks(i);
        let document = match docs.document(rec) {
            Ok(d) => d.map(Ok),
            Err(e) => Some(Err(e.to_string())),
        };
        let md = pack_markdown(
            inputs,
            i,
            &tasks,
            document,
            renderer,
            &pdir,
            opts.check_file,
        )?;
        crate::write_file(&pdir.join("inspect.md"), md)?;
        let bad: Vec<&str> = tasks
            .iter()
            .filter(|(_, t)| t.is_mismatch())
            .map(|(n, _)| n.as_str())
            .collect();
        let _ = writeln!(
            index,
            "| [{id}]({id}/inspect.md) | {} | {} | {} |",
            cell(&rec.locator, 120),
            status_name(&rec.status),
            bad.join(", ")
        );
        summary.packs.push(pdir);
    }
    crate::write_file(&dir.join("index.md"), index)?;
    Ok(summary)
}

/// A Markdown table cell: no pipes or newlines, at most `max` characters.
fn cell(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let flat = flat.replace('|', "\\|");
    if flat.chars().count() > max {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

fn fact_value(f: &Fact) -> String {
    match &f.value {
        quail_core::Value::List(items) if items.len() > 8 => {
            let head: Vec<String> = items.iter().take(8).map(value_string).collect();
            format!("[{} … ({} items)]", head.join(", "), items.len())
        }
        v => value_string(v),
    }
}

fn zone_name(z: Zone) -> &'static str {
    match z {
        Zone::Unknown => "unknown",
        Zone::Header => "header",
        Zone::Footer => "footer",
        Zone::Body => "body",
        Zone::Title => "title",
        Zone::Table => "table",
        Zone::Signature => "signature",
    }
}

fn file_check(rec: &FileRecord) -> String {
    let path = Path::new(&rec.locator);
    let Some(want) = blake3_of(rec) else {
        return "no hash in the record".into();
    };
    match std::fs::read(path) {
        Ok(bytes) if crate::blake3_hex(&bytes) == want => "same bytes as recorded".into(),
        Ok(_) => "**CHANGED since the run** (hash differs)".into(),
        Err(_) => "not readable at the locator".into(),
    }
}

#[allow(clippy::too_many_lines)]
fn pack_markdown(
    inputs: &Inputs<'_>,
    i: usize,
    tasks: &[(String, DocTask)],
    document: Option<std::result::Result<Document, String>>,
    renderer: &dyn PageRenderer,
    pdir: &Path,
    check_file: bool,
) -> Result<String> {
    let rec = &inputs.records.records[i];
    let mut s = String::new();
    let _ = writeln!(s, "# {}\n", cell(&rec.locator, 300));
    let _ = writeln!(s, "- file: `{}`", rec.locator);
    let _ = writeln!(
        s,
        "- blake3: `{}`",
        blake3_of(rec).unwrap_or_else(|| "?".into())
    );
    if check_file {
        let _ = writeln!(s, "- file on disk: {}", file_check(rec));
    }
    let _ = writeln!(
        s,
        "- format: {} · size: {} bytes · pages: {}",
        rec.format.as_str(),
        rec.size,
        rec.pages.map_or_else(|| "—".into(), |p| p.to_string())
    );
    match &rec.status {
        Status::Ok => {
            let _ = writeln!(s, "- status: ok");
        }
        Status::Partial { problems } => {
            let _ = writeln!(s, "- status: partial: {}", cell(&problems.join("; "), 500));
        }
        Status::Failed { error } => {
            let _ = writeln!(s, "- status: failed: {}", cell(error, 500));
        }
    }
    for w in &rec.warnings {
        let _ = writeln!(s, "- warning: {}", cell(w, 300));
    }
    let steps: Vec<String> = rec.steps.iter().map(ToString::to_string).collect();
    let _ = writeln!(s, "- steps: {}", steps.join(", "));
    if let Some(g) = inputs.gold_for(i) {
        let _ = writeln!(
            s,
            "- gold: dataset {} · split {}{}",
            g.dataset,
            g.split.as_str(),
            g.source
                .as_ref()
                .map_or_else(String::new, |x| format!(" · source {x}"))
        );
    } else {
        let _ = writeln!(s, "- gold: none for this file");
    }

    // Gold vs predicted.
    let _ = writeln!(s, "\n## Gold vs predicted\n");
    let _ = writeln!(
        s,
        "| task | gold | predicted (conf, fact #) | result |\n|---|---|---|---|"
    );
    let scored: std::collections::HashSet<&str> = tasks.iter().map(|(n, _)| n.as_str()).collect();
    for (name, t) in tasks {
        let gold = if t.absent {
            "*(absent)*".to_string()
        } else {
            cell(&t.gold.join(" · "), 200)
        };
        let preds: Vec<String> = t
            .pred
            .iter()
            .map(|p| {
                format!(
                    "{} {} ({:.2}, #{})",
                    if p.correct { "✓" } else { "✗" },
                    cell(&p.value, 120),
                    p.conf,
                    p.fact + 1
                )
            })
            .collect();
        let result = if t.is_mismatch() {
            let mut r = format!("**TP {} FP {} FN {}**", t.tp, t.fp, t.fn_);
            if !t.missed.is_empty() {
                let _ = write!(r, " missed: {}", cell(&t.missed.join(" · "), 120));
            }
            r
        } else if t.absent {
            "correct abstention".into()
        } else {
            format!("TP {}", t.tp)
        };
        let _ = writeln!(
            s,
            "| {name} | {gold} | {} | {result} |",
            if preds.is_empty() {
                "*(none)*".into()
            } else {
                preds.join("<br>")
            }
        );
    }
    for (name, spec) in &inputs.tasks.tasks {
        if scored.contains(name.as_str()) {
            continue;
        }
        let (preds, _) = predictions(rec, spec, inputs.min_conf);
        if preds.is_empty() {
            continue;
        }
        let p: Vec<String> = preds
            .iter()
            .map(|p| format!("{} ({:.2}, #{})", cell(&p.value, 120), p.conf, p.fact + 1))
            .collect();
        let _ = writeln!(s, "| {name} | *(not annotated)* | {} | — |", p.join("<br>"));
    }

    // Facts.
    let _ = writeln!(
        s,
        "\n## Facts\n\n| # | key | value | conf | src | page | zone | span |\n|---:|---|---|---:|---|---:|---|---|"
    );
    for (n, f) in rec.facts.iter().enumerate() {
        let ev = f.evidence.as_ref();
        let _ = writeln!(
            s,
            "| {} | {} | {} | {:.2} | {} | {} | {} | {} |",
            n + 1,
            f.key,
            cell(&fact_value(f), 160),
            f.conf,
            f.src,
            ev.and_then(|e| e.page)
                .map_or_else(|| "—".into(), |p| (p + 1).to_string()),
            ev.map_or("—", |e| zone_name(e.zone)),
            ev.and_then(|e| e.span)
                .map_or_else(|| "—".into(), |sp| format!("{}..{}", sp.start, sp.end)),
        );
    }

    // Pages.
    let _ = writeln!(s, "\n## Pages\n");
    match document {
        None => {
            let _ = writeln!(
                s,
                "*No page text available*: no document source covers this file (plain-text files are read directly; others need `--documents <dump.jsonl>` or an extractor)."
            );
        }
        Some(Err(e)) => {
            let _ = writeln!(s, "*Document source failed*: {}", cell(&e, 300));
        }
        Some(Ok(doc)) => {
            let _ = writeln!(
                s,
                "Markers: `⟦n|text|n⟧` = evidence of fact #n · `«zone»` = zone change. Page numbers are 1-based; spans are byte offsets.\n"
            );
            for page in &doc.pages {
                let (text, bad) = highlight(page, &rec.facts);
                let _ = writeln!(
                    s,
                    "### Page {} ({:?}, text {:?}{})\n",
                    page.index + 1,
                    page.kind,
                    page.text_source,
                    if page.needs_ocr { ", needs OCR" } else { "" }
                );
                match renderer.render(rec, page, pdir)? {
                    Some(img) => {
                        let rel = img.strip_prefix(pdir).unwrap_or(&img);
                        let _ = writeln!(s, "![page {}]({})\n", page.index + 1, rel.display());
                    }
                    None => {
                        let _ = writeln!(s, "*(no page image: no renderer configured)*\n");
                    }
                }
                if !bad.is_empty() {
                    let list: Vec<String> = bad.iter().map(|n| format!("#{n}")).collect();
                    let _ = writeln!(s, "Spans outside this page's text: {}\n", list.join(", "));
                }
                let _ = writeln!(s, "````text\n{text}\n````\n");
            }
            let shown: std::collections::HashSet<u32> = doc.pages.iter().map(|p| p.index).collect();
            let orphans: Vec<String> = rec
                .facts
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.evidence
                        .as_ref()
                        .and_then(|e| e.page)
                        .is_some_and(|p| !shown.contains(&p))
                })
                .map(|(n, _)| format!("#{}", n + 1))
                .collect();
            if !orphans.is_empty() {
                let _ = writeln!(
                    s,
                    "Facts pointing at pages not in the document: {}",
                    orphans.join(", ")
                );
            }
        }
    }
    Ok(s)
}

/// The page text with fact evidence spans and zone changes marked. Returns the marked text
/// and the (1-based) numbers of facts on this page whose span does not fit its text.
pub fn highlight(page: &Page, facts: &[Fact]) -> (String, Vec<usize>) {
    let text = &page.text;
    // (position, rank, marker). Rank orders markers at one position: closes, zone, opens.
    let mut events: Vec<(usize, u8, Reverse<usize>, String)> = Vec::new();
    let mut bad = Vec::new();
    for (n, f) in facts.iter().enumerate() {
        let Some(ev) = &f.evidence else { continue };
        if ev.page != Some(page.index) {
            continue;
        }
        let Some(sp) = ev.span else { continue };
        let (a, b) = (sp.start as usize, sp.end as usize);
        if a > b || b > text.len() || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            bad.push(n + 1);
            continue;
        }
        let id = n + 1;
        // Opens: longer spans first. Closes: the later-opened first.
        events.push((a, 2, Reverse(b), format!("⟦{id}|")));
        events.push((b, 0, Reverse(a), format!("|{id}⟧")));
    }
    let mut last_zone = None;
    for blk in &page.blocks {
        if last_zone != Some(blk.zone) {
            let a = blk.span.start as usize;
            if a <= text.len() && text.is_char_boundary(a) {
                events.push((a, 1, Reverse(0), format!("«{}»", zone_name(blk.zone))));
            }
            last_zone = Some(blk.zone);
        }
    }
    events.sort_by_key(|e| (e.0, e.1, e.2));
    let mut out = String::with_capacity(text.len() + events.len() * 6);
    let mut pos = 0;
    for (at, _, _, marker) in events {
        out.push_str(&text[pos..at]);
        out.push_str(&marker);
        pos = at;
    }
    out.push_str(&text[pos..]);
    (out, bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quail_core::{AnalyzerId, Block, BlockKind, Evidence, FactKey, Span, Value};

    fn fact(page: u32, a: u32, b: u32) -> Fact {
        Fact {
            key: FactKey::new("t.x"),
            value: Value::text("v"),
            conf: 0.5,
            src: AnalyzerId::new("t.x", 1),
            evidence: Some(Evidence {
                page: Some(page),
                zone: Zone::Body,
                span: Some(Span { start: a, end: b }),
                bbox: None,
            }),
        }
    }

    #[test]
    fn marks_nested_and_rejects_bad_spans() {
        let mut d = text_document("Agreement between Alpha Corp and Beta".into());
        d.pages[0].blocks.push(Block {
            span: Span { start: 0, end: 9 },
            kind: BlockKind::Heading { level: 1 },
            zone: Zone::Title,
            bbox: None,
        });
        d.pages[0].blocks.push(Block {
            span: Span { start: 10, end: 37 },
            kind: BlockKind::Paragraph,
            zone: Zone::Body,
            bbox: None,
        });
        let facts = vec![
            fact(0, 18, 28),
            fact(0, 18, 23),
            fact(0, 30, 99),
            fact(1, 0, 1),
        ];
        let (t, bad) = highlight(&d.pages[0], &facts);
        assert_eq!(
            t,
            "«title»Agreement «body»between ⟦1|⟦2|Alpha|2⟧ Corp|1⟧ and Beta"
        );
        assert_eq!(bad, vec![3]);
    }

    #[test]
    fn where_parses() {
        assert_eq!("low_conf".parse::<Where>(), Ok(Where::LowConf));
        assert!("x".parse::<Where>().is_err());
    }
}

//! Wiring: the presentation's slide list, each slide with its layout positions and notes,
//! and the package-level facts.

use std::collections::HashMap;

use quail_core::{
    Block, BlockKind, Document, Emitter, Error, Format, Limits, Page, PageKind, Result, Span,
    TextSource, Value, Zone,
};
use quail_ooxml::package::Package;
use quail_ooxml::rels::Rels;
use quail_ooxml::xml::{Ev, Flow, Stop, local, name, walk};
use quail_ooxml::{Notes, clean, parse_date, root_fields};

use crate::keys;
use crate::slide::{Bullet, Node, Parser, Ph, Shape};

/// Facts gathered so far, emitted together at the end.
pub(crate) type Facts = Vec<(quail_core::FactKey, Value)>;

/// Most slides listed in `presentation.xml`.
pub(crate) const MAX_SLIDES: usize = 20_000;
/// Most blocks per file.
pub(crate) const MAX_BLOCKS: usize = 250_000;
/// Largest text a page may hold (spans are `u32`).
const MAX_PAGE_TEXT: u64 = (u32::MAX / 2) as u64;
/// Layouts and masters kept parsed (positions of their placeholders).
const MAX_CACHED: usize = 1000;

fn count(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// The text budget and block cap shared by every slide.
struct Budget {
    left: u64,
    blocks: usize,
    truncated: bool,
    capped: bool,
}

impl Budget {
    /// Charges `text` plus its separator; cuts it at a char boundary when the budget ends.
    fn take(&mut self, text: &str) -> Option<String> {
        if self.blocks >= MAX_BLOCKS {
            self.capped = true;
            return None;
        }
        if self.left == 0 {
            self.truncated = true;
            return None;
        }
        let need = text.len() as u64 + 1;
        let mut t = text.to_owned();
        if need > self.left {
            let mut keep = usize::try_from(self.left - 1).unwrap_or(0);
            while keep > 0 && !t.is_char_boundary(keep) {
                keep -= 1;
            }
            t.truncate(keep);
            self.left = 0;
            self.truncated = true;
        } else {
            self.left -= need;
        }
        self.blocks += 1;
        (!t.is_empty()).then_some(t)
    }
}

#[derive(Default)]
struct PageText {
    text: String,
    blocks: Vec<Block>,
}

impl PageText {
    fn push(&mut self, budget: &mut Budget, text: &str, kind: BlockKind, zone: Zone) {
        let text = text.trim();
        if text.is_empty() || self.text.len() as u64 + text.len() as u64 + 1 > MAX_PAGE_TEXT {
            return;
        }
        let Some(text) = budget.take(text) else {
            return;
        };
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
        self.text.push_str(&text);
        let end = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
        self.blocks
            .push(Block::new(Span { start, end }, kind, zone, None));
    }
}

/// Placeholder positions of a layout or master: by `idx` and by type.
#[derive(Default)]
struct Positions {
    by_idx: HashMap<String, (i64, i64)>,
    by_type: HashMap<String, (i64, i64)>,
    /// The master of a layout.
    master: Option<String>,
}

/// Parses one slide-like part.
fn parse(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Notes,
    timed_out: &mut bool,
) -> Option<(Vec<Node>, crate::slide::Info)> {
    let reader = pkg.open_part(part)?;
    let mut p = Parser::new();
    let r = walk(reader, out, |ev| p.ev(ev));
    if let Err(stop) = r {
        *timed_out |= stop == Stop::Timeout;
        warn.stop(&stop, part);
    }
    let info = std::mem::take(&mut p.info);
    Some((p.finish(), info))
}

/// The title placeholders read first; footers last.
fn rank(ph: Option<&Ph>) -> u8 {
    match ph.map(|p| p.kind.as_str()) {
        Some("title" | "ctrTitle") => 0,
        Some("subTitle") => 1,
        Some("dt" | "ftr" | "sldNum" | "hdr") => 3,
        _ => 2,
    }
}

#[allow(clippy::struct_field_names)] // `hidden_slides` counts slides, like the key
struct Slides<'a, 'p> {
    pkg: &'a mut Package<'p>,
    out: &'a Emitter,
    warn: &'a mut Notes,
    budget: Budget,
    layouts: HashMap<String, Positions>,
    timed_out: bool,
    tables: usize,
    notes: usize,
    hidden_slides: usize,
    hidden_shapes: u64,
    capped_parts: usize,
    cut_parts: usize,
}

impl Slides<'_, '_> {
    fn positions(&mut self, part: &str) -> &Positions {
        if !self.layouts.contains_key(part) {
            let mut pos = Positions::default();
            if self.layouts.len() < MAX_CACHED
                && let Some((nodes, _)) =
                    parse(self.pkg, part, self.out, self.warn, &mut self.timed_out)
            {
                collect_positions(&nodes, &mut pos);
                pos.master =
                    Rels::read(self.pkg, part, self.out, self.warn).part_of_kind("slideMaster");
            }
            self.layouts.insert(part.to_owned(), pos);
        }
        &self.layouts[part]
    }

    /// Where a placeholder without its own position sits: the layout's placeholder with the
    /// same `idx`, else the same type, else the master's of that type.
    fn inherited(&mut self, layout: Option<&str>, ph: &Ph) -> Option<(i64, i64)> {
        let layout = layout?;
        let (found, master) = {
            let l = self.positions(layout);
            let found = ph
                .idx
                .as_ref()
                .and_then(|i| l.by_idx.get(i))
                .or_else(|| l.by_type.get(&ph.kind))
                .copied();
            (found, l.master.clone())
        };
        if found.is_some() {
            return found;
        }
        let master = master?;
        let kind = match ph.kind.as_str() {
            "ctrTitle" => "title",
            "obj" | "subTitle" => "body",
            k => k,
        };
        self.positions(&master).by_type.get(kind).copied()
    }

    /// One slide part to one page.
    fn slide(&mut self, part: &str, index: u32) -> Page {
        let mut page = PageText::default();
        let rels = Rels::read(self.pkg, part, self.out, self.warn);
        if let Some((mut nodes, info)) =
            parse(self.pkg, part, self.out, self.warn, &mut self.timed_out)
        {
            self.note_info(&info, part);
            if info.hidden_slide {
                self.hidden_slides += 1;
            }
            let layout = rels.part_of_kind("slideLayout");
            self.place(&mut nodes, layout.as_deref(), 0);
            let mut tables = 0u16;
            let mut footers = Vec::new();
            self.emit(&nodes, &mut page, &mut tables, &mut footers);
            for s in footers {
                for p in &s.paras {
                    page.push(
                        &mut self.budget,
                        &p.text,
                        BlockKind::Paragraph,
                        Zone::Footer,
                    );
                }
            }
            self.tables += usize::from(tables);
        }
        if let Some(notes) = rels.part_of_kind("notesSlide")
            && !self.timed_out
            && let Some((nodes, info)) =
                parse(self.pkg, &notes, self.out, self.warn, &mut self.timed_out)
        {
            self.note_info(&info, &notes);
            let before = page.blocks.len();
            let mut shapes = Vec::new();
            flatten(&nodes, &mut shapes);
            for s in shapes {
                // The notes text is the body placeholder; the slide image, number, header
                // and footer placeholders are not notes.
                if s.ph.as_ref().is_some_and(|p| p.kind == "body") {
                    for p in &s.paras {
                        page.push(&mut self.budget, &p.text, BlockKind::Note, Zone::Body);
                    }
                }
            }
            self.notes += usize::from(page.blocks.len() > before);
        }
        Page {
            text: page.text,
            blocks: page.blocks,
            text_source: TextSource::Native,
            ..Page::new(index, PageKind::Logical)
        }
    }

    fn note_info(&mut self, info: &crate::slide::Info, part: &str) {
        self.hidden_shapes += info.hidden_shapes;
        if info.capped {
            self.capped_parts += 1;
            self.warn.push(format!(
                "{part}: more than {} shapes, paragraphs and cells or groups nested deeper \
                 than {}: the rest flattened or dropped",
                crate::slide::MAX_NODES,
                crate::slide::MAX_GROUP_DEPTH
            ));
        }
        if info.cut {
            self.cut_parts += 1;
        }
    }

    /// Fills in inherited positions and sorts every group into reading order: title
    /// placeholders, then top to bottom and left to right, footers last. Shapes without a
    /// known position keep their file order after the placed ones.
    fn place(&mut self, nodes: &mut [Node], layout: Option<&str>, depth: usize) {
        for n in nodes.iter_mut() {
            match n {
                Node::Shape(s) => {
                    if s.pos.is_none()
                        && let Some(ph) = s.ph.clone()
                    {
                        s.pos = self.inherited(layout, &ph);
                    }
                }
                Node::Group { nodes, pos } => {
                    if depth < crate::slide::MAX_GROUP_DEPTH {
                        self.place(nodes, layout, depth + 1);
                    }
                    if pos.is_none() {
                        *pos = nodes.iter().filter_map(node_pos).min();
                    }
                }
            }
        }
        nodes.sort_by_key(|n| {
            let ph = match n {
                Node::Shape(s) => s.ph.as_ref(),
                Node::Group { .. } => None,
            };
            let p = node_pos(n);
            (rank(ph), p.is_none(), p.map(|(x, y)| (y, x)))
        });
    }

    fn emit<'n>(
        &mut self,
        nodes: &'n [Node],
        page: &mut PageText,
        tables: &mut u16,
        footers: &mut Vec<&'n Shape>,
    ) {
        for n in nodes {
            match n {
                Node::Group { nodes, .. } => self.emit(nodes, page, tables, footers),
                Node::Shape(s) => {
                    if rank(s.ph.as_ref()) == 3 {
                        footers.push(s);
                        continue;
                    }
                    self.shape(s, page, tables);
                }
            }
        }
    }

    fn shape(&mut self, s: &Shape, page: &mut PageText, tables: &mut u16) {
        if let Some(cells) = &s.cells {
            if !cells.is_empty() {
                let table = *tables;
                *tables = tables.saturating_add(1);
                let clamp = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
                for c in cells {
                    let kind = BlockKind::TableCell {
                        table,
                        row: clamp(c.row),
                        col: clamp(c.col),
                    };
                    page.push(&mut self.budget, &c.text, kind, Zone::Table);
                }
            }
            return;
        }
        let kind = s.ph.as_ref().map(|p| p.kind.as_str());
        if let Some("title" | "ctrTitle" | "subTitle") = kind {
            let level = if kind == Some("subTitle") { 2 } else { 1 };
            let text: Vec<&str> = s
                .paras
                .iter()
                .map(|p| p.text.trim())
                .filter(|t| !t.is_empty())
                .collect();
            let kind = BlockKind::Heading { level };
            page.push(&mut self.budget, &text.join("\n"), kind, Zone::Title);
            return;
        }
        // Body placeholders are bulleted by the master unless a paragraph says otherwise;
        // text boxes only where a paragraph asks for a bullet.
        let body = matches!(kind, Some("body" | "obj"));
        for p in &s.paras {
            let list = match p.bullet {
                Bullet::On => true,
                Bullet::Off => false,
                Bullet::Inherit => body,
            };
            let kind = if list {
                BlockKind::ListItem
            } else {
                BlockKind::Paragraph
            };
            page.push(&mut self.budget, &p.text, kind, Zone::Body);
        }
    }
}

fn node_pos(n: &Node) -> Option<(i64, i64)> {
    match n {
        Node::Shape(s) => s.pos,
        Node::Group { pos, .. } => *pos,
    }
}

fn flatten<'n>(nodes: &'n [Node], out: &mut Vec<&'n Shape>) {
    for n in nodes {
        match n {
            Node::Shape(s) => out.push(s),
            Node::Group { nodes, .. } => flatten(nodes, out),
        }
    }
}

fn collect_positions(nodes: &[Node], pos: &mut Positions) {
    let mut shapes = Vec::new();
    flatten(nodes, &mut shapes);
    for s in shapes {
        if let (Some(ph), Some(p)) = (&s.ph, s.pos) {
            if let Some(i) = &ph.idx {
                pos.by_idx.entry(i.clone()).or_insert(p);
            }
            pos.by_type.entry(ph.kind.clone()).or_insert(p);
        }
    }
}

/// The relationship ids of `p:sldIdLst/p:sldId`, in order.
fn slide_ids(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Notes,
) -> (Vec<String>, bool) {
    let mut ids = Vec::new();
    let mut capped = false;
    let mut in_list = false;
    if let Some(reader) = pkg.open_part(part) {
        let r = walk(reader, out, |ev| {
            match ev {
                Ev::Start(e) if name(e) == "sldIdLst" => in_list = true,
                Ev::End(n) if local(n) == "sldIdLst" => in_list = false,
                Ev::Start(e) | Ev::Empty(e) if in_list && name(e) == "sldId" => {
                    if ids.len() >= MAX_SLIDES {
                        capped = true;
                        return Flow::Stop;
                    }
                    // `r:id`, not the numeric `id`: the prefixed attribute named `id`.
                    let rid = e
                        .attributes()
                        .with_checks(false)
                        .flatten()
                        .find(|a| {
                            let k = a.key.as_ref();
                            k.contains(':') && local(k) == "id"
                        })
                        .map(|a| a.value.into_owned());
                    if let Some(r) = rid {
                        ids.push(r.trim().to_owned());
                    }
                }
                _ => {}
            }
            Flow::Go
        });
        if let Err(stop) = r {
            warn.stop(&stop, part);
        }
    }
    (ids, capped)
}

fn text_fact(facts: &mut Facts, key: quail_core::FactKey, raw: &str) {
    let v = clean(raw);
    if !v.is_empty() {
        facts.push((key, Value::text(v)));
    }
}

fn props(pkg: &mut Package<'_>, rels: &Rels, out: &Emitter, warn: &mut Notes, facts: &mut Facts) {
    let core = rels
        .part_of_kind("core-properties")
        .unwrap_or_else(|| "docProps/core.xml".to_owned());
    for (n, v) in root_fields(pkg, &core, out, warn) {
        let key = match n.as_str() {
            "title" => keys::TITLE,
            "subject" => keys::SUBJECT,
            "creator" => keys::CREATOR,
            "lastModifiedBy" => keys::LAST_MODIFIED_BY,
            "keywords" => keys::KEYWORDS,
            "created" | "modified" => {
                let key = if n == "created" {
                    keys::CREATED
                } else {
                    keys::MODIFIED
                };
                match parse_date(&v) {
                    Some(d) => facts.push((key, Value::Date(d))),
                    None if !v.trim().is_empty() => {
                        warn.push(format!("{core}: {n} is not a W3CDTF date: {:?}", clean(&v)));
                    }
                    None => {}
                }
                continue;
            }
            _ => continue,
        };
        text_fact(facts, key, &v);
    }
    let app = rels
        .part_of_kind("extended-properties")
        .unwrap_or_else(|| "docProps/app.xml".to_owned());
    for (n, v) in root_fields(pkg, &app, out, warn) {
        match n.as_str() {
            "Application" => text_fact(facts, keys::APPLICATION, &v),
            "Company" => text_fact(facts, keys::COMPANY, &v),
            _ => {}
        }
    }
}

/// Extracts the presentation, pushing facts and warnings as it goes. Only an unreadable
/// container is an error; everything else degrades to a partial document.
#[allow(clippy::too_many_lines)] // one linear recipe; splitting it hides the part order
pub(crate) fn run(
    bytes: &[u8],
    limits: &Limits,
    out: &Emitter,
    warn: &mut Notes,
    facts: &mut Facts,
) -> Result<Document> {
    let mut pkg = Package::open(bytes, Format::Pptx, limits, out)?;
    warn.append(&mut pkg.warnings);
    facts.push((keys::ZIP_ENTRIES, count(pkg.entry_count())));
    facts.push((keys::DUPLICATE_ENTRIES, count(pkg.duplicates)));
    let pkg_rels = Rels::read(&mut pkg, "", out, warn);
    let main = pkg_rels
        .part_of_kind("officeDocument")
        .filter(|p| pkg.has(p))
        .or_else(|| {
            pkg.has("ppt/presentation.xml")
                .then(|| "ppt/presentation.xml".to_owned())
        })
        .ok_or_else(|| Error::Malformed {
            format: Format::Pptx,
            msg: "no presentation part".to_owned(),
        })?;
    let under = |pkg: &Package<'_>, dir: &str| {
        pkg.names()
            .iter()
            .filter(|n| {
                n.len() > dir.len()
                    && n.get(..dir.len())
                        .is_some_and(|p| p.eq_ignore_ascii_case(dir))
            })
            .count()
    };
    let has_vba = pkg
        .names()
        .iter()
        .any(|n| n.to_ascii_lowercase().ends_with("vbaproject.bin"));
    facts.push((keys::HAS_MACROS, Value::Bool(has_vba)));
    facts.push((keys::IMAGES, count(under(&pkg, "ppt/media/"))));
    facts.push((
        keys::EMBEDDED_OBJECTS,
        count(under(&pkg, "ppt/embeddings/")),
    ));

    // Slides first: they are what the file is for, so they get the budgets before the
    // properties.
    let pres_rels = Rels::read(&mut pkg, &main, out, warn);
    let (ids, ids_capped) = slide_ids(&mut pkg, &main, out, warn);
    if ids_capped {
        warn.problem(format!(
            "more than {MAX_SLIDES} slides listed: the rest skipped"
        ));
    }
    let max_pages = usize::try_from(limits.max_pages).unwrap_or(usize::MAX);
    let mut s = Slides {
        pkg: &mut pkg,
        out,
        warn,
        budget: Budget {
            left: limits.max_text_bytes,
            blocks: 0,
            truncated: false,
            capped: false,
        },
        layouts: HashMap::new(),
        timed_out: false,
        tables: 0,
        notes: 0,
        hidden_slides: 0,
        hidden_shapes: 0,
        capped_parts: 0,
        cut_parts: 0,
    };
    let mut pages = Vec::new();
    let mut missing = 0usize;
    for id in &ids {
        if pages.len() >= max_pages {
            break;
        }
        if s.timed_out || out.check_deadline().is_err() {
            s.timed_out = true;
            break;
        }
        let Some(part) = pres_rels.part_by_id(id).filter(|p| s.pkg.has(p)) else {
            missing += 1;
            continue;
        };
        let index = u32::try_from(pages.len()).unwrap_or(u32::MAX);
        pages.push(s.slide(&part, index));
        if s.budget.capped || s.budget.truncated {
            break;
        }
    }
    let read = pages.len();
    let Slides {
        budget,
        timed_out,
        tables,
        notes,
        hidden_slides,
        hidden_shapes,
        cut_parts,
        ..
    } = s;
    if pages.is_empty() {
        pages.push(Page {
            text_source: TextSource::Native,
            ..Page::new(0, PageKind::Logical)
        });
    }
    facts.push((keys::SLIDES, count(ids.len())));
    facts.push((keys::HIDDEN_SLIDES, count(hidden_slides)));
    facts.push((keys::NOTES_SLIDES, count(notes)));
    facts.push((keys::TABLES, count(tables)));
    facts.push((
        keys::HIDDEN_SHAPES,
        Value::Int(i64::try_from(hidden_shapes).unwrap_or(i64::MAX)),
    ));
    if !timed_out {
        props(&mut pkg, &pkg_rels, out, warn, facts);
    }
    if missing > 0 {
        warn.push(format!(
            "{missing} slides listed but missing from the package"
        ));
    }
    if read + missing < ids.len() && !budget.truncated && !budget.capped && !timed_out {
        warn.problem(format!(
            "{} slides beyond max_pages ({}) skipped",
            ids.len() - read - missing,
            limits.max_pages
        ));
    }
    if cut_parts > 0 {
        warn.problem(format!(
            "{cut_parts} parts with a paragraph over 1 MiB: the rest of it dropped"
        ));
    }
    if budget.truncated {
        warn.problem(format!(
            "text limit of {} bytes reached, rest of the text dropped",
            limits.max_text_bytes
        ));
    }
    if budget.capped {
        warn.problem(format!("more than {MAX_BLOCKS} blocks: the rest dropped"));
    }
    if timed_out {
        warn.problem("deadline reached: document is partial");
    }
    Ok(Document { pages })
}

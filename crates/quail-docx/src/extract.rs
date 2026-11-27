//! Wiring: which parts are read in which order, and how sections become pages.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use quail_core::{
    Block, BlockKind, Document, Emitter, Error, Format, Limits, Page, PageKind, Result, Span,
    TextSource, Value, Zone,
};

use crate::body::{Container, Item, Mode, Parsed, SectRefs, Shared, parse_part};
use crate::keys;
use crate::numbering::{Counters, Numbering};
use crate::package::Package;
use crate::props::{self, Facts};
use crate::rels::Rels;
use crate::styles::Styles;
use crate::util::{Distinct, Notes, link_domain};
use crate::xml::Stop;

/// Largest text a page may hold (spans are `u32`).
const MAX_PAGE_TEXT: u64 = (u32::MAX / 2) as u64;

fn count(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// Extracts the document, pushing facts and warnings as it goes. Only an unreadable
/// container is an error; everything else degrades to a partial document.
#[allow(clippy::too_many_lines)] // one linear recipe; splitting it hides the part order
pub(crate) fn run(
    bytes: &[u8],
    limits: &Limits,
    out: &Emitter,
    warn: &mut Notes,
    facts: &mut Facts,
) -> Result<Document> {
    let mut pkg = Package::open(bytes, Format::Docx, limits, out)?;
    warn.append(&mut pkg.warnings);
    facts.push((keys::ZIP_ENTRIES, count(pkg.entry_count())));
    facts.push((keys::DUPLICATE_ENTRIES, count(pkg.duplicates)));

    let pkg_rels = Rels::read(&mut pkg, "", out, warn);
    let main = pkg_rels
        .part_of_kind("officeDocument")
        .filter(|p| pkg.has(p))
        .or_else(|| {
            pkg.has("word/document.xml")
                .then(|| "word/document.xml".to_owned())
        })
        .ok_or_else(|| Error::Malformed {
            format: Format::Docx,
            msg: "no main document part".to_owned(),
        })?;

    // Package-level facts.
    let part_or = |kind: &str, default: &str| {
        pkg_rels
            .part_of_kind(kind)
            .unwrap_or_else(|| default.to_owned())
    };
    let core = part_or("core-properties", "docProps/core.xml");
    let app = part_or("extended-properties", "docProps/app.xml");
    let custom = part_or("custom-properties", "docProps/custom.xml");
    props::core(&mut pkg, &core, out, warn, facts);
    props::app(&mut pkg, &app, out, warn, facts);
    if pkg.has(&custom) {
        props::custom(&mut pkg, &custom, out, warn, facts);
    }
    let content_type = props::main_content_type(&mut pkg, &main, out, warn);
    let has_vba = pkg
        .names()
        .iter()
        .any(|n| n.to_ascii_lowercase().ends_with("vbaproject.bin"));
    let macro_ct = content_type
        .as_deref()
        .is_some_and(|c| c.to_ascii_lowercase().contains("macroenabled"));
    facts.push((keys::HAS_MACROS, Value::Bool(has_vba || macro_ct)));
    if let Some(ct) = content_type {
        facts.push((keys::MAIN_CONTENT_TYPE, Value::text(ct)));
    }
    let under = |dir: &str| {
        pkg.names()
            .iter()
            .filter(|n| {
                n.len() > dir.len()
                    && n.get(..dir.len())
                        .is_some_and(|p| p.eq_ignore_ascii_case(dir))
            })
            .count()
    };
    facts.push((keys::IMAGES, count(under("word/media/"))));
    facts.push((keys::EMBEDDED_OBJECTS, count(under("word/embeddings/"))));

    // Parts the main document points at.
    let doc_rels = Rels::read(&mut pkg, &main, out, warn);
    let styles = doc_rels
        .part_of_kind("styles")
        .map(|p| Styles::read(&mut pkg, &p, out, warn))
        .unwrap_or_default();
    let numbering = doc_rels
        .part_of_kind("numbering")
        .map(|p| Numbering::read(&mut pkg, &p, &styles, out, warn))
        .unwrap_or_default();
    if let Some(p) = doc_rels.part_of_kind("settings") {
        props::settings(&mut pkg, &p, out, warn, facts);
    }

    let mut shared = Shared::new(
        limits.max_text_bytes,
        usize::try_from(limits.max_pages).unwrap_or(usize::MAX),
    );
    let mut domains = Distinct::default();
    let mut timed_out = false;
    let add_domains = |rels: &Rels, domains: &mut Distinct| {
        for r in rels.all() {
            if r.external
                && r.kind.eq_ignore_ascii_case("hyperlink")
                && let Some(d) = link_domain(&r.target)
            {
                domains.add(&d);
            }
        }
    };
    add_domains(&doc_rels, &mut domains);

    // The body first: it is what the file is for, so it gets the decompression and text
    // budgets before notes and comments (the assembler resolves their ids later).
    let mut counters = Counters::default();
    let (body, stop) = if timed_out || out.check_deadline().is_err() {
        timed_out = true;
        (Parsed::default(), None)
    } else {
        parse_part(
            &mut pkg,
            &main,
            Mode::Body,
            &styles,
            &numbering,
            &mut counters,
            &mut shared,
            out,
            warn,
        )
    };
    timed_out |= stop == Some(Stop::Timeout);

    // Notes and comments, which the body refers to by id.
    let notes = |pkg: &mut Package<'_>,
                 kind: &str,
                 shared: &mut Shared,
                 warn: &mut Notes,
                 timed_out: &mut bool,
                 domains: &mut Distinct|
     -> Option<Vec<Container>> {
        let part = doc_rels.part_of_kind(kind)?;
        if *timed_out || out.check_deadline().is_err() {
            *timed_out = true;
            return None;
        }
        let rels = Rels::read(pkg, &part, out, warn);
        add_domains(&rels, domains);
        let mut counters = Counters::default();
        let (parsed, stop) = parse_part(
            pkg,
            &part,
            Mode::Notes,
            &styles,
            &numbering,
            &mut counters,
            shared,
            out,
            warn,
        );
        *timed_out |= stop == Some(Stop::Timeout);
        Some(parsed.containers)
    };
    let footnotes = notes(
        &mut pkg,
        "footnotes",
        &mut shared,
        warn,
        &mut timed_out,
        &mut domains,
    );
    let endnotes = notes(
        &mut pkg,
        "endnotes",
        &mut shared,
        warn,
        &mut timed_out,
        &mut domains,
    );
    let comments = notes(
        &mut pkg,
        "comments",
        &mut shared,
        warn,
        &mut timed_out,
        &mut domains,
    );
    if let Some(f) = &footnotes {
        facts.push((keys::FOOTNOTES, count(f.len())));
    }
    if let Some(e) = &endnotes {
        facts.push((keys::ENDNOTES, count(e.len())));
    }
    if let Some(c) = &comments {
        facts.push((keys::COMMENTS, count(c.len())));
        let mut authors = Distinct::default();
        for a in c.iter().filter_map(|c| c.author.as_deref()) {
            authors.add(a);
        }
        if !authors.is_empty() {
            facts.push((keys::COMMENT_AUTHORS, text_list(authors)));
        }
    }
    let by_id = |v: Option<Vec<Container>>| -> HashMap<String, String> {
        v.unwrap_or_default()
            .into_iter()
            .map(|c| (c.id, c.text))
            .collect()
    };
    let footnotes = by_id(footnotes);
    let endnotes = by_id(endnotes);
    let comments = by_id(comments);

    // Sections to pages.
    let mut asm = Assembler {
        pkg: &mut pkg,
        doc_rels: &doc_rels,
        styles: &styles,
        numbering: &numbering,
        shared: &mut shared,
        out,
        warn,
        footnotes: &footnotes,
        endnotes: &endnotes,
        comments: &comments,
        max_pages: limits.max_pages,
        pages: Vec::new(),
        sections: 0,
        cache: HashMap::new(),
        seen_parts: [HashSet::new(), HashSet::new()],
        seen_texts: HashSet::new(),
        seen_notes: HashSet::new(),
        seen_comments: HashSet::new(),
        domains: &mut domains,
        timed_out,
    };
    let mut section: Vec<Item> = Vec::new();
    for item in body.items {
        match item {
            Item::SectionEnd(refs) => asm.section(std::mem::take(&mut section), &refs),
            other => section.push(other),
        }
    }
    if !section.is_empty() || asm.sections == 0 {
        asm.section(section, &SectRefs::default());
    }
    let skipped = asm.sections.saturating_sub(asm.pages.len());
    let pages = std::mem::take(&mut asm.pages);
    let sections = asm.sections;
    let headers = asm.seen_parts[0].len();
    let footers = asm.seen_parts[1].len();
    let timed_out = asm.timed_out;
    drop(asm);
    if skipped > 0 {
        warn.problem(format!(
            "{skipped} sections beyond max_pages ({}) skipped",
            limits.max_pages
        ));
    }

    let stats = shared.stats;
    for u in &stats.field_urls {
        if let Some(d) = link_domain(u) {
            domains.add(&d);
        }
    }
    facts.push((keys::SECTIONS, count(sections)));
    facts.push((keys::PARAGRAPHS, Value::Int(stats.paragraphs)));
    facts.push((keys::TABLES, Value::Int(stats.tables)));
    facts.push((keys::NESTED_TABLES, Value::Int(stats.nested_tables)));
    facts.push((keys::HEADERS, count(headers)));
    facts.push((keys::FOOTERS, count(footers)));
    facts.push((keys::TRACKED_INSERTIONS, Value::Int(stats.insertions)));
    facts.push((keys::TRACKED_DELETIONS, Value::Int(stats.deletions)));
    facts.push((
        keys::TRACKED_FORMAT_CHANGES,
        Value::Int(stats.format_changes),
    ));
    facts.push((keys::DELETED_TEXT_CHARS, Value::Int(stats.deleted_chars)));
    facts.push((keys::HIDDEN_TEXT_CHARS, Value::Int(stats.hidden_chars)));
    if !stats.revision_authors.is_empty() {
        facts.push((keys::REVISION_AUTHORS, text_list(stats.revision_authors)));
    }
    if !domains.is_empty() {
        facts.push((keys::HYPERLINK_DOMAINS, text_list(domains)));
    }
    if !timed_out {
        props::clm(&mut pkg, out, warn, facts);
    }
    if shared.truncated {
        warn.problem(format!(
            "text limit of {} bytes reached, rest of the text dropped",
            limits.max_text_bytes
        ));
    }
    if shared.sections_stopped {
        warn.problem(format!(
            "body reading stopped after max_pages ({}) sections",
            limits.max_pages
        ));
    }
    if shared.items_capped {
        warn.problem(format!(
            "more than {} blocks and references: the rest dropped",
            crate::body::MAX_ITEMS
        ));
    }
    if timed_out {
        warn.problem("deadline reached: document is partial");
    }
    Ok(Document { pages })
}

fn text_list(d: Distinct) -> Value {
    Value::List(d.into_vec().into_iter().map(Value::text).collect())
}

/// Builds pages section by section.
struct Assembler<'a, 'p> {
    pkg: &'a mut Package<'p>,
    doc_rels: &'a Rels,
    styles: &'a Styles,
    numbering: &'a Numbering,
    shared: &'a mut Shared,
    out: &'a Emitter,
    warn: &'a mut Notes,
    footnotes: &'a HashMap<String, String>,
    endnotes: &'a HashMap<String, String>,
    comments: &'a HashMap<String, String>,
    max_pages: u32,
    pages: Vec<Page>,
    sections: usize,
    /// Parsed header/footer parts: part name → blocks.
    cache: HashMap<String, Running>,
    /// Distinct header (0) and footer (1) parts referenced.
    seen_parts: [HashSet<String>; 2],
    /// Header/footer texts already emitted.
    seen_texts: HashSet<u64>,
    seen_notes: HashSet<(bool, String)>,
    seen_comments: HashSet<String>,
    domains: &'a mut Distinct,
    timed_out: bool,
}

/// A parsed header or footer part.
struct Running {
    blocks: Vec<(String, BlockKind)>,
    /// Already on a page (or its text is).
    emitted: bool,
}

#[derive(Default)]
struct PageText {
    text: String,
    blocks: Vec<Block>,
}

impl PageText {
    fn push(&mut self, text: &str, kind: BlockKind, zone: Zone) {
        if text.is_empty() || self.text.len() as u64 + text.len() as u64 + 1 > MAX_PAGE_TEXT {
            return;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
        self.text.push_str(text);
        let end = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
        self.blocks
            .push(Block::new(Span { start, end }, kind, zone, None));
    }
}

impl Assembler<'_, '_> {
    fn section(&mut self, items: Vec<Item>, refs: &SectRefs) {
        self.sections += 1;
        if self.pages.len() >= usize::try_from(self.max_pages).unwrap_or(usize::MAX) {
            return;
        }
        if !self.timed_out && self.out.check_deadline().is_err() {
            self.timed_out = true;
        }
        let mut page = PageText::default();
        self.running(&mut page, &refs.headers, Mode::Header);
        let mut notes: Vec<(bool, String)> = Vec::new();
        for item in items {
            match item {
                Item::Block { text, kind, zone } => page.push(&text, kind, zone),
                Item::Comment(id) => {
                    if let Some(t) = self.comments.get(&id)
                        && self.seen_comments.insert(id)
                    {
                        page.push(t, BlockKind::Annotation, Zone::Body);
                    }
                }
                Item::Deleted(t) => page.push(&t, BlockKind::Annotation, Zone::Body),
                Item::NoteRef { endnote, id } => notes.push((endnote, id)),
                Item::SectionEnd(_) => {}
            }
        }
        for (endnote, id) in notes {
            let map = if endnote {
                self.endnotes
            } else {
                self.footnotes
            };
            if let Some(t) = map.get(&id)
                && self.seen_notes.insert((endnote, id))
            {
                page.push(t, BlockKind::Note, Zone::Body);
            }
        }
        self.running(&mut page, &refs.footers, Mode::Footer);
        let index = u32::try_from(self.pages.len()).unwrap_or(u32::MAX);
        self.pages.push(Page {
            text: page.text,
            blocks: page.blocks,
            text_source: TextSource::Native,
            ..Page::new(index, PageKind::Logical)
        });
    }

    /// Adds the header or footer blocks of a section, once per distinct text.
    fn running(&mut self, page: &mut PageText, rids: &[String], mode: Mode) {
        let zone = if mode == Mode::Header {
            Zone::Header
        } else {
            Zone::Footer
        };
        let slot = usize::from(mode == Mode::Footer);
        for rid in rids {
            let Some(part) = self.doc_rels.part_by_id(rid) else {
                continue;
            };
            if !self.cache.contains_key(&part) {
                self.seen_parts[slot].insert(part.clone());
                let blocks = self.parse_running(&part, mode);
                // Identical text in another part counts as already emitted.
                let mut h = std::collections::hash_map::DefaultHasher::new();
                for (t, _) in &blocks {
                    t.hash(&mut h);
                }
                let fresh = !blocks.is_empty() && self.seen_texts.insert(h.finish());
                self.cache.insert(
                    part.clone(),
                    Running {
                        blocks,
                        emitted: !fresh,
                    },
                );
            }
            let Some(r) = self.cache.get_mut(&part) else {
                continue;
            };
            if r.emitted {
                continue;
            }
            r.emitted = true;
            for (t, kind) in &r.blocks {
                page.push(t, *kind, zone);
            }
        }
    }

    fn parse_running(&mut self, part: &str, mode: Mode) -> Vec<(String, BlockKind)> {
        if self.timed_out || self.out.check_deadline().is_err() {
            self.timed_out = true;
            return Vec::new();
        }
        let rels = Rels::read(self.pkg, part, self.out, self.warn);
        for r in rels.all() {
            if r.external
                && r.kind.eq_ignore_ascii_case("hyperlink")
                && let Some(d) = link_domain(&r.target)
            {
                self.domains.add(&d);
            }
        }
        let mut counters = Counters::default();
        let (parsed, stop) = parse_part(
            self.pkg,
            part,
            mode,
            self.styles,
            self.numbering,
            &mut counters,
            self.shared,
            self.out,
            self.warn,
        );
        self.timed_out |= stop == Some(Stop::Timeout);
        parsed
            .items
            .into_iter()
            .filter_map(|i| match i {
                Item::Block { text, kind, .. } => Some((text, kind)),
                Item::Deleted(t) => Some((t, BlockKind::Annotation)),
                _ => None,
            })
            .collect()
    }
}

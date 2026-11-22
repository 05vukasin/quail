//! The streaming parser of `WordprocessingML` text parts: the main document, headers,
//! footers, footnotes, endnotes and comments.
//!
//! It never builds a tree. Paragraph, table and field state live on small stacks; each
//! finished paragraph (or table cell) becomes one [`Item`] in reading order.
//!
//! Text follows Word's *accepted changes* view: tracked insertions are part of the text,
//! tracked deletions are not; the text of each run of adjacent deletions is kept as an
//! [`Item::Deleted`] after its paragraph. Field codes are skipped and field results kept.
//! Hidden runs (`w:vanish` in the run's own properties) are left out and counted. The legacy
//! copy of a drawing (`mc:Fallback`) is skipped so text boxes are not read twice.
//!
//! Memory is bounded by the text budget (every byte that reaches page text, separators and
//! numbering labels included, is charged to it) and by [`MAX_ITEMS`] for everything else.

use quail_core::{BlockKind, Emitter, Zone};

use crate::numbering::{Counters, Numbering};
use crate::package::Package;
use crate::styles::Styles;
use crate::util::{Distinct, Notes};
use crate::xml::{Ev, Flow, MAX_EVENT, Stop, attr, name, on, val, val_int, walk_with};

/// Deepest field nesting tracked; deeper `begin`s are counted but not stacked.
const MAX_FIELD_DEPTH: usize = 64;
/// Deepest paragraph nesting (text boxes inside text boxes).
const MAX_PARA_DEPTH: usize = 64;
/// Deepest table nesting tracked.
const MAX_TABLE_DEPTH: usize = 64;
/// Most hyperlink targets collected from field codes.
const MAX_URLS: usize = 10_000;
/// Most items (blocks, references, section ends, notes) kept per file.
pub(crate) const MAX_ITEMS: u64 = 250_000;
/// Most header (and footer) references kept per section: Word writes at most three.
const MAX_SECT_REFS: usize = 8;

/// Which kind of part is parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// `word/document.xml`.
    Body,
    /// A header part.
    Header,
    /// A footer part.
    Footer,
    /// Footnotes, endnotes or comments: each `w:footnote` / `w:endnote` / `w:comment` is one
    /// container whose paragraphs are joined.
    Notes,
}

/// A section's header and footer relationship ids.
#[derive(Clone, Debug, Default)]
pub(crate) struct SectRefs {
    /// `w:headerReference` ids, in order.
    pub headers: Vec<String>,
    /// `w:footerReference` ids, in order.
    pub footers: Vec<String>,
}

/// One unit of output in reading order.
#[derive(Debug)]
pub(crate) enum Item {
    /// A paragraph, heading, list item or table cell.
    Block {
        /// Its text (never empty).
        text: String,
        /// What it is.
        kind: BlockKind,
        /// Where it sits.
        zone: Zone,
    },
    /// A reference to comment `id`.
    Comment(String),
    /// Text of a tracked deletion.
    Deleted(String),
    /// A reference to a footnote or endnote.
    NoteRef {
        /// Endnote rather than footnote.
        endnote: bool,
        /// Its `w:id`.
        id: String,
    },
    /// End of a section.
    SectionEnd(SectRefs),
}

/// A footnote, endnote or comment.
#[derive(Debug)]
pub(crate) struct Container {
    /// `w:id`.
    pub id: String,
    /// `w:author` (comments).
    pub author: Option<String>,
    /// Its paragraphs joined by `\n`.
    pub text: String,
}

/// Counters over every parsed text part.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// `w:ins` + `w:moveTo`.
    pub insertions: i64,
    /// `w:del` + `w:moveFrom`.
    pub deletions: i64,
    /// `w:*PrChange`.
    pub format_changes: i64,
    /// Body paragraphs.
    pub paragraphs: i64,
    /// Body tables.
    pub tables: i64,
    /// Body tables inside a table cell.
    pub nested_tables: i64,
    /// Characters of tracked deletions.
    pub deleted_chars: i64,
    /// Characters of hidden (`w:vanish`) runs.
    pub hidden_chars: i64,
    /// Authors of tracked changes.
    pub revision_authors: Distinct,
    /// Targets of `HYPERLINK` fields.
    pub field_urls: Vec<String>,
}

/// State shared by all parts of one file.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Text bytes still allowed ([`quail_core::Limits::max_text_bytes`]).
    pub text_left: u64,
    /// The text limit was hit.
    pub truncated: bool,
    /// Items still allowed ([`MAX_ITEMS`]).
    pub items_left: u64,
    /// The item cap was hit.
    pub items_capped: bool,
    /// Sections after which the body stops ([`quail_core::Limits::max_pages`]).
    pub max_sections: usize,
    /// The body stopped at `max_sections`.
    pub sections_stopped: bool,
    /// Counters.
    pub stats: Stats,
}

impl Shared {
    /// New state for a file.
    pub fn new(text_bytes: u64, max_sections: usize) -> Self {
        Self {
            text_left: text_bytes,
            items_left: MAX_ITEMS,
            max_sections,
            ..Self::default()
        }
    }

    /// Takes `n` bytes from the text budget; `false` (and truncated) when they do not fit.
    fn charge(&mut self, n: usize) -> bool {
        let n = n as u64;
        if self.truncated || n > self.text_left {
            self.truncated = true;
            return false;
        }
        self.text_left -= n;
        true
    }

    /// Takes one item from the item budget.
    fn take_item(&mut self) -> bool {
        if self.items_left == 0 {
            self.items_capped = true;
            return false;
        }
        self.items_left -= 1;
        true
    }
}

/// What parsing one part gave.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    /// Items in reading order (not in [`Mode::Notes`]).
    pub items: Vec<Item>,
    /// Containers ([`Mode::Notes`] only).
    pub containers: Vec<Container>,
}

/// Parses one text part. Errors end the part early and are reported in `warn`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parse_part(
    pkg: &mut Package<'_>,
    part: &str,
    mode: Mode,
    styles: &Styles,
    numbering: &Numbering,
    counters: &mut Counters,
    shared: &mut Shared,
    out: &Emitter,
    warn: &mut Notes,
) -> (Parsed, Option<Stop>) {
    let Some(reader) = pkg.open_part(part) else {
        // An earlier part spent the budget: say which part got nothing, it may be the body.
        if pkg.exhausted() {
            warn.stop(&Stop::Budget, part);
            return (Parsed::default(), Some(Stop::Budget));
        }
        return (Parsed::default(), None);
    };
    // One text node may not be much larger than what the text budget still takes.
    let max_event = shared.text_left.min(MAX_EVENT) + (1 << 20);
    let mut p = Parser {
        mode,
        styles,
        numbering,
        counters,
        shared,
        parsed: Parsed::default(),
        paras: Vec::new(),
        para_overflow: 0,
        tables: Vec::new(),
        table_overflow: 0,
        cell: None,
        tables_on_page: 0,
        sections: 0,
        stop: false,
        fields: Vec::new(),
        hidden: 0,
        overflow_fields: 0,
        abandoned_fields: 0,
        target: Target::None,
        del_depth: 0,
        del_buf: String::new(),
        del_pending: false,
        ppr_depth: 0,
        run_rpr: false,
        run_hidden: false,
        sect: None,
        sect_in_para: false,
        container: None,
    };
    let r = walk_with(reader, out, max_event, |ev| p.event(ev));
    p.finish();
    if p.abandoned_fields > 0 {
        warn.push(format!(
            "{part}: {} fields without a result or end abandoned at their paragraph's end",
            p.abandoned_fields
        ));
    }
    if !p.fields.is_empty() {
        warn.push(format!(
            "{part}: {} fields still open at the end of the part",
            p.fields.len()
        ));
    }
    let stop = match r {
        Ok(info) => {
            if info.doctype {
                warn.push(format!("{part}: has a DOCTYPE (not expanded)"));
            }
            if info.unknown_entities > 0 {
                warn.push(format!(
                    "{part}: {} references to undeclared or DOCTYPE entities dropped",
                    info.unknown_entities
                ));
            }
            None
        }
        Err(s) => {
            warn.stop(&s, part);
            Some(s)
        }
    };
    (p.parsed, stop)
}

#[derive(Debug)]
enum Annot {
    Comment(String),
    Deleted(String),
}

#[derive(Debug, Default)]
struct Para {
    text: String,
    style: Option<String>,
    num_id: Option<String>,
    ilvl: Option<u8>,
    outline: Option<u8>,
    sect: Option<SectRefs>,
    annots: Vec<Annot>,
}

#[derive(Debug)]
struct Table {
    index: u16,
    row: Option<u16>,
    col: u16,
}

#[derive(Debug)]
struct Cell {
    table: u16,
    row: u16,
    col: u16,
    span: u16,
    text: String,
    annots: Vec<Annot>,
}

#[derive(Debug)]
struct Field {
    result: bool,
    instr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    None,
    /// `w:t`: main text (or deleted text inside `w:del` / `w:moveFrom`).
    Text,
    /// `w:delText`.
    Deleted,
    /// `w:instrText`.
    Instr,
}

#[allow(clippy::struct_excessive_bools)] // independent parser flags, not a mode
struct Parser<'s> {
    mode: Mode,
    styles: &'s Styles,
    numbering: &'s Numbering,
    counters: &'s mut Counters,
    shared: &'s mut Shared,
    parsed: Parsed,
    paras: Vec<Para>,
    /// `w:p` starts beyond [`MAX_PARA_DEPTH`] whose ends are still to come.
    para_overflow: usize,
    tables: Vec<Table>,
    /// `w:tbl` starts beyond [`MAX_TABLE_DEPTH`] whose ends are still to come.
    table_overflow: usize,
    /// The open cell of the outermost table; nested tables write into it.
    cell: Option<Cell>,
    tables_on_page: u16,
    sections: usize,
    /// Stop reading the part (section limit reached).
    stop: bool,
    fields: Vec<Field>,
    /// Open fields still in their instruction part.
    hidden: usize,
    /// `begin`s beyond [`MAX_FIELD_DEPTH`] not yet ended.
    overflow_fields: usize,
    /// Fields dropped because their paragraph ended before their result.
    abandoned_fields: usize,
    target: Target,
    del_depth: u32,
    del_buf: String,
    /// A deletion ended; the next adjacent one joins it.
    del_pending: bool,
    ppr_depth: u32,
    /// Inside a run's own `w:rPr`.
    run_rpr: bool,
    /// The current run is hidden (`w:vanish`).
    run_hidden: bool,
    sect: Option<SectRefs>,
    sect_in_para: bool,
    container: Option<Container>,
}

impl Parser<'_> {
    #[allow(clippy::too_many_lines)]
    fn event(&mut self, ev: Ev<'_>) -> Flow {
        if self.stop {
            return Flow::Stop;
        }
        match ev {
            Ev::Text(t) => {
                match self.target {
                    Target::None => {}
                    Target::Text if self.del_depth > 0 => self.push_deleted(t),
                    Target::Text => self.push_main(t),
                    Target::Deleted => self.push_deleted(t),
                    Target::Instr => {
                        if self.overflow_fields == 0
                            && let Some(f) = self.fields.last_mut()
                            && !f.result
                            && f.instr.len() < 4096
                        {
                            f.instr.push_str(t);
                        }
                    }
                }
                Flow::Go
            }
            Ev::Start(e) => {
                let n = name(e);
                match n {
                    "p" => {
                        if self.paras.len() < MAX_PARA_DEPTH {
                            self.paras.push(Para::default());
                        } else {
                            self.para_overflow += 1;
                        }
                    }
                    "r" => self.run_hidden = false,
                    "rPr" if self.ppr_depth == 0 => self.run_rpr = true,
                    "t" => self.target = Target::Text,
                    "delText" => self.target = Target::Deleted,
                    "instrText" => self.target = Target::Instr,
                    "pPr" => self.ppr_depth += 1,
                    "tbl" => self.start_table(),
                    "tr" => {
                        if let Some(t) = self.tables.last_mut() {
                            t.row = Some(t.row.map_or(0, |r| r.saturating_add(1)));
                            t.col = 0;
                        }
                    }
                    "tc" => self.start_cell(),
                    "sectPr" => self.start_sect(),
                    "del" | "moveFrom" => {
                        self.revision(e, Rev::Del);
                        self.del_depth += 1;
                    }
                    "ins" | "moveTo" => self.revision(e, Rev::Ins),
                    "fldSimple" => {
                        if let Some(i) = attr(e, "instr") {
                            self.instr(&i);
                        }
                    }
                    "fldChar" => self.fld_char(e),
                    "footnote" | "endnote" | "comment" if self.mode == Mode::Notes => {
                        return self.start_container(e);
                    }
                    // Legacy copy of a drawing, content-control and field metadata, binary
                    // payloads: no reading-order text in there.
                    "Fallback" | "sdtPr" | "fldData" | "binData" => return Flow::Skip,
                    _ if is_format_change(n) => {
                        self.revision(e, Rev::Format);
                        return Flow::Skip;
                    }
                    _ => self.property(n, e),
                }
                Flow::Go
            }
            Ev::Empty(e) => {
                let n = name(e);
                match n {
                    "tab" | "ptab" if self.ppr_depth == 0 => self.push_run("\t"),
                    "br" | "cr" => self.push_run("\n"),
                    "noBreakHyphen" => self.push_run("-"),
                    "vanish" if self.run_rpr && self.ppr_depth == 0 => {
                        self.run_hidden = on(e);
                    }
                    "sym" => {
                        if let Some(c) = attr(e, "char")
                            .and_then(|c| u32::from_str_radix(c.trim(), 16).ok())
                            .and_then(char::from_u32)
                            .filter(|c| !('\u{e000}'..='\u{f8ff}').contains(c))
                        {
                            let mut b = [0u8; 4];
                            self.push_run(c.encode_utf8(&mut b));
                        }
                    }
                    "del" | "moveFrom" => self.revision(e, Rev::Del),
                    "ins" | "moveTo" => self.revision(e, Rev::Ins),
                    "fldChar" => self.fld_char(e),
                    "fldSimple" => {
                        if let Some(i) = attr(e, "instr") {
                            self.instr(&i);
                        }
                    }
                    "sectPr" => {
                        self.start_sect();
                        self.end_sect();
                    }
                    "commentReference" if self.mode != Mode::Notes => {
                        if let Some(id) = attr(e, "id") {
                            self.flush_deleted();
                            self.annot(Annot::Comment(id.into_owned()));
                        }
                    }
                    "footnoteReference" | "endnoteReference" if self.mode != Mode::Notes => {
                        if let Some(id) = attr(e, "id") {
                            self.push_item(Item::NoteRef {
                                endnote: n == "endnoteReference",
                                id: id.into_owned(),
                            });
                        }
                    }
                    "gridSpan" => {
                        if self.tables.len() == 1
                            && let Some(c) = self.cell.as_mut()
                        {
                            c.span = val_int(e)
                                .and_then(|v| u16::try_from(v.clamp(1, 1000)).ok())
                                .unwrap_or(1);
                        }
                    }
                    "gridBefore" => {
                        if let Some(t) = self.tables.last_mut() {
                            let v = val_int(e).unwrap_or(0).clamp(0, 1000);
                            t.col = t.col.saturating_add(u16::try_from(v).unwrap_or(0));
                        }
                    }
                    _ if is_format_change(n) => self.revision(e, Rev::Format),
                    _ => self.property(n, e),
                }
                Flow::Go
            }
            Ev::End(n) => {
                match n {
                    "p" => {
                        if self.para_overflow > 0 {
                            self.para_overflow -= 1;
                        } else {
                            self.end_para();
                        }
                    }
                    "r" => self.run_hidden = false,
                    "rPr" => self.run_rpr = false,
                    "t" | "delText" | "instrText" => self.target = Target::None,
                    "pPr" => self.ppr_depth = self.ppr_depth.saturating_sub(1),
                    "tbl" => {
                        if self.table_overflow > 0 {
                            self.table_overflow -= 1;
                        } else {
                            self.end_table();
                        }
                    }
                    "tc" => self.end_cell(),
                    "sectPr" => self.end_sect(),
                    "del" | "moveFrom" => {
                        if self.del_depth > 0 {
                            self.del_depth -= 1;
                            if self.del_depth == 0 {
                                // Kept open: an adjacent deletion joins this one.
                                self.del_pending = true;
                            }
                        }
                    }
                    "footnote" | "endnote" | "comment" if self.mode == Mode::Notes => {
                        if let Some(c) = self.container.take() {
                            self.push_container(c);
                        }
                    }
                    _ => {}
                }
                if self.stop { Flow::Stop } else { Flow::Go }
            }
        }
    }

    /// Paragraph properties and section references.
    fn property(&mut self, n: &str, e: &quick_xml::events::BytesStart<'_>) {
        if self.ppr_depth > 0
            && self.sect.is_none()
            && let Some(p) = self.paras.last_mut()
        {
            match n {
                "pStyle" => p.style = val(e).map(std::borrow::Cow::into_owned),
                "numId" => p.num_id = val(e).map(|v| v.trim().to_owned()),
                "ilvl" => p.ilvl = val_int(e).and_then(|v| u8::try_from(v).ok()),
                "outlineLvl" => p.outline = val_int(e).and_then(|v| u8::try_from(v).ok()),
                _ => {}
            }
            return;
        }
        if let Some(s) = self.sect.as_mut() {
            let list = match n {
                "headerReference" => &mut s.headers,
                "footerReference" => &mut s.footers,
                _ => return,
            };
            if list.len() < MAX_SECT_REFS
                && let Some(id) = attr(e, "id")
            {
                list.push(id.into_owned());
            }
        }
    }

    fn start_container(&mut self, e: &quick_xml::events::BytesStart<'_>) -> Flow {
        let separator = attr(e, "type").is_some_and(|t| {
            matches!(
                t.as_ref(),
                "separator" | "continuationSeparator" | "continuationNotice"
            )
        });
        if separator || self.container.is_some() {
            return Flow::Skip;
        }
        self.container = Some(Container {
            id: attr(e, "id").unwrap_or_default().into_owned(),
            author: attr(e, "author").map(std::borrow::Cow::into_owned),
            text: String::new(),
        });
        Flow::Go
    }

    fn push_container(&mut self, c: Container) {
        // The note's text was charged as it was read; its separator in the page and its
        // slot are charged here.
        if !c.text.is_empty() && self.shared.take_item() && self.shared.charge(1) {
            self.parsed.containers.push(c);
        }
    }

    fn push_item(&mut self, item: Item) {
        if !self.shared.take_item() {
            // Nothing more can be kept: stop reading the part.
            self.stop = true;
            return;
        }
        if let Item::SectionEnd(_) = item {
            self.sections += 1;
            self.tables_on_page = 0;
            if self.mode == Mode::Body && self.sections >= self.shared.max_sections {
                self.shared.sections_stopped = true;
                self.stop = true;
            }
        }
        self.parsed.items.push(item);
    }

    fn revision(&mut self, e: &quick_xml::events::BytesStart<'_>, kind: Rev) {
        let stats = &mut self.shared.stats;
        match kind {
            Rev::Ins => stats.insertions += 1,
            Rev::Del => stats.deletions += 1,
            Rev::Format => stats.format_changes += 1,
        }
        if let Some(a) = attr(e, "author") {
            stats.revision_authors.add(&a);
        }
    }

    fn fld_char(&mut self, e: &quick_xml::events::BytesStart<'_>) {
        match attr(e, "fldCharType").as_deref() {
            Some("begin") => {
                if self.fields.len() < MAX_FIELD_DEPTH && self.overflow_fields == 0 {
                    self.fields.push(Field {
                        result: false,
                        instr: String::new(),
                    });
                    self.hidden += 1;
                } else {
                    self.overflow_fields += 1;
                }
            }
            Some("separate") => {
                if self.overflow_fields > 0 {
                    return;
                }
                if let Some(f) = self.fields.last_mut()
                    && !f.result
                {
                    f.result = true;
                    self.hidden -= 1;
                    let instr = std::mem::take(&mut f.instr);
                    self.instr(&instr);
                }
            }
            Some("end") => {
                if self.overflow_fields > 0 {
                    self.overflow_fields -= 1;
                    return;
                }
                if let Some(f) = self.fields.pop()
                    && !f.result
                {
                    self.hidden -= 1;
                    self.instr(&f.instr);
                }
            }
            _ => {}
        }
    }

    /// A field's instruction never spans paragraphs: fields still in their instruction part
    /// when a paragraph ends are malformed and dropped, so they cannot hide the rest of the
    /// part. Fields in their result part (a table of contents) may span paragraphs.
    fn abandon_open_instructions(&mut self) {
        if self.hidden == 0 && self.overflow_fields == 0 {
            return;
        }
        let before = self.fields.len();
        self.fields.retain(|f| f.result);
        self.abandoned_fields += before - self.fields.len() + self.overflow_fields;
        self.hidden = 0;
        self.overflow_fields = 0;
    }

    /// A field instruction: only `HYPERLINK` targets are kept (for link domains).
    fn instr(&mut self, instr: &str) {
        let s = instr.trim_start();
        let Some(rest) = s
            .get(..9)
            .filter(|k| k.eq_ignore_ascii_case("HYPERLINK"))
            .and_then(|_| s.get(9..))
        else {
            return;
        };
        let rest = rest.trim_start();
        let url = if let Some(q) = rest.strip_prefix('"') {
            q.split('"').next().unwrap_or("")
        } else {
            rest.split_whitespace().next().unwrap_or("")
        };
        // `HYPERLINK \l "bookmark"` is an internal link.
        if !url.is_empty() && !url.starts_with('\\') {
            let urls = &mut self.shared.stats.field_urls;
            if urls.len() < MAX_URLS {
                urls.push(url.to_owned());
            }
        }
    }

    /// Run content that is not `w:t` (tabs, breaks, symbols).
    fn push_run(&mut self, s: &str) {
        if self.del_depth > 0 {
            self.push_deleted(s);
        } else {
            self.push_main(s);
        }
    }

    fn push_main(&mut self, s: &str) {
        if self.hidden > 0 || self.overflow_fields > 0 {
            return;
        }
        if self.run_hidden {
            self.shared.stats.hidden_chars += char_count(s);
            return;
        }
        if self.del_pending && !s.trim().is_empty() {
            self.flush_deleted();
        }
        let Some(p) = self.paras.last_mut() else {
            return;
        };
        push_limited(&mut p.text, s, self.shared);
    }

    fn push_deleted(&mut self, s: &str) {
        if self.hidden > 0 || self.overflow_fields > 0 {
            return;
        }
        self.shared.stats.deleted_chars += char_count(s);
        if self.mode == Mode::Notes {
            return;
        }
        push_limited(&mut self.del_buf, s, self.shared);
    }

    fn flush_deleted(&mut self) {
        self.del_pending = false;
        if self.del_depth > 0 {
            return;
        }
        let t = self.del_buf.trim();
        if !t.is_empty() {
            let t = t.to_owned();
            self.annot(Annot::Deleted(t));
        }
        self.del_buf.clear();
    }

    fn annot(&mut self, a: Annot) {
        if self.mode == Mode::Notes {
            return;
        }
        if let Some(p) = self.paras.last_mut() {
            if p.annots.len() < 10_000 {
                p.annots.push(a);
            }
        } else if let Some(c) = self.cell.as_mut() {
            if c.annots.len() < 10_000 {
                c.annots.push(a);
            }
        } else {
            self.push_annot(a);
        }
    }

    fn push_annot(&mut self, a: Annot) {
        match a {
            Annot::Comment(id) => self.push_item(Item::Comment(id)),
            Annot::Deleted(t) => {
                // Its separator in the page; its text was charged as it was read.
                if self.shared.charge(1) {
                    self.push_item(Item::Deleted(t));
                }
            }
        }
    }

    fn end_para(&mut self) {
        self.flush_deleted();
        self.abandon_open_instructions();
        let Some(p) = self.paras.pop() else {
            return;
        };
        if self.mode == Mode::Body {
            self.shared.stats.paragraphs += 1;
        }
        let text = p.text.trim();
        let num = if self.numbering.is_empty() {
            None
        } else {
            self.numbering.numbering_of(
                self.styles,
                p.style.as_deref(),
                p.num_id.as_deref(),
                p.ilvl,
            )
        };
        let label = num
            .as_ref()
            .map(|r| self.counters.label(self.numbering, r))
            .unwrap_or_default();
        let heading = p.outline.filter(|&o| o < 9).map(|o| o + 1).or_else(|| {
            p.style
                .as_deref()
                .and_then(|s| self.styles.heading_level(s))
        });
        let kind = match (heading, &num) {
            (Some(level), _) => BlockKind::Heading { level },
            (None, Some(_)) => BlockKind::ListItem,
            (None, None) => BlockKind::Paragraph,
        };
        // The label and its space go into page text: charged like any text.
        let text = if label.is_empty() || text.is_empty() || !self.shared.charge(label.len() + 1) {
            text.to_owned()
        } else {
            format!("{label} {text}")
        };
        self.sink(text, kind, p.annots);
        if let Some(s) = p.sect
            && self.mode == Mode::Body
        {
            self.push_item(Item::SectionEnd(s));
        }
    }

    /// Where a finished paragraph goes: the open table cell, the open note, or the output.
    fn sink(&mut self, text: String, kind: BlockKind, annots: Vec<Annot>) {
        if let Some(c) = self.cell.as_mut() {
            if !text.is_empty() && (c.text.is_empty() || self.shared.charge(1)) {
                if !c.text.is_empty() {
                    c.text.push('\n');
                }
                c.text.push_str(&text);
            }
            c.annots.extend(annots);
            return;
        }
        if let Some(c) = self.container.as_mut() {
            if !text.is_empty() && (c.text.is_empty() || self.shared.charge(1)) {
                if !c.text.is_empty() {
                    c.text.push('\n');
                }
                c.text.push_str(&text);
            }
            return;
        }
        self.push_block(text, kind);
        for a in annots {
            self.push_annot(a);
        }
    }

    /// A finished block. Its separator in the page is charged here; when that one byte no
    /// longer fits the block is still kept (its text was within budget), and every later
    /// text is empty: the page text overshoots the budget by at most one byte.
    fn push_block(&mut self, text: String, kind: BlockKind) {
        if !text.is_empty() {
            let _ = self.shared.charge(1);
            let zone = self.zone(kind);
            self.push_item(Item::Block { text, kind, zone });
        }
    }

    fn zone(&self, kind: BlockKind) -> Zone {
        match self.mode {
            Mode::Header => Zone::Header,
            Mode::Footer => Zone::Footer,
            Mode::Body | Mode::Notes => match kind {
                BlockKind::Heading { .. } => Zone::Title,
                BlockKind::TableCell { .. } => Zone::Table,
                _ => Zone::Body,
            },
        }
    }

    fn start_table(&mut self) {
        if self.mode == Mode::Body {
            self.shared.stats.tables += 1;
            if !self.tables.is_empty() {
                self.shared.stats.nested_tables += 1;
            }
        }
        if self.tables.len() >= MAX_TABLE_DEPTH {
            self.table_overflow += 1;
            return;
        }
        let index = if self.tables.is_empty() {
            let i = self.tables_on_page;
            self.tables_on_page = self.tables_on_page.saturating_add(1);
            i
        } else {
            0
        };
        self.tables.push(Table {
            index,
            row: None,
            col: 0,
        });
    }

    fn end_table(&mut self) {
        if self.tables.len() == 1 && self.cell.is_some() {
            // Malformed: a cell left open at the end of its table.
            self.end_cell();
        }
        self.tables.pop();
    }

    fn start_cell(&mut self) {
        if self.tables.len() != 1 {
            return;
        }
        if self.cell.is_some() {
            self.end_cell();
        }
        if let Some(t) = self.tables.last() {
            self.cell = Some(Cell {
                table: t.index,
                row: t.row.unwrap_or(0),
                col: t.col,
                span: 1,
                text: String::new(),
                annots: Vec::new(),
            });
        }
    }

    fn end_cell(&mut self) {
        if self.tables.len() > 1 || self.table_overflow > 0 {
            if let Some(t) = self.tables.last_mut() {
                t.col = t.col.saturating_add(1);
            }
            return;
        }
        let Some(c) = self.cell.take() else {
            return;
        };
        if let Some(t) = self.tables.last_mut() {
            t.col = t.col.saturating_add(c.span);
        }
        let kind = BlockKind::TableCell {
            table: c.table,
            row: c.row,
            col: c.col,
        };
        if let Some(ct) = self.container.as_mut() {
            if !c.text.is_empty() && (ct.text.is_empty() || self.shared.charge(1)) {
                if !ct.text.is_empty() {
                    ct.text.push('\n');
                }
                ct.text.push_str(&c.text);
            }
            return;
        }
        self.push_block(c.text, kind);
        for a in c.annots {
            self.push_annot(a);
        }
    }

    fn start_sect(&mut self) {
        if self.sect.is_none() {
            self.sect = Some(SectRefs::default());
            self.sect_in_para = self.ppr_depth > 0;
        }
    }

    fn end_sect(&mut self) {
        let Some(s) = self.sect.take() else {
            return;
        };
        if self.mode != Mode::Body {
            return;
        }
        if self.sect_in_para
            && let Some(p) = self.paras.last_mut()
        {
            p.sect = Some(s);
        } else {
            self.push_item(Item::SectionEnd(s));
        }
    }

    /// Flushes whatever a truncated part left open.
    fn finish(&mut self) {
        self.stop = false;
        self.flush_deleted();
        while !self.paras.is_empty() {
            self.end_para();
        }
        self.table_overflow = 0;
        while !self.tables.is_empty() {
            self.end_table();
        }
        if let Some(c) = self.container.take() {
            self.push_container(c);
        }
    }
}

#[derive(Clone, Copy)]
enum Rev {
    Ins,
    Del,
    Format,
}

fn char_count(s: &str) -> i64 {
    i64::try_from(s.chars().count()).unwrap_or(i64::MAX)
}

/// Elements that record a tracked formatting change (their content is the old formatting).
fn is_format_change(n: &str) -> bool {
    matches!(
        n,
        "rPrChange"
            | "pPrChange"
            | "sectPrChange"
            | "tblPrChange"
            | "tblPrExChange"
            | "trPrChange"
            | "tcPrChange"
            | "tblGridChange"
            | "numberingChange"
    )
}

/// Appends `s` to `buf` within the text budget, dropping control characters other than tab
/// and newline (and the form feed that separates pages in `full_text`). Once the budget is
/// spent nothing more is appended anywhere.
fn push_limited(buf: &mut String, s: &str, shared: &mut Shared) {
    if shared.truncated {
        return;
    }
    let len = s.len() as u64;
    if len <= shared.text_left
        && !s
            .bytes()
            .any(|b| (b < 0x20 && b != b'\t' && b != b'\n') || b == 0x7f || b == 0xc2)
    {
        shared.text_left -= len;
        buf.push_str(s);
        return;
    }
    for c in s.chars() {
        if c.is_control() && c != '\t' && c != '\n' {
            continue;
        }
        let len = c.len_utf8() as u64;
        if len > shared.text_left {
            shared.truncated = true;
            break;
        }
        shared.text_left -= len;
        buf.push(c);
    }
}

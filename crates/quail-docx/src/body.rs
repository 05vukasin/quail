//! The streaming parser of `WordprocessingML` text parts: the main document, headers,
//! footers, footnotes, endnotes and comments.
//!
//! It never builds a tree. Paragraph, table and field state live on small stacks; each
//! finished paragraph (or table cell) becomes one [`Item`] in reading order.
//!
//! Text follows Word's *accepted changes* view: tracked insertions are part of the text,
//! tracked deletions are not; each deletion's text is kept as an [`Item::Deleted`] after its
//! paragraph. Field codes are skipped and field results kept. The legacy copy of a drawing
//! (`mc:Fallback`) is skipped so text boxes are not read twice.

use quail_core::{BlockKind, Emitter, Zone};

use crate::numbering::{Counters, Numbering};
use crate::package::Package;
use crate::styles::Styles;
use crate::util::Distinct;
use crate::xml::{Ev, Flow, Stop, attr, name, val, val_int, walk};

/// Deepest field nesting tracked; deeper `begin`s are counted but not stacked.
const MAX_FIELD_DEPTH: usize = 64;
/// Deepest paragraph nesting (text boxes inside text boxes).
const MAX_PARA_DEPTH: usize = 64;
/// Deepest table nesting tracked.
const MAX_TABLE_DEPTH: usize = 64;
/// Most hyperlink targets collected from field codes.
const MAX_URLS: usize = 10_000;

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
    /// Counters.
    pub stats: Stats,
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
    warn: &mut Vec<String>,
) -> (Parsed, Option<Stop>) {
    let Some(reader) = pkg.open_part(part) else {
        return (Parsed::default(), None);
    };
    let mut p = Parser {
        mode,
        styles,
        numbering,
        counters,
        shared,
        parsed: Parsed::default(),
        paras: Vec::new(),
        tables: Vec::new(),
        cell: None,
        tables_on_page: 0,
        fields: Vec::new(),
        hidden: 0,
        overflow_fields: 0,
        target: Target::None,
        del_depth: 0,
        del_buf: String::new(),
        ppr_depth: 0,
        sect: None,
        sect_in_para: false,
        container: None,
    };
    let r = walk(reader, out, |ev| p.event(ev));
    p.finish();
    let stop = r.err();
    if let Some(s) = &stop {
        warn.push(s.describe(part));
    }
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

struct Parser<'s> {
    mode: Mode,
    styles: &'s Styles,
    numbering: &'s Numbering,
    counters: &'s mut Counters,
    shared: &'s mut Shared,
    parsed: Parsed,
    paras: Vec<Para>,
    tables: Vec<Table>,
    /// The open cell of the outermost table; nested tables write into it.
    cell: Option<Cell>,
    tables_on_page: u16,
    fields: Vec<Field>,
    /// Open fields still in their instruction part.
    hidden: usize,
    /// `begin`s beyond [`MAX_FIELD_DEPTH`] not yet ended.
    overflow_fields: usize,
    target: Target,
    del_depth: u32,
    del_buf: String,
    ppr_depth: u32,
    sect: Option<SectRefs>,
    sect_in_para: bool,
    container: Option<Container>,
}

impl Parser<'_> {
    #[allow(clippy::too_many_lines)]
    fn event(&mut self, ev: Ev<'_>) -> Flow {
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
                        }
                    }
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
                            self.annot(Annot::Comment(id.into_owned()));
                        }
                    }
                    "footnoteReference" | "endnoteReference" if self.mode != Mode::Notes => {
                        if let Some(id) = attr(e, "id") {
                            self.parsed.items.push(Item::NoteRef {
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
                    "p" => self.end_para(),
                    "t" | "delText" | "instrText" => self.target = Target::None,
                    "pPr" => self.ppr_depth = self.ppr_depth.saturating_sub(1),
                    "tbl" => self.end_table(),
                    "tc" => self.end_cell(),
                    "sectPr" => self.end_sect(),
                    "del" | "moveFrom" => {
                        if self.del_depth > 0 {
                            self.del_depth -= 1;
                            if self.del_depth == 0 {
                                self.flush_deleted();
                            }
                        }
                    }
                    "footnote" | "endnote" | "comment" if self.mode == Mode::Notes => {
                        if let Some(c) = self.container.take()
                            && !c.text.is_empty()
                        {
                            self.parsed.containers.push(c);
                        }
                    }
                    _ => {}
                }
                Flow::Go
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
            match n {
                "headerReference" => {
                    if let Some(id) = attr(e, "id") {
                        s.headers.push(id.into_owned());
                    }
                }
                "footerReference" => {
                    if let Some(id) = attr(e, "id") {
                        s.footers.push(id.into_owned());
                    }
                }
                _ => {}
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
        let Some(p) = self.paras.last_mut() else {
            return;
        };
        push_limited(&mut p.text, s, self.shared);
    }

    fn push_deleted(&mut self, s: &str) {
        if self.hidden > 0 || self.overflow_fields > 0 || self.mode == Mode::Notes {
            return;
        }
        push_limited(&mut self.del_buf, s, self.shared);
    }

    fn flush_deleted(&mut self) {
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
            p.annots.push(a);
        } else if let Some(c) = self.cell.as_mut() {
            c.annots.push(a);
        } else {
            self.parsed.items.push(annot_item(a));
        }
    }

    fn end_para(&mut self) {
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
        let text = if label.is_empty() || text.is_empty() {
            text.to_owned()
        } else {
            format!("{label} {text}")
        };
        self.sink(text, kind, p.annots);
        if let Some(s) = p.sect
            && self.mode == Mode::Body
        {
            self.parsed.items.push(Item::SectionEnd(s));
            self.tables_on_page = 0;
        }
    }

    /// Where a finished paragraph goes: the open table cell, the open note, or the output.
    fn sink(&mut self, text: String, kind: BlockKind, annots: Vec<Annot>) {
        if let Some(c) = self.cell.as_mut() {
            if !text.is_empty() {
                if !c.text.is_empty() {
                    c.text.push('\n');
                }
                c.text.push_str(&text);
            }
            c.annots.extend(annots);
            return;
        }
        if let Some(c) = self.container.as_mut() {
            if !text.is_empty() {
                if !c.text.is_empty() {
                    c.text.push('\n');
                }
                c.text.push_str(&text);
            }
            return;
        }
        if !text.is_empty() {
            let zone = self.zone(kind);
            self.parsed.items.push(Item::Block { text, kind, zone });
        }
        self.parsed.items.extend(annots.into_iter().map(annot_item));
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
        if self.tables.len() > 1 {
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
            if !c.text.is_empty() {
                if !ct.text.is_empty() {
                    ct.text.push('\n');
                }
                ct.text.push_str(&c.text);
            }
            return;
        }
        if !c.text.is_empty() {
            let zone = self.zone(kind);
            self.parsed.items.push(Item::Block {
                text: c.text,
                kind,
                zone,
            });
        }
        self.parsed
            .items
            .extend(c.annots.into_iter().map(annot_item));
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
            self.parsed.items.push(Item::SectionEnd(s));
            self.tables_on_page = 0;
        }
    }

    /// Flushes whatever a truncated part left open.
    fn finish(&mut self) {
        while !self.paras.is_empty() {
            self.end_para();
        }
        while !self.tables.is_empty() {
            self.end_table();
        }
        if let Some(c) = self.container.take()
            && !c.text.is_empty()
        {
            self.parsed.containers.push(c);
        }
    }
}

#[derive(Clone, Copy)]
enum Rev {
    Ins,
    Del,
    Format,
}

fn annot_item(a: Annot) -> Item {
    match a {
        Annot::Comment(id) => Item::Comment(id),
        Annot::Deleted(t) => Item::Deleted(t),
    }
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
/// and newline (and the form feed that separates pages in `full_text`).
fn push_limited(buf: &mut String, s: &str, shared: &mut Shared) {
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

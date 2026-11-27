//! Tokens to pages: an element stack of our own (bounded), block flushing, tables, hidden
//! content, page breaks and the inline XBRL bits.

use std::cell::RefCell;

use html5ever::LocalName;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use quail_core::{Block, BlockKind, Emitter, Limits, Page, PageKind, Span, TextSource, Zone};

/// Bytes of input decoded and tokenized per step (the deadline is checked between steps).
const CHUNK: usize = 64 << 10;
/// Most open elements tracked; deeper ones are read as plain inline content.
pub(crate) const MAX_DEPTH: usize = 512;
/// Most tables open at once; a deeper table is read as plain blocks.
pub(crate) const MAX_TABLE_DEPTH: usize = 32;
/// Columns tracked per table row (row and column spans included).
const MAX_COLS: u32 = 1024;
/// Largest `colspan` honoured (the HTML limit).
const MAX_COLSPAN: u32 = 1000;
/// Largest `rowspan` honoured (the HTML limit).
const MAX_ROWSPAN: u32 = 65_534;
/// Most blocks per file.
pub(crate) const MAX_BLOCKS: usize = 250_000;
/// Largest input read without a single token being completed (one comment, tag or
/// attribute value that never ends).
pub(crate) const MAX_TOKEN: u64 = 16 << 20;
/// Largest text a page may hold (spans are `u32`).
const MAX_PAGE_TEXT: u64 = (u32::MAX / 2) as u64;
/// Longest title, `dei:` value or generator kept, in bytes.
const MAX_SMALL: usize = 4096;
const MAX_DEI_VALUE: usize = 1024;
const MAX_DEI: usize = 200;
/// The inline XBRL namespaces (1.0 and 1.1).
const IX_NS: [&str; 2] = [
    "http://www.xbrl.org/2008/inlineXBRL",
    "http://www.xbrl.org/2013/inlineXBRL",
];

/// Elements that never have content.
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "image", "img",
    "input", "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Elements whose content is never page text.
const DROPPED: &[&str] = &[
    "applet", "audio", "button", "canvas", "datalist", "frameset", "iframe", "map", "math", "nav",
    "noembed", "noframes", "noscript", "object", "script", "select", "style", "svg", "template",
    "textarea", "video", "xmp",
];

/// Elements that start and end a block.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "legend",
    "li",
    "listing",
    "main",
    "menu",
    "ol",
    "p",
    "plaintext",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

/// Start tags that close an open `<p>` (in button scope).
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dd",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "plaintext",
    "pre",
    "section",
    "summary",
    "table",
    "ul",
    "xmp",
];

/// Elements a search for an open `<p>` does not cross.
const P_SCOPE: &[&str] = &[
    "applet", "button", "caption", "html", "marquee", "object", "table", "td", "template", "th",
];

fn is(name: &str, set: &[&str]) -> bool {
    set.contains(&name)
}

/// What an open element means for the text inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Inline,
    Block,
    Heading(u8),
    ListItem,
    Pre,
    Zone(Zone),
    Table,
    Row,
    Cell,
}

#[allow(clippy::struct_excessive_bools)] // independent flags of one element
struct Open {
    name: LocalName,
    role: Role,
    /// Its content is not page text (it started a hidden or dropped subtree).
    skip: bool,
    /// A page break follows its end.
    break_after: bool,
    /// It holds the `dei:` value being captured.
    capture: bool,
    /// It is the `<title>` being captured.
    title: bool,
}

/// A finished piece of content, kept while a table around it is still open.
enum Item {
    Block {
        text: String,
        kind: BlockKind,
        zone: Zone,
    },
    Table(TableB),
    PageBreak,
}

struct CellB {
    row: u32,
    col: u32,
    items: Vec<Item>,
}

#[derive(Default)]
struct TableB {
    cells: Vec<CellB>,
    cell: Option<CellB>,
    /// Current row (`None` before the first).
    row: Option<u32>,
    col: u32,
    /// Rows each column is covered by a `rowspan`, counted from the current row.
    spans: Vec<u32>,
    cols: u32,
}

impl TableB {
    fn next_row(&mut self) {
        self.row = Some(self.row.map_or(0, |r| r.saturating_add(1)));
        self.col = 0;
        for s in &mut self.spans {
            *s = s.saturating_sub(1);
        }
    }

    fn open_cell(&mut self, colspan: u32, rowspan: u32) {
        if self.row.is_none() {
            self.next_row();
        }
        while self.spans.get(self.col as usize).is_some_and(|&s| s > 0) {
            self.col += 1;
        }
        let col = self.col.min(MAX_COLS);
        let end = col.saturating_add(colspan).min(MAX_COLS);
        if self.spans.len() < end as usize {
            self.spans.resize(end as usize, 0);
        }
        for c in col..end {
            let s = &mut self.spans[c as usize];
            // This row included: the next row start takes one off.
            *s = (*s).max(rowspan);
        }
        self.col = end;
        self.cols = self.cols.max(end);
        self.cell = Some(CellB {
            row: self.row.unwrap_or(0),
            col,
            items: Vec::new(),
        });
    }

    fn close_cell(&mut self) {
        if let Some(c) = self.cell.take()
            && !c.items.is_empty()
        {
            self.cells.push(c);
        }
    }

    /// A layout table: one cell in all, or a cell that holds more than plain paragraphs.
    fn is_layout(&self) -> bool {
        let rows = self.row.map_or(0, |r| r + 1);
        (rows <= 1 && self.cols <= 1)
            || self.cells.iter().any(|c| {
                c.items.iter().any(|i| {
                    !matches!(
                        i,
                        Item::Block {
                            kind: BlockKind::Paragraph,
                            ..
                        }
                    )
                })
            })
    }
}

#[derive(Default)]
struct PageText {
    text: String,
    blocks: Vec<Block>,
    tables: u16,
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

/// What the walk counted, for facts and problems.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // independent counters and flags
pub(crate) struct Stats {
    pub title: Option<String>,
    pub lang: Option<String>,
    pub generator: Option<String>,
    pub inline_xbrl: bool,
    pub ix_facts: i64,
    pub dei: Vec<String>,
    pub tables: i64,
    pub layout_tables: i64,
    pub hidden: i64,
    pub scripts: i64,
    pub elements: i64,
    pub truncated: bool,
    pub pages_capped: bool,
    pub blocks_capped: bool,
    pub depth_capped: bool,
    pub tables_capped: bool,
}

struct State {
    open: Vec<Open>,
    /// Open elements with `skip`.
    skip: usize,
    /// Open `<pre>`-like elements (whitespace kept).
    pre: usize,
    tables: Vec<TableB>,
    /// Open `p`, `li` and `dt`/`dd` elements, so start tags only search the stack for
    /// an implied end when there is one to find.
    open_p: usize,
    open_li: usize,
    open_dt: usize,
    cur: String,
    space: bool,
    page: PageText,
    pages: Vec<Page>,
    max_pages: usize,
    /// Text bytes still allowed.
    budget: u64,
    blocks: usize,
    /// Nothing more is collected (a limit was reached).
    stopped: bool,
    /// XHTML (an XML declaration or the XHTML namespace): `<x/>` closes itself.
    xhtml: bool,
    ix_prefixes: Vec<String>,
    dei: Option<(String, String)>,
    title: Option<String>,
    tokens: u64,
    stats: Stats,
}

impl State {
    fn new(limits: &Limits) -> Self {
        Self {
            open: Vec::new(),
            skip: 0,
            pre: 0,
            tables: Vec::new(),
            open_p: 0,
            open_li: 0,
            open_dt: 0,
            cur: String::new(),
            space: false,
            page: PageText::default(),
            pages: Vec::new(),
            max_pages: usize::try_from(limits.max_pages.max(1)).unwrap_or(usize::MAX),
            budget: limits.max_text_bytes,
            blocks: 0,
            stopped: false,
            xhtml: false,
            ix_prefixes: vec!["ix".to_owned()],
            dei: None,
            title: None,
            tokens: 0,
            stats: Stats::default(),
        }
    }

    // ---- text ----

    fn text(&mut self, t: &str) {
        if let Some((_, v)) = self.dei.as_mut() {
            push_capped(v, t, MAX_DEI_VALUE);
        }
        if let Some(title) = self.title.as_mut() {
            push_capped(title, t, MAX_SMALL);
            return;
        }
        if self.skip > 0 || self.stopped {
            return;
        }
        for c in t.chars() {
            if self.cur.len() as u64 >= self.budget {
                break;
            }
            if self.pre > 0 && (c == '\n' || c == '\t') {
                self.cur.push(c);
                self.space = false;
            } else if c.is_whitespace() || c.is_control() {
                if self.pre > 0 && !c.is_control() {
                    self.cur.push(' ');
                } else {
                    self.space = !self.cur.is_empty() && !self.cur.ends_with('\n');
                }
            } else {
                if self.space {
                    self.cur.push(' ');
                    self.space = false;
                }
                self.cur.push(c);
            }
        }
    }

    fn line_break(&mut self) {
        if self.skip > 0 || self.stopped {
            return;
        }
        let t = self.cur.trim_end_matches(' ').len();
        self.cur.truncate(t);
        if !self.cur.is_empty() && !self.cur.ends_with('\n') {
            self.cur.push('\n');
        }
        self.space = false;
    }

    /// Ends the current block.
    fn flush(&mut self) {
        self.space = false;
        if self.cur.is_empty() {
            return;
        }
        let mut text = std::mem::take(&mut self.cur);
        let t = text.trim_end().len();
        text.truncate(t);
        let lead = text.len() - text.trim_start().len();
        if lead > 0 {
            text.drain(..lead);
        }
        if text.is_empty() || self.stopped {
            return;
        }
        if self.blocks >= MAX_BLOCKS {
            self.stats.blocks_capped = true;
            self.stopped = true;
            return;
        }
        let need = text.len() as u64 + 1;
        if need > self.budget {
            let mut keep = usize::try_from(self.budget.saturating_sub(1)).unwrap_or(0);
            while keep > 0 && !text.is_char_boundary(keep) {
                keep -= 1;
            }
            text.truncate(keep);
            self.stats.truncated = true;
            self.stopped = true;
            self.budget = 0;
            if text.is_empty() {
                return;
            }
        } else {
            self.budget -= need;
        }
        self.blocks += 1;
        let (kind, zone) = self.context();
        self.push_item(Item::Block { text, kind, zone });
    }

    /// Kind and zone of a block from the open elements, up to the nearest cell or table.
    fn context(&self) -> (BlockKind, Zone) {
        let mut kind = None;
        let mut zone = Zone::Body;
        for o in self.open.iter().rev() {
            match o.role {
                Role::Cell | Role::Table => break,
                Role::Heading(level) if kind.is_none() => {
                    kind = Some((BlockKind::Heading { level }, Zone::Title));
                }
                Role::ListItem if kind.is_none() => kind = Some((BlockKind::ListItem, Zone::Body)),
                Role::Zone(z) if zone == Zone::Body => zone = z,
                _ => {}
            }
        }
        match kind {
            Some((k @ BlockKind::Heading { .. }, z)) => (k, z),
            Some((k, _)) => (k, zone),
            None => (BlockKind::Paragraph, zone),
        }
    }

    /// Puts an item into the innermost open table cell, or on the page.
    fn push_item(&mut self, item: Item) {
        for t in self.tables.iter_mut().rev() {
            if let Some(c) = t.cell.as_mut() {
                c.items.push(item);
                return;
            }
        }
        self.emit(item);
    }

    /// An item outside every table, onto the page.
    fn emit(&mut self, item: Item) {
        match item {
            Item::Block { text, kind, zone } => self.page.push(&text, kind, zone),
            Item::PageBreak => self.page_break(),
            Item::Table(t) => self.render_table(t),
        }
    }

    fn render_table(&mut self, t: TableB) {
        if t.is_layout() {
            self.stats.layout_tables += 1;
            for c in t.cells {
                for i in c.items {
                    self.emit(i);
                }
            }
            return;
        }
        self.stats.tables += 1;
        let table = self.page.tables;
        self.page.tables = self.page.tables.saturating_add(1);
        let clamp = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
        for c in t.cells {
            let mut text = String::new();
            for i in &c.items {
                if let Item::Block { text: s, .. } = i {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(s);
                }
            }
            let kind = BlockKind::TableCell {
                table,
                row: clamp(c.row),
                col: clamp(c.col),
            };
            self.page.push(&text, kind, Zone::Table);
        }
    }

    fn page_break(&mut self) {
        if self.page.text.is_empty() {
            return;
        }
        if self.pages.len() + 1 >= self.max_pages {
            self.stats.pages_capped = true;
            self.stopped = true;
            return;
        }
        self.finish_page();
    }

    fn finish_page(&mut self) {
        let page = std::mem::take(&mut self.page);
        let index = u32::try_from(self.pages.len()).unwrap_or(u32::MAX);
        self.pages.push(Page {
            text: page.text,
            blocks: page.blocks,
            text_source: TextSource::Native,
            ..Page::new(index, PageKind::Logical)
        });
    }

    /// A page break at this point: on the page, or kept in the open table cell (a layout
    /// table passes it on).
    fn break_here(&mut self) {
        self.flush();
        if self.skip == 0 {
            self.push_item(Item::PageBreak);
        }
    }

    // ---- elements ----

    #[allow(clippy::too_many_lines)] // one element's whole effect, in order
    fn start(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        self.stats.elements += 1;
        let name: &str = &tag.name;
        self.namespaces(tag);
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        let ix = !prefix.is_empty() && self.ix_prefixes.iter().any(|p| p == prefix);
        if ix && matches!(local, "nonfraction" | "nonnumeric") {
            self.stats.ix_facts += 1;
            self.stats.inline_xbrl = true;
        } else if ix && local == "header" {
            self.stats.inline_xbrl = true;
        }
        match name {
            "html" => {
                if self.stats.lang.is_none() {
                    self.stats.lang = attr(tag, "lang")
                        .or_else(|| attr(tag, "xml:lang"))
                        .map(|s| clean(&s))
                        .filter(|l| !l.is_empty());
                }
                return TokenSinkResult::Continue;
            }
            "body" => {
                self.flush();
                return TokenSinkResult::Continue;
            }
            "meta" => {
                if self.stats.generator.is_none()
                    && attr(tag, "name").is_some_and(|n| n.eq_ignore_ascii_case("generator"))
                {
                    self.stats.generator = attr(tag, "content")
                        .map(|s| clean(&s))
                        .filter(|g| !g.is_empty());
                }
                return TokenSinkResult::Continue;
            }
            "br" => {
                self.line_break();
                return TokenSinkResult::Continue;
            }
            _ => {}
        }

        self.implied_closes(name);
        let style = attr(tag, "style").map(|s| compact_style(&s));
        let style = style.as_deref().unwrap_or("");
        let hidden = attr(tag, "hidden").is_some()
            || style.contains("display:none")
            || style.contains("visibility:hidden")
            || (ix && local == "header");
        let block = is(name, BLOCKS);
        if block {
            self.flush();
        }
        if style.contains("page-break-before:always") || style.contains("break-before:page") {
            self.break_here();
        }
        let break_after =
            style.contains("page-break-after:always") || style.contains("break-after:page");

        let void = is(name, VOID);
        let closes = void || (tag.self_closing && (self.xhtml || !prefix.is_empty()));
        let raw = if closes { None } else { raw_kind(name) };
        if name == "script" {
            self.stats.scripts += 1;
        }
        if closes {
            if block {
                self.flush();
            }
            if break_after {
                self.break_here();
            }
            return TokenSinkResult::Continue;
        }
        if self.open.len() >= MAX_DEPTH {
            self.stats.depth_capped = true;
            return raw.unwrap_or(TokenSinkResult::Continue);
        }

        let dropped = is(name, DROPPED);
        let skip = hidden || dropped;
        if hidden && self.skip == 0 {
            self.stats.hidden += 1;
        }
        let title = name == "title" && self.title.is_none() && self.stats.title.is_none();
        let role = if skip || self.skip > 0 || title {
            Role::Inline
        } else {
            self.role(name, tag)
        };
        let capture = ix
            && self.dei.is_none()
            && matches!(local, "nonfraction" | "nonnumeric")
            && attr(tag, "name").is_some_and(|n| n.starts_with("dei:"));
        if capture {
            let n = attr(tag, "name").unwrap_or_default();
            self.dei = Some((n.trim_start_matches("dei:").to_owned(), String::new()));
        }
        if title {
            self.title = Some(String::new());
        }
        self.skip += usize::from(skip || title);
        self.count(name, 1);
        self.open.push(Open {
            name: tag.name.clone(),
            role,
            skip: skip || title,
            break_after,
            capture,
            title,
        });
        raw.unwrap_or(TokenSinkResult::Continue)
    }

    /// The role of an element that starts outside hidden content; opens tables, rows and
    /// cells.
    fn role(&mut self, name: &str, tag: &Tag) -> Role {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                Role::Heading(name.as_bytes()[1].saturating_sub(b'0'))
            }
            "li" => Role::ListItem,
            "pre" | "listing" | "plaintext" => {
                self.pre += 1;
                Role::Pre
            }
            "header" => Role::Zone(Zone::Header),
            "footer" => Role::Zone(Zone::Footer),
            "table" => {
                if self.tables.len() >= MAX_TABLE_DEPTH {
                    self.stats.tables_capped = true;
                    return Role::Block;
                }
                self.tables.push(TableB::default());
                Role::Table
            }
            "tr" if self.in_table() => {
                if let Some(t) = self.tables.last_mut() {
                    t.close_cell();
                    t.next_row();
                }
                Role::Row
            }
            "td" | "th" if self.in_table() => {
                let span = |n: &str, max: u32| {
                    attr(tag, n)
                        .and_then(|v| v.trim().parse::<u32>().ok())
                        .map_or(1, |v| v.clamp(1, max))
                };
                let (cs, rs) = (span("colspan", MAX_COLSPAN), span("rowspan", MAX_ROWSPAN));
                if let Some(t) = self.tables.last_mut() {
                    t.close_cell();
                    t.open_cell(cs, rs);
                }
                Role::Cell
            }
            n if is(n, BLOCKS) => Role::Block,
            _ => Role::Inline,
        }
    }

    /// The nearest open table on the stack is the innermost table being built.
    fn in_table(&self) -> bool {
        !self.tables.is_empty() && self.open.iter().any(|o| o.role == Role::Table)
    }

    /// End tags a start tag implies (`<p>` closes an open `<p>`, `<td>` the open cell, ...).
    fn implied_closes(&mut self, name: &str) {
        if self.open_p > 0
            && is(name, CLOSES_P)
            && let Some(i) = self.find_open(|n| n == "p", |n| is(n, P_SCOPE))
        {
            self.pop_to(i);
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if self
                    .open
                    .last()
                    .is_some_and(|o| matches!(o.role, Role::Heading(_)))
                {
                    self.pop_to(self.open.len() - 1);
                }
            }
            "li" if self.open_li > 0 => {
                if let Some(i) = self.find_open(
                    |n| n == "li",
                    |n| matches!(n, "ul" | "ol" | "menu" | "table" | "td" | "th"),
                ) {
                    self.pop_to(i);
                }
            }
            "dt" | "dd" if self.open_dt > 0 => {
                if let Some(i) = self.find_open(
                    |n| matches!(n, "dt" | "dd"),
                    |n| matches!(n, "dl" | "table" | "td" | "th"),
                ) {
                    self.pop_to(i);
                }
            }
            "tr" | "tbody" | "thead" | "tfoot" | "caption" | "colgroup" => {
                if let Some(t) = self.open.iter().rposition(|o| o.role == Role::Table) {
                    self.pop_to(t + 1);
                }
            }
            "td" | "th" => {
                if let Some(t) = self.open.iter().rposition(|o| o.role == Role::Table) {
                    match self.open[t..].iter().rposition(|o| o.role == Role::Row) {
                        Some(r) => self.pop_to(t + r + 1),
                        None => self.pop_to(t + 1),
                    }
                }
            }
            _ => {}
        }
    }

    /// Index of the nearest open element matching `want`, not crossing `stop`.
    fn find_open(&self, want: impl Fn(&str) -> bool, stop: impl Fn(&str) -> bool) -> Option<usize> {
        for (i, o) in self.open.iter().enumerate().rev() {
            if want(&o.name) {
                return Some(i);
            }
            if stop(&o.name) {
                return None;
            }
        }
        None
    }

    fn end(&mut self, name: &str) {
        match name {
            "html" | "body" | "head" => {
                self.flush();
                return;
            }
            "br" => {
                self.line_break();
                return;
            }
            _ => {}
        }
        if let Some(i) = self.open.iter().rposition(|o| &*o.name == name) {
            self.pop_to(i);
        } else if is(name, BLOCKS) {
            self.flush();
        }
    }

    /// Closes the open elements from the top down to index `i` (included).
    fn pop_to(&mut self, i: usize) {
        while self.open.len() > i {
            // The block ends while its element is still open, so it gets the element's kind.
            if self
                .open
                .last()
                .is_some_and(|o| o.role != Role::Inline || is(&o.name, BLOCKS))
            {
                self.flush();
            }
            let Some(o) = self.open.pop() else { break };
            self.close(&o);
        }
    }

    fn close(&mut self, o: &Open) {
        self.count(&o.name, -1);
        if o.skip {
            self.skip = self.skip.saturating_sub(1);
        }
        if o.title
            && let Some(t) = self.title.take()
        {
            let t = clean(&t);
            if !t.is_empty() {
                self.stats.title = Some(t);
            }
        }
        if o.capture
            && let Some((n, v)) = self.dei.take()
            && self.stats.dei.len() < MAX_DEI
        {
            let v = clean(&v);
            if !n.is_empty() && !v.is_empty() {
                self.stats.dei.push(format!("{n}={v}"));
            }
        }
        match o.role {
            Role::Pre => self.pre = self.pre.saturating_sub(1),
            Role::Cell | Role::Row => {
                if let Some(t) = self.tables.last_mut() {
                    t.close_cell();
                }
            }
            Role::Table => {
                if let Some(mut t) = self.tables.pop() {
                    t.close_cell();
                    self.push_item(Item::Table(t));
                }
            }
            _ => {}
        }
        if o.break_after && self.skip == 0 {
            self.break_here();
        }
    }

    fn count(&mut self, name: &str, by: isize) {
        let n = match name {
            "p" => &mut self.open_p,
            "li" => &mut self.open_li,
            "dt" | "dd" => &mut self.open_dt,
            _ => return,
        };
        *n = n.saturating_add_signed(by);
    }

    /// Prefixes bound to the inline XBRL namespace, and whether this is XHTML.
    fn namespaces(&mut self, tag: &Tag) {
        for a in &tag.attrs {
            let n: &str = &a.name.local;
            let v: &str = &a.value;
            if n == "xmlns" && v == "http://www.w3.org/1999/xhtml" {
                self.xhtml = true;
            } else if let Some(p) = n.strip_prefix("xmlns:")
                && IX_NS.contains(&v.trim())
            {
                self.stats.inline_xbrl = true;
                if self.ix_prefixes.len() < 8 && !self.ix_prefixes.iter().any(|x| x == p) {
                    self.ix_prefixes.push(p.to_owned());
                }
            }
        }
    }

    /// End of input: everything still open is closed.
    fn finish(mut self) -> (Vec<Page>, Stats) {
        self.title = None;
        self.pop_to(0);
        self.flush();
        while let Some(mut t) = self.tables.pop() {
            t.close_cell();
            self.push_item(Item::Table(t));
        }
        if !self.page.text.is_empty() || self.pages.is_empty() {
            self.finish_page();
        }
        (self.pages, self.stats)
    }
}

fn push_capped(s: &mut String, t: &str, max: usize) {
    for c in t.chars() {
        if s.len() + c.len_utf8() > max {
            break;
        }
        s.push(c);
    }
}

/// Whitespace collapsed and trimmed, control characters as spaces, at most 4 KiB.
pub(crate) fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if out.len() + c.len_utf8() + 1 > MAX_SMALL {
            break;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// A `style` value lowercased without whitespace (first 4 KiB), for substring tests such
/// as `display:none`.
fn compact_style(s: &str) -> String {
    s.chars()
        .take(MAX_SMALL)
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn attr(tag: &Tag, name: &str) -> Option<String> {
    tag.attrs
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| a.value.to_string())
}

/// The tokenizer state the content of this element is read in (the tree builder's job,
/// which is not used).
fn raw_kind(name: &str) -> Option<TokenSinkResult<()>> {
    Some(match name {
        "script" => TokenSinkResult::RawData(RawKind::ScriptData),
        "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
            TokenSinkResult::RawData(RawKind::Rawtext)
        }
        "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
        "plaintext" => TokenSinkResult::Plaintext,
        _ => return None,
    })
}

struct Sink(RefCell<State>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut st = self.0.borrow_mut();
        st.tokens = st.tokens.wrapping_add(1);
        match token {
            Token::TagToken(tag) => match tag.kind {
                TagKind::StartTag => return st.start(&tag),
                TagKind::EndTag => st.end(&tag.name),
            },
            Token::CharacterTokens(t) => st.text(&t),
            // `<?xml ...?>` is a bogus comment to an HTML tokenizer.
            Token::CommentToken(c) if c.starts_with("?xml") => st.xhtml = true,
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// Why reading stopped early, besides the limits counted in [`Stats`].
#[derive(Debug, Default)]
pub(crate) struct Ended {
    pub timed_out: bool,
    /// Bytes read without a token completing.
    pub oversize_token: bool,
    pub decode_errors: u64,
}

/// Decodes `bytes` (BOM already skipped) with `enc` chunk by chunk and builds the pages.
pub(crate) fn run(
    bytes: &[u8],
    enc: &'static encoding_rs::Encoding,
    limits: &Limits,
    out: &Emitter,
) -> (Vec<Page>, Stats, Ended) {
    let tok = Tokenizer::new(
        Sink(RefCell::new(State::new(limits))),
        TokenizerOpts::default(),
    );
    let input = BufferQueue::default();
    let mut dec = enc.new_decoder_without_bom_handling();
    let mut ended = Ended::default();
    let mut buf = String::new();
    let mut since = 0u64;
    let n = bytes.len().div_ceil(CHUNK).max(1);
    let mut complete = true;
    for i in 0..n {
        if out.check_deadline().is_err() {
            ended.timed_out = true;
            complete = false;
            break;
        }
        let chunk = bytes
            .get(i * CHUNK..((i + 1) * CHUNK).min(bytes.len()))
            .unwrap_or(&[]);
        let last = i + 1 == n;
        buf.clear();
        buf.reserve(
            dec.max_utf8_buffer_length(chunk.len())
                .unwrap_or(chunk.len() * 3 + 16),
        );
        let (_, _, errors) = dec.decode_to_string(chunk, &mut buf, last);
        if errors {
            ended.decode_errors += buf.matches('\u{fffd}').count() as u64;
        }
        let before = tok.sink.0.borrow().tokens;
        if !buf.is_empty() {
            input.push_back(StrTendril::from_slice(&buf));
            let _ = tok.feed(&input);
        }
        let st = tok.sink.0.borrow();
        if st.tokens == before {
            since += chunk.len() as u64;
        } else {
            since = 0;
        }
        if st.stopped {
            complete = false;
            break;
        }
        if since > MAX_TOKEN {
            ended.oversize_token = true;
            complete = false;
            break;
        }
    }
    if complete {
        tok.end();
    }
    let st = tok.sink.0.into_inner();
    let (pages, stats) = st.finish();
    (pages, stats, ended)
}

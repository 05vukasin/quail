//! One slide-like part (slide, layout, master, notes) as a tree of shapes: placeholders,
//! positions, paragraphs and tables. Streaming, with every count bounded.

use quick_xml::events::BytesStart;

use quail_ooxml::xml::{Ev, Flow, attr, local, name};

/// Most groups nested; deeper groups are read as part of their parent.
pub(crate) const MAX_GROUP_DEPTH: usize = 64;
/// Most shapes, paragraphs and cells per part.
pub(crate) const MAX_NODES: usize = 100_000;
/// Longest text kept per paragraph (more is cut).
const MAX_PARA: usize = 1 << 20;

/// A placeholder reference: `p:ph type` (missing: `obj`) and `idx`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ph {
    pub kind: String,
    pub idx: Option<String>,
}

/// How a paragraph's bullet is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Bullet {
    /// Not set here (inherited).
    #[default]
    Inherit,
    /// `a:buNone`.
    Off,
    /// `a:buChar`, `a:buAutoNum` or `a:buBlip`.
    On,
}

#[derive(Debug, Default)]
pub(crate) struct Para {
    pub text: String,
    pub bullet: Bullet,
}

#[derive(Debug)]
pub(crate) struct Cell {
    pub row: u32,
    pub col: u32,
    pub text: String,
}

#[derive(Debug, Default)]
pub(crate) struct Shape {
    pub ph: Option<Ph>,
    pub pos: Option<(i64, i64)>,
    pub paras: Vec<Para>,
    /// Cells of an `a:tbl`, merged continuation cells left out.
    pub cells: Option<Vec<Cell>>,
}

#[derive(Debug)]
pub(crate) enum Node {
    Shape(Shape),
    Group {
        pos: Option<(i64, i64)>,
        nodes: Vec<Node>,
    },
}

#[derive(Default)]
struct Group {
    pos: Option<(i64, i64)>,
    hidden: bool,
    nodes: Vec<Node>,
}

/// What the walk found besides the shapes.
#[derive(Debug, Default)]
pub(crate) struct Info {
    /// `p:sld show="0"`.
    pub hidden_slide: bool,
    /// Shapes left out because `cNvPr hidden="1"`.
    pub hidden_shapes: u64,
    /// More than [`MAX_NODES`] nodes or [`MAX_GROUP_DEPTH`] groups: the rest flattened or
    /// dropped.
    pub capped: bool,
    /// Text over the per-paragraph cap was cut.
    pub cut: bool,
}

/// Streaming builder of one part's shape tree.
pub(crate) struct Parser {
    groups: Vec<Group>,
    /// `grpSp` starts not pushed (too deep), so their ends are not popped.
    group_overflow: usize,
    shape: Option<(Shape, bool)>,
    /// `sp` / `graphicFrame` starts inside a shape (invalid nesting), ignored.
    shape_overflow: usize,
    para: Option<Para>,
    in_t: bool,
    in_xfrm: bool,
    /// Table being read: current row, column, cell text.
    row: Option<u32>,
    col: u32,
    cell: Option<(u32, u32, String, bool)>,
    nodes: usize,
    pub info: Info,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            groups: vec![Group::default()],
            group_overflow: 0,
            shape: None,
            shape_overflow: 0,
            para: None,
            in_t: false,
            in_xfrm: false,
            row: None,
            col: 0,
            cell: None,
            nodes: 0,
            info: Info::default(),
        }
    }

    /// The shapes of the part, in file order (groups nested).
    pub fn finish(mut self) -> Vec<Node> {
        self.end_shape();
        while self.groups.len() > 1 {
            self.end_group();
        }
        self.groups.pop().map(|g| g.nodes).unwrap_or_default()
    }

    fn count(&mut self) -> bool {
        if self.nodes >= MAX_NODES {
            self.info.capped = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    pub fn ev(&mut self, ev: Ev<'_>) -> Flow {
        match ev {
            Ev::Start(e) => self.start(e, false),
            Ev::Empty(e) => {
                let f = self.start(e, true);
                if f == Flow::Go {
                    self.end(name(e));
                }
                Flow::Go
            }
            Ev::End(n) => {
                self.end(n);
                Flow::Go
            }
            Ev::Text(t) => {
                if self.in_t
                    && let Some(p) = self.para.as_mut()
                {
                    push_capped(&mut p.text, t, &mut self.info.cut);
                }
                Flow::Go
            }
        }
    }

    #[allow(clippy::many_single_char_names)] // e, n, x, y, s: the usual XML walk names
    fn start(&mut self, e: &BytesStart<'_>, empty: bool) -> Flow {
        let n = name(e);
        match n {
            // One copy of alternate content (the `mc:Choice`), extensions never.
            "Fallback" | "extLst" => return if empty { Flow::Go } else { Flow::Skip },
            "sld" => {
                self.info.hidden_slide = attr(e, "show").is_some_and(|v| off(&v));
            }
            "grpSp" if self.shape.is_none() => {
                if self.groups.len() >= MAX_GROUP_DEPTH || !self.count() {
                    self.info.capped = true;
                    self.group_overflow += 1;
                } else {
                    self.groups.push(Group::default());
                }
            }
            "sp" | "graphicFrame" | "cxnSp" | "pic" => {
                if self.shape.is_some() {
                    self.shape_overflow += 1;
                } else if self.count() {
                    // Connectors and pictures have no text body; read for nothing.
                    let keep = matches!(n, "sp" | "graphicFrame");
                    self.shape = Some((Shape::default(), !keep));
                } else {
                    return if empty { Flow::Go } else { Flow::Skip };
                }
            }
            "cNvPr" => {
                if attr(e, "hidden").is_some_and(|v| on(&v)) {
                    match self.shape.as_mut() {
                        Some((_, hidden)) => *hidden = true,
                        None => {
                            if let Some(g) = self.groups.last_mut() {
                                g.hidden = true;
                            }
                        }
                    }
                }
            }
            "ph" => {
                if let Some((s, _)) = self.shape.as_mut() {
                    s.ph = Some(Ph {
                        kind: attr(e, "type")
                            .map_or_else(|| "obj".to_owned(), std::borrow::Cow::into_owned),
                        idx: attr(e, "idx").map(std::borrow::Cow::into_owned),
                    });
                }
            }
            "xfrm" => self.in_xfrm = true,
            "off" if self.in_xfrm => {
                let x = attr(e, "x").and_then(|v| v.trim().parse::<i64>().ok());
                let y = attr(e, "y").and_then(|v| v.trim().parse::<i64>().ok());
                if let (Some(x), Some(y)) = (x, y) {
                    match self.shape.as_mut() {
                        Some((s, _)) => {
                            s.pos.get_or_insert((x, y));
                        }
                        None => {
                            if let Some(g) = self.groups.last_mut() {
                                g.pos.get_or_insert((x, y));
                            }
                        }
                    }
                }
            }
            "graphicData" => {
                // Only tables have text in the slide part (charts and diagrams live in
                // parts of their own).
                let uri = attr(e, "uri").unwrap_or_default();
                if !uri.ends_with("/table") {
                    return if empty { Flow::Go } else { Flow::Skip };
                }
            }
            "tbl" => {
                if let Some((s, _)) = self.shape.as_mut() {
                    s.cells.get_or_insert_with(Vec::new);
                    self.row = None;
                }
            }
            "tr" if self.shape.is_some() => {
                self.row = Some(self.row.map_or(0, |r| r.saturating_add(1)));
                self.col = 0;
            }
            "tc" if self.shape.is_some() => {
                let merged = ["hMerge", "vMerge"]
                    .iter()
                    .any(|a| attr(e, a).is_some_and(|v| on(&v)));
                self.cell = Some((self.row.unwrap_or(0), self.col, String::new(), merged));
                self.col = self.col.saturating_add(1);
            }
            "p" if self.shape.is_some() && self.para.is_none() => {
                self.para = Some(Para::default());
            }
            "buNone" => self.bullet(Bullet::Off),
            "buChar" | "buAutoNum" | "buBlip" => self.bullet(Bullet::On),
            "t" if self.para.is_some() => self.in_t = true,
            "br" => {
                if let Some(p) = self.para.as_mut() {
                    push_capped(&mut p.text, "\n", &mut self.info.cut);
                }
            }
            _ => {}
        }
        Flow::Go
    }

    fn bullet(&mut self, b: Bullet) {
        if let Some(p) = self.para.as_mut() {
            p.bullet = b;
        }
    }

    fn end(&mut self, n: &str) {
        match local(n) {
            "grpSp" if self.shape.is_none() => {
                if self.group_overflow > 0 {
                    self.group_overflow -= 1;
                } else if self.groups.len() > 1 {
                    self.end_group();
                }
            }
            "sp" | "graphicFrame" | "cxnSp" | "pic" => {
                if self.shape_overflow > 0 {
                    self.shape_overflow -= 1;
                } else {
                    self.end_shape();
                }
            }
            "xfrm" => self.in_xfrm = false,
            "t" => self.in_t = false,
            "p" => {
                if let Some(p) = self.para.take() {
                    match self.cell.as_mut() {
                        Some((.., text, _)) => {
                            if !p.text.trim().is_empty() {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(p.text.trim());
                            }
                        }
                        None => {
                            if let Some((s, _)) = self.shape.as_mut() {
                                if self.nodes < MAX_NODES {
                                    self.nodes += 1;
                                    s.paras.push(p);
                                } else {
                                    self.info.capped = true;
                                }
                            }
                        }
                    }
                }
            }
            "tc" => {
                if let Some((row, col, text, merged)) = self.cell.take()
                    && !merged
                    && !text.is_empty()
                    && let Some((s, _)) = self.shape.as_mut()
                    && let Some(cells) = s.cells.as_mut()
                {
                    if self.nodes < MAX_NODES {
                        self.nodes += 1;
                        cells.push(Cell { row, col, text });
                    } else {
                        self.info.capped = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn end_shape(&mut self) {
        self.para = None;
        self.cell = None;
        self.in_t = false;
        if let Some((s, hidden)) = self.shape.take() {
            let text = !s.paras.is_empty() || s.cells.as_ref().is_some_and(|c| !c.is_empty());
            if hidden {
                self.info.hidden_shapes += u64::from(text);
            } else if (text || s.ph.is_some())
                && let Some(g) = self.groups.last_mut()
            {
                g.nodes.push(Node::Shape(s));
            }
        }
    }

    fn end_group(&mut self) {
        if let Some(g) = self.groups.pop() {
            if g.hidden {
                self.info.hidden_shapes += 1;
                return;
            }
            if let Some(parent) = self.groups.last_mut() {
                parent.nodes.push(Node::Group {
                    pos: g.pos,
                    nodes: g.nodes,
                });
            }
        }
    }
}

fn push_capped(s: &mut String, t: &str, cut: &mut bool) {
    if s.len() + t.len() <= MAX_PARA {
        s.push_str(t);
        return;
    }
    *cut = true;
    let mut room = MAX_PARA.saturating_sub(s.len());
    while room > 0 && !t.is_char_boundary(room) {
        room -= 1;
    }
    s.push_str(&t[..room]);
}

/// An OOXML boolean that is on (`1`, `true`, `on`).
fn on(v: &str) -> bool {
    matches!(v.trim(), "1" | "true" | "on")
}

fn off(v: &str) -> bool {
    matches!(v.trim(), "0" | "false" | "off")
}

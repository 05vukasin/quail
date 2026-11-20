//! `word/styles.xml`: what each paragraph style means for structure (heading level,
//! numbering), resolved down the `w:basedOn` chain.

use std::collections::HashMap;

use quail_core::Emitter;

use crate::package::Package;
use crate::xml::{Ev, Flow, attr, name, val, val_int, walk};

/// Longest `w:basedOn` chain followed (also stops cycles).
const MAX_CHAIN: usize = 32;
/// Most styles kept.
const MAX_STYLES: usize = 20_000;

#[derive(Clone, Debug, Default)]
struct Style {
    name: Option<String>,
    based_on: Option<String>,
    /// `w:outlineLvl`, 0-based (9 = body text).
    outline: Option<u8>,
    num_id: Option<String>,
    ilvl: Option<u8>,
}

/// Paragraph styles of a document.
#[derive(Debug, Default)]
pub(crate) struct Styles {
    map: HashMap<String, Style>,
    default_para: Option<String>,
    heading: HashMap<String, u8>,
}

impl Styles {
    /// Reads `part`; missing or broken parts give what could be read.
    pub fn read(pkg: &mut Package<'_>, part: &str, out: &Emitter, warn: &mut Vec<String>) -> Self {
        let mut s = Self::default();
        let Some(reader) = pkg.open_part(part) else {
            return s;
        };
        // Depth of the current element; `style_depth` is the depth of the open w:style.
        let mut depth = 0usize;
        let mut style_depth = 0usize;
        let mut ppr_depth = 0usize;
        let mut cur: Option<(String, Style)> = None;
        let map = &mut s.map;
        let default_para = &mut s.default_para;
        let r = walk(reader, out, |ev| {
            match ev {
                Ev::Start(e) | Ev::Empty(e) => {
                    let start = matches!(ev, Ev::Start(_));
                    let n = name(e);
                    if start {
                        depth += 1;
                    }
                    match n {
                        "style" if start && cur.is_none() => {
                            let id = attr(e, "styleId").unwrap_or_default().into_owned();
                            let is_para = attr(e, "type").is_none_or(|t| t == "paragraph");
                            if is_para
                                && attr(e, "default").is_some_and(|d| d == "1" || d == "true")
                            {
                                *default_para = Some(id.clone());
                            }
                            style_depth = depth;
                            cur = Some((id, Style::default()));
                        }
                        "name" if depth == style_depth + usize::from(start) => {
                            if let Some((_, st)) = cur.as_mut() {
                                st.name = val(e).map(|v| v.to_lowercase());
                            }
                        }
                        "basedOn" if depth == style_depth + usize::from(start) => {
                            if let Some((_, st)) = cur.as_mut() {
                                st.based_on = val(e).map(std::borrow::Cow::into_owned);
                            }
                        }
                        "pPr" if start && cur.is_some() && depth == style_depth + 1 => {
                            ppr_depth = depth;
                        }
                        "outlineLvl" if ppr_depth > 0 => {
                            if let Some((_, st)) = cur.as_mut() {
                                st.outline = val_int(e).and_then(|v| u8::try_from(v).ok());
                            }
                        }
                        "ilvl" if ppr_depth > 0 => {
                            if let Some((_, st)) = cur.as_mut() {
                                st.ilvl = val_int(e).and_then(|v| u8::try_from(v).ok());
                            }
                        }
                        "numId" if ppr_depth > 0 => {
                            if let Some((_, st)) = cur.as_mut() {
                                st.num_id = val(e).map(|v| v.trim().to_owned());
                            }
                        }
                        // Table-style conditional formatting carries its own pPr: not ours.
                        "tblStylePr" if start => {
                            depth -= 1;
                            return Flow::Skip;
                        }
                        _ => {}
                    }
                }
                Ev::End(n) => {
                    if n == "pPr" && depth == ppr_depth {
                        ppr_depth = 0;
                    }
                    if n == "style" && depth == style_depth {
                        if let Some((id, st)) = cur.take()
                            && map.len() < MAX_STYLES
                        {
                            map.entry(id).or_insert(st);
                        }
                        style_depth = 0;
                    }
                    depth = depth.saturating_sub(1);
                }
                Ev::Text(_) => {}
            }
            Flow::Go
        });
        if let Err(stop) = r {
            warn.push(stop.describe(part));
        }
        s.heading = s
            .map
            .keys()
            .filter_map(|id| s.compute_heading(id).map(|l| (id.clone(), l)))
            .collect();
        s
    }

    fn compute_heading(&self, id: &str) -> Option<u8> {
        let mut id = id;
        for _ in 0..MAX_CHAIN {
            let st = self.map.get(id)?;
            if let Some(o) = st.outline {
                // 0..=8 are heading levels 1..=9; 9 means body text.
                return (o < 9).then_some(o + 1);
            }
            if let Some(l) = st.name.as_deref().and_then(heading_name_level) {
                return Some(l);
            }
            id = st.based_on.as_deref()?;
        }
        None
    }

    /// Heading level (1 = highest) of a paragraph style, if it is a heading style.
    pub fn heading_level(&self, style: &str) -> Option<u8> {
        self.heading.get(style).copied()
    }

    /// The default paragraph style id.
    pub fn default_paragraph(&self) -> Option<&str> {
        self.default_para.as_deref()
    }

    /// `numId` and `ilvl` a style carries, each inherited separately down the chain (Word's
    /// stock "heading 2" carries only its level and takes the list from "heading 1").
    pub fn numbering(&self, style: &str) -> (Option<&str>, Option<u8>) {
        let mut num_id = None;
        let mut ilvl = None;
        let mut id = Some(style);
        for _ in 0..MAX_CHAIN {
            let Some(st) = id.and_then(|i| self.map.get(i)) else {
                break;
            };
            num_id = num_id.or(st.num_id.as_deref());
            ilvl = ilvl.or(st.ilvl);
            if num_id.is_some() && ilvl.is_some() {
                break;
            }
            id = st.based_on.as_deref();
        }
        (num_id, ilvl)
    }
}

/// `heading 1` .. `heading 9`, `title`: the built-in names Word writes whatever the UI
/// language.
fn heading_name_level(name: &str) -> Option<u8> {
    if name == "title" {
        return Some(1);
    }
    let rest = name.strip_prefix("heading")?.trim();
    match rest.parse::<u8>() {
        Ok(l @ 1..=9) => Some(l),
        _ => None,
    }
}

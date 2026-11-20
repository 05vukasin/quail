//! `word/numbering.xml`: the labels Word shows before numbered paragraphs ("7", "7.1",
//! "(a)"), computed the way Word counts.
//!
//! Contract templates often share one list between article headings and clauses ("7 AUDIT
//! RIGHTS" over "7.1", "7.2"); writing the label into the text keeps that structure visible
//! to every later analyzer.

use std::collections::{HashMap, HashSet};

use quail_core::Emitter;

use crate::package::Package;
use crate::styles::Styles;
use crate::xml::{Ev, Flow, attr, name, on, val, val_int, walk};

/// Levels of a list.
pub(crate) const LEVELS: usize = 9;
/// Label of a bullet level.
const BULLET: &str = "-";
/// Longest level text kept, in characters.
const MAX_LEVEL_TEXT: usize = 64;
/// Longest label kept, in characters.
const MAX_LABEL: usize = 32;
/// Most lists kept.
const MAX_LISTS: usize = 20_000;

/// One level definition as written: every field may be missing (an override keeps what it
/// leaves out from the definition it overrides).
#[derive(Clone, Debug, Default)]
struct RawLevel {
    start: Option<i64>,
    fmt: Option<String>,
    text: Option<String>,
    restart: Option<i64>,
    legal: Option<bool>,
    style: Option<String>,
}

impl RawLevel {
    fn over(self, base: &RawLevel) -> RawLevel {
        RawLevel {
            start: self.start.or(base.start),
            fmt: self.fmt.or_else(|| base.fmt.clone()),
            text: self.text.or_else(|| base.text.clone()),
            restart: self.restart.or(base.restart),
            legal: self.legal.or(base.legal),
            style: self.style.or_else(|| base.style.clone()),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Level {
    start: i64,
    fmt: String,
    text: String,
    restart: Option<i64>,
    legal: bool,
    style: Option<String>,
}

impl From<&RawLevel> for Level {
    fn from(r: &RawLevel) -> Self {
        Self {
            start: r.start.unwrap_or(0),
            fmt: r.fmt.clone().unwrap_or_else(|| "decimal".to_owned()),
            text: r.text.clone().unwrap_or_default(),
            restart: r.restart,
            legal: r.legal.unwrap_or(false),
            style: r.style.clone(),
        }
    }
}

/// A `w:num`: the abstract list it draws from, with its overrides applied.
#[derive(Clone, Debug)]
struct Instance {
    abstract_id: String,
    levels: Vec<Level>,
    restarts: Vec<usize>,
}

impl Instance {
    fn level(&self, ilvl: usize) -> Option<&Level> {
        self.levels.get(ilvl)
    }
}

#[derive(Default)]
struct RawAbstract {
    levels: Vec<RawLevel>,
    style_link: Option<String>,
}

#[derive(Default)]
struct RawNum {
    abstract_id: Option<String>,
    overrides: Vec<(usize, Option<RawLevel>, Option<i64>)>,
}

/// The numbering definitions of a document.
#[derive(Debug, Default)]
pub(crate) struct Numbering {
    instances: HashMap<String, Instance>,
}

/// Which list level a paragraph is numbered at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NumRef {
    num_id: String,
    ilvl: usize,
}

impl Numbering {
    /// Reads `part`. Broken parts give what could be read.
    #[allow(clippy::too_many_lines)]
    pub fn read(
        pkg: &mut Package<'_>,
        part: &str,
        styles: &Styles,
        out: &Emitter,
        warn: &mut Vec<String>,
    ) -> Self {
        let Some(reader) = pkg.open_part(part) else {
            return Self::default();
        };
        let mut abstracts: HashMap<String, RawAbstract> = HashMap::new();
        let mut nums: Vec<(String, RawNum)> = Vec::new();
        let mut cur_abs: Option<(String, RawAbstract)> = None;
        let mut cur_num: Option<(String, RawNum)> = None;
        // (ilvl, level) being read, inside an abstractNum or a lvlOverride.
        let mut cur_lvl: Option<(usize, RawLevel)> = None;
        // (ilvl, startOverride) of the open lvlOverride.
        let mut cur_ovr: Option<(usize, Option<RawLevel>, Option<i64>)> = None;
        let r = walk(reader, out, |ev| {
            match ev {
                Ev::Start(e) | Ev::Empty(e) => {
                    let start = matches!(ev, Ev::Start(_));
                    match name(e) {
                        "abstractNum" if start => {
                            let id = attr(e, "abstractNumId").unwrap_or_default().into_owned();
                            cur_abs = Some((id, RawAbstract::default()));
                        }
                        "num" if start && cur_abs.is_none() => {
                            let id = attr(e, "numId").unwrap_or_default().trim().to_owned();
                            cur_num = Some((id, RawNum::default()));
                        }
                        "abstractNumId" => {
                            if let Some((_, n)) = cur_num.as_mut() {
                                n.abstract_id = val(e).map(|v| v.trim().to_owned());
                            }
                        }
                        "lvlOverride" if start => {
                            if let Some(i) = ilvl_attr(e) {
                                cur_ovr = Some((i, None, None));
                            }
                        }
                        "startOverride" => {
                            if let Some((_, _, s)) = cur_ovr.as_mut() {
                                *s = val_int(e).map(clamp_start);
                            }
                        }
                        "lvl" if start => {
                            cur_lvl = ilvl_attr(e).map(|i| (i, RawLevel::default()));
                        }
                        "start" => set(&mut cur_lvl, |l| l.start = val_int(e).map(clamp_start)),
                        "numFmt" => set(&mut cur_lvl, |l| {
                            l.fmt = val(e).map(std::borrow::Cow::into_owned);
                        }),
                        "lvlText" => set(&mut cur_lvl, |l| {
                            // Real level texts are a few characters; a hostile one must not be
                            // copied into every numbered paragraph.
                            let v = val(e).unwrap_or_default();
                            l.text = Some(v.chars().take(MAX_LEVEL_TEXT).collect());
                        }),
                        "lvlRestart" => set(&mut cur_lvl, |l| l.restart = val_int(e)),
                        "isLgl" => set(&mut cur_lvl, |l| l.legal = Some(on(e))),
                        "pStyle" => set(&mut cur_lvl, |l| {
                            l.style = val(e).map(std::borrow::Cow::into_owned);
                        }),
                        "numStyleLink" => {
                            if let Some((_, a)) = cur_abs.as_mut() {
                                a.style_link = val(e).map(std::borrow::Cow::into_owned);
                            }
                        }
                        // A level's paragraph/run properties hold nothing we need.
                        "pPr" | "rPr" if start => return Flow::Skip,
                        _ => {}
                    }
                }
                Ev::End(n) => match n {
                    "lvl" => {
                        if let Some((i, lvl)) = cur_lvl.take() {
                            if let Some((_, ovr, _)) = cur_ovr.as_mut() {
                                *ovr = Some(lvl);
                            } else if let Some((_, a)) = cur_abs.as_mut() {
                                if a.levels.len() < LEVELS {
                                    a.levels.resize_with(LEVELS, RawLevel::default);
                                }
                                if let Some(slot) = a.levels.get_mut(i) {
                                    *slot = lvl;
                                }
                            }
                        }
                    }
                    "lvlOverride" => {
                        if let (Some(o), Some((_, n))) = (cur_ovr.take(), cur_num.as_mut()) {
                            // Word keeps one override per level: the last one counts.
                            if let Some(slot) = n.overrides.iter_mut().find(|x| x.0 == o.0) {
                                *slot = o;
                            } else {
                                n.overrides.push(o);
                            }
                        }
                    }
                    "abstractNum" => {
                        if let Some((id, a)) = cur_abs.take()
                            && abstracts.len() < MAX_LISTS
                        {
                            abstracts.entry(id).or_insert(a);
                        }
                    }
                    "num" => {
                        if let Some(n) = cur_num.take()
                            && nums.len() < MAX_LISTS
                        {
                            nums.push(n);
                        }
                    }
                    _ => {}
                },
                Ev::Text(_) => {}
            }
            Flow::Go
        });
        if let Err(stop) = r {
            warn.push(stop.describe(part));
        }

        let targets: HashMap<&str, &str> = nums
            .iter()
            .filter_map(|(id, n)| Some((id.as_str(), n.abstract_id.as_deref()?)))
            .collect();
        let mut instances = HashMap::with_capacity(nums.len());
        for (num_id, n) in &nums {
            let Some(abs_id) = n.abstract_id.as_deref() else {
                continue;
            };
            let abs_id = linked_abstract(abs_id, &abstracts, &targets, styles);
            let Some(a) = abstracts.get(abs_id) else {
                continue;
            };
            let mut raw: Vec<RawLevel> = a.levels.clone();
            raw.resize_with(LEVELS, RawLevel::default);
            let mut restarts = Vec::new();
            for (ilvl, lvl, start) in &n.overrides {
                let Some(slot) = raw.get_mut(*ilvl) else {
                    continue;
                };
                if let Some(l) = lvl {
                    *slot = l.clone().over(slot);
                }
                if let Some(s) = start {
                    slot.start = Some(*s);
                    restarts.push(*ilvl);
                }
            }
            instances.entry(num_id.clone()).or_insert(Instance {
                abstract_id: abs_id.to_owned(),
                levels: raw.iter().map(Level::from).collect(),
                restarts,
            });
        }
        Self { instances }
    }

    /// True when no list is defined.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The list and level a paragraph is numbered at: its own `w:numPr`, else its style's,
    /// down the `basedOn` chain. `numId` 0 switches numbering off.
    pub fn numbering_of(
        &self,
        styles: &Styles,
        p_style: Option<&str>,
        num_id: Option<&str>,
        ilvl: Option<u8>,
    ) -> Option<NumRef> {
        let style_id = p_style.or(styles.default_paragraph());
        let (style_num, style_ilvl) = style_id.map_or((None, None), |s| styles.numbering(s));
        let num_id = num_id.or(style_num)?;
        let inst = self.instances.get(num_id)?;
        let ilvl = match ilvl.or(style_ilvl) {
            Some(i) => usize::from(i),
            None => inst
                .levels
                .iter()
                .position(|l| style_id.is_some() && l.style.as_deref() == style_id)
                .unwrap_or(0),
        };
        Some(NumRef {
            num_id: num_id.to_owned(),
            ilvl: ilvl.min(LEVELS - 1),
        })
    }
}

/// Word accepts start values 0..=32767; anything else is clamped into that range.
fn clamp_start(v: i64) -> i64 {
    v.clamp(0, 32_767)
}

fn set(cur: &mut Option<(usize, RawLevel)>, f: impl FnOnce(&mut RawLevel)) {
    if let Some((_, l)) = cur.as_mut() {
        f(l);
    }
}

fn ilvl_attr(e: &quick_xml::events::BytesStart<'_>) -> Option<usize> {
    attr(e, "ilvl")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&i| i < LEVELS)
}

/// A list defined by a numbering style (`w:numStyleLink`) takes its levels from the list
/// that style points at.
fn linked_abstract<'s>(
    mut abs_id: &'s str,
    abstracts: &'s HashMap<String, RawAbstract>,
    targets: &HashMap<&'s str, &'s str>,
    styles: &'s Styles,
) -> &'s str {
    let mut seen = HashSet::new();
    while let Some(link) = abstracts.get(abs_id).and_then(|a| a.style_link.as_deref()) {
        if !seen.insert(abs_id) {
            break;
        }
        let Some(linked) = styles.numbering(link).0.and_then(|n| targets.get(n)) else {
            break;
        };
        abs_id = linked;
    }
    abs_id
}

/// Word's counters: shared by every `w:num` drawing from one abstract list; a level restarts
/// when a shallower one advances (unless `w:lvlRestart` says otherwise) or when a
/// `w:startOverride` instance is first used.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    values: HashMap<String, [Option<i64>; LEVELS]>,
    started: HashSet<String>,
}

impl Counters {
    /// Advances the counter of `r` and returns the label Word shows (may be empty).
    pub fn label(&mut self, numbering: &Numbering, r: &NumRef) -> String {
        let Some(inst) = numbering.instances.get(&r.num_id) else {
            return String::new();
        };
        let values = self
            .values
            .entry(inst.abstract_id.clone())
            .or_insert([None; LEVELS]);
        if self.started.insert(r.num_id.clone()) {
            for &l in &inst.restarts {
                for v in values.iter_mut().skip(l) {
                    *v = None;
                }
            }
        }
        let ilvl = r.ilvl;
        let start = |l: usize| inst.level(l).map_or(0, |x| x.start);
        if let Some(v) = values.get_mut(ilvl) {
            *v = Some(v.map_or_else(|| start(ilvl), |c| c.saturating_add(1)));
        }
        let ilvl1 = i64::try_from(ilvl).unwrap_or(0);
        for (deeper, v) in values.iter_mut().enumerate().skip(ilvl + 1) {
            let restart = inst.level(deeper).and_then(|l| l.restart);
            if restart.is_none_or(|r| r != 0 && ilvl1 < r) {
                *v = None;
            }
        }
        let shown: Vec<i64> = (0..LEVELS)
            .map(|l| values[l].unwrap_or_else(|| start(l)))
            .collect();
        label(inst, ilvl, &shown)
    }
}

/// The level's text with each `%N` replaced by level N's count ("%1.%2" → "7.1"); a legal
/// level shows every count in arabic figures.
fn label(inst: &Instance, ilvl: usize, values: &[i64]) -> String {
    let Some(level) = inst.level(ilvl) else {
        return String::new();
    };
    if level.fmt == "bullet" {
        return BULLET.to_owned();
    }
    let mut out = String::new();
    let mut chars = level.text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(&d) = chars.peek()
            && let Some(n @ 1..=9) = d.to_digit(10)
        {
            chars.next();
            let referenced = (n - 1) as usize;
            let fmt = inst.level(referenced).map_or("decimal", |l| l.fmt.as_str());
            let fmt = if level.legal && fmt != "none" {
                "decimal"
            } else {
                fmt
            };
            out.push_str(&formatted(
                values.get(referenced).copied().unwrap_or(0),
                fmt,
            ));
        } else {
            out.push(c);
        }
    }
    // A label is a few characters; a hostile level text is cut.
    out.trim().chars().take(MAX_LABEL).collect()
}

fn formatted(v: i64, fmt: &str) -> String {
    match fmt {
        "none" => String::new(),
        "decimalZero" => format!("{v:02}"),
        "upperRoman" => roman(v),
        "lowerRoman" => roman(v).to_lowercase(),
        "upperLetter" => letters(v),
        "lowerLetter" => letters(v).to_lowercase(),
        "ordinal" => ordinal(v),
        _ => v.to_string(),
    }
}

fn roman(mut v: i64) -> String {
    const TABLE: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    // Beyond 3999 Roman numerals need overlines; a hostile start value must not make a
    // gigabyte of "M"s either.
    if !(1..=3999).contains(&v) {
        return v.to_string();
    }
    let mut out = String::new();
    for (amount, numeral) in TABLE {
        while v >= amount {
            out.push_str(numeral);
            v -= amount;
        }
    }
    out
}

fn letters(v: i64) -> String {
    if !(1..=26 * 20).contains(&v) {
        return v.to_string();
    }
    let idx = u8::try_from((v - 1) % 26).unwrap_or(0);
    let times = usize::try_from((v - 1) / 26 + 1).unwrap_or(1);
    char::from(b'A' + idx).to_string().repeat(times)
}

fn ordinal(v: i64) -> String {
    let suffix = if (10..=20).contains(&(v % 100)) {
        "th"
    } else {
        match v % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{v}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(roman(5000), "5000");
        assert_eq!(letters(1), "A");
        assert_eq!(letters(27), "AA");
        assert_eq!(letters(100_000), "100000");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
        assert_eq!(formatted(3, "decimalZero"), "03");
        assert_eq!(formatted(3, "lowerRoman"), "iii");
    }
}

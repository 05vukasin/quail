//! Shared OOXML plumbing for `quail-docx` and `quail-pptx`: the package (zip) read under one
//! decompression budget per file, a streaming XML walk with deadline checks and an event
//! size cap, relationship parts, and the core-properties fields.
//!
//! Nothing here emits facts: the extractors decide what to keep. Everything is
//! hostile-input safe in the same way: declared sizes are never trusted, unsafe entry names
//! are ignored, DOCTYPEs are never expanded, and malformed XML ends only its part.

pub mod package;
pub mod rels;
pub mod xml;

use quail_core::{Date, Emitter};

use crate::package::Package;
use crate::xml::{Ev, Flow, name, walk};

/// Longest property value kept, in bytes.
pub const MAX_VALUE: usize = 4096;

/// Control characters replaced by spaces, whitespace runs collapsed, trimmed, capped at
/// [`MAX_VALUE`] bytes.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_VALUE));
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() || c == '\u{fffd}' {
            space = true;
            continue;
        }
        if out.len() + c.len_utf8() + 1 > MAX_VALUE {
            break;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

/// What an extractor tells the record besides facts: warnings (notes about the file) and
/// problems (it stopped short: a limit, the deadline, a broken part), which make the record
/// partial.
#[derive(Debug, Default)]
pub struct Notes {
    /// Warnings.
    pub warnings: Vec<String>,
    /// Problems.
    pub problems: Vec<String>,
}

impl Notes {
    /// A warning.
    pub fn push(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// A problem: the result is partial.
    pub fn problem(&mut self, msg: impl Into<String>) {
        self.problems.push(msg.into());
    }

    /// A part ended early: always a problem.
    pub fn stop(&mut self, stop: &xml::Stop, part: &str) {
        self.problem(stop.describe(part));
    }

    /// Moves warnings in.
    pub fn append(&mut self, v: &mut Vec<String>) {
        self.warnings.append(v);
    }
}

/// Children of the root element with their text: `(local name, text)`. At most 1000
/// fields of at most 64 KiB each (`docProps/core.xml`, `docProps/app.xml`).
pub fn root_fields(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Notes,
) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let Some(reader) = pkg.open_part(part) else {
        return fields;
    };
    let mut depth = 0usize;
    let mut cur: Option<(String, String)> = None;
    let r = walk(reader, out, |ev| {
        match ev {
            Ev::Start(e) => {
                depth += 1;
                if depth == 2 {
                    cur = Some((name(e).to_owned(), String::new()));
                }
            }
            Ev::Text(t) => {
                if depth == 2
                    && let Some((_, s)) = cur.as_mut()
                    && s.len() < 64 << 10
                {
                    s.push_str(t);
                }
            }
            Ev::End(_) => {
                if depth == 2
                    && let Some(f) = cur.take()
                    && fields.len() < 1000
                {
                    fields.push(f);
                }
                depth = depth.saturating_sub(1);
            }
            Ev::Empty(_) => {}
        }
        Flow::Go
    });
    if let Err(stop) = r {
        warn.stop(&stop, part);
    }
    fields
}

/// The calendar date of a W3CDTF value (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`, optionally
/// followed by `Thh:mm...`). Missing month or day count as 1.
pub fn parse_date(v: &str) -> Option<Date> {
    let v = v.trim();
    let date = v.split(['T', ' ']).next()?;
    let mut it = date.split('-');
    let y: i16 = it.next().filter(|y| y.len() == 4)?.parse().ok()?;
    let m: u8 = match it.next() {
        Some(m) if m.len() == 2 => m.parse().ok()?,
        Some(_) => return None,
        None => 1,
    };
    let d: u8 = match it.next() {
        Some(d) if d.len() == 2 => d.parse().ok()?,
        Some(_) => return None,
        None => 1,
    };
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(Date { y, m, d })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaning() {
        assert_eq!(clean("  a \u{1}\n b  "), "a b");
    }

    #[test]
    fn dates() {
        assert_eq!(
            parse_date("2021-05-04T08:00:00Z"),
            Some(Date {
                y: 2021,
                m: 5,
                d: 4
            })
        );
        assert_eq!(
            parse_date("2021"),
            Some(Date {
                y: 2021,
                m: 1,
                d: 1
            })
        );
        assert_eq!(parse_date("2021-13-01"), None);
        assert_eq!(parse_date("yesterday"), None);
    }
}

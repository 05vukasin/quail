//! Page-of-page markers ("Strana 2 od 5", "Page 2 of 5") and version markers ("v2",
//! "rev. 3", "DRAFT", "Execution version", "Redline").

use quail_core::Zone;

use crate::patterns::Patterns;
use crate::prep::Prepared;
use crate::scan::{FOOT_LINES, HEAD_LINES};

/// A page marker: page `n` of `of`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageMarker {
    pub n: i64,
    pub of: i64,
    pub ns: usize,
    pub ne: usize,
    pub labelled: bool,
}

/// The first page marker of a page (labelled ones win over bare "2 of 5" lines).
pub(crate) fn page_marker(p: &Patterns, pp: &Prepared<'_>) -> Option<PageMarker> {
    let pick = |n: Option<regex::Match<'_>>, m: Option<regex::Match<'_>>| -> Option<(i64, i64)> {
        let n: i64 = n?.as_str().parse().ok()?;
        let m: i64 = m?.as_str().parse().ok()?;
        (n >= 1 && n <= m).then_some((n, m))
    };
    for c in p.page_marker.captures_iter(&pp.keys) {
        let Some(whole) = c.get(0) else { continue };
        let Some((n, of)) = pick(c.name("n"), c.name("m1").or_else(|| c.name("m2"))) else {
            continue;
        };
        if let Some(r) = range(pp, whole.start(), whole.end()) {
            return Some(PageMarker {
                n,
                of,
                ns: r.0,
                ne: r.1,
                labelled: true,
            });
        }
    }
    for c in p.bare_marker.captures_iter(&pp.keys) {
        let Some(whole) = c.get(0) else { continue };
        let Some((n, of)) = pick(c.name("n"), c.name("m")) else {
            continue;
        };
        let Some(r) = range(pp, whole.start(), whole.end()) else {
            continue;
        };
        let Some(t) = pp.tok_starting(whole.start()) else {
            continue;
        };
        let zone = pp.zone_at(r.0);
        let edge = matches!(zone, Zone::Header | Zone::Footer)
            || pp.line(t) < 3
            || pp.line(t) + FOOT_LINES >= pp.lines;
        if edge {
            return Some(PageMarker {
                n,
                of,
                ns: r.0,
                ne: r.1,
                labelled: false,
            });
        }
    }
    None
}

/// Normalized range of key bytes `ks..ke`; `None` unless both ends are token ends (a match
/// inside a longer token is no marker).
fn range(pp: &Prepared<'_>, ks: usize, ke: usize) -> Option<(usize, usize)> {
    let a = pp.tok_starting(ks)?;
    let b = pp.tok_ending(ke)?;
    Some((pp.toks[a].ns, pp.toks[b].ne))
}

/// A version marker.
#[derive(Clone, Debug)]
pub(crate) struct VersionHint {
    /// `version`, `revision`, `draft`, `final`, `execution`, `redline` or `clean`.
    pub kind: &'static str,
    /// The number of a `version` / `revision` ("2", "1.3").
    pub n: Option<String>,
    pub ns: usize,
    pub ne: usize,
    /// In a header, footer, title zone or a head line, rather than a line of its own elsewhere.
    pub strong: bool,
}

/// Version markers of a page, only where a marker is written rather than a word used:
///
/// - in a header, footer or title zone (when the extractor gives zones);
/// - on a head line of the first page with at most eight tokens ("DRAFT", "Execution
///   Version", "MASTER SERVICES AGREEMENT v2");
/// - on a line of its own ("V1.0", "Draft"; a numbered marker may carry one more token,
///   "Rev. 3 12.03.2023").
///
/// Prose ("the final payment", "NIST SP 800-171, Rev 2", "Final Audit Report") and codes
/// ("ABC-DB-R100-V2") stay out.
pub(crate) fn version_hints(p: &Patterns, pp: &Prepared<'_>) -> Vec<VersionHint> {
    let mut out = Vec::new();
    for c in p.version.captures_iter(&pp.keys) {
        let Some(whole) = c.get(0) else { continue };
        let (Some(a), Some(b)) = (pp.tok_starting(whole.start()), pp.tok_ending(whole.end()))
        else {
            continue;
        };
        // Glued into a code: "ABC-DB-R100-S-4-V2", "file_v2".
        let before = pp
            .norm
            .get(..pp.toks[a].ns)
            .and_then(|t| t.chars().next_back());
        if before.is_some_and(|c| matches!(c, '-' | '_' | '/' | '.')) {
            continue;
        }
        let zone = pp.zone_at(pp.toks[a].ns);
        let (la, lb) = pp.line_bounds(a);
        let zoned = matches!(zone, Zone::Header | Zone::Footer);
        let head =
            (zone == Zone::Title || (pp.page.index == 0 && pp.line(a) < HEAD_LINES)) && lb - la < 8;
        let numbered = c.name("w").is_none();
        let alone = la == a && (lb == b || (numbered && lb == b + 1));
        // On a head line a single marker word counts in capitals ("DRAFT") only: "Final SAP"
        // is a deliverable.
        let caps = pp
            .text(a)
            .chars()
            .all(|c| !c.is_alphabetic() || c.is_uppercase());
        let head = head && (numbered || caps || b > a);
        if !(zoned || head || alone) {
            continue;
        }
        let (kind, n) = if let Some(w) = c.name("w") {
            (word_kind(w.as_str()), None)
        } else {
            let kw = c.name("kw").map_or("", |m| m.as_str());
            let n = c
                .name("n")
                .or_else(|| c.name("n2"))
                .or_else(|| c.name("n3"));
            let revision = kw.starts_with("rev") || c.name("n3").is_some();
            (
                if revision { "revision" } else { "version" },
                n.map(|m| m.as_str().to_owned()),
            )
        };
        out.push(VersionHint {
            kind,
            n,
            ns: pp.toks[a].ns,
            ne: pp.toks[b].ne,
            strong: zoned || head,
        });
    }
    out
}

fn word_kind(w: &str) -> &'static str {
    if w.contains("draft") || w == "nacrt" {
        "draft"
    } else if w.starts_with("execut") {
        "execution"
    } else if w.starts_with("redline") || w == "blackline" {
        "redline"
    } else if w.starts_with("clean") {
        "clean"
    } else {
        "final"
    }
}

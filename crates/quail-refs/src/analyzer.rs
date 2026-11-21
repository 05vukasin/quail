//! `ref.references@1`.

use quail_core::{
    Analyzer, AnalyzerId, Ctx, Date, Document, Emitter, Evidence, Profile, Result, Value,
};

use crate::keys;
use crate::kind::{Kind, Relation, record};
use crate::markers::{page_marker, version_hints};
use crate::number::{clm_record_ids, study_codes, without_version_tail};
use crate::patterns::Patterns;
use crate::prep::Prepared;
use crate::scan::{Mention, Number, generic_numbers, mentions};
use crate::title;

/// Distinct own numbers kept per document.
pub const MAX_DOC_NUMBERS: usize = 16;
/// Distinct cited documents kept per document.
pub const MAX_CITES: usize = 64;
/// Distinct version markers kept per document.
pub const MAX_VERSION_HINTS: usize = 16;
/// Distinct study codes and CLM record ids kept per document (each).
pub const MAX_IDS: usize = 32;

/// Own numbers, cited documents, title, instrument kind, ordinal, page and version markers.
///
/// Requires the `document` artifact, runs in [`Profile::Fast`]. See the crate docs for the
/// keys and the record shapes. Reads at most `Limits::max_text_bytes` of text, checks the
/// deadline per page, skips pages flagged `needs_ocr`, and records a problem when a cap or the
/// text limit cuts the output short.
#[derive(Debug)]
pub struct RefAnalyzer {
    pats: Patterns,
}

impl Default for RefAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl RefAnalyzer {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("ref.references", 2);

    /// Compiles the patterns (a few milliseconds); build once and share.
    pub fn new() -> Self {
        Self {
            pats: Patterns::new(),
        }
    }
}

impl Analyzer for RefAnalyzer {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn after(&self) -> &'static [&'static str] {
        &["revised_document"]
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.document() else {
            return Ok(());
        };
        let budget = usize::try_from(ctx.limits.max_text_bytes).unwrap_or(usize::MAX);
        run_document(&self.pats, doc, budget, out)
    }
}

fn within(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Clone)]
struct Own {
    kind: Option<Kind>,
    number: Number,
    date: Option<Date>,
    conf: f32,
    ev: Evidence,
}

#[derive(Clone)]
struct Cite {
    kind: Kind,
    relation: Relation,
    number: Option<Number>,
    ordinal: Option<i64>,
    date: Option<Date>,
    conf: f32,
    ev: Evidence,
}

#[derive(Default)]
struct Acc {
    own: Vec<Own>,
    cites: Vec<Cite>,
    versions: Vec<(crate::markers::VersionHint, Evidence)>,
    studies: Vec<(String, Evidence)>,
    clms: Vec<(String, Evidence)>,
    ordinal: Option<(i64, f32, Evidence)>,
    capped: [bool; 5],
}

fn run_document(p: &Patterns, doc: &Document, mut budget: usize, out: &mut Emitter) -> Result<()> {
    // The first two pages are prepared first: they hold the title, which decides how every
    // number is read (own or cited). Later pages are prepared one at a time.
    let mut first: Vec<Prepared<'_>> = Vec::new();
    let mut stopped = None;
    for page in doc.pages.iter().take(2) {
        out.check_deadline()?;
        if page.needs_ocr {
            continue;
        }
        if budget == 0 {
            stopped = Some(page.index);
            break;
        }
        let text = within(&page.text, budget);
        budget -= text.len();
        if text.len() < page.text.len() {
            stopped = Some(page.index);
        }
        first.push(Prepared::new(page, text));
    }
    let title = title::find(p, &first);
    let mut acc = Acc::default();
    let mut doc_kind = title.as_ref().and_then(|t| t.kind.map(|k| k.0));
    let mut kind_conf = title
        .as_ref()
        .and_then(|t| t.kind.map(|k| k.1))
        .unwrap_or(0.0);
    let mut kind_ev = title
        .as_ref()
        .map(|t| first[t.page].evidence_toks(t.a, t.b));
    if let Some(t) = &title {
        let pp = &first[t.page];
        out.push(
            keys::TITLE,
            Value::text(t.text.as_str()),
            t.conf,
            Some(pp.evidence_toks(t.a, t.b)),
        );
        if let Some((n, a, b)) = t.ordinal {
            acc.ordinal = Some((n, (t.conf * 0.95).min(0.9), pp.evidence_toks(a, b)));
        }
    }
    let first_mentions: Vec<Vec<Mention>> = first.iter().map(|pp| mentions(p, pp)).collect();
    if doc_kind.is_none()
        && let Some((pi, m)) = first_mentions
            .iter()
            .enumerate()
            .flat_map(|(i, ms)| ms.iter().map(move |m| (i, m)))
            .find(|(_, m)| inferable(m))
    {
        doc_kind = Some(m.kind);
        kind_conf = 0.5;
        kind_ev = Some(first[pi].evidence(m.ns, m.ne));
    }
    if let (Some(k), Some(ev)) = (doc_kind, kind_ev) {
        out.push(
            keys::INSTRUMENT_KIND,
            Value::text(k.as_str()),
            kind_conf,
            Some(ev),
        );
    }
    let inferred = title.as_ref().and_then(|t| t.kind).is_none();
    let ctx = Classify { doc_kind, inferred };
    for (pp, ms) in first.iter().zip(&first_mentions) {
        page_pass(p, pp, ms, &ctx, &mut acc, out);
    }
    drop(first);
    if stopped.is_none() {
        for page in doc.pages.iter().skip(2) {
            out.check_deadline()?;
            if page.needs_ocr {
                continue;
            }
            if budget == 0 {
                stopped = Some(page.index);
                break;
            }
            let text = within(&page.text, budget);
            budget -= text.len();
            if text.len() < page.text.len() {
                stopped = Some(page.index);
            }
            let pp = Prepared::new(page, text);
            let ms = mentions(p, &pp);
            page_pass(p, &pp, &ms, &ctx, &mut acc, out);
        }
    }
    if let Some(i) = stopped {
        out.problem(format!(
            "ref.references: text limit reached, page {i} and later not read in full"
        ));
    }
    emit(acc, out);
    Ok(())
}

/// Whether a head mention on the first pages can tell the document's kind when its title
/// does not.
fn inferable(m: &Mention) -> bool {
    m.head
        && m.trigger.is_none()
        && !m.self_ref
        && !matches!(m.kind, Kind::Annex | Kind::Procurement | Kind::Registry)
        && m.number.as_ref().is_some_and(|n| !n.key.is_empty())
}

struct Classify {
    doc_kind: Option<Kind>,
    inferred: bool,
}

fn page_pass(
    p: &Patterns,
    pp: &Prepared<'_>,
    ms: &[Mention],
    c: &Classify,
    acc: &mut Acc,
    out: &mut Emitter,
) {
    for m in ms {
        classify(pp, m, c, acc);
    }
    for (n, registry) in generic_numbers(p, pp) {
        if acc.own.iter().any(|o| o.number.key == n.key)
            || ms
                .iter()
                .any(|m| m.number.as_ref().is_some_and(|x| x.ns == n.ns))
        {
            continue;
        }
        let ev = pp.evidence(n.ns, n.ne);
        add_own(
            acc,
            Own {
                kind: if registry {
                    Some(Kind::Registry)
                } else {
                    c.doc_kind
                },
                number: n,
                date: None,
                conf: if registry { 0.6 } else { 0.45 },
                ev,
            },
        );
    }
    if let Some(mk) = page_marker(p, pp) {
        let v = record(vec![("n", Value::Int(mk.n)), ("of", Value::Int(mk.of))]);
        let conf = if mk.labelled { 0.9 } else { 0.7 };
        out.push(keys::PAGE_MARKER, v, conf, Some(pp.evidence(mk.ns, mk.ne)));
    }
    for h in version_hints(p, pp) {
        let same = acc
            .versions
            .iter()
            .position(|(x, _)| x.kind == h.kind && x.n == h.n);
        match same {
            Some(i) if !acc.versions[i].0.strong && h.strong => {
                let ev = pp.evidence(h.ns, h.ne);
                acc.versions[i] = (h, ev);
            }
            Some(_) => {}
            None if acc.versions.len() >= MAX_VERSION_HINTS => acc.capped[2] = true,
            None => {
                let ev = pp.evidence(h.ns, h.ne);
                acc.versions.push((h, ev));
            }
        }
    }
    for (s, e, code) in study_codes(&pp.norm) {
        push_id(
            &mut acc.studies,
            &mut acc.capped[3],
            code,
            pp.evidence(s, e),
        );
    }
    for (s, e, id) in clm_record_ids(&pp.norm) {
        push_id(
            &mut acc.clms,
            &mut acc.capped[4],
            id.to_ascii_lowercase(),
            pp.evidence(s, e),
        );
    }
}

fn push_id(list: &mut Vec<(String, Evidence)>, capped: &mut bool, id: String, ev: Evidence) {
    if list.iter().any(|(x, _)| *x == id) {
        return;
    }
    if list.len() >= MAX_IDS {
        *capped = true;
        return;
    }
    list.push((id, ev));
}

#[allow(clippy::too_many_lines)]
fn classify(pp: &Prepared<'_>, m: &Mention, c: &Classify, acc: &mut Acc) {
    let ev = pp.evidence(m.ns, m.ne);
    let kind = effective_kind(m);
    let key = m.number.as_ref().map_or("", |n| n.key.as_str());
    let has_key = !key.is_empty();
    let fits = c.doc_kind.map(|d| kind.fits(d));
    let unsure = if c.doc_kind.is_none() {
        0.1
    } else if c.inferred {
        0.05
    } else {
        0.0
    };
    if m.self_ref {
        if has_key && let Some(n) = &m.number {
            let conf = own_conf(m) - unsure;
            add_own(
                acc,
                Own {
                    kind: Some(kind),
                    number: n.clone(),
                    date: m.date,
                    conf,
                    ev,
                },
            );
        }
        return;
    }
    if let Some(rel) = m.trigger {
        let relation = if rel == Relation::References {
            Relation::default_for(c.doc_kind, kind)
        } else {
            rel
        };
        let explicit = rel != Relation::References;
        let parentish = matches!(
            kind,
            Kind::Msa
                | Kind::Sow
                | Kind::Po
                | Kind::Nda
                | Kind::AmendedRestated
                | Kind::FrameworkAgreement
        );
        let base = if has_key {
            0.8
        } else if m.date.is_some() {
            0.65
        } else if m.ordinal.is_some() && kind.ordinal_citable() {
            0.55
        } else if explicit
            && parentish
            && fits != Some(true)
            && (matches!(rel, Relation::Amends | Relation::AnnexTo)
                || Relation::default_for(c.doc_kind, kind) != Relation::References)
        {
            // A bare "under the Statement of Work" in an MSA names no document; in a child
            // of that kind it names its parent.
            0.4
        } else {
            return;
        };
        let conf = base + if explicit { 0.05 } else { 0.0 } - unsure;
        add_cite(acc, cite(m, kind, relation, conf, ev));
        return;
    }
    let relation = Relation::default_for(c.doc_kind, kind);
    if has_key {
        // A SOW or order prints the CLM contract id it is filed under as "Contract ID" in its
        // head: that is its own number, not its parent's.
        let filed_id = kind == Kind::Contract
            && m.label
            && m.head
            && matches!(c.doc_kind, Some(Kind::Sow | Kind::Order));
        let own = match fits {
            Some(true) => true,
            Some(false) => filed_id,
            None => m.own_region,
        };
        if own && let Some(n) = &m.number {
            let conf = own_conf(m) - unsure;
            add_own(
                acc,
                Own {
                    kind: Some(kind),
                    number: n.clone(),
                    date: m.date,
                    conf,
                    ev,
                },
            );
        } else {
            let conf = if fits == Some(true) { 0.45 } else { 0.75 } - unsure;
            add_cite(acc, cite(m, kind, relation, conf, ev));
        }
        return;
    }
    if let Some(o) = m.ordinal {
        if fits == Some(true) {
            if m.head && acc.ordinal.is_none() {
                acc.ordinal = Some((o, 0.6, ev));
            }
        } else if fits == Some(false) && kind.ordinal_citable() {
            add_cite(acc, cite(m, kind, relation, 0.5 - unsure, ev));
        }
        return;
    }
    if let Some(n) = &m.number {
        if fits == Some(true) && m.own_region && m.label {
            let conf = own_conf(m) * 0.5;
            add_own(
                acc,
                Own {
                    kind: Some(kind),
                    number: n.clone(),
                    date: m.date,
                    conf,
                    ev,
                },
            );
        }
        return;
    }
    let parent = matches!(
        kind,
        Kind::Msa
            | Kind::Contract
            | Kind::Nda
            | Kind::Sow
            | Kind::Po
            | Kind::Order
            | Kind::AmendedRestated
            | Kind::FrameworkAgreement
    );
    let child_doc = matches!(
        c.doc_kind,
        Some(
            Kind::Amendment
                | Kind::Annex
                | Kind::ChangeOrder
                | Kind::SideLetter
                | Kind::AmendedRestated
                | Kind::Invoice
                | Kind::Sow
                | Kind::Po
                | Kind::Order
        )
    );
    if m.date.is_some() && m.definite && parent && (fits == Some(false) || child_doc) {
        add_cite(acc, cite(m, kind, relation, 0.6 - unsure, ev));
    }
}

/// The kind a generic "Contract ID: 6619/AMD/QX" really names: identifiers often carry the
/// instrument's type code (`AMD`, `CR`, `SOW`, `PO`, `INV`, `MSA`).
fn effective_kind(m: &Mention) -> Kind {
    if m.kind != Kind::Contract {
        return m.kind;
    }
    let Some(n) = &m.number else {
        return m.kind;
    };
    n.raw
        .split(|c: char| !c.is_alphanumeric())
        .find_map(|seg| match seg.to_ascii_uppercase().as_str() {
            "AMD" | "AMND" | "AMEND" => Some(Kind::Amendment),
            "CR" | "CCN" => Some(Kind::ChangeOrder),
            "SOW" | "WO" => Some(Kind::Sow),
            "PO" => Some(Kind::Po),
            "INV" => Some(Kind::Invoice),
            "MSA" => Some(Kind::Msa),
            _ => None,
        })
        .unwrap_or(m.kind)
}

fn own_conf(m: &Mention) -> f32 {
    let mut base: f32 = if m.head {
        0.85
    } else if m.own_region {
        0.75
    } else if m.first_page {
        0.6
    } else {
        0.45
    };
    if !m.label {
        base -= 0.1;
    }
    base
}

fn cite(m: &Mention, kind: Kind, relation: Relation, conf: f32, ev: Evidence) -> Cite {
    Cite {
        kind,
        relation,
        number: m.number.clone().filter(|n| !n.key.is_empty()),
        ordinal: m.ordinal,
        date: m.date,
        conf: conf.clamp(0.05, 0.95),
        ev,
    }
}

fn add_own(acc: &mut Acc, o: Own) {
    let id = |x: &Own| {
        if x.number.key.is_empty() {
            x.number.raw.clone()
        } else {
            x.number.key.clone()
        }
    };
    let oid = id(&o);
    if let Some(i) = acc.own.iter().position(|x| id(x) == oid) {
        let x = &mut acc.own[i];
        // Seen again: more support, and the better-placed sighting is kept.
        let conf = (x.conf.max(o.conf) + 0.05).min(0.95);
        if o.conf > x.conf {
            *x = o;
        }
        x.conf = conf;
        return;
    }
    if acc.own.len() >= MAX_DOC_NUMBERS {
        acc.capped[0] = true;
        return;
    }
    acc.own.push(Own {
        conf: o.conf.clamp(0.05, 0.95),
        ..o
    });
}

fn add_cite(acc: &mut Acc, c: Cite) {
    let same = |x: &Cite| {
        x.kind == c.kind
            && x.number.as_ref().map(|n| &n.key) == c.number.as_ref().map(|n| &n.key)
            && x.ordinal == c.ordinal
            && (x.date == c.date || x.date.is_none() || c.date.is_none())
    };
    if let Some(i) = acc.cites.iter().position(same) {
        let x = &mut acc.cites[i];
        if x.relation == Relation::References && c.relation != Relation::References {
            x.relation = c.relation;
        }
        if x.date.is_none() {
            x.date = c.date;
        }
        x.conf = (x.conf.max(c.conf) + 0.02).min(0.95);
        return;
    }
    if acc.cites.len() >= MAX_CITES {
        acc.capped[1] = true;
        return;
    }
    acc.cites.push(c);
}

fn date_value(d: Date) -> Value {
    Value::Date(d)
}

fn number_fields(fields: &mut Vec<(&'static str, Value)>, n: &Number) {
    fields.push(("raw", Value::text(n.raw.as_str())));
    if !n.key.is_empty() {
        fields.push(("key", Value::text(n.key.as_str())));
        let base = without_version_tail(&n.key);
        if base != n.key && !base.is_empty() {
            fields.push(("base", Value::text(base)));
        }
    }
}

fn emit(acc: Acc, out: &mut Emitter) {
    for o in acc.own {
        let mut f = vec![("kind", Value::text(o.kind.map_or("unknown", Kind::as_str)))];
        number_fields(&mut f, &o.number);
        if let Some(d) = o.date {
            f.push(("date", date_value(d)));
        }
        out.push(keys::DOC_NUMBER, record(f), o.conf, Some(o.ev));
    }
    for c in acc.cites {
        let mut f = vec![
            ("kind", Value::text(c.kind.as_str())),
            ("relation", Value::text(c.relation.as_str())),
        ];
        if let Some(n) = &c.number {
            number_fields(&mut f, n);
        }
        if let Some(o) = c.ordinal {
            f.push(("ordinal", Value::Int(o)));
        }
        if let Some(d) = c.date {
            f.push(("date", date_value(d)));
        }
        out.push(keys::CITES, record(f), c.conf, Some(c.ev));
    }
    if let Some((n, conf, ev)) = acc.ordinal {
        out.push(keys::ORDINAL, Value::Int(n), conf, Some(ev));
    }
    for (h, ev) in acc.versions {
        let mut f = vec![("kind", Value::text(h.kind))];
        if let Some(n) = h.n {
            f.push(("n", Value::text(n)));
        }
        let conf = if h.strong { 0.75 } else { 0.5 };
        out.push(keys::VERSION_HINT, record(f), conf, Some(ev));
    }
    for (s, ev) in acc.studies {
        out.push(keys::STUDY_CODE, Value::text(s), 0.9, Some(ev));
    }
    for (s, ev) in acc.clms {
        out.push(keys::CLM_RECORD, Value::text(s), 0.9, Some(ev));
    }
    let names = [
        "ref.doc_number",
        "ref.cites",
        "ref.version_hint",
        "ref.study_code",
        "ref.clm_record",
    ];
    let caps = [
        MAX_DOC_NUMBERS,
        MAX_CITES,
        MAX_VERSION_HINTS,
        MAX_IDS,
        MAX_IDS,
    ];
    for i in 0..5 {
        if acc.capped[i] {
            out.problem(format!(
                "ref.references: more than {} distinct {} values, the rest dropped",
                caps[i], names[i]
            ));
        }
    }
}

//! `party.candidates@1`: turns page scans into facts.

use std::collections::HashMap;

use quail_core::{
    Analyzer, AnalyzerId, Ctx, Document, Emitter, Evidence, Profile, Result, Value, Zone,
};

use crate::keys;
use crate::names::{core_key, key_tokens, normalized_key};
use crate::quality;
use crate::roles::Role;
use crate::scan::{Mention, PagePos, PageScan, domain_labels, scan};
use crate::vocab;
use crate::words::PageWords;

/// Mentions emitted per core key (the strongest ones).
pub const MAX_MENTIONS_PER_KEY: usize = 3;
/// Core keys emitted per document.
pub const MAX_KEYS: usize = 32;
/// Name statements emitted per document.
pub const MAX_STATEMENTS: usize = 16;
/// Signature titles emitted per document.
pub const MAX_TITLES: usize = 8;
/// Entries of `party.candidates`.
pub const MAX_RANKED: usize = 10;
/// Text read per document, whatever `Limits::max_text_bytes` allows.
pub const MAX_SCAN_BYTES: usize = 16 << 20;

/// Company and role candidates of a document. See the crate documentation for the keys and
/// the confidence table.
#[derive(Clone, Copy, Debug, Default)]
pub struct PartyCandidates;

impl PartyCandidates {
    /// Analyzer id.
    pub const ID: AnalyzerId = AnalyzerId::new("party.candidates", 1);
}

impl Analyzer for PartyCandidates {
    fn id(&self) -> AnalyzerId {
        Self::ID
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn profile(&self) -> Profile {
        Profile::Fast
    }

    fn run(&self, ctx: &Ctx<'_>, out: &mut Emitter) -> Result<()> {
        let Some(doc) = ctx.get::<Document>() else {
            return Ok(());
        };
        let budget = usize::try_from(ctx.limits.max_text_bytes).unwrap_or(usize::MAX);
        let found = collect(doc, budget.min(MAX_SCAN_BYTES), out);
        emit(&found, out);
        Ok(())
    }
}

/// One mention, resolved to text and evidence.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Surface form (normalized whitespace, quotes and dashes).
    pub surface: String,
    /// Core key.
    pub key: String,
    /// Normalized name.
    pub norm: String,
    /// Canonical legal form.
    pub form: Option<&'static str>,
    /// Jurisdiction hint and its confidence.
    pub jurisdiction: Option<(String, f32)>,
    /// `ok` or `suspect`.
    pub quality: &'static str,
    /// Roles with their confidence.
    pub roles: Vec<(Role, f32)>,
    /// Defined term.
    pub term: Option<String>,
    /// Confidence.
    pub conf: f32,
    /// Evidence.
    pub evidence: Evidence,
}

/// A name statement resolved to text.
#[derive(Clone, Debug)]
pub struct AliasPair {
    /// Current or legal name.
    pub current: String,
    /// Former or trading name.
    pub other: String,
    /// `former_name`, `trading_name`, `dba`.
    pub kind: &'static str,
    /// Evidence covering both names.
    pub evidence: Evidence,
}

/// Everything found in a document.
#[derive(Default)]
pub struct Found {
    /// All accepted mentions, in document order.
    pub candidates: Vec<Candidate>,
    /// Name statements.
    pub aliases: Vec<AliasPair>,
    /// Signature titles with evidence.
    pub titles: Vec<(String, Evidence)>,
    /// Lines of signature blocks naming a signatory.
    pub signatory_lines: usize,
    /// Mentions the per-page cap dropped.
    pub dropped: usize,
}

/// Scans every page of `doc` within `budget` bytes of text.
pub fn collect(doc: &Document, budget: usize, out: &mut Emitter) -> Found {
    let mut found = Found::default();
    let mut left = budget;
    let n = doc.pages.len();
    let has_zones = doc
        .pages
        .iter()
        .any(|p| p.blocks.iter().any(|b| b.zone != Zone::Unknown));
    let domains = document_domains(doc, budget);
    for (i, page) in doc.pages.iter().enumerate() {
        if out.check_deadline().is_err() {
            out.problem(format!(
                "party: deadline reached, pages from {} not scanned",
                page.index
            ));
            break;
        }
        if left == 0 {
            out.problem(format!(
                "party: text limit reached, pages from {} not scanned",
                page.index
            ));
            break;
        }
        if page.needs_ocr || page.text.trim().is_empty() {
            continue;
        }
        let w = PageWords::new(page, left);
        if page.text.len() > left {
            out.problem(format!(
                "party: text limit reached inside page {}",
                page.index
            ));
        }
        left = left.saturating_sub(page.text.len());
        let pos = PagePos {
            first: i == 0,
            near_end: i + 2 >= n,
            has_zones,
        };
        let s = scan(&w, pos, &domains);
        absorb(&mut found, &w, &s, page.index);
    }
    if found.dropped > 0 {
        out.problem(format!(
            "party: {} mentions over the per-page cap not scanned",
            found.dropped
        ));
    }
    found
}

/// Domain labels of the whole document (within the scan budget), at most 16.
fn document_domains(doc: &Document, budget: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut left = budget;
    for p in &doc.pages {
        if left == 0 || out.len() >= 16 {
            break;
        }
        let mut end = left.min(p.text.len());
        while !p.text.is_char_boundary(end) {
            end -= 1;
        }
        for l in domain_labels(&p.text[..end], 16) {
            if !out.contains(&l) && out.len() < 16 {
                out.push(l);
            }
        }
        left -= end;
    }
    out
}

fn evidence(w: &PageWords, page: u32, a: usize, b: usize, zone: Zone) -> Evidence {
    Evidence {
        page: Some(page),
        zone,
        span: w.source_span(a, b),
        bbox: None,
    }
}

fn absorb(found: &mut Found, w: &PageWords, s: &PageScan, page: u32) {
    let mut resolved: Vec<Option<String>> = Vec::with_capacity(s.mentions.len());
    for m in &s.mentions {
        let c = candidate(w, m, page);
        resolved.push(c.as_ref().map(|c| c.surface.clone()));
        if let Some(c) = c {
            found.candidates.push(c);
        }
    }
    for st in &s.statements {
        let (Some(Some(cur)), Some(Some(oth))) = (resolved.get(st.current), resolved.get(st.other))
        else {
            continue;
        };
        if core_key(cur) == core_key(oth) {
            continue;
        }
        found.aliases.push(AliasPair {
            current: cur.clone(),
            other: oth.clone(),
            kind: st.kind,
            evidence: evidence(w, page, st.a, st.b, s.mentions[st.current].zone),
        });
    }
    for t in &s.titles {
        found
            .titles
            .push((t.text.clone(), evidence(w, page, t.a, t.b, Zone::Signature)));
    }
    found.signatory_lines += s.signatory_lines;
    found.dropped += s.dropped;
}

/// Base confidence with a strong legal form.
pub const BASE_STRONG: f32 = 0.75;
/// Base confidence with a weak legal form (`AS`, `AD`, `Co.`, `Group`, ...).
pub const BASE_WEAK: f32 = 0.40;
/// Base confidence of a formless name spelling a domain of the document.
pub const BASE_DOMAIN: f32 = 0.65;
/// Base confidence of a formless name across a name statement.
pub const BASE_STATEMENT: f32 = 0.6;
/// Base confidence of a formless name after a role label.
pub const BASE_LABEL: f32 = 0.55;

/// Confidence of a mention: see the crate documentation's table.
pub fn mention_conf(m: &Mention) -> f32 {
    let mut c: f32 = match m.form {
        Some(f) if f.weak => BASE_WEAK,
        Some(_) => BASE_STRONG,
        None if m.ctx.domain => BASE_DOMAIN,
        None if m.from_statement => BASE_STATEMENT,
        None => BASE_LABEL,
    };
    if m.ctx.clause {
        c += 0.12;
    }
    match &m.term {
        Some((_, Some(_))) => c += 0.10,
        Some((_, None)) => c += 0.05,
        None => {}
    }
    if m.ctx.header {
        c += 0.06;
    } else if m.ctx.footer {
        c += 0.03;
    }
    if m.ctx.signature {
        c += 0.15;
    }
    if m.ctx.label && m.form.is_some() {
        c += 0.3;
    }
    if m.ctx.title {
        c += 0.05;
    }
    if m.ctx.bank {
        c -= 0.3;
    }
    c.clamp(0.05, 0.97)
}

/// How sure a role is, given the mention is a company.
fn role_weight(m: &Mention, r: Role) -> f32 {
    let from_term = m.term.as_ref().is_some_and(|(_, tr)| *tr == Some(r));
    match r {
        _ if from_term => 0.95,
        Role::BillTo | Role::ShipTo => 0.95,
        Role::Supplier | Role::Customer | Role::Party => 0.9,
        Role::Signatory => 0.85,
        Role::Issuer => {
            if m.ctx.header {
                0.8
            } else {
                0.6
            }
        }
        Role::Mentioned => 1.0,
    }
}

fn jurisdiction(m: &Mention, surface: &str, tokens: &[String]) -> Option<(String, f32)> {
    if let Some(c) = tokens
        .iter()
        .find_map(|t| vocab::lookup(vocab::GEO_COUNTRY, t))
    {
        return Some((c.to_owned(), 0.85));
    }
    if let Some(c) = m.seat {
        return Some((c.to_owned(), 0.9));
    }
    let f = m.form?;
    let cyrillic = surface
        .chars()
        .any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
    if cyrillic && f.jurisdictions.contains(&"rs") {
        return Some(("rs".to_owned(), 0.8));
    }
    match f.jurisdictions.len() {
        0 => None,
        1 => Some((f.jurisdictions[0].to_owned(), 0.8)),
        n if n <= 5 => Some((f.jurisdictions.join("|"), 0.5)),
        _ => None,
    }
}

fn candidate(w: &PageWords, m: &Mention, page: u32) -> Option<Candidate> {
    let surface = w.slice(m.a, m.b).trim().to_owned();
    let key = core_key(w.slice(m.a, m.name_end));
    if key.is_empty() {
        return None;
    }
    let q = quality::verdict(&surface).ok()?;
    let conf = mention_conf(m);
    let tokens = key_tokens(&surface);
    let roles = m
        .roles
        .iter()
        .map(|&r| (r, conf * role_weight(m, r)))
        .collect();
    Some(Candidate {
        norm: normalized_key(&surface),
        form: m.form.map(|f| f.canon),
        jurisdiction: jurisdiction(m, &surface, &tokens),
        quality: q.as_str(),
        roles,
        term: m.term.as_ref().map(|(t, _)| t.clone()),
        conf,
        evidence: evidence(w, page, m.a, m.b, m.zone),
        surface,
        key,
    })
}

/// Per key: its mentions (indices into `candidates`, strongest first) and its score
/// (noisy-or of every mention's confidence).
pub fn rank(candidates: &[Candidate]) -> Vec<(String, Vec<usize>, f32)> {
    let mut groups: Vec<(String, Vec<usize>, f32)> = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        let g = *at.entry(c.key.as_str()).or_insert_with(|| {
            groups.push((c.key.clone(), Vec::new(), 1.0));
            groups.len() - 1
        });
        groups[g].1.push(i);
        groups[g].2 *= 1.0 - c.conf;
    }
    for g in &mut groups {
        g.2 = (1.0 - g.2).min(0.99);
        g.1.sort_by(|&x, &y| candidates[y].conf.total_cmp(&candidates[x].conf));
    }
    groups.sort_by(|x, y| y.2.total_cmp(&x.2));
    groups
}

fn emit(found: &Found, out: &mut Emitter) {
    let groups = rank(&found.candidates);
    if groups.len() > MAX_KEYS {
        out.problem(format!(
            "party: {} candidate companies, the {} strongest emitted",
            groups.len(),
            MAX_KEYS
        ));
    }
    let mut capped = 0;
    for (_, idx, _) in groups.iter().take(MAX_KEYS) {
        capped += idx.len().saturating_sub(MAX_MENTIONS_PER_KEY);
        for &i in idx.iter().take(MAX_MENTIONS_PER_KEY) {
            emit_candidate(&found.candidates[i], out);
        }
    }
    if capped > 0 {
        out.warn(format!(
            "party: {capped} weaker repeat mentions not emitted"
        ));
    }
    let ranked: Vec<Value> = groups
        .iter()
        .take(MAX_RANKED)
        .map(|(key, idx, score)| {
            let best = &found.candidates[idx[0]];
            let mut roles: Vec<&str> = Vec::new();
            for &i in idx {
                for (r, _) in &found.candidates[i].roles {
                    if !roles.contains(&r.as_str()) {
                        roles.push(r.as_str());
                    }
                }
            }
            Value::record([
                ("key", Value::text(key.as_str())),
                ("surface", Value::text(best.surface.as_str())),
                ("score", Value::Float(f64::from(*score))),
                (
                    "roles",
                    Value::List(roles.into_iter().map(Value::text).collect()),
                ),
            ])
        })
        .collect();
    if !ranked.is_empty() {
        out.fact(keys::CANDIDATES, Value::List(ranked));
    }
    if found.aliases.len() > MAX_STATEMENTS {
        out.problem(format!(
            "party: {} name statements, {} emitted",
            found.aliases.len(),
            MAX_STATEMENTS
        ));
    }
    for a in found.aliases.iter().take(MAX_STATEMENTS) {
        let v = Value::record([
            ("current", Value::text(a.current.as_str())),
            ("other", Value::text(a.other.as_str())),
            ("kind", Value::text(a.kind)),
        ]);
        out.push(keys::ALIAS_OF, v, 0.85, Some(a.evidence.clone()));
    }
    for (t, ev) in found.titles.iter().take(MAX_TITLES) {
        out.push(
            keys::SIGNATORY_TITLE,
            Value::text(t.as_str()),
            0.7,
            Some(ev.clone()),
        );
    }
    if found.signatory_lines > 0 {
        let n = i64::try_from(found.signatory_lines).unwrap_or(i64::MAX);
        out.push(keys::SIGNATORY_COUNT, Value::Int(n), 0.6, None);
    }
}

/// The record of one mention (fields in this order; optional ones left out when absent).
pub fn candidate_record(c: &Candidate) -> Value {
    let mut fields: Vec<(&str, Value)> = vec![
        ("surface", Value::text(c.surface.as_str())),
        ("key", Value::text(c.key.as_str())),
        ("norm", Value::text(c.norm.as_str())),
        ("quality", Value::text(c.quality)),
    ];
    if let Some(f) = c.form {
        fields.push(("legal_form", Value::text(f)));
    }
    if let Some((j, jc)) = &c.jurisdiction {
        fields.push(("jurisdiction", Value::text(j.as_str())));
        fields.push((
            "jurisdiction_conf",
            Value::Float(f64::from(c.conf.min(*jc))),
        ));
    }
    fields.push((
        "roles",
        Value::List(
            c.roles
                .iter()
                .map(|(r, _)| Value::text(r.as_str()))
                .collect(),
        ),
    ));
    fields.push((
        "role_conf",
        Value::List(
            c.roles
                .iter()
                .map(|(_, rc)| Value::Float(f64::from(*rc)))
                .collect(),
        ),
    ));
    if let Some(t) = &c.term {
        fields.push(("defined_term", Value::text(t.as_str())));
    }
    Value::record(fields)
}

fn emit_candidate(c: &Candidate, out: &mut Emitter) {
    out.push(
        keys::COMPANY,
        candidate_record(c),
        c.conf,
        Some(c.evidence.clone()),
    );
}

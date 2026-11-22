//! The file card: what an LLM agent reads about a file, built only from facts.
//!
//! [`FileCard::from_facts`] picks the strongest candidates of the fact producers (type, title,
//! language, numbers, parties, dates, amounts, identifiers, items, sketches, producer, flags);
//! [`FileCard::to_value`] is the stored `summary.card` record and [`FileCard::render`] the
//! compact `key: value` text of `summary.card_text` under a hard token budget.
//!
//! Names, numbers and identifiers are copied verbatim, in their original script. Everything
//! the card adds itself (keys, roles, ISO dates, `EUR 4,200.00`) is English and
//! language-neutral.

use std::collections::HashMap;

use std::fmt::Write as _;

use quail_core::{Date, Fact, Money, Value};

use crate::format::{self, clip, one_line};
use crate::input::{self as k, Facts, field_text};
use crate::tokens;

/// Parties below this `party.candidates` score are left out.
pub const MIN_PARTY_SCORE: f64 = 0.3;
/// Parties kept in the card.
pub const MAX_PARTIES: usize = 4;
/// Cited documents kept in the card.
pub const MAX_CITES: usize = 6;
/// Dates kept in the card.
pub const MAX_DATES: usize = 5;
/// Amounts kept in the card.
pub const MAX_AMOUNTS: usize = 4;
/// Values kept per identifier kind.
pub const MAX_ID_VALUES: usize = 2;
/// Item categories kept in the card.
pub const MAX_CATEGORIES: usize = 3;
/// Keywords kept in the card (`summary.keywords`, best first).
pub const MAX_KEYWORDS: usize = 8;
/// Item categories whose top-candidate score is below this are left out (CPV matches of
/// generic descriptions such as "Phase II" score low).
pub const MIN_CATEGORY_SCORE: f64 = 0.3;
/// A largest amount without a role is shown only from this value up (small numbers next to a
/// currency are usually rates, fees per unit or noise).
pub const MIN_MAX_AMOUNT: f64 = 100.0;
/// Default hard budget of the rendered card, in estimated tokens (see [`crate::tokens`]).
pub const DEFAULT_MAX_TOKENS: usize = 300;

/// Roles of `party.candidates`, most telling first; `mentioned` is no role.
const ROLE_ORDER: &[&str] = &[
    "supplier",
    "customer",
    "issuer",
    "bill_to",
    "ship_to",
    "party",
    "signatory",
];

/// Relations of `ref.cites` that name the document's parent, most telling first.
const PARENT_RELATIONS: &[&str] = &["amends", "annex_to", "issued_under", "billed_against"];

/// The document's own number.
#[derive(Clone, Debug, PartialEq)]
pub struct DocNumber {
    /// As written.
    pub raw: String,
    /// Kind of the number (`contract`, `invoice`, `registry`, ...), when known.
    pub kind: Option<String>,
}

/// A document this one cites.
#[derive(Clone, Debug, PartialEq)]
pub struct Cite {
    /// `amends`, `annex_to`, `issued_under`, `billed_against` or `references`.
    pub relation: String,
    /// Kind of the cited document (`msa`, `po`, `procurement`, ...).
    pub kind: Option<String>,
    /// Its number as written.
    pub raw: String,
    /// Its ordinal (Amendment No. 3), when written.
    pub ordinal: Option<i64>,
    /// Its date, when written.
    pub date: Option<Date>,
}

/// A party with its strongest role.
#[derive(Clone, Debug, PartialEq)]
pub struct Party {
    /// Surface form as written.
    pub name: String,
    /// Strongest role (`supplier`, `customer`, ...), `None` when only mentioned.
    pub role: Option<String>,
    /// `party.candidates` score (or mention confidence).
    pub score: f64,
}

/// One kind of identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct Ids {
    /// Short kind name: `pib`, `mb`, `vat`, `iban`, ...
    pub kind: &'static str,
    /// The most confident values, at most [`MAX_ID_VALUES`].
    pub values: Vec<String>,
    /// Distinct values in the file.
    pub count: usize,
}

/// Price items.
#[derive(Clone, Debug, PartialEq)]
pub struct Items {
    /// Number of `item.line` facts.
    pub count: i64,
    /// Most frequent top category of the lines: `(CPV code, label, lines)`.
    pub categories: Vec<(String, String, usize)>,
}

/// Similarity sketches, for "same text / same template" lookups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sketch {
    /// `sketch.simhash_text`.
    pub text: Option<u64>,
    /// `sketch.simhash_template`.
    pub template: Option<u64>,
    /// First 8 bytes of `sketch.text_key`, hex.
    pub text_key: Option<String>,
}

/// What the agent reads about one file. Every field is optional; see the crate README for the
/// record layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileCard {
    /// Detected format (`pdf`, `docx`, ...).
    pub format: Option<String>,
    /// Instrument kind (`msa`, `amendment`, `invoice`, ...).
    pub kind: Option<String>,
    /// Ordinal of the instrument (Amendment No. 2 → 2).
    pub ordinal: Option<i64>,
    /// Title as written.
    pub title: Option<String>,
    /// `text` (from the content) or `metadata` (from the file properties).
    pub title_source: Option<&'static str>,
    /// ISO 639-1 language.
    pub lang: Option<String>,
    /// Dominant script (`latin`, `cyrillic`, `mixed`, ...).
    pub script: Option<String>,
    /// Page count.
    pub pages: Option<i64>,
    /// The document's own number.
    pub number: Option<DocNumber>,
    /// Cited documents, parent relations first, at most [`MAX_CITES`].
    pub cites: Vec<Cite>,
    /// Distinct cited documents in the file.
    pub cites_total: usize,
    /// Parties, best first, at most [`MAX_PARTIES`].
    pub parties: Vec<Party>,
    /// Parties above [`MIN_PARTY_SCORE`] in the file.
    pub parties_total: usize,
    /// Dates by role (`signed`, `effective`, `expires`, `issued`, `due`, or `date`).
    pub dates: Vec<(&'static str, Date)>,
    /// Amounts by role (`total`, `net`, `gross`, or `max`).
    pub amounts: Vec<(&'static str, Money)>,
    /// Currencies of all amounts, most frequent first.
    pub currencies: Vec<String>,
    /// Identifiers by kind.
    pub ids: Vec<Ids>,
    /// Valid personal ids (JMBG) counted; values are never stored.
    pub personal_ids: Option<i64>,
    /// Price items.
    pub items: Option<Items>,
    /// The document's top keywords (`summary.keywords`), best first.
    pub keywords: Vec<String>,
    /// Similarity sketches.
    pub sketch: Sketch,
    /// Producing application.
    pub producer: Option<String>,
    /// Author from the file properties.
    pub author: Option<String>,
    /// Company from the file properties.
    pub company: Option<String>,
    /// Flags: `scanned`, `ocr`, `needs_ocr`, `encrypted`, `signed`, `signature_fields`,
    /// `macros`, `javascript`, `hidden_text`, `tracked_changes`, `comments`, `embedded`,
    /// `updated`, `protected`, `garbled_text`, with a count where one is known.
    pub flags: Vec<(&'static str, Option<i64>)>,
}

impl FileCard {
    /// Builds the card from the facts produced so far. `doc_pages` is the page count of the
    /// extracted document, used when no extractor fact gives one.
    pub fn from_facts(facts: &[Fact], doc_pages: Option<usize>) -> Self {
        let f = Facts(facts);
        let mut c = FileCard {
            format: f.text(k::ENGINE_FORMAT).map(str::to_owned),
            kind: f.text(k::REF_KIND).map(str::to_owned),
            ordinal: f.int(k::REF_ORDINAL),
            lang: f.text(k::TEXT_LANG).map(str::to_owned),
            script: f.text(k::TEXT_SCRIPT).map(str::to_owned),
            ..FileCard::default()
        };
        c.title_from(f);
        c.pages = f
            .int(k::PDF_PAGE_COUNT)
            .or_else(|| f.int(k::DOCX_PAGES).filter(|&p| p > 0))
            .or_else(|| doc_pages.and_then(|p| i64::try_from(p).ok()));
        c.number = f.best(k::REF_DOC_NUMBER).and_then(|n| {
            Some(DocNumber {
                raw: one_line(field_text(&n.value, "raw")?),
                kind: field_text(&n.value, "kind").map(str::to_owned),
            })
        });
        c.cites_from(f);
        c.parties_from(f);
        c.dates_from(f);
        c.amounts_from(f);
        c.ids_from(f);
        c.personal_ids = f.int(k::SIGNAL_JMBG_COUNT).filter(|&n| n > 0);
        c.items_from(f);
        if let Some(Value::List(ks)) = f.best(crate::keys::KEYWORDS.as_str()).map(|x| &x.value) {
            c.keywords = ks
                .iter()
                .filter_map(|k| field_text(k, "term"))
                .map(|t| clip(&one_line(t), 40))
                .take(MAX_KEYWORDS)
                .collect();
        }
        c.sketch = Sketch {
            text: f.u64(k::SKETCH_SIMHASH_TEXT),
            template: f.u64(k::SKETCH_SIMHASH_TEMPLATE),
            text_key: f.all(k::SKETCH_TEXT_KEY).find_map(|x| match &x.value {
                Value::Bytes(b) => Some(format::hex(&b[..b.len().min(8)])),
                _ => None,
            }),
        };
        c.producer = f
            .text(k::PDF_PRODUCER)
            .or_else(|| f.text(k::DOCX_APPLICATION))
            .or_else(|| f.text(k::PDF_CREATOR))
            .map(one_line);
        c.author = f
            .text(k::PDF_AUTHOR)
            .or_else(|| f.text(k::DOCX_CREATOR))
            .map(one_line);
        c.company = f.text(k::DOCX_COMPANY).map(one_line);
        c.flags_from(f);
        c
    }

    fn title_from(&mut self, f: Facts<'_>) {
        if let Some(t) = f.text(k::REF_TITLE) {
            self.title = Some(one_line(t));
            self.title_source = Some("text");
            return;
        }
        let meta = f.text(k::PDF_TITLE).or_else(|| f.text(k::DOCX_TITLE));
        if let Some(t) = meta.map(clean_meta_title).filter(|t| !t.is_empty()) {
            self.title = Some(t);
            self.title_source = Some("metadata");
        }
    }

    fn cites_from(&mut self, f: Facts<'_>) {
        let mut all: Vec<(f32, Cite)> = Vec::new();
        for x in f.all(k::REF_CITES) {
            let Some(raw) = field_text(&x.value, "raw") else {
                continue;
            };
            let key = field_text(&x.value, "key").unwrap_or(raw);
            let relation = field_text(&x.value, "relation").unwrap_or("references");
            let cite = Cite {
                relation: relation.to_owned(),
                kind: field_text(&x.value, "kind").map(str::to_owned),
                raw: one_line(raw),
                ordinal: match x.value.field("ordinal") {
                    Some(Value::Int(i)) => Some(*i),
                    _ => None,
                },
                date: match x.value.field("date") {
                    Some(Value::Date(d)) => Some(*d),
                    _ => None,
                },
            };
            // One entry per cited number: the most confident mention wins.
            let dup = all
                .iter_mut()
                .find(|(_, c)| c.raw.eq_ignore_ascii_case(raw) || c.raw.eq_ignore_ascii_case(key));
            match dup {
                Some((conf, c)) if x.conf > *conf => {
                    *conf = x.conf;
                    *c = cite;
                }
                Some(_) => {}
                None => all.push((x.conf, cite)),
            }
        }
        self.cites_total = all.len();
        let rank = |c: &Cite| {
            PARENT_RELATIONS
                .iter()
                .position(|r| *r == c.relation)
                .unwrap_or(PARENT_RELATIONS.len())
        };
        all.sort_by(|a, b| rank(&a.1).cmp(&rank(&b.1)).then(b.0.total_cmp(&a.0)));
        self.cites = all.into_iter().take(MAX_CITES).map(|(_, c)| c).collect();
    }

    fn parties_from(&mut self, f: Facts<'_>) {
        let mut parties: Vec<Party> = Vec::new();
        if let Some(Value::List(cands)) = f.best(k::PARTY_CANDIDATES).map(|x| &x.value) {
            for cand in cands {
                let Some(name) = field_text(cand, "surface") else {
                    continue;
                };
                let score = match cand.field("score") {
                    Some(Value::Float(s)) => *s,
                    _ => 0.0,
                };
                if score < MIN_PARTY_SCORE {
                    continue;
                }
                let roles: Vec<&str> = match cand.field("roles") {
                    Some(Value::List(r)) => r.iter().filter_map(Value::as_text).collect(),
                    _ => Vec::new(),
                };
                parties.push(Party {
                    name: one_line(name),
                    role: best_role(&roles),
                    score,
                });
            }
        }
        if parties.is_empty() {
            // Older records or a build without candidates: the strongest mentions.
            let mut seen: Vec<String> = Vec::new();
            let mut mentions: Vec<&Fact> = f
                .all(k::PARTY_COMPANY)
                .filter(|x| f64::from(x.conf) >= MIN_PARTY_SCORE)
                .collect();
            mentions.sort_by(|a, b| b.conf.total_cmp(&a.conf));
            for x in mentions {
                let (Some(name), key) =
                    (field_text(&x.value, "surface"), field_text(&x.value, "key"))
                else {
                    continue;
                };
                let key = key.unwrap_or(name).to_owned();
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                let roles: Vec<&str> = match x.value.field("roles") {
                    Some(Value::List(r)) => r.iter().filter_map(Value::as_text).collect(),
                    _ => Vec::new(),
                };
                parties.push(Party {
                    name: one_line(name),
                    role: best_role(&roles),
                    score: f64::from(x.conf),
                });
            }
        }
        if parties.is_empty() {
            // CLM form fields of a DOCX: first party is the client, second the supplier.
            for (key, role) in [
                (k::DOCX_CLM_FIRST, "customer"),
                (k::DOCX_CLM_SECOND, "supplier"),
            ] {
                if let Some(name) = f.text(key) {
                    parties.push(Party {
                        name: one_line(name),
                        role: Some(role.to_owned()),
                        score: 0.9,
                    });
                }
            }
        }
        // `"KI Inc.`: a stray quote of the defining clause is no part of the name.
        for p in &mut parties {
            let t = p
                .name
                .trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '\'' | '‘' | '’'));
            if t.len() != p.name.len() {
                p.name = t.trim().to_owned();
            }
        }
        // "II, LLC", "GP, LLC", ", Inc.": the tail of a name cut at its start is no party.
        parties.retain(|p| !truncated(&p.name));
        // "Cisco" after "Cisco Systems, Inc.": a shorter name inside a stronger one is the same
        // party.
        let mut kept: Vec<Party> = Vec::new();
        for p in parties {
            let words = name_words(&p.name);
            let dup = kept.iter().any(|q| {
                let qw = name_words(&q.name);
                !words.is_empty() && words.iter().all(|w| qw.contains(w))
            });
            if !dup {
                kept.push(p);
            }
        }
        let mut parties = kept;
        self.parties_total = parties.len();
        parties.truncate(MAX_PARTIES);
        self.parties = parties;
    }

    fn dates_from(&mut self, f: Facts<'_>) {
        for &(key, role) in k::SIGNAL_DATES {
            if let Some(Value::Date(d)) = f.best(key).map(|x| &x.value) {
                self.dates.push((role, *d));
            }
        }
        if self.dates.is_empty()
            && let Some(Value::Date(d)) = f.best(k::SIGNAL_DATE).map(|x| &x.value)
        {
            self.dates.push(("date", *d));
        }
        // An expiry before the agreement takes effect (or is signed) is a misread date.
        let start = self
            .dates
            .iter()
            .find(|(r, _)| *r == "effective")
            .or_else(|| self.dates.iter().find(|(r, _)| *r == "signed"))
            .map(|(_, d)| *d);
        if let Some(start) = start {
            self.dates.retain(|(r, d)| *r != "expires" || *d >= start);
        }
        self.dates.truncate(MAX_DATES);
    }

    fn amounts_from(&mut self, f: Facts<'_>) {
        let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
        let mut any: Vec<Money> = Vec::new();
        for x in f.all(k::SIGNAL_AMOUNT) {
            if let Value::Money(m) = x.value {
                if let Some(c) = m.currency {
                    *counts.entry(c).or_default() += 1;
                }
                any.push(m);
            }
        }
        let mut cur: Vec<([u8; 3], usize)> = counts.into_iter().collect();
        cur.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        self.currencies = cur
            .iter()
            .filter_map(|(c, _)| std::str::from_utf8(c).ok().map(str::to_owned))
            .collect();
        for &(key, role) in k::SIGNAL_AMOUNTS {
            let mut xs: Vec<&Fact> = f.all(key).collect();
            xs.sort_by(|a, b| b.conf.total_cmp(&a.conf));
            let mut taken = 0;
            for x in xs {
                if let Value::Money(m) = x.value
                    && !self.amounts.iter().any(|(_, a)| *a == m)
                {
                    self.amounts.push((role, m));
                    taken += 1;
                    if taken == 2 {
                        break;
                    }
                }
            }
        }
        if self.amounts.is_empty() {
            // No role: the largest amount in the dominant currency.
            let main = cur.first().map(|(c, _)| *c);
            let max = any
                .into_iter()
                .filter(|m| main.is_none() || m.currency == main)
                .max_by(|a, b| format::money_value(*a).total_cmp(&format::money_value(*b)));
            if let Some(m) = max.filter(|m| format::money_value(*m) >= MIN_MAX_AMOUNT) {
                self.amounts.push(("max", m));
            }
        }
        self.amounts.truncate(MAX_AMOUNTS);
    }

    fn ids_from(&mut self, f: Facts<'_>) {
        for &(key, kind) in k::SIGNAL_IDS {
            let mut xs: Vec<(&str, f32)> = Vec::new();
            for x in f.all(key) {
                if let Some(v) = x.value.as_text().map(str::trim).filter(|v| !v.is_empty()) {
                    match xs.iter_mut().find(|(s, _)| *s == v) {
                        Some((_, c)) => *c = c.max(x.conf),
                        None => xs.push((v, x.conf)),
                    }
                }
            }
            if xs.is_empty() {
                continue;
            }
            xs.sort_by(|a, b| b.1.total_cmp(&a.1));
            self.ids.push(Ids {
                kind,
                count: xs.len(),
                values: xs
                    .iter()
                    .take(MAX_ID_VALUES)
                    .map(|(v, _)| (*v).to_owned())
                    .collect(),
            });
        }
    }

    fn items_from(&mut self, f: Facts<'_>) {
        let lines: Vec<&Fact> = f.all(k::ITEM_LINE).collect();
        let count = f
            .int(k::ITEM_COUNT)
            .or_else(|| i64::try_from(lines.len()).ok())
            .unwrap_or(0);
        if count <= 0 {
            return;
        }
        let mut cats: Vec<(String, String, usize)> = Vec::new();
        for l in &lines {
            let Some(Value::List(cands)) = l.value.field("category_candidates") else {
                continue;
            };
            let Some(top) = cands.first() else { continue };
            let score = match top.field("score") {
                Some(Value::Float(x)) => *x,
                _ => 0.0,
            };
            if score < MIN_CATEGORY_SCORE {
                continue;
            }
            let (Some(code), Some(label)) = (field_text(top, "code"), field_text(top, "label"))
            else {
                continue;
            };
            match cats.iter_mut().find(|(c, _, _)| c == code) {
                Some((_, _, n)) => *n += 1,
                None => cats.push((code.to_owned(), label.to_owned(), 1)),
            }
        }
        cats.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        cats.truncate(MAX_CATEGORIES);
        self.items = Some(Items {
            count,
            categories: cats,
        });
    }

    fn flags_from(&mut self, f: Facts<'_>) {
        let pos = |key| f.int(key).filter(|&n| n > 0);
        if let Some(n) = pos(k::PDF_IMAGE_ONLY_PAGES) {
            self.flags.push(("scanned", Some(n)));
        }
        if let Some(n) = pos(k::OCR_PAGES) {
            self.flags.push(("ocr", Some(n)));
        } else if let Some(n) = pos(k::PDF_NEEDS_OCR) {
            self.flags.push(("needs_ocr", Some(n)));
        }
        if f.bool(k::PDF_ENCRYPTED) == Some(true) {
            self.flags.push(("encrypted", None));
        }
        if f.bool(k::PDF_SIGNED) == Some(true) {
            self.flags.push(("signed", None));
        } else if let Some(n) = pos(k::PDF_SIGNATURE_FIELDS) {
            self.flags.push(("signature_fields", Some(n)));
        }
        if f.bool(k::DOCX_MACROS) == Some(true) {
            self.flags.push(("macros", None));
        }
        if f.bool(k::PDF_JAVASCRIPT) == Some(true) {
            self.flags.push(("javascript", None));
        }
        if let Some(n) = pos(k::DOCX_HIDDEN) {
            self.flags.push(("hidden_text", Some(n)));
        }
        let tracked: i64 = [k::DOCX_INS, k::DOCX_DEL, k::DOCX_FMT]
            .iter()
            .filter_map(|&key| pos(key))
            .sum();
        if tracked > 0 {
            self.flags.push(("tracked_changes", Some(tracked)));
        }
        if let Some(n) = pos(k::DOCX_COMMENTS) {
            self.flags.push(("comments", Some(n)));
        }
        if let Some(n) = pos(k::PDF_EMBEDDED).or_else(|| pos(k::DOCX_EMBEDDED)) {
            self.flags.push(("embedded", Some(n)));
        }
        if let Some(n) = pos(k::PDF_UPDATES) {
            self.flags.push(("updated", Some(n)));
        }
        if f.text(k::DOCX_PROTECTION).is_some() {
            self.flags.push(("protected", None));
        }
        if f.float(k::TEXT_GARBAGE).is_some_and(|g| g > 0.3) {
            self.flags.push(("garbled_text", None));
        }
    }

    /// The parent document this one amends, is an annex to, is issued under or billed against.
    pub fn parent(&self) -> Option<&Cite> {
        self.cites
            .first()
            .filter(|c| PARENT_RELATIONS.contains(&c.relation.as_str()))
    }

    /// Language tag for display: `en`, `sr-Latn`, `sr-Cyrl`, ...
    pub fn lang_tag(&self) -> Option<String> {
        let lang = self.lang.as_deref()?;
        let script = match self.script.as_deref() {
            Some("latin") => "Latn",
            Some("cyrillic") => "Cyrl",
            _ => return Some(lang.to_owned()),
        };
        // A script subtag only where the language is written in more than one.
        if matches!(lang, "sr" | "bs" | "sh" | "me" | "mk") {
            Some(format!("{lang}-{script}"))
        } else {
            Some(lang.to_owned())
        }
    }

    /// The stored `summary.card` record (fields in this order, absent ones left out).
    #[allow(clippy::too_many_lines)] // one block per field
    pub fn to_value(&self) -> Value {
        let mut r: Vec<(&str, Value)> = Vec::new();
        let text = |s: &str| Value::text(s);
        if let Some(x) = &self.format {
            r.push(("format", text(x)));
        }
        if let Some(x) = &self.kind {
            r.push(("kind", text(x)));
        }
        if let Some(x) = self.ordinal {
            r.push(("ordinal", Value::Int(x)));
        }
        if let Some(x) = &self.title {
            r.push(("title", text(x)));
        }
        if let Some(x) = self.title_source {
            r.push(("title_source", text(x)));
        }
        if let Some(x) = self.lang_tag() {
            r.push(("lang", Value::text(x)));
        }
        if let Some(x) = &self.script {
            r.push(("script", text(x)));
        }
        if let Some(x) = self.pages {
            r.push(("pages", Value::Int(x)));
        }
        if let Some(n) = &self.number {
            let mut f = vec![("raw", text(&n.raw))];
            if let Some(kd) = &n.kind {
                f.push(("kind", text(kd)));
            }
            r.push(("number", Value::record(f)));
        }
        if !self.cites.is_empty() {
            let list = self
                .cites
                .iter()
                .map(|c| {
                    let mut f = vec![("relation", text(&c.relation))];
                    if let Some(kd) = &c.kind {
                        f.push(("kind", text(kd)));
                    }
                    f.push(("raw", text(&c.raw)));
                    if let Some(o) = c.ordinal {
                        f.push(("ordinal", Value::Int(o)));
                    }
                    if let Some(d) = c.date {
                        f.push(("date", Value::Date(d)));
                    }
                    Value::record(f)
                })
                .collect();
            r.push(("cites", Value::List(list)));
            r.push(("cites_total", count(self.cites_total)));
        }
        if !self.parties.is_empty() {
            let list = self
                .parties
                .iter()
                .map(|p| {
                    let mut f = vec![("name", text(&p.name))];
                    if let Some(role) = &p.role {
                        f.push(("role", text(role)));
                    }
                    f.push(("score", Value::Float((p.score * 1000.0).round() / 1000.0)));
                    Value::record(f)
                })
                .collect();
            r.push(("parties", Value::List(list)));
            r.push(("parties_total", count(self.parties_total)));
        }
        if !self.dates.is_empty() {
            let list = self
                .dates
                .iter()
                .map(|(role, d)| Value::record([("role", text(role)), ("date", Value::Date(*d))]))
                .collect();
            r.push(("dates", Value::List(list)));
        }
        if !self.amounts.is_empty() {
            let list = self
                .amounts
                .iter()
                .map(|(role, m)| {
                    Value::record([("role", text(role)), ("amount", Value::Money(*m))])
                })
                .collect();
            r.push(("amounts", Value::List(list)));
        }
        if !self.currencies.is_empty() {
            r.push((
                "currencies",
                Value::List(self.currencies.iter().map(|c| text(c)).collect()),
            ));
        }
        if !self.ids.is_empty() {
            let list = self
                .ids
                .iter()
                .map(|i| {
                    Value::record([
                        ("kind", text(i.kind)),
                        (
                            "values",
                            Value::List(i.values.iter().map(|v| text(v)).collect()),
                        ),
                        ("count", count(i.count)),
                    ])
                })
                .collect();
            r.push(("ids", Value::List(list)));
        }
        if let Some(n) = self.personal_ids {
            r.push(("personal_ids", Value::Int(n)));
        }
        if let Some(it) = &self.items {
            let cats = it
                .categories
                .iter()
                .map(|(code, label, n)| {
                    Value::record([
                        ("code", text(code)),
                        ("label", text(label)),
                        ("lines", count(*n)),
                    ])
                })
                .collect();
            r.push((
                "items",
                Value::record([
                    ("count", Value::Int(it.count)),
                    ("categories", Value::List(cats)),
                ]),
            ));
        }
        if !self.keywords.is_empty() {
            r.push((
                "keywords",
                Value::List(self.keywords.iter().map(|k| text(k)).collect()),
            ));
        }
        let s = &self.sketch;
        if s.text.is_some() || s.template.is_some() || s.text_key.is_some() {
            let mut f = Vec::new();
            if let Some(x) = s.text {
                f.push(("simhash_text", Value::U64(x)));
            }
            if let Some(x) = s.template {
                f.push(("simhash_template", Value::U64(x)));
            }
            if let Some(x) = &s.text_key {
                f.push(("text_key", text(x)));
            }
            r.push(("sketch", Value::record(f)));
        }
        for (name, v) in [
            ("producer", &self.producer),
            ("author", &self.author),
            ("company", &self.company),
        ] {
            if let Some(x) = v {
                r.push((name, text(x)));
            }
        }
        if !self.flags.is_empty() {
            r.push((
                "flags",
                Value::List(self.flags.iter().map(|f| Value::text(flag(*f))).collect()),
            ));
        }
        Value::record(r)
    }

    /// The compact `key: value` text (see [`render`](Self::render)) under the default budget.
    pub fn text(&self) -> CardText {
        self.render(DEFAULT_MAX_TOKENS)
    }

    /// Renders the card as compact `key: value` lines, at most `max_tokens` estimated tokens
    /// ([`crate::tokens::estimate`]).
    ///
    /// Per-field limits first (title 100 characters, names 60, labels 40, list lengths as in
    /// the `MAX_*` constants, `+N` for what is left out), then whole lines are dropped from the
    /// least useful up (`sketch`, `meta`, `items`, `flags`, `ids`, `keywords`, `refs`,
    /// `amounts`, `dates`, `parties`) until the text fits; the `doc` line is clipped last.
    #[allow(clippy::too_many_lines)] // one block per line
    pub fn render(&self, max_tokens: usize) -> CardText {
        // (priority, line); lower priority is dropped first.
        let mut lines: Vec<(u8, String)> = Vec::new();
        lines.push((20, self.doc_line()));
        if !self.parties.is_empty() {
            let mut parts: Vec<String> = self
                .parties
                .iter()
                .map(|p| match &p.role {
                    Some(r) => format!("{r}={}", clip(&p.name, 60)),
                    None => clip(&p.name, 60),
                })
                .collect();
            more(&mut parts, self.parties_total, self.parties.len());
            lines.push((18, format!("parties: {}", parts.join("; "))));
        }
        if !self.dates.is_empty() {
            let parts: Vec<String> = self
                .dates
                .iter()
                .map(|(r, d)| format!("{r}={}", format::date(*d)))
                .collect();
            lines.push((16, format!("dates: {}", parts.join(" "))));
        }
        if !self.amounts.is_empty() {
            let parts: Vec<String> = self
                .amounts
                .iter()
                .map(|(r, m)| format!("{r}={}", format::money(*m)))
                .collect();
            lines.push((14, format!("amounts: {}", parts.join("; "))));
        }
        if !self.cites.is_empty() {
            let mut parts: Vec<String> = self
                .cites
                .iter()
                .take(4)
                .map(|c| {
                    let mut s = format!("{}=", c.relation);
                    if let Some(kd) = &c.kind {
                        s.push_str(kd);
                        s.push(' ');
                    }
                    s.push_str(&clip(&c.raw, 40));
                    if let Some(d) = c.date {
                        let _ = write!(s, " ({})", format::date(d));
                    }
                    s
                })
                .collect();
            let shown = parts.len();
            more(&mut parts, self.cites_total, shown);
            lines.push((12, format!("refs: {}", parts.join("; "))));
        }
        if !self.keywords.is_empty() {
            lines.push((11, format!("keywords: {}", self.keywords.join(", "))));
        }
        if !self.ids.is_empty() || self.personal_ids.is_some() {
            let mut parts: Vec<String> = self
                .ids
                .iter()
                .map(|i| {
                    let mut s = format!("{}={}", i.kind, i.values.join(","));
                    if i.count > i.values.len() {
                        let _ = write!(s, " (+{})", i.count - i.values.len());
                    }
                    s
                })
                .collect();
            if let Some(n) = self.personal_ids {
                parts.push(format!("personal_ids×{n}"));
            }
            lines.push((10, format!("ids: {}", parts.join(" "))));
        }
        if !self.flags.is_empty() {
            let parts: Vec<String> = self.flags.iter().map(|f| flag(*f)).collect();
            lines.push((8, format!("flags: {}", parts.join(" "))));
        }
        if let Some(it) = &self.items {
            let mut s = format!("items: {}", it.count);
            if !it.categories.is_empty() {
                let cats: Vec<String> = it
                    .categories
                    .iter()
                    .map(|(code, label, n)| format!("{code} {} ({n})", clip(label, 40)))
                    .collect();
                let _ = write!(s, "; top: {}", cats.join(", "));
            }
            lines.push((6, s));
        }
        let meta: Vec<String> = [
            ("producer", &self.producer),
            ("author", &self.author),
            ("company", &self.company),
        ]
        .iter()
        .filter_map(|(n, v)| v.as_ref().map(|v| format!("{n}=\"{}\"", clip(v, 40))))
        .collect();
        if !meta.is_empty() {
            lines.push((4, format!("meta: {}", meta.join(" "))));
        }
        let s = &self.sketch;
        let mut sk: Vec<String> = Vec::new();
        if let Some(x) = s.text {
            sk.push(format!("text={x:016x}"));
        }
        if let Some(x) = s.template {
            sk.push(format!("tmpl={x:016x}"));
        }
        if let Some(x) = &s.text_key {
            sk.push(format!("key={x}"));
        }
        if !sk.is_empty() {
            lines.push((2, format!("sketch: {}", sk.join(" "))));
        }

        let cost = |ls: &[(u8, String)]| ls.iter().map(|(_, l)| tokens::estimate(l)).sum::<usize>();
        while cost(&lines) > max_tokens && lines.len() > 1 {
            // Drop the lowest-priority line.
            let (i, _) = lines
                .iter()
                .enumerate()
                .min_by_key(|(_, (p, _))| *p)
                .unwrap_or((0, &lines[0]));
            lines.remove(i);
        }
        let mut text: String = lines
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if tokens::estimate(&text) > max_tokens {
            text = tokens::head(&text, max_tokens).to_owned();
        }
        let tokens = tokens::estimate(&text);
        CardText { text, tokens }
    }

    fn doc_line(&self) -> String {
        let mut s = String::from("doc: ");
        s.push_str(self.kind.as_deref().unwrap_or("document"));
        if let Some(o) = self.ordinal {
            let _ = write!(s, " #{o}");
        }
        if let Some(n) = &self.number {
            let _ = write!(s, " no={}", clip(&n.raw, 40));
        }
        if let Some(t) = &self.title {
            let _ = write!(s, " \"{}\"", clip(t, 100).replace('"', "'"));
        }
        if let Some(l) = self.lang_tag() {
            let _ = write!(s, " lang={l}");
        }
        if let Some(p) = self.pages {
            let _ = write!(s, " pages={p}");
        }
        if let Some(fmt) = &self.format {
            s.push(' ');
            s.push_str(fmt);
        }
        s
    }
}

/// A rendered card and its estimated token count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardText {
    /// The `key: value` lines.
    pub text: String,
    /// [`crate::tokens::estimate`] of `text`.
    pub tokens: usize,
}

fn count(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn flag((name, n): (&'static str, Option<i64>)) -> String {
    match n {
        Some(n) => format!("{name}({n})"),
        None => name.to_owned(),
    }
}

fn more(parts: &mut Vec<String>, total: usize, shown: usize) {
    if total > shown {
        parts.push(format!("+{}", total - shown));
    }
}

/// Folded words of a name, legal forms and punctuation dropped (for duplicate checks).
/// A name cut at its start: it opens with a comma, `&` or a lower-case word, or nothing but a
/// roman numeral or a two-letter word stands before its legal form (`II, LLC`, `GP, LLC`).
fn truncated(name: &str) -> bool {
    let t = name.trim();
    if t.starts_with([',', '&', '.', ';']) || t.starts_with(|c: char| c.is_lowercase()) {
        return true;
    }
    let words = name_words(t);
    match words.as_slice() {
        [] => true,
        // `GP, LLC`, but not `HP Inc.` or `3M Company`.
        [w] => {
            (w.len() <= 4 && w.chars().all(|c| matches!(c, 'i' | 'v' | 'x')))
                || (w.len() <= 2 && t.split_once(',').is_some_and(|(a, _)| a.trim().len() <= 2))
        }
        _ => false,
    }
}

fn name_words(name: &str) -> Vec<String> {
    const FORMS: &[&str] = &[
        "inc",
        "llc",
        "ltd",
        "limited",
        "corp",
        "corporation",
        "co",
        "company",
        "plc",
        "gmbh",
        "ag",
        "sa",
        "s",
        "a",
        "l",
        "p",
        "lp",
        "llp",
        "doo",
        "d",
        "o",
        "ad",
        "bv",
        "nv",
        "the",
    ];
    let f = quail_text::fold::fold(name, &quail_text::fold::FoldOptions::KEY);
    f.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !FORMS.contains(w))
        .map(str::to_owned)
        .collect()
}

fn best_role(roles: &[&str]) -> Option<String> {
    ROLE_ORDER
        .iter()
        .find(|r| roles.contains(r))
        .map(|r| (*r).to_owned())
}

/// A title from file properties without the usual noise: `Microsoft Word - x.docx` → `x`.
fn clean_meta_title(t: &str) -> String {
    let t = one_line(t);
    let t = t
        .strip_prefix("Microsoft Word - ")
        .or_else(|| t.strip_prefix("Microsoft Word – "))
        .unwrap_or(&t);
    let lower = t.to_ascii_lowercase();
    let t = [".docx", ".doc", ".pdf", ".rtf", ".odt"]
        .iter()
        .find_map(|ext| lower.ends_with(ext).then(|| &t[..t.len() - ext.len()]))
        .unwrap_or(t);
    let t = t.trim();
    // A file-name-like or placeholder title says nothing.
    if t.len() < 3 || t.eq_ignore_ascii_case("untitled") || t.eq_ignore_ascii_case("title") {
        return String::new();
    }
    t.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_titles() {
        assert_eq!(clean_meta_title("Microsoft Word - Aneks 2.docx"), "Aneks 2");
        assert_eq!(clean_meta_title("Untitled"), "");
        assert_eq!(clean_meta_title("  Ugovor  o zakupu "), "Ugovor o zakupu");
    }

    #[test]
    fn shorter_names_inside_stronger_ones_are_one_party() {
        assert_eq!(name_words("Cisco Systems, Inc."), vec!["cisco", "systems"]);
        assert_eq!(
            name_words("Балкан Логистик д.о.о."),
            vec!["balkan", "logistik"]
        );
    }

    #[test]
    fn truncated_names_are_no_parties() {
        assert!(truncated("II, LLC"));
        assert!(truncated("GP, LLC"));
        assert!(truncated(", Inc."));
        assert!(truncated("LLC"));
        assert!(!truncated("HP Inc."));
        assert!(!truncated("3M Company"));
        assert!(!truncated("ValueAct Capital Master Fund II, L.P."));
        assert!(!truncated("Cisco Systems, Inc."));
    }

    #[test]
    fn roles() {
        assert_eq!(
            best_role(&["mentioned", "customer", "signatory"]).as_deref(),
            Some("customer")
        );
        assert_eq!(best_role(&["mentioned"]), None);
    }
}

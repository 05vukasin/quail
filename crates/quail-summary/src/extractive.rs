//! The model-free summary: a template sentence from the card plus the document's own subject
//! sentence.
//!
//! - [`template_sentence`]: "Amendment No. 2 (no. 114/23-A2) to the master services agreement
//!   114/23 between X (supplier) and Y (customer), signed 2023-09-12, total EUR 4,200.00."
//!   English words, card values verbatim.
//! - [`subject`]: the first sentence of the subject clause ("Subject of the Agreement",
//!   "Purpose", "Scope of Work", "Article 1", "Predmet ugovora", "Član 1", in Latin or
//!   Cyrillic), else the top sentence of a `TextRank` over the first pages.
//! - [`summary`]: both, bounded. A non-English subject sentence is quoted in its original
//!   language and marked with the language code.

use std::fmt::Write as _;

use quail_core::{Document, TextSource};
use quail_text::fold::{FoldOptions, fold};
use quail_text::normalize::{NormalizeOptions, normalize};

use crate::card::FileCard;
use crate::format::{self, capitalize, clip, one_line};

/// Pages read for the subject sentence.
pub const MAX_PAGES: usize = 4;
/// Bytes of normalized text read for the subject sentence.
pub const MAX_BYTES: usize = 48 * 1024;
/// Words of the subject sentence kept.
pub const MAX_SUBJECT_WORDS: usize = 40;
/// Characters of the template sentence kept.
pub const MAX_TEMPLATE_CHARS: usize = 420;
/// Sentences ranked by `TextRank` at most (the graph is quadratic).
pub const MAX_RANKED: usize = 150;

/// How the subject sentence was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectMethod {
    /// Under a subject / purpose / scope heading or label.
    Clause,
    /// Top sentence of `TextRank` over the first pages.
    TextRank,
}

impl SubjectMethod {
    /// `clause` or `textrank`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clause => "clause",
            Self::TextRank => "textrank",
        }
    }
}

/// The document's subject sentence, verbatim (normalized whitespace).
#[derive(Clone, Debug, PartialEq)]
pub struct Subject {
    /// The sentence, at most [`MAX_SUBJECT_WORDS`] words.
    pub text: String,
    /// Zero-based page it starts on.
    pub page: u32,
    /// How it was found.
    pub method: SubjectMethod,
    /// The heading or label it was found under (folded), for clauses.
    pub label: Option<&'static str>,
}

/// Subject labels (folded: lowercase, no diacritics, Serbian Cyrillic as Latin) and their
/// strength. Longer labels first so the longest match wins.
const LABELS: &[(&str, u8)] = &[
    ("subject matter of the agreement", 3),
    ("subject matter of this agreement", 3),
    ("subject of the agreement", 3),
    ("subject of this agreement", 3),
    ("subject of the contract", 3),
    ("subject of this contract", 3),
    ("subject of agreement", 3),
    ("subject of contract", 3),
    ("object of the agreement", 3),
    ("subject matter", 3),
    ("predmet okvirnog sporazuma", 3),
    ("predmet ugovora", 3),
    ("predmet sporazuma", 3),
    ("predmet aneksa", 3),
    ("predmet nabavke", 3),
    ("predmet javne nabavke", 3),
    ("predmet", 3),
    ("purpose of the agreement", 2),
    ("purpose of this agreement", 2),
    ("scope of work", 2),
    ("scope of services", 2),
    ("scope of the services", 2),
    ("scope of the agreement", 2),
    ("description of services", 2),
    ("description of the services", 2),
    ("purpose", 2),
    ("scope", 2),
    ("svrha ugovora", 2),
    ("svrha", 2),
    ("opis usluga", 2),
    ("obim usluga", 2),
    ("background", 1),
];

/// Words before a clause number.
const ARTICLE_WORDS: &[&str] = &["article", "clan", "section", "clause", "tacka", "odeljak"];

/// Abbreviations a period does not end a sentence after (folded).
const ABBREVIATIONS: &[&str] = &[
    "no", "nr", "br", "art", "cl", "st", "str", "tel", "mr", "mrs", "ms", "dr", "inc", "ltd", "co",
    "corp", "e.g", "i.e", "etc", "vs", "sec", "para", "pp", "approx", "d.o.o", "a.d", "ul", "god",
    "tj", "npr", "sl", "itd", "dipl", "ing", "prof", "jan", "feb", "mar", "apr", "jun", "jul",
    "aug", "sep", "sept", "oct", "nov", "dec", "u.s", "s.a", "n.v", "b.v", "fig", "ref",
];

/// Folded words that mark a sentence as a parties block, an address or a signature line.
const BOILERPLATE: &[&str] = &[
    "between",
    "by and between",
    "registered office",
    "registered address",
    "principal place of business",
    "represented by",
    "in witness whereof",
    "hereinafter",
    "sa sedistem",
    "koga zastupa",
    "koje zastupa",
    "maticni broj",
    "pib",
    "tekuci racun",
    "iban",
    "swift",
    "tel",
    "e-mail",
    "email",
    "fax",
    "www",
    "page",
    "strana",
    "signature",
    "potpis",
    "copyright",
    "all rights reserved",
];

/// Short function words (folded) left out of `TextRank` similarity, English and Serbian.
const STOP: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "shall",
    "will",
    "from",
    "are",
    "was",
    "has",
    "have",
    "been",
    "any",
    "all",
    "its",
    "not",
    "such",
    "which",
    "under",
    "upon",
    "into",
    "may",
    "each",
    "other",
    "their",
    "these",
    "those",
    "than",
    "then",
    "also",
    "only",
    "out",
    "our",
    "your",
    "who",
    "whom",
    "per",
    "you",
    "can",
    "but",
    "about",
    "over",
    "between",
    "said",
    "agreement",
    "ili",
    "koji",
    "koja",
    "koje",
    "kao",
    "biti",
    "bude",
    "sto",
    "ovog",
    "ovaj",
    "ove",
    "ova",
    "ovim",
    "ugovora",
    "ugovor",
    "strane",
    "svojih",
    "radi",
    "nije",
    "kod",
    "iz",
    "na",
    "za",
    "od",
    "do",
    "po",
    "se",
    "je",
    "su",
    "da",
    "sa",
    "uz",
];

/// "Amendment No. 2 (no. 114/23-A2)", "Invoice no. 7", "PDF document \"Title\"".
fn head_phrase(card: &FileCard) -> String {
    if let Some(kind) = card.kind.as_deref() {
        let mut s = kind_name(kind).to_owned();
        // The title names the real type under the generic `contract`, or when nothing else
        // (ordinal, number, parent) identifies the document.
        let bare = card.ordinal.is_none() && card.number.is_none() && card.parent().is_none();
        if let Some(t) = card
            .title
            .as_deref()
            .filter(|t| (kind == "contract" || bare) && title_adds(t, kind))
        {
            let _ = write!(s, " \"{}\"", clip(t, 80));
        }
        let ordinal = card.ordinal.filter(|&o| o > 0);
        if let Some(o) = ordinal {
            let _ = write!(s, " No. {o}");
        }
        match (&card.number, ordinal) {
            (Some(n), Some(_)) => {
                let _ = write!(s, " (no. {})", clip(&n.raw, 40));
            }
            (Some(n), None) => {
                let _ = write!(s, " no. {}", clip(&n.raw, 40));
            }
            (None, _) => {}
        }
        s
    } else {
        let mut s = match card.format.as_deref() {
            Some("pdf") => "PDF document".to_owned(),
            Some("docx") => "Word document".to_owned(),
            _ => "Document".to_owned(),
        };
        if let Some(t) = &card.title {
            let _ = write!(s, " \"{}\"", clip(t, 80));
        }
        if let Some(n) = &card.number {
            let _ = write!(s, " no. {}", clip(&n.raw, 40));
        }
        s
    }
}

/// Whether a title says more than the kind name ("SERVICES AGREEMENT" does under `contract`,
/// "Invoice" does not under `invoice`).
fn title_adds(title: &str, kind: &str) -> bool {
    let t = words_of(title);
    let k = words_of(kind_name(kind));
    t.len() > 1
        && !t
            .iter()
            .all(|w| k.contains(w) || w.chars().all(|c| c.is_ascii_digit()))
}

/// The template sentence of the card, in English, ending with a period.
pub fn template_sentence(card: &FileCard) -> String {
    let mut s = head_phrase(card);
    if let Some(p) = card.parent() {
        let rel = match p.relation.as_str() {
            "issued_under" => "under",
            "billed_against" => "against",
            _ => "to",
        };
        let kind = p
            .kind
            .as_deref()
            .map_or("document", kind_name)
            .to_lowercase();
        let _ = write!(s, " {rel} the {kind} {}", clip(&p.raw, 40));
    }
    let named: Vec<String> = card
        .parties
        .iter()
        .take(3)
        .map(|p| match &p.role {
            Some(r) => format!("{} ({})", clip(&p.name, 60), r.replace('_', "-")),
            None => clip(&p.name, 60),
        })
        .collect();
    let others = card.parties_total.saturating_sub(named.len());
    match (named.as_slice(), others) {
        ([], _) => {}
        ([one], 0) => {
            let _ = write!(s, " involving {one}");
        }
        ([a, b], 0) => {
            let _ = write!(s, " between {a} and {b}");
        }
        (all, 0) => {
            let (last, head) = all.split_last().unwrap_or((&all[0], &[]));
            let _ = write!(s, " between {} and {last}", head.join(", "));
        }
        (all, _) => {
            // No count: the card has the exact one, and a number the card does not show
            // would be a claim of its own.
            let _ = write!(s, " between {} and others", all.join(", "));
        }
    }
    let mut tail: Vec<String> = Vec::new();
    for (role, d) in &card.dates {
        let word = match *role {
            "signed" => "signed",
            "effective" => "effective",
            "expires" => "expiring",
            "issued" => "issued",
            "due" => "due",
            _ => "dated",
        };
        tail.push(format!("{word} {}", format::date(*d)));
    }
    for (role, m) in card.amounts.iter().take(2) {
        let word = match *role {
            "total" => "total",
            "net" => "net",
            "gross" => "gross",
            _ => "largest amount",
        };
        tail.push(format!("{word} {}", format::money(*m)));
    }
    if card.flags.iter().any(|(f, _)| *f == "scanned") {
        tail.push("scanned".to_owned());
    }
    if !tail.is_empty() {
        s.push_str(", ");
        s.push_str(&tail.join(", "));
    }
    let mut s = clip(&s, MAX_TEMPLATE_CHARS);
    if !s.ends_with(['.', '…']) {
        s.push('.');
    }
    s
}

/// English name of an instrument kind of `quail-refs`.
pub fn kind_name(kind: &str) -> &str {
    match kind {
        "msa" => "Master services agreement",
        "contract" => "Contract",
        "nda" => "Non-disclosure agreement",
        "sow" => "Statement of work",
        "amendment" => "Amendment",
        "amended_restated" => "Amended and restated agreement",
        "annex" => "Annex",
        "change_order" => "Change order",
        "side_letter" => "Side letter",
        "po" => "Purchase order",
        "order" => "Order",
        "invoice" => "Invoice",
        "quote" => "Quote",
        "framework_agreement" => "Framework agreement",
        "award_decision" => "Contract award decision",
        "decision" => "Decision",
        "price_list" => "Price list",
        "notice" => "Notice",
        "tender_documentation" => "Tender documentation",
        "procurement" => "Procurement",
        "registry" => "Document",
        other => other,
    }
}

/// The first pages of `doc`, normalized, as `(page index, text)`, at most [`MAX_PAGES`] pages
/// and [`MAX_BYTES`] bytes.
pub fn head_pages(doc: &Document) -> Vec<(u32, String)> {
    let opts = NormalizeOptions::default();
    let mut out = Vec::new();
    let mut bytes = 0;
    for p in doc.pages.iter().take(MAX_PAGES) {
        if p.text_source == TextSource::None && p.text.trim().is_empty() {
            continue;
        }
        let mut t = normalize(&p.text, &opts).into_owned();
        if bytes + t.len() > MAX_BYTES {
            let mut end = MAX_BYTES.saturating_sub(bytes);
            while end > 0 && !t.is_char_boundary(end) {
                end -= 1;
            }
            t.truncate(end);
        }
        bytes += t.len();
        out.push((p.index, t));
        if bytes >= MAX_BYTES {
            break;
        }
    }
    out
}

/// The document's subject sentence: a subject clause, else the `TextRank` top sentence.
pub fn subject(pages: &[(u32, String)]) -> Option<Subject> {
    clause(pages).or_else(|| textrank(pages))
}

/// Kinds whose summary is the template sentence alone.
pub const NO_SUBJECT_KINDS: &[&str] = &["invoice", "po", "order", "quote", "price_list"];

/// The summary: template sentence plus the subject sentence. `lang` is the document language
/// (ISO 639-1); a non-English subject is quoted and marked.
pub fn summary(card: &FileCard, subject: Option<&Subject>) -> String {
    let mut s = template_sentence(card);
    // On invoices and orders the card says it all; their prose is terms and conditions.
    let transactional = card
        .kind
        .as_deref()
        .is_some_and(|k| NO_SUBJECT_KINDS.contains(&k));
    if let Some(sub) = subject.filter(|_| !transactional) {
        match card.lang.as_deref() {
            Some(l) if l != "en" => {
                let _ = write!(s, " Subject ({l}, original): \"{}\"", sub.text);
            }
            _ => {
                s.push_str(" Subject: ");
                s.push_str(&sub.text);
                if !sub.text.ends_with(['.', '!', '?', '…']) {
                    s.push('.');
                }
            }
        }
    }
    s
}

/// Folds a line into words: lowercase, no diacritics, Latin, punctuation as spaces.
fn words_of(line: &str) -> Vec<String> {
    let f = fold(line, &FoldOptions::KEY);
    f.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_number_word(w: &str) -> bool {
    w.chars().all(|c| c.is_ascii_digit())
        || (w.len() <= 4 && w.chars().all(|c| matches!(c, 'i' | 'v' | 'x')))
}

/// What a line says as a heading: `(label, strength, words after the label)`.
fn heading(line: &str) -> Option<(&'static str, u8, usize)> {
    let ws = words_of(line);
    let mut i = 0;
    let mut article = false;
    if ws
        .first()
        .is_some_and(|w| ARTICLE_WORDS.contains(&w.as_str()))
    {
        i = 1;
        article = true;
    }
    while ws.get(i).is_some_and(|w| is_number_word(w)) {
        i += 1;
    }
    if article && i == 1 {
        return None; // "Article" without a number is prose
    }
    let rest = &ws[i..];
    for &(label, strength) in LABELS {
        let lw: Vec<&str> = label.split(' ').collect();
        if rest.len() >= lw.len() && rest.iter().zip(&lw).all(|(a, b)| a == b) {
            let after = rest.len() - lw.len();
            // A label that runs on into prose is a heading only when a colon or dash ends it.
            let labelled = line.contains([':', '–', '—']);
            return (after <= 3 || labelled).then_some((label, strength, after));
        }
    }
    if article && rest.is_empty() {
        return ws
            .get(1)
            .is_some_and(|w| w == "1" || w == "i")
            .then_some(("article 1", 1, 0));
    }
    None
}

/// A line that starts the next clause: "Article 2", "2.", "Član 3", or a short line without a
/// final period.
fn ends_clause(line: &str) -> bool {
    let ws = words_of(line);
    let Some(first) = ws.first() else {
        return false;
    };
    let numbered = ARTICLE_WORDS.contains(&first.as_str())
        || (is_number_word(first) && line.trim_start().starts_with(|c: char| c.is_ascii_digit()));
    numbered && ws.len() <= 8 && !line.trim_end().ends_with(['.', ',', ';'])
}

fn clause(pages: &[(u32, String)]) -> Option<Subject> {
    let mut best: Option<(u8, Subject)> = None;
    for (page, text) in pages {
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.len() > 200 {
                continue;
            }
            let Some((label, strength, after)) = heading(trimmed) else {
                continue;
            };
            if best.as_ref().is_some_and(|(s, _)| *s >= strength) {
                continue;
            }
            // The content: after a colon on the same line, or the next lines.
            let mut content = String::new();
            if let Some((_, tail)) = trimmed.split_once([':', '–', '—'])
                && !tail.trim().is_empty()
                && after > 0
            {
                content.push_str(tail.trim());
            } else if after > 3 {
                continue;
            }
            let mut j = i + 1;
            let mut strength = strength;
            let mut label = label;
            // "Article 1" followed by its title line ("Subject of the Agreement").
            if content.is_empty()
                && let Some(next) = lines.get(j)
                && let Some((l2, s2, 0)) = heading(next.trim())
            {
                label = l2;
                strength = strength.max(s2);
                j += 1;
            }
            while let Some(next) = lines.get(j) {
                if content.len() > 900 || (!content.is_empty() && ends_clause(next)) {
                    break;
                }
                if heading(next.trim()).is_some_and(|(_, _, a)| a == 0) && !content.is_empty() {
                    break;
                }
                if !content.is_empty() {
                    content.push(' ');
                }
                content.push_str(next.trim());
                j += 1;
            }
            let Some(sentence) = first_sentence(&content) else {
                continue;
            };
            best = Some((
                strength,
                Subject {
                    text: sentence,
                    page: *page,
                    method: SubjectMethod::Clause,
                    label: Some(label),
                },
            ));
            if strength >= 3 {
                return best.map(|(_, s)| s);
            }
        }
    }
    best.map(|(_, s)| s)
}

/// The first full sentence of `text` with at least four words, at most
/// [`MAX_SUBJECT_WORDS`] words.
fn first_sentence(text: &str) -> Option<String> {
    for s in sentences(text) {
        let s = strip_enumerator(&s);
        if is_definition(&s) {
            return None;
        }
        let n = s.split_whitespace().count();
        if n >= 4 && s.len() >= 20 {
            return Some(limit_words(&s, MAX_SUBJECT_WORDS));
        }
    }
    None
}

fn limit_words(s: &str, max: usize) -> String {
    let ws: Vec<&str> = s.split_whitespace().collect();
    if ws.len() <= max {
        return ws.join(" ");
    }
    let mut out = ws[..max].join(" ");
    out = out.trim_end_matches([',', ';', ':']).to_owned();
    out.push('…');
    out
}

/// Splits `text` into sentences at `.`, `!`, `?` (and `;` far into a long sentence) followed by
/// whitespace and an upper-case letter, a digit or a quote, except after abbreviations, single
/// letters and numbers.
fn sentences(text: &str) -> Vec<String> {
    let text = one_line(text);
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (n, &(i, c)) in chars.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let next = chars.get(n + 1).map(|&(_, c)| c);
        let after = chars.get(n + 2).map(|&(_, c)| c);
        let boundary = match (next, after) {
            (None, _) => true,
            (Some(' '), Some(a)) => a.is_uppercase() || a.is_ascii_digit() || a == '"' || a == '(',
            _ => false,
        };
        if !boundary {
            continue;
        }
        if c == '.' {
            let word = text[start..i]
                .rsplit(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or("");
            let fw = fold(word, &FoldOptions::KEY);
            let fw = fw.trim_matches(|c: char| !c.is_alphanumeric());
            if fw.chars().count() <= 1
                || fw.chars().all(|c| c.is_ascii_digit() || c == '.')
                || ABBREVIATIONS.contains(&fw)
            {
                continue;
            }
        }
        let s = text[start..=i].trim();
        if !s.is_empty() {
            out.push(s.to_owned());
        }
        start = i + c.len_utf8();
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

/// A sentence worth ranking: 8–60 words, mostly letters, not shouting, no boilerplate.
fn rankable(s: &str) -> bool {
    let n = s.split_whitespace().count();
    if !(8..=60).contains(&n) {
        return false;
    }
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    let upper = s.chars().filter(|c| c.is_uppercase()).count();
    let total = s.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 || letters * 10 < total * 7 || upper * 2 > letters {
        return false;
    }
    if s.contains("/s/") || s.contains("By:") || s.contains("Name:") || is_definition(s) {
        return false;
    }
    let f = format!(" {} ", words_of(s).join(" "));
    !BOILERPLATE.iter().any(|b| f.contains(&format!(" {b} ")))
}

/// A definition ("\"Affiliate\" shall mean ...") says nothing about the subject.
fn is_definition(s: &str) -> bool {
    let f = format!(" {} ", words_of(s).join(" "));
    [
        " shall mean ",
        " means ",
        " shall have the meaning",
        " respective meanings ",
        " have the meanings ",
        " is defined ",
        " are defined ",
        " terms defined ",
        " for the purposes of this ",
        " for purposes of this ",
    ]
    .iter()
    .any(|d| f.contains(d))
}

/// Folded phrases of sentences that state what a document does: who provides what to whom.
const CUES: &[&str] = &[
    "agrees to",
    "agree to",
    "shall provide",
    "will provide",
    "to provide",
    "shall perform",
    "hereby",
    "engage",
    "engages",
    "appoint",
    "appoints",
    "grants",
    "purpose",
    "desires to",
    "wishes to",
    "whereas",
    "set forth the terms",
    "sets forth the terms",
    "pruza usluge",
    "obavezuje se",
    "isporuc",
];

fn has_cue(s: &str) -> bool {
    let f = format!(" {} ", words_of(s).join(" "));
    CUES.iter().any(|c| f.contains(&format!(" {c}")))
}

/// Drops leading clause numbers and labels: "1.1 ", "12.2.3 ", "(v) ", "Exhibit 10.39 ",
/// "Section 2. ", "ARTICLE I ".
fn strip_enumerator(s: &str) -> String {
    let mut ws: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < ws.len().min(4) {
        let w = ws[i];
        let bare = w.trim_matches(|c: char| matches!(c, '(' | ')' | '.' | ':' | '-'));
        let lower = bare.to_lowercase();
        let numbering = !bare.is_empty()
            && (bare.chars().all(|c| c.is_ascii_digit() || c == '.')
                || (w.starts_with('(') && w.ends_with(')') && bare.len() <= 4)
                || (bare.len() <= 4
                    && bare
                        .chars()
                        .all(|c| matches!(c, 'i' | 'v' | 'x' | 'I' | 'V' | 'X'))
                    && i > 0));
        let label = matches!(
            lower.as_str(),
            "exhibit" | "section" | "article" | "clause" | "schedule" | "annex" | "član" | "clan"
        ) && ws.get(i + 1).is_some_and(|n| {
            n.chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c.is_uppercase())
        });
        if numbering {
            i += 1;
        } else if label {
            i += 2;
        } else {
            break;
        }
    }
    ws.drain(..i.min(ws.len()));
    ws.join(" ")
}

fn textrank(pages: &[(u32, String)]) -> Option<Subject> {
    let mut cands: Vec<(u32, String, Vec<String>)> = Vec::new();
    'pages: for (page, text) in pages {
        // Paragraphs: lines joined until a blank-ish break; sentences within.
        for s in sentences(&text.replace('\n', " ")) {
            let s = strip_enumerator(&s);
            if !rankable(&s) {
                continue;
            }
            let mut ws: Vec<String> = words_of(&s)
                .into_iter()
                .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
                .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
                .collect();
            ws.sort_unstable();
            ws.dedup();
            if ws.len() < 3 {
                continue;
            }
            cands.push((*page, s, ws));
            if cands.len() >= MAX_RANKED {
                break 'pages;
            }
        }
    }
    if cands.is_empty() {
        return None;
    }
    let n = cands.len();
    #[allow(clippy::cast_precision_loss)] // word counts are small
    let sim = |a: &[String], b: &[String]| -> f64 {
        let common = a.iter().filter(|w| b.binary_search(w).is_ok()).count();
        if common == 0 {
            return 0.0;
        }
        let d = (a.len() as f64).ln() + (b.len() as f64).ln();
        if d <= 0.0 { 0.0 } else { common as f64 / d }
    };
    let mut w = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let s = sim(&cands[i].2, &cands[j].2);
            w[i][j] = s;
            w[j][i] = s;
        }
    }
    let out: Vec<f64> = w.iter().map(|r| r.iter().sum()).collect();
    // Personalized `PageRank` with a position prior: earlier sentences are likelier to state
    // what the document is about.
    #[allow(clippy::cast_precision_loss)]
    let prior: Vec<f64> = {
        let raw: Vec<f64> = (0..n)
            .map(|i| {
                let cue = if has_cue(&cands[i].1) { 3.0 } else { 1.0 };
                cue / (1.0 + i as f64 / 8.0)
            })
            .collect();
        let s: f64 = raw.iter().sum();
        raw.into_iter().map(|x| x / s).collect()
    };
    let mut score = prior.clone();
    let d = 0.85;
    for _ in 0..30 {
        let mut next = vec![0.0; n];
        for i in 0..n {
            let mut acc = 0.0;
            for j in 0..n {
                if w[j][i] > 0.0 && out[j] > 0.0 {
                    acc += w[j][i] / out[j] * score[j];
                }
            }
            next[i] = (1.0 - d) * prior[i] + d * acc;
        }
        score = next;
    }
    let best = (0..n).max_by(|&a, &b| score[a].total_cmp(&score[b]).then(b.cmp(&a)))?;
    let (page, s, _) = &cands[best];
    Some(Subject {
        text: limit_words(s, MAX_SUBJECT_WORDS),
        page: *page,
        method: SubjectMethod::TextRank,
        label: None,
    })
}

/// Capitalized kind name for display.
pub fn kind_title(kind: &str) -> String {
    capitalize(kind_name(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(t: &str) -> Vec<(u32, String)> {
        vec![(0, t.to_owned())]
    }

    #[test]
    fn splits_sentences_around_abbreviations() {
        let s = sentences("Contract No. 5 is signed. The Supplier d.o.o. Beograd delivers. Done");
        assert_eq!(
            s,
            vec![
                "Contract No. 5 is signed.",
                "The Supplier d.o.o. Beograd delivers.",
                "Done"
            ]
        );
    }

    #[test]
    fn finds_an_english_subject_clause() {
        let t = "MASTER SERVICES AGREEMENT\nThis Agreement is made between A Ltd and B Inc.\n\
                 Article 1\nSubject of the Agreement\nThe Supplier shall design and maintain a \
                 warehouse management system for the Customer. Further terms apply.\nArticle 2\n\
                 Price\nThe price is fixed.";
        let s = subject(&pages(t)).unwrap();
        assert_eq!(s.method, SubjectMethod::Clause);
        assert_eq!(s.label, Some("subject of the agreement"));
        assert_eq!(
            s.text,
            "The Supplier shall design and maintain a warehouse management system for the Customer."
        );
    }

    #[test]
    fn finds_a_serbian_cyrillic_clause_and_a_colon_label() {
        let t = "УГОВОР О ЗАКУПУ\nЧлан 1.\nПредмет уговора је закуп магацинског простора у \
                 Новом Саду, површине 1.200 м2. Остало.\n";
        let s = subject(&pages(t)).unwrap();
        assert_eq!(s.label, Some("article 1"));
        assert!(s.text.starts_with("Предмет уговора је закуп"), "{}", s.text);

        let t = "Scope of Work: Consultant will provide market research services in Germany. More.";
        let s = subject(&pages(t)).unwrap();
        assert_eq!(s.label, Some("scope of work"));
        assert_eq!(
            s.text,
            "Consultant will provide market research services in Germany."
        );
    }

    #[test]
    fn falls_back_to_textrank() {
        let t = "Quarterly report on the logistics network.\n\
                 The logistics network expanded to three new warehouses in the northern region this quarter.\n\
                 Warehouse capacity in the northern region grew while logistics costs stayed flat overall.\n\
                 Lunch was served at noon in the main hall for all of the visiting guests today.";
        let s = subject(&pages(t)).unwrap();
        assert_eq!(s.method, SubjectMethod::TextRank);
        assert!(
            s.text.contains("warehouse") || s.text.contains("Warehouse"),
            "{}",
            s.text
        );
    }

    #[test]
    fn strips_clause_numbers_and_skips_definitions() {
        assert_eq!(
            strip_enumerator("1.1 Cargill agrees to perform"),
            "Cargill agrees to perform"
        );
        assert_eq!(
            strip_enumerator("(v) The provisions apply"),
            "The provisions apply"
        );
        assert_eq!(
            strip_enumerator("Exhibit 10.39 12.2.3 Distributor shall"),
            "Distributor shall"
        );
        assert_eq!(
            strip_enumerator("Section 2. Term of the lease"),
            "Term of the lease"
        );
        assert_eq!(strip_enumerator("The Supplier shall"), "The Supplier shall");
        assert!(is_definition(
            "\"Affiliate\" shall mean any entity controlling a party."
        ));
        assert!(!is_definition(
            "The Supplier shall deliver the goods to the Buyer."
        ));
    }

    #[test]
    fn prose_mentioning_purpose_is_not_a_heading() {
        assert!(heading("Purpose of the agreement").is_some());
        assert!(
            heading("Purpose of this meeting was to agree on the next steps of the plan").is_none()
        );
        assert!(heading("Article").is_none());
        assert_eq!(heading("ČLAN 1").map(|h| h.0), Some("article 1"));
    }
}

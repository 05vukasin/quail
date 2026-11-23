//! Fact consistency of a generated summary against what the generator was given.
//!
//! Every **date**, **number**, **identifier** and **name word** of the output must be found in
//! the sources (the card text, the document text the model saw and, in `summary.llm`, the
//! whole document text, [`document_sources`]), after normalization:
//!
//! - dates: ISO `2023-09-12`, `12.09.2023`, `09/12/2023` (both readings), `12 September 2023`,
//!   `September 12, 2023`, `September 2023`, Serbian month names in Latin or Cyrillic
//!   (`12. septembra 2023.`); in the sources also two-digit years (`03/27/15`, `27.03.15`;
//!   `69`–`99` are 19xx, the rest 20xx); compared as calendar dates;
//! - numbers: `4,200.00` = `4.200,00` = `4200`; years, days and months of source dates count
//!   as numbers; `24-month`, `3rd` check the number only;
//! - identifiers: tokens mixing letters and digits (`SA-2026/0417`, `DE136695976`), compared by
//!   their upper-case alphanumeric key;
//! - name words: capitalized words that are not common English words (and not the first word
//!   of a sentence on their own), compared folded (case, diacritics, script). A word also
//!   passes as the English translation of a source word in a short list of institution and
//!   business words (`Fund` for `фонд`, `Health` for `здравствено`; Serbian, Croatian and
//!   Bosnian), and, from 8 letters, one OCR error away from a source word (`Pharmalogix` for
//!   `Pharmalolgix`). An invented name still has to be in the sources as written.
//!
//! The deterministic card and extractive summary pass by construction: they only copy values.
//! A failed check makes the LLM summary fall back to the extractive one.

use std::collections::{BTreeSet, HashSet};

use quail_core::Document;
use quail_text::fold::{FoldOptions, fold};

/// The verdict of [`check`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactCheck {
    /// Claims checked.
    pub claims: usize,
    /// Claims not found in the sources, as `kind:value` (`date:2023-09-13`, `number:4300`,
    /// `id:SA2026`, `name:Acme`).
    pub unsupported: Vec<String>,
}

impl FactCheck {
    /// No unsupported claim.
    pub fn passed(&self) -> bool {
        self.unsupported.is_empty()
    }
}

/// Bytes of document text [`document_sources`] gives by default: about 1,000 pages, a few
/// hundred milliseconds of indexing.
pub const DOCUMENT_SOURCE_BYTES: usize = 4 << 20;

/// The page texts of `doc` (the first pages, at most `max_bytes` of them), as sources for
/// [`check`] besides what the model read: a claim the document makes passes even when the
/// digest left it out.
pub fn document_sources(doc: &Document, max_bytes: usize) -> Vec<&str> {
    let mut left = max_bytes;
    let mut out = Vec::new();
    for p in &doc.pages {
        if p.text.len() > left {
            break;
        }
        left -= p.text.len();
        if !p.text.trim().is_empty() {
            out.push(p.text.as_str());
        }
    }
    out
}

/// Checks every claim of `output` against `sources`. `names` turns the name-word check on
/// (off for translated titles, whose words are English by design).
pub fn check(output: &str, sources: &[&str], names: bool) -> FactCheck {
    let src = Index::build(sources);
    let mut fc = FactCheck::default();
    let claims = claims(output);
    for c in claims {
        let ok = match &c {
            Claim::Date(y, m, d) => src.has_date(*y, *m, *d),
            Claim::AmbiguousDate(a, b) => {
                src.has_date(a.0, a.1, a.2) || src.has_date(b.0, b.1, b.2)
            }
            Claim::Number(n) => src.numbers.contains(n),
            Claim::Id(id) => src.ids.contains(id) || src.id_blob.contains(id.as_str()),
            Claim::Name(w) => {
                if !names {
                    continue;
                }
                src.has_name_word(w)
            }
        };
        fc.claims += 1;
        if !ok {
            fc.unsupported.push(c.label());
        }
    }
    fc.unsupported.sort();
    fc.unsupported.dedup();
    fc
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Claim {
    Date(i32, u32, u32),
    AmbiguousDate((i32, u32, u32), (i32, u32, u32)),
    Number(String),
    Id(String),
    Name(String),
}

impl Claim {
    fn label(&self) -> String {
        match self {
            Self::Date(y, m, d) | Self::AmbiguousDate((y, m, d), _) => {
                if *d == 0 {
                    format!("date:{y:04}-{m:02}")
                } else {
                    format!("date:{y:04}-{m:02}-{d:02}")
                }
            }
            Self::Number(n) => format!("number:{n}"),
            Self::Id(i) => format!("id:{i}"),
            Self::Name(w) => format!("name:{w}"),
        }
    }
}

/// What the sources contain.
struct Index {
    dates: HashSet<(i32, u32, u32)>,
    numbers: HashSet<String>,
    ids: HashSet<String>,
    id_blob: String,
    /// Folded words, sorted (prefix lookups for [`TRANSLATIONS`] and [`Index::near`]).
    words: BTreeSet<String>,
}

impl Index {
    fn build(sources: &[&str]) -> Self {
        let mut ix = Index {
            dates: HashSet::new(),
            numbers: HashSet::new(),
            ids: HashSet::new(),
            id_blob: String::new(),
            words: BTreeSet::new(),
        };
        for s in sources {
            for c in claims_all(s) {
                match c {
                    Claim::Date(y, m, d) => ix.add_date(y, m, d),
                    Claim::AmbiguousDate(a, b) => {
                        ix.add_date(a.0, a.1, a.2);
                        ix.add_date(b.0, b.1, b.2);
                    }
                    Claim::Number(n) => {
                        ix.numbers.insert(n);
                    }
                    Claim::Id(i) => {
                        ix.ids.insert(i);
                    }
                    Claim::Name(_) => {}
                }
            }
            for tok in s.split_whitespace() {
                let key = id_key(tok);
                if !key.is_empty() {
                    ix.id_blob.push_str(&key);
                    ix.id_blob.push(' ');
                }
                // Every digit run is a number too ("No.5", "A2").
                for run in tok
                    .split(|c: char| !c.is_ascii_digit())
                    .filter(|r| !r.is_empty())
                {
                    ix.numbers.insert(trim_zeros(run).to_owned());
                }
            }
            let norm = quail_text::normalize::normalize(
                s,
                &quail_text::normalize::NormalizeOptions::default(),
            );
            let folded = fold(&norm, &FoldOptions::KEY);
            for w in folded.split(|c: char| !c.is_alphanumeric()) {
                if !w.is_empty() {
                    ix.words.insert(w.to_owned());
                }
            }
        }
        ix
    }

    fn add_date(&mut self, y: i32, m: u32, d: u32) {
        self.dates.insert((y, m, d));
        self.dates.insert((y, m, 0));
        self.numbers.insert(y.to_string());
        self.numbers.insert(m.to_string());
        if d > 0 {
            self.numbers.insert(d.to_string());
        }
    }

    fn has_date(&self, y: i32, m: u32, d: u32) -> bool {
        self.dates.contains(&(y, m, d))
    }

    /// A folded name word `w` of the output is in the sources: as is, as the English
    /// translation of a source word ([`TRANSLATIONS`]), or one OCR error away ([`Self::near`]).
    fn has_name_word(&self, w: &str) -> bool {
        self.words.contains(w) || self.translated(w) || self.near(w)
    }

    /// Some source word starts with a stem `w` translates ("fund" ← "fond", "fonda"; "health"
    /// ← "zdravstveno", "zdravlja"). Plural `-s` / `-ies` read as the singular.
    fn translated(&self, w: &str) -> bool {
        let singular = w
            .strip_suffix("ies")
            .map(|b| format!("{b}y"))
            .or_else(|| w.strip_suffix('s').map(str::to_owned));
        let stems = |key: &str| {
            TRANSLATIONS
                .iter()
                .find(|(en, _)| *en == key)
                .map_or(&[][..], |(_, s)| *s)
        };
        let mut all = stems(w)
            .iter()
            .chain(singular.as_deref().map_or(&[][..], stems).iter());
        all.any(|stem| self.has_prefix(stem))
    }

    fn has_prefix(&self, stem: &str) -> bool {
        self.words
            .range::<str, _>((std::ops::Bound::Included(stem), std::ops::Bound::Unbounded))
            .next()
            .is_some_and(|w| w.starts_with(stem))
    }

    /// A source word one edit (insertion, deletion, substitution or swap of neighbours) from
    /// `w`, for words of at least [`NEAR_MIN_CHARS`] letters with the same first letter: the
    /// model reads "Pharmalolgix" (an OCR error) and writes "Pharmalogix". Short words, where
    /// one letter makes another name, need an exact match.
    fn near(&self, w: &str) -> bool {
        let a: Vec<char> = w.chars().collect();
        if a.len() < NEAR_MIN_CHARS || !a.iter().all(|c| c.is_alphabetic()) {
            return false;
        }
        let mut buf = [0u8; 4];
        let first: &str = a[0].encode_utf8(&mut buf);
        self.words
            .range::<str, _>((std::ops::Bound::Included(first), std::ops::Bound::Unbounded))
            .take_while(|s| s.starts_with(first))
            .any(|s| {
                let b: Vec<char> = s.chars().collect();
                b.len().abs_diff(a.len()) <= 1 && one_edit(&a, &b)
            })
    }
}

/// Letters of a name word from which [`Index::near`] accepts one OCR error.
const NEAR_MIN_CHARS: usize = 8;

/// `a` and `b` differ by at most one insertion, deletion, substitution or swap of two
/// neighbouring letters.
fn one_edit(a: &[char], b: &[char]) -> bool {
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if p == a.len() && p == b.len() {
        return true;
    }
    let (ra, rb) = (&a[p..], &b[p..]);
    let tail = |x: &[char], y: &[char]| x == y;
    match ra.len().cmp(&rb.len()) {
        std::cmp::Ordering::Equal => {
            tail(&ra[1..], &rb[1..])
                || (ra.len() >= 2 && ra[0] == rb[1] && ra[1] == rb[0] && tail(&ra[2..], &rb[2..]))
        }
        std::cmp::Ordering::Less => tail(ra, &rb[1..]),
        std::cmp::Ordering::Greater => tail(&ra[1..], rb),
    }
}

/// English, then Serbian (folded Latin) month names; index + 1 is the month.
const MONTHS: &[(&str, u32)] = &[
    ("january", 1),
    ("jan", 1),
    ("february", 2),
    ("feb", 2),
    ("march", 3),
    ("mar", 3),
    ("april", 4),
    ("apr", 4),
    ("may", 5),
    ("june", 6),
    ("jun", 6),
    ("july", 7),
    ("jul", 7),
    ("august", 8),
    ("aug", 8),
    ("september", 9),
    ("sep", 9),
    ("sept", 9),
    ("october", 10),
    ("oct", 10),
    ("november", 11),
    ("nov", 11),
    ("december", 12),
    ("dec", 12),
    ("januar", 1),
    ("januara", 1),
    ("februar", 2),
    ("februara", 2),
    ("mart", 3),
    ("marta", 3),
    ("aprila", 4),
    ("maj", 5),
    ("maja", 5),
    ("juna", 6),
    ("jula", 7),
    ("avgust", 8),
    ("avgusta", 8),
    ("septembar", 9),
    ("septembra", 9),
    ("oktobar", 10),
    ("oktobra", 10),
    ("novembar", 11),
    ("novembra", 11),
    ("decembar", 12),
    ("decembra", 12),
];

/// Common English words that are capitalized in summaries without being names.
const COMMON: &[&str] = &[
    "the",
    "this",
    "that",
    "these",
    "those",
    "it",
    "its",
    "a",
    "an",
    "in",
    "on",
    "of",
    "for",
    "to",
    "by",
    "with",
    "under",
    "between",
    "and",
    "or",
    "as",
    "at",
    "from",
    "is",
    "are",
    "was",
    "be",
    "no",
    "not",
    "all",
    "any",
    "each",
    "both",
    "per",
    "via",
    "upon",
    "after",
    "before",
    "agreement",
    "agreements",
    "contract",
    "contracts",
    "amendment",
    "amendments",
    "annex",
    "annexes",
    "invoice",
    "invoices",
    "order",
    "orders",
    "purchase",
    "master",
    "services",
    "service",
    "statement",
    "work",
    "works",
    "non",
    "disclosure",
    "nda",
    "msa",
    "sow",
    "po",
    "party",
    "parties",
    "supplier",
    "customer",
    "client",
    "vendor",
    "buyer",
    "seller",
    "company",
    "companies",
    "lessor",
    "lessee",
    "landlord",
    "tenant",
    "provider",
    "consultant",
    "contractor",
    "licensor",
    "licensee",
    "employer",
    "employee",
    "distributor",
    "manufacturer",
    "article",
    "section",
    "clause",
    "schedule",
    "exhibit",
    "appendix",
    "attachment",
    "document",
    "documents",
    "lease",
    "license",
    "licence",
    "framework",
    "decision",
    "notice",
    "price",
    "prices",
    "list",
    "quote",
    "quotation",
    "tender",
    "change",
    "side",
    "letter",
    "effective",
    "date",
    "dated",
    "term",
    "terms",
    "total",
    "subject",
    "purpose",
    "scope",
    "summary",
    "title",
    "english",
    "serbian",
    "croatian",
    "german",
    "monthly",
    "annual",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "eur",
    "usd",
    "rsd",
    "gbp",
    "chf",
    "vat",
    "pib",
    "mb",
    "iban",
    "inc",
    "ltd",
    "llc",
    "plc",
    "corp",
    "corporation",
    "limited",
    "gmbh",
    "ag",
    "doo",
    "ad",
    "public",
    "procurement",
    "award",
    "supply",
    "delivery",
    "goods",
    "products",
    "product",
    "software",
    "hardware",
    "support",
    "maintenance",
    "consulting",
    "research",
    "development",
    "clinical",
    "study",
    "trial",
    "project",
    "phase",
    "program",
    "programme",
    "plan",
    "budget",
    "fee",
    "fees",
    "payment",
    "payments",
    "rate",
    "rates",
    "net",
    "gross",
    "amount",
    "amounts",
    "addendum",
    "renewal",
    "extension",
    "termination",
    "confidentiality",
    "data",
    "protection",
    "processing",
    "addition",
    "additional",
    "new",
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "parent",
    "related",
    "original",
    "amended",
    "restated",
    "signed",
    "executed",
    "warehouse",
    "office",
    "space",
    "premises",
    "republic",
    "serbia",
    "europe",
    "eu",
    "uk",
    "us",
    "usa",
    "state",
    "states",
    "united",
    "government",
    "ministry",
    "city",
    "municipality",
];

/// English words of institution and business names with the stems of their Serbian,
/// Croatian and Bosnian equivalents (folded: Latin, no diacritics, `đ` → `dj`). A summary of a
/// non-English document translates names ("Републички фонд за здравствено осигурање" →
/// "Republic Health Insurance Fund"); a name word passes when the source has a word starting
/// with one of its stems. Only words of the list do: an invented name ("Medilink") still has to
/// be in the sources as written.
const TRANSLATIONS: &[(&str, &[&str])] = &[
    ("administration", &["uprav", "administracij"]),
    ("agency", &["agencij"]),
    ("agriculture", &["poljopriv"]),
    ("agricultural", &["poljopriv"]),
    ("airport", &["aerodrom", "zracn"]),
    ("army", &["vojsk"]),
    ("assembly", &["skupstin"]),
    ("authority", &["organ", "uprav", "agencij", "vlast"]),
    ("bank", &["bank"]),
    ("belgrade", &["beograd"]),
    ("blood", &["krv"]),
    ("bosnia", &["bosn"]),
    ("broadcasting", &["radiodifuz"]),
    ("business", &["privred", "poslov"]),
    ("care", &["zastit", "nega", "njeg"]),
    ("center", &["centar", "centr"]),
    ("centre", &["centar", "centr"]),
    ("chamber", &["komor"]),
    ("clinic", &["klinik"]),
    ("commerce", &["trgov", "privred"]),
    ("commercial", &["komercij", "trgov"]),
    ("council", &["savet", "vijec", "vece"]),
    ("court", &["sud"]),
    ("croatia", &["hrvat"]),
    ("culture", &["kultur"]),
    ("customs", &["carin"]),
    ("defence", &["odbran", "obran"]),
    ("defense", &["odbran", "obran"]),
    ("device", &["sredst", "uredjaj", "aparat", "pomagal"]),
    ("directorate", &["direkcij", "uprav"]),
    ("disability", &["invalid"]),
    ("drug", &["lek", "lijek"]),
    ("economic", &["privred", "ekonom", "gospodar"]),
    ("economy", &["privred", "ekonom", "gospodar"]),
    ("education", &["obrazov", "prosvet"]),
    ("electric", &["elektr"]),
    ("electricity", &["elektr", "struj"]),
    ("emergency", &["hitn", "vanred"]),
    ("employment", &["zaposl"]),
    ("energy", &["energ"]),
    ("enterprise", &["preduzec", "poduzec"]),
    ("environment", &["zivotn", "okolis"]),
    ("environmental", &["zivotn", "okolis"]),
    ("equipment", &["oprem"]),
    ("faculty", &["fakultet"]),
    ("finance", &["finansij", "financij"]),
    ("financial", &["finansij", "financij"]),
    ("fund", &["fond"]),
    ("gas", &["gas", "plin"]),
    ("health", &["zdravstv", "zdravlj"]),
    ("healthcare", &["zdravstv"]),
    ("herzegovina", &["hercegovin"]),
    ("hospital", &["bolnic"]),
    ("industry", &["industrij", "privred"]),
    ("institute", &["institut", "zavod"]),
    ("institution", &["ustanov", "institucij"]),
    ("insurance", &["osigur"]),
    ("interior", &["unutrasnj"]),
    ("justice", &["pravd"]),
    ("lottery", &["lutrij"]),
    ("medical", &["medicin", "lekar", "ljekar"]),
    ("medicine", &["lek", "lijek", "medicin"]),
    ("military", &["vojn"]),
    ("montenegro", &["crn"]),
    ("municipal", &["opstin", "komunaln"]),
    ("national", &["nacional", "narodn", "drzavn"]),
    ("parliament", &["skupstin", "sabor"]),
    ("pension", &["penzij", "mirovin"]),
    ("pharmaceutical", &["farmac", "lek"]),
    ("pharmacy", &["apotek", "ljekarn"]),
    ("police", &["polic"]),
    ("post", &["posta", "postansk"]),
    ("power", &["elektr", "energ"]),
    ("primary", &["primarn"]),
    ("province", &["pokrajin"]),
    ("provincial", &["pokrajin"]),
    ("railway", &["zeleznic", "zeljeznic"]),
    ("regional", &["regionaln", "pokrajin"]),
    ("registry", &["registr"]),
    ("school", &["skol"]),
    ("science", &["nauk", "naucn", "znanost"]),
    ("security", &["bezbed", "sigurn"]),
    ("social", &["socijal"]),
    ("statistical", &["statist"]),
    ("statistics", &["statist"]),
    ("supply", &["snabd", "nabav", "isporuk"]),
    ("tax", &["porez"]),
    ("telecommunications", &["telekom"]),
    ("television", &["televiz"]),
    ("town", &["grad"]),
    ("trade", &["trgov"]),
    ("transfusion", &["transfuz"]),
    ("transport", &["saobrac", "transport", "prevoz", "promet"]),
    ("treasury", &["trezor"]),
    ("university", &["univerzitet", "sveucilist"]),
    ("utility", &["komunaln"]),
    (
        "water",
        &["voda", "vode", "vodu", "vodom", "vodn", "vodoprivred"],
    ),
    ("waterworks", &["vodovod"]),
];

/// Claims of a text being checked (names included, sentence starts skipped).
fn claims(text: &str) -> Vec<Claim> {
    extract(text, true)
}

/// Every date, number and identifier of a source.
fn claims_all(text: &str) -> Vec<Claim> {
    extract(text, false)
}

fn extract(text: &str, with_names: bool) -> Vec<Claim> {
    // NFKC and typography as everywhere else (`™` → `TM`); card lines are
    // `role=value; role=value`.
    let text =
        quail_text::normalize::normalize(text, &quail_text::normalize::NormalizeOptions::default())
            .replace(['=', ';'], " ");
    let raw: Vec<&str> = text.split_whitespace().collect();
    let toks: Vec<String> = raw.iter().map(|t| strip(t).to_owned()).collect();
    let folded: Vec<String> = toks
        .iter()
        .map(|t| fold(t, &FoldOptions::KEY).trim_end_matches('.').to_owned())
        .collect();
    let mut out = Vec::new();
    let mut used = vec![false; toks.len()];
    // Month-name dates first.
    for i in 0..toks.len() {
        let Some(m) = month(&folded[i]) else { continue };
        // d Month yyyy
        if i >= 1
            && let (Some(d), Some(y)) =
                (day(&folded[i - 1]), folded.get(i + 1).and_then(|t| year(t)))
            && !used[i - 1]
        {
            out.push(Claim::Date(y, m, d));
            used[i - 1] = true;
            used[i] = true;
            used[i + 1] = true;
            continue;
        }
        // Month d, yyyy
        if let (Some(d), Some(y)) = (
            folded.get(i + 1).and_then(|t| day(t)),
            folded.get(i + 2).and_then(|t| year(t)),
        ) {
            out.push(Claim::Date(y, m, d));
            used[i..=i + 2].iter_mut().for_each(|u| *u = true);
            continue;
        }
        // Month yyyy
        if let Some(y) = folded.get(i + 1).and_then(|t| year(t)) {
            out.push(Claim::Date(y, m, 0));
            used[i] = true;
            used[i + 1] = true;
        }
    }
    let mut sentence_start = true;
    for (i, t) in toks.iter().enumerate() {
        let starts = sentence_start;
        sentence_start = raw[i].ends_with(['.', '!', '?', ':']) && !raw[i].ends_with("..");
        if used[i] || t.is_empty() {
            continue;
        }
        // Two-digit years (`03/27/15`) only on the source side: a summary writes the year.
        if let Some(c) = numeric_date(t, !with_names) {
            out.push(c);
            continue;
        }
        let has_digit = t.chars().any(|c| c.is_ascii_digit());
        if has_digit {
            out.extend(numberish(t));
            continue;
        }
        if with_names && is_name_word(t) {
            let next_cap = toks.get(i + 1).is_some_and(|n| is_name_word(n));
            let prev_cap = i > 0 && !starts && toks.get(i - 1).is_some_and(|p| is_name_word(p));
            if !starts || next_cap || prev_cap {
                let w = fold(t, &FoldOptions::KEY);
                for part in w
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|p| p.len() > 1)
                {
                    if !COMMON.contains(&part) {
                        out.push(Claim::Name(part.to_owned()));
                    }
                }
            }
        }
    }
    out
}

/// Trims quotes, brackets and trailing sentence punctuation.
fn strip(t: &str) -> &str {
    let t = t.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\''
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | ':'
                | '“'
                | '”'
                | '„'
                | '‘'
                | '’'
                | '«'
                | '»'
                | '!'
                | '?'
                | '…'
        )
    });
    // A final period ends the sentence unless the token is an abbreviation-like code.
    match t.strip_suffix('.') {
        Some(s) if !s.contains('.') || s.ends_with(|c: char| c.is_ascii_digit()) => s,
        _ => t,
    }
}

fn month(w: &str) -> Option<u32> {
    MONTHS.iter().find(|(n, _)| *n == w).map(|(_, m)| *m)
}

fn day(w: &str) -> Option<u32> {
    let w = w.trim_end_matches('.');
    let w = ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|s| w.strip_suffix(s))
        .unwrap_or(w);
    w.parse::<u32>().ok().filter(|d| (1..=31).contains(d))
}

fn year(w: &str) -> Option<i32> {
    let w = w.trim_end_matches(['.', ',']);
    (w.len() == 4)
        .then(|| w.parse::<i32>().ok())
        .flatten()
        .filter(|y| (1900..=2100).contains(y))
}

fn valid(y: i32, m: u32, d: u32) -> bool {
    (1900..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// Two-digit years from this value up are 19xx, below it 20xx (the POSIX `%y` pivot).
const CENTURY_PIVOT: u32 = 69;

/// `2023-09-12`, `12.09.2023`, `12/09/2023`, `12-09-2023`; with `short_year` also
/// `09/12/23` and `12.09.23` (see [`CENTURY_PIVOT`]). Day and month take 1–2 digits.
#[allow(clippy::many_single_char_names)] // y, m, d
fn numeric_date(t: &str, short_year: bool) -> Option<Claim> {
    let parts: Vec<&str> = t.split(['-', '.', '/']).collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let n: Vec<u32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    if n.len() != 3 {
        return None;
    }
    if parts[0].len() == 4 {
        let y = i32::try_from(n[0]).ok()?;
        return valid(y, n[1], n[2]).then_some(Claim::Date(y, n[1], n[2]));
    }
    if parts[0].len() > 2 || parts[1].len() > 2 {
        return None;
    }
    let y = match parts[2].len() {
        4 => i32::try_from(n[2]).ok()?,
        // One separator twice, `/` or `.` (`12-34-56` is more often a part number).
        2 if short_year && (t.matches('/').count() == 2 || t.matches('.').count() == 2) => {
            let c = if n[2] >= CENTURY_PIVOT { 1900 } else { 2000 };
            i32::try_from(c + n[2]).ok()?
        }
        _ => return None,
    };
    let dm = valid(y, n[1], n[0]).then_some((y, n[1], n[0]));
    let md = valid(y, n[0], n[1]).then_some((y, n[0], n[1]));
    match (dm, md) {
        (Some(a), Some(b)) if a != b => Some(Claim::AmbiguousDate(a, b)),
        (Some(a), _) | (None, Some(a)) => Some(Claim::Date(a.0, a.1, a.2)),
        (None, None) => None,
    }
}

/// A token with digits: an identifier, or one or more numbers.
fn numberish(t: &str) -> Vec<Claim> {
    let t = t.trim_end_matches('%');
    let t = t
        .trim_start_matches(['€', '$', '£', '#', '+', '~', '≈'])
        .trim_start_matches("No.");
    let segs: Vec<&str> = t.split(['-', '/', '_']).filter(|s| !s.is_empty()).collect();
    let lettered = |s: &str| s.chars().any(char::is_alphabetic);
    let plain_word = |s: &str| s.chars().all(char::is_lowercase);
    let ordinal = |s: &str| day(s).is_some() && s.chars().any(char::is_alphabetic);
    if segs
        .iter()
        .all(|s| !lettered(s) || plain_word(s) || ordinal(s))
    {
        // Numbers (with words such as "24-month"); a bare slash run is an id ("114/23").
        if t.contains('/') && segs.len() > 1 {
            return vec![Claim::Id(id_key(t))];
        }
        return segs
            .iter()
            .filter(|s| !lettered(s) || ordinal(s))
            .filter_map(|s| number_key(s.trim_end_matches(char::is_alphabetic)))
            .map(Claim::Number)
            .collect();
    }
    let key = id_key(t);
    if key.is_empty() {
        Vec::new()
    } else {
        vec![Claim::Id(key)]
    }
}

/// Upper-case alphanumeric key of an identifier.
fn id_key(t: &str) -> String {
    t.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn trim_zeros(s: &str) -> &str {
    let t = s.trim_start_matches('0');
    if t.is_empty() { "0" } else { t }
}

/// Canonical number: integer digits without leading zeros, then `.` and the fraction without
/// trailing zeros when it has any. `4,200.00` → `4200`, `4.200,50` → `4200.5`.
fn number_key(s: &str) -> Option<String> {
    let s = s.trim_matches(|c: char| !c.is_ascii_digit());
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '\'')
    {
        return None;
    }
    let last = s.rfind(['.', ',']);
    let (int, frac) = match last {
        Some(i) => {
            let tail = &s[i + 1..];
            let sep = &s[i..=i];
            let other_sep_before = s[..i].contains(if sep == "." { ',' } else { '.' });
            // A last separator followed by exactly three digits groups thousands, unless the
            // other separator came first ("1,234.567" stays decimal).
            if tail.len() == 3 && !other_sep_before && !s.starts_with('0') {
                (s.to_owned(), String::new())
            } else {
                (s[..i].to_owned(), tail.to_owned())
            }
        }
        None => (s.to_owned(), String::new()),
    };
    let int: String = int.chars().filter(char::is_ascii_digit).collect();
    let frac = frac.trim_end_matches('0');
    let int = trim_zeros(&int);
    Some(if frac.is_empty() {
        int.to_owned()
    } else {
        format!("{int}.{frac}")
    })
}

fn is_name_word(t: &str) -> bool {
    let mut cs = t.chars();
    cs.next().is_some_and(char::is_uppercase)
        && t.chars().filter(|c| c.is_alphabetic()).count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_normalize() {
        assert_eq!(number_key("4,200.00").as_deref(), Some("4200"));
        assert_eq!(number_key("4.200,50").as_deref(), Some("4200.5"));
        assert_eq!(number_key("1.200").as_deref(), Some("1200"));
        assert_eq!(number_key("0.25").as_deref(), Some("0.25"));
        assert_eq!(number_key("007").as_deref(), Some("7"));
    }

    #[test]
    fn dates_match_across_formats() {
        let src = [
            "signed=2023-09-12",
            "Zaključen dana 1. oktobra 2023. godine",
        ];
        assert!(check("Signed on 12 September 2023.", &src, true).passed());
        assert!(check("Signed on September 12, 2023.", &src, true).passed());
        assert!(check("Signed 12.09.2023 and in October 2023.", &src, true).passed());
        let fc = check("Signed on 13 September 2023.", &src, true);
        assert_eq!(fc.unsupported, vec!["date:2023-09-13"]);
    }

    #[test]
    fn amounts_ids_and_names() {
        let src = [
            "parties: supplier=BLUE RIVER SOFTWARE LTD; customer=Northwind Logistika d.o.o.\n\
             amounts: total=EUR 184,500.00\nrefs: amends=msa SA-2026/0417",
            "Ugovor br. SA-2026/0417, podrška 24 meseca.",
        ];
        let ok = "Amendment to agreement SA-2026/0417 between Blue River Software Ltd and \
                  Northwind Logistika for EUR 184,500 with 24-month support.";
        let fc = check(ok, &src, true);
        assert!(fc.passed(), "{fc:?}");
        let bad = "The Acme Corp agreement SA-2026/0418 is worth EUR 185,000.";
        let fc = check(bad, &src, true);
        assert_eq!(
            fc.unsupported,
            vec!["id:SA20260418", "name:acme", "number:185000"]
        );
        // A translated title is not checked for names.
        assert!(check("Warehouse Lease Agreement", &src, false).passed());
    }

    #[test]
    fn cyrillic_names_fold() {
        let src = ["Закупац: Дунав Некретнине а.д."];
        assert!(check("The lessee is Dunav Nekretnine a.d.", &src, true).passed());
    }

    #[test]
    fn two_digit_years_in_the_sources() {
        let src = ["Invoice Date 04/24/15\nInvoice Period 03/27/15 - 04/19/15\nFlight 04/06/15"];
        let ok = "Broadcasts from March 27, 2015 to April 19, 2015, invoiced on 24 April 2015.";
        let fc = check(ok, &src, true);
        assert!(fc.passed(), "{fc:?}");
        // `04/06/15` reads both ways.
        assert!(check("Aired on 6 April 2015.", &src, true).passed());
        assert!(check("Aired on 4 June 2015.", &src, true).passed());
        let fc = check("Broadcasts from March 28, 2015.", &src, true);
        assert_eq!(fc.unsupported, vec!["date:2015-03-28"]);
        // Day first with periods; the pivot puts 69-99 in the 1900s.
        let src = ["Zaključeno 27.03.15. i 31/12/99"];
        assert!(
            check(
                "Signed on 27 March 2015 after 31 December 1999.",
                &src,
                true
            )
            .passed()
        );
        assert!(!check("Signed on 27 March 1915.", &src, true).passed());
        // A `-` run with a two-digit tail stays a part number.
        assert!(!check("Dated 12 May 2010.", &["Part 05-12-10"], true).passed());
    }

    #[test]
    fn translated_names_pass_and_invented_ones_do_not() {
        let src = [
            "НАРУЧИЛАЦ: Републички фонд за здравствено осигурање, Београд\n\
             ДОБАВЉАЧ: Pharmalolgix d.o.o., Београд",
        ];
        let ok = "An amendment between the Republic Health Insurance Fund in Belgrade and \
                  Pharmalogix d.o.o.";
        let fc = check(ok, &src, true);
        assert!(fc.passed(), "{fc:?}");
        assert!(check("The Health Insurance Funds.", &src, true).passed());
        // An invented supplier, an untranslated word and a name two letters off fail.
        let fc = check(
            "Agreement of the Republic Health Fund with Medilink d.o.o.",
            &src,
            true,
        );
        assert_eq!(fc.unsupported, vec!["name:medilink"]);
        let fc = check("Signed by the Republic Pension Fund.", &src, true);
        assert_eq!(fc.unsupported, vec!["name:pension"]);
        let fc = check("Supplied by Pharmalogixika d.o.o.", &src, true);
        assert_eq!(fc.unsupported, vec!["name:pharmalogixika"]);
        // One letter off is accepted only from 8 letters.
        let fc = check("Signed by Acne Corp.", &["Acme Corp"], true);
        assert_eq!(fc.unsupported, vec!["name:acne"]);
    }

    #[test]
    fn one_edit_apart() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(one_edit(&c("pharmalogix"), &c("pharmalolgix")));
        assert!(one_edit(&c("northwind"), &c("nortwhind")));
        assert!(one_edit(&c("northwind"), &c("northwand")));
        assert!(one_edit(&c("northwind"), &c("northwind")));
        assert!(!one_edit(&c("northwind"), &c("northwinds2")));
        assert!(!one_edit(&c("northwind"), &c("southwind")));
    }

    #[test]
    fn document_text_is_a_source() {
        use quail_core::{Page, PageKind};
        let page = |i: u32, t: &str| Page {
            text: t.to_owned(),
            ..Page::new(i, PageKind::Physical)
        };
        let doc = Document {
            pages: vec![
                page(0, "Cover page"),
                page(1, "Signed on 3 March 2021 by Ivanka."),
            ],
        };
        let all = document_sources(&doc, DOCUMENT_SOURCE_BYTES);
        assert_eq!(all.len(), 2);
        let out = "Signed by Ivanka on 3 March 2021.";
        assert!(!check(out, &["doc: contract"], true).passed());
        let mut src = vec!["doc: contract"];
        src.extend(all);
        assert!(check(out, &src, true).passed());
        // The byte cap keeps the first pages only.
        assert_eq!(document_sources(&doc, 12).len(), 1);
    }
}

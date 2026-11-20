//! Is a string plausibly a legal entity name? The reference system's deterministic gates
//! (`name_quality.py`), reproduced rule by rule and extended with Serbian vocabulary.
//!
//! - [`classify`]: `Reject` (structurally impossible: length, line break, e-mail/URL,
//!   addressee label, postal code, digits plus an address word, more than
//!   [`MAX_ENTITY_WORDS`] words, personal title, "Name Surname, Role", no letters, a bracket
//!   holding a name-change sentence), `Suspect` (two bare Titlecase words: a person's shape),
//!   `Ok`.
//! - [`is_role_term`]: only a contract role ("the Supplier", "Naručilac").
//! - [`is_name_fragment`]: prose cut out of a sentence (opens or closes on a connector, a
//!   label, a legal form; states a relation; a single generic word; mojibake).
//! - [`names_a_product`]: ends on the goods ("... Licence", "... Support").
//! - [`verdict`]: all of them, with the first reason that refuses the name.

use quail_text::fold::{FoldOptions, fold};

use crate::vocab::{self, has};

/// Longest name in words.
pub const MAX_ENTITY_WORDS: usize = 10;
/// Longest name in characters.
pub const MAX_ENTITY_CHARS: usize = 120;
/// Shortest name in characters.
pub const MIN_ENTITY_CHARS: usize = 2;

/// Shape verdict of [`classify`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Nothing against it.
    Ok,
    /// Two bare Titlecase words: could be a person ("Petar Petrović") or a brand.
    Suspect,
    /// Structurally impossible as a company name.
    Reject,
}

impl Quality {
    /// Lowercase name, as emitted in `party.company_quality`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Suspect => "suspect",
            Self::Reject => "reject",
        }
    }
}

/// Why [`verdict`] refuses a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// [`classify`] said `Reject`.
    Shape,
    /// Only a contract role.
    Role,
    /// A fragment of prose.
    Fragment,
    /// Names the goods.
    Product,
    /// No distinctive word: only legal forms, places, roles, connectors and state words
    /// ("Republika Srbija", "Group Ltd").
    Generic,
}

/// [`classify`] plus the writer-side refusals (`is_unusable_supplier_name`).
pub fn verdict(name: &str) -> Result<Quality, Refusal> {
    let q = classify(name);
    if q == Quality::Reject {
        return Err(Refusal::Shape);
    }
    if is_role_term(name) {
        return Err(Refusal::Role);
    }
    if is_name_fragment(name) {
        return Err(Refusal::Fragment);
    }
    if names_a_product(name) {
        return Err(Refusal::Product);
    }
    if !has_distinctive_word(name) {
        return Err(Refusal::Generic);
    }
    Ok(q)
}

/// Words of a state or government rather than of a company.
const STATE_WORDS: &[&str] = &[
    "republika",
    "republike",
    "republici",
    "republic",
    "state",
    "kingdom",
    "government",
    "vlada",
    "vlade",
    "ministarstvo",
    "ministry",
    "grad",
    "opstina",
    "municipality",
    // US states and Canadian provinces: "a Florida Corporation" states where, not who.
    "alabama",
    "alaska",
    "arizona",
    "arkansas",
    "california",
    "colorado",
    "connecticut",
    "delaware",
    "florida",
    "georgia",
    "hawaii",
    "idaho",
    "illinois",
    "indiana",
    "iowa",
    "kansas",
    "kentucky",
    "louisiana",
    "maine",
    "maryland",
    "massachusetts",
    "michigan",
    "minnesota",
    "mississippi",
    "missouri",
    "montana",
    "nebraska",
    "nevada",
    "hampshire",
    "jersey",
    "mexico",
    "york",
    "carolina",
    "dakota",
    "ohio",
    "oklahoma",
    "oregon",
    "pennsylvania",
    "rhode",
    "island",
    "tennessee",
    "texas",
    "utah",
    "vermont",
    "virginia",
    "washington",
    "west",
    "wisconsin",
    "wyoming",
    "north",
    "south",
    "new",
    "british",
    "columbia",
    "ontario",
    "quebec",
    "alberta",
    "manitoba",
    "nova",
    "scotia",
    "cayman",
    "islands",
    "bermuda",
    "virgin",
    "wales",
    "england",
    "scotland",
];

/// Whether some word of `name` could identify a company: not a legal form, place, role,
/// connector, state word, trade noun or calendar word.
pub fn has_distinctive_word(name: &str) -> bool {
    word_keys(name).iter().any(|t| {
        !(vocab::is_legal_form_word(t)
            || vocab::is_geo(t)
            || is_role_key(t)
            || has(vocab::CONNECTOR_WORDS, t)
            || has(STATE_WORDS, t)
            || has(vocab::TRADE_NOUNS, t)
            || has(vocab::CALENDAR_WORDS, t)
            || has(vocab::NOISE_EXTRA, t))
    })
}

/// Folded words of `name`, split on everything that is not a letter or digit.
pub fn word_keys(name: &str) -> Vec<String> {
    fold(name, &FoldOptions::KEY)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

fn alnum_key(token: &str) -> String {
    fold(token, &FoldOptions::KEY)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

const ADDRESSEE_LABELS: &[&str] = &[
    "attn",
    "c/o",
    "from:",
    "cc:",
    "bcc:",
    "re:",
    "bill to",
    "ship to",
    "invoice to",
    "sold to",
    "remit to",
];

fn is_addressee_header(stripped: &str) -> bool {
    let lower = stripped.to_lowercase();
    if ADDRESSEE_LABELS.iter().any(|l| lower.starts_with(l)) {
        return true;
    }
    for word in ["to", "attention"] {
        if let Some(rest) = lower.strip_prefix(word) {
            let rest = rest.trim_start_matches([' ', '\t']);
            if rest.starts_with([':', '.', '-', '\n']) {
                return true;
            }
        }
    }
    false
}

fn is_outward(t: &str) -> bool {
    let c: Vec<char> = t.chars().collect();
    let letters = c.iter().take_while(|x| x.is_ascii_alphabetic()).count();
    if !(1..=2).contains(&letters) || c.len() < letters + 1 || c.len() > letters + 2 {
        return false;
    }
    c[letters].is_ascii_digit() && c[letters + 1..].iter().all(char::is_ascii_alphanumeric)
}

fn is_inward(t: &str) -> bool {
    let c: Vec<char> = t.chars().collect();
    c.len() == 3
        && c[0].is_ascii_digit()
        && c[1].is_ascii_alphabetic()
        && c[2].is_ascii_alphabetic()
}

fn has_postal_code(stripped: &str) -> bool {
    // US ZIP (and every other five-digit postal code: Serbian ones are five digits too).
    let b = stripped.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let before_ok = s == 0 || !(b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_');
            let after_ok = i == b.len() || !(b[i].is_ascii_alphanumeric() || b[i] == b'_');
            if i - s == 5 && before_ok && after_ok {
                return true;
            }
        } else {
            i += 1;
        }
    }
    // UK postcode closing the name or a line, never opening it.
    let toks: Vec<&str> = stripped.split_whitespace().collect();
    for (k, t) in toks.iter().enumerate() {
        let closes = |x: &str| x.ends_with(',') || k + 1 == toks.len() || x.ends_with('\n');
        let bare = t.trim_end_matches(',');
        if !bare.is_ascii() {
            continue;
        }
        if k > 0 && bare.len() >= 5 && closes(t) {
            let (o, inw) = bare.split_at(bare.len() - 3);
            if is_outward(o) && is_inward(inw) {
                return true;
            }
        }
        if k > 0 && k + 1 < toks.len() && is_outward(bare) {
            let next = toks[k + 1];
            let nb = next.trim_end_matches(',');
            if is_inward(nb) && (next.ends_with(',') || k + 2 == toks.len()) {
                return true;
            }
        }
    }
    false
}

fn has_prose_paren(stripped: &str) -> bool {
    let lower = stripped.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(p) = rest.find('(') {
        let inner = &rest[p + 1..];
        let inner = inner.split(')').next().unwrap_or("");
        let words: Vec<&str> = inner.split(|c: char| !c.is_alphanumeric()).collect();
        if words.iter().any(|w| matches!(*w, "changed" | "renamed"))
            || inner.contains("name change")
        {
            return true;
        }
        rest = &rest[p + 1..];
    }
    false
}

fn is_rejected_text(name: &str, stripped: &str) -> bool {
    let len = stripped.chars().count();
    if !(MIN_ENTITY_CHARS..=MAX_ENTITY_CHARS).contains(&len) {
        return true;
    }
    if name.contains(['\n', '\r']) || name.contains('@') {
        return true;
    }
    let lower = name.to_lowercase();
    if lower.contains("http://") || lower.contains("https://") || lower.contains("www.") {
        return true;
    }
    is_addressee_header(stripped) || has_prose_paren(stripped) || has_postal_code(stripped)
}

fn has_trailing_role(stripped: &str) -> bool {
    // "Firstname Lastname, Role".
    let mut parts = stripped.splitn(2, ',');
    let (Some(head), Some(tail)) = (parts.next(), parts.next()) else {
        return false;
    };
    let head_tokens: Vec<&str> = head.split_whitespace().collect();
    if head_tokens.len() != 2 {
        return false;
    }
    tail.split_whitespace()
        .next()
        .is_some_and(|r| has(vocab::ROLE_WORDS, &alnum_key(r)))
}

fn is_rejected_structure(stripped: &str, tokens: &[&str]) -> bool {
    let keys: Vec<String> = tokens.iter().map(|t| alnum_key(t)).collect();
    let has_digit = stripped.chars().any(|c| c.is_ascii_digit());
    if has_digit && keys.iter().any(|k| has(vocab::ADDRESS_TOKENS, k)) {
        return true;
    }
    if tokens.len() > MAX_ENTITY_WORDS {
        return true;
    }
    if keys.first().is_some_and(|k| has(vocab::PERSONAL_TITLES, k)) {
        return true;
    }
    if has_trailing_role(stripped) {
        return true;
    }
    !stripped.chars().any(char::is_alphabetic)
}

fn is_bare_person_shape(stripped: &str, tokens: &[&str]) -> bool {
    if tokens.len() != 2 || stripped.contains('&') {
        return false;
    }
    tokens.iter().all(|t| {
        let key = alnum_key(t);
        if key.is_empty()
            || !key.chars().all(char::is_alphabetic)
            || vocab::is_legal_form_word(&key)
        {
            return false;
        }
        let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
        letters.first().is_some_and(|c| c.is_uppercase())
            && letters[1..].iter().all(|c| c.is_lowercase())
    })
}

/// Shape verdict for a candidate name (`classify_supplier_name`).
///
/// ```
/// use quail_parties::quality::{classify, Quality};
/// assert_eq!(classify("ACME d.o.o."), Quality::Ok);
/// assert_eq!(classify("Petar Petrović"), Quality::Suspect);
/// assert_eq!(classify("Bulevar Mihajla Pupina 10, 11000 Beograd"), Quality::Reject);
/// assert_eq!(classify("Attn: John Smith"), Quality::Reject);
/// ```
pub fn classify(name: &str) -> Quality {
    let stripped = name.trim();
    if stripped.is_empty() {
        return Quality::Reject;
    }
    let tokens: Vec<&str> = stripped.split_whitespace().collect();
    if is_rejected_text(name, stripped) || is_rejected_structure(stripped, &tokens) {
        return Quality::Reject;
    }
    if is_bare_person_shape(stripped, &tokens) {
        return Quality::Suspect;
    }
    Quality::Ok
}

/// The bare two-word shape of a person's name in any letter case (`PETAR PETROVIĆ` too).
pub fn is_person_shaped(name: &str) -> bool {
    let titled: Vec<String> = name
        .split_whitespace()
        .map(|t| {
            let mut c = t.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect();
    classify(&titled.join(" ")) == Quality::Suspect
}

/// Whether `name` carries a legal-form word.
pub fn has_legal_form(name: &str) -> bool {
    name.split_whitespace()
        .any(|t| vocab::is_legal_form_word(&alnum_key(t)))
}

/// Only a contract role ("the Supplier", "Naručilac", "Second Party").
///
/// ```
/// use quail_parties::quality::is_role_term;
/// assert!(is_role_term("the Supplier"));
/// assert!(is_role_term("Наручилац"));
/// assert!(!is_role_term("Nordic Group"));
/// ```
pub fn is_role_term(name: &str) -> bool {
    let tokens = word_keys(name);
    let names_a_role = tokens
        .iter()
        .any(|t| is_role_key(t) && !has(vocab::ROLE_FILLER_WORDS, t));
    names_a_role
        && tokens.iter().all(|t| {
            is_role_key(t)
                || vocab::is_legal_form_word(t)
                || vocab::is_geo(t)
                || has(vocab::CONNECTOR_WORDS, t)
        })
}

/// A role word, Serbian declensions included (`naručioca`, `dobavljača`, `kupcu`).
pub fn is_role_key(t: &str) -> bool {
    vocab::is_role_term_word(t) || crate::roles::role_of_word(t).is_some()
}

fn is_generic_word(t: &str) -> bool {
    is_role_key(t)
        || vocab::is_legal_form_word(t)
        || vocab::is_geo(t)
        || has(vocab::CONNECTOR_WORDS, t)
}

fn is_entity_form_word(t: &str) -> bool {
    (vocab::is_legal_form_word(t) && t != "as" && t != "co") || has(vocab::ENTITY_FORM_EXTRA, t)
}

fn names_an_entity(stripped: &str, tokens: &[String]) -> bool {
    if tokens.len() <= 2 {
        return false;
    }
    let lower = stripped.to_lowercase();
    if lower.contains("a/s") || lower.contains("a / s") {
        return true;
    }
    tokens[1..].iter().any(|t| is_entity_form_word(t))
}

fn opens_as_fragment(stripped: &str, tokens: &[String]) -> bool {
    let head = tokens[0].as_str();
    if vocab::is_legal_form_word(head) && (tokens.len() <= 2 || stripped.contains('(')) {
        // Prefix forms open real names ("JP Elektroprivreda", "OOO Romashka").
        if !crate::forms::is_prefix_form(head) {
            return true;
        }
    }
    if has(vocab::CONNECTOR_WORDS, head) {
        return true;
    }
    let leads = has(vocab::FRAGMENT_LEAD_WORDS, head)
        || (is_role_key(head) && !has(vocab::ROLE_FILLER_WORDS, head));
    if !(leads || head.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    !names_an_entity(stripped, tokens)
        || (leads
            && tokens
                .get(1)
                .is_some_and(|t| has(vocab::LABEL_FOLLOWER_WORDS, t)))
}

fn closes_as_fragment(tokens: &[String]) -> bool {
    let Some(tail) = tokens.last().map(String::as_str) else {
        return false;
    };
    if has(vocab::PARTY_ROLE_WORDS, tail) {
        return true;
    }
    if has(vocab::AMOUNT_TAIL_WORDS, tail)
        && tokens.iter().any(|t| t.chars().all(|c| c.is_ascii_digit()))
    {
        return true;
    }
    let run = tokens
        .iter()
        .rev()
        .take_while(|t| t.chars().count() == 1)
        .count();
    if run >= 2 && tokens.len() > run {
        return false;
    }
    has(vocab::CONNECTOR_WORDS, tail)
        || has(vocab::ROLE_FILLER_WORDS, tail)
        || has(vocab::VERB_TAIL_WORDS, tail)
}

/// Prose the capture cut out rather than a name (`is_name_fragment`).
pub fn is_name_fragment(name: &str) -> bool {
    let stripped = name.trim();
    if stripped.is_empty() || stripped.contains('\u{FFFD}') {
        return true;
    }
    let lower = stripped.to_lowercase();
    if ["'s", "\u{2019}s", "\u{02bc}s"]
        .iter()
        .any(|t| lower.ends_with(t))
    {
        return true;
    }
    if vocab::RELATION_PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    let tokens = word_keys(stripped);
    if tokens.is_empty() {
        return true;
    }
    if tokens.len() == 1 && is_generic_word(&tokens[0]) {
        return true;
    }
    opens_as_fragment(stripped, &tokens) || closes_as_fragment(&tokens)
}

/// Ends on the goods rather than on a company (`names_a_product`).
pub fn names_a_product(name: &str) -> bool {
    word_keys(name)
        .last()
        .is_some_and(|t| has(vocab::PRODUCT_TAIL_WORDS, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_cases() {
        assert_eq!(classify("3M Company"), Quality::Ok);
        assert_eq!(classify("B2B 2Go Ltd"), Quality::Ok);
        assert_eq!(classify("Attention Media Ltd"), Quality::Ok);
        assert_eq!(classify("To: Acme"), Quality::Reject);
        assert_eq!(classify("Mara Example, Director"), Quality::Reject);
        assert_eq!(classify("Acme Ltd, London SW1A 1AA"), Quality::Reject);
        assert_eq!(classify("Mr Smith Holdings"), Quality::Reject);
        assert_eq!(
            classify("Qorvix (changed company name from X 2017)"),
            Quality::Reject
        );
        assert!(is_name_fragment("Kestrelon's"));
        assert!(is_name_fragment("X Ltd, a subsidiary of Y plc"));
        assert!(is_name_fragment("Vendor Ltd"));
        assert!(!is_name_fragment("First Data Corporation"));
        assert!(!is_name_fragment("Altavero S.A."));
        assert!(is_name_fragment("Ltd (trading as Pelmora)"));
        assert!(names_a_product("Trackwell Track & Trace Licence"));
        assert!(!names_a_product("Quenton Support Services Ltd"));
        assert_eq!(verdict("Republika Srbija"), Err(Refusal::Generic));
        assert_eq!(verdict("Group Ltd"), Err(Refusal::Fragment));
        assert_eq!(verdict("ACME d.o.o."), Ok(Quality::Ok));
    }
}

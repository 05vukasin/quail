//! Legal forms: how each is written, where it stands in a name, and which jurisdictions use it.
//!
//! A form is looked up by its *bare* key: the folded spelling without dots or slashes, so
//! `d.o.o.`, `D.O.O.`, `doo`, `DOO`, `д.о.о.` and `d. o. o.` are all `doo`. Spellings that
//! are also ordinary words (`AS`, `AD`, `SA`, `Co`, `Company`, `Group`) are *weak*: they must be
//! written the way a form is (uppercase, dotted or Titlecase) and give lower confidence.

/// Where a form stands relative to the name it qualifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    /// After the name: `ACME d.o.o.`, `Acme Ltd`.
    Suffix,
    /// Before the name: `JP Elektroprivreda`, `OOO Romashka`.
    Prefix,
    /// Inside a sole trader's name: `Petar Petrović PR Agencija X`.
    Infix,
}

/// How the surface of a form must be written to count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Any casing.
    Any,
    /// Not all lowercase.
    NotLower,
    /// All letters uppercase.
    Upper,
    /// Dotted (`a.d.`) or all uppercase (`AD`).
    DottedOrUpper,
    /// Dotted only (`S.L.`).
    Dotted,
}

/// One legal form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Form {
    /// Bare key (folded, no dots).
    pub bare: &'static str,
    /// Canonical spelling, as emitted in `party.legal_form`.
    pub canon: &'static str,
    /// Where it stands.
    pub position: Position,
    /// Casing rule.
    pub gate: Gate,
    /// Also an ordinary word: needs the gate and gives lower confidence.
    pub weak: bool,
    /// ISO 3166-1 alpha-2 codes of the jurisdictions that use it (lowercase); empty when too
    /// many do to say anything (`Ltd`, `S.A.`).
    pub jurisdictions: &'static [&'static str],
}

const fn f(
    bare: &'static str,
    canon: &'static str,
    position: Position,
    gate: Gate,
    weak: bool,
    jurisdictions: &'static [&'static str],
) -> Form {
    Form {
        bare,
        canon,
        position,
        gate,
        weak,
        jurisdictions,
    }
}

use std::collections::HashMap;
use std::sync::LazyLock;

use Gate::{Any, Dotted, DottedOrUpper, NotLower, Upper};
use Position::{Infix, Prefix, Suffix};

const EXYU: &[&str] = &["rs", "hr", "ba", "me", "si"];

/// Every form quail recognises.
pub const FORMS: &[Form] = &[
    // Serbia and the region.
    f("doo", "d.o.o.", Suffix, Any, false, EXYU),
    f("dooel", "DOOEL", Suffix, Any, false, &["mk"]),
    f("jdoo", "j.d.o.o.", Suffix, Any, false, &["hr"]),
    f(
        "dd",
        "d.d.",
        Suffix,
        DottedOrUpper,
        true,
        &["hr", "ba", "si"],
    ),
    f(
        "ad",
        "a.d.",
        Suffix,
        DottedOrUpper,
        true,
        &["rs", "ba", "me", "mk"],
    ),
    f("kd", "k.d.", Suffix, Dotted, true, &["rs", "hr", "si"]),
    f("od", "o.d.", Suffix, Dotted, true, &["rs"]),
    f("pr", "PR", Infix, Upper, true, &["rs"]),
    f("preduzetnik", "preduzetnik", Infix, Any, true, &["rs"]),
    f("szr", "SZR", Infix, Upper, true, &["rs"]),
    f("str", "STR", Infix, Upper, true, &["rs"]),
    f("jp", "JP", Prefix, Upper, true, &["rs", "ba", "me"]),
    f("jkp", "JKP", Prefix, Upper, false, &["rs"]),
    f("ooo", "OOO", Prefix, Upper, true, &["ru", "by"]),
    f("zao", "ZAO", Prefix, Upper, true, &["ru"]),
    f("oao", "OAO", Prefix, Upper, true, &["ru"]),
    f("pao", "PAO", Prefix, Upper, true, &["ru"]),
    // German-speaking.
    f("gmbh", "GmbH", Suffix, Any, false, &["de", "at", "ch"]),
    f("ag", "AG", Suffix, Upper, true, &["de", "ch", "at"]),
    f("kg", "KG", Suffix, Upper, true, &["de", "at"]),
    f("ohg", "OHG", Suffix, Upper, true, &["de"]),
    // English-speaking.
    f("llc", "LLC", Suffix, NotLower, false, &["us"]),
    f("llp", "LLP", Suffix, NotLower, false, &["us", "gb"]),
    f("lp", "L.P.", Suffix, DottedOrUpper, true, &["us"]),
    f("inc", "Inc.", Suffix, NotLower, false, &["us", "ca"]),
    f(
        "incorporated",
        "Incorporated",
        Suffix,
        NotLower,
        false,
        &["us"],
    ),
    f("corp", "Corp.", Suffix, NotLower, false, &["us"]),
    f("corporation", "Corporation", Suffix, NotLower, false, &[]),
    f("ltd", "Ltd", Suffix, NotLower, false, &[]),
    f("limited", "Limited", Suffix, NotLower, false, &[]),
    f("plc", "plc", Suffix, Any, false, &["gb", "ie"]),
    f("pty", "Pty", Suffix, NotLower, false, &["au", "za"]),
    f("pvt", "Pvt", Suffix, NotLower, false, &["in"]),
    f("co", "Co.", Suffix, NotLower, true, &[]),
    f("company", "Company", Suffix, NotLower, true, &[]),
    f("group", "Group", Suffix, NotLower, true, &[]),
    f("holdings", "Holdings", Suffix, NotLower, true, &[]),
    f("holding", "Holding", Suffix, NotLower, true, &[]),
    // Romance languages.
    f("sa", "S.A.", Suffix, DottedOrUpper, true, &[]),
    f("sas", "SAS", Suffix, Upper, true, &["fr"]),
    f("sarl", "SARL", Suffix, NotLower, false, &["fr", "lu", "ch"]),
    f("srl", "S.r.l.", Suffix, NotLower, false, &["it", "ro"]),
    f("spa", "S.p.A.", Suffix, DottedOrUpper, true, &["it"]),
    f("sl", "S.L.", Suffix, Dotted, true, &["es"]),
    f("ltda", "Ltda.", Suffix, NotLower, false, &["br", "pt"]),
    // Benelux.
    f("bv", "B.V.", Suffix, DottedOrUpper, true, &["nl", "be"]),
    f("nv", "N.V.", Suffix, DottedOrUpper, true, &["nl", "be"]),
    f("bvba", "BVBA", Suffix, NotLower, false, &["be"]),
    f("sprl", "SPRL", Suffix, NotLower, false, &["be"]),
    // Nordic.
    f("ab", "AB", Suffix, Upper, true, &["se"]),
    f("as", "AS", Suffix, Upper, true, &["no", "dk"]),
    f("asa", "ASA", Suffix, Upper, true, &["no"]),
    f("aps", "ApS", Suffix, NotLower, false, &["dk"]),
    f("oy", "Oy", Suffix, NotLower, true, &["fi"]),
    f("oyj", "Oyj", Suffix, NotLower, false, &["fi"]),
    // Central Europe.
    f("kft", "Kft.", Suffix, NotLower, false, &["hu"]),
    f("zrt", "Zrt.", Suffix, NotLower, false, &["hu"]),
    f("nyrt", "Nyrt.", Suffix, NotLower, false, &["hu"]),
    f("sro", "s.r.o.", Suffix, Any, false, &["cz", "sk"]),
    f("spzoo", "Sp. z o.o.", Suffix, Any, false, &["pl"]),
];

/// Bare key → form, built once (read-only).
static BY_BARE: LazyLock<HashMap<&'static str, &'static Form>> =
    LazyLock::new(|| FORMS.iter().map(|f| (f.bare, f)).collect());

/// Longest bare key.
const MAX_BARE: usize = 12;

/// The form with this bare key.
pub fn by_bare(bare: &str) -> Option<&'static Form> {
    if bare.is_empty() || bare.len() > MAX_BARE {
        return None;
    }
    BY_BARE.get(bare).copied()
}

/// Whether `bare` is a prefix form (`JP`, `OOO`, ...).
pub fn is_prefix_form(bare: &str) -> bool {
    by_bare(bare).is_some_and(|f| f.position == Position::Prefix)
}

/// Countries that write company names in Cyrillic (lowercase ISO codes).
const CYRILLIC_COUNTRIES: &[&str] = &["rs", "ba", "me", "mk", "ru", "by", "ua", "bg", "kz"];

/// Whether `form` may be written in Cyrillic: one of a country that writes Cyrillic
/// (`д.о.о.`, `ООО`). `СА`, `АГ` or `ИНЦ` are Serbian words or transliterations, not forms.
pub fn cyrillic_ok(form: &Form) -> bool {
    form.jurisdictions
        .iter()
        .any(|j| CYRILLIC_COUNTRIES.contains(j))
}

/// Whether `surface` (the form as written, dots kept) satisfies `gate`.
pub fn gate_ok(gate: Gate, surface: &str, dotted: bool) -> bool {
    let mut letters = surface.chars().filter(|c| c.is_alphabetic()).peekable();
    if letters.peek().is_none() {
        return false;
    }
    let all_upper = surface
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(char::is_uppercase);
    let all_lower = surface
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(char::is_lowercase);
    match gate {
        Any => true,
        NotLower => !all_lower,
        Upper => all_upper,
        DottedOrUpper => dotted || all_upper,
        Dotted => dotted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_keys_are_unique_and_folded() {
        for (i, a) in FORMS.iter().enumerate() {
            assert!(a.bare.len() <= MAX_BARE, "{}", a.bare);
            assert!(a.bare.bytes().all(|b| b.is_ascii_lowercase()), "{}", a.bare);
            assert!(
                FORMS[i + 1..].iter().all(|b| b.bare != a.bare),
                "{}",
                a.bare
            );
        }
    }

    #[test]
    fn gates() {
        assert!(gate_ok(Gate::DottedOrUpper, "a.d", true));
        assert!(gate_ok(Gate::DottedOrUpper, "AD", false));
        assert!(!gate_ok(Gate::DottedOrUpper, "ad", false));
        assert!(!gate_ok(Gate::NotLower, "limited", false));
        assert!(gate_ok(Gate::Any, "doo", false));
    }
}

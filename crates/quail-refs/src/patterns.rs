//! The patterns, compiled once per analyzer. Every pattern but the value patterns runs over a
//! page's key line (folded tokens, see `prep`), so one spelling covers upper/lower case,
//! diacritics and Serbian Cyrillic. All are `regex` crate automata: linear in the input.

use quail_core::Date;
use regex::Regex;

use crate::kind::Relation;

/// Kind phrases (key-line spelling). Longer phrases come first: the leftmost-first
/// alternation then prefers them at the same position.
const KIND: &str = r"(?-u:\b)(?:amended and restated(?: [a-z]+){0,3} (?:agreement|contract)|master(?: [a-z]+){0,2} agreement|msa|okvirn[a-z]* (?:ugovor|sporazum)[a-z]*|statement of work|sow|work order|radn[a-z]* nalog[a-z]*|change order|change request|side letter|non disclosure agreement|nondisclosure agreement|confidentiality agreement|nda|ugovor[a-z]{0,3} o (?:poverljivosti|cuvanju poverljiv[a-z]*)|amendment|addendum|avenant|aneks[a-z]{0,3}|izmen[a-z]{0,3} i dopun[a-z]{0,3}|dodatak|dodatka|dodatkom|annex|exhibit|schedule|appendix|attachment|prilog[a-z]{0,2}|purchase order|bon de commande|p\.o|po|narudzbenic[a-z]{0,2}|porudzbenic[a-z]{0,2}|narudzbin[a-z]{0,2}|order form|service order|order|invoice|facture|inv|faktur[a-z]{0,2}|profaktur[a-z]{0,2}|predracun[a-z]{0,2}|racun[a-z]{0,2}|quotation|quote|ponud[a-z]{0,2}|agreement|contract|contrat|ugovor[a-z]{0,3}|sporazum[a-z]{0,3})(?-u:\b)";

const MONTHS: &str = r"jan[a-z]*|feb[a-z]*|mar[a-z]*|apr[a-z]*|maj[a-z]?|may|jun[a-z]*|jul[a-z]*|avg[a-z]*|aug[a-z]*|sep[a-z]*|okt[a-z]*|oct[a-z]*|nov[a-z]*|dec[a-z]*";

/// Compiled patterns, shared read-only by every run.
#[derive(Debug)]
pub(crate) struct Patterns {
    pub kind: Regex,
    pub triggers: Vec<(Relation, Regex)>,
    pub date: Regex,
    pub value: Regex,
    pub spaced_value: Regex,
    pub bank_account: Regex,
    pub page_marker: Regex,
    pub bare_marker: Regex,
    pub version: Regex,
    pub generic_label: Regex,
    pub amendment_tag: Regex,
    pub change_order_tag: Regex,
}

fn re(p: &str) -> Regex {
    // The patterns are constants covered by tests; failing to compile one is a bug.
    Regex::new(p).unwrap_or_else(|e| panic!("quail-refs pattern: {e}"))
}

impl Patterns {
    pub fn new() -> Self {
        let art = r"(?: (?:the|that|said|such|an|a|our|your))?$";
        let trig = |body: &str| re(&format!("(?:^| )(?:{body}){art}"));
        Self {
            kind: re(KIND),
            triggers: vec![
                (
                    Relation::Amends,
                    trig(
                        r"(?:amendment|addendum|aneks[a-z]{0,3}|izmen[a-z]{0,3}|dopun[a-z]{0,3}|dodat[a-z]{0,3})(?: [^ ]+){0,3} (?:to|of|uz|na|za|for)|amending|amends|amend|amended|izmenom|menja|menjaju|izmen[a-z]{0,3} i dopun[a-z]{0,3}|aneks[a-z]{0,3}(?: br| broj| no| number)?(?: [0-9]{1,3}| [ivx]{1,5})?",
                    ),
                ),
                (
                    Relation::AnnexTo,
                    trig(
                        r"(?:annex|exhibit|schedule|appendix|attachment|prilog[a-z]{0,2})(?: [^ ]+){0,3} (?:to|of|uz|na|za|for|under)",
                    ),
                ),
                (
                    Relation::IssuedUnder,
                    trig(
                        r"pursuant to|issued under|under|in accordance with|governed by|subject to|in connection with|na osnovu|u skladu sa|u skladu s|po osnovu|shodno|temeljem|prema|sa pozivom na|u okviru",
                    ),
                ),
                (
                    Relation::BilledAgainst,
                    trig(
                        r"billed against|invoiced against|charged against|against|in respect of|po",
                    ),
                ),
                (
                    Relation::References,
                    trig(
                        r"referenced in|as defined in|defined in|referred to in|with reference to|reference to|see|vidi|u vezi sa|u vezi s",
                    ),
                ),
            ],
            date: re(&format!(
                r"^(?:dated|od|of|from|dana|date|effective|zakljucen[a-z]*|potpisan[a-z]*|signed|executed|entered into|sacinjen[a-z]*)(?: (?:as of|on|dana|od|na dan))? (?:(?P<d1>[0-9]{{1,2}})[./](?P<m1>[0-9]{{1,2}})[./](?P<y1>[0-9]{{4}})|(?P<y2>[0-9]{{4}})-(?P<m2>[0-9]{{1,2}})-(?P<d2>[0-9]{{1,2}})|(?P<d3>[0-9]{{1,2}})(?:st|nd|rd|th)?(?: day of)? (?P<n3>{MONTHS}) (?P<y3>[0-9]{{4}})|(?P<n4>{MONTHS}) (?P<d4>[0-9]{{1,2}})(?:st|nd|rd|th)? (?P<y4>[0-9]{{4}}))(?-u:\b)"
            )),
            value: re(r"^[\p{L}\p{N}]+(?:[-/._][\p{L}\p{N}]+)*"),
            spaced_value: re(r"^[A-Z]{1,5} [0-9][\p{L}\p{N}]*(?:[-/._][\p{L}\p{N}]+)*"),
            bank_account: re(r"^[0-9]{3}-[0-9]{5,13}-[0-9]{2}$"),
            page_marker: re(
                r"(?-u:\b)(?:page|pages|pg|strana|stranica|str|seite|pagina)(?: no| br)? (?P<n>[0-9]{1,4})(?:/(?P<m1>[0-9]{1,4})| (?:of|od|from|von|di|de|ukupno) (?P<m2>[0-9]{1,4}))(?-u:\b)",
            ),
            bare_marker: re(r"(?m)^(?P<n>[0-9]{1,4})(?: (?:of|od) |/)(?P<m>[0-9]{1,4})$"),
            version: re(
                r"(?-u:\b)(?:(?P<kw>version|verzija|verzije|ver|v|revision|revizija|rev)(?: no| br)? (?P<n>[0-9]{1,3}(?:\.[0-9]{1,2})?)|v(?P<n2>[0-9]{1,3}(?:\.[0-9]{1,2})?)|rev(?P<n3>[0-9]{1,3})|(?P<w>execution version|execution copy|executed version|final version|final draft|finalna verzija|konacna verzija|konacni tekst|clean version|clean copy|redlined|redline|blackline|draft|nacrt|final))(?-u:\b)",
            ),
            generic_label: re(
                r"^(?:(?:document|doc|reference|ref|our ref|your ref|contract|delovodni) )?(?:no|nr|number|num|br|broj|ref|reference)$",
            ),
            amendment_tag: re(r"(?:^|[^A-Za-z0-9])AM[_\s-]?([0-9]{1,2})(?:[^0-9]|$)"),
            change_order_tag: re(r"(?:^|[^A-Za-z0-9])CO[_\s-]?([0-9]{1,2})(?:[^0-9]|$)"),
        }
    }

    /// The relation a trigger right before a mention names (`lookback` is the key line of
    /// up to six tokens before it, on its line).
    pub fn trigger(&self, lookback: &str) -> Option<Relation> {
        self.triggers
            .iter()
            .find(|(_, r)| r.is_match(lookback))
            .map(|(rel, _)| *rel)
    }

    /// A date written at the start of `keys` after a date word ("dated 1 March 2021",
    /// "od 12.03.2021"), and the key bytes it took.
    pub fn date_at(&self, keys: &str) -> Option<(Date, usize)> {
        let c = self.date.captures(keys)?;
        let num = |n: &str| c.name(n).and_then(|m| m.as_str().parse::<u32>().ok());
        let month = |n: &str| c.name(n).and_then(|m| month_of(m.as_str()));
        let (y, m, d) = if let Some(y) = num("y1") {
            (y, num("m1")?, num("d1")?)
        } else if let Some(y) = num("y2") {
            (y, num("m2")?, num("d2")?)
        } else if let Some(y) = num("y3") {
            (y, month("n3")?, num("d3")?)
        } else {
            (num("y4")?, month("n4")?, num("d4")?)
        };
        let date = valid_date(y, m, d)?;
        Some((date, c.get(0)?.end()))
    }
}

fn month_of(name: &str) -> Option<u32> {
    let p = name.get(..3)?;
    Some(match p {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "maj" | "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "avg" | "aug" => 8,
        "sep" => 9,
        "okt" | "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

/// A real calendar date between 1900 and 2100.
pub(crate) fn valid_date(y: u32, m: u32, d: u32) -> Option<Date> {
    if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) || d == 0 {
        return None;
    }
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d > days {
        return None;
    }
    Some(Date {
        y: i16::try_from(y).ok()?,
        m: u8::try_from(m).ok()?,
        d: u8::try_from(d).ok()?,
    })
}

/// A Roman numeral from I to XXX, as an ordinal.
pub(crate) fn roman(s: &str) -> Option<i64> {
    const R: [&str; 30] = [
        "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii", "xiv",
        "xv", "xvi", "xvii", "xviii", "xix", "xx", "xxi", "xxii", "xxiii", "xxiv", "xxv", "xxvi",
        "xxvii", "xxviii", "xxix", "xxx",
    ];
    let l = s.to_ascii_lowercase();
    R.iter()
        .position(|r| *r == l)
        .and_then(|i| i64::try_from(i + 1).ok())
}

/// Ordinal of an ordinal word ("third", "treći" folded to `treci`).
pub(crate) fn ordinal_word(w: &str) -> Option<i64> {
    const EN: [&str; 12] = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
        "tenth", "eleventh", "twelfth",
    ];
    const SR: [&str; 10] = [
        "prv", "drug", "trec", "cetvrt", "pet", "sest", "sedm", "osm", "devet", "deset",
    ];
    if let Some(i) = EN.iter().position(|x| *x == w) {
        return i64::try_from(i + 1).ok();
    }
    // Longest stem first so "deset" is not read as "devet" or "sest".
    let mut best: Option<(usize, usize)> = None;
    for (i, stem) in SR.iter().enumerate() {
        if w.starts_with(stem)
            && w.len() <= stem.len() + 2
            && best.is_none_or(|(_, l)| stem.len() > l)
        {
            best = Some((i, stem.len()));
        }
    }
    best.and_then(|(i, _)| i64::try_from(i + 1).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let p = Patterns::new();
        let d = |s: &str| p.date_at(s).map(|(d, _)| (d.y, d.m, d.d));
        assert_eq!(d("dated 1 march 2021"), Some((2021, 3, 1)));
        assert_eq!(d("dated as of march 1 2021"), Some((2021, 3, 1)));
        assert_eq!(d("od 12.03.2021 godine"), Some((2021, 3, 12)));
        assert_eq!(d("od 12 marta 2021"), Some((2021, 3, 12)));
        assert_eq!(d("dated 2021-03-01"), Some((2021, 3, 1)));
        assert_eq!(d("dated 31.02.2021"), None);
        assert_eq!(d("1 march 2021"), None);
    }

    #[test]
    fn ordinals() {
        assert_eq!(roman("III"), Some(3));
        assert_eq!(roman("iiii"), None);
        assert_eq!(ordinal_word("third"), Some(3));
        assert_eq!(ordinal_word("treci"), Some(3));
        assert_eq!(ordinal_word("deseti"), Some(10));
        assert_eq!(ordinal_word("drugi"), Some(2));
    }

    #[test]
    fn triggers() {
        let p = Patterns::new();
        assert_eq!(p.trigger("aneks br 2 uz"), Some(Relation::Amends));
        assert_eq!(p.trigger("amendment no 2 to the"), Some(Relation::Amends));
        assert_eq!(p.trigger("pursuant to the"), Some(Relation::IssuedUnder));
        assert_eq!(p.trigger("na osnovu"), Some(Relation::IssuedUnder));
        assert_eq!(p.trigger("exhibit b to the"), Some(Relation::AnnexTo));
        assert_eq!(p.trigger("the price"), None);
    }
}

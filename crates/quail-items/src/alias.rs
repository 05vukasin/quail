//! A small curated layer over the CPV index: common English and Serbian service phrases that
//! CPV labels do not use (`consulting`, `project management`, `medical writing`, `SaaS
//! subscription`, `Phase III`, `Senior Consultant – Onshore`) mapped to the CPV service
//! codes a procurement officer would pick.
//!
//! A phrase is a sequence of slots; each slot lists alternatives separated by `|`. An
//! alternative matches a word of the description that starts with it, or, when it ends
//! with `$`, a word equal to it. The slots must match consecutive words (`Phase III`, not
//! `pH … IV`). Aliases read the description only, never the section above it. Phrases with
//! more slots are more specific and win; among equally specific ones
//! the table order decides. At most [`MAX_CODES`] codes are taken, in order, without
//! repeats.

use quail_text::fold::{FoldOptions, fold};

/// Codes returned at most.
pub const MAX_CODES: usize = 3;

const CLIN: &[&str] = &["73110000", "73100000", "73000000"];
const STAT: &[&str] = &["79330000", "72316000", "73110000"];
const CONS: &[&str] = &["79411000", "79410000", "79400000"];
const PM: &[&str] = &["79421000", "72224000", "79411000"];
const ROLE: &[&str] = &["79620000", "79411000", "79421000"];
const SWLIC: &[&str] = &["48000000", "72268000", "72416000"];
const SAAS: &[&str] = &["72416000", "48000000", "72268000"];
const SWDEV: &[&str] = &["72262000", "72200000", "72212000"];
const SUPPORT: &[&str] = &["72261000", "72610000", "72267000"];
const ANALYTICS: &[&str] = &["72316000", "79330000", "72221000"];
const MKT: &[&str] = &["79342000", "79340000", "79310000"];
const LEGAL: &[&str] = &["79100000", "79110000", "79111000"];
const TRAVEL: &[&str] = &["79997000", "63515000", "63510000"];
const TRAINING: &[&str] = &["80500000", "80511000", "80533000"];

/// `(phrase, codes)`, see the module documentation.
const ALIASES: &[(&str, &[&str])] = &[
    // Clinical research and biostatistics.
    ("medic writ", &["73110000", "73000000", "79550000"]),
    ("clinic monitor", CLIN),
    ("clinic trial|stud", CLIN),
    ("phase$|ph$ i$|ii$|iii$|iv$|ib$|iia$|iib$", CLIN),
    ("stud report", CLIN),
    ("!csr$", CLIN),
    ("data manag", &["72322000", "72300000", "72316000"]),
    (
        "data process|entr|captur|collect",
        &["72310000", "72312000", "72300000"],
    ),
    (
        "!sdtm$|adam$|cdisc$|dataset",
        &["72330000", "79330000", "72316000"],
    ),
    ("!tfl$|tfls$|tlf$|tlfs$|statist|biostat", STAT),
    ("surviv|interim analys", STAT),
    (
        "!pharmacovigil|regulator",
        &["73110000", "79411000", "85100000"],
    ),
    (
        "!clinic|oncolog|patient|trial$|protocol|idmc$|dmc$|investigat",
        CLIN,
    ),
    // Management and consulting.
    ("project|program|programme manag|leader|lead$", PM),
    (
        "account|engagement manag|director",
        &["79420000", "79411000", "79620000"],
    ),
    ("pmo$|pm$", PM),
    ("manag servic", &["72510000", "72514000", "79400000"]),
    ("strateg", &["79411000", "79410000", "72222000"]),
    ("!consult|advis|konsalt|savetov|konsultant", CONS),
    ("professional$", CONS),
    (
        "analyst$|associate$|assoc$|principal$|manager$|director$|leader$|specialist$|\
         consultant$|engineer$|architect$|developer$|partner$|scientist$|coordinator$|\
         officer$|expert$|writer$|programmer$|statistician$|menadzer$|rukovodilac$",
        ROLE,
    ),
    // Software and IT.
    ("softwar licen|licenc", SWLIC),
    ("!licen|licenc|licenca|licence", SWLIC),
    ("vmware|sap$|salesforce|veeva|oracle$|microsoft", SWLIC),
    ("!saas$|subscript|pretplat", SAAS),
    ("user$|users$ month|year$", SAAS),
    ("softwar develop", SWDEV),
    ("razvoj softver", SWDEV),
    ("implement", &["72263000", "72224100", "72262000"]),
    ("configur|integrat", &["72265000", "72227000", "72263000"]),
    ("support|helpdesk", SUPPORT),
    ("host|cloud", &["72415000", "72416000", "72317000"]),
    ("analytic|insight|dashboard|report|izvestavanj", ANALYTICS),
    // Business services.
    (
        "market|campaign|brand|promot|target|segment|marketin|oglasavanj|reklam",
        MKT,
    ),
    ("advertis", &["79341000", "79340000", "79342000"]),
    ("survey", &["79310000", "79311000", "79330000"]),
    ("istrazivanj trzist", &["79310000", "79311000", "79330000"]),
    ("!legal|lawyer|attorney|counsel|notar|advokat|pravn", LEGAL),
    ("!translat|prevod", &["79530000", "79540000"]),
    ("train|workshop|course|obuk", TRAINING),
    (
        "recruit|staffing|fte$|ftes$|augment",
        &["79620000", "79600000", "79610000"],
    ),
    ("audit|revizij", &["79212000", "79210000"]),
    ("accounting|bookkeep|knjigovod", &["79211000", "79210000"]),
    ("!payroll", &["79211110", "79631000"]),
    ("!travel|accommodat|hotel|airfare", TRAVEL),
    ("putni trosk", TRAVEL),
    ("sluzben put", TRAVEL),
    ("t$ e$", TRAVEL),
    (
        "event|conferenc|congress|kongres",
        &["79952000", "79951000"],
    ),
    ("print|stamp", &["79810000", "79800000"]),
    (
        "graphic|mockup|wirefram|ux$",
        &["79822500", "79415200", "72413000"],
    ),
    ("liabil insur", &["66516000", "66516400", "66516500"]),
    ("!insur|osiguranj", &["66510000"]),
    (
        "shipping|freight|courier",
        &["60000000", "64120000", "63000000"],
    ),
];

/// Words of a description for alias matching: folded, split on anything but letters and
/// digits (digits kept: `Phase 3`).
fn words(text: &str) -> Vec<String> {
    let f = fold(text, &FoldOptions::KEY);
    f.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn alt_matches(alt: &str, w: &str) -> bool {
    match alt.strip_suffix('$') {
        Some(exact) => w == exact,
        None => w.starts_with(alt),
    }
}

/// Whether the slots of `phrase` match consecutive words of `w`.
fn matches(phrase: &str, w: &[String]) -> bool {
    let slots: Vec<&str> = phrase.split(' ').filter(|s| !s.is_empty()).collect();
    let hit = |slot: &str, x: &str| slot.split('|').any(|alt| alt_matches(alt.trim(), x));
    (0..w.len()).any(|start| {
        slots.len() <= w.len() - start
            && slots.iter().zip(&w[start..]).all(|(slot, x)| hit(slot, x))
    })
}

/// The alias codes (8 digits, best first) for a description. Phrases of one slot (`support`,
/// a role word, `target`) count only when the line reads as a service (`service`), unless
/// they start with `!` (unambiguous terms: `consult`, `licence`, `SaaS`, `clinical`, `TFL`,
/// `legal`, `travel`); longer phrases always count.
pub(crate) fn codes(text: &str, service: bool) -> Vec<&'static str> {
    let w = words(text);
    if w.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(usize, usize, &'static [&'static str])> = ALIASES
        .iter()
        .enumerate()
        .filter_map(|(i, (p, c))| {
            let strong = p.starts_with('!');
            let p = p.trim_start_matches('!');
            let slots = p.split(' ').filter(|s| !s.is_empty()).count();
            (matches(p, &w) && (service || strong || slots > 1)).then_some((slots, i, *c))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<&'static str> = Vec::new();
    for (_, _, cs) in hits {
        for c in cs {
            if out.len() < MAX_CODES && !out.contains(c) {
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_map_to_service_codes() {
        assert_eq!(codes("Senior Consultant – Onshore", true)[0], "79411000");
        assert_eq!(codes("Project Leader", true)[0], "79421000");
        assert_eq!(codes("Snr Medical Writer", true)[0], "73110000");
        assert_eq!(
            codes("Phase III remaining deliverables", true)[0],
            "73110000"
        );
        assert_eq!(
            codes("Supplier SaaS Product Subscription-2025", true)[0],
            "72416000"
        );
        assert_eq!(
            codes("Software License Fees for Jan 2025", true)[0],
            "48000000"
        );
        assert_eq!(
            codes("Travel & Administrative Expenses", true)[0],
            "79997000"
        );
        assert_eq!(codes("Usluge prevođenja", true)[0], "79530000");
        assert!(codes("Pumpa za vodu 1,1 kW", true).is_empty());
        assert!(codes("", true).is_empty());
        assert!(codes("Support bracket", false).is_empty());
        assert!(codes("3 phase motor", true).is_empty());
        assert_eq!(codes("Phase III", false)[0], "73110000");
        assert!(codes("Senior Consultant", true).len() <= MAX_CODES);
    }

    #[test]
    fn every_alias_code_is_a_cpv_code() {
        let ix = crate::cpv::CpvIndex::embedded();
        for (p, cs) in ALIASES {
            for c in *cs {
                assert!(ix.entry_of_code(c).is_some(), "{p}: {c}");
            }
        }
    }
}

//! What kind of thing is paid for: one category from a small taxonomy, by keywords in Serbian
//! and English.
//!
//! The whole taxonomy is [`CATEGORIES`]: add a keyword or a category there and nothing else
//! changes. Keywords are folded (lowercase, no diacritics, Cyrillic → Latin) and match one
//! token; a trailing `*` matches any ending (`softver*` covers `softvera`, `softverskog`).
//! Each keyword has a weight: specific words (`softver`, `osiguranje`) weigh 3, words that also
//! appear around other services (`održavanje`, `oprema`) weigh 1.

/// One category of the taxonomy.
#[derive(Debug)]
pub struct Category {
    /// Stable code, written into `payment.category`.
    pub code: &'static str,
    /// English label.
    pub label: &'static str,
    /// Keywords and their weights.
    pub keywords: &'static [(&'static str, u8)],
}

/// The taxonomy, in tie-break order. `other` (no keyword) is the answer when a subject is
/// known and no keyword matches.
pub const CATEGORIES: &[Category] = &[
    Category {
        code: "it_software",
        label: "IT and software",
        keywords: &[
            ("softver*", 3),
            ("software", 3),
            ("aplikacij*", 2),
            ("application*", 1),
            ("saas", 3),
            ("hosting*", 3),
            ("cloud", 2),
            ("server*", 2),
            ("racunar*", 2),
            ("computer*", 2),
            ("it", 1),
            ("informacion*", 2),
            ("informatick*", 2),
            ("programir*", 2),
            ("programming", 2),
            ("programer*", 2),
            ("developer*", 2),
            ("development", 1),
            ("website", 3),
            ("web", 2),
            ("sajt*", 2),
            ("erp", 3),
            ("crm", 2),
            ("licenc*", 2),
            ("licens*", 2),
            ("database*", 2),
            ("digital*", 1),
            ("helpdesk", 2),
        ],
    },
    Category {
        code: "telecom",
        label: "Telecom",
        keywords: &[
            ("telekomunikac*", 3),
            ("telecom*", 3),
            ("telecommunication*", 3),
            ("internet*", 2),
            ("mobiln*", 2),
            ("mobile", 2),
            ("telefon*", 2),
            ("phone*", 2),
            ("broadband", 3),
            ("roaming", 3),
            ("sim", 2),
        ],
    },
    Category {
        code: "legal",
        label: "Legal",
        keywords: &[
            ("pravn*", 2),
            ("advokat*", 3),
            ("legal", 3),
            ("attorney*", 3),
            ("lawyer*", 3),
            ("notar*", 3),
            ("litigation", 3),
            ("counsel", 2),
            ("zastupanj*", 1),
        ],
    },
    Category {
        code: "accounting_audit",
        label: "Accounting and audit",
        keywords: &[
            ("racunovodstv*", 3),
            ("knjigovodstv*", 3),
            ("revizij*", 3),
            ("revizor*", 3),
            ("audit*", 3),
            ("accounting", 3),
            ("bookkeeping", 3),
            ("payroll", 3),
            ("obracun*", 1),
            ("poresk*", 1),
            ("tax", 1),
        ],
    },
    Category {
        code: "consulting",
        label: "Consulting",
        keywords: &[
            ("konsalting*", 3),
            ("konsultant*", 3),
            ("konsultac*", 3),
            ("savetovanj*", 3),
            ("savetodavn*", 3),
            ("consulting", 3),
            ("consultancy", 3),
            ("consultant*", 3),
            ("advisory", 3),
        ],
    },
    Category {
        code: "marketing",
        label: "Marketing",
        keywords: &[
            ("marketin*", 3),
            ("reklam*", 3),
            ("oglas*", 2),
            ("advertis*", 3),
            ("promocij*", 2),
            ("promotion*", 2),
            ("medij*", 1),
            ("media", 1),
            ("kampanj*", 2),
            ("campaign*", 2),
            ("branding", 3),
            ("seo", 3),
        ],
    },
    Category {
        code: "rent_lease",
        label: "Rent and lease",
        keywords: &[
            ("zakup*", 3),
            ("najam*", 3),
            ("najm*", 3),
            ("iznajmljiv*", 3),
            ("rent*", 3),
            ("lease*", 3),
            ("leasing", 3),
            ("prostor*", 1),
            ("premises", 1),
        ],
    },
    Category {
        code: "utilities",
        label: "Utilities",
        keywords: &[
            ("komunaln*", 3),
            ("utilit*", 3),
            ("voda", 2),
            ("vode", 1),
            ("vodu", 2),
            ("vodovod*", 3),
            ("water", 2),
            ("kanalizac*", 3),
            ("sewage", 3),
            ("grejanj*", 3),
            ("heating", 3),
            ("otpad*", 2),
            ("waste", 2),
        ],
    },
    Category {
        code: "fuel_energy",
        label: "Fuel and energy",
        keywords: &[
            ("goriv*", 3),
            ("fuel", 3),
            ("dizel*", 3),
            ("diesel", 3),
            ("benzin*", 3),
            ("petrol", 3),
            ("gasoline", 3),
            ("energij*", 2),
            ("energy", 2),
            ("elektricn*", 2),
            ("electricity", 3),
            ("gas", 2),
            ("plin*", 2),
            ("naft*", 2),
        ],
    },
    Category {
        code: "logistics_transport",
        label: "Logistics and transport",
        keywords: &[
            ("transport*", 3),
            ("prevoz*", 3),
            ("spedicij*", 3),
            ("shipping", 3),
            ("freight", 3),
            ("logistic*", 3),
            ("logistik*", 3),
            ("dostav*", 1),
            ("delivery", 1),
            ("kurir*", 3),
            ("courier*", 3),
            ("carin*", 2),
            ("customs", 2),
        ],
    },
    Category {
        code: "cleaning_facility",
        label: "Cleaning and facility",
        keywords: &[
            ("ciscenj*", 3),
            ("higijen*", 2),
            ("cleaning", 3),
            ("janitor*", 3),
            ("facility", 2),
            ("obezbedjenj*", 2),
            ("security", 1),
            ("dezinfekcij*", 2),
            ("deratizacij*", 3),
        ],
    },
    Category {
        code: "maintenance_repair",
        label: "Maintenance and repair",
        keywords: &[
            ("odrzavanj*", 1),
            ("maintenance", 1),
            ("servisiranj*", 2),
            ("servis*", 1),
            ("popravk*", 2),
            ("repair*", 2),
            ("remont*", 2),
            ("instalacij*", 1),
            ("installation", 1),
            ("montaz*", 1),
        ],
    },
    Category {
        code: "insurance",
        label: "Insurance",
        keywords: &[
            ("osiguranj*", 3),
            ("insurance", 3),
            ("polis*", 2),
            ("premij*", 2),
            ("premium", 1),
        ],
    },
    Category {
        code: "goods_materials",
        label: "Goods and materials",
        keywords: &[
            ("materijal*", 2),
            ("material*", 2),
            ("roba", 2),
            ("robe", 2),
            ("robu", 2),
            ("goods", 2),
            ("oprem*", 1),
            ("equipment", 1),
            ("kancelarijsk*", 1),
            ("supplies", 2),
            ("toner*", 2),
            ("papir*", 1),
            ("paper", 1),
            ("namestaj*", 2),
            ("furniture", 2),
            ("delov*", 1),
            ("parts", 1),
            ("proizvod*", 1),
            ("products", 1),
            ("artikl*", 1),
            ("isporuk*", 1),
        ],
    },
    Category {
        code: "financial_services",
        label: "Financial services",
        keywords: &[
            ("bankarsk*", 3),
            ("banking", 3),
            ("kredit*", 3),
            ("loan*", 3),
            ("faktoring*", 3),
            ("factoring", 3),
            ("platn*", 1),
            ("provizij*", 2),
            ("commission*", 1),
            ("brokers*", 3),
            ("brokerage", 3),
            ("kastodi*", 3),
            ("custody", 3),
            ("investicion*", 2),
        ],
    },
    Category {
        code: "other",
        label: "Other",
        keywords: &[],
    },
];

/// The answer for one text.
#[derive(Clone, Debug)]
pub struct Pick {
    /// The category.
    pub category: &'static Category,
    /// Summed weight of its matched keywords.
    pub score: u32,
    /// Another category scored as high.
    pub tied: bool,
    /// Index of the token with the heaviest matched keyword (first one on ties).
    pub token: usize,
}

fn matches(kw: &str, key: &str) -> bool {
    crate::lex::word_matches(kw, key)
}

/// The category of the folded tokens `keys`, or `None` when no keyword matches.
pub fn categorize<'k>(keys: impl IntoIterator<Item = &'k str>) -> Option<Pick> {
    let mut score = vec![0u32; CATEGORIES.len()];
    let mut best_tok = vec![(0u8, usize::MAX); CATEGORIES.len()];
    for (i, key) in keys.into_iter().enumerate() {
        for (c, cat) in CATEGORIES.iter().enumerate() {
            if let Some(&(_, w)) = cat.keywords.iter().find(|(kw, _)| matches(kw, key)) {
                score[c] += u32::from(w);
                if w > best_tok[c].0 {
                    best_tok[c] = (w, i);
                }
            }
        }
    }
    let top = *score.iter().max()?;
    if top == 0 {
        return None;
    }
    let c = score.iter().position(|&s| s == top)?;
    Some(Pick {
        category: &CATEGORIES[c],
        score: top,
        tied: score.iter().filter(|&&s| s == top).count() > 1,
        token: best_tok[c].1,
    })
}

/// The `other` category.
pub fn other() -> &'static Category {
    CATEGORIES
        .iter()
        .find(|c| c.code == "other")
        .unwrap_or(&CATEGORIES[CATEGORIES.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(words: &str) -> Option<&'static str> {
        let keys: Vec<String> = words.split(' ').map(str::to_owned).collect();
        categorize(keys.iter().map(String::as_str)).map(|p| p.category.code)
    }

    #[test]
    fn specific_words_beat_generic_ones() {
        assert_eq!(code("odrzavanje softvera"), Some("it_software"));
        assert_eq!(code("odrzavanje lifta"), Some("maintenance_repair"));
        assert_eq!(code("zakup poslovnog prostora"), Some("rent_lease"));
        assert_eq!(code("software maintenance and support"), Some("it_software"));
        assert_eq!(code("legal services"), Some("legal"));
        assert_eq!(code("isporuka dizel goriva"), Some("fuel_energy"));
        assert_eq!(code("nesto sasvim drugo"), None);
    }

    #[test]
    fn codes_are_unique_and_other_is_last() {
        let mut codes: Vec<_> = CATEGORIES.iter().map(|c| c.code).collect();
        assert_eq!(codes.last(), Some(&"other"));
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), CATEGORIES.len());
    }
}

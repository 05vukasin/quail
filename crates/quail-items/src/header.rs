//! Column roles from header cells, in English and Serbian (Latin and Cyrillic: everything is
//! folded first, so `Количина` reads as `kolicina`).

use crate::num;

/// What a table column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Role {
    Ordinal,
    Code,
    Desc,
    Unit,
    Qty,
    UnitPrice,
    Total,
    VatRate,
    VatAmount,
    Discount,
    Currency,
    StartDate,
    EndDate,
}

/// A header cell read as a role, with what its words say about VAT and currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeaderCell {
    pub role: Role,
    /// `Some(true)` with VAT (`sa PDV-om`, `incl. VAT`, `gross`), `Some(false)` without.
    pub vat: Option<bool>,
    /// Currency named in the header (`Cena (RSD)`, `Amount USD`, `u dinarima`).
    pub currency: Option<[u8; 3]>,
}

/// Folded words of a header cell; `%`, `#` and `$`/`€` kept as words.
pub(crate) fn words(text: &str) -> Vec<String> {
    let folded = quail_text::fold::fold(text, &quail_text::fold::FoldOptions::KEY);
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in folded.chars() {
        if c.is_alphanumeric() {
            cur.push(c);
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if matches!(c, '%' | '#' | '$' | '€' | '£') {
                out.push(c.to_string());
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn any(w: &[String], set: &[&str]) -> bool {
    w.iter().any(|x| set.contains(&x.as_str()))
}

fn prefix(w: &[String], set: &[&str]) -> bool {
    w.iter().any(|x| set.iter().any(|p| x.starts_with(p)))
}

fn pair(w: &[String], a: &[&str], b: &[&str]) -> bool {
    w.windows(2)
        .any(|p| a.contains(&p[0].as_str()) && b.contains(&p[1].as_str()))
}

const VAT_WORDS: &[&str] = &["pdv", "vat", "tax", "porez", "pdva", "gst"];
const TOTAL_PREFIX: &[&str] = &[
    "ukupn",
    "sveukupn",
    "vrednost",
    "vrijednost",
    "iznos",
    "subtotal",
];
const TOTAL_WORDS: &[&str] = &[
    "total",
    "amount",
    "sum",
    "ext",
    "extended",
    "extension",
    "svega",
    "net",
    "gross",
];
const PRICE_PREFIX: &[&str] = &[
    "cen", "cijen", "tarif", "naknad", "price", "rate", "cost", "fee",
];
const UNIT_QUAL: &[&str] = &[
    "jedinicna",
    "jedinicne",
    "jed",
    "unit",
    "each",
    "per",
    "po",
    "pojedinici",
    "hourly",
    "daily",
];

/// The role of a header cell, if its words name one.
#[allow(clippy::too_many_lines)]
pub(crate) fn role(text: &str) -> Option<HeaderCell> {
    let w = words(text);
    if w.is_empty() || w.len() > 10 || text.len() > 90 {
        return None;
    }
    // A header names things; a cell with a money amount is data.
    if num::scan(text, num::Dec::Unknown, num::DateOrder::Dmy)
        .iter()
        .any(|t| t.number().is_some_and(|n| n.units.abs() >= 100) && !t.glued)
    {
        return None;
    }
    let vat = if pair(&w, &["bez"], VAT_WORDS)
        || any(
            &w,
            &["excl", "exclusive", "excluding", "neto", "net", "nett"],
        )
        || pair(&w, &["before", "ex"], VAT_WORDS)
    {
        Some(false)
    } else if pair(
        &w,
        &[
            "sa",
            "s",
            "with",
            "incl",
            "inc",
            "including",
            "ukljucujuci",
            "uracunatim",
            "uracunat",
        ],
        VAT_WORDS,
    ) || any(&w, &["bruto", "gross", "inclusive"])
    {
        Some(true)
    } else {
        None
    };
    let currency = w.iter().find_map(|x| match x.as_str() {
        "dinarima" | "dinara" | "rsd" | "din" => Some(*b"RSD"),
        "evrima" | "eurima" | "eur" | "euro" | "€" => Some(*b"EUR"),
        "dolarima" | "usd" | "$" => Some(*b"USD"),
        "gbp" | "£" => Some(*b"GBP"),
        "chf" => Some(*b"CHF"),
        _ => None,
    });
    let has = |r: Role| {
        Some(HeaderCell {
            role: r,
            vat,
            currency,
        })
    };

    let vat_word = any(&w, VAT_WORDS);
    let price = prefix(&w, PRICE_PREFIX);
    let total = prefix(&w, TOTAL_PREFIX) || any(&w, TOTAL_WORDS);
    if vat_word
        && (any(&w, &["%", "stopa", "rate", "procenat"])
            || pair(&w, &["tax", "vat"], &["rate", "code"]))
    {
        return has(Role::VatRate);
    }
    if vat_word && vat.is_none() && !price && !any(&w, &["ukupno", "ukupna", "ukupan", "total"]) {
        // `PDV`, `Iznos PDV-a`, `VAT amount`, `Tax`.
        return has(Role::VatAmount);
    }
    if prefix(&w, &["rabat", "popust", "discount"]) {
        return has(Role::Discount);
    }
    if price && (any(&w, UNIT_QUAL) || prefix(&w, &["jedinic"])) && !prefix(&w, &["ukupn"]) {
        return has(Role::UnitPrice);
    }
    if total {
        return has(Role::Total);
    }
    if price {
        return has(Role::UnitPrice);
    }
    if prefix(&w, &["kolicin", "kolic", "quantit"])
        || any(
            &w,
            &["kol", "qty", "qnty", "spots", "hours", "sati", "broj"],
        ) && w.len() <= 3
    {
        return has(Role::Qty);
    }
    if pair(
        &w,
        &["jedinica", "jed", "j", "unit"],
        &["mere", "mera", "mjere", "m", "of", "measure"],
    ) || any(
        &w,
        &[
            "jm", "uom", "um", "mera", "mere", "jedinica", "unit", "units", "jmj",
        ],
    ) {
        return has(Role::Unit);
    }
    if pair(
        &w,
        &["item", "part", "art", "article", "product"],
        &["no", "number", "code", "nr", "#"],
    ) || prefix(&w, &["sifr", "katalos"])
        || any(&w, &["code", "sku", "kod"])
    {
        return has(Role::Code);
    }
    if prefix(
        &w,
        &[
            "naziv",
            "opis",
            "uslug",
            "predmet",
            "vrsta",
            "artik",
            "proizvod",
            "dobr",
            "rob",
            "stavk",
            "pozicij",
            "descr",
            "item",
            "service",
            "product",
            "particular",
            "detail",
            "program",
            "deliverable",
            "activit",
            "aktivnost",
            "role",
            "position",
            "pozicija",
            "specifikac",
            "radov",
            "rad",
            "posao",
            "poslov",
            "materijal",
        ],
    ) {
        return has(Role::Desc);
    }
    if any(
        &w,
        &[
            "rb", "rbr", "no", "nr", "#", "pos", "poz", "ln", "line", "redni", "br", "r",
        ],
    ) && w.len() <= 3
    {
        return has(Role::Ordinal);
    }
    if any(&w, &["start", "from", "od", "begin", "pocetak"])
        && any(&w, &["date", "datum", "od", "from", "start"])
    {
        return has(Role::StartDate);
    }
    if any(&w, &["end", "to", "do", "until", "kraj"])
        && any(&w, &["date", "datum", "do", "to", "end"])
    {
        return has(Role::EndDate);
    }
    if any(&w, &["date", "datum"]) {
        return has(Role::StartDate);
    }
    if any(&w, &["valuta", "currency", "curr"]) {
        return has(Role::Currency);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Option<Role> {
        role(s).map(|h| h.role)
    }

    #[test]
    fn serbian_and_english_headers() {
        assert_eq!(r("Naziv usluge"), Some(Role::Desc));
        assert_eq!(r("Опис"), Some(Role::Desc));
        assert_eq!(r("Jedinica mere"), Some(Role::Unit));
        assert_eq!(r("J.M."), Some(Role::Unit));
        assert_eq!(r("Količina"), Some(Role::Qty));
        assert_eq!(r("Kol."), Some(Role::Qty));
        assert_eq!(r("Jedinična cena bez PDV-a"), Some(Role::UnitPrice));
        assert_eq!(
            role("Jedinična cena bez PDV-a").and_then(|h| h.vat),
            Some(false)
        );
        assert_eq!(r("Cena"), Some(Role::UnitPrice));
        assert_eq!(r("Ukupna cena sa PDV-om"), Some(Role::Total));
        assert_eq!(
            role("Ukupna cena sa PDV-om").and_then(|h| h.vat),
            Some(true)
        );
        assert_eq!(r("Iznos"), Some(Role::Total));
        assert_eq!(r("PDV %"), Some(Role::VatRate));
        assert_eq!(r("Iznos PDV-a"), Some(Role::VatAmount));
        assert_eq!(r("Р.бр."), Some(Role::Ordinal));
        assert_eq!(r("Šifra"), Some(Role::Code));
        assert_eq!(r("Description"), Some(Role::Desc));
        assert_eq!(r("Qty"), Some(Role::Qty));
        assert_eq!(r("Unit"), Some(Role::Unit));
        assert_eq!(r("Unit Price"), Some(Role::UnitPrice));
        assert_eq!(r("Rate"), Some(Role::UnitPrice));
        assert_eq!(r("Amount"), Some(Role::Total));
        assert_eq!(r("VAT"), Some(Role::VatAmount));
        assert_eq!(r("Item No."), Some(Role::Code));
        assert_eq!(r("Start Date"), Some(Role::StartDate));
        assert_eq!(r("End Date"), Some(Role::EndDate));
        assert_eq!(role("Cena (RSD)").and_then(|h| h.currency), Some(*b"RSD"));
        assert_eq!(r("Usluga čišćenja poslovnog prostora 1.200,00"), None);
    }
}

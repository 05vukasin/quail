//! Units of measure: what a quantity or a price is counted in, normalized to a short code.
//!
//! | code | written as (folded) |
//! |---|---|
//! | `pcs` | kom, komad, komada, kos, pcs, pc, piece, pieces, ea, each, unit, units, stk |
//! | `h` | h, hr, hrs, hour, hours, sat, sati, sata, cas, casova, manhour, cs (čovek-sat) |
//! | `day` | dan, dana, day, days, md (man-day), mandays, radni dan |
//! | `week`, `month`, `year` | nedelja, week, mesec, mes, month, mo, godina, god, year, yr, annual ... |
//! | `min` | min, minut, minuta, minute, minutes |
//! | `m`, `m2`, `m3`, `km` | m, m1, metar, m2, m², kvm, sqm, m3, m³, km |
//! | `kg`, `g`, `t` | kg, kilogram, g, gr, t, tona, ton, tonne |
//! | `l` | l, lit, litar, litara, liter, litre |
//! | `kwh` | kwh |
//! | `lump_sum` | pausal, pausalno, lump, ls, flat |
//! | `set`, `pack`, `pair` | set, komplet, kpl, pak, paket, pack, par, pair |
//! | `person`, `night`, `trip`, `page`, `spot`, `item` | osoba, lice, person, pax, noc, nocenje, night, tura, voznja, trip, strana, page, spot, stavka, item |

/// Normalized code of a folded unit word, if it is one.
pub(crate) fn normalize(key: &str) -> Option<&'static str> {
    let k = key.trim_matches(|c: char| c == '.' || c == ':' || c == '(' || c == ')');
    let k = k.strip_prefix("po ").unwrap_or(k);
    let k = k.strip_prefix("per ").unwrap_or(k);
    UNITS.iter().find(|(w, _)| *w == k).map(|(_, c)| *c)
}

const UNITS: &[(&str, &str)] = &[
    ("kom", "pcs"),
    ("komad", "pcs"),
    ("komada", "pcs"),
    ("komadu", "pcs"),
    ("kos", "pcs"),
    ("pcs", "pcs"),
    ("pc", "pcs"),
    ("piece", "pcs"),
    ("pieces", "pcs"),
    ("ea", "pcs"),
    ("each", "pcs"),
    ("unit", "pcs"),
    ("units", "pcs"),
    ("stk", "pcs"),
    ("h", "h"),
    ("hr", "h"),
    ("hrs", "h"),
    ("hour", "h"),
    ("hours", "h"),
    ("hourly", "h"),
    ("sat", "h"),
    ("sati", "h"),
    ("sata", "h"),
    ("satu", "h"),
    ("cas", "h"),
    ("casova", "h"),
    ("casu", "h"),
    ("manhour", "h"),
    ("cs", "h"),
    ("cov.sat", "h"),
    ("dan", "day"),
    ("dana", "day"),
    ("danu", "day"),
    ("day", "day"),
    ("days", "day"),
    ("daily", "day"),
    ("md", "day"),
    ("manday", "day"),
    ("mandays", "day"),
    ("nedelja", "week"),
    ("nedeljno", "week"),
    ("sedmica", "week"),
    ("week", "week"),
    ("weeks", "week"),
    ("weekly", "week"),
    ("wk", "week"),
    ("mesec", "month"),
    ("meseci", "month"),
    ("mesecno", "month"),
    ("mesecu", "month"),
    ("mes", "month"),
    ("mjesec", "month"),
    ("mjesecno", "month"),
    ("month", "month"),
    ("months", "month"),
    ("monthly", "month"),
    ("mo", "month"),
    ("mth", "month"),
    ("godina", "year"),
    ("godine", "year"),
    ("godisnje", "year"),
    ("godisnji", "year"),
    ("god", "year"),
    ("year", "year"),
    ("years", "year"),
    ("yearly", "year"),
    ("annual", "year"),
    ("annum", "year"),
    ("p.a", "year"),
    ("annually", "year"),
    ("yr", "year"),
    ("min", "min"),
    ("minut", "min"),
    ("minuta", "min"),
    ("minute", "min"),
    ("minutes", "min"),
    ("m", "m"),
    ("m1", "m"),
    ("metar", "m"),
    ("metara", "m"),
    ("meter", "m"),
    ("metre", "m"),
    ("m2", "m2"),
    ("m²", "m2"),
    ("kvm", "m2"),
    ("sqm", "m2"),
    ("sqft", "sqft"),
    ("m3", "m3"),
    ("m³", "m3"),
    ("km", "km"),
    ("kg", "kg"),
    ("kilogram", "kg"),
    ("kilograma", "kg"),
    ("g", "g"),
    ("gr", "g"),
    ("t", "t"),
    ("tona", "t"),
    ("tone", "t"),
    ("ton", "t"),
    ("tonne", "t"),
    ("tonnes", "t"),
    ("l", "l"),
    ("lit", "l"),
    ("litar", "l"),
    ("litara", "l"),
    ("liter", "l"),
    ("litre", "l"),
    ("litres", "l"),
    ("kwh", "kwh"),
    ("pausal", "lump_sum"),
    ("pausalno", "lump_sum"),
    ("lump", "lump_sum"),
    ("lumpsum", "lump_sum"),
    ("ls", "lump_sum"),
    ("flat", "lump_sum"),
    ("set", "set"),
    ("komplet", "set"),
    ("kompl", "set"),
    ("kpl", "set"),
    ("pak", "pack"),
    ("paket", "pack"),
    ("pack", "pack"),
    ("pkg", "pack"),
    ("par", "pair"),
    ("pair", "pair"),
    ("osoba", "person"),
    ("osobi", "person"),
    ("lice", "person"),
    ("licu", "person"),
    ("person", "person"),
    ("pax", "person"),
    ("noc", "night"),
    ("nocenje", "night"),
    ("night", "night"),
    ("nights", "night"),
    ("tura", "trip"),
    ("voznja", "trip"),
    ("trip", "trip"),
    ("strana", "page"),
    ("str", "page"),
    ("page", "page"),
    ("spot", "spot"),
    ("spots", "spot"),
    ("stavka", "item"),
    ("item", "item"),
    ("usluga", "service"),
];

/// Folds a word for unit lookup: lowercase, no diacritics, Cyrillic to Latin, `čovek-sat`
/// → `cs`, `man-hour` → `manhour`.
pub(crate) fn key(word: &str) -> String {
    let f = quail_text::fold::fold(word.trim(), &quail_text::fold::FoldOptions::KEY);
    let f = f.trim_matches(|c: char| c == '.' || c == ',' || c == ':' || c == ';');
    match f {
        "covek-sat" | "covek sat" | "cov-sat" | "c/s" | "c-s" | "cov.sat" => "cs".to_owned(),
        "man-hour" | "man hour" | "man-hours" => "manhour".to_owned(),
        "man-day" | "man day" | "man-days" | "person-day" => "manday".to_owned(),
        "lump sum" | "lump-sum" => "lumpsum".to_owned(),
        "sq m" | "sq. m" | "sq.m" => "sqm".to_owned(),
        "sq ft" | "sq. ft" | "sq.ft" => "sqft".to_owned(),
        "kw/h" | "kw h" => "kwh".to_owned(),
        _ => f.to_owned(),
    }
}

/// Normalized unit of a whole cell or phrase (`kom.`, `čovek-sat`, `po satu`, `m²`).
pub(crate) fn of_phrase(text: &str) -> Option<&'static str> {
    let k = key(text);
    if k.is_empty() || k.len() > 24 {
        return None;
    }
    normalize(&k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_normalize() {
        assert_eq!(of_phrase("kom."), Some("pcs"));
        assert_eq!(of_phrase("Čovek-sat"), Some("h"));
        assert_eq!(of_phrase("сат"), Some("h"));
        assert_eq!(of_phrase("m²"), Some("m2"));
        assert_eq!(of_phrase("Paušal"), Some("lump_sum"));
        assert_eq!(of_phrase("po satu"), Some("h"));
        assert_eq!(of_phrase("Lump sum"), Some("lump_sum"));
        assert_eq!(of_phrase("consulting"), None);
    }
}

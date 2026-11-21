//! Jurisdictions a governing-law clause names: US states and the District of Columbia,
//! Canadian provinces, Australian states, UK nations and countries. Names are matched as
//! lowercase word sequences, longest first.

/// One jurisdiction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// Canonical English name (`Delaware`, `England and Wales`).
    pub name: &'static str,
    /// ISO 3166-2 subdivision code (`US-DE`) or ISO 3166-1 alpha-2 country code (`DE`).
    pub code: &'static str,
    /// Lowercase words that spell it.
    pub words: &'static [&'static str],
}

macro_rules! places {
    ($( $name:literal, $code:literal, [$($w:literal),+] );+ $(;)?) => {
        &[ $( Place { name: $name, code: $code, words: &[$($w),+] } ),+ ]
    };
}

/// Every known jurisdiction. Several spellings of one place share its name and code.
pub const PLACES: &[Place] = places![
    "Alabama", "US-AL", ["alabama"];
    "Alaska", "US-AK", ["alaska"];
    "Arizona", "US-AZ", ["arizona"];
    "Arkansas", "US-AR", ["arkansas"];
    "California", "US-CA", ["california"];
    "Colorado", "US-CO", ["colorado"];
    "Connecticut", "US-CT", ["connecticut"];
    "Delaware", "US-DE", ["delaware"];
    "District of Columbia", "US-DC", ["district", "of", "columbia"];
    "Florida", "US-FL", ["florida"];
    "Georgia", "US-GA", ["georgia"];
    "Hawaii", "US-HI", ["hawaii"];
    "Idaho", "US-ID", ["idaho"];
    "Illinois", "US-IL", ["illinois"];
    "Indiana", "US-IN", ["indiana"];
    "Iowa", "US-IA", ["iowa"];
    "Kansas", "US-KS", ["kansas"];
    "Kentucky", "US-KY", ["kentucky"];
    "Louisiana", "US-LA", ["louisiana"];
    "Maine", "US-ME", ["maine"];
    "Maryland", "US-MD", ["maryland"];
    "Massachusetts", "US-MA", ["massachusetts"];
    "Michigan", "US-MI", ["michigan"];
    "Minnesota", "US-MN", ["minnesota"];
    "Mississippi", "US-MS", ["mississippi"];
    "Missouri", "US-MO", ["missouri"];
    "Montana", "US-MT", ["montana"];
    "Nebraska", "US-NE", ["nebraska"];
    "Nevada", "US-NV", ["nevada"];
    "New Hampshire", "US-NH", ["new", "hampshire"];
    "New Jersey", "US-NJ", ["new", "jersey"];
    "New Mexico", "US-NM", ["new", "mexico"];
    "New York", "US-NY", ["new", "york"];
    "North Carolina", "US-NC", ["north", "carolina"];
    "North Dakota", "US-ND", ["north", "dakota"];
    "Ohio", "US-OH", ["ohio"];
    "Oklahoma", "US-OK", ["oklahoma"];
    "Oregon", "US-OR", ["oregon"];
    "Pennsylvania", "US-PA", ["pennsylvania"];
    "Rhode Island", "US-RI", ["rhode", "island"];
    "South Carolina", "US-SC", ["south", "carolina"];
    "South Dakota", "US-SD", ["south", "dakota"];
    "Tennessee", "US-TN", ["tennessee"];
    "Texas", "US-TX", ["texas"];
    "Utah", "US-UT", ["utah"];
    "Vermont", "US-VT", ["vermont"];
    "Virginia", "US-VA", ["virginia"];
    "Washington", "US-WA", ["washington"];
    "West Virginia", "US-WV", ["west", "virginia"];
    "Wisconsin", "US-WI", ["wisconsin"];
    "Wyoming", "US-WY", ["wyoming"];
    "Puerto Rico", "US-PR", ["puerto", "rico"];
    "United States", "US", ["united", "states", "of", "america"];
    "United States", "US", ["united", "states"];
    "United States", "US", ["usa"];
    "Ontario", "CA-ON", ["ontario"];
    "Quebec", "CA-QC", ["quebec"];
    "Quebec", "CA-QC", ["québec"];
    "British Columbia", "CA-BC", ["british", "columbia"];
    "Alberta", "CA-AB", ["alberta"];
    "Manitoba", "CA-MB", ["manitoba"];
    "Saskatchewan", "CA-SK", ["saskatchewan"];
    "Nova Scotia", "CA-NS", ["nova", "scotia"];
    "New Brunswick", "CA-NB", ["new", "brunswick"];
    "Canada", "CA", ["canada"];
    "New South Wales", "AU-NSW", ["new", "south", "wales"];
    "Victoria", "AU-VIC", ["victoria"];
    "Queensland", "AU-QLD", ["queensland"];
    "Western Australia", "AU-WA", ["western", "australia"];
    "Australia", "AU", ["australia"];
    "England and Wales", "GB-EAW", ["england", "and", "wales"];
    "England", "GB-ENG", ["england"];
    "Scotland", "GB-SCT", ["scotland"];
    "Northern Ireland", "GB-NIR", ["northern", "ireland"];
    "United Kingdom", "GB", ["united", "kingdom"];
    "Ireland", "IE", ["ireland"];
    "Germany", "DE", ["germany"];
    "Germany", "DE", ["federal", "republic", "of", "germany"];
    "France", "FR", ["france"];
    "Switzerland", "CH", ["switzerland"];
    "Netherlands", "NL", ["the", "netherlands"];
    "Netherlands", "NL", ["netherlands"];
    "Belgium", "BE", ["belgium"];
    "Luxembourg", "LU", ["luxembourg"];
    "Austria", "AT", ["austria"];
    "Italy", "IT", ["italy"];
    "Spain", "ES", ["spain"];
    "Portugal", "PT", ["portugal"];
    "Sweden", "SE", ["sweden"];
    "Norway", "NO", ["norway"];
    "Denmark", "DK", ["denmark"];
    "Finland", "FI", ["finland"];
    "Poland", "PL", ["poland"];
    "Czech Republic", "CZ", ["czech", "republic"];
    "Hungary", "HU", ["hungary"];
    "Serbia", "RS", ["serbia"];
    "Serbia", "RS", ["republic", "of", "serbia"];
    "Croatia", "HR", ["croatia"];
    "Slovenia", "SI", ["slovenia"];
    "Greece", "GR", ["greece"];
    "Cyprus", "CY", ["cyprus"];
    "Israel", "IL", ["israel"];
    "India", "IN", ["india"];
    "Singapore", "SG", ["singapore"];
    "Hong Kong", "HK", ["hong", "kong"];
    "China", "CN", ["people", "'", "s", "republic", "of", "china"];
    "China", "CN", ["china"];
    "Japan", "JP", ["japan"];
    "South Korea", "KR", ["republic", "of", "korea"];
    "South Korea", "KR", ["korea"];
    "Taiwan", "TW", ["taiwan"];
    "Mexico", "MX", ["mexico"];
    "Brazil", "BR", ["brazil"];
    "Cayman Islands", "KY", ["cayman", "islands"];
    "Bermuda", "BM", ["bermuda"];
    "British Virgin Islands", "VG", ["british", "virgin", "islands"];
    "Bahamas", "BS", ["bahamas"];
    "New Zealand", "NZ", ["new", "zealand"];
    "South Africa", "ZA", ["south", "africa"];
    "United Arab Emirates", "AE", ["united", "arab", "emirates"];
];

/// The longest place whose words start at token `i` of `key`, and one past its last token.
pub fn place_at<'k>(key: impl Fn(usize) -> &'k str, i: usize) -> Option<(&'static Place, usize)> {
    let mut best: Option<(&'static Place, usize)> = None;
    for p in PLACES {
        let n = p.words.len();
        if best.is_some_and(|(b, _)| b.words.len() >= n) {
            continue;
        }
        if p.words.iter().enumerate().all(|(k, w)| key(i + k) == *w) {
            best = Some((p, i + n));
        }
    }
    best
}

/// Is the place a country (or a union of nations) rather than a state or province?
pub fn is_country(p: &Place) -> bool {
    !p.code.contains('-') || p.code == "GB-EAW"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_match_wins() {
        let words = ["new", "south", "wales", "law"];
        let key = |i: usize| words.get(i).copied().unwrap_or("");
        let (p, end) = place_at(key, 0).unwrap();
        assert_eq!((p.name, p.code, end), ("New South Wales", "AU-NSW", 3));
        let words = ["england", "and", "wales"];
        let key = |i: usize| words.get(i).copied().unwrap_or("");
        assert_eq!(place_at(key, 0).unwrap().0.name, "England and Wales");
        let words = ["new", "yorker"];
        let key = |i: usize| words.get(i).copied().unwrap_or("");
        assert!(place_at(key, 0).is_none());
    }

    #[test]
    fn codes_are_unique_per_name() {
        for a in PLACES {
            for b in PLACES {
                assert!(
                    a.code != b.code || a.name == b.name,
                    "{} / {}",
                    a.name,
                    b.name
                );
            }
            assert!(a.words.iter().all(|w| *w == w.to_lowercase()));
        }
        assert!(is_country(&PLACES[PLACES.len() - 1]));
    }
}

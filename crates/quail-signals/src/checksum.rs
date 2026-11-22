//! Check-digit algorithms of business identifiers, on compact ASCII input.
//!
//! Every function takes the identifier without separators (`compact` form, uppercase for
//! letters) and returns `false` for anything malformed instead of panicking. They are
//! cross-checked against python-stdnum 2.2 on random and generated-valid numbers (see the
//! crate README), except [`rs_mb`], whose algorithm is not in stdnum: weights `2 7 6 5 4 3 2`,
//! `k = 11 − (Σ mod 11)`, `10` and `11` become `0` (checked against published registration
//! numbers of large Serbian companies).

/// Digits of `s` as numbers, or `None` if `s` is empty or has a non-digit.
fn digits(s: &str) -> Option<Vec<u32>> {
    if s.is_empty() {
        return None;
    }
    s.bytes()
        .map(|b| b.is_ascii_digit().then(|| u32::from(b - b'0')))
        .collect()
}

/// ISO 7064 MOD 11,10 over all digits (the last one is the check digit).
pub fn mod_11_10(s: &str) -> bool {
    let Some(d) = digits(s) else { return false };
    let mut check = 5u32;
    for n in d {
        let c = if check == 0 { 10 } else { check };
        check = ((c * 2) % 11 + n) % 10;
    }
    check == 1
}

/// ISO 7064 MOD 97-10 over an alphanumeric string (letters count as 10..35): true when the
/// remainder is 1. Used by IBAN (after rotation) and LEI.
pub fn mod_97_10(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut r: u32 = 0;
    for b in s.bytes() {
        match b {
            b'0'..=b'9' => r = (r * 10 + u32::from(b - b'0')) % 97,
            b'A'..=b'Z' => {
                let v = u32::from(b - b'A') + 10;
                r = (r * 100 + v) % 97;
            }
            _ => return false,
        }
    }
    r == 1
}

/// Luhn over all digits (the last one is the check digit).
pub fn luhn(s: &str) -> bool {
    luhn_sum(s) == Some(0)
}

fn luhn_sum(s: &str) -> Option<u32> {
    let d = digits(s)?;
    let mut sum = 0;
    for (i, n) in d.iter().rev().enumerate() {
        sum += if i % 2 == 1 {
            let x = n * 2;
            x / 10 + x % 10
        } else {
            *n
        };
    }
    Some(sum % 10)
}

/// IBAN: two-letter country, two check digits, country-specific length, MOD 97-10.
/// Input is compact and uppercase (`RS35260005601001611379`).
pub fn iban(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 15 || b.len() > 34 {
        return false;
    }
    if !b[..2].iter().all(u8::is_ascii_uppercase) || !b[2..4].iter().all(u8::is_ascii_digit) {
        return false;
    }
    if !b
        .iter()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return false;
    }
    if iban_length(&s[..2]) != Some(b.len()) {
        return false;
    }
    let mut rotated = String::with_capacity(b.len());
    rotated.push_str(&s[4..]);
    rotated.push_str(&s[..4]);
    mod_97_10(&rotated)
}

/// IBAN lengths of the SWIFT IBAN registry (release 101), every listed country.
const IBAN_LENGTHS: &[(&str, u8)] = &[
    ("AD", 24),
    ("AE", 23),
    ("AL", 28),
    ("AT", 20),
    ("AZ", 28),
    ("BA", 20),
    ("BE", 16),
    ("BG", 22),
    ("BH", 22),
    ("BI", 27),
    ("BR", 29),
    ("BY", 28),
    ("CH", 21),
    ("CR", 22),
    ("CY", 28),
    ("CZ", 24),
    ("DE", 22),
    ("DJ", 27),
    ("DK", 18),
    ("DO", 28),
    ("EE", 20),
    ("EG", 29),
    ("ES", 24),
    ("FI", 18),
    ("FK", 18),
    ("FO", 18),
    ("FR", 27),
    ("GB", 22),
    ("GE", 22),
    ("GI", 23),
    ("GL", 18),
    ("GR", 27),
    ("GT", 28),
    ("HN", 28),
    ("HR", 21),
    ("HU", 28),
    ("IE", 22),
    ("IL", 23),
    ("IQ", 23),
    ("IS", 26),
    ("IT", 27),
    ("JO", 30),
    ("KW", 30),
    ("KZ", 20),
    ("LB", 28),
    ("LC", 32),
    ("LI", 21),
    ("LT", 20),
    ("LU", 20),
    ("LV", 21),
    ("LY", 25),
    ("MC", 27),
    ("MD", 24),
    ("ME", 22),
    ("MK", 19),
    ("MN", 20),
    ("MR", 27),
    ("MT", 31),
    ("MU", 30),
    ("NI", 28),
    ("NL", 18),
    ("NO", 15),
    ("OM", 23),
    ("PK", 24),
    ("PL", 28),
    ("PS", 29),
    ("PT", 25),
    ("QA", 29),
    ("RO", 24),
    ("RS", 22),
    ("RU", 33),
    ("SA", 24),
    ("SC", 31),
    ("SD", 18),
    ("SE", 24),
    ("SI", 19),
    ("SK", 24),
    ("SM", 27),
    ("SO", 23),
    ("ST", 25),
    ("SV", 28),
    ("TL", 23),
    ("TN", 24),
    ("TR", 26),
    ("UA", 29),
    ("VA", 22),
    ("VG", 24),
    ("XK", 20),
    ("YE", 30),
];

/// Length of an IBAN of `country` (two uppercase letters), if the registry lists it.
pub fn iban_length(country: &str) -> Option<usize> {
    IBAN_LENGTHS
        .binary_search_by(|(c, _)| (*c).cmp(country))
        .ok()
        .map(|i| usize::from(IBAN_LENGTHS[i].1))
}

/// LEI (ISO 17442): 18 alphanumerics and two check digits, MOD 97-10.
pub fn lei(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b.iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && b[18..].iter().all(u8::is_ascii_digit)
        && mod_97_10(s)
}

/// Serbian PIB (tax number): 9 digits, ISO 7064 MOD 11,10.
pub fn rs_pib(s: &str) -> bool {
    s.len() == 9 && mod_11_10(s)
}

/// Serbian matični broj of a legal entity or entrepreneur: 8 digits, MOD 11 with weights
/// `2 7 6 5 4 3 2`, a result of 10 or 11 written as 0.
pub fn rs_mb(s: &str) -> bool {
    const W: [u32; 7] = [2, 7, 6, 5, 4, 3, 2];
    let Some(d) = digits(s) else { return false };
    if d.len() != 8 {
        return false;
    }
    let sum: u32 = d.iter().zip(W).map(|(a, w)| a * w).sum();
    let k = 11 - sum % 11;
    let k = if k > 9 { 0 } else { k };
    d[7] == k
}

/// Croatian OIB: 11 digits, ISO 7064 MOD 11,10.
pub fn hr_oib(s: &str) -> bool {
    s.len() == 11 && mod_11_10(s)
}

/// Serbian / ex-Yugoslav JMBG: 13 digits `DDMMYYYRRBBBK`, a real birth date and MOD 11 with
/// weights `7 6 5 4 3 2` twice (10 and 11 become 0).
pub fn jmbg(s: &str) -> bool {
    const W: [u32; 12] = [7, 6, 5, 4, 3, 2, 7, 6, 5, 4, 3, 2];
    let Some(d) = digits(s) else { return false };
    if d.len() != 13 {
        return false;
    }
    let day = d[0] * 10 + d[1];
    let month = d[2] * 10 + d[3];
    let yyy = d[4] * 100 + d[5] * 10 + d[6];
    let year = if yyy >= 800 { 1000 + yyy } else { 2000 + yyy };
    if year > 2100 || !valid_ymd(i32::try_from(year).unwrap_or(0), month, day) {
        return false;
    }
    let sum: u32 = d.iter().zip(W).map(|(a, w)| a * w).sum();
    let k = 11 - sum % 11;
    let k = if k > 9 { 0 } else { k };
    d[12] == k
}

/// Serbian bank account `BBB-NNNNNNNNNNNNN-KK` as 18 digits (the middle part zero-padded to
/// 13): the whole number MOD 97 is 1 (ISO 7064 MOD 97-10).
pub fn rs_account(s: &str) -> bool {
    s.len() == 18 && s.bytes().all(|b| b.is_ascii_digit()) && mod_97_10(s)
}

/// Whether `y-m-d` is a real calendar date (Gregorian).
pub fn valid_ymd(y: i32, m: u32, d: u32) -> bool {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max).contains(&d)
}

/// Outcome of checking a VAT number body against its country's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VatCheck {
    /// Format and check digits are right.
    Valid,
    /// The format is right and the country has no public check-digit algorithm here.
    FormatOnly,
    /// The format is right but the check digits are wrong.
    BadChecksum,
    /// Not a VAT number of that country.
    Malformed,
}

/// Checks an EU (and GB/XI, CH-less) VAT number: `country` is the two-letter prefix as
/// written on VAT numbers (`EL` for Greece), `body` the rest, compact and uppercase.
///
/// Check digits are verified for AT, BE, DE, ES, FR, GB/XI, HR, IT, NL, PL, SI (the
/// algorithms python-stdnum documents); the other member states are checked for format
/// only and give [`VatCheck::FormatOnly`].
pub fn eu_vat(country: &str, body: &str) -> VatCheck {
    let b = body.as_bytes();
    let all_digits = !b.is_empty() && b.iter().all(u8::is_ascii_digit);
    let judge = |ok: bool| {
        if ok {
            VatCheck::Valid
        } else {
            VatCheck::BadChecksum
        }
    };
    match country {
        "AT" => {
            if b.len() == 9 && b[0] == b'U' && b[1..].iter().all(u8::is_ascii_digit) {
                // Luhn over digits 2..8 then (6 - sum) % 10.
                let Some(sum) = luhn_sum(&body[1..8]) else {
                    return VatCheck::Malformed;
                };
                let check = (16 - sum) % 10;
                judge(u32::from(b[8] - b'0') == check)
            } else {
                VatCheck::Malformed
            }
        }
        "BE" => {
            let n: String = if b.len() == 9 {
                format!("0{body}")
            } else {
                body.to_owned()
            };
            if n.len() != 10 || !n.bytes().all(|c| c.is_ascii_digit()) || !n.starts_with(['0', '1'])
            {
                return VatCheck::Malformed;
            }
            let head: u64 = n[..8].parse().unwrap_or(0);
            let tail: u64 = n[8..].parse().unwrap_or(0);
            // The check is 97 - head % 97 (01..=97). python-stdnum only tests
            // (head + tail) % 97 == 0, which also passes 00, 98 and 99: never issued.
            judge(head > 0 && tail == 97 - head % 97)
        }
        "DE" => {
            if b.len() == 9 && all_digits && b[0] != b'0' {
                judge(mod_11_10(body))
            } else {
                VatCheck::Malformed
            }
        }
        "ES" => es_nif(body),
        "FR" => fr_tva(body),
        "GB" | "XI" => gb_vat(body),
        "HR" => {
            if b.len() == 11 && all_digits {
                judge(mod_11_10(body))
            } else {
                VatCheck::Malformed
            }
        }
        "IT" => {
            if b.len() != 11 || !all_digits {
                return VatCheck::Malformed;
            }
            let office = &body[7..10];
            let office_ok = ("001"..="100").contains(&office)
                || matches!(office, "120" | "121" | "888" | "999");
            if body[..7] == *"0000000" || !office_ok {
                return VatCheck::Malformed;
            }
            judge(luhn(body))
        }
        "NL" => nl_btw(body),
        "PL" => {
            const W: [i32; 10] = [6, 5, 7, 2, 3, 4, 5, 6, 7, -1];
            if b.len() != 10 || !all_digits {
                return VatCheck::Malformed;
            }
            let sum: i32 = b.iter().zip(W).map(|(c, w)| i32::from(c - b'0') * w).sum();
            judge(sum.rem_euclid(11) == 0)
        }
        "SI" => {
            if b.len() != 8 || !all_digits || b[0] == b'0' {
                return VatCheck::Malformed;
            }
            let sum: u32 = b[..7]
                .iter()
                .enumerate()
                .map(|(i, c)| (8 - u32::try_from(i).unwrap_or(0)) * u32::from(c - b'0'))
                .sum();
            let check = 11 - sum % 11;
            let ok = match check {
                10 => b[7] == b'0',
                11 => false,
                c => u32::from(b[7] - b'0') == c,
            };
            judge(ok)
        }
        _ => format_only(country, body),
    }
}

/// Format rules of the member states without a check-digit implementation here.
fn format_only(country: &str, body: &str) -> VatCheck {
    let b = body.as_bytes();
    let n = b.len();
    let digits = b.iter().all(u8::is_ascii_digit);
    let alnum = b
        .iter()
        .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
    let ok = match country {
        "BG" => digits && (n == 9 || n == 10),
        "CY" => n == 9 && b[..8].iter().all(u8::is_ascii_digit) && b[8].is_ascii_uppercase(),
        "CZ" => digits && (8..=10).contains(&n),
        "DK" | "FI" | "HU" | "LU" | "MT" => digits && n == 8,
        "EE" | "EL" | "PT" => digits && n == 9,
        "IE" => alnum && (n == 8 || n == 9) && b[..1].iter().all(u8::is_ascii_digit),
        "LT" => digits && (n == 9 || n == 12),
        "LV" => digits && n == 11,
        "RO" => digits && (2..=10).contains(&n),
        "SE" => digits && n == 12,
        "SK" => digits && n == 10,
        _ => return VatCheck::Malformed,
    };
    if ok {
        VatCheck::FormatOnly
    } else {
        VatCheck::Malformed
    }
}

fn es_nif(body: &str) -> VatCheck {
    const DNI: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";
    let b = body.as_bytes();
    if b.len() != 9 || !b[1..8].iter().all(u8::is_ascii_digit) {
        return VatCheck::Malformed;
    }
    let dni_letter = |num: &str| -> Option<u8> {
        let n: usize = num.parse().ok()?;
        Some(DNI[n % 23])
    };
    let first = b[0];
    let last = b[8];
    let ok = if matches!(first, b'K' | b'L' | b'M') {
        dni_letter(&body[1..8]) == Some(last)
    } else if first.is_ascii_digit() {
        dni_letter(&body[..8]) == Some(last)
    } else if let Some(pos) = b"XYZ".iter().position(|&c| c == first) {
        dni_letter(&format!("{pos}{}", &body[1..8])) == Some(last)
    } else if b"ABCDEFGHJNPQRSUVW".contains(&first) {
        // CIF: Luhn check digit over the 7 digits, written as a digit or a letter.
        let Some(sum) = luhn_sum(&format!("{}0", &body[1..8])) else {
            return VatCheck::Malformed;
        };
        let check = (10 - sum) % 10;
        let letter = b"JABCDEFGHI"[check as usize];
        last == b'0' + u8::try_from(check).unwrap_or(0) || last == letter
    } else {
        return VatCheck::Malformed;
    };
    if ok {
        VatCheck::Valid
    } else {
        VatCheck::BadChecksum
    }
}

fn fr_tva(body: &str) -> VatCheck {
    const ALPHA: &[u8] = b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let b = body.as_bytes();
    if b.len() != 11 || !b[2..].iter().all(u8::is_ascii_digit) {
        return VatCheck::Malformed;
    }
    let Some(i0) = ALPHA.iter().position(|&c| c == b[0]) else {
        return VatCheck::Malformed;
    };
    let Some(i1) = ALPHA.iter().position(|&c| c == b[1]) else {
        return VatCheck::Malformed;
    };
    let siren = &body[2..];
    if &siren[..3] != "000" && !luhn(siren) {
        return VatCheck::BadChecksum;
    }
    let Ok(num) = siren.parse::<u64>() else {
        return VatCheck::Malformed;
    };
    let ok = if b[0].is_ascii_digit() && b[1].is_ascii_digit() {
        let key = u64::from((b[0] - b'0') * 10 + (b[1] - b'0'));
        key == (num * 100 + 12) % 97
    } else {
        let (i0, i1) = (
            i64::try_from(i0).unwrap_or(0),
            i64::try_from(i1).unwrap_or(0),
        );
        let check = if b[0].is_ascii_digit() {
            i0 * 24 + i1 - 10
        } else {
            i0 * 34 + i1 - 100
        };
        let n = i64::try_from(num).unwrap_or(0);
        (n + 1 + check.div_euclid(11)).rem_euclid(11) == check.rem_euclid(11)
    };
    if ok {
        VatCheck::Valid
    } else {
        VatCheck::BadChecksum
    }
}

fn gb_vat(body: &str) -> VatCheck {
    const W: [u32; 9] = [8, 7, 6, 5, 4, 3, 2, 10, 1];
    let b = body.as_bytes();
    let n = b.len();
    if n == 5 {
        let digits_ok = b[2..].iter().all(u8::is_ascii_digit);
        let num: u32 = body.get(2..).and_then(|s| s.parse().ok()).unwrap_or(0);
        let ok = digits_ok
            && ((body.starts_with("GD") && num < 500) || (body.starts_with("HA") && num >= 500));
        return if ok {
            VatCheck::FormatOnly
        } else {
            VatCheck::Malformed
        };
    }
    if (n != 9 && n != 12) || !b.iter().all(u8::is_ascii_digit) {
        return VatCheck::Malformed;
    }
    let sum: u32 = b[..9]
        .iter()
        .zip(W)
        .map(|(c, w)| u32::from(c - b'0') * w)
        .sum();
    let r = sum % 97;
    let head: u32 = body[..3].parse().unwrap_or(0);
    let ok = if head >= 100 {
        matches!(r, 0 | 42 | 55)
    } else {
        r == 0
    };
    if ok {
        VatCheck::Valid
    } else {
        VatCheck::BadChecksum
    }
}

fn nl_btw(body: &str) -> VatCheck {
    let b = body.as_bytes();
    if b.len() != 12
        || b[9] != b'B'
        || !b[..9].iter().all(u8::is_ascii_digit)
        || !b[10..].iter().all(u8::is_ascii_digit)
        || body[..9] == *"000000000"
        || body[10..] == *"00"
    {
        return VatCheck::Malformed;
    }
    // Old style: the first nine digits are a BSN (11-proof); new style: MOD 97-10 over
    // "NL" + the whole number.
    let bsn = {
        let sum: i32 = b[..8]
            .iter()
            .enumerate()
            .map(|(i, c)| (9 - i32::try_from(i).unwrap_or(0)) * i32::from(c - b'0'))
            .sum::<i32>()
            - i32::from(b[8] - b'0');
        sum.rem_euclid(11) == 0
    };
    if bsn || mod_97_10_prefixed("NL", body) {
        VatCheck::Valid
    } else {
        VatCheck::BadChecksum
    }
}

/// MOD 97-10 of `prefix` followed by `body` (letters as 10..35), true when the remainder is 1.
fn mod_97_10_prefixed(prefix: &str, body: &str) -> bool {
    let mut s = String::with_capacity(prefix.len() + body.len());
    s.push_str(prefix);
    s.push_str(body);
    mod_97_10(&s)
}

/// US EIN prefixes (the first two digits) that the IRS assigns.
pub fn us_ein_prefix(p: &str) -> bool {
    const VALID: &[&str] = &[
        "01", "02", "03", "04", "05", "06", "10", "11", "12", "13", "14", "15", "16", "20", "21",
        "22", "23", "24", "25", "26", "27", "30", "31", "32", "33", "34", "35", "36", "37", "38",
        "39", "40", "41", "42", "43", "44", "45", "46", "47", "48", "50", "51", "52", "53", "54",
        "55", "56", "57", "58", "59", "60", "61", "62", "63", "64", "65", "66", "67", "68", "71",
        "72", "73", "74", "75", "76", "77", "80", "81", "82", "83", "84", "85", "86", "87", "88",
        "90", "91", "92", "93", "94", "95", "98", "99",
    ];
    VALID.contains(&p)
}

/// ISO 3166-1 alpha-2 country codes (current), for BIC country checks.
pub fn iso_country(c: &str) -> bool {
    const C: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ \
        BL BM BN BO BQ BR BS BT BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX \
        CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI \
        GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM \
        JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF \
        MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU \
        NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RU RW SA SB SC SD SE SG SH \
        SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW \
        TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS XK YE YT ZA ZM ZW";
    c.len() == 2 && C.split_ascii_whitespace().any(|x| x == c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Appends the MOD 11,10 check digit.
    fn with_11_10(base: &str) -> String {
        for d in 0..10 {
            let s = format!("{base}{d}");
            if mod_11_10(&s) {
                return s;
            }
        }
        unreachable!("one digit always fits")
    }

    #[test]
    fn mod_11_10_has_exactly_one_check_digit() {
        for base in ["10000000", "12345678", "99999999", "0000000000"] {
            let n = (0..10).filter(|d| mod_11_10(&format!("{base}{d}"))).count();
            assert_eq!(n, 1, "{base}");
        }
        assert!(rs_pib(&with_11_10("10123456")));
        assert!(hr_oib(&with_11_10("1234567890")));
        assert!(!rs_pib("12345678"));
        assert!(!rs_pib("1234567x9"));
    }

    #[test]
    fn iban_lengths_sorted_and_checked() {
        assert!(IBAN_LENGTHS.windows(2).all(|w| w[0].0 < w[1].0));
        // Check digits computed in code: 98 - mod97(bban + "RS00").
        let bban = "260005601001611379";
        let mut r = 0u32;
        for b in format!("{bban}RS00").bytes() {
            r = if b.is_ascii_digit() {
                (r * 10 + u32::from(b - b'0')) % 97
            } else {
                (r * 100 + u32::from(b - b'A') + 10) % 97
            };
        }
        let iban_ok = format!("RS{:02}{bban}", 98 - r);
        assert!(iban(&iban_ok));
        let bad = iban_ok.replace("RS", "ME");
        assert!(!iban(&bad), "ME has the same length but other check digits");
        assert!(!iban(&iban_ok[..21]));
        assert!(!iban("rs35260005601001611379"));
    }

    #[test]
    fn mb_and_jmbg_and_account() {
        // Built so that the weighted sum fits: 1000000 -> 2*1 = 2, k = 9.
        assert!(rs_mb("10000009"));
        assert!(!rs_mb("10000008"));
        assert!(!rs_mb("1000000"));
        // JMBG: 01.01.1990, region 71, serial 000 -> compute k.
        let base = "010199071000";
        let w = [7, 6, 5, 4, 3, 2, 7, 6, 5, 4, 3, 2];
        let s: u32 = base
            .bytes()
            .zip(w)
            .map(|(b, w)| u32::from(b - b'0') * w)
            .sum();
        let k = match 11 - s % 11 {
            k if k > 9 => 0,
            k => k,
        };
        assert!(jmbg(&format!("{base}{k}")));
        assert!(!jmbg(&format!("{base}{}", (k + 1) % 10)));
        assert!(!jmbg("3102990710005"), "31 February");
        // Account 160-0000000123456-KK.
        let head = "1600000000123456";
        let mut r = 0u64;
        for b in format!("{head}00").bytes() {
            r = (r * 10 + u64::from(b - b'0')) % 97;
        }
        assert!(rs_account(&format!("{head}{:02}", 98 - r)));
        assert!(!rs_account(&format!("{head}{:02}", (99 - r) % 100)));
    }

    #[test]
    fn vat_examples() {
        // Numbers built from the documented algorithms.
        assert_eq!(eu_vat("DE", &with_11_10("13645678")), VatCheck::Valid);
        assert_eq!(eu_vat("DE", &with_11_10("03645678")), VatCheck::Malformed);
        assert_eq!(eu_vat("HR", &with_11_10("3364567890")), VatCheck::Valid);
        // BE: 0123456 7xx with (head + tail) % 97 == 0.
        let head = 1_234_567_u64;
        let tail = 97 - head % 97;
        assert_eq!(eu_vat("BE", &format!("0{head}{tail:02}")), VatCheck::Valid);
        assert_eq!(
            eu_vat("BE", &format!("0{head}{:02}", (tail + 1) % 97)),
            VatCheck::BadChecksum
        );
        // BE check digits are 97 - head % 97, so 01..=97: a head that is a multiple of 97
        // takes 97, never 00, and 98/99 never occur. (head + tail) % 97 == 0 alone accepts
        // all three. Invented numbers.
        let head = 9_700_000_u64; // 97 * 100_000
        assert_eq!(eu_vat("BE", &format!("0{head}97")), VatCheck::Valid);
        assert_eq!(eu_vat("BE", &format!("0{head}00")), VatCheck::BadChecksum);
        let head = 9_700_096_u64; // head % 97 == 96, check 01
        assert_eq!(eu_vat("BE", &format!("0{head}01")), VatCheck::Valid);
        assert_eq!(eu_vat("BE", &format!("0{head}98")), VatCheck::BadChecksum);
        let head = 9_700_095_u64; // head % 97 == 95, check 02
        assert_eq!(eu_vat("BE", &format!("0{head}02")), VatCheck::Valid);
        assert_eq!(eu_vat("BE", &format!("0{head}99")), VatCheck::BadChecksum);
        assert_eq!(eu_vat("PL", "7104219052"), VatCheck::Valid);
        assert_eq!(eu_vat("PL", "7104219053"), VatCheck::BadChecksum);
        assert_eq!(eu_vat("GB", "717296178"), VatCheck::Valid);
        assert_eq!(eu_vat("GB", "717296179"), VatCheck::BadChecksum);
        assert_eq!(eu_vat("AT", "U04571784"), VatCheck::Valid);
        assert_eq!(eu_vat("AT", "U04571785"), VatCheck::BadChecksum);
        assert_eq!(eu_vat("SI", "59278951"), VatCheck::Valid);
        assert_eq!(eu_vat("IT", "13627010500"), VatCheck::Valid);
        assert_eq!(eu_vat("NL", "245732147B01"), VatCheck::Valid);
        assert_eq!(eu_vat("FR", "40779211259"), VatCheck::Valid);
        assert_eq!(eu_vat("ES", "B05258983"), VatCheck::Valid);
        assert_eq!(eu_vat("ES", "28088642F"), VatCheck::Valid);
        assert_eq!(eu_vat("DK", "31415926"), VatCheck::FormatOnly);
        assert_eq!(eu_vat("DK", "3141592"), VatCheck::Malformed);
        assert_eq!(eu_vat("XX", "1"), VatCheck::Malformed);
    }

    #[test]
    fn hostile_inputs_do_not_panic() {
        for s in [
            "",
            "A",
            "€€€",
            "AT",
            "U",
            "ÜÜÜÜÜÜÜÜÜ",
            "B€",
            "NL€€€€€€€€€B01",
        ] {
            let _ = iban(s);
            let _ = lei(s);
            let _ = jmbg(s);
            for c in [
                "AT", "BE", "DE", "ES", "FR", "GB", "HR", "IT", "NL", "PL", "SI", "CY", "IE",
            ] {
                let _ = eu_vat(c, s);
            }
        }
    }
}

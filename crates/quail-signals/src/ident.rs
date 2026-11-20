//! Identifiers: IBAN, BIC, EU VAT, Serbian PIB / MB / bank account / JMBG, Croatian OIB,
//! LEI, DUNS, US EIN.
//!
//! Every candidate is a token (or a run of tokens) of the right shape; the check digits and
//! a label in front of it decide the confidence (see [`crate::conf`]). Bare digit runs
//! without a label are kept only where the checksum is strong (IBAN, LEI, Serbian bank
//! accounts) or, for PIB, on pages that read as Serbian business text, at low confidence.

use quail_core::Value;
use quail_text::token::TokenKind;

use crate::checksum::{self, VatCheck};
use crate::conf;
use crate::keys;
use crate::page::{PageText, span};
use crate::phrase::{Reach, find_after, find_before};
use crate::scan::Cand;

/// What a label says the value next to it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Label {
    /// Serbian tax number (`PIB`, `ПИБ`, `poreski identifikacioni broj`, `PDV`).
    Pib,
    /// Any tax / VAT number (`VAT No`, `Tax ID`, `TIN`).
    Tax,
    /// A VAT number of one country by its local name (`USt-IdNr` → DE, `NIP` → PL).
    Country(&'static str),
    /// Serbian registration number (`MB`, `Matični broj`, `Mat. br.`).
    RegRs,
    /// Any company registration number (`Company number`, `Registration No`).
    Reg,
    /// DUNS.
    Duns,
    /// LEI.
    Lei,
    /// IBAN.
    Iban,
    /// BIC / SWIFT.
    Bic,
    /// Bank account (`račun`, `tekući račun`, `account`).
    Account,
    /// Croatian OIB.
    Oib,
    /// US EIN (`EIN`, `Employer Identification No.`, `I.R.S.`).
    Ein,
}

/// Labels as folded phrases. Only the head words are needed: `VAT Registration Number`
/// is found through `vat`, two words away from the value.
const LABELS: &[(&[&str], Label)] = &[
    (&["pib"], Label::Pib),
    (&["poreski", "identifikacioni"], Label::Pib),
    (&["poreski", "broj"], Label::Pib),
    (&["pdv"], Label::Pib),
    (&["vat"], Label::Tax),
    (&["tax", "id"], Label::Tax),
    (&["tax", "identification"], Label::Tax),
    (&["tax", "number"], Label::Tax),
    (&["tax", "no"], Label::Tax),
    (&["tax", "reg"], Label::Tax),
    (&["tax", "registration"], Label::Tax),
    (&["taxpayer", "identification"], Label::Tax),
    (&["tin"], Label::Tax),
    (&["ust", "idnr"], Label::Country("DE")),
    (&["ust", "id"], Label::Country("DE")),
    (&["ustid"], Label::Country("DE")),
    (&["ustidnr"], Label::Country("DE")),
    (
        &["umsatzsteuer", "identifikationsnummer"],
        Label::Country("DE"),
    ),
    (&["uid"], Label::Country("AT")),
    (&["partita", "iva"], Label::Country("IT")),
    (&["p", "iva"], Label::Country("IT")),
    (&["nif"], Label::Country("ES")),
    (&["cif"], Label::Country("ES")),
    (&["tva"], Label::Country("FR")),
    (&["btw"], Label::Country("NL")),
    (&["nip"], Label::Country("PL")),
    (&["ddv"], Label::Country("SI")),
    (&["oib"], Label::Oib),
    (&["mb"], Label::RegRs),
    (&["maticni"], Label::RegRs),
    (&["mat", "br"], Label::RegRs),
    (&["mat", "broj"], Label::RegRs),
    (&["mbr"], Label::RegRs),
    (&["registration"], Label::Reg),
    (&["company", "number"], Label::Reg),
    (&["company", "no"], Label::Reg),
    (&["registered", "number"], Label::Reg),
    (&["registered", "no"], Label::Reg),
    (&["reg", "no"], Label::Reg),
    (&["reg", "nr"], Label::Reg),
    (&["duns"], Label::Duns),
    (&["d", "u", "n", "s"], Label::Duns),
    (&["dun", "bradstreet"], Label::Duns),
    (&["lei"], Label::Lei),
    (&["legal", "entity", "identifier"], Label::Lei),
    (&["iban"], Label::Iban),
    (&["bic"], Label::Bic),
    (&["swift"], Label::Bic),
    (&["s", "w", "i", "f", "t"], Label::Bic),
    (&["racun"], Label::Account),
    (&["racuna"], Label::Account),
    (&["tr"], Label::Account),
    (&["t", "r"], Label::Account),
    (&["account"], Label::Account),
    (&["acc"], Label::Account),
    (&["ein"], Label::Ein),
    (&["fein"], Label::Ein),
    (&["employer", "identification"], Label::Ein),
    (&["i.r.s"], Label::Ein),
    (&["irs"], Label::Ein),
];

/// Prefixes of VAT numbers (EU member states, `EL` for Greece, GB and XI).
const VAT_PREFIXES: &[&str] = &[
    "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "EL", "ES", "FI", "FR", "GB", "HR", "HU", "IE",
    "IT", "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK", "XI",
];

const REACH: Reach = Reach { words: 4, lines: 1 };

/// State kept across one page's identifier scan.
pub(crate) struct IdentScan<'a> {
    p: &'a PageText,
    /// The page reads as Serbian business text (gates unlabeled PIBs).
    serbian: bool,
    /// Token index of the last IBAN, for BICs printed next to it.
    last_iban: Option<usize>,
    /// Valid JMBGs seen (only counted).
    pub jmbg: u32,
}

impl<'a> IdentScan<'a> {
    pub fn new(p: &'a PageText, serbian: bool) -> Self {
        Self {
            p,
            serbian,
            last_iban: None,
            jmbg: 0,
        }
    }

    /// Scans every token; pushes candidates to `out`.
    pub fn run(&mut self, out: &mut Vec<Cand>) {
        let mut i = 0;
        while i < self.p.len() {
            let next = match self.p.kind(i) {
                Some(TokenKind::Alnum | TokenKind::Word) => self.alnum(i, out),
                Some(TokenKind::Number) => {
                    self.number(i, out);
                    None
                }
                _ => None,
            };
            i = next.map_or(i + 1, |n| n.max(i + 1));
        }
    }

    fn label(&self, i: usize) -> Option<Label> {
        find_before(self.p, i, LABELS, REACH).map(|(l, _)| l)
    }

    /// Up to three labels in a row before token `i` (`PIB/MB:`), nearest first.
    fn labels(&self, i: usize) -> Vec<Label> {
        let mut out = Vec::new();
        let mut at = i;
        let near = Reach { words: 1, lines: 0 };
        let mut reach = REACH;
        while out.len() < 3 {
            let Some((l, first)) = find_before(self.p, at, LABELS, reach) else {
                break;
            };
            out.push(l);
            at = first;
            reach = near;
        }
        out
    }

    /// Alphanumeric token: IBAN, LEI, glued VAT, BIC, or a VAT prefix followed by numbers.
    /// Returns the next token to look at when it consumed several.
    #[allow(clippy::too_many_lines)] // one flat decision list, easier to audit in one place
    fn alnum(&mut self, i: usize, out: &mut Vec<Cand>) -> Option<usize> {
        let p = self.p;
        let text = p.text(i);
        // Codes are uppercase letters and digits (spaces only inside a spaced IBAN); checked
        // before allocating, since most capitalised words fail here.
        if !text
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ')
        {
            return None;
        }
        let compact: String = text.chars().filter(|c| *c != ' ').collect();
        if compact.is_empty() {
            return None;
        }
        let b = compact.as_bytes();
        let sp = p.span(i, i);
        // IBAN.
        if b.len() >= 15
            && b[..2].iter().all(u8::is_ascii_uppercase)
            && b[2..4].iter().all(u8::is_ascii_digit)
            && checksum::iban_length(&compact[..2]) == Some(b.len())
        {
            let label = self.label(i);
            let conf = match (checksum::iban(&compact), label == Some(Label::Iban)) {
                (true, true) => conf::IBAN_LABELED,
                (true, false) => conf::IBAN,
                (false, true) => conf::INVALID_LABELED,
                (false, false) => return None,
            };
            self.last_iban = Some(i);
            out.push(Cand::new(keys::IBAN, Value::text(compact), conf, sp));
            return None;
        }
        // LEI.
        if b.len() == 20 && b.iter().any(u8::is_ascii_digit) && b.iter().any(u8::is_ascii_uppercase)
        {
            let label = self.label(i) == Some(Label::Lei);
            let conf = match (checksum::lei(&compact), label) {
                (true, true) => conf::LEI_LABELED,
                (true, false) => conf::LEI,
                (false, true) => conf::INVALID_LABELED,
                (false, false) => return None,
            };
            out.push(Cand::new(keys::LEI, Value::text(compact), conf, sp));
            return None;
        }
        // VAT number with its prefix glued on: ATU12345678, DE123456789, NL...B01.
        if b.len() > 2 && b.len() <= 14 && b[..2].iter().all(u8::is_ascii_uppercase) {
            let (cc, body) = compact.split_at(2);
            if body.bytes().any(|c| c.is_ascii_digit()) {
                if cc == "RS" && body.len() == 9 && body.bytes().all(|c| c.is_ascii_digit()) {
                    let conf = if checksum::rs_pib(body) {
                        conf::VAT_GLUED_VALID
                    } else if self.label(i).is_some() {
                        conf::INVALID_LABELED
                    } else {
                        return None;
                    };
                    out.push(Cand::new(keys::RS_PIB, Value::text(body), conf, sp));
                    return None;
                }
                if VAT_PREFIXES.contains(&cc) {
                    let label = self.label(i);
                    Self::vat(cc, body, label, true, sp, out);
                    return None;
                }
            }
        }
        // BIC: needs a label or an IBAN just before.
        if (b.len() == 8 || b.len() == 11)
            && b[..6].iter().all(u8::is_ascii_uppercase)
            && checksum::iso_country(&compact[4..6])
        {
            let labeled = self.label(i) == Some(Label::Bic);
            let near_iban = self
                .last_iban
                .is_some_and(|j| i - j <= 12 && p.line(i) - p.line(j) <= 1);
            if labeled || near_iban {
                let conf = if labeled {
                    conf::BIC_LABELED
                } else {
                    conf::BIC_NEAR_IBAN
                };
                out.push(Cand::new(keys::BIC, Value::text(compact), conf, sp));
            }
            return None;
        }
        // A VAT prefix on its own, followed by the number: `DE 123456789`, `GB 123 4567 89`,
        // `BE 0123.456.789`, `ATU 12345678`.
        let cc = text;
        let is_prefix = VAT_PREFIXES.contains(&cc) || cc == "RS";
        let at_u = cc == "ATU";
        if !(is_prefix || at_u) {
            return None;
        }
        // Collect the run of numbers after the prefix; try the longest body first, then
        // shorter ones (a trailing number may belong to the next field).
        let mut body = String::from(if at_u { "U" } else { "" });
        let mut ends: Vec<(String, usize)> = Vec::new();
        let mut j = i + 1;
        'run: while j < p.len() && j <= i + 8 && p.tight(j) {
            let t = p.text(j);
            match p.kind(j) {
                Some(TokenKind::Number | TokenKind::Alnum) => {
                    for c in t.chars() {
                        if c.is_ascii_alphanumeric() {
                            body.push(c.to_ascii_uppercase());
                        } else if !matches!(c, '.' | '-' | ' ') {
                            break 'run;
                        }
                    }
                    ends.push((body.clone(), j));
                }
                Some(TokenKind::Punct) if matches!(t, "." | "-") && p.touch(j) => {}
                _ => break,
            }
            if body.len() > 14 {
                break;
            }
            j += 1;
        }
        if ends.is_empty() {
            return None;
        }
        let label = self.label(i);
        let cc = if at_u { "AT" } else { cc };
        for (body, j) in ends.iter().rev() {
            let sp = p.span(i, *j);
            if cc == "RS" {
                if body.len() == 9 && body.bytes().all(|c| c.is_ascii_digit()) {
                    let conf = if checksum::rs_pib(body) {
                        conf::VAT_SPACED_VALID
                    } else if label.is_some() {
                        conf::INVALID_LABELED
                    } else {
                        continue;
                    };
                    out.push(Cand::new(
                        keys::RS_PIB,
                        Value::text(body.as_str()),
                        conf,
                        sp,
                    ));
                    return Some(j + 1);
                }
                continue;
            }
            if Self::vat(cc, body, label, false, sp, out) {
                return Some(j + 1);
            }
        }
        None
    }

    /// Emits a VAT candidate for `cc` + `body` if its rules and the label allow. Returns
    /// whether it emitted.
    fn vat(
        cc: &str,
        body: &str,
        label: Option<Label>,
        glued: bool,
        sp: quail_core::Span,
        out: &mut Vec<Cand>,
    ) -> bool {
        let labeled = matches!(
            label,
            Some(Label::Tax | Label::Pib | Label::Oib | Label::Country(_))
        );
        let conf = match (checksum::eu_vat(cc, body), glued, labeled) {
            (VatCheck::Valid, _, true) => conf::VAT_LABELED_VALID,
            (VatCheck::Valid, true, false) => conf::VAT_GLUED_VALID,
            (VatCheck::Valid, false, false) => conf::VAT_SPACED_VALID,
            (VatCheck::FormatOnly, _, true) => conf::VAT_FORMAT_LABELED,
            (VatCheck::FormatOnly, true, false) => conf::VAT_FORMAT_GLUED,
            (VatCheck::BadChecksum, _, true) => conf::INVALID_LABELED,
            _ => return false,
        };
        let value = format!("{cc}{body}");
        out.push(Cand::new(keys::VAT, Value::text(value), conf, sp));
        if cc == "HR" && checksum::hr_oib(body) {
            out.push(Cand::new(keys::HR_OIB, Value::text(body), conf, sp));
        }
        true
    }

    /// Number token: PIB, MB, OIB, JMBG, bank account, EIN, DUNS, unprefixed VAT.
    fn number(&mut self, i: usize, out: &mut Vec<Cand>) {
        let p = self.p;
        let text = p.text(i);
        let start = p.toks[i].start;
        // Identifiers have 8 or more digits: shorter numbers (amounts, dates, counts, the bulk
        // of number tokens) are skipped before any work.
        if text.len() < 8
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'/'))
        {
            return;
        }
        let parts: Vec<(usize, &str)> = split_parts(text);
        let seps: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_digit()).collect();
        let digits: String = text
            .bytes()
            .filter(u8::is_ascii_digit)
            .map(char::from)
            .collect();
        let lens: Vec<usize> = parts.iter().map(|(_, s)| s.len()).collect();
        let label = || self.label(i);
        let sp = p.span(i, i);
        match (lens.as_slice(), seps.as_slice()) {
            ([n], []) => {
                // Lengths that never need a label skip the label search.
                let label = if *n == 13 { None } else { self.label(i) };
                self.plain(label, &digits, sp, out);
            }
            // PIB/MB pairs and lists: `101234567/07005466`.
            (_, s)
                if s.iter().all(|&c| c == b'/')
                    && lens.iter().all(|l| matches!(l, 8 | 9 | 11 | 13)) =>
            {
                let labels = self.labels(i);
                for (off, part) in &parts {
                    let s0 = start + off;
                    let label = list_label(&labels, part.len());
                    self.plain(label, part, span(s0, s0 + part.len()), out);
                }
            }
            // Serbian bank account: 3-x-2, middle zero-padded to 13.
            ([3, m, 2], [b'-', b'-'])
                if (5..=13).contains(m) && parts[1].1.bytes().any(|b| b != b'0') =>
            {
                let (bank, mid, ctl) = (parts[0].1, parts[1].1, parts[2].1);
                let full = format!("{bank}{mid:0>13}{ctl}");
                let labeled = label() == Some(Label::Account);
                let conf = match (checksum::rs_account(&full), labeled) {
                    (true, true) => conf::ACCOUNT_LABELED,
                    (true, false) => conf::ACCOUNT_DASHED,
                    (false, true) => conf::INVALID_LABELED,
                    (false, false) => return,
                };
                let value = format!("{bank}-{mid:0>13}-{ctl}");
                out.push(Cand::new(keys::RS_ACCOUNT, Value::text(value), conf, sp));
            }
            // US EIN 12-3456789: label before or after (cover pages print it below).
            ([2, 7], [b'-']) => {
                let before = label();
                let after = find_after(p, i, LABELS, REACH).map(|(l, _)| l);
                let ein = |l: Option<Label>| matches!(l, Some(Label::Ein | Label::Tax));
                if (ein(before) || ein(after)) && checksum::us_ein_prefix(&digits[..2]) {
                    let value = format!("{}-{}", &digits[..2], &digits[2..]);
                    out.push(Cand::new(keys::US_EIN, Value::text(value), conf::EIN, sp));
                }
            }
            // DUNS 12-345-6789.
            ([2, 3, 4], [b'-', b'-']) => {
                if label() == Some(Label::Duns) {
                    out.push(Cand::new(keys::DUNS, Value::text(digits), conf::DUNS, sp));
                }
            }
            // A country-labelled VAT written with separators (`NIP 123-456-78-90`,
            // `BTW 0123.456.789`).
            _ => {
                if let Some(Label::Country(cc)) = label()
                    && checksum::eu_vat(cc, &digits) != VatCheck::Malformed
                {
                    Self::vat(cc, &digits, Some(Label::Country(cc)), false, sp, out);
                }
            }
        }
    }

    /// A run of digits without separators (or one part of a `/` list).
    fn plain(&mut self, label: Option<Label>, d: &str, sp: quail_core::Span, out: &mut Vec<Cand>) {
        let push = |out: &mut Vec<Cand>, key, conf| {
            out.push(Cand::new(key, Value::text(d), conf, sp));
        };
        match d.len() {
            8 => match label {
                Some(Label::RegRs | Label::Reg) if self.serbian || label == Some(Label::RegRs) => {
                    if checksum::rs_mb(d) {
                        push(out, keys::RS_MB, conf::MB_LABELED);
                    } else if label == Some(Label::RegRs) {
                        push(out, keys::RS_MB, conf::INVALID_LABELED);
                    }
                }
                Some(Label::Country(cc)) => Self::country(cc, d, sp, out),
                _ => {}
            },
            9 => match label {
                Some(Label::Pib) => {
                    let c = if checksum::rs_pib(d) {
                        conf::PIB_LABELED
                    } else {
                        conf::INVALID_LABELED
                    };
                    push(out, keys::RS_PIB, c);
                }
                Some(Label::Tax) => {
                    if checksum::rs_pib(d) && self.serbian {
                        push(out, keys::RS_PIB, conf::PIB_LABELED);
                    } else if checksum::eu_vat("GB", d) == VatCheck::Valid {
                        out.push(Cand::new(
                            keys::VAT,
                            Value::text(format!("GB{d}")),
                            conf::VAT_GB_UNPREFIXED,
                            sp,
                        ));
                    }
                }
                Some(Label::Duns) => push(out, keys::DUNS, conf::DUNS),
                Some(Label::Country(cc)) => Self::country(cc, d, sp, out),
                None if self.serbian && d.starts_with('1') && checksum::rs_pib(d) => {
                    push(out, keys::RS_PIB, conf::PIB_UNLABELED);
                }
                _ => {}
            },
            11 => match label {
                Some(Label::Oib) => {
                    let c = if checksum::hr_oib(d) {
                        conf::OIB_LABELED
                    } else {
                        conf::INVALID_LABELED
                    };
                    push(out, keys::HR_OIB, c);
                }
                Some(Label::Country(cc)) => Self::country(cc, d, sp, out),
                _ => {}
            },
            13 => {
                if checksum::jmbg(d) {
                    self.jmbg += 1;
                }
            }
            18 => {
                let labeled = label == Some(Label::Account);
                let c = match (checksum::rs_account(d), labeled) {
                    (true, true) => conf::ACCOUNT_LABELED,
                    (true, false) => conf::ACCOUNT_PLAIN,
                    (false, true) => conf::INVALID_LABELED,
                    (false, false) => return,
                };
                let value = format!("{}-{}-{}", &d[..3], &d[3..16], &d[16..]);
                out.push(Cand::new(keys::RS_ACCOUNT, Value::text(value), c, sp));
            }
            _ => {
                if let Some(Label::Country(cc)) = label {
                    Self::country(cc, d, sp, out);
                }
            }
        }
    }

    /// An unprefixed number after a country's own VAT label (`NIP: 5260250995`).
    fn country(cc: &'static str, d: &str, sp: quail_core::Span, out: &mut Vec<Cand>) {
        if checksum::eu_vat(cc, d) != VatCheck::Malformed {
            Self::vat(cc, d, Some(Label::Country(cc)), false, sp, out);
        }
    }
}

/// The label of one part of a `/` list (`PIB/MB: 101234567/07005466`): the part's length
/// picks the label it fits, else the nearest one.
fn list_label(labels: &[Label], len: usize) -> Option<Label> {
    let fits = |l: &Label| match len {
        8 => matches!(l, Label::RegRs | Label::Reg),
        9 => matches!(l, Label::Pib | Label::Tax | Label::Duns),
        11 => matches!(l, Label::Oib | Label::Country(_)),
        _ => false,
    };
    labels
        .iter()
        .copied()
        .find(fits)
        .or_else(|| labels.first().copied())
}

/// Digit runs of a number token with their byte offsets.
fn split_parts(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut s = None;
    for (k, b) in text.bytes().enumerate() {
        if b.is_ascii_digit() {
            s.get_or_insert(k);
        } else if let Some(a) = s.take() {
            out.push((a, &text[a..k]));
        }
    }
    if let Some(a) = s {
        out.push((a, &text[a..]));
    }
    out
}

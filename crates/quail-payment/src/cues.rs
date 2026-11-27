//! Cue phrases, in folded form (lowercase, no diacritics, Cyrillic → Latin): a trailing `*`
//! matches any ending (`iznos*` covers `iznos`, `iznosi`, `iznosu`). Punctuation
//! between the words of a phrase is skipped (`PDV-a` reads as `pdv a`).

/// How strongly a cue says "this is the price".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    /// `Iznos`, `Amount`: an amount, not necessarily the price.
    Weak = 1,
    /// `Cena`, `Naknada`, `Fee`, `Price`, `Ukupno`, `Total`.
    Fair = 3,
    /// `Ukupna vrednost ugovora`, `Ukupno za uplatu`, `Contract Value`, `Amount Due`.
    Strong = 4,
}

/// What an amount after a cue is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The cue says nothing about VAT.
    Plain,
    /// Without VAT.
    Net,
    /// VAT itself.
    Vat,
    /// With VAT.
    Gross,
}

/// How often an amount is paid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    /// Once.
    OneOff,
    /// Every month.
    Monthly,
    /// Every quarter.
    Quarterly,
    /// Every year.
    Yearly,
    /// Per hour, day or unit: a rate, never the document's price.
    Rate,
}

impl Period {
    /// The value written into `payment.period`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OneOff => "one_off",
            Self::Monthly => "monthly",
            Self::Quarterly => "quarterly",
            Self::Yearly => "yearly",
            Self::Rate => "rate",
        }
    }
}

/// A price cue: its strength, the VAT role it implies, the period it implies.
pub type Cue = (Strength, Role, Option<Period>);

const S: Strength = Strength::Strong;
const F: Strength = Strength::Fair;
const W: Strength = Strength::Weak;

/// Cues before the price (Serbian and English).
pub const PRICE: &[(&[&str], Cue)] = &[
    // Serbian: the contract value.
    (&["ukupna", "vrednost"], (S, Role::Plain, None)),
    (&["ukupna", "vrijednost"], (S, Role::Plain, None)),
    (&["ukupna", "ugovorena", "vrednost"], (S, Role::Plain, None)),
    (&["vrednost", "ugovora"], (S, Role::Plain, None)),
    (&["vrijednost", "ugovora"], (S, Role::Plain, None)),
    (&["ugovorena", "vrednost"], (S, Role::Plain, None)),
    (&["ugovorena", "cena"], (S, Role::Plain, None)),
    (&["ukupna", "cena"], (S, Role::Plain, None)),
    (&["ukupna", "ugovorena", "cena"], (S, Role::Plain, None)),
    (&["ukupna", "naknada"], (S, Role::Plain, None)),
    // Serbian: what is to be paid.
    (&["ukupno", "za", "uplatu"], (S, Role::Gross, None)),
    (&["ukupno", "za", "placanje"], (S, Role::Gross, None)),
    (&["iznos", "za", "uplatu"], (S, Role::Gross, None)),
    (&["iznos", "za", "placanje"], (S, Role::Gross, None)),
    (&["za", "uplatu"], (S, Role::Gross, None)),
    (&["za", "placanje"], (S, Role::Gross, None)),
    (&["sveukupno"], (S, Role::Plain, None)),
    (&["ukupno"], (F, Role::Plain, None)),
    (&["ukupan", "iznos"], (F, Role::Plain, None)),
    (&["ukupni", "iznos"], (F, Role::Plain, None)),
    (&["svega"], (F, Role::Plain, None)),
    (&["osnovica"], (F, Role::Net, None)),
    (&["poreska", "osnovica"], (F, Role::Net, None)),
    (&["medjuzbir"], (F, Role::Net, None)),
    // Serbian: fees and prices.
    (&["mesecna", "naknada"], (F, Role::Plain, Some(Period::Monthly))),
    (&["mesecni", "iznos"], (F, Role::Plain, Some(Period::Monthly))),
    (&["godisnja", "naknada"], (F, Role::Plain, Some(Period::Yearly))),
    (&["kvartalna", "naknada"], (F, Role::Plain, Some(Period::Quarterly))),
    (&["jednokratna", "naknada"], (F, Role::Plain, Some(Period::OneOff))),
    (&["naknada"], (F, Role::Plain, None)),
    (&["naknade"], (F, Role::Plain, None)),
    (&["naknadu"], (F, Role::Plain, None)),
    (&["naknadom"], (F, Role::Plain, None)),
    (&["cena"], (F, Role::Plain, None)),
    (&["cenu"], (F, Role::Plain, None)),
    (&["honorar*"], (F, Role::Plain, None)),
    (&["zakupnin*"], (F, Role::Plain, None)),
    (&["iznos*"], (W, Role::Plain, None)),
    // English.
    (&["contract", "value"], (S, Role::Plain, None)),
    (&["total", "contract", "value"], (S, Role::Plain, None)),
    (&["contract", "price"], (S, Role::Plain, None)),
    (&["total", "price"], (S, Role::Plain, None)),
    (&["total", "value"], (S, Role::Plain, None)),
    (&["total", "fee"], (S, Role::Plain, None)),
    (&["total", "fees"], (S, Role::Plain, None)),
    (&["amount", "due"], (S, Role::Gross, None)),
    (&["total", "amount", "due"], (S, Role::Gross, None)),
    (&["total", "due"], (S, Role::Gross, None)),
    (&["balance", "due"], (S, Role::Gross, None)),
    (&["amount", "payable"], (S, Role::Gross, None)),
    (&["grand", "total"], (S, Role::Plain, None)),
    (&["total", "amount"], (F, Role::Plain, None)),
    (&["total"], (F, Role::Plain, None)),
    (&["subtotal"], (F, Role::Net, None)),
    (&["sub", "total"], (F, Role::Net, None)),
    (&["monthly", "fee"], (F, Role::Plain, Some(Period::Monthly))),
    (&["monthly", "rent"], (F, Role::Plain, Some(Period::Monthly))),
    (&["monthly", "charge"], (F, Role::Plain, Some(Period::Monthly))),
    (&["annual", "fee"], (F, Role::Plain, Some(Period::Yearly))),
    (&["yearly", "fee"], (F, Role::Plain, Some(Period::Yearly))),
    (&["quarterly", "fee"], (F, Role::Plain, Some(Period::Quarterly))),
    (&["one", "time", "fee"], (F, Role::Plain, Some(Period::OneOff))),
    (&["fee"], (F, Role::Plain, None)),
    (&["fees"], (F, Role::Plain, None)),
    (&["price"], (F, Role::Plain, None)),
    (&["rent"], (F, Role::Plain, None)),
    (&["retainer"], (F, Role::Plain, None)),
    (&["amount"], (W, Role::Plain, None)),
];

/// Phrases that make an amount VAT itself when they open the line (within its first
/// tokens): `PDV 20%: 200.000,00`, `VAT (20%) EUR 2,000.00`.
pub const VAT_LABEL: &[(&[&str], ())] = &[
    (&["pdv"], ()),
    (&["iznos", "pdv"], ()),
    (&["obracunati", "pdv"], ()),
    (&["porez", "na", "dodatu", "vrednost"], ()),
    (&["vat"], ()),
    (&["vat", "amount"], ()),
    (&["value", "added", "tax"], ()),
    (&["tax"], ()),
];

/// Net and gross markers next to an amount (`bez PDV-a`, `sa PDV-om`, `excl. VAT`).
pub const VAT_MARK: &[(&[&str], Role)] = &[
    (&["sa", "pdv*"], Role::Gross),
    (&["s", "pdv*"], Role::Gross),
    (&["ukljucujuci", "pdv*"], Role::Gross),
    (&["ukljucen", "pdv*"], Role::Gross),
    (&["sa", "uracunatim", "pdv*"], Role::Gross),
    (&["uracunat", "pdv*"], Role::Gross),
    (&["sa", "porezom"], Role::Gross),
    (&["bruto"], Role::Gross),
    (&["incl", "vat"], Role::Gross),
    (&["inc", "vat"], Role::Gross),
    (&["including", "vat"], Role::Gross),
    (&["inclusive", "of", "vat"], Role::Gross),
    (&["vat", "included"], Role::Gross),
    (&["vat", "inclusive"], Role::Gross),
    (&["gross"], Role::Gross),
    (&["bez", "pdv*"], Role::Net),
    (&["plus", "pdv*"], Role::Net),
    (&["+", "pdv*"], Role::Net),
    (&["bez", "poreza"], Role::Net),
    (&["neto"], Role::Net),
    (&["excl", "vat"], Role::Net),
    (&["ex", "vat"], Role::Net),
    (&["excluding", "vat"], Role::Net),
    (&["exclusive", "of", "vat"], Role::Net),
    (&["plus", "vat"], Role::Net),
    (&["plus", "applicable", "vat"], Role::Net),
    (&["net", "of", "vat"], Role::Net),
    (&["vat", "excluded"], Role::Net),
    (&["vat", "exclusive"], Role::Net),
    (&["before", "vat"], Role::Net),
    (&["net"], Role::Net),
];

/// How often (or per what) an amount is paid.
pub const PERIOD: &[(&[&str], Period)] = &[
    (&["mesecn*"], Period::Monthly),
    (&["po", "mesecu"], Period::Monthly),
    (&["na", "mesecnom", "nivou"], Period::Monthly),
    (&["za", "svaki", "mesec"], Period::Monthly),
    (&["/", "mesec*"], Period::Monthly),
    (&["/", "mes"], Period::Monthly),
    (&["monthly"], Period::Monthly),
    (&["per", "month"], Period::Monthly),
    (&["a", "month"], Period::Monthly),
    (&["each", "month"], Period::Monthly),
    (&["every", "month"], Period::Monthly),
    (&["per", "calendar", "month"], Period::Monthly),
    (&["/", "month"], Period::Monthly),
    (&["/", "mo"], Period::Monthly),
    (&["kvartaln*"], Period::Quarterly),
    (&["tromesecn*"], Period::Quarterly),
    (&["po", "kvartalu"], Period::Quarterly),
    (&["quarterly"], Period::Quarterly),
    (&["per", "quarter"], Period::Quarterly),
    (&["each", "quarter"], Period::Quarterly),
    (&["godisnj*"], Period::Yearly),
    (&["po", "godini"], Period::Yearly),
    (&["na", "godisnjem", "nivou"], Period::Yearly),
    (&["/", "god*"], Period::Yearly),
    (&["annually"], Period::Yearly),
    (&["annual"], Period::Yearly),
    (&["yearly"], Period::Yearly),
    (&["per", "year"], Period::Yearly),
    (&["per", "annum"], Period::Yearly),
    (&["each", "year"], Period::Yearly),
    (&["/", "year"], Period::Yearly),
    (&["jednokratn*"], Period::OneOff),
    (&["one", "time"], Period::OneOff),
    (&["one", "off"], Period::OneOff),
    (&["lump", "sum"], Period::OneOff),
    (&["po", "satu"], Period::Rate),
    (&["po", "radnom", "satu"], Period::Rate),
    (&["po", "danu"], Period::Rate),
    (&["po", "komadu"], Period::Rate),
    (&["po", "jedinici"], Period::Rate),
    (&["jedinicn*"], Period::Rate),
    (&["dnevn*"], Period::Rate),
    (&["/", "h"], Period::Rate),
    (&["/", "sat*"], Period::Rate),
    (&["/", "dan"], Period::Rate),
    (&["/", "hour"], Period::Rate),
    (&["/", "day"], Period::Rate),
    (&["per", "hour"], Period::Rate),
    (&["per", "day"], Period::Rate),
    (&["per", "unit"], Period::Rate),
    (&["per", "item"], Period::Rate),
    (&["hourly"], Period::Rate),
    (&["daily"], Period::Rate),
    (&["unit", "price"], Period::Rate),
    (&["rate"], Period::Rate),
];

/// Words that make an amount on their line something other than the price: penalties,
/// guarantees, deposits, interest, advances, share capital, damages, liability caps. Ignored after a strong cue.
pub const NOT_PRICE: &[&str] = &[
    "kazn*",
    "penal*",
    "penalty",
    "penalties",
    "liquidated",
    "garanc*",
    "guarantee*",
    "menic*",
    "depozit*",
    "deposit*",
    "kamat*",
    "interest",
    "avans*",
    "advance",
    "kapital*",
    "capital",
    "stet*",
    "damages",
    "limit*",
    "cap",
];

/// Words that mark a line as being about paying (for a period cue away from the amount:
/// `Naknada se plaća mesečno`).
pub const PAY_WORDS: &[&str] = &[
    "naknada", "naknade", "naknadu", "plac*", "plat*", "uplat*", "fakturis*", "faktur*", "cen*", "zakupnin*", "fee",
    "fees", "pay*", "invoic*", "billed", "price", "rent", "charge*",
];

/// Labels of the subject: `(phrase, needs a colon or a verb after it, opens the line only)`.
pub const SUBJECT: &[(&[&str], (bool, bool))] = &[
    (&["predmet", "ovog", "ugovora"], (false, false)),
    (&["predmet", "ugovora"], (false, false)),
    (&["predmet", "nabavke"], (false, false)),
    (&["predmet", "ponude"], (false, false)),
    (&["predmet", "fakture"], (false, false)),
    (&["opis", "usluga"], (false, true)),
    (&["opis", "usluge"], (false, true)),
    (&["opis", "posla"], (false, true)),
    (&["opis", "radova"], (false, true)),
    (&["vrsta", "usluge"], (true, true)),
    (&["predmet"], (true, true)),
    (&["usluga"], (true, true)),
    (&["usluge"], (true, true)),
    (&["subject", "of", "the", "agreement"], (false, false)),
    (&["subject", "of", "this", "agreement"], (false, false)),
    (&["subject", "of", "the", "contract"], (false, false)),
    (&["subject", "of", "this", "contract"], (false, false)),
    (&["subject", "matter"], (false, true)),
    (&["scope", "of", "services"], (false, false)),
    (&["scope", "of", "the", "services"], (false, false)),
    (&["scope", "of", "work"], (false, false)),
    (&["description", "of", "services"], (false, false)),
    (&["description", "of", "the", "services"], (false, false)),
    (&["services"], (true, true)),
    (&["description"], (true, true)),
    (&["subject"], (true, true)),
];

/// Words skipped between a subject label and its text (`je`, `is`, `shall be`).
pub const SUBJECT_GLUE: &[&str] = &[
    "je", "su", "jeste", "is", "are", "shall", "will", "be", "include", "includes", "including",
    "obuhvata", "cini", "sledece", "following", "the", "as", "follows",
];

/// Words that open an invoice title (`Faktura`, `Račun br.`, `Invoice`).
pub const INVOICE_TITLE: &[&str] = &[
    "faktura", "racun", "predracun", "profaktura", "invoice", "proforma", "avansni",
];

/// Words of a contract or invoice title that say nothing about what is paid for.
pub const GENERIC_TITLE: &[&str] = &[
    "ugovor", "ugovora", "o", "za", "pruzanju", "usluga", "usluge", "okvirni", "sporazum",
    "aneks", "ponuda", "faktura", "racun", "predracun", "broj", "br", "agreement", "contract",
    "service", "services", "for", "the", "of", "master", "framework", "offer", "invoice", "no",
    "number", "and", "i", "tax", "proforma", "provision", "supply",
];

//! Party roles and the words that state them, Serbian declensions included.

/// What a document makes of a company it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Supplier side: supplier, vendor, provider, seller, contractor, dobavljač, izvršilac,
    /// prodavac, pružalac usluga, "Remit to".
    Supplier,
    /// Customer side: customer, client, buyer, purchaser, naručilac, kupac, korisnik.
    Customer,
    /// The sender of the document: letterhead, running header or footer.
    Issuer,
    /// A "Bill to" / "Invoice to" / "Sold to" block (customer side).
    BillTo,
    /// A "Ship to" / "Deliver to" / "Consignee" block (customer side).
    ShipTo,
    /// Named in a signature block ("For and on behalf of X", "Za X").
    Signatory,
    /// A party of a parties clause whose side the clause does not say.
    Party,
    /// An auditor, banker, solicitor or other adviser named as such ("Auditors" / "X LLP",
    /// "X LLP, Chartered Accountants"): it serves the document's author, it is not a party.
    Adviser,
    /// Mentioned anywhere else.
    Mentioned,
}

impl Role {
    /// Lowercase name, as emitted in `party.role`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supplier => "supplier",
            Self::Customer => "customer",
            Self::Issuer => "issuer",
            Self::BillTo => "bill_to",
            Self::ShipTo => "ship_to",
            Self::Signatory => "signatory",
            Self::Party => "party",
            Self::Adviser => "adviser",
            Self::Mentioned => "mentioned",
        }
    }

    /// Whether the role says the company is named, not that it takes part: `mentioned` and
    /// `adviser`. A key with weak roles only scores by its strongest mention.
    pub fn is_weak(self) -> bool {
        matches!(self, Self::Mentioned | Self::Adviser)
    }

    /// Whether the role puts the company on the buying side of the document.
    pub fn is_customer_side(self) -> bool {
        matches!(self, Self::Customer | Self::BillTo | Self::ShipTo)
    }
}

const SUPPLIER_WORDS: &[&str] = &[
    "supplier",
    "suppliers",
    "vendor",
    "provider",
    "seller",
    "contractor",
    "consultant",
    "licensor",
    "dobavljac",
    "isporucilac",
    "izvodjac",
    "konsultant",
    "izdavalac",
];
const SUPPLIER_STEMS: &[&str] = &[
    "dobavljac",
    "izvrsil",
    "izvrsio",
    "isporucil",
    "isporucio",
    "prodavac",
    "prodavc",
    "pruzala",
    "pruzao",
    "izvodjac",
    "davalac",
    "davaoc",
    "konsultant",
    // Award decisions and notices: "ugovor se dodeljuje ponuđaču / privrednom subjektu".
    "ponudjac",
    "subjekt",
];
const CUSTOMER_WORDS: &[&str] = &[
    "customer",
    "client",
    "buyer",
    "purchaser",
    "licensee",
    "kupac",
    "korisnik",
    "klijent",
    "investitor",
    "narucilac",
    "narucioc",
];
const CUSTOMER_STEMS: &[&str] = &[
    "narucil",
    "narucio",
    "kupc",
    "kupac",
    "korisnik",
    "klijent",
    "investitor",
];

/// The role a single folded word states: `supplier`, `Dobavljača`, `Naručioca`, `kupcu`.
///
/// ```
/// use quail_parties::roles::{role_of_word, Role};
/// assert_eq!(role_of_word("dobavljaca"), Some(Role::Supplier));
/// assert_eq!(role_of_word("narucioca"), Some(Role::Customer));
/// assert_eq!(role_of_word("acme"), None);
/// ```
pub fn role_of_word(key: &str) -> Option<Role> {
    if SUPPLIER_WORDS.contains(&key) || stem_hit(SUPPLIER_STEMS, key) {
        return Some(Role::Supplier);
    }
    if CUSTOMER_WORDS.contains(&key) || stem_hit(CUSTOMER_STEMS, key) {
        return Some(Role::Customer);
    }
    None
}

/// Serbian role words of [`SUPPLIER_WORDS`] and [`CUSTOMER_WORDS`].
const SERBIAN_WORDS: &[&str] = &[
    "dobavljac",
    "isporucilac",
    "izvodjac",
    "izdavalac",
    "kupac",
    "korisnik",
    "investitor",
    "narucilac",
    "narucioc",
];

/// Whether a folded role word is Serbian (`naručilac`, `Добављач`, `kupca`): its label's
/// value may be a name in sentence case (`Republički fond za zdravstveno osiguranje`).
///
/// ```
/// use quail_parties::roles::is_serbian_role_word;
/// assert!(is_serbian_role_word("narucilac"));
/// assert!(is_serbian_role_word("dobavljaca"));
/// assert!(!is_serbian_role_word("supplier"));
/// ```
pub fn is_serbian_role_word(key: &str) -> bool {
    SERBIAN_WORDS.contains(&key) || stem_hit(SUPPLIER_STEMS, key) || stem_hit(CUSTOMER_STEMS, key)
}

/// A declined form: the stem plus at most three letters (`narucioca`, `kupcima`).
fn stem_hit(stems: &[&str], key: &str) -> bool {
    stems.iter().any(|s| {
        key.strip_prefix(s)
            .is_some_and(|rest| rest.len() <= 3 && rest.bytes().all(|b| b.is_ascii_lowercase()))
    })
}

/// Multi-word labels (folded keys) and the role they state; longest first.
pub const LABELS: &[(&[&str], Role)] = &[
    (&["service", "provider"], Role::Supplier),
    (&["remit", "to"], Role::Supplier),
    (&["pay", "to"], Role::Supplier),
    (&["izdavalac", "racuna"], Role::Supplier),
    (&["bill", "to"], Role::BillTo),
    (&["billed", "to"], Role::BillTo),
    (&["invoice", "to"], Role::BillTo),
    (&["invoiced", "to"], Role::BillTo),
    (&["sold", "to"], Role::BillTo),
    (&["ship", "to"], Role::ShipTo),
    (&["shipped", "to"], Role::ShipTo),
    (&["deliver", "to"], Role::ShipTo),
    (&["delivered", "to"], Role::ShipTo),
    (&["delivery", "address"], Role::ShipTo),
    (&["shipping", "address"], Role::ShipTo),
    (&["mesto", "isporuke"], Role::ShipTo),
    (&["adresa", "isporuke"], Role::ShipTo),
    (&["consignee"], Role::ShipTo),
];

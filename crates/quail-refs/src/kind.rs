//! Instrument kinds, relations between documents and the record values facts carry.

use quail_core::Value;

/// What kind of instrument a document (or a cited document) is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Master / framework agreement ("Master Services Agreement", "Okvirni ugovor").
    Msa,
    /// Any other agreement or contract ("Agreement", "Ugovor", "Sporazum").
    Contract,
    /// Non-disclosure / confidentiality agreement.
    Nda,
    /// Statement of work or work order.
    Sow,
    /// Amendment or addendum. Serbian *aneks* is a contract amendment by law, so it is here.
    Amendment,
    /// Amended and restated agreement: replaces its predecessor.
    AmendedRestated,
    /// Annex, exhibit, schedule, appendix, attachment, *prilog*.
    Annex,
    /// Change order or change request.
    ChangeOrder,
    /// Side letter.
    SideLetter,
    /// Purchase order (*narudžbenica*).
    Po,
    /// Order form or service order.
    Order,
    /// Invoice (*faktura*, *račun*, pro forma).
    Invoice,
    /// Quotation or offer (*ponuda*).
    Quote,
    /// Public-procurement framework agreement (*okvirni sporazum*, "framework agreement").
    /// A private *okvirni ugovor* stays [`Kind::Msa`].
    FrameworkAgreement,
    /// Contract award decision (*odluka o dodeli ugovora / okvirnog sporazuma*, *odluka o
    /// izboru najpovoljnije ponude*).
    AwardDecision,
    /// Any other decision or ruling (*odluka*, *rešenje*).
    Decision,
    /// Price list (*cenovnik*, "price list").
    PriceList,
    /// Public notice or call (*obaveštenje*, *javni poziv*, *poziv za podnošenje ponuda*,
    /// *oglas*, "contract award notice").
    Notice,
    /// Tender documentation or a model contract inside it (*konkursna / tenderska
    /// dokumentacija*, *model ugovora*).
    TenderDocumentation,
    /// A public procurement procedure (*javna nabavka*, *JN*): only ever cited, with its
    /// number ("javna nabavka br. 404-1-110/19").
    Procurement,
    /// The registry (*delovodni*) number of a Serbian document ("Broj: 404-12/25-3" at its
    /// head): only ever an own number's kind, never a document kind.
    Registry,
}

impl Kind {
    /// Stable lowercase name used in facts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FrameworkAgreement => "framework_agreement",
            Self::AwardDecision => "award_decision",
            Self::Decision => "decision",
            Self::PriceList => "price_list",
            Self::Notice => "notice",
            Self::TenderDocumentation => "tender_documentation",
            Self::Procurement => "procurement",
            Self::Registry => "registry",
            Self::Msa => "msa",
            Self::Contract => "contract",
            Self::Nda => "nda",
            Self::Sow => "sow",
            Self::Amendment => "amendment",
            Self::AmendedRestated => "amended_restated",
            Self::Annex => "annex",
            Self::ChangeOrder => "change_order",
            Self::SideLetter => "side_letter",
            Self::Po => "po",
            Self::Order => "order",
            Self::Invoice => "invoice",
            Self::Quote => "quote",
        }
    }

    /// The kind a matched, folded kind phrase names (see the crate docs for the phrases).
    #[allow(clippy::too_many_lines)] // one flat table of phrase tests
    pub(crate) fn of_phrase(p: &str) -> Option<Self> {
        let first = p.split(' ').next().unwrap_or("");
        let second = p.split(' ').nth(1).unwrap_or("");
        let k = if p.starts_with("amended and restated") {
            Self::AmendedRestated
        } else if (first.starts_with("okvirn") && second.starts_with("sporazum"))
            || p == "framework agreement"
        {
            Self::FrameworkAgreement
        } else if first.starts_with("odluk") && (p.contains(" dodel") || p.contains(" izbor")) {
            Self::AwardDecision
        } else if first.starts_with("odluk") || first.starts_with("resenj") {
            Self::Decision
        } else if first.starts_with("cenovnik") || p == "price list" || p == "spisak cena" {
            Self::PriceList
        } else if first.starts_with("obavestenj")
            || first.starts_with("oglas")
            || p.contains("poziv")
            || p.ends_with(" notice")
        {
            Self::Notice
        } else if p.contains("dokumentacij") || first == "model" {
            Self::TenderDocumentation
        } else if first.starts_with("javn") || p == "jn" {
            Self::Procurement
        } else if p.starts_with("master ") || p == "msa" || first.starts_with("okvirn") {
            Self::Msa
        } else if p == "statement of work"
            || p == "sow"
            || p == "work order"
            || first.starts_with("radn")
        {
            Self::Sow
        } else if p.starts_with("change ") {
            Self::ChangeOrder
        } else if p == "side letter" {
            Self::SideLetter
        } else if p.contains("disclosure")
            || p.starts_with("confidentiality")
            || p == "nda"
            || p.contains("poverljiv")
        {
            Self::Nda
        } else if [
            "amendment",
            "addendum",
            "avenant",
            "aneks",
            "izmen",
            "dodat",
        ]
        .iter()
        .any(|w| first.starts_with(w))
        {
            Self::Amendment
        } else if [
            "annex",
            "exhibit",
            "schedule",
            "appendix",
            "attachment",
            "prilog",
        ]
        .iter()
        .any(|w| first.starts_with(w))
        {
            Self::Annex
        } else if p == "purchase order"
            || p == "bon de commande"
            || p == "po"
            || p == "p.o"
            || ["narudzb", "porudzb"].iter().any(|w| first.starts_with(w))
        {
            Self::Po
        } else if p == "order form" || p == "service order" || p == "order" {
            Self::Order
        } else if [
            "invoice",
            "facture",
            "inv",
            "faktur",
            "profaktur",
            "predracun",
            "racun",
        ]
        .iter()
        .any(|w| first.starts_with(w))
        {
            Self::Invoice
        } else if ["quotation", "quote", "ponud"]
            .iter()
            .any(|w| first.starts_with(w))
        {
            Self::Quote
        } else if ["agreement", "contract", "contrat", "ugovor", "sporazum"]
            .iter()
            .any(|w| first.starts_with(w))
        {
            Self::Contract
        } else {
            return None;
        };
        Some(k)
    }

    /// Whether a number labelled with `self` can be the own number of a document of kind
    /// `doc`.
    pub(crate) fn fits(self, doc: Self) -> bool {
        use Kind::{
            AmendedRestated, Contract, FrameworkAgreement, Msa, Nda, Order, Po, Procurement,
            SideLetter, Sow,
        };
        if self == Procurement {
            return false;
        }
        self == doc
            || matches!(
                (doc, self),
                (
                    Msa | Nda | AmendedRestated | SideLetter | FrameworkAgreement,
                    Contract
                ) | (AmendedRestated, Msa)
                    | (Po | Sow, Order)
                    | (Order, Po)
            )
    }

    /// Ordinal-only mentions ("SOW No. 3") are worth citing for these kinds.
    pub(crate) fn ordinal_citable(self) -> bool {
        matches!(self, Self::Sow | Self::Amendment | Self::ChangeOrder)
    }
}

/// How a document relates to one it cites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Relation {
    /// It amends the cited document ("Amendment No. 2 to the MSA", "Aneks uz Ugovor").
    Amends,
    /// It is an annex, exhibit or schedule of the cited document.
    AnnexTo,
    /// It was issued under the cited document ("pursuant to the MSA", "na osnovu Ugovora").
    IssuedUnder,
    /// It bills against the cited document (an invoice citing an order or a contract).
    BilledAgainst,
    /// Any other reference.
    References,
}

impl Relation {
    /// Stable lowercase name used in facts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amends => "amends",
            Self::AnnexTo => "annex_to",
            Self::IssuedUnder => "issued_under",
            Self::BilledAgainst => "billed_against",
            Self::References => "references",
        }
    }

    /// The relation a document of kind `doc` most likely has to a cited `cited`, when the
    /// text names none.
    pub(crate) fn default_for(doc: Option<Kind>, cited: Kind) -> Self {
        use Kind::{
            AmendedRestated, Amendment, Annex, ChangeOrder, Contract, FrameworkAgreement, Invoice,
            Msa, Nda, Order, Po, Quote, SideLetter, Sow,
        };
        let parent = matches!(
            cited,
            Msa | Contract | Nda | Sow | Po | Order | AmendedRestated | FrameworkAgreement
        );
        match doc {
            Some(Amendment | ChangeOrder | SideLetter | AmendedRestated) if parent => Self::Amends,
            Some(Annex) if parent => Self::AnnexTo,
            Some(Contract | Po | Order) if cited == FrameworkAgreement => Self::IssuedUnder,
            Some(Sow | Order | Po) if matches!(cited, Msa | Contract | Nda | AmendedRestated) => {
                Self::IssuedUnder
            }
            Some(Po) if cited == Sow => Self::IssuedUnder,
            Some(Invoice) if parent || cited == Quote => Self::BilledAgainst,
            _ => Self::References,
        }
    }
}

/// A record value with the parts in order.
pub(crate) fn record(fields: Vec<(&'static str, Value)>) -> Value {
    Value::record(fields)
}

/// The part `name` of a record value, if present: a thin wrapper over [`Value::field`].
///
/// ```
/// use quail_core::Value;
/// let v = Value::record([("kind", Value::text("po"))]);
/// assert_eq!(quail_refs::field(&v, "kind"), Some(&Value::text("po")));
/// assert_eq!(quail_refs::field(&v, "key"), None);
/// ```
pub fn field<'v>(record: &'v Value, name: &str) -> Option<&'v Value> {
    record.field(name)
}

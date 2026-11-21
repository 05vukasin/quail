//! Confidence of each kind of candidate, set so that `conf` ≈ the precision measured on
//! real text (see the crate README for the table and how it was measured). Values that
//! could not be measured (no such text in the corpora) are marked *prior*: they are set from
//! the strength of the check (the chance a random well-formed string passes it) and the
//! label, and are deliberately conservative.

// Identifiers.
/// IBAN with valid check digits and country length, no label.
pub const IBAN: f32 = 0.97;
/// IBAN, valid, after an `IBAN` label.
pub const IBAN_LABELED: f32 = 0.99;
/// Any well-formed identifier after its label but with wrong check digits (a typo, an OCR
/// error, or a different number): kept, rarely right as written.
pub const INVALID_LABELED: f32 = 0.2;
/// BIC after a `BIC` / `SWIFT` label.
pub const BIC_LABELED: f32 = 0.95;
/// BIC within a few tokens after an IBAN (prior).
pub const BIC_NEAR_IBAN: f32 = 0.85;
/// LEI with valid check digits, no label.
pub const LEI: f32 = 0.9;
/// LEI, valid, after a `LEI` label.
pub const LEI_LABELED: f32 = 0.99;
/// VAT number with its country prefix glued on and valid check digits (`DE136695976`).
pub const VAT_GLUED_VALID: f32 = 0.97;
/// VAT number after a tax / VAT label, valid check digits.
pub const VAT_LABELED_VALID: f32 = 0.98;
/// VAT number with a separate prefix (`DE 136695976`), valid, no label.
pub const VAT_SPACED_VALID: f32 = 0.9;
/// VAT number of a country without a check-digit algorithm here, prefix glued, no label.
pub const VAT_FORMAT_GLUED: f32 = 0.7;
/// Same, after a tax / VAT label.
pub const VAT_FORMAT_LABELED: f32 = 0.85;
/// Nine digits after a generic `VAT No` label that pass the UK check: read as `GB...`.
pub const VAT_GB_UNPREFIXED: f32 = 0.7;
/// PIB after a PIB / tax label, valid (prior: MOD 11,10 lets 1 in 10 random numbers pass,
/// the label carries the rest).
pub const PIB_LABELED: f32 = 0.97;
/// Bare 9-digit number starting with 1 with a valid PIB check digit, on a page that reads
/// as Serbian business text (prior).
pub const PIB_UNLABELED: f32 = 0.3;
/// Matični broj after its label, valid (prior).
pub const MB_LABELED: f32 = 0.95;
/// OIB after its label, valid (prior).
pub const OIB_LABELED: f32 = 0.97;
/// Serbian account `BBB-N-KK`, valid, after an account label.
pub const ACCOUNT_LABELED: f32 = 0.98;
/// Serbian account written with dashes, valid, no label (1 in 97 random numbers pass).
pub const ACCOUNT_DASHED: f32 = 0.9;
/// Serbian account as 18 bare digits, valid, no label.
pub const ACCOUNT_PLAIN: f32 = 0.6;
/// DUNS after its label.
pub const DUNS: f32 = 0.9;
/// US EIN `12-3456789` with an EIN / tax label before or after and a valid prefix.
pub const EIN: f32 = 0.9;
/// The JMBG count: valid date and check digit.
pub const JMBG: f32 = 0.9;

// Dates.
/// `12.09.2023` (day first, dots).
pub const DATE_DOTTED: f32 = 0.97;
/// `2023-09-12`.
pub const DATE_ISO: f32 = 0.97;
/// Slash/dash date whose order is certain (`25/09/2023`).
pub const DATE_SLASH: f32 = 0.95;
/// Ambiguous slash date read by the file's order evidence.
pub const DATE_AMBIGUOUS_EVIDENCE: f32 = 0.85;
/// Ambiguous slash date without evidence (read day-first).
pub const DATE_AMBIGUOUS_DEFAULT: f32 = 0.6;
/// Date with a month name.
pub const DATE_MONTH_NAME: f32 = 0.97;
/// Precision of the effective-date role (multiplies the date's conf).
pub const DATE_ROLE_EFFECTIVE: f32 = 0.78;
/// Precision of the first effective-role date on the first two pages (the agreement's own;
/// measured as the effective-date prediction on Kleister-NDA / CUAD dev: 0.95 / 0.83 right,
/// absent-gold documents counted wrong).
pub const DATE_ROLE_EFFECTIVE_FIRST: f32 = 0.92;
/// Precision of the expiry-date role.
pub const DATE_ROLE_EXPIRY: f32 = 0.8;
/// Factor on an expiry date before the agreement's effective date.
pub const DATE_EXPIRY_BEFORE_EFFECTIVE: f32 = 0.3;
/// Precision of the signing-date role.
pub const DATE_ROLE_SIGNED: f32 = 0.65;
/// Precision of the first signing phrase on the first two pages (measured as the date
/// prediction on Kleister-NDA / CUAD dev: about 0.9 with absent-gold documents counted
/// wrong).
pub const DATE_ROLE_SIGNED_FIRST: f32 = 0.85;
/// Factor on a signing date two years or more before the agreement's effective date.
pub const DATE_SIGNED_YEARS_BEFORE: f32 = 0.5;
/// Precision of a bare `Date:` / `Datum:` / `dana` label right before a date (signing).
pub const DATE_ROLE_LABEL: f32 = 0.5;
/// Precision of a date standing alone on a line at the head of the first page (the date
/// of a letter agreement or a cover page; signing role).
pub const DATE_ROLE_HEAD: f32 = 0.9;
/// Precision of the invoice-date role.
pub const DATE_ROLE_INVOICE: f32 = 0.95;
/// Precision of the due-date role.
pub const DATE_ROLE_DUE: f32 = 0.95;

// Money.
/// Amount next to a currency code or word.
pub const MONEY_CODE: f32 = 0.97;
/// Amount next to a currency symbol.
pub const MONEY_SYMBOL: f32 = 0.97;
/// Amount with decimals after a total keyword, no currency.
pub const MONEY_BARE_TOTAL: f32 = 0.6;
/// Precision of the total role (multiplies the amount's conf).
pub const MONEY_TOTAL_ROLE: f32 = 0.9;
/// Precision of the net / gross VAT role (multiplies the amount's conf).
pub const MONEY_VAT_ROLE: f32 = 0.9;

// Contacts.
/// E-mail domain from an e-mail address.
pub const EMAIL: f32 = 0.99;
/// Web domain from a URL or `www.` host.
pub const URL: f32 = 0.97;
/// Web domain written bare (`firma.rs`), common TLDs only.
pub const WEB_BARE: f32 = 0.9;
/// International phone number after a phone label.
pub const PHONE_INTL_LABELED: f32 = 0.97;
/// International phone number, no label.
pub const PHONE_INTL: f32 = 0.95;
/// Domestic number after a phone label (Serbian or North American).
pub const PHONE_DOMESTIC_LABELED: f32 = 0.95;
/// North American `(AAA) BBB-CCCC` without a label.
pub const PHONE_NANP_PAREN: f32 = 0.9;

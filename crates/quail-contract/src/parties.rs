//! Parties: who is supplier and who is customer, their identifiers, addresses,
//! representatives and signatories.
//!
//! A party is found by its role mark: a label at the start of a line (`Naručilac:`,
//! `Supplier:`), or a defined term (`(u daljem tekstu: Izvođač)`, `(hereinafter the
//! "Customer")`, `(the "Supplier")`). The text the mark closes (back to the previous mark in
//! the sentence, or the label line and the detail lines under it) is the party's segment:
//! its name is the `party.company` mention there, and identifiers (`PIB`, `MB`, `VAT No.`,
//! `IBAN`, `tekući račun`), the address (`sa sedištem u`, `registered office at`) and the
//! representative (`koga zastupa direktor`, `represented by`) found there are the party's.
//! A representative right after an English mark (`(the "Supplier"), represented by ...`)
//! belongs to it. Signature blocks (`Za Naručioca`, `For the Supplier`) give the signatory's
//! name and title from the lines under the label.

use quail_core::{Span, Value};
use quail_payment::lex::{Line, Tok, match_at, word_matches};
use quail_text::token::TokenKind;

use crate::analyzer::{Found, Input, conf, rec, t};
use crate::keys;
use crate::text::{find, name_at, raw, span_of, text_of, upper_start};

/// The side a party is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Supplier,
    Customer,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Self::Supplier => "supplier",
            Self::Customer => "customer",
        }
    }
}

/// Role words (folded, `*` a prefix).
const ROLES: &[(&str, Side)] = &[
    ("dobavljac*", Side::Supplier),
    ("izvodjac*", Side::Supplier),
    ("isporucilac", Side::Supplier),
    ("isporucioc*", Side::Supplier),
    ("prodavac", Side::Supplier),
    ("prodavc*", Side::Supplier),
    ("pruzalac", Side::Supplier),
    ("pruzaoc*", Side::Supplier),
    ("davalac", Side::Supplier),
    ("davaoc*", Side::Supplier),
    ("zakupodav*", Side::Supplier),
    ("izvrsilac", Side::Supplier),
    ("izvrsioc*", Side::Supplier),
    ("licencodav*", Side::Supplier),
    ("supplier*", Side::Supplier),
    ("vendor*", Side::Supplier),
    ("contractor*", Side::Supplier),
    ("provider*", Side::Supplier),
    ("seller*", Side::Supplier),
    ("licensor*", Side::Supplier),
    ("lessor*", Side::Supplier),
    ("landlord*", Side::Supplier),
    ("consultant*", Side::Supplier),
    ("narucilac", Side::Customer),
    ("narucioc*", Side::Customer),
    ("kupac", Side::Customer),
    ("kupc*", Side::Customer),
    ("korisnik*", Side::Customer),
    ("zakupac", Side::Customer),
    ("zakupc*", Side::Customer),
    ("investitor*", Side::Customer),
    ("primalac", Side::Customer),
    ("primaoc*", Side::Customer),
    ("licencoprim*", Side::Customer),
    ("klijent*", Side::Customer),
    ("customer*", Side::Customer),
    ("client*", Side::Customer),
    ("buyer*", Side::Customer),
    ("purchaser*", Side::Customer),
    ("licensee*", Side::Customer),
    ("lessee*", Side::Customer),
    ("tenant*", Side::Customer),
];

fn side(key: &str) -> Option<Side> {
    ROLES
        .iter()
        .find(|(w, _)| word_matches(w, key))
        .map(|&(_, s)| s)
}

/// Phrases that introduce a defined term.
const DEFINE: &[(&[&str], ())] = &[
    (&["u", "daljem", "tekstu"], ()),
    (&["u", "daljnjem", "tekstu"], ()),
    (&["u", "daljem", "tekstu", "ugovora"], ()),
    (&["dalje"], ()),
    (&["hereinafter"], ()),
    (&["hereinafter", "referred", "to", "as"], ()),
    (&["referred", "to", "as"], ()),
    (&["("], ()),
];

/// Tokens skipped between a defined-term phrase and the role word.
fn define_skip(t: &Tok) -> bool {
    matches!(
        t.key.as_str(),
        ":" | "\"" | "„" | "“" | "”" | "'" | "‘" | "’" | "«" | "»" | "the" | "as" | "kao" | "called"
            | "-" | ","
    )
}

/// Identifier kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdKind {
    Pib,
    Mb,
    Vat,
    Reg,
    Iban,
    Account,
    Bic,
}

impl IdKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pib => "pib",
            Self::Mb => "mb",
            Self::Vat => "vat",
            Self::Reg => "reg",
            Self::Iban => "iban",
            Self::Account => "account",
            Self::Bic => "bic",
        }
    }

    /// The `quail-signals` key that checks this kind.
    fn signal(self) -> Option<&'static str> {
        Some(match self {
            Self::Pib => "signal.rs_pib",
            Self::Mb => "signal.rs_mb",
            Self::Vat => "signal.vat",
            Self::Iban => "signal.iban",
            Self::Account => "signal.rs_account",
            Self::Bic => "signal.bic",
            Self::Reg => return None,
        })
    }
}

const IDS: &[(&[&str], IdKind)] = &[
    (&["pib"], IdKind::Pib),
    (&["poreski", "identifikacioni", "broj"], IdKind::Pib),
    (&["poreski", "broj"], IdKind::Pib),
    (&["mb"], IdKind::Mb),
    (&["mbr"], IdKind::Mb),
    (&["maticni", "broj"], IdKind::Mb),
    (&["maticni", "br"], IdKind::Mb),
    (&["mat", "br"], IdKind::Mb),
    (&["mat", "broj"], IdKind::Mb),
    (&["vat"], IdKind::Vat),
    (&["vat", "id"], IdKind::Vat),
    (&["registration", "number"], IdKind::Reg),
    (&["registration", "no"], IdKind::Reg),
    (&["company", "registration", "number"], IdKind::Reg),
    (&["reg", "no"], IdKind::Reg),
    (&["company", "number"], IdKind::Reg),
    (&["company", "no"], IdKind::Reg),
    (&["registarski", "broj"], IdKind::Reg),
    (&["iban"], IdKind::Iban),
    (&["tekuci", "racun"], IdKind::Account),
    (&["ziro", "racun"], IdKind::Account),
    (&["poslovni", "racun"], IdKind::Account),
    (&["racun", "broj"], IdKind::Account),
    (&["broj", "racuna"], IdKind::Account),
    (&["broj", "tekuceg", "racuna"], IdKind::Account),
    (&["account", "number"], IdKind::Account),
    (&["account", "no"], IdKind::Account),
    (&["bank", "account"], IdKind::Account),
    (&["swift"], IdKind::Bic),
    (&["swift", "code"], IdKind::Bic),
    (&["bic"], IdKind::Bic),
];

/// Words between an identifier label and its value.
fn id_skip(t: &Tok) -> bool {
    t.kind == TokenKind::Punct
        || matches!(
            t.key.as_str(),
            "br" | "broj" | "no" | "number" | "nr" | "id" | "code" | "je" | "is" | "reg" | "kod"
        )
}

/// Address labels; `true` when the label is part of the address (`ul.`).
const ADDRESS: &[(&[&str], bool)] = &[
    (&["sa", "sedistem", "u"], false),
    (&["sa", "sedistem", "na", "adresi"], false),
    (&["sa", "sedistem"], false),
    (&["sediste"], false),
    (&["sedistem", "u"], false),
    (&["adresa"], false),
    (&["adresa", "sedista"], false),
    (&["na", "adresi"], false),
    (&["ul"], true),
    (&["ulica"], true),
    (&["bulevar"], true),
    (&["registered", "office", "at"], false),
    (&["registered", "office"], false),
    (&["registered", "address"], false),
    (&["with", "its", "registered", "office", "at"], false),
    (&["having", "its", "registered", "office", "at"], false),
    (&["principal", "place", "of", "business", "at"], false),
    (&["address"], false),
    (&["located", "at"], false),
];

/// Representative cues.
const REP: &[(&[&str], ())] = &[
    (&["koga", "zastupa"], ()),
    (&["koje", "zastupa"], ()),
    (&["kog", "zastupa"], ()),
    (&["koju", "zastupa"], ()),
    (&["zastupano", "po"], ()),
    (&["zastupan", "po"], ()),
    (&["zastupano", "od", "strane"], ()),
    (&["zastupan", "od", "strane"], ()),
    (&["koga", "predstavlja"], ()),
    (&["zastupnik"], ()),
    (&["zakonski", "zastupnik"], ()),
    (&["represented", "by"], ()),
    (&["duly", "represented", "by"], ()),
    (&["acting", "through"], ()),
    (&["acting", "by"], ()),
];

/// Words of a title (`direktor`, `Managing Director`, `CEO`).
const TITLE: &[&str] = &[
    "direktor*", "direktork*", "generaln*", "izvrsn*", "predsednik*", "zastupnik*", "prokurist*",
    "vlasnik*", "osnivac*", "ovlascen*", "upravnik*", "rukovodilac", "lice", "managing",
    "director*", "ceo", "cfo", "coo", "cto", "president", "chairman", "partner", "officer",
    "general", "manager", "owner", "authorised", "authorized", "signatory", "chief",
    "executive", "financial", "operating", "head", "vice", "clan", "member", "board",
];

/// Words that are never part of a person's name.
const NOT_NAME: &[&str] = &[
    "direktor*", "generaln*", "predsednik*", "zastupnik*", "prokurist*", "managing", "director*",
    "ceo", "cfo", "president", "chairman", "manager", "owner", "chief", "executive", "ugovor*",
    "agreement", "contract", "republik*", "beograd*", "novi", "sad*", "pib", "mb", "the",
    "naruci*", "izvodj*", "dobavlj*", "kupac", "kupc*", "supplier", "customer", "client",
    "potpis*", "signature", "datum", "date", "name", "title", "ime", "funkcija", "pecat",
    "doo", "llc", "ltd", "inc", "gmbh", "plc", "srbij*", "serbia", "zakupac", "zakupodavac",
    "seller", "buyer", "landlord", "tenant", "lessor", "lessee",
];

fn is_title(t: &Tok) -> bool {
    t.kind == TokenKind::Word && TITLE.iter().any(|w| word_matches(w, &t.key))
}

/// Lines that start a party's detail (kept in a label-style party block).
const DETAIL_START: &[&str] = &[
    "pib", "mb", "mbr", "maticni", "adresa", "sediste", "ul", "ulica", "zastupnik", "zastupa",
    "koga", "direktor", "tekuci", "ziro", "racun", "iban", "swift", "bic", "vat", "reg",
    "registration", "address", "represented", "company", "account", "tel", "telefon", "e",
    "email", "mail", "phone", "registered", "poreski", "sifra", "delatnost", "kontakt",
];

/// A role mark: the side, the token range of the role word, where the party's segment ends
/// (exclusive) and where its tail ends (the representative after an English mark).
struct Mark {
    side: Side,
    role: usize,
    seg_end: usize,
    tail_end: usize,
}

/// The defined-term marks of a sentence.
fn define_marks(s: &Line) -> Vec<Mark> {
    let mut out: Vec<Mark> = Vec::new();
    for h in find(&s.toks, DEFINE) {
        let mut j = h.end;
        let mut role = None;
        while j < s.toks.len() && j < h.end + 5 {
            let tok = &s.toks[j];
            if let Some(sd) = side(&tok.key) {
                role = Some((sd, j));
                break;
            }
            if !define_skip(tok) {
                break;
            }
            j += 1;
        }
        let Some((sd, r)) = role else { continue };
        // A parenthesis mark must close right after the role word.
        let open = s.toks[h.start].key == "(";
        let mut close = r + 1;
        while close < s.toks.len()
            && close < r + 4
            && matches!(
                s.toks[close].key.as_str(),
                "\"" | "”" | "“" | "'" | "’" | "»" | ")"
            )
        {
            close += 1;
        }
        if open && !s.toks[r + 1..close].iter().any(|t| t.key == ")") {
            continue;
        }
        if out.iter().any(|m| m.role == r) {
            continue;
        }
        out.push(Mark {
            side: sd,
            role: r,
            seg_end: close,
            tail_end: close,
        });
    }
    // A representative right after the mark belongs to it.
    for m in &mut out {
        let reps = find(&s.toks[m.seg_end..], REP);
        if let Some(h) = reps.first()
            && h.start <= 2
        {
            let mut k = m.seg_end + h.end;
            while k < s.toks.len() && !matches!(s.toks[k].key.as_str(), "and" | "i" | ";" | "(") {
                k += 1;
            }
            m.tail_end = k;
        }
    }
    out.sort_by_key(|m| m.role);
    out
}

/// Joins consecutive lines into one token line.
fn join(lines: &[Line]) -> Line {
    let mut l = lines[0].clone();
    for x in &lines[1..] {
        l.toks.extend(x.toks.iter().cloned());
        l.span.end = x.span.end;
    }
    l
}

/// One party segment: tokens `a..b` of `seg`.
#[derive(Clone)]
struct Segment<'l> {
    seg: &'l Line,
    a: usize,
    b: usize,
    side: Side,
    label: String,
    /// The role word's token, for the evidence of `contract.party`.
    role_tok: usize,
}

/// The `party.company` mention inside the segment's byte range, most confident first.
fn company_in(inp: &Input<'_>, s: &Segment<'_>) -> Option<(String, f32, Span)> {
    if s.a >= s.b {
        return None;
    }
    let lo = s.seg.toks[s.a].start;
    let hi = s.seg.toks[s.b - 1].end;
    let mut best: Option<(String, f32, Span)> = None;
    for (f, page, span) in inp.located("party.company") {
        if page != s.seg.page || span.start < lo || span.end > hi {
            continue;
        }
        let Some(name) = f.value.field("surface").and_then(Value::as_text) else {
            continue;
        };
        if best.as_ref().is_none_or(|b| f.conf > b.1) {
            best = Some((name.to_owned(), f.conf, span));
        }
    }
    best
}

/// Whether the identifier `value` of `kind` is one `quail-signals` confirmed (check digits).
/// Returns the signal's normalised value.
fn checked(inp: &Input<'_>, kind: IdKind, value: &str) -> Option<String> {
    let key = kind.signal()?;
    let norm = |s: &str| -> String {
        if kind == IdKind::Account {
            // `160-0000000123456-78` and `160-123456-78` are one account.
            let parts: Vec<&str> = s.split('-').collect();
            if parts.len() == 3 {
                return format!("{}-{}-{}", parts[0], parts[1].trim_start_matches('0'), parts[2]);
            }
        }
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_uppercase()
    };
    let want = norm(value);
    inp.facts
        .iter()
        .filter(|f| f.key.as_str() == key)
        .filter_map(|f| f.value.as_text())
        .find(|v| norm(v) == want)
        .map(str::to_owned)
}

/// The identifier value after its label at token `i`: compact, and whether its shape fits.
fn id_value(inp: &Input<'_>, seg: &Line, kind: IdKind, i: usize, b: usize) -> Option<(String, usize)> {
    let mut k = i;
    let mut skipped = 0;
    while k < b && skipped < 4 && id_skip(&seg.toks[k]) {
        k += 1;
        skipped += 1;
    }
    let tok = seg.toks.get(k).filter(|_| k < b)?;
    let s = raw(inp.doc, seg, tok);
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = compact.chars().filter(char::is_ascii_digit).count();
    let ok = match kind {
        IdKind::Pib => tok.kind == TokenKind::Number && compact.len() == 9 && digits == 9,
        IdKind::Mb => tok.kind == TokenKind::Number && compact.len() == 8 && digits == 8,
        IdKind::Iban => {
            let b = compact.as_bytes();
            (15..=34).contains(&b.len())
                && b[..2].iter().all(u8::is_ascii_alphabetic)
                && b[2..4].iter().all(u8::is_ascii_digit)
                && b.iter().all(u8::is_ascii_alphanumeric)
        }
        IdKind::Account => {
            tok.kind == TokenKind::Number
                && (10..=20).contains(&digits)
                && compact.chars().all(|c| c.is_ascii_digit() || c == '-')
        }
        IdKind::Vat => {
            let b = compact.as_bytes();
            (8..=14).contains(&b.len())
                && b.iter().all(u8::is_ascii_alphanumeric)
                && digits >= 6
        }
        IdKind::Bic => {
            (compact.len() == 8 || compact.len() == 11)
                && compact.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && compact.chars().take(4).all(|c| c.is_ascii_uppercase())
        }
        IdKind::Reg => {
            // `HRB 12345`, `12345678`.
            if tok.kind == TokenKind::Word
                && compact.len() <= 4
                && compact.chars().all(|c| c.is_ascii_uppercase())
                && let Some(n) = seg.toks.get(k + 1).filter(|t| t.kind == TokenKind::Number)
            {
                let v = format!("{compact} {}", raw(inp.doc, seg, n));
                return Some((v, k + 1));
            }
            matches!(tok.kind, TokenKind::Number | TokenKind::Alnum) && digits >= 4
        }
    };
    if !ok {
        return None;
    }
    let value = match kind {
        IdKind::Account | IdKind::Reg => compact,
        _ => compact.replace('-', "").to_ascii_uppercase(),
    };
    Some((value, k))
}

/// The address after its label at tokens `h.start..h.end`.
fn address(seg: &Line, end: usize, b: usize) -> Option<usize> {
    let mut last = None;
    let mut k = end;
    while k < seg.toks.len() && k < b && k < end + 14 {
        let tok = &seg.toks[k];
        if matches!(tok.key.as_str(), "(" | ";")
            || match_at(&seg.toks, k, &["u", "daljem"]).is_some()
            || !find(&seg.toks[k..(k + 1).min(seg.toks.len())], IDS).is_empty()
            || !find(&seg.toks[k..(k + 2).min(seg.toks.len())], REP).is_empty()
            || (k > end && seg.toks[k - 1].key == "," && is_title(tok))
        {
            break;
        }
        if tok.kind != TokenKind::Punct || matches!(tok.key.as_str(), "." | "/") {
            last = Some(k);
        }
        k += 1;
    }
    let last = last?;
    let words = (end..=last)
        .filter(|&k| seg.toks[k].kind != TokenKind::Punct)
        .count();
    let has_number = (end..=last).any(|k| seg.toks[k].kind == TokenKind::Number);
    (words >= 2 || has_number).then_some(last)
}

/// A representative after its cue ending at token `end`: `(title, name, last token)`.
fn representative(
    inp: &Input<'_>,
    seg: &Line,
    end: usize,
    b: usize,
) -> Option<(Option<String>, Option<String>, usize)> {
    let doc = inp.doc;
    let mut k = end;
    while k < b && matches!(seg.toks[k].key.as_str(), ":" | "its" | "njegov" | "njen" | "the") {
        k += 1;
    }
    let title_run = |k: usize| -> Option<usize> {
        let mut j = k;
        while j < b && is_title(&seg.toks[j]) {
            j += 1;
        }
        (j > k).then(|| j - 1)
    };
    let mut title = None;
    let mut name = None;
    let mut last = None;
    if let Some(te) = title_run(k) {
        title = Some(text_of(doc, seg, k, te));
        last = Some(te);
        let mut n = te + 1;
        if n < b && seg.toks[n].key == "," {
            n += 1;
        }
        if n < b
            && let Some(ne) = name_at(doc, seg, n, NOT_NAME)
            && ne < b
        {
            name = Some(text_of(doc, seg, n, ne));
            last = Some(ne);
        }
    } else if let Some(ne) = name_at(doc, seg, k, NOT_NAME).filter(|&ne| ne < b) {
        name = Some(text_of(doc, seg, k, ne));
        last = Some(ne);
        let mut n = ne + 1;
        if n < b && seg.toks[n].key == "," {
            n += 1;
        }
        if n < b
            && let Some(te) = title_run(n)
        {
            title = Some(text_of(doc, seg, n, te));
            last = Some(te);
        }
    }
    Some((title, name, last?))
}

/// Facts of one party segment.
fn segment(inp: &Input<'_>, s: &Segment<'_>, fallback: Option<&(String, Span)>, out: &mut Vec<Found>) {
    let seg = s.seg;
    let role = s.side.as_str();
    let company = company_in(inp, s);
    let party_name = company
        .as_ref()
        .map(|c| c.0.clone())
        .or_else(|| fallback.map(|f| f.0.clone()));
    if let Some(name) = &party_name {
        let (base, span) = match (&company, fallback) {
            (Some(c), _) => (0.85f32.min(c.1 + 0.1).max(0.7), c.2),
            (None, Some(f)) => (0.55, f.1),
            (None, None) => (
                0.5,
                Span {
                    start: seg.toks[s.role_tok].start,
                    end: seg.toks[s.role_tok].end,
                },
            ),
        };
        let span = Span {
            start: span.start.min(seg.toks[s.role_tok].start),
            end: span.end.max(seg.toks[s.role_tok].end),
        };
        out.push(Found {
            key: keys::PARTY,
            value: rec(vec![
                ("role", t(role)),
                ("label", t(s.label.as_str())),
                ("name", t(name.as_str())),
            ]),
            conf: conf(seg, base),
            page: seg.page,
            span,
        });
    }
    let party = || party_name.as_deref().map(Value::text);
    let window = &seg.toks[..s.b];
    // Identifiers.
    for h in find(&window[s.a..], IDS) {
        let (hs, he) = (s.a + h.start, s.a + h.end);
        let kind = *h.what;
        let Some((value, last)) = id_value(inp, seg, kind, he, s.b) else {
            continue;
        };
        let signal = checked(inp, kind, &value);
        let base = if signal.is_some() { 0.9 } else { 0.6 };
        out.push(Found {
            key: keys::PARTY_ID,
            value: rec(vec![
                ("role", t(role)),
                ("party", party()),
                ("kind", t(kind.as_str())),
                ("value", t(signal.clone().unwrap_or(value))),
                ("checked", Some(Value::Bool(signal.is_some()))),
            ]),
            conf: conf(seg, base),
            page: seg.page,
            span: span_of(seg, hs, last),
        });
    }
    // Address: the first one.
    for h in find(&window[s.a..], ADDRESS) {
        let (hs, he) = (s.a + h.start, s.a + h.end);
        let from = if *h.what { hs } else { he };
        let Some(last) = address(seg, from, s.b) else {
            continue;
        };
        let mut first = from;
        while first < last && seg.toks[first].kind == TokenKind::Punct {
            first += 1;
        }
        out.push(Found {
            key: keys::ADDRESS,
            value: rec(vec![
                ("role", t(role)),
                ("party", party()),
                ("text", t(text_of(inp.doc, seg, first, last))),
            ]),
            conf: conf(seg, 0.7),
            page: seg.page,
            span: span_of(seg, hs, last),
        });
        break;
    }
    // Representative: in the segment or its tail.
    for h in find(&window[s.a..], REP) {
        let (hs, he) = (s.a + h.start, s.a + h.end);
        let Some((title, name, last)) = representative(inp, seg, he, s.b) else {
            continue;
        };
        let base = if name.is_some() { 0.8 } else { 0.65 };
        out.push(Found {
            key: keys::REPRESENTATIVE,
            value: rec(vec![
                ("role", t(role)),
                ("party", party()),
                ("name", name.map(Value::text)),
                ("title", title.map(Value::text)),
            ]),
            conf: conf(seg, base),
            page: seg.page,
            span: span_of(seg, hs, last),
        });
        break;
    }
}

/// A label line: `Naručilac: ACME d.o.o.`, `Supplier: ...`. Returns the side, the role
/// token and the token after the colon.
fn label_line(doc: &quail_core::Document, l: &Line) -> Option<(Side, usize, usize)> {
    let first = l.toks.first()?;
    let sd = side(&first.key)?;
    if !upper_start(raw(doc, l, first)) {
        return None;
    }
    let colon = l.toks.iter().take(3).position(|t| t.key == ":")?;
    if colon == 2 && l.toks[1].kind != TokenKind::Word {
        return None;
    }
    Some((sd, 0, colon + 1))
}

/// The name after a label's colon when no company mention is there: up to the first comma,
/// 1 to 8 words, capitalised, no digits.
fn label_fallback(inp: &Input<'_>, l: &Line, from: usize) -> Option<(String, Span)> {
    let mut last = None;
    for k in from..l.toks.len() {
        let tok = &l.toks[k];
        if tok.key == "," {
            break;
        }
        if tok.kind == TokenKind::Number {
            return None;
        }
        last = Some(k);
    }
    let last = last?;
    if last - from >= 12 || !upper_start(raw(inp.doc, l, &l.toks[from])) {
        return None;
    }
    let words = (from..=last).filter(|&k| l.toks[k].kind == TokenKind::Word).count();
    if words == 0 || words > 8 {
        return None;
    }
    Some((text_of(inp.doc, l, from, last), span_of(l, from, last)))
}

/// Signature-block cues before the role word.
const SIG: &[(&[&str], ())] = &[
    (&["za"], ()),
    (&["u", "ime"], ()),
    (&["for"], ()),
    (&["for", "the"], ()),
    (&["on", "behalf", "of"], ()),
    (&["on", "behalf", "of", "the"], ()),
    (&["for", "and", "on", "behalf", "of"], ()),
    (&["for", "and", "on", "behalf", "of", "the"], ()),
];

/// A signature label line: the side, the label's last token.
fn sig_line(doc: &quail_core::Document, l: &Line) -> Option<(Side, usize)> {
    let first = l.toks.first()?;
    if !upper_start(raw(doc, l, first)) {
        return None;
    }
    let h = find(&l.toks[..l.toks.len().min(7)], SIG).into_iter().next()?;
    if h.start != 0 {
        return None;
    }
    let r = h.end;
    let sd = side(&l.toks.get(r)?.key)?;
    // Two labels on one line are two columns: their names cannot be told apart.
    let roles = l.toks.iter().filter(|t| side(&t.key).is_some()).count();
    if roles > 1 || l.toks.iter().filter(|t| t.kind != TokenKind::Punct).count() > 10 {
        return None;
    }
    Some((sd, r))
}

/// Lines in a signature block that carry no name (`Potpis`, `M.P.`, `______`, `Datum:`).
fn sig_noise(l: &Line) -> bool {
    l.toks.iter().any(|t| {
        matches!(t.key.as_str(), "potpis" | "signature" | "m.p" | "mp" | "datum" | "date" | "pecat")
    })
}

/// Name and title labels in a signature block.
const NAME_LABEL: &[&[&str]] = &[&["ime", "i", "prezime"], &["name"], &["ime"], &["by"]];
const TITLE_LABEL: &[&[&str]] = &[&["title"], &["funkcija"], &["position"], &["svojstvo"]];

fn signatories(inp: &Input<'_>, out: &mut Vec<Found>) {
    let doc = inp.doc;
    let lines = inp.lines;
    for (li, l) in lines.iter().enumerate() {
        let Some((sd, r)) = sig_line(doc, l) else { continue };
        let label = text_of(doc, l, 0, r);
        let mut name: Option<String> = None;
        let mut title: Option<String> = None;
        let mut end = l.toks[r].end;
        // The rest of the label line, then up to three lines under it.
        let mut chunks: Vec<(&Line, usize)> = Vec::new();
        if r + 1 < l.toks.len() {
            chunks.push((l, r + 1));
        }
        for x in lines.iter().skip(li + 1).take(3) {
            if x.pos != l.pos || sig_line(doc, x).is_some() {
                break;
            }
            chunks.push((x, 0));
        }
        for (x, mut k) in chunks {
            // A name may still follow a `Name:` label on a noisy line.
            if sig_noise(x) && !NAME_LABEL.iter().any(|p| match_at(&x.toks, 0, p).is_some()) {
                continue;
            }
            while k < x.toks.len() && x.toks[k].kind == TokenKind::Punct {
                k += 1;
            }
            if let Some(p) = NAME_LABEL.iter().find_map(|p| match_at(&x.toks, k, p)) {
                k = p;
                while k < x.toks.len() && x.toks[k].kind == TokenKind::Punct {
                    k += 1;
                }
                if name.is_none()
                    && let Some(ne) = name_at(doc, x, k, NOT_NAME)
                {
                    name = Some(text_of(doc, x, k, ne));
                    end = end.max(x.toks[ne].end);
                }
                continue;
            }
            if let Some(p) = TITLE_LABEL.iter().find_map(|p| match_at(&x.toks, k, p)) {
                k = p;
                while k < x.toks.len() && x.toks[k].kind == TokenKind::Punct {
                    k += 1;
                }
                if title.is_none() && k < x.toks.len() {
                    title = Some(text_of(doc, x, k, x.toks.len() - 1));
                    end = end.max(x.span.end);
                }
                continue;
            }
            if k >= x.toks.len() {
                continue;
            }
            if is_title(&x.toks[k]) {
                let mut j = k;
                while j < x.toks.len() && is_title(&x.toks[j]) {
                    j += 1;
                }
                if title.is_none() {
                    title = Some(text_of(doc, x, k, j - 1));
                    end = end.max(x.toks[j - 1].end);
                }
                k = j;
                if k < x.toks.len() && x.toks[k].key == "," {
                    k += 1;
                }
            }
            if name.is_none()
                && k < x.toks.len()
                && let Some(ne) = name_at(doc, x, k, NOT_NAME)
            {
                name = Some(text_of(doc, x, k, ne));
                end = end.max(x.toks[ne].end);
                let mut j = ne + 1;
                if j < x.toks.len() && x.toks[j].key == "," {
                    j += 1;
                }
                if title.is_none() && j < x.toks.len() && is_title(&x.toks[j]) {
                    let mut e = j;
                    while e < x.toks.len() && is_title(&x.toks[e]) {
                        e += 1;
                    }
                    title = Some(text_of(doc, x, j, e - 1));
                    end = end.max(x.toks[e - 1].end);
                }
            }
        }
        if name.is_none() && title.is_none() {
            continue;
        }
        let base = if name.is_some() { 0.75 } else { 0.6 };
        out.push(Found {
            key: keys::SIGNATORY,
            value: rec(vec![
                ("role", t(sd.as_str())),
                ("label", t(label)),
                ("name", name.map(Value::text)),
                ("title", title.map(Value::text)),
            ]),
            conf: conf(l, base),
            page: l.page,
            span: Span {
                start: l.toks[0].start,
                end,
            },
        });
    }
}

/// Every party fact of the document.
pub(crate) fn scan(inp: &Input<'_>, out: &mut Vec<Found>) {
    let doc = inp.doc;
    // Defined terms in sentences.
    for s in inp.sents {
        let marks = define_marks(s);
        let mut from = 0;
        for m in &marks {
            // The name must sit before the role mark.
            let named = Segment {
                seg: s,
                a: from,
                b: m.seg_end,
                side: m.side,
                label: raw(doc, s, &s.toks[m.role]).to_owned(),
                role_tok: m.role,
            };
            let company = company_in(inp, &named);
            let full = Segment {
                b: m.tail_end.max(m.seg_end),
                ..named
            };
            if company.is_some() || has_detail(&full) {
                segment(inp, &full, None, out);
            }
            from = m.tail_end;
        }
    }
    // Label lines and the detail lines under them.
    let lines = inp.lines;
    let mut li = 0;
    while li < lines.len() {
        let l = &lines[li];
        let Some((sd, role_tok, after)) = label_line(doc, l) else {
            li += 1;
            continue;
        };
        let mut n = 1;
        while li + n < lines.len() && n < 8 {
            let x = &lines[li + n];
            if x.pos != l.pos || label_line(doc, x).is_some() {
                break;
            }
            if !DETAIL_START.contains(&x.toks[0].key.as_str()) {
                break;
            }
            n += 1;
        }
        let block = join(&lines[li..li + n]);
        let seg = Segment {
            seg: &block,
            a: 0,
            b: block.toks.len(),
            side: sd,
            label: raw(doc, l, &l.toks[role_tok]).to_owned(),
            role_tok,
        };
        let head = Segment {
            b: l.toks.len(),
            ..seg.clone()
        };
        let fallback = if company_in(inp, &head).is_none() {
            label_fallback(inp, l, after)
        } else {
            None
        };
        segment(inp, &seg, fallback.as_ref(), out);
        li += n;
    }
    signatories(inp, out);
}

/// Whether a segment holds an identifier, address or representative cue.
fn has_detail(s: &Segment<'_>) -> bool {
    let w = &s.seg.toks[s.a..s.b];
    !find(w, IDS).is_empty() || !find(w, REP).is_empty() || !find(w, ADDRESS).is_empty()
}

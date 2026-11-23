//! End-to-end tests on documents built in code. Every company, person and address here is
//! invented.

use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, Artifacts, Blob, Block, BlockKind, Ctx, Document, Emitter, Evidence, Fact, Format,
    Limits, Page, PageKind, Profile, Span, Value, Zone,
};
use quail_parties::{PartyCandidates, keys};

fn page_of(index: u32, parts: &[(&str, Zone)]) -> Page {
    let mut text = String::new();
    let mut blocks = Vec::new();
    for (t, z) in parts {
        if !text.is_empty() {
            text.push('\n');
        }
        let start = u32::try_from(text.len()).unwrap();
        text.push_str(t);
        blocks.push(Block::new(
            Span {
                start,
                end: u32::try_from(text.len()).unwrap(),
            },
            BlockKind::Paragraph,
            *z,
            None,
        ));
    }
    Page {
        text,
        blocks,
        ..Page::new(index, PageKind::Logical)
    }
}

/// Pages without zones (as a PDF text layer arrives).
fn plain(pages: &[&str]) -> Document {
    Document {
        pages: pages
            .iter()
            .enumerate()
            .map(|(i, t)| Page {
                text: (*t).to_owned(),
                ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
            })
            .collect(),
    }
}

fn run_with(doc: Document, limits: &Limits, deadline: Option<Instant>) -> (Vec<Fact>, Vec<String>) {
    let mut artifacts = Artifacts::new();
    artifacts.insert(doc);
    let blob = Blob::from(Vec::new());
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://test",
        artifacts: &artifacts,
        facts: &[],
        limits,
        profile: Profile::Fast,
    };
    let mut out = match deadline {
        Some(d) => Emitter::with_deadline(PartyCandidates::ID, d),
        None => Emitter::new(PartyCandidates::ID),
    };
    PartyCandidates.run(&ctx, &mut out).unwrap();
    let problems = out.take_problems();
    (out.into_parts().0, problems)
}

fn run(doc: Document) -> Vec<Fact> {
    run_with(doc, &Limits::default(), None).0
}

fn text(v: &Value) -> &str {
    match v {
        Value::Text(t) => t,
        _ => "",
    }
}

/// One emitted mention, joined on its evidence.
#[derive(Debug)]
struct M {
    surface: String,
    key: String,
    conf: f32,
    roles: Vec<String>,
    term: Option<String>,
    form: Option<String>,
    jurisdiction: Option<String>,
    evidence: Evidence,
}

fn mentions(facts: &[Fact]) -> Vec<M> {
    let field = |v: &Value, k: &str| v.field(k).and_then(Value::as_text).map(str::to_owned);
    facts
        .iter()
        .filter(|f| f.key == keys::COMPANY)
        .map(|c| M {
            surface: field(&c.value, "surface").unwrap(),
            key: field(&c.value, "key").unwrap(),
            conf: c.conf,
            roles: match c.value.field("roles") {
                Some(Value::List(r)) => r.iter().map(|x| text(x).to_owned()).collect(),
                _ => Vec::new(),
            },
            term: field(&c.value, "defined_term"),
            form: field(&c.value, "legal_form"),
            jurisdiction: field(&c.value, "jurisdiction"),
            evidence: c.evidence.clone().unwrap(),
        })
        .collect()
}

fn by_key<'a>(ms: &'a [M], key: &str) -> Vec<&'a M> {
    ms.iter().filter(|m| m.key == key).collect()
}

fn has_role(ms: &[M], key: &str, role: &str) -> bool {
    by_key(ms, key)
        .iter()
        .any(|m| m.roles.iter().any(|r| r == role))
}

fn keys_of(ms: &[M]) -> Vec<&str> {
    let mut k: Vec<&str> = ms.iter().map(|m| m.key.as_str()).collect();
    k.sort_unstable();
    k.dedup();
    k
}

const SR_LATIN: &str = "UGOVOR O PRUŽANJU USLUGA\n\
Zaključen u Beogradu dana 12.03.2024. godine između:\n\
1. ALFATEK d.o.o. Beograd, sa sedištem u Beogradu, ulica Nemanjina 4, matični broj 12345678, PIB 100200300, koga zastupa direktor (u daljem tekstu: Naručilac)\n\
i\n\
2. Betasoft DOO, Novi Sad, sa sedištem u Novom Sadu, Bulevar oslobođenja 10 (u daljem tekstu: Dobavljač)\n\
Član 1.\n\
Predmet ovog ugovora je pružanje usluga održavanja softvera.";

#[test]
fn serbian_latin_parties_clause() {
    let ms = mentions(&run(plain(&[SR_LATIN])));
    assert_eq!(keys_of(&ms), ["alfatek", "betasoft"], "{ms:#?}");
    let a = by_key(&ms, "alfatek")[0];
    assert_eq!(a.surface, "ALFATEK d.o.o. Beograd");
    assert_eq!(a.form.as_deref(), Some("d.o.o."));
    assert_eq!(a.jurisdiction.as_deref(), Some("rs"));
    assert_eq!(a.term.as_deref(), Some("Naručilac"));
    assert!(has_role(&ms, "alfatek", "customer"));
    assert!(has_role(&ms, "betasoft", "supplier"));
    assert_eq!(
        by_key(&ms, "betasoft")[0].term.as_deref(),
        Some("Dobavljač")
    );
    assert!(a.conf >= 0.9, "{}", a.conf);
}

#[test]
fn serbian_cyrillic_parties_clause() {
    let t = "УГОВОР\nЗакључен између:\nГАМА ПРОЈЕКТ д.о.о. Београд, са седиштем у Београду, ПИБ 100200301 (у даљем тексту: Наручилац)\nи\nДЕЛТА ИНЖЕЊЕРИНГ ДОО (у даљем тексту: Извршилац)";
    let ms = mentions(&run(plain(&[t])));
    assert_eq!(
        keys_of(&ms),
        ["delta inzenjering", "gama projekt"],
        "{ms:#?}"
    );
    assert!(has_role(&ms, "gama projekt", "customer"));
    assert!(has_role(&ms, "delta inzenjering", "supplier"));
    let g = by_key(&ms, "gama projekt")[0];
    assert_eq!(g.jurisdiction.as_deref(), Some("rs"));
    assert_eq!(g.surface, "ГАМА ПРОЈЕКТ д.о.о. Београд");
}

#[test]
fn english_by_and_between() {
    let t = "MASTER SERVICES AGREEMENT\nThis Master Services Agreement is entered into by and between Northwind Analytics Inc., a Delaware corporation with its principal office at 1 Example Plaza (the \"Customer\"), and Bluefin Data Ltd, a company registered in England (hereinafter \"Supplier\").";
    let ms = mentions(&run(plain(&[t])));
    assert_eq!(
        keys_of(&ms),
        ["bluefin data", "northwind analytics"],
        "{ms:#?}"
    );
    assert!(has_role(&ms, "northwind analytics", "customer"));
    assert!(has_role(&ms, "bluefin data", "supplier"));
    let n = by_key(&ms, "northwind analytics")[0];
    assert_eq!(n.surface, "Northwind Analytics Inc.");
    assert_eq!(n.jurisdiction.as_deref(), Some("us|ca"));
}

#[test]
fn letterhead_and_bill_to_ship_to() {
    let t = "KAPPA SYSTEMS GmbH\nHauptstrasse 5, 10115 Berlin\n\nINVOICE No. 2024-17\n\nBill to:\nOmega Retail LLC\n55 Sample Road\nShip to: Omega Warehouse Services LLC\n\nItem Qty Price\nConsulting 10 100.00\nTotal 1000.00";
    let ms = mentions(&run(plain(&[t])));
    assert_eq!(
        keys_of(&ms),
        ["kappa systems", "omega retail", "omega warehouse services"],
        "{ms:#?}"
    );
    assert!(has_role(&ms, "kappa systems", "issuer"));
    assert_eq!(by_key(&ms, "kappa systems")[0].evidence.zone, Zone::Header);
    assert!(has_role(&ms, "omega retail", "bill_to"));
    assert!(!has_role(&ms, "omega retail", "issuer"));
    assert!(has_role(&ms, "omega warehouse services", "ship_to"));
    let k = by_key(&ms, "kappa systems")[0];
    let o = by_key(&ms, "omega retail")[0];
    assert!(k.conf > 0.8 && o.conf > 0.8);
}

#[test]
fn labels_without_a_legal_form() {
    let t = "PONUDA\nDobavljač: Sunčani Sistemi Plus\nKupac: Petar Petrović\nNaručilac: PETAR PETROVIĆ\nVendor: Acme Robotics Works\nCustomer: the Customer";
    let ms = mentions(&run(plain(&["x\nx\nx\nx\nx\nx\nx", t])));
    // A formless name is taken after a label; a person's shape and a role word are not.
    assert_eq!(
        keys_of(&ms),
        ["acme robotics works", "suncani sistemi plus"],
        "{ms:#?}"
    );
    assert!(has_role(&ms, "suncani sistemi plus", "supplier"));
    assert!(has_role(&ms, "acme robotics works", "supplier"));
    assert!(ms.iter().all(|m| m.conf < 0.7));
}

#[test]
fn docx_zones_are_used() {
    let doc = Document {
        pages: vec![page_of(
            0,
            &[
                ("Delta Consulting d.o.o.", Zone::Header),
                ("Ugovor o saradnji", Zone::Title),
                (
                    "Predmet ugovora je saradnja sa firmom Omikron Trade d.o.o. na projektu.",
                    Zone::Body,
                ),
                (
                    "Delta Consulting d.o.o., Kralja Petra 1, Beograd",
                    Zone::Footer,
                ),
            ],
        )],
    };
    let ms = mentions(&run(doc));
    let d = by_key(&ms, "delta consulting");
    assert_eq!(d.len(), 2, "{ms:#?}");
    assert!(d.iter().any(|m| m.evidence.zone == Zone::Header));
    assert!(d.iter().any(|m| m.evidence.zone == Zone::Footer));
    let o = by_key(&ms, "omikron trade")[0];
    assert_eq!(o.evidence.zone, Zone::Body);
    assert_eq!(o.roles, ["mentioned"]);
    assert!(d[0].conf > o.conf);
}

#[test]
fn signature_block_titles_without_names() {
    let p1 = "This Agreement is made between Northwind Analytics Inc. (\"Customer\") and Bluefin Data Ltd (\"Supplier\").";
    let p2 = "IN WITNESS WHEREOF the parties have executed this Agreement.\nFor and on behalf of Northwind Analytics Inc.\nBy: ________\nName: John Example\nTitle: Managing Director\n\nZa Dobavljača\nBetasoft DOO\nDirektor\nMarko Primerović";
    let facts = run(plain(&[p1, p2]));
    let ms = mentions(&facts);
    let sig: Vec<&M> = ms.iter().filter(|m| m.evidence.page == Some(1)).collect();
    assert!(
        sig.iter()
            .any(|m| m.key == "northwind analytics" && m.roles.iter().any(|r| r == "signatory")),
        "{ms:#?}"
    );
    assert!(sig.iter().all(|m| m.evidence.zone == Zone::Signature));
    let titles: Vec<&str> = facts
        .iter()
        .filter(|f| f.key == keys::SIGNATORY_TITLE)
        .map(|f| text(&f.value))
        .collect();
    assert!(
        titles.contains(&"Managing Director") && titles.contains(&"Direktor"),
        "{titles:?}"
    );
    let count = facts
        .iter()
        .find(|f| f.key == keys::SIGNATORY_COUNT)
        .map(|f| f.value.clone());
    assert!(matches!(count, Some(Value::Int(n)) if n >= 2), "{count:?}");
    // No fact carries a person's name.
    for f in &facts {
        let s = format!("{:?}", f.value);
        assert!(!s.contains("John") && !s.contains("Marko"), "{s}");
    }
}

#[test]
fn name_statements() {
    let t = "Zeta Labs Ltd (formerly Old Zeta Research Ltd) shall deliver.\nAcme Holdings LLC d/b/a Rocket Foods agrees.\nSigma Trade d.o.o. (prethodno: Sigma Commerce d.o.o.) je dobavljač.\nNova Mreža d.o.o., now known as Mreža Plus d.o.o., pays.";
    let facts = run(plain(&["x\nx\nx\nx\nx\nx\nx", t]));
    let pairs: Vec<Vec<String>> = facts
        .iter()
        .filter(|f| f.key == keys::ALIAS_OF)
        .map(|f| {
            ["current", "other", "kind"]
                .iter()
                .map(|k| {
                    f.value
                        .field(k)
                        .and_then(Value::as_text)
                        .unwrap_or("")
                        .to_owned()
                })
                .collect()
        })
        .collect();
    let want = [
        ["Zeta Labs Ltd", "Old Zeta Research Ltd", "former_name"],
        ["Acme Holdings LLC", "Rocket Foods", "dba"],
        ["Sigma Trade d.o.o.", "Sigma Commerce d.o.o.", "former_name"],
        ["Mreža Plus d.o.o.", "Nova Mreža d.o.o.", "former_name"],
    ];
    for w in want {
        assert!(pairs.iter().any(|p| p == &w), "{w:?} not in {pairs:?}");
    }
    // The statement's evidence covers both names.
    let ev = facts
        .iter()
        .find(|f| f.key == keys::ALIAS_OF)
        .unwrap()
        .evidence
        .clone()
        .unwrap();
    let span = ev.span.unwrap();
    let src = &plain(&["x\nx\nx\nx\nx\nx\nx", t]).pages[1].text;
    assert_eq!(
        span.slice(src),
        "Zeta Labs Ltd (formerly Old Zeta Research Ltd"
    );
    let ms = mentions(&facts);
    assert!(!by_key(&ms, "rocket foods").is_empty());
}

#[test]
fn false_positive_traps() {
    let t = "x\nx\nx\nx\nx\nx\nx\n\
Republika Srbija, Ministarstvo finansija\n\
Adresa: Bulevar Mihajla Pupina 10, 11000 Beograd\n\
Supplier: the Supplier\n\
Dobavljač:\n\
Microsoft Office 365 License\n\
Ship to: Warehouse 5, 123 Main Street\n\
5. LIMITED WARRANTY\n\
SUCH COMPANY SHALL NOT\n\
The Company shall pay as follows.\n\
Ltd\n\
the parties agree that the supplier and the customer\n\
Naručilac i Dobavljač su saglasni.";
    let ms = mentions(&run(plain(&[t])));
    assert!(ms.is_empty(), "{ms:#?}");
}

#[test]
fn english_name_shapes() {
    let t = "x\nx\nx\nx\nx\nx\nx\n\
The Hershey Example Company agrees with Build-A-Bear Sample, Inc. and L.E.K. Sample Consulting LLC.\n\
Brenwick, Dalton and Company shall deliver.\n\
Samplecorp, a Florida Corporation, and Article 12 Limited apply.\n\
Promptly notify the Company, Section 5 Company.";
    let ms = mentions(&run(plain(&[t])));
    let ks = keys_of(&ms);
    assert_eq!(
        ks,
        [
            "brenwick dalton",
            "buildabear sample",
            "hershey example",
            "lek sample consulting"
        ],
        "{ms:#?}"
    );
}

#[test]
fn walk_fixes_on_english_names() {
    let t = "x\nx\nx\nx\nx\nx\nx\n\
This Agreement is made by and between\n\
ZentaMOS DEVICES INC., a company incorporated under the laws of Exland (\"ZentaMOS\"), and\n\
GALVEN (CHEMICALS) LIMITED of 4 Sample Street and 1234567 Exland Limited and 7Q Company\n\
and also by At Home Example Corporation and with AT&T Sample Corp. here.\n\
Then Quillon &\n\
Brack, Incorporated (\"Company\") and Yu Sample and GENEXOTE SAMPLES INC. agree,\n\
Velto ASP and Velto AB join, Norvin and Company too.\n\
columns V W X Y Z AA AB\n\
Licensee's Company, Brian J. Sample Company.";
    let ms = mentions(&run(plain(&[t])));
    let surfaces: Vec<&str> = ms.iter().map(|m| m.surface.as_str()).collect();
    for want in [
        "ZentaMOS DEVICES INC.",
        "GALVEN (CHEMICALS) LIMITED",
        "1234567 Exland Limited",
        "7Q Company",
        "At Home Example Corporation",
        "AT&T Sample Corp.",
        "Quillon & Brack, Incorporated",
        "GENEXOTE SAMPLES INC.",
        "Velto AB",
    ] {
        assert!(surfaces.contains(&want), "{want}: {surfaces:#?}");
    }
    for bad in [
        "Norvin and Company",
        "Licensee's Company",
        "Brian J. Sample Company",
    ] {
        assert!(!surfaces.contains(&bad), "{bad}: {surfaces:#?}");
    }
    assert!(
        surfaces.iter().all(|s| !s.ends_with("AA AB")),
        "{surfaces:#?}"
    );
}

#[test]
fn signature_blocks_filer_lines_and_columns() {
    let body = "This Agreement is entered into between Orvane Sample, Inc. (\"Orvane\") and \
        Pelko Example LLC (\"Pelko\"). ";
    let p0 = body.repeat(6);
    let p1 = "IN WITNESS WHEREOF the parties sign.\n\
Snap Sample          Ultra Example Airlines, Inc.\n\
By: /s/ Someone      By: /s/ Someone Else\n\
TRIVEX SAMPLE FUND LP\n\
By: Trivex Manager LLC, its general partner\n\
For and on behalf of\n\
Kestro Example S.A.\n\
Source: ORVANE SAMPLE INC, 10-K, 3/19/2007";
    let ms = mentions(&run(plain(&[&p0, p1])));
    let surfaces: Vec<&str> = ms.iter().map(|m| m.surface.as_str()).collect();
    assert!(
        surfaces.contains(&"Ultra Example Airlines, Inc."),
        "{surfaces:#?}"
    );
    assert!(
        !surfaces.iter().any(|s| s.starts_with("Snap Sample ")),
        "{surfaces:#?}"
    );
    assert!(has_role(&ms, "trivex sample fund", "signatory"), "{ms:#?}");
    assert!(has_role(&ms, "kestro example", "signatory"), "{ms:#?}");
    let filer = by_key(&ms, "orvane sample");
    assert!(
        filer
            .iter()
            .filter(|m| m.surface.starts_with("ORVANE"))
            .all(|m| m.conf <= 0.5 && !m.roles.iter().any(|r| r == "signatory")),
        "{filer:#?}"
    );
}

#[test]
fn code_and_product_traps() {
    let t = "x\nx\nx\nx\nx\nx\nx\n\
Part AV-W-AD replaced.\n\
HYPAK PR 1MLL PP NATURAL 100 pcs\n\
D1532C00057 - NCI A/B\n\
Bank: Example Bank plc, IBAN GB00 0000 0000 0000 0000 00";
    let ms = mentions(&run(plain(&[t])));
    assert!(ms.iter().all(|m| m.key == "example bank"), "{ms:#?}");
    assert!(
        ms.iter().all(|m| m.conf < 0.5 && m.roles == ["mentioned"]),
        "{ms:#?}"
    );
}

#[test]
fn brand_spelled_by_a_domain() {
    let t = "KESTRELONX\nQuotation 17\nContact: sales@kestrelonx.example.com, www.kestrelonx.com\nPrices valid 30 days.";
    let ms = mentions(&run(plain(&[t])));
    let h = by_key(&ms, "kestrelonx");
    assert!(!h.is_empty(), "{ms:#?}");
    assert!(has_role(&ms, "kestrelonx", "issuer"));
    // Without the domain the same word is nothing.
    let ms = mentions(&run(plain(&[
        "KESTRELONX\nQuotation 17\nPrices valid 30 days.",
    ])));
    assert!(ms.is_empty(), "{ms:#?}");
}

#[test]
fn spans_point_into_the_source_text() {
    // NBSP, a ligature and a soft line break before the name: spans must still cut the
    // name out of the page text as extracted.
    let t = "Ugovor\u{00a0}o\u{00a0}saradnji\nﬁrma:  Kraljević   d.o.o.\u{00a0}Beograd i ostali";
    let doc = plain(&["x\nx\nx\nx\nx\nx\nx", t]);
    let src = doc.pages[1].text.clone();
    let ms = mentions(&run(doc));
    let k = by_key(&ms, "kraljevic")[0];
    assert_eq!(k.surface, "Kraljević d.o.o. Beograd");
    assert_eq!(
        k.evidence.span.unwrap().slice(&src),
        "Kraljević   d.o.o.\u{00a0}Beograd"
    );
}

#[test]
fn prefix_and_infix_forms() {
    let t = "x\nx\nx\nx\nx\nx\nx\nIsporučilac je JP Elektromreža Srbije, Beograd.\nIzvođač radova: Marko Marković PR Molerski Radovi Cvet\nOOO Romashka Trade ist ein Kunde.\nAkcionarsko društvo Fabrika Sunce ima sedište u Nišu.";
    let ms = mentions(&run(plain(&[t])));
    let ks = keys_of(&ms);
    assert!(ks.contains(&"elektromreza srbije"), "{ms:#?}");
    assert!(
        ks.contains(&"marko markovic molerski radovi cvet"),
        "{ms:#?}"
    );
    assert!(ks.contains(&"romashka trade"), "{ms:#?}");
    assert!(ks.contains(&"fabrika sunce"), "{ms:#?}");
}

#[test]
fn many_mentions_are_capped_with_a_problem() {
    let mut t = String::new();
    for i in 0..400 {
        use std::fmt::Write as _;
        let _ = writeln!(t, "Firma{i} Alfa d.o.o.");
    }
    let (facts, problems) = run_with(plain(&[&t]), &Limits::default(), None);
    let companies = facts.iter().filter(|f| f.key == keys::COMPANY).count();
    assert!(companies <= quail_parties::MAX_KEYS * quail_parties::MAX_MENTIONS_PER_KEY);
    assert!(!problems.is_empty());
}

fn run_full(doc: Document) -> (Vec<Fact>, Vec<String>, Vec<String>) {
    let mut artifacts = Artifacts::new();
    artifacts.insert(doc);
    let blob = Blob::from(Vec::new());
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://test",
        artifacts: &artifacts,
        facts: &[],
        limits: &limits,
        profile: Profile::Fast,
    };
    let mut out = Emitter::new(PartyCandidates::ID);
    PartyCandidates.run(&ctx, &mut out).unwrap();
    let problems = out.take_problems();
    let (facts, _, warnings) = out.into_parts();
    (facts, warnings, problems)
}

/// `n` lines naming distinct companies, each line `"{prefix}Kompanija{i} Beta d.o.o."`.
fn names(prefix: &str, from: usize, n: usize) -> String {
    use std::fmt::Write as _;
    let mut t = String::new();
    for i in from..from + n {
        let _ = writeln!(t, "{prefix}Kompanija{i} Beta d.o.o.");
    }
    t
}

#[test]
fn key_cap_drops_weak_names_with_a_warning() {
    let opening = "Ugovor zaključen između Alfa Gradnja d.o.o. (\"Izvođač\") i Omega Trgovina d.o.o. (\"Naručilac\").";
    let later = names("", 0, 120);
    let (facts, warnings, problems) = run_full(plain(&[opening, "x", "x", &later]));
    assert!(problems.is_empty(), "{problems:?}");
    assert!(warnings.iter().any(|w| w.contains("weak")), "{warnings:?}");
    let ms = mentions(&facts);
    let ks = keys_of(&ms);
    assert_eq!(ks.len(), quail_parties::MAX_KEYS);
    assert!(ks.contains(&"alfa gradnja"), "{ks:?}");
    assert!(ks.contains(&"omega trgovina"), "{ks:?}");
}

#[test]
fn key_cap_is_a_problem_when_strong_names_are_dropped_or_input_is_pathological() {
    // More labelled suppliers than the cap: real candidates are lost.
    let t = names("Dobavljač: ", 0, 120);
    let (_, _, problems) = run_full(plain(&[&t]));
    assert!(
        problems.iter().any(|p| p.contains("candidate companies")),
        "{problems:?}"
    );
    // A list of names, not a contract: always a problem.
    let pages: Vec<String> = (0..4).map(|p| names("", p * 150, 150)).collect();
    let refs: Vec<&str> = ["x", "x", "x"]
        .into_iter()
        .chain(pages.iter().map(String::as_str))
        .collect();
    let (_, _, problems) = run_full(plain(&refs));
    assert!(
        problems.iter().any(|p| p.contains("candidate companies")),
        "{problems:?}"
    );
}

#[test]
fn hostile_input_is_fast_and_safe() {
    // Garbage, long runs of forms and capitalised words, odd Unicode.
    let mut t = String::new();
    for i in 0..20_000u32 {
        let c = char::from_u32(0x20 + (i * 7919) % 0x2fff).unwrap_or(' ');
        t.push(c);
        if i % 13 == 0 {
            t.push_str(" d.o.o. ");
        }
        if i % 17 == 0 {
            t.push_str(" Ltd (formerly X Y Z) ( u daljem tekstu: ");
        }
    }
    let big = "Alfa Beta Gama ".repeat(60_000) + " d.o.o.";
    let start = Instant::now();
    let _ = run(plain(&[&t, &big, "", "\u{feff}\u{200b}"]));
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn deadline_stops_with_a_problem() {
    let (_, problems) = run_with(
        plain(&["ALFATEK d.o.o."; 5]),
        &Limits::default(),
        Some(Instant::now()),
    );
    assert!(problems.iter().any(|p| p.contains("deadline")));
}

#[test]
fn text_limit_is_respected() {
    let limits = Limits {
        max_text_bytes: 10,
        ..Limits::default()
    };
    let (facts, problems) = run_with(
        plain(&["x", "ALFATEK d.o.o. Beograd sa sedištem"]),
        &limits,
        None,
    );
    assert!(facts.iter().all(|f| f.key != keys::COMPANY));
    assert!(!problems.is_empty());
}

#[test]
fn ranked_candidates() {
    let facts = run(plain(&[SR_LATIN]));
    let ranked = facts.iter().find(|f| f.key == keys::CANDIDATES).unwrap();
    let Value::List(items) = &ranked.value else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    let json = serde_json::to_string(&ranked.value).unwrap();
    assert!(json.contains("alfatek") && json.contains("betasoft"));
}

#[test]
fn mentioned_only_mentions_are_less_sure() {
    let p1 = "x\nx\nx\nx\nx\nx\nx\n\
This Agreement is entered into by and between Northwind Analytics Inc. (the \"Customer\") and Bluefin Data Ltd (the \"Supplier\").";
    let p2 = "x\nx\nx\nx\nx\nx\nx\n\
The Supplier may use the services of Kestrel Hosting LLC for storage.\nx\nx\nx\nx\nx\nx\nx\nx";
    let ms = mentions(&run(plain(&[p1, p2])));
    let party = by_key(&ms, "northwind analytics")[0].conf;
    let other = by_key(&ms, "kestrel hosting")[0];
    assert_eq!(other.roles, ["mentioned"]);
    assert!(other.conf < 0.3, "{}", other.conf);
    assert!(party > other.conf + 0.4, "{party} vs {}", other.conf);
}

#[test]
fn name_walk_stops_at_recitals_titles_and_signatories() {
    let t = "x\nx\nx\nx\nx\nx\nx\n\
WHEREAS, Tarasample Acquisition Corp. wishes to proceed.\n\
JOINDER OF NORTHSAMPLE HOLDINGS, INC.\n\
Board of Directors of Westsample Corporation\n\
Vice President ARCSAMPLE COMMUNICATIONS, INC.\n\
Mark G. Parker NIKESAMPLE, Inc.\n\
Return of Company\n\
HOLDING CO.";
    let ms = mentions(&run(plain(&[t])));
    let mut surfaces: Vec<&str> = ms.iter().map(|m| m.surface.as_str()).collect();
    surfaces.sort_unstable();
    surfaces.dedup();
    assert_eq!(
        surfaces,
        [
            "ARCSAMPLE COMMUNICATIONS, INC.",
            "NIKESAMPLE, Inc.",
            "NORTHSAMPLE HOLDINGS, INC.",
            "Tarasample Acquisition Corp.",
            "Westsample Corporation",
        ],
        "{ms:#?}"
    );
}

#[test]
fn names_wrapped_over_a_paragraph_line() {
    let t = "x\nx\nx\nx\nx\nx\nx\n\
This Confidentiality Agreement is made by and between Vertexsample\n\
Pharmaceuticals Incorporated, a Massachusetts corporation, and you.\n\
CONFIDENTIAL\n\
Acme Sample Inc.";
    let ms = mentions(&run(plain(&[t])));
    assert_eq!(
        keys_of(&ms),
        ["acme sample", "vertexsample pharmaceuticals"],
        "{ms:#?}"
    );
    let v = by_key(&ms, "vertexsample pharmaceuticals")[0];
    assert!(!v.surface.contains('\n'), "{}", v.surface);
}

/// Facts of `party.candidates` over `doc`, with the `normalized_text` artifact when `shared`.
fn facts_with_shared(doc: &Document, limits: &Limits, shared: bool) -> Vec<Fact> {
    let mut artifacts = Artifacts::new();
    artifacts.insert(doc.clone());
    if shared {
        artifacts.insert(quail_text::NormalizedText::of(doc));
    }
    let blob = Blob::from(Vec::new());
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://test",
        artifacts: &artifacts,
        facts: &[],
        limits,
        profile: Profile::Fast,
    };
    let mut out = Emitter::new(PartyCandidates::ID);
    PartyCandidates.run(&ctx, &mut out).unwrap();
    out.into_parts().0
}

#[test]
fn same_facts_with_the_shared_artifact() {
    let mut doc = plain(&[
        "SERVICES AGREEMENT\nThis Agreement is made between Brenwick Dalton Ltd., a company \
         incorporated in England (\u{201c}Supplier\u{201d}), and Ostvale Pharma GmbH \
         (the \u{201c}Customer\u{201d}).",
        "",
        "Kupac: Zelenkor d.o.o. Beo-\ngrad, PIB 100000001 — prodavac „Marvic Trade“ AD.\n\
         formerly known as Marvic Holdings plc; www.marvic.example",
        "IN WITNESS WHEREOF\nBRENWICK DALTON LTD.\nBy: ____\nName: Ana Petrović\nTitle: Director",
        // The rules of party.candidates@3: column gaps, wrapped forms, filer lines.
        "Snap Sample\u{a0}\u{a0}        Ultra Example Airlines, Inc.\nBy: /s/ A      By: /s/ B\n\
         For and on behalf of\nKestro Example S.A.\nmade by At Home Example Corporation and\n\
         Sibannac Sample,\nInc. and 1234567 Exland Limited\n\
         Source: ORVANE SAMPLE INC, 10-K, 3/19/2007",
    ]);
    doc.pages.push(page_of(
        5,
        &[
            ("Ostvale Pharma GmbH", Zone::Header),
            ("Signed for Ostvale Pharma GmbH", Zone::Body),
        ],
    ));
    let all = facts_with_shared(&doc, &Limits::default(), false);
    assert!(!all.is_empty());
    assert_eq!(facts_with_shared(&doc, &Limits::default(), true), all);
    // A limit that cuts page 2: that page is normalized by the analyzer itself.
    let cut = Limits {
        max_text_bytes: 260,
        ..Limits::default()
    };
    assert_eq!(
        facts_with_shared(&doc, &cut, true),
        facts_with_shared(&doc, &cut, false)
    );
    assert!(PartyCandidates.after().contains(&"normalized_text"));
}

#[test]
fn serbian_procurement_labels_over_sentence_case_names() {
    // A framework-agreement annex: the buyer, a public institution written in sentence case,
    // is on the line under its label; the title line has the preposition "СА", not an S.A.
    let doc = plain(&["НАРУЧИЛАЦ:\n\
         Републички фонд за здравствено осигурање, ул. Цветна 12, Београд\n\
         заступник: Петар Петровић\n\
         Матични број: 01234567\n\
         (у даљем тексту: Наручилац)\n\
         ДОБАВЉАЧ:\n\
         Лековита Биљка д.о.о., ул. Липова бр. 7, Ниш\n\
         (у даљем тексту: Добављач)\n\
         АНЕКС ОКВИРНОГ СПОРАЗУМА БР. 1-2/25\n\
         ЗА ЈАВНУ НАБАВКУ ОПРЕМЕ СА УГРАДЊОМ\n\
         Наручилац и Добављач су сагласни са изменом."]);
    let ms = mentions(&run(doc));
    let keys = keys_of(&ms);
    assert!(
        has_role(&ms, "republicki fond za zdravstveno osiguranje", "customer"),
        "{ms:#?}"
    );
    assert!(has_role(&ms, "lekovita biljka", "supplier"), "{keys:?}");
    assert!(
        !keys
            .iter()
            .any(|k| k.contains("nabavku") || k.contains("opreme")),
        "{keys:?}"
    );
    // Latin script: the same label shape, and a Latin S.A. is still a form.
    let doc = plain(&["Naručilac:\nGradska uprava za imovinu, Trg Slobode 1\n\
         Dobavljač: Acme Mlin S.A."]);
    let ms = mentions(&run(doc));
    assert!(
        has_role(&ms, "gradska uprava za imovinu", "customer"),
        "{ms:#?}"
    );
    let acme = by_key(&ms, "acme mlin");
    assert!(
        acme.iter().any(|m| m.form.as_deref() == Some("S.A.")),
        "{ms:#?}"
    );
}

#[test]
fn a_label_over_running_text_is_no_party() {
    // Sentence case after an English label, or a sentence after a Serbian one: no name.
    let doc = plain(&["Supplier:\nSame as above, see page 2\n\
         Dobavljač:\nDostavlja robu u roku od 30 dana od dana prijema narudžbenice.\n\
         НАРУЧИЛАЦ\nДОБАВЉАЧ\nРепублички фонд за\nздравствено осигурање"]);
    let ms = mentions(&run(doc));
    assert!(ms.is_empty(), "{ms:#?}");
}

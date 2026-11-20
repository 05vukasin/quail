//! End-to-end tests of `signal.scan` on text built in code. Identifiers get their check
//! digits computed here, so no real number appears in the repository.

use quail_core::{
    Analyzer, Artifacts, BBox, Blob, Block, BlockKind, Ctx, Date, Document, Emitter, Fact, FactKey,
    Format, Limits, Money, Page, PageKind, Profile, Span, TextSource, Value, Zone,
};
use std::fmt::Write as _;

use quail_signals::{SignalScan, checksum, keys};

fn doc(pages: &[&str]) -> Document {
    Document {
        pages: pages
            .iter()
            .enumerate()
            .map(|(i, t)| Page {
                text: (*t).to_owned(),
                text_source: TextSource::Native,
                ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
            })
            .collect(),
    }
}

fn run_doc(d: Document, limits: &Limits) -> (Vec<Fact>, Vec<String>) {
    let mut artifacts = Artifacts::new();
    artifacts.insert(d);
    let blob = Blob::from(Vec::new());
    let ctx = Ctx {
        blob: &blob,
        format: Format::Text,
        locator: "test",
        artifacts: &artifacts,
        facts: &[],
        limits,
        profile: Profile::Fast,
    };
    let mut out = Emitter::new(SignalScan::ID);
    SignalScan.run(&ctx, &mut out).unwrap();
    let problems = out.take_problems();
    (out.facts().to_vec(), problems)
}

fn run(text: &str) -> Vec<Fact> {
    run_doc(doc(&[text]), &Limits::default()).0
}

fn values(facts: &[Fact], key: &FactKey) -> Vec<Value> {
    facts
        .iter()
        .filter(|f| &f.key == key)
        .map(|f| f.value.clone())
        .collect()
}

fn texts(facts: &[Fact], key: &FactKey) -> Vec<String> {
    values(facts, key)
        .into_iter()
        .map(|v| match v {
            Value::Text(t) => t.into(),
            other => format!("{other:?}"),
        })
        .collect()
}

fn conf_of(facts: &[Fact], key: &FactKey) -> f32 {
    facts.iter().find(|f| &f.key == key).map_or(0.0, |f| f.conf)
}

/// Completes `base` with the one digit that makes `valid` true.
fn complete(base: &str, valid: fn(&str) -> bool) -> String {
    (0..10)
        .map(|d| format!("{base}{d}"))
        .find(|s| valid(s))
        .expect("a check digit exists")
}

/// Same, but returns a well-formed number with a wrong check digit.
fn wrong(base: &str, valid: fn(&str) -> bool) -> String {
    (0..10)
        .map(|d| format!("{base}{d}"))
        .find(|s| !valid(s))
        .unwrap()
}

fn pib() -> String {
    complete("10765432", checksum::rs_pib)
}

fn mb() -> String {
    complete("1765432", checksum::rs_mb)
}

fn iban_rs() -> String {
    let bban = "265100000012345678";
    for k in 2..=98 {
        let s = format!("RS{k:02}{bban}");
        if checksum::iban(&s) {
            return s;
        }
    }
    unreachable!()
}

fn spaced(s: &str) -> String {
    s.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn pib_and_mb_with_latin_and_cyrillic_labels() {
    let (p, m) = (pib(), mb());
    for text in [
        format!("PIB: {p}, MB: {m}"),
        format!("ПИБ: {p} Матични број: {m}"),
        format!("Poreski identifikacioni broj {p}\nMatični broj {m}"),
        format!("PIB/MB: {p}/{m}"),
        format!("PIB\n{p}\nMat. br.\n{m}"),
    ] {
        let f = run(&text);
        assert_eq!(texts(&f, &keys::RS_PIB), std::slice::from_ref(&p), "{text}");
        assert_eq!(texts(&f, &keys::RS_MB), std::slice::from_ref(&m), "{text}");
        assert!(conf_of(&f, &keys::RS_PIB) >= 0.95);
    }
}

#[test]
fn invalid_checksum_is_low_conf_with_label_and_dropped_without() {
    let bad = wrong("10765432", checksum::rs_pib);
    let f = run(&format!("PIB: {bad}"));
    assert_eq!(texts(&f, &keys::RS_PIB), std::slice::from_ref(&bad));
    assert!(conf_of(&f, &keys::RS_PIB) <= 0.25);
    let f = run(&format!("broj {bad} ugovor"));
    assert!(texts(&f, &keys::RS_PIB).is_empty());
}

#[test]
fn bare_numbers_stay_low_or_absent() {
    let p = pib();
    // English page: a bare 9-digit number is not a PIB.
    assert!(texts(&run(&format!("Order {p} shipped")), &keys::RS_PIB).is_empty());
    // Serbian page: kept, at low confidence.
    let f = run(&format!("Ugovor o nabavci, broj {p}"));
    assert_eq!(texts(&f, &keys::RS_PIB), [p]);
    assert!(conf_of(&f, &keys::RS_PIB) <= 0.3);
    // An 8-digit number without its label is never an MB.
    assert!(texts(&run(&format!("Ugovor {}", mb())), &keys::RS_MB).is_empty());
}

#[test]
fn phone_pib_and_account_are_told_apart() {
    let p = pib();
    let head = "2650000000123456";
    let acct = (0..100)
        .map(|k| format!("{head}{k:02}"))
        .find(|s| checksum::rs_account(s))
        .unwrap();
    let dashed = format!("{}-{}-{}", &acct[..3], &acct[3..16], &acct[16..]);
    let short = format!(
        "{}-{}-{}",
        &acct[..3],
        acct[3..16].trim_start_matches('0'),
        &acct[16..]
    );
    let text = format!(
        "PIB {p}\nTel: 011/123-4567, +381 64 123 4567\nTekući račun: {short}\nBroj strane 5 od 12, godina 2023."
    );
    let f = run(&text);
    assert_eq!(texts(&f, &keys::RS_PIB), [p]);
    assert_eq!(texts(&f, &keys::RS_ACCOUNT), [dashed]);
    let phones = texts(&f, &keys::PHONE);
    assert!(phones.contains(&"+381111234567".to_owned()), "{phones:?}");
    assert!(phones.contains(&"+381641234567".to_owned()), "{phones:?}");
    assert!(texts(&f, &keys::RS_MB).is_empty());
    assert!(
        values(&f, &keys::DATE).is_empty(),
        "page numbers and years are no dates"
    );
}

#[test]
fn iban_spaced_unspaced_and_invalid() {
    let i = iban_rs();
    for text in [
        format!("IBAN: {i}"),
        format!("IBAN {}", spaced(&i)),
        format!("na račun {i}."),
    ] {
        let f = run(&text);
        assert_eq!(texts(&f, &keys::IBAN), std::slice::from_ref(&i), "{text}");
    }
    // Same digits, other check digits.
    let bad = format!(
        "RS{:02}{}",
        (i[2..4].parse::<u32>().unwrap() + 1) % 100,
        &i[4..]
    );
    assert!(texts(&run(&format!("ref {bad}")), &keys::IBAN).is_empty());
    let f = run(&format!("IBAN: {bad}"));
    assert!(conf_of(&f, &keys::IBAN) <= 0.25);
}

#[test]
fn bic_needs_label_or_iban() {
    let i = iban_rs();
    let f = run(&format!("IBAN {i} SWIFT: AAAARSBG"));
    assert_eq!(texts(&f, &keys::BIC), ["AAAARSBG"]);
    let f = run("BIC/SWIFT code AAAADEFFXXX");
    assert_eq!(texts(&f, &keys::BIC), ["AAAADEFFXXX"]);
    // Capitalised words of BIC shape are no BICs without context.
    assert!(texts(&run("THE BUSINESS AGREEMENT"), &keys::BIC).is_empty());
}

#[test]
fn eu_vat_numbers() {
    let de = complete("13669597", checksum::mod_11_10);
    let hr = complete("1234567890", checksum::mod_11_10);
    let f = run(&format!(
        "USt-IdNr.: DE{de}\nVAT No: DE {de}\nATU04571784 / NL245732147B01 / IT13627010500\n\
         TVA FR40779211259, NIF ESB05258983, NIP: 710-421-90-52, DDV: SI59278951\n\
         VAT Reg No GB 717 2961 78, BE 0123.456.749? OIB: {hr}, PDV ID HR{hr}"
    ));
    let v = texts(&f, &keys::VAT);
    for want in [
        format!("DE{de}"),
        "ATU04571784".into(),
        "NL245732147B01".into(),
        "IT13627010500".into(),
        "FR40779211259".into(),
        "ESB05258983".into(),
        "PL7104219052".into(),
        "SI59278951".into(),
        "GB717296178".into(),
        format!("HR{hr}"),
    ] {
        assert!(v.contains(&want), "{want} missing from {v:?}");
    }
    assert_eq!(texts(&f, &keys::HR_OIB), [hr]);
    // Prefix words in running text are no VAT numbers.
    let f = run("PAYABLE AT 30 DAYS, IT IS 2023 AND DE 12");
    assert!(texts(&f, &keys::VAT).is_empty());
}

#[test]
fn lei_duns_ein() {
    // LEI: 18 characters + MOD 97-10 check digits computed here.
    let base = "5299000ABCDEFGHIJK";
    let lei = (0..100)
        .map(|k| format!("{base}{k:02}"))
        .find(|s| checksum::lei(s))
        .unwrap();
    let f = run(&format!(
        "LEI: {lei}\nD-U-N-S Number: 12-345-6789\nDUNS 123456789\n\
         Delaware 12-3456789\n(State) (I.R.S. Employer Identification No.)\nInvoice 98-7654321"
    ));
    assert_eq!(texts(&f, &keys::LEI), [lei]);
    assert_eq!(texts(&f, &keys::DUNS), ["123456789"]);
    assert_eq!(texts(&f, &keys::US_EIN), ["12-3456789"]);
}

#[test]
fn jmbg_is_counted_never_emitted() {
    let base = "010199071000";
    let j = complete(base, checksum::jmbg);
    let j2 = complete("150685571001", checksum::jmbg);
    let f = run(&format!(
        "JMBG: {j}, drugi {j2}, pogrešan {}",
        wrong(base, checksum::jmbg)
    ));
    assert_eq!(values(&f, &keys::JMBG_COUNT), [Value::Int(2)]);
    for fact in &f {
        let s = format!("{:?}", fact.value);
        assert!(
            !s.contains(&j) && !s.contains(&j2),
            "JMBG leaked in {fact:?}"
        );
        assert!(fact.key != keys::JMBG_COUNT || fact.evidence.is_none());
    }
}

fn date(y: i16, m: u8, d: u8) -> Value {
    Value::Date(Date { y, m, d })
}

#[test]
fn dates_in_all_forms() {
    for (text, want) in [
        ("dana 12.09.2023. godine", date(2023, 9, 12)),
        ("dana 12. 09. 2023. godine", date(2023, 9, 12)),
        ("2023-09-12", date(2023, 9, 12)),
        ("12. septembra 2023.", date(2023, 9, 12)),
        ("12. септембра 2023.", date(2023, 9, 12)),
        ("1. марта 2024. године", date(2024, 3, 1)),
        ("12 rujna 2023.", date(2023, 9, 12)),
        ("September 12, 2023", date(2023, 9, 12)),
        ("Sept. 12, 2023", date(2023, 9, 12)),
        ("12th of September 2023", date(2023, 9, 12)),
        ("this 1st day of March, 2024", date(2024, 3, 1)),
        ("25/09/2023", date(2023, 9, 25)),
        ("09/25/2023", date(2023, 9, 25)),
        ("29.02.2024", date(2024, 2, 29)),
    ] {
        assert_eq!(values(&run(text), &keys::DATE), [want], "{text}");
    }
    for text in [
        "29.02.2023",
        "32.01.2023",
        "12.13.2023",
        "Page 3 of 12",
        "in 2023",
        "1.200,50",
        "May 2023",
    ] {
        assert!(values(&run(text), &keys::DATE).is_empty(), "{text}");
    }
}

#[test]
fn ambiguous_slash_dates_follow_file_evidence() {
    // No evidence: day first, low confidence.
    let f = run("05/06/2023");
    assert_eq!(values(&f, &keys::DATE), [date(2023, 6, 5)]);
    assert!(conf_of(&f, &keys::DATE) < 0.7);
    // Month-first evidence elsewhere in the file.
    let f = run_doc(
        doc(&["Signed September 12, 2023; 09/25/2023", "05/06/2023"]),
        &Limits::default(),
    )
    .0;
    assert!(values(&f, &keys::DATE).contains(&date(2023, 5, 6)));
    // Day-first evidence.
    let f = run("12.09.2023 and 05/06/2023");
    assert!(values(&f, &keys::DATE).contains(&date(2023, 6, 5)));
}

#[test]
fn date_roles() {
    let f = run(
        "Effective Date: 01.02.2024\nDatum dospeća: 15.03.2024\nUgovor važi do 31.12.2025.\n\
         Invoice date 2024-01-31\nzaključen dana 10. januara 2024. godine\nValuta: 20.02.2024",
    );
    assert_eq!(values(&f, &keys::DATE_EFFECTIVE), [date(2024, 2, 1)]);
    assert_eq!(
        values(&f, &keys::DATE_DUE),
        [date(2024, 3, 15), date(2024, 2, 20)]
    );
    assert_eq!(values(&f, &keys::DATE_EXPIRY), [date(2025, 12, 31)]);
    assert_eq!(values(&f, &keys::DATE_INVOICE), [date(2024, 1, 31)]);
    assert_eq!(values(&f, &keys::DATE_SIGNED), [date(2024, 1, 10)]);
    assert_eq!(values(&f, &keys::DATE).len(), 6);
    assert!(conf_of(&f, &keys::DATE_DUE) < conf_of(&f, &keys::DATE));
}

fn money(units: i64, scale: u8, cur: [u8; 3]) -> Value {
    Value::Money(Money {
        units,
        scale,
        currency: Some(cur),
    })
}

#[test]
fn money_formats() {
    for (text, want) in [
        ("cena 1.200,50 RSD", money(120_050, 2, *b"RSD")),
        ("cena 1 200,50 din.", money(120_050, 2, *b"RSD")),
        ("cena 1 200 000 дин.", money(1_200_000, 0, *b"RSD")),
        ("RSD 1.200,50", money(120_050, 2, *b"RSD")),
        ("price €1,200.50", money(120_050, 2, *b"EUR")),
        ("price EUR 1.200", money(1200, 0, *b"EUR")),
        ("fee of $5,000", money(5000, 0, *b"USD")),
        ("USD 2.5 million", money(25_000_000, 1, *b"USD")),
        ("£ 300", money(300, 0, *b"GBP")),
        ("1.000,00 evra", money(100_000, 2, *b"EUR")),
    ] {
        assert_eq!(values(&run(text), &keys::AMOUNT), [want], "{text}");
    }
    for text in [
        "5 100 primeraka",
        "Article 1.200",
        "RSD",
        "TRY harder 100",
        "1.20.000 RSD",
    ] {
        assert!(values(&run(text), &keys::AMOUNT).is_empty(), "{text}");
    }
}

#[test]
fn money_totals_and_vat() {
    let f = run(
        "Iznos bez PDV-a: 1.000,00 RSD\nPDV 20%: 200,00 RSD\nUKUPNO ZA UPLATU: 1.200,00 RSD\n\
         Total: 99.90 EUR incl. VAT\nUkupno: 5.000,00",
    );
    assert_eq!(values(&f, &keys::AMOUNT_NET), [money(100_000, 2, *b"RSD")]);
    assert_eq!(values(&f, &keys::AMOUNT_GROSS), [money(9990, 2, *b"EUR")]);
    let totals = values(&f, &keys::AMOUNT_TOTAL);
    assert!(totals.contains(&money(120_000, 2, *b"RSD")), "{totals:?}");
    assert!(totals.contains(&Value::Money(Money {
        units: 500_000,
        scale: 2,
        currency: None
    })));
    assert!(!totals.contains(&money(20_000, 2, *b"RSD")));
}

#[test]
fn contacts() {
    let f = run(
        "e-mail: ime.prezime@firma-primer.rs, drugi@gmail.com\nwww.primer.co.rs i https://shop.example.com/x?y\n\
         Tel: (212) 555-1234 Fax +44 (0)20 7946 0958 Mob. 064/123-45-67",
    );
    assert_eq!(texts(&f, &keys::EMAIL_DOMAIN), ["firma-primer.rs"]);
    let w = texts(&f, &keys::WEB_DOMAIN);
    assert!(
        w.contains(&"primer.co.rs".to_owned()) && w.contains(&"shop.example.com".to_owned()),
        "{w:?}"
    );
    let p = texts(&f, &keys::PHONE);
    for want in ["+12125551234", "+442079460958", "+381641234567"] {
        assert!(p.contains(&want.to_owned()), "{want} not in {p:?}");
    }
}

#[test]
fn evidence_points_into_original_text_with_zone_and_box() {
    let p = pib();
    // NBSP, a soft hyphen and a zero-width space before the number shift normalized offsets.
    let text = format!("Zaglavlje\nPIB:\u{a0}\u{200b}{p}\nTekst");
    let start = text.find(&p).unwrap();
    let block_start = text.find("PIB").unwrap();
    let bbox = BBox {
        x0: 1.0,
        y0: 2.0,
        x1: 3.0,
        y1: 4.0,
    };
    let page = Page {
        text: text.clone(),
        blocks: vec![
            Block {
                span: Span { start: 0, end: 9 },
                kind: BlockKind::Paragraph,
                zone: Zone::Header,
                bbox: None,
            },
            Block {
                span: Span {
                    start: u32::try_from(block_start).unwrap(),
                    end: u32::try_from(start + p.len()).unwrap(),
                },
                kind: BlockKind::Paragraph,
                zone: Zone::Header,
                bbox: Some(bbox),
            },
        ],
        ..Page::new(0, PageKind::Physical)
    };
    let f = run_doc(Document { pages: vec![page] }, &Limits::default()).0;
    let fact = f.iter().find(|f| f.key == keys::RS_PIB).unwrap();
    let ev = fact.evidence.as_ref().unwrap();
    assert_eq!(ev.page, Some(0));
    assert_eq!(ev.span.unwrap().slice(&text), p);
    assert_eq!(ev.zone, Zone::Header);
    assert_eq!(ev.bbox, Some(bbox));
}

#[test]
fn cap_and_limits_make_problems() {
    let mut text = String::new();
    for k in 0..300 {
        writeln!(text, "{} RSD", 1000 + k).unwrap();
    }
    let (f, problems) = run_doc(doc(&[&text]), &Limits::default());
    assert_eq!(values(&f, &keys::AMOUNT).len(), quail_signals::MAX_PER_KEY);
    assert!(problems.iter().any(|p| p.contains("signal.amount")));
    let limits = Limits {
        max_text_bytes: 10,
        ..Limits::default()
    };
    let (_, problems) = run_doc(doc(&["abcdefghij", "PIB 1"]), &limits);
    assert!(problems.iter().any(|p| p.contains("text limit")));
}

#[test]
fn hostile_text_is_fast_and_does_not_panic() {
    let mut s = String::new();
    for _ in 0..20_000 {
        s.push_str("1.1/1-1 +1 (1) RS1 ДИН € $ 12. maj ");
    }
    s.push_str(&"9".repeat(100_000));
    s.push_str(&"A".repeat(50_000));
    let t = std::time::Instant::now();
    let _ = run(&s);
    assert!(t.elapsed().as_secs() < 10);
    for odd in [
        "\u{0}",
        "€",
        "+",
        "(",
        "12.",
        "PIB:",
        "\u{feff}PIB 1",
        "a@b",
        "www.",
        "http://",
        "1 RSD 1",
        "RS 1",
    ] {
        let _ = run(odd);
    }
}

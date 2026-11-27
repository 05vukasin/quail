//! `payment.extract@1` over contracts and invoices built in code (no real documents): the
//! price lines run through `item.lines` first, as in the engine.

use quail_core::{
    Analyzer, Artifacts, Blob, Block, BlockKind, Ctx, Document, Emitter, Fact, Format, Limits,
    Page, PageKind, Profile, Span, TextSource, Value, Zone,
};
use quail_items::ItemLines;
use quail_payment::{PaymentExtract, keys};

enum Part<'a> {
    P(&'a str),
    Table(&'a [&'a [&'a str]]),
}

fn page(index: u32, parts: &[Part<'_>]) -> Page {
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut push = |t: &str, kind: BlockKind, zone: Zone| {
        if !text.is_empty() {
            text.push('\n');
        }
        let start = u32::try_from(text.len()).unwrap();
        text.push_str(t);
        let end = u32::try_from(text.len()).unwrap();
        blocks.push(Block::new(Span { start, end }, kind, zone, None));
    };
    let mut tables = 0u16;
    for part in parts {
        match part {
            Part::P(t) => push(t, BlockKind::Paragraph, Zone::Body),
            Part::Table(rows) => {
                for (r, row) in rows.iter().enumerate() {
                    for (c, cell) in row.iter().enumerate() {
                        let kind = BlockKind::TableCell {
                            table: tables,
                            row: u16::try_from(r).unwrap(),
                            col: u16::try_from(c).unwrap(),
                        };
                        push(cell, kind, Zone::Table);
                    }
                }
                tables += 1;
            }
        }
    }
    Page {
        text,
        blocks,
        text_source: TextSource::Native,
        ..Page::new(index, PageKind::Logical)
    }
}

fn doc(parts: &[Part<'_>]) -> Document {
    Document {
        pages: vec![page(0, parts)],
    }
}

fn lines(text: &str) -> Document {
    let parts: Vec<Part<'_>> = text.lines().map(Part::P).collect();
    doc(&parts)
}

fn run(doc: Document) -> Vec<Fact> {
    let blob = Blob::from(Vec::new());
    let mut arts = Artifacts::new();
    arts.insert(doc);
    let limits = Limits::default();
    let mut facts = {
        let ctx = Ctx {
            blob: &blob,
            format: Format::Docx,
            locator: "test",
            artifacts: &arts,
            facts: &[],
            limits: &limits,
            profile: Profile::Fast,
        };
        let mut out = Emitter::new(ItemLines::ID);
        ItemLines::new().run(&ctx, &mut out).unwrap();
        out.facts().to_vec()
    };
    let ctx = Ctx {
        blob: &blob,
        format: Format::Docx,
        locator: "test",
        artifacts: &arts,
        facts: &facts,
        limits: &limits,
        profile: Profile::Fast,
    };
    let mut out = Emitter::new(PaymentExtract::ID);
    PaymentExtract.run(&ctx, &mut out).unwrap();
    let mine = out.facts().to_vec();
    facts.extend(mine);
    facts
        .into_iter()
        .filter(|f| f.key.namespace() == "payment")
        .collect()
}

fn get<'a>(facts: &'a [Fact], key: &quail_core::FactKey) -> Option<&'a Fact> {
    facts.iter().find(|f| &f.key == key)
}

/// The money of `key` as (value, currency).
fn money(facts: &[Fact], key: &quail_core::FactKey) -> Option<(f64, String)> {
    match &get(facts, key)?.value {
        Value::Money(m) => {
            #[allow(clippy::cast_precision_loss)]
            let v = m.units as f64 / 10f64.powi(i32::from(m.scale));
            let c = m
                .currency
                .map(|c| String::from_utf8_lossy(&c).into_owned())
                .unwrap_or_default();
            Some((v, c))
        }
        _ => None,
    }
}

fn text<'a>(facts: &'a [Fact], key: &quail_core::FactKey) -> Option<&'a str> {
    get(facts, key)?.value.as_text()
}

fn field<'a>(facts: &'a [Fact], key: &quail_core::FactKey, name: &str) -> Option<&'a str> {
    get(facts, key)?.value.field(name)?.as_text()
}

/// The quote of the highlight of `key`.
fn quote<'a>(facts: &'a [Fact], key: &quail_core::FactKey) -> Option<&'a str> {
    facts
        .iter()
        .filter(|f| f.key == keys::HIGHLIGHT)
        .find(|f| f.value.field("key").and_then(Value::as_text) == Some(key.as_str()))?
        .value
        .field("quote")?
        .as_text()
}

fn close(a: Option<(f64, String)>, v: f64, c: &str) {
    let (x, cur) = a.unwrap_or_else(|| panic!("no amount, want {v} {c}"));
    assert!((x - v).abs() < 0.005, "{x} != {v}");
    assert_eq!(cur, c);
}

const SR_CONTRACT: &str = "UGOVOR O PRUŽANJU USLUGA ODRŽAVANJA SOFTVERA
zaključen između Alfa Soft d.o.o. i Beta Trgovina d.o.o.
Član 1. Predmet ugovora
Predmet ovog ugovora je pružanje usluga održavanja poslovnog softvera Naručioca.
Član 2. Cena
Ukupna vrednost ugovora iznosi 1.234.567,89 RSD bez PDV-a.
PDV (20%) iznosi 246.913,58 RSD.
Ukupna vrednost ugovora sa PDV-om iznosi 1.481.481,47 RSD.
Plaćanje se vrši u roku od 15 dana od dana prijema fakture.
Ugovorna kazna iznosi 50.000,00 RSD.";

#[test]
fn serbian_service_contract_with_net_vat_gross() {
    let f = run(lines(SR_CONTRACT));
    close(money(&f, &keys::NET), 1_234_567.89, "RSD");
    close(money(&f, &keys::VAT), 246_913.58, "RSD");
    close(money(&f, &keys::GROSS), 1_481_481.47, "RSD");
    close(money(&f, &keys::AMOUNT), 1_481_481.47, "RSD");
    assert_eq!(text(&f, &keys::AMOUNT_BASIS), Some("gross"));
    assert_eq!(text(&f, &keys::CURRENCY), Some("RSD"));
    assert_eq!(
        field(&f, &keys::SUBJECT, "text"),
        Some("pružanje usluga održavanja poslovnog softvera Naručioca")
    );
    assert_eq!(field(&f, &keys::SUBJECT, "source"), Some("clause"));
    assert_eq!(field(&f, &keys::CATEGORY, "code"), Some("it_software"));
    // A contract without a period word has no period: nothing is guessed.
    assert!(get(&f, &keys::PERIOD).is_none());
    // Net + VAT = gross raises the confidence above the cue's own.
    assert!(get(&f, &keys::GROSS).unwrap().conf > 0.85);
    assert_eq!(
        quote(&f, &keys::NET),
        Some("Ukupna vrednost ugovora iznosi 1.234.567,89 RSD bez PDV-a")
    );
    // Every fact has evidence and a highlight.
    for k in [&keys::AMOUNT, &keys::SUBJECT, &keys::CATEGORY, &keys::VAT] {
        assert!(get(&f, k).unwrap().evidence.is_some(), "{k}");
        assert!(quote(&f, k).is_some(), "{k}");
    }
}

#[test]
fn highlight_points_at_the_block_and_characters() {
    let f = run(lines(SR_CONTRACT));
    let h = f
        .iter()
        .filter(|f| f.key == keys::HIGHLIGHT)
        .find(|f| f.value.field("key").and_then(Value::as_text) == Some("payment.subject"))
        .unwrap();
    // Line 4 of the document is block 3; the subject starts after the label.
    assert_eq!(h.value.field("block"), Some(&Value::Int(3)));
    assert_eq!(h.value.field("page"), Some(&Value::Int(0)));
    assert_eq!(h.value.field("start"), Some(&Value::Int(0)));
    let q = h.value.field("quote").and_then(Value::as_text).unwrap();
    assert!(q.starts_with("Predmet ovog ugovora je pružanje"), "{q}");
    let end = match h.value.field("end") {
        Some(Value::Int(e)) => *e,
        _ => panic!(),
    };
    assert_eq!(usize::try_from(end).unwrap(), q.chars().count());
}

#[test]
fn serbian_cyrillic_lease_with_monthly_rent() {
    let f = run(lines(
        "УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА
Предмет уговора: закуп пословног простора површине 120 м2 у Београду.
Месечна закупнина износи 1.500,00 EUR.
Закупнина се плаћа до 10. у месецу.",
    ));
    close(money(&f, &keys::AMOUNT), 1500.0, "EUR");
    assert_eq!(text(&f, &keys::AMOUNT_BASIS), Some("unstated"));
    assert_eq!(text(&f, &keys::PERIOD), Some("monthly"));
    assert_eq!(
        field(&f, &keys::SUBJECT, "text"),
        Some("закуп пословног простора површине 120 м2 у Београду")
    );
    assert_eq!(field(&f, &keys::CATEGORY, "code"), Some("rent_lease"));
    assert!(get(&f, &keys::NET).is_none() && get(&f, &keys::GROSS).is_none());
}

#[test]
fn serbian_monthly_fee_is_not_multiplied() {
    let f = run(lines(
        "UGOVOR O PRUŽANJU KNJIGOVODSTVENIH USLUGA
Predmet ugovora je pružanje knjigovodstvenih usluga i obračun zarada za Naručioca.
Mesečna naknada iznosi 45.000,00 dinara.",
    ));
    close(money(&f, &keys::AMOUNT), 45_000.0, "RSD");
    assert_eq!(text(&f, &keys::PERIOD), Some("monthly"));
    assert_eq!(field(&f, &keys::CATEGORY, "code"), Some("accounting_audit"));
}

#[test]
fn english_service_contract() {
    let f = run(lines(
        "SERVICES AGREEMENT
This Agreement is made between Acme Ltd and Example Corp.
1. Subject of the Agreement
The Contractor shall provide legal services to the Client in connection with the acquisition.
2. Fees
The total fee for the Services is EUR 25,000.00 excluding VAT.
VAT at 20% amounts to EUR 5,000.00.",
    ));
    close(money(&f, &keys::NET), 25_000.0, "EUR");
    close(money(&f, &keys::VAT), 5_000.0, "EUR");
    assert!(get(&f, &keys::GROSS).is_none());
    close(money(&f, &keys::AMOUNT), 25_000.0, "EUR");
    assert_eq!(text(&f, &keys::AMOUNT_BASIS), Some("net"));
    assert_eq!(field(&f, &keys::SUBJECT, "source"), Some("after_heading"));
    assert!(
        field(&f, &keys::SUBJECT, "text")
            .unwrap()
            .contains("legal services")
    );
    assert_eq!(field(&f, &keys::CATEGORY, "code"), Some("legal"));
}

#[test]
fn english_monthly_subscription_in_usd() {
    let f = run(lines(
        "SOFTWARE SUBSCRIPTION AGREEMENT
Scope of Services: hosting and support of the Client's online shop.
The Client shall pay a monthly fee of USD 1,200.00 for the Services.",
    ));
    close(money(&f, &keys::AMOUNT), 1200.0, "USD");
    assert_eq!(text(&f, &keys::PERIOD), Some("monthly"));
    assert_eq!(field(&f, &keys::CATEGORY, "code"), Some("it_software"));
    assert_eq!(
        field(&f, &keys::SUBJECT, "text"),
        Some("hosting and support of the Client's online shop")
    );
}

#[test]
fn serbian_invoice_with_line_items() {
    let f = run(doc(&[
        Part::P("FAKTURA br. 12/2026"),
        Part::P("Datum prometa: 15.01.2026."),
        Part::Table(&[
            &["Opis", "Količina", "JM", "Cena", "Iznos"],
            &["Održavanje servera", "10", "sat", "3.000,00", "30.000,00"],
            &["Licenca za antivirus", "5", "kom", "2.000,00", "10.000,00"],
        ]),
        Part::P("Ukupno bez PDV-a: 40.000,00 RSD"),
        Part::P("PDV 20%: 8.000,00 RSD"),
        Part::P("Ukupno za uplatu: 48.000,00 RSD"),
    ]));
    close(money(&f, &keys::NET), 40_000.0, "RSD");
    close(money(&f, &keys::VAT), 8_000.0, "RSD");
    close(money(&f, &keys::GROSS), 48_000.0, "RSD");
    close(money(&f, &keys::AMOUNT), 48_000.0, "RSD");
    assert_eq!(text(&f, &keys::PERIOD), Some("one_off"));
    let Some(Value::List(items)) = get(&f, &keys::LINES).map(|f| &f.value) else {
        panic!("no payment.lines: {f:#?}");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].field("description").and_then(Value::as_text),
        Some("Održavanje servera")
    );
    assert_eq!(field(&f, &keys::SUBJECT, "source"), Some("line_items"));
    assert_eq!(field(&f, &keys::CATEGORY, "code"), Some("it_software"));
    // One highlight per line, on the row block (cells joined by tabs).
    let rows: Vec<&str> = f
        .iter()
        .filter(|f| f.key == keys::HIGHLIGHT)
        .filter(|f| f.value.field("key").and_then(Value::as_text) == Some("payment.lines"))
        .filter_map(|f| f.value.field("quote").and_then(Value::as_text))
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].starts_with("Održavanje servera"), "{rows:?}");
}

#[test]
fn english_invoice_total_is_net_plus_tax() {
    let f = run(lines(
        "INVOICE No. 2026-0042
Bill to: Example Corp
Subtotal: $1,500.00
Tax (8%): $120.00
Total: $1,620.00",
    ));
    close(money(&f, &keys::NET), 1500.0, "USD");
    close(money(&f, &keys::VAT), 120.0, "USD");
    close(money(&f, &keys::GROSS), 1620.0, "USD");
    close(money(&f, &keys::AMOUNT), 1620.0, "USD");
    assert_eq!(text(&f, &keys::AMOUNT_BASIS), Some("gross"));
    assert_eq!(text(&f, &keys::PERIOD), Some("one_off"));
}

#[test]
fn amount_in_the_next_table_cell() {
    let f = run(doc(&[
        Part::P("RAČUN"),
        Part::Table(&[&["Ukupno za uplatu:", "120.000,00 RSD"]]),
    ]));
    close(money(&f, &keys::AMOUNT), 120_000.0, "RSD");
}

#[test]
fn spaced_thousands_and_euro_sign() {
    let f = run(lines("Ugovorena cena: 1 234 567,89 RSD\nCena usluge: €2.500,00"));
    close(money(&f, &keys::AMOUNT), 1_234_567.89, "RSD");
}

#[test]
fn no_price_no_amount() {
    for text in [
        "UGOVOR O POSLOVNOJ SARADNJI
Predmet ovog ugovora je uređivanje međusobnih odnosa ugovornih strana.
Ugovor stupa na snagu danom potpisivanja i važi 2 godine.
Ugovorna kazna iznosi 100.000,00 RSD.
Rok za odgovor je 30 dana od 15.01.2026.",
        "MUTUAL NON-DISCLOSURE AGREEMENT
This Agreement remains in force for 3 years from 1 January 2026.
In case of breach, liquidated damages in the amount of EUR 10,000.00 apply.
The deposit amount is EUR 2,000.00.",
        "Zapisnik sa sastanka održanog 12.03.2026. u 10 časova. Prisutno 12 članova.",
    ] {
        let f = run(lines(text));
        for k in [
            &keys::AMOUNT,
            &keys::NET,
            &keys::GROSS,
            &keys::VAT,
            &keys::CURRENCY,
            &keys::PERIOD,
        ] {
            assert!(get(&f, k).is_none(), "{k} in {text}: {f:#?}");
        }
    }
}

#[test]
fn rates_and_unit_prices_are_not_the_price() {
    let f = run(lines(
        "Cena po satu iznosi 50,00 EUR.\nThe hourly rate is USD 120.00 per hour.",
    ));
    assert!(get(&f, &keys::AMOUNT).is_none(), "{f:#?}");
}

#[test]
fn empty_and_blank_documents() {
    assert!(run(Document::default()).is_empty());
    assert!(run(lines("   \n\n")).is_empty());
}

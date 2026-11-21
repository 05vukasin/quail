//! Cards and extractive summaries from facts shaped like the other crates' output. All names
//! and numbers are invented.

use quail_core::{
    Analyzer, AnalyzerId, Artifacts, Blob, Ctx, Date, Document, Emitter, Fact, FactKey, Format,
    Limits, Money, Page, PageKind, Profile, TextSource, Value,
};
use quail_summary::{SummaryCard, SummaryExtractive, factcheck, keys, tokens};

const SRC: AnalyzerId = AnalyzerId::new("test.facts", 1);

fn fact(key: &'static str, value: Value, conf: f32) -> Fact {
    Fact {
        key: FactKey::new(key),
        value,
        conf,
        src: SRC,
        evidence: None,
    }
}

fn text(s: &str) -> Value {
    Value::text(s)
}

fn date(y: i16, m: u8, d: u8) -> Value {
    Value::Date(Date { y, m, d })
}

fn eur(units: i64) -> Value {
    Value::Money(Money {
        units,
        scale: 2,
        currency: Some(*b"EUR"),
    })
}

fn candidate(surface: &str, score: f64, roles: &[&str]) -> Value {
    Value::record([
        ("key", text(&surface.to_lowercase())),
        ("surface", text(surface)),
        ("score", Value::Float(score)),
        (
            "roles",
            Value::List(roles.iter().map(|r| text(r)).collect()),
        ),
    ])
}

fn amendment_facts() -> Vec<Fact> {
    vec![
        fact("engine.format", text("pdf"), 1.0),
        fact("pdf.page_count", Value::Int(3), 1.0),
        fact("pdf.producer", text("Microsoft® Word 2016"), 1.0),
        fact("pdf.signed", Value::Bool(true), 1.0),
        fact("text.lang", text("en"), 0.98),
        fact("text.script", text("latin"), 1.0),
        fact(
            "ref.title",
            text("Amendment No. 2 to the Master Services Agreement"),
            0.9,
        ),
        fact("ref.instrument_kind", text("amendment"), 0.9),
        fact("ref.ordinal", Value::Int(2), 0.9),
        fact(
            "ref.doc_number",
            Value::record([("kind", text("amendment")), ("raw", text("114/23-A2"))]),
            0.8,
        ),
        fact(
            "ref.cites",
            Value::record([
                ("kind", text("msa")),
                ("relation", text("amends")),
                ("raw", text("114/23")),
                ("key", text("11423")),
            ]),
            0.85,
        ),
        fact(
            "ref.cites",
            Value::record([
                ("kind", text("po")),
                ("relation", text("references")),
                ("raw", text("PO-77812")),
            ]),
            0.6,
        ),
        fact(
            "party.candidates",
            Value::List(vec![
                candidate("Northwind Logistics Ltd", 0.95, &["customer", "signatory"]),
                candidate("Blue River Software GmbH", 0.91, &["supplier"]),
                candidate("Some Bank AG", 0.2, &["mentioned"]),
            ]),
            1.0,
        ),
        fact("signal.date", date(2023, 9, 12), 0.9),
        fact("signal.date_effective", date(2023, 9, 12), 0.8),
        fact("signal.date_expiry", date(2026, 12, 31), 0.7),
        fact("signal.amount", eur(420_000), 0.9),
        fact("signal.amount_total", eur(420_000), 0.8),
        fact("signal.vat", text("DE136695976"), 0.95),
        fact("signal.iban", text("DE89370400440532013000"), 0.9),
        fact("signal.iban", text("GB82WEST12345698765432"), 0.8),
        fact("signal.iban", text("FR1420041010050500013M02606"), 0.7),
        fact(
            "sketch.simhash_text",
            Value::U64(0x9f3a_61c2_b7d4_0e15),
            1.0,
        ),
        fact(
            "sketch.simhash_template",
            Value::U64(0x11c0_7a3e_5b92_d468),
            1.0,
        ),
    ]
}

const AMENDMENT_TEXT: &str = "AMENDMENT No. 2 TO THE MASTER SERVICES AGREEMENT 114/23\n\
This Amendment is made between Northwind Logistics Ltd and Blue River Software GmbH.\n\
1. Purpose\n\
The purpose of this Amendment is to extend the support term of the warehouse management system \
until 31 December 2026 and to set the monthly fee. All other terms remain in force.\n\
2. Fees\n\
The monthly fee is EUR 4,200.00.";

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

fn run(a: &dyn Analyzer, facts: &[Fact], document: Document) -> Vec<Fact> {
    let blob = Blob::new(Vec::new());
    let mut artifacts = Artifacts::new();
    artifacts.insert(document);
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "x.pdf",
        artifacts: &artifacts,
        facts,
        limits: &limits,
        profile: Profile::Fast,
    };
    let mut out = Emitter::new(a.id());
    a.run(&ctx, &mut out).unwrap();
    out.into_parts().0
}

fn get<'a>(facts: &'a [Fact], key: &FactKey) -> &'a Value {
    &facts.iter().find(|f| &f.key == key).unwrap().value
}

#[test]
fn amendment_card() {
    let facts = amendment_facts();
    let out = run(&SummaryCard::new(), &facts, doc(&[AMENDMENT_TEXT]));
    let text = get(&out, &keys::CARD_TEXT).as_text().unwrap();
    assert_eq!(
        text,
        "doc: amendment #2 no=114/23-A2 \"Amendment No. 2 to the Master Services Agreement\" lang=en pages=3 pdf\n\
         parties: customer=Northwind Logistics Ltd; supplier=Blue River Software GmbH\n\
         dates: effective=2023-09-12 expires=2026-12-31\n\
         amounts: total=EUR 4,200.00\n\
         refs: amends=msa 114/23; references=po PO-77812\n\
         ids: vat=DE136695976 iban=DE89370400440532013000,GB82WEST12345698765432 (+1)\n\
         flags: signed\n\
         meta: producer=\"Microsoft® Word 2016\"\n\
         sketch: text=9f3a61c2b7d40e15 tmpl=11c07a3e5b92d468"
    );
    let Value::Int(n) = get(&out, &keys::CARD_TOKENS) else {
        panic!()
    };
    assert_eq!(usize::try_from(*n).unwrap(), tokens::estimate(text));
    assert!((100..=300).contains(n), "{n}");

    let card = get(&out, &keys::CARD);
    assert_eq!(
        card.field("kind").and_then(Value::as_text),
        Some("amendment")
    );
    let Some(Value::List(parties)) = card.field("parties") else {
        panic!()
    };
    assert_eq!(
        parties.len(),
        2,
        "the bank under the score floor is left out"
    );
    assert_eq!(
        parties[1].field("role").and_then(Value::as_text),
        Some("supplier")
    );
    // The record survives a JSON round trip.
    let j = serde_json::to_string(card).unwrap();
    assert_eq!(&serde_json::from_str::<Value>(&j).unwrap(), card);
}

#[test]
fn budget_drops_low_priority_lines_first() {
    let facts = amendment_facts();
    let out = run(
        &SummaryCard::with_max_tokens(60),
        &facts,
        doc(&[AMENDMENT_TEXT]),
    );
    let text = get(&out, &keys::CARD_TEXT).as_text().unwrap();
    assert!(tokens::estimate(text) <= 60, "{text}");
    assert!(text.starts_with("doc: amendment"));
    assert!(text.contains("parties: "));
    assert!(!text.contains("sketch: "));
    assert!(!text.contains("meta: "));
}

#[test]
fn amendment_extractive_summary() {
    let facts = amendment_facts();
    let out = run(&SummaryExtractive, &facts, doc(&[AMENDMENT_TEXT]));
    let s = get(&out, &keys::EXTRACTIVE).as_text().unwrap();
    assert_eq!(
        s,
        "Amendment No. 2 (no. 114/23-A2) to the master services agreement 114/23 between \
         Northwind Logistics Ltd (customer) and Blue River Software GmbH (supplier), effective \
         2023-09-12, expiring 2026-12-31, total EUR 4,200.00. Subject: The purpose of this \
         Amendment is to extend the support term of the warehouse management system until 31 \
         December 2026 and to set the monthly fee."
    );
    let subject = get(&out, &keys::SUBJECT);
    assert_eq!(
        subject.field("method").and_then(Value::as_text),
        Some("clause")
    );
    assert_eq!(
        subject.field("label").and_then(Value::as_text),
        Some("purpose")
    );
    // Consistent by construction: every value is in the card or the text.
    let card_out = run(&SummaryCard::new(), &facts, doc(&[AMENDMENT_TEXT]));
    let card = get(&card_out, &keys::CARD_TEXT).as_text().unwrap();
    let fc = factcheck::check(s, &[card, AMENDMENT_TEXT], true);
    assert!(fc.passed(), "{fc:?}");
    assert!(fc.claims >= 8, "{fc:?}");
}

#[test]
fn serbian_cyrillic_lease_keeps_names_and_quotes_the_subject() {
    let facts = vec![
        fact("engine.format", text("docx"), 1.0),
        fact("docx.pages", Value::Int(2), 1.0),
        fact("docx.tracked_insertions", Value::Int(4), 1.0),
        fact("docx.has_macros", Value::Bool(false), 1.0),
        fact("text.lang", text("sr"), 0.97),
        fact("text.script", text("cyrillic"), 1.0),
        fact("ref.instrument_kind", text("contract"), 0.8),
        fact("ref.title", text("УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА"), 0.9),
        fact(
            "party.candidates",
            Value::List(vec![
                candidate("Дунав Некретнине а.д.", 0.93, &["party"]),
                candidate("Балкан Логистик д.о.о.", 0.9, &["customer"]),
            ]),
            1.0,
        ),
        fact("signal.date_signed", date(2023, 9, 12), 0.8),
        fact("signal.rs_pib", text("101234567"), 0.95),
        fact("signal.rs_mb", text("07005466"), 0.9),
    ];
    let body = "УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА\nЧлан 1.\nПредмет уговора\nЗакуподавац даје у \
                закуп Закупцу магацински простор у Новом Саду, површине 1.200 м2. Остало.";
    let out = run(&SummaryCard::new(), &facts, doc(&[body]));
    let text = get(&out, &keys::CARD_TEXT).as_text().unwrap();
    assert!(
        text.starts_with(
            "doc: contract \"УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА\" lang=sr-Cyrl pages=2 docx\n\
         parties: party=Дунав Некретнине а.д.; customer=Балкан Логистик д.о.о.\n\
         dates: signed=2023-09-12\nids: pib=101234567 mb=07005466\nflags: tracked_changes(4)"
        ),
        "{text}"
    );

    let out = run(&SummaryExtractive, &facts, doc(&[body]));
    let s = get(&out, &keys::EXTRACTIVE).as_text().unwrap();
    assert_eq!(
        s,
        "Contract \"УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА\" between Дунав Некретнине а.д. (party) and Балкан Логистик д.о.о. (customer), \
         signed 2023-09-12. Subject (sr, original): \"Закуподавац даје у закуп Закупцу \
         магацински простор у Новом Саду, површине 1.200 м2.\""
    );
}

#[test]
fn empty_scan_still_gets_a_card_and_a_sentence() {
    let facts = vec![
        fact("engine.format", text("pdf"), 1.0),
        fact("pdf.page_count", Value::Int(5), 1.0),
        fact("pdf.image_only_pages", Value::Int(5), 1.0),
        fact("pdf.pages_needing_ocr", Value::Int(5), 1.0),
    ];
    let d = Document {
        pages: (0..5).map(|i| Page::new(i, PageKind::Physical)).collect(),
    };
    let out = run(&SummaryCard::new(), &facts, d.clone());
    assert_eq!(
        get(&out, &keys::CARD_TEXT).as_text(),
        Some("doc: document pages=5 pdf\nflags: scanned(5) needs_ocr(5)")
    );
    let out = run(&SummaryExtractive, &facts, d);
    assert_eq!(
        get(&out, &keys::EXTRACTIVE).as_text(),
        Some("PDF document, scanned.")
    );
    assert!(!out.iter().any(|f| f.key == keys::SUBJECT));
}

//! End-to-end tests of `ref.references` on documents built in code.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use quail_core::{
    Analyzer, Artifacts, Blob, Block, BlockKind, Ctx, Date, Document, Emitter, Error, Fact, Format,
    Limits, Page, PageKind, Profile, Span, Value, Zone,
};
use quail_refs::{RefAnalyzer, field, keys};

/// One block per line, all `Paragraph` in `Unknown` zone (like a plain text layer).
fn page(index: u32, text: &str) -> Page {
    let mut blocks = Vec::new();
    let mut at = 0u32;
    for line in text.split('\n') {
        let len = u32::try_from(line.len()).unwrap();
        blocks.push(Block {
            span: Span {
                start: at,
                end: at + len,
            },
            kind: BlockKind::Paragraph,
            zone: Zone::Unknown,
            bbox: None,
        });
        at += len + 1;
    }
    Page {
        text: text.to_owned(),
        blocks,
        ..Page::new(index, PageKind::Physical)
    }
}

fn doc(pages: &[&str]) -> Document {
    Document {
        pages: pages
            .iter()
            .enumerate()
            .map(|(i, t)| page(u32::try_from(i).unwrap(), t))
            .collect(),
    }
}

fn run_with(d: Document, limits: &Limits, out: &mut Emitter) -> quail_core::Result<()> {
    let mut artifacts = Artifacts::new();
    artifacts.insert(d);
    let blob = Blob::new(Vec::<u8>::new());
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://test",
        artifacts: &artifacts,
        facts: &[],
        limits,
        profile: Profile::Fast,
    };
    RefAnalyzer::new().run(&ctx, out)
}

fn run(pages: &[&str]) -> Vec<Fact> {
    let mut out = Emitter::new(RefAnalyzer::ID);
    run_with(doc(pages), &Limits::default(), &mut out).unwrap();
    out.into_parts().0
}

fn of<'f>(facts: &'f [Fact], key: &quail_core::FactKey) -> Vec<&'f Fact> {
    facts.iter().filter(|f| &f.key == key).collect()
}

fn text(v: Option<&Value>) -> Option<&str> {
    match v? {
        Value::Text(s) => Some(s),
        _ => None,
    }
}

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

fn date(v: Option<&Value>) -> Option<(i16, u8, u8)> {
    match v? {
        Value::Date(Date { y, m, d }) => Some((*y, *m, *d)),
        _ => None,
    }
}

/// (kind, key) of every own number.
fn own(facts: &[Fact]) -> Vec<(String, String)> {
    of(facts, &keys::DOC_NUMBER)
        .iter()
        .map(|f| {
            (
                text(field(&f.value, "kind")).unwrap_or("").to_owned(),
                text(field(&f.value, "key")).unwrap_or("").to_owned(),
            )
        })
        .collect()
}

/// (kind, relation, key, ordinal, date) of every cite.
type CiteRow = (String, String, String, Option<i64>, Option<(i16, u8, u8)>);
fn cites(facts: &[Fact]) -> Vec<CiteRow> {
    of(facts, &keys::CITES)
        .iter()
        .map(|f| {
            (
                text(field(&f.value, "kind")).unwrap_or("").to_owned(),
                text(field(&f.value, "relation")).unwrap_or("").to_owned(),
                text(field(&f.value, "key")).unwrap_or("").to_owned(),
                int(field(&f.value, "ordinal")),
                date(field(&f.value, "date")),
            )
        })
        .collect()
}

fn single_text(facts: &[Fact], key: &quail_core::FactKey) -> Option<String> {
    of(facts, key)
        .first()
        .and_then(|f| text(Some(&f.value)))
        .map(str::to_owned)
}

fn s(x: &str) -> String {
    x.to_owned()
}

#[test]
fn serbian_contract_title_and_number() {
    let f =
        run(&["UGOVOR O PRUŽANJU USLUGA\nbr. 114/23\nZaključen dana 12.03.2023. godine između:"]);
    assert_eq!(
        single_text(&f, &keys::TITLE).as_deref(),
        Some("UGOVOR O PRUŽANJU USLUGA")
    );
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("contract")
    );
    assert_eq!(own(&f), vec![(s("contract"), s("11423"))]);
}

#[test]
fn serbian_labelled_number_with_date_and_raw_form() {
    let f = run(&["Ugovor br. 114/23 od 12.03.2023. godine\nPredmet ugovora je isporuka opreme."]);
    let d = of(&f, &keys::DOC_NUMBER);
    assert_eq!(d.len(), 1);
    assert!(matches!(d[0].value, Value::Record(_)));
    assert_eq!(
        d[0].value.field("raw").and_then(Value::as_text),
        Some("114/23")
    );
    assert_eq!(text(field(&d[0].value, "raw")), Some("114/23"));
    assert_eq!(text(field(&d[0].value, "key")), Some("11423"));
    assert_eq!(date(field(&d[0].value, "date")), Some((2023, 3, 12)));
}

#[test]
fn serbian_cyrillic_contract() {
    let f = run(&["УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА\nУговор бр. 1145/2023\nСтрана 1 од 4"]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("contract")
    );
    assert_eq!(own(&f), vec![(s("contract"), s("11452023"))]);
    let m = of(&f, &keys::PAGE_MARKER);
    assert_eq!(m.len(), 1);
    assert_eq!(int(field(&m[0].value, "n")), Some(1));
    assert_eq!(int(field(&m[0].value, "of")), Some(4));
}

#[test]
fn serbian_annex_cites_its_contract() {
    let f = run(&["ANEKS BR. 2\nUGOVORA O ODRŽAVANJU BR. 114/23\nOvim aneksom menja se član 5."]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("amendment")
    );
    assert_eq!(
        int(of(&f, &keys::ORDINAL).first().map(|x| &x.value)),
        Some(2)
    );
    assert_eq!(
        cites(&f),
        vec![(s("contract"), s("amends"), s("11423"), None, None)]
    );
    assert!(own(&f).is_empty());

    let f = run(&["Aneks br. 2 uz Ugovor br. 114/23 od 01.02.2023.\nTekst aneksa."]);
    assert_eq!(
        cites(&f),
        vec![(
            s("contract"),
            s("amends"),
            s("11423"),
            None,
            Some((2023, 2, 1))
        )]
    );
}

#[test]
fn cyrillic_annex() {
    let f = run(&["АНЕКС БР. 3\nуз Уговор бр. 514/2022 од 12. марта 2022. године"]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("amendment")
    );
    assert_eq!(
        int(of(&f, &keys::ORDINAL).first().map(|x| &x.value)),
        Some(3)
    );
    assert_eq!(
        cites(&f),
        vec![(
            s("contract"),
            s("amends"),
            s("5142022"),
            None,
            Some((2022, 3, 12))
        )]
    );
}

#[test]
fn serbian_invoice_cites_contract_and_skips_bank_account() {
    let f = run(&[
        "FAKTURA\nFaktura broj: 2023-001234\nTekući račun: 160-0000000123456-78\nPo ugovoru br. 114/23\nUkupno za uplatu",
    ]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("invoice")
    );
    assert_eq!(own(&f), vec![(s("invoice"), s("2023001234"))]);
    assert_eq!(
        cites(&f),
        vec![(s("contract"), s("billed_against"), s("11423"), None, None)]
    );
}

#[test]
fn racun_label_is_an_invoice() {
    let f = run(&["RAČUN\nRačun br. 0457/2023\nDatum prometa: 01.02.2023."]);
    assert_eq!(own(&f), vec![(s("invoice"), s("4572023"))]);
}

#[test]
fn english_msa_own_number() {
    let f = run(&[
        "MASTER SERVICES AGREEMENT\nAgreement No. KW-482917\nThis Master Services Agreement is entered into by the parties.",
    ]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("msa")
    );
    assert_eq!(own(&f), vec![(s("contract"), s("kw482917"))]);
}

#[test]
fn english_amendment_title_cites_msa_with_date() {
    let f = run(&[
        "AMENDMENT NO. 2 TO THE MASTER SERVICES AGREEMENT DATED 1 MARCH 2021\nThe parties agree as follows.",
    ]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("amendment")
    );
    assert_eq!(
        int(of(&f, &keys::ORDINAL).first().map(|x| &x.value)),
        Some(2)
    );
    assert_eq!(
        cites(&f),
        vec![(s("msa"), s("amends"), s(""), None, Some((2021, 3, 1)))]
    );
}

#[test]
fn third_amendment_ordinal_word() {
    let f = run(&["THIRD AMENDMENT TO LEASE AGREEMENT\nText."]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("amendment")
    );
    assert_eq!(
        int(of(&f, &keys::ORDINAL).first().map(|x| &x.value)),
        Some(3)
    );
}

#[test]
fn sow_issued_under_msa() {
    let f = run(&[
        "STATEMENT OF WORK NO. 3\nThis Statement of Work is issued pursuant to the Master Services Agreement No. MSA-2020-0045 dated March 1, 2021.",
    ]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("sow")
    );
    assert_eq!(
        int(of(&f, &keys::ORDINAL).first().map(|x| &x.value)),
        Some(3)
    );
    assert_eq!(
        cites(&f),
        vec![(
            s("msa"),
            s("issued_under"),
            s("msa20200045"),
            None,
            Some((2021, 3, 1))
        )]
    );
}

#[test]
fn sow_number_cited_from_invoice() {
    let f =
        run(&["INVOICE\nInvoice No: INV-2023-0017\nServices pursuant to SOW #12 under the MSA."]);
    assert_eq!(own(&f), vec![(s("invoice"), s("INV20230017"))]);
    let c = cites(&f);
    assert!(
        c.contains(&(s("sow"), s("issued_under"), s(""), Some(12), None)),
        "{c:?}"
    );
    assert!(
        c.contains(&(s("msa"), s("issued_under"), s(""), None, None)),
        "{c:?}"
    );
}

#[test]
fn invoice_po_is_billed_against_po_own_on_po() {
    let f = run(&[
        "INVOICE\nInvoice Number: 5821\nPO Number: 4500012345\nissued under Purchase Order 4500012345",
    ]);
    assert_eq!(own(&f), vec![(s("invoice"), s("5821"))]);
    assert_eq!(
        cites(&f),
        vec![(s("po"), s("billed_against"), s("4500012345"), None, None)]
    );

    let f = run(&["PURCHASE ORDER\nPO Number: 4500012345\nContract No. KW-482917"]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("po")
    );
    assert_eq!(own(&f), vec![(s("po"), s("4500012345"))]);
    assert_eq!(
        cites(&f),
        vec![(s("contract"), s("issued_under"), s("kw482917"), None, None)]
    );
}

#[test]
fn glued_codes_and_padding() {
    let f = run(&["SERVICE ORDER\nService Order No. QXPSO_078\nPO-4500012399"]);
    assert_eq!(own(&f)[0], (s("order"), s("qxpso78")));
    assert!(own(&f).contains(&(s("po"), s("po4500012399"))));
}

#[test]
fn version_tail_gives_base() {
    let f = run(&["AMENDMENT\nAmendment No. KW482917-QTRS-v14"]);
    let d = of(&f, &keys::DOC_NUMBER);
    assert_eq!(text(field(&d[0].value, "key")), Some("kw482917qtrsv14"));
    assert_eq!(text(field(&d[0].value, "base")), Some("kw482917qtrs"));
}

#[test]
fn unlabelled_document_infers_kind_from_head_label() {
    let f = run(&["Acme d.o.o.\nBeograd\nDatum: 01.02.2023.\nNapomena: Račun br. 2023-0456"]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("invoice")
    );
    let k = of(&f, &keys::INSTRUMENT_KIND);
    assert!(k[0].conf <= 0.5);
    assert_eq!(own(&f), vec![(s("invoice"), s("20230456"))]);
}

#[test]
fn generic_head_label() {
    let f = run(&["PURCHASE ORDER\nNo.: 4500077712\nDelivery address"]);
    assert_eq!(own(&f), vec![(s("po"), s("4500077712"))]);
}

#[test]
fn page_markers() {
    let f = run(&[
        "Text one.\nPage 2 of 5",
        "Strana 3 od 5\nText.",
        "Стр. 4 од 5",
        "Text\n5/5",
    ]);
    let m: Vec<_> = of(&f, &keys::PAGE_MARKER)
        .iter()
        .map(|x| {
            (
                x.evidence.as_ref().and_then(|e| e.page),
                int(field(&x.value, "n")),
                int(field(&x.value, "of")),
            )
        })
        .collect();
    assert_eq!(
        m,
        vec![
            (Some(0), Some(2), Some(5)),
            (Some(1), Some(3), Some(5)),
            (Some(2), Some(4), Some(5)),
            (Some(3), Some(5), Some(5))
        ]
    );
}

#[test]
fn version_hints() {
    let f = run(&["DRAFT\nMASTER SERVICES AGREEMENT\nExecution Version\nv2\nrev. 3\nThe text."]);
    let mut v: Vec<(String, Option<String>)> = of(&f, &keys::VERSION_HINT)
        .iter()
        .map(|x| {
            (
                text(field(&x.value, "kind")).unwrap().to_owned(),
                text(field(&x.value, "n")).map(str::to_owned),
            )
        })
        .collect();
    v.sort();
    assert_eq!(
        v,
        vec![
            (s("draft"), None),
            (s("execution"), None),
            (s("revision"), Some(s("3"))),
            (s("version"), Some(s("2"))),
        ]
    );
    let f = run(&["Nacrt\nUGOVOR O ZAKUPU\nVerzija 2"]);
    assert_eq!(of(&f, &keys::VERSION_HINT).len(), 2);
}

#[test]
fn heading_block_is_the_title() {
    let mut d = doc(&["ACME LTD\nSide Letter to the Supply Agreement\nDear Sirs,"]);
    d.pages[0].blocks[1].kind = BlockKind::Heading { level: 1 };
    d.pages[0].blocks[1].zone = Zone::Title;
    let mut out = Emitter::new(RefAnalyzer::ID);
    run_with(d, &Limits::default(), &mut out).unwrap();
    let f = out.into_parts().0;
    assert_eq!(
        single_text(&f, &keys::TITLE).as_deref(),
        Some("Side Letter to the Supply Agreement")
    );
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("side_letter")
    );
    let t = of(&f, &keys::TITLE)[0];
    assert_eq!(t.evidence.as_ref().map(|e| e.zone), Some(Zone::Title));
}

#[test]
fn instrument_kinds_from_titles() {
    let cases = [
        ("CHANGE ORDER NO. 4", "change_order", Some(4)),
        (
            "AMENDED AND RESTATED MASTER SERVICES AGREEMENT",
            "amended_restated",
            None,
        ),
        ("MUTUAL NON-DISCLOSURE AGREEMENT", "nda", None),
        ("Purchase Order", "po", None),
        ("NARUDŽBENICA", "po", None),
        ("ANEKS II", "amendment", Some(2)),
        ("PRILOG 1", "annex", Some(1)),
        ("OKVIRNI SPORAZUM", "framework_agreement", None),
        ("OKVIRNI UGOVOR", "msa", None),
        ("ODLUKA O DODELI UGOVORA", "award_decision", None),
        ("ОДЛУКА О ИЗМЕНИ УГОВОРА", "decision", None),
        ("CENOVNIK USLUGA", "price_list", None),
        ("ОБАВЕШТЕЊЕ О ЗАКЉУЧЕНОМ УГОВОРУ", "notice", None),
        ("KONKURSNA DOKUMENTACIJA", "tender_documentation", None),
        ("MODEL UGOVORA", "tender_documentation", None),
        ("PONUDA", "quote", None),
        ("Amendment AM9", "amendment", Some(9)),
    ];
    for (title, kind, ord) in cases {
        let f = run(&[&format!("{title}\nBody text of the document follows here.")]);
        assert_eq!(
            single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
            Some(kind),
            "{title}"
        );
        assert_eq!(
            int(of(&f, &keys::ORDINAL).first().map(|x| &x.value)),
            ord,
            "{title}"
        );
    }
}

#[test]
fn traps_years_dates_phones_pages_prose() {
    let f = run(&[
        "SERVICES AGREEMENT\nThis Agreement dated 2021 is made.\nAgreement No. 2021\nContract No. 12.03.2023\nTel. No. +381 11 123 4567\nPage 5\nNo. of units: 500\nIn order to 1234 the parties.\nUgovor od 12.03.2023.\nThe final payment shall be made after the draft report is approved by both parties in writing.\nInvoice dated 12/05/2023\nPO box 12345",
    ]);
    assert!(own(&f).iter().all(|(_, k)| k.is_empty()), "{:?}", own(&f));
    assert!(cites(&f).is_empty(), "{:?}", cites(&f));
    assert!(of(&f, &keys::PAGE_MARKER).is_empty());
    assert!(
        of(&f, &keys::VERSION_HINT).is_empty(),
        "{:?}",
        of(&f, &keys::VERSION_HINT)
    );
}

#[test]
fn self_reference_is_not_a_cite() {
    let f = run(&[
        "SERVICES AGREEMENT\nText.\nUnder this Agreement dated 1 March 2021 the parties agree.\nOvaj ugovor od 12.03.2023.",
    ]);
    assert!(cites(&f).is_empty(), "{:?}", cites(&f));
}

#[test]
fn ids() {
    let f = run(&["Study D123AC45678 record con_4821 and D123AC45678 again"]);
    assert_eq!(
        single_text(&f, &keys::STUDY_CODE).as_deref(),
        Some("D123AC45678")
    );
    assert_eq!(of(&f, &keys::STUDY_CODE).len(), 1);
    assert_eq!(
        single_text(&f, &keys::CLM_RECORD).as_deref(),
        Some("con_4821")
    );
}

#[test]
fn evidence_points_into_original_text() {
    // NBSP, soft hyphen, zero-width space and a ligature before the number move offsets.
    let original =
        "U\u{00a0}G\u{00ad}O\u{200b}VOR\nﬁrma\u{00a0}d.o.o.\nUgovor\u{00a0}br.\u{00a0}114/23";
    let f = run(&[original]);
    let d = of(&f, &keys::DOC_NUMBER);
    assert_eq!(d.len(), 1);
    let span = d[0].evidence.as_ref().and_then(|e| e.span).unwrap();
    assert_eq!(span.slice(original), "Ugovor\u{00a0}br.\u{00a0}114/23");
    assert_eq!(d[0].evidence.as_ref().and_then(|e| e.page), Some(0));
}

#[test]
fn caps_record_a_problem() {
    let mut body = String::new();
    for i in 0..100 {
        writeln!(body, "Reference to Purchase Order 45000{i:05}").unwrap();
    }
    let mut out = Emitter::new(RefAnalyzer::ID);
    run_with(doc(&["INVOICE", &body]), &Limits::default(), &mut out).unwrap();
    let problems = out.take_problems();
    let f = out.into_parts().0;
    assert_eq!(of(&f, &keys::CITES).len(), quail_refs::MAX_CITES);
    assert_eq!(problems.len(), 1, "{problems:?}");
}

#[test]
fn text_limit_records_a_problem() {
    let limits = Limits {
        max_text_bytes: 30,
        ..Limits::default()
    };
    let mut out = Emitter::new(RefAnalyzer::ID);
    run_with(
        doc(&[
            "UGOVOR\nUgovor br. 114/23",
            "x",
            "Purchase Order 4500012345",
        ]),
        &limits,
        &mut out,
    )
    .unwrap();
    assert_eq!(out.take_problems().len(), 1);
}

#[test]
fn deadline_is_checked() {
    let mut out = Emitter::with_deadline(RefAnalyzer::ID, Instant::now());
    let r = run_with(doc(&["UGOVOR"]), &Limits::default(), &mut out);
    assert!(matches!(r, Err(Error::Timeout)));
}

#[test]
fn hostile_inputs_do_not_panic() {
    let long_num = "Ugovor br. ".to_owned() + &"9".repeat(10_000);
    let inputs: Vec<String> = vec![
        String::new(),
        "\u{0}\u{1}\u{2}".into(),
        "br. br. br. No. No.".into(),
        "Ugovor".into(),
        long_num,
        "Page 9999 of 1".into(),
        "Ugovor br.\n\n\n".into(),
        "AMENDMENT NO. TO THE".into(),
        "ﬁ".repeat(1000),
        "Страна".repeat(500),
        "PO-".repeat(3000),
    ];
    for t in &inputs {
        let _ = run(&[t.as_str(), t.as_str()]);
    }
    // Blocks whose spans are out of range or overlapping.
    let mut d = doc(&["Ugovor br. 114/23"]);
    d.pages[0].blocks = vec![Block {
        span: Span { start: 50, end: 10 },
        kind: BlockKind::Paragraph,
        zone: Zone::Header,
        bbox: None,
    }];
    let mut out = Emitter::new(RefAnalyzer::ID);
    run_with(d, &Limits::default(), &mut out).unwrap();
}

#[test]
fn linear_on_large_pages() {
    let line = "Ugovor br. 114/23 uz Aneks br. 2 pursuant to the Master Services Agreement No. KW-482917 dated 1 March 2021, Page 1 of 2, v2 draft.\n";
    let big = line.repeat(20_000); // ~2.7 MB on one page
    let t = Instant::now();
    let f = run(&[&big]);
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    assert!(!f.is_empty());
}

#[test]
fn look_alikes_are_not_references() {
    let f = run(&[
        "SUPPLY AGREEMENT\nText of the agreement follows below.\nRemit to P.O. Box 910091\nas listed under Executive Order 13224 of the government\nthe product approved under NDA No. 21-748 by the agency\nfiled as EXHIBIT 10.12 to the report",
    ]);
    assert!(cites(&f).is_empty(), "{:?}", cites(&f));
    assert!(own(&f).is_empty(), "{:?}", own(&f));
}

#[test]
fn type_code_in_a_contract_id_names_the_instrument() {
    // The amendment's own filing id carries AMD; the SOW it amends carries SOW.
    let f = run(&[
        "AMENDMENT NO. 3\nThis amendment changes the statement of work listed below.\nContract ID: 731552/SOW/QRST\nThe remaining terms stay in force without change.\nMore text here.\nAnd more text.\nSigned by both parties.\nContract ID: 6619/AMD/QX",
    ]);
    assert_eq!(own(&f), vec![(s("amendment"), s("6619amdqx"))]);
    assert_eq!(
        cites(&f),
        vec![(s("sow"), s("amends"), s("731552sowqrst"), None, None)]
    );
}

#[test]
fn field_lines_are_not_titles() {
    let f = run(&[
        "Napomena: Ugovor o zakupu se primenjuje\nAgreement Number 12965\nCONTENT LICENSE AGREEMENT\nText.",
    ]);
    assert_eq!(
        single_text(&f, &keys::TITLE).as_deref(),
        Some("CONTENT LICENSE AGREEMENT")
    );
    let f = run(&["EXHIBIT C\nFRANCHISE AGREEMENT\nText."]);
    assert_eq!(
        single_text(&f, &keys::TITLE).as_deref(),
        Some("FRANCHISE AGREEMENT")
    );
}

#[test]
fn version_words_in_prose_and_codes_are_not_markers() {
    let long = "x ".repeat(30);
    let f = run(&[&format!(
        "{long}\nThe supplier keeps ISO 27001 and NIST SP 800-171, Rev 2. certifications in force.\nItem ABC-DB-R100-V2 server\nFinal SAP\nThe final date for payment is seven days after the invoice."
    )]);
    assert!(
        of(&f, &keys::VERSION_HINT).is_empty(),
        "{:?}",
        of(&f, &keys::VERSION_HINT)
    );
}

#[test]
fn serbian_framework_agreement_number_and_procurement() {
    let f = run(&["ОКВИРНИ СПОРАЗУМ БР. 97-4/17\nЗА ЈАВНУ НАБАВКУ ЛЕКОВА\n\
         Наручилац и Добављач закључују у поступку јавне набавке бр. 404-1-110/17\n"]);
    assert_eq!(
        single_text(&f, &keys::TITLE).as_deref(),
        Some("ОКВИРНИ СПОРАЗУМ БР. 97-4/17")
    );
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("framework_agreement")
    );
    assert_eq!(own(&f), vec![(s("framework_agreement"), s("97417"))]);
    let c = cites(&f);
    assert!(
        c.iter().any(|r| r.0 == "procurement" && r.2 == "404111017"),
        "{c:?}"
    );
}

#[test]
fn serbian_registry_numbers_in_the_head() {
    let f = run(&[
        "Број акта: 92/16\nДатум: 13.04.2018.\nОБАВЕШТЕЊЕ\nо закљученом оквирном споразуму\n",
    ]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("notice")
    );
    assert!(
        own(&f).contains(&(s("registry"), s("9216"))),
        "{:?}",
        own(&f)
    );
    let f = run(&["ODLUKA O DODELI UGOVORA\nBroj: 404-13/26\nNa osnovu člana 27."]);
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("award_decision")
    );
    assert_eq!(own(&f), vec![(s("registry"), s("4041326"))]);
}

#[test]
fn field_labels_and_header_lines() {
    // A field label naming a contract in the genitive is no title.
    let f = run(&["Naziv naručioca: Grad\nPrvobitna vrednost ugovora\n1.000.000,00"]);
    assert_ne!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("contract")
    );
    // An upper-case kind line in the header zone of page 1 is the title.
    let mut d = doc(&["ОДЛУКА О ИЗМЕНИ УГОВОРА\nНазив наручиоца: Град\nАдреса: улица 2"]);
    d.pages[0].blocks[0].zone = Zone::Header;
    let mut out = Emitter::new(RefAnalyzer::ID);
    run_with(d, &Limits::default(), &mut out).unwrap();
    let f = out.into_parts().0;
    assert_eq!(
        single_text(&f, &keys::TITLE).as_deref(),
        Some("ОДЛУКА О ИЗМЕНИ УГОВОРА")
    );
    assert_eq!(
        single_text(&f, &keys::INSTRUMENT_KIND).as_deref(),
        Some("decision")
    );
}

#[test]
fn serbian_registry_shape() {
    use quail_refs::number::{is_serbian_registry_shape, normalize_contract_number};
    for v in ["97-4/17", "3-1/25", "116-21/2025", "23-5/20-1"] {
        assert!(is_serbian_registry_shape(v), "{v}");
    }
    for v in ["12/2023", "1-2/3", "4/17", "a-1/20", "1-2/123"] {
        assert!(!is_serbian_registry_shape(v), "{v}");
    }
    assert_eq!(normalize_contract_number("97-4/17"), "97417");
}

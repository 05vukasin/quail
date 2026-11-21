//! End-to-end tests over documents built in code: DOCX-style tables (`TableCell` blocks, as
//! `quail-docx` emits them) and PDF-style pages (paragraph blocks with boxes, as `quail-pdf`
//! emits them).

use quail_core::{
    Analyzer, Artifacts, BBox, Blob, Block, BlockKind, Ctx, Document, Emitter, Fact, Format,
    Limits, Page, PageKind, Profile, Span, TextSource, Value, Zone,
};
use quail_items::{ItemLines, keys};

/// Builds a page block by block.
struct PageBuilder {
    text: String,
    blocks: Vec<Block>,
}

impl PageBuilder {
    fn new() -> Self {
        Self {
            text: String::new(),
            blocks: Vec::new(),
        }
    }

    fn push(&mut self, t: &str, kind: BlockKind, zone: Zone, bbox: Option<BBox>) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = u32::try_from(self.text.len()).unwrap();
        self.text.push_str(t);
        let end = u32::try_from(self.text.len()).unwrap();
        self.blocks.push(Block {
            span: Span { start, end },
            kind,
            zone,
            bbox,
        });
    }

    fn table(&mut self, table: u16, rows: &[&[&str]]) {
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if cell.is_empty() {
                    continue;
                }
                self.push(
                    cell,
                    BlockKind::TableCell {
                        table,
                        row: u16::try_from(r).unwrap(),
                        col: u16::try_from(c).unwrap(),
                    },
                    Zone::Table,
                    None,
                );
            }
        }
    }

    fn para(&mut self, t: &str) {
        self.push(t, BlockKind::Paragraph, Zone::Body, None);
    }

    /// A PDF-style block at `x0..x1`, top `y0`, `lines` lines of 12 pt.
    fn block(&mut self, t: &str, x0: f32, x1: f32, y0: f32) {
        let n = t.split('\n').count() as f32;
        self.push(
            t,
            BlockKind::Paragraph,
            Zone::Unknown,
            Some(BBox {
                x0,
                y0,
                x1,
                y1: y0 + 12.0 * n,
            }),
        );
    }

    fn logical(self, index: u32) -> Page {
        Page {
            text: self.text,
            blocks: self.blocks,
            text_source: TextSource::Native,
            ..Page::new(index, PageKind::Logical)
        }
    }

    fn physical(self, index: u32) -> Page {
        Page {
            size: Some((595.0, 842.0)),
            text: self.text,
            blocks: self.blocks,
            text_source: TextSource::Native,
            ..Page::new(index, PageKind::Physical)
        }
    }
}

fn run(doc: Document, profile: Profile) -> Vec<Fact> {
    let blob = Blob::from(Vec::new());
    let mut arts = Artifacts::new();
    arts.insert(doc);
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Docx,
        locator: "test",
        artifacts: &arts,
        facts: &[],
        limits: &limits,
        profile,
    };
    let an = ItemLines::new();
    let mut out = Emitter::new(ItemLines::ID);
    an.run(&ctx, &mut out).unwrap();
    out.facts().to_vec()
}

fn lines(facts: &[Fact]) -> Vec<&Fact> {
    facts.iter().filter(|f| f.key == keys::LINE).collect()
}

fn text<'a>(f: &'a Fact, name: &str) -> Option<&'a str> {
    f.value.field(name).and_then(Value::as_text)
}

fn units(f: &Fact, name: &str) -> Option<(i64, u8)> {
    match f.value.field(name) {
        Some(Value::Money(m)) => Some((m.units, m.scale)),
        _ => None,
    }
}

fn float(f: &Fact, name: &str) -> Option<f64> {
    match f.value.field(name) {
        Some(Value::Float(x)) => Some(*x),
        _ => None,
    }
}

#[test]
fn docx_serbian_offer_table() {
    let mut p = PageBuilder::new();
    p.para("PONUDA br. 12/2026");
    p.para("Sve cene su izražene u dinarima, bez PDV-a.");
    p.table(
        0,
        &[
            &[
                "Р.бр.",
                "Опис услуге",
                "Јед. мере",
                "Количина",
                "Јединична цена",
                "Укупно",
            ],
            &[
                "1.",
                "Čišćenje poslovnog prostora",
                "m2",
                "250",
                "120,00",
                "30.000,00",
            ],
            &["2.", "Pranje prozora", "m2", "80", "90,00", "7.200,00"],
            &[
                "3.",
                "Održavanje zelenih površina",
                "mesec",
                "12",
                "15.000,00",
                "180.000,00",
            ],
            &["", "Ukupno bez PDV-a", "", "", "", "217.200,00"],
            &["", "PDV 20%", "", "", "", "43.440,00"],
            &["", "Ukupno sa PDV-om", "", "", "", "260.640,00"],
        ],
    );
    let facts = run(
        Document {
            pages: vec![p.logical(0)],
        },
        Profile::Standard,
    );
    let l = lines(&facts);
    assert_eq!(l.len(), 3, "{l:#?}");
    let a = l[0];
    assert_eq!(text(a, "description"), Some("Čišćenje poslovnog prostora"));
    assert_eq!(text(a, "desc_key"), Some("ciscenje poslovnog prostora"));
    assert_eq!(text(a, "unit"), Some("m2"));
    assert_eq!(float(a, "quantity"), Some(250.0));
    assert_eq!(units(a, "unit_price"), Some((12_000, 2)));
    assert_eq!(units(a, "total"), Some((3_000_000, 2)));
    assert_eq!(text(a, "currency"), Some("RSD"));
    assert_eq!(a.value.field("vat_included"), Some(&Value::Bool(false)));
    assert_eq!(text(a, "source"), Some("table"));
    assert_eq!(text(l[2], "unit"), Some("month"));
    let ev = a.evidence.as_ref().unwrap();
    assert_eq!(ev.zone, Zone::Table);
    assert_eq!(ev.page, Some(0));
    // Categories: cleaning first.
    let Some(Value::List(cands)) = a.value.field("category_candidates") else {
        panic!("no candidates");
    };
    assert!(!cands.is_empty() && cands.len() <= 3);
    let first = cands[0].field("code").and_then(Value::as_text).unwrap();
    assert!(first.starts_with("909"), "{cands:?}");
    let totals: Vec<_> = facts
        .iter()
        .filter(|f| f.key == keys::TABLE_TOTAL)
        .collect();
    let kinds: Vec<_> = totals.iter().map(|t| text(t, "kind").unwrap()).collect();
    assert_eq!(kinds, ["net", "vat", "gross"]);
    assert!(
        facts
            .iter()
            .any(|f| f.key == keys::COUNT && f.value == Value::Int(3))
    );
}

#[test]
fn docx_english_invoice_and_fast_profile() {
    let mut p = PageBuilder::new();
    p.para("INVOICE No. 2026-0042");
    p.table(
        0,
        &[
            &["Item", "Description", "Qty", "Unit", "Unit Price", "Amount"],
            &[
                "SVC-01",
                "Software development services",
                "40",
                "hours",
                "$95.00",
                "$3,800.00",
            ],
            &[
                "SVC-02",
                "Project management",
                "10",
                "hours",
                "$110.00",
                "$1,100.00",
            ],
            &["", "", "", "", "Subtotal", "$4,900.00"],
            &["", "", "", "", "Sales Tax", "$0.00"],
            &["", "", "", "", "Total", "$4,900.00"],
        ],
    );
    let doc = Document {
        pages: vec![p.logical(0)],
    };
    let facts = run(doc.clone(), Profile::Fast);
    let l = lines(&facts);
    assert_eq!(l.len(), 2, "{l:#?}");
    assert_eq!(text(l[0], "code"), Some("SVC-01"));
    assert_eq!(text(l[0], "unit"), Some("h"));
    assert_eq!(units(l[0], "unit_price"), Some((9_500, 2)));
    assert_eq!(text(l[0], "currency"), Some("USD"));
    assert!(l[0].value.field("category_candidates").is_none());
    assert!(l[0].conf > 0.9);
    let totals: Vec<_> = facts
        .iter()
        .filter(|f| f.key == keys::TABLE_TOTAL)
        .collect();
    assert_eq!(totals.len(), 3);
    let std = run(doc, Profile::Standard);
    let Some(Value::List(c)) = lines(&std)[0].value.field("category_candidates") else {
        panic!("no candidates");
    };
    let first = c[0].field("code").and_then(Value::as_text).unwrap();
    assert!(first.starts_with("72"), "{c:?}");
}

#[test]
fn pdf_layout_table_from_column_blocks() {
    // A price table as quail-pdf lays it out: the header cells on one baseline as separate
    // blocks, then one block per column (the description column has a wrapped line).
    let mut p = PageBuilder::new();
    p.block("Price schedule", 50.0, 200.0, 60.0);
    for (t, x0, x1) in [
        ("Description", 50.0, 120.0),
        ("Qty", 300.0, 320.0),
        ("Unit price", 360.0, 420.0),
        ("Amount", 470.0, 520.0),
    ] {
        p.block(t, x0, x1, 100.0);
    }
    p.block(
        "Network audit\nFirewall configuration\nOn-site support",
        50.0,
        210.0,
        120.0,
    );
    p.block("1\n2\n16", 308.0, 320.0, 120.0);
    p.block("1,500.00\n750.00\n90.00", 370.0, 420.0, 120.0);
    p.block("1,500.00\n1,500.00\n1,440.00", 470.0, 520.0, 120.0);
    p.block("Total 4,440.00 EUR", 400.0, 520.0, 170.0);
    let facts = run(
        Document {
            pages: vec![p.physical(0)],
        },
        Profile::Fast,
    );
    let l = lines(&facts);
    assert_eq!(l.len(), 3, "{l:#?}");
    assert_eq!(text(l[1], "description"), Some("Firewall configuration"));
    assert_eq!(float(l[1], "quantity"), Some(2.0));
    assert_eq!(units(l[1], "unit_price"), Some((75_000, 2)));
    assert_eq!(units(l[1], "total"), Some((150_000, 2)));
    assert_eq!(text(l[2], "source"), Some("layout"));
    assert_eq!(text(l[2], "currency"), Some("EUR"));
    let ev = l[0].evidence.as_ref().unwrap();
    assert!(ev.bbox.is_some());
    assert_eq!(ev.zone, Zone::Table);
}

#[test]
fn pdf_rate_card_lines_and_prose() {
    let mut p = PageBuilder::new();
    p.block("Rate card 2026", 50.0, 200.0, 60.0);
    p.block(
        "Senior consultant – 85 EUR/h\nJunior consultant – 45 EUR/h\nTravel day – 400 EUR per day",
        50.0,
        300.0,
        100.0,
    );
    p.block(
        "The total value of this agreement shall not exceed EUR 50,000.00 and is payable within\n\
         thirty days of the invoice date, as set out in Article 4 of the agreement.",
        50.0,
        540.0,
        200.0,
    );
    let facts = run(
        Document {
            pages: vec![p.physical(0)],
        },
        Profile::Fast,
    );
    let l = lines(&facts);
    assert_eq!(l.len(), 3, "{l:#?}");
    assert_eq!(text(l[0], "description"), Some("Senior consultant"));
    assert_eq!(text(l[0], "unit"), Some("h"));
    assert_eq!(text(l[2], "unit"), Some("day"));
    assert_eq!(text(l[2], "source"), Some("text"));
}

#[test]
fn table_continued_on_next_page() {
    let mut p0 = PageBuilder::new();
    p0.table(
        0,
        &[
            &["Naziv", "JM", "Kol.", "Cena", "Iznos"],
            &["Toner crni", "kom", "5", "4.500,00", "22.500,00"],
        ],
    );
    let mut p1 = PageBuilder::new();
    p1.table(0, &[&["Papir A4", "pak", "20", "550,00", "11.000,00"]]);
    let facts = run(
        Document {
            pages: vec![p0.logical(0), p1.logical(1)],
        },
        Profile::Fast,
    );
    let l = lines(&facts);
    assert_eq!(l.len(), 2);
    assert_eq!(text(l[1], "unit"), Some("pack"));
    assert_eq!(l[1].evidence.as_ref().unwrap().page, Some(1));
}

#[test]
fn hostile_layouts_do_not_panic() {
    let mut p = PageBuilder::new();
    for i in 0..3000 {
        let x = (i % 37) as f32 * 13.0;
        let y = (i / 37) as f32 * 3.0;
        p.block(&format!("{i},00 x {i}"), x, x + 10.0, y);
    }
    p.push(
        "NaN 1.200,00",
        BlockKind::Paragraph,
        Zone::Unknown,
        Some(BBox {
            x0: f32::NAN,
            y0: f32::INFINITY,
            x1: -1e30,
            y1: f32::NAN,
        }),
    );
    p.table(
        0,
        &[
            &["", "1"],
            &["9", "", "", "", "", "", "", "", "", "", "", "x"],
        ],
    );
    let _ = run(
        Document {
            pages: vec![p.physical(0)],
        },
        Profile::Standard,
    );
    let _ = run(Document::default(), Profile::Standard);
}

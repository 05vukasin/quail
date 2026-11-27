//! Generated inputs shared by the integration tests: DOCX and PDF built in code, no real
//! documents.
#![allow(dead_code, clippy::cast_precision_loss, clippy::similar_names)]

use std::io::{Cursor, Write};

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// A minimal DOCX: one section with a title paragraph, a body paragraph and core properties.
pub fn docx(title: &str, body: &str) -> Vec<u8> {
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_owned(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#.to_owned(),
        ),
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="{W}"><w:body><w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>{title}</w:t></w:r></w:p><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"#
            ),
        ),
        (
            "docProps/core.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></cp:coreProperties>"#
            ),
        ),
    ];
    for (name, data) in parts {
        z.start_file(name, opts).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}

/// US Letter, points.
const W: f32 = 612.0;
const H: f32 = 792.0;

/// One page of a generated PDF.
pub enum PdfPage<'a> {
    /// Lines of Helvetica text, top to bottom.
    Text(&'a [&'a str]),
    /// One grayscale image drawn over the whole page, no text: what a scanner produces.
    Scan,
}

/// A PDF with these pages and an Info dictionary (`Title`, `Producer`).
pub fn pdf(pages: &[PdfPage<'_>], title: &str) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    // A 64x64 diagonal gradient with a dark block: a non-flat image, so the pHash has
    // something to hash.
    let side = 64u32;
    let pixels: Vec<u8> = (0..side * side)
        .map(|i| {
            let (x, y) = (i % side, i / side);
            if (16..32).contains(&x) && (20..44).contains(&y) {
                10
            } else {
                u8::try_from((x + y) * 2).unwrap_or(255)
            }
        })
        .collect();
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => i64::from(side),
            "Height" => i64::from(side),
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        },
        pixels,
    ));
    let mut kids = Vec::new();
    for page in pages {
        let mut ops = Vec::new();
        match page {
            PdfPage::Scan => {
                ops.push(Operation::new("q", vec![]));
                ops.push(Operation::new(
                    "cm",
                    vec![W.into(), 0.into(), 0.into(), H.into(), 0.into(), 0.into()],
                ));
                ops.push(Operation::new("Do", vec!["Im1".into()]));
                ops.push(Operation::new("Q", vec![]));
            }
            PdfPage::Text(lines) => {
                for (i, l) in lines.iter().enumerate() {
                    let top = 72.0 + 16.0 * i as f32;
                    ops.push(Operation::new("BT", vec![]));
                    ops.push(Operation::new("Tf", vec!["F1".into(), 11.into()]));
                    ops.push(Operation::new("Td", vec![72.into(), (H - top).into()]));
                    ops.push(Operation::new(
                        "Tj",
                        vec![Object::string_literal(l.as_bytes().to_vec())],
                    ));
                    ops.push(Operation::new("ET", vec![]));
                }
            }
        }
        let content = Content { operations: ops };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        kids.push(page_id.into());
    }
    let count = i64::try_from(kids.len()).unwrap();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => font_id },
                "XObject" => dictionary! { "Im1" => image_id },
            },
            "MediaBox" => vec![0.into(), 0.into(), W.into(), H.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    let info_id = doc.add_object(dictionary! {
        "Title" => Object::string_literal(title),
        "Producer" => Object::string_literal("quail test generator"),
    });
    doc.trailer.set("Root", catalog_id);
    doc.trailer.set("Info", info_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// Body lines of a contract-like page: identifiers, dates, amounts, a document number.
pub const CONTRACT: &[&str] = &[
    "SUPPLY AGREEMENT No. 114/23",
    "This Agreement is made between Alpha Widgets LLC (the Supplier)",
    "and Beta Components GmbH (the Customer).",
    "Effective Date: March 1, 2021. The supplier shall deliver the services",
    "described in this agreement within thirty days of the effective date,",
    "and the customer shall pay every invoice within forty five days of receipt.",
    "Payments go to IBAN DE89 3704 0044 0532 0130 00. Total: EUR 4,200.50.",
];

/// A PDF whose pages are the first `pages` pages of `pdf` rendered at `dpi` (8-bit gray) and
/// drawn as one image each, without a text layer: what scanning the printed document gives.
#[cfg(feature = "pdf")]
pub fn scanned(pdf: &[u8], pages: u32, dpi: f32) -> Vec<u8> {
    use quail_core::{Blob, Limits, PageRaster};
    let raster = quail_pdf::PdfRaster::new(Blob::from(pdf.to_vec()));
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for p in 0..pages {
        let img = raster.render(p, dpi, &Limits::default()).unwrap();
        let gray: Vec<u8> = match img.channels {
            1 => img.data,
            n => img
                .data
                .chunks_exact(usize::from(n))
                .map(|px| px[0] / 3 + px[1] / 3 + px[2] / 3)
                .collect(),
        };
        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => i64::from(img.width),
                "Height" => i64::from(img.height),
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            gray,
        ));
        let ops = vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![W.into(), 0.into(), 0.into(), H.into(), 0.into(), 0.into()],
            ),
            Operation::new("Do", vec!["Im1".into()]),
            Operation::new("Q", vec![]),
        ];
        let content = Content { operations: ops };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image_id } },
        });
        kids.push(page_id.into());
    }
    let count = i64::try_from(kids.len()).unwrap();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
            "MediaBox" => vec![0.into(), 0.into(), W.into(), H.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A Serbian service contract (Latin, written without diacritics, as Helvetica in `WinAnsi`
/// cannot draw `č ć đ`): subject clause, net, VAT and gross, a penalty that is not the price.
pub const PAY_SR_CONTRACT: &[&str] = &[
    "UGOVOR O PRUZANJU USLUGA ODRZAVANJA SOFTVERA",
    "zakljucen izmedju Alfa Soft d.o.o. i Beta Trgovina d.o.o.",
    "Clan 1. Predmet ugovora",
    "Predmet ovog ugovora je pruzanje usluga odrzavanja poslovnog softvera Narucioca.",
    "Clan 2. Cena",
    "Ukupna vrednost ugovora iznosi 1.234.567,89 RSD bez PDV-a.",
    "PDV (20%) iznosi 246.913,58 RSD.",
    "Ukupna vrednost ugovora sa PDV-om iznosi 1.481.481,47 RSD.",
    "Ugovorna kazna iznosi 50.000,00 RSD.",
];

/// An English monthly-fee agreement in EUR.
pub const PAY_EN_MONTHLY: &[&str] = &[
    "SOFTWARE SUBSCRIPTION AGREEMENT",
    "This Agreement is made between Acme Ltd and Example Corp.",
    "Scope of Services: hosting and support of the online shop of the Client.",
    "The Client shall pay a monthly fee of EUR 2,500.00 excluding VAT.",
];

/// An English invoice in USD with line items, subtotal, tax and total.
pub const PAY_EN_INVOICE: &[&str] = &[
    "INVOICE No. 2026-0042",
    "Bill to: Example Corp",
    "Description Qty Unit Unit price Amount",
    "Consulting services 10 h 150.00 1,500.00",
    "Travel expenses 1 pcs 300.00 300.00",
    "Subtotal: $1,800.00",
    "Tax (8%): $144.00",
    "Total: $1,944.00",
];

/// A Serbian invoice in RSD with line items and net, VAT and the amount due.
pub const PAY_SR_INVOICE: &[&str] = &[
    "FAKTURA br. 12/2026",
    "Kupac: Beta Trgovina d.o.o.",
    "Opis Kolicina JM Cena Iznos",
    "Odrzavanje servera 10 h 3.000,00 30.000,00",
    "Licenca za antivirus 5 kom 2.000,00 10.000,00",
    "Ukupno bez PDV-a: 40.000,00 RSD",
    "PDV 20%: 8.000,00 RSD",
    "Ukupno za uplatu: 48.000,00 RSD",
];

/// A Serbian contract without a price.
pub const PAY_SR_NO_PRICE: &[&str] = &[
    "UGOVOR O POSLOVNOJ SARADNJI",
    "Predmet ovog ugovora je uredjivanje medjusobnih odnosa ugovornih strana.",
    "Ugovor stupa na snagu danom potpisivanja i vazi 2 godine.",
    "Rok za odgovor je 30 dana od 15.01.2026.",
];

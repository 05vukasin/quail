//! Generated seed inputs: DOCX and PPTX packages from small builders, PDFs from lopdf and
//! from raw bytes, HTML shaped like EDGAR filings and old web pages, plain text full of
//! awkward Unicode. All content is synthetic.

// Seeds are built once per run: readable `map(format!).collect()` over speed.
#![allow(clippy::format_collect)]

use lopdf::content::{Content, Operation};
use lopdf::{Document as LoDoc, Object, Stream, dictionary};

use crate::pdfgen::{self, FONT, page_pdf, stream};
use crate::zipfmt::{self, Entry, Eocd, Method};

/// The family of an input, which decides the structure-aware mutators and the extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A zip package (DOCX).
    Docx,
    /// A PDF.
    Pdf,
    /// Plain text.
    Text,
    /// A zip package (PPTX).
    Pptx,
    /// HTML.
    Html,
}

impl Kind {
    /// Guess from the bytes.
    pub fn of(bytes: &[u8]) -> Self {
        let head = &bytes[..bytes.len().min(1024)];
        if bytes.starts_with(b"PK") {
            if pdfgen::find(bytes, b"ppt/presentation.xml").is_some() {
                Self::Pptx
            } else {
                Self::Docx
            }
        } else if pdfgen::find(head, b"%PDF").is_some() {
            Self::Pdf
        } else if [&b"<html"[..], b"<!doctype html", b"<body", b"<table"]
            .iter()
            .any(|t| pdfgen::find(&head.to_ascii_lowercase(), t).is_some())
        {
            Self::Html
        } else {
            Self::Text
        }
    }

    /// File extension.
    pub fn ext(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Pdf => "pdf",
            Self::Text => "txt",
            Self::Pptx => "pptx",
            Self::Html => "html",
        }
    }

    /// Lowercase name.
    pub fn as_str(self) -> &'static str {
        self.ext()
    }
}

/// Synthetic business-like text that wakes up the signal, reference and party analyzers.
pub const BUSINESS: &[&str] = &[
    "UGOVOR O KUPOPRODAJI br. 12/2024",
    "Zaključen dana 15.03.2024. godine u Beogradu između:",
    "1. Primer d.o.o. Beograd, Ulica Bb 1, PIB 100000024, MB 07000005 (u daljem tekstu: Prodavac)",
    "2. Uzorak a.d. Novi Sad, PIB 100000032 (u daljem tekstu: Kupac)",
    "Ukupna cena iznosi 1.234.567,89 RSD (slovima: milion...) odnosno EUR 12,500.00.",
    "Plaćanje na račun 160-0000000000001-23, IBAN RS35260005601001611379, SWIFT DBDBRSBG.",
    "Kontakt: office@example.com, tel. +381 11 123 4567, www.example.com",
    "Ovaj Aneks br. 2 uz Ugovor broj 45-2023 od 01.02.2023. stupa na snagu 2024-04-01.",
    "AGREEMENT No. AG-2024/0077 between Example Trading Ltd (the \"Supplier\") and Sample GmbH (the \"Buyer\").",
    "VAT DE136695976, IBAN DE89370400440532013000, amount USD 1,000,000.00 due on March 3, 2025.",
    "Члан 5. Купац се обавезује да плати износ од 250.000,00 динара до 31.12.2024. године.",
    "Strana 1 od 3 — Verzija 1.2 — Prilog 1 uz Okvirni sporazum br. 7/2022",
];

/// Hard Unicode: combining marks, bidi controls, ZWJ sequences, case-mapping length
/// changes, NFKC expansions, non-ASCII digits, noncharacters, surrogate-range escapes.
pub const TRICKY: &[&str] = &[
    "e\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}\u{309}\u{30a}",
    "\u{202e}abc\u{202c} \u{2067}xyz\u{2069} \u{200f}\u{200e}",
    "👩‍👩‍👧‍👦 🏳️‍🌈 🇷🇸 \u{fe0f}\u{fe0e}",
    "ß ẞ İ ı ǅ ŉ ﬁ ﬃ ﬆ Å Ω K",
    "１２３．４５６，７８ ٣٤٥٦ ०१२३ ১২৩ ¹²³ ½ Ⅻ",
    "\u{fffe}\u{ffff}\u{fdd0}\u{e000}\u{10ffff}",
    "\u{0}\u{1}\u{7}\u{8}\u{b}\u{c}\u{1b}[31m\u{7f}\u{85}\u{2028}\u{2029}",
    "\u{feff}\u{200b}\u{200c}\u{200d}\u{2060}\u{180e}",
    "ﷺ ﷽ ꧅ 𒐫 𒈙 ௌ ᄀᄀᄀ각",
    "Ǆ\u{345}\u{345}\u{345} ΐ ΰ և ﬓ ẖ",
    "dž DŽ Dž lj LJ nj NJ đ Đ ć č š ž",
    "1.000.000.000.000.000.000.000,00 EUR 31.02.2024 99.99.9999 0000-00-00",
];

/// Joins lines into paragraphs of `n` lines with blank lines between.
fn text_body(lines: &[&str]) -> String {
    lines.join("\n")
}

// -- DOCX -------------------------------------------------------------------------------------

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A paragraph with plain text.
pub fn p(text: &str) -> String {
    format!(
        r#"<w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        esc(text)
    )
}

fn wpart(root: &str, inner: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} xmlns:w="{W}" xmlns:r="{R}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">{inner}</w:{root}>"#
    )
}

/// A DOCX package under construction.
pub struct Docx {
    body: String,
    parts: Vec<(String, Vec<u8>)>,
    rels: Vec<(String, String, String)>,
    overrides: Vec<(String, String)>,
    store: bool,
}

impl Docx {
    /// A package whose body is `body` (inner XML of `w:body`).
    pub fn new(body: &str) -> Self {
        Self {
            body: body.to_owned(),
            parts: Vec::new(),
            rels: Vec::new(),
            overrides: Vec::new(),
            store: false,
        }
    }

    /// Store entries instead of deflating them.
    pub fn stored(mut self) -> Self {
        self.store = true;
        self
    }

    /// A part related from the main document (`kind` is the relationship type suffix,
    /// e.g. `styles`), with its content type override.
    pub fn related(mut self, id: &str, kind: &str, target: &str, ct: &str, content: &str) -> Self {
        self.rels
            .push((id.to_owned(), format!("{REL}/{kind}"), target.to_owned()));
        self.parts
            .push((format!("word/{target}"), content.as_bytes().to_vec()));
        self.overrides.push((
            format!("/word/{target}"),
            format!("application/vnd.openxmlformats-officedocument.wordprocessingml.{ct}+xml"),
        ));
        self
    }

    /// Any part, unrelated.
    pub fn part(mut self, name: &str, content: impl Into<Vec<u8>>) -> Self {
        self.parts.push((name.to_owned(), content.into()));
        self
    }

    /// The zip bytes.
    pub fn build(self) -> Vec<u8> {
        let mut ct = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#,
        );
        for (part, t) in &self.overrides {
            ct.push_str(&format!(
                r#"<Override PartName="{part}" ContentType="{t}"/>"#
            ));
        }
        ct.push_str("</Types>");
        let root_rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>"#
        );
        let mut doc_rels =
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PKG}">"#);
        for (id, t, target) in &self.rels {
            let mode = if target.starts_with("http") {
                r#" TargetMode="External""#
            } else {
                ""
            };
            doc_rels.push_str(&format!(
                r#"<Relationship Id="{id}" Type="{t}" Target="{target}"{mode}/>"#
            ));
        }
        doc_rels.push_str("</Relationships>");
        let core = r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Ugovor — пример ✓</dc:title><dc:creator>Test Author</dc:creator><cp:lastModifiedBy>Editor ß</cp:lastModifiedBy><cp:revision>7</cp:revision><dcterms:created xsi:type="dcterms:W3CDTF">2024-03-15T10:00:00Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2024-03-16T11:30:00Z</dcterms:modified><cp:keywords>a; b; c</cp:keywords></cp:coreProperties>"#;
        let app = r#"<?xml version="1.0" encoding="UTF-8"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Microsoft Office Word</Application><AppVersion>16.0000</AppVersion><Pages>3</Pages><Words>420</Words><TotalTime>12</TotalTime><Company>Example</Company><Template>Normal.dotm</Template></Properties>"#;
        let mut entries = vec![
            Entry::new("[Content_Types].xml", ct),
            Entry::new("_rels/.rels", root_rels),
            Entry::new(
                "word/document.xml",
                wpart("document", &format!("<w:body>{}</w:body>", self.body)),
            ),
            Entry::new("word/_rels/document.xml.rels", doc_rels),
            Entry::new("docProps/core.xml", core),
            Entry::new("docProps/app.xml", app),
        ];
        for (name, data) in self.parts {
            entries.push(Entry::new(name, data));
        }
        if self.store {
            for e in &mut entries {
                e.method = Method::Store;
            }
        }
        zipfmt::write(&entries, Eocd::default(), b"")
    }
}

fn business_body() -> String {
    let mut b = String::new();
    b.push_str(
        r#"<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>UGOVOR</w:t></w:r></w:p>"#,
    );
    for l in BUSINESS {
        b.push_str(&p(l));
    }
    b
}

fn table(rows: usize, cols: usize, cell: &dyn Fn(usize, usize) -> String) -> String {
    let mut t =
        String::from(r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:tblGrid>"#);
    for _ in 0..cols {
        t.push_str(r#"<w:gridCol w:w="2000"/>"#);
    }
    t.push_str("</w:tblGrid>");
    for r in 0..rows {
        t.push_str("<w:tr>");
        for c in 0..cols {
            t.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/>{}</w:tcPr>{}</w:tc>"#,
                if c == 0 && r == 0 {
                    r#"<w:gridSpan w:val="1"/><w:vMerge w:val="restart"/>"#
                } else {
                    ""
                },
                cell(r, c)
            ));
        }
        t.push_str("</w:tr>");
    }
    t.push_str("</w:tbl>");
    t
}

const STYLES: &str = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="sr-Latn-RS"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Title"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="List"><w:name w:val="List Paragraph"/><w:basedOn w:val="Heading1"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;

const NUMBERING: &str = r#"<w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="multilevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2)"/></w:lvl><w:lvl w:ilvl="2"><w:start w:val="3"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%1.%2.%3"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num>"#;

fn numbered(num: u32, ilvl: u32, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num}"/></w:numPr></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>"#,
        esc(text)
    )
}

/// Every generated DOCX seed.
pub fn docx_seeds() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    v.push(Docx::new(&business_body()).build());
    v.push(Docx::new(&business_body()).stored().build());
    // tables, nested tables, merged cells
    let inner = table(2, 2, &|r, c| p(&format!("inner {r}.{c} 1.000,00 RSD")));
    let outer = table(3, 3, &|r, c| {
        if r == 1 && c == 1 {
            format!("{inner}{}", p("after inner"))
        } else {
            p(BUSINESS[(r * 3 + c) % BUSINESS.len()])
        }
    });
    v.push(Docx::new(&format!("{}{outer}{}", p("Tabela 1"), p("kraj"))).build());
    // styles + numbering
    let mut body = String::new();
    for i in 0..6u32 {
        body.push_str(&numbered(
            1 + i % 2,
            i % 3,
            BUSINESS[i as usize % BUSINESS.len()],
        ));
    }
    body.push_str(
        r#"<w:p><w:pPr><w:pStyle w:val="List"/></w:pPr><w:r><w:t>styled list</w:t></w:r></w:p>"#,
    );
    body.push_str(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Član 1.</w:t></w:r></w:p>"#,
    );
    v.push(
        Docx::new(&body)
            .related(
                "rId8",
                "styles",
                "styles.xml",
                "styles",
                &wpart("styles", STYLES),
            )
            .related(
                "rId9",
                "numbering",
                "numbering.xml",
                "numbering",
                &wpart("numbering", NUMBERING),
            )
            .build(),
    );
    // headers, footers, notes, comments, sections
    let sect = r#"<w:sectPr><w:headerReference w:type="default" r:id="rId10"/><w:footerReference w:type="default" r:id="rId11"/><w:pgSz w:w="11906" w:h="16838"/></w:sectPr>"#;
    let body = format!(
        "{}<w:p><w:r><w:t>with note</w:t></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r><w:r><w:endnoteReference w:id=\"3\"/></w:r><w:commentRangeStart w:id=\"0\"/><w:r><w:t>commented</w:t></w:r><w:commentRangeEnd w:id=\"0\"/><w:r><w:commentReference w:id=\"0\"/></w:r></w:p><w:p><w:pPr>{sect}</w:pPr></w:p>{}<w:sectPr><w:pgSz w:w=\"16838\" w:h=\"11906\" w:orient=\"landscape\"/></w:sectPr>",
        p(BUSINESS[0]),
        p(BUSINESS[4])
    );
    v.push(
        Docx::new(&body)
            .related("rId10", "header", "header1.xml", "header", &wpart("hdr", &p("Primer d.o.o. — zaglavlje")))
            .related("rId11", "footer", "footer1.xml", "footer", &wpart("ftr", &p("Strana 1 od 3")))
            .related("rId12", "footnotes", "footnotes.xml", "footnotes", &wpart("footnotes", &format!(r#"<w:footnote w:type="separator" w:id="-1"><w:p/></w:footnote><w:footnote w:id="2">{}</w:footnote>"#, p("Napomena uz član 2."))))
            .related("rId13", "endnotes", "endnotes.xml", "endnotes", &wpart("endnotes", &format!(r#"<w:endnote w:id="3">{}</w:endnote>"#, p("Endnote text"))))
            .related("rId14", "comments", "comments.xml", "comments", &wpart("comments", &format!(r#"<w:comment w:id="0" w:author="Reviewer" w:date="2024-03-15T10:00:00Z" w:initials="R">{}</w:comment>"#, p("Proveriti iznos."))))
            .build(),
    );
    // fields, hyperlinks, tracked changes, content controls, alternate content, bidi
    let body = format!(
        concat!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE \* MERGEFORMAT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
            r#"<w:p><w:fldSimple w:instr=" DATE \@ &quot;dd.MM.yyyy&quot; "><w:r><w:t>15.03.2024</w:t></w:r></w:fldSimple></w:p>"#,
            r#"<w:p><w:hyperlink r:id="rId20" w:history="1"><w:r><w:t>example.com</w:t></w:r></w:hyperlink><w:hyperlink w:anchor="bm1"><w:r><w:t>anchor</w:t></w:r></w:hyperlink><w:bookmarkStart w:id="1" w:name="bm1"/><w:bookmarkEnd w:id="1"/></w:p>"#,
            r#"<w:p><w:ins w:id="5" w:author="A" w:date="2024-01-01T00:00:00Z"><w:r><w:t>inserted</w:t></w:r></w:ins><w:del w:id="6" w:author="B" w:date="2024-01-02T00:00:00Z"><w:r><w:delText>deleted</w:delText></w:r></w:del><w:r w:rsidR="00A1B2C3"><w:t>kept</w:t></w:r></w:p>"#,
            r#"<w:sdt><w:sdtPr><w:alias w:val="Datum"/><w:tag w:val="date"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>01.01.2025</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
            r#"<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:inline><a:graphic><a:graphicData><w:txbxContent>{}</w:txbxContent></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:pict><w:txbxContent>{}</w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:bidi/></w:pPr><w:r><w:rPr><w:rtl/></w:rPr><w:t>مرحبا 123</w:t></w:r><w:r><w:tab/><w:t>tab</w:t><w:br/><w:t>br</w:t><w:cr/><w:sym w:font="Wingdings" w:char="F0FC"/><w:noBreakHyphen/><w:softHyphen/></w:r></w:p>"#,
            "{}"
        ),
        p("in text box"),
        p("fallback box"),
        p(BUSINESS[8])
    );
    v.push(
        Docx::new(&body)
            .related("rId20", "hyperlink", "http://example.com/a?b=c", "x", "")
            .build(),
    );
    // Cyrillic + tricky unicode, custom props
    let mut body = String::new();
    for t in TRICKY {
        body.push_str(&p(t));
    }
    body.push_str(&p(BUSINESS[10]));
    v.push(
        Docx::new(&body)
            .part(
                "docProps/custom.xml",
                r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="Broj"><vt:lpwstr>12/2024</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="3" name="Iznos"><vt:r8>1234.5</vt:r8></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="4" name="Datum"><vt:filetime>2024-03-15T00:00:00Z</vt:filetime></property></Properties>"#,
            )
            .build(),
    );
    // many small paragraphs (amplification target for mutators)
    let mut body = String::new();
    for i in 0..300 {
        body.push_str(&p(BUSINESS[i % BUSINESS.len()]));
    }
    v.push(Docx::new(&body).build());
    v
}

// -- PDF --------------------------------------------------------------------------------------

/// A lopdf document of `pages`, each a list of text lines.
fn lopdf_doc(pages: &[Vec<&str>], compress: bool, info: bool) -> Vec<u8> {
    let mut doc = LoDoc::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let image_id = doc.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 4, "Height" => 4, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 },
        vec![0x80; 16],
    ));
    let mut kids = Vec::new();
    for lines in pages {
        let mut ops = vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    50.into(),
                    0.into(),
                    0.into(),
                    50.into(),
                    500.into(),
                    700.into(),
                ],
            ),
            Operation::new("Do", vec!["Im1".into()]),
            Operation::new("Q", vec![]),
        ];
        for (i, l) in lines.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let y = 740.0 - 14.0 * i as f32;
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new("Tf", vec!["F1".into(), 11.into()]));
            ops.push(Operation::new("Td", vec![72.into(), y.into()]));
            // WinAnsi: keep the Latin-1 subset, others become '?'
            let bytes: Vec<u8> = l
                .chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                .collect();
            if i % 3 == 2 {
                ops.push(Operation::new(
                    "TJ",
                    vec![Object::Array(vec![
                        Object::string_literal(bytes[..bytes.len() / 2].to_vec()),
                        Object::Integer(-250),
                        Object::string_literal(bytes[bytes.len() / 2..].to_vec()),
                    ])],
                ));
            } else {
                ops.push(Operation::new("Tj", vec![Object::string_literal(bytes)]));
            }
            ops.push(Operation::new("ET", vec![]));
        }
        let Ok(content) = (Content { operations: ops }).encode() else {
            continue;
        };
        let mut s = Stream::new(dictionary! {}, content);
        if compress {
            let _ = s.compress();
        }
        let cid = doc.add_object(s);
        let pid = doc
            .add_object(dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => cid });
        kids.push(pid.into());
    }
    let count = i64::try_from(kids.len()).unwrap_or(0);
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => count,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id }, "XObject" => dictionary! { "Im1" => image_id } },
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    if info {
        let info_id = doc.add_object(dictionary! {
            "Producer" => Object::string_literal("quail-fuzz seed"),
            "Author" => Object::string_literal("Test Author"),
            "Title" => Object::String(vec![0xfe, 0xff, 0, b'U', 0x04, 0x1f, 0x04, 0x40], lopdf::StringFormat::Hexadecimal),
            "CreationDate" => Object::string_literal("D:20240315100000+01'00'"),
            "ModDate" => Object::string_literal("D:20240316"),
        });
        doc.trailer.set("Info", info_id);
    }
    let mut out = Vec::new();
    if doc.save_to(&mut out).is_err() {
        return Vec::new();
    }
    out
}

/// A Type0 font with an Identity-H encoding and a `ToUnicode` `CMap`.
fn cid_font_pdf() -> Vec<u8> {
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /X def 1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfrange <0001> <001A> <0041> <0100> <0110> [<0416> <0436> <00DF> <FB01> <D83D DE00>] endbfrange 1 beginbfchar <0020> <0020> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let content = b"BT /F2 12 Tf 72 700 Td <000100020003002000040005> Tj 0 -20 Td [<0100> -100 <0101> 200 <0102>] TJ 0 -20 Td <0103 0104> Tj ET";
    page_pdf(
        content,
        b"/Font << /F2 10 0 R >>",
        vec![
            (10, b"<< /Type /Font /Subtype /Type0 /BaseFont /Fake /Encoding /Identity-H /DescendantFonts [11 0 R] /ToUnicode 12 0 R >>".to_vec()),
            (11, b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Fake /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 500 /W [1 [600 600] 256 [700]] >>".to_vec()),
            (12, stream(cmap, b"", true)),
        ],
    )
}

/// Every generated PDF seed (well-formed and mildly hostile).
pub fn pdf_seeds() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let p1: Vec<&str> = BUSINESS[..6].to_vec();
    let p2: Vec<&str> = BUSINESS[6..].to_vec();
    v.push(lopdf_doc(&[p1.clone(), p2.clone()], true, true));
    v.push(lopdf_doc(std::slice::from_ref(&p1), false, false));
    v.push(lopdf_doc(
        &[p2, p1.clone(), vec!["Strana 3 od 3"]],
        true,
        false,
    ));
    v.push(cid_font_pdf());
    // form XObject drawn twice, nested once
    v.push(page_pdf(
        b"/X Do /X Do BT /F1 10 Tf 72 72 Td (footer 1/1) Tj ET",
        b"/XObject << /X 10 0 R >> /Font << /F1 12 0 R >>",
        vec![
            (10, stream(b"q /Y Do Q BT /F1 12 Tf 72 700 Td (in form) Tj ET", b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Y 11 0 R >> /Font << /F1 12 0 R >> >>", true)),
            (11, stream(b"BT /F1 9 Tf 72 600 Td (nested form 15.03.2024) Tj ET", b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 12 0 R >> >>", true)),
            (12, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()),
        ],
    ));
    // object stream holding the page, xref without the page entry (needs the parser's object streams)
    {
        let inner = b"3 0 << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Contents 4 0 R >>";
        let objs = vec![
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (
                4,
                stream(
                    b"BT /F1 12 Tf 72 700 Td (object stream page) Tj ET",
                    b"",
                    true,
                ),
            ),
            (5, stream(inner, b"/Type /ObjStm /N 1 /First 4", true)),
        ];
        v.push(pdfgen::build(&objs, 1, false));
    }
    // incremental update changing the content
    {
        let mut base = page_pdf(b"BT /F1 12 Tf 72 700 Td (version 1) Tj ET", FONT, vec![]);
        let prev = pdfgen::rfind(&base, b"\nxref\n").map_or(0, |p| p + 1);
        base.extend_from_slice(b"4 0 obj\n");
        base.extend_from_slice(&stream(
            b"BT /F1 12 Tf 72 700 Td (version 2 Verzija 2.0) Tj ET",
            b"",
            true,
        ));
        base.extend_from_slice(b"\nendobj\n");
        pdfgen::append_xref(&mut base, 1, Some(prev));
        v.push(base);
    }
    // an inline image, an annotation, an AcroForm, outlines
    v.push(page_pdf(
        b"q 10 0 0 10 100 100 cm BI /W 4 /H 4 /CS /G /BPC 8 ID \x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80 EI Q BT /F1 12 Tf 72 700 Td (inline image page) Tj ET",
        FONT,
        vec![],
    ));
    v.retain(|b| !b.is_empty());
    v
}

// -- text -------------------------------------------------------------------------------------

/// Every generated text seed.
pub fn text_seeds() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    v.push(text_body(BUSINESS).into_bytes());
    v.push(text_body(TRICKY).into_bytes());
    let mut mixed = String::new();
    for (i, l) in BUSINESS.iter().enumerate() {
        mixed.push_str(l);
        mixed.push_str(TRICKY[i % TRICKY.len()]);
        mixed.push_str(if i % 4 == 3 { "\n\n\u{c}" } else { "\n" });
    }
    v.push(mixed.into_bytes());
    let mut t = String::from("# Naslov\n\n| Rb | Opis | Iznos |\n| 1 | Usluga | 1.000,00 |\n\n");
    t.push_str(&"word ".repeat(2000));
    t.push_str("\n\nhttps://example.com/very/long/path?x=1&y=2 mail@example.org\r\n\r\n");
    v.push(t.into_bytes());
    // UTF-16LE with BOM
    let mut u16 = vec![0xff, 0xfe];
    for c in text_body(&BUSINESS[..4]).encode_utf16() {
        u16.extend_from_slice(&c.to_le_bytes());
    }
    v.push(u16);
    // invalid UTF-8 sprinkled in
    let mut bad = BUSINESS[4].as_bytes().to_vec();
    bad.extend_from_slice(b"\xc0\xaf\xed\xa0\x80\xf4\x90\x80\x80\xff\xfe ");
    bad.extend_from_slice(BUSINESS[5].as_bytes());
    v.push(bad);
    v
}

// -- PPTX -------------------------------------------------------------------------------------

const PNS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

/// A shape with an optional placeholder, an optional offset and paragraphs.
fn pshape(id: u32, ph: &str, pos: Option<(i64, i64)>, lines: &[&str]) -> String {
    let xfrm = pos
        .map(|(x, y)| format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="1" cy="1"/></a:xfrm>"#))
        .unwrap_or_default();
    let paras: String = lines
        .iter()
        .map(|l| {
            format!(
                "<a:p><a:r><a:rPr lang=\"sr-Latn-RS\"/><a:t>{}</a:t></a:r></a:p>",
                esc(l)
            )
        })
        .collect();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="s{id}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{xfrm}</p:spPr><p:txBody><a:bodyPr/>{paras}</p:txBody></p:sp>"#
    )
}

fn psld(root: &str, shapes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:{root} {PNS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{shapes}</p:spTree></p:cSld></p:{root}>"#
    )
}

/// A presentation from slides (shapes XML, notes text).
fn pptx(slides: &[(String, Option<&str>)]) -> Vec<u8> {
    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let rels = |inner: String| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PKG}">{inner}</Relationships>"#
        )
    };
    let rel = |id: &str, t: &str, target: &str| {
        format!(r#"<Relationship Id="{id}" Type="{REL}/{t}" Target="{target}"/>"#)
    };
    let mut e = vec![
        Entry::new(
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>"#,
        ),
        Entry::new(
            "_rels/.rels",
            rels(rel("rId1", "officeDocument", "ppt/presentation.xml")),
        ),
    ];
    let ids: String = (1..=slides.len())
        .map(|i| format!(r#"<p:sldId id="{}" r:id="rIdS{i}"/>"#, 255 + i))
        .collect();
    e.push(Entry::new(
        "ppt/presentation.xml",
        format!(r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation {PNS}><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"#),
    ));
    let prels: String = (1..=slides.len())
        .map(|i| {
            rel(
                &format!("rIdS{i}"),
                "slide",
                &format!("slides/slide{i}.xml"),
            )
        })
        .collect();
    e.push(Entry::new("ppt/_rels/presentation.xml.rels", rels(prels)));
    for (k, (shapes, notes)) in slides.iter().enumerate() {
        let i = k + 1;
        e.push(Entry::new(
            format!("ppt/slides/slide{i}.xml"),
            psld("sld", shapes),
        ));
        let mut r = rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml");
        if let Some(n) = notes {
            r.push_str(&rel(
                "rId2",
                "notesSlide",
                &format!("../notesSlides/notesSlide{i}.xml"),
            ));
            let body = pshape(2, r#"<p:ph type="body" idx="1"/>"#, None, &[n]);
            e.push(Entry::new(
                format!("ppt/notesSlides/notesSlide{i}.xml"),
                psld("notes", &body),
            ));
        }
        e.push(Entry::new(
            format!("ppt/slides/_rels/slide{i}.xml.rels"),
            rels(r),
        ));
    }
    let layout = pshape(2, r#"<p:ph type="title"/>"#, Some((457_200, 274_638)), &[])
        + &pshape(3, r#"<p:ph idx="1"/>"#, Some((457_200, 1_600_200)), &[]);
    e.push(Entry::new(
        "ppt/slideLayouts/slideLayout1.xml",
        psld("sldLayout", &layout),
    ));
    zipfmt::write(&e, Eocd::default(), b"")
}

/// Every generated PPTX seed.
pub fn pptx_seeds() -> Vec<Vec<u8>> {
    let title = |t: &str| pshape(2, r#"<p:ph type="title"/>"#, None, &[t]);
    let body = |lines: &[&str]| pshape(3, r#"<p:ph idx="1"/>"#, None, lines);
    let tc = |t: &str, a: &str| {
        format!(
            "<a:tc{a}><a:txBody><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></a:txBody></a:tc>",
            esc(t)
        )
    };
    let table = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="t"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="3000000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tr>{}{}{}</a:tr><a:tr>{}{}{}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        tc("Rb", ""),
        tc("Opis", r#" gridSpan="2""#),
        tc("", r#" hMerge="1""#),
        tc("1", r#" rowSpan="2""#),
        tc("Usluga", ""),
        tc("1.234.567,89 RSD", ""),
    );
    let group = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="g"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="10" y="10"/></a:xfrm></p:grpSpPr>{}{}</p:grpSp>"#,
        pshape(21, "", Some((5_000_000, 10)), &[BUSINESS[6]]),
        pshape(22, "", Some((10, 10)), &[BUSINESS[7]]),
    );
    vec![
        pptx(&[
            (
                title(BUSINESS[0]) + &body(&BUSINESS[1..5]),
                Some(BUSINESS[5]),
            ),
            (title("Cene") + &table, None),
            (
                group + &pshape(30, "", Some((0, 6_000_000)), &TRICKY[..3]),
                Some(TRICKY[3]),
            ),
        ]),
        pptx(&[(
            title(BUSINESS[8]) + &body(&[BUSINESS[9], BUSINESS[10], BUSINESS[11]]),
            None,
        )]),
    ]
}

// -- HTML -------------------------------------------------------------------------------------

/// Every generated HTML seed: an inline XBRL 10-K, an old upper-case 8-K, a web page with
/// scripts, navigation and hidden content, a windows-1250 page.
pub fn html_seeds() -> Vec<Vec<u8>> {
    let p: String = BUSINESS
        .iter()
        .map(|l| format!("<p><font size=\"2\">{}</font></p>\n", esc(l)))
        .collect();
    let ten_k = format!(
        r#"<?xml version='1.0' encoding='ASCII'?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:ix="http://www.xbrl.org/2013/inlineXBRL" xmlns:dei="http://xbrl.sec.gov/dei/2023">
<head><title>exmp-20231231</title></head><body>
<div style="display:none"><ix:header><ix:hidden><ix:nonNumeric name="dei:EntityCentralIndexKey" contextRef="c">0000999999</ix:nonNumeric></ix:hidden></ix:header></div>
<table style="width:100%"><tr><td>
<p style="text-align:center">FORM <ix:nonNumeric name="dei:DocumentType" contextRef="c">10-K</ix:nonNumeric></p>
{p}<hr style="page-break-after:always"/>
<h2>Item&#160;8. Financial Statements</h2>
<table><tr><td>&#160;</td><td colspan="2"><b>2023</b></td></tr>
<tr><td>Net sales</td><td>$</td><td><ix:nonFraction name="us-gaap:Revenues" contextRef="c" unitRef="usd" scale="6">1,234</ix:nonFraction></td></tr>
<tr><td rowspan="2">Net loss</td><td></td><td>(<ix:nonFraction name="us-gaap:NetIncomeLoss" contextRef="c" unitRef="usd" sign="-">56</ix:nonFraction>)</td></tr></table>
<p style="page-break-before:always">Signatures</p></td></tr></table></body></html>"#
    );
    let eight_k = r#"<HTML><HEAD><TITLE>form8-k.htm</TITLE><META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=iso-8859-1"></HEAD>
<BODY><CENTER><FONT SIZE=4><B>FORM 8-K</B></FONT></CENTER>
<TABLE WIDTH="100%"><TR><TD><TABLE><TR><TD><FONT SIZE=2>Delaware</FONT><TD>&nbsp;<TD>001-99999
<TR><TD>(State)<TD>&nbsp;<TD>(File No.)</TABLE></TD></TR></TABLE>
<P><FONT SIZE=2><B>Item&nbsp;2.02</B>&nbsp;&nbsp;Results of Operations.</FONT>
<P>On May&nbsp;1 the Company issued a release (the &#147;Release&#148;).
</BODY></HTML>"#.to_owned();
    let tricky: String = TRICKY
        .iter()
        .map(|t| format!("<li>{}</li>", esc(t)))
        .collect();
    let web = format!(
        r#"<!DOCTYPE html><html lang="sr"><head><meta charset="utf-8"><title>Ugovor</title><style>p{{color:red}}</style><script>var s = "</p><p>"; if (a < b && c > d) {{}}</script></head>
<body><nav><a href="/">Početna</a></nav><header><h1>Okvirni sporazum</h1></header>
<main><p hidden>skriveno</p><div style="visibility:hidden">nevidljivo</div>
<ul>{tricky}</ul><pre>  Rb   Opis
  1    Usluga</pre><table><caption>Cene</caption><thead><tr><th>Rb<th>Iznos<tbody><tr><td>1<td>1.000,00</table>
<noscript>JS</noscript><template><p>t</p></template><textarea>x</textarea>
&amp; &copy; &#8212; &#x1F600; &notin &bogus; &#0; &#xD800;</main><footer>Strana 1 od 3</footer></body></html>"#
    );
    let mut cp1250 = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1250\"></head><body><p>\x8Aifra \x9Eiro ra\xE8un \xE8lan 5</p>".to_vec();
    cp1250.extend_from_slice(b"<table><tr><td>Iznos<td>1.234,00</table></body></html>");
    vec![
        ten_k.into_bytes(),
        eight_k.into_bytes(),
        web.into_bytes(),
        cp1250,
    ]
}

/// All generated seeds with their kind.
pub fn all() -> Vec<(Kind, Vec<u8>)> {
    let mut v: Vec<(Kind, Vec<u8>)> = Vec::new();
    v.extend(docx_seeds().into_iter().map(|b| (Kind::Docx, b)));
    v.extend(pdf_seeds().into_iter().map(|b| (Kind::Pdf, b)));
    v.extend(text_seeds().into_iter().map(|b| (Kind::Text, b)));
    v.extend(pptx_seeds().into_iter().map(|b| (Kind::Pptx, b)));
    v.extend(html_seeds().into_iter().map(|b| (Kind::Html, b)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_what_they_claim() {
        let all = all();
        assert!(all.len() > 15);
        for (k, b) in &all {
            assert_eq!(Kind::of(b), *k);
            if *k == Kind::Docx {
                let parts = zipfmt::read(b, 1 << 20).unwrap();
                assert!(parts.iter().any(|(n, _)| n == b"word/document.xml"));
            }
            if *k == Kind::Pptx {
                let parts = zipfmt::read(b, 1 << 20).unwrap();
                assert!(parts.iter().any(|(n, _)| n == b"ppt/slides/slide1.xml"));
            }
        }
    }
}

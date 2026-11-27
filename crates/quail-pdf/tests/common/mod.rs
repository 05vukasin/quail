//! Synthetic PDFs for tests and benches, written with `lopdf`. No real documents.
#![allow(
    dead_code,
    clippy::missing_panics_doc,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};

/// Page size used everywhere: US Letter.
pub const W: f32 = 612.0;
/// Page height.
pub const H: f32 = 792.0;

/// A line of text at `x` points from the left and `top` points from the top.
#[derive(Clone)]
pub struct Line {
    pub x: f32,
    pub top: f32,
    pub size: f32,
    pub text: String,
}

/// What goes on one page.
#[derive(Clone, Default)]
pub struct PageSpec {
    pub lines: Vec<Line>,
    /// Draw a full-page grayscale image.
    pub full_page_image: bool,
}

pub fn line(x: f32, top: f32, text: &str) -> Line {
    Line {
        x,
        top,
        size: 11.0,
        text: text.to_owned(),
    }
}

/// Extra document-level parts.
#[derive(Clone, Default)]
pub struct Extras {
    pub info: Vec<(&'static str, &'static str)>,
    /// Add an `AcroForm` with one text field and one signed signature field.
    pub forms: bool,
    /// Add a document-level JavaScript name tree.
    pub javascript: bool,
    /// Add one embedded file.
    pub embedded_file: bool,
    /// Add an XMP metadata stream.
    pub xmp: bool,
    /// The XMP packet to use instead of the empty one (implies `xmp`).
    pub xmp_packet: Option<String>,
    /// Compress content streams.
    pub compress: bool,
}

/// Builds the document (not yet saved).
pub fn document(pages: &[PageSpec], extras: &Extras) -> Document {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 4,
            "Height" => 4,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        },
        vec![0x80; 16],
    ));
    let mut kids = Vec::new();
    for spec in pages {
        let mut ops = Vec::new();
        if spec.full_page_image {
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "cm",
                vec![W.into(), 0.into(), 0.into(), H.into(), 0.into(), 0.into()],
            ));
            ops.push(Operation::new("Do", vec!["Im1".into()]));
            ops.push(Operation::new("Q", vec![]));
        }
        for l in &spec.lines {
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new("Tf", vec!["F1".into(), l.size.into()]));
            ops.push(Operation::new("Td", vec![l.x.into(), (H - l.top).into()]));
            ops.push(Operation::new(
                "Tj",
                vec![Object::string_literal(l.text.as_bytes().to_vec())],
            ));
            ops.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations: ops };
        let mut stream = Stream::new(dictionary! {}, content.encode().expect("encode"));
        if extras.compress {
            let _ = stream.compress();
        }
        let content_id = doc.add_object(stream);
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        kids.push(page_id.into());
    }
    let resources = dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Im1" => image_id },
    };
    let count = i64::try_from(kids.len()).expect("count");
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), W.into(), H.into()],
        }),
    );
    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if extras.forms {
        let text_field = doc.add_object(dictionary! {
            "FT" => "Tx",
            "T" => Object::string_literal("name"),
        });
        let sig_value = doc.add_object(dictionary! {
            "Type" => "Sig",
            "Filter" => "Adobe.PPKLite",
            "Contents" => Object::String(vec![0; 8], lopdf::StringFormat::Hexadecimal),
        });
        let sig_field = doc.add_object(dictionary! {
            "FT" => "Sig",
            "T" => Object::string_literal("sig1"),
            "V" => sig_value,
        });
        let empty_sig = doc.add_object(dictionary! {
            "FT" => "Sig",
            "T" => Object::string_literal("sig2"),
        });
        catalog.set(
            "AcroForm",
            dictionary! {
                "Fields" => vec![text_field.into(), sig_field.into(), empty_sig.into()],
                "SigFlags" => 3,
            },
        );
    }
    let mut names = Dictionary::new();
    if extras.javascript {
        let action = doc.add_object(dictionary! {
            "S" => "JavaScript",
            "JS" => Object::string_literal("app.alert(1)"),
        });
        names.set(
            "JavaScript",
            dictionary! { "Names" => vec![Object::string_literal("a"), action.into()] },
        );
    }
    if extras.embedded_file {
        let file = doc.add_object(Stream::new(
            dictionary! { "Type" => "EmbeddedFile" },
            b"hello".to_vec(),
        ));
        let spec = doc.add_object(dictionary! {
            "Type" => "Filespec",
            "F" => Object::string_literal("a.txt"),
            "EF" => dictionary! { "F" => file },
        });
        names.set(
            "EmbeddedFiles",
            dictionary! { "Names" => vec![Object::string_literal("a.txt"), spec.into()] },
        );
    }
    if !names.is_empty() {
        catalog.set("Names", names);
    }
    if extras.xmp || extras.xmp_packet.is_some() {
        let packet = extras.xmp_packet.as_deref().map_or_else(
            || b"<x:xmpmeta xmlns:x='adobe:ns:meta/'></x:xmpmeta>".to_vec(),
            |p| p.as_bytes().to_vec(),
        );
        let mut stream = Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            packet,
        );
        if extras.compress {
            let _ = stream.compress();
        }
        let xmp = doc.add_object(stream);
        catalog.set("Metadata", xmp);
    }
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    if !extras.info.is_empty() {
        let mut info = Dictionary::new();
        for (k, v) in &extras.info {
            info.set(*k, Object::string_literal(*v));
        }
        let info_id = doc.add_object(info);
        doc.trailer.set("Info", info_id);
    }
    doc
}

pub fn save(doc: &mut Document) -> Vec<u8> {
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save");
    out
}

/// Builds and saves.
pub fn pdf(pages: &[PageSpec], extras: &Extras) -> Vec<u8> {
    save(&mut document(pages, extras))
}

/// Encrypts with RC4-128 under the given user password (owner password "owner").
pub fn encrypted(pages: &[PageSpec], user_password: &str) -> Vec<u8> {
    let mut doc = document(pages, &Extras::default());
    doc.trailer.set(
        "ID",
        vec![
            Object::String(
                b"0123456789abcdef".to_vec(),
                lopdf::StringFormat::Hexadecimal,
            ),
            Object::String(
                b"0123456789abcdef".to_vec(),
                lopdf::StringFormat::Hexadecimal,
            ),
        ],
    );
    let version = lopdf::EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner",
        user_password,
        key_length: 128,
        permissions: lopdf::Permissions::all(),
    };
    let state = lopdf::EncryptionState::try_from(version).expect("state");
    doc.encrypt(&state).expect("encrypt");
    save(&mut doc)
}

/// A page with `n` body lines of numbered text.
pub fn text_page(page: usize, n: usize) -> PageSpec {
    PageSpec {
        lines: (0..n)
            .map(|i| {
                line(
                    72.0,
                    150.0 + 14.0 * i as f32,
                    &format!("Page {page} body line {i} lorem ipsum dolor sit amet"),
                )
            })
            .collect(),
        full_page_image: false,
    }
}

//! Reduced decodes of DCT (JPEG) scans: scaled IDCT at 1/2, 1/4 and 1/8, generated in code.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::missing_panics_doc
)]

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use quail_core::{Blob, Emitter, Extractor, Image, Limits, Raster};
use quail_pdf::PdfExtractor;

/// Image size: 128 × 96 blocks of 8 × 8.
const IW: u32 = 1024;
const IH: u32 = 768;

/// Writes bits MSB first, stuffing a zero byte after every 0xFF.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u32, len: u32) {
        for i in (0..len).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                let b = self.acc as u8;
                self.out.push(b);
                if b == 0xFF {
                    self.out.push(0);
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        while self.n != 0 {
            self.put(1, 1);
        }
        self.out
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, body: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
    out.extend_from_slice(body);
}

/// A baseline JPEG whose 8 × 8 blocks are flat: block `(bx, by)` has luma
/// `128 + dc(bx, by)`; chroma (when `components == 3`) is neutral. Every block carries only its
/// DC coefficient, so any IDCT scale reproduces it exactly. Huffman tables are made up for the
/// test: DC category `c` is the 4-bit code `c`, and the AC table holds only EOB (`00`).
fn flat_jpeg(components: u8, dc: impl Fn(u32, u32) -> i32) -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8];
    // Quantizer 8: a DC of `d` decodes to 128 + d.
    let mut dqt = vec![0u8];
    dqt.extend_from_slice(&[8; 64]);
    segment(&mut j, 0xDB, &dqt);
    let mut sof = vec![8];
    sof.extend_from_slice(&u16::try_from(IH).unwrap().to_be_bytes());
    sof.extend_from_slice(&u16::try_from(IW).unwrap().to_be_bytes());
    sof.push(components);
    for c in 1..=components {
        sof.extend_from_slice(&[c, 0x11, 0]);
    }
    segment(&mut j, 0xC0, &sof);
    // DC table 0: nine codes of length 4 (categories 0..=8).
    let mut dht = vec![0x00, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    dht.extend(0..=8u8);
    segment(&mut j, 0xC4, &dht);
    // AC table 0: EOB (`00`) and one unused symbol (`01`); no all-ones code.
    segment(
        &mut j,
        0xC4,
        &[
            0x10, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x01,
        ],
    );
    let mut sos = vec![components];
    for c in 1..=components {
        sos.extend_from_slice(&[c, 0x00]);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    segment(&mut j, 0xDA, &sos);
    let mut bits = Bits::default();
    let mut prev = vec![0i32; usize::from(components)];
    for by in 0..IH / 8 {
        for bx in 0..IW / 8 {
            for (c, p) in prev.iter_mut().enumerate() {
                let v = if c == 0 { dc(bx, by) } else { 0 };
                let diff = v - *p;
                *p = v;
                let cat = 32 - diff.unsigned_abs().leading_zeros();
                bits.put(cat, 4);
                if cat > 0 {
                    let raw = if diff < 0 {
                        diff + (1 << cat) - 1
                    } else {
                        diff
                    };
                    bits.put(raw as u32, cat);
                }
                bits.put(0, 2); // EOB
            }
        }
    }
    j.extend(bits.finish());
    j.extend_from_slice(&[0xFF, 0xD9]);
    j
}

/// Dark (28) top-left quarter, light (228) elsewhere.
fn pattern(bx: u32, by: u32) -> i32 {
    if bx < IW / 16 && by < IH / 16 {
        -100
    } else {
        100
    }
}

/// A one-page PDF whose page is the JPEG, drawn at one point per pixel.
fn scan_pdf(jpeg: Vec<u8>, components: u8) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let image = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => i64::from(IW),
            "Height" => i64::from(IH),
            "ColorSpace" => if components == 1 { "DeviceGray" } else { "DeviceRGB" },
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpeg,
    ));
    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new(
            "cm",
            vec![IW.into(), 0.into(), 0.into(), IH.into(), 0.into(), 0.into()],
        ),
        Operation::new("Do", vec!["Im1".into()]),
        Operation::new("Q", vec![]),
    ];
    let content = doc.add_object(Stream::new(
        dictionary! {},
        Content { operations: ops }.encode().unwrap(),
    ));
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page.into()],
            "Count" => 1,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image } },
            "MediaBox" => vec![0.into(), 0.into(), IW.into(), IH.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn raster(pdf: Vec<u8>) -> Raster {
    let mut out = Emitter::new(PdfExtractor::ID);
    let doc = PdfExtractor::new()
        .extract(&Blob::from(pdf), &Limits::default(), &mut out)
        .expect("doc");
    assert!(doc.pages[0].is_scan());
    let (_, artifacts, _) = out.into_parts();
    artifacts.get::<Raster>().cloned().expect("raster")
}

fn px(im: &Image, x: u32, y: u32) -> &[u8] {
    let c = usize::from(im.channels);
    let i = (y as usize * im.width as usize + x as usize) * c;
    &im.data[i..i + c]
}

fn check_pattern(im: &Image) {
    let (w, h) = (im.width, im.height);
    for (x, y, want) in [
        (0, 0, 28),
        (w / 4, h / 4, 28),
        (w - 1, h - 1, 228),
        (w - 1, 0, 228),
    ] {
        for &v in px(im, x, y) {
            assert!(
                v.abs_diff(want) <= 2,
                "{w}x{h} at ({x}, {y}): {v} != {want}"
            );
        }
    }
}

#[test]
fn dct_scans_decode_at_the_smallest_scale_that_keeps_the_requested_side() {
    for components in [1u8, 3] {
        let r = raster(scan_pdf(flat_jpeg(components, pattern), components));
        let limits = Limits::default();
        let full =
            r.0.embedded_scan(0, &limits)
                .expect("scan")
                .expect("decoded");
        assert_eq!(
            (full.width, full.height, full.channels),
            (IW, IH, components)
        );
        check_pattern(&full);
        // Longer side asked for -> IDCT scale -> output size.
        for (side, want) in [
            (1, (128, 96)),
            (128, (128, 96)),
            (129, (256, 192)),
            (256, (256, 192)),
            (300, (512, 384)),
            (512, (512, 384)),
            (513, (IW, IH)),
            (4000, (IW, IH)),
        ] {
            let red =
                r.0.embedded_scan_reduced(0, side, &limits)
                    .expect("scan")
                    .expect("decoded");
            assert_eq!(
                (red.width, red.height),
                want,
                "max_side {side}, {components} components"
            );
            assert_eq!(red.channels, components);
            assert!(red.width.max(red.height) >= side.min(IW));
            check_pattern(&red);
            if want == (IW, IH) {
                assert_eq!(red, full);
            }
        }
        // Renders keep working (the renderer asks for reduced decodes too).
        let page = r.0.render(0, 9.0, &limits).expect("render");
        assert_eq!((page.width, page.height), (128, 96));
    }
}

#[test]
fn corrupt_dct_data_is_survived_by_the_reduced_decode() {
    let jpeg = flat_jpeg(1, pattern);
    // Cut the entropy-coded data at several points; the reduced decode must not panic and,
    // when it decodes, keep the requested size.
    for keep in [jpeg.len() / 4, jpeg.len() / 2, jpeg.len() - 3] {
        let mut cut = jpeg[..keep].to_vec();
        cut.extend_from_slice(&[0xFF, 0xD9]);
        let r = raster(scan_pdf(cut, 1));
        if let Some(Ok(red)) = r.0.embedded_scan_reduced(0, 128, &Limits::default()) {
            assert!(red.width.max(red.height) >= 128);
        }
    }
}

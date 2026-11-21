//! Hostile-input matrix: generates every crafted case from the reviews of #7 in code, then
//! runs extract / render 150 dpi / render 300 dpi / embedded scan on each in a child process
//! under `prlimit --as=1500M` (default `Limits`, 5 s timeout), and prints one table row per
//! file. A child that dies without printing its result is an ABORT; one still running after
//! 60 s is a TIMEOUT; a result after more than 10 s (2 × the timeout) is flagged SLOW.
//!
//! ```text
//! cargo run -p quail-pdf --example hostile --profile profiling -- [out_dir] [filter]
//! ```
//!
//! Generated files are cached in `out_dir` (default `target/hostile-cases`); nothing is
//! committed. Linux only (needs `prlimit` and `/proc/self/status`).
#![allow(
    missing_docs,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    clippy::format_push_string,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::type_complexity,
    clippy::match_same_arms
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use md5::{Digest, Md5};
use quail_core::{Blob, Emitter, Extractor, Limits, PageRaster};
use quail_pdf::{PdfExtractor, PdfRaster};

const MIB: usize = 1 << 20;
const AS_LIMIT: &str = "--as=1500000000";
const CHILD_KILL: Duration = Duration::from_mins(1);
const TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------------------------
// PDF building blocks.

type Objs = BTreeMap<u32, Vec<u8>>;

const HDR: &[u8] = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";
const FONT: &[u8] = b"/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>";

fn build(objs: &Objs, root: u32, trailer_extra: &[u8]) -> Vec<u8> {
    let mut out = HDR.to_vec();
    let mut offs = BTreeMap::new();
    for (n, body) in objs {
        offs.insert(*n, out.len());
        writeln!(out, "{n} 0 obj").unwrap();
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    let size = objs.keys().max().copied().unwrap_or(0) + 1;
    write!(out, "xref\n0 {size}\n0000000000 65535 f \n").unwrap();
    for n in 1..size {
        match offs.get(&n) {
            Some(o) => writeln!(out, "{o:010} 00000 n ").unwrap(),
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    write!(out, "trailer\n<< /Size {size} /Root {root} 0 R ").unwrap();
    out.extend_from_slice(trailer_extra);
    write!(out, ">>\nstartxref\n{x}\n%%EOF\n").unwrap();
    out
}

/// Top-level objects plus objects stored in object streams (`compressed`: n → (stream, index)),
/// with an xref stream.
fn build_xs(objs: &Objs, compressed: &BTreeMap<u32, (u32, u16)>, trailer_extra: &[u8]) -> Vec<u8> {
    let mut out = HDR.to_vec();
    let mut offs = BTreeMap::new();
    for (n, body) in objs {
        offs.insert(*n, out.len());
        writeln!(out, "{n} 0 obj").unwrap();
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xsn = objs
        .keys()
        .chain(compressed.keys())
        .max()
        .copied()
        .unwrap_or(0)
        + 1;
    offs.insert(xsn, out.len());
    let mut rows = Vec::new();
    for n in 0..=xsn {
        if let Some(o) = offs.get(&n) {
            rows.push(1u8);
            rows.extend_from_slice(&(*o as u32).to_be_bytes());
            rows.extend_from_slice(&0u16.to_be_bytes());
        } else if let Some((s, i)) = compressed.get(&n) {
            rows.push(2u8);
            rows.extend_from_slice(&s.to_be_bytes());
            rows.extend_from_slice(&i.to_be_bytes());
        } else {
            rows.push(0u8);
            rows.extend_from_slice(&0u32.to_be_bytes());
            rows.extend_from_slice(&0xFFFFu16.to_be_bytes());
        }
    }
    write!(
        out,
        "{xsn} 0 obj\n<< /Type /XRef /Size {} /W [1 4 2] /Root 1 0 R ",
        xsn + 1
    )
    .unwrap();
    out.extend_from_slice(trailer_extra);
    write!(out, "/Length {} >>\nstream\n", rows.len()).unwrap();
    out.extend_from_slice(&rows);
    write!(
        out,
        "\nendstream\nendobj\nstartxref\n{}\n%%EOF\n",
        offs[&xsn]
    )
    .unwrap();
    out
}

fn sd(body: &[u8], dict: &[u8]) -> Vec<u8> {
    let mut s = b"<< ".to_vec();
    s.extend_from_slice(dict);
    write!(s, "/Length {} >>\nstream\n", body.len()).unwrap();
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

fn fl(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// zlib of `prefix` followed by `n` bytes of `fill`, streamed (never held decompressed).
fn fl_big(n: usize, fill: u8, prefix: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    e.write_all(prefix).unwrap();
    let chunk = vec![fill; MIB];
    for _ in 0..n / MIB {
        e.write_all(&chunk).unwrap();
    }
    e.finish().unwrap()
}

fn stream(data: &[u8], dict: &[u8]) -> Vec<u8> {
    let mut d = b"/Filter /FlateDecode ".to_vec();
    d.extend_from_slice(dict);
    sd(&fl(data), &d)
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn page_objs(content: &[u8], res: &[u8], mb: &[u8], raw_content: Option<Vec<u8>>) -> Objs {
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    o.insert(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    o.insert(
        3,
        cat(&[
            b"<< /Type /Page /Parent 2 0 R /MediaBox ",
            mb,
            b" /Resources << ",
            res,
            b" >> /Contents 4 0 R >>",
        ]),
    );
    o.insert(4, raw_content.unwrap_or_else(|| stream(content, b"")));
    o
}

const LETTER: &[u8] = b"[0 0 612 792]";

/// PDF LZW, `EarlyChange` 1, matching hayro's decoder.
fn lzw(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut nacc) = (0u64, 0u32);
    let mut put = |code: u16, width: u32, out: &mut Vec<u8>| {
        acc = (acc << width) | u64::from(code);
        nacc += width;
        while nacc >= 8 {
            nacc -= 8;
            out.push((acc >> nacc) as u8);
        }
        acc &= (1u64 << nacc) - 1;
    };
    let width = |emitted: usize| {
        let adjusted = 258 + emitted.saturating_sub(1) + 1;
        if adjusted >= 2048 {
            12
        } else if adjusted >= 1024 {
            11
        } else if adjusted >= 512 {
            10
        } else {
            9
        }
    };
    let mut dict: std::collections::HashMap<(u16, u8), u16> = std::collections::HashMap::new();
    let (mut next, mut emitted) = (258u16, 0usize);
    put(256, 9, &mut out);
    let mut w: Option<u16> = None;
    for &b in data {
        let Some(cur) = w else {
            w = Some(u16::from(b));
            continue;
        };
        if let Some(&c) = dict.get(&(cur, b)) {
            w = Some(c);
            continue;
        }
        put(cur, width(emitted), &mut out);
        emitted += 1;
        dict.insert((cur, b), next);
        next += 1;
        w = Some(u16::from(b));
        if next >= 3800 {
            put(256, width(emitted), &mut out);
            dict.clear();
            next = 258;
            emitted = 0;
        }
    }
    if let Some(cur) = w {
        put(cur, width(emitted), &mut out);
        emitted += 1;
    }
    put(257, width(emitted), &mut out);
    if nacc > 0 {
        out.push((acc << (8 - nacc)) as u8);
    }
    out
}

fn a85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 5 / 4 + 8);
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        let mut v = u32::from_be_bytes(b);
        if chunk.len() == 4 && v == 0 {
            out.push(b'z');
            continue;
        }
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = (v % 85) as u8 + b'!';
            v /= 85;
        }
        out.extend_from_slice(&digits[..=chunk.len()]);
    }
    out.extend_from_slice(b"~>");
    out
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, usize::from(j));
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|&c| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[usize::from(i)]);
            s.swap(usize::from(i), usize::from(j));
            c ^ s[usize::from(s[usize::from(i)].wrapping_add(s[usize::from(j)]))]
        })
        .collect()
}

fn md5(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().to_vec()
}

const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];
const ID0: &[u8] = b"0123456789abcdef";

/// RC4-40, empty user password: (file key, /Encrypt dict, trailer entries).
fn enc_setup() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let owner = cat(&[b"owner", &PAD[..27]]);
    let okey = md5(&[&owner])[..5].to_vec();
    let o = rc4(&okey, &PAD);
    let p: i32 = -4;
    let key = md5(&[&PAD, &o, &p.to_le_bytes(), ID0])[..5].to_vec();
    let u = rc4(&key, &PAD);
    let hex = |b: &[u8]| {
        b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    };
    let edict = format!(
        "<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 /O <{}> /U <{}> >>",
        hex(&o),
        hex(&u)
    )
    .into_bytes();
    let tr = format!("/Encrypt 90 0 R /ID [<{}> <{}>] ", hex(ID0), hex(ID0)).into_bytes();
    (key, edict, tr)
}

fn objkey(key: &[u8], n: u32) -> Vec<u8> {
    md5(&[key, &n.to_le_bytes()[..3], &[0, 0]])[..10].to_vec()
}

// ---------------------------------------------------------------------------------------------
// The cases (first and second review).

fn cases() -> Vec<(&'static str, fn() -> Vec<u8>)> {
    vec![
        ("fanout16", || fanout(16)),
        ("fanout40", || fanout(40)),
        ("quad20000", || quad(20_000)),
        ("quad80000", || quad(80_000)),
        ("objstm2", objstm2),
        ("jbig2", || jbig2_page(65_536)),
        ("formimg", formimg),
        ("shared", || shared(200, 64, false)),
        ("contentsarr", || shared(200, 64, true)),
        ("inlineform", inlineform),
        ("inline49m", inline49m),
        ("tile_tiny", tile_tiny),
        ("tile_huge", tile_huge),
        ("tile_nested", tile_nested),
        ("smask_huge", smask_huge),
        ("smask_img", smask_img),
        ("groups40", groups40),
        ("type3_rec", type3_rec),
        ("type3_mutual", type3_mutual),
        ("chain_objstm", chain_objstm),
        ("chain_twolayer", chain_twolayer),
        ("enc_objstm", || enc_objstm(true)),
        ("enc_objstm_ok", || enc_objstm(false)),
        ("enc_content", enc_content),
        ("tree_cycle", tree_cycle),
        ("tree_inline", tree_inline),
        ("tree_deep", tree_deep),
        ("many_contents", many_contents),
        ("many_pages", many_pages),
        ("flate1100m", || flate_big(1126)),
        ("flate1000m", || flate_big(1000)),
        ("hugepage", hugepage),
        ("bigimage", || bigimage(false)),
        ("bigimage30", || bigimage(true)),
        ("jbig2_ht", || jbig2_ht(false)),
        ("jbig2_ht_skip", || jbig2_ht(true)),
        ("grayimgs8", || grayimgs(8)),
        ("grayimgs24", || grayimgs(24)),
        ("bitimgs100", bitimgs100),
    ]
}

fn fanout(levels: u32) -> Vec<u8> {
    let mut o = page_objs(b"/X Do", b"/XObject << /X 10 0 R >>", LETTER, None);
    for k in 0..levels {
        let n = 10 + k;
        let (c, r) = if k + 1 < levels {
            (
                b"/X Do /X Do".to_vec(),
                format!("/XObject << /X {} 0 R >>", n + 1).into_bytes(),
            )
        } else {
            (b"BT /F1 1 Tf (a) Tj ET".to_vec(), FONT.to_vec())
        };
        o.insert(
            n,
            stream(
                &c,
                &cat(&[
                    b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << ",
                    &r,
                    b" >> ",
                ]),
            ),
        );
    }
    build(&o, 1, b"")
}

fn quad(n: usize) -> Vec<u8> {
    let mut c = b"BT /F1 0.5 Tf 10 780 Td ".to_vec();
    for _ in 0..n {
        c.extend_from_slice(b"0 -1 Td (a) Tj ");
    }
    c.extend_from_slice(b"ET");
    build(&page_objs(&c, FONT, LETTER, None), 1, b"")
}

const OBJSTM_BODY: &[u8] =
    b"3 0 << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >> ";

fn objstm_base() -> Objs {
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    o.insert(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    o.insert(4, stream(b"BT /F1 12 Tf 10 700 Td (hi) Tj ET", b""));
    o
}

fn objstm2() -> Vec<u8> {
    let l1 = fl_big(1024 * MIB, b' ', OBJSTM_BODY);
    let l2 = fl(&l1);
    let mut o = objstm_base();
    o.insert(
        5,
        sd(
            &l2,
            b"/Type /ObjStm /N 1 /First 4 /Filter [/FlateDecode /FlateDecode] ",
        ),
    );
    build_xs(&o, &BTreeMap::from([(3, (5, 0))]), b"")
}

fn jbig2_seg(num: u32, typ: u8, data: &[u8], refs: &[u8]) -> Vec<u8> {
    let mut s = num.to_be_bytes().to_vec();
    s.push(typ);
    s.push((refs.len() as u8) << 5);
    s.extend_from_slice(refs);
    s.push(1);
    s.extend_from_slice(&(data.len() as u32).to_be_bytes());
    s.extend_from_slice(data);
    s
}

fn jbig2_pdf(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let img = sd(
        data,
        format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode ").as_bytes(),
    );
    let mut o = page_objs(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        b"/XObject << /Im0 5 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, img);
    build(&o, 1, b"")
}

fn jbig2_page(side: u32) -> Vec<u8> {
    let mut pinfo = side.to_be_bytes().to_vec();
    pinfo.extend_from_slice(&side.to_be_bytes());
    pinfo.extend_from_slice(&[0; 11]);
    let data = cat(&[&jbig2_seg(0, 48, &pinfo, &[]), &jbig2_seg(1, 49, b"", &[])]);
    jbig2_pdf(&data, 100, 100)
}

fn jbig2_ht(skip: bool) -> Vec<u8> {
    let mut pinfo = 64u32.to_be_bytes().to_vec();
    pinfo.extend_from_slice(&64u32.to_be_bytes());
    pinfo.extend_from_slice(&[0; 11]);
    let mut pdict = vec![0x00, 1, 1];
    pdict.extend_from_slice(&1u32.to_be_bytes());
    pdict.extend_from_slice(&[0; 64]);
    let g: u32 = 16_384;
    let mut ht = Vec::new();
    for v in [64u32, 64, 0, 0] {
        ht.extend_from_slice(&v.to_be_bytes());
    }
    ht.push(0);
    ht.push(if skip { 0x08 } else { 0 });
    ht.extend_from_slice(&g.to_be_bytes());
    ht.extend_from_slice(&g.to_be_bytes());
    ht.extend_from_slice(&0i32.to_be_bytes());
    ht.extend_from_slice(&0i32.to_be_bytes());
    ht.extend_from_slice(&256u16.to_be_bytes());
    ht.extend_from_slice(&0u16.to_be_bytes());
    ht.extend_from_slice(&[0; 256]);
    let data = cat(&[
        &jbig2_seg(0, 48, &pinfo, &[]),
        &jbig2_seg(1, 16, &pdict, &[]),
        &jbig2_seg(2, 22, &ht, &[1]),
        &jbig2_seg(3, 49, b"", &[]),
    ]);
    jbig2_pdf(&data, 64, 64)
}

fn formimg() -> Vec<u8> {
    let img = stream(
        &[0; 1000],
        b"/Type /XObject /Subtype /Image /Width 100000 /Height 100000 /BitsPerComponent 8 /ColorSpace /DeviceRGB ",
    );
    let form = stream(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Im0 6 0 R >> >> ",
    );
    let mut o = page_objs(b"/F Do", b"/XObject << /F 5 0 R >>", LETTER, None);
    o.insert(5, form);
    o.insert(6, img);
    build(&o, 1, b"")
}

fn shared(npages: u32, mib: usize, array: bool) -> Vec<u8> {
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    o.insert(
        4,
        sd(&fl_big(mib * MIB, b' ', b""), b"/Filter /FlateDecode "),
    );
    if array {
        o.insert(2, b"<< /Type /Pages /Kids [10 0 R] /Count 1 >>".to_vec());
        o.insert(
            10,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [{}] >>",
                "4 0 R ".repeat(npages as usize)
            )
            .into_bytes(),
        );
    } else {
        let kids: Vec<String> = (0..npages).map(|i| format!("{} 0 R", 10 + i)).collect();
        o.insert(
            2,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {npages} >>",
                kids.join(" ")
            )
            .into_bytes(),
        );
        for i in 0..npages {
            o.insert(
                10 + i,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            );
        }
    }
    build(&o, 1, b"")
}

fn inline_image(w: u32, h: u32, cs: &str, comps: usize) -> Vec<u8> {
    let data = fl(&vec![0x80; w as usize * h as usize * comps]);
    cat(&[
        format!("q 612 0 0 792 0 0 cm BI /W {w} /H {h} /BPC 8 /CS /{cs} /F /Fl ID ").as_bytes(),
        &data,
        b"\nEI Q",
    ])
}

fn inlineform() -> Vec<u8> {
    let form = stream(
        &inline_image(4600, 4600, "RGB", 3),
        b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] ",
    );
    let mut o = page_objs(
        &b"/F Do ".repeat(400),
        b"/XObject << /F 5 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, form);
    build(&o, 1, b"")
}

fn inline49m() -> Vec<u8> {
    build(
        &page_objs(&inline_image(7000, 7000, "G", 1), b"", LETTER, None),
        1,
        b"",
    )
}

fn pattern_page(pat: Vec<u8>) -> Vec<u8> {
    let mut o = page_objs(
        b"/Pattern cs /P0 scn 0 0 612 792 re f",
        b"/Pattern << /P0 5 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, pat);
    build(&o, 1, b"")
}

fn tile_tiny() -> Vec<u8> {
    pattern_page(stream(
        b"0 0 1 rg 0 0 0.5 0.5 re f",
        b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 1 1] /XStep 0.01 /YStep 0.01 /Resources << >> ",
    ))
}

fn tile_huge() -> Vec<u8> {
    pattern_page(stream(
        b"0 0 1 rg 0 0 50 50 re f",
        b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 100] /XStep 100 /YStep 100 /Matrix [1000 0 0 1000 0 0] /Resources << >> ",
    ))
}

fn tile_nested() -> Vec<u8> {
    let mut o = page_objs(
        b"/Pattern cs /P0 scn 0 0 612 792 re f",
        b"/Pattern << /P0 10 0 R >>",
        LETTER,
        None,
    );
    let n_levels = 6;
    for k in 0..n_levels {
        let n = 10 + k;
        let (c, r) = if k + 1 < n_levels {
            (
                b"/Pattern cs /P0 scn 0 0 10 10 re f".to_vec(),
                format!("/Pattern << /P0 {} 0 R >>", n + 1).into_bytes(),
            )
        } else {
            (b"0 0 1 rg 0 0 5 5 re f".to_vec(), Vec::new())
        };
        o.insert(
            n,
            stream(
                &c,
                &cat(&[b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 1 /YStep 1 /Resources << ", &r, b" >> "]),
            ),
        );
    }
    build(&o, 1, b"")
}

fn smask_huge() -> Vec<u8> {
    let grp = stream(
        b"1 g 0 0 612 792 re f",
        b"/Type /XObject /Subtype /Form /BBox [-1e9 -1e9 1e9 1e9] /Group << /S /Transparency /CS /DeviceGray >> /Matrix [1000 0 0 1000 0 0] ",
    );
    let gs = b"<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 5 0 R >> >>".to_vec();
    let mut o = page_objs(
        b"/GS0 gs 0 0 1 rg 0 0 612 792 re f ",
        b"/ExtGState << /GS0 6 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, grp);
    o.insert(6, gs);
    build(&o, 1, b"")
}

fn smask_img() -> Vec<u8> {
    let sm = stream(
        &[0xff; 100],
        b"/Type /XObject /Subtype /Image /Width 60000 /Height 60000 /BitsPerComponent 8 /ColorSpace /DeviceGray ",
    );
    let im = stream(
        &[0x80; 300],
        b"/Type /XObject /Subtype /Image /Width 10 /Height 10 /BitsPerComponent 8 /ColorSpace /DeviceRGB /SMask 6 0 R ",
    );
    let mut o = page_objs(
        b"q 612 0 0 792 0 0 cm /Im0 Do Q",
        b"/XObject << /Im0 5 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, im);
    o.insert(6, sm);
    build(&o, 1, b"")
}

fn groups40() -> Vec<u8> {
    let mut o = page_objs(
        b"/GS0 gs /X Do",
        b"/XObject << /X 10 0 R >> /ExtGState << /GS0 9 0 R >>",
        LETTER,
        None,
    );
    let n_levels = 40;
    for k in 0..n_levels {
        let n = 10 + k;
        let (c, r) = if k + 1 < n_levels {
            (
                b"/GS0 gs /X Do".to_vec(),
                format!(
                    "/XObject << /X {} 0 R >> /ExtGState << /GS0 9 0 R >>",
                    n + 1
                )
                .into_bytes(),
            )
        } else {
            (b"0 0 1 rg 0 0 612 792 re f".to_vec(), Vec::new())
        };
        o.insert(
            n,
            stream(
                &c,
                &cat(&[b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency >> /Resources << ", &r, b" >> "]),
            ),
        );
    }
    o.insert(
        8,
        stream(
            b"0.5 g 0 0 612 792 re f",
            b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >> ",
        ),
    );
    o.insert(
        9,
        b"<< /Type /ExtGState /ca 0.5 /SMask << /Type /Mask /S /Luminosity /G 8 0 R >> >>".to_vec(),
    );
    build(&o, 1, b"")
}

const T3: &[u8] = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << /Font << /T 5 0 R >> >> >>";

fn type3_rec() -> Vec<u8> {
    let mut o = page_objs(
        b"BT /T 12 Tf 10 700 Td (aaaa) Tj ET",
        b"/Font << /T 5 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, T3.to_vec());
    o.insert(6, stream(b"1000 0 d0 BT /T 1 Tf (aa) Tj ET", b""));
    build(&o, 1, b"")
}

fn type3_mutual() -> Vec<u8> {
    let t3a = String::from_utf8_lossy(T3)
        .replace("/a 6 0 R", "/a 7 0 R")
        .replace("/Font << /T 5 0 R >>", "/Font << /T 6 0 R >>");
    let t3b = String::from_utf8_lossy(T3)
        .replace("/a 6 0 R", "/a 8 0 R")
        .replace("/Font << /T 5 0 R >>", "/Font << /U 5 0 R >>");
    let mut o = page_objs(
        b"BT /T 12 Tf 10 700 Td (aaaa) Tj ET",
        b"/Font << /T 5 0 R >>",
        LETTER,
        None,
    );
    o.insert(5, t3a.into_bytes());
    o.insert(6, t3b.into_bytes());
    o.insert(7, stream(b"1000 0 d0 BT /T 1 Tf (aa) Tj ET", b""));
    o.insert(8, stream(b"1000 0 d0 BT /U 1 Tf (aa) Tj ET", b""));
    build(&o, 1, b"")
}

fn chain_objstm() -> Vec<u8> {
    let l1 = fl_big(3 << 30, b' ', OBJSTM_BODY);
    let l3 = a85(&lzw(&l1));
    let mut o = objstm_base();
    o.insert(
        5,
        sd(
            &l3,
            b"/Type /ObjStm /N 1 /First 4 /Filter [/ASCII85Decode /LZWDecode /FlateDecode] ",
        ),
    );
    build_xs(&o, &BTreeMap::from([(3, (5, 0))]), b"")
}

fn chain_twolayer() -> Vec<u8> {
    let mut l1 = fl_big(900 * MIB, b' ', OBJSTM_BODY);
    l1.resize(300 * MIB, 0);
    let outer = fl(&l1);
    drop(l1);
    let mut o = objstm_base();
    o.insert(
        5,
        sd(
            &outer,
            b"/Type /ObjStm /N 1 /First 4 /Filter [/FlateDecode /FlateDecode] ",
        ),
    );
    build_xs(&o, &BTreeMap::from([(3, (5, 0))]), b"")
}

fn enc_objstm(bomb: bool) -> Vec<u8> {
    let (key, edict, tr) = enc_setup();
    let mut o = objstm_base();
    o.insert(
        4,
        sd(
            &rc4(&objkey(&key, 4), &fl(b"BT /F1 12 Tf 10 700 Td (hi) Tj ET")),
            b"/Filter /FlateDecode ",
        ),
    );
    let body = if bomb {
        fl_big(2 << 30, b' ', OBJSTM_BODY)
    } else {
        fl(OBJSTM_BODY)
    };
    o.insert(
        5,
        sd(
            &rc4(&objkey(&key, 5), &body),
            b"/Type /ObjStm /N 1 /First 4 /Filter /FlateDecode ",
        ),
    );
    o.insert(90, edict);
    build_xs(&o, &BTreeMap::from([(3, (5, 0))]), &tr)
}

fn enc_content() -> Vec<u8> {
    let (key, edict, tr) = enc_setup();
    let b1 = fl_big(4 << 30, b' ', b"BT /F1 12 Tf 10 700 Td (hi) Tj ET ");
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    o.insert(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    o.insert(
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
    );
    o.insert(
        4,
        sd(
            &rc4(&objkey(&key, 4), &fl(&b1)),
            b"/Filter [/FlateDecode /FlateDecode] ",
        ),
    );
    o.insert(90, edict);
    build(&o, 1, &tr)
}

fn tree_base() -> Objs {
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    o.insert(5, stream(b"BT /F1 12 Tf 10 700 Td (hi) Tj ET", b""));
    o
}

fn tree_cycle() -> Vec<u8> {
    let n = 400_000;
    let mut o = tree_base();
    o.insert(
        2,
        cat(&[
            b"<< /Type /Pages /Kids [",
            &b"2 0 R 3 0 R 4 0 R ".repeat(n),
            b"] /Count 1 >>",
        ]),
    );
    o.insert(
        3,
        cat(&[
            b"<< /Type /Pages /Kids [",
            &b"2 0 R 3 0 R ".repeat(n),
            b"] >>",
        ]),
    );
    o.insert(
        4,
        b"<< /Type /Page /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_vec(),
    );
    build(&o, 1, b"")
}

fn tree_inline() -> Vec<u8> {
    let mut o = tree_base();
    o.insert(
        2,
        cat(&[
            b"<< /Type /Pages /Kids [",
            &b"<</Type/Page/MediaBox[0 0 9 9]/Contents 5 0 R>>".repeat(300_000),
            b"] /Count 300000 >>",
        ]),
    );
    build(&o, 1, b"")
}

fn tree_deep() -> Vec<u8> {
    let d = 20_000;
    let s = cat(&[
        &b"<</Type/Pages/Kids[".repeat(d),
        b"<</Type/Page/MediaBox[0 0 9 9]/Contents 5 0 R>>",
        &b"]>>".repeat(d),
    ]);
    let mut o = tree_base();
    o.insert(2, cat(&[b"<< /Type /Pages /Kids [", &s, b"] /Count 1 >>"]));
    build(&o, 1, b"")
}

fn one_mib_stream() -> Vec<u8> {
    sd(
        &fl_big(MIB, b' ', b"BT /F1 12 Tf 10 700 Td (hi) Tj ET\n"),
        b"/Filter /FlateDecode ",
    )
}

fn many_contents() -> Vec<u8> {
    let n = 1400;
    let one = one_mib_stream();
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    o.insert(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    let refs: Vec<String> = (0..n).map(|i| format!("{} 0 R", 10 + i)).collect();
    o.insert(
        3,
        cat(&[
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << ",
            FONT,
            b" >> /Contents [",
            refs.join(" ").as_bytes(),
            b"] >>",
        ]),
    );
    for i in 0..n {
        o.insert(10 + i, one.clone());
    }
    build(&o, 1, b"")
}

fn many_pages() -> Vec<u8> {
    let (p, k) = (200u32, 8u32);
    let one = one_mib_stream();
    let mut o = Objs::new();
    o.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: Vec<String> = (0..p).map(|i| format!("{} 0 R", 5000 + i)).collect();
    o.insert(
        2,
        format!("<< /Type /Pages /Kids [{}] /Count {p} >>", kids.join(" ")).into_bytes(),
    );
    for pg in 0..p {
        let refs: Vec<String> = (0..k).map(|j| format!("{} 0 R", 10 + pg * k + j)).collect();
        o.insert(
            5000 + pg,
            cat(&[
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << ",
                FONT,
                b" >> /Contents [",
                refs.join(" ").as_bytes(),
                b"] >>",
            ]),
        );
        for j in 0..k {
            o.insert(10 + pg * k + j, one.clone());
        }
    }
    build(&o, 1, b"")
}

fn flate_big(mib: usize) -> Vec<u8> {
    let big = fl_big(mib * MIB, b' ', b"BT /F1 12 Tf 10 700 Td (hi) Tj ET\n");
    build(
        &page_objs(b"", FONT, LETTER, Some(sd(&big, b"/Filter /FlateDecode "))),
        1,
        b"",
    )
}

fn hugepage() -> Vec<u8> {
    build(
        &page_objs(
            b"0 0 1 rg 0 0 1517 1517 re f BT /F1 200 Tf 100 700 Td (Huge) Tj ET",
            FONT,
            b"[0 0 1517 1517]",
            None,
        ),
        1,
        b"",
    )
}

fn bigimage(thirty: bool) -> Vec<u8> {
    let (w, h) = (6300usize, 6300usize);
    let im = sd(
        &fl(&vec![0x40; w * h * 4]),
        format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /BitsPerComponent 8 /ColorSpace /DeviceCMYK /Filter /FlateDecode ").as_bytes(),
    );
    let mb: &[u8] = b"[0 0 1517 1517]";
    let mut o = if thirty {
        page_objs(&b"/F Do ".repeat(30), b"/XObject << /F 6 0 R >>", mb, None)
    } else {
        page_objs(
            b"q 1517 0 0 1517 0 0 cm /Im0 Do Q",
            b"/XObject << /Im0 5 0 R >>",
            mb,
            None,
        )
    };
    o.insert(5, im);
    if thirty {
        o.insert(
            6,
            stream(
                b"q 1517 0 0 1517 0 0 cm /Im0 Do Q",
                b"/Type /XObject /Subtype /Form /BBox [0 0 1517 1517] /Resources << /XObject << /Im0 5 0 R >> >> ",
            ),
        );
    }
    build(&o, 1, b"")
}

fn many_images(k: u32, raw: &[u8], dict: &str) -> Vec<u8> {
    let data = fl(raw);
    let mut content = Vec::new();
    let mut res = b"/XObject << ".to_vec();
    for i in 0..k {
        write!(content, "q 612 0 0 792 0 0 cm /I{i} Do Q ").unwrap();
        write!(res, "/I{i} {} 0 R ", 10 + i).unwrap();
    }
    res.extend_from_slice(b">>");
    let mut o = page_objs(&content, &res, LETTER, None);
    for i in 0..k {
        o.insert(10 + i, sd(&data, dict.as_bytes()));
    }
    build(&o, 1, b"")
}

fn grayimgs(k: u32) -> Vec<u8> {
    many_images(
        k,
        &vec![0x40; 6300 * 6300],
        "/Type /XObject /Subtype /Image /Width 6300 /Height 6300 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode ",
    )
}

fn bitimgs100() -> Vec<u8> {
    many_images(
        100,
        &vec![0x55; 6300_usize.div_ceil(8) * 6300],
        "/Type /XObject /Subtype /Image /Width 6300 /Height 6300 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /FlateDecode ",
    )
}

// ---------------------------------------------------------------------------------------------
// Child: one mode on one file, prints `RESULT\t<outcome>\t<secs>\t<peak KB>`.

fn hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}

fn child(path: &Path, mode: &str, pages: u32, dpi: f32) {
    let bytes = std::fs::read(path).unwrap();
    let limits = Limits {
        timeout: TIMEOUT,
        ..Limits::default()
    };
    let blob = Blob::from(bytes);
    let t = Instant::now();
    let res = match mode {
        "count" => {
            let mut out = Emitter::with_deadline(PdfExtractor::ID, Instant::now() + limits.timeout);
            match PdfExtractor::new().extract(&blob, &limits, &mut out) {
                Ok(d) => format!("{}", d.pages.len().max(1)),
                Err(_) => "1".to_owned(),
            }
        }
        "extract" => {
            let mut out = Emitter::with_deadline(PdfExtractor::ID, Instant::now() + limits.timeout);
            let r = PdfExtractor::new().extract(&blob, &limits, &mut out);
            let probs = out.take_problems();
            match &r {
                Ok(d) if probs.is_empty() => format!("ok pages={}", d.pages.len()),
                Ok(d) => format!("partial pages={}", d.pages.len()),
                Err(e) => format!("error {e}"),
            }
        }
        _ => {
            let r = PdfRaster::new(blob);
            let (mut ok, mut none, mut errs) = (0, 0, Vec::<String>::new());
            for p in 0..pages {
                if mode == "scan" {
                    match r.embedded_scan(p, &limits) {
                        None => none += 1,
                        Some(Ok(_)) => ok += 1,
                        Some(Err(e)) => errs.push(e.to_string()),
                    }
                } else {
                    match r.render(p, dpi, &limits) {
                        Ok(_) => ok += 1,
                        Err(e) => errs.push(e.to_string()),
                    }
                }
            }
            errs.dedup();
            let kind = if errs.is_empty() {
                "ok"
            } else if ok + none > 0 {
                "partial"
            } else {
                "error"
            };
            let first = errs.first().map_or(String::new(), |e| format!(" ({e})"));
            format!("{kind} ok={ok} none={none}{first}")
        }
    };
    println!(
        "RESULT\t{res}\t{:.2}\t{}",
        t.elapsed().as_secs_f64(),
        hwm_kb()
    );
}

// ---------------------------------------------------------------------------------------------
// Parent: generate, run the matrix, print the table.

struct Outcome {
    text: String,
    bad: bool,
}

fn run_child(exe: &Path, file: &Path, args: &[String]) -> Outcome {
    let t = Instant::now();
    let mut child = match Command::new("prlimit")
        .arg(AS_LIMIT)
        .arg(exe)
        .arg("--child")
        .arg(file)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Outcome {
                text: format!("SPAWN FAILED {e}"),
                bad: true,
            };
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t.elapsed() > CHILD_KILL => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome {
                    text: format!("TIMEOUT >{}s", CHILD_KILL.as_secs()),
                    bad: true,
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let out = child
        .wait_with_output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    let wall = t.elapsed().as_secs_f64();
    let text = String::from_utf8_lossy(&out);
    match text.lines().find(|l| l.starts_with("RESULT\t")) {
        Some(line) => {
            let f: Vec<&str> = line.split('\t').collect();
            let secs: f64 = f.get(2).and_then(|s| s.parse().ok()).unwrap_or(wall);
            let kb: u64 = f.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            let slow = secs > 2.0 * TIMEOUT.as_secs_f64();
            Outcome {
                text: format!(
                    "{}{} {:.1}s {}MB",
                    if slow { "SLOW " } else { "" },
                    f.get(1).unwrap_or(&""),
                    secs,
                    kb / 1024
                ),
                bad: slow,
            }
        }
        None => Outcome {
            text: format!("ABORT after {wall:.1}s"),
            bad: true,
        },
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--child") {
        let path = PathBuf::from(&args[2]);
        let mode = args[3].as_str();
        let pages = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
        let dpi = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(150.0);
        child(&path, mode, pages, dpi);
        return;
    }
    let dir = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "target/hostile-cases".into()),
    );
    let filter = args.get(2).cloned();
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap();
    println!("| file | extract | render 150 dpi | render 300 dpi | scan |");
    println!("|---|---|---|---|---|");
    let (mut bad, mut total) = (0, 0);
    for (name, make) in cases() {
        if filter.as_deref().is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let path = dir.join(format!("{name}.pdf"));
        if !path.exists() {
            std::fs::write(&path, make()).unwrap();
        }
        let n: u32 = {
            let o = Command::new("prlimit")
                .arg(AS_LIMIT)
                .arg(&exe)
                .arg("--child")
                .arg(&path)
                .arg("count")
                .output();
            o.ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .find(|l| l.starts_with("RESULT\t"))
                        .and_then(|l| l.split('\t').nth(1).and_then(|n| n.parse().ok()))
                })
                .unwrap_or(1)
        };
        let np = n.clamp(1, 20).to_string();
        let modes: [Vec<String>; 4] = [
            vec!["extract".into()],
            vec!["render".into(), np.clone(), "150".into()],
            vec!["render".into(), np.clone(), "300".into()],
            vec!["scan".into(), np.clone()],
        ];
        let mut row = format!("| {name} ({n} p) |");
        for m in &modes {
            let o = run_child(&exe, &path, m);
            total += 1;
            if o.bad {
                bad += 1;
            }
            row.push_str(&format!(" {} |", o.text));
        }
        println!("{row}");
        let _ = std::io::stdout().flush();
    }
    println!();
    println!(
        "{bad} of {total} runs aborted, timed out or took more than {}s (prlimit {AS_LIMIT}, default Limits, timeout {}s)",
        2 * TIMEOUT.as_secs(),
        TIMEOUT.as_secs()
    );
}

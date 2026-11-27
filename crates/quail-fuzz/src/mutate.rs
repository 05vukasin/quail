//! Mutators: plain byte edits for every input, plus structure-aware ones that take DOCX and
//! PPTX packages apart (mutate the XML parts, re-zip with lies), edit PDF objects and content
//! streams (re-compressed, xref rebuilt) so mutations reach the parsers behind the container,
//! and inject hostile HTML (deep nesting, endless tokens, charset swaps, inline XBRL).

use std::sync::OnceLock;

use crate::pdfgen::{self, find, rfind, stream};
use crate::rng::Rng;
use crate::seeds::{BUSINESS, Kind, TRICKY};
use crate::zipfmt::{self, Entry, Eocd, Lie, Method};

/// Largest input a mutation may produce (bombs stay small: they are compressed).
pub const MAX_LEN: usize = 8 << 20;

/// Numbers that break size and offset arithmetic.
const NUMBERS: &[&str] = &[
    "0",
    "-1",
    "1",
    "2",
    "7",
    "255",
    "256",
    "65535",
    "65536",
    "1000000",
    "100000000",
    "2147483647",
    "2147483648",
    "-2147483648",
    "4294967295",
    "4294967296",
    "9223372036854775807",
    "9223372036854775808",
    "18446744073709551616",
    "99999999999999999999999999",
    "1e308",
    "-1e308",
    "1e-308",
    "NaN",
    "inf",
    "-0",
    "0.000001",
    "3.4028236e38",
    "00000000000000000000001",
    "1.5",
    "-",
];

/// Tokens that change PDF syntax.
const PDF_TOKENS: &[&str] = &[
    "<<",
    ">>",
    "[",
    "]",
    " R",
    " 0 R",
    "obj",
    "endobj",
    "stream\n",
    "\nendstream",
    "(",
    ")",
    "\\",
    "%",
    "<",
    ">",
    "null",
    "true",
    "/",
    "xref",
    "trailer",
    "startxref",
    "%%EOF",
    "\r",
    "\n",
    " 1 0 obj ",
    "/Type /Page",
    "/Type /Pages",
    "/Kids [",
    "/Count ",
    "/Length ",
    "/Filter /FlateDecode",
    "/Parent 1 0 R",
    "/Contents ",
    "/Resources << >>",
    "<FEFF",
    "(\\377\\376",
    "(\\(\\)\\\\\\n\\r\\t\\b\\f\\0\\777)",
    "/#00#FF",
    "1 0 R 2 0 R 3 0 R",
];

/// Names a PDF parser dispatches on.
const PDF_NAMES: &[&str] = &[
    "/FlateDecode",
    "/LZWDecode",
    "/ASCIIHexDecode",
    "/ASCII85Decode",
    "/RunLengthDecode",
    "/DCTDecode",
    "/JPXDecode",
    "/JBIG2Decode",
    "/CCITTFaxDecode",
    "/Crypt",
    "/Type",
    "/Page",
    "/Pages",
    "/Catalog",
    "/XObject",
    "/Form",
    "/Image",
    "/Font",
    "/Type0",
    "/Type1",
    "/Type3",
    "/TrueType",
    "/CIDFontType0",
    "/CIDFontType2",
    "/Identity-H",
    "/Identity-V",
    "/ObjStm",
    "/XRef",
    "/Encrypt",
    "/Contents",
    "/Resources",
    "/Kids",
    "/Parent",
    "/MediaBox",
    "/CropBox",
    "/Rotate",
    "/Annots",
    "/AcroForm",
    "/Names",
    "/Outlines",
    "/ToUnicode",
    "/Encoding",
    "/Differences",
    "/Widths",
    "/DescendantFonts",
    "/DeviceRGB",
    "/DeviceCMYK",
    "/DeviceGray",
    "/Indexed",
    "/ICCBased",
    "/Pattern",
    "/Separation",
    "/DeviceN",
    "/Lab",
    "/Shading",
    "/Function",
    "/Predictor",
    "/Columns",
    "/BitsPerComponent",
    "/Width",
    "/Height",
    "/Length",
    "/Prev",
    "/Root",
    "/Info",
    "/Size",
    "/W",
    "/Index",
    "/N",
    "/First",
    "/Extends",
    "/SMask",
    "/Mask",
    "/Decode",
    "/ImageMask",
];

/// Content-stream fragments.
const CONTENT: &[&str] = &[
    "q ",
    "Q ",
    "BT ",
    "ET ",
    "1e38 1e38 Td ",
    "-1e38 0 Td ",
    "999999 Tz ",
    "-1 Tc ",
    "1e30 Tw ",
    "0 0 0 0 0 0 cm ",
    "NaN 0 0 1 0 0 cm ",
    "1e30 0 0 1e30 0 0 cm ",
    "(x) Tj ",
    "0 -0.001 Td (y) Tj ",
    "[(a) -99999999 (b) 1e30 (c)] TJ ",
    "/F1 1e30 Tf ",
    "/F1 0 Tf ",
    "/F1 -12 Tf ",
    "/F9 12 Tf ",
    "/X9 Do ",
    "/Im1 Do ",
    "/X Do ",
    "BI /W 99999 /H 99999 /BPC 8 /CS /RGB ID xyz EI ",
    "BI /W 1 /H 1 ID EI ",
    "/Sh0 sh ",
    "/P0 scn ",
    "0 0 d0 ",
    "0 0 0 0 0 0 d1 ",
    "0 0 612 792 re W n ",
    "1 0 0 RG ",
    "/GS0 gs ",
    "[1 2] 0 d ",
    "(unbalanced Tj ",
    "<4142 Tj ",
    "<<>> BDC ",
    "EMC ",
    "/OC /MC0 BDC ",
    "1 2 3 4 5 6 7 8 9 10 Tj ",
    "T* ' \" ",
    "Tj Tj Tj ",
    "100 Tr ",
    "0 0 m 1e30 1e30 l S ",
    "0 0 m 1 1 2 2 3 3 c f ",
    "%comment\n",
    "BX ",
    "EX ",
];

/// OOXML fragments.
const OOXML: &[&str] = &[
    "<w:p>",
    "</w:p>",
    "<w:r>",
    "</w:r>",
    "<w:t>",
    "</w:t>",
    "<w:p/>",
    "<w:r><w:t>x</w:t></w:r>",
    "<w:tbl>",
    "</w:tbl>",
    "<w:tr>",
    "<w:tc>",
    "<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>",
    r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
    r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
    r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
    r#"<w:r><w:instrText>HYPERLINK "x"</w:instrText></w:r>"#,
    r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#,
    r#"<w:footnoteReference w:id="1"/>"#,
    r#"<w:footnoteReference w:id="-99999999999"/>"#,
    r#"<w:commentReference w:id="0"/>"#,
    r#"<w:endnoteReference w:id="2"/>"#,
    r#"<w:sectPr><w:pgSz w:w="0" w:h="0"/></w:sectPr>"#,
    "<w:sectPr/>",
    r#"<w:pPr><w:pStyle w:val="Heading1"/></w:pPr>"#,
    r#"<w:pPr><w:numPr><w:ilvl w:val="8"/><w:numId w:val="1"/></w:numPr></w:pPr>"#,
    r#"<w:pPr><w:numPr><w:ilvl w:val="2147483647"/><w:numId w:val="-1"/></w:numPr></w:pPr>"#,
    r#"<w:hyperlink r:id="rId999"><w:r><w:t>l</w:t></w:r></w:hyperlink>"#,
    "<w:ins><w:r><w:t>i</w:t></w:r></w:ins>",
    "<w:del><w:r><w:delText>d</w:delText></w:r></w:del>",
    "<w:sdt><w:sdtContent>",
    "</w:sdtContent></w:sdt>",
    "<w:txbxContent>",
    "</w:txbxContent>",
    "<mc:AlternateContent><mc:Choice Requires=\"x\">",
    "</mc:Choice></mc:AlternateContent>",
    "<w:tab/>",
    "<w:br/>",
    "<w:cr/>",
    "<w:noBreakHyphen/>",
    r#"<w:sym w:char="FFFF"/>"#,
    r#"<w:gridSpan w:val="65535"/>"#,
    r#"<w:vMerge w:val="restart"/>"#,
    "<w:vMerge/>",
    "<![CDATA[x]]>",
    "<!-- c -->",
    "<?pi x?>",
    "&amp;",
    "&lt;",
    "&#0;",
    "&#xD800;",
    "&#x10FFFF;",
    "&#99999999999;",
    "&undefined;",
    "<w:t xml:space=\"preserve\">  </w:t>",
    "<x:unknown xmlns:x=\"u\"/>",
    r#"<w:bookmarkStart w:id="1" w:name="b"/>"#,
    r#"<w:bookmarkEnd w:id="1"/>"#,
];

/// Pairs used to inject deep nesting.
const NESTS: &[(&str, &str)] = &[
    ("<w:p>", "</w:p>"),
    ("<w:tbl><w:tr><w:tc>", "</w:tc></w:tr></w:tbl>"),
    ("<w:sdt><w:sdtContent>", "</w:sdtContent></w:sdt>"),
    ("<w:r>", "</w:r>"),
    ("<w:ins>", "</w:ins>"),
    ("<w:hyperlink>", "</w:hyperlink>"),
    (r#"<w:fldSimple w:instr="PAGE">"#, "</w:fldSimple>"),
    ("<w:txbxContent>", "</w:txbxContent>"),
    (
        "<mc:AlternateContent><mc:Choice>",
        "</mc:Choice></mc:AlternateContent>",
    ),
    ("<w:smartTag>", "</w:smartTag>"),
    ("<a>", "</a>"),
    (r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#, ""),
];

/// Attribute values.
const ATTRS: &[&str] = &[
    "",
    "-1",
    "0",
    "2147483648",
    "99999999999999999999",
    "1e308",
    "NaN",
    "%1%1%1%1%1%1%1%1%1%1%1%1%1%1",
    "&#0;",
    "&#xD800;",
    "&lt;&gt;",
    "rId1",
    "../../../../x.xml",
    "/word/document.xml",
    "#bm1",
    "http://example.com",
    "\u{202e}x",
    "ß",
    "true",
    "false",
    "on",
    "auto",
    "decimal",
    "bullet",
    "upperRoman",
    "lowerLetter",
    "ordinal",
    "cardinalText",
    "none",
];

/// Prefix of the wordprocessing namespace and a few others it can be swapped for.
const NAMESPACES: &[&str] = &[
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
    "",
    "urn:x",
];

/// Caches of expensive payloads, shared by all jobs.
static ZERO_BOMB: OnceLock<Vec<u8>> = OnceLock::new();
static PDF_BOMB: OnceLock<Vec<u8>> = OnceLock::new();

/// Size of the cached zip bomb entry.
const BOMB_SIZE: usize = 256 << 20;

fn zero_bomb() -> &'static [u8] {
    ZERO_BOMB.get_or_init(|| zipfmt::zero_bomb(BOMB_SIZE))
}

/// Two Flate layers around 128 MiB of zeros (inner layer zlib, outer zlib).
fn pdf_bomb() -> &'static [u8] {
    PDF_BOMB.get_or_init(|| {
        let raw = zipfmt::zero_bomb(128 << 20);
        // zlib framing around the raw deflate: header, data, (fake) adler
        let mut inner = vec![0x78, 0x9c];
        inner.extend_from_slice(&raw);
        inner.extend_from_slice(&[0, 0, 0, 1]);
        pdfgen::zlib(&inner)
    })
}

/// Text that stresses the analyzers: long runs, many matches, unbalanced constructs.
pub fn hostile_text(rng: &mut Rng) -> String {
    let n = rng.log_size(200_000);
    match rng.below(18) {
        0 => "a".repeat(n),
        1 => "1".repeat(n),
        2 => "1.".repeat(n),
        3 => format!("e{}", "\u{301}".repeat(n)),
        4 => "\u{202e}".repeat(n),
        5 => "a@a.".repeat(n / 4 + 1),
        6 => "Ugovor br. ".repeat(n / 8 + 1),
        7 => "(u daljem tekstu: \"".repeat(n / 16 + 1),
        8 => "d.o.o. ".repeat(n / 7 + 1),
        9 => "\"".repeat(n),
        10 => "\n".repeat(n),
        11 => "\u{c}".repeat(n.min(5000)),
        12 => "RS35 2600 0560 1001 6113 79 ".repeat(n / 28 + 1),
        13 => "15.03.2024. ".repeat(n / 12 + 1),
        14 => "1.234.567,89 RSD ".repeat(n / 17 + 1),
        15 => TRICKY.join("").repeat(n / 200 + 1),
        16 => format!("{}{}", "(".repeat(n / 2), ")".repeat(n / 3)),
        _ => BUSINESS.join(" ").repeat(n / 400 + 1),
    }
}

// -- shared byte-level helpers ------------------------------------------------------------------

fn cap(v: &mut Vec<u8>) {
    v.truncate(MAX_LEN);
}

fn insert(v: &mut Vec<u8>, at: usize, what: &[u8]) {
    let at = at.min(v.len());
    if v.len() + what.len() > MAX_LEN {
        return;
    }
    v.splice(at..at, what.iter().copied());
}

/// Replaces a random run of ASCII digits with an interesting number.
fn number(rng: &mut Rng, v: &mut Vec<u8>) -> bool {
    if v.is_empty() {
        return false;
    }
    let start = rng.below(v.len());
    let Some(off) = v[start..].iter().position(u8::is_ascii_digit) else {
        return false;
    };
    let s = start + off;
    let e = s + v[s..].iter().take_while(|b| b.is_ascii_digit()).count();
    let new: Vec<u8> = if rng.chance(1, 4) {
        let k = rng.range(2, 9);
        v[s..e].repeat(k)
    } else {
        rng.pick(NUMBERS).as_bytes().to_vec()
    };
    v.splice(s..e, new);
    true
}

/// One plain byte mutation; returns its name.
fn bytes(rng: &mut Rng, v: &mut Vec<u8>, dict: &[&str], other: &[u8]) -> &'static str {
    let len = v.len();
    match rng.below(12) {
        0 if len > 0 => {
            let i = rng.below(len);
            v[i] ^= 1 << rng.below(8);
            "bitflip"
        }
        1 if len > 0 => {
            let i = rng.below(len);
            v[i] = *rng.pick(&[
                0, 0xff, 0x7f, 0x80, b'<', b'>', b'(', b')', b'/', b'[', b']', b'"', b'\n', b'&',
                b'%',
            ]);
            "byte"
        }
        2 if len >= 4 => {
            let i = rng.below(len - 3);
            let x: u32 = *rng.pick(&[
                0,
                u32::MAX,
                0x7fff_ffff,
                0x8000_0000,
                0xffff,
                0x1_0000,
                u32::try_from(len).unwrap_or(0),
            ]);
            let b = if rng.chance(1, 2) {
                x.to_le_bytes()
            } else {
                x.to_be_bytes()
            };
            v[i..i + 4].copy_from_slice(&b);
            "int"
        }
        3 => {
            let n = rng.log_size(64);
            let r: Vec<u8> = (0..n)
                .map(|_| u8::try_from(rng.below(256)).unwrap_or(0))
                .collect();
            let at = rng.below(len + 1);
            insert(v, at, &r);
            "insert"
        }
        4 if !dict.is_empty() => {
            let at = rng.below(len + 1);
            let t = rng.pick(dict).as_bytes().to_vec();
            insert(v, at, &t);
            "token"
        }
        5 if len > 1 => {
            let s = rng.below(len);
            let n = rng.log_size(len - s).min(len - s);
            v.drain(s..s + n);
            "delete"
        }
        6 if len > 1 => {
            let s = rng.below(len);
            let n = rng.log_size((len - s).min(4096));
            let chunk = v[s..(s + n).min(len)].to_vec();
            let at = rng.below(len + 1);
            insert(v, at, &chunk);
            "duplicate"
        }
        7 if len > 1 => {
            let s = rng.below(len);
            let n = rng.log_size((len - s).min(256));
            let chunk = v[s..(s + n).min(len)].to_vec();
            let times = rng.log_size(MAX_LEN / chunk.len().max(1) / 2).max(2);
            let rep = chunk.repeat(times);
            insert(v, s, &rep);
            "amplify"
        }
        8 if !other.is_empty() && len > 0 => {
            let a = rng.below(len);
            let b = rng.below(other.len());
            v.truncate(a);
            v.extend_from_slice(&other[b..]);
            cap(v);
            "splice"
        }
        9 if len > 1 => {
            v.truncate(rng.below(len));
            "truncate"
        }
        _ => {
            if number(rng, v) {
                "number"
            } else {
                v.push(b'0');
                "append"
            }
        }
    }
}

// -- XML ------------------------------------------------------------------------------------------

/// A random position just before a `<` (or anywhere when there is none).
fn tag_pos(rng: &mut Rng, v: &[u8]) -> usize {
    if v.is_empty() {
        return 0;
    }
    let start = rng.below(v.len());
    v[start..]
        .iter()
        .position(|&b| b == b'<')
        .map_or(start, |p| start + p)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One XML-aware mutation of a part.
fn xml(rng: &mut Rng, v: &mut Vec<u8>) -> &'static str {
    match rng.below(16) {
        0 | 1 => {
            let at = tag_pos(rng, v);
            let t = rng.pick(OOXML).as_bytes().to_vec();
            insert(v, at, &t);
            "xml-snippet"
        }
        2 | 3 => {
            let (open, close) = *rng.pick(NESTS);
            let n = *rng.pick(&[10usize, 100, 1000, 10_000, 100_000, 1_000_000]);
            let n = n.min(MAX_LEN / (open.len() + close.len()).max(1) / 2);
            let unclosed = rng.chance(1, 3);
            let mut s = open.repeat(n);
            if !unclosed {
                s.push_str(&close.repeat(n));
            }
            let at = find(v, b"<w:body>").map_or_else(|| tag_pos(rng, v), |p| p + 8);
            insert(v, at, s.as_bytes());
            "xml-nest"
        }
        4 => {
            // repeat one element
            let Some(s) = find(v, b"<w:p>").or_else(|| find(v, b"<w:p ")) else {
                return "xml-none";
            };
            let Some(e) = find(&v[s..], b"</w:p>") else {
                return "xml-none";
            };
            let el = v[s..s + e + 6].to_vec();
            let k = rng.log_size(MAX_LEN / el.len().max(1) / 2).max(2);
            insert(v, s, &el.repeat(k));
            "xml-repeat"
        }
        5 if !v.is_empty() => {
            let s = tag_pos(rng, v);
            if let Some(e) = v[s..].iter().position(|&b| b == b'>') {
                v.drain(s..=s + e);
            }
            "xml-deltag"
        }
        6 => {
            let s = rng.below(v.len().max(1));
            let Some(p) = find(&v[s.min(v.len())..], b"=\"") else {
                return "xml-none";
            };
            let a = s + p + 2;
            let Some(e) = v[a..].iter().position(|&b| b == b'"') else {
                return "xml-none";
            };
            let val = if rng.chance(1, 5) {
                "A".repeat(rng.log_size(1 << 20))
            } else {
                (*rng.pick(ATTRS)).to_owned()
            };
            v.splice(a..a + e, val.into_bytes());
            "xml-attr"
        }
        7 => {
            let dtd = match rng.below(4) {
                0 => {
                    r#"<!DOCTYPE d [<!ENTITY a "AAAAAAAAAAAAAAAAAAAA"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;">]>"#
                }
                1 => r#"<!DOCTYPE d SYSTEM "file:///etc/passwd">"#,
                2 => r#"<!DOCTYPE d [<!ENTITY % p SYSTEM "http://example.com/x"> %p;]>"#,
                _ => r#"<!DOCTYPE d [<!ENTITY e "&e;">]>"#,
            };
            let at = find(v, b"?>").map_or(0, |p| p + 2);
            insert(v, at, dtd.as_bytes());
            if rng.chance(1, 2) {
                let at = find(v, b"<w:t>").map_or_else(|| tag_pos(rng, v), |p| p + 5);
                insert(v, at, b"&d;&e;&c;");
            }
            "xml-doctype"
        }
        8 => {
            match rng.below(4) {
                0 => insert(v, 0, b"\xef\xbb\xbf"),
                1 => {
                    if let Some(p) = find(v, b"UTF-8") {
                        v.splice(p..p + 5, b"UTF-16".iter().copied());
                    }
                }
                2 => {
                    // re-encode as UTF-16LE with a BOM
                    if let Ok(s) = std::str::from_utf8(v) {
                        let mut out = vec![0xff, 0xfe];
                        for c in s.encode_utf16() {
                            out.extend_from_slice(&c.to_le_bytes());
                        }
                        *v = out;
                        cap(v);
                    }
                }
                _ => insert(v, 0, b"\xfe\xff\x00<"),
            }
            "xml-encoding"
        }
        9 => {
            let t = esc(&hostile_text(rng));
            let at = find(v, b"<w:t>")
                .or_else(|| find(v, b"<w:t xml:space=\"preserve\">").map(|p| p + 21))
                .map_or_else(|| tag_pos(rng, v), |p| p + 5);
            insert(v, at, t.as_bytes());
            "xml-text"
        }
        10 => {
            if let Some(p) = find(v, b"wordprocessingml/2006/main") {
                let s = v[..p].iter().rposition(|&b| b == b'"').unwrap_or(p);
                let e = p + v[p..].iter().position(|&b| b == b'"').unwrap_or(0);
                v.splice(s + 1..e, rng.pick(NAMESPACES).bytes());
            } else if let Some(p) = find(v, b"xmlns") {
                v.drain(p..p + 5);
            }
            "xml-namespace"
        }
        11 => {
            let k = rng.log_size(100_000);
            let s = rng.pick(&[
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
                r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#,
                "<w:sectPr/>",
                r#"<w:bookmarkStart w:id="1" w:name="b"/>"#,
                "<w:p/>",
            ]);
            let at = find(v, b"<w:p>").map_or_else(|| tag_pos(rng, v), |p| p + 5);
            insert(v, at, s.repeat(k).as_bytes());
            "xml-many"
        }
        _ => bytes(rng, v, OOXML, &[]),
    }
}

// -- DOCX packages -------------------------------------------------------------------------------

fn is_xml(name: &[u8]) -> bool {
    name.ends_with(b".xml") || name.ends_with(b".rels")
}

/// Mutates a zip package; `None` when it is not readable as one.
fn docx(rng: &mut Rng, input: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    let parts = zipfmt::read(input, 16 << 20)?;
    let mut es: Vec<Entry> = parts.into_iter().map(|(n, d)| Entry::new(n, d)).collect();
    let mut eocd = Eocd::default();
    let mut prefix = Vec::new();
    let mut name = "zip";
    let rounds = rng.range(1, 3);
    for _ in 0..rounds {
        let i = rng.below(es.len());
        name = match rng.below(20) {
            0..=8 => {
                // mutate an XML part, the main document most often
                let i = if rng.chance(1, 2) {
                    es.iter()
                        .position(|e| {
                            e.name == b"word/document.xml" || e.name == b"ppt/slides/slide1.xml"
                        })
                        .unwrap_or(i)
                } else {
                    let xmls: Vec<usize> = (0..es.len()).filter(|&j| is_xml(&es[j].name)).collect();
                    if xmls.is_empty() { i } else { *rng.pick(&xmls) }
                };
                let n = rng.range(1, 3);
                let mut last = "xml";
                for _ in 0..n {
                    last = xml(rng, &mut es[i].data);
                }
                last
            }
            9 if es.len() > 1 => {
                es.remove(i);
                "zip-drop"
            }
            10 => {
                let mut dup = es[i].clone();
                xml(rng, &mut dup.data);
                let at = rng.below(es.len() + 1);
                es.insert(at, dup);
                "zip-duplicate"
            }
            11 => {
                let n = &mut es[i].name;
                match rng.below(5) {
                    0 => n.make_ascii_uppercase(),
                    1 => n.insert(0, b'/'),
                    2 => {
                        for b in n.iter_mut() {
                            if *b == b'/' {
                                *b = b'\\';
                            }
                        }
                    }
                    3 => n.splice(0..0, b"../".iter().copied()).for_each(drop),
                    _ => n.extend_from_slice(" \u{0}\u{fffd}".as_bytes()),
                }
                "zip-rename"
            }
            12 => {
                let k = rng.log_size(60_000);
                for j in 0..k {
                    es.push(Entry {
                        method: Method::Store,
                        ..Entry::new(format!("a/{j}"), Vec::new())
                    });
                }
                "zip-many-entries"
            }
            13 => {
                // a bomb: the main document (or any part) inflates to 256 MiB of zeros
                let bomb = zero_bomb().to_vec();
                let crc = 0; // CRC of the real data is not worth computing; readers must cope
                let e = &mut es[i];
                e.raw = Some((bomb, crc, u32::try_from(BOMB_SIZE).unwrap_or(u32::MAX)));
                e.method = Method::Deflate;
                "zip-bomb"
            }
            14 => {
                es[i].lie = match rng.below(5) {
                    0 => Lie::Size(*rng.pick(&[0, 1, u32::MAX, 0x7fff_ffff, 0x1000_0000])),
                    1 => Lie::CompSize(*rng.pick(&[0, 1, u32::MAX, 0x7fff_ffff])),
                    2 => Lie::Crc,
                    _ => Lie::CentralName,
                };
                "zip-header-lie"
            }
            15 => {
                es[i].method = match rng.below(5) {
                    0 => Method::Store,
                    1 => Method::Claim(0),
                    2 => Method::Claim(12),
                    3 => Method::Claim(14),
                    _ => Method::Claim(99),
                };
                "zip-method"
            }
            16 => {
                if rng.chance(1, 2) {
                    eocd.count = Some(*rng.pick(&[0, 1, u16::MAX, 2]));
                } else {
                    eocd.offset_delta = *rng.pick(&[-1i64, 1, -22, 1000, -100_000]);
                }
                if rng.chance(1, 3) {
                    prefix = b"MZ\x90\x00 self-extracting stub ".repeat(rng.log_size(64));
                }
                "zip-eocd-lie"
            }
            17 => {
                // relationships pointing at themselves, outside the package, or in cycles
                let rel = rng.pick(&[
                    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="document.xml"/>"#,
                    r#"<Relationship Id="rIdX" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="../../../../etc/passwd"/>"#,
                    r#"<Relationship Id="rIdY" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="/word/document.xml"/>"#,
                    r#"<Relationship Id="rIdZ" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>"#,
                    r#"<Relationship Id="rId8" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/>"#,
                    r#"<Relationship Id="" Type="" Target=""/>"#,
                    r#"<Relationship Id="rIdE" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="file:///C:/x" TargetMode="External"/>"#,
                ]);
                let k = if rng.chance(1, 4) {
                    rng.log_size(50_000)
                } else {
                    1
                };
                let rels: Vec<usize> = (0..es.len())
                    .filter(|&j| es[j].name.ends_with(b".rels"))
                    .collect();
                if let Some(&j) = rels.get(rng.below(rels.len().max(1))) {
                    let at = rfind(&es[j].data, b"</Relationships>").unwrap_or(es[j].data.len());
                    insert(&mut es[j].data, at, rel.repeat(k).as_bytes());
                }
                "zip-rels"
            }
            18 => {
                // the main part under another content type, or a part of a type the reader skips
                if let Some(j) = es.iter().position(|e| e.name == b"[Content_Types].xml") {
                    let ct = rng.pick(&[
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
                        "application/vnd.ms-word.document.macroEnabled.main+xml",
                        "application/xml",
                        "",
                    ]);
                    let d = String::from_utf8_lossy(&es[j].data).replace(
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
                        ct,
                    );
                    es[j].data = d.into_bytes();
                }
                "zip-content-type"
            }
            _ => {
                let a = rng.below(es.len());
                es.swap(i, a);
                "zip-shuffle"
            }
        };
    }
    if rng.chance(1, 5) {
        for e in &mut es {
            if e.method == Method::Deflate && e.raw.is_none() {
                e.method = Method::Store;
            }
        }
    }
    let mut out = zipfmt::write(&es, eocd, &prefix);
    if rng.chance(1, 20) {
        out.extend_from_slice(b"trailing junk after the directory");
    }
    cap(&mut out);
    Some((out, name))
}

// -- PDF -----------------------------------------------------------------------------------------

/// Byte range of the data of a random stream, and the start of its dictionary.
fn pick_stream(rng: &mut Rng, pdf: &[u8]) -> Option<(usize, usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while let Some(p) = find(&pdf[i..], b"stream") {
        let at = i + p;
        // "stream" not "endstream"
        if at < 3 || &pdf[at - 3..at] != b"end" {
            starts.push(at);
        }
        i = at + 6;
        if starts.len() > 2000 {
            break;
        }
    }
    let at = *starts.get(rng.below(starts.len()))?;
    let mut ds = at + 6;
    if pdf.get(ds) == Some(&b'\r') {
        ds += 1;
    }
    if pdf.get(ds) == Some(&b'\n') {
        ds += 1;
    }
    let de = ds + find(&pdf[ds..], b"endstream")?;
    let dict = rfind(&pdf[..at], b"<<")?;
    Some((dict, ds, de))
}

/// Mutates the decoded data of a stream (a content stream most of the time) and writes it
/// back compressed with a matching `/Length`.
fn pdf_content(rng: &mut Rng, pdf: &[u8]) -> Option<Vec<u8>> {
    let (dict, ds, de) = pick_stream(rng, pdf)?;
    let mut end = de;
    while end > ds && matches!(pdf[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    let d = &pdf[dict..ds];
    let flate = find(d, b"/FlateDecode").is_some();
    let raw = &pdf[ds..end];
    let mut data = if flate {
        pdfgen::unzlib(raw, 16 << 20)?
    } else {
        raw.to_vec()
    };
    for _ in 0..rng.range(1, 4) {
        match rng.below(5) {
            0 | 1 => {
                let t = *rng.pick(CONTENT);
                let k = if rng.chance(1, 4) {
                    rng.log_size(100_000)
                } else {
                    1
                };
                let at = rng.below(data.len() + 1);
                insert(&mut data, at, t.repeat(k).as_bytes());
            }
            2 => {
                let t = hostile_text(rng);
                let s = format!(
                    "BT /F1 12 Tf 72 700 Td ({}) Tj ET ",
                    t.replace(['(', ')', '\\'], " ")
                );
                let at = rng.below(data.len() + 1);
                insert(&mut data, at, s.as_bytes());
            }
            3 => {
                number(rng, &mut data);
            }
            _ => {
                bytes(rng, &mut data, CONTENT, &[]);
            }
        }
    }
    let body = if flate { pdfgen::zlib(&data) } else { data };
    let mut dict_bytes = pdf[dict..ds].to_vec();
    if let Some(p) = find(&dict_bytes, b"/Length") {
        let s = p + 7;
        let e = s + dict_bytes[s..]
            .iter()
            .take_while(|b| b.is_ascii_whitespace() || b.is_ascii_digit())
            .count();
        // an indirect length (`5 0 R`) becomes `<n> R`: one more broken object to survive
        dict_bytes.splice(s..e, format!(" {} ", body.len()).into_bytes());
    }
    let mut out = pdf[..dict].to_vec();
    out.extend_from_slice(&dict_bytes);
    out.extend_from_slice(&body);
    out.extend_from_slice(b"\n");
    out.extend_from_slice(&pdf[de..]);
    Some(out)
}

/// Numbered objects, the pages among them, and the construct's name.
type Hostile = (Vec<(u32, Vec<u8>)>, Vec<u32>, &'static str);

/// Objects of one hostile construct, numbered from `b`, and the pages among them (whose
/// `/Parent` is `{P}`, filled in later).
fn hostile_pdf(rng: &mut Rng, b: u32) -> Hostile {
    let font = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>";
    let page = |n: u32, contents: &str, res: &str| -> (u32, Vec<u8>) {
        (
            n,
            format!("<< /Type /Page /Parent {{P}} /MediaBox [0 0 612 792] /Resources << {font} {res} >> /Contents {contents} >>")
                .into_bytes(),
        )
    };
    let form = |content: &[u8], res: &str| {
        stream(
            content,
            format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << {font} {res} >>"
            )
            .as_bytes(),
            true,
        )
    };
    let c = b + 1;
    match rng.below(20) {
        0 => {
            // chain of forms, each drawing the next twice: 2^n draws
            let n = u32::try_from(rng.range(8, 40)).unwrap_or(20);
            let mut objs = vec![page(
                b,
                &format!("{c} 0 R"),
                &format!("/XObject << /X {} 0 R >>", c + 1),
            )];
            objs.push((c, stream(b"/X Do", b"", true)));
            for k in 0..n {
                let me = c + 1 + k;
                let body: &[u8] = if k + 1 < n {
                    b"/X Do /X Do"
                } else {
                    b"BT /F1 1 Tf (a) Tj ET"
                };
                objs.push((me, form(body, &format!("/XObject << /X {} 0 R >>", me + 1))));
            }
            (objs, vec![b], "pdf-form-fanout")
        }
        1 => {
            let objs = vec![
                page(
                    b,
                    &format!("{c} 0 R"),
                    &format!("/XObject << /X {} 0 R >>", c + 1),
                ),
                (c, stream(b"/X Do", b"", true)),
                (
                    c + 1,
                    form(
                        b"q /X Do Q /Y Do",
                        &format!("/XObject << /X {} 0 R /Y {} 0 R >>", c + 1, c + 2),
                    ),
                ),
                (
                    c + 2,
                    form(b"/X Do", &format!("/XObject << /X {} 0 R >>", c + 1)),
                ),
            ];
            (objs, vec![b], "pdf-form-recursion")
        }
        2 => {
            let k = rng.log_size(200_000);
            let arr = format!("[{}]", format!("{c} 0 R ").repeat(k));
            let objs = vec![
                page(b, &arr, ""),
                (
                    c,
                    stream(b"BT /F1 12 Tf 72 700 Td (repeated) Tj ET ", b"", true),
                ),
            ];
            (objs, vec![b], "pdf-contents-repeat")
        }
        3 => {
            let dim = [
                "0",
                "1",
                "65535",
                "100000",
                "2147483647",
                "4294967295",
                "-1",
                "1e10",
            ];
            let (w, h) = (*rng.pick(&dim), *rng.pick(&dim));
            let bpc = *rng.pick(&["1", "8", "16", "0", "32"]);
            let cs = *rng.pick(&[
                "/DeviceRGB",
                "/DeviceCMYK",
                "/DeviceGray",
                "[/Indexed /DeviceRGB 255 <00>]",
                "[/Indexed /DeviceRGB 999999 <00>]",
                "/Pattern",
                "[/DeviceN [/A /B /C /D /E /F /G /H /I] /DeviceGray 0 0 R]",
                "[/ICCBased 0 0 R]",
                "/Nonexistent",
            ]);
            let filter = *rng.pick(&[
                "", "/Filter /FlateDecode", "/Filter /DCTDecode", "/Filter /JPXDecode", "/Filter /JBIG2Decode",
                "/Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 100000 /Rows 100000 >>",
                "/Filter /RunLengthDecode", "/Filter /LZWDecode",
                "/Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 2147483647 /Colors 64 /BitsPerComponent 16 >>",
            ]);
            let data: &[u8] = if filter == "/Filter /FlateDecode" {
                &[0x78, 0x9c, 3, 0, 0, 0, 0, 1]
            } else {
                b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01garbage"
            };
            let img = stream(data, format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /BitsPerComponent {bpc} /ColorSpace {cs} {filter} ").as_bytes(), false);
            let objs = vec![
                page(
                    b,
                    &format!("{c} 0 R"),
                    &format!("/XObject << /Im0 {} 0 R >>", c + 1),
                ),
                (
                    c,
                    stream(
                        b"q 612 0 0 792 0 0 cm /Im0 Do Q q 1e5 0 0 1e5 0 0 cm /Im0 Do Q",
                        b"",
                        true,
                    ),
                ),
                (c + 1, img),
            ];
            (objs, vec![b], "pdf-huge-image")
        }
        4 => {
            // JBIG2 page information with a huge page size (as in the PDF review)
            let wh =
                u32::try_from(*rng.pick(&[100_000usize, 1 << 20, 0xffff_ffff])).unwrap_or(u32::MAX);
            let seg = |num: u32, typ: u8, data: &[u8]| {
                let mut s = num.to_be_bytes().to_vec();
                s.extend_from_slice(&[typ, 0, 1]);
                s.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
                s.extend_from_slice(data);
                s
            };
            let mut pinfo = wh.to_be_bytes().to_vec();
            pinfo.extend_from_slice(&wh.to_be_bytes());
            pinfo.extend_from_slice(&[0; 9]);
            pinfo.extend_from_slice(&[0, 0]);
            let mut data = seg(0, 48, &pinfo);
            data.extend(seg(1, 49, b""));
            let img = stream(&data, b"/Type /XObject /Subtype /Image /Width 100 /Height 100 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode ", false);
            let objs = vec![
                page(
                    b,
                    &format!("{c} 0 R"),
                    &format!("/XObject << /Im0 {} 0 R >>", c + 1),
                ),
                (c, stream(b"q 612 0 0 792 0 0 cm /Im0 Do Q", b"", true)),
                (c + 1, img),
            ];
            (objs, vec![b], "pdf-jbig2")
        }
        5 => {
            // object stream with an absurd /N, or a double-Flate bomb as a content stream
            let objs = if rng.chance(1, 2) {
                let n = *rng.pick(&["1000000000", "2147483647", "-1", "99999"]);
                vec![
                    page(b, &format!("{c} 0 R"), ""),
                    (c, stream(b"BT /F1 12 Tf 72 700 Td (x) Tj ET", b"", true)),
                    (
                        c + 1,
                        stream(
                            format!("{} 0 {} 5 ", c + 2, c + 3).repeat(50).as_bytes(),
                            format!("/Type /ObjStm /N {n} /First 4").as_bytes(),
                            true,
                        ),
                    ),
                ]
            } else {
                let bomb = pdf_bomb();
                let mut s = format!(
                    "<< /Filter [/FlateDecode /FlateDecode] /Length {} >>\nstream\n",
                    bomb.len()
                )
                .into_bytes();
                s.extend_from_slice(bomb);
                s.extend_from_slice(b"\nendstream");
                vec![page(b, &format!("{c} 0 R"), ""), (c, s)]
            };
            (objs, vec![b], "pdf-objstm-bomb")
        }
        6 => {
            let n = rng.log_size(200_000);
            let content = format!("BT /F1 0.5 Tf 10 780 Td {}ET", "0 -1 Td (a) Tj ".repeat(n));
            (
                vec![
                    page(b, &format!("{c} 0 R"), ""),
                    (c, stream(content.as_bytes(), b"", true)),
                ],
                vec![b],
                "pdf-many-text",
            )
        }
        7 => {
            let n = rng.log_size(500_000);
            let open = *rng.pick(&["[", "<<", "q ", "BT ", "<< /A ", "[[ "]);
            let content = format!("{}BT /F1 12 Tf (deep) Tj ET", open.repeat(n));
            let objs = if rng.chance(1, 2) {
                vec![
                    page(b, &format!("{c} 0 R"), ""),
                    (c, stream(content.as_bytes(), b"", true)),
                ]
            } else {
                vec![(
                    b,
                    format!(
                        "<< /Type /Page /Parent {{P}} /MediaBox [0 0 612 792] /Deep {} >>",
                        "[".repeat(n)
                    )
                    .into_bytes(),
                )]
            };
            (objs, vec![b], "pdf-deep-nesting")
        }
        8 => {
            // a page tree node that is its own kid, with a huge count
            let objs = vec![
                (b, format!("<< /Type /Pages /Parent {{P}} /Kids [{b} 0 R {c} 0 R {b} 0 R] /Count 2147483647 >>").into_bytes()),
                page(c, &format!("{} 0 R", c + 1), ""),
                (c + 1, stream(b"BT /F1 12 Tf 72 700 Td (cycle) Tj ET", b"", true)),
            ];
            let mut objs = objs;
            objs[1].1 = String::from_utf8_lossy(&objs[1].1)
                .replace("{P}", &format!("{b} 0 R"))
                .into_bytes();
            (objs, vec![b], "pdf-page-tree-cycle")
        }
        9 => {
            // Type3 font whose glyph draws text in the same font
            let objs = vec![
                page(b, &format!("{c} 0 R"), &format!("/Font << /T3 {} 0 R >>", c + 1)),
                (c, stream(b"BT /T3 12 Tf 72 700 Td (aaaa) Tj ET", b"", true)),
                (c + 1, format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a {} 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << /Font << /T3 {} 0 R >> >> >>", c + 2, c + 1).into_bytes()),
                (c + 2, stream(b"1000 0 d0 BT /T3 12 Tf (a) Tj ET", b"", true)),
            ];
            (objs, vec![b], "pdf-type3-recursion")
        }
        10 => {
            let objs = vec![
                page(b, &format!("{c} 0 R"), &format!("/Pattern << /P0 {} 0 R >>", c + 1)),
                (c, stream(b"/Pattern cs /P0 scn 0 0 612 792 re f", b"", true)),
                (c + 1, stream(b"/Pattern cs /P0 scn 0 0 10 10 re f", format!("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 0.0001 /YStep 0.0001 /Resources << /Pattern << /P0 {} 0 R >> >>", c + 1).as_bytes(), true)),
            ];
            (objs, vec![b], "pdf-pattern-recursion")
        }
        11 => {
            let k = rng.log_size(100_000);
            let annots = format!("[{}]", format!("{} 0 R ", c + 1).repeat(k));
            let mut p = page(b, &format!("{c} 0 R"), "");
            p.1 = String::from_utf8_lossy(&p.1)
                .replace("/Contents", &format!("/Annots {annots} /Contents"))
                .into_bytes();
            let objs = vec![
                p,
                (c, stream(b"BT /F1 12 Tf 72 700 Td (annots) Tj ET", b"", true)),
                (c + 1, format!("<< /Type /Annot /Subtype /Widget /Rect [0 0 100 100] /FT /Tx /T (f) /V (value 1.000,00) /AP << /N {} 0 R >> /Parent {} 0 R /Kids [{} 0 R] >>", c + 2, c + 1, c + 1).into_bytes()),
                (c + 2, form(b"/X Do", &format!("/XObject << /X {} 0 R >>", c + 2))),
            ];
            (objs, vec![b], "pdf-annots")
        }
        12 => {
            let k = rng.log_size(20_000);
            let cmap = format!(
                "/CIDInit /ProcSet findresource begin begincmap 1 begincodespacerange <0000> <FFFF> endcodespacerange {k} beginbfrange {}endbfrange 1 beginbfchar <0001> <{}> endbfchar endcmap end",
                "<0000> <FFFF> <D83DDE00> ".repeat(k),
                "0041".repeat(rng.log_size(10_000))
            );
            let objs = vec![
                page(b, &format!("{c} 0 R"), &format!("/Font << /F2 {} 0 R >>", c + 1)),
                (c, stream(b"BT /F2 12 Tf 72 700 Td <0001000200030004FFFF> Tj ET", b"", true)),
                (c + 1, format!("<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /W [0 [1e30 -1e30]] >>] /ToUnicode {} 0 R >>", c + 2).into_bytes()),
                (c + 2, stream(cmap.as_bytes(), b"", true)),
            ];
            (objs, vec![b], "pdf-cmap")
        }
        13 => {
            let size = *rng.pick(&[
                "[65535 65535]",
                "[2147483647]",
                "[0]",
                "[100000 100000 100000]",
            ]);
            let objs = vec![
                page(b, &format!("{c} 0 R"), &format!("/Shading << /Sh0 {} 0 R >>", c + 1)),
                (c, stream(b"/Sh0 sh", b"", true)),
                (c + 1, format!("<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 612 792] /Function {} 0 R /Extend [true true] >>", c + 2).into_bytes()),
                (c + 2, stream(b"\x00\x01\x02", format!("/FunctionType 0 /Domain [0 1 0 1 0 1] /Range [0 1 0 1 0 1] /Size {size} /BitsPerSample 32").as_bytes(), false)),
            ];
            (objs, vec![b], "pdf-shading-function")
        }
        14 => {
            let mb = *rng.pick(&[
                "[0 0 1e30 1e30]",
                "[0 0 0 0]",
                "[612 792 0 0]",
                "[-1e30 -1e30 1e30 1e30]",
                "[0 0 NaN 1]",
                "[0 0 612]",
                "[]",
            ]);
            let objs = vec![
                (b, format!("<< /Type /Page /Parent {{P}} /MediaBox {mb} /Rotate {} /UserUnit 75000 /Resources << {font} >> /Contents {c} 0 R >>", rng.pick(&["45", "-90", "2147483647", "270"])).into_bytes()),
                (c, stream(b"BT /F1 12 Tf 72 700 Td (box) Tj ET", b"", true)),
            ];
            (objs, vec![b], "pdf-mediabox")
        }
        15 => {
            // incremental chain: a page redefined many times in one file
            let k = rng.log_size(2000);
            let mut objs = Vec::new();
            for i in 0..k {
                objs.push((
                    c,
                    stream(
                        format!("BT /F1 12 Tf 72 700 Td (rev {i}) Tj ET").as_bytes(),
                        b"",
                        true,
                    ),
                ));
            }
            objs.push(page(b, &format!("{c} 0 R"), ""));
            (objs, vec![b], "pdf-redefined")
        }
        17 => {
            // soft mask over a transparency group with a huge BBox and matrix
            let objs = vec![
                page(b, &format!("{c} 0 R"), &format!("/ExtGState << /GS0 {} 0 R >>", c + 1)),
                (c, stream(b"/GS0 gs 0 0 1 rg 0 0 612 792 re f", b"", true)),
                (c + 1, format!("<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G {} 0 R >> >>", c + 2).into_bytes()),
                (c + 2, stream(b"1 g 0 0 612 792 re f", b"/Type /XObject /Subtype /Form /BBox [-1e9 -1e9 1e9 1e9] /Group << /S /Transparency /CS /DeviceGray >> /Matrix [1000 0 0 1000 0 0]", true)),
            ];
            (objs, vec![b], "pdf-smask-huge")
        }
        18 => {
            // nested transparency groups / tiling patterns, each level drawing the next
            let n = u32::try_from(rng.range(4, 60)).unwrap_or(8);
            let tiling = rng.chance(1, 2);
            let mut objs = if tiling {
                vec![page(
                    b,
                    &format!("{c} 0 R"),
                    &format!("/Pattern << /P0 {} 0 R >>", c + 1),
                )]
            } else {
                vec![page(
                    b,
                    &format!("{c} 0 R"),
                    &format!("/XObject << /G {} 0 R >>", c + 1),
                )]
            };
            objs.push((
                c,
                stream(
                    if tiling {
                        b"/Pattern cs /P0 scn 0 0 612 792 re f"
                    } else {
                        b"/G Do"
                    },
                    b"",
                    true,
                ),
            ));
            for k in 0..n {
                let me = c + 1 + k;
                let last = k + 1 == n;
                let o = if tiling {
                    let body: &[u8] = if last {
                        b"0 0 1 rg 0 0 5 5 re f"
                    } else {
                        b"/Pattern cs /P0 scn 0 0 10 10 re f"
                    };
                    stream(body, format!("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 0.1 /YStep 0.1 /Resources << /Pattern << /P0 {} 0 R >> >>", me + 1).as_bytes(), true)
                } else {
                    let body: &[u8] = if last {
                        b"0 0 1 rg 0 0 612 792 re f"
                    } else {
                        b"0.5 g /GS0 gs /G Do /G Do"
                    };
                    stream(body, format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /K true /I true >> /Resources << /XObject << /G {} 0 R >> /ExtGState << /GS0 << /CA 0.5 /BM /Multiply >> >> >>", me + 1).as_bytes(), true)
                };
                objs.push((me, o));
            }
            (
                objs,
                vec![b],
                if tiling {
                    "pdf-tiling-nested"
                } else {
                    "pdf-groups-nested"
                },
            )
        }
        _ => {
            // an object that is a reference to itself, used as the contents and a resource
            let objs = vec![
                page(
                    b,
                    &format!("{c} 0 R"),
                    &format!("/XObject {c} 0 R /ExtGState << /G {c} 0 R >>"),
                ),
                (c, format!("{c} 0 R").into_bytes()),
            ];
            (objs, vec![b], "pdf-self-reference")
        }
    }
}

/// Appends a hostile construct as an incremental update whose catalog points at it (and at
/// the old page tree, when it can be found).
fn pdf_append(rng: &mut Rng, pdf: &[u8]) -> (Vec<u8>, &'static str) {
    let base = pdfgen::max_object(pdf) + 1;
    let (objs, pages, name) = hostile_pdf(rng, base + 2);
    let old_pages = find(pdf, b"/Pages ").and_then(|p| {
        let s: String = pdf[p + 7..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| char::from(b))
            .collect();
        s.parse::<u32>().ok()
    });
    let pages_id = base + 1;
    let mut kids: Vec<String> = pages.iter().map(|n| format!("{n} 0 R")).collect();
    if let Some(old) = old_pages
        && rng.chance(1, 2)
    {
        kids.push(format!("{old} 0 R"));
    }
    let mut out = pdf.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let prev = rfind(pdf, b"startxref").and_then(|p| {
        let s: String = pdf[p + 9..]
            .iter()
            .skip_while(|b| b.is_ascii_whitespace())
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| char::from(b))
            .collect();
        s.parse::<usize>().ok()
    });
    let mut push = |n: u32, body: &[u8]| {
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    push(
        base,
        format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").as_bytes(),
    );
    push(
        pages_id,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            kids.len()
        )
        .as_bytes(),
    );
    let parent = format!("{pages_id} 0 R");
    for (n, body) in objs {
        let body = if find(&body, b"{P}").is_some() {
            String::from_utf8_lossy(&body)
                .replace("{P}", &parent)
                .into_bytes()
        } else {
            body
        };
        push(n, &body);
    }
    pdfgen::append_xref(&mut out, base, prev);
    cap(&mut out);
    (out, name)
}

/// Breaks the cross-reference machinery.
fn pdf_xref(rng: &mut Rng, pdf: &mut Vec<u8>) -> &'static str {
    match rng.below(8) {
        0 => {
            if let Some(p) = rfind(pdf, b"startxref") {
                let s = p + 9;
                let e = s + pdf[s..]
                    .iter()
                    .take_while(|b| b.is_ascii_whitespace() || b.is_ascii_digit())
                    .count();
                let v = rng.pick(&[
                    "\n0\n",
                    "\n-1\n",
                    "\n99999999999\n",
                    "\n18446744073709551615\n",
                    "\n12\n",
                    "\n",
                ]);
                pdf.splice(s..e, v.bytes());
            }
            "pdf-startxref"
        }
        1 => {
            if let Some(p) = rfind(pdf, b"\nxref") {
                let e = rfind(pdf, b"trailer")
                    .filter(|&t| t > p)
                    .unwrap_or(pdf.len());
                pdf.drain(p + 1..e);
            }
            "pdf-no-xref"
        }
        2 => {
            if let Some(p) = rfind(pdf, b"\nxref\n").map(|p| p + 1) {
                let s = p + 5;
                let e = s + pdf[s..].iter().position(|&b| b == b'\n').unwrap_or(0);
                let v = rng.pick(&["0 999999999", "0 2147483647", "4294967295 2", "0 -5", "1 1"]);
                pdf.splice(s..e, v.bytes());
            }
            "pdf-xref-subsection"
        }
        3 => {
            if let Some(p) = rfind(pdf, b"\nxref\n").map(|p| p + 1) {
                // garble offsets of the table
                let mut i = p + 5;
                while i + 10 < pdf.len() {
                    if pdf[i].is_ascii_digit() && rng.chance(1, 8) {
                        pdf[i] = b'0' + u8::try_from(rng.below(10)).unwrap_or(0);
                    }
                    i += 1;
                    if pdf[i..].starts_with(b"trailer") {
                        break;
                    }
                }
            }
            "pdf-xref-offsets"
        }
        4 => {
            if let Some(p) = rfind(pdf, b"trailer") {
                pdf.drain(p..p + 7);
            }
            "pdf-no-trailer"
        }
        5 => {
            // a /Prev that loops back to the same table
            if let Some(x) = rfind(pdf, b"\nxref\n").map(|p| p + 1)
                && let Some(t) = rfind(pdf, b"<< /Size")
            {
                insert(pdf, t + 2, format!(" /Prev {x} /XRefStm {x}").as_bytes());
            }
            "pdf-prev-loop"
        }
        6 => {
            // an xref stream with absurd /W and /Index
            let n = pdfgen::max_object(pdf) + 1;
            let root = pdfgen::root_of(pdf).unwrap_or(1);
            let w = rng.pick(&["[1 2 1]", "[8 8 8]", "[0 0 0]", "[4294967295 1 1]", "[1 2]"]);
            let idx = rng.pick(&["[0 2147483647]", "[0 1 0 1 0 1]", "[]", "[5 -1]"]);
            let at = pdf.len();
            let body = stream(
                &[1, 0, 0, 0, 1, 0, 10, 0],
                format!("/Type /XRef /Size {n} /W {w} /Index {idx} /Root {root} 0 R").as_bytes(),
                true,
            );
            pdf.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            pdf.extend_from_slice(&body);
            pdf.extend_from_slice(format!("\nendobj\nstartxref\n{at}\n%%EOF\n").as_bytes());
            "pdf-xref-stream"
        }
        _ => {
            // /Length lies
            let starts: Vec<usize> = pdf
                .windows(7)
                .enumerate()
                .filter(|(_, w)| *w == b"/Length")
                .map(|(i, _)| i)
                .take(500)
                .collect();
            if let Some(&p) = starts.get(rng.below(starts.len())) {
                let s = p + 7;
                let e = s + pdf[s..]
                    .iter()
                    .take_while(|b| b.is_ascii_whitespace() || b.is_ascii_digit())
                    .count();
                let v = rng.pick(&[" 0 ", " 999999999 ", " -1 ", " 1 0 R ", " 4 0 R ", " 1e9 "]);
                pdf.splice(s..e, v.bytes());
            }
            "pdf-length-lie"
        }
    }
}

/// Mutates a PDF.
fn pdf(rng: &mut Rng, input: &[u8], other: &[u8]) -> (Vec<u8>, &'static str) {
    let mut v = input.to_vec();
    let (name, fix) = match rng.below(14) {
        0..=3 => match pdf_content(rng, &v) {
            Some(out) => {
                v = out;
                ("pdf-content", true)
            }
            None => (bytes(rng, &mut v, PDF_TOKENS, other), false),
        },
        4..=6 => {
            let (out, name) = pdf_append(rng, &v);
            v = out;
            (name, false)
        }
        7 => (pdf_xref(rng, &mut v), false),
        8 => {
            // swap a name
            let starts: Vec<usize> = v
                .iter()
                .enumerate()
                .filter(|&(_, &b)| b == b'/')
                .map(|(i, _)| i)
                .take(5000)
                .collect();
            if let Some(&s) = starts.get(rng.below(starts.len())) {
                let e = s
                    + 1
                    + v[s + 1..]
                        .iter()
                        .take_while(|b| b.is_ascii_alphanumeric())
                        .count();
                v.splice(s..e, rng.pick(PDF_NAMES).bytes());
            }
            ("pdf-name", rng.chance(1, 2))
        }
        9 => {
            // a filter chain on a stream dictionary
            if let Some((dict, _, _)) = pick_stream(rng, &v) {
                let n = rng.range(1, 6);
                let chain: Vec<&str> = (0..n).map(|_| *rng.pick(&PDF_NAMES[..9])).collect();
                let parms = rng.pick(&[
                    "",
                    " /DecodeParms << /Predictor 15 /Columns 2147483647 >>",
                    " /DecodeParms [null << /Predictor 2 /Colors 0 >>]",
                    " /DecodeParms << /EarlyChange 0 >>",
                ]);
                insert(
                    &mut v,
                    dict + 2,
                    format!(" /Filter [{}]{parms} ", chain.join(" ")).as_bytes(),
                );
            }
            ("pdf-filter-chain", true)
        }
        10 => {
            number(rng, &mut v);
            ("pdf-number", rng.chance(1, 2))
        }
        _ => (bytes(rng, &mut v, PDF_TOKENS, other), rng.chance(1, 3)),
    };
    if fix {
        v = pdfgen::fix_xref(&v);
    }
    cap(&mut v);
    (v, name)
}

// -- text ----------------------------------------------------------------------------------------

fn text(rng: &mut Rng, input: &[u8], other: &[u8]) -> (Vec<u8>, &'static str) {
    let mut v = input.to_vec();
    let name = match rng.below(6) {
        0 | 1 => {
            let t = hostile_text(rng);
            let at = rng.below(v.len() + 1);
            insert(&mut v, at, t.as_bytes());
            "text-hostile"
        }
        2 => {
            let t = rng.pick(TRICKY).as_bytes().to_vec();
            let k = rng.log_size(10_000);
            let at = rng.below(v.len() + 1);
            insert(&mut v, at, &t.repeat(k));
            "text-unicode"
        }
        3 => {
            let at = rng.below(v.len() + 1);
            let l = rng.pick(BUSINESS).as_bytes().to_vec();
            insert(&mut v, at, &l);
            "text-business"
        }
        _ => bytes(rng, &mut v, TRICKY, other),
    };
    (v, name)
}

/// One mutated input derived from `input` (and sometimes `other`, another corpus entry).
/// Returns the bytes and the name of the main mutation.
pub fn mutate(rng: &mut Rng, kind: Kind, input: &[u8], other: &[u8]) -> (Vec<u8>, &'static str) {
    // a quarter of the time a plain byte mutation, whatever the kind
    if rng.chance(1, 4) {
        let mut v = input.to_vec();
        let mut name = "bytes";
        for _ in 0..rng.range(1, 4) {
            name = bytes(rng, &mut v, &[], other);
        }
        return (v, name);
    }
    match kind {
        Kind::Docx => docx(rng, input).unwrap_or_else(|| {
            let mut v = input.to_vec();
            let n = bytes(rng, &mut v, OOXML, other);
            (v, n)
        }),
        Kind::Pptx => docx(rng, input).unwrap_or_else(|| {
            let mut v = input.to_vec();
            let n = bytes(rng, &mut v, PPTX, other);
            (v, n)
        }),
        Kind::Pdf => pdf(rng, input, other),
        Kind::Text => text(rng, input, other),
        Kind::Html => html(rng, input, other),
    }
}

// -- HTML ----------------------------------------------------------------------------------------

/// `PresentationML` snippets for the byte mutator of broken PPTX packages.
const PPTX: &[&str] = &[
    "<p:grpSp>",
    "</p:grpSp>",
    "<p:sp>",
    "<a:p>",
    "<a:t>",
    r#"<p:ph type="title"/>"#,
    r#"<a:off x="-9223372036854775808" y="9223372036854775807"/>"#,
    r#"<a:tc gridSpan="4294967295" rowSpan="0" hMerge="1">"#,
    r#"<p:sldId id="256" r:id="rId1"/>"#,
    r#"<p:cNvPr id="1" name="" hidden="1"/>"#,
];

/// HTML snippets inserted at random places.
const HTML: &[&str] = &[
    "<p>",
    "</p>",
    "<td>",
    "<tr>",
    "<table>",
    "</table>",
    "<li>",
    "<br>",
    "<hr style=\"page-break-after:always\">",
    "<div style=\"display:none\">",
    "<div hidden>",
    "</div>",
    "<script>",
    "</script>",
    "<style>",
    "<title>",
    "<textarea>",
    "<plaintext>",
    "<xmp>",
    "<!--",
    "-->",
    "<![CDATA[",
    "<?xml version='1.0' encoding='utf-16'?>",
    "<meta charset=\"windows-1251\">",
    "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=shift_jis\">",
    "<ix:nonNumeric name=\"dei:DocumentType\">",
    "</ix:nonNumeric>",
    "<ix:header>",
    "<ix:nonFraction name=\"x\"/>",
    "<td colspan=\"4294967295\" rowspan=\"65535\">",
    "<h1>",
    "<pre>",
    "<svg><style><p>",
    "<select><option>",
    "&amp;",
    "&#x110000;",
    "&#0;",
    "&CounterClockwiseContourIntegral;",
    "&notin",
    "\u{0}",
    "\r\n",
];

/// (open, close) pairs for nesting injection.
const HTML_NESTS: &[(&str, &str)] = &[
    ("<div>", "</div>"),
    ("<table><tr><td>", "</td></tr></table>"),
    ("<ul><li>", "</li></ul>"),
    ("<span style=\"display:none\">", "</span>"),
    ("<b>", ""),
    ("<p>", ""),
    ("<ix:nonNumeric name=\"dei:A\">", "</ix:nonNumeric>"),
];

fn html(rng: &mut Rng, input: &[u8], other: &[u8]) -> (Vec<u8>, &'static str) {
    let mut v = input.to_vec();
    let at = |rng: &mut Rng, v: &[u8]| rng.below(v.len() + 1);
    let name = match rng.below(10) {
        0 | 1 => {
            let k = rng.range(1, 8);
            for _ in 0..k {
                let p = at(rng, &v);
                let t = rng.pick(HTML).as_bytes().to_vec();
                insert(&mut v, p, &t);
            }
            "html-snippet"
        }
        2 | 3 => {
            let (open, close) = *rng.pick(HTML_NESTS);
            let n = *rng.pick(&[10usize, 100, 1000, 10_000, 100_000, 1_000_000]);
            let n = n.min(MAX_LEN / (open.len() + close.len()).max(1) / 2);
            let mut t = open.repeat(n);
            t.push_str(rng.pick(BUSINESS));
            if rng.chance(2, 3) {
                t.push_str(&close.repeat(n));
            }
            let p = at(rng, &v);
            insert(&mut v, p, t.as_bytes());
            "html-nest"
        }
        4 => {
            // a token that never ends: a comment, a tag, an attribute value
            let start = *rng.pick(&["<!--", "<p title=\"", "<p ", "<!DOCTYPE x \"", "<script>"]);
            let n = rng.log_size(MAX_LEN - start.len());
            let p = at(rng, &v);
            let mut t = start.as_bytes().to_vec();
            t.extend(std::iter::repeat_n(b'x', n));
            insert(&mut v, p, &t);
            "html-endless"
        }
        5 => {
            // a flood of one element or reference
            let t = *rng.pick(&[
                "<p>x",
                "<td>",
                "<tr>",
                "&amp;",
                "<br>",
                "<hr style=\"break-after:page\">x",
                "<a>",
            ]);
            let k = rng.log_size(MAX_LEN / t.len() / 2);
            let p = at(rng, &v);
            insert(&mut v, p, t.repeat(k).as_bytes());
            "html-flood"
        }
        6 => {
            // swap the declared charset
            let cs = *rng.pick(&[
                "utf-16le",
                "utf-8",
                "iso-2022-jp",
                "x-user-defined",
                "bogus",
                "gb18030",
                "windows-1252",
            ]);
            let d = String::from_utf8_lossy(&v)
                .replace("charset=", &format!("charset={cs};x="))
                .into_bytes();
            v = d;
            "html-charset"
        }
        7 => {
            // re-encode as UTF-16
            let t = String::from_utf8_lossy(&v).into_owned();
            let be = rng.chance(1, 2);
            let mut out = if rng.chance(1, 2) {
                vec![]
            } else if be {
                vec![0xFE, 0xFF]
            } else {
                vec![0xFF, 0xFE]
            };
            for u in t.encode_utf16() {
                out.extend_from_slice(&if be { u.to_be_bytes() } else { u.to_le_bytes() });
            }
            v = out;
            "html-utf16"
        }
        _ => bytes(rng, &mut v, HTML, other),
    };
    cap(&mut v);
    (v, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seeds;

    #[test]
    fn mutations_are_deterministic_and_bounded() {
        let all = seeds::all();
        let run = |seed| {
            let mut rng = Rng::new(seed, 0);
            let mut sizes = Vec::new();
            for i in 0..300 {
                let (k, b) = &all[i % all.len()];
                let (o, _) = mutate(&mut rng, *k, b, &all[(i + 1) % all.len()].1);
                assert!(o.len() <= MAX_LEN + 1024, "{}", o.len());
                sizes.push(o.len());
            }
            sizes
        };
        assert_eq!(run(1), run(1));
    }

    #[test]
    fn docx_mutations_stay_zips_mostly() {
        let docx = &seeds::docx_seeds()[0];
        let mut rng = Rng::new(3, 0);
        let mut readable = 0;
        for _ in 0..100 {
            if let Some((o, _)) = super::docx(&mut rng, docx)
                && zipfmt::read(&o, 1 << 10).is_some()
            {
                readable += 1;
            }
        }
        assert!(readable > 50, "{readable}");
    }
}

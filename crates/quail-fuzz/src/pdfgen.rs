//! Raw PDF writing: objects as bytes, an xref table rebuilt by scanning, Flate streams.
//! Hand-written instead of lopdf so the output can be broken in every way a parser must survive.

/// Flate (zlib) compression.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// Zlib inflate of at most `cap` output bytes; `None` when the stream is broken or larger.
pub fn unzlib(data: &[u8], cap: usize) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, cap).ok()
}

/// A stream object body: `<< extra /Length n >> stream ... endstream`, Flate-compressed when
/// `compress`.
pub fn stream(data: &[u8], extra: &[u8], compress: bool) -> Vec<u8> {
    let (body, filter): (Vec<u8>, &[u8]) = if compress {
        (zlib(data), b"/Filter /FlateDecode ")
    } else {
        (data.to_vec(), b"")
    };
    let mut o = Vec::with_capacity(body.len() + 64);
    o.extend_from_slice(b"<< ");
    o.extend_from_slice(filter);
    o.extend_from_slice(extra);
    o.extend_from_slice(format!(" /Length {} >>\nstream\n", body.len()).as_bytes());
    o.extend_from_slice(&body);
    o.extend_from_slice(b"\nendstream");
    o
}

/// A whole file: header, objects in order, then (when `xref`) an xref table and trailer.
pub fn build(objs: &[(u32, Vec<u8>)], root: u32, xref: bool) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    for (n, body) in objs {
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    if xref {
        append_xref(&mut out, root, None);
    } else {
        out.extend_from_slice(format!("trailer\n<< /Root {root} 0 R >>\n%%EOF\n").as_bytes());
    }
    out
}

/// Offsets of every `N G obj` in the file (last one wins per number), as the xref of a
/// repaired file would have them.
pub fn scan_objects(pdf: &[u8]) -> Vec<(u32, usize)> {
    let mut found = std::collections::BTreeMap::new();
    let mut i = 0;
    while let Some(p) = find(&pdf[i..], b" obj") {
        let at = i + p;
        // walk back over "G" and "N"
        let mut j = at;
        let mut nums = Vec::new();
        for _ in 0..2 {
            while j > 0 && pdf[j - 1] == b' ' {
                j -= 1;
            }
            let end = j;
            while j > 0 && pdf[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j == end {
                break;
            }
            nums.push(&pdf[j..end]);
        }
        if nums.len() == 2
            && (j == 0 || matches!(pdf[j - 1], b'\n' | b'\r' | b' '))
            && let Some(n) = std::str::from_utf8(nums[1])
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
            && n < 10_000_000
        {
            found.insert(n, j);
        }
        i = at + 4;
    }
    found.into_iter().collect()
}

/// Appends an xref table for every object found by [`scan_objects`] and a trailer with
/// `root` (and `/Prev` when given).
pub fn append_xref(out: &mut Vec<u8>, root: u32, prev: Option<usize>) {
    let objs = scan_objects(out);
    let size = objs.last().map_or(1, |(n, _)| n + 1);
    let x = out.len();
    let mut t = format!("xref\n0 {size}\n0000000000 65535 f \n");
    let mut it = objs.iter().peekable();
    for n in 1..size {
        if let Some((_, off)) = it.next_if(|(m, _)| *m == n) {
            t.push_str(&format!("{off:010} 00000 n \n"));
        } else {
            t.push_str("0000000000 65535 f \n");
        }
    }
    t.push_str(&format!("trailer\n<< /Size {size} /Root {root} 0 R"));
    if let Some(p) = prev {
        t.push_str(&format!(" /Prev {p}"));
    }
    t.push_str(&format!(" >>\nstartxref\n{x}\n%%EOF\n"));
    out.extend_from_slice(t.as_bytes());
}

/// The file with everything from its last `xref` / `startxref` on replaced by a fresh table:
/// offsets match again after a mutation moved objects.
pub fn fix_xref(pdf: &[u8]) -> Vec<u8> {
    let cut = rfind(pdf, b"\nxref")
        .or_else(|| rfind(pdf, b"trailer"))
        .or_else(|| rfind(pdf, b"startxref"))
        .unwrap_or(pdf.len());
    let mut out = pdf[..cut].to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let root = root_of(pdf).unwrap_or(1);
    append_xref(&mut out, root, None);
    out
}

/// The object number after `/Root` in the last trailer, if any.
pub fn root_of(pdf: &[u8]) -> Option<u32> {
    let p = rfind(pdf, b"/Root")?;
    let rest = &pdf[p + 5..];
    let s: String = rest
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_digit())
        .map(|&b| char::from(b))
        .collect();
    s.parse().ok()
}

/// Highest object number in the file.
pub fn max_object(pdf: &[u8]) -> u32 {
    scan_objects(pdf).last().map_or(0, |(n, _)| *n)
}

/// First position of `needle`.
pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Last position of `needle`.
pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// A one-page file around `content` with `resources` (inner of the dict) and extra objects
/// numbered from 10.
pub fn page_pdf(content: &[u8], resources: &[u8], extra: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
    let mut objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            [
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << ".as_slice(),
                resources,
                b" >> /Contents 4 0 R >>",
            ]
            .concat(),
        ),
        (4, stream(content, b"", true)),
    ];
    objs.extend(extra);
    build(&objs, 1, true)
}

/// Standard Helvetica font resource entry.
pub const FONT: &[u8] =
    b"/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xref_is_rebuilt() {
        let pdf = page_pdf(b"BT /F1 12 Tf 72 700 Td (hi) Tj ET", FONT, vec![]);
        let objs = scan_objects(&pdf);
        assert_eq!(objs.iter().map(|o| o.0).collect::<Vec<_>>(), [1, 2, 3, 4]);
        for (_, off) in &objs {
            assert!(pdf[*off..].starts_with(b"1 0 obj") || pdf[*off].is_ascii_digit());
        }
        let mut moved = b"%PDF-1.7\n% padding padding\n".to_vec();
        moved.extend_from_slice(&pdf[9..]);
        let fixed = fix_xref(&moved);
        let objs2 = scan_objects(&fixed);
        let x = rfind(&fixed, b"\nxref\n").map(|p| p + 1).unwrap();
        let table = std::str::from_utf8(&fixed[x..]).unwrap();
        assert!(table.contains(&format!("{:010} 00000 n", objs2[0].1)));
        assert_eq!(root_of(&fixed), Some(1));
        let s = stream(b"abc", b"", true);
        let body = &s[find(&s, b"stream\n").unwrap() + 7..s.len() - 10];
        assert_eq!(unzlib(body, 10).unwrap(), b"abc");
    }
}

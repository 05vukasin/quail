//! Robustness over a local corpus: every DOCX under `$QUAIL_TESTDATA` must extract, and
//! byte-mutated copies of it must never panic. Skipped when the variable is unset.

#![allow(clippy::cast_possible_truncation)]

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use quail_core::{Blob, Emitter, Extractor, Limits};
use quail_docx::DocxExtractor;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("docx"))
        {
            out.push(p);
        }
    }
}

/// The same package with every entry stored, so mutations land in the XML itself.
fn restore(zip: &[u8]) -> Option<Vec<u8>> {
    let mut a = zip::ZipArchive::new(Cursor::new(zip)).ok()?;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..a.len() {
        let mut f = a.by_index(i).ok()?;
        let name = f.name().ok()?.into_owned();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        w.start_file(name, opts).ok()?;
        w.write_all(&buf).ok()?;
    }
    Some(w.finish().ok()?.into_inner())
}

#[test]
fn corpus_extracts_and_mutations_never_panic() {
    let Some(root) = std::env::var_os("QUAIL_TESTDATA") else {
        eprintln!("QUAIL_TESTDATA unset: skipped");
        return;
    };
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    let rounds: usize = std::env::var("QUAIL_DOCX_MUTATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let limits = Limits::default();
    let (mut ok, mut failed, mut mutated) = (0usize, 0usize, 0usize);
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    for f in &files {
        let Ok(data) = std::fs::read(f) else {
            continue;
        };
        let mut out = Emitter::new(quail_docx::ID);
        if DocxExtractor
            .extract(&Blob::from(data.clone()), &limits, &mut out)
            .is_ok()
        {
            ok += 1;
        } else {
            failed += 1;
        }
        let Some(stored) = restore(&data) else {
            continue;
        };
        for _ in 0..rounds {
            let mut b = stored.clone();
            for _ in 0..8 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let i = (seed as usize) % b.len();
                b[i] = (seed >> 40) as u8;
            }
            let mut out = Emitter::new(quail_docx::ID);
            let _ = DocxExtractor.extract(&Blob::from(b), &limits, &mut out);
            mutated += 1;
        }
    }
    eprintln!(
        "files {} ok {ok} failed {failed} mutated runs {mutated}",
        files.len()
    );
    assert_eq!(failed, 0);
}

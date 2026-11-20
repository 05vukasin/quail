//! Kleister-NDA (Applica, <https://github.com/applicaai/kleister-nda>): 540 NDAs from EDGAR,
//! annotated with parties, effective date, jurisdiction and term.
//!
//! Layout read (the repository as published):
//!
//! ```text
//! <root>/documents/<file>.pdf
//! <root>/{train,dev-0,test-A}/in.tsv        first column: the PDF file name
//! <root>/{train,dev-0}/expected.tsv         one line per in.tsv line:
//!                                           `effective_date=2000-04-17 jurisdiction=New_York party=Acme_Corp term=2_years`
//! ```
//!
//! `in.tsv` ships xz-compressed (`in.tsv.xz`); decompress it first (`xz -dk in.tsv.xz`), this
//! crate has no xz decoder. Values write spaces as `_`. Every document is annotated for all
//! four keys, so a key missing from a line is recorded as `absent`. Splits without
//! `expected.tsv` (test-A) are skipped.
//!
//! Status: tested on generated samples only; not yet run on the real dataset.

use std::path::Path;

use super::{ConvertOptions, Converted, GoldAdapter, assign_split, gold_file};
use crate::gold::{GoldDoc, Label, MatchMode, Split};
use crate::{Error, Result};

/// The Kleister-NDA adapter (`kleister-nda`).
#[derive(Clone, Copy, Debug, Default)]
pub struct KleisterNda;

/// Kleister key → (task, match mode).
const KEYS: &[(&str, &str, MatchMode)] = &[
    ("party", "party", MatchMode::Normalized),
    ("effective_date", "effective_date", MatchMode::Normalized),
    ("jurisdiction", "jurisdiction", MatchMode::Normalized),
    ("term", "term", MatchMode::Normalized),
];

impl GoldAdapter for KleisterNda {
    fn name(&self) -> &'static str {
        "kleister-nda"
    }

    fn convert(&self, root: &Path, opts: &ConvertOptions) -> Result<Converted> {
        let mut out = Converted::default();
        for split in ["train", "dev-0", "test-A"] {
            let dir = root.join(split);
            let expected = dir.join("expected.tsv");
            if !expected.is_file() {
                continue;
            }
            let input = dir.join("in.tsv");
            if !input.is_file() {
                return Err(Error::Invalid(format!(
                    "{}: missing; decompress in.tsv.xz first (xz -dk)",
                    input.display()
                )));
            }
            let names: Vec<String> = crate::read_to_string(&input)?
                .lines()
                .map(|l| l.split('\t').next().unwrap_or("").trim().to_string())
                .collect();
            let lines: Vec<String> = crate::read_to_string(&expected)?
                .lines()
                .map(str::to_string)
                .collect();
            if names.len() != lines.len() {
                return Err(Error::parse(
                    &expected,
                    format!("{} lines, but in.tsv has {}", lines.len(), names.len()),
                ));
            }
            for (name, line) in names.iter().zip(&lines) {
                out.bump("documents");
                let pdf = root.join("documents").join(name);
                let file = if pdf.is_file() {
                    Some(pdf.as_path())
                } else {
                    out.bump("file_not_found");
                    None
                };
                let mut doc = GoldDoc {
                    file: gold_file(root, file, &format!("documents/{name}")),
                    dataset: self.name().into(),
                    split: Split::Dev,
                    source: Some(format!("kleister-nda/{split}")),
                    labels: vec![],
                    absent: vec![],
                };
                for (k, v) in parse_expected(line) {
                    match KEYS.iter().find(|(key, _, _)| *key == k) {
                        Some((_, task, mode)) => {
                            doc.labels.push(Label::text(*task, v, *mode));
                            out.bump("labels");
                        }
                        None => out.bump("unknown_key"),
                    }
                }
                for (_, task, _) in KEYS {
                    if !doc.labels.iter().any(|l| l.task == *task) {
                        doc.absent.push((*task).to_string());
                    }
                }
                assign_split(&mut doc, opts);
                out.docs.push(doc);
            }
        }
        if out.docs.is_empty() {
            return Err(Error::Invalid(format!(
                "{}: no <split>/expected.tsv found (is this the kleister-nda repository?)",
                root.display()
            )));
        }
        Ok(out)
    }
}

/// `key=value` pairs of one expected.tsv line, `_` turned back into spaces, duplicates
/// removed.
pub fn parse_expected(line: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for tok in line.split_whitespace() {
        let Some((k, v)) = tok.split_once('=') else {
            continue;
        };
        let v = v.replace('_', " ").trim().to_string();
        if v.is_empty() {
            continue;
        }
        let pair = (k.to_string(), v);
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        assert_eq!(
            parse_expected(
                "effective_date=2001-02-03 party=Alpha_Corp party=Beta_LLC party=Alpha_Corp term=2_years"
            ),
            vec![
                ("effective_date".into(), "2001-02-03".into()),
                ("party".into(), "Alpha Corp".into()),
                ("party".into(), "Beta LLC".into()),
                ("term".into(), "2 years".into()),
            ]
        );
    }

    #[test]
    fn converts_a_generated_sample() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        std::fs::create_dir_all(r.join("documents")).unwrap();
        std::fs::create_dir_all(r.join("train")).unwrap();
        std::fs::create_dir_all(r.join("test-A")).unwrap();
        std::fs::write(r.join("documents/a.pdf"), b"%PDF-1.4 synthetic a").unwrap();
        std::fs::write(
            r.join("train/in.tsv"),
            "a.pdf\tkeys\ttext\nb.pdf\tkeys\ttext\n",
        )
        .unwrap();
        std::fs::write(
            r.join("train/expected.tsv"),
            "effective_date=2001-02-03 jurisdiction=New_York party=Alpha_Corp party=Beta_LLC term=2_years\nparty=Gamma_Ltd\n",
        )
        .unwrap();
        std::fs::write(r.join("test-A/in.tsv"), "c.pdf\tkeys\ttext\n").unwrap();
        let c = KleisterNda.convert(r, &ConvertOptions::default()).unwrap();
        assert_eq!(c.docs.len(), 2);
        let a = &c.docs[0];
        assert_eq!(
            a.file.blake3.as_deref(),
            Some(crate::blake3_hex(b"%PDF-1.4 synthetic a").as_str())
        );
        assert_eq!(a.labels.len(), 5);
        assert!(a.absent.is_empty());
        let b = &c.docs[1];
        assert_eq!(b.file.blake3, None);
        assert_eq!(b.file.locator_hint.as_deref(), Some("documents/b.pdf"));
        assert_eq!(b.absent, vec!["effective_date", "jurisdiction", "term"]);
        assert_eq!(c.stats["file_not_found"], 1);
        for d in &c.docs {
            d.validate().unwrap();
        }
    }

    #[test]
    fn requires_decompressed_input() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("dev-0")).unwrap();
        std::fs::write(root.path().join("dev-0/expected.tsv"), "party=X\n").unwrap();
        let e = KleisterNda
            .convert(root.path(), &ConvertOptions::default())
            .unwrap_err();
        assert!(e.to_string().contains("xz -dk"));
    }
}

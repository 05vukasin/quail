//! CUAD v1 (The Atticus Project, <https://www.atticusprojectai.org/cuad>): 510 commercial
//! contracts from EDGAR with 41 clause categories annotated as answer spans.
//!
//! Layout read:
//!
//! ```text
//! <root>/**/CUAD_v1.json            SQuAD 2.0 form: data[].title, data[].paragraphs[].qas[]
//!                                   qas[].id = "<title>__<Category>", answers[].text, is_impossible
//! <root>/**/<title>.pdf | .txt      the contract (full_contract_pdf/, full_contract_txt/)
//! ```
//!
//! Categories used (task): `Document Name` (`doc_title`, contains), `Parties` (`party`,
//! contains: CUAD party spans include defined-term aliases such as "Distributor", so
//! precision against them is low by construction; recall is the meaningful number),
//! `Agreement Date`, `Effective Date`, `Expiration Date` (`agreement_date`,
//! `effective_date`, `expiration_date`: answers that parse as a date become ISO labels;
//! several distinct dates become one `any_of` label; answers that are not a date, such as a
//! renewal clause, are skipped and counted). A category with no answer is `absent`.
//! `master_clauses.csv` carries the same spans and is not needed.
//!
//! Status: run on the real dataset (510 documents, one without a contract file).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{ConvertOptions, Converted, GoldAdapter, assign_split, gold_file, walk};
use crate::gold::{GoldDoc, Label, MatchMode, Split};
use crate::normalize::{parse_date, text};
use crate::{Error, Result};

/// The CUAD adapter (`cuad`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Cuad;

#[derive(Deserialize)]
struct Squad {
    data: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    title: String,
    #[serde(default)]
    paragraphs: Vec<Paragraph>,
}

#[derive(Deserialize)]
struct Paragraph {
    #[serde(default)]
    qas: Vec<Qa>,
}

#[derive(Deserialize)]
struct Qa {
    id: String,
    #[serde(default)]
    answers: Vec<Answer>,
    #[serde(default)]
    is_impossible: bool,
}

#[derive(Deserialize)]
struct Answer {
    text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Date,
}

const CATEGORIES: &[(&str, &str, Kind)] = &[
    ("Document Name", "doc_title", Kind::Text),
    ("Parties", "party", Kind::Text),
    ("Agreement Date", "agreement_date", Kind::Date),
    ("Effective Date", "effective_date", Kind::Date),
    ("Expiration Date", "expiration_date", Kind::Date),
];

fn find_json(root: &Path, files: &[PathBuf]) -> Result<PathBuf> {
    files
        .iter()
        .find(|p| p.file_name().is_some_and(|n| n == "CUAD_v1.json"))
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("{}: no CUAD_v1.json found", root.display())))
}

/// Contract files by lowercase stem; PDF preferred over text.
fn contracts_by_stem(files: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut by_stem: HashMap<String, PathBuf> = HashMap::new();
    for p in files {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        let rank = match ext.as_deref() {
            Some("pdf") => 0,
            Some("txt") => 1,
            _ => continue,
        };
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let key = stem.trim().to_lowercase();
        let better = by_stem.get(&key).is_none_or(|old| {
            let old_pdf = old
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            rank == 0 && !old_pdf
        });
        if better {
            by_stem.insert(key, p.clone());
        }
    }
    by_stem
}

impl GoldAdapter for Cuad {
    fn name(&self) -> &'static str {
        "cuad"
    }

    fn convert(&self, root: &Path, opts: &ConvertOptions) -> Result<Converted> {
        let files = walk(root)?;
        let json = find_json(root, &files)?;
        let squad: Squad = serde_json::from_str(&crate::read_to_string(&json)?)
            .map_err(|e| Error::parse(&json, e))?;

        let by_stem = contracts_by_stem(&files);

        let mut out = Converted::default();
        for entry in &squad.data {
            out.bump("documents");
            let file = by_stem.get(&entry.title.trim().to_lowercase());
            if file.is_none() {
                out.bump("file_not_found");
            }
            let mut doc = GoldDoc {
                file: gold_file(
                    root,
                    file.map(PathBuf::as_path),
                    &format!("{}.pdf", entry.title),
                ),
                dataset: self.name().into(),
                split: Split::Dev,
                source: Some("cuad/v1".into()),
                labels: vec![],
                absent: vec![],
            };
            for (cat, task, kind) in CATEGORIES {
                let answers: Vec<&str> = entry
                    .paragraphs
                    .iter()
                    .flat_map(|p| &p.qas)
                    .filter(|q| q.id.rsplit("__").next() == Some(cat))
                    .filter(|q| !q.is_impossible)
                    .flat_map(|q| q.answers.iter().map(|a| a.text.trim()))
                    .filter(|t| !t.is_empty())
                    .collect();
                if answers.is_empty() {
                    doc.absent.push((*task).to_string());
                    continue;
                }
                match kind {
                    Kind::Text => {
                        let mut seen = Vec::new();
                        for a in answers {
                            let n = text(a);
                            if n.is_empty() || seen.contains(&n) {
                                continue;
                            }
                            seen.push(n);
                            doc.labels.push(Label::text(*task, a, MatchMode::Contains));
                            out.bump("labels");
                        }
                    }
                    Kind::Date => {
                        let mut dates: Vec<String> = Vec::new();
                        for a in &answers {
                            match parse_date(a) {
                                Some(d) if !dates.contains(&d) => dates.push(d),
                                Some(_) => {}
                                None => out.bump("date_not_parsed"),
                            }
                        }
                        match dates.len() {
                            0 => {}
                            1 => {
                                doc.labels.push(Label::text(
                                    *task,
                                    dates.remove(0),
                                    MatchMode::Normalized,
                                ));
                                out.bump("labels");
                            }
                            _ => {
                                let mut l = Label::text(*task, "", MatchMode::AnyOf);
                                l.value = serde_json::json!(dates);
                                l.note = Some("several dates annotated".into());
                                doc.labels.push(l);
                                out.bump("labels");
                            }
                        }
                    }
                }
            }
            assign_split(&mut doc, opts);
            out.docs.push(doc);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qa(title: &str, cat: &str, answers: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "id": format!("{title}__{cat}"),
            "question": "synthetic",
            "is_impossible": answers.is_empty(),
            "answers": answers.iter().map(|a| serde_json::json!({"text": a, "answer_start": 0})).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn converts_a_generated_sample() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        std::fs::create_dir_all(r.join("CUAD_v1/full_contract_pdf/Part_I")).unwrap();
        std::fs::create_dir_all(r.join("CUAD_v1/full_contract_txt")).unwrap();
        std::fs::write(
            r.join("CUAD_v1/full_contract_pdf/Part_I/SYNTH_CO_1-EX-10.pdf"),
            b"%PDF synthetic",
        )
        .unwrap();
        std::fs::write(
            r.join("CUAD_v1/full_contract_txt/SYNTH_CO_1-EX-10.txt"),
            b"synthetic",
        )
        .unwrap();
        let t1 = "SYNTH_CO_1-EX-10";
        let t2 = "MISSING_2-EX-99";
        let json = serde_json::json!({ "version": "v1", "data": [
            { "title": t1, "paragraphs": [ { "context": "synthetic", "qas": [
                qa(t1, "Document Name", &["SUPPLY AGREEMENT"]),
                qa(t1, "Parties", &["Alpha Corp", "Beta LLC", "ALPHA CORP", "Supplier"]),
                qa(t1, "Agreement Date", &["February 3, 2001"]),
                qa(t1, "Effective Date", &["the date first written above"]),
                qa(t1, "Expiration Date", &["3/1/2002", "1 March 2003"]),
                qa(t1, "Governing Law", &["New York"]),
            ] } ] },
            { "title": t2, "paragraphs": [ { "context": "synthetic", "qas": [
                qa(t2, "Parties", &[]),
            ] } ] },
        ]});
        std::fs::write(r.join("CUAD_v1/CUAD_v1.json"), json.to_string()).unwrap();

        let c = Cuad.convert(r, &ConvertOptions::default()).unwrap();
        assert_eq!(c.docs.len(), 2);
        let d = &c.docs[0];
        d.validate().unwrap();
        assert_eq!(
            d.file.blake3.as_deref(),
            Some(crate::blake3_hex(b"%PDF synthetic").as_str())
        );
        let parties: Vec<String> = d.labels_for("party").map(Label::display).collect();
        assert_eq!(parties, vec!["Alpha Corp", "Beta LLC", "Supplier"]);
        assert_eq!(
            d.labels_for("agreement_date").next().unwrap().display(),
            "2001-02-03"
        );
        // Unparsable effective date: neither labelled nor absent.
        assert!(d.labels_for("effective_date").next().is_none());
        assert!(!d.is_absent("effective_date"));
        let exp = d.labels_for("expiration_date").next().unwrap();
        assert_eq!(exp.mode, MatchMode::AnyOf);
        assert_eq!(exp.alternatives(), vec!["2002-03-01", "2003-03-01"]);
        assert_eq!(c.stats["date_not_parsed"], 1);
        let m = &c.docs[1];
        m.validate().unwrap();
        assert_eq!(m.file.blake3, None);
        assert!(m.is_absent("party"));
        assert_eq!(c.stats["file_not_found"], 1);
    }
}

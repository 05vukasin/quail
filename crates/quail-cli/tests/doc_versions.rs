//! Docs drift: every step id with a version (`<ns>.<name>@<N>`) named in README.md,
//! docs/*.md and crates/*/README.md is the version the code has. Registered steps are
//! checked against `quail version --json`; steps outside the default registry against the
//! `AnalyzerId::new` of the file that declares them; older versions named on purpose
//! (version histories, numbers measured on a past version) are listed in `HISTORY`.

#![cfg(all(
    feature = "docx",
    feature = "pdf",
    feature = "text",
    feature = "signals",
    feature = "refs",
    feature = "parties",
    feature = "clauses",
    feature = "sketch",
    feature = "items",
    feature = "summary"
))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

/// Steps the default build does not register, with the file whose `AnalyzerId::new` has
/// their version: feature-gated (`ocr`, `llm`), off by default, or test-only.
const DECLARED: &[(&str, &str)] = &[
    ("ocr.pages", "crates/quail-ocr/src/analyzer.rs"),
    ("summary.llm", "crates/quail-summary/src/llm.rs"),
    ("summary.digest", "crates/quail-summary/src/analyzer.rs"),
    ("engine.dedup", "crates/quail-engine/src/dedup.rs"),
    ("fuzz.text", "crates/quail-fuzz/src/target.rs"),
];

/// `(file, id)`: an older version named on purpose.
const HISTORY: &[(&str, &str)] = &[
    ("crates/quail-items/README.md", "item.lines@1"),
    ("crates/quail-items/README.md", "item.lines@2"),
    ("crates/quail-items/README.md", "item.lines@3"),
    ("crates/quail-items/README.md", "item.lines@4"),
    ("crates/quail-items/README.md", "item.lines@5"),
    ("docs/performance.md", "item.lines@5"),
    ("docs/performance.md", "item.lines@4"),
    ("crates/quail-parties/README.md", "party.candidates@2"),
    ("crates/quail-parties/README.md", "party.candidates@3"),
    ("crates/quail-parties/README.md", "party.candidates@4"),
    ("crates/quail-parties/README.md", "party.candidates@5"),
    ("docs/performance.md", "party.candidates@4"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `name → version` of every step `quail version --json` lists.
fn registered() -> BTreeMap<String, u32> {
    let o = Command::new(env!("CARGO_BIN_EXE_quail"))
        .args(["version", "--json"])
        .output()
        .unwrap();
    assert!(o.status.success());
    let v: Json = serde_json::from_slice(&o.stdout).unwrap();
    v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| split(s["id"].as_str().unwrap()).unwrap())
        .collect()
}

/// `ns.name@N` → `(ns.name, N)`.
fn split(id: &str) -> Option<(String, u32)> {
    let (name, v) = id.rsplit_once('@')?;
    Some((name.to_owned(), v.parse().ok()?))
}

/// The version in `AnalyzerId::new("<name>", N)` in `file`.
fn declared(name: &str, file: &str) -> u32 {
    let src = fs::read_to_string(root().join(file)).unwrap();
    let pat = format!("AnalyzerId::new(\"{name}\",");
    let at = src
        .find(&pat)
        .unwrap_or_else(|| panic!("{file} does not declare {name}"));
    let rest = &src[at + pat.len()..];
    let end = rest.find(')').unwrap();
    rest[..end].trim().parse().unwrap()
}

/// Every `ns.name@N` in `text`: lowercase ASCII words joined by one dot, then `@` digits.
fn mentions(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_';
    let mut out = Vec::new();
    for (at, _) in text.match_indices('@') {
        let digits = b[at + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digits == 0 {
            continue;
        }
        let mut start = at;
        while start > 0 && (word(b[start - 1]) || b[start - 1] == b'.') {
            start -= 1;
        }
        let name = &text[start..at];
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() == 2
            && parts
                .iter()
                .all(|p| p.as_bytes().first().is_some_and(u8::is_ascii_lowercase))
        {
            out.push(text[start..at + 1 + digits].to_owned());
        }
    }
    out
}

fn doc_files() -> Vec<(String, PathBuf)> {
    let root = root();
    let mut files = vec![("README.md".to_owned(), root.join("README.md"))];
    for dir in ["docs", "crates"] {
        let mut entries: Vec<_> = fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            let (rel, file) = if dir == "docs" {
                if p.extension().is_none_or(|e| e != "md") {
                    continue;
                }
                let n = p.file_name().unwrap().to_string_lossy().into_owned();
                (format!("docs/{n}"), p)
            } else {
                let f = p.join("README.md");
                if !f.is_file() {
                    continue;
                }
                let n = p.file_name().unwrap().to_string_lossy().into_owned();
                (format!("crates/{n}/README.md"), f)
            };
            files.push((rel, file));
        }
    }
    files
}

#[test]
fn step_versions_in_the_docs_match_the_code() {
    let mut want = registered();
    for (name, file) in DECLARED {
        assert!(
            !want.contains_key(*name),
            "{name} is registered now: drop it from DECLARED"
        );
        want.insert((*name).to_owned(), declared(name, file));
    }
    let mut used_history = BTreeSet::new();
    let mut wrong = Vec::new();
    let files = doc_files();
    assert!(files.len() > 10, "{files:?}");
    for (rel, path) in &files {
        let text = fs::read_to_string(path).unwrap();
        for id in mentions(&text) {
            let Some((name, v)) = split(&id) else {
                continue;
            };
            if HISTORY.contains(&(rel.as_str(), id.as_str())) {
                used_history.insert((rel.clone(), id.clone()));
                continue;
            }
            match want.get(&name) {
                Some(&have) if have == v => {}
                Some(&have) => wrong.push(format!("{rel}: {id}, the code has {name}@{have}")),
                None => wrong.push(format!(
                    "{rel}: {id} is not a registered step nor in DECLARED"
                )),
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "step versions out of date:\n{}",
        wrong.join("\n")
    );
    for (file, id) in HISTORY {
        assert!(
            used_history.contains(&((*file).to_owned(), (*id).to_owned())),
            "HISTORY entry {file}: {id} is no longer mentioned: drop it"
        );
    }
}

#[test]
fn mentions_are_found() {
    let m = mentions("`signal.scan@5` and src item.lines@12, not a@b, me@4 or x.y.z@2.");
    assert_eq!(m, ["signal.scan@5", "item.lines@12"]);
}

//! Every key `src/input.rs` reads is declared by its owner crate, so a renamed key breaks
//! this test instead of silently emptying a card field.

use std::path::Path;

fn owner(namespace: &str) -> Option<&'static str> {
    Some(match namespace {
        "engine" => "quail-engine",
        "text" => "quail-text",
        "ref" => "quail-refs",
        "party" => "quail-parties",
        "signal" => "quail-signals",
        "item" => "quail-items",
        "sketch" => "quail-sketch",
        "ocr" => "quail-ocr",
        "pdf" => "quail-pdf",
        "docx" => "quail-docx",
        _ => return None,
    })
}

fn sources(dir: &Path, out: &mut String) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
}

#[test]
fn input_keys_exist_in_their_crates() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = std::fs::read_to_string(here.join("src/input.rs")).unwrap();
    let mut checked = 0;
    for (i, part) in input.split('"').enumerate() {
        // Odd parts are inside quotes; keys are `namespace.name`.
        if i % 2 == 0 || part.contains(' ') || !part.contains('.') {
            continue;
        }
        let ns = part.split('.').next().unwrap_or("");
        let Some(krate) = owner(ns) else { continue };
        let dir = here.join("..").join(krate).join("src");
        if !dir.exists() {
            eprintln!("skipping {part}: {krate} is not in this checkout");
            continue;
        }
        let mut src = String::new();
        sources(&dir, &mut src);
        assert!(
            src.contains(&format!("FactKey::new(\"{part}\")")),
            "{part} is not declared in {krate}"
        );
        checked += 1;
    }
    assert!(checked > 50, "only {checked} keys checked");
}

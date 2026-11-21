//! The backtracking patterns of the Python-compatible sketches (`fancy-regex`, bounded by
//! `compat::BACKTRACK_LIMIT`) stay fast on 1 MB of hostile text, and a pattern that gives up
//! degrades to a warning, never a panic or a hang.

mod common;

use std::time::{Duration, Instant};

use common::text_doc;
use quail_core::{Analyzer, Artifacts, Blob, Ctx, Emitter, Format, Limits, Profile};
use quail_sketch::{CompatMinHash, InvoiceLayout, compat, invoice};

const MB: usize = 1 << 20;
/// Wall-time bound per input for the whole compatibility path, in a debug build. Locally it takes
/// about 1 s; shared CI runners (macOS especially) are several times slower. Exponential
/// backtracking on these 1 MB inputs would take minutes, so 10 s still catches it.
const BOUND: Duration = Duration::from_secs(10);

fn repeat(unit: &str) -> String {
    unit.repeat(MB / unit.len() + 1)
}

fn inputs() -> Vec<(&'static str, String)> {
    vec![
        ("aa-aa-", repeat("aa-")),
        ("aa/aa/", repeat("aa/")),
        ("1.1.1.", repeat("1.")),
        ("1,1,1,", repeat("1,")),
        ("1/1/1/", repeat("1/")),
        ("1-12-", repeat("1-12-")),
        ("word chars, no end", "a".repeat(MB)),
        ("digits, no end", "7".repeat(MB)),
        ("alnum run", repeat("ab12")),
        ("a@a@", repeat("a@")),
        ("a.a@a.", repeat("a.a@a.")),
        ("Capitalized words, no legal form", repeat("Abc ")),
        ("Capitalized with commas", repeat("Abc, ")),
        ("12 march 12 march", repeat("12 march ")),
        ("1 march 1", repeat("1 march 1")),
        ("Cyrillic and dotted i", repeat("Ђİıa-1 ")),
        ("İı runs", repeat("İıİ) ")),
        ("combining marks", repeat("1\u{345}a\u{301}-")),
    ]
}

fn run_analyzer(a: &dyn Analyzer, text: &str) -> (usize, Vec<String>) {
    let blob = Blob::new(Vec::<u8>::new());
    let mut arts = Artifacts::new();
    arts.insert(text_doc(text));
    let limits = Limits::default();
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "generated",
        artifacts: &arts,
        facts: &[],
        limits: &limits,
        profile: Profile::Deep,
    };
    let mut out = Emitter::new(a.id());
    a.run(&ctx, &mut out).unwrap();
    let (facts, _, warnings) = out.into_parts();
    (facts.len(), warnings)
}

#[test]
fn compat_path_is_bounded_on_hostile_text() {
    let mut slowest = (Duration::ZERO, "");
    for (name, text) in inputs() {
        // Every pattern, directly: errors are allowed, panics and hangs are not.
        let t = Instant::now();
        let _ = compat::mask_entities(&text);
        let _ = invoice::number_patterns(&text);
        let _ = compat::structural_fp([text.as_str()]);
        let patterns = t.elapsed();
        assert!(patterns < BOUND, "{name}: patterns alone {patterns:?}");
        // The whole compatibility path, as the analyzers run it; a pattern that gave up is a warning.
        let t = Instant::now();
        let (_, w1) = run_analyzer(&CompatMinHash, &text);
        let (_, w2) = run_analyzer(&InvoiceLayout, &text);
        for w in w1.iter().chain(&w2) {
            assert!(
                w.contains("backtracking limit"),
                "{name}: unexpected warning {w}"
            );
        }
        let took = t.elapsed();
        println!("{name}: patterns {patterns:?}, analyzers {took:?}");
        assert!(took < BOUND, "{name}: {took:?}");
        if took > slowest.0 {
            slowest = (took, name);
        }
    }
    println!("slowest: {:?} on {}", slowest.0, slowest.1);
}

#[test]
fn limit_error_degrades_to_a_warning() {
    // An unterminated word run after a label: the invoice-number pattern backtracks through
    // the whole run. Either it finishes (the limit scales with the text) or it gives up, and
    // then the analyzer warns and emits nothing.
    let text = format!("invoice no {}", "a".repeat(MB));
    let t = Instant::now();
    let (facts, warnings) = run_analyzer(&InvoiceLayout, &text);
    assert!(t.elapsed() < BOUND);
    match invoice::features(&text) {
        Err(compat::RegexLimit) => {
            assert_eq!(facts, 0);
            assert!(warnings[0].contains("backtracking limit"));
        }
        Ok(f) => {
            assert_eq!(f.labels, ["invoice no"]);
            assert_eq!(facts, 3);
            assert!(warnings.is_empty());
        }
    }
}

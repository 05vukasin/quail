//! End-to-end tests on documents built in code. Every name here is invented.

use std::time::{Duration, Instant};

use quail_clauses::{ClauseTerms, keys};
use quail_core::{
    Analyzer, Artifacts, Blob, Ctx, Document, Emitter, Fact, FactKey, Format, Limits, Page,
    PageKind, Profile, Value,
};

fn doc(pages: &[&str]) -> Document {
    Document {
        pages: pages
            .iter()
            .enumerate()
            .map(|(i, t)| Page {
                text: (*t).to_owned(),
                ..Page::new(u32::try_from(i).unwrap(), PageKind::Physical)
            })
            .collect(),
    }
}

fn run_with(d: Document, limits: &Limits, deadline: Option<Instant>) -> (Vec<Fact>, Vec<String>) {
    let mut artifacts = Artifacts::new();
    artifacts.insert(d);
    let blob = Blob::from(Vec::new());
    let ctx = Ctx {
        blob: &blob,
        format: Format::Pdf,
        locator: "mem://test",
        artifacts: &artifacts,
        facts: &[],
        limits,
        profile: Profile::Standard,
    };
    let mut out = match deadline {
        Some(t) => Emitter::with_deadline(ClauseTerms::ID, t),
        None => Emitter::new(ClauseTerms::ID),
    };
    ClauseTerms.run(&ctx, &mut out).unwrap();
    let problems = out.take_problems();
    (out.facts().to_vec(), problems)
}

fn run(pages: &[&str]) -> Vec<Fact> {
    run_with(doc(pages), &Limits::default(), None).0
}

fn field<'a>(f: &'a Fact, name: &str) -> &'a str {
    f.value.field(name).and_then(Value::as_text).unwrap_or("")
}

fn of<'a>(facts: &'a [Fact], key: &FactKey) -> Vec<&'a Fact> {
    facts.iter().filter(|f| &f.key == key).collect()
}

const NDA: &str = "MUTUAL NONDISCLOSURE AGREEMENT\n\
This Agreement is made between Orvane Sample, Inc. and Pelko Example LLC.\n\
1. Term. This Agreement shall remain in effect for a period of two (2) years from the\n\
Effective Date and shall be automatically renewed for successive one-year periods.\n\
2. Either party may terminate this Agreement for convenience upon thirty (30) days' prior\n\
written notice, and immediately upon a material breach by the other party.\n\
3. The Recipient's obligations of confidentiality shall survive for five (5) years.\n\
4. Each party agrees that, for a period of one (1) year, it will not solicit the other's employees.";

const LAW: &str = "9. Governing Law. This Agreement shall be governed by and construed in\n\
accordance with the internal laws of the State of New York, without regard to its conflict\n\
of laws rules. The parties submit to the exclusive jurisdiction of the courts of the State of\n\
Delaware. Pelko Example LLC is a limited liability company organized under the laws of the\n\
State of Texas.";

#[test]
fn governing_law_beats_forum_and_incorporation() {
    let f = run(&[NDA, LAW]);
    let laws = of(&f, &keys::GOVERNING_LAW);
    assert_eq!(field(laws[0], "jurisdiction"), "New York", "{laws:#?}");
    assert_eq!(field(laws[0], "code"), "US-NY");
    assert_eq!(field(laws[0], "basis"), "laws_of");
    assert!(field(laws[0], "raw").starts_with("laws of the State of New York"));
    assert!(laws[0].conf > 0.9);
    let ev = laws[0].evidence.clone().unwrap();
    assert_eq!(ev.page, Some(1));
    assert_eq!(ev.span.unwrap().slice(LAW), "laws of the State of New York");
    for other in &laws[1..] {
        assert!(other.conf < 0.4, "{other:#?}");
    }
}

#[test]
fn term_renewal_survival_notice_and_termination() {
    let f = run(&[NDA, LAW]);
    let terms = of(&f, &keys::TERM);
    assert_eq!(terms.len(), 1, "{terms:#?}");
    assert_eq!(field(terms[0], "text"), "2 years");
    assert_eq!(field(terms[0], "duration"), "P2Y");
    assert_eq!(field(terms[0], "renewal"), "auto");
    assert!(terms[0].conf > 0.85);
    let survival = of(&f, &keys::SURVIVAL);
    assert_eq!(field(survival[0], "text"), "5 years");
    let notice = of(&f, &keys::NOTICE_PERIOD);
    assert_eq!(field(notice[0], "text"), "30 days");
    let kinds: Vec<&str> = of(&f, &keys::TERMINATION)
        .iter()
        .map(|t| field(t, "kind"))
        .collect();
    assert!(
        kinds.contains(&"convenience") && kinds.contains(&"breach"),
        "{kinds:?}"
    );
    // The non-solicitation period is no term.
    assert!(f.iter().all(|x| field(x, "text") != "1 year"), "{f:#?}");
}

#[test]
fn nothing_on_ordinary_text_and_no_panics_on_junk() {
    let f = run(&[
        "Invoice 2024-001. Payment is due within thirty (30) days. He has 20 years of experience.",
    ]);
    assert!(
        of(&f, &keys::TERM).is_empty() && of(&f, &keys::GOVERNING_LAW).is_empty(),
        "{f:#?}"
    );
    let junk: String =
        "laws of the State of ( two ( years governed law \u{0}\u{feff}é".repeat(2000);
    let t = Instant::now();
    let _ = run(&[&junk, "", "of"]);
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn limits_and_deadline_make_problems() {
    let limits = Limits {
        max_text_bytes: 40,
        ..Limits::default()
    };
    let (_, problems) = run_with(doc(&[NDA, LAW]), &limits, None);
    assert!(
        problems.iter().any(|p| p.contains("text limit")),
        "{problems:?}"
    );
    let (facts, problems) = run_with(doc(&[NDA, LAW]), &Limits::default(), Some(Instant::now()));
    assert!(facts.is_empty());
    assert!(
        problems.iter().any(|p| p.contains("deadline")),
        "{problems:?}"
    );
}

#[test]
fn pages_that_need_ocr_are_skipped() {
    let mut d = doc(&[LAW]);
    d.pages[0].needs_ocr = true;
    let (facts, problems) = run_with(d, &Limits::default(), None);
    assert!(facts.is_empty() && problems.is_empty());
}

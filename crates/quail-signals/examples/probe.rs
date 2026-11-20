//! Runs `signal.scan` over plain-text files (pages split on form feeds) and prints each fact
//! with the text it points at: `cargo run -p quail-signals --example probe -- a.txt b.txt`.

use quail_core::{
    Analyzer, Artifacts, Blob, Ctx, Document, Emitter, Format, Limits, Page, PageKind, Profile,
    TextSource,
};
use quail_signals::SignalScan;

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("{path}: unreadable");
            continue;
        };
        let pages = text
            .split('\u{c}')
            .enumerate()
            .map(|(i, t)| Page {
                text: t.to_owned(),
                text_source: TextSource::Native,
                ..Page::new(u32::try_from(i).unwrap_or(u32::MAX), PageKind::Physical)
            })
            .collect();
        let doc = Document { pages };
        let mut artifacts = Artifacts::new();
        let blob = Blob::from(Vec::new());
        let limits = Limits::default();
        let ctx_doc = doc.clone();
        artifacts.insert(ctx_doc);
        let ctx = Ctx {
            blob: &blob,
            format: Format::Text,
            locator: &path,
            artifacts: &artifacts,
            facts: &[],
            limits: &limits,
            profile: Profile::Fast,
        };
        let mut out = Emitter::new(SignalScan::ID);
        if let Err(e) = SignalScan.run(&ctx, &mut out) {
            eprintln!("{path}: {e}");
        }
        for f in out.facts() {
            let snippet = f
                .evidence
                .as_ref()
                .and_then(|e| Some((e.page?, e.span?)))
                .map(|(p, s)| s.slice(&doc.pages[p as usize].text).replace('\n', " "))
                .unwrap_or_default();
            println!("{path}\t{}\t{:?}\t{:.2}\t{snippet}", f.key, f.value, f.conf);
        }
    }
}

//! Runs the HTML extractor over bytes built in the test.

#![allow(dead_code)]

use quail_core::{Blob, BlockKind, Document, Emitter, Extractor, Fact, Limits, Value, Zone};
use quail_html::HtmlExtractor;

pub struct Run {
    pub doc: Document,
    pub out: Emitter,
}

pub fn run(bytes: impl AsRef<[u8]>) -> Run {
    run_with(bytes, &Limits::default())
}

pub fn run_with(bytes: impl AsRef<[u8]>, limits: &Limits) -> Run {
    let blob = Blob::new(bytes.as_ref().to_vec());
    let mut out = Emitter::new(quail_html::ID);
    let doc = HtmlExtractor::new()
        .extract(&blob, limits, &mut out)
        .expect("extract");
    Run { doc, out }
}

impl Run {
    /// Every block of every page: `(page, kind, zone, text)`.
    pub fn blocks(&self) -> Vec<(u32, BlockKind, Zone, String)> {
        self.doc
            .pages
            .iter()
            .flat_map(|p| {
                p.blocks
                    .iter()
                    .map(move |b| (p.index, b.kind, b.zone, b.span.slice(&p.text).to_owned()))
            })
            .collect()
    }

    /// Block texts in order, over all pages.
    pub fn texts(&self) -> Vec<String> {
        self.blocks().into_iter().map(|b| b.3).collect()
    }

    /// The whole text, pages joined by a form feed.
    pub fn text(&self) -> String {
        self.doc
            .pages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\u{c}")
    }

    pub fn fact(&self, key: &str) -> Option<&Value> {
        self.out
            .facts()
            .iter()
            .find(|f: &&Fact| f.key.as_str() == key)
            .map(|f| &f.value)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        match self.fact(key) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<String> {
        match self.fact(key) {
            Some(Value::Text(t)) => Some(t.to_string()),
            _ => None,
        }
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        match self.fact(key) {
            Some(Value::List(v)) => v
                .iter()
                .filter_map(|x| match x {
                    Value::Text(t) => Some(t.to_string()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Problems (the record would be partial).
    pub fn problems(mut self) -> Vec<String> {
        self.out.take_problems()
    }

    /// Every span is on a char boundary inside its page text, blocks in order.
    pub fn assert_spans(&self) {
        for p in &self.doc.pages {
            let mut last = 0;
            for b in &p.blocks {
                assert!(b.span.start >= last && b.span.end >= b.span.start);
                assert!((b.span.end as usize) <= p.text.len());
                assert!(p.text.is_char_boundary(b.span.start as usize));
                assert!(p.text.is_char_boundary(b.span.end as usize));
                last = b.span.end;
            }
        }
    }
}

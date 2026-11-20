//! Throughput of the DOCX extractor on generated documents.

#![allow(missing_docs, clippy::format_push_string)]

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use common::{Docx, p, pn, ps, wpart};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use quail_core::{Blob, Emitter, Extractor, Limits};
use quail_docx::DocxExtractor;

const STYLES: &str = r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
const NUMBERING: &str = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:lvlText w:val="%1"/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:lvlText w:val="%1.%2"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;

/// A contract-like document: `articles` numbered articles of clauses, a table each, a
/// tracked change, a header and a footer.
fn contract(articles: usize) -> Vec<u8> {
    let mut body = String::new();
    for a in 0..articles {
        body.push_str(&ps("Heading1", &format!("Article heading {a}")));
        for c in 0..8 {
            body.push_str(&pn(
                1,
                1,
                &format!(
                    "The Supplier shall provide the services described in schedule {c} with due \
                     care, skill and diligence, in accordance with good industry practice."
                ),
            ));
        }
        body.push_str(r#"<w:p><w:r><w:t xml:space="preserve">Fee: </w:t></w:r><w:del w:id="1" w:author="A"><w:r><w:delText>100</w:delText></w:r></w:del><w:ins w:id="2" w:author="B"><w:r><w:t>200</w:t></w:r></w:ins></w:p>"#);
        body.push_str("<w:tbl>");
        for r in 0..4 {
            body.push_str("<w:tr>");
            for c in 0..3 {
                body.push_str(&format!(
                    "<w:tc>{}</w:tc>",
                    p(&format!("cell {r} {c} value"))
                ));
            }
            body.push_str("</w:tr>");
        }
        body.push_str("</w:tbl>");
        body.push_str(&p(""));
    }
    body.push_str(r#"<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/><w:footerReference w:type="default" r:id="rIdF"/></w:sectPr>"#);
    Docx::new(&body)
        .related("rId1", "styles", "styles.xml", &wpart("styles", STYLES))
        .related(
            "rId2",
            "numbering",
            "numbering.xml",
            &wpart("numbering", NUMBERING),
        )
        .related(
            "rIdH",
            "header",
            "header1.xml",
            &wpart("hdr", &p("Letterhead")),
        )
        .related(
            "rIdF",
            "footer",
            "footer1.xml",
            &wpart("ftr", &p("Confidential")),
        )
        .build()
}

fn bench(c: &mut Criterion) {
    let limits = Limits::default();
    let mut g = c.benchmark_group("docx_extract");
    for (name, articles) in [("contract_20", 20), ("contract_500", 500)] {
        let blob = Blob::from(contract(articles));
        g.throughput(Throughput::Bytes(blob.len() as u64));
        g.bench_function(name, |b| {
            b.iter(|| {
                let mut out = Emitter::new(quail_docx::ID);
                let doc = DocxExtractor
                    .extract(black_box(&blob), &limits, &mut out)
                    .unwrap();
                black_box((doc, out))
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

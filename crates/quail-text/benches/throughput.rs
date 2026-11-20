//! Throughput (MB/s) of the text utilities on generated text.
//!
//! `cargo bench -p quail-text --bench throughput` (filter: `-- normalize`, `-- lang` ...).
//! Inputs are built in code: a Serbian Latin contract-like text, its Cyrillic
//! transliteration and an English one, ~256 KiB each, plus a "dirty" variant with
//! hyphenation, ligatures, typographic quotes and odd whitespace.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use quail_text::fold::{FoldOptions, fold_into};
use quail_text::garbage::garbage_report;
use quail_text::lang::LanguageDetector;
use quail_text::normalize::{NormalizeOptions, normalize, normalize_into};
use quail_text::script::ScriptCounts;
use quail_text::shingle::{char_ngrams, text_shingles};
use quail_text::token::tokens;
use quail_text::translit::{cyrillic_to_latin, latin_to_cyrillic};

const SR: &str = "Član 5. Zakupac je dužan da plaća zakupninu u iznosu od 1.200,50 dinara do \
petog u mesecu na tekući račun broj 160-0000000123456-78, prema rešenju broj 114/23 od \
12.05.2023. godine. U slučaju kašnjenja zakupac plaća zakonsku zateznu kamatu. Ugovor je \
sačinjen u četiri istovetna primerka, po dva za svaku ugovornu stranu.\n";
const EN: &str = "Section 5. The tenant shall pay the rent in the amount of EUR 1,200.50 by \
the fifth day of each month to the account IBAN RS35 2600 0560 1001 6113 79, according to \
decision no. 114/23 of 12 May 2023. In case of delay the tenant pays statutory default \
interest. This agreement is made in four identical copies, two for each party.\n";
const DIRTY: &str = "Član 5. Zakupac je du\u{17e}an da pla\u{107}a zakup-\nninu u  iznosu od \
1\u{a0}200,50 dinara — „prema rešenju“ broj 114/23\u{200b} od ﬁnansijskog ﬂuksa \r\n\
ugo-\nvor o zakupu   \t poslovnog prostora.\n";

fn repeat_to(s: &str, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + s.len());
    while out.len() < bytes {
        out.push_str(s);
    }
    out
}

fn inputs() -> Vec<(&'static str, String)> {
    let n = 256 << 10;
    let sr = repeat_to(SR, n);
    let cyr = latin_to_cyrillic(&sr).into_owned();
    vec![
        ("sr-latn", sr),
        ("sr-cyrl", cyr),
        ("en", repeat_to(EN, n)),
        ("dirty", repeat_to(DIRTY, n)),
    ]
}

fn group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut g = c.benchmark_group(name);
    g.sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    g
}

fn utilities(c: &mut Criterion) {
    let inputs = inputs();
    let opts = NormalizeOptions::default();
    let mut g = group(c, "text");
    for (name, text) in &inputs {
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::new("normalize", name), text, |b, t| {
            b.iter(|| black_box(normalize(t, &opts)));
        });
        let mut buf = String::new();
        g.bench_with_input(BenchmarkId::new("normalize_into", name), text, |b, t| {
            b.iter(|| {
                buf.clear();
                normalize_into(t, &opts, &mut buf);
                black_box(buf.len())
            });
        });
        let mut fbuf = String::new();
        g.bench_with_input(BenchmarkId::new("fold_key", name), text, |b, t| {
            b.iter(|| {
                fbuf.clear();
                fold_into(t, &FoldOptions::KEY, &mut fbuf);
                black_box(fbuf.len())
            });
        });
        g.bench_with_input(BenchmarkId::new("tokenize", name), text, |b, t| {
            b.iter(|| black_box(tokens(t).count()));
        });
        g.bench_with_input(BenchmarkId::new("shingles_k5", name), text, |b, t| {
            b.iter(|| black_box(text_shingles(t, 5, &FoldOptions::KEY).fold(0u64, |a, h| a ^ h)));
        });
        g.bench_with_input(BenchmarkId::new("char_ngrams_5", name), text, |b, t| {
            b.iter(|| black_box(char_ngrams(t, 5).fold(0u64, |a, h| a ^ h)));
        });
        g.bench_with_input(BenchmarkId::new("scripts", name), text, |b, t| {
            b.iter(|| black_box(ScriptCounts::of(t)));
        });
        g.bench_with_input(BenchmarkId::new("garbage", name), text, |b, t| {
            b.iter(|| black_box(garbage_report(t)));
        });
        g.bench_with_input(BenchmarkId::new("cyr_to_lat", name), text, |b, t| {
            b.iter(|| black_box(cyrillic_to_latin(t).len()));
        });
        g.bench_with_input(BenchmarkId::new("lat_to_cyr", name), text, |b, t| {
            b.iter(|| black_box(latin_to_cyrillic(t).len()));
        });
    }
    g.finish();
}

fn language(c: &mut Criterion) {
    let det = LanguageDetector::new();
    let inputs = inputs();
    let mut g = group(c, "lang");
    for (name, text) in &inputs {
        // Per-document cost with the default 3 000-character sample.
        g.throughput(Throughput::Elements(1));
        g.bench_with_input(BenchmarkId::new("detect_doc_256k", name), text, |b, t| {
            b.iter(|| black_box(det.detect(t)));
        });
        let sentence = &text[..text.char_indices().nth(120).map_or(text.len(), |(i, _)| i)];
        g.bench_with_input(
            BenchmarkId::new("detect_120_chars", name),
            sentence,
            |b, t| {
                b.iter(|| black_box(det.detect(t)));
            },
        );
    }
    g.finish();
}

criterion_group!(benches, utilities, language);
criterion_main!(benches);

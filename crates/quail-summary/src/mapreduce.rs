//! The hierarchical (map-reduce) summary of a very long document, independent of the model.
//!
//! [`map_reduce`] takes the parts of [`crate::digest::map_parts`] (at most
//! [`MapReduceConfig::max_parts`](crate::digest::MapReduceConfig::max_parts)) and a `generate`
//! function (one model call: a user message in, raw JSON out):
//!
//! 1. **map**: one call per part ([`prompt::map_message`], output [`prompt::MAP_SCHEMA`]): what
//!    this part covers. Each part summary is [fact-checked](crate::factcheck) against the card
//!    and that part's excerpts; one that fails (or whose call fails) is replaced by the part's
//!    first excerpt, verbatim, so the reduce step only ever reads grounded text;
//! 2. **reduce**: one call over the card, the outline and the part summaries
//!    ([`prompt::reduce_message`], output [`prompt::SCHEMA`]), fact-checked against everything
//!    the map steps read.
//!
//! So a document of any length costs at most `max_parts + 1` calls of a bounded size. The
//! model runtime lives behind the `llm` feature ([`crate::llm`]); this module is pure and
//! tested with a scripted `generate`.

use quail_core::Result;

use crate::digest::MapPart;
use crate::factcheck;
use crate::prompt;
use crate::summarizer::{Method, Summary};

/// Which step a model call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A part summary ([`prompt::MAP_SCHEMA`]).
    Map,
    /// The final summary ([`prompt::SCHEMA`]).
    Reduce,
}

/// The result of [`map_reduce`].
#[derive(Clone, Debug, PartialEq)]
pub struct MapReduce {
    /// The final summary (its `checked` flag is the reduce step's fact check).
    pub summary: Summary,
    /// Model calls made (map and reduce).
    pub calls: usize,
    /// Part summaries replaced by an excerpt (failed call, bad JSON or fact check).
    pub map_fallbacks: usize,
    /// The part summaries the reduce step read, `(part title, summary)`.
    pub parts: Vec<(String, String)>,
}

/// Summarizes `parts` with `generate(step, user_message)` (see the module docs).
///
/// `card_text` is the card, `outline` the rendered outline, `lang` the document language.
/// Fails only when the reduce call fails or its output is no valid JSON; a summary that fails
/// the fact check comes back with `checked = false` and a warning.
pub fn map_reduce(
    card_text: &str,
    outline: &str,
    parts: &[MapPart],
    lang: Option<&str>,
    generate: &mut dyn FnMut(Step, &str) -> Result<String>,
) -> Result<MapReduce> {
    let n = parts.len();
    let mut calls = 0;
    let mut fallbacks = 0;
    let mut warnings = Vec::new();
    let mut summaries: Vec<(String, String)> = Vec::with_capacity(n);
    for (i, part) in parts.iter().enumerate() {
        let user = prompt::map_message(card_text, &part.title, i + 1, n, &part.body, lang);
        calls += 1;
        let out = generate(Step::Map, &user)
            .map_err(|e| e.to_string())
            .and_then(|raw| prompt::parse_map(&raw));
        let checked = out.and_then(|s| {
            let fc = factcheck::check(&s, &[card_text, &part.body, &part.title], true);
            if fc.passed() {
                Ok(s)
            } else {
                Err(format!("fact check: {}", fc.unsupported.join(", ")))
            }
        });
        let text = match checked {
            Ok(s) => s,
            Err(why) => {
                fallbacks += 1;
                warnings.push(format!("part {} of {n}: {why}", i + 1));
                first_excerpt(part)
            }
        };
        if !text.is_empty() {
            summaries.push((part.title.clone(), text));
        }
    }
    let user = prompt::reduce_message(card_text, outline, &summaries, lang);
    calls += 1;
    let raw = generate(Step::Reduce, &user)?;
    let out = prompt::parse(&raw)
        .map_err(|e| quail_core::Error::Resource(format!("model output: {e}")))?;
    // Grounded in what the map steps read: the card, the outline, every part.
    let mut sources: Vec<&str> = vec![card_text, outline];
    sources.extend(parts.iter().map(|p| p.body.as_str()));
    sources.extend(parts.iter().map(|p| p.title.as_str()));
    sources.extend(summaries.iter().map(|(_, s)| s.as_str()));
    let mut fc = factcheck::check(&format!("{} {}", out.summary, out.subject), &sources, true);
    let title = factcheck::check(&out.title_en, &sources, false);
    fc.claims += title.claims;
    fc.unsupported.extend(title.unsupported);
    let checked = fc.passed();
    if !checked {
        warnings.push(format!(
            "fact check failed: {} of {} claims not in the card or text ({})",
            fc.unsupported.len(),
            fc.claims,
            fc.unsupported.join(", ")
        ));
    }
    let nonempty = |s: String| (!s.is_empty()).then_some(s);
    Ok(MapReduce {
        summary: Summary {
            text: out.summary,
            subject: nonempty(out.subject),
            title_en: nonempty(out.title_en),
            method: Method::Llm,
            checked,
            warnings,
        },
        calls,
        map_fallbacks: fallbacks,
        parts: summaries,
    })
}

/// The first excerpt of a part (verbatim), or nothing.
fn first_excerpt(part: &MapPart) -> String {
    part.sections
        .iter()
        .flat_map(|s| &s.excerpts)
        .next()
        .map(|e| e.text.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::{DigestSection, Excerpt};

    fn part(title: &str, sentence: &str) -> MapPart {
        MapPart {
            title: title.into(),
            page: 0,
            end_page: 9,
            body: format!("## {title} (pp. 1-10)\n{sentence}"),
            tokens: 20,
            sections: vec![DigestSection {
                title: title.into(),
                page: 0,
                end_page: 9,
                size_tokens: 100,
                budget: 50,
                candidates: 1,
                excerpts: vec![Excerpt {
                    text: sentence.into(),
                    page: 0,
                }],
                keywords: Vec::new(),
            }],
            truncated: false,
        }
    }

    #[test]
    fn maps_then_reduces_with_fact_checks() {
        let parts = vec![
            part("Storage", "Engines keep 42 segments on disk for Northwind."),
            part("Replication", "Leaders copy every write to followers."),
        ];
        let mut seen: Vec<(Step, String)> = Vec::new();
        let mut generate = |step: Step, user: &str| -> Result<String> {
            seen.push((step, user.to_owned()));
            Ok(match (step, seen.len()) {
                // Grounded: 42 and Northwind are in the part.
                (Step::Map, 1) => r#"{"summary": "Northwind keeps 42 segments on disk."}"#.into(),
                // Not grounded: 77 is nowhere; replaced by the part's first excerpt.
                (Step::Map, _) => r#"{"summary": "Leaders copy 77 writes."}"#.into(),
                (Step::Reduce, _) => r#"{"summary": "A book on storage and replication for Northwind.", "subject": "data systems", "title_en": "Data Systems"}"#.into(),
            })
        };
        let r = map_reduce(
            "doc: book",
            "- Storage (pp. 1-10)",
            &parts,
            Some("en"),
            &mut generate,
        )
        .unwrap();
        assert_eq!(r.calls, 3);
        assert_eq!(r.map_fallbacks, 1);
        assert_eq!(r.parts[0].1, "Northwind keeps 42 segments on disk.");
        assert_eq!(r.parts[1].1, "Leaders copy every write to followers.");
        assert!(r.summary.checked, "{:?}", r.summary.warnings);
        assert_eq!(r.summary.method, Method::Llm);
        let reduce = &seen[2].1;
        assert!(reduce.contains("[2] Replication: Leaders copy every write to followers."));

        // A reduce output with an invented number fails the check.
        let mut bad = |step: Step, _: &str| -> Result<String> {
            Ok(match step {
                Step::Map => r#"{"summary": "It covers storage engines."}"#.into(),
                Step::Reduce => r#"{"summary": "A book of 900 pages on storage.", "subject": "x", "title_en": "x"}"#.into(),
            })
        };
        let r = map_reduce("doc: book", "", &parts, None, &mut bad).unwrap();
        assert!(!r.summary.checked);
        assert!(
            r.summary.warnings.iter().any(|w| w.contains("number:900")),
            "{:?}",
            r.summary.warnings
        );
    }
}

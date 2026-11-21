//! The LLM prompt, its output schema and the output parser. Model-independent and pure, so
//! the default build tests them.
//!
//! The fixed instructions come first so a runtime can reuse their KV cache across files. The
//! card is the source of truth: the prompt asks for names copied from it and no numbers or
//! dates beyond it, and [`crate::factcheck`] enforces that afterwards.

use std::fmt::Write as _;

use serde_json::Value as Json;

/// Bump when the prompt or the schema changes: it is part of the cache key.
pub const PROMPT_VERSION: u32 = 2;

/// System message.
pub const SYSTEM: &str = "You write search-index metadata for business documents. Output JSON \
only, in English. Use only FACTS and TEXT. Copy company names, numbers and identifiers exactly \
as written in FACTS or TEXT, in their original spelling and script; never translate names. Do \
not state any number or date that is not in FACTS or TEXT.";

/// JSON schema of the output; `maxLength` is a safety net, the prompt steers the length.
pub const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["summary","subject","title_en"],"properties":{"summary":{"type":"string","maxLength":400},"subject":{"type":"string","maxLength":120},"title_en":{"type":"string","maxLength":120}}}"#;

/// What the summary, subject and title must be (the end of the single and reduce messages).
const WRITE: &str = "Write: summary = 1-3 English sentences, at most 60 words: what the \
document is, between whom, its subject and purpose. subject = at most 15 words. title_en = the \
document title in English.";

/// The user message for one file: the card and `text`, the document's
/// [digest](crate::digest) body (the whole text of a short document; outline, start and
/// excerpts of a long one).
pub fn user_message(card_text: &str, text: &str, lang: Option<&str>) -> String {
    format!(
        "FACTS:\n{card_text}\n\nTEXT (the document, or for a long one its outline, its start \
         and sentences copied from each section; language={}):\n{text}\n\n{WRITE}",
        lang.unwrap_or("unknown")
    )
}

/// JSON schema of a map step's output (one part of a long document).
pub const MAP_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["summary"],"properties":{"summary":{"type":"string","maxLength":360}}}"#;

/// The user message of map step `i` (one-based) of `n`: the card and one part's excerpts.
pub fn map_message(
    card_text: &str,
    title: &str,
    i: usize,
    n: usize,
    body: &str,
    lang: Option<&str>,
) -> String {
    format!(
        "FACTS (the whole document):\n{card_text}\n\nPART {i} of {n}: {title}\nEXCERPTS \
         (sentences copied from this part, language={}):\n{body}\n\nWrite: summary = 1-2 \
         English sentences, at most 45 words: what this part of the document covers.",
        lang.unwrap_or("unknown")
    )
}

/// The user message of the reduce step: the card, the outline and the part summaries
/// (`title`, `summary`).
pub fn reduce_message(
    card_text: &str,
    outline: &str,
    parts: &[(String, String)],
    lang: Option<&str>,
) -> String {
    let mut list = String::new();
    for (i, (title, summary)) in parts.iter().enumerate() {
        let _ = writeln!(list, "[{}] {title}: {summary}", i + 1);
    }
    format!(
        "FACTS:\n{card_text}\n\nOUTLINE:\n{outline}\n\nPART SUMMARIES (of the whole \
         document, in order; language={}):\n{list}\n{WRITE}",
        lang.unwrap_or("unknown")
    )
}

/// Parses a map step's JSON: its summary, trimmed to the last full sentence.
pub fn parse_map(raw: &str) -> Result<String, String> {
    let start = raw.find('{').ok_or("no JSON object")?;
    let mut de = serde_json::Deserializer::from_str(&raw[start..]).into_iter::<Json>();
    let v = match de.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(format!("invalid JSON: {e}")),
        None => return Err("no JSON object".to_owned()),
    };
    let s = v
        .get("summary")
        .and_then(Json::as_str)
        .map(|s| last_sentence(&crate::format::one_line(s.trim())))
        .unwrap_or_default();
    if s.split_whitespace().count() < 3 {
        return Err("empty summary".to_owned());
    }
    Ok(s)
}

/// A `ChatML` prompt (Qwen) with thinking switched off: an empty `<think>` block opens the
/// assistant turn, as the Qwen3 chat template does for `enable_thinking=false`.
pub fn chatml(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n\
         <|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
}

/// The parsed model output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// 1–3 sentences, trimmed to the last full sentence.
    pub summary: String,
    /// A few words.
    pub subject: String,
    /// English title.
    pub title_en: String,
}

/// Parses the model's JSON. Text after the object is ignored; a summary cut by `maxLength`
/// is trimmed to its last full sentence.
pub fn parse(raw: &str) -> Result<Output, String> {
    let start = raw.find('{').ok_or("no JSON object")?;
    let mut de = serde_json::Deserializer::from_str(&raw[start..]).into_iter::<Json>();
    let v = match de.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(format!("invalid JSON: {e}")),
        None => return Err("no JSON object".to_owned()),
    };
    let get = |k: &str| {
        v.get(k)
            .and_then(Json::as_str)
            .map(|s| crate::format::one_line(s.trim()))
            .unwrap_or_default()
    };
    let out = Output {
        summary: last_sentence(&get("summary")),
        subject: get("subject"),
        title_en: get("title_en"),
    };
    if out.summary.split_whitespace().count() < 3 {
        return Err("empty summary".to_owned());
    }
    Ok(out)
}

/// `s` up to its last sentence end (`.`, `!`, `?`), or all of it when none.
fn last_sentence(s: &str) -> String {
    match s.rfind(['.', '!', '?']) {
        Some(i) if i + 1 < s.len() && i > s.len() / 3 => s[..=i].to_owned(),
        _ => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_json() {
        let v: Json = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(v["required"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn parses_and_trims() {
        let o = parse(
            "{\"summary\": \"An amendment between A and B. It extends the term to 2026. It also chan\", \
             \"subject\": \"term extension\", \"title_en\": \"Amendment No. 2\"}\n<|im_end|>",
        )
        .unwrap();
        assert_eq!(
            o.summary,
            "An amendment between A and B. It extends the term to 2026."
        );
        assert_eq!(o.subject, "term extension");
        assert!(parse("nothing").is_err());
        assert!(parse("{\"summary\": \"x\"}").is_err());
    }

    #[test]
    fn map_and_reduce_messages() {
        let v: Json = serde_json::from_str(MAP_SCHEMA).unwrap();
        assert_eq!(v["required"][0], "summary");
        let m = map_message(
            "doc: book",
            "Chapter 2 … Chapter 3",
            2,
            5,
            "## Chapter 2\nA.",
            None,
        );
        assert!(
            m.contains("PART 2 of 5: Chapter 2 … Chapter 3\nEXCERPTS"),
            "{m}"
        );
        let r = reduce_message(
            "doc: book",
            "- Chapter 1 (pp. 1-9)",
            &[("Chapter 1".into(), "It covers storage.".into())],
            Some("en"),
        );
        assert!(r.contains("[1] Chapter 1: It covers storage.\n"), "{r}");
        assert!(r.ends_with("document title in English."));
        assert_eq!(
            parse_map("{\"summary\": \"It covers storage engines. It also lists\"}").unwrap(),
            "It covers storage engines."
        );
        assert!(parse_map("{}").is_err());
    }

    #[test]
    fn prompt_has_the_fixed_prefix_first() {
        let p = chatml(SYSTEM, &user_message("doc: msa", "TEXT", Some("en")));
        assert!(p.starts_with("<|im_start|>system\nYou write search-index metadata"));
        assert!(p.ends_with("<think>\n\n</think>\n\n"));
        assert!(p.contains("language=en"));
    }
}

//! The LLM prompt, its output schema and the output parser. Model-independent and pure, so
//! the default build tests them.
//!
//! The fixed instructions come first so a runtime can reuse their KV cache across files. The
//! card is the source of truth: the prompt asks for names copied from it and no numbers or
//! dates beyond it, and [`crate::factcheck`] enforces that afterwards.

use serde_json::Value as Json;

/// Bump when the prompt or the schema changes: it is part of the cache key.
pub const PROMPT_VERSION: u32 = 1;

/// System message.
pub const SYSTEM: &str = "You write search-index metadata for business documents. Output JSON \
only, in English. Use only FACTS and TEXT. Copy company names, numbers and identifiers exactly \
as written in FACTS or TEXT, in their original spelling and script; never translate names. Do \
not state any number or date that is not in FACTS or TEXT.";

/// JSON schema of the output; `maxLength` is a safety net, the prompt steers the length.
pub const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["summary","subject","title_en"],"properties":{"summary":{"type":"string","maxLength":400},"subject":{"type":"string","maxLength":120},"title_en":{"type":"string","maxLength":120}}}"#;

/// The user message for one file.
pub fn user_message(card_text: &str, text_head: &str, lang: Option<&str>) -> String {
    format!(
        "FACTS:\n{card_text}\n\nTEXT (start of the document, language={}):\n{text_head}\n\n\
         Write: summary = 1-3 English sentences, at most 60 words: what the document is, \
         between whom, its subject and purpose. subject = at most 15 words. title_en = the \
         document title in English.",
        lang.unwrap_or("unknown")
    )
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
    fn prompt_has_the_fixed_prefix_first() {
        let p = chatml(SYSTEM, &user_message("doc: msa", "TEXT", Some("en")));
        assert!(p.starts_with("<|im_start|>system\nYou write search-index metadata"));
        assert!(p.ends_with("<think>\n\n</think>\n\n"));
        assert!(p.contains("language=en"));
    }
}

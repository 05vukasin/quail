//! Request bodies and response parsing of the three wire protocols. Pure: no I/O ([`request`]
//! reads the clock for a `SigV4` signature; [`request_at`] takes the time).
//!
//! | protocol | request | text | usage |
//! |---|---|---|---|
//! | `OpenAI` Chat Completions | `model`, `messages` (system, user), `max_tokens` (`max_completion_tokens` on api.openai.com), `temperature`, `response_format = json_object`, the preset's thinking switch | `choices[0].message.content` | `usage.prompt_tokens`, `usage.completion_tokens` |
//! | Anthropic Messages | `model`, `max_tokens`, `system`, `messages` (user), `output_config.effort = low` (no `thinking` field); headers `x-api-key`, `anthropic-version` | the `text` blocks of `content` | `usage.input_tokens`, `usage.output_tokens` |
//! | Bedrock Converse | `messages` (user), `system`, `inferenceConfig` (`maxTokens`, `temperature`); header `Authorization: Bearer`, or `SigV4` (`Authorization: AWS4-HMAC-SHA256 ...`, `x-amz-date`, `x-amz-security-token`, `host`) | the `text` blocks of `output.message.content` | `usage.inputTokens`, `usage.outputTokens` |
//!
//! The *optional* fields (temperature, JSON mode, thinking switches, effort) are left out of a
//! `plain` request: a gateway that rejects one of them with 400/422 gets the request again
//! without them, as hedgehog does for its thinking switch.

use serde_json::{Map, Value as Json, json};

use std::time::SystemTime;

use super::endpoint::{Endpoint, Kind};
use super::sigv4;
use crate::summarizer::Usage;

/// The `anthropic-version` header.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Decoding settings of a request.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoding {
    /// Output tokens at most.
    pub max_tokens: u32,
    /// Sampling temperature (`None`: not sent).
    pub temperature: Option<f64>,
    /// Ask for a JSON object (`OpenAI` protocol `response_format`).
    pub json_mode: bool,
    /// Switch thinking off where the preset has a switch (`OpenAI` protocol only; Anthropic gets
    /// a low [`Self::effort`] instead).
    pub thinking_off: bool,
    /// Anthropic `output_config.effort` (`None`: not sent).
    pub effort: Option<String>,
}

/// One request: headers (the key among them) and the JSON body.
pub struct Request {
    /// Extra headers (`content-type` is added by the client).
    pub headers: Vec<(&'static str, String)>,
    /// JSON body.
    pub body: String,
}

impl std::fmt::Debug for Request {
    /// Header names only: one of them carries the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| *k).collect();
        f.debug_struct("Request")
            .field("headers", &names)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// The request for `system` + `user` to `ep`; `plain` leaves the optional fields out.
pub fn request(ep: &Endpoint, system: &str, user: &str, d: &Decoding, plain: bool) -> Request {
    request_at(ep, system, user, d, plain, SystemTime::now())
}

/// [`request`] signed at `now` (Bedrock with IAM credentials; other requests ignore it).
pub fn request_at(
    ep: &Endpoint,
    system: &str,
    user: &str,
    d: &Decoding,
    plain: bool,
    now: SystemTime,
) -> Request {
    let mut headers: Vec<(&'static str, String)> = Vec::new();
    let mut body = Map::new();
    match ep.kind {
        Kind::OpenAi => {
            if let Some(k) = &ep.key {
                headers.push(("authorization", format!("Bearer {}", k.expose())));
            }
            body.insert("model".into(), json!(ep.model));
            body.insert(
                "messages".into(),
                json!([
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ]),
            );
            let max_key = if ep.preset == "openai" {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body.insert(max_key.into(), json!(d.max_tokens));
            body.insert("stream".into(), json!(false));
            if !plain {
                if let Some(t) = d.temperature {
                    body.insert("temperature".into(), json!(t));
                }
                if d.json_mode {
                    body.insert("response_format".into(), json!({"type": "json_object"}));
                }
                if d.thinking_off
                    && let Some(Json::Object(extra)) = ep.thinking.body()
                {
                    body.extend(extra);
                }
            }
        }
        Kind::Anthropic => {
            if let Some(k) = &ep.key {
                headers.push(("x-api-key", k.expose().to_owned()));
            }
            headers.push(("anthropic-version", ANTHROPIC_VERSION.to_owned()));
            body.insert("model".into(), json!(ep.model));
            body.insert("max_tokens".into(), json!(d.max_tokens));
            body.insert("system".into(), json!(system));
            body.insert(
                "messages".into(),
                json!([{"role": "user", "content": user}]),
            );
            if !plain {
                if let Some(t) = d.temperature {
                    body.insert("temperature".into(), json!(t));
                }
                // No `thinking` field: `{"type": "disabled"}` is a 400 on Claude Opus 5.5, Sonnet
                // 5.5 and Fable; a low effort keeps adaptive thinking short on every model.
                if let Some(e) = &d.effort {
                    body.insert("output_config".into(), json!({"effort": e}));
                }
            }
        }
        Kind::Bedrock => {
            if let Some(k) = &ep.key {
                headers.push(("authorization", format!("Bearer {}", k.expose())));
            }
            body.insert(
                "messages".into(),
                json!([{"role": "user", "content": [{"text": user}]}]),
            );
            body.insert("system".into(), json!([{"text": system}]));
            let mut inference = Map::new();
            inference.insert("maxTokens".into(), json!(d.max_tokens));
            if !plain && let Some(t) = d.temperature {
                inference.insert("temperature".into(), json!(t));
            }
            body.insert("inferenceConfig".into(), Json::Object(inference));
        }
    }
    let body = Json::Object(body).to_string();
    if ep.kind == Kind::Bedrock
        && ep.key.is_none()
        && let Some(c) = &ep.aws
    {
        let region = ep.region.as_deref().unwrap_or_default();
        headers.extend(sigv4::bedrock_headers(c, region, &ep.url(), &body, now));
    }
    Request { headers, body }
}

/// Whether `d` has anything a `plain` request leaves out for `ep`.
pub fn has_optional(ep: &Endpoint, d: &Decoding) -> bool {
    match ep.kind {
        Kind::OpenAi => {
            d.temperature.is_some()
                || d.json_mode
                || (d.thinking_off && ep.thinking.body().is_some())
        }
        Kind::Anthropic => d.temperature.is_some() || d.effort.is_some(),
        Kind::Bedrock => d.temperature.is_some(),
    }
}

/// The model's text and token usage in a success response of `kind`.
pub fn parse(kind: Kind, body: &str) -> Result<(String, Usage), String> {
    let v: Json = serde_json::from_str(body).map_err(|e| format!("malformed response: {e}"))?;
    let u = |o: &Json, k: &str| o.get(k).and_then(Json::as_u64).unwrap_or(0);
    let (text, usage) = match kind {
        Kind::OpenAi => {
            let choice = v
                .get("choices")
                .and_then(|c| c.get(0))
                .ok_or("malformed response: no choices")?;
            if choice.get("finish_reason").and_then(Json::as_str) == Some("content_filter") {
                return Err("the provider's content filter stopped the output".to_owned());
            }
            let msg = choice
                .get("message")
                .ok_or("malformed response: no message")?;
            if let Some(r) = msg.get("refusal").and_then(Json::as_str)
                && !r.is_empty()
            {
                return Err("the model refused".to_owned());
            }
            let text = msg
                .get("content")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned();
            let us = v.get("usage").cloned().unwrap_or(Json::Null);
            (
                text,
                Usage {
                    prompt_tokens: u(&us, "prompt_tokens"),
                    output_tokens: u(&us, "completion_tokens"),
                },
            )
        }
        Kind::Anthropic => {
            if v.get("stop_reason").and_then(Json::as_str) == Some("refusal") {
                return Err("the model refused".to_owned());
            }
            let blocks = v
                .get("content")
                .and_then(Json::as_array)
                .ok_or("malformed response: no content")?;
            let us = v.get("usage").cloned().unwrap_or(Json::Null);
            (
                texts(blocks, |b| {
                    (b.get("type").and_then(Json::as_str) == Some("text"))
                        .then(|| b.get("text").and_then(Json::as_str))
                        .flatten()
                }),
                Usage {
                    prompt_tokens: u(&us, "input_tokens")
                        + u(&us, "cache_read_input_tokens")
                        + u(&us, "cache_creation_input_tokens"),
                    output_tokens: u(&us, "output_tokens"),
                },
            )
        }
        Kind::Bedrock => {
            if matches!(
                v.get("stopReason").and_then(Json::as_str),
                Some("guardrail_intervened" | "content_filtered")
            ) {
                return Err("the provider's content filter stopped the output".to_owned());
            }
            let blocks = v
                .pointer("/output/message/content")
                .and_then(Json::as_array)
                .ok_or("malformed response: no output.message.content")?;
            let us = v.get("usage").cloned().unwrap_or(Json::Null);
            (
                texts(blocks, |b| b.get("text").and_then(Json::as_str)),
                Usage {
                    prompt_tokens: u(&us, "inputTokens"),
                    output_tokens: u(&us, "outputTokens"),
                },
            )
        }
    };
    let text = strip_think(&text);
    if text.trim().is_empty() {
        return Err("empty response".to_owned());
    }
    Ok((text, usage))
}

fn texts<'a>(blocks: &'a [Json], f: impl Fn(&'a Json) -> Option<&'a str>) -> String {
    blocks.iter().filter_map(f).collect::<Vec<_>>().join("")
}

/// `text` without `<think>…</think>` blocks (reasoning models that ignore the switch).
pub fn strip_think(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("<think>") {
        out.push_str(&rest[..at]);
        match rest[at..].find("</think>") {
            Some(end) => rest = &rest[at + end + "</think>".len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A short, single-line description of an error response (the provider's message when it
/// has one), at most 200 characters.
pub fn error_message(body: &str) -> String {
    let v: Option<Json> = serde_json::from_str(body).ok();
    let msg = v
        .as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .or_else(|| v.get("Message"))
                .or_else(|| v.get("error"))
                .and_then(Json::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.to_owned());
    let one: String = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::endpoint::{Connect, Endpoint};
    use crate::api::env::Env;

    fn ep(provider: &str, pairs: &[(&str, &str)]) -> Endpoint {
        let req = Connect {
            provider: Some(provider.into()),
            ..Connect::default()
        };
        Endpoint::resolve(&req, &Env::from_map(pairs.iter().map(|&(k, v)| (k, v)))).unwrap()
    }

    fn dec() -> Decoding {
        Decoding {
            max_tokens: 512,
            temperature: Some(0.0),
            json_mode: true,
            thinking_off: true,
            effort: Some("low".into()),
        }
    }

    #[test]
    fn openai_compatible_bodies_switch_thinking_off() {
        let vllm = ep(
            "vllm",
            &[("API_URL", "http://gpu:8000"), ("API_MODEL", "Qwen/Qwen3")],
        );
        let r = request(&vllm, "S", "U", &dec(), false);
        let b: Json = serde_json::from_str(&r.body).unwrap();
        assert_eq!(b["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(b["max_tokens"], 512);
        assert_eq!(b["temperature"], 0.0);
        assert_eq!(b["response_format"]["type"], "json_object");
        assert_eq!(b["messages"][0]["role"], "system");
        assert!(r.headers.is_empty(), "no key, no authorization header");
        let or = ep(
            "openrouter",
            &[("OPENROUTER_API_KEY", "sk-or"), ("OPENROUTER_MODEL", "q")],
        );
        let b: Json = serde_json::from_str(&request(&or, "S", "U", &dec(), false).body).unwrap();
        assert_eq!(b["reasoning"]["enabled"], false);
        let oa = ep("openai", &[("OPENAI_API_KEY", "sk-1")]);
        let r = request(&oa, "S", "U", &dec(), true);
        let b: Json = serde_json::from_str(&r.body).unwrap();
        assert_eq!(b["max_completion_tokens"], 512);
        assert!(b.get("temperature").is_none() && b.get("response_format").is_none());
        assert_eq!(r.headers, vec![("authorization", "Bearer sk-1".to_owned())]);
        let dbg = format!("{r:?}");
        assert!(!dbg.contains("sk-1"), "{dbg}");
    }

    #[test]
    fn anthropic_and_bedrock_bodies() {
        let a = ep("anthropic", &[("ANTHROPIC_API_KEY", "sk-ant")]);
        let d = Decoding {
            temperature: None,
            ..dec()
        };
        let r = request(&a, "S", "U", &d, false);
        let b: Json = serde_json::from_str(&r.body).unwrap();
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["system"], "S");
        assert!(
            b.get("thinking").is_none(),
            "thinking=disabled is a 400 on most Claude models"
        );
        assert_eq!(b["output_config"]["effort"], "low");
        assert!(b.get("temperature").is_none());
        assert!(
            r.headers
                .contains(&("anthropic-version", ANTHROPIC_VERSION.to_owned()))
        );
        assert!(has_optional(&a, &d));
        let br = ep(
            "bedrock",
            &[
                ("AI_BEDROCK_BEARER_TOKEN", "t"),
                ("AI_BEDROCK_REGION", "eu-west-2"),
                ("AI_BEDROCK_MODEL", "m"),
            ],
        );
        let b: Json = serde_json::from_str(&request(&br, "S", "U", &dec(), false).body).unwrap();
        assert_eq!(b["inferenceConfig"]["maxTokens"], 512);
        assert_eq!(b["inferenceConfig"]["temperature"], 0.0);
        assert_eq!(b["messages"][0]["content"][0]["text"], "U");
    }

    #[test]
    fn bedrock_with_iam_credentials_is_signed() {
        let pairs = [
            ("AI_BEDROCK_REGION", "eu-west-2"),
            ("AI_BEDROCK_MODEL", "qwen.q3:0"),
            ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
            (
                "AWS_SECRET_ACCESS_KEY",
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            ),
        ];
        let br = ep("bedrock", &pairs);
        let secs = 1_440_938_160;
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let r = request_at(&br, "S", "U", &dec(), false, t);
        let names: Vec<&str> = r.headers.iter().map(|(k, _)| *k).collect();
        assert_eq!(names, ["host", "x-amz-date", "authorization"]);
        assert_eq!(r.headers[0].1, "bedrock-runtime.eu-west-2.amazonaws.com");
        assert_eq!(r.headers[1].1, "20150830T123600Z");
        assert!(r.headers[2].1.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-2/bedrock/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, Signature="
        ));
        // Same input, same time: the same signature; the body is part of it.
        assert_eq!(
            request_at(&br, "S", "U", &dec(), false, t).headers,
            r.headers
        );
        assert_ne!(
            request_at(&br, "S", "V", &dec(), false, t).headers[2],
            r.headers[2]
        );
        let dbg = format!("{r:?}");
        assert!(!dbg.contains("Signature") && !dbg.contains("AKID"), "{dbg}");
    }

    #[test]
    fn parses_each_protocol() {
        let (t, u) = parse(
            Kind::OpenAi,
            r#"{"choices":[{"message":{"content":"<think>hm {x}</think>{\"summary\":\"a\"}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
        )
        .unwrap();
        assert_eq!(
            (t.as_str(), u.prompt_tokens, u.output_tokens),
            ("{\"summary\":\"a\"}", 10, 3)
        );
        let (t, u) = parse(
            Kind::Anthropic,
            r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"{}"}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":2}}"#,
        )
        .unwrap();
        assert_eq!((t.as_str(), u.prompt_tokens), ("{}", 7));
        assert!(
            parse(Kind::Anthropic, r#"{"content":[],"stop_reason":"refusal"}"#)
                .unwrap_err()
                .contains("refused")
        );
        let (t, u) = parse(
            Kind::Bedrock,
            r#"{"output":{"message":{"role":"assistant","content":[{"text":"{}"}]}},"stopReason":"end_turn","usage":{"inputTokens":5,"outputTokens":1}}"#,
        )
        .unwrap();
        assert_eq!((t.as_str(), u.output_tokens), ("{}", 1));
        assert!(
            parse(Kind::OpenAi, "not json")
                .unwrap_err()
                .starts_with("malformed")
        );
        assert!(parse(Kind::OpenAi, r#"{"choices":[]}"#).is_err());
        assert!(
            parse(Kind::OpenAi, r#"{"choices":[{"message":{"content":""}}]}"#)
                .unwrap_err()
                .contains("empty")
        );
    }

    #[test]
    fn error_messages_are_short_lines() {
        assert_eq!(
            error_message(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
            ),
            "Overloaded"
        );
        assert_eq!(
            error_message(r#"{"message":"Too many\nrequests"}"#),
            "Too many requests"
        );
        assert_eq!(error_message(&"x".repeat(500)).len(), 200);
    }
}

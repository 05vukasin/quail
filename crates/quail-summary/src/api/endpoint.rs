//! Which API to call: presets, and the resolution of every setting.
//!
//! Each setting comes from, in this order: the caller's argument ([`Connect`]), the process
//! environment, the `.env` file the caller named ([`Env::with_file`]; none is read
//! otherwise), the preset's default, else a clear error. The variable names are hedgehog's
//! (`.env.example` at the repository root), so one `.env` serves both:
//!
//! | preset | key | base URL | model |
//! |---|---|---|---|
//! | `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` (optional) | `OPENAI_MODEL`, else `gpt-4o-mini` |
//! | `openai_compatible` | `OPENAI_API_KEY` (optional on localhost) | `OPENAI_BASE_URL` (required) | `OPENAI_MODEL` (required) |
//! | `openrouter` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`, else `https://openrouter.ai/api/v1` | `OPENROUTER_MODEL` (required) |
//! | `groq` | `GROQ_API_KEY` | `GROQ_BASE_URL`, else `https://api.groq.com/openai/v1` | `GROQ_MODEL` (required) |
//! | `vllm` (also `local`, `lmstudio`) | `API_TOKEN` (optional) | `API_URL` (required; `/v1` appended) | `API_MODEL` (required) |
//! | `ollama` | `API_TOKEN` (optional) | `API_URL`, else `http://localhost:11434/v1` | `API_MODEL` (required) |
//! | `anthropic` (also `claude`) | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL`, else `https://api.anthropic.com` | `ANTHROPIC_MODEL`, else `claude-haiku-5-5` |
//! | `bedrock` (also `aws`) | `AI_BEDROCK_BEARER_TOKEN` (or `AWS_BEARER_TOKEN_BEDROCK`) | region `AI_BEDROCK_REGION` (or `AWS_REGION`, `AWS_DEFAULT_REGION`) | `AI_BEDROCK_MODEL` (required) |
//!
//! `AI_PROVIDER` picks the preset when the caller names none; when it is unset too and exactly
//! one preset has credentials, that one is used. `AI_CHAT_MODEL` is the last model fallback,
//! used only when `AI_PROVIDER` is unset or names the same preset (so a Bedrock model id never
//! reaches Anthropic). `AI_MAX_CONCURRENT_LLM` caps concurrent calls process-wide (default 4).
//!
//! llama.cpp's `llama-server`, vLLM, `SGLang`, Ollama and LM Studio all speak the `OpenAI` Chat
//! Completions protocol: a self-hosted (for example fine-tuned) model is the `vllm` preset
//! with `API_URL`.

use std::fmt::{self, Write as _};

use serde_json::{Value as Json, json};

use super::env::Env;
use quail_core::{Error, Result};

/// The `OpenAI` preset's model when none is set: small and cheap (a summary only phrases what
/// the card and the text already say).
pub const OPENAI_DEFAULT_MODEL: &str = "gpt-4o-mini";
/// The Anthropic preset's model when none is set: the cheapest current Claude model, for bulk
/// summaries. `ANTHROPIC_MODEL` overrides it.
pub const ANTHROPIC_DEFAULT_MODEL: &str = "claude-haiku-5-5";
/// Every preset name.
pub const PRESETS: &[&str] = &[
    "openai",
    "openai_compatible",
    "anthropic",
    "bedrock",
    "openrouter",
    "groq",
    "vllm",
    "ollama",
];
/// Process-wide concurrent calls when `AI_MAX_CONCURRENT_LLM` is unset.
pub const DEFAULT_MAX_CONCURRENT: usize = 4;

/// An API key or bearer token. Its `Debug` prints `Secret(***)`; it has no `Display`, so it
/// cannot end up in a log line, an error or a record by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a key.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key itself, for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// The wire protocol of an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `OpenAI` Chat Completions (`POST {base}/chat/completions`), also every compatible server.
    OpenAi,
    /// Anthropic Messages (`POST {base}/v1/messages`).
    Anthropic,
    /// AWS Bedrock Converse (`POST {base}/model/{model}/converse`) with a bearer token.
    Bedrock,
}

impl Kind {
    /// `openai`, `anthropic` or `bedrock`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Bedrock => "bedrock",
        }
    }
}

/// How a preset switches thinking off (Qwen-style reasoning models behind vLLM, Ollama,
/// `SGLang`; `OpenRouter`'s own switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkingOff {
    /// Nothing is sent.
    None,
    /// `chat_template_kwargs.enable_thinking = false` (vLLM, Ollama, `SGLang`, llama-server).
    Template,
    /// `reasoning.enabled = false` (`OpenRouter`).
    OpenRouter,
}

impl ThinkingOff {
    /// The request fields, if any.
    pub fn body(self) -> Option<Json> {
        match self {
            Self::None => None,
            Self::Template => Some(json!({"chat_template_kwargs": {"enable_thinking": false}})),
            Self::OpenRouter => Some(json!({"reasoning": {"enabled": false}})),
        }
    }
}

struct Preset {
    name: &'static str,
    kind: Kind,
    key_env: &'static str,
    url_env: &'static str,
    model_env: &'static str,
    default_url: Option<&'static str>,
    key_optional: bool,
    thinking: ThinkingOff,
    append_v1: bool,
}

const fn preset(
    name: &'static str,
    kind: Kind,
    key: &'static str,
    url: &'static str,
    model: &'static str,
) -> Preset {
    Preset {
        name,
        kind,
        key_env: key,
        url_env: url,
        model_env: model,
        default_url: None,
        key_optional: false,
        thinking: ThinkingOff::None,
        append_v1: false,
    }
}

const ALL: &[Preset] = &[
    preset(
        "openai",
        Kind::OpenAi,
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
    ),
    Preset {
        thinking: ThinkingOff::Template,
        ..preset(
            "openai_compatible",
            Kind::OpenAi,
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
        )
    },
    Preset {
        default_url: Some("https://openrouter.ai/api/v1"),
        thinking: ThinkingOff::OpenRouter,
        ..preset(
            "openrouter",
            Kind::OpenAi,
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
        )
    },
    Preset {
        default_url: Some("https://api.groq.com/openai/v1"),
        ..preset(
            "groq",
            Kind::OpenAi,
            "GROQ_API_KEY",
            "GROQ_BASE_URL",
            "GROQ_MODEL",
        )
    },
    Preset {
        key_optional: true,
        thinking: ThinkingOff::Template,
        append_v1: true,
        ..preset("vllm", Kind::OpenAi, "API_TOKEN", "API_URL", "API_MODEL")
    },
    Preset {
        default_url: Some("http://localhost:11434/v1"),
        key_optional: true,
        thinking: ThinkingOff::Template,
        append_v1: true,
        ..preset("ollama", Kind::OpenAi, "API_TOKEN", "API_URL", "API_MODEL")
    },
    Preset {
        default_url: Some("https://api.anthropic.com"),
        ..preset(
            "anthropic",
            Kind::Anthropic,
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
        )
    },
    preset(
        "bedrock",
        Kind::Bedrock,
        "AI_BEDROCK_BEARER_TOKEN",
        "AI_BEDROCK_REGION",
        "AI_BEDROCK_MODEL",
    ),
];

fn find(name: &str) -> Result<&'static Preset> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let n = match n.as_str() {
        "compatible" => "openai_compatible",
        "local" | "localmodels" | "lmstudio" | "lm_studio" => "vllm",
        "claude" => "anthropic",
        "aws" | "aws_bedrock" => "bedrock",
        other => other,
    };
    ALL.iter().find(|p| p.name == n).ok_or_else(|| {
        config(format!(
            "unknown LLM provider {name:?}; use one of {}",
            PRESETS.join(", ")
        ))
    })
}

fn config(msg: impl Into<String>) -> Error {
    let msg: String = msg.into();
    Error::Other(format!("llm config: {msg}"))
}

/// The host of `url` (`http://user@host:8000/v1` → `host:8000`), for fingerprints and logs.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit_once('@').map_or(authority, |(_, h)| h)
}

fn is_local(url: &str) -> bool {
    let host = host_of(url).to_ascii_lowercase();
    let name = if host.starts_with('[') {
        host.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
        || name.strip_suffix(".local").is_some_and(|h| !h.is_empty())
}

fn with_v1(url: &str) -> String {
    let base = url.trim_end_matches('/');
    if base.ends_with("/v1") {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}

/// What the caller says explicitly; every `None` is looked up in the [`Env`].
#[derive(Clone, Debug, Default)]
pub struct Connect {
    /// Preset name (`anthropic`, `openai`, `vllm`, ...).
    pub provider: Option<String>,
    /// Model id.
    pub model: Option<String>,
    /// Base URL (for Bedrock: the endpoint instead of the regional one).
    pub base_url: Option<String>,
    /// API key or bearer token.
    pub api_key: Option<Secret>,
    /// Bedrock region.
    pub region: Option<String>,
}

/// A resolved endpoint: everything needed to call one model. `Debug` never shows the key.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// The preset it came from.
    pub preset: &'static str,
    /// Wire protocol.
    pub kind: Kind,
    /// Base URL: `.../v1` for the `OpenAI` protocol, the API root for Anthropic, the regional
    /// `bedrock-runtime` root for Bedrock.
    pub base_url: String,
    /// Model id.
    pub model: String,
    /// Key or bearer token; `None` for a local server that needs none.
    pub key: Option<Secret>,
    /// Bedrock region.
    pub region: Option<String>,
    /// How thinking is switched off.
    pub thinking: ThinkingOff,
    /// Concurrent calls process-wide (`AI_MAX_CONCURRENT_LLM`).
    pub max_concurrent: usize,
}

impl Endpoint {
    /// Resolves `req` against `env` (see the module docs for the order).
    #[allow(clippy::too_many_lines)] // one table of presets, in order
    pub fn resolve(req: &Connect, env: &Env) -> Result<Self> {
        let env_provider = env.get("AI_PROVIDER");
        let name = if let Some(n) = req.provider.clone().or_else(|| env_provider.clone()) {
            n
        } else {
            let found: Vec<&str> = available(env)
                .into_iter()
                .filter(|n| *n != "openai_compatible")
                .collect();
            if let [one] = found.as_slice() {
                (*one).to_owned()
            } else {
                let have = if found.is_empty() {
                    String::new()
                } else {
                    format!(" (credentials found for: {})", found.join(", "))
                };
                return Err(config(format!(
                    "no LLM provider given: pass one or set AI_PROVIDER to one of {}{have}",
                    PRESETS.join(", ")
                )));
            }
        };
        let p = find(&name)?;
        let same_as_env = env_provider
            .as_ref()
            .is_none_or(|e| find(e).is_ok_and(|e| e.name == p.name));
        let model = req
            .model
            .clone()
            .or_else(|| env.get(p.model_env))
            .or_else(|| same_as_env.then(|| env.get("AI_CHAT_MODEL")).flatten());
        let max_concurrent = env
            .get("AI_MAX_CONCURRENT_LLM")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_CONCURRENT);
        let key = req
            .api_key
            .clone()
            .or_else(|| env.get(p.key_env).map(Secret::new));
        let mut ep = Self {
            preset: p.name,
            kind: p.kind,
            base_url: String::new(),
            model: String::new(),
            key: None,
            region: None,
            thinking: p.thinking,
            max_concurrent,
        };
        match p.kind {
            Kind::Anthropic => {
                ep.key = Some(key.ok_or_else(|| {
                    config("anthropic needs an API key: set ANTHROPIC_API_KEY".to_owned())
                })?);
                ep.model = model.unwrap_or_else(|| ANTHROPIC_DEFAULT_MODEL.to_owned());
                req.base_url
                    .clone()
                    .or_else(|| env.get(p.url_env))
                    .or_else(|| p.default_url.map(str::to_owned))
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .clone_into(&mut ep.base_url);
            }
            Kind::OpenAi => {
                let url = req
                    .base_url
                    .clone()
                    .or_else(|| env.get(p.url_env))
                    .or_else(|| p.default_url.map(str::to_owned));
                let url = match (url, p.name) {
                    (Some(u), _) if p.append_v1 => with_v1(&u),
                    (Some(u), _) => u.trim_end_matches('/').to_owned(),
                    (None, "openai") => "https://api.openai.com/v1".to_owned(),
                    (None, _) => {
                        return Err(config(format!(
                            "{} needs a base URL: set {} (e.g. http://localhost:8000/v1)",
                            p.name, p.url_env
                        )));
                    }
                };
                let keyless = p.key_optional || (p.name == "openai_compatible" && is_local(&url));
                if key.is_none() && !keyless {
                    return Err(config(format!(
                        "{} needs an API key: set {}",
                        p.name, p.key_env
                    )));
                }
                ep.key = key;
                ep.model = match model {
                    Some(m) => m,
                    None if p.name == "openai" => OPENAI_DEFAULT_MODEL.to_owned(),
                    None => {
                        return Err(config(format!(
                            "{} needs a model: pass one or set {}",
                            p.name, p.model_env
                        )));
                    }
                };
                ep.base_url = url;
            }
            Kind::Bedrock => {
                let key = key.or_else(|| env.get("AWS_BEARER_TOKEN_BEDROCK").map(Secret::new));
                ep.key = Some(key.ok_or_else(|| {
                    config(
                        "bedrock needs an API key (bearer token): set AI_BEDROCK_BEARER_TOKEN"
                            .to_owned(),
                    )
                })?);
                let region = req
                    .region
                    .clone()
                    .or_else(|| env.get(p.url_env))
                    .or_else(|| env.get("AWS_REGION"))
                    .or_else(|| env.get("AWS_DEFAULT_REGION"))
                    .ok_or_else(|| {
                        config("bedrock needs a region: set AI_BEDROCK_REGION".to_owned())
                    })?;
                if !region
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    return Err(config(format!("bedrock region {region:?} is not a region")));
                }
                ep.model = model.ok_or_else(|| {
                    config("bedrock needs a model id: pass one or set AI_BEDROCK_MODEL".to_owned())
                })?;
                ep.base_url = req.base_url.clone().map_or_else(
                    || format!("https://bedrock-runtime.{region}.amazonaws.com"),
                    |u| u.trim_end_matches('/').to_owned(),
                );
                ep.region = Some(region);
            }
        }
        if ep.model.trim().is_empty() {
            return Err(config(format!("{}: empty model id", p.name)));
        }
        Ok(ep)
    }

    /// The URL a call goes to.
    pub fn url(&self) -> String {
        match self.kind {
            Kind::OpenAi => format!("{}/chat/completions", self.base_url),
            Kind::Anthropic => format!("{}/v1/messages", self.base_url),
            Kind::Bedrock => format!("{}/model/{}/converse", self.base_url, encode(&self.model)),
        }
    }

    /// The host of [`Self::base_url`].
    pub fn host(&self) -> &str {
        host_of(&self.base_url)
    }

    /// `provider=… model=… host=…`: what a log line may say (never the key).
    pub fn describe(&self) -> String {
        format!(
            "provider={} model={} host={}",
            self.preset,
            self.model,
            self.host()
        )
    }
}

/// Percent-encodes a path segment (Bedrock model ids hold `:`).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Presets that have credentials in `env` (names only): a key, or for `vllm` its URL.
pub fn available(env: &Env) -> Vec<&'static str> {
    let mut out = Vec::new();
    for name in [
        "openai",
        "anthropic",
        "bedrock",
        "openrouter",
        "groq",
        "vllm",
    ] {
        let Ok(p) = find(name) else { continue };
        let ok = match name {
            "vllm" => env.get(p.url_env).is_some(),
            "bedrock" => {
                env.get(p.key_env).is_some() || env.get("AWS_BEARER_TOKEN_BEDROCK").is_some()
            }
            _ => env.get(p.key_env).is_some(),
        };
        if ok {
            out.push(name);
        }
    }
    if env.get("OPENAI_BASE_URL").is_some() && out.contains(&"openai") {
        out.push("openai_compatible");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_map(pairs.iter().map(|&(k, v)| (k, v)))
    }

    fn resolve(provider: Option<&str>, pairs: &[(&str, &str)]) -> Result<Endpoint> {
        let req = Connect {
            provider: provider.map(str::to_owned),
            ..Connect::default()
        };
        Endpoint::resolve(&req, &env(pairs))
    }

    #[test]
    fn anthropic_defaults_to_haiku_and_never_shows_the_key() {
        let e = resolve(Some("claude"), &[("ANTHROPIC_API_KEY", "sk-ant-XYZ")]).unwrap();
        assert_eq!((e.preset, e.kind), ("anthropic", Kind::Anthropic));
        assert_eq!(e.model, "claude-haiku-5-5");
        assert_eq!(e.url(), "https://api.anthropic.com/v1/messages");
        assert_eq!(
            e.describe(),
            "provider=anthropic model=claude-haiku-5-5 host=api.anthropic.com"
        );
        let dbg = format!("{e:?}");
        assert!(!dbg.contains("XYZ") && dbg.contains("Secret(***)"), "{dbg}");
        let e = resolve(
            None,
            &[
                ("AI_PROVIDER", "anthropic"),
                ("ANTHROPIC_API_KEY", "k"),
                ("ANTHROPIC_MODEL", "claude-sonnet-5-5"),
            ],
        )
        .unwrap();
        assert_eq!(e.model, "claude-sonnet-5-5");
    }

    #[test]
    fn argument_beats_environment_beats_default() {
        let pairs = [("OPENAI_API_KEY", "k"), ("OPENAI_MODEL", "gpt-env")];
        assert_eq!(resolve(Some("openai"), &pairs).unwrap().model, "gpt-env");
        let req = Connect {
            provider: Some("openai".into()),
            model: Some("gpt-arg".into()),
            ..Connect::default()
        };
        let e = Endpoint::resolve(&req, &env(&pairs)).unwrap();
        assert_eq!(e.model, "gpt-arg");
        let e = resolve(Some("openai"), &[("OPENAI_API_KEY", "k")]).unwrap();
        assert_eq!(e.model, "gpt-4o-mini");
        assert_eq!(e.url(), "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn ai_chat_model_only_for_the_same_provider() {
        let e = resolve(
            Some("anthropic"),
            &[
                ("AI_PROVIDER", "bedrock"),
                ("AI_CHAT_MODEL", "qwen.qwen3-x:0"),
                ("ANTHROPIC_API_KEY", "k"),
            ],
        )
        .unwrap();
        assert_eq!(e.model, "claude-haiku-5-5");
        let e = resolve(
            None,
            &[
                ("AI_PROVIDER", "bedrock"),
                ("AI_CHAT_MODEL", "qwen.qwen3-235b-a22b-2507-v1:0"),
                ("AI_BEDROCK_BEARER_TOKEN", "tok"),
                ("AI_BEDROCK_REGION", "eu-west-2"),
            ],
        )
        .unwrap();
        assert_eq!(
            e.url(),
            "https://bedrock-runtime.eu-west-2.amazonaws.com/model/qwen.qwen3-235b-a22b-2507-v1%3A0/converse"
        );
    }

    #[test]
    fn presets_and_clear_errors() {
        let e = resolve(
            Some("vllm"),
            &[
                ("API_URL", "http://gpu:8000"),
                ("API_MODEL", "Qwen/Qwen3-32B"),
            ],
        )
        .unwrap();
        assert_eq!(e.base_url, "http://gpu:8000/v1");
        assert!(e.key.is_none());
        assert_eq!(e.thinking, ThinkingOff::Template);
        let e = resolve(Some("ollama"), &[("API_MODEL", "qwen3:4b")]).unwrap();
        assert_eq!(e.url(), "http://localhost:11434/v1/chat/completions");
        let e = resolve(
            Some("openrouter"),
            &[
                ("OPENROUTER_API_KEY", "k"),
                ("OPENROUTER_MODEL", "qwen/qwen3"),
            ],
        )
        .unwrap();
        assert_eq!(e.thinking, ThinkingOff::OpenRouter);
        assert_eq!(e.host(), "openrouter.ai");
        let e = resolve(
            Some("openai_compatible"),
            &[
                ("OPENAI_BASE_URL", "http://localhost:8080/v1/"),
                ("OPENAI_MODEL", "m"),
            ],
        )
        .unwrap();
        assert!(e.key.is_none(), "local servers need no key");
        let err =
            |p: &str, pairs: &[(&str, &str)]| resolve(Some(p), pairs).unwrap_err().to_string();
        assert!(err("anthropic", &[]).contains("ANTHROPIC_API_KEY"));
        assert!(err("groq", &[("GROQ_API_KEY", "k")]).contains("GROQ_MODEL"));
        assert!(err("openai_compatible", &[("OPENAI_MODEL", "m")]).contains("OPENAI_BASE_URL"));
        assert!(
            err(
                "openai_compatible",
                &[
                    ("OPENAI_BASE_URL", "https://gw.example/v1"),
                    ("OPENAI_MODEL", "m")
                ]
            )
            .contains("OPENAI_API_KEY")
        );
        assert!(
            err(
                "bedrock",
                &[("AI_BEDROCK_BEARER_TOKEN", "t"), ("AI_BEDROCK_MODEL", "m")]
            )
            .contains("AI_BEDROCK_REGION")
        );
        assert!(err("nope", &[]).contains("unknown LLM provider"));
        assert!(err("vllm", &[("API_MODEL", "m")]).contains("API_URL"));
    }

    #[test]
    fn auto_detects_a_single_configured_provider() {
        let e = resolve(None, &[("GROQ_API_KEY", "k"), ("GROQ_MODEL", "llama")]).unwrap();
        assert_eq!(e.preset, "groq");
        let msg = resolve(
            None,
            &[
                ("GROQ_API_KEY", "gsk-SECRET"),
                ("OPENAI_API_KEY", "sk-SECRET"),
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(msg.contains("credentials found for: openai, groq"), "{msg}");
        assert!(!msg.contains("SECRET"), "{msg}");
    }

    #[test]
    fn concurrency_from_the_environment() {
        let e = resolve(
            Some("ollama"),
            &[("API_MODEL", "m"), ("AI_MAX_CONCURRENT_LLM", "2")],
        )
        .unwrap();
        assert_eq!(e.max_concurrent, 2);
        let e = resolve(
            Some("ollama"),
            &[("API_MODEL", "m"), ("AI_MAX_CONCURRENT_LLM", "x")],
        )
        .unwrap();
        assert_eq!(e.max_concurrent, DEFAULT_MAX_CONCURRENT);
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("http://u:p@host:8000/v1?q"), "host:8000");
        assert_eq!(host_of("https://api.anthropic.com"), "api.anthropic.com");
        assert!(is_local("http://[::1]:8000/v1"));
        assert!(is_local("http://box.local/v1"));
        assert!(!is_local("https://gw.example/v1"));
    }
}

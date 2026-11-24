//! Settings from the environment and from a `.env` file. Values are never logged.
//!
//! [`Env`] looks a name up in the process environment first, then in a `.env` file the caller
//! names explicitly ([`Env::with_file`]); nothing is loaded from the working directory on its
//! own. [`Env::from_map`] reads only a given map (tests, programs with their own config).

use std::collections::HashMap;
use std::path::Path;

use quail_core::{Error, Result};

/// `KEY=VALUE` lines: `#` comments, an optional `export ` prefix, single or double quotes, a
/// ` #` comment after an unquoted value. Empty values are left out (an empty
/// `OPENAI_API_KEY=` in a template means "not set"). The same rules as hedgehog's `parse_env`.
pub fn parse_env(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        if let Some(rest) = s.strip_prefix("export ") {
            s = rest.trim_start();
        }
        let Some((key, value)) = s.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
            continue;
        }
        let value = value.trim();
        let value = match value.chars().next() {
            Some(q @ ('\'' | '"')) => {
                let inner = &value[1..];
                let inner = inner.find(q).map_or(inner, |end| &inner[..end]);
                if q == '"' {
                    inner.replace("\\n", "\n").replace("\\\"", "\"")
                } else {
                    inner.to_owned()
                }
            }
            _ => value
                .find(" #")
                .map_or(value, |at| &value[..at])
                .trim()
                .to_owned(),
        };
        if !value.is_empty() {
            out.insert(key.to_owned(), value);
        }
    }
    out
}

/// Where settings are looked up: the process environment (unless [`Env::from_map`]), then the
/// values of a `.env` file or map. Blank values count as unset.
#[derive(Clone, Default)]
pub struct Env {
    extra: HashMap<String, String>,
    use_process: bool,
}

impl std::fmt::Debug for Env {
    /// Names only, never values.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.extra.keys().collect();
        names.sort();
        f.debug_struct("Env")
            .field("use_process", &self.use_process)
            .field("names", &names)
            .finish()
    }
}

impl Env {
    /// The process environment only.
    pub fn process() -> Self {
        Self {
            extra: HashMap::new(),
            use_process: true,
        }
    }

    /// The process environment, then the `.env` file at `path` (which must exist).
    pub fn with_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Resource(format!("env file {}: {e}", path.display())))?;
        Ok(Self {
            extra: parse_env(&text),
            use_process: true,
        })
    }

    /// Only `map`: the process environment is not read.
    pub fn from_map<K: Into<String>, V: Into<String>>(
        map: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self {
            extra: map.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            use_process: false,
        }
    }

    /// The value of `name`, trimmed; `None` when unset or blank.
    pub fn get(&self, name: &str) -> Option<String> {
        if name.is_empty() {
            return None;
        }
        if self.use_process
            && let Ok(v) = std::env::var(name)
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_owned());
        }
        self.extra
            .get(name)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_hedgehog() {
        let m = parse_env(
            "# comment\n\
             AI_PROVIDER=anthropic\n\
             export OPENAI_MODEL = gpt-4o-mini  # cheap\n\
             EMPTY=\n\
             QUOTED=\"a b #c\\nd\"\n\
             SINGLE='x # y'\n\
             bad key=1\n\
             NOEQUALS\n",
        );
        assert_eq!(m.get("AI_PROVIDER").map(String::as_str), Some("anthropic"));
        assert_eq!(
            m.get("OPENAI_MODEL").map(String::as_str),
            Some("gpt-4o-mini")
        );
        assert_eq!(m.get("QUOTED").map(String::as_str), Some("a b #c\nd"));
        assert_eq!(m.get("SINGLE").map(String::as_str), Some("x # y"));
        assert!(!m.contains_key("EMPTY"));
        assert!(!m.contains_key("bad key"));
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn a_map_hides_the_process_environment_and_debug_hides_values() {
        // PATH is set in every test environment; a map-only Env must not see it.
        let e = Env::from_map([("ANTHROPIC_API_KEY", "sk-ant-secret"), ("BLANK", "  ")]);
        assert_eq!(e.get("PATH"), None);
        assert_eq!(e.get("BLANK"), None);
        assert_eq!(e.get("ANTHROPIC_API_KEY").as_deref(), Some("sk-ant-secret"));
        let dbg = format!("{e:?}");
        assert!(
            dbg.contains("ANTHROPIC_API_KEY") && !dbg.contains("secret"),
            "{dbg}"
        );
    }
}

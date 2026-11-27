//! Small text helpers.

use std::collections::HashSet;

pub(crate) use quail_ooxml::{Notes, clean};

/// Most distinct names kept in a list fact.
pub(crate) const MAX_LIST: usize = 1000;

/// Distinct values in first-seen order, compared case-insensitively, first spelling kept.
#[derive(Debug, Default)]
pub(crate) struct Distinct {
    seen: HashSet<String>,
    list: Vec<String>,
}

impl Distinct {
    /// Adds a value (cleaned; empty values are ignored).
    pub fn add(&mut self, v: &str) {
        if self.list.len() >= MAX_LIST {
            return;
        }
        let v = clean(v);
        if v.is_empty() {
            return;
        }
        if self.seen.insert(v.to_lowercase()) {
            self.list.push(v);
        }
    }

    /// The values.
    pub fn into_vec(self) -> Vec<String> {
        self.list
    }

    /// Whether nothing was added.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// Lowercase host of an absolute `http(s)`/`ftp(s)` URL, or the domain of a `mailto:`
/// address. `None` for anything else (relative links, `file:` paths, bookmarks).
pub(crate) fn link_domain(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.get(..7).map(str::to_ascii_lowercase);
    if lower.as_deref() == Some("mailto:") {
        let addr = url.get(7..)?;
        let addr = addr.split(['?', ',', ';']).next()?;
        let (_, domain) = addr.rsplit_once('@')?;
        return host(domain);
    }
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps"
    ) {
        return None;
    }
    let authority = rest.split(['/', '?', '#', '\\']).next()?;
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let h = if let Some(v6) = hostport.strip_prefix('[') {
        return v6.split(']').next().and_then(|h| {
            (!h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit() || c == ':'))
                .then(|| h.to_ascii_lowercase())
        });
    } else {
        hostport.split(':').next()?
    };
    host(h)
}

fn host(h: &str) -> Option<String> {
    let h = percent_lower(h.trim().trim_end_matches('.'));
    let ok = !h.is_empty()
        && h.len() <= 253
        && h.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '.' || c == '_');
    ok.then_some(h)
}

fn percent_lower(h: &str) -> String {
    h.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains() {
        assert_eq!(
            link_domain("https://User@Example.COM:8080/x?y").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            link_domain("mailto:a.b@Mail.Example.org?subject=x").as_deref(),
            Some("mail.example.org")
        );
        assert_eq!(link_domain("http://[::1]/").as_deref(), Some("::1"));
        assert_eq!(link_domain("file:///C:/x.docx"), None);
        assert_eq!(link_domain("#bookmark"), None);
        assert_eq!(link_domain("http:///nohost"), None);
        assert_eq!(link_domain("http://bad host/"), None);
    }

    #[test]
    fn cleaning() {
        let mut d = Distinct::default();
        d.add("Ann");
        d.add("ann ");
        d.add("Bob");
        assert_eq!(d.into_vec(), vec!["Ann", "Bob"]);
    }
}

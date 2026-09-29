//! Sensitive paths: files Workbench does not show, serve or search unless the user
//! explicitly asks (`allowSensitive=true`). Patterns come from `project.sensitive`
//! (gitignore-like globs) plus a small built-in list of credential files that often
//! sit next to code or in `~/.claude`.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Always sensitive (unless listed in `DEFAULT_EXCEPTIONS`).
const DEFAULT_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519",
    ".netrc",
    ".pgpass",
    ".credentials.json",
    // JetBrains HTTP Client: the secret half of its variables.
    "http-client.private.env.json",
    ".gitlab_token",
    ".atlassian_token",
    // Key files named after what they hold (suffix only, so `api_key.rs` stays visible).
    "*_api_key",
    "*_api_sk",
    "*.api_key",
    "*.api_sk",
    "*_token",
    "*.token",
];

/// Templates that match a default pattern but hold no secrets.
const DEFAULT_EXCEPTIONS: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.template",
    ".env.dist",
    ".env.defaults",
    "*.example",
    "*.sample",
    "*.template",
];

pub struct Sensitive {
    /// Explicit project patterns (never overridden by the exceptions).
    project: GlobSet,
    defaults: GlobSet,
    exceptions: GlobSet,
}

impl Sensitive {
    /// Built-in patterns plus `patterns` (from `project.sensitive`).
    pub fn new(patterns: &[String]) -> Self {
        Self {
            project: build(patterns.iter().map(String::as_str)),
            defaults: build(DEFAULT_PATTERNS.iter().copied()),
            exceptions: build(DEFAULT_EXCEPTIONS.iter().copied()),
        }
    }

    /// Only the built-in patterns (paths outside any project).
    pub fn defaults() -> Self {
        Self::new(&[])
    }

    /// Whether `rel` (relative to the project or allowed root, `/`-separated) is sensitive.
    pub fn matches(&self, rel: &str) -> bool {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            return false;
        }
        self.project.is_match(rel) || (self.defaults.is_match(rel) && !self.exceptions.is_match(rel))
    }
}

/// Gitignore-flavoured expansion: a pattern without a `/` matches at any depth;
/// a leading `/` (or an inner `/`) anchors it at the root; a match on a directory
/// covers everything below it.
fn expand(pattern: &str) -> Vec<String> {
    let p = pattern.trim();
    if p.is_empty() || p.starts_with('#') {
        return vec![];
    }
    let anchored = p.starts_with('/') || p.trim_end_matches('/').contains('/');
    let base = p.trim_start_matches('/').trim_end_matches('/');
    if base.is_empty() {
        return vec![];
    }
    if anchored {
        vec![base.to_string(), format!("{base}/**")]
    } else {
        vec![base.to_string(), format!("**/{base}"), format!("{base}/**"), format!("**/{base}/**")]
    }
}

fn build<'a>(patterns: impl Iterator<Item = &'a str>) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        for g in expand(p) {
            match GlobBuilder::new(&g).literal_separator(true).case_insensitive(crate::util::os::path::CASE_INSENSITIVE).build() {
                Ok(glob) => {
                    b.add(glob);
                }
                Err(e) => tracing::warn!("ignoring bad sensitive pattern {p:?}: {e}"),
            }
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_credentials_but_not_templates() {
        let s = Sensitive::defaults();
        assert!(s.matches(".env"));
        assert!(s.matches("app/.env.production"));
        assert!(s.matches("certs/server.pem"));
        assert!(s.matches(".credentials.json"));
        assert!(!s.matches(".env.example"));
        assert!(!s.matches("app/.env.sample"));
        assert!(!s.matches("src/env.rs"));
        assert!(!s.matches("keyboard.rs"));
        assert!(s.matches("~.anthropic_api_sk"));
        assert!(s.matches("deploy/ci_token"));
        assert!(!s.matches("src/api_key.rs"));
        assert!(!s.matches("src/tokens.rs"));
        assert!(!s.matches(""));
    }

    #[test]
    fn project_patterns_are_gitignore_like() {
        let s = Sensitive::new(&["outreach/".into(), "/secrets.toml".into(), "*.csv".into(), "data/pii/*.json".into()]);
        assert!(s.matches("outreach/leads.xlsx"));
        assert!(s.matches("sub/outreach/x"));
        assert!(s.matches("outreach"));
        assert!(s.matches("secrets.toml"));
        assert!(!s.matches("config/secrets.toml"));
        assert!(s.matches("a/b/c.csv"));
        assert!(s.matches("data/pii/people.json"));
        assert!(!s.matches("other/data/pii/people.json"));
        assert!(!s.matches("src/main.rs"));
    }

    #[test]
    fn explicit_project_patterns_beat_exceptions() {
        let s = Sensitive::new(&[".env.example".into()]);
        assert!(s.matches(".env.example"));
    }

    #[test]
    fn bad_patterns_are_skipped() {
        let s = Sensitive::new(&["[".into(), "ok.txt".into()]);
        assert!(s.matches("ok.txt"));
    }
}

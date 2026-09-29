//! Secret resolution. Config files hold *references* (`{ file = "~/.gitlab_token" }`);
//! values are read lazily, kept only in memory (briefly cached), and never sent to
//! the browser or put in a child's argv.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use crate::config::{SecretRef, expand_tilde};
use crate::error::ApiError;
use crate::util::os::perm::{self, Privacy};

const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Secret(Arc<SecretString>);

impl Secret {
    /// The value. Use only to build an outgoing request header or a child env var.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    /// Wrap a value that must be treated as secret (masked in terminal output), e.g. a
    /// host environment variable a dev container config passes into a container.
    pub fn from_value(value: String) -> Self {
        Secret(Arc::new(SecretString::from(value)))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(••••)")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub name: String,
    /// "file" | "env" | "keyring" | "dotenv" | "command"
    pub source: String,
    /// Where it lives (path, variable name, keyring entry) — never the value.
    pub location: String,
    pub resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub warnings: Vec<String>,
    /// Project the reference is defined in; `None` = global config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Default)]
pub struct SecretStore {
    cache: Mutex<HashMap<String, (Instant, Secret)>>,
}

impl SecretStore {
    /// Resolve `r` (cached for a minute under `cache_key`).
    pub fn resolve_ref(&self, cache_key: &str, r: &SecretRef) -> Result<Secret, ApiError> {
        if let Some((at, s)) = self.cache.lock().get(cache_key) {
            if at.elapsed() < CACHE_TTL {
                return Ok(s.clone());
            }
        }
        let mut warnings = vec![];
        let value = resolve(r, &mut warnings).map_err(|e| ApiError::not_configured(format!("secret {cache_key:?}: {e}")))?;
        let s = Secret(Arc::new(value));
        self.cache.lock().insert(cache_key.to_string(), (Instant::now(), s.clone()));
        Ok(s)
    }

    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// Describe a reference without exposing its value.
    pub fn status(&self, name: &str, r: &SecretRef, project_id: Option<&str>) -> SecretStatus {
        let (source, location) = describe(r);
        let mut warnings = vec![];
        let result = resolve(r, &mut warnings);
        SecretStatus {
            name: name.to_string(),
            source: source.into(),
            location,
            resolved: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
            warnings,
            project_id: project_id.map(str::to_string),
        }
    }
}

fn describe(r: &SecretRef) -> (&'static str, String) {
    match r {
        SecretRef::File(p) => ("file", p.clone()),
        SecretRef::Env(k) => ("env", k.clone()),
        SecretRef::Keyring(k) => ("keyring", k.clone()),
        SecretRef::Dotenv { path, key } => ("dotenv", format!("{path}#{key}")),
        SecretRef::Command(argv) => ("command", argv.first().cloned().unwrap_or_default()),
    }
}

/// Read a reference. Never logs the value.
pub fn resolve(r: &SecretRef, warnings: &mut Vec<String>) -> anyhow::Result<SecretString> {
    let v = match r {
        SecretRef::File(p) => {
            let path = expand_tilde(p);
            if let Privacy::Exposed(why) = perm::privacy(&path)? {
                warnings.push(format!("{p} is readable by other users ({why}); {}", perm::MAKE_PRIVATE));
            }
            std::fs::read_to_string(&path)?.trim().to_string()
        }
        SecretRef::Env(k) => std::env::var(k).map_err(|_| anyhow::anyhow!("environment variable {k} is not set"))?,
        SecretRef::Keyring(sa) => {
            let (service, account) =
                sa.split_once('/').ok_or_else(|| anyhow::anyhow!("keyring reference must be service/account"))?;
            keyring::Entry::new(service, account)?.get_password()?
        }
        SecretRef::Dotenv { path, key } => {
            let text = std::fs::read_to_string(expand_tilde(path))?;
            text.lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim().trim_start_matches("export ").trim() == key)
                .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_string())
                .ok_or_else(|| anyhow::anyhow!("{key} not found in {path}"))?
        }
        SecretRef::Command(argv) => {
            anyhow::ensure!(!argv.is_empty(), "empty command");
            let out = crate::util::os::exe::configured(argv)?.output()?;
            anyhow::ensure!(out.status.success(), "secret command exited with {}", out.status);
            String::from_utf8(out.stdout)?.trim().to_string()
        }
    };
    anyhow::ensure!(!v.is_empty(), "secret is empty");
    Ok(SecretString::from(v))
}

/// Replace known secret values in text bound for the browser (defence in depth).
pub fn redact(text: &str, secrets: &[Secret]) -> String {
    let mut s = text.to_string();
    for sec in secrets {
        let v = sec.expose();
        if v.len() >= 8 && s.contains(v) {
            s = s.replace(v, "••••••");
        }
    }
    s
}

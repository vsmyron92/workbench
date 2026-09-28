//! Pure pieces of environment checks: auth path matching, health evaluation,
//! deployed-version extraction and frame-embedding header analysis.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::config::project::BasicAuth;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Up,
    Down,
    Degraded,
    #[default]
    Unknown,
}

/// Caddy-style path pattern: `/api/*` (prefix), `*.json` (suffix), `*` (all) or exact.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match (pattern.strip_suffix('*'), pattern.strip_prefix('*')) {
        (Some(prefix), _) => path.starts_with(prefix) || path == prefix.trim_end_matches('/'),
        (None, Some(suffix)) => path.ends_with(suffix),
        (None, None) => path == pattern,
    }
}

/// Whether a request to `path` must carry the env's basic auth.
pub fn needs_auth(auth: &BasicAuth, path: &str) -> bool {
    !auth.except.iter().any(|p| path_matches(p, path))
}

/// Status words a JSON health body may report without a configured pointer.
const BAD_STATUS: &[&str] = &["error", "down", "fail", "failed", "failing", "unhealthy", "degraded", "critical", "red"];

/// Judge a health response.
/// * wrong HTTP status: 5xx (or no response) → down; anything else → degraded;
/// * `json_pointer` set: the value must equal `equals` (or be truthy when `equals` is unset);
/// * no pointer: a JSON body whose top-level `status` is a known bad word → degraded.
pub fn evaluate(
    expect: u16,
    status: u16,
    body: Option<&Value>,
    pointer: Option<&str>,
    equals: Option<&str>,
) -> (HealthStatus, Option<String>) {
    if status != expect {
        let s = if status >= 500 { HealthStatus::Down } else { HealthStatus::Degraded };
        return (s, Some(format!("HTTP {status}, expected {expect}")));
    }
    match pointer {
        Some(ptr) => {
            let Some(body) = body else {
                return (HealthStatus::Degraded, Some(format!("expected JSON with {ptr}")));
            };
            match body.pointer(ptr) {
                None => (HealthStatus::Degraded, Some(format!("{ptr} missing from the response"))),
                Some(v) => {
                    let got = value_string(v);
                    match equals {
                        Some(want) if got != want => (HealthStatus::Degraded, Some(format!("{ptr} is {got:?}, expected {want:?}"))),
                        None if matches!(v, Value::Null | Value::Bool(false)) => {
                            (HealthStatus::Degraded, Some(format!("{ptr} is {got}")))
                        }
                        _ => (HealthStatus::Up, None),
                    }
                }
            }
        }
        None => {
            if let Some(s) = body.and_then(|b| b.get("status")).and_then(Value::as_str) {
                if BAD_STATUS.contains(&s.to_ascii_lowercase().as_str()) {
                    return (HealthStatus::Degraded, Some(format!("status is {s:?}")));
                }
            }
            (HealthStatus::Up, None)
        }
    }
}

/// A JSON value as the string a config compares against (`"ok"` → `ok`, `1` → `1`).
pub fn value_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

static SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{7,40}\b").unwrap());

/// The commit sha in version text: the `pattern`'s `sha` group (or group 1, or the
/// whole match); without a pattern the first 7–40 hex-digit word.
pub fn extract_sha(text: &str, pattern: Option<&Regex>) -> Option<String> {
    match pattern {
        Some(re) => {
            let c = re.captures(text)?;
            let m = c.name("sha").or_else(|| c.get(1)).or_else(|| c.get(0))?;
            Some(m.as_str().to_string())
        }
        None => SHA.find(text).map(|m| m.as_str().to_string()),
    }
}

/// Whether two shas name the same commit (one may be abbreviated).
pub fn same_commit(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    let n = a.len().min(b.len());
    n >= 7 && a[..n] == b[..n]
}

/// Whether response headers forbid showing the page in a Workbench iframe.
pub fn frame_blocked(xfo: Option<&str>, csp: Option<&str>) -> bool {
    if let Some(x) = xfo {
        let x = x.trim().to_ascii_lowercase();
        if x == "deny" || x == "sameorigin" || x.starts_with("allow-from") {
            return true;
        }
    }
    if let Some(csp) = csp {
        for d in csp.split(';') {
            let d = d.trim().to_ascii_lowercase();
            if let Some(v) = d.strip_prefix("frame-ancestors") {
                let sources: Vec<&str> = v.split_whitespace().collect();
                return !sources.contains(&"*");
            }
        }
    }
    false
}

/// A CSP with its `frame-ancestors` directive removed (`None` if nothing is left).
pub fn strip_frame_ancestors(csp: &str) -> Option<String> {
    let kept: Vec<&str> = csp
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.to_ascii_lowercase().starts_with("frame-ancestors"))
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn caddy_style_paths() {
        assert!(path_matches("/api/*", "/api/health"));
        assert!(path_matches("/api/*", "/api"));
        assert!(!path_matches("/api/*", "/apiary"));
        assert!(!path_matches("/api/*", "/"));
        assert!(path_matches("*.json", "/x/manifest.json"));
        assert!(path_matches("/health", "/health"));
        assert!(!path_matches("/health", "/healthz"));
        let auth = BasicAuth { user: "staging".into(), password: "pw".into(), except: vec!["/api/*".into()] };
        assert!(!needs_auth(&auth, "/api/health"));
        assert!(needs_auth(&auth, "/"));
        assert!(needs_auth(&auth, "/assets/index.js"));
    }

    #[test]
    fn health_with_pointer() {
        let ok = json!({"status": "ok"});
        assert_eq!(evaluate(200, 200, Some(&ok), Some("/status"), Some("ok")).0, HealthStatus::Up);
        let bad = json!({"status": "starting"});
        let (s, e) = evaluate(200, 200, Some(&bad), Some("/status"), Some("ok"));
        assert_eq!(s, HealthStatus::Degraded);
        assert!(e.unwrap().contains("starting"));
        assert_eq!(evaluate(200, 200, None, Some("/status"), Some("ok")).0, HealthStatus::Degraded);
        assert_eq!(evaluate(200, 200, Some(&json!({"db": true})), Some("/db"), None).0, HealthStatus::Up);
        assert_eq!(evaluate(200, 200, Some(&json!({"db": false})), Some("/db"), None).0, HealthStatus::Degraded);
        assert_eq!(evaluate(200, 200, Some(&json!({"n": 1})), Some("/n"), Some("1")).0, HealthStatus::Up);
    }

    #[test]
    fn health_status_codes_and_bodies() {
        assert_eq!(evaluate(200, 502, None, None, None).0, HealthStatus::Down);
        assert_eq!(evaluate(200, 503, None, None, None).0, HealthStatus::Down);
        assert_eq!(evaluate(200, 401, None, None, None).0, HealthStatus::Degraded);
        assert_eq!(evaluate(204, 204, None, None, None).0, HealthStatus::Up);
        assert_eq!(evaluate(200, 200, Some(&json!({"status": "ok"})), None, None).0, HealthStatus::Up);
        assert_eq!(evaluate(200, 200, Some(&json!({"status": "DOWN"})), None, None).0, HealthStatus::Degraded);
        assert_eq!(evaluate(200, 200, Some(&json!([1, 2])), None, None).0, HealthStatus::Up);
    }

    #[test]
    fn version_extraction() {
        let re = Regex::new(r":(?P<sha>[0-9a-f]{8})\b").unwrap();
        let tags = "registry.gitlab.com/acme/shop:latest registry.gitlab.com/acme/shop:5babfd54";
        assert_eq!(extract_sha(tags, Some(&re)).as_deref(), Some("5babfd54"));
        assert_eq!(extract_sha("{\"version\":\"4b8e2508c0ffee\"}", None).as_deref(), Some("4b8e2508c0ffee"));
        assert_eq!(extract_sha("no version here", None), None);
        assert!(same_commit("4b8e2508", "4b8e2508c0ffee1234"));
        assert!(!same_commit("4b8e2508", "5babfd54"));
        assert!(!same_commit("4b8", "4b8e2508"));
    }

    #[test]
    fn frame_headers() {
        assert!(frame_blocked(Some("DENY"), None));
        assert!(frame_blocked(Some("SAMEORIGIN"), None));
        assert!(frame_blocked(None, Some("default-src 'self'; frame-ancestors 'self'")));
        assert!(!frame_blocked(None, Some("default-src 'self'; frame-ancestors *")));
        assert!(!frame_blocked(None, Some("default-src 'self'")));
        assert!(!frame_blocked(None, None));
        assert_eq!(strip_frame_ancestors("default-src 'self'; frame-ancestors 'none'").as_deref(), Some("default-src 'self'"));
        assert_eq!(strip_frame_ancestors("frame-ancestors 'none'"), None);
    }
}

//! HTTP plumbing for Atlassian Cloud: which site and credentials a request uses,
//! Basic auth, `429 Retry-After` back-off, pagination links and error mapping.
//!
//! Credentials come from `[atlassian]` in config.toml (site, email, token = the name
//! of a `[secrets]` entry). A project's `[links.confluence]` / `[links.jira]` may
//! override site, email and token. The token secret holds either a bare API token
//! or `email:token`.

use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine;
use reqwest::Method;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::error::ApiError;
use crate::secrets::Secret;

/// Upstream JSON bodies larger than this are refused (defence against runaway responses).
const MAX_JSON_BYTES: u64 = 48 * 1024 * 1024;
/// Error bodies are read up to this size to extract a message.
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_RETRIES: u32 = 3;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Product {
    Confluence,
    Jira,
}

impl Product {
    fn name(self) -> &'static str {
        match self {
            Product::Confluence => "Confluence",
            Product::Jira => "Jira",
        }
    }
}

/// One Atlassian site plus the credentials used for it.
#[derive(Clone)]
pub struct Site {
    /// `https://x.atlassian.net`: no trailing slash, no `/wiki`, no userinfo.
    pub base: String,
    pub email: String,
    auth: HeaderValue,
    token: Secret,
    /// A short one-way digest of the credentials, so cached checks (status, "auth known
    /// bad") belong to the token they were made with: fixing the token takes effect at
    /// once. Kept in memory only, never output.
    cred_digest: String,
}

impl std::fmt::Debug for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Site").field("base", &self.base).field("email", &self.email).finish()
    }
}

impl Site {
    /// `raw_token` is the secret's value: a bare API token or `email:token`.
    pub fn new(base: &str, email: &str, secret: Secret) -> Result<Site, ApiError> {
        let base = normalize_site(base)?;
        let raw = secret.expose();
        let (email, token) = match raw.split_once(':') {
            Some((e, t)) if e.contains('@') && !t.trim().is_empty() => (e.trim().to_string(), t.trim().to_string()),
            _ => (email.trim().to_string(), raw.trim().to_string()),
        };
        if email.is_empty() {
            return Err(ApiError::not_configured(
                "Atlassian needs the account email: set [atlassian] email = \"you@example.com\" in config.toml \
                 (or store the token secret as email:token)",
            ));
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"));
        let mut auth = HeaderValue::from_str(&format!("Basic {b64}"))
            .map_err(|_| ApiError::not_configured("the Atlassian token contains characters that cannot be sent"))?;
        auth.set_sensitive(true);
        let cred_digest = hex::encode(&Sha256::digest(auth.as_bytes())[..8]);
        Ok(Site { base, email, auth, token: secret, cred_digest })
    }

    /// Cache key: one entry per site, account and credentials.
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.base, self.email, self.cred_digest)
    }

    pub fn host(&self) -> &str {
        self.base.split_once("://").map(|(_, h)| h).unwrap_or(&self.base)
    }

    /// Remove the token from text that might reach the browser (defence in depth).
    pub fn redact(&self, text: &str) -> String {
        crate::secrets::redact(text, std::slice::from_ref(&self.token))
    }
}

/// `https://x.atlassian.net/wiki/` → `https://x.atlassian.net`. Rejects anything that
/// is not a bare http(s) origin (paths, userinfo, query strings).
pub fn normalize_site(s: &str) -> Result<String, ApiError> {
    let s = s.trim().trim_end_matches('/');
    let s = s.strip_suffix("/wiki").unwrap_or(s).trim_end_matches('/');
    let bad = || {
        ApiError::not_configured(format!(
            "[atlassian] site must look like https://<your-site>.atlassian.net (got {s:?})"
        ))
    };
    let (scheme, rest) = s.split_once("://").ok_or_else(bad)?;
    let scheme = scheme.to_ascii_lowercase();
    if (scheme != "https" && scheme != "http") || rest.is_empty() || rest.contains(['@', '/', '?', '#', ' ']) {
        return Err(bad());
    }
    Ok(format!("{scheme}://{}", rest.to_ascii_lowercase()))
}

const SETUP_HELP: &str = "Add to config.toml:\n\n[atlassian]\nsite = \"https://<your-site>.atlassian.net\"\nemail = \"you@example.com\"\ntoken = \"atlassian\"\n\n[secrets]\natlassian = { file = \"~/.atlassian_token\" }";

/// Resolve the site and credentials for `product`, honouring a project's overrides.
pub fn resolve_site(state: &AppState, project_id: Option<&str>, product: Product) -> Result<Site, ApiError> {
    let project = project_id.and_then(|id| state.projects.get(id));
    let global = state.config.read().atlassian.clone();
    let (o_site, o_email, o_token) = project
        .as_ref()
        .and_then(|p| match product {
            Product::Confluence => p.config.links.confluence.as_ref().map(|c| (c.site.clone(), c.email.clone(), c.token.clone())),
            Product::Jira => p.config.links.jira.as_ref().map(|j| (j.site.clone(), j.email.clone(), j.token.clone())),
        })
        .unwrap_or_default();
    let pick = |over: String, global: Option<&str>| {
        if !over.trim().is_empty() { over } else { global.unwrap_or_default().to_string() }
    };
    let site = pick(o_site, global.as_ref().map(|g| g.site.as_str()));
    let email = pick(o_email, global.as_ref().map(|g| g.email.as_str()));
    let token_name = pick(o_token, global.as_ref().map(|g| g.token.as_str()));
    if site.trim().is_empty() {
        return Err(ApiError::not_configured(if global.is_none() {
            format!("Atlassian is not set up. {SETUP_HELP}")
        } else {
            "Set [atlassian] site = \"https://<your-site>.atlassian.net\" in config.toml".to_string()
        }));
    }
    if token_name.trim().is_empty() {
        return Err(ApiError::not_configured(format!("No Atlassian API token configured. {SETUP_HELP}")));
    }
    let secret = state.secret(project.as_deref(), token_name.trim())?;
    Site::new(&site, &email, secret)
}

/// Confluence v2 list envelope.
#[derive(Debug, Deserialize)]
pub struct V2List<T> {
    #[serde(default = "Vec::new")]
    pub results: Vec<T>,
    #[serde(rename = "_links", default)]
    pub links: Option<NextLinks>,
}

#[derive(Debug, Default, Deserialize)]
pub struct NextLinks {
    #[serde(default)]
    pub next: Option<String>,
}

/// An authenticated client for one site.
#[derive(Clone)]
pub struct Api {
    pub http: reqwest::Client,
    pub site: Site,
    pub product: Product,
    /// The last status check found the credentials rejected; Confluence v2 reports that
    /// as 404, so 404s are then reported as a setup problem.
    pub auth_known_bad: bool,
}

impl Api {
    pub fn new(http: reqwest::Client, site: Site, product: Product) -> Self {
        Api { http, site, product, auth_known_bad: false }
    }

    /// `{site}/wiki{path}` (Confluence).
    pub fn wiki(&self, path: &str) -> String {
        format!("{}/wiki{path}", self.site.base)
    }

    /// `{site}{path}` (Jira and anything else).
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.site.base)
    }

    /// Turn a pagination link from the site into an absolute URL on the same site.
    /// `prefix` is what relative links lack (`""` for v2, whose links include `/wiki`;
    /// `"/wiki"` for v1 search). Links to other hosts are refused so credentials never
    /// leave the site.
    pub fn follow_link(&self, next: &str, prefix: &str) -> Result<String, ApiError> {
        if next.starts_with('/') {
            return Ok(format!("{}{prefix}{next}", self.site.base));
        }
        if let Some(rest) = next.strip_prefix(&self.site.base) {
            if rest.starts_with('/') {
                return Ok(next.to_string());
            }
        }
        Err(ApiError::upstream("Atlassian returned a pagination link to another host"))
    }

    /// Send a request with credentials, retrying `429 Too Many Requests` after the
    /// server's `Retry-After`. Returns the final response whatever its status.
    pub async fn send(&self, method: Method, url: &str, body: Option<&Value>) -> Result<reqwest::Response, ApiError> {
        let mut attempt = 0;
        loop {
            let mut rb = self
                .http
                .request(method.clone(), url)
                .header(header::AUTHORIZATION, self.site.auth.clone())
                .header(header::ACCEPT, "application/json");
            if method != Method::GET {
                // XSRF check bypass for API clients (required by some Confluence v1 writes).
                rb = rb.header("X-Atlassian-Token", "no-check");
            }
            if let Some(b) = body {
                rb = rb.json(b);
            }
            let resp = rb
                .send()
                .await
                .map_err(|e| ApiError::upstream(format!("cannot reach {}: {}", self.site.host(), e.without_url())))?;
            if resp.status() == StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                let wait = retry_after(resp.headers()).unwrap_or(Duration::from_millis(500 << attempt));
                attempt += 1;
                tracing::info!("atlassian: rate limited by {}, retrying in {:?}", self.site.host(), wait);
                tokio::time::sleep(wait.min(MAX_RETRY_WAIT)).await;
                continue;
            }
            return Ok(resp);
        }
    }

    /// Send a multipart form (attachment uploads) with credentials and the XSRF bypass
    /// header. Not retried: a streamed body cannot be sent twice. `timeout` replaces
    /// the client's default for this request (large uploads).
    pub async fn send_form(
        &self,
        method: Method,
        url: &str,
        form: reqwest::multipart::Form,
        timeout: Duration,
    ) -> Result<reqwest::Response, ApiError> {
        let resp = self
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, self.site.auth.clone())
            .header(header::ACCEPT, "application/json")
            .header("X-Atlassian-Token", "no-check")
            .timeout(timeout)
            .multipart(form)
            .send()
            .await
            .map_err(|e| ApiError::upstream(format!("cannot reach {}: {}", self.site.host(), e.without_url())))?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ApiError::upstream(format!("{} is rate limiting requests; try the upload again shortly", self.product.name())));
        }
        Ok(resp)
    }

    /// Like `send`, but non-2xx responses become an `ApiError`.
    pub async fn ok(&self, method: Method, url: &str, body: Option<&Value>) -> Result<reqwest::Response, ApiError> {
        let resp = self.send(method, url, body).await?;
        if resp.status().is_success() {
            Ok(resp)
        } else {
            Err(self.error_from(resp).await)
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let resp = self.ok(Method::GET, url, None).await?;
        self.json(resp).await
    }

    pub async fn send_json<T: DeserializeOwned>(&self, method: Method, url: &str, body: &Value) -> Result<T, ApiError> {
        let resp = self.ok(method, url, Some(body)).await?;
        self.json(resp).await
    }

    /// For endpoints that answer 204 No Content.
    pub async fn send_no_content(&self, method: Method, url: &str, body: &Value) -> Result<(), ApiError> {
        self.ok(method, url, Some(body)).await.map(|_| ())
    }

    pub async fn json<T: DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T, ApiError> {
        if resp.content_length().is_some_and(|n| n > MAX_JSON_BYTES) {
            return Err(ApiError::upstream("Atlassian response is too large"));
        }
        let bytes = resp.bytes().await.map_err(|e| ApiError::upstream(e.without_url().to_string()))?;
        if bytes.is_empty() {
            return serde_json::from_value(Value::Null)
                .map_err(|_| ApiError::upstream(format!("{} returned an empty response", self.product.name())));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::upstream(format!("unexpected response from {}: {e}", self.product.name())))
    }

    /// Follow Confluence v2 `_links.next` until `max` items; returns `(items, truncated)`.
    pub async fn v2_all<T: DeserializeOwned>(&self, first: String, max: usize) -> Result<(Vec<T>, bool), ApiError> {
        let mut url = first;
        let mut out: Vec<T> = vec![];
        loop {
            let page: V2List<T> = self.get(&url).await?;
            out.extend(page.results);
            match page.links.and_then(|l| l.next) {
                Some(_) if out.len() >= max => return Ok((out, true)),
                Some(next) => url = self.follow_link(&next, "")?,
                None => return Ok((out, false)),
            }
        }
    }

    /// Map a failed response to an `ApiError`, keeping only a short, redacted message.
    pub async fn error_from(&self, resp: reqwest::Response) -> ApiError {
        let status = resp.status();
        let is_v2 = resp.url().path().starts_with("/wiki/api/v2/");
        let mut body = Vec::new();
        let mut stream = resp;
        while let Ok(Some(chunk)) = stream.chunk().await {
            body.extend_from_slice(&chunk);
            if body.len() >= MAX_ERROR_BYTES {
                break;
            }
        }
        let msg = self.site.redact(&upstream_message(&body));
        map_status(status, &msg, self.product, is_v2 && self.auth_known_bad)
    }
}

/// Seconds from a `Retry-After` header (the HTTP-date form falls back to the default).
pub fn retry_after(h: &HeaderMap) -> Option<Duration> {
    let v = h.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    v.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).map(Duration::from_secs_f64)
}

/// Pull a human-readable message out of an Atlassian error body. Confluence v2:
/// `{"errors":[{"title":…}]}`; v1: `{"message":…}`; Jira: `{"errorMessages":[…],"errors":{field:msg}}`.
pub fn upstream_message(body: &[u8]) -> String {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        let text = String::from_utf8_lossy(body);
        // HTML error pages are noise; plain text is kept (short).
        return if text.trim_start().starts_with('<') { String::new() } else { truncate(text.trim(), 300) };
    };
    let mut parts: Vec<String> = vec![];
    if let Some(m) = v.get("message").and_then(Value::as_str) {
        parts.push(m.to_string());
    }
    if let Some(m) = v.get("errorMessage").and_then(Value::as_str) {
        parts.push(m.to_string());
    }
    if let Some(arr) = v.get("errorMessages").and_then(Value::as_array) {
        parts.extend(arr.iter().filter_map(Value::as_str).map(str::to_string));
    }
    match v.get("errors") {
        Some(Value::Array(arr)) => {
            for e in arr {
                let t = e.get("title").or_else(|| e.get("message")).and_then(Value::as_str);
                let d = e.get("detail").and_then(Value::as_str);
                match (t, d) {
                    (Some(t), Some(d)) if !d.is_empty() => parts.push(format!("{t}: {d}")),
                    (Some(t), _) => parts.push(t.to_string()),
                    (None, Some(d)) => parts.push(d.to_string()),
                    _ => {}
                }
            }
        }
        Some(Value::Object(map)) => {
            parts.extend(map.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or(&v.to_string()))));
        }
        _ => {}
    }
    if let Some(arr) = v.pointer("/data/errors").and_then(Value::as_array) {
        parts.extend(
            arr.iter()
                .filter_map(|e| e.pointer("/message/translation").or_else(|| e.get("message")).and_then(Value::as_str))
                .map(str::to_string),
        );
    }
    parts.dedup();
    truncate(&parts.join("; "), 300)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

pub fn map_status(status: StatusCode, msg: &str, product: Product, not_found_means_auth: bool) -> ApiError {
    let name = product.name();
    let detail = if msg.is_empty() { String::new() } else { format!(": {msg}") };
    match status.as_u16() {
        400 => ApiError::bad_request(format!("{name} rejected the request{detail}")),
        401 => ApiError::not_configured(format!(
            "{name} rejected the credentials (HTTP 401). Check [atlassian] email and the API token secret{detail}"
        )),
        403 => ApiError::forbidden(format!("{name}: permission denied{detail}")),
        404 if not_found_means_auth => ApiError::not_configured(format!(
            "{name} answered 404, and the last credential check failed: check [atlassian] email and the API token"
        )),
        404 => ApiError::not_found(format!("{name}: not found{detail}")),
        409 => ApiError::conflict(format!("{name}: conflict{detail}")),
        413 => ApiError::bad_request(format!("{name}: request too large{detail}")),
        429 => ApiError::upstream(format!("{name} is rate limiting requests; try again shortly")),
        s => ApiError::upstream(format!("{name} returned HTTP {s}{detail}")),
    }
}

/// The `cursor` query parameter of a pagination link (sent back by the UI for "load more").
pub fn cursor_of(next: &str) -> Option<String> {
    let query = next.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "cursor").then(|| urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_else(|_| v.to_string()))
    })
}

/// Percent-encode a query value.
pub fn q(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_site_urls() {
        assert_eq!(normalize_site("https://X.atlassian.net/wiki/").unwrap(), "https://x.atlassian.net");
        assert_eq!(normalize_site(" https://x.atlassian.net ").unwrap(), "https://x.atlassian.net");
        assert_eq!(normalize_site("http://127.0.0.1:4010").unwrap(), "http://127.0.0.1:4010");
        assert!(normalize_site("x.atlassian.net").is_err());
        assert!(normalize_site("https://user:pw@x.atlassian.net").is_err());
        assert!(normalize_site("https://x.atlassian.net/confluence").is_err());
        assert!(normalize_site("ftp://x").is_err());
    }

    #[test]
    fn extracts_upstream_messages_without_echoing_bodies() {
        let v2 = br#"{"errors":[{"status":409,"code":"CONFLICT","title":"Version must be incremented","detail":null}]}"#;
        assert_eq!(upstream_message(v2), "Version must be incremented");
        let jira = br#"{"errorMessages":["Issue does not exist"],"errors":{"summary":"required"}}"#;
        assert_eq!(upstream_message(jira), "Issue does not exist; summary: required");
        let v1 = br#"{"statusCode":400,"data":{"errors":[{"message":{"translation":"bad xhtml"}}]},"message":"Error parsing xhtml"}"#;
        assert_eq!(upstream_message(v1), "Error parsing xhtml; bad xhtml");
        assert_eq!(upstream_message(b"<html><body>Oops</body></html>"), "");
        assert_eq!(upstream_message(&vec![b'a'; 1000]).chars().count(), 301);
    }

    #[test]
    fn maps_statuses() {
        assert_eq!(map_status(StatusCode::UNAUTHORIZED, "", Product::Jira, false).code, "not_configured");
        assert_eq!(map_status(StatusCode::NOT_FOUND, "", Product::Confluence, false).code, "not_found");
        assert_eq!(map_status(StatusCode::NOT_FOUND, "", Product::Confluence, true).code, "not_configured");
        assert_eq!(map_status(StatusCode::CONFLICT, "x", Product::Confluence, false).code, "conflict");
        assert_eq!(map_status(StatusCode::BAD_GATEWAY, "", Product::Confluence, false).code, "upstream");
    }

    #[test]
    fn parses_retry_after_and_cursors() {
        let mut h = HeaderMap::new();
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("3"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(3)));
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
        assert_eq!(retry_after(&h), None);
        assert_eq!(
            cursor_of("/wiki/api/v2/pages/1/versions?limit=50&cursor=eyJ%3D%3D").as_deref(),
            Some("eyJ==")
        );
        assert_eq!(cursor_of("/rest/api/search?next=true&cursor=abc&limit=25").as_deref(), Some("abc"));
        assert_eq!(cursor_of("/wiki/api/v2/pages"), None);
    }
}

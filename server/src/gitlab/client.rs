//! The GitLab REST client: per-project connection (API base URL, token, numeric
//! project id), JSON requests, `Link rel="next"` pagination, rate-limit handling
//! and error mapping that never echoes upstream bodies.
//!
//! * The token comes from the project's `repo.gitlab.token` secret, else from the
//!   global `[gitlab] token` when its host is the project's base URL, scheme
//!   included (an `http://` twin of an https host never gets it). It is only ever
//!   put into the `PRIVATE-TOKEN` header of requests to that GitLab origin.
//! * Redirects are followed by hand: same-origin hops keep the token, cross-origin
//!   hops (artifact storage) are fetched without it. reqwest's automatic redirects
//!   would forward the custom `PRIVATE-TOKEN` header to any host.
//! * `host` may carry a scheme (`http://127.0.0.1:8929`) for self-managed or mock
//!   servers; a bare host means `https://`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, LINK, LOCATION, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::Pipeline;
use super::poller::PollState;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::projects::Project;
use crate::secrets::Secret;

/// Per-request timeout for API calls (downloads set their own).
const API_TIMEOUT: Duration = Duration::from_secs(45);
/// How long project metadata (numeric id, default branch…) stays cached.
const META_TTL: Duration = Duration::from_secs(600);
/// Largest JSON body we accept from GitLab.
const MAX_JSON_BODY: usize = 48 * 1024 * 1024;
/// Largest error body we read (only to extract `message`).
const MAX_ERROR_BODY: usize = 64 * 1024;
/// Longest `Retry-After` we wait out inside a request; longer ones fail fast.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(20);
/// Cap on cached pipeline details (cleared wholesale when exceeded).
const PIPELINE_CACHE_CAP: usize = 4000;

/// Slice state, one per server (`AppState::gitlab`).
#[derive(Default)]
pub struct GitlabState {
    http: OnceLock<reqwest::Client>,
    meta: Mutex<HashMap<String, (Instant, Arc<ProjectMeta>)>>,
    rate: Mutex<HashMap<String, RateInfo>>,
    /// Pipeline details keyed by `api|id`, valid while `updated_at` is unchanged.
    pipelines: Mutex<HashMap<String, (String, Pipeline)>>,
    /// Short-lived summaries keyed by project id (dedupes several open tabs).
    pub(super) summaries: Mutex<HashMap<String, (Instant, Value)>>,
    pub(super) poll: PollState,
}

/// What the last response said about the rate limit of one host.
#[derive(Debug, Clone, Copy, Default)]
pub struct RateInfo {
    #[allow(dead_code)]
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    /// Epoch seconds when the window resets.
    pub reset: Option<i64>,
}

impl GitlabState {
    /// The slice's HTTP client: no automatic redirects (see the module docs).
    pub(super) fn http(&self) -> reqwest::Client {
        self.http
            .get_or_init(|| {
                reqwest::Client::builder()
                    .user_agent(concat!("workbench/", env!("CARGO_PKG_VERSION")))
                    .connect_timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap_or_default()
            })
            .clone()
    }

    fn note_rate(&self, host: &str, headers: &HeaderMap) {
        let num = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<i64>().ok());
        if num("ratelimit-remaining").is_none() {
            return;
        }
        let info = RateInfo {
            limit: num("ratelimit-limit").map(|v| v.max(0) as u64),
            remaining: num("ratelimit-remaining").map(|v| v.max(0) as u64),
            reset: num("ratelimit-reset"),
        };
        self.rate.lock().insert(host.to_string(), info);
    }

    /// The last rate-limit headers seen for `host`.
    pub fn rate(&self, host: &str) -> Option<RateInfo> {
        self.rate.lock().get(host).copied()
    }

    /// True when the host said we are nearly out of requests for this window.
    pub fn rate_low(&self, host: &str) -> bool {
        match self.rate(host) {
            Some(RateInfo { remaining: Some(r), reset: Some(reset), .. }) => {
                r < 60 && reset > chrono::Utc::now().timestamp()
            }
            _ => false,
        }
    }

    pub(super) fn cached_pipeline(&self, key: &str, updated_at: &str) -> Option<Pipeline> {
        let map = self.pipelines.lock();
        map.get(key).filter(|(u, _)| u == updated_at).map(|(_, p)| p.clone())
    }

    pub(super) fn cache_pipeline(&self, key: String, p: &Pipeline) {
        let Some(updated) = p.updated_at.clone() else { return };
        let mut map = self.pipelines.lock();
        if map.len() >= PIPELINE_CACHE_CAP {
            map.clear();
        }
        map.insert(key, (updated, p.clone()));
    }

    /// Forget the cached summary of a project (after a mutation).
    pub(super) fn invalidate_summary(&self, project_id: &str) {
        self.summaries.lock().remove(project_id);
    }
}

/// Project facts from `GET /projects/:id`, cached for `META_TTL`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct ProjectMeta {
    pub id: u64,
    pub path_with_namespace: String,
    pub default_branch: Option<String>,
    pub web_url: String,
    pub merge_method: Option<String>,
    pub squash_option: Option<String>,
    pub remove_source_branch_after_merge: Option<bool>,
    pub only_allow_merge_if_pipeline_succeeds: Option<bool>,
    pub container_registry_enabled: Option<bool>,
    pub container_registry_image_prefix: Option<String>,
    pub issues_enabled: Option<bool>,
    pub merge_requests_enabled: Option<bool>,
    pub jobs_enabled: Option<bool>,
    pub open_issues_count: Option<u64>,
    pub permissions: Option<Permissions>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Permissions {
    pub project_access: Option<AccessLevel>,
    pub group_access: Option<AccessLevel>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct AccessLevel {
    pub access_level: u64,
}

impl ProjectMeta {
    /// Highest access level (10 guest … 30 developer, 40 maintainer, 50 owner).
    pub fn access_level(&self) -> Option<u64> {
        let p = self.permissions.as_ref()?;
        let a = p.project_access.as_ref().map(|a| a.access_level);
        let g = p.group_access.as_ref().map(|a| a.access_level);
        a.max(g)
    }
}

/// `(api base, web base)` for a configured host (`gitlab.com` or `http://host:port`,
/// optionally with a relative URL root such as `https://example.com/gitlab`).
/// The scheme and authority of the web base are lower-case.
pub fn base_urls(host: &str) -> ApiResult<(String, String)> {
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        return Err(ApiError::not_configured("the GitLab host is empty"));
    }
    let (scheme, rest) = match host.split_once("://") {
        None => ("https", host),
        Some((s, r)) if s.eq_ignore_ascii_case("https") => ("https", r),
        Some((s, r)) if s.eq_ignore_ascii_case("http") => ("http", r),
        Some(_) => return Err(ApiError::bad_request("the GitLab host must be a host name or an http(s):// URL")),
    };
    let (authority, root) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if authority.is_empty()
        || rest.contains(['@', '?', '#', '\\'])
        || rest.contains("://")
        || rest.contains(char::is_whitespace)
    {
        return Err(ApiError::bad_request("the GitLab host must not contain credentials, a query, backslashes or spaces"));
    }
    let web = format!("{scheme}://{}{root}", authority.to_ascii_lowercase());
    Ok((format!("{web}/api/v4"), web))
}

/// `gitlab.com` from `https://gitlab.com` (for display and rate-limit keys; token
/// matching compares whole base URLs, see `global_host_matches`).
pub fn display_host(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    h.split_once("://").map(|(_, r)| r).unwrap_or(h).to_ascii_lowercase()
}

/// `scheme://authority` of a URL (for same-origin checks).
fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[..scheme.len() + 3 + end])
}

/// Whether config.toml's `[gitlab] host` is the GitLab at `web` (a project's web
/// base from `base_urls`). The whole base URL must match, **scheme included**: a
/// repository layer that names `http://gitlab.com` must not receive the token the
/// owner configured for `https://gitlab.com`, which would cross the network in
/// clear text on the first request.
fn global_host_matches(global_host: &str, web: &str) -> bool {
    base_urls(global_host).is_ok_and(|(_, g)| g.eq_ignore_ascii_case(web))
}

/// A web base for messages: `gitlab.com` when it is https, else the full
/// `http://host:port`, so that twins differing only in scheme read differently.
fn shown_origin(web: &str) -> String {
    web.strip_prefix("https://").unwrap_or(web).to_string()
}

/// Resolve the token for a project whose GitLab is at `web` (from `base_urls`):
/// the project's own `repo.gitlab.token` first, then the global `[gitlab]` token
/// when it is configured for that same base URL (`global_host_matches`).
fn token_for(state: &AppState, project: &Project, web: &str) -> ApiResult<Secret> {
    let own = project
        .config
        .repo
        .as_ref()
        .and_then(|r| r.gitlab.as_ref())
        .map(|g| g.token.trim().to_string())
        .filter(|t| !t.is_empty());
    if let Some(name) = own {
        return state.secret(Some(project), &name);
    }
    let global = state.config.read().gitlab.clone();
    match global {
        Some(g) if !g.token.trim().is_empty() => {
            if !global_host_matches(&g.host, web) {
                let configured = base_urls(&g.host).map(|(_, w)| shown_origin(&w)).unwrap_or_else(|_| g.host.trim().to_string());
                return Err(ApiError::not_configured(format!(
                    "no GitLab token for {}: the global [gitlab] token is for {configured}; set repo.gitlab.token in this project's overlay",
                    shown_origin(web),
                )));
            }
            state.secret(Some(project), g.token.trim())
        }
        _ => Err(ApiError::not_configured(
            "GitLab is not set up: add [gitlab] token = \"gitlab\" and [secrets] gitlab = { file = \"~/.gitlab_token\" } to config.toml",
        )),
    }
}

/// An authenticated connection to one GitLab instance.
#[derive(Clone)]
pub struct Conn {
    pub state: AppState,
    /// Display host (`gitlab.com`).
    pub host: String,
    /// `https://gitlab.com/api/v4`
    pub api: String,
    /// `https://gitlab.com`
    pub web: String,
    token: Secret,
    http: reqwest::Client,
}

/// A connection bound to one project.
#[derive(Clone)]
pub struct GlCtx {
    pub conn: Conn,
    pub project: Arc<Project>,
    pub meta: Arc<ProjectMeta>,
}

impl std::ops::Deref for GlCtx {
    type Target = Conn;
    fn deref(&self) -> &Conn {
        &self.conn
    }
}

/// One page of a list endpoint.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: Option<u32>,
    pub next_page: Option<u32>,
    pub total: Option<u64>,
    pub next_url: Option<String>,
}

/// Connection + project for a Workbench project id.
pub async fn ctx(state: &AppState, pid: &str) -> ApiResult<GlCtx> {
    let project = state.projects.require(pid)?;
    ctx_for(state, project).await
}

pub async fn ctx_for(state: &AppState, project: Arc<Project>) -> ApiResult<GlCtx> {
    let (host, path) = project.gitlab().ok_or_else(|| {
        ApiError::not_configured("this project is not on GitLab (no GitLab remote and no [repo.gitlab] section)")
    })?;
    let (api, web) = base_urls(&host)?;
    let token = token_for(state, &project, &web)?;
    let conn = Conn { state: state.clone(), host: display_host(&host), api, web, token, http: state.gitlab.http() };
    let configured_id = project.config.repo.as_ref().and_then(|r| r.gitlab.as_ref()).and_then(|g| g.project_id);
    let meta = conn.project_meta(&path, configured_id).await?;
    Ok(GlCtx { conn, project, meta })
}

impl GlCtx {
    /// Numeric GitLab project id.
#[cfg(test)]
    pub fn id(&self) -> u64 {
        self.meta.id
    }

    /// `{api}/projects/{id}{rel}`
    pub fn purl(&self, rel: &str) -> String {
        format!("{}/projects/{}{}", self.api, self.meta.id, rel)
    }

    pub fn default_branch(&self) -> Option<&str> {
        self.meta.default_branch.as_deref().filter(|b| !b.is_empty())
    }
}

impl Conn {
    /// Metadata for `ns/proj` (cached). Resolves the numeric id once per TTL.
    async fn project_meta(&self, path: &str, configured_id: Option<u64>) -> ApiResult<Arc<ProjectMeta>> {
        let key = format!("{}|{path}", self.api);
        if let Some((at, m)) = self.state.gitlab.meta.lock().get(&key) {
            if at.elapsed() < META_TTL {
                return Ok(m.clone());
            }
        }
        let ident = match configured_id {
            Some(id) => id.to_string(),
            None => urlencoding::encode(path).into_owned(),
        };
        let url = format!("{}/projects/{ident}", self.api);
        let meta: ProjectMeta = match self.get(&url, &[]).await {
            Ok(m) => m,
            Err(e) if e.status == StatusCode::NOT_FOUND => {
                return Err(ApiError::not_found(format!(
                    "GitLab project {path} was not found on {} or this token cannot see it",
                    self.host
                )));
            }
            Err(e) => return Err(e),
        };
        if meta.id == 0 {
            return Err(ApiError::upstream("GitLab returned a project without an id"));
        }
        let meta = Arc::new(meta);
        self.state.gitlab.meta.lock().insert(key, (Instant::now(), meta.clone()));
        Ok(meta)
    }

    fn same_origin(&self, url: &str) -> bool {
        origin(url).is_some_and(|o| Some(o) == origin(&self.web))
    }

    fn authed(&self, method: Method, url: &str) -> RequestBuilder {
        self.http.request(method, url).header("PRIVATE-TOKEN", self.token.expose()).timeout(API_TIMEOUT)
    }

    fn net_error(&self, e: reqwest::Error) -> ApiError {
        if e.is_timeout() {
            ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timeout", format!("{} did not answer in time", self.host))
        } else {
            ApiError::upstream(format!("cannot reach {}: {}", self.host, e.without_url()))
        }
    }

    /// Send a request (with `build` applied), waiting out short 429s and retrying a
    /// transient 502/503/504 once for idempotent methods. Follows same-origin
    /// redirects with the token and cross-origin ones without it (GET only).
    /// Returns the response whatever its status.
    pub async fn send_raw(
        &self,
        method: Method,
        url: &str,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
    ) -> ApiResult<Response> {
        if !self.same_origin(url) {
            return Err(ApiError::internal("refusing to send the GitLab token to another origin"));
        }
        let idempotent = matches!(method, Method::GET | Method::HEAD | Method::PUT | Method::DELETE);
        let mut attempt = 0u32;
        loop {
            let resp = build(self.authed(method.clone(), url)).send().await.map_err(|e| self.net_error(e))?;
            self.state.gitlab.note_rate(&self.host, resp.headers());
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let wait = retry_after(resp.headers()).unwrap_or(Duration::from_secs(5));
                if attempt < 2 && wait <= MAX_RETRY_WAIT {
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                    continue;
                }
                return Err(ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    format!("{} rate limit reached; try again in {}s", self.host, wait.as_secs().max(1)),
                ));
            }
            if idempotent
                && attempt == 0
                && matches!(status, StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT)
            {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(700)).await;
                continue;
            }
            if status.is_redirection() && method == Method::GET {
                return self.follow_redirects(url, resp).await;
            }
            return Ok(resp);
        }
    }

    async fn follow_redirects(&self, from: &str, mut resp: Response) -> ApiResult<Response> {
        let mut current = from.to_string();
        for _ in 0..5 {
            if !resp.status().is_redirection() {
                return Ok(resp);
            }
            let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok()) else {
                return Ok(resp);
            };
            let next = resolve_location(&current, loc).ok_or_else(|| ApiError::upstream("GitLab sent a bad redirect"))?;
            resp = if self.same_origin(&next) {
                self.authed(Method::GET, &next).send().await.map_err(|e| self.net_error(e))?
            } else {
                // Object storage (signed URL): never send the token there.
                self.http
                    .get(&next)
                    .timeout(Duration::from_secs(3600))
                    .send()
                    .await
                    .map_err(|e| self.net_error(e))?
            };
            current = next;
        }
        Err(ApiError::upstream("too many redirects from GitLab"))
    }

    /// Like `send_raw`, but non-2xx statuses become `ApiError`s.
    pub async fn send(
        &self,
        method: Method,
        url: &str,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
    ) -> ApiResult<Response> {
        let resp = self.send_raw(method, url, build).await?;
        if resp.status().is_success() {
            Ok(resp)
        } else {
            Err(self.error_from(resp).await)
        }
    }

    /// Map a failed response to an `ApiError`. Only GitLab's `message`/`error`
    /// fields are used (never the raw body), and known secrets are redacted.
    pub async fn error_from(&self, resp: Response) -> ApiError {
        let status = resp.status();
        let body = read_limited(resp, MAX_ERROR_BODY).await.unwrap_or_default();
        let msg = extract_message(&body).map(|m| crate::secrets::redact(&m, std::slice::from_ref(&self.token)));
        map_status(status, msg, &self.host)
    }

    /// Parse a JSON response body (bounded).
    pub async fn parse<T: DeserializeOwned>(&self, resp: Response) -> ApiResult<T> {
        let bytes = read_limited(resp, MAX_JSON_BODY).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::upstream(format!("unexpected response from {}: {e}", self.host)))
    }

    pub async fn get<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)]) -> ApiResult<T> {
        let resp = self.send(Method::GET, url, |rb| rb.query(query)).await?;
        self.parse(resp).await
    }

    /// GET that maps 404 to `None`.
    pub async fn get_opt<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)]) -> ApiResult<Option<T>> {
        let resp = self.send_raw(Method::GET, url, |rb| rb.query(query)).await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(self.error_from(resp).await);
        }
        self.parse(resp).await.map(Some)
    }

    /// POST/PUT/DELETE with a JSON body; returns the parsed response (`Value::Null` when empty).
    pub async fn write<T: DeserializeOwned>(&self, method: Method, url: &str, body: Option<&Value>) -> ApiResult<T> {
        let resp = self
            .send(method, url, |rb| match body {
                Some(b) => rb.json(b),
                None => rb,
            })
            .await?;
        let bytes = read_limited(resp, MAX_JSON_BODY).await?;
        let bytes: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) { b"null" } else { &bytes };
        serde_json::from_slice(bytes)
            .map_err(|e| ApiError::upstream(format!("unexpected response from {}: {e}", self.host)))
    }

    /// One page of a list endpoint, with GitLab's pagination headers.
    pub async fn get_page<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)]) -> ApiResult<Page<T>> {
        let resp = self.send(Method::GET, url, |rb| rb.query(query)).await?;
        let headers = resp.headers().clone();
        let items: Vec<T> = self.parse(resp).await?;
        let num = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        Ok(Page {
            items,
            page: num("x-page").map(|v| v as u32),
            next_page: num("x-next-page").map(|v| v as u32),
            total: num("x-total"),
            next_url: link_next(&headers).filter(|u| self.same_origin(u)),
        })
    }

    /// Every item of a list endpoint (following `Link rel="next"`), up to `cap`.
    /// The bool is true when items were left out because of the cap.
    pub async fn get_all<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
        cap: usize,
    ) -> ApiResult<(Vec<T>, bool)> {
        let mut q: Vec<(&str, String)> = query.to_vec();
        if !q.iter().any(|(k, _)| *k == "per_page") {
            q.push(("per_page", "100".into()));
        }
        let mut page = self.get_page::<T>(url, &q).await?;
        let mut out = Vec::new();
        loop {
            out.extend(page.items);
            if out.len() >= cap {
                let more = out.len() > cap || page.next_url.is_some();
                out.truncate(cap);
                return Ok((out, more));
            }
            match page.next_url {
                Some(next) => page = self.get_page::<T>(&next, &[]).await?,
                None => return Ok((out, false)),
            }
        }
    }

    /// A read-only GraphQL query, sent as GET (`?query=&variables=`) so it can never mutate.
    pub async fn graphql(&self, query: &str, variables: Value) -> ApiResult<Value> {
        let url = format!("{}/api/graphql", self.web);
        let vars = variables.to_string();
        let resp = self
            .send(Method::GET, &url, |rb| rb.query(&[("query", query), ("variables", vars.as_str())]))
            .await?;
        let mut v: Value = self.parse(resp).await?;
        if v.get("data").is_none_or(Value::is_null) {
            let msg = v
                .pointer("/errors/0/message")
                .and_then(Value::as_str)
                .unwrap_or("GraphQL query failed")
                .chars()
                .take(300)
                .collect::<String>();
            return Err(ApiError::upstream(format!("GitLab GraphQL: {msg}")));
        }
        Ok(v["data"].take())
    }
}

/// Read at most `cap` bytes of a body; larger bodies are an error.
pub async fn read_limited(mut resp: Response, cap: usize) -> ApiResult<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| ApiError::upstream(e.without_url().to_string()))? {
        if out.len() + chunk.len() > cap {
            return Err(ApiError::upstream(format!("GitLab response larger than {} MB", cap / (1024 * 1024))));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Read a body keeping only its last `keep` bytes (bounded memory for huge logs).
/// Returns `(tail, total_bytes, dropped_front)`.
pub async fn read_tail(mut resp: Response, keep: usize) -> ApiResult<(Vec<u8>, u64, bool)> {
    let mut buf: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    let mut dropped = false;
    while let Some(chunk) = resp.chunk().await.map_err(|e| ApiError::upstream(e.without_url().to_string()))? {
        total += chunk.len() as u64;
        buf.extend_from_slice(&chunk);
        // Compact only when the buffer is well past the limit to keep this O(n).
        if buf.len() > keep * 2 {
            let cut = buf.len() - keep;
            buf.drain(..cut);
            dropped = true;
        }
    }
    if buf.len() > keep {
        let cut = buf.len() - keep;
        buf.drain(..cut);
        dropped = true;
    }
    Ok((buf, total, dropped))
}

/// `Retry-After` as a duration (seconds or an HTTP date).
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let secs = (at.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64;
    Some(Duration::from_secs(secs))
}

/// The `rel="next"` URL of a `Link` header (RFC 8288), if any.
pub fn link_next(headers: &HeaderMap) -> Option<String> {
    headers.get_all(LINK).iter().filter_map(|v| v.to_str().ok()).find_map(parse_link_next)
}

pub fn parse_link_next(value: &str) -> Option<String> {
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        let end = start + rest[start..].find('>')?;
        let url = &rest[start + 1..end];
        let after = &rest[end + 1..];
        let params_end = after.find('<').unwrap_or(after.len());
        let params = &after[..params_end];
        let is_next = params.split(';').any(|p| {
            let p = p.trim().trim_end_matches(',').trim();
            let Some(v) = p.strip_prefix("rel=").or_else(|| p.strip_prefix("REL=")) else { return false };
            v.trim_matches('"').split_whitespace().any(|r| r.eq_ignore_ascii_case("next"))
        });
        if is_next {
            return Some(url.to_string());
        }
        rest = &after[params_end..];
    }
    None
}

/// Resolve a `Location` header against the request URL.
fn resolve_location(base: &str, loc: &str) -> Option<String> {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return Some(loc.to_string());
    }
    let o = origin(base)?;
    if let Some(rest) = loc.strip_prefix("//") {
        let scheme = base.split_once("://")?.0;
        return Some(format!("{scheme}://{rest}"));
    }
    if loc.starts_with('/') {
        return Some(format!("{o}{loc}"));
    }
    None
}

/// GitLab's human message from an error body: `message` (string, list or
/// field → errors map), or `error` + `error_description`. Never the raw body.
pub fn extract_message(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    fn flat(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => {
                let parts: Vec<String> = a.iter().filter_map(flat).collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            Value::Object(m) => {
                let parts: Vec<String> = m
                    .iter()
                    .filter_map(|(k, v)| flat(v).map(|s| if k == "base" { s } else { format!("{k} {s}") }))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            _ => None,
        }
    }
    let mut msg = match (v.get("message"), v.get("error")) {
        (Some(m), _) => flat(m)?,
        (None, Some(e)) => {
            let mut s = flat(e)?;
            if let Some(d) = v.get("error_description").and_then(Value::as_str) {
                s = format!("{s}: {d}");
            }
            s
        }
        _ => return None,
    };
    if msg.chars().count() > 500 {
        msg = msg.chars().take(500).collect::<String>() + "…";
    }
    Some(msg)
}

/// Map an upstream status to the right `ApiError` constructor.
pub fn map_status(status: StatusCode, msg: Option<String>, host: &str) -> ApiError {
    let m = msg.unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string());
    match status.as_u16() {
        401 => ApiError::not_configured(format!(
            "{host} rejected the GitLab token (401 Unauthorized); check the token secret and its expiry date"
        )),
        403 => ApiError::forbidden(format!("GitLab: {m}")),
        404 => ApiError::not_found(format!("GitLab: {m}")),
        // 405/406: "not mergeable"/"not allowed in this state"; 409: SHA or state moved on.
        405 | 406 | 409 | 412 => ApiError::conflict(format!("GitLab: {m}")),
        400 | 422 => ApiError::bad_request(format!("GitLab: {m}")),
        429 => ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", format!("GitLab: {m}")),
        _ => ApiError::upstream(format!("GitLab returned {}: {m}", status.as_u16())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_header_next_is_found_among_others() {
        let v = r#"<https://gitlab.com/api/v4/projects/1/pipelines?page=2&per_page=1>; rel="next", <https://gitlab.com/api/v4/projects/1/pipelines?page=1&per_page=1>; rel="first", <https://gitlab.com/api/v4/projects/1/pipelines?page=833&per_page=1>; rel="last""#;
        assert_eq!(
            parse_link_next(v).as_deref(),
            Some("https://gitlab.com/api/v4/projects/1/pipelines?page=2&per_page=1")
        );
        // next listed last, unquoted rel, multiple rel values
        let v = "<https://x/a?page=1>; rel=\"first\", <https://x/a?page=3>; rel=next";
        assert_eq!(parse_link_next(v).as_deref(), Some("https://x/a?page=3"));
        let v = "<https://x/a?id_after=5>; rel=\"next last\"";
        assert_eq!(parse_link_next(v).as_deref(), Some("https://x/a?id_after=5"));
        assert_eq!(parse_link_next("<https://x/a?page=1>; rel=\"first\""), None);
        assert_eq!(parse_link_next(""), None);
        assert_eq!(parse_link_next("<broken"), None);
    }

    #[test]
    fn keyset_link_is_followed() {
        let v = r#"<https://gitlab.com/api/v4/projects/85344789/registry/repositories/12111782/tags?id=85344789&last=ff27b11f&page=1&pagination=keyset&per_page=3&repository_id=12111782&sort=desc>; rel="next""#;
        assert!(parse_link_next(v).unwrap().contains("last=ff27b11f"));
    }

    #[test]
    fn base_urls_accept_hosts_and_scheme_urls() {
        assert_eq!(
            base_urls("gitlab.com").unwrap(),
            ("https://gitlab.com/api/v4".to_string(), "https://gitlab.com".to_string())
        );
        assert_eq!(base_urls("http://127.0.0.1:8929/").unwrap().0, "http://127.0.0.1:8929/api/v4");
        assert!(base_urls("https://user:pw@gitlab.com").is_err());
        assert!(base_urls("").is_err());
        assert_eq!(display_host("https://GitLab.example.com/"), "gitlab.example.com");
        // Scheme and host are normalised; a relative URL root keeps its case.
        assert_eq!(base_urls("HTTPS://GitLab.Example.com/Git/").unwrap().1, "https://gitlab.example.com/Git");
        assert_eq!(base_urls("Http://127.0.0.1:8929").unwrap().1, "http://127.0.0.1:8929");
        // Only http(s), and nothing that makes the URL name another host than it seems.
        for bad in [
            "ftp://gitlab.com",
            "xyz://gitlab.com",
            "https://HTTP://gitlab.com",
            "gitlab.com?x=1",
            "gitlab.com#frag",
            "gitlab.com\\@evil.example",
            "evil.example\\.gitlab.com",
            "https:///path",
            "gitlab .com",
        ] {
            assert_eq!(base_urls(bad).map_err(|e| e.code), Err("bad_request"), "{bad}");
        }
    }

    #[test]
    fn the_global_token_host_must_match_including_the_scheme() {
        let web = |h: &str| base_urls(h).unwrap().1;
        // Same base URL, however it is spelled.
        assert!(global_host_matches("gitlab.com", &web("gitlab.com")));
        assert!(global_host_matches("https://GitLab.com/", &web("gitlab.com")));
        assert!(global_host_matches("gitlab.com", &web("HTTPS://gitlab.com")));
        assert!(global_host_matches("http://127.0.0.1:8929", &web("http://127.0.0.1:8929/")));
        assert!(global_host_matches("https://example.com/gitlab", &web("https://example.com/gitlab")));
        // The plain-http twin of an https host (a bare host means https), and back.
        assert!(!global_host_matches("gitlab.com", &web("http://gitlab.com")));
        assert!(!global_host_matches("https://gitlab.com", &web("http://gitlab.com")));
        assert!(!global_host_matches("127.0.0.1:8929", &web("http://127.0.0.1:8929")));
        assert!(!global_host_matches("http://gitlab.internal", &web("gitlab.internal")));
        // Other ports, hosts, roots, and hosts that are not valid at all.
        assert!(!global_host_matches("gitlab.com", &web("gitlab.com:8443")));
        assert!(!global_host_matches("gitlab.com", &web("gitlab.com.evil.example")));
        assert!(!global_host_matches("gitlab.com", &web("https://gitlab.com/other")));
        assert!(!global_host_matches("", &web("gitlab.com")));
        assert!(!global_host_matches("xyz://gitlab.com", &web("gitlab.com")));
        // Messages tell the twins apart.
        assert_eq!(shown_origin(&web("gitlab.com")), "gitlab.com");
        assert_eq!(shown_origin(&web("http://gitlab.com")), "http://gitlab.com");
    }

    #[test]
    fn origins_and_redirects() {
        assert_eq!(origin("https://gitlab.com/api/v4/x?y"), Some("https://gitlab.com"));
        assert_eq!(origin("http://127.0.0.1:99"), Some("http://127.0.0.1:99"));
        assert_eq!(resolve_location("https://gitlab.com/api/v4/a", "/b?c").as_deref(), Some("https://gitlab.com/b?c"));
        assert_eq!(
            resolve_location("https://gitlab.com/a", "https://storage.googleapis.com/x").as_deref(),
            Some("https://storage.googleapis.com/x")
        );
        assert_eq!(resolve_location("https://gitlab.com/a", "//cdn.example/x").as_deref(), Some("https://cdn.example/x"));
    }

    #[test]
    fn error_messages_are_extracted_not_echoed() {
        assert_eq!(extract_message(br#"{"message":"404 Project Not Found"}"#).as_deref(), Some("404 Project Not Found"));
        assert_eq!(
            extract_message(br#"{"message":["Another open merge request already exists for this source branch: !7"]}"#)
                .as_deref(),
            Some("Another open merge request already exists for this source branch: !7")
        );
        assert_eq!(
            extract_message(br#"{"message":{"title":["can't be blank"],"base":["is invalid"]}}"#).as_deref(),
            Some("is invalid; title can't be blank")
        );
        assert_eq!(
            extract_message(br#"{"error":"insufficient_scope","error_description":"needs api"}"#).as_deref(),
            Some("insufficient_scope: needs api")
        );
        // HTML error pages and arbitrary bodies are never passed through.
        assert_eq!(extract_message(b"<html>secret token abc</html>"), None);
        assert_eq!(extract_message(br#"{"other":"x"}"#), None);
    }

    #[test]
    fn statuses_map_to_the_right_error_kinds() {
        assert_eq!(map_status(StatusCode::UNAUTHORIZED, None, "gitlab.com").code, "not_configured");
        assert_eq!(map_status(StatusCode::NOT_FOUND, None, "h").code, "not_found");
        assert_eq!(map_status(StatusCode::CONFLICT, Some("SHA does not match".into()), "h").code, "conflict");
        assert_eq!(map_status(StatusCode::METHOD_NOT_ALLOWED, None, "h").code, "conflict");
        assert_eq!(map_status(StatusCode::UNPROCESSABLE_ENTITY, None, "h").code, "bad_request");
        assert_eq!(map_status(StatusCode::BAD_GATEWAY, None, "h").code, "upstream");
    }

    #[test]
    fn retry_after_parses_seconds() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert(RETRY_AFTER, "garbage".parse().unwrap());
        assert_eq!(retry_after(&h), None);
    }
}

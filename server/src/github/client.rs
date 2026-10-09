//! The GitHub REST client: per-project connection (API base, token or
//! anonymous), JSON requests with an ETag/TTL response cache, `Link rel="next"`
//! pagination, rate-limit handling and error mapping that never echoes
//! upstream bodies.
//!
//! * **Token.** The project's `repo.github.token` secret (resolved by
//!   `AppState::secret`, so a name from repository config only resolves against
//!   the machine overlay), else the global `[github] token` when its host is the
//!   project's host (scheme included: an `http://` host never gets the token).
//!   Without a token the client is **anonymous**: public repositories only, read
//!   only, 60 requests an hour per IP. The token only ever goes into the
//!   `Authorization` header of requests to the API origin.
//! * **API base.** `https://api.github.com` for github.com, `https://<host>/api/v3`
//!   (GraphQL `https://<host>/api/graphql`) for GitHub Enterprise. `host` may carry
//!   a scheme (`http://127.0.0.1:8930`) for Enterprise servers and mocks.
//! * **Redirects** are followed by hand: same-origin hops keep the token,
//!   cross-origin ones (signed log URLs, object storage) are fetched without it.
//! * **Rate limits.** `x-ratelimit-*` headers are remembered per API, identity and
//!   resource; a request is not even sent while the quota is known to be
//!   exhausted. Secondary limits (403/429 with `Retry-After`) are waited out when
//!   short. A cached response is served (stale) rather than failing on a limit.
//! * **Caching.** GET responses are cached with their ETag. Within a freshness
//!   window (`Fresh`, much longer when anonymous) the cache answers alone; after
//!   it the request is conditional (`If-None-Match`, a 304 is free for
//!   authenticated requests). Writes drop the repository's cached responses.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use bytes::Bytes;
use parking_lot::Mutex;
use reqwest::header::{ACCEPT, AUTHORIZATION, ETAG, HeaderMap, IF_NONE_MATCH, LINK, LOCATION, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;

use super::poller::PollState;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::forge::RepoParam;
use crate::projects::Project;
use crate::secrets::Secret;

/// Per-request timeout for API calls (downloads set their own).
const API_TIMEOUT: Duration = Duration::from_secs(45);
/// How long repository metadata stays cached.
const META_TTL: Duration = Duration::from_secs(600);
/// How long "this repository does not exist (for us)" is remembered, so a
/// private repository without a token does not spend the anonymous quota on
/// every summary refresh and poll.
const MISSING_TTL_ANON: Duration = Duration::from_secs(600);
const MISSING_TTL_AUTH: Duration = Duration::from_secs(60);
/// Largest JSON body we accept from GitHub.
const MAX_JSON_BODY: usize = 48 * 1024 * 1024;
/// Largest error body we read (only to extract `message`).
const MAX_ERROR_BODY: usize = 64 * 1024;
/// Longest wait for a rate limit inside a request; longer ones fail fast.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(20);
/// Response cache bounds.
const CACHE_MAX_BYTES: usize = 96 * 1024 * 1024;
const CACHE_MAX_ENTRY: usize = 8 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 3000;
/// Requests kept in reserve: the poller stops below this many.
const RESERVE_ANON: u64 = 12;
const RESERVE_AUTH: u64 = 150;

/// How long a cached GET answers without asking GitHub again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    /// Things that move (runs, pull requests, checks).
    Live,
    /// Things that rarely change (repository settings, workflows, releases).
    Slow,
    /// Content addressed by a commit (files at a sha, merge bases).
    Fixed,
}

impl Fresh {
    fn ttl(self, anonymous: bool) -> Duration {
        match (self, anonymous) {
            (Fresh::Live, false) => Duration::from_secs(3),
            // Anonymous: at least as long as the UI and the poller wait between
            // refreshes (an anonymous 304 still costs one of 60 requests an hour).
            (Fresh::Live, true) => Duration::from_secs(300),
            (Fresh::Slow, false) => Duration::from_secs(60),
            (Fresh::Slow, true) => Duration::from_secs(900),
            (Fresh::Fixed, _) => Duration::from_secs(24 * 3600),
        }
    }
}

struct CacheEntry {
    etag: Option<String>,
    body: Bytes,
    next: Option<String>,
    last_page: Option<u32>,
    at: Instant,
}

#[derive(Default)]
struct RespCache {
    map: HashMap<String, CacheEntry>,
    bytes: usize,
}

impl RespCache {
    fn insert(&mut self, key: String, e: CacheEntry) {
        // Whatever was cached under this key is outdated now.
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= old.body.len();
        }
        if e.body.len() > CACHE_MAX_ENTRY {
            return;
        }
        self.bytes += e.body.len();
        self.map.insert(key, e);
        while (self.bytes > CACHE_MAX_BYTES || self.map.len() > CACHE_MAX_ENTRIES) && !self.map.is_empty() {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) else { break };
            if let Some(old) = self.map.remove(&oldest) {
                self.bytes -= old.body.len();
            }
        }
    }

    fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let bytes = &mut self.bytes;
        self.map.retain(|k, e| {
            let k2 = keep(k);
            if !k2 {
                *bytes -= e.body.len();
            }
            k2
        });
    }
}

/// Slice state, one per server (`AppState::github`).
#[derive(Default)]
pub struct GithubState {
    http: OnceLock<reqwest::Client>,
    meta: Mutex<HashMap<String, (Instant, Arc<RepoMeta>)>>,
    /// Repositories GitHub answered 404 for, keyed like `meta`.
    missing: Mutex<HashMap<String, Instant>>,
    /// When the UI or an agent last asked for a repository's GitHub data (by scope key).
    viewed: Mutex<HashMap<String, Instant>>,
    rate: Mutex<HashMap<String, RateInfo>>,
    cache: Mutex<RespCache>,
    viewers: Mutex<HashMap<String, (Instant, Option<String>)>>,
    /// Short-lived summaries keyed by `Project::scope_key` (dedupes several open tabs).
    pub(super) summaries: Mutex<HashMap<String, (Instant, Value)>>,
    pub(super) poll: PollState,
    /// Downloaded logs of finished jobs (they never change), newest last.
    pub(super) logs: Mutex<Vec<(String, Arc<super::logs::JobLogData>)>>,
    /// `api|base..head` → merge base (commits never change).
    pub(super) merge_bases: Mutex<HashMap<String, String>>,
}

/// What the last response said about one rate limit.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateInfo {
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    /// Epoch seconds when the window resets.
    pub reset: Option<i64>,
    pub used: Option<u64>,
}

impl RateInfo {
    fn exhausted_until(&self) -> Option<i64> {
        match (self.remaining, self.reset) {
            (Some(0), Some(r)) if r > chrono::Utc::now().timestamp() => Some(r),
            _ => None,
        }
    }
}

impl GithubState {
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

    fn note_rate(&self, prefix: &str, headers: &HeaderMap) {
        let num = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<i64>().ok());
        let Some(remaining) = num("x-ratelimit-remaining") else { return };
        let resource = headers.get("x-ratelimit-resource").and_then(|v| v.to_str().ok()).unwrap_or("core");
        let info = RateInfo {
            limit: num("x-ratelimit-limit").map(|v| v.max(0) as u64),
            remaining: Some(remaining.max(0) as u64),
            reset: num("x-ratelimit-reset"),
            used: num("x-ratelimit-used").map(|v| v.max(0) as u64),
        };
        self.rate.lock().insert(format!("{prefix}|{resource}"), info);
    }

    pub(super) fn rate(&self, prefix: &str, resource: &str) -> Option<RateInfo> {
        self.rate.lock().get(&format!("{prefix}|{resource}")).copied()
    }

    /// Forget the cached summary of a repository (after a mutation); `scope` is its
    /// `Project::scope_key`.
    pub(super) fn invalidate_summary(&self, scope: &str) {
        self.summaries.lock().remove(scope);
    }

    /// Someone (a browser tab, an agent) asked for this repository's GitHub data.
    pub(super) fn note_viewed(&self, scope: &str) {
        self.viewed.lock().insert(scope.to_string(), Instant::now());
    }

    /// Whether the repository's GitHub data was asked for within `within`.
    pub(super) fn viewed_within(&self, scope: &str, within: Duration) -> bool {
        self.viewed.lock().get(scope).is_some_and(|t| t.elapsed() < within)
    }
}

/// Repository facts from `GET /repos/{owner}/{repo}`, cached for `META_TTL`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct RepoMeta {
    pub id: u64,
    pub node_id: String,
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub private: bool,
    pub visibility: Option<String>,
    pub html_url: String,
    pub default_branch: Option<String>,
    /// Issues **and** pull requests.
    pub open_issues_count: u64,
    pub has_issues: bool,
    pub archived: bool,
    pub fork: bool,
    pub allow_merge_commit: Option<bool>,
    pub allow_squash_merge: Option<bool>,
    pub allow_rebase_merge: Option<bool>,
    pub delete_branch_on_merge: Option<bool>,
    /// Only for authenticated requests.
    pub permissions: Option<Permissions>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Permissions {
    pub admin: bool,
    pub maintain: bool,
    pub push: bool,
    pub triage: bool,
    pub pull: bool,
}

/// `(api base, web base, graphql url)` for a configured host (`github.com`,
/// `ghe.example.com` or `http://host:port`).
pub fn base_urls(host: &str) -> ApiResult<(String, String, String)> {
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        return Err(ApiError::not_configured("the GitHub host is empty"));
    }
    let web = if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("https://{host}")
    };
    let authority = web.split_once("://").map(|(_, r)| r).unwrap_or("");
    if authority.is_empty() || authority.contains(['@', '/', '?', '#']) || authority.contains(char::is_whitespace) {
        return Err(ApiError::bad_request("the GitHub host must be a host name (no credentials, path or spaces)"));
    }
    let web = web.to_ascii_lowercase();
    if web == "https://github.com" || web == "https://api.github.com" || web == "https://www.github.com" {
        return Ok((
            "https://api.github.com".into(),
            "https://github.com".into(),
            "https://api.github.com/graphql".into(),
        ));
    }
    Ok((format!("{web}/api/v3"), web.clone(), format!("{web}/api/graphql")))
}

/// `github.com` from `https://github.com` (for display).
pub fn display_host(web: &str) -> String {
    let h = web.trim().trim_end_matches('/');
    h.split_once("://").map(|(_, r)| r).unwrap_or(h).to_ascii_lowercase()
}

/// `scheme://authority` of a URL (for same-origin checks).
pub fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[..scheme.len() + 3 + end])
}

/// `owner/repo` → `(owner, repo)`: exactly two segments of GitHub's name characters.
pub fn valid_repo_path(path: &str) -> ApiResult<(String, String)> {
    let path = path.trim().trim_end_matches(".git").trim_matches('/');
    let ok_seg = |s: &str| {
        !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match path.split_once('/') {
        Some((o, r)) if ok_seg(o) && ok_seg(r) => Ok((o.to_string(), r.to_string())),
        _ => Err(ApiError::bad_request(format!("{path:?} is not a GitHub owner/repository"))),
    }
}

/// How requests authenticate.
#[derive(Clone)]
pub enum Auth {
    Token(Secret),
    /// Public, read-only access; the reason says why there is no token.
    Anonymous(String),
}

/// Resolve the token for a project on the GitHub instance at `web`.
fn auth_for(state: &AppState, project: &Project, web: &str) -> ApiResult<Auth> {
    let own = project
        .config
        .repo
        .as_ref()
        .and_then(|r| r.github.as_ref())
        .map(|g| g.token.trim().to_string())
        .filter(|t| !t.is_empty());
    if let Some(name) = own {
        return state.secret(Some(project), &name).map(Auth::Token);
    }
    let global = state.config.read().github.clone();
    match global {
        Some(g) if !g.token.trim().is_empty() => {
            let (_, gweb, _) = base_urls(&g.host)?;
            if gweb != web {
                return Ok(Auth::Anonymous(format!(
                    "the global [github] token is for {}, not {}; set repo.github.token in this project's overlay",
                    display_host(&gweb),
                    display_host(web)
                )));
            }
            // The owner's own setting for this host: resolved from config.toml.
            state.secret(None, g.token.trim()).map(Auth::Token)
        }
        _ => Ok(Auth::Anonymous(
            "no GitHub token is set up: add [github] token = \"github\" and [secrets] github = { file = \"~/.github_token\" } to config.toml".into(),
        )),
    }
}

/// A connection to one GitHub instance with one identity.
#[derive(Clone)]
pub struct Conn {
    pub state: AppState,
    /// Display host (`github.com`).
    pub host: String,
    /// `https://api.github.com`
    pub api: String,
    /// `https://github.com`
    pub web: String,
    pub graphql_url: String,
    pub auth: Auth,
    /// Rate-limit and cache namespace: API base plus a token fingerprint or `anon`.
    ident: String,
    http: reqwest::Client,
}

/// A connection bound to one project's repository.
#[derive(Clone)]
pub struct GhCtx {
    pub conn: Conn,
    pub project: Arc<Project>,
    pub owner: String,
    pub repo: String,
    pub meta: Arc<RepoMeta>,
}

impl std::ops::Deref for GhCtx {
    type Target = Conn;
    fn deref(&self) -> &Conn {
        &self.conn
    }
}

/// One page of a list endpoint: the parsed body and the pagination links.
#[derive(Debug)]
pub struct Page<T> {
    pub body: T,
    pub next_url: Option<String>,
    /// `page=` of `rel="last"` (so with `per_page=1`, the number of items).
    pub last_page: Option<u32>,
}

pub fn conn_for(state: &AppState, project: &Project, host: &str) -> ApiResult<Conn> {
    let (api, web, graphql_url) = base_urls(host)?;
    let auth = auth_for(state, project, &web)?;
    let ident = match &auth {
        Auth::Token(t) => {
            let d = sha2::Sha256::digest(t.expose().as_bytes());
            format!("{api}|t:{}", hex::encode(&d[..8]))
        }
        Auth::Anonymous(_) => format!("{api}|anon"),
    };
    Ok(Conn { state: state.clone(), host: display_host(&web), api, web, graphql_url, auth, ident, http: state.github.http() })
}

/// Connection + GitHub repository for a Workbench project id, seen through the git
/// repository `repo` names (`?repo=`; none: the default one, `404 unknown_repo` for an id
/// it does not have). REST handlers and MCP tools; it also marks the repository as
/// looked at, for the poller.
pub async fn ctx(state: &AppState, pid: &str, repo: &RepoParam) -> ApiResult<GhCtx> {
    let project = state.projects.require_repo(pid, repo.id())?;
    state.github.note_viewed(&project.scope_key());
    ctx_for(state, project).await
}

pub async fn ctx_for(state: &AppState, project: Arc<Project>) -> ApiResult<GhCtx> {
    let (host, path) = project.github().ok_or_else(|| {
        ApiError::not_configured("this project is not on GitHub (no GitHub remote and no [repo.github] section)")
    })?;
    let (owner, repo) = valid_repo_path(&path)?;
    let conn = conn_for(state, &project, &host)?;
    let meta = conn.repo_meta(&owner, &repo).await?;
    Ok(GhCtx { conn, project, owner, repo, meta })
}

impl GhCtx {
    /// `{api}/repos/{owner}/{repo}{rel}`
    pub fn rurl(&self, rel: &str) -> String {
        format!("{}/repos/{}/{}{}", self.api, self.owner, self.repo, rel)
    }

    pub fn default_branch(&self) -> Option<&str> {
        self.meta.default_branch.as_deref().filter(|b| !b.is_empty())
    }

    pub fn full_name(&self) -> String {
        if self.meta.full_name.is_empty() { format!("{}/{}", self.owner, self.repo) } else { self.meta.full_name.clone() }
    }

    /// A write (POST/PUT/PATCH/DELETE) with a JSON body. Needs a token; drops
    /// the repository's cached responses and the project's summary.
    pub async fn write<T: DeserializeOwned>(&self, method: Method, url: &str, body: Option<&Value>) -> ApiResult<T> {
        self.require_token("change anything on GitHub")?;
        let out = self.conn.write_raw(method, url, body).await;
        self.forget_cached();
        out
    }

    /// Drop cached responses of this repository (after a change here),
    /// including its metadata (open issue counts move with issues and PRs).
    pub fn forget_cached(&self) {
        let a = format!("/repos/{}/{}", self.owner, self.repo).to_ascii_lowercase();
        let b = format!("/repositories/{}/", self.meta.id);
        let keep = |k: &str| {
            let k = k.to_ascii_lowercase();
            !(k.contains(&format!("{a}/")) || k.contains(&format!("{a}?")) || k.ends_with(&a) || k.contains(&b))
        };
        self.state.github.cache.lock().retain(keep);
        self.state.github.meta.lock().remove(&self.meta_key(&self.owner, &self.repo));
        self.state.github.invalidate_summary(&self.project.scope_key());
    }

    /// Drop cached answers about one workflow run (its detail, its jobs, job
    /// details) and the checks of its commit: the poller saw it change, so the
    /// views that refetch on the event must not get an older copy.
    pub fn forget_run(&self, run_id: u64, head_sha: &str) {
        let repo = format!("/repos/{}/{}", self.owner, self.repo).to_ascii_lowercase();
        let run = format!("{repo}/actions/runs/{run_id}");
        let run_by_id = format!("/repositories/{}/actions/runs/{run_id}", self.meta.id);
        let jobs = format!("{repo}/actions/jobs/");
        let sha = head_sha.to_ascii_lowercase();
        let commit = format!("{repo}/commits/{sha}/");
        let by_sha = format!("head_sha={sha}");
        let under = |k: &str, p: &str| k.find(p).is_some_and(|i| matches!(k.as_bytes().get(i + p.len()), None | Some(b'/' | b'?')));
        let keep = |k: &str| {
            let k = k.to_ascii_lowercase();
            let stale = under(&k, &run)
                || under(&k, &run_by_id)
                || k.contains(&jobs)
                || (!sha.is_empty() && (k.contains(&commit) || k.contains(&by_sha)));
            !stale
        };
        self.state.github.cache.lock().retain(keep);
    }

    /// The login of the token's user (`None` when anonymous or unknown).
    pub async fn viewer(&self) -> Option<String> {
        if self.is_anonymous() {
            return None;
        }
        if let Some((at, v)) = self.state.github.viewers.lock().get(&self.ident) {
            if at.elapsed() < Duration::from_secs(3600) {
                return v.clone();
            }
        }
        let login = self
            .get::<super::model::User>(&format!("{}/user", self.api), &[], Fresh::Slow)
            .await
            .ok()
            .map(|u| u.login)
            .filter(|l| !l.is_empty());
        self.state.github.viewers.lock().insert(self.ident.clone(), (Instant::now(), login.clone()));
        login
    }
}

impl Conn {
    pub fn is_anonymous(&self) -> bool {
        matches!(self.auth, Auth::Anonymous(_))
    }

    /// Why there is no token (anonymous connections only).
    pub fn anonymous_reason(&self) -> Option<&str> {
        match &self.auth {
            Auth::Anonymous(r) => Some(r),
            Auth::Token(_) => None,
        }
    }

    pub fn require_token(&self, what: &str) -> ApiResult<()> {
        match &self.auth {
            Auth::Token(_) => Ok(()),
            Auth::Anonymous(reason) => Err(ApiError::not_configured(format!(
                "a GitHub token is needed to {what} ({reason})"
            ))),
        }
    }

    pub fn is_github_com(&self) -> bool {
        self.web == "https://github.com"
    }

    /// The last rate-limit state seen for this connection (`core` or `search`).
    pub fn rate(&self, resource: &str) -> Option<RateInfo> {
        self.state.github.rate(&self.ident, resource)
    }

    /// Nearly out of requests for this window (background work should wait).
    pub fn rate_low(&self) -> bool {
        let reserve = if self.is_anonymous() { RESERVE_ANON } else { RESERVE_AUTH };
        match self.rate("core") {
            Some(RateInfo { remaining: Some(r), reset: Some(reset), .. }) => {
                r < reserve && reset > chrono::Utc::now().timestamp()
            }
            _ => false,
        }
    }

    /// Key of `owner/repo` in the metadata maps (per API and identity).
    fn meta_key(&self, owner: &str, repo: &str) -> String {
        format!("{}|{owner}/{repo}", self.ident).to_ascii_lowercase()
    }

    /// The error for a repository GitHub does not show us. Without a token
    /// that is what a private repository looks like, so it is a setup problem
    /// (`not_configured`, which the UI shows as setup help), not a failure.
    fn missing_repo(&self, owner: &str, repo: &str) -> ApiError {
        match &self.auth {
            Auth::Anonymous(reason) => ApiError::not_configured(format!(
                "GitHub repository {owner}/{repo} is private or does not exist on {}: private repositories need a GitHub token ({reason})",
                self.host
            )),
            Auth::Token(_) => ApiError::not_found(format!(
                "GitHub repository {owner}/{repo} was not found on {}, or this token cannot see it",
                self.host
            )),
        }
    }

    /// Metadata for `owner/repo` (cached; a 404 is remembered too).
    async fn repo_meta(&self, owner: &str, repo: &str) -> ApiResult<Arc<RepoMeta>> {
        let key = self.meta_key(owner, repo);
        if let Some((at, m)) = self.state.github.meta.lock().get(&key) {
            if at.elapsed() < META_TTL {
                return Ok(m.clone());
            }
        }
        let missing_ttl = if self.is_anonymous() { MISSING_TTL_ANON } else { MISSING_TTL_AUTH };
        if self.state.github.missing.lock().get(&key).is_some_and(|at| at.elapsed() < missing_ttl) {
            return Err(self.missing_repo(owner, repo));
        }
        let url = format!("{}/repos/{owner}/{repo}", self.api);
        let meta: RepoMeta = match self.get(&url, &[], Fresh::Slow).await {
            Ok(m) => m,
            Err(e) if e.status == StatusCode::NOT_FOUND => {
                let mut missing = self.state.github.missing.lock();
                missing.retain(|_, at| at.elapsed() < MISSING_TTL_ANON);
                missing.insert(key, Instant::now());
                return Err(self.missing_repo(owner, repo));
            }
            Err(e) => return Err(e),
        };
        self.state.github.missing.lock().remove(&key);
        if meta.id == 0 {
            return Err(ApiError::upstream("GitHub returned a repository without an id"));
        }
        let meta = Arc::new(meta);
        self.state.github.meta.lock().insert(key, (Instant::now(), meta.clone()));
        Ok(meta)
    }

    fn same_origin(&self, url: &str) -> bool {
        origin(url).is_some_and(|o| Some(o) == origin(&self.api))
    }

    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let rb = self.http.request(method, url).header("X-GitHub-Api-Version", "2022-11-28").timeout(API_TIMEOUT);
        match &self.auth {
            Auth::Token(t) => rb.header(AUTHORIZATION, format!("Bearer {}", t.expose())),
            Auth::Anonymous(_) => rb,
        }
    }

    /// Send a built request; GitHub's JSON media type unless the caller chose one
    /// (`header` appends, so the default cannot be set up front).
    async fn execute(&self, rb: RequestBuilder) -> ApiResult<Response> {
        let mut req = rb.build().map_err(|e| ApiError::internal(format!("bad GitHub request: {}", e.without_url())))?;
        if !req.headers().contains_key(ACCEPT) {
            req.headers_mut().insert(ACCEPT, reqwest::header::HeaderValue::from_static("application/vnd.github+json"));
        }
        self.http.execute(req).await.map_err(|e| self.net_error(e))
    }

    fn net_error(&self, e: reqwest::Error) -> ApiError {
        if e.is_timeout() {
            ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timeout", format!("{} did not answer in time", self.host))
        } else {
            ApiError::upstream(format!("cannot reach {}: {}", self.host, e.without_url()))
        }
    }

    fn resource_of(url: &str) -> &'static str {
        if url.contains("/search/") {
            "search"
        } else if url.ends_with("/graphql") {
            "graphql"
        } else {
            "core"
        }
    }

    /// The error for a rate limit that lasts `wait` more.
    fn rate_error(&self, wait: Duration) -> ApiError {
        let mins = wait.as_secs().div_ceil(60).max(1);
        let hint = if self.is_anonymous() {
            " Without a token GitHub allows 60 requests an hour; a token raises that to 5000."
        } else {
            ""
        };
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            format!("{} rate limit reached; it resets in {mins} min.{hint}", self.host),
        )
    }

    /// Send a request (with `build` applied), waiting out short rate limits and
    /// retrying a transient 502/503/504 once for idempotent methods. Follows
    /// same-origin redirects with the token and cross-origin ones (GET only)
    /// without it. Returns the response whatever its status.
    pub async fn send_raw(
        &self,
        method: Method,
        url: &str,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
    ) -> ApiResult<Response> {
        if !self.same_origin(url) {
            return Err(ApiError::internal("refusing to send a GitHub request to another origin"));
        }
        let resource = Self::resource_of(url);
        if let Some(reset) = self.rate(resource).and_then(|r| r.exhausted_until()) {
            let wait = (reset - chrono::Utc::now().timestamp()).max(1) as u64;
            return Err(self.rate_error(Duration::from_secs(wait)));
        }
        let idempotent = matches!(method, Method::GET | Method::HEAD | Method::PUT | Method::DELETE);
        let mut attempt = 0u32;
        loop {
            let resp = self.execute(build(self.request(method.clone(), url))).await?;
            self.state.github.note_rate(&self.ident, resp.headers());
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN {
                if let Some(wait) = limit_wait(status, resp.headers()) {
                    if attempt < 2 && wait <= MAX_RETRY_WAIT {
                        attempt += 1;
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Err(self.rate_error(wait));
                }
            }
            if idempotent
                && attempt == 0
                && matches!(status, StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT)
            {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(700)).await;
                continue;
            }
            if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
                if method == Method::GET {
                    return self.follow_redirects(url, resp).await;
                }
                // A moved repository answers writes with 307/308: repeat on the same origin.
                if matches!(status, StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT) {
                    let next = resp
                        .headers()
                        .get(LOCATION)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|l| resolve_location(url, l));
                    if let Some(next) = next.filter(|n| self.same_origin(n)) {
                        let resp = self.execute(build(self.request(method.clone(), &next))).await?;
                        self.state.github.note_rate(&self.ident, resp.headers());
                        return Ok(resp);
                    }
                }
                return Err(ApiError::upstream("GitHub redirected this request elsewhere; the repository may have moved"));
            }
            return Ok(resp);
        }
    }

    async fn follow_redirects(&self, from: &str, mut resp: Response) -> ApiResult<Response> {
        let mut current = from.to_string();
        for _ in 0..5 {
            if !resp.status().is_redirection() || resp.status() == StatusCode::NOT_MODIFIED {
                return Ok(resp);
            }
            let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok()) else {
                return Ok(resp);
            };
            let next = resolve_location(&current, loc).ok_or_else(|| ApiError::upstream("GitHub sent a bad redirect"))?;
            resp = if self.same_origin(&next) {
                let r = self.execute(self.request(Method::GET, &next)).await?;
                self.state.github.note_rate(&self.ident, r.headers());
                r
            } else {
                // Signed storage URL (job logs): never send the token there.
                self.http.get(&next).timeout(Duration::from_secs(600)).send().await.map_err(|e| self.net_error(e))?
            };
            current = next;
        }
        Err(ApiError::upstream("too many redirects from GitHub"))
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

    /// Map a failed response to an `ApiError`. Only GitHub's `message`/`errors`
    /// fields are used (never the raw body), and the token is redacted.
    pub async fn error_from(&self, resp: Response) -> ApiError {
        let status = resp.status();
        let body = read_limited(resp, MAX_ERROR_BODY).await.unwrap_or_default();
        let msg = extract_message(&body).map(|m| match &self.auth {
            Auth::Token(t) => crate::secrets::redact(&m, std::slice::from_ref(t)),
            Auth::Anonymous(_) => m,
        });
        map_status(status, msg, &self.host, self.anonymous_reason())
    }

    /// Parse a JSON body (bounded).
    pub async fn parse<T: DeserializeOwned>(&self, resp: Response) -> ApiResult<T> {
        let bytes = read_limited(resp, MAX_JSON_BODY).await?;
        self.parse_bytes(&bytes)
    }

    fn parse_bytes<T: DeserializeOwned>(&self, bytes: &[u8]) -> ApiResult<T> {
        serde_json::from_slice(bytes).map_err(|e| ApiError::upstream(format!("unexpected response from {}: {e}", self.host)))
    }

    /// A cached GET (see the module docs). Returns the body and pagination links.
    async fn get_cached(&self, url: &str, query: &[(&str, String)], fresh: Fresh) -> ApiResult<Page<Bytes>> {
        let full = reqwest::Url::parse_with_params(url, query.iter().map(|(k, v)| (*k, v.as_str())))
            .map_err(|e| ApiError::internal(format!("bad GitHub URL: {e}")))?
            .to_string();
        let key = format!("{}|{full}", self.ident);
        let ttl = fresh.ttl(self.is_anonymous());
        let (etag, stale) = {
            let cache = self.state.github.cache.lock();
            match cache.map.get(&key) {
                Some(e) if e.at.elapsed() < ttl => {
                    return Ok(Page { body: e.body.clone(), next_url: e.next.clone(), last_page: e.last_page });
                }
                Some(e) => (e.etag.clone(), Some(Page { body: e.body.clone(), next_url: e.next.clone(), last_page: e.last_page })),
                None => (None, None),
            }
        };
        let resp = match self
            .send_raw(Method::GET, &full, |rb| match &etag {
                Some(e) => rb.header(IF_NONE_MATCH, e),
                None => rb,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // Out of quota: an older answer beats none.
                if e.code == "rate_limited" {
                    if let Some(page) = stale {
                        return Ok(page);
                    }
                }
                return Err(e);
            }
        };
        if resp.status() == StatusCode::NOT_MODIFIED {
            if let Some(page) = stale {
                if let Some(e) = self.state.github.cache.lock().map.get_mut(&key) {
                    e.at = Instant::now();
                }
                return Ok(page);
            }
            return Err(ApiError::upstream("GitHub answered 304 for an uncached request"));
        }
        if !resp.status().is_success() {
            let err = self.error_from(resp).await;
            if err.code == "rate_limited" {
                if let Some(page) = stale {
                    return Ok(page);
                }
            }
            return Err(err);
        }
        let headers = resp.headers().clone();
        let body = Bytes::from(read_limited(resp, MAX_JSON_BODY).await?);
        let next = link_rel(&headers, "next").filter(|u| self.same_origin(u));
        let last_page = link_rel(&headers, "last").and_then(|u| page_param(&u));
        let etag = headers.get(ETAG).and_then(|v| v.to_str().ok()).map(str::to_string);
        self.state.github.cache.lock().insert(
            key,
            CacheEntry { etag, body: body.clone(), next: next.clone(), last_page, at: Instant::now() },
        );
        Ok(Page { body, next_url: next, last_page })
    }

    pub async fn get<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)], fresh: Fresh) -> ApiResult<T> {
        let page = self.get_cached(url, query, fresh).await?;
        self.parse_bytes(&page.body)
    }

    /// GET that maps 404 to `None`.
    pub async fn get_opt<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)], fresh: Fresh) -> ApiResult<Option<T>> {
        match self.get_cached(url, query, fresh).await {
            Ok(page) => self.parse_bytes(&page.body).map(Some),
            Err(e) if e.status == StatusCode::NOT_FOUND => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// One page of a list endpoint.
    pub async fn get_page<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)], fresh: Fresh) -> ApiResult<Page<T>> {
        let page = self.get_cached(url, query, fresh).await?;
        Ok(Page { body: self.parse_bytes(&page.body)?, next_url: page.next_url, last_page: page.last_page })
    }

    /// Every item of a list endpoint (following `Link rel="next"`), up to `cap`.
    /// `field` names the array inside a wrapping object (`workflow_runs`, `jobs`…).
    /// The bool is true when items were left out because of the cap.
    pub async fn get_all<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
        cap: usize,
        field: Option<&str>,
        fresh: Fresh,
    ) -> ApiResult<(Vec<T>, bool)> {
        let mut q: Vec<(&str, String)> = query.to_vec();
        if !q.iter().any(|(k, _)| *k == "per_page") {
            q.push(("per_page", "100".into()));
        }
        let mut page = self.get_page::<Value>(url, &q, fresh).await?;
        let mut out: Vec<T> = Vec::new();
        loop {
            let items = match field {
                Some(f) => page.body.get_mut(f).map(Value::take).unwrap_or(Value::Array(vec![])),
                None => page.body,
            };
            let items: Vec<T> = serde_json::from_value(items)
                .map_err(|e| ApiError::upstream(format!("unexpected response from {}: {e}", self.host)))?;
            out.extend(items);
            if out.len() >= cap {
                let more = out.len() > cap || page.next_url.is_some();
                out.truncate(cap);
                return Ok((out, more));
            }
            match page.next_url {
                Some(next) => page = self.get_page::<Value>(&next, &[], fresh).await?,
                None => return Ok((out, false)),
            }
        }
    }

    /// Raw bytes of a GET (bounded to `cap`; the bool says it was larger), uncached.
    pub async fn get_bytes(&self, url: &str, accept: &str, cap: usize) -> ApiResult<Option<(Vec<u8>, bool)>> {
        let resp = self.send_raw(Method::GET, url, |rb| rb.header(ACCEPT, accept)).await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(self.error_from(resp).await);
        }
        let (bytes, total, _) = read_head(resp, cap).await?;
        Ok(Some((bytes, total > cap as u64)))
    }

    /// github.com without a token only: a file of a public repository from
    /// raw.githubusercontent.com, which does not count against the API quota.
    /// Never authenticated. `(bytes, larger than cap)`; `None` when missing.
    pub async fn raw_public(&self, owner: &str, repo: &str, sha: &str, path: &str, cap: usize) -> ApiResult<Option<(Vec<u8>, bool)>> {
        if !(self.is_anonymous() && self.is_github_com()) {
            return Err(ApiError::internal("raw.githubusercontent.com is only for anonymous github.com access"));
        }
        let enc: Vec<String> = path.split('/').map(|s| urlencoding::encode(s).into_owned()).collect();
        let url = format!("https://raw.githubusercontent.com/{owner}/{repo}/{sha}/{}", enc.join("/"));
        let resp = self.http.get(&url).timeout(API_TIMEOUT).send().await.map_err(|e| self.net_error(e))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(ApiError::upstream(format!("raw.githubusercontent.com returned {}", resp.status().as_u16())));
        }
        let (bytes, total, _) = read_head(resp, cap).await?;
        Ok(Some((bytes, total > cap as u64)))
    }

    /// A write with a JSON body; returns the parsed response (`Value::Null` when empty).
    pub async fn write_raw<T: DeserializeOwned>(&self, method: Method, url: &str, body: Option<&Value>) -> ApiResult<T> {
        let resp = self
            .send(method, url, |rb| match body {
                Some(b) => rb.json(b),
                None => rb,
            })
            .await?;
        let bytes = read_limited(resp, MAX_JSON_BODY).await?;
        let bytes: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) { b"null" } else { &bytes };
        self.parse_bytes(bytes)
    }

    /// A GraphQL request (queries and mutations). Needs a token.
    pub async fn graphql(&self, query: &str, variables: Value) -> ApiResult<Value> {
        self.require_token("use GitHub's GraphQL API")?;
        let url = self.graphql_url.clone();
        let payload = json!({ "query": query, "variables": variables });
        let resp = self.send(Method::POST, &url, |rb| rb.json(&payload)).await?;
        let mut v: Value = self.parse(resp).await?;
        let first_error = v.pointer("/errors/0/message").and_then(Value::as_str).map(|m| m.chars().take(300).collect::<String>());
        if v.get("data").is_none_or(Value::is_null) || first_error.is_some() {
            let msg = first_error.unwrap_or_else(|| "GraphQL request failed".into());
            let lower = msg.to_ascii_lowercase();
            if lower.contains("not have permission") || lower.contains("forbidden") || lower.contains("must have") {
                return Err(ApiError::forbidden(format!("GitHub: {msg}")));
            }
            if lower.contains("could not resolve") || lower.contains("not found") {
                return Err(ApiError::not_found(format!("GitHub: {msg}")));
            }
            return Err(ApiError::upstream(format!("GitHub GraphQL: {msg}")));
        }
        Ok(v["data"].take())
    }
}

/// How long to wait for a rate limit, when this 403/429 is one.
fn limit_wait(status: StatusCode, h: &HeaderMap) -> Option<Duration> {
    let num = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<i64>().ok());
    if let Some(d) = retry_after(h) {
        return Some(d);
    }
    if num("x-ratelimit-remaining") == Some(0) {
        let reset = num("x-ratelimit-reset").unwrap_or(0);
        return Some(Duration::from_secs((reset - chrono::Utc::now().timestamp()).max(1) as u64));
    }
    // A bare 429 is a secondary limit: GitHub asks for at least a minute.
    (status == StatusCode::TOO_MANY_REQUESTS).then_some(Duration::from_secs(60))
}

/// Read at most `cap` bytes of a body; larger bodies are an error.
pub async fn read_limited(mut resp: Response, cap: usize) -> ApiResult<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| ApiError::upstream(e.without_url().to_string()))? {
        if out.len() + chunk.len() > cap {
            return Err(ApiError::upstream(format!("GitHub response larger than {} MB", cap / (1024 * 1024))));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Read the first `cap` bytes of a body and count the rest.
/// Returns `(head, total_bytes, cut)`.
pub async fn read_head(mut resp: Response, cap: usize) -> ApiResult<(Vec<u8>, u64, bool)> {
    let mut out = Vec::new();
    let mut total = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| ApiError::upstream(e.without_url().to_string()))? {
        total += chunk.len() as u64;
        if out.len() < cap {
            let take = (cap - out.len()).min(chunk.len());
            out.extend_from_slice(&chunk[..take]);
        } else if total > (cap as u64) * 4 {
            break; // enough to know it is too large
        }
    }
    let cut = total > cap as u64;
    Ok((out, total, cut))
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

/// The URL of a `Link` relation (`next`, `last`…), if any.
pub fn link_rel(headers: &HeaderMap, rel: &str) -> Option<String> {
    headers.get_all(LINK).iter().filter_map(|v| v.to_str().ok()).find_map(|v| parse_link(v, rel))
}

pub fn parse_link(value: &str, want: &str) -> Option<String> {
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        let end = start + rest[start..].find('>')?;
        let url = &rest[start + 1..end];
        let after = &rest[end + 1..];
        let params_end = after.find('<').unwrap_or(after.len());
        let params = &after[..params_end];
        let matches = params.split(';').any(|p| {
            let p = p.trim().trim_end_matches(',').trim();
            let Some(v) = p.strip_prefix("rel=").or_else(|| p.strip_prefix("REL=")) else { return false };
            v.trim_matches('"').split_whitespace().any(|r| r.eq_ignore_ascii_case(want))
        });
        if matches {
            return Some(url.to_string());
        }
        rest = &after[params_end..];
    }
    None
}

/// `page=` of a URL's query.
pub fn page_param(url: &str) -> Option<u32> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.strip_prefix("page=")).and_then(|v| v.parse().ok())
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

/// GitHub's human message from an error body: `message` plus the `errors`
/// details (validation failures). Never the raw body.
pub fn extract_message(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let mut msg = v.get("message").and_then(Value::as_str)?.to_string();
    let details: Vec<String> = v
        .get("errors")
        .and_then(Value::as_array)
        .map(|errs| {
            errs.iter()
                .filter_map(|e| match e {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(_) => e.get("message").and_then(Value::as_str).map(str::to_string).or_else(|| {
                        let field = e.get("field").and_then(Value::as_str).unwrap_or("");
                        let code = e.get("code").and_then(Value::as_str).unwrap_or("");
                        (!code.is_empty()).then(|| format!("{field} {code}").trim().to_string())
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if !details.is_empty() {
        msg = format!("{msg}: {}", details.join("; "));
    }
    if msg.chars().count() > 500 {
        msg = msg.chars().take(500).collect::<String>() + "…";
    }
    Some(msg)
}

/// Map an upstream status to the right `ApiError` constructor.
pub fn map_status(status: StatusCode, msg: Option<String>, host: &str, anonymous: Option<&str>) -> ApiError {
    let m = msg.unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string());
    let lower = m.to_ascii_lowercase();
    match status.as_u16() {
        401 => match anonymous {
            Some(reason) => ApiError::not_configured(format!("GitHub needs a token for this ({reason})")),
            None => ApiError::not_configured(format!(
                "{host} rejected the GitHub token (401 Unauthorized); check the token secret and its expiry date"
            )),
        },
        403 | 429 if lower.contains("rate limit") => {
            ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", format!("GitHub: {m}"))
        }
        403 => ApiError::forbidden(format!("GitHub: {m}")),
        404 => ApiError::not_found(format!("GitHub: {m}")),
        // 405: not mergeable; 409: head moved or merge conflict; 412: precondition.
        405 | 409 | 412 => ApiError::conflict(format!("GitHub: {m}")),
        400 | 422 => ApiError::bad_request(format!("GitHub: {m}")),
        429 => ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", format!("GitHub: {m}")),
        _ => ApiError::upstream(format!("GitHub returned {}: {m}", status.as_u16())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_for_github_com_and_enterprise() {
        let (api, web, gql) = base_urls("github.com").unwrap();
        assert_eq!((api.as_str(), web.as_str(), gql.as_str()), ("https://api.github.com", "https://github.com", "https://api.github.com/graphql"));
        assert_eq!(base_urls("https://GitHub.com/").unwrap().0, "https://api.github.com");
        let (api, web, gql) = base_urls("ghe.example.com").unwrap();
        assert_eq!(api, "https://ghe.example.com/api/v3");
        assert_eq!(web, "https://ghe.example.com");
        assert_eq!(gql, "https://ghe.example.com/api/graphql");
        assert_eq!(base_urls("http://127.0.0.1:8930").unwrap().0, "http://127.0.0.1:8930/api/v3");
        // Plain HTTP to github.com is an Enterprise-style URL, not github.com: it never matches the global token's host.
        assert_eq!(base_urls("http://github.com").unwrap().1, "http://github.com");
        assert!(base_urls("https://user:pw@github.com").is_err());
        assert!(base_urls("github.com/evil").is_err());
        assert!(base_urls("").is_err());
        assert_eq!(display_host("https://GitHub.com/"), "github.com");
    }

    #[test]
    fn repo_paths_are_two_safe_segments() {
        assert_eq!(valid_repo_path("octo-org/hello-world").unwrap(), ("octo-org".into(), "hello-world".into()));
        assert_eq!(valid_repo_path("o/r.git").unwrap().1, "r");
        assert!(valid_repo_path("o/../x").is_err());
        assert!(valid_repo_path("a/b/c").is_err());
        assert!(valid_repo_path("o/..").is_err());
        assert!(valid_repo_path("o/r?x=1").is_err());
        assert!(valid_repo_path("justone").is_err());
    }

    #[test]
    fn link_headers() {
        let v = r#"<https://api.github.com/repositories/117356231/actions/runs?per_page=2&status=completed&page=2>; rel="next", <https://api.github.com/repositories/117356231/actions/runs?per_page=2&status=completed&page=135>; rel="last""#;
        assert_eq!(
            parse_link(v, "next").as_deref(),
            Some("https://api.github.com/repositories/117356231/actions/runs?per_page=2&status=completed&page=2")
        );
        assert_eq!(parse_link(v, "last").and_then(|u| page_param(&u)), Some(135));
        assert_eq!(parse_link("<https://x/a?page=1>; rel=\"prev\"", "next"), None);
        assert_eq!(parse_link("<broken", "next"), None);
        assert_eq!(page_param("https://x/a?per_page=1&page=17"), Some(17));
    }

    #[test]
    fn errors_are_extracted_not_echoed() {
        assert_eq!(
            extract_message(br#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"A pull request already exists for o:feature."}],"documentation_url":"x"}"#).as_deref(),
            Some("Validation Failed: A pull request already exists for o:feature.")
        );
        assert_eq!(
            extract_message(br#"{"message":"Validation Failed","errors":[{"resource":"Issue","field":"title","code":"missing_field"}]}"#).as_deref(),
            Some("Validation Failed: title missing_field")
        );
        assert_eq!(extract_message(b"<html>token abc</html>"), None);
        assert_eq!(extract_message(br#"{"other":"x"}"#), None);
    }

    #[test]
    fn statuses_map_to_error_kinds() {
        assert_eq!(map_status(StatusCode::UNAUTHORIZED, None, "github.com", None).code, "not_configured");
        assert_eq!(map_status(StatusCode::UNAUTHORIZED, None, "github.com", Some("no token")).code, "not_configured");
        assert_eq!(
            map_status(StatusCode::FORBIDDEN, Some("API rate limit exceeded for 1.2.3.4.".into()), "h", Some("x")).code,
            "rate_limited"
        );
        assert_eq!(map_status(StatusCode::FORBIDDEN, Some("Resource not accessible".into()), "h", None).code, "forbidden");
        assert_eq!(map_status(StatusCode::METHOD_NOT_ALLOWED, Some("Pull Request is not mergeable".into()), "h", None).code, "conflict");
        assert_eq!(map_status(StatusCode::CONFLICT, None, "h", None).code, "conflict");
        assert_eq!(map_status(StatusCode::UNPROCESSABLE_ENTITY, None, "h", None).code, "bad_request");
        assert_eq!(map_status(StatusCode::BAD_GATEWAY, None, "h", None).code, "upstream");
    }

    #[test]
    fn rate_limit_waits() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(limit_wait(StatusCode::FORBIDDEN, &h), Some(Duration::from_secs(7)));
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining", "0".parse().unwrap());
        h.insert("x-ratelimit-reset", (chrono::Utc::now().timestamp() + 600).to_string().parse().unwrap());
        assert!(limit_wait(StatusCode::FORBIDDEN, &h).unwrap() > MAX_RETRY_WAIT);
        // A permission 403 is not a rate limit.
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining", "4999".parse().unwrap());
        assert_eq!(limit_wait(StatusCode::FORBIDDEN, &h), None);
        assert_eq!(limit_wait(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new()), Some(Duration::from_secs(60)));
    }

    #[test]
    fn origins_and_redirects() {
        assert_eq!(origin("https://api.github.com/repos/o/r?x"), Some("https://api.github.com"));
        assert_eq!(resolve_location("https://api.github.com/a", "/b?c").as_deref(), Some("https://api.github.com/b?c"));
        assert_eq!(
            resolve_location("https://api.github.com/a", "https://results.blob.core.windows.net/x?sig=1").as_deref(),
            Some("https://results.blob.core.windows.net/x?sig=1")
        );
    }

    #[test]
    fn anonymous_answers_last_as_long_as_the_poller_waits() {
        // An anonymous 304 still costs one of 60 requests an hour: a cached answer
        // must outlast the anonymous poll interval while something runs (and the
        // summary is shared exactly as long), or every refresh reaches GitHub.
        assert!(Fresh::Live.ttl(true) >= super::super::poller::FAST_ANON);
        assert_eq!(super::super::misc::SUMMARY_TTL_ANON, Fresh::Live.ttl(true));
        assert!(Fresh::Slow.ttl(true) >= Fresh::Live.ttl(true));
    }

    #[test]
    fn cache_is_bounded() {
        let mut c = RespCache::default();
        c.insert("big".into(), CacheEntry { etag: Some("v1".into()), body: Bytes::from_static(b"old"), next: None, last_page: None, at: Instant::now() });
        let big = Bytes::from(vec![0u8; CACHE_MAX_ENTRY + 1]);
        c.insert("big".into(), CacheEntry { etag: None, body: big, next: None, last_page: None, at: Instant::now() });
        assert!(c.map.is_empty(), "oversized bodies are not cached, and the older entry is gone");
        assert_eq!(c.bytes, 0);
        for i in 0..(CACHE_MAX_ENTRIES + 5) {
            c.insert(format!("k{i}"), CacheEntry { etag: None, body: Bytes::from_static(b"x"), next: None, last_page: None, at: Instant::now() });
        }
        assert_eq!(c.map.len(), CACHE_MAX_ENTRIES);
        assert_eq!(c.bytes, CACHE_MAX_ENTRIES);
        c.retain(|k| k != "k10");
        assert_eq!(c.bytes, CACHE_MAX_ENTRIES - 1);
    }
}

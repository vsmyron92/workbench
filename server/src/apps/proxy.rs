//! Loopback preview proxy for environments the browser cannot frame directly:
//! sites behind basic auth, and sites whose headers forbid framing.
//!
//! `GET /api/projects/{pid}/envs/{name}/proxy-url` lazily starts one proxy per env on
//! `127.0.0.1:<ephemeral>` (plus `[::1]` on the same port when available) and returns a
//! one-time URL. The proxy
//! * forwards every request to the env's origin, injecting `Authorization: Basic …`
//!   (from the env's `auth.password` secret) except on `auth.except` paths or when the
//!   page sent its own `Authorization` (the app's Bearer token);
//! * strips `X-Frame-Options` and CSP `frame-ancestors`, rewrites `Location` headers
//!   that point at the upstream origin, and relaxes `Set-Cookie` (`Secure`, `Domain`)
//!   so the app's own cookies work over plain-http loopback;
//! * **namespaces cookies per environment.** Browsers do not separate cookies by port,
//!   so every proxy, Workbench itself and any other local web UI share one cookie jar
//!   for `127.0.0.1`. Upstream cookies are therefore stored as `wbp<hash>_<name>`
//!   (`hash` of project, env and origin), and only this env's cookies — prefix
//!   removed — are forwarded upstream. Another env's session, Workbench's own
//!   cookies and other local services' cookies never leave the machine, and an
//!   upstream cannot overwrite a Workbench cookie. (Scripts that read or write
//!   cookies through `document.cookie` see the prefixed names: open such apps in a
//!   new window instead.)
//! * only answers loopback `Host` names, and only browsers that redeemed a one-time
//!   token (10 min) for its HttpOnly cookie — other local processes cannot borrow the
//!   injected credentials.
//!
//! It is local-only: a remote device (phone over Tailscale) cannot reach the
//! loopback port, so `proxy-url` returns `url: null` and the UI offers "open in a new
//! window" instead. WebSocket upgrades are not proxied.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::TryStreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::health;
use crate::app::AppState;
use crate::error::ApiError;

const AUTH_PATH: &str = "/__workbench__/auth";
const TOKEN_TTL: Duration = Duration::from_secs(600);

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus `host`, which the client sets for the upstream.
/// `content-length` is kept both ways: a streamed body with a known length must not
/// turn into chunked encoding (many small servers cannot read chunked requests).
const HOP: &[&str] =
    &["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade", "host"];

struct ProxyCtx {
    state: AppState,
    pid: String,
    env: String,
    port: u16,
    /// Cookie value that proves the browser redeemed a token.
    session: String,
    tokens: Mutex<HashMap<String, Instant>>,
    client: reqwest::Client,
}

pub struct ProxyHandle {
    ctx: Arc<ProxyCtx>,
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct Proxies {
    by_env: tokio::sync::Mutex<HashMap<(String, String), ProxyHandle>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyUrl {
    /// One-time URL to load in the iframe; `null` when the caller is not local.
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Loopback host name the browser used to reach Workbench, if it is one.
fn loopback_host(headers: &HeaderMap) -> Option<&'static str> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let name = if let Some(rest) = host.strip_prefix('[') { rest.split(']').next()? } else { host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host) };
    match name {
        "127.0.0.1" => Some("127.0.0.1"),
        "localhost" => Some("localhost"),
        "::1" => Some("[::1]"),
        _ => None,
    }
}

/// Start (once) the proxy for an env and mint a one-time URL for `path`.
pub async fn proxy_url(state: &AppState, pid: &str, env: &str, path: &str, headers: &HeaderMap) -> Result<ProxyUrl, ApiError> {
    let project = state.projects.require(pid)?;
    let e = super::envs::find(&project, env)?;
    let upstream = reqwest::Url::parse(&e.url).map_err(|_| ApiError::bad_request(format!("{env}: url {:?} is not valid", e.url)))?;
    if !matches!(upstream.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(format!("{env}: only http(s) sites can be previewed")));
    }
    let Some(host) = loopback_host(headers) else {
        return Ok(ProxyUrl {
            url: None,
            port: None,
            reason: Some("The preview proxy only listens on this machine's loopback; open the site in a new window instead.".into()),
        });
    };
    let mut map = state.apps.proxies.by_env.lock().await;
    let k = (pid.to_string(), env.to_string());
    if map.get(&k).is_some_and(|h| h.cancel.is_cancelled()) {
        map.remove(&k);
    }
    if !map.contains_key(&k) {
        let h = spawn_proxy(state, pid, env).await?;
        map.insert(k.clone(), h);
    }
    let Some(h) = map.get(&k) else { return Err(ApiError::internal("proxy vanished")) };
    let token = crate::util::random_token(24);
    {
        let mut t = h.ctx.tokens.lock();
        t.retain(|_, at| at.elapsed() < TOKEN_TTL);
        if t.len() >= 64 {
            // Bounded: drop the oldest when a client keeps minting without redeeming.
            if let Some(oldest) = t.iter().min_by_key(|(_, at)| **at).map(|(k, _)| k.clone()) {
                t.remove(&oldest);
            }
        }
        t.insert(token.clone(), Instant::now());
    }
    let next = if path.starts_with('/') && !path.starts_with("//") { path } else { "/" };
    let url = format!("http://{host}:{}{AUTH_PATH}?t={token}&next={}", h.ctx.port, urlencoding::encode(next));
    Ok(ProxyUrl { url: Some(url), port: Some(h.ctx.port), reason: None })
}

async fn spawn_proxy(state: &AppState, pid: &str, env: &str) -> Result<ProxyHandle, ApiError> {
    let v4 = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| ApiError::internal(format!("proxy bind: {e}")))?;
    let port = v4.local_addr().map_err(|e| ApiError::internal(e.to_string()))?.port();
    // Same port on IPv6 loopback, for browsers that resolve `localhost` to ::1 first.
    let v6 = tokio::net::TcpListener::bind(SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))).await.ok();
    let client = reqwest::Client::builder()
        .user_agent(concat!("workbench-preview/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let ctx = Arc::new(ProxyCtx {
        state: state.clone(),
        pid: pid.to_string(),
        env: env.to_string(),
        port,
        session: crate::util::random_token(24),
        tokens: Mutex::new(HashMap::new()),
        client,
    });
    let cancel = state.apps.shutdown.child_token();
    let app = Router::new().fallback(handle).with_state(ctx.clone());
    for listener in std::iter::once(v4).chain(v6) {
        let app = app.clone();
        let c = cancel.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).with_graceful_shutdown(async move { c.cancelled().await }).await;
        });
    }
    tracing::info!(project = pid, env, port, "preview proxy started");
    Ok(ProxyHandle { ctx, cancel })
}

fn plain(status: StatusCode, msg: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Workbench preview</title><body style=\"font:14px system-ui;padding:24px\"><p>{}</p>",
        ammonia::clean_text(msg)
    );
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

fn cookie_name(port: u16) -> String {
    format!("wb_preview_{port}")
}

fn has_session(headers: &HeaderMap, name: &str, session: &str) -> bool {
    use subtle::ConstantTimeEq;
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .any(|(k, v)| k == name && bool::from(v.as_bytes().ct_eq(session.as_bytes())))
}

async fn handle(State(ctx): State<Arc<ProxyCtx>>, req: Request) -> Response {
    let Some(host) = loopback_host(req.headers()) else {
        return plain(StatusCode::FORBIDDEN, "This preview proxy only answers loopback addresses.");
    };
    let path_and_query = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
    let cname = cookie_name(ctx.port);

    if req.uri().path() == AUTH_PATH {
        let q: HashMap<String, String> = req
            .uri()
            .query()
            .map(|q| url_query(q))
            .unwrap_or_default();
        let token = q.get("t").cloned().unwrap_or_default();
        let valid = ctx.tokens.lock().remove(&token).is_some_and(|at| at.elapsed() < TOKEN_TTL);
        if !valid {
            return plain(StatusCode::FORBIDDEN, "This preview link has expired. Reload the preview from Workbench.");
        }
        let next = q.get("next").filter(|n| n.starts_with('/') && !n.starts_with("//")).cloned().unwrap_or_else(|| "/".into());
        let cookie = format!("{cname}={}; Path=/; HttpOnly; SameSite=Lax", ctx.session);
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, next)
            .header(header::SET_COOKIE, cookie)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::empty())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "bad redirect"));
    }
    if !has_session(req.headers(), &cname, &ctx.session) {
        return plain(StatusCode::FORBIDDEN, "Open this preview from Workbench (the link carries a one-time token).");
    }
    if req.headers().get(header::UPGRADE).is_some() {
        return plain(StatusCode::NOT_IMPLEMENTED, "WebSocket connections are not proxied; open the site in a new window.");
    }

    // Fresh config on every request: edits apply without restarting the proxy.
    let Some(project) = ctx.state.projects.get(&ctx.pid) else { return plain(StatusCode::NOT_FOUND, "project removed") };
    let Some(env) = project.config.envs.iter().find(|e| e.name == ctx.env).cloned() else {
        return plain(StatusCode::NOT_FOUND, "environment removed");
    };
    let Ok(base) = reqwest::Url::parse(&env.url) else { return plain(StatusCode::BAD_GATEWAY, "bad environment URL") };
    let origin = base.origin().ascii_serialization();
    let Ok(target) = reqwest::Url::parse(&format!("{origin}{path_and_query}")) else {
        return plain(StatusCode::BAD_REQUEST, "bad path");
    };
    let proxy_origin = format!("http://{host}:{}", ctx.port);
    let prefix = cookie_prefix(&ctx.pid, &ctx.env, &origin);

    let (parts, body) = req.into_parts();
    let mut out_headers = reqwest::header::HeaderMap::new();
    let has_own_auth = parts.headers.contains_key(header::AUTHORIZATION);
    let mut cookies: Vec<&str> = vec![];
    for (k, v) in parts.headers.iter() {
        let name = k.as_str();
        if HOP.contains(&name) {
            continue;
        }
        match name {
            // Only this env's cookies, under their real names (see the module docs).
            "cookie" => cookies.extend(forward_cookies(v.to_str().unwrap_or(""), &prefix)),
            "origin" | "referer" => {
                let s = v.to_str().unwrap_or("").replacen(&proxy_origin, &origin, 1);
                if let Ok(hv) = HeaderValue::from_str(&s) {
                    out_headers.append(k.clone(), hv);
                }
            }
            _ => {
                out_headers.append(k.clone(), v.clone());
            }
        }
    }
    if !cookies.is_empty() {
        if let Ok(hv) = HeaderValue::from_str(&cookies.join("; ")) {
            out_headers.insert(header::COOKIE, hv);
        }
    }
    let mut builder = ctx.client.request(parts.method.clone(), target.clone()).headers(out_headers);
    if let Some(auth) = &env.auth {
        if !has_own_auth && health::needs_auth(auth, target.path()) {
            match ctx.state.secret(Some(&project), &auth.password) {
                Ok(pw) => builder = builder.basic_auth(&auth.user, Some(pw.expose())),
                Err(e) => {
                    return plain(
                        StatusCode::BAD_GATEWAY,
                        &format!("Basic auth for {} is not set up: store the password as secret {:?}. ({})", env.name, auth.password, e.message),
                    );
                }
            }
        }
    }
    if !matches!(parts.method, axum::http::Method::GET | axum::http::Method::HEAD) {
        let stream = body.into_data_stream().map_err(std::io::Error::other);
        builder = builder.body(reqwest::Body::wrap_stream(stream));
    }
    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &format!("{} is unreachable: {}", env.name, super::envs::describe(&e, Duration::from_secs(120)))),
    };

    let status = resp.status();
    let mut rb = Response::builder().status(status);
    for (k, v) in resp.headers().iter() {
        let name = k.as_str();
        if HOP.contains(&name) {
            continue;
        }
        match name {
            "x-frame-options" | "strict-transport-security" => continue,
            "content-security-policy" | "content-security-policy-report-only" => {
                if let Some(csp) = v.to_str().ok().and_then(health::strip_frame_ancestors) {
                    if let Ok(hv) = HeaderValue::from_str(&csp) {
                        rb = rb.header(k, hv);
                    }
                }
            }
            "location" => {
                let loc = v.to_str().unwrap_or("");
                let rewritten = rewrite_location(loc, &origin, &proxy_origin);
                if let Ok(hv) = HeaderValue::from_str(&rewritten) {
                    rb = rb.header(k, hv);
                }
            }
            "set-cookie" => {
                if let Some(hv) = v.to_str().ok().and_then(|c| rewrite_set_cookie(c, &prefix)).and_then(|c| HeaderValue::from_str(&c).ok()) {
                    rb = rb.header(k, hv);
                }
            }
            _ => {
                rb = rb.header(HeaderName::from(k), v.clone());
            }
        }
    }
    let stream = resp.bytes_stream().map_err(std::io::Error::other);
    rb.body(Body::from_stream(stream)).unwrap_or_else(|_| plain(StatusCode::BAD_GATEWAY, "bad upstream response"))
}

fn url_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(v).map(|s| s.into_owned()).unwrap_or_default()))
        .collect()
}

/// Point redirects to the upstream origin back at the proxy.
pub fn rewrite_location(loc: &str, upstream_origin: &str, proxy_origin: &str) -> String {
    if let Some(rest) = loc.strip_prefix(upstream_origin) {
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') {
            return format!("{proxy_origin}{rest}");
        }
    }
    let proto_relative = upstream_origin.split_once("://").map(|(_, h)| format!("//{h}"));
    if let Some(rest) = proto_relative.as_deref().and_then(|p| loc.strip_prefix(p)) {
        if rest.is_empty() || rest.starts_with('/') {
            return format!("{proxy_origin}{rest}");
        }
    }
    loc.to_string()
}

/// Name prefix of one environment's cookies in the browser's `127.0.0.1` jar. Stable
/// across restarts (the proxy port is not), different for every project, env and
/// upstream origin.
pub fn cookie_prefix(pid: &str, env: &str, origin: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("{pid}\n{env}\n{origin}").as_bytes());
    format!("wbp{}_", &hex::encode(h)[..10])
}

/// Browser `Cookie` header → the cookies to send upstream: only those in this env's
/// namespace, with the prefix removed. Everything else in the shared jar (other envs,
/// Workbench, other local services) is dropped.
pub fn forward_cookies<'a>(header: &'a str, prefix: &str) -> Vec<&'a str> {
    header
        .split(';')
        .map(str::trim)
        .filter_map(|kv| kv.strip_prefix(prefix))
        .filter(|kv| kv.split_once('=').is_some_and(|(name, _)| !name.trim().is_empty()))
        .collect()
}

/// Upstream `Set-Cookie` → the browser: relaxed for plain-http loopback and renamed
/// into this env's namespace, so it can never replace a cookie of Workbench, of
/// another env or of another local service. Nameless cookies are dropped.
pub fn rewrite_set_cookie(c: &str, prefix: &str) -> Option<String> {
    let relaxed = relax_cookie(c);
    let (name, rest) = relaxed.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.contains(|ch: char| ch.is_whitespace() || ch.is_control()) {
        return None;
    }
    Some(format!("{prefix}{name}={rest}"))
}

/// Make an upstream cookie storable on `http://127.0.0.1:<port>`: drop `Secure` and
/// `Domain`, and `SameSite=None` (which requires Secure) becomes `Lax`.
pub fn relax_cookie(c: &str) -> String {
    c.split(';')
        .map(str::trim)
        .filter(|a| {
            let l = a.to_ascii_lowercase();
            l != "secure" && !l.starts_with("domain=") && l != "partitioned"
        })
        .map(|a| if a.eq_ignore_ascii_case("samesite=none") { "SameSite=Lax" } else { a })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Stop every proxy (shutdown).
pub async fn shutdown(state: &AppState) {
    for (_, h) in state.apps.proxies.by_env.lock().await.drain() {
        h.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_upstream_redirects_only() {
        let up = "https://staging.shop.example.com";
        let px = "http://127.0.0.1:41234";
        assert_eq!(rewrite_location("https://staging.shop.example.com/login?x=1", up, px), "http://127.0.0.1:41234/login?x=1");
        assert_eq!(rewrite_location("https://staging.shop.example.com", up, px), "http://127.0.0.1:41234");
        assert_eq!(rewrite_location("//staging.shop.example.com/a", up, px), "http://127.0.0.1:41234/a");
        assert_eq!(rewrite_location("/relative", up, px), "/relative");
        assert_eq!(rewrite_location("https://accounts.example.com/sso", up, px), "https://accounts.example.com/sso");
        assert_eq!(rewrite_location("https://staging.shop.example.com.evil.com/", up, px), "https://staging.shop.example.com.evil.com/");
    }

    #[test]
    fn relaxes_cookies_for_http_loopback() {
        assert_eq!(
            relax_cookie("sid=abc; Path=/; Domain=shop.example.com; Secure; HttpOnly; SameSite=None"),
            "sid=abc; Path=/; HttpOnly; SameSite=Lax"
        );
        assert_eq!(relax_cookie("a=b"), "a=b");
    }

    #[test]
    fn cookies_are_namespaced_per_environment() {
        let prod = cookie_prefix("shop", "production", "https://shop.example.com");
        let staging = cookie_prefix("shop", "staging", "https://staging.shop.example.com");
        assert!(prod.starts_with("wbp") && prod.ends_with('_') && prod.len() == 14, "{prod}");
        assert_ne!(prod, staging);
        assert_ne!(prod, cookie_prefix("other", "production", "https://shop.example.com"));
        assert_eq!(prod, cookie_prefix("shop", "production", "https://shop.example.com"), "stable across restarts");

        // Upstream cookies are stored under the env's prefix, relaxed for http loopback.
        let set = rewrite_set_cookie("sid=abc; Path=/; Domain=shop.example.com; Secure; HttpOnly; SameSite=None", &prod).unwrap();
        assert_eq!(set, format!("{prod}sid=abc; Path=/; HttpOnly; SameSite=Lax"));
        // An upstream cannot overwrite Workbench's session (or any other name) in the shared jar.
        let toss = rewrite_set_cookie("wb_session_7853=junk; Path=/api; HttpOnly", &prod).unwrap();
        assert_eq!(toss, format!("{prod}wb_session_7853=junk; Path=/api; HttpOnly"));
        assert_eq!(rewrite_set_cookie("__Host-sid=x; Path=/; Secure", &prod).unwrap(), format!("{prod}__Host-sid=x; Path=/"));
        assert_eq!(rewrite_set_cookie("novalue", &prod), None);
        assert_eq!(rewrite_set_cookie("=x", &prod), None);

        // Only this env's cookies go upstream, under their real names.
        let jar = format!(
            "wb_preview_41000=sess; wb_session_7853=wb; {prod}sid=abc; {staging}sid=STAGING; local_admin_session=LOCAL; {prod}__Host-t=1; {prod}=nameless"
        );
        assert_eq!(forward_cookies(&jar, &prod), vec!["sid=abc", "__Host-t=1"]);
        assert_eq!(forward_cookies(&jar, &staging), vec!["sid=STAGING"]);
        assert!(forward_cookies("local_admin_session=LOCAL; wb_session_7853=wb", &prod).is_empty());
    }

    #[test]
    fn session_cookie_check_is_exact() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("x=1; wb_preview_4000=secret-value; y=2"));
        assert!(has_session(&h, "wb_preview_4000", "secret-value"));
        assert!(!has_session(&h, "wb_preview_4000", "secret-valuX"));
        assert!(!has_session(&h, "wb_preview_4001", "secret-value"));
    }

    #[test]
    fn loopback_hosts_only() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7806"));
        assert_eq!(loopback_host(&h), Some("127.0.0.1"));
        h.insert(header::HOST, HeaderValue::from_static("localhost:7806"));
        assert_eq!(loopback_host(&h), Some("localhost"));
        h.insert(header::HOST, HeaderValue::from_static("[::1]:7806"));
        assert_eq!(loopback_host(&h), Some("[::1]"));
        h.insert(header::HOST, HeaderValue::from_static("box.tailnet.ts.net"));
        assert_eq!(loopback_host(&h), None);
    }
}

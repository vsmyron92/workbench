//! Authentication and request guarding.
//!
//! * A **master token** (random, `data_dir/token`, 0600) proves the caller can read
//!   Workbench's files. `/auth?token=…` swaps it for a device cookie and redirects so
//!   it never stays in the address bar. `workbench open` (and the desktop launcher)
//!   never put it on a browser's command line: they mint a one-time pairing code with
//!   the token in a header and open `/pair?code=…`.
//! * Signing in again from a browser that already holds a valid session cookie (the
//!   launcher, clicked every day) keeps that session: it gains a fresh device key
//!   (earlier keys keep working, least recently used dropped first, so the browser's
//!   open tabs stay signed in) instead of a new session per launch, so the device
//!   list and its push subscription stay.
//! * **Device sessions**: an HttpOnly, SameSite=Strict cookie (`wb_session_<port>` —
//!   cookies are not port-isolated, so the port keeps parallel instances apart).
//!   Only a SHA-256 of the cookie value is stored (`data_dir/auth.json`).
//! * **Device keys**: browsers send cookies for 127.0.0.1 to *every* local port, so
//!   any other local server the browser talks to (a dev server, an app preview)
//!   receives the cookie. The cookie alone therefore only reads (GET). Writes and
//!   WebSocket upgrades also need the session's device key, which the SPA keeps in
//!   its own origin's `localStorage` and sends as `X-Workbench-Key` (WebSockets:
//!   `?wbk=`). The key is handed over once at sign-in, in the redirect's URL
//!   fragment (`/#wbk=…`, never sent to a server) or the login response body.
//! * **Pairing**: a signed-in device mints a short-lived one-time code; another device
//!   (a phone) opens `/pair?code=…` and gets its own revocable session.
//! * **Agent tokens**: each hosted Claude session gets a token in its environment,
//!   valid only for `/api/hooks/*` and `/mcp`, bound to that terminal.
//! * **Ending a session** (revoke, logout, expiry) also closes that device's open
//!   WebSockets: socket loops select on `SessionWatch::ended`.
//!
//! The guard also pins the `Host` header (DNS-rebinding defence) and requires a
//! same-origin `Origin` on cookie-authenticated writes and WebSocket upgrades (CSRF).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::app::AppState;
use crate::config::Paths;
use crate::config::global::ServerConfig;
use crate::error::{ApiError, ApiResult};
use crate::util;

const SESSION_TTL_DAYS: i64 = 30;
const SESSION_TTL_MS: i64 = SESSION_TTL_DAYS * 86_400_000;
const PAIR_TTL: Duration = Duration::from_secs(600);
/// Header that carries the device key on fetches.
pub const KEY_HEADER: &str = "x-workbench-key";
/// Query parameter that carries the device key on WebSocket upgrades (browsers
/// cannot set headers there).
const KEY_QUERY: &str = "wbk";
/// WebSocket close code sent when the device session behind a socket ends.
pub const CLOSE_SESSION_ENDED: u16 = 4401;

/// Marks a request built inside the process (MCP tool dispatch). Extensions cannot
/// be set from the network, so the guard can trust it.
#[derive(Clone, Copy)]
pub struct InternalCall;

/// Who made an authenticated request (inserted as a request extension by the guard).
/// Fields are attribution for handlers that need to know who asked.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Caller {
    /// Bearer master token (CLI, scripts).
    Token,
    /// A device session.
    Device { session_id: String, name: String },
    /// In-process call (MCP tools). `terminal_id` is the agent session that asked.
    Internal { terminal_id: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSession {
    pub id: String,
    /// SHA-256 (hex) of the cookie value.
    pub token_hash: String,
    /// SHA-256 (hex) of the device key; empty for sessions created before device keys
    /// (those can still read, and must sign in again to write).
    #[serde(default)]
    pub key_hash: String,
    /// When `key_hash` was issued or last used (ms, updated at most once a minute).
    #[serde(default)]
    pub key_used_at: i64,
    /// Earlier device keys of this session that still work: a sign-in from a browser
    /// holding the session's cookie adds a key (`add_device_key`) while the browser's
    /// open tabs keep the key they hold. At most `MAX_OTHER_KEYS`; the least recently
    /// used goes first (a key a sign-in handed over that the page did not need is never
    /// used, so it goes before the key of a tab in use).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_keys: Vec<OtherKey>,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub remote: bool,
    #[serde(default)]
    pub user_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherKey {
    /// SHA-256 (hex) of the key.
    pub hash: String,
    /// Issued or last used (ms).
    pub used_at: i64,
}

/// Which of a session's device keys a request presented.
#[derive(Debug, PartialEq)]
enum KeyMatch {
    None,
    Current,
    Other(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub remote: bool,
    pub user_agent: String,
    pub current: bool,
}

struct PairCode {
    /// The device's name; `None`: from its browser (`device_name`).
    name: Option<String>,
    /// Minted by `workbench open` for this computer's own browser: no "Paired a new
    /// device" note.
    quiet: bool,
    expires: Instant,
}

/// Earlier device keys a session keeps besides its current one.
const MAX_OTHER_KEYS: usize = 8;
/// Key use is recorded at most this often per key.
const KEY_USE_EVERY_MS: i64 = 60_000;

pub struct AuthState {
    master_token: String,
    port: u16,
    file: std::path::PathBuf,
    sessions: RwLock<Vec<DeviceSession>>,
    pairing: Mutex<HashMap<String, PairCode>>,
    pair_failures: Mutex<Vec<Instant>>,
    agent_tokens: RwLock<HashMap<String, String>>,
    /// Bumped whenever device sessions end, so open sockets re-check theirs.
    ended: tokio::sync::watch::Sender<u64>,
}

impl AuthState {
    pub fn load(paths: &Paths, port: u16) -> anyhow::Result<Self> {
        let token_file = paths.data_dir.join("token");
        let master_token = match std::fs::read_to_string(&token_file) {
            Ok(t) if t.trim().len() >= 32 => t.trim().to_string(),
            _ => {
                let t = util::random_token(32);
                util::fs::write_atomic(&token_file, t.as_bytes(), 0o600)?;
                t
            }
        };
        util::fs::set_mode(&token_file, 0o600);
        let file = paths.data_dir.join("auth.json");
        let now = util::now_ms();
        let sessions: Vec<DeviceSession> = util::fs::read_json(&file)?.unwrap_or_default();
        let sessions = sessions.into_iter().filter(|s| now - s.last_seen_at < SESSION_TTL_MS).collect();
        Ok(Self {
            master_token,
            port,
            file,
            sessions: RwLock::new(sessions),
            pairing: Mutex::new(HashMap::new()),
            pair_failures: Mutex::new(vec![]),
            agent_tokens: RwLock::new(HashMap::new()),
            ended: tokio::sync::watch::channel(0).0,
        })
    }

    /// Whether device session `id` exists and has not expired.
    pub fn session_active(&self, id: &str) -> bool {
        let now = util::now_ms();
        self.sessions.read().iter().any(|s| s.id == id && now - s.last_seen_at <= SESSION_TTL_MS)
    }

    /// Remove sessions matching `f`; wakes every `SessionWatch` when any went.
    fn end_sessions(&self, f: impl Fn(&DeviceSession) -> bool) -> usize {
        let removed = {
            let mut sessions = self.sessions.write();
            let before = sessions.len();
            sessions.retain(|s| !f(s));
            before - sessions.len()
        };
        if removed > 0 {
            self.persist();
            self.ended.send_modify(|g| *g += 1);
        }
        removed
    }

    /// Watch the device session behind `caller` (from the request extensions). Socket
    /// loops select on `SessionWatch::ended` and close when it fires.
    pub fn watch(&self, caller: Option<&Caller>) -> SessionWatch {
        let session_id = match caller {
            Some(Caller::Device { session_id, .. }) => Some(session_id.clone()),
            _ => None,
        };
        let mut recheck = tokio::time::interval(Duration::from_secs(60));
        recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        SessionWatch { session_id, rx: self.ended.subscribe(), recheck }
    }

    /// Changes whenever device sessions end (revoke, logout): platform push drops
    /// the ended sessions' subscriptions. Expiry is seen through `session_active`.
    pub fn sessions_ended(&self) -> tokio::sync::watch::Receiver<u64> {
        self.ended.subscribe()
    }

    pub fn master_token(&self) -> &str {
        &self.master_token
    }

    pub fn cookie_name(&self) -> String {
        format!("wb_session_{}", self.port)
    }

    fn is_master(&self, candidate: &str) -> bool {
        candidate.as_bytes().ct_eq(self.master_token.as_bytes()).into()
    }

    fn persist(&self) {
        let sessions = self.sessions.read().clone();
        if let Err(e) = util::fs::write_json(&self.file, &sessions) {
            tracing::warn!("cannot save auth sessions: {e:#}");
        }
    }

    fn session_for_cookie(&self, value: &str) -> Option<DeviceSession> {
        let hash = sha256_hex(value);
        let mut sessions = self.sessions.write();
        let s = sessions.iter_mut().find(|s| bool::from(s.token_hash.as_bytes().ct_eq(hash.as_bytes())))?;
        let now = util::now_ms();
        if now - s.last_seen_at > SESSION_TTL_MS {
            return None;
        }
        let stale = now - s.last_seen_at > 3_600_000;
        s.last_seen_at = now;
        let out = s.clone();
        drop(sessions);
        if stale {
            self.persist();
        }
        Some(out)
    }

    /// Create a device session; returns the cookie value and the device key.
    fn create_session(&self, name: &str, remote: bool, user_agent: &str) -> (String, String) {
        let value = util::random_token(32);
        let key = util::random_token(32);
        let now = util::now_ms();
        self.sessions.write().push(DeviceSession {
            id: util::random_token(9),
            token_hash: sha256_hex(&value),
            key_hash: sha256_hex(&key),
            key_used_at: now,
            other_keys: vec![],
            name: name.chars().take(80).collect(),
            created_at: now,
            last_seen_at: now,
            remote,
            user_agent: user_agent.chars().take(200).collect(),
        });
        self.persist();
        (value, key)
    }

    /// A new device key for the live session behind cookie `value` (a sign-in from a
    /// browser that already has the session): the session, its id and its push
    /// subscription stay; the keys its open tabs hold keep working (`other_keys`).
    /// `None` when the cookie names no live session.
    fn add_device_key(&self, value: &str) -> Option<String> {
        let hash = sha256_hex(value);
        let key = util::random_token(32);
        let now = util::now_ms();
        {
            let mut sessions = self.sessions.write();
            let s = sessions.iter_mut().find(|s| bool::from(s.token_hash.as_bytes().ct_eq(hash.as_bytes())))?;
            if now - s.last_seen_at > SESSION_TTL_MS {
                return None;
            }
            let old = std::mem::replace(&mut s.key_hash, sha256_hex(&key));
            if !old.is_empty() {
                s.other_keys.push(OtherKey { hash: old, used_at: s.key_used_at });
            }
            s.key_used_at = now;
            while s.other_keys.len() > MAX_OTHER_KEYS {
                let lru = s.other_keys.iter().enumerate().min_by_key(|(_, k)| k.used_at).map(|(i, _)| i).unwrap_or(0);
                s.other_keys.remove(lru);
            }
            s.last_seen_at = now;
        }
        self.persist();
        Some(key)
    }

    /// Record that a request used `matched` (throttled to `KEY_USE_EVERY_MS`), so the
    /// keys of tabs in use are the last `add_device_key` drops.
    fn note_key_use(&self, session: &DeviceSession, matched: &KeyMatch) {
        let now = util::now_ms();
        let stale = match matched {
            KeyMatch::None => false,
            KeyMatch::Current => now - session.key_used_at > KEY_USE_EVERY_MS,
            KeyMatch::Other(h) => session.other_keys.iter().any(|k| &k.hash == h && now - k.used_at > KEY_USE_EVERY_MS),
        };
        if !stale {
            return;
        }
        let mut sessions = self.sessions.write();
        let Some(s) = sessions.iter_mut().find(|s| s.id == session.id) else { return };
        match matched {
            KeyMatch::Current if s.key_hash == session.key_hash => s.key_used_at = now,
            KeyMatch::Other(h) => {
                if let Some(k) = s.other_keys.iter_mut().find(|k| &k.hash == h) {
                    k.used_at = now;
                }
            }
            _ => {}
        }
    }

    /// Mint a one-time pairing code, valid for `PAIR_TTL`.
    fn mint_pair_code(&self, name: Option<String>, quiet: bool) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
        use rand::Rng;
        let mut rng = rand::rng();
        let code: String = (0..10).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
        let mut map = self.pairing.lock();
        map.retain(|_, c| c.expires > Instant::now());
        map.insert(code.clone(), PairCode { name, quiet, expires: Instant::now() + PAIR_TTL });
        code
    }

    /// A login URL for this computer's own browser (`serve --open`): a one-time code,
    /// so the master token never reaches a browser's command line.
    pub fn local_login_path(&self) -> String {
        format!("/pair?code={}", self.mint_pair_code(None, true))
    }

    /// Issue a token for a hosted agent session. It authorizes only hook and MCP
    /// calls attributed to `terminal_id`.
    pub fn issue_agent_token(&self, terminal_id: &str) -> String {
        let t = format!("wba_{}", util::random_token(24));
        self.agent_tokens.write().insert(t.clone(), terminal_id.to_string());
        t
    }

    pub fn revoke_agent_tokens(&self, terminal_id: &str) {
        self.agent_tokens.write().retain(|_, v| v != terminal_id);
    }

    /// Tests: whether an agent token of `terminal_id` is valid.
    #[cfg(test)]
    pub fn has_agent_token(&self, terminal_id: &str) -> bool {
        self.agent_tokens.read().values().any(|v| v == terminal_id)
    }

    /// The terminal an agent token belongs to (from `Authorization: Bearer`).
    pub fn agent_from_headers(&self, headers: &HeaderMap) -> Option<String> {
        let token = bearer(headers)?;
        let map = self.agent_tokens.read();
        map.iter()
            .find(|(k, _)| bool::from(k.as_bytes().ct_eq(token.as_bytes())))
            .map(|(_, v)| v.clone())
    }

    /// The master token also works on agent endpoints (manual testing, scripts).
    pub fn is_master_bearer(&self, headers: &HeaderMap) -> bool {
        bearer(headers).is_some_and(|t| self.is_master(t))
    }
}

/// Resolves when a device session ends. See `AuthState::watch`.
pub struct SessionWatch {
    /// `None` for callers that are not device sessions (master token, in-process):
    /// those never end.
    session_id: Option<String>,
    rx: tokio::sync::watch::Receiver<u64>,
    recheck: tokio::time::Interval,
}

impl SessionWatch {
    /// Completes once the session was revoked, logged out or expired. Cancel-safe,
    /// so it can sit in a `tokio::select!` loop.
    pub async fn ended(&mut self, auth: &AuthState) {
        let Some(id) = self.session_id.clone() else {
            return std::future::pending().await;
        };
        loop {
            tokio::select! {
                r = self.rx.changed() => {
                    if r.is_err() {
                        return std::future::pending().await;
                    }
                }
                _ = self.recheck.tick() => {}
            }
            if !auth.session_active(&id) {
                return;
            }
        }
    }
}

/// The close frame for a socket whose session ended.
pub fn session_ended_close() -> axum::extract::ws::Message {
    axum::extract::ws::Message::Close(Some(axum::extract::ws::CloseFrame {
        code: CLOSE_SESSION_ENDED,
        reason: "signed out".into(),
    }))
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// The device key a request presents: the header, or `?wbk=` on WebSocket upgrades.
fn presented_key<'a>(headers: &'a HeaderMap, query: Option<&'a str>, is_ws: bool) -> Option<&'a str> {
    if let Some(k) = headers.get(KEY_HEADER).and_then(|v| v.to_str().ok()) {
        return Some(k.trim());
    }
    if !is_ws {
        return None;
    }
    query?.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == KEY_QUERY).map(|(_, v)| v)
}

/// Which device key of `session` `key` is: its current one or an earlier one it keeps
/// (sessions from before device keys have none).
fn key_match(session: &DeviceSession, key: Option<&str>) -> KeyMatch {
    let Some(k) = key.filter(|k| !k.is_empty() && !session.key_hash.is_empty()) else { return KeyMatch::None };
    let h = sha256_hex(k);
    if bool::from(h.as_bytes().ct_eq(session.key_hash.as_bytes())) {
        return KeyMatch::Current;
    }
    match session.other_keys.iter().find(|o| bool::from(h.as_bytes().ct_eq(o.hash.as_bytes()))) {
        Some(o) => KeyMatch::Other(o.hash.clone()),
        None => KeyMatch::None,
    }
}

#[cfg(test)]
fn key_matches(session: &DeviceSession, key: Option<&str>) -> bool {
    key_match(session, key) != KeyMatch::None
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The name part of a `Host` header value (`[::1]:7777` → `::1`).
pub(crate) fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
}

/// `localhost` or a loopback IP literal. A DNS name that merely starts with `127.`
/// (`127.attacker.example`) is not: it could be rebound to anything.
pub(crate) fn is_loopback_name(h: &str) -> bool {
    h.eq_ignore_ascii_case("localhost") || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether Host pinning accepts `host` (a `Host` header value) for a server bound to
/// `bind_ip` with `server` settings. The single rule for the guard and for Settings.
pub(crate) fn host_accepted(server: &ServerConfig, bind_ip: IpAddr, host: &str) -> bool {
    let name = host_name(host);
    if is_loopback_name(name) || name == bind_ip.to_string() {
        return true;
    }
    let public_host = server
        .public_url
        .as_deref()
        .and_then(|u| u.split("://").nth(1))
        .map(|rest| rest.split('/').next().unwrap_or(rest).to_string());
    server.allowed_hosts.iter().any(|a| a == host || a == name) || public_host.is_some_and(|p| p == host || host_name(&p) == name)
}

/// DNS-rebinding defence: only answer to Host names we expect.
fn host_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    host_accepted(&state.config.read().server, state.bind_addr.ip(), host)
}

/// Same-origin check: the `Origin` header's host must equal the `Host` header.
fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
    match (host, origin) {
        (Some(h), Some(o)) => o.split("://").nth(1).is_some_and(|rest| rest.trim_end_matches('/') == h),
        (Some(_), None) => headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "same-origin" || v == "none"),
        _ => false,
    }
}

fn is_public_path(path: &str) -> bool {
    matches!(path, "/api/health" | "/api/auth/login" | "/api/auth/status" | "/auth" | "/pair")
        || !(path.starts_with("/api/") || path == "/mcp")
}

/// Paths that authenticate with agent tokens inside their handlers.
fn is_agent_path(path: &str) -> bool {
    path.starts_with("/api/hooks/") || path == "/mcp"
}

pub async fn guard(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    if req.extensions().get::<InternalCall>().is_some() {
        return next.run(req).await;
    }
    let headers = req.headers();
    if !host_allowed(&state, headers) {
        return ApiError::forbidden("unrecognized Host header; add it to server.allowed_hosts").into_response();
    }
    let path = req.uri().path().to_string();
    if is_public_path(&path) || is_agent_path(&path) {
        return next.run(req).await;
    }
    if let Some(t) = bearer(headers) {
        if state.auth.is_master(t) {
            req.extensions_mut().insert(Caller::Token);
            return next.run(req).await;
        }
        return ApiError::unauthorized("invalid token").into_response();
    }
    let session = cookie_value(headers, &state.auth.cookie_name()).and_then(|v| state.auth.session_for_cookie(v));
    let Some(session) = session else {
        return ApiError::unauthorized("sign in required").into_response();
    };
    let is_ws = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let unsafe_method = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if unsafe_method || is_ws {
        if !same_origin(headers) {
            return ApiError::forbidden("cross-origin request refused").into_response();
        }
        // The cookie reaches every local port; acting needs the key only this origin holds.
        let matched = key_match(&session, presented_key(headers, req.uri().query(), is_ws));
        if matched == KeyMatch::None {
            return ApiError::unauthorized("this browser must sign in again").into_response();
        }
        state.auth.note_key_use(&session, &matched);
    }
    req.extensions_mut().insert(Caller::Device { session_id: session.id, name: session.name });
    next.run(req).await
}

fn session_cookie(state: &AppState, value: &str, secure: bool) -> String {
    format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        state.auth.cookie_name(),
        SESSION_TTL_DAYS * 86_400,
        if secure { "; Secure" } else { "" }
    )
}

fn is_https(state: &AppState, headers: &HeaderMap) -> bool {
    state.config.read().server.tls.is_some()
        || headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
}

fn device_name(headers: &HeaderMap, fallback: &str) -> String {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let os = if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Mac OS") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        fallback
    };
    let browser = if ua.contains("Firefox") {
        "Firefox"
    } else if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Chrome") {
        "Chrome"
    } else if ua.contains("Safari") {
        "Safari"
    } else {
        "browser"
    };
    format!("{browser} on {os}")
}

/// Start a device session: sets its cookie on `resp`, returns the device key.
fn start_session(state: &AppState, headers: &HeaderMap, addr: SocketAddr, name: Option<String>, resp: &mut Response) -> String {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let name = name.unwrap_or_else(|| device_name(headers, "device"));
    // `to_canonical`: a dual-stack `[::]` listener reports IPv4 peers as `::ffff:127.0.0.1`.
    let (value, key) = state.auth.create_session(&name, !addr.ip().to_canonical().is_loopback(), ua);
    let cookie = session_cookie(state, &value, is_https(state, headers));
    resp.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
    resp.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    resp.headers_mut().insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    key
}

/// Sign in and redirect to the app. The device key rides in the URL fragment, which
/// browsers never send to a server; the SPA stores it and removes it from the URL.
/// A browser that already holds a live session cookie of this port keeps that
/// session (the launcher opens Workbench this way every day): it gets another device
/// key and its cookie's lifetime renewed, not a new session. Returns whether the
/// session was kept.
fn login_redirect(state: &AppState, headers: &HeaderMap, addr: SocketAddr, name: Option<String>) -> (Response, bool) {
    let mut resp = Redirect::to("/").into_response();
    let cookie = cookie_value(headers, &state.auth.cookie_name());
    let (key, kept) = match cookie.and_then(|v| state.auth.add_device_key(v).map(|k| (v, k))) {
        Some((value, key)) => {
            let cookie = session_cookie(state, value, is_https(state, headers));
            resp.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
            resp.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
            resp.headers_mut().insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
            (key, true)
        }
        None => (start_session(state, headers, addr, name, &mut resp), false),
    };
    resp.headers_mut().insert(header::LOCATION, format!("/#{KEY_QUERY}={key}").parse().unwrap());
    (resp, kept)
}

#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

/// `GET /auth?token=…` — exchange the master token for a device cookie.
pub async fn token_login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    match q.token {
        Some(t) if state.auth.is_master(&t) => login_redirect(&state, &headers, addr, None).0,
        _ => {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Redirect::to("/?login=failed").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct PairQuery {
    code: String,
}

/// `GET /pair?code=…` — redeem a pairing code minted by a signed-in device.
pub async fn pair_redeem(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<PairQuery>,
) -> Response {
    {
        let mut fails = state.auth.pair_failures.lock();
        fails.retain(|t| t.elapsed() < Duration::from_secs(600));
        if fails.len() >= 20 {
            return (StatusCode::TOO_MANY_REQUESTS, "too many failed pairing attempts; try again later").into_response();
        }
    }
    let code = q.code.trim().to_ascii_uppercase();
    let entry = {
        let mut map = state.auth.pairing.lock();
        map.retain(|_, c| c.expires > Instant::now());
        map.remove(&code)
    };
    match entry {
        Some(c) => {
            let shown = c.name.clone();
            let (resp, kept) = login_redirect(&state, &headers, addr, c.name);
            if !c.quiet && !kept {
                state.events.notify("info", &format!("Paired a new device: {}", shown.as_deref().unwrap_or("a browser")));
            }
            resp
        }
        None => {
            state.auth.pair_failures.lock().push(Instant::now());
            tokio::time::sleep(Duration::from_millis(600)).await;
            Redirect::to("/?login=pair-failed").into_response()
        }
    }
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
    name: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    if !same_origin(&headers) {
        return ApiError::forbidden("cross-origin request refused").into_response();
    }
    if !state.auth.is_master(body.token.trim()) {
        tokio::time::sleep(Duration::from_millis(400)).await;
        return ApiError::unauthorized("wrong token").into_response();
    }
    let mut resp = Json(serde_json::json!({})).into_response();
    let key = start_session(&state, &headers, addr, body.name, &mut resp);
    *resp.body_mut() = axum::body::Body::from(serde_json::json!({ "ok": true, "key": key }).to_string());
    resp
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(v) = cookie_value(&headers, &state.auth.cookie_name()) {
        let hash = sha256_hex(v);
        state.auth.end_sessions(|s| s.token_hash == hash);
    }
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    let clear = format!("{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0", state.auth.cookie_name());
    resp.headers_mut().insert(header::SET_COOKIE, clear.parse().unwrap());
    Ok(resp)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusOut {
    authenticated: bool,
    device: Option<String>,
    version: &'static str,
}

/// Signed in means able to act: a device session *with* its device key (a session
/// from before device keys, or a browser that lost its key, must sign in again).
async fn status(State(state): State<AppState>, headers: HeaderMap) -> Json<StatusOut> {
    let session = cookie_value(&headers, &state.auth.cookie_name()).and_then(|v| state.auth.session_for_cookie(v)).filter(|s| {
        let matched = key_match(s, presented_key(&headers, None, false));
        state.auth.note_key_use(s, &matched);
        matched != KeyMatch::None
    });
    let bearer_ok = bearer(&headers).is_some_and(|t| state.auth.is_master(t));
    Json(StatusOut {
        authenticated: session.is_some() || bearer_ok,
        device: session.map(|s| s.name),
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairBody {
    name: Option<String>,
    /// `workbench open`: a code for this computer's own browser (named after the
    /// browser, no "Paired a new device" note).
    #[serde(default)]
    launch: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairOut {
    code: String,
    url: String,
    expires_at: i64,
}

/// Mint a one-time pairing code (valid 10 minutes).
async fn pair_create(State(state): State<AppState>, headers: HeaderMap, body: Option<Json<PairBody>>) -> ApiResult<Json<PairOut>> {
    let body = body.map(|b| b.0);
    // Only the master token (the `workbench open` CLI) mints launch codes.
    let launch = body.as_ref().is_some_and(|b| b.launch) && state.auth.is_master_bearer(&headers);
    let name = body.and_then(|b| b.name).or_else(|| (!launch).then(|| "Paired device".into()));
    let code = state.auth.mint_pair_code(name, launch);
    let base = state.config.read().server.public_url.clone().unwrap_or_else(|| {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("127.0.0.1");
        let scheme = if is_https(&state, &headers) { "https" } else { "http" };
        format!("{scheme}://{host}")
    });
    Ok(Json(PairOut {
        url: format!("{}/pair?code={code}", base.trim_end_matches('/')),
        code,
        expires_at: util::now_ms() + PAIR_TTL.as_millis() as i64,
    }))
}

async fn devices(State(state): State<AppState>, req: Request) -> Json<Vec<DeviceInfo>> {
    let current = match req.extensions().get::<Caller>() {
        Some(Caller::Device { session_id, .. }) => Some(session_id.clone()),
        _ => None,
    };
    let list = state
        .auth
        .sessions
        .read()
        .iter()
        .map(|s| DeviceInfo {
            id: s.id.clone(),
            name: s.name.clone(),
            created_at: s.created_at,
            last_seen_at: s.last_seen_at,
            remote: s.remote,
            user_agent: s.user_agent.clone(),
            current: current.as_deref() == Some(&s.id),
        })
        .collect();
    Json(list)
}

/// Revoke a device: its REST calls fail from now on and its open WebSockets close.
async fn revoke_device(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    if state.auth.end_sessions(|s| s.id == id) == 0 {
        return Err(ApiError::not_found("no such device"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/status", get(status))
        .route("/api/auth/pair", post(pair_create))
        .route("/api/auth/devices", get(devices))
        .route("/api/auth/devices/{id}", delete(revoke_device))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    #[test]
    fn same_origin_requires_matching_origin_host() {
        assert!(same_origin(&headers(&[("host", "127.0.0.1:7777"), ("origin", "http://127.0.0.1:7777")])));
        assert!(!same_origin(&headers(&[("host", "127.0.0.1:7777"), ("origin", "http://evil.example")])));
        assert!(!same_origin(&headers(&[("host", "127.0.0.1:7777"), ("origin", "http://127.0.0.1:7778")])));
        assert!(same_origin(&headers(&[("host", "127.0.0.1:7777"), ("sec-fetch-site", "same-origin")])));
        assert!(!same_origin(&headers(&[("host", "127.0.0.1:7777"), ("sec-fetch-site", "cross-site")])));
    }

    #[test]
    fn cookie_parsing_picks_the_named_cookie() {
        let h = headers(&[("cookie", "a=1; wb_session_7777=abc; wb_session_7778=def")]);
        assert_eq!(cookie_value(&h, "wb_session_7777"), Some("abc"));
        assert_eq!(cookie_value(&h, "wb_session_7778"), Some("def"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }

    #[test]
    fn host_names_strip_ports_and_brackets() {
        assert_eq!(host_name("127.0.0.1:7777"), "127.0.0.1");
        assert_eq!(host_name("[::1]:7777"), "::1");
        assert_eq!(host_name("box.tailnet.ts.net"), "box.tailnet.ts.net");
        assert!(is_loopback_name("localhost"));
        assert!(is_loopback_name("127.0.0.2"));
        assert!(is_loopback_name("::1"));
        assert!(!is_loopback_name("attacker.example"));
    }

    #[test]
    fn dns_names_that_look_like_loopback_are_not_accepted() {
        let s = ServerConfig::default();
        let bind: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(host_accepted(&s, bind, "127.0.0.1:7777"));
        assert!(host_accepted(&s, bind, "[::1]:7777"));
        assert!(!host_accepted(&s, bind, "127.evil.example:7777"), "a rebindable DNS name");
        assert!(!host_accepted(&s, bind, "127.0.0.1.nip.io:7777"));
        assert!(!host_accepted(&s, bind, "localhost.attacker.example"));
    }

    #[test]
    fn sessions_without_a_device_key_cannot_act() {
        let mut s = DeviceSession {
            id: "a".into(),
            token_hash: sha256_hex("cookie"),
            key_hash: String::new(),
            key_used_at: 0,
            other_keys: vec![],
            name: "n".into(),
            created_at: 0,
            last_seen_at: 0,
            remote: false,
            user_agent: String::new(),
        };
        assert!(!key_matches(&s, Some("")), "a session from before device keys");
        assert!(!key_matches(&s, None));
        s.key_hash = sha256_hex("k1");
        assert!(key_matches(&s, Some("k1")));
        assert!(!key_matches(&s, Some("k2")));
        assert!(!key_matches(&s, Some("")));
        // Earlier keys a session keeps still work (its other tabs).
        s.other_keys = vec![OtherKey { hash: sha256_hex("k0"), used_at: 0 }];
        assert_eq!(key_match(&s, Some("k0")), KeyMatch::Other(sha256_hex("k0")));
        assert_eq!(key_match(&s, Some("k1")), KeyMatch::Current);
        assert!(key_matches(&s, Some("k0")));
        assert!(key_matches(&s, Some("k1")));
        assert!(!key_matches(&s, Some("k2")));
        // Only WebSocket upgrades may carry the key in the query.
        let h = headers(&[]);
        assert_eq!(presented_key(&h, Some("x=1&wbk=k1"), true), Some("k1"));
        assert_eq!(presented_key(&h, Some("wbk=k1"), false), None);
    }

    // ---------------------------------------------------------------- over a real socket

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    struct Served {
        state: AppState,
        addr: SocketAddr,
        _dirs: tempfile::TempDir,
    }

    async fn served() -> Served {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let mut cfg = crate::config::GlobalConfig::default();
        cfg.projects.roots = vec![];
        cfg.notify.desktop = false;
        cfg.agents.restore_on_start = false;
        let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
        crate::terminals::start(&state).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = crate::app::build_router(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
        });
        Served { state, addr, _dirs: dir }
    }

    /// Sign in through `/auth?token=`: the session cookie and the device key.
    async fn sign_in(s: &Served) -> (String, String) {
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let r = http.get(format!("http://{}/auth?token={}", s.addr, s.state.auth.master_token())).send().await.unwrap();
        assert!(r.status().is_redirection());
        let location = r.headers()[header::LOCATION].to_str().unwrap().to_string();
        let key = location.strip_prefix("/#wbk=").expect("the key rides in the fragment").to_string();
        let cookie = r.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
        assert!(cookie.starts_with(&s.state.auth.cookie_name()));
        (cookie, key)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn signing_in_again_keeps_the_browsers_session() {
        let s = served().await;
        let (cookie, key1) = sign_in(&s).await;
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let origin = format!("http://{}", s.addr);
        let sessions = || s.state.auth.sessions.read().iter().map(|x| x.id.clone()).collect::<Vec<_>>();
        let before = sessions();
        assert_eq!(before.len(), 1);

        // The launcher: `workbench open` mints a launch code with the token in a header…
        let r = http
            .post(format!("{origin}/api/auth/pair"))
            .bearer_auth(s.state.auth.master_token())
            .json(&serde_json::json!({ "launch": true }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let code = r.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap().to_string();
        // …and the browser, which still has its cookie, opens /pair with it.
        let r = http.get(format!("{origin}/pair?code={code}")).header("cookie", &cookie).send().await.unwrap();
        assert!(r.status().is_redirection());
        let key2 = r.headers()[header::LOCATION].to_str().unwrap().strip_prefix("/#wbk=").unwrap().to_string();
        assert_ne!(key1, key2);
        let renewed = r.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(renewed.starts_with(&cookie), "the same cookie, its lifetime renewed");
        assert_eq!(sessions(), before, "no new device session per launch");
        // The same through /auth?token=.
        let r = http.get(format!("{origin}/auth?token={}", s.state.auth.master_token())).header("cookie", &cookie).send().await.unwrap();
        let key3 = r.headers()[header::LOCATION].to_str().unwrap().strip_prefix("/#wbk=").unwrap().to_string();
        assert_eq!(sessions(), before);
        // Every tab's key acts: the one open before the launch and the new ones.
        for k in [&key1, &key2, &key3] {
            let r = http.post(format!("{origin}/api/projects/reload")).header("cookie", &cookie).header("origin", &origin).header(KEY_HEADER, k.as_str()).send().await.unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
        // A code is single use.
        let r = http.get(format!("{origin}/pair?code={code}")).send().await.unwrap();
        assert_eq!(r.headers()[header::LOCATION].to_str().unwrap(), "/?login=pair-failed");
        // Without a cookie (another browser) a code still creates a session of its own.
        let r = http.post(format!("{origin}/api/auth/pair")).bearer_auth(s.state.auth.master_token()).json(&serde_json::json!({ "launch": true })).send().await.unwrap();
        let code = r.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap().to_string();
        let r = http.get(format!("{origin}/pair?code={code}")).header("user-agent", "Mozilla/5.0 (X11; Linux x86_64) Chrome/140").send().await.unwrap();
        assert!(r.headers().get(header::SET_COOKIE).is_some());
        assert_eq!(sessions().len(), 2);
        assert_eq!(s.state.auth.sessions.read()[1].name, "Chrome on Linux", "a launch code names the browser");
        // Many launches later, the key of a tab in use still works; handed-over keys
        // no page needed go first (least recently used).
        let act = |k: String| {
            let (http, cookie, origin) = (http.clone(), cookie.clone(), origin.clone());
            async move { http.post(format!("{origin}/api/projects/reload")).header("cookie", &cookie).header("origin", &origin).header(KEY_HEADER, k).send().await.unwrap().status() }
        };
        // Time passes between launches (every key's last use moves two minutes back);
        // the long-lived tab keeps acting with key1 meanwhile.
        let age = |st: &AppState| {
            for x in st.auth.sessions.write().iter_mut() {
                x.key_used_at -= 2 * KEY_USE_EVERY_MS;
                for k in x.other_keys.iter_mut() {
                    k.used_at -= 2 * KEY_USE_EVERY_MS;
                }
            }
        };
        for _ in 0..MAX_OTHER_KEYS + 2 {
            age(&s.state);
            assert_eq!(act(key1.clone()).await, StatusCode::OK, "the long-lived tab acts (its key's use is recorded)");
            http.get(format!("{origin}/auth?token={}", s.state.auth.master_token())).header("cookie", &cookie).send().await.unwrap();
        }
        assert_eq!(act(key1.clone()).await, StatusCode::OK, "the key in use survives");
        assert_eq!(act(key2.clone()).await, StatusCode::UNAUTHORIZED, "an unused handed-over key went");
        assert!(s.state.auth.sessions.read().iter().all(|x| x.other_keys.len() <= MAX_OTHER_KEYS));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stolen_cookie_can_read_but_not_act() {
        let s = served().await;
        let (cookie, key) = sign_in(&s).await;
        let http = reqwest::Client::new();
        let origin = format!("http://{}", s.addr);
        let post = |key: Option<&str>| {
            let mut r = http.post(format!("{origin}/api/projects/reload")).header("cookie", &cookie).header("origin", &origin);
            if let Some(k) = key {
                r = r.header(KEY_HEADER, k);
            }
            r.send()
        };
        assert_eq!(post(None).await.unwrap().status(), StatusCode::UNAUTHORIZED, "cookie alone");
        assert_eq!(post(Some("wrong")).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(post(Some(&key)).await.unwrap().status(), StatusCode::OK);
        let get = http.get(format!("{origin}/api/projects")).header("cookie", &cookie).send().await.unwrap();
        assert_eq!(get.status(), StatusCode::OK, "reads (images, downloads) work with the cookie");
        let status = |key: Option<&str>| {
            let mut r = http.get(format!("{origin}/api/auth/status")).header("cookie", &cookie);
            if let Some(k) = key {
                r = r.header(KEY_HEADER, k);
            }
            async move { r.send().await.unwrap().json::<serde_json::Value>().await.unwrap()["authenticated"].clone() }
        };
        assert_eq!(status(None).await, false, "the SPA signs in again when it lacks the key");
        assert_eq!(status(Some(&key)).await, true);
        // WebSockets: the key comes in the query.
        let ws = |q: &str| {
            let mut req = format!("ws://{}/api/events/ws{q}", s.addr).into_client_request().unwrap();
            req.headers_mut().insert("cookie", cookie.parse().unwrap());
            req.headers_mut().insert("origin", origin.parse().unwrap());
            tokio_tungstenite::connect_async(req)
        };
        assert!(ws("").await.is_err(), "no upgrade without the key");
        assert!(ws(&format!("?wbk={key}")).await.is_ok());
        // Password login answers with the key in the body.
        let r = http
            .post(format!("{origin}/api/auth/login"))
            .header("origin", &origin)
            .json(&serde_json::json!({ "token": s.state.auth.master_token() }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.headers().contains_key(header::SET_COOKIE));
        let body = r.json::<serde_json::Value>().await.unwrap();
        assert!(body["key"].as_str().is_some_and(|k| k.len() >= 32 && k != key), "{body}");
    }

    async fn close_code(ws: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> Option<u16> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while let Ok(Some(m)) = tokio::time::timeout_at(deadline, ws.next()).await {
            match m {
                Ok(WsMessage::Close(f)) => return f.map(|f| u16::from(f.code)),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn revoking_a_device_closes_its_open_sockets() {
        let s = served().await;
        let (cookie, key) = sign_in(&s).await;
        let origin = format!("http://{}", s.addr);
        let dir = tempfile::tempdir().unwrap();
        let term = s
            .state
            .terminals
            .spawn(
                &s.state,
                crate::terminals::SpawnSpec {
                    kind: crate::terminals::TerminalKind::Command,
                    title: "t".into(),
                    project_id: None,
                    cwd: dir.path().to_path_buf(),
                    argv: vec!["bash".into(), "-c".into(), "while read -r l; do echo \"got:$l\"; done".into()],
                    env: vec![],
                    cols: Some(80),
                    rows: Some(24),
                    meta: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        let open = |path: String| {
            let mut req = format!("ws://{}{path}?wbk={key}", s.addr).into_client_request().unwrap();
            req.headers_mut().insert("cookie", cookie.parse().unwrap());
            req.headers_mut().insert("origin", origin.parse().unwrap());
            tokio_tungstenite::connect_async(req)
        };
        let (mut events, _) = open("/api/events/ws".into()).await.unwrap();
        let (mut pty, _) = open(format!("/api/terminals/{}/ws", term.id)).await.unwrap();

        // Revoke from another device (here: the master token).
        let http = reqwest::Client::new();
        let bearer = format!("Bearer {}", s.state.auth.master_token());
        let devices: serde_json::Value =
            http.get(format!("{origin}/api/auth/devices")).header("authorization", &bearer).send().await.unwrap().json().await.unwrap();
        let id = devices[0]["id"].as_str().unwrap().to_string();
        let r = http.delete(format!("{origin}/api/auth/devices/{id}")).header("authorization", &bearer).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        assert_eq!(close_code(&mut events).await, Some(CLOSE_SESSION_ENDED), "the event stream ends");
        assert_eq!(close_code(&mut pty).await, Some(CLOSE_SESSION_ENDED), "the shell socket ends");
        // Input after the revoke never reaches the PTY.
        let _ = pty.send(WsMessage::Binary(b"after-revoke\r".to_vec().into())).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let screen = s.state.terminals.screen_text(&term.id, 50).unwrap();
        assert!(!screen.contains("got:after-revoke"), "{screen:?}");
        // New upgrades are refused.
        assert!(open("/api/events/ws".into()).await.is_err());
        s.state.terminals.kill(&term.id).await.unwrap();
    }

    #[test]
    fn public_and_agent_paths() {
        assert!(is_public_path("/"));
        assert!(is_public_path("/assets/app.js"));
        assert!(is_public_path("/api/health"));
        assert!(!is_public_path("/api/projects"));
        assert!(!is_public_path("/mcp"));
        assert!(is_agent_path("/api/hooks/claude/abc"));
        assert!(is_agent_path("/mcp"));
    }
}

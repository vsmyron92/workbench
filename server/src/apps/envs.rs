//! Environments (production, staging, …): health polling, deployed version,
//! remote logs and commands. Deploys are in `deploy.rs`, the preview proxy in `proxy.rs`.
//!
//! * One background poller per env with a `health` probe, every `interval_s`
//!   (clamped to 10 s … 1 day). It reconciles with the project registry every 15 s,
//!   so config edits and reloads restart or stop pollers.
//! * A failed probe is retried once after 2 s before the env counts as down; up→down
//!   and down→up transitions also raise a `ui.notify` toast.
//! * The last 60 samples are kept for the sparkline. Every check emits `env.health`.
//! * Versions: `version.http` (+ `json_pointer`) is probed with the health checks;
//!   `version.command` (ssh) runs only on demand (`POST …/version`, after a deploy).
//! * Basic auth comes from the env's `auth.password` secret and is sent only to
//!   paths outside `auth.except`.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use parking_lot::Mutex;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use super::health::{self, HealthStatus};
use super::remote;
use crate::app::AppState;
use crate::config::project::{EnvKind, Environment};
use crate::error::ApiError;
use crate::projects::Project;
use crate::terminals::{SpawnSpec, TerminalInfo, TerminalKind};

pub const HISTORY: usize = 60;
const MAX_BODY: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub t: i64,
    pub ok: bool,
    /// Latency; `None` when there was no response.
    pub ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub status: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub history: VecDeque<Sample>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    pub checked_at: i64,
    /// "http" | "command"
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    /// "direct" (iframe the URL) | "proxy" (loopback proxy injects auth / strips frame headers)
    pub mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Default)]
struct Live {
    health: HealthView,
    version: Option<VersionInfo>,
    /// Response headers forbid framing (probed at most every 30 min).
    frame_blocked: Option<(bool, Instant)>,
}

type Key = (String, String);

#[derive(Default)]
pub struct Envs {
    live: Mutex<HashMap<Key, Live>>,
    pollers: Mutex<HashMap<Key, (u64, tokio::task::AbortHandle)>>,
    /// Serializes checks of one env (poller vs. "check now").
    locks: DashMap<Key, Arc<tokio::sync::Mutex<()>>>,
    /// Running deploy terminal per env.
    pub(crate) deploys: Mutex<HashMap<Key, String>>,
}

fn key(pid: &str, env: &str) -> Key {
    (pid.to_string(), env.to_string())
}

impl Envs {
    pub fn version(&self, pid: &str, env: &str) -> Option<VersionInfo> {
        self.live.lock().get(&key(pid, env)).and_then(|l| l.version.clone())
    }

    pub fn health(&self, pid: &str, env: &str) -> HealthView {
        self.live.lock().get(&key(pid, env)).map(|l| l.health.clone()).unwrap_or_default()
    }

    pub(crate) fn set_version(&self, pid: &str, env: &str, v: VersionInfo) {
        self.live.lock().entry(key(pid, env)).or_default().version = Some(v);
    }
}

// ---------------------------------------------------------------- views

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvView {
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
    pub config: Value,
    pub health: HealthView,
    pub version: Option<VersionInfo>,
    pub preview: PreviewInfo,
    /// Terminal id of a deploy in progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploying: Option<String>,
}

/// The env config as the UI sees it: no secret values (only secret *names*).
pub fn config_view(project: &Project, e: &Environment) -> Value {
    let target = remote::env_target(project, e).map(|t| t.label()).ok();
    json!({
        "host": e.host,
        "target": target,
        "health": e.health.as_ref().map(|h| json!({
            "url": h.url, "expectStatus": h.expect_status, "jsonPointer": h.json_pointer, "equals": h.equals,
            "intervalS": h.interval_s, "timeoutMs": h.timeout_ms, "viaHost": h.via_host,
        })),
        "version": e.version.as_ref().map(|v| json!({ "http": v.http, "jsonPointer": v.json_pointer, "command": v.command.is_some() })),
        "auth": e.auth.as_ref().map(|a| json!({ "user": a.user, "secret": a.password, "except": a.except })),
        "logs": e.logs.iter().map(|l| json!({ "name": l.name, "command": l.command })).collect::<Vec<_>>(),
        "commands": e.commands.iter().map(|c| json!({ "name": c.name, "command": c.command, "confirm": c.confirm })).collect::<Vec<_>>(),
        "deploy": e.deploy.as_ref().map(|d| json!({
            "command": d.command, "local": d.local, "confirm": d.confirm, "requireGreenPipeline": d.require_green_pipeline,
            "onlyRef": d.only_ref, "after": d.after,
        })),
    })
}

fn preview_info(state: &AppState, pid: &str, e: &Environment) -> PreviewInfo {
    if e.auth.is_some() {
        return PreviewInfo { mode: "proxy", reason: Some("basic auth is injected by a local proxy".into()) };
    }
    let blocked = state.apps.envs.live.lock().get(&key(pid, &e.name)).and_then(|l| l.frame_blocked).map(|(b, _)| b);
    match blocked {
        Some(true) => PreviewInfo { mode: "proxy", reason: Some("the site forbids framing; a local proxy strips those headers".into()) },
        _ => PreviewInfo { mode: "direct", reason: None },
    }
}

pub fn list(state: &AppState, project: &Project) -> Vec<EnvView> {
    let views = project
        .config
        .envs
        .iter()
        .map(|e| {
            let (health, version) = {
                let live = state.apps.envs.live.lock();
                let l = live.get(&key(&project.id, &e.name));
                (l.map(|l| l.health.clone()).unwrap_or_default(), l.and_then(|l| l.version.clone()))
            };
            EnvView {
                name: e.name.clone(),
                kind: e.kind,
                url: e.url.clone(),
                config: config_view(project, e),
                health,
                version,
                preview: preview_info(state, &project.id, e),
                deploying: super::deploy::deploying_terminal(state, &project.id, &e.name),
            }
        })
        .collect();
    // Probe framing headers lazily for envs that do not need the proxy anyway.
    for e in project.config.envs.iter().filter(|e| e.auth.is_none()) {
        let due = state.apps.envs.live.lock().get(&key(&project.id, &e.name)).and_then(|l| l.frame_blocked).is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(1800));
        if due {
            let st = state.clone();
            let pid = project.id.clone();
            let e = e.clone();
            tokio::spawn(async move { probe_frame(&st, &pid, &e).await });
        }
    }
    views
}

pub fn find<'a>(project: &'a Project, name: &str) -> Result<&'a Environment, ApiError> {
    project
        .config
        .envs
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| ApiError::not_found(format!("no environment {name:?} in {}", project.id)))
}

async fn probe_frame(state: &AppState, pid: &str, e: &Environment) {
    // Mark as probed first so concurrent list() calls do not stampede.
    let before = {
        let mut live = state.apps.envs.live.lock();
        let l = live.entry(key(pid, &e.name)).or_default();
        let before = l.frame_blocked.map(|(b, _)| b);
        l.frame_blocked = Some((before.unwrap_or(false), Instant::now()));
        before
    };
    let Ok(resp) = state.http.get(&e.url).timeout(Duration::from_secs(8)).send().await else { return };
    let h = resp.headers();
    let xfo = h.get("x-frame-options").and_then(|v| v.to_str().ok());
    let csp = h.get("content-security-policy").and_then(|v| v.to_str().ok());
    let blocked = health::frame_blocked(xfo, csp);
    state.apps.envs.live.lock().entry(key(pid, &e.name)).or_default().frame_blocked = Some((blocked, Instant::now()));
    if before != Some(blocked) && blocked {
        state.events.emit("env.health", Some(pid), json!({ "env": e.name, "previewChanged": true }));
    }
}

// ---------------------------------------------------------------- health checks

struct CheckOutcome {
    status: HealthStatus,
    http_status: Option<u16>,
    latency_ms: Option<u64>,
    error: Option<String>,
}

/// Run one health check now (serialized per env), record it and emit `env.health`.
pub async fn check(state: &AppState, project: &Project, e: &Environment) -> HealthView {
    let lock = state.apps.envs.locks.entry(key(&project.id, &e.name)).or_default().clone();
    let _guard = lock.lock().await;
    let outcome = match &e.health {
        None => CheckOutcome { status: HealthStatus::Unknown, http_status: None, latency_ms: None, error: Some("no health probe configured".into()) },
        Some(h) if h.via_host => check_via_host(project, e, h).await,
        Some(h) => {
            let first = check_http(state, project, e, h).await;
            if first.status == HealthStatus::Down {
                tokio::time::sleep(Duration::from_secs(2)).await;
                check_http(state, project, e, h).await
            } else {
                first
            }
        }
    };
    if e.version.as_ref().is_some_and(|v| v.http.is_some()) && outcome.status != HealthStatus::Down {
        let _ = probe_version_http(state, project, e).await;
    }
    record(state, project, e, outcome)
}

fn record(state: &AppState, project: &Project, e: &Environment, o: CheckOutcome) -> HealthView {
    let now = crate::util::now_ms();
    let (prev, view, version) = {
        let mut live = state.apps.envs.live.lock();
        let l = live.entry(key(&project.id, &e.name)).or_default();
        let prev = l.health.status;
        l.health.status = o.status;
        l.health.http_status = o.http_status;
        l.health.latency_ms = o.latency_ms;
        l.health.checked_at = Some(now);
        l.health.error = o.error.clone();
        if o.status != HealthStatus::Unknown {
            l.health.history.push_back(Sample { t: now, ok: o.status == HealthStatus::Up, ms: o.latency_ms });
            while l.health.history.len() > HISTORY {
                l.health.history.pop_front();
            }
        }
        (prev, l.health.clone(), l.version.clone())
    };
    state.events.emit(
        "env.health",
        Some(&project.id),
        json!({
            "env": e.name,
            "status": view.status,
            "httpStatus": view.http_status,
            "latencyMs": view.latency_ms,
            "version": version.as_ref().and_then(|v| v.sha.clone()),
            "checkedAt": now,
            "error": view.error,
            "sample": view.history.back(),
        }),
    );
    let was_up = matches!(prev, HealthStatus::Up | HealthStatus::Degraded);
    match (was_up, prev, o.status) {
        (true, _, HealthStatus::Down) => state.events.notify(
            "error",
            &format!("{} · {} is down{}", project.name, e.name, o.error.as_deref().map(|x| format!(": {x}")).unwrap_or_default()),
        ),
        (_, HealthStatus::Down, HealthStatus::Up) => state.events.notify("success", &format!("{} · {} is back up", project.name, e.name)),
        _ => {}
    }
    view
}

/// GET with the env's basic auth when the URL's path is guarded.
fn authed_get(state: &AppState, project: &Project, e: &Environment, url: &reqwest::Url, timeout: Duration) -> Result<reqwest::RequestBuilder, ApiError> {
    let mut req = state.http.get(url.clone()).timeout(timeout);
    if let Some(auth) = &e.auth {
        if health::needs_auth(auth, url.path()) {
            let pw = state.secret(Some(project), &auth.password).map_err(|err| {
                ApiError::not_configured(format!(
                    "{} needs basic auth for {}: store the password as secret {:?} ({})",
                    e.name,
                    url.path(),
                    auth.password,
                    err.message
                ))
            })?;
            req = req.basic_auth(&auth.user, Some(pw.expose()));
        }
    }
    Ok(req)
}

async fn check_http(state: &AppState, project: &Project, e: &Environment, h: &crate::config::project::Health) -> CheckOutcome {
    let unknown = |msg: String| CheckOutcome { status: HealthStatus::Unknown, http_status: None, latency_ms: None, error: Some(msg) };
    let url = match reqwest::Url::parse(&h.url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u,
        _ => return unknown(format!("health url {:?} is not an http(s) URL", h.url)),
    };
    let timeout = Duration::from_millis(u64::from(h.timeout_ms).clamp(200, 60_000));
    let req = match authed_get(state, project, e, &url, timeout) {
        Ok(r) => r,
        Err(err) => return unknown(err.message),
    };
    let t0 = Instant::now();
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let json_body = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("json"));
            let body = if json_body || h.json_pointer.is_some() { read_capped(resp, MAX_BODY).await } else { None };
            let ms = t0.elapsed().as_millis() as u64;
            let parsed: Option<Value> = body.and_then(|b| serde_json::from_slice(&b).ok());
            let (s, err) = health::evaluate(h.expect_status, status, parsed.as_ref(), h.json_pointer.as_deref(), h.equals.as_deref());
            CheckOutcome { status: s, http_status: Some(status), latency_ms: Some(ms), error: err }
        }
        Err(err) => CheckOutcome { status: HealthStatus::Down, http_status: None, latency_ms: None, error: Some(describe(&err, timeout)) },
    }
}

/// Read at most `cap` bytes of a response body.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if buf.len() + chunk.len() > cap {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    Some(buf)
}

/// A reqwest error as a short message without the URL (which could carry userinfo).
pub fn describe(err: &reqwest::Error, timeout: Duration) -> String {
    if err.is_timeout() {
        format!("timed out after {} ms", timeout.as_millis())
    } else if err.is_connect() {
        "connection failed".into()
    } else if err.is_redirect() {
        "too many redirects".into()
    } else {
        let msg = err.to_string();
        let mut s = match err.url() {
            Some(u) => msg.replace(u.as_str(), "<url>"),
            None => msg,
        };
        s.truncate(300);
        s
    }
}

/// The probe run on the host for `health.via_host`: prints `<http_code> <seconds>`.
/// Auth is never passed: a host-local port is not behind the site's basic auth, and a
/// password must not appear in a remote argv.
pub fn via_host_command(url: &str, secs: u64) -> String {
    format!("curl -sS -o /dev/null -w '%{{http_code}} %{{time_total}}' --max-time {secs} {}", super::expand::shell_quote(url))
}

/// Parse `via_host_command` output: `(status, latency_ms)`; status 0 means no response.
pub fn parse_via_host(stdout: &str) -> (Option<u16>, Option<u64>) {
    let mut parts = stdout.split_whitespace();
    let code = parts.next().and_then(|c| c.parse::<u16>().ok()).filter(|c| *c > 0);
    let ms = parts.next().and_then(|t| t.parse::<f64>().ok()).map(|t| (t * 1000.0).round() as u64);
    (code, ms)
}

async fn check_via_host(project: &Project, e: &Environment, h: &crate::config::project::Health) -> CheckOutcome {
    let target = match remote::env_target(project, e) {
        Ok(t) => t,
        Err(err) => return CheckOutcome { status: HealthStatus::Unknown, http_status: None, latency_ms: None, error: Some(err.message) },
    };
    let secs = (u64::from(h.timeout_ms) / 1000).clamp(1, 60);
    let argv = remote::argv(&target, &via_host_command(&h.url, secs), false);
    let mut c = tokio::process::Command::new(&argv[0]);
    c.args(&argv[1..]).current_dir(&project.root);
    match crate::util::proc::run_cmd(c, Duration::from_secs(secs + 20)).await {
        Ok(out) => {
            let (code, ms) = parse_via_host(&out.stdout);
            match code {
                Some(code) => {
                    let (s, err) = health::evaluate(h.expect_status, code, None, None, None);
                    CheckOutcome { status: s, http_status: Some(code), latency_ms: ms, error: err }
                }
                None => CheckOutcome {
                    status: HealthStatus::Down,
                    http_status: None,
                    latency_ms: None,
                    error: Some(crate::apps::detect::text::ellipsize(&out.message(), 200)),
                },
            }
        }
        Err(err) => CheckOutcome { status: HealthStatus::Unknown, http_status: None, latency_ms: None, error: Some(err.message) },
    }
}

// ---------------------------------------------------------------- versions

async fn probe_version_http(state: &AppState, project: &Project, e: &Environment) -> Result<VersionInfo, ApiError> {
    let v = e.version.as_ref().ok_or_else(|| ApiError::not_configured(format!("{} has no version probe", e.name)))?;
    let url_s = v.http.as_deref().ok_or_else(|| ApiError::not_configured("no version.http"))?;
    let url = reqwest::Url::parse(url_s).map_err(|_| ApiError::bad_request(format!("bad version url {url_s:?}")))?;
    let timeout = Duration::from_secs(8);
    let resp = authed_get(state, project, e, &url, timeout)?.send().await.map_err(|err| ApiError::upstream(describe(&err, timeout)))?;
    let status = resp.status();
    let body = read_capped(resp, MAX_BODY).await.unwrap_or_default();
    let info = if !status.is_success() {
        VersionInfo { sha: None, raw: None, checked_at: crate::util::now_ms(), source: "http", error: Some(format!("HTTP {}", status.as_u16())) }
    } else {
        let text = String::from_utf8_lossy(&body).into_owned();
        let raw = match (&v.json_pointer, serde_json::from_str::<Value>(&text)) {
            (Some(ptr), Ok(json)) => json.pointer(ptr).map(health::value_string),
            _ => Some(text.trim().to_string()),
        }
        .map(|r| crate::apps::detect::text::ellipsize(&r, 200));
        let pattern = v.pattern.as_deref().and_then(|p| Regex::new(p).ok());
        let sha = raw.as_deref().and_then(|r| health::extract_sha(r, pattern.as_ref()));
        VersionInfo { sha, raw, checked_at: crate::util::now_ms(), source: "http", error: None }
    };
    state.apps.envs.set_version(&project.id, &e.name, info.clone());
    Ok(info)
}

/// Probe the deployed version now: `version.http`, else `version.command` (over ssh
/// unless the env is local). Emits `env.health` with the version.
pub async fn probe_version(state: &AppState, project: &Project, e: &Environment) -> Result<VersionInfo, ApiError> {
    let v = e.version.as_ref().ok_or_else(|| ApiError::not_configured(format!("{} has no [env.version] probe", e.name)))?;
    let info = if v.http.is_some() {
        probe_version_http(state, project, e).await?
    } else if let Some(cmd) = &v.command {
        let target = remote::env_target(project, e)?;
        let argv = remote::argv(&target, cmd, false);
        let mut c = tokio::process::Command::new(&argv[0]);
        c.args(&argv[1..]).current_dir(&project.root);
        let out = crate::util::proc::run_cmd(c, Duration::from_secs(40)).await?;
        let info = if out.ok() {
            let raw = out.stdout.trim().to_string();
            let pattern = match v.pattern.as_deref() {
                Some(p) => Some(Regex::new(p).map_err(|err| ApiError::bad_request(format!("version.pattern: {err}")))?),
                None => None,
            };
            let sha = health::extract_sha(&raw, pattern.as_ref());
            VersionInfo {
                sha,
                raw: Some(crate::apps::detect::text::ellipsize(&raw, 500)),
                checked_at: crate::util::now_ms(),
                source: "command",
                error: None,
            }
        } else {
            VersionInfo {
                sha: None,
                raw: None,
                checked_at: crate::util::now_ms(),
                source: "command",
                error: Some(crate::apps::detect::text::ellipsize(&out.message(), 300)),
            }
        };
        state.apps.envs.set_version(&project.id, &e.name, info.clone());
        info
    } else {
        return Err(ApiError::not_configured(format!("{}: [env.version] needs http or command", e.name)));
    };
    let h = state.apps.envs.health(&project.id, &e.name);
    state.events.emit(
        "env.health",
        Some(&project.id),
        json!({ "env": e.name, "status": h.status, "httpStatus": h.http_status, "latencyMs": h.latency_ms,
                "checkedAt": h.checked_at, "version": info.sha, "versionInfo": info }),
    );
    Ok(info)
}

// ---------------------------------------------------------------- pollers

fn fingerprint(e: &Environment) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&(&e.url, &e.health, &e.auth, &e.version, &e.host)).unwrap_or_default().hash(&mut h);
    h.finish()
}

/// Keep exactly one poller per env with a health probe.
pub fn reconcile(state: &AppState) {
    let mut desired: HashMap<Key, (u64, Environment)> = HashMap::new();
    for p in state.projects.list() {
        for e in p.config.envs.iter().filter(|e| e.health.is_some()) {
            desired.insert(key(&p.id, &e.name), (fingerprint(e), e.clone()));
        }
    }
    let mut pollers = state.apps.envs.pollers.lock();
    pollers.retain(|k, (fp, handle)| {
        let keep = desired.get(k).is_some_and(|(d, _)| d == fp);
        if !keep {
            handle.abort();
        }
        keep
    });
    let known: std::collections::HashSet<Key> = desired.keys().cloned().collect();
    state.apps.envs.live.lock().retain(|k, _| known.contains(k) || state.projects.get(&k.0).is_some_and(|p| p.config.envs.iter().any(|e| e.name == k.1)));
    for (k, (fp, e)) in desired {
        if pollers.contains_key(&k) {
            continue;
        }
        let st = state.clone();
        let (pid, name) = k.clone();
        let interval = Duration::from_secs(u64::from(e.health.as_ref().map(|h| h.interval_s).unwrap_or(60)).clamp(10, 86_400));
        let cancel = state.apps.shutdown.clone();
        let handle = tokio::spawn(async move {
            // Spread the first checks so a reload does not fire them all at once.
            let jitter = Duration::from_millis(u64::from(rand::random::<u16>() % 3000));
            tokio::select! { _ = cancel.cancelled() => return, _ = tokio::time::sleep(jitter) => {} }
            loop {
                let Some(project) = st.projects.get(&pid) else { return };
                let Some(env) = project.config.envs.iter().find(|x| x.name == name).cloned() else { return };
                check(&st, &project, &env).await;
                tokio::select! { _ = cancel.cancelled() => return, _ = tokio::time::sleep(interval) => {} }
            }
        });
        pollers.insert(k, (fp, handle.abort_handle()));
    }
}

/// Background: reconcile pollers now, on `projects.changed`, and every 15 s.
pub fn start_supervisor(state: &AppState) {
    let st = state.clone();
    let cancel = state.apps.shutdown.clone();
    let mut events = state.events.subscribe();
    tokio::spawn(async move {
        reconcile(&st);
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => reconcile(&st),
                ev = events.recv() => match ev {
                    Ok(ev) if ev.kind == "projects.changed" => reconcile(&st),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                },
            }
        }
        for (_, (_, h)) in st.apps.envs.pollers.lock().drain() {
            h.abort();
        }
    });
}

// ---------------------------------------------------------------- logs & commands

/// Open a terminal following one of the env's log commands.
pub async fn open_logs(state: &AppState, project: &Project, e: &Environment, which: Option<&str>) -> Result<TerminalInfo, ApiError> {
    let log = match which {
        Some(n) => e.logs.iter().find(|l| l.name == n).ok_or_else(|| ApiError::not_found(format!("{} has no log {n:?}", e.name)))?,
        None => e.logs.first().ok_or_else(|| ApiError::not_configured(format!("{} has no [[env.logs]] commands", e.name)))?,
    };
    let target = remote::env_target(project, e)?;
    let argv = remote::argv(&target, &log.command, true);
    state
        .terminals
        .spawn(
            state,
            SpawnSpec {
                kind: TerminalKind::Command,
                title: format!("{} logs · {}", e.name, log.name),
                project_id: Some(project.id.clone()),
                cwd: project.root.clone(),
                argv,
                env: vec![],
                cols: None,
                rows: None,
                meta: json!({ "env": e.name, "action": "logs", "log": log.name, "target": target.label() }),
            },
        )
        .await
}

/// Run one of the env's custom commands (confirmed by the user when it asks for it).
pub async fn run_command(state: &AppState, project: &Project, e: &Environment, name: &str, confirmed: bool) -> Result<TerminalInfo, ApiError> {
    let c = e.commands.iter().find(|c| c.name == name).ok_or_else(|| ApiError::not_found(format!("{} has no command {name:?}", e.name)))?;
    if c.confirm && !confirmed {
        return Err(ApiError::new(axum::http::StatusCode::PRECONDITION_REQUIRED, "confirmation_required", format!("{name} on {} needs confirmation", e.name)));
    }
    let target = remote::env_target(project, e)?;
    let argv = remote::argv(&target, &c.command, false);
    state
        .terminals
        .spawn(
            state,
            SpawnSpec {
                kind: TerminalKind::Command,
                title: format!("{} · {}", e.name, c.name),
                project_id: Some(project.id.clone()),
                cwd: project.root.clone(),
                argv,
                env: vec![],
                cols: None,
                rows: None,
                meta: json!({ "env": e.name, "action": "command", "command": c.name, "target": target.label() }),
            },
        )
        .await
}

/// Probe a target for the MCP `env_status` tool: health + version per env.
pub fn status_json(state: &AppState, project: &Project) -> Value {
    json!(project.config.envs.iter().map(|e| {
        let h = state.apps.envs.health(&project.id, &e.name);
        let v = state.apps.envs.version(&project.id, &e.name);
        json!({
            "env": e.name, "kind": e.kind, "url": e.url, "status": h.status, "httpStatus": h.http_status,
            "latencyMs": h.latency_ms, "checkedAt": h.checked_at, "error": h.error,
            "uptime": if h.history.is_empty() { Value::Null } else {
                json!(format!("{}/{} recent checks ok", h.history.iter().filter(|s| s.ok).count(), h.history.len()))
            },
            "version": v.as_ref().and_then(|v| v.sha.clone()), "versionCheckedAt": v.as_ref().map(|v| v.checked_at),
        })
    }).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn via_host_probe_command_and_output() {
        let c = via_host_command("http://127.0.0.1:8081/api/health", 5);
        assert_eq!(c, "curl -sS -o /dev/null -w '%{http_code} %{time_total}' --max-time 5 http://127.0.0.1:8081/api/health");
        assert!(via_host_command("http://x/a b", 5).ends_with("'http://x/a b'"));
        assert_eq!(parse_via_host("200 0.0123"), (Some(200), Some(12)));
        assert_eq!(parse_via_host("000 5.001"), (None, Some(5001)));
        assert_eq!(parse_via_host(""), (None, None));
    }

    #[test]
    fn fingerprints_change_with_probe_config() {
        let mut e = Environment { name: "s".into(), url: "https://x".into(), ..Default::default() };
        let a = fingerprint(&e);
        e.health = Some(crate::config::project::Health { url: "https://x/h".into(), interval_s: 60, ..Default::default() });
        assert_ne!(a, fingerprint(&e));
    }
}

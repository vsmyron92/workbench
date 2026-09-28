//! Web Push (OWNER: platform slice): notifications on phones and other devices
//! while Workbench is closed or in the background, through the browser's push
//! service (VAPID, RFC 8291 message encryption).
//!
//! * `vapid.rs` — the server's P-256 identity (`data_dir/push/vapid.json`) and the
//!   ES256 JWT of RFC 8292.
//! * `ece.rs` — RFC 8291 / RFC 8188 `aes128gcm` payload encryption.
//! * `endpoint.rs` — the push endpoint allowlist (an SSRF boundary).
//! * `routes.rs` — `/api/push/**`.
//! * here — subscriptions (`data_dir/push/subscriptions.json`, one per device
//!   session), device presence, the delivery queue with per-tag coalescing, and
//!   delivery with bounded retries.
//!
//! Notes come from `notify.rs`'s listener (the same events and rate limits as
//! desktop notifications) and carry titles and short summaries only.

pub mod ece;
pub mod endpoint;
mod routes;
pub mod vapid;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, mpsc};

use crate::app::AppState;
use crate::terminals::{AgentInfo, AgentState};
use crate::util;

pub use crate::terminals::PendingPermission;

pub use routes::routes;

/// `[push]` in config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PushConfig {
    /// VAPID contact (`sub`): `mailto:you@example.com` or an https URL. Default:
    /// `server.public_url` when it is https, else the https origin a device
    /// subscribed from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Push service hosts accepted besides the browsers' own (Google FCM, Mozilla,
    /// Apple, Microsoft WNS): exact names, or `*.example.com` for subdomains.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_endpoint_hosts: Vec<String>,
}

/// What a notification is about; each device picks the topics it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    /// An agent waits: a permission request, a question, an error.
    Attention,
    /// An agent finished its turn.
    Done,
    /// An environment went down or recovered.
    Env,
    /// A deploy finished.
    Deploy,
    /// A pipeline or workflow run failed.
    Pipeline,
    /// An agent's `workbench_notify`.
    Notify,
    /// The Settings test button (always delivered).
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Topics {
    pub attention: bool,
    pub done: bool,
    pub env: bool,
    pub deploy: bool,
    pub pipeline: bool,
    pub notify: bool,
}

impl Default for Topics {
    fn default() -> Self {
        Self { attention: true, done: true, env: true, deploy: true, pipeline: true, notify: true }
    }
}

impl Topics {
    pub fn wants(&self, t: Topic) -> bool {
        match t {
            Topic::Attention => self.attention,
            Topic::Done => self.done,
            Topic::Env => self.env,
            Topic::Deploy => self.deploy,
            Topic::Pipeline => self.pipeline,
            Topic::Notify => self.notify,
            Topic::Test => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low,
    Normal,
    High,
}

impl Urgency {
    fn header(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// The panel a notification opens when tapped (a `ui.open`-style target).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    pub kind: String,
    pub id: String,
    pub params: Value,
}

impl OpenTarget {
    pub fn terminal(terminal_id: &str) -> Self {
        Self { kind: "terminal".into(), id: format!("terminal:{terminal_id}"), params: json!({ "terminalId": terminal_id }) }
    }
}

/// An agent attention note: it goes out only while the session still waits in
/// `state` once the coalescing window has passed, and then carries the permission
/// request the session waits on (Allow / Deny), when Workbench can answer it.
#[derive(Debug, Clone)]
pub struct AgentWait {
    pub terminal_id: String,
    pub state: String,
    /// The permission id a `terminal.updated` saw (dropped when it was already pushed).
    pub permission_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PushNote {
    pub topic: Topic,
    /// Coalescing key: a newer note with the same tag replaces an older one, in the
    /// queue, at the push service (`Topic` header) and on the device (notification tag).
    pub tag: String,
    pub title: String,
    pub body: String,
    /// info | success | warning | error
    pub level: &'static str,
    pub urgency: Urgency,
    pub ttl_secs: u32,
    pub project_id: Option<String>,
    pub open: Option<OpenTarget>,
    pub agent: Option<AgentWait>,
}

/// Most a permission request may hold for a notification to offer a one-tap Allow:
/// the terminals' rule for the attention toast (`oneTapAllow` in
/// `features/agents/lib/permission.ts`).
const GLANCE_CHARS: usize = 300;
const GLANCE_LINES: usize = 4;

/// What a one-tap surface shows of a request: its whole `detail`, else its summary.
fn glance_text(p: &PendingPermission) -> &str {
    if p.detail.is_empty() { &p.summary } else { &p.detail }
}

/// Whether a notification may offer Allow for `p`: the request is whole (nothing cut
/// or masked: `complete`) and short enough for the notification to show all of it.
/// Otherwise it offers Deny and "Review" (open the session) only.
pub fn one_tap_allow(p: &PendingPermission) -> bool {
    let text = glance_text(p);
    p.complete && text.chars().count() <= GLANCE_CHARS && text.split('\n').count() <= GLANCE_LINES
}

/// `AgentState` as events name it (`needs_permission`…).
fn state_name(s: AgentState) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// A session's state (as `agent.attention` names it) and the permission request it
/// waits on (`AgentInfo.pendingPermission`, terminals contract).
pub fn agent_wait_state(agent: Option<&AgentInfo>) -> (Option<String>, Option<PendingPermission>) {
    let Some(a) = agent else { return (None, None) };
    (Some(state_name(a.state)), a.pending_permission.clone().filter(|p| !p.id.is_empty()))
}

/// The agent of a `TerminalInfo` carried by an event (`terminal.updated`).
pub fn agent_of_event(info: &Value) -> Option<AgentInfo> {
    info.get("agent").filter(|a| a.is_object()).and_then(|a| serde_json::from_value(a.clone()).ok())
}

/// Whether an agent note still applies, given the session's agent and the permission
/// request last pushed for it: `None` = drop it; `Some(perm)` = send it, with the
/// pending permission request when it waits on one.
pub fn decide_agent(wait: &AgentWait, agent: Option<&AgentInfo>, last_pushed: Option<&str>) -> Option<Option<PendingPermission>> {
    let (agent_state, perm) = agent_wait_state(agent);
    if agent_state.as_deref() != Some(wait.state.as_str()) {
        return None;
    }
    let perm = perm.filter(|_| wait.state == "needs_permission");
    match &perm {
        // Pushed already (the attention event and a `terminal.updated` both saw it).
        Some(p) if last_pushed == Some(p.id.as_str()) => None,
        // A sighting of a request that is gone again.
        None if wait.permission_hint.is_some() => None,
        _ => Some(perm),
    }
}

/// `[agents] permission_wait` (terminals): how long a permission request stays
/// answerable from Workbench, within the terminals' bounds (30–3600 s).
pub fn permission_wait_secs(state: &AppState) -> u64 {
    crate::terminals::permission_wait_secs(state.config.read().agents.permission_wait)
}

/// The push `TTL` of a permission request: no longer than it stays answerable, so a
/// phone that comes back online later is not offered Allow / Deny for a request
/// Workbench can no longer answer (0: deliver now or not at all).
pub fn permission_ttl(ttl_secs: u32, wait_secs: u64, since_ms: Option<i64>, now_ms: i64) -> u32 {
    let wait_ms = i64::try_from(wait_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let left_ms = match since_ms {
        Some(since) if since > 0 && since <= now_ms => since.saturating_add(wait_ms).saturating_sub(now_ms),
        _ => wait_ms,
    };
    u32::try_from(left_ms.max(0) / 1000).unwrap_or(u32::MAX).min(ttl_secs)
}

// ---------------------------------------------------------------- subscriptions

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    id: String,
    /// The device session it belongs to: it ends with the session.
    session_id: String,
    device: String,
    endpoint: String,
    /// base64url, uncompressed P-256 point.
    p256dh: String,
    /// base64url, 16 bytes.
    auth: String,
    /// The https origin the device subscribed from (VAPID `sub` fallback).
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    user_agent: String,
    created_at: i64,
    #[serde(default)]
    topics: Topics,
    /// Hold pushes while another device is in active use.
    #[serde(default = "yes")]
    quiet_when_active: bool,
    #[serde(default)]
    last_ok_at: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_error_at: Option<i64>,
    #[serde(default)]
    failures: u32,
}

fn yes() -> bool {
    true
}

/// A subscription as the UI sees it (no endpoint path, no keys).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubInfo {
    pub id: String,
    pub session_id: String,
    pub device: String,
    /// "Google (FCM)", "Apple", … or the host.
    pub service: String,
    pub created_at: i64,
    pub topics: Topics,
    pub quiet_when_active: bool,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    /// The device asking.
    pub current: bool,
    /// First 16 hex digits of the endpoint's SHA-256: the page checks that the
    /// server still has its browser's current subscription.
    pub endpoint_hash: String,
}

impl Subscription {
    fn info(&self, current_session: Option<&str>) -> SubInfo {
        let host = reqwest::Url::parse(&self.endpoint).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
        SubInfo {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            device: self.device.clone(),
            service: endpoint::service_name(&host).map(str::to_string).unwrap_or(host),
            created_at: self.created_at,
            topics: self.topics.clone(),
            quiet_when_active: self.quiet_when_active,
            last_ok_at: self.last_ok_at,
            last_error: self.last_error.clone(),
            last_error_at: self.last_error_at,
            current: current_session == Some(self.session_id.as_str()),
            endpoint_hash: hex::encode(Sha256::digest(self.endpoint.as_bytes()))[..16].to_string(),
        }
    }
}

/// At most this many subscriptions in all (one per device session).
const MAX_SUBSCRIPTIONS: usize = 64;

// ---------------------------------------------------------------- delivery

/// Newer notes with the same tag within this window replace older ones; it also
/// gives a permission request time to show up on the agent (`pendingPermission`).
const COALESCE: Duration = Duration::from_millis(700);
/// A device that reported "visible" is treated as looking for this long.
const PRESENCE_FOR: Duration = Duration::from_secs(75);
const QUEUE: usize = 256;
const ATTEMPTS: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
const CONCURRENT_SENDS: usize = 8;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryResult {
    pub subscription_id: String,
    pub device: String,
    /// sent | gone (the push service forgot it: removed) | failed | skipped | superseded
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
struct Presence {
    until: Instant,
    /// The user interacted with the page recently (not just an open tab).
    active: bool,
}

/// Tabs remembered per device session (a runaway page cannot grow the map).
const MAX_TABS: usize = 32;

/// What `try_send` of one attempt gave.
enum Attempt {
    Sent(u16),
    Gone(u16),
    Retry(Option<u16>, Option<Duration>, String),
    Failed(Option<u16>, String),
}

pub struct Engine {
    dir: PathBuf,
    vapid: vapid::Vapid,
    subs: RwLock<Vec<Subscription>>,
    /// Serializes writes of `subscriptions.json`; each write takes the latest snapshot.
    write_lock: Mutex<()>,
    /// Device session → page (tab) → its last report. Every tab of a browser shares
    /// the session: the device looks at Workbench while any of its tabs does.
    presence: Mutex<HashMap<String, HashMap<String, Presence>>>,
    tx: mpsc::Sender<PushNote>,
    client: reqwest::Client,
    seq: AtomicU64,
    /// Newest note sequence per (subscription, tag): a retry of an older one stops.
    latest: Mutex<HashMap<(String, String), u64>>,
    /// The last permission request pushed per terminal.
    permissions: Mutex<HashMap<String, String>>,
    sends: Semaphore,
    last_test: Mutex<Option<Instant>>,
    #[cfg(test)]
    test_mode: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
pub struct PushState {
    engine: OnceLock<Arc<Engine>>,
    /// Why push could not start (no randomness, data dir not writable).
    error: OnceLock<String>,
}

impl PushState {
    pub fn engine(&self) -> Option<&Arc<Engine>> {
        self.engine.get()
    }

    /// Queue a note for every subscribed device that wants it. Never blocks.
    /// Returns what happened: `queued`, `none` (no device subscribed), `full`, `off`.
    pub fn enqueue(&self, note: PushNote) -> &'static str {
        let Some(engine) = self.engine.get() else { return "off" };
        if engine.subs.read().is_empty() {
            return "none";
        }
        match engine.tx.try_send(note) {
            Ok(()) => "queued",
            Err(_) => {
                tracing::warn!("push: queue full, dropping a notification");
                "full"
            }
        }
    }

    /// A `terminal.updated` showed a pending permission request: push it unless
    /// the attention note already did (the worker checks, in order).
    pub fn permission_seen(&self, note: PushNote) {
        let _ = self.enqueue(note);
    }
}

fn clean(s: &str, max: usize) -> String {
    let s: String = s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
    super::truncate_chars(s.trim(), max)
}

/// The `Topic` header: at most 32 characters of the URL-safe base64 alphabet.
fn topic_header(tag: &str) -> String {
    B64.encode(Sha256::digest(tag.as_bytes()))[..32].to_string()
}

fn b64_field(s: &str) -> Option<Vec<u8>> {
    // Browsers give base64url without padding; accept padded or standard base64 too.
    let norm: String = s
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    B64.decode(norm).ok()
}

impl Engine {
    fn new(dir: PathBuf, vapid: vapid::Vapid, subs: Vec<Subscription>, tx: mpsc::Sender<PushNote>) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("workbench/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(REQUEST_TIMEOUT)
            // The endpoint is browser-supplied: never follow it elsewhere.
            .redirect(reqwest::redirect::Policy::none())
            .https_only(!cfg!(test))
            .build()?;
        Ok(Self {
            dir,
            vapid,
            subs: RwLock::new(subs),
            write_lock: Mutex::new(()),
            presence: Mutex::new(HashMap::new()),
            tx,
            client,
            seq: AtomicU64::new(0),
            latest: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            sends: Semaphore::new(CONCURRENT_SENDS),
            last_test: Mutex::new(None),
            #[cfg(test)]
            test_mode: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn public_key(&self) -> &str {
        self.vapid.public_key()
    }

    /// Tests only: deliver to a mock push service on `http://127.0.0.1`.
    #[cfg(test)]
    pub fn allow_loopback_http_for_tests(&self) {
        self.test_mode.store(true, Ordering::Relaxed);
    }

    fn file(&self) -> PathBuf {
        self.dir.join("subscriptions.json")
    }

    /// Write the current subscriptions (0600) off the async threads.
    async fn persist(self: &Arc<Self>) {
        let this = self.clone();
        let res = tokio::task::spawn_blocking(move || {
            let _w = this.write_lock.lock();
            let snapshot = this.subs.read().clone();
            vapid::create_private_dir(&this.dir)?;
            util::fs::write_json(&this.file(), &snapshot)?;
            util::fs::set_mode(&this.file(), 0o600);
            anyhow::Ok(())
        })
        .await;
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("push: cannot save subscriptions: {e:#}"),
            Err(e) => tracing::warn!("push: cannot save subscriptions: {e}"),
        }
    }

    fn policy(&self, state: &AppState) -> endpoint::EndpointPolicy {
        #[allow(unused_mut)]
        let mut p = endpoint::EndpointPolicy::new(&state.config.read().push.extra_endpoint_hosts);
        #[cfg(test)]
        {
            p.allow_loopback_http = self.test_mode.load(Ordering::Relaxed);
        }
        p
    }

    /// The VAPID `sub`: `[push] subject`, else an https `public_url`, else the https
    /// origin the device subscribed from.
    fn subject(&self, state: &AppState, sub: &Subscription) -> String {
        let cfg = state.config.read();
        if let Some(s) = cfg.push.subject.as_deref().map(str::trim).filter(|s| valid_subject(s)) {
            return s.to_string();
        }
        if let Some(u) = cfg.server.public_url.as_deref().filter(|u| u.starts_with("https://")) {
            return u.trim_end_matches('/').to_string();
        }
        if let Some(o) = sub.origin.as_deref().filter(|o| o.starts_with("https://")) {
            return o.to_string();
        }
        "mailto:workbench@localhost".into()
    }

    pub fn list(&self, current_session: Option<&str>) -> Vec<SubInfo> {
        let mut out: Vec<SubInfo> = self.subs.read().iter().map(|s| s.info(current_session)).collect();
        out.sort_by(|a, b| b.current.cmp(&a.current).then(b.created_at.cmp(&a.created_at)));
        out
    }

    // ------------------------------------------------------------ presence

    /// A page (`tab`: its own random id; "" for a page from before tab ids) of
    /// `session` reports whether it is shown. Hiding one tab leaves the others.
    pub fn report_presence(&self, session: &str, tab: &str, visible: bool, active: bool) {
        let mut p = self.presence.lock();
        let now = Instant::now();
        p.retain(|_, tabs| {
            tabs.retain(|_, v| v.until > now);
            !tabs.is_empty()
        });
        if visible {
            let tabs = p.entry(session.to_string()).or_default();
            if tabs.len() >= MAX_TABS && !tabs.contains_key(tab) {
                // The report seen longest ago goes.
                if let Some(oldest) = tabs.iter().min_by_key(|(_, v)| v.until).map(|(k, _)| k.clone()) {
                    tabs.remove(&oldest);
                }
            }
            tabs.insert(tab.to_string(), Presence { until: now + PRESENCE_FOR, active });
        } else if let Some(tabs) = p.get_mut(session) {
            tabs.remove(tab);
            if tabs.is_empty() {
                p.remove(session);
            }
        }
    }

    /// Some tab of this device session shows Workbench.
    fn visible(&self, session: &str) -> bool {
        let now = Instant::now();
        self.presence.lock().get(session).is_some_and(|tabs| tabs.values().any(|p| p.until > now))
    }

    /// Some tab of another device session shows Workbench and was used recently.
    fn active_elsewhere(&self, session: &str) -> bool {
        let now = Instant::now();
        self.presence.lock().iter().any(|(s, tabs)| s != session && tabs.values().any(|p| p.active && p.until > now))
    }

    // ------------------------------------------------------------ sessions

    /// Drop subscriptions whose device session ended (revoked, signed out, expired).
    pub async fn prune_sessions(self: &Arc<Self>, state: &AppState) -> usize {
        let removed = {
            let mut subs = self.subs.write();
            let before = subs.len();
            subs.retain(|s| state.auth.session_active(&s.session_id));
            before - subs.len()
        };
        if removed > 0 {
            tracing::info!("push: dropped {removed} subscription(s) of ended device sessions");
            self.persist().await;
            state.events.emit("push.changed", None, json!({}));
        }
        removed
    }

    // ------------------------------------------------------------ the queue

    /// Resolve an agent note once its window passed: `None` when the session no
    /// longer waits (answered meanwhile) or its permission request was pushed already.
    fn resolve_agent(&self, state: &AppState, note: &PushNote) -> Option<Option<PendingPermission>> {
        let Some(wait) = &note.agent else { return Some(None) };
        let info = state.terminals.info(&wait.terminal_id)?;
        let mut pushed = self.permissions.lock();
        let out = decide_agent(wait, info.agent.as_ref(), pushed.get(&wait.terminal_id).map(String::as_str));
        if let Some(Some(p)) = &out {
            if pushed.len() > 512 {
                pushed.clear();
            }
            pushed.insert(wait.terminal_id.clone(), p.id.clone());
        }
        if out.is_none() {
            tracing::debug!(terminal = wait.terminal_id, "push: agent no longer waits or already pushed");
        }
        out
    }

    fn payload(note: &PushNote, perm: Option<&PendingPermission>, ts: i64) -> Vec<u8> {
        let title = clean(&note.title, 100);
        let mut body = clean(&note.body, 300);
        // Allow only for a request the notification shows whole (the toast's rule).
        let allow = perm.is_some_and(one_tap_allow);
        if let Some(p) = perm {
            body = if allow {
                let tool = p.tool.trim();
                let head = if tool.is_empty() { "Needs your permission".to_string() } else { format!("Needs your permission · {tool}") };
                // `clean` keeps newlines; the glance text is at most GLANCE_CHARS.
                clean(&format!("{head}\n{}", glance_text(p)), GLANCE_CHARS + 100)
            } else {
                let what = match (p.tool.trim(), p.summary.trim()) {
                    ("", "") => String::new(),
                    (t, "") => t.to_string(),
                    ("", s) => s.to_string(),
                    (t, s) => format!("{t}: {s}"),
                };
                clean(&format!("Needs your permission\n{what}"), 300)
            };
        }
        let mut v = json!({
            "v": 1,
            "topic": note.topic,
            "tag": note.tag,
            "title": title,
            "body": body,
            "level": note.level,
            "ts": ts,
            "renotify": true,
        });
        if let Some(pid) = &note.project_id {
            v["projectId"] = json!(pid);
        }
        if let Some(open) = &note.open {
            v["open"] = json!(open);
        }
        if let Some(w) = &note.agent {
            v["terminalId"] = json!(w.terminal_id);
        }
        if let Some(p) = perm {
            v["permissionId"] = json!(p.id);
            v["tool"] = json!(clean(&p.tool, 60));
            v["allow"] = json!(allow);
        }
        let mut out = serde_json::to_vec(&v).unwrap_or_default();
        if out.len() > ece::MAX_PLAINTEXT - 64 {
            v["body"] = json!(clean(&body, 80));
            // The request no longer shows whole.
            if perm.is_some() {
                v["allow"] = json!(false);
            }
            out = serde_json::to_vec(&v).unwrap_or_default();
        }
        out
    }

    /// Deliver `note` to its subscriptions now. `only`: one subscription (tests,
    /// the test button); `force`: ignore presence and topics.
    pub async fn deliver(self: &Arc<Self>, state: &AppState, mut note: PushNote, only: Option<&str>, force: bool) -> Vec<DeliveryResult> {
        let Some(perm) = self.resolve_agent(state, &note) else { return vec![] };
        if let Some(p) = &perm {
            note.ttl_secs = permission_ttl(note.ttl_secs, permission_wait_secs(state), Some(p.since), util::now_ms());
        }
        let payload = Arc::new(Self::payload(&note, perm.as_ref(), util::now_ms()));
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        // Someone waits on the test button: one attempt, and its answer.
        let attempts = if note.topic == Topic::Test { 1 } else { ATTEMPTS };

        self.prune_sessions(state).await;
        let targets: Vec<Subscription> = self.subs.read().iter().filter(|s| only.is_none_or(|id| s.id == id)).cloned().collect();
        let mut out = vec![];
        let mut sends = vec![];
        for sub in targets {
            let skip = if force {
                None
            } else if !sub.topics.wants(note.topic) {
                Some("topic off on this device")
            } else if self.visible(&sub.session_id) {
                Some("Workbench is open on this device")
            } else if sub.quiet_when_active && self.active_elsewhere(&sub.session_id) {
                Some("in use on another device")
            } else {
                None
            };
            if let Some(why) = skip {
                tracing::debug!(device = sub.device, "push: skipped ({why})");
                out.push(DeliveryResult { subscription_id: sub.id.clone(), device: sub.device.clone(), outcome: "skipped", status: None, error: Some(why.into()) });
                continue;
            }
            {
                let mut latest = self.latest.lock();
                if latest.len() > 4096 {
                    latest.clear();
                }
                latest.insert((sub.id.clone(), note.tag.clone()), seq);
            }
            let (this, state, payload, note) = (self.clone(), state.clone(), payload.clone(), note.clone());
            sends.push(async move { this.send_one(&state, sub, payload, &note, seq, attempts).await });
        }
        out.extend(futures::future::join_all(sends).await);
        if out.iter().any(|r| r.outcome != "skipped") {
            self.persist().await;
        }
        if out.iter().any(|r| r.outcome == "gone") {
            state.events.emit("push.changed", None, json!({}));
        }
        out
    }

    fn superseded(&self, sub: &str, tag: &str, seq: u64) -> bool {
        self.latest.lock().get(&(sub.to_string(), tag.to_string())).is_some_and(|s| *s != seq)
    }

    async fn send_one(self: &Arc<Self>, state: &AppState, sub: Subscription, payload: Arc<Vec<u8>>, note: &PushNote, seq: u64, attempts: u32) -> DeliveryResult {
        let result = |outcome, status, error: Option<String>| DeliveryResult {
            subscription_id: sub.id.clone(),
            device: sub.device.clone(),
            outcome,
            status,
            error,
        };
        let url = match self.policy(state).check(&sub.endpoint) {
            Ok(u) => u,
            Err(e) => {
                self.record(&sub.id, Err(e.clone()));
                return result("failed", None, Some(e));
            }
        };
        let host = url.host_str().unwrap_or("").to_string();
        let (Some(p256dh), Some(auth)) = (b64_field(&sub.p256dh), b64_field(&sub.auth)) else {
            self.record(&sub.id, Err("stored keys are unreadable".into()));
            return result("failed", None, Some("stored keys are unreadable".into()));
        };
        let audience = endpoint::origin(&url);
        let subject = self.subject(state, &sub);
        let mut last = (None, String::new());
        for attempt in 0..attempts {
            if attempt > 0 && self.superseded(&sub.id, &note.tag, seq) {
                return result("superseded", None, None);
            }
            let body = match ece::encrypt(&p256dh, &auth, &payload) {
                Ok(b) => b,
                Err(e) => {
                    self.record(&sub.id, Err(e.to_string()));
                    return result("failed", None, Some(e.to_string()));
                }
            };
            let authorization = self.vapid.authorization(&audience, &subject, util::now_ms() / 1000);
            // A send slot per attempt, not across the retry waits: a push service that
            // is down must not hold up deliveries to the others.
            let permit = self.sends.acquire().await;
            let req = self
                .client
                .post(url.clone())
                .header("TTL", note.ttl_secs.to_string())
                .header("Urgency", note.urgency.header())
                .header("Topic", topic_header(&note.tag))
                .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .header(reqwest::header::AUTHORIZATION, authorization)
                .body(body);
            let answer = classify(req.send().await, &host);
            drop(permit);
            match answer {
                Attempt::Sent(status) => {
                    self.record(&sub.id, Ok(()));
                    return result("sent", Some(status), None);
                }
                Attempt::Gone(status) => {
                    tracing::info!(device = sub.device, "push: {host} answered {status}; dropping the subscription");
                    self.subs.write().retain(|s| s.id != sub.id);
                    return result("gone", Some(status), Some("the push service no longer knows this subscription".into()));
                }
                Attempt::Failed(status, e) => {
                    tracing::warn!(device = sub.device, "push to {host} failed: {e}");
                    self.record(&sub.id, Err(e.clone()));
                    return result("failed", status, Some(e));
                }
                Attempt::Retry(status, wait, e) => {
                    last = (status, e);
                    if attempt + 1 < attempts {
                        let wait = wait.unwrap_or(Duration::from_secs(2 * 3u64.pow(attempt))).min(MAX_RETRY_WAIT);
                        tracing::debug!(device = sub.device, "push to {host}: {}; retrying in {wait:?}", last.1);
                        tokio::time::sleep(wait).await;
                    }
                }
            }
        }
        tracing::warn!(device = sub.device, "push to {host} failed after {attempts} attempt(s): {}", last.1);
        self.record(&sub.id, Err(last.1.clone()));
        result("failed", last.0, Some(last.1))
    }

    fn record(&self, id: &str, r: Result<(), String>) {
        let mut subs = self.subs.write();
        let Some(s) = subs.iter_mut().find(|s| s.id == id) else { return };
        match r {
            Ok(()) => {
                s.last_ok_at = Some(util::now_ms());
                s.last_error = None;
                s.failures = 0;
            }
            Err(e) => {
                s.last_error = Some(super::truncate_chars(&e, 200));
                s.last_error_at = Some(util::now_ms());
                s.failures = s.failures.saturating_add(1);
            }
        }
    }
}

/// `[push] subject` must be a mailto: or https: URI (RFC 8292).
pub fn valid_subject(s: &str) -> bool {
    let s = s.trim();
    if let Some(addr) = s.strip_prefix("mailto:") {
        return addr.contains('@') && !addr.chars().any(char::is_whitespace);
    }
    reqwest::Url::parse(s).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some() && u.username().is_empty())
}

fn classify(res: Result<reqwest::Response, reqwest::Error>, host: &str) -> Attempt {
    let resp = match res {
        Ok(r) => r,
        Err(e) => {
            let transient = e.is_timeout() || e.is_connect() || e.is_request();
            // `without_url`: the endpoint is a capability URL, never logged. The root
            // cause (DNS, TLS, refused, timed out) carries no URL and says the most.
            let e = e.without_url();
            let mut root: &dyn std::error::Error = &e;
            while let Some(c) = root.source() {
                root = c;
            }
            let why = if e.is_timeout() { "timed out".to_string() } else { root.to_string() };
            let msg = super::truncate_chars(&format!("could not reach {host}: {why}"), 200);
            return if transient {
                Attempt::Retry(None, None, msg)
            } else {
                Attempt::Failed(None, msg)
            };
        }
    };
    let status = resp.status().as_u16();
    match status {
        200..=299 => Attempt::Sent(status),
        404 | 410 => Attempt::Gone(status),
        429 | 500 | 502 | 503 | 504 => {
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(|s| Duration::from_secs(s.max(1)));
            Attempt::Retry(Some(status), wait, format!("{host} answered HTTP {status}"))
        }
        // Upstream bodies are not echoed: a short reason by status instead.
        400 => Attempt::Failed(Some(status), format!("{host} rejected the message (HTTP 400)")),
        401 | 403 => Attempt::Failed(
            Some(status),
            format!("{host} refused Workbench's VAPID signature (HTTP {status}); turn push off and on again on that device"),
        ),
        413 => Attempt::Failed(Some(status), format!("{host}: message too large (HTTP 413)")),
        _ => Attempt::Failed(Some(status), format!("{host} answered HTTP {status}")),
    }
}

// ---------------------------------------------------------------- startup

pub async fn start(state: &AppState) {
    let dir = state.paths.data_dir.join("push");
    let loaded = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let (vapid, fresh) = vapid::Vapid::load_or_create(&dir)?;
        let file = dir.join("subscriptions.json");
        let subs: Vec<Subscription> = match util::fs::read_json(&file) {
            Ok(Some(s)) if !fresh => s,
            Ok(Some(s)) => {
                let s: Vec<Subscription> = s;
                if !s.is_empty() {
                    tracing::warn!("push: new VAPID key; {} subscription(s) made for the old one are dropped (devices re-subscribe when opened)", s.len());
                }
                let _ = std::fs::remove_file(&file);
                vec![]
            }
            Ok(None) => vec![],
            Err(e) => {
                tracing::warn!("push: {} is unreadable ({e:#}); starting without subscriptions", file.display());
                vec![]
            }
        };
        if file.exists() {
            util::fs::set_mode(&file, 0o600);
        }
        Ok((dir, vapid, subs))
    })
    .await;
    let (dir, vapid, subs) = match loaded {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!("push: disabled: {e:#}");
            let _ = state.platform.push.error.set(format!("{e:#}"));
            return;
        }
        Err(e) => {
            let _ = state.platform.push.error.set(e.to_string());
            return;
        }
    };
    let (tx, rx) = mpsc::channel(QUEUE);
    let engine = match Engine::new(dir, vapid, subs, tx) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            tracing::warn!("push: disabled: {e:#}");
            let _ = state.platform.push.error.set(format!("{e:#}"));
            return;
        }
    };
    if state.platform.push.engine.set(engine.clone()).is_err() {
        return;
    }
    tokio::spawn(worker(state.clone(), engine.clone(), rx));
    // Subscriptions end with their device session.
    let (st, eng) = (state.clone(), engine);
    tokio::spawn(async move {
        let mut ended = st.auth.sessions_ended();
        let mut hourly = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tokio::select! {
                r = ended.changed() => if r.is_err() { break },
                _ = hourly.tick() => {}
            }
            eng.prune_sessions(&st).await;
        }
    });
}

/// Coalesce notes per tag for `COALESCE`, then deliver each in its own task.
async fn worker(state: AppState, engine: Arc<Engine>, mut rx: mpsc::Receiver<PushNote>) {
    let mut pending: HashMap<String, (PushNote, tokio::time::Instant)> = HashMap::new();
    loop {
        let next = pending.values().map(|(_, at)| *at).min();
        tokio::select! {
            note = rx.recv() => {
                let Some(note) = note else { break };
                // A permission sighting already pushed (or about to be, by the note waiting here).
                if let Some(w) = &note.agent {
                    if let Some(hint) = &w.permission_hint {
                        if engine.permissions.lock().get(&w.terminal_id) == Some(hint) {
                            continue;
                        }
                        if pending.get(&note.tag).is_some_and(|(p, _)| p.agent.is_some()) {
                            continue;
                        }
                    }
                }
                let due = tokio::time::Instant::now() + COALESCE;
                pending
                    .entry(note.tag.clone())
                    .and_modify(|e| e.0 = note.clone())
                    .or_insert((note, due));
            }
            _ = async { tokio::time::sleep_until(next.unwrap_or_else(tokio::time::Instant::now)).await }, if next.is_some() => {
                let now = tokio::time::Instant::now();
                let due: Vec<String> = pending.iter().filter(|(_, (_, at))| *at <= now).map(|(k, _)| k.clone()).collect();
                for tag in due {
                    let Some((note, _)) = pending.remove(&tag) else { continue };
                    let (state, engine) = (state.clone(), engine.clone());
                    tokio::spawn(async move {
                        engine.deliver(&state, note, None, false).await;
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

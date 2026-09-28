//! Per project: the documents browsers have open, one server slot per server id, the
//! diagnostics cache and the connected sockets.
//!
//! **Documents** are shared by every socket (browser tab) of the project, but each tab
//! has its own editor buffer, and two tabs can hold different unsaved text for one file.
//! A language server has one text per URI, with a version that only grows: that of the
//! tab **in use** (the last one that edited, saved or asked anything; opening a file
//! does not count, so a tab that restores its layout never replaces another tab's
//! unsaved text). Each socket's own text is kept, so when a tab is used again its texts
//! go back to the servers, and when the tab whose text a server has closes the
//! document, the most recently used other tab's text takes over. Diagnostics go only to
//! the tabs whose text they were computed for (by the publish's `version`, else the
//! current one), and to tabs without the file open. A server sees `didOpen` for a
//! document when it becomes ready (all of its language's open documents at once) or
//! when the document opens while it is ready; both happen under the project lock, so no
//! change is lost or sent twice.
//!
//! **Servers** start lazily: when a document of their language opens in an enabled
//! project. A server with no open document and no request for `idle` is stopped. A
//! crash restarts it with backoff (1 s, 4 s); the third crash within three minutes
//! leaves it `crashed` until the user restarts it.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};

use super::config::{Availability, ServerSpec, extension, language_id};
use super::launch;
use super::server::{Progress, Server};
use super::trust::TrustStore;
use super::uri::{self, AllowSet, ClientUri};
use crate::app::AppState;
use crate::projects::Project;

/// Largest document synchronized with a server.
pub const MAX_DOC_BYTES: usize = 8 * 1024 * 1024;
/// Diagnostics kept per file and per project.
const MAX_DIAGS_PER_FILE: usize = 1000;
const MAX_DIAG_FILES: usize = 20_000;
const CRASH_WINDOW: Duration = Duration::from_secs(180);
const MAX_CRASHES: usize = 3;
/// A failed start (missing command, bad container) is retried on a new document after this.
const FAILED_RETRY: Duration = Duration::from_secs(60);
/// Socket id of documents opened by MCP tools for one request.
pub const TOOL_SOCKET: u64 = 0;

/// Slice state on `AppState::lsp`.
#[derive(Default)]
pub struct LspState {
    projects: Mutex<HashMap<String, Arc<ProjectLsp>>>,
    pub avail: Availability,
    pub trust: TrustStore,
    next_socket: AtomicU64,
}

impl LspState {
    pub fn get(&self, pid: &str) -> Option<Arc<ProjectLsp>> {
        self.projects.lock().get(pid).cloned()
    }

    /// The project's record, created on first use (for the project's current root).
    pub fn project(&self, project: &Project) -> Arc<ProjectLsp> {
        let mut map = self.projects.lock();
        if let Some(p) = map.get(&project.id).filter(|p| p.root == project.root) {
            return p.clone();
        }
        let p = Arc::new(ProjectLsp::new(&project.id, &project.root));
        map.insert(project.id.clone(), p.clone());
        p
    }

    pub fn all(&self) -> Vec<Arc<ProjectLsp>> {
        self.projects.lock().values().cloned().collect()
    }

    pub fn remove(&self, pid: &str) -> Option<Arc<ProjectLsp>> {
        self.projects.lock().remove(pid)
    }

    pub fn next_socket(&self) -> u64 {
        self.next_socket.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// What a server slot is doing (the UI's server state).
#[derive(Debug, Clone, PartialEq)]
pub enum SlotStatus {
    Off,
    Starting,
    Running,
    /// Stopped by the user: stays off until restarted.
    Stopped,
    /// Crashed repeatedly: stays off until restarted.
    Crashed(String),
    /// Could not start (missing command, container gone…).
    Failed(String, Instant),
}

struct Slot {
    status: SlotStatus,
    server: Option<Arc<Server>>,
    /// The latest process, running or not (its log outlives it).
    last: Option<Arc<Server>>,
    /// Bumped on every start and stop, so late tasks of an old start give up.
    generation: u64,
    crashes: VecDeque<Instant>,
    restarts: u32,
    last_error: Option<String>,
    last_emit: Option<Instant>,
}

impl Default for Slot {
    fn default() -> Self {
        Self { status: SlotStatus::Off, server: None, last: None, generation: 0, crashes: VecDeque::new(), restarts: 0, last_error: None, last_emit: None }
    }
}

/// One socket's hold on an open document.
struct DocRef {
    /// Opens not yet closed (MCP tool requests share one socket id).
    count: u32,
    /// The text this socket's editor has (the same `Arc` as `Doc::text` while equal).
    text: Arc<String>,
    /// The latest version whose text is this socket's text, if a server had it.
    synced: Option<i64>,
    /// When the socket last used the document (`Inner::use_seq`).
    used: u64,
}

struct Doc {
    server: Option<String>,
    language_id: String,
    /// Version of `text`: only grows.
    version: i64,
    /// The text the servers have: that of the socket that used the document last.
    text: Arc<String>,
    refs: HashMap<u64, DocRef>,
    /// Pull diagnostics: the latest scheduled request (debounce).
    pull_gen: u64,
}

impl Doc {
    fn new(socket: u64, server: Option<String>, language_id: String, text: String, used: u64) -> Self {
        let text = Arc::new(text);
        let r = DocRef { count: 1, text: text.clone(), synced: Some(1), used };
        Doc { server, language_id, version: 1, text, refs: HashMap::from([(socket, r)]), pull_gen: 0 }
    }

    fn held_by(&self, r: &DocRef) -> bool {
        Arc::ptr_eq(&r.text, &self.text) || *r.text == *self.text
    }

    /// Whether some socket's text is the servers' text.
    fn held(&self) -> bool {
        self.refs.values().any(|r| self.held_by(r))
    }

    /// `text` becomes the servers' text, as a new version; the sockets that have the
    /// same text are in sync with it.
    fn set_text(&mut self, text: Arc<String>) {
        self.version += 1;
        self.text = text;
        for r in self.refs.values_mut() {
            if Arc::ptr_eq(&r.text, &self.text) || *r.text == *self.text {
                r.text = self.text.clone();
                r.synced = Some(self.version);
            }
        }
    }

    /// The socket's text is the servers' current one.
    fn in_sync(&self, socket: u64) -> bool {
        self.refs.get(&socket).is_some_and(|r| r.synced == Some(self.version))
    }
}

/// A server's latest diagnostics for one file.
struct Diags {
    list: Vec<Value>,
    /// The document version they were computed for (`None`: the file was not open).
    version: Option<i64>,
}

struct SocketHandle {
    tx: mpsc::Sender<String>,
    kill: Arc<Notify>,
}

#[derive(Default)]
struct Inner {
    docs: HashMap<String, Doc>,
    /// Bumped whenever a socket uses its documents (`DocRef::used`).
    use_seq: u64,
    slots: BTreeMap<String, Slot>,
    /// Client URI → server id → diagnostics (client URIs inside).
    diagnostics: BTreeMap<String, BTreeMap<String, Diags>>,
    sockets: HashMap<u64, SocketHandle>,
    replies: HashMap<u64, (u64, oneshot::Sender<Value>)>,
    next_reply: u64,
    last_socket: Option<u64>,
    /// Root marker scan (cached a minute).
    markers: Option<(Instant, HashSet<String>)>,
}

pub struct ProjectLsp {
    pub id: String,
    pub root: PathBuf,
    pub root_canon: PathBuf,
    inner: Mutex<Inner>,
    /// `lsp-src` paths servers of this project returned.
    pub allow: Mutex<AllowSet>,
    diag_scheduled: AtomicBool,
}

/// Counts for the status bar and the Problems window.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub hints: usize,
    pub files: usize,
}

/// The result of opening a document.
pub struct Opened {
    pub server: Option<String>,
    pub diagnostics: Vec<(String, Vec<Value>)>,
}

impl ProjectLsp {
    fn new(id: &str, root: &std::path::Path) -> Self {
        Self {
            id: id.to_string(),
            root: root.to_path_buf(),
            root_canon: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            inner: Mutex::new(Inner::default()),
            allow: Mutex::new(AllowSet::default()),
            diag_scheduled: AtomicBool::new(false),
        }
    }

    // ------------------------------------------------------------ sockets

    pub fn add_socket(&self, id: u64, tx: mpsc::Sender<String>, kill: Arc<Notify>) {
        self.inner.lock().sockets.insert(id, SocketHandle { tx, kill });
    }

    /// A socket went away: its documents lose their references, its replies are dropped.
    pub fn remove_socket(&self, state: &AppState, id: u64) {
        let handed_over = {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            inner.sockets.remove(&id);
            inner.replies.retain(|_, (s, _)| *s != id);
            if inner.last_socket == Some(id) {
                inner.last_socket = None;
            }
            let uris: Vec<String> = inner.docs.iter().filter(|(_, d)| d.refs.contains_key(&id)).map(|(u, _)| u.clone()).collect();
            let mut handed_over = vec![];
            for u in uris {
                if let Some(d) = inner.docs.get_mut(&u) {
                    d.refs.remove(&id);
                }
                if self.released(inner, &u) {
                    handed_over.push(u);
                }
            }
            handed_over
        };
        for u in handed_over {
            self.schedule_pull(state, &u);
        }
    }

    /// Send to every socket. A socket that cannot keep up is closed (it reconnects and
    /// resynchronizes).
    pub fn broadcast(&self, msg: &Value) {
        let text = msg.to_string();
        let inner = self.inner.lock();
        for s in inner.sockets.values() {
            if s.tx.try_send(text.clone()).is_err() {
                s.kill.notify_one();
            }
        }
    }

    fn send_to(inner: &Inner, socket: u64, msg: &Value) {
        if let Some(s) = inner.sockets.get(&socket) {
            if s.tx.try_send(msg.to_string()).is_err() {
                s.kill.notify_one();
            }
        }
    }

    pub fn touch_socket(&self, socket: u64) {
        self.inner.lock().last_socket = Some(socket);
    }

    /// Ask a browser (the most recently active one) to handle a server request
    /// (`workspace/applyEdit`, `window/showMessageRequest`). `None`: nobody answered.
    pub async fn ask_client(&self, _state: &AppState, server: &str, method: &str, params: Value, timeout: Duration) -> Option<Value> {
        let (rid, rx) = {
            let mut inner = self.inner.lock();
            let target = inner.last_socket.filter(|s| inner.sockets.contains_key(s)).or_else(|| inner.sockets.keys().copied().max())?;
            inner.next_reply += 1;
            let rid = inner.next_reply;
            let (tx, rx) = oneshot::channel();
            inner.replies.insert(rid, (target, tx));
            Self::send_to(&inner, target, &json!({ "t": "request", "id": rid, "server": server, "method": method, "params": params }));
            (rid, rx)
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => Some(v),
            _ => {
                self.inner.lock().replies.remove(&rid);
                None
            }
        }
    }

    /// A browser's answer to `ask_client`.
    pub fn reply(&self, socket: u64, id: u64, value: Value) {
        let mut inner = self.inner.lock();
        if inner.replies.get(&id).is_some_and(|(s, _)| *s == socket) {
            if let Some((_, tx)) = inner.replies.remove(&id) {
                let _ = tx.send(value);
            }
        }
    }

    // ------------------------------------------------------------ servers

    pub fn server(&self, id: &str) -> Option<Arc<Server>> {
        self.inner.lock().slots.get(id).and_then(|s| s.server.clone())
    }

    /// A server that is initialized and holds the open documents.
    pub fn ready_server(&self, id: &str) -> Option<Arc<Server>> {
        self.server(id).filter(|s| s.ready.load(Ordering::SeqCst))
    }

    pub fn ready_servers(&self) -> Vec<Arc<Server>> {
        self.inner.lock().slots.values().filter_map(|s| s.server.clone()).filter(|s| s.ready.load(Ordering::SeqCst)).collect()
    }

    /// The server a document is synchronized with.
    pub fn doc_server(&self, uri: &str) -> Option<String> {
        self.inner.lock().docs.get(uri).and_then(|d| d.server.clone())
    }

    /// Text of an open document.
    pub fn doc_text(&self, uri: &str) -> Option<Arc<String>> {
        self.inner.lock().docs.get(uri).map(|d| d.text.clone())
    }

    pub fn open_uris(&self) -> HashSet<String> {
        self.inner.lock().docs.keys().cloned().collect()
    }

    /// The first enabled, installed server that handles `path` (by extension, else
    /// the editor's language), where it would run now.
    pub async fn choose(&self, state: &AppState, path: &str, language: Option<&str>) -> Option<Arc<ServerSpec>> {
        let Some(project) = state.projects.get(&self.id) else { return None };
        let rec = state.lsp.trust.get(state, &self.id);
        let (specs, _) = state.config.read().lsp.specs();
        let ext = extension(path);
        let candidates: Vec<ServerSpec> =
            specs.into_iter().filter(|s| s.enabled && !rec.disabled_servers.contains(&s.id) && s.handles(&ext, language)).collect();
        if candidates.is_empty() {
            return None;
        }
        let target = launch::container_for(state, &self.id, rec.mode).await;
        for spec in candidates {
            if launch::place_in(state, &project, &spec, rec.mode, &target).await.is_ok() {
                return Some(Arc::new(spec));
            }
        }
        None
    }

    /// Start the slot's server unless it runs, starts, or was stopped by the user or
    /// crashed. Call with the lock held.
    fn ensure_started(self: &Arc<Self>, state: &AppState, inner: &mut Inner, id: &str) {
        let slot = inner.slots.entry(id.to_string()).or_default();
        match &slot.status {
            SlotStatus::Off => {}
            SlotStatus::Failed(_, at) if at.elapsed() > FAILED_RETRY => {}
            _ => return,
        }
        slot.status = SlotStatus::Starting;
        slot.generation += 1;
        let gen_ = slot.generation;
        tokio::spawn(start_task(state.clone(), self.clone(), id.to_string(), gen_, Duration::ZERO));
        self.emit_state(state, id, &slot_view(slot), true);
    }

    /// Record a started process as the slot's server. `false`: the start is stale
    /// (stopped or restarted meanwhile) and the process must go.
    fn attach(&self, id: &str, gen_: u64, server: &Arc<Server>) -> bool {
        let mut inner = self.inner.lock();
        let Some(slot) = inner.slots.get_mut(id) else { return false };
        if slot.generation != gen_ || slot.status != SlotStatus::Starting {
            return false;
        }
        slot.server = Some(server.clone());
        slot.last = Some(server.clone());
        true
    }

    fn set_failed(&self, state: &AppState, id: &str, gen_: u64, msg: String) {
        let view = {
            let mut inner = self.inner.lock();
            let Some(slot) = inner.slots.get_mut(id) else { return };
            if slot.generation != gen_ {
                return;
            }
            slot.status = SlotStatus::Failed(msg.clone(), Instant::now());
            slot.last_error = Some(msg);
            slot.server = None;
            slot_view(slot)
        };
        self.emit_state(state, id, &view, true);
    }

    /// The server is initialized: hand it every open document of its language.
    fn on_ready(&self, state: &AppState, server: &Arc<Server>, gen_: u64) -> bool {
        let id = server.spec.id.clone();
        let (view, caps) = {
            let mut inner = self.inner.lock();
            let current = inner.slots.get(&id).is_some_and(|s| s.generation == gen_ && s.server.as_ref().is_some_and(|x| Arc::ptr_eq(x, server)));
            if !current {
                return false;
            }
            let allow = self.allow.lock();
            for (u, d) in inner.docs.iter().filter(|(_, d)| d.server.as_deref() == Some(id.as_str())) {
                if let Some(su) = server.to_server_uri(u, &allow) {
                    server.notify("textDocument/didOpen", json!({ "textDocument": { "uri": su, "languageId": d.language_id, "version": d.version, "text": *d.text } }));
                }
            }
            drop(allow);
            server.ready.store(true, Ordering::SeqCst);
            let slot = inner.slots.get_mut(&id).expect("checked above");
            slot.status = SlotStatus::Running;
            slot.last_error = None;
            (slot_view(slot), server.capabilities())
        };
        self.broadcast(&json!({ "t": "caps", "server": id, "capabilities": caps }));
        self.emit_state(state, &id, &view, true);
        let pulls: Vec<String> = {
            let inner = self.inner.lock();
            inner.docs.iter().filter(|(_, d)| d.server.as_deref() == Some(id.as_str())).map(|(u, _)| u.clone()).collect()
        };
        for u in pulls {
            self.schedule_pull(state, &u);
        }
        true
    }

    /// The process exited. A stop that was asked for is not a crash.
    pub fn on_server_exit(self: &Arc<Self>, state: &AppState, server: &Arc<Server>, desc: &str) {
        let id = server.spec.id.clone();
        let view = {
            let mut inner = self.inner.lock();
            let has_docs = inner.docs.values().any(|d| d.server.as_deref() == Some(id.as_str()));
            let Some(slot) = inner.slots.get_mut(&id) else { return };
            if !slot.server.as_ref().is_some_and(|s| Arc::ptr_eq(s, server)) {
                return;
            }
            slot.server = None;
            if server.stopping.load(Ordering::SeqCst) {
                if slot.status == SlotStatus::Running || slot.status == SlotStatus::Starting {
                    slot.status = SlotStatus::Off;
                }
            } else {
                let now = Instant::now();
                slot.crashes.push_back(now);
                while slot.crashes.front().is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW) {
                    slot.crashes.pop_front();
                }
                slot.last_error = Some(format!("{} {desc}", server.spec.label));
                slot.generation += 1;
                if slot.crashes.len() >= MAX_CRASHES {
                    slot.status = SlotStatus::Crashed(desc.to_string());
                    state.events.notify("warning", &format!("{} crashed {} times in {}: restart it from the status bar when fixed", server.spec.label, slot.crashes.len(), self.id));
                } else if has_docs {
                    slot.status = SlotStatus::Starting;
                    slot.restarts += 1;
                    let delay = Duration::from_secs(if slot.crashes.len() <= 1 { 1 } else { 4 });
                    tokio::spawn(start_task(state.clone(), self.clone(), id.clone(), slot.generation, delay));
                } else {
                    slot.status = SlotStatus::Off;
                }
            }
            slot_view(slot)
        };
        self.broadcast(&json!({ "t": "down", "server": id }));
        self.clear_server_diagnostics(state, &id);
        self.emit_state(state, &id, &view, true);
    }

    /// Stop a server. `user`: it stays off until restarted (else it starts again with
    /// its next document).
    pub async fn stop_server(&self, state: &AppState, id: &str, user: bool) {
        let (server, view) = {
            let mut inner = self.inner.lock();
            let slot = inner.slots.entry(id.to_string()).or_default();
            slot.generation += 1;
            slot.status = if user { SlotStatus::Stopped } else { SlotStatus::Off };
            (slot.server.take(), slot_view(slot))
        };
        if let Some(s) = server {
            s.stop().await;
            self.broadcast(&json!({ "t": "down", "server": id }));
        }
        self.clear_server_diagnostics(state, id);
        self.emit_state(state, id, &view, true);
    }

    /// Stop (if running) and start again, forgetting crashes.
    pub async fn restart_server(self: &Arc<Self>, state: &AppState, id: &str) {
        self.stop_server(state, id, false).await;
        let mut inner = self.inner.lock();
        if let Some(slot) = inner.slots.get_mut(id) {
            slot.crashes.clear();
            slot.status = SlotStatus::Off;
        }
        self.ensure_started(state, &mut inner, id);
    }

    /// Stop every server (disable, project removed, Workbench exiting).
    pub async fn shutdown(&self, state: &AppState) {
        let ids: Vec<String> = self.inner.lock().slots.keys().cloned().collect();
        let stops = ids.iter().map(|id| self.stop_server(state, id, false));
        futures::future::join_all(stops).await;
    }

    /// Close every socket and forget the documents: code intelligence was disabled
    /// (`disabled`: the browsers stop), or settings changed (they reconnect and open
    /// their documents again, choosing servers anew).
    pub fn disconnect_all(&self, disabled: bool) {
        let mut inner = self.inner.lock();
        for s in inner.sockets.values() {
            if disabled {
                let _ = s.tx.try_send(json!({ "t": "disabled" }).to_string());
            }
            s.kill.notify_one();
        }
        inner.docs.clear();
        inner.diagnostics.clear();
    }

    /// The log of a server's latest process.
    pub fn log_of(&self, id: &str, tail: usize) -> Option<(Vec<super::server::LogLine>, bool)> {
        let s = self.inner.lock().slots.get(id).and_then(|s| s.last.clone())?;
        let lines = s.log.lock().tail(tail);
        Some((lines, !s.exited()))
    }

    /// Stop servers with no open document and no request for `idle`.
    pub async fn sweep_idle(&self, state: &AppState, idle: Duration) {
        let now = crate::util::now_ms();
        let idle_ids: Vec<String> = {
            let inner = self.inner.lock();
            inner
                .slots
                .iter()
                .filter(|(id, s)| {
                    s.status == SlotStatus::Running
                        && s.server.as_ref().is_some_and(|srv| now - srv.last_activity.load(Ordering::Relaxed) >= idle.as_millis() as i64)
                        && !inner.docs.values().any(|d| d.server.as_deref() == Some(id.as_str()))
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in idle_ids {
            if let Some(s) = self.server(&id) {
                s.log.lock().push("workbench", "idle: no open document and no request, stopping");
            }
            self.stop_server(state, &id, false).await;
        }
    }

    // ------------------------------------------------------------ documents

    /// A socket opened a document. Starts its server (unless `allow_start` is false:
    /// MCP tools only use servers that already run).
    pub async fn open(self: &Arc<Self>, state: &AppState, socket: u64, uri: &str, language: Option<&str>, text: String, allow_start: bool) -> Result<Opened, String> {
        if text.len() > MAX_DOC_BYTES {
            return Err("the file is too large for code intelligence".into());
        }
        let path = match uri::parse_client_uri(uri) {
            Some(ClientUri::Project { pid, rel }) if pid == self.id => rel,
            Some(ClientUri::Source { pid, path }) if pid == self.id && self.allow.lock().get(&path).is_some() => path,
            _ => return Err(format!("{uri} is not a document of project {}", self.id)),
        };
        let spec = self.choose(state, &path, language).await;
        let lang = language_id(&extension(&path), language);
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        inner.last_socket = Some(socket);
        inner.use_seq += 1;
        let used = inner.use_seq;
        if let Some(d) = inner.docs.get_mut(uri) {
            let server = d.server.clone();
            let text = if *d.text == text { d.text.clone() } else { Arc::new(text) };
            let synced = Arc::ptr_eq(&text, &d.text).then_some(d.version);
            let r = d.refs.entry(socket).or_insert_with(|| DocRef { count: 0, text: text.clone(), synced, used });
            r.count += 1;
            r.used = used;
            if !Arc::ptr_eq(&r.text, &text) {
                r.text = text.clone();
                r.synced = synced;
            }
            // Opening does not replace another socket's text (a tab restoring its
            // layout, a reconnect): the servers keep it until this socket is used.
            let adopt = !d.held();
            if adopt {
                d.set_text(text);
                self.send_change(inner, uri);
            }
            let diagnostics = diags_for(inner, uri, socket);
            drop(guard);
            self.schedule_pull(state, uri);
            return Ok(Opened { server, diagnostics });
        }
        let server_id = spec.as_ref().map(|s| s.id.clone());
        inner.docs.insert(uri.to_string(), Doc::new(socket, server_id.clone(), lang.clone(), text, used));
        if let Some(id) = &server_id {
            match inner.slots.get(id).and_then(|s| s.server.clone()).filter(|s| s.ready.load(Ordering::SeqCst)) {
                Some(server) => {
                    let allow = self.allow.lock();
                    if let (Some(su), Some(d)) = (server.to_server_uri(uri, &allow), inner.docs.get(uri)) {
                        server.notify("textDocument/didOpen", json!({ "textDocument": { "uri": su, "languageId": lang, "version": 1, "text": *d.text } }));
                        server.touch();
                    }
                }
                None if allow_start => self.ensure_started(state, inner, id),
                None => {}
            }
        }
        let diagnostics = diags_for(inner, uri, socket);
        drop(guard);
        self.schedule_pull(state, uri);
        Ok(Opened { server: server_id, diagnostics })
    }

    /// A socket's editor changed a document (full text). Its text goes to the servers,
    /// and so do its other documents' texts (`activate`).
    pub fn change(&self, state: &AppState, socket: u64, uri: &str, text: String) -> Result<(), String> {
        if text.len() > MAX_DOC_BYTES {
            return Err("the file is too large for code intelligence".into());
        }
        let changed = {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            inner.last_socket = Some(socket);
            let Some(d) = inner.docs.get_mut(uri) else { return Err(format!("{uri} is not open")) };
            let Some(r) = d.refs.get_mut(&socket) else { return Err(format!("{uri} is not open on this connection")) };
            let (changed, resync) = if *d.text == text {
                // The servers have this text already: the socket is in sync (again).
                r.text = d.text.clone();
                let resync = r.synced != Some(d.version);
                r.synced = Some(d.version);
                (false, resync)
            } else {
                let text = Arc::new(text);
                r.text = text.clone();
                d.set_text(text);
                (true, false)
            };
            if changed {
                self.send_change(inner, uri);
            }
            if resync {
                // What it shows was computed for another text: the current diagnostics.
                for (server, list) in diags_for(inner, uri, socket) {
                    Self::send_to(inner, socket, &json!({ "t": "diagnostics", "uri": uri, "server": server, "diagnostics": list }));
                }
            }
            changed
        };
        if changed {
            self.schedule_pull(state, uri);
        }
        self.activate(state, socket);
        Ok(())
    }

    /// The socket is the one in use: the servers get its text of every document it has
    /// open (where another socket's text was theirs), before its request or edit.
    pub fn activate(&self, state: &AppState, socket: u64) {
        let changed: Vec<String> = {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            inner.use_seq += 1;
            let used = inner.use_seq;
            let mut changed = vec![];
            for (u, d) in inner.docs.iter_mut() {
                let Some(r) = d.refs.get_mut(&socket) else { continue };
                r.used = used;
                if !(Arc::ptr_eq(&r.text, &d.text) || *r.text == *d.text) {
                    let text = r.text.clone();
                    d.set_text(text);
                    changed.push(u.clone());
                }
            }
            for u in &changed {
                self.send_change(inner, u);
            }
            changed
        };
        for u in changed {
            self.schedule_pull(state, &u);
        }
    }

    fn send_change(&self, inner: &Inner, uri: &str) {
        let Some(d) = inner.docs.get(uri) else { return };
        let Some(server) = d.server.as_ref().and_then(|id| inner.slots.get(id)).and_then(|s| s.server.clone()) else { return };
        if !server.ready.load(Ordering::SeqCst) {
            return;
        }
        let caps = server.capabilities();
        let kind = match &caps["textDocumentSync"] {
            Value::Number(n) => n.as_u64().unwrap_or(1),
            Value::Object(o) => o.get("change").and_then(Value::as_u64).unwrap_or(1),
            _ => 1,
        };
        if kind == 0 {
            return;
        }
        let allow = self.allow.lock();
        if let Some(su) = server.to_server_uri(uri, &allow) {
            // Full text is valid for both full and incremental servers.
            server.notify("textDocument/didChange", json!({ "textDocument": { "uri": su, "version": d.version }, "contentChanges": [{ "text": *d.text }] }));
            server.touch();
        }
    }

    pub fn close(&self, state: &AppState, socket: u64, uri: &str) {
        let handed_over = {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            if let Some(d) = inner.docs.get_mut(uri) {
                if let Some(r) = d.refs.get_mut(&socket) {
                    r.count -= 1;
                    if r.count == 0 {
                        d.refs.remove(&socket);
                    }
                }
            }
            self.released(inner, uri)
        };
        if handed_over {
            self.schedule_pull(state, uri);
        }
    }

    /// A socket let go of a document: closed on the servers when nobody has it open;
    /// when the socket had the servers' text, the most recently used other socket's text
    /// takes over (`true`).
    fn released(&self, inner: &mut Inner, uri: &str) -> bool {
        let Some(d) = inner.docs.get_mut(uri) else { return false };
        if !d.refs.is_empty() {
            if d.held() {
                return false;
            }
            let Some(next) = d.refs.values().max_by_key(|r| r.used).map(|r| r.text.clone()) else { return false };
            d.set_text(next);
            self.send_change(inner, uri);
            return true;
        }
        let Some(d) = inner.docs.remove(uri) else { return false };
        // What is cached now describes the file, not an open document's version.
        if let Some(m) = inner.diagnostics.get_mut(uri) {
            m.values_mut().for_each(|c| c.version = None);
        }
        if let Some(server) = d.server.as_ref().and_then(|id| inner.slots.get(id)).and_then(|s| s.server.clone()) {
            if server.ready.load(Ordering::SeqCst) {
                let allow = self.allow.lock();
                if let Some(su) = server.to_server_uri(uri, &allow) {
                    server.notify("textDocument/didClose", json!({ "textDocument": { "uri": su } }));
                }
            }
        }
        false
    }

    /// The socket saved a document: its texts go to the servers first (`activate`).
    pub fn save(&self, state: &AppState, socket: u64, uri: &str) {
        self.activate(state, socket);
        let inner = self.inner.lock();
        let Some(d) = inner.docs.get(uri) else { return };
        let Some(server) = d.server.as_ref().and_then(|id| inner.slots.get(id)).and_then(|s| s.server.clone()) else { return };
        if !server.ready.load(Ordering::SeqCst) {
            return;
        }
        let caps = server.capabilities();
        let save = &caps["textDocumentSync"]["save"];
        if save.is_null() || save == &Value::Bool(false) {
            return;
        }
        let allow = self.allow.lock();
        if let Some(su) = server.to_server_uri(uri, &allow) {
            let mut params = json!({ "textDocument": { "uri": su } });
            if save["includeText"] == true {
                params["text"] = Value::String((*d.text).clone());
            }
            server.notify("textDocument/didSave", params);
        }
    }

    /// Servers that only offer pull diagnostics (`textDocument/diagnostic`) are asked
    /// half a second after the last change.
    fn schedule_pull(&self, state: &AppState, uri: &str) {
        let (server, gen_) = {
            let mut inner = self.inner.lock();
            let Some(sid) = inner.docs.get(uri).and_then(|d| d.server.clone()) else { return };
            let Some(server) = inner.slots.get(&sid).and_then(|s| s.server.clone()).filter(|s| s.ready.load(Ordering::SeqCst)) else { return };
            if server.capabilities().get("diagnosticProvider").is_none_or(Value::is_null) {
                return;
            }
            let Some(d) = inner.docs.get_mut(uri) else { return };
            d.pull_gen += 1;
            (server, d.pull_gen)
        };
        let Some(project) = state.lsp.get(&self.id) else { return };
        let uri = uri.to_string();
        let state = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            // Asked under the lock, so the answer is for this version's text.
            let (su, version, rid, rx) = {
                let inner = project.inner.lock();
                let Some(d) = inner.docs.get(&uri).filter(|d| d.pull_gen == gen_) else { return };
                let allow = project.allow.lock();
                let Some(su) = server.to_server_uri(&uri, &allow) else { return };
                let (rid, rx) = server.start_request("textDocument/diagnostic", json!({ "textDocument": { "uri": su } }));
                (su, d.version, rid, rx)
            };
            let r = match tokio::time::timeout(Duration::from_secs(30), rx).await {
                Ok(Ok(Ok(r))) => r,
                Err(_) => return server.cancel(rid),
                _ => return,
            };
            if r["kind"] == "full" {
                let items = r.get("items").cloned().unwrap_or(json!([]));
                project.publish_diagnostics(&state, &server, json!({ "uri": su, "diagnostics": items, "version": version }));
            }
        });
    }

    // ------------------------------------------------------------ diagnostics

    pub fn publish_diagnostics(&self, state: &AppState, server: &Server, mut params: Value) {
        let Some(server_uri) = params["uri"].as_str().map(str::to_string) else { return };
        let (client_uri, diags) = {
            let mut allow = self.allow.lock();
            let Some(client_uri) = server.translator().to_client(&server_uri, &mut allow) else { return };
            let mut diags = params.get_mut("diagnostics").map(Value::take).unwrap_or(json!([]));
            if let Some(a) = diags.as_array_mut() {
                a.truncate(MAX_DIAGS_PER_FILE);
            }
            server.result_to_client(&mut diags, &mut allow);
            (client_uri, diags)
        };
        let list = match diags {
            Value::Array(a) => a,
            _ => vec![],
        };
        let id = server.spec.id.clone();
        {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            // The version of the open document they were computed for: the one the
            // server names (one we sent), else the current one.
            let version = inner.docs.get(&client_uri).map(|d| params["version"].as_i64().filter(|v| (1..=d.version).contains(v)).unwrap_or(d.version));
            let newer_cached = inner.diagnostics.get(&client_uri).and_then(|m| m.get(&id)).is_some_and(|c| matches!((c.version, version), (Some(a), Some(b)) if a > b));
            if newer_cached {
                // A late answer for an older text.
            } else if list.is_empty() {
                if let Some(m) = inner.diagnostics.get_mut(&client_uri) {
                    m.remove(&id);
                    if m.is_empty() {
                        inner.diagnostics.remove(&client_uri);
                    }
                }
            } else if inner.diagnostics.contains_key(&client_uri) || inner.diagnostics.len() < MAX_DIAG_FILES {
                inner.diagnostics.entry(client_uri.clone()).or_default().insert(id.clone(), Diags { list: list.clone(), version });
            }
            // A socket with the file open gets them only when they were computed for its
            // text: another tab's unsaved edits would put them on the wrong lines.
            let text = json!({ "t": "diagnostics", "uri": client_uri, "server": id, "diagnostics": list, "version": params["version"] }).to_string();
            let doc = inner.docs.get(&client_uri);
            for (sid, s) in &inner.sockets {
                if doc.and_then(|d| d.refs.get(sid)).is_some_and(|r| r.synced != version) {
                    continue;
                }
                if s.tx.try_send(text.clone()).is_err() {
                    s.kill.notify_one();
                }
            }
        }
        self.schedule_counts(state);
    }

    fn clear_server_diagnostics(&self, state: &AppState, id: &str) {
        let cleared: Vec<String> = {
            let mut inner = self.inner.lock();
            let mut cleared = vec![];
            inner.diagnostics.retain(|uri, m| {
                if m.remove(id).is_some() {
                    cleared.push(uri.clone());
                }
                !m.is_empty()
            });
            cleared
        };
        for uri in &cleared {
            self.broadcast(&json!({ "t": "diagnostics", "uri": uri, "server": id, "diagnostics": [] }));
        }
        if !cleared.is_empty() {
            self.schedule_counts(state);
        }
    }

    /// `lsp.diagnostics` with the counts, at most every 400 ms.
    fn schedule_counts(&self, state: &AppState) {
        if self.diag_scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(project) = state.lsp.get(&self.id) else {
            self.diag_scheduled.store(false, Ordering::SeqCst);
            return;
        };
        let state = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            project.diag_scheduled.store(false, Ordering::SeqCst);
            let counts = project.counts();
            state.events.emit("lsp.diagnostics", Some(&project.id), &counts);
        });
    }

    pub fn counts(&self) -> Counts {
        let inner = self.inner.lock();
        let mut c = Counts::default();
        for (uri, m) in &inner.diagnostics {
            if !uri.starts_with("file:///") {
                continue;
            }
            let mut any = false;
            for d in m.values().flat_map(|c| &c.list) {
                any = true;
                match d["severity"].as_u64().unwrap_or(1) {
                    2 => c.warnings += 1,
                    3 => c.infos += 1,
                    4 => c.hints += 1,
                    _ => c.errors += 1,
                }
            }
            if any {
                c.files += 1;
            }
        }
        c
    }

    /// Every diagnostic of the project's files: `(client uri, server, diagnostics)`.
    pub fn diagnostics(&self) -> Vec<(String, String, Vec<Value>)> {
        let inner = self.inner.lock();
        inner
            .diagnostics
            .iter()
            .flat_map(|(uri, m)| m.iter().map(move |(s, c)| (uri.clone(), s.clone(), c.list.clone())))
            .collect()
    }

    // ------------------------------------------------------------ status

    pub fn state_changed(&self, state: &AppState, server: &Server, transition: bool) {
        let view = {
            let mut inner = self.inner.lock();
            let Some(slot) = inner.slots.get_mut(&server.spec.id) else { return };
            if !slot.server.as_ref().is_some_and(|s| std::ptr::eq(Arc::as_ptr(s), server)) {
                return;
            }
            // Progress reports: at most a few a second.
            if !transition && slot.last_emit.is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
                return;
            }
            slot.last_emit = Some(Instant::now());
            slot_view(slot)
        };
        self.emit_state(state, &server.spec.id, &view, transition);
    }

    fn emit_state(&self, state: &AppState, id: &str, view: &SlotView, transition: bool) {
        state.events.emit(
            "lsp.state",
            Some(&self.id),
            json!({ "server": id, "state": view.state, "progress": view.progress, "transition": transition }),
        );
    }

    /// Root markers present in the project (for "relevant" servers), cached a minute.
    pub async fn markers(&self, specs: &[ServerSpec]) -> HashSet<String> {
        if let Some((at, m)) = &self.inner.lock().markers {
            if at.elapsed() < Duration::from_secs(60) {
                return m.clone();
            }
        }
        let root = self.root.clone();
        let list: Vec<(String, Vec<String>)> = specs.iter().map(|s| (s.id.clone(), s.root_markers.clone())).collect();
        let found = tokio::task::spawn_blocking(move || {
            list.into_iter().filter(|(_, m)| launch::has_root_marker(&root, m)).map(|(id, _)| id).collect::<HashSet<String>>()
        })
        .await
        .unwrap_or_default();
        self.inner.lock().markers = Some((Instant::now(), found.clone()));
        found
    }

    /// Slot states and open document counts per server id.
    pub fn slot_views(&self) -> HashMap<String, (SlotView, usize)> {
        let inner = self.inner.lock();
        inner
            .slots
            .iter()
            .map(|(id, s)| {
                let docs = inner.docs.values().filter(|d| d.server.as_deref() == Some(id.as_str())).count();
                (id.clone(), (slot_view(s), docs))
            })
            .collect()
    }
}

/// What a socket that opens (or catches up with) a document is sent, per server: the
/// cached diagnostics when they were computed for its text, else an empty list (which
/// clears what it kept from before it had the file open).
fn diags_for(inner: &Inner, uri: &str, socket: u64) -> Vec<(String, Vec<Value>)> {
    let Some(m) = inner.diagnostics.get(uri) else { return vec![] };
    let synced = inner.docs.get(uri).filter(|d| d.in_sync(socket)).map(|d| d.version);
    m.iter()
        .map(|(s, c)| {
            let fits = synced.is_some() && (c.version.is_none() || c.version == synced);
            (s.clone(), if fits { c.list.clone() } else { vec![] })
        })
        .collect()
}

/// A slot as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    /// `off`, `starting`, `indexing`, `ready`, `stopped`, `crashed`, `failed`.
    pub state: &'static str,
    pub progress: Option<Progress>,
    pub error: Option<String>,
    pub restarts: u32,
    pub side: Option<&'static str>,
    pub os_pid: Option<u32>,
    pub started_at: Option<i64>,
    pub command: Option<String>,
    pub server_info: Option<Value>,
}

fn slot_view(s: &Slot) -> SlotView {
    let (state, progress) = match &s.status {
        SlotStatus::Off => ("off", None),
        SlotStatus::Starting => ("starting", None),
        SlotStatus::Running => {
            let (p, busy) = s.server.as_ref().map(|x| x.progress()).unwrap_or((None, false));
            (if busy { "indexing" } else { "ready" }, p)
        }
        SlotStatus::Stopped => ("stopped", None),
        SlotStatus::Crashed(_) => ("crashed", None),
        SlotStatus::Failed(..) => ("failed", None),
    };
    let error = match &s.status {
        SlotStatus::Crashed(e) => Some(s.last_error.clone().unwrap_or_else(|| e.clone())),
        SlotStatus::Failed(e, _) => Some(e.clone()),
        _ => s.last_error.clone(),
    };
    let info = s.server.as_ref().map(|x| x.server_info()).filter(|v| !v.is_null());
    SlotView {
        state,
        progress,
        error,
        restarts: s.restarts,
        side: s.server.as_ref().map(|x| x.side),
        os_pid: s.server.as_ref().and_then(|x| x.os_pid()),
        started_at: s.server.as_ref().map(|x| x.started_at),
        command: s.server.as_ref().map(|x| x.display.clone()),
        server_info: info,
    }
}

/// Spawn + initialize a slot's server (after `delay`, for crash restarts).
async fn start_task(state: AppState, project: Arc<ProjectLsp>, id: String, gen_: u64, delay: Duration) {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let current = || project.inner.lock().slots.get(&id).is_some_and(|s| s.generation == gen_ && s.status == SlotStatus::Starting);
    if !current() {
        return;
    }
    let Some(p) = state.projects.get(&project.id) else { return };
    if !state.lsp.trust.enabled(&state, &p) {
        project.set_failed(&state, &id, gen_, "code intelligence is not enabled for this project".into());
        return;
    }
    let (specs, _) = state.config.read().lsp.specs();
    let Some(spec) = specs.into_iter().find(|s| s.id == id) else {
        project.set_failed(&state, &id, gen_, format!("no server {id} in the configuration"));
        return;
    };
    let mode = state.lsp.trust.get(&state, &project.id).mode;
    let placement = match launch::place(&state, &p, &spec, mode).await {
        Ok(pl) => pl,
        Err(e) => return project.set_failed(&state, &id, gen_, e),
    };
    let mut spec = spec;
    launch::preset_defaults(&mut spec, &placement);
    let launch = match launch::launch(&p, &spec, placement) {
        Ok(l) => l,
        Err(e) => return project.set_failed(&state, &id, gen_, e),
    };
    let server = match Server::spawn(&state, &project, Arc::new(spec), launch) {
        Ok(s) => s,
        Err(e) => return project.set_failed(&state, &id, gen_, e),
    };
    if !project.attach(&id, gen_, &server) {
        server.stop().await;
        return;
    }
    let view = project.inner.lock().slots.get(&id).map(slot_view);
    if let Some(v) = view {
        project.emit_state(&state, &id, &v, true);
    }
    if let Err(e) = server.initialize(Duration::from_secs(120)).await {
        server.log.lock().push("workbench", &e);
        project.set_failed(&state, &id, gen_, e);
        server.stop().await;
        return;
    }
    if !project.on_ready(&state, &server, gen_) {
        server.stop().await;
    }
}

/// Is `trust` enabled for this project's current directory?
pub fn enabled(state: &AppState, project: &Project) -> bool {
    state.lsp.trust.enabled(state, project)
}

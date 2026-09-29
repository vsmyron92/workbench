//! One running language server process: spawned on the host or in the project's dev
//! container, spoken to over stdio (`jsonrpc`), with its stderr in a bounded log ring.
//!
//! The process leads its own process group, so stopping it (shutdown → exit → SIGTERM →
//! SIGKILL to the group) also ends what it started (cargo check, tsserver, go list).
//! In a container, the process inside is signalled too (`devcontainer::kill_inside`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use super::config::ServerSpec;
use super::jsonrpc::{self, Kind, RpcError};
use super::manager::ProjectLsp;
use super::uri::{self, AllowSet, Origin, PathMap, Translator};
use crate::app::AppState;

/// Lines kept per server (stderr, log messages, lifecycle notes).
const LOG_LINES: usize = 3000;
const LOG_LINE_MAX: usize = 4000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub seq: u64,
    pub ts: i64,
    /// `stderr`, `log` (window/logMessage), `message` (window/showMessage) or `workbench`.
    pub stream: &'static str,
    pub text: String,
}

#[derive(Default)]
pub struct LogRing {
    lines: VecDeque<LogLine>,
    seq: u64,
}

impl LogRing {
    pub fn push(&mut self, stream: &'static str, text: &str) {
        let full = text.trim_end();
        let mut text: String = full.chars().take(LOG_LINE_MAX).collect();
        if text.len() < full.len() {
            text.push('…');
        }
        self.seq += 1;
        if self.lines.len() >= LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine { seq: self.seq, ts: crate::util::now_ms(), stream, text });
    }

    pub fn tail(&self, n: usize) -> Vec<LogLine> {
        self.lines.iter().skip(self.lines.len().saturating_sub(n)).cloned().collect()
    }
}

/// Work-done progress the server reports (`$/progress`).
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<u32>,
}

/// `workspace/didChangeWatchedFiles` registrations: glob patterns (relative to the
/// project root, or to a base URI), and which kinds of change they want.
#[derive(Debug, Clone)]
pub struct Watcher {
    pub registration: String,
    pub glob: globset::GlobMatcher,
    /// Host directory the glob is relative to.
    pub base: PathBuf,
    /// Bit set: 1 create, 2 change, 4 delete.
    pub kind: u8,
}

#[derive(Default)]
struct Info {
    capabilities: Value,
    server_info: Value,
    progress: HashMap<String, Progress>,
    /// rust-analyzer's `experimental/serverStatus`: not quiescent while it loads.
    busy: Option<String>,
    os_pid: Option<u32>,
}

/// Where and how the process runs.
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// The process's own environment additions (`None` removes).
    pub env: Vec<(String, Option<String>)>,
    pub cwd: PathBuf,
    pub map: PathMap,
    pub origin: Origin,
    /// `host` or `container`.
    pub side: &'static str,
    /// Human description of the command (shown in the UI and the log).
    pub display: String,
    /// In a container: what `kill_inside` needs.
    pub container: Option<(String, String, String)>,
    /// The server's workspace folder (server-side path).
    pub root_server: String,
}

pub struct Server {
    pub spec: Arc<ServerSpec>,
    pub pid: String,
    pub root: PathBuf,
    pub root_canon: PathBuf,
    pub map: PathMap,
    pub origin: Origin,
    pub side: &'static str,
    pub display: String,
    pub started_at: i64,
    root_server: String,
    out: mpsc::UnboundedSender<Value>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>,
    next_id: AtomicI64,
    /// Initialized and holding every open document of its language.
    pub ready: AtomicBool,
    /// A stop was asked for: its exit is not a crash.
    pub stopping: AtomicBool,
    info: Mutex<Info>,
    pub log: Mutex<LogRing>,
    watchers: Mutex<Vec<Watcher>>,
    /// Last document change or request (idle shutdown).
    pub last_activity: AtomicI64,
    exited: watch::Receiver<Option<String>>,
    group: crate::util::os::proc::ProcGroup,
    container: Option<(String, String, String)>,
    project: Weak<ProjectLsp>,
}

impl Server {
    /// Start the process (not yet initialized: see `initialize`).
    pub fn spawn(state: &AppState, project: &Arc<ProjectLsp>, spec: Arc<ServerSpec>, launch: Launch) -> Result<Arc<Server>, String> {
        let mut cmd = tokio::process::Command::new(&launch.program);
        cmd.args(&launch.args)
            .current_dir(&launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::util::os::proc::ProcGroup::prepare(&mut cmd);
        crate::util::proc::clean_env(&mut cmd);
        for (k, v) in &launch.env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", launch.display))?;
        let group = crate::util::os::proc::ProcGroup::attach(&child);
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            return Err("the process has no stdio".into());
        };
        let os_pid = child.id();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();
        let (exit_tx, exit_rx) = watch::channel(None);
        let server = Arc::new(Server {
            spec,
            pid: project.id.clone(),
            root: project.root.clone(),
            root_canon: project.root_canon.clone(),
            map: launch.map,
            origin: launch.origin,
            side: launch.side,
            display: launch.display.clone(),
            started_at: crate::util::now_ms(),
            root_server: launch.root_server,
            out: out_tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            ready: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            info: Mutex::new(Info { os_pid, ..Default::default() }),
            log: Mutex::new(LogRing::default()),
            watchers: Mutex::new(vec![]),
            last_activity: AtomicI64::new(crate::util::now_ms()),
            exited: exit_rx,
            group,
            container: launch.container,
            project: Arc::downgrade(project),
        });
        server.log.lock().push("workbench", &format!("started {} ({}, pid {})", launch.display, launch.side, os_pid.unwrap_or(0)));

        tokio::spawn(write_loop(stdin, out_rx));
        tokio::spawn(read_loop(state.clone(), Arc::downgrade(&server), stdout));
        tokio::spawn(stderr_loop(Arc::downgrade(&server), stderr));
        let weak = Arc::downgrade(&server);
        let st = state.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            let desc = match status {
                Ok(s) => match (s.code(), crate::util::os::proc::exit_signal(&s)) {
                    (Some(c), _) => format!("exited with code {c}"),
                    (None, Some(sig)) => format!("killed by signal {sig}"),
                    _ => "exited".into(),
                },
                Err(e) => format!("wait failed: {e}"),
            };
            let _ = exit_tx.send(Some(desc.clone()));
            if let Some(s) = weak.upgrade() {
                s.log.lock().push("workbench", &desc);
                s.fail_pending(RpcError::new(jsonrpc::SERVER_GONE, format!("{} {desc}", s.spec.label)));
                s.ready.store(false, Ordering::SeqCst);
                if let Some(p) = s.project.upgrade() {
                    p.on_server_exit(&st, &s, &desc);
                }
            }
        });
        Ok(server)
    }

    /// `initialize` → `initialized` (→ `workspace/didChangeConfiguration`).
    pub async fn initialize(&self, init_timeout: Duration) -> Result<(), String> {
        let root_uri = uri::file_uri(&self.root_server);
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.pid.clone());
        let params = json!({
            // A process in the container cannot see ours: it would exit at once.
            "processId": if self.side == "host" { json!(std::process::id()) } else { Value::Null },
            "clientInfo": { "name": "Workbench", "version": env!("CARGO_PKG_VERSION") },
            "locale": "en",
            "rootPath": self.root_server,
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": name }],
            "capabilities": client_capabilities(),
            "initializationOptions": self.spec.initialization_options.clone().unwrap_or(Value::Null),
            "trace": "off",
        });
        let result = tokio::select! {
            r = self.request("initialize", params, init_timeout) => r.map_err(|e| format!("initialize failed: {e}"))?,
            _ = self.wait_exit() => return Err(format!("{} exited during initialize", self.spec.label)),
        };
        {
            let mut info = self.info.lock();
            info.capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
            info.server_info = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        }
        if let Some(si) = result.get("serverInfo") {
            let v = format!("{} {}", si["name"].as_str().unwrap_or(""), si["version"].as_str().unwrap_or(""));
            self.log.lock().push("workbench", &format!("initialized: {}", v.trim()));
        }
        self.notify("initialized", json!({}));
        if let Some(settings) = &self.spec.settings {
            self.notify("workspace/didChangeConfiguration", json!({ "settings": settings }));
        }
        Ok(())
    }

    pub fn capabilities(&self) -> Value {
        self.info.lock().capabilities.clone()
    }

    pub fn server_info(&self) -> Value {
        self.info.lock().server_info.clone()
    }

    pub fn os_pid(&self) -> Option<u32> {
        self.info.lock().os_pid
    }

    /// Current progress (the one with a percentage first) and whether it is busy.
    pub fn progress(&self) -> (Option<Progress>, bool) {
        let info = self.info.lock();
        let mut all: Vec<&Progress> = info.progress.values().collect();
        all.sort_by_key(|p| p.percentage.is_none());
        let p = all.first().map(|p| (*p).clone()).or_else(|| info.busy.as_ref().map(|m| Progress { title: m.clone(), ..Default::default() }));
        let busy = !info.progress.is_empty() || info.busy.is_some();
        (p, busy)
    }

    pub fn exited(&self) -> bool {
        self.exited.borrow().is_some()
    }

    async fn wait_exit(&self) {
        let mut rx = self.exited.clone();
        let _ = rx.wait_for(|v| v.is_some()).await;
    }

    pub fn touch(&self) {
        self.last_activity.store(crate::util::now_ms(), Ordering::Relaxed);
    }

    // ------------------------------------------------------------ messages

    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.out.send(jsonrpc::notification(method, params));
    }

    fn respond(&self, id: &Value, result: Value) {
        let _ = self.out.send(jsonrpc::response(id, result));
    }

    fn respond_error(&self, id: &Value, code: i64, message: &str) {
        let _ = self.out.send(jsonrpc::error_response(id, code, message));
    }

    /// Send a request; the receiver gets the result. `cancel(id)` gives up on it.
    pub fn start_request(&self, method: &str, params: Value) -> (i64, oneshot::Receiver<Result<Value, RpcError>>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut p = self.pending.lock();
            // Waiters that gave up (a closed socket) leave entries until the answer comes.
            if p.len() > 512 {
                p.retain(|_, tx| !tx.is_closed());
            }
            p.insert(id, tx);
        }
        if self.exited() || self.out.send(jsonrpc::request(id, method, params)).is_err() {
            if let Some(tx) = self.pending.lock().remove(&id) {
                let _ = tx.send(Err(RpcError::new(jsonrpc::SERVER_GONE, format!("{} is not running", self.spec.label))));
            }
        }
        (id, rx)
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        let (id, rx) = self.start_request(method, params);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(RpcError::new(jsonrpc::SERVER_GONE, format!("{} went away", self.spec.label))),
            Err(_) => {
                self.cancel(id);
                Err(RpcError::new(jsonrpc::TIMED_OUT, format!("{} did not answer {method} within {}s", self.spec.label, timeout.as_secs())))
            }
        }
    }

    /// `$/cancelRequest`, and forget the waiter.
    pub fn cancel(&self, id: i64) {
        if self.pending.lock().remove(&id).is_some() {
            self.notify("$/cancelRequest", json!({ "id": id }));
        }
    }

    fn fail_pending(&self, err: RpcError) {
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(err.clone()));
        }
    }

    // ------------------------------------------------------------ URIs

    pub fn translator(&self) -> Translator<'_> {
        Translator { pid: &self.pid, root: &self.root, root_canon: &self.root_canon, map: &self.map, origin: &self.origin }
    }

    /// Browser URI → this server's URI.
    pub fn to_server_uri(&self, client: &str, allow: &AllowSet) -> Option<String> {
        self.translator().to_server(client, &|p| allow.get(p).is_some())
    }

    /// Rewrite the browser URIs in `v`'s URI fields for this server (unmappable ones stay).
    pub fn params_to_server(&self, v: &mut Value, allow: &AllowSet) {
        let tr = self.translator();
        uri::rewrite(v, &["file://", "lsp-src://"], &mut |s| tr.to_server(s, &|p| allow.get(p).is_some()));
    }

    /// Rewrite this server's URIs in `v`'s URI fields for the browser; files outside the
    /// project they name become readable through the source route (`uri::rewrite`).
    pub fn result_to_client(&self, v: &mut Value, allow: &mut AllowSet) {
        let tr = self.translator();
        uri::rewrite(v, &["file://"], &mut |s| tr.to_client(s, allow));
    }

    // ------------------------------------------------------------ watched files

    pub fn watchers(&self) -> Vec<Watcher> {
        self.watchers.lock().clone()
    }

    fn register(&self, params: &Value) {
        for reg in params["registrations"].as_array().into_iter().flatten() {
            if reg["method"] != "workspace/didChangeWatchedFiles" {
                continue;
            }
            let id = reg["id"].as_str().unwrap_or("").to_string();
            let mut added = vec![];
            for w in reg["registerOptions"]["watchers"].as_array().into_iter().flatten() {
                let kind = w["kind"].as_u64().unwrap_or(7) as u8;
                let (pattern, base) = match &w["globPattern"] {
                    Value::String(s) => (s.clone(), self.root.clone()),
                    Value::Object(o) => {
                        let pattern = o.get("pattern").and_then(Value::as_str).unwrap_or("").to_string();
                        // RelativePattern: baseUri is a URI or a WorkspaceFolder.
                        let base_uri = o.get("baseUri").and_then(|b| b.as_str().map(str::to_string).or_else(|| b["uri"].as_str().map(str::to_string)));
                        let base = base_uri
                            .and_then(|u| uri::parse_file_uri(&u))
                            .and_then(|p| self.map.to_host(&p))
                            .unwrap_or_else(|| self.root.clone());
                        (pattern, base)
                    }
                    _ => continue,
                };
                // Absolute patterns are relative to the file system's root (`/`; on Windows
                // the drive's, as in rust-analyzer's `C:\p/**/*.rs`).
                let (pattern, base) = if crate::util::os::path::is_absolute_str(&pattern) {
                    match self.map.to_host(&pattern) {
                        Some(h) => {
                            let (root, rest) = crate::util::os::path::root_and_rest(&h);
                            (rest, root)
                        }
                        None => continue,
                    }
                } else {
                    (pattern, base)
                };
                let glob = globset::GlobBuilder::new(&pattern).literal_separator(true).build();
                if let Ok(g) = glob {
                    added.push(Watcher { registration: id.clone(), glob: g.compile_matcher(), base, kind });
                }
            }
            let mut w = self.watchers.lock();
            if w.len() + added.len() <= 2000 {
                w.extend(added);
            }
        }
    }

    fn unregister(&self, params: &Value) {
        // The spec's misspelling is the field name.
        let list = params.get("unregisterations").or_else(|| params.get("unregistrations"));
        for u in list.and_then(Value::as_array).into_iter().flatten() {
            if let Some(id) = u["id"].as_str() {
                self.watchers.lock().retain(|w| w.registration != id);
            }
        }
    }

    // ------------------------------------------------------------ stop

    /// Ask the server to leave (shutdown → exit), then end its process group.
    pub async fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.ready.store(false, Ordering::SeqCst);
        if !self.exited() {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.request("shutdown", Value::Null, Duration::from_secs(3))).await;
            self.notify("exit", Value::Null);
            if tokio::time::timeout(Duration::from_secs(2), self.wait_exit()).await.is_err() {
                self.terminate();
                if tokio::time::timeout(Duration::from_secs(2), self.wait_exit()).await.is_err() {
                    self.kill();
                    let _ = tokio::time::timeout(Duration::from_secs(2), self.wait_exit()).await;
                }
            }
        }
        // Children of the server in its group outlive a clean exit sometimes.
        self.group.kill();
        if let Some((docker, container, term)) = &self.container {
            crate::devcontainer::kill_inside(docker, container, term).await;
        }
    }

    /// SIGTERM to the process (until it is reaped: its pid could be reused then) and to its group.
    fn terminate(&self) {
        if !self.exited() {
            self.group.terminate_leader();
        }
        self.group.terminate();
    }

    /// `terminate` with SIGKILL.
    fn kill(&self) {
        if !self.exited() {
            self.group.kill_leader();
        }
        self.group.kill();
    }
}

// ---------------------------------------------------------------- I/O loops

async fn write_loop(mut stdin: tokio::process::ChildStdin, mut rx: mpsc::UnboundedReceiver<Value>) {
    while let Some(msg) = rx.recv().await {
        if jsonrpc::write_message(&mut stdin, &msg).await.is_err() {
            break;
        }
    }
}

async fn stderr_loop(server: Weak<Server>, stderr: tokio::process::ChildStderr) {
    let mut r = BufReader::with_capacity(16 * 1024, stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_line_truncated(&mut r, &mut line, 8 * 1024).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let Some(s) = server.upgrade() else { break };
                let text = crate::util::ansi::strip(&String::from_utf8_lossy(&line));
                s.log.lock().push("stderr", &text);
            }
        }
    }
}

/// One line, keeping at most `max` bytes of it (the rest is skipped).
async fn read_line_truncated<R: AsyncRead + Unpin>(r: &mut BufReader<R>, out: &mut Vec<u8>, max: usize) -> std::io::Result<usize> {
    let mut total = 0;
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok(total);
        }
        let (take, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        let room = max.saturating_sub(out.len());
        out.extend_from_slice(&buf[..take.min(room)]);
        r.consume(take);
        total += take;
        if done {
            return Ok(total);
        }
    }
}

async fn read_loop(state: AppState, server: Weak<Server>, stdout: tokio::process::ChildStdout) {
    let mut r = BufReader::with_capacity(256 * 1024, stdout);
    loop {
        let msg = match jsonrpc::read_message(&mut r).await {
            Ok(m) => m,
            Err(jsonrpc::FrameError::Eof) => break,
            Err(e) => {
                if let Some(s) = server.upgrade() {
                    s.log.lock().push("workbench", &format!("protocol error, stopping: {e}"));
                    s.terminate();
                }
                break;
            }
        };
        let Some(s) = server.upgrade() else { break };
        dispatch(&state, &s, msg);
    }
}

fn dispatch(state: &AppState, s: &Arc<Server>, mut msg: Value) {
    match jsonrpc::classify(&msg) {
        Kind::Response { id } => {
            if let Some(tx) = s.pending.lock().remove(&id) {
                let _ = tx.send(jsonrpc::result_of(msg));
            }
        }
        Kind::Request { .. } => {
            let id = msg["id"].clone();
            let method = msg["method"].as_str().unwrap_or("").to_string();
            let params = msg.get_mut("params").map(Value::take).unwrap_or(Value::Null);
            server_request(state, s, id, &method, params);
        }
        Kind::Notification { method } => {
            let method = method.to_string();
            let params = msg.get_mut("params").map(Value::take).unwrap_or(Value::Null);
            server_notification(state, s, &method, params);
        }
        Kind::Invalid => {}
    }
}

fn server_notification(state: &AppState, s: &Arc<Server>, method: &str, params: Value) {
    let project = s.project.upgrade();
    match method {
        "textDocument/publishDiagnostics" => {
            if let Some(p) = project {
                p.publish_diagnostics(state, s, params);
            }
        }
        "$/progress" => {
            let token = match &params["token"] {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            let v = &params["value"];
            let changed = {
                let mut info = s.info.lock();
                match v["kind"].as_str() {
                    Some("begin") => {
                        info.progress.insert(
                            token,
                            Progress {
                                title: v["title"].as_str().unwrap_or("").chars().take(200).collect(),
                                message: v["message"].as_str().map(|m| m.chars().take(200).collect()),
                                percentage: v["percentage"].as_u64().map(|p| p.min(100) as u32),
                            },
                        );
                        true
                    }
                    Some("report") => {
                        if let Some(p) = info.progress.get_mut(&token) {
                            if let Some(m) = v["message"].as_str() {
                                p.message = Some(m.chars().take(200).collect());
                            }
                            if let Some(pc) = v["percentage"].as_u64() {
                                p.percentage = Some(pc.min(100) as u32);
                            }
                        }
                        false
                    }
                    Some("end") => info.progress.remove(&token).is_some(),
                    _ => false,
                }
            };
            if let Some(p) = project {
                p.state_changed(state, s, changed);
            }
        }
        "experimental/serverStatus" => {
            let quiescent = params["quiescent"].as_bool().unwrap_or(true);
            let msg = params["message"].as_str().map(|m| m.chars().take(300).collect::<String>());
            {
                let mut info = s.info.lock();
                info.busy = (!quiescent).then(|| msg.clone().unwrap_or_else(|| "Loading".into()));
            }
            if params["health"] != "ok" {
                if let Some(m) = &msg {
                    s.log.lock().push("message", m);
                }
            }
            if let Some(p) = project {
                p.state_changed(state, s, true);
            }
        }
        "window/logMessage" => {
            if let Some(m) = params["message"].as_str() {
                let mut log = s.log.lock();
                for line in m.lines().take(200) {
                    log.push("log", line);
                }
            }
        }
        "window/showMessage" => {
            let text = params["message"].as_str().unwrap_or("").chars().take(2000).collect::<String>();
            s.log.lock().push("message", &text);
            let level = params["type"].as_u64().unwrap_or(4);
            if let Some(p) = project {
                p.broadcast(&json!({ "t": "message", "server": s.spec.id, "level": level, "message": text }));
            }
        }
        _ => {}
    }
}

fn server_request(state: &AppState, s: &Arc<Server>, id: Value, method: &str, params: Value) {
    match method {
        "workspace/configuration" => {
            let settings = s.spec.settings.clone().unwrap_or(Value::Null);
            let items: Vec<Value> = params["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| config_section(&settings, item["section"].as_str()))
                .collect();
            s.respond(&id, Value::Array(items));
        }
        "client/registerCapability" => {
            s.register(&params);
            s.respond(&id, Value::Null);
        }
        "client/unregisterCapability" => {
            s.unregister(&params);
            s.respond(&id, Value::Null);
        }
        "window/workDoneProgress/create" => s.respond(&id, Value::Null),
        "workspace/workspaceFolders" => {
            let root_uri = uri::file_uri(&s.root_server);
            let name = s.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            s.respond(&id, json!([{ "uri": root_uri, "name": name }]));
        }
        "window/showDocument" => s.respond(&id, json!({ "success": false })),
        "workspace/semanticTokens/refresh" | "workspace/inlayHint/refresh" | "workspace/codeLens/refresh" | "workspace/diagnostic/refresh" | "workspace/foldingRange/refresh" => {
            s.respond(&id, Value::Null);
            if let Some(p) = s.project.upgrade() {
                let what = method.trim_start_matches("workspace/").trim_end_matches("/refresh");
                p.broadcast(&json!({ "t": "refresh", "server": s.spec.id, "what": what }));
            }
        }
        "window/showMessageRequest" | "workspace/applyEdit" => {
            let s = s.clone();
            let state = state.clone();
            let method = method.to_string();
            tokio::spawn(async move {
                let Some(project) = s.project.upgrade() else { return };
                let mut params = params;
                if method == "workspace/applyEdit" {
                    let mut allow = project.allow.lock();
                    s.result_to_client(&mut params, &mut allow);
                }
                let timeout = if method == "workspace/applyEdit" { Duration::from_secs(30) } else { Duration::from_secs(300) };
                let answer = project.ask_client(&state, &s.spec.id, &method, params, timeout).await;
                let result = match (method.as_str(), answer) {
                    ("workspace/applyEdit", Some(a)) => a,
                    ("workspace/applyEdit", None) => json!({ "applied": false, "failureReason": "no Workbench editor is connected to apply it" }),
                    (_, Some(a)) => a,
                    (_, None) => Value::Null,
                };
                s.respond(&id, result);
            });
        }
        _ => s.respond_error(&id, jsonrpc::METHOD_NOT_FOUND, &format!("Workbench does not handle {method}")),
    }
}

/// The value of a `workspace/configuration` section (`rust-analyzer`, `python.analysis`).
pub fn config_section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.clone();
    };
    if let Some(v) = settings.get(section) {
        return v.clone();
    }
    let mut cur = settings;
    for part in section.split('.') {
        match cur.get(part) {
            Some(v) => cur = v,
            None => return Value::Null,
        }
    }
    cur.clone()
}

/// What Workbench's editor supports, as LSP client capabilities.
pub fn client_capabilities() -> Value {
    let symbol_kinds: Vec<u32> = (1..=26).collect();
    let completion_kinds: Vec<u32> = (1..=25).collect();
    json!({
        "general": {
            "positionEncodings": ["utf-16"],
            "markdown": { "parser": "marked", "version": "1.1.0" },
            "staleRequestSupport": { "cancel": true, "retryOnContentModified": [] },
        },
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true, "resourceOperations": [], "failureHandling": "abort", "normalizesLineEndings": true },
            "didChangeConfiguration": { "dynamicRegistration": false },
            "didChangeWatchedFiles": { "dynamicRegistration": true, "relativePatternSupport": true },
            "symbol": { "dynamicRegistration": false, "symbolKind": { "valueSet": symbol_kinds }, "tagSupport": { "valueSet": [1] } },
            "executeCommand": { "dynamicRegistration": false },
            "workspaceFolders": true,
            "configuration": true,
            "semanticTokens": { "refreshSupport": true },
            "inlayHint": { "refreshSupport": true },
            "diagnostics": { "refreshSupport": true },
        },
        "textDocument": {
            "synchronization": { "dynamicRegistration": false, "willSave": false, "willSaveWaitUntil": false, "didSave": true },
            "completion": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "insertTextMode": 2,
                "completionItem": {
                    "snippetSupport": true,
                    "commitCharactersSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "deprecatedSupport": true,
                    "preselectSupport": true,
                    "tagSupport": { "valueSet": [1] },
                    "insertReplaceSupport": true,
                    "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] },
                    "insertTextModeSupport": { "valueSet": [1, 2] },
                    "labelDetailsSupport": true,
                },
                "completionItemKind": { "valueSet": completion_kinds },
                "completionList": { "itemDefaults": ["commitCharacters", "editRange", "insertTextFormat", "insertTextMode", "data"] },
            },
            "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
            "signatureHelp": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true,
                },
            },
            "declaration": { "dynamicRegistration": false, "linkSupport": true },
            "definition": { "dynamicRegistration": false, "linkSupport": true },
            "typeDefinition": { "dynamicRegistration": false, "linkSupport": true },
            "implementation": { "dynamicRegistration": false, "linkSupport": true },
            "references": { "dynamicRegistration": false },
            "documentHighlight": { "dynamicRegistration": false },
            "documentSymbol": {
                "dynamicRegistration": false,
                "symbolKind": { "valueSet": symbol_kinds },
                "hierarchicalDocumentSymbolSupport": true,
                "tagSupport": { "valueSet": [1] },
                "labelSupport": true,
            },
            "codeAction": {
                "dynamicRegistration": false,
                "isPreferredSupport": true,
                "disabledSupport": true,
                "dataSupport": true,
                "resolveSupport": { "properties": ["edit"] },
                "codeActionLiteralSupport": {
                    "codeActionKind": {
                        "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports", "source.fixAll"]
                    }
                },
                "honorsChangeAnnotations": false,
            },
            "formatting": { "dynamicRegistration": false },
            "rangeFormatting": { "dynamicRegistration": false },
            "rename": { "dynamicRegistration": false, "prepareSupport": true, "prepareSupportDefaultBehavior": 1 },
            "publishDiagnostics": {
                "relatedInformation": true,
                "tagSupport": { "valueSet": [1, 2] },
                "versionSupport": true,
                "codeDescriptionSupport": true,
                "dataSupport": true,
            },
            "foldingRange": { "dynamicRegistration": false, "rangeLimit": 5000, "lineFoldingOnly": false, "foldingRangeKind": { "valueSet": ["comment", "imports", "region"] } },
            "selectionRange": { "dynamicRegistration": false },
            "callHierarchy": { "dynamicRegistration": false },
            "typeHierarchy": { "dynamicRegistration": false },
            "inlayHint": { "dynamicRegistration": false, "resolveSupport": { "properties": ["tooltip", "textEdits", "label.tooltip", "label.location", "label.command"] } },
            "diagnostic": { "dynamicRegistration": false, "relatedDocumentSupport": false },
            "semanticTokens": {
                "dynamicRegistration": false,
                "requests": { "range": false, "full": { "delta": false } },
                "tokenTypes": [
                    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property",
                    "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number", "regexp",
                    "operator", "decorator"
                ],
                "tokenModifiers": ["declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification", "documentation", "defaultLibrary"],
                "formats": ["relative"],
                "overlappingTokenSupport": false,
                "multilineTokenSupport": false,
                "serverCancelSupport": true,
                "augmentsSyntaxTokens": true,
            },
        },
        "window": {
            "workDoneProgress": true,
            "showMessage": { "messageActionItem": { "additionalPropertiesSupport": false } },
            "showDocument": { "support": false },
        },
        // rust-analyzer: report when it is still loading the workspace.
        "experimental": { "serverStatusNotification": true },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_sections_resolve_by_name_or_path() {
        let s = json!({ "rust-analyzer": { "check": { "command": "clippy" } }, "python": { "analysis": { "typeCheckingMode": "strict" } } });
        assert_eq!(config_section(&s, Some("rust-analyzer"))["check"]["command"], "clippy");
        assert_eq!(config_section(&s, Some("python.analysis"))["typeCheckingMode"], "strict");
        assert_eq!(config_section(&s, Some("nope")), Value::Null);
        assert_eq!(config_section(&s, None), s);
        assert_eq!(config_section(&Value::Null, Some("x")), Value::Null);
    }

    #[test]
    fn log_ring_is_bounded_and_truncates() {
        let mut r = LogRing::default();
        for i in 0..(LOG_LINES + 10) {
            r.push("stderr", &format!("line {i}"));
        }
        let t = r.tail(5);
        assert_eq!(t.len(), 5);
        assert_eq!(t[4].text, format!("line {}", LOG_LINES + 9));
        assert_eq!(r.tail(usize::MAX).len(), LOG_LINES);
        r.push("stderr", &"x".repeat(LOG_LINE_MAX * 2));
        assert_eq!(r.tail(1)[0].text.chars().count(), LOG_LINE_MAX + 1);
    }
}

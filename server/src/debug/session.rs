//! Debug sessions: one adapter process (or connection) each, driven through DAP.
//!
//! Start-up (`drive`): the pre-launch step (a run configuration, a command, or the
//! derived build that names the executable) in a visible terminal, the adapter,
//! `initialize`, `launch`/`attach` (sent without waiting: gdb answers it only after
//! `configurationDone`), the `initialized` event, breakpoints, `configurationDone`.
//! An event loop per session applies events (`stopped`, `continued`, `output`,
//! `breakpoint`, `thread`, `process`, `exited`, `terminated`) and answers reverse
//! requests (`runInTerminal` → a Workbench terminal, `startDebugging` → a child
//! session). State changes reach the UI as `debug.session`, console output as
//! `debug.output` (batched per burst of events), breakpoint verification as
//! `debug.breakpoints`.
//!
//! Ending (`finish`, idempotent): the adapter's stdin is closed, then its process
//! group is signalled; a debuggee Workbench launched (its pid from the `process`
//! event, still a child of the adapter) and debuggee terminals are killed too.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::adapters::{Adapter, AdapterKind};
use super::breakpoints;
use super::client::{DapClient, DapError, Incoming};
use super::derive;
use super::launch::{self, Build, Plan, PreLaunch};
use super::process::{self, AdapterDir, AdapterProc};
use crate::app::AppState;
use crate::config::project::DebugRequest;
use crate::devcontainer::ExecTarget;
use crate::error::ApiError;
use crate::projects::Project;
use crate::secrets::Secret;
use crate::terminals::{SpawnSpec, TerminalKind};

pub const INIT_TIMEOUT: Duration = Duration::from_secs(30);
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(180);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const EVAL_TIMEOUT: Duration = Duration::from_secs(30);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_OUTPUT_ENTRIES: usize = 3000;
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 64 * 1024;
/// Ended sessions kept per project (their console stays readable).
const KEEP_ENDED: usize = 6;
/// Files outside the project a session's frames and output named (see `Session::knows_source`).
const MAX_SOURCES: usize = 5000;
/// The adapter's last lines of stderr (or non-DAP stdout), for "exited unexpectedly".
const ADAPTER_TAIL: usize = 6;
pub const MAX_LIVE: usize = 16;
const PRELAUNCH_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Starting,
    Running,
    Stopped,
    Terminated,
    Failed,
}

impl SessionState {
    pub fn live(self) -> bool {
        matches!(self, SessionState::Starting | SessionState::Running | SessionState::Stopped)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopInfo {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    pub all_threads_stopped: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hit_breakpoint_ids: Vec<String>,
    /// The program stopped while evaluating this expression (a watch that calls a
    /// function hit a breakpoint in it): the UI does not evaluate it by itself again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub during_evaluation: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputLine {
    pub seq: u64,
    /// DAP categories (`console`, `important`, `stdout`, `stderr`) plus `adapter` (the
    /// adapter's own stderr) and `workbench` (Workbench's messages).
    pub category: String,
    pub text: String,
    pub at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BpStatus {
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    pub adapter: String,
    pub adapter_label: String,
    pub request: DebugRequest,
    pub state: SessionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StopInfo>,
    /// Bumped on every stop, resume and `invalidated` event: frame ids and variable
    /// references of an older epoch are stale.
    pub stop_epoch: u64,
    pub threads: Vec<ThreadView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessView>,
    pub capabilities: Value,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub in_container: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Terminals of this session: the pre-launch step's and the debuggee's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prelaunch_terminal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debuggee_terminal_id: Option<String>,
    pub output_seq: u64,
    /// The user stopped the session (Stop or Rerun): it ended as "Stopped" (a launch)
    /// or "Detached" (an attach), not with the killed program's exit code.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stop_requested: bool,
}

/// Host paths ↔ the paths the adapter sees (the same, or the dev container's).
#[derive(Debug, Clone)]
pub struct PathMap {
    pub root: PathBuf,
    canon_root: PathBuf,
    container: Option<(PathBuf, String)>,
}

impl PathMap {
    pub fn new(root: &Path, target: Option<&ExecTarget>) -> Self {
        Self {
            root: root.to_path_buf(),
            canon_root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            container: target.and_then(|t| t.map.clone()),
        }
    }

    pub fn to_adapter(&self, host: &Path) -> String {
        if let Some((src, dst)) = &self.container {
            if let Ok(rel) = host.strip_prefix(src) {
                let rel = rel.to_string_lossy();
                return if rel.is_empty() { dst.clone() } else { format!("{}/{rel}", dst.trim_end_matches('/')) };
            }
        }
        host.display().to_string()
    }

    pub fn host_of(&self, adapter_path: &str) -> PathBuf {
        if let Some((src, dst)) = &self.container {
            if let Ok(rel) = Path::new(adapter_path).strip_prefix(dst) {
                return src.join(rel);
            }
        }
        PathBuf::from(adapter_path)
    }

    /// The project-relative path of a host path, following symlinks when the plain
    /// prefix does not match (a program built through a symlinked folder).
    pub fn project_rel(&self, host: &Path) -> Option<String> {
        if let Some(r) = crate::util::paths::relative_to(&self.root, host) {
            return Some(r);
        }
        let canon = host.canonicalize().ok()?;
        crate::util::paths::relative_to(&self.canon_root, &canon)
    }

    /// A DAP `Source` as the UI shows it.
    pub fn source_view(&self, src: &Value) -> Value {
        let name = src.get("name").cloned().unwrap_or(Value::Null);
        let reference = src.get("sourceReference").and_then(Value::as_i64).filter(|r| *r > 0);
        match src.get("path").and_then(Value::as_str) {
            Some(p) if !p.is_empty() => {
                let host = self.host_of(p);
                match self.project_rel(&host) {
                    Some(rel) => json!({ "name": name, "path": rel, "inProject": true, "sourceReference": reference }),
                    None => json!({ "name": name, "path": host.display().to_string(), "inProject": false, "sourceReference": reference }),
                }
            }
            _ => json!({ "name": name, "inProject": false, "sourceReference": reference }),
        }
    }
}

#[derive(Default)]
struct Data {
    state: Option<SessionState>,
    phase: Option<String>,
    error: Option<String>,
    capabilities: Value,
    threads: Vec<ThreadView>,
    stopped: Option<StopInfo>,
    stop_epoch: u64,
    exit_code: Option<i64>,
    process: Option<ProcessView>,
    output: VecDeque<OutputLine>,
    output_bytes: usize,
    out_seq: u64,
    pending_out: Vec<OutputLine>,
    /// Our breakpoint id → its status in this session.
    bp: HashMap<String, BpStatus>,
    /// The adapter's breakpoint id → ours.
    adapter_bp: HashMap<i64, String>,
    /// Files we sent breakpoints for (to clear them when the last one goes).
    sent_sources: BTreeSet<String>,
    /// Run to cursor: a breakpoint only this session has, until the next stop.
    temp_bp: Option<(String, i64)>,
    debuggee_terminals: Vec<String>,
    prelaunch_terminal: Option<String>,
    debuggee_pid: Option<i32>,
    temp_files: Vec<PathBuf>,
    ended_at: Option<i64>,
    /// The adapter's last lines of stderr / non-DAP output.
    adapter_tail: VecDeque<String>,
    /// A TCP adapter's port (child sessions connect to it).
    adapter_port: Option<u16>,
    /// Absolute paths outside the project the adapter named in frames and output.
    sources: HashSet<String>,
    configured: bool,
    dirty: bool,
    bp_dirty: bool,
}

pub struct Session {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub config: Option<String>,
    pub adapter: Adapter,
    pub request: DebugRequest,
    pub parent: Option<String>,
    pub paths: PathMap,
    pub cancel: CancellationToken,
    pub started_at: i64,
    plan: Plan,
    client: Mutex<Option<Arc<DapClient>>>,
    proc_: tokio::sync::Mutex<Option<AdapterProc>>,
    initialized: watch::Sender<bool>,
    terminated: AtomicBool,
    finished: AtomicBool,
    /// Stop (or Rerun) was asked for: the end is the user's, not the program's.
    stop_requested: AtomicBool,
    /// Set once `finish` completed (other callers wait for it).
    ended: watch::Sender<bool>,
    data: Mutex<Data>,
    redact: Vec<Secret>,
}

fn new_id() -> String {
    format!("d{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// The capabilities the UI uses (the rest stay on the server).
fn ui_capabilities(c: &Value) -> Value {
    const KEYS: &[&str] = &[
        "supportsFunctionBreakpoints",
        "supportsConditionalBreakpoints",
        "supportsHitConditionalBreakpoints",
        "supportsLogPoints",
        "supportsSetVariable",
        "supportsSetExpression",
        "supportsCompletionsRequest",
        "completionTriggerCharacters",
        "supportsTerminateRequest",
        "supportsEvaluateForHovers",
        "supportsStepBack",
        "supportsRestartFrame",
        "supportsGotoTargetsRequest",
        "supportsValueFormattingOptions",
        "exceptionBreakpointFilters",
    ];
    let mut m = serde_json::Map::new();
    if let Some(o) = c.as_object() {
        for k in KEYS {
            if let Some(v) = o.get(*k) {
                m.insert(k.to_string(), v.clone());
            }
        }
    }
    Value::Object(m)
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

impl Session {
    fn new(project: &Project, plan: &Plan, parent: Option<String>) -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            id: new_id(),
            project_id: project.id.clone(),
            name: plan.name.clone(),
            config: plan.config.clone(),
            adapter: plan.adapter.clone(),
            request: plan.request,
            parent,
            paths: PathMap::new(&project.root, plan.target.as_ref()),
            cancel: CancellationToken::new(),
            started_at: crate::util::now_ms(),
            plan: plan.clone(),
            client: Mutex::new(None),
            proc_: tokio::sync::Mutex::new(None),
            initialized: tx,
            terminated: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            ended: watch::channel(false).0,
            data: Mutex::new(Data { state: Some(SessionState::Starting), dirty: true, ..Default::default() }),
            redact: plan.secrets.clone(),
        }
    }

    pub fn state(&self) -> SessionState {
        self.data.lock().state.unwrap_or(SessionState::Starting)
    }

    pub fn is_live(&self) -> bool {
        self.state().live()
    }

    pub fn client(&self) -> Result<Arc<DapClient>, ApiError> {
        match self.client.lock().clone() {
            Some(c) if !c.is_closed() => Ok(c),
            Some(_) => Err(ApiError::conflict("the debug session has ended")),
            None => Err(ApiError::conflict("the debugger is still starting")),
        }
    }

    pub fn capabilities(&self) -> Value {
        self.data.lock().capabilities.clone()
    }

    fn cap(&self, k: &str) -> bool {
        self.data.lock().capabilities.get(k).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn info(&self) -> SessionInfo {
        let d = self.data.lock();
        SessionInfo {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            name: self.name.clone(),
            config: self.config.clone(),
            adapter: self.adapter.id.clone(),
            adapter_label: self.adapter.label.clone(),
            request: self.request,
            state: d.state.unwrap_or(SessionState::Starting),
            phase: d.phase.clone(),
            error: d.error.clone(),
            stopped: d.stopped.clone(),
            stop_epoch: d.stop_epoch,
            threads: d.threads.clone(),
            exit_code: d.exit_code,
            process: d.process.clone(),
            capabilities: ui_capabilities(&d.capabilities),
            started_at: self.started_at,
            ended_at: d.ended_at,
            in_container: self.plan.target.is_some(),
            parent_id: self.parent.clone(),
            prelaunch_terminal_id: d.prelaunch_terminal.clone(),
            debuggee_terminal_id: d.debuggee_terminals.last().cloned(),
            output_seq: d.out_seq,
            stop_requested: self.stop_requested.load(Ordering::SeqCst),
        }
    }

    /// `text` with the values of the session's `${secret:…}` variables masked: what
    /// the debuggee holds (variables, evaluations, stop messages) reaches the browser
    /// and agents only through this, like the console.
    pub fn redact(&self, text: &str) -> String {
        crate::secrets::redact(text, &self.redact)
    }

    /// The process an attach session attached to.
    pub fn attach_pid(&self) -> Option<u32> {
        (self.request == DebugRequest::Attach).then_some(self.plan.pid).flatten()
    }

    /// The dev container the adapter runs in.
    pub fn target(&self) -> Option<&ExecTarget> {
        self.plan.target.as_ref()
    }

    /// Remember a file outside the project the adapter named (a frame's or an output
    /// line's source): the session's read-only source view may show it.
    pub fn note_source(&self, path: &str) {
        if !path.starts_with('/') || path.len() > 4096 {
            return;
        }
        let mut d = self.data.lock();
        if d.sources.len() < MAX_SOURCES {
            d.sources.insert(path.to_string());
        }
    }

    /// Whether the adapter named `path` in this session's frames or output.
    pub fn knows_source(&self, path: &str) -> bool {
        self.data.lock().sources.contains(path)
    }

    fn note_adapter_line(&self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            return;
        }
        let mut d = self.data.lock();
        if d.adapter_tail.len() >= ADAPTER_TAIL {
            d.adapter_tail.pop_front();
        }
        d.adapter_tail.push_back(truncate(line.to_string(), 300));
    }

    pub fn bp_status(&self, id: &str) -> Option<BpStatus> {
        self.data.lock().bp.get(id).cloned()
    }

    /// Console entries after `after` (seq), and whether older ones were dropped.
    pub fn output_after(&self, after: u64, limit: usize) -> (Vec<OutputLine>, bool) {
        let d = self.data.lock();
        let dropped = d.output.front().is_some_and(|f| f.seq > after + 1);
        let v: Vec<OutputLine> = d.output.iter().filter(|l| l.seq > after).take(limit).cloned().collect();
        (v, dropped)
    }

    fn set_phase(&self, state: &AppState, phase: impl Into<String>) {
        {
            let mut d = self.data.lock();
            d.phase = Some(phase.into());
            d.dirty = true;
        }
        self.flush(state);
    }

    /// Append console output (redacted, capped).
    pub fn log(&self, category: &str, text: impl Into<String>, source: Option<(String, i64)>) {
        let text: String = text.into();
        if text.is_empty() {
            return;
        }
        let text = truncate(crate::secrets::redact(&text, &self.redact), MAX_ENTRY_BYTES);
        let mut d = self.data.lock();
        d.out_seq += 1;
        let line = OutputLine {
            seq: d.out_seq,
            category: category.to_string(),
            text,
            at: crate::util::now_ms(),
            path: source.as_ref().map(|s| s.0.clone()),
            line: source.map(|s| s.1),
        };
        d.output_bytes += line.text.len();
        d.output.push_back(line.clone());
        while d.output.len() > MAX_OUTPUT_ENTRIES || d.output_bytes > MAX_OUTPUT_BYTES {
            match d.output.pop_front() {
                Some(old) => d.output_bytes -= old.text.len(),
                None => break,
            }
        }
        if d.pending_out.len() < 500 {
            d.pending_out.push(line);
        } else {
            // A flood: the UI refetches from `output?after=` instead.
            d.dirty = true;
        }
    }

    /// Emit what changed since the last flush.
    pub fn flush(&self, state: &AppState) {
        let (out, dirty, bp_dirty) = {
            let mut d = self.data.lock();
            let out = std::mem::take(&mut d.pending_out);
            let r = (out, d.dirty, d.bp_dirty);
            d.dirty = false;
            d.bp_dirty = false;
            r
        };
        if !out.is_empty() {
            state.events.emit("debug.output", Some(&self.project_id), json!({ "sessionId": self.id, "lines": out }));
        }
        if dirty {
            state.events.emit("debug.session", Some(&self.project_id), self.info());
        }
        if bp_dirty {
            emit_breakpoints(state, &self.project_id);
        }
    }

    fn update(&self, f: impl FnOnce(&mut Data)) {
        let mut d = self.data.lock();
        f(&mut d);
        d.dirty = true;
    }

    fn fail(&self, state: &AppState, msg: String) {
        let msg = crate::secrets::redact(&msg, &self.redact);
        let mut changed = false;
        self.update(|d| {
            if d.state.is_some_and(|s| s.live()) {
                d.state = Some(SessionState::Failed);
                d.error = Some(msg.clone());
                changed = true;
            }
        });
        // A second explanation of the same end (startup and the event loop both saw
        // the adapter go) stays out of the console.
        if changed {
            self.log("workbench", format!("{msg}\n"), None);
        }
        self.flush(state);
    }

    async fn wait_initialized(&self) {
        let mut rx = self.initialized.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    /// Send a request to the adapter (the session must have one).
    pub async fn request(&self, command: &str, args: Value, timeout: Duration) -> Result<Value, ApiError> {
        let c = self.client()?;
        c.request(command, args, timeout).await.map_err(ApiError::from)
    }

    /// Evaluate: a stop while it runs (a called function hit a breakpoint) names
    /// `expr` in `StopInfo::during_evaluation`.
    pub async fn evaluate(&self, expr: &str, args: Value, timeout: Duration) -> Result<Value, ApiError> {
        let c = self.client()?;
        c.request_tagged("evaluate", args, timeout, Some(expr.to_string())).await.map_err(ApiError::from)
    }
}

// ---------------------------------------------------------------- registry helpers

pub fn emit_breakpoints(state: &AppState, pid: &str) {
    state.events.emit("debug.breakpoints", Some(pid), breakpoints_view(state, pid));
}

/// Breakpoints of a project with their verification in the live sessions: verified
/// when any session verified it; unverified (with the adapter's message) when
/// sessions run and none did; no status when no session runs.
pub fn breakpoints_view(state: &AppState, pid: &str) -> Value {
    let pd = state.debug.store.get(&state.paths.data_dir, pid);
    let live: Vec<Arc<Session>> = state.debug.sessions_of(pid).into_iter().filter(|s| s.is_live()).collect();
    let status = |id: &str| -> Option<BpStatus> {
        let all: Vec<BpStatus> = live.iter().filter_map(|s| s.bp_status(id)).collect();
        all.iter().find(|s| s.verified).cloned().or_else(|| all.first().cloned())
    };
    let bps: Vec<Value> = pd
        .breakpoints
        .iter()
        .map(|b| {
            let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
            v["status"] = serde_json::to_value(status(&b.id)).unwrap_or(Value::Null);
            v
        })
        .collect();
    let fbs: Vec<Value> = pd
        .function_breakpoints
        .iter()
        .map(|b| {
            let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
            v["status"] = serde_json::to_value(status(&b.id)).unwrap_or(Value::Null);
            v
        })
        .collect();
    let known = state.debug.known_filters.lock().clone();
    let filters: Vec<Value> = known
        .iter()
        .map(|(adapter, (label, filters))| {
            let enabled: Vec<String> = match pd.exception_filters.get(adapter) {
                Some(e) => e.clone(),
                None => default_filters(filters),
            };
            json!({ "adapter": adapter, "label": label, "filters": filters, "enabled": enabled })
        })
        .collect();
    json!({
        "breakpoints": bps,
        "functionBreakpoints": fbs,
        "exceptionFilters": filters,
        "muted": pd.muted,
        "watches": pd.watches,
        "lastConfig": pd.last_config,
        "live": !live.is_empty(),
    })
}

fn default_filters(filters: &Value) -> Vec<String> {
    filters
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f.get("default").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|f| f.get("filter").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- start

/// A child session an adapter asked for (`startDebugging`).
pub struct ChildRequest {
    pub project: Arc<Project>,
    pub plan: Plan,
    pub parent: String,
}

/// Start child sessions as adapters ask for them (runs for the server's lifetime).
pub async fn children(state: AppState, mut rx: mpsc::UnboundedReceiver<ChildRequest>) {
    while let Some(c) = rx.recv().await {
        if let Err(e) = start(&state, c.project, c.plan, Some(c.parent)).await {
            state.events.notify("error", &format!("child debug session: {}", e.message));
        }
    }
}

/// Create a session for `plan` and start it in the background. Errors found before
/// anything runs come back here; later ones end the session as `failed`.
pub async fn start(state: &AppState, project: Arc<Project>, plan: Plan, parent: Option<String>) -> Result<SessionInfo, ApiError> {
    if state.debug.shutdown.is_cancelled() {
        return Err(ApiError::conflict("Workbench is shutting down"));
    }
    if state.debug.all().iter().filter(|s| s.is_live()).count() >= MAX_LIVE {
        return Err(ApiError::conflict(format!("at most {MAX_LIVE} debug sessions can run at once; stop one first")));
    }
    let s = Arc::new(Session::new(&project, &plan, parent));
    state.debug.insert(s.clone());
    s.log("workbench", format!("Debugging {} with {}{}\n", plan.name, plan.adapter.label, if plan.target.is_some() { " in the dev container" } else { "" }), None);
    s.flush(state);
    if plan.config.is_some() && s.parent.is_none() {
        let name = plan.config.clone();
        let _ = state.debug.store.update(&state.paths.data_dir, &project.id, |p| {
            p.last_config = name;
            Ok(())
        }).await;
    }
    let info = s.info();
    let st = state.clone();
    let s2 = s.clone();
    tokio::spawn(async move {
        let result = tokio::select! {
            r = startup(&st, &project, &s2, plan) => r,
            _ = s2.cancel.cancelled() => Ok(()),
        };
        if let Err(msg) = result {
            s2.fail(&st, msg);
            finish(&st, &s2).await;
        }
    });
    Ok(info)
}

async fn startup(state: &AppState, project: &Arc<Project>, s: &Arc<Session>, plan: Plan) -> Result<(), String> {
    let target = plan.target.clone();
    let mut program = plan.program.as_ref().map(|p| launch::adapter_path(target.as_ref(), p));

    // 1. Pre-launch: the build that names the executable, a run configuration, or a command.
    if let Some(pre) = plan.pre.clone() {
        s.set_phase(state, format!("Pre-launch: {}", pre.describe()));
        match pre {
            PreLaunch::Run(name) => run_prelaunch_config(state, project, s, &name).await?,
            PreLaunch::Command(cmd) => {
                run_in_terminal(state, s, &plan, &format!("Before debugging: {}", plan.name), &cmd).await?;
            }
            PreLaunch::Build(Build::Cargo(t)) => program = Some(cargo_build(state, s, &plan, &t).await?),
            PreLaunch::Build(Build::Cmake(t)) => {
                let cmd = format!("cmake --build {} --target {}", crate::apps::expand::shell_quote(&t.build_dir), crate::apps::expand::shell_quote(&t.name));
                let mut root_plan = plan.clone();
                root_plan.cwd = project.root.clone();
                run_in_terminal(state, s, &root_plan, &format!("Build {}", t.name), &cmd).await?;
                let dir = project.root.join(&t.build_dir);
                let found = tokio::task::spawn_blocking(move || derive::find_executable(&dir, &t.name)).await.map_err(|e| e.to_string())?;
                let exe = found.ok_or_else(|| "the build finished but its executable was not found in the build directory".to_string())?;
                program = Some(launch::adapter_path(target.as_ref(), &exe));
            }
        }
        if let Some(p) = &program {
            s.log("workbench", format!("Program: {p}\n"), None);
        }
    }

    // 2. The adapter: a new process, or (a child session) a new connection to the
    // parent's adapter, which knows the child process.
    let (client, incoming) = if let Some((host, port)) = plan.connect.clone() {
        s.set_phase(state, format!("Connecting to {}", plan.adapter.label));
        s.log("workbench", format!("Connecting to {} on {host}:{port}\n", plan.adapter.label), None);
        process::connect(&host, port, plan.adapter.connect_timeout).await?
    } else {
        s.set_phase(state, format!("Starting {}", plan.adapter.label));
        let inside = target.as_ref().zip(plan.inside_command.clone());
        let mut extra_args = vec![];
        if plan.adapter.kind == AdapterKind::Gdb && plan.request == DebugRequest::Launch && plan.raw_arguments.is_none() {
            if launch::language_of(&plan.launch, &project.root) == "rust" && target.is_none() {
                extra_args.extend(rust_gdb_args(&project.root).await);
            }
            // Load the program at once: breakpoints resolve when they are set instead of
            // staying pending until the launch (gdb reads the file only then).
            if let Some(p) = &program {
                extra_args.push(p.clone());
            }
        }
        let neutral = neutral_dir(state);
        let dir = if process::runs_in_project(plan.adapter.kind, plan.request) && plan.raw_arguments.is_none() {
            AdapterDir::Project(&plan.cwd)
        } else {
            AdapterDir::Neutral(&neutral)
        };
        let started = process::spawn(&plan.adapter, &s.id, dir, inside, &extra_args).await?;
        *s.proc_.lock().await = Some(started.proc);
        s.update(|d| d.adapter_port = started.port);
        for r in [started.stderr.map(|e| Box::new(e) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), started.stdout_log.map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)]
            .into_iter()
            .flatten()
        {
            let (st, s2) = (state.clone(), s.clone());
            process::forward_lines(r, move |line| {
                s2.note_adapter_line(&line);
                s2.log("adapter", format!("{line}\n"), None);
                s2.flush(&st);
            });
        }
        (started.client, started.incoming)
    };
    *s.client.lock() = Some(client.clone());
    tokio::spawn(event_loop(state.clone(), s.clone(), incoming));

    // 3. initialize.
    let caps = client
        .request(
            "initialize",
            json!({
                "clientID": "workbench",
                "clientName": "Workbench",
                "adapterID": plan.adapter.adapter_id,
                "locale": "en",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsVariableType": true,
                "supportsVariablePaging": true,
                "supportsRunInTerminalRequest": true,
                "supportsStartDebuggingRequest": true,
                "supportsInvalidatedEvent": true,
                "supportsMemoryReferences": false,
                "supportsProgressReporting": false,
                "supportsArgsCanBeInterpretedByShell": false,
            }),
            INIT_TIMEOUT,
        )
        .await
        .map_err(|e| format!("{} did not initialize: {e}", plan.adapter.label))?;
    let caps = if caps.is_object() { caps } else { json!({}) };
    if let Some(filters) = caps.get("exceptionBreakpointFilters").filter(|f| f.as_array().is_some_and(|a| !a.is_empty())) {
        state.debug.known_filters.lock().insert(plan.adapter.id.clone(), (plan.adapter.label.clone(), filters.clone()));
    }
    s.update(|d| d.capabilities = caps.clone());

    // 4. launch / attach, without waiting for its answer (see the module docs).
    let attach = plan.request == DebugRequest::Attach;
    if attach && plan.pid == Some(0) {
        return Err("no process to attach to".into());
    }
    s.set_phase(state, if attach { "Attaching" } else { "Launching" });
    let terminal = plan.adapter.supports_terminal() && plan.launch.console.as_deref() != Some("console");
    let args = launch::arguments(&plan, program.as_deref(), terminal);
    let command = if attach { "attach" } else { "launch" };
    let c2 = client.clone();
    let mut launch_task = tokio::spawn(async move { c2.request(command, args, LAUNCH_TIMEOUT).await });
    let mut launched: Option<Value> = None;
    let explain = |e: DapError| -> String {
        let mut m = format!("{command} failed: {e}");
        if attach && super::procs::ptrace_scope().is_some_and(|x| x > 0) {
            let low = m.to_ascii_lowercase();
            if low.contains("not permitted") || low.contains("ptrace") || low.contains("permission") {
                if let Some(h) = super::procs::ptrace_hint(super::procs::ptrace_scope()) {
                    m.push_str(". ");
                    m.push_str(&h);
                }
            }
        }
        m
    };
    tokio::select! {
        _ = s.wait_initialized() => {}
        r = &mut launch_task => {
            match r.map_err(|e| e.to_string())? {
                Err(e) => return Err(explain(e)),
                Ok(v) => {
                    launched = Some(v);
                    // Some adapters send `initialized` only after answering.
                    let _ = tokio::time::timeout(Duration::from_secs(3), s.wait_initialized()).await;
                }
            }
        }
        _ = tokio::time::sleep(LAUNCH_TIMEOUT) => return Err(format!("{} never became ready (no `initialized` event)", plan.adapter.label)),
    }

    // 5. Configuration.
    configure(state, s, &client).await;
    if s.cap("supportsConfigurationDoneRequest") {
        client.request("configurationDone", Value::Null, REQUEST_TIMEOUT).await.map_err(|e| format!("configurationDone failed: {e}"))?;
    }
    if launched.is_none() {
        match launch_task.await.map_err(|e| e.to_string())? {
            Ok(_) => {}
            Err(e) => return Err(explain(e)),
        }
    }
    // gdb (17) answers `attach` with success even when ptrace refused, and says
    // nothing: check that a native debugger really traces the process.
    if let (true, Some(pid), None) = (attach, plan.pid, &target) {
        if matches!(plan.adapter.kind, AdapterKind::Gdb | AdapterKind::Lldb | AdapterKind::Codelldb) && !traced(pid).await {
            let mut m = format!("{} could not attach to process {pid}", plan.adapter.label);
            if let Some(h) = super::procs::ptrace_hint(super::procs::ptrace_scope()) {
                m.push_str(". ");
                m.push_str(&h);
            }
            return Err(m);
        }
    }
    s.update(|d| {
        d.phase = None;
        if d.state == Some(SessionState::Starting) {
            d.state = Some(SessionState::Running);
        }
    });
    s.flush(state);
    Ok(())
}

/// The directory adapters that need no project directory run in (see
/// `process::runs_in_project`): `data_dir/debug/adapter`, empty and private.
fn neutral_dir(state: &AppState) -> PathBuf {
    let dir = state.paths.data_dir.join("debug").join("adapter");
    if std::fs::create_dir_all(&dir).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        return dir;
    }
    PathBuf::from("/")
}

/// Whether some process traces `pid` (`TracerPid` in `/proc/<pid>/status`), waiting
/// up to three seconds for the debugger to get there.
async fn traced(pid: u32) -> bool {
    for _ in 0..15 {
        let status = tokio::fs::read_to_string(format!("/proc/{pid}/status")).await.unwrap_or_default();
        let tracer = status.lines().find_map(|l| l.strip_prefix("TracerPid:")).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
        if tracer != 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// What `rust-gdb` adds: the toolchain's pretty printers (`Vec`, `String`,
/// `Option`… shown as values). The sysroot is the default toolchain's (`rustc
/// --print sysroot` outside the project, without rustup installing anything): a
/// `rust-toolchain.toml` could name a toolchain inside the repository, whose scripts
/// gdb would then load.
async fn rust_gdb_args(root: &Path) -> Vec<String> {
    let mut cmd = tokio::process::Command::new("rustc");
    cmd.args(["--print", "sysroot"]).current_dir("/").env("RUSTUP_AUTO_INSTALL", "0");
    let Ok(out) = crate::util::proc::run_cmd(cmd, Duration::from_secs(10)).await else { return vec![] };
    let sysroot = PathBuf::from(out.stdout.trim());
    let etc = sysroot.join("lib/rustlib/etc");
    if !out.ok() || !sysroot.is_absolute() || !etc.join("gdb_load_rust_pretty_printers.py").is_file() || etc.starts_with(root) {
        return vec![];
    }
    let etc = etc.display().to_string();
    vec![format!("--directory={etc}"), "-iex".into(), format!("add-auto-load-safe-path {etc}")]
}

/// A pre-launch run configuration: started through the apps slice (its terminal,
/// dependencies and readiness), then waited for until it exits (0 = go on) or is
/// ready (a server).
async fn run_prelaunch_config(state: &AppState, project: &Arc<Project>, s: &Arc<Session>, name: &str) -> Result<(), String> {
    use crate::apps::runs::RunState;
    crate::apps::runs::start(state, project, name, false).await.map_err(|e| format!("pre-launch run {name:?}: {}", e.message))?;
    let deadline = tokio::time::Instant::now() + PRELAUNCH_TIMEOUT;
    let mut seen_active = false;
    let mut idle_polls = 0;
    loop {
        let live = state.apps.runs.live(&project.id, name);
        if let Some(t) = &live.terminal_id {
            if s.data.lock().prelaunch_terminal.as_deref() != Some(t) {
                let t = t.clone();
                s.update(|d| d.prelaunch_terminal = Some(t));
                s.flush(state);
            }
        }
        match live.state {
            RunState::Ready => return Ok(()),
            RunState::Exited => {
                return match live.exit.as_ref().and_then(|e| e.code) {
                    Some(0) => Ok(()),
                    Some(c) => Err(format!("pre-launch run {name:?} failed (exit code {c})")),
                    None => Err(format!("pre-launch run {name:?} was stopped")),
                };
            }
            RunState::Failed => return Err(format!("pre-launch run {name:?} failed: {}", live.error.unwrap_or_default())),
            RunState::Starting | RunState::Running => seen_active = true,
            RunState::Stopped => {
                idle_polls += 1;
                if seen_active || idle_polls > 20 {
                    return Err(format!("pre-launch run {name:?} was stopped"));
                }
            }
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("pre-launch run {name:?} did not finish within an hour"));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Run `command` in the run shell (`bash -lc` on Unix) in a visible terminal of the
/// project (in its dev container when the session uses it) and wait for it to succeed.
async fn run_in_terminal(state: &AppState, s: &Arc<Session>, plan: &Plan, title: &str, command: &str) -> Result<String, String> {
    let mut meta = json!({ "debug": s.id, "debugPreLaunch": true });
    if plan.target.is_some() {
        meta["inContainer"] = json!(true);
    }
    let spec = SpawnSpec {
        kind: TerminalKind::Command,
        title: title.to_string(),
        project_id: Some(s.project_id.clone()),
        cwd: plan.cwd.clone(),
        argv: crate::util::os::shell::run_argv(command),
        env: vec![],
        cols: None,
        rows: None,
        meta,
    };
    s.log("workbench", format!("$ {command}\n"), None);
    let info = state.terminals.spawn(state, spec).await.map_err(|e| format!("could not start the pre-launch step: {}", e.message))?;
    let id = info.id.clone();
    s.update(|d| d.prelaunch_terminal = Some(id.clone()));
    s.flush(state);
    let mut rx = state.terminals.exit_watch(&id).ok_or("the pre-launch terminal vanished")?;
    let exit = tokio::time::timeout(PRELAUNCH_TIMEOUT, rx.wait_for(|v| v.is_some()))
        .await
        .map_err(|_| "the pre-launch step did not finish within an hour".to_string())?
        .map_err(|_| "the pre-launch terminal vanished".to_string())?
        .clone();
    match exit {
        Some(e) if e.code == Some(0) => Ok(id),
        Some(e) => Err(format!(
            "the pre-launch step failed ({}); its output is in the terminal \"{title}\"",
            match (e.code, e.signal) {
                (Some(c), _) => format!("exit code {c}"),
                (None, Some(sig)) => format!("signal {sig}"),
                _ => "no exit code".into(),
            }
        )),
        None => Err("the pre-launch terminal ended".into()),
    }
}

/// Build a Cargo target in a terminal and return the executable (as the adapter sees it).
async fn cargo_build(state: &AppState, s: &Arc<Session>, plan: &Plan, t: &derive::CargoTarget) -> Result<String, String> {
    let q = crate::apps::expand::shell_quote;
    let (file_arg, host_file) = match &plan.target {
        Some(_) => (format!("/tmp/.workbench-debug-{}.json", s.id), None),
        None => {
            let dir = state.paths.data_dir.join("debug").join("tmp");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let f = dir.join(format!("{}.cargo.json", s.id));
            s.update(|d| d.temp_files.push(f.clone()));
            (f.display().to_string(), Some(f))
        }
    };
    let args: Vec<String> = t.build_args().iter().map(|a| q(a)).collect();
    // JSON messages go to the file; the human-readable diagnostics stay in the terminal.
    let cmd = format!("cargo {} > {}", args.join(" "), q(&file_arg));
    let mut root_plan = plan.clone();
    root_plan.cwd = s.paths.root.join(&t.workspace);
    let title = format!("Build {}", t.config_name().trim_start_matches("Cargo: "));
    let result = run_in_terminal(state, s, &root_plan, &title, &cmd).await;
    let text = match (&host_file, &plan.target) {
        (Some(f), _) => {
            let f = f.clone();
            tokio::task::spawn_blocking(move || {
                let meta = std::fs::metadata(&f).ok()?;
                (meta.len() < 64 * 1024 * 1024).then(|| std::fs::read_to_string(&f).ok()).flatten()
            })
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
        }
        (None, Some(t)) => {
            let out = crate::devcontainer::docker::exec(&t.docker, &t.container_id, t.user.as_deref(), &["cat", &file_arg], Duration::from_secs(20)).await;
            let _ = crate::devcontainer::docker::exec(&t.docker, &t.container_id, t.user.as_deref(), &["rm", "-f", &file_arg], Duration::from_secs(10)).await;
            out.map(|o| o.stdout).unwrap_or_default()
        }
        (None, None) => String::new(),
    };
    if let Some(f) = host_file {
        let _ = std::fs::remove_file(f);
    }
    result?;
    derive::cargo_executable(&text, t).ok_or_else(|| format!("cargo built {} but reported no executable for it", t.name))
}

// ---------------------------------------------------------------- breakpoints

async fn configure(state: &AppState, s: &Arc<Session>, client: &Arc<DapClient>) {
    sync_breakpoints(state, s, None).await;
    sync_functions(state, s).await;
    sync_exceptions(state, s).await;
    let _ = client;
    s.update(|d| d.configured = true);
    s.flush(state);
}

/// Send the breakpoints of every file (or just `only`) to the session.
pub async fn sync_breakpoints(state: &AppState, s: &Arc<Session>, only: Option<&str>) {
    let Ok(client) = s.client() else { return };
    let pd = state.debug.store.get(&state.paths.data_dir, &s.project_id);
    let caps = s.capabilities();
    let files = pd.by_file();
    let (temp, sent) = {
        let d = s.data.lock();
        (d.temp_bp.clone(), d.sent_sources.clone())
    };
    let mut targets: BTreeSet<String> = files.keys().cloned().collect();
    targets.extend(sent);
    if let Some((p, _)) = &temp {
        targets.insert(p.clone());
    }
    for path in targets {
        if only.is_some_and(|o| o != path) {
            continue;
        }
        let list: Vec<&breakpoints::LineBreakpoint> = if pd.muted { vec![] } else { files.get(&path).cloned().unwrap_or_default() };
        let (mut sent_bps, skipped) = breakpoints::to_dap(&list, &caps);
        if let Some((_, line)) = temp.as_ref().filter(|(p, _)| *p == path) {
            if !sent_bps.iter().any(|(_, v)| v["line"] == json!(line)) {
                sent_bps.push((String::new(), json!({ "line": line })));
            }
        }
        let host = s.paths.root.join(&path);
        let name = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let body = json!({
            "source": { "path": s.paths.to_adapter(&host), "name": name },
            "breakpoints": sent_bps.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
            "lines": sent_bps.iter().map(|(_, v)| v["line"].clone()).collect::<Vec<_>>(),
            "sourceModified": false,
        });
        let r = client.request("setBreakpoints", body, REQUEST_TIMEOUT).await;
        let mut d = s.data.lock();
        let ids_here: Vec<String> = pd.breakpoints.iter().filter(|b| b.path == path).map(|b| b.id.clone()).collect();
        for id in &ids_here {
            d.bp.remove(id);
        }
        d.adapter_bp.retain(|_, ours| !ids_here.contains(ours));
        match r {
            Ok(body) => {
                let answered = body.get("breakpoints").and_then(Value::as_array).cloned().unwrap_or_default();
                for (i, (id, _)) in sent_bps.iter().enumerate() {
                    if id.is_empty() {
                        continue; // the run-to-cursor breakpoint
                    }
                    let a = answered.get(i).cloned().unwrap_or(Value::Null);
                    if let Some(aid) = a.get("id").and_then(Value::as_i64) {
                        d.adapter_bp.insert(aid, id.clone());
                    }
                    d.bp.insert(
                        id.clone(),
                        BpStatus {
                            verified: a.get("verified").and_then(Value::as_bool).unwrap_or(false),
                            line: a.get("line").and_then(Value::as_i64),
                            message: a.get("message").and_then(Value::as_str).map(|m| truncate(m.to_string(), 500)),
                        },
                    );
                }
            }
            Err(e) => {
                for (id, _) in &sent_bps {
                    if !id.is_empty() {
                        d.bp.insert(id.clone(), BpStatus { verified: false, line: None, message: Some(e.to_string()) });
                    }
                }
            }
        }
        for (id, why) in skipped {
            d.bp.insert(id, BpStatus { verified: false, line: None, message: Some(why) });
        }
        if sent_bps.is_empty() {
            d.sent_sources.remove(&path);
        } else {
            d.sent_sources.insert(path.clone());
        }
        d.bp_dirty = true;
    }
}

pub async fn sync_functions(state: &AppState, s: &Arc<Session>) {
    if !s.cap("supportsFunctionBreakpoints") {
        return;
    }
    let Ok(client) = s.client() else { return };
    let pd = state.debug.store.get(&state.paths.data_dir, &s.project_id);
    let conditional = s.cap("supportsConditionalBreakpoints");
    let list: Vec<&breakpoints::FunctionBreakpoint> = if pd.muted { vec![] } else { pd.function_breakpoints.iter().filter(|f| f.enabled).collect() };
    let body: Vec<Value> = list
        .iter()
        .map(|f| {
            let mut v = json!({ "name": f.name });
            if let (Some(c), true) = (&f.condition, conditional) {
                v["condition"] = json!(c);
            }
            v
        })
        .collect();
    let r = client.request("setFunctionBreakpoints", json!({ "breakpoints": body }), REQUEST_TIMEOUT).await;
    let mut d = s.data.lock();
    for f in &pd.function_breakpoints {
        d.bp.remove(&f.id);
    }
    match r {
        Ok(b) => {
            let answered = b.get("breakpoints").and_then(Value::as_array).cloned().unwrap_or_default();
            for (i, f) in list.iter().enumerate() {
                let a = answered.get(i).cloned().unwrap_or(Value::Null);
                if let Some(aid) = a.get("id").and_then(Value::as_i64) {
                    d.adapter_bp.insert(aid, f.id.clone());
                }
                d.bp.insert(
                    f.id.clone(),
                    BpStatus {
                        verified: a.get("verified").and_then(Value::as_bool).unwrap_or(false),
                        line: a.get("line").and_then(Value::as_i64),
                        message: a.get("message").and_then(Value::as_str).map(str::to_string),
                    },
                );
            }
        }
        Err(e) => {
            for f in &list {
                d.bp.insert(f.id.clone(), BpStatus { verified: false, line: None, message: Some(e.to_string()) });
            }
        }
    }
    d.bp_dirty = true;
}

pub async fn sync_exceptions(state: &AppState, s: &Arc<Session>) {
    let caps = s.capabilities();
    let Some(filters) = caps.get("exceptionBreakpointFilters").filter(|f| f.is_array()) else { return };
    let Ok(client) = s.client() else { return };
    let pd = state.debug.store.get(&state.paths.data_dir, &s.project_id);
    let enabled = match pd.exception_filters.get(&s.adapter.id) {
        Some(e) => e.clone(),
        None => default_filters(filters),
    };
    let enabled: Vec<String> = if pd.muted { vec![] } else { enabled };
    if let Err(e) = client.request("setExceptionBreakpoints", json!({ "filters": enabled }), REQUEST_TIMEOUT).await {
        s.log("workbench", format!("exception breakpoints: {e}\n"), None);
    }
}

/// Which line breakpoints a change touched.
#[derive(Debug, Clone, Copy)]
pub enum Lines<'a> {
    None,
    File(&'a str),
    All,
}

/// After a change of the project's breakpoints: update every configured live session.
pub async fn breakpoints_changed(state: &AppState, pid: &str, lines: Lines<'_>, functions: bool, exceptions: bool) {
    let sessions: Vec<Arc<Session>> = state.debug.sessions_of(pid).into_iter().filter(|s| s.is_live() && s.data.lock().configured).collect();
    for s in &sessions {
        match lines {
            Lines::None => {}
            Lines::File(f) => sync_breakpoints(state, s, Some(f)).await,
            Lines::All => sync_breakpoints(state, s, None).await,
        }
        if functions {
            sync_functions(state, s).await;
        }
        if exceptions {
            sync_exceptions(state, s).await;
        }
        s.flush(state);
    }
    emit_breakpoints(state, pid);
}

// ---------------------------------------------------------------- the event loop

async fn event_loop(state: AppState, s: Arc<Session>, mut rx: mpsc::Receiver<Incoming>) {
    let mut lost = false;
    loop {
        let first = tokio::select! {
            m = rx.recv() => m,
            _ = s.cancel.cancelled() => None,
        };
        let Some(m) = first else { break };
        let mut closed = handle(&state, &s, m).await;
        let mut n = 0;
        while !closed && n < 256 {
            match rx.try_recv() {
                Ok(m) => closed = handle(&state, &s, m).await,
                Err(_) => break,
            }
            n += 1;
        }
        s.flush(&state);
        if closed {
            lost = true;
            break;
        }
    }
    if lost {
        if let Some(msg) = unexpected_end(&s).await {
            s.fail(&state, msg);
        }
    }
    s.flush(&state);
    finish(&state, &s).await;
}

/// Why the adapter's stream ended, when nobody ended the session: not after the
/// debuggee exited (`exited`, `terminated`), a Stop, or Workbench ending it. The
/// adapter crashed or was killed: say so with its exit status and last words.
async fn unexpected_end(s: &Arc<Session>) -> Option<String> {
    let expected = s.terminated.load(Ordering::SeqCst)
        || s.stop_requested.load(Ordering::SeqCst)
        || s.finished.load(Ordering::SeqCst)
        || s.cancel.is_cancelled()
        || s.data.lock().exit_code.is_some()
        || !s.is_live();
    if expected {
        return None;
    }
    let status = match s.proc_.lock().await.as_mut() {
        Some(p) => p.exit_status(Duration::from_millis(800)).await,
        None => None,
    };
    // Give the stderr reader a moment to catch the last lines.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut msg = match (&status, s.plan.connect.is_some()) {
        (Some(st), _) => format!("{} exited unexpectedly ({})", s.adapter.label, describe_status(st)),
        (None, true) => format!("the connection to {} closed unexpectedly", s.adapter.label),
        (None, false) => format!("{} exited unexpectedly", s.adapter.label),
    };
    let tail: Vec<String> = s.data.lock().adapter_tail.iter().cloned().collect();
    if !tail.is_empty() {
        msg.push_str(": ");
        msg.push_str(&tail.join(" / "));
    }
    Some(msg)
}

fn describe_status(st: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (st.code(), st.signal()) {
        (Some(c), _) => format!("exit code {c}"),
        (None, Some(sig)) => match nix::sys::signal::Signal::try_from(sig) {
            Ok(n) => format!("killed by {}", n.as_str()),
            Err(_) => format!("killed by signal {sig}"),
        },
        _ => "no exit status".into(),
    }
}

/// Apply one message; `true` when the adapter is gone.
async fn handle(state: &AppState, s: &Arc<Session>, m: Incoming) -> bool {
    match m {
        Incoming::Event(e) => on_event(state, s, e),
        Incoming::Request(r) => on_reverse_request(state, s, r).await,
        Incoming::Noise(t) => {
            s.note_adapter_line(&t);
            s.log("adapter", t, None)
        }
        Incoming::Closed(reason) => {
            if let Some(r) = reason {
                s.log("workbench", format!("Connection to the debugger lost: {r}\n"), None);
            }
            return true;
        }
    }
    false
}

fn on_event(state: &AppState, s: &Arc<Session>, e: Value) {
    let body = e.get("body").cloned().unwrap_or(Value::Null);
    let str_of = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
    match e.get("event").and_then(Value::as_str).unwrap_or("") {
        "initialized" => {
            s.initialized.send_replace(true);
        }
        "stopped" => {
            let reason = str_of("reason").unwrap_or_else(|| "pause".into());
            let thread_id = body.get("threadId").and_then(Value::as_i64);
            let mut temp = None;
            let epoch;
            {
                let mut d = s.data.lock();
                let hits: Vec<String> = body
                    .get("hitBreakpointIds")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_i64).filter_map(|id| d.adapter_bp.get(&id).cloned()).collect())
                    .unwrap_or_default();
                d.state = Some(SessionState::Stopped);
                d.stopped = Some(StopInfo {
                    reason,
                    // An exception's message can quote what the program read.
                    description: str_of("description").map(|t| s.redact(&t)),
                    text: str_of("text").map(|t| truncate(s.redact(&t), 2000)),
                    thread_id: thread_id.or_else(|| d.stopped.as_ref().and_then(|x| x.thread_id)).or_else(|| d.threads.first().map(|t| t.id)),
                    all_threads_stopped: body.get("allThreadsStopped").and_then(Value::as_bool).unwrap_or(false),
                    hit_breakpoint_ids: hits,
                    during_evaluation: e.get(super::client::PENDING_TAGS).and_then(|t| t.get(0)).and_then(Value::as_str).map(|t| truncate(t.to_string(), 500)),
                    at: crate::util::now_ms(),
                });
                d.stop_epoch += 1;
                epoch = d.stop_epoch;
                if let Some(t) = d.temp_bp.take() {
                    temp = Some(t.0);
                }
                d.dirty = true;
            }
            let (st, s2) = (state.clone(), s.clone());
            tokio::spawn(async move {
                if let Some(path) = temp {
                    sync_breakpoints(&st, &s2, Some(&path)).await;
                }
                refresh_threads(&st, &s2, epoch).await;
            });
        }
        "continued" => {
            let all = body.get("allThreadsContinued").and_then(Value::as_bool).unwrap_or(true);
            let tid = body.get("threadId").and_then(Value::as_i64);
            s.update(|d| {
                let same = d.stopped.as_ref().is_some_and(|x| x.thread_id == tid);
                if d.state == Some(SessionState::Stopped) && (all || same) {
                    d.state = Some(SessionState::Running);
                    d.stopped = None;
                    d.stop_epoch += 1;
                }
            });
        }
        "exited" => {
            let code = body.get("exitCode").and_then(Value::as_i64);
            if s.stop_requested.load(Ordering::SeqCst) {
                // The code of a program the debugger killed says nothing (gdb reports 0).
                s.log("workbench", "\nProcess stopped\n", None);
                return;
            }
            s.update(|d| d.exit_code = code);
            if let Some(c) = code {
                s.log("workbench", format!("\nProcess finished with exit code {c}\n"), None);
            }
        }
        "terminated" => {
            if !s.terminated.swap(true, Ordering::SeqCst) {
                let (st, s2) = (state.clone(), s.clone());
                tokio::spawn(async move {
                    // The adapter ended the debuggee: end the session politely, after
                    // its children (a subprocess that outlives the program is
                    // detached, and keeps running).
                    stop_children(st.clone(), s2.id.clone(), false).await;
                    if let Ok(c) = s2.client() {
                        let _ = c.request("disconnect", json!({ "restart": false }), DISCONNECT_TIMEOUT).await;
                    }
                    if let Some(p) = s2.proc_.lock().await.as_mut() {
                        p.wait_exit(Duration::from_millis(1500)).await;
                    }
                    finish(&st, &s2).await;
                });
            }
        }
        "thread" => {
            let Some(id) = body.get("threadId").and_then(Value::as_i64) else { return };
            let reason = str_of("reason").unwrap_or_default();
            s.update(|d| {
                if reason == "exited" {
                    d.threads.retain(|t| t.id != id);
                } else if !d.threads.iter().any(|t| t.id == id) && d.threads.len() < 10_000 {
                    d.threads.push(ThreadView { id, name: format!("Thread {id}") });
                }
            });
        }
        "output" => {
            let category = str_of("category").unwrap_or_else(|| "console".into());
            if category == "telemetry" {
                return;
            }
            let Some(text) = str_of("output") else { return };
            let source = body.get("source").and_then(|src| {
                let v = s.paths.source_view(src);
                let path = v.get("path").and_then(Value::as_str)?.to_string();
                if v.get("inProject") == Some(&Value::Bool(false)) {
                    s.note_source(&path);
                }
                Some((path, body.get("line").and_then(Value::as_i64).unwrap_or(0)))
            });
            s.log(&category, text, source);
        }
        "breakpoint" => {
            let bp = body.get("breakpoint").cloned().unwrap_or(Value::Null);
            let Some(aid) = bp.get("id").and_then(Value::as_i64) else { return };
            let reason = str_of("reason").unwrap_or_default();
            let mut d = s.data.lock();
            let Some(ours) = d.adapter_bp.get(&aid).cloned() else { return };
            if reason == "removed" {
                d.bp.remove(&ours);
                d.adapter_bp.remove(&aid);
            } else {
                let prev = d.bp.get(&ours).cloned();
                d.bp.insert(
                    ours,
                    BpStatus {
                        verified: bp.get("verified").and_then(Value::as_bool).unwrap_or(false),
                        line: bp.get("line").and_then(Value::as_i64).or(prev.as_ref().and_then(|p| p.line)),
                        message: bp.get("message").and_then(Value::as_str).map(|m| truncate(m.to_string(), 500)),
                    },
                );
            }
            d.bp_dirty = true;
        }
        "process" => {
            let name = str_of("name").unwrap_or_default();
            let pid = body.get("systemProcessId").and_then(Value::as_i64);
            let method = str_of("startMethod").unwrap_or_else(|| "launch".into());
            let local = body.get("isLocalProcess").and_then(Value::as_bool).unwrap_or(true);
            let ours = s.request == DebugRequest::Launch && method != "attach" && method != "attachForSuspendedLaunch" && local && s.plan.target.is_none();
            s.update(|d| {
                d.process = Some(ProcessView { pid, name: truncate(name, 300) });
                if ours {
                    d.debuggee_pid = pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 1);
                }
            });
        }
        "capabilities" => {
            if let Some(c) = body.get("capabilities").and_then(Value::as_object) {
                s.update(|d| {
                    if let Some(m) = d.capabilities.as_object_mut() {
                        for (k, v) in c {
                            m.insert(k.clone(), v.clone());
                        }
                    }
                });
            }
        }
        "invalidated" => s.update(|d| d.stop_epoch += 1),
        _ => {}
    }
}

async fn refresh_threads(state: &AppState, s: &Arc<Session>, epoch: u64) {
    let Ok(body) = s.request("threads", Value::Null, REQUEST_TIMEOUT).await else { return };
    let list: Vec<ThreadView> = body
        .get("threads")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(10_000)
                .filter_map(|t| Some(ThreadView { id: t.get("id")?.as_i64()?, name: truncate(s.redact(t.get("name").and_then(Value::as_str).unwrap_or("")), 200) }))
                .collect()
        })
        .unwrap_or_default();
    s.update(|d| {
        d.threads = list;
        // A stop without a thread id: take the first thread.
        if d.stop_epoch == epoch {
            if let Some(st) = d.stopped.as_mut() {
                if st.thread_id.is_none() {
                    st.thread_id = d.threads.first().map(|t| t.id);
                }
            }
        }
    });
    s.flush(state);
}

async fn on_reverse_request(state: &AppState, s: &Arc<Session>, r: Value) {
    let Ok(client) = s.client() else { return };
    let args = r.get("arguments").cloned().unwrap_or(Value::Null);
    match r.get("command").and_then(Value::as_str).unwrap_or("") {
        "runInTerminal" => match run_in_terminal_request(state, s, &args).await {
            Ok(id) => {
                s.log("workbench", "The program runs in its own terminal.\n", None);
                s.update(|d| d.debuggee_terminals.push(id));
                client.respond(&r, true, json!({}), None);
            }
            Err(e) => client.respond(&r, false, Value::Null, Some(&e)),
        },
        "startDebugging" => {
            let configuration = args.get("configuration").cloned().unwrap_or_else(|| json!({}));
            let request = if args.get("request").and_then(Value::as_str) == Some("attach") { DebugRequest::Attach } else { DebugRequest::Launch };
            let Some(project) = state.projects.get(&s.project_id) else {
                client.respond(&r, false, Value::Null, Some("the project is gone"));
                return;
            };
            // Where the child's debugger is: debugpy names its own listener
            // (`configuration.connect`), a TCP adapter serves every session on its
            // port; otherwise (a stdio adapter) the child gets an adapter of its own.
            let connect = match child_connection(&configuration) {
                Some(c) => Some(c),
                None if s.adapter.transport == super::adapters::Transport::Tcp => s.data.lock().adapter_port.map(|p| ("127.0.0.1".to_string(), p)),
                None => None,
            };
            if let Some((host, port)) = &connect {
                let refuse = if s.plan.target.is_some() {
                    Some("child sessions of a debugger in a dev container are not supported yet".to_string())
                } else if process::loopback_host(host).is_none() {
                    Some(format!("the child session's debugger is at {host}:{port}, not on this machine's loopback interface"))
                } else {
                    None
                };
                if let Some(why) = refuse {
                    s.log("workbench", format!("A child debug session was not started: {why}.\n"), None);
                    client.respond(&r, false, Value::Null, Some(&why));
                    return;
                }
            }
            let mut plan = s.plan.clone();
            plan.name = configuration.get("name").and_then(Value::as_str).map(|n| truncate(n.to_string(), 100)).unwrap_or_else(|| format!("{} (child)", s.name));
            plan.config = None;
            plan.pre = None;
            plan.request = request;
            plan.pid = None;
            plan.raw_arguments = Some(configuration);
            plan.connect = connect;
            client.respond(&r, true, json!({}), None);
            // Started by `children` (a task of `debug::start`): a session cannot
            // start another from inside its own event loop.
            let sent = state.debug.children.get().map(|tx| tx.send(ChildRequest { project, plan, parent: s.id.clone() }).is_ok());
            if sent != Some(true) {
                s.log("workbench", "A child debug session could not be started.\n", None);
            }
        }
        other => client.respond(&r, false, Value::Null, Some(&format!("Workbench does not support the {other} request"))),
    }
}

/// debugpy's `startDebugging` configuration names the adapter's listener for the
/// subprocess: `connect: {host, port}`.
fn child_connection(configuration: &Value) -> Option<(String, u16)> {
    let c = configuration.get("connect")?;
    let port = c.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0)?;
    let host = c.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1").to_string();
    Some((host, port))
}

/// `runInTerminal`: the debuggee in a Workbench terminal, so it has a real TTY.
async fn run_in_terminal_request(state: &AppState, s: &Arc<Session>, args: &Value) -> Result<String, String> {
    let argv: Vec<String> = args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if argv.is_empty() {
        return Err("runInTerminal without a command".into());
    }
    let cwd = args.get("cwd").and_then(Value::as_str).map(|c| s.paths.host_of(c)).filter(|p| p.is_dir()).unwrap_or_else(|| s.paths.root.clone());
    let env: Vec<(String, Option<String>)> = args
        .get("env")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter(|(k, _)| !k.is_empty() && !k.contains('=')).map(|(k, v)| (k.clone(), v.as_str().map(str::to_string))).collect())
        .unwrap_or_default();
    let title = args.get("title").and_then(Value::as_str).map(|t| truncate(t.to_string(), 80)).unwrap_or_else(|| s.name.clone());
    let mut meta = json!({ "debug": s.id, "debuggee": true });
    if s.plan.target.is_some() {
        meta["inContainer"] = json!(true);
    }
    let spec = SpawnSpec {
        kind: TerminalKind::Command,
        title: format!("Debug: {title}"),
        project_id: Some(s.project_id.clone()),
        cwd,
        argv,
        env,
        cols: None,
        rows: None,
        meta,
    };
    let info = state.terminals.spawn_redacted(state, spec, s.redact.clone()).await.map_err(|e| e.message)?;
    Ok(info.id)
}

// ---------------------------------------------------------------- control

/// Stepping and resuming. The state turns `running` before the request goes out, so
/// a `stopped` event that overtakes the response is not overwritten.
pub async fn control(state: &AppState, s: &Arc<Session>, action: &str, thread_id: Option<i64>) -> Result<(), ApiError> {
    let command = match action {
        "continue" => "continue",
        "pause" => "pause",
        "next" | "stepOver" => "next",
        "stepIn" | "stepInto" => "stepIn",
        "stepOut" => "stepOut",
        _ => return Err(ApiError::bad_request(format!("unknown action {action:?}"))),
    };
    let client = s.client()?;
    let (cur, stopped, tid) = {
        let d = s.data.lock();
        let tid = thread_id.or_else(|| d.stopped.as_ref().and_then(|x| x.thread_id)).or_else(|| d.threads.first().map(|t| t.id));
        (d.state.unwrap_or(SessionState::Starting), d.stopped.clone(), tid)
    };
    match (command, cur) {
        ("pause", SessionState::Running) => {}
        ("pause", _) => return Err(ApiError::conflict("the program is not running")),
        (_, SessionState::Stopped) => {}
        _ => return Err(ApiError::conflict("the program is not suspended")),
    }
    let tid = tid.unwrap_or(0);
    if command != "pause" {
        s.update(|d| {
            d.state = Some(SessionState::Running);
            d.stopped = None;
            d.stop_epoch += 1;
        });
        s.flush(state);
    }
    let r = client.request(command, json!({ "threadId": tid }), REQUEST_TIMEOUT).await;
    if let Err(e) = r {
        if command != "pause" {
            s.update(|d| {
                if d.state == Some(SessionState::Running) {
                    d.state = Some(SessionState::Stopped);
                    d.stopped = stopped;
                    d.stop_epoch += 1;
                }
            });
            s.flush(state);
        }
        return Err(e.into());
    }
    Ok(())
}

/// Run to a line: a breakpoint only this session has, removed at the next stop.
pub async fn run_to(state: &AppState, s: &Arc<Session>, path: &str, line: i64, thread_id: Option<i64>) -> Result<(), ApiError> {
    let path = breakpoints::normalize_path(path)?;
    if s.state() != SessionState::Stopped {
        return Err(ApiError::conflict("the program is not suspended"));
    }
    s.update(|d| d.temp_bp = Some((path.clone(), line)));
    sync_breakpoints(state, s, Some(&path)).await;
    control(state, s, "continue", thread_id).await
}

/// Stop the session: terminate (or detach from) the debuggee, then end the adapter.
pub async fn stop(state: &AppState, s: &Arc<Session>) {
    // A child session of a program Workbench launched ends that process too.
    stop_as(state, s, s.plan.launched).await
}

async fn stop_as(state: &AppState, s: &Arc<Session>, terminate_debuggee: bool) {
    if s.finished.load(Ordering::SeqCst) {
        finish(state, s).await;
        return;
    }
    if s.is_live() {
        s.stop_requested.store(true, Ordering::SeqCst);
        s.update(|_| {});
    }
    // Child sessions first: they talk to this session's adapter.
    stop_children(state.clone(), s.id.clone(), terminate_debuggee).await;
    if let Ok(c) = s.client() {
        let launch = s.request == DebugRequest::Launch;
        if launch && s.cap("supportsTerminateRequest") && !s.terminated.load(Ordering::SeqCst) {
            let _ = c.request("terminate", json!({}), Duration::from_secs(2)).await;
            for _ in 0..15 {
                if s.terminated.load(Ordering::SeqCst) || c.is_closed() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        if !c.is_closed() {
            let _ = c.request("disconnect", json!({ "restart": false, "terminateDebuggee": terminate_debuggee }), DISCONNECT_TIMEOUT).await;
        }
    }
    finish(state, s).await;
}

/// Stop the live child sessions of session `parent` (they talk to its adapter, which
/// is about to go): terminating their processes, or detaching (the parent program
/// ended by itself; a subprocess that outlives it keeps running). Boxed: `stop`
/// ends in `finish`, which stops children too.
fn stop_children(state: AppState, parent: String, terminate_debuggee: bool) -> futures::future::BoxFuture<'static, ()> {
    Box::pin(async move {
        let children: Vec<Arc<Session>> = state.debug.all().into_iter().filter(|c| c.parent.as_deref() == Some(parent.as_str()) && c.is_live()).collect();
        if children.is_empty() {
            return;
        }
        let stops = children.iter().map(|c| stop_as(&state, c, terminate_debuggee));
        let _ = tokio::time::timeout(Duration::from_secs(8), futures::future::join_all(stops)).await;
    })
}

fn parent_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// End the session (idempotent): the adapter, the debuggee Workbench launched, the
/// debuggee's terminal, a pre-launch step still running.
pub async fn finish(state: &AppState, s: &Arc<Session>) {
    if s.finished.swap(true, Ordering::SeqCst) {
        // Another caller is ending it: return once it has.
        let mut rx = s.ended.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|v| *v)).await;
        return;
    }
    s.cancel.cancel();
    // Children still running (the adapter is about to go): detached.
    stop_children(state.clone(), s.id.clone(), false).await;
    let (debuggee, terminals, prelaunch, temp_files) = {
        let mut d = s.data.lock();
        (d.debuggee_pid.take(), std::mem::take(&mut d.debuggee_terminals), d.prelaunch_terminal.clone(), std::mem::take(&mut d.temp_files))
    };
    // Dropping our client closes the adapter's stdin (EOF ends stdio adapters).
    let client = s.client.lock().take();
    drop(client);
    if let Some(mut p) = s.proc_.lock().await.take() {
        // A debuggee we launched that is still the adapter's child: gone with it.
        if let (Some(dpid), Some(apid)) = (debuggee, p.child.id()) {
            if parent_of(dpid) == Some(apid as i32) {
                let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(dpid), nix::sys::signal::Signal::SIGKILL);
            }
        }
        p.wait_exit(Duration::from_millis(500)).await;
        p.kill().await;
    }
    for t in terminals {
        if state.terminals.info(&t).is_some_and(|i| i.status != crate::terminals::TerminalStatus::Exited) {
            let _ = state.terminals.kill(&t).await;
        }
    }
    if let Some(t) = prelaunch {
        // A run configuration's terminal belongs to the apps slice: leave it.
        let ours = state.terminals.info(&t).is_some_and(|i| i.meta.get("debug").and_then(Value::as_str) == Some(s.id.as_str()));
        if ours && state.terminals.info(&t).is_some_and(|i| i.status != crate::terminals::TerminalStatus::Exited) {
            let _ = state.terminals.kill(&t).await;
        }
    }
    for f in temp_files {
        let _ = std::fs::remove_file(f);
    }
    s.update(|d| {
        if d.state != Some(SessionState::Failed) {
            d.state = Some(SessionState::Terminated);
        }
        d.phase = None;
        d.stopped = None;
        d.ended_at = Some(crate::util::now_ms());
        d.bp.clear();
        d.bp_dirty = true;
    });
    s.log("workbench", "Debug session ended.\n", None);
    s.flush(state);
    s.ended.send_replace(true);
    for id in state.debug.prune(&s.project_id, KEEP_ENDED) {
        state.events.emit("debug.session", Some(&s.project_id), json!({ "id": id, "projectId": s.project_id, "removed": true }));
    }
}

/// End every live session (Workbench is shutting down).
pub async fn shutdown(state: &AppState) {
    state.debug.shutdown.cancel();
    let live: Vec<Arc<Session>> = state.debug.all().into_iter().filter(|s| s.is_live()).collect();
    let all = futures::future::join_all(live.iter().map(|s| stop(state, s)));
    let _ = tokio::time::timeout(Duration::from_secs(8), all).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_paths_map_both_ways() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("app");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let m = PathMap { root: root.clone(), canon_root: root.canonicalize().unwrap(), container: Some((root.clone(), "/workspaces/app".into())) };
        assert_eq!(m.to_adapter(&root.join("src/main.c")), "/workspaces/app/src/main.c");
        assert_eq!(m.to_adapter(Path::new("/usr/include/stdio.h")), "/usr/include/stdio.h");
        assert_eq!(m.host_of("/workspaces/app/src/main.c"), root.join("src/main.c"));
        assert_eq!(m.host_of("/workspaces/app2/x.c"), PathBuf::from("/workspaces/app2/x.c"), "component-wise prefixes only");
        let v = m.source_view(&json!({"name": "main.c", "path": "/workspaces/app/src/main.c"}));
        assert_eq!((v["path"].as_str(), v["inProject"].as_bool()), (Some("src/main.c"), Some(true)));
        let v = m.source_view(&json!({"name": "stdio.h", "path": "/usr/include/stdio.h"}));
        assert_eq!(v["inProject"], false);
        let v = m.source_view(&json!({"name": "<generated>", "sourceReference": 7}));
        assert_eq!(v["sourceReference"], 7);
        // A symlinked view of the project still maps into it.
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let host = PathMap::new(&root, None);
        assert_eq!(host.project_rel(&link.join("src")), Some("src".into()));
    }

    #[test]
    fn capabilities_for_the_ui_are_a_subset() {
        let c = ui_capabilities(&json!({"supportsLogPoints": true, "supportsReadMemoryRequest": true, "exceptionBreakpointFilters": []}));
        assert_eq!(c, json!({"supportsLogPoints": true, "exceptionBreakpointFilters": []}));
        assert_eq!(default_filters(&json!([{"filter": "a", "default": true}, {"filter": "b"}])), vec!["a"]);
    }
}

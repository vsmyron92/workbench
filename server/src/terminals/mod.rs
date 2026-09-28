//! Terminals & agents slice (OWNER: terminals slice).
//!
//! PTY-backed terminals (agent sessions of Claude Code, Codex, Kimi, Gemini, Aider or custom CLIs,
//! shells, run configurations, one-off commands), their server-side screen mirror and
//! snapshots, the terminal WebSocket, Claude hooks/status line intake, session history
//! and resume.
//!
//! CONTRACT — other slices call these; keep the signatures:
//! `Terminals::{spawn, spawn_redacted, respawn_redacted, info, list, kill, write, send_text, subscribe_output, exit_watch, screen_text}`,
//! the `SpawnSpec`/`TerminalInfo`/`AgentInfo`/`ExitInfo` types, `start`, `shutdown`,
//! `router`, `mcp_tools`, `cli_statusline`. Routes: `/api/terminals/**`, `/api/agents/**`,
//! `/api/hooks/**` (see docs/ARCHITECTURE.md).
//!
//! Restarting (`POST /api/terminals/{id}/restart`): agents resume, shells get a new shell,
//! and a run/command terminal re-runs only when that is safe without its owner. Run
//! configurations are restarted by apps (it tracks them); deploys and env commands are
//! started again from their environment, so their gates and confirmations run again.
//! Anything else another slice spawns is refused unless its `meta` says
//! `"restartable": true` (log follows and Remote Control servers are known to be safe).
//!
//! Layout of the module:
//! * `pty` — one process + the vt100 mirror, snapshots, device-query answers, redaction,
//!   session kill;
//! * `viewers` — which attached view decides the PTY size;
//! * `store` — persistence under `data_dir/terminals/<id>/`;
//! * `agent` — launching agent sessions of every provider (Claude Code settings, MCP
//!   config, resume/fork; Codex and Kimi command lines), the per-provider watchers
//!   (transcript tail, rollout tail, output activity), `ask`, Remote Control servers,
//!   history and live sessions;
//! * `providers` — the configured agent CLIs and their command lines;
//! * `hooks` — the Claude hook/status line state machine;
//! * `permission` — Claude permission requests answered from Workbench (the held hook);
//! * `transcript` — Claude's files on disk; `codex`, `kimi`, `gemini` — theirs;
//! * `activity` — the output-activity heuristic for CLIs that report nothing;
//! * `routes` — REST + WebSocket; `input` — sanitising and attachments;
//! * `statusline` — the `workbench statusline` helper; `tools` — MCP tools.

mod activity;
mod agent;
mod codex;
mod gemini;
mod hooks;
mod input;
mod kimi;
mod permission;
mod providers;
mod pty;
mod routes;
mod statusline;
mod store;
mod tools;
mod transcript;
mod viewers;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, watch};

use crate::app::AppState;
use crate::error::ApiError;
use crate::events::EventBus;
use crate::secrets::Secret;
use crate::util;

pub use permission::{PendingPermission, wait_secs as permission_wait_secs};
pub use providers::ProviderKind;
pub use routes::router;

/// The executable names of the enabled agent providers (for the dev container probe:
/// which of them exist inside a container).
pub fn provider_commands(cfg: &crate::config::global::AgentsConfig) -> Vec<String> {
    providers::list(cfg)
        .0
        .into_iter()
        .filter(|p| p.enabled)
        .map(|p| Path::new(&p.command).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(p.command))
        .filter(|c| !c.is_empty() && c.chars().all(|x| x.is_ascii_alphanumeric() || "._-+".contains(x)))
        .collect()
}

/// Whether a terminal runs inside its project's dev container (`meta.inContainer`).
pub fn in_container(info: &TerminalInfo) -> bool {
    info.meta.get("inContainer") == Some(&Value::Bool(true))
}

/// `(docker, container)` of a terminal whose process runs in a dev container.
fn container_exec(info: &TerminalInfo) -> Option<(String, String)> {
    if !in_container(info) {
        return None;
    }
    let c = info.meta.get("container")?;
    let id = c.get("id")?.as_str()?.to_string();
    let docker = c.get("docker").and_then(Value::as_str).unwrap_or("docker").to_string();
    Some((docker, id))
}
pub use statusline::cli_statusline;
pub use tools::mcp_tools;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    /// An agent CLI session (Claude Code, Codex, Kimi, Gemini, Aider or a custom CLI).
    Agent,
    /// An interactive login shell.
    Shell,
    /// A project run configuration (owned by the apps slice).
    Run,
    /// A one-off command: deploy, remote logs, git operations…
    Command,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Starting,
    Running,
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Starting,
    Idle,
    Working,
    NeedsPermission,
    NeedsInput,
    Error,
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    /// The CLI's conversation id: Claude Code's UUID (known before launch via
    /// `--session-id`), Codex's thread id or Kimi's `session_…` id once discovered.
    /// Empty while unknown (and always for custom CLIs).
    pub session_id: String,
    /// What kind of CLI runs the session. Records from before providers are Claude.
    #[serde(default)]
    pub provider: ProviderKind,
    /// The configured provider (`[agents.providers.<id>]`: `claude`, `codex`, `kimi`,
    /// `aider`…). `None` in records from before providers: `claude`.
    #[serde(default)]
    pub provider_id: Option<String>,
    pub state: AgentState,
    /// A finished turn the user has not looked at yet.
    pub unread: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub remote_control: bool,
    /// claude.ai URL of the Remote Control bridge, once known.
    pub remote_url: Option<String>,
    /// Auto title (transcript `ai-title`) or the user's name for the session.
    pub title: Option<String>,
    /// Tail of the last assistant message.
    pub last_message: Option<String>,
    /// Why the session wants attention (permission prompt text, error…).
    pub attention: Option<String>,
    pub context_pct: Option<f64>,
    pub cost_usd: Option<f64>,
    pub last_event_at: i64,
    /// The Claude Code permission request the session waits on, when Workbench can
    /// answer it (`POST /api/agents/{id}/permission`); `null` otherwise.
    #[serde(default)]
    pub pending_permission: Option<PendingPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub kind: TerminalKind,
    pub title: String,
    pub project_id: Option<String>,
    pub cwd: String,
    /// The command, for display. Never contains secrets.
    pub argv: Vec<String>,
    pub status: TerminalStatus,
    pub exit: Option<ExitInfo>,
    pub created_at: i64,
    pub last_output_at: i64,
    pub cols: u16,
    pub rows: u16,
    /// Shown as a tab (closed terminals stay in history).
    pub open: bool,
    pub pinned: bool,
    pub color: Option<String>,
    pub order: i64,
    pub agent: Option<AgentInfo>,
    /// Slice-specific metadata, e.g. `{"run": "api"}` or `{"env": "production", "action": "deploy"}`.
    /// `"restartable": true` lets the terminals UI re-run a command another slice spawned.
    pub meta: Value,
    /// Processes still running in the terminal's session after its main process exited
    /// (background jobs). Kill, close and restart end them.
    #[serde(default)]
    pub lingering: u32,
}

/// A request to start a non-agent terminal (agents start through `/api/agents`).
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub kind: TerminalKind,
    pub title: String,
    pub project_id: Option<String>,
    pub cwd: PathBuf,
    /// Program and arguments, e.g. `["bash", "-lc", "cargo run"]`.
    pub argv: Vec<String>,
    /// Extra environment. `None` removes the variable.
    pub env: Vec<(String, Option<String>)>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub meta: Value,
}

// ---------------------------------------------------------------- internals

pub(crate) const AGENT_SCROLLBACK: usize = 5000;
pub(crate) const OTHER_SCROLLBACK: usize = 3000;
const KILL_GRACE: Duration = Duration::from_secs(3);
/// Closed terminals kept in history (oldest are forgotten first; pinned ones are kept).
const MAX_CLOSED: usize = 150;
/// Screens are saved at most this often per terminal while output flows.
const SCREEN_SAVE_EVERY: Duration = Duration::from_secs(10);
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 32;

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// One terminal: its persisted record, the mirror, and the live process if any.
pub(crate) struct Entry {
    pub id: String,
    pub screen: Arc<pty::Screen>,
    pub rec: Mutex<store::Record>,
    pub pty: Mutex<Option<Arc<pty::Pty>>>,
    pub exit_tx: watch::Sender<Option<ExitInfo>>,
    pub meta_dirty: AtomicBool,
    screen_saved_at: Mutex<Option<Instant>>,
    /// Serializes programmatic sends so pastes and their Enter never interleave.
    pub input_lock: tokio::sync::Mutex<()>,
    /// Serializes start/stop/restart.
    pub lifecycle: tokio::sync::Mutex<()>,
    /// How a run/command terminal was started (in memory only: its env may hold secrets).
    pub spec: Mutex<Option<SpawnSpec>>,
    /// Secret values its processes may print (in memory only), masked in the output.
    pub redact: Mutex<Vec<Secret>>,
    /// Sessions of exited processes that still have members: `(sid, members)`.
    pub lingering: Mutex<Vec<(i32, u32)>>,
    pub rt: Mutex<hooks::AgentRt>,
    /// Signalled by the SessionStart hook (restores wait for it before starting the next).
    pub started: Notify,
    /// When the user last typed into the terminal (ms), so programmatic sends can wait.
    pub last_typed_at: std::sync::atomic::AtomicI64,
    /// When the PTY was last resized (ms): the repaint that follows is not activity.
    pub last_resized_at: std::sync::atomic::AtomicI64,
}

impl Entry {
    pub fn info(&self) -> TerminalInfo {
        let rec = self.rec.lock();
        self.info_locked(&rec)
    }

    fn info_locked(&self, rec: &store::Record) -> TerminalInfo {
        let mut i = rec.info.clone();
        i.last_output_at = i.last_output_at.max(self.screen.last_output_at.load(Ordering::Relaxed));
        i
    }

    pub fn running_pty(&self) -> Option<Arc<pty::Pty>> {
        self.pty.lock().clone()
    }

    pub fn kind(&self) -> TerminalKind {
        self.rec.lock().info.kind
    }

    /// The agent CLI kind of an agent terminal (`None` for other terminals).
    pub fn provider(&self) -> Option<ProviderKind> {
        self.rec.lock().info.agent.as_ref().map(|a| a.provider)
    }

    /// Record keyboard input from a client. Escape sequences (arrows, focus and mouse
    /// reports, replies to device queries) do not count as typing.
    pub fn note_input(&self, data: &[u8]) {
        if is_typing(data) {
            self.last_typed_at.store(util::now_ms(), Ordering::Relaxed);
        }
    }

    /// Claude Code: a permission request is pending and a key is about to reach the
    /// session. Answering in the terminal takes a key, and the dialog is on screen at that
    /// moment: note it, so its disappearance counts as an answer even when it came before
    /// the next periodic look (`permission::Queue::note_visible`).
    pub fn look_for_permission_dialog(&self) {
        if self.rt.lock().permissions.is_empty() {
            return;
        }
        let visible = permission::dialog_visible(&pty::visible_text(&mut self.screen.mirror(), permission::DIALOG_ROWS));
        if visible {
            self.rt.lock().permissions.note_visible();
        }
    }

    /// Wait (up to `max`) until the user has not typed for `quiet`. False if still typing.
    pub async fn wait_typing_pause(&self, quiet: Duration, max: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + max;
        loop {
            let since = util::now_ms() - self.last_typed_at.load(Ordering::Relaxed);
            if since >= quiet.as_millis() as i64 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// Whether client input is the user typing (not an escape sequence the terminal sent).
pub(crate) fn is_typing(data: &[u8]) -> bool {
    !data.is_empty() && data[0] != 0x1b
}

/// Whether client input comes from the user (keys, including arrows and Esc, and mouse
/// reports) rather than from the terminal itself: focus reports, and replies to device,
/// status, mode and OSC/DCS queries. The user's view then takes the PTY size.
pub(crate) fn is_user_input(data: &[u8]) -> bool {
    let Some((&first, rest)) = data.split_first() else { return false };
    if first != 0x1b || rest.is_empty() {
        return true;
    }
    match rest[0] {
        // OSC and DCS strings are replies (ESC ] / ESC P alone are Alt+] / Alt+P).
        b']' | b'P' => rest.len() == 1,
        b'[' => {
            let body = &rest[1..];
            if body == b"I" || body == b"O" {
                return false;
            }
            let Some((&last, params)) = body.split_last() else { return true };
            // DA1/DA2 (`?…c`, `>…c`), cursor position (`r;cR`), status (`0n`), mode
            // (`?…$y`) and window (`…t`) reports: parameters and one final byte.
            let reply_final = matches!(last, b'c' | b'R' | b'n' | b'y' | b't');
            let reply_params = params.iter().all(|b| b.is_ascii_digit() || b";?>$".contains(b));
            !(reply_final && reply_params && !params.is_empty())
        }
        _ => true,
    }
}

struct Ctx {
    events: EventBus,
    /// `data_dir/terminals`
    root: PathBuf,
    /// `data_dir/attachments`
    attachments: PathBuf,
}

/// Registry of live and recent terminals.
#[derive(Default)]
pub struct Terminals {
    entries: RwLock<HashMap<String, Arc<Entry>>>,
    ctx: OnceLock<Ctx>,
    shutting_down: AtomicBool,
    history: Mutex<transcript::HistoryCache>,
    codex_history: Mutex<codex::HistoryCache>,
    /// Optional Codex flags per executable, from its `--help` (keyed by path and mtime).
    codex_features: Mutex<HashMap<PathBuf, (std::time::SystemTime, providers::CodexFeatures)>>,
}

enum Write {
    Meta(store::Record),
    Screen(String, Vec<u8>),
}

impl Terminals {
    /// Start a PTY running `spec.argv`. Emits `terminal.created`.
    pub async fn spawn(&self, state: &AppState, spec: SpawnSpec) -> Result<TerminalInfo, ApiError> {
        self.spawn_redacted(state, spec, vec![]).await
    }

    /// `spawn` for a process whose environment holds secret values (`${secret:…}`): they
    /// are masked (••••••) before the output reaches the screen, so no client, saved
    /// screen, `screen_text` or MCP tool ever sees them. Values shorter than 8 bytes are
    /// not masked.
    pub async fn spawn_redacted(&self, state: &AppState, spec: SpawnSpec, secrets: Vec<Secret>) -> Result<TerminalInfo, ApiError> {
        if spec.kind == TerminalKind::Agent {
            return Err(ApiError::bad_request("agent sessions start through /api/agents"));
        }
        if spec.argv.is_empty() || spec.argv[0].is_empty() {
            return Err(ApiError::bad_request("empty command"));
        }
        if !spec.cwd.is_dir() {
            return Err(ApiError::bad_request(format!("working directory {} does not exist", spec.cwd.display())));
        }
        let (cols, rows) = pty::clamp_size(spec.cols.unwrap_or(DEFAULT_COLS), spec.rows.unwrap_or(DEFAULT_ROWS));
        let id = new_id();
        let info = TerminalInfo {
            id: id.clone(),
            kind: spec.kind,
            title: transcript::clean_line(&spec.title, 120),
            project_id: spec.project_id.clone(),
            cwd: spec.cwd.display().to_string(),
            argv: spec.argv.iter().map(|a| transcript::truncate_chars(a, 400)).collect(),
            status: TerminalStatus::Starting,
            exit: None,
            created_at: util::now_ms(),
            last_output_at: 0,
            cols,
            rows,
            open: true,
            pinned: false,
            color: None,
            order: self.next_order(),
            agent: None,
            meta: if spec.meta.is_null() { json!({}) } else { spec.meta.clone() },
            lingering: 0,
        };
        let rec = store::Record { info, launch: None, title_locked: true, transcript_path: None, was_running: false, aider_history: None };
        let entry = self.insert(rec, OTHER_SCROLLBACK);
        *entry.spec.lock() = Some(spec.clone());
        *entry.redact.lock() = secrets;
        self.emit("terminal.created", &entry);
        let mut env = base_env(state, &id);
        env.extend(spec.env.iter().cloned());
        let launch = pty::LaunchSpec { argv: spec.argv.clone(), cwd: spec.cwd.clone(), env, cols, rows, redact: vec![] };
        {
            let _l = entry.lifecycle.lock().await;
            self.launch(state, &entry, launch, true).await?;
        }
        // Every start of a run configuration or env command makes a terminal.
        self.prune_groups().await;
        Ok(entry.info())
    }

    /// Run `spec` in an existing run/command terminal whose process has exited: same
    /// id and tab, new output below a `restarted` separator. Lets a run configuration
    /// keep one terminal across starts. Emits `terminal.updated`.
    /// Secret values in `secrets` are masked at the source, as in `spawn_redacted`.
    pub async fn respawn_redacted(&self, state: &AppState, id: &str, spec: SpawnSpec, secrets: Vec<Secret>) -> Result<TerminalInfo, ApiError> {
        let entry = self.require(id)?;
        if !matches!(spec.kind, TerminalKind::Run | TerminalKind::Command) || entry.kind() != spec.kind {
            return Err(ApiError::bad_request("only run and command terminals can be respawned, as the same kind"));
        }
        if spec.argv.is_empty() || spec.argv[0].is_empty() {
            return Err(ApiError::bad_request("empty command"));
        }
        if !spec.cwd.is_dir() {
            return Err(ApiError::bad_request(format!("working directory {} does not exist", spec.cwd.display())));
        }
        let _l = entry.lifecycle.lock().await;
        if entry.running_pty().is_some() {
            return Err(ApiError::conflict("the terminal is still running"));
        }
        *entry.spec.lock() = Some(spec.clone());
        *entry.redact.lock() = secrets;
        self.update(&entry, |r| {
            r.info.title = transcript::clean_line(&spec.title, 120);
            r.info.project_id = spec.project_id.clone();
            r.info.cwd = spec.cwd.display().to_string();
            r.info.argv = spec.argv.iter().map(|a| transcript::truncate_chars(a, 400)).collect();
            r.info.meta = if spec.meta.is_null() { json!({}) } else { spec.meta.clone() };
            r.info.open = true;
            true
        });
        let mut env = base_env(state, &entry.id);
        env.extend(spec.env.iter().cloned());
        let launch = pty::LaunchSpec { argv: spec.argv, cwd: spec.cwd, env, cols: 0, rows: 0, redact: vec![] };
        self.launch(state, &entry, launch, false).await?;
        Ok(entry.info())
    }

    pub fn info(&self, id: &str) -> Option<TerminalInfo> {
        self.get(id).map(|e| e.info())
    }

    pub fn list(&self) -> Vec<TerminalInfo> {
        let mut v: Vec<TerminalInfo> = self.entries.read().values().map(|e| e.info()).collect();
        v.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(a.order.cmp(&b.order)).then(a.created_at.cmp(&b.created_at)));
        v
    }

    /// Terminate the whole process session (SIGHUP, then SIGKILL after a grace period),
    /// and whatever earlier processes left running in theirs.
    pub async fn kill(&self, id: &str) -> Result<(), ApiError> {
        let entry = self.require(id)?;
        let _l = entry.lifecycle.lock().await;
        self.stop_process(&entry).await;
        Ok(())
    }

    /// Raw bytes to the PTY.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), ApiError> {
        let entry = self.require(id)?;
        let pty = entry.running_pty().ok_or_else(|| ApiError::conflict("the terminal is not running"))?;
        entry.look_for_permission_dialog();
        pty.write(Bytes::copy_from_slice(data)).map_err(ApiError::conflict)
    }

    /// Paste text (bracketed when the program enabled it); press Enter afterwards if `submit`.
    pub async fn send_text(&self, id: &str, text: &str, submit: bool) -> Result<(), ApiError> {
        self.send_input(id, text, submit, true).await
    }

    /// Live output after the moment of subscription.
    pub fn subscribe_output(&self, id: &str) -> Option<broadcast::Receiver<Bytes>> {
        self.get(id).map(|e| e.screen.out_tx.subscribe())
    }

    /// Resolves to `Some(exit)` when the process has exited.
    pub fn exit_watch(&self, id: &str) -> Option<watch::Receiver<Option<ExitInfo>>> {
        self.get(id).map(|e| e.exit_tx.subscribe())
    }

    /// Plain text of the last `max_lines` lines (scrollback + screen). Secrets the spawner
    /// declared are already masked.
    pub fn screen_text(&self, id: &str, max_lines: usize) -> Option<String> {
        let e = self.get(id)?;
        let text = pty::screen_text(&mut e.screen.mirror(), max_lines.max(1));
        self.maybe_hibernate(&e);
        Some(text)
    }

    // ------------------------------------------------------------ registry helpers

    pub(crate) fn get(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries.read().get(id).cloned()
    }

    /// Whether `entry` is still the registered terminal of its id (not forgotten).
    fn is_registered(&self, entry: &Arc<Entry>) -> bool {
        self.entries.read().get(&entry.id).is_some_and(|e| Arc::ptr_eq(e, entry))
    }

    pub(crate) fn require(&self, id: &str) -> Result<Arc<Entry>, ApiError> {
        self.get(id).ok_or_else(|| ApiError::not_found(format!("no terminal {id:?}")))
    }

    pub(crate) fn all(&self) -> Vec<Arc<Entry>> {
        self.entries.read().values().cloned().collect()
    }

    fn ctx(&self) -> Option<&Ctx> {
        self.ctx.get()
    }

    fn root(&self) -> Option<PathBuf> {
        self.ctx().map(|c| c.root.clone())
    }

    pub(crate) fn attachments_dir(&self) -> Result<PathBuf, ApiError> {
        self.ctx().map(|c| c.attachments.clone()).ok_or_else(|| ApiError::internal("terminals not started"))
    }

    fn next_order(&self) -> i64 {
        self.entries.read().values().map(|e| e.rec.lock().info.order).max().unwrap_or(0) + 1
    }

    fn insert(&self, rec: store::Record, scrollback: usize) -> Arc<Entry> {
        let (cols, rows) = pty::clamp_size(rec.info.cols, rec.info.rows);
        let exit = rec.info.exit.clone().filter(|_| rec.info.status == TerminalStatus::Exited);
        let (exit_tx, _) = watch::channel(exit);
        let entry = Arc::new(Entry {
            id: rec.info.id.clone(),
            screen: Arc::new(pty::Screen::new(rows, cols, scrollback)),
            rec: Mutex::new(rec),
            pty: Mutex::new(None),
            exit_tx,
            meta_dirty: AtomicBool::new(true),
            screen_saved_at: Mutex::new(None),
            input_lock: tokio::sync::Mutex::new(()),
            lifecycle: tokio::sync::Mutex::new(()),
            spec: Mutex::new(None),
            redact: Mutex::new(vec![]),
            lingering: Mutex::new(vec![]),
            rt: Mutex::new(hooks::AgentRt::default()),
            started: Notify::new(),
            last_typed_at: std::sync::atomic::AtomicI64::new(0),
            last_resized_at: std::sync::atomic::AtomicI64::new(0),
        });
        self.entries.write().insert(entry.id.clone(), entry.clone());
        entry
    }

    /// Emit `kind` with the terminal's current info.
    pub(crate) fn emit(&self, kind: &str, entry: &Entry) {
        let rec = entry.rec.lock();
        self.emit_locked(kind, entry, &rec);
    }

    fn emit_locked(&self, kind: &str, entry: &Entry, rec: &store::Record) {
        if let Some(ctx) = self.ctx() {
            let info = entry.info_locked(rec);
            ctx.events.emit(kind, info.project_id.as_deref(), &info);
        }
    }

    /// Mutate the record; when `f` reports a change, mark it for saving and emit
    /// `terminal.updated`. The event is built under the record lock, so events for one
    /// terminal can never arrive out of order.
    pub(crate) fn update(&self, entry: &Entry, f: impl FnOnce(&mut store::Record) -> bool) -> bool {
        self.update_as("terminal.updated", entry, f)
    }

    fn update_as(&self, kind: &str, entry: &Entry, f: impl FnOnce(&mut store::Record) -> bool) -> bool {
        let mut rec = entry.rec.lock();
        if !f(&mut rec) {
            return false;
        }
        entry.meta_dirty.store(true, Ordering::Relaxed);
        self.emit_locked(kind, entry, &rec);
        true
    }

    /// `agent.attention {terminalId, state, message, title}`.
    fn emit_attention_locked(&self, entry: &Entry, rec: &store::Record) {
        let (Some(ctx), Some(agent)) = (self.ctx(), rec.info.agent.as_ref()) else { return };
        let message = agent
            .attention
            .clone()
            .or_else(|| (agent.state == AgentState::Idle).then(|| agent.last_message.clone()).flatten())
            .unwrap_or_else(|| match agent.state {
                AgentState::Idle => "Turn complete".into(),
                AgentState::Error => "Something went wrong".into(),
                _ => "Needs your attention".into(),
            });
        ctx.events.emit(
            "agent.attention",
            rec.info.project_id.as_deref(),
            json!({
                "terminalId": entry.id,
                "state": agent.state,
                "message": message,
                "title": rec.info.title,
                "permission": agent.pending_permission,
            }),
        );
    }

    /// `agent.attention` for a permission request Workbench can answer (each one gets
    /// its own, also when it waits behind another): `permission` carries its id.
    fn emit_permission_attention_locked(&self, entry: &Entry, rec: &store::Record, p: &PendingPermission) {
        let Some(ctx) = self.ctx() else { return };
        ctx.events.emit(
            "agent.attention",
            rec.info.project_id.as_deref(),
            json!({
                "terminalId": entry.id,
                "state": AgentState::NeedsPermission,
                "message": p.summary,
                "title": rec.info.title,
                "permission": p,
            }),
        );
    }

    // ------------------------------------------------------------ lifecycle

    /// Start a process in `entry`. `fresh_screen` replaces the mirror (clients
    /// re-snapshot); otherwise the new process continues below the old output.
    pub(crate) async fn launch(
        &self,
        state: &AppState,
        entry: &Arc<Entry>,
        mut spec: pty::LaunchSpec,
        fresh_screen: bool,
    ) -> Result<(), ApiError> {
        // A terminal forgotten while this start waited for the lifecycle lock stays dead.
        if !self.is_registered(entry) {
            return Err(ApiError::not_found(format!("no terminal {:?}", entry.id)));
        }
        // A terminal that runs in its project's dev container: `docker exec` around the
        // command, resolved at every start (the container may have been recreated).
        let container_of = {
            let r = entry.rec.lock();
            in_container(&r.info).then(|| r.info.project_id.clone())
        };
        let mut container_error = None;
        if let Some(pid) = container_of {
            let target = match pid {
                Some(pid) => crate::devcontainer::running_target(state, &pid).await,
                None => Err("this terminal has no project, so no dev container".to_string()),
            };
            match target {
                Ok(t) => {
                    let (argv, env) = t.wrap(&entry.id, &spec.argv, &spec.cwd, &spec.env);
                    spec.argv = argv;
                    spec.env = env;
                    let d = t.describe();
                    self.update(entry, |r| {
                        r.info.meta["container"] = d;
                        true
                    });
                }
                Err(msg) => container_error = Some(msg),
            }
        }
        if fresh_screen {
            entry.screen.reset();
        } else {
            // Undo whatever modes the old process left behind, then a separator line.
            // `?1049l` only when the alternate screen is active: on the normal screen it
            // would restore a stale saved cursor.
            let alt = {
                let mut m = entry.screen.mirror();
                m.callbacks_mut().reset_modes();
                m.screen().alternate_screen()
            };
            let mut reset: Vec<u8> = vec![];
            if alt {
                reset.extend_from_slice(b"\x1b[?1049l");
            }
            reset.extend_from_slice(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b>\x1b[?25h\x1b[0m\r\n");
            reset.extend_from_slice("\x1b[2m── restarted ──\x1b[0m\r\n".as_bytes());
            entry.screen.feed(&reset);
        }
        // Start at the size of the most recently active view: it may have changed while
        // no process ran (an exited terminal keeps the size its last screen was drawn at).
        {
            let viewers = entry.screen.viewers.lock();
            if let Some((cols, rows)) = viewers.preferred().filter(|s| *s != entry.screen.size()) {
                entry.screen.set_size(cols, rows);
                let mut rec = entry.rec.lock();
                rec.info.cols = cols;
                rec.info.rows = rows;
            }
        }
        let (cols, rows) = entry.screen.size();
        spec.cols = cols;
        spec.rows = rows;
        spec.redact = entry.redact.lock().iter().map(|s| s.expose().as_bytes().to_vec()).collect();
        let program = spec.argv.first().cloned().unwrap_or_default();
        let proc_gen = entry.screen.next_proc_gen();
        let screen = entry.screen.clone();
        self.update(entry, |r| {
            r.info.status = TerminalStatus::Starting;
            r.info.exit = None;
            true
        });
        let spawned = match container_error {
            Some(msg) => Err(anyhow::anyhow!("{msg}")),
            None => tokio::task::spawn_blocking(move || pty::Pty::spawn(&spec, screen, proc_gen))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
                .and_then(|r| r),
        };
        match spawned {
            Ok((pty, events)) => {
                *entry.pty.lock() = Some(pty.clone());
                entry.exit_tx.send_replace(None);
                self.update(entry, |r| {
                    r.info.status = TerminalStatus::Running;
                    r.was_running = false;
                    true
                });
                let state2 = state.clone();
                let entry2 = entry.clone();
                tokio::spawn(async move {
                    let info = events.exit.await.unwrap_or(ExitInfo { code: None, signal: None, at: util::now_ms() });
                    // Let the reader drain the last output before the final screen is saved.
                    let _ = tokio::time::timeout(Duration::from_millis(800), events.reader_done).await;
                    state2.terminals.on_exit(&state2, &entry2, &pty, info).await;
                });
                Ok(())
            }
            Err(e) => {
                let msg = format!("cannot start {program}: {e:#}");
                entry.screen.feed(format!("\r\n\x1b[31m{msg}\x1b[0m\r\n").as_bytes());
                let exit = ExitInfo { code: None, signal: Some("failed to start".into()), at: util::now_ms() };
                entry.exit_tx.send_replace(Some(exit.clone()));
                self.update_as("terminal.exited", entry, |r| {
                    r.info.status = TerminalStatus::Exited;
                    r.info.exit = Some(exit);
                    if let Some(a) = r.info.agent.as_mut() {
                        a.state = AgentState::Error;
                        a.attention = Some(msg.clone());
                    }
                    true
                });
                Err(ApiError::internal(msg))
            }
        }
    }

    async fn on_exit(&self, state: &AppState, entry: &Arc<Entry>, pty: &Arc<pty::Pty>, info: ExitInfo) {
        {
            let mut cur = entry.pty.lock();
            match &*cur {
                // A newer process already replaced this one (restart): nothing to report.
                Some(p) if Arc::ptr_eq(p, pty) => *cur = None,
                _ => return,
            }
        }
        entry.exit_tx.send_replace(Some(info.clone()));
        if self.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        let is_agent = self.update_as("terminal.exited", entry, |r| {
            r.info.status = TerminalStatus::Exited;
            r.info.exit = Some(info.clone());
            if let Some(a) = r.info.agent.as_mut() {
                a.state = AgentState::Exited;
                a.attention = None;
                a.pending_permission = None;
            }
            // Held permission hooks get no decision: their session is gone.
            entry.rt.lock().permissions.clear();
            true
        }) && entry.kind() == TerminalKind::Agent;
        if is_agent {
            state.auth.revoke_agent_tokens(&entry.id);
        }
        self.save_now(entry).await;
        // Background jobs the process left in its session keep running (a shell's
        // `npm run dev &` before `exit`): keep track of them so Kill and Close reach them.
        tokio::spawn(watch_lingering(state.clone(), entry.clone(), pty.pid));
        self.maybe_hibernate(entry);
    }

    /// Kill the current process (if any) and wait until its exit is recorded; also end
    /// whatever earlier processes left running in their sessions.
    pub(crate) async fn stop_process(&self, entry: &Arc<Entry>) {
        if let Some(p) = entry.running_pty() {
            // Killing `docker exec` leaves its process running in the container: end
            // that too (and first, so a server's port is free when this returns).
            let inside = container_exec(&entry.rec.lock().info);
            if let Some((docker, container)) = inside {
                crate::devcontainer::kill_inside(&docker, &container, &entry.id).await;
            }
            p.kill(KILL_GRACE).await;
            let mut rx = entry.exit_tx.subscribe();
            let _ = tokio::time::timeout(Duration::from_secs(5), async move { rx.wait_for(|v| v.is_some()).await.map(|_| ()) }).await;
        }
        let sids: Vec<i32> = entry.lingering.lock().iter().map(|(sid, _)| *sid).collect();
        if sids.is_empty() {
            return;
        }
        futures::future::join_all(sids.iter().map(|sid| pty::kill_session(*sid, KILL_GRACE, || true))).await;
        entry.lingering.lock().retain(|(sid, _)| !sids.contains(sid));
        self.note_lingering(entry);
    }

    /// Publish the number of lingering processes.
    fn note_lingering(&self, entry: &Entry) {
        let n: u32 = entry.lingering.lock().iter().map(|(_, n)| n).sum();
        self.update(entry, |r| {
            let changed = r.info.lingering != n;
            r.info.lingering = n;
            changed
        });
    }

    /// Start the terminal's process again: agents resume their session, shells get a
    /// fresh login shell below the old output, and run/command terminals re-run when
    /// that is safe without their owner (see the module docs).
    pub(crate) async fn restart(&self, state: &AppState, id: &str) -> Result<TerminalInfo, ApiError> {
        let entry = self.require(id)?;
        // Before anything is stopped: a refused restart must not kill a running deploy.
        if let Some(why) = restart_refusal(&entry.info()) {
            return Err(ApiError::new(axum::http::StatusCode::CONFLICT, "not_restartable", why));
        }
        let _l = entry.lifecycle.lock().await;
        self.stop_process(&entry).await;
        match entry.kind() {
            TerminalKind::Agent => self.relaunch_agent(state, &entry).await?,
            TerminalKind::Shell => {
                let spec = self.shell_launch(state, &entry);
                self.launch(state, &entry, spec, false).await?;
            }
            TerminalKind::Run | TerminalKind::Command => {
                let spec = entry.spec.lock().clone().ok_or_else(|| {
                    ApiError::conflict("this terminal was started before Workbench restarted; start it again from where it came from")
                })?;
                let mut env = base_env(state, &entry.id);
                env.extend(spec.env.iter().cloned());
                let launch = pty::LaunchSpec { argv: spec.argv, cwd: spec.cwd, env, cols: 0, rows: 0, redact: vec![] };
                let remote_control = entry.rec.lock().info.meta.get("remoteControlServer") == Some(&Value::Bool(true));
                if remote_control {
                    // The new server prints a new claude.ai link; the old one is dead.
                    self.update(&entry, |r| {
                        r.info.meta["urls"] = json!([]);
                        true
                    });
                }
                self.launch(state, &entry, launch, false).await?;
                if remote_control {
                    agent::watch_remote_control(state, &entry);
                }
            }
        }
        self.update(&entry, |r| {
            let was = r.info.open;
            r.info.open = true;
            !was
        });
        Ok(entry.info())
    }

    /// Close a terminal: stop its process and hide the tab (it stays in history), or
    /// forget it entirely.
    pub(crate) async fn close(&self, state: &AppState, id: &str, forget: bool) -> Result<(), ApiError> {
        let entry = self.require(id)?;
        {
            let _l = entry.lifecycle.lock().await;
            self.stop_process(&entry).await;
        }
        if forget {
            self.entries.write().remove(id);
            state.auth.revoke_agent_tokens(id);
            if let Some(root) = self.root() {
                let id = id.to_string();
                let _ = tokio::task::spawn_blocking(move || store::remove(&root, &id)).await;
            }
            if let Some(ctx) = self.ctx() {
                let pid = entry.rec.lock().info.project_id.clone();
                ctx.events.emit("terminal.removed", pid.as_deref(), json!({ "id": id }));
            }
        } else {
            self.update(&entry, |r| {
                let was = r.info.open;
                r.info.open = false;
                was
            });
            self.save_now(&entry).await;
            self.maybe_hibernate(&entry);
            self.prune_history().await;
        }
        Ok(())
    }

    /// Forget the oldest closed terminals beyond `MAX_CLOSED` (pinned ones stay).
    async fn prune_history(&self) {
        let mut closed: Vec<(i64, String)> = self
            .all()
            .iter()
            .filter(|e| e.running_pty().is_none() && e.lingering.lock().is_empty())
            .filter_map(|e| {
                let r = e.rec.lock();
                (!r.info.open && !r.info.pinned)
                    .then(|| (r.info.exit.as_ref().map(|x| x.at).unwrap_or(r.info.created_at), r.info.id.clone()))
            })
            .collect();
        if closed.len() <= MAX_CLOSED {
            return;
        }
        closed.sort();
        let drop_n = closed.len() - MAX_CLOSED;
        self.forget_idle(closed.into_iter().take(drop_n).map(|(_, id)| id).collect()).await;
    }

    /// Forget exited run/command terminals beyond the newest `KEEP_EXITED_PER_GROUP` of
    /// each run configuration or env action: every start creates a terminal, and they
    /// would otherwise pile up (open tabs are never pruned by `prune_history`).
    async fn prune_groups(&self) {
        let mut groups: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        for e in self.all() {
            if e.running_pty().is_some() || !e.lingering.lock().is_empty() {
                continue;
            }
            let r = e.rec.lock();
            if r.info.pinned || r.info.status != TerminalStatus::Exited {
                continue;
            }
            if let Some(g) = history_group(&r.info) {
                groups.entry(g).or_default().push((r.info.created_at, r.info.id.clone()));
            }
        }
        let mut ids = vec![];
        for (_, mut v) in groups {
            if v.len() > KEEP_EXITED_PER_GROUP {
                v.sort_by(|a, b| b.cmp(a));
                ids.extend(v.into_iter().skip(KEEP_EXITED_PER_GROUP).map(|(_, id)| id));
            }
        }
        self.forget_idle(ids).await;
    }

    /// Forget terminals nothing is starting or stopping right now (others are skipped),
    /// with their files; tells clients (`terminal.removed`).
    async fn forget_idle(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let mut removed: Vec<(String, Option<String>)> = vec![];
        {
            let mut map = self.entries.write();
            for id in ids {
                let Some(e) = map.get(&id).cloned() else { continue };
                let Ok(_l) = e.lifecycle.try_lock() else { continue };
                if e.running_pty().is_some() {
                    continue;
                }
                map.remove(&id);
                removed.push((id, e.rec.lock().info.project_id.clone()));
            }
        }
        if let Some(ctx) = self.ctx() {
            for (id, pid) in &removed {
                ctx.events.emit("terminal.removed", pid.as_deref(), json!({ "id": id }));
            }
        }
        if let Some(root) = self.root() {
            let _ = tokio::task::spawn_blocking(move || removed.iter().for_each(|(id, _)| store::remove(&root, id))).await;
        }
    }

    /// Drop the mirror of a terminal no process feeds and no client shows; only its
    /// snapshot stays in memory (`Screen::hibernate`).
    pub(crate) fn maybe_hibernate(&self, entry: &Arc<Entry>) {
        if entry.running_pty().is_some() || entry.screen.attached.load(Ordering::Acquire) > 0 {
            return;
        }
        let e = entry.clone();
        let work = move || {
            // A start or stop in progress decides for itself.
            let Ok(_l) = e.lifecycle.try_lock() else { return };
            if e.running_pty().is_none() {
                e.screen.hibernate();
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => drop(h.spawn_blocking(work)),
            Err(_) => work(),
        }
    }

    // ------------------------------------------------------------ input

    pub(crate) async fn send_input(&self, id: &str, text: &str, submit: bool, paste: bool) -> Result<(), ApiError> {
        if text.len() > input::MAX_TEXT {
            return Err(ApiError::bad_request("text is too long (1 MB max)"));
        }
        let entry = self.require(id)?;
        let _g = entry.input_lock.lock().await;
        let pty = entry.running_pty().ok_or_else(|| ApiError::conflict("the terminal is not running"))?;
        entry.look_for_permission_dialog();
        let payload = if paste {
            input::paste_payload(text, entry.screen.bracketed_paste())
        } else {
            input::typed_payload(text)
        };
        if !payload.is_empty() {
            pty.write_wait(Bytes::from(payload)).await.map_err(ApiError::conflict)?;
        }
        if submit {
            // TUIs (and paste guards) can swallow an Enter that arrives with the paste.
            let delay = entry.provider().map_or(300, ProviderKind::submit_delay_ms);
            tokio::time::sleep(Duration::from_millis(delay)).await;
            pty.write_wait(Bytes::from_static(b"\r")).await.map_err(ApiError::conflict)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ size (see `viewers`)

    /// A view reported the size it fits; it is the active view now.
    pub(crate) fn viewer_resize(&self, entry: &Entry, viewer: u64, cols: u16, rows: u16) -> Result<(), ApiError> {
        if !(pty::MIN_COLS..=pty::MAX_COLS).contains(&cols) || !(pty::MIN_ROWS..=pty::MAX_ROWS).contains(&rows) {
            return Err(ApiError::bad_request("terminal size out of range"));
        }
        let mut v = entry.screen.viewers.lock();
        v.report(viewer, cols, rows);
        self.fit_to_viewers(entry, &v);
        Ok(())
    }

    /// The user typed or clicked in a view: it takes the size back.
    pub(crate) fn viewer_active(&self, entry: &Entry, viewer: u64) {
        let mut v = entry.screen.viewers.lock();
        v.touch(viewer);
        self.fit_to_viewers(entry, &v);
    }

    /// A view detached: the most recently active remaining view's size comes back.
    pub(crate) fn viewer_left(&self, entry: &Entry, viewer: u64) {
        let mut v = entry.screen.viewers.lock();
        v.leave(viewer);
        self.fit_to_viewers(entry, &v);
    }

    /// Give the running process the preferred view's size (called with the viewers lock
    /// held, so concurrent views apply their sizes in order).
    fn fit_to_viewers(&self, entry: &Entry, viewers: &viewers::Viewers) {
        let Some((cols, rows)) = viewers.preferred() else { return };
        // An exited terminal keeps the size its final screen was drawn at; the next
        // process starts at the preferred size (`launch`).
        let Some(p) = entry.running_pty() else { return };
        if entry.screen.size() == (cols, rows) {
            return;
        }
        if let Err(e) = p.resize(cols, rows) {
            tracing::debug!("cannot resize terminal {}: {e:#}", entry.id);
            return;
        }
        entry.screen.set_size(cols, rows);
        entry.last_resized_at.store(util::now_ms(), Ordering::Relaxed);
        let mut rec = entry.rec.lock();
        rec.info.cols = cols;
        rec.info.rows = rows;
        entry.meta_dirty.store(true, Ordering::Relaxed);
    }

    // ------------------------------------------------------------ shells

    fn shell_launch(&self, state: &AppState, entry: &Entry) -> pty::LaunchSpec {
        let (cwd, argv) = {
            let r = entry.rec.lock();
            (PathBuf::from(&r.info.cwd), r.info.argv.clone())
        };
        let cwd = if cwd.is_dir() { cwd } else { dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")) };
        let argv = if argv.is_empty() { vec![login_shell(), "-l".into()] } else { argv };
        pty::LaunchSpec { argv, cwd, env: base_env(state, &entry.id), cols: 0, rows: 0, redact: vec![] }
    }

    pub(crate) async fn create_shell(
        &self,
        state: &AppState,
        project_id: Option<String>,
        cwd: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        container: Option<bool>,
    ) -> Result<TerminalInfo, ApiError> {
        let project = match &project_id {
            Some(pid) => Some(state.projects.require(pid)?),
            None => None,
        };
        let base = project.as_ref().map(|p| p.root.clone()).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/"));
        let cwd = resolve_cwd(&base, cwd.as_deref())?;
        // In the project's dev container: asked for, or the project's default.
        let target = match (&project_id, container) {
            (Some(pid), Some(true)) => Some(crate::devcontainer::running_target(state, pid).await.map_err(ApiError::conflict)?),
            (Some(pid), None) => crate::devcontainer::exec_target(state, pid).await,
            (None, Some(true)) => return Err(ApiError::bad_request("a dev container shell needs a project")),
            _ => None,
        };
        let label = if target.is_some() { "Container" } else { "Local" };
        let n = self
            .all()
            .iter()
            .filter(|e| {
                let r = e.rec.lock();
                r.info.kind == TerminalKind::Shell && r.info.open && r.info.project_id == project_id && in_container(&r.info) == target.is_some()
            })
            .count();
        let title = if n == 0 { label.to_string() } else { format!("{label} ({})", n + 1) };
        let (argv, meta) = match &target {
            // The container user's login shell.
            Some(t) => (vec![t.shell.clone(), "-l".into()], json!({ "inContainer": true, "container": t.describe() })),
            None => (vec![login_shell(), "-l".into()], json!({})),
        };
        self.spawn(state, SpawnSpec { kind: TerminalKind::Shell, title, project_id, cwd, argv, env: vec![], cols, rows, meta }).await
    }

    // ------------------------------------------------------------ persistence

    fn collect_writes(&self, force: bool) -> Vec<Write> {
        let mut out = vec![];
        for e in self.all() {
            let screen_due = e.screen.dirty.load(Ordering::Relaxed)
                && (force
                    || e.running_pty().is_none()
                    || e.screen_saved_at.lock().is_none_or(|t| t.elapsed() >= SCREEN_SAVE_EVERY));
            if screen_due {
                e.screen.dirty.store(false, Ordering::Relaxed);
                *e.screen_saved_at.lock() = Some(Instant::now());
                out.push(Write::Screen(e.id.clone(), e.screen.snapshot()));
            }
            if e.meta_dirty.swap(false, Ordering::Relaxed) || screen_due {
                let mut rec = e.rec.lock().clone();
                rec.info.last_output_at = rec.info.last_output_at.max(e.screen.last_output_at.load(Ordering::Relaxed));
                out.push(Write::Meta(rec));
            }
        }
        out
    }

    fn write_all(root: &Path, writes: Vec<Write>) {
        for w in writes {
            let r = match &w {
                Write::Meta(rec) => store::save_meta(root, rec),
                Write::Screen(id, data) => store::save_screen(root, id, data),
            };
            if let Err(e) = r {
                tracing::warn!("cannot save terminal state: {e:#}");
            }
        }
    }

    /// Save one terminal's record and screen now.
    pub(crate) async fn save_now(&self, entry: &Arc<Entry>) {
        let Some(root) = self.root() else { return };
        let entry = entry.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut writes = vec![];
            if entry.screen.dirty.swap(false, Ordering::Relaxed) {
                *entry.screen_saved_at.lock() = Some(Instant::now());
                writes.push(Write::Screen(entry.id.clone(), entry.screen.snapshot()));
            }
            entry.meta_dirty.store(false, Ordering::Relaxed);
            let mut rec = entry.rec.lock().clone();
            rec.info.last_output_at = rec.info.last_output_at.max(entry.screen.last_output_at.load(Ordering::Relaxed));
            writes.push(Write::Meta(rec));
            Self::write_all(&root, writes);
        })
        .await;
    }
}

/// The user's login shell.
fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty() && Path::new(s).is_file())
        .unwrap_or_else(|| "/bin/bash".into())
}

/// Resolve a client-supplied working directory: project-relative, or absolute.
pub(crate) fn resolve_cwd(base: &Path, cwd: Option<&str>) -> Result<PathBuf, ApiError> {
    let dir = match cwd.map(str::trim).filter(|c| !c.is_empty()) {
        None => base.to_path_buf(),
        Some(c) if c.starts_with('/') || c.starts_with('~') => crate::config::expand_tilde(c),
        Some(c) => util::paths::resolve_in_root(base, c)?,
    };
    if !dir.is_dir() {
        return Err(ApiError::bad_request(format!("{} is not a directory", dir.display())));
    }
    Ok(dir)
}

/// Variables describing whatever terminal Workbench itself was started from; wrong for ours.
const PARENT_TERMINAL_VARS: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERMINAL_EMULATOR",
    "VTE_VERSION",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "WEZTERM_PANE",
    "WEZTERM_EXECUTABLE",
    "ITERM_SESSION_ID",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "WINDOWID",
    "COLUMNS",
    "LINES",
];

/// Variables Codex sets for the commands it runs: they describe the Codex session
/// Workbench itself may have been started from, never ours.
const PARENT_AGENT_VARS: &[&str] = &[
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CODEX_TURN_ID",
    "CODEX_SHELL",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CODEX_ESCALATE_SOCKET",
    "CODEX_PERMISSION_PROFILE",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
];

/// Environment every Workbench terminal gets (session hygiene is applied by `pty`).
pub(crate) fn base_env(state: &AppState, id: &str) -> Vec<(String, Option<String>)> {
    let mut env: Vec<(String, Option<String>)> = vec![
        ("TERM".into(), Some("xterm-256color".into())),
        ("COLORTERM".into(), Some("truecolor".into())),
        ("WORKBENCH_URL".into(), Some(state.local_base_url())),
        ("WORKBENCH_TERMINAL_ID".into(), Some(id.to_string())),
    ];
    for k in PARENT_TERMINAL_VARS.iter().chain(PARENT_AGENT_VARS) {
        env.push((k.to_string(), None));
    }
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("VSCODE_") || k.starts_with("WORKBENCH_AGENT_") {
            env.push((k.into_owned(), None));
        }
    }
    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
    if !has_locale {
        env.push(("LANG".into(), Some("C.UTF-8".into())));
    }
    env
}

// ---------------------------------------------------------------- start / shutdown

pub async fn start(state: &AppState) {
    let t = &state.terminals;
    let root = state.paths.data("terminals");
    util::fs::set_mode(&root, 0o700);
    let _ = t.ctx.set(Ctx {
        events: state.events.clone(),
        root: root.clone(),
        attachments: state.paths.data_dir.join("attachments"),
    });
    let loaded = tokio::task::spawn_blocking(move || store::load_all(&root)).await.unwrap_or_default();
    let now = util::now_ms();
    for (mut rec, screen) in loaded {
        // A process recorded as running did not survive the previous Workbench.
        if rec.info.status != TerminalStatus::Exited {
            rec.was_running = true;
            rec.info.status = TerminalStatus::Exited;
            if rec.info.exit.is_none() {
                rec.info.exit = Some(ExitInfo { code: None, signal: Some("Workbench stopped".into()), at: now });
            }
        }
        if let Some(a) = rec.info.agent.as_mut() {
            a.state = AgentState::Exited;
            a.attention = None;
            a.pending_permission = None;
        }
        // Whatever the previous Workbench left running is not ours to track.
        rec.info.lingering = 0;
        let sb = if rec.info.kind == TerminalKind::Agent { AGENT_SCROLLBACK } else { OTHER_SCROLLBACK };
        let entry = t.insert(rec, sb);
        if let Some(bytes) = screen {
            // Kept as the snapshot it is; the mirror is only rebuilt when someone looks.
            entry.screen.hibernate_with(bytes);
        }
    }
    t.prune_history().await;
    t.prune_groups().await;
    tokio::spawn(flusher(state.clone()));
    tokio::spawn(agent::restore(state.clone()));
}

/// Saves dirty records and screens every couple of seconds, one terminal at a time.
async fn flusher(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tick.tick().await;
        let t = &state.terminals;
        if t.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        // Snapshots and disk writes are blocking work.
        let st = state.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let writes = st.terminals.collect_writes(false);
            if let (false, Some(root)) = (writes.is_empty(), st.terminals.root()) {
                Terminals::write_all(&root, writes);
            }
        })
        .await;
    }
}

/// Persist every screen and stop every process. Terminals running now are resumed
/// (agents) or restarted (shells) on the next start.
pub async fn shutdown(state: &AppState) {
    let t = &state.terminals;
    t.shutting_down.store(true, Ordering::Relaxed);
    let running: Vec<(Arc<Entry>, Arc<pty::Pty>)> =
        t.all().into_iter().filter_map(|e| e.running_pty().map(|p| (e, p))).collect();
    for (e, _) in &running {
        e.rec.lock().was_running = true;
        e.meta_dirty.store(true, Ordering::Relaxed);
    }
    let writes = t.collect_writes(true);
    if let Some(root) = t.root() {
        let _ = tokio::task::spawn_blocking(move || Terminals::write_all(&root, writes)).await;
    }
    // Processes in dev containers outlive their `docker exec`: end them too.
    let inside: Vec<(String, String, String)> =
        running.iter().filter_map(|(e, _)| container_exec(&e.rec.lock().info).map(|(d, c)| (d, c, e.id.clone()))).collect();
    let _ = tokio::time::timeout(
        Duration::from_secs(4),
        futures::future::join_all(inside.iter().map(|(d, c, id)| crate::devcontainer::kill_inside(d, c, id))),
    )
    .await;
    let kills = running.iter().map(|(_, p)| p.kill(Duration::from_secs(2)));
    let lingering: Vec<i32> = t.all().iter().flat_map(|e| e.lingering.lock().iter().map(|(sid, _)| *sid).collect::<Vec<_>>()).collect();
    let leftovers = lingering.iter().map(|sid| pty::kill_session(*sid, Duration::from_secs(2), || true));
    let _ = tokio::time::timeout(
        Duration::from_secs(4),
        futures::future::join(futures::future::join_all(kills), futures::future::join_all(leftovers)),
    )
    .await;
}

/// Why `restart` must not re-run this terminal itself (`None`: it may). Other slices
/// start their terminals behind checks a plain re-run would skip.
pub(crate) fn restart_refusal(info: &TerminalInfo) -> Option<&'static str> {
    let meta = |k: &str| info.meta.get(k);
    let flag = |k: &str| meta(k) == Some(&Value::Bool(true));
    match info.kind {
        TerminalKind::Agent | TerminalKind::Shell => None,
        TerminalKind::Run => Some(
            "run configurations are restarted from the Run tool window (or POST /api/projects/{pid}/runs/{name}/restart), \
             so Workbench keeps tracking them",
        ),
        TerminalKind::Command if flag("remoteControlServer") || flag("restartable") => None,
        TerminalKind::Command => match meta("action").and_then(Value::as_str) {
            // Following logs is read-only.
            Some("logs") => None,
            Some("deploy") => Some("a deploy is started again from its environment's Deploy button, so its gates and confirmation run again"),
            Some("command") => Some("environment commands are started again from the Apps tool window (they may need confirmation)"),
            _ => Some("this terminal was started by another part of Workbench; start it again from there"),
        },
    }
}

/// Exited run/command terminals kept per run configuration or env action.
const KEEP_EXITED_PER_GROUP: usize = 3;

/// What a run/command terminal is a repeat of: the same run configuration, env action
/// or Remote Control server of the same project. Agents and shells are not grouped.
fn history_group(info: &TerminalInfo) -> Option<String> {
    if !matches!(info.kind, TerminalKind::Run | TerminalKind::Command) {
        return None;
    }
    let m = &info.meta;
    let s = |k: &str| m.get(k).and_then(Value::as_str);
    let key = if let Some(run) = s("run") {
        format!("run:{run}")
    } else if let Some(env) = s("env") {
        format!("env:{env}:{}:{}", s("action").unwrap_or(""), s("command").or(s("log")).unwrap_or(""))
    } else if m.get("remoteControlServer") == Some(&Value::Bool(true)) {
        "remote-control".to_string()
    } else {
        format!("title:{}", info.title)
    };
    Some(format!("{}|{key}", info.project_id.as_deref().unwrap_or("")))
}

/// Follow the processes an exited process left in its session until they are gone, and
/// publish how many there are (`TerminalInfo::lingering`).
async fn watch_lingering(state: AppState, entry: Arc<Entry>, sid: i32) {
    if sid <= 1 {
        return;
    }
    // Children commonly need a moment to notice their parent is gone.
    tokio::time::sleep(Duration::from_secs(1)).await;
    loop {
        let n = tokio::task::spawn_blocking(move || pty::session_members(sid).len()).await.unwrap_or(0) as u32;
        let t = &state.terminals;
        let known = t.is_registered(&entry);
        {
            let mut l = entry.lingering.lock();
            let at = l.iter().position(|(s, _)| *s == sid);
            match (at, n > 0 && known) {
                (Some(i), true) => l[i].1 = n,
                (None, true) => l.push((sid, n)),
                (Some(i), false) => {
                    l.remove(i);
                }
                (None, false) => {}
            }
        }
        t.note_lingering(&entry);
        if n == 0 || !known {
            t.maybe_hibernate(&entry);
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// An agent record for tests.
#[cfg(test)]
pub(crate) fn test_agent() -> AgentInfo {
    AgentInfo {
        session_id: String::new(),
        provider: ProviderKind::Claude,
        provider_id: None,
        state: AgentState::Starting,
        unread: false,
        model: None,
        effort: None,
        permission_mode: None,
        remote_control: false,
        remote_url: None,
        title: None,
        last_message: None,
        attention: None,
        context_pct: None,
        cost_usd: None,
        last_event_at: 0,
        pending_permission: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_keys_count_as_typing() {
        assert!(is_typing(b"hello"));
        assert!(is_typing(b"\r"));
        assert!(!is_typing(b"\x1b[I"));
        assert!(!is_typing(b"\x1b[<35;10;5M"));
        assert!(!is_typing(b"\x1b[?1;2c"));
        assert!(!is_typing(b""));
    }

    #[test]
    fn keys_and_mouse_are_user_input_terminal_replies_are_not() {
        for keys in [&b"a"[..], b"\r", b"\x03", b"\x1b", b"\x1b[A", b"\x1bOA", b"\x1b[1;5A", b"\x1b[15~", b"\x1b[<0;10;5M", b"\x1bb", b"\x1bP"] {
            assert!(is_user_input(keys), "{keys:?}");
        }
        for reply in [&b""[..], b"\x1b[I", b"\x1b[O", b"\x1b[?1;2c", b"\x1b[>0;276;0c", b"\x1b[12;5R", b"\x1b[0n", b"\x1b[?2026;2$y", b"\x1b]11;rgb:0000/0000/0000\x07", b"\x1bP>|x\x1b\\"] {
            assert!(!is_user_input(reply), "{reply:?}");
        }
    }

    fn info(kind: TerminalKind, meta: Value) -> TerminalInfo {
        TerminalInfo {
            id: "t".into(),
            kind,
            title: "T".into(),
            project_id: Some("p".into()),
            cwd: "/".into(),
            argv: vec![],
            status: TerminalStatus::Exited,
            exit: None,
            created_at: 0,
            last_output_at: 0,
            cols: 80,
            rows: 24,
            open: true,
            pinned: false,
            color: None,
            order: 0,
            agent: None,
            meta,
            lingering: 0,
        }
    }

    #[test]
    fn only_safe_terminals_restart_themselves() {
        use TerminalKind::*;
        assert!(restart_refusal(&info(Shell, json!({}))).is_none());
        assert!(restart_refusal(&info(Agent, json!({}))).is_none());
        assert!(restart_refusal(&info(Command, json!({ "env": "production", "action": "deploy", "restartable": false }))).is_some());
        assert!(restart_refusal(&info(Command, json!({ "env": "production", "action": "command" }))).is_some());
        assert!(restart_refusal(&info(Run, json!({ "run": "api", "restartable": true }))).is_some());
        assert!(restart_refusal(&info(Command, json!({}))).is_some());
        assert!(restart_refusal(&info(Command, json!({ "env": "production", "action": "logs" }))).is_none());
        assert!(restart_refusal(&info(Command, json!({ "remoteControlServer": true }))).is_none());
        assert!(restart_refusal(&info(Command, json!({ "restartable": true }))).is_none());
    }

    #[test]
    fn repeats_of_a_run_or_env_action_share_a_group() {
        use TerminalKind::*;
        let g = |kind, meta| history_group(&info(kind, meta));
        assert_eq!(g(Run, json!({ "run": "api" })), g(Run, json!({ "run": "api" })));
        assert_ne!(g(Run, json!({ "run": "api" })), g(Run, json!({ "run": "web" })));
        assert_ne!(
            g(Command, json!({ "env": "prod", "action": "logs", "log": "api" })),
            g(Command, json!({ "env": "prod", "action": "logs", "log": "db" }))
        );
        assert_ne!(g(Command, json!({ "env": "prod", "action": "deploy" })), g(Command, json!({ "env": "staging", "action": "deploy" })));
        assert_eq!(g(Shell, json!({})), None);
        assert_eq!(g(Agent, json!({})), None);
    }

    #[test]
    fn ids_are_short_lowercase_and_validated() {
        let id = new_id();
        assert_eq!(id.len(), 12);
        assert!(valid_id(&id));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        assert!(!valid_id("ABC"));
    }

    #[test]
    fn cwd_resolution_stays_in_the_root_for_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(resolve_cwd(dir.path(), None).unwrap(), dir.path());
        assert_eq!(resolve_cwd(dir.path(), Some("sub")).unwrap(), dir.path().join("sub"));
        assert!(resolve_cwd(dir.path(), Some("../..")).is_err());
        assert!(resolve_cwd(dir.path(), Some("missing")).is_err());
        assert!(resolve_cwd(dir.path(), Some(&dir.path().join("sub").display().to_string())).is_ok());
    }
}

#[cfg(test)]
mod e2e;
#[cfg(test)]
mod e2e_agents;

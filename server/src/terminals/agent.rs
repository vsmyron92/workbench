//! Agent sessions of every provider: launch, resume/fork, Claude Code's per-session
//! settings (HTTP hooks, status line, Workbench MCP), the per-provider watchers (Claude's
//! transcript tail, Codex's rollout tail, the output-activity heuristic for Kimi and
//! custom CLIs), `ask`, Remote Control servers, restore on start, and the history /
//! live-session listings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::activity::Activity;
use super::hooks::{self, AgentRt};
use super::permission;
use super::providers::{self, Continue, LaunchArgs, Provider, ProviderKind};
use super::store::AgentLaunch;
use super::transcript::{self, HistoryEntry, LineBuffer, Record};
use super::{
    AGENT_SCROLLBACK, AgentInfo, AgentState, Entry, TerminalInfo, TerminalKind, TerminalStatus, Terminals, base_env, codex, gemini,
    kimi, new_id, pty, resolve_cwd, store,
};
use crate::app::AppState;
use crate::config::global::AgentsConfig;
use crate::error::ApiError;
use crate::projects::Project;
use crate::secrets::Secret;
use crate::util;

pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
pub const PERMISSION_MODES: &[&str] = &["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];
/// Prompts longer than this are pasted after startup instead of passed in argv.
const MAX_ARGV_PROMPT: usize = 64 * 1024;
/// HTTP hook events Workbench listens to. Tool events take a match-all matcher.
/// `SessionStart` is not here: Claude Code skips HTTP hooks for it ("HTTP hooks are not
/// supported for SessionStart"), so it is a command hook through `workbench statusline`.
const HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];
const TOOL_EVENTS: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure", "PermissionRequest", "PermissionDenied"];

// ---------------------------------------------------------------- requests

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentRequest {
    pub project_id: String,
    /// `[agents.providers.<id>]`; `None`: `[agents].default_provider` (or `claude`).
    pub provider: Option<String>,
    pub cwd: Option<String>,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub remote_control: Option<bool>,
    pub resume: Option<String>,
    pub fork: bool,
    pub add_dirs: Vec<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// Run the session in the project's running dev container (its CLI must exist there).
    pub in_container: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskRequest {
    pub project_id: Option<String>,
    pub prompt: String,
    pub terminal_id: Option<String>,
    /// Only sessions of this provider (and new sessions start with it). `None`: the most
    /// recent session of any provider, or a new one of the default provider.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub new_session: bool,
    pub name: Option<String>,
    pub submit: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlRequest {
    pub project_id: String,
    pub spawn: Option<String>,
    pub name: Option<String>,
    pub permission_mode: Option<String>,
}

// ---------------------------------------------------------------- pure helpers

#[derive(Debug, Clone, PartialEq)]
pub enum SessionArg {
    New(String),
    Resume(String),
    Fork { from: String, new_id: String },
}

impl SessionArg {
    /// The id the running session will have.
    pub fn id(&self) -> &str {
        match self {
            SessionArg::New(id) | SessionArg::Resume(id) => id,
            SessionArg::Fork { new_id, .. } => new_id,
        }
    }
}

/// The `claude` command line for a session.
pub fn build_argv(
    command: &str,
    session: &SessionArg,
    settings: &Path,
    mcp: &Path,
    l: &AgentLaunch,
    prompt: Option<&str>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![command.to_string()];
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match session {
        SessionArg::New(id) => push(&["--session-id", id]),
        SessionArg::Resume(id) => push(&["--resume", id]),
        // `--session-id` is accepted with `--resume` only together with `--fork-session`.
        SessionArg::Fork { from, new_id } => push(&["--resume", from, "--fork-session", "--session-id", new_id]),
    }
    push(&["--settings", &settings.to_string_lossy(), "--mcp-config", &mcp.to_string_lossy()]);
    if let Some(n) = &l.name {
        push(&["--name", n]);
    }
    if let Some(m) = &l.model {
        push(&["--model", m]);
    }
    if let Some(e) = &l.effort {
        push(&["--effort", e]);
    }
    if let Some(p) = &l.permission_mode {
        push(&["--permission-mode", p]);
    }
    for d in &l.add_dirs {
        push(&["--add-dir", d]);
    }
    if l.remote_control {
        push(&["--remote-control"]);
    }
    // `--` ends the options: variadic flags (`--add-dir`, `--mcp-config`) and the
    // optional `--remote-control [name]` cannot swallow the prompt.
    if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
        push(&["--", p]);
    }
    a
}

/// Per-session settings: HTTP hooks to Workbench, the `SessionStart` command hook and,
/// optionally, the status line. `helper` is the shell command running
/// `workbench statusline` (it forwards hook payloads and renders status lines).
/// `permission_wait`: Workbench holds `PermissionRequest` hooks for up to that many
/// seconds to answer them from a device (`None`: they are only observed).
pub fn session_settings(hook_url: &str, token: &str, helper: Option<&str>, statusline: bool, permission_wait: Option<u64>) -> Value {
    let hook = |timeout: u64| {
        json!({
            "type": "http",
            "url": hook_url,
            "headers": { "Authorization": format!("Bearer {token}") },
            "timeout": timeout,
        })
    };
    let mut hooks = serde_json::Map::new();
    for ev in HOOK_EVENTS {
        // Claude cancels a hook at its timeout: a held one must outlast our wait.
        let h = match (*ev, permission_wait) {
            ("PermissionRequest", Some(w)) => hook(w + super::permission::HOOK_TIMEOUT_MARGIN_SECS),
            _ => hook(5),
        };
        let group = if TOOL_EVENTS.contains(ev) {
            json!([{ "matcher": "*", "hooks": [h] }])
        } else {
            json!([{ "hooks": [h] }])
        };
        hooks.insert(ev.to_string(), group);
    }
    if let Some(cmd) = helper {
        hooks.insert("SessionStart".into(), json!([{ "hooks": [{ "type": "command", "command": cmd, "timeout": 5 }] }]));
    }
    let mut s = json!({ "hooks": hooks });
    if let (Some(cmd), true) = (helper, statusline) {
        s["statusLine"] = json!({ "type": "command", "command": cmd, "padding": 0 });
    }
    s
}

pub fn mcp_config(url: &str, token: &str, terminal_id: &str) -> Value {
    json!({
        "mcpServers": {
            "workbench": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer {token}"), "X-Workbench-Terminal": terminal_id },
            }
        }
    })
}

/// How long `PermissionRequest` hooks are held for an answer from Workbench (`None`:
/// answering from Workbench is off, the hooks are only observed).
pub fn permission_wait(cfg: &AgentsConfig) -> Option<u64> {
    cfg.answer_permissions.then(|| super::permission::wait_secs(cfg.permission_wait))
}

/// Whether any of the user's own settings files define a status line (then ours is
/// not installed: `--settings` would override theirs).
pub fn user_defines_statusline(claude_dir: &Path, dirs: &[&Path]) -> bool {
    let mut files = vec![claude_dir.join("settings.json")];
    for d in dirs {
        files.push(d.join(".claude/settings.json"));
        files.push(d.join(".claude/settings.local.json"));
    }
    files.iter().any(|f| {
        std::fs::read(f)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v.get("statusLine").is_some_and(|s| !s.is_null()))
    })
}

/// The Workbench executable for the `statusline` helper. After the binary was replaced
/// on disk (an upgrade, a rebuild), Linux reports `… (deleted)`: use the new file at the
/// same path, or else the running image through `/proc/<pid>/exe`.
fn helper_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if exe.is_file() {
        return Some(exe);
    }
    helper_fallback(&exe, std::process::id())
}

fn helper_fallback(exe: &Path, pid: u32) -> Option<PathBuf> {
    let s = exe.to_string_lossy();
    if let Some(p) = s.strip_suffix(" (deleted)").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let proc_exe = PathBuf::from(format!("/proc/{pid}/exe"));
    proc_exe.exists().then_some(proc_exe)
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Find the Claude Code executable: as configured, on PATH, or in the usual install spots
/// (a desktop-launched Workbench often lacks `~/.local/bin` on PATH).
pub fn resolve_command(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    if cmd.contains('/') || cmd.starts_with('~') {
        let p = crate::config::expand_tilde(cmd);
        return p.is_file().then_some(p);
    }
    if let Some(p) = util::which_path(cmd) {
        return Some(p);
    }
    let home = dirs::home_dir()?;
    [".local/bin", ".claude/local", ".npm-global/bin", "bin", ".bun/bin"]
        .iter()
        .map(|d| home.join(d).join(cmd))
        .chain(std::iter::once(PathBuf::from("/usr/local/bin").join(cmd)))
        .find(|p| p.is_file())
}

use providers::valid_model;

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Request values over project `[agent]` over the provider's config, validated for the
/// provider's kind. Project `[agent]` model, effort and permission mode are Claude Code
/// settings: other kinds take only the request and their own provider config. A
/// dangerous permission preset of another kind is never a default: only a request (the
/// user's explicit choice) selects it.
pub fn resolve_launch(provider: &Provider, cfg: &AgentsConfig, project: &Project, req: &AgentRequest) -> Result<AgentLaunch, ApiError> {
    let kind = provider.kind;
    let claude = kind == ProviderKind::Claude;
    let pa = &project.config.agent;
    let project_value = |v: &Option<String>| if claude { non_empty(v.clone()) } else { None };
    let model = non_empty(req.model.clone()).or_else(|| project_value(&pa.model)).or_else(|| non_empty(provider.model.clone()));
    let effort = non_empty(req.effort.clone()).or_else(|| project_value(&pa.effort)).or_else(|| non_empty(provider.effort.clone()));
    let permission_mode = non_empty(req.permission_mode.clone()).or_else(|| project_value(&pa.permission_mode)).or_else(|| {
        non_empty(provider.permission_mode.clone()).filter(|m| claude || provider.permission_preset(m).is_some_and(|p| !p.dangerous))
    });
    if let Some(m) = &model {
        if kind == ProviderKind::Custom {
            return Err(ApiError::bad_request(format!("{} takes no model option; put it in its args in config.toml", provider.label)));
        }
        if !valid_model(m) {
            return Err(ApiError::bad_request(format!("invalid model {m:?}")));
        }
    }
    if let Some(e) = &effort {
        let efforts = kind.efforts();
        if efforts.is_empty() {
            return Err(ApiError::bad_request(format!("{} has no effort setting", provider.label)));
        }
        if !efforts.contains(&e.as_str()) {
            return Err(ApiError::bad_request(format!("effort must be one of {}", efforts.join(", "))));
        }
    }
    if let Some(p) = &permission_mode {
        if provider.permission_preset(p).is_none() {
            let ids: Vec<&str> = kind.permission_modes().iter().map(|p| p.id).collect();
            return Err(ApiError::bad_request(if ids.is_empty() {
                format!("{} has no permission modes", provider.label)
            } else {
                format!("permission mode must be one of {}", ids.join(", "))
            }));
        }
    }
    let mut add_dirs: Vec<String> = vec![];
    if kind.takes_add_dirs() {
        for d in pa.add_dirs.iter().chain(req.add_dirs.iter()) {
            let d = d.trim();
            if d.is_empty() {
                continue;
            }
            let p = if util::os::path::is_absolute_str(d) || d.starts_with('~') { crate::config::expand_tilde(d) } else { project.root.join(d) };
            if !p.is_dir() {
                return Err(ApiError::bad_request(format!("additional directory {} does not exist", p.display())));
            }
            let s = p.display().to_string();
            if !add_dirs.contains(&s) {
                add_dirs.push(s);
            }
        }
    }
    Ok(AgentLaunch {
        provider: Some(provider.id.clone()),
        name: non_empty(req.name.clone()).map(|n| transcript::clean_line(&n, 80)),
        model,
        effort,
        permission_mode,
        remote_control: claude && req.remote_control.unwrap_or(pa.remote_control || cfg.remote_control),
        add_dirs,
    })
}

/// Directories where agents write Workspace deliverables (`data_dir/workspace/<project>`
/// and `data_dir/workspace/home`), created 0700 when missing. The workspace slice's MCP
/// tools hand out folders under them.
pub fn workspace_dirs(data_dir: &Path, project_id: Option<&str>) -> Vec<String> {
    let root = data_dir.join("workspace");
    let safe = |id: &&str| {
        !id.is_empty() && *id != "." && *id != ".." && *id != "home" && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    let mut out = vec![];
    for scope in project_id.filter(safe).into_iter().chain(std::iter::once("home")) {
        let dir = root.join(scope);
        if std::fs::create_dir_all(&dir).is_ok() {
            util::fs::set_mode(&root, 0o700);
            util::fs::set_mode(&dir, 0o700);
            out.push(dir.display().to_string());
        }
    }
    out
}

/// The value an agent environment gives `key` (the last assignment wins).
fn env_value(env: &[(String, Option<String>)], key: &str) -> Option<String> {
    env.iter().rev().find(|(k, _)| k == key).and_then(|(_, v)| v.clone())
}

static SECRET_REF: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\$\{secret:([A-Za-z0-9_.-]+)\}").unwrap());
static CLAUDE_URL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"https://claude\.ai/code[^\s\x07\x1b"'<>\\)\]]*"#).unwrap());

/// Claude.ai links in text (Remote Control output), trailing punctuation trimmed.
pub fn claude_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for m in CLAUDE_URL.find_iter(text) {
        let u = m.as_str().trim_end_matches(['.', ',', ';', ':']).to_string();
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// Whether a prompt can be typed into a session in this state. A session showing a
/// permission, question or trust dialog must not receive one: the Enter that submits the
/// prompt would answer the dialog instead.
pub fn accepts_prompt(s: AgentState) -> bool {
    matches!(s, AgentState::Idle | AgentState::Working | AgentState::Error)
}

/// Why text must not be typed into a session in this state through `/input` (`None`: it
/// may). The phone's compose box sends text + Enter; into a dialog, the Enter picks the
/// highlighted option ("Yes" on a permission prompt). Dialogs are answered with keys.
pub fn input_refusal(s: AgentState) -> Option<&'static str> {
    match s {
        AgentState::NeedsPermission => Some("The session is asking for permission: answer the prompt in the terminal first"),
        AgentState::NeedsInput => Some("The session is showing a question or dialog: answer it in the terminal first"),
        AgentState::Starting => Some("The session is still starting: try again in a moment"),
        _ => None,
    }
}

/// Rows at the bottom of the visible screen where a CLI's dialogs are looked for.
const DIALOG_ROWS: usize = 16;
/// How long an initial prompt waits for a session to take input (a trust prompt the user
/// has not answered yet, say) before it is dropped.
const DELIVER_MAX: Duration = Duration::from_secs(600);

/// A tab title from a prompt.
fn title_from_prompt(p: &str) -> String {
    transcript::clean_line(p, 48)
}

// ---------------------------------------------------------------- Terminals: agents

/// The provider `id` (`None`: the default provider), enabled.
pub fn find_provider(cfg: &AgentsConfig, id: Option<&str>) -> Result<Provider, ApiError> {
    let p = providers::find(cfg, id).ok_or_else(|| match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => ApiError::not_configured(format!("no agent provider {name:?}; add [agents.providers.{name}] to config.toml")),
        None => {
            let name = providers::default_id(cfg);
            ApiError::not_configured(format!(
                "[agents].default_provider is {name:?}, which is not a configured provider; correct it or add [agents.providers.{name}] to config.toml"
            ))
        }
    })?;
    if !p.enabled {
        return Err(ApiError::not_configured(format!("{} is disabled in config.toml ([agents.providers.{}] enabled = false)", p.label, p.id)));
    }
    Ok(p)
}

/// The error for a provider whose command is not installed.
fn missing_command(p: &Provider) -> ApiError {
    let key = if p.id == "claude" { "[agents].command".to_string() } else { format!("[agents.providers.{}].command", p.id) };
    let hint = if p.install_hint.is_empty() { String::new() } else { format!(" (`{}`)", p.install_hint) };
    ApiError::not_configured(format!("{} ({:?}) was not found. Install it{hint}, or set {key} in config.toml", p.label, p.command))
}

/// An initial prompt goes into argv unless it is huge or must not be submitted.
fn split_prompt(prompt: Option<String>, submit: bool) -> (Option<String>, Option<String>) {
    match prompt {
        Some(p) if p.len() <= MAX_ARGV_PROMPT && submit => (Some(p), None),
        Some(p) => (None, Some(p)),
        None => (None, None),
    }
}

/// For display: long values shortened, home as `~`, `hidden` paths by file name only.
fn display_argv(argv: &[String], hidden: &[String]) -> Vec<String> {
    argv.iter()
        .map(|a| {
            if hidden.contains(a) {
                format!("…/{}", Path::new(a).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            } else {
                transcript::truncate_chars(&crate::config::contract_tilde(Path::new(a)), 120)
            }
        })
        .collect()
}

/// Everything a launch of any provider shares.
struct Prepared {
    cwd: PathBuf,
    launch: AgentLaunch,
    project: Option<Arc<Project>>,
    provider: Provider,
    command: PathBuf,
    env: Vec<(String, Option<String>)>,
    /// `data_dir/terminals/<id>` (0700): per-session files.
    dir: PathBuf,
    token: String,
    /// The launch's directories plus the Workspace deliverable folders.
    add_dirs: Vec<String>,
    /// The session runs in the project's dev container (`command` is its path there).
    container: Option<crate::devcontainer::ExecTarget>,
}

/// A Codex session's rollout, as the watcher follows it.
struct CodexWatch {
    home: PathBuf,
    path: Option<PathBuf>,
    offset: Option<u64>,
    launched_at: i64,
    /// A fork's new rollout may start with the parent's history: skip older lines.
    fork: bool,
    /// Rollouts found to be another Codex's (held open, or Codex running in the folder,
    /// outside the waiting hosted sessions): never taken.
    foreign: HashSet<PathBuf>,
}

/// A new Kimi session still looking for its id (its home and the index ids that existed
/// at launch are in its `AgentRt`).
#[derive(Default)]
struct KimiWatch {
    /// Entries only a screen may attribute: when they were the evidence's pick, Kimi also
    /// ran in that folder outside the hosted sessions.
    ambiguous: HashSet<String>,
}

impl Terminals {
    fn find_agent_by_session(&self, provider_id: &str, session_id: &str) -> Option<Arc<Entry>> {
        if session_id.is_empty() {
            return None;
        }
        self.all().into_iter().find(|e| {
            let r = e.rec.lock();
            r.info.agent.as_ref().is_some_and(|a| a.session_id == session_id && a.provider_id.as_deref().unwrap_or("claude") == provider_id)
        })
    }

    /// Start a new agent session of any provider (or resume/fork one).
    pub(crate) async fn spawn_agent(&self, state: &AppState, req: AgentRequest) -> Result<TerminalInfo, ApiError> {
        let project = state.projects.require(&req.project_id)?;
        let cwd = resolve_cwd(&project.root, req.cwd.as_deref())?;
        let cfg = state.config.read().agents.clone();
        let provider = find_provider(&cfg, req.provider.as_deref())?;
        let kind = provider.kind;
        if let Some(r) = &req.resume {
            if !kind.resumes() {
                return Err(ApiError::bad_request(format!("{} sessions cannot be resumed", provider.label)));
            }
            if req.fork && !kind.forks() {
                return Err(ApiError::bad_request(format!("{} sessions cannot be forked", provider.label)));
            }
            if !providers::valid_session_id(kind, r) {
                return Err(ApiError::bad_request(format!("resume must be a {} session id", provider.label)));
            }
            // Resuming a session this Workbench already hosts reuses its terminal.
            if !req.fork {
                if let Some(e) = self.find_agent_by_session(&provider.id, r) {
                    if e.running_pty().is_none() {
                        self.restart(state, &e.id).await?;
                    } else {
                        self.update(&e, |rec| {
                            let was = rec.info.open;
                            rec.info.open = true;
                            !was
                        });
                    }
                    return Ok(e.info());
                }
            }
        } else if req.fork {
            return Err(ApiError::bad_request("fork needs resume: the session to fork"));
        }
        let launch = resolve_launch(&provider, &cfg, &project, &req)?;
        // In the dev container, the CLI must exist there; on the host, here.
        let container = if req.in_container {
            let (t, _) = crate::devcontainer::agent_command(state, &project.id, &provider.command).await.map_err(ApiError::conflict)?;
            Some(t)
        } else {
            if resolve_command(&provider.command).is_none() {
                return Err(missing_command(&provider));
            }
            None
        };
        let cont = match (&req.resume, req.fork) {
            (Some(r), true) => Continue::Fork(r.clone()),
            (Some(r), false) => Continue::Resume(r.clone()),
            (None, _) => Continue::New,
        };
        // Claude's and Gemini's ids are chosen up front (`--session-id`); the others'
        // are discovered.
        let session_id = match (&cont, kind) {
            (Continue::Resume(id), _) => id.clone(),
            (_, k) if k.id_at_launch() => uuid::Uuid::new_v4().to_string(),
            _ => String::new(),
        };
        let prompt = non_empty(req.prompt.clone());
        let title = launch.name.clone().or_else(|| prompt.as_deref().map(title_from_prompt)).unwrap_or_else(|| {
            if req.resume.is_some() {
                "Resumed session".into()
            } else if kind == ProviderKind::Claude {
                "Claude".into()
            } else {
                provider.label.clone()
            }
        });
        let (cols, rows) = pty::clamp_size(req.cols.unwrap_or(120), req.rows.unwrap_or(32));
        let now = util::now_ms();
        let id = new_id();
        let info = TerminalInfo {
            id: id.clone(),
            kind: TerminalKind::Agent,
            title,
            project_id: Some(project.id.clone()),
            cwd: cwd.display().to_string(),
            argv: vec![],
            status: TerminalStatus::Starting,
            exit: None,
            created_at: now,
            last_output_at: 0,
            cols,
            rows,
            open: true,
            pinned: false,
            color: None,
            order: self.next_order(),
            agent: Some(AgentInfo {
                session_id,
                provider: kind,
                provider_id: Some(provider.id.clone()),
                state: AgentState::Starting,
                unread: false,
                model: launch.model.clone(),
                effort: launch.effort.clone(),
                permission_mode: launch.permission_mode.clone(),
                remote_control: launch.remote_control,
                remote_url: None,
                title: None,
                last_message: None,
                attention: None,
                context_pct: None,
                cost_usd: None,
                last_event_at: now,
                pending_permission: None,
            }),
            meta: {
                let mut m = match &cont {
                    Continue::Fork(from) => json!({ "forkedFrom": from }),
                    _ => json!({}),
                };
                if let Some(t) = &container {
                    m["inContainer"] = json!(true);
                    m["container"] = t.describe();
                }
                m
            },
            lingering: 0,
        };
        let rec = store::Record {
            info,
            launch: Some(launch.clone()),
            title_locked: launch.name.is_some(),
            transcript_path: None,
            was_running: false,
            aider_history: None,
        };
        let entry = self.insert(rec, AGENT_SCROLLBACK);
        self.emit("terminal.created", &entry);
        let _l = entry.lifecycle.lock().await;
        self.launch_agent(state, &entry, cont, prompt, true, false).await?;
        Ok(entry.info())
    }

    /// Continue the entry's conversation in a fresh process (a new one when it has none
    /// yet, and always for custom CLIs; Aider restores its chat history).
    pub(crate) async fn relaunch_agent(&self, state: &AppState, entry: &Arc<Entry>) -> Result<(), ApiError> {
        let (kind, session_id) = {
            let r = entry.rec.lock();
            r.info.agent.as_ref().map(|a| (a.provider, a.session_id.clone())).unwrap_or_default()
        };
        let cont = if kind.resumes() && providers::valid_session_id(kind, &session_id) { Continue::Resume(session_id) } else { Continue::New };
        self.launch_agent(state, entry, cont, None, true, true).await
    }

    /// `restarted`: the terminal's conversation continues (a restart or a restore).
    async fn launch_agent(
        &self,
        state: &AppState,
        entry: &Arc<Entry>,
        cont: Continue,
        prompt: Option<String>,
        submit_prompt: bool,
        restarted: bool,
    ) -> Result<(), ApiError> {
        let prep = self.prepare_launch(state, entry).await?;
        match prep.provider.kind {
            ProviderKind::Claude => self.launch_claude(state, entry, prep, cont, prompt, submit_prompt).await,
            // Codex's rollout lives in the container: in one, it is followed like a
            // plain CLI (output activity), without Workbench's MCP.
            ProviderKind::Codex if prep.container.is_none() => self.launch_codex(state, entry, prep, cont, prompt, submit_prompt).await,
            ProviderKind::Codex | ProviderKind::Kimi | ProviderKind::Gemini | ProviderKind::Aider | ProviderKind::Custom => {
                self.launch_plain(state, entry, prep, cont, prompt, submit_prompt, restarted).await
            }
        }
    }

    /// The provider, command, environment, agent token and directories of a launch.
    async fn prepare_launch(&self, state: &AppState, entry: &Arc<Entry>) -> Result<Prepared, ApiError> {
        let (cwd, launch, project_id, in_container) = {
            let r = entry.rec.lock();
            (PathBuf::from(&r.info.cwd), r.launch.clone().unwrap_or_default(), r.info.project_id.clone(), super::in_container(&r.info))
        };
        if !cwd.is_dir() {
            return Err(ApiError::conflict(format!("{} no longer exists", cwd.display())));
        }
        let project = project_id.as_deref().and_then(|p| state.projects.get(p));
        let cfg = state.config.read().agents.clone();
        // Records from before providers are Claude sessions, whatever the default is now.
        let provider = find_provider(&cfg, Some(launch.provider.as_deref().unwrap_or("claude")))?;
        let (command, container) = if in_container {
            let pid = project_id.as_deref().ok_or_else(|| ApiError::conflict("a dev container session needs its project"))?;
            let (t, path) = crate::devcontainer::agent_command(state, pid, &provider.command).await.map_err(ApiError::conflict)?;
            (PathBuf::from(path), Some(t))
        } else {
            (resolve_command(&provider.command).ok_or_else(|| missing_command(&provider))?, None)
        };

        // Environment: base, the provider's (config.toml), then the project overlay's.
        let mut env = base_env(state, &entry.id);
        for (k, v) in &provider.env {
            let v = if v.starts_with("~/") { crate::config::expand_tilde(v).display().to_string() } else { v.clone() };
            env.push((k.clone(), Some(v)));
        }
        let mut secrets = vec![];
        if let Some(p) = &project {
            for (k, v) in &p.config.agent.env {
                env.push((k.clone(), Some(expand_env_value(state, p, v, &mut secrets)?)));
            }
        }
        // Secret values from `${secret:…}` are masked if the session prints them.
        *entry.redact.lock() = secrets;

        // A fresh token per process; the old one dies here.
        state.auth.revoke_agent_tokens(&entry.id);
        let token = state.auth.issue_agent_token(&entry.id);
        let root = self.root().ok_or_else(|| ApiError::internal("terminals not started"))?;
        let dir = store::ensure_dir(&root, &entry.id)?;

        let mut add_dirs = launch.add_dirs.clone();
        if let Some(t) = &container {
            // Inside, only folders of the workspace mount exist (at their container path);
            // the Workspace deliverable folders stay on the host (workspace_write_file).
            add_dirs = add_dirs.iter().filter_map(|d| t.map_path(Path::new(d))).collect();
        } else if provider.kind.takes_add_dirs() {
            let data_dir = state.paths.data_dir.clone();
            let pid = project_id.clone();
            let ws = tokio::task::spawn_blocking(move || workspace_dirs(&data_dir, pid.as_deref())).await.unwrap_or_default();
            for d in ws {
                if !add_dirs.contains(&d) {
                    add_dirs.push(d);
                }
            }
        }
        Ok(Prepared { cwd, launch, project, provider, command, env, dir, token, add_dirs, container })
    }

    async fn launch_claude(
        &self,
        state: &AppState,
        entry: &Arc<Entry>,
        prep: Prepared,
        cont: Continue,
        prompt: Option<String>,
        submit_prompt: bool,
    ) -> Result<(), ApiError> {
        let current = entry.rec.lock().info.agent.as_ref().map(|a| a.session_id.clone()).unwrap_or_default();
        let fresh = || if transcript::is_uuid(&current) { current.clone() } else { uuid::Uuid::new_v4().to_string() };
        let session = match cont {
            Continue::New => SessionArg::New(fresh()),
            Continue::Resume(id) => SessionArg::Resume(id),
            Continue::Fork(from) => {
                let new_id = if transcript::is_uuid(&current) && current != from { current.clone() } else { uuid::Uuid::new_v4().to_string() };
                SessionArg::Fork { from, new_id }
            }
        };
        if prep.container.is_some() {
            return self.launch_claude_in_container(state, entry, prep, session, prompt, submit_prompt).await;
        }
        let Prepared { cwd, mut launch, project, provider, command, mut env, dir, token, add_dirs, .. } = prep;
        // The Workspace folders go on the command line only; the record keeps the choice.
        launch.add_dirs = add_dirs;
        let cfg = state.config.read().agents.clone();
        let claude_dir = transcript::claude_dir(env_value(&env, "CLAUDE_CONFIG_DIR").as_deref());

        // A session closed before its first prompt has no transcript: resuming it would
        // fail, so start it again under the same id.
        let (session, transcript_path) = match session {
            SessionArg::Resume(id) => {
                let here = transcript::transcript_path(&claude_dir, &cwd, &id);
                if here.is_file() {
                    (SessionArg::Resume(id), here)
                } else if let Some(elsewhere) = transcript::find_transcript(&claude_dir, &id) {
                    (SessionArg::Resume(id), elsewhere)
                } else {
                    (SessionArg::New(id), here)
                }
            }
            other => {
                let p = transcript::transcript_path(&claude_dir, &cwd, other.id());
                (other, p)
            }
        };
        // Tail from the end of what exists now, so old turns are not replayed. A fork's
        // new file starts with copied history: skip whatever it holds when it appears.
        let start_offset = match &session {
            SessionArg::Fork { .. } => None,
            _ => Some(std::fs::metadata(&transcript_path).map(|m| m.len()).unwrap_or(0)),
        };

        // Per-session files, 0600.
        let settings_path = dir.join("claude-settings.json");
        let mcp_path = dir.join("mcp.json");
        let helper = helper_exe().map(|exe| format!("{} statusline", shell_quote(&exe.to_string_lossy())));
        let statusline = cfg.statusline && {
            let mut dirs: Vec<&Path> = vec![&cwd];
            if let Some(p) = &project {
                dirs.push(&p.root);
            }
            !user_defines_statusline(&claude_dir, &dirs)
        };
        let base = state.local_base_url();
        let settings = session_settings(&format!("{base}/api/hooks/claude/{}", entry.id), &token, helper.as_deref(), statusline, permission_wait(&cfg));
        let mcp = mcp_config(&format!("{base}/mcp"), &token, &entry.id);
        {
            let (sp, mp) = (settings_path.clone(), mcp_path.clone());
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                util::fs::write_atomic(&sp, &serde_json::to_vec_pretty(&settings)?, 0o600)?;
                util::fs::write_atomic(&mp, &serde_json::to_vec_pretty(&mcp)?, 0o600)?;
                Ok(())
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .map_err(ApiError::from)?;
        }
        env.push(("WORKBENCH_AGENT_TOKEN".into(), Some(token)));

        let (argv_prompt, paste_prompt) = split_prompt(prompt, submit_prompt);
        let mut argv = build_argv(&command.to_string_lossy(), &session, &settings_path, &mcp_path, &launch, argv_prompt.as_deref());
        // Extra arguments from `[agents.providers.claude] args` go before the prompt.
        if !provider.args.is_empty() {
            let at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.splice(at..at, provider.args.iter().cloned());
        }
        // For display: our own config files by name, long values shortened.
        let display = display_argv(&argv, &[settings_path.display().to_string(), mcp_path.display().to_string()]);
        let session_id = session.id().to_string();
        let tp = transcript_path.display().to_string();
        self.update(entry, |r| {
            r.info.argv = display;
            r.transcript_path = Some(tp);
            if let Some(a) = r.info.agent.as_mut() {
                a.session_id = session_id;
                a.state = AgentState::Starting;
                a.attention = None;
                a.pending_permission = None;
                a.remote_control = launch.remote_control;
                if launch.remote_control {
                    a.remote_url = None;
                }
            }
            true
        });
        *entry.rt.lock() = AgentRt::default();
        let spec = pty::LaunchSpec { argv, cwd, env, cols: 0, rows: 0, redact: vec![] };
        self.launch(state, entry, spec, true).await?;
        spawn_tail(state, entry, transcript_path, start_offset);
        if let Some(p) = paste_prompt {
            deliver_later(state, entry, p, submit_prompt);
        }
        Ok(())
    }

    /// Claude Code in the project's dev container. Hooks and MCP reach Workbench through
    /// the bridge listener (`WORKBENCH_URL` inside); the per-session settings and MCP
    /// files are written into the container (0600, through stdin). The transcript lives
    /// in the container, so state comes from the hooks alone: no transcript tail (title,
    /// last message and context come only as far as hooks report them), no status line
    /// (the `workbench statusline` helper is a host binary), and `SessionStart` is posted
    /// by `curl` when the container has it.
    async fn launch_claude_in_container(
        &self,
        state: &AppState,
        entry: &Arc<Entry>,
        prep: Prepared,
        session: SessionArg,
        prompt: Option<String>,
        submit_prompt: bool,
    ) -> Result<(), ApiError> {
        let Prepared { cwd, mut launch, provider, command, mut env, dir, token, add_dirs, container, .. } = prep;
        let t = container.ok_or_else(|| ApiError::internal("no container"))?;
        let pid = t.project_id.clone();
        launch.add_dirs = add_dirs;
        let base = t.workbench_url.clone().ok_or_else(|| {
            ApiError::conflict("Workbench cannot listen on the container network's gateway, so an agent inside could not report to it")
        })?;
        // Resuming needs the transcript inside; without one, start again under the id.
        let session = match session {
            SessionArg::Resume(id) => {
                let found = crate::devcontainer::docker::exec(
                    &t.docker,
                    &t.container_id,
                    t.user.as_deref(),
                    &["/bin/sh", "-c", "ls \"${CLAUDE_CONFIG_DIR:-$HOME/.claude}\"/projects/*/\"$1\".jsonl >/dev/null 2>&1", "probe", &id],
                    Duration::from_secs(10),
                )
                .await
                .is_ok_and(|o| o.ok());
                if found { SessionArg::Resume(id) } else { SessionArg::New(id) }
            }
            other => other,
        };
        let files_dir = format!("/tmp/workbench-session-{}", entry.id);
        let settings_path = format!("{files_dir}/claude-settings.json");
        let mcp_path = format!("{files_dir}/mcp.json");
        let session_start = crate::devcontainer::container_has_curl(state, &pid).await.then(|| {
            "sh -c 'curl -fsS -m 3 -X POST -H \"Authorization: Bearer $WORKBENCH_AGENT_TOKEN\" -H \"Content-Type: application/json\" \
             --data-binary @- \"$WORKBENCH_URL/api/hooks/claude/$WORKBENCH_TERMINAL_ID\" >/dev/null 2>&1; true'"
                .to_string()
        });
        let wait = permission_wait(&state.config.read().agents);
        let mut settings = session_settings(&format!("{base}/api/hooks/claude/{}", entry.id), &token, session_start.as_deref(), false, wait);
        if session_start.is_none() {
            if let Some(h) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
                h.remove("SessionStart");
            }
        }
        let mcp = mcp_config(&format!("{base}/mcp"), &token, &entry.id);
        crate::devcontainer::write_into(&t, &settings_path, &serde_json::to_vec_pretty(&settings)?).await.map_err(ApiError::conflict)?;
        crate::devcontainer::write_into(&t, &mcp_path, &serde_json::to_vec_pretty(&mcp)?).await.map_err(ApiError::conflict)?;
        env.push(("WORKBENCH_AGENT_TOKEN".into(), Some(token)));

        let (argv_prompt, paste_prompt) = split_prompt(prompt, submit_prompt);
        let mut argv = build_argv(&command.to_string_lossy(), &session, Path::new(&settings_path), Path::new(&mcp_path), &launch, argv_prompt.as_deref());
        if !provider.args.is_empty() {
            let at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.splice(at..at, provider.args.iter().cloned());
        }
        let display = display_argv(&argv, &[settings_path.clone(), mcp_path.clone()]);
        let session_id = session.id().to_string();
        self.update(entry, |r| {
            r.info.argv = display;
            r.transcript_path = None;
            r.info.meta["container"] = t.describe();
            if let Some(a) = r.info.agent.as_mut() {
                a.session_id = session_id;
                a.state = AgentState::Starting;
                a.attention = None;
                a.pending_permission = None;
                a.remote_control = launch.remote_control;
                if launch.remote_control {
                    a.remote_url = None;
                }
            }
            true
        });
        *entry.rt.lock() = AgentRt::default();
        let spec = pty::LaunchSpec { argv, cwd, env, cols: 0, rows: 0, redact: vec![] };
        self.launch(state, entry, spec, true).await?;
        // Nothing to tail on this computer; the loop still watches the startup.
        spawn_tail(state, entry, dir.join("in-container.jsonl"), Some(0));
        if let Some(p) = paste_prompt {
            deliver_later(state, entry, p, submit_prompt);
        }
        Ok(())
    }

    async fn launch_codex(
        &self,
        state: &AppState,
        entry: &Arc<Entry>,
        prep: Prepared,
        cont: Continue,
        prompt: Option<String>,
        submit_prompt: bool,
    ) -> Result<(), ApiError> {
        let Prepared { cwd, launch, provider, command, mut env, token, add_dirs, .. } = prep;
        let home = codex::codex_home(env_value(&env, "CODEX_HOME").as_deref());
        let features = self.codex_features(&command).await;
        // Resuming needs the rollout; a session that never had a turn has none: start fresh.
        let (cont, rollout) = match cont {
            Continue::Resume(id) => {
                let suffix = format!("-{id}.jsonl");
                let known = entry.rec.lock().transcript_path.clone().map(PathBuf::from).filter(|p| p.is_file() && p.to_string_lossy().ends_with(&suffix));
                let found = match known {
                    Some(p) => Some(p),
                    None => {
                        let (h, i) = (home.clone(), id.clone());
                        tokio::task::spawn_blocking(move || codex::find_rollout(&h, &i)).await.ok().flatten()
                    }
                };
                match found {
                    Some(p) => (Continue::Resume(id), Some(p)),
                    None => (Continue::New, None),
                }
            }
            other => (other, None),
        };
        let fork_of = match &cont {
            Continue::Fork(from) => Some(from.clone()),
            _ => None,
        };
        let offset = rollout.as_ref().map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
        // The MCP bearer token is read from this variable by Codex (`bearer_token_env_var`).
        env.push(("WORKBENCH_AGENT_TOKEN".into(), Some(token)));
        let mcp_url = format!("{}/mcp", state.local_base_url());
        let (argv_prompt, paste_prompt) = split_prompt(prompt, submit_prompt);
        let args = LaunchArgs {
            model: launch.model.as_deref(),
            effort: launch.effort.as_deref(),
            permission: launch.permission_mode.as_deref().and_then(|m| provider.permission_preset(m)),
            add_dirs: &add_dirs,
            extra_args: &provider.args,
            mcp: Some((&mcp_url, &entry.id)),
            prompt: argv_prompt.as_deref(),
        };
        let argv = providers::codex_argv(&command.to_string_lossy(), &cont, features, &args);
        let display = display_argv(&argv, &[]);
        let session_id = match &cont {
            Continue::Resume(id) => id.clone(),
            _ => String::new(),
        };
        let now = util::now_ms();
        self.update(entry, |r| {
            r.info.argv = display;
            r.transcript_path = rollout.as_ref().map(|p| p.display().to_string());
            if let Some(a) = r.info.agent.as_mut() {
                a.session_id = session_id.clone();
                a.state = AgentState::Starting;
                a.attention = None;
                a.remote_control = false;
            }
            true
        });
        *entry.rt.lock() = AgentRt {
            pending_since: session_id.is_empty().then_some(now),
            fork_of: fork_of.clone(),
            home: Some(home.clone()),
            ..Default::default()
        };
        let spec = pty::LaunchSpec { argv, cwd, env, cols: 0, rows: 0, redact: vec![] };
        self.launch(state, entry, spec, true).await?;
        if let Some(pty) = entry.running_pty() {
            let watch = CodexWatch { home, path: rollout, offset, launched_at: now, fork: fork_of.is_some(), foreign: HashSet::new() };
            tokio::spawn(codex_loop(state.clone(), entry.clone(), pty, watch));
        }
        if let Some(p) = paste_prompt {
            deliver_later(state, entry, p, submit_prompt);
        }
        Ok(())
    }

    /// Aider: whether this start restores the chat history (`providers::aider_restores`).
    /// A first start notes the history file as it is before Aider runs.
    async fn aider_restore(&self, entry: &Entry, cwd: &Path, restarted: bool) -> bool {
        let (path, root) = providers::aider_history_path(cwd);
        let now = tokio::task::spawn_blocking(move || providers::FileMark::of(&path)).await.unwrap_or_default();
        let first = entry.rec.lock().aider_history.clone();
        let Some(first) = first.filter(|_| restarted) else {
            entry.rec.lock().aider_history = Some(now);
            entry.meta_dirty.store(true, Ordering::Relaxed);
            return false;
        };
        if !now.exists || now == first {
            return false;
        }
        let tracked = match root {
            Some(r) => git_tracks(&r, providers::AIDER_HISTORY).await,
            None => false,
        };
        providers::aider_restores(Some(&first), &now, tracked)
    }

    /// Kimi Code, Gemini CLI, Aider, custom CLIs (and Codex in a dev container): the
    /// command line, the activity heuristic and (Kimi) the session id from its index.
    /// Initial prompts are pasted once the program is up. `restarted`: Aider may restore
    /// the chat history it wrote for the session.
    #[allow(clippy::too_many_arguments)]
    async fn launch_plain(
        &self,
        state: &AppState,
        entry: &Arc<Entry>,
        prep: Prepared,
        cont: Continue,
        prompt: Option<String>,
        submit_prompt: bool,
        restarted: bool,
    ) -> Result<(), ApiError> {
        let Prepared { cwd, launch, provider, command, mut env, token, add_dirs, container, .. } = prep;
        let kind = provider.kind;
        // Not used by these CLIs themselves; there for a user's own MCP setup to reference.
        env.push(("WORKBENCH_AGENT_TOKEN".into(), Some(token)));
        let current = entry.rec.lock().info.agent.as_ref().map(|a| a.session_id.clone()).unwrap_or_default();
        let (cont, session_id) = match (kind, cont) {
            (ProviderKind::Kimi, Continue::Resume(id)) => (Continue::Resume(id.clone()), id),
            (ProviderKind::Gemini, Continue::Resume(id)) => {
                // A session that never started has no file (resuming it fails): start it
                // again under its id. In a dev container its files are out of sight.
                let exists = container.is_some() || {
                    let (g, dir, i) = (gemini::gemini_dir(env_value(&env, "GEMINI_CLI_HOME").as_deref()), cwd.clone(), id.clone());
                    tokio::task::spawn_blocking(move || gemini::session_exists(&g, &dir, &i)).await.unwrap_or(false)
                };
                if exists { (Continue::Resume(id.clone()), id) } else { (Continue::New, id) }
            }
            (ProviderKind::Gemini, _) => {
                let id = if transcript::is_uuid(&current) { current } else { uuid::Uuid::new_v4().to_string() };
                (Continue::New, id)
            }
            _ => (Continue::New, String::new()),
        };
        let args = LaunchArgs {
            model: launch.model.as_deref(),
            effort: None,
            permission: launch.permission_mode.as_deref().and_then(|m| provider.permission_preset(m)),
            add_dirs: &add_dirs,
            extra_args: &provider.args,
            mcp: None,
            prompt: None,
        };
        let command = command.to_string_lossy();
        let argv = match kind {
            ProviderKind::Kimi => providers::kimi_argv(&command, &cont, &args),
            ProviderKind::Gemini => providers::gemini_argv(&command, &cont, &session_id, &args),
            ProviderKind::Aider => providers::aider_argv(&command, self.aider_restore(entry, &cwd, restarted).await, &args),
            _ => providers::custom_argv(&command, &args),
        };
        let home = (kind == ProviderKind::Kimi).then(|| kimi::kimi_home(env_value(&env, "KIMI_CODE_HOME").as_deref()));
        let known = match (&home, session_id.is_empty()) {
            (Some(h), true) => {
                let h = h.clone();
                let ids: HashSet<String> =
                    tokio::task::spawn_blocking(move || kimi::load_index(&h).into_iter().map(|e| e.session_id).collect()).await.unwrap_or_default();
                Some(Arc::new(ids))
            }
            _ => None,
        };
        let kimi_watch = known.is_some().then(KimiWatch::default);
        let display = display_argv(&argv, &[]);
        let now = util::now_ms();
        self.update(entry, |r| {
            r.info.argv = display;
            r.transcript_path = None;
            if let Some(a) = r.info.agent.as_mut() {
                a.session_id = session_id.clone();
                a.state = AgentState::Starting;
                a.attention = None;
                a.remote_control = false;
            }
            true
        });
        *entry.rt.lock() = AgentRt { pending_since: known.is_some().then_some(now), home, kimi_known: known, ..Default::default() };
        let spec = pty::LaunchSpec { argv, cwd, env, cols: 0, rows: 0, redact: vec![] };
        self.launch(state, entry, spec, true).await?;
        if let Some(pty) = entry.running_pty() {
            tokio::spawn(activity_loop(state.clone(), entry.clone(), pty, kimi_watch, now));
        }
        if let Some(p) = prompt {
            deliver_later(state, entry, p, submit_prompt);
        }
        Ok(())
    }

    /// Optional flags the installed Codex supports, from its `--help` (cached per
    /// executable and modification time). Without an answer, none are used.
    async fn codex_features(&self, command: &Path) -> providers::CodexFeatures {
        let mtime = std::fs::metadata(command).and_then(|m| m.modified()).ok();
        if let Some(t) = mtime {
            if let Some((cached_t, f)) = self.codex_features.lock().get(command) {
                if *cached_t == t {
                    return *f;
                }
            }
        }
        let cwd = std::env::temp_dir();
        match util::proc::run(&command.to_string_lossy(), &["--help"], &cwd, Duration::from_secs(15)).await {
            Ok(out) if out.ok() => {
                let f = providers::CodexFeatures::from_help(&out.stdout);
                if let Some(t) = mtime {
                    self.codex_features.lock().insert(command.to_path_buf(), (t, f));
                }
                f
            }
            Ok(out) => {
                tracing::warn!("`{} --help` failed: {}", command.display(), transcript::clean_line(&out.message(), 200));
                providers::CodexFeatures::default()
            }
            Err(e) => {
                tracing::warn!("`{} --help` failed: {}", command.display(), e.message);
                providers::CodexFeatures::default()
            }
        }
    }

    // ------------------------------------------------------------ codex, kimi, activity

    /// Hosted sessions of `kind` and CLI home `home` still waiting for their id:
    /// `(pending, leader pid = process session id, entry)`. Sessions of another home
    /// (`CODEX_HOME`, `KIMI_CODE_HOME`) write elsewhere and never compete.
    fn pending_sessions(&self, kind: ProviderKind, home: &Path) -> Vec<(codex::Pending, i32, Arc<Entry>)> {
        self.all()
            .into_iter()
            .filter_map(|e| {
                let pty = e.running_pty()?;
                let pending = {
                    let r = e.rec.lock();
                    let a = r.info.agent.as_ref()?;
                    if a.provider != kind || !a.session_id.is_empty() {
                        return None;
                    }
                    let rt = e.rt.lock();
                    if rt.home.as_deref().is_some_and(|h| h != home) {
                        return None;
                    }
                    codex::Pending { terminal_id: e.id.clone(), cwd: r.info.cwd.clone(), launched_at: rt.pending_since?, fork_of: rt.fork_of.clone() }
                };
                Some((pending, pty.pid, e))
            })
            .collect()
    }

    /// Process session ids of the running hosted sessions of `kind`.
    fn agent_sids(&self, kind: ProviderKind) -> HashSet<i32> {
        self.all()
            .into_iter()
            .filter(|e| e.rec.lock().info.agent.as_ref().is_some_and(|a| a.provider == kind))
            .filter_map(|e| e.running_pty().map(|p| p.pid))
            .collect()
    }

    /// Session ids other terminals already have.
    fn claimed_ids(&self, except: &str) -> HashSet<String> {
        self.all()
            .into_iter()
            .filter(|e| e.id != except)
            .filter_map(|e| e.rec.lock().info.agent.as_ref().map(|a| a.session_id.clone()).filter(|s| !s.is_empty()))
            .collect()
    }

    /// The rollout of a new or forked Codex session, when the evidence is unambiguous: the
    /// file its own processes hold open, or else the only candidate while no other hosted
    /// session there waits, provided nothing outside the waiting sessions holds it or runs
    /// Codex in that folder (a Codex in another terminal or an editor writes rollouts
    /// there too). A candidate found to be someone else's is remembered in `w.foreign`.
    async fn codex_discover(&self, entry: &Entry, w: &mut CodexWatch) -> Option<(String, PathBuf)> {
        let pending: Vec<(codex::Pending, i32)> = self.pending_sessions(ProviderKind::Codex, &w.home).into_iter().map(|(p, pid, _)| (p, pid)).collect();
        let me = pending.iter().find(|(p, _)| p.terminal_id == entry.id)?.0.clone();
        let claimed = self.claimed_ids(&entry.id);
        let (home, foreign) = (w.home.clone(), w.foreign.clone());
        let (found, not_ours) = tokio::task::spawn_blocking(move || {
            let mut candidates = codex::recent_candidates(&home, me.launched_at);
            candidates.retain(|c| c.meta.cwd.trim_end_matches('/') == me.cwd.trim_end_matches('/') && !foreign.contains(&c.path));
            if candidates.is_empty() {
                return (None, None);
            }
            let sessions: Vec<(String, i32)> = pending.iter().map(|(p, pid)| (p.terminal_id.clone(), *pid)).collect();
            let paths: Vec<PathBuf> = candidates.iter().map(|c| c.path.clone()).collect();
            let held = codex::holders(&sessions, &paths);
            for c in &mut candidates {
                c.holders = held.get(&c.path).cloned().unwrap_or_default();
            }
            let ours: HashSet<i32> = pending.iter().map(|(_, pid)| *pid).collect();
            let all: Vec<codex::Pending> = pending.into_iter().map(|(p, _)| p).collect();
            match codex::choose(&me, &all, &candidates, &claimed) {
                codex::Choice::Certain(c) => (Some((c.meta.id.clone(), c.path.clone())), None),
                codex::Choice::Unproven(c) => {
                    if codex::held_elsewhere(&c.path, &ours) || pty::cli_running_in(Path::new(&me.cwd), "codex", "/@openai/codex/", &ours) {
                        (None, Some(c.path.clone()))
                    } else {
                        (Some((c.meta.id.clone(), c.path.clone())), None)
                    }
                }
                codex::Choice::Unknown => (None, None),
            }
        })
        .await
        .ok()?;
        if let Some(p) = not_ours {
            tracing::debug!(terminal = %entry.id, "codex rollout {} belongs to a session outside Workbench", p.display());
            w.foreign.insert(p);
        }
        found
    }

    /// The id of a new Kimi session: the new index entry of its folder that the evidence
    /// gives it (`kimi::assign`: its own screen shows the id, or it is the only entry it can
    /// have created and no other waiting hosted session of that folder and Kimi home could
    /// have). Without a screen, only while no Kimi outside the hosted sessions runs in that
    /// folder: one in another terminal writes entries there too.
    async fn kimi_discover(&self, entry: &Arc<Entry>, w: &mut KimiWatch) -> Option<String> {
        let home = entry.rt.lock().home.clone()?;
        let waiting = self.pending_sessions(ProviderKind::Kimi, &home);
        let cwd = waiting.iter().find(|(p, _, _)| p.terminal_id == entry.id)?.0.cwd.clone();
        let waiting: Vec<(String, Arc<HashSet<String>>, Arc<Entry>)> = waiting
            .into_iter()
            .filter(|(p, _, _)| p.cwd.trim_end_matches('/') == cwd.trim_end_matches('/'))
            .filter_map(|(p, _, e)| {
                let known = e.rt.lock().kimi_known.clone()?;
                Some((p.terminal_id, known, e))
            })
            .collect();
        let claimed = self.claimed_ids(&entry.id);
        let ours = self.agent_sids(ProviderKind::Kimi);
        let (me, ambiguous) = (entry.id.clone(), w.ambiguous.clone());
        let (found, unsure) = tokio::task::spawn_blocking(move || {
            let entries = kimi::load_index(&home);
            let waiting: Vec<kimi::Waiting> = waiting
                .iter()
                .map(|(t, known, e)| kimi::Waiting {
                    terminal_id: t.clone(),
                    known: known.clone(),
                    on_screen: kimi::id_on_screen(&pty::screen_text(&mut e.screen.mirror(), 80)),
                })
                .collect();
            match kimi::assign(&cwd, &entries, &waiting, &claimed).remove(&me) {
                Some((id, kimi::Evidence::Screen)) => (Some(id), None),
                Some((id, kimi::Evidence::Elimination)) => {
                    if ambiguous.contains(&id) {
                        (None, None)
                    } else if pty::cli_running_in(Path::new(&cwd), "kimi", "/@moonshot-ai/kimi-code/", &ours) {
                        (None, Some(id))
                    } else {
                        (Some(id), None)
                    }
                }
                None => (None, None),
            }
        })
        .await
        .ok()?;
        if let Some(id) = unsure {
            tracing::debug!(terminal = %entry.id, "kimi session {id} may belong to a Kimi outside Workbench");
            w.ambiguous.insert(id);
        }
        found
    }

    /// Record a discovered session id (and the file it writes).
    fn set_discovered(&self, entry: &Entry, id: String, path: Option<String>) {
        entry.rt.lock().pending_since = None;
        self.update(entry, |r| {
            if let Some(p) = path {
                r.transcript_path = Some(p);
            }
            match r.info.agent.as_mut() {
                Some(a) if a.session_id != id => {
                    a.session_id = id;
                    true
                }
                _ => false,
            }
        });
    }

    fn apply_codex(&self, entry: &Entry, events: Vec<codex::Event>) {
        if events.is_empty() {
            return;
        }
        let now = util::now_ms();
        let mut rec = entry.rec.lock();
        let title_locked = rec.title_locked;
        let mut new_title = None;
        let out = {
            let Some(agent) = rec.info.agent.as_mut() else { return };
            // Untitled sessions take their first prompt as title.
            if agent.title.is_none() && !title_locked {
                let first = events.iter().find_map(|e| match e {
                    codex::Event::UserMessage(m) if !codex::injected(m) => Some(title_from_prompt(m)),
                    _ => None,
                });
                if let Some(t) = first.filter(|t| !t.is_empty()) {
                    agent.title = Some(t.clone());
                    new_title = Some(t);
                }
            }
            let mut rt = entry.rt.lock();
            let out = codex::apply(agent, &mut rt.codex, events, now);
            if out.turn_ended {
                rt.turn_done = true;
            }
            out
        };
        let mut changed = out.changed;
        if let Some(t) = new_title {
            rec.info.title = t;
            changed = true;
        }
        if changed {
            entry.meta_dirty.store(true, Ordering::Relaxed);
            self.emit_locked("terminal.updated", entry, &rec);
        }
        if out.attention {
            self.emit_attention_locked(entry, &rec);
        }
    }

    /// A dialog of a Codex, Kimi, Gemini, Aider or custom CLI on the session's screen right now (Claude
    /// Code's dialogs come through hooks).
    pub(crate) fn screen_dialog(&self, entry: &Entry) -> Option<(AgentState, &'static str)> {
        let kind = entry.rec.lock().info.agent.as_ref()?.provider;
        if kind == ProviderKind::Claude {
            return None;
        }
        let text = pty::visible_text(&mut entry.screen.mirror(), DIALOG_ROWS);
        providers::dialog_on_screen(kind, &text)
    }

    /// Why text must not be typed into this agent session now (`None`: it may, and always
    /// for other terminals): its state says it shows a dialog or is starting, or a dialog
    /// of its CLI is on screen (checked now, not at the watcher's last look). Into a
    /// dialog, the Enter after a paste picks the highlighted choice ("Yes, proceed").
    pub(crate) fn prompt_refusal(&self, entry: &Entry) -> Option<&'static str> {
        let st = entry.rec.lock().info.agent.as_ref().map(|a| a.state)?;
        input_refusal(st).or_else(|| self.screen_dialog(entry).and_then(|(s, _)| input_refusal(s)))
    }

    /// Whether `ask` may pick this session by itself (no terminal named). Claude Code:
    /// when its state takes a prompt (hooks report its dialogs). Codex and Kimi: only idle
    /// after a turn of this process ended, with no dialog of theirs on screen; neither the
    /// rollout nor the output heuristic sees their dialogs, and right after startup a trust
    /// or sign-in screen may be up. Custom CLIs: never, nothing tells their dialogs from
    /// their prompt.
    fn takes_prompt_unasked(&self, e: &Entry) -> bool {
        let (kind, st) = {
            let r = e.rec.lock();
            let Some(a) = r.info.agent.as_ref() else { return false };
            (a.provider, a.state)
        };
        match kind {
            ProviderKind::Claude => accepts_prompt(st),
            ProviderKind::Codex | ProviderKind::Kimi | ProviderKind::Gemini | ProviderKind::Aider => {
                let settled = {
                    let rt = e.rt.lock();
                    rt.turn_done && rt.screen_dialog.is_none()
                };
                matches!(st, AgentState::Idle | AgentState::Error) && settled && self.screen_dialog(e).is_none()
            }
            ProviderKind::Custom => false,
        }
    }

    /// Follow a CLI's dialogs on its screen (Codex, Kimi and custom CLIs report none): the
    /// session needs permission or input while one shows (with an attention event), and
    /// returns to `after` once it is gone (`None`: for Codex, working while its turn is
    /// open, else idle, or starting when the dialog came up during startup).
    fn check_screen_dialog(&self, entry: &Entry, after: Option<AgentState>) {
        let seen = self.screen_dialog(entry);
        let now = util::now_ms();
        let mut rec = entry.rec.lock();
        let Some(a) = rec.info.agent.as_mut() else { return };
        let mut rt = entry.rt.lock();
        let interruptible = |s: AgentState| matches!(s, AgentState::Starting | AgentState::Idle | AgentState::Working);
        let mut attention = false;
        match (seen, rt.screen_dialog) {
            (None, None) => return,
            (Some((s, note)), prev) => {
                rt.dialog_misses = 0;
                let before = match prev {
                    // Still up: only undo, quietly, a state a watcher set meanwhile.
                    Some((p, before)) if p == s => {
                        if a.state == s || !interruptible(a.state) {
                            return;
                        }
                        before
                    }
                    // Another dialog replaced it.
                    Some((p, before)) if a.state == p || interruptible(a.state) => {
                        attention = true;
                        before
                    }
                    None if interruptible(a.state) => {
                        attention = true;
                        a.state
                    }
                    // Never over an error, an exit or what hooks reported.
                    _ => return,
                };
                rt.screen_dialog = Some((s, before));
                a.state = s;
                a.attention = Some(note.to_string());
                a.last_event_at = now;
            }
            (None, Some((p, before))) => {
                // A redraw can hide it for a moment: gone only on the second look.
                rt.dialog_misses += 1;
                if rt.dialog_misses < 2 {
                    return;
                }
                rt.dialog_misses = 0;
                rt.screen_dialog = None;
                if a.state != p {
                    return;
                }
                a.state = after.unwrap_or(if before == AgentState::Starting {
                    AgentState::Starting
                } else if rt.codex.turn_open {
                    AgentState::Working
                } else {
                    AgentState::Idle
                });
                a.attention = None;
                a.last_event_at = now;
            }
        }
        drop(rt);
        entry.meta_dirty.store(true, Ordering::Relaxed);
        self.emit_locked("terminal.updated", entry, &rec);
        if attention {
            self.emit_attention_locked(entry, &rec);
        }
    }

    /// Heuristic state changes (Starting / Idle / Working only: never over a state a
    /// better source set, never after exit).
    fn set_activity_state(&self, entry: &Entry, s: AgentState) -> bool {
        let now = util::now_ms();
        self.update(entry, |r| match r.info.agent.as_mut() {
            Some(a) if a.state != s && matches!(a.state, AgentState::Starting | AgentState::Idle | AgentState::Working) => {
                a.state = s;
                a.last_event_at = now;
                true
            }
            _ => false,
        })
    }
    // ------------------------------------------------------------ hooks & transcript

    pub(crate) fn apply_hook(&self, id: &str, v: &Value) -> Result<(), ApiError> {
        let entry = self.require(id)?;
        let now = util::now_ms();
        let mut rec = entry.rec.lock();
        let Some(agent) = rec.info.agent.as_mut() else { return Ok(()) };
        let out = {
            let mut rt = entry.rt.lock();
            hooks::apply_hook(agent, &mut rt, v, now)
        };
        let changed = note_transcript(&mut rec, out.transcript_path.clone()) || out.changed;
        if changed {
            entry.meta_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            self.emit_locked("terminal.updated", &entry, &rec);
        }
        if out.attention {
            self.emit_attention_locked(&entry, &rec);
        }
        drop(rec);
        if out.session_started {
            entry.started.notify_one();
        }
        Ok(())
    }

    /// A `PermissionRequest` hook. When Workbench may answer it (`[agents]
    /// answer_permissions`, a running session, a tool the terminal need not answer), the
    /// hook's response is held until a device answers or the request stops being pending
    /// (`permission`); the response then carries the decision, or none. Claude shows its
    /// own dialog meanwhile, and the first answer wins.
    pub(crate) async fn permission_request(&self, state: &AppState, id: &str, v: &Value) -> Result<Value, ApiError> {
        let entry = self.require(id)?;
        let wait = permission_wait(&state.config.read().agents);
        let answerable = wait.is_some() && permission::answerable(v) && entry.running_pty().is_some();
        let secrets = entry.redact.lock().clone();
        let now = util::now_ms();
        let held = {
            let mut rec = entry.rec.lock();
            let Some(agent) = rec.info.agent.as_mut() else { return Ok(json!({})) };
            let mut rt = entry.rt.lock();
            let out = hooks::apply_hook(agent, &mut rt, v, now);
            // The session's own secrets (project `[agent].env`) are masked as well.
            if let Some(a) = agent.attention.as_mut() {
                *a = crate::secrets::redact(a, &secrets);
            }
            let held = answerable.then(|| {
                let (req, rx) = permission::Request::new(new_permission_id(), v, out.permission_call.clone(), &secrets, now);
                let info = req.info.clone();
                rt.permissions.push(req);
                (info, rx)
            });
            hooks::sync_pending(agent, &rt);
            drop(rt);
            note_transcript(&mut rec, out.transcript_path.clone());
            entry.meta_dirty.store(true, Ordering::Relaxed);
            self.emit_locked("terminal.updated", &entry, &rec);
            match &held {
                // Every answerable request is announced (a toast with Allow / Deny).
                Some((info, _)) => self.emit_permission_attention_locked(&entry, &rec, info),
                None if out.attention => self.emit_attention_locked(&entry, &rec),
                None => {}
            }
            held
        };
        let Some((info, rx)) = held else { return Ok(json!({})) };
        // Claude shows the dialog as it sends the hook: it may be up already (a quick
        // answer in the terminal may come before the next periodic look).
        entry.look_for_permission_dialog();
        let secs = Duration::from_secs(wait.unwrap_or(permission::MIN_WAIT_SECS));
        // A background subagent's dialog waits for its hooks: never hold it long.
        let limit = if permission::from_subagent(v) { secs.min(permission::SUBAGENT_WAIT) } else { secs };
        let _held = HeldPermission { state: state.clone(), entry: entry.clone(), id: info.id.clone() };
        match tokio::time::timeout(limit, rx).await {
            Ok(Ok(decision)) => Ok(decision),
            // Answered elsewhere, the session moved on, or the wait ran out: no decision,
            // Claude's own prompt decides.
            _ => Ok(json!({})),
        }
    }

    /// A device answers a pending permission request (`POST /api/agents/{id}/permission`).
    /// `409 not_pending` when it is no longer pending.
    ///
    /// Sending the answer is not the session going on: Claude drops a hook's allow for
    /// tools that need the user's own interaction, and after it answered in the terminal.
    /// While the dialog it answered is still on screen the session stays "needs
    /// permission" and the screen check watches it (`permission::Look`).
    pub(crate) fn answer_permission(&self, id: &str, request_id: &str, d: &permission::Decision) -> Result<TerminalInfo, ApiError> {
        let entry = self.require(id)?;
        let gone = || {
            ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "not_pending",
                "This permission request is no longer pending: it was answered in the terminal, timed out, or the session moved on",
            )
        };
        // Before the answer goes out (Claude closes its dialog once it takes it).
        let dialog_up = entry.running_pty().is_some() && permission_dialog_on_screen(&entry);
        let mut rec = entry.rec.lock();
        let Some(agent) = rec.info.agent.as_mut() else { return Err(ApiError::bad_request("that terminal is not an agent session")) };
        let mut rt = entry.rt.lock();
        let first = rt.permissions.current().is_some_and(|p| p.id == request_id);
        let req = rt.permissions.take(request_id).ok_or_else(gone)?;
        let sent = req.answer(d);
        if sent.is_ok() && first {
            match rt.permissions.current() {
                // The session shows its next dialog.
                Some(next) => {
                    rt.pending_permission = Some(rt.permissions.current_tool_use().unwrap_or_else(|| "?".into()));
                    agent.attention = Some(next.summary);
                }
                // Its dialog is up: the session goes on once it closes.
                None if dialog_up && agent.state == AgentState::NeedsPermission => {
                    let key = rt.pending_permission.get_or_insert_with(|| "?".into()).clone();
                    rt.permissions.await_answer(key);
                    agent.attention = Some(permission::ANSWER_SENT.into());
                }
                None => {
                    rt.pending_permission = None;
                    if agent.state == AgentState::NeedsPermission {
                        agent.state = AgentState::Working;
                        agent.attention = None;
                    }
                }
            }
            agent.last_event_at = util::now_ms();
        }
        hooks::sync_pending(agent, &rt);
        drop(rt);
        entry.meta_dirty.store(true, Ordering::Relaxed);
        self.emit_locked("terminal.updated", &entry, &rec);
        let info = entry.info_locked(&rec);
        drop(rec);
        // The hook call had already ended (Claude gave up on it).
        sent.map_err(|_| gone())?;
        tracing::debug!(terminal = %id, "permission request answered from Workbench");
        Ok(info)
    }

    /// A held request's handler ended without an answer from Workbench (timed out, or
    /// Claude hung up): it is no longer answerable here. The terminal still asks.
    fn forget_permission(&self, entry: &Entry, request_id: &str) {
        let mut rec = entry.rec.lock();
        let Some(agent) = rec.info.agent.as_mut() else { return };
        let mut rt = entry.rt.lock();
        if !rt.permissions.remove(request_id) {
            return;
        }
        hooks::sync_pending(agent, &rt);
        drop(rt);
        entry.meta_dirty.store(true, Ordering::Relaxed);
        self.emit_locked("terminal.updated", entry, &rec);
    }

    /// HEURISTIC, one look every 500 ms while a request is pending or an answer awaited
    /// (`permission::Queue::observe_screen`): the first request's dialog was seen and is
    /// gone (answered in the terminal: an allowed tool may run for long before its
    /// `PostToolUse`); a dialog answered from Workbench closed (the session goes on), or
    /// is still up seconds later (Claude did not take the answer: the terminal must).
    fn check_permission_dialog(&self, entry: &Entry) {
        if !entry.rt.lock().permissions.watching() {
            return;
        }
        let visible = permission_dialog_on_screen(entry);
        let mut rec = entry.rec.lock();
        let Some(agent) = rec.info.agent.as_mut() else { return };
        let mut rt = entry.rt.lock();
        // An awaited answer whose prompt the session no longer shows was settled by a hook.
        if rt.permissions.awaiting().is_some_and(|k| rt.pending_permission.as_deref() != Some(k)) {
            rt.permissions.drop_awaiting();
        }
        let mut attention = false;
        match rt.permissions.observe_screen(visible) {
            permission::Look::Nothing => return,
            permission::Look::AnsweredInTerminal => {
                tracing::debug!(terminal = %entry.id, "permission dialog answered in the terminal");
                match rt.permissions.current() {
                    Some(next) => {
                        rt.pending_permission = Some(rt.permissions.current_tool_use().unwrap_or_else(|| "?".into()));
                        agent.attention = Some(next.summary);
                    }
                    None => {
                        rt.pending_permission = None;
                        if agent.state == AgentState::NeedsPermission {
                            agent.state = AgentState::Working;
                            agent.attention = None;
                        }
                    }
                }
            }
            permission::Look::Closed => {
                tracing::debug!(terminal = %entry.id, "the dialog answered from Workbench closed");
                rt.pending_permission = None;
                if agent.state == AgentState::NeedsPermission {
                    agent.state = AgentState::Working;
                    agent.attention = None;
                }
            }
            permission::Look::Ignored => {
                tracing::debug!(terminal = %entry.id, "the dialog answered from Workbench is still up");
                if agent.state != AgentState::NeedsPermission {
                    return;
                }
                agent.attention = Some(permission::ANSWER_IGNORED.into());
                attention = true;
            }
        }
        hooks::sync_pending(agent, &rt);
        drop(rt);
        entry.meta_dirty.store(true, Ordering::Relaxed);
        self.emit_locked("terminal.updated", entry, &rec);
        if attention {
            self.emit_attention_locked(entry, &rec);
        }
    }

    pub(crate) fn apply_status(&self, id: &str, v: &Value) -> Result<(), ApiError> {
        let entry = self.require(id)?;
        let mut rec = entry.rec.lock();
        let Some(agent) = rec.info.agent.as_mut() else { return Ok(()) };
        if hooks::apply_status(agent, v) {
            entry.meta_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            self.emit_locked("terminal.updated", &entry, &rec);
        }
        Ok(())
    }

    fn apply_records(&self, entry: &Entry, records: Vec<Record>) {
        let mut rec = entry.rec.lock();
        let title_locked = rec.title_locked;
        let mut new_title: Option<String> = None;
        let mut changed = false;
        let mut attention = false;
        {
            let Some(agent) = rec.info.agent.as_mut() else { return };
            let mut rt = entry.rt.lock();
            let set_state = |agent: &mut AgentInfo, s: AgentState, changed: &mut bool| {
                if agent.state != s {
                    agent.state = s;
                    *changed = true;
                }
            };
            for r in records {
                match r {
                    Record::Title(t) => {
                        if agent.title.as_deref() != Some(t.as_str()) {
                            agent.title = Some(t.clone());
                            new_title = Some(t);
                            changed = true;
                        }
                    }
                    Record::RemoteUrl(u) => {
                        if agent.remote_url.as_deref() != Some(u.as_str()) {
                            agent.remote_url = Some(u);
                            changed = true;
                        }
                    }
                    Record::PermissionMode(m) => {
                        if agent.permission_mode.as_deref() != Some(m.as_str()) {
                            agent.permission_mode = Some(m);
                            changed = true;
                        }
                    }
                    Record::Model(m) => {
                        if agent.model.is_none() {
                            agent.model = Some(m);
                            changed = true;
                        }
                    }
                    Record::Cost(c) => {
                        let c = (c * 1000.0).round() / 1000.0;
                        if agent.cost_usd.is_none_or(|old| c > old) {
                            agent.cost_usd = Some(c);
                            changed = true;
                        }
                    }
                    Record::ApiError { message, final_attempt } => {
                        if final_attempt {
                            set_state(agent, AgentState::Error, &mut changed);
                            agent.attention = Some(message);
                            attention = true;
                        } else {
                            agent.attention = Some(format!("API error, retrying: {message}"));
                        }
                        changed = true;
                    }
                    Record::TurnStarted => {
                        if !rt.hooks_seen {
                            set_state(agent, AgentState::Working, &mut changed);
                        }
                    }
                    Record::TurnCompleted { id, text } => {
                        if id.is_empty() || rt.last_completed_id.as_deref() != Some(id.as_str()) {
                            rt.last_completed_id = Some(id);
                            let tail = transcript::tail_chars(&text, 600);
                            if agent.last_message.as_deref() != Some(tail.as_str()) {
                                agent.last_message = Some(tail);
                                changed = true;
                            }
                            if !rt.hooks_seen {
                                set_state(agent, AgentState::Idle, &mut changed);
                                agent.unread = true;
                                agent.attention = None;
                                attention = true;
                            }
                        }
                    }
                    Record::TurnEnded => {
                        if !rt.hooks_seen && agent.state == AgentState::Working {
                            set_state(agent, AgentState::Idle, &mut changed);
                        }
                    }
                    Record::Interrupted { at } => {
                        // Hooks do not report interrupts; ignore one older than the last hook.
                        let stale = rt.hooks_seen && at.is_some_and(|t| t < agent.last_event_at);
                        let busy = matches!(agent.state, AgentState::Working | AgentState::NeedsPermission | AgentState::NeedsInput);
                        if busy && !stale {
                            rt.pending_permission = None;
                            rt.permissions.clear();
                            agent.attention = None;
                            set_state(agent, AgentState::Idle, &mut changed);
                        }
                    }
                    Record::ToolResult { tool_use_id } => {
                        rt.open_tools.finished(&tool_use_id);
                        let answered = rt.permissions.resolve_tool_use(&tool_use_id);
                        // Declined in the terminal with feedback: no hook reports it and
                        // the turn goes on; the prompt it waited on is gone.
                        if rt.hooks_seen && rt.pending_permission.as_deref() == Some(tool_use_id.as_str()) {
                            match rt.permissions.current() {
                                Some(next) => {
                                    rt.pending_permission = Some(rt.permissions.current_tool_use().unwrap_or_else(|| "?".into()));
                                    agent.attention = Some(next.summary);
                                }
                                None => {
                                    rt.pending_permission = None;
                                    if agent.state == AgentState::NeedsPermission {
                                        agent.attention = None;
                                        set_state(agent, AgentState::Working, &mut changed);
                                    }
                                }
                            }
                            changed = true;
                        } else if answered {
                            changed = true;
                        }
                    }
                }
            }
            if hooks::sync_pending(agent, &rt) {
                changed = true;
            }
        }
        if let Some(t) = new_title {
            if !title_locked && rec.info.title != t {
                rec.info.title = t;
            }
        }
        if changed {
            entry.meta_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            self.emit_locked("terminal.updated", entry, &rec);
        }
        if attention {
            self.emit_attention_locked(entry, &rec);
        }
    }

    /// A new session in an untrusted folder waits on Claude's "trust this folder?" dialog
    /// before any hook fires. Surface it as needing the user.
    ///
    /// Also a fallback for setups where hooks cannot run (e.g. `disableAllHooks`): a
    /// session still "starting" after `started_for` with a quiet screen is idle.
    fn check_startup(&self, entry: &Entry, started_for: Duration) {
        let (state, flagged, hooks_seen) = {
            let r = entry.rec.lock();
            let Some(a) = r.info.agent.as_ref() else { return };
            let rt = entry.rt.lock();
            (a.state, rt.trust_prompt, rt.hooks_seen)
        };
        if !(state == AgentState::Starting || (flagged && state == AgentState::NeedsInput)) {
            return;
        }
        let text = pty::screen_text(&mut entry.screen.mirror(), 40);
        let visible = text.contains("trust this folder") || text.contains("Do you trust the files");
        if visible == flagged {
            let quiet = util::now_ms() - entry.screen.last_output_at.load(std::sync::atomic::Ordering::Relaxed) > 3000;
            if !visible && !hooks_seen && state == AgentState::Starting && started_for > Duration::from_secs(20) && quiet {
                self.update(entry, |r| match r.info.agent.as_mut() {
                    Some(a) if a.state == AgentState::Starting => {
                        a.state = AgentState::Idle;
                        true
                    }
                    _ => false,
                });
            }
            return;
        }
        let mut rec = entry.rec.lock();
        let Some(a) = rec.info.agent.as_mut() else { return };
        entry.rt.lock().trust_prompt = visible;
        if visible {
            a.state = AgentState::NeedsInput;
            a.attention = Some("Claude asks whether to trust this folder — answer in the terminal".into());
        } else if a.state == AgentState::NeedsInput {
            a.state = AgentState::Starting;
            a.attention = None;
        }
        entry.meta_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        self.emit_locked("terminal.updated", entry, &rec);
        if visible {
            self.emit_attention_locked(entry, &rec);
        }
    }

    fn set_remote_url(&self, entry: &Entry, url: String) {
        self.update(entry, |r| match r.info.agent.as_mut() {
            Some(a) if a.remote_url.as_deref() != Some(url.as_str()) => {
                a.remote_url = Some(url);
                true
            }
            _ => false,
        });
    }

    // ------------------------------------------------------------ ask

    /// The project's most recently active running agent that `ask` may pick by itself now
    /// (`takes_prompt_unasked`), of the given provider (`None`: any).
    fn most_recent_agent(&self, project_id: &str, provider: Option<&str>) -> Option<Arc<Entry>> {
        self.all()
            .into_iter()
            .filter(|e| e.running_pty().is_some())
            .filter_map(|e| {
                let key = {
                    let r = e.rec.lock();
                    let a = r.info.agent.as_ref()?;
                    let of_provider = provider.is_none_or(|p| a.provider_id.as_deref().unwrap_or("claude") == p);
                    (r.info.open && r.info.project_id.as_deref() == Some(project_id) && of_provider)
                        .then(|| a.last_event_at.max(e.screen.last_output_at.load(Ordering::Relaxed)))?
                };
                self.takes_prompt_unasked(&e).then_some((key, e))
            })
            .max_by_key(|(k, _)| *k)
            .map(|(_, e)| e)
    }

    pub(crate) async fn ask(&self, state: &AppState, req: AskRequest) -> Result<TerminalInfo, ApiError> {
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(ApiError::bad_request("prompt is empty"));
        }
        let submit = req.submit.unwrap_or(true);
        if let Some(tid) = req.terminal_id.as_deref().filter(|t| !t.is_empty()) {
            let e = self.require(tid)?;
            if e.kind() != TerminalKind::Agent {
                return Err(ApiError::bad_request("that terminal is not an agent session"));
            }
            if e.running_pty().is_none() {
                return Err(ApiError::conflict("that session is not running; resume it first"));
            }
            let showing = || {
                let st = e.rec.lock().info.agent.as_ref().map(|a| a.state);
                st.is_some_and(|s| !accepts_prompt(s)) || self.screen_dialog(&e).is_some()
            };
            let refused = || ApiError::conflict("that session is showing a prompt (permission, question or trust); answer it first");
            if showing() {
                return Err(refused());
            }
            if !e.wait_typing_pause(Duration::from_millis(2500), Duration::from_secs(8)).await {
                return Err(ApiError::conflict("you are typing in that session; try again in a moment"));
            }
            // It may have opened a dialog while we waited.
            if showing() {
                return Err(refused());
            }
            self.send_text(tid, &prompt, submit).await?;
            return Ok(e.info());
        }
        let pid = req.project_id.clone().filter(|p| !p.is_empty()).ok_or_else(|| ApiError::bad_request("projectId is required"))?;
        let provider = req.provider.as_deref().map(str::trim).filter(|p| !p.is_empty());
        if !req.new_session {
            if let Some(e) = self.most_recent_agent(&pid, provider) {
                // Never interleave with what the user is typing there.
                if !e.wait_typing_pause(Duration::from_millis(2500), Duration::from_secs(8)).await {
                    return Err(ApiError::conflict("you are typing in that session; try again in a moment"));
                }
                // Unless it opened a dialog meanwhile (then a new session takes the prompt).
                if self.takes_prompt_unasked(&e) {
                    self.send_text(&e.id, &prompt, submit).await?;
                    self.update(&e, |r| {
                        let was = r.info.open;
                        r.info.open = true;
                        !was
                    });
                    return Ok(e.info());
                }
            }
        }
        let request = AgentRequest {
            project_id: pid,
            provider: provider.map(str::to_string),
            prompt: Some(prompt.clone()),
            name: req.name.clone(),
            ..Default::default()
        };
        if submit {
            return self.spawn_agent(state, request).await;
        }
        // Not submitted: start empty and paste once the session takes input.
        let info = self.spawn_agent(state, AgentRequest { prompt: None, ..request }).await?;
        if let Some(e) = self.get(&info.id) {
            deliver_later(state, &e, prompt, false);
        }
        Ok(info)
    }

    /// When `terminal_id` is a hosted agent session whose CLI runs its commands in a
    /// sandbox (`Provider::sandboxed`: Codex unless it bypasses it), how to name it in a
    /// refusal; `None` otherwise. Workbench must not run commands for such a session
    /// outside that sandbox: MCP `run_start` refuses it.
    pub fn sandboxed_agent(&self, state: &AppState, terminal_id: &str) -> Option<String> {
        let e = self.get(terminal_id)?;
        let (kind, provider_id, mode) = {
            let r = e.rec.lock();
            let a = r.info.agent.as_ref()?;
            (a.provider, a.provider_id.clone().unwrap_or_else(|| "claude".into()), a.permission_mode.clone())
        };
        let cfg = state.config.read().agents.clone();
        let (sandboxed, label) = match providers::find(&cfg, Some(&provider_id)) {
            Some(p) if p.kind == kind => (p.sandboxed(mode.as_deref()), p.label),
            // Its provider is gone from config.toml: judge by the kind and mode alone.
            _ => {
                let dangerous = mode.as_deref().and_then(|m| kind.permission_modes().iter().find(|x| x.id == m)).is_some_and(|x| x.dangerous);
                (kind == ProviderKind::Codex && !dangerous, provider_id)
            }
        };
        let preset = mode.as_deref().and_then(|m| kind.permission_modes().iter().find(|x| x.id == m)).map(|x| x.label);
        sandboxed.then(|| match preset {
            Some(p) => format!("This {label} session ({p})"),
            None => format!("This {label} session"),
        })
    }

    // ------------------------------------------------------------ remote control

    pub(crate) async fn spawn_remote_control(&self, state: &AppState, req: RemoteControlRequest) -> Result<TerminalInfo, ApiError> {
        let project = state.projects.require(&req.project_id)?;
        let mode = req.spawn.as_deref().unwrap_or("same-dir");
        if !["same-dir", "worktree", "session"].contains(&mode) {
            return Err(ApiError::bad_request("spawn must be same-dir, worktree or session"));
        }
        let command = {
            let cfg = state.config.read();
            resolve_command(&cfg.agents.command).ok_or_else(|| ApiError::not_configured("Claude Code was not found; set [agents].command"))?
        };
        let mut argv = vec![command.to_string_lossy().into_owned(), "remote-control".into(), "--spawn".into(), mode.into()];
        let name = non_empty(req.name.clone()).map(|n| transcript::clean_line(&n, 60));
        if let Some(n) = &name {
            argv.extend(["--name".into(), n.clone()]);
        }
        if let Some(p) = non_empty(req.permission_mode.clone()) {
            if !PERMISSION_MODES.contains(&p.as_str()) {
                return Err(ApiError::bad_request("invalid permission mode"));
            }
            argv.extend(["--permission-mode".into(), p]);
        }
        let title = match &name {
            Some(n) => format!("Remote Control · {n}"),
            None => "Remote Control".into(),
        };
        let info = self
            .spawn(
                state,
                super::SpawnSpec {
                    kind: TerminalKind::Command,
                    title,
                    project_id: Some(project.id.clone()),
                    cwd: project.root.clone(),
                    argv,
                    env: vec![],
                    cols: Some(120),
                    rows: Some(32),
                    meta: json!({ "remoteControlServer": true, "spawn": mode, "urls": [] }),
                },
            )
            .await?;
        if let Some(e) = self.get(&info.id) {
            watch_remote_control(state, &e);
        }
        Ok(info)
    }

    fn add_remote_urls(&self, entry: &Entry, found: Vec<String>) {
        if found.is_empty() {
            return;
        }
        self.update(entry, |r| {
            let mut urls: Vec<String> = r
                .info
                .meta
                .get("urls")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let before = urls.len();
            for u in found {
                if !urls.contains(&u) && urls.len() < 10 {
                    urls.push(u);
                }
            }
            if urls.len() == before {
                return false;
            }
            r.info.meta["urls"] = json!(urls);
            true
        });
    }

    // ------------------------------------------------------------ listings

    /// Past conversations of a provider (`None`: Claude Code) in a project, newest first.
    /// Read-only: Claude's transcripts, Codex's rollouts, Kimi's session index.
    pub(crate) async fn history(&self, state: &AppState, project_id: &str, provider: Option<&str>, limit: usize) -> Result<Vec<HistoryEntry>, ApiError> {
        let project = state.projects.require(project_id)?;
        let cfg = state.config.read().agents.clone();
        let provider = find_provider(&cfg, Some(provider.unwrap_or("claude")))?;
        // The session environment decides where the CLI keeps its files: the project
        // overlay's value, else the provider's.
        let env_dir = |key: &str| {
            project
                .config
                .agent
                .env
                .get(key)
                .or_else(|| provider.env.get(key))
                .filter(|v| !v.contains("${"))
                .cloned()
        };
        let st = state.clone();
        let root = project.root.clone();
        let join = |e: tokio::task::JoinError| ApiError::internal(e.to_string());
        let mut rows: Vec<HistoryEntry> = match provider.kind {
            ProviderKind::Claude => {
                let dir = transcript::project_dir(&transcript::claude_dir(env_dir("CLAUDE_CONFIG_DIR").as_deref()), &root);
                let rows = tokio::task::spawn_blocking(move || st.terminals.history.lock().list(&dir, limit)).await.map_err(join)?;
                rows.into_iter()
                    .map(|(id, last, size, s)| {
                        let title = s.ai_title.clone().or_else(|| s.first_prompt.clone()).unwrap_or_else(|| "(untitled session)".into());
                        HistoryEntry {
                            first_prompt: s.first_prompt.filter(|p| *p != title),
                            title,
                            id,
                            last_activity: last,
                            size_bytes: size,
                            git_branch: s.git_branch,
                            last_message: None,
                            ..Default::default()
                        }
                    })
                    .collect()
            }
            ProviderKind::Codex => {
                let home = codex::codex_home(env_dir("CODEX_HOME").as_deref());
                let rows = tokio::task::spawn_blocking(move || st.terminals.codex_history.lock().list(&home, &root, limit)).await.map_err(join)?;
                rows.into_iter()
                    .map(|(s, last, size)| HistoryEntry {
                        title: s.first_prompt.clone().unwrap_or_else(|| "(untitled session)".into()),
                        id: s.id,
                        last_activity: last,
                        size_bytes: size,
                        git_branch: s.git_branch,
                        last_message: s.last_message,
                        ..Default::default()
                    })
                    .collect()
            }
            ProviderKind::Kimi => {
                let home = kimi::kimi_home(env_dir("KIMI_CODE_HOME").as_deref());
                tokio::task::spawn_blocking(move || {
                    let root_s = root.to_string_lossy().trim_end_matches('/').to_string();
                    let entries: Vec<kimi::IndexEntry> = kimi::load_index(&home)
                        .into_iter()
                        .filter(|e| {
                            let w = e.work_dir.trim_end_matches('/');
                            w == root_s || w.starts_with(&format!("{root_s}/"))
                        })
                        .collect();
                    // Newest entries last in the index; read at most a few hundred.
                    let mut rows: Vec<HistoryEntry> = entries
                        .iter()
                        .rev()
                        .take(400)
                        .map(|e| {
                            let s = kimi::summarize(&home, e);
                            let title = s.title.clone().or_else(|| s.last_prompt.clone()).unwrap_or_else(|| "(untitled session)".into());
                            HistoryEntry {
                                first_prompt: s.last_prompt.filter(|p| *p != title),
                                title,
                                id: s.id,
                                last_activity: s.updated_at.unwrap_or(0),
                                ..Default::default()
                            }
                        })
                        .collect();
                    rows.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
                    rows.truncate(limit);
                    rows
                })
                .await
                .map_err(join)?
            }
            ProviderKind::Gemini => {
                let dir = gemini::gemini_dir(env_dir("GEMINI_CLI_HOME").as_deref());
                tokio::task::spawn_blocking(move || {
                    gemini::list(&dir, &root, limit)
                        .into_iter()
                        .map(|s| HistoryEntry {
                            title: s.first_prompt.clone().unwrap_or_else(|| "(untitled session)".into()),
                            id: s.id,
                            last_activity: s.last_activity,
                            size_bytes: s.size,
                            last_message: s.last_message,
                            ..Default::default()
                        })
                        .collect()
                })
                .await
                .map_err(join)?
            }
            ProviderKind::Aider | ProviderKind::Custom => vec![],
        };
        let hosted: Vec<(String, String, bool)> = self
            .all()
            .iter()
            .filter_map(|e| {
                let r = e.rec.lock();
                let a = r.info.agent.as_ref()?;
                (a.provider_id.as_deref().unwrap_or("claude") == provider.id && !a.session_id.is_empty()).then(|| (a.session_id.clone(), e.id.clone(), r.info.open))
            })
            .collect();
        for row in &mut rows {
            row.provider = provider.id.clone();
            if let Some(h) = hosted.iter().find(|h| h.0 == row.id) {
                row.terminal_id = Some(h.1.clone());
                row.open = h.2;
            }
        }
        Ok(rows)
    }

    /// Live Claude sessions on this machine that Workbench does not host.
    pub(crate) async fn external(&self, state: &AppState) -> Vec<transcript::ExternalSession> {
        let (pids, sessions): (HashSet<i32>, HashSet<String>) = {
            let mut pids = HashSet::new();
            let mut sessions = HashSet::new();
            for e in self.all() {
                if let Some(p) = e.running_pty() {
                    pids.insert(p.pid);
                }
                if let Some(a) = e.rec.lock().info.agent.as_ref() {
                    sessions.insert(a.session_id.clone());
                }
            }
            (pids, sessions)
        };
        let dir = transcript::claude_dir(None);
        let mut list = tokio::task::spawn_blocking(move || transcript::read_live_sessions(&dir)).await.unwrap_or_default();
        list.retain(|s| pty::pid_alive(s.pid) && !pids.contains(&s.pid) && !sessions.contains(&s.session_id));
        for s in &mut list {
            s.project_id = state.projects.find_by_path(Path::new(&s.cwd)).map(|p| p.id.clone());
        }
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list
    }
}

/// Follow the session to the transcript a hook names (`/clear`, fork, resume). A dev
/// container session's transcript is a path in the container, not here.
fn note_transcript(rec: &mut store::Record, path: Option<String>) -> bool {
    let inside = super::in_container(&rec.info);
    let Some(tp) = path.filter(|p| !inside && util::os::path::is_absolute_str(p) && p.ends_with(".jsonl") && !util::os::path::segments(p).any(|s| s == "..")) else {
        return false;
    };
    if rec.transcript_path.as_deref() == Some(tp.as_str()) {
        return false;
    }
    rec.transcript_path = Some(tp);
    true
}

/// Whether git tracks `rel` (a path at the repository root `root`). When git cannot
/// tell (not installed, not a repository it accepts), the answer is yes: the caller
/// then treats the file as repository content.
async fn git_tracks(root: &Path, rel: &str) -> bool {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false", "ls-files", "--error-unmatch", "--"])
        .arg(format!(":(literal){rel}"))
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    // `--error-unmatch` exits 1 for a path git does not know.
    !matches!(crate::util::proc::run_cmd(cmd, Duration::from_secs(10)).await, Ok(o) if o.code == Some(1))
}

/// Whether Claude's permission dialog is on the bottom rows of the session's screen.
fn permission_dialog_on_screen(entry: &Entry) -> bool {
    permission::dialog_visible(&pty::visible_text(&mut entry.screen.mirror(), permission::DIALOG_ROWS))
}

/// A permission request id (URL- and JSON-safe, unguessable).
fn new_permission_id() -> String {
    util::random_token(12).replace(['-', '_'], "x")
}

/// A held `PermissionRequest` hook. When its handler ends — answered, timed out, or
/// dropped because Claude hung up — the request is no longer answerable from Workbench.
struct HeldPermission {
    state: AppState,
    entry: Arc<Entry>,
    id: String,
}

impl Drop for HeldPermission {
    fn drop(&mut self) {
        self.state.terminals.forget_permission(&self.entry, &self.id);
    }
}

/// Expand `~/` and `${secret:NAME}` in a project `[agent].env` value. The secrets used
/// are added to `used` (for masking the session's output).
fn expand_env_value(state: &AppState, project: &Project, v: &str, used: &mut Vec<Secret>) -> Result<String, ApiError> {
    let v = if let Some(rest) = v.strip_prefix("~/") {
        crate::config::expand_tilde(&format!("~/{rest}")).display().to_string()
    } else {
        v.to_string()
    };
    let mut err = None;
    let out = SECRET_REF.replace_all(&v, |c: &regex::Captures| match state.secret(Some(project), &c[1]) {
        Ok(s) => {
            let value = s.expose().to_string();
            used.push(s);
            value
        }
        Err(e) => {
            err = Some(e);
            String::new()
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(out.into_owned()),
    }
}

// ---------------------------------------------------------------- background tasks

const TAIL_BACKLOG: u64 = 4 * 1024 * 1024;

/// Follow the session's transcript while its process runs. `offset: None` skips
/// whatever the file already holds when it first appears.
fn spawn_tail(state: &AppState, entry: &Arc<Entry>, path: PathBuf, offset: Option<u64>) {
    let Some(pty) = entry.running_pty() else { return };
    tokio::spawn(tail_loop(state.clone(), entry.clone(), pty, path, offset));
}

async fn tail_loop(state: AppState, entry: Arc<Entry>, pty: Arc<pty::Pty>, mut path: PathBuf, mut offset: Option<u64>) {
    let mut lines = LineBuffer::default();
    let mut tick: u32 = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let alive = entry.pty.lock().as_ref().is_some_and(|p| Arc::ptr_eq(p, &pty));
        // Follow the session to a new transcript (/clear, fork) as hooks report it.
        let reported = entry.rec.lock().transcript_path.clone();
        if let Some(tp) = reported {
            let tp = PathBuf::from(tp);
            if tp != path {
                path = tp;
                offset = tokio::fs::metadata(&path).await.ok().map(|m| m.len()).or(Some(0));
                lines.clear();
            }
        }
        let records: Vec<Record> = read_new_lines(&path, &mut offset, &mut lines).await.iter().flat_map(|l| transcript::parse_line(l)).collect();
        if !records.is_empty() {
            state.terminals.apply_records(&entry, records);
        }
        tick = tick.wrapping_add(1);
        if tick % 10 == 1 {
            check_bridge(&state, &entry, pty.pid).await;
        }
        if alive && tick >= 3 && tick % 2 == 1 {
            state.terminals.check_startup(&entry, Duration::from_millis(500) * tick);
        }
        if alive {
            state.terminals.check_permission_dialog(&entry);
        }
        if !alive {
            return;
        }
    }
}

/// The complete lines appended since `offset`. `offset: None` skips whatever the file
/// holds when it first appears; a file that shrank (rewritten) is read from the start.
async fn read_new_lines(path: &Path, offset: &mut Option<u64>, lines: &mut LineBuffer) -> Vec<Vec<u8>> {
    let Ok(md) = tokio::fs::metadata(path).await else { return vec![] };
    let len = md.len();
    let mut start = match *offset {
        None => {
            *offset = Some(len);
            return vec![];
        }
        Some(o) if o > len => {
            // Truncated or replaced: start over.
            lines.clear();
            0
        }
        Some(o) => o,
    };
    if start == len {
        return vec![];
    }
    let mut skip_partial = false;
    if len - start > TAIL_BACKLOG {
        start = len - TAIL_BACKLOG;
        lines.clear();
        skip_partial = true;
    }
    let Ok(mut f) = tokio::fs::File::open(path).await else { return vec![] };
    if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return vec![];
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if f.take(len - start).read_to_end(&mut buf).await.is_err() {
        return vec![];
    }
    *offset = Some(start + buf.len() as u64);
    let mut data = &buf[..];
    if skip_partial {
        match data.iter().position(|&b| b == b'\n') {
            Some(i) => data = &data[i + 1..],
            None => return vec![],
        }
    }
    let mut out = vec![];
    lines.push(data, |l| out.push(l.to_vec()));
    out
}

/// Paste a prompt into a session once it is up (Claude: its SessionStart hook; Codex,
/// Kimi and custom CLIs: the first quiet spell after startup), and never into a dialog:
/// while one shows (a trust prompt nobody answered yet, whose highlighted choice the
/// Enter would pick), it waits for as long as the process runs, up to `DELIVER_MAX`.
fn deliver_later(state: &AppState, entry: &Arc<Entry>, prompt: String, submit: bool) {
    let Some(pty) = entry.running_pty() else { return };
    let (state, entry) = (state.clone(), entry.clone());
    tokio::spawn(async move {
        let _ = tokio::time::timeout(Duration::from_secs(60), entry.started.notified()).await;
        // Give the input box a moment to mount.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let deadline = tokio::time::Instant::now() + DELIVER_MAX;
        loop {
            if !entry.pty.lock().as_ref().is_some_and(|p| Arc::ptr_eq(p, &pty)) {
                return;
            }
            if state.terminals.prompt_refusal(&entry).is_none() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(terminal = %entry.id, "the initial prompt was not delivered: the session kept showing a dialog");
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if let Err(e) = state.terminals.send_text(&entry.id, &prompt, submit).await {
            tracing::warn!("cannot deliver the initial prompt: {e}");
        }
    });
}

/// Startup of a CLI that reports nothing until its first turn: over at the first quiet
/// spell after some output, or after `STARTUP_MAX_MS`.
fn startup_over(entry: &Entry, started_at: i64, now: i64) -> bool {
    let last = entry.screen.last_output_at.load(Ordering::Relaxed);
    (last >= started_at && now - last >= 2500) || now - started_at >= super::activity::STARTUP_MAX_MS
}

/// Follow a Codex session while its process runs: find its rollout (new sessions and
/// forks), then apply the events appended to it.
async fn codex_loop(state: AppState, entry: Arc<Entry>, pty: Arc<pty::Pty>, mut w: CodexWatch) {
    let mut lines = LineBuffer::default();
    let mut tick: u32 = 0;
    let started_at = util::now_ms();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let alive = entry.pty.lock().as_ref().is_some_and(|p| Arc::ptr_eq(p, &pty));
        if w.path.is_none() && alive && tick % 2 == 0 {
            if let Some((id, path)) = state.terminals.codex_discover(&entry, &mut w).await {
                tracing::debug!(terminal = %entry.id, "codex session {id}");
                state.terminals.set_discovered(&entry, id, Some(path.display().to_string()));
                // Everything a new session's file holds is ours.
                w.offset = Some(0);
                w.path = Some(path);
            }
        }
        if let Some(path) = w.path.clone() {
            let raw = read_new_lines(&path, &mut w.offset, &mut lines).await;
            let oldest = w.fork.then_some(w.launched_at - 2000);
            let events: Vec<codex::Event> = raw
                .iter()
                .filter(|l| oldest.is_none_or(|min| codex::line_time(l).is_none_or(|t| t >= min)))
                .flat_map(|l| codex::parse_line(l))
                .collect();
            state.terminals.apply_codex(&entry, events);
        }
        // Approval prompts, questions and the trust prompt never reach the rollout.
        if alive {
            state.terminals.check_screen_dialog(&entry, None);
        }
        if alive && tick >= 2 {
            let starting = entry.rec.lock().info.agent.as_ref().is_some_and(|a| a.state == AgentState::Starting);
            if starting && startup_over(&entry, started_at, util::now_ms()) && state.terminals.set_activity_state(&entry, AgentState::Idle) {
                entry.started.notify_one();
            }
        }
        if !alive {
            return;
        }
        tick = tick.wrapping_add(1);
    }
}

/// Kimi and custom CLIs: working while output flows, idle after a quiet spell
/// (`activity`), and a new Kimi session's id.
async fn activity_loop(state: AppState, entry: Arc<Entry>, pty: Arc<pty::Pty>, mut kimi_watch: Option<KimiWatch>, launched_at: i64) {
    let mut rx = entry.screen.out_tx.subscribe();
    let mut exit = entry.exit_tx.subscribe();
    let mut act = Activity::new(launched_at);
    // Whatever it printed before we subscribed still counts for the end of startup.
    let before = entry.screen.last_output_at.load(Ordering::Relaxed);
    if before >= launched_at {
        act.saw_output(before);
    }
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n: u32 = 0;
    loop {
        tokio::select! {
            r = rx.recv() => {
                let bytes = match r {
                    Ok(chunk) => chunk.len(),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => 4096,
                    Err(_) => return,
                };
                act.output(
                    util::now_ms(),
                    bytes,
                    entry.last_typed_at.load(Ordering::Relaxed),
                    entry.last_resized_at.load(Ordering::Relaxed),
                );
            }
            _ = tick.tick() => {
                if !entry.pty.lock().as_ref().is_some_and(|p| Arc::ptr_eq(p, &pty)) {
                    return;
                }
                let before = act.state();
                if let Some(s) = act.tick(util::now_ms()) {
                    if before == AgentState::Working && s == AgentState::Idle {
                        // Output flowed and stopped: the nearest thing to a finished turn.
                        entry.rt.lock().turn_done = true;
                    }
                    state.terminals.set_activity_state(&entry, s);
                    if before == AgentState::Starting {
                        entry.started.notify_one();
                    }
                }
                // Approval panels, questions and trust prompts look like a quiet spell.
                state.terminals.check_screen_dialog(&entry, Some(act.state()));
                n = n.wrapping_add(1);
                if n % 4 == 0 {
                    if let Some(w) = &mut kimi_watch {
                        if let Some(id) = state.terminals.kimi_discover(&entry, w).await {
                            tracing::debug!(terminal = %entry.id, "kimi session {id}");
                            state.terminals.set_discovered(&entry, id, None);
                            kimi_watch = None;
                        }
                    }
                }
            }
            r = exit.changed() => {
                if r.is_err() || exit.borrow().is_some() {
                    return;
                }
            }
        }
    }
}

/// Remote Control sessions also advertise their bridge in `~/.claude/sessions/<pid>.json`.
async fn check_bridge(state: &AppState, entry: &Entry, pid: i32) {
    let wants = entry.rec.lock().info.agent.as_ref().is_some_and(|a| a.remote_control && a.remote_url.is_none());
    if !wants || pid <= 0 {
        return;
    }
    let file = transcript::claude_dir(None).join("sessions").join(format!("{pid}.json"));
    let Ok(bytes) = tokio::fs::read(&file).await else { return };
    if let Some(url) = transcript::parse_live_session(&bytes).and_then(|s| s.remote_url) {
        state.terminals.set_remote_url(entry, url);
    }
}

/// Collect the claude.ai links of the Remote Control server running in `entry` now.
pub(crate) fn watch_remote_control(state: &AppState, entry: &Arc<Entry>) {
    if let Some(p) = entry.running_pty() {
        tokio::spawn(watch_remote_urls(state.clone(), entry.clone(), p));
    }
}

/// Collect claude.ai links a `claude remote-control` server prints, until that process
/// (`pty`) is gone.
async fn watch_remote_urls(state: AppState, entry: Arc<Entry>, pty: Arc<pty::Pty>) {
    let mut rx = entry.screen.out_tx.subscribe();
    // Whatever it printed before we subscribed is in the mirror (a restarted server's
    // screen also holds the previous run's link above the "restarted" line).
    let initial = {
        let mut m = entry.screen.mirror();
        let text = pty::screen_text(&mut m, 500);
        match text.rfind("── restarted ──") {
            Some(i) => text[i..].to_string(),
            None => text,
        }
    };
    state.terminals.add_remote_urls(&entry, claude_urls(&initial));
    let mut exit = entry.exit_tx.subscribe();
    let mut tail = String::new();
    loop {
        tokio::select! {
            r = rx.recv() => match r {
                Ok(chunk) => {
                    // Raw text keeps OSC 8 link targets that the visible text may shorten.
                    tail.push_str(&String::from_utf8_lossy(&chunk));
                    let found = claude_urls(&tail);
                    state.terminals.add_remote_urls(&entry, found);
                    if tail.len() > 2048 {
                        let cut = tail.len() - 1024;
                        let cut = (cut..tail.len()).find(|i| tail.is_char_boundary(*i)).unwrap_or(tail.len());
                        tail.drain(..cut);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let text = pty::screen_text(&mut entry.screen.mirror(), 500);
                    state.terminals.add_remote_urls(&entry, claude_urls(&text));
                }
                Err(_) => return,
            },
            r = exit.changed() => {
                let replaced = !entry.pty.lock().as_ref().is_some_and(|p| Arc::ptr_eq(p, &pty));
                if r.is_err() || exit.borrow().is_some() || replaced {
                    return;
                }
            }
        }
    }
}

/// On start: shells that were running get a fresh shell below their last screen; agent
/// sessions that were running resume (three at a time) when `[agents].restore_on_start`.
pub async fn restore(state: AppState) {
    let restore_agents = state.config.read().agents.restore_on_start;
    let candidates: Vec<Arc<Entry>> = state
        .terminals
        .all()
        .into_iter()
        .filter(|e| {
            let r = e.rec.lock();
            r.info.open && r.was_running
        })
        .collect();
    for e in candidates.iter().filter(|e| e.kind() == TerminalKind::Shell) {
        let (state, e) = (state.clone(), e.clone());
        tokio::spawn(async move {
            let _l = e.lifecycle.lock().await;
            let spec = state.terminals.shell_launch(&state, &e);
            if let Err(err) = state.terminals.launch(&state, &e, spec, false).await {
                tracing::warn!("cannot restart shell {}: {}", e.id, err.message);
            }
        });
    }
    if !restore_agents {
        return;
    }
    let sem = Arc::new(tokio::sync::Semaphore::new(3));
    for e in candidates.into_iter().filter(|e| e.kind() == TerminalKind::Agent) {
        let Ok(permit) = sem.clone().acquire_owned().await else { return };
        let state = state.clone();
        tokio::spawn(async move {
            {
                let _l = e.lifecycle.lock().await;
                if e.running_pty().is_none() {
                    if let Err(err) = state.terminals.relaunch_agent(&state, &e).await {
                        tracing::warn!("cannot resume agent {}: {}", e.id, err.message);
                    }
                }
            }
            // Hold the slot until the session is up, so startups do not pile up.
            let _ = tokio::time::timeout(Duration::from_secs(20), e.started.notified()).await;
            drop(permit);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> AgentLaunch {
        AgentLaunch {
            provider: None,
            name: Some("fix ci".into()),
            model: Some("haiku".into()),
            effort: Some("high".into()),
            permission_mode: Some("plan".into()),
            remote_control: true,
            add_dirs: vec!["/tmp/x".into()],
        }
    }

    #[test]
    fn argv_for_new_resume_and_fork() {
        let (s, m) = (Path::new("/d/s.json"), Path::new("/d/m.json"));
        let a = build_argv("claude", &SessionArg::New("u1".into()), s, m, &launch(), Some("-starts with a dash"));
        assert_eq!(
            a,
            [
                "claude", "--session-id", "u1", "--settings", "/d/s.json", "--mcp-config", "/d/m.json", "--name", "fix ci", "--model", "haiku",
                "--effort", "high", "--permission-mode", "plan", "--add-dir", "/tmp/x", "--remote-control", "--", "-starts with a dash"
            ]
        );
        let a = build_argv("claude", &SessionArg::Resume("u2".into()), s, m, &AgentLaunch::default(), Some("  "));
        assert_eq!(a, ["claude", "--resume", "u2", "--settings", "/d/s.json", "--mcp-config", "/d/m.json"]);
        let a = build_argv("claude", &SessionArg::Fork { from: "u2".into(), new_id: "u3".into() }, s, m, &AgentLaunch::default(), None);
        assert_eq!(&a[1..6], ["--resume", "u2", "--fork-session", "--session-id", "u3"]);
    }

    #[test]
    fn settings_carry_hooks_and_optional_status_line() {
        let s = session_settings("http://127.0.0.1:1/api/hooks/claude/abc", "wba_tok", Some("/bin/wb statusline"), true, Some(600));
        let stop = &s["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["type"], "http");
        assert_eq!(stop["url"], "http://127.0.0.1:1/api/hooks/claude/abc");
        assert_eq!(stop["headers"]["Authorization"], "Bearer wba_tok");
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(s["hooks"]["Stop"][0].get("matcher").is_none());
        // SessionStart cannot be an HTTP hook: it runs the helper command.
        let start = &s["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(start["type"], "command");
        assert_eq!(start["command"], "/bin/wb statusline");
        assert_eq!(s["statusLine"]["command"], "/bin/wb statusline");
        assert_eq!(s["hooks"].as_object().unwrap().len(), HOOK_EVENTS.len() + 1);
        let no_status = session_settings("u", "t", Some("/bin/wb statusline"), false, None);
        assert!(no_status.get("statusLine").is_none());
        assert!(no_status["hooks"].get("SessionStart").is_some());
        assert!(session_settings("u", "t", None, true, None).get("statusLine").is_none());
        // A held PermissionRequest outlasts Workbench's wait; the others answer at once.
        assert_eq!(s["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"], 630);
        assert_eq!(s["hooks"]["PermissionRequest"][0]["matcher"], "*");
        assert_eq!(no_status["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"], 5);
        let m = mcp_config("http://127.0.0.1:1/mcp", "wba_tok", "abc");
        assert_eq!(m["mcpServers"]["workbench"]["headers"]["X-Workbench-Terminal"], "abc");
    }

    #[test]
    fn detects_user_status_lines() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        assert!(!user_defines_statusline(home.path(), &[proj.path()]));
        std::fs::create_dir_all(proj.path().join(".claude")).unwrap();
        std::fs::write(proj.path().join(".claude/settings.local.json"), r#"{"statusLine":{"type":"command","command":"x"}}"#).unwrap();
        assert!(user_defines_statusline(home.path(), &[proj.path()]));
        std::fs::write(home.path().join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        assert!(!user_defines_statusline(home.path(), &[]));
    }

    #[test]
    fn finds_claude_links() {
        let t = "Visit: https://claude.ai/code?workspace=http://localhost:6/ and \x1b]8;;https://claude.ai/code/session_AB1\x07here\x1b]8;;\x07.";
        assert_eq!(claude_urls(t), vec!["https://claude.ai/code?workspace=http://localhost:6/", "https://claude.ai/code/session_AB1"]);
        assert!(claude_urls("https://claude.ai/login").is_empty());
    }

    #[test]
    fn quoting_for_the_status_line_command() {
        assert_eq!(shell_quote("/home/u/.cache/wb/debug/workbench"), "/home/u/.cache/wb/debug/workbench");
        assert_eq!(shell_quote("/opt/my apps/wb"), "'/opt/my apps/wb'");
    }

    #[test]
    fn helper_survives_a_replaced_binary() {
        let dir = tempfile::tempdir().unwrap();
        let new_bin = dir.path().join("workbench");
        std::fs::write(&new_bin, b"").unwrap();
        let deleted = PathBuf::from(format!("{} (deleted)", new_bin.display()));
        assert_eq!(helper_fallback(&deleted, std::process::id()), Some(new_bin.clone()));
        std::fs::remove_file(&new_bin).unwrap();
        let me = std::process::id();
        assert_eq!(helper_fallback(&deleted, me), Some(PathBuf::from(format!("/proc/{me}/exe"))));
    }

    #[test]
    fn prompts_never_go_into_dialogs() {
        assert!(accepts_prompt(AgentState::Idle));
        assert!(accepts_prompt(AgentState::Working));
        assert!(!accepts_prompt(AgentState::NeedsPermission));
        assert!(!accepts_prompt(AgentState::NeedsInput));
        assert!(!accepts_prompt(AgentState::Starting));
        assert!(!accepts_prompt(AgentState::Exited));
    }

    #[test]
    fn model_validation() {
        assert!(valid_model("claude-opus-4-5[1m]"));
        assert!(valid_model("haiku"));
        assert!(!valid_model("x; rm -rf /"));
    }

    fn project(dir: &Path, agent_toml: &str) -> Project {
        let file: crate::config::ProjectFile = toml::from_str(&format!("schema = 1\n[project]\nid = \"p\"\nname = \"p\"\nroot = \".\"\n{agent_toml}")).unwrap();
        Project {
            id: "p".into(),
            name: "p".into(),
            root: dir.to_path_buf(),
            config: file,
            remote: None,
            warnings: vec![],
            repo_secret_names: Default::default(),
        }
    }

    fn cfg(text: &str) -> AgentsConfig {
        toml::from_str::<crate::config::GlobalConfig>(text).unwrap().agents
    }

    #[test]
    fn launch_settings_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path(), "[agent]\nmodel = \"haiku\"\neffort = \"low\"\npermission_mode = \"plan\"\n");
        let c = cfg("[agents]\neffort = \"high\"\n[agents.providers.codex]\nmodel = \"gpt-5.5\"\npermission_mode = \"bypass\"\n[agents.providers.kimi]\npermission_mode = \"plan\"\n");
        let prov = |id: &str| providers::find(&c, Some(id)).unwrap();
        // Claude: project [agent] over [agents].
        let l = resolve_launch(&prov("claude"), &c, &p, &AgentRequest::default()).unwrap();
        assert_eq!((l.provider.as_deref(), l.model.as_deref(), l.effort.as_deref(), l.permission_mode.as_deref()), (Some("claude"), Some("haiku"), Some("low"), Some("plan")));
        // Codex: the project's Claude settings do not apply; a dangerous default is ignored…
        let l = resolve_launch(&prov("codex"), &c, &p, &AgentRequest::default()).unwrap();
        assert_eq!((l.model.as_deref(), l.effort.as_deref(), l.permission_mode.as_deref(), l.remote_control), (Some("gpt-5.5"), None, None, false));
        // …but an explicit choice selects it.
        let req = AgentRequest { permission_mode: Some("bypass".into()), effort: Some("xhigh".into()), remote_control: Some(true), ..Default::default() };
        let l = resolve_launch(&prov("codex"), &c, &p, &req).unwrap();
        assert_eq!((l.permission_mode.as_deref(), l.effort.as_deref(), l.remote_control), (Some("bypass"), Some("xhigh"), false));
        // Values of another kind are refused.
        let bad = AgentRequest { permission_mode: Some("acceptEdits".into()), ..Default::default() };
        assert!(resolve_launch(&prov("codex"), &c, &p, &bad).is_err());
        let bad = AgentRequest { effort: Some("high".into()), ..Default::default() };
        assert!(resolve_launch(&prov("kimi"), &c, &p, &bad).is_err());
        let l = resolve_launch(&prov("kimi"), &c, &p, &AgentRequest::default()).unwrap();
        assert_eq!(l.permission_mode.as_deref(), Some("plan"));
        // Custom CLIs take no model and no directories.
        let c2 = cfg("[agents.providers.mycli]\ncommand = \"mycli\"\n");
        let mycli = providers::find(&c2, Some("mycli")).unwrap();
        assert!(resolve_launch(&mycli, &c2, &p, &AgentRequest { model: Some("x".into()), ..Default::default() }).is_err());
        let l = resolve_launch(&mycli, &c2, &p, &AgentRequest { add_dirs: vec!["/".into()], ..Default::default() }).unwrap();
        assert!(l.add_dirs.is_empty());
        // Aider takes a model and its dangerous preset only when asked, but no directories;
        // Gemini takes directories.
        let aider = providers::find(&c2, Some("aider")).unwrap();
        let l = resolve_launch(&aider, &c2, &p, &AgentRequest { model: Some("sonnet".into()), add_dirs: vec!["/".into()], ..Default::default() }).unwrap();
        assert_eq!((l.model.as_deref(), l.add_dirs.len(), l.permission_mode.as_deref()), (Some("sonnet"), 0, None));
        let l = resolve_launch(&aider, &c2, &p, &AgentRequest { permission_mode: Some("yes-always".into()), ..Default::default() }).unwrap();
        assert_eq!(l.permission_mode.as_deref(), Some("yes-always"));
        let gem = providers::find(&c2, Some("gemini")).unwrap();
        let l = resolve_launch(&gem, &c2, &p, &AgentRequest { add_dirs: vec!["/".into()], ..Default::default() }).unwrap();
        assert_eq!(l.add_dirs, ["/"]);
        assert!(resolve_launch(&gem, &c2, &p, &AgentRequest { effort: Some("high".into()), ..Default::default() }).is_err());
        // Settings a kind cannot take are dropped (with a warning) instead of failing each start.
        let c3 = cfg("[agents.providers.kimi]\neffort = \"high\"\n[agents.providers.mycli2]\ncommand = \"x\"\nmodel = \"gpt-x\"\n");
        assert!(resolve_launch(&providers::find(&c3, Some("kimi")).unwrap(), &c3, &p, &AgentRequest::default()).is_ok());
        assert!(resolve_launch(&providers::find(&c3, Some("mycli2")).unwrap(), &c3, &p, &AgentRequest::default()).is_ok());
        // A default provider that does not exist is named as such.
        let typo = cfg("[agents]\ndefault_provider = \"codx\"\n");
        let err = find_provider(&typo, None).unwrap_err();
        assert!(err.code == "not_configured" && err.message.contains("[agents].default_provider is \"codx\""), "{}", err.message);
    }

    #[test]
    fn workspace_folders_are_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let dirs = workspace_dirs(data.path(), Some("shop"));
        assert_eq!(dirs, [data.path().join("workspace/shop").display().to_string(), data.path().join("workspace/home").display().to_string()]);
        for d in &dirs {
            assert_eq!(std::fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // Ids that are not plain names get only the home folder.
        assert_eq!(workspace_dirs(data.path(), Some("../etc")).len(), 1);
        assert_eq!(workspace_dirs(data.path(), None).len(), 1);
    }
}

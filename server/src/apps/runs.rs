//! Run configurations: start / stop / restart, dependencies, readiness and test results.
//!
//! A run is a PTY terminal (`TerminalKind::Run`, meta `{run: name}`) owned by the
//! terminals slice; this module decides *when* to spawn it and tracks its state:
//!
//! `stopped → starting (waiting for dependencies) → running → ready → exited | failed`
//!
//! * **Dependencies** start first. A named run must become ready (servers, services)
//!   or finish successfully (tasks, tests, builds); `port:N` waits until something
//!   accepts connections on N, starting the configured run that owns N if nothing does.
//! * **Ports.** A start whose port is taken fails with `409 port_in_use` unless the
//!   request carries `freePort: true` (the UI asks first); then `fuser -k N/tcp` (or
//!   our own run on that port is stopped). Never `pkill -f`.
//! * **Readiness**: all configured probes must pass — `ready.log` (regex over
//!   ANSI-stripped output), `ready.http` (polled) — or, for servers with neither, the
//!   port accepting connections. The first capture group of `ready.log`, when it is
//!   a URL, becomes the run's URL (Vite's `Local: http://localhost:5173/`).
//! * **Services** (`kind = "service"`) are daemons: `command` starts them, `status`
//!   (exit 0 = running) and `stop` manage them.
//! * **Results**: `result_pattern` lines are parsed into `{passed, failed, items}`.
//!
//! Every state change emits `run.state` (`{name, state, port?, url?, terminalId?, …}`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use super::expand::{self, Vars};
use super::output::{LineBuffer, ReadyMatcher, ResultParser, TestResult};
use crate::app::AppState;
use crate::config::project::{RunConfig, RunKind};
use crate::error::ApiError;
use crate::projects::Project;
use crate::secrets::Secret;
use crate::terminals::{ExitInfo, SpawnSpec, TerminalKind, TerminalStatus};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    #[default]
    Stopped,
    Starting,
    Running,
    Ready,
    Failed,
    Exited,
}

impl RunState {
    pub fn active(self) -> bool {
        matches!(self, RunState::Starting | RunState::Running | RunState::Ready)
    }
}

/// Live state of one run (serialized into `GET …/runs` and `run.state`).
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunLive {
    pub state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TestResult>,
    /// Why it failed, or a warning while running (ready check timed out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What a starting run is waiting for ("waiting for api", "waiting for :8080").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The process runs in the project's dev container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_container: Option<bool>,
    /// How this computer reaches its port in the container: `published` (127.0.0.1)
    /// or `container-ip`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reach: Option<&'static str>,
}

struct Slot {
    live: RunLive,
    epoch: u64,
    cancel: CancellationToken,
    stopping: bool,
    /// Secret values in this run's env, for redacting output shown to agents.
    secrets: Vec<Secret>,
    /// A service Workbench started (and so stops on shutdown).
    started_by_us: bool,
    service_checked: Option<Instant>,
}

impl Slot {
    fn new() -> Self {
        Self {
            live: RunLive::default(),
            epoch: 0,
            cancel: CancellationToken::new(),
            stopping: false,
            secrets: vec![],
            started_by_us: false,
            service_checked: None,
        }
    }
}

type Key = (String, String);

#[derive(Default)]
pub struct Runs {
    slots: Mutex<HashMap<Key, Slot>>,
    changed: Notify,
    epochs: AtomicU64,
}

fn key(pid: &str, name: &str) -> Key {
    (pid.to_string(), name.to_string())
}

impl Runs {
    pub fn live(&self, pid: &str, name: &str) -> RunLive {
        self.slots.lock().get(&key(pid, name)).map(|s| s.live.clone()).unwrap_or_default()
    }

    pub fn secrets(&self, pid: &str, name: &str) -> Vec<Secret> {
        self.slots.lock().get(&key(pid, name)).map(|s| s.secrets.clone()).unwrap_or_default()
    }

    /// Apply `f` to the slot if it still belongs to start `epoch`; emits `run.state`.
    fn update(&self, state: &AppState, pid: &str, name: &str, epoch: u64, f: impl FnOnce(&mut Slot)) -> bool {
        let live = {
            let mut slots = self.slots.lock();
            let Some(slot) = slots.get_mut(&key(pid, name)) else { return false };
            if slot.epoch != epoch {
                return false;
            }
            f(slot);
            slot.live.clone()
        };
        self.changed.notify_waiters();
        emit(state, pid, name, &live);
        true
    }
}

fn emit(state: &AppState, pid: &str, name: &str, live: &RunLive) {
    let mut data = serde_json::to_value(live).unwrap_or_else(|_| json!({}));
    if let Some(o) = data.as_object_mut() {
        o.insert("name".into(), json!(name));
    }
    state.events.emit("run.state", Some(pid), data);
}

// ---------------------------------------------------------------- views

/// A run configuration as the UI sees it (camelCase; env values are references, not secrets).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigView {
    pub kind: RunKind,
    pub command: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub free_port: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<serde_json::Value>,
    pub depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_pattern: Option<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub has_stop: bool,
    pub has_status: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub name: String,
    pub config: RunConfigView,
    #[serde(flatten)]
    pub live: RunLive,
    /// Something listens on the configured port while the run is not ours/active.
    pub port_in_use: bool,
    /// Why a start would fail (missing toolchain, bad regex, missing cwd…).
    pub problems: Vec<String>,
    /// Starting it deploys, releases, reaches a remote host or destroys data: the UI
    /// asks first (`confirmed: true`), and agents cannot start it.
    pub needs_confirm: bool,
    /// Pinned to the host although the project uses its dev container. (Where it
    /// runs, or would run, is `inContainer` of the live state.)
    pub host_pinned: bool,
}

fn config_view(c: &RunConfig) -> RunConfigView {
    RunConfigView {
        kind: c.kind,
        command: c.command.clone(),
        cwd: c.cwd.clone(),
        port: c.port,
        free_port: c.free_port,
        ready: c.ready.as_ref().map(|r| json!({ "log": r.log, "http": r.http, "timeoutS": r.timeout_s })),
        depends_on: c.depends_on.clone(),
        preview: c.preview.clone(),
        group: c.group.clone(),
        source: c.source.clone(),
        result_pattern: c.result_pattern.clone(),
        env: c.env.clone(),
        has_stop: c.stop.is_some(),
        has_status: c.status.is_some(),
    }
}

/// Problems that would make a start fail, computed without side effects. `inside`: the
/// run goes into the project's dev container, whose programs this computer cannot see.
pub fn problems(project: &Project, c: &RunConfig, vars: &Vars, inside: bool) -> Vec<String> {
    let mut v = vec![];
    for text in [Some(c.command.as_str()), c.stop.as_deref(), c.status.as_deref()].into_iter().flatten() {
        for name in expand::unknown_placeholders(text, vars) {
            if !["sha", "sha8", "branch"].contains(&name.as_str()) && !v.iter().any(|p: &String| p.contains(&format!("{{{name}}}"))) {
                v.push(format!("unknown placeholder {{{name}}}: add it under [toolchains]"));
            }
        }
    }
    for (name, path) in &project.config.toolchains {
        if c.command.contains(&format!("{{{name}}}")) && !crate::config::expand_tilde(path).exists() {
            v.push(format!("toolchain {{{name}}} not found at {path}"));
        }
    }
    if let Some(r) = c.ready.as_ref().and_then(|r| r.log.as_ref()) {
        if let Err(e) = ReadyMatcher::new(r) {
            v.push(format!("ready.log is not a valid regex: {e}"));
        }
    }
    if let Some(r) = &c.result_pattern {
        if let Err(e) = ResultParser::new(r) {
            v.push(format!("result_pattern is not a valid regex: {e}"));
        }
    }
    match resolve_cwd(project, &c.cwd) {
        Ok(p) if !p.is_dir() => v.push(format!("working directory {} does not exist", c.cwd)),
        Err(e) => v.push(e.message),
        _ => {}
    }
    for d in &c.depends_on {
        if d.strip_prefix("port:").is_none() && !project.config.runs.iter().any(|r| &r.name == d) {
            v.push(format!("depends on unknown run {d:?}"));
        }
    }
    if let Some(prog) = command_program(&c.command).filter(|p| !inside && !program_available(p)) {
        v.push(format!("`{prog}` is not installed (not found on PATH)"));
    }
    v
}

/// Whether something accepts connections on `port` where run `name` of project `pid`
/// listens: in its dev container (probed at the container's address: a published
/// port's proxy accepts connections before anything listens inside), else here.
pub async fn run_port_open(state: &AppState, pid: &str, inside: bool, port: u16, timeout: Duration) -> bool {
    if inside {
        return match crate::devcontainer::port_route(state, pid, port) {
            Some(r) => matches!(tokio::time::timeout(timeout, tokio::net::TcpStream::connect(r.probe)).await, Ok(Ok(_))),
            None => false,
        };
    }
    port_open(port, timeout).await
}

/// A URL a run inside the dev container printed or was configured with
/// (`http://localhost:8000/`, `http://0.0.0.0:8000`): as this computer reaches it —
/// the published 127.0.0.1 port, else the container's address.
pub fn container_url(state: &AppState, pid: &str, url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let Some((host, port)) = authority.rsplit_once(':') else { return url.to_string() };
    let Ok(port) = port.parse::<u16>() else { return url.to_string() };
    if !matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "[::]" | "[::1]" | "::") {
        return url.to_string();
    }
    match crate::devcontainer::port_route(state, pid, port) {
        Some(r) => format!("{scheme}://{}:{}{path}", r.url_host, r.url_port),
        None => url.to_string(),
    }
}

/// Shell builtins and keywords: never looked up on `PATH`.
const SHELL_BUILTINS: &[&str] = &[
    ".", "source", "export", "set", "unset", "if", "for", "while", "until", "case", "function", "select", "echo",
    "printf", "true", "false", "test", "read", "eval", "trap", "ulimit", "umask", "wait", "type", "alias", "declare",
    "local", "exit", "return", "shift", "kill", "let", "readonly", "shopt", "hash", "builtin", "caller", "jobs",
];

/// The program a command line starts when the shell looks it up on `PATH`: `go` for
/// `go run ./cmd/api`, `npm` for `PORT=3000 npm start`, `cargo` for `cd server &&
/// cargo build`. `None` for a path (`./gradlew`), a `{toolchain}`, shell syntax and
/// builtins, which a `PATH` lookup cannot judge.
pub(crate) fn command_program(cmd: &str) -> Option<String> {
    let mut words = cmd.split_whitespace();
    loop {
        let w = words.next()?;
        if w.contains('=') && !w.starts_with(['-', '=']) {
            continue; // `VAR=value`
        }
        match w {
            "exec" | "command" | "nohup" | "time" | "env" => continue,
            // `cd dir && prog …`: the program after the change of directory.
            "cd" | "pushd" => {
                loop {
                    let n = words.next()?;
                    if n == "&&" || n.ends_with(';') {
                        break;
                    }
                }
                continue;
            }
            _ => {}
        }
        let plain = w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
        return (plain && !w.starts_with(['-', '.']) && !SHELL_BUILTINS.contains(&w)).then(|| w.to_string());
    }
}

/// Where tools often live when the Workbench process (started from a desktop
/// launcher or a service) has a shorter `PATH` than the login shell runs get.
fn user_bin_dirs() -> Vec<std::path::PathBuf> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let mut out: Vec<std::path::PathBuf> = [
        ".local/bin", "bin", ".cargo/bin", "go/bin", ".bun/bin", ".deno/bin", ".dotnet", ".dotnet/tools", ".volta/bin",
        ".asdf/shims", ".local/share/mise/shims", ".pyenv/shims", ".rbenv/shims", ".nodenv/shims", ".local/share/pnpm",
        ".yarn/bin", ".npm-global/bin", ".juliaup/bin", ".ghcup/bin", ".elan/bin", ".mix/escripts", ".composer/vendor/bin",
        ".config/composer/vendor/bin",
    ]
    .iter()
    .map(|d| home.join(d))
    .collect();
    out.extend(["/usr/local/go/bin", "/usr/local/bin", "/snap/bin", "/opt/homebrew/bin", "/home/linuxbrew/.linuxbrew/bin"].map(Into::into));
    // Version managers with one directory per installed version.
    for (base, sub) in [(".nvm/versions/node", "bin"), (".sdkman/candidates", "current/bin"), (".rustup/toolchains", "bin")] {
        if let Ok(rd) = std::fs::read_dir(home.join(base)) {
            out.extend(rd.flatten().take(20).map(|e| e.path().join(sub)));
        }
    }
    out
}

/// Whether `prog` can be found: on this process's `PATH` or in a usual user tool
/// directory. Answers are remembered for a few seconds (run lists poll).
fn program_available(prog: &str) -> bool {
    static CACHE: std::sync::LazyLock<Mutex<HashMap<String, (bool, Instant)>>> = std::sync::LazyLock::new(Default::default);
    if let Some((ok, at)) = CACHE.lock().get(prog) {
        if at.elapsed() < Duration::from_secs(10) {
            return *ok;
        }
    }
    let ok = crate::util::which(prog) || crate::util::os::exe::find_in(&user_bin_dirs(), prog).is_some();
    let mut cache = CACHE.lock();
    if cache.len() > 512 {
        cache.clear();
    }
    cache.insert(prog.to_string(), (ok, Instant::now()));
    ok
}

/// A run's cwd: project-relative (contained), or absolute / `~/` from user config.
pub fn resolve_cwd(project: &Project, cwd: &str) -> Result<std::path::PathBuf, ApiError> {
    if cwd.starts_with('/') || cwd.starts_with("~/") {
        return Ok(crate::config::expand_tilde(cwd));
    }
    crate::util::paths::resolve_in_root(&project.root, cwd)
}

/// Whether a start of `c` would run in the project's dev container: the project uses
/// it, the run is not pinned to the host, and its folder is in the workspace mount.
fn starts_inside(state: &AppState, project: &Project, c: &RunConfig) -> bool {
    crate::devcontainer::run_inside(state, &project.id, &c.name)
        && resolve_cwd(project, &c.cwd).is_ok_and(|d| d.starts_with(&project.root))
}

async fn run_view(state: &AppState, project: &Project, c: &RunConfig, vars: &Vars) -> RunView {
    let mut live = state.apps.runs.live(&project.id, &c.name);
    let inside = if live.state.active() { live.in_container.unwrap_or(false) } else { starts_inside(state, project, c) };
    live.in_container = inside.then_some(true);
    let problems = problems(project, c, vars, inside);
    let port_in_use = match (c.port, live.state.active()) {
        (Some(p), false) => run_port_open(state, &project.id, inside, p, Duration::from_millis(150)).await,
        _ => false,
    };
    let host_pinned = crate::devcontainer::uses_container(state, &project.id) && !crate::devcontainer::run_inside(state, &project.id, &c.name);
    RunView {
        name: c.name.clone(),
        config: config_view(c),
        live,
        port_in_use,
        problems,
        needs_confirm: needs_confirmation(c),
        host_pinned,
    }
}

pub async fn list(state: &AppState, project: &Project) -> Vec<RunView> {
    refresh_services(state, project);
    let vars = expand::base_vars(project);
    futures::future::join_all(project.config.runs.iter().map(|c| run_view(state, project, c, &vars))).await
}

pub async fn view(state: &AppState, project: &Project, name: &str) -> Result<RunView, ApiError> {
    let c = find(project, name)?;
    let vars = expand::base_vars(project);
    Ok(run_view(state, project, c, &vars).await)
}

/// Whether starting `c` must be confirmed by the user: by its name or command it
/// deploys, releases, publishes, reaches a remote host, destroys data or handles
/// secrets (the checks that keep such doc commands from being offered at all).
pub fn needs_confirmation(c: &RunConfig) -> bool {
    c.group.as_deref() == Some("deploy")
        || super::detect::is_risky_command(&c.name)
        || super::detect::is_risky_command(&c.command)
        || super::detect::reaches_out(&c.command)
}

/// Runs that starting (or restarting) `name` would launch — itself and its inactive
/// dependencies — that the caller may not start without the user's say-so: those
/// that need confirmation, and for agents also documentation suggestions.
pub async fn gated(state: &AppState, project: &Project, name: &str, restart: bool, for_agent: bool) -> Result<Vec<String>, ApiError> {
    find(project, name)?;
    let plan = plan(state, project, name).await?;
    Ok(plan
        .iter()
        .filter(|(n, _)| (restart && n == name) || !state.apps.runs.live(&project.id, n).state.active())
        .filter_map(|(n, _)| find(project, n).ok())
        .filter(|c| needs_confirmation(c) || (for_agent && c.group.as_deref() == Some("suggested")))
        .map(|c| c.name.clone())
        .collect())
}

fn find<'a>(project: &'a Project, name: &str) -> Result<&'a RunConfig, ApiError> {
    project
        .config
        .runs
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| ApiError::not_found(format!("no run configuration {name:?} in {}", project.id)))
}

// ---------------------------------------------------------------- ports

/// Whether something accepts TCP connections on `port` (IPv4 or IPv6 loopback).
pub async fn port_open(port: u16, timeout: Duration) -> bool {
    let v4 = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(("127.0.0.1", port)));
    let v6 = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(("::1", port)));
    let (a, b) = tokio::join!(v4, v6);
    matches!(a, Ok(Ok(_))) || matches!(b, Ok(Ok(_)))
}

/// `fuser -k PORT/tcp`, then wait up to 5 s for the port to close.
async fn free_port(port: u16) -> Result<(), String> {
    if !crate::util::which("fuser") {
        return Err("fuser is not installed (package psmisc); free the port yourself".into());
    }
    let out = crate::util::proc::run("fuser", &["-k", &format!("{port}/tcp")], std::path::Path::new("/"), Duration::from_secs(10))
        .await
        .map_err(|e| e.message)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !port_open(port, Duration::from_millis(200)).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!("port {port} is still in use after fuser -k ({})", out.message()))
}

// ---------------------------------------------------------------- start

#[derive(Debug, Clone, PartialEq)]
enum Dep {
    Run(String),
    Port(u16),
}

/// Start `name` (and, first, whatever it depends on). Returns once every run is
/// marked `starting`; the rest happens in background tasks.
pub async fn start(state: &AppState, project: &Arc<Project>, name: &str, free: bool) -> Result<(), ApiError> {
    find(project, name)?;
    if state.apps.runs.live(&project.id, name).state.active() {
        return Ok(()); // already starting or running: starting is idempotent
    }
    let plan = plan(state, project, name).await?;
    let texts: Vec<&str> = plan
        .iter()
        .filter_map(|(n, _)| find(project, n).ok())
        .flat_map(|c| std::iter::once(c.command.as_str()).chain(c.env.values().map(String::as_str)).chain(c.preview.as_deref()))
        .collect();
    let vars = expand::with_git(expand::base_vars(project), &project.root, &texts).await?;

    // Preflight everything that is not already running, before touching any state.
    let mut prepared: Vec<(String, Vec<Dep>, Prepared)> = vec![];
    let mut busy: Vec<String> = vec![];
    for (run_name, deps) in &plan {
        if state.apps.runs.live(&project.id, run_name).state.active() {
            continue;
        }
        let c = find(project, run_name)?;
        let mut p = prepare(state, project, c, &vars)?;
        p.inside = starts_inside(state, project, c);
        // `free_port = true` in the config is standing consent to free the port.
        if let (Some(port), false, false) = (c.port, c.kind == RunKind::Service, c.free_port) {
            if run_port_open(state, &project.id, p.inside, port, Duration::from_millis(250)).await {
                let owner = owner_of_port(state, project, port, run_name);
                busy.push(match (&owner, p.inside) {
                    (Some(o), _) => format!("port {port} is used by run {o:?}"),
                    (None, true) => format!("port {port} is in use inside the dev container"),
                    (None, false) => format!("port {port} is in use by another process"),
                });
            }
        }
        prepared.push((run_name.clone(), deps.clone(), p));
    }
    if !busy.is_empty() && !free {
        return Err(ApiError::new(axum::http::StatusCode::CONFLICT, "port_in_use", busy.join("; ")));
    }

    for (run_name, deps, p) in prepared {
        let (epoch, cancel) = {
            let mut slots = state.apps.runs.slots.lock();
            let slot = slots.entry(key(&project.id, &run_name)).or_insert_with(Slot::new);
            if slot.live.state.active() {
                continue; // another request started it meanwhile
            }
            slot.cancel.cancel();
            let epoch = state.apps.runs.epochs.fetch_add(1, Ordering::Relaxed) + 1;
            slot.epoch = epoch;
            slot.cancel = state.apps.shutdown.child_token();
            slot.stopping = false;
            slot.secrets = p.secrets.clone();
            slot.live = RunLive {
                state: RunState::Starting,
                port: p.port,
                // The run keeps its terminal: the new process runs in the same tab.
                terminal_id: slot.live.terminal_id.take(),
                started_at: Some(crate::util::now_ms()),
                phase: (!deps.is_empty()).then(|| "waiting for dependencies".into()),
                in_container: p.inside.then_some(true),
                ..Default::default()
            };
            (epoch, slot.cancel.clone())
        };
        let live = state.apps.runs.live(&project.id, &run_name);
        emit(state, &project.id, &run_name, &live);
        state.apps.runs.changed.notify_waiters();
        let st = state.clone();
        let pid = project.id.clone();
        let free = free || p.config.free_port;
        tokio::spawn(async move {
            supervise(st, pid, run_name, epoch, cancel, deps, p, free).await;
        });
    }
    Ok(())
}

/// The run on `port` that is ours and active (other than `except`).
fn owner_of_port(state: &AppState, project: &Project, port: u16, except: &str) -> Option<String> {
    project
        .config
        .runs
        .iter()
        .filter(|r| r.name != except && r.port == Some(port))
        .find(|r| state.apps.runs.live(&project.id, &r.name).state.active())
        .map(|r| r.name.clone())
}

/// Dependency closure of `name` in start order (dependencies first), each with its direct deps.
async fn plan(state: &AppState, project: &Project, name: &str) -> Result<Vec<(String, Vec<Dep>)>, ApiError> {
    let mut order: Vec<(String, Vec<Dep>)> = vec![];
    let mut visiting: HashSet<String> = HashSet::new();
    let mut stack: Vec<(String, bool)> = vec![(name.to_string(), false)];
    // Iterative DFS with an explicit "children done" marker.
    while let Some((n, done)) = stack.pop() {
        if order.iter().any(|(o, _)| *o == n) {
            continue;
        }
        let c = find(project, &n)?;
        let mut deps = vec![];
        for d in &c.depends_on {
            match d.strip_prefix("port:") {
                Some(p) => {
                    let port: u16 = p.trim().parse().map_err(|_| ApiError::bad_request(format!("{n}: bad dependency {d:?}")))?;
                    // Ours and running → wait for it; open → nothing to do; owned by a
                    // configured run → start that run; else wait for the port.
                    if let Some(owner) = owner_of_port(state, project, port, &n) {
                        deps.push(Dep::Run(owner));
                    } else if port_open(port, Duration::from_millis(200)).await {
                        deps.push(Dep::Port(port));
                    } else if let Some(owner) = project.config.runs.iter().find(|r| r.name != n && r.port == Some(port)) {
                        deps.push(Dep::Run(owner.name.clone()));
                    } else {
                        deps.push(Dep::Port(port));
                    }
                }
                None => {
                    find(project, d).map_err(|_| ApiError::bad_request(format!("{n} depends on unknown run {d:?}")))?;
                    deps.push(Dep::Run(d.clone()));
                }
            }
        }
        if done {
            visiting.remove(&n);
            order.push((n, deps));
            continue;
        }
        if !visiting.insert(n.clone()) {
            return Err(ApiError::bad_request(format!("dependency cycle through {n:?}")));
        }
        stack.push((n.clone(), true));
        for d in deps.iter().rev() {
            if let Dep::Run(r) = d {
                if visiting.contains(r) {
                    return Err(ApiError::bad_request(format!("dependency cycle: {n:?} → {r:?}")));
                }
                if !order.iter().any(|(o, _)| o == r) {
                    stack.push((r.clone(), false));
                }
            }
        }
    }
    Ok(order)
}

#[cfg(test)]
pub(crate) async fn plan_for_tests(state: &AppState, project: &Project, name: &str) -> Result<Vec<(String, Vec<String>)>, ApiError> {
    Ok(plan(state, project, name).await?.into_iter().map(|(n, d)| (n, d.iter().map(|x| format!("{x:?}")).collect())).collect())
}

/// Everything needed to spawn, computed up front so errors reach the HTTP caller.
#[derive(Clone)]
struct Prepared {
    config: RunConfig,
    command: String,
    status: Option<String>,
    cwd: std::path::PathBuf,
    env: Vec<(String, Option<String>)>,
    secrets: Vec<Secret>,
    port: Option<u16>,
    ready_log: Option<Arc<ReadyMatcher>>,
    results: Option<Arc<ResultParser>>,
    preview: Option<String>,
    /// Runs in the project's dev container.
    inside: bool,
}

fn prepare(state: &AppState, project: &Project, c: &RunConfig, vars: &Vars) -> Result<Prepared, ApiError> {
    for (name, path) in &project.config.toolchains {
        if c.command.contains(&format!("{{{name}}}")) && !crate::config::expand_tilde(path).exists() {
            return Err(ApiError::not_configured(format!(
                "toolchain {{{name}}} not found at {path}; install it or set [toolchains] {name} = \"…\" in ~/.config/workbench/projects/{}.toml",
                project.id
            )));
        }
    }
    let unknown: Vec<String> = expand::unknown_placeholders(&c.command, vars);
    if let Some(u) = unknown.first() {
        return Err(ApiError::not_configured(format!(
            "{}: unknown placeholder {{{u}}}; add it under [toolchains] in ~/.config/workbench/projects/{}.toml",
            c.name, project.id
        )));
    }
    let cwd = resolve_cwd(project, &c.cwd)?;
    if !cwd.is_dir() {
        return Err(ApiError::bad_request(format!("{}: working directory {} does not exist", c.name, c.cwd)));
    }
    let ready_log = match c.ready.as_ref().and_then(|r| r.log.as_deref()) {
        Some(p) => Some(Arc::new(ReadyMatcher::new(p).map_err(|e| ApiError::bad_request(format!("{}: ready.log: {e}", c.name)))?)),
        None => None,
    };
    let results = match c.result_pattern.as_deref() {
        Some(p) => Some(Arc::new(ResultParser::new(p).map_err(|e| ApiError::bad_request(format!("{}: result_pattern: {e}", c.name)))?)),
        None => None,
    };
    let (env, secrets) = expand::run_env(state, project, &c.env, vars)?;
    Ok(Prepared {
        config: c.clone(),
        command: expand::placeholders(&c.command, vars),
        status: c.status.as_deref().map(|s| expand::placeholders(s, vars)),
        cwd,
        env,
        secrets,
        port: c.port,
        ready_log,
        results,
        preview: c.preview.as_deref().map(|p| expand::placeholders(p, vars)),
        inside: false,
    })
}

async fn supervise(
    state: AppState,
    pid: String,
    name: String,
    epoch: u64,
    cancel: CancellationToken,
    deps: Vec<Dep>,
    p: Prepared,
    free: bool,
) {
    let runs = &state.apps.runs;
    let fail = |msg: String| {
        runs.update(&state, &pid, &name, epoch, |s| {
            s.live.state = RunState::Failed;
            s.live.phase = None;
            s.live.error = Some(msg);
        });
    };

    // 1. Dependencies.
    for d in &deps {
        let label = match d {
            Dep::Run(r) => format!("waiting for {r}"),
            Dep::Port(n) => format!("waiting for :{n}"),
        };
        runs.update(&state, &pid, &name, epoch, |s| s.live.phase = Some(label.clone()));
        let r = tokio::select! {
            _ = cancel.cancelled() => return,
            r = wait_dep(&state, &pid, d) => r,
        };
        if let Err(e) = r {
            fail(e);
            return;
        }
    }

    // 2. Service already running? Then there is nothing to start.
    if p.config.kind == RunKind::Service {
        if let Some(status) = &p.status {
            if run_quiet(status, &p, Duration::from_secs(10)).await == Some(true) {
                runs.update(&state, &pid, &name, epoch, |s| {
                    s.live.state = RunState::Ready;
                    s.live.phase = None;
                    s.live.ready_at = Some(crate::util::now_ms());
                });
                return;
            }
        }
    }

    // 3. Free the port if the user agreed.
    if let (true, Some(port), false) = (free, p.port, p.config.kind == RunKind::Service) {
        if run_port_open(&state, &pid, p.inside, port, Duration::from_millis(250)).await {
            runs.update(&state, &pid, &name, epoch, |s| s.live.phase = Some(format!("freeing :{port}")));
            let project = state.projects.get(&pid);
            let ours = project.as_ref().and_then(|pr| owner_of_port(&state, pr, port, &name));
            let r = match ours {
                Some(o) => match project {
                    Some(pr) => stop(&state, &pr, &o).await.map(|_| ()).map_err(|e| e.message),
                    None => Ok(()),
                },
                // `fuser` here cannot see into the container (and would hit docker's proxy).
                None if p.inside => Err(format!("port {port} is in use inside the dev container; stop what listens there")),
                None => free_port(port).await,
            };
            if let Err(e) = r {
                fail(e);
                return;
            }
        }
    }

    // 4. Spawn.
    let spec = SpawnSpec {
        kind: TerminalKind::Run,
        title: name.clone(),
        project_id: Some(pid.clone()),
        cwd: p.cwd.clone(),
        argv: crate::util::os::shell::run_argv(&p.command),
        env: p.env.clone(),
        cols: None,
        rows: None,
        // The terminals slice runs it with `docker exec` in the dev container.
        meta: if p.inside { json!({ "run": name, "inContainer": true }) } else { json!({ "run": name }) },
    };
    if cancel.is_cancelled() {
        return;
    }
    // One terminal per run: the previous (exited) one is reused, so an open output tab
    // follows the new process instead of showing a dead one, and tabs do not pile up.
    let previous = runs
        .live(&pid, &name)
        .terminal_id
        .filter(|t| state.terminals.info(t).is_some_and(|i| i.status == TerminalStatus::Exited && i.kind == TerminalKind::Run));
    let mut early_out = None;
    let spawned = match previous {
        Some(tid) => {
            // Subscribed before the launch: every byte of the new process is seen, and
            // the old output on the screen is never mistaken for a ready line.
            early_out = state.terminals.subscribe_output(&tid);
            // `${secret:…}` values in the env are masked in the output at the source.
            match state.terminals.respawn_redacted(&state, &tid, spec.clone(), p.secrets.clone()).await {
                Err(e) if e.code != "internal" => {
                    // Taken over meanwhile (restarted from its tab, removed): use a new one.
                    early_out = None;
                    state.terminals.spawn_redacted(&state, spec, p.secrets.clone()).await
                }
                r => r,
            }
        }
        None => state.terminals.spawn_redacted(&state, spec, p.secrets.clone()).await,
    };
    let info = match spawned {
        Ok(i) => i,
        Err(e) => {
            fail(format!("could not start: {}", e.message));
            return;
        }
    };
    let tid = info.id.clone();
    let url = default_url(&p).map(|u| if p.inside { container_url(&state, &pid, &u) } else { u });
    let reach = if p.inside { p.port.and_then(|port| crate::devcontainer::port_route(&state, &pid, port)).map(|r| r.via) } else { None };
    let tracked = runs.update(&state, &pid, &name, epoch, |s| {
        s.live.state = RunState::Running;
        s.live.phase = None;
        s.live.terminal_id = Some(tid.clone());
        s.live.started_at = Some(crate::util::now_ms());
        s.live.reach = reach;
        if s.live.url.is_none() {
            s.live.url = url.clone();
        }
        if p.config.kind == RunKind::Service {
            s.started_by_us = true;
        }
    });
    if !tracked {
        // Stopped or restarted while we were spawning: do not leave an orphan.
        let _ = state.terminals.kill(&tid).await;
        return;
    }
    watch_process(&state, &pid, &name, epoch, &cancel, &tid, &p, early_out).await;
}

/// URL shown before the ready line reveals one: the preview, else localhost:port for servers.
fn default_url(p: &Prepared) -> Option<String> {
    p.preview.clone().or_else(|| match (p.config.kind, p.port) {
        (RunKind::Server, Some(port)) => Some(format!("http://localhost:{port}/")),
        _ => None,
    })
}

/// Wait until a dependency is satisfied (or fails).
async fn wait_dep(state: &AppState, pid: &str, d: &Dep) -> Result<(), String> {
    match d {
        Dep::Port(port) => {
            let deadline = Instant::now() + Duration::from_secs(600);
            while Instant::now() < deadline {
                if port_open(*port, Duration::from_millis(300)).await {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(format!("nothing is listening on :{port} after 10 minutes"))
        }
        Dep::Run(r) => {
            let kind = state
                .projects
                .get(pid)
                .and_then(|p| p.config.runs.iter().find(|c| c.name == *r).map(|c| (c.kind, c.ready.as_ref().map(|x| x.timeout_s))))
                .unwrap_or((RunKind::Task, None));
            let limit = Duration::from_secs(kind.1.map(u64::from).unwrap_or(900).max(30) + 60);
            let deadline = Instant::now() + limit;
            loop {
                let notified = state.apps.runs.changed.notified();
                let live = state.apps.runs.live(pid, r);
                let finishes = matches!(kind.0, RunKind::Task | RunKind::Test | RunKind::Build);
                match live.state {
                    RunState::Ready => return Ok(()),
                    RunState::Exited if finishes => return Ok(()),
                    RunState::Running if !finishes && !has_readiness(state, pid, r) => return Ok(()),
                    RunState::Failed => return Err(format!("dependency {r} failed{}", live.error.map(|e| format!(": {e}")).unwrap_or_default())),
                    RunState::Stopped | RunState::Exited => return Err(format!("dependency {r} is not running")),
                    _ => {}
                }
                if Instant::now() >= deadline {
                    return Err(format!("dependency {r} was not ready after {}s", limit.as_secs()));
                }
                let _ = tokio::time::timeout(Duration::from_millis(500), notified).await;
            }
        }
    }
}

fn has_readiness(state: &AppState, pid: &str, name: &str) -> bool {
    state
        .projects
        .get(pid)
        .and_then(|p| p.config.runs.iter().find(|c| c.name == name).map(|c| c.ready.is_some() || c.port.is_some()))
        .unwrap_or(false)
}

/// Follow a spawned run until it exits: readiness, results, exit state. `early_out`
/// is an output subscription taken before a reused terminal was relaunched.
#[allow(clippy::too_many_arguments)]
async fn watch_process(
    state: &AppState,
    pid: &str,
    name: &str,
    epoch: u64,
    cancel: &CancellationToken,
    tid: &str,
    p: &Prepared,
    early_out: Option<broadcast::Receiver<bytes::Bytes>>,
) {
    let runs = &state.apps.runs;
    let reused = early_out.is_some();
    let mut out = early_out.or_else(|| state.terminals.subscribe_output(tid));
    let mut exit = state.terminals.exit_watch(tid);
    let kind = p.config.kind;
    let ready_cfg = p.config.ready.clone();
    // Inside the dev container, `localhost:<port>` means the container's port.
    let need_http = ready_cfg.as_ref().and_then(|r| r.http.clone()).map(|u| if p.inside { container_url(state, pid, &u) } else { u });
    let need_log = p.ready_log.clone();
    let need_port = need_http.is_none() && need_log.is_none() && matches!(kind, RunKind::Server) && p.port.is_some();
    let (mut log_ok, mut http_ok, mut port_ok) = (need_log.is_none(), need_http.is_none(), !need_port);
    let has_probe = need_log.is_some() || need_http.is_some() || need_port;
    let timeout = Duration::from_secs(ready_cfg.as_ref().map(|r| u64::from(r.timeout_s)).unwrap_or(120).max(1));
    let started = Instant::now();
    let mut ready = false;
    let mut timed_out = false;
    let mut lines = LineBuffer::default();
    let mut result = p.results.as_ref().map(|_| TestResult::default());
    let mut result_dirty = false;
    let mut last_emit = Instant::now();
    let mut found_url: Option<String> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut last_http = Instant::now() - Duration::from_secs(10);

    // Output printed before we subscribed is in the screen mirror (a fresh terminal only:
    // a reused one was subscribed before its relaunch, and its screen holds old output).
    if let (false, Some(m), Some(text)) = (reused, &need_log, state.terminals.screen_text(tid, 300)) {
        for l in text.lines() {
            if let Some(url) = m.check(&crate::util::ansi::strip(l)) {
                log_ok = true;
                found_url = found_url.or(url);
            }
        }
    }

    let exit_info: Option<ExitInfo> = loop {
        if !ready && has_probe && log_ok && http_ok && port_ok {
            ready = true;
            let url = found_url.clone().map(|u| if p.inside { container_url(state, pid, &u) } else { u });
            runs.update(state, pid, name, epoch, |s| {
                s.live.state = RunState::Ready;
                s.live.ready_at = Some(crate::util::now_ms());
                s.live.error = None;
                if let Some(u) = url {
                    s.live.port = s.live.port.or_else(|| port_of(&u));
                    s.live.url = Some(u);
                }
            });
        }
        let mut out_closed = false;
        let mut exit_closed = false;
        let mut lagged = false;
        tokio::select! {
            _ = cancel.cancelled() => return,
            msg = recv(&mut out) => match msg {
                Some(Ok(bytes)) => {
                    let mut fresh = lines.push(&bytes);
                    if let Some(partial) = lines.partial() {
                        // Prompts and progress lines have no newline yet: readiness only.
                        if let (false, Some(m)) = (log_ok, &need_log) {
                            if let Some(url) = m.check(&partial) { log_ok = true; found_url = found_url.or(url); }
                        }
                    }
                    for l in fresh.drain(..) {
                        if let (false, Some(m)) = (log_ok, &need_log) {
                            if let Some(url) = m.check(&l) { log_ok = true; found_url = found_url.or(url); }
                        }
                        if let (Some(parser), Some(acc)) = (&p.results, result.as_mut()) {
                            result_dirty |= parser.feed(&l, acc);
                        }
                    }
                }
                Some(Err(broadcast::error::RecvError::Lagged(_))) => lagged = true,
                Some(Err(broadcast::error::RecvError::Closed)) | None => out_closed = true,
            },
            changed = watch_exit(&mut exit) => {
                match changed {
                    Some(info) => break Some(info),
                    None => exit_closed = true,
                }
            },
            _ = tick.tick() => {
                if !http_ok && last_http.elapsed() >= Duration::from_secs(1) {
                    last_http = Instant::now();
                    if let Some(url) = &need_http { http_ok = http_ready(state, url).await; }
                }
                if !port_ok {
                    if let Some(port) = p.port { port_ok = run_port_open(state, pid, p.inside, port, Duration::from_millis(200)).await; }
                }
                if !ready && has_probe && !timed_out && started.elapsed() > timeout {
                    timed_out = true;
                    let msg = format!("not ready after {}s", timeout.as_secs());
                    runs.update(state, pid, name, epoch, |s| s.live.error = Some(msg.clone()));
                    state.events.notify("warning", &format!("{name}: {msg}"));
                }
                if result_dirty && last_emit.elapsed() >= Duration::from_millis(500) {
                    result_dirty = false;
                    last_emit = Instant::now();
                    let r = result.clone();
                    runs.update(state, pid, name, epoch, |s| s.live.result = r);
                }
            }
        }
        if out_closed {
            out = None;
        }
        if exit_closed {
            exit = None;
            if out.is_none() {
                break None;
            }
        }
        // Output we skipped may have held the ready line: look at the screen mirror.
        if let (true, false, Some(m)) = (lagged, log_ok, &need_log) {
            if let Some(text) = state.terminals.screen_text(tid, 300) {
                for l in since_restart(&text).lines() {
                    if let Some(url) = m.check(&crate::util::ansi::strip(l)) {
                        log_ok = true;
                        found_url = found_url.or(url);
                    }
                }
            }
        }
        if out.is_none() && exit.is_none() {
            break None;
        }
    };

    // Drain what is still buffered, then settle the final state.
    if let Some(rx) = out.as_mut() {
        while let Ok(bytes) = rx.try_recv() {
            for l in lines.push(&bytes) {
                if let (Some(parser), Some(acc)) = (&p.results, result.as_mut()) {
                    parser.feed(&l, acc);
                }
            }
        }
    }
    if let (Some(l), Some(parser), Some(acc)) = (lines.flush(), &p.results, result.as_mut()) {
        parser.feed(&l, acc);
    }
    finish(state, pid, name, epoch, p, exit_info, result).await;
}

async fn recv(out: &mut Option<broadcast::Receiver<bytes::Bytes>>) -> Option<Result<bytes::Bytes, broadcast::error::RecvError>> {
    match out {
        Some(rx) => Some(rx.recv().await),
        None => std::future::pending().await,
    }
}

/// Resolves with the exit info once the process exits; `None` if the watch closed without one.
async fn watch_exit(exit: &mut Option<tokio::sync::watch::Receiver<Option<ExitInfo>>>) -> Option<ExitInfo> {
    let Some(rx) = exit else { return std::future::pending().await };
    loop {
        let current = rx.borrow_and_update().clone();
        if current.is_some() {
            return current;
        }
        if rx.changed().await.is_err() {
            let last = rx.borrow().clone();
            return last;
        }
    }
}

/// The part of a terminal's screen text printed by its current process: after the
/// last `── restarted ──` separator the terminals slice writes when a terminal is
/// relaunched (the whole text for a terminal that never was).
fn since_restart(text: &str) -> &str {
    match text.rfind("── restarted ──") {
        Some(i) => text[i..].split_once('\n').map(|(_, rest)| rest).unwrap_or(""),
        None => text,
    }
}

fn port_of(url: &str) -> Option<u16> {
    let rest = url.split("://").nth(1)?;
    let authority = rest.split('/').next()?;
    authority.rsplit_once(':').and_then(|(_, p)| p.parse().ok())
}

async fn http_ready(state: &AppState, url: &str) -> bool {
    match state.http.get(url).timeout(Duration::from_secs(2)).send().await {
        Ok(r) => r.status().is_success() || r.status().is_redirection(),
        Err(_) => false,
    }
}

async fn finish(state: &AppState, pid: &str, name: &str, epoch: u64, p: &Prepared, exit: Option<ExitInfo>, result: Option<TestResult>) {
    let runs = &state.apps.runs;
    let stopping = runs.slots.lock().get(&key(pid, name)).is_some_and(|s| s.epoch == epoch && s.stopping);
    let code = exit.as_ref().and_then(|e| e.code);
    let failed_tests = result.as_ref().is_some_and(|r| r.failed > 0);
    let kind = p.config.kind;

    // A service's start command daemonizes and exits: it is up if `status` (or the port) says so.
    if kind == RunKind::Service && code == Some(0) && !stopping {
        let up = match &p.status {
            Some(st) => {
                let mut ok = false;
                let timeout = p.config.ready.as_ref().map(|r| u64::from(r.timeout_s)).unwrap_or(60).clamp(1, 600);
                let deadline = Instant::now() + Duration::from_secs(timeout);
                while Instant::now() < deadline {
                    if run_quiet(st, p, Duration::from_secs(10)).await == Some(true) {
                        ok = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                ok
            }
            None => match p.port {
                Some(port) => port_open(port, Duration::from_millis(500)).await,
                None => true,
            },
        };
        runs.update(state, pid, name, epoch, |s| {
            s.live.exit = exit.clone();
            if up {
                s.live.state = RunState::Ready;
                s.live.ready_at = Some(crate::util::now_ms());
            } else {
                s.live.state = RunState::Failed;
                s.live.error = Some("the service did not report running after its start command".into());
            }
        });
        return;
    }

    let new_state = if stopping {
        RunState::Stopped
    } else if code == Some(0) && !failed_tests {
        RunState::Exited
    } else if code.is_none() && exit.as_ref().is_some_and(|e| e.signal.is_some()) {
        RunState::Exited // terminated from outside (terminal closed)
    } else {
        RunState::Failed
    };
    let summary = result.as_ref().map(|r| format!("{} passed, {} failed", r.passed, r.failed));
    // 127 is the shell's "command not found": name the program when it is the
    // command's own (`go run ./cmd/api` without Go), so nobody has to open the output.
    let not_found = (code == Some(127) && new_state == RunState::Failed).then(|| match command_program(&p.command) {
        Some(prog) if !program_available(&prog) => format!("command not found: {prog}"),
        _ => "exited with code 127 (command not found)".to_string(),
    });
    runs.update(state, pid, name, epoch, |s| {
        s.live.state = new_state;
        s.live.exit = exit.clone();
        s.live.phase = None;
        if result.is_some() {
            s.live.result = result.clone();
        }
        if new_state == RunState::Failed && s.live.error.is_none() {
            s.live.error = Some(match (code, &summary) {
                (_, Some(sum)) if failed_tests => sum.clone(),
                (Some(127), _) => not_found.clone().unwrap_or_else(|| "exited with code 127".into()),
                (Some(c), _) => format!("exited with code {c}"),
                (None, _) => "exited".into(),
            });
        }
    });
    if stopping {
        return;
    }
    match (kind, new_state) {
        (RunKind::Test | RunKind::Build | RunKind::Task, RunState::Exited) => {
            let msg = match summary {
                Some(s) => format!("{name}: {s}"),
                None => format!("{name} finished"),
            };
            state.events.notify("success", &msg);
        }
        (_, RunState::Failed) => {
            let why = runs.live(pid, name).error.unwrap_or_default();
            state.events.notify("error", &format!("{name} failed: {why}"));
        }
        (RunKind::Server, RunState::Exited) => state.events.notify("warning", &format!("{name} exited")),
        _ => {}
    }
}

/// Run a short command (service status/stop) with the run's cwd and env; `Some(success)`.
async fn run_quiet(cmd: &str, p: &Prepared, timeout: Duration) -> Option<bool> {
    let mut c = crate::util::os::shell::run_command(cmd);
    c.current_dir(&p.cwd);
    for (k, v) in &p.env {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    crate::util::proc::run_cmd(c, timeout).await.ok().map(|o| o.ok())
}

// ---------------------------------------------------------------- stop

pub async fn stop(state: &AppState, project: &Project, name: &str) -> Result<RunLive, ApiError> {
    let c = find(project, name)?.clone();
    // A new epoch makes any in-flight supervisor stale: it can no longer change the
    // state, and one that is mid-spawn kills the terminal it just created.
    let (tid, epoch) = {
        let mut slots = state.apps.runs.slots.lock();
        let slot = slots.entry(key(&project.id, name)).or_insert_with(Slot::new);
        slot.stopping = true;
        slot.cancel.cancel();
        slot.epoch = state.apps.runs.epochs.fetch_add(1, Ordering::Relaxed) + 1;
        (slot.live.terminal_id.clone(), slot.epoch)
    };
    let mut error = None;
    if c.kind == RunKind::Service {
        if let Some(stop_cmd) = &c.stop {
            let vars = expand::base_vars(project);
            let cwd = resolve_cwd(project, &c.cwd)?;
            let mut cmd = crate::util::os::shell::run_command(&expand::placeholders(stop_cmd, &vars));
            cmd.current_dir(&cwd);
            match crate::util::proc::run_cmd(cmd, Duration::from_secs(60)).await {
                Ok(o) if !o.ok() => {
                    error = Some(format!("stop command failed: {}", crate::apps::detect::text::ellipsize(&o.message(), 300)))
                }
                Err(e) => error = Some(e.message),
                _ => {}
            }
        }
    }
    if let Some(tid) = &tid {
        if state.terminals.info(tid).is_some_and(|i| i.exit.is_none()) {
            let _ = state.terminals.kill(tid).await;
            if let Some(mut rx) = state.terminals.exit_watch(tid) {
                let _ = tokio::time::timeout(Duration::from_secs(8), async {
                    loop {
                        let done = rx.borrow().is_some();
                        if done || rx.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await;
            }
        }
    }
    let live = {
        let mut slots = state.apps.runs.slots.lock();
        let Some(slot) = slots.get_mut(&key(&project.id, name)) else { return Ok(RunLive::default()) };
        if slot.epoch == epoch {
            slot.stopping = false;
            slot.live.state = RunState::Stopped;
            slot.live.phase = None;
            slot.live.error = error.clone();
            slot.live.exit = tid.as_deref().and_then(|t| state.terminals.info(t)).and_then(|i| i.exit).or(slot.live.exit.take());
            slot.started_by_us = false;
            slot.service_checked = None;
        }
        slot.live.clone()
    };
    state.apps.runs.changed.notify_waiters();
    emit(state, &project.id, name, &live);
    match error {
        Some(e) => Err(ApiError::internal(e)),
        None => Ok(live),
    }
}

/// Stop every active run of a project.
pub async fn stop_all(state: &AppState, project: &Project) -> usize {
    let active: Vec<String> = project
        .config
        .runs
        .iter()
        .filter(|r| state.apps.runs.live(&project.id, &r.name).state.active())
        .map(|r| r.name.clone())
        .collect();
    let n = active.len();
    futures::future::join_all(active.iter().map(|r| stop(state, project, r))).await;
    n
}

pub async fn restart(state: &AppState, project: &Arc<Project>, name: &str, free: bool) -> Result<(), ApiError> {
    if state.apps.runs.live(&project.id, name).state.active() {
        stop(state, project, name).await?;
    }
    start(state, project, name, free).await
}

/// Refresh service states from their `status` command (throttled, in the background).
fn refresh_services(state: &AppState, project: &Project) {
    let due: Vec<RunConfig> = {
        let mut slots = state.apps.runs.slots.lock();
        project
            .config
            .runs
            .iter()
            .filter(|r| r.kind == RunKind::Service && r.status.is_some())
            .filter(|r| {
                let slot = slots.entry(key(&project.id, &r.name)).or_insert_with(Slot::new);
                let due = slot.service_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(15))
                    && !matches!(slot.live.state, RunState::Starting)
                    && !slot.stopping;
                if due {
                    slot.service_checked = Some(Instant::now());
                }
                due
            })
            .cloned()
            .collect()
    };
    for c in due {
        let st = state.clone();
        let pid = project.id.clone();
        let root = project.root.clone();
        let vars = expand::base_vars(project);
        let cwd = resolve_cwd(project, &c.cwd).unwrap_or(root);
        tokio::spawn(async move {
            let Some(status) = c.status.as_deref() else { return };
            let mut cmd = crate::util::os::shell::run_command(&expand::placeholders(status, &vars));
            cmd.current_dir(&cwd);
            let Ok(out) = crate::util::proc::run_cmd(cmd, Duration::from_secs(3)).await else { return };
            let up = out.ok();
            let live = {
                let mut slots = st.apps.runs.slots.lock();
                let Some(slot) = slots.get_mut(&key(&pid, &c.name)) else { return };
                let before = slot.live.state;
                let after = match (up, before) {
                    (true, RunState::Ready | RunState::Running | RunState::Starting) => before,
                    (true, _) => RunState::Ready,
                    (false, RunState::Ready) if slot.live.terminal_id.as_ref().is_none_or(|t| st.terminals.info(t).is_none_or(|i| i.exit.is_some())) => RunState::Stopped,
                    (false, s) => s,
                };
                if after == before {
                    return;
                }
                slot.live.state = after;
                slot.live.clone()
            };
            st.apps.runs.changed.notify_waiters();
            emit(&st, &pid, &c.name, &live);
        });
    }
}

/// On server shutdown: kill run terminals and stop services Workbench started.
pub async fn shutdown(state: &AppState) {
    let (terminals, services): (Vec<String>, Vec<(String, String)>) = {
        let slots = state.apps.runs.slots.lock();
        let t = slots
            .values()
            .filter(|s| s.live.state.active())
            .filter_map(|s| s.live.terminal_id.clone())
            .collect();
        let sv = slots.iter().filter(|(_, s)| s.started_by_us).map(|(k, _)| k.clone()).collect();
        (t, sv)
    };
    let kills = futures::future::join_all(terminals.iter().map(|t| state.terminals.kill(t)));
    let stops = futures::future::join_all(services.iter().filter_map(|(pid, name)| {
        let p = state.projects.get(pid)?;
        let st = state.clone();
        let name = name.clone();
        Some(async move {
            let _ = stop(&st, &p, &name).await;
        })
    }));
    let _ = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(kills, stops) }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_from_urls() {
        assert_eq!(port_of("http://localhost:5173/"), Some(5173));
        assert_eq!(port_of("http://[::1]:4173"), Some(4173));
        assert_eq!(port_of("https://example.com/x"), None);
    }

    #[test]
    fn only_the_current_process_output_counts_after_a_restart() {
        assert_eq!(since_restart("a\nb\n"), "a\nb\n");
        assert_eq!(since_restart("READY\n── restarted ──\nnew\n"), "new\n");
        assert_eq!(since_restart("x\n── restarted ──\nREADY\n── restarted ──\n"), "");
    }

    #[test]
    fn run_state_activity() {
        assert!(RunState::Starting.active() && RunState::Running.active() && RunState::Ready.active());
        assert!(!RunState::Stopped.active() && !RunState::Failed.active() && !RunState::Exited.active());
        assert_eq!(serde_json::to_value(RunState::Ready).unwrap(), "ready");
    }
}

//! Dev containers slice (OWNER: devcontainer slice).
//!
//! A project with a `devcontainer.json` can have its container built and started on
//! this computer's Docker; its shells, run configurations and (optionally) agent
//! sessions then run *inside* it (`docker exec` in the host PTY), while the files
//! stay shared through the workspace bind mount, so the editor, git and search keep
//! working on the host.
//!
//! **Trust.** The config, its Dockerfile and compose files are repository content.
//! Nothing is ever built or started by itself: `POST …/devcontainer/start` needs the
//! approval hash of the exact plan the user saw (`plan`), remembered per project and
//! config in `data_dir/devcontainer/<id>.json`; any change asks again. Agents (MCP)
//! can read the status but never start, rebuild, stop or remove.
//!
//! Modules: `jsonc` (comments, trailing commas), `config` (discovery, parsing,
//! variables), `plan` (engine choice, risks, approval hash), `engine` (the up script
//! a visible terminal runs), `docker` (CLI helpers with timeouts), `exec` (running
//! terminals inside), `bridge` (hooks/MCP listener for agents in containers), `store`
//! (per-project state), `ops` (start/stop/remove), `scaffold` (a starter config),
//! `services` (every container and image for the Services tool window, `/api/docker`),
//! `routes`, `tools`.
//!
//! CONTRACT (other slices): `summary` (projects), `running_target` / `exec_target` /
//! `kill_inside` (terminals), `run_inside` / `port_route` (apps), `agent_command`
//! (terminals, agents in containers), `router`, `start`, `shutdown`, `mcp_tools`.

mod bridge;
pub(crate) mod config;
pub(crate) mod docker;
mod engine;
pub(crate) mod exec;
mod jsonc;
mod ops;
pub(crate) mod plan;
mod routes;
mod scaffold;
mod services;
mod store;
mod tools;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;

use crate::app::AppState;
use crate::mcp::McpTool;
use crate::projects::Project;

pub use exec::ExecTarget;
pub use routes::router;

use docker::ContainerInfo;
use plan::Engines;

// ---------------------------------------------------------------- shared helpers

/// A shell word: bare when safe, else single-quoted.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c)) {
        s.to_string()
    } else {
        sh_quote_always(s)
    }
}

pub fn sh_quote_always(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A path for display: project-relative when inside `root`.
pub fn rel_display(root: &Path, abs: &str) -> String {
    match Path::new(abs).strip_prefix(root) {
        Ok(r) if r.as_os_str().is_empty() => ".".into(),
        Ok(r) => r.display().to_string(),
        Err(_) => abs.to_string(),
    }
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DcState {
    None,
    Stopped,
    Running,
    Building,
    Error,
}

/// `ProjectSummary.devcontainer`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub configs: Vec<String>,
    pub state: DcState,
    pub in_container: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub kind: &'static str,
    pub terminal_id: Option<String>,
    pub started_at: i64,
}

/// What `docker exec` probing found in a container (per container id).
#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub container_id: String,
    pub shell: String,
    pub has_bash: bool,
    pub has_curl: bool,
    /// Agent CLI name → absolute path (or `None`: not installed).
    pub commands: BTreeMap<String, Option<String>>,
    pub at: Option<Instant>,
}

#[derive(Default)]
struct ProjectRt {
    /// The project's container (the selected config's, else a running one).
    container: Option<ContainerInfo>,
    /// Every container labelled with the project folder.
    all: Vec<ContainerInfo>,
    op: Option<Operation>,
    error: Option<String>,
    emitted: Option<(DcState, Option<String>, bool)>,
    probe: Option<Probe>,
    saved: Option<store::Saved>,
}

#[derive(Default)]
pub struct DevcontainerState {
    rt: Mutex<HashMap<String, ProjectRt>>,
    engines: Mutex<Option<(Instant, Engines)>>,
    bridge: bridge::Bridge,
    refresh_lock: tokio::sync::Mutex<()>,
    refresh_wanted: tokio::sync::Notify,
    docker_ok: AtomicBool,
    started: AtomicBool,
    /// Cancelled on shutdown: stops the poller and the docker events watcher.
    stop: tokio_util::sync::CancellationToken,
}

impl DevcontainerState {
    fn saved(&self, state: &AppState, pid: &str) -> store::Saved {
        let mut rt = self.rt.lock();
        let e = rt.entry(pid.to_string()).or_default();
        if e.saved.is_none() {
            e.saved = Some(store::load(&state.paths.data_dir, pid));
        }
        e.saved.clone().unwrap_or_default()
    }

    fn update_saved(&self, state: &AppState, pid: &str, f: impl FnOnce(&mut store::Saved)) -> anyhow::Result<store::Saved> {
        let mut s = self.saved(state, pid);
        f(&mut s);
        store::save(&state.paths.data_dir, pid, &s)?;
        self.rt.lock().entry(pid.to_string()).or_default().saved = Some(s.clone());
        Ok(s)
    }
}

/// `[devcontainer]` settings.
fn settings(state: &AppState) -> crate::config::global::DevcontainerConfig {
    state.config.read().devcontainer.clone()
}

pub(crate) fn docker_path(state: &AppState) -> String {
    docker::docker_bin(&settings(state).docker)
}

/// How to run the devcontainer CLI, if it is installed or configured.
pub(crate) fn cli_command(state: &AppState) -> Option<Vec<String>> {
    let c = settings(state).cli.trim().to_string();
    if c == "npx" {
        return crate::util::which_path("npx").map(|p| vec![p.display().to_string(), "-y".into(), "@devcontainers/cli".into()]);
    }
    if c.is_empty() {
        return crate::util::which_path("devcontainer").map(|p| vec![p.display().to_string()]);
    }
    let p = crate::config::expand_tilde(&c);
    if p.is_file() {
        return Some(vec![p.display().to_string()]);
    }
    crate::util::which_path(&c).map(|p| vec![p.display().to_string()])
}

/// What is installed (cached for 30 s).
pub(crate) async fn engines(state: &AppState) -> Engines {
    if let Some((at, e)) = state.devcontainer.engines.lock().as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return e.clone();
        }
    }
    let docker = docker_path(state);
    let (version, compose) = tokio::join!(docker::server_version(&docker), docker::compose_version(&docker));
    let s = settings(state);
    let e = Engines {
        docker: version.as_ref().ok().cloned(),
        docker_error: version.err(),
        compose,
        cli: cli_command(state).map(|v| v.join(" ")),
        preference: if s.engine.is_empty() { "auto".into() } else { s.engine.clone() },
    };
    state.devcontainer.docker_ok.store(e.docker.is_some(), Ordering::Relaxed);
    *state.devcontainer.engines.lock() = Some((Instant::now(), e.clone()));
    e
}

fn state_of(rt: &ProjectRt) -> DcState {
    if rt.op.as_ref().is_some_and(|o| o.kind == "start" || o.kind == "rebuild") {
        return DcState::Building;
    }
    match &rt.container {
        Some(c) if c.running => DcState::Running,
        _ if rt.error.is_some() => DcState::Error,
        Some(_) => DcState::Stopped,
        None => DcState::None,
    }
}

fn use_container_of(saved: &store::Saved) -> bool {
    saved.use_container.unwrap_or(saved.attached)
}

/// `ProjectSummary.devcontainer`: `None` for a project without configs or containers.
pub fn summary(state: &AppState, p: &Project) -> Option<Summary> {
    let configs = config::discover(&p.root);
    let saved = state.devcontainer.saved(state, &p.id);
    let rt = state.devcontainer.rt.lock();
    let r = rt.get(&p.id);
    let has_container = r.is_some_and(|r| r.container.is_some());
    if configs.is_empty() && !has_container {
        return None;
    }
    let st = r.map(state_of).unwrap_or(DcState::None);
    Some(Summary { configs, state: st, in_container: st == DcState::Running && use_container_of(&saved) })
}

/// The container's labels name this project folder.
fn folder_matches(p: &Project, folder: &str) -> bool {
    let root = p.root.display().to_string();
    folder == root || std::fs::canonicalize(folder).is_ok_and(|c| c == p.root)
}

fn config_abs(p: &Project, rel: &str) -> String {
    p.root.join(rel).display().to_string()
}

/// The selected config: the saved choice when it still exists, else the first.
pub(crate) fn selected_config(state: &AppState, p: &Project) -> Option<String> {
    let configs = config::discover(&p.root);
    let saved = state.devcontainer.saved(state, &p.id);
    saved.config.filter(|c| configs.contains(c)).or_else(|| configs.first().cloned())
}

fn pick(p: &Project, all: &[ContainerInfo], selected: Option<&str>) -> Option<ContainerInfo> {
    let want = selected.map(|c| config_abs(p, c));
    let same_config = |c: &&ContainerInfo| want.as_deref().is_some_and(|w| c.label("devcontainer.config_file") == Some(w));
    all.iter()
        .filter(same_config)
        .max_by_key(|c| (c.running, c.created.clone()))
        .or_else(|| all.iter().filter(|c| c.running).max_by_key(|c| c.created.clone()))
        .or_else(|| if selected.is_none() { all.iter().max_by_key(|c| c.created.clone()) } else { None })
        .cloned()
}

/// Refresh every project's container from Docker (one `ps`, one `inspect`) and emit
/// `devcontainer.state` for changes. Serialized; cheap when nothing is labelled.
pub async fn refresh(state: &AppState) {
    let _serial = state.devcontainer.refresh_lock.lock().await;
    let docker = docker_path(state);
    let listed = match docker::list_devcontainers(&docker).await {
        Ok(l) => {
            state.devcontainer.docker_ok.store(true, Ordering::Relaxed);
            l
        }
        // Docker not answering (stopped, slow): keep what is known rather than report
        // every container gone.
        Err(_) => {
            state.devcontainer.docker_ok.store(false, Ordering::Relaxed);
            return;
        }
    };
    let projects = state.projects.list();
    let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
    for (id, folder) in &listed {
        if let Some(p) = projects.iter().find(|p| folder_matches(p, folder)) {
            by_project.entry(p.id.clone()).or_default().push(id.clone());
        }
    }
    let ids: Vec<String> = by_project.values().flatten().cloned().collect();
    let infos = docker::inspect(&docker, &ids).await.unwrap_or_default();
    let mut changed = vec![];
    for p in &projects {
        let mine: Vec<ContainerInfo> = infos.iter().filter(|c| by_project.get(&p.id).is_some_and(|v| v.contains(&c.id))).cloned().collect();
        let selected = selected_config(state, p);
        let chosen = pick(p, &mine, selected.as_deref());
        let saved = state.devcontainer.saved(state, &p.id);
        let mut rt = state.devcontainer.rt.lock();
        let r = rt.entry(p.id.clone()).or_default();
        if chosen.as_ref().is_some_and(|c| c.running) && r.op.is_none() {
            r.error = None;
        }
        if r.probe.as_ref().is_some_and(|pr| chosen.as_ref().is_none_or(|c| c.id != pr.container_id || !c.running)) {
            r.probe = None;
        }
        r.container = chosen;
        r.all = mine;
        let now = (state_of(r), r.container.as_ref().map(|c| c.id.clone()), use_container_of(&saved));
        if r.emitted.as_ref() != Some(&now) {
            r.emitted = Some(now.clone());
            changed.push((p.id.clone(), now));
        }
    }
    for (pid, (st, cid, inside)) in changed {
        emit_state(state, &pid, st, cid.as_deref(), inside);
    }
    sync_bridge(state).await;
}

fn emit_state(state: &AppState, pid: &str, st: DcState, cid: Option<&str>, in_container: bool) {
    state.events.emit(
        "devcontainer.state",
        Some(pid),
        json!({ "projectId": pid, "state": st, "containerId": cid.map(|c| &c[..c.len().min(12)]), "inContainer": in_container && st == DcState::Running }),
    );
}

/// Emit the current state of one project (after an operation changed it).
pub(crate) fn announce(state: &AppState, pid: &str) {
    let saved = state.devcontainer.saved(state, pid);
    let now = {
        let mut rt = state.devcontainer.rt.lock();
        let r = rt.entry(pid.to_string()).or_default();
        let now = (state_of(r), r.container.as_ref().map(|c| c.id.clone()), use_container_of(&saved));
        r.emitted = Some(now.clone());
        now
    };
    emit_state(state, pid, now.0, now.1.as_deref(), now.2);
}

/// Listen on the gateways of the running containers Workbench uses (started or
/// attached, used for terminals, or running a terminal right now); close the rest.
async fn sync_bridge(state: &AppState) {
    let mut needed: Vec<IpAddr> = vec![];
    // Containers with a terminal of ours running inside (short ids).
    let busy: Vec<String> = state
        .terminals
        .list()
        .into_iter()
        .filter(|t| t.status != crate::terminals::TerminalStatus::Exited && crate::terminals::in_container(t))
        .filter_map(|t| t.meta.get("container").and_then(|c| c.get("id")).and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let projects: Vec<String> = state.devcontainer.rt.lock().keys().cloned().collect();
    for pid in projects {
        let saved = state.devcontainer.saved(state, &pid);
        let found = {
            let rt = state.devcontainer.rt.lock();
            rt.get(&pid).and_then(|r| r.container.as_ref()).filter(|c| c.running).map(|c| (c.gateway.clone(), c.short_id().to_string()))
        };
        if let Some((Some(gw), short)) = found {
            let Ok(gw) = gw.parse::<IpAddr>() else { continue };
            if saved.attached || saved.use_container == Some(true) || busy.contains(&short) {
                needed.push(gw);
            }
        }
    }
    for ip in &needed {
        state.devcontainer.bridge.ensure(state, *ip).await;
    }
    state.devcontainer.bridge.retain(&needed);
}

fn container_of(state: &AppState, pid: &str) -> Option<ContainerInfo> {
    state.devcontainer.rt.lock().get(pid).and_then(|r| r.container.clone())
}

/// The remote user of a container: what the last start reported, else the
/// `devcontainer.metadata` label (VS Code, the CLI and Workbench write it).
fn remote_user_of(c: &ContainerInfo, saved: &store::Saved) -> Option<String> {
    if let Some(l) = saved.last.as_ref().filter(|l| l.container_id == c.id) {
        if let Some(u) = l.remote_user.clone().filter(|u| !u.is_empty()) {
            return Some(u);
        }
    }
    let meta: serde_json::Value = c.label("devcontainer.metadata").and_then(|m| serde_json::from_str(m).ok()).unwrap_or_default();
    let entries = meta.as_array().cloned().unwrap_or_default();
    let find = |k: &str| entries.iter().rev().find_map(|e| e.get(k).and_then(|v| v.as_str()).map(str::to_string));
    find("remoteUser").or_else(|| find("containerUser")).or_else(|| (!c.user.is_empty()).then(|| c.user.clone()))
}

/// Host folder ↔ container folder of the project: the bind mount whose source is the
/// project root or one of its parents (the CLI mounts the git root).
fn mapping_of(p: &Project, c: &ContainerInfo) -> Option<(std::path::PathBuf, String)> {
    let canon = |s: &str| std::fs::canonicalize(s).unwrap_or_else(|_| s.into());
    c.mounts
        .iter()
        .filter(|m| m.kind == "bind" && !m.source.is_empty())
        .filter(|m| p.root.starts_with(canon(&m.source)))
        .max_by_key(|m| m.source.len())
        .map(|m| (canon(&m.source), m.destination.clone()))
}

/// The workspace mount (host folder, container folder, as `ExecTarget::map`) of the
/// project's container whose id starts with `container` (a terminal's
/// `meta.container.id`), from what the poller last saw: no Docker call. `None`
/// when that container is not known (then a container path cannot be mapped).
pub fn workspace_mount(state: &AppState, pid: &str, container: &str) -> Option<(std::path::PathBuf, String)> {
    if container.is_empty() {
        return None;
    }
    let p = state.projects.get(pid)?;
    let c = {
        let rt = state.devcontainer.rt.lock();
        let r = rt.get(pid)?;
        r.container.iter().chain(r.all.iter()).find(|c| c.id.starts_with(container)).cloned()?
    };
    mapping_of(&p, &c)
}

/// The config's remoteEnv for execution (host variables resolved), if it parses.
fn remote_env(p: &Project, cfg_rel: Option<&str>) -> Vec<(String, Option<String>)> {
    let Some(rel) = cfg_rel else { return vec![] };
    config::load(&p.root, rel, config::LocalEnv::Resolve).map(|c| c.remote_env.into_iter().collect()).unwrap_or_default()
}

/// Probe a running container for its shell, bash, curl and agent CLIs (cached per
/// container for a minute).
pub(crate) async fn probe(state: &AppState, pid: &str, c: &ContainerInfo, user: Option<&str>) -> Probe {
    if let Some(p) = state.devcontainer.rt.lock().get(pid).and_then(|r| r.probe.clone()) {
        if p.container_id == c.id && p.at.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return p;
        }
    }
    let cfg = state.config.read().agents.clone();
    let mut names: Vec<String> = crate::terminals::provider_commands(&cfg);
    names.sort();
    names.dedup();
    let script = r#"u=$(id -un 2>/dev/null || echo root)
s=$(sed -n "s/^$u:[^:]*:[^:]*:[^:]*:[^:]*:[^:]*://p" /etc/passwd 2>/dev/null | head -n1)
echo "shell=$s"
echo "bash=$(command -v bash)"
echo "curl=$(command -v curl)"
for c in "$@"; do echo "cmd:$c=$(command -v "$c" 2>/dev/null)"; done"#;
    let mut argv: Vec<&str> = vec!["/bin/sh", "-lc", script, "probe"];
    argv.extend(names.iter().map(String::as_str));
    let out = docker::exec(&docker_path(state), &c.id, user, &argv, Duration::from_secs(15)).await;
    let mut p = Probe { container_id: c.id.clone(), at: Some(Instant::now()), ..Default::default() };
    if let Ok(o) = out {
        for l in o.stdout.lines() {
            if let Some(v) = l.strip_prefix("shell=") {
                p.shell = v.trim().to_string();
            } else if let Some(v) = l.strip_prefix("bash=") {
                p.has_bash = !v.trim().is_empty();
            } else if let Some(v) = l.strip_prefix("curl=") {
                p.has_curl = !v.trim().is_empty();
            } else if let Some(rest) = l.strip_prefix("cmd:") {
                if let Some((name, path)) = rest.split_once('=') {
                    let path = path.trim();
                    p.commands.insert(name.to_string(), (path.starts_with('/')).then(|| path.to_string()));
                }
            }
        }
    }
    // No passwd entry, or a system user's `nologin`: bash, else sh.
    if p.shell.is_empty() || !p.shell.starts_with('/') || p.shell.ends_with("nologin") || p.shell.ends_with("/false") {
        p.shell = if p.has_bash { "/bin/bash".into() } else { "/bin/sh".into() };
    }
    if let Some(r) = state.devcontainer.rt.lock().get_mut(pid) {
        r.probe = Some(p.clone());
    }
    p
}

/// The project's running container as terminals use it, whatever "use the container"
/// says (a terminal that chose the container keeps it). `Err` explains why not.
pub async fn running_target(state: &AppState, pid: &str) -> Result<ExecTarget, String> {
    let p = state.projects.get(pid).ok_or_else(|| format!("no project {pid:?}"))?;
    let mut c = container_of(state, pid);
    if c.as_ref().is_none_or(|c| !c.running) {
        // State may be stale (started from outside): look once.
        refresh(state).await;
        c = container_of(state, pid);
    }
    let c = c.filter(|c| c.running).ok_or_else(|| format!("the dev container of {pid} is not running: start it from the Dev container panel, or use a host terminal"))?;
    let saved = state.devcontainer.saved(state, pid);
    let user = remote_user_of(&c, &saved);
    let map = mapping_of(&p, &c);
    // remoteEnv of the config that made this container (another config may be selected).
    let cfg_rel = c
        .label("devcontainer.config_file")
        .and_then(|f| std::path::Path::new(f).strip_prefix(&p.root).ok().map(|r| r.display().to_string()))
        .filter(|r| config::is_config(&p.root, r))
        .or_else(|| selected_config(state, &p));
    let folder = saved
        .last
        .as_ref()
        .filter(|l| l.container_id == c.id)
        .and_then(|l| l.workspace_folder.clone())
        .or_else(|| map.as_ref().and_then(|(src, dst)| p.root.strip_prefix(src).ok().map(|r| {
            let r = r.display().to_string();
            if r.is_empty() { dst.clone() } else { format!("{}/{r}", dst.trim_end_matches('/')) }
        })))
        .unwrap_or_else(|| "/".into());
    let probe = probe(state, pid, &c, user.as_deref()).await;
    let workbench_url = bridge_url(state, &c).await;
    Ok(ExecTarget {
        project_id: pid.to_string(),
        container_id: c.id.clone(),
        container_name: c.name.clone(),
        user,
        map,
        folder,
        remote_env: remote_env(&p, cfg_rel.as_deref()),
        workbench_url,
        docker: docker_path(state),
        shell: probe.shell,
        has_bash: probe.has_bash,
    })
}

/// `running_target` when the project's terminals and runs should use the container
/// ("Run terminals and runs in the container" is on).
pub async fn exec_target(state: &AppState, pid: &str) -> Option<ExecTarget> {
    if !uses_container(state, pid) {
        return None;
    }
    running_target(state, pid).await.ok()
}

/// Whether new shells and runs of the project go into its running container.
pub fn uses_container(state: &AppState, pid: &str) -> bool {
    let running = container_of(state, pid).is_some_and(|c| c.running);
    running && use_container_of(&state.devcontainer.saved(state, pid))
}

/// Whether run configuration `name` runs inside (the container is used and the run is
/// not pinned to the host).
pub fn run_inside(state: &AppState, pid: &str, name: &str) -> bool {
    uses_container(state, pid) && !state.devcontainer.saved(state, pid).host_runs.contains(name)
}

/// The bridge URL for agents in `c` (starting its listener), or the host's loopback URL
/// when the container shares the host network.
pub(crate) async fn bridge_url(state: &AppState, c: &ContainerInfo) -> Option<String> {
    if c.network_mode == "host" {
        return Some(state.local_base_url());
    }
    let gw: IpAddr = match c.gateway.as_deref().and_then(|g| g.parse().ok()) {
        Some(g) => g,
        None => docker::network_gateway(&docker_path(state), "bridge").await?.parse().ok()?,
    };
    state.devcontainer.bridge.ensure(state, gw).await
}

/// How the host reaches a port a process in the container listens on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortRoute {
    pub port: u16,
    /// Where readiness is probed: the container's own address (a published port's
    /// proxy accepts connections before anything listens inside).
    pub probe: SocketAddr,
    /// Where a browser on this computer opens it.
    pub url_host: String,
    pub url_port: u16,
    /// `published` (127.0.0.1:<hostPort>), `container-ip` or `host-network`.
    pub via: &'static str,
}

/// The route to `port` of the project's running container.
pub fn port_route(state: &AppState, pid: &str, port: u16) -> Option<PortRoute> {
    let c = container_of(state, pid).filter(|c| c.running)?;
    if c.network_mode == "host" {
        let a = SocketAddr::from(([127, 0, 0, 1], port));
        return Some(PortRoute { port, probe: a, url_host: "localhost".into(), url_port: port, via: "host-network" });
    }
    let ip: IpAddr = c.ip.as_deref()?.parse().ok()?;
    let probe = SocketAddr::new(ip, port);
    let published = c
        .ports
        .iter()
        .find(|m| m.port == port && m.proto == "tcp" && m.host_port.is_some() && m.host_ip.as_deref().is_none_or(|h| h == "127.0.0.1" || h == "0.0.0.0"));
    Some(match published {
        Some(m) => PortRoute { port, probe, url_host: "localhost".into(), url_port: m.host_port.unwrap_or(port), via: "published" },
        None => PortRoute { port, probe, url_host: ip.to_string(), url_port: port, via: "container-ip" },
    })
}

/// The absolute path of an agent CLI inside the project's running container.
pub async fn agent_command(state: &AppState, pid: &str, command: &str) -> Result<(ExecTarget, String), String> {
    let t = running_target(state, pid).await?;
    let c = container_of(state, pid).ok_or("the dev container is not running")?;
    let name = Path::new(command).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| command.to_string());
    let mut p = probe(state, pid, &c, t.user.as_deref()).await;
    if p.commands.get(&name).is_none_or(|path| path.is_none()) {
        // Not probed (not a configured provider's command), or missing a minute ago
        // (it may have been installed since): look again.
        let out = docker::exec(&t.docker, &c.id, t.user.as_deref(), &["/bin/sh", "-lc", "command -v \"$1\"", "probe", &name], Duration::from_secs(10)).await;
        let path = out.ok().map(|o| o.stdout.trim().to_string()).filter(|s| s.starts_with('/'));
        if let Some(pr) = state.devcontainer.rt.lock().get_mut(pid).and_then(|r| r.probe.as_mut()) {
            pr.commands.insert(name.clone(), path.clone());
        }
        p.commands.insert(name.clone(), path);
    }
    match p.commands.get(&name).cloned().flatten() {
        Some(path) => Ok((t, path)),
        None => Err(format!("{name} is not installed in the dev container (as {})", t.user.as_deref().unwrap_or("its default user"))),
    }
}

/// Whether the container can post the SessionStart hook itself (it has curl).
pub async fn container_has_curl(state: &AppState, pid: &str) -> bool {
    let Some(c) = container_of(state, pid) else { return false };
    let saved = state.devcontainer.saved(state, pid);
    probe(state, pid, &c, remote_user_of(&c, &saved).as_deref()).await.has_curl
}

pub use exec::kill_inside;

/// Write a per-session file into the container (0600, owned by the exec user).
pub async fn write_into(t: &ExecTarget, path: &str, data: &[u8]) -> Result<(), String> {
    docker::write_file(&t.docker, &t.container_id, t.user.as_deref(), path, data).await
}

// ---------------------------------------------------------------- lifecycle

pub async fn start(state: &AppState) {
    if state.devcontainer.started.swap(true, Ordering::Relaxed) {
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        refresh(&st).await;
        // Poll every 15 s while someone looks; events wake it up sooner.
        loop {
            let wake = st.devcontainer.refresh_wanted.notified();
            let _ = tokio::time::timeout(Duration::from_secs(15), wake).await;
            if st.devcontainer.stop.is_cancelled() {
                return;
            }
            if st.events.ui_clients() > 0 || st.devcontainer.bridge.active().len() > 0 {
                // A burst of docker events settles first.
                tokio::time::sleep(Duration::from_millis(300)).await;
                refresh(&st).await;
            }
        }
    });
    tokio::spawn(watch_docker_events(state.clone()));
}

/// Follow `docker events` for containers and images: a dev container's event refreshes
/// the projects' state; any event that changes what the Services tool window shows
/// becomes one `docker.changed {kinds}` (300 ms debounce). Health checks' and probes'
/// `exec_*` events change nothing.
async fn watch_docker_events(state: AppState) {
    use tokio::io::AsyncBufReadExt;
    const FORMAT: &str = "{{.Type}}\t{{.Action}}\t{{index .Actor.Attributes \"devcontainer.local_folder\"}}";
    loop {
        let docker = docker_path(&state);
        let mut cmd = tokio::process::Command::new(&docker);
        cmd.args(["events", "--format", FORMAT, "--filter", "type=container", "--filter", "type=image"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        crate::util::proc::clean_env(&mut cmd);
        if let Ok(mut child) = cmd.spawn() {
            if let Some(out) = child.stdout.take() {
                let mut lines = tokio::io::BufReader::new(out).lines();
                // When to emit `docker.changed`, and for which kinds.
                let mut due: Option<(tokio::time::Instant, Vec<String>)> = None;
                loop {
                    let wake = due.as_ref().map(|d| d.0).unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(5));
                    tokio::select! {
                        l = lines.next_line() => match l {
                            Ok(Some(l)) => {
                                let mut f = l.split('\t');
                                let (kind, action, folder) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
                                if !services::event_matters(action) {
                                    continue;
                                }
                                if kind == "container" && !folder.trim().is_empty() {
                                    state.devcontainer.refresh_wanted.notify_one();
                                }
                                let d = due.get_or_insert_with(|| (tokio::time::Instant::now() + Duration::from_millis(300), vec![]));
                                if !d.1.iter().any(|k| k == kind) {
                                    d.1.push(kind.to_string());
                                }
                            }
                            _ => break,
                        },
                        _ = tokio::time::sleep_until(wake) => {
                            if let Some((_, kinds)) = due.take() {
                                state.events.emit("docker.changed", None, json!({ "kinds": kinds }));
                            }
                            if state.devcontainer.stop.is_cancelled() { return; }
                        }
                    }
                }
            }
            let _ = child.kill().await;
        }
        if state.devcontainer.stop.is_cancelled() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

pub async fn shutdown(state: &AppState) {
    state.devcontainer.stop.cancel();
    state.devcontainer.bridge.shutdown();
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

/// For tests and ops: the project's current container.
pub(crate) fn current(state: &AppState, pid: &str) -> Option<ContainerInfo> {
    container_of(state, pid)
}

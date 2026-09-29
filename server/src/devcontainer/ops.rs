//! Start (behind the approval), rebuild, stop and remove; the panel's view; settings.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::{self, LocalEnv};
use super::plan::{self, Engine, Plan};
use super::{DcState, Operation, announce, docker, engine, store};
use crate::app::AppState;
use crate::error::ApiError;
use crate::projects::Project;
use crate::terminals::{SpawnSpec, TerminalKind};

/// Why a start needs the user first: the plan to show and its hash.
pub struct ApprovalRequired {
    pub plan: Plan,
    pub message: String,
}

pub enum StartError {
    Api(ApiError),
    Approval(Box<ApprovalRequired>),
}

impl From<ApiError> for StartError {
    fn from(e: ApiError) -> Self {
        StartError::Api(e)
    }
}

/// The config a request names (must be one of the project's), else the selected one.
pub fn config_rel(state: &AppState, p: &Project, requested: Option<&str>) -> Result<String, ApiError> {
    match requested.map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => {
            if config::is_config(&p.root, r) {
                Ok(r.to_string())
            } else {
                Err(ApiError::not_found(format!("{r} is not a devcontainer.json of {}", p.id)))
            }
        }
        None => super::selected_config(state, p).ok_or_else(|| ApiError::not_found(format!("{} has no devcontainer.json", p.id))),
    }
}

/// The display plan of `rel`.
pub async fn plan_of(state: &AppState, p: &Project, rel: &str) -> Result<Plan, ApiError> {
    let engines = super::engines(state).await;
    let root = p.root.clone();
    let rel = rel.to_string();
    tokio::task::spawn_blocking(move || -> Result<Plan, ApiError> {
        let abs = crate::util::paths::resolve_in_root(&root, &rel)?;
        let text = std::fs::read(&abs).map_err(|e| ApiError::not_found(format!("{rel}: {e}")))?;
        let c = config::load(&root, &rel, LocalEnv::Keep).map_err(ApiError::bad_request)?;
        Ok(plan::build(&root, c, &text, &engines))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

fn short_hash(s: &str) -> String {
    hex::encode(&Sha256::digest(s.as_bytes())[..4])
}

fn busy(state: &AppState, pid: &str) -> Option<&'static str> {
    state.devcontainer.rt.lock().get(pid).and_then(|r| r.op.as_ref().map(|o| o.kind))
}

fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Start (or rebuild) the project's dev container from `rel`, if `approve` is the
/// current plan's hash. Returns the terminal that shows the output.
pub async fn start(state: &AppState, p: &Arc<Project>, requested: Option<&str>, approve: Option<&str>, rebuild: bool) -> Result<Value, StartError> {
    let rel = config_rel(state, p, requested)?;
    let plan = plan_of(state, p, &rel).await?;
    // The approval first: the dialog shows the plan with its problems either way.
    if approve != Some(plan.hash.as_str()) {
        let changed = state.devcontainer.saved(state, &p.id).approvals.get(&rel).is_some_and(|h| *h != plan.hash);
        let message = if changed {
            format!("{rel} (or a file it uses) changed since you approved it; review what will run and approve again")
        } else {
            format!("starting the dev container of {} runs what {rel} defines; review and approve it", p.id)
        };
        return Err(StartError::Approval(Box::new(ApprovalRequired { plan, message })));
    }
    if !plan.problems.is_empty() {
        return Err(ApiError::new(axum::http::StatusCode::CONFLICT, "not_startable", plan.problems.join("; ")).into());
    }
    let engine_kind = plan.engine.ok_or_else(|| ApiError::conflict("no engine can start this config"))?;
    {
        let mut rt = state.devcontainer.rt.lock();
        let r = rt.entry(p.id.clone()).or_default();
        if let Some(op) = &r.op {
            return Err(ApiError::conflict(format!("the dev container is busy ({})", op.kind)).into());
        }
        r.op = Some(Operation { kind: if rebuild { "rebuild" } else { "start" }, terminal_id: None, started_at: crate::util::now_ms() });
        r.error = None;
    }
    announce(state, &p.id);
    match launch(state, p, &rel, &plan, engine_kind, rebuild).await {
        Ok(v) => Ok(v),
        Err(e) => {
            if let Some(r) = state.devcontainer.rt.lock().get_mut(&p.id) {
                r.op = None;
                r.error = Some(e.message.clone());
            }
            announce(state, &p.id);
            Err(e.into())
        }
    }
}

async fn launch(state: &AppState, p: &Arc<Project>, rel: &str, plan: &Plan, engine_kind: Engine, rebuild: bool) -> Result<Value, ApiError> {
    let pid = p.id.clone();
    state
        .devcontainer
        .update_saved(state, &pid, |s| {
            s.approvals.insert(rel.to_string(), plan.hash.clone());
            s.config = Some(rel.to_string());
        })
        .map_err(ApiError::from)?;
    super::refresh(state).await;
    let (root, rel_s) = (p.root.clone(), rel.to_string());
    let real = tokio::task::spawn_blocking(move || config::load(&root, &rel_s, LocalEnv::Resolve))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(ApiError::bad_request)?;
    let abs = p.root.join(rel);
    let abs_s = abs.display().to_string();
    // The container this config made before (Workbench's, VS Code's or the CLI's).
    let existing = state
        .devcontainer
        .rt
        .lock()
        .get(&pid)
        .and_then(|r| r.all.iter().filter(|c| c.label("devcontainer.config_file") == Some(abs_s.as_str())).max_by_key(|c| (c.running, c.created.clone())).cloned());
    let work = store::work_dir(&state.paths.data_dir, &pid)?;
    let h8 = short_hash(rel);
    let container_name = format!("wbdc-{pid}-{h8}");
    let image_tag = format!("wbdc-{pid}-{h8}:latest");
    // The same host port when it is free (or held by the container a rebuild replaces),
    // else any free one: previews and readiness follow the published port.
    let replaced: Vec<u16> = if rebuild { existing.iter().flat_map(|c| c.ports.iter().filter_map(|m| m.host_port)).collect() } else { vec![] };
    let publish: Vec<(u16, u16)> = plan::ports(&real)
        .into_iter()
        .map(|port| {
            let wanted = real.app_ports.iter().find(|a| a.port == port).and_then(|a| a.host_port).unwrap_or(port);
            (port, if replaced.contains(&wanted) || port_free(wanted) { wanted } else { 0 })
        })
        .collect();
    let uid = nix::unistd::getuid().as_raw();
    let gid = nix::unistd::getgid().as_raw();
    let input = engine::UpInput {
        project_id: &pid,
        root: &p.root,
        config_abs: &abs,
        cfg: &real,
        shown: &plan.config,
        hash: &plan.hash,
        engine: engine_kind,
        docker: &super::docker_path(state),
        cli: super::cli_command(state),
        existing: existing.as_ref(),
        rebuild,
        check_marker: existing.as_ref().is_some_and(|c| c.label("workbench.project").is_some()),
        work: &work,
        host_uid: uid,
        host_gid: gid,
        cols: 120,
        rows: 32,
        container_name,
        compose_project: engine::compose_project_name(&p.root),
        image_tag,
        publish,
    };
    let script = engine::script(&input).map_err(ApiError::from)?;
    let script_path = work.join("up.sh");
    crate::util::fs::write_atomic(&script_path, script.script.as_bytes(), 0o700)?;
    let mut env = script.env.clone();
    env.push(("DOCKER_CLI_HINTS".into(), Some("false".into())));
    let secrets: Vec<crate::secrets::Secret> = script.secrets.iter().map(|s| crate::secrets::Secret::from_value(s.clone())).collect();
    let title = format!("Dev container · {}", plan.config.name.clone().unwrap_or_else(|| p.name.clone()));
    let spec = SpawnSpec {
        kind: TerminalKind::Command,
        title,
        project_id: Some(pid.clone()),
        cwd: p.root.clone(),
        argv: crate::util::os::shell::script_argv(&script_path),
        env,
        cols: None,
        rows: None,
        meta: json!({ "devcontainer": true, "action": if rebuild { "rebuild" } else { "start" }, "config": rel }),
    };
    let info = state.terminals.spawn_redacted(state, spec, secrets).await?;
    let tid = info.id.clone();
    if let Some(r) = state.devcontainer.rt.lock().get_mut(&pid) {
        if let Some(op) = r.op.as_mut() {
            op.terminal_id = Some(tid.clone());
        }
    }
    announce(state, &pid);
    let st = state.clone();
    let rel = rel.to_string();
    let engine_name = match engine_kind {
        Engine::Docker => "docker",
        Engine::Cli => "cli",
    };
    let (result_file, temps) = (script.result_file.clone(), script.temp_files.clone());
    let folder = real.workspace_folder.clone();
    let remote_user = engine::hook_user(&real);
    tokio::spawn(async move {
        let exit = match st.terminals.exit_watch(&tid) {
            Some(mut rx) => loop {
                if let Some(e) = rx.borrow().clone() {
                    break Some(e);
                }
                if rx.changed().await.is_err() {
                    break rx.borrow().clone();
                }
            },
            None => None,
        };
        for f in &temps {
            let _ = std::fs::remove_file(f);
        }
        let ok = exit.as_ref().is_some_and(|e| e.code == Some(0));
        let cli = result_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| engine::parse_cli_result(&t));
        if let Some(f) = &result_file {
            let _ = std::fs::remove_file(f);
        }
        super::refresh(&st).await;
        let cid = st.devcontainer.rt.lock().get(&pid).and_then(|r| r.container.as_ref().filter(|c| c.running).map(|c| c.id.clone()));
        let ok = ok && cli.as_ref().is_none_or(|r| r.outcome == "success") && cid.is_some();
        if ok {
            let last = store::LastUp {
                config: rel.clone(),
                container_id: cli.as_ref().and_then(|r| r.container_id.clone()).or(cid.clone()).unwrap_or_default(),
                engine: engine_name.into(),
                remote_user: cli.as_ref().and_then(|r| r.remote_user.clone()).or(remote_user),
                workspace_folder: cli.as_ref().and_then(|r| r.remote_workspace_folder.clone()).or(Some(folder)),
                compose_project: cli.as_ref().and_then(|r| r.compose_project.clone()),
                at: crate::util::now_ms(),
            };
            let _ = st.devcontainer.update_saved(&st, &pid, |s| {
                s.attached = true;
                s.last = Some(last);
            });
        }
        {
            let mut rt = st.devcontainer.rt.lock();
            if let Some(r) = rt.get_mut(&pid) {
                r.op = None;
                r.probe = None;
                r.error = (!ok).then(|| match (&exit, &cli) {
                    (_, Some(c)) if c.outcome != "success" => format!("devcontainer up failed: {}", c.message.clone().unwrap_or_default()),
                    (Some(e), _) if e.code != Some(0) => format!("the start failed (exit {}); see the Dev container terminal", e.code.map(|c| c.to_string()).unwrap_or_else(|| e.signal.clone().unwrap_or_default())),
                    _ => "the container is not running after the start; see the Dev container terminal".to_string(),
                });
            }
        }
        super::sync_bridge(&st).await;
        announce(&st, &pid);
        if ok {
            st.events.notify("success", &format!("{pid}: dev container is running"));
        } else {
            st.events.notify("error", &format!("{pid}: dev container start failed"));
        }
    });
    Ok(json!({ "terminalId": info.id, "state": DcState::Building }))
}

/// Stop the container (the compose project when `shutdownAction` says so).
pub async fn stop(state: &AppState, p: &Project) -> Result<(), ApiError> {
    if let Some(k) = busy(state, &p.id) {
        return Err(ApiError::conflict(format!("the dev container is busy ({k})")));
    }
    super::refresh(state).await;
    let c = super::current(state, &p.id).ok_or_else(|| ApiError::not_found("no dev container"))?;
    let docker = super::docker_path(state);
    let compose = c.label("com.docker.compose.project").map(str::to_string);
    let stop_compose = compose.is_some()
        && super::selected_config(state, p)
            .and_then(|rel| config::load(&p.root, &rel, LocalEnv::Keep).ok())
            .is_none_or(|cfg| cfg.shutdown_action == "stopCompose");
    set_op(state, &p.id, Some("stop"));
    let out = match (&compose, stop_compose) {
        (Some(proj), true) => docker::run(&docker, &["compose", "-p", proj, "stop"], Duration::from_secs(120)).await,
        _ => docker::run(&docker, &["stop", "-t", "10", &c.id], Duration::from_secs(60)).await,
    };
    set_op(state, &p.id, None);
    let _ = state.devcontainer.update_saved(state, &p.id, |s| s.attached = false);
    super::refresh(state).await;
    announce(state, &p.id);
    match out {
        Ok(o) if o.ok() => Ok(()),
        Ok(o) => Err(ApiError::internal(format!("docker stop failed: {}", docker::first_line(&o.message())))),
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// Remove the container (compose: `down`, which keeps volumes). Images stay.
pub async fn remove(state: &AppState, p: &Project) -> Result<(), ApiError> {
    if let Some(k) = busy(state, &p.id) {
        return Err(ApiError::conflict(format!("the dev container is busy ({k})")));
    }
    super::refresh(state).await;
    let c = super::current(state, &p.id).ok_or_else(|| ApiError::not_found("no dev container"))?;
    let docker = super::docker_path(state);
    set_op(state, &p.id, Some("remove"));
    let out = match c.label("com.docker.compose.project") {
        Some(proj) => docker::run(&docker, &["compose", "-p", proj, "down"], Duration::from_secs(180)).await,
        None => docker::run(&docker, &["rm", "-f", &c.id], Duration::from_secs(60)).await,
    };
    set_op(state, &p.id, None);
    let _ = state.devcontainer.update_saved(state, &p.id, |s| {
        s.attached = false;
        s.last = None;
    });
    super::refresh(state).await;
    announce(state, &p.id);
    match out {
        Ok(o) if o.ok() => Ok(()),
        Ok(o) => Err(ApiError::internal(format!("docker failed: {}", docker::first_line(&o.message())))),
        Err(e) => Err(ApiError::internal(e)),
    }
}

fn set_op(state: &AppState, pid: &str, kind: Option<&'static str>) {
    let mut rt = state.devcontainer.rt.lock();
    let r = rt.entry(pid.to_string()).or_default();
    r.op = kind.map(|k| Operation { kind: k, terminal_id: None, started_at: crate::util::now_ms() });
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsBody {
    pub use_container: Option<bool>,
    pub config: Option<String>,
    /// `{name, host}`: pin one run configuration to the host (or back).
    pub host_run: Option<HostRun>,
}

#[derive(Debug, Deserialize)]
pub struct HostRun {
    pub name: String,
    pub host: bool,
}

pub async fn settings(state: &AppState, p: &Project, b: SettingsBody) -> Result<(), ApiError> {
    if let Some(c) = &b.config {
        if !config::is_config(&p.root, c) {
            return Err(ApiError::not_found(format!("{c} is not a devcontainer.json of {}", p.id)));
        }
    }
    state
        .devcontainer
        .update_saved(state, &p.id, |s| {
            if let Some(u) = b.use_container {
                s.use_container = Some(u);
            }
            if let Some(c) = b.config {
                s.config = Some(c);
            }
            if let Some(h) = b.host_run {
                if h.host {
                    s.host_runs.insert(h.name);
                } else {
                    s.host_runs.remove(&h.name);
                }
            }
        })
        .map_err(ApiError::from)?;
    super::refresh(state).await;
    announce(state, &p.id);
    Ok(())
}

/// Everything the panel shows. Read-only: agents see it through MCP too.
pub async fn view(state: &AppState, p: &Arc<Project>, requested: Option<&str>) -> Result<Value, ApiError> {
    // Docker state on demand (one `ps`, one `inspect`), so a container started or
    // removed outside Workbench shows at once.
    super::refresh(state).await;
    let configs = config::discover(&p.root);
    let rel = match requested {
        Some(_) => Some(config_rel(state, p, requested)?),
        None => super::selected_config(state, p),
    };
    let (plan, plan_error) = match &rel {
        Some(r) => match plan_of(state, p, r).await {
            Ok(pl) => (Some(pl), None),
            Err(e) => (None, Some(e.message)),
        },
        None => (None, None),
    };
    let engines = super::engines(state).await;
    let saved = state.devcontainer.saved(state, &p.id);
    let (st, container, op, error, others) = {
        let rt = state.devcontainer.rt.lock();
        let r = rt.get(&p.id);
        (
            r.map(super::state_of).unwrap_or(DcState::None),
            r.and_then(|r| r.container.clone()),
            r.and_then(|r| r.op.clone()),
            r.and_then(|r| r.error.clone()),
            r.map(|r| r.all.len()).unwrap_or(0),
        )
    };
    let approved = match (&plan, &rel) {
        (Some(pl), Some(r)) => saved.approvals.get(r).is_some_and(|h| *h == pl.hash),
        _ => false,
    };
    let use_container = super::use_container_of(&saved);
    let running = container.as_ref().is_some_and(|c| c.running);
    let mut ports = vec![];
    let mut agents = json!({});
    let mut remote_user = None;
    let mut folder = None;
    let mut bridge = None;
    if let Some(c) = container.as_ref().filter(|c| c.running) {
        let wanted: Vec<u16> = plan.as_ref().map(|pl| pl.ports.clone()).unwrap_or_default();
        let mut all: Vec<u16> = wanted.clone();
        all.extend(c.ports.iter().filter(|m| m.proto == "tcp").map(|m| m.port));
        all.sort();
        all.dedup();
        for port in all {
            if let Some(r) = super::port_route(state, &p.id, port) {
                let label = plan.as_ref().and_then(|pl| {
                    pl.config.forward_ports.iter().chain(pl.config.app_ports.iter()).find(|x| x.port == port).and_then(|x| x.label.clone())
                });
                ports.push(json!({
                    "port": port,
                    "label": label,
                    "url": format!("http://{}:{}/", r.url_host, r.url_port),
                    "via": r.via,
                    "hostPort": (r.via == "published").then_some(r.url_port),
                }));
            }
        }
        if let Ok(t) = super::running_target(state, &p.id).await {
            remote_user = t.user.clone();
            folder = Some(t.folder.clone());
            let pr = super::probe(state, &p.id, c, t.user.as_deref()).await;
            agents = json!(pr.commands);
        }
        bridge = c.gateway.as_deref().and_then(|g| g.parse().ok()).and_then(|g| state.devcontainer.bridge.url(g));
    }
    Ok(json!({
        "projectId": p.id,
        "configs": configs,
        "config": rel,
        "plan": plan,
        "planError": plan_error,
        "engines": engines,
        "state": st,
        "container": container.as_ref().map(|c| json!({
            "id": c.short_id(),
            "name": c.name,
            "status": c.status,
            "image": c.image,
            "ports": c.ports,
            "ip": c.ip,
            "created": c.created,
            // Made by Workbench: its own engine's label, or the CLI's container of its last start.
            "managed": c.label("workbench.project").is_some() || saved.last.as_ref().is_some_and(|l| l.container_id == c.id),
            "configFile": c.label("devcontainer.config_file").map(|f| super::rel_display(&p.root, f)),
            "compose": c.label("com.docker.compose.project"),
        })),
        "containers": others,
        "approved": approved,
        "useContainer": use_container,
        "inContainer": running && use_container,
        "operation": op,
        "error": error,
        "hostRuns": saved.host_runs,
        "remoteUser": remote_user,
        "workspaceFolder": folder,
        "ports": ports,
        "agents": agents,
        "bridge": bridge,
    }))
}

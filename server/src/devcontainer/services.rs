//! Services: this computer's Docker containers, compose projects and images for the
//! Services tool window (CLion's Services › Docker). REST under `/api/docker`:
//!
//! * `GET containers` → `{containers, error?}`: every container (`docker ps -a`) with
//!   its compose project and service, dev container folder and Workbench project.
//! * `GET containers/{id}` → the container inspected: ports, mounts, networks, env and
//!   labels, and the whole `inspect`; values of secret-looking names and passwords in
//!   URLs are masked (`••••`) in all of them.
//! * `POST containers/{id}/{start|stop|restart|pause|unpause|kill}`,
//!   `POST containers/{id}/remove {force?}`.
//! * `POST containers/{id}/logs|shell {projectId?}` → `TerminalInfo`: a terminal
//!   following `docker logs -f`, or a shell inside (`docker exec -it`).
//! * `POST compose/{project}/{start|stop|restart|down}`: `docker compose -p`, which
//!   needs no compose file (compose finds the containers by their labels).
//! * `GET images`, `GET images/{id}`, `POST images/remove {ref}`, `POST images/prune`
//!   (dangling images only).
//!
//! Docker is root on this computer, so only the user acts: in-process callers (agents
//! over MCP) get 403 on every route that changes something or opens a terminal, and
//! agent tokens are not valid under `/api/docker` at all. Nothing here runs by itself:
//! the lists are read when the tool window asks, and changes arrive as `docker.changed`
//! from the `docker events` watcher in `mod.rs`.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::docker::{self, PortMap};
use crate::app::AppState;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::terminals::{SpawnSpec, TerminalInfo, TerminalKind};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/docker/containers", get(containers))
        .route("/api/docker/containers/{id}", get(container))
        .route("/api/docker/containers/{id}/logs", post(logs))
        .route("/api/docker/containers/{id}/shell", post(shell))
        .route("/api/docker/containers/{id}/{action}", post(container_action))
        .route("/api/docker/compose/{project}/{action}", post(compose_action))
        .route("/api/docker/images", get(images))
        .route("/api/docker/images/prune", post(prune_images))
        .route("/api/docker/images/remove", post(remove_image))
        .route("/api/docker/images/{id}", get(image))
}

const MASK: &str = "••••";
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
/// `stop` waits for the container's grace period (10 s by default) per container.
const ACTION_TIMEOUT: Duration = Duration::from_secs(120);

/// Everything that changes Docker or opens a terminal is the user's.
fn user_only(caller: &Option<Extension<Caller>>) -> ApiResult<()> {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("Docker containers and images are managed by the user only")),
        _ => Ok(()),
    }
}

fn docker_failed(message: &str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "docker_failed", docker::first_line(message))
}

/// A container id or name (Docker names: `[a-zA-Z0-9][a-zA-Z0-9_.-]+`).
fn valid_container(id: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$").unwrap());
    RE.is_match(id)
}

/// An image id or reference (`registry:5000/ns/name:tag`, `name@sha256:…`, `sha256:…`).
fn valid_image(r: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/:@-]{0,254}$").unwrap());
    RE.is_match(r)
}

/// A compose project name (compose normalizes them to this).
fn valid_compose(name: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,127}$").unwrap());
    RE.is_match(name)
}

async fn run(state: &AppState, args: &[&str], timeout: Duration) -> Result<crate::util::proc::Output, ApiError> {
    docker::run(&super::docker_path(state), args, timeout)
        .await
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "docker_unavailable", e))
}

/// Tell the UI (and the dev container poller) at once; the events watcher would too,
/// a moment later, when it runs.
fn changed(state: &AppState, kind: &str) {
    state.events.emit("docker.changed", None, json!({ "kinds": [kind] }));
    state.devcontainer.refresh_wanted.notify_one();
}

// ---------------------------------------------------------------- masking

/// Names whose values are credentials (`POSTGRES_PASSWORD`, `GITHUB_TOKEN`,
/// `AWS_SECRET_ACCESS_KEY`, `STRIPE_KEY`…).
pub(crate) fn secret_name(name: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(pass(w|$|_|phrase)|pwd|secret|token|api_?key|access_?key|private_?key|credential|(^|_)auth($|_)|cookie|session_?key|(^|_)key$|dsn$|signature)").unwrap()
    });
    RE.is_match(name)
}

/// `scheme://user:password@host` → `scheme://user:••••@host`.
pub(crate) fn mask_url_passwords(s: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(://[^:/@\s]*):([^@/\s]+)@").unwrap());
    if !s.contains("://") {
        return s.to_string();
    }
    RE.replace_all(s, format!("${{1}}:{MASK}@").as_str()).into_owned()
}

fn mask_value(name: &str, value: &str) -> String {
    if secret_name(name) && !value.is_empty() { MASK.into() } else { mask_url_passwords(value) }
}

/// `K=V` (an `Env` entry).
fn mask_env_entry(e: &str) -> String {
    match e.split_once('=') {
        Some((k, v)) => format!("{k}={}", mask_value(k, v)),
        None => e.to_string(),
    }
}

/// A label or other named string: masked whole for a secret name; JSON inside
/// (`devcontainer.metadata` carries `containerEnv` / `remoteEnv`) masked member by
/// member; URL passwords masked.
fn mask_named(name: &str, value: &str) -> String {
    if secret_name(name) && !value.is_empty() {
        return MASK.into();
    }
    if value.starts_with(['{', '[']) {
        if let Ok(mut inner) = serde_json::from_str::<Value>(value) {
            mask_json(&mut inner);
            return inner.to_string();
        }
    }
    mask_url_passwords(value)
}

/// Mask a whole `docker inspect` document: `Env` arrays, members with secret names,
/// JSON labels, and URL passwords anywhere.
pub(crate) fn mask_json(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                match x {
                    Value::Array(a) if k == "Env" => {
                        for e in a.iter_mut() {
                            if let Value::String(s) = e {
                                *s = mask_env_entry(s);
                            }
                        }
                    }
                    Value::String(s) => *s = mask_named(k, s),
                    _ => mask_json(x),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(mask_json),
        Value::String(s) => *s = mask_url_passwords(s),
        _ => {}
    }
}

// ---------------------------------------------------------------- containers

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Compose {
    pub project: String,
    pub service: String,
    pub working_dir: String,
    pub config_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `created`, `paused`, `restarting`, `removing`, `dead`.
    pub state: String,
    /// Docker's words: `Up 2 hours (healthy)`, `Exited (0) 3 days ago`.
    pub status: String,
    pub created_at: String,
    pub ports: Vec<PortMap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose: Option<Compose>,
    /// The `devcontainer.local_folder` label: the folder a dev container was made for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devcontainer: Option<String>,
    /// The Workbench project the container belongs to (its compose folder or dev
    /// container folder is inside the project).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

const PS_FORMAT: &str = concat!(
    "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.CreatedAt}}\t{{.Ports}}",
    "\t{{.Label \"com.docker.compose.project\"}}\t{{.Label \"com.docker.compose.service\"}}",
    "\t{{.Label \"com.docker.compose.project.working_dir\"}}\t{{.Label \"com.docker.compose.project.config_files\"}}",
    "\t{{.Label \"com.docker.compose.container-number\"}}\t{{.Label \"devcontainer.local_folder\"}}",
);

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// `docker ps -a --format PS_FORMAT` lines.
pub(crate) fn parse_ps(out: &str) -> Vec<ServiceContainer> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 13 || f[0].trim().is_empty() {
                return None;
            }
            let compose = non_empty(f[7]).map(|project| Compose {
                project,
                service: f[8].trim().to_string(),
                working_dir: f[9].trim().to_string(),
                config_files: f[10].split(',').filter_map(non_empty).collect(),
                number: f[11].trim().parse().ok(),
            });
            Some(ServiceContainer {
                id: f[0].trim().to_string(),
                name: f[1].trim().to_string(),
                image: f[2].trim().to_string(),
                state: f[3].trim().to_string(),
                status: f[4].trim().to_string(),
                created_at: f[5].trim().to_string(),
                ports: parse_ports(f[6]),
                compose,
                devcontainer: non_empty(f[12]),
                project_id: None,
            })
        })
        .collect()
}

fn port_range(s: &str) -> Option<(u16, u16)> {
    match s.split_once('-') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => s.parse().ok().map(|p| (p, p)),
    }
}

/// `docker ps`'s Ports column: `0.0.0.0:8000->80/tcp, [::]:8000->80/tcp, 9000/tcp`,
/// ranges `127.0.0.1:8000-8001->8000-8001/tcp`. IPv4 and IPv6 bindings of one publish
/// count once; a range expands to at most 64 ports.
pub(crate) fn parse_ports(s: &str) -> Vec<PortMap> {
    let mut out: Vec<PortMap> = vec![];
    for part in s.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        let (host, inner) = match part.split_once("->") {
            Some((h, i)) => (Some(h), i),
            None => (None, part),
        };
        let (ports, proto) = inner.split_once('/').unwrap_or((inner, "tcp"));
        let Some((c0, c1)) = port_range(ports) else { continue };
        let (host_ip, host_ports) = match host.and_then(|h| h.rsplit_once(':')) {
            Some((ip, hp)) => {
                let ip = ip.trim_start_matches('[').trim_end_matches(']');
                (non_empty(if ip == "::" || ip.is_empty() { "" } else { ip }), port_range(hp))
            }
            None => (None, None),
        };
        for (i, port) in (c0..=c1).enumerate().take(64) {
            let host_port = host_ports.and_then(|(h0, h1)| h0.checked_add(i as u16).filter(|p| *p <= h1));
            // `[::]:8000` repeats `0.0.0.0:8000`.
            if out.iter().any(|m| m.port == port && m.proto == proto && m.host_port == host_port) {
                continue;
            }
            out.push(PortMap { port, proto: proto.to_string(), host_ip: host_ip.clone(), host_port });
        }
    }
    out
}

/// The project whose folder holds `dir` (the deepest one).
fn project_of_dir(state: &AppState, dir: &str) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    let p = PathBuf::from(dir);
    let p = crate::util::os::path::canonicalize(&p).unwrap_or(p);
    state.projects.find_by_path(&p).map(|p| p.id.clone())
}

fn attach_projects(state: &AppState, list: &mut [ServiceContainer]) {
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    for c in list.iter_mut() {
        let dir = c.devcontainer.clone().or_else(|| c.compose.as_ref().map(|x| x.working_dir.clone())).unwrap_or_default();
        c.project_id = cache.entry(dir.clone()).or_insert_with(|| project_of_dir(state, &dir)).clone();
    }
}

async fn list_containers(state: &AppState) -> Result<Vec<ServiceContainer>, ApiError> {
    let out = run(state, &["ps", "-a", "--no-trunc", "--format", PS_FORMAT], LIST_TIMEOUT).await?;
    if !out.ok() {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "docker_unavailable", docker::first_line(&out.message())));
    }
    let mut list = parse_ps(&out.stdout);
    attach_projects(state, &mut list);
    Ok(list)
}

/// Docker not answering is the tool window's empty state, not an error.
async fn containers(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    match list_containers(&state).await {
        Ok(list) => Ok(Json(json!({ "containers": list }))),
        Err(e) if e.code == "docker_unavailable" => Ok(Json(json!({ "containers": [], "error": e.message }))),
        Err(e) => Err(e),
    }
}

async fn inspect_one(state: &AppState, kind: &str, id: &str) -> ApiResult<Value> {
    let out = run(state, &["inspect", "--type", kind, id], LIST_TIMEOUT).await?;
    let v: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    match v.as_array().and_then(|a| a.first()) {
        Some(x) => Ok(x.clone()),
        None if out.ok() => Err(ApiError::not_found(format!("no {kind} {id}"))),
        None => Err(ApiError::not_found(docker::first_line(&out.message()))),
    }
}

fn str_at(v: &Value, p: &str) -> String {
    v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A command line from an argv (`Path` + `Args`, `Config.Cmd`).
fn join_argv(parts: impl IntoIterator<Item = String>) -> String {
    parts.into_iter().map(|a| super::sh_quote(&a)).collect::<Vec<_>>().join(" ")
}

fn strings_at(v: &Value, p: &str) -> Vec<String> {
    v.pointer(p).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Docker's zero time (`0001-01-01T00:00:00Z`) means never.
fn time_at(v: &Value, p: &str) -> Option<String> {
    let t = str_at(v, p);
    (!t.is_empty() && !t.starts_with("0001-")).then_some(t)
}

pub(crate) fn container_detail(state: Option<&AppState>, raw: Value) -> Option<Value> {
    let info = docker::parse_inspect(&raw)?;
    let mut masked = raw.clone();
    mask_json(&mut masked);
    let v = &raw;
    let env: Vec<(String, String)> = strings_at(v, "/Config/Env")
        .iter()
        .map(|e| match e.split_once('=') {
            Some((k, x)) => (k.to_string(), mask_value(k, x)),
            None => (e.clone(), String::new()),
        })
        .collect();
    let labels: Vec<(String, String)> = info.labels.iter().map(|(k, x)| (k.clone(), mask_named(k, x))).collect();
    let networks: Vec<Value> = v
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(name, n)| json!({ "name": name, "ip": str_at(n, "/IPAddress"), "gateway": str_at(n, "/Gateway"), "aliases": strings_at(n, "/Aliases") }))
                .collect()
        })
        .unwrap_or_default();
    let mounts: Vec<Value> = v
        .pointer("/Mounts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| {
                    json!({
                        "type": str_at(m, "/Type"),
                        "source": if str_at(m, "/Type") == "volume" { str_at(m, "/Name") } else { str_at(m, "/Source") },
                        "destination": str_at(m, "/Destination"),
                        "rw": m.get("RW").and_then(Value::as_bool).unwrap_or(true),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let command = join_argv(std::iter::once(str_at(v, "/Path")).filter(|p| !p.is_empty()).chain(strings_at(v, "/Args")));
    let compose = info.label("com.docker.compose.project").map(|project| Compose {
        project: project.to_string(),
        service: info.label("com.docker.compose.service").unwrap_or("").to_string(),
        working_dir: info.label("com.docker.compose.project.working_dir").unwrap_or("").to_string(),
        config_files: info.label("com.docker.compose.project.config_files").unwrap_or("").split(',').filter_map(non_empty).collect(),
        number: info.label("com.docker.compose.container-number").and_then(|n| n.parse().ok()),
    });
    let devcontainer = info.label("devcontainer.local_folder").map(str::to_string);
    let project_id = state.and_then(|s| {
        let dir = devcontainer.clone().or_else(|| compose.as_ref().map(|c| c.working_dir.clone()))?;
        project_of_dir(s, &dir)
    });
    Some(json!({
        "id": info.id,
        "name": info.name,
        "image": info.image,
        "imageId": str_at(v, "/Image"),
        "state": info.status,
        "running": info.running,
        "paused": v.pointer("/State/Paused").and_then(Value::as_bool).unwrap_or(false),
        "createdAt": info.created,
        "startedAt": time_at(v, "/State/StartedAt"),
        "finishedAt": time_at(v, "/State/FinishedAt"),
        "exitCode": v.pointer("/State/ExitCode").and_then(Value::as_i64),
        "error": non_empty(&str_at(v, "/State/Error")),
        "health": non_empty(&str_at(v, "/State/Health/Status")),
        "restartPolicy": non_empty(&str_at(v, "/HostConfig/RestartPolicy/Name")).filter(|p| p != "no"),
        "restartCount": v.pointer("/RestartCount").and_then(Value::as_i64).unwrap_or(0),
        "command": command,
        "workingDir": str_at(v, "/Config/WorkingDir"),
        "user": info.user,
        "hostname": str_at(v, "/Config/Hostname"),
        "networkMode": info.network_mode,
        "ports": info.ports,
        "networks": networks,
        "mounts": mounts,
        "env": env,
        "labels": labels,
        "compose": compose,
        "devcontainer": devcontainer,
        "projectId": project_id,
        "inspect": masked,
    }))
}

async fn container(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if !valid_container(&id) {
        return Err(ApiError::bad_request("not a container id or name"));
    }
    let raw = inspect_one(&state, "container", &id).await?;
    container_detail(Some(&state), raw).map(Json).ok_or_else(|| ApiError::not_found(format!("no container {id}")))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RemoveBody {
    force: bool,
}

async fn container_action(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path((id, action)): Path<(String, String)>,
    body: Option<Json<RemoveBody>>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    if !valid_container(&id) {
        return Err(ApiError::bad_request("not a container id or name"));
    }
    let force = body.is_some_and(|b| b.force);
    let args: Vec<&str> = match action.as_str() {
        "start" | "stop" | "restart" | "pause" | "unpause" | "kill" => vec![action.as_str(), &id],
        "remove" if force => vec!["rm", "--force", &id],
        "remove" => vec!["rm", &id],
        _ => return Err(ApiError::not_found(format!("no container action {action:?}"))),
    };
    let out = run(&state, &args, ACTION_TIMEOUT).await?;
    changed(&state, "container");
    if !out.ok() {
        return Err(docker_failed(&out.message()));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn compose_action(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path((project, action)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    if !valid_compose(&project) {
        return Err(ApiError::bad_request("not a compose project name"));
    }
    if !matches!(action.as_str(), "start" | "stop" | "restart" | "down") {
        return Err(ApiError::not_found(format!("no compose action {action:?}")));
    }
    // `docker::run` works in `/`, where compose finds no file of its own: it takes the
    // project's containers from their labels.
    let out = run(&state, &["compose", "--project-name", &project, &action], ACTION_TIMEOUT).await?;
    changed(&state, "container");
    if !out.ok() {
        return Err(docker_failed(&out.message()));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct TerminalBody {
    project_id: Option<String>,
}

/// The container's name and project, for a terminal's title and list.
async fn terminal_target(state: &AppState, id: &str) -> ApiResult<(String, String, Option<String>)> {
    if !valid_container(id) {
        return Err(ApiError::bad_request("not a container id or name"));
    }
    let raw = inspect_one(state, "container", id).await?;
    let d = container_detail(Some(state), raw).ok_or_else(|| ApiError::not_found(format!("no container {id}")))?;
    let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
    Ok((s("id"), s("name"), d["projectId"].as_str().map(str::to_string)))
}

async fn spawn_docker(
    state: &AppState,
    title: String,
    project: Option<String>,
    argv: Vec<String>,
    meta: Value,
) -> ApiResult<TerminalInfo> {
    let project = project.and_then(|p| state.projects.get(&p));
    let cwd = project.as_ref().map(|p| p.root.clone()).or_else(dirs::home_dir).unwrap_or_else(|| FsPath::new("/").to_path_buf());
    state
        .terminals
        .spawn(
            state,
            SpawnSpec {
                kind: TerminalKind::Command,
                title,
                project_id: project.map(|p| p.id.clone()),
                cwd,
                argv,
                env: vec![],
                cols: None,
                rows: None,
                meta,
            },
        )
        .await
}

/// A terminal following the container's log (`meta.action = "logs"`: restartable).
async fn logs(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(id): Path<String>,
    body: Option<Json<TerminalBody>>,
) -> ApiResult<Json<TerminalInfo>> {
    user_only(&caller)?;
    let (full, name, pid) = terminal_target(&state, &id).await?;
    let pid = pid.or_else(|| body.and_then(|b| b.0.project_id));
    let argv = vec![super::docker_path(&state), "logs".into(), "--follow".into(), "--tail".into(), "1000".into(), full.clone()];
    let meta = json!({ "action": "logs", "docker": { "container": full, "name": name } });
    Ok(Json(spawn_docker(&state, format!("{name} · log"), pid, argv, meta).await?))
}

/// An interactive shell inside the container: bash when it has one, else sh.
async fn shell(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(id): Path<String>,
    body: Option<Json<TerminalBody>>,
) -> ApiResult<Json<TerminalInfo>> {
    user_only(&caller)?;
    let (full, name, pid) = terminal_target(&state, &id).await?;
    let pid = pid.or_else(|| body.and_then(|b| b.0.project_id));
    let script = "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi";
    let argv = vec![super::docker_path(&state), "exec".into(), "--interactive".into(), "--tty".into(), full.clone(), "/bin/sh".into(), "-c".into(), script.into()];
    let meta = json!({ "action": "docker-shell", "docker": { "container": full, "name": name } });
    Ok(Json(spawn_docker(&state, format!("{name} · shell"), pid, argv, meta).await?))
}

// ---------------------------------------------------------------- images

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceImage {
    pub id: String,
    /// `<none>` for a dangling image.
    pub repository: String,
    pub tag: String,
    pub created_at: String,
    pub size: String,
    /// Containers (running or not) made from it.
    pub containers: Vec<String>,
}

const IMAGES_FORMAT: &str = "{{.ID}}\t{{.Repository}}\t{{.Tag}}\t{{.CreatedAt}}\t{{.Size}}";

pub(crate) fn parse_images(out: &str) -> Vec<ServiceImage> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 5 || f[0].trim().is_empty() {
                return None;
            }
            Some(ServiceImage {
                id: f[0].trim().to_string(),
                repository: f[1].trim().to_string(),
                tag: f[2].trim().to_string(),
                created_at: f[3].trim().to_string(),
                size: f[4].trim().to_string(),
                containers: vec![],
            })
        })
        .collect()
}

async fn images(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let out = match run(&state, &["images", "--no-trunc", "--format", IMAGES_FORMAT], LIST_TIMEOUT).await {
        Ok(o) if o.ok() => o,
        Ok(o) => return Ok(Json(json!({ "images": [], "error": docker::first_line(&o.message()) }))),
        Err(e) => return Ok(Json(json!({ "images": [], "error": e.message }))),
    };
    let mut list = parse_images(&out.stdout);
    // Which containers use each image: their image ids.
    let ids = run(&state, &["ps", "-a", "--no-trunc", "--quiet"], LIST_TIMEOUT).await.map(|o| o.stdout).unwrap_or_default();
    let ids: Vec<&str> = ids.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if !ids.is_empty() {
        let mut args = vec!["inspect", "--type", "container", "--format", "{{.Image}}\t{{.Name}}"];
        args.extend(ids.iter().copied());
        if let Ok(o) = run(&state, &args, LIST_TIMEOUT).await {
            for l in o.stdout.lines() {
                if let Some((img, name)) = l.split_once('\t') {
                    for i in list.iter_mut().filter(|i| i.id == img.trim()) {
                        i.containers.push(name.trim().trim_start_matches('/').to_string());
                    }
                }
            }
        }
    }
    Ok(Json(json!({ "images": list })))
}

async fn image(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if !valid_image(&id) {
        return Err(ApiError::bad_request("not an image id or reference"));
    }
    let raw = inspect_one(&state, "image", &id).await?;
    let v = &raw;
    let env: Vec<(String, String)> = strings_at(v, "/Config/Env")
        .iter()
        .map(|e| match e.split_once('=') {
            Some((k, x)) => (k.to_string(), mask_value(k, x)),
            None => (e.clone(), String::new()),
        })
        .collect();
    let mut labels: Vec<(String, String)> = v
        .pointer("/Config/Labels")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, x)| (k.clone(), mask_named(k, x.as_str().unwrap_or("")))).collect())
        .unwrap_or_default();
    labels.sort();
    let mut exposed: Vec<String> = v.pointer("/Config/ExposedPorts").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default();
    exposed.sort();
    let mut masked = raw.clone();
    mask_json(&mut masked);
    Ok(Json(json!({
        "id": str_at(v, "/Id"),
        "tags": strings_at(v, "/RepoTags"),
        "digests": strings_at(v, "/RepoDigests"),
        "createdAt": str_at(v, "/Created"),
        "size": v.pointer("/Size").and_then(Value::as_u64).unwrap_or(0),
        "architecture": str_at(v, "/Architecture"),
        "os": str_at(v, "/Os"),
        "entrypoint": join_argv(strings_at(v, "/Config/Entrypoint")),
        "cmd": join_argv(strings_at(v, "/Config/Cmd")),
        "workingDir": str_at(v, "/Config/WorkingDir"),
        "user": str_at(v, "/Config/User"),
        "exposedPorts": exposed,
        "layers": v.pointer("/RootFS/Layers").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "env": env,
        "labels": labels,
        "inspect": masked,
    })))
}

#[derive(Deserialize)]
struct ImageRef {
    #[serde(rename = "ref")]
    reference: String,
}

/// `docker rmi`: a tag is untagged (the image stays while other tags name it); an
/// image a container uses is refused by Docker.
async fn remove_image(State(state): State<AppState>, caller: Option<Extension<Caller>>, Json(b): Json<ImageRef>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    if !valid_image(&b.reference) {
        return Err(ApiError::bad_request("not an image id or reference"));
    }
    let out = run(&state, &["rmi", &b.reference], ACTION_TIMEOUT).await?;
    changed(&state, "image");
    if !out.ok() {
        return Err(docker_failed(&out.message()));
    }
    Ok(Json(json!({ "ok": true, "output": out.stdout.trim() })))
}

/// `docker image prune --force`: dangling images only (never `--all`).
async fn prune_images(State(state): State<AppState>, caller: Option<Extension<Caller>>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let out = run(&state, &["image", "prune", "--force"], ACTION_TIMEOUT).await?;
    changed(&state, "image");
    if !out.ok() {
        return Err(docker_failed(&out.message()));
    }
    let reclaimed = out.stdout.lines().find_map(|l| l.strip_prefix("Total reclaimed space:")).map(|s| s.trim().to_string());
    Ok(Json(json!({ "ok": true, "reclaimed": reclaimed })))
}

// ---------------------------------------------------------------- events

/// Which `docker events` change what the tool window shows: not the `exec_*` of
/// health checks and probes, attaches, resizes or copies.
pub(crate) fn event_matters(action: &str) -> bool {
    let a = action.split(':').next().unwrap_or("").trim();
    !(a.starts_with("exec_") || matches!(a, "attach" | "detach" | "resize" | "top" | "archive-path" | "extract-to-dir" | "export" | "commit" | "copy"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_column() {
        let p = parse_ports("0.0.0.0:8000->80/tcp, [::]:8000->80/tcp, 9000/tcp, 127.0.0.1:5000-5001->6000-6001/udp");
        assert_eq!(p.len(), 4, "{p:?}");
        assert_eq!((p[0].port, p[0].host_port, p[0].host_ip.as_deref()), (80, Some(8000), Some("0.0.0.0")));
        assert_eq!((p[1].port, p[1].host_port), (9000, None));
        assert_eq!((p[2].port, p[2].host_port, p[2].proto.as_str(), p[2].host_ip.as_deref()), (6000, Some(5000), "udp", Some("127.0.0.1")));
        assert_eq!((p[3].port, p[3].host_port), (6001, Some(5001)));
        // Older Docker writes IPv6 as `:::`; the IPv6 binding alone has no host address.
        let p = parse_ports(":::8080->80/tcp");
        assert_eq!((p[0].port, p[0].host_port, p[0].host_ip.clone()), (80, Some(8080), None));
        assert!(parse_ports("").is_empty());
        assert_eq!(parse_ports("0.0.0.0:1-65535->1-65535/tcp").len(), 64);
    }

    #[test]
    fn ps_lines_with_compose_and_devcontainer_labels() {
        let out = "abc\tweb-1\tnginx:1\trunning\tUp 2 hours (healthy)\t2026-09-27 10:00:00 +0100 IST\t127.0.0.1:8080->80/tcp\tshop\tweb\t/src/shop\t/src/shop/compose.yaml,/x/override.yml\t1\t\n\
                   def\tdc\timg\texited\tExited (0) 3 days ago\t2026-09-26 10:00:00 +0100 IST\t\t\t\t\t\t\t/src/app\n\
                   short line\n";
        let l = parse_ps(out);
        assert_eq!(l.len(), 2);
        let c = l[0].compose.as_ref().unwrap();
        assert_eq!((c.project.as_str(), c.service.as_str(), c.working_dir.as_str(), c.number), ("shop", "web", "/src/shop", Some(1)));
        assert_eq!(c.config_files, vec!["/src/shop/compose.yaml", "/x/override.yml"]);
        assert_eq!(l[0].status, "Up 2 hours (healthy)");
        assert_eq!(l[0].ports[0].host_port, Some(8080));
        assert!(l[1].compose.is_none());
        assert_eq!(l[1].devcontainer.as_deref(), Some("/src/app"));
    }

    #[test]
    fn secrets_are_masked_by_name_and_in_urls() {
        for n in ["POSTGRES_PASSWORD", "DB_PASS", "GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "STRIPE_KEY", "api_key", "SENTRY_DSN", "PGPASSWORD", "BASIC_AUTH"] {
            assert!(secret_name(n), "{n}");
        }
        for n in ["PATH", "HOME", "KEYCLOAK_URL", "AUTHOR", "SandboxKey", "PASSENGER", "LANG"] {
            assert!(!secret_name(n), "{n}");
        }
        assert_eq!(mask_url_passwords("postgres://app:hunter2@db:5432/app"), "postgres://app:••••@db:5432/app");
        assert_eq!(mask_url_passwords("redis://:hunter2@cache"), "redis://:••••@cache");
        assert_eq!(mask_url_passwords("https://example.com/a:b@c"), "https://example.com/a:b@c");
        let mut v = json!({
            "Config": {
                "Env": ["DB_PASSWORD=hunter2", "DATABASE_URL=postgres://app:hunter2@db/app", "PATH=/bin", "EMPTY_TOKEN="],
                "Labels": {
                    "devcontainer.metadata": "[{\"remoteEnv\":{\"GITHUB_TOKEN\":\"hunter2\",\"EDITOR\":\"vi\"}}]",
                    "com.example.api-token": "hunter2",
                },
            },
            "Args": ["--url", "amqp://guest:hunter2@mq"],
        });
        mask_json(&mut v);
        let s = v.to_string();
        assert!(!s.contains("hunter2"), "{s}");
        assert_eq!(v["Config"]["Env"][0], "DB_PASSWORD=••••");
        assert_eq!(v["Config"]["Env"][2], "PATH=/bin");
        assert_eq!(v["Config"]["Env"][3], "EMPTY_TOKEN=");
        assert!(s.contains("EDITOR") && s.contains("vi"), "{s}");
    }

    #[test]
    fn exec_events_do_not_matter() {
        assert!(!event_matters("exec_start: /bin/sh -c pg_isready -U app"));
        assert!(!event_matters("exec_die"));
        for a in ["start", "die", "destroy", "health_status: healthy", "pull", "delete", "untag", "rename"] {
            assert!(event_matters(a), "{a}");
        }
    }

    #[test]
    fn ids_names_and_references() {
        for ok in ["0123abcd", "web-1", "shop_web.1"] {
            assert!(valid_container(ok), "{ok}");
        }
        for bad in ["-rf", "--help", "a b", "a/b", "", "../x"] {
            assert!(!valid_container(bad), "{bad}");
        }
        for ok in ["nginx:1", "registry.gitlab.com/g/p:tag", "sha256:0123", "name@sha256:0123"] {
            assert!(valid_image(ok), "{ok}");
        }
        assert!(!valid_image("-f") && !valid_image("a b"));
        assert!(valid_compose("shop_devcontainer") && !valid_compose("My Proj") && !valid_compose("-p"));
    }

    /// A docker that records its calls and answers from canned files.
    fn fake_docker(dir: &FsPath, repo: &FsPath) -> PathBuf {
        let work = repo.join("deploy").display().to_string();
        std::fs::create_dir_all(repo.join("deploy")).unwrap();
        let ps = format!(
            "{id}\tshop-web-1\tnginx:1\trunning\tUp 2 hours\t2026-09-27 10:00:00 +0100 IST\t127.0.0.1:8080->80/tcp\tshop\tweb\t{work}\t{work}/compose.yaml\t1\t\n\
             {other}\tstray\tbusybox\texited\tExited (0) 3 days ago\t2026-09-26 10:00:00 +0100 IST\t\t\t\t\t\t\t\n",
            id = "c".repeat(64),
            other = "d".repeat(64),
        );
        std::fs::write(dir.join("ps.txt"), ps).unwrap();
        let inspect = json!([{
            "Id": "c".repeat(64),
            "Name": "/shop-web-1",
            "Created": "2026-09-27T09:00:00Z",
            "Path": "docker-entrypoint.sh",
            "Args": ["nginx", "-g", "daemon off;"],
            "Image": "sha256:img1",
            "State": {"Status": "running", "Running": true, "Paused": false, "StartedAt": "2026-09-27T09:00:01Z", "FinishedAt": "0001-01-01T00:00:00Z", "ExitCode": 0},
            "Config": {
                "Image": "nginx:1", "User": "", "Hostname": "web", "WorkingDir": "/srv",
                "Env": ["DB_PASSWORD=hunter2", "DATABASE_URL=postgres://app:hunter2@db/app", "LANG=C.UTF-8"],
                "Labels": {"com.docker.compose.project": "shop", "com.docker.compose.service": "web", "com.docker.compose.project.working_dir": work},
            },
            "HostConfig": {"NetworkMode": "shop_default", "RestartPolicy": {"Name": "unless-stopped"}},
            "NetworkSettings": {"Ports": {"80/tcp": [{"HostIp": "127.0.0.1", "HostPort": "8080"}]}, "Networks": {"shop_default": {"IPAddress": "172.20.0.2", "Gateway": "172.20.0.1", "Aliases": ["web"]}}},
            "Mounts": [{"Type": "volume", "Name": "shop_data", "Source": "/var/lib/docker/volumes/shop_data/_data", "Destination": "/data", "RW": true}],
        }]);
        std::fs::write(dir.join("container.json"), inspect.to_string()).unwrap();
        std::fs::write(dir.join("images.txt"), "sha256:img1\tnginx\t1\t2026-09-20 10:00:00 +0100 IST\t190MB\nsha256:img2\t<none>\t<none>\t2026-09-19 10:00:00 +0100 IST\t12MB\n").unwrap();
        install_fake_docker(dir)
    }

    /// `testdata/fake_docker.py` in `dir`, as `[devcontainer] docker` names it
    /// (`util::os::exe::test_cli`). Windows: an npm-style shim, `docker.cmd` (what the lists
    /// and actions run, through cmd.exe) with the `docker.ps1` Workbench reads to start
    /// Python on `docker.py` directly (the log and shell terminals).
    fn install_fake_docker(dir: &FsPath) -> PathBuf {
        crate::util::os::exe::test_cli(dir, "docker", include_str!("testdata/fake_docker.py"))
    }

    /// The list and details (masked), the actions' exact argv, terminals, and only the
    /// user acting: agents (in-process MCP calls) are refused every write.
    #[tokio::test]
    async fn containers_images_actions_and_only_the_user_acts() {
        use crate::mcp::{McpCtx, call_api};
        use axum::http::Method;
        use tower::ServiceExt;

        let bin_dir = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let docker = fake_docker(bin_dir.path(), repo.path());
        let mut cfg = crate::config::GlobalConfig::default();
        cfg.projects.roots.clear();
        cfg.projects.include = vec![repo.path().display().to_string()];
        cfg.notify.desktop = false;
        cfg.devcontainer.docker = docker.display().to_string();
        let t = crate::platform::testutil::app_with(cfg).await;
        let pid = t.state.projects.list()[0].id.clone();
        let token = t.state.auth.master_token().to_string();
        let send = |method: &str, path: String, body: Value| {
            let router = t.router.clone();
            let token = token.clone();
            let method = method.to_string();
            async move {
                let req = axum::http::Request::builder()
                    .method(method.as_str())
                    .uri(path)
                    .header("host", "127.0.0.1:7999")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap();
                let resp = router.oneshot(req).await.unwrap();
                let status = resp.status().as_u16();
                let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap_or_default())
            }
        };
        let calls = || std::fs::read_to_string(bin_dir.path().join("calls")).unwrap_or_default();
        let id = "c".repeat(64);

        let (s, v) = send("GET", "/api/docker/containers".into(), Value::Null).await;
        assert_eq!(s, 200, "{v}");
        let list = v["containers"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["projectId"], pid.as_str(), "the compose folder is inside the project");
        assert_eq!(list[0]["compose"]["service"], "web");
        assert!(list[1].get("projectId").is_none());

        let (s, v) = send("GET", format!("/api/docker/containers/{id}"), Value::Null).await;
        assert_eq!(s, 200, "{v}");
        assert!(!v.to_string().contains("hunter2"), "{v}");
        assert_eq!(v["env"][0], json!(["DB_PASSWORD", "••••"]));
        assert_eq!(v["env"][1], json!(["DATABASE_URL", "postgres://app:••••@db/app"]));
        assert_eq!(v["command"], "docker-entrypoint.sh nginx -g 'daemon off;'");
        assert_eq!(v["finishedAt"], Value::Null);
        assert_eq!(v["restartPolicy"], "unless-stopped");
        assert_eq!(v["mounts"][0]["source"], "shop_data");
        assert_eq!(v["networks"][0]["ip"], "172.20.0.2");
        assert_eq!(v["projectId"], pid.as_str());

        let (s, _) = send("POST", format!("/api/docker/containers/{id}/stop"), json!({})).await;
        assert_eq!(s, 200);
        assert!(calls().contains(&format!("stop {id}")), "{}", calls());
        let (s, v) = send("POST", format!("/api/docker/containers/{id}/remove"), json!({})).await;
        assert_eq!((s, v["error"]["code"].as_str()), (409, Some("docker_failed")));
        assert!(v["error"]["message"].as_str().unwrap().contains("container is running"));
        let (s, _) = send("POST", format!("/api/docker/containers/{id}/remove"), json!({ "force": true })).await;
        assert_eq!(s, 200);
        assert!(calls().contains(&format!("rm --force {id}")));
        let (s, _) = send("POST", "/api/docker/containers/-rf/stop".into(), json!({})).await;
        assert_eq!(s, 400);
        let (s, _) = send("POST", format!("/api/docker/containers/{id}/exec"), json!({})).await;
        assert_eq!(s, 404);
        let (s, _) = send("POST", "/api/docker/compose/shop/down".into(), json!({})).await;
        assert_eq!(s, 200);
        assert!(calls().contains("compose --project-name shop down"));
        let (s, _) = send("POST", "/api/docker/compose/Shop%20X/down".into(), json!({})).await;
        assert_eq!(s, 400);

        // Terminals: the log follows (restartable), the shell goes inside; both belong
        // to the container's project.
        let (s, v) = send("POST", format!("/api/docker/containers/{id}/logs"), json!({})).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["meta"]["action"], "logs");
        assert_eq!(v["projectId"], pid.as_str());
        assert_eq!(v["title"], "shop-web-1 · log");
        let argv: Vec<String> = serde_json::from_value(v["argv"].clone()).unwrap();
        assert_eq!(argv[1..], ["logs", "--follow", "--tail", "1000", id.as_str()]);
        let (s, v) = send("POST", format!("/api/docker/containers/{id}/shell"), json!({})).await;
        assert_eq!(s, 200, "{v}");
        let argv: Vec<String> = serde_json::from_value(v["argv"].clone()).unwrap();
        assert_eq!(argv[1..6], ["exec", "--interactive", "--tty", id.as_str(), "/bin/sh"]);

        let (s, v) = send("GET", "/api/docker/images".into(), Value::Null).await;
        assert_eq!(s, 200);
        assert_eq!(v["images"][0]["containers"], json!(["shop-web-1"]));
        assert_eq!(v["images"][1]["repository"], "<none>");
        let (s, v) = send("POST", "/api/docker/images/prune".into(), json!({})).await;
        assert_eq!((s, v["reclaimed"].as_str()), (200, Some("12MB")));
        assert!(calls().contains("image prune --force"));
        let (s, _) = send("POST", "/api/docker/images/remove".into(), json!({ "ref": "nginx:1" })).await;
        assert_eq!(s, 200);
        assert!(calls().contains("rmi nginx:1"));
        let (s, _) = send("GET", "/api/docker/images/sha256:nope".into(), Value::Null).await;
        assert_eq!(s, 404);

        // An agent reads, and nothing else. The log and shell terminals have run docker by
        // then (a slow start must not look like a call of the agent's).
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !(calls().contains("logs --follow") && calls().contains("exec --interactive")) {
            assert!(std::time::Instant::now() < deadline, "the terminals never ran docker: {}", calls());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let before = calls();
        let agent = McpCtx { terminal_id: Some("t1".into()), project_id: Some(pid.clone()) };
        for (path, body) in [
            (format!("/api/docker/containers/{id}/start"), json!({})),
            (format!("/api/docker/containers/{id}/remove"), json!({ "force": true })),
            (format!("/api/docker/containers/{id}/logs"), json!({})),
            (format!("/api/docker/containers/{id}/shell"), json!({})),
            ("/api/docker/compose/shop/down".to_string(), json!({})),
            ("/api/docker/images/remove".to_string(), json!({ "ref": "nginx:1" })),
            ("/api/docker/images/prune".to_string(), json!({})),
        ] {
            let e = call_api(&t.state, Method::POST, &path, Some(body), &agent).await.unwrap_err();
            assert_eq!(e.status.as_u16(), 403, "{path}: {e}");
        }
        assert_eq!(calls(), before, "an agent's request ran docker");
        assert!(call_api(&t.state, Method::GET, "/api/docker/containers", None, &agent).await.is_ok());
    }

    #[tokio::test]
    async fn docker_missing_is_an_empty_list_with_the_reason() {
        let mut cfg = crate::config::GlobalConfig::default();
        cfg.projects.roots.clear();
        cfg.notify.desktop = false;
        cfg.devcontainer.docker = "/nonexistent/docker".into();
        let t = crate::platform::testutil::app_with(cfg).await;
        let v = crate::mcp::call_api(&t.state, axum::http::Method::GET, "/api/docker/containers", None, &crate::mcp::McpCtx::default()).await.unwrap();
        assert_eq!(v["containers"], json!([]));
        assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{v}");
        let v = crate::mcp::call_api(&t.state, axum::http::Method::GET, "/api/docker/images", None, &crate::mcp::McpCtx::default()).await.unwrap();
        assert!(v["error"].as_str().is_some(), "{v}");
    }
}

//! REST routes under `/api/projects/{pid}/lsp`.
//!
//! * `GET  …/lsp`: status (enabled, where servers run, every server with its state,
//!   availability and install hint, diagnostic counts);
//! * `POST …/lsp/enable {mode?}`, `POST …/lsp/disable`, `PUT …/lsp/settings {mode?, disabledServers?}`;
//! * `POST …/lsp/servers/{sid}/restart`, `POST …/lsp/servers/{sid}/stop`,
//!   `GET …/lsp/servers/{sid}/log?tail=`;
//! * `GET  …/lsp/diagnostics`: every diagnostic of the project's files;
//! * `GET  …/lsp/source?uri=lsp-src://…`: a file outside the project a server pointed to;
//! * `GET  …/lsp/ws`: the editor's socket (`ws.rs`).
//!
//! Writes are the user's: in-process callers (MCP tools) are refused.

use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::config::ServerSpec;
use super::launch;
use super::manager::{Counts, SlotView};
use super::trust::Mode;
use super::uri::{self, ClientUri, Origin};
use crate::app::AppState;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::projects::Project;

/// Largest outside file served to the source panel.
const MAX_SOURCE: u64 = 5 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{pid}/lsp", get(status))
        .route("/api/projects/{pid}/lsp/enable", post(enable))
        .route("/api/projects/{pid}/lsp/disable", post(disable))
        .route("/api/projects/{pid}/lsp/settings", put(settings))
        .route("/api/projects/{pid}/lsp/servers/{sid}/restart", post(restart))
        .route("/api/projects/{pid}/lsp/servers/{sid}/stop", post(stop))
        .route("/api/projects/{pid}/lsp/servers/{sid}/log", get(log))
        .route("/api/projects/{pid}/lsp/diagnostics", get(diagnostics))
        .route("/api/projects/{pid}/lsp/source", get(source))
        .route("/api/projects/{pid}/lsp/ws", get(super::ws::ws))
}

fn user_only(caller: &Option<Extension<Caller>>) -> ApiResult<()> {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("code intelligence is turned on, off and controlled by the user only")),
        _ => Ok(()),
    }
}

fn require_enabled(state: &AppState, p: &Project) -> ApiResult<()> {
    if super::manager::enabled(state, p) {
        Ok(())
    } else {
        Err(ApiError::not_configured("code intelligence is not enabled for this project"))
    }
}

fn valid_sid(sid: &str) -> ApiResult<()> {
    if super::config::valid_id(sid) { Ok(()) } else { Err(ApiError::bad_request("bad server id")) }
}

// ---------------------------------------------------------------- status

/// `GET /api/projects/{pid}/lsp`.
pub async fn status(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    Ok(Json(status_of(&state, &p).await))
}

pub async fn status_of(state: &AppState, p: &Project) -> Value {
    let rec = state.lsp.trust.get(state, &p.id);
    let enabled = super::manager::enabled(state, p);
    let (specs, warnings) = state.config.read().lsp.specs();
    let lsp = state.lsp.get(&p.id);
    let views = lsp.as_ref().map(|l| l.slot_views()).unwrap_or_default();
    let markers = match &lsp {
        Some(l) => l.markers(&specs).await,
        None => {
            let root = p.root.clone();
            let list: Vec<(String, Vec<String>)> = specs.iter().map(|s| (s.id.clone(), s.root_markers.clone())).collect();
            tokio::task::spawn_blocking(move || list.into_iter().filter(|(_, m)| launch::has_root_marker(&root, m)).map(|(id, _)| id).collect())
                .await
                .unwrap_or_default()
        }
    };
    let target = launch::container_for(state, &p.id, rec.mode).await;
    let placements = futures::future::join_all(specs.iter().map(|s| async {
        match launch::place_in(state, p, s, rec.mode, &target).await {
            Ok(pl) => (Some(pl.side()), None),
            Err(e) => (None, Some(e)),
        }
    }))
    .await;
    let servers: Vec<Value> = specs
        .iter()
        .zip(placements)
        .map(|(s, (side, missing))| server_json(s, &rec.disabled_servers, side, missing, views.get(&s.id), markers.contains(&s.id)))
        .collect();
    let dc = crate::devcontainer::summary(state, p).map(|d| json!({ "state": d.state, "inContainer": d.in_container }));
    json!({
        "projectId": p.id,
        "enabled": enabled,
        "mode": rec.mode,
        "enabledAt": if enabled { json!(rec.enabled_at) } else { Value::Null },
        "devcontainer": dc,
        "servers": servers,
        "counts": lsp.map(|l| l.counts()).unwrap_or_default(),
        "warnings": warnings,
    })
}

fn server_json(s: &ServerSpec, disabled_here: &[String], side: Option<&str>, missing: Option<String>, view: Option<&(SlotView, usize)>, marker: bool) -> Value {
    let off_here = disabled_here.contains(&s.id);
    let (state, progress, error, docs) = match view {
        Some((v, docs)) if v.state != "off" => (v.state, v.progress.clone(), v.error.clone(), *docs),
        Some((v, docs)) => (if !s.enabled || off_here { "disabled" } else if missing.is_some() { "unavailable" } else { "off" }, None, v.error.clone(), *docs),
        None => (if !s.enabled || off_here { "disabled" } else if missing.is_some() { "unavailable" } else { "off" }, None, None, 0),
    };
    let v = view.map(|(v, _)| v);
    json!({
        "id": s.id,
        "label": s.label,
        "languages": s.languages,
        "extensions": s.extensions,
        "command": std::iter::once(display_command(&s.command)).chain(s.args.iter().cloned()).collect::<Vec<_>>().join(" "),
        "preset": s.preset,
        "enabled": s.enabled,
        "disabledHere": off_here,
        "available": missing.is_none(),
        "missing": missing,
        "installHint": s.install_hint,
        "side": v.and_then(|v| v.side).or(side),
        "state": state,
        "progress": progress,
        "error": error,
        "restarts": v.map(|v| v.restarts).unwrap_or(0),
        "pid": v.and_then(|v| v.os_pid),
        "startedAt": v.and_then(|v| v.started_at),
        "running": v.and_then(|v| v.command.clone()),
        "serverInfo": v.and_then(|v| v.server_info.clone()),
        "openDocs": docs,
        "relevant": marker || docs > 0 || matches!(state, "starting" | "indexing" | "ready" | "crashed" | "failed"),
    })
}

/// A command as shown: `~/…` for paths under the home directory.
fn display_command(c: &str) -> String {
    if crate::util::os::exe::names_path(c) { crate::config::contract_tilde(&crate::config::expand_tilde(c)) } else { c.to_string() }
}

// ---------------------------------------------------------------- enable / settings

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct EnableBody {
    mode: Option<Mode>,
}

async fn enable(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path(pid): Path<String>, body: Option<Json<EnableBody>>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let mode = body.and_then(|b| b.0.mode);
    if mode == Some(Mode::Container) {
        crate::devcontainer::require_supported()?;
    }
    let root = p.root.display().to_string();
    let st = state.clone();
    let id = p.id.clone();
    tokio::task::spawn_blocking(move || {
        st.lsp.trust.update(&st, &id, |r| {
            r.enabled = true;
            r.root = root;
            r.enabled_at = crate::util::now_ms();
            if let Some(m) = mode {
                r.mode = m;
            }
        })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    state.events.emit("lsp.state", Some(&p.id), json!({ "enabled": true }));
    Ok(Json(status_of(&state, &p).await))
}

async fn disable(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let st = state.clone();
    let id = p.id.clone();
    tokio::task::spawn_blocking(move || st.lsp.trust.update(&st, &id, |r| r.enabled = false))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??;
    if let Some(lsp) = state.lsp.remove(&p.id) {
        lsp.disconnect_all(true);
        lsp.shutdown(&state).await;
    }
    state.events.emit("lsp.state", Some(&p.id), json!({ "enabled": false }));
    state.events.emit("lsp.diagnostics", Some(&p.id), Counts::default());
    Ok(Json(status_of(&state, &p).await))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct SettingsBody {
    mode: Option<Mode>,
    disabled_servers: Option<Vec<String>>,
}

async fn settings(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path(pid): Path<String>, Json(body): Json<SettingsBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    if body.mode == Some(Mode::Container) {
        crate::devcontainer::require_supported()?;
    }
    if let Some(list) = &body.disabled_servers {
        if list.len() > 100 || list.iter().any(|s| !super::config::valid_id(s)) {
            return Err(ApiError::bad_request("bad server ids"));
        }
    }
    let before = state.lsp.trust.get(&state, &p.id);
    let st = state.clone();
    let id = p.id.clone();
    let (mode, disabled) = (body.mode, body.disabled_servers.clone());
    let after = tokio::task::spawn_blocking(move || {
        st.lsp.trust.update(&st, &id, |r| {
            if let Some(m) = mode {
                r.mode = m;
            }
            if let Some(d) = disabled {
                r.disabled_servers = d;
            }
        })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    // Servers move (host ↔ container) or change: stop them, and let the browsers open
    // their documents again so each picks its server anew.
    if before.mode != after.mode || before.disabled_servers != after.disabled_servers {
        if let Some(lsp) = state.lsp.get(&p.id) {
            lsp.disconnect_all(false);
            lsp.shutdown(&state).await;
        }
    }
    state.events.emit("lsp.state", Some(&p.id), json!({ "settings": true }));
    Ok(Json(status_of(&state, &p).await))
}

// ---------------------------------------------------------------- servers

async fn restart(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    valid_sid(&sid)?;
    let p = state.projects.require(&pid)?;
    require_enabled(&state, &p)?;
    let (specs, _) = state.config.read().lsp.specs();
    if !specs.iter().any(|s| s.id == sid) {
        return Err(ApiError::not_found(format!("no language server {sid:?}")));
    }
    state.lsp.avail.clear();
    let lsp = state.lsp.project(&p);
    lsp.restart_server(&state, &sid).await;
    Ok(Json(status_of(&state, &p).await))
}

async fn stop(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    valid_sid(&sid)?;
    let p = state.projects.require(&pid)?;
    if let Some(lsp) = state.lsp.get(&p.id) {
        lsp.stop_server(&state, &sid, true).await;
    }
    Ok(Json(status_of(&state, &p).await))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LogQuery {
    tail: Option<usize>,
}

async fn log(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>, Query(q): Query<LogQuery>) -> ApiResult<Json<Value>> {
    valid_sid(&sid)?;
    let p = state.projects.require(&pid)?;
    let tail = q.tail.unwrap_or(1000).min(3000);
    let (lines, running) = state.lsp.get(&p.id).and_then(|l| l.log_of(&sid, tail)).unwrap_or_default();
    Ok(Json(json!({ "server": sid, "running": running, "lines": lines })))
}

// ---------------------------------------------------------------- diagnostics

/// Diagnostics served at most (the Problems window).
const MAX_LISTED: usize = 5000;

async fn diagnostics(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    let Some(lsp) = state.lsp.get(&p.id) else {
        return Ok(Json(json!({ "files": [], "counts": Counts::default(), "truncated": false })));
    };
    Ok(Json(diagnostics_json(&lsp.diagnostics(), lsp.counts())))
}

pub fn diagnostics_json(all: &[(String, String, Vec<Value>)], counts: Counts) -> Value {
    let mut by_file: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    for (uri, server, list) in all {
        if !uri.starts_with("file:///") {
            continue;
        }
        let e = by_file.entry(uri.clone()).or_default();
        for d in list {
            let mut d = d.clone();
            d["server"] = Value::String(server.clone());
            e.push(d);
        }
    }
    let mut listed = 0;
    let mut truncated = false;
    let mut files = vec![];
    for (u, mut list) in by_file {
        let path = match uri::parse_client_uri(&u) {
            Some(ClientUri::Project { rel, .. }) => rel,
            _ => continue,
        };
        list.sort_by_key(|d| (d["severity"].as_u64().unwrap_or(1), d["range"]["start"]["line"].as_u64().unwrap_or(0)));
        if listed + list.len() > MAX_LISTED {
            list.truncate(MAX_LISTED.saturating_sub(listed));
            truncated = true;
        }
        listed += list.len();
        if !list.is_empty() {
            files.push(json!({ "uri": u, "path": path, "diagnostics": list }));
        }
        if truncated {
            break;
        }
    }
    json!({ "files": files, "counts": counts, "truncated": truncated })
}

// ---------------------------------------------------------------- source

#[derive(Deserialize)]
struct SourceQuery {
    uri: String,
}

async fn source(State(state): State<AppState>, Path(pid): Path<String>, Query(q): Query<SourceQuery>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    let Some(ClientUri::Source { pid: upid, path }) = uri::parse_client_uri(&q.uri) else {
        return Err(ApiError::bad_request("expected an lsp-src:// URI"));
    };
    if upid != p.id {
        return Err(ApiError::bad_request("the URI belongs to another project"));
    }
    let origin = state
        .lsp
        .get(&p.id)
        .and_then(|l| l.allow.lock().get(&path).cloned())
        .ok_or_else(|| ApiError::forbidden("only files a language server of this project pointed to can be shown; navigate to it again"))?;
    let bytes = match origin {
        Origin::Host => {
            let pb = std::path::PathBuf::from(&path);
            tokio::task::spawn_blocking(move || -> ApiResult<Vec<u8>> {
                let md = std::fs::metadata(&pb)?;
                if !md.is_file() {
                    return Err(ApiError::bad_request("not a file"));
                }
                if md.len() > MAX_SOURCE {
                    return Err(ApiError::bad_request(format!("the file is larger than {} MB", MAX_SOURCE / 1024 / 1024)));
                }
                Ok(std::fs::read(&pb)?)
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??
        }
        Origin::Container { docker, container_id, user } => {
            let mut cmd = tokio::process::Command::new(&docker);
            cmd.arg("exec");
            if let Some(u) = &user {
                cmd.args(["-u", u]);
            }
            let limit = (MAX_SOURCE + 1).to_string();
            cmd.args([container_id.as_str(), "/bin/sh", "-c", "head -c \"$1\" -- \"$2\"", "sh", &limit, &path]);
            let out = crate::util::proc::run_cmd(cmd, Duration::from_secs(20)).await?;
            if !out.ok() {
                return Err(ApiError::not_found(format!("cannot read {path} in the dev container")));
            }
            if out.stdout.len() as u64 > MAX_SOURCE {
                return Err(ApiError::bad_request(format!("the file is larger than {} MB", MAX_SOURCE / 1024 / 1024)));
            }
            out.stdout.into_bytes()
        }
    };
    let content = String::from_utf8(bytes).map_err(|_| ApiError::bad_request("not a UTF-8 text file"))?;
    let name = crate::util::os::path::segments(&path).last().unwrap_or(&path).to_string();
    Ok(Json(json!({ "uri": q.uri, "path": path, "name": name, "content": content })))
}

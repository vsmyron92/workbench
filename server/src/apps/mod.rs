//! Apps slice (OWNER: apps slice).
//!
//! Run configurations (start/stop, readiness, dependencies), environments
//! (production/staging health, version, preview, logs, deploy), and project
//! auto-detection. Routes: `/api/projects/{pid}/runs/**`, `/api/projects/{pid}/envs/**`.
//!
//! CONTRACT: `detect` (called by the project registry on every reload), `shutdown`.
//!
//! Modules: `detect` (zero-config proposal), `runs` (run manager), `envs` (health
//! pollers, versions, logs, commands), `deploy` (gates + deploy terminal), `proxy`
//! (loopback preview proxy), `expand` (placeholders, `${secret:…}`), `output`
//! (line buffering, ready matcher, result parser), `health` (pure checks),
//! `remote` (ssh argv), `mcp_tools`.
//!
//! REST (all under `/api/projects/{pid}`):
//! * `GET runs` → `RunView[]`; `POST runs/{name}/start|restart {freePort?, confirmed?}`; `POST runs/{name}/stop`;
//!   `POST runs/stop-all`. A busy port answers `409 {code: "port_in_use"}` (unless the run has
//!   `free_port = true`); a run that deploys, releases or reaches a remote host (`needsConfirm`,
//!   also as a dependency) answers `428 {code: "confirmation_required"}` without `confirmed: true`.
//! * `GET envs` → `EnvView[]`; `POST envs/check`; `POST envs/{name}/check|version`;
//!   `POST envs/{name}/logs {name?}` / `command {name, confirmed}` → `TerminalInfo`;
//!   `POST envs/{name}/deploy/check {sha?}` → `DeployPlan`; `POST envs/{name}/deploy {sha?, confirmation}` → `TerminalInfo`;
//!   `GET envs/{name}/proxy-url?path=` → `{url|null, port?, reason?}`.

pub(crate) mod deploy;
pub(crate) mod detect;
pub(crate) mod envs;
pub(crate) mod expand;
pub(crate) mod health;
pub(crate) mod http_client;
mod mcp_tools;
pub(crate) mod output;
pub(crate) mod proxy;
pub(crate) mod remote;
pub(crate) mod runs;
#[cfg(test)]
mod tests;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::app::AppState;
use crate::error::ApiResult;
use crate::mcp::McpTool;
use crate::terminals::TerminalInfo;

pub use detect::detect;

#[derive(Default)]
pub struct AppsState {
    pub(crate) runs: runs::Runs,
    pub(crate) envs: envs::Envs,
    pub(crate) proxies: proxy::Proxies,
    /// Cancelled on shutdown: stops pollers, supervisors and proxies.
    pub(crate) shutdown: CancellationToken,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{pid}/runs", get(list_runs))
        .route("/api/projects/{pid}/runs/stop-all", post(stop_all))
        .route("/api/projects/{pid}/runs/{name}/start", post(start_run))
        .route("/api/projects/{pid}/runs/{name}/restart", post(restart_run))
        .route("/api/projects/{pid}/runs/{name}/stop", post(stop_run))
        .route("/api/projects/{pid}/envs", get(list_envs))
        .route("/api/projects/{pid}/envs/check", post(check_all))
        .route("/api/projects/{pid}/envs/{name}/check", post(check_env))
        .route("/api/projects/{pid}/envs/{name}/version", post(env_version))
        .route("/api/projects/{pid}/envs/{name}/logs", post(env_logs))
        .route("/api/projects/{pid}/envs/{name}/command", post(env_command))
        .route("/api/projects/{pid}/envs/{name}/deploy/check", post(deploy_check))
        .route("/api/projects/{pid}/envs/{name}/deploy", post(deploy_run))
        .route("/api/projects/{pid}/envs/{name}/proxy-url", get(proxy_url))
        .route("/api/projects/{pid}/http/envs", get(http_client::envs))
        .route("/api/projects/{pid}/http/run", post(http_client::run))
}

pub async fn start(state: &AppState) {
    envs::start_supervisor(state);
}

/// Stop run configurations started by Workbench, pollers and preview proxies.
pub async fn shutdown(state: &AppState) {
    runs::shutdown(state).await;
    state.apps.shutdown.cancel();
    proxy::shutdown(state).await;
}

pub fn mcp_tools() -> Vec<McpTool> {
    mcp_tools::tools()
}

// ---------------------------------------------------------------- run handlers

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    #[serde(default)]
    free_port: bool,
    /// The user confirmed starting runs that deploy, release or reach remote hosts.
    #[serde(default)]
    confirmed: bool,
}

async fn list_runs(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Vec<runs::RunView>>> {
    let p = state.projects.require(&pid)?;
    Ok(Json(runs::list(&state, &p).await))
}

/// `428 confirmation_required` unless the user confirmed the risky runs this start launches.
async fn require_confirmation(state: &AppState, p: &crate::projects::Project, name: &str, restart: bool, confirmed: bool) -> ApiResult<()> {
    if confirmed {
        return Ok(());
    }
    let gated = runs::gated(state, p, name, restart, false).await?;
    if gated.is_empty() {
        return Ok(());
    }
    Err(crate::error::ApiError::new(
        axum::http::StatusCode::PRECONDITION_REQUIRED,
        "confirmation_required",
        format!("{} may deploy, release or reach a remote host; confirm to start it", gated.join(", ")),
    ))
}

async fn start_run(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    body: Option<Json<StartBody>>,
) -> ApiResult<Json<runs::RunView>> {
    let p = state.projects.require(&pid)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    require_confirmation(&state, &p, &name, false, b.confirmed).await?;
    runs::start(&state, &p, &name, b.free_port).await?;
    Ok(Json(runs::view(&state, &p, &name).await?))
}

async fn restart_run(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    body: Option<Json<StartBody>>,
) -> ApiResult<Json<runs::RunView>> {
    let p = state.projects.require(&pid)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    require_confirmation(&state, &p, &name, true, b.confirmed).await?;
    runs::restart(&state, &p, &name, b.free_port).await?;
    Ok(Json(runs::view(&state, &p, &name).await?))
}

async fn stop_run(State(state): State<AppState>, Path((pid, name)): Path<(String, String)>) -> ApiResult<Json<runs::RunView>> {
    let p = state.projects.require(&pid)?;
    runs::stop(&state, &p, &name).await?;
    Ok(Json(runs::view(&state, &p, &name).await?))
}

async fn stop_all(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    let n = runs::stop_all(&state, &p).await;
    Ok(Json(json!({ "stopped": n })))
}

// ---------------------------------------------------------------- env handlers

async fn list_envs(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Vec<envs::EnvView>>> {
    let p = state.projects.require(&pid)?;
    Ok(Json(envs::list(&state, &p)))
}

async fn check_all(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Vec<envs::EnvView>>> {
    let p = state.projects.require(&pid)?;
    futures::future::join_all(p.config.envs.iter().filter(|e| e.health.is_some()).map(|e| envs::check(&state, &p, e))).await;
    Ok(Json(envs::list(&state, &p)))
}

async fn check_env(State(state): State<AppState>, Path((pid, name)): Path<(String, String)>) -> ApiResult<Json<envs::HealthView>> {
    let p = state.projects.require(&pid)?;
    let e = envs::find(&p, &name)?;
    Ok(Json(envs::check(&state, &p, e).await))
}

async fn env_version(State(state): State<AppState>, Path((pid, name)): Path<(String, String)>) -> ApiResult<Json<envs::VersionInfo>> {
    let p = state.projects.require(&pid)?;
    let e = envs::find(&p, &name)?;
    Ok(Json(envs::probe_version(&state, &p, e).await?))
}

#[derive(Deserialize, Default)]
struct LogsBody {
    name: Option<String>,
}

async fn env_logs(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    body: Option<Json<LogsBody>>,
) -> ApiResult<Json<TerminalInfo>> {
    let p = state.projects.require(&pid)?;
    let e = envs::find(&p, &name)?;
    let which = body.and_then(|b| b.0.name);
    Ok(Json(envs::open_logs(&state, &p, e, which.as_deref()).await?))
}

#[derive(Deserialize)]
struct CommandBody {
    name: String,
    #[serde(default)]
    confirmed: bool,
}

async fn env_command(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    Json(body): Json<CommandBody>,
) -> ApiResult<Json<TerminalInfo>> {
    let p = state.projects.require(&pid)?;
    let e = envs::find(&p, &name)?;
    Ok(Json(envs::run_command(&state, &p, e, &body.name, body.confirmed).await?))
}

#[derive(Deserialize, Default)]
struct DeployBody {
    sha: Option<String>,
    #[serde(default)]
    confirmation: Value,
}

async fn deploy_check(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    body: Option<Json<DeployBody>>,
) -> ApiResult<Json<deploy::DeployPlan>> {
    let p = state.projects.require(&pid)?;
    let e = envs::find(&p, &name)?;
    let sha = body.and_then(|b| b.0.sha);
    Ok(Json(deploy::plan(&state, &p, e, sha.as_deref()).await?))
}

async fn deploy_run(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    Json(body): Json<DeployBody>,
) -> ApiResult<Json<TerminalInfo>> {
    let p = state.projects.require(&pid)?;
    let e = envs::find(&p, &name)?;
    Ok(Json(deploy::deploy(&state, &p, e, body.sha.as_deref(), &body.confirmation).await?))
}

#[derive(Deserialize, Default)]
struct ProxyQuery {
    path: Option<String>,
}

async fn proxy_url(
    State(state): State<AppState>,
    Path((pid, name)): Path<(String, String)>,
    Query(q): Query<ProxyQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<proxy::ProxyUrl>> {
    let path = q.path.unwrap_or_else(|| "/".into());
    Ok(Json(proxy::proxy_url(&state, &pid, &name, &path, &headers).await?))
}

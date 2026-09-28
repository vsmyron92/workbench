//! REST under `/api/projects/{pid}/devcontainer`.
//!
//! * `GET` → the panel's view (`?config=` another config's plan).
//! * `POST start|rebuild {config?, approve?}` → `{terminalId, state}`; without the
//!   current plan's hash: `409 {error: {code: "approval_required"}, plan}`.
//! * `POST stop`, `POST remove {confirm: true}`, `PUT settings {useContainer?, config?,
//!   hostRun?: {name, host}}`.
//! * `GET scaffold` → a proposed `.devcontainer/devcontainer.json`; `POST scaffold
//!   {path, content}` writes it, never over an existing file.
//!
//! Only the user acts: in-process calls (MCP tools, i.e. agents) get 403 on every
//! write, so an agent can read the status but never start, rebuild, stop or remove.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ops::{self, StartError};
use super::scaffold;
use crate::app::AppState;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{pid}/devcontainer", get(view))
        .route("/api/projects/{pid}/devcontainer/start", post(start))
        .route("/api/projects/{pid}/devcontainer/rebuild", post(rebuild))
        .route("/api/projects/{pid}/devcontainer/stop", post(stop))
        .route("/api/projects/{pid}/devcontainer/remove", post(remove))
        .route("/api/projects/{pid}/devcontainer/settings", put(settings))
        .route("/api/projects/{pid}/devcontainer/scaffold", get(scaffold_get).post(scaffold_write))
        .merge(super::services::router())
}

/// Writes are the user's: refuse in-process (agent) callers.
fn user_only(caller: &Option<Extension<Caller>>) -> ApiResult<()> {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("dev containers are started, stopped and removed by the user only")),
        _ => Ok(()),
    }
}

#[derive(Deserialize, Default)]
struct ViewQuery {
    config: Option<String>,
}

async fn view(State(state): State<AppState>, Path(pid): Path<String>, Query(q): Query<ViewQuery>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    Ok(Json(ops::view(&state, &p, q.config.as_deref()).await?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct StartBody {
    config: Option<String>,
    approve: Option<String>,
}

fn start_response(r: Result<Value, StartError>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(StartError::Api(e)) => e.into_response(),
        Err(StartError::Approval(a)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": { "code": "approval_required", "message": a.message }, "plan": a.plan })),
        )
            .into_response(),
    }
}

async fn start(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(pid): Path<String>,
    body: Option<Json<StartBody>>,
) -> Response {
    if let Err(e) = user_only(&caller) {
        return e.into_response();
    }
    let p = match state.projects.require(&pid) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let b = body.map(|b| b.0).unwrap_or_default();
    start_response(ops::start(&state, &p, b.config.as_deref(), b.approve.as_deref(), false).await)
}

async fn rebuild(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(pid): Path<String>,
    body: Option<Json<StartBody>>,
) -> Response {
    if let Err(e) = user_only(&caller) {
        return e.into_response();
    }
    let p = match state.projects.require(&pid) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let b = body.map(|b| b.0).unwrap_or_default();
    start_response(ops::start(&state, &p, b.config.as_deref(), b.approve.as_deref(), true).await)
}

async fn stop(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    ops::stop(&state, &p).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RemoveBody {
    confirm: bool,
}

async fn remove(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(pid): Path<String>,
    body: Option<Json<RemoveBody>>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    if !body.is_some_and(|b| b.confirm) {
        return Err(ApiError::bad_request("removing the container needs {\"confirm\": true}"));
    }
    let p = state.projects.require(&pid)?;
    ops::remove(&state, &p).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn settings(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(pid): Path<String>,
    Json(b): Json<ops::SettingsBody>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    ops::settings(&state, &p, b).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn scaffold_get(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<scaffold::Proposal>> {
    let p = state.projects.require(&pid)?;
    let root = p.root.clone();
    let config = p.config.clone();
    let proposal = tokio::task::spawn_blocking(move || scaffold::propose(&root, &config))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(proposal))
}

#[derive(Deserialize)]
struct ScaffoldBody {
    path: String,
    content: String,
}

async fn scaffold_write(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(pid): Path<String>,
    Json(b): Json<ScaffoldBody>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let path = scaffold::write(&p.root, &b.path, &b.content)?;
    super::refresh(&state).await;
    state.events.emit("projects.changed", None, json!({}));
    Ok(Json(json!({ "ok": true, "path": path })))
}

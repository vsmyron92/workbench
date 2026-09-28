//! Database slice (OWNER: db slice): the Database tool window's data sources
//! (PostgreSQL), their schemas, and SQL consoles.
//!
//! Data sources are `[[database]]` entries of the project config (merged by name
//! like `[[run]]`); credentials are secret *names* (`password`, or `url` for a whole
//! connection URL), resolved in the backend and never sent anywhere but the database.
//! Without a password `~/.pgpass` is used, as libpq would. The UI edits the machine
//! overlay's entries (`sources`).
//!
//! REST under `/api/projects/{pid}/db`:
//! * `GET` → `{sources, secretNames}` (no values).
//! * `PUT _sources/{name} {source, previousName?}`, `DELETE _sources/{name}`: the overlay.
//! * `POST {name}/test` → `{version, ssl, ms}`.
//! * `GET {name}/schema` → schemas with tables and views; `GET {name}/table?schema=&table=`
//!   → columns, indexes, foreign keys.
//! * `POST {name}/query {sql, console, maxRows?}` → results (the console's session:
//!   one connection per console, idle 15 min), `POST {name}/consoles/{console}/cancel`,
//!   `DELETE {name}/consoles/{console}`.
//!
//! Queries run what the user typed with the database user's rights: only the user
//! acts. In-process callers (agents over MCP) get 403 on everything but the list.

mod conn;
pub mod query;
mod schema;
mod sources;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::extract::{Extension, Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::auth::Caller;
use crate::config::project::DatabaseSource;
use crate::error::{ApiError, ApiResult};
use crate::projects::Project;
use query::{Key, Session, Sessions};

/// The tree's own session per source (never a console's).
const META: &str = "_meta";

#[derive(Default)]
pub struct DbState {
    sessions: Sessions,
    started: AtomicBool,
    stop: tokio_util::sync::CancellationToken,
}

pub fn router() -> Router<AppState> {
    let p = "/api/projects/{pid}/db";
    Router::new()
        .route(p, get(list))
        .route(&format!("{p}/_sources/{{name}}"), put(put_source).delete(delete_source))
        .route(&format!("{p}/{{name}}/test"), post(test))
        .route(&format!("{p}/{{name}}/schema"), get(schema_route))
        .route(&format!("{p}/{{name}}/table"), get(table_route))
        .route(&format!("{p}/{{name}}/query"), post(query_route))
        .route(&format!("{p}/{{name}}/consoles/{{console}}/cancel"), post(cancel))
        .route(&format!("{p}/{{name}}/consoles/{{console}}"), delete(close))
}

pub async fn start(state: &AppState) {
    if state.db.started.swap(true, Ordering::Relaxed) {
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => st.db.sessions.reap(),
                _ = st.db.stop.cancelled() => return,
            }
        }
    });
}

pub async fn shutdown(state: &AppState) {
    state.db.stop.cancel();
    state.db.sessions.map.lock().clear();
}

/// Everything that connects runs the user's SQL with the database user's rights.
fn user_only(caller: &Option<Extension<Caller>>) -> ApiResult<()> {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("databases are queried by the user only")),
        _ => Ok(()),
    }
}

fn source_of(p: &Project, name: &str) -> ApiResult<DatabaseSource> {
    p.config.databases.iter().find(|d| d.name == name).cloned().ok_or_else(|| ApiError::not_found(format!("no data source {name:?} in {}", p.name)))
}

fn valid_console(c: &str) -> bool {
    !c.is_empty() && c.len() <= 64 && !c.starts_with('_') && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The console's session, connecting (again) when there is none, it died, or the
/// source's settings or secrets changed.
async fn session(state: &AppState, p: &Project, src: &DatabaseSource, console: &str) -> ApiResult<Arc<Session>> {
    let r = conn::resolve(state, p, src)?;
    let key: Key = (p.id.clone(), src.name.clone(), console.to_string());
    if let Some(s) = state.db.sessions.get(&key) {
        if s.fingerprint == r.fingerprint && !s.client.is_closed() {
            return Ok(s);
        }
        state.db.sessions.remove(&key);
    }
    let c = conn::connect(&r).await?;
    let s = Arc::new(Session::new(c, r.fingerprint));
    state.db.sessions.map.lock().insert(key, s.clone());
    Ok(s)
}

// ---------------------------------------------------------------- routes

async fn list(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    let overlay = std::fs::read_to_string(state.paths.project_overlay(&p.id)).unwrap_or_default();
    let own = sources::overlay_names(&overlay);
    let sources: Vec<Value> = p
        .config
        .databases
        .iter()
        .map(|d| {
            json!({
                "name": d.name, "kind": if d.kind.is_empty() { "postgres" } else { d.kind.as_str() },
                "host": d.host, "port": d.port, "database": d.database, "user": d.user,
                "password": d.password, "url": d.url, "sslmode": d.sslmode, "readOnly": d.read_only,
                "origin": if own.contains(&d.name) { "overlay" } else { "repository" },
            })
        })
        .collect();
    let mut names: Vec<String> = p.config.secrets.keys().cloned().collect();
    names.extend(state.config.read().secrets.keys().cloned());
    names.sort();
    names.dedup();
    Ok(Json(json!({ "sources": sources, "secretNames": names, "overlayPath": crate::config::contract_tilde(&state.paths.project_overlay(&p.id)) })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutSource {
    source: DatabaseSource,
    previous_name: Option<String>,
}

async fn put_source(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path((pid, name)): Path<(String, String)>,
    Json(b): Json<PutSource>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let mut src = b.source;
    src.name = name.clone();
    sources::validate(&src)?;
    let _serial = state.platform.save_lock.lock().await;
    let path = state.paths.project_overlay(&p.id);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let prev = b.previous_name.filter(|n| n != &name);
    // A rename replaces the old entry in place.
    let text = match &prev {
        Some(old) if sources::overlay_names(&text).contains(old) => {
            let t = sources::edit(&text, old, Some(&src))?.unwrap_or(text);
            t
        }
        _ => sources::edit(&text, &name, Some(&src))?.unwrap_or(text),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ApiError::internal(format!("create {}: {e}", dir.display())))?;
    }
    crate::util::fs::write_atomic(&path, text.as_bytes(), 0o600)?;
    state.projects.reload(&state).await;
    state.db.sessions.close_source(&p.id, &name);
    if let Some(old) = prev {
        state.db.sessions.close_source(&p.id, &old);
    }
    Ok(Json(json!({ "ok": true })))
}

async fn delete_source(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let _serial = state.platform.save_lock.lock().await;
    let path = state.paths.project_overlay(&p.id);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let Some(out) = sources::edit(&text, &name, None)? else {
        return Err(ApiError::conflict(format!("{name:?} is defined by the repository's .workbench.toml, not the machine overlay: change it there")));
    };
    crate::util::fs::write_atomic(&path, out.as_bytes(), 0o600)?;
    state.projects.reload(&state).await;
    state.db.sessions.close_source(&p.id, &name);
    Ok(Json(json!({ "ok": true })))
}

/// Connect afresh (not a console's session), report the server and whether TLS is on.
async fn test(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let src = source_of(&p, &name)?;
    let r = conn::resolve(&state, &p, &src)?;
    let started = Instant::now();
    let c = conn::connect(&r).await?;
    let row = c
        .client
        .query_one("SELECT version(), COALESCE((SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()), false), current_user::text", &[])
        .await
        .map_err(|e| ApiError::upstream(conn::connect_error(&e)))?;
    Ok(Json(json!({
        "version": row.get::<_, String>(0), "ssl": row.get::<_, bool>(1), "user": row.get::<_, String>(2),
        "ms": started.elapsed().as_millis() as u64, "display": r.display, "sslmode": r.tls.name(),
    })))
}

async fn schema_route(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let src = source_of(&p, &name)?;
    let s = session(&state, &p, &src, META).await?;
    let _busy = s.busy.lock().await;
    let c = schema::catalog(&s.client).await.map_err(|e| ApiError::upstream(e.message))?;
    Ok(Json(serde_json::to_value(c)?))
}

#[derive(Deserialize)]
struct TableQuery {
    schema: String,
    table: String,
}

async fn table_route(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path((pid, name)): Path<(String, String)>,
    Query(q): Query<TableQuery>,
) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let src = source_of(&p, &name)?;
    let s = session(&state, &p, &src, META).await?;
    let _busy = s.busy.lock().await;
    let t = schema::table(&s.client, &q.schema, &q.table).await.map_err(|e| ApiError::bad_request(e.message))?;
    Ok(Json(serde_json::to_value(t)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryBody {
    sql: String,
    console: String,
    max_rows: Option<usize>,
}

async fn query_route(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path((pid, name)): Path<(String, String)>,
    Json(b): Json<QueryBody>,
) -> ApiResult<Json<query::QueryOutcome>> {
    user_only(&caller)?;
    if !valid_console(&b.console) {
        return Err(ApiError::bad_request("console is 1–64 letters, digits, - or _"));
    }
    if b.sql.trim().is_empty() {
        return Err(ApiError::bad_request("nothing to run"));
    }
    let p = state.projects.require(&pid)?;
    let src = source_of(&p, &name)?;
    let s = session(&state, &p, &src, &b.console).await?;
    let Ok(_busy) = s.busy.try_lock() else {
        return Err(ApiError::conflict("a query is still running in this console: cancel it or wait"));
    };
    let max = b.max_rows.unwrap_or(query::DEFAULT_MAX_ROWS).clamp(1, query::MAX_ROWS);
    Ok(Json(query::run(&s, &b.sql, max).await))
}

async fn cancel(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, name, console)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let Some(s) = state.db.sessions.get(&(pid, name, console)) else { return Ok(Json(json!({ "ok": true, "running": false }))) };
    let running = s.busy.try_lock().is_err();
    if running {
        s.canceller.cancel().await.map_err(|e| ApiError::upstream(conn::connect_error(&e)))?;
    }
    Ok(Json(json!({ "ok": true, "running": running })))
}

async fn close(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path((pid, name, console)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let closed = state.db.sessions.remove(&(pid, name, console)).is_some();
    Ok(Json(json!({ "ok": true, "closed": closed })))
}

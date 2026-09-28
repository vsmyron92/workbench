//! Workspace slice (OWNER: workspace slice).
//!
//! Mr. Mak-style deliverable cards, generalized to any project: per project (and a
//! global `home`), each card holds ordered steps — HTML reports, markdown, image
//! galleries, 3D compare manifests, PDFs, videos. Agents register their outputs
//! through MCP tools (`workspace_*`); the UI shows them in a home grid and a card panel.
//! Adapted from Mr. Mak Workspace (MIT): registry schema, freshness sort, auto-archive,
//! report chrome and the report lightbox.
//!
//! * `model`  — the registry schema, order-preserving JSON, dates, sort and archive rules.
//! * `store`  — scopes, compare-and-swap registry writes, card files, text content.
//! * `view`   — capability grants and the sandboxed content server (`/view/{grant}/**`).
//! * `routes` — REST `/api/workspace/**`.
//! * `tools`  — MCP tools.
//! * `trash`  — deleted cards: list, restore, delete for good (`…/{scope}/trash/**`).
//! * `watch`  — registry and card-file watching → `workspace.changed`.

mod model;
mod routes;
mod store;
mod tools;
mod trash;
mod view;
mod watch;

use std::collections::HashMap;
use std::path::PathBuf;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use parking_lot::Mutex;
use serde_json::json;

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::McpTool;

#[derive(Default)]
pub struct WorkspaceState {
    /// Capability URLs for card folders.
    grants: view::Grants,
    /// Serializes our registry and content writes (other writers are handled by CAS).
    write_lock: tokio::sync::Mutex<()>,
    /// Report file → the first local image in it, by (mtime, size).
    thumbs: Mutex<HashMap<PathBuf, ((i64, u64), Option<String>)>>,
    watcher: watch::Watcher,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workspace/scopes", get(routes::scopes))
        .route("/api/workspace/cards", get(routes::all_cards))
        .route("/api/workspace/{scope}/cards", get(routes::list_cards).post(routes::create))
        .route("/api/workspace/{scope}/cards/{id}", get(routes::get_card).patch(routes::patch).delete(routes::delete))
        .route("/api/workspace/{scope}/cards/{id}/steps", post(routes::add_step))
        .route("/api/workspace/{scope}/cards/{id}/steps/{index}", axum::routing::patch(routes::patch_step).delete(routes::delete_step))
        .route("/api/workspace/{scope}/cards/{id}/files", get(routes::files))
        .route(
            "/api/workspace/{scope}/cards/{id}/content",
            get(routes::read_content).put(routes::write_content).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/api/workspace/{scope}/cards/{id}/upload", post(routes::upload).layer(DefaultBodyLimit::disable()))
        .route("/api/workspace/{scope}/cards/{id}/grant", post(routes::grant))
        .route("/api/workspace/{scope}/trash", get(routes::trash_list).delete(routes::trash_empty))
        .route("/api/workspace/{scope}/trash/{item}", axum::routing::delete(routes::trash_delete))
        .route("/api/workspace/{scope}/trash/{item}/restore", post(routes::trash_restore))
        .route("/view", get(view::view_root))
        .route("/view/", get(view::view_root))
        .route("/view/{grant}", get(view::view_root))
        .route("/view/{grant}/", get(view::view_root))
        .route("/view/{grant}/{*path}", get(view::view))
}

pub async fn start(state: &AppState) {
    watch::start(state).await;
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

/// Run blocking filesystem work off the async workers.
async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError::internal(format!("background task failed: {e}")))?
}

/// `workspace.trash {scope}` (`projectId` = the scope's project): its trash changed.
fn emit_trash(state: &AppState, scope: &str, project: Option<&str>) {
    state.events.emit("workspace.trash", project, json!({ "scope": scope }));
}

/// `workspace.changed {scope, cardId?}` (`projectId` = the scope's project).
fn emit_changed(state: &AppState, scope: &str, project: Option<&str>, card: Option<&str>) {
    let data = match card {
        Some(c) => json!({ "scope": scope, "cardId": c }),
        None => json!({ "scope": scope }),
    };
    state.events.emit("workspace.changed", project, data);
}

#[cfg(test)]
mod tests;

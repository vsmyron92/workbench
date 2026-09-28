//! Atlassian slice (OWNER: atlassian slice).
//!
//! Confluence (spaces, page tree, view/edit, comments, search, history, attachments)
//! and Jira (enabled only when the site has Jira). Routes: `/api/atlassian/**`,
//! `/api/confluence/**`, `/api/jira/**`. MCP tools: `confluence_*`, `jira_*`.
//!
//! * `client`  — site/credential resolution, auth, 429 back-off, error mapping
//! * `status`  — cached credential/product check (`GET /api/atlassian/status`; "not set
//!   up" is a 200 with `configured: false`, not a 412)
//! * `confluence`, `jira` — REST handlers and the core functions MCP tools reuse
//!   (page updates require the base `version`: no blind overwrites)
//! * `html`    — view HTML → sanitized HTML (ammonia) with proxied images and
//!   internal links marked for the UI
//! * `storage`, `xml` — storage format: inline-comment markers, validation, text
//! * `markdown` — markdown → storage / ADF, ADF → markdown
//! * `comment_md` — comment bodies as markdown for editing, and whether that is lossy
//! * `attachments` — authenticated attachment proxy with a byte-bounded LRU
//! * `comments` — inline comments on a selection, comment edits, resolve, delete
//! * `files` — page attachments: list, upload (streamed), delete to the trash
//! * `pages` — labels, move, copy, trash / restore, watching, people search
//! * `agile` — Jira boards, columns, quick filters, sprints, board issues
//!
//! Events: `confluence.page` `{pageId, version, title, action}` after a page is
//! created or updated or commented on through Workbench; `jira.issue` `{key, action}`
//! after an issue changes.

mod agile;
mod attachments;
mod client;
mod comment_md;
mod comments;
mod confluence;
mod entities;
mod files;
mod html;
mod jira;
mod markdown;
mod pages;
mod status;
mod storage;
mod tools;
mod xml;

#[cfg(test)]
mod mock_tests;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use dashmap::DashMap;
use parking_lot::Mutex;

use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::McpTool;

pub use client::Product;
use client::{Api, resolve_site};

#[derive(Default)]
pub struct AtlassianState {
    status: Mutex<HashMap<String, status::Entry>>,
    status_lock: tokio::sync::Mutex<()>,
    /// `site|accountId` → display name.
    names: DashMap<String, String>,
    /// `site|spaceId` → space.
    spaces: DashMap<String, confluence::SpaceOut>,
    attachments: Mutex<attachments::Lru>,
}

const MAX_NAME_CACHE: usize = 5000;
/// Status entries older than this are forgotten when another one is stored.
const STATUS_KEEP: Duration = Duration::from_secs(3600);

impl AtlassianState {
    fn cached_status(&self, key: &str) -> Option<status::StatusOut> {
        let map = self.status.lock();
        let e = map.get(key)?;
        (e.at.elapsed() < e.ttl).then(|| e.status.clone())
    }

    fn store_status(&self, key: &str, s: status::StatusOut, ttl: Duration) {
        let mut map = self.status.lock();
        // Keys include a credentials digest, so replaced tokens leave entries behind.
        // Expired entries still answer `auth_known_bad` for a while; drop old ones.
        map.retain(|_, e| e.at.elapsed() < STATUS_KEEP);
        map.insert(key.to_string(), status::Entry { at: Instant::now(), ttl, status: s });
    }

    /// The last check for this site found the credentials rejected.
    fn auth_known_bad(&self, key: &str) -> bool {
        self.status.lock().get(key).is_some_and(|e| e.status.auth_failed)
    }

    fn remember_name(&self, key: String, name: String) {
        if self.names.len() > MAX_NAME_CACHE {
            self.names.clear();
        }
        self.names.insert(key, name);
    }
}

/// An authenticated client for the site a project (or the global config) uses.
pub(crate) fn api_for(state: &AppState, project_id: Option<&str>, product: Product) -> Result<Api, ApiError> {
    let site = resolve_site(state, project_id, product)?;
    let mut api = Api::new(state.http.clone(), site, product);
    api.auth_known_bad = state.atlassian.auth_known_bad(&api.site.key());
    Ok(api)
}

/// A Jira client, or `not_configured` when the site has no Jira.
pub(crate) async fn jira_api(state: &AppState, project_id: Option<&str>) -> Result<Api, ApiError> {
    let api = api_for(state, project_id, Product::Jira)?;
    let st = status::check(state, project_id, false).await?;
    if !st.jira && st.site == api.site.base {
        return Err(ApiError::not_configured(format!(
            "{} has no Jira (GET /rest/api/3/serverInfo did not succeed)",
            api.site.host()
        )));
    }
    Ok(api)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/atlassian/status", get(status::handler))
        .route("/api/confluence/spaces", get(confluence::spaces_handler))
        .route("/api/confluence/spaces/{id}/pages", get(confluence::space_pages_handler))
        .route("/api/confluence/pages", get(confluence::pages_by_ids_handler).post(confluence::create_handler))
        .route(
            "/api/confluence/pages/{id}",
            get(confluence::page_handler).put(confluence::update_handler).delete(pages::trash_handler),
        )
        .route("/api/confluence/pages/{id}/restore", post(pages::restore_handler))
        .route("/api/confluence/pages/{id}/children", get(confluence::children_handler))
        .route("/api/confluence/pages/{id}/versions", get(confluence::versions_handler))
        .route("/api/confluence/pages/{id}/versions/{n}", get(confluence::version_handler))
        .route("/api/confluence/pages/{id}/comments", get(confluence::comments_handler).post(confluence::add_comment_handler))
        .route("/api/confluence/pages/{id}/inline-comments", post(comments::create_inline_handler))
        .route("/api/confluence/comments/{kind}/{cid}", put(comments::update_handler).delete(comments::delete_handler))
        .route(
            "/api/confluence/pages/{id}/attachments",
            get(files::list_handler).post(files::upload_handler).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/confluence/pages/{id}/labels", get(pages::labels_handler).post(pages::add_labels_handler))
        .route("/api/confluence/pages/{id}/labels/{name}", delete(pages::remove_label_handler))
        .route("/api/confluence/pages/{id}/move", post(pages::move_handler))
        .route("/api/confluence/pages/{id}/copy", post(pages::copy_handler))
        .route("/api/confluence/pages/{id}/watch", get(pages::watch_get_handler).put(pages::watch_put_handler))
        .route("/api/confluence/users", get(pages::users_handler))
        .route("/api/confluence/search", get(confluence::search_handler))
        .route("/api/confluence/markdown", post(confluence::markdown_handler))
        .route("/api/confluence/attachments/{att}", delete(files::delete_handler))
        .route("/api/confluence/attachments/{page}/by-name/{name}", get(confluence::attachment_by_name_handler))
        .route("/api/confluence/attachments/{page}/{att}", get(confluence::attachment_handler))
        .route("/api/jira/myself", get(jira::myself_handler))
        .route("/api/jira/projects", get(jira::projects_handler))
        .route("/api/jira/search", get(jira::search_get_handler).post(jira::search_post_handler))
        .route("/api/jira/issues", post(jira::create_handler))
        .route("/api/jira/issues/{key}", get(jira::issue_handler).put(jira::update_handler))
        .route("/api/jira/issues/{key}/transitions", get(agile::transitions_handler).post(jira::transition_handler))
        .route("/api/jira/issues/{key}/comments", get(jira::comments_handler).post(jira::add_comment_handler))
        .route("/api/jira/issues/{key}/assignee", put(jira::assign_handler))
        .route("/api/jira/createmeta/{project}", get(jira::createmeta_handler))
        .route("/api/jira/attachments/{id}", get(jira::attachment_handler))
        .route("/api/jira/boards", get(agile::boards_handler))
        .route("/api/jira/boards/{id}", get(agile::board_handler))
        .route("/api/jira/boards/{id}/sprints", get(agile::sprints_handler))
        .route("/api/jira/boards/{id}/issues", get(agile::issues_handler))
}

pub async fn start(_state: &AppState) {}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::all()
}

//! Files & editor slice (OWNER: files slice).
//!
//! Project file tree, read/write with optimistic concurrency, file watching
//! (emits `fs.changed` / `git.changed`), project search and quick-open.
//! Routes: `/api/projects/{pid}/files/**`, `/api/projects/{pid}/search`, `/api/fs/**`.
//!
//! Layout of the slice:
//! * `listing`   — one directory at a time for the lazy tree (gitignore-aware).
//! * `content`   — read / stat / raw (Range) / write (sha256 etag, atomic, re-checked).
//! * `ops`       — mkdir, create, rename, copy, delete (to the trash), uploads.
//! * `abs`       — read-only access to absolute paths inside project and extra roots.
//! * `watch`     — one inotify watcher per project (ignore-aware, capped).
//! * `search`    — find/replace in files with ripgrep's libraries.
//! * `quickopen` + `fuzzy` — cached file list and fuzzy ranking for "Go to file".
//! * `tools`     — MCP tools (`workbench_open_file`, `workbench_open_markdown`).
//! * `history`   — Local History: versions saved here or seen on disk, labels,
//!   `…/files/history/**`, MCP `files_local_history`.
//!
//! Every client path goes through `resolve` (→ `util::paths::resolve_in_root`), and
//! every blocking filesystem call runs on the blocking pool (`blocking`).

mod abs;
mod content;
mod fuzzy;
mod gitignore;
pub mod history;
mod listing;
mod ops;
mod quickopen;
mod search;
mod sensitive;
mod todo;
mod tools;
mod trash;
mod watch;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::McpTool;
use crate::projects::Project;
use crate::util;

pub use sensitive::Sensitive;

/// Text files larger than this are not sent to the editor (`tooLarge`).
pub const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;
/// Directory names never watched, indexed or searched even without a `.gitignore`.
pub const HARD_IGNORE: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".venv"];

/// Slice state on `AppState::files`.
pub struct FilesState {
    /// Project id → its watcher.
    watchers: Mutex<HashMap<String, watch::ProjectWatch>>,
    /// Serializes watcher reconciliation (start + `projects.changed`).
    sync_lock: tokio::sync::Mutex<()>,
    /// Quick-open file lists.
    quick: quickopen::QuickOpenCache,
    /// Saves are check-then-rename; serializing them closes the window where two
    /// concurrent saves of one file both pass the etag check.
    write_lock: tokio::sync::Mutex<()>,
    /// Local History stores and agent notes.
    pub(crate) history: history::HistoryState,
}

impl Default for FilesState {
    fn default() -> Self {
        Self {
            watchers: Mutex::new(HashMap::new()),
            sync_lock: tokio::sync::Mutex::new(()),
            quick: quickopen::QuickOpenCache::default(),
            write_lock: tokio::sync::Mutex::new(()),
            history: history::HistoryState::default(),
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{pid}/files/list", get(listing::list))
        .route("/api/projects/{pid}/files/read", get(content::read))
        .route("/api/projects/{pid}/files/stat", get(content::stat))
        .route("/api/projects/{pid}/files/raw", get(content::raw))
        .route(
            "/api/projects/{pid}/files/write",
            put(content::write).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/api/projects/{pid}/files/op", post(ops::op))
        .route(
            "/api/projects/{pid}/files/upload",
            post(ops::upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/projects/{pid}/files/find", get(quickopen::find))
        .route("/api/projects/{pid}/files/watch", get(watch::status))
        .route("/api/projects/{pid}/files/history", get(history::routes::file_history))
        .route("/api/projects/{pid}/files/history/dir", get(history::routes::dir_history))
        .route("/api/projects/{pid}/files/history/revision", get(history::routes::revision))
        .route("/api/projects/{pid}/files/history/diff", get(history::routes::diff))
        .route("/api/projects/{pid}/files/history/label", post(history::routes::label))
        .route("/api/projects/{pid}/files/history/stats", get(history::routes::stats))
        .route("/api/projects/{pid}/files/history/session", get(history::routes::session))
        .route("/api/projects/{pid}/files/todos", get(todo::todos))
        .route("/api/projects/{pid}/search", get(search::search))
        .route(
            "/api/projects/{pid}/search/replace",
            post(search::replace).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/api/fs/read", get(abs::read))
        .route("/api/fs/stat", get(abs::stat))
        .route("/api/fs/raw", get(abs::raw))
}

/// Start a watcher for every project now, and keep the set in sync with
/// `projects.changed`.
pub async fn start(state: &AppState) {
    history::start(state);
    watch::sync_all(state).await;
    let st = state.clone();
    tokio::spawn(async move {
        let mut rx = st.events.subscribe();
        loop {
            match rx.recv().await {
                Ok(ev) if ev.kind == "projects.changed" => watch::sync_all(&st).await,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => watch::sync_all(&st).await,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

pub fn mcp_tools() -> Vec<McpTool> {
    let mut v = tools::tools();
    v.extend(history::mcp_tools());
    v
}

// ---------------------------------------------------------------- shared helpers

/// A client path resolved inside a project.
pub(crate) struct Resolved {
    pub project: Arc<Project>,
    /// Absolute path (not canonicalized: symlinks inside the project keep their spelling).
    pub abs: PathBuf,
    /// Normalized project-relative path with `/` separators (`""` = the root).
    pub rel: String,
}

/// Resolve `rel` (project-relative, as sent by the client) inside project `pid`.
pub(crate) fn resolve(state: &AppState, pid: &str, rel: &str) -> ApiResult<Resolved> {
    let project = state.projects.require(pid)?;
    let abs = util::paths::resolve_in_root(&project.root, rel)?;
    let rel = util::paths::relative_to(&project.root, &abs).unwrap_or_default();
    Ok(Resolved { project, abs, rel })
}

/// Like [`resolve`] for operations on the entry itself (trash, rename, copy source):
/// a final symlink is not followed, so a link that points outside the project can be
/// removed or moved. Never read or write through the result.
pub(crate) fn resolve_entry(state: &AppState, pid: &str, rel: &str) -> ApiResult<Resolved> {
    let project = state.projects.require(pid)?;
    let abs = util::paths::resolve_entry_in_root(&project.root, rel)?;
    let rel = util::paths::relative_to(&project.root, &abs).unwrap_or_default();
    Ok(Resolved { project, abs, rel })
}

/// Whether a project-relative path lies inside a `.git` directory (read-only for us;
/// `.GIT` too where names ignore case).
pub(crate) fn in_git_dir(rel: &str) -> bool {
    Path::new(rel).components().any(|c| matches!(c, Component::Normal(n) if util::os::path::same_name(n, ".git")))
}

/// Run blocking filesystem work off the async workers.
pub(crate) async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("background task failed: {e}")))?
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Modification time in ms since the epoch (0 when unknown).
pub(crate) fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The last path segment of a `/`-separated relative path.
pub(crate) fn basename(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Join a relative directory and a name with `/`.
pub(crate) fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// A file or directory name a client may create: no separators, not `.`/`..`, and
/// one Windows keeps as it is (`os::path::check_component`).
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !util::os::path::has_separator(name)
        && !name.contains('\0')
        && util::os::path::check_component(name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_dir_detection_matches_any_component() {
        assert!(in_git_dir(".git"));
        assert!(in_git_dir(".git/HEAD"));
        assert!(in_git_dir("vendor/lib/.git/config"));
        assert!(!in_git_dir(".github/workflows/ci.yml"));
        assert!(!in_git_dir(".gitignore"));
        assert!(!in_git_dir(""));
    }

    #[test]
    fn names_reject_separators_and_dots() {
        assert!(valid_name("a.txt"));
        assert!(valid_name(".env"));
        assert!(!valid_name(""));
        assert!(!valid_name("."));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\0b"));
        assert!(!valid_name(&"x".repeat(256)));
        #[cfg(windows)]
        for bad in [r"a\b", "a:b", "CON", "nul.txt", "x.", "x "] {
            assert!(!valid_name(bad), "{bad}");
        }
        #[cfg(windows)]
        assert!(in_git_dir(".GIT/config"));
    }

    #[test]
    fn rel_helpers() {
        assert_eq!(basename("src/main.rs"), "main.rs");
        assert_eq!(basename("main.rs"), "main.rs");
        assert_eq!(join_rel("", "a"), "a");
        assert_eq!(join_rel("src", "a"), "src/a");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}

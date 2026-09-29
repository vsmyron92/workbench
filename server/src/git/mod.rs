//! Version control slice (OWNER: git slice).
//!
//! CLion-style git on the git CLI: status, diffs, (hunk and line) staging, commit
//! (partial too), branches, log graph, blame, stash, shelf, changelists, conflicts,
//! interactive rebase, bisect, fetch/pull/push. Routes live under
//! `/api/projects/{pid}/git/**` (see `routes.rs`); cross-slice shapes (`GitStatus`,
//! `GitFileDiff`, `GitBlame`) are fixed by docs/ARCHITECTURE.md.
//!
//! Concurrency: several agents may share a worktree, so every mutating operation
//! takes a per-repository async mutex, read-only commands never take git's
//! optional locks, and hunk operations are guarded by a diff fingerprint.
//!
//! CONTRACT: `cli_askpass` (GIT_ASKPASS helper), `cli_git_editor` (the interactive
//! rebase's non-interactive editor), `router`, `start`, `mcp_tools`.

mod askpass;
mod bisect;
mod blame;
mod changelists;
mod cmd;
mod conflicts;
mod diff;
mod eol;
mod lines;
mod log;
mod mcp;
mod ops;
mod rebase_i;
mod refs;
mod remote;
mod repo;
mod routes;
mod shelf;
mod stash;
mod status;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_flows;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::Router;
use dashmap::DashMap;

use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::McpTool;
use crate::projects::Project;

pub use repo::Repo;

#[derive(Default)]
pub struct GitState {
    /// One write lock per working tree (keyed by its top-level path).
    locks: DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>,
    /// Discovered repositories per project id (root, repo).
    repos: DashMap<String, (PathBuf, Arc<Repo>)>,
    /// Running and recent remote operations.
    pub ops: remote::OpRegistry,
    /// Environment pointing remote git commands at the askpass helper, once set up
    /// (`util::os::helper::askpass_env`).
    pub askpass: OnceLock<Vec<(String, String)>>,
    /// Shell command prefix of the git editor helper (`'<exe>' git-editor`; `/`
    /// separators on Windows, where Git for Windows runs it with its sh).
    pub editor: OnceLock<String>,
    /// Serialize changelist file access per project.
    changelist_locks: DashMap<String, Arc<parking_lot::Mutex<()>>>,
    /// Serialize shelf changes per project.
    shelf_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

impl GitState {
    pub fn changelist_lock(&self, pid: &str) -> Arc<parking_lot::Mutex<()>> {
        self.changelist_locks.entry(pid.to_string()).or_default().clone()
    }

    pub fn shelf_lock(&self, pid: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.shelf_locks.entry(pid.to_string()).or_default().clone()
    }

    /// The write lock of a working tree.
    pub fn lock(&self, top: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.entry(top.to_path_buf()).or_default().clone()
    }

    /// The repository of a project (cached; rediscovered when its git dir vanishes).
    pub async fn repo(&self, p: &Project) -> Result<Arc<Repo>, ApiError> {
        if let Some(e) = self.repos.get(&p.id) {
            let (root, repo) = e.value();
            if root == &p.root && repo.git_dir.exists() {
                return Ok(repo.clone());
            }
        }
        let repo = Repo::discover(&p.id, &p.root).await?.arc();
        self.repos.insert(p.id.clone(), (p.root.clone(), repo.clone()));
        Ok(repo)
    }
}

pub fn router() -> Router<AppState> {
    routes::router()
}

pub async fn start(state: &AppState) {
    match crate::util::os::helper::askpass_env(&state.paths.data_dir) {
        Ok(env) => {
            let _ = state.git.askpass.set(env);
        }
        Err(e) => tracing::warn!("git: cannot set up the askpass helper ({e:#}); remote operations rely on credential helpers"),
    }
    match crate::util::os::proc::current_exe() {
        Ok(exe) => {
            let _ = state.git.editor.set(format!("{} git-editor", rebase_i::sh_path(&exe)));
        }
        Err(e) => tracing::warn!("git: cannot locate the workbench binary ({e}); interactive rebase is unavailable"),
    }
}

pub fn mcp_tools() -> Vec<McpTool> {
    mcp::tools()
}

/// `workbench askpass "<prompt>"` — answers git's credential prompts from the
/// configured secrets (GitLab: user `oauth2`, password = token).
pub fn cli_askpass(prompt: &str) -> anyhow::Result<()> {
    askpass::cli_askpass(prompt)
}

/// `workbench git-editor <todo|message> <dir> <file>` — git's sequence and message
/// editor during an interactive rebase started by Workbench: writes the prepared todo
/// or commit message, never waits for a person.
pub fn cli_git_editor(mode: &str, dir: &str, file: &str) -> anyhow::Result<()> {
    rebase_i::cli_git_editor(mode, dir, file)
}

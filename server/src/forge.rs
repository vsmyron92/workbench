//! Code-hosting ("forge") dispatch: features that do not care whether a project
//! lives on GitLab or GitHub (deploy gates, the top-bar CI status) call these.
//!
//! A project may hold several git repositories (`projects/repos.rs`), each on its own
//! GitLab or GitHub project. The forge routes and tools therefore work on **one
//! repository** of a project: [`RepoParam`] reads `?repo=<id>` and
//! `ProjectRegistry::require_repo` turns it into the project seen through that
//! repository. Without a repository id it is the project's default one.

use std::sync::Arc;

use serde::Deserialize;

use crate::app::AppState;
use crate::error::ApiError;
use crate::projects::Project;

pub use crate::gitlab::CiStatus;

/// `?repo=<id>` of a forge route: which repository of the project the request is about
/// (`axum::extract::Query<RepoParam>`, next to the handler's own query). Absent or empty
/// means the default repository.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RepoParam {
    #[serde(default)]
    pub repo: Option<String>,
}

impl RepoParam {
    /// The selection a tool argument makes (`None`: the default repository).
    pub fn named(repo: Option<&str>) -> Self {
        Self { repo: repo.map(str::to_string) }
    }

    pub fn id(&self) -> Option<&str> {
        self.repo.as_deref().filter(|r| !r.is_empty())
    }
}

/// Which forge hosts a project, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    Gitlab,
    Github,
}

pub fn forge_of(project: &Project) -> Option<Forge> {
    if project.gitlab().is_some() {
        Some(Forge::Gitlab)
    } else if project.github().is_some() {
        Some(Forge::Github)
    } else {
        None
    }
}

/// The registry's copy of `project` (so a connection does not copy the whole
/// project), unless the caller holds another repository's view of it or a project the
/// registry does not know: then its own value.
pub(crate) fn shared(state: &AppState, project: &Project) -> Arc<Project> {
    state
        .projects
        .get(&project.id)
        .filter(|p| p.repo_id() == project.repo_id())
        .unwrap_or_else(|| Arc::new(project.clone()))
}

/// The repository id a panel opened for `project` must carry (`params.repo`), `None` for
/// the project's default repository: the web views address that one by the project id
/// alone.
pub(crate) fn panel_repo(state: &AppState, project: &Project) -> Option<String> {
    let default = state.projects.get(&project.id).map(|p| p.repo_id().to_string());
    Some(project.repo_id().to_string()).filter(|r| default.as_deref() != Some(r.as_str()))
}

/// Every repository of every project, each as the project seen through it: what a
/// background watcher walks (a project with one repository, or none, is just itself).
pub(crate) fn repo_views(state: &AppState) -> Vec<Arc<Project>> {
    let mut out = vec![];
    for project in state.projects.list() {
        if project.repos.len() < 2 {
            out.push(project);
            continue;
        }
        for entry in project.repos.iter() {
            if entry.id == project.repo_id() {
                out.push(project.clone());
            } else if let Some(view) = project.scoped(&entry.id) {
                out.push(Arc::new(view));
            }
        }
    }
    out
}

/// CI state of `sha` (full or short) on the project's forge. `Ok(None)` when the
/// commit has no pipeline/workflow run or the project is on no known forge.
pub async fn commit_ci_status(state: &AppState, project: &Project, sha: &str) -> Result<Option<CiStatus>, ApiError> {
    match forge_of(project) {
        Some(Forge::Gitlab) => crate::gitlab::commit_ci_status(state, project, sha).await,
        Some(Forge::Github) => crate::github::commit_ci_status(state, project, sha).await,
        None => Ok(None),
    }
}

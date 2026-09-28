//! Code-hosting ("forge") dispatch: features that do not care whether a project
//! lives on GitLab or GitHub (deploy gates, the top-bar CI status) call these.

use crate::app::AppState;
use crate::error::ApiError;
use crate::projects::Project;

pub use crate::gitlab::CiStatus;

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

/// CI state of `sha` (full or short) on the project's forge. `Ok(None)` when the
/// commit has no pipeline/workflow run or the project is on no known forge.
pub async fn commit_ci_status(state: &AppState, project: &Project, sha: &str) -> Result<Option<CiStatus>, ApiError> {
    match forge_of(project) {
        Some(Forge::Gitlab) => crate::gitlab::commit_ci_status(state, project, sha).await,
        Some(Forge::Github) => crate::github::commit_ci_status(state, project, sha).await,
        None => Ok(None),
    }
}

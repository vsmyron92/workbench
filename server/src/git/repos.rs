//! The project's repositories as a whole: `GET …/git/repos` (what the repository
//! switcher shows) and `GET …/git/status?repo=all` (the files tree's view of every
//! repository at once). Everything else in the slice works on one repository at a time.

use std::sync::Arc;

use futures::StreamExt;
use serde::Serialize;
use serde_json::json;

use super::repo::Repo;
use super::status::{self, GitStatus};
use crate::app::AppState;
use crate::error::ApiError;
use crate::projects::{Project, RepoEntry};

/// Repositories whose state is read at the same time.
const CONCURRENCY: usize = 4;

/// The repository id that, as `?repo=all` of the status, means the whole project.
pub const ALL: &str = "all";

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
}

/// One repository of a project and where it stands (`GET …/git/repos`). A repository that
/// cannot be read (not a repository any more, git refuses it) carries `error` instead of
/// its state, so one broken repository never hides the others.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoInfo {
    pub id: String,
    pub name: String,
    /// The working tree's directory relative to the project root; empty for `.`.
    pub path: String,
    pub default: bool,
    /// `null` when HEAD is detached (or the repository is unreadable).
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Tracked files with changes (untracked files are not counted: that would scan them).
    pub changed: usize,
    pub conflicts: usize,
    /// clean | merging | rebasing | cherry-picking | reverting | bisecting
    pub state: &'static str,
    pub gitlab: Option<serde_json::Value>,
    pub github: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Problem>,
}

fn problem(e: &ApiError) -> Problem {
    Problem { code: e.code, message: e.message.clone() }
}

/// A repository's state without scanning for untracked files (cheap on big trees).
async fn summary(repo: &Repo) -> Result<GitStatus, ApiError> {
    let out = repo
        .git()
        .args(["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=no", "--"])
        .arg(super::cmd::literal(&repo.scope()))
        .run_ok()
        .await?;
    let mut st = status::parse_porcelain_v2(&out.stdout);
    st.state = status::detect_state(&repo.git_dir).0;
    Ok(st)
}

async fn info(state: &AppState, project: &Project, entry: &RepoEntry) -> GitRepoInfo {
    let view = project.scoped(&entry.id);
    let forge = |f: Option<(String, String)>| f.map(|(host, path)| json!({ "host": host, "path": path }));
    let mut out = GitRepoInfo {
        id: entry.id.clone(),
        name: entry.name.clone(),
        path: if entry.is_root() { String::new() } else { entry.id.clone() },
        default: project.repos.first().is_some_and(|d| d.id == entry.id),
        branch: None,
        head: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        changed: 0,
        conflicts: 0,
        state: "clean",
        gitlab: forge(entry.gitlab()),
        github: forge(entry.github()),
        error: None,
    };
    let Some(view) = view else { return out };
    let read = async {
        let repo = state.git.repo(&view).await?;
        summary(&repo).await
    };
    match read.await {
        Ok(st) => {
            out.branch = st.branch;
            out.head = st.head;
            out.upstream = st.upstream;
            out.ahead = st.ahead;
            out.behind = st.behind;
            out.changed = st.files.iter().filter(|f| !matches!(f.index, '?' | '!')).count();
            out.conflicts = st.files.iter().filter(|f| f.conflict).count();
            out.state = st.state;
        }
        Err(e) => out.error = Some(problem(&e)),
    }
    out
}

/// `GET …/git/repos`: every repository of the project, the default one first. A folder in
/// no repository has none.
pub async fn list(state: &AppState, project: &Arc<Project>) -> Vec<GitRepoInfo> {
    let reads: Vec<_> = project.repos.iter().map(|entry| info(state, project, entry)).collect();
    futures::stream::iter(reads).buffered(CONCURRENCY).collect().await
}

async fn one_status(state: &AppState, project: &Project, entry: &RepoEntry, include_ignored: bool) -> Result<(String, GitStatus), ApiError> {
    let view = project.scoped(&entry.id).ok_or_else(|| ApiError::not_found(format!("no repository {:?}", entry.id)))?;
    let repo = state.git.repo(&view).await?;
    Ok((entry.id.clone(), status::status(&repo, include_ignored).await?))
}

/// `GET …/git/status?repo=all`: the default repository's branch, upstream and state, and
/// the changed files of every repository (project-relative paths, each file with its
/// `repo`). A repository that cannot be read is left out; the answer fails only when
/// none can be (with the default repository's error).
pub async fn project_status(state: &AppState, project: &Arc<Project>, include_ignored: bool) -> Result<GitStatus, ApiError> {
    // No repository at all: the answer a plain status gives (not_a_repo).
    if project.repos.is_empty() {
        let repo = state.git.repo(project).await?;
        return status::status(&repo, include_ignored).await;
    }
    let reads: Vec<_> = project.repos.iter().map(|entry| one_status(state, project, entry, include_ignored)).collect();
    let results: Vec<_> = futures::stream::iter(reads).buffered(CONCURRENCY).collect().await;
    let mut results = results.into_iter();
    let first = results.next().expect("a project with repositories has a first one");
    let mut rest: Vec<(String, GitStatus)> = results.filter_map(Result::ok).collect();
    let (mut merged, first_failed) = match first {
        Ok((id, st)) => (Some((id, st)), None),
        Err(e) => (None, Some(e)),
    };
    // The default repository failed: the next one that works speaks for the project.
    if merged.is_none() && !rest.is_empty() {
        merged = Some(rest.remove(0));
    }
    let Some((id, mut head)) = merged else { return Err(first_failed.expect("the first repository failed")) };
    for f in &mut head.files {
        f.repo = Some(id.clone());
    }
    for (id, st) in rest {
        head.truncated |= st.truncated;
        head.files.extend(st.files.into_iter().map(|mut f| {
            f.repo = Some(id.clone());
            f
        }));
    }
    Ok(head)
}

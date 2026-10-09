//! HTTP handlers: `/api/projects/{pid}/git/**`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::changelists::{self, Changelists};
use super::cmd::{check_ref_name, check_rev};
use super::diff::{DiffQuery, GitFileDiff, HunkOp, HunkRequest};
use super::lines::{LineOp, LinesRequest};
use super::log::{LogPage, LogQuery};
use super::ops::{self, OpOutcome};
use super::rebase_i::{self, PlanQuery, RebasePlan};
use super::remote::{self, RemoteOpSpec};
use super::repo::Repo;
use super::shelf::{self, ShelfMeta};
use super::{bisect, blame, conflicts, diff, log, refs, repos, stash, status};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

pub fn router() -> Router<AppState> {
    let r = |p: &str| format!("/api/projects/{{pid}}/git/{p}");
    Router::new()
        .route(&r("status"), get(get_status))
        .route(&r("repos"), get(get_repos))
        .route(&r("diff"), get(get_diff))
        .route(&r("blame"), get(get_blame))
        .route(&r("log"), get(get_log))
        .route(&r("commits/{sha}"), get(get_commit))
        .route(&r("compare"), get(get_compare))
        .route(&r("branches"), get(get_branches).post(post_create_branch))
        .route(&r("branches/rename"), post(post_rename_branch))
        .route(&r("branches/delete"), post(post_delete_branch))
        .route(&r("checkout"), post(post_checkout))
        .route(&r("merge"), post(post_merge))
        .route(&r("rebase"), post(post_rebase))
        .route(&r("continue"), post(post_continue))
        .route(&r("abort"), post(post_abort))
        .route(&r("skip"), post(post_skip))
        .route(&r("cherry-pick"), post(post_cherry_pick))
        .route(&r("revert"), post(post_revert))
        .route(&r("reset"), post(post_reset))
        .route(&r("tags"), post(post_create_tag))
        .route(&r("tags/delete"), post(post_delete_tag))
        .route(&r("stage"), post(post_stage))
        .route(&r("unstage"), post(post_unstage))
        .route(&r("stage-hunks"), post(post_stage_hunks))
        .route(&r("unstage-hunks"), post(post_unstage_hunks))
        .route(&r("discard-hunks"), post(post_discard_hunks))
        .route(&r("stage-lines"), post(post_stage_lines))
        .route(&r("unstage-lines"), post(post_unstage_lines))
        .route(&r("discard-lines"), post(post_discard_lines))
        .route(&r("undo-commit"), post(post_undo_commit))
        .route(&r("rebase/plan"), get(get_rebase_plan))
        .route(&r("rebase/interactive"), post(post_rebase_interactive))
        .route(&r("bisect"), get(get_bisect))
        .route(&r("bisect/start"), post(post_bisect_start))
        .route(&r("bisect/mark"), post(post_bisect_mark))
        .route(&r("bisect/reset"), post(post_bisect_reset))
        .route(&r("changelists"), get(get_changelists).post(post_changelist))
        .route(&r("changelists/move"), post(post_changelists_move))
        .route(&r("changelists/{id}"), axum::routing::patch(patch_changelist).delete(delete_changelist))
        .route(&r("shelf"), get(get_shelves).post(post_shelve))
        .route(&r("shelf/{id}"), get(get_shelf).patch(patch_shelf).delete(delete_shelf))
        .route(&r("shelf/{id}/unshelve"), post(post_unshelve))
        .route(&r("discard"), post(post_discard))
        .route(&r("commit"), post(post_commit))
        .route(&r("last-commit-message"), get(get_last_message))
        .route(&r("stashes"), get(get_stashes).post(post_stash))
        .route(&r("stashes/{index}"), get(get_stash))
        .route(&r("stash/apply"), post(post_stash_apply))
        .route(&r("stash/pop"), post(post_stash_pop))
        .route(&r("stash/drop"), post(post_stash_drop))
        .route(&r("conflict"), get(get_conflict))
        .route(&r("conflict/resolve"), post(post_resolve))
        .route(&r("worktrees"), get(get_worktrees))
        .route(&r("remotes"), get(get_remotes))
        .route(&r("fetch"), post(post_fetch))
        .route(&r("pull"), post(post_pull))
        .route(&r("push"), post(post_push))
        .route(&r("push-preview"), get(get_push_preview))
        .route(&r("remote-branches/delete"), post(post_delete_remote_branch))
        .route(&r("ops"), get(get_ops))
        .route(&r("ops/{op_id}"), get(get_op))
        .route(&r("ops/{op_id}/cancel"), post(post_cancel_op))
}

// ---------------------------------------------------------------- helpers

/// The repository a request is about: `?repo=<id>`; absent or empty selects the
/// project's default repository. Every handler takes one, and `repo_for` / `mutate` need it,
/// so no route can quietly act on the default repository.
#[derive(Debug, Clone, Default)]
pub(super) struct RepoSel(pub Option<String>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for RepoSel {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, ApiError> {
        #[derive(Deserialize)]
        struct Q {
            repo: Option<String>,
        }
        let Query(q) = Query::<Q>::try_from_uri(&parts.uri).map_err(|e| ApiError::bad_request(e.to_string()))?;
        Ok(Self(q.repo.filter(|r| !r.is_empty())))
    }
}

impl RepoSel {
    pub fn id(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// The project seen through the requested repository (404 `unknown_repo`).
pub(super) fn project_for(state: &AppState, pid: &str, rs: &RepoSel) -> Result<Arc<crate::projects::Project>, ApiError> {
    state.projects.require_repo(pid, rs.id())
}

pub(super) async fn repo_for(state: &AppState, pid: &str, rs: &RepoSel) -> Result<Arc<Repo>, ApiError> {
    let p = project_for(state, pid, rs)?;
    state.git.repo(&p).await
}

/// How long a local operation waits for the repository write lock before it
/// answers 409 instead of hanging (a pull still downloading, a commit whose
/// hooks run long).
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// Run a mutation under the repository write lock and announce `git.changed`
/// afterwards (also on failure: a failed merge can still have moved things).
async fn mutate<T, F, Fut>(state: &AppState, pid: &str, rs: &RepoSel, f: F) -> Result<T, ApiError>
where
    F: FnOnce(Arc<Repo>) -> Fut,
    Fut: Future<Output = Result<T, ApiError>>,
{
    mutate_waiting(state, pid, rs, LOCK_WAIT, f).await
}

pub(super) async fn mutate_waiting<T, F, Fut>(state: &AppState, pid: &str, rs: &RepoSel, wait: Duration, f: F) -> Result<T, ApiError>
where
    F: FnOnce(Arc<Repo>) -> Fut,
    Fut: Future<Output = Result<T, ApiError>>,
{
    let repo = repo_for(state, pid, rs).await?;
    let lock = state.git.lock(&repo.top);
    let Ok(guard) = tokio::time::timeout(wait, lock.lock_owned()).await else {
        return Err(busy(state, &repo));
    };
    let result = f(repo.clone()).await;
    drop(guard);
    state.events.emit("git.changed", Some(&repo.project_id), json!({ "repo": repo.id }));
    result
}

/// The repository write lock is taken: say by what, when it is a running update.
fn busy(state: &AppState, repo: &Repo) -> ApiError {
    let msg = match state.git.ops.list(&repo.scope).into_iter().find(|o| !o.done && o.op == "pull") {
        Some(op) => format!("{} is still running in this repository. Wait for it to finish (or cancel it), then try again.", op.title),
        None => "Another git operation is still running in this repository. Try again when it has finished.".to_string(),
    };
    ApiError::new(StatusCode::CONFLICT, "busy", msg)
}

/// Put an automatic Local History label ("Before git checkout main") on the project
/// before an operation that rewrites its working tree, so the versions from before
/// it are easy to find there (CLion's automatic labels). Best effort and cheap: one
/// index line. Operations that run as a `git.op` (pull, interactive rebase) are
/// labelled by the files slice from their first event.
async fn label_before(s: &AppState, pid: &str, rs: &RepoSel, what: String) {
    let text: String = what.chars().filter(|c| !c.is_control()).take(120).collect();
    // The label is on the project; name the repository when it is not the root one.
    let label = match rs.id().filter(|r| *r != crate::projects::ROOT_REPO) {
        Some(repo) => format!("Before {} in {}", text.trim(), repo.chars().filter(|c| !c.is_control()).take(80).collect::<String>()),
        None => format!("Before {}", text.trim()),
    };
    crate::files::history::auto_label(s, pid, &label).await;
}

fn ok() -> Json<Value> {
    Json(json!({ "ok": true }))
}

// ---------------------------------------------------------------- read

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusQuery {
    #[serde(default)]
    ignored: bool,
}

async fn get_repos(State(s): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<Vec<repos::GitRepoInfo>>> {
    let project = s.projects.require(&pid)?;
    Ok(Json(repos::list(&s, &project).await))
}

async fn get_status(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<StatusQuery>) -> ApiResult<Json<status::GitStatus>> {
    // `?repo=all`: every repository's files in one answer (a repository in a folder that is
    // itself called `all` keeps the name).
    if rs.id() == Some(repos::ALL) {
        let project = s.projects.require(&pid)?;
        if !project.repos.iter().any(|r| r.id == repos::ALL) {
            return Ok(Json(repos::project_status(&s, &project, q.ignored).await?));
        }
    }
    let repo = repo_for(&s, &pid, &rs).await?;
    let st = status::status(&repo, q.ignored).await?;
    // New changes join the active changelist when they are first seen.
    if let Err(e) = reconcile_changelists(&s, &repo, &st, |_| ()).await {
        tracing::debug!("git: changelists of {}: {}", repo.scope, e.message);
    }
    Ok(Json(st))
}

/// Changed tracked files (the changelist members) of a status.
fn changed_paths(st: &status::GitStatus) -> Vec<String> {
    st.files.iter().filter(|f| f.index != '?' && f.index != '!').map(|f| f.path.clone()).collect()
}

/// The same, with the HEAD version each change was made against.
fn changed_files(st: &status::GitStatus) -> Vec<changelists::Changed> {
    st.files
        .iter()
        .filter(|f| f.index != '?' && f.index != '!')
        .map(|f| changelists::Changed { path: f.path.clone(), head_blob: f.head_blob.clone() })
        .collect()
}

/// Run `f` on the repository's changelist store (reconciled with `st`), serialized per repository.
async fn reconcile_changelists<T: Send + 'static>(
    s: &AppState,
    repo: &Repo,
    st: &status::GitStatus,
    f: impl FnOnce(&mut changelists::Store) -> T + Send + 'static,
) -> Result<T, ApiError> {
    let changed = changed_files(st);
    let complete = !st.truncated;
    let path = changelists::store_path(&s.paths.data_dir, &repo.scope);
    let lock = s.git.changelist_lock(&repo.scope);
    tokio::task::spawn_blocking(move || {
        let _g = lock.lock();
        changelists::with_store(&path, |store| {
            let changed_now = store.reconcile(&changed, complete, crate::util::now_ms());
            Ok((f(store), changed_now))
        })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

async fn get_diff(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<DiffQuery>) -> ApiResult<Json<GitFileDiff>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(diff::file_diff(&repo, &q).await?.0))
}

#[derive(Deserialize)]
struct BlameQuery {
    path: String,
    rev: Option<String>,
}

async fn get_blame(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<BlameQuery>) -> ApiResult<Json<blame::GitBlame>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(blame::blame(&repo, &q.path, q.rev.as_deref()).await?))
}

async fn get_log(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<LogQuery>) -> ApiResult<Json<LogPage>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(log::log(&repo, &q).await?))
}

async fn get_commit(State(s): State<AppState>, UrlPath((pid, sha)): UrlPath<(String, String)>, rs: RepoSel) -> ApiResult<Json<log::CommitDetails>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(log::commit_details(&repo, &sha).await?))
}

#[derive(Deserialize)]
struct CompareQuery {
    base: String,
    head: String,
}

async fn get_compare(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<CompareQuery>) -> ApiResult<Json<log::Comparison>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(log::compare(&repo, &q.base, &q.head).await?))
}

async fn get_branches(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<refs::Branches>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(refs::branches(&repo).await?))
}

async fn get_last_message(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Value>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(json!({ "message": ops::last_commit_message(&repo).await? })))
}

async fn get_stashes(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Vec<stash::StashEntry>>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(stash::list(&repo).await?))
}

async fn get_stash(State(s): State<AppState>, UrlPath((pid, index)): UrlPath<(String, u32)>, rs: RepoSel) -> ApiResult<Json<stash::StashDetails>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(stash::show(&repo, index).await?))
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

async fn get_conflict(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<PathQuery>) -> ApiResult<Json<conflicts::ConflictVersions>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(conflicts::versions(&repo, &q.path).await?))
}

async fn get_worktrees(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Vec<refs::WorktreeInfo>>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(refs::worktrees(&repo).await?))
}

async fn get_remotes(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Vec<refs::RemoteInfo>>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(refs::remotes(&repo).await?))
}

// ---------------------------------------------------------------- staging & commit

async fn post_stage(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::PathsRequest>) -> ApiResult<Json<Value>> {
    let skipped = mutate(&s, &pid, &rs, |r| async move { ops::stage(&r, &b).await }).await?;
    // `skipped`: submodules whose only change is inside them (git add cannot stage that).
    Ok(Json(json!({ "ok": true, "skipped": skipped })))
}

async fn post_unstage(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::PathsRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { ops::unstage(&r, &b).await }).await?;
    Ok(ok())
}

async fn hunks(s: AppState, pid: String, rs: RepoSel, op: HunkOp, b: HunkRequest) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { diff::apply_hunks(&r, op, &b).await }).await?;
    Ok(ok())
}

async fn post_stage_hunks(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<HunkRequest>) -> ApiResult<Json<Value>> {
    hunks(s, pid, rs, HunkOp::Stage, b).await
}

async fn post_unstage_hunks(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<HunkRequest>) -> ApiResult<Json<Value>> {
    hunks(s, pid, rs, HunkOp::Unstage, b).await
}

async fn post_discard_hunks(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<HunkRequest>) -> ApiResult<Json<Value>> {
    label_before(&s, &pid, &rs, "rollback".into()).await;
    hunks(s, pid, rs, HunkOp::Discard, b).await
}

async fn lines(s: AppState, pid: String, rs: RepoSel, op: LineOp, b: LinesRequest) -> ApiResult<Json<Value>> {
    let trash = s.paths.data("trash");
    mutate(&s, &pid, &rs, |r| async move { super::lines::apply_lines(&r, op, &b, &trash).await }).await?;
    Ok(ok())
}

async fn post_stage_lines(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<LinesRequest>) -> ApiResult<Json<Value>> {
    lines(s, pid, rs, LineOp::Stage, b).await
}

async fn post_unstage_lines(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<LinesRequest>) -> ApiResult<Json<Value>> {
    lines(s, pid, rs, LineOp::Unstage, b).await
}

async fn post_discard_lines(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<LinesRequest>) -> ApiResult<Json<Value>> {
    label_before(&s, &pid, &rs, "rollback".into()).await;
    lines(s, pid, rs, LineOp::Discard, b).await
}

async fn post_undo_commit(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::UndoCommitRequest>) -> ApiResult<Json<Value>> {
    let message = mutate(&s, &pid, &rs, |r| async move { ops::undo_commit(&r, &b).await }).await?;
    Ok(Json(json!({ "ok": true, "message": message })))
}

async fn post_discard(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::DiscardRequest>) -> ApiResult<Json<ops::DiscardResult>> {
    let trash = s.paths.data("trash");
    let paths = b.paths.clone();
    label_before(&s, &pid, &rs, "rollback".into()).await;
    let res = mutate(&s, &pid, &rs, |r| async move { ops::discard(&r, &b, &trash).await }).await?;
    // Rolled back in Workbench: the files leave their changelists (a later change is new).
    match current_status(&s, &pid, &rs).await {
        Ok((repo, st)) => {
            if let Err(e) = reconcile_changelists(&s, &repo, &st, move |store| store.forget_away(&paths)).await {
                tracing::debug!("git: changelists after rollback: {}", e.message);
            }
        }
        Err(e) => tracing::debug!("git: changelists after rollback: {}", e.message),
    }
    Ok(Json(res))
}

async fn post_commit(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::CommitRequest>) -> ApiResult<Json<ops::CommitResult>> {
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::commit(&r, &b).await }).await?))
}

// ---------------------------------------------------------------- branches & history

async fn post_checkout(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::CheckoutRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, format!("git checkout {}", b.create.as_deref().or(b.rev.as_deref()).unwrap_or(""))).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::checkout(&r, &b).await }).await?))
}

async fn post_create_branch(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::CreateBranchRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { ops::create_branch(&r, &b).await }).await?;
    Ok(ok())
}

async fn post_rename_branch(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::RenameBranchRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { ops::rename_branch(&r, &b).await }).await?;
    Ok(ok())
}

async fn post_delete_branch(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::DeleteBranchRequest>) -> ApiResult<Json<Value>> {
    match mutate(&s, &pid, &rs, |r| async move { ops::delete_branch(&r, &b).await }).await? {
        None => Ok(ok()),
        Some(message) => Ok(Json(json!({ "ok": false, "notMerged": true, "message": message }))),
    }
}

async fn post_merge(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::MergeRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, "git merge".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::merge(&r, &b).await }).await?))
}

async fn post_rebase(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::RebaseRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, "git rebase".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::rebase(&r, &b).await }).await?))
}

async fn sequencer(s: AppState, pid: String, rs: RepoSel, action: &'static str) -> ApiResult<Json<OpOutcome>> {
    if action == "abort" {
        label_before(&s, &pid, &rs, "abort".into()).await;
    }
    let program = s.git.editor.get().cloned();
    Ok(Json(
        mutate(&s, &pid, &rs, |r| async move {
            // A rebase started by the interactive rebase dialog keeps using its message editor.
            let (pre, env) = rebase_i::continue_env(&r.git_dir, program.as_deref());
            ops::sequencer(&r, action, &pre, &env).await
        })
        .await?,
    ))
}

// ---------------------------------------------------------------- interactive rebase

async fn get_rebase_plan(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<PlanQuery>) -> ApiResult<Json<RebasePlan>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(rebase_i::plan(&repo, &q).await?))
}

async fn post_rebase_interactive(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<rebase_i::RunRequest>) -> ApiResult<impl IntoResponse> {
    let repo = repo_for(&s, &pid, &rs).await?;
    let id = rebase_i::start(&s, repo, &b).await?;
    Ok(accepted(id))
}

// ---------------------------------------------------------------- bisect

async fn get_bisect(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<bisect::BisectState>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    Ok(Json(bisect::state(&repo).await?))
}

async fn post_bisect_start(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<bisect::StartRequest>) -> ApiResult<Json<bisect::BisectOutcome>> {
    label_before(&s, &pid, &rs, "git bisect".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { bisect::start(&r, &b).await }).await?))
}

async fn post_bisect_mark(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<bisect::MarkRequest>) -> ApiResult<Json<bisect::BisectOutcome>> {
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { bisect::mark(&r, &b).await }).await?))
}

async fn post_bisect_reset(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<bisect::BisectOutcome>> {
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { bisect::reset(&r).await }).await?))
}

// ---------------------------------------------------------------- changelists

fn changelists_changed(s: &AppState, repo: &Repo) {
    s.events.emit("git.changelists", Some(&repo.project_id), json!({ "repo": repo.id }));
}

async fn current_status(s: &AppState, pid: &str, rs: &RepoSel) -> Result<(Arc<Repo>, status::GitStatus), ApiError> {
    let repo = repo_for(s, pid, rs).await?;
    let st = status::status(&repo, false).await?;
    Ok((repo, st))
}

async fn get_changelists(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Changelists>> {
    let (repo, st) = current_status(&s, &pid, &rs).await?;
    let changed = changed_paths(&st);
    Ok(Json(reconcile_changelists(&s, &repo, &st, move |store| store.view(&changed)).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewChangelist {
    name: String,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    active: bool,
    /// Move these files into the new list.
    #[serde(default)]
    paths: Vec<String>,
}

async fn post_changelist(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<NewChangelist>) -> ApiResult<Json<Value>> {
    let (repo, st) = current_status(&s, &pid, &rs).await?;
    for p in &b.paths {
        repo.to_repo(p)?;
    }
    let id = reconcile_changelists(&s, &repo, &st, move |store| {
        let id = store.create(&b.name, &b.comment, b.active)?;
        if !b.paths.is_empty() {
            store.move_files(&b.paths, &id)?;
        }
        Ok::<_, ApiError>(id)
    })
    .await??;
    changelists_changed(&s, &repo);
    Ok(Json(json!({ "ok": true, "id": id })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchChangelist {
    name: Option<String>,
    comment: Option<String>,
    /// Make it the active list.
    #[serde(default)]
    active: bool,
}

async fn patch_changelist(State(s): State<AppState>, UrlPath((pid, id)): UrlPath<(String, String)>, rs: RepoSel, Json(b): Json<PatchChangelist>) -> ApiResult<Json<Value>> {
    let (repo, st) = current_status(&s, &pid, &rs).await?;
    reconcile_changelists(&s, &repo, &st, move |store| store.update(&id, b.name.as_deref(), b.comment.as_deref(), b.active)).await??;
    changelists_changed(&s, &repo);
    Ok(ok())
}

async fn delete_changelist(State(s): State<AppState>, UrlPath((pid, id)): UrlPath<(String, String)>, rs: RepoSel) -> ApiResult<Json<Value>> {
    let (repo, st) = current_status(&s, &pid, &rs).await?;
    reconcile_changelists(&s, &repo, &st, move |store| store.delete(&id)).await??;
    changelists_changed(&s, &repo);
    Ok(ok())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveBody {
    paths: Vec<String>,
    to: String,
}

async fn post_changelists_move(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<MoveBody>) -> ApiResult<Json<Value>> {
    if b.paths.is_empty() || b.paths.len() > 10_000 {
        return Err(ApiError::bad_request("give 1 to 10000 paths"));
    }
    let repo = repo_for(&s, &pid, &rs).await?;
    for p in &b.paths {
        repo.to_repo(p)?;
    }
    let st = status::status(&repo, false).await?;
    reconcile_changelists(&s, &repo, &st, move |store| store.move_files(&b.paths, &b.to)).await??;
    changelists_changed(&s, &repo);
    Ok(ok())
}

/// The files of a changelist, as the status shows them now.
async fn changelist_files(s: &AppState, repo: &Repo, id: &str) -> Result<(String, Vec<String>), ApiError> {
    let st = status::status(repo, false).await?;
    let changed = changed_paths(&st);
    let view = reconcile_changelists(s, repo, &st, move |store| store.view(&changed)).await?;
    let l = view.lists.into_iter().find(|l| l.id == id).ok_or_else(|| ApiError::not_found(format!("no changelist {id}")))?;
    Ok((l.name, l.files))
}

// ---------------------------------------------------------------- shelf

fn shelf_changed(s: &AppState, project_id: &str, repo_id: &str) {
    s.events.emit("git.shelf", Some(project_id), json!({ "repo": repo_id }));
}

fn shelf_root(s: &AppState, scope: &str) -> std::path::PathBuf {
    shelf::shelf_root(&s.paths.data_dir, scope)
}

async fn get_shelves(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Vec<ShelfMeta>>> {
    let root = shelf_root(&s, &project_for(&s, &pid, &rs)?.scope_key());
    Ok(Json(tokio::task::spawn_blocking(move || shelf::list(&root)).await.map_err(|e| ApiError::internal(e.to_string()))?))
}

async fn get_shelf(State(s): State<AppState>, UrlPath((pid, id)): UrlPath<(String, String)>, rs: RepoSel) -> ApiResult<Json<ShelfMeta>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    let root = shelf_root(&s, &repo.scope);
    let lock = s.git.shelf_lock(&repo.scope);
    let _g = lock.lock().await;
    Ok(Json(shelf::view_commit(&repo, &root, &id).await?))
}

async fn post_shelve(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<shelf::ShelveRequest>) -> ApiResult<Json<ShelfMeta>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    let mut paths = b.paths.clone();
    let mut name = b.name.clone();
    if let Some(cl) = b.changelist.as_deref().filter(|c| !c.is_empty()) {
        let (list_name, files) = changelist_files(&s, &repo, cl).await?;
        if files.is_empty() {
            return Err(ApiError::bad_request(format!("the changelist {list_name:?} has no changes")));
        }
        paths.extend(files);
        if name.trim().is_empty() {
            name = list_name;
        }
    }
    // The changelist of each file, remembered on the shelf (unshelving puts them back there).
    let lists = match status::status(&repo, false).await {
        Ok(st) => {
            let p2 = paths.clone();
            reconcile_changelists(&s, &repo, &st, move |store| store.lists_of(&p2)).await.unwrap_or_default()
        }
        Err(_) => Default::default(),
    };
    let root = shelf_root(&s, &repo.scope);
    let lock = s.git.shelf_lock(&repo.scope);
    let _g = lock.lock().await;
    let root2 = root.clone();
    if !b.keep {
        label_before(&s, &pid, &rs, format!("shelve {name}")).await;
    }
    let mut meta = mutate(&s, &pid, &rs, |r| async move { shelf::shelve(&r, &root2, &name, &paths, b.keep).await }).await?;
    if !lists.is_empty() {
        let (root3, id) = (root.clone(), meta.id.clone());
        match tokio::task::spawn_blocking(move || shelf::record_changelists(&root3, &id, &lists)).await {
            Ok(Ok(m)) => meta = m,
            Ok(Err(e)) => tracing::debug!("git: shelf changelists: {}", e.message),
            Err(e) => tracing::debug!("git: shelf changelists: {e}"),
        }
    }
    shelf_changed(&s, &repo.project_id, &repo.id);
    Ok(Json(meta))
}

#[derive(Deserialize)]
struct RenameShelf {
    name: String,
}

async fn patch_shelf(State(s): State<AppState>, UrlPath((pid, id)): UrlPath<(String, String)>, rs: RepoSel, Json(b): Json<RenameShelf>) -> ApiResult<Json<ShelfMeta>> {
    let p = project_for(&s, &pid, &rs)?;
    let (scope, repo_id) = (p.scope_key(), p.repo_id().to_string());
    let root = shelf_root(&s, &scope);
    let lock = s.git.shelf_lock(&scope);
    let _g = lock.lock().await;
    let meta = tokio::task::spawn_blocking(move || shelf::rename(&root, &id, &b.name)).await.map_err(|e| ApiError::internal(e.to_string()))??;
    shelf_changed(&s, &p.id, &repo_id);
    Ok(Json(meta))
}

async fn delete_shelf(State(s): State<AppState>, UrlPath((pid, id)): UrlPath<(String, String)>, rs: RepoSel) -> ApiResult<Json<Value>> {
    let p = project_for(&s, &pid, &rs)?;
    let (scope, repo_id) = (p.scope_key(), p.repo_id().to_string());
    let root = shelf_root(&s, &scope);
    let lock = s.git.shelf_lock(&scope);
    let _g = lock.lock().await;
    tokio::task::spawn_blocking(move || shelf::delete(&root, &id)).await.map_err(|e| ApiError::internal(e.to_string()))??;
    shelf_changed(&s, &p.id, &repo_id);
    Ok(ok())
}

async fn post_unshelve(State(s): State<AppState>, UrlPath((pid, id)): UrlPath<(String, String)>, rs: RepoSel, Json(b): Json<shelf::UnshelveRequest>) -> ApiResult<Json<shelf::UnshelveResult>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    let root = shelf_root(&s, &repo.scope);
    let lock = s.git.shelf_lock(&repo.scope);
    let _g = lock.lock().await;
    let cl = b.changelist.clone().filter(|c| !c.is_empty());
    // Where each file was shelved from (used when no changelist is chosen).
    let recorded: std::collections::BTreeMap<String, String> = {
        let (r, i) = (root.clone(), id.clone());
        match tokio::task::spawn_blocking(move || shelf::load(&r, &i)).await {
            Ok(Ok(m)) => m.files.into_iter().filter_map(|f| f.changelist.map(|c| (f.path, c))).collect(),
            _ => Default::default(),
        }
    };
    label_before(&s, &pid, &rs, "unshelve".into()).await;
    let res = mutate(&s, &pid, &rs, |r| async move { shelf::unshelve(&r, &root, &id, &b).await }).await?;
    shelf_changed(&s, &repo.project_id, &repo.id);
    if !res.applied.is_empty() && (cl.is_some() || !recorded.is_empty()) {
        let st = status::status(&repo, false).await?;
        let applied = res.applied.clone();
        let moved = reconcile_changelists(&s, &repo, &st, move |store| match cl {
            Some(cl) => store.move_files(&applied, &cl),
            None => {
                // Back into the list each file came from, when that list still exists.
                for p in &applied {
                    if let Some(l) = recorded.get(p) {
                        let _ = store.move_files(std::slice::from_ref(p), l);
                    }
                }
                Ok(())
            }
        })
        .await
        .and_then(|r| r);
        if let Err(e) = moved {
            tracing::debug!("git: unshelve into changelist: {}", e.message);
        }
        changelists_changed(&s, &repo);
    }
    Ok(Json(res))
}

async fn post_continue(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<OpOutcome>> {
    sequencer(s, pid, rs, "continue").await
}

async fn post_abort(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<OpOutcome>> {
    sequencer(s, pid, rs, "abort").await
}

async fn post_skip(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<OpOutcome>> {
    sequencer(s, pid, rs, "skip").await
}

async fn post_cherry_pick(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::ShasRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, "git cherry-pick".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::cherry_pick(&r, &b).await }).await?))
}

async fn post_revert(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::ShasRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, "git revert".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::revert(&r, &b).await }).await?))
}

async fn post_reset(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::ResetRequest>) -> ApiResult<Json<Value>> {
    // Only these modes touch the working tree.
    if matches!(b.mode.as_str(), "hard" | "keep") {
        label_before(&s, &pid, &rs, format!("git reset --{} {}", b.mode, b.rev)).await;
    }
    mutate(&s, &pid, &rs, |r| async move { ops::reset(&r, &b).await }).await?;
    Ok(ok())
}

async fn post_create_tag(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::TagRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { ops::create_tag(&r, &b).await }).await?;
    Ok(ok())
}

async fn post_delete_tag(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::NameRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { ops::delete_tag(&r, &b.name).await }).await?;
    Ok(ok())
}

// ---------------------------------------------------------------- stash & conflicts

async fn post_stash(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::StashPushRequest>) -> ApiResult<Json<Value>> {
    label_before(&s, &pid, &rs, "git stash".into()).await;
    mutate(&s, &pid, &rs, |r| async move { ops::stash_push(&r, &b).await }).await?;
    Ok(ok())
}

async fn post_stash_apply(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::StashRefRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, "git stash apply".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::stash_apply(&r, &b, false).await }).await?))
}

async fn post_stash_pop(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::StashRefRequest>) -> ApiResult<Json<OpOutcome>> {
    label_before(&s, &pid, &rs, "git stash pop".into()).await;
    Ok(Json(mutate(&s, &pid, &rs, |r| async move { ops::stash_apply(&r, &b, true).await }).await?))
}

async fn post_stash_drop(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<ops::StashRefRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { ops::stash_drop(&r, &b).await }).await?;
    Ok(ok())
}

async fn post_resolve(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<conflicts::ResolveRequest>) -> ApiResult<Json<Value>> {
    mutate(&s, &pid, &rs, |r| async move { conflicts::resolve(&r, &b).await }).await?;
    Ok(ok())
}

// ---------------------------------------------------------------- remote operations

fn accepted(op_id: String) -> impl IntoResponse {
    (StatusCode::ACCEPTED, Json(json!({ "opId": op_id })))
}

async fn remote_names(repo: &Repo) -> Result<Vec<String>, ApiError> {
    let out = repo.git().arg("remote").run_ok().await?;
    Ok(out.text().lines().map(str::to_string).filter(|s| !s.is_empty()).collect())
}

async fn check_remote(repo: &Repo, name: &str) -> Result<(), ApiError> {
    if remote_names(repo).await?.iter().any(|r| r == name) {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!("no remote named {name:?}")))
    }
}

async fn config_get(repo: &Repo, key: &str) -> Result<Option<String>, ApiError> {
    let out = repo.git().args(["config", "--get", key]).run().await?;
    Ok(out.ok().then(|| out.text().trim().to_string()).filter(|s| !s.is_empty()))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FetchBody {
    op_id: Option<String>,
    remote: Option<String>,
    #[serde(default)]
    prune: bool,
    #[serde(default)]
    tags: bool,
}

async fn post_fetch(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, body: Option<Json<FetchBody>>) -> ApiResult<impl IntoResponse> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let repo = repo_for(&s, &pid, &rs).await?;
    let mut args = vec!["fetch".to_string(), "--progress".into()];
    if b.prune {
        args.push("--prune".into());
    }
    if b.tags {
        args.push("--tags".into());
    }
    let title = match b.remote.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => {
            check_remote(&repo, r).await?;
            args.push(r.to_string());
            format!("Fetch {r}")
        }
        None => {
            if remote_names(&repo).await?.is_empty() {
                return Err(ApiError::not_configured("this repository has no remote"));
            }
            args.push("--all".into());
            "Fetch".into()
        }
    };
    let id = remote::start(&s, repo, RemoteOpSpec::new("fetch", title, args, false), b.op_id.as_deref())?;
    Ok(accepted(id))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PullBody {
    op_id: Option<String>,
    /// true = rebase, false = merge, absent = the repository's `pull.rebase` (merge if unset).
    rebase: Option<bool>,
    #[serde(default)]
    autostash: bool,
    remote: Option<String>,
    branch: Option<String>,
}

async fn post_pull(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, body: Option<Json<PullBody>>) -> ApiResult<impl IntoResponse> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let repo = repo_for(&s, &pid, &rs).await?;
    let branch = crate::util::git::current_branch(&repo.top)
        .await
        .ok_or_else(|| ApiError::bad_request("HEAD is detached: check out a branch to update it"))?;
    let mut args = vec!["pull".to_string(), "--progress".into()];
    let rebase = match b.rebase {
        Some(r) => r,
        None => config_get(&repo, "pull.rebase").await?.is_some_and(|v| v != "false"),
    };
    args.push(if rebase { "--rebase".into() } else { "--no-rebase".into() });
    if b.autostash {
        args.push("--autostash".into());
    }
    match (b.remote.as_deref().filter(|r| !r.is_empty()), b.branch.as_deref().filter(|r| !r.is_empty())) {
        (Some(r), br) => {
            check_remote(&repo, r).await?;
            args.push(r.to_string());
            if let Some(br) = br {
                args.push(check_rev(br)?.to_string());
            }
        }
        (None, _) => {
            if config_get(&repo, &format!("branch.{branch}.merge")).await?.is_none() {
                return Err(ApiError::bad_request(format!(
                    "{branch} has no upstream branch: push it with an upstream first, or choose a remote branch to pull"
                )));
            }
        }
    }
    let title = format!("Update {branch} ({})", if rebase { "rebase" } else { "merge" });
    let id = remote::start(&s, repo, RemoteOpSpec::new("pull", title, args, true), b.op_id.as_deref())?;
    Ok(accepted(id))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PushBody {
    op_id: Option<String>,
    remote: Option<String>,
    /// Remote branch name (default: the upstream's, else the local name).
    branch: Option<String>,
    set_upstream: Option<bool>,
    #[serde(default)]
    force_with_lease: bool,
    /// Push annotated tags reachable from the pushed commits.
    #[serde(default)]
    follow_tags: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PushTarget {
    local_branch: String,
    remote: String,
    remote_branch: String,
    /// The branch has an upstream configured.
    has_upstream: bool,
    /// `remote/remoteBranch` exists locally (after the last fetch).
    remote_exists: bool,
    remotes: Vec<String>,
}

async fn push_target(repo: &Repo, remote: Option<&str>, branch: Option<&str>) -> Result<PushTarget, ApiError> {
    let local = crate::util::git::current_branch(&repo.top)
        .await
        .ok_or_else(|| ApiError::bad_request("HEAD is detached: check out a branch to push"))?;
    let remotes = remote_names(repo).await?;
    if remotes.is_empty() {
        return Err(ApiError::not_configured("this repository has no remote to push to"));
    }
    let cfg_remote = config_get(repo, &format!("branch.{local}.remote")).await?;
    let cfg_merge = config_get(repo, &format!("branch.{local}.merge")).await?;
    let remote = match remote.filter(|r| !r.is_empty()) {
        Some(r) => r.to_string(),
        None => cfg_remote
            .clone()
            .filter(|r| remotes.contains(r))
            .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
            .unwrap_or_else(|| remotes[0].clone()),
    };
    if !remotes.contains(&remote) {
        return Err(ApiError::bad_request(format!("no remote named {remote:?}")));
    }
    let remote_branch = match branch.filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => match (&cfg_remote, &cfg_merge) {
            (Some(r), Some(m)) if r == &remote => m.strip_prefix("refs/heads/").unwrap_or(m).to_string(),
            _ => local.clone(),
        },
    };
    check_ref_name(&repo.top, &remote_branch, "branch").await?;
    let remote_exists = repo
        .git()
        .args(["show-ref", "--verify", "--quiet", &format!("refs/remotes/{remote}/{remote_branch}")])
        .run()
        .await?
        .ok();
    Ok(PushTarget { local_branch: local, remote, remote_branch, has_upstream: cfg_merge.is_some(), remote_exists, remotes })
}

#[derive(Deserialize)]
struct PushPreviewQuery {
    remote: Option<String>,
    branch: Option<String>,
}

/// What a push would send: the target and the outgoing commits.
async fn get_push_preview(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Query(q): Query<PushPreviewQuery>) -> ApiResult<Json<Value>> {
    let repo = repo_for(&s, &pid, &rs).await?;
    let t = push_target(&repo, q.remote.as_deref(), q.branch.as_deref()).await?;
    let lq = if t.remote_exists {
        LogQuery { rev: Some(format!("{}/{}..HEAD", t.remote, t.remote_branch)), limit: Some(500), ..Default::default() }
    } else {
        LogQuery { rev: Some("HEAD".into()), limit: Some(500), ..Default::default() }
    };
    let mut page = log::log(&repo, &lq).await?;
    if !t.remote_exists {
        // A new branch: commits not yet on any branch of that remote.
        let out = repo
            .git()
            .args(["rev-list", "--max-count=501", "HEAD", "--not", &format!("--remotes={}", t.remote)])
            .run_ok()
            .await?;
        let outgoing: std::collections::HashSet<String> = out.text().lines().map(str::to_string).collect();
        page.commits.retain(|c| outgoing.contains(&c.sha));
        page.has_more = outgoing.len() > 500;
    }
    Ok(Json(json!({ "target": t, "commits": page.commits, "hasMore": page.has_more })))
}

async fn post_push(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, body: Option<Json<PushBody>>) -> ApiResult<impl IntoResponse> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let repo = repo_for(&s, &pid, &rs).await?;
    let t = push_target(&repo, b.remote.as_deref(), b.branch.as_deref()).await?;
    let mut args = vec!["push".to_string(), "--progress".into()];
    if b.force_with_lease {
        args.push("--force-with-lease".into());
    }
    if b.set_upstream.unwrap_or(!t.has_upstream) {
        args.push("--set-upstream".into());
    }
    if b.follow_tags {
        args.push("--follow-tags".into());
    }
    args.push(t.remote.clone());
    args.push(format!("refs/heads/{}:refs/heads/{}", t.local_branch, t.remote_branch));
    let title = format!(
        "Push {} → {}/{}{}",
        t.local_branch,
        t.remote,
        t.remote_branch,
        if b.force_with_lease { " (force)" } else { "" }
    );
    let id = remote::start(&s, repo, RemoteOpSpec::new("push", title, args, false), b.op_id.as_deref())?;
    Ok(accepted(id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRemoteBranchBody {
    op_id: Option<String>,
    remote: String,
    branch: String,
}

async fn post_delete_remote_branch(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel, Json(b): Json<DeleteRemoteBranchBody>) -> ApiResult<impl IntoResponse> {
    let repo = repo_for(&s, &pid, &rs).await?;
    check_remote(&repo, &b.remote).await?;
    check_ref_name(&repo.top, &b.branch, "branch").await?;
    let args = vec!["push".into(), "--progress".into(), b.remote.clone(), "--delete".into(), format!("refs/heads/{}", b.branch)];
    let title = format!("Delete {}/{}", b.remote, b.branch);
    let id = remote::start(&s, repo, RemoteOpSpec::new("delete-remote-branch", title, args, false), b.op_id.as_deref())?;
    Ok(accepted(id))
}

async fn get_ops(State(s): State<AppState>, UrlPath(pid): UrlPath<String>, rs: RepoSel) -> ApiResult<Json<Vec<remote::OpInfo>>> {
    Ok(Json(s.git.ops.list(&project_for(&s, &pid, &rs)?.scope_key())))
}

async fn get_op(State(s): State<AppState>, UrlPath((pid, op_id)): UrlPath<(String, String)>, rs: RepoSel) -> ApiResult<Json<remote::OpInfo>> {
    // An operation is found by its (random) id within the project, whichever repository it ran in.
    project_for(&s, &pid, &rs)?;
    match s.git.ops.get(&op_id) {
        Some(op) if op.project_id == pid => Ok(Json(op)),
        _ => Err(ApiError::not_found(format!("no operation {op_id}"))),
    }
}

async fn post_cancel_op(State(s): State<AppState>, UrlPath((pid, op_id)): UrlPath<(String, String)>, rs: RepoSel) -> ApiResult<Json<Value>> {
    project_for(&s, &pid, &rs)?;
    match s.git.ops.get(&op_id) {
        Some(op) if op.project_id == pid => Ok(Json(json!({ "ok": s.git.ops.cancel(&op_id) }))),
        _ => Err(ApiError::not_found(format!("no operation {op_id}"))),
    }
}

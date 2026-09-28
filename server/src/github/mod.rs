//! GitHub slice (OWNER: github slice).
//!
//! Pull requests (list, diffs, reviews/comments, approve, merge, create), Actions
//! workflow runs and jobs (logs, re-run, cancel, dispatch), issues, releases.
//! Routes: `/api/projects/{pid}/github/**`, `/api/github/**`.
//!
//! CONTRACT: `commit_ci_status` (called through `forge::commit_ci_status`).
//!
//! Layout:
//! * `client`: connection per project (API base, token or anonymous), cached
//!   GETs with ETags, pagination, rate limits, redirects, safe error mapping;
//! * `model`: GitHub JSON (snake_case in, camelCase out) and pure helpers,
//!   including the mapping onto the shared CI vocabulary;
//! * `logs`: job log timestamp stripping, step positions, plain text;
//! * `ci`, `pulls`, `misc`: operations plus their REST handlers;
//! * `poller`: background Actions watcher (`github.run` events);
//! * `tools`: MCP tools for hosted agents.
//!
//! Events (all with `projectId`): `github.run {runId, workflowId, name, state,
//! status, conclusion, branch, sha, event, runNumber, webUrl, action,
//! previousState?}`, `github.job {jobId, runId, action}`, `github.pr {number,
//! action}` and `github.issue {number, action}`.

mod ci;
mod client;
mod logs;
mod misc;
mod model;
mod poller;
mod pulls;
mod tools;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Router;
use serde_json::json;

pub use client::GithubState;
use client::GhCtx;

use crate::app::AppState;
use crate::error::ApiError;
pub use crate::forge::CiStatus;
use crate::mcp::McpTool;
use crate::projects::Project;
use model::Run;

/// Combined status of `sha` on GitHub (check runs, workflow runs and legacy
/// commit statuses), in the GitLab vocabulary (`success`, `failed`, `running`,
/// `pending`, `canceled`, `skipped`, `manual`), with `pipeline_id` the workflow
/// run that explains it. `Ok(None)` when nothing ran for the commit, the commit
/// is unknown to GitHub, or the project is not on GitHub.
pub async fn commit_ci_status(state: &AppState, project: &Project, sha: &str) -> Result<Option<CiStatus>, ApiError> {
    if project.github().is_none() {
        return Ok(None);
    }
    let sha = ci::valid_sha(sha)?.to_string();
    let project = state.projects.get(&project.id).unwrap_or_else(|| Arc::new(project.clone()));
    let ctx = client::ctx_for(state, project).await?;
    ci::ci_status_for(&ctx, &sha).await
}

/// Emit `github.run` for a run.
fn emit_run(state: &AppState, project_id: &str, r: &Run, action: &str, previous: Option<&str>) {
    state.events.emit(
        "github.run",
        Some(project_id),
        json!({ "runId": r.id, "workflowId": r.workflow_id, "name": r.name, "state": r.state, "status": r.status,
                "conclusion": r.conclusion, "branch": r.head_branch, "sha": r.head_sha, "event": r.event,
                "runNumber": r.run_number, "webUrl": r.html_url, "action": action, "previousState": previous }),
    );
}

/// A run changed because of an action here: tell the UI, remember it for the
/// poller, and poll this project quickly for a while.
fn run_changed(ctx: &GhCtx, r: &Run, action: &str) {
    let state = &ctx.state;
    state.github.invalidate_summary(&ctx.project.id);
    if let Some(b) = &r.head_branch {
        state.github.poll.note(&ctx.project.id, b, r.id, &r.state);
    }
    state.github.poll.mark_hot(&ctx.project.id);
    emit_run(state, &ctx.project.id, r, action, None);
}

/// A pull request changed because of an action here.
fn pr_changed(ctx: &GhCtx, number: u64, action: &str) {
    ctx.state.github.invalidate_summary(&ctx.project.id);
    ctx.state.events.emit("github.pr", Some(&ctx.project.id), json!({ "number": number, "action": action }));
}

/// An issue changed because of an action here.
fn issue_changed(ctx: &GhCtx, number: u64, action: &str) {
    ctx.state.github.invalidate_summary(&ctx.project.id);
    ctx.state.events.emit("github.issue", Some(&ctx.project.id), json!({ "number": number, "action": action }));
}

pub fn router() -> Router<AppState> {
    Router::new().merge(ci::routes()).merge(pulls::routes()).merge(misc::routes())
}

pub async fn start(state: &AppState) {
    poller::spawn(state.clone());
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::mcp_tools()
}

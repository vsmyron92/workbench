//! GitLab slice (OWNER: gitlab slice).
//!
//! Merge requests (list, diffs, discussions, approve, merge, create), pipelines and
//! jobs (logs, retry, cancel, play), environments/deployments, issues, registry.
//! Routes: `/api/projects/{pid}/gitlab/**`, `/api/gitlab/**`.
//!
//! CONTRACT: `commit_ci_status` (used by the apps slice to gate deploys).
//!
//! Layout:
//! * `client`: connection per project (token, numeric id, API base), pagination,
//!   rate limits, redirects and safe error mapping;
//! * `model`: GitLab JSON (snake_case in, camelCase out) and small pure helpers;
//! * `trace`: job log prefix stripping, incremental offsets, plain text;
//! * `pipelines`, `mrs`, `misc`: operations plus their REST handlers;
//! * `poller`: background pipeline watcher (`gitlab.pipeline` events);
//! * `tools`: MCP tools for hosted agents.
//!
//! Events (all with `projectId`, and `repo`, the id of the repository of the project
//! they are about): `gitlab.pipeline {pipelineId, status, ref, sha, iid, webUrl,
//! previousStatus?, repo}` (contract fields plus extras), plus `gitlab.mr {iid, action,
//! repo}`, `gitlab.issue {iid, action, repo}` and `gitlab.job {jobId, previousJobId,
//! action, status, pipelineId, repo}` for open views.
//!
//! A project may have several repositories, each on its own GitLab project: every route
//! and tool works on the one `?repo=` (the `repo` input of a tool) names, the default
//! repository without it (`forge::RepoParam`). Caches, polling state and summaries are
//! kept per repository (`Project::scope_key`).

mod client;
mod misc;
mod model;
mod mrs;
mod pipelines;
mod poller;
mod tools;
mod trace;

#[cfg(test)]
mod tests;

use axum::Router;
use serde::Serialize;
use serde_json::json;

pub use client::GitlabState;
use client::GlCtx;

use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::McpTool;
use crate::projects::Project;

/// CI state of one commit.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiStatus {
    /// GitLab pipeline status: success | failed | running | pending | canceled | skipped | manual | …
    pub status: String,
    pub pipeline_id: Option<u64>,
    pub web_url: Option<String>,
    /// Full sha of the commit (the input may have been short).
    pub sha: Option<String>,
    /// Ref the pipeline ran for.
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
}

/// Status of the latest pipeline for `sha` (full or short). `Ok(None)` when the
/// commit has no pipeline or the project is not on GitLab.
pub async fn commit_ci_status(state: &AppState, project: &Project, sha: &str) -> Result<Option<CiStatus>, ApiError> {
    if project.gitlab().is_none() {
        return Ok(None);
    }
    let sha = pipelines::valid_sha(sha)?.to_string();
    let project = crate::forge::shared(state, project);
    let ctx = client::ctx_for(state, project).await?;
    ci_status_for(&ctx, &sha).await
}

/// `GET /projects/:id/repository/commits/:sha` → `last_pipeline` (resolves short shas).
async fn ci_status_for(ctx: &GlCtx, sha: &str) -> Result<Option<CiStatus>, ApiError> {
    let url = ctx.purl(&format!("/repository/commits/{sha}"));
    let commit: Option<model::Commit> = ctx.get_opt(&url, &[]).await?;
    Ok(commit.and_then(|c| {
        c.last_pipeline.map(|p| CiStatus {
            status: p.status,
            pipeline_id: Some(p.id),
            web_url: Some(p.web_url).filter(|u| !u.is_empty()),
            sha: Some(c.id),
            git_ref: Some(p.git_ref).filter(|r| !r.is_empty()),
        })
    }))
}

/// Emit `gitlab.pipeline` for a repository of a project.
#[allow(clippy::too_many_arguments)]
fn emit_pipeline(
    state: &AppState,
    project: &Project,
    id: u64,
    iid: Option<u64>,
    status: &str,
    git_ref: &str,
    sha: &str,
    web_url: &str,
    previous: Option<&str>,
) {
    state.events.emit(
        "gitlab.pipeline",
        Some(&project.id),
        json!({ "pipelineId": id, "iid": iid, "status": status, "ref": git_ref, "sha": sha,
                "webUrl": web_url, "previousStatus": previous, "repo": project.repo_id() }),
    );
}

/// A pipeline changed because of an action here: tell the UI, remember it for
/// the poller, and poll this repository quickly for a while.
fn pipeline_changed(ctx: &GlCtx, id: u64, iid: Option<u64>, status: &str, git_ref: &str, sha: &str, web_url: &str) {
    let state = &ctx.state;
    let scope = ctx.project.scope_key();
    state.gitlab.invalidate_summary(&scope);
    state.gitlab.poll.note(&scope, git_ref, id, status);
    state.gitlab.poll.mark_hot(&scope);
    emit_pipeline(state, &ctx.project, id, iid, status, git_ref, sha, web_url, None);
}

/// A merge request changed because of an action here.
fn mr_changed(ctx: &GlCtx, iid: u64, action: &str) {
    ctx.state.gitlab.invalidate_summary(&ctx.project.scope_key());
    ctx.state.events.emit(
        "gitlab.mr",
        Some(&ctx.project.id),
        json!({ "iid": iid, "action": action, "repo": ctx.project.repo_id() }),
    );
}

pub fn router() -> Router<AppState> {
    Router::new().merge(pipelines::routes()).merge(mrs::routes()).merge(misc::routes())
}

pub async fn start(state: &AppState) {
    poller::spawn(state.clone());
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::mcp_tools()
}

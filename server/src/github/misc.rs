//! Repository summary (tool window header, top/status bar widgets), branch
//! lookups, issues, releases, and the global token status.
//!
//! Without a token GitHub allows 60 requests an hour, so the anonymous summary
//! asks for less (no open-PR count, the head's state from the branch's runs
//! instead of every check) and is shared longer.

use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::CiStatus;
use super::ci::{branch_runs, ci_status_for, valid_branch};
use super::client::{Fresh, GhCtx, RateInfo, ctx};
use super::model::{Issue, IssueComment, ListPage, Pull, Release, Run, aggregate};
use super::pulls::{open_pr_count, pr_for_branch, valid_body};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Summaries are shared by all open tabs for this long (while the local
/// branch and HEAD stay the same). Anonymous: as long as `Fresh::Live` lasts.
const SUMMARY_TTL: Duration = Duration::from_secs(8);
pub(super) const SUMMARY_TTL_ANON: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------- summary

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthInfo {
    pub authenticated: bool,
    /// Why there is no token (anonymous only).
    pub reason: Option<String>,
    pub viewer: Option<String>,
    /// The core API quota as last seen.
    pub rate: Option<RateInfo>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeMethods {
    pub merge: bool,
    pub squash: bool,
    pub rebase: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub host: String,
    /// `owner/repo`
    pub path: String,
    pub web_url: String,
    pub description: Option<String>,
    pub private: bool,
    pub archived: bool,
    pub default_branch: Option<String>,
    /// Local checkout: current branch and HEAD.
    pub branch: Option<String>,
    pub head: Option<String>,
    /// Combined checks of the local HEAD on GitHub (`None`: not pushed, or nothing ran).
    pub head_status: Option<CiStatus>,
    /// Newest runs of the current branch (all workflows) and the newest of the default branch.
    pub branch_runs: Vec<Run>,
    pub branch_run: Option<Run>,
    pub default_run: Option<Run>,
    /// The pull request whose head is the current branch.
    pub current_pr: Option<Pull>,
    pub open_pr_count: Option<u64>,
    pub open_issue_count: Option<u64>,
    pub has_issues: bool,
    /// `None` when unknown (anonymous).
    pub can_push: Option<bool>,
    pub merge_methods: Option<MergeMethods>,
    pub delete_branch_on_merge: Option<bool>,
    pub auth: AuthInfo,
    /// Parts that could not be loaded (the rest of the summary is still valid).
    pub warnings: Vec<String>,
    pub fetched_at: i64,
}

fn soft<T>(r: ApiResult<T>, what: &str, warnings: &mut Vec<String>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            warnings.push(format!("{what}: {}", e.message));
            None
        }
    }
}

/// The state of `head` from the branch's newest runs (the anonymous shortcut).
fn head_status_from_runs(runs: &[Run], head: &str) -> Option<CiStatus> {
    let mine: Vec<&Run> = runs.iter().filter(|r| r.head_sha == head).collect();
    let state = aggregate(mine.iter().map(|r| r.state.as_str()))?;
    let run = mine.iter().find(|r| r.state == state).or(mine.first())?;
    Some(CiStatus {
        status: state.to_string(),
        pipeline_id: Some(run.id),
        web_url: Some(run.html_url.clone()).filter(|u| !u.is_empty()),
        sha: Some(head.to_string()),
        git_ref: run.head_branch.clone(),
    })
}

/// The local checkout's current branch and HEAD (both read locally, free).
async fn checkout(ctx: &GhCtx) -> (Option<String>, Option<String>) {
    let root = &ctx.project.root;
    tokio::join!(crate::util::git::current_branch_logged(root), crate::util::git::head_sha_logged(root))
}

/// The summary for the checkout as it is now (the handler reads the checkout
/// itself, to decide whether a shared summary still fits).
#[cfg(test)]
pub async fn summary(ctx: &GhCtx) -> ApiResult<Summary> {
    let (branch, head) = checkout(ctx).await;
    summary_at(ctx, branch, head).await
}

/// The summary for a checkout at `branch` / `head`.
async fn summary_at(ctx: &GhCtx, branch: Option<String>, head: Option<String>) -> ApiResult<Summary> {
    let default = ctx.default_branch().map(str::to_string);
    let on_default = branch.is_some() && branch == default;
    let anon = ctx.is_anonymous();
    let (runs, default_runs, current_pr, prs, head_status, viewer) = tokio::join!(
        async {
            match &branch {
                Some(b) => branch_runs(ctx, b, 10).await,
                None => Ok(vec![]),
            }
        },
        async {
            match (&default, on_default) {
                (Some(d), false) => branch_runs(ctx, d, 10).await,
                _ => Ok(vec![]),
            }
        },
        async {
            match &branch {
                Some(b) if !on_default => pr_for_branch(ctx, b).await,
                _ => Ok(None),
            }
        },
        async { if anon { Ok(None) } else { open_pr_count(ctx).await.map(Some) } },
        async {
            match &head {
                // An unpushed commit is unknown to GitHub: `None`.
                Some(h) if !anon => ci_status_for(ctx, h).await,
                _ => Ok(None),
            }
        },
        ctx.viewer(),
    );
    let mut warnings = vec![];
    let branch_runs = soft(runs, "branch runs", &mut warnings).unwrap_or_default();
    let default_runs = soft(default_runs, "default branch runs", &mut warnings).unwrap_or_default();
    let current_pr = soft(current_pr, "pull request", &mut warnings).flatten();
    let open_pr_count = soft(prs, "pull requests", &mut warnings).flatten();
    let mut head_status = soft(head_status, "commit checks", &mut warnings).flatten();
    if anon {
        head_status = head.as_deref().and_then(|h| head_status_from_runs(&branch_runs, h));
    }
    let branch_run = branch_runs.first().cloned();
    let default_run = if on_default { branch_run.clone() } else { default_runs.into_iter().next() };
    let m = &ctx.meta;
    let open_issue_count = open_pr_count.map(|p| m.open_issues_count.saturating_sub(p));
    let merge_methods = match (m.allow_merge_commit, m.allow_squash_merge, m.allow_rebase_merge) {
        (None, None, None) => None,
        (a, b, c) => Some(MergeMethods { merge: a.unwrap_or(true), squash: b.unwrap_or(true), rebase: c.unwrap_or(true) }),
    };
    Ok(Summary {
        host: ctx.host.clone(),
        path: ctx.full_name(),
        web_url: m.html_url.clone(),
        description: m.description.clone(),
        private: m.private,
        archived: m.archived,
        default_branch: default,
        branch,
        head,
        head_status,
        branch_runs,
        branch_run,
        default_run,
        current_pr,
        open_pr_count,
        open_issue_count,
        has_issues: m.has_issues,
        can_push: m.permissions.as_ref().map(|p| p.push),
        merge_methods,
        delete_branch_on_merge: m.delete_branch_on_merge,
        auth: AuthInfo {
            authenticated: !anon,
            reason: ctx.anonymous_reason().map(str::to_string),
            viewer,
            rate: ctx.rate("core"),
        },
        warnings,
        fetched_at: crate::util::now_ms(),
    })
}

async fn h_summary(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, &pid).await?;
    let ttl = if ctx.is_anonymous() { SUMMARY_TTL_ANON } else { SUMMARY_TTL };
    // The branch and HEAD are local: a checkout or commit shows at once, even
    // while the GitHub side of the summary is still shared.
    let (branch, head) = checkout(&ctx).await;
    if let Some((at, v)) = state.github.summaries.lock().get(&pid) {
        let same_checkout = v["branch"].as_str() == branch.as_deref() && v["head"].as_str() == head.as_deref();
        if at.elapsed() < ttl && same_checkout {
            let mut v = v.clone();
            // The quota moves even when the summary does not.
            v["auth"]["rate"] = json!(ctx.rate("core"));
            return Ok(Json(v));
        }
    }
    let v = serde_json::to_value(summary_at(&ctx, branch, head).await?)?;
    state.github.summaries.lock().insert(pid, (Instant::now(), v.clone()));
    Ok(Json(v))
}

/// The quota as last seen, without asking GitHub.
async fn h_rate(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(json!({ "authenticated": !ctx.is_anonymous(), "core": ctx.rate("core"), "search": ctx.rate("search") })))
}

// ---------------------------------------------------------------- branch

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub exists: bool,
    pub protected: bool,
    pub sha: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BranchQuery {
    name: String,
}

pub async fn branch(ctx: &GhCtx, name: &str) -> ApiResult<BranchInfo> {
    let name = valid_branch(name)?;
    let enc: Vec<String> = name.split('/').map(|s| urlencoding::encode(s).into_owned()).collect();
    let url = ctx.rurl(&format!("/branches/{}", enc.join("/")));
    Ok(match ctx.get_opt::<Value>(&url, &[], Fresh::Live).await? {
        Some(v) => BranchInfo {
            name: name.to_string(),
            exists: true,
            protected: v["protected"].as_bool().unwrap_or(false),
            sha: v.pointer("/commit/sha").and_then(Value::as_str).map(str::to_string),
            title: v.pointer("/commit/commit/message").and_then(Value::as_str).map(super::model::first_line),
        },
        None => BranchInfo { name: name.to_string(), ..Default::default() },
    })
}

async fn h_branch(State(s): State<AppState>, Path(pid): Path<String>, Query(q): Query<BranchQuery>) -> ApiResult<Json<BranchInfo>> {
    Ok(Json(branch(&ctx(&s, &pid).await?, &q.name).await?))
}

// ---------------------------------------------------------------- issues

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssuesQuery {
    /// `open` (default) | `closed` | `all`
    pub state: Option<String>,
    pub search: Option<String>,
    pub labels: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn list_issues(ctx: &GhCtx, q: &IssuesQuery) -> ApiResult<ListPage<Issue>> {
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let per_page = q.per_page.unwrap_or(25).clamp(1, 100);
    let state = q.state.as_deref().filter(|s| !s.is_empty()).unwrap_or("open");
    if !matches!(state, "open" | "closed" | "all") {
        return Err(ApiError::bad_request(format!("unknown issue state {state:?}")));
    }
    if let Some(s) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let mut terms = format!("repo:{} is:issue", ctx.full_name());
        if state != "all" {
            terms.push_str(&format!(" is:{state}"));
        }
        terms.push(' ');
        terms.push_str(&s.chars().filter(|c| !c.is_control()).take(200).collect::<String>());
        let query = [
            ("q", terms),
            ("sort", "updated".to_string()),
            ("order", "desc".to_string()),
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ];
        let res = ctx.get_page::<Value>(&format!("{}/search/issues", ctx.api), &query, Fresh::Live).await?;
        let mut body = res.body;
        let total = body.get("total_count").and_then(Value::as_u64);
        let items: Vec<Issue> = serde_json::from_value(body["items"].take()).unwrap_or_default();
        return Ok(ListPage { items, page, next_page: res.next_url.map(|_| page + 1), total });
    }
    let mut query: Vec<(&str, String)> = vec![
        ("state", state.to_string()),
        ("sort", "updated".into()),
        ("direction", "desc".into()),
        ("page", page.to_string()),
        ("per_page", per_page.to_string()),
    ];
    if let Some(l) = q.labels.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query.push(("labels", l.chars().take(500).collect()));
    }
    let res = ctx.get_page::<Vec<Issue>>(&ctx.rurl("/issues"), &query, Fresh::Live).await?;
    // The issues API lists pull requests too.
    let items: Vec<Issue> = res.body.into_iter().filter(|i| i.pull_request.is_none()).collect();
    Ok(ListPage { items, page, next_page: res.next_url.map(|_| page + 1), total: None })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub issue: Issue,
    pub comments: Vec<IssueComment>,
}

pub async fn issue_detail(ctx: &GhCtx, n: u64) -> ApiResult<IssueDetail> {
    let url = ctx.rurl(&format!("/issues/{n}"));
    let comments_url = format!("{url}/comments");
    let (issue, comments) = tokio::join!(
        ctx.get::<Issue>(&url, &[], Fresh::Live),
        ctx.get_all::<IssueComment>(&comments_url, &[], 2000, None, Fresh::Live)
    );
    let issue = issue?;
    if issue.pull_request.is_some() {
        return Err(ApiError::bad_request(format!("#{n} is a pull request")));
    }
    Ok(IssueDetail { issue, comments: comments?.0 })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreateIssue {
    pub title: String,
    pub body: Option<String>,
    pub labels: Vec<String>,
}

pub async fn create_issue(ctx: &GhCtx, b: &CreateIssue) -> ApiResult<Issue> {
    let title = b.title.trim();
    if title.is_empty() {
        return Err(ApiError::bad_request("a title is required"));
    }
    let mut payload = json!({ "title": title.chars().take(256).collect::<String>() });
    if let Some(body) = b.body.as_deref().filter(|s| !s.trim().is_empty()) {
        payload["body"] = json!(body);
    }
    if !b.labels.is_empty() {
        payload["labels"] = json!(b.labels.iter().take(50).collect::<Vec<_>>());
    }
    let issue: Issue = ctx.write(Method::POST, &ctx.rurl("/issues"), Some(&payload)).await?;
    super::issue_changed(ctx, issue.number, "created");
    Ok(issue)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub body: Option<String>,
    /// `open` | `closed`
    pub state: Option<String>,
    /// With `closed`: `completed` | `not_planned`
    pub state_reason: Option<String>,
}

pub async fn update_issue(ctx: &GhCtx, n: u64, b: &UpdateIssue) -> ApiResult<Issue> {
    let mut payload = serde_json::Map::new();
    if let Some(t) = b.title.as_deref() {
        if t.trim().is_empty() {
            return Err(ApiError::bad_request("the title cannot be empty"));
        }
        payload.insert("title".into(), json!(t.trim()));
    }
    if let Some(body) = &b.body {
        payload.insert("body".into(), json!(body));
    }
    if let Some(s) = b.state.as_deref() {
        if !matches!(s, "open" | "closed") {
            return Err(ApiError::bad_request("state must be open or closed"));
        }
        payload.insert("state".into(), json!(s));
    }
    if let Some(r) = b.state_reason.as_deref() {
        if !matches!(r, "completed" | "not_planned" | "reopened") {
            return Err(ApiError::bad_request("stateReason must be completed or not_planned"));
        }
        payload.insert("state_reason".into(), json!(r));
    }
    if payload.is_empty() {
        return Err(ApiError::bad_request("nothing to update"));
    }
    let issue: Issue = ctx.write(Method::PATCH, &ctx.rurl(&format!("/issues/{n}")), Some(&Value::Object(payload))).await?;
    super::issue_changed(ctx, n, "updated");
    Ok(issue)
}

pub async fn add_issue_comment(ctx: &GhCtx, n: u64, body: &str) -> ApiResult<IssueComment> {
    let body = valid_body(body)?;
    let c: IssueComment = ctx.write(Method::POST, &ctx.rurl(&format!("/issues/{n}/comments")), Some(&json!({ "body": body }))).await?;
    super::issue_changed(ctx, n, "comment");
    Ok(c)
}

async fn h_issues(State(s): State<AppState>, Path(pid): Path<String>, Query(q): Query<IssuesQuery>) -> ApiResult<Json<ListPage<Issue>>> {
    Ok(Json(list_issues(&ctx(&s, &pid).await?, &q).await?))
}
async fn h_issue_create(State(s): State<AppState>, Path(pid): Path<String>, Json(b): Json<CreateIssue>) -> ApiResult<Json<Issue>> {
    Ok(Json(create_issue(&ctx(&s, &pid).await?, &b).await?))
}
async fn h_issue(State(s): State<AppState>, Path((pid, n)): Path<(String, u64)>) -> ApiResult<Json<IssueDetail>> {
    Ok(Json(issue_detail(&ctx(&s, &pid).await?, n).await?))
}
async fn h_issue_update(State(s): State<AppState>, Path((pid, n)): Path<(String, u64)>, Json(b): Json<UpdateIssue>) -> ApiResult<Json<Issue>> {
    Ok(Json(update_issue(&ctx(&s, &pid).await?, n, &b).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BodyOnly {
    body: String,
}

async fn h_issue_comment(State(s): State<AppState>, Path((pid, n)): Path<(String, u64)>, Json(b): Json<BodyOnly>) -> ApiResult<Json<IssueComment>> {
    Ok(Json(add_issue_comment(&ctx(&s, &pid).await?, n, &b.body).await?))
}

// ---------------------------------------------------------------- releases

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PageQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn releases(ctx: &GhCtx, q: &PageQuery) -> ApiResult<ListPage<Release>> {
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let per_page = q.per_page.unwrap_or(20).clamp(1, 100);
    let query = [("page", page.to_string()), ("per_page", per_page.to_string())];
    let res = ctx.get_page::<Vec<Release>>(&ctx.rurl("/releases"), &query, Fresh::Slow).await?;
    Ok(ListPage { items: res.body, page, next_page: res.next_url.map(|_| page + 1), total: None })
}

async fn h_releases(State(s): State<AppState>, Path(pid): Path<String>, Query(q): Query<PageQuery>) -> ApiResult<Json<ListPage<Release>>> {
    Ok(Json(releases(&ctx(&s, &pid).await?, &q).await?))
}

// ---------------------------------------------------------------- global

/// `GET /api/github/status`: whether a global token is configured and works.
async fn h_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let cfg = state.config.read().github.clone();
    let Some(cfg) = cfg.filter(|g| !g.token.trim().is_empty()) else {
        return Ok(Json(json!({ "configured": false })));
    };
    let token = match state.secret(None, cfg.token.trim()) {
        Ok(t) => t,
        Err(e) => return Ok(Json(json!({ "configured": false, "host": cfg.host, "error": e.message }))),
    };
    let (api, _, _) = super::client::base_urls(&cfg.host)?;
    let resp = state
        .github
        .http()
        .get(format!("{api}/user"))
        .header("Authorization", format!("Bearer {}", token.expose()))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| ApiError::upstream(format!("cannot reach {}: {}", cfg.host, e.without_url())))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Ok(Json(json!({ "configured": true, "host": cfg.host, "valid": false, "status": status })));
    }
    let user: super::model::User = resp.json().await.unwrap_or_default();
    Ok(Json(json!({ "configured": true, "host": cfg.host, "valid": true, "user": { "login": user.login, "name": user.name } })))
}

pub fn routes() -> Router<AppState> {
    let p = "/api/projects/{pid}/github";
    Router::new()
        .route("/api/github/status", get(h_status))
        .route(&format!("{p}/summary"), get(h_summary))
        .route(&format!("{p}/rate"), get(h_rate))
        .route(&format!("{p}/branch"), get(h_branch))
        .route(&format!("{p}/issues"), get(h_issues).post(h_issue_create))
        .route(&format!("{p}/issues/{{n}}"), get(h_issue).patch(h_issue_update))
        .route(&format!("{p}/issues/{{n}}/comments"), post(h_issue_comment))
        .route(&format!("{p}/releases"), get(h_releases))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_head_status_comes_from_runs() {
        let run = |id: u64, sha: &str, state: &str| Run {
            id,
            head_sha: sha.into(),
            state: state.into(),
            html_url: format!("https://github.com/o/r/actions/runs/{id}"),
            ..Default::default()
        };
        let runs = vec![run(3, "aaa", "running"), run(2, "aaa", "success"), run(1, "bbb", "failed")];
        let st = head_status_from_runs(&runs, "aaa").unwrap();
        assert_eq!((st.status.as_str(), st.pipeline_id), ("running", Some(3)));
        assert!(head_status_from_runs(&runs, "ccc").is_none());
    }
}

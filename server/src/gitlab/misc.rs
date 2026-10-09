//! Project summary (tool window header, top/status bar widgets), branch and
//! commit lookups, issues, environments and deployments, container registry.

use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::CiStatus;
use super::client::{GlCtx, ctx, ctx_for};
use super::model::{CommitRef, Deployment, Environment, Issue, ListPage, Mr, Note, Pipeline, RegistryRepo, RegistryTag};
use super::mrs::mr_for_branch;
use super::pipelines::{latest_pipeline, valid_sha};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::forge::RepoParam;

/// Summaries are shared by all open tabs for this long.
const SUMMARY_TTL: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- summary

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub host: String,
    /// `namespace/project`
    pub path: String,
    pub gitlab_project_id: u64,
    pub web_url: String,
    pub default_branch: Option<String>,
    /// Local checkout: current branch and HEAD.
    pub branch: Option<String>,
    pub head: Option<String>,
    /// CI status of the local HEAD commit on GitLab (`None`: unknown there or no pipeline).
    pub head_status: Option<CiStatus>,
    /// Newest pipeline of the current branch and of the default branch.
    pub branch_pipeline: Option<Pipeline>,
    pub default_pipeline: Option<Pipeline>,
    /// The merge request whose source is the current branch.
    pub current_mr: Option<Mr>,
    pub open_mr_count: Option<u64>,
    pub open_issue_count: Option<u64>,
    pub environment_count: Option<u64>,
    pub registry_enabled: bool,
    pub issues_enabled: bool,
    pub merge_requests_enabled: bool,
    pub merge_method: Option<String>,
    pub squash_option: Option<String>,
    pub remove_source_branch_after_merge: Option<bool>,
    pub only_allow_merge_if_pipeline_succeeds: Option<bool>,
    pub access_level: Option<u64>,
    /// Parts that could not be loaded (the rest of the summary is still valid).
    pub warnings: Vec<String>,
    pub fetched_at: i64,
}

/// Total of a list endpoint filtered by one `key=value` (one item fetched).
async fn count(ctx: &GlCtx, rel: &str, key: &str, value: &str) -> ApiResult<Option<u64>> {
    let q = [(key, value.to_string()), ("per_page", "1".to_string())];
    let page = ctx.get_page::<Value>(&ctx.purl(rel), &q).await?;
    // x-total is omitted above 10k items; fall back to "at least what we saw".
    Ok(page.total.or(Some(page.items.len() as u64)))
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

pub async fn summary(ctx: &GlCtx) -> ApiResult<Summary> {
    // The checkout of this repository, not the project root.
    let dir = ctx.project.repo_dir().to_path_buf();
    let (branch, head) = tokio::join!(crate::util::git::current_branch_logged(&dir), crate::util::git::head_sha_logged(&dir));
    let default = ctx.default_branch().map(str::to_string);
    let on_default = branch.is_some() && branch == default;
    let (branch_pipeline, default_pipeline, current_mr, head_status, mrs, issues, envs) = tokio::join!(
        async {
            match &branch {
                Some(b) => latest_pipeline(ctx, b).await,
                None => Ok(None),
            }
        },
        async {
            match (&default, on_default) {
                (Some(d), false) => latest_pipeline(ctx, d).await,
                _ => Ok(None),
            }
        },
        async {
            match &branch {
                Some(b) if !on_default => mr_for_branch(ctx, b).await,
                _ => Ok(None),
            }
        },
        async {
            match &head {
                Some(h) => super::ci_status_for(ctx, h).await,
                None => Ok(None),
            }
        },
        count(ctx, "/merge_requests", "state", "opened"),
        async {
            if ctx.meta.issues_enabled == Some(false) {
                Ok(None)
            } else {
                count(ctx, "/issues", "state", "opened").await
            }
        },
        count(ctx, "/environments", "states", "available"),
    );
    let mut warnings = vec![];
    let branch_pipeline = soft(branch_pipeline, "branch pipeline", &mut warnings).flatten();
    let default_pipeline = soft(default_pipeline, "default branch pipeline", &mut warnings).flatten();
    let default_pipeline = if on_default { branch_pipeline.clone() } else { default_pipeline };
    let current_mr = soft(current_mr, "merge request", &mut warnings).flatten();
    let head_status = soft(head_status, "commit status", &mut warnings).flatten();
    let open_mr_count = soft(mrs, "merge requests", &mut warnings).flatten();
    let open_issue_count = soft(issues, "issues", &mut warnings).flatten();
    // Environments are often disabled (403); that is not worth a warning.
    let environment_count = envs.ok().flatten();
    let m = &ctx.meta;
    Ok(Summary {
        host: ctx.host.clone(),
        path: m.path_with_namespace.clone(),
        gitlab_project_id: m.id,
        web_url: m.web_url.clone(),
        default_branch: default,
        branch,
        head,
        head_status,
        branch_pipeline,
        default_pipeline,
        current_mr,
        open_mr_count,
        open_issue_count,
        environment_count,
        registry_enabled: m.container_registry_enabled.unwrap_or(false),
        issues_enabled: m.issues_enabled.unwrap_or(true),
        merge_requests_enabled: m.merge_requests_enabled.unwrap_or(true),
        merge_method: m.merge_method.clone(),
        squash_option: m.squash_option.clone(),
        remove_source_branch_after_merge: m.remove_source_branch_after_merge,
        only_allow_merge_if_pipeline_succeeds: m.only_allow_merge_if_pipeline_succeeds,
        access_level: m.access_level(),
        warnings,
        fetched_at: crate::util::now_ms(),
    })
}

async fn h_summary(State(state): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let project = state.projects.require_repo(&pid, repo.id())?;
    let scope = project.scope_key();
    if let Some((at, v)) = state.gitlab.summaries.lock().get(&scope) {
        if at.elapsed() < SUMMARY_TTL {
            return Ok(Json(v.clone()));
        }
    }
    let ctx = ctx_for(&state, project).await?;
    let v = serde_json::to_value(summary(&ctx).await?)?;
    state.gitlab.summaries.lock().insert(scope, (Instant::now(), v.clone()));
    Ok(Json(v))
}

// ---------------------------------------------------------------- branch / commit

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Branch {
    pub name: String,
    pub merged: bool,
    pub protected: bool,
    pub default: bool,
    pub can_push: bool,
    pub web_url: Option<String>,
    pub commit: Option<CommitRef>,
    /// Filled in: whether GitLab has the branch at all.
    pub exists: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BranchQuery {
    name: String,
}

pub async fn branch(ctx: &GlCtx, name: &str) -> ApiResult<Branch> {
    let name = super::mrs::valid_branch(name)?;
    let url = ctx.purl(&format!("/repository/branches/{}", urlencoding::encode(name)));
    Ok(match ctx.get_opt::<Branch>(&url, &[]).await? {
        Some(mut b) => {
            b.exists = true;
            b
        }
        None => Branch { name: name.to_string(), ..Default::default() },
    })
}

async fn h_branch(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>, Query(q): Query<BranchQuery>) -> ApiResult<Json<Branch>> {
    Ok(Json(branch(&ctx(&s, &pid, &repo).await?, &q.name).await?))
}

async fn h_commit_status(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path((pid, sha)): Path<(String, String)>,
) -> ApiResult<Json<Option<CiStatus>>> {
    let sha = valid_sha(&sha)?.to_string();
    Ok(Json(super::ci_status_for(&ctx(&s, &pid, &repo).await?, &sha).await?))
}

// ---------------------------------------------------------------- issues

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssuesQuery {
    pub state: Option<String>,
    pub search: Option<String>,
    pub labels: Option<String>,
    pub scope: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn list_issues(ctx: &GlCtx, q: &IssuesQuery) -> ApiResult<ListPage<Issue>> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(30).clamp(1, 100);
    let state = q.state.as_deref().filter(|s| !s.is_empty()).unwrap_or("opened");
    if !matches!(state, "opened" | "closed" | "all") {
        return Err(ApiError::bad_request(format!("unknown issue state {state:?}")));
    }
    let mut query: Vec<(&str, String)> = vec![
        ("page", page.to_string()),
        ("per_page", per_page.to_string()),
        ("order_by", "updated_at".into()),
        ("sort", "desc".into()),
    ];
    if state != "all" {
        query.push(("state", state.into()));
    }
    if let Some(s) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query.push(("search", s.chars().take(200).collect()));
    }
    if let Some(l) = q.labels.as_deref().filter(|s| !s.is_empty()) {
        query.push(("labels", l.into()));
    }
    if let Some(s) = q.scope.as_deref().filter(|s| !s.is_empty()) {
        // `all` is the project default; `/issues?scope=all` is a 500 on gitlab.com.
        if !matches!(s, "created_by_me" | "assigned_to_me") {
            return Err(ApiError::bad_request(format!("unknown scope {s:?}")));
        }
        query.push(("scope", s.into()));
    }
    let res = ctx.get_page::<Issue>(&ctx.purl("/issues"), &query).await?;
    Ok(ListPage { items: res.items, page: res.page.unwrap_or(page), next_page: res.next_page, total: res.total })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub issue: Issue,
    pub notes: Vec<Note>,
}

pub async fn issue_detail(ctx: &GlCtx, iid: u64) -> ApiResult<IssueDetail> {
    let url = ctx.purl(&format!("/issues/{iid}"));
    let notes_url = format!("{url}/notes");
    let notes_q = [("sort", "asc".to_string()), ("order_by", "created_at".to_string())];
    let (issue, notes) = tokio::join!(ctx.get::<Issue>(&url, &[]), ctx.get_all::<Note>(&notes_url, &notes_q, 2000));
    Ok(IssueDetail { issue: issue?, notes: notes?.0 })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreateIssue {
    pub title: String,
    pub description: Option<String>,
    pub labels: Vec<String>,
    pub confidential: bool,
}

pub async fn create_issue(ctx: &GlCtx, b: &CreateIssue) -> ApiResult<Issue> {
    let title = b.title.trim();
    if title.is_empty() {
        return Err(ApiError::bad_request("a title is required"));
    }
    let mut payload = json!({ "title": title.chars().take(255).collect::<String>() });
    if let Some(d) = &b.description {
        payload["description"] = json!(d);
    }
    if !b.labels.is_empty() {
        payload["labels"] = json!(b.labels.join(","));
    }
    if b.confidential {
        payload["confidential"] = json!(true);
    }
    let issue: Issue = ctx.write(Method::POST, &ctx.purl("/issues"), Some(&payload)).await?;
    issue_changed(ctx, issue.iid, "created");
    Ok(issue)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub description: Option<String>,
    /// `close` | `reopen`
    pub state_event: Option<String>,
    pub labels: Option<Vec<String>>,
}

pub async fn update_issue(ctx: &GlCtx, iid: u64, b: &UpdateIssue) -> ApiResult<Issue> {
    let mut payload = serde_json::Map::new();
    if let Some(t) = b.title.as_deref() {
        if t.trim().is_empty() {
            return Err(ApiError::bad_request("the title cannot be empty"));
        }
        payload.insert("title".into(), json!(t.trim()));
    }
    if let Some(d) = &b.description {
        payload.insert("description".into(), json!(d));
    }
    if let Some(e) = b.state_event.as_deref() {
        if !matches!(e, "close" | "reopen") {
            return Err(ApiError::bad_request("stateEvent must be close or reopen"));
        }
        payload.insert("state_event".into(), json!(e));
    }
    if let Some(l) = &b.labels {
        payload.insert("labels".into(), json!(l.join(",")));
    }
    if payload.is_empty() {
        return Err(ApiError::bad_request("nothing to update"));
    }
    let issue: Issue =
        ctx.write(Method::PUT, &ctx.purl(&format!("/issues/{iid}")), Some(&Value::Object(payload))).await?;
    issue_changed(ctx, iid, "updated");
    Ok(issue)
}

pub async fn add_issue_note(ctx: &GlCtx, iid: u64, body: &str) -> ApiResult<Note> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request("the comment is empty"));
    }
    let note: Note =
        ctx.write(Method::POST, &ctx.purl(&format!("/issues/{iid}/notes")), Some(&json!({ "body": body }))).await?;
    issue_changed(ctx, iid, "note");
    Ok(note)
}

fn issue_changed(ctx: &GlCtx, iid: u64, action: &str) {
    ctx.state.gitlab.invalidate_summary(&ctx.project.scope_key());
    ctx.state.events.emit(
        "gitlab.issue",
        Some(&ctx.project.id),
        json!({ "iid": iid, "action": action, "repo": ctx.project.repo_id() }),
    );
}

async fn h_issues(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>, Query(q): Query<IssuesQuery>) -> ApiResult<Json<ListPage<Issue>>> {
    Ok(Json(list_issues(&ctx(&s, &pid, &repo).await?, &q).await?))
}
async fn h_issue_create(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>, Json(b): Json<CreateIssue>) -> ApiResult<Json<Issue>> {
    Ok(Json(create_issue(&ctx(&s, &pid, &repo).await?, &b).await?))
}
async fn h_issue(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Path<(String, u64)>) -> ApiResult<Json<IssueDetail>> {
    Ok(Json(issue_detail(&ctx(&s, &pid, &repo).await?, iid).await?))
}
async fn h_issue_update(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path((pid, iid)): Path<(String, u64)>,
    Json(b): Json<UpdateIssue>,
) -> ApiResult<Json<Issue>> {
    Ok(Json(update_issue(&ctx(&s, &pid, &repo).await?, iid, &b).await?))
}
async fn h_issue_note(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path((pid, iid)): Path<(String, u64)>,
    Json(b): Json<super::mrs::NoteBody>,
) -> ApiResult<Json<Note>> {
    Ok(Json(add_issue_note(&ctx(&s, &pid, &repo).await?, iid, &b.body).await?))
}

// ---------------------------------------------------------------- environments

/// Available environments, each with its last deployment (the list endpoint
/// omits it, so the first 30 are fetched individually).
pub async fn environments(ctx: &GlCtx) -> ApiResult<Vec<Environment>> {
    let q = [("states", "available".to_string())];
    let (list, _) = ctx.get_all::<Environment>(&ctx.purl("/environments"), &q, 200).await?;
    let detailed: Vec<Environment> = futures::stream::iter(list.into_iter().enumerate().map(|(i, e)| async move {
        if i >= 30 {
            return e;
        }
        ctx.get::<Environment>(&ctx.purl(&format!("/environments/{}", e.id)), &[]).await.unwrap_or(e)
    }))
    .buffered(6)
    .collect()
    .await;
    Ok(detailed)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeploymentsQuery {
    pub environment: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn deployments(ctx: &GlCtx, q: &DeploymentsQuery) -> ApiResult<ListPage<Deployment>> {
    let page = q.page.unwrap_or(1).max(1);
    // order_by=id together with environment= is a 500 on gitlab.com; updated_at works.
    let mut query: Vec<(&str, String)> = vec![
        ("page", page.to_string()),
        ("per_page", q.per_page.unwrap_or(20).clamp(1, 100).to_string()),
        ("order_by", "updated_at".into()),
        ("sort", "desc".into()),
    ];
    if let Some(e) = q.environment.as_deref().filter(|e| !e.is_empty()) {
        query.push(("environment", e.into()));
    }
    let res = ctx.get_page::<Deployment>(&ctx.purl("/deployments"), &query).await?;
    Ok(ListPage { items: res.items, page: res.page.unwrap_or(page), next_page: res.next_page, total: res.total })
}

async fn h_envs(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>) -> ApiResult<Json<Vec<Environment>>> {
    Ok(Json(environments(&ctx(&s, &pid, &repo).await?).await?))
}
async fn h_deployments(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path(pid): Path<String>,
    Query(q): Query<DeploymentsQuery>,
) -> ApiResult<Json<ListPage<Deployment>>> {
    Ok(Json(deployments(&ctx(&s, &pid, &repo).await?, &q).await?))
}

// ---------------------------------------------------------------- registry

pub async fn registry_repos(ctx: &GlCtx) -> ApiResult<Vec<RegistryRepo>> {
    let q = [("tags_count", "true".to_string())];
    Ok(ctx.get_all::<RegistryRepo>(&ctx.purl("/registry/repositories"), &q, 200).await?.0)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TagsQuery {
    /// Opaque cursor from the previous page.
    pub cursor: Option<String>,
    pub per_page: Option<u32>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagsPage {
    pub items: Vec<RegistryTag>,
    pub next_cursor: Option<String>,
    /// `published` (newest first) or `name` (REST fallback: alphabetical, no dates).
    pub order: String,
    pub total: Option<u64>,
}

const TAGS_QUERY: &str = "query($id: ContainerRepositoryID!, $first: Int, $after: String) { containerRepository(id: $id) { tagsCount tags(first: $first, after: $after, sort: PUBLISHED_AT_DESC) { pageInfo { hasNextPage endCursor } nodes { name path location digest publishedAt createdAt totalSize } } } }";

/// Tags newest first. GitLab's REST API only sorts by name, so this uses the
/// read-only GraphQL API (sent as GET) and falls back to REST keyset paging.
pub async fn registry_tags(ctx: &GlCtx, rid: u64, q: &TagsQuery) -> ApiResult<TagsPage> {
    let per_page = q.per_page.unwrap_or(50).clamp(1, 100);
    let cursor = q.cursor.as_deref().unwrap_or("");
    if !cursor.starts_with("r:") {
        let after = cursor.strip_prefix("g:").filter(|c| !c.is_empty());
        let vars = json!({ "id": format!("gid://gitlab/ContainerRepository/{rid}"), "first": per_page, "after": after });
        match ctx.graphql(TAGS_QUERY, vars).await {
            Ok(data) if !data["containerRepository"].is_null() => return Ok(tags_from_graphql(&data)),
            Ok(_) | Err(_) if !cursor.is_empty() => {
                return Err(ApiError::upstream("the registry tag list could not be continued; reload it"));
            }
            _ => {} // fall back to REST from the start
        }
    }
    let mut query: Vec<(&str, String)> = vec![
        ("pagination", "keyset".into()),
        ("per_page", per_page.to_string()),
        ("sort", "asc".into()),
    ];
    if let Some(last) = cursor.strip_prefix("r:").filter(|c| !c.is_empty()) {
        query.push(("last", last.to_string()));
    }
    let page = ctx.get_page::<RegistryTag>(&ctx.purl(&format!("/registry/repositories/{rid}/tags")), &query).await?;
    let next_cursor = page
        .next_url
        .as_ref()
        .and_then(|_| page.items.last())
        .map(|t| format!("r:{}", t.name));
    Ok(TagsPage { items: page.items, next_cursor, order: "name".into(), total: page.total })
}

pub fn tags_from_graphql(data: &Value) -> TagsPage {
    let repo = &data["containerRepository"];
    let tags = &repo["tags"];
    let items = tags["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .map(|n| RegistryTag {
                    name: n["name"].as_str().unwrap_or_default().to_string(),
                    path: n["path"].as_str().map(str::to_string),
                    location: n["location"].as_str().unwrap_or_default().to_string(),
                    digest: n["digest"].as_str().map(str::to_string),
                    created_at: n["createdAt"].as_str().map(str::to_string),
                    published_at: n["publishedAt"].as_str().map(str::to_string),
                    // BigInt arrives as a string; 0 means "not computed" on gitlab.com.
                    total_size: match &n["totalSize"] {
                        Value::String(s) => s.parse().ok(),
                        Value::Number(x) => x.as_u64(),
                        _ => None,
                    }
                    .filter(|s| *s > 0),
                    revision: None,
                })
                .collect()
        })
        .unwrap_or_default();
    let next_cursor = (tags["pageInfo"]["hasNextPage"].as_bool() == Some(true))
        .then(|| tags["pageInfo"]["endCursor"].as_str().map(|c| format!("g:{c}")))
        .flatten();
    TagsPage { items, next_cursor, order: "published".into(), total: repo["tagsCount"].as_u64() }
}

fn valid_tag(t: &str) -> ApiResult<&str> {
    let ok = !t.is_empty()
        && t.len() <= 128
        && t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok { Ok(t) } else { Err(ApiError::bad_request(format!("invalid tag {t:?}"))) }
}

pub async fn registry_tag(ctx: &GlCtx, rid: u64, tag: &str) -> ApiResult<RegistryTag> {
    let tag = valid_tag(tag)?;
    let mut t: RegistryTag = ctx.get(&ctx.purl(&format!("/registry/repositories/{rid}/tags/{tag}")), &[]).await?;
    if t.total_size == Some(0) {
        t.total_size = None;
    }
    Ok(t)
}

async fn h_registry(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>) -> ApiResult<Json<Vec<RegistryRepo>>> {
    Ok(Json(registry_repos(&ctx(&s, &pid, &repo).await?).await?))
}
async fn h_tags(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path((pid, rid)): Path<(String, u64)>,
    Query(q): Query<TagsQuery>,
) -> ApiResult<Json<TagsPage>> {
    Ok(Json(registry_tags(&ctx(&s, &pid, &repo).await?, rid, &q).await?))
}
async fn h_tag(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, rid, tag)): Path<(String, u64, String)>) -> ApiResult<Json<RegistryTag>> {
    Ok(Json(registry_tag(&ctx(&s, &pid, &repo).await?, rid, &tag).await?))
}

// ---------------------------------------------------------------- global

/// `GET /api/gitlab/status`: whether a global token is configured and valid.
async fn h_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let cfg = state.config.read().gitlab.clone();
    let Some(cfg) = cfg.filter(|g| !g.token.trim().is_empty()) else {
        return Ok(Json(json!({ "configured": false })));
    };
    let token = match state.secret(None, cfg.token.trim()) {
        Ok(t) => t,
        Err(e) => return Ok(Json(json!({ "configured": false, "host": cfg.host, "error": e.message }))),
    };
    let (api, _) = super::client::base_urls(&cfg.host)?;
    let resp = state
        .gitlab
        .http()
        .get(format!("{api}/user"))
        .header("PRIVATE-TOKEN", token.expose())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| ApiError::upstream(format!("cannot reach {}: {}", cfg.host, e.without_url())))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Ok(Json(json!({ "configured": true, "host": cfg.host, "valid": false, "status": status })));
    }
    let user: super::model::User = resp.json().await.unwrap_or_default();
    Ok(Json(json!({ "configured": true, "host": cfg.host, "valid": true, "user": user })))
}

pub fn routes() -> Router<AppState> {
    let p = "/api/projects/{pid}/gitlab";
    Router::new()
        .route("/api/gitlab/status", get(h_status))
        .route(&format!("{p}/summary"), get(h_summary))
        .route(&format!("{p}/branch"), get(h_branch))
        .route(&format!("{p}/commits/{{sha}}/status"), get(h_commit_status))
        .route(&format!("{p}/issues"), get(h_issues).post(h_issue_create))
        .route(&format!("{p}/issues/{{iid}}"), get(h_issue).put(h_issue_update))
        .route(&format!("{p}/issues/{{iid}}/notes"), post(h_issue_note))
        .route(&format!("{p}/environments"), get(h_envs))
        .route(&format!("{p}/deployments"), get(h_deployments))
        .route(&format!("{p}/registry"), get(h_registry))
        .route(&format!("{p}/registry/{{rid}}/tags"), get(h_tags))
        .route(&format!("{p}/registry/{{rid}}/tags/{{tag}}"), get(h_tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_tags_map_newest_first_with_cursor() {
        let data = json!({ "containerRepository": { "tagsCount": 770, "tags": {
            "pageInfo": { "hasNextPage": true, "endCursor": "MjAy" },
            "nodes": [
                { "name": "6bc07e5a", "location": "registry.gitlab.com/v/h:6bc07e5a", "digest": "sha256:6c72",
                  "publishedAt": "2026-09-26T10:51:19+00:00", "createdAt": "2026-09-26T10:51:19+00:00", "totalSize": "0" },
                { "name": "buildcache", "location": "l", "totalSize": "1152773166" }
            ]
        }}});
        let page = tags_from_graphql(&data);
        assert_eq!(page.order, "published");
        assert_eq!(page.total, Some(770));
        assert_eq!(page.next_cursor.as_deref(), Some("g:MjAy"));
        assert_eq!(page.items[0].name, "6bc07e5a");
        assert_eq!(page.items[0].total_size, None);
        assert_eq!(page.items[1].total_size, Some(1152773166));
        let last = json!({ "containerRepository": { "tags": { "pageInfo": { "hasNextPage": false, "endCursor": "x" }, "nodes": [] } } });
        assert_eq!(tags_from_graphql(&last).next_cursor, None);
    }

    #[test]
    fn tag_names_are_validated() {
        assert!(valid_tag("4b8e2508").is_ok());
        assert!(valid_tag("v1.2.3-rc_1").is_ok());
        assert!(valid_tag("../x").is_err());
        assert!(valid_tag(".hidden").is_err());
        assert!(valid_tag("").is_err());
    }
}

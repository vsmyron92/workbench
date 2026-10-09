//! Merge requests: list, detail (with approvals), diffs and file versions for
//! side-by-side review, discussions (notes, replies, diff-line comments,
//! resolve), approve, merge, rebase, create and update.

use std::sync::LazyLock;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use regex::Regex;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{GlCtx, ctx, read_tail};
use super::model::{Commit, Discussion, ListPage, Mr, MrDiffFile, Note, Pipeline, diff_stats};
use super::pipelines::{enrich_pipelines, is_hex_sha};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::forge::RepoParam;

/// Largest file version we send to the diff viewer.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
/// Per-file diff text beyond this is dropped (the viewer diffs raw files anyway).
const MAX_DIFF_TEXT: usize = 1024 * 1024;
/// GitLab's own limit on a note body.
const MAX_NOTE: usize = 1_000_000;

// ---------------------------------------------------------------- list / detail

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MrsQuery {
    pub state: Option<String>,
    pub scope: Option<String>,
    pub search: Option<String>,
    pub source_branch: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn list_mrs(ctx: &GlCtx, q: &MrsQuery) -> ApiResult<ListPage<Mr>> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).clamp(1, 100);
    let state = q.state.as_deref().filter(|s| !s.is_empty()).unwrap_or("opened");
    if !matches!(state, "opened" | "closed" | "merged" | "locked" | "all") {
        return Err(ApiError::bad_request(format!("unknown merge request state {state:?}")));
    }
    let mut query: Vec<(&str, String)> = vec![
        ("state", state.into()),
        ("page", page.to_string()),
        ("per_page", per_page.to_string()),
        ("order_by", "updated_at".into()),
        ("sort", "desc".into()),
    ];
    if let Some(s) = q.scope.as_deref().filter(|s| !s.is_empty()) {
        if !matches!(s, "created_by_me" | "assigned_to_me" | "all") {
            return Err(ApiError::bad_request(format!("unknown scope {s:?}")));
        }
        query.push(("scope", s.into()));
    }
    if let Some(s) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        query.push(("search", s.chars().take(200).collect()));
    }
    if let Some(b) = q.source_branch.as_deref().filter(|s| !s.is_empty()) {
        query.push(("source_branch", b.into()));
    }
    let res = ctx.get_page::<Mr>(&ctx.purl("/merge_requests"), &query).await?;
    Ok(ListPage { items: res.items, page: res.page.unwrap_or(page), next_page: res.next_page, total: res.total })
}

/// The MR whose source is `branch`: the open one, else the most recently updated.
pub async fn mr_for_branch(ctx: &GlCtx, branch: &str) -> ApiResult<Option<Mr>> {
    for state in ["opened", "all"] {
        let q = MrsQuery {
            state: Some(state.into()),
            source_branch: Some(branch.into()),
            per_page: Some(1),
            ..Default::default()
        };
        if let Some(mr) = list_mrs(ctx, &q).await?.items.into_iter().next() {
            return Ok(Some(mr));
        }
    }
    Ok(None)
}

pub async fn get_mr(ctx: &GlCtx, iid: u64) -> ApiResult<Mr> {
    let url = ctx.purl(&format!("/merge_requests/{iid}"));
    let approvals_url = format!("{url}/approvals");
    let (mr, approvals) = tokio::join!(ctx.get::<Mr>(&url, &[]), ctx.get_opt(&approvals_url, &[]));
    let mut mr = mr?;
    // Approvals are optional (older servers, restricted tokens).
    mr.approvals = approvals.ok().flatten();
    if let Some(p) = mr.head_pipeline.take() {
        mr.head_pipeline = enrich_pipelines(ctx, vec![p]).await.into_iter().next();
    }
    Ok(mr)
}

// ---------------------------------------------------------------- create / update

static DRAFT_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:\[draft\]\s*|\(draft\)\s*|draft:\s*|draft\s+-\s+)+").expect("valid regex"));

#[cfg(test)]
pub fn is_draft_title(t: &str) -> bool {
    DRAFT_PREFIX.is_match(t)
}

/// Add or remove GitLab's draft marker (`Draft: `) on a title.
pub fn set_draft(title: &str, draft: bool) -> String {
    let bare = DRAFT_PREFIX.replace(title, "").trim().to_string();
    if draft { format!("Draft: {bare}") } else { bare }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreateMr {
    /// Default: the project's current local branch.
    pub source_branch: Option<String>,
    /// Default: the project's default branch.
    pub target_branch: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub draft: bool,
    pub remove_source_branch: Option<bool>,
    pub squash: Option<bool>,
    pub labels: Vec<String>,
    pub assignee_ids: Vec<u64>,
    pub reviewer_ids: Vec<u64>,
}

pub fn valid_branch(b: &str) -> ApiResult<&str> {
    let b = b.trim();
    if b.is_empty()
        || b.len() > 255
        || b.starts_with('-')
        || b.contains(['\0', ' ', '~', '^', ':', '?', '*', '[', '\\'])
        || b.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(ApiError::bad_request(format!("invalid branch name {b:?}")));
    }
    Ok(b)
}

fn valid_title(t: &str) -> ApiResult<String> {
    let t = t.trim();
    if t.is_empty() {
        return Err(ApiError::bad_request("a title is required"));
    }
    Ok(t.chars().take(255).collect())
}

fn valid_body(b: &str) -> ApiResult<&str> {
    if b.trim().is_empty() {
        return Err(ApiError::bad_request("the comment is empty"));
    }
    if b.len() > MAX_NOTE {
        return Err(ApiError::bad_request("the comment is too long"));
    }
    Ok(b)
}

pub async fn create_mr(ctx: &GlCtx, body: &CreateMr) -> ApiResult<Mr> {
    let source = match body.source_branch.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(s) => s.trim().to_string(),
        None => crate::util::git::current_branch(ctx.project.repo_dir())
            .await
            .ok_or_else(|| ApiError::bad_request("the project is not on a branch; pass sourceBranch"))?,
    };
    let target = match body.target_branch.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(t) => t.trim().to_string(),
        None => ctx
            .default_branch()
            .map(str::to_string)
            .ok_or_else(|| ApiError::bad_request("the project has no default branch; pass targetBranch"))?,
    };
    valid_branch(&source)?;
    valid_branch(&target)?;
    if source == target {
        return Err(ApiError::bad_request(format!("source and target are both {source}")));
    }
    let title = valid_title(&body.title)?;
    let title = if body.draft { set_draft(&title, true) } else { title };
    let mut payload = json!({ "source_branch": source, "target_branch": target, "title": title });
    if let Some(d) = &body.description {
        payload["description"] = json!(d);
    }
    if let Some(r) = body.remove_source_branch {
        payload["remove_source_branch"] = json!(r);
    }
    if let Some(s) = body.squash {
        payload["squash"] = json!(s);
    }
    if !body.labels.is_empty() {
        payload["labels"] = json!(body.labels.join(","));
    }
    if !body.assignee_ids.is_empty() {
        payload["assignee_ids"] = json!(body.assignee_ids);
    }
    if !body.reviewer_ids.is_empty() {
        payload["reviewer_ids"] = json!(body.reviewer_ids);
    }
    let mr: Mr = ctx.write(Method::POST, &ctx.purl("/merge_requests"), Some(&payload)).await?;
    super::mr_changed(ctx, mr.iid, "created");
    Ok(mr)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateMr {
    pub title: Option<String>,
    pub description: Option<String>,
    /// `close` | `reopen`
    pub state_event: Option<String>,
    pub draft: Option<bool>,
    pub target_branch: Option<String>,
    pub remove_source_branch: Option<bool>,
    pub squash: Option<bool>,
    pub labels: Option<Vec<String>>,
}

pub async fn update_mr(ctx: &GlCtx, iid: u64, body: &UpdateMr) -> ApiResult<Mr> {
    let mut payload = serde_json::Map::new();
    let title = match &body.title {
        Some(t) => Some(valid_title(t)?),
        None => None,
    };
    match (body.draft, title) {
        (Some(draft), t) => {
            let base = match t {
                Some(t) => t,
                None => get_mr(ctx, iid).await?.title,
            };
            payload.insert("title".into(), json!(set_draft(&base, draft)));
        }
        (None, Some(t)) => {
            payload.insert("title".into(), json!(t));
        }
        (None, None) => {}
    }
    if let Some(d) = &body.description {
        payload.insert("description".into(), json!(d));
    }
    if let Some(e) = body.state_event.as_deref() {
        if !matches!(e, "close" | "reopen") {
            return Err(ApiError::bad_request("stateEvent must be close or reopen"));
        }
        payload.insert("state_event".into(), json!(e));
    }
    if let Some(t) = &body.target_branch {
        payload.insert("target_branch".into(), json!(valid_branch(t)?));
    }
    if let Some(r) = body.remove_source_branch {
        payload.insert("remove_source_branch".into(), json!(r));
    }
    if let Some(s) = body.squash {
        payload.insert("squash".into(), json!(s));
    }
    if let Some(l) = &body.labels {
        payload.insert("labels".into(), json!(l.join(",")));
    }
    if payload.is_empty() {
        return Err(ApiError::bad_request("nothing to update"));
    }
    let _: Value = ctx
        .write(Method::PUT, &ctx.purl(&format!("/merge_requests/{iid}")), Some(&Value::Object(payload)))
        .await?;
    super::mr_changed(ctx, iid, "updated");
    get_mr(ctx, iid).await
}

// ---------------------------------------------------------------- diffs

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MrDiffs {
    pub files: Vec<MrDiffFile>,
    /// More files exist than we fetched.
    pub truncated: bool,
}

pub async fn mr_diffs(ctx: &GlCtx, iid: u64) -> ApiResult<MrDiffs> {
    let url = ctx.purl(&format!("/merge_requests/{iid}/diffs"));
    let (mut files, truncated) = ctx.get_all::<MrDiffFile>(&url, &[], 3000).await?;
    for f in &mut files {
        let (a, d) = diff_stats(&f.diff);
        f.additions = a;
        f.deletions = d;
        if f.diff.len() > MAX_DIFF_TEXT {
            f.diff = String::new();
            f.too_large = Some(true);
        }
    }
    Ok(MrDiffs { files, truncated })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileQuery {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// `diff_refs.base_sha` / `head_sha`; fetched from the MR when absent.
    pub base: Option<String>,
    pub head: Option<String>,
    pub new_file: bool,
    pub deleted_file: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersions {
    pub original: String,
    pub modified: String,
    pub binary: bool,
    pub too_large: bool,
    pub base_sha: String,
    pub head_sha: String,
}

struct RawFile {
    text: String,
    binary: bool,
    too_large: bool,
}

/// A repository path for `/repository/files/:path`. Dot segments are refused:
/// an unencoded `..` would be normalized away and change the API path.
pub fn valid_repo_path(p: &str) -> ApiResult<&str> {
    if p.is_empty() || p.len() > 4096 || p.contains('\0') || p.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(ApiError::bad_request("invalid file path"));
    }
    Ok(p)
}

async fn raw_file(ctx: &GlCtx, path: &str, sha: &str) -> ApiResult<RawFile> {
    let url = ctx.purl(&format!("/repository/files/{}/raw", urlencoding::encode(path)));
    let resp = ctx.send_raw(Method::GET, &url, |rb| rb.query(&[("ref", sha)])).await?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(RawFile { text: String::new(), binary: false, too_large: false });
    }
    if !resp.status().is_success() {
        return Err(ctx.error_from(resp).await);
    }
    let (bytes, total, _) = read_tail(resp, MAX_FILE_BYTES).await?;
    if total > MAX_FILE_BYTES as u64 {
        return Ok(RawFile { text: String::new(), binary: false, too_large: true });
    }
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Ok(RawFile { text: String::new(), binary: true, too_large: false });
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(RawFile { text, binary: false, too_large: false }),
        Err(_) => Ok(RawFile { text: String::new(), binary: true, too_large: false }),
    }
}

/// Both sides of one changed file, from raw files at `base_sha` and `head_sha`.
pub async fn mr_file(ctx: &GlCtx, iid: u64, q: &FileQuery) -> ApiResult<FileVersions> {
    let new_path = q.new_path.as_deref().or(q.old_path.as_deref()).ok_or_else(|| ApiError::bad_request("newPath is required"))?;
    let old_path = q.old_path.as_deref().unwrap_or(new_path);
    valid_repo_path(new_path)?;
    valid_repo_path(old_path)?;
    let (base, head) = match (q.base.as_deref(), q.head.as_deref()) {
        (Some(b), Some(h)) if is_hex_sha(b) && is_hex_sha(h) => (b.to_string(), h.to_string()),
        _ => {
            let mr: Mr = ctx.get(&ctx.purl(&format!("/merge_requests/{iid}")), &[]).await?;
            let refs = mr.diff_refs.ok_or_else(|| ApiError::conflict("GitLab has not computed this merge request's diff yet"))?;
            (refs.base_sha, refs.head_sha)
        }
    };
    let original = async {
        if q.new_file { Ok(None) } else { raw_file(ctx, old_path, &base).await.map(Some) }
    };
    let modified = async {
        if q.deleted_file { Ok(None) } else { raw_file(ctx, new_path, &head).await.map(Some) }
    };
    let (original, modified) = tokio::join!(original, modified);
    let (original, modified) = (original?, modified?);
    let binary = original.as_ref().is_some_and(|f| f.binary) || modified.as_ref().is_some_and(|f| f.binary);
    let too_large = original.as_ref().is_some_and(|f| f.too_large) || modified.as_ref().is_some_and(|f| f.too_large);
    Ok(FileVersions {
        original: original.map(|f| f.text).unwrap_or_default(),
        modified: modified.map(|f| f.text).unwrap_or_default(),
        binary,
        too_large,
        base_sha: base,
        head_sha: head,
    })
}

// ---------------------------------------------------------------- discussions

fn valid_discussion_id(id: &str) -> ApiResult<&str> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ApiError::bad_request("invalid discussion id"));
    }
    Ok(id)
}

pub async fn mr_discussions(ctx: &GlCtx, iid: u64) -> ApiResult<Vec<Discussion>> {
    let url = ctx.purl(&format!("/merge_requests/{iid}/discussions"));
    Ok(ctx.get_all::<Discussion>(&url, &[], 5000).await?.0)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NoteBody {
    pub body: String,
}

pub async fn add_note(ctx: &GlCtx, iid: u64, body: &str) -> ApiResult<Note> {
    let body = valid_body(body)?;
    let note: Note = ctx
        .write(Method::POST, &ctx.purl(&format!("/merge_requests/{iid}/notes")), Some(&json!({ "body": body })))
        .await?;
    super::mr_changed(ctx, iid, "note");
    Ok(note)
}

/// A line in the MR diff: `newLine` for added lines, `oldLine` for removed
/// ones, both for unchanged context lines.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LinePosition {
    pub old_path: Option<String>,
    pub new_path: String,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NewDiscussion {
    pub body: String,
    pub position: Option<LinePosition>,
}

/// GitLab's `position` object for a diff-line comment, from the MR's `diff_refs`.
pub fn build_position(refs: &super::model::DiffRefs, pos: &LinePosition) -> ApiResult<Value> {
    let new_path = valid_repo_path(pos.new_path.trim())?;
    let old_path = pos.old_path.as_deref().map(str::trim).filter(|p| !p.is_empty()).unwrap_or(new_path);
    valid_repo_path(old_path)?;
    if pos.old_line.is_none() && pos.new_line.is_none() {
        return Err(ApiError::bad_request("a diff comment needs oldLine or newLine"));
    }
    if pos.old_line == Some(0) || pos.new_line == Some(0) {
        return Err(ApiError::bad_request("line numbers start at 1"));
    }
    let mut p = json!({
        "position_type": "text",
        "base_sha": refs.base_sha,
        "start_sha": refs.start_sha,
        "head_sha": refs.head_sha,
        "old_path": old_path,
        "new_path": new_path,
    });
    if let Some(l) = pos.old_line {
        p["old_line"] = json!(l);
    }
    if let Some(l) = pos.new_line {
        p["new_line"] = json!(l);
    }
    Ok(p)
}

pub async fn add_discussion(ctx: &GlCtx, iid: u64, req: &NewDiscussion) -> ApiResult<Discussion> {
    let body = valid_body(&req.body)?;
    let mut payload = json!({ "body": body });
    if let Some(pos) = &req.position {
        let mr: Mr = ctx.get(&ctx.purl(&format!("/merge_requests/{iid}")), &[]).await?;
        let refs = mr.diff_refs.ok_or_else(|| ApiError::conflict("GitLab has not computed this merge request's diff yet"))?;
        payload["position"] = build_position(&refs, pos)?;
    }
    let d: Discussion =
        ctx.write(Method::POST, &ctx.purl(&format!("/merge_requests/{iid}/discussions")), Some(&payload)).await?;
    super::mr_changed(ctx, iid, "discussion");
    Ok(d)
}

pub async fn reply(ctx: &GlCtx, iid: u64, did: &str, body: &str) -> ApiResult<Note> {
    let did = valid_discussion_id(did)?;
    let body = valid_body(body)?;
    let url = ctx.purl(&format!("/merge_requests/{iid}/discussions/{did}/notes"));
    let note: Note = ctx.write(Method::POST, &url, Some(&json!({ "body": body }))).await?;
    super::mr_changed(ctx, iid, "note");
    Ok(note)
}

pub async fn resolve(ctx: &GlCtx, iid: u64, did: &str, resolved: bool) -> ApiResult<Discussion> {
    let did = valid_discussion_id(did)?;
    let url = ctx.purl(&format!("/merge_requests/{iid}/discussions/{did}"));
    let resp = ctx
        .send(Method::PUT, &url, |rb| rb.query(&[("resolved", if resolved { "true" } else { "false" })]))
        .await?;
    let d: Discussion = ctx.parse(resp).await?;
    super::mr_changed(ctx, iid, "resolve");
    Ok(d)
}

// ---------------------------------------------------------------- approve / merge / rebase

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ApproveBody {
    /// Approve only if the MR head is still this commit.
    pub sha: Option<String>,
}

pub async fn approve(ctx: &GlCtx, iid: u64, approve: bool, sha: Option<&str>) -> ApiResult<Mr> {
    let action = if approve { "approve" } else { "unapprove" };
    let body = match sha.filter(|s| approve && !s.is_empty()) {
        Some(s) if is_hex_sha(s) => Some(json!({ "sha": s })),
        Some(_) => return Err(ApiError::bad_request("invalid sha")),
        None => None,
    };
    let _: Value = ctx.write(Method::POST, &ctx.purl(&format!("/merge_requests/{iid}/{action}")), body.as_ref()).await?;
    super::mr_changed(ctx, iid, action);
    get_mr(ctx, iid).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MergeBody {
    /// Required: the MR head the user reviewed. GitLab refuses (409) if it moved.
    pub sha: String,
    pub squash: Option<bool>,
    pub remove_source_branch: Option<bool>,
    /// Merge when the pipeline succeeds instead of now.
    pub auto_merge: bool,
    pub merge_commit_message: Option<String>,
    pub squash_commit_message: Option<String>,
}

pub fn merge_payload(b: &MergeBody) -> ApiResult<Value> {
    if !is_hex_sha(b.sha.trim()) {
        return Err(ApiError::bad_request("sha (the reviewed head commit) is required to merge"));
    }
    let mut p = json!({ "sha": b.sha.trim() });
    if let Some(s) = b.squash {
        p["squash"] = json!(s);
    }
    if let Some(r) = b.remove_source_branch {
        p["should_remove_source_branch"] = json!(r);
    }
    if b.auto_merge {
        // `auto_merge` on GitLab ≥ 17.11; the deprecated name for older servers,
        // which would otherwise merge immediately.
        p["auto_merge"] = json!(true);
        p["merge_when_pipeline_succeeds"] = json!(true);
    }
    if let Some(m) = b.merge_commit_message.as_deref().filter(|m| !m.trim().is_empty()) {
        p["merge_commit_message"] = json!(m);
    }
    if let Some(m) = b.squash_commit_message.as_deref().filter(|m| !m.trim().is_empty()) {
        p["squash_commit_message"] = json!(m);
    }
    Ok(p)
}

pub async fn merge(ctx: &GlCtx, iid: u64, b: &MergeBody) -> ApiResult<Mr> {
    let payload = merge_payload(b)?;
    let _: Value = ctx.write(Method::PUT, &ctx.purl(&format!("/merge_requests/{iid}/merge")), Some(&payload)).await?;
    super::mr_changed(ctx, iid, if b.auto_merge { "auto_merge" } else { "merged" });
    get_mr(ctx, iid).await
}

pub async fn rebase(ctx: &GlCtx, iid: u64) -> ApiResult<Value> {
    let v: Value = ctx.write(Method::PUT, &ctx.purl(&format!("/merge_requests/{iid}/rebase")), None).await?;
    super::mr_changed(ctx, iid, "rebase");
    Ok(json!({ "rebaseInProgress": v.get("rebase_in_progress").and_then(Value::as_bool).unwrap_or(true) }))
}

pub async fn mr_commits(ctx: &GlCtx, iid: u64) -> ApiResult<Vec<Commit>> {
    Ok(ctx.get_all::<Commit>(&ctx.purl(&format!("/merge_requests/{iid}/commits")), &[], 1000).await?.0)
}

pub async fn mr_pipelines(ctx: &GlCtx, iid: u64) -> ApiResult<Vec<Pipeline>> {
    let q = [("per_page", "30".to_string())];
    let page = ctx.get_page::<Pipeline>(&ctx.purl(&format!("/merge_requests/{iid}/pipelines")), &q).await?;
    Ok(enrich_pipelines(ctx, page.items).await)
}

// ---------------------------------------------------------------- handlers

type Iid = Path<(String, u64)>;

async fn h_list(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>, Query(q): Query<MrsQuery>) -> ApiResult<Json<ListPage<Mr>>> {
    Ok(Json(list_mrs(&ctx(&s, &pid, &repo).await?, &q).await?))
}
async fn h_create(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path(pid): Path<String>, Json(b): Json<CreateMr>) -> ApiResult<Json<Mr>> {
    Ok(Json(create_mr(&ctx(&s, &pid, &repo).await?, &b).await?))
}
async fn h_get(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<Mr>> {
    Ok(Json(get_mr(&ctx(&s, &pid, &repo).await?, iid).await?))
}
async fn h_update(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid, Json(b): Json<UpdateMr>) -> ApiResult<Json<Mr>> {
    Ok(Json(update_mr(&ctx(&s, &pid, &repo).await?, iid, &b).await?))
}
async fn h_diffs(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<MrDiffs>> {
    Ok(Json(mr_diffs(&ctx(&s, &pid, &repo).await?, iid).await?))
}
async fn h_file(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid, Query(q): Query<FileQuery>) -> ApiResult<Json<FileVersions>> {
    Ok(Json(mr_file(&ctx(&s, &pid, &repo).await?, iid, &q).await?))
}
async fn h_discussions(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<Vec<Discussion>>> {
    Ok(Json(mr_discussions(&ctx(&s, &pid, &repo).await?, iid).await?))
}
async fn h_note(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid, Json(b): Json<NoteBody>) -> ApiResult<Json<Note>> {
    Ok(Json(add_note(&ctx(&s, &pid, &repo).await?, iid, &b.body).await?))
}
async fn h_discussion(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid, Json(b): Json<NewDiscussion>) -> ApiResult<Json<Discussion>> {
    Ok(Json(add_discussion(&ctx(&s, &pid, &repo).await?, iid, &b).await?))
}
async fn h_reply(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path((pid, iid, did)): Path<(String, u64, String)>,
    Json(b): Json<NoteBody>,
) -> ApiResult<Json<Note>> {
    Ok(Json(reply(&ctx(&s, &pid, &repo).await?, iid, &did, &b.body).await?))
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ResolveBody {
    resolved: bool,
}
async fn h_resolve(
    State(s): State<AppState>,
    Query(repo): Query<RepoParam>,
    Path((pid, iid, did)): Path<(String, u64, String)>,
    Json(b): Json<ResolveBody>,
) -> ApiResult<Json<Discussion>> {
    Ok(Json(resolve(&ctx(&s, &pid, &repo).await?, iid, &did, b.resolved).await?))
}
async fn h_approve(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid, b: Option<Json<ApproveBody>>) -> ApiResult<Json<Mr>> {
    let sha = b.and_then(|Json(b)| b.sha);
    Ok(Json(approve(&ctx(&s, &pid, &repo).await?, iid, true, sha.as_deref()).await?))
}
async fn h_unapprove(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<Mr>> {
    Ok(Json(approve(&ctx(&s, &pid, &repo).await?, iid, false, None).await?))
}
async fn h_merge(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid, Json(b): Json<MergeBody>) -> ApiResult<Json<Mr>> {
    Ok(Json(merge(&ctx(&s, &pid, &repo).await?, iid, &b).await?))
}
async fn h_rebase(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<Value>> {
    Ok(Json(rebase(&ctx(&s, &pid, &repo).await?, iid).await?))
}
async fn h_commits(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<Vec<Commit>>> {
    Ok(Json(mr_commits(&ctx(&s, &pid, &repo).await?, iid).await?))
}
async fn h_pipelines(State(s): State<AppState>, Query(repo): Query<RepoParam>, Path((pid, iid)): Iid) -> ApiResult<Json<Vec<Pipeline>>> {
    Ok(Json(mr_pipelines(&ctx(&s, &pid, &repo).await?, iid).await?))
}

pub fn routes() -> Router<AppState> {
    let p = "/api/projects/{pid}/gitlab/mrs";
    Router::new()
        .route(p, get(h_list).post(h_create))
        .route(&format!("{p}/{{iid}}"), get(h_get).put(h_update))
        .route(&format!("{p}/{{iid}}/diffs"), get(h_diffs))
        .route(&format!("{p}/{{iid}}/file"), get(h_file))
        .route(&format!("{p}/{{iid}}/discussions"), get(h_discussions).post(h_discussion))
        .route(&format!("{p}/{{iid}}/discussions/{{did}}"), put(h_resolve))
        .route(&format!("{p}/{{iid}}/discussions/{{did}}/notes"), post(h_reply))
        .route(&format!("{p}/{{iid}}/notes"), post(h_note))
        .route(&format!("{p}/{{iid}}/approve"), post(h_approve))
        .route(&format!("{p}/{{iid}}/unapprove"), post(h_unapprove))
        .route(&format!("{p}/{{iid}}/merge"), post(h_merge))
        .route(&format!("{p}/{{iid}}/rebase"), post(h_rebase))
        .route(&format!("{p}/{{iid}}/commits"), get(h_commits))
        .route(&format!("{p}/{{iid}}/pipelines"), get(h_pipelines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitlab::model::DiffRefs;

    #[test]
    fn draft_prefix_round_trips() {
        assert!(is_draft_title("Draft: x"));
        assert!(is_draft_title("[Draft] x"));
        assert!(is_draft_title("(draft) x"));
        assert!(!is_draft_title("Drafting the plan"));
        assert_eq!(set_draft("Fix it", true), "Draft: Fix it");
        assert_eq!(set_draft("Draft: Fix it", true), "Draft: Fix it");
        assert_eq!(set_draft("[Draft] Draft: Fix it", false), "Fix it");
        assert_eq!(set_draft("Fix it", false), "Fix it");
    }

    #[test]
    fn diff_line_positions_come_from_diff_refs() {
        let refs = DiffRefs { base_sha: "b".into(), head_sha: "h".into(), start_sha: "s".into() };
        let p = build_position(&refs, &LinePosition { new_path: "src/a.rs".into(), new_line: Some(12), ..Default::default() }).unwrap();
        assert_eq!(p["base_sha"], "b");
        assert_eq!(p["start_sha"], "s");
        assert_eq!(p["head_sha"], "h");
        assert_eq!(p["old_path"], "src/a.rs");
        assert_eq!(p["new_line"], 12);
        assert!(p.get("old_line").is_none());
        let ctxline = build_position(
            &refs,
            &LinePosition { old_path: Some("old.rs".into()), new_path: "new.rs".into(), old_line: Some(3), new_line: Some(4) },
        )
        .unwrap();
        assert_eq!((ctxline["old_path"].as_str(), ctxline["old_line"].as_u64()), (Some("old.rs"), Some(3)));
        assert!(build_position(&refs, &LinePosition { new_path: "a".into(), ..Default::default() }).is_err());
        assert!(build_position(&refs, &LinePosition { new_path: "a".into(), new_line: Some(0), ..Default::default() }).is_err());
    }

    #[test]
    fn merge_requires_the_reviewed_sha() {
        assert!(merge_payload(&MergeBody::default()).is_err());
        let p = merge_payload(&MergeBody {
            sha: "5babfd548d64a14eabeba53b847fdec5fa5f0ca9".into(),
            squash: Some(true),
            remove_source_branch: Some(false),
            auto_merge: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(p["squash"], true);
        assert_eq!(p["should_remove_source_branch"], false);
        assert_eq!(p["auto_merge"], true);
        assert_eq!(p["merge_when_pipeline_succeeds"], true);
        let p = merge_payload(&MergeBody { sha: "5babfd54".into(), ..Default::default() }).unwrap();
        assert!(p.get("auto_merge").is_none());
    }

    #[test]
    fn branch_names_are_checked() {
        assert!(valid_branch("feature/x-1").is_ok());
        assert!(valid_branch("-rf").is_err());
        assert!(valid_branch("a b").is_err());
        assert!(valid_branch("").is_err());
        assert!(valid_branch("a/../b").is_err());
        assert!(valid_branch("..").is_err());
        assert!(valid_repo_path("src/../x").is_err());
        assert!(valid_repo_path("..").is_err());
        assert!(valid_repo_path("src/a..b.rs").is_ok());
    }
}

//! Pull requests: list (and search), detail with checks and review states,
//! changed files and both versions of a file for side-by-side review, review
//! threads (resolve through GraphQL when there is a token), conversation,
//! commits, comments and replies, approve / request changes, merge with a
//! head-sha guard, create and update.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ci::{commit_checks, is_hex_sha, valid_branch};
use super::client::{Fresh, GhCtx, ctx};
use super::model::{Commit, IssueComment, ListPage, PrFile, Pull, PullDetail, RawCommit, Review, ReviewComment, Thread, group_threads, review_states};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Largest file version we send to the diff viewer.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
/// A file's patch beyond this is dropped (the viewer diffs whole files anyway).
const MAX_PATCH: usize = 1024 * 1024;
/// GitHub's own limit on a comment body.
const MAX_BODY: usize = 65_536;

// ---------------------------------------------------------------- validation

pub fn valid_body(b: &str) -> ApiResult<&str> {
    if b.trim().is_empty() {
        return Err(ApiError::bad_request("the comment is empty"));
    }
    if b.chars().count() > MAX_BODY {
        return Err(ApiError::bad_request("the comment is too long"));
    }
    Ok(b)
}

fn valid_title(t: &str) -> ApiResult<String> {
    let t = t.trim();
    if t.is_empty() {
        return Err(ApiError::bad_request("a title is required"));
    }
    Ok(t.chars().take(256).collect())
}

/// A repository file path for URLs and `git cat-file`.
pub fn valid_repo_file(p: &str) -> ApiResult<&str> {
    if p.is_empty()
        || p.len() > 4096
        || p.starts_with('/')
        || p.contains(['\0', '\n', '\r'])
        || p.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(ApiError::bad_request("invalid file path"));
    }
    Ok(p)
}

// ---------------------------------------------------------------- files at a commit

pub enum FileText {
    Text(String),
    Binary,
    TooLarge,
    Missing,
}

fn classify(bytes: Vec<u8>, too_large: bool) -> FileText {
    if too_large {
        return FileText::TooLarge;
    }
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return FileText::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(t) => FileText::Text(t),
        Err(_) => FileText::Binary,
    }
}

/// `git cat-file` from the local clone: free, and often the commits are there.
async fn local_blob(root: &std::path::Path, sha: &str, path: &str, cap: usize) -> Option<FileText> {
    if !is_hex_sha(sha) || sha.len() < 40 {
        return None;
    }
    let spec = format!("{sha}:{path}");
    let run = |args: Vec<String>| {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(&args)
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        crate::util::proc::clean_env(&mut cmd);
        async move { tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok() }
    };
    // Is the commit here at all? Then a missing path means the file is absent there.
    let commit = run(vec!["cat-file".into(), "-e".into(), format!("{sha}^{{commit}}")]).await?;
    if !commit.status.success() {
        return None;
    }
    let size = run(vec!["cat-file".into(), "-s".into(), spec.clone()]).await?;
    if !size.status.success() {
        return Some(FileText::Missing);
    }
    let n: usize = String::from_utf8_lossy(&size.stdout).trim().parse().ok()?;
    if n > cap {
        return Some(FileText::TooLarge);
    }
    let blob = run(vec!["cat-file".into(), "blob".into(), spec]).await?;
    if !blob.status.success() {
        return None;
    }
    Some(classify(blob.stdout, false))
}

/// A file at a commit (or ref): the local clone first, then GitHub (raw
/// content, not counted against the quota when anonymous on github.com).
pub async fn file_at(ctx: &GhCtx, path: &str, git_ref: &str, cap: usize) -> ApiResult<FileText> {
    let path = valid_repo_file(path)?;
    if let Some(f) = local_blob(&ctx.project.root, git_ref, path, cap).await {
        return Ok(f);
    }
    let got = if ctx.is_anonymous() && ctx.is_github_com() {
        ctx.raw_public(&ctx.owner, &ctx.repo, git_ref, path, cap).await?
    } else {
        let enc: Vec<String> = path.split('/').map(|s| urlencoding::encode(s).into_owned()).collect();
        let url = format!("{}?ref={}", ctx.rurl(&format!("/contents/{}", enc.join("/"))), urlencoding::encode(git_ref));
        ctx.get_bytes(&url, "application/vnd.github.raw+json", cap).await?
    };
    Ok(match got {
        None => FileText::Missing,
        Some((bytes, too_large)) => classify(bytes, too_large),
    })
}

// ---------------------------------------------------------------- list / detail

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PullsQuery {
    /// `open` (default) | `closed` | `merged` | `all`
    pub state: Option<String>,
    pub search: Option<String>,
    /// Branch name (in this repository).
    pub head: Option<String>,
    pub base: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

/// Search API results are issue-shaped: map what a list row needs.
fn pull_from_search(mut v: Value) -> Option<Pull> {
    let merged_at = v.pointer("/pull_request/merged_at").and_then(Value::as_str).map(str::to_string);
    if let Some(o) = v.as_object_mut() {
        o.remove("pull_request");
    }
    let mut p: Pull = serde_json::from_value(v).ok()?;
    p.merged = merged_at.is_some();
    p.merged_at = merged_at;
    Some(p)
}

pub async fn list_pulls(ctx: &GhCtx, q: &PullsQuery) -> ApiResult<ListPage<Pull>> {
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let per_page = q.per_page.unwrap_or(25).clamp(1, 100);
    let state = q.state.as_deref().filter(|s| !s.is_empty()).unwrap_or("open");
    if !matches!(state, "open" | "closed" | "merged" | "all") {
        return Err(ApiError::bad_request(format!("unknown pull request state {state:?}")));
    }
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if search.is_some() || state == "merged" {
        let mut terms = format!("repo:{} is:pr", ctx.full_name());
        if state != "all" {
            terms.push_str(&format!(" is:{state}"));
        }
        if let Some(s) = search {
            terms.push(' ');
            terms.push_str(&s.chars().filter(|c| !c.is_control()).take(200).collect::<String>());
        }
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
        let items: Vec<Pull> = match body["items"].take() {
            Value::Array(a) => a.into_iter().filter_map(pull_from_search).collect(),
            _ => vec![],
        };
        return Ok(ListPage { items, page, next_page: res.next_url.map(|_| page + 1), total });
    }
    let mut query: Vec<(&str, String)> = vec![
        ("state", state.to_string()),
        ("sort", "updated".into()),
        ("direction", "desc".into()),
        ("page", page.to_string()),
        ("per_page", per_page.to_string()),
    ];
    if let Some(h) = q.head.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        query.push(("head", format!("{}:{}", ctx.owner, valid_branch(h)?)));
    }
    if let Some(b) = q.base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        query.push(("base", valid_branch(b)?.to_string()));
    }
    let res = ctx.get_page::<Vec<Pull>>(&ctx.rurl("/pulls"), &query, Fresh::Live).await?;
    let mut items = res.body;
    for p in &mut items {
        p.merged = p.merged || p.merged_at.is_some();
    }
    Ok(ListPage { items, page, next_page: res.next_url.map(|_| page + 1), total: None })
}

/// The newest pull request whose head is `branch` of this repository (open or not).
pub async fn pr_for_branch(ctx: &GhCtx, branch: &str) -> ApiResult<Option<Pull>> {
    let q = [
        ("head", format!("{}:{}", ctx.owner, valid_branch(branch)?)),
        ("state", "all".to_string()),
        ("per_page", "1".to_string()),
    ];
    let res = ctx.get_page::<Vec<Pull>>(&ctx.rurl("/pulls"), &q, Fresh::Live).await?;
    Ok(res.body.into_iter().next().map(|mut p| {
        p.merged = p.merged || p.merged_at.is_some();
        p
    }))
}

/// Open pull requests (one item fetched; the `last` link gives the count).
pub async fn open_pr_count(ctx: &GhCtx) -> ApiResult<u64> {
    let q = [("state", "open".to_string()), ("per_page", "1".to_string())];
    let res = ctx.get_page::<Vec<Value>>(&ctx.rurl("/pulls"), &q, Fresh::Live).await?;
    Ok(res.last_page.map(u64::from).unwrap_or(res.body.len() as u64))
}

pub async fn get_pull_only(ctx: &GhCtx, n: u64) -> ApiResult<Pull> {
    let mut p: Pull = ctx.get(&ctx.rurl(&format!("/pulls/{n}")), &[], Fresh::Live).await?;
    p.merged = p.merged || p.merged_at.is_some();
    Ok(p)
}

/// The merge base of `base` and `head`: from the local clone when it has both
/// commits, else GitHub's compare API. Cached (commits never change).
pub async fn merge_base(ctx: &GhCtx, base: &str, head: &str) -> ApiResult<String> {
    if !is_hex_sha(base) || !is_hex_sha(head) {
        return Err(ApiError::bad_request("merge base needs two commit shas"));
    }
    let key = format!("{}|{base}..{head}", ctx.api);
    if let Some(m) = ctx.state.github.merge_bases.lock().get(&key) {
        return Ok(m.clone());
    }
    let mut found = None;
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["merge-base", base, head])
        .current_dir(&ctx.project.root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Ok(out) = crate::util::proc::run_cmd(cmd, Duration::from_secs(10)).await {
        let s = out.stdout.trim();
        if out.ok() && is_hex_sha(s) {
            found = Some(s.to_string());
        }
    }
    let m = match found {
        Some(m) => m,
        None => {
            let url = ctx.rurl(&format!("/compare/{base}...{head}"));
            let v: Value = ctx.get(&url, &[("per_page", "1".to_string())], Fresh::Fixed).await?;
            v.pointer("/merge_base_commit/sha")
                .and_then(Value::as_str)
                .filter(|s| is_hex_sha(s))
                .map(str::to_string)
                .ok_or_else(|| ApiError::upstream("GitHub did not say where the branches meet"))?
        }
    };
    let mut map = ctx.state.github.merge_bases.lock();
    if map.len() > 500 {
        map.clear();
    }
    map.insert(key, m.clone());
    Ok(m)
}

/// The commit a pull request's changes are diffed against: where its head and
/// its recorded base meet, as GitHub's "Files changed" does, merged or not.
/// (For a merged pull request GitHub freezes `base.sha` at the base tip just
/// before the merge, which is usually past the branch point.) Only when a
/// merged head is already contained in that base (merged by a push) is the
/// meeting point the head itself, which would show nothing: then the recorded
/// base is the better guess.
fn diff_base(merged: bool, recorded: &str, head: &str, merge_base: String) -> String {
    if merged && merge_base.eq_ignore_ascii_case(head) && !recorded.is_empty() {
        recorded.to_string()
    } else {
        merge_base
    }
}

pub async fn get_pull(ctx: &GhCtx, n: u64) -> ApiResult<PullDetail> {
    let pull = get_pull_only(ctx, n).await?;
    let head = pull.head.as_ref().map(|h| h.sha.clone()).unwrap_or_default();
    let base = pull.base.as_ref().map(|b| b.sha.clone()).unwrap_or_default();
    let reviews_url = ctx.rurl(&format!("/pulls/{n}/reviews"));
    let (checks, reviews, mb) = tokio::join!(
        async {
            if is_hex_sha(&head) { commit_checks(ctx, &head).await.map(Some) } else { Ok(None) }
        },
        ctx.get_all::<Review>(&reviews_url, &[], 1000, None, Fresh::Live),
        async {
            if !is_hex_sha(&base) || !is_hex_sha(&head) { Ok(None) } else { merge_base(ctx, &base, &head).await.map(Some) }
        },
    );
    let mut warnings = vec![];
    let checks = checks.unwrap_or_else(|e| {
        warnings.push(format!("checks: {}", e.message));
        None
    });
    let review_states = match reviews {
        Ok((r, _)) => review_states(&r),
        Err(e) => {
            warnings.push(format!("reviews: {}", e.message));
            vec![]
        }
    };
    let merge_base_sha = match mb {
        Ok(Some(m)) => Some(diff_base(pull.merged, &base, &head, m)),
        Ok(None) => (!base.is_empty()).then_some(base),
        Err(e) => {
            warnings.push(format!("merge base: {}", e.message));
            (!base.is_empty()).then_some(base)
        }
    };
    Ok(PullDetail { pull, checks, review_states, merge_base_sha, warnings })
}

// ---------------------------------------------------------------- files

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrFiles {
    pub files: Vec<PrFile>,
    /// More files exist than GitHub lists (it stops at 3000).
    pub truncated: bool,
}

pub async fn pr_files(ctx: &GhCtx, n: u64) -> ApiResult<PrFiles> {
    let (mut files, truncated) = ctx.get_all::<PrFile>(&ctx.rurl(&format!("/pulls/{n}/files")), &[], 3000, None, Fresh::Live).await?;
    for f in &mut files {
        if f.patch.as_ref().is_some_and(|p| p.len() > MAX_PATCH) {
            f.patch = None;
            f.too_large = true;
        }
    }
    Ok(PrFiles { files, truncated })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileQuery {
    pub path: String,
    pub previous_path: Option<String>,
    /// GitHub's file status (`added`, `removed`…).
    pub status: Option<String>,
    /// Merge base and head; looked up from the pull request when absent.
    pub base: Option<String>,
    pub head: Option<String>,
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

/// Both sides of one changed file: the merge base and the head.
pub async fn pr_file(ctx: &GhCtx, n: u64, q: &FileQuery) -> ApiResult<FileVersions> {
    let new_path = valid_repo_file(q.path.trim())?;
    let old_path = match q.previous_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => valid_repo_file(p)?,
        None => new_path,
    };
    let (base, head) = match (q.base.as_deref(), q.head.as_deref()) {
        (Some(b), Some(h)) if is_hex_sha(b) && is_hex_sha(h) => (b.to_string(), h.to_string()),
        _ => {
            let p = get_pull_only(ctx, n).await?;
            let head = p.head.map(|h| h.sha).unwrap_or_default();
            let recorded = p.base.map(|b| b.sha).unwrap_or_default();
            if !is_hex_sha(&head) || !is_hex_sha(&recorded) {
                return Err(ApiError::conflict("GitHub has not computed this pull request's commits yet"));
            }
            // As `get_pull` does: where the branches meet (a merged pull request
            // falls back to its recorded base when that cannot be computed).
            let base = match merge_base(ctx, &recorded, &head).await {
                Ok(m) => diff_base(p.merged, &recorded, &head, m),
                Err(_) if p.merged => recorded,
                Err(e) => return Err(e),
            };
            (base, head)
        }
    };
    let status = q.status.as_deref().unwrap_or("modified");
    let original = async {
        if status == "added" { Ok(FileText::Missing) } else { file_at(ctx, old_path, &base, MAX_FILE_BYTES).await }
    };
    let modified = async {
        if status == "removed" { Ok(FileText::Missing) } else { file_at(ctx, new_path, &head, MAX_FILE_BYTES).await }
    };
    let (original, modified) = tokio::join!(original, modified);
    let (original, modified) = (original?, modified?);
    let mut out = FileVersions { base_sha: base, head_sha: head, ..Default::default() };
    for (f, slot) in [(original, &mut out.original), (modified, &mut out.modified)] {
        match f {
            FileText::Text(t) => *slot = t,
            FileText::Binary => out.binary = true,
            FileText::TooLarge => out.too_large = true,
            FileText::Missing => {}
        }
    }
    if out.binary || out.too_large {
        out.original.clear();
        out.modified.clear();
    }
    Ok(out)
}

// ---------------------------------------------------------------- threads, reviews, conversation

const THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { id isResolved resolvedBy { login } viewerCanResolve viewerCanUnresolve comments(first: 1) { nodes { databaseId } } } } } } }";

/// Review threads: REST comments grouped by reply chain; with a token, GraphQL
/// adds each thread's id and resolution state.
pub async fn threads(ctx: &GhCtx, n: u64) -> ApiResult<Vec<Thread>> {
    let url = ctx.rurl(&format!("/pulls/{n}/comments"));
    let (comments, _) = ctx.get_all::<ReviewComment>(&url, &[], 3000, None, Fresh::Live).await?;
    let mut threads = group_threads(comments);
    if ctx.is_anonymous() || threads.is_empty() {
        return Ok(threads);
    }
    let mut after: Option<String> = None;
    for _ in 0..10 {
        let vars = json!({ "owner": ctx.owner, "name": ctx.repo, "number": n, "after": after });
        let Ok(data) = ctx.graphql(THREADS_QUERY, vars).await else { break };
        let rt = &data["repository"]["pullRequest"]["reviewThreads"];
        for node in rt["nodes"].as_array().into_iter().flatten() {
            let Some(root) = node.pointer("/comments/nodes/0/databaseId").and_then(Value::as_u64) else { continue };
            if let Some(t) = threads.iter_mut().find(|t| t.root_id == root) {
                if let Some(id) = node["id"].as_str() {
                    t.id = id.to_string();
                }
                t.resolved = node["isResolved"].as_bool();
                t.resolved_by = node.pointer("/resolvedBy/login").and_then(Value::as_str).map(str::to_string);
                t.can_resolve = node["viewerCanResolve"].as_bool().unwrap_or(false)
                    || node["viewerCanUnresolve"].as_bool().unwrap_or(false);
            }
        }
        if rt.pointer("/pageInfo/hasNextPage").and_then(Value::as_bool) != Some(true) {
            break;
        }
        after = rt.pointer("/pageInfo/endCursor").and_then(Value::as_str).map(str::to_string);
    }
    Ok(threads)
}

pub async fn reviews(ctx: &GhCtx, n: u64) -> ApiResult<Vec<Review>> {
    Ok(ctx.get_all::<Review>(&ctx.rurl(&format!("/pulls/{n}/reviews")), &[], 1000, None, Fresh::Live).await?.0)
}

pub async fn issue_comments(ctx: &GhCtx, n: u64) -> ApiResult<Vec<IssueComment>> {
    Ok(ctx.get_all::<IssueComment>(&ctx.rurl(&format!("/issues/{n}/comments")), &[], 2000, None, Fresh::Live).await?.0)
}

pub async fn commits(ctx: &GhCtx, n: u64) -> ApiResult<Vec<Commit>> {
    let (raw, _) = ctx.get_all::<RawCommit>(&ctx.rurl(&format!("/pulls/{n}/commits")), &[], 250, None, Fresh::Live).await?;
    Ok(raw.into_iter().map(Commit::from).collect())
}

// ---------------------------------------------------------------- comments

pub async fn add_comment(ctx: &GhCtx, n: u64, body: &str) -> ApiResult<IssueComment> {
    let body = valid_body(body)?;
    let c: IssueComment = ctx.write(Method::POST, &ctx.rurl(&format!("/issues/{n}/comments")), Some(&json!({ "body": body }))).await?;
    super::pr_changed(ctx, n, "comment");
    Ok(c)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NewReviewComment {
    pub body: String,
    pub path: String,
    /// Line in the file (new side for `RIGHT`, old side for `LEFT`); absent = a file comment.
    pub line: Option<u64>,
    /// `RIGHT` (default) | `LEFT`
    pub side: Option<String>,
    pub start_line: Option<u64>,
    pub start_side: Option<String>,
    /// The head commit the user saw; defaults to the pull request's head.
    pub commit_id: Option<String>,
}

pub fn review_comment_payload(c: &NewReviewComment, head: &str) -> ApiResult<Value> {
    let body = valid_body(&c.body)?;
    let path = valid_repo_file(c.path.trim())?;
    let commit = c.commit_id.as_deref().filter(|s| !s.is_empty()).unwrap_or(head);
    if !is_hex_sha(commit) {
        return Err(ApiError::bad_request("invalid commit id"));
    }
    let side = |s: Option<&str>| -> ApiResult<&'static str> {
        match s.unwrap_or("RIGHT") {
            "RIGHT" => Ok("RIGHT"),
            "LEFT" => Ok("LEFT"),
            other => Err(ApiError::bad_request(format!("side must be LEFT or RIGHT, not {other:?}"))),
        }
    };
    let mut p = json!({ "body": body, "commit_id": commit, "path": path });
    match c.line {
        Some(0) => return Err(ApiError::bad_request("line numbers start at 1")),
        Some(line) => {
            p["line"] = json!(line);
            p["side"] = json!(side(c.side.as_deref())?);
            if let Some(start) = c.start_line.filter(|s| *s != line) {
                if start == 0 || start > line {
                    return Err(ApiError::bad_request("startLine must be before line"));
                }
                p["start_line"] = json!(start);
                p["start_side"] = json!(side(c.start_side.as_deref().or(c.side.as_deref()))?);
            }
        }
        None => p["subject_type"] = json!("file"),
    }
    Ok(p)
}

pub async fn add_review_comment(ctx: &GhCtx, n: u64, c: &NewReviewComment) -> ApiResult<ReviewComment> {
    let head = match c.commit_id.as_deref().filter(|s| !s.is_empty()) {
        Some(h) => h.to_string(),
        None => get_pull_only(ctx, n).await?.head.map(|h| h.sha).unwrap_or_default(),
    };
    let payload = review_comment_payload(c, &head)?;
    let rc: ReviewComment = ctx.write(Method::POST, &ctx.rurl(&format!("/pulls/{n}/comments")), Some(&payload)).await?;
    super::pr_changed(ctx, n, "review_comment");
    Ok(rc)
}

pub async fn reply(ctx: &GhCtx, n: u64, comment_id: u64, body: &str) -> ApiResult<ReviewComment> {
    let body = valid_body(body)?;
    let url = ctx.rurl(&format!("/pulls/{n}/comments/{comment_id}/replies"));
    let rc: ReviewComment = ctx.write(Method::POST, &url, Some(&json!({ "body": body }))).await?;
    super::pr_changed(ctx, n, "review_comment");
    Ok(rc)
}

fn valid_node_id(id: &str) -> ApiResult<&str> {
    if id.is_empty() || id.len() > 200 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=')) {
        return Err(ApiError::bad_request("invalid thread id"));
    }
    if id.starts_with('c') && id[1..].chars().all(|c| c.is_ascii_digit()) {
        return Err(ApiError::not_configured("resolving review threads needs a GitHub token"));
    }
    Ok(id)
}

pub async fn resolve(ctx: &GhCtx, n: u64, thread_id: &str, resolved: bool) -> ApiResult<Value> {
    let id = valid_node_id(thread_id)?;
    let m = if resolved { "resolveReviewThread" } else { "unresolveReviewThread" };
    let q = format!("mutation($id: ID!) {{ {m}(input: {{threadId: $id}}) {{ thread {{ id isResolved }} }} }}");
    let data = ctx.graphql(&q, json!({ "id": id })).await?;
    ctx.forget_cached();
    super::pr_changed(ctx, n, "resolve");
    Ok(json!({ "id": id, "resolved": data[m]["thread"]["isResolved"].as_bool().unwrap_or(resolved) }))
}

// ---------------------------------------------------------------- review / merge

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReviewBody {
    /// `APPROVE` | `REQUEST_CHANGES` | `COMMENT`
    pub event: String,
    pub body: Option<String>,
    /// Review this head commit (the one the user saw).
    pub sha: Option<String>,
}

pub fn review_payload(b: &ReviewBody) -> ApiResult<Value> {
    let event = b.event.trim().to_ascii_uppercase();
    if !matches!(event.as_str(), "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
        return Err(ApiError::bad_request("event must be APPROVE, REQUEST_CHANGES or COMMENT"));
    }
    let body = b.body.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if event != "APPROVE" && body.is_none() {
        return Err(ApiError::bad_request("say what should change (a review comment is required)"));
    }
    let mut p = json!({ "event": event });
    if let Some(body) = body {
        p["body"] = json!(valid_body(body)?);
    }
    if let Some(sha) = b.sha.as_deref().filter(|s| !s.is_empty()) {
        if !is_hex_sha(sha) {
            return Err(ApiError::bad_request("invalid sha"));
        }
        p["commit_id"] = json!(sha);
    }
    Ok(p)
}

pub async fn review(ctx: &GhCtx, n: u64, b: &ReviewBody) -> ApiResult<Review> {
    let payload = review_payload(b)?;
    let r: Review = ctx.write(Method::POST, &ctx.rurl(&format!("/pulls/{n}/reviews")), Some(&payload)).await?;
    super::pr_changed(ctx, n, "review");
    Ok(r)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MergeBody {
    /// Required: the head the user reviewed. GitHub refuses (409) if it moved.
    pub sha: String,
    /// `merge` | `squash` | `rebase`
    pub method: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    /// Delete the head branch afterwards (same-repository branches only).
    pub delete_branch: bool,
}

pub fn merge_payload(b: &MergeBody) -> ApiResult<Value> {
    if !is_hex_sha(b.sha.trim()) {
        return Err(ApiError::bad_request("sha (the reviewed head commit) is required to merge"));
    }
    let method = b.method.as_deref().unwrap_or("merge");
    if !matches!(method, "merge" | "squash" | "rebase") {
        return Err(ApiError::bad_request("method must be merge, squash or rebase"));
    }
    let mut p = json!({ "sha": b.sha.trim(), "merge_method": method });
    if let Some(t) = b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        p["commit_title"] = json!(t);
    }
    if let Some(m) = b.message.as_deref().filter(|m| !m.trim().is_empty()) {
        p["commit_message"] = json!(m);
    }
    Ok(p)
}

pub async fn merge(ctx: &GhCtx, n: u64, b: &MergeBody) -> ApiResult<PullDetail> {
    let payload = merge_payload(b)?;
    let _: Value = ctx.write(Method::PUT, &ctx.rurl(&format!("/pulls/{n}/merge")), Some(&payload)).await?;
    super::pr_changed(ctx, n, "merged");
    let mut detail = get_pull(ctx, n).await?;
    if b.delete_branch {
        let p = &detail.pull;
        let same_repo = matches!((&p.head, &p.base), (Some(h), Some(bs))
            if h.repo.as_ref().map(|r| r.id) == bs.repo.as_ref().map(|r| r.id) && h.repo.is_some());
        let branch = p.head.as_ref().map(|h| h.git_ref.clone()).unwrap_or_default();
        if !same_repo {
            detail.warnings.push("the branch lives in a fork; delete it there".into());
        } else if Some(branch.as_str()) == ctx.default_branch() || valid_branch(&branch).is_err() {
            detail.warnings.push(format!("kept {branch}"));
        } else {
            let enc: Vec<String> = branch.split('/').map(|s| urlencoding::encode(s).into_owned()).collect();
            let url = ctx.rurl(&format!("/git/refs/heads/{}", enc.join("/")));
            if let Err(e) = ctx.write::<Value>(Method::DELETE, &url, None).await {
                detail.warnings.push(format!("could not delete {branch}: {}", e.message));
            }
        }
    }
    Ok(detail)
}

// ---------------------------------------------------------------- create / update

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreatePr {
    pub title: String,
    pub body: Option<String>,
    /// Default: the project's current local branch.
    pub head: Option<String>,
    /// Default: the repository's default branch.
    pub base: Option<String>,
    pub draft: bool,
}

pub async fn create_pr(ctx: &GhCtx, b: &CreatePr) -> ApiResult<Pull> {
    let head = match b.head.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => h.to_string(),
        None => crate::util::git::current_branch(&ctx.project.root)
            .await
            .ok_or_else(|| ApiError::bad_request("the project is not on a branch; pass head"))?,
    };
    let base = match b.base.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(x) => x.to_string(),
        None => ctx
            .default_branch()
            .map(str::to_string)
            .ok_or_else(|| ApiError::bad_request("the repository has no default branch; pass base"))?,
    };
    valid_branch(&head)?;
    valid_branch(&base)?;
    if head == base {
        return Err(ApiError::bad_request(format!("head and base are both {head}")));
    }
    let title = valid_title(&b.title)?;
    let mut payload = json!({ "title": title, "head": head, "base": base, "draft": b.draft });
    if let Some(body) = b.body.as_deref().filter(|s| !s.trim().is_empty()) {
        if body.chars().count() > MAX_BODY {
            return Err(ApiError::bad_request("the description is too long"));
        }
        payload["body"] = json!(body);
    }
    let p: Pull = ctx.write(Method::POST, &ctx.rurl("/pulls"), Some(&payload)).await?;
    super::pr_changed(ctx, p.number, "created");
    Ok(p)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdatePr {
    pub title: Option<String>,
    pub body: Option<String>,
    /// `open` | `closed`
    pub state: Option<String>,
    pub base: Option<String>,
    /// Convert to draft / mark ready for review (GraphQL).
    pub draft: Option<bool>,
}

pub async fn update_pr(ctx: &GhCtx, n: u64, b: &UpdatePr) -> ApiResult<PullDetail> {
    let mut payload = serde_json::Map::new();
    if let Some(t) = &b.title {
        payload.insert("title".into(), json!(valid_title(t)?));
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
    if let Some(base) = &b.base {
        payload.insert("base".into(), json!(valid_branch(base)?));
    }
    if payload.is_empty() && b.draft.is_none() {
        return Err(ApiError::bad_request("nothing to update"));
    }
    if !payload.is_empty() {
        let _: Value = ctx.write(Method::PATCH, &ctx.rurl(&format!("/pulls/{n}")), Some(&Value::Object(payload))).await?;
    }
    if let Some(draft) = b.draft {
        let p = get_pull_only(ctx, n).await?;
        if p.draft != draft {
            let m = if draft { "convertPullRequestToDraft" } else { "markPullRequestReadyForReview" };
            let q = format!("mutation($id: ID!) {{ {m}(input: {{pullRequestId: $id}}) {{ pullRequest {{ isDraft }} }} }}");
            ctx.graphql(&q, json!({ "id": p.node_id })).await?;
            ctx.forget_cached();
        }
    }
    super::pr_changed(ctx, n, "updated");
    get_pull(ctx, n).await
}

// ---------------------------------------------------------------- handlers

type N = Path<(String, u64)>;

async fn h_list(State(s): State<AppState>, Path(pid): Path<String>, Query(q): Query<PullsQuery>) -> ApiResult<Json<ListPage<Pull>>> {
    Ok(Json(list_pulls(&ctx(&s, &pid).await?, &q).await?))
}
async fn h_create(State(s): State<AppState>, Path(pid): Path<String>, Json(b): Json<CreatePr>) -> ApiResult<Json<Pull>> {
    Ok(Json(create_pr(&ctx(&s, &pid).await?, &b).await?))
}
async fn h_get(State(s): State<AppState>, Path((pid, n)): N) -> ApiResult<Json<PullDetail>> {
    Ok(Json(get_pull(&ctx(&s, &pid).await?, n).await?))
}
async fn h_update(State(s): State<AppState>, Path((pid, n)): N, Json(b): Json<UpdatePr>) -> ApiResult<Json<PullDetail>> {
    Ok(Json(update_pr(&ctx(&s, &pid).await?, n, &b).await?))
}
async fn h_files(State(s): State<AppState>, Path((pid, n)): N) -> ApiResult<Json<PrFiles>> {
    Ok(Json(pr_files(&ctx(&s, &pid).await?, n).await?))
}
async fn h_file(State(s): State<AppState>, Path((pid, n)): N, Query(q): Query<FileQuery>) -> ApiResult<Json<FileVersions>> {
    Ok(Json(pr_file(&ctx(&s, &pid).await?, n, &q).await?))
}
async fn h_threads(State(s): State<AppState>, Path((pid, n)): N) -> ApiResult<Json<Vec<Thread>>> {
    Ok(Json(threads(&ctx(&s, &pid).await?, n).await?))
}
async fn h_review_comment(State(s): State<AppState>, Path((pid, n)): N, Json(b): Json<NewReviewComment>) -> ApiResult<Json<ReviewComment>> {
    Ok(Json(add_review_comment(&ctx(&s, &pid).await?, n, &b).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BodyOnly {
    body: String,
}

async fn h_reply(State(s): State<AppState>, Path((pid, n, cid)): Path<(String, u64, u64)>, Json(b): Json<BodyOnly>) -> ApiResult<Json<ReviewComment>> {
    Ok(Json(reply(&ctx(&s, &pid).await?, n, cid, &b.body).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ResolveBody {
    resolved: bool,
}

async fn h_resolve(
    State(s): State<AppState>,
    Path((pid, n, tid)): Path<(String, u64, String)>,
    Json(b): Json<ResolveBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(resolve(&ctx(&s, &pid).await?, n, &tid, b.resolved).await?))
}
async fn h_reviews(State(s): State<AppState>, Path((pid, n)): N) -> ApiResult<Json<Vec<Review>>> {
    Ok(Json(reviews(&ctx(&s, &pid).await?, n).await?))
}
async fn h_review(State(s): State<AppState>, Path((pid, n)): N, Json(b): Json<ReviewBody>) -> ApiResult<Json<Review>> {
    Ok(Json(review(&ctx(&s, &pid).await?, n, &b).await?))
}
async fn h_comments(State(s): State<AppState>, Path((pid, n)): N) -> ApiResult<Json<Vec<IssueComment>>> {
    Ok(Json(issue_comments(&ctx(&s, &pid).await?, n).await?))
}
async fn h_comment(State(s): State<AppState>, Path((pid, n)): N, Json(b): Json<BodyOnly>) -> ApiResult<Json<IssueComment>> {
    Ok(Json(add_comment(&ctx(&s, &pid).await?, n, &b.body).await?))
}
async fn h_commits(State(s): State<AppState>, Path((pid, n)): N) -> ApiResult<Json<Vec<Commit>>> {
    Ok(Json(commits(&ctx(&s, &pid).await?, n).await?))
}
async fn h_merge(State(s): State<AppState>, Path((pid, n)): N, Json(b): Json<MergeBody>) -> ApiResult<Json<PullDetail>> {
    Ok(Json(merge(&ctx(&s, &pid).await?, n, &b).await?))
}

pub fn routes() -> Router<AppState> {
    let p = "/api/projects/{pid}/github/pulls";
    Router::new()
        .route(p, get(h_list).post(h_create))
        .route(&format!("{p}/{{n}}"), get(h_get).patch(h_update))
        .route(&format!("{p}/{{n}}/files"), get(h_files))
        .route(&format!("{p}/{{n}}/file"), get(h_file))
        .route(&format!("{p}/{{n}}/threads"), get(h_threads))
        .route(&format!("{p}/{{n}}/threads/{{tid}}/resolve"), post(h_resolve))
        .route(&format!("{p}/{{n}}/review-comments"), post(h_review_comment))
        .route(&format!("{p}/{{n}}/review-comments/{{cid}}/replies"), post(h_reply))
        .route(&format!("{p}/{{n}}/reviews"), get(h_reviews).post(h_review))
        .route(&format!("{p}/{{n}}/comments"), get(h_comments).post(h_comment))
        .route(&format!("{p}/{{n}}/commits"), get(h_commits))
        .route(&format!("{p}/{{n}}/merge"), post(h_merge))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "5babfd548d64a14eabeba53b847fdec5fa5f0ca9";

    #[test]
    fn review_comment_payloads() {
        let c = NewReviewComment { body: "Why?".into(), path: "src/a.rs".into(), line: Some(12), ..Default::default() };
        let p = review_comment_payload(&c, SHA).unwrap();
        assert_eq!(p["commit_id"], SHA);
        assert_eq!(p["line"], 12);
        assert_eq!(p["side"], "RIGHT");
        assert!(p.get("start_line").is_none());
        let multi = NewReviewComment { start_line: Some(10), side: Some("LEFT".into()), ..c };
        let p = review_comment_payload(&multi, SHA).unwrap();
        assert_eq!((p["start_line"].as_u64(), p["start_side"].as_str()), (Some(10), Some("LEFT")));
        let file = NewReviewComment { body: "x".into(), path: "a".into(), ..Default::default() };
        assert_eq!(review_comment_payload(&file, SHA).unwrap()["subject_type"], "file");
        let bad = NewReviewComment { body: "x".into(), path: "../etc".into(), line: Some(1), ..Default::default() };
        assert!(review_comment_payload(&bad, SHA).is_err());
        let zero = NewReviewComment { body: "x".into(), path: "a".into(), line: Some(0), ..Default::default() };
        assert!(review_comment_payload(&zero, SHA).is_err());
        let side = NewReviewComment { body: "x".into(), path: "a".into(), line: Some(1), side: Some("UP".into()), ..Default::default() };
        assert!(review_comment_payload(&side, SHA).is_err());
    }

    #[test]
    fn reviews_need_a_body_unless_approving() {
        assert_eq!(review_payload(&ReviewBody { event: "approve".into(), ..Default::default() }).unwrap()["event"], "APPROVE");
        assert!(review_payload(&ReviewBody { event: "REQUEST_CHANGES".into(), ..Default::default() }).is_err());
        let p = review_payload(&ReviewBody { event: "REQUEST_CHANGES".into(), body: Some("Fix X".into()), sha: Some(SHA.into()) }).unwrap();
        assert_eq!((p["body"].as_str(), p["commit_id"].as_str()), (Some("Fix X"), Some(SHA)));
        assert!(review_payload(&ReviewBody { event: "DISMISS".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn merge_requires_the_reviewed_sha() {
        assert!(merge_payload(&MergeBody::default()).is_err());
        let p = merge_payload(&MergeBody { sha: SHA.into(), method: Some("squash".into()), title: Some("T".into()), ..Default::default() }).unwrap();
        assert_eq!((p["merge_method"].as_str(), p["commit_title"].as_str()), (Some("squash"), Some("T")));
        assert!(merge_payload(&MergeBody { sha: SHA.into(), method: Some("octopus".into()), ..Default::default() }).is_err());
    }

    #[test]
    fn file_paths_and_thread_ids() {
        assert!(valid_repo_file("src/a..b.rs").is_ok());
        assert!(valid_repo_file("src/../x").is_err());
        assert!(valid_repo_file("/etc/passwd").is_err());
        assert!(valid_repo_file("a\nb").is_err());
        assert!(valid_node_id("PRRT_kwDOABCD12").is_ok());
        assert_eq!(valid_node_id("c123").unwrap_err().code, "not_configured");
        assert!(valid_node_id("x y").is_err());
    }

    #[test]
    fn search_items_become_pulls() {
        let v = json!({ "number": 5, "title": "T", "state": "closed", "draft": false,
                        "pull_request": { "merged_at": "2026-09-01T00:00:00Z", "html_url": "x" },
                        "user": { "login": "dev" }, "comments": 3 });
        let p = pull_from_search(v).unwrap();
        assert!(p.merged);
        assert_eq!(p.comments, Some(3));
        assert!(p.head.is_none());
        assert!(matches!(classify(vec![b'a', 0, b'b'], false), FileText::Binary));
        assert!(matches!(classify(b"text".to_vec(), false), FileText::Text(_)));
        assert!(matches!(classify(vec![], true), FileText::TooLarge));
    }
}

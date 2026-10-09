//! Integration tests against a mock GitHub (axum on 127.0.0.1:0, Enterprise-style
//! `/api/v3` and `/api/graphql`) plus a second origin standing in for the signed
//! log storage. Nothing here talks to github.com. The project under test is a
//! throwaway git repository whose `.workbench.toml` points `[repo.github]` at
//! the mock; it has real commits so local-first lookups can be checked too.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::{ci, client, misc, pulls};
use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::forge::RepoParam;
use crate::mcp::{McpCtx, ToolOutput};

const TOKEN: &str = "ghp_mock-token-value-0123456789";
/// The token of the second GitHub repository (`other/api`, the `api` repository in the
/// multi-repository tests): the first repository's token must never reach it.
const TOKEN_B: &str = "ghp_mock-token-for-the-api-repo-9876";
const PID: &str = "repo";
const RUN_SHA: &str = "5babfd548d64a14eabeba53b847fdec5fa5f0ca9";
/// A full sha GitHub has never seen (an unpushed commit).
const UNKNOWN_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    query: String,
    body: Option<Value>,
    auth: Option<String>,
    accept: Option<String>,
    if_none_match: Option<String>,
}

#[derive(Default)]
struct Mock {
    base: Mutex<String>,
    storage: Mutex<String>,
    log: Mutex<Vec<Req>>,
    storage_log: Mutex<Vec<Req>>,
    /// Local commits of the test repository: (main, feature).
    shas: Mutex<(String, String)>,
    /// `base.sha` GitHub recorded for the merged pull requests #9 and #10.
    merged_base: Mutex<String>,
    run_status: Mutex<(String, Option<String>)>,
    /// Status and conclusion of run 888, the newest of branch `dev` of `other/api`.
    api_run: Mutex<(String, Option<String>)>,
    secondary_hits: AtomicUsize,
    not_modified: AtomicUsize,
    /// Answer every request as anonymous GitHub would (no permissions).
    remaining: AtomicUsize,
    exhausted: AtomicBool,
}

impl Mock {
    fn requests(&self, method: &str, suffix: &str) -> Vec<Req> {
        self.log.lock().iter().filter(|r| r.method == method && r.path.ends_with(suffix)).cloned().collect()
    }
}

fn query_map(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(&v.replace('+', " ")).map(|c| c.into_owned()).unwrap_or_default()))
        .collect()
}

fn user(login: &str) -> Value {
    json!({ "id": 1, "login": login, "html_url": format!("https://example.test/{login}"), "type": "User", "avatar_url": "x" })
}

fn run_json(base: &str, id: u64, branch: &str, sha: &str, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "id": id, "name": "CI", "display_title": "Fix things", "run_number": id - 100, "run_attempt": 1,
        "event": "push", "status": status, "conclusion": conclusion, "workflow_id": 11, "head_branch": branch,
        "head_sha": sha, "html_url": format!("{base}/mock/proj/actions/runs/{id}"),
        "created_at": "2026-09-26T10:00:00Z", "updated_at": "2026-09-26T10:04:00Z", "run_started_at": "2026-09-26T10:00:00Z",
        "head_commit": { "id": sha, "message": "Fix things\n\nbody" }, "pull_requests": [], "actor": user("dev")
    })
}

fn pr_json(base: &str, n: u64, title: &str, head_sha: &str, base_sha: &str) -> Value {
    json!({
        "id": 1000 + n, "node_id": format!("PR_node{n}"), "number": n, "state": "open", "title": title, "body": "Body",
        "draft": false, "user": user("dev"), "labels": [{ "id": 1, "name": "bug", "color": "d73a4a" }],
        "head": { "label": "mock:feature", "ref": "feature", "sha": head_sha, "repo": { "id": 42, "full_name": "mock/proj", "fork": false } },
        "base": { "label": "mock:main", "ref": "main", "sha": base_sha, "repo": { "id": 42, "full_name": "mock/proj", "fork": false } },
        "html_url": format!("{base}/mock/proj/pull/{n}"), "created_at": "2026-09-26T09:00:00Z", "updated_at": "2026-09-26T10:00:00Z",
        "mergeable": true, "mergeable_state": "clean", "merged": false, "comments": 1, "review_comments": 2, "commits": 1,
        "additions": 1, "deletions": 0, "changed_files": 1
    })
}

fn json_resp(status: StatusCode, v: Value) -> Response {
    (status, Json(v)).into_response()
}

async fn mock_handler(State(m): State<Arc<Mock>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let base = m.base.lock().clone();
    let storage = m.storage.lock().clone();
    let (main_sha, feature_sha) = m.shas.lock().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let h = |k: header::HeaderName| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let auth = h(header::AUTHORIZATION);
    m.log.lock().push(Req {
        method: method.to_string(),
        path: path.clone(),
        query: query.clone(),
        body: serde_json::from_slice(&body).ok(),
        auth: auth.clone(),
        accept: h(header::ACCEPT),
        if_none_match: h(header::IF_NONE_MATCH),
    });
    // Repository `other/api` (77) has a token of its own.
    let second = path.contains("/repos/other/api") || path.contains("/repositories/77");
    let authed = auth.as_deref() == Some(&format!("Bearer {}", if second { TOKEN_B } else { TOKEN }));
    if auth.is_some() && !authed {
        return json_resp(StatusCode::UNAUTHORIZED, json!({ "message": "Bad credentials" }));
    }
    let q = query_map(&query);
    let page: u64 = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
    let reset = chrono::Utc::now().timestamp() + 3600;
    let remaining = m.remaining.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
    let rate = |mut r: Response| {
        let hh = r.headers_mut();
        hh.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
        hh.insert("x-ratelimit-remaining", HeaderValue::from_str(&remaining.to_string()).unwrap());
        hh.insert("x-ratelimit-reset", HeaderValue::from_str(&reset.to_string()).unwrap());
        hh.insert("x-ratelimit-resource", HeaderValue::from_static("core"));
        r
    };
    if m.exhausted.load(Ordering::SeqCst) {
        let mut r = json_resp(StatusCode::FORBIDDEN, json!({ "message": "API rate limit exceeded for 127.0.0.1." }));
        r.headers_mut().insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        r.headers_mut().insert("x-ratelimit-reset", HeaderValue::from_str(&(reset - 3000).to_string()).unwrap());
        return r;
    }
    if path == "/api/graphql" {
        let b: Value = serde_json::from_slice(&body).unwrap_or_default();
        let qtext = b["query"].as_str().unwrap_or("");
        if qtext.contains("reviewThreads") {
            return rate(json_resp(StatusCode::OK, json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
                "pageInfo": { "hasNextPage": false, "endCursor": null },
                "nodes": [{ "id": "PRRT_thread1", "isResolved": false, "resolvedBy": null, "viewerCanResolve": true,
                            "viewerCanUnresolve": false, "comments": { "nodes": [{ "databaseId": 501 }] } }]
            } } } } })));
        }
        if qtext.contains("resolveReviewThread") {
            return rate(json_resp(StatusCode::OK, json!({ "data": { "resolveReviewThread": { "thread": { "id": "PRRT_thread1", "isResolved": true } } } })));
        }
        return json_resp(StatusCode::OK, json!({ "errors": [{ "message": "unknown query" }] }));
    }
    let rel = path.strip_prefix("/api/v3/").unwrap_or(&path);
    let segs: Vec<&str> = rel.split('/').collect();
    let r = match (method.as_str(), segs.as_slice()) {
        ("GET", ["user"]) if authed => json_resp(StatusCode::OK, user("dev")),
        ("GET", ["repos", "mock", "proj"]) => {
            let mut v = json!({ "id": 42, "node_id": "R_42", "name": "proj", "full_name": "mock/proj", "private": false,
                "html_url": format!("{base}/mock/proj"), "default_branch": "main", "open_issues_count": 5, "has_issues": true,
                "description": "A mock repository" });
            if authed {
                v["permissions"] = json!({ "admin": false, "push": true, "pull": true });
                v["allow_merge_commit"] = json!(true);
                v["allow_squash_merge"] = json!(true);
                v["allow_rebase_merge"] = json!(false);
            }
            json_resp(StatusCode::OK, v)
        }
        ("GET", ["repos", "other", "api"]) => json_resp(
            StatusCode::OK,
            json!({ "id": 77, "node_id": "R_77", "name": "api", "full_name": "other/api", "private": false,
                    "html_url": format!("{base}/other/api"), "default_branch": "main", "open_issues_count": 0, "has_issues": true,
                    "permissions": { "admin": false, "push": true, "pull": true } }),
        ),
        ("GET", ["repos", "other", "api", "actions", "runs"]) => {
            let (status, conclusion) = m.api_run.lock().clone();
            let items = if q.get("branch").is_none_or(|b| b == "dev") {
                vec![run_json(&base, 888, "dev", &"2".repeat(40), &status, conclusion.as_deref())]
            } else {
                vec![]
            };
            // A new ETag with every state: the client never answers from a stale copy.
            let etag = format!("W/\"api-runs-{status}\"");
            let mut resp = json_resp(StatusCode::OK, json!({ "total_count": items.len(), "workflow_runs": items }));
            resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
            resp
        }
        ("POST", ["repos", "other", "api", "pulls"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            let mut p = pr_json(&base, 4, b["title"].as_str().unwrap_or(""), &"2".repeat(40), &"3".repeat(40));
            p["head"]["ref"] = b["head"].clone();
            json_resp(StatusCode::CREATED, p)
        }
        ("GET", ["repos", "mock", "proj", "actions", "runs"]) => {
            let (status, conclusion) = m.run_status.lock().clone();
            let per_page: u64 = q.get("per_page").and_then(|p| p.parse().ok()).unwrap_or(30);
            let mut all = vec![
                run_json(&base, 303, "main", RUN_SHA, &status, conclusion.as_deref()),
                run_json(&base, 302, "main", RUN_SHA, "completed", Some("success")),
                run_json(&base, 301, "feature", &feature_sha, "completed", Some("failure")),
            ];
            if let Some(b) = q.get("branch") {
                all.retain(|r| r["head_branch"] == b.as_str());
            }
            if let Some(s) = q.get("head_sha") {
                all.retain(|r| r["head_sha"] == s.as_str());
            }
            let total = all.len() as u64;
            let start = ((page - 1) * per_page) as usize;
            let items: Vec<Value> = all.into_iter().skip(start).take(per_page as usize).collect();
            // Weak ETag per content, as GitHub does.
            let etag = format!("W/\"runs-{status}-{page}-{per_page}-{}\"", q.get("branch").map(String::as_str).unwrap_or(""));
            if h(header::IF_NONE_MATCH).as_deref() == Some(etag.as_str()) {
                m.not_modified.fetch_add(1, Ordering::SeqCst);
                return rate(StatusCode::NOT_MODIFIED.into_response());
            }
            let mut resp = json_resp(StatusCode::OK, json!({ "total_count": total, "workflow_runs": items }));
            resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
            if (start as u64) + per_page < total {
                let link = format!(
                    "<{base}/api/v3/repositories/42/actions/runs?per_page={per_page}&page={}>; rel=\"next\", <{base}/api/v3/repositories/42/actions/runs?per_page={per_page}&page=9>; rel=\"last\"",
                    page + 1
                );
                resp.headers_mut().insert(header::LINK, HeaderValue::from_str(&link).unwrap());
            }
            resp
        }
        ("GET", ["repositories", "42", "actions", "runs"]) => {
            let items = vec![run_json(&base, 301, "feature", &feature_sha, "completed", Some("failure"))];
            json_resp(StatusCode::OK, json!({ "total_count": 3, "workflow_runs": items }))
        }
        ("GET", ["repos", "mock", "proj", "actions", "runs", id]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let (status, conclusion) = m.run_status.lock().clone();
            json_resp(StatusCode::OK, run_json(&base, id, "main", RUN_SHA, &status, conclusion.as_deref()))
        }
        ("GET", ["repos", "mock", "proj", "actions", "runs", _, "jobs"]) => {
            let job = |id: u64, name: &str, conclusion: &str| json!({
                "id": id, "run_id": 303, "name": name, "status": "completed", "conclusion": conclusion,
                "started_at": "2026-09-26T10:00:00Z", "completed_at": "2026-09-26T10:01:00Z", "head_sha": RUN_SHA,
                "html_url": format!("{base}/mock/proj/actions/runs/303/job/{id}"),
                "steps": [
                    { "name": "Set up job", "status": "completed", "conclusion": "success", "number": 1, "started_at": "2026-09-26T10:00:00Z", "completed_at": "2026-09-26T10:00:01Z" },
                    { "name": "Run tests", "status": "completed", "conclusion": conclusion, "number": 2, "started_at": "2026-09-26T10:00:02Z", "completed_at": "2026-09-26T10:01:00Z" }
                ]
            });
            if page == 1 {
                let mut resp = json_resp(StatusCode::OK, json!({ "total_count": 2, "jobs": [job(9001, "test", "failure")] }));
                let link = format!("<{base}/api/v3/repositories/42/actions/runs/303/jobs?filter=latest&per_page=100&page=2>; rel=\"next\"");
                resp.headers_mut().insert(header::LINK, HeaderValue::from_str(&link).unwrap());
                resp
            } else {
                json_resp(StatusCode::OK, json!({ "total_count": 2, "jobs": [job(9002, "lint", "success")] }))
            }
        }
        ("GET", ["repositories", "42", "actions", "runs", "303", "jobs"]) => json_resp(
            StatusCode::OK,
            json!({ "total_count": 2, "jobs": [{ "id": 9002, "run_id": 303, "name": "lint", "status": "completed", "conclusion": "success", "steps": [] }] }),
        ),
        ("POST", ["repos", "mock", "proj", "actions", "runs", _, _]) => json_resp(StatusCode::CREATED, json!({})),
        ("POST", ["repos", "mock", "proj", "actions", "jobs", _, "rerun"]) => json_resp(StatusCode::CREATED, json!({})),
        ("GET", ["repos", "mock", "proj", "actions", "jobs", id]) => json_resp(
            StatusCode::OK,
            json!({ "id": id.parse::<u64>().unwrap_or(0), "run_id": 303, "name": "test", "status": "completed", "conclusion": "failure",
                    "head_sha": RUN_SHA, "started_at": "2026-09-26T10:00:00Z", "completed_at": "2026-09-26T10:01:00Z",
                    "steps": [
                        { "name": "Set up job", "status": "completed", "conclusion": "success", "number": 1, "started_at": "2026-09-26T10:00:00Z" },
                        { "name": "Run tests", "status": "completed", "conclusion": "failure", "number": 2, "started_at": "2026-09-26T10:00:02Z" }
                    ] }),
        ),
        ("GET", ["repos", "mock", "proj", "actions", "runs", _, "artifacts"]) => json_resp(
            StatusCode::OK,
            json!({ "total_count": 2, "artifacts": [
                { "id": 71, "name": "coverage report", "size_in_bytes": 2048, "expired": false, "created_at": "2026-09-26T10:01:00Z", "expires_at": "2026-12-25T10:01:00Z", "digest": "sha256:ab" },
                { "id": 72, "name": "old", "size_in_bytes": 10, "expired": true }
            ] }),
        ),
        ("GET", ["repos", "mock", "proj", "actions", "artifacts", id]) => {
            let expired = *id == "72";
            json_resp(StatusCode::OK, json!({ "id": id.parse::<u64>().unwrap_or(0), "name": if expired { "old" } else { "coverage report" }, "size_in_bytes": 6, "expired": expired }))
        }
        ("GET", ["repos", "mock", "proj", "actions", "artifacts", id, "zip"]) => {
            let mut r = StatusCode::FOUND.into_response();
            r.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&format!("{storage}/artifacts/{id}?sig=signed")).unwrap());
            r
        }
        ("GET", ["repos", "mock", "proj", "actions", "jobs", id, "logs"]) => {
            let mut r = StatusCode::FOUND.into_response();
            r.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&format!("{storage}/logs/{id}?sig=signed")).unwrap());
            r
        }
        ("GET", ["repos", "mock", "proj", "check-runs", _, "annotations"]) => json_resp(
            StatusCode::OK,
            json!([{ "path": "src/lib.rs", "start_line": 3, "end_line": 3, "annotation_level": "failure", "message": "mismatched types" }]),
        ),
        ("GET", ["repos", "mock", "proj", "actions", "workflows"]) => json_resp(
            StatusCode::OK,
            json!({ "total_count": 1, "workflows": [{ "id": 11, "name": "Deploy", "path": ".github/workflows/deploy.yml", "state": "active" }] }),
        ),
        ("GET", ["repos", "mock", "proj", "actions", "workflows", "11"]) => json_resp(
            StatusCode::OK,
            json!({ "id": 11, "name": "Deploy", "path": ".github/workflows/deploy.yml", "state": "active" }),
        ),
        ("GET", ["repos", "mock", "proj", "contents", ..]) => {
            if h(header::ACCEPT).as_deref() != Some("application/vnd.github.raw+json") {
                return json_resp(StatusCode::BAD_REQUEST, json!({ "message": "wanted raw" }));
            }
            let file = rel.split("/contents/").nth(1).unwrap_or("");
            match file {
                ".github/workflows/deploy.yml" => "on:\n  workflow_dispatch:\n    inputs:\n      env:\n        type: choice\n        options: [staging, prod]\n".into_response(),
                "remote-only.txt" => format!("remote at {}\n", q.get("ref").cloned().unwrap_or_default()).into_response(),
                _ => json_resp(StatusCode::NOT_FOUND, json!({ "message": "Not Found" })),
            }
        }
        ("POST", ["repos", "mock", "proj", "actions", "workflows", _, "dispatches"]) => StatusCode::NO_CONTENT.into_response(),
        ("GET", ["repos", "mock", "proj", "commits", sha, "check-runs"]) if *sha == RUN_SHA => json_resp(
            StatusCode::OK,
            json!({ "total_count": 2, "check_runs": [
                { "id": 9001, "name": "test", "status": "completed", "conclusion": "failure", "app": { "slug": "github-actions", "name": "GitHub Actions" },
                  "details_url": format!("{base}/mock/proj/actions/runs/303/job/9001") },
                { "id": 77, "name": "codecov", "status": "completed", "conclusion": "success", "app": { "slug": "codecov", "name": "Codecov" },
                  "details_url": "https://codecov.example/x" }
            ] }),
        ),
        ("GET", ["repos", "mock", "proj", "commits", sha, "check-runs" | "status"]) if *sha == UNKNOWN_SHA => {
            json_resp(StatusCode::UNPROCESSABLE_ENTITY, json!({ "message": format!("No commit found for SHA: {sha}") }))
        }
        ("GET", ["repos", "mock", "proj", "commits", _, "check-runs"]) => json_resp(StatusCode::OK, json!({ "total_count": 0, "check_runs": [] })),
        ("GET", ["repos", "mock", "proj", "commits", sha, "status"]) if *sha == RUN_SHA => json_resp(
            StatusCode::OK,
            json!({ "state": "pending", "total_count": 1, "statuses": [{ "context": "ci/legacy", "state": "success", "target_url": "https://ci.example/1" }] }),
        ),
        ("GET", ["repos", "mock", "proj", "commits", _, "status"]) => json_resp(StatusCode::OK, json!({ "state": "pending", "total_count": 0, "statuses": [] })),
        ("GET", ["repos", "mock", "proj", "commits", sha]) => {
            if RUN_SHA.starts_with(sha) {
                json_resp(StatusCode::OK, json!({ "sha": RUN_SHA }))
            } else {
                json_resp(StatusCode::UNPROCESSABLE_ENTITY, json!({ "message": "No commit found for SHA" }))
            }
        }
        ("GET", ["repos", "mock", "proj", "pulls"]) => {
            if q.get("head").is_some_and(|h| h == "mock:feature") {
                json_resp(StatusCode::OK, json!([pr_json(&base, 7, "Existing", &feature_sha, &main_sha)]))
            } else if q.get("per_page").is_some_and(|p| p == "1") {
                let mut r = json_resp(StatusCode::OK, json!([pr_json(&base, 7, "Existing", &feature_sha, &main_sha)]));
                let link = format!("<{base}/api/v3/repositories/42/pulls?state=open&per_page=1&page=2>; rel=\"next\", <{base}/api/v3/repositories/42/pulls?state=open&per_page=1&page=3>; rel=\"last\"");
                r.headers_mut().insert(header::LINK, HeaderValue::from_str(&link).unwrap());
                r
            } else {
                json_resp(StatusCode::OK, json!([pr_json(&base, 7, "Existing", &feature_sha, &main_sha)]))
            }
        }
        ("POST", ["repos", "mock", "proj", "pulls"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            if b["head"] == b["base"] {
                return json_resp(StatusCode::UNPROCESSABLE_ENTITY, json!({ "message": "Validation Failed", "errors": [{ "message": "No commits between main and main" }] }));
            }
            let mut p = pr_json(&base, 8, b["title"].as_str().unwrap_or(""), &feature_sha, &main_sha);
            p["draft"] = b["draft"].clone();
            json_resp(StatusCode::CREATED, p)
        }
        ("GET", ["repos", "mock", "proj", "pulls", n @ ("9" | "10")]) => {
            // Merged after `main` moved on: GitHub froze base.sha at main's tip
            // just before the merge. #10's head (the branch point) is already in it.
            let head = if *n == "9" { feature_sha.clone() } else { main_sha.clone() };
            let mut p = pr_json(&base, n.parse().unwrap_or(0), "Merged", &head, &m.merged_base.lock());
            p["state"] = json!("closed");
            p["merged"] = json!(true);
            p["merged_at"] = json!("2026-09-26T11:00:00Z");
            json_resp(StatusCode::OK, p)
        }
        ("GET", ["repos", "mock", "proj", "pulls", "8"]) => {
            // A pull request whose commits the local clone does not have.
            json_resp(StatusCode::OK, pr_json(&base, 8, "Remote", "cccccccccccccccccccccccccccccccccccccccc", "dddddddddddddddddddddddddddddddddddddddd"))
        }
        ("GET", ["repos", "mock", "proj", "pulls", n]) => {
            json_resp(StatusCode::OK, pr_json(&base, n.parse().unwrap_or(0), "Existing", &feature_sha, &main_sha))
        }
        ("PATCH", ["repos", "mock", "proj", "pulls", n]) => {
            json_resp(StatusCode::OK, pr_json(&base, n.parse().unwrap_or(0), "Existing", &feature_sha, &main_sha))
        }
        ("GET", ["repos", "mock", "proj", "compare", spec]) => {
            assert!(spec.contains("..."), "{spec}");
            json_resp(StatusCode::OK, json!({ "merge_base_commit": { "sha": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" }, "files": [] }))
        }
        ("GET", ["repos", "mock", "proj", "pulls", _, "files"]) => {
            let file = |n: u64| json!({ "filename": format!("f{n}.rs"), "status": "modified", "additions": 2, "deletions": 1, "changes": 3, "patch": "@@ -1 +1,2 @@\n-a\n+b\n+c" });
            let mut resp = json_resp(StatusCode::OK, json!([file(page * 2 - 1), file(page * 2)]));
            if page < 2 {
                let link = format!("<{base}/api/v3/repositories/42/pulls/7/files?per_page=100&page=2>; rel=\"next\"");
                resp.headers_mut().insert(header::LINK, HeaderValue::from_str(&link).unwrap());
            }
            resp
        }
        ("GET", ["repositories", "42", "pulls", "7", "files"]) => {
            let file = |n: u64| json!({ "filename": format!("f{n}.rs"), "status": "added", "additions": 1, "deletions": 0, "changes": 1, "patch": "@@ -0,0 +1 @@\n+x" });
            json_resp(StatusCode::OK, json!([file(3), file(4)]))
        }
        ("GET", ["repos", "mock", "proj", "pulls", _, "reviews"]) => json_resp(
            StatusCode::OK,
            json!([{ "id": 1, "user": user("rev"), "state": "CHANGES_REQUESTED", "submitted_at": "2026-09-26T09:30:00Z" },
                   { "id": 2, "user": user("rev"), "state": "APPROVED", "submitted_at": "2026-09-26T09:40:00Z" }]),
        ),
        ("POST", ["repos", "mock", "proj", "pulls", _, "reviews"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            json_resp(StatusCode::OK, json!({ "id": 3, "user": user("dev"), "state": if b["event"] == "APPROVE" { "APPROVED" } else { "CHANGES_REQUESTED" } }))
        }
        ("GET", ["repos", "mock", "proj", "pulls", _, "comments"]) => json_resp(
            StatusCode::OK,
            json!([
                { "id": 501, "node_id": "PRRC_1", "path": "a.txt", "line": 2, "original_line": 2, "side": "RIGHT", "body": "Why?", "user": user("rev"), "diff_hunk": "@@" },
                { "id": 502, "node_id": "PRRC_2", "path": "a.txt", "line": 2, "side": "RIGHT", "body": "Because.", "user": user("dev"), "in_reply_to_id": 501, "diff_hunk": "@@" }
            ]),
        ),
        ("POST", ["repos", "mock", "proj", "pulls", _, "comments"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            json_resp(StatusCode::CREATED, json!({ "id": 503, "path": b["path"], "line": b["line"], "body": b["body"] }))
        }
        ("POST", ["repos", "mock", "proj", "pulls", _, "comments", _, "replies"]) => {
            json_resp(StatusCode::CREATED, json!({ "id": 504, "path": "a.txt", "body": "reply", "in_reply_to_id": 501 }))
        }
        ("PUT", ["repos", "mock", "proj", "pulls", _, "merge"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            if b["sha"].as_str() != Some(feature_sha.as_str()) {
                return json_resp(StatusCode::CONFLICT, json!({ "message": "Head branch was modified. Review and try the merge again." }));
            }
            json_resp(StatusCode::OK, json!({ "merged": true, "sha": "f00d", "message": "Pull Request successfully merged" }))
        }
        ("DELETE", ["repos", "mock", "proj", "git", "refs", "heads", "feature"]) => StatusCode::NO_CONTENT.into_response(),
        ("GET", ["repos", "mock", "proj", "issues"]) => json_resp(
            StatusCode::OK,
            json!([
                { "id": 1, "number": 3, "title": "A bug", "state": "open", "user": user("dev"), "labels": [], "comments": 0 },
                { "id": 2, "number": 7, "title": "A pull request", "state": "open", "pull_request": { "url": "x" } }
            ]),
        ),
        ("GET", ["repos", "mock", "proj", "issues", "500"]) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("<html>stack trace with {TOKEN} and internals</html>")).into_response()
        }
        ("POST", ["repos", "mock", "proj", "issues"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            json_resp(StatusCode::CREATED, json!({ "id": 9, "number": 9, "title": b["title"], "state": "open" }))
        }
        ("PATCH", ["repos", "mock", "proj", "issues", n]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            json_resp(StatusCode::OK, json!({ "id": 9, "number": n.parse::<u64>().unwrap_or(0), "title": "x", "state": b["state"] }))
        }
        ("POST", ["repos", "mock", "proj", "issues", _, "comments"]) => json_resp(StatusCode::CREATED, json!({ "id": 71, "body": "hi" })),
        ("GET", ["repos", "mock", "proj", "releases"]) => {
            if m.secondary_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut r = json_resp(StatusCode::FORBIDDEN, json!({ "message": "You have exceeded a secondary rate limit." }));
                r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                return r;
            }
            json_resp(StatusCode::OK, json!([{ "id": 1, "tag_name": "v1.0.0", "name": "One", "assets": [] }]))
        }
        _ => json_resp(StatusCode::NOT_FOUND, json!({ "message": "Not Found" })),
    };
    rate(r)
}

async fn storage_handler(State(m): State<Arc<Mock>>, method: Method, uri: Uri, headers: HeaderMap) -> Response {
    m.storage_log.lock().push(Req {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().unwrap_or("").to_string(),
        body: None,
        auth: headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string),
        accept: None,
        if_none_match: None,
    });
    if uri.path().starts_with("/artifacts/") {
        return ([(header::CONTENT_TYPE, "application/zip")], &b"PK\x03\x04zip"[..]).into_response();
    }
    let log = "\u{feff}2026-09-26T10:00:00.1000000Z Current runner version: '2.328.0'\n\
2026-09-26T10:00:00.2000000Z ##[group]Operating System\n\
2026-09-26T10:00:00.3000000Z Ubuntu\n\
2026-09-26T10:00:00.4000000Z ##[endgroup]\n\
2026-09-26T10:00:02.1000000Z ##[group]Run cargo test\n\
2026-09-26T10:00:02.2000000Z ##[endgroup]\n\
2026-09-26T10:00:50.0000000Z \x1b[31merror[E0308]\x1b[0m: mismatched types\n\
2026-09-26T10:00:59.0000000Z ##[error]Process completed with exit code 101.\n";
    log.into_response()
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    base
}

async fn start_mock() -> (String, Arc<Mock>) {
    let m = Arc::new(Mock::default());
    *m.run_status.lock() = ("in_progress".into(), None);
    *m.api_run.lock() = ("in_progress".into(), None);
    m.remaining.store(5000, Ordering::SeqCst);
    let base = serve(Router::new().fallback(mock_handler).with_state(m.clone())).await;
    let storage = serve(Router::new().fallback(storage_handler).with_state(m.clone())).await;
    *m.base.lock() = base.clone();
    *m.storage.lock() = storage;
    (base, m)
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Setup {
    state: AppState,
    mock: Arc<Mock>,
    _dir: tempfile::TempDir,
}

/// A Workbench state whose only project (`repo`, on branch `feature`) points at
/// the mock. `token`: whether the project names a token secret.
async fn setup_with(token: bool) -> Setup {
    setup_custom(token, |_| String::new(), &[], &[]).await
}

/// `setup_with`, plus more `.workbench.toml` (built from the mock's origin), git
/// repositories below the project's root (`nested`: directory and branch) and more
/// secrets in the machine overlay (`secrets`: name and token value).
async fn setup_custom(token: bool, more_toml: impl FnOnce(&str) -> String, nested: &[(&str, &str)], secrets: &[(&str, &str)]) -> Setup {
    let (base, mock) = start_mock().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let main_sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "Add two"]);
    let feature_sha = git(&repo, &["rev-parse", "HEAD"]);
    *mock.shas.lock() = (main_sha, feature_sha);
    for (sub, branch) in nested {
        std::fs::create_dir_all(repo.join(sub)).unwrap();
        git(&repo.join(sub), &["init", "-q", "-b", branch]);
    }
    let token_file = dir.path().join("token");
    crate::util::fs::write_atomic(&token_file, TOKEN.as_bytes(), 0o600).unwrap();
    let token_line = if token { "token = \"mock\"\n" } else { "" };
    let more = more_toml(&base);
    std::fs::write(repo.join(".workbench.toml"), format!("[repo.github]\nhost = \"{base}\"\npath = \"mock/proj\"\n{token_line}{more}")).unwrap();
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(paths.config_dir.join("projects")).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    // Secret references live in the machine overlay; the committed file only names them.
    // The path goes in as a TOML string: a Windows path pasted into "…" is read as
    // escapes (`\U` wants eight hex digits), and an overlay that does not parse
    // vouches for nothing.
    let token_ref = toml::Value::String(token_file.display().to_string());
    let mut overlay = format!("[secrets]\nmock = {{ file = {token_ref} }}\n");
    for (name, value) in secrets {
        let file = dir.path().join(format!("secret-{name}"));
        crate::util::fs::write_atomic(&file, value.as_bytes(), 0o600).unwrap();
        overlay.push_str(&format!("{name} = {{ file = {} }}\n", toml::Value::String(file.display().to_string())));
    }
    std::fs::write(paths.project_overlay(PID), overlay).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![repo.display().to_string()];
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let _router = crate::app::build_router(state.clone());
    let project = state.projects.get(PID).expect("test project registered");
    assert!(project.config.secrets.contains_key("mock"), "the machine overlay parses: {:?}", project.warnings);
    Setup { state, mock, _dir: dir }
}

async fn setup() -> Setup {
    setup_with(true).await
}

#[tokio::test]
async fn resolves_repo_and_sends_token_only_in_header() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert!(!ctx.is_anonymous());
    assert_eq!(ctx.default_branch(), Some("main"));
    assert_eq!(ctx.rurl("/pulls"), format!("{}/api/v3/repos/mock/proj/pulls", t.mock.base.lock()));
    let _ = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert_eq!(t.mock.requests("GET", "/repos/mock/proj").len(), 1, "metadata is cached");
    for r in t.mock.log.lock().iter() {
        assert_eq!(r.auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
        assert!(!r.query.contains(TOKEN));
    }
}

#[tokio::test]
async fn pagination_follows_link_next_across_url_forms() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let page = ci::list_runs(&ctx, &ci::RunsQuery { per_page: Some(2), ..Default::default() }).await.unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.total, Some(3));
    assert_eq!(page.next_page, Some(2));
    assert_eq!(page.items[0].state, "running");
    assert_eq!(page.items[1].duration, Some(240.0));
    // Link next points at /repositories/{id}/…: still followed, still authenticated.
    let files = pulls::pr_files(&ctx, 7).await.unwrap();
    assert_eq!(files.files.iter().map(|f| f.filename.as_str()).collect::<Vec<_>>(), ["f1.rs", "f2.rs", "f3.rs", "f4.rs"]);
    assert!(!files.truncated);
    let d = ci::run_detail(&ctx, 303).await.unwrap();
    assert_eq!(d.jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["test", "lint"]);
    assert_eq!(d.jobs[0].state, "failed");
    assert_eq!(d.jobs[0].steps[1].state, "failed");
    let (runs, more) = ctx.get_all::<Value>(&ctx.rurl("/actions/runs"), &[("per_page", "1".into())], 1, Some("workflow_runs"), client::Fresh::Live).await.unwrap();
    assert_eq!((runs.len(), more), (1, true));
}

#[tokio::test]
async fn etags_revalidate_cached_responses() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let q = ci::RunsQuery { branch: Some("main".into()), ..Default::default() };
    let a = ci::list_runs(&ctx, &q).await.unwrap();
    let _ = ci::list_runs(&ctx, &q).await.unwrap();
    assert_eq!(t.mock.requests("GET", "/actions/runs").len(), 1, "fresh answers come from the cache");
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let b = ci::list_runs(&ctx, &q).await.unwrap();
    let reqs = t.mock.requests("GET", "/actions/runs");
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].if_none_match.as_deref().is_some_and(|e| e.starts_with("W/\"runs-")));
    assert_eq!(t.mock.not_modified.load(Ordering::SeqCst), 1);
    assert_eq!(a.items.len(), b.items.len());
    // A write drops the repository's cached responses.
    ci::run_action(&ctx, 303, "cancel").await.unwrap();
    let _ = ci::list_runs(&ctx, &q).await.unwrap();
    let reqs = t.mock.requests("GET", "/actions/runs");
    assert!(reqs.last().unwrap().if_none_match.is_none(), "refetched after the write");
}

#[tokio::test]
async fn secondary_rate_limits_are_waited_out() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let started = std::time::Instant::now();
    let page = misc::releases(&ctx, &misc::PageQuery::default()).await.unwrap();
    assert_eq!(page.items[0].tag_name, "v1.0.0");
    assert!(started.elapsed() >= Duration::from_millis(900), "Retry-After honoured");
    assert_eq!(t.mock.secondary_hits.load(Ordering::SeqCst), 2);
    let rate = ctx.rate("core").unwrap();
    assert_eq!(rate.limit, Some(5000));
    assert!(rate.remaining.unwrap() < 5000);
}

#[tokio::test]
async fn an_exhausted_quota_fails_fast_and_serves_stale_data() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let q = ci::RunsQuery { branch: Some("main".into()), ..Default::default() };
    let before = ci::list_runs(&ctx, &q).await.unwrap();
    t.mock.exhausted.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(3200)).await;
    // The cached list is older than its freshness window, but beats nothing.
    let stale = ci::list_runs(&ctx, &q).await.unwrap();
    assert_eq!(stale.items.len(), before.items.len());
    let err = ci::get_job(&ctx, 9001).await.unwrap_err();
    assert_eq!(err.code, "rate_limited");
    assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);
    // Known exhausted: the next request is not even sent.
    let sent = t.mock.log.lock().len();
    assert_eq!(ci::get_job(&ctx, 9002).await.unwrap_err().code, "rate_limited");
    assert_eq!(t.mock.log.lock().len(), sent);
}

#[tokio::test]
async fn job_logs_follow_the_redirect_without_the_token() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let log = ci::job_log(&ctx, 9001).await.unwrap();
    assert!(log.available);
    let data = log.data.clone().unwrap().0;
    let lines: Vec<&str> = data.text.split('\n').collect();
    assert_eq!(lines[0], "Current runner version: '2.328.0'", "BOM and timestamps stripped");
    assert!(lines.contains(&"##[group]Operating System"), "group markers kept for folding");
    assert!(data.text.contains("\x1b[31merror[E0308]"), "ANSI kept");
    assert_eq!(data.steps.iter().map(|s| (s.number, s.line)).collect::<Vec<_>>(), [(1, 0), (2, 4)]);
    let storage = t.mock.storage_log.lock().clone();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].path, "/logs/9001");
    assert_eq!(storage[0].query, "sig=signed");
    assert!(storage[0].auth.is_none(), "the token never goes to the storage origin");
    let api_log = t.mock.requests("GET", "/actions/jobs/9001/logs");
    assert_eq!(api_log[0].auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    // Finished logs are cached.
    let _ = ci::job_log(&ctx, 9001).await.unwrap();
    assert_eq!(t.mock.storage_log.lock().len(), 1);
    // Plain tail for agents.
    let (_, tail, _) = ci::log_tail(&ctx, 9001, 2, true).await.unwrap();
    let (text, total, _) = tail.unwrap();
    assert_eq!(total, 6);
    assert_eq!(text, "error[E0308]: mismatched types\nError: Process completed with exit code 101.");
}

/// Artifacts are listed, and a download follows GitHub's redirect to its storage
/// without the token, streaming the zip through with the artifact's name.
#[tokio::test]
async fn run_artifacts_list_and_download_without_leaking_the_token() {
    let t = setup().await;
    let mctx = McpCtx::default();
    let v = crate::mcp::call_api(&t.state, Method::GET, "/api/projects/repo/github/actions/runs/4242/artifacts", None, &mctx).await.unwrap();
    assert_eq!(v[0]["name"], "coverage report");
    assert_eq!(v[0]["sizeInBytes"], 2048);
    assert_eq!(v[1]["expired"], true);

    let router = crate::app::build_router(t.state.clone());
    let get = |path: &str| {
        let router = router.clone();
        let req = axum::http::Request::builder()
            .uri(path)
            .header("host", "127.0.0.1:7999")
            .header("authorization", format!("Bearer {}", t.state.auth.master_token()))
            .body(axum::body::Body::empty())
            .unwrap();
        async move { tower::ServiceExt::oneshot(router, req).await.unwrap() }
    };
    let resp = get("/api/projects/repo/github/actions/artifacts/71/zip").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_DISPOSITION], "attachment; filename=\"coverage_report.zip\"");
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(&body[..], b"PK\x03\x04zip");
    let storage = t.mock.storage_log.lock().clone();
    assert_eq!(storage.last().map(|r| (r.path.as_str(), r.query.as_str())), Some(("/artifacts/71", "sig=signed")));
    assert!(storage.iter().all(|r| r.auth.is_none()), "the token never goes to the storage origin");
    // An expired artifact is not fetched.
    let resp = get("/api/projects/repo/github/actions/artifacts/72/zip").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(t.mock.requests("GET", "/artifacts/72/zip").is_empty());
}

#[tokio::test]
async fn anonymous_mode_is_read_only() {
    let t = setup_with(false).await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert!(ctx.is_anonymous());
    assert!(ctx.anonymous_reason().unwrap().contains("no GitHub token"));
    let _ = pulls::list_pulls(&ctx, &pulls::PullsQuery::default()).await.unwrap();
    assert!(t.mock.log.lock().iter().all(|r| r.auth.is_none()));
    // Logs need a signed-in user: said plainly, without a request.
    let log = ci::job_log(&ctx, 9001).await.unwrap();
    assert!(!log.available);
    assert_eq!(log.reason.as_deref(), Some("needs_token"));
    assert!(t.mock.requests("GET", "/logs").is_empty());
    // Writes are refused before anything is sent.
    let sent = t.mock.log.lock().len();
    let err = pulls::add_comment(&ctx, 7, "hi").await.unwrap_err();
    assert_eq!(err.code, "not_configured");
    let err = ci::run_action(&ctx, 303, "rerun").await.unwrap_err();
    assert_eq!(err.code, "not_configured");
    assert_eq!(t.mock.log.lock().len(), sent);
    // Threads come without resolution state (that needs GraphQL, which needs a token).
    let threads = pulls::threads(&ctx, 7).await.unwrap();
    assert_eq!(threads[0].resolved, None);
    assert!(t.mock.requests("POST", "/api/graphql").is_empty());
    // The summary skips what costs requests and says why there is no token.
    let s = misc::summary(&ctx).await.unwrap();
    assert!(!s.auth.authenticated);
    assert_eq!(s.open_pr_count, None);
    assert_eq!(s.can_push, None);
}

#[tokio::test]
async fn repository_config_cannot_borrow_global_secrets() {
    let t = setup_with(false).await;
    // The repository names a secret that only config.toml defines.
    let project = t.state.projects.get(PID).unwrap();
    let mut p = (*project).clone();
    if let Some(g) = p.config.repo.as_mut().and_then(|r| r.github.as_mut()) {
        g.token = "globalgh".into();
    }
    p.repo_secret_names.insert("globalgh".into());
    t.state.config.write().secrets.insert("globalgh".into(), crate::config::SecretRef::Env("PATH".into()));
    let err = match client::ctx_for(&t.state, Arc::new(p)).await {
        Err(e) => e,
        Ok(_) => panic!("a repository-named secret must not resolve from config.toml"),
    };
    assert_eq!(err.code, "not_configured");
    // The global [github] token is used only for its own host (scheme included).
    t.state.config.write().github = Some(crate::config::global::GithubConfig { host: "github.com".into(), token: "globalgh".into() });
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert!(ctx.is_anonymous());
    assert!(ctx.anonymous_reason().unwrap().contains("is for github.com"));
    assert!(t.mock.log.lock().iter().all(|r| r.auth.is_none()));
}

#[tokio::test]
async fn commit_ci_status_combines_checks_runs_and_statuses() {
    let t = setup().await;
    let project = t.state.projects.get(PID).unwrap();
    let st = super::commit_ci_status(&t.state, &project, &RUN_SHA[..8]).await.unwrap().unwrap();
    // A failed job check beats the running run and the passing status.
    assert_eq!(st.status, "failed");
    assert_eq!(st.sha.as_deref(), Some(RUN_SHA));
    assert_eq!(st.git_ref.as_deref(), Some("main"));
    assert!(st.pipeline_id.is_some());
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let checks = ci::commit_checks(&ctx, RUN_SHA).await.unwrap();
    let kinds: Vec<(&str, &str)> = checks.items.iter().map(|i| (i.name.as_str(), i.state.as_str())).collect();
    assert!(kinds.contains(&("test", "failed")));
    assert!(kinds.contains(&("codecov", "success")));
    assert!(kinds.contains(&("ci/legacy", "success")));
    let test = checks.items.iter().find(|i| i.name == "test").unwrap();
    assert_eq!((test.run_id, test.job_id), (Some(303), Some(9001)));
    assert_eq!((test.workflow.as_deref(), test.event.as_deref()), (Some("CI"), Some("push")));
    // Unknown commits and non-GitHub projects are "no status".
    assert!(super::commit_ci_status(&t.state, &project, "deadbeef").await.unwrap().is_none());
    assert_eq!(super::commit_ci_status(&t.state, &project, "not-a-sha").await.unwrap_err().code, "bad_request");
    let mut other = (*project).clone();
    other.config.repo = None;
    other.remote = None;
    assert!(super::commit_ci_status(&t.state, &other, RUN_SHA).await.unwrap().is_none());
    // The forge dispatch routes GitHub projects here.
    let via_forge = crate::forge::commit_ci_status(&t.state, &project, RUN_SHA).await.unwrap().unwrap();
    assert_eq!(via_forge.status, "failed");
}

#[tokio::test]
async fn pull_request_detail_and_file_versions() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let (main_sha, feature_sha) = t.mock.shas.lock().clone();
    let d = pulls::get_pull(&ctx, 7).await.unwrap();
    assert_eq!(d.merge_base_sha.as_deref(), Some(main_sha.as_str()), "merge base from the local clone");
    assert!(t.mock.requests("GET", "...").is_empty() && t.mock.log.lock().iter().all(|r| !r.path.contains("/compare/")));
    assert_eq!(d.review_states.len(), 1);
    assert_eq!(d.review_states[0].state, "APPROVED");
    // Both sides of a local file come from git, not GitHub.
    let v = pulls::pr_file(&ctx, 7, &pulls::FileQuery { path: "a.txt".into(), ..Default::default() }).await.unwrap();
    assert_eq!((v.original.as_str(), v.modified.as_str()), ("one\n", "one\ntwo\n"));
    assert_eq!((v.base_sha.as_str(), v.head_sha.as_str()), (main_sha.as_str(), feature_sha.as_str()));
    assert!(t.mock.requests("GET", "/a.txt").is_empty());
    assert!(t.mock.requests("GET", "/contents/remote-only.txt").iter().all(|r| r.accept.as_deref() == Some("application/vnd.github.raw+json")));
    // Commits the clone lacks: compare API for the base, raw contents API for files.
    let v = pulls::pr_file(&ctx, 8, &pulls::FileQuery { path: "remote-only.txt".into(), ..Default::default() }).await.unwrap();
    assert_eq!(v.base_sha, "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    assert_eq!(v.modified, "remote at cccccccccccccccccccccccccccccccccccccccc\n");
    assert_eq!(v.original, "remote at eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\n");
    let added = pulls::pr_file(&ctx, 8, &pulls::FileQuery { path: "remote-only.txt".into(), status: Some("added".into()), ..Default::default() }).await.unwrap();
    assert_eq!(added.original, "");
    // Threads: grouped replies, plus resolution state and ids from GraphQL.
    let threads = pulls::threads(&ctx, 7).await.unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, "PRRT_thread1");
    assert_eq!(threads[0].resolved, Some(false));
    assert!(threads[0].can_resolve);
    assert_eq!(threads[0].comments.len(), 2);
    let r = pulls::resolve(&ctx, 7, "PRRT_thread1", true).await.unwrap();
    assert_eq!(r["resolved"], true);
    let gql = t.mock.requests("POST", "/api/graphql");
    assert!(gql.last().unwrap().body.as_ref().unwrap()["query"].as_str().unwrap().contains("resolveReviewThread"));
    assert_eq!(gql.last().unwrap().auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
}

#[tokio::test]
async fn writes_go_where_they_should() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let (_, feature_sha) = t.mock.shas.lock().clone();
    let mut events = t.state.events.subscribe();
    // Create: head is the current branch, base the default branch.
    let p = pulls::create_pr(&ctx, &pulls::CreatePr { title: "Add two".into(), draft: true, ..Default::default() }).await.unwrap();
    assert_eq!(p.number, 8);
    let b = t.mock.requests("POST", "/repos/mock/proj/pulls")[0].body.clone().unwrap();
    assert_eq!((b["head"].as_str(), b["base"].as_str(), b["draft"].as_bool()), (Some("feature"), Some("main"), Some(true)));
    let ev = events.recv().await.unwrap();
    assert_eq!((ev.kind.as_str(), ev.project_id.as_deref()), ("github.pr", Some(PID)));
    let same = pulls::CreatePr { title: "x".into(), base: Some("feature".into()), ..Default::default() };
    assert_eq!(pulls::create_pr(&ctx, &same).await.unwrap_err().code, "bad_request");
    // Merge is guarded by the reviewed head.
    let stale = pulls::MergeBody { sha: "1111111111111111111111111111111111111111".into(), ..Default::default() };
    let err = pulls::merge(&ctx, 7, &stale).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("Head branch was modified"));
    let ok = pulls::MergeBody { sha: feature_sha.clone(), method: Some("squash".into()), delete_branch: true, ..Default::default() };
    let d = pulls::merge(&ctx, 7, &ok).await.unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let mb = t.mock.requests("PUT", "/pulls/7/merge").last().unwrap().body.clone().unwrap();
    assert_eq!(mb["merge_method"], "squash");
    assert_eq!(t.mock.requests("DELETE", "/git/refs/heads/feature").len(), 1);
    // Reviews and comments.
    pulls::review(&ctx, 7, &pulls::ReviewBody { event: "APPROVE".into(), sha: Some(feature_sha.clone()), ..Default::default() }).await.unwrap();
    let rb = t.mock.requests("POST", "/pulls/7/reviews")[0].body.clone().unwrap();
    assert_eq!((rb["event"].as_str(), rb["commit_id"].as_str()), (Some("APPROVE"), Some(feature_sha.as_str())));
    let c = pulls::NewReviewComment { body: "Why?".into(), path: "a.txt".into(), line: Some(2), ..Default::default() };
    pulls::add_review_comment(&ctx, 7, &c).await.unwrap();
    let cb = t.mock.requests("POST", "/pulls/7/comments")[0].body.clone().unwrap();
    assert_eq!((cb["commit_id"].as_str(), cb["side"].as_str(), cb["line"].as_u64()), (Some(feature_sha.as_str()), Some("RIGHT"), Some(2)));
    pulls::reply(&ctx, 7, 501, "Because").await.unwrap();
    assert_eq!(t.mock.requests("POST", "/pulls/7/comments/501/replies").len(), 1);
    // Draft toggles go through GraphQL.
    pulls::update_pr(&ctx, 7, &pulls::UpdatePr { draft: Some(true), ..Default::default() }).await.ok();
    assert!(t.mock.requests("POST", "/api/graphql").iter().any(|r| r.body.as_ref().is_some_and(|b| b["variables"]["id"] == "PR_node7")));
    // Actions: re-run failed, dispatch with inputs.
    let run = ci::run_action(&ctx, 303, "rerun-failed-jobs").await.unwrap();
    assert_eq!(t.mock.requests("POST", "/actions/runs/303/rerun-failed-jobs").len(), 1);
    assert!(matches!(run.state.as_str(), "running" | "pending"));
    let info = ci::workflow_inputs(&ctx, 11, None).await.unwrap();
    assert!(info.dispatchable);
    assert_eq!(info.inputs[0].options, ["staging", "prod"]);
    let mut inputs = serde_json::Map::new();
    inputs.insert("env".into(), json!("staging"));
    ci::dispatch(&ctx, 11, &ci::DispatchBody { git_ref: "main".into(), inputs }).await.unwrap();
    let db = t.mock.requests("POST", "/actions/workflows/11/dispatches")[0].body.clone().unwrap();
    assert_eq!(db, json!({ "ref": "main", "inputs": { "env": "staging" } }));
    // Issues.
    let i = misc::create_issue(&ctx, &misc::CreateIssue { title: "Broken".into(), ..Default::default() }).await.unwrap();
    assert_eq!(i.number, 9);
    misc::update_issue(&ctx, 9, &misc::UpdateIssue { state: Some("closed".into()), state_reason: Some("completed".into()), ..Default::default() })
        .await
        .unwrap();
    misc::add_issue_comment(&ctx, 9, "Done").await.unwrap();
    let list = misc::list_issues(&ctx, &misc::IssuesQuery::default()).await.unwrap();
    assert_eq!(list.items.iter().map(|i| i.number).collect::<Vec<_>>(), [3], "pull requests are filtered out");
    // Every write carried the token.
    for r in t.mock.log.lock().iter().filter(|r| r.method != "GET") {
        assert_eq!(r.auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()), "{} {}", r.method, r.path);
    }
}

#[tokio::test]
async fn upstream_bodies_are_never_echoed() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let err = misc::issue_detail(&ctx, 500).await.unwrap_err();
    assert_eq!(err.code, "upstream");
    assert!(!err.message.contains(TOKEN));
    assert!(!err.message.contains("stack trace"));
    let err = pulls::list_pulls(&ctx, &pulls::PullsQuery { state: Some("bogus".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!(err.code, "bad_request");
}

#[tokio::test]
async fn routes_serialize_camel_case_and_map_errors() {
    let t = setup().await;
    let mctx = McpCtx::default();
    let v = crate::mcp::call_api(&t.state, Method::GET, "/api/projects/repo/github/actions/runs?perPage=2", None, &mctx).await.unwrap();
    assert_eq!(v["items"][0]["htmlUrl"].as_str().map(|u| u.ends_with("/actions/runs/303")), Some(true));
    assert_eq!(v["nextPage"], 2);
    let v = crate::mcp::call_api(&t.state, Method::GET, "/api/projects/repo/github/pulls/7", None, &mctx).await.unwrap();
    assert_eq!(v["mergeableState"], "clean");
    assert_eq!(v["head"]["ref"], "feature");
    assert!(v["mergeBaseSha"].is_string());
    let v = crate::mcp::call_api(&t.state, Method::GET, "/api/projects/repo/github/summary", None, &mctx).await.unwrap();
    assert_eq!(v["path"], "mock/proj");
    assert_eq!(v["branch"], "feature");
    assert_eq!(v["currentPr"]["number"], 7);
    assert_eq!(v["openPrCount"], 3);
    assert_eq!(v["openIssueCount"], 2);
    assert_eq!(v["auth"]["authenticated"], true);
    assert_eq!(v["auth"]["viewer"], "dev");
    assert_eq!(v["canPush"], true);
    assert_eq!(v["mergeMethods"]["rebase"], false);
    assert_eq!(v["branchRun"]["id"], 301);
    let v = crate::mcp::call_api(&t.state, Method::GET, "/api/projects/repo/github/actions/jobs/9001/logs", None, &mctx).await.unwrap();
    assert_eq!(v["available"], true);
    assert_eq!(v["steps"][1]["number"], 2);
    assert_eq!(v["job"]["state"], "failed");
    let err = crate::mcp::call_api(&t.state, Method::GET, "/api/projects/nope/github/summary", None, &mctx).await.unwrap_err();
    assert_eq!(err.status, StatusCode::NOT_FOUND);
    let err = crate::mcp::call_api(&t.state, Method::POST, "/api/projects/repo/github/pulls/7/merge", Some(json!({ "sha": "zzz" })), &mctx)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mcp_tools_use_the_session_project() {
    let t = setup().await;
    let tools = super::mcp_tools();
    let find = |n: &str| tools.iter().find(|x| x.name == n).unwrap().handler.clone();
    let mctx = McpCtx { terminal_id: None, project_id: Some(PID.into()) };
    let out = find("github_job_log")(t.state.clone(), mctx.clone(), json!({ "jobId": 9001, "tailLines": 50 })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.contains("error[E0308]: mismatched types"), "{text}");
    assert!(!text.contains('\x1b'));
    assert!(text.contains("failed step: Run tests"), "{text}");
    let out = find("github_pr")(t.state.clone(), mctx.clone(), json!({ "number": 7 })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.contains("Review threads: 1 (1 unresolved)"), "{text}");
    let out = find("github_pr_diff")(t.state.clone(), mctx.clone(), json!({ "number": 7, "path": "f2.rs" })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.starts_with("diff --git a/f2.rs b/f2.rs\n--- a/f2.rs\n+++ b/f2.rs\n@@"), "{text}");
    let out = find("github_rerun")(t.state.clone(), mctx.clone(), json!({ "runId": 303, "failedOnly": true })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.contains("the failed jobs of run 303"), "{text}");
    // Without a session project the caller must say which project.
    let err = find("github_prs")(t.state.clone(), McpCtx::default(), json!({})).await.unwrap_err();
    assert_eq!(err.code, "bad_request");
}

#[tokio::test]
async fn mcp_sessions_cannot_reach_other_projects() {
    let t = setup().await;
    let tools = super::mcp_tools();
    let find = |n: &str| tools.iter().find(|x| x.name == n).unwrap().handler.clone();
    let other = McpCtx { terminal_id: Some("t-other".into()), project_id: Some("elsewhere".into()) };
    for (name, args) in [
        ("github_rerun", json!({ "projectId": PID, "runId": 303 })),
        ("github_pr_comment", json!({ "projectId": PID, "number": 7, "body": "hi" })),
        ("github_create_pr", json!({ "projectId": PID, "title": "x" })),
        ("github_job_log", json!({ "projectId": PID, "jobId": 9001 })),
        ("github_runs", json!({ "projectId": PID })),
    ] {
        let err = find(name)(t.state.clone(), other.clone(), args).await.unwrap_err();
        assert_eq!(err.code, "forbidden", "{name}: {}", err.message);
    }
    assert!(t.mock.log.lock().is_empty(), "nothing reached GitHub: {:?}", t.mock.log.lock());
    let own = McpCtx { terminal_id: Some("t1".into()), project_id: Some(PID.into()) };
    find("github_runs")(t.state.clone(), own, json!({ "projectId": PID })).await.unwrap();
}

#[tokio::test]
async fn poller_emits_on_run_changes_only() {
    let t = setup().await;
    let project = t.state.projects.get(PID).unwrap();
    let mut events = t.state.events.subscribe();
    // First sight of each ref: recorded, not announced. The main run is in progress.
    let (active, anon) = super::poller::poll_project(&t.state, project.clone()).await.unwrap();
    assert!(active && !anon);
    *t.mock.run_status.lock() = ("completed".into(), Some("failure".into()));
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let (active, _) = super::poller::poll_project(&t.state, project.clone()).await.unwrap();
    assert!(!active);
    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert_eq!(ev.kind, "github.run");
    assert_eq!(ev.data["runId"], 303);
    assert_eq!(ev.data["state"], "failed");
    assert_eq!(ev.data["previousState"], "running");
    tokio::time::sleep(Duration::from_millis(3200)).await;
    super::poller::poll_project(&t.state, project).await.unwrap();
    assert!(events.try_recv().is_err(), "unchanged: nothing more");
}

#[tokio::test]
async fn merged_pull_requests_diff_from_the_branch_point() {
    let t = setup().await;
    let (main_sha, feature_sha) = t.mock.shas.lock().clone();
    // `main` moves on after `feature` branched off, then the PR is merged.
    let repo = t.state.projects.get(PID).unwrap().root.clone();
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "zero\none\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "Later on main"]);
    let later = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "feature"]);
    *t.mock.merged_base.lock() = later.clone();
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let d = pulls::get_pull(&ctx, 9).await.unwrap();
    assert!(d.pull.merged);
    assert_eq!(d.merge_base_sha.as_deref(), Some(main_sha.as_str()), "where the branches met, not main's tip at the merge");
    // Both ways of asking for a file agree: without shas (looked up) and with the detail's.
    let v = pulls::pr_file(&ctx, 9, &pulls::FileQuery { path: "a.txt".into(), ..Default::default() }).await.unwrap();
    assert_eq!((v.original.as_str(), v.modified.as_str()), ("one\n", "one\ntwo\n"), "main's later edit is not the PR's");
    assert_eq!((v.base_sha.as_str(), v.head_sha.as_str()), (main_sha.as_str(), feature_sha.as_str()));
    // A merged head already contained in the recorded base would diff to nothing: the recorded base stays.
    let d = pulls::get_pull(&ctx, 10).await.unwrap();
    assert_eq!(d.merge_base_sha.as_deref(), Some(later.as_str()));
    let v = pulls::pr_file(&ctx, 10, &pulls::FileQuery { path: "a.txt".into(), ..Default::default() }).await.unwrap();
    assert_eq!(v.base_sha, later);
}

#[tokio::test]
async fn summary_follows_the_local_checkout() {
    // Anonymous: the summary is shared for minutes, but not across a checkout or commit.
    let t = setup_with(false).await;
    let (main_sha, feature_sha) = t.mock.shas.lock().clone();
    let mctx = McpCtx::default();
    let url = "/api/projects/repo/github/summary";
    let v = crate::mcp::call_api(&t.state, Method::GET, url, None, &mctx).await.unwrap();
    assert_eq!((v["branch"].as_str(), v["head"].as_str()), (Some("feature"), Some(feature_sha.as_str())));
    assert_eq!(v["headStatus"]["pipelineId"], 301);
    let sent = t.mock.log.lock().len();
    let again = crate::mcp::call_api(&t.state, Method::GET, url, None, &mctx).await.unwrap();
    assert_eq!(again["fetchedAt"], v["fetchedAt"], "same checkout: the shared summary");
    assert_eq!(t.mock.log.lock().len(), sent);
    let repo = t.state.projects.get(PID).unwrap().root.clone();
    git(&repo, &["checkout", "-q", "main"]);
    let v = crate::mcp::call_api(&t.state, Method::GET, url, None, &mctx).await.unwrap();
    assert_eq!((v["branch"].as_str(), v["head"].as_str()), (Some("main"), Some(main_sha.as_str())));
    assert!(v["headStatus"].is_null(), "the old HEAD's checks are gone: {}", v["headStatus"]);
    assert!(v["currentPr"].is_null());
}

#[tokio::test]
async fn private_repositories_without_a_token_are_setup_help_asked_once() {
    let t = setup_with(false).await;
    let mut p = (*t.state.projects.get(PID).unwrap()).clone();
    if let Some(g) = p.config.repo.as_mut().and_then(|r| r.github.as_mut()) {
        g.path = "mock/secret".into();
    }
    let p = Arc::new(p);
    for _ in 0..3 {
        let err = match client::ctx_for(&t.state, p.clone()).await {
            Err(e) => e,
            Ok(_) => panic!("a missing repository has no context"),
        };
        assert_eq!(err.code, "not_configured", "{}", err.message);
        assert!(err.message.contains("mock/secret") && err.message.contains("token"), "{}", err.message);
    }
    assert_eq!(t.mock.requests("GET", "/repos/mock/secret").len(), 1, "the 404 is remembered");
    // With a token a 404 means the token cannot see it: not found, remembered briefly.
    let t = setup().await;
    let mut p = (*t.state.projects.get(PID).unwrap()).clone();
    if let Some(g) = p.config.repo.as_mut().and_then(|r| r.github.as_mut()) {
        g.path = "mock/secret".into();
    }
    let p = Arc::new(p);
    for _ in 0..2 {
        let err = match client::ctx_for(&t.state, p.clone()).await {
            Err(e) => e,
            Ok(_) => panic!("a missing repository has no context"),
        };
        assert_eq!(err.code, "not_found");
        assert!(err.message.contains("this token cannot see it"), "{}", err.message);
    }
    assert_eq!(t.mock.requests("GET", "/repos/mock/secret").len(), 1);
}

#[tokio::test]
async fn writes_refresh_the_repository_metadata() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let _ = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert_eq!(t.mock.requests("GET", "/repos/mock/proj").len(), 1);
    // The open issue count comes from the metadata: a new issue must show in it.
    misc::create_issue(&ctx, &misc::CreateIssue { title: "Broken".into(), ..Default::default() }).await.unwrap();
    let _ = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert_eq!(t.mock.requests("GET", "/repos/mock/proj").len(), 2, "metadata refetched after the write");
}

#[tokio::test]
async fn unknown_commits_have_no_ci_status() {
    // Contract (as GitLab's): a commit GitHub has never seen is `None`, not an error.
    let t = setup().await;
    let project = t.state.projects.get(PID).unwrap();
    assert!(super::commit_ci_status(&t.state, &project, UNKNOWN_SHA).await.unwrap().is_none());
    assert!(crate::forge::commit_ci_status(&t.state, &project, UNKNOWN_SHA).await.unwrap().is_none());
    let url = format!("/api/projects/repo/github/commits/{UNKNOWN_SHA}/checks");
    let v = crate::mcp::call_api(&t.state, Method::GET, &url, None, &McpCtx::default()).await.unwrap();
    assert!(v.is_null(), "{v}");
}

#[tokio::test]
async fn the_anonymous_poller_watches_only_projects_someone_looks_at() {
    let t = setup_with(false).await;
    let project = t.state.projects.get(PID).unwrap();
    assert!(!super::poller::wants_poll(&t.state, &project), "nobody looked at it yet");
    let _ = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    assert!(super::poller::wants_poll(&t.state, &project), "the UI asked for it");
    // Background lookups (the poller, deploy gates) do not count as looking.
    let t = setup_with(false).await;
    let project = t.state.projects.get(PID).unwrap();
    let _ = super::poller::poll_project(&t.state, project.clone()).await.unwrap();
    let _ = super::commit_ci_status(&t.state, &project, RUN_SHA).await.unwrap();
    assert!(!super::poller::wants_poll(&t.state, &project));
    // With a token polling is free (304s): always.
    let t = setup().await;
    assert!(super::poller::wants_poll(&t.state, &t.state.projects.get(PID).unwrap()));
}

#[tokio::test]
async fn a_changed_run_drops_its_cached_detail_and_checks() {
    let t = setup().await;
    let ctx = client::ctx(&t.state, PID, &RepoParam::default()).await.unwrap();
    let count = |suffix: &str| t.mock.requests("GET", suffix).len();
    let branch_lists = || t.mock.log.lock().iter().filter(|r| r.path.ends_with("/actions/runs") && r.query.contains("branch=main")).count();
    let by_sha = || t.mock.log.lock().iter().filter(|r| r.path.ends_with("/actions/runs") && r.query.contains("head_sha=")).count();
    let reads = || async {
        let _ = ci::get_run(&ctx, 303).await.unwrap();
        let _ = ci::get_run(&ctx, 3030).await.unwrap();
        let _ = ci::get_job(&ctx, 9001).await.unwrap();
        let _ = ci::commit_checks(&ctx, RUN_SHA).await.unwrap();
        let _ = ci::branch_runs(&ctx, "main", 10).await.unwrap();
    };
    reads().await;
    let before = (count("/actions/runs/303"), count("/actions/runs/3030"), count("/check-runs"), by_sha(), branch_lists(), count("/actions/jobs/9001"));
    ctx.forget_run(303, RUN_SHA);
    reads().await;
    assert_eq!(count("/actions/jobs/9001"), before.5 + 1, "job details are asked again");
    assert_eq!(count("/actions/runs/303"), before.0 + 1, "the changed run is asked again");
    assert_eq!(count("/actions/runs/3030"), before.1, "another run stays cached");
    assert_eq!(count("/check-runs"), before.2 + 1, "its commit's checks are asked again");
    assert_eq!(by_sha(), before.3 + 1, "and the commit's runs");
    assert_eq!(branch_lists(), before.4, "the branch list the poller just fetched stays");
}

// ---------------------------------------------------------------- several repositories

/// The root repository on `mock/proj` and `api/` on `other/api`, each with its own token.
fn api_repo(base: &str) -> String {
    format!("\n[[repository]]\npath = \"api\"\n[repository.github]\nhost = \"{base}\"\npath = \"other/api\"\ntoken = \"api\"\n")
}

/// The root repository (branch `feature`), `api` (branch `dev`) and a repository on no
/// forge (`web`, found below the root).
async fn setup_repos() -> Setup {
    setup_custom(true, api_repo, &[("api", "dev"), ("web", "main")], &[("api", TOKEN_B)]).await
}

async fn get(state: &AppState, path: &str) -> Result<Value, crate::error::ApiError> {
    crate::mcp::call_api(state, Method::GET, path, None, &McpCtx::default()).await
}

#[tokio::test]
async fn each_repository_uses_its_own_github_repository_and_token() {
    let t = setup_repos().await;
    let project = t.state.projects.get(PID).unwrap();
    let ids: Vec<&str> = project.repos.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, [".", "api", "web"], "{:?}", project.warnings);

    for path in ["/api/projects/repo/github/actions/runs", "/api/projects/repo/github/actions/runs?repo=.", "/api/projects/repo/github/actions/runs?repo="] {
        let v = get(&t.state, path).await.unwrap();
        assert!(v.to_string().contains("\"id\":303"), "{path}: {v}");
    }
    let v = get(&t.state, "/api/projects/repo/github/actions/runs?repo=api").await.unwrap();
    assert!(v.to_string().contains("\"id\":888") && !v.to_string().contains("\"id\":303"), "{v}");
    for r in t.mock.log.lock().iter().filter(|r| r.path != "/api/graphql") {
        let second = r.path.contains("/repos/other/api") || r.path.contains("/repositories/77");
        let want = format!("Bearer {}", if second { TOKEN_B } else { TOKEN });
        assert_eq!(r.auth.as_deref(), Some(want.as_str()), "{} {}", r.method, r.path);
    }
    assert!(!t.mock.requests("GET", "/repos/other/api/actions/runs").is_empty());

    // Summaries are cached per repository, and read the repository's own checkout.
    let a = get(&t.state, "/api/projects/repo/github/summary").await.unwrap();
    let b = get(&t.state, "/api/projects/repo/github/summary?repo=api").await.unwrap();
    assert_eq!((a["path"].as_str(), a["branch"].as_str()), (Some("mock/proj"), Some("feature")));
    assert_eq!((b["path"].as_str(), b["branch"].as_str()), (Some("other/api"), Some("dev")));
    assert_eq!(get(&t.state, "/api/projects/repo/github/summary").await.unwrap()["path"], "mock/proj");

    let e = get(&t.state, "/api/projects/repo/github/summary?repo=web").await.unwrap_err();
    assert_eq!(e.code, "not_configured", "{}", e.message);
    let e = get(&t.state, "/api/projects/repo/github/actions/runs?repo=nope").await.unwrap_err();
    assert_eq!((e.status, e.code), (StatusCode::NOT_FOUND, "unknown_repo"));
}

#[tokio::test]
async fn actions_name_their_repository_in_events_and_use_its_checkout() {
    let t = setup_repos().await;
    let mut events = t.state.events.subscribe();
    let ctx = McpCtx::default();
    let body = json!({ "title": "API change" });
    crate::mcp::call_api(&t.state, Method::POST, "/api/projects/repo/github/pulls?repo=api", Some(body), &ctx).await.unwrap();
    let sent = t.mock.requests("POST", "/repos/other/api/pulls");
    assert_eq!(sent[0].body.as_ref().unwrap()["head"], "dev", "the head is the current branch of that repository");
    let ev = events.try_recv().unwrap();
    assert_eq!((ev.kind.as_str(), ev.project_id.as_deref()), ("github.pr", Some(PID)));
    assert_eq!((ev.data["number"].clone(), ev.data["repo"].clone()), (json!(4), json!("api")));
}

#[tokio::test]
async fn the_poller_watches_every_repository_on_github() {
    let t = setup_repos().await;
    let views: Vec<_> = crate::forge::repo_views(&t.state).into_iter().filter(|p| p.github().is_some()).collect();
    let keys: Vec<String> = views.iter().map(|p| p.scope_key()).collect();
    assert_eq!(keys, ["repo", "repo@api"], "`web` is on no forge and is not polled");
    let mut events = t.state.events.subscribe();
    for p in &views {
        assert!(super::poller::wants_poll(&t.state, p));
        let (active, _) = super::poller::poll_project(&t.state, p.clone()).await.unwrap();
        assert!(active);
    }
    *t.mock.run_status.lock() = ("completed".into(), Some("failure".into()));
    *t.mock.api_run.lock() = ("completed".into(), Some("failure".into()));
    tokio::time::sleep(Duration::from_millis(3200)).await;
    for p in &views {
        super::poller::poll_project(&t.state, p.clone()).await.unwrap();
    }
    let mut changes = vec![];
    while let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_millis(200), events.recv()).await {
        if ev.kind == "github.run" {
            assert_eq!(ev.project_id.as_deref(), Some(PID));
            changes.push((ev.data["repo"].as_str().unwrap().to_string(), ev.data["runId"].as_u64().unwrap(), ev.data["state"].as_str().unwrap().to_string()));
        }
    }
    changes.sort();
    assert_eq!(changes, [(".".to_string(), 303, "failed".to_string()), ("api".to_string(), 888, "failed".to_string())]);
    // What each repository saw is remembered under its own key.
    assert!(t.state.github.poll.observe("repo@api", "dev", &[(888, "failed".into())]).is_empty());
}

#[tokio::test]
async fn tools_and_the_deploy_gate_take_a_repository() {
    let t = setup_repos().await;
    let tools = super::mcp_tools();
    let find = |n: &str| tools.iter().find(|t| t.name == n).unwrap().clone();
    for tool in &tools {
        assert!(tool.input_schema["properties"].get("repo").is_some(), "{} takes a repo", tool.name);
    }
    let mctx = McpCtx { terminal_id: None, project_id: Some(PID.into()) };
    let ToolOutput::Text(text) = (find("github_runs").handler)(t.state.clone(), mctx.clone(), json!({ "repo": "api" })).await.unwrap() else {
        panic!("text output")
    };
    assert!(text.contains("other/api") && text.contains("888"), "{text}");
    let e = (find("github_runs").handler)(t.state.clone(), mctx.clone(), json!({ "repo": "nope" })).await.unwrap_err();
    assert_eq!(e.code, "unknown_repo");

    // A panel opened for a repository other than the default one carries it, in its id too.
    let mut events = t.state.events.subscribe();
    for (repo, want, id) in [(json!({ "repo": "api" }), Some("api"), "pr:repo::api:4"), (json!({}), None, "pr:repo:8")] {
        let mut args = json!({ "title": "x" });
        args.as_object_mut().unwrap().extend(repo.as_object().unwrap().clone());
        (find("github_create_pr").handler)(t.state.clone(), mctx.clone(), args).await.unwrap();
        let ev = loop {
            let ev = events.recv().await.unwrap();
            if ev.kind == "ui.open" {
                break ev;
            }
        };
        assert_eq!(ev.data["params"]["projectId"], PID);
        assert_eq!(ev.data["params"]["repo"].as_str(), want, "{:?}", ev.data);
        assert_eq!(ev.data["id"], id, "{:?}", ev.data);
    }

    // A deploy's CI gate asks about the project it is given: a view through `api` is `other/api`.
    let project = t.state.projects.get(PID).unwrap();
    let api = project.scoped("api").unwrap();
    assert_eq!(crate::forge::forge_of(&api), Some(crate::forge::Forge::Github));
    assert_eq!(api.github().unwrap().1, "other/api");
    assert_eq!(project.github().unwrap().1, "mock/proj");
}

/// `[[repository]]` comes from repository content: its token name resolves against the
/// machine overlay only, never against config.toml's secrets.
#[tokio::test]
async fn a_repositorys_token_name_resolves_only_against_the_overlay() {
    let more = |base: &str| format!("\n[[repository]]\npath = \"api\"\n[repository.github]\nhost = \"{base}\"\npath = \"other/api\"\ntoken = \"globaltok\"\n");
    let t = setup_custom(true, more, &[("api", "dev")], &[]).await;
    let global = t._dir.path().join("global-token");
    crate::util::fs::write_atomic(&global, TOKEN_B.as_bytes(), 0o600).unwrap();
    t.state.config.write().secrets.insert("globaltok".into(), crate::config::SecretRef::File(global.display().to_string()));
    t.mock.log.lock().clear();
    let e = get(&t.state, "/api/projects/repo/github/actions/runs?repo=api").await.unwrap_err();
    assert_eq!(e.code, "not_configured", "{}", e.message);
    assert!(e.message.contains("machine overlay"), "{}", e.message);
    assert!(t.mock.log.lock().is_empty(), "nothing reached GitHub: {:?}", t.mock.log.lock());
}

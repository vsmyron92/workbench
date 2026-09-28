//! Integration tests against a mock GitLab (axum on 127.0.0.1:0). Nothing here
//! talks to gitlab.com. The project under test is a throwaway git repository
//! whose `.workbench.toml` points `[repo.gitlab]` at the mock.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::client;
use super::{mrs, pipelines};
use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::mcp::{McpCtx, ToolOutput};

const TOKEN: &str = "mock-token-value-0123456789";
const HEAD_SHA: &str = "5babfd548d64a14eabeba53b847fdec5fa5f0ca9";
const PID: &str = "repo";

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    query: String,
    body: Option<Value>,
    token: Option<String>,
}

#[derive(Default)]
struct Mock {
    base: Mutex<String>,
    log: Mutex<Vec<Req>>,
    trace: Mutex<String>,
    job_status: Mutex<String>,
    pipeline_status: Mutex<String>,
    rate_limit_hits: AtomicUsize,
    /// Serve the whole trace (200) even when a Range is asked for.
    ignore_range: std::sync::atomic::AtomicBool,
}

impl Mock {
    fn requests(&self, method: &str, path_suffix: &str) -> Vec<Req> {
        self.log.lock().iter().filter(|r| r.method == method && r.path.ends_with(path_suffix)).cloned().collect()
    }
}

fn query_map(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_default()))
        .collect()
}

fn mr_json(base: &str, iid: u64, title: &str) -> Value {
    json!({
        "id": 1000 + iid, "iid": iid, "project_id": 42, "title": title, "state": "opened", "draft": false,
        "source_branch": "feature", "target_branch": "main", "sha": HEAD_SHA,
        "web_url": format!("{base}/mock/proj/-/merge_requests/{iid}"),
        "diff_refs": { "base_sha": "aaaaaaaa11111111aaaaaaaa11111111aaaaaaaa", "head_sha": HEAD_SHA, "start_sha": "bbbbbbbb22222222bbbbbbbb22222222bbbbbbbb" },
        "detailed_merge_status": "mergeable", "user": { "can_merge": true }, "references": { "short": format!("!{iid}"), "full": format!("mock/proj!{iid}") },
        "author": { "id": 1, "username": "dev", "name": "Dev", "avatar_url": "http://x/avatar" }
    })
}

fn pipeline_json(base: &str, id: u64, status: &str) -> Value {
    json!({ "id": id, "iid": id - 100, "project_id": 42, "sha": HEAD_SHA, "ref": "main", "status": status, "source": "push",
            // updated_at moves with the status, as on GitLab (detail caches key on it).
            "created_at": "2026-09-26T10:00:00Z", "updated_at": format!("2026-09-26T10:0{}:{:02}Z", id % 10, status.len()),
            "web_url": format!("{base}/mock/proj/-/pipelines/{id}") })
}

async fn mock_handler(State(m): State<Arc<Mock>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let base = m.base.lock().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let token = headers.get("private-token").and_then(|v| v.to_str().ok()).map(str::to_string);
    m.log.lock().push(Req {
        method: method.to_string(),
        path: path.clone(),
        query: query.clone(),
        body: serde_json::from_slice(&body).ok(),
        token: token.clone(),
    });
    if token.as_deref() != Some(TOKEN) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "message": "401 Unauthorized" }))).into_response();
    }
    let q = query_map(&query);
    let rel = path.strip_prefix("/api/v4/").unwrap_or(&path);
    let segs: Vec<&str> = rel.split('/').collect();
    let page: u64 = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
    match (method.as_str(), segs.as_slice()) {
        ("GET", ["projects", "mock%2Fproj"]) | ("GET", ["projects", "42"]) => Json(json!({
            "id": 42, "path_with_namespace": "mock/proj", "default_branch": "main",
            "web_url": format!("{base}/mock/proj"), "container_registry_enabled": true, "issues_enabled": true,
            "permissions": { "project_access": { "access_level": 40 }, "group_access": null }
        }))
        .into_response(),
        ("GET", ["projects", "42", "pipelines"]) if q.get("ref").is_some_and(|r| r != "main") => {
            Json(Vec::<Value>::new()).into_response()
        }
        ("GET", ["projects", "42", "pipelines"]) => {
            let per_page: u64 = q.get("per_page").and_then(|p| p.parse().ok()).unwrap_or(20);
            let status = m.pipeline_status.lock().clone();
            let all: Vec<Value> = (0..3u64).map(|i| pipeline_json(&base, 303 - i, if i == 0 { &status } else { "success" })).collect();
            let start = ((page - 1) * per_page) as usize;
            let items: Vec<Value> = all.iter().skip(start).take(per_page as usize).cloned().collect();
            let mut resp = Json(items).into_response();
            let h = resp.headers_mut();
            h.insert("x-total", HeaderValue::from_static("3"));
            h.insert("x-page", HeaderValue::from_str(&page.to_string()).unwrap());
            if (start as u64) + per_page < 3 {
                h.insert("x-next-page", HeaderValue::from_str(&(page + 1).to_string()).unwrap());
                let link = format!(
                    "<{base}/api/v4/projects/42/pipelines?page={}&per_page={per_page}>; rel=\"next\", <{base}/api/v4/projects/42/pipelines?page=1&per_page={per_page}>; rel=\"first\"",
                    page + 1
                );
                h.insert(header::LINK, HeaderValue::from_str(&link).unwrap());
            }
            resp
        }
        ("GET", ["projects", "42", "pipelines", "999"]) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("<html>stack trace with {TOKEN} and internals</html>"),
        )
            .into_response(),
        ("GET", ["projects", "42", "pipelines", "303", "test_report"]) => Json(json!({
            "total_time": 1.5, "total_count": 3, "success_count": 1, "failed_count": 1, "skipped_count": 0, "error_count": 1,
            "test_suites": [{
                "name": "unit", "total_time": 1.5, "total_count": 3, "success_count": 1, "failed_count": 1, "skipped_count": 0, "error_count": 1,
                "test_cases": [
                    { "status": "success", "name": "passes", "classname": "m", "execution_time": 0.1 },
                    { "status": "failed", "name": "adds", "classname": "calc", "file": "src/calc.rs", "execution_time": 0.2,
                      "system_output": "assertion `left == right` failed\n  left: 4\n right: 3", "stack_trace": null },
                    { "status": "error", "name": "setup", "classname": "db", "execution_time": 0.0, "system_output": "connection refused" }
                ]
            }]
        }))
        .into_response(),
        ("GET", ["projects", "42", "pipelines", id]) => {
            let id: u64 = id.parse().unwrap_or(0);
            let status = if id == 303 { m.pipeline_status.lock().clone() } else { "success".into() };
            let mut p = pipeline_json(&base, id, &status);
            p["duration"] = json!(343);
            p["user"] = json!({ "id": 1, "username": "dev", "name": "Dev" });
            Json(p).into_response()
        }
        ("POST", ["projects", "42", "pipeline"]) => Json(pipeline_json(&base, 304, "created")).into_response(),
        ("GET", ["projects", "42", "merge_requests"]) => {
            if m.rate_limit_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut r = (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "message": "Retry later" }))).into_response();
                r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                return r;
            }
            let mut resp = Json(vec![mr_json(&base, 7, "Existing")]).into_response();
            resp.headers_mut().insert("x-total", HeaderValue::from_static("1"));
            resp
        }
        ("POST", ["projects", "42", "merge_requests"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            let mut mr = mr_json(&base, 8, b["title"].as_str().unwrap_or(""));
            mr["source_branch"] = b["source_branch"].clone();
            mr["target_branch"] = b["target_branch"].clone();
            (StatusCode::CREATED, Json(mr)).into_response()
        }
        ("GET", ["projects", "42", "merge_requests", iid]) => {
            Json(mr_json(&base, iid.parse().unwrap_or(0), "Existing")).into_response()
        }
        ("GET", ["projects", "42", "merge_requests", _, "approvals"]) => Json(json!({
            "approved": false, "approvals_required": 1, "approvals_left": 1, "approved_by": [],
            "user_can_approve": true, "user_has_approved": false
        }))
        .into_response(),
        ("PUT", ["projects", "42", "merge_requests", iid, "merge"]) => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            if b["sha"].as_str() != Some(HEAD_SHA) {
                return (StatusCode::CONFLICT, Json(json!({ "message": "SHA does not match HEAD of source branch" })))
                    .into_response();
            }
            let mut mr = mr_json(&base, iid.parse().unwrap_or(0), "Existing");
            mr["state"] = json!("merged");
            Json(mr).into_response()
        }
        ("GET", ["projects", "42", "merge_requests", _, "diffs"]) => {
            let file = |n: u64| json!({ "old_path": format!("f{n}.rs"), "new_path": format!("f{n}.rs"), "diff": "@@ -1 +1,2 @@\n-a\n+b\n+c\n", "new_file": false });
            let mut resp = Json(vec![file(page * 2 - 1), file(page * 2)]).into_response();
            if page < 2 {
                let link = format!("<{base}/api/v4/projects/42/merge_requests/7/diffs?page=2&per_page=100>; rel=\"next\"");
                resp.headers_mut().insert(header::LINK, HeaderValue::from_str(&link).unwrap());
            }
            resp
        }
        ("POST", ["projects", "42", "merge_requests", _, "discussions"]) => Json(json!({
            "id": "0123abcd", "individual_note": false,
            "notes": [{ "id": 5, "type": "DiffNote", "body": "x", "resolvable": true, "resolved": false }]
        }))
        .into_response(),
        ("GET", ["projects", "42", "jobs", id]) => Json(json!({
            "id": id.parse::<u64>().unwrap_or(0), "name": "server-tests", "stage": "test", "status": m.job_status.lock().clone(),
            "ref": "main", "pipeline": { "id": 303, "status": "running", "ref": "main", "sha": HEAD_SHA, "web_url": "" },
            "failure_reason": "script_failure"
        }))
        .into_response(),
        ("GET", ["projects", "42", "jobs", _, "trace"]) => {
            let trace = m.trace.lock().clone();
            let len = trace.len();
            let from = headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.trim_end_matches('-').parse::<usize>().ok())
                .filter(|_| !m.ignore_range.load(Ordering::SeqCst));
            match from {
                Some(n) if n >= len => {
                    let mut r = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                    r.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes */{len}")).unwrap());
                    r
                }
                Some(n) => {
                    let mut r = (StatusCode::PARTIAL_CONTENT, trace[n..].to_string()).into_response();
                    let cr = format!("bytes {n}-{}/{len}", len - 1);
                    r.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&cr).unwrap());
                    r
                }
                None => trace.into_response(),
            }
        }
        ("POST", ["projects", "42", "jobs", _, "retry"]) => Json(json!({
            "id": 9001, "name": "server-tests", "stage": "test", "status": "pending",
            "pipeline": { "id": 303, "status": "running", "ref": "main", "sha": HEAD_SHA }
        }))
        .into_response(),
        ("GET", ["projects", "42", "repository", "commits", sha]) => {
            if HEAD_SHA.starts_with(sha) {
                Json(json!({ "id": HEAD_SHA, "short_id": &HEAD_SHA[..8], "title": "t",
                             "last_pipeline": { "id": 303, "status": "success", "ref": "main", "sha": HEAD_SHA, "web_url": format!("{base}/p/303") } }))
                .into_response()
            } else {
                (StatusCode::NOT_FOUND, Json(json!({ "message": "404 Commit Not Found" }))).into_response()
            }
        }
        _ => (StatusCode::NOT_FOUND, Json(json!({ "message": "404 Not found" }))).into_response(),
    }
}

async fn start_mock() -> (String, Arc<Mock>) {
    let m = Arc::new(Mock::default());
    *m.job_status.lock() = "running".into();
    *m.pipeline_status.lock() = "running".into();
    let app = Router::new().fallback(mock_handler).with_state(m.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *m.base.lock() = base.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, m)
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(ok, "git {args:?} failed");
}

/// A Workbench state whose only project (`repo`, on branch `feature`) points at the mock.
async fn setup() -> (AppState, Arc<Mock>, tempfile::TempDir) {
    let (state, mock, dir, _) =
        setup_with(|base| format!("[repo.gitlab]\nhost = \"{base}\"\npath = \"mock/proj\"\ntoken = \"mock\"\n")).await;
    (state, mock, dir)
}

/// `setup` with the repository's `.workbench.toml` built from the mock's origin
/// (`http://127.0.0.1:<port>`, also returned). The token is in `<dir>/token`.
async fn setup_with(workbench_toml: impl FnOnce(&str) -> String) -> (AppState, Arc<Mock>, tempfile::TempDir, String) {
    let (base, mock) = start_mock().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "feature"]);
    let token_file = dir.path().join("token");
    crate::util::fs::write_atomic(&token_file, TOKEN.as_bytes(), 0o600).unwrap();
    std::fs::write(repo.join(".workbench.toml"), workbench_toml(&base)).unwrap();
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(paths.config_dir.join("projects")).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    // Secret references live in the machine overlay; the committed file only names them.
    std::fs::write(paths.project_overlay(PID), format!("[secrets]\nmock = {{ file = \"{}\" }}\n", token_file.display())).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![repo.display().to_string()];
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let _router = crate::app::build_router(state.clone());
    assert!(state.projects.get(PID).is_some(), "test project registered");
    (state, mock, dir, base)
}

#[tokio::test]
async fn resolves_project_and_sends_token_only_in_header() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    assert_eq!(ctx.id(), 42);
    assert_eq!(ctx.default_branch(), Some("main"));
    // The numeric id is resolved once and cached.
    let _ = client::ctx(&state, PID).await.unwrap();
    assert_eq!(mock.requests("GET", "/projects/mock%2Fproj").len(), 1);
    for r in mock.log.lock().iter() {
        assert_eq!(r.token.as_deref(), Some(TOKEN));
        assert!(!r.query.contains(TOKEN));
    }
}

#[tokio::test]
async fn the_global_token_never_goes_to_a_plain_http_twin_of_its_host() {
    // The repository asks for plain http:// and names no token of its own, so only
    // config.toml's [gitlab] token could apply.
    let (state, mock, dir, base) = setup_with(|base| format!("[repo.gitlab]\nhost = \"{base}\"\npath = \"mock/proj\"\n")).await;
    let authority = base.strip_prefix("http://").unwrap().to_string();
    let token_file = dir.path().join("token").display().to_string();
    state.config.write().secrets.insert("gl".into(), crate::config::SecretRef::File(token_file));
    let global = |host: String| Some(crate::config::global::GitlabConfig { host, token: "gl".into() });
    let refused = |state: &AppState| {
        let state = state.clone();
        async move {
            match client::ctx(&state, PID).await {
                Err(e) => e,
                Ok(_) => panic!("the global token was handed to the http:// twin of its https:// host"),
            }
        }
    };
    // The global token is for https://<authority> (a bare host means https).
    for host in [authority.clone(), format!("https://{authority}"), format!("HTTPS://{}/", authority.to_uppercase())] {
        state.config.write().gitlab = global(host.clone());
        let err = refused(&state).await;
        assert_eq!(err.code, "not_configured", "{host}: {}", err.message);
        assert!(err.message.contains(&format!("no GitLab token for {base}")), "{}", err.message);
        assert!(err.message.contains(&format!("is for {authority}")), "{}", err.message);
    }
    // A scheme the client does not speak is refused before any token lookup.
    state.config.write().gitlab = global(authority.clone());
    let mut p = (*state.projects.get(PID).unwrap()).clone();
    p.config.repo.as_mut().unwrap().gitlab.as_mut().unwrap().host = format!("ftp://{authority}");
    let err = match client::ctx_for(&state, Arc::new(p)).await {
        Err(e) => e,
        Ok(_) => panic!("ftp:// is not a GitLab origin"),
    };
    assert_eq!(err.code, "bad_request", "{}", err.message);
    assert!(mock.log.lock().is_empty(), "nothing reached the mock: {:?}", mock.log.lock());
    // The owner configuring that very http:// origin globally is their own choice.
    state.config.write().gitlab = global(base.clone());
    let ctx = client::ctx(&state, PID).await.unwrap();
    assert_eq!(ctx.id(), 42);
    let log = mock.log.lock();
    assert!(!log.is_empty() && log.iter().all(|r| r.token.as_deref() == Some(TOKEN)), "{log:?}");
}

#[tokio::test]
async fn pagination_follows_link_next_and_reports_totals() {
    let (state, _mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let page = pipelines::list_pipelines(&ctx, &pipelines::PipelinesQuery { per_page: Some(2), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.total, Some(3));
    assert_eq!(page.next_page, Some(2));
    // Enriched from the detail endpoint.
    assert_eq!(page.items[0].duration, Some(343.0));
    let diffs = mrs::mr_diffs(&ctx, 7).await.unwrap();
    assert_eq!(diffs.files.len(), 4, "both pages collected");
    assert!(!diffs.truncated);
    assert_eq!((diffs.files[0].additions, diffs.files[0].deletions), (2, 1));
    let (all, more) = ctx.get_all::<Value>(&ctx.purl("/pipelines"), &[("per_page", "1".into())], 2).await.unwrap();
    assert_eq!((all.len(), more), (2, true));
}

#[tokio::test]
async fn create_mr_defaults_to_current_branch_and_marks_drafts() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let mut events = state.events.subscribe();
    let body = mrs::CreateMr { title: "Add thing".into(), draft: true, squash: Some(true), ..Default::default() };
    let mr = mrs::create_mr(&ctx, &body).await.unwrap();
    assert_eq!(mr.iid, 8);
    let sent = mock.requests("POST", "/projects/42/merge_requests");
    let b = sent[0].body.clone().unwrap();
    assert_eq!(b["source_branch"], "feature");
    assert_eq!(b["target_branch"], "main");
    assert_eq!(b["title"], "Draft: Add thing");
    assert_eq!(b["squash"], true);
    let ev = events.recv().await.unwrap();
    assert_eq!(ev.kind, "gitlab.mr");
    assert_eq!(ev.project_id.as_deref(), Some(PID));
    // Same branch on both sides is refused before any request.
    let bad = mrs::CreateMr { title: "x".into(), target_branch: Some("feature".into()), ..Default::default() };
    assert_eq!(mrs::create_mr(&ctx, &bad).await.unwrap_err().code, "bad_request");
}

#[tokio::test]
async fn merge_is_guarded_by_the_reviewed_sha() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let stale = mrs::MergeBody { sha: "1111111111111111111111111111111111111111".into(), ..Default::default() };
    let err = mrs::merge(&ctx, 7, &stale).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("SHA does not match"));
    let ok = mrs::MergeBody { sha: HEAD_SHA.into(), squash: Some(true), remove_source_branch: Some(true), ..Default::default() };
    mrs::merge(&ctx, 7, &ok).await.unwrap();
    let sent = mock.requests("PUT", "/merge_requests/7/merge");
    let b = sent.last().unwrap().body.clone().unwrap();
    assert_eq!(b["should_remove_source_branch"], true);
    assert_eq!(b["squash"], true);
}

#[tokio::test]
async fn diff_line_comment_position_comes_from_diff_refs() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let req = mrs::NewDiscussion {
        body: "Why?".into(),
        position: Some(mrs::LinePosition { new_path: "src/a.rs".into(), new_line: Some(4), old_line: Some(3), ..Default::default() }),
    };
    let d = mrs::add_discussion(&ctx, 7, &req).await.unwrap();
    assert_eq!(d.id, "0123abcd");
    let b = mock.requests("POST", "/merge_requests/7/discussions")[0].body.clone().unwrap();
    assert_eq!(b["position"]["head_sha"], HEAD_SHA);
    assert_eq!(b["position"]["start_sha"], "bbbbbbbb22222222bbbbbbbb22222222bbbbbbbb");
    assert_eq!(b["position"]["new_line"], 4);
    assert_eq!(b["position"]["old_line"], 3);
    assert_eq!(b["position"]["position_type"], "text");
}

#[tokio::test]
async fn trace_is_read_incrementally_by_offset() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let p = |flag: &str, s: &str| format!("2026-09-26T10:00:00.000001Z 00O{flag}{s}\n");
    *mock.trace.lock() = p(" ", "\x1b[32mstep one\x1b[0m") + "2026-09-26T10:00:01.000001Z 00O partial";
    let first = pipelines::read_trace(&ctx, 1, 0).await.unwrap();
    assert!(first.reset);
    assert!(!first.complete);
    assert_eq!(first.text, "\x1b[32mstep one\x1b[0m", "ANSI kept, prefix stripped, partial line held back");
    let consumed = p(" ", "\x1b[32mstep one\x1b[0m").len() as u64;
    assert_eq!(first.offset, consumed);

    // The partial line completes, a continuation follows, and the job finishes.
    let full = p(" ", "\x1b[32mstep one\x1b[0m") + &p(" ", "partial line") + &p("+", " continued") + &p(" ", "done");
    *mock.trace.lock() = full.clone();
    *mock.job_status.lock() = "success".into();
    let second = pipelines::read_trace(&ctx, 1, first.offset).await.unwrap();
    assert!(!second.reset);
    assert!(second.complete);
    assert_eq!(first.text.clone() + &second.text, super::trace::strip_prefixes(&full, true));
    assert_eq!(second.offset, full.len() as u64);

    // Nothing new: 416 → empty chunk at the same offset.
    let third = pipelines::read_trace(&ctx, 1, second.offset).await.unwrap();
    assert_eq!((third.text.as_str(), third.offset, third.complete), ("", second.offset, true));

    // Plain tail for agents and the "ask agent" prompt.
    let (_, tail, total, _) = pipelines::log_tail(&ctx, 1, 2, true).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(tail, "partial line continued\ndone");
}

#[tokio::test]
async fn trace_offsets_work_without_range_support_and_restart_when_the_log_shrinks() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let line = |s: &str| format!("2026-09-26T10:00:00Z 00O {s}\n");
    *mock.trace.lock() = line("one") + &line("two");
    let first = pipelines::read_trace(&ctx, 1, 0).await.unwrap();
    assert_eq!(first.text, "one\ntwo");
    // A server that ignores Range sends everything again: only the new part is returned.
    mock.ignore_range.store(true, Ordering::SeqCst);
    *mock.trace.lock() = line("one") + &line("two") + &line("three");
    let second = pipelines::read_trace(&ctx, 1, first.offset).await.unwrap();
    assert!(!second.reset);
    assert_eq!(second.text, "\nthree");
    // The job was retried under the same id and its log restarted shorter.
    mock.ignore_range.store(false, Ordering::SeqCst);
    *mock.trace.lock() = line("fresh");
    let third = pipelines::read_trace(&ctx, 1, second.offset).await.unwrap();
    assert!(third.reset, "416 with a smaller total restarts from the top");
    assert_eq!(third.text, "fresh");
}

#[tokio::test]
async fn rate_limits_are_waited_out() {
    let (state, mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let started = std::time::Instant::now();
    let page = mrs::list_mrs(&ctx, &mrs::MrsQuery::default()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(started.elapsed() >= Duration::from_millis(900), "Retry-After honoured");
    assert_eq!(mock.rate_limit_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn upstream_bodies_are_never_echoed() {
    let (state, _mock, _dir) = setup().await;
    let ctx = client::ctx(&state, PID).await.unwrap();
    let err = pipelines::get_pipeline(&ctx, 999).await.unwrap_err();
    assert_eq!(err.code, "upstream");
    assert!(!err.message.contains(TOKEN));
    assert!(!err.message.contains("stack trace"));
    let err = mrs::list_mrs(&ctx, &mrs::MrsQuery { state: Some("bogus".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!(err.code, "bad_request");
}

#[tokio::test]
async fn commit_ci_status_contract() {
    let (state, _mock, _dir) = setup().await;
    let project = state.projects.get(PID).unwrap();
    let st = super::commit_ci_status(&state, &project, &HEAD_SHA[..8]).await.unwrap().unwrap();
    assert_eq!(st.status, "success");
    assert_eq!(st.pipeline_id, Some(303));
    assert_eq!(st.sha.as_deref(), Some(HEAD_SHA));
    assert!(super::commit_ci_status(&state, &project, "deadbeef").await.unwrap().is_none());
    assert_eq!(super::commit_ci_status(&state, &project, "not-a-sha").await.unwrap_err().code, "bad_request");
    // A project without GitLab is simply "no status".
    let mut other = (*project).clone();
    other.config.repo = None;
    other.remote = None;
    assert!(super::commit_ci_status(&state, &other, HEAD_SHA).await.unwrap().is_none());
}

#[tokio::test]
async fn routes_serialize_camel_case_and_map_errors() {
    let (state, _mock, _dir) = setup().await;
    let ctx = McpCtx::default();
    let v = crate::mcp::call_api(&state, Method::GET, "/api/projects/repo/gitlab/pipelines?perPage=2", None, &ctx)
        .await
        .unwrap();
    assert_eq!(v["items"][0]["webUrl"].as_str().map(|u| u.ends_with("/pipelines/303")), Some(true));
    assert_eq!(v["nextPage"], 2);
    let v = crate::mcp::call_api(&state, Method::GET, "/api/projects/repo/gitlab/mrs/7", None, &ctx).await.unwrap();
    assert_eq!(v["diffRefs"]["headSha"], HEAD_SHA);
    assert_eq!(v["approvals"]["approvalsLeft"], 1);
    let err = crate::mcp::call_api(&state, Method::GET, "/api/projects/nope/gitlab/summary", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::NOT_FOUND);
    let body = json!({ "sha": "zzz" });
    let err = crate::mcp::call_api(&state, Method::POST, "/api/projects/repo/gitlab/mrs/7/merge", Some(body), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mcp_tools_use_the_session_project() {
    let (state, mock, _dir) = setup().await;
    *mock.trace.lock() = "2026-09-26T10:00:00Z 00O \x1b[31merror: boom\x1b[0m\n".into();
    *mock.job_status.lock() = "failed".into();
    let tools = super::mcp_tools();
    let find = |n: &str| tools.iter().find(|t| t.name == n).unwrap().handler.clone();
    let mctx = McpCtx { terminal_id: None, project_id: Some(PID.into()) };
    let out = find("gitlab_job_log")(state.clone(), mctx.clone(), json!({ "jobId": 5, "tailLines": 50 })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.contains("error: boom"), "{text}");
    assert!(!text.contains('\x1b'));
    assert!(text.contains("script_failure"));
    // Without a session project the caller must say which project.
    let err = find("gitlab_mrs")(state.clone(), McpCtx::default(), json!({})).await.unwrap_err();
    assert_eq!(err.code, "bad_request");
    let out = find("gitlab_retry_job")(state.clone(), mctx, json!({ "jobId": "5" })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.contains("new job 9001"), "{text}");
}

#[tokio::test]
async fn mcp_sessions_cannot_reach_other_projects() {
    let (state, mock, _dir) = setup().await;
    *mock.job_status.lock() = "failed".into();
    let tools = super::mcp_tools();
    let find = |n: &str| tools.iter().find(|t| t.name == n).unwrap().handler.clone();
    // An agent session of another project (a prompt-injected one, say) names this project.
    let other = McpCtx { terminal_id: Some("t-other".into()), project_id: Some("elsewhere".into()) };
    for (name, args) in [
        ("gitlab_retry_job", json!({ "projectId": PID, "jobId": 5 })),
        ("gitlab_mr_comment", json!({ "projectId": PID, "iid": 7, "body": "hi" })),
        ("gitlab_create_mr", json!({ "projectId": PID, "title": "x", "sourceBranch": "feature" })),
        ("gitlab_job_log", json!({ "projectId": PID, "jobId": 5 })),
        ("gitlab_pipelines", json!({ "projectId": PID })),
    ] {
        let err = find(name)(state.clone(), other.clone(), args).await.unwrap_err();
        assert_eq!(err.code, "forbidden", "{name}: {}", err.message);
        assert!(err.message.contains("elsewhere"), "{}", err.message);
    }
    assert!(mock.log.lock().is_empty(), "nothing reached GitLab: {:?}", mock.log.lock());
    // A session without a project cannot pick one either.
    let unbound = McpCtx { terminal_id: Some("t-none".into()), project_id: None };
    let err = find("gitlab_retry_job")(state.clone(), unbound, json!({ "projectId": PID, "jobId": 5 })).await.unwrap_err();
    assert_eq!(err.code, "forbidden");
    // Naming its own project is fine.
    let own = McpCtx { terminal_id: Some("t1".into()), project_id: Some(PID.into()) };
    let out = find("gitlab_retry_job")(state.clone(), own, json!({ "projectId": PID, "jobId": 5 })).await.unwrap();
    let ToolOutput::Text(text) = out else { panic!("text output") };
    assert!(text.contains("new job 9001"), "{text}");
    // The master token without a session (scripts, the owner) may name any project.
    let master = McpCtx { terminal_id: None, project_id: Some("elsewhere".into()) };
    find("gitlab_pipelines")(state.clone(), master, json!({ "projectId": PID })).await.unwrap();
}

#[tokio::test]
async fn poller_emits_on_pipeline_changes_only() {
    let (state, mock, _dir) = setup().await;
    let project = state.projects.get(PID).unwrap();
    let mut events = state.events.subscribe();
    // First sight of each ref: recorded, not announced. `main` pipeline is running.
    let active = super::poller::poll_project(&state, project.clone()).await.unwrap();
    assert!(active);
    *mock.pipeline_status.lock() = "failed".into();
    let active = super::poller::poll_project(&state, project.clone()).await.unwrap();
    assert!(!active);
    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert_eq!(ev.kind, "gitlab.pipeline");
    assert_eq!(ev.data["pipelineId"], 303);
    assert_eq!(ev.data["status"], "failed");
    assert_eq!(ev.data["previousStatus"], "running");
    // Unchanged: nothing more.
    super::poller::poll_project(&state, project).await.unwrap();
    assert!(events.try_recv().is_err());
}

/// The pipeline's failed tests with their output, for the panel and for agents; a
/// pipeline without a report is a 404 for the panel and a plain answer for agents.
#[tokio::test]
async fn test_failures_reach_the_panel_and_agents() {
    let (state, _mock, _dir) = setup().await;
    let mctx = McpCtx::default();
    let v = crate::mcp::call_api(&state, Method::GET, &format!("/api/projects/{PID}/gitlab/pipelines/303/tests"), None, &mctx).await.unwrap();
    assert_eq!(v["total"]["failed"], 1);
    let cases = v["suites"][0]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2, "successes are not listed");
    assert_eq!(cases[0]["name"], "adds");
    assert_eq!(cases[0]["file"], "src/calc.rs");
    assert!(cases[0]["output"].as_str().unwrap().contains("left: 4"));
    let e = crate::mcp::call_api(&state, Method::GET, &format!("/api/projects/{PID}/gitlab/pipelines/304/tests"), None, &mctx).await.unwrap_err();
    assert_eq!(e.status, StatusCode::NOT_FOUND);

    let agent = McpCtx { terminal_id: None, project_id: Some(PID.into()) };
    let tool = super::mcp_tools().into_iter().find(|t| t.name == "gitlab_test_failures").unwrap();
    let ToolOutput::Text(out) = (tool.handler)(state.clone(), agent.clone(), json!({ "pipelineId": 303 })).await.unwrap() else { panic!("text expected") };
    assert!(out.contains("FAILED calc › adds") && out.contains("File: src/calc.rs") && out.contains("ERROR db › setup"), "{out}");
    let ToolOutput::Text(out) = (tool.handler)(state, agent, json!({ "pipelineId": 304 })).await.unwrap() else { panic!("text expected") };
    assert!(out.contains("no test report"), "{out}");
}

//! GitHub Actions and commit checks: workflow runs (list, detail with jobs and
//! steps), job logs and annotations, re-run / re-run failed / cancel, workflow
//! dispatch with its inputs, the combined check state of a commit, and the artifacts
//! a run uploaded (listed, and downloaded through Workbench with the token).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{Fresh, GhCtx, ctx, read_tail};
use super::logs::{self, JobLogData, SharedLog};
use super::model::{
    Annotation, Artifact, CheckItem, CheckRun, CombinedStatus, CommitChecks, Job, ListPage, Run, Workflow, aggregate, actions_ids,
    run_state, status_state,
};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Bytes of log kept per job (the tail wins when a log is larger).
const LOG_KEEP: usize = 16 * 1024 * 1024;
/// Finished logs kept in memory.
const LOG_CACHE_ENTRIES: usize = 8;
const LOG_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// `status=` values GitHub accepts on the runs list.
const RUN_STATUSES: &[&str] = &[
    "completed", "action_required", "cancelled", "failure", "neutral", "skipped", "stale", "success", "timed_out",
    "in_progress", "queued", "requested", "waiting", "pending",
];

pub fn is_hex_sha(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_sha(s: &str) -> ApiResult<&str> {
    let s = s.trim();
    if is_hex_sha(s) { Ok(s) } else { Err(ApiError::bad_request(format!("not a commit sha: {s:?}"))) }
}

/// A branch name we put into a URL path or query.
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

// ---------------------------------------------------------------- commit checks

/// The full sha for `sha` (full or short): the local clone first, else GitHub.
pub async fn full_sha(ctx: &GhCtx, sha: &str) -> ApiResult<Option<String>> {
    let sha = valid_sha(sha)?;
    if sha.len() >= 40 {
        return Ok(Some(sha.to_ascii_lowercase()));
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["rev-parse", "--verify", "--quiet", &format!("{sha}^{{commit}}")])
        .current_dir(&ctx.project.root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Ok(out) = crate::util::proc::run_cmd(cmd, Duration::from_secs(5)).await {
        let full = out.stdout.trim();
        if out.ok() && full.len() >= 40 && is_hex_sha(full) {
            return Ok(Some(full.to_string()));
        }
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct C {
        sha: String,
    }
    let c: Option<C> = match ctx.get_opt(&ctx.rurl(&format!("/commits/{sha}")), &[], Fresh::Fixed).await {
        Ok(c) => c,
        // An ambiguous or unknown short sha is a 422.
        Err(e) if e.status == StatusCode::BAD_REQUEST => None,
        Err(e) => return Err(e),
    };
    Ok(c.map(|c| c.sha).filter(|s| !s.is_empty()))
}

/// Everything that reported on a commit: check runs (latest per name), legacy
/// statuses, and workflow runs (newest per workflow and event), combined.
pub async fn commit_checks(ctx: &GhCtx, sha: &str) -> ApiResult<CommitChecks> {
    let runs_q = [("head_sha", sha.to_string()), ("per_page", "100".to_string())];
    let checks_q = [("filter", "latest".to_string())];
    let (runs_url, checks_url, status_url) =
        (ctx.rurl("/actions/runs"), ctx.rurl(&format!("/commits/{sha}/check-runs")), ctx.rurl(&format!("/commits/{sha}/status")));
    let (runs, checks, statuses) = tokio::join!(
        ctx.get_opt::<Value>(&runs_url, &runs_q, Fresh::Live),
        ctx.get_all::<CheckRun>(&checks_url, &checks_q, 300, Some("check_runs"), Fresh::Live),
        ctx.get_opt::<CombinedStatus>(&status_url, &[], Fresh::Live),
    );
    // Actions may be disabled (404) or forbidden; checks and statuses still count.
    let mut runs: Vec<Run> = match runs {
        Ok(Some(mut v)) => serde_json::from_value(v["workflow_runs"].take()).unwrap_or_default(),
        Ok(None) => vec![],
        Err(e) if e.code == "rate_limited" => return Err(e),
        Err(_) => vec![],
    };
    let checks = match checks {
        Ok((c, _)) => c,
        Err(e) if e.status == StatusCode::NOT_FOUND || e.status == StatusCode::FORBIDDEN => vec![],
        Err(e) => return Err(e),
    };
    let statuses = statuses?.unwrap_or_default();
    // A workflow re-triggered for the same commit: only its newest run counts.
    runs.sort_by(|a, b| b.id.cmp(&a.id));
    let mut seen = std::collections::HashSet::new();
    runs.retain(|r| seen.insert((r.workflow_id, r.event.clone())));
    for r in &mut runs {
        r.fill();
    }
    let mut items: Vec<CheckItem> = checks
        .into_iter()
        .map(|c| {
            let url = c.details_url.clone().filter(|u| !u.is_empty()).or(c.html_url.clone());
            let (run_id, job_id) = url.as_deref().map(actions_ids).unwrap_or((None, None));
            let is_actions = c.app.as_ref().is_some_and(|a| a.slug == "github-actions");
            CheckItem {
                state: run_state(&c.status, c.conclusion.as_deref()).to_string(),
                kind: "check".into(),
                name: c.name,
                status: c.status,
                conclusion: c.conclusion,
                url,
                app: c.app.map(|a| a.name),
                description: None,
                run_id,
                job_id: if is_actions { job_id.or(Some(c.id)) } else { job_id },
                workflow: None,
                event: None,
                started_at: c.started_at,
                completed_at: c.completed_at,
            }
        })
        .collect();
    // Runs with no check runs yet (queued, waiting for approval) still count.
    for r in &runs {
        if !items.iter().any(|i| i.run_id == Some(r.id)) {
            items.push(CheckItem {
                name: r.name.clone().unwrap_or_else(|| format!("run {}", r.run_number)),
                kind: "run".into(),
                state: r.state.clone(),
                status: r.status.clone(),
                conclusion: r.conclusion.clone(),
                url: Some(r.html_url.clone()),
                app: Some("GitHub Actions".into()),
                description: None,
                run_id: Some(r.id),
                job_id: None,
                workflow: r.name.clone(),
                event: Some(r.event.clone()),
                started_at: r.run_started_at.clone(),
                completed_at: None,
            });
        }
    }
    for s in statuses.statuses {
        items.push(CheckItem {
            state: status_state(&s.state).to_string(),
            kind: "status".into(),
            name: s.context,
            status: s.state,
            conclusion: None,
            url: s.target_url,
            app: None,
            description: s.description,
            run_id: None,
            job_id: None,
            workflow: None,
            event: None,
            started_at: s.created_at,
            completed_at: s.updated_at,
        });
    }
    for i in &mut items {
        if let Some(r) = i.run_id.and_then(|id| runs.iter().find(|r| r.id == id)) {
            i.workflow = r.name.clone();
            i.event = Some(r.event.clone());
        }
    }
    let state = aggregate(items.iter().map(|i| i.state.as_str()).chain(runs.iter().map(|r| r.state.as_str())));
    Ok(CommitChecks { sha: sha.to_string(), state: state.map(str::to_string), items, runs })
}

/// The run that best represents a commit's state: a failed one, else one that
/// is still going, else the newest.
pub fn main_run<'a>(checks: &'a CommitChecks) -> Option<&'a Run> {
    let pick = |s: &str| checks.runs.iter().find(|r| r.state == s);
    let want = checks.state.as_deref().unwrap_or("");
    pick(want).or_else(|| pick("failed")).or_else(|| pick("running")).or_else(|| checks.runs.first())
}

/// GitHub answers the checks of a commit it does not know (not pushed yet)
/// with 422 "No commit found for SHA" (a 404 on some endpoints).
fn unknown_commit(e: &ApiError) -> bool {
    matches!(e.status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND)
}

/// `commit_checks`, with `None` for a commit GitHub does not know.
pub async fn known_commit_checks(ctx: &GhCtx, sha: &str) -> ApiResult<Option<CommitChecks>> {
    match commit_checks(ctx, sha).await {
        Ok(c) => Ok(Some(c)),
        Err(e) if unknown_commit(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

pub async fn ci_status_for(ctx: &GhCtx, sha: &str) -> ApiResult<Option<super::CiStatus>> {
    let Some(sha) = full_sha(ctx, sha).await? else { return Ok(None) };
    let Some(checks) = known_commit_checks(ctx, &sha).await? else { return Ok(None) };
    let Some(state) = checks.state.clone() else { return Ok(None) };
    let run = main_run(&checks);
    let web_url = run.map(|r| r.html_url.clone()).filter(|u| !u.is_empty()).or_else(|| {
        checks.items.iter().find(|i| i.state == state).and_then(|i| i.url.clone())
    });
    Ok(Some(super::CiStatus {
        status: state,
        pipeline_id: run.map(|r| r.id),
        web_url,
        sha: Some(sha),
        git_ref: run.and_then(|r| r.head_branch.clone()),
    }))
}

// ---------------------------------------------------------------- runs

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunsQuery {
    pub branch: Option<String>,
    pub status: Option<String>,
    pub event: Option<String>,
    pub workflow_id: Option<u64>,
    pub head_sha: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn list_runs(ctx: &GhCtx, q: &RunsQuery) -> ApiResult<ListPage<Run>> {
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let per_page = q.per_page.unwrap_or(25).clamp(1, 100);
    let mut query: Vec<(&str, String)> = vec![("page", page.to_string()), ("per_page", per_page.to_string())];
    if let Some(b) = q.branch.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        query.push(("branch", valid_branch(b)?.to_string()));
    }
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty()) {
        if !RUN_STATUSES.contains(&s) {
            return Err(ApiError::bad_request(format!("unknown run status {s:?}")));
        }
        query.push(("status", s.to_string()));
    }
    if let Some(e) = q.event.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        if !e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || e.len() > 64 {
            return Err(ApiError::bad_request(format!("unknown event {e:?}")));
        }
        query.push(("event", e.to_string()));
    }
    if let Some(s) = q.head_sha.as_deref().filter(|s| !s.is_empty()) {
        query.push(("head_sha", valid_sha(s)?.to_string()));
    }
    let url = match q.workflow_id {
        Some(w) => ctx.rurl(&format!("/actions/workflows/{w}/runs")),
        None => ctx.rurl("/actions/runs"),
    };
    let res = ctx.get_page::<Value>(&url, &query, Fresh::Live).await?;
    let mut body = res.body;
    let total = body.get("total_count").and_then(Value::as_u64);
    let mut items: Vec<Run> = serde_json::from_value(body["workflow_runs"].take())
        .map_err(|e| ApiError::upstream(format!("unexpected runs list from GitHub: {e}")))?;
    for r in &mut items {
        r.fill();
    }
    Ok(ListPage { items, page, next_page: res.next_url.map(|_| page + 1), total })
}

/// The newest runs of a branch (all workflows).
pub async fn branch_runs(ctx: &GhCtx, branch: &str, n: u32) -> ApiResult<Vec<Run>> {
    let q = RunsQuery { branch: Some(branch.to_string()), per_page: Some(n), ..Default::default() };
    Ok(list_runs(ctx, &q).await?.items)
}

pub async fn get_run(ctx: &GhCtx, id: u64) -> ApiResult<Run> {
    let mut r: Run = ctx.get(&ctx.rurl(&format!("/actions/runs/{id}")), &[], Fresh::Live).await?;
    r.fill();
    Ok(r)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<Job>,
    /// More jobs exist than we fetched.
    pub truncated: bool,
}

pub async fn run_detail(ctx: &GhCtx, id: u64) -> ApiResult<RunDetail> {
    let jobs_url = ctx.rurl(&format!("/actions/runs/{id}/jobs"));
    let q = [("filter", "latest".to_string())];
    let (run, jobs) = tokio::join!(get_run(ctx, id), ctx.get_all::<Job>(&jobs_url, &q, 1000, Some("jobs"), Fresh::Live));
    let run = run?;
    let (mut jobs, truncated) = jobs?;
    for j in &mut jobs {
        j.fill();
    }
    Ok(RunDetail { run, jobs, truncated })
}

pub async fn get_job(ctx: &GhCtx, id: u64) -> ApiResult<Job> {
    let mut j: Job = ctx.get(&ctx.rurl(&format!("/actions/jobs/{id}")), &[], Fresh::Live).await?;
    j.fill();
    Ok(j)
}

/// `rerun`, `rerun-failed-jobs` or `cancel` a run.
pub async fn run_action(ctx: &GhCtx, id: u64, action: &str) -> ApiResult<Run> {
    if !matches!(action, "rerun" | "rerun-failed-jobs" | "cancel") {
        return Err(ApiError::bad_request(format!("unknown run action {action:?}")));
    }
    let _: Value = ctx.write(Method::POST, &ctx.rurl(&format!("/actions/runs/{id}/{action}")), Some(&json!({}))).await?;
    let mut run = get_run(ctx, id).await?;
    // GitHub takes a moment to requeue a run; say what is about to happen.
    if action != "cancel" && run.status == "completed" {
        run.status = "queued".into();
        run.conclusion = None;
        run.fill();
    }
    super::run_changed(ctx, &run, if action == "cancel" { "cancel" } else { "rerun" });
    Ok(run)
}

/// Re-run one job (and, as GitHub does, the jobs that depend on it).
pub async fn rerun_job(ctx: &GhCtx, id: u64) -> ApiResult<Job> {
    let _: Value = ctx.write(Method::POST, &ctx.rurl(&format!("/actions/jobs/{id}/rerun")), Some(&json!({}))).await?;
    let job = get_job(ctx, id).await?;
    ctx.state.events.emit(
        "github.job",
        Some(&ctx.project.id),
        json!({ "jobId": id, "runId": job.run_id, "action": "rerun" }),
    );
    if let Ok(mut run) = get_run(ctx, job.run_id).await {
        if run.status == "completed" {
            run.status = "queued".into();
            run.conclusion = None;
            run.fill();
        }
        super::run_changed(ctx, &run, "rerun");
    }
    Ok(job)
}

// ---------------------------------------------------------------- workflows

pub async fn workflows(ctx: &GhCtx) -> ApiResult<Vec<Workflow>> {
    Ok(ctx.get_all::<Workflow>(&ctx.rurl("/actions/workflows"), &[], 500, Some("workflows"), Fresh::Slow).await?.0)
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchInput {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub default: Option<String>,
    /// `string` | `choice` | `boolean` | `number` | `environment`
    #[serde(rename = "type")]
    pub input_type: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchInfo {
    /// The workflow has an `on: workflow_dispatch` trigger.
    pub dispatchable: bool,
    pub inputs: Vec<DispatchInput>,
}

fn yaml_str(v: &serde_norway::Value) -> Option<String> {
    match v {
        serde_norway::Value::String(s) => Some(s.clone()),
        serde_norway::Value::Bool(b) => Some(b.to_string()),
        serde_norway::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `on.workflow_dispatch` of a workflow file (untrusted YAML: only read).
pub fn dispatch_info(yaml: &str) -> DispatchInfo {
    use serde_norway::Value as Y;
    let Ok(doc) = serde_norway::from_str::<Y>(yaml) else { return DispatchInfo::default() };
    // `on` is a string key in YAML 1.2; YAML 1.1 readers see `true`.
    let on = doc.get("on").or_else(|| doc.as_mapping().and_then(|m| m.get(Y::Bool(true))));
    let Some(on) = on else { return DispatchInfo::default() };
    match on {
        Y::String(s) => DispatchInfo { dispatchable: s == "workflow_dispatch", inputs: vec![] },
        Y::Sequence(seq) => DispatchInfo {
            dispatchable: seq.iter().any(|v| v.as_str() == Some("workflow_dispatch")),
            inputs: vec![],
        },
        Y::Mapping(m) => {
            let Some(wd) = m.get(Y::String("workflow_dispatch".into())) else { return DispatchInfo::default() };
            let inputs = wd
                .get("inputs")
                .and_then(Y::as_mapping)
                .map(|inputs| {
                    inputs
                        .iter()
                        .take(25)
                        .filter_map(|(k, v)| {
                            let name = yaml_str(k)?;
                            Some(DispatchInput {
                                name,
                                description: v.get("description").and_then(yaml_str),
                                required: v.get("required").and_then(Y::as_bool).unwrap_or(false),
                                default: v.get("default").and_then(yaml_str),
                                input_type: v.get("type").and_then(yaml_str).unwrap_or_else(|| "string".into()),
                                options: v
                                    .get("options")
                                    .and_then(Y::as_sequence)
                                    .map(|o| o.iter().filter_map(yaml_str).collect())
                                    .unwrap_or_default(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            DispatchInfo { dispatchable: true, inputs }
        }
        _ => DispatchInfo::default(),
    }
}

/// Whether a workflow can be dispatched, and its inputs, from its file at `git_ref`.
pub async fn workflow_inputs(ctx: &GhCtx, id: u64, git_ref: Option<&str>) -> ApiResult<DispatchInfo> {
    let wf: Workflow = ctx.get(&ctx.rurl(&format!("/actions/workflows/{id}")), &[], Fresh::Slow).await?;
    let path = super::pulls::valid_repo_file(&wf.path)?;
    let r = match git_ref.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => valid_branch(r)?.to_string(),
        None => ctx.default_branch().unwrap_or("HEAD").to_string(),
    };
    let text = super::pulls::file_at(ctx, path, &r, 1024 * 1024).await?;
    match text {
        super::pulls::FileText::Text(t) => Ok(dispatch_info(&t)),
        _ => Ok(DispatchInfo::default()),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DispatchBody {
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub inputs: serde_json::Map<String, Value>,
}

pub fn dispatch_payload(b: &DispatchBody) -> ApiResult<Value> {
    let r = valid_branch(&b.git_ref)?;
    if b.inputs.len() > 25 {
        return Err(ApiError::bad_request("at most 25 inputs"));
    }
    let mut inputs = serde_json::Map::new();
    for (k, v) in &b.inputs {
        if k.is_empty() || k.len() > 100 || !k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
            return Err(ApiError::bad_request(format!("invalid input name {k:?}")));
        }
        // GitHub takes every input as a string.
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Bool(x) => x.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Null => continue,
            _ => return Err(ApiError::bad_request(format!("input {k} must be a string, number or boolean"))),
        };
        inputs.insert(k.clone(), Value::String(s));
    }
    Ok(json!({ "ref": r, "inputs": inputs }))
}

pub async fn dispatch(ctx: &GhCtx, id: u64, b: &DispatchBody) -> ApiResult<Value> {
    let payload = dispatch_payload(b)?;
    let _: Value = ctx.write(Method::POST, &ctx.rurl(&format!("/actions/workflows/{id}/dispatches")), Some(&payload)).await?;
    ctx.state.github.poll.mark_hot(&ctx.project.id);
    ctx.state.events.emit(
        "github.run",
        Some(&ctx.project.id),
        json!({ "runId": null, "workflowId": id, "action": "dispatch", "branch": payload["ref"] }),
    );
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------- logs

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobLog {
    #[serde(flatten)]
    pub data: Option<SharedLog>,
    /// The log could be read.
    pub available: bool,
    /// Why not: `running` (published when the job finishes), `needs_token`
    /// (GitHub serves logs only to signed-in users), `gone` (expired or deleted).
    pub reason: Option<String>,
    pub message: Option<String>,
    pub job: Job,
}

fn cached_log(state: &AppState, key: &str) -> Option<Arc<JobLogData>> {
    state.github.logs.lock().iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn cache_log(state: &AppState, key: String, data: Arc<JobLogData>) {
    let mut logs = state.github.logs.lock();
    logs.retain(|(k, _)| *k != key);
    logs.push((key, data));
    while logs.len() > LOG_CACHE_ENTRIES || logs.iter().map(|(_, d)| d.text.len()).sum::<usize>() > LOG_CACHE_BYTES {
        if logs.is_empty() {
            break;
        }
        logs.remove(0);
    }
}

/// A job's log. GitHub publishes it once the job has finished; before that
/// (and for anonymous callers) the answer says why there is none.
pub async fn job_log(ctx: &GhCtx, id: u64) -> ApiResult<JobLog> {
    let job = get_job(ctx, id).await?;
    let key = format!("{}|{id}", ctx.rurl(""));
    let finished = job.status == "completed";
    if finished {
        if let Some(data) = cached_log(&ctx.state, &key) {
            return Ok(JobLog { data: Some(SharedLog(data)), available: true, reason: None, message: None, job });
        }
    }
    let unavailable = |reason: &str, message: String, job: Job| JobLog {
        data: None,
        available: false,
        reason: Some(reason.into()),
        message: Some(message),
        job,
    };
    if ctx.is_anonymous() {
        return Ok(unavailable(
            "needs_token",
            "GitHub shows job logs only to signed-in users. Add a GitHub token (read-only access is enough) to see them here."
                .into(),
            job,
        ));
    }
    let url = ctx.rurl(&format!("/actions/jobs/{id}/logs"));
    let resp = ctx.send_raw(Method::GET, &url, |rb| rb.timeout(Duration::from_secs(300))).await?;
    let status = resp.status();
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        // A log appears a little after its job completes.
        let just_finished = job.completed_at.as_deref().and_then(|t| super::model::seconds_between(Some(t), None)).is_some_and(|s| s < 180.0);
        return Ok(if !finished {
            unavailable("running", "GitHub publishes the log when the job finishes.".into(), job)
        } else if just_finished {
            unavailable("running", "GitHub is still publishing this job's log.".into(), job)
        } else {
            unavailable("gone", "GitHub no longer has this job's log (logs expire, or it was deleted).".into(), job)
        });
    }
    if !status.is_success() {
        return Err(ctx.error_from(resp).await);
    }
    let (bytes, size, dropped) = read_tail(resp, LOG_KEEP).await?;
    let mut raw = String::from_utf8_lossy(&bytes).into_owned();
    if dropped {
        // Start on a whole line.
        if let Some(i) = raw.find('\n') {
            raw.drain(..=i);
        }
    }
    let data = Arc::new(logs::parse(&raw, &job.steps, dropped, size));
    if finished {
        cache_log(&ctx.state, key, data.clone());
    }
    Ok(JobLog { data: Some(SharedLog(data)), available: true, reason: None, message: None, job })
}

/// The last `n` lines of a job's log (plain text when `plain`), with the job.
/// `None` for the text when GitHub has no log for us.
pub async fn log_tail(ctx: &GhCtx, id: u64, n: usize, plain: bool) -> ApiResult<(Job, Option<(String, usize, bool)>, Option<String>)> {
    let log = job_log(ctx, id).await?;
    let Some(SharedLog(data)) = log.data else { return Ok((log.job, None, log.message)) };
    let text = if plain { logs::plain(&data.text) } else { data.text.clone() };
    let (tail, total) = logs::tail_lines(&text, n);
    Ok((log.job, Some((tail, total, data.truncated)), None))
}

pub async fn annotations(ctx: &GhCtx, id: u64) -> ApiResult<Vec<Annotation>> {
    let url = ctx.rurl(&format!("/check-runs/{id}/annotations"));
    match ctx.get_all::<Annotation>(&url, &[], 200, None, Fresh::Live).await {
        Ok((a, _)) => Ok(a),
        Err(e) if e.status == StatusCode::NOT_FOUND => Ok(vec![]),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------- handlers

type Id = Path<(String, u64)>;

async fn h_runs(State(s): State<AppState>, Path(pid): Path<String>, Query(q): Query<RunsQuery>) -> ApiResult<Json<ListPage<Run>>> {
    Ok(Json(list_runs(&ctx(&s, &pid).await?, &q).await?))
}
async fn h_run(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<RunDetail>> {
    Ok(Json(run_detail(&ctx(&s, &pid).await?, id).await?))
}
async fn h_rerun(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Run>> {
    Ok(Json(run_action(&ctx(&s, &pid).await?, id, "rerun").await?))
}
async fn h_rerun_failed(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Run>> {
    Ok(Json(run_action(&ctx(&s, &pid).await?, id, "rerun-failed-jobs").await?))
}
async fn h_cancel(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Run>> {
    Ok(Json(run_action(&ctx(&s, &pid).await?, id, "cancel").await?))
}
async fn h_job(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Job>> {
    Ok(Json(get_job(&ctx(&s, &pid).await?, id).await?))
}
async fn h_job_rerun(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Job>> {
    Ok(Json(rerun_job(&ctx(&s, &pid).await?, id).await?))
}
async fn h_annotations(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Vec<Annotation>>> {
    Ok(Json(annotations(&ctx(&s, &pid).await?, id).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LogQuery {
    /// Only the last N lines (≤ 5000) instead of the whole log.
    tail: Option<usize>,
    /// With `tail`: ANSI and markers removed.
    plain: Option<bool>,
}

async fn h_logs(State(s): State<AppState>, Path((pid, id)): Id, Query(q): Query<LogQuery>) -> ApiResult<Json<Value>> {
    let ctx = ctx(&s, &pid).await?;
    if let Some(n) = q.tail {
        let (job, tail, message) = log_tail(&ctx, id, n.clamp(1, 5000), q.plain.unwrap_or(false)).await?;
        let (text, total, truncated) = tail.unwrap_or_default();
        return Ok(Json(json!({
            "text": text, "totalLines": total, "truncated": truncated, "available": message.is_none(),
            "message": message, "state": job.state, "complete": job.status == "completed",
        })));
    }
    Ok(Json(serde_json::to_value(job_log(&ctx, id).await?)?))
}

/// The artifacts of a run.
pub async fn run_artifacts(ctx: &GhCtx, id: u64) -> ApiResult<Vec<Artifact>> {
    let url = ctx.rurl(&format!("/actions/runs/{id}/artifacts"));
    Ok(ctx.get_all::<Artifact>(&url, &[], 500, Some("artifacts"), Fresh::Live).await?.0)
}

async fn h_artifacts(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Json<Vec<Artifact>>> {
    Ok(Json(run_artifacts(&ctx(&s, &pid).await?, id).await?))
}

/// Stream an artifact's zip through (never buffered whole). GitHub answers with a
/// redirect to its blob storage, followed without the token (`send_raw`); downloads
/// need a token even on public repositories.
async fn h_artifact_zip(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Response> {
    let ctx = ctx(&s, &pid).await?;
    ctx.require_token("downloading artifacts")?;
    let a: Artifact = ctx.get(&ctx.rurl(&format!("/actions/artifacts/{id}")), &[], Fresh::Live).await?;
    if a.expired {
        return Err(ApiError::not_found(format!("artifact {} has expired", a.name)));
    }
    let upstream = ctx.send(Method::GET, &ctx.rurl(&format!("/actions/artifacts/{id}/zip")), |rb| rb.timeout(Duration::from_secs(3600))).await?;
    let length = upstream.headers().get(header::CONTENT_LENGTH).cloned();
    let mut resp = Response::new(axum::body::Body::from_stream(upstream.bytes_stream()));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    if let Some(l) = length {
        h.insert(header::CONTENT_LENGTH, l);
    }
    let name = zip_filename(&a.name);
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// `<artifact name>.zip`, ASCII only, for `Content-Disposition`.
fn zip_filename(name: &str) -> String {
    let base: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).take(100).collect();
    let base = base.trim_matches(['_', '.']);
    format!("{}.zip", if base.is_empty() { "artifact" } else { base })
}

/// Download the job log as a text file.
async fn h_log_download(State(s): State<AppState>, Path((pid, id)): Id) -> ApiResult<Response> {
    let ctx = ctx(&s, &pid).await?;
    let log = job_log(&ctx, id).await?;
    let Some(SharedLog(data)) = log.data else {
        return Err(ApiError::not_found(log.message.unwrap_or_else(|| "no log for this job".into())));
    };
    let mut resp = (logs::plain(&data.text) + "\n").into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"job-{id}.log\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

async fn h_workflows(State(s): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Vec<Workflow>>> {
    Ok(Json(workflows(&ctx(&s, &pid).await?).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct InputsQuery {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

async fn h_workflow_inputs(State(s): State<AppState>, Path((pid, id)): Id, Query(q): Query<InputsQuery>) -> ApiResult<Json<DispatchInfo>> {
    Ok(Json(workflow_inputs(&ctx(&s, &pid).await?, id, q.git_ref.as_deref()).await?))
}
async fn h_dispatch(State(s): State<AppState>, Path((pid, id)): Id, Json(b): Json<DispatchBody>) -> ApiResult<Json<Value>> {
    Ok(Json(dispatch(&ctx(&s, &pid).await?, id, &b).await?))
}

async fn h_commit_checks(State(s): State<AppState>, Path((pid, sha)): Path<(String, String)>) -> ApiResult<Json<Option<CommitChecks>>> {
    let ctx = ctx(&s, &pid).await?;
    let Some(full) = full_sha(&ctx, &sha).await? else { return Ok(Json(None)) };
    Ok(Json(known_commit_checks(&ctx, &full).await?))
}

pub fn routes() -> Router<AppState> {
    let p = "/api/projects/{pid}/github";
    Router::new()
        .route(&format!("{p}/actions/runs"), get(h_runs))
        .route(&format!("{p}/actions/runs/{{id}}"), get(h_run))
        .route(&format!("{p}/actions/runs/{{id}}/artifacts"), get(h_artifacts))
        .route(&format!("{p}/actions/artifacts/{{id}}/zip"), get(h_artifact_zip))
        .route(&format!("{p}/actions/runs/{{id}}/rerun"), post(h_rerun))
        .route(&format!("{p}/actions/runs/{{id}}/rerun-failed"), post(h_rerun_failed))
        .route(&format!("{p}/actions/runs/{{id}}/cancel"), post(h_cancel))
        .route(&format!("{p}/actions/jobs/{{id}}"), get(h_job))
        .route(&format!("{p}/actions/jobs/{{id}}/rerun"), post(h_job_rerun))
        .route(&format!("{p}/actions/jobs/{{id}}/logs"), get(h_logs))
        .route(&format!("{p}/actions/jobs/{{id}}/log"), get(h_log_download))
        .route(&format!("{p}/actions/jobs/{{id}}/annotations"), get(h_annotations))
        .route(&format!("{p}/actions/workflows"), get(h_workflows))
        .route(&format!("{p}/actions/workflows/{{id}}/inputs"), get(h_workflow_inputs))
        .route(&format!("{p}/actions/workflows/{{id}}/dispatch"), post(h_dispatch))
        .route(&format!("{p}/commits/{{sha}}/checks"), get(h_commit_checks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_inputs_from_yaml() {
        let y = r#"
name: Deploy
on:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      environment:
        description: Where to deploy
        required: true
        type: choice
        options: [staging, production]
        default: staging
      dry_run:
        type: boolean
        default: true
      note:
        description: Free text
jobs: {}
"#;
        let d = dispatch_info(y);
        assert!(d.dispatchable);
        assert_eq!(d.inputs.len(), 3);
        assert_eq!(d.inputs[0].name, "environment");
        assert_eq!(d.inputs[0].options, ["staging", "production"]);
        assert!(d.inputs[0].required);
        assert_eq!(d.inputs[1].input_type, "boolean");
        assert_eq!(d.inputs[1].default.as_deref(), Some("true"));
        assert_eq!(d.inputs[2].input_type, "string");
        assert!(dispatch_info("on: workflow_dispatch").dispatchable);
        assert!(dispatch_info("on: [push, workflow_dispatch]").dispatchable);
        assert!(!dispatch_info("on: [push]").dispatchable);
        assert!(!dispatch_info("not: [valid").dispatchable);
        assert!(dispatch_info("on:\n  workflow_dispatch:\n").dispatchable);
    }

    #[test]
    fn dispatch_payloads_are_checked() {
        let mut inputs = serde_json::Map::new();
        inputs.insert("dry_run".into(), json!(true));
        inputs.insert("count".into(), json!(3));
        let p = dispatch_payload(&DispatchBody { git_ref: "main".into(), inputs }).unwrap();
        assert_eq!(p["inputs"]["dry_run"], "true");
        assert_eq!(p["inputs"]["count"], "3");
        assert!(dispatch_payload(&DispatchBody { git_ref: "-x".into(), ..Default::default() }).is_err());
        let mut bad = serde_json::Map::new();
        bad.insert("a b".into(), json!("x"));
        assert!(dispatch_payload(&DispatchBody { git_ref: "main".into(), inputs: bad }).is_err());
    }

    #[test]
    fn shas_and_branches() {
        assert!(is_hex_sha("4b8e2508"));
        assert!(!is_hex_sha("--all"));
        assert!(valid_branch("feature/x-1").is_ok());
        assert!(valid_branch("a/../b").is_err());
        assert!(valid_branch("-rf").is_err());
    }

    #[test]
    fn the_main_run_explains_the_state() {
        let run = |id: u64, state: &str| Run { id, state: state.into(), ..Default::default() };
        let checks = CommitChecks {
            sha: "x".into(),
            state: Some("failed".into()),
            items: vec![],
            runs: vec![run(3, "success"), run(2, "failed"), run(1, "running")],
        };
        assert_eq!(main_run(&checks).map(|r| r.id), Some(2));
        let checks = CommitChecks { state: Some("success".into()), runs: vec![run(3, "success")], ..checks };
        assert_eq!(main_run(&checks).map(|r| r.id), Some(3));
    }
}

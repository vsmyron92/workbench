//! Pipelines and jobs: lists (enriched with durations and commit titles),
//! details grouped by stage with the test summary, incremental job logs,
//! retry/cancel/play, running a pipeline, artifact and log downloads.

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{GlCtx, ctx, read_tail};
use super::model::{self, Job, ListPage, Pipeline, PipelineDetail, TestSummary, group_stages};
use super::trace;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Bytes of log kept in memory per read (the tail wins when a log is larger).
pub const TRACE_KEEP: usize = 4 * 1024 * 1024;
/// Bytes of log kept for downloads and agent tails.
const TRACE_KEEP_FULL: usize = 64 * 1024 * 1024;
const KNOWN_STATUSES: &[&str] = &[
    "created", "waiting_for_resource", "preparing", "pending", "running", "success", "failed", "canceled",
    "skipped", "manual", "scheduled",
];

// ---------------------------------------------------------------- operations

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PipelinesQuery {
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub sha: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn list_pipelines(ctx: &GlCtx, q: &PipelinesQuery) -> ApiResult<ListPage<Pipeline>> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).clamp(1, 100);
    let mut query: Vec<(&str, String)> = vec![
        ("page", page.to_string()),
        ("per_page", per_page.to_string()),
        ("order_by", "id".into()),
        ("sort", "desc".into()),
    ];
    if let Some(r) = q.git_ref.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        query.push(("ref", r.to_string()));
    }
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty()) {
        if !KNOWN_STATUSES.contains(&s) {
            return Err(ApiError::bad_request(format!("unknown pipeline status {s:?}")));
        }
        query.push(("status", s.to_string()));
    }
    if let Some(s) = q.source.as_deref().filter(|s| !s.is_empty()) {
        query.push(("source", s.to_string()));
    }
    if let Some(s) = q.sha.as_deref().filter(|s| !s.is_empty()) {
        query.push(("sha", valid_sha(s)?.to_string()));
    }
    let res = ctx.get_page::<Pipeline>(&ctx.purl("/pipelines"), &query).await?;
    let items = enrich_pipelines(ctx, res.items).await;
    Ok(ListPage { items, page: res.page.unwrap_or(page), next_page: res.next_page, total: res.total })
}

/// The newest pipeline of a ref (enriched), if any.
pub async fn latest_pipeline(ctx: &GlCtx, git_ref: &str) -> ApiResult<Option<Pipeline>> {
    let q = PipelinesQuery { git_ref: Some(git_ref.to_string()), per_page: Some(1), ..Default::default() };
    Ok(list_pipelines(ctx, &q).await?.items.into_iter().next())
}

/// List entries lack durations and users: fill them from pipeline details
/// (cached while `updated_at` is unchanged) and add local commit titles.
pub async fn enrich_pipelines(ctx: &GlCtx, list: Vec<Pipeline>) -> Vec<Pipeline> {
    let mut out: Vec<Pipeline> = futures::stream::iter(list.into_iter().map(|p| async move {
        let key = format!("{}|{}", ctx.api, p.id);
        if let Some(updated) = p.updated_at.as_deref() {
            if let Some(cached) = ctx.state.gitlab.cached_pipeline(&key, updated) {
                return cached;
            }
        }
        match ctx.get::<Pipeline>(&ctx.purl(&format!("/pipelines/{}", p.id)), &[]).await {
            Ok(detail) => {
                ctx.state.gitlab.cache_pipeline(key, &detail);
                detail
            }
            Err(_) => p,
        }
    }))
    .buffered(6)
    .collect()
    .await;
    add_commit_titles(ctx, &mut out).await;
    out
}

async fn add_commit_titles(ctx: &GlCtx, pipelines: &mut [Pipeline]) {
    let shas: Vec<String> = pipelines.iter().map(|p| p.sha.clone()).collect();
    let titles = local_commit_titles(&ctx.project.root, &shas).await;
    for p in pipelines {
        if p.commit_title.is_none() {
            p.commit_title = titles.get(&p.sha).cloned();
        }
    }
}

/// Subjects of the given commits from the local repository (one `git log`
/// process; commits the clone does not have are skipped).
pub async fn local_commit_titles(root: &FsPath, shas: &[String]) -> HashMap<String, String> {
    let mut valid: Vec<&str> = shas.iter().map(String::as_str).filter(|s| is_hex_sha(s)).collect();
    valid.sort_unstable();
    valid.dedup();
    if valid.is_empty() {
        return HashMap::new();
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-c", "core.quotepath=false", "log", "--no-walk=unsorted", "--ignore-missing", "--format=%H%x1f%s"])
        .args(&valid)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    let Ok(out) = crate::util::proc::run_cmd(cmd, Duration::from_secs(5)).await else { return HashMap::new() };
    out.stdout
        .lines()
        .filter_map(|l| l.split_once('\u{1f}'))
        .map(|(sha, subject)| (sha.to_string(), subject.to_string()))
        .collect()
}

pub fn is_hex_sha(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_sha(s: &str) -> ApiResult<&str> {
    let s = s.trim();
    if is_hex_sha(s) { Ok(s) } else { Err(ApiError::bad_request(format!("not a commit sha: {s:?}"))) }
}

pub async fn get_pipeline(ctx: &GlCtx, id: u64) -> ApiResult<Pipeline> {
    let mut p: Pipeline = ctx.get(&ctx.purl(&format!("/pipelines/{id}")), &[]).await?;
    ctx.state.gitlab.cache_pipeline(format!("{}|{}", ctx.api, p.id), &p);
    add_commit_titles(ctx, std::slice::from_mut(&mut p)).await;
    Ok(p)
}

pub async fn pipeline_detail(ctx: &GlCtx, id: u64) -> ApiResult<PipelineDetail> {
    let jobs_url = ctx.purl(&format!("/pipelines/{id}/jobs"));
    let bridges_url = ctx.purl(&format!("/pipelines/{id}/bridges"));
    let tests_url = ctx.purl(&format!("/pipelines/{id}/test_report_summary"));
    let (pipeline, jobs, bridges, tests) = tokio::join!(
        get_pipeline(ctx, id),
        ctx.get_all::<Job>(&jobs_url, &[], 2000),
        ctx.get_all::<Job>(&bridges_url, &[], 200),
        ctx.get_opt::<TestSummary>(&tests_url, &[]),
    );
    let pipeline = pipeline?;
    let (mut jobs, _) = jobs?;
    for j in &mut jobs {
        j.kind = "job".into();
    }
    // Bridges and test reports are optional extras (older servers, no permission).
    if let Ok((bridges, _)) = bridges {
        jobs.extend(bridges.into_iter().map(|mut b| {
            b.kind = "bridge".into();
            b
        }));
    }
    let test_summary = tests.ok().flatten().filter(|t| t.total.count > 0 || t.total.suite_error.is_some());
    Ok(PipelineDetail { pipeline, stages: group_stages(jobs), test_summary })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Variable {
    pub key: String,
    pub value: String,
    /// `env_var` (default) or `file`.
    pub variable_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreatePipeline {
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub variables: Vec<Variable>,
}

pub fn valid_variables(vars: &[Variable]) -> ApiResult<Vec<Value>> {
    if vars.len() > 50 {
        return Err(ApiError::bad_request("at most 50 variables"));
    }
    vars.iter()
        .filter(|v| !v.key.trim().is_empty())
        .map(|v| {
            let key = v.key.trim();
            let ok = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && key.len() <= 255;
            if !ok {
                return Err(ApiError::bad_request(format!("invalid variable name {key:?}")));
            }
            let t = v.variable_type.as_deref().unwrap_or("env_var");
            if t != "env_var" && t != "file" {
                return Err(ApiError::bad_request("variableType must be env_var or file"));
            }
            Ok(json!({ "key": key, "value": v.value, "variable_type": t }))
        })
        .collect()
}

pub async fn create_pipeline(ctx: &GlCtx, body: &CreatePipeline) -> ApiResult<Pipeline> {
    let git_ref = body.git_ref.trim();
    if git_ref.is_empty() {
        return Err(ApiError::bad_request("ref is required"));
    }
    let vars = valid_variables(&body.variables)?;
    let mut payload = json!({ "ref": git_ref });
    if !vars.is_empty() {
        payload["variables"] = Value::Array(vars);
    }
    let p: Pipeline = ctx.write(Method::POST, &ctx.purl("/pipeline"), Some(&payload)).await?;
    super::pipeline_changed(ctx, p.id, p.iid, &p.status, &p.git_ref, &p.sha, &p.web_url);
    Ok(p)
}

pub async fn pipeline_action(ctx: &GlCtx, id: u64, action: &str) -> ApiResult<Pipeline> {
    let p: Pipeline = ctx.write(Method::POST, &ctx.purl(&format!("/pipelines/{id}/{action}")), None).await?;
    super::pipeline_changed(ctx, p.id, p.iid, &p.status, &p.git_ref, &p.sha, &p.web_url);
    Ok(p)
}

pub async fn get_job(ctx: &GlCtx, id: u64) -> ApiResult<Job> {
    let mut j: Job = ctx.get(&ctx.purl(&format!("/jobs/{id}")), &[]).await?;
    j.kind = "job".into();
    Ok(j)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlayBody {
    pub variables: Vec<Variable>,
}

/// `retry` (returns the new job), `cancel` or `play` (manual jobs).
pub async fn job_action(ctx: &GlCtx, id: u64, action: &str, vars: &[Variable]) -> ApiResult<Job> {
    let body = if action == "play" && !vars.is_empty() {
        let attrs: Vec<Value> = valid_variables(vars)?
            .into_iter()
            .map(|v| json!({ "key": v["key"], "value": v["value"] }))
            .collect();
        Some(json!({ "job_variables_attributes": attrs }))
    } else {
        None
    };
    let mut j: Job = ctx.write(Method::POST, &ctx.purl(&format!("/jobs/{id}/{action}")), body.as_ref()).await?;
    j.kind = "job".into();
    ctx.state.events.emit(
        "gitlab.job",
        Some(&ctx.project.id),
        json!({ "jobId": j.id, "previousJobId": id, "action": action, "status": j.status,
                "pipelineId": j.pipeline.as_ref().map(|p| p.id) }),
    );
    if let Some(p) = &j.pipeline {
        // The job's embedded pipeline status may lag; the poller refines it.
        let status = if model::is_active_status(&j.status) { "running" } else { p.status.as_str() };
        super::pipeline_changed(ctx, p.id, p.iid, status, &p.git_ref, &p.sha, &p.web_url);
    }
    Ok(j)
}

/// An incremental piece of a job log.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceChunk {
    /// Prefix-stripped log text (ANSI and section markers kept).
    pub text: String,
    /// Pass back as `?offset=` on the next poll.
    pub offset: u64,
    /// The job has finished and the whole log has been delivered.
    pub complete: bool,
    /// Job status at the time of the read.
    pub status: String,
    /// `text` replaces what the client has (first read, or the log restarted).
    pub reset: bool,
    /// The start of the log was left out (it is larger than we keep).
    pub truncated: bool,
    /// Raw log size in bytes, when known.
    pub size: Option<u64>,
}

/// Read the log from byte `offset` (see `trace` for the offset rules).
pub async fn read_trace(ctx: &GlCtx, id: u64, offset: u64) -> ApiResult<TraceChunk> {
    read_trace_inner(ctx, id, offset, true).await
}

async fn read_trace_inner(ctx: &GlCtx, id: u64, offset: u64, may_restart: bool) -> ApiResult<TraceChunk> {
    // Status first: when it says finished, the log read afterwards is final.
    let job = get_job(ctx, id).await?;
    let finished = model::is_finished_status(&job.status);
    let url = ctx.purl(&format!("/jobs/{id}/trace"));
    let resp = ctx
        .send_raw(Method::GET, &url, |rb| if offset > 0 { rb.header(header::RANGE, format!("bytes={offset}-")) } else { rb })
        .await?;
    let status = resp.status();
    let range = resp
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .map(trace::parse_content_range)
        .unwrap_or((None, None));
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        if let (_, Some(total)) = range {
            if total < offset && may_restart {
                // The log got shorter (erased or restarted): start over.
                return Box::pin(read_trace_inner(ctx, id, 0, false)).await;
            }
        }
        return Ok(TraceChunk {
            text: String::new(),
            offset,
            complete: finished,
            status: job.status,
            reset: false,
            truncated: false,
            size: range.1,
        });
    }
    if !status.is_success() {
        return Err(ctx.error_from(resp).await);
    }
    let partial = status == StatusCode::PARTIAL_CONTENT;
    let start = if partial { range.0.unwrap_or(offset) } else { 0 };
    let (mut bytes, read, dropped) = read_tail(resp, TRACE_KEEP).await?;
    let end = start + read;
    let mut begin = end - bytes.len() as u64;
    let mut reset = offset == 0;
    let mut truncated = dropped;
    if !partial && offset > 0 {
        if end < offset {
            reset = true; // shorter than before: restarted
        } else if begin <= offset {
            let skip = (offset - begin) as usize;
            bytes.drain(..skip);
            begin = offset;
        } else {
            reset = true; // more new output than we keep: show the tail
            truncated = true;
        }
    } else if partial && start != offset {
        reset = true;
    }
    if truncated {
        let a = trace::align_to_line(&bytes);
        bytes.drain(..a);
        begin += a as u64;
        reset = true;
    }
    let take = trace::consumable_len(&bytes, finished);
    let text = trace::strip_prefixes(&String::from_utf8_lossy(&bytes[..take]), reset);
    Ok(TraceChunk {
        text,
        offset: begin + take as u64,
        complete: finished && take == bytes.len(),
        status: job.status,
        reset,
        truncated,
        size: range.1.or(Some(end)),
    })
}

/// The whole log (last 64 MB at most), prefixes stripped.
pub async fn full_log(ctx: &GlCtx, id: u64) -> ApiResult<(String, bool)> {
    let url = ctx.purl(&format!("/jobs/{id}/trace"));
    let resp = ctx.send(Method::GET, &url, |rb| rb.timeout(Duration::from_secs(300))).await?;
    let (mut bytes, _, dropped) = read_tail(resp, TRACE_KEEP_FULL).await?;
    if dropped {
        let a = trace::align_to_line(&bytes);
        bytes.drain(..a);
    }
    Ok((trace::strip_prefixes(&String::from_utf8_lossy(&bytes), true), dropped))
}

/// The last `n` lines of a job's log (plain text when `plain`), with the job.
pub async fn log_tail(ctx: &GlCtx, id: u64, n: usize, plain: bool) -> ApiResult<(Job, String, usize, bool)> {
    let job = get_job(ctx, id).await?;
    let url = ctx.purl(&format!("/jobs/{id}/trace"));
    let resp = ctx.send(Method::GET, &url, |rb| rb).await?;
    let (mut bytes, _, dropped) = read_tail(resp, TRACE_KEEP).await?;
    if dropped {
        let a = trace::align_to_line(&bytes);
        bytes.drain(..a);
    }
    let text = trace::strip_prefixes(&String::from_utf8_lossy(&bytes), true);
    let text = if plain { trace::plain(&text) } else { text };
    let (tail, total) = trace::tail_lines(&text, n);
    Ok((job, tail, total, dropped))
}

// ---------------------------------------------------------------- handlers

async fn h_list(
    State(state): State<AppState>,
    Path(pid): Path<String>,
    Query(q): Query<PipelinesQuery>,
) -> ApiResult<Json<ListPage<Pipeline>>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(list_pipelines(&ctx, &q).await?))
}

async fn h_create(
    State(state): State<AppState>,
    Path(pid): Path<String>,
    Json(body): Json<CreatePipeline>,
) -> ApiResult<Json<Pipeline>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(create_pipeline(&ctx, &body).await?))
}

async fn h_detail(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<PipelineDetail>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(pipeline_detail(&ctx, id).await?))
}

// ---------------------------------------------------------------- test report

/// Failed and errored test cases listed at most (the counts cover every case).
const MAX_FAILED_CASES: usize = 200;
/// Output kept per case: the failure message comes first, the stack trace after it.
const MAX_CASE_OUTPUT: usize = 12 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlTestReport {
    total_time: f64,
    total_count: u64,
    success_count: u64,
    failed_count: u64,
    skipped_count: u64,
    error_count: u64,
    test_suites: Vec<GlReportSuite>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlReportSuite {
    name: String,
    total_time: f64,
    total_count: u64,
    failed_count: u64,
    skipped_count: u64,
    error_count: u64,
    suite_error: Option<String>,
    test_cases: Vec<GlTestCase>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlTestCase {
    status: String,
    name: String,
    classname: String,
    file: Option<String>,
    execution_time: f64,
    /// A string, or (older servers) an array of lines.
    system_output: Value,
    stack_trace: Option<String>,
    recent_failures: Option<GlRecentFailures>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlRecentFailures {
    count: u64,
    base_branch: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestFailures {
    pub total: model::TestTotals,
    pub suites: Vec<SuiteFailures>,
    /// More failed cases exist than are listed.
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteFailures {
    pub name: String,
    pub total_count: u64,
    pub failed_count: u64,
    pub error_count: u64,
    pub skipped_count: u64,
    pub time: f64,
    pub suite_error: Option<String>,
    pub cases: Vec<FailedCase>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedCase {
    /// `failed` or `error`.
    pub status: String,
    pub name: String,
    pub classname: String,
    /// The test's file as the report names it (JUnit `file`), when it does.
    pub file: Option<String>,
    pub time: f64,
    /// The failure message and the stack trace, cut at `MAX_CASE_OUTPUT`.
    pub output: String,
    pub output_truncated: bool,
    /// Failures of this test on the base branch lately (GitLab's `recent_failures`).
    pub recent_failures: Option<u64>,
    pub base_branch: Option<String>,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Keep what matters of GitLab's test report: counts, and the failed cases with
/// their output (successes, often thousands, stay on GitLab).
fn test_failures(r: GlTestReport) -> TestFailures {
    let mut listed = 0usize;
    let mut truncated = false;
    let suites = r
        .test_suites
        .into_iter()
        .map(|s| {
            let mut failing: Vec<GlTestCase> = s.test_cases.into_iter().filter(|c| c.status == "failed" || c.status == "error").collect();
            failing.sort_by_key(|c| c.status != "failed");
            let room = MAX_FAILED_CASES.saturating_sub(listed);
            truncated |= failing.len() > room;
            failing.truncate(room);
            listed += failing.len();
            let cases = failing
                .into_iter()
                .map(|c| {
                    let mut output = text_of(&c.system_output).trim_end().to_string();
                    if let Some(t) = c.stack_trace.as_deref().map(str::trim).filter(|t| !t.is_empty() && !output.contains(*t)) {
                        if !output.is_empty() {
                            output.push_str("\n\n");
                        }
                        output.push_str(t);
                    }
                    let output_truncated = output.len() > MAX_CASE_OUTPUT;
                    if output_truncated {
                        let mut cut = MAX_CASE_OUTPUT;
                        while !output.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        output.truncate(cut);
                    }
                    let (recent_failures, base_branch) = match c.recent_failures {
                        Some(f) if f.count > 0 => (Some(f.count), Some(f.base_branch).filter(|b| !b.is_empty())),
                        _ => (None, None),
                    };
                    FailedCase {
                        status: c.status,
                        name: c.name,
                        classname: c.classname,
                        file: c.file.filter(|f| !f.is_empty()),
                        time: c.execution_time,
                        output,
                        output_truncated,
                        recent_failures,
                        base_branch,
                    }
                })
                .collect();
            SuiteFailures {
                name: s.name,
                total_count: s.total_count,
                failed_count: s.failed_count,
                error_count: s.error_count,
                skipped_count: s.skipped_count,
                time: s.total_time,
                suite_error: s.suite_error.filter(|e| !e.is_empty()),
                cases,
            }
        })
        .collect();
    TestFailures {
        total: model::TestTotals {
            time: r.total_time,
            count: r.total_count,
            success: r.success_count,
            failed: r.failed_count,
            skipped: r.skipped_count,
            error: r.error_count,
            suite_error: None,
        },
        suites,
        truncated,
    }
}

/// The pipeline's failed tests (GitLab's `test_report`, JUnit reports of its jobs).
pub async fn pipeline_test_failures(ctx: &GlCtx, id: u64) -> ApiResult<Option<TestFailures>> {
    let r = ctx.get_opt::<GlTestReport>(&ctx.purl(&format!("/pipelines/{id}/test_report")), &[]).await?;
    Ok(r.map(test_failures))
}

async fn h_tests(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<TestFailures>> {
    let ctx = ctx(&state, &pid).await?;
    pipeline_test_failures(&ctx, id).await?.map(Json).ok_or_else(|| ApiError::not_found("this pipeline has no test report"))
}

async fn h_retry(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<Pipeline>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(pipeline_action(&ctx, id, "retry").await?))
}

async fn h_cancel(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<Pipeline>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(pipeline_action(&ctx, id, "cancel").await?))
}

async fn h_job(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<Job>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(get_job(&ctx, id).await?))
}

async fn h_job_retry(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<Job>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(job_action(&ctx, id, "retry", &[]).await?))
}

async fn h_job_cancel(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Json<Job>> {
    let ctx = ctx(&state, &pid).await?;
    Ok(Json(job_action(&ctx, id, "cancel", &[]).await?))
}

async fn h_job_play(
    State(state): State<AppState>,
    Path((pid, id)): Path<(String, u64)>,
    body: Option<Json<PlayBody>>,
) -> ApiResult<Json<Job>> {
    let ctx = ctx(&state, &pid).await?;
    let vars = body.map(|Json(b)| b.variables).unwrap_or_default();
    Ok(Json(job_action(&ctx, id, "play", &vars).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TraceQuery {
    offset: Option<u64>,
    /// Return only the last N lines (≤ 5000) instead of an incremental chunk.
    tail: Option<usize>,
    /// With `tail`: ANSI and section markers removed.
    plain: Option<bool>,
}

async fn h_trace(
    State(state): State<AppState>,
    Path((pid, id)): Path<(String, u64)>,
    Query(q): Query<TraceQuery>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, &pid).await?;
    if let Some(n) = q.tail {
        let (job, text, total, truncated) = log_tail(&ctx, id, n.clamp(1, 5000), q.plain.unwrap_or(false)).await?;
        return Ok(Json(json!({
            "text": text, "totalLines": total, "truncated": truncated,
            "status": job.status, "complete": model::is_finished_status(&job.status),
        })));
    }
    let chunk = read_trace(&ctx, id, q.offset.unwrap_or(0)).await?;
    Ok(Json(serde_json::to_value(chunk)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LogQuery {
    /// Keep ANSI colours and section markers (default: plain text).
    ansi: Option<bool>,
}

/// Download the job log as a file.
async fn h_log_download(
    State(state): State<AppState>,
    Path((pid, id)): Path<(String, u64)>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Response> {
    let ctx = ctx(&state, &pid).await?;
    let (text, _) = full_log(&ctx, id).await?;
    let text = if q.ansi.unwrap_or(false) { text } else { trace::plain(&text) };
    let mut resp = (text + "\n").into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"job-{id}.log\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// Stream the job's artifacts archive through (never buffered whole).
async fn h_artifacts(State(state): State<AppState>, Path((pid, id)): Path<(String, u64)>) -> ApiResult<Response> {
    let ctx = ctx(&state, &pid).await?;
    let job = get_job(&ctx, id).await?;
    if !job.has_archive() {
        return Err(ApiError::not_found("this job has no artifacts archive"));
    }
    let url = ctx.purl(&format!("/jobs/{id}/artifacts"));
    let upstream = ctx.send(Method::GET, &url, |rb| rb.timeout(Duration::from_secs(3600))).await?;
    let name = job
        .artifacts_file
        .as_ref()
        .map(|f| f.filename.clone())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "artifacts.zip".into());
    let name = safe_filename(&format!("{}-{}", job.name, name));
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let length = upstream.headers().get(header::CONTENT_LENGTH).cloned();
    let mut resp = Response::new(Body::from_stream(upstream.bytes_stream()));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        content_type.unwrap_or_else(|| HeaderValue::from_static("application/octet-stream")),
    );
    if let Some(l) = length {
        h.insert(header::CONTENT_LENGTH, l);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// ASCII-only file name for `Content-Disposition`.
pub fn safe_filename(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "download".into() } else { out.chars().take(120).collect() }
}

pub fn routes() -> Router<AppState> {
    let p = "/api/projects/{pid}/gitlab";
    Router::new()
        .route(&format!("{p}/pipelines"), get(h_list).post(h_create))
        .route(&format!("{p}/pipelines/{{id}}"), get(h_detail))
        .route(&format!("{p}/pipelines/{{id}}/tests"), get(h_tests))
        .route(&format!("{p}/pipelines/{{id}}/retry"), post(h_retry))
        .route(&format!("{p}/pipelines/{{id}}/cancel"), post(h_cancel))
        .route(&format!("{p}/jobs/{{id}}"), get(h_job))
        .route(&format!("{p}/jobs/{{id}}/trace"), get(h_trace))
        .route(&format!("{p}/jobs/{{id}}/log"), get(h_log_download))
        .route(&format!("{p}/jobs/{{id}}/artifacts"), get(h_artifacts))
        .route(&format!("{p}/jobs/{{id}}/retry"), post(h_job_retry))
        .route(&format!("{p}/jobs/{{id}}/cancel"), post(h_job_cancel))
        .route(&format!("{p}/jobs/{{id}}/play"), post(h_job_play))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_are_validated() {
        let ok = valid_variables(&[Variable { key: "DEPLOY_ENV".into(), value: "x y".into(), variable_type: None }]).unwrap();
        assert_eq!(ok[0]["variable_type"], "env_var");
        assert!(valid_variables(&[Variable { key: "1BAD".into(), ..Default::default() }]).is_err());
        assert!(valid_variables(&[Variable { key: "A-B".into(), ..Default::default() }]).is_err());
        assert!(valid_variables(&[Variable { key: "OK".into(), variable_type: Some("x".into()), ..Default::default() }]).is_err());
        // Blank rows from the UI are ignored.
        assert!(valid_variables(&[Variable::default()]).unwrap().is_empty());
    }

    #[test]
    fn test_reports_keep_failures_with_their_output() {
        let r: GlTestReport = serde_json::from_value(json!({
            "total_time": 3.5, "total_count": 4, "success_count": 1, "failed_count": 2, "skipped_count": 0, "error_count": 1,
            "test_suites": [{
                "name": "rspec", "total_time": 3.5, "total_count": 4, "failed_count": 2, "error_count": 1, "skipped_count": 0, "suite_error": "",
                "test_cases": [
                    {"status": "success", "name": "ok", "classname": "A", "execution_time": 0.1},
                    {"status": "error", "name": "boom", "classname": "A", "execution_time": 0.2, "system_output": ["line 1", "line 2"]},
                    {"status": "failed", "name": "adds", "classname": "Calc", "file": "spec/calc_spec.rb", "execution_time": 0.3,
                     "system_output": "expected 3, got 4", "stack_trace": "calc_spec.rb:12", "recent_failures": {"count": 2, "base_branch": "main"}},
                    {"status": "failed", "name": "huge", "classname": "Calc", "execution_time": 0.3, "system_output": "é".repeat(MAX_CASE_OUTPUT)}
                ]
            }]
        }))
        .unwrap();
        let f = test_failures(r);
        assert_eq!((f.total.count, f.total.failed, f.total.error), (4, 2, 1));
        let cases = &f.suites[0].cases;
        assert_eq!(cases.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["adds", "huge", "boom"], "failures first, successes gone");
        assert_eq!(cases[0].output, "expected 3, got 4\n\ncalc_spec.rb:12");
        assert_eq!(cases[0].file.as_deref(), Some("spec/calc_spec.rb"));
        assert_eq!((cases[0].recent_failures, cases[0].base_branch.as_deref()), (Some(2), Some("main")));
        assert!(cases[1].output_truncated && cases[1].output.len() <= MAX_CASE_OUTPUT);
        assert_eq!(cases[2].output, "line 1\nline 2");
        assert!(f.suites[0].suite_error.is_none() && !f.truncated);
    }

    #[test]
    fn shas_and_filenames() {
        assert!(is_hex_sha("4b8e2508"));
        assert!(!is_hex_sha("--all"));
        assert!(!is_hex_sha("4b8e"));
        assert_eq!(safe_filename("web build-artifacts.zip"), "web_build-artifacts.zip");
        assert_eq!(safe_filename("\"; rm"), "rm");
        assert_eq!(safe_filename("///"), "download");
    }
}

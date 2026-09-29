//! REST under `/api/projects/{pid}/debug/`. Everything that starts, steps, evaluates
//! in, changes or stops a session (and every breakpoint write) refuses in-process
//! callers: agents read the state through the `debug_state` tool only.
//!
//! * `GET adapters` → `{adapters: AdapterView[], warnings}`; `GET configs` →
//!   `{configs: LaunchConfigView[]}`; `GET processes` → `ProcessList` (attach picker).
//! * `GET sessions` → `SessionInfo[]`; `POST sessions {config, stopOnEntry?}`;
//!   `POST sessions/attach {pid, adapter?, language?, program?}`;
//!   `GET|DELETE sessions/{sid}`; `POST sessions/{sid}/stop|restart`;
//!   `POST sessions/{sid}/control {action: continue|pause|next|stepIn|stepOut, threadId?}`;
//!   `POST sessions/{sid}/run-to {path, line, threadId?}`.
//! * Inspection (valid while suspended; ids belong to the session's `stopEpoch`):
//!   `GET sessions/{sid}/threads`, `stack?threadId=&start=&levels=`, `scopes?frameId=`,
//!   `variables?ref=&start=&count=&filter=`, `source?ref=`, `file?path=` (a file outside
//!   the project the session's frames or output named; user only), `output?after=&limit=`;
//!   `POST sessions/{sid}/evaluate {expression, frameId?, context}`,
//!   `set-variable {variablesReference, name, value}`, `completions {text, column, frameId?}`.
//! * `GET breakpoints`; `PUT breakpoints/file {path, breakpoints}`,
//!   `PUT breakpoints/functions {breakpoints}`, `PUT breakpoints/exceptions {adapter, filters}`,
//!   `PUT breakpoints/mute {muted}`, `POST breakpoints/clear`; `PUT watches {expressions}`.

use axum::extract::{Extension, Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::breakpoints::{FunctionBreakpoint, LineBreakpoint};
use super::session::{self, EVAL_TIMEOUT, Lines, REQUEST_TIMEOUT, Session, SessionInfo};
use super::{adapters, launch, procs};
use crate::app::AppState;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};

pub fn router() -> Router<AppState> {
    let b = "/api/projects/{pid}/debug";
    Router::new()
        .route(&format!("{b}/adapters"), get(adapters_list))
        .route(&format!("{b}/configs"), get(configs))
        .route(&format!("{b}/processes"), get(processes))
        .route(&format!("{b}/sessions"), get(sessions).post(start_session))
        .route(&format!("{b}/sessions/attach"), post(attach))
        .route(&format!("{b}/sessions/{{sid}}"), get(session_info).delete(forget))
        .route(&format!("{b}/sessions/{{sid}}/stop"), post(stop))
        .route(&format!("{b}/sessions/{{sid}}/restart"), post(restart))
        .route(&format!("{b}/sessions/{{sid}}/control"), post(control))
        .route(&format!("{b}/sessions/{{sid}}/run-to"), post(run_to))
        .route(&format!("{b}/sessions/{{sid}}/threads"), get(threads))
        .route(&format!("{b}/sessions/{{sid}}/stack"), get(stack))
        .route(&format!("{b}/sessions/{{sid}}/scopes"), get(scopes))
        .route(&format!("{b}/sessions/{{sid}}/variables"), get(variables))
        .route(&format!("{b}/sessions/{{sid}}/source"), get(source))
        .route(&format!("{b}/sessions/{{sid}}/file"), get(file))
        .route(&format!("{b}/sessions/{{sid}}/output"), get(output))
        .route(&format!("{b}/sessions/{{sid}}/evaluate"), post(evaluate))
        .route(&format!("{b}/sessions/{{sid}}/set-variable"), post(set_variable))
        .route(&format!("{b}/sessions/{{sid}}/completions"), post(completions))
        .route(&format!("{b}/breakpoints"), get(breakpoints_get))
        .route(&format!("{b}/breakpoints/file"), put(breakpoints_file))
        .route(&format!("{b}/breakpoints/functions"), put(breakpoints_functions))
        .route(&format!("{b}/breakpoints/exceptions"), put(breakpoints_exceptions))
        .route(&format!("{b}/breakpoints/mute"), put(breakpoints_mute))
        .route(&format!("{b}/breakpoints/clear"), post(breakpoints_clear))
        .route(&format!("{b}/watches"), put(watches))
}

type C = Option<Extension<Caller>>;

/// Debug sessions run project code: only the user starts, steps or changes them.
fn user_only(caller: &C) -> ApiResult<()> {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("debug sessions are started and controlled by the user only; agents can read them with debug_state")),
        _ => Ok(()),
    }
}

fn session_of(state: &AppState, pid: &str, sid: &str) -> ApiResult<std::sync::Arc<Session>> {
    state.projects.require(pid)?;
    state.debug.get(pid, sid).ok_or_else(|| ApiError::not_found(format!("no debug session {sid}")))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

// ---------------------------------------------------------------- setup

async fn adapters_list(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    state.projects.require(&pid)?;
    let (list, warnings) = adapters::views(&state).await;
    Ok(Json(json!({ "adapters": list, "warnings": warnings })))
}

async fn configs(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    let p2 = p.clone();
    // Deriving reads manifests from disk.
    let st = state.clone();
    let list = tokio::spawn(async move { launch::views(&st, &p2).await }).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let last = state.debug.store.get(&state.paths.data_dir, &pid).last_config;
    Ok(Json(json!({ "configs": list, "lastConfig": last })))
}

async fn processes(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<procs::ProcessList>> {
    state.projects.require(&pid)?;
    let l = tokio::task::spawn_blocking(|| procs::list(std::path::Path::new("/proc"))).await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(l))
}

// ---------------------------------------------------------------- sessions

async fn sessions(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Vec<SessionInfo>>> {
    state.projects.require(&pid)?;
    Ok(Json(state.debug.sessions_of(&pid).iter().map(|s| s.info()).collect()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    config: String,
    #[serde(default)]
    stop_on_entry: Option<bool>,
    /// The process an attach configuration without a `pid` attaches to (the UI asks
    /// after `400 pid_required`).
    #[serde(default)]
    pid: Option<u32>,
}

async fn start_session(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<StartBody>) -> ApiResult<Json<SessionInfo>> {
    user_only(&caller)?;
    let p = state.projects.require(&pid)?;
    let plan = launch::plan_config(&state, &p, &b.config, b.stop_on_entry, b.pid).await?;
    Ok(Json(session::start(&state, p, plan, None).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachBody {
    pid: u32,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    program: Option<String>,
}

async fn attach(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<AttachBody>) -> ApiResult<Json<SessionInfo>> {
    user_only(&caller)?;
    if b.pid <= 1 || b.pid == std::process::id() {
        return Err(ApiError::bad_request("pick another process"));
    }
    let p = state.projects.require(&pid)?;
    let plan = launch::plan_attach(&state, &p, b.pid, b.adapter.as_deref(), b.language.as_deref(), b.program.as_deref()).await?;
    Ok(Json(session::start(&state, p, plan, None).await?))
}

async fn session_info(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<SessionInfo>> {
    Ok(Json(session_of(&state, &pid, &sid)?.info()))
}

async fn forget(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    if s.is_live() {
        session::stop(&state, &s).await;
    }
    state.debug.remove(&sid);
    state.events.emit("debug.session", Some(&pid), json!({ "id": sid, "projectId": pid, "removed": true }));
    Ok(Json(json!({ "ok": true })))
}

async fn stop(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<SessionInfo>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    session::stop(&state, &s).await;
    Ok(Json(s.info()))
}

async fn restart(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<SessionInfo>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    let Some(config) = s.config.clone() else {
        return Err(ApiError::bad_request("only sessions of a launch configuration can be rerun"));
    };
    let p = state.projects.require(&pid)?;
    // Plan first: a configuration that no longer works keeps the old session. An
    // attach configuration reattaches to the process it was given.
    let plan = launch::plan_config(&state, &p, &config, None, s.attach_pid()).await?;
    session::stop(&state, &s).await;
    state.debug.remove(&sid);
    state.events.emit("debug.session", Some(&pid), json!({ "id": sid, "projectId": pid, "removed": true }));
    Ok(Json(session::start(&state, p, plan, None).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlBody {
    action: String,
    #[serde(default)]
    thread_id: Option<i64>,
}

async fn control(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>, Json(b): Json<ControlBody>) -> ApiResult<Json<SessionInfo>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    session::control(&state, &s, &b.action, b.thread_id).await?;
    Ok(Json(s.info()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunToBody {
    path: String,
    line: i64,
    #[serde(default)]
    thread_id: Option<i64>,
}

async fn run_to(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>, Json(b): Json<RunToBody>) -> ApiResult<Json<SessionInfo>> {
    user_only(&caller)?;
    if b.line < 1 {
        return Err(ApiError::bad_request("invalid line"));
    }
    let s = session_of(&state, &pid, &sid)?;
    session::run_to(&state, &s, &b.path, b.line, b.thread_id).await?;
    Ok(Json(s.info()))
}

// ---------------------------------------------------------------- inspection

async fn threads(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let s = session_of(&state, &pid, &sid)?;
    let body = s.request("threads", Value::Null, REQUEST_TIMEOUT).await?;
    let threads: Vec<Value> = body
        .get("threads")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|t| json!({ "id": t.get("id"), "name": s.redact(t.get("name").and_then(Value::as_str).unwrap_or("")) })).collect())
        .unwrap_or_default();
    Ok(Json(json!({ "threads": threads })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackQuery {
    thread_id: i64,
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    levels: Option<i64>,
}

/// Frames with their sources mapped to project paths. Files outside the project
/// become readable through `file?path=` for this session.
pub fn frames_view(s: &Session, body: &Value) -> Value {
    let frames: Vec<Value> = body
        .get("stackFrames")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| {
                    let source = f.get("source").map(|src| s.paths.source_view(src));
                    if let Some(src) = &source {
                        if src.get("inProject") == Some(&Value::Bool(false)) {
                            if let Some(p) = src.get("path").and_then(Value::as_str) {
                                s.note_source(p);
                            }
                        }
                    }
                    json!({
                        "id": f.get("id").cloned().unwrap_or(Value::Null),
                        "name": truncate(&s.redact(f.get("name").and_then(Value::as_str).unwrap_or("?")), 300),
                        "line": f.get("line").and_then(Value::as_i64).unwrap_or(0),
                        "column": f.get("column").and_then(Value::as_i64).unwrap_or(0),
                        "source": source,
                        "presentationHint": f.get("presentationHint"),
                        "moduleId": f.get("moduleId"),
                        "instructionPointerReference": f.get("instructionPointerReference"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "frames": frames, "totalFrames": body.get("totalFrames") })
}

async fn stack(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>, Query(q): Query<StackQuery>) -> ApiResult<Json<Value>> {
    let s = session_of(&state, &pid, &sid)?;
    let levels = q.levels.unwrap_or(50).clamp(1, 200);
    let body = s
        .request("stackTrace", json!({ "threadId": q.thread_id, "startFrame": q.start.unwrap_or(0).max(0), "levels": levels }), REQUEST_TIMEOUT)
        .await?;
    Ok(Json(frames_view(&s, &body)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopesQuery {
    frame_id: i64,
}

async fn scopes(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>, Query(q): Query<ScopesQuery>) -> ApiResult<Json<Value>> {
    let s = session_of(&state, &pid, &sid)?;
    let body = s.request("scopes", json!({ "frameId": q.frame_id }), REQUEST_TIMEOUT).await?;
    let scopes: Vec<Value> = body
        .get("scopes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|sc| {
                    json!({
                        "name": sc.get("name"),
                        "variablesReference": sc.get("variablesReference"),
                        "expensive": sc.get("expensive").and_then(Value::as_bool).unwrap_or(false),
                        "presentationHint": sc.get("presentationHint"),
                        "namedVariables": sc.get("namedVariables"),
                        "indexedVariables": sc.get("indexedVariables"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(json!({ "scopes": scopes })))
}

/// A DAP `Variable` (or evaluate result) with long values cut and the session's
/// secret values masked (a program that read its token from the environment holds
/// it in a variable: the Variables view, watches, hover, "Ask agent" and
/// `debug_state` see it masked, like the console).
pub fn variable_view(s: &Session, v: &Value, value_key: &str) -> Value {
    json!({
        "name": v.get("name").and_then(Value::as_str).map(|n| s.redact(n)),
        "value": truncate(&s.redact(v.get(value_key).and_then(Value::as_str).unwrap_or("")), 16 * 1024),
        "type": v.get("type").and_then(Value::as_str).map(|t| truncate(&s.redact(t), 500)),
        "variablesReference": v.get("variablesReference").and_then(Value::as_i64).unwrap_or(0),
        "namedVariables": v.get("namedVariables"),
        "indexedVariables": v.get("indexedVariables"),
        "evaluateName": v.get("evaluateName").and_then(Value::as_str).map(|n| s.redact(n)),
        "presentationHint": v.get("presentationHint"),
        "memoryReference": v.get("memoryReference"),
    })
}

#[derive(Deserialize)]
struct VariablesQuery {
    #[serde(rename = "ref")]
    reference: i64,
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    count: Option<i64>,
    #[serde(default)]
    filter: Option<String>,
}

async fn variables(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>, Query(q): Query<VariablesQuery>) -> ApiResult<Json<Value>> {
    let s = session_of(&state, &pid, &sid)?;
    if q.reference <= 0 {
        return Err(ApiError::bad_request("invalid variables reference"));
    }
    let mut args = json!({ "variablesReference": q.reference });
    if let Some(f) = q.filter.as_deref().filter(|f| *f == "indexed" || *f == "named") {
        args["filter"] = json!(f);
    }
    if q.start.is_some() || q.count.is_some() {
        args["start"] = json!(q.start.unwrap_or(0).max(0));
        args["count"] = json!(q.count.unwrap_or(100).clamp(1, 1000));
    }
    let body = s.request("variables", args, EVAL_TIMEOUT).await?;
    let all = body.get("variables").and_then(Value::as_array).cloned().unwrap_or_default();
    let truncated = all.len() > 2000;
    let vars: Vec<Value> = all.iter().take(2000).map(|v| variable_view(&s, v, "value")).collect();
    Ok(Json(json!({ "variables": vars, "truncated": truncated })))
}

#[derive(Deserialize)]
struct SourceQuery {
    #[serde(rename = "ref")]
    reference: i64,
}

async fn source(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>, Query(q): Query<SourceQuery>) -> ApiResult<Json<Value>> {
    let s = session_of(&state, &pid, &sid)?;
    let body = s.request("source", json!({ "sourceReference": q.reference, "source": { "sourceReference": q.reference } }), REQUEST_TIMEOUT).await?;
    let content = body.get("content").and_then(Value::as_str).unwrap_or("");
    Ok(Json(json!({ "content": s.redact(&truncate(content, MAX_FILE as usize)), "mimeType": body.get("mimeType") })))
}

/// Largest file `file?path=` serves.
const MAX_FILE: u64 = 5 * 1024 * 1024;

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

/// Credential stores, by the name of a path component (on the host or in a dev
/// container).
const CREDENTIAL_NAMES: &[&str] = &[".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".netrc", ".git-credentials", ".pgpass", ".password-store", "keyrings"];

fn credential_path(path: &str) -> bool {
    crate::util::os::path::segments(path).any(|c| CREDENTIAL_NAMES.iter().any(|n| crate::util::os::path::same_name(c, n)))
}

/// Where Workbench never shows a file from, whatever a program's debug information
/// names as its source: Workbench's own configuration and data, secret files,
/// credentials.
fn private_file(state: &AppState, pid: &str, path: &std::path::Path) -> bool {
    if credential_path(&path.to_string_lossy()) {
        return true;
    }
    let home = crate::config::expand_tilde("~/");
    let mut roots: Vec<std::path::PathBuf> = vec![state.paths.config_dir.clone(), state.paths.data_dir.clone()];
    for d in [".config/gh", ".config/gcloud", ".config/workbench", ".local/share/workbench"] {
        roots.push(home.join(d));
    }
    let mut refs: Vec<crate::config::project::SecretRef> = state.config.read().secrets.values().cloned().collect();
    if let Some(p) = state.projects.get(pid) {
        refs.extend(p.config.secrets.values().cloned());
    }
    for r in &refs {
        if let crate::config::project::SecretRef::File(p) | crate::config::project::SecretRef::Dotenv { path: p, .. } = r {
            roots.push(crate::config::expand_tilde(p));
        }
    }
    use crate::util::os::path::{canonicalize, starts_with};
    roots.iter().any(|r| starts_with(path, r) || canonicalize(r).is_ok_and(|c| starts_with(path, &c)))
}

/// A file outside the project (a library header, a crate's source, the standard
/// library) that this session's frames or output named: read-only, text, at most
/// 5 MB, for the debugger's source view. Only paths the adapter reported are served
/// (and never Workbench's own files or credentials); agents use `debug_state`.
async fn file(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>, Query(q): Query<FileQuery>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    if !crate::util::os::path::is_absolute_str(&q.path) || !s.knows_source(&q.path) {
        return Err(ApiError::forbidden("only files this debug session's frames named can be shown; select the frame again"));
    }
    if credential_path(&q.path) {
        return Err(ApiError::forbidden("Workbench does not show this file"));
    }
    let bytes = match s.target() {
        Some(t) => {
            let limit = (MAX_FILE + 1).to_string();
            let args = ["/bin/sh", "-c", "test -f \"$2\" && head -c \"$1\" -- \"$2\"", "sh", limit.as_str(), q.path.as_str()];
            let out = crate::devcontainer::docker::exec(&t.docker, &t.container_id, t.user.as_deref(), &args, std::time::Duration::from_secs(20))
                .await
                .map_err(|e| ApiError::not_found(format!("cannot read {} in the dev container: {e}", q.path)))?;
            if !out.ok() {
                return Err(ApiError::not_found(format!("{} is not a file in the dev container", q.path)));
            }
            out.stdout.into_bytes()
        }
        None => {
            let path = std::path::PathBuf::from(&q.path);
            let (st, project) = (state.clone(), pid.clone());
            tokio::task::spawn_blocking(move || -> ApiResult<Vec<u8>> {
                let canon = crate::util::os::path::canonicalize(&path).map_err(|_| ApiError::not_found(format!("{} does not exist on this machine", path.display())))?;
                if private_file(&st, &project, &path) || private_file(&st, &project, &canon) {
                    return Err(ApiError::forbidden("Workbench does not show this file"));
                }
                let md = std::fs::metadata(&canon)?;
                if !md.is_file() {
                    return Err(ApiError::bad_request("not a file"));
                }
                if md.len() > MAX_FILE {
                    return Err(ApiError::bad_request(format!("the file is larger than {} MB", MAX_FILE / 1024 / 1024)));
                }
                Ok(std::fs::read(&canon)?)
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??
        }
    };
    if bytes.len() as u64 > MAX_FILE {
        return Err(ApiError::bad_request(format!("the file is larger than {} MB", MAX_FILE / 1024 / 1024)));
    }
    if bytes.iter().take(64 * 1024).any(|b| *b == 0) {
        return Err(ApiError::bad_request("a binary file"));
    }
    let content = String::from_utf8_lossy(&bytes);
    Ok(Json(json!({ "path": q.path, "content": s.redact(&content) })))
}

#[derive(Deserialize)]
struct OutputQuery {
    #[serde(default)]
    after: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn output(State(state): State<AppState>, Path((pid, sid)): Path<(String, String)>, Query(q): Query<OutputQuery>) -> ApiResult<Json<Value>> {
    let s = session_of(&state, &pid, &sid)?;
    let (lines, dropped) = s.output_after(q.after.unwrap_or(0), q.limit.unwrap_or(3000).clamp(1, 5000));
    Ok(Json(json!({ "lines": lines, "dropped": dropped, "seq": s.info().output_seq })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateBody {
    expression: String,
    #[serde(default)]
    frame_id: Option<i64>,
    #[serde(default)]
    context: Option<String>,
}

async fn evaluate(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>, Json(b): Json<EvaluateBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    let expr = b.expression.trim_end().to_string();
    if expr.trim().is_empty() || expr.len() > 10_000 || expr.contains('\0') {
        return Err(ApiError::bad_request("an expression of up to 10000 characters"));
    }
    let context = match b.context.as_deref() {
        Some(c @ ("watch" | "repl" | "hover" | "clipboard")) => c,
        _ => "repl",
    };
    let mut args = json!({ "expression": expr, "context": context });
    if let Some(f) = b.frame_id {
        args["frameId"] = json!(f);
    }
    let repl = context == "repl";
    if repl {
        s.log("repl-in", format!("> {expr}\n"), None);
    }
    let r = s.evaluate(&expr, args, EVAL_TIMEOUT).await;
    if repl {
        match &r {
            Ok(v) => {
                let text = v.get("result").and_then(Value::as_str).unwrap_or("");
                if !text.is_empty() {
                    s.log("repl-out", format!("{}\n", text.trim_end_matches('\n')), None);
                }
            }
            Err(e) => s.log("repl-err", format!("{}\n", e.message), None),
        }
        s.flush(&state);
    }
    Ok(Json(variable_view(&s, &r?, "result")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetVariableBody {
    variables_reference: i64,
    name: String,
    value: String,
}

async fn set_variable(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>, Json(b): Json<SetVariableBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    if s.capabilities().get("supportsSetVariable").and_then(Value::as_bool) != Some(true) {
        return Err(ApiError::bad_request(format!("{} cannot change variables", s.adapter.label)));
    }
    if b.value.len() > 10_000 {
        return Err(ApiError::bad_request("value too long"));
    }
    let body = s
        .request("setVariable", json!({ "variablesReference": b.variables_reference, "name": b.name, "value": b.value }), EVAL_TIMEOUT)
        .await?;
    let mut v = variable_view(&s, &body, "value");
    v["name"] = json!(b.name);
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionsBody {
    text: String,
    column: i64,
    #[serde(default)]
    frame_id: Option<i64>,
}

async fn completions(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>, Json(b): Json<CompletionsBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    if s.capabilities().get("supportsCompletionsRequest").and_then(Value::as_bool) != Some(true) || b.text.len() > 10_000 {
        return Ok(Json(json!({ "targets": [] })));
    }
    let mut args = json!({ "text": b.text, "column": b.column.max(1) });
    if let Some(f) = b.frame_id {
        args["frameId"] = json!(f);
    }
    let body = s.request("completions", args, REQUEST_TIMEOUT).await?;
    let targets: Vec<Value> = body
        .get("targets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(200)
                .map(|t| {
                    let mut t = t.clone();
                    for k in ["label", "text", "detail"] {
                        if let Some(v) = t.get(k).and_then(Value::as_str).map(|v| s.redact(v)) {
                            t[k] = json!(v);
                        }
                    }
                    t
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(json!({ "targets": targets })))
}

// ---------------------------------------------------------------- breakpoints

async fn breakpoints_get(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    state.projects.require(&pid)?;
    Ok(Json(session::breakpoints_view(&state, &pid)))
}

#[derive(Deserialize)]
struct FileBody {
    path: String,
    breakpoints: Vec<LineBreakpoint>,
}

async fn breakpoints_file(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<FileBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    state.projects.require(&pid)?;
    let path = super::breakpoints::normalize_path(&b.path)?;
    state.debug.store.update(&state.paths.data_dir, &pid, |p| p.set_file(&path, b.breakpoints)).await?;
    session::breakpoints_changed(&state, &pid, Lines::File(&path), false, false).await;
    Ok(Json(session::breakpoints_view(&state, &pid)))
}

#[derive(Deserialize)]
struct FunctionsBody {
    breakpoints: Vec<FunctionBreakpoint>,
}

async fn breakpoints_functions(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<FunctionsBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    state.projects.require(&pid)?;
    state.debug.store.update(&state.paths.data_dir, &pid, |p| p.set_functions(b.breakpoints)).await?;
    session::breakpoints_changed(&state, &pid, Lines::None, true, false).await;
    Ok(Json(session::breakpoints_view(&state, &pid)))
}

#[derive(Deserialize)]
struct ExceptionsBody {
    adapter: String,
    filters: Vec<String>,
}

async fn breakpoints_exceptions(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<ExceptionsBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    state.projects.require(&pid)?;
    if !adapters::valid_id(&b.adapter) || b.filters.len() > 50 || b.filters.iter().any(|f| f.is_empty() || f.len() > 200) {
        return Err(ApiError::bad_request("invalid exception filters"));
    }
    state
        .debug
        .store
        .update(&state.paths.data_dir, &pid, |p| {
            p.exception_filters.insert(b.adapter.clone(), b.filters);
            Ok(())
        })
        .await?;
    session::breakpoints_changed(&state, &pid, Lines::None, false, true).await;
    Ok(Json(session::breakpoints_view(&state, &pid)))
}

#[derive(Deserialize)]
struct MuteBody {
    muted: bool,
}

async fn breakpoints_mute(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<MuteBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    state.projects.require(&pid)?;
    state
        .debug
        .store
        .update(&state.paths.data_dir, &pid, |p| {
            p.muted = b.muted;
            Ok(())
        })
        .await?;
    session::breakpoints_changed(&state, &pid, Lines::All, true, true).await;
    Ok(Json(session::breakpoints_view(&state, &pid)))
}

async fn breakpoints_clear(State(state): State<AppState>, caller: C, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    state.projects.require(&pid)?;
    state
        .debug
        .store
        .update(&state.paths.data_dir, &pid, |p| {
            p.breakpoints.clear();
            p.function_breakpoints.clear();
            Ok(())
        })
        .await?;
    session::breakpoints_changed(&state, &pid, Lines::All, true, false).await;
    Ok(Json(session::breakpoints_view(&state, &pid)))
}

#[derive(Deserialize)]
struct WatchesBody {
    expressions: Vec<String>,
}

async fn watches(State(state): State<AppState>, caller: C, Path(pid): Path<String>, Json(b): Json<WatchesBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    state.projects.require(&pid)?;
    let p = state.debug.store.update(&state.paths.data_dir, &pid, |p| p.set_watches(b.expressions)).await?;
    session::emit_breakpoints(&state, &pid);
    Ok(Json(json!({ "watches": p.watches })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_stores_are_never_source_files() {
        assert!(credential_path("/home/u/.ssh/id_ed25519"));
        assert!(credential_path("/root/.aws/credentials"));
        assert!(credential_path("/home/vscode/.netrc"));
        assert!(credential_path("/home/u/.local/share/keyrings/login.keyring"));
        assert!(!credential_path("/usr/include/c++/14/bits/stl_vector.h"));
        assert!(!credential_path("/home/u/.cargo/registry/src/index.crates.io-1/serde-1.0.0/src/lib.rs"));
        assert!(!credential_path("/home/u/src/sshd.c"));
    }
}

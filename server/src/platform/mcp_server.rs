//! `/mcp` — the Workbench MCP server for hosted Claude sessions.
//!
//! Streamable HTTP, stateless, plain JSON responses (never SSE). The server is
//! "dual-era" in the terms of the MCP 2026-07-28 revision:
//!
//! * **Legacy** clients (revisions 2024-11-05 … 2025-11-25) open with
//!   `initialize`. We negotiate the version (echo the client's when supported,
//!   otherwise offer our newest legacy one), mint no session, and serve every
//!   later request statelessly. Single messages and JSON-RPC batches are accepted.
//! * **Modern** clients (2026-07-28) put the version and client capabilities in
//!   every request's `params._meta` and mirror `method`/`params.name` into the
//!   `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers, which must match
//!   the body. They may call `server/discover`.
//!
//! Tool failures are *results* with `isError: true` (the model can react);
//! malformed requests are JSON-RPC errors. A tool's pictures (`ToolOutput::Images`)
//! follow its text as `image` content blocks (base64 PNG).
//!
//! Auth: an agent token (`WORKBENCH_AGENT_TOKEN`, bound to one terminal) or the
//! master token as `Authorization: Bearer`. With the master token a caller may
//! name its terminal (`X-Workbench-Terminal`) or project (`X-Workbench-Project`).

use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Map, Value, json};

use crate::app::AppState;
use crate::mcp::{McpCtx, McpTool, ToolOutput};

use super::activity::{McpCallRecord, summarize_args};

/// Per-request-metadata revisions we serve.
pub const MODERN_VERSIONS: &[&str] = &["2026-07-28"];
/// `initialize`-handshake revisions we serve, newest first.
pub const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// Upper bound for one tool call; the client usually gives up earlier.
const MAX_TOOL_TIME: Duration = Duration::from_secs(15 * 60);
/// Tool output is cut beyond this many characters.
const MAX_OUTPUT_CHARS: usize = 400_000;
const MAX_BATCH: usize = 64;
/// How long clients may cache `tools/list` (modern revision).
const LIST_TTL_MS: u64 = 300_000;

const INSTRUCTIONS: &str = "Workbench is the developer workspace hosting this agent session. \
Its tools act on the user's open Workbench UI and its integrations: open files, diffs and apps in the UI, \
notify the user, read CI pipelines and logs, merge requests, Confluence pages and Jira issues, and check \
environment health and run output. Prefer your own shell for plain file and git work; use these tools for \
anything that needs Workbench's UI or credentials. Tools marked destructive change remote state.";

// ---------------------------------------------------------------- replies

/// What one JSON-RPC message produced: an HTTP status and an optional body.
#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub body: Option<Value>,
}

impl Reply {
    fn accepted() -> Self {
        Self { status: StatusCode::ACCEPTED, body: None }
    }
    fn result(id: &Value, result: Value) -> Self {
        Self { status: StatusCode::OK, body: Some(json!({ "jsonrpc": "2.0", "id": id, "result": result })) }
    }
    fn error(status: StatusCode, id: &Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        let mut err = json!({ "code": code, "message": message.into() });
        if let Some(d) = data {
            err["data"] = d;
        }
        Self { status, body: Some(json!({ "jsonrpc": "2.0", "id": id, "error": err })) }
    }
}

fn json_response(status: StatusCode, body: Option<Value>) -> Response {
    match body {
        None => status.into_response(),
        Some(v) => {
            let mut resp = (status, v.to_string()).into_response();
            resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            resp
        }
    }
}

/// `GET`/`DELETE /mcp`: no standalone SSE stream and no sessions.
pub async fn method_not_allowed() -> Response {
    let mut resp = json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        Some(json!({ "jsonrpc": "2.0", "error": { "code": INVALID_REQUEST, "message": "Workbench's MCP endpoint accepts POST only" } })),
    );
    resp.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    resp
}

// ---------------------------------------------------------------- caller

/// Who is calling, resolved from the request headers.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    pub terminal_id: Option<String>,
    pub project_hint: Option<String>,
}

fn authenticate(state: &AppState, headers: &HeaderMap) -> Option<Caller> {
    if let Some(tid) = state.auth.agent_from_headers(headers) {
        return Some(Caller { terminal_id: Some(tid), project_hint: None });
    }
    if state.auth.is_master_bearer(headers) {
        let h = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 200)
                .map(str::to_string)
        };
        return Some(Caller { terminal_id: h("x-workbench-terminal"), project_hint: h("x-workbench-project") });
    }
    None
}

impl Caller {
    fn ctx(&self, state: &AppState) -> McpCtx {
        let info = self.terminal_id.as_deref().and_then(|t| state.terminals.info(t));
        let project_id = info
            .as_ref()
            .and_then(|i| i.project_id.clone())
            .or_else(|| self.project_hint.clone().filter(|p| state.projects.get(p).is_some()));
        McpCtx { terminal_id: self.terminal_id.clone(), project_id }
    }

    fn session_title(&self, state: &AppState) -> Option<String> {
        session_title(state.terminals.info(self.terminal_id.as_deref()?)?)
    }
}

/// The session's tab title: the name its attention rows, card and tab show. It
/// follows Claude's automatic title unless the user renamed the session;
/// `agent.title` does not (a later automatic title overwrites a rename there).
pub(super) fn session_title(info: crate::terminals::TerminalInfo) -> Option<String> {
    if !info.title.trim().is_empty() {
        return Some(info.title);
    }
    info.agent.and_then(|a| a.title).filter(|t| !t.trim().is_empty())
}

/// DNS-rebinding defence required by the transport: a present `Origin` must be
/// this server. (The router's guard already pins `Host`.)
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    origin.split("://").nth(1).is_some_and(|rest| rest.trim_end_matches('/') == host)
}

// ---------------------------------------------------------------- headers

/// The request-metadata headers of the modern Streamable HTTP binding.
#[derive(Debug, Default, Clone)]
pub struct RpcHeaders {
    pub protocol_version: Option<String>,
    pub method: Option<String>,
    /// Decoded `Mcp-Name`; `Some(None)` when present but undecodable.
    pub name: Option<Option<String>>,
}

impl RpcHeaders {
    pub fn from_headers(h: &HeaderMap) -> Self {
        let get = |n: &str| h.get(n).map(|v| v.to_str().map(|s| s.trim().to_string()).ok());
        Self {
            protocol_version: get("mcp-protocol-version").flatten(),
            method: get("mcp-method").flatten(),
            name: get("mcp-name").map(|v| v.and_then(|s| decode_header_value(&s))),
        }
    }
}

/// Undo the `=?base64?…?=` sentinel encoding used for non-ASCII header values.
pub fn decode_header_value(v: &str) -> Option<String> {
    match v.strip_prefix("=?base64?").and_then(|r| r.strip_suffix("?=")) {
        Some(b64) => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
            String::from_utf8(bytes).ok()
        }
        None => Some(v.to_string()),
    }
}

// ---------------------------------------------------------------- entry point

/// `POST /mcp`.
pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(caller) = authenticate(&state, &headers) else {
        return json_response(
            StatusCode::UNAUTHORIZED,
            Some(json!({ "jsonrpc": "2.0", "error": { "code": INVALID_REQUEST, "message": "Workbench MCP needs an agent token or the Workbench token as a Bearer token" } })),
        );
    };
    if !origin_ok(&headers) {
        return json_response(
            StatusCode::FORBIDDEN,
            Some(json!({ "jsonrpc": "2.0", "error": { "code": INVALID_REQUEST, "message": "cross-origin request refused" } })),
        );
    }
    let rpc_headers = RpcHeaders::from_headers(&headers);
    let message: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            let r = Reply::error(StatusCode::BAD_REQUEST, &Value::Null, PARSE_ERROR, format!("Parse error: {e}"), None);
            return json_response(r.status, r.body);
        }
    };
    let reply = match message {
        Value::Array(items) => handle_batch(&state, &caller, &rpc_headers, items).await,
        msg => handle_message(&state, &caller, &rpc_headers, msg).await,
    };
    json_response(reply.status, reply.body)
}

async fn handle_batch(state: &AppState, caller: &Caller, headers: &RpcHeaders, items: Vec<Value>) -> Reply {
    if items.is_empty() {
        return Reply::error(StatusCode::BAD_REQUEST, &Value::Null, INVALID_REQUEST, "empty batch", None);
    }
    if items.len() > MAX_BATCH {
        return Reply::error(StatusCode::BAD_REQUEST, &Value::Null, INVALID_REQUEST, format!("batch larger than {MAX_BATCH}"), None);
    }
    let mut out = vec![];
    // Sequential on purpose: batches are small and legacy-only.
    for item in items {
        let r = Box::pin(handle_message(state, caller, headers, item)).await;
        if let Some(b) = r.body {
            out.push(b);
        }
    }
    if out.is_empty() { Reply::accepted() } else { Reply { status: StatusCode::OK, body: Some(Value::Array(out)) } }
}

/// Handle one JSON-RPC message (request, notification or stray response).
pub async fn handle_message(state: &AppState, caller: &Caller, headers: &RpcHeaders, msg: Value) -> Reply {
    let Value::Object(mut obj) = msg else {
        return Reply::error(StatusCode::BAD_REQUEST, &Value::Null, INVALID_REQUEST, "expected a JSON-RPC object", None);
    };
    let id = obj.get("id").cloned();
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Reply::error(StatusCode::BAD_REQUEST, id.as_ref().unwrap_or(&Value::Null), INVALID_REQUEST, "jsonrpc must be \"2.0\"", None);
    }
    let Some(method) = obj.get("method").and_then(Value::as_str).map(str::to_string) else {
        // A response to a server request (we send none) — acknowledge and ignore.
        if obj.contains_key("result") || obj.contains_key("error") {
            return Reply::accepted();
        }
        return Reply::error(StatusCode::BAD_REQUEST, id.as_ref().unwrap_or(&Value::Null), INVALID_REQUEST, "missing method", None);
    };
    let Some(id) = id else {
        // Notifications (initialized, cancelled, roots/list_changed…) need no action:
        // we are stateless and tool calls are cancelled by dropping the connection.
        tracing::debug!(method, "mcp notification");
        return Reply::accepted();
    };
    if !(id.is_string() || id.is_i64() || id.is_u64()) {
        return Reply::error(StatusCode::BAD_REQUEST, &Value::Null, INVALID_REQUEST, "id must be a string or an integer", None);
    }
    let params = match obj.remove("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Reply::error(StatusCode::OK, &id, INVALID_PARAMS, "params must be an object", None),
    };

    // `initialize` always selects the legacy handshake semantics.
    if method == "initialize" {
        return initialize(&id, &params);
    }
    let meta_version = params
        .get("_meta")
        .and_then(|m| m.get(META_VERSION))
        .and_then(Value::as_str)
        .map(str::to_string);
    match meta_version {
        Some(version) => modern(state, caller, headers, &id, &method, params, &version).await,
        None => {
            if let Some(h) = headers.protocol_version.as_deref() {
                if MODERN_VERSIONS.contains(&h) {
                    return Reply::error(
                        StatusCode::BAD_REQUEST,
                        &id,
                        INVALID_PARAMS,
                        format!("MCP-Protocol-Version {h} requires params._meta[\"{META_VERSION}\"]"),
                        None,
                    );
                }
                if !LEGACY_VERSIONS.contains(&h) {
                    return unsupported_version(&id, h);
                }
            }
            tracing::debug!(method, version = headers.protocol_version.as_deref().unwrap_or("-"), "mcp legacy request");
            legacy(state, caller, &id, &method, params).await
        }
    }
}

fn all_versions() -> Vec<&'static str> {
    MODERN_VERSIONS.iter().chain(LEGACY_VERSIONS).copied().collect()
}

fn unsupported_version(id: &Value, requested: &str) -> Reply {
    Reply::error(
        StatusCode::BAD_REQUEST,
        id,
        UNSUPPORTED_VERSION,
        "Unsupported protocol version",
        Some(json!({ "supported": all_versions(), "requested": requested })),
    )
}

fn server_info() -> Value {
    json!({ "name": "workbench", "title": "Workbench", "version": env!("CARGO_PKG_VERSION") })
}

fn capabilities() -> Value {
    json!({ "tools": { "listChanged": false } })
}

/// The version we answer a legacy `initialize` with.
pub fn negotiate_legacy(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| LEGACY_VERSIONS.iter().find(|v| **v == r).copied())
        .unwrap_or(LEGACY_VERSIONS[0])
}

fn initialize(id: &Value, params: &Map<String, Value>) -> Reply {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = negotiate_legacy(requested);
    let client = params.get("clientInfo").and_then(|c| c.get("name")).and_then(Value::as_str).unwrap_or("?");
    tracing::debug!(requested = requested.unwrap_or("-"), version, client, "mcp initialize");
    Reply::result(
        id,
        json!({
            "protocolVersion": version,
            "capabilities": capabilities(),
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        }),
    )
}

async fn legacy(state: &AppState, caller: &Caller, id: &Value, method: &str, params: Map<String, Value>) -> Reply {
    match method {
        "ping" => Reply::result(id, json!({})),
        "tools/list" => Reply::result(id, json!({ "tools": tool_list(state) })),
        "tools/call" => match call_tool(state, caller, params).await {
            Ok(result) => Reply::result(id, result),
            Err((code, msg)) => Reply::error(StatusCode::OK, id, code, msg, None),
        },
        _ => Reply::error(StatusCode::OK, id, METHOD_NOT_FOUND, format!("Method not found: {method}"), None),
    }
}

/// The 2026-07-28 revision: validate the mirrored headers and per-request
/// metadata, then serve statelessly.
async fn modern(
    state: &AppState,
    caller: &Caller,
    headers: &RpcHeaders,
    id: &Value,
    method: &str,
    params: Map<String, Value>,
    version: &str,
) -> Reply {
    let mismatch = |msg: String| Reply::error(StatusCode::BAD_REQUEST, id, HEADER_MISMATCH, format!("Header mismatch: {msg}"), None);
    match headers.protocol_version.as_deref() {
        None => return mismatch("missing MCP-Protocol-Version header".into()),
        Some(h) if h != version => {
            return mismatch(format!("MCP-Protocol-Version header value '{h}' does not match body value '{version}'"));
        }
        _ => {}
    }
    match headers.method.as_deref() {
        None => return mismatch("missing Mcp-Method header".into()),
        Some(h) if h != method => return mismatch(format!("Mcp-Method header value '{h}' does not match body value '{method}'")),
        _ => {}
    }
    if matches!(method, "tools/call" | "resources/read" | "prompts/get") {
        let body_name = params.get("name").or_else(|| params.get("uri")).and_then(Value::as_str).unwrap_or("");
        match &headers.name {
            None => return mismatch("missing Mcp-Name header".into()),
            Some(None) => return mismatch("Mcp-Name header is not valid base64 UTF-8".into()),
            Some(Some(h)) if h != body_name => {
                return mismatch(format!("Mcp-Name header value '{h}' does not match body value '{body_name}'"));
            }
            _ => {}
        }
    }
    if !MODERN_VERSIONS.contains(&version) {
        return unsupported_version(id, version);
    }
    let caps_ok = params.get("_meta").and_then(|m| m.get(META_CLIENT_CAPS)).is_some_and(Value::is_object);
    if !caps_ok {
        return Reply::error(
            StatusCode::BAD_REQUEST,
            id,
            INVALID_PARAMS,
            format!("params._meta[\"{META_CLIENT_CAPS}\"] is required"),
            None,
        );
    }
    tracing::debug!(method, version, "mcp modern request");
    let with_meta = |mut v: Value| {
        v["resultType"] = json!("complete");
        v["_meta"] = json!({ META_SERVER_INFO: server_info() });
        v
    };
    match method {
        "server/discover" => Reply::result(
            id,
            with_meta(json!({
                "supportedVersions": all_versions(),
                "capabilities": capabilities(),
                "instructions": INSTRUCTIONS,
                "ttlMs": LIST_TTL_MS,
                "cacheScope": "private",
            })),
        ),
        "tools/list" => Reply::result(
            id,
            with_meta(json!({ "tools": tool_list(state), "ttlMs": LIST_TTL_MS, "cacheScope": "private" })),
        ),
        "tools/call" => match call_tool(state, caller, params).await {
            Ok(result) => Reply::result(id, with_meta(result)),
            Err((code, msg)) => Reply::error(StatusCode::OK, id, code, msg, None),
        },
        // Removed in 2026-07-28, but answering costs nothing.
        "ping" => Reply::result(id, with_meta(json!({}))),
        _ => Reply::error(StatusCode::NOT_FOUND, id, METHOD_NOT_FOUND, format!("Method not found: {method}"), None),
    }
}

// ---------------------------------------------------------------- tools

/// `tools/list` entries (deterministic order: as the slices declare them).
pub fn tool_list(state: &AppState) -> Vec<Value> {
    state.platform.tools().iter().map(tool_json).collect()
}

fn tool_json(t: &McpTool) -> Value {
    let schema = if t.input_schema.is_object() { t.input_schema.clone() } else { json!({ "type": "object" }) };
    json!({
        "name": t.name,
        "description": t.description,
        "inputSchema": schema,
        "annotations": {
            // A tool that acts (`McpTool::acts`) is neither: it does something for the user
            // to see, so it is not read-only, and it changes no remote state.
            "readOnlyHint": !t.mutating && !t.acts,
            "destructiveHint": t.mutating,
        },
    })
}

/// Cancels the spawned tool task if the request future is dropped (the client
/// disconnected), so abandoned calls stop working.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// How much of a text argument is looked at for what to mask before it is summarised. A summary
/// shows the first characters of a text's first line, which lie well inside this many bytes, so
/// a page body of megabytes costs no more than a short line.
const SUMMARY_LOOK: usize = 16 * 1024;

/// The one-line summary of a call's arguments for the Activity view and the `mcp.call` event.
///
/// A session's call is summarised with what that session must not show masked, as its permission
/// details are (`Terminals::redact_text`: what looks like a credential, and the secret values the
/// session was started with). The texts are masked before the summary cuts them, so a cut never
/// leaves the beginning of a secret standing, and the summary passes once more as a whole (an
/// argument's name and its value together can be the credential). A caller that is no session
/// (the master token without a terminal) has no secrets of a session to mask.
fn summary_of(state: &AppState, terminal_id: Option<&str>, args: &mut Map<String, Value>) -> String {
    let Some(tid) = terminal_id else { return summarize_args(args) };
    let masked = |text: &str| {
        if text.len() <= SUMMARY_LOOK {
            return state.terminals.redact_text(tid, text);
        }
        let cut = (0..=SUMMARY_LOOK).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
        format!("{}{}", state.terminals.redact_text(tid, &text[..cut]), &text[cut..])
    };
    // In place, and put back afterwards: the arguments go to the tool as they came, and none
    // but the texts is copied.
    let mut texts = vec![];
    for value in args.values_mut() {
        if let Value::String(text) = value {
            let shown = masked(text);
            texts.push(std::mem::replace(text, shown));
        }
    }
    let summary = summarize_args(args);
    let mut texts = texts.into_iter();
    for value in args.values_mut() {
        if let Value::String(text) = value {
            if let Some(original) = texts.next() {
                *text = original;
            }
        }
    }
    state.terminals.redact_text(tid, &summary)
}

/// Run `tools/call`. `Err` is a protocol error (unknown tool, bad params);
/// tool failures are `Ok` results with `isError: true`.
async fn call_tool(state: &AppState, caller: &Caller, params: Map<String, Value>) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "tools/call needs params.name".to_string()))?
        .to_string();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err((INVALID_PARAMS, "arguments must be an object".into())),
    };
    let tools = state.platform.tools();
    let tool = tools.iter().find(|t| t.name == name).ok_or((INVALID_PARAMS, format!("Unknown tool: {name}")))?;
    Ok(run_tool(state, caller, tool, args).await)
}

/// One call of a tool: its answer as the result of `tools/call` (the text, then its pictures),
/// and the Activity record and `mcp.call` event made from its arguments and its error.
async fn run_tool(state: &AppState, caller: &Caller, tool: &McpTool, mut args: Map<String, Value>) -> Value {
    let name = tool.name.clone();
    let ctx = caller.ctx(state);
    let summary = summary_of(state, ctx.terminal_id.as_deref(), &mut args);
    let started = Instant::now();
    let task = tokio::spawn((tool.handler)(state.clone(), ctx.clone(), Value::Object(args)));
    let _abort = AbortOnDrop(task.abort_handle());
    // Pictures go to the session after the text; everything else here works from the text.
    let mut images: Vec<Vec<u8>> = vec![];
    let outcome: Result<String, String> = match tokio::time::timeout(MAX_TOOL_TIME, task).await {
        Ok(Ok(Ok(ToolOutput::Text(t)))) => Ok(t),
        Ok(Ok(Ok(ToolOutput::Json(v)))) => Ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())),
        Ok(Ok(Ok(ToolOutput::Images { text, png }))) => {
            images = png;
            Ok(text)
        }
        Ok(Ok(Err(e))) => Err(e.message),
        Ok(Err(join)) if join.is_panic() => {
            tracing::error!(tool = name, "mcp tool panicked");
            Err("internal error: the tool crashed".into())
        }
        Ok(Err(_)) => Err("the tool was cancelled".into()),
        Err(_) => Err(format!("the tool did not finish within {} minutes", MAX_TOOL_TIME.as_secs() / 60)),
    };
    let ms = started.elapsed().as_millis() as u64;

    let ok = outcome.is_ok();
    if ok && tool.quiet {
        // A quiet tool (`McpTool::quiet`) is called often with little to tell: what
        // succeeded stays out of the Activity ring, so it does not push the rest out.
        tracing::debug!(tool = name, ok, ms, terminal = ctx.terminal_id.as_deref().unwrap_or("-"), "mcp tool call");
    } else {
        // A refusal may quote what the session passed: it is masked like the summary, before it is
        // cut. The session is answered with it as it was (below): the value is its own.
        let error = outcome.as_ref().err().map(|e| {
            let shown = match ctx.terminal_id.as_deref() {
                Some(tid) => state.terminals.redact_text(tid, e),
                None => e.clone(),
            };
            super::truncate_chars(&shown, 300)
        });
        let rec = McpCallRecord {
            id: state.platform.activity.next_id(),
            at: crate::util::now_ms(),
            terminal_id: ctx.terminal_id.clone(),
            project_id: ctx.project_id.clone(),
            session: caller.session_title(state),
            tool: name.clone(),
            ok,
            mutating: tool.mutating,
            ms,
            summary,
            error,
        };
        state.platform.activity.push_call(rec.clone());
        state.events.emit("mcp.call", ctx.project_id.as_deref(), &rec);
        tracing::info!(tool = name, ok, ms, terminal = ctx.terminal_id.as_deref().unwrap_or("-"), "mcp tool call");
    }

    let (text, is_error) = match outcome {
        Ok(t) => (t, false),
        Err(e) => (e, true),
    };
    let mut content = vec![json!({ "type": "text", "text": cap_output(text) })];
    content.extend(images.iter().map(|png| json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(png), "mimeType": "image/png" })));
    json!({ "content": content, "isError": is_error })
}

fn cap_output(text: String) -> String {
    let n = text.chars().count();
    if n <= MAX_OUTPUT_CHARS {
        return text;
    }
    let mut out: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    out.push_str(&format!("\n… [output truncated: {} more characters]", n - MAX_OUTPUT_CHARS));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::testutil;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn negotiates_legacy_versions() {
        assert_eq!(negotiate_legacy(Some("2024-11-05")), "2024-11-05");
        assert_eq!(negotiate_legacy(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate_legacy(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate_legacy(Some("2025-11-25")), "2025-11-25");
        assert_eq!(negotiate_legacy(Some("1999-01-01")), "2025-11-25");
        assert_eq!(negotiate_legacy(None), "2025-11-25");
    }

    #[test]
    fn decodes_base64_sentinel_headers() {
        assert_eq!(decode_header_value("workbench_notify").as_deref(), Some("workbench_notify"));
        assert_eq!(decode_header_value("=?base64?SGVsbG8sIOS4lueVjA==?=").as_deref(), Some("Hello, 世界"));
        assert_eq!(decode_header_value("=?base64?!!!?="), None);
    }

    struct Req<'a> {
        token: Option<&'a str>,
        headers: Vec<(&'a str, String)>,
        body: String,
    }

    async fn send(app: &testutil::TestApp, r: Req<'_>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "127.0.0.1:7999")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(t) = r.token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        for (k, v) in r.headers {
            b = b.header(k, v);
        }
        let mut req = b.body(Body::from(r.body)).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 5555))));
        let resp = app.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        (status, v)
    }

    fn master(app: &testutil::TestApp) -> String {
        app.state.auth.master_token().to_string()
    }

    #[tokio::test]
    async fn legacy_handshake_list_and_call() {
        let app = testutil::app().await;
        let tok = master(&app);
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(v["result"]["serverInfo"]["name"], "workbench");
        assert_eq!(v["result"]["capabilities"]["tools"]["listChanged"], false);
        // Every hosted CLI that is given the tools reads this, not Claude Code alone.
        let instructions = v["result"]["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("Workbench is the developer workspace hosting this agent session. ") && !instructions.contains("Claude"), "{instructions}");

        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![("mcp-protocol-version", "2025-06-18".into())],
            body: json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::ACCEPTED);
        assert_eq!(v, Value::Null);

        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![("mcp-protocol-version", "2025-06-18".into())],
            body: json!({"jsonrpc":"2.0","id":"a","method":"tools/list"}).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::OK);
        let tools = v["result"]["tools"].as_array().unwrap();
        let notify = tools.iter().find(|t| t["name"] == "workbench_notify").unwrap();
        assert_eq!(notify["annotations"]["readOnlyHint"], true);
        assert!(notify["inputSchema"]["properties"]["message"].is_object());
        assert!(v["result"].get("resultType").is_none(), "legacy results stay legacy-shaped");

        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"workbench_projects","arguments":{}}}).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["result"]["isError"], false, "{v}");
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"count\": 0"), "{text}");

        // The call was recorded for the activity view.
        let calls = app.state.platform.activity.calls(10);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "workbench_projects");
        assert!(calls[0].ok);
    }

    #[tokio::test]
    async fn tool_errors_are_results_and_protocol_errors_are_errors() {
        let app = testutil::app().await;
        let tok = master(&app);
        // Invalid arguments → tool error result.
        let (_, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"workbench_open_url","arguments":{"url":"javascript:alert(1)"}}}).to_string(),
        })
        .await;
        assert_eq!(v["result"]["isError"], true, "{v}");
        // Unknown tool → JSON-RPC error.
        let (_, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}).to_string(),
        })
        .await;
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
        // Unknown method.
        let (_, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!({"jsonrpc":"2.0","id":5,"method":"resources/list"}).to_string(),
        })
        .await;
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        // Parse error.
        let (s, v) = send(&app, Req { token: Some(&tok), headers: vec![], body: "{not json".into() }).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], PARSE_ERROR);
        // Missing jsonrpc.
        let (s, _) = send(&app, Req { token: Some(&tok), headers: vec![], body: json!({"id":1,"method":"ping"}).to_string() }).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Unsupported legacy header version.
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![("mcp-protocol-version", "2023-01-01".into())],
            body: json!({"jsonrpc":"2.0","id":6,"method":"tools/list"}).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], UNSUPPORTED_VERSION);
    }

    #[tokio::test]
    async fn batches_mix_requests_and_notifications() {
        let app = testutil::app().await;
        let tok = master(&app);
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!([
                {"jsonrpc":"2.0","id":1,"method":"ping"},
                {"jsonrpc":"2.0","method":"notifications/initialized"},
                {"jsonrpc":"2.0","id":2,"method":"tools/list"}
            ])
            .to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::OK);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[1]["id"], 2);
        let (s, _) = send(&app, Req {
            token: Some(&tok),
            headers: vec![],
            body: json!([{"jsonrpc":"2.0","method":"notifications/initialized"}]).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::ACCEPTED);
        let (s, _) = send(&app, Req { token: Some(&tok), headers: vec![], body: "[]".into() }).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    fn modern_body(method: &str, params: Value) -> String {
        let mut p = params;
        p["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        json!({"jsonrpc":"2.0","id":7,"method":method,"params":p}).to_string()
    }

    #[tokio::test]
    async fn modern_requests_validate_headers() {
        let app = testutil::app().await;
        let tok = master(&app);
        let hdrs = |m: &str| vec![("mcp-protocol-version", "2026-07-28".to_string()), ("mcp-method", m.to_string())];

        let (s, v) = send(&app, Req { token: Some(&tok), headers: hdrs("server/discover"), body: modern_body("server/discover", json!({})) }).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["result"]["resultType"], "complete");
        assert!(v["result"]["supportedVersions"].as_array().unwrap().contains(&json!("2026-07-28")));
        assert_eq!(v["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "workbench");

        let (_, v) = send(&app, Req { token: Some(&tok), headers: hdrs("tools/list"), body: modern_body("tools/list", json!({})) }).await;
        assert_eq!(v["result"]["cacheScope"], "private");
        assert!(v["result"]["ttlMs"].as_u64().unwrap() > 0);

        // tools/call needs Mcp-Name matching params.name.
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: hdrs("tools/call"),
            body: modern_body("tools/call", json!({"name":"workbench_projects","arguments":{}})),
        })
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], HEADER_MISMATCH);
        let mut h = hdrs("tools/call");
        h.push(("mcp-name", "workbench_projects".into()));
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: h,
            body: modern_body("tools/call", json!({"name":"workbench_projects","arguments":{}})),
        })
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["result"]["isError"], false);
        assert_eq!(v["result"]["resultType"], "complete");

        // Method header must match the body.
        let (s, v) = send(&app, Req { token: Some(&tok), headers: hdrs("tools/call"), body: modern_body("tools/list", json!({})) }).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], HEADER_MISMATCH);

        // Unknown modern version.
        let body = json!({"jsonrpc":"2.0","id":8,"method":"tools/list","params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion":"2031-01-01",
            "io.modelcontextprotocol/clientCapabilities":{}}}})
        .to_string();
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: vec![("mcp-protocol-version", "2031-01-01".into()), ("mcp-method", "tools/list".into())],
            body,
        })
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], UNSUPPORTED_VERSION);
        assert_eq!(v["error"]["data"]["requested"], "2031-01-01");

        // Unknown modern method → 404.
        let (s, v) = send(&app, Req { token: Some(&tok), headers: hdrs("prompts/list"), body: modern_body("prompts/list", json!({})) }).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);

        // Modern header without _meta is malformed.
        let (s, v) = send(&app, Req {
            token: Some(&tok),
            headers: hdrs("tools/list"),
            body: json!({"jsonrpc":"2.0","id":9,"method":"tools/list"}).to_string(),
        })
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn auth_and_origin_are_enforced() {
        let app = testutil::app().await;
        let body = json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string();
        let (s, _) = send(&app, Req { token: None, headers: vec![], body: body.clone() }).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = send(&app, Req { token: Some("wrong"), headers: vec![], body: body.clone() }).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // An agent token works and is attributed to its terminal.
        let agent = app.state.auth.issue_agent_token("term-1");
        let (s, v) = send(&app, Req { token: Some(&agent), headers: vec![], body: body.clone() }).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let tok = master(&app);
        let (s, _) = send(&app, Req { token: Some(&tok), headers: vec![("origin", "http://evil.example".into())], body }).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }

    #[test]
    fn calls_carry_the_sessions_tab_title() {
        let info = |title: &str, ai: Option<&str>| -> crate::terminals::TerminalInfo {
            serde_json::from_value(json!({
                "id": "0a297845bc8b", "kind": "agent", "title": title, "projectId": null, "cwd": "/tmp",
                "argv": ["claude"], "status": "running", "exit": null, "createdAt": 0, "lastOutputAt": 0,
                "cols": 80, "rows": 24, "open": true, "pinned": false, "color": null, "order": 0, "meta": {},
                "agent": {
                    "sessionId": "", "state": "idle", "unread": false, "model": null, "effort": null,
                    "permissionMode": null, "remoteControl": false, "remoteUrl": null, "title": ai,
                    "lastMessage": null, "attention": null, "contextPct": null, "costUsd": null, "lastEventAt": 0
                }
            }))
            .unwrap()
        };
        // Renamed "Pong" → "Pinger"; Claude's automatic title then came back as "Pong".
        assert_eq!(session_title(info("Pinger", Some("Pong"))).as_deref(), Some("Pinger"));
        assert_eq!(session_title(info("Fix the build", Some("Fix the build"))).as_deref(), Some("Fix the build"));
        assert_eq!(session_title(info(" ", Some("Pong"))).as_deref(), Some("Pong"));
        assert_eq!(session_title(info("", None)), None);
    }

    #[tokio::test]
    async fn get_and_delete_are_not_allowed() {
        let app = testutil::app().await;
        for m in ["GET", "DELETE"] {
            let req = Request::builder().method(m).uri("/mcp").header("host", "127.0.0.1:7999").body(Body::empty()).unwrap();
            let resp = app.router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
            assert_eq!(resp.headers().get("allow").unwrap(), "POST");
        }
    }

    #[test]
    fn output_is_capped() {
        let s = "x".repeat(MAX_OUTPUT_CHARS + 5);
        let out = cap_output(s);
        assert!(out.ends_with("[output truncated: 5 more characters]"));
    }

    fn tool_call(name: &str, arguments: Value) -> String {
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string()
    }

    /// `McpTool::acts`: a tool that does something for the user to see is listed as neither
    /// read-only nor destructive; a tool that only reads and a tool that changes remote state
    /// are listed as before.
    #[tokio::test]
    async fn tools_that_act_are_listed_as_neither_read_only_nor_destructive() {
        let app = testutil::app().await;
        let tok = master(&app);
        let (_, v) = send(&app, Req { token: Some(&tok), headers: vec![], body: json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string() }).await;
        let listed = v["result"]["tools"].as_array().unwrap();
        let hints = |name: &str| {
            let t = listed.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no tool {name}"));
            (t["annotations"]["readOnlyHint"].clone(), t["annotations"]["destructiveHint"].clone())
        };
        for name in ["avatar_design", "avatar_say", "avatar_ask", "avatar_act"] {
            assert_eq!(hints(name), (json!(false), json!(false)), "{name}");
        }
        for name in ["avatar_status", "workbench_notify", "workbench_projects"] {
            assert_eq!(hints(name), (json!(true), json!(false)), "{name}");
        }
        assert_eq!(hints("data_append"), (json!(false), json!(true)));
        // For every tool the two hints follow from its traits, and a tool is never both.
        let tools = app.state.platform.tools();
        assert_eq!(tools.len(), listed.len());
        for t in tools.iter() {
            assert_eq!(tool_json(t)["annotations"], json!({ "readOnlyHint": !t.mutating && !t.acts, "destructiveHint": t.mutating }), "{}", t.name);
            assert!(!(t.mutating && t.acts), "{}: a tool that changes remote state is mutating, not one that merely acts", t.name);
        }
    }

    /// `McpTool::quiet`: what succeeds stays out of the Activity ring and emits no `mcp.call`;
    /// a failure of the same tool is recorded like any other call.
    #[tokio::test]
    async fn a_quiet_tool_is_recorded_only_when_it_fails() {
        let app = testutil::app().await;
        // A session with an avatar: `avatar_say` and `avatar_act` are the quiet tools.
        let tid = app.state.terminals.insert_test_agent(None, json!({})).id;
        crate::avatar::device::turn_on(&app.state, &tid, crate::util::now_ms()).unwrap();
        let agent = app.state.auth.issue_agent_token(&tid);
        let mut events = app.state.events.subscribe();

        for (name, arguments) in [("avatar_say", json!({ "text": "Tests are green." })), ("avatar_act", json!({ "play": "nod" }))] {
            let (_, v) = send(&app, Req { token: Some(&agent), headers: vec![], body: tool_call(name, arguments) }).await;
            assert_eq!(v["result"]["isError"], false, "{v}");
        }
        assert!(app.state.platform.activity.calls(10).is_empty(), "a line said and a gesture played are not Activity");

        // The same tool, refused: recorded, with what was wrong.
        let (_, v) = send(&app, Req { token: Some(&agent), headers: vec![], body: tool_call("avatar_say", json!({ "gesture": "nod" })) }).await;
        assert_eq!(v["result"]["isError"], true, "{v}");
        let calls = app.state.platform.activity.calls(10);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].tool.as_str(), calls[0].ok, calls[0].mutating, calls[0].terminal_id.as_deref()), ("avatar_say", false, false, Some(tid.as_str())));
        assert!(calls[0].error.as_deref().is_some_and(|e| e.starts_with("say needs text")), "{:?}", calls[0].error);

        // A tool that is not quiet is recorded when it succeeds, as before.
        let (_, v) = send(&app, Req { token: Some(&agent), headers: vec![], body: tool_call("avatar_status", json!({})) }).await;
        assert_eq!(v["result"]["isError"], false, "{v}");
        let calls = app.state.platform.activity.calls(10);
        assert_eq!(calls.iter().map(|c| (c.tool.as_str(), c.ok)).collect::<Vec<_>>(), [("avatar_status", true), ("avatar_say", false)], "newest first");

        // The events follow the ring: two calls were announced, the quiet two were not.
        let mut announced = vec![];
        while let Ok(event) = events.try_recv() {
            if event.kind == "mcp.call" {
                announced.push((event.data["tool"].as_str().unwrap_or_default().to_string(), event.data["ok"].clone()));
            }
        }
        assert_eq!(announced, [("avatar_say".to_string(), json!(false)), ("avatar_status".to_string(), json!(true))]);
    }

    /// What a session passes to a tool may hold what it must not show: the summary that goes to
    /// the Activity view and into `mcp.call` is masked like the session's permission details,
    /// before it is cut, and the tool still gets its arguments as they came.
    #[tokio::test]
    async fn the_argument_summary_of_a_sessions_call_is_masked() {
        const SECRET: &str = "hunter2hunter2hunter2hunter2hunter2";
        let app = testutil::app().await;
        let state = &app.state;
        let tid = state.terminals.insert_test_agent(None, json!({})).id;
        // A secret value the session was started with (project `[agent].env`, an API key).
        state.terminals.get(&tid).unwrap().redact.lock().push(crate::secrets::Secret::from_value(SECRET.into()));
        crate::avatar::device::turn_on(state, &tid, crate::util::now_ms()).unwrap();
        let agent = state.auth.issue_agent_token(&tid);
        let mut events = state.events.subscribe();
        let summary_of_last = || state.platform.activity.calls(1)[0].summary.clone();
        let call = |name: &'static str, arguments: Value| {
            let agent = agent.clone();
            let app = &app;
            async move { send(app, Req { token: Some(&agent), headers: vec![], body: tool_call(name, arguments) }).await.1 }
        };

        // A short text is summarised whole: the secret in it and what looks like a credential are masked.
        let v = call("workbench_notify", json!({ "message": format!("key {SECRET} and --token abcdef123456"), "title": "Deploy" })).await;
        assert_eq!(v["result"]["isError"], false, "{v}");
        assert_eq!(summary_of_last(), "message=\"key •••••• and --token ••••••\", title=\"Deploy\"");

        // A long text is cut after its first words. The cut falls inside the secret: none of it is left.
        let long = format!("The deploy key is {SECRET}, keep it to yourself. {}", "More words. ".repeat(20));
        let v = call("workbench_notify", json!({ "message": long })).await;
        assert_eq!(v["result"]["isError"], false, "{v}");
        let summary = summary_of_last();
        assert!(summary.starts_with("message=\"The deploy key is ••••••, keep it to yo…\" (") && summary.ends_with(" chars)"), "{summary}");
        assert!(!summary.contains("hunter"), "the beginning of the secret is left: {summary}");
        // The tool itself got the text as it came.
        let got = state.platform.activity.events(5);
        assert!(got.iter().any(|e| e.message.contains(SECRET)), "{got:?}");

        // A call that fails is recorded too (also a quiet tool's): its summary is masked the same way.
        let v = call("avatar_say", json!({ "text": format!("Use {SECRET} now."), "gesture": 7 })).await;
        assert_eq!(v["result"]["isError"], true, "{v}");
        let summary = summary_of_last();
        assert!(summary.contains("text=\"Use •••••• now.\"") && !summary.contains("hunter"), "{summary}");

        // Megabytes of text cost no more to summarise, and reach the tool whole; what a summary
        // shows of them is masked.
        let page = format!("PASSWORD={SECRET}\n{}", "x".repeat(3 * 1024 * 1024));
        let v = call("workbench_notify", json!({ "message": page })).await;
        assert_eq!(v["result"]["isError"], false, "{v}");
        let summary = summary_of_last();
        assert!(summary.starts_with("message=\"PASSWORD=••••••\" (") && !summary.contains("hunter"), "{summary}");

        // What is announced is what is recorded.
        let mut announced = vec![];
        while let Ok(event) = events.try_recv() {
            if event.kind == "mcp.call" {
                announced.push(event.data["summary"].as_str().unwrap_or_default().to_string());
            }
        }
        assert_eq!(announced.len(), 4);
        assert!(announced.iter().all(|s| !s.contains("hunter") && s.contains("••••••")), "{announced:?}");
        let recorded: Vec<String> = state.platform.activity.calls(10).into_iter().rev().map(|c| c.summary).collect();
        assert_eq!(announced, recorded);

        // A caller that is no session has no session's secrets: its summary is as before.
        let tok = master(&app);
        let (_, v) = send(&app, Req { token: Some(&tok), headers: vec![], body: tool_call("workbench_notify", json!({ "message": format!("--token abcdef123456 {SECRET}") })) }).await;
        assert_eq!(v["result"]["isError"], false, "{v}");
        assert_eq!(summary_of_last(), format!("message=\"--token abcdef123456 {SECRET}\""));
        // The master token naming that session speaks as it: masked.
        let (_, v) = send(&app, Req { token: Some(&tok), headers: vec![("x-workbench-terminal", tid.clone())], body: tool_call("workbench_notify", json!({ "message": format!("again {SECRET}") })) }).await;
        assert_eq!(v["result"]["isError"], false, "{v}");
        assert_eq!(summary_of_last(), "message=\"again ••••••\"");
    }

    /// A tool's refusal can quote what the session passed: the error of a failed call is masked
    /// like its summary before it is cut, recorded and announced. The session itself is answered
    /// with the text as it was: the value is its own.
    #[tokio::test]
    async fn a_failed_call_records_its_error_masked_like_its_summary() {
        const SECRET: &str = "hunter2hunter2";
        // Longer than any quote is cut to.
        const LONG: &str = "q7zv4kx9w2mj8rt5bn3hc6yd1pf0gs4lq7zv4kx9w2mj8rt5bn";
        let app = testutil::app().await;
        let state = &app.state;
        let tid = state.terminals.insert_test_agent(None, json!({})).id;
        for secret in [SECRET, LONG] {
            state.terminals.get(&tid).unwrap().redact.lock().push(crate::secrets::Secret::from_value(secret.into()));
        }
        crate::avatar::device::turn_on(state, &tid, crate::util::now_ms()).unwrap();
        let agent = state.auth.issue_agent_token(&tid);
        let mut events = state.events.subscribe();
        let error_of_last = || state.platform.activity.calls(1)[0].error.clone().unwrap_or_default();
        let call = |name: &'static str, arguments: Value| {
            let agent = agent.clone();
            let app = &app;
            async move { send(app, Req { token: Some(&agent), headers: vec![], body: tool_call(name, arguments) }).await.1 }
        };

        // A refusal that quotes an argument: the record shows it masked, the session reads it as it was.
        let v = call("workbench_terminal_output", json!({ "terminalId": SECRET })).await;
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["content"][0]["text"], format!("no terminal \"{SECRET}\" in this session's project"));
        assert_eq!(error_of_last(), "no terminal \"••••••\" in this session's project");

        // Artwork that holds a secret outside its text: no word of it in what is recorded, nor the
        // beginning of a long one.
        let design = |secret: &str| json!({ "svg": format!("<svg viewBox='0 0 200 200'>{secret}<circle id='c' r='5'/></svg>") });
        for secret in [SECRET, LONG] {
            let v = call("avatar_design", design(secret)).await;
            assert_eq!(v["result"]["isError"], true, "{v}");
            let error = error_of_last();
            assert!(error.contains("••••••"), "{error}");
            assert!((0..=secret.len() - 8).all(|at| !error.contains(&secret[at..at + 8])), "a piece of the secret is left: {error}");
        }

        // What is announced is what is recorded.
        let mut announced = vec![];
        while let Ok(event) = events.try_recv() {
            if event.kind == "mcp.call" {
                announced.push(event.data["error"].as_str().unwrap_or_default().to_string());
            }
        }
        let recorded: Vec<String> = state.platform.activity.calls(10).into_iter().rev().map(|c| c.error.unwrap_or_default()).collect();
        assert_eq!(announced, recorded);
        assert!(announced.iter().all(|e| !e.contains("hunter2") && !e.contains(&LONG[..8])), "{announced:?}");
    }

    #[tokio::test]
    async fn a_summary_is_made_of_masked_texts_and_the_arguments_come_back_as_they_were() {
        let app = testutil::app().await;
        let state = &app.state;
        let tid = state.terminals.insert_test_agent(None, json!({})).id;
        state.terminals.get(&tid).unwrap().redact.lock().push(crate::secrets::Secret::from_value("s3cr3t-value-0001".into()));
        let original = json!({ "a": "s3cr3t-value-0001", "b": 5, "c": ["s3cr3t-value-0001", 1], "d": { "k": "v" }, "e": "plain", "f": null, "g": true });
        let mut args = original.as_object().unwrap().clone();
        // A caller that is no session: the plain summary.
        assert_eq!(summary_of(state, None, &mut args), "a=\"s3cr3t-value-0001\", b=5, c=[2 items], d={1 keys}, e=\"plain\", f=null, g=true");
        assert_eq!(Value::Object(args.clone()), original);
        assert_eq!(summary_of(state, Some(&tid), &mut args), "a=\"••••••\", b=5, c=[2 items], d={1 keys}, e=\"plain\", f=null, g=true");
        assert_eq!(Value::Object(args.clone()), original, "the tool gets what the caller sent");
        // An unknown terminal (a name the master token gave) masks nothing, as `redact_text` says.
        assert_eq!(summary_of(state, Some("0a297845bc8b"), &mut args), "a=\"s3cr3t-value-0001\", b=5, c=[2 items], d={1 keys}, e=\"plain\", f=null, g=true");
        // A text longer than what is looked at keeps its length in the summary and its end untouched.
        let long = format!("{}{}", "é".repeat(SUMMARY_LOOK), "s3cr3t-value-0001");
        let mut args = json!({ "text": long }).as_object().unwrap().clone();
        let summary = summary_of(state, Some(&tid), &mut args);
        assert!(summary.ends_with(&format!("({} chars)", SUMMARY_LOOK + 17)) && !summary.contains("s3cr3t"), "{summary}");
        assert_eq!(args["text"], json!(long));
    }

    /// `ToolOutput::Images`: the session reads the text first, then each picture as an `image`
    /// block of base64 PNG. A failed call of the same tool is its error text alone. The Activity
    /// record and `mcp.call` are made from the arguments and the error, as for any call, and the
    /// session's secrets are masked there.
    #[tokio::test]
    async fn pictures_follow_the_text_and_an_error_stays_text() {
        const SECRET: &str = "hunter2hunter2hunter2";
        let app = testutil::app().await;
        let state = &app.state;
        let tid = state.terminals.insert_test_agent(None, json!({})).id;
        state.terminals.get(&tid).unwrap().redact.lock().push(crate::secrets::Secret::from_value(SECRET.into()));
        let pictures = crate::mcp::tool("test_pictures", "Draws two pictures.", json!({ "type": "object" }), false, |_, _, args: Value| async move {
            if let Some(why) = args.get("fail").and_then(Value::as_str) {
                return Err(crate::error::ApiError::bad_request(format!("no pictures: {why}")));
            }
            Ok(ToolOutput::Images { text: "Two pictures: light, then dark.".into(), png: vec![b"\x89PNG\r\n\x1a\none".to_vec(), b"\x89PNG\r\n\x1a\ntwo".to_vec()] })
        });
        let caller = Caller { terminal_id: Some(tid.clone()), project_hint: None };
        let mut events = state.events.subscribe();
        let args = |v: Value| v.as_object().unwrap().clone();

        let out = run_tool(state, &caller, &pictures, args(json!({ "look": format!("key {SECRET}") }))).await;
        let png = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(
            out,
            json!({ "content": [
                { "type": "text", "text": "Two pictures: light, then dark." },
                { "type": "image", "data": png(b"\x89PNG\r\n\x1a\none"), "mimeType": "image/png" },
                { "type": "image", "data": png(b"\x89PNG\r\n\x1a\ntwo"), "mimeType": "image/png" }
            ], "isError": false })
        );

        let out = run_tool(state, &caller, &pictures, args(json!({ "fail": format!("the look {SECRET} is off") }))).await;
        assert_eq!(out, json!({ "content": [{ "type": "text", "text": format!("no pictures: the look {SECRET} is off") }], "isError": true }));

        let calls = state.platform.activity.calls(10);
        assert_eq!(calls.iter().map(|c| (c.tool.as_str(), c.ok)).collect::<Vec<_>>(), [("test_pictures", false), ("test_pictures", true)], "newest first");
        assert_eq!(calls[1].summary, "look=\"key ••••••\"");
        assert_eq!(calls[0].error.as_deref(), Some("no pictures: the look •••••• is off"));
        let mut announced = vec![];
        while let Ok(event) = events.try_recv() {
            if event.kind == "mcp.call" {
                assert!(!event.data.to_string().contains("hunter2") && !event.data.to_string().contains("image/png"), "{}", event.data);
                announced.push(event.data["ok"].clone());
            }
        }
        assert_eq!(announced, [json!(true), json!(false)]);
    }
}

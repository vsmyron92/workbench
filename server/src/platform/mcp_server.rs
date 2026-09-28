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
//! malformed requests are JSON-RPC errors.
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

const INSTRUCTIONS: &str = "Workbench is the developer workspace hosting this Claude Code session. \
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
            "readOnlyHint": !t.mutating,
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

    let ctx = caller.ctx(state);
    let summary = summarize_args(&args);
    let started = Instant::now();
    let task = tokio::spawn((tool.handler)(state.clone(), ctx.clone(), Value::Object(args)));
    let _abort = AbortOnDrop(task.abort_handle());
    let outcome: Result<String, String> = match tokio::time::timeout(MAX_TOOL_TIME, task).await {
        Ok(Ok(Ok(ToolOutput::Text(t)))) => Ok(t),
        Ok(Ok(Ok(ToolOutput::Json(v)))) => Ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())),
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
        error: outcome.as_ref().err().map(|e| super::truncate_chars(e, 300)),
    };
    state.platform.activity.push_call(rec.clone());
    state.events.emit("mcp.call", ctx.project_id.as_deref(), &rec);
    tracing::info!(tool = name, ok, ms, terminal = ctx.terminal_id.as_deref().unwrap_or("-"), "mcp tool call");

    let (text, is_error) = match outcome {
        Ok(t) => (t, false),
        Err(e) => (e, true),
    };
    Ok(json!({ "content": [{ "type": "text", "text": cap_output(text) }], "isError": is_error }))
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
}

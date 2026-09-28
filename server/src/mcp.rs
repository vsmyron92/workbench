//! Shared MCP tool definitions.
//!
//! Hosted Claude sessions get Workbench as an MCP server (`/mcp`, implemented by the
//! `platform` slice). Every slice contributes tools for its own domain through a
//! `pub fn mcp_tools() -> Vec<McpTool>` in its module; `all_tools()` gathers them.
//! A tool handler receives the calling session (`McpCtx`) and its JSON arguments.
//!
//! `call_api` lets a tool reuse any REST route in-process (no network, no auth
//! round-trip) when that is simpler than calling the slice's Rust functions.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::Value;
use tower::ServiceExt;

use crate::app::AppState;
use crate::auth::{Caller, InternalCall};
use crate::error::ApiError;

/// Who is calling a tool.
#[derive(Debug, Clone, Default)]
pub struct McpCtx {
    /// The Workbench terminal hosting the calling Claude session.
    pub terminal_id: Option<String>,
    /// That terminal's project, when it has one.
    pub project_id: Option<String>,
}

impl McpCtx {
    /// Whether the caller is a hosted session: an agent token (always bound to its
    /// terminal) or the master token naming a terminal (`X-Workbench-Terminal`).
    /// Sessions are confined to their own project.
    pub fn is_session(&self) -> bool {
        self.terminal_id.is_some()
    }

    /// The project a tool acts on: `requested` (the tool's `projectId`-style
    /// argument) or else the calling session's project.
    ///
    /// A session may only name its own project, so a prompt-injected agent cannot
    /// reach another project's GitLab, run configurations or terminals. Callers
    /// that are not a session (the master token without a terminal) may name any.
    pub fn project_for(&self, requested: Option<&str>) -> Result<String, ApiError> {
        let requested = requested.map(str::trim).filter(|p| !p.is_empty());
        match (requested, &self.project_id) {
            (Some(req), Some(own)) if self.is_session() && req != own => Err(ApiError::forbidden(format!(
                "this session belongs to project {own:?} and cannot act on project {req:?}"
            ))),
            (Some(req), None) if self.is_session() => Err(ApiError::forbidden(format!(
                "this session has no project and cannot act on project {req:?}"
            ))),
            (Some(req), _) => Ok(req.to_string()),
            (None, Some(own)) => Ok(own.clone()),
            (None, None) => Err(ApiError::bad_request(
                "this session has no project; pass projectId (a Workbench project id)",
            )),
        }
    }

    /// Whether the caller may see something that belongs to `project` (a terminal,
    /// a run): anything for non-session callers; for a session, only its own
    /// project's things.
    pub fn may_see_project(&self, project: Option<&str>) -> bool {
        !self.is_session() || (self.project_id.is_some() && self.project_id.as_deref() == project)
    }
}

/// What a tool returns. `Text` is shown to the model as-is; `Json` is pretty-printed.
#[derive(Debug, Clone)]
pub enum ToolOutput {
    Text(String),
    Json(Value),
}

pub type ToolFuture = Pin<Box<dyn Future<Output = Result<ToolOutput, ApiError>> + Send>>;
pub type ToolHandler = Arc<dyn Fn(AppState, McpCtx, Value) -> ToolFuture + Send + Sync>;

#[derive(Clone)]
pub struct McpTool {
    /// `snake_case`, prefixed by domain: `git_…`, `gitlab_…`, `confluence_…`, `workbench_…`.
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (`{"type":"object","properties":{…},"required":[…]}`).
    pub input_schema: Value,
    /// Tools that change remote state (post a comment, edit a page) set this so the
    /// UI can show what agents did.
    pub mutating: bool,
    pub handler: ToolHandler,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool").field("name", &self.name).finish()
    }
}

/// Build a tool from an async closure.
pub fn tool<F, Fut>(name: &str, description: &str, input_schema: Value, mutating: bool, f: F) -> McpTool
where
    F: Fn(AppState, McpCtx, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ToolOutput, ApiError>> + Send + 'static,
{
    McpTool {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        mutating,
        handler: Arc::new(move |s, c, v| Box::pin(f(s, c, v))),
    }
}

/// Every tool from every slice.
pub fn all_tools() -> Vec<McpTool> {
    let mut v = vec![];
    v.extend(crate::terminals::mcp_tools());
    v.extend(crate::files::mcp_tools());
    v.extend(crate::git::mcp_tools());
    v.extend(crate::gitlab::mcp_tools());
    v.extend(crate::github::mcp_tools());
    v.extend(crate::workspace::mcp_tools());
    v.extend(crate::atlassian::mcp_tools());
    v.extend(crate::apps::mcp_tools());
    v.extend(crate::devcontainer::mcp_tools());
    v.extend(crate::lsp::mcp_tools());
    v.extend(crate::debug::mcp_tools());
    v.extend(crate::platform::mcp_tools());
    v
}

/// Call a Workbench REST route in-process and return its JSON body.
/// Non-2xx responses become `ApiError` with the route's own message.
pub async fn call_api(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<Value>,
    ctx: &McpCtx,
) -> Result<Value, ApiError> {
    let router = state.router().ok_or_else(|| ApiError::internal("router not ready"))?;
    let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1");
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&v)?)
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).map_err(|e| ApiError::internal(e.to_string()))?;
    req.extensions_mut().insert(InternalCall);
    req.extensions_mut().insert(Caller::Internal { terminal_id: ctx.terminal_id.clone() });
    let resp = router.oneshot(req).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    if status.is_success() {
        Ok(value)
    } else {
        let msg = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        Err(ApiError::new(status, "upstream", msg))
    }
}

#[cfg(test)]
mod tests {
    use super::McpCtx;

    fn ctx(terminal: Option<&str>, project: Option<&str>) -> McpCtx {
        McpCtx { terminal_id: terminal.map(str::to_string), project_id: project.map(str::to_string) }
    }

    #[test]
    fn sessions_are_confined_to_their_project() {
        let agent = ctx(Some("t1"), Some("shop"));
        assert_eq!(agent.project_for(None).unwrap(), "shop");
        assert_eq!(agent.project_for(Some(" shop ")).unwrap(), "shop");
        assert_eq!(agent.project_for(Some("")).unwrap(), "shop");
        let err = agent.project_for(Some("other")).unwrap_err();
        assert_eq!(err.code, "forbidden");
        assert!(agent.may_see_project(Some("shop")));
        assert!(!agent.may_see_project(Some("other")));
        assert!(!agent.may_see_project(None));

        // A session without a project can neither pick one nor see others' things.
        let loose = ctx(Some("t2"), None);
        assert_eq!(loose.project_for(Some("shop")).unwrap_err().code, "forbidden");
        assert_eq!(loose.project_for(None).unwrap_err().code, "bad_request");
        assert!(!loose.may_see_project(None));
    }

    #[test]
    fn non_session_callers_may_name_any_project() {
        let master = ctx(None, Some("shop"));
        assert_eq!(master.project_for(None).unwrap(), "shop");
        assert_eq!(master.project_for(Some("other")).unwrap(), "other");
        assert!(master.may_see_project(Some("other")) && master.may_see_project(None));
        assert_eq!(McpCtx::default().project_for(None).unwrap_err().code, "bad_request");
        assert_eq!(McpCtx::default().project_for(Some("shop")).unwrap(), "shop");
    }
}

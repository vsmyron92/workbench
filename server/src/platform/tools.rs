//! Platform MCP tools: `workbench_projects`, `workbench_notify`, `workbench_open_url`.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::Method;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{self, McpCtx, McpTool, ToolOutput};

use super::notify::{DesktopNote, NoteLevel, Priority};
use super::push::{OpenTarget, PushNote, Topic, Urgency};

pub fn platform_tools() -> Vec<McpTool> {
    vec![
        mcp::tool(
            "workbench_projects",
            "List the projects open in Workbench: id, name, root directory, current git branch, GitLab path, \
             environments and number of run configurations. `current: true` marks the project of this session. \
             Other Workbench tools take these project ids. A project with several git repositories also lists \
             them as `repositories`; the git, GitLab and GitHub tools take one of their ids as `repo` (the default \
             repository when omitted).",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            false,
            projects,
        ),
        mcp::tool(
            "workbench_notify",
            "Notify the user: a toast in every open Workbench window, and whatever else they set up: a desktop \
             notification on the computer Workbench runs on (where its OS supports them), their notify command, \
             a push to their phone or other devices. The result says whether the desktop notification and the \
             push went out. Use it when a long task finishes or you need the user's attention; keep the message \
             short.",
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "What to tell the user (one or two sentences).", "maxLength": 2000 },
                    "level": { "type": "string", "enum": ["info", "success", "warning", "error"], "default": "info" }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
            false,
            notify,
        ),
        mcp::tool(
            "workbench_open_url",
            "Open a web page (e.g. the local dev server, a staging or production URL, a CI page) in a Workbench \
             app panel next to this session, so the user can look at it without leaving Workbench.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "http:// or https:// URL." },
                    "title": { "type": "string", "description": "Tab title (optional)." }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            false,
            open_url,
        ),
    ]
}

async fn projects(state: AppState, ctx: McpCtx, _args: Value) -> Result<ToolOutput, ApiError> {
    let list = mcp::call_api(&state, Method::GET, "/api/projects", None, &ctx).await?;
    let items: Vec<Value> = list
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let mut o = json!({
                "id": p["id"],
                "name": p["name"],
                "root": p["rootAbs"],
                "branch": p["branch"],
                "gitlab": p["gitlab"]["path"],
                "github": p["github"]["path"],
                "environments": p["envs"],
                "runConfigurations": p["runs"],
                "confluence": p["hasConfluence"],
                "jira": p["hasJira"],
                "current": ctx.project_id.is_some() && p["id"].as_str() == ctx.project_id.as_deref(),
            });
            if p["warnings"].as_array().is_some_and(|w| !w.is_empty()) {
                o["warnings"] = p["warnings"].clone();
            }
            // Only worth saying for a project with several: the ids the git, GitLab and
            // GitHub tools take as `repo`.
            if let Some(repos) = p["repos"].as_array().filter(|r| r.len() > 1) {
                o["repositories"] = repos
                    .iter()
                    .map(|r| {
                        json!({
                            "id": r["id"], "name": r["name"], "path": r["path"], "default": r["default"],
                            "gitlab": r["gitlab"]["path"], "github": r["github"]["path"],
                        })
                    })
                    .collect();
            }
            o
        })
        .collect();
    Ok(ToolOutput::Json(json!({ "count": items.len(), "projects": items })))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

async fn notify(state: AppState, ctx: McpCtx, args: Value) -> Result<ToolOutput, ApiError> {
    let message = str_arg(&args, "message").ok_or_else(|| ApiError::bad_request("message is required"))?;
    let message = super::truncate_chars(message, 2000);
    let level = match str_arg(&args, "level").unwrap_or("info") {
        l @ ("info" | "success" | "warning" | "error") => l,
        other => return Err(ApiError::bad_request(format!("level must be info, success, warning or error (got {other:?})"))),
    };
    state.events.notify(level, &message);

    let session = ctx.terminal_id.as_deref().and_then(|t| state.terminals.info(t)).and_then(super::mcp_server::session_title);
    let project = ctx.project_id.as_deref().and_then(|p| state.projects.get(p)).map(|p| p.name.clone());
    let title = [Some("Workbench".to_string()), project, session].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    super::activity::record_event(&state, "notify", level, ctx.project_id.as_deref(), ctx.terminal_id.as_deref(), &title, &message);
    let push = PushNote {
        topic: Topic::Notify,
        tag: format!("notify:{}", ctx.terminal_id.as_deref().unwrap_or("-")),
        title: title.strip_prefix("Workbench · ").unwrap_or(&title).to_string(),
        body: super::truncate_chars(&message, 200),
        level: match level {
            "success" => "success",
            "warning" => "warning",
            "error" => "error",
            _ => "info",
        },
        urgency: if level == "error" { Urgency::High } else { Urgency::Normal },
        ttl_secs: 3600,
        project_id: ctx.project_id.clone(),
        open: ctx.terminal_id.as_deref().map(OpenTarget::terminal),
        agent: None,
    };
    let outcome = state.platform.notifier.send(
        &state,
        DesktopNote {
            key: format!("tool:{}", ctx.terminal_id.as_deref().unwrap_or("-")),
            repeat_after: Duration::from_secs(20),
            priority: Priority::Normal,
            title,
            body: message,
            level: NoteLevel::parse(level),
            event: "agent.notify",
            push: Some(push),
        },
    );
    let pushed = if outcome.push == "queued" { " Pushed to the user's devices." } else { "" };
    Ok(ToolOutput::Text(format!("Shown in Workbench. Desktop notification: {}.{pushed}", outcome.desktop)))
}

/// Accept only absolute http(s) URLs without embedded credentials.
pub fn validate_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.len() > 4096 {
        return Err("URL is too long".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|e| format!("not a valid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("only http and https URLs can be opened (got {}:)", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with embedded credentials are not allowed".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("URL has no host".into());
    }
    Ok(url.to_string())
}

async fn open_url(state: AppState, ctx: McpCtx, args: Value) -> Result<ToolOutput, ApiError> {
    let raw = str_arg(&args, "url").ok_or_else(|| ApiError::bad_request("url is required"))?;
    let url = validate_url(raw).map_err(ApiError::bad_request)?;
    let title = str_arg(&args, "title").map(|t| super::truncate_chars(t, 80));
    state.events.ui_open("app", json!({ "projectId": ctx.project_id, "url": url }), title.as_deref());
    Ok(ToolOutput::Text(format!("Opened {url} in Workbench.")))
}

/// `GET /api/platform/tools` — every MCP tool Workbench offers agents.
pub async fn list_route(State(state): State<AppState>) -> Json<Value> {
    Json(tools_summary(&state))
}

pub fn tools_summary(state: &AppState) -> Value {
    let tools: Vec<Value> = state
        .platform
        .tools()
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "mutating": t.mutating }))
        .collect();
    Value::Array(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_validation() {
        assert_eq!(validate_url("https://staging.shop.example.com").unwrap(), "https://staging.shop.example.com/");
        assert!(validate_url("http://127.0.0.1:5173/app?x=1").is_ok());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("https://user:pw@example.com").is_err());
        assert!(validate_url("not a url").is_err());
    }

    #[test]
    fn tool_names_follow_the_convention() {
        for t in platform_tools() {
            assert!(t.name.starts_with("workbench_"), "{}", t.name);
            assert!(t.input_schema["type"] == "object");
            assert!(!t.mutating);
        }
    }
}

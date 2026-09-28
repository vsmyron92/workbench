//! MCP tools for agents: environment health and run configurations.
//! Deploys, logs and remote commands are deliberately not exposed.

use serde_json::{Value, json};

use super::{envs, runs};
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

/// The calling session's project; a session cannot name another one (`McpCtx::project_for`).
fn project_of(state: &AppState, ctx: &McpCtx, args: &Value) -> Result<std::sync::Arc<crate::projects::Project>, ApiError> {
    let pid = ctx.project_for(args.get("projectId").and_then(Value::as_str))?;
    state.projects.require(&pid)
}

fn name_arg(args: &Value) -> Result<String, ApiError> {
    args.get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::bad_request("name is required"))
}

fn project_prop() -> Value {
    json!({ "type": "string", "description": "Workbench project id; defaults to the calling session's project." })
}

pub fn tools() -> Vec<McpTool> {
    vec![
        tool(
            "env_status",
            "Health (up/down/degraded, HTTP status, latency, recent uptime) and deployed version of each environment \
             (production, staging, …) of a project, as last checked by Workbench's poller.",
            json!({ "type": "object", "properties": { "projectId": project_prop() } }),
            false,
            |state, ctx, args| async move {
                let p = project_of(&state, &ctx, &args)?;
                Ok(ToolOutput::Json(json!({ "project": p.id, "environments": envs::status_json(&state, &p) })))
            },
        ),
        tool(
            "run_list",
            "Run configurations of a project (dev servers, tests, builds, tasks) with their live state \
             (stopped|starting|running|ready|failed|exited), port, URL and test results.",
            json!({ "type": "object", "properties": { "projectId": project_prop() } }),
            false,
            |state, ctx, args| async move {
                let p = project_of(&state, &ctx, &args)?;
                let list = runs::list(&state, &p).await;
                let out: Vec<Value> = list
                    .iter()
                    .map(|r| {
                        json!({
                            "name": r.name, "kind": r.config.kind, "command": r.config.command, "cwd": r.config.cwd,
                            "group": r.config.group, "state": r.live.state, "port": r.live.port.or(r.config.port),
                            "url": r.live.url, "error": r.live.error, "result": r.live.result, "problems": r.problems,
                            "portInUse": r.port_in_use, "needsConfirm": r.needs_confirm,
                        })
                    })
                    .collect();
                Ok(ToolOutput::Json(json!({ "project": p.id, "runs": out })))
            },
        ),
        tool(
            "run_start",
            "Start a run configuration by name (its dependencies start first). Returns immediately; poll run_list \
             for readiness. Fails if its port is taken by another process. Runs that deploy, release or reach \
             remote hosts (needsConfirm in run_list) and documentation suggestions cannot be started by agents: \
             ask the user to start them.",
            json!({ "type": "object", "properties": { "name": { "type": "string" }, "projectId": project_prop() }, "required": ["name"] }),
            false,
            |state, ctx, args| async move {
                let p = project_of(&state, &ctx, &args)?;
                let name = name_arg(&args)?;
                // A run's command comes from files the agent can edit (Makefile, package.json):
                // started for a sandboxed session, it would run outside that sandbox.
                if let Some(who) = ctx.terminal_id.as_deref().and_then(|t| state.terminals.sandboxed_agent(&state, t)) {
                    return Err(ApiError::forbidden(format!(
                        "{who} runs its commands in a sandbox, and run configurations run outside it; ask the user to \
                         start {name} from Workbench"
                    )));
                }
                let gated = runs::gated(&state, &p, &name, false, true).await?;
                if !gated.is_empty() {
                    return Err(ApiError::forbidden(format!(
                        "agents cannot start {} (it may deploy, release or reach a remote host, or it is an unvetted \
                         documentation suggestion); ask the user to start it from Workbench",
                        gated.join(", ")
                    )));
                }
                runs::start(&state, &p, &name, false).await?;
                let v = runs::view(&state, &p, &name).await?;
                Ok(ToolOutput::Json(json!({ "name": name, "state": v.live.state, "phase": v.live.phase })))
            },
        ),
        tool(
            "run_stop",
            "Stop a running run configuration by name.",
            json!({ "type": "object", "properties": { "name": { "type": "string" }, "projectId": project_prop() }, "required": ["name"] }),
            false,
            |state, ctx, args| async move {
                let p = project_of(&state, &ctx, &args)?;
                let name = name_arg(&args)?;
                runs::stop(&state, &p, &name).await?;
                Ok(ToolOutput::Text(format!("{name}: stopped")))
            },
        ),
        tool(
            "run_output",
            "The last lines of a run configuration's terminal output (plain text, at most 500 lines).",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "lines": { "type": "integer", "minimum": 1, "maximum": 500, "default": 120 },
                    "projectId": project_prop()
                },
                "required": ["name"]
            }),
            false,
            |state, ctx, args| async move {
                let p = project_of(&state, &ctx, &args)?;
                let name = name_arg(&args)?;
                let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(120).clamp(1, 500) as usize;
                let live = state.apps.runs.live(&p.id, &name);
                // A starting run keeps its previous terminal until the new process runs in it.
                let fresh = live.state != runs::RunState::Starting
                    || live.terminal_id.as_deref().and_then(|t| state.terminals.info(t)).is_some_and(|i| i.exit.is_none());
                let Some(tid) = live.terminal_id.filter(|_| fresh) else {
                    if !p.config.runs.iter().any(|r| r.name == name) {
                        return Err(ApiError::not_found(format!("no run configuration {name:?}")));
                    }
                    let st = serde_json::to_value(live.state).unwrap_or(Value::Null);
                    return Ok(ToolOutput::Text(format!("{name} has no output yet (state: {})", st.as_str().unwrap_or("?"))));
                };
                let text = state.terminals.screen_text(&tid, lines).unwrap_or_default();
                let secrets = state.apps.runs.secrets(&p.id, &name);
                Ok(ToolOutput::Text(crate::secrets::redact(&text, &secrets)))
            },
        ),
    ]
}

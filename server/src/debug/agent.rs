//! MCP tools by which an agent steers the user's debug sessions: continue, pause, step,
//! run to a line, stop, and plain breakpoints. `debug_state` (in `tools`) reads.
//!
//! **Trust.** The REST routes still refuse in-process callers; these tools are the agents'
//! way in, and they cannot make anything run that the user did not start:
//! * an agent steers sessions the user started: there is no tool to start a session, attach
//!   to a process, rerun a configuration or evaluate an expression (a gdb expression can call
//!   `$_shell("…")`, a start runs a configuration's build command and debug server);
//! * confined to the calling session's project (`McpCtx::project_for`);
//! * `mutating`, so the agent's own permission prompts and Workbench's Activity view treat
//!   them as writes;
//! * breakpoints are plain lines and functions: conditions and log messages are expressions,
//!   and an agent cannot set them (the user does, in the editor).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::breakpoints::{FunctionBreakpoint, LineBreakpoint};
use super::session::{self, Session, SessionInfo, SessionState};
use super::tools::report;
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

const MAX_WAIT: u64 = 300;

fn project_id(ctx: &McpCtx, state: &AppState, args: &Value) -> Result<String, ApiError> {
    let pid = ctx.project_for(args.get("projectId").and_then(Value::as_str))?;
    state.projects.require(&pid)?;
    Ok(pid)
}

fn project_prop() -> Value {
    json!({ "type": "string", "description": "Workbench project id; defaults to the calling session's project." })
}

fn wait_arg(args: &Value, default: u64) -> Duration {
    Duration::from_secs(args.get("waitSeconds").and_then(Value::as_u64).unwrap_or(default).min(MAX_WAIT))
}

/// The session a call means: `sessionId`, else the project's only live session.
fn live_session(state: &AppState, pid: &str, args: &Value) -> Result<Arc<Session>, ApiError> {
    if let Some(id) = args.get("sessionId").and_then(Value::as_str) {
        return state.debug.get(pid, id).ok_or_else(|| ApiError::not_found(format!("no debug session {id} in project {pid}")));
    }
    let live: Vec<Arc<Session>> = state.debug.sessions_of(pid).into_iter().filter(|s| s.is_live() && s.parent.is_none()).collect();
    match live.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(ApiError::conflict("no live debug session in this project: the user starts one from the Debug tool window")),
        many => Err(ApiError::bad_request(format!(
            "{} live debug sessions: name one with sessionId ({})",
            many.len(),
            many.iter().map(|s| format!("{} \"{}\"", s.id, s.name)).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// Not running and not starting: stopped somewhere, or over.
fn settled(i: &SessionInfo) -> bool {
    !matches!(i.state, SessionState::Running | SessionState::Starting)
}

fn has_expression(b: &LineBreakpoint) -> bool {
    [&b.condition, &b.log_message].iter().any(|t| t.as_deref().is_some_and(|t| !t.trim().is_empty()))
}

pub fn tools() -> Vec<McpTool> {
    vec![
        tool(
            "debug_control",
            "Steer a debug session the user started: `continue`, `pause`, `next` (step over), `stepIn`, `stepOut`, \
             `runTo` (with path and line) or `stop` (ends it; a launched program is terminated, an attach or a \
             microcontroller is detached). Continue, the steps and runTo wait until the program stops again, ends or \
             the wait is over, then answer like debug_state: where it stopped, the stack, the locals (the CPU \
             registers with `registers`) and the console's tail. The session defaults to the project's only live \
             one. Agents cannot start a session, attach, rerun or evaluate expressions: the user does.",
            json!({ "type": "object", "properties": {
                "action": { "type": "string", "enum": ["continue", "pause", "next", "stepIn", "stepOut", "runTo", "stop"] },
                "sessionId": { "type": "string" },
                "threadId": { "type": "integer", "description": "The thread to step (default: the one that stopped)." },
                "path": { "type": "string", "description": "runTo: project-relative file." },
                "line": { "type": "integer", "description": "runTo: 1-based line." },
                "waitSeconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT, "description": "Seconds to wait for the program to stop or end before answering (default 15); the state is reported either way." },
                "frame": { "type": "integer", "description": "Stack frame for the locals in the answer (0 = innermost)." },
                "registers": { "type": "boolean", "description": "Include the CPU registers of that frame." },
                "projectId": project_prop()
            }, "required": ["action"] }),
            true,
            |state, ctx, args| async move {
                let pid = project_id(&ctx, &state, &args)?;
                let action = args.get("action").and_then(Value::as_str).unwrap_or("");
                let s = live_session(&state, &pid, &args)?;
                let thread = args.get("threadId").and_then(Value::as_i64);
                let frame = args.get("frame").and_then(Value::as_u64).unwrap_or(0).min(29) as usize;
                let registers = args.get("registers").and_then(Value::as_bool).unwrap_or(false);
                let done = match action {
                    "stop" => {
                        session::stop(&state, &s).await;
                        true
                    }
                    "runTo" => {
                        let path = args.get("path").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("runTo needs path and line"))?;
                        let line = args.get("line").and_then(Value::as_i64).filter(|l| *l >= 1).ok_or_else(|| ApiError::bad_request("runTo needs path and line"))?;
                        session::run_to(&state, &s, path, line, thread).await?;
                        session::wait_for(&s, wait_arg(&args, 15), settled).await
                    }
                    "continue" | "pause" | "next" | "stepIn" | "stepOut" => {
                        session::control(&state, &s, action, thread).await?;
                        session::wait_for(&s, wait_arg(&args, 15), settled).await
                    }
                    other => return Err(ApiError::bad_request(format!("unknown action {other:?}"))),
                };
                let mut v = report(&s, frame, thread, registers).await;
                v["settled"] = json!(done);
                Ok(ToolOutput::Json(v))
            },
        ),
        tool(
            "debug_breakpoints",
            "The project's debug breakpoints (shared by the user's editor and every session; they apply to live \
             sessions at once). `list` shows them with whether a live session placed (verified) each; `add` adds \
             line breakpoints to a file keeping the others; `remove` removes the given lines of a file (all of its \
             breakpoints without `lines`); `set` replaces a file's breakpoints; `functions` replaces the function \
             breakpoints; `mute` mutes or unmutes all; `clear` removes every breakpoint. A breakpoint is \
             {line, enabled?, hitCondition?}; conditions and log messages are expressions, which agents cannot set.",
            json!({ "type": "object", "properties": {
                "action": { "type": "string", "enum": ["list", "add", "remove", "set", "functions", "mute", "clear"] },
                "path": { "type": "string", "description": "Project-relative file (add, remove, set)." },
                "breakpoints": { "type": "array", "items": { "type": "object" }, "description": "add, set: {line, enabled?, hitCondition?}; functions: {name, enabled?}." },
                "lines": { "type": "array", "items": { "type": "integer" }, "description": "remove: the lines." },
                "muted": { "type": "boolean" },
                "projectId": project_prop()
            }, "required": ["action"] }),
            true,
            |state, ctx, args| async move {
                let pid = project_id(&ctx, &state, &args)?;
                let action = args.get("action").and_then(Value::as_str).unwrap_or("");
                let path = || args.get("path").and_then(Value::as_str).map(str::to_string).ok_or_else(|| ApiError::bad_request("this action needs a path"));
                let list = args.get("breakpoints").cloned().unwrap_or(Value::Array(vec![]));
                match action {
                    "list" => {}
                    "add" | "set" => {
                        let path = path()?;
                        let new: Vec<LineBreakpoint> = serde_json::from_value(list).map_err(|e| ApiError::bad_request(format!("breakpoints: {e}")))?;
                        if new.iter().any(has_expression) {
                            return Err(ApiError::forbidden("conditions and log messages are expressions the debugger evaluates; agents cannot set them: ask the user to set them in the editor"));
                        }
                        let norm = super::breakpoints::normalize_path(&path)?;
                        let merged = if action == "set" {
                            new
                        } else {
                            // The others of the file stay, a new one replaces the one on its line.
                            let mut keep: Vec<LineBreakpoint> = state.debug.store.get(&state.paths.data_dir, &pid).breakpoints.into_iter().filter(|b| b.path == norm && !new.iter().any(|n| n.line == b.line)).collect();
                            keep.extend(new);
                            keep
                        };
                        session::set_file_breakpoints(&state, &pid, &path, merged).await?;
                    }
                    "remove" => {
                        let path = path()?;
                        let norm = super::breakpoints::normalize_path(&path)?;
                        let drop: Vec<u32> = args.get("lines").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).filter_map(|l| u32::try_from(l).ok()).collect()).unwrap_or_default();
                        // No lines: all of the file's breakpoints go.
                        let keep: Vec<LineBreakpoint> = state
                            .debug
                            .store
                            .get(&state.paths.data_dir, &pid)
                            .breakpoints
                            .into_iter()
                            .filter(|b| b.path == norm && !drop.is_empty() && !drop.contains(&b.line))
                            .collect();
                        session::set_file_breakpoints(&state, &pid, &path, keep).await?;
                    }
                    "functions" => {
                        let list: Vec<FunctionBreakpoint> = serde_json::from_value(list).map_err(|e| ApiError::bad_request(format!("breakpoints: {e}")))?;
                        if list.iter().any(|f| f.condition.as_deref().is_some_and(|c| !c.trim().is_empty())) {
                            return Err(ApiError::forbidden("conditions are expressions the debugger evaluates; agents cannot set them: ask the user to set them in the editor"));
                        }
                        session::set_function_breakpoints(&state, &pid, list).await?;
                    }
                    "mute" => session::set_muted(&state, &pid, args.get("muted").and_then(Value::as_bool).unwrap_or(true)).await?,
                    "clear" => session::clear_breakpoints(&state, &pid).await?,
                    other => return Err(ApiError::bad_request(format!("unknown action {other:?}"))),
                }
                Ok(ToolOutput::Json(session::breakpoints_view(&state, &pid)))
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_are_bounded_and_expressions_are_found() {
        assert_eq!(wait_arg(&json!({}), 15), Duration::from_secs(15));
        assert_eq!(wait_arg(&json!({ "waitSeconds": 0 }), 15), Duration::ZERO);
        assert_eq!(wait_arg(&json!({ "waitSeconds": 99999 }), 15), Duration::from_secs(MAX_WAIT));
        let plain = LineBreakpoint { line: 3, ..Default::default() };
        assert!(!has_expression(&plain));
        assert!(!has_expression(&LineBreakpoint { condition: Some("  ".into()), ..plain.clone() }));
        assert!(has_expression(&LineBreakpoint { condition: Some("x > 1".into()), ..plain.clone() }));
        assert!(has_expression(&LineBreakpoint { log_message: Some("x={x}".into()), ..plain }));
    }
}

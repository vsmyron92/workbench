//! MCP tools by which an agent drives debug sessions: start a configuration, attach, rerun,
//! continue, pause, step, run to a line, stop, evaluate an expression, and set breakpoints
//! (conditions and log messages too). `debug_state` (in `tools`) reads.
//!
//! **Trust.** The REST routes still refuse in-process callers; these tools are the agents'
//! way in. The owner decided agents get all of it without an opt-in, and that a session
//! whose CLI sandboxes its commands (Codex) is held back by that CLI's own sandbox and
//! approvals, not by a refusal here. What contains it is what was already there:
//! * not confined to the calling session's project: `projectId` names any project, and the
//!   session's own is the default (every other MCP tool is confined);
//! * `mutating`, so the agent's own permission prompts and Workbench's Activity view treat
//!   them as writes; an evaluation made in the console is echoed there with `(agent)` on it;
//! * the planner still refuses a pre-launch run that deploys or reaches another host, for
//!   every caller.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::breakpoints::{FunctionBreakpoint, LineBreakpoint};
use super::launch;
use super::routes::variable_view;
use super::session::{self, REQUEST_TIMEOUT, Session, SessionInfo, SessionState};
use super::tools::report;
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

const MAX_WAIT: u64 = 300;

fn project_id(ctx: &McpCtx, state: &AppState, args: &Value) -> Result<String, ApiError> {
    let pid = args
        .get("projectId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| ctx.project_id.clone())
        .ok_or_else(|| ApiError::bad_request("this session has no project; pass projectId (a Workbench project id)"))?;
    state.projects.require(&pid)?;
    Ok(pid)
}

fn project_prop() -> Value {
    json!({ "type": "string", "description": "Workbench project id; defaults to the calling session's project (any project may be named)." })
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

/// Wait for a session the agent just started (or restarted) to settle, then answer like
/// `debug_state`: where it stopped, the stack, the locals.
async fn started(state: &AppState, pid: &str, info: SessionInfo, args: &Value) -> Result<ToolOutput, ApiError> {
    let s = state.debug.get(pid, &info.id).ok_or_else(|| ApiError::internal("the session ended before it could be read"))?;
    let frame = args.get("frame").and_then(Value::as_u64).unwrap_or(0).min(29) as usize;
    let registers = args.get("registers").and_then(Value::as_bool).unwrap_or(false);
    let done = session::wait_for(&s, wait_arg(args, 60), settled).await;
    let mut v = report(&s, frame, None, registers).await;
    v["settled"] = json!(done);
    Ok(ToolOutput::Json(v))
}

/// The id of stack frame `index` (0 = innermost) of the thread the session stopped in.
async fn frame_id(s: &Arc<Session>, thread: Option<i64>, index: usize) -> Option<i64> {
    let info = s.info();
    if info.state != SessionState::Stopped {
        return None;
    }
    let tid = thread.or(info.stopped.as_ref().and_then(|x| x.thread_id)).or(info.threads.first().map(|t| t.id))?;
    let body = s.request("stackTrace", json!({ "threadId": tid, "startFrame": 0, "levels": index + 1 }), REQUEST_TIMEOUT).await.ok()?;
    body.get("stackFrames")?.as_array()?.get(index)?.get("id")?.as_i64()
}

pub fn tools() -> Vec<McpTool> {
    vec![
        tool(
            "debug_start",
            "Start a debug configuration of the project by name (see debug_state for the sessions, or the Debug \
             window for the configurations). Its pre-launch build step runs first, then its debug server, then the \
             debugger; an attach configuration needs `pid`. Waits (`waitSeconds`, default 60) until the program \
             stops, ends or the wait is over, then answers like debug_state: where it stopped, the stack, the \
             locals and the console's tail.",
            json!({ "type": "object", "properties": {
                "config": { "type": "string", "description": "The configuration's name." },
                "stopOnEntry": { "type": "boolean", "description": "Stop at the program's entry (default: the configuration's own setting)." },
                "pid": { "type": "integer", "description": "The process an attach configuration without a pid attaches to." },
                "waitSeconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT },
                "frame": { "type": "integer" },
                "registers": { "type": "boolean" },
                "projectId": project_prop()
            }, "required": ["config"] }),
            true,
            |state, ctx, args| async move {
                let pid = project_id(&ctx, &state, &args)?;
                let config = args.get("config").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("config: the configuration's name"))?;
                let project = state.projects.require(&pid)?;
                let stop = args.get("stopOnEntry").and_then(Value::as_bool);
                let attach = args.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok());
                let plan = launch::plan_config(&state, &project, config, stop, attach).await?;
                let info = session::start(&state, project, plan, None).await?;
                started(&state, &pid, info, &args).await
            },
        ),
        tool(
            "debug_attach",
            "Attach the debugger to a running process of this computer by pid (language or adapter and program \
             optional: they are guessed from the process). Waits like debug_start.",
            json!({ "type": "object", "properties": {
                "pid": { "type": "integer" },
                "adapter": { "type": "string", "description": "A debug adapter id (default: guessed)." },
                "language": { "type": "string" },
                "program": { "type": "string", "description": "The program's path, for symbols." },
                "waitSeconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT },
                "frame": { "type": "integer" },
                "registers": { "type": "boolean" },
                "projectId": project_prop()
            }, "required": ["pid"] }),
            true,
            |state, ctx, args| async move {
                let pid = project_id(&ctx, &state, &args)?;
                let target = args.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()).ok_or_else(|| ApiError::bad_request("pid: a process id"))?;
                if target <= 1 || target == std::process::id() {
                    return Err(ApiError::bad_request("pick another process"));
                }
                let project = state.projects.require(&pid)?;
                let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
                let plan = launch::plan_attach(&state, &project, target, text("adapter").as_deref(), text("language").as_deref(), text("program").as_deref()).await?;
                let info = session::start(&state, project, plan, None).await?;
                started(&state, &pid, info, &args).await
            },
        ),
        tool(
            "debug_restart",
            "Rerun a session's configuration: stops the session, runs its build step again and starts a new one \
             (a new sessionId, in the answer). Waits like debug_start. Defaults to the project's only live session.",
            json!({ "type": "object", "properties": {
                "sessionId": { "type": "string" },
                "waitSeconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT },
                "frame": { "type": "integer" },
                "registers": { "type": "boolean" },
                "projectId": project_prop()
            } }),
            true,
            |state, ctx, args| async move {
                let pid = project_id(&ctx, &state, &args)?;
                let s = live_session(&state, &pid, &args)?;
                let project = state.projects.require(&pid)?;
                let info = session::restart(&state, project, &s).await?;
                started(&state, &pid, info, &args).await
            },
        ),
        tool(
            "debug_evaluate",
            "Evaluate an expression in a session that is stopped. `context` `watch` (default) gives the value of an \
             expression in a frame; `repl` runs a debugger console command (gdb: `info registers`, `x/16x $sp`, \
             `monitor reset halt`). The debugger runs it, so it can call functions in the program and change its \
             state; a `repl` command is echoed into the user's console marked as the agent's.",
            json!({ "type": "object", "properties": {
                "expression": { "type": "string" },
                "context": { "type": "string", "enum": ["watch", "repl"] },
                "frame": { "type": "integer", "description": "Stack frame (0 = innermost)." },
                "threadId": { "type": "integer" },
                "sessionId": { "type": "string" },
                "projectId": project_prop()
            }, "required": ["expression"] }),
            true,
            |state, ctx, args| async move {
                let pid = project_id(&ctx, &state, &args)?;
                let s = live_session(&state, &pid, &args)?;
                let expr = args.get("expression").and_then(Value::as_str).unwrap_or("").trim_end().to_string();
                if expr.trim().is_empty() || expr.len() > 10_000 || expr.contains('\0') {
                    return Err(ApiError::bad_request("an expression of up to 10000 characters"));
                }
                let context = if args.get("context").and_then(Value::as_str) == Some("repl") { "repl" } else { "watch" };
                let index = args.get("frame").and_then(Value::as_u64).unwrap_or(0).min(29) as usize;
                let fid = frame_id(&s, args.get("threadId").and_then(Value::as_i64), index).await;
                let r = session::evaluate_logged(&state, &s, &expr, fid, context, Some("agent")).await?;
                Ok(ToolOutput::Json(json!({ "sessionId": s.id, "expression": expr, "context": context, "result": variable_view(&s, &r, "result") })))
            },
        ),
        tool(
            "debug_control",
            "Steer a debug session the user started: `continue`, `pause`, `next` (step over), `stepIn`, `stepOut`, \
             `runTo` (with path and line) or `stop` (ends it; a launched program is terminated, an attach or a \
             microcontroller is detached). Continue, the steps and runTo wait until the program stops again, ends or \
             the wait is over, then answer like debug_state: where it stopped, the stack, the locals (the CPU \
             registers with `registers`) and the console's tail. The session defaults to the project's only live \
             one. To start, attach or rerun use debug_start, debug_attach and debug_restart.",
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
             {line, enabled?, condition?, hitCondition?, logMessage?}: a condition stops only when the debugger evaluates it true, a log message prints \
             `{expression}` instead of stopping; functions: {name, enabled?, condition?}.",
            json!({ "type": "object", "properties": {
                "action": { "type": "string", "enum": ["list", "add", "remove", "set", "functions", "mute", "clear"] },
                "path": { "type": "string", "description": "Project-relative file (add, remove, set)." },
                "breakpoints": { "type": "array", "items": { "type": "object" }, "description": "add, set: {line, enabled?, condition?, hitCondition?, logMessage?}; functions: {name, enabled?, condition?}." },
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
    fn waits_are_bounded() {
        assert_eq!(wait_arg(&json!({}), 15), Duration::from_secs(15));
        assert_eq!(wait_arg(&json!({ "waitSeconds": 0 }), 15), Duration::ZERO);
        assert_eq!(wait_arg(&json!({ "waitSeconds": 99999 }), 15), Duration::from_secs(MAX_WAIT));
    }
}

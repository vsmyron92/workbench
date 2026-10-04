//! MCP: `debug_state` (read-only). An agent sees what the user's debugger sees —
//! sessions, why the program stopped, the top of the stack, the locals of a frame, the
//! core registers of a halted microcontroller on request, and the console's tail —
//! but changes nothing: the requests made here (`stackTrace`, `scopes`, `variables`) only
//! read. The tools that act on a session are in `agent`.

use serde_json::{Value, json};

use super::routes::{frames_view, variable_view};
use super::session::{REQUEST_TIMEOUT, SessionState};
use crate::mcp::{McpTool, ToolOutput, tool};

const MAX_FRAMES: i64 = 30;
const MAX_LOCALS: usize = 60;
const MAX_REGISTERS: usize = 64;
const VALUE_CHARS: usize = 300;

fn short(v: &str, max: usize) -> String {
    if v.chars().count() <= max {
        return v.to_string();
    }
    let s: String = v.chars().take(max).collect();
    format!("{s}…")
}

/// The last `max` characters of `v`: the newest console output is what explains a
/// stop or a crash.
fn tail(v: &str, max: usize) -> String {
    let n = v.chars().count();
    if n <= max {
        return v.to_string();
    }
    let s: String = v.chars().skip(n - max).collect();
    format!("…{s}")
}

pub fn tools() -> Vec<McpTool> {
    vec![tool(
        "debug_state",
        "The state of the user's debug sessions in Workbench (gdb, lldb, debugpy, delve… over DAP): for each \
         session its launch configuration, state (starting|running|stopped|terminated|failed), and when stopped: \
         the stop reason, the threads, the call stack of the stopped thread (function, file:line) and the local \
         variables of one frame (values truncated), plus the last lines of the debug console (for a remote target \
         also the debug server's output: OpenOCD, J-Link, QEMU). With `registers` the CPU registers of the frame \
         too: what a HardFault on a microcontroller needs (pc, lr, sp, xpsr). Use it to help the user understand a \
         stop, a crash or a wrong value. Read-only; debug_start, debug_attach, debug_restart, debug_evaluate, debug_control and debug_breakpoints act.",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": "Workbench project id; defaults to the calling session's project." },
            "sessionId": { "type": "string", "description": "One debug session (default: every session of the project)." },
            "frame": { "type": "integer", "description": "Stack frame index for the locals (0 = the innermost, default)." },
            "threadId": { "type": "integer", "description": "Thread whose stack to show (default: the thread that stopped)." },
            "registers": { "type": "boolean", "description": "Also list the CPU registers of that frame (default: false)." }
        } }),
        false,
        |state, ctx, args| async move {
            let pid = ctx.project_for(args.get("projectId").and_then(Value::as_str))?;
            state.projects.require(&pid)?;
            let only = args.get("sessionId").and_then(Value::as_str);
            let frame_index = args.get("frame").and_then(Value::as_i64).unwrap_or(0).clamp(0, MAX_FRAMES - 1) as usize;
            let want_registers = args.get("registers").and_then(Value::as_bool).unwrap_or(false);
            let mut out = vec![];
            for s in state.debug.sessions_of(&pid) {
                if only.is_some_and(|o| o != s.id) {
                    continue;
                }
                out.push(report(&s, frame_index, args.get("threadId").and_then(Value::as_i64), want_registers).await);
            }
            if out.is_empty() {
                return Ok(ToolOutput::Text(format!("No debug sessions in project {pid}. The user starts them from the Debug tool window (Shift+F9).")));
            }
            Ok(ToolOutput::Json(json!({ "projectId": pid, "sessions": out })))
        },
    )]
}

/// The CPU registers of a frame (the `registers` scope), `name: value`, redacted like
/// every value; null when the adapter has no such scope.
async fn registers(s: &super::session::Session, frame_id: i64) -> Value {
    let Ok(body) = s.request("scopes", json!({ "frameId": frame_id }), REQUEST_TIMEOUT).await else { return Value::Null };
    let scope = body.get("scopes").and_then(Value::as_array).and_then(|a| {
        a.iter().find(|sc| sc.get("presentationHint").and_then(Value::as_str) == Some("registers") || sc.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case("registers")))
    });
    let Some(r) = scope.and_then(|sc| sc.get("variablesReference")).and_then(Value::as_i64).filter(|r| *r > 0) else { return Value::Null };
    let Ok(vars) = s.request("variables", json!({ "variablesReference": r }), REQUEST_TIMEOUT).await else { return Value::Null };
    let list = vars.get("variables").and_then(Value::as_array).cloned().unwrap_or_default();
    let map: serde_json::Map<String, Value> = list
        .iter()
        .take(MAX_REGISTERS)
        .map(|v| variable_view(s, v, "value"))
        .map(|v| (v["name"].as_str().unwrap_or("?").to_string(), json!(short(v["value"].as_str().unwrap_or(""), 80))))
        .collect();
    Value::Object(map)
}

/// One session as an agent reads it: configuration, state, and when stopped the reason,
/// threads, the stack of the stopped thread, a frame's locals (and its registers on request),
/// plus the console's tail. Only `stackTrace`, `scopes` and `variables` are sent.
pub(super) async fn report(s: &std::sync::Arc<super::session::Session>, frame_index: usize, thread: Option<i64>, registers_wanted: bool) -> Value {
    let info = s.info();
    let mut v = json!({
        "sessionId": info.id,
        "name": info.name,
        "adapter": info.adapter_label,
        "request": info.request,
        "state": info.state,
        "error": info.error,
        "exitCode": info.exit_code,
        "process": info.process,
    });
    if let Some(r) = &info.remote {
        v["remote"] = json!({ "server": r.server, "target": r.target });
    }
    if info.state == SessionState::Stopped {
        let stop = info.stopped.clone();
        v["stop"] = json!(stop.as_ref().map(|st| json!({
            "reason": st.reason, "description": st.description, "text": st.text, "threadId": st.thread_id
        })));
        v["threads"] = json!(info.threads.iter().take(40).collect::<Vec<_>>());
        let tid = thread.or(stop.and_then(|x| x.thread_id)).or(info.threads.first().map(|t| t.id));
        if let Some(tid) = tid {
            match s.request("stackTrace", json!({ "threadId": tid, "startFrame": 0, "levels": MAX_FRAMES }), REQUEST_TIMEOUT).await {
                Ok(body) => {
                    let frames = frames_view(s, &body);
                    let list = frames["frames"].as_array().cloned().unwrap_or_default();
                    v["stack"] = json!(list
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            let file = f.pointer("/source/path").and_then(Value::as_str).unwrap_or("?");
                            format!("#{i} {} at {file}:{}", f["name"].as_str().unwrap_or("?"), f["line"])
                        })
                        .collect::<Vec<_>>());
                    if let Some(frame_id) = list.get(frame_index).and_then(|f| f["id"].as_i64()) {
                        v["frame"] = json!(frame_index);
                        v["locals"] = locals(s, frame_id).await;
                        if registers_wanted {
                            v["registers"] = registers(s, frame_id).await;
                        }
                    }
                }
                Err(e) => v["stackError"] = json!(e.message),
            }
        }
    }
    // What the Live tab watches, with each value as last read from the running target.
    let live = s.live.values();
    if !live.is_empty() {
        v["live"] = json!(live);
    }
    let (lines, _) = s.output_after(info.output_seq.saturating_sub(40), 40);
    let text: String = lines.iter().filter(|l| l.category != "telemetry").map(|l| l.text.as_str()).collect();
    v["console"] = json!(tail(&text, 4000));
    v
}

/// Locals of a frame: every scope that is not marked expensive (registers are
/// left out), one level deep, values truncated.
async fn locals(s: &super::session::Session, frame_id: i64) -> Value {
    let Ok(body) = s.request("scopes", json!({ "frameId": frame_id }), REQUEST_TIMEOUT).await else { return Value::Null };
    let mut scopes = vec![];
    let mut total = 0;
    for sc in body.get("scopes").and_then(Value::as_array).cloned().unwrap_or_default() {
        let name = sc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let hint = sc.get("presentationHint").and_then(Value::as_str).unwrap_or("");
        let lower = name.to_ascii_lowercase();
        let noise = matches!(lower.as_str(), "registers" | "globals" | "global" | "statics" | "static");
        if sc.get("expensive").and_then(Value::as_bool).unwrap_or(false) || hint == "registers" || noise {
            continue;
        }
        let Some(r) = sc.get("variablesReference").and_then(Value::as_i64).filter(|r| *r > 0) else { continue };
        let Ok(vars) = s.request("variables", json!({ "variablesReference": r }), REQUEST_TIMEOUT).await else { continue };
        let list: Vec<Value> = vars
            .get("variables")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .take(MAX_LOCALS.saturating_sub(total))
            .map(|v| {
                let v = variable_view(s, v, "value");
                json!({ "name": v["name"], "type": v["type"], "value": short(v["value"].as_str().unwrap_or(""), VALUE_CHARS) })
            })
            .collect();
        total += list.len();
        scopes.push(json!({ "scope": name, "variables": list }));
        if total >= MAX_LOCALS {
            break;
        }
    }
    json!(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_tail_keeps_the_newest_output() {
        let text = format!("{}FINAL LINE", "x".repeat(5000));
        let t = tail(&text, 4000);
        assert!(t.ends_with("FINAL LINE"), "{}", &t[t.len() - 20..]);
        assert_eq!(t.chars().count(), 4001);
        assert!(t.starts_with('…'));
        assert_eq!(tail("short", 4000), "short");
        // Characters, not bytes.
        assert_eq!(tail("ééé", 2), "…éé");
    }
}

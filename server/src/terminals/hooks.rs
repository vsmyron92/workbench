//! Claude Code HTTP hooks and status line intake → `AgentState`.
//!
//! Each hosted session gets `--settings` with HTTP hooks posting to
//! `/api/hooks/claude/<terminalId>` (Bearer: the terminal's agent token). Handlers
//! answer `200 {}` immediately and never decide anything, with one exception: a
//! `PermissionRequest` is held until a device answers it from Workbench, or it stops
//! being pending (`permission`); Claude's own dialog stays usable meanwhile.
//!
//! The state machine is a pure function over the hook payload so it can be tested.

use serde_json::Value;

use super::permission::{self, OpenTools, Queue};
use super::transcript::{clean_line, is_uuid, tail_chars};
use super::{AgentInfo, AgentState};

/// Runtime-only agent bookkeeping (not persisted).
#[derive(Debug, Default)]
pub struct AgentRt {
    /// At least one hook arrived: the transcript is enrichment only, not the state source.
    pub hooks_seen: bool,
    /// The tool call waiting for a permission answer (its tool_use_id; `?` when unknown,
    /// which any tool call's end resolves).
    pub pending_permission: Option<String>,
    /// Claude Code: permission requests Workbench can answer, oldest first.
    pub permissions: Queue,
    /// Claude Code: tool calls between `PreToolUse` and their end.
    pub open_tools: OpenTools,
    /// Dedupes transcript completions (thinking and text are separate records sharing an id).
    pub last_completed_id: Option<String>,
    /// The startup "trust this folder?" dialog is on screen (no hook reports it).
    pub trust_prompt: bool,
    /// Codex: rollout event bookkeeping.
    pub codex: super::codex::Tracker,
    /// Codex and Kimi: the process start (ms) while the session id is still unknown.
    pub pending_since: Option<i64>,
    /// Codex: the session a fork continues (its rollout names it).
    pub fork_of: Option<String>,
    /// Codex and Kimi: where the CLI keeps its files (`CODEX_HOME`, `KIMI_CODE_HOME`).
    pub home: Option<std::path::PathBuf>,
    /// Kimi, while its id is unknown: the index ids that existed at launch.
    pub kimi_known: Option<std::sync::Arc<std::collections::HashSet<String>>>,
    /// Codex, Kimi and custom CLIs: a dialog of the CLI recognized on screen, as
    /// `(the state it set, the state before it)`.
    pub screen_dialog: Option<(AgentState, AgentState)>,
    /// Checks in a row that no longer saw `screen_dialog` (a redraw can hide it briefly).
    pub dialog_misses: u8,
    /// Codex and Kimi: a turn ended in this process (Codex: its rollout said so; Kimi:
    /// output flowed and stopped). `ask` without a terminal only picks such a session:
    /// right after startup a trust or login dialog may be up.
    pub turn_done: bool,
}

/// What a hook changed.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub changed: bool,
    /// Emit `agent.attention` (needs the user, turn complete, error).
    pub attention: bool,
    pub session_started: bool,
    /// The session moved to a new transcript (`/clear`, fork, resume).
    pub transcript_path: Option<String>,
    /// PermissionRequest: the tool call it is for, when known.
    pub permission_call: Option<String>,
}

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// `"effort": "high"` or `"effort": {"level": "high"}`.
fn effort_of(v: &Value) -> Option<String> {
    match v.get("effort")? {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("level").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

/// `"model": "claude-…"` or `"model": {"id": …, "display_name": …}`.
fn model_of(v: &Value) -> Option<String> {
    match v.get("model")? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(o) => o
            .get("display_name")
            .or_else(|| o.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn set<T: PartialEq>(slot: &mut T, value: T, changed: &mut bool) {
    if *slot != value {
        *slot = value;
        *changed = true;
    }
}

/// A one-line description of what a permission prompt asks for. File paths under the
/// session's `cwd` are shown relative to it. What looks like a credential is masked
/// before anything is cut (`permission::redact_patterns`); invisible characters are
/// shown. Cut and on one line, it is never the whole request (`PendingPermission.detail`).
pub fn describe_permission(tool: Option<&str>, input: Option<&Value>, cwd: Option<&str>) -> String {
    let tool = tool.unwrap_or("a tool");
    let field = |k: &str| input.and_then(|i| i.get(k)).and_then(Value::as_str).map(|v| permission::redact_patterns(&permission::show_invisible(v)));
    let short = |p: &str| -> String { cwd.and_then(|c| crate::util::os::path::below_dir(p, c)).unwrap_or(p).to_string() };
    let what = match tool {
        "Bash" => field("command").map(|c| format!("run `{}`", clean_line(&c, 140))),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Read" => {
            field("file_path").or_else(|| field("notebook_path")).map(|p| format!("{} {}", tool.to_lowercase(), clean_line(&short(&p), 160)))
        }
        "WebFetch" => field("url").map(|u| format!("fetch {}", clean_line(&u, 160))),
        "WebSearch" => field("query").map(|q| format!("search the web for “{}”", clean_line(&q, 80))),
        _ => None,
    };
    match what {
        Some(w) => format!("Permission to {w}"),
        None => format!("Permission to use {}", clean_line(&permission::show_invisible(tool), 80)),
    }
}

/// Keep `AgentInfo.pendingPermission` in line with the queue. Returns whether it changed.
pub fn sync_pending(agent: &mut AgentInfo, rt: &AgentRt) -> bool {
    let now = rt.permissions.current();
    if agent.pending_permission != now {
        agent.pending_permission = now;
        return true;
    }
    false
}

/// Apply one hook payload.
pub fn apply_hook(agent: &mut AgentInfo, rt: &mut AgentRt, v: &Value, now: i64) -> Outcome {
    let mut out = Outcome::default();
    let mut ch = false;
    rt.hooks_seen = true;
    agent.last_event_at = now;
    let event = str_at(v, "hook_event_name").unwrap_or("");

    if let Some(id) = str_at(v, "session_id").filter(|s| is_uuid(s)) {
        set(&mut agent.session_id, id.to_string(), &mut ch);
    }
    if let Some(pm) = str_at(v, "permission_mode") {
        set(&mut agent.permission_mode, Some(pm.to_string()), &mut ch);
    }
    if let Some(e) = effort_of(v) {
        set(&mut agent.effort, Some(e), &mut ch);
    }
    if let Some(tp) = str_at(v, "transcript_path") {
        out.transcript_path = Some(tp.to_string());
    }
    let prev = agent.state;
    let message = str_at(v, "message").map(|m| clean_line(m, 300));
    let tool_use_id = str_at(v, "tool_use_id");
    let tool_name = str_at(v, "tool_name");
    let tool_key = tool_use_id.or(tool_name).map(str::to_string);

    match event {
        "SessionStart" => {
            set(&mut agent.state, AgentState::Idle, &mut ch);
            set(&mut agent.attention, None, &mut ch);
            if let Some(m) = model_of(v) {
                set(&mut agent.model, Some(m), &mut ch);
            }
            rt.pending_permission = None;
            rt.permissions.clear();
            rt.open_tools.clear();
            rt.trust_prompt = false;
            out.session_started = true;
        }
        "UserPromptSubmit" => {
            set(&mut agent.state, AgentState::Working, &mut ch);
            set(&mut agent.unread, false, &mut ch);
            set(&mut agent.attention, None, &mut ch);
            rt.pending_permission = None;
            rt.permissions.clear();
            rt.open_tools.clear();
        }
        "PreToolUse" | "PreCompact" => {
            if let (Some(id), Some(name)) = (tool_use_id, tool_name) {
                rt.open_tools.started(id, name, v.get("tool_input"));
            }
            if rt.pending_permission.is_none() {
                set(&mut agent.state, AgentState::Working, &mut ch);
                set(&mut agent.attention, None, &mut ch);
            }
        }
        "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
            if let Some(id) = tool_use_id {
                rt.open_tools.finished(id);
                // Its permission was answered, wherever that happened.
                rt.permissions.resolve_tool_use(id);
            }
            let resolves = match (&rt.pending_permission, &tool_key) {
                (Some(p), _) if p == "?" => {
                    // A prompt whose call is unknown ends with any call.
                    rt.permissions.resolve_first_unkeyed();
                    true
                }
                (Some(p), Some(k)) => p == k,
                (Some(_), None) => true,
                (None, _) => true,
            };
            if resolves {
                // A dialog answered from Workbench is settled too: its call ended.
                rt.permissions.drop_awaiting();
                match rt.permissions.current() {
                    // The next queued dialog is on screen now.
                    Some(next) => {
                        rt.pending_permission = Some(rt.permissions.current_tool_use().unwrap_or_else(|| "?".into()));
                        set(&mut agent.state, AgentState::NeedsPermission, &mut ch);
                        set(&mut agent.attention, Some(next.summary), &mut ch);
                    }
                    None => {
                        rt.pending_permission = None;
                        set(&mut agent.state, AgentState::Working, &mut ch);
                        set(&mut agent.attention, None, &mut ch);
                    }
                }
            }
        }
        "PermissionRequest" => {
            // Claude asks again: a dialog answered from Workbench before was taken.
            rt.permissions.drop_awaiting();
            // Its payload has no tool_use_id: the PreToolUse before it names the call.
            let call = tool_use_id.map(str::to_string).or_else(|| rt.open_tools.find(tool_name.unwrap_or(""), v.get("tool_input")));
            out.permission_call = call.clone();
            // Claude shows one dialog at a time: a request queued behind another does not
            // change what the session shows.
            if rt.permissions.is_empty() {
                rt.pending_permission = Some(call.unwrap_or_else(|| "?".into()));
                let text = match tool_name.and_then(permission::terminal_only_text) {
                    Some(t) => t.to_string(),
                    None => describe_permission(tool_name, v.get("tool_input"), str_at(v, "cwd")),
                };
                set(&mut agent.attention, Some(text), &mut ch);
            }
            set(&mut agent.state, AgentState::NeedsPermission, &mut ch);
            out.attention = prev != AgentState::NeedsPermission;
        }
        "Notification" => match str_at(v, "notification_type").unwrap_or("") {
            "permission_prompt" => {
                if prev != AgentState::NeedsPermission {
                    rt.pending_permission.get_or_insert_with(|| "?".into());
                    set(&mut agent.state, AgentState::NeedsPermission, &mut ch);
                    set(&mut agent.attention, Some(message.unwrap_or_else(|| "Claude needs your permission".into())), &mut ch);
                    out.attention = true;
                }
            }
            "idle_prompt" => {
                // Claude has waited ~a minute for input. After a finished turn the session
                // is already idle (and its answer possibly unread): keep that state, so the
                // answer stays visible and it does not outrank sessions that are blocked.
                // Otherwise (a Stop we never saw) it is waiting on the user.
                if prev != AgentState::Idle && prev != AgentState::NeedsInput {
                    set(&mut agent.state, AgentState::NeedsInput, &mut ch);
                    set(&mut agent.attention, Some(message.unwrap_or_else(|| "Waiting for your input".into())), &mut ch);
                    out.attention = !agent.unread;
                }
            }
            "agent_needs_input" | "elicitation_dialog" | "elicitation_url_dialog" => {
                set(&mut agent.state, AgentState::NeedsInput, &mut ch);
                set(&mut agent.attention, Some(message.unwrap_or_else(|| "Claude needs your input".into())), &mut ch);
                out.attention = prev != AgentState::NeedsInput;
            }
            "elicitation_complete" | "elicitation_response" | "auth_success" => {
                if prev == AgentState::NeedsInput {
                    set(&mut agent.state, AgentState::Working, &mut ch);
                    set(&mut agent.attention, None, &mut ch);
                }
            }
            _ => {}
        },
        "Stop" => {
            rt.pending_permission = None;
            rt.permissions.clear();
            rt.open_tools.clear();
            set(&mut agent.state, AgentState::Idle, &mut ch);
            set(&mut agent.attention, None, &mut ch);
            set(&mut agent.unread, true, &mut ch);
            if let Some(m) = str_at(v, "last_assistant_message") {
                set(&mut agent.last_message, Some(tail_chars(m, 600)), &mut ch);
            }
            out.attention = true;
        }
        "StopFailure" => {
            rt.pending_permission = None;
            rt.permissions.clear();
            rt.open_tools.clear();
            let err = str_at(v, "error")
                .or_else(|| v.pointer("/error/message").and_then(Value::as_str))
                .or_else(|| str_at(v, "error_message"))
                .or_else(|| str_at(v, "reason"))
                .map(|e| clean_line(e, 300))
                .unwrap_or_else(|| "The turn failed".into());
            set(&mut agent.state, AgentState::Error, &mut ch);
            set(&mut agent.attention, Some(err), &mut ch);
            out.attention = true;
        }
        "PostCompact" => {
            // A manual /compact ends idle; an automatic one continues the turn.
            if str_at(v, "trigger") == Some("manual") {
                set(&mut agent.state, AgentState::Idle, &mut ch);
            }
        }
        "SessionEnd" => {
            rt.pending_permission = None;
            rt.permissions.clear();
            rt.open_tools.clear();
            set(&mut agent.state, AgentState::Exited, &mut ch);
            set(&mut agent.attention, None, &mut ch);
        }
        _ => {}
    }
    if sync_pending(agent, rt) {
        ch = true;
    }
    out.changed = ch;
    out
}

/// Apply a status line payload. Returns whether anything visible changed. The payload's
/// `session_name` is not used: for unnamed sessions it is a derived name that would
/// hide the better automatic title.
pub fn apply_status(agent: &mut AgentInfo, v: &Value) -> bool {
    let mut ch = false;
    if let Some(m) = v.pointer("/model/display_name").and_then(Value::as_str).filter(|m| !m.is_empty()) {
        set(&mut agent.model, Some(clean_line(m, 60)), &mut ch);
    }
    let pct = v.pointer("/context_window/used_percentage").and_then(Value::as_f64).or_else(|| {
        let used = v.pointer("/context_window/total_input_tokens").and_then(Value::as_f64)?;
        let size = v.pointer("/context_window/context_window_size").and_then(Value::as_f64)?;
        (size > 0.0).then(|| used * 100.0 / size)
    });
    if let Some(p) = pct.filter(|p| p.is_finite()) {
        let p = (p.clamp(0.0, 100.0) * 10.0).round() / 10.0;
        set(&mut agent.context_pct, Some(p), &mut ch);
    }
    if let Some(c) = v.pointer("/cost/total_cost_usd").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0) {
        let c = (c * 1000.0).round() / 1000.0;
        set(&mut agent.cost_usd, Some(c), &mut ch);
    }
    if let Some(e) = effort_of(v) {
        set(&mut agent.effort, Some(e), &mut ch);
    }
    ch
}

/// The compact status line printed by `workbench statusline`.
pub fn status_line_text(v: &Value) -> String {
    let mut parts: Vec<String> = vec![];
    if let Some(m) = v.pointer("/model/display_name").and_then(Value::as_str) {
        parts.push(m.to_string());
    }
    if let Some(p) = v.pointer("/context_window/used_percentage").and_then(Value::as_f64) {
        parts.push(format!("{}% ctx", p.round() as i64));
    }
    if let Some(c) = v.pointer("/cost/total_cost_usd").and_then(Value::as_f64) {
        parts.push(format!("${c:.2}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent() -> AgentInfo {
        AgentInfo { session_id: "00000000-0000-4000-8000-000000000000".into(), ..crate::terminals::test_agent() }
    }

    #[test]
    fn full_turn_with_a_permission_prompt() {
        let mut a = agent();
        let mut rt = AgentRt::default();
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"SessionStart","source":"startup","model":"claude-haiku-4-5","transcript_path":"/t/x.jsonl","permission_mode":"default"}), 1);
        assert!(o.session_started && o.changed);
        assert_eq!(o.transcript_path.as_deref(), Some("/t/x.jsonl"));
        assert_eq!(a.state, AgentState::Idle);
        assert_eq!(a.model.as_deref(), Some("claude-haiku-4-5"));

        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"UserPromptSubmit","prompt":"hi"}), 2);
        assert_eq!(a.state, AgentState::Working);

        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"t1"}), 3);
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"npm test"}}), 4);
        assert!(o.attention);
        assert_eq!(a.state, AgentState::NeedsPermission);
        assert_eq!(a.attention.as_deref(), Some("Permission to run `npm test`"));

        // The follow-up notification does not notify twice.
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}), 5);
        assert!(!o.attention);
        assert_eq!(a.attention.as_deref(), Some("Permission to run `npm test`"));

        // Another parallel tool finishing does not clear the pending prompt.
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Read","tool_use_id":"t2"}), 6);
        assert_eq!(a.state, AgentState::NeedsPermission);
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"t1"}), 7);
        assert_eq!(a.state, AgentState::Working);
        assert_eq!(a.attention, None);

        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Stop","last_assistant_message":"All tests pass."}), 8);
        assert!(o.attention);
        assert_eq!(a.state, AgentState::Idle);
        assert!(a.unread);
        assert_eq!(a.last_message.as_deref(), Some("All tests pass."));

        // idle_prompt after a finished turn changes nothing: the answer stays visible.
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Notification","notification_type":"idle_prompt","message":"Claude is waiting for your input"}), 9);
        assert!(!o.attention);
        assert_eq!(a.state, AgentState::Idle);
        assert!(a.unread);
        // Without a Stop (e.g. a hook was lost) it means the session waits on the user.
        let mut b = agent();
        b.state = AgentState::Working;
        let o = apply_hook(&mut b, &mut AgentRt::default(), &json!({"hook_event_name":"Notification","notification_type":"idle_prompt","message":"Claude is waiting for your input"}), 1);
        assert!(o.attention);
        assert_eq!(b.state, AgentState::NeedsInput);

        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"UserPromptSubmit"}), 10);
        assert!(!a.unread);
        assert_eq!(a.last_event_at, 10);
    }

    #[test]
    fn failures_input_requests_and_session_end() {
        let mut a = agent();
        let mut rt = AgentRt::default();
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"StopFailure","error":"rate_limit"}), 1);
        assert!(o.attention);
        assert_eq!(a.state, AgentState::Error);
        assert_eq!(a.attention.as_deref(), Some("rate_limit"));

        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Notification","notification_type":"elicitation_dialog","message":"Pick one"}), 2);
        assert!(o.attention);
        assert_eq!(a.state, AgentState::NeedsInput);
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Notification","notification_type":"elicitation_complete"}), 3);
        assert_eq!(a.state, AgentState::Working);

        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"SessionEnd","reason":"exit"}), 4);
        assert_eq!(a.state, AgentState::Exited);
    }

    #[test]
    fn session_id_and_effort_follow_the_session() {
        let mut a = agent();
        let mut rt = AgentRt::default();
        let new_id = "11111111-2222-4333-8444-555555555555";
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"SessionStart","source":"clear","session_id":new_id,"effort":{"level":"high"}}), 1);
        assert!(o.changed);
        assert_eq!(a.session_id, new_id);
        assert_eq!(a.effort.as_deref(), Some("high"));
        // Garbage ids are ignored.
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PreToolUse","session_id":"../../etc"}), 2);
        assert_eq!(a.session_id, new_id);
    }

    #[test]
    fn status_line_updates_only_on_change() {
        let mut a = agent();
        let v = json!({"model":{"display_name":"Haiku 4.5"},"context_window":{"used_percentage":23.04},"cost":{"total_cost_usd":1.2004},"session_name":"fix-ci"});
        let ch = apply_status(&mut a, &v);
        assert!(ch);
        assert_eq!(a.context_pct, Some(23.0));
        assert_eq!(a.cost_usd, Some(1.2));
        assert!(!apply_status(&mut a, &v));
        assert_eq!(status_line_text(&v), "Haiku 4.5 · 23% ctx · $1.20");
        assert_eq!(status_line_text(&json!({})), "");
    }

    /// A PermissionRequest as Claude Code 2.1.283 sends it (no tool_use_id), queued the
    /// way `Terminals::permission_request` queues it.
    fn request(a: &mut AgentInfo, rt: &mut AgentRt, id: &str, cmd: &str, t: i64) -> tokio::sync::oneshot::Receiver<Value> {
        let v = json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":cmd},"cwd":"/w/p"});
        let o = apply_hook(a, rt, &v, t);
        let (r, rx) = permission::Request::new(id.into(), &v, o.permission_call, &[], t);
        rt.permissions.push(r);
        sync_pending(a, rt);
        rx
    }

    fn pre_tool(a: &mut AgentInfo, rt: &mut AgentRt, id: &str, cmd: &str, t: i64) {
        apply_hook(a, rt, &json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":id,"tool_input":{"command":cmd}}), t);
    }

    #[test]
    fn permission_requests_are_pending_until_answered_anywhere() {
        use tokio::sync::oneshot::error::TryRecvError;
        let mut a = agent();
        let mut rt = AgentRt::default();
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"UserPromptSubmit"}), 1);

        // Pending → answered from Workbench.
        pre_tool(&mut a, &mut rt, "t1", "touch a.txt", 2);
        let mut rx = request(&mut a, &mut rt, "p1", "touch a.txt", 3);
        let p = a.pending_permission.clone().expect("pending");
        assert_eq!((p.id.as_str(), p.tool.as_str(), p.summary.as_str()), ("p1", "Bash", "Permission to run `touch a.txt`"));
        assert_eq!(rt.pending_permission.as_deref(), Some("t1"), "found through its PreToolUse");
        rt.permissions.take("p1").unwrap().answer(&permission::Decision::Allow { session: false }).unwrap();
        assert_eq!(rx.try_recv().unwrap()["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert!(sync_pending(&mut a, &rt) && a.pending_permission.is_none());
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"t1"}), 4);
        assert_eq!(a.state, AgentState::Working);

        // Pending → answered in the terminal: its tool call ends, the hook gets no decision.
        pre_tool(&mut a, &mut rt, "t2", "touch b.txt", 5);
        let mut rx = request(&mut a, &mut rt, "p2", "touch b.txt", 6);
        assert_eq!(a.state, AgentState::NeedsPermission);
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"t2"}), 7);
        assert!(o.changed);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Closed)));
        assert_eq!((a.state, a.pending_permission.as_ref()), (AgentState::Working, None));

        // Two parallel calls: the second waits behind the first.
        pre_tool(&mut a, &mut rt, "t3", "touch c.txt", 8);
        pre_tool(&mut a, &mut rt, "t4", "touch d.txt", 8);
        let _rx3 = request(&mut a, &mut rt, "p3", "touch c.txt", 9);
        let _rx4 = request(&mut a, &mut rt, "p4", "touch d.txt", 9);
        assert_eq!(a.pending_permission.as_ref().map(|p| p.id.as_str()), Some("p3"));
        assert_eq!(a.attention.as_deref(), Some("Permission to run `touch c.txt`"), "the first dialog is the one on screen");
        assert_eq!(rt.pending_permission.as_deref(), Some("t3"));
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"t3"}), 10);
        assert_eq!(a.pending_permission.as_ref().map(|p| p.id.as_str()), Some("p4"));
        assert_eq!((a.state, a.attention.as_deref()), (AgentState::NeedsPermission, Some("Permission to run `touch d.txt`")));
        assert_eq!(rt.pending_permission.as_deref(), Some("t4"));

        // The turn ends (e.g. declined in the terminal, then Stop): nothing stays pending.
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Stop"}), 11);
        assert!(o.changed && a.pending_permission.is_none() && rt.permissions.is_empty());

        // Timed out: the handler removes it.
        pre_tool(&mut a, &mut rt, "t5", "rm x", 12);
        let mut rx = request(&mut a, &mut rt, "p5", "rm x", 13);
        assert!(rt.permissions.remove("p5"));
        assert!(sync_pending(&mut a, &rt) && a.pending_permission.is_none());
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Closed)));

        // The session ends while one is pending.
        let mut rx = request(&mut a, &mut rt, "p6", "rm y", 14);
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"SessionEnd","reason":"exit"}), 15);
        assert!(a.pending_permission.is_none());
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Closed)));
        assert_eq!(a.state, AgentState::Exited);
    }

    #[test]
    fn a_permission_prompt_without_a_known_call_ends_with_any_tool() {
        let mut a = agent();
        let mut rt = AgentRt::default();
        // No PreToolUse was seen (hooks disabled for it, say): the key is unknown.
        let _rx = request(&mut a, &mut rt, "p1", "make", 1);
        assert_eq!(rt.pending_permission.as_deref(), Some("?"));
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"tx"}), 2);
        assert_eq!(a.state, AgentState::Working);
        // The notification alone (no PermissionRequest hook) is resolved the same way.
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}), 3);
        assert_eq!(a.state, AgentState::NeedsPermission);
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"ty"}), 4);
        assert_eq!(a.state, AgentState::Working);
    }

    #[test]
    fn requests_only_the_terminal_answers_say_so() {
        let mut a = agent();
        let mut rt = AgentRt::default();
        let o = apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PermissionRequest","tool_name":"ExitPlanMode","tool_input":{"plan":"1. x"}}), 1);
        assert!(o.attention);
        assert_eq!((a.state, a.attention.as_deref()), (AgentState::NeedsPermission, Some("Claude asks you to approve its plan — answer in the terminal")));
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"ExitPlanMode","tool_use_id":"t1"}), 2);
        assert_eq!(a.state, AgentState::Working);
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PermissionRequest","tool_name":"AskUserQuestion","tool_input":{}}), 3);
        assert_eq!(a.attention.as_deref(), Some("Claude asks you a question — answer in the terminal"));
    }

    #[test]
    fn an_awaited_answer_is_settled_by_its_call_ending() {
        let mut a = agent();
        let mut rt = AgentRt::default();
        pre_tool(&mut a, &mut rt, "t1", "touch a", 1);
        let _rx = request(&mut a, &mut rt, "p1", "touch a", 2);
        // Answered from a device while its dialog is up (as `answer_permission` does).
        rt.permissions.take("p1").unwrap().answer(&permission::Decision::Allow { session: false }).unwrap();
        rt.permissions.await_answer("t1".into());
        sync_pending(&mut a, &rt);
        assert_eq!(a.state, AgentState::NeedsPermission);
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"t1"}), 3);
        assert_eq!(a.state, AgentState::Working);
        assert!(!rt.permissions.watching());
        // A new request means Claude went on.
        rt.permissions.await_answer("t2".into());
        apply_hook(&mut a, &mut rt, &json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"ls"}}), 4);
        assert!(rt.permissions.awaiting().is_none());
    }

    #[test]
    fn permission_descriptions() {
        assert_eq!(describe_permission(Some("Edit"), Some(&json!({"file_path":"/a/b.rs"})), None), "Permission to edit /a/b.rs");
        assert_eq!(describe_permission(Some("Write"), Some(&json!({"file_path":"/w/p/src/x.rs"})), Some("/w/p")), "Permission to write src/x.rs");
        assert_eq!(describe_permission(Some("Write"), Some(&json!({"file_path":"/w/pp/x.rs"})), Some("/w/p")), "Permission to write /w/pp/x.rs");
        assert_eq!(describe_permission(Some("Read"), Some(&json!({"file_path":"/etc/hosts"})), Some("/")), "Permission to read /etc/hosts");
        // Windows: below the cwd in any case, with `\` or `/`; Unix compares the strings.
        let edit = describe_permission(Some("Edit"), Some(&json!({"file_path": r"C:\Users\me\Proj\src\a.rs"})), Some(r"c:\users\me\proj\"));
        let write = describe_permission(Some("Write"), Some(&json!({"file_path": "C:/Users/me/Proj/b.rs"})), Some(r"C:\Users\me\Proj"));
        if cfg!(windows) {
            assert_eq!((edit.as_str(), write.as_str()), (r"Permission to edit src\a.rs", "Permission to write b.rs"));
        } else {
            assert_eq!((edit.as_str(), write.as_str()), (r"Permission to edit C:\Users\me\Proj\src\a.rs", "Permission to write C:/Users/me/Proj/b.rs"));
        }
        assert_eq!(describe_permission(Some("mcp__x__y"), None, None), "Permission to use mcp__x__y");
        assert_eq!(describe_permission(None, None, None), "Permission to use a tool");
        // Invisible characters are shown, never passed through.
        assert_eq!(describe_permission(Some("Bash"), Some(&json!({"command":"ls \u{202E}fdp.exe"})), None), "Permission to run `ls ⟨U+202E⟩fdp.exe`");
    }
}

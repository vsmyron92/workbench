//! Notifications outside the browser, and the activity log's event intake.
//!
//! A background task follows the event bus. Agent attention, environment
//! down/up transitions, finished deploys and failed pipelines become activity
//! records and, when `[notify]` allows, a desktop notification (`notify-send`)
//! and/or the user's `[notify].command`. The command runs through `sh -c` with the
//! text in `WORKBENCH_TITLE` / `WORKBENCH_MESSAGE` environment variables: the text
//! is never interpolated into the command line.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::events::Event;
use crate::util;

use super::activity::record_event;
use super::push::{self, AgentWait, OpenTarget, PushNote, Topic, Urgency};

/// Default minimum gap between two notifications with the same key (one env, one pipeline ref…).
const PER_KEY_INTERVAL: Duration = Duration::from_secs(20);
/// Gap that collapses duplicate reports of one blocking prompt (a hook and the
/// transcript can both report it). A new prompt comes after the user answered
/// the previous one, so it is further apart than this.
const REPEAT_INTERVAL: Duration = Duration::from_secs(5);
/// At most this many ordinary notifications per minute overall.
const GLOBAL_PER_MINUTE: usize = 8;
/// Urgent notifications skip that budget; this ceiling only stops a runaway.
const URGENT_PER_MINUTE: usize = 30;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl NoteLevel {
    pub fn parse(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "warning" => Self::Warning,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
    fn urgency(self) -> &'static str {
        match self {
            Self::Error => "critical",
            Self::Warning => "normal",
            _ => "low",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Error => "dialog-error",
            Self::Warning => "dialog-warning",
            _ => "dialog-information",
        }
    }
}

/// How a notification is rate-limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Dropped when the global per-minute budget is spent.
    Normal,
    /// Someone waits on the user (an agent is blocked, an environment went down):
    /// counted, but only a runaway ceiling drops it.
    Urgent,
    /// Never rate-limited (a finished deploy, the test button).
    Always,
}

pub struct DesktopNote {
    /// Rate-limit key: a repeat with the same key inside `repeat_after` is dropped.
    pub key: String,
    pub repeat_after: Duration,
    pub priority: Priority,
    pub title: String,
    pub body: String,
    pub level: NoteLevel,
    /// What caused it (`agent.attention`, `env.health`…), passed to the command.
    pub event: &'static str,
    /// The same news for devices with Web Push, sent when the note passes the limiter.
    pub push: Option<PushNote>,
}

/// What happened to a notification, for the test button and the notify tool.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOutcome {
    /// sent | disabled | unavailable | rate-limited
    pub desktop: &'static str,
    /// ran | none | rate-limited
    pub command: &'static str,
    /// queued | none (no device subscribed) | off | full | rate-limited | n/a
    pub push: &'static str,
}

/// Sliding-window limiter: per key and global.
#[derive(Default)]
pub struct RateLimiter {
    /// Key → until when a repeat is dropped.
    quiet_until: HashMap<String, Instant>,
    /// Everything allowed in the last minute.
    recent: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn allow(&mut self, key: &str, repeat_after: Duration, priority: Priority, now: Instant) -> bool {
        while self.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.recent.pop_front();
        }
        let budget = match priority {
            Priority::Normal => GLOBAL_PER_MINUTE,
            Priority::Urgent => URGENT_PER_MINUTE,
            Priority::Always => usize::MAX,
        };
        if self.recent.len() >= budget {
            return false;
        }
        if self.quiet_until.get(key).is_some_and(|t| now < *t) {
            return false;
        }
        if self.quiet_until.len() > 512 {
            self.quiet_until.retain(|_, t| now < *t);
        }
        self.quiet_until.insert(key.to_string(), now + repeat_after);
        self.recent.push_back(now);
        true
    }
}

#[derive(Default)]
pub struct Notifier {
    limiter: Mutex<RateLimiter>,
    /// Keys of the notifications that passed the limiter.
    #[cfg(test)]
    sent: Mutex<Vec<String>>,
}

/// Control characters out, length bounded (titles and bodies of notifications).
fn clean(s: &str, max: usize) -> String {
    let s: String = s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
    super::truncate_chars(s.trim(), max)
}

/// notify-send bodies may be interpreted as markup by the notification server.
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Notifier {
    #[cfg(test)]
    pub fn sent_keys(&self) -> Vec<String> {
        self.sent.lock().clone()
    }

    /// Deliver a notification in the background. Never blocks on the child processes.
    pub fn send(&self, state: &AppState, note: DesktopNote) -> SendOutcome {
        let (desktop, command) = {
            let cfg = state.config.read();
            (cfg.notify.desktop, cfg.notify.command.clone().filter(|c| !c.trim().is_empty()))
        };
        if !self.limiter.lock().allow(&note.key, note.repeat_after, note.priority, Instant::now()) {
            tracing::debug!(key = note.key, "notification rate-limited");
            return SendOutcome { desktop: "rate-limited", command: "rate-limited", push: "rate-limited" };
        }
        #[cfg(test)]
        self.sent.lock().push(note.key.clone());
        let push_status = match note.push {
            Some(p) => state.platform.push.enqueue(p),
            None => "n/a",
        };
        let title = clean(&note.title, 120);
        let body = clean(&note.body, 500);

        let desktop_status = if !desktop {
            "disabled"
        } else if let Some(bin) = util::which_path("notify-send") {
            let mut cmd = tokio::process::Command::new(bin);
            cmd.arg("--app-name=Workbench")
                .arg(format!("--urgency={}", note.level.urgency()))
                .arg(format!("--icon={}", note.level.icon()))
                // End of options: the title and body are never parsed as flags.
                .arg("--")
                .arg(&title)
                .arg(escape_markup(&body));
            tokio::spawn(async move {
                match util::proc::run_cmd(cmd, Duration::from_secs(10)).await {
                    Ok(out) if out.ok() => {}
                    Ok(out) => tracing::debug!("notify-send exited with {:?}", out.code),
                    Err(e) => tracing::debug!("notify-send failed: {e}"),
                }
            });
            "sent"
        } else {
            "unavailable"
        };

        let command_status = match command {
            None => "none",
            Some(command) => {
                let mut cmd = util::os::shell::plain_command(&command);
                cmd.env("WORKBENCH_TITLE", &title)
                    .env("WORKBENCH_MESSAGE", &body)
                    .env("WORKBENCH_LEVEL", note.level.as_str())
                    .env("WORKBENCH_EVENT", note.event)
                    .current_dir(dirs::home_dir().unwrap_or_else(|| "/".into()));
                tokio::spawn(async move {
                    // Output is not logged: user commands may print anything, credentials included.
                    match util::proc::run_cmd(cmd, COMMAND_TIMEOUT).await {
                        Ok(out) if out.ok() => {}
                        Ok(out) => tracing::warn!("[notify].command exited with {:?}", out.code),
                        Err(e) => tracing::warn!("[notify].command failed: {}", e.message),
                    }
                });
                "ran"
            }
        };
        SendOutcome { desktop: desktop_status, command: command_status, push: push_status }
    }
}

// ---------------------------------------------------------------- env transitions

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Up,
    Down,
    Other,
}

pub fn classify_health(status: &str) -> Health {
    match status.to_ascii_lowercase().as_str() {
        "up" | "ok" | "healthy" | "online" | "success" => Health::Up,
        "down" | "error" | "unhealthy" | "failing" | "failed" | "fail" | "unreachable" | "timeout" | "offline" => Health::Down,
        _ => Health::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    WentDown,
    Recovered,
}

/// `previous` is the last classified status seen for this environment.
pub fn health_transition(previous: Option<Health>, now: Health) -> Option<Transition> {
    match (previous, now) {
        (Some(Health::Down), Health::Down) => None,
        (_, Health::Down) => Some(Transition::WentDown),
        (Some(Health::Down), Health::Up) => Some(Transition::Recovered),
        _ => None,
    }
}

/// State the listener keeps between events.
#[derive(Default)]
struct ListenerState {
    env: HashMap<(String, String), Health>,
    pipelines: HashMap<String, String>,
    deploys_started: HashMap<String, i64>,
}

pub fn spawn_listener(state: AppState) {
    let mut rx = state.events.subscribe();
    tokio::spawn(async move {
        let mut ls = ListenerState::default();
        loop {
            match rx.recv().await {
                Ok(ev) => handle_event(&state, &mut ls, &ev),
                Err(broadcast::error::RecvError::Lagged(n)) => tracing::debug!("notify listener skipped {n} events"),
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn project_name(state: &AppState, pid: Option<&str>) -> Option<String> {
    pid.and_then(|p| state.projects.get(p)).map(|p| p.name.clone())
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Rate-limit key, repeat window and priority of an `agent.attention` notification.
///
/// The terminals slice emits one event per transition into an attention state,
/// so a notification is only dropped as a duplicate report of the same thing:
/// - a permission prompt or question (blocking): per prompt text, for a few
///   seconds; the next prompt comes after the user answered, so it always gets
///   through, and it is urgent (skips the ordinary per-minute budget);
/// - an error (blocking): per session, for a few seconds (a failed turn can be
///   reported by both the hook and the transcript, in different words);
/// - a finished turn: per answer text, ordinary priority.
fn attention_limit(tid: &str, agent_state: &str, message: &str) -> (String, Duration, Priority) {
    match agent_state {
        "needs_permission" | "needs_input" => (format!("attention:{tid}:{agent_state}:{message}"), REPEAT_INTERVAL, Priority::Urgent),
        "error" => (format!("attention:{tid}:error"), REPEAT_INTERVAL, Priority::Urgent),
        _ => (format!("attention:{tid}:{agent_state}:{message}"), PER_KEY_INTERVAL, Priority::Normal),
    }
}

/// How long a push service keeps an undelivered notification (a phone that is off).
const PUSH_TTL: u32 = 3600;

/// The push for an `agent.attention`: blocking states are urgent and go out only
/// while the session still waits (with Allow / Deny for a permission request);
/// a finished turn carries the first line of the answer only.
fn attention_push(tid: &str, pid: Option<&str>, agent_state: &str, title: &str, message: &str, level: &'static str) -> PushNote {
    let done = agent_state == "idle";
    let body = if done {
        let first = message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Finished its turn");
        super::truncate_chars(first, 120)
    } else {
        super::truncate_chars(message, 200)
    };
    PushNote {
        topic: if done { Topic::Done } else { Topic::Attention },
        tag: format!("agent:{tid}"),
        title: title.to_string(),
        body,
        level,
        urgency: if done { Urgency::Normal } else { Urgency::High },
        ttl_secs: PUSH_TTL,
        project_id: pid.map(str::to_string),
        open: Some(OpenTarget::terminal(tid)),
        agent: Some(AgentWait { terminal_id: tid.to_string(), state: agent_state.to_string(), permission_hint: None }),
    }
}

fn pipeline_push(pid: Option<&str>, reference: &str, title: &str, body: &str, open: Option<OpenTarget>) -> PushNote {
    PushNote {
        topic: Topic::Pipeline,
        tag: format!("pipeline:{}:{reference}", pid.unwrap_or("")),
        title: title.to_string(),
        body: body.to_string(),
        level: "error",
        urgency: Urgency::Normal,
        ttl_secs: PUSH_TTL,
        project_id: pid.map(str::to_string),
        open,
        agent: None,
    }
}

fn handle_event(state: &AppState, ls: &mut ListenerState, ev: &Arc<Event>) {
    let pid = ev.project_id.as_deref();
    let d = &ev.data;
    match ev.kind.as_str() {
        "agent.attention" => {
            let tid = s(d, "terminalId");
            let session = s(d, "title").unwrap_or("Agent").to_string();
            let agent_state = s(d, "state").unwrap_or("");
            let message = s(d, "message").map(str::to_string).unwrap_or_else(|| {
                match agent_state {
                    "needs_permission" => "is waiting for permission",
                    "needs_input" => "is waiting for your input",
                    "error" => "stopped with an error",
                    "idle" => "finished its turn",
                    _ => "needs your attention",
                }
                .to_string()
            });
            let level = if agent_state == "error" { "error" } else { "warning" };
            let pname = project_name(state, pid);
            let title = match &pname {
                Some(p) => format!("{p} · {session}"),
                None => session.clone(),
            };
            record_event(state, "attention", level, pid, tid, &title, &message);
            let (key, repeat_after, priority) = attention_limit(tid.unwrap_or("-"), agent_state, &message);
            let push = tid.map(|tid| attention_push(tid, pid, agent_state, &title, &message, level));
            state.platform.notifier.send(
                state,
                DesktopNote {
                    key,
                    repeat_after,
                    priority,
                    title: format!("Workbench · {title}"),
                    body: message,
                    level: NoteLevel::parse(level),
                    event: "agent.attention",
                    push,
                },
            );
        }
        "terminal.updated" => {
            // A permission request Workbench can answer (terminals contract): push it
            // with Allow / Deny unless the attention event's push already carried it.
            let Some(tid) = s(d, "id") else { return };
            let (agent_state, Some(perm)) = push::agent_wait_state(push::agent_of_event(d).as_ref()) else { return };
            if agent_state.as_deref() != Some("needs_permission") {
                return;
            }
            let session = s(d, "title").or_else(|| d.get("agent").and_then(|a| s(a, "title"))).unwrap_or("Agent");
            let title = match project_name(state, pid) {
                Some(p) => format!("{p} · {session}"),
                None => session.to_string(),
            };
            let mut note = attention_push(tid, pid, "needs_permission", &title, "Needs your permission", "warning");
            if let Some(w) = note.agent.as_mut() {
                w.permission_hint = Some(perm.id);
            }
            state.platform.push.permission_seen(note);
        }
        "env.health" => {
            let Some(env) = s(d, "env") else { return };
            let Some(status) = s(d, "status") else { return };
            let now = classify_health(status);
            let key = (pid.unwrap_or("").to_string(), env.to_string());
            let prev = ls.env.get(&key).copied();
            if now != Health::Other {
                ls.env.insert(key, now);
            }
            let Some(t) = health_transition(prev, now) else { return };
            let pname = project_name(state, pid).unwrap_or_else(|| "Workbench".into());
            let detail = match d.get("httpStatus").and_then(Value::as_i64) {
                Some(code) => format!(" (HTTP {code})"),
                None => String::new(),
            };
            let (level, title, message) = match t {
                Transition::WentDown => ("error", format!("{pname} · {env} is down"), format!("{env} health check reports {status}{detail}")),
                Transition::Recovered => ("success", format!("{pname} · {env} recovered"), format!("{env} is healthy again{detail}")),
            };
            record_event(state, "env", level, pid, None, &title, &message);
            let push = PushNote {
                topic: Topic::Env,
                tag: format!("env:{}:{env}", pid.unwrap_or("")),
                title: title.clone(),
                body: message.clone(),
                level,
                urgency: if t == Transition::WentDown { Urgency::High } else { Urgency::Low },
                ttl_secs: PUSH_TTL,
                project_id: pid.map(str::to_string),
                open: None,
                agent: None,
            };
            state.platform.notifier.send(
                state,
                DesktopNote {
                    key: format!("env:{}:{env}", pid.unwrap_or("")),
                    repeat_after: PER_KEY_INTERVAL,
                    priority: if t == Transition::WentDown { Priority::Urgent } else { Priority::Normal },
                    title: format!("Workbench · {title}"),
                    body: message,
                    level: NoteLevel::parse(level),
                    event: "env.health",
                    push: Some(push),
                },
            );
        }
        "terminal.created" | "terminal.exited" => {
            let meta = d.get("meta").unwrap_or(&Value::Null);
            if s(meta, "action") != Some("deploy") {
                return;
            }
            let Some(tid) = s(d, "id") else { return };
            let env = s(meta, "env").unwrap_or("an environment");
            let pname = project_name(state, pid).unwrap_or_else(|| "Workbench".into());
            if ev.kind == "terminal.created" {
                if ls.deploys_started.contains_key(tid) {
                    return;
                }
                if ls.deploys_started.len() > 200 {
                    ls.deploys_started.clear();
                }
                ls.deploys_started.insert(tid.to_string(), ev.ts);
                record_event(state, "deploy", "info", pid, Some(tid), format!("{pname} · deploy to {env}"), "Deploy started");
            } else {
                ls.deploys_started.remove(tid);
                let code = d.get("exit").and_then(|e| e.get("code")).and_then(Value::as_i64);
                let (level, message) = match code {
                    Some(0) => ("success", "Deploy finished".to_string()),
                    Some(c) => ("error", format!("Deploy failed (exit code {c})")),
                    None => ("error", "Deploy was interrupted".to_string()),
                };
                let title = format!("{pname} · deploy to {env}");
                record_event(state, "deploy", level, pid, Some(tid), &title, &message);
                let push = PushNote {
                    topic: Topic::Deploy,
                    tag: format!("deploy:{tid}"),
                    title: title.clone(),
                    body: message.clone(),
                    level,
                    urgency: if level == "error" { Urgency::High } else { Urgency::Normal },
                    ttl_secs: PUSH_TTL,
                    project_id: pid.map(str::to_string),
                    open: Some(OpenTarget::terminal(tid)),
                    agent: None,
                };
                state.platform.notifier.send(
                    state,
                    DesktopNote {
                        key: format!("deploy:{tid}"),
                        repeat_after: PER_KEY_INTERVAL,
                        priority: Priority::Always,
                        title: format!("Workbench · {title}"),
                        body: message,
                        level: NoteLevel::parse(level),
                        event: "deploy",
                        push: Some(push),
                    },
                );
            }
        }
        "gitlab.pipeline" => {
            let Some(status) = s(d, "status") else { return };
            let id = d.get("pipelineId").map(|v| v.to_string()).unwrap_or_default();
            let key = format!("{}:{id}", pid.unwrap_or(""));
            if ls.pipelines.get(&key).is_some_and(|prev| prev == status) {
                return;
            }
            if ls.pipelines.len() > 500 {
                ls.pipelines.clear();
            }
            ls.pipelines.insert(key, status.to_string());
            if !matches!(status, "failed" | "success" | "canceled") {
                return;
            }
            let pname = project_name(state, pid).unwrap_or_else(|| "Workbench".into());
            let reference = s(d, "ref").unwrap_or("?");
            let title = format!("{pname} · pipeline #{id} on {reference}");
            let level = match status {
                "failed" => "error",
                "success" => "success",
                _ => "info",
            };
            record_event(state, "pipeline", level, pid, None, &title, format!("Pipeline {status}"));
            if status == "failed" {
                let open = pid.zip(d.get("pipelineId").filter(|v| v.is_u64())).map(|(p, id)| OpenTarget {
                    kind: "pipeline".into(),
                    id: format!("pipeline:{p}:{id}"),
                    params: json!({ "projectId": p, "pipelineId": id }),
                });
                let push = pipeline_push(pid, reference, &title, "Pipeline failed", open);
                state.platform.notifier.send(
                    state,
                    DesktopNote {
                        key: format!("pipeline:{}:{reference}", pid.unwrap_or("")),
                        repeat_after: PER_KEY_INTERVAL,
                        priority: Priority::Normal,
                        title: format!("Workbench · {title}"),
                        body: "Pipeline failed".into(),
                        level: NoteLevel::Error,
                        event: "gitlab.pipeline",
                        push: Some(push),
                    },
                );
            }
        }
        "github.run" => {
            // A workflow dispatch has no run yet (`runId: null`, no state).
            let Some(run_state) = s(d, "state") else { return };
            let Some(run_id) = d.get("runId").filter(|v| v.is_u64()).map(Value::to_string) else { return };
            let key = format!("{}:gh:{run_id}", pid.unwrap_or(""));
            if ls.pipelines.get(&key).is_some_and(|prev| prev == run_state) {
                return;
            }
            if ls.pipelines.len() > 500 {
                ls.pipelines.clear();
            }
            ls.pipelines.insert(key, run_state.to_string());
            if !matches!(run_state, "failed" | "success" | "canceled") {
                return;
            }
            let pname = project_name(state, pid).unwrap_or_else(|| "Workbench".into());
            let workflow = s(d, "name").unwrap_or("workflow");
            let number = d.get("runNumber").and_then(Value::as_u64).map(|n| format!(" #{n}")).unwrap_or_default();
            let branch = s(d, "branch").unwrap_or("?");
            let title = format!("{pname} · {workflow}{number} on {branch}");
            let level = match run_state {
                "failed" => "error",
                "success" => "success",
                _ => "info",
            };
            record_event(state, "pipeline", level, pid, None, &title, format!("Workflow run {run_state}"));
            if run_state == "failed" {
                let open = pid.map(|p| OpenTarget {
                    kind: "gh.run".into(),
                    id: format!("gh.run:{p}:{run_id}"),
                    params: json!({ "projectId": p, "runId": d["runId"] }),
                });
                let push = pipeline_push(pid, branch, &title, "Workflow run failed", open);
                state.platform.notifier.send(
                    state,
                    DesktopNote {
                        key: format!("pipeline:{}:{branch}", pid.unwrap_or("")),
                        repeat_after: PER_KEY_INTERVAL,
                        priority: Priority::Normal,
                        title: format!("Workbench · {title}"),
                        body: "Workflow run failed".into(),
                        level: NoteLevel::Error,
                        event: "github.run",
                        push: Some(push),
                    },
                );
            }
        }
        _ => {}
    }
}

/// `POST /api/platform/notify-test` — send a test notification through the configured channels.
pub async fn test_route(State(state): State<AppState>) -> Json<Value> {
    let outcome = state.platform.notifier.send(
        &state,
        DesktopNote {
            key: "test".into(),
            repeat_after: Duration::ZERO,
            priority: Priority::Always,
            title: "Workbench".into(),
            body: "Test notification: desktop notifications work.".into(),
            level: NoteLevel::Info,
            event: "test",
            // Devices test their push with `POST /api/push/test`.
            push: None,
        },
    );
    Json(json!(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_enforces_per_key_and_global_limits() {
        let mut rl = RateLimiter::default();
        let t0 = Instant::now();
        let allow = |rl: &mut RateLimiter, key: &str, at: Instant| rl.allow(key, PER_KEY_INTERVAL, Priority::Normal, at);
        assert!(allow(&mut rl, "a", t0));
        assert!(!allow(&mut rl, "a", t0 + Duration::from_secs(5)));
        assert!(allow(&mut rl, "a", t0 + PER_KEY_INTERVAL));
        // Fill the global window with distinct keys.
        let mut allowed = 2;
        for i in 0..20 {
            if allow(&mut rl, &format!("k{i}"), t0 + PER_KEY_INTERVAL) {
                allowed += 1;
            }
        }
        assert_eq!(allowed, GLOBAL_PER_MINUTE);
        // Urgent notes still get through, up to the runaway ceiling.
        let mut urgent = 0;
        for i in 0..100 {
            if rl.allow(&format!("u{i}"), REPEAT_INTERVAL, Priority::Urgent, t0 + PER_KEY_INTERVAL) {
                urgent += 1;
            }
        }
        assert_eq!(urgent, URGENT_PER_MINUTE - GLOBAL_PER_MINUTE);
        assert!(rl.allow("deploy", PER_KEY_INTERVAL, Priority::Always, t0 + PER_KEY_INTERVAL));
        // A minute later the window has room again.
        assert!(allow(&mut rl, "late", t0 + Duration::from_secs(90)));
    }

    #[test]
    fn attention_keys_separate_prompts_and_collapse_duplicates() {
        let mut rl = RateLimiter::default();
        let t0 = Instant::now();
        let mut allow = |state: &str, message: &str, secs: u64| {
            let (key, after, prio) = attention_limit("t1", state, message);
            rl.allow(&key, after, prio, t0 + Duration::from_secs(secs))
        };
        assert!(allow("idle", "Done.", 0));
        assert!(allow("needs_permission", "Permission to run `npm test`", 1));
        // The same prompt reported twice.
        assert!(!allow("needs_permission", "Permission to run `npm test`", 2));
        // The next prompt, after the user approved the first.
        assert!(allow("needs_permission", "Permission to run `cargo publish`", 8));
        // The same command asked again later is a new prompt.
        assert!(allow("needs_permission", "Permission to run `npm test`", 30));
        assert!(allow("error", "API error: overloaded", 40));
        assert!(!allow("error", "The turn failed", 41));
        assert!(allow("error", "The turn failed", 60));
        // Another turn with a different answer.
        assert!(allow("idle", "All tests pass.", 61));
    }

    #[test]
    fn health_transitions() {
        use Health::*;
        assert_eq!(health_transition(None, Down), Some(Transition::WentDown));
        assert_eq!(health_transition(Some(Up), Down), Some(Transition::WentDown));
        assert_eq!(health_transition(Some(Down), Down), None);
        assert_eq!(health_transition(Some(Down), Up), Some(Transition::Recovered));
        assert_eq!(health_transition(None, Up), None);
        assert_eq!(health_transition(Some(Up), Up), None);
        assert_eq!(health_transition(Some(Down), Other), None);
        assert_eq!(classify_health("DOWN"), Down);
        assert_eq!(classify_health("up"), Up);
        assert_eq!(classify_health("checking"), Other);
    }

    #[test]
    fn text_is_cleaned_and_escaped() {
        assert_eq!(clean("a\u{1b}[31mb\u{7}", 50), "a [31mb");
        assert_eq!(escape_markup("<b>x & y</b>"), "&lt;b&gt;x &amp; y&lt;/b&gt;");
    }

    #[tokio::test]
    async fn listener_records_env_transitions_once() {
        let app = crate::platform::testutil::app().await;
        let state = &app.state;
        let mut ls = ListenerState::default();
        let ev = |status: &str| {
            Arc::new(Event {
                kind: "env.health".into(),
                project_id: Some("p".into()),
                data: json!({ "env": "production", "status": status, "httpStatus": 502 }),
                ts: 0,
            })
        };
        handle_event(state, &mut ls, &ev("up"));
        handle_event(state, &mut ls, &ev("down"));
        handle_event(state, &mut ls, &ev("down"));
        handle_event(state, &mut ls, &ev("up"));
        let events = state.platform.activity.events(10);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[1].level, "error");
        assert!(events[1].title.contains("production is down"));
        assert!(events[1].message.contains("HTTP 502"));
        assert_eq!(events[0].level, "success");
    }

    fn attention(tid: &str, state: &str, message: &str) -> Arc<Event> {
        Arc::new(Event {
            kind: "agent.attention".into(),
            project_id: None,
            data: json!({ "terminalId": tid, "state": state, "message": message, "title": "Pong" }),
            ts: 0,
        })
    }

    /// The review's sequence: a permission prompt, approved, then another one
    /// seconds later. Both block the agent, so both must reach the user.
    #[tokio::test]
    async fn every_new_blocking_prompt_is_delivered() {
        let app = crate::platform::testutil::app().await;
        let state = &app.state;
        let mut ls = ListenerState::default();
        handle_event(state, &mut ls, &attention("t1", "idle", "Done."));
        handle_event(state, &mut ls, &attention("t1", "needs_permission", "Permission to run `npm test`"));
        handle_event(state, &mut ls, &attention("t1", "needs_permission", "Permission to run `git push origin main`"));
        handle_event(state, &mut ls, &attention("t1", "needs_input", "Claude needs your input"));
        handle_event(state, &mut ls, &attention("t1", "error", "API error"));
        let sent = state.platform.notifier.sent_keys();
        assert_eq!(sent.len(), 5, "{sent:?}");
    }

    #[tokio::test]
    async fn listener_records_deploys() {
        let app = crate::platform::testutil::app().await;
        let state = &app.state;
        let mut ls = ListenerState::default();
        let meta = json!({ "env": "staging", "action": "deploy" });
        handle_event(
            state,
            &mut ls,
            &Arc::new(Event { kind: "terminal.created".into(), project_id: None, data: json!({ "id": "t1", "meta": meta }), ts: 1 }),
        );
        handle_event(
            state,
            &mut ls,
            &Arc::new(Event {
                kind: "terminal.exited".into(),
                project_id: None,
                data: json!({ "id": "t1", "meta": meta, "exit": { "code": 2 } }),
                ts: 2,
            }),
        );
        let events = state.platform.activity.events(10);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message, "Deploy failed (exit code 2)");
        assert_eq!(events[1].message, "Deploy started");
    }

    /// GitHub Actions runs reach the Activity log and, when they fail, the desktop,
    /// like GitLab pipelines.
    #[tokio::test]
    async fn listener_reports_github_runs() {
        let app = crate::platform::testutil::app().await;
        let state = &app.state;
        let mut ls = ListenerState::default();
        let run = |id: Value, run_state: Option<&str>| {
            let mut data = json!({ "runId": id, "name": "CI", "runNumber": 41, "branch": "main", "action": "poll" });
            if let Some(s) = run_state {
                data["state"] = json!(s);
            }
            Arc::new(Event { kind: "github.run".into(), project_id: Some("p".into()), data, ts: 0 })
        };
        handle_event(state, &mut ls, &run(Value::Null, None)); // a dispatch: no run yet
        handle_event(state, &mut ls, &run(json!(7), Some("running")));
        handle_event(state, &mut ls, &run(json!(7), Some("failed")));
        handle_event(state, &mut ls, &run(json!(7), Some("failed"))); // the same news again
        handle_event(state, &mut ls, &run(json!(8), Some("success")));
        let events = state.platform.activity.events(10);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!((events[1].kind.as_str(), events[1].level.as_str()), ("pipeline", "error"));
        assert!(events[1].title.ends_with("CI #41 on main"), "{}", events[1].title);
        assert_eq!(events[1].message, "Workflow run failed");
        assert_eq!(events[0].level, "success");
        assert_eq!(state.platform.notifier.sent_keys(), vec!["pipeline:p:main".to_string()]);
    }
}

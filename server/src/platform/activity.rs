//! The activity log: recent MCP tool calls made by agents and notable events
//! (agent attention, environment up/down transitions, deploys, pipelines).
//! Both are bounded in-memory ring buffers; the UI loads them once and then
//! follows the `mcp.call` and `platform.activity` events.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::extract::{Query, State};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::AppState;

/// Entries kept per buffer.
pub const CAPACITY: usize = 500;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallRecord {
    pub id: u64,
    pub at: i64,
    pub terminal_id: Option<String>,
    pub project_id: Option<String>,
    /// Title of the calling session, when known.
    pub session: Option<String>,
    pub tool: String,
    pub ok: bool,
    pub mutating: bool,
    pub ms: u64,
    /// Short, value-truncated rendering of the arguments (never secrets or bodies).
    pub summary: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecord {
    pub id: u64,
    pub at: i64,
    /// attention | env | deploy | pipeline | notify
    pub kind: String,
    /// info | success | warning | error
    pub level: String,
    pub project_id: Option<String>,
    pub terminal_id: Option<String>,
    pub title: String,
    pub message: String,
}

#[derive(Default)]
pub struct ActivityLog {
    seq: AtomicU64,
    calls: Mutex<VecDeque<McpCallRecord>>,
    events: Mutex<VecDeque<ActivityRecord>>,
}

impl ActivityLog {
    pub fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn push_call(&self, rec: McpCallRecord) {
        push_bounded(&mut self.calls.lock(), rec);
    }

    pub fn push_event(&self, rec: ActivityRecord) {
        push_bounded(&mut self.events.lock(), rec);
    }

    /// Newest first.
    pub fn calls(&self, limit: usize) -> Vec<McpCallRecord> {
        self.calls.lock().iter().rev().take(limit).cloned().collect()
    }

    /// Newest first.
    pub fn events(&self, limit: usize) -> Vec<ActivityRecord> {
        self.events.lock().iter().rev().take(limit).cloned().collect()
    }
}

fn push_bounded<T>(q: &mut VecDeque<T>, item: T) {
    if q.len() >= CAPACITY {
        q.pop_front();
    }
    q.push_back(item);
}

/// Record a notable event and tell connected UIs.
pub fn record_event(
    state: &AppState,
    kind: &str,
    level: &str,
    project_id: Option<&str>,
    terminal_id: Option<&str>,
    title: impl Into<String>,
    message: impl Into<String>,
) -> ActivityRecord {
    let rec = ActivityRecord {
        id: state.platform.activity.next_id(),
        at: crate::util::now_ms(),
        kind: kind.to_string(),
        level: level.to_string(),
        project_id: project_id.map(str::to_string),
        terminal_id: terminal_id.map(str::to_string),
        title: super::truncate_chars(&title.into(), 200),
        message: super::truncate_chars(&message.into(), 1000),
    };
    state.platform.activity.push_event(rec.clone());
    state.events.emit("platform.activity", project_id, &rec);
    rec
}

/// A one-line rendering of tool arguments for the activity view. Long strings
/// are cut, bulky values are summarized, and anything that looks like a
/// credential is masked.
pub fn summarize_args(args: &Map<String, Value>) -> String {
    const SECRETISH: &[&str] = &["token", "password", "secret", "authorization", "apikey", "api_key", "cookie"];
    let mut parts = vec![];
    for (k, v) in args {
        let lk = k.to_ascii_lowercase();
        let rendered = if SECRETISH.iter().any(|s| lk.contains(s)) {
            "••••".to_string()
        } else {
            match v {
                Value::String(s) if s.chars().count() > 60 || s.contains('\n') => {
                    let first = s.lines().next().unwrap_or("");
                    format!("\"{}\" ({} chars)", super::truncate_chars(first, 40), s.chars().count())
                }
                Value::String(s) => format!("\"{s}\""),
                Value::Array(a) => format!("[{} items]", a.len()),
                Value::Object(o) => format!("{{{} keys}}", o.len()),
                other => other.to_string(),
            }
        };
        parts.push(format!("{k}={rendered}"));
    }
    super::truncate_chars(&parts.join(", "), 240)
}

#[derive(Deserialize)]
pub struct ListQuery {
    limit: Option<usize>,
}

/// `GET /api/platform/activity?limit=` → `{calls, events}` (newest first).
pub async fn list(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Json<Value> {
    let limit = q.limit.unwrap_or(CAPACITY).min(CAPACITY);
    Json(json!({
        "calls": state.platform.activity.calls(limit),
        "events": state.platform.activity.events(limit),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_keeps_the_newest() {
        let log = ActivityLog::default();
        for i in 0..(CAPACITY + 10) {
            log.push_event(ActivityRecord {
                id: i as u64,
                at: 0,
                kind: "env".into(),
                level: "info".into(),
                project_id: None,
                terminal_id: None,
                title: String::new(),
                message: String::new(),
            });
        }
        let ev = log.events(CAPACITY + 100);
        assert_eq!(ev.len(), CAPACITY);
        assert_eq!(ev[0].id, (CAPACITY + 9) as u64);
        assert_eq!(ev.last().unwrap().id, 10);
    }

    #[test]
    fn summaries_mask_secrets_and_cut_bodies() {
        let args = json!({
            "pageId": 123,
            "token": "glpat-abcdefgh",
            "body": "line one\nline two",
            "labels": ["a", "b"],
            "path": "src/main.rs",
        });
        let s = summarize_args(args.as_object().unwrap());
        assert!(s.contains("pageId=123"), "{s}");
        assert!(s.contains("token=••••"), "{s}");
        assert!(!s.contains("glpat"), "{s}");
        assert!(s.contains("body=\"line one\" (17 chars)"), "{s}");
        assert!(s.contains("labels=[2 items]"), "{s}");
        assert!(s.contains("path=\"src/main.rs\""), "{s}");
    }
}

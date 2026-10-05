//! Claude Code's files on disk, read-only: transcripts (`~/.claude/projects/<slug>/<id>.jsonl`)
//! and the live session registry (`~/.claude/sessions/<pid>.json`).
//!
//! Transcript records are undocumented and version-dependent, so parsing is lenient:
//! unknown or malformed lines are ignored, never guessed. Hooks are the primary source of
//! agent state; the transcript adds titles, Remote Control links, API errors, the last
//! answer, and the interrupts no hook reports.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

/// `~/.claude`, or `$CLAUDE_CONFIG_DIR` (the agent environment's value wins).
pub fn claude_dir(env_override: Option<&str>) -> PathBuf {
    if let Some(d) = env_override.filter(|d| !d.is_empty()) {
        return crate::config::expand_tilde(d);
    }
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::home_dir().unwrap_or_default().join(".claude"),
    }
}

/// Claude's project directory name for a working directory: every character that is
/// not an ASCII letter or digit becomes `-`. Windows too (`C:\Users\me\proj` →
/// `C--Users-me-proj`), so `cwd` must be spelled as the session sees it: no `\\?\`
/// prefix (`os::path::canonicalize`); the drive letter's case does not matter there,
/// since the directory lookup ignores case.
pub fn slug(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Where Claude keeps transcripts for `cwd`. Very long slugs are truncated (with a
/// suffix) by Claude, so fall back to a prefix match on existing directories.
pub fn project_dir(claude_dir: &Path, cwd: &Path) -> PathBuf {
    let root = claude_dir.join("projects");
    let s = slug(cwd);
    let exact = root.join(&s);
    if exact.is_dir() || s.len() <= 200 {
        return exact;
    }
    let prefix: String = s.chars().take(200).collect();
    std::fs::read_dir(&root)
        .ok()
        .and_then(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
        })
        .unwrap_or(exact)
}

/// Where Claude keeps the auto-memory of `cwd` under the config folder `claude_dir`
/// (`None`: the slug is so long that Claude adds a hash to it, which we do not reproduce).
pub fn memory_dir(claude_dir: &Path, cwd: &Path) -> Option<PathBuf> {
    let s = slug(cwd);
    (s.len() <= 200).then(|| claude_dir.join("projects").join(s).join("memory"))
}

pub fn transcript_path(claude_dir: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    project_dir(claude_dir, cwd).join(format!("{session_id}.jsonl"))
}

/// Find a session's transcript in any project directory (sessions resumed from another cwd).
pub fn find_transcript(claude_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(claude_dir.join("projects"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(&name))
        .find(|p| p.is_file())
}

pub fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok() && s.len() == 36
}

// ---------------------------------------------------------------- records

/// What one transcript record tells us.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    Title(String),
    RemoteUrl(String),
    /// An API error; `final_attempt` is false while Claude is still retrying.
    ApiError { message: String, final_attempt: bool },
    TurnStarted,
    /// An assistant message ending the turn with text (`id` dedupes split records).
    TurnCompleted { id: String, text: String },
    /// `system/turn_duration`: the turn is over, whatever the stop reason.
    TurnEnded,
    /// The user interrupted the turn (Esc, or declined a permission prompt). `at` is the
    /// record's timestamp, so a stale interrupt cannot undo a newer hook state.
    Interrupted { at: Option<i64> },
    PermissionMode(String),
    Model(String),
    /// `cost-state`: the session's running cost (a fallback when our status line is
    /// not installed because the user has their own).
    Cost(f64),
    /// A tool call's result: its permission, if it asked, was answered.
    ToolResult { tool_use_id: String },
}

fn text_blocks(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Parse one JSONL line into zero or more records.
pub fn parse_line(line: &[u8]) -> Vec<Record> {
    let Ok(v) = serde_json::from_slice::<Value>(line) else { return vec![] };
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return vec![];
    }
    match s("type") {
        Some("ai-title") => s("aiTitle").map(|t| vec![Record::Title(clean_line(t, 120))]).unwrap_or_default(),
        Some("permission-mode") => s("permissionMode").map(|m| vec![Record::PermissionMode(m.to_string())]).unwrap_or_default(),
        Some("cost-state") => v
            .get("totalCostUSD")
            .and_then(Value::as_f64)
            .filter(|c| c.is_finite() && *c >= 0.0)
            .map(|c| vec![Record::Cost(c)])
            .unwrap_or_default(),
        Some("system") => match s("subtype") {
            Some("bridge_status") => s("url")
                .filter(|u| u.starts_with("https://claude.ai/"))
                .map(|u| vec![Record::RemoteUrl(u.to_string())])
                .unwrap_or_default(),
            Some("api_error") => {
                let message = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| v.get("content").and_then(Value::as_str))
                    .unwrap_or("API error");
                let attempt = v.get("retryAttempt").and_then(Value::as_u64);
                let max = v.get("maxRetries").and_then(Value::as_u64);
                let final_attempt = match (attempt, max) {
                    (Some(a), Some(m)) => a >= m,
                    _ => true,
                };
                vec![Record::ApiError { message: clean_line(message, 300), final_attempt }]
            }
            Some("turn_duration") => vec![Record::TurnEnded],
            _ => vec![],
        },
        Some("user") => {
            if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return vec![];
            }
            let content = v.pointer("/message/content").cloned().unwrap_or(Value::Null);
            let text = text_blocks(&content);
            let mut out = if text.starts_with("[Request interrupted by user") {
                let at = s("timestamp")
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|d| d.timestamp_millis());
                vec![Record::Interrupted { at }]
            } else {
                vec![Record::TurnStarted]
            };
            for b in content.as_array().into_iter().flatten() {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if let Some(id) = b.get("tool_use_id").and_then(Value::as_str).filter(|id| !id.is_empty() && id.len() <= 200) {
                        out.push(Record::ToolResult { tool_use_id: id.to_string() });
                    }
                }
            }
            out
        }
        Some("assistant") => {
            let msg = v.get("message").cloned().unwrap_or(Value::Null);
            let mut out = vec![];
            if let Some(m) = msg.get("model").and_then(Value::as_str).filter(|m| !m.starts_with('<')) {
                out.push(Record::Model(m.to_string()));
            }
            let stop = msg.get("stop_reason").and_then(Value::as_str);
            let text = text_blocks(msg.get("content").unwrap_or(&Value::Null));
            if stop == Some("end_turn") && !text.trim().is_empty() {
                let id = msg.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                out.push(Record::TurnCompleted { id, text });
            } else if stop != Some("end_turn") {
                out.push(Record::TurnStarted);
            }
            out
        }
        _ => vec![],
    }
}

/// One line, control characters removed, at most `max` characters.
pub fn clean_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&flat, max)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The last `max` characters of `s` (a message tail), starting at a line or word boundary.
pub fn tail_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - max).collect();
    let cut = tail.find('\n').or_else(|| tail.find(' ')).filter(|&i| i < tail.len() / 3).map(|i| i + 1).unwrap_or(0);
    format!("…{}", &tail[cut..])
}

/// Splits a growing byte stream into complete lines. Lines split across reads are
/// buffered; since `\n` never occurs inside a UTF-8 sequence, splitting on bytes is safe.
#[derive(Default)]
pub struct LineBuffer {
    partial: Vec<u8>,
}

impl LineBuffer {
    /// Longest line kept; longer ones (huge tool results) are dropped whole.
    const MAX_LINE: usize = 8 * 1024 * 1024;

    pub fn push(&mut self, data: &[u8], mut on_line: impl FnMut(&[u8])) {
        let mut rest = data;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            if self.partial.is_empty() {
                on_line(&rest[..i]);
            } else {
                self.partial.extend_from_slice(&rest[..i]);
                if self.partial.len() <= Self::MAX_LINE {
                    on_line(&self.partial);
                }
                self.partial.clear();
            }
            rest = &rest[i + 1..];
        }
        if self.partial.len() + rest.len() <= Self::MAX_LINE {
            self.partial.extend_from_slice(rest);
        } else {
            // Keep only the length so the eventual line is dropped, not misparsed.
            self.partial.clear();
            self.partial.resize(Self::MAX_LINE + 1, b' ');
        }
    }

    pub fn clear(&mut self) {
        self.partial.clear();
    }
}

// ---------------------------------------------------------------- history

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// The CLI's session id (what `resume` takes).
    pub id: String,
    /// The provider (`[agents.providers.<id>]`) the session belongs to.
    pub provider: String,
    pub title: String,
    /// The first real prompt (Kimi: the last one), when it differs from the title.
    pub first_prompt: Option<String>,
    /// The tail of the last answer, when the history records it (Codex).
    pub last_message: Option<String>,
    pub last_activity: i64,
    pub size_bytes: u64,
    pub git_branch: Option<String>,
    /// Workbench terminal hosting this session now, if any.
    pub terminal_id: Option<String>,
    pub open: bool,
}

/// Cached per transcript file, keyed by path, valid while `(mtime, size)` match.
#[derive(Default)]
pub struct HistoryCache {
    files: HashMap<PathBuf, (SystemTime, u64, Option<Summary>)>,
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub ai_title: Option<String>,
    pub first_prompt: Option<String>,
    pub git_branch: Option<String>,
}

const HEAD_BYTES: u64 = 96 * 1024;
const TAIL_BYTES: u64 = 96 * 1024;

/// Summarize a transcript from its head and tail only. `None` for transcripts with no
/// conversation (a session closed before its first prompt).
pub fn summarize(path: &Path, size: u64) -> Option<Summary> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; size.min(HEAD_BYTES) as usize];
    f.read_exact(&mut head).ok()?;
    let mut tail = vec![];
    if size > HEAD_BYTES {
        let start = size.saturating_sub(TAIL_BYTES).max(HEAD_BYTES);
        f.seek(SeekFrom::Start(start)).ok()?;
        f.take(TAIL_BYTES).read_to_end(&mut tail).ok()?;
        // The first tail line is probably partial.
        match tail.iter().position(|&b| b == b'\n') {
            Some(i) => {
                tail.drain(..=i);
            }
            None => tail.clear(),
        }
    }
    let mut sum = Summary::default();
    let mut has_conversation = false;
    let mut visit = |line: &[u8], sum: &mut Summary| {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        let t = v.get("type").and_then(Value::as_str);
        if t == Some("ai-title") {
            if let Some(a) = v.get("aiTitle").and_then(Value::as_str) {
                sum.ai_title = Some(clean_line(a, 120));
            }
        }
        if matches!(t, Some("user") | Some("assistant")) {
            has_conversation = true;
            if let Some(b) = v.get("gitBranch").and_then(Value::as_str).filter(|b| !b.is_empty() && *b != "HEAD") {
                sum.git_branch = Some(b.to_string());
            }
        }
        if t == Some("user")
            && sum.first_prompt.is_none()
            && v.get("isMeta").and_then(Value::as_bool) != Some(true)
            && v.get("isSidechain").and_then(Value::as_bool) != Some(true)
        {
            let text = text_blocks(v.pointer("/message/content").unwrap_or(&Value::Null));
            let text = text.trim();
            let is_noise = text.is_empty()
                || text.starts_with('<')
                || text.starts_with("Caveat:")
                || text.starts_with("[Request interrupted");
            if !is_noise {
                sum.first_prompt = Some(clean_line(text, 140));
            }
        }
    };
    for line in head.split(|&b| b == b'\n') {
        visit(line, &mut sum);
    }
    for line in tail.split(|&b| b == b'\n') {
        visit(line, &mut sum);
    }
    // A file larger than the head always had a conversation.
    (has_conversation || size > HEAD_BYTES).then_some(sum)
}

impl HistoryCache {
    /// Transcripts in `dir`, newest first, at most `limit`.
    pub fn list(&mut self, dir: &Path, limit: usize) -> Vec<(String, i64, u64, Summary)> {
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut files: Vec<(PathBuf, SystemTime, u64)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                    return None;
                }
                let stem = p.file_stem()?.to_str()?;
                if !is_uuid(stem) {
                    return None;
                }
                let md = e.metadata().ok()?;
                md.is_file().then(|| (p.clone(), md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len()))
            })
            .collect();
        files.sort_by(|a, b| b.1.cmp(&a.1));
        let mut out = vec![];
        let mut seen = std::collections::HashSet::new();
        for (path, mtime, size) in files {
            if out.len() >= limit {
                break;
            }
            seen.insert(path.clone());
            let cached = self.files.get(&path).filter(|c| c.0 == mtime && c.1 == size).map(|c| c.2.clone());
            let summary = match cached {
                Some(s) => s,
                None => {
                    let s = summarize(&path, size);
                    self.files.insert(path.clone(), (mtime, size, s.clone()));
                    s
                }
            };
            let Some(summary) = summary else { continue };
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ms = mtime.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
            out.push((id, ms, size, summary));
        }
        // Bound the cache to what the latest listings touched (plus other directories).
        if self.files.len() > 4000 {
            self.files.retain(|p, _| seen.contains(p));
        }
        out
    }
}

// ---------------------------------------------------------------- live sessions

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSession {
    pub pid: i32,
    pub session_id: String,
    pub name: Option<String>,
    pub cwd: String,
    pub status: Option<String>,
    pub started_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub remote_url: Option<String>,
    pub project_id: Option<String>,
    pub entrypoint: Option<String>,
}

/// `https://claude.ai/code/session_<x>` for a bridge session id `cse_<x>`.
pub fn remote_url_for_bridge(bridge_id: &str) -> Option<String> {
    let tail = bridge_id.split_once('_').map(|(_, t)| t).unwrap_or(bridge_id);
    (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| format!("https://claude.ai/code/session_{tail}"))
}

/// Parse `~/.claude/sessions/<pid>.json` (the `<pid>.<hash>.key` files next to it are
/// secrets and are never opened: only names that are all digits plus `.json` are read).
pub fn read_live_sessions(claude_dir: &Path) -> Vec<ExternalSession> {
    let Ok(rd) = std::fs::read_dir(claude_dir.join("sessions")) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".json") else { continue };
        if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(bytes) = std::fs::read(e.path()) else { continue };
        if let Some(s) = parse_live_session(&bytes) {
            out.push(s);
        }
    }
    out
}

pub fn parse_live_session(bytes: &[u8]) -> Option<ExternalSession> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(ExternalSession {
        pid: v.get("pid")?.as_i64()? as i32,
        session_id: s("sessionId")?,
        name: s("name"),
        cwd: s("cwd").unwrap_or_default(),
        status: s("status"),
        started_at: v.get("startedAt").and_then(Value::as_i64),
        updated_at: v.get("updatedAt").and_then(Value::as_i64),
        remote_url: v.get("bridgeSessionId").and_then(Value::as_str).and_then(remote_url_for_bridge),
        project_id: None,
        entrypoint: s("entrypoint"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_every_non_alphanumeric() {
        assert_eq!(slug(Path::new("/home/u/workspace/shop")), "-home-u-workspace-shop");
        assert_eq!(slug(Path::new("/tmp/a_b.c d")), "-tmp-a-b-c-d");
        // Claude Code on Windows: the same rule over `C:\…` (checked against 2.1.284).
        assert_eq!(slug(Path::new(r"C:\Users\me\my proj")), "C--Users-me-my-proj");
    }

    #[test]
    fn parses_record_kinds() {
        assert_eq!(parse_line(br#"{"type":"ai-title","aiTitle":"Fix the\nbuild","sessionId":"x"}"#), vec![Record::Title("Fix the build".into())]);
        assert_eq!(
            parse_line(br#"{"type":"system","subtype":"bridge_status","url":"https://claude.ai/code/session_abc"}"#),
            vec![Record::RemoteUrl("https://claude.ai/code/session_abc".into())]
        );
        assert!(parse_line(br#"{"type":"system","subtype":"bridge_status","url":"http://evil/x"}"#).is_empty());
        assert_eq!(
            parse_line(br#"{"type":"system","subtype":"api_error","error":{"message":"overloaded"},"retryAttempt":2,"maxRetries":10}"#),
            vec![Record::ApiError { message: "overloaded".into(), final_attempt: false }]
        );
        assert_eq!(parse_line(br#"{"type":"system","subtype":"turn_duration","durationMs":5}"#), vec![Record::TurnEnded]);
        assert_eq!(parse_line(br#"{"type":"user","message":{"role":"user","content":"hello"}}"#), vec![Record::TurnStarted]);
        assert_eq!(
            parse_line(br#"{"type":"user","timestamp":"2026-09-26T10:00:00.000Z","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#),
            vec![Record::Interrupted { at: Some(1_790_416_800_000) }]
        );
        assert!(parse_line(br#"{"type":"user","isMeta":true,"message":{"content":"x"}}"#).is_empty());
        assert!(parse_line(br#"{"type":"assistant","isSidechain":true,"message":{"stop_reason":"end_turn"}}"#).is_empty());
        assert_eq!(
            parse_line(br#"{"type":"assistant","message":{"id":"m1","model":"claude-haiku","stop_reason":"end_turn","content":[{"type":"thinking","thinking":"..."},{"type":"text","text":"Done."}]}}"#),
            vec![Record::Model("claude-haiku".into()), Record::TurnCompleted { id: "m1".into(), text: "Done.".into() }]
        );
        assert_eq!(
            parse_line(br#"{"type":"assistant","message":{"model":"<synthetic>","stop_reason":"tool_use","content":[]}}"#),
            vec![Record::TurnStarted]
        );
        assert_eq!(parse_line(br#"{"type":"permission-mode","permissionMode":"plan"}"#), vec![Record::PermissionMode("plan".into())]);
        assert_eq!(parse_line(br#"{"type":"cost-state","totalCostUSD":0.25}"#), vec![Record::Cost(0.25)]);
        assert!(parse_line(b"not json").is_empty());
    }

    #[test]
    fn line_buffer_handles_splits_and_multibyte() {
        let mut lb = LineBuffer::default();
        let mut lines: Vec<String> = vec![];
        let data = "{\"a\":\"é漢\"}\n{\"b\":1}\n{\"c\"".as_bytes();
        // Split inside the multibyte character.
        lb.push(&data[..8], |l| lines.push(String::from_utf8(l.to_vec()).unwrap()));
        lb.push(&data[8..], |l| lines.push(String::from_utf8(l.to_vec()).unwrap()));
        assert_eq!(lines, vec!["{\"a\":\"é漢\"}", "{\"b\":1}"]);
        lb.push(b":2}\n", |l| lines.push(String::from_utf8(l.to_vec()).unwrap()));
        assert_eq!(lines.last().unwrap(), "{\"c\":2}");
    }

    #[test]
    fn tails_and_truncation() {
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(truncate_chars("abc", 4), "abc");
        let t = tail_chars(&"word ".repeat(100), 20);
        assert!(t.starts_with('…') && t.chars().count() <= 21);
        assert_eq!(clean_line("a\n\tb   c", 10), "a b c");
    }

    #[test]
    fn summarizes_head_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let mut text = String::new();
        text.push_str(r#"{"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>"}}"#);
        text.push('\n');
        text.push_str(r#"{"type":"user","gitBranch":"main","message":{"content":"Refactor the parser please"}}"#);
        text.push('\n');
        for _ in 0..3000 {
            text.push_str(r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[]}}"#);
            text.push('\n');
        }
        text.push_str(r#"{"type":"ai-title","aiTitle":"Parser refactor"}"#);
        text.push('\n');
        std::fs::write(&p, &text).unwrap();
        let s = summarize(&p, text.len() as u64).unwrap();
        assert_eq!(s.ai_title.as_deref(), Some("Parser refactor"));
        assert_eq!(s.first_prompt.as_deref(), Some("Refactor the parser please"));
        assert_eq!(s.git_branch.as_deref(), Some("main"));
        let empty = dir.path().join("e.jsonl");
        std::fs::write(&empty, "{\"type\":\"mode\",\"mode\":\"x\"}\n").unwrap();
        assert!(summarize(&empty, 27).is_none());
    }

    #[test]
    fn live_session_registry_and_bridge_links() {
        let s = parse_live_session(
            br#"{"pid":42,"sessionId":"s1","cwd":"/w/x","name":"x-1","status":"busy","bridgeSessionId":"cse_ABC123","startedAt":1}"#,
        )
        .unwrap();
        assert_eq!(s.pid, 42);
        assert_eq!(s.remote_url.as_deref(), Some("https://claude.ai/code/session_ABC123"));
        assert!(parse_live_session(b"{}").is_none());
        assert_eq!(remote_url_for_bridge("cse_a/b"), None);
    }
}

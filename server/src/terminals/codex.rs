//! OpenAI Codex CLI files on disk, read-only: rollouts under
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<local time>-<thread id>.jsonl`.
//!
//! Verified against codex-cli 0.157.1 and its source (`codex-rs/rollout`, `protocol`):
//! * The first line is `{"timestamp", "type":"session_meta", "payload":{"id", "cwd",
//!   "originator", "source", "forked_from_id"?, "parent_thread_id"?, "timestamp", …,
//!   "git":{"branch"}?}}`. The date folders are in **local** time.
//! * A new session's file is created lazily (at its first turn); a resumed session
//!   appends to its existing file; a fork writes a new file with `forked_from_id`.
//! * `event_msg` payloads persisted in every history mode: `task_started`
//!   (alias `turn_started`), `task_complete` (alias `turn_complete`, with
//!   `last_agent_message` and an optional `error`), `turn_aborted`, `token_count`.
//!   `user_message` / `agent_message` are persisted in the default ("legacy") history mode.
//!   Approval requests and errors are *not* persisted: the session watcher recognizes
//!   Codex's dialogs on its screen instead (`providers::dialog_on_screen`).
//! * Lines are parsed leniently: unknown or malformed ones are ignored, never guessed.
//!
//! Which rollout belongs to which hosted session: candidates are files created after the
//! launch whose `session_meta` names the session's cwd. The file the session's own
//! processes hold open (`/proc/<pid>/fd`; on Windows the Restart Manager) is its file;
//! without that evidence a candidate is taken only when it is the only one, no other hosted
//! Codex session in that cwd is still waiting for its id, and nothing outside the waiting
//! sessions holds it open or runs Codex in that folder (`held_elsewhere`,
//! `pty::cli_running_in`). Two sessions sharing a cwd are never guessed, and neither is a
//! Codex outside Workbench.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::transcript::{clean_line, is_uuid, tail_chars};

/// `$CODEX_HOME` (the session environment's value wins) or `~/.codex`.
pub fn codex_home(env_override: Option<&str>) -> PathBuf {
    if let Some(d) = env_override.filter(|d| !d.is_empty()) {
        return crate::config::expand_tilde(d);
    }
    match std::env::var("CODEX_HOME") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::home_dir().unwrap_or_default().join(".codex"),
    }
}

/// The `session_meta` line of a rollout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Meta {
    pub id: String,
    pub cwd: String,
    pub originator: Option<String>,
    /// `cli`, `vscode`, `exec`, `mcp`, … (`None`: not a string, e.g. a sub-agent).
    pub source: Option<String>,
    pub forked_from: Option<String>,
    /// A sub-agent's thread (never a hosted session itself).
    pub child: bool,
    /// Start time (ms).
    pub started_at: Option<i64>,
    pub git_branch: Option<String>,
}

fn parse_ts(s: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s?).ok().map(|d| d.timestamp_millis())
}

/// Parse a rollout's first line.
pub fn parse_meta(line: &[u8]) -> Option<Meta> {
    let v: Value = serde_json::from_slice(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let p = v.get("payload")?;
    let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
    let id = s("id").filter(|id| is_uuid(id))?;
    Some(Meta {
        id,
        cwd: s("cwd")?,
        originator: s("originator"),
        source: match p.get("source") {
            None => Some("cli".into()),
            Some(Value::String(x)) => Some(x.clone()),
            Some(_) => None,
        },
        forked_from: s("forked_from_id").filter(|f| is_uuid(f)),
        child: p.get("parent_thread_id").is_some_and(|x| !x.is_null()) || p.get("agent_nickname").is_some_and(|x| !x.is_null()),
        started_at: parse_ts(p.get("timestamp").and_then(Value::as_str)).or_else(|| parse_ts(v.get("timestamp").and_then(Value::as_str))),
        git_branch: p.pointer("/git/branch").and_then(Value::as_str).filter(|b| !b.is_empty()).map(str::to_string),
    })
}

/// Whether a session of this source is an interactive one Workbench could host.
fn interactive(m: &Meta) -> bool {
    !m.child && matches!(m.source.as_deref(), Some("cli") | Some("vscode"))
}

/// What one rollout line tells us.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TurnStarted { turn_id: String },
    TurnComplete { turn_id: String, last_message: Option<String>, error: Option<String> },
    TurnAborted { reason: String },
    AgentMessage(String),
    UserMessage(String),
    Model(String),
    Effort(String),
    ContextPct(f64),
    /// The usage windows of the account (`token_count.rate_limits`).
    RateLimits(Vec<super::usage::Reported>),
    /// The turn failed because the account is at its usage limit (the error's text).
    UsageLimit(String),
}

fn user_text(p: &Value) -> Option<String> {
    // Legacy: {"type":"user_message","message":"…"}; paginated: an `item_completed`
    // whose item is {"type":"UserMessage","content":[{"type":"text","text":"…"}]}.
    if let Some(m) = p.get("message").and_then(Value::as_str) {
        return Some(m.to_string());
    }
    let items = p.pointer("/item/content")?.as_array()?;
    let text: Vec<&str> = items.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

/// A rollout line's own timestamp (ms).
pub fn line_time(line: &[u8]) -> Option<i64> {
    #[derive(serde::Deserialize)]
    struct Stamp<'a> {
        #[serde(borrow)]
        timestamp: Option<std::borrow::Cow<'a, str>>,
    }
    let s: Stamp = serde_json::from_slice(line).ok()?;
    parse_ts(s.timestamp.as_deref())
}

/// Parse one rollout line.
pub fn parse_line(line: &[u8]) -> Vec<Event> {
    let Ok(v) = serde_json::from_slice::<Value>(line) else { return vec![] };
    let Some(p) = v.get("payload") else { return vec![] };
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    match v.get("type").and_then(Value::as_str) {
        Some("event_msg") => match s("type") {
            Some("task_started") | Some("turn_started") => vec![Event::TurnStarted { turn_id: s("turn_id").unwrap_or_default().to_string() }],
            Some("task_complete") | Some("turn_complete") => {
                let error = p.get("error").filter(|e| !e.is_null()).map(|e| {
                    let m = e.get("message").and_then(Value::as_str).or_else(|| e.as_str()).unwrap_or("The turn failed");
                    clean_line(m, 300)
                });
                let mut out = vec![];
                // The error's code, not its wording, says the account is at its limit.
                if p.pointer("/error/codex_error_info").and_then(Value::as_str) == Some("usage_limit_exceeded") {
                    out.push(Event::UsageLimit(p.pointer("/error/message").and_then(Value::as_str).unwrap_or_default().chars().take(400).collect()));
                }
                out.push(Event::TurnComplete {
                    turn_id: s("turn_id").unwrap_or_default().to_string(),
                    last_message: s("last_agent_message").filter(|m| !m.trim().is_empty()).map(str::to_string),
                    error,
                });
                out
            }
            Some("turn_aborted") => vec![Event::TurnAborted { reason: s("reason").unwrap_or("interrupted").to_string() }],
            Some("agent_message") => s("message").filter(|m| !m.trim().is_empty()).map(|m| vec![Event::AgentMessage(m.to_string())]).unwrap_or_default(),
            Some("user_message") => user_text(p).map(|m| vec![Event::UserMessage(m)]).unwrap_or_default(),
            Some("item_completed") => match p.pointer("/item/type").and_then(Value::as_str) {
                Some("UserMessage") => user_text(p).map(|m| vec![Event::UserMessage(m)]).unwrap_or_default(),
                Some("AgentMessage") => user_text(p).map(|m| vec![Event::AgentMessage(m)]).unwrap_or_default(),
                _ => vec![],
            },
            Some("token_count") => {
                let used = p.pointer("/info/last_token_usage/total_tokens").and_then(Value::as_f64);
                let window = p.pointer("/info/model_context_window").and_then(Value::as_f64);
                let mut out = vec![];
                if let (Some(u), Some(w)) = (used, window) {
                    if w > 0.0 && u.is_finite() {
                        out.push(Event::ContextPct(((u * 100.0 / w).clamp(0.0, 100.0) * 10.0).round() / 10.0));
                    }
                }
                let windows = p.get("rate_limits").map(super::usage::codex_reported).unwrap_or_default();
                if !windows.is_empty() {
                    out.push(Event::RateLimits(windows));
                }
                out
            }
            _ => vec![],
        },
        Some("turn_context") => {
            let mut out = vec![];
            if let Some(m) = s("model").filter(|m| !m.is_empty()) {
                out.push(Event::Model(clean_line(m, 60)));
            }
            if let Some(e) = s("effort").filter(|e| !e.is_empty()) {
                out.push(Event::Effort(clean_line(e, 20)));
            }
            out
        }
        _ => vec![],
    }
}

/// Codex session state from rollout events (runtime only).
#[derive(Debug, Default)]
pub struct Tracker {
    last_completed: Option<String>,
    /// The latest agent message of the running turn (a fallback when the completion
    /// carries none).
    turn_message: Option<String>,
    /// A turn started and has not ended yet.
    pub turn_open: bool,
}

/// What applying events changed.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    pub changed: bool,
    pub attention: bool,
    /// A turn completed or was aborted: Codex is back at its prompt.
    pub turn_ended: bool,
}

/// Apply rollout events to the session's `AgentInfo`.
pub fn apply(agent: &mut super::AgentInfo, t: &mut Tracker, events: Vec<Event>, now: i64) -> Applied {
    use super::AgentState as S;
    let mut out = Applied::default();
    // Lines read after the process exited (the final read) update the answer, not the state.
    let exited = agent.state == S::Exited;
    let set_state = |agent: &mut super::AgentInfo, s: S, out: &mut Applied| {
        if agent.state != s && !exited {
            agent.state = s;
            out.changed = true;
        }
    };
    for e in events {
        match e {
            Event::TurnStarted { .. } => {
                set_state(agent, S::Working, &mut out);
                if agent.unread || agent.attention.is_some() {
                    agent.unread = false;
                    agent.attention = None;
                    out.changed = true;
                }
                t.turn_message = None;
                t.turn_open = true;
                agent.last_event_at = now;
            }
            Event::TurnComplete { turn_id, last_message, error } => {
                if !turn_id.is_empty() && t.last_completed.as_deref() == Some(turn_id.as_str()) {
                    continue;
                }
                t.last_completed = Some(turn_id);
                t.turn_open = false;
                out.turn_ended = true;
                if let Some(m) = last_message.or_else(|| t.turn_message.take()) {
                    let tail = tail_chars(&m, 600);
                    if agent.last_message.as_deref() != Some(tail.as_str()) {
                        agent.last_message = Some(tail);
                        out.changed = true;
                    }
                }
                agent.last_event_at = now;
                out.changed = true;
                if exited {
                    continue;
                }
                match error {
                    Some(err) => {
                        set_state(agent, S::Error, &mut out);
                        agent.attention = Some(err);
                    }
                    None => {
                        set_state(agent, S::Idle, &mut out);
                        agent.attention = None;
                        agent.unread = true;
                    }
                }
                out.attention = true;
            }
            Event::TurnAborted { .. } => {
                if matches!(agent.state, S::Working | S::Starting) {
                    set_state(agent, S::Idle, &mut out);
                }
                t.turn_message = None;
                t.turn_open = false;
                out.turn_ended = true;
                agent.last_event_at = now;
            }
            Event::AgentMessage(m) => t.turn_message = Some(m),
            Event::UserMessage(_) => {}
            Event::Model(m) => {
                if agent.model.as_deref() != Some(m.as_str()) {
                    agent.model = Some(m);
                    out.changed = true;
                }
            }
            Event::Effort(e) => {
                if agent.effort.as_deref() != Some(e.as_str()) {
                    agent.effort = Some(e);
                    out.changed = true;
                }
            }
            Event::ContextPct(p) => {
                if agent.context_pct != Some(p) {
                    agent.context_pct = Some(p);
                    out.changed = true;
                }
            }
            // Account usage is kept apart from the session (`Terminals::note_codex_usage`).
            Event::RateLimits(_) | Event::UsageLimit(_) => {}
        }
    }
    out
}

// ---------------------------------------------------------------- discovery

/// A hosted Codex session still waiting for its rollout.
#[derive(Debug, Clone)]
pub struct Pending {
    pub terminal_id: String,
    pub cwd: String,
    /// When the process started (ms).
    pub launched_at: i64,
    /// The session this one forks (its rollout names it in `forked_from_id`).
    pub fork_of: Option<String>,
}

/// A rollout file that could be a pending session's.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub meta: Meta,
    /// Created (or first written) at, ms.
    pub created_at: i64,
    /// Hosted terminals whose processes hold the file open.
    pub holders: Vec<String>,
}

/// Slack for clocks and file timestamps.
const LAUNCH_SLACK_MS: i64 = 2000;

/// Whether `c` fits `p` at all (cwd, time, kind of session).
fn fits(p: &Pending, c: &Candidate) -> bool {
    interactive(&c.meta)
        && crate::util::os::path::same_dir(&c.meta.cwd, &p.cwd)
        && c.created_at >= p.launched_at - LAUNCH_SLACK_MS
        && c.meta.forked_from == p.fork_of
}

/// What the evidence says about `me`'s rollout.
#[derive(Debug)]
pub enum Choice<'a> {
    /// Its own processes hold the file open.
    Certain(&'a Candidate),
    /// The only fitting file none of the waiting sessions holds, while no other hosted
    /// session there waits for one. It is `me`'s only if nothing outside the hosted
    /// sessions has it open or runs Codex in that folder (the caller checks, through
    /// `util::os::session`).
    Unproven(&'a Candidate),
    Unknown,
}

/// The rollout of `me`, if the evidence is unambiguous. `pending` are the hosted Codex
/// sessions (of `me`'s Codex home) waiting for a rollout, including `me`; `claimed` are
/// session ids other terminals already have.
pub fn choose<'a>(me: &Pending, pending: &[Pending], candidates: &'a [Candidate], claimed: &HashSet<String>) -> Choice<'a> {
    let fitting: Vec<&Candidate> = candidates.iter().filter(|c| !claimed.contains(&c.meta.id) && fits(me, c)).collect();
    // Our own processes hold it open: certain.
    let mine: Vec<&&Candidate> = fitting.iter().filter(|c| c.holders.iter().any(|h| *h == me.terminal_id)).collect();
    if let Some(c) = mine.into_iter().max_by_key(|c| c.created_at) {
        return Choice::Certain(c);
    }
    // Without that evidence: the only candidate no waiting session holds, and nobody else
    // in this cwd (and of the same launch kind) is waiting for one.
    let free: Vec<&&Candidate> = fitting.iter().filter(|c| c.holders.is_empty()).collect();
    let rivals = pending.iter().filter(|o| o.terminal_id != me.terminal_id && crate::util::os::path::same_dir(&o.cwd, &me.cwd) && o.fork_of == me.fork_of).count();
    match (free.as_slice(), rivals) {
        ([only], 0) => Choice::Unproven(only),
        _ => Choice::Unknown,
    }
}

/// Whether a process outside the process sessions `ours` holds `path` open (blocking;
/// reads every readable `/proc/<pid>/fd`, on Windows asks the Restart Manager). A rollout
/// another Codex keeps open (one in a terminal outside Workbench, an editor extension) is
/// that session's, never ours.
pub fn held_elsewhere(path: &Path, ours: &HashSet<u32>) -> bool {
    crate::util::os::session::held_outside(path, ours)
}

/// Date folders to look in for sessions started at `since` (local time) until now.
fn day_dirs(home: &Path, since_ms: i64) -> Vec<PathBuf> {
    use chrono::{Datelike, Local, TimeZone};
    let now = Local::now();
    let start = Local.timestamp_millis_opt(since_ms - LAUNCH_SLACK_MS).single().unwrap_or(now);
    let mut out = vec![];
    let mut d = start.date_naive();
    let last = now.date_naive();
    while d <= last && out.len() < 8 {
        out.push(home.join("sessions").join(format!("{:04}", d.year())).join(format!("{:02}", d.month())).join(format!("{:02}", d.day())));
        match d.succ_opt() {
            Some(n) => d = n,
            None => break,
        }
    }
    out
}

/// The first line of a file, at most `max` bytes (`None` when longer or unreadable).
pub fn first_line(path: &Path, max: usize) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::new(f.take(max as u64 + 1));
    let mut buf = vec![];
    r.read_until(b'\n', &mut buf).ok()?;
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > max {
        return None;
    }
    Some(buf)
}

/// `session_meta` can carry the base instructions (tens of KB).
const META_MAX: usize = 512 * 1024;

fn millis(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn is_rollout_name(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

/// Rollouts created since `since_ms` (blocking).
pub fn recent_candidates(home: &Path, since_ms: i64) -> Vec<Candidate> {
    let mut out = vec![];
    for dir in day_dirs(home, since_ms) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_rollout_name(&name) {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            let created = millis(md.created().or_else(|_| md.modified()).unwrap_or(SystemTime::UNIX_EPOCH));
            if created < since_ms - LAUNCH_SLACK_MS {
                continue;
            }
            let path = e.path();
            if let Some(meta) = first_line(&path, META_MAX).as_deref().and_then(parse_meta) {
                // The thread's start time (set when the session starts; the file itself is
                // only written at the first turn).
                out.push(Candidate { created_at: meta.started_at.unwrap_or(created), path, meta, holders: vec![] });
            }
        }
    }
    out
}

/// Which of `paths` the processes of each session (`(terminal, session id = leader
/// pid)`) hold open (blocking; reads `/proc/<pid>/fd` of our own children, on Windows asks
/// the Restart Manager). Keys are the given paths.
pub fn holders(sessions: &[(String, u32)], paths: &[PathBuf]) -> HashMap<PathBuf, Vec<String>> {
    let sids: Vec<u32> = sessions.iter().map(|(_, sid)| *sid).collect();
    let held = crate::util::os::session::holders(&sids, paths);
    let mut out: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for (path, by) in held {
        let v = out.entry(path).or_default();
        for (terminal, sid) in sessions {
            if by.contains(sid) && !v.contains(terminal) {
                v.push(terminal.clone());
            }
        }
    }
    out
}

/// A session's rollout by id: the file names end in `-<id>.jsonl` (blocking; newest
/// folders first, bounded).
pub fn find_rollout(home: &Path, id: &str) -> Option<PathBuf> {
    if !is_uuid(id) {
        return None;
    }
    let suffix = format!("-{id}.jsonl");
    for day in sorted_day_dirs(home, 800) {
        let Ok(rd) = std::fs::read_dir(&day) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("rollout-") && name.ends_with(&suffix) {
                return Some(e.path());
            }
        }
    }
    None
}

/// `sessions/YYYY/MM/DD` folders, newest first, at most `max`.
fn sorted_day_dirs(home: &Path, max: usize) -> Vec<PathBuf> {
    let list = |p: &Path, len: usize| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        let n = e.file_name();
                        let n = n.to_string_lossy();
                        n.len() == len && n.bytes().all(|b| b.is_ascii_digit())
                    })
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by(|a, b| b.cmp(a));
        v
    };
    let mut out = vec![];
    for y in list(&home.join("sessions"), 4) {
        for m in list(&y, 2) {
            for d in list(&m, 2) {
                out.push(d);
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- history

/// One Codex conversation in a project.
#[derive(Debug, Clone)]
pub struct Summary {
    pub id: String,
    pub first_prompt: Option<String>,
    pub last_message: Option<String>,
    pub git_branch: Option<String>,
}

const HEAD_BYTES: u64 = 256 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;

/// Whether a user message is one Codex injects itself (instructions, environment).
pub fn injected(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty() || t.starts_with('<') || t.starts_with("# AGENTS.md") || t.starts_with("# Context from my IDE")
}

/// Summarize a rollout from its head and tail (blocking). `None` without a
/// `session_meta` line, for non-interactive sessions, or before the first prompt.
pub fn summarize(path: &Path, size: u64) -> Option<Summary> {
    let head = first_line(path, META_MAX)?;
    let meta = parse_meta(&head)?;
    if !interactive(&meta) {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; size.min(HEAD_BYTES) as usize];
    f.read_exact(&mut buf).ok()?;
    let mut first_prompt = None;
    let mut last = None;
    let mut turns = false;
    let mut visit = |line: &[u8]| {
        for e in parse_line(line) {
            match e {
                Event::UserMessage(m) if first_prompt.is_none() && !injected(&m) => first_prompt = Some(clean_line(&m, 140)),
                Event::TurnStarted { .. } => turns = true,
                Event::TurnComplete { last_message: Some(m), .. } => last = Some(tail_chars(&m, 300)),
                Event::AgentMessage(m) => last = Some(tail_chars(&m, 300)),
                _ => {}
            }
        }
    };
    for line in buf.split(|&b| b == b'\n').skip(1) {
        visit(line);
    }
    if size > HEAD_BYTES {
        let start = size.saturating_sub(TAIL_BYTES).max(HEAD_BYTES);
        let mut tail = vec![];
        if f.seek(SeekFrom::Start(start)).is_ok() && f.take(TAIL_BYTES).read_to_end(&mut tail).is_ok() {
            // The first tail line is probably partial.
            if let Some(i) = tail.iter().position(|&b| b == b'\n') {
                for line in tail[i + 1..].split(|&b| b == b'\n') {
                    visit(line);
                }
            }
        }
    }
    (turns || first_prompt.is_some() || size > HEAD_BYTES).then_some(Summary {
        id: meta.id,
        first_prompt,
        last_message: last,
        git_branch: meta.git_branch,
    })
}

/// The cwd of an interactive session's rollout, from its first line alone (blocking).
fn listed_cwd(path: &Path) -> Option<String> {
    let meta = parse_meta(&first_line(path, META_MAX)?)?;
    interactive(&meta).then_some(meta.cwd)
}

/// What the history cache knows about one rollout.
#[derive(Default)]
struct Cached {
    /// `listed_cwd`, read at `meta_size` (`None`: not read yet). The first line never
    /// changes once written; a file without a usable one is read again when it grew.
    cwd: Option<Option<String>>,
    meta_size: u64,
    /// The summary, valid while `(mtime, size)` match; only made for rollouts inside a
    /// listed project, so a first listing never parses every session the user has.
    summary: Option<(SystemTime, u64, Option<Summary>)>,
}

/// Cached rollout cwds and summaries, keyed by path.
#[derive(Default)]
pub struct HistoryCache {
    files: HashMap<PathBuf, Cached>,
}

/// Rollouts scanned per listing at most (newest first).
const SCAN_MAX: usize = 3000;

impl HistoryCache {
    /// Sessions whose cwd is `root` or inside it, newest first: `(summary, mtime ms, size)`.
    pub fn list(&mut self, home: &Path, root: &Path, limit: usize) -> Vec<(Summary, i64, u64)> {
        let root_s = root.to_string_lossy();
        let inside = |cwd: &str| crate::util::os::path::dir_within(cwd, &root_s);
        let mut files: Vec<(PathBuf, SystemTime, u64)> = vec![];
        for day in sorted_day_dirs(home, 400) {
            let Ok(rd) = std::fs::read_dir(&day) else { continue };
            for e in rd.flatten() {
                if !is_rollout_name(&e.file_name().to_string_lossy()) {
                    continue;
                }
                let Ok(md) = e.metadata() else { continue };
                if md.is_file() {
                    files.push((e.path(), md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len()));
                }
            }
            if files.len() >= SCAN_MAX {
                break;
            }
        }
        files.sort_by(|a, b| b.1.cmp(&a.1));
        files.truncate(SCAN_MAX);
        let mut out = vec![];
        let mut seen = HashSet::new();
        for (path, mtime, size) in files {
            seen.insert(path.clone());
            let c = self.files.entry(path.clone()).or_default();
            let read_meta = match &c.cwd {
                None => true,
                Some(None) => c.meta_size != size,
                Some(Some(_)) => false,
            };
            if read_meta {
                c.cwd = Some(listed_cwd(&path));
                c.meta_size = size;
            }
            // Other projects' sessions are skipped on their first line alone.
            if !c.cwd.as_ref().and_then(|x| x.as_deref()).is_some_and(|cwd| inside(cwd)) {
                continue;
            }
            let summary = match &c.summary {
                Some((m, s, v)) if *m == mtime && *s == size => v.clone(),
                _ => {
                    let v = summarize(&path, size);
                    c.summary = Some((mtime, size, v.clone()));
                    v
                }
            };
            let Some(s) = summary else { continue };
            out.push((s, millis(mtime), size));
            if out.len() >= limit {
                break;
            }
        }
        if self.files.len() > SCAN_MAX * 2 {
            self.files.retain(|p, _| seen.contains(p));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID1: &str = "019a1111-0000-7000-8000-000000000001";
    const ID2: &str = "019a2222-0000-7000-8000-000000000002";

    fn meta_line(id: &str, cwd: &str, extra: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-09-26T10:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","session_id":"{id}","timestamp":"2026-09-26T10:00:00.000Z","cwd":"{cwd}","originator":"codex-tui","cli_version":"0.157.1","source":"cli","model_provider":"openai","base_instructions":{{"text":"You are Codex"}},"git":{{"branch":"main"}}{extra}}}}}"#
        )
    }

    #[test]
    fn parses_session_meta() {
        let m = parse_meta(meta_line(ID1, "/w/p", "").as_bytes()).unwrap();
        assert_eq!((m.id.as_str(), m.cwd.as_str(), m.source.as_deref()), (ID1, "/w/p", Some("cli")));
        assert_eq!(m.started_at, Some(1_790_416_800_000));
        assert!(m.forked_from.is_none() && !m.child);
        let f = parse_meta(meta_line(ID2, "/w/p", &format!(r#","forked_from_id":"{ID1}""#)).as_bytes()).unwrap();
        assert_eq!(f.forked_from.as_deref(), Some(ID1));
        let sub = parse_meta(meta_line(ID2, "/w/p", &format!(r#","parent_thread_id":"{ID1}""#)).as_bytes()).unwrap();
        assert!(sub.child && !interactive(&sub));
        let exec = parse_meta(meta_line(ID2, "/w/p", "").replace("\"source\":\"cli\"", "\"source\":\"exec\"").as_bytes()).unwrap();
        assert!(!interactive(&exec));
        assert!(parse_meta(br#"{"type":"event_msg","payload":{}}"#).is_none());
        assert!(parse_meta(br#"{"type":"session_meta","payload":{"id":"../x","cwd":"/"}}"#).is_none());
        assert!(parse_meta(b"garbage").is_none());
    }

    #[test]
    fn parses_events() {
        assert_eq!(
            parse_line(br#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started","turn_id":"t1","model_context_window":258400}}"#),
            vec![Event::TurnStarted { turn_id: "t1".into() }]
        );
        assert_eq!(
            parse_line(br#"{"type":"event_msg","payload":{"type":"turn_started","turn_id":"t9"}}"#),
            vec![Event::TurnStarted { turn_id: "t9".into() }]
        );
        assert_eq!(
            parse_line(br#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"All green."}}"#),
            vec![Event::TurnComplete { turn_id: "t1".into(), last_message: Some("All green.".into()), error: None }]
        );
        assert_eq!(
            parse_line(br#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t2","last_agent_message":null,"error":{"message":"usage limit\nreached"}}}"#),
            vec![Event::TurnComplete { turn_id: "t2".into(), last_message: None, error: Some("usage limit reached".into()) }]
        );
        assert_eq!(
            parse_line(br#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t3","reason":"interrupted"}}"#),
            vec![Event::TurnAborted { reason: "interrupted".into() }]
        );
        assert_eq!(parse_line(br#"{"type":"event_msg","payload":{"type":"user_message","message":"fix it"}}"#), vec![Event::UserMessage("fix it".into())]);
        assert_eq!(
            parse_line(br#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"i","content":[{"type":"text","text":"hi"}]}}}"#),
            vec![Event::UserMessage("hi".into())]
        );
        assert_eq!(
            parse_line(br#"{"type":"turn_context","payload":{"cwd":"/w","model":"gpt-5.5","effort":"high","approval_policy":"on-request"}}"#),
            vec![Event::Model("gpt-5.5".into()), Event::Effort("high".into())]
        );
        assert_eq!(
            parse_line(br#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":25840},"model_context_window":258400}}}"#),
            vec![Event::ContextPct(10.0)]
        );
        assert!(parse_line(br#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#).is_empty());
        // The account's windows ride on the same event, with or without token usage.
        let full = parse_line(
            br#"{"timestamp":"2026-10-03T09:32:56.931Z","ordinal":11,"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":100.0,"window_minutes":300,"resets_at":1893456000},"secondary":{"used_percent":41.5,"window_minutes":10080,"resets_at":1893999000},"credits":null,"plan_type":null}}}"#,
        );
        match full.as_slice() {
            [Event::RateLimits(w)] => assert_eq!((w.len(), w[0].used_pct, w[0].resets_at, w[1].window_minutes), (2, 100.0, Some(1893456000000), Some(10080))),
            other => panic!("{other:?}"),
        }
        // A provider that has no limits (a local model) reports every field null.
        assert!(parse_line(br#"{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":null,"secondary":null}}}"#).is_empty());
        // The failed turn carries a code; the wording is only shown.
        let failed = parse_line(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t1\",\"last_agent_message\":null,\"error\":{\"message\":\"You\u{2019}ve hit your usage limit. Try again at 3:45 PM.\",\"codex_error_info\":\"usage_limit_exceeded\"}}}".as_bytes(),
        );
        assert!(matches!(&failed[..], [Event::UsageLimit(m), Event::TurnComplete { error: Some(_), .. }] if m.contains("usage limit")), "{failed:?}");
        let other = parse_line(br#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t2","error":{"message":"boom","codex_error_info":"server_overloaded"}}}"#);
        assert!(matches!(&other[..], [Event::TurnComplete { .. }]));
        assert!(parse_line(br#"{"type":"response_item","payload":{"type":"message"}}"#).is_empty());
        assert!(parse_line(b"{not json").is_empty());
    }

    fn agent() -> crate::terminals::AgentInfo {
        crate::terminals::test_agent()
    }

    #[test]
    fn event_state_machine() {
        use crate::terminals::AgentState as S;
        let mut a = agent();
        a.state = S::Idle;
        let mut t = Tracker::default();
        let o = apply(&mut a, &mut t, vec![Event::TurnStarted { turn_id: "t1".into() }], 5);
        assert!(o.changed && !o.attention);
        assert_eq!(a.state, S::Working);
        apply(&mut a, &mut t, vec![Event::AgentMessage("partial".into())], 6);
        let o = apply(&mut a, &mut t, vec![Event::TurnComplete { turn_id: "t1".into(), last_message: None, error: None }], 7);
        assert!(o.attention);
        assert_eq!((a.state, a.unread, a.last_message.as_deref()), (S::Idle, true, Some("partial")));
        // The same completion again (a re-read) changes nothing.
        let o = apply(&mut a, &mut t, vec![Event::TurnComplete { turn_id: "t1".into(), last_message: Some("x".into()), error: None }], 8);
        assert_eq!(o, Applied::default());
        // A new turn clears the unread answer; an interrupt ends it without one.
        apply(&mut a, &mut t, vec![Event::TurnStarted { turn_id: "t2".into() }], 9);
        assert!(!a.unread);
        let o = apply(&mut a, &mut t, vec![Event::TurnAborted { reason: "interrupted".into() }], 10);
        assert!(o.changed && !o.attention);
        assert_eq!((a.state, a.unread), (S::Idle, false));
        // A failed turn needs the user.
        apply(&mut a, &mut t, vec![Event::TurnStarted { turn_id: "t3".into() }], 11);
        let o = apply(&mut a, &mut t, vec![Event::TurnComplete { turn_id: "t3".into(), last_message: None, error: Some("quota".into()) }], 12);
        assert!(o.attention);
        assert_eq!((a.state, a.attention.as_deref()), (S::Error, Some("quota")));
        apply(&mut a, &mut t, vec![Event::Model("gpt-5.5".into()), Event::ContextPct(12.5)], 13);
        assert_eq!((a.model.as_deref(), a.context_pct), (Some("gpt-5.5"), Some(12.5)));
        // After the process exited, a late completion keeps the session exited.
        a.state = S::Exited;
        let o = apply(&mut a, &mut t, vec![Event::TurnComplete { turn_id: "t4".into(), last_message: Some("late".into()), error: None }], 14);
        assert!(!o.attention);
        assert_eq!((a.state, a.last_message.as_deref()), (S::Exited, Some("late")));
    }

    fn pending(id: &str, cwd: &str, at: i64) -> Pending {
        Pending { terminal_id: id.into(), cwd: cwd.into(), launched_at: at, fork_of: None }
    }

    fn cand(id: &str, cwd: &str, at: i64, holders: &[&str]) -> Candidate {
        Candidate {
            path: PathBuf::from(format!("/c/rollout-x-{id}.jsonl")),
            meta: Meta { id: id.into(), cwd: cwd.into(), source: Some("cli".into()), started_at: Some(at), ..Default::default() },
            created_at: at,
            holders: holders.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn picked<'a>(c: Choice<'a>) -> Option<&'a str> {
        match c {
            Choice::Certain(c) | Choice::Unproven(c) => Some(c.meta.id.as_str()),
            Choice::Unknown => None,
        }
    }

    #[test]
    fn discovery_never_guesses_between_sessions_in_one_cwd() {
        let none = HashSet::new();
        let a = pending("ta", "/w/p", 1_000_000);
        let b = pending("tb", "/w/p", 1_000_500);
        // Alone in its cwd: the one new rollout there is ours, unless something outside
        // Workbench turns out to hold it (the caller checks).
        let c1 = cand(ID1, "/w/p/", 1_003_000, &[]);
        assert!(matches!(choose(&a, std::slice::from_ref(&a), std::slice::from_ref(&c1), &none), Choice::Unproven(c) if c.meta.id == ID1));
        let held = cand(ID1, "/w/p", 1_003_000, &["ta"]);
        assert!(matches!(choose(&a, std::slice::from_ref(&a), std::slice::from_ref(&held), &none), Choice::Certain(c) if c.meta.id == ID1));
        // Older files, other folders, exec runs and claimed ids never match.
        let old = cand(ID1, "/w/p", 900_000, &[]);
        assert!(picked(choose(&a, std::slice::from_ref(&a), &[old], &none)).is_none());
        let elsewhere = cand(ID1, "/w/other", 1_003_000, &[]);
        assert!(picked(choose(&a, std::slice::from_ref(&a), &[elsewhere], &none)).is_none());
        let mut exec = c1.clone();
        exec.meta.source = Some("exec".into());
        assert!(picked(choose(&a, std::slice::from_ref(&a), &[exec], &none)).is_none());
        let claimed: HashSet<String> = [ID1.to_string()].into();
        assert!(picked(choose(&a, std::slice::from_ref(&a), std::slice::from_ref(&c1), &claimed)).is_none());
        // Two sessions waiting in one cwd: without evidence, nobody gets it…
        let both = [a.clone(), b.clone()];
        assert!(picked(choose(&a, &both, std::slice::from_ref(&c1), &none)).is_none());
        assert!(picked(choose(&b, &both, std::slice::from_ref(&c1), &none)).is_none());
        // …with it, each gets its own, and never the other's.
        let ca = cand(ID1, "/w/p", 1_003_000, &["ta"]);
        let cb = cand(ID2, "/w/p", 1_004_000, &["tb"]);
        let both_files = [ca.clone(), cb.clone()];
        assert_eq!(picked(choose(&a, &both, &both_files, &none)), Some(ID1));
        assert_eq!(picked(choose(&b, &both, &both_files, &none)), Some(ID2));
        // b's file is held by b: a (alone otherwise) must not take it.
        assert!(picked(choose(&a, std::slice::from_ref(&a), std::slice::from_ref(&cb), &none)).is_none());
        // Sessions in different folders do not compete.
        let c = pending("tc", "/w/q", 1_000_000);
        assert_eq!(picked(choose(&a, &[a.clone(), c], std::slice::from_ref(&c1), &none)), Some(ID1));
    }

    #[test]
    fn forks_match_only_their_own_origin() {
        let none = HashSet::new();
        let mut f = pending("tf", "/w/p", 1_000_000);
        f.fork_of = Some(ID1.into());
        let mut forked = cand(ID2, "/w/p", 1_002_000, &[]);
        forked.meta.forked_from = Some(ID1.into());
        let fresh = cand("019a3333-0000-7000-8000-000000000003", "/w/p", 1_002_000, &[]);
        assert_eq!(picked(choose(&f, std::slice::from_ref(&f), &[fresh.clone(), forked.clone()], &none)), Some(ID2));
        // A new session never takes a fork's file.
        let n = pending("tn", "/w/p", 1_000_000);
        assert_eq!(picked(choose(&n, &[n.clone(), f.clone()], &[forked, fresh.clone()], &none)), Some(fresh.meta.id.as_str()));
    }

    #[test]
    fn finds_rollouts_and_summarizes_them() {
        let home = tempfile::tempdir().unwrap();
        let day = home.path().join("sessions/2026/09/26");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("rollout-2026-09-26T10-00-00-{ID1}.jsonl"));
        let mut text = meta_line(ID1, "/w/p/sub", "");
        text.push('\n');
        text.push_str(r#"{"type":"event_msg","payload":{"type":"user_message","message":"<environment_context>x</environment_context>"}}"#);
        text.push('\n');
        text.push_str(r#"{"type":"event_msg","payload":{"type":"user_message","message":"Make the tests pass"}}"#);
        text.push('\n');
        text.push_str(r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#);
        text.push('\n');
        text.push_str(r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"Done"}}"#);
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        assert_eq!(find_rollout(home.path(), ID1), Some(path.clone()));
        assert_eq!(find_rollout(home.path(), ID2), None);
        let s = summarize(&path, text.len() as u64).unwrap();
        assert_eq!((s.first_prompt.as_deref(), s.last_message.as_deref(), s.git_branch.as_deref()), (Some("Make the tests pass"), Some("Done"), Some("main")));
        // A session with no turn yet is not history.
        let empty = day.join(format!("rollout-2026-09-26T10-00-01-{ID2}.jsonl"));
        std::fs::write(&empty, meta_line(ID2, "/w/p", "") + "\n").unwrap();
        let mut cache = HistoryCache::default();
        let rows = cache.list(home.path(), Path::new("/w/p"), 10);
        assert_eq!(rows.iter().map(|r| r.0.id.as_str()).collect::<Vec<_>>(), [ID1]);
        assert!(cache.list(home.path(), Path::new("/w/pp"), 10).is_empty());
        // Recent candidates (the file times are now).
        let c = recent_candidates(home.path(), crate::util::now_ms() - 60_000);
        let today = chrono::Local::now().format("%Y/%m/%d").to_string();
        if today == "2026/09/26" {
            assert_eq!(c.len(), 2);
        }
    }

    /// Unix: this test process stands in for a session. The Windows counterpart (a job and
    /// the Restart Manager) is in `util::os::session`.
    #[cfg(unix)]
    #[test]
    fn open_files_of_our_processes_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rollout-held.jsonl");
        std::fs::write(&file, "x\n").unwrap();
        let held = std::fs::File::open(&file).unwrap();
        // This test process stands in for a session leader.
        let sid = u32::try_from(nix::unistd::getsid(None).unwrap().as_raw()).unwrap();
        let found = holders(&[("me".into(), sid)], std::slice::from_ref(&file));
        assert_eq!(found.get(&file).map(|v| v.as_slice()), Some(&["me".to_string()][..]));
        let other = dir.path().join("rollout-free.jsonl");
        std::fs::write(&other, "y\n").unwrap();
        assert!(!holders(&[("me".into(), sid)], std::slice::from_ref(&other)).contains_key(&other));
        // Held by a process outside the given sessions: someone else's rollout.
        assert!(held_elsewhere(&file, &HashSet::new()));
        assert!(!held_elsewhere(&file, &[sid].into()));
        assert!(!held_elsewhere(&other, &HashSet::new()));
        drop(held);
        assert!(!held_elsewhere(&file, &HashSet::new()));
    }

    #[test]
    fn history_summarizes_only_the_projects_rollouts() {
        let home = tempfile::tempdir().unwrap();
        let day = home.path().join("sessions/2026/09/26");
        std::fs::create_dir_all(&day).unwrap();
        let turn = r#"{"type":"event_msg","payload":{"type":"user_message","message":"do it"}}"#;
        let mine = day.join(format!("rollout-2026-09-26T10-00-00-{ID1}.jsonl"));
        std::fs::write(&mine, format!("{}\n{turn}\n", meta_line(ID1, "/w/p", ""))).unwrap();
        let other = day.join(format!("rollout-2026-09-26T10-00-01-{ID2}.jsonl"));
        std::fs::write(&other, format!("{}\n{turn}\n", meta_line(ID2, "/w/elsewhere", ""))).unwrap();
        let mut cache = HistoryCache::default();
        let rows = cache.list(home.path(), Path::new("/w/p"), 10);
        assert_eq!(rows.iter().map(|r| r.0.id.as_str()).collect::<Vec<_>>(), [ID1]);
        // The other project's rollout was judged on its first line and never summarized.
        let o = &cache.files[&other];
        assert_eq!(o.cwd, Some(Some("/w/elsewhere".to_string())));
        assert!(o.summary.is_none());
        assert!(cache.files[&mine].summary.is_some());
        // A rollout whose first line is still being written is read again once it grew.
        let late = day.join("rollout-2026-09-26T10-00-02-019a3333-0000-7000-8000-000000000003.jsonl");
        std::fs::write(&late, r#"{"type":"session_me"#).unwrap();
        assert_eq!(cache.list(home.path(), Path::new("/w/p"), 10).len(), 1);
        std::fs::write(&late, format!("{}\n{turn}\n", meta_line("019a3333-0000-7000-8000-000000000003", "/w/p", ""))).unwrap();
        assert_eq!(cache.list(home.path(), Path::new("/w/p"), 10).len(), 2);
    }
}

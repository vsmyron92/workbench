//! Carrying a conversation from one account to another.
//!
//! * **Resume** (the same CLI, Claude Code to Claude Code or Codex to Codex, also onto a
//!   model of your own): the session's own file is copied into the other account's folder
//!   and the new session resumes it, so the model sees the conversation itself. Checked
//!   against Claude Code 2.1.288 and Codex 0.160.0: a transcript copied into another
//!   `CLAUDE_CONFIG_DIR`, or a rollout into another `CODEX_HOME`, is sent to the model as
//!   the history of the resumed session.
//! * **Digest** (a different CLI): the conversation read out of the file as Markdown, the
//!   user's and the assistant's words with a line for each tool call and none of the tool
//!   output; the new session is told to read it first.
//! * **Notes**: only a short note on where the old session stopped (no readable file, a
//!   CLI whose files are not read here, or `[agents] transfer = "notes"`).
//!
//! Nothing here reads a login: only the conversation files of the session being moved.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::Value;

use super::permission::redact_patterns;
use super::providers::ProviderKind;
use super::transcript::{self, clean_line, truncate_chars};
use crate::util;

/// A transcript or rollout larger than this is not copied (it would be read whole).
const MAX_COPY_BYTES: u64 = 256 * 1024 * 1024;
/// Claude Code's sidecar folder of a session (large tool outputs, sub-agents) is copied up to this.
const MAX_SIDECAR_BYTES: u64 = 128 * 1024 * 1024;
/// How much of a large file a digest reads: the start (the task) and the end (where it stopped).
const DIGEST_HEAD: usize = 2 * 1024 * 1024;
const DIGEST_TAIL: usize = 62 * 1024 * 1024;
/// The most a digest file holds, and the most a prompt carries inline.
pub const FILE_BUDGET: usize = 60_000;
pub const INLINE_BUDGET: usize = 7_000;
/// A single turn is cut to this in the digest.
const TURN_CAP: usize = 4_000;

/// `[agents] transfer`: what a session moved to another account takes along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// The conversation (the default).
    Conversation,
    /// Only a short note on where it stopped.
    Notes,
}

impl Transfer {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "conversation" => Some(Self::Conversation),
            "notes" => Some(Self::Notes),
            _ => None,
        }
    }

    pub fn of(agents: &crate::config::global::AgentsConfig) -> Self {
        agents.transfer.as_deref().and_then(Self::parse).unwrap_or(Self::Conversation)
    }
}

/// How a conversation is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Resume,
    Digest,
    Notes,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Digest => "digest",
            Self::Notes => "notes",
        }
    }
}

/// The CLIs whose conversation files are read here.
fn readable(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::Claude | ProviderKind::Codex)
}

/// How a conversation of a `from` CLI goes to a `to` CLI (`have_file`: its file was found).
pub fn how(from: ProviderKind, to: ProviderKind, transfer: Transfer, have_file: bool) -> How {
    if transfer == Transfer::Notes || !have_file || !readable(from) {
        How::Notes
    } else if from == to {
        How::Resume
    } else {
        How::Digest
    }
}

// ---------------------------------------------------------------- reading

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

fn push(turns: &mut Vec<Turn>, role: Role, text: &str) {
    let text = redact_patterns(text.trim());
    if text.is_empty() {
        return;
    }
    match turns.last_mut() {
        Some(t) if t.role == role => {
            t.text.push_str("\n\n");
            t.text.push_str(&text);
        }
        _ => turns.push(Turn { role, text }),
    }
}

/// Text a CLI put into the user's side that the user did not write.
fn injected(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty() || t.starts_with("<system-reminder>") || t.starts_with("<local-command") || t.starts_with("<command-") || t.starts_with("Caveat:")
}

/// One line for a tool call: what it was and on what, never its output.
fn tool_line(name: &str, input: Option<&Value>) -> String {
    let s = |k: &str| input.and_then(|i| i.get(k)).and_then(Value::as_str);
    let what = match name {
        "Bash" => s("command").map(|c| clean_line(c, 140)),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => s("file_path").or_else(|| s("notebook_path")).map(|p| clean_line(p, 160)),
        "Glob" | "Grep" => s("pattern").map(|p| clean_line(p, 100)),
        "WebFetch" => s("url").map(|u| clean_line(u, 160)),
        "WebSearch" => s("query").map(|q| clean_line(q, 100)),
        "Task" | "Agent" => s("description").map(|d| clean_line(d, 100)),
        _ => None,
    };
    match what {
        Some(w) => format!("[used {name}: {w}]"),
        None => format!("[used {name}]"),
    }
}

/// The turns of a Claude Code transcript (JSON lines): what the user and the assistant said,
/// and a line per tool call. Sub-agent work, meta records and thinking are left out.
pub fn claude_turns(bytes: &[u8]) -> Vec<Turn> {
    let mut turns = vec![];
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) || v.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let content = v.pointer("/message/content");
        match v.get("type").and_then(Value::as_str) {
            Some("user") => match content {
                Some(Value::String(s)) if !injected(s) => push(&mut turns, Role::User, s),
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        if b.get("type").and_then(Value::as_str) == Some("text") {
                            if let Some(t) = b.get("text").and_then(Value::as_str).filter(|t| !injected(t)) {
                                push(&mut turns, Role::User, t);
                            }
                        }
                    }
                }
                _ => {}
            },
            Some("assistant") => {
                let Some(Value::Array(blocks)) = content else { continue };
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str) {
                                push(&mut turns, Role::Assistant, t);
                            }
                        }
                        Some("tool_use") => push(&mut turns, Role::Assistant, &tool_line(b.get("name").and_then(Value::as_str).unwrap_or("a tool"), b.get("input"))),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    turns
}

/// The turns of a Codex rollout: the user's and the agent's messages, and a line per tool call.
pub fn codex_turns(bytes: &[u8]) -> Vec<Turn> {
    let mut turns = vec![];
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        let Some(p) = v.get("payload") else { continue };
        let msg = || p.get("message").and_then(Value::as_str);
        match (v.get("type").and_then(Value::as_str), p.get("type").and_then(Value::as_str)) {
            (Some("event_msg"), Some("user_message")) => {
                if let Some(m) = msg().filter(|m| !super::codex::injected(m)) {
                    push(&mut turns, Role::User, m);
                }
            }
            (Some("event_msg"), Some("agent_message")) => {
                if let Some(m) = msg() {
                    push(&mut turns, Role::Assistant, m);
                }
            }
            (Some("response_item"), Some("function_call")) => {
                let name = p.get("name").and_then(Value::as_str).unwrap_or("a tool");
                // Arguments are a JSON string: `{"command": ["bash", "-lc", "ls"]}` or `{"cmd": "ls"}`.
                let args = p.get("arguments").and_then(Value::as_str).and_then(|a| serde_json::from_str::<Value>(a).ok());
                let command = args.as_ref().and_then(|a| {
                    a.get("cmd").and_then(Value::as_str).map(str::to_string).or_else(|| {
                        a.get("command").and_then(|c| match c {
                            Value::String(s) => Some(s.clone()),
                            Value::Array(parts) => Some(parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
                            _ => None,
                        })
                    })
                });
                push(&mut turns, Role::Assistant, &match command {
                    Some(c) => format!("[used {name}: {}]", clean_line(&c, 140)),
                    None => format!("[used {name}]"),
                });
            }
            _ => {}
        }
    }
    turns
}

/// What a digest says: the turns that fit, and how many there were.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub markdown: String,
    pub shown: usize,
    pub total: usize,
}

impl Rendered {
    #[cfg(test)]
    pub fn truncated(&self) -> bool {
        self.shown < self.total
    }
}

fn cap_middle(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let half = max / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().rev().take(half).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}\n[… cut …]\n{tail}")
}

/// The turns as Markdown of at most about `budget` characters: the first turn (the task) and
/// as many of the latest as fit, with a line saying how many were left out between.
pub fn render(turns: &[Turn], budget: usize) -> Rendered {
    let section = |t: &Turn, cap: usize| format!("## {}\n{}\n\n", if t.role == Role::User { "User" } else { "Assistant" }, cap_middle(&t.text, cap));
    let all: Vec<String> = turns.iter().map(|t| section(t, TURN_CAP)).collect();
    let total = all.len();
    if total == 0 {
        return Rendered { markdown: String::new(), shown: 0, total: 0 };
    }
    if all.iter().map(String::len).sum::<usize>() <= budget {
        return Rendered { markdown: all.concat(), shown: total, total };
    }
    let first = section(&turns[0], TURN_CAP * 3 / 4);
    let mut room = budget.saturating_sub(first.len() + 80);
    let mut tail: Vec<&String> = vec![];
    for s in all.iter().skip(1).rev() {
        if s.len() > room {
            break;
        }
        room -= s.len();
        tail.push(s);
    }
    // The first turn counts when it is also the only one that fits.
    tail.reverse();
    let left_out = total - 1 - tail.len();
    let mut markdown = first;
    if left_out > 0 {
        markdown.push_str(&format!("[… {left_out} earlier turn{} left out …]\n\n", if left_out == 1 { "" } else { "s" }));
    }
    markdown.extend(tail.iter().map(|s| s.as_str()));
    Rendered { markdown, shown: 1 + tail.len(), total }
}

/// The part of a conversation file a digest reads (blocking): all of it, or for a very large one the
/// start and the end.
pub fn read_for_digest(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len() as usize;
    if len <= DIGEST_HEAD + DIGEST_TAIL {
        let mut b = Vec::with_capacity(len);
        f.read_to_end(&mut b)?;
        return Ok(b);
    }
    let mut head = vec![0; DIGEST_HEAD];
    f.read_exact(&mut head)?;
    head.truncate(head.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1));
    f.seek(SeekFrom::Start((len - DIGEST_TAIL) as u64))?;
    let mut tail = Vec::with_capacity(DIGEST_TAIL);
    f.read_to_end(&mut tail)?;
    let start = tail.iter().position(|b| *b == b'\n').map_or(tail.len(), |i| i + 1);
    head.extend_from_slice(&tail[start..]);
    Ok(head)
}

// ---------------------------------------------------------------- copying

/// The bytes up to and including the last newline: a file another process is still writing may end in half a line.
fn whole_lines(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1)]
}

fn read_capped(path: &Path) -> anyhow::Result<Vec<u8>> {
    let len = std::fs::metadata(path).with_context(|| format!("{} is not readable", path.display()))?.len();
    anyhow::ensure!(len <= MAX_COPY_BYTES, "{} is too large to copy ({} MB)", path.display(), len / 1_000_000);
    Ok(std::fs::read(path)?)
}

/// Copy a folder's files (no links) while the total stays under `left`; returns the bytes copied.
fn copy_dir(from: &Path, to: &Path, left: &mut u64) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    util::fs::set_mode(to, 0o700);
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let Ok(meta) = std::fs::symlink_metadata(&src) else { continue };
        if meta.is_dir() {
            copy_dir(&src, &dst, left)?;
        } else if meta.is_file() && meta.len() <= *left {
            std::fs::copy(&src, &dst)?;
            util::fs::set_mode(&dst, 0o600);
            *left -= meta.len();
        }
    }
    Ok(())
}

/// Copy a Claude Code transcript into another account's folder, where `claude --resume <id>`
/// finds it, with the folder next to it that holds the session's large tool outputs and
/// sub-agent work. Returns the new path.
pub fn copy_claude(src: &Path, dest_claude_dir: &Path, cwd: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let bytes = read_capped(src)?;
    let dest = transcript::transcript_path(dest_claude_dir, cwd, id);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
        util::fs::set_mode(dest_claude_dir, 0o700);
        util::fs::set_mode(&dest_claude_dir.join("projects"), 0o700);
        util::fs::set_mode(dir, 0o700);
    }
    util::fs::write_atomic(&dest, whole_lines(&bytes), 0o600)?;
    let side = src.with_extension("");
    if side.is_dir() {
        let mut left = MAX_SIDECAR_BYTES;
        if let Err(e) = copy_dir(&side, &dest.with_extension(""), &mut left) {
            // The conversation resumes without them; the model is only missing some old tool output.
            tracing::debug!("session folder not fully copied: {e}");
        }
    }
    Ok(dest)
}

/// Copy a Codex rollout into another `CODEX_HOME`, under the same dated path.
pub fn copy_codex(src: &Path, src_home: &Path, dest_home: &Path) -> anyhow::Result<PathBuf> {
    let bytes = read_capped(src)?;
    let name = src.file_name().and_then(|n| n.to_str()).context("the rollout has no name")?;
    let rel = match src.strip_prefix(src_home) {
        Ok(r) if r.starts_with("sessions") => r.to_path_buf(),
        // `rollout-2026-10-03T11-01-56-<id>.jsonl` belongs under sessions/2026/10/03.
        _ => {
            let date = name.strip_prefix("rollout-").and_then(|n| n.get(..10)).context("the rollout's name has no date")?;
            let mut parts = date.split('-');
            let (y, m, d) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            anyhow::ensure!(y.len() == 4 && m.len() == 2 && d.len() == 2 && date.bytes().all(|b| b.is_ascii_digit() || b == b'-'), "the rollout's name has no date");
            PathBuf::from("sessions").join(y).join(m).join(d).join(name)
        }
    };
    let dest = dest_home.join(rel);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    util::fs::write_atomic(&dest, whole_lines(&bytes), 0o600)?;
    Ok(dest)
}

/// Write a digest where the new session can read it: a folder of its own under `root`
/// (`data_dir/handoffs`). Returns the folder and the file.
pub fn write_digest(root: &Path, markdown: &str) -> anyhow::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(root)?;
    util::fs::set_mode(root, 0o700);
    let dir = root.join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&dir)?;
    util::fs::set_mode(&dir, 0o700);
    let file = dir.join("conversation.md");
    util::fs::write_atomic(&file, markdown.as_bytes(), 0o600)?;
    Ok((dir, file))
}

/// Remove digests older than `max_age` (blocking, best effort).
pub fn prune(root: &Path, max_age: std::time::Duration) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

// ---------------------------------------------------------------- what the new session is told

/// Why the old session stopped, when it did (else the user simply moved the work).
fn why(limited: bool) -> &'static str {
    if limited { ", which stopped because that account reached its usage limit" } else { "" }
}

/// After a resume: the model has the conversation, it only needs to know the account changed.
pub fn resume_prompt(old_label: &str, new_label: &str, limited: bool) -> String {
    let because = if limited { format!(", because {old_label} reached its usage limit") } else { String::new() };
    format!(
        "This is the same conversation, moved from {old_label} to {new_label}{because}. Carry on with what was in progress; check git status first if you are unsure where it stopped."
    )
}

/// After a digest: where the conversation is (a file, or inline when it is short).
pub fn digest_prompt(old_label: &str, new_label: &str, title: &str, limited: bool, file: Option<&Path>, inline: Option<&str>) -> String {
    let mut p = format!(
        "This session continues a conversation that was held in a session on {old_label}{}. You are on {new_label} now, in the same folder. The session was titled: {}.\n",
        why(limited),
        truncate_chars(title, 80)
    );
    match (file, inline) {
        (Some(f), _) => p.push_str(&format!(
            "\nThe conversation so far is in {} (Markdown: what was said and a line for each tool call, without tool output). Read it first.\n",
            f.display()
        )),
        (None, Some(text)) => p.push_str(&format!("\nThe conversation so far (what was said and a line for each tool call, without tool output):\n\n{text}\n")),
        (None, None) => {}
    }
    p.push_str("\nThen look at the state of the repository (git status and git diff) and carry on with what was in progress instead of starting over.");
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(vals: &[Value]) -> Vec<u8> {
        vals.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n").into_bytes()
    }

    #[test]
    fn a_claude_transcript_becomes_turns() {
        let t = lines(&[
            json!({"type":"queue-operation","operation":"enqueue","content":"x"}),
            json!({"type":"user","isSidechain":false,"message":{"role":"user","content":"fix the build"},"sessionId":"s"}),
            json!({"type":"attachment","attachment":{"type":"environment"}}),
            json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat: ignore</local-command-caveat>"}}),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"secret thoughts","signature":"sig"},{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build 2>&1 | tail -20"}}]}}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"error[E0432]: unresolved import"}]}}),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/p/src/lib.rs","old_string":"a","new_string":"b"}},{"type":"text","text":"Fixed the import."}]}}),
            json!({"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"sub-agent chatter"}]}}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>hook says hi</system-reminder>"},{"type":"text","text":"thanks, now the tests (token=ghp_abcdefghijklmnopqrstuvwxyz0123456789)"}]}}),
        ]);
        let turns = claude_turns(&t);
        let roles: Vec<Role> = turns.iter().map(|t| t.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
        assert_eq!(turns[0].text, "fix the build");
        // Thinking, tool output and sub-agent work stay out; each tool call is one line.
        assert_eq!(turns[1].text, "Looking.\n\n[used Bash: cargo build 2>&1 | tail -20]\n\n[used Edit: /p/src/lib.rs]\n\nFixed the import.");
        assert!(!turns.iter().any(|t| t.text.contains("secret thoughts") || t.text.contains("E0432") || t.text.contains("chatter") || t.text.contains("hook says")));
        // A credential the user pasted is masked.
        assert!(turns[2].text.starts_with("thanks, now the tests") && !turns[2].text.contains("ghp_abcdefghijklmnop"), "{}", turns[2].text);
        assert!(claude_turns(b"not json\n\n{}").is_empty());
    }

    #[test]
    fn a_codex_rollout_becomes_turns() {
        let t = lines(&[
            json!({"type":"session_meta","payload":{"id":"x","cwd":"/w"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"<environment_context>cwd</environment_context>"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"add a test"}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}","call_id":"c"}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls -la\"}","call_id":"d"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"ok"}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"Added it."}}),
        ]);
        let turns = codex_turns(&t);
        assert_eq!(turns.len(), 2);
        assert_eq!((turns[0].role, turns[0].text.as_str()), (Role::User, "add a test"));
        assert_eq!(turns[1].text, "[used shell: bash -lc cargo test]\n\n[used exec_command: ls -la]\n\nAdded it.");
    }

    #[test]
    fn a_long_conversation_keeps_its_task_and_its_end() {
        let turns: Vec<Turn> = (0..60)
            .map(|i| Turn { role: if i % 2 == 0 { Role::User } else { Role::Assistant }, text: format!("turn {i} {}", "x".repeat(900)) })
            .collect();
        let all = render(&turns, 1_000_000);
        assert_eq!((all.shown, all.total, all.truncated()), (60, 60, false));
        let r = render(&turns, 10_000);
        assert!(r.truncated() && r.markdown.len() <= 10_500, "{}", r.markdown.len());
        assert!(r.markdown.starts_with("## User\nturn 0 "), "the task comes first");
        assert!(r.markdown.contains("earlier turns left out"));
        assert!(r.markdown.contains("turn 59 ") && !r.markdown.contains("turn 30 "), "the end is kept, the middle is not");
        // What is said to be shown is what is there, whole.
        assert_eq!(r.markdown.matches("## User\n").count() + r.markdown.matches("## Assistant\n").count(), r.shown);
        assert!(render(&[], 1000).markdown.is_empty());
        // One huge turn is cut in the middle, not dropped.
        let big = [Turn { role: Role::User, text: "a".repeat(20_000) }];
        let r = render(&big, 50_000);
        assert!(r.markdown.contains("[… cut …]") && r.markdown.len() < 5_000);
    }

    #[test]
    fn the_cli_pair_decides_how_a_conversation_goes() {
        use ProviderKind::*;
        let t = Transfer::Conversation;
        assert_eq!(how(Claude, Claude, t, true), How::Resume);
        assert_eq!(how(Codex, Codex, t, true), How::Resume);
        assert_eq!(how(Claude, Codex, t, true), How::Digest);
        assert_eq!(how(Codex, Aider, t, true), How::Digest);
        assert_eq!(how(Claude, Custom, t, true), How::Digest);
        // No file, a CLI whose files are not read here, or the setting.
        assert_eq!(how(Claude, Claude, t, false), How::Notes);
        assert_eq!(how(Gemini, Claude, t, true), How::Notes);
        assert_eq!(how(Aider, Aider, t, true), How::Notes);
        assert_eq!(how(Claude, Claude, Transfer::Notes, true), How::Notes);
        assert_eq!((Transfer::parse("notes"), Transfer::parse("conversation"), Transfer::parse("x")), (Some(Transfer::Notes), Some(Transfer::Conversation), None));
    }

    #[test]
    fn copies_a_claude_transcript_whole_lines_and_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let cwd = dir.path().join("proj");
        let id = "11111111-2222-4333-8444-555555555555";
        let src = transcript::transcript_path(&a, &cwd, id);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        // The session is still writing: the last line is half there.
        std::fs::write(&src, "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n{\"type\":\"us").unwrap();
        let side = src.with_extension("");
        std::fs::create_dir_all(side.join("tool-results")).unwrap();
        std::fs::write(side.join("tool-results/out.txt"), "big output").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", side.join("link")).unwrap();

        let dest = copy_claude(&src, &b, &cwd, id).unwrap();
        assert_eq!(dest, transcript::transcript_path(&b, &cwd, id));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n");
        assert_eq!(std::fs::read_to_string(dest.with_extension("").join("tool-results/out.txt")).unwrap(), "big output");
        assert!(!dest.with_extension("").join("link").exists(), "links are not followed");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&dest).unwrap().permissions()) & 0o777, 0o600);
        // Again, after the session went on: the file is replaced.
        std::fs::write(&src, "{\"type\":\"user\"}\n").unwrap();
        copy_claude(&src, &b, &cwd, id).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "{\"type\":\"user\"}\n");
        assert!(copy_claude(&a.join("missing.jsonl"), &b, &cwd, id).is_err());
    }

    #[test]
    fn copies_a_codex_rollout_under_its_dated_path() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let name = "rollout-2026-10-03T11-01-56-01a1016d-8459-7410-bb89-262e6f8aec4d.jsonl";
        let src = a.join("sessions/2026/10/03").join(name);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "{\"type\":\"session_meta\"}\n").unwrap();
        let dest = copy_codex(&src, &a, &b).unwrap();
        assert_eq!(dest, b.join("sessions/2026/10/03").join(name));
        assert!(dest.is_file());
        // From outside the home (an archive), the name says where it goes.
        let elsewhere = dir.path().join("archived").join(name);
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "{}\n").unwrap();
        let c = dir.path().join("c");
        assert_eq!(copy_codex(&elsewhere, &a, &c).unwrap(), c.join("sessions/2026/10/03").join(name));
        let bad = dir.path().join("notes.jsonl");
        std::fs::write(&bad, "{}\n").unwrap();
        assert!(copy_codex(&bad, &a, &c).is_err());
    }

    #[test]
    fn a_digest_is_written_privately_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("handoffs");
        let (d, f) = write_digest(&root, "## User\nhi\n").unwrap();
        assert_eq!((f.file_name().unwrap(), std::fs::read_to_string(&f).unwrap().as_str()), (std::ffi::OsStr::new("conversation.md"), "## User\nhi\n"));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&d).unwrap().permissions()) & 0o777, 0o700);
        prune(&root, std::time::Duration::from_secs(3600));
        assert!(d.is_dir(), "a new one stays");
        prune(&root, std::time::Duration::ZERO);
        assert!(!d.exists());
    }

    #[test]
    fn reads_the_head_and_tail_of_a_very_large_file_in_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.jsonl");
        let line = format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}\n", "z".repeat(1000));
        std::fs::write(&p, line.repeat(10)).unwrap();
        assert_eq!(read_for_digest(&p).unwrap().len(), line.len() * 10);
        assert_eq!(whole_lines(b"a\nb"), b"a\n");
        assert_eq!(whole_lines(b"ab"), b"");
    }

    #[test]
    fn the_prompts_say_what_was_carried() {
        let r = resume_prompt("Claude Code", "Claude · Work", true);
        assert!(r.contains("same conversation") && r.contains("usage limit") && r.contains("Claude · Work"));
        assert!(resume_prompt("A", "B", false).starts_with("This is the same conversation, moved from A to B."));
        assert!(!resume_prompt("A", "B", false).contains("usage limit"));
        let f = digest_prompt("Claude Code", "Codex", "fix ci", true, Some(Path::new("/d/h/conversation.md")), None);
        assert!(f.contains("/d/h/conversation.md") && f.contains("Read it first") && f.contains("git status"));
        let i = digest_prompt("A", "B", "t", false, None, Some("## User\nhi"));
        assert!(i.contains("## User\nhi") && !i.contains("Read it first"));
    }
}

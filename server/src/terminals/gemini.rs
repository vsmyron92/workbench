//! Google Gemini CLI files on disk, read-only.
//!
//! Verified against the bundled source of `@google/gemini-cli` 0.61.0 (no session was
//! run: Gemini needs an account):
//! * Home: `$GEMINI_CLI_HOME/.gemini`, else `~/.gemini`.
//! * `projects.json` maps a project directory to a short slug:
//!   `{"projects": {"/abs/path": "slug"}}`; older versions named the folder by the
//!   sha256 (hex) of the path, which Gemini migrates.
//! * Sessions: `tmp/<slug>/chats/session-<YYYY-MM-DDTHH-MM>-<first 8 of id>.jsonl`
//!   (`.json` for older ones). A `.jsonl` file starts with `{"sessionId","projectHash",
//!   "startTime","lastUpdated","kind"}`, then messages `{"id","timestamp","type":
//!   "user"|"gemini"|…,"content"}` (a string or `[{"text"}…]` parts) and updates
//!   `{"$set":{…}}`. A `.json` file is one object with `messages`.
//! * The project directory is the working directory Gemini runs in (its `targetDir`).
//!
//! Lines are parsed leniently: anything unknown is skipped, never guessed.

use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::transcript::{clean_line, is_uuid, tail_chars};

/// `$GEMINI_CLI_HOME/.gemini` (the session environment's value wins) or `~/.gemini`.
pub fn gemini_dir(env_override: Option<&str>) -> PathBuf {
    let home = match env_override.filter(|d| !d.is_empty()) {
        Some(d) => crate::config::expand_tilde(d),
        None => match std::env::var("GEMINI_CLI_HOME") {
            Ok(d) if !d.is_empty() => PathBuf::from(d),
            _ => dirs::home_dir().unwrap_or_default(),
        },
    };
    home.join(".gemini")
}

/// The chat folders that can hold a project directory's sessions.
fn chat_dirs(gemini: &Path, project_dir: &Path) -> Vec<PathBuf> {
    let key = project_dir.to_string_lossy().trim_end_matches('/').to_string();
    let mut out = vec![];
    let slug = std::fs::read(gemini.join("projects.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.pointer("/projects").and_then(|p| p.get(&key)).and_then(Value::as_str).map(str::to_string))
        .filter(|s| !s.is_empty() && !crate::util::os::path::has_separator(s) && s != "." && s != ".." && crate::util::os::path::check_component(s).is_ok());
    if let Some(s) = slug {
        out.push(gemini.join("tmp").join(s).join("chats"));
    }
    let hash = hex::encode(Sha256::digest(key.as_bytes()));
    out.push(gemini.join("tmp").join(hash).join("chats"));
    out
}

fn is_session_file(name: &str) -> bool {
    name.starts_with("session-") && (name.ends_with(".jsonl") || name.ends_with(".json"))
}

/// Whether Gemini has a session `id` for this directory (resuming one it does not have
/// fails, so a session that never started is started again under its id).
pub fn session_exists(gemini: &Path, project_dir: &Path, id: &str) -> bool {
    if !is_uuid(id) {
        return false;
    }
    let short = &id[..8];
    chat_dirs(gemini, project_dir).iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|rd| {
            rd.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                is_session_file(&name)
                    && (name.ends_with(&format!("-{short}.jsonl")) || name.ends_with(&format!("-{short}.json")))
                    && summarize(&e.path()).is_some_and(|s| s.id == id)
            })
        })
    })
}

/// What the history list shows of a session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub id: String,
    pub first_prompt: Option<String>,
    pub last_message: Option<String>,
    pub last_activity: i64,
    pub size: u64,
}

fn text_of(content: &Value) -> Option<String> {
    let t = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn millis(ts: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts?).ok().map(|d| d.timestamp_millis())
}

/// Read a session file: its head for the id and first prompt, its tail for the last
/// answer (bounded, whatever the file's size).
pub fn summarize(path: &Path) -> Option<Summary> {
    const HEAD: u64 = 512 * 1024;
    const TAIL: u64 = 512 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let md = f.metadata().ok()?;
    let size = md.len();
    let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    let mut s = Summary { size, last_activity: mtime, ..Default::default() };
    let name = path.file_name()?.to_string_lossy().into_owned();
    if name.ends_with(".json") {
        // One object; read it only when it is of a reasonable size.
        if size > 8 * 1024 * 1024 {
            return None;
        }
        let v: Value = serde_json::from_reader(std::io::BufReader::new(f)).ok()?;
        s.id = v.get("sessionId").and_then(Value::as_str).filter(|i| is_uuid(i))?.to_string();
        let msgs = v.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
        s.first_prompt = msgs.iter().find(|m| m.get("type").and_then(Value::as_str) == Some("user")).and_then(|m| text_of(m.get("content")?));
        s.last_message = msgs.iter().rev().find(|m| m.get("type").and_then(Value::as_str) == Some("gemini")).and_then(|m| text_of(m.get("content")?));
    } else {
        let mut head = String::new();
        (&mut f).take(HEAD).read_to_string(&mut head).ok();
        let mut lines = head.lines();
        let meta: Value = serde_json::from_str(lines.next()?).ok()?;
        s.id = meta.get("sessionId").and_then(Value::as_str).filter(|i| is_uuid(i))?.to_string();
        for l in lines {
            let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
            if v.get("type").and_then(Value::as_str) == Some("user") {
                if let Some(t) = v.get("content").and_then(text_of) {
                    if !t.starts_with('/') {
                        s.first_prompt = Some(t);
                        break;
                    }
                }
            }
        }
        let start = size.saturating_sub(TAIL);
        if f.seek(SeekFrom::Start(start)).is_ok() {
            let mut r = std::io::BufReader::new(f);
            let mut line = String::new();
            let mut first = start > 0;
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // The first line of a tail read may be cut.
                if std::mem::take(&mut first) {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(line.trim_end()) else { continue };
                if v.get("type").and_then(Value::as_str) == Some("gemini") {
                    if let Some(t) = v.get("content").and_then(text_of) {
                        s.last_message = Some(t);
                    }
                }
                if let Some(t) = millis(v.pointer("/$set/lastUpdated").and_then(Value::as_str)) {
                    s.last_activity = s.last_activity.max(t);
                }
            }
        }
    }
    s.first_prompt = s.first_prompt.map(|p| clean_line(&p, 200));
    s.last_message = s.last_message.map(|m| tail_chars(&m, 300));
    Some(s)
}

/// The directory's sessions, newest first (blocking).
pub fn list(gemini: &Path, project_dir: &Path, limit: usize) -> Vec<Summary> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = vec![];
    for dir in chat_dirs(gemini, project_dir) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_session_file(&name) {
                continue;
            }
            let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            files.push((mtime, e.path()));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<Summary> = vec![];
    for (_, p) in files.into_iter().take(limit.saturating_mul(2).max(limit)) {
        if let Some(s) = summarize(&p) {
            // A `.json` migrated to `.jsonl` may exist twice: keep the newest.
            if !out.iter().any(|o| o.id == s.id) {
                out.push(s);
            }
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0f0e0d0c-1111-4222-8333-444455556666";

    fn fixture(root: &Path, project: &Path) -> PathBuf {
        let g = root.join(".gemini");
        let chats = g.join("tmp/proj-a/chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(g.join("projects.json"), format!(r#"{{"projects":{{"{}":"proj-a"}}}}"#, project.display())).unwrap();
        let body = [
            format!(r#"{{"sessionId":"{ID}","projectHash":"x","startTime":"2026-09-27T10:00:00.000Z","lastUpdated":"2026-09-27T10:00:00.000Z","kind":"main"}}"#),
            r#"{"id":"m0","timestamp":"2026-09-27T10:00:01.000Z","type":"user","content":"/model"}"#.to_string(),
            r#"{"id":"m1","timestamp":"2026-09-27T10:00:02.000Z","type":"user","content":[{"text":"Fix the failing test"}]}"#.to_string(),
            r#"{"id":"m2","timestamp":"2026-09-27T10:00:05.000Z","type":"gemini","content":"Fixed: the fixture was stale."}"#.to_string(),
            r#"{"$set":{"lastUpdated":"2026-09-27T10:00:06.000Z"}}"#.to_string(),
            "not json".to_string(),
        ]
        .join("\n");
        std::fs::write(chats.join(format!("session-2026-09-27T10-00-{}.jsonl", &ID[..8])), body).unwrap();
        g
    }

    #[test]
    fn sessions_are_found_and_summarized() {
        let home = tempfile::tempdir().unwrap();
        let project = PathBuf::from("/w/project");
        let g = fixture(home.path(), &project);
        assert!(session_exists(&g, &project, ID));
        assert!(!session_exists(&g, &project, "0f0e0d0c-1111-4222-8333-000000000000"), "same short id, other session");
        assert!(!session_exists(&g, Path::new("/w/other"), ID));
        let rows = list(&g, &project, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, ID);
        assert_eq!(rows[0].first_prompt.as_deref(), Some("Fix the failing test"));
        assert_eq!(rows[0].last_message.as_deref(), Some("Fixed: the fixture was stale."));
        // The legacy hash folder and `.json` files are read too.
        let legacy = g.join("tmp").join(hex::encode(Sha256::digest(b"/w/old"))).join("chats");
        std::fs::create_dir_all(&legacy).unwrap();
        let id2 = "1f0e0d0c-1111-4222-8333-444455556666";
        std::fs::write(
            legacy.join(format!("session-2026-01-01T00-00-{}.json", &id2[..8])),
            format!(r#"{{"sessionId":"{id2}","messages":[{{"type":"user","content":"hello"}},{{"type":"gemini","content":[{{"text":"hi there"}}]}}]}}"#),
        )
        .unwrap();
        let old = list(&g, Path::new("/w/old"), 10);
        assert_eq!((old[0].id.as_str(), old[0].first_prompt.as_deref(), old[0].last_message.as_deref()), (id2, Some("hello"), Some("hi there")));
        assert!(session_exists(&g, Path::new("/w/old"), id2));
        assert_eq!(gemini_dir(Some("/opt/g")), PathBuf::from("/opt/g/.gemini"));
    }
}

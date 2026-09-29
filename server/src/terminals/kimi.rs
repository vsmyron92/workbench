//! Kimi Code CLI files on disk, read-only (verified against `@moonshot-ai/kimi-code`
//! 2.1.1: its bundled source and published data-location docs):
//! * `$KIMI_CODE_HOME` (default `~/.kimi-code`) holds `session_index.jsonl`: one JSON
//!   record per line, `{"sessionId","sessionDir","workDir"}` appended when a session is
//!   created or forked, `{"sessionId","deleted":true}` when one is deleted. The file is
//!   compacted (rewritten) now and then.
//! * Session ids look like `session_<uuid>`.
//! * `<sessionDir>/state.json` has the title, `lastPrompt` and timestamps.
//!
//! Kimi offers no hook Workbench can install per session and no machine-readable turn
//! log it documents, so a hosted Kimi session's state is the output-activity heuristic
//! (`activity`). Its session id comes from the index: a record for the session's cwd
//! that appeared after its launch, when the evidence gives it to exactly this session
//! (`assign`), or the id printed on its own screen.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use serde_json::Value;

use super::transcript::clean_line;

/// `$KIMI_CODE_HOME` (the session environment's value wins) or `~/.kimi-code`.
pub fn kimi_home(env_override: Option<&str>) -> PathBuf {
    if let Some(d) = env_override.filter(|d| !d.is_empty()) {
        return crate::config::expand_tilde(d);
    }
    match std::env::var("KIMI_CODE_HOME") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::home_dir().unwrap_or_default().join(".kimi-code"),
    }
}

pub const INDEX: &str = "session_index.jsonl";

/// One live index record.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    pub session_id: String,
    pub session_dir: PathBuf,
    pub work_dir: String,
}

/// What one index line says.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexLine {
    Added(IndexEntry),
    Deleted(String),
}

pub fn valid_id(s: &str) -> bool {
    super::providers::valid_session_id(super::ProviderKind::Kimi, s)
}

pub fn parse_index_line(line: &[u8]) -> Option<IndexLine> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let id = v.get("sessionId")?.as_str().filter(|s| valid_id(s))?.to_string();
    if v.get("deleted").and_then(Value::as_bool) == Some(true) {
        return Some(IndexLine::Deleted(id));
    }
    Some(IndexLine::Added(IndexEntry {
        session_id: id,
        session_dir: PathBuf::from(v.get("sessionDir")?.as_str()?),
        work_dir: v.get("workDir")?.as_str()?.to_string(),
    }))
}

/// The live sessions of an index, in index order (later records win).
pub fn read_index(text: &[u8]) -> Vec<IndexEntry> {
    let mut order: Vec<String> = vec![];
    let mut live: HashMap<String, IndexEntry> = HashMap::new();
    for line in text.split(|&b| b == b'\n') {
        match parse_index_line(line) {
            Some(IndexLine::Added(e)) => {
                order.retain(|x| *x != e.session_id);
                order.push(e.session_id.clone());
                live.insert(e.session_id.clone(), e);
            }
            Some(IndexLine::Deleted(id)) => {
                live.remove(&id);
            }
            None => {}
        }
    }
    order.into_iter().filter_map(|id| live.remove(&id)).collect()
}

/// Largest index read (a hostile or corrupt file is not slurped).
const INDEX_MAX: u64 = 16 * 1024 * 1024;

pub fn load_index(home: &Path) -> Vec<IndexEntry> {
    let path = home.join(INDEX);
    match std::fs::metadata(&path) {
        Ok(m) if m.len() <= INDEX_MAX => std::fs::read(&path).map(|b| read_index(&b)).unwrap_or_default(),
        _ => vec![],
    }
}

/// A hosted Kimi session waiting for its id (all of one cwd and Kimi home).
#[derive(Debug, Clone)]
pub struct Waiting {
    pub terminal_id: String,
    /// Index ids that existed when it was launched.
    pub known: Arc<HashSet<String>>,
    /// A fresh id its own screen shows (`id_on_screen`), if any.
    pub on_screen: Option<String>,
}

/// How an id was assigned.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// The session's own screen shows it.
    Screen,
    /// It is the only new entry the session can have created and no other waiting session
    /// could have created it. Sound only if nothing outside the hosted sessions creates
    /// entries there (the caller checks for Kimi processes in that folder).
    Elimination,
}

/// Which waiting sessions created which new index entries of `cwd`, as far as the
/// evidence goes. An entry is possible for a session when it was not in the index at the
/// session's launch and nobody claims it. A session whose screen shows one of its
/// possible entries has it; otherwise a session is assigned the entry that is its only
/// possibility when it is no other waiting session's only possibility, and assignments
/// repeat until nothing changes. So sessions launched one after the other resolve (each
/// knew the entries of those before), and truly simultaneous launches never guess.
pub fn assign(cwd: &str, entries: &[IndexEntry], waiting: &[Waiting], claimed: &HashSet<String>) -> HashMap<String, (String, Evidence)> {
    let fresh: Vec<&str> = entries
        .iter()
        .filter(|e| crate::util::os::path::same_dir(&e.work_dir, cwd) && !claimed.contains(&e.session_id))
        .map(|e| e.session_id.as_str())
        .collect();
    let mut possible: HashMap<&str, Vec<&str>> =
        waiting.iter().map(|w| (w.terminal_id.as_str(), fresh.iter().copied().filter(|id| !w.known.contains(*id)).collect())).collect();
    let mut out: HashMap<String, (String, Evidence)> = HashMap::new();
    let take =|out: &mut HashMap<String, (String, Evidence)>, possible: &mut HashMap<&str, Vec<&str>>, t: &str, id: String, how: Evidence| {
        possible.remove(t);
        for p in possible.values_mut() {
            p.retain(|x| *x != id);
        }
        out.insert(t.to_string(), (id, how));
    };
    for w in waiting {
        if let Some(id) = w.on_screen.as_deref().filter(|id| possible.get(w.terminal_id.as_str()).is_some_and(|p| p.contains(id))) {
            take(&mut out, &mut possible, &w.terminal_id, id.to_string(), Evidence::Screen);
        }
    }
    loop {
        let next = waiting.iter().find_map(|w| match possible.get(w.terminal_id.as_str())?.as_slice() {
            [only] if !possible.iter().any(|(o, q)| *o != w.terminal_id && q.as_slice() == [*only]) => Some((w.terminal_id.clone(), only.to_string())),
            _ => None,
        });
        let Some((t, id)) = next else { break };
        take(&mut out, &mut possible, &t, id, Evidence::Elimination);
    }
    out
}

static SCREEN_ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\bsession_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap());

/// A Kimi session id shown on the session's own screen (its status line or exit note),
/// if exactly one distinct id is visible.
pub fn id_on_screen(text: &str) -> Option<String> {
    let mut found: Vec<&str> = SCREEN_ID.find_iter(text).map(|m| m.as_str()).collect();
    found.dedup();
    found.sort_unstable();
    found.dedup();
    match found.as_slice() {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------- history

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub id: String,
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    pub updated_at: Option<i64>,
}

fn ts(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().map(|x| if x < 10_000_000_000 { x * 1000 } else { x }),
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis()),
        _ => None,
    }
}

/// `state.json` of a session (blocking). Only read inside `home/sessions`.
pub fn summarize(home: &Path, e: &IndexEntry) -> Summary {
    let mut s = Summary { id: e.session_id.clone(), ..Default::default() };
    let sessions = home.join("sessions");
    let Ok(dir) = crate::util::os::path::canonicalize(&e.session_dir) else { return s };
    let inside = crate::util::os::path::canonicalize(&sessions).is_ok_and(|root| crate::util::os::path::starts_with(&dir, &root));
    if !inside {
        return s;
    }
    let file = dir.join("state.json");
    let Ok(md) = std::fs::metadata(&file) else { return s };
    if md.len() > 4 * 1024 * 1024 {
        return s;
    }
    let Some(v) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return s };
    let str_at = |k: &str| v.get(k).and_then(Value::as_str).map(|t| clean_line(t, 140)).filter(|t| !t.is_empty());
    s.title = str_at("title");
    s.last_prompt = str_at("lastPrompt");
    s.updated_at = ts(v.get("updatedAt")).or_else(|| ts(v.get("createdAt")));
    if s.updated_at.is_none() {
        s.updated_at = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
    }
    s
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const S1: &str = "session_0f0e0d0c-0b0a-4000-8000-000000000001";
    const S2: &str = "session_0f0e0d0c-0b0a-4000-8000-000000000002";

    fn line(id: &str, dir: &str, wd: &str) -> String {
        format!(r#"{{"sessionId":"{id}","sessionDir":"{dir}","workDir":"{wd}"}}"#)
    }

    #[test]
    fn index_records_and_deletions() {
        let text = [
            line(S1, "/h/sessions/wd_p_1/s1", "/w/p"),
            "garbage".into(),
            line(S2, "/h/sessions/wd_p_1/s2", "/w/p"),
            format!(r#"{{"sessionId":"{S1}","deleted":true}}"#),
            r#"{"sessionId":"../../x","sessionDir":"/","workDir":"/"}"#.into(),
        ]
        .join("\n");
        let live = read_index(text.as_bytes());
        assert_eq!(live.iter().map(|e| e.session_id.as_str()).collect::<Vec<_>>(), [S2]);
        assert_eq!(live[0].work_dir, "/w/p");
    }

    const S0: &str = "session_0f0e0d0c-0b0a-4000-8000-000000000000";
    const S3: &str = "session_0f0e0d0c-0b0a-4000-8000-000000000003";

    fn waiting(t: &str, known: &[&str]) -> Waiting {
        Waiting { terminal_id: t.into(), known: Arc::new(known.iter().map(|s| s.to_string()).collect()), on_screen: None }
    }

    fn entries(ids: &[&str]) -> Vec<IndexEntry> {
        ids.iter().map(|id| IndexEntry { session_id: id.to_string(), session_dir: format!("/h/{id}").into(), work_dir: "/w/p/".into() }).collect()
    }

    fn ids(m: &HashMap<String, (String, Evidence)>) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = m.iter().map(|(t, (id, _))| (t.clone(), id.clone())).collect();
        v.sort();
        v
    }

    #[test]
    fn new_sessions_are_found_only_when_unambiguous() {
        let none = HashSet::new();
        let pair = |t: &str, id: &str| (t.to_string(), id.to_string());
        // Alone: the one new entry of its folder.
        let found = assign("/w/p", &entries(&[S0, S1]), &[waiting("a", &[S0])], &none);
        assert_eq!(found.get("a"), Some(&(S1.to_string(), Evidence::Elimination)));
        // Two new ones, other folders, claimed ids: no guess.
        assert!(assign("/w/p", &entries(&[S1, S2]), &[waiting("a", &[])], &none).is_empty());
        assert!(assign("/w/q", &entries(&[S1]), &[waiting("a", &[])], &none).is_empty());
        assert!(assign("/w/p", &entries(&[S1]), &[waiting("a", &[])], &[S1.to_string()].into()).is_empty());

        // Back to back: b was launched after a's entry existed. Both resolve, whichever
        // entry appeared first, even while both wait.
        let (a, b) = (waiting("a", &[S0]), waiting("b", &[S0, S1]));
        assert_eq!(ids(&assign("/w/p", &entries(&[S0, S1]), &[a.clone(), b.clone()], &none)), [pair("a", S1)]);
        assert_eq!(ids(&assign("/w/p", &entries(&[S0, S1, S2]), &[a.clone(), b.clone()], &none)), [pair("a", S1), pair("b", S2)]);
        // Launched together (neither knew the other's entry): never guessed…
        let (a, b) = (waiting("a", &[S0]), waiting("b", &[S0]));
        assert!(assign("/w/p", &entries(&[S0, S1]), &[a.clone(), b.clone()], &none).is_empty());
        assert!(assign("/w/p", &entries(&[S0, S1, S2]), &[a.clone(), b.clone()], &none).is_empty());
        // …unless a screen shows an id, which settles the other one too.
        let a_sees = Waiting { on_screen: Some(S2.into()), ..a.clone() };
        let found = assign("/w/p", &entries(&[S0, S1, S2]), &[a_sees, b.clone()], &none);
        assert_eq!(found.get("a"), Some(&(S2.to_string(), Evidence::Screen)));
        assert_eq!(found.get("b"), Some(&(S1.to_string(), Evidence::Elimination)));
        // An id on screen that is not a new entry proves nothing.
        let stale = Waiting { on_screen: Some(S0.into()), ..a.clone() };
        assert!(assign("/w/p", &entries(&[S0, S1, S2]), &[stale, b], &none).is_empty());
        // Three in a row.
        let w = [waiting("a", &[]), waiting("b", &[S1]), waiting("c", &[S1, S2])];
        assert_eq!(ids(&assign("/w/p", &entries(&[S1, S2, S3]), &w, &none)), [pair("a", S1), pair("b", S2), pair("c", S3)]);
    }

    #[test]
    fn ids_on_screen() {
        assert_eq!(id_on_screen(&format!("… {S1} · kimi-k2 · 12% ctx")), Some(S1.to_string()));
        assert_eq!(id_on_screen(&format!("{S1}\n{S1}")), Some(S1.to_string()));
        assert_eq!(id_on_screen(&format!("{S1} {S2}")), None);
        assert_eq!(id_on_screen("no id here"), None);
    }

    #[test]
    fn state_json_summaries_stay_inside_the_home() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/wd_p_abc/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.json"), r#"{"title":"Fix\nlogin","lastPrompt":"fix the login","updatedAt":"2026-09-26T10:00:00Z"}"#).unwrap();
        let e = IndexEntry { session_id: S1.into(), session_dir: dir.clone(), work_dir: "/w/p".into() };
        let s = summarize(home.path(), &e);
        assert_eq!((s.title.as_deref(), s.last_prompt.as_deref(), s.updated_at), (Some("Fix login"), Some("fix the login"), Some(1_790_416_800_000)));
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("state.json"), r#"{"title":"secret"}"#).unwrap();
        let e = IndexEntry { session_id: S2.into(), session_dir: outside.path().to_path_buf(), work_dir: "/w/p".into() };
        assert!(summarize(home.path(), &e).title.is_none());
    }
}

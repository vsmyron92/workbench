//! Breakpoints, watches and exception filters of a project, persisted in
//! `data_dir/debug/<project>.json` (0600). Line breakpoints are keyed by the
//! project-relative path; every live session of the project gets them with
//! `setBreakpoints`, one request per source file.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ApiError;

pub const MAX_BREAKPOINTS: usize = 2000;
pub const MAX_PER_FILE: usize = 300;
pub const MAX_FUNCTIONS: usize = 200;
pub const MAX_WATCHES: usize = 100;
const MAX_TEXT: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LineBreakpoint {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub path: String,
    pub line: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// DAP `hitCondition` (`5`, `>= 10`, `% 2`, as the adapter understands it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    /// A logpoint: log this message (`{expr}` interpolated by the adapter) instead of stopping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FunctionBreakpoint {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectDebug {
    pub version: u32,
    pub breakpoints: Vec<LineBreakpoint>,
    pub function_breakpoints: Vec<FunctionBreakpoint>,
    /// Enabled exception filter ids per adapter id (absent: the adapter's defaults).
    pub exception_filters: BTreeMap<String, Vec<String>>,
    pub watches: Vec<String>,
    /// "Mute breakpoints": sessions get none while set.
    pub muted: bool,
    /// The launch configuration last started, for the Debug button.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_config: Option<String>,
}

fn clean_text(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(|mut s| {
        if s.len() > MAX_TEXT {
            let mut cut = MAX_TEXT;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            s.truncate(cut);
        }
        s
    })
}

/// A project-relative path as breakpoints store it: no leading `./` or `/`, no `..`,
/// no empty or `.` components, no NUL.
pub fn normalize_path(p: &str) -> Result<String, ApiError> {
    let p = p.trim();
    if p.is_empty() || p.contains('\0') || p.starts_with('/') {
        return Err(ApiError::bad_request("breakpoints take a project-relative path"));
    }
    let mut parts = vec![];
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => return Err(ApiError::bad_request("breakpoint paths cannot leave the project")),
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        return Err(ApiError::bad_request("empty path"));
    }
    Ok(parts.join("/"))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

impl ProjectDebug {
    /// Replace the breakpoints of one file (sorted by line, one per line).
    pub fn set_file(&mut self, path: &str, list: Vec<LineBreakpoint>) -> Result<(), ApiError> {
        let path = normalize_path(path)?;
        let mut by_line: BTreeMap<u32, LineBreakpoint> = BTreeMap::new();
        for mut b in list {
            if b.line == 0 || b.line > 10_000_000 {
                return Err(ApiError::bad_request(format!("invalid line {}", b.line)));
            }
            if b.id.is_empty() || b.id.len() > 40 || !b.id.chars().all(|c| c.is_ascii_alphanumeric()) {
                b.id = new_id();
            }
            b.path = path.clone();
            b.condition = clean_text(b.condition);
            b.hit_condition = clean_text(b.hit_condition);
            b.log_message = clean_text(b.log_message);
            by_line.insert(b.line, b);
        }
        if by_line.len() > MAX_PER_FILE {
            return Err(ApiError::bad_request(format!("at most {MAX_PER_FILE} breakpoints per file")));
        }
        let others = self.breakpoints.iter().filter(|b| b.path != path).count();
        if others + by_line.len() > MAX_BREAKPOINTS {
            return Err(ApiError::bad_request(format!("at most {MAX_BREAKPOINTS} breakpoints per project")));
        }
        // Keep ids unique across the project (a client may copy one).
        let taken: std::collections::HashSet<String> = self.breakpoints.iter().filter(|b| b.path != path).map(|b| b.id.clone()).collect();
        self.breakpoints.retain(|b| b.path != path);
        for mut b in by_line.into_values() {
            if taken.contains(&b.id) {
                b.id = new_id();
            }
            self.breakpoints.push(b);
        }
        Ok(())
    }

    pub fn set_functions(&mut self, list: Vec<FunctionBreakpoint>) -> Result<(), ApiError> {
        if list.len() > MAX_FUNCTIONS {
            return Err(ApiError::bad_request(format!("at most {MAX_FUNCTIONS} function breakpoints")));
        }
        let mut out: Vec<FunctionBreakpoint> = vec![];
        for mut f in list {
            f.name = f.name.trim().to_string();
            if f.name.is_empty() || f.name.len() > 500 || f.name.contains('\0') {
                return Err(ApiError::bad_request("a function breakpoint needs a function name"));
            }
            if f.id.is_empty() || f.id.len() > 40 || !f.id.chars().all(|c| c.is_ascii_alphanumeric()) || out.iter().any(|o| o.id == f.id) {
                f.id = new_id();
            }
            f.condition = clean_text(f.condition);
            if !out.iter().any(|o| o.name == f.name) {
                out.push(f);
            }
        }
        self.function_breakpoints = out;
        Ok(())
    }

    pub fn set_watches(&mut self, list: Vec<String>) -> Result<(), ApiError> {
        let list: Vec<String> = list.into_iter().map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect();
        if list.len() > MAX_WATCHES || list.iter().any(|w| w.len() > MAX_TEXT || w.contains('\0')) {
            return Err(ApiError::bad_request(format!("at most {MAX_WATCHES} watches of up to {MAX_TEXT} characters")));
        }
        self.watches = list;
        Ok(())
    }

    /// Files with breakpoints, each with its enabled breakpoints (in line order).
    pub fn by_file(&self) -> BTreeMap<String, Vec<&LineBreakpoint>> {
        let mut m: BTreeMap<String, Vec<&LineBreakpoint>> = BTreeMap::new();
        for b in &self.breakpoints {
            let e = m.entry(b.path.clone()).or_default();
            if b.enabled {
                e.push(b);
            }
        }
        m
    }
}

/// DAP `SourceBreakpoint`s for `list`, within the adapter's capabilities. A logpoint
/// the adapter cannot do is left out (it would stop instead of logging); so is a
/// conditional breakpoint the adapter cannot condition. The second value names the
/// ids left out, with why.
pub fn to_dap(list: &[&LineBreakpoint], caps: &Value) -> (Vec<(String, Value)>, Vec<(String, String)>) {
    let cap = |k: &str| caps.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut out = vec![];
    let mut skipped = vec![];
    for b in list {
        let mut v = json!({ "line": b.line });
        if let Some(c) = &b.condition {
            if !cap("supportsConditionalBreakpoints") {
                skipped.push((b.id.clone(), "this debugger does not support conditional breakpoints".to_string()));
                continue;
            }
            v["condition"] = json!(c);
        }
        if let Some(h) = &b.hit_condition {
            if !cap("supportsHitConditionalBreakpoints") {
                skipped.push((b.id.clone(), "this debugger does not support hit counts".to_string()));
                continue;
            }
            v["hitCondition"] = json!(h);
        }
        if let Some(m) = &b.log_message {
            if !cap("supportsLogPoints") {
                skipped.push((b.id.clone(), "this debugger does not support log points".to_string()));
                continue;
            }
            v["logMessage"] = json!(m);
        }
        out.push((b.id.clone(), v));
    }
    (out, skipped)
}

/// In-memory copies of the per-project files, written through on every change.
#[derive(Default)]
pub struct Store {
    cache: Mutex<HashMap<String, ProjectDebug>>,
    /// Serializes writes of one project (the file is replaced atomically).
    write_lock: tokio::sync::Mutex<()>,
}

pub fn file_of(data_dir: &Path, pid: &str) -> PathBuf {
    data_dir.join("debug").join(format!("{pid}.json"))
}

impl Store {
    pub fn get(&self, data_dir: &Path, pid: &str) -> ProjectDebug {
        if let Some(p) = self.cache.lock().get(pid) {
            return p.clone();
        }
        let loaded = match crate::util::fs::read_json::<ProjectDebug>(&file_of(data_dir, pid)) {
            Ok(Some(p)) => p,
            Ok(None) => ProjectDebug::default(),
            Err(e) => {
                // Keep the unreadable file (the next write replaces it); start empty.
                tracing::warn!("debug: {e:#}");
                ProjectDebug::default()
            }
        };
        self.cache.lock().entry(pid.to_string()).or_insert(loaded).clone()
    }

    /// Apply `f` and persist. Returns the new state.
    pub async fn update(
        &self,
        data_dir: &Path,
        pid: &str,
        f: impl FnOnce(&mut ProjectDebug) -> Result<(), ApiError>,
    ) -> Result<ProjectDebug, ApiError> {
        let _w = self.write_lock.lock().await;
        let mut cur = self.get(data_dir, pid);
        f(&mut cur)?;
        cur.version = 1;
        let path = file_of(data_dir, pid);
        let copy = cur.clone();
        tokio::task::spawn_blocking(move || crate::util::fs::write_json(&path, &copy))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .map_err(|e| ApiError::internal(format!("{e:#}")))?;
        self.cache.lock().insert(pid.to_string(), cur.clone());
        Ok(cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bp(line: u32) -> LineBreakpoint {
        LineBreakpoint { line, enabled: true, ..Default::default() }
    }

    #[tokio::test]
    async fn persisted_per_project_with_normalized_paths() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::default();
        let mut cond = bp(12);
        cond.condition = Some("  i == 3 ".into());
        cond.log_message = Some("   ".into());
        let mut off = bp(4);
        off.enabled = false;
        let p = store
            .update(d.path(), "app", |p| p.set_file("./src//main.c", vec![cond, off, bp(12)]))
            .await
            .unwrap();
        // One breakpoint per line (the last wins), sorted, ids assigned, text trimmed.
        assert_eq!(p.breakpoints.iter().map(|b| b.line).collect::<Vec<_>>(), vec![4, 12]);
        assert!(p.breakpoints.iter().all(|b| b.path == "src/main.c" && b.id.len() == 10));
        assert_eq!(p.breakpoints[1].condition, None, "the later duplicate replaced the first");
        store.update(d.path(), "app", |p| p.set_watches(vec!["total".into(), " ".into(), "p.name".into()])).await.unwrap();
        // A fresh store reads the file back.
        let again = Store::default().get(d.path(), "app");
        assert_eq!(again.breakpoints.len(), 2);
        assert_eq!(again.watches, vec!["total", "p.name"]);
        crate::util::os::perm::assert_mode(&file_of(d.path(), "app"), 0o600);
        // Only enabled ones are sent.
        let files = again.by_file();
        assert_eq!(files["src/main.c"].iter().map(|b| b.line).collect::<Vec<_>>(), vec![12]);
        // Paths never leave the project.
        assert!(normalize_path("../etc/passwd").is_err());
        assert!(normalize_path("/etc/passwd").is_err());
        assert!(normalize_path("a/../../b").is_err());
        assert_eq!(normalize_path("a/./b").unwrap(), "a/b");
        // Unknown projects start empty; a corrupt file does not break anything.
        std::fs::write(file_of(d.path(), "bad"), "{not json").unwrap();
        assert_eq!(Store::default().get(d.path(), "bad"), ProjectDebug::default());
    }

    #[test]
    fn dap_mapping_respects_capabilities() {
        let mut c = bp(10);
        c.id = "c".into();
        c.condition = Some("x > 1".into());
        let mut l = bp(11);
        l.id = "l".into();
        l.log_message = Some("x = {x}".into());
        let mut h = bp(12);
        h.id = "h".into();
        h.hit_condition = Some("3".into());
        let list = [&c, &l, &h];
        let all = json!({"supportsConditionalBreakpoints": true, "supportsLogPoints": true, "supportsHitConditionalBreakpoints": true});
        let (sent, skipped) = to_dap(&list, &all);
        assert!(skipped.is_empty());
        assert_eq!(sent[0].1, json!({"line": 10, "condition": "x > 1"}));
        assert_eq!(sent[1].1, json!({"line": 11, "logMessage": "x = {x}"}));
        assert_eq!(sent[2].1, json!({"line": 12, "hitCondition": "3"}));
        let (sent, skipped) = to_dap(&list, &json!({}));
        assert!(sent.is_empty());
        assert_eq!(skipped.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["c", "l", "h"]);
    }
}

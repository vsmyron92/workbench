//! Files that change on disk while no browser has them open (an agent editing, a
//! `git checkout`, a code generator) reach the servers as
//! `workspace/didChangeWatchedFiles`, filtered by the globs each server registered.
//! The source is the files slice's watcher (`fs.changed`, ignore-aware).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use super::uri;
use crate::app::AppState;

/// Change types of the protocol.
const CREATED: u8 = 1;
const CHANGED: u8 = 2;
const DELETED: u8 = 3;

pub async fn on_fs_changed(state: &AppState, pid: &str, data: &Value) {
    let Some(lsp) = state.lsp.get(pid) else { return };
    let servers: Vec<_> = lsp.ready_servers().into_iter().filter(|s| !s.watchers().is_empty()).collect();
    if servers.is_empty() || data["overflow"] == true {
        return;
    }
    let paths: Vec<String> = data["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).take(1000).map(str::to_string).collect();
    if paths.is_empty() {
        return;
    }
    let open: HashSet<String> = lsp.open_uris();
    let root = lsp.root.clone();
    let pid = pid.to_string();
    let changes = tokio::task::spawn_blocking(move || classify(&root, &pid, &paths, &open)).await.unwrap_or_default();
    if changes.is_empty() {
        return;
    }
    for s in servers {
        let watchers = s.watchers();
        let list: Vec<Value> = changes
            .iter()
            .filter(|(host, kind)| {
                watchers.iter().any(|w| w.kind & (1 << (kind - 1)) != 0 && crate::util::os::path::strip_prefix(host, &w.base).is_some_and(|rel| w.glob.is_match(rel)))
            })
            .filter_map(|(host, kind)| s.map.to_server(host).map(|p| json!({ "uri": uri::file_uri(&p), "type": kind })))
            .collect();
        if !list.is_empty() {
            s.notify("workspace/didChangeWatchedFiles", json!({ "changes": list }));
        }
    }
}

/// Host paths with their change type; documents a browser has open are skipped (the
/// buffer, not the disk, is what the server sees for them), and so are directories.
pub fn classify(root: &std::path::Path, pid: &str, paths: &[String], open: &HashSet<String>) -> Vec<(PathBuf, u8)> {
    let now = SystemTime::now();
    paths
        .iter()
        .filter_map(|rel| {
            let rel = uri::clean_rel(rel)?;
            if rel.is_empty() || open.contains(&uri::project_uri(pid, &rel)) {
                return None;
            }
            let host = root.join(&rel);
            match std::fs::symlink_metadata(&host) {
                Ok(m) if m.is_dir() => None,
                Ok(m) => {
                    let fresh = m.created().ok().and_then(|c| now.duration_since(c).ok()).is_some_and(|age| age < Duration::from_secs(5));
                    Some((host, if fresh { CREATED } else { CHANGED }))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some((host, DELETED)),
                Err(_) => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_classified_and_open_documents_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/new.rs"), "").unwrap();
        std::fs::write(root.join("src/open.rs"), "").unwrap();
        let open: HashSet<String> = [uri::project_uri("p", "src/open.rs")].into();
        let paths: Vec<String> = ["src/new.rs", "src/open.rs", "src/gone.rs", "src/sub", "../escape.rs"].iter().map(|s| s.to_string()).collect();
        let got = classify(root, "p", &paths, &open);
        let kinds: Vec<(String, u8)> = got.iter().map(|(p, k)| (p.strip_prefix(root).unwrap().display().to_string(), *k)).collect();
        // Birth times exist on most Linux filesystems; either way it is a create or a change.
        assert!(kinds.contains(&("src/new.rs".into(), CREATED)) || kinds.contains(&("src/new.rs".into(), CHANGED)), "{kinds:?}");
        assert!(kinds.contains(&("src/gone.rs".into(), DELETED)), "{kinds:?}");
        assert_eq!(kinds.len(), 2, "{kinds:?}");
    }
}

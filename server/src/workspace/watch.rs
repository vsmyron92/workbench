//! Live updates: agents and repositories change registries and card files behind our
//! back. One debounced watcher covers `data_dir/workspace` (recursively) and every
//! project's Mr. Mak `workspace/` directory, and turns changes into
//! `workspace.changed {scope, cardId?}` events.
//!
//! Directories are watched, not the registry files: an atomic save replaces the
//! file's inode, and a watch on the old inode goes quiet (the Mr. Mak lesson).

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use notify_debouncer_full::DebounceEventResult;
use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use parking_lot::Mutex;

use super::store;
use crate::app::AppState;
use crate::util::os::watch::{Debouncer as Deb, debouncer};

/// Cards named per batch before it degrades to one scope-wide event.
const MAX_CARDS_PER_BATCH: usize = 50;

#[derive(Default)]
pub struct Watcher {
    deb: Mutex<Option<Deb>>,
    /// `<project root>/workspace` → project id, for projects with a Mr. Mak registry.
    repos: Mutex<HashMap<PathBuf, String>>,
}

pub async fn start(state: &AppState) {
    // Adding recursive inotify watches walks directories: keep it off the async workers.
    let st = state.clone();
    if tokio::task::spawn_blocking(move || setup(&st)).await.is_err() {
        tracing::warn!("workspace: watcher setup failed");
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        let mut rx = st.events.subscribe();
        loop {
            let resync = match rx.recv().await {
                Ok(ev) if ev.kind == "projects.changed" => true,
                // A repository just got (or lost) its registry.
                Ok(ev) if ev.kind == "fs.changed" && mentions_registry(&ev.data) => {
                    if let Some(pid) = &ev.project_id {
                        super::emit_changed(&st, pid, Some(pid), None);
                    }
                    true
                }
                Ok(_) => false,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if resync {
                let s2 = st.clone();
                let _ = tokio::task::spawn_blocking(move || sync_repos(&s2)).await;
            }
        }
    });
}

/// Create the debounced watcher on `data_dir/workspace` and the repositories' registries.
fn setup(state: &AppState) {
    let root = store::root_dir(state);
    if let Err(e) = std::fs::create_dir_all(&root) {
        tracing::warn!("workspace: cannot create {}: {e}", root.display());
        return;
    }
    crate::util::fs::set_mode(&root, 0o700);
    store::migrate_legacy_dirs(state);
    let st = state.clone();
    let deb = debouncer(Duration::from_millis(300), move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let paths: Vec<PathBuf> = events.into_iter().filter(|e| is_change(&e.event.kind)).flat_map(|e| e.event.paths).collect();
        if !paths.is_empty() {
            dispatch(&st, &paths);
        }
    });
    let mut deb = match deb {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("workspace: no file watcher ({e}); cards refresh on reload only");
            return;
        }
    };
    if let Err(e) = deb.watch(&root, RecursiveMode::Recursive) {
        tracing::warn!("workspace: cannot watch {}: {e}", root.display());
    }
    *state.workspace.watcher.deb.lock() = Some(deb);
    sync_repos(state);
}

/// Reads (inotify open/access/close-without-write) are not changes: listing cards
/// reads the registry, and treating that as a change would refetch forever.
fn is_change(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

/// An `fs.changed` that may have added or removed a repository's registry: the file
/// itself, or its `workspace/` directory arriving or leaving whole (a checkout, a
/// clone, `cp -r`), which the files slice reports as the directory alone.
fn mentions_registry(data: &serde_json::Value) -> bool {
    data.get("paths").and_then(|p| p.as_array()).is_some_and(|a| a.iter().any(|p| matches!(p.as_str(), Some("workspace/workspace.json" | "workspace"))))
        || data.get("overflow").and_then(|o| o.as_bool()).unwrap_or(false)
}

/// Watch exactly the repositories that have a Mr. Mak registry.
fn sync_repos(state: &AppState) {
    let want: HashMap<PathBuf, String> = state
        .projects
        .list()
        .iter()
        .filter(|p| !store::is_projectless(&p.id))
        .map(|p| (p.root.join("workspace"), p.id.clone()))
        .filter(|(dir, _)| dir.join("workspace.json").is_file())
        .collect();
    let mut deb = state.workspace.watcher.deb.lock();
    let Some(deb) = deb.as_mut() else { return };
    let mut repos = state.workspace.watcher.repos.lock();
    for dir in repos.keys().filter(|d| !want.contains_key(*d)).cloned().collect::<Vec<_>>() {
        let _ = deb.unwatch(&dir);
        repos.remove(&dir);
    }
    for (dir, pid) in want {
        if repos.contains_key(&dir) {
            continue;
        }
        match deb.watch(&dir, RecursiveMode::Recursive) {
            Ok(()) => {
                repos.insert(dir, pid);
            }
            Err(e) => tracing::info!("workspace: cannot watch {}: {e}", dir.display()),
        }
    }
}

/// What a changed path means: `(registry dir, scope, first component under it)`.
fn classify<'a>(root: &'a Path, repos: &'a HashMap<PathBuf, String>, path: &'a Path) -> Option<(PathBuf, String, Option<String>, bool)> {
    let name_ok = |p: &Path| p.file_name().map(|n| n.to_string_lossy()).is_some_and(|n| !n.starts_with('.') || n.starts_with(".workspace.json"));
    if !name_ok(path) {
        return None;
    }
    let plain = |c: Component<'_>| match c {
        Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
        _ => None,
    };
    if let Ok(rest) = path.strip_prefix(root) {
        let mut comps = rest.components().filter_map(plain);
        let scope = comps.next()?;
        if scope.starts_with('.') {
            return None; // trash, backups
        }
        let first = comps.next();
        return Some((root.join(&scope), scope, first, false));
    }
    for (dir, pid) in repos {
        if let Ok(rest) = path.strip_prefix(dir) {
            let first = rest.components().filter_map(plain).next();
            return Some((dir.clone(), pid.clone(), first, true));
        }
    }
    None
}

fn dispatch(state: &AppState, paths: &[PathBuf]) {
    let root = store::root_dir(state);
    let repos = state.workspace.watcher.repos.lock().clone();
    // (scope, card) pairs; `None` card = the scope as a whole.
    let mut changed: BTreeSet<(String, Option<String>)> = BTreeSet::new();
    let mut folders: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    for p in paths {
        let Some((dir, scope, first, repo)) = classify(&root, &repos, p) else { continue };
        let first = match first {
            None => None,
            Some(f) if f == "workspace.json" || f.starts_with(".workspace.json") || f == store::SHARED_DIR => None,
            Some(f) => Some(f),
        };
        let card = first.and_then(|folder| {
            let map = folders.entry(dir.clone()).or_insert_with(|| folder_map(&dir.join("workspace.json")));
            map.get(&folder).map(|id| if repo { format!("{}{id}", store::REPO_PREFIX) } else { id.clone() })
        });
        changed.insert((scope, card));
    }
    let scopes: BTreeSet<&String> = changed.iter().map(|(s, _)| s).collect();
    for scope in scopes {
        let cards: Vec<&String> = changed.iter().filter(|(s, _)| s == scope).filter_map(|(_, c)| c.as_ref()).collect();
        let whole = changed.contains(&(scope.clone(), None)) || cards.len() > MAX_CARDS_PER_BATCH;
        let project = if store::is_projectless(scope) { None } else { Some(scope.as_str()) };
        if whole {
            super::emit_changed(state, scope, project, None);
        } else {
            for c in cards {
                super::emit_changed(state, scope, project, Some(c));
            }
        }
    }
}

/// Folder → card id for one registry (empty when it cannot be read).
fn folder_map(registry: &Path) -> HashMap<String, String> {
    let Ok(Some((_, doc))) = store::read_registry(registry) else { return HashMap::new() };
    super::model::Card::all_from(&doc).0.into_iter().map(|c| (c.folder, c.id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_paths_map_to_scopes_and_cards() {
        let root = PathBuf::from("/data/workspace");
        let mut repos = HashMap::new();
        repos.insert(PathBuf::from("/repo/workspace"), "shop".to_string());
        let c = |p: &str| classify(&root, &repos, Path::new(p)).map(|(_, s, f, r)| (s, f, r));
        assert_eq!(c("/data/workspace/home/workspace.json"), Some(("home".into(), Some("workspace.json".into()), false)));
        assert_eq!(c("/data/workspace/shop/2026_x/report.html"), Some(("shop".into(), Some("2026_x".into()), false)));
        assert_eq!(c("/repo/workspace/2026_y/img/a.png"), Some(("shop".into(), Some("2026_y".into()), true)));
        assert_eq!(c("/data/workspace/.trash/shop/x"), None);
        assert_eq!(c("/data/workspace/shop/2026_x/.upload-1.part"), None);
        assert!(c("/data/workspace/shop/.workspace.json.wb-tmp-abc").is_some());
        assert_eq!(c("/elsewhere/file"), None);
    }

    #[test]
    fn reads_are_not_changes() {
        use notify_debouncer_full::notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};
        assert!(!is_change(&EventKind::Access(AccessKind::Open(AccessMode::Any))));
        assert!(!is_change(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
        assert!(is_change(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_change(&EventKind::Create(CreateKind::File)));
    }

    #[test]
    fn a_workspace_folder_arriving_whole_is_a_registry_change() {
        let m = |v: serde_json::Value| mentions_registry(&v);
        assert!(m(serde_json::json!({ "paths": ["workspace/workspace.json"] })));
        assert!(m(serde_json::json!({ "paths": ["src/a.rs", "workspace"] })));
        assert!(m(serde_json::json!({ "paths": [], "overflow": true })));
        assert!(!m(serde_json::json!({ "paths": ["workspace/2026_x/a.md", "workspaces"] })));
    }
}

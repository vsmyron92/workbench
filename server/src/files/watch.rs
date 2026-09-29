//! One file watcher per project.
//!
//! A recursive inotify watch on a repository puts a watch on every directory,
//! including `target/` and `node_modules/` (3,486 watches for a mid-sized Rust and Node repository against a
//! 65,536 per-user limit). Instead we walk with the `ignore` crate (gitignore-aware)
//! and add one non-recursive watch per directory we would show, capped per project.
//! New directories get watches as they appear (and are then reported, since files
//! may have landed in them before the watch existed).
//!
//! Events are debounced (200 ms), deduplicated and capped (500 paths, then
//! `overflow: true`), and emitted as `fs.changed {paths}` (project-relative). Changes
//! to `.git/HEAD`, the index or refs emit `git.changed`. Changed files are handed to
//! Local History (`history::changed`).

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use notify_debouncer_full::notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;

use super::HARD_IGNORE;
use super::gitignore::IgnoreChecker;
use crate::app::AppState;
use crate::error::ApiResult;
use crate::projects::Project;

/// Directories watched per project before we stop adding (and log once).
const MAX_WATCHED_DIRS: usize = 8000;
/// Paths per `fs.changed` event before it degrades to `overflow: true`.
const MAX_EVENT_PATHS: usize = 500;
/// New directories queued for watching per batch.
const MAX_NEW_DIRS: usize = 1000;

type Deb = Debouncer<RecommendedWatcher, RecommendedCache>;

#[derive(Default)]
struct Pending {
    paths: BTreeSet<String>,
    overflow: bool,
    git: bool,
    /// Created or renamed-to paths that may be directories needing watches.
    new_dirs: Vec<PathBuf>,
    /// A directory went away or moved: prune our bookkeeping.
    prune: bool,
}

struct Inner {
    root: PathBuf,
    deb: Mutex<Option<Deb>>,
    dirs: Mutex<HashSet<PathBuf>>,
    pending: Mutex<Pending>,
    wake: tokio::sync::Notify,
    capped: AtomicBool,
    errors: AtomicUsize,
}

pub struct ProjectWatch {
    pub root: PathBuf,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProjectWatch {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(deb) = self.inner.deb.lock().take() {
            deb.stop_nonblocking();
        }
    }
}

/// Where git keeps HEAD/index (`gitdir`) and refs (`commondir`); both are the
/// project's `.git` except in linked worktrees.
#[derive(Debug, Clone)]
struct GitDirs {
    gitdir: PathBuf,
    commondir: PathBuf,
}

/// The git dirs of the repository containing `root`: like git's own discovery, the
/// nearest `.git` (directory, or a `gitdir:` file for worktrees and submodules) in
/// `root` or an ancestor, so a project rooted in a subdirectory of a repository
/// still hears about commits, checkouts and staging.
fn git_dirs(root: &Path) -> Option<GitDirs> {
    let (top, dotgit) = root.ancestors().find_map(|dir| {
        let dotgit = dir.join(".git");
        std::fs::metadata(&dotgit).is_ok().then(|| (dir.to_path_buf(), dotgit))
    })?;
    let gitdir = if dotgit.is_dir() {
        dotgit
    } else {
        let text = std::fs::read_to_string(&dotgit).ok()?;
        let rel = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let p = PathBuf::from(rel);
        // Relative to the directory holding the `.git` file.
        if p.is_absolute() { p } else { top.join(p) }
    };
    let gitdir = crate::util::os::path::canonicalize(&gitdir).unwrap_or(gitdir);
    let commondir = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(t) => {
            let p = PathBuf::from(t.trim());
            let p = if p.is_absolute() { p } else { gitdir.join(p) };
            crate::util::os::path::canonicalize(&p).unwrap_or(p)
        }
        Err(_) => gitdir.clone(),
    };
    Some(GitDirs { gitdir, commondir })
}

/// Git metadata files whose change means "HEAD, index or refs moved".
fn is_git_signal(rel_in_git: &Path) -> bool {
    let s = rel_in_git.to_string_lossy();
    if s.ends_with(".lock") {
        return false;
    }
    let first = rel_in_git.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    matches!(
        first.as_str(),
        "HEAD"
            | "index"
            | "packed-refs"
            | "ORIG_HEAD"
            | "MERGE_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "REBASE_HEAD"
            | "BISECT_LOG"
            | "rebase-merge"
            | "rebase-apply"
            | "sequencer"
            | "refs"
    )
}

#[derive(Debug, PartialEq)]
enum Class {
    Git,
    Noise,
    File(String),
}

fn classify(root: &Path, git: Option<&GitDirs>, path: &Path) -> Class {
    if let Some(g) = git {
        for dir in [&g.gitdir, &g.commondir] {
            if let Ok(rest) = path.strip_prefix(dir) {
                return if is_git_signal(rest) { Class::Git } else { Class::Noise };
            }
        }
    }
    let Ok(rel) = path.strip_prefix(root) else { return Class::Noise };
    let rel_s = rel.to_string_lossy().replace('\\', "/");
    if rel_s.is_empty()
        || rel.components().any(|c| c.as_os_str() == ".git")
        || rel_s.contains(".wb-tmp-")
    {
        return Class::Noise;
    }
    Class::File(rel_s)
}

fn hard_ignored(name: &str) -> bool {
    HARD_IGNORE.contains(&name)
}

/// Directories to watch under `start` (inclusive), honouring .gitignore.
fn walk_dirs(start: &Path) -> impl Iterator<Item = PathBuf> {
    ignore::WalkBuilder::new(start)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .parents(true)
        .follow_links(false)
        .filter_entry(|e| !hard_ignored(&e.file_name().to_string_lossy()))
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_dir()))
        .map(|e| e.into_path())
}

impl Inner {
    /// Watch `dir` and its (non-ignored) subdirectories. Blocking.
    fn add_tree(&self, dir: &Path, check_ignored: bool) {
        if check_ignored {
            let rel_ignored = dir
                .strip_prefix(&self.root)
                .map(|r| r.components().any(|c| hard_ignored(&c.as_os_str().to_string_lossy())))
                .unwrap_or(true);
            if rel_ignored {
                return;
            }
            // The walk does not filter its own starting point: check it explicitly,
            // or `cargo build` creating target/ would get thousands of watches.
            let parent = dir.parent().unwrap_or(&self.root);
            if IgnoreChecker::for_dir(&self.root, parent).is_ignored(dir, true) {
                return;
            }
        }
        for d in walk_dirs(dir) {
            let mut dirs = self.dirs.lock();
            if dirs.len() >= MAX_WATCHED_DIRS && !dirs.contains(&d) {
                if !self.capped.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "{}: more than {MAX_WATCHED_DIRS} directories; not watching the rest (add them to .gitignore?)",
                        self.root.display()
                    );
                }
                return;
            }
            // Re-adding an existing watch is cheap and refreshes notify's path for a
            // renamed directory.
            let mut deb = self.deb.lock();
            let Some(deb) = deb.as_mut() else { return };
            match deb.watch(&d, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    dirs.insert(d);
                }
                Err(e) => {
                    if self.errors.fetch_add(1, Ordering::Relaxed) < 5 {
                        tracing::warn!("cannot watch {}: {e}", d.display());
                    }
                }
            }
        }
    }

    fn add_git_watches(&self, g: &GitDirs) {
        let mut deb = self.deb.lock();
        let Some(deb) = deb.as_mut() else { return };
        let mut targets = vec![(g.gitdir.clone(), RecursiveMode::NonRecursive)];
        if g.commondir != g.gitdir {
            targets.push((g.commondir.clone(), RecursiveMode::NonRecursive));
        }
        targets.push((g.commondir.join("refs"), RecursiveMode::Recursive));
        for (p, mode) in targets {
            if p.is_dir() {
                if let Err(e) = deb.watch(&p, mode) {
                    tracing::warn!("cannot watch {}: {e}", p.display());
                }
            }
        }
    }
}

/// Start watching `project`. The initial walk runs on the blocking pool.
async fn start_watch(state: &AppState, project: &Project) -> anyhow::Result<ProjectWatch> {
    let root = project.root.clone();
    let git = git_dirs(&root);
    let inner = Arc::new(Inner {
        root: root.clone(),
        deb: Mutex::new(None),
        dirs: Mutex::new(HashSet::new()),
        pending: Mutex::new(Pending::default()),
        wake: tokio::sync::Notify::new(),
        capped: AtomicBool::new(false),
        errors: AtomicUsize::new(0),
    });
    let cb_inner = inner.clone();
    let cb_git = git.clone();
    let deb = new_debouncer(Duration::from_millis(200), None, move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let inner = &cb_inner;
        let mut any = false;
        let mut p = inner.pending.lock();
        for ev in events {
            if matches!(ev.kind, EventKind::Access(_)) {
                continue;
            }
            let maybe_dir = matches!(
                ev.kind,
                EventKind::Create(CreateKind::Folder | CreateKind::Any) | EventKind::Modify(ModifyKind::Name(_))
            );
            if matches!(ev.kind, EventKind::Remove(RemoveKind::Folder | RemoveKind::Any) | EventKind::Modify(ModifyKind::Name(_))) {
                p.prune = true;
            }
            for path in &ev.paths {
                match classify(&inner.root, cb_git.as_ref(), path) {
                    Class::Git => {
                        p.git = true;
                        any = true;
                    }
                    Class::Noise => {}
                    Class::File(rel) => {
                        any = true;
                        if p.paths.len() < MAX_EVENT_PATHS {
                            p.paths.insert(rel);
                        } else if !p.paths.contains(&rel) {
                            p.overflow = true;
                        }
                        if maybe_dir && p.new_dirs.len() < MAX_NEW_DIRS {
                            p.new_dirs.push(path.clone());
                        }
                    }
                }
            }
        }
        drop(p);
        if any {
            inner.wake.notify_one();
        }
    })?;
    *inner.deb.lock() = Some(deb);

    let setup = inner.clone();
    let git_setup = git.clone();
    tokio::task::spawn_blocking(move || {
        setup.add_tree(&setup.root, false);
        if let Some(g) = &git_setup {
            setup.add_git_watches(g);
        }
    })
    .await?;
    tracing::debug!("watching {} ({} dirs)", root.display(), inner.dirs.lock().len());

    let task = tokio::spawn(run(state.clone(), project.id.clone(), inner.clone()));
    Ok(ProjectWatch { root, inner, task })
}

/// Drains debounced batches: adds watches for new directories, then emits events.
async fn run(state: AppState, pid: String, inner: Arc<Inner>) {
    loop {
        inner.wake.notified().await;
        let batch = std::mem::take(&mut *inner.pending.lock());
        // Files may have landed in a new folder before its watch existed: Local
        // History looks inside (the tree only needs the folder).
        let created: Vec<String> = batch
            .new_dirs
            .iter()
            .filter_map(|d| d.strip_prefix(&inner.root).ok())
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .filter(|r| !r.is_empty())
            .collect();
        if !batch.new_dirs.is_empty() || batch.prune {
            let w = inner.clone();
            let new_dirs = batch.new_dirs;
            let prune = batch.prune;
            let _ = tokio::task::spawn_blocking(move || {
                if prune {
                    w.dirs.lock().retain(|d| d.is_dir());
                    if w.dirs.lock().len() < MAX_WATCHED_DIRS {
                        w.capped.store(false, Ordering::Relaxed);
                    }
                }
                for d in new_dirs {
                    if d.is_dir() {
                        w.add_tree(&d, true);
                    }
                }
            })
            .await;
        }
        if !batch.paths.is_empty() || batch.overflow {
            state.files.quick.invalidate(&pid);
            state
                .events
                .emit("fs.changed", Some(&pid), json!({ "paths": batch.paths, "overflow": batch.overflow }));
            // Local History snapshots what changed (not an overflowing batch: a
            // checkout of thousands of files is recorded by the VCS, not by us).
            if !batch.overflow {
                super::history::changed(&state, &pid, batch.paths.into_iter().collect(), created);
            }
        }
        if batch.git {
            state.events.emit("git.changed", Some(&pid), json!({}));
        }
    }
}

/// Reconcile watchers with the project list: stop removed ones, start new ones.
pub async fn sync_all(state: &AppState) {
    let _guard = state.files.sync_lock.lock().await;
    let projects = state.projects.list_with_scratches();
    {
        let mut w = state.files.watchers.lock();
        w.retain(|id, pw| projects.iter().any(|p| &p.id == id && p.root == pw.root));
    }
    for p in projects {
        if state.files.watchers.lock().contains_key(&p.id) {
            continue;
        }
        match start_watch(state, &p).await {
            Ok(pw) => {
                state.files.watchers.lock().insert(p.id.clone(), pw);
            }
            Err(e) => tracing::warn!("cannot watch project {}: {e:#}", p.id),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatus {
    pub watching: bool,
    pub dirs: usize,
    /// Hit the per-project cap: some directories are not watched.
    pub capped: bool,
    pub errors: usize,
}

/// `GET /api/projects/{pid}/files/watch` — watcher diagnostics.
pub async fn status(State(state): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<WatchStatus>> {
    state.projects.require(&pid)?;
    let w = state.files.watchers.lock();
    Ok(Json(match w.get(&pid) {
        Some(pw) => WatchStatus {
            watching: true,
            dirs: pw.inner.dirs.lock().len(),
            capped: pw.inner.capped.load(Ordering::Relaxed),
            errors: pw.inner.errors.load(Ordering::Relaxed),
        },
        None => WatchStatus { watching: false, dirs: 0, capped: false, errors: 0 },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_signals() {
        assert!(is_git_signal(Path::new("HEAD")));
        assert!(is_git_signal(Path::new("index")));
        assert!(is_git_signal(Path::new("refs/heads/main")));
        assert!(is_git_signal(Path::new("refs/remotes/origin/main")));
        assert!(is_git_signal(Path::new("packed-refs")));
        assert!(!is_git_signal(Path::new("index.lock")));
        assert!(!is_git_signal(Path::new("refs/heads/main.lock")));
        assert!(!is_git_signal(Path::new("FETCH_HEAD")));
        assert!(!is_git_signal(Path::new("objects/ab/cdef")));
        assert!(!is_git_signal(Path::new("logs/HEAD")));
    }

    #[test]
    fn classification() {
        let root = Path::new("/p");
        let g = GitDirs { gitdir: "/p/.git".into(), commondir: "/p/.git".into() };
        assert_eq!(classify(root, Some(&g), Path::new("/p/.git/HEAD")), Class::Git);
        assert_eq!(classify(root, Some(&g), Path::new("/p/.git/objects/x")), Class::Noise);
        assert_eq!(classify(root, Some(&g), Path::new("/p/src/main.rs")), Class::File("src/main.rs".into()));
        assert_eq!(classify(root, Some(&g), Path::new("/p/src/.main.rs.wb-tmp-abc")), Class::Noise);
        assert_eq!(classify(root, Some(&g), Path::new("/p/vendor/x/.git/HEAD")), Class::Noise);
        assert_eq!(classify(root, Some(&g), Path::new("/elsewhere/x")), Class::Noise);
        assert_eq!(classify(root, None, Path::new("/p")), Class::Noise);
    }

    #[test]
    fn walk_skips_ignored_and_hard_ignored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["src/a", "node_modules/x", "build/out", ".github/workflows", ".git/objects"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        let mut got: Vec<String> = walk_dirs(root)
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();
        got.sort();
        assert_eq!(got, vec!["", ".github", ".github/workflows", "src", "src/a"]);
    }

    #[test]
    fn worktree_git_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main/.git");
        let wt = main.join("worktrees/feature");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("commondir"), "../..\n").unwrap();
        let checkout = dir.path().join("feature");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join(".git"), format!("gitdir: {}\n", wt.display())).unwrap();
        let g = git_dirs(&checkout).unwrap();
        assert_eq!(g.gitdir, crate::util::os::path::canonicalize(&wt).unwrap());
        assert_eq!(g.commondir, crate::util::os::path::canonicalize(&main).unwrap());
    }

    #[test]
    fn subdirectory_projects_find_the_repository_git_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let repo = crate::util::os::path::canonicalize(dir.path()).unwrap().join("mono");
        std::fs::create_dir_all(repo.join(".git/refs/heads")).unwrap();
        std::fs::create_dir_all(repo.join("app/web/src")).unwrap();
        let g = git_dirs(&repo.join("app/web")).unwrap();
        assert_eq!((&g.gitdir, &g.commondir), (&repo.join(".git"), &repo.join(".git")));
        let root = repo.join("app/web");
        assert_eq!(classify(&root, Some(&g), &repo.join(".git/index")), Class::Git);
        assert_eq!(classify(&root, Some(&g), &repo.join(".git/refs/heads/main")), Class::Git);
        assert_eq!(classify(&root, Some(&g), &root.join("src/a.ts")), Class::File("src/a.ts".into()));
        assert_eq!(classify(&root, Some(&g), &repo.join("app/other.ts")), Class::Noise);

        // A submodule inside it: its `.git` file wins, relative to the submodule's top.
        let modules = repo.join(".git/modules/lib");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::create_dir_all(repo.join("lib/src")).unwrap();
        std::fs::write(repo.join("lib/.git"), "gitdir: ../.git/modules/lib\n").unwrap();
        let g = git_dirs(&repo.join("lib/src")).unwrap();
        assert_eq!(g.gitdir, modules);

        // Outside any repository.
        let plain = tempfile::tempdir().unwrap();
        if !plain.path().ancestors().any(|d| d.join(".git").exists()) {
            assert!(git_dirs(plain.path()).is_none());
        }
    }

    /// Through the real watcher and event bus: a project rooted in a repository
    /// subdirectory gets `git.changed` when the repository's index or HEAD moves.
    #[tokio::test(flavor = "multi_thread")]
    async fn subdirectory_project_emits_git_changed() {
        use crate::config::{GlobalConfig, Paths};
        let (cfg, data, tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let repo = crate::util::os::path::canonicalize(tmp.path()).unwrap().join("mono");
        std::fs::create_dir_all(repo.join(".git/refs/heads")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(repo.join("app/web")).unwrap();
        std::fs::write(repo.join("app/web/a.txt"), "a\n").unwrap();
        let mut config = GlobalConfig::default();
        config.projects.roots = vec![];
        config.projects.include = vec![repo.join("app/web").display().to_string()];
        config.notify.desktop = false;
        let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
        let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
        let pid = state.projects.list()[0].id.clone();
        let mut rx = state.events.subscribe();
        sync_all(&state).await;

        std::fs::write(repo.join(".git/index"), "staged").unwrap();
        std::fs::write(repo.join(".git/refs/heads/other"), "0123\n").unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let got = loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(ev)) if ev.kind == "git.changed" => break Some(ev.project_id.clone()),
                Ok(Ok(_)) => continue,
                _ => break None,
            }
        };
        assert_eq!(got, Some(Some(pid)));
    }

    /// End to end on a temp dir: a burst of writes becomes one event, new
    /// directories get watched, ignored ones stay silent, and .git/HEAD signals git.
    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_reports_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        let inner = Arc::new(Inner {
            root: root.clone(),
            deb: Mutex::new(None),
            dirs: Mutex::new(HashSet::new()),
            pending: Mutex::new(Pending::default()),
            wake: tokio::sync::Notify::new(),
            capped: AtomicBool::new(false),
            errors: AtomicUsize::new(0),
        });
        let git = git_dirs(&root);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Vec<String>, bool)>();
        let cb_inner = inner.clone();
        let cb_git = git.clone();
        let deb = new_debouncer(Duration::from_millis(100), None, move |res: DebounceEventResult| {
            let Ok(events) = res else { return };
            let mut paths = vec![];
            let mut gitc = false;
            for ev in events {
                if matches!(ev.kind, EventKind::Access(_)) {
                    continue;
                }
                for p in &ev.paths {
                    match classify(&cb_inner.root, cb_git.as_ref(), p) {
                        Class::Git => gitc = true,
                        Class::File(r) => paths.push(r),
                        Class::Noise => {}
                    }
                }
            }
            let _ = tx.send((paths, gitc));
        })
        .unwrap();
        *inner.deb.lock() = Some(deb);
        inner.add_tree(&root, false);
        inner.add_git_watches(git.as_ref().unwrap());

        async fn collect(rx: &mut tokio::sync::mpsc::UnboundedReceiver<(Vec<String>, bool)>) -> (BTreeSet<String>, bool) {
            let mut out = BTreeSet::new();
            let mut git = false;
            while let Ok(Some((p, g))) = tokio::time::timeout(Duration::from_millis(600), rx.recv()).await {
                out.extend(p);
                git |= g;
            }
            (out, git)
        }

        for i in 0..10 {
            std::fs::write(root.join("src/lib.rs"), format!("{i}")).unwrap();
        }
        std::fs::create_dir_all(root.join("build/out")).unwrap();
        let (paths, git) = collect(&mut rx).await;
        assert!(paths.contains("src/lib.rs"), "{paths:?}");
        assert!(paths.contains("build"), "{paths:?}");
        assert!(!git);

        // New dir: the ignored one gets no watch; the regular one does.
        inner.add_tree(&root.join("build"), true);
        std::fs::create_dir_all(root.join("src/new")).unwrap();
        let _ = collect(&mut rx).await;
        inner.add_tree(&root.join("src/new"), true);
        std::fs::write(root.join("build/out/x.o"), "x").unwrap();
        std::fs::write(root.join("src/new/inner.rs"), "x").unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();
        let (paths, git) = collect(&mut rx).await;
        assert!(paths.contains("src/new/inner.rs"), "{paths:?}");
        assert!(!paths.iter().any(|p| p.starts_with("build/")), "{paths:?}");
        assert!(git);
        assert!(!inner.dirs.lock().contains(&root.join("build")));
    }
}

//! One file watcher per project.
//!
//! A recursive inotify watch on a repository puts a watch on every directory,
//! including `target/` and `node_modules/` (3,486 watches for a mid-sized Rust and Node repository against a
//! 65,536 per-user limit). Instead we walk with the `ignore` crate (gitignore-aware)
//! and add one non-recursive watch per directory we would show, capped per project.
//! New directories get watches as they appear (and are then reported, since files
//! may have landed in them before the watch existed).
//!
//! Windows (`os::watch::RECURSIVE`): one recursive watch on the root instead, since an
//! open directory handle keeps the folders above it from being renamed. It sees ignored
//! folders too: the same walk keeps the folders a watch each would cover (`dirs`), and
//! changes elsewhere are dropped, so the events are the same. A watch that stops on an
//! error is made again.
//!
//! Events are debounced (200 ms), deduplicated and capped (500 paths, then
//! `overflow: true`, as when the watcher itself lost events), and emitted as
//! `fs.changed {paths}` (project-relative). Changes to `.git/HEAD`, the index or refs
//! emit `git.changed`. Changed files are handed to Local History (`history::changed`).

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use notify_debouncer_full::notify::{self, RecursiveMode};
use notify_debouncer_full::notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;

use super::HARD_IGNORE;
use super::gitignore::IgnoreChecker;
use crate::app::AppState;
use crate::error::ApiResult;
use crate::projects::Project;
use crate::util::os;

/// Directories watched per project before we stop adding (and log once).
const MAX_WATCHED_DIRS: usize = 8000;
/// Paths per `fs.changed` event before it degrades to `overflow: true`.
const MAX_EVENT_PATHS: usize = 500;
/// New directories queued for watching per batch.
const MAX_NEW_DIRS: usize = 1000;

type Deb = os::watch::Debouncer;

#[derive(Default)]
struct Pending {
    paths: BTreeSet<String>,
    overflow: bool,
    git: bool,
    /// Created or renamed-to paths that may be directories needing watches.
    new_dirs: Vec<PathBuf>,
    /// A directory went away or moved: prune our bookkeeping.
    prune: bool,
    /// A watch stopped on an error (`os::watch::RECURSIVE`): make the watches again.
    rewatch: bool,
}

struct Inner {
    root: PathBuf,
    git: Option<GitDirs>,
    deb: Mutex<Option<Deb>>,
    /// The folders watched, one watch each; with one recursive watch, those it reports.
    dirs: Mutex<HashSet<PathBuf>>,
    pending: Mutex<Pending>,
    wake: tokio::sync::Notify,
    capped: AtomicBool,
    errors: AtomicUsize,
    /// Times the watches were made again after an error (`Pending::rewatch`).
    rewatches: AtomicU32,
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
            if let Some(rest) = os::path::strip_prefix(path, dir) {
                return if is_git_signal(rest) { Class::Git } else { Class::Noise };
            }
        }
    }
    let Some(rel) = os::path::strip_prefix(path, root) else { return Class::Noise };
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

/// What changed, for `run`.
#[derive(Debug, Default)]
struct Batch {
    paths: BTreeSet<String>,
    overflow: bool,
    git: bool,
    /// Created or moved-in paths that may be folders (project-relative).
    created: Vec<String>,
}

impl Inner {
    fn new(root: PathBuf, git: Option<GitDirs>) -> Arc<Inner> {
        Arc::new(Inner {
            root,
            git,
            deb: Mutex::new(None),
            dirs: Mutex::new(HashSet::new()),
            pending: Mutex::new(Pending::default()),
            wake: tokio::sync::Notify::new(),
            capped: AtomicBool::new(false),
            errors: AtomicUsize::new(0),
            rewatches: AtomicU32::new(0),
        })
    }

    /// Create the debouncer (`debounce`: how long events settle); the watches come with
    /// `watch_all`.
    fn start(self: &Arc<Self>, debounce: Duration) -> anyhow::Result<()> {
        let inner = self.clone();
        let deb = os::watch::debouncer(debounce, move |res: DebounceEventResult| match res {
            Ok(events) => inner.on_events(events),
            Err(errors) if os::watch::RECURSIVE => inner.on_errors(errors),
            Err(_) => {}
        })?;
        *self.deb.lock() = Some(deb);
        Ok(())
    }

    /// A debounced batch: sort it into `pending` and wake `run`.
    fn on_events(&self, events: Vec<DebouncedEvent>) {
        let git = self.git.as_ref();
        let mut p = self.pending.lock();
        let mut any = false;
        for ev in events {
            if os::watch::RESCAN_IS_OVERFLOW && ev.need_rescan() {
                // The watcher lost events (its buffer overflowed): refresh everything, git
                // included (a `.git` inside the root has no watch of its own).
                p.overflow = true;
                p.git |= git.is_some();
                any = true;
                continue;
            }
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
                match classify(&self.root, git, path) {
                    Class::Git => {
                        p.git = true;
                        any = true;
                    }
                    Class::Noise => {}
                    // One recursive watch also sees ignored folders: keep what a watch per
                    // directory would have seen.
                    Class::File(_) if os::watch::RECURSIVE && !self.covers(path) => {}
                    // A folder "modified" by its entries: they are reported themselves.
                    Class::File(_) if os::watch::FOLDERS_MODIFY && ev.kind == EventKind::Modify(ModifyKind::Any) && path.is_dir() => {}
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
            self.wake.notify_one();
        }
    }

    /// Whether a change of `path` is one a watch per directory reports: its folder is
    /// among `dirs`.
    fn covers(&self, path: &Path) -> bool {
        path.parent().is_some_and(|d| self.dirs.lock().contains(d))
    }

    /// Watches that stopped on an error (`os::watch::RECURSIVE`: a watch's thread ended):
    /// counted, and made again by `next_batch`, everything refreshed since events were lost
    /// meanwhile. Nothing is watched again once the root is gone.
    fn on_errors(&self, errors: Vec<notify::Error>) {
        for e in &errors {
            if self.errors.fetch_add(1, Ordering::Relaxed) < 5 {
                tracing::warn!("{}: a watch stopped: {e}", self.root.display());
            }
        }
        if errors.is_empty() || !self.root.is_dir() {
            return;
        }
        let mut p = self.pending.lock();
        p.rewatch = true;
        p.overflow = true;
        p.git |= self.git.is_some();
        drop(p);
        self.wake.notify_one();
    }

    /// The first watches: the root's tree (one recursive watch, or one per directory) and
    /// the git dirs. Blocking.
    fn watch_all(&self) {
        if os::watch::RECURSIVE {
            self.watch_root();
        }
        self.add_tree(&self.root, false);
        if let Some(g) = &self.git {
            self.add_git_watches(g);
        }
    }

    /// One recursive watch on the root (`os::watch::RECURSIVE`). Blocking.
    fn watch_root(&self) {
        let mut deb = self.deb.lock();
        let Some(deb) = deb.as_mut() else { return };
        if let Err(e) = deb.watch(&self.root, RecursiveMode::Recursive) {
            self.errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("cannot watch {}: {e}", self.root.display());
        }
    }

    /// Wait for the next batch. New folders are added to `dirs` first (with watches per
    /// directory, they get theirs), and folders that went away are forgotten.
    async fn next_batch(self: &Arc<Self>) -> Batch {
        self.wake.notified().await;
        let batch = std::mem::take(&mut *self.pending.lock());
        if os::watch::RECURSIVE && batch.rewatch {
            // After a pause that doubles each time (1 s, up to 64 s), should it keep stopping.
            let n = self.rewatches.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(1 << n.min(6))).await;
            let w = self.clone();
            let _ = tokio::task::spawn_blocking(move || {
                w.watch_root();
                if let Some(g) = &w.git {
                    w.add_git_watches(g);
                }
            })
            .await;
        }
        // Files may have landed in a new folder before its watch existed: Local
        // History looks inside (the tree only needs the folder).
        let created: Vec<String> = batch
            .new_dirs
            .iter()
            .filter_map(|d| d.strip_prefix(&self.root).ok())
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .filter(|r| !r.is_empty())
            .collect();
        if !batch.new_dirs.is_empty() || batch.prune {
            let w = self.clone();
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
        Batch { paths: batch.paths, overflow: batch.overflow, git: batch.git, created }
    }

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
            if os::watch::RECURSIVE {
                // The root's watch sees it already.
                dirs.insert(d);
                continue;
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
            // The recursive watch on the root already sees a `.git` inside it.
            if os::watch::RECURSIVE && os::path::starts_with(&p, &self.root) {
                continue;
            }
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
    let inner = Inner::new(root.clone(), git_dirs(&root));
    inner.start(Duration::from_millis(200))?;
    let setup = inner.clone();
    tokio::task::spawn_blocking(move || setup.watch_all()).await?;
    tracing::debug!("watching {} ({} dirs)", root.display(), inner.dirs.lock().len());

    let task = tokio::spawn(run(state.clone(), project.id.clone(), inner.clone()));
    Ok(ProjectWatch { root, inner, task })
}

/// Drains debounced batches (`Inner::next_batch`) and emits events.
async fn run(state: AppState, pid: String, inner: Arc<Inner>) {
    loop {
        let batch = inner.next_batch().await;
        if !batch.paths.is_empty() || batch.overflow {
            state.files.quick.invalidate(&pid);
            state
                .events
                .emit("fs.changed", Some(&pid), json!({ "paths": batch.paths, "overflow": batch.overflow }));
            // Local History snapshots what changed (not an overflowing batch: a
            // checkout of thousands of files is recorded by the VCS, not by us).
            if !batch.overflow {
                super::history::changed(&state, &pid, batch.paths.into_iter().collect(), batch.created);
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
    /// Directories watched (Windows: covered by the root's one watch).
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
        let mut got: Vec<String> = walk_dirs(root).map(|p| os::path::to_slash(p.strip_prefix(root).unwrap())).collect();
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

    /// A watcher on `root` as `start_watch` makes it (with a shorter debounce), stopped when
    /// dropped as `ProjectWatch` is.
    struct Watching(Arc<Inner>);

    impl Drop for Watching {
        fn drop(&mut self) {
            if let Some(deb) = self.0.deb.lock().take() {
                deb.stop_nonblocking();
            }
        }
    }

    impl std::ops::Deref for Watching {
        type Target = Arc<Inner>;
        fn deref(&self) -> &Arc<Inner> {
            &self.0
        }
    }

    async fn watching(root: &Path) -> Watching {
        let inner = Inner::new(root.to_path_buf(), git_dirs(root));
        inner.start(Duration::from_millis(100)).unwrap();
        let setup = inner.clone();
        tokio::task::spawn_blocking(move || setup.watch_all()).await.unwrap();
        Watching(inner)
    }

    /// `std::fs::rename`, retried for 5 s: on Windows a virus scanner may hold a new file
    /// for a moment (a handle of the watcher's own would outlast that).
    fn rename(from: &Path, to: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Err(e) = std::fs::rename(from, to) {
            assert!(std::time::Instant::now() < deadline, "rename {}: {e}", from.display());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The batches until none comes for 600 ms: paths, git, overflow.
    async fn collect(inner: &Arc<Inner>) -> (BTreeSet<String>, bool, bool) {
        let (mut paths, mut git, mut overflow) = (BTreeSet::new(), false, false);
        while let Ok(b) = tokio::time::timeout(Duration::from_millis(600), inner.next_batch()).await {
            paths.extend(b.paths);
            git |= b.git;
            overflow |= b.overflow;
        }
        (paths, git, overflow)
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
        let inner = watching(&root).await;

        for i in 0..10 {
            std::fs::write(root.join("src/lib.rs"), format!("{i}")).unwrap();
        }
        std::fs::create_dir_all(root.join("build/out")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();
        let (paths, git, overflow) = collect(&inner).await;
        assert!(paths.contains("src/lib.rs"), "{paths:?}");
        assert!(paths.contains("build"), "{paths:?}");
        assert!(!paths.iter().any(|p| p.starts_with("build/") || p.starts_with("node_modules/")), "{paths:?}");
        assert!(!git && !overflow);

        // New dir: the ignored one gets no watch (and a recursive watch drops its changes);
        // the regular one does.
        std::fs::create_dir_all(root.join("src/new")).unwrap();
        let _ = collect(&inner).await;
        std::fs::write(root.join("build/out/x.o"), "x").unwrap();
        std::fs::write(root.join("src/new/inner.rs"), "x").unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();
        let (paths, git, _) = collect(&inner).await;
        assert!(paths.contains("src/new/inner.rs"), "{paths:?}");
        assert!(!paths.iter().any(|p| p.starts_with("build/")), "{paths:?}");
        // The folder is not a change of its own (Windows reports it as modified).
        assert!(!paths.contains("src/new"), "{paths:?}");
        assert!(git);
        assert!(!inner.dirs.lock().contains(&root.join("build")));
    }

    /// Renaming a folder of a watched project works (on Windows a handle per folder would
    /// refuse it) and is reported under both names.
    #[tokio::test(flavor = "multi_thread")]
    async fn folders_of_a_watched_project_can_be_renamed() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("src/deep/er")).unwrap();
        std::fs::write(root.join("src/deep/er/a.rs"), "x").unwrap();
        let inner = watching(&root).await;
        rename(&root.join("src/deep"), &root.join("src/moved"));
        rename(&root.join("src"), &root.join("lib"));
        let (paths, _, _) = collect(&inner).await;
        assert!(paths.contains("src") && paths.contains("lib"), "{paths:?}");
        std::fs::write(root.join("lib/moved/er/a.rs"), "y").unwrap();
        let (paths, _, _) = collect(&inner).await;
        assert!(paths.contains("lib/moved/er/a.rs"), "{paths:?}");
    }

    /// Windows' watcher lost events: `overflow`, and git is looked at again. inotify's queue
    /// overflow stays unreported on Linux, as it always was.
    #[tokio::test]
    async fn lost_events_are_an_overflow() {
        use notify_debouncer_full::notify::Event;
        use notify_debouncer_full::notify::event::Flag;
        let g = GitDirs { gitdir: "/p/.git".into(), commondir: "/p/.git".into() };
        let inner = Inner::new(PathBuf::from("/p"), Some(g));
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        inner.on_events(vec![DebouncedEvent::new(rescan, std::time::Instant::now())]);
        let b = tokio::time::timeout(Duration::from_millis(500), inner.next_batch()).await;
        if os::watch::RESCAN_IS_OVERFLOW {
            let b = b.unwrap();
            assert!(b.overflow && b.git && b.paths.is_empty(), "{b:?}");
        } else {
            assert!(b.is_err(), "nothing to report");
        }
    }

    /// A watch that stopped on an error (Windows) is made again, and everything refreshed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_watch_is_made_again() {
        if !os::watch::RECURSIVE {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let inner = watching(&root).await;
        let stopped = notify::Error::io(std::io::ErrorKind::Other.into()).add_path(root.clone());
        inner.on_errors(vec![stopped]);
        let b = tokio::time::timeout(Duration::from_secs(5), inner.next_batch()).await.unwrap();
        assert!(b.overflow, "{b:?}");
        assert_eq!(inner.errors.load(Ordering::Relaxed), 1);
        std::fs::write(root.join("src/after.rs"), "x").unwrap();
        let (paths, _, _) = collect(&inner).await;
        assert!(paths.contains("src/after.rs"), "{paths:?}");
    }

    /// The folders whose changes are reported: those the walk enters, as a watch each
    /// (Linux) or kept from one recursive watch (Windows). Ignore files above the root
    /// count, as they do for the walk.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_walk_decides_which_folders_report() {
        let dir = tempfile::tempdir().unwrap();
        let repo = crate::util::os::path::canonicalize(dir.path()).unwrap();
        let root = repo.join("app").join("web");
        for d in ["src/gen/x", "build/out", "node_modules/p", "docs/keep/deep", "a/b/c", "dist/assets"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(repo.join(".gitignore"), "dist/\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n*.log\n").unwrap();
        std::fs::write(root.join("src/.gitignore"), "gen/\n").unwrap();
        let inner = watching(&root).await;
        for rel in ["src/a.rs", "build", "node_modules", "src/gen", "x.log", "a/b/c/d.rs", "docs/keep/deep/f.md", ".gitignore", "dist"] {
            assert!(inner.covers(&root.join(rel)), "{rel}");
        }
        for rel in ["build/out", "build/out/x.o", "node_modules/p/i.js", "src/gen/x", "src/gen/x/y.rs", "dist/app.js", "dist/assets/a.css"] {
            assert!(!inner.covers(&root.join(rel)), "{rel}");
        }
        assert!(!inner.covers(Path::new("/elsewhere/x")));
        // End to end: only the covered folder's change is reported.
        std::fs::write(root.join("dist/app.js"), "x").unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        let (paths, _, _) = collect(&inner).await;
        assert!(paths.contains("src/a.rs") && !paths.contains("dist/app.js"), "{paths:?}");
    }
}

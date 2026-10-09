//! Local History (CLion's): every version of a project text file that Workbench
//! saves, and every change the files watcher sees (agents editing!), kept in
//! `data_dir/local-history/<pid>/` (see `store`).
//!
//! What is recorded, and how it is labelled:
//! * **Saved in Workbench** (`save`): editor saves and Replace in Files, with the
//!   exact bytes written. The version on disk before the first save of a file is
//!   kept too (`base`), so the first save can be undone.
//! * **Changed on disk** (`disk`): a change the watcher saw (for a folder that
//!   appeared, the files in it), or one found when a file is read and its content
//!   is not the newest version we have.
//! * **External (agent) edit** (`agent`): a Claude Code session's `PostToolUse`
//!   hook named a file of the session's own project (Write, Edit, MultiEdit,
//!   NotebookEdit; container paths map through the workspace mount): the file is
//!   snapshotted then; if the watcher recorded that very content first (within
//!   30 s), its revision becomes the agent's. Changes made by an agent's shell
//!   commands (`sed -i`, a formatter) are not attributed: nothing names the file,
//!   and we do not guess.
//! * **Opened in Workbench** (`base`): the first version of a file Workbench read.
//!   **Last commit (HEAD)** (`base`): before the first recorded change of a file
//!   git tracks and Local History has nothing of, its committed version (so an
//!   agent's first edit of a file nobody opened can still be reviewed).
//! * **Deleted**, user labels ("Put Label…") and automatic labels before git
//!   operations that rewrite the working tree (`git.op` events other than fetch
//!   and push; other slices can call [`auto_label`]).
//!
//! Never recorded: sensitive files (the slice's own rules), `.git`, hard-ignored
//! and gitignored paths, binary files, files over 2 MB, symlinks leaving the
//! project. Kept: 7 days, at most 100 versions per file, 256 MB of compressed
//! blobs per project (oldest first), pruned in the background hourly and when a
//! project goes over its cap.
//!
//! Snapshots of one project run one after the other (`ProjectHistory::order`), so
//! revisions land in the order the disk went through them; saves and hooks never
//! wait for them (they are spawned).

mod diff;
pub mod routes;
pub mod store;
mod tools;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};

pub use store::{Entry, Kind};
use store::{MAX_FILE_BYTES, NewRevision, Policy, Store};

use super::content::{Decoded, decode_text};
use super::gitignore::IgnoreChecker;
use super::{HARD_IGNORE, Sensitive, in_git_dir, sha256_hex};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::McpTool;
use crate::projects::Project;
use crate::util;

/// A late hook re-labels a `disk` revision recorded this recently.
const ATTRIBUTE_WINDOW_MS: i64 = 30_000;
/// Paths snapshotted from one watcher batch.
const MAX_BATCH: usize = 300;
/// Watcher batches up to this size seed first changes from git HEAD.
const MAX_SEEDED_BATCH: usize = 20;
/// Entries (files and folders) looked at below the new folders of one batch.
const MAX_NEW_DIR_ENTRIES: usize = 2000;
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
const FIRST_PRUNE_AFTER: Duration = Duration::from_secs(60);

pub(crate) fn policy() -> Policy {
    Policy::default()
}

#[derive(Default)]
pub struct HistoryState {
    projects: Mutex<HashMap<String, Arc<ProjectHistory>>>,
    wake_pruner: tokio::sync::Notify,
}

pub struct ProjectHistory {
    pid: String,
    dir: PathBuf,
    /// Serializes snapshots: read the disk, then record.
    order: tokio::sync::Mutex<()>,
    store: Mutex<Option<Store>>,
}

impl ProjectHistory {
    /// Run `f` on the store, opening it first if needed. Blocking.
    fn with<R>(&self, f: impl FnOnce(&mut Store) -> std::io::Result<R>) -> std::io::Result<R> {
        let mut guard = self.store.lock();
        let store = match guard.as_mut() {
            Some(s) => s,
            None => guard.insert(Store::open(&self.dir)?),
        };
        f(store)
    }

    /// The newest hash of `path` if the store is already open (never blocks on disk).
    fn cached_latest_hash(&self, path: &str) -> Option<Option<String>> {
        // Called on async workers: never wait for a snapshot or a prune holding it.
        self.store.try_lock()?.as_ref().map(|s| s.latest_hash(path).map(str::to_string))
    }
}

/// `data_dir/local-history`.
fn root_dir(state: &AppState) -> PathBuf {
    state.paths.data_dir.join("local-history")
}

impl HistoryState {
    fn handle(&self, state: &AppState, pid: &str) -> Option<Arc<ProjectHistory>> {
        if !super::valid_name(pid) || pid.starts_with('.') {
            return None;
        }
        let mut map = self.projects.lock();
        Some(
            map.entry(pid.to_string())
                .or_insert_with(|| {
                    Arc::new(ProjectHistory {
                        pid: pid.to_string(),
                        dir: root_dir(state).join(pid),
                        order: tokio::sync::Mutex::new(()),
                        store: Mutex::new(None),
                    })
                })
                .clone(),
        )
    }
}

// ---------------------------------------------------------------- what is tracked

/// Path rules that need no disk access.
fn trackable_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !in_git_dir(rel)
        && !rel.contains(".wb-tmp-")
        && !rel.split('/').any(|c| HARD_IGNORE.contains(&c))
}

/// Why a file has no local history (`None`: it is tracked). Blocking.
pub(crate) fn untracked_reason(project: &Project, rel: &str) -> Option<&'static str> {
    if in_git_dir(rel) {
        return Some("git");
    }
    if Sensitive::new(&project.config.project.sensitive).matches(rel) {
        return Some("sensitive");
    }
    if !trackable_rel(rel) {
        return Some("ignored");
    }
    let abs = util::paths::resolve_in_root(&project.root, rel).ok()?;
    let md = std::fs::metadata(&abs).ok()?;
    if !md.is_file() {
        return None;
    }
    if IgnoreCache::default().ignored(&project.root, &abs) {
        return Some("ignored");
    }
    if md.len() > MAX_FILE_BYTES {
        return Some("tooLarge");
    }
    let mut head = vec![];
    use std::io::Read;
    if std::fs::File::open(&abs).and_then(|f| f.take(8192).read_to_end(&mut head)).is_ok() && head.contains(&0) {
        return Some("binary");
    }
    None
}

/// gitignore matchers per directory, for one batch.
#[derive(Default)]
struct IgnoreCache(HashMap<PathBuf, IgnoreChecker>);

impl IgnoreCache {
    fn ignored(&mut self, root: &Path, abs: &Path) -> bool {
        let Some(parent) = abs.parent() else { return false };
        self.0.entry(parent.to_path_buf()).or_insert_with(|| IgnoreChecker::for_dir(root, parent)).is_ignored(abs, false)
    }
}

enum Disk {
    Text(Vec<u8>),
    Gone,
    Skip,
}

/// The file's bytes if Local History keeps it. Blocking.
fn read_trackable(project: &Project, sensitive: &Sensitive, ignore: &mut IgnoreCache, rel: &str) -> Disk {
    if !trackable_rel(rel) || sensitive.matches(rel) {
        return Disk::Skip;
    }
    // Containment: a symlink out of the project is never read.
    let Ok(abs) = util::paths::resolve_in_root(&project.root, rel) else { return Disk::Skip };
    let md = match std::fs::metadata(&abs) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Disk::Gone,
        Err(_) => return Disk::Skip,
    };
    if !md.is_file() || md.len() > MAX_FILE_BYTES || ignore.ignored(&project.root, &abs) {
        return Disk::Skip;
    }
    match std::fs::read(&abs) {
        Ok(b) if b.len() as u64 <= MAX_FILE_BYTES && decode_text(&b) != Decoded::Binary => Disk::Text(b),
        Ok(_) => Disk::Skip,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Disk::Gone,
        Err(_) => Disk::Skip,
    }
}

// ---------------------------------------------------------------- recording

fn emit(state: &AppState, pid: &str, paths: Vec<String>) {
    if !paths.is_empty() {
        state.events.emit("files.history", Some(pid), json!({ "paths": paths }));
    }
}

/// After recording: wake the pruner when the project is over its caps.
fn check_caps(state: &AppState, store: &Store) {
    let p = policy();
    if store.stored_bytes() > p.max_bytes || store.len() > p.max_entries {
        state.files.history.wake_pruner.notify_one();
    }
}

/// Run `prepare` (disk reads, without the store lock, so history queries do not
/// wait for them) and then `work` on the store, for project `pid`, in snapshot
/// order, on the blocking pool; emit `files.history` for the paths it recorded.
/// Never fails the caller.
fn spawn_ordered<P, T, F>(state: &AppState, project: Arc<Project>, prepare: P, work: F)
where
    P: FnOnce(&Project) -> T + Send + 'static,
    T: Send + 'static,
    F: FnOnce(&Project, &mut Store, T) -> std::io::Result<Vec<String>> + Send + 'static,
{
    let Some(h) = state.files.history.handle(state, &project.id) else { return };
    let state = state.clone();
    tokio::spawn(async move {
        let _order = h.order.lock().await;
        let st = state.clone();
        let hh = h.clone();
        let res = tokio::task::spawn_blocking(move || {
            let prepared = prepare(&project);
            hh.with(|store| {
                let paths = work(&project, store, prepared)?;
                check_caps(&st, store);
                Ok(paths)
            })
        })
        .await;
        match res {
            Ok(Ok(paths)) => emit(&state, &h.pid, paths),
            Ok(Err(e)) => tracing::warn!("local history of {}: {e}", h.pid),
            Err(e) => tracing::warn!("local history of {}: {e}", h.pid),
        }
    });
}

/// A save in Workbench wrote `data` over `prev` (`None`: a new file).
pub(crate) fn saved(state: &AppState, project: Arc<Project>, rel: String, prev: Option<Vec<u8>>, data: Vec<u8>, label: Option<&'static str>) {
    if !trackable_rel(&rel) || data.len() as u64 > MAX_FILE_BYTES {
        return;
    }
    spawn_ordered(state, project, |_| (), move |project, store, ()| {
        let sensitive = Sensitive::new(&project.config.project.sensitive);
        if sensitive.matches(&rel) || decode_text(&data) == Decoded::Binary {
            return Ok(vec![]);
        }
        if let Ok(abs) = util::paths::resolve_in_root(&project.root, &rel) {
            if IgnoreCache::default().ignored(&project.root, &abs) {
                return Ok(vec![]);
            }
        }
        let now = util::now_ms();
        if let Some(prev) = prev.as_deref().filter(|p| p.len() as u64 <= MAX_FILE_BYTES && decode_text(p) != Decoded::Binary) {
            if store.latest_hash(&rel) != Some(sha256_hex(prev).as_str()) {
                let mut r = NewRevision::content(&rel, Kind::Base, now - 1, prev);
                r.label = Some("Before the first save".into());
                store.record(r)?;
            }
        }
        let mut r = NewRevision::content(&rel, Kind::Save, now, &data);
        r.label = label.map(str::to_string);
        Ok(store.record(r)?.map(|_| vec![rel]).unwrap_or_default())
    });
}

/// The editor read `rel` (with this etag): keep that version if it is new to us.
pub(crate) fn opened(state: &AppState, project: Arc<Project>, rel: String, etag: String) {
    if !trackable_rel(&rel) {
        return;
    }
    if let Some(h) = state.files.history.handle(state, &project.id) {
        if h.cached_latest_hash(&rel).flatten().as_deref() == Some(etag.as_str()) {
            return;
        }
    }
    spawn_ordered(state, project, |_| (), move |project, store, ()| {
        if store.latest_hash(&rel) == Some(etag.as_str()) {
            return Ok(vec![]);
        }
        let sensitive = Sensitive::new(&project.config.project.sensitive);
        let Disk::Text(bytes) = read_trackable(project, &sensitive, &mut IgnoreCache::default(), &rel) else { return Ok(vec![]) };
        let mut r = NewRevision::content(&rel, Kind::Base, util::now_ms(), &bytes);
        r.label = Some("Opened in Workbench".into());
        Ok(store.record(r)?.map(|_| vec![rel]).unwrap_or_default())
    });
}

/// The watcher saw `paths` change; `new_dirs` (also among `paths`) are folders
/// that appeared (created, or moved in): files written into them before the
/// watcher watched them are only found by looking.
pub(crate) fn changed(state: &AppState, pid: &str, paths: Vec<String>, new_dirs: Vec<String>) {
    let Some(project) = state.projects.get(pid) else { return };
    let mut paths: Vec<String> = paths.into_iter().filter(|p| trackable_rel(p)).take(MAX_BATCH).collect();
    let new_dirs: Vec<String> = new_dirs.into_iter().filter(|p| trackable_rel(p)).collect();
    if paths.is_empty() && new_dirs.is_empty() {
        return;
    }
    let read = move |project: &Project| {
        let sensitive = Sensitive::new(&project.config.project.sensitive);
        let mut ignore = IgnoreCache::default();
        files_in_new_dirs(project, &new_dirs, &mut paths);
        paths.into_iter().map(|rel| (read_trackable(project, &sensitive, &mut ignore, &rel), rel)).collect::<Vec<_>>()
    };
    spawn_ordered(state, project, read, move |project, store, files| {
        // A big batch is a checkout, a generator or a formatter run: no git per file.
        let seed = files.len() <= MAX_SEEDED_BATCH;
        let mut checkout = Checkout::new(&project.root);
        let mut out = vec![];
        for (disk, rel) in files {
            let now = util::now_ms();
            match disk {
                Disk::Text(bytes) => {
                    if seed {
                        seed_from_head(store, &mut checkout, &rel, &bytes, now)?;
                    }
                    if store.record(NewRevision::content(&rel, Kind::Disk, now, &bytes))?.is_some() {
                        out.push(rel);
                    }
                }
                Disk::Gone => out.extend(mark_deleted(store, &rel, now)?),
                Disk::Skip => {}
            }
        }
        Ok(out)
    });
}

/// Add the files below `dirs` (new folders) to `paths`, within the batch's budget
/// (`MAX_BATCH`). A folder holding more than the budget left (a clone, a checkout,
/// an unpacked archive) is skipped whole rather than recorded in part. The walk
/// honours .gitignore, skips hard-ignored folders and never follows links; the
/// files it finds go through the usual checks (sensitive, binary, size). Blocking.
fn files_in_new_dirs(project: &Project, dirs: &[String], paths: &mut Vec<String>) {
    let mut seen: std::collections::HashSet<String> = paths.iter().cloned().collect();
    let nested = project.nested_repo_dirs();
    let mut entries = 0usize;
    for dir in dirs {
        let Ok(abs) = util::paths::resolve_in_root(&project.root, dir) else { continue };
        let Ok(md) = std::fs::symlink_metadata(&abs) else { continue };
        if !md.is_dir() {
            continue;
        }
        // The walk does not filter its own starting point.
        let parent = abs.parent().unwrap_or(&project.root);
        if IgnoreChecker::for_dir(&project.root, parent).is_ignored(&abs, true) {
            continue;
        }
        let budget = MAX_BATCH.saturating_sub(paths.len());
        let mut found = vec![];
        let mut too_many = false;
        let walk = super::gitignore::walk(&abs, &nested).parents(true).build();
        for e in walk.flatten() {
            entries += 1;
            if entries > MAX_NEW_DIR_ENTRIES {
                too_many = true;
                break;
            }
            if !e.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Some(rel) = util::paths::relative_to(&project.root, e.path()) else { continue };
            if seen.contains(&rel) {
                continue;
            }
            if found.len() >= budget {
                too_many = true;
                break;
            }
            found.push(rel);
        }
        if too_many {
            tracing::debug!("local history of {}: {dir} holds too many files to snapshot", project.id);
            if entries > MAX_NEW_DIR_ENTRIES {
                return;
            }
            continue;
        }
        for rel in found {
            seen.insert(rel.clone());
            paths.push(rel);
        }
    }
}

/// Before the first recorded change of a file with no history, keep its version in
/// the last commit (when git tracks it and it differs), so that change can be
/// compared with something. Labelled as what it is: it may predate edits made while
/// Workbench was not running. It is kept as a checkout writes it ([`Checkout`]).
fn seed_from_head(store: &mut Store, checkout: &mut Checkout, rel: &str, now: &[u8], ts: i64) -> std::io::Result<()> {
    if store.latest(rel).is_some() {
        return Ok(());
    }
    let Some(head) = head_blob(checkout.root, rel) else { return Ok(()) };
    let head = checkout.as_checked_out(rel, &head, now);
    if *head == *now || decode_text(&head) == Decoded::Binary {
        return Ok(());
    }
    let mut r = NewRevision::content(rel, Kind::Base, ts - 1, &head);
    r.label = Some("Last commit (HEAD)".into());
    store.record(r).map(|_| ())
}

/// Committed files as a checkout of `root` writes them: with CRLF line ends where git's
/// rules say so (`core.autocrlf`, the default of Git for Windows, or the `text`/`eol`
/// attributes, on every OS), else as committed. Compared as committed, every line of such a
/// file would differ. The settings are read from git once (bounded, like `head_blob`), the
/// attributes per file, and only for a file with a line end to convert that has CRLFs on
/// disk: any other file is kept as committed with no lookup (like the git slice's `eol`).
struct Checkout<'a> {
    root: &'a Path,
    config: Option<EolConfig>,
}

impl<'a> Checkout<'a> {
    fn new(root: &'a Path) -> Self {
        Checkout { root, config: None }
    }

    /// `head` as the checkout that wrote `now`, the file on disk, would have written it.
    fn as_checked_out<'b>(&mut self, rel: &str, head: &'b [u8], now: &[u8]) -> Cow<'b, [u8]> {
        if !now.windows(2).any(|w| w == b"\r\n") || !has_lone_lf(head) {
            return Cow::Borrowed(head);
        }
        let root = self.root;
        let config = *self.config.get_or_insert_with(|| EolConfig::read(root));
        let attrs = git_output(root, &["check-attr", "-z", "text", "eol", "crlf", "--", rel], 64 * 1024).unwrap_or_default();
        let eol = config.eol(&EolAttrs::parse(&attrs));
        to_worktree(head, eol)
    }
}

/// What a checkout does to the line ends of one file (git's `convert.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eol {
    AsCommitted,
    /// Lone LFs become CRLF.
    Crlf,
    /// The same, unless the file has a CR already or looks binary (`text=auto`, or
    /// `core.autocrlf` for a file without attributes).
    AutoCrlf,
}

/// `core.autocrlf` and `core.eol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EolConfig {
    /// `Some(true)`: `true`; `Some(false)`: `input`; `None`: `false` or not set.
    autocrlf: Option<bool>,
    /// `core.eol` is `crlf`, or `native` (or not set) where git's native line end is CRLF.
    eol_crlf: bool,
}

impl EolConfig {
    /// From `git config` (every scope). Blocking.
    fn read(root: &Path) -> Self {
        let out = git_output(root, &["config", "-z", "--get-regexp", r"^core\.(autocrlf|eol)$"], 64 * 1024).unwrap_or_default();
        Self::parse(&String::from_utf8_lossy(&out), crate::util::os::fs::NATIVE_CRLF)
    }

    /// `git config -z --get-regexp` output: `key\nvalue\0` entries (a key alone is `true`);
    /// the last value of a key wins.
    fn parse(out: &str, native_crlf: bool) -> Self {
        let (mut autocrlf, mut eol) = (None, None);
        for entry in out.split('\0').filter(|e| !e.is_empty()) {
            let (key, value) = entry.split_once('\n').map_or((entry, None), |(k, v)| (k, Some(v.to_ascii_lowercase())));
            match key {
                "core.autocrlf" => {
                    autocrlf = match value.as_deref() {
                        None | Some("true" | "yes" | "on") => Some(true),
                        Some("input") => Some(false),
                        Some(v) => v.parse::<i64>().is_ok_and(|n| n != 0).then_some(true),
                    }
                }
                "core.eol" => eol = value,
                _ => {}
            }
        }
        let eol_crlf = match eol.as_deref() {
            Some("lf") => false,
            Some("crlf") => true,
            _ => native_crlf,
        };
        EolConfig { autocrlf, eol_crlf }
    }

    /// Whether `text` files get CRLF (`text_eol_is_crlf`).
    fn text_crlf(&self) -> bool {
        self.autocrlf.unwrap_or(self.eol_crlf)
    }

    /// Git's decision for a file with these attributes (`convert_attrs`, `output_eol`).
    fn eol(&self, attrs: &EolAttrs) -> Eol {
        #[derive(PartialEq)]
        enum Action {
            Undefined,
            Binary,
            Text,
            TextInput,
            TextCrlf,
            Auto,
            AutoInput,
            AutoCrlf,
        }
        let parse = |v: &str| match v {
            "set" => Action::Text,
            "unset" => Action::Binary,
            "input" => Action::TextInput,
            "auto" => Action::Auto,
            _ => Action::Undefined,
        };
        let mut action = parse(&attrs.text);
        if action == Action::Undefined {
            action = parse(&attrs.crlf);
        }
        if action != Action::Binary {
            action = match (action, attrs.eol.as_str()) {
                (Action::Auto, "lf") => Action::AutoInput,
                (Action::Auto, "crlf") => Action::AutoCrlf,
                (_, "lf") => Action::TextInput,
                (_, "crlf") => Action::TextCrlf,
                (a, _) => a,
            };
        }
        let text_crlf = self.text_crlf();
        match action {
            Action::Text if text_crlf => Eol::Crlf,
            Action::Auto if text_crlf => Eol::AutoCrlf,
            Action::Undefined if self.autocrlf == Some(true) => Eol::AutoCrlf,
            Action::TextCrlf => Eol::Crlf,
            Action::AutoCrlf => Eol::AutoCrlf,
            _ => Eol::AsCommitted,
        }
    }
}

/// The `text`, `eol` and (older) `crlf` attributes of a file: `set`, `unset`,
/// `unspecified` or a value, as `git check-attr` prints them.
#[derive(Debug, Default)]
struct EolAttrs {
    text: String,
    eol: String,
    crlf: String,
}

impl EolAttrs {
    /// `git check-attr -z` output: `path\0attribute\0value\0` for each.
    fn parse(out: &[u8]) -> Self {
        let mut attrs = EolAttrs::default();
        let fields: Vec<String> = out.split(|&b| b == 0).map(|f| String::from_utf8_lossy(f).into_owned()).collect();
        for triple in fields.chunks_exact(3) {
            let value = triple[2].clone();
            match triple[1].as_str() {
                "text" => attrs.text = value,
                "eol" => attrs.eol = value,
                "crlf" => attrs.crlf = value,
                _ => {}
            }
        }
        attrs
    }
}

/// A LF with no CR before it.
fn has_lone_lf(text: &[u8]) -> bool {
    text.iter().enumerate().any(|(i, &b)| b == b'\n' && (i == 0 || text[i - 1] != b'\r'))
}

/// `head` checked out with `eol` (`crlf_to_worktree`).
fn to_worktree(head: &[u8], eol: Eol) -> Cow<'_, [u8]> {
    let convert = match eol {
        Eol::AsCommitted => false,
        Eol::Crlf => true,
        Eol::AutoCrlf => !head.contains(&b'\r') && !head.contains(&0),
    };
    if !convert || !has_lone_lf(head) {
        return Cow::Borrowed(head);
    }
    let mut out = Vec::with_capacity(head.len() + head.len() / 16);
    for (i, &b) in head.iter().enumerate() {
        if b == b'\n' && (i == 0 || head[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    Cow::Owned(out)
}

/// `rel` as committed in HEAD (`git cat-file`, bounded in size and time). Blocking.
fn head_blob(root: &Path, rel: &str) -> Option<Vec<u8>> {
    let spec = format!("HEAD:./{rel}");
    let size: u64 = String::from_utf8(git_output(root, &["cat-file", "-s", &spec], 64)?).ok()?.trim().parse().ok()?;
    if size > MAX_FILE_BYTES {
        return None;
    }
    git_output(root, &["cat-file", "blob", &spec], MAX_FILE_BYTES)
}

/// Run `git -C root args…` and return its stdout (at most `max` bytes), or `None`
/// when it fails. A watchdog kills it after 5 s.
fn git_output(root: &Path, args: &[&str], max: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    cmd.env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0");
    for k in util::proc::SESSION_ENV_VARS {
        cmd.env_remove(k);
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    // Unix: git alone. Windows: also what it starts (Git for Windows' `git.exe` on PATH is
    // a launcher whose child holds the pipe); the group lives until git is waited for.
    let group = util::os::proc::ProcGroup::attach_single(child.id());
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn({
        let group = group.clone();
        move || {
            if done_rx.recv_timeout(Duration::from_secs(5)).is_err() {
                // The child is not reaped before `done` is sent: the pid is still its own.
                group.kill();
            }
        }
    });
    let mut out = vec![];
    let read = child.stdout.take().map(|s| s.take(max + 1).read_to_end(&mut out));
    let _ = done_tx.send(());
    let status = child.wait();
    let _ = watchdog.join();
    match (read, status) {
        (Some(Ok(_)), Ok(s)) if s.success() && out.len() as u64 <= max => Some(out),
        _ => None,
    }
}

/// `rel` is gone: it, or every tracked file below it (a folder moved away).
fn mark_deleted(store: &mut Store, rel: &str, now: i64) -> std::io::Result<Vec<String>> {
    let prefix = format!("{rel}/");
    let mut targets: Vec<String> = vec![];
    if store.latest_hash(rel).is_some() {
        targets.push(rel.to_string());
    }
    targets.extend(store.tracked_paths().filter(|p| p.starts_with(&prefix)).map(str::to_string));
    let mut out = vec![];
    for t in targets {
        let r = NewRevision { path: &t, kind: Kind::Deleted, ts: now, content: None, label: None, by: None, who: None };
        if store.record(r)?.is_some() {
            out.push(t);
        }
    }
    Ok(out)
}

/// A Claude Code hook arrived for `terminal_id` (called by the terminals slice for
/// every hook payload). A `PostToolUse` of a file-writing tool names the file the
/// agent changed: snapshot it now as the agent's edit, or, when the watcher got
/// there first with the same content, make that revision the agent's.
///
/// Only files of the session's own project count (as for its MCP tools): a hook
/// naming another project's file, or sent by a session without a project, changes
/// nothing (the watcher still records what really changed, as "Changed on disk").
pub fn agent_hook(state: &AppState, terminal_id: &str, payload: &Value) {
    let Some(info) = state.terminals.info(terminal_id) else { return };
    agent_edit(state, &info, payload);
}

/// `agent_hook` for a known session.
fn agent_edit(state: &AppState, info: &crate::terminals::TerminalInfo, payload: &Value) {
    if payload.get("hook_event_name").and_then(Value::as_str) != Some("PostToolUse") {
        return;
    }
    let tool = payload.get("tool_name").and_then(Value::as_str).unwrap_or("");
    if !matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return;
    }
    let input = payload.get("tool_input");
    let Some(file) = input.and_then(|i| i.get("file_path").or_else(|| i.get("notebook_path"))).and_then(Value::as_str) else { return };
    let Some(project) = info.project_id.as_deref().and_then(|p| state.projects.get(p)) else { return };
    let container = crate::terminals::in_container(info).then(|| {
        let id = info.meta.get("container").and_then(|c| c.get("id")).and_then(Value::as_str).unwrap_or("");
        crate::devcontainer::workspace_mount(state, &project.id, id)
    });
    let Some(abs) = host_file(file, Path::new(&info.cwd), container.as_ref().map(Option::as_ref)) else { return };
    let Some(rel) = rel_in_project(&project.root, &abs) else { return };
    let who = Some(info.title.clone()).filter(|t| !t.trim().is_empty());
    let now = util::now_ms();
    let terminal = info.id.clone();
    let path = rel.clone();
    let read = move |project: &Project| {
        let sensitive = Sensitive::new(&project.config.project.sensitive);
        read_trackable(project, &sensitive, &mut IgnoreCache::default(), &path)
    };
    spawn_ordered(state, project, read, move |project, store, disk| {
        match disk {
            Disk::Text(bytes) => {
                let hash = sha256_hex(&bytes);
                if store.latest_hash(&rel) == Some(hash.as_str()) {
                    let done = store.attribute(&rel, &hash, now - ATTRIBUTE_WINDOW_MS, &terminal, who.as_deref())?;
                    return Ok(done.map(|_| vec![rel]).unwrap_or_default());
                }
                seed_from_head(store, &mut Checkout::new(&project.root), &rel, &bytes, now)?;
                let r = NewRevision { path: &rel, kind: Kind::Agent, ts: now, content: Some(&bytes), label: None, by: Some(terminal), who };
                Ok(store.record(r)?.map(|_| vec![rel]).unwrap_or_default())
            }
            Disk::Gone => mark_deleted(store, &rel, now),
            Disk::Skip => Ok(vec![]),
        }
    });
}

/// The host path of a file an agent named. `container`: for a session in a dev
/// container, the workspace mount `(host folder, container folder)` (`None` inside
/// when it is not known). A container path maps through that mount, not through
/// the workspace folder (which is only the default working directory: `/` for
/// compose, or a folder below the mount); a path outside the mount is not guessed
/// at. Relative paths are relative to the session's working directory.
fn host_file(file: &str, cwd: &Path, container: Option<Option<&(PathBuf, String)>>) -> Option<PathBuf> {
    let named = Path::new(file);
    if !named.is_absolute() {
        return Some(cwd.join(named));
    }
    match container {
        None => Some(named.to_path_buf()),
        Some(mount) => {
            let (src, dst) = mount?;
            let rest = named.strip_prefix(dst).ok()?;
            Some(if rest.as_os_str().is_empty() { src.clone() } else { src.join(rest) })
        }
    }
}

/// `abs` as a path in the project at `root` (which it must stay inside, also
/// through symlinks), trying the canonical folder when the plain path is not under
/// `root` (a session started in a symlinked checkout). Spelled as the disk spells it
/// (`os::path::on_disk_case`), so the store keys a file one way whatever case an agent
/// wrote it in (Windows).
fn rel_in_project(root: &Path, abs: &Path) -> Option<String> {
    let rel = util::paths::relative_to(root, abs).or_else(|| {
        let canon = util::os::path::canonicalize(abs.parent()?).ok()?.join(abs.file_name()?);
        util::paths::relative_to(root, &canon)
    })?;
    let checked = util::paths::resolve_in_root(root, &rel).ok()?;
    let disk = util::os::path::on_disk_case(root, &checked);
    // Another spelling is checked like the first.
    let checked = if disk == checked { checked } else { util::paths::resolve_in_root(root, &util::paths::relative_to(root, &disk)?).ok()? };
    util::paths::relative_to(root, &checked).filter(|r| !r.is_empty())
}

// ---------------------------------------------------------------- labels

/// Put a label on the project (`path` empty), a folder or a file.
pub(crate) async fn put_label(state: &AppState, pid: &str, path: &str, text: &str, kind: Kind) -> ApiResult<Entry> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("a label needs text"));
    }
    if text.chars().count() > 200 || text.chars().any(char::is_control) {
        return Err(ApiError::bad_request("a label is one line of at most 200 characters"));
    }
    let h = state.files.history.handle(state, pid).ok_or_else(|| ApiError::bad_request("unusable project id"))?;
    let (path, text) = (path.to_string(), text.to_string());
    let _order = h.order.lock().await;
    let hh = h.clone();
    let entry = tokio::task::spawn_blocking(move || {
        hh.with(|s| s.record(NewRevision { path: &path, kind, ts: util::now_ms(), content: None, label: Some(text), by: None, who: None }))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(|e| ApiError::internal(format!("cannot write the local history: {e}")))?
    .ok_or_else(|| ApiError::internal("label not recorded"))?;
    emit(state, pid, vec![entry.path.clone()]);
    Ok(entry)
}

/// An automatic label ("Before git pull"), for slices about to rewrite a project's
/// working tree. Best effort.
pub async fn auto_label(state: &AppState, pid: &str, text: &str) {
    if let Err(e) = put_label(state, pid, "", text, Kind::Auto).await {
        tracing::debug!("local history label for {pid}: {}", e.message);
    }
}

// ---------------------------------------------------------------- queries (for routes and tools)

impl HistoryState {
    /// Read-only access to a project's store. Blocking.
    pub(crate) fn read<R>(&self, state: &AppState, pid: &str, f: impl FnOnce(&Store) -> R) -> ApiResult<R> {
        let h = self.handle(state, pid).ok_or_else(|| ApiError::bad_request("unusable project id"))?;
        h.with(|s| Ok(f(s))).map_err(|e| ApiError::internal(format!("cannot read the local history: {e}")))
    }
}

/// A revision as the API shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryOut {
    pub id: u64,
    pub ts: i64,
    pub path: String,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Agent edits: the session's terminal id and title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
}

impl From<&Entry> for EntryOut {
    fn from(e: &Entry) -> Self {
        EntryOut {
            id: e.id,
            ts: e.ts,
            path: e.path.clone(),
            kind: e.kind,
            label: e.label.clone(),
            size: e.size,
            hash: e.hash.clone(),
            by: e.by.clone(),
            who: e.who.clone(),
        }
    }
}

/// How a revision reads in a list ("Saved in Workbench", "External (agent) edit · …").
pub fn describe(e: &EntryOut) -> String {
    match e.kind {
        Kind::Save => e.label.clone().unwrap_or_else(|| "Saved in Workbench".into()),
        Kind::Disk => "Changed on disk".into(),
        Kind::Agent => match &e.who {
            Some(w) => format!("External (agent) edit · {w}"),
            None => "External (agent) edit".into(),
        },
        Kind::Base => e.label.clone().unwrap_or_else(|| "Opened in Workbench".into()),
        Kind::Deleted => "Deleted".into(),
        Kind::Label | Kind::Auto | Kind::Attr => e.label.clone().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------- background work

pub(crate) fn start(state: &AppState) {
    let st = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_PRUNE_AFTER).await;
        loop {
            prune_all(&st).await;
            tokio::select! {
                _ = tokio::time::sleep(PRUNE_EVERY) => {}
                _ = st.files.history.wake_pruner.notified() => {
                    // Let a burst of recording settle first.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
    let st = state.clone();
    tokio::spawn(async move {
        let mut rx = st.events.subscribe();
        loop {
            match rx.recv().await {
                Ok(ev) if ev.kind == "git.op" => {
                    let Some(pid) = ev.project_id.clone() else { continue };
                    let op = ev.data.get("op").and_then(Value::as_str).unwrap_or("");
                    let first = ev.data.get("line").and_then(Value::as_str).is_some_and(|l| l.starts_with("$ git "));
                    if first && rewrites_working_tree(op) {
                        auto_label(&st, &pid, &format!("Before git {op}")).await;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// `git.op` kinds that can change files in the working tree.
fn rewrites_working_tree(op: &str) -> bool {
    !op.is_empty() && !matches!(op, "fetch" | "push" | "delete-remote-branch")
}

/// Prune every project's history (registered or not: a removed project's history
/// simply expires).
pub(crate) async fn prune_all(state: &AppState) {
    let root = root_dir(state);
    let dirs: Vec<String> = match std::fs::read_dir(&root) {
        Ok(d) => d.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => return,
    };
    for pid in dirs {
        let Some(h) = state.files.history.handle(state, &pid) else { continue };
        let _order = h.order.lock().await;
        let project = state.projects.get(&pid);
        let hh = h.clone();
        let res = tokio::task::spawn_blocking(move || {
            let sensitive = project.as_ref().map(|p| Sensitive::new(&p.config.project.sensitive)).unwrap_or_else(Sensitive::defaults);
            let r = hh.with(|s| {
                let stats = s.prune(&policy(), util::now_ms(), |p| sensitive.matches(p))?;
                Ok((stats, s.is_empty()))
            });
            if let Ok((_, true)) = &r {
                // Nothing left: drop the folder too (the store stays valid and empty).
                let _ = std::fs::remove_dir_all(&hh.dir);
            }
            r
        })
        .await;
        match res {
            Ok(Ok((stats, _))) if stats.removed_entries > 0 => {
                tracing::debug!("local history of {pid}: pruned {} entries, {} blobs", stats.removed_entries, stats.removed_blobs)
            }
            Ok(Err(e)) => tracing::warn!("local history of {pid}: pruning failed: {e}"),
            _ => {}
        }
    }
}

pub(crate) fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

#[cfg(test)]
mod tests;

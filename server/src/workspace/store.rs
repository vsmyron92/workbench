//! Scopes, registries and card files: everything that touches the disk. All of it is
//! blocking; handlers run it through `super::blocking` while holding the slice's write
//! lock for anything that writes.
//!
//! * A **scope** is `home` or a project id. Its cards live in
//!   `data_dir/workspace/<scope>/workspace.json`, their files in
//!   `data_dir/workspace/<scope>/<folder>/` (repositories stay clean).
//! * A project whose root has a Mr. Mak `workspace/workspace.json` also shows those
//!   cards (origin `repo`, id `repo:<id>`). They are repository content, so untrusted:
//!   folders and paths are validated like any client path, and only `status`,
//!   `pinned` and `updated` are ever written back.
//! * Every registry write is a compare-and-swap: the new file is staged (written and
//!   synced) first, and only swapped in while the registry still holds what was read
//!   (`renameat2(RENAME_EXCHANGE)`, checked after the swap and undone on a mismatch),
//!   else the update starts over on the newer file. An agent that replaces the file
//!   meanwhile never loses its change.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{self, Card, Doc};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::projects::Project;
use crate::util;

pub const HOME: &str = "home";
/// Public id prefix of cards from a repository's own registry.
pub const REPO_PREFIX: &str = "repo:";
/// Shared report assets every scope gets (`../_shared/report.css` from a card folder).
pub const SHARED_DIR: &str = "_shared";
/// Markdown and text content served for viewing and editing.
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// Uploads into a card folder.
pub const MAX_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;
/// Entries listed per directory (per page).
pub const MAX_LIST: usize = 2000;
/// Names read from one directory before a listing gives up on the rest.
const MAX_SCAN: usize = 50_000;
/// Copying a directory into a card (a gallery dragged from the project tree).
const MAX_IMPORT_FILES: usize = 2000;
const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Markdown backups kept per card.
const MAX_BACKUPS: usize = 20;

const REPORT_CSS: &str = include_str!("assets/report.css");
const REPORT_JS: &str = include_str!("assets/report.js");

// ---------------------------------------------------------------- scopes

pub struct Scope {
    pub id: String,
    pub name: String,
    /// `data_dir/workspace/<id>`.
    pub dir: PathBuf,
    pub project: Option<Arc<Project>>,
}

impl Scope {
    pub fn registry(&self) -> PathBuf {
        self.dir.join("workspace.json")
    }

    /// `<root>/workspace` when the project root holds a Mr. Mak registry.
    pub fn repo_dir(&self) -> Option<PathBuf> {
        let dir = self.project.as_ref()?.root.join("workspace");
        dir.join("workspace.json").is_file().then_some(dir)
    }

    /// `projectId` for events about this scope.
    pub fn project_id(&self) -> Option<&str> {
        self.project.as_ref().map(|p| p.id.as_str())
    }
}

/// `data_dir/workspace`.
pub fn root_dir(state: &AppState) -> PathBuf {
    state.paths.data_dir.join("workspace")
}

/// Where deleted cards go (`workspace-trash/<scope>/<folder>~<time>/`, with the card's
/// entry). Outside `data_dir/workspace`, so the watcher never holds inotify watches
/// on deleted cards.
pub fn trash_dir(state: &AppState) -> PathBuf {
    state.paths.data_dir.join("workspace-trash")
}

/// Previous versions of edited markdown (`workspace-backups/<scope>/<card>/`), also
/// outside the watched tree.
pub fn backups_dir(state: &AppState) -> PathBuf {
    state.paths.data_dir.join("workspace-backups")
}

/// Move the trash and backups of earlier versions (`data_dir/workspace/.trash`,
/// `.backups`) out of the watched tree. Best effort; runs before the watcher starts.
pub fn migrate_legacy_dirs(state: &AppState) {
    let root = root_dir(state);
    for (old, new) in [(root.join(".trash"), trash_dir(state)), (root.join(".backups"), backups_dir(state))] {
        if !old.is_dir() {
            continue;
        }
        let dest = if new.exists() {
            // Both exist (an older build ran in between): keep the old one beside.
            new.join(format!("legacy-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")))
        } else {
            new.clone()
        };
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(&old, &dest) {
            Ok(()) => util::fs::set_mode(&new, 0o700),
            Err(e) => tracing::warn!("workspace: cannot move {} to {}: {e}", old.display(), dest.display()),
        }
    }
}

/// `home` or a known project. No project gets the id `home` (the registry reserves
/// it), so the Home scope never hides a project's cards.
pub fn scope(state: &AppState, id: &str) -> ApiResult<Scope> {
    let id = id.trim();
    if id == HOME {
        return Ok(Scope { id: HOME.into(), name: "Home".into(), dir: root_dir(state).join(HOME), project: None });
    }
    let project = state.projects.get(id).ok_or_else(|| ApiError::not_found(format!("no workspace scope {id:?}: use \"home\" or a project id")))?;
    // Project ids are slugs; never let one become a path.
    if !model::valid_folder(&project.id) {
        return Err(ApiError::bad_request("unusable project id for a workspace scope"));
    }
    Ok(Scope { id: project.id.clone(), name: project.name.clone(), dir: root_dir(state).join(&project.id), project: Some(project) })
}

/// Home first, then every project.
pub fn all_scopes(state: &AppState) -> Vec<Scope> {
    let mut out = vec![];
    if let Ok(h) = scope(state, HOME) {
        out.push(h);
    }
    for p in state.projects.list() {
        if p.id == HOME {
            continue;
        }
        if let Ok(s) = scope(state, &p.id) {
            out.push(s);
        }
    }
    out
}

/// Create the scope directory and its shared report assets (never overwriting edits).
pub fn ensure_scope_dir(scope: &Scope) -> ApiResult<()> {
    std::fs::create_dir_all(&scope.dir)?;
    util::fs::set_mode(&scope.dir, 0o700);
    let shared = scope.dir.join(SHARED_DIR);
    std::fs::create_dir_all(&shared)?;
    for (name, body) in [("report.css", REPORT_CSS), ("report.js", REPORT_JS)] {
        let p = shared.join(name);
        if !p.exists() {
            util::fs::write_atomic(&p, body.as_bytes(), 0o644)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- registries

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Our own registry in the data dir.
    Workbench,
    /// A repository's Mr. Mak `workspace/workspace.json` (read-only but status/pin).
    Repo,
}

/// A card and where it lives.
#[derive(Debug, Clone)]
pub struct Located {
    pub card: Card,
    pub origin: Origin,
    /// The directory holding the card folders.
    pub base: PathBuf,
    pub registry: PathBuf,
}

impl Located {
    pub fn public_id(&self) -> String {
        match self.origin {
            Origin::Workbench => self.card.id.clone(),
            Origin::Repo => format!("{REPO_PREFIX}{}", self.card.id),
        }
    }

    pub fn editable(&self) -> bool {
        self.origin == Origin::Workbench
    }

    /// The card folder (validated; it may not exist yet).
    pub fn dir(&self) -> ApiResult<PathBuf> {
        card_dir(&self.base, &self.card.folder)
    }

    fn require_editable(&self) -> ApiResult<()> {
        if self.editable() {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "this card comes from the repository's workspace/workspace.json: Workbench only changes its status and pin there",
            ))
        }
    }
}

/// `base/folder`, refusing folder names that are not one plain component and
/// folders that resolve (through a symlink) outside `base`.
pub fn card_dir(base: &Path, folder: &str) -> ApiResult<PathBuf> {
    if !model::valid_folder(folder) {
        return Err(ApiError::forbidden(format!("unusable card folder {folder:?}")));
    }
    util::paths::resolve_in_root(base, folder)
}

fn invalid_registry(path: &Path, e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_registry",
        format!("{} is not a valid workspace registry ({e}); fix the file or move it away", crate::config::contract_tilde(path)),
    )
}

/// Read and parse a registry; `Ok(None)` when the file does not exist. A parse
/// failure is retried briefly: another writer may be half-way through a plain write.
pub fn read_registry(path: &Path) -> ApiResult<Option<(Vec<u8>, Doc)>> {
    let mut last = None;
    for attempt in 0..3 {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match Doc::parse(&bytes) {
            Ok(doc @ Doc::Object(_)) => return Ok(Some((bytes, doc))),
            Ok(_) => last = Some("the top level is not an object".to_string()),
            Err(e) => last = Some(e.to_string()),
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    }
    Err(invalid_registry(path, last.unwrap_or_default()))
}

/// Compare-and-swap update: read, apply `f`, stage the result, and put it in place
/// only while the file still holds what was read (else start over on the newer
/// file). `f` may run several times. A registry that does not parse is never
/// overwritten.
pub fn update_registry<R>(path: &Path, create: bool, mut f: impl FnMut(&mut Doc) -> ApiResult<R>) -> ApiResult<R> {
    for _ in 0..8 {
        let (source, mut doc) = match read_registry(path)? {
            Some(x) => x,
            None if create => (vec![], model::empty_registry()),
            None => return Err(ApiError::not_found(format!("{} does not exist", crate::config::contract_tilde(path)))),
        };
        if model::entities_mut(&mut doc).is_none() {
            return Err(invalid_registry(path, "no entities array"));
        }
        let result = f(&mut doc)?;
        // Stage first, check last: serializing and syncing take a while, and a write
        // that lands meanwhile must be seen by the check.
        let ours = doc.to_pretty();
        let tmp = stage_registry(path, &ours)?;
        #[cfg(test)]
        test_hooks::before_commit(path);
        if commit_registry(&tmp, path, &source, &ours)? {
            return Ok(result);
        }
        // Someone (an agent, a repository checkout) wrote in between: redo on theirs.
    }
    Err(ApiError::conflict("the workspace registry is being changed by someone else right now; try again"))
}

/// Write `bytes` to a synced temp file beside `path` (the registry's mode, else 0600).
fn stage_registry(path: &Path, bytes: &[u8]) -> ApiResult<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = path.parent().ok_or_else(|| ApiError::internal("registry path without a directory"))?;
    std::fs::create_dir_all(dir)?;
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "workspace.json".into());
    let tmp = dir.join(format!(".{name}.wb-tmp-{}", util::random_token(6)));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(mode).open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(tmp)
}

fn read_or_empty(path: &Path) -> std::io::Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// Put the staged `tmp` in place of `path` if `path` still holds `source` (empty:
/// did not exist). `Ok(false)` when someone else wrote first: their file stays, and
/// `tmp` is gone either way.
fn commit_registry(tmp: &Path, path: &Path, source: &[u8], ours: &[u8]) -> ApiResult<bool> {
    let result = (|| -> std::io::Result<bool> {
        // Most concurrent writes are caught here, before anything is swapped.
        if read_or_empty(path)? != source {
            return Ok(false);
        }
        if source.is_empty() {
            // Create only if it still does not exist.
            return match renameat2(tmp, path, libc::RENAME_NOREPLACE) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(e) if unsupported(&e) => std::fs::rename(tmp, path).map(|_| true),
                Err(e) => Err(e),
            };
        }
        swap_if_unchanged(tmp, path, source, ours)
    })();
    match result {
        // `tmp` is gone (renamed), or holds a file that lost: ours, or the registry we
        // replaced after a successful check.
        Ok(done) => {
            let _ = std::fs::remove_file(tmp);
            Ok(done)
        }
        Err(e) => {
            // Half-way through a swap `tmp` may hold someone else's registry: only
            // ever delete our own bytes.
            if read_or_empty(tmp).is_ok_and(|b| b == ours) {
                let _ = std::fs::remove_file(tmp);
            }
            Err(e.into())
        }
    }
}

/// The atomic part of the compare-and-swap: exchange `tmp` and `path`, then check
/// that what came out is `source`. If a write landed after the last check, swap it
/// back (keeping whatever is newest) and report `false`.
fn swap_if_unchanged(tmp: &Path, path: &Path, source: &[u8], ours: &[u8]) -> std::io::Result<bool> {
    match renameat2(tmp, path, libc::RENAME_EXCHANGE) {
        Ok(()) => {}
        // Deleted meanwhile: let the caller reread (and report it missing).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        // A filesystem without RENAME_EXCHANGE: the check above was the last one.
        Err(e) if unsupported(&e) => return std::fs::rename(tmp, path).map(|_| true),
        Err(e) => return Err(e),
    }
    // `tmp` now names the file we replaced.
    let replaced = read_or_empty(tmp);
    if replaced.as_deref().is_ok_and(|b| b == source) {
        return Ok(true);
    }
    // Not what we read (or unreadable): put it back.
    renameat2(tmp, path, libc::RENAME_EXCHANGE)?;
    replaced?;
    if read_or_empty(tmp)? != ours {
        // Yet another write replaced ours in that instant: it is the newest, keep it.
        renameat2(tmp, path, libc::RENAME_EXCHANGE)?;
    }
    Ok(false)
}

fn unsupported(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP))
}

pub(super) fn renameat2(from: &Path, to: &Path, flags: libc::c_uint) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let f = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let t = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both pointers are valid NUL-terminated strings for the call's duration.
    let rc = unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), flags) };
    if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Test-only injection point: run code between staging and committing a registry.
#[cfg(test)]
pub(super) mod test_hooks {
    use std::cell::RefCell;
    use std::path::Path;

    thread_local! {
        static BEFORE_COMMIT: RefCell<Option<Box<dyn FnMut(&Path)>>> = RefCell::new(None);
    }

    pub fn set_before_commit(f: Option<Box<dyn FnMut(&Path)>>) {
        BEFORE_COMMIT.with(|h| *h.borrow_mut() = f);
    }

    pub fn before_commit(path: &Path) {
        BEFORE_COMMIT.with(|h| {
            if let Some(f) = h.borrow_mut().as_mut() {
                f(path)
            }
        });
    }
}

/// A scope's cards, with problems that did not stop the listing.
pub struct ScopeCards {
    pub cards: Vec<Located>,
    pub warnings: Vec<String>,
}

/// Every card of a scope: ours, then the repository's (if any). An unreadable own
/// registry is an error; an unreadable repository registry is a warning.
pub fn load(scope: &Scope) -> ApiResult<ScopeCards> {
    let mut cards = vec![];
    let mut warnings = vec![];
    if let Some((_, doc)) = read_registry(&scope.registry())? {
        let (list, skipped) = Card::all_from(&doc);
        if skipped > 0 {
            warnings.push(format!("{skipped} entr{} in workspace.json without an id or folder", if skipped == 1 { "y" } else { "ies" }));
        }
        cards.extend(list.into_iter().map(|card| Located { card, origin: Origin::Workbench, base: scope.dir.clone(), registry: scope.registry() }));
    }
    if let Some(repo) = scope.repo_dir() {
        let reg = repo.join("workspace.json");
        match read_registry(&reg) {
            Ok(Some((_, doc))) => {
                let (list, skipped) = Card::all_from(&doc);
                if skipped > 0 {
                    warnings.push(format!("{skipped} unusable entr{} in the repository's workspace/workspace.json", if skipped == 1 { "y" } else { "ies" }));
                }
                let mut seen = HashSet::new();
                for card in list {
                    // Ids address cards; a duplicate in the repository file is shown once.
                    if !seen.insert(card.id.clone()) {
                        continue;
                    }
                    if !model::valid_folder(&card.folder) {
                        warnings.push(format!("card {:?} in the repository registry has an unusable folder", card.id));
                        continue;
                    }
                    cards.push(Located { card, origin: Origin::Repo, base: repo.clone(), registry: reg.clone() });
                }
            }
            Ok(None) => {}
            Err(e) => warnings.push(e.message),
        }
    }
    Ok(ScopeCards { cards, warnings })
}

/// Find a card by its public id (`repo:` prefix for repository cards).
pub fn find(scope: &Scope, public_id: &str) -> ApiResult<Located> {
    let loaded = load(scope)?;
    loaded
        .cards
        .into_iter()
        .find(|c| c.public_id() == public_id)
        .ok_or_else(|| ApiError::not_found(format!("no card {public_id:?} in {}", scope.name)))
}

/// Apply `f` to the card's entity inside its registry (CAS), then set `updated`.
fn update_card<R>(loc: &Located, mut f: impl FnMut(&mut Doc) -> ApiResult<R>) -> ApiResult<R> {
    let id = loc.card.id.clone();
    let stamp = match loc.origin {
        Origin::Workbench => model::now_stamp(),
        Origin::Repo => model::local_day(),
    };
    update_registry(&loc.registry, false, |doc| {
        let i = model::find_entity(doc, &id).ok_or_else(|| ApiError::not_found(format!("card {id:?} was removed meanwhile")))?;
        let entity = &mut model::entities_mut(doc).ok_or_else(|| ApiError::internal("registry without entities"))?[i];
        let r = f(entity)?;
        entity.set_after("updated", Doc::str(stamp.clone()), &["created"]);
        Ok(r)
    })
}

/// Mark a card as just touched (its files changed).
pub fn touch(loc: &Located) -> ApiResult<()> {
    update_card(loc, |_| Ok(()))
}

// ---------------------------------------------------------------- card operations

pub struct NewCard {
    pub title: String,
    pub description: String,
    pub category: String,
    pub icon: Option<String>,
}

fn clean_line(s: &str, max: usize, what: &str) -> ApiResult<String> {
    let s = s.trim();
    if s.chars().count() > max {
        return Err(ApiError::bad_request(format!("{what} is longer than {max} characters")));
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(ApiError::bad_request(format!("{what} contains control characters")));
    }
    Ok(s.to_string())
}

fn clean_category(s: &str) -> ApiResult<String> {
    let c = clean_line(s, 40, "category")?.to_lowercase();
    Ok(c.split_whitespace().collect::<Vec<_>>().join("-"))
}

/// Create a card (and its folder) in the scope's own registry. Returns its id.
pub fn create_card(scope: &Scope, input: NewCard) -> ApiResult<String> {
    let title = clean_line(&input.title, 200, "title")?;
    if title.is_empty() {
        return Err(ApiError::bad_request("a card needs a title"));
    }
    let description = clean_line(&input.description, 4000, "description")?;
    let category = clean_category(&input.category)?;
    let icon = match input.icon.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(i) => Some(check_rel(i)?),
        None => None,
    };
    ensure_scope_dir(scope)?;
    let slug = model::card_slug(&title);
    let day = model::local_day();
    let stamp = model::now_stamp();
    let (id, folder) = update_registry(&scope.registry(), true, |doc| {
        let (cards, _) = Card::all_from(doc);
        let ids: HashSet<&str> = cards.iter().map(|c| c.id.as_str()).collect();
        let folders: HashSet<&str> = cards.iter().map(|c| c.folder.as_str()).collect();
        let id = model::unique(&slug, |c| ids.contains(c) || c.starts_with(REPO_PREFIX));
        let folder = model::unique(&format!("{day}_{slug}"), |f| folders.contains(f) || taken(&scope.dir, f));
        let mut fields = vec![
            ("id", Doc::str(&id)),
            ("title", Doc::str(&title)),
            ("description", Doc::str(&description)),
        ];
        if let Some(i) = &icon {
            fields.push(("icon", Doc::str(i)));
        }
        fields.extend([
            ("type", Doc::str("standalone")),
            ("category", Doc::str(&category)),
            ("created", Doc::str(&stamp)),
            ("updated", Doc::str(&stamp)),
            ("folder", Doc::str(&folder)),
            ("steps", Doc::Array(vec![])),
            ("status", Doc::str("active")),
        ]);
        model::entities_mut(doc).ok_or_else(|| ApiError::internal("registry without entities"))?.push(Doc::object(fields));
        Ok((id, folder))
    })?;
    std::fs::create_dir_all(scope.dir.join(&folder))?;
    Ok(id)
}

#[derive(Default)]
pub struct CardPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
    pub pinned: Option<bool>,
    /// `Some(None)` clears it.
    pub default_step: Option<Option<i64>>,
    pub icon: Option<Option<String>>,
}

pub fn patch_card(loc: &Located, p: CardPatch) -> ApiResult<()> {
    if let Some(s) = &p.status {
        if !model::STATUSES.contains(&s.as_str()) {
            return Err(ApiError::bad_request("status must be active, done or archived"));
        }
    }
    let only_state = p.title.is_none() && p.description.is_none() && p.category.is_none() && p.default_step.is_none() && p.icon.is_none();
    if !only_state {
        loc.require_editable()?;
    }
    if p.status.is_none() && p.pinned.is_none() && only_state {
        return Err(ApiError::bad_request("nothing to change"));
    }
    let title = match &p.title {
        Some(t) => {
            let t = clean_line(t, 200, "title")?;
            if t.is_empty() {
                return Err(ApiError::bad_request("a card needs a title"));
            }
            Some(t)
        }
        None => None,
    };
    let description = p.description.as_deref().map(|d| clean_line(d, 4000, "description")).transpose()?;
    let category = p.category.as_deref().map(clean_category).transpose()?;
    let icon = match &p.icon {
        Some(Some(i)) if !i.trim().is_empty() => Some(Some(check_rel(i)?)),
        Some(_) => Some(None),
        None => None,
    };
    if let Some(Some(i)) = p.default_step {
        if i < 0 || i as usize >= loc.card.steps.len() {
            return Err(ApiError::bad_request("defaultStep is not a step of this card"));
        }
    }
    update_card(loc, |e| {
        if let Some(t) = &title {
            e.set("title", Doc::str(t));
        }
        if let Some(d) = &description {
            e.set_after("description", Doc::str(d), &["title"]);
        }
        if let Some(c) = &category {
            e.set_after("category", Doc::str(c), &["type", "description"]);
        }
        if let Some(s) = &p.status {
            e.set("status", Doc::str(s));
        }
        if let Some(pin) = p.pinned {
            e.set_after("pinned", Doc::bool(pin), &["status"]);
        }
        match p.default_step {
            Some(Some(i)) => e.set("defaultStep", Doc::int(i)),
            Some(None) => {
                e.remove("defaultStep");
            }
            None => {}
        }
        match &icon {
            Some(Some(i)) => e.set_after("icon", Doc::str(i), &["description"]),
            Some(None) => {
                e.remove("icon");
            }
            None => {}
        }
        Ok(())
    })
}

/// Remove a card from its registry and move its folder to the Workspace trash
/// (`<trash>/<scope>/<folder>~<time>/`, with the entry in `.card.json` and
/// `.trash.json`; see `trash`). A folder another card still uses stays where it is:
/// the item then holds just the entry, and restoring puts the card back on it.
/// Returns the trash item's name.
pub fn delete_card(trash: &Path, scope: &Scope, loc: &Located) -> ApiResult<String> {
    loc.require_editable()?;
    let id = loc.card.id.clone();
    let folder = loc.card.folder.clone();
    let (removed, shared) = update_registry(&loc.registry, false, |doc| {
        let i = model::find_entity(doc, &id).ok_or_else(|| ApiError::not_found(format!("no card {id:?}")))?;
        let list = model::entities_mut(doc).ok_or_else(|| ApiError::internal("registry without entities"))?;
        let removed = list.remove(i);
        let shared = list.iter().any(|e| e.get("folder").and_then(Doc::as_str) == Some(folder.as_str()));
        Ok((removed, shared))
    })?;
    let dir = loc.dir()?;
    let moved = !shared && dir.is_dir();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest_parent = trash.join(&scope.id);
    std::fs::create_dir_all(&dest_parent)?;
    util::fs::set_mode(trash, 0o700);
    let dest = dest_parent.join(model::unique(&format!("{folder}~{stamp}"), |n| taken(&dest_parent, n)));
    if moved {
        std::fs::rename(&dir, &dest)?;
    } else {
        std::fs::create_dir(&dest)?;
    }
    // The entry travels with the files, so the card can be put back.
    let _ = util::fs::write_atomic(&dest.join(super::trash::CARD_FILE), &removed.to_pretty(), 0o600);
    let meta = super::trash::Meta { deleted_at: util::now_ms(), moved };
    let _ = util::fs::write_json(&dest.join(super::trash::META_FILE), &meta);
    Ok(dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

// ---------------------------------------------------------------- steps

pub fn check_viewer(v: Option<&str>) -> ApiResult<Option<String>> {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        None | Some("auto") => Ok(None),
        Some(v) if model::VIEWERS.contains(&v) => Ok(Some(v.to_string())),
        Some(v) => Err(ApiError::bad_request(format!("unknown viewer {v:?}; use one of {}", model::VIEWERS.join(", ")))),
    }
}

pub(super) fn step_doc(name: &str, path: &str, viewer: Option<&str>) -> Doc {
    let mut fields = vec![("name", Doc::str(name)), ("path", Doc::str(path))];
    if let Some(v) = viewer {
        fields.push(("viewer", Doc::str(v)));
    }
    Doc::object(fields)
}

fn steps_of(e: &mut Doc) -> &mut Vec<Doc> {
    if !matches!(e.get("steps"), Some(Doc::Array(_))) {
        e.set_after("steps", Doc::Array(vec![]), &["folder"]);
    }
    match e.get_mut("steps") {
        Some(Doc::Array(a)) => a,
        _ => unreachable!("steps was just set to an array"),
    }
}

/// Add a step (path relative to the card folder). Returns its index.
pub fn add_step(loc: &Located, name: &str, rel: &str, viewer: Option<&str>) -> ApiResult<usize> {
    loc.require_editable()?;
    let rel = check_rel(rel)?;
    if rel.is_empty() {
        return Err(ApiError::bad_request("a step needs a file or folder inside the card"));
    }
    let name = clean_line(name, 120, "step name")?;
    let name = if name.is_empty() { rel.rsplit('/').next().unwrap_or(&rel).to_string() } else { name };
    update_card(loc, |e| {
        let steps = steps_of(e);
        steps.push(step_doc(&name, &rel, viewer));
        Ok(steps.len() - 1)
    })
}

#[derive(Default)]
pub struct StepPatch {
    pub name: Option<String>,
    /// `Some(None)` = back to auto.
    pub viewer: Option<Option<String>>,
    pub position: Option<usize>,
}

fn check_step<'a>(steps: &'a mut [Doc], index: usize, expect: Option<&str>) -> ApiResult<&'a mut Doc> {
    let len = steps.len();
    let step = steps.get_mut(index).ok_or_else(|| ApiError::not_found(format!("no step {index} (the card has {len})")))?;
    if let Some(expect) = expect {
        if step.get("path").and_then(Doc::as_str) != Some(expect) {
            return Err(ApiError::conflict("the card's steps changed meanwhile; reload and try again"));
        }
    }
    Ok(step)
}

/// Where the step at `i` ends up after moving the step at `from` to `to`.
fn moved_index(i: usize, from: usize, to: usize) -> usize {
    if i == from {
        to
    } else if from < to && i > from && i <= to {
        i - 1
    } else if to < from && i >= to && i < from {
        i + 1
    } else {
        i
    }
}

fn default_of(e: &Doc) -> Option<usize> {
    e.get("defaultStep").and_then(Doc::as_i64).and_then(|i| usize::try_from(i).ok())
}

pub fn patch_step(loc: &Located, index: usize, expect: Option<&str>, p: StepPatch) -> ApiResult<()> {
    loc.require_editable()?;
    let name = p.name.as_deref().map(|n| clean_line(n, 120, "step name")).transpose()?;
    if name.as_deref() == Some("") {
        return Err(ApiError::bad_request("a step needs a name"));
    }
    update_card(loc, |e| {
        let default = default_of(e);
        let steps = steps_of(e);
        let step = check_step(steps, index, expect)?;
        if let Some(n) = &name {
            step.set("name", Doc::str(n));
        }
        match &p.viewer {
            Some(Some(v)) => step.set("viewer", Doc::str(v)),
            Some(None) => {
                step.remove("viewer");
            }
            None => {}
        }
        if let Some(to) = p.position {
            let to = to.min(steps.len() - 1);
            let item = steps.remove(index);
            steps.insert(to, item);
            // The default tab stays on the same step.
            if let Some(d) = default {
                e.set("defaultStep", Doc::int(moved_index(d, index, to) as i64));
            }
        }
        Ok(())
    })
}

pub fn delete_step(loc: &Located, index: usize, expect: Option<&str>) -> ApiResult<()> {
    loc.require_editable()?;
    update_card(loc, |e| {
        let default = default_of(e);
        let steps = steps_of(e);
        check_step(steps, index, expect)?;
        steps.remove(index);
        match default {
            Some(d) if d == index => {
                e.remove("defaultStep");
            }
            Some(d) if d > index => e.set("defaultStep", Doc::int(d as i64 - 1)),
            _ => {}
        }
        Ok(())
    })
}

// ---------------------------------------------------------------- paths inside a card

/// Names never served, listed or imported: dotfiles and common credential files.
pub fn is_private_name(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    name.starts_with('.')
        || l == "node_modules"
        || l == "auth.json"
        || l == "token.json"
        || l == "tokens.json"
        || (l.starts_with("oauth_") && l.ends_with(".json"))
        || (l.contains("secret") && l.ends_with(".json"))
        || (l.starts_with("credentials") && l.ends_with(".json"))
        || [".pem", ".key", ".p12", ".pfx", "_token", ".token", "_api_key", ".api_key"].iter().any(|s| l.ends_with(s))
        || ["id_rsa", "id_ecdsa", "id_ed25519", "id_dsa"].iter().any(|s| l.starts_with(s))
}

/// Normalize a path relative to a card folder: `/` separators, no `..`, no absolute
/// paths, no private segments. `""` is the folder itself.
pub fn check_rel(rel: &str) -> ApiResult<String> {
    if rel.contains('\0') || rel.contains('\\') {
        return Err(ApiError::bad_request("unusable characters in the path"));
    }
    if rel.starts_with('/') || rel.starts_with('~') {
        return Err(ApiError::bad_request("expected a path relative to the card folder"));
    }
    let mut parts = vec![];
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(ApiError::forbidden("the path leaves the card folder")),
            s if is_private_name(s) => return Err(ApiError::forbidden(format!("{s:?} is private: dotfiles and credential files are not served"))),
            s => parts.push(s),
        }
    }
    Ok(parts.join("/"))
}

/// Resolve `rel` inside the card folder (symlinks leaving it are refused).
pub fn resolve_in_card(dir: &Path, rel: &str) -> ApiResult<(PathBuf, String)> {
    let rel = check_rel(rel)?;
    Ok((util::paths::resolve_in_root(dir, &rel)?, rel))
}

/// Where a step's file comes from.
#[derive(Debug, PartialEq)]
pub enum StepSource {
    /// Already inside the card folder (relative path).
    Inside(String),
    /// Elsewhere in an allowed root: copy it in first.
    Import(PathBuf),
}

/// Classify a step path: relative to the card folder, or absolute inside the card
/// folder or one of `import_roots` (project roots). Anything else is refused.
pub fn classify_step_path(card_dir: &Path, input: &str, import_roots: &[PathBuf]) -> ApiResult<StepSource> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ApiError::bad_request("a step needs a path"));
    }
    if !(input.starts_with('/') || input.starts_with("~/")) {
        return Ok(StepSource::Inside(check_rel(input)?));
    }
    let abs = crate::config::expand_tilde(input);
    let canon = abs.canonicalize().map_err(|_| ApiError::not_found(format!("{input} does not exist")))?;
    if let Ok(dir) = card_dir.canonicalize() {
        if let Ok(rest) = canon.strip_prefix(&dir) {
            return Ok(StepSource::Inside(check_rel(&rest.to_string_lossy())?));
        }
    }
    for root in import_roots {
        let Ok(root) = root.canonicalize() else { continue };
        if let Ok(rest) = canon.strip_prefix(&root) {
            check_rel(&rest.to_string_lossy())?;
            if rest.as_os_str().is_empty() {
                return Err(ApiError::bad_request("cannot add a whole project as a step"));
            }
            return Ok(StepSource::Import(canon));
        }
    }
    Err(ApiError::forbidden("the path is outside the card folder and the project"))
}

/// Whether `dir/name` is any directory entry: a dangling symlink is taken too (an
/// agent can plant one in a card folder that points anywhere).
pub(super) fn taken(dir: &Path, name: &str) -> bool {
    std::fs::symlink_metadata(dir.join(name)).is_ok()
}

/// A name for `wanted` in `dir` that is not taken: `name.ext`, `name (2).ext`…
fn free_name(dir: &Path, wanted: &str) -> String {
    if !taken(dir, wanted) {
        return wanted.to_string();
    }
    let (stem, ext) = match wanted.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (wanted, String::new()),
    };
    (2..10_000)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|c| !taken(dir, c))
        .unwrap_or_else(|| format!("{stem}-{}{ext}", util::random_token(4)))
}

/// Copy a regular file to a path that must not exist yet. Never writes through an
/// existing entry (a symlink planted in the card folder, a file that appeared
/// meanwhile: `AlreadyExists`). Keeps the permission bits, like `fs::copy`.
fn copy_new_file(src: &Path, dest: &Path) -> std::io::Result<u64> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut from = std::fs::File::open(src)?;
    let mode = from.metadata()?.permissions().mode() & 0o777;
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(dest)?;
    let copied = std::io::copy(&mut from, &mut to).and_then(|n| to.set_permissions(std::fs::Permissions::from_mode(mode)).map(|_| n));
    if copied.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    copied
}

/// A file name a client may give an upload: one component, not private.
pub fn check_file_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(ApiError::bad_request("unusable file name"));
    }
    if is_private_name(name) {
        return Err(ApiError::forbidden("dotfiles and credential files cannot be added to a card"));
    }
    Ok(name.to_string())
}

/// Copy a file or directory into the card folder under a free name. Returns the
/// relative path. Directories skip private names and symlinks, within caps.
pub fn import_into(card_dir: &Path, src: &Path) -> ApiResult<String> {
    std::fs::create_dir_all(card_dir)?;
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| ApiError::bad_request("nothing to copy"))?;
    let wanted = check_file_name(&name)?;
    let md = std::fs::metadata(src)?;
    if !md.is_file() && !md.is_dir() {
        return Err(ApiError::bad_request("only files and folders can be added"));
    }
    for _ in 0..20 {
        let name = free_name(card_dir, &wanted);
        let dest = card_dir.join(&name);
        if md.is_file() {
            match copy_new_file(src, &dest) {
                Ok(_) => return Ok(name),
                // Taken meanwhile: pick another name.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        // `create_dir` (not `_all`) fails on anything already there, a symlink included.
        match std::fs::create_dir(&dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
        let mut files = 0usize;
        let mut bytes = 0u64;
        let result = copy_dir(src, &dest, &mut files, &mut bytes);
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        return result.map(|_| name);
    }
    Err(ApiError::conflict("could not find a free name in the card folder"))
}

/// Copy the contents of `src` into the (new, empty) directory `dest`.
fn copy_dir(src: &Path, dest: &Path, files: &mut usize, bytes: &mut u64) -> ApiResult<()> {
    for entry in std::fs::read_dir(src)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(ft) = entry.file_type() else { continue };
        if is_private_name(&name) || ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            std::fs::create_dir(dest.join(&name))?;
            copy_dir(&entry.path(), &dest.join(&name), files, bytes)?;
        } else if ft.is_file() {
            *files += 1;
            *bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            if *files > MAX_IMPORT_FILES || *bytes > MAX_IMPORT_BYTES {
                return Err(ApiError::bad_request(format!(
                    "the folder is too large to copy into a card (over {MAX_IMPORT_FILES} files or {} GB)",
                    MAX_IMPORT_BYTES >> 30
                )));
            }
            copy_new_file(&entry.path(), &dest.join(&name))?;
        }
    }
    Ok(())
}

/// Move a finished upload (a temp file in the card folder) to a free name in `sub`.
pub fn finish_upload(card_dir: &Path, tmp: &Path, sub: &str, name: &str) -> ApiResult<String> {
    let (dir, sub) = resolve_in_card(card_dir, sub)?;
    std::fs::create_dir_all(&dir)?;
    let name = check_file_name(name)?;
    for _ in 0..20 {
        let final_name = free_name(&dir, &name);
        let dest = dir.join(&final_name);
        // link + unlink: never replaces a file that appeared meanwhile.
        match std::fs::hard_link(tmp, &dest) {
            Ok(()) => {
                let _ = std::fs::remove_file(tmp);
                return Ok(if sub.is_empty() { final_name } else { format!("{sub}/{final_name}") });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::conflict("could not find a free file name"))
}

// ---------------------------------------------------------------- listing and content

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// Relative to the card folder.
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub kind: &'static str,
}

pub fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One page of a directory listing.
pub struct Listing {
    pub entries: Vec<FileEntry>,
    /// Entries after this page (or the directory was too large to read whole).
    pub truncated: bool,
    /// Entries in the directory (as far as it was read).
    pub total: usize,
}

/// One directory of a card folder: folders first, then files, by name.
pub fn list_files(card_dir: &Path, sub: &str) -> ApiResult<(Vec<FileEntry>, bool)> {
    let l = list_files_page(card_dir, sub, 0)?;
    Ok((l.entries, l.truncated))
}

/// [`list_files`] from `offset`, at most [`MAX_LIST`] entries. Every name is read and
/// sorted before anything is cut, so a large folder lists its first entries in order
/// (not whatever the directory happens to return first).
pub fn list_files_page(card_dir: &Path, sub: &str, offset: usize) -> ApiResult<Listing> {
    let (dir, sub) = resolve_in_card(card_dir, sub)?;
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && sub.is_empty() => return Ok(Listing { entries: vec![], truncated: false, total: 0 }),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => return Err(ApiError::bad_request(format!("{sub} is not a folder"))),
        Err(e) => return Err(e.into()),
    };
    let rel_of = |name: &str| if sub.is_empty() { name.to_string() } else { format!("{sub}/{name}") };
    // Names and whether each is a folder: the entry's type is free (no stat), except
    // for symlinks, which are followed only when they stay inside the card folder.
    let mut names: Vec<(bool, String)> = vec![];
    let mut cut = false;
    for entry in rd.flatten() {
        if names.len() >= MAX_SCAN {
            cut = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_private_name(&name) {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let is_dir = if ft.is_symlink() {
            let Ok(abs) = util::paths::resolve_in_root(card_dir, &rel_of(&name)) else { continue };
            match std::fs::metadata(&abs) {
                Ok(md) => md.is_dir(),
                Err(_) => continue,
            }
        } else {
            ft.is_dir()
        };
        names.push((is_dir, name));
    }
    names.sort_by_cached_key(|(is_dir, name)| (!*is_dir, name_key(name)));
    let total = names.len();
    let mut out = vec![];
    let mut rest = names.into_iter().skip(offset);
    for (_, name) in rest.by_ref() {
        let path = rel_of(&name);
        let Ok(abs) = util::paths::resolve_in_root(card_dir, &path) else { continue };
        let Ok(md) = std::fs::metadata(&abs) else { continue };
        let dir = md.is_dir();
        out.push(FileEntry { kind: if dir { "dir" } else { model::kind_for_path(&name) }, name, path, dir, size: if dir { 0 } else { md.len() }, mtime: mtime_ms(&md) });
        if out.len() >= MAX_LIST {
            break;
        }
    }
    Ok(Listing { entries: out, truncated: cut || rest.next().is_some(), total })
}

/// Natural order of the stem, then the extension: `r.md` < `r (2).md`, `img2` < `img10`.
fn name_key(name: &str) -> (Vec<(u8, u64, String)>, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (natural_key(stem), ext.to_lowercase()),
        _ => (natural_key(name), String::new()),
    }
}

/// Sort key that orders `img2` before `img10`.
fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut out = vec![];
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                n.push(d);
                chars.next();
            }
            out.push((0, n.parse().unwrap_or(u64::MAX), String::new()));
        } else {
            let mut t = String::new();
            while let Some(&d) = chars.peek().filter(|d| !d.is_ascii_digit()) {
                t.extend(d.to_lowercase());
                chars.next();
            }
            out.push((1, 0, t));
        }
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub path: String,
    pub text: String,
    /// sha256 of the bytes on disk; send it back when saving.
    pub revision: String,
    pub size: u64,
    pub editable: bool,
}

pub fn is_markdown(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    l.ends_with(".md") || l.ends_with(".markdown")
}

/// A text file of the card (markdown, JSON, logs…), at most [`MAX_TEXT_BYTES`].
pub fn read_text(card_dir: &Path, rel: &str, card_editable: bool) -> ApiResult<TextContent> {
    let (abs, rel) = resolve_in_card(card_dir, rel)?;
    let md = std::fs::metadata(&abs)?;
    if !md.is_file() {
        return Err(ApiError::bad_request("not a file"));
    }
    if md.len() > MAX_TEXT_BYTES {
        return Err(ApiError::bad_request(format!("the file is larger than {} MB; open it raw instead", MAX_TEXT_BYTES >> 20)));
    }
    let mut bytes = Vec::with_capacity(md.len() as usize);
    std::fs::File::open(&abs)?.take(MAX_TEXT_BYTES + 1).read_to_end(&mut bytes)?;
    let revision = sha256_hex(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| ApiError::bad_request("not a UTF-8 text file"))?;
    Ok(TextContent { editable: card_editable && is_markdown(&rel), path: rel, size: md.len(), text, revision })
}

/// Save markdown if the file still has `revision` (409 otherwise, unless `force`).
/// The previous version is kept in `backups` (the newest [`MAX_BACKUPS`]).
pub fn write_markdown(card_dir: &Path, rel: &str, text: &str, revision: Option<&str>, force: bool, backups: &Path) -> ApiResult<(String, String, u64)> {
    let (abs, rel) = resolve_in_card(card_dir, rel)?;
    if !is_markdown(&rel) {
        return Err(ApiError::bad_request("only markdown files can be edited here"));
    }
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err(ApiError::bad_request(format!("markdown is limited to {} MB", MAX_TEXT_BYTES >> 20)));
    }
    let current = match std::fs::read(&abs) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if !force {
        let on_disk = current.as_deref().map(sha256_hex);
        if on_disk.as_deref() != revision.filter(|r| !r.is_empty()) {
            return Err(ApiError::conflict("the file changed on disk since you opened it; your draft is kept"));
        }
    }
    if let Some(old) = &current {
        std::fs::create_dir_all(backups)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
        let flat = rel.replace('/', "__");
        util::fs::write_atomic(&backups.join(format!("{stamp}-{}-{flat}", util::random_token(3))), old, 0o600)?;
        prune_backups(backups);
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    util::fs::write_atomic(&abs, text.as_bytes(), 0o644)?;
    Ok((rel, sha256_hex(text.as_bytes()), text.len() as u64))
}

fn prune_backups(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    if names.len() <= MAX_BACKUPS {
        return;
    }
    names.sort();
    for p in &names[..names.len() - MAX_BACKUPS] {
        let _ = std::fs::remove_file(p);
    }
}

// ---------------------------------------------------------------- output

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOut {
    pub index: usize,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<String>,
    /// What the UI shows: html, markdown, image, gallery, compare3d, pdf, video, audio, text or file.
    pub kind: &'static str,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Changes when the file does (the UI reloads the view).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardOut {
    pub id: String,
    pub scope: String,
    pub scope_name: String,
    pub origin: Origin,
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub category: String,
    pub created: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    pub folder: String,
    /// Absolute path of the card folder (agents write files there).
    pub folder_path: String,
    pub steps: Vec<StepOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_step: Option<i64>,
    /// The tab to open by default (`defaultStep`, else the last step).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_index: Option<usize>,
    pub status: String,
    pub pinned: bool,
    pub sample: bool,
    /// Explicitly archived, or not touched for 7 days (unless pinned or a sample).
    pub archived: bool,
    /// `updated ?? created`, in ms.
    pub touched_at: i64,
    /// Everything but status and pin can be changed (not a repository card).
    pub editable: bool,
    /// Capability URL prefix for the card's files: `/view/<grant>/<folder>/`.
    pub base: String,
    /// When `base` stops working (ms); fetch the card again before then.
    pub grant_expires_at: i64,
    /// Relative path of a picture for the card (icon, first image, first report image).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

fn step_out(dir: Option<&Path>, index: usize, s: &model::Step) -> StepOut {
    let md = dir.and_then(|d| resolve_in_card(d, &s.path).ok()).filter(|(_, rel)| !rel.is_empty()).and_then(|(abs, _)| std::fs::metadata(abs).ok());
    let is_dir = md.as_ref().is_some_and(|m| m.is_dir());
    StepOut {
        index,
        name: s.name.clone(),
        path: s.path.clone(),
        viewer: s.viewer.clone(),
        kind: model::step_kind(s.viewer.as_deref(), &s.path, is_dir),
        exists: md.is_some(),
        size: md.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
        mtime: md.as_ref().map(mtime_ms),
    }
}

const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".gif", ".avif", ".svg", ".bmp"];

fn is_image(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    IMAGE_EXTS.iter().any(|e| l.ends_with(e))
}

/// The first local `<img src>` of an HTML report (first 64 KiB), relative to the card.
fn first_report_image(dir: &Path, rel: &str) -> Option<String> {
    use std::sync::LazyLock;
    static IMG: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"(?is)<img\b[^>]*?\bsrc\s*=\s*["']([^"'<>]+)["']"#).expect("static regex"));
    let (abs, _) = resolve_in_card(dir, rel).ok()?;
    let mut head = vec![];
    std::fs::File::open(abs).ok()?.take(64 * 1024).read_to_end(&mut head).ok()?;
    let text = String::from_utf8_lossy(&head);
    let base = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    IMG.captures_iter(&text).take(12).find_map(|c| {
        let src = c.get(1)?.as_str().trim();
        if src.contains(':') || src.starts_with('/') || src.starts_with("//") || !is_image(src.split(['?', '#']).next()?) {
            return None;
        }
        let src = percent_encoding::percent_decode_str(src.split(['?', '#']).next()?).decode_utf8().ok()?;
        let joined = if base.is_empty() { src.to_string() } else { format!("{base}/{src}") };
        let (abs, rel) = resolve_in_card(dir, &joined).ok()?;
        abs.is_file().then_some(rel)
    })
}

/// Pick a picture for the card: its icon, else from the step that opens by default,
/// else any image step, gallery (its first image) or HTML report (its first local image).
fn thumbnail(state: &AppState, dir: &Path, card: &Card, steps: &[StepOut]) -> Option<String> {
    if let Some(icon) = &card.icon {
        if let Ok((abs, rel)) = resolve_in_card(dir, icon) {
            if abs.is_file() {
                return Some(rel);
            }
        }
    }
    let from_step = |s: &StepOut| -> Option<String> {
        if !s.exists {
            return None;
        }
        match s.kind {
            "image" => Some(s.path.clone()),
            "gallery" => first_image_in(dir, &s.path, 1),
            "html" => report_image(state, dir, s),
            _ => None,
        }
    };
    if let Some(found) = card.default_index().and_then(|i| from_step(&steps[i])) {
        return Some(found);
    }
    ["image", "gallery", "html"].iter().find_map(|kind| steps.iter().filter(|s| s.kind == *kind).find_map(from_step))
}

/// The first image of a gallery folder, looking `depth` levels into subfolders.
fn first_image_in(dir: &Path, sub: &str, depth: usize) -> Option<String> {
    let (entries, _) = list_files(dir, sub).ok()?;
    if let Some(f) = entries.iter().find(|f| !f.dir && is_image(&f.name)) {
        return Some(f.path.clone());
    }
    if depth == 0 {
        return None;
    }
    entries.iter().filter(|f| f.dir).take(8).find_map(|f| first_image_in(dir, &f.path, depth - 1))
}

/// [`first_report_image`], cached by the report's mtime and size.
fn report_image(state: &AppState, dir: &Path, html: &StepOut) -> Option<String> {
    let key = dir.join(&html.path);
    let stamp = (html.mtime.unwrap_or(0), html.size.unwrap_or(0));
    if let Some((k, v)) = state.workspace.thumbs.lock().get(&key) {
        if *k == stamp {
            return v.clone();
        }
    }
    let found = first_report_image(dir, &html.path);
    let mut cache = state.workspace.thumbs.lock();
    if cache.len() > 4096 {
        cache.clear();
    }
    cache.insert(key, (stamp, found.clone()));
    found
}

/// Everything but RFC 3986's unreserved characters.
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Percent-encode one path segment for a URL.
pub fn encode_segment(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, SEGMENT).to_string()
}

pub fn card_out(state: &AppState, scope: &Scope, loc: &Located, cutoff: i64) -> CardOut {
    let dir = loc.dir().ok();
    let steps: Vec<StepOut> = loc.card.steps.iter().enumerate().map(|(i, s)| step_out(dir.as_deref(), i, s)).collect();
    let thumb = dir.as_deref().and_then(|d| thumbnail(state, d, &loc.card, &steps));
    let (token, grant_expires_at) = state.workspace.grants.mint(&loc.base, &loc.card.folder);
    let c = &loc.card;
    CardOut {
        id: loc.public_id(),
        scope: scope.id.clone(),
        scope_name: scope.name.clone(),
        origin: loc.origin,
        title: c.title.clone(),
        description: c.description.clone(),
        icon: c.icon.clone(),
        kind: c.kind.clone(),
        category: c.category.clone(),
        created: c.created.clone(),
        updated: c.updated.clone(),
        folder: c.folder.clone(),
        folder_path: dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
        default_step: c.default_step,
        default_index: c.default_index(),
        status: c.status.clone(),
        pinned: c.pinned,
        sample: c.sample,
        archived: c.archived(cutoff),
        touched_at: c.touched_ms(),
        editable: loc.editable(),
        base: format!("/view/{token}/{}/", encode_segment(&c.folder)),
        grant_expires_at,
        thumb,
        steps,
    }
}

/// Cards of a scope, sorted (pinned, then freshest).
pub fn scope_cards(state: &AppState, scope: &Scope) -> ApiResult<(Vec<CardOut>, Vec<String>)> {
    let mut loaded = load(scope)?;
    loaded.cards.sort_by(|a, b| model::compare_cards(&a.card, &b.card));
    let cutoff = model::archive_cutoff_ms();
    Ok((loaded.cards.iter().map(|c| card_out(state, scope, c, cutoff)).collect(), loaded.warnings))
}

pub fn one_card(state: &AppState, scope: &Scope, public_id: &str) -> ApiResult<CardOut> {
    let loc = find(scope, public_id)?;
    Ok(card_out(state, scope, &loc, model::archive_cutoff_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn located(dir: &Path, origin: Origin, registry: &str) -> Located {
        std::fs::write(dir.join("workspace.json"), registry).unwrap();
        let (_, doc) = read_registry(&dir.join("workspace.json")).unwrap().unwrap();
        let (cards, _) = Card::all_from(&doc);
        Located { card: cards[0].clone(), origin, base: dir.to_path_buf(), registry: dir.join("workspace.json") }
    }

    const ONE: &str = r#"{"entities":[{"id":"a","title":"A","description":"","type":"group","category":"x","created":"2026-09-15","folder":"2026-09-15_a","steps":[{"name":"One","path":"one.html"},{"name":"Two","path":"two.md","extra":true}],"status":"active","pinned":false,"sample":true,"defaultStep":1,"mine":{"z":1,"y":2}}],"other":1}"#;

    #[test]
    fn cas_retries_when_someone_writes_in_between() {
        let dir = tempfile::tempdir().unwrap();
        let reg = dir.path().join("workspace.json");
        std::fs::write(&reg, ONE).unwrap();
        let mut first = true;
        update_registry(&reg, false, |doc| {
            if first {
                first = false;
                // An agent rewrites the file while we are working.
                let mut theirs = Doc::parse(ONE.as_bytes()).unwrap();
                model::entities_mut(&mut theirs).unwrap()[0].set("title", Doc::str("Theirs"));
                std::fs::write(&reg, theirs.to_pretty()).unwrap();
            }
            model::entities_mut(doc).unwrap()[0].set("status", Doc::str("done"));
            Ok(())
        })
        .unwrap();
        let text = std::fs::read_to_string(&reg).unwrap();
        assert!(text.contains("\"Theirs\""), "their change survived: {text}");
        assert!(text.contains("\"status\": \"done\""));
        assert!(text.contains("\"mine\"") && text.contains("\"other\": 1"), "unknown keys kept");
    }

    /// Their write lands while ours is being staged (serialized, written, synced):
    /// the check that follows must see it.
    #[test]
    fn cas_sees_a_write_made_from_another_thread_while_staging() {
        let dir = tempfile::tempdir().unwrap();
        let reg = dir.path().join("workspace.json");
        std::fs::write(&reg, ONE).unwrap();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let theirs_path = reg.clone();
        let writer = std::thread::spawn(move || {
            wait.recv().unwrap();
            // An agent's atomic save: temp file, then rename over the registry.
            let mut theirs = Doc::parse(ONE.as_bytes()).unwrap();
            model::entities_mut(&mut theirs).unwrap()[0].set("title", Doc::str("Theirs"));
            let tmp = theirs_path.with_extension("agent-tmp");
            std::fs::write(&tmp, theirs.to_pretty()).unwrap();
            std::fs::rename(&tmp, &theirs_path).unwrap();
            done.send(()).unwrap();
        });
        let mut first = true;
        test_hooks::set_before_commit(Some(Box::new(move |_| {
            if std::mem::take(&mut first) {
                go.send(()).unwrap();
                finished.recv().unwrap();
            }
        })));
        let mut runs = 0;
        update_registry(&reg, false, |doc| {
            runs += 1;
            model::entities_mut(doc).unwrap()[0].set("status", Doc::str("done"));
            Ok(())
        })
        .unwrap();
        test_hooks::set_before_commit(None);
        writer.join().unwrap();
        let text = std::fs::read_to_string(&reg).unwrap();
        assert!(text.contains("\"Theirs\""), "their change survived: {text}");
        assert!(text.contains("\"status\": \"done\""), "ours was redone on theirs");
        assert_eq!(runs, 2);
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["workspace.json"], "no temp files left");
    }

    /// The swap itself: a write that slipped in after the last check is put back.
    #[test]
    fn swap_puts_back_a_registry_it_displaced() {
        let dir = tempfile::tempdir().unwrap();
        let reg = dir.path().join("workspace.json");
        std::fs::write(&reg, "theirs").unwrap();
        let tmp = dir.path().join(".workspace.json.wb-tmp-x");
        std::fs::write(&tmp, "ours").unwrap();
        // We based ours on "source", but "theirs" is there now.
        assert!(!swap_if_unchanged(&tmp, &reg, b"source", b"ours").unwrap());
        assert_eq!(std::fs::read_to_string(&reg).unwrap(), "theirs");
        assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "ours");
        // Based on what is there: swapped in, and the old file is what `tmp` names.
        assert!(swap_if_unchanged(&tmp, &reg, b"theirs", b"ours").unwrap());
        assert_eq!(std::fs::read_to_string(&reg).unwrap(), "ours");
        // A registry that appeared meanwhile is never replaced by a create.
        let fresh = dir.path().join("new.json");
        std::fs::write(&fresh, "someone's").unwrap();
        let staged = stage_registry(&fresh, b"mine").unwrap();
        assert!(!commit_registry(&staged, &fresh, b"", b"mine").unwrap());
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "someone's");
        assert!(!staged.exists());
    }

    #[test]
    fn broken_registries_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let reg = dir.path().join("workspace.json");
        std::fs::write(&reg, "{ not json").unwrap();
        let err = update_registry(&reg, true, |_| Ok(())).unwrap_err();
        assert_eq!(err.code, "invalid_registry");
        assert_eq!(std::fs::read_to_string(&reg).unwrap(), "{ not json");
        // Missing + create starts an empty registry.
        let fresh = dir.path().join("new.json");
        update_registry(&fresh, true, |doc| {
            model::entities_mut(doc).unwrap().push(Doc::object(vec![("id", Doc::str("x"))]));
            Ok(())
        })
        .unwrap();
        assert!(std::fs::read_to_string(&fresh).unwrap().contains("\"id\": \"x\""));
    }

    #[test]
    fn repository_cards_only_change_status_and_pin() {
        let dir = tempfile::tempdir().unwrap();
        let loc = located(dir.path(), Origin::Repo, ONE);
        assert_eq!(loc.public_id(), "repo:a");
        let err = patch_card(&loc, CardPatch { title: Some("New".into()), ..Default::default() }).unwrap_err();
        assert_eq!(err.code, "forbidden");
        assert_eq!(add_step(&loc, "x", "x.md", None).unwrap_err().code, "forbidden");
        patch_card(&loc, CardPatch { status: Some("done".into()), pinned: Some(true), ..Default::default() }).unwrap();
        let text = std::fs::read_to_string(dir.path().join("workspace.json")).unwrap();
        assert!(text.contains("\"status\": \"done\"") && text.contains("\"pinned\": true"));
        // Mr. Mak's own format: a local day, placed after `created`.
        assert!(text.contains(&format!("\"updated\": \"{}\"", model::local_day())));
        assert!(text.find("\"created\"").unwrap() < text.find("\"updated\"").unwrap());
        assert!(text.find("\"updated\"").unwrap() < text.find("\"folder\"").unwrap());
        assert!(patch_card(&loc, CardPatch { status: Some("gone".into()), ..Default::default() }).is_err());
    }

    #[test]
    fn steps_move_and_the_default_follows() {
        let dir = tempfile::tempdir().unwrap();
        let loc = located(dir.path(), Origin::Workbench, ONE);
        let reload = || {
            let (_, doc) = read_registry(&dir.path().join("workspace.json")).unwrap().unwrap();
            Card::all_from(&doc).0.remove(0)
        };
        // Default is "Two" (index 1). Move it to the front.
        patch_step(&loc, 1, Some("two.md"), StepPatch { position: Some(0), ..Default::default() }).unwrap();
        let c = reload();
        assert_eq!(c.steps[0].path, "two.md");
        assert_eq!(c.default_step, Some(0));
        // A stale expectation is a conflict.
        assert_eq!(delete_step(&loc, 0, Some("one.html")).unwrap_err().code, "conflict");
        let i = add_step(&loc, "", "shots/", Some("gallery")).unwrap();
        assert_eq!(i, 2);
        let c = reload();
        assert_eq!(c.steps[2].name, "shots");
        assert_eq!(c.steps[2].viewer.as_deref(), Some("gallery"));
        delete_step(&loc, 0, Some("two.md")).unwrap();
        let c = reload();
        assert_eq!(c.default_step, None, "the default step was deleted");
        assert_eq!(c.steps.len(), 2);
        let text = std::fs::read_to_string(dir.path().join("workspace.json")).unwrap();
        assert!(text.contains("\"mine\""));
    }

    #[test]
    fn create_and_delete_cards() {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope { id: "p".into(), name: "P".into(), dir: dir.path().join("p"), project: None };
        let id = create_card(&scope, NewCard { title: "Load report".into(), description: "d".into(), category: "Research Notes".into(), icon: None }).unwrap();
        let id2 = create_card(&scope, NewCard { title: "Load report".into(), description: String::new(), category: String::new(), icon: None }).unwrap();
        assert_eq!((id.as_str(), id2.as_str()), ("load-report", "load-report-2"));
        assert!(scope.dir.join("_shared/report.css").is_file());
        let loc = find(&scope, "load-report").unwrap();
        assert_eq!(loc.card.category, "research-notes");
        let folder = loc.dir().unwrap();
        assert!(folder.is_dir() && loc.card.folder.ends_with("_load-report"));
        std::fs::write(folder.join("r.html"), "<p>x</p>").unwrap();
        let trash = dir.path().join("trash");
        delete_card(&trash, &scope, &loc).unwrap();
        assert!(!folder.exists());
        let moved: Vec<_> = std::fs::read_dir(trash.join("p")).unwrap().flatten().collect();
        assert_eq!(moved.len(), 1);
        assert!(moved[0].path().join("r.html").is_file() && moved[0].path().join(".card.json").is_file());
        assert!(find(&scope, "load-report").is_err());
        assert!(create_card(&scope, NewCard { title: "  ".into(), description: String::new(), category: String::new(), icon: None }).is_err());
    }

    #[test]
    fn paths_stay_inside_the_card() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("card");
        let project = dir.path().join("project");
        std::fs::create_dir_all(card.join("sub")).unwrap();
        std::fs::create_dir_all(project.join("docs/shots")).unwrap();
        std::fs::write(project.join("docs/r.md"), "# r").unwrap();
        std::fs::write(project.join("docs/shots/a.png"), "png").unwrap();
        std::fs::write(project.join("docs/shots/.env"), "SECRET=1").unwrap();
        std::fs::write(project.join(".env"), "SECRET=1").unwrap();
        std::fs::write(card.join("sub/x.md"), "x").unwrap();

        assert_eq!(check_rel("./a//b/c.md").unwrap(), "a/b/c.md");
        for bad in ["../x", "a/../../x", "/etc/passwd", ".env", "a/.git/config", "id_rsa", "keys/server.pem", "~/.ssh"] {
            assert!(check_rel(bad).is_err(), "{bad}");
        }
        let roots = [project.clone()];
        assert_eq!(classify_step_path(&card, "sub/x.md", &roots).unwrap(), StepSource::Inside("sub/x.md".into()));
        let abs_inside = card.join("sub/x.md").display().to_string();
        assert_eq!(classify_step_path(&card, &abs_inside, &roots).unwrap(), StepSource::Inside("sub/x.md".into()));
        let from_project = project.join("docs/r.md");
        assert_eq!(classify_step_path(&card, &from_project.display().to_string(), &roots).unwrap(), StepSource::Import(from_project.canonicalize().unwrap()));
        assert!(classify_step_path(&card, &project.join(".env").display().to_string(), &roots).is_err());
        assert!(classify_step_path(&card, "/etc/hostname", &roots).is_err());
        assert!(classify_step_path(&card, &project.display().to_string(), &roots).is_err());

        // Imports get free names and skip private files.
        assert_eq!(import_into(&card, &from_project).unwrap(), "r.md");
        assert_eq!(import_into(&card, &from_project).unwrap(), "r (2).md");
        assert_eq!(import_into(&card, &project.join("docs/shots")).unwrap(), "shots");
        assert!(card.join("shots/a.png").is_file());
        assert!(!card.join("shots/.env").exists());

        // Symlinks out of the card are neither listed nor resolved.
        std::os::unix::fs::symlink(&project, card.join("escape")).unwrap();
        assert!(resolve_in_card(&card, "escape/docs/r.md").is_err());
        let (list, _) = list_files(&card, "").unwrap();
        let names: Vec<_> = list.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["shots", "sub", "r.md", "r (2).md"]);
    }

    #[test]
    fn uploads_never_replace_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), "old").unwrap();
        let tmp = dir.path().join(".upload-1.part");
        std::fs::write(&tmp, "new").unwrap();
        assert_eq!(finish_upload(dir.path(), &tmp, "", "a.png").unwrap(), "a (2).png");
        assert!(!tmp.exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("a.png")).unwrap(), "old");
        std::fs::write(&tmp, "x").unwrap();
        assert_eq!(finish_upload(dir.path(), &tmp, "shots", "b.png").unwrap(), "shots/b.png");
        assert!(check_file_name(".env").is_err() && check_file_name("a/b").is_err());
    }

    #[test]
    fn markdown_saves_check_the_revision_and_keep_backups() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("card");
        let backups = dir.path().join("backups");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("notes.md"), "# one").unwrap();
        let t = read_text(&card, "notes.md", true).unwrap();
        assert!(t.editable);
        let (_, rev2, _) = write_markdown(&card, "notes.md", "# two", Some(&t.revision), false, &backups).unwrap();
        // The old revision no longer matches.
        let err = write_markdown(&card, "notes.md", "# three", Some(&t.revision), false, &backups).unwrap_err();
        assert_eq!(err.code, "conflict");
        assert_eq!(std::fs::read_to_string(card.join("notes.md")).unwrap(), "# two");
        write_markdown(&card, "notes.md", "# three", Some(&rev2), false, &backups).unwrap();
        write_markdown(&card, "notes.md", "# forced", Some("stale"), true, &backups).unwrap();
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 3);
        assert!(write_markdown(&card, "page.html", "x", None, true, &backups).is_err());
        assert!(!read_text(&card, "notes.md", false).unwrap().editable);
    }

    #[test]
    fn report_thumbnails_come_from_local_images() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("img")).unwrap();
        std::fs::write(dir.path().join("img/a%20b.png"), "x").unwrap();
        std::fs::write(dir.path().join("img/c d.webp"), "x").unwrap();
        std::fs::write(
            dir.path().join("r.html"),
            r#"<img src="https://x/y.png"><img alt="" src="../escape.png"><IMG class=a SRC='img/c%20d.webp?v=1'>"#,
        )
        .unwrap();
        assert_eq!(first_report_image(dir.path(), "r.html").as_deref(), Some("img/c d.webp"));
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.png", "img2.png", "Img1.png"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, ["Img1.png", "img2.png", "img10.png"]);
    }
}

//! The Workspace trash: deleted cards, restorable until deleted for good.
//!
//! `data_dir/workspace-trash/<scope>/<folder>~<YYYYMMDD-HHMMSS>/` (outside the
//! watched `data_dir/workspace`) holds the card's files, `.card.json` (its registry
//! entry as it was) and `.trash.json` (`{deletedAt, moved}`; `moved: false` when the
//! folder stayed with another card that shares it). Items from before `.trash.json`
//! existed count as moved, deleted at the time their name carries.
//!
//! Restoring moves the folder back (under a free name if a card or a directory took
//! the old one meanwhile, never onto anything: `RENAME_NOREPLACE`) and appends the
//! entry to the registry (compare-and-swap, a free id if the old one was reused); a
//! registry failure moves the folder back to the trash. Blocking; the routes run it
//! under the slice's write lock.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{self, Card, Doc};
use super::store::{self, Scope};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::util;

pub const CARD_FILE: &str = ".card.json";
pub const META_FILE: &str = ".trash.json";
/// Files counted per item before the listing says "at least".
const MAX_COUNT: usize = 10_000;
/// Items listed per scope.
const MAX_ITEMS: usize = 2000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// Unix milliseconds.
    pub deleted_at: i64,
    /// The card's folder is in the item (else it stayed in the scope).
    pub moved: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    /// The item's folder name in the trash.
    pub id: String,
    pub scope: String,
    pub scope_name: String,
    pub card_id: Option<String>,
    pub title: String,
    pub description: String,
    pub category: String,
    /// The card folder it had.
    pub folder: Option<String>,
    pub deleted_at: i64,
    pub moved: bool,
    pub files: usize,
    pub bytes: u64,
    /// More than `files` (counting stopped).
    pub files_capped: bool,
    pub restorable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// `data_dir/workspace-trash`.
pub fn root(state: &AppState) -> PathBuf {
    store::trash_dir(state)
}

/// A trash item name we accept from a client: one plain component.
fn valid_item(name: &str) -> bool {
    model::valid_folder(name)
}

/// A scope whose trash can be read: home, a project, or a scope that only has a
/// trash left (a project removed from Workbench; its items cannot be restored).
pub struct TrashScope {
    pub id: String,
    pub name: String,
    pub scope: Option<Scope>,
}

pub fn trash_scope(state: &AppState, id: &str) -> ApiResult<TrashScope> {
    match store::scope(state, id) {
        Ok(s) => Ok(TrashScope { id: s.id.clone(), name: s.name.clone(), scope: Some(s) }),
        Err(e) => {
            if model::valid_folder(id) && root(state).join(id).is_dir() {
                Ok(TrashScope { id: id.to_string(), name: id.to_string(), scope: None })
            } else {
                Err(e)
            }
        }
    }
}

/// Home, every project, then scopes that only have a trash.
pub fn all_trash_scopes(state: &AppState) -> Vec<TrashScope> {
    let mut out: Vec<TrashScope> = store::all_scopes(state).into_iter().map(|s| TrashScope { id: s.id.clone(), name: s.name.clone(), scope: Some(s) }).collect();
    let known: HashSet<String> = out.iter().map(|s| s.id.clone()).collect();
    if let Ok(dirs) = std::fs::read_dir(root(state)) {
        let mut orphans: Vec<String> = dirs
            .flatten()
            .filter(|d| d.file_type().is_ok_and(|t| t.is_dir()))
            .map(|d| d.file_name().to_string_lossy().into_owned())
            .filter(|n| model::valid_folder(n) && !known.contains(n) && !n.starts_with("legacy-"))
            .collect();
        orphans.sort();
        out.extend(orphans.into_iter().map(|id| TrashScope { name: id.clone(), id, scope: None }));
    }
    out
}

/// `<root>/<scope>/<item>`: validated, a real directory (not a symlink).
fn item_dir(trash_root: &Path, scope: &str, item: &str) -> ApiResult<PathBuf> {
    if !valid_item(item) || !model::valid_folder(scope) {
        return Err(ApiError::bad_request("not a trash item"));
    }
    let p = trash_root.join(scope).join(item);
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.is_dir() => Ok(p),
        Ok(_) => Err(ApiError::bad_request("not a trash item")),
        Err(_) => Err(ApiError::not_found(format!("{item} is not in the trash (any more)"))),
    }
}

/// The time in an item's name (`…~20260927-101500`, maybe with `-2`), local time.
fn stamp_of(name: &str) -> Option<i64> {
    use chrono::TimeZone;
    let tail = name.rsplit_once('~')?.1;
    let tail = tail.get(..15)?;
    let naive = chrono::NaiveDateTime::parse_from_str(tail, "%Y%m%d-%H%M%S").ok()?;
    chrono::Local.from_local_datetime(&naive).earliest().map(|t| t.timestamp_millis())
}

fn read_meta(dir: &Path, name: &str) -> Meta {
    if let Ok(Some(m)) = util::fs::read_json::<Meta>(&dir.join(META_FILE)) {
        return m;
    }
    let deleted_at = stamp_of(name).unwrap_or_else(|| std::fs::metadata(dir).map(|m| crate::files::mtime_ms(&m)).unwrap_or(0));
    Meta { deleted_at, moved: true }
}

fn read_entry(dir: &Path) -> ApiResult<(Doc, Card)> {
    let bytes = std::fs::read(dir.join(CARD_FILE)).map_err(|_| ApiError::bad_request("this item has no card entry (.card.json)"))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(ApiError::bad_request("the card entry is too large"));
    }
    let doc = Doc::parse(&bytes).map_err(|e| ApiError::bad_request(format!("the card entry does not parse: {e}")))?;
    let card = Card::from_doc(&doc).ok_or_else(|| ApiError::bad_request("the card entry has no id or folder"))?;
    if !model::valid_folder(&card.folder) {
        return Err(ApiError::bad_request(format!("unusable card folder {:?}", card.folder)));
    }
    Ok((doc, card))
}

/// Files and bytes below `dir` (not following symlinks), without our two files.
fn measure(dir: &Path) -> (usize, u64, bool) {
    let (mut files, mut bytes) = (0usize, 0u64);
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_COUNT {
                return (files, bytes, true);
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if d == dir && (e.file_name() == CARD_FILE || e.file_name() == META_FILE) {
                continue;
            } else {
                files += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (files, bytes, false)
}

/// A scope's trash, newest first.
pub fn list(trash_root: &Path, ts: &TrashScope) -> Vec<TrashItem> {
    let dir = trash_root.join(&ts.id);
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![] };
    let mut items: Vec<TrashItem> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| valid_item(n))
        .take(MAX_ITEMS)
        .map(|name| {
            let path = dir.join(&name);
            let meta = read_meta(&path, &name);
            let (files, bytes, capped) = if meta.moved { measure(&path) } else { (0, 0, false) };
            let (card, mut problem) = match read_entry(&path) {
                Ok((_, c)) => (Some(c), None),
                Err(e) => (None, Some(e.message)),
            };
            if ts.scope.is_none() && problem.is_none() {
                problem = Some(format!("{} is no longer a Workbench project", ts.id));
            }
            let fallback_title = name.rsplit_once('~').map(|(f, _)| f.to_string()).unwrap_or_else(|| name.clone());
            TrashItem {
                id: name.clone(),
                scope: ts.id.clone(),
                scope_name: ts.name.clone(),
                card_id: card.as_ref().map(|c| c.id.clone()),
                title: card.as_ref().map(|c| c.title.clone()).unwrap_or(fallback_title),
                description: card.as_ref().map(|c| c.description.clone()).unwrap_or_default(),
                category: card.as_ref().map(|c| c.category.clone()).unwrap_or_default(),
                folder: card.as_ref().map(|c| c.folder.clone()),
                deleted_at: meta.deleted_at,
                moved: meta.moved,
                files,
                bytes,
                files_capped: capped,
                restorable: problem.is_none(),
                problem,
            }
        })
        .collect();
    items.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then_with(|| a.id.cmp(&b.id)));
    items
}

/// Put an item back into its scope. Returns the card's (possibly new) id.
pub fn restore(trash_root: &Path, scope: &Scope, item: &str) -> ApiResult<String> {
    let src = item_dir(trash_root, &scope.id, item)?;
    let (entry, card) = read_entry(&src)?;
    let meta = read_meta(&src, item);
    store::ensure_scope_dir(scope)?;
    let registered: Vec<Card> = match store::read_registry(&scope.registry())? {
        Some((_, doc)) => Card::all_from(&doc).0,
        None => vec![],
    };
    let target = if meta.moved {
        let folders: HashSet<&str> = registered.iter().map(|c| c.folder.as_str()).collect();
        model::unique(&card.folder, |f| folders.contains(f) || store::taken(&scope.dir, f))
    } else {
        card.folder.clone()
    };
    let dest = scope.dir.join(&target);
    if meta.moved {
        match store::renameat2(&src, &dest, libc::RENAME_NOREPLACE) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(ApiError::conflict(format!("{target} appeared in the workspace meanwhile; try again")));
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP)) => {
                if std::fs::symlink_metadata(&dest).is_ok() {
                    return Err(ApiError::conflict(format!("{target} appeared in the workspace meanwhile; try again")));
                }
                std::fs::rename(&src, &dest)?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let stamp = model::now_stamp();
    let added = store::update_registry(&scope.registry(), true, |doc| {
        let (cards, _) = Card::all_from(doc);
        let ids: HashSet<&str> = cards.iter().map(|c| c.id.as_str()).collect();
        let id = model::unique(&card.id, |c| ids.contains(c) || c.starts_with(store::REPO_PREFIX));
        let mut e = entry.clone();
        e.set("id", Doc::str(&id));
        e.set("folder", Doc::str(&target));
        e.set_after("updated", Doc::str(&stamp), &["created"]);
        model::entities_mut(doc).ok_or_else(|| ApiError::internal("registry without entities"))?.push(e);
        Ok(id)
    });
    match added {
        Ok(id) => {
            if meta.moved {
                let _ = std::fs::remove_file(dest.join(CARD_FILE));
                let _ = std::fs::remove_file(dest.join(META_FILE));
            } else {
                // The folder stayed in the scope; the item held only the entry.
                if !dest.exists() {
                    let _ = std::fs::create_dir_all(&dest);
                }
                let _ = std::fs::remove_dir_all(&src);
            }
            remove_empty_scope_dir(trash_root, &scope.id);
            Ok(id)
        }
        Err(e) => {
            if meta.moved {
                if let Err(back) = std::fs::rename(&dest, &src) {
                    tracing::warn!("workspace trash: could not move {} back to the trash: {back}", dest.display());
                }
            }
            Err(e)
        }
    }
}

fn remove_empty_scope_dir(trash_root: &Path, scope: &str) {
    let _ = std::fs::remove_dir(trash_root.join(scope));
}

/// Delete one item for good.
pub fn purge(trash_root: &Path, scope: &str, item: &str) -> ApiResult<()> {
    let dir = item_dir(trash_root, scope, item)?;
    // remove_dir_all removes symlinks inside, never what they point at.
    std::fs::remove_dir_all(&dir)?;
    remove_empty_scope_dir(trash_root, scope);
    Ok(())
}

/// Delete every item of a scope for good. Returns how many went.
pub fn empty(trash_root: &Path, scope: &str) -> ApiResult<usize> {
    if !model::valid_folder(scope) {
        return Err(ApiError::bad_request("not a workspace scope"));
    }
    let dir = trash_root.join(scope);
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(0) };
    let mut n = 0;
    let mut failed = vec![];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !valid_item(&name) || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        match std::fs::remove_dir_all(e.path()) {
            Ok(()) => n += 1,
            Err(err) => failed.push(format!("{name}: {err}")),
        }
    }
    remove_empty_scope_dir(trash_root, scope);
    if !failed.is_empty() {
        return Err(ApiError::internal(format!("could not delete {} item(s): {}", failed.len(), failed.join("; "))));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::store::{NewCard, create_card, delete_card, find};

    fn scope(dir: &Path) -> Scope {
        Scope { id: "p".into(), name: "P".into(), dir: dir.join("ws/p"), project: None }
    }

    fn new_card(s: &Scope, title: &str) -> String {
        create_card(s, NewCard { title: title.into(), description: "about".into(), category: "report".into(), icon: None }).unwrap()
    }

    #[test]
    fn delete_restore_and_purge_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, trash) = (scope(tmp.path()), tmp.path().join("trash"));
        let id = new_card(&s, "Load report");
        let loc = find(&s, &id).unwrap();
        let folder = loc.card.folder.clone();
        std::fs::write(loc.dir().unwrap().join("r.html"), "<p>report</p>").unwrap();
        delete_card(&trash, &s, &loc).unwrap();

        let ts = TrashScope { id: "p".into(), name: "P".into(), scope: None };
        let items = list(&trash, &TrashScope { scope: Some(scope(tmp.path())), ..ts });
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!((it.title.as_str(), it.card_id.as_deref(), it.files, it.moved, it.restorable), ("Load report", Some(id.as_str()), 1, true, true));
        assert!(it.deleted_at > 0);

        // A new card takes the old id and folder name meanwhile: the restored one gets free ones.
        std::fs::create_dir_all(s.dir.join(&folder)).unwrap();
        let other = new_card(&s, "Load report");
        assert_eq!(other, id);
        let restored = restore(&trash, &s, &it.id).unwrap();
        assert_eq!(restored, format!("{id}-2"));
        let loc = find(&s, &restored).unwrap();
        assert_ne!(loc.card.folder, folder);
        assert_eq!(std::fs::read_to_string(loc.dir().unwrap().join("r.html")).unwrap(), "<p>report</p>");
        assert!(!loc.dir().unwrap().join(CARD_FILE).exists() && !loc.dir().unwrap().join(META_FILE).exists());
        assert_eq!(loc.card.description, "about");
        assert!(!trash.join("p").exists(), "an emptied scope folder goes");

        // Delete again, then for good.
        delete_card(&trash, &s, &loc).unwrap();
        let name = std::fs::read_dir(trash.join("p")).unwrap().flatten().next().unwrap().file_name().to_string_lossy().into_owned();
        purge(&trash, "p", &name).unwrap();
        assert!(!trash.join("p").exists());
        assert!(purge(&trash, "p", &name).is_err());
    }

    #[test]
    fn shared_folders_stay_and_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, trash) = (scope(tmp.path()), tmp.path().join("trash"));
        let a = new_card(&s, "A");
        let folder = find(&s, &a).unwrap().card.folder;
        // A second entry pointing at the same folder (as hand-edited registries do).
        store::update_registry(&s.registry(), false, |doc| {
            let e = Doc::object(vec![("id", Doc::str("b")), ("title", Doc::str("B")), ("folder", Doc::str(&folder)), ("status", Doc::str("active"))]);
            model::entities_mut(doc).unwrap().push(e);
            Ok(())
        })
        .unwrap();
        delete_card(&trash, &s, &find(&s, "b").unwrap()).unwrap();
        assert!(s.dir.join(&folder).is_dir(), "the shared folder stays");
        let items = list(&trash, &TrashScope { id: "p".into(), name: "P".into(), scope: Some(scope(tmp.path())) });
        assert_eq!((items.len(), items[0].moved), (1, false));
        assert_eq!(restore(&trash, &s, &items[0].id).unwrap(), "b");
        assert_eq!(find(&s, "b").unwrap().card.folder, folder);
    }

    #[test]
    fn bad_items_are_refused_and_empty_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, trash) = (scope(tmp.path()), tmp.path().join("trash"));
        for bad in ["..", ".card.json", "a/b", ""] {
            assert!(restore(&trash, &s, bad).is_err(), "{bad}");
            assert!(purge(&trash, "p", bad).is_err(), "{bad}");
        }
        // A legacy item (no .trash.json): moved, deleted at the time in its name.
        let legacy = trash.join("p/2026-09-01_old~20260901-120000");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(CARD_FILE), r#"{"id":"old","title":"Old","folder":"2026-09-01_old"}"#).unwrap();
        std::fs::write(legacy.join("x.md"), "x").unwrap();
        // An item whose entry is unusable (a folder escaping the scope).
        let evil = trash.join("p/evil~20260902-120000");
        std::fs::create_dir_all(&evil).unwrap();
        std::fs::write(evil.join(CARD_FILE), r#"{"id":"e","folder":"../../etc"}"#).unwrap();
        // A symlink is never an item.
        std::os::unix::fs::symlink(tmp.path(), trash.join("p/link~20260903-120000")).unwrap();
        let ts = TrashScope { id: "p".into(), name: "P".into(), scope: Some(scope(tmp.path())) };
        let items = list(&trash, &ts);
        assert_eq!(items.len(), 2, "{items:?}");
        let old = items.iter().find(|i| i.card_id.as_deref() == Some("old")).unwrap();
        assert!(old.moved && old.restorable && old.files == 1);
        assert_eq!(old.deleted_at, stamp_of("x~20260901-120000").unwrap());
        let bad = items.iter().find(|i| i.id.starts_with("evil")).unwrap();
        assert!(!bad.restorable && bad.problem.is_some());
        assert!(restore(&trash, &s, &bad.id).is_err());
        assert!(restore(&trash, &s, "link~20260903-120000").is_err());
        assert!(tmp.path().exists());
        assert_eq!(empty(&trash, "p").unwrap(), 2);
        assert!(tmp.path().exists(), "the symlink's target is untouched");
        assert!(empty(&trash, "../x").is_err());
    }
}

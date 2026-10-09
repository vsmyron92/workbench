//! Changelists (JetBrains): named groups of changed files, one of them active. Every
//! changed tracked file belongs to exactly one list (default "Changes"); a file seen
//! changed for the first time joins the active list. Stored per repository in
//! `data_dir/git/changelists/<scope>.json` (`<scope>`: the project id for the root
//! repository, else `Project::scope_key`), keyed by project-relative path, and
//! reconciled with `git status` (status and changelist reads).
//!
//! A file whose change leaves the working tree for a while (stash, shelve, an
//! autostash during a pull or rebase) keeps its list: when the change comes back and
//! HEAD still has the version it was made against, it returns to that list. A change
//! that comes back on another HEAD version (the file was committed meanwhile, e.g. from
//! a terminal) is new and joins the active list. Entries of files gone longer than
//! [`AWAY_TTL_MS`] are pruned, and a rollback in Workbench forgets the file at once.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ApiError;

pub const DEFAULT_ID: &str = "default";
const DEFAULT_NAME: &str = "Changes";
const MAX_LISTS: usize = 200;
const MAX_NAME: usize = 200;
/// How long a file whose change left the working tree keeps its list.
pub const AWAY_TTL_MS: i64 = 14 * 24 * 3600 * 1000;
/// At most this many entries of files that are not changed right now (oldest go first).
const MAX_AWAY: usize = 5_000;

/// A changed tracked file as reconcile sees it: its path and HEAD blob (`hH`).
#[derive(Debug, Clone, PartialEq)]
pub struct Changed {
    pub path: String,
    pub head_blob: Option<String>,
}

/// Bookkeeping of an entry of `Store::files` (a separate map, so `files` stays path → id).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    /// The file's blob in HEAD when its change was last seen ("" = unknown).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base: String,
    /// Unix ms since the change is gone from the working tree; 0 = changed now.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub away: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListMeta {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    #[serde(default = "one")]
    pub version: u32,
    pub active: String,
    pub lists: Vec<ListMeta>,
    /// Project-relative path → list id.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Per entry of `files`: the HEAD version it was changed against, and since when
    /// its change is gone (files not changed right now keep their list for a while).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub seen: BTreeMap<String, Seen>,
}

fn one() -> u32 {
    1
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: 1,
            active: DEFAULT_ID.into(),
            lists: vec![ListMeta { id: DEFAULT_ID.into(), name: DEFAULT_NAME.into(), comment: String::new(), created: 0 }],
            files: BTreeMap::new(),
            seen: BTreeMap::new(),
        }
    }
}

impl Store {
    fn has(&self, id: &str) -> bool {
        self.lists.iter().any(|l| l.id == id)
    }

    /// Repair what a hand edit or an older version could leave: no lists, an active
    /// id that does not exist, files in lists that do not exist.
    fn normalize(&mut self) {
        if self.lists.is_empty() {
            *self = Store { files: std::mem::take(&mut self.files), seen: std::mem::take(&mut self.seen), ..Store::default() };
        }
        if !self.has(&self.active) {
            self.active = self.lists[0].id.clone();
        }
        let ids: HashSet<String> = self.lists.iter().map(|l| l.id.clone()).collect();
        let active = self.active.clone();
        for v in self.files.values_mut() {
            if !ids.contains(v) {
                *v = active.clone();
            }
        }
    }

    /// Assign changed files that have no list to the active one; files whose change
    /// came back keep their list when HEAD still has the version the change was made
    /// against (else it is a new change: the active list). With a `complete` status
    /// (not truncated), files no longer changed are marked away, and entries away
    /// longer than [`AWAY_TTL_MS`] (or beyond [`MAX_AWAY`], oldest first) are pruned.
    /// Returns whether anything changed.
    pub fn reconcile(&mut self, changed: &[Changed], complete: bool, now: i64) -> bool {
        let before = (self.files.clone(), self.seen.clone());
        for c in changed {
            let base = c.head_blob.clone().unwrap_or_default();
            if self.files.contains_key(&c.path) {
                let s = self.seen.entry(c.path.clone()).or_default();
                if s.away != 0 {
                    if !s.base.is_empty() && !base.is_empty() && s.base != base {
                        // Committed (or replaced in HEAD) while it was gone: a new change.
                        self.files.insert(c.path.clone(), self.active.clone());
                    }
                    s.away = 0;
                }
                if !base.is_empty() {
                    s.base = base;
                }
            } else {
                self.files.insert(c.path.clone(), self.active.clone());
                self.seen.insert(c.path.clone(), Seen { base, away: 0 });
            }
        }
        if complete {
            let set: HashSet<&str> = changed.iter().map(|c| c.path.as_str()).collect();
            for p in self.files.keys() {
                if !set.contains(p.as_str()) {
                    let s = self.seen.entry(p.clone()).or_default();
                    if s.away == 0 {
                        s.away = now;
                    }
                }
            }
            let seen = &self.seen;
            self.files.retain(|p, _| seen.get(p).is_none_or(|s| s.away == 0 || now - s.away <= AWAY_TTL_MS));
            let mut away: Vec<(i64, String)> = self.seen.iter().filter(|(p, s)| s.away != 0 && self.files.contains_key(*p)).map(|(p, s)| (s.away, p.clone())).collect();
            if away.len() > MAX_AWAY {
                away.sort();
                for (_, p) in &away[..away.len() - MAX_AWAY] {
                    self.files.remove(p);
                }
            }
        }
        let files = &self.files;
        self.seen.retain(|p, _| files.contains_key(p));
        (self.files.clone(), self.seen.clone()) != before
    }

    /// Forget these files if their change is gone (rolled back in Workbench): a later
    /// change is new and joins the active list. Call after `reconcile`.
    pub fn forget_away(&mut self, paths: &[String]) -> bool {
        let mut changed = false;
        for p in paths {
            if self.seen.get(p).is_some_and(|s| s.away != 0) {
                self.files.remove(p);
                self.seen.remove(p);
                changed = true;
            }
        }
        changed
    }

    /// The list each of these files is in (files without an entry are left out).
    pub fn lists_of(&self, paths: &[String]) -> BTreeMap<String, String> {
        paths.iter().filter_map(|p| self.files.get(p).map(|l| (p.clone(), l.clone()))).collect()
    }

    fn check_name(&self, name: &str, except: Option<&str>) -> Result<String, ApiError> {
        let n = name.trim();
        if n.is_empty() {
            return Err(ApiError::bad_request("the changelist needs a name"));
        }
        if n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
            return Err(ApiError::bad_request("the changelist name is too long or has control characters"));
        }
        if self.lists.iter().any(|l| Some(l.id.as_str()) != except && l.name.eq_ignore_ascii_case(n)) {
            return Err(ApiError::conflict(format!("a changelist named {n:?} already exists")));
        }
        Ok(n.to_string())
    }

    pub fn create(&mut self, name: &str, comment: &str, make_active: bool) -> Result<String, ApiError> {
        if self.lists.len() >= MAX_LISTS {
            return Err(ApiError::bad_request("too many changelists"));
        }
        let name = self.check_name(name, None)?;
        let id = format!("cl-{}", crate::util::random_token(6));
        self.lists.push(ListMeta { id: id.clone(), name, comment: comment.trim().to_string(), created: crate::util::now_ms() });
        if make_active {
            self.active = id.clone();
        }
        Ok(id)
    }

    pub fn update(&mut self, id: &str, name: Option<&str>, comment: Option<&str>, active: bool) -> Result<(), ApiError> {
        if !self.has(id) {
            return Err(ApiError::not_found(format!("no changelist {id}")));
        }
        let name = match name {
            Some(n) => Some(self.check_name(n, Some(id))?),
            None => None,
        };
        let l = self.lists.iter_mut().find(|l| l.id == id).ok_or_else(|| ApiError::not_found("changelist vanished"))?;
        if let Some(n) = name {
            l.name = n;
        }
        if let Some(c) = comment {
            l.comment = c.trim().to_string();
        }
        if active {
            self.active = id.to_string();
        }
        Ok(())
    }

    /// Delete a list; its files move to the active list (to the first remaining one
    /// when the active list itself is deleted, which also becomes active).
    pub fn delete(&mut self, id: &str) -> Result<(), ApiError> {
        if !self.has(id) {
            return Err(ApiError::not_found(format!("no changelist {id}")));
        }
        if self.lists.len() == 1 {
            return Err(ApiError::bad_request("the last changelist cannot be deleted"));
        }
        self.lists.retain(|l| l.id != id);
        if self.active == id {
            self.active = self.lists[0].id.clone();
        }
        let to = self.active.clone();
        for v in self.files.values_mut() {
            if v == id {
                *v = to.clone();
            }
        }
        Ok(())
    }

    pub fn move_files(&mut self, paths: &[String], to: &str) -> Result<(), ApiError> {
        if !self.has(to) {
            return Err(ApiError::not_found(format!("no changelist {to}")));
        }
        for p in paths {
            self.files.insert(p.clone(), to.to_string());
        }
        Ok(())
    }

    /// The API view: every list with its files (only files in `changed`).
    pub fn view(&self, changed: &[String]) -> Changelists {
        let lists = self
            .lists
            .iter()
            .map(|l| Changelist {
                id: l.id.clone(),
                name: l.name.clone(),
                comment: l.comment.clone(),
                active: l.id == self.active,
                files: changed.iter().filter(|p| self.files.get(*p).unwrap_or(&self.active) == &l.id).cloned().collect(),
            })
            .collect();
        Changelists { active: self.active.clone(), lists }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Changelist {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub active: bool,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Changelists {
    pub active: String,
    pub lists: Vec<Changelist>,
}

pub fn store_path(data_dir: &Path, scope: &str) -> PathBuf {
    data_dir.join("git").join("changelists").join(format!("{scope}.json"))
}

/// Read (a missing or unreadable file is the default store), change and write back
/// only when `f` reports a change. Callers serialize access per project.
pub fn with_store<T>(path: &Path, f: impl FnOnce(&mut Store) -> Result<(T, bool), ApiError>) -> Result<T, ApiError> {
    let mut store = match crate::util::fs::read_json::<Store>(path) {
        Ok(Some(s)) => s,
        Ok(None) => Store::default(),
        Err(e) => {
            tracing::warn!("git: changelists {} unreadable ({e:#}); starting over", path.display());
            Store::default()
        }
    };
    let before = store.clone();
    store.normalize();
    let (out, changed) = f(&mut store)?;
    if changed || store != before {
        crate::util::fs::write_json(path, &store).map_err(|e| ApiError::internal(format!("cannot save changelists: {e:#}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Changed files, each against the HEAD blob "h<path>".
    fn ch(v: &[&str]) -> Vec<Changed> {
        v.iter().map(|p| Changed { path: p.to_string(), head_blob: Some(format!("h{p}")) }).collect()
    }

    const T0: i64 = 1_000_000;

    #[test]
    fn new_changes_join_the_active_list_and_entries_expire() {
        let mut s = Store::default();
        assert!(s.reconcile(&ch(&["a", "b"]), true, T0));
        let feature = s.create("Feature", "", true).unwrap();
        assert!(s.reconcile(&ch(&["a", "b", "c"]), true, T0));
        assert_eq!(s.files["a"], DEFAULT_ID);
        assert_eq!(s.files["c"], feature);
        // b is no longer changed: marked away (kept). A truncated status marks nothing.
        assert!(!s.reconcile(&ch(&["a", "c"]), false, T0));
        assert!(s.reconcile(&ch(&["a", "c"]), true, T0 + 5));
        assert_eq!(s.seen["b"].away, T0 + 5);
        assert!(!s.reconcile(&ch(&["a", "c"]), true, T0 + 10), "nothing new: nothing to write");
        let v = s.view(&paths(&["a", "c", "d"]));
        assert_eq!(v.lists[0].files, vec!["a"]);
        // d is not reconciled yet: shown in the active list.
        assert_eq!(v.lists[1].files, vec!["c", "d"]);
        assert!(v.lists[1].active);
        // Away longer than the TTL: pruned.
        assert!(s.reconcile(&ch(&["a", "c"]), true, T0 + 6 + AWAY_TTL_MS));
        assert!(!s.files.contains_key("b") && !s.seen.contains_key("b"));
    }

    #[test]
    fn a_change_that_comes_back_keeps_its_list_unless_it_was_committed_meanwhile() {
        let mut s = Store::default();
        let later = s.create("Do not commit", "", false).unwrap();
        s.reconcile(&ch(&["cfg", "x"]), true, T0);
        s.move_files(&paths(&["cfg", "x"]), &later).unwrap();
        // Stashed (or shelved, or autostashed by a pull): both leave the working tree…
        s.reconcile(&[], true, T0 + 1);
        assert_eq!(s.files["cfg"], later, "kept while away");
        // …and come back on the same HEAD versions: back in their list, not the active one.
        s.reconcile(&ch(&["cfg", "x"]), true, T0 + 2);
        assert_eq!((s.files["cfg"].as_str(), s.files["x"].as_str()), (later.as_str(), later.as_str()));
        assert_eq!(s.seen["cfg"].away, 0);
        // x was committed from a terminal, then changed again: HEAD has a new version of it.
        s.reconcile(&ch(&["cfg"]), true, T0 + 3);
        let x2 = Changed { path: "x".into(), head_blob: Some("new blob".into()) };
        s.reconcile(&[ch(&["cfg"]).remove(0), x2], true, T0 + 4);
        assert_eq!(s.files["x"], DEFAULT_ID, "a new change joins the active list");
        assert_eq!(s.files["cfg"], later);
        // Rolled back in Workbench: forgotten at once, the next change is new.
        s.reconcile(&ch(&["x"]), true, T0 + 5);
        assert!(s.forget_away(&paths(&["cfg", "x"])));
        assert!(!s.files.contains_key("cfg") && s.files.contains_key("x"), "only files whose change is gone are forgotten");
        s.reconcile(&ch(&["cfg", "x"]), true, T0 + 6);
        assert_eq!(s.files["cfg"], DEFAULT_ID);
        assert_eq!(s.lists_of(&paths(&["cfg", "nope"])), [("cfg".to_string(), DEFAULT_ID.to_string())].into_iter().collect());
    }

    #[test]
    fn caps_the_entries_of_files_that_are_away() {
        let mut s = Store::default();
        let many: Vec<String> = (0..MAX_AWAY + 10).map(|i| format!("f{i:05}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        s.reconcile(&ch(&refs), true, T0);
        // They leave one by one (older first).
        for (i, p) in many.iter().enumerate() {
            let rest: Vec<&str> = refs.iter().copied().filter(|q| q > &p.as_str()).collect();
            if i % 1000 == 0 || i == many.len() - 1 {
                s.reconcile(&ch(&rest), true, T0 + i as i64);
            }
        }
        assert_eq!(s.files.len(), MAX_AWAY);
        assert!(!s.files.contains_key("f00000"), "the oldest away entries go first");
        assert!(s.files.contains_key(many.last().unwrap()));
    }

    #[test]
    fn moves_renames_and_deletes() {
        let mut s = Store::default();
        let x = s.create("X", "note", false).unwrap();
        assert_eq!(s.create("x", "", false).unwrap_err().status.as_u16(), 409, "names are unique, case-insensitively");
        s.reconcile(&ch(&["a", "b"]), true, T0);
        s.move_files(&paths(&["b"]), &x).unwrap();
        assert!(s.move_files(&paths(&["a"]), "nope").is_err());
        s.update(&x, Some("Renamed"), None, true).unwrap();
        assert_eq!(s.active, x);
        assert!(s.update(DEFAULT_ID, Some("Renamed"), None, false).is_err());
        // Deleting the active list: the first list becomes active and takes its files.
        s.delete(&x).unwrap();
        assert_eq!(s.active, DEFAULT_ID);
        assert_eq!(s.files["b"], DEFAULT_ID);
        assert!(s.delete(DEFAULT_ID).is_err(), "the last list stays");
    }

    #[test]
    fn repairs_hand_edited_files_and_persists() {
        let d = tempfile::tempdir().unwrap();
        let p = store_path(d.path(), "proj");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, r#"{"active":"gone","lists":[{"id":"l1","name":"One"}],"files":{"a":"missing"}}"#).unwrap();
        let v = with_store(&p, |s| Ok((s.view(&paths(&["a"])), false))).unwrap();
        assert_eq!(v.active, "l1");
        assert_eq!(v.lists[0].files, vec!["a"]);
        let saved: Store = crate::util::fs::read_json(&p).unwrap().unwrap();
        assert_eq!(saved.files["a"], "l1");
        // Garbage starts over.
        std::fs::write(&p, "not json").unwrap();
        let v = with_store(&p, |s| Ok((s.view(&[]), false))).unwrap();
        assert_eq!(v.lists[0].name, "Changes");
    }
}

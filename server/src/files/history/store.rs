//! The local history of one project on disk: `data_dir/local-history/<pid>/`.
//!
//! * `blobs/<h0h1>/<sha256>.zst` — every content version once, zstd-compressed,
//!   addressed by the sha256 of the raw bytes (the same hash the editor uses as the
//!   file's etag).
//! * `index.jsonl` — append-only, one JSON object per line: a revision of a path
//!   (`save`, `disk`, `agent`, `base`), a deletion, a label, or an attribution fix
//!   (`attr`, folded into the revision it names when loading). A torn last line (a
//!   crash or a full disk mid-append) is skipped and cut off when the store opens,
//!   so the next line never lands on it; a failed append truncates its own partial
//!   line. Pruning rewrites it (temp file, fsync, rename) and then deletes blobs
//!   nothing references.
//!
//! Everything here is blocking; the slice runs it on the blocking pool under the
//! project's lock.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::util::os::perm;

/// Largest file version kept (larger files are not tracked).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// zstd level: fast, and text compresses well at it.
const ZSTD_LEVEL: i32 = 3;
const INDEX: &str = "index.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Saved in Workbench (editor save, replace in files).
    Save,
    /// Changed on disk by something Workbench cannot name.
    Disk,
    /// Changed on disk by an agent session (its hook named the file).
    Agent,
    /// The version Workbench first saw (opened, or on disk before the first save).
    Base,
    /// The file disappeared.
    Deleted,
    /// A label the user put.
    Label,
    /// A label Workbench put itself (before a git operation).
    Auto,
    /// Index-only: marks revision `of` as an agent edit (the hook came late).
    Attr,
}

impl Kind {
    /// Holds a content version.
    pub fn has_content(self) -> bool {
        matches!(self, Kind::Save | Kind::Disk | Kind::Agent | Kind::Base)
    }

    /// A change of a file (what directory history and Recent Changes list).
    pub fn is_change(self) -> bool {
        matches!(self, Kind::Save | Kind::Disk | Kind::Agent | Kind::Deleted)
    }

    pub fn is_label(self) -> bool {
        matches!(self, Kind::Label | Kind::Auto)
    }

    /// A revision of one path (content or deletion).
    fn is_revision(self) -> bool {
        self.has_content() || self == Kind::Deleted
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// Unix milliseconds.
    #[serde(rename = "t")]
    pub ts: i64,
    /// Project-relative path; for labels the folder or file they were put on (`""`: the project).
    #[serde(rename = "p", default)]
    pub path: String,
    #[serde(rename = "k")]
    pub kind: Kind,
    /// sha256 (hex) of the content.
    #[serde(rename = "h", default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Content size in bytes.
    #[serde(rename = "s", default)]
    pub size: u64,
    /// Compressed blob size.
    #[serde(rename = "z", default)]
    pub stored: u64,
    /// Label text, or a detail of the change ("Replace in Files").
    #[serde(rename = "l", default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Agent edits: the terminal id of the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Agent edits: the session's title when it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// `attr` lines: the revision they attribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u64>,
}

/// One file an agent session changed ([`Store::session_files`]).
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: String,
    pub before: Option<Entry>,
    pub first: Entry,
    pub last: Entry,
    pub edits: usize,
    pub latest: Entry,
}

/// What to record.
pub struct NewRevision<'a> {
    pub path: &'a str,
    pub kind: Kind,
    pub ts: i64,
    /// Required for content kinds.
    pub content: Option<&'a [u8]>,
    pub label: Option<String>,
    pub by: Option<String>,
    pub who: Option<String>,
}

impl<'a> NewRevision<'a> {
    pub fn content(path: &'a str, kind: Kind, ts: i64, content: &'a [u8]) -> Self {
        Self { path, kind, ts, content: Some(content), label: None, by: None, who: None }
    }
}

/// Retention rules.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub max_age_ms: i64,
    /// Revisions kept per file (newest first).
    pub max_versions: usize,
    /// Compressed blob bytes per project.
    pub max_bytes: u64,
    /// Index entries per project.
    pub max_entries: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self { max_age_ms: 7 * 24 * 3600 * 1000, max_versions: 100, max_bytes: 256 * 1024 * 1024, max_entries: 200_000 }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneStats {
    pub removed_entries: usize,
    pub removed_blobs: usize,
}

pub struct Store {
    dir: PathBuf,
    /// Ascending by id (the order they were recorded in).
    entries: Vec<Entry>,
    next_id: u64,
    /// Path → index in `entries` of its newest revision.
    latest: HashMap<String, usize>,
    /// Hash → (compressed size, references).
    blobs: HashMap<String, (u64, u32)>,
    stored_bytes: u64,
    /// `attr` lines in the index (folded away by the next rewrite).
    loose_lines: usize,
}

fn mkdir(p: &Path) -> std::io::Result<()> {
    perm::create_dir_private(p)
}

pub fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl Store {
    /// Open (or start) the history in `dir`. Unreadable index lines are skipped.
    pub fn open(dir: &Path) -> std::io::Result<Store> {
        let mut store = Store {
            dir: dir.to_path_buf(),
            entries: vec![],
            next_id: 1,
            latest: HashMap::new(),
            blobs: HashMap::new(),
            stored_bytes: 0,
            loose_lines: 0,
        };
        let file = match File::open(dir.join(INDEX)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(store),
            Err(e) => return Err(e),
        };
        let mut attrs = vec![];
        let mut bad = 0usize;
        let mut reader = BufReader::new(file);
        let mut line = vec![];
        // Bytes up to the end of the last complete line, and an unterminated tail.
        let mut complete = 0u64;
        let mut tail: Option<bool> = None;
        loop {
            line.clear();
            let n = reader.read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            let terminated = line.last() == Some(&b'\n');
            if terminated {
                line.pop();
            }
            let parsed = if line.iter().all(u8::is_ascii_whitespace) {
                true
            } else {
                match serde_json::from_slice::<Entry>(&line) {
                    Ok(e) if e.kind == Kind::Attr => {
                        attrs.push(e);
                        true
                    }
                    Ok(e) if e.kind.has_content() && !e.hash.as_deref().is_some_and(valid_hash) => {
                        bad += 1;
                        false
                    }
                    Ok(e) => {
                        store.entries.push(e);
                        true
                    }
                    Err(_) => {
                        bad += 1;
                        false
                    }
                }
            };
            if terminated {
                complete += n as u64;
            } else {
                tail = Some(parsed);
            }
        }
        if bad > 0 {
            tracing::warn!("local history {}: skipped {bad} unreadable index line(s)", dir.display());
        }
        if let Some(parsed) = tail {
            // The last append never finished: without this, the next line would be
            // glued onto it and be lost at the next open.
            let f = OpenOptions::new().append(true).open(dir.join(INDEX))?;
            if parsed {
                (&f).write_all(b"\n")?;
            } else {
                f.set_len(complete)?;
                bad -= 1;
            }
        }
        store.entries.sort_by_key(|e| e.id);
        store.entries.dedup_by_key(|e| e.id);
        store.loose_lines = attrs.len() + bad;
        for a in attrs {
            if let Some(i) = a.of.and_then(|id| store.position(id)) {
                let e = &mut store.entries[i];
                if e.kind == Kind::Disk {
                    e.kind = Kind::Agent;
                    e.by = a.by;
                    e.who = a.who;
                }
            }
        }
        store.rebuild();
        Ok(store)
    }

    fn rebuild(&mut self) {
        self.latest.clear();
        self.blobs.clear();
        self.stored_bytes = 0;
        self.next_id = self.next_id.max(self.entries.last().map(|e| e.id + 1).unwrap_or(1));
        for (i, e) in self.entries.iter().enumerate() {
            if e.kind.is_revision() {
                self.latest.insert(e.path.clone(), i);
            }
            if let (true, Some(h)) = (e.kind.has_content(), &e.hash) {
                let slot = self.blobs.entry(h.clone()).or_insert((e.stored, 0));
                if slot.1 == 0 {
                    self.stored_bytes += e.stored;
                }
                slot.1 += 1;
            }
        }
    }

    fn position(&self, id: u64) -> Option<usize> {
        self.entries.binary_search_by_key(&id, |e| e.id).ok()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }

    /// Paths with at least one revision.
    pub fn file_count(&self) -> usize {
        self.latest.len()
    }

    /// Paths whose newest revision is content (not deleted).
    pub fn tracked_paths(&self) -> impl Iterator<Item = &str> {
        self.latest.iter().filter(|(_, i)| self.entries[**i].kind.has_content()).map(|(p, _)| p.as_str())
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.position(id).map(|i| &self.entries[i])
    }

    /// The newest revision (content or deletion) of `path`.
    pub fn latest(&self, path: &str) -> Option<&Entry> {
        self.latest.get(path).map(|&i| &self.entries[i])
    }

    /// The hash of the newest content of `path` (`None` when unknown or deleted).
    pub fn latest_hash(&self, path: &str) -> Option<&str> {
        self.latest(path).filter(|e| e.kind.has_content()).and_then(|e| e.hash.as_deref())
    }

    /// The revision of the same path recorded before `id` (content or deletion).
    pub fn previous(&self, id: u64) -> Option<&Entry> {
        let i = self.position(id)?;
        let path = &self.entries[i].path;
        self.entries[..i].iter().rev().find(|e| e.kind.is_revision() && &e.path == path)
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.dir.join("blobs").join(&hash[..2]).join(format!("{hash}.zst"))
    }

    /// The content of a version.
    pub fn read_blob(&self, hash: &str) -> std::io::Result<Vec<u8>> {
        if !valid_hash(hash) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad hash"));
        }
        let f = File::open(self.blob_path(hash))?;
        let mut out = vec![];
        // Bounded: a corrupt or planted blob cannot make us allocate without end.
        zstd::stream::read::Decoder::new(f)?.take(MAX_FILE_BYTES * 2 + 1).read_to_end(&mut out)?;
        if out.len() as u64 > MAX_FILE_BYTES * 2 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "history blob too large"));
        }
        Ok(out)
    }

    /// Write the blob for `content` unless it is there. Returns its compressed size.
    fn put_blob(&self, hash: &str, content: &[u8]) -> std::io::Result<u64> {
        let path = self.blob_path(hash);
        if let Ok(md) = std::fs::metadata(&path) {
            if md.len() > 0 {
                return Ok(md.len());
            }
        }
        let dir = path.parent().ok_or_else(|| std::io::Error::other("blob path without parent"))?;
        mkdir(dir)?;
        let packed = zstd::bulk::compress(content, ZSTD_LEVEL)?;
        let tmp = dir.join(format!(".{hash}.tmp-{}", crate::util::random_token(4)));
        let written = (|| -> std::io::Result<()> {
            let mut f = perm::open_new(&tmp, 0o600, false)?;
            f.write_all(&packed)?;
            f.sync_data()?;
            perm::rename_into_place(&tmp, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written?;
        Ok(packed.len() as u64)
    }

    fn append(&mut self, e: &Entry) -> std::io::Result<()> {
        mkdir(&self.dir)?;
        let mut line = serde_json::to_vec(e).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut f = perm::open_append(&self.dir.join(INDEX), 0o600)?;
        let before = f.metadata()?.len();
        let written = f.write_all(&line);
        if written.is_err() {
            // A full disk mid-line: take the fragment back, or the next line joins it.
            let _ = perm::set_len(&f, before);
        }
        written
    }

    fn push(&mut self, e: Entry) {
        let i = self.entries.len();
        if e.kind.is_revision() {
            self.latest.insert(e.path.clone(), i);
        }
        if let (true, Some(h)) = (e.kind.has_content(), &e.hash) {
            let slot = self.blobs.entry(h.clone()).or_insert((e.stored, 0));
            if slot.1 == 0 {
                self.stored_bytes += e.stored;
            }
            slot.1 += 1;
        }
        self.next_id = e.id + 1;
        self.entries.push(e);
    }

    /// Record a version. `Ok(None)` when it adds nothing: the same content as the
    /// path's newest revision, or a deletion of a path that has none.
    pub fn record(&mut self, n: NewRevision<'_>) -> std::io::Result<Option<Entry>> {
        let mut e = Entry {
            id: self.next_id,
            ts: n.ts,
            path: n.path.to_string(),
            kind: n.kind,
            hash: None,
            size: 0,
            stored: 0,
            label: n.label,
            by: n.by,
            who: n.who,
            of: None,
        };
        match n.kind {
            k if k.has_content() => {
                let content = n.content.ok_or_else(|| std::io::Error::other("a revision needs content"))?;
                if content.len() as u64 > MAX_FILE_BYTES {
                    return Ok(None);
                }
                let hash = crate::files::sha256_hex(content);
                if self.latest_hash(n.path) == Some(hash.as_str()) {
                    return Ok(None);
                }
                // A base is the first version of a path only.
                if k == Kind::Base && self.latest(n.path).is_some_and(|l| l.kind != Kind::Deleted) {
                    e.kind = Kind::Disk;
                    e.label = None;
                }
                e.stored = match self.blobs.get(&hash) {
                    Some(&(z, _)) if self.blob_path(&hash).is_file() => z,
                    _ => self.put_blob(&hash, content)?,
                };
                e.size = content.len() as u64;
                e.hash = Some(hash);
            }
            Kind::Deleted => {
                if !self.latest(n.path).is_some_and(|l| l.kind.has_content()) {
                    return Ok(None);
                }
            }
            Kind::Label | Kind::Auto => {
                if e.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a label needs text"));
                }
            }
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a recordable kind")),
        }
        self.append(&e)?;
        self.push(e.clone());
        Ok(Some(e))
    }

    /// The hook of an agent named `path` after the watcher already recorded the
    /// change: mark that revision (the newest, with this content, recorded at or
    /// after `since`) as the agent's. Returns the updated entry.
    pub fn attribute(&mut self, path: &str, hash: &str, since: i64, by: &str, who: Option<&str>) -> std::io::Result<Option<Entry>> {
        let Some(&i) = self.latest.get(path) else { return Ok(None) };
        let e = &self.entries[i];
        if e.kind != Kind::Disk || e.hash.as_deref() != Some(hash) || e.ts < since {
            return Ok(None);
        }
        let fix = Entry {
            id: 0,
            ts: e.ts,
            path: path.to_string(),
            kind: Kind::Attr,
            hash: None,
            size: 0,
            stored: 0,
            label: None,
            by: Some(by.to_string()),
            who: who.map(str::to_string),
            of: Some(e.id),
        };
        self.append(&fix)?;
        self.loose_lines += 1;
        let e = &mut self.entries[i];
        e.kind = Kind::Agent;
        e.by = fix.by;
        e.who = fix.who;
        Ok(Some(e.clone()))
    }

    /// What an agent session (`by`: its terminal id) changed, one item per file in the
    /// order it first changed them: the revision before its first edit (`None`: nothing
    /// recorded, a file it created), its first and last edits, and the file's newest
    /// revision (someone else may have changed it since).
    pub fn session_files(&self, by: &str) -> Vec<SessionFile> {
        let mut order: Vec<&str> = vec![];
        let mut seen: std::collections::HashMap<&str, (usize, usize, usize)> = std::collections::HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.kind != Kind::Agent || e.by.as_deref() != Some(by) {
                continue;
            }
            match seen.get_mut(e.path.as_str()) {
                Some(s) => {
                    s.1 = i;
                    s.2 += 1;
                }
                None => {
                    order.push(e.path.as_str());
                    seen.insert(e.path.as_str(), (i, i, 1));
                }
            }
        }
        order
            .into_iter()
            .map(|path| {
                let (fi, li, edits) = seen[path];
                // A file deleted before the session did not exist when it started.
                let before = self.entries[..fi].iter().rev().find(|e| e.kind.is_revision() && e.path == path).filter(|e| e.kind.has_content()).cloned();
                let last = self.entries[li].clone();
                let latest = self.entries[li..].iter().rev().find(|e| e.kind.is_revision() && e.path == path).cloned().unwrap_or_else(|| last.clone());
                SessionFile { path: path.to_string(), before, first: self.entries[fi].clone(), last, edits, latest }
            })
            .collect()
    }

    /// Newest first: the revisions of `path` and the labels that apply to it (put on
    /// the project, one of its folders, or the file), older than `before` if given.
    pub fn file_history(&self, path: &str, limit: usize, before: Option<u64>) -> (Vec<Entry>, bool) {
        let applies = |e: &Entry| {
            if e.kind.is_label() {
                e.path.is_empty() || e.path == path || path.starts_with(&format!("{}/", e.path))
            } else {
                e.kind.is_revision() && e.path == path
            }
        };
        self.collect(limit, before, applies)
    }

    /// Newest first: changes of files in `dir` (`""`: the whole project) and the
    /// labels put on it, above it or below it.
    pub fn dir_history(&self, dir: &str, limit: usize, before: Option<u64>, visible: impl Fn(&str) -> bool) -> (Vec<Entry>, bool) {
        let prefix = format!("{dir}/");
        let within = |p: &str| dir.is_empty() || p == dir || p.starts_with(&prefix);
        let applies = |e: &Entry| {
            if e.kind.is_label() {
                within(&e.path) || e.path.is_empty() || dir.starts_with(&format!("{}/", e.path))
            } else {
                e.kind.is_change() && within(&e.path) && visible(&e.path)
            }
        };
        self.collect(limit, before, applies)
    }

    fn collect(&self, limit: usize, before: Option<u64>, applies: impl Fn(&Entry) -> bool) -> (Vec<Entry>, bool) {
        let end = match before {
            Some(b) => self.entries.partition_point(|e| e.id < b),
            None => self.entries.len(),
        };
        let mut out = vec![];
        for e in self.entries[..end].iter().rev() {
            if applies(e) {
                if out.len() == limit {
                    return (out, true);
                }
                out.push(e.clone());
            }
        }
        (out, false)
    }

    /// Apply `policy` at time `now`: drop old entries, extra versions per file, and
    /// the oldest ones while the blobs take more than the cap (down to 90% of it) or
    /// the index holds too many entries. Rewrites the index and deletes blobs no
    /// longer referenced. Paths `drop_path` names (now sensitive) go too.
    pub fn prune(&mut self, policy: &Policy, now: i64, drop_path: impl Fn(&str) -> bool) -> std::io::Result<PruneStats> {
        let n = self.entries.len();
        let mut keep = vec![true; n];
        let cutoff = now - policy.max_age_ms;
        let mut per_path: HashMap<&str, usize> = HashMap::new();
        for i in (0..n).rev() {
            let e = &self.entries[i];
            if e.ts < cutoff || (!e.kind.is_label() && drop_path(&e.path)) {
                keep[i] = false;
                continue;
            }
            if e.kind.is_revision() {
                let c = per_path.entry(e.path.as_str()).or_insert(0);
                *c += 1;
                if *c > policy.max_versions {
                    keep[i] = false;
                }
            }
        }
        // Size and count caps: the oldest go first.
        let mut refs: HashMap<&str, (u64, u32)> = HashMap::new();
        let mut kept = 0usize;
        for (i, e) in self.entries.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            kept += 1;
            if let (true, Some(h)) = (e.kind.has_content(), e.hash.as_deref()) {
                let r = refs.entry(h).or_insert((e.stored, 0));
                r.1 += 1;
            }
        }
        let mut bytes: u64 = refs.values().map(|r| r.0).sum();
        let byte_goal = if bytes > policy.max_bytes { policy.max_bytes / 10 * 9 } else { u64::MAX };
        let count_goal = if kept > policy.max_entries { policy.max_entries / 10 * 9 } else { usize::MAX };
        if bytes > byte_goal || kept > count_goal {
            for (i, e) in self.entries.iter().enumerate() {
                if bytes <= byte_goal && kept <= count_goal {
                    break;
                }
                if !keep[i] {
                    continue;
                }
                keep[i] = false;
                kept -= 1;
                if let (true, Some(h)) = (e.kind.has_content(), e.hash.as_deref()) {
                    if let Some(r) = refs.get_mut(h) {
                        r.1 -= 1;
                        if r.1 == 0 {
                            bytes -= r.0;
                        }
                    }
                }
            }
        }
        let removed = keep.iter().filter(|k| !**k).count();
        if removed == 0 && self.loose_lines == 0 {
            return Ok(PruneStats::default());
        }
        let mut i = 0;
        self.entries.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        // A deletion with nothing before it says nothing.
        let mut seen: HashSet<String> = HashSet::new();
        self.entries.retain(|e| {
            if !e.kind.is_revision() {
                return true;
            }
            let first = seen.insert(e.path.clone());
            !(first && e.kind == Kind::Deleted)
        });
        let removed_entries = n - self.entries.len();
        self.rewrite_index()?;
        self.loose_lines = 0;
        self.rebuild();
        let removed_blobs = self.gc_blobs()?;
        Ok(PruneStats { removed_entries, removed_blobs })
    }

    fn rewrite_index(&self) -> std::io::Result<()> {
        mkdir(&self.dir)?;
        let path = self.dir.join(INDEX);
        if self.entries.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let tmp = self.dir.join(format!(".{INDEX}.tmp-{}", crate::util::random_token(4)));
        let written = (|| -> std::io::Result<()> {
            let f = perm::open_new(&tmp, 0o600, false)?;
            let mut w = std::io::BufWriter::new(f);
            for e in &self.entries {
                serde_json::to_writer(&mut w, e).map_err(std::io::Error::other)?;
                w.write_all(b"\n")?;
            }
            let f = w.into_inner().map_err(|e| e.into_error())?;
            f.sync_all()?;
            perm::rename_into_place(&tmp, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written?;
        if let Ok(d) = File::open(&self.dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }

    /// Delete blobs (and stale temp files) nothing references.
    fn gc_blobs(&self) -> std::io::Result<usize> {
        let root = self.dir.join("blobs");
        let Ok(shards) = std::fs::read_dir(&root) else { return Ok(0) };
        let mut removed = 0;
        for shard in shards.flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else { continue };
            let mut left = 0;
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let referenced = name.strip_suffix(".zst").is_some_and(|h| self.blobs.contains_key(h));
                if referenced {
                    left += 1;
                } else if std::fs::remove_file(f.path()).is_ok() {
                    removed += 1;
                } else {
                    left += 1;
                }
            }
            if left == 0 {
                let _ = std::fs::remove_dir(shard.path());
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600 * 1000;

    fn rev<'a>(path: &'a str, kind: Kind, ts: i64, content: &'a [u8]) -> NewRevision<'a> {
        NewRevision::content(path, kind, ts, content)
    }

    fn blob_files(dir: &Path) -> usize {
        std::fs::read_dir(dir.join("blobs"))
            .map(|d| d.flatten().map(|s| std::fs::read_dir(s.path()).map(|f| f.count()).unwrap_or(0)).sum())
            .unwrap_or(0)
    }

    #[test]
    fn records_dedups_and_survives_a_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("h");
        let mut s = Store::open(&dir).unwrap();
        let a = s.record(rev("src/a.rs", Kind::Base, 1, b"one\n")).unwrap().unwrap();
        assert_eq!(a.kind, Kind::Base);
        // Same content again: nothing new.
        assert!(s.record(rev("src/a.rs", Kind::Disk, 2, b"one\n")).unwrap().is_none());
        let b = s.record(rev("src/a.rs", Kind::Save, 3, b"two\n")).unwrap().unwrap();
        // Another path with the same content shares the blob.
        let c = s.record(rev("src/b.rs", Kind::Disk, 4, b"two\n")).unwrap().unwrap();
        assert_eq!(b.hash, c.hash);
        assert_eq!(blob_files(&dir), 2);
        // A base for a path that has history is just a change on disk.
        let d = s.record(rev("src/a.rs", Kind::Base, 5, b"three\n")).unwrap().unwrap();
        assert_eq!(d.kind, Kind::Disk);
        assert_eq!(s.read_blob(b.hash.as_deref().unwrap()).unwrap(), b"two\n");
        assert_eq!(s.previous(d.id).unwrap().id, b.id);
        assert!(s.previous(a.id).is_none());

        // Deletion only for known paths, once.
        assert!(s.record(NewRevision { path: "nope", kind: Kind::Deleted, ts: 6, content: None, label: None, by: None, who: None }).unwrap().is_none());
        let del = NewRevision { path: "src/b.rs", kind: Kind::Deleted, ts: 6, content: None, label: None, by: None, who: None };
        assert!(s.record(del).unwrap().is_some());
        let del = NewRevision { path: "src/b.rs", kind: Kind::Deleted, ts: 7, content: None, label: None, by: None, who: None };
        assert!(s.record(del).unwrap().is_none());
        assert_eq!(s.latest_hash("src/b.rs"), None);
        // Recreated with the old content: a new revision.
        assert!(s.record(rev("src/b.rs", Kind::Disk, 8, b"two\n")).unwrap().is_some());

        let before = s.entries.clone();
        let s2 = Store::open(&dir).unwrap();
        assert_eq!(s2.entries, before);
        assert_eq!(s2.stored_bytes(), s.stored_bytes());
        assert_eq!(s2.next_id, s.next_id);
        // File mode: private.
        crate::util::os::perm::assert_mode(&dir.join(INDEX), 0o600);
    }

    #[test]
    fn torn_and_garbage_lines_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        s.record(rev("a", Kind::Disk, 1, b"x")).unwrap();
        let mut f = OpenOptions::new().append(true).open(tmp.path().join(INDEX)).unwrap();
        f.write_all(b"not json\n{\"id\":9,\"t\":1,\"k\":\"disk\",\"h\":\"../../etc/passwd\"}\n{\"id\":5,\"t\":2,\"p\":\"a\",\"k\":\"sa").unwrap();
        let s = Store::open(tmp.path()).unwrap();
        assert_eq!(s.len(), 1);
        assert!(s.read_blob("../../etc/passwd").is_err());
    }

    /// A torn last line is cut off when the store opens, so the next revision is not
    /// glued onto it (and lost, its id reused, at the next open).
    #[test]
    fn a_torn_line_does_not_swallow_the_next_revision() {
        let tmp = tempfile::tempdir().unwrap();
        let index = tmp.path().join(INDEX);
        let mut s = Store::open(tmp.path()).unwrap();
        s.record(rev("a", Kind::Disk, 1, b"x")).unwrap();
        let mut f = OpenOptions::new().append(true).open(&index).unwrap();
        f.write_all(b"{\"id\":999,\"t\":2,\"p\":\"a\",\"k\":\"sa").unwrap();
        drop(f);

        let mut s = Store::open(tmp.path()).unwrap();
        assert_eq!(s.len(), 1);
        assert!(std::fs::read(&index).unwrap().ends_with(b"\n"));
        let next = s.record(rev("a", Kind::Disk, 3, b"y")).unwrap().unwrap();
        assert_eq!(next.id, 2);
        let s = Store::open(tmp.path()).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.latest("a").unwrap().id, next.id);
        assert_eq!(s.loose_lines, 0);

        // A whole line whose newline never made it is kept, and terminated.
        let mut f = OpenOptions::new().append(true).open(&index).unwrap();
        f.write_all(br#"{"id":7,"t":4,"p":"b","k":"label","l":"kept"}"#).unwrap();
        drop(f);
        let mut s = Store::open(tmp.path()).unwrap();
        assert_eq!(s.len(), 3);
        let e = s.record(rev("a", Kind::Disk, 5, b"z")).unwrap().unwrap();
        assert_eq!(e.id, 8);
        let s = Store::open(tmp.path()).unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s.get(7).unwrap().label.as_deref(), Some("kept"));
    }

    #[test]
    fn labels_and_history_queries() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        s.record(rev("src/a.rs", Kind::Save, 1, b"1")).unwrap();
        fn label<'a>(path: &'a str, text: &str, ts: i64) -> NewRevision<'a> {
            NewRevision { path, kind: Kind::Label, ts, content: None, label: Some(text.into()), by: None, who: None }
        }
        s.record(label("", "project label", 2)).unwrap();
        s.record(label("src", "src label", 3)).unwrap();
        s.record(label("docs", "docs label", 4)).unwrap();
        s.record(rev("src/a.rs", Kind::Disk, 5, b"2")).unwrap();
        s.record(rev("src/b.rs", Kind::Disk, 6, b"3")).unwrap();
        s.record(rev("docs/x.md", Kind::Base, 7, b"4")).unwrap();
        assert!(s.record(label("", "  ", 8)).is_err());

        let (h, more) = s.file_history("src/a.rs", 10, None);
        let texts: Vec<_> = h.iter().map(|e| e.label.clone().unwrap_or_else(|| format!("{:?}", e.kind))).collect();
        assert_eq!(texts, ["Disk", "src label", "project label", "Save"]);
        assert!(!more);
        let (h, more) = s.file_history("src/a.rs", 2, None);
        assert_eq!(h.len(), 2);
        assert!(more);
        let (h, _) = s.file_history("src/a.rs", 10, Some(h[1].id));
        assert_eq!(h.len(), 2);

        // Directory history: changes only (no base), and the labels above or in it.
        let (h, _) = s.dir_history("src", 10, None, |_| true);
        let paths: Vec<_> = h.iter().map(|e| (e.path.as_str(), e.kind)).collect();
        assert_eq!(paths, [("src/b.rs", Kind::Disk), ("src/a.rs", Kind::Disk), ("src", Kind::Label), ("", Kind::Label), ("src/a.rs", Kind::Save)]);
        let (h, _) = s.dir_history("", 10, None, |p| p != "src/b.rs");
        assert!(h.iter().all(|e| e.path != "src/b.rs" && e.kind != Kind::Base));
        assert_eq!(h.iter().filter(|e| e.kind.is_label()).count(), 3);
    }

    #[test]
    fn late_agent_hooks_attribute_the_disk_revision() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        let e = s.record(rev("a.rs", Kind::Disk, 100, b"agent wrote this")).unwrap().unwrap();
        let h = e.hash.clone().unwrap();
        assert!(s.attribute("a.rs", &h, 200, "t1", Some("Claude")).unwrap().is_none(), "too old");
        assert!(s.attribute("a.rs", "0".repeat(64).as_str(), 0, "t1", None).unwrap().is_none(), "other content");
        let fixed = s.attribute("a.rs", &h, 50, "t1", Some("Claude")).unwrap().unwrap();
        assert_eq!((fixed.kind, fixed.by.as_deref(), fixed.who.as_deref()), (Kind::Agent, Some("t1"), Some("Claude")));
        assert!(s.attribute("a.rs", &h, 50, "t2", None).unwrap().is_none(), "already attributed");
        // The fix survives a reload and is folded in by a rewrite.
        let reloaded = Store::open(tmp.path()).unwrap();
        assert_eq!(reloaded.get(e.id).unwrap().kind, Kind::Agent);
        let mut reloaded = reloaded;
        reloaded.prune(&Policy::default(), 100, |_| false).unwrap();
        let text = std::fs::read_to_string(tmp.path().join(INDEX)).unwrap();
        assert!(!text.contains("\"attr\"") && text.contains("\"agent\""), "{text}");
    }

    #[test]
    fn pruning_applies_age_versions_and_size() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        let now = 100 * DAY;
        // Old versions (8 days) of a.rs go; the recent ones stay.
        s.record(rev("a.rs", Kind::Disk, now - 8 * DAY, b"old")).unwrap();
        s.record(rev("a.rs", Kind::Disk, now - DAY, b"new")).unwrap();
        // 5 versions of b.rs with a cap of 3.
        for i in 0..5 {
            s.record(rev("b.rs", Kind::Save, now - 1000 + i, format!("v{i}").as_bytes())).unwrap();
        }
        // A deletion whose content is gone says nothing: it goes too.
        s.record(rev("c.rs", Kind::Disk, now - 9 * DAY, b"c")).unwrap();
        s.record(NewRevision { path: "c.rs", kind: Kind::Deleted, ts: now - DAY, content: None, label: None, by: None, who: None }).unwrap();
        let policy = Policy { max_versions: 3, ..Policy::default() };
        let blobs_before = blob_files(tmp.path());
        let stats = s.prune(&policy, now, |_| false).unwrap();
        assert_eq!(stats.removed_entries, 5);
        assert_eq!(blob_files(tmp.path()), blobs_before - stats.removed_blobs);
        let (h, _) = s.file_history("b.rs", 10, None);
        let versions: Vec<_> = h.iter().map(|e| String::from_utf8(s.read_blob(e.hash.as_deref().unwrap()).unwrap()).unwrap()).collect();
        assert_eq!(versions, ["v4", "v3", "v2"]);
        assert_eq!(s.file_history("a.rs", 10, None).0.len(), 1);
        assert!(s.latest("c.rs").is_none());
        // What was pruned stays pruned after a reload.
        assert_eq!(Store::open(tmp.path()).unwrap().len(), s.len());

        // Size cap: incompressible content, oldest first, down to 90% of the cap.
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        let mut seed = 1u64;
        for i in 0..10 {
            let data: Vec<u8> = (0..10_000)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (seed >> 33) as u8
                })
                .collect();
            s.record(rev(&format!("f{i}"), Kind::Disk, now - 100 + i, &data)).unwrap();
        }
        let total = s.stored_bytes();
        let policy = Policy { max_bytes: total / 2, ..Policy::default() };
        s.prune(&policy, now, |_| false).unwrap();
        assert!(s.stored_bytes() <= policy.max_bytes / 10 * 9, "{} > cap", s.stored_bytes());
        assert!(s.latest("f9").is_some() && s.latest("f0").is_none());
        // Paths that became sensitive are dropped entirely.
        s.prune(&Policy::default(), now, |p| p == "f9").unwrap();
        assert!(s.latest("f9").is_none());
        // Everything expired: the index goes.
        s.prune(&Policy::default(), now + 30 * DAY, |_| false).unwrap();
        assert!(s.is_empty());
        assert!(!tmp.path().join(INDEX).exists());
        assert_eq!(blob_files(tmp.path()), 0);
    }

    #[test]
    fn oversized_content_is_not_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        let big = vec![b'a'; MAX_FILE_BYTES as usize + 1];
        assert!(s.record(rev("big.txt", Kind::Save, 1, &big)).unwrap().is_none());
        assert!(s.is_empty());
    }
}

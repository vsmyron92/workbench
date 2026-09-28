//! REST: `/api/projects/{pid}/files/history/**`.
//!
//! * `GET  …/files/history?path=&limit=&before=` — a file's revisions and the labels
//!   that apply to it, newest first (`before`: an entry id, for the next page).
//! * `GET  …/files/history/dir?path=&limit=&before=` — changes of the files in a
//!   folder (`path` empty: the project, i.e. Recent Changes) and labels.
//! * `GET  …/files/history/revision?id=` — one revision with its text.
//! * `GET  …/files/history/diff?id=&against=previous|current|<id>&context=` — a
//!   unified diff (for agents and scripts; the UI diffs in Monaco).
//! * `POST …/files/history/label {path?, label}` — Put Label.
//! * `GET  …/files/history/stats` — what the project's history holds.
//!
//! Sensitive paths are never served, even when a pattern was added after a file was
//! recorded (the pruner then drops those revisions).

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use serde::{Deserialize, Serialize};

use super::store::{Entry, Kind, MAX_FILE_BYTES};
use super::{EntryOut, diff, policy, untracked_reason};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::files::content::{Decoded, decode_text};
use crate::files::{Sensitive, blocking, resolve};

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 2000;
/// Unified diffs are cut here (agents read them).
pub const MAX_DIFF_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistory {
    pub path: String,
    pub entries: Vec<EntryOut>,
    /// More entries older than the last one.
    pub truncated: bool,
    /// Why the file is not tracked: `sensitive`, `git`, `ignored`, `binary`, `tooLarge`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untracked: Option<&'static str>,
}

pub async fn file_history(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Query(q): Query<ListQuery>) -> ApiResult<Json<FileHistory>> {
    let r = resolve(&state, &pid, &q.path)?;
    if r.rel.is_empty() {
        return Err(ApiError::bad_request("path names a file; use …/history/dir for the project"));
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let st = state.clone();
    Ok(Json(blocking(move || file_history_blocking(&st, &r.project, &r.rel, limit, q.before)).await?))
}

pub(crate) fn file_history_blocking(state: &AppState, project: &crate::projects::Project, rel: &str, limit: usize, before: Option<u64>) -> ApiResult<FileHistory> {
    let untracked = untracked_reason(project, rel);
    if untracked == Some("sensitive") {
        return Ok(FileHistory { path: rel.to_string(), entries: vec![], truncated: false, untracked });
    }
    let (entries, truncated) = state.files.history.read(state, &project.id, |s| s.file_history(rel, limit, before))?;
    Ok(FileHistory { path: rel.to_string(), entries: entries.iter().map(EntryOut::from).collect(), truncated, untracked })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirHistory {
    pub path: String,
    pub entries: Vec<EntryOut>,
    pub truncated: bool,
}

pub async fn dir_history(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Query(q): Query<ListQuery>) -> ApiResult<Json<DirHistory>> {
    let r = resolve(&state, &pid, &q.path)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let st = state.clone();
    Ok(Json(blocking(move || dir_history_blocking(&st, &r.project, &r.rel, limit, q.before)).await?))
}

pub(crate) fn dir_history_blocking(state: &AppState, project: &crate::projects::Project, rel: &str, limit: usize, before: Option<u64>) -> ApiResult<DirHistory> {
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    let (entries, truncated) = state.files.history.read(state, &project.id, |s| s.dir_history(rel, limit, before, |p| !sensitive.matches(p)))?;
    Ok(DirHistory { path: rel.to_string(), entries: entries.iter().map(EntryOut::from).collect(), truncated })
}

#[derive(Deserialize)]
pub struct RevisionQuery {
    id: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub entry: EntryOut,
    /// The text (`null` for deletions and labels).
    pub content: Option<String>,
    /// Not valid UTF-8: shown lossily.
    pub lossy: bool,
    /// The revision of the same file before this one.
    pub previous: Option<EntryOut>,
}

/// One entry with its content, refusing sensitive paths.
pub(crate) fn load_revision(state: &AppState, project: &crate::projects::Project, id: u64) -> ApiResult<(Entry, Option<Vec<u8>>, Option<Entry>)> {
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    state.files.history.read(state, &project.id, |s| -> ApiResult<_> {
        let e = s.get(id).filter(|e| e.kind != Kind::Attr).ok_or_else(|| ApiError::not_found(format!("no local history entry {id}")))?.clone();
        if !e.kind.is_label() && sensitive.matches(&e.path) {
            return Err(ApiError::forbidden(format!("{} is marked sensitive", e.path)));
        }
        let content = match (&e.hash, e.kind.has_content()) {
            (Some(h), true) => Some(s.read_blob(h).map_err(|err| ApiError::not_found(format!("the content of entry {id} is gone ({err})")))?),
            _ => None,
        };
        let prev = s.previous(id).cloned();
        Ok((e, content, prev))
    })?
}

fn text_of(bytes: &[u8]) -> (String, bool) {
    match decode_text(bytes) {
        Decoded::Text { text, lossy, .. } => (text, lossy),
        Decoded::Binary => (String::from_utf8_lossy(bytes).into_owned(), true),
    }
}

pub async fn revision(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Query(q): Query<RevisionQuery>) -> ApiResult<Json<Revision>> {
    let project = state.projects.require(&pid)?;
    let st = state.clone();
    let (e, content, prev) = blocking(move || load_revision(&st, &project, q.id)).await?;
    let (content, lossy) = match content {
        Some(b) => {
            let (t, lossy) = text_of(&b);
            (Some(t), lossy)
        }
        None => (None, false),
    };
    Ok(Json(Revision { entry: EntryOut::from(&e), content, lossy, previous: prev.as_ref().map(EntryOut::from) }))
}

#[derive(Deserialize)]
pub struct DiffQuery {
    id: u64,
    /// `previous` (what this revision changed; default), `current` (this revision → the
    /// file on disk now) or another entry id of the same file.
    #[serde(default)]
    against: Option<String>,
    #[serde(default)]
    context: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDiff {
    pub path: String,
    /// The older side (`null`: nothing, e.g. the file's first revision).
    pub from: Option<EntryOut>,
    /// The newer side (`null`: the file on disk now).
    pub to: Option<EntryOut>,
    pub diff: String,
    pub truncated: bool,
}

pub async fn diff(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Query(q): Query<DiffQuery>) -> ApiResult<Json<HistoryDiff>> {
    let project = state.projects.require(&pid)?;
    let st = state.clone();
    Ok(Json(blocking(move || diff_blocking(&st, &project, q.id, q.against.as_deref().unwrap_or("previous"), q.context.unwrap_or(3).min(50))).await?))
}

pub(crate) fn diff_blocking(state: &AppState, project: &crate::projects::Project, id: u64, against: &str, context: usize) -> ApiResult<HistoryDiff> {
    let (e, content, prev) = load_revision(state, project, id)?;
    if e.kind.is_label() {
        return Err(ApiError::bad_request("a label has no content to compare"));
    }
    let this = content.as_deref().map(text_of).map(|t| t.0).unwrap_or_default();
    let (from, to, old, new) = match against {
        "previous" => {
            let old = match &prev {
                Some(p) if p.kind.has_content() => load_revision(state, project, p.id)?.1.as_deref().map(text_of).map(|t| t.0).unwrap_or_default(),
                _ => String::new(),
            };
            (prev.clone(), Some(e.clone()), old, this)
        }
        "current" => {
            let abs = crate::util::paths::resolve_in_root(&project.root, &e.path)?;
            let now = match std::fs::metadata(&abs) {
                Ok(md) if md.len() > MAX_FILE_BYTES * 2 => return Err(ApiError::bad_request("the file on disk is too large to compare")),
                Ok(_) => text_of(&std::fs::read(&abs)?).0,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => return Err(err.into()),
            };
            (Some(e.clone()), None, this, now)
        }
        other => {
            let other_id: u64 = other.parse().map_err(|_| ApiError::bad_request("against is previous, current or an entry id"))?;
            let (o, oc, _) = load_revision(state, project, other_id)?;
            if o.path != e.path {
                return Err(ApiError::bad_request("both revisions must be of the same file"));
            }
            let other_text = oc.as_deref().map(text_of).map(|t| t.0).unwrap_or_default();
            if o.id < e.id { (Some(o), Some(e.clone()), other_text, this) } else { (Some(e.clone()), Some(o), this, other_text) }
        }
    };
    let name = |x: &Option<Entry>, side: &str| match x {
        Some(x) => format!("{side}/{} (local history #{})", e.path, x.id),
        None if side == "b" => format!("b/{} (on disk now)", e.path),
        None => "/dev/null".to_string(),
    };
    let mut text = diff::unified(&old, &new, &name(&from, "a"), &name(&to, "b"), context);
    let truncated = text.len() > MAX_DIFF_BYTES;
    if truncated {
        let mut cut = MAX_DIFF_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… (diff truncated)\n");
    }
    Ok(HistoryDiff { path: e.path.clone(), from: from.as_ref().map(EntryOut::from), to: to.as_ref().map(EntryOut::from), diff: text, truncated })
}

#[derive(Deserialize)]
pub struct SessionQuery {
    /// The session's terminal id.
    by: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileOut {
    pub path: String,
    /// The version before the session's first edit (`null`: none recorded, a new file).
    pub before: Option<EntryOut>,
    pub first: EntryOut,
    pub last: EntryOut,
    pub edits: usize,
    /// The file's newest version.
    pub latest: EntryOut,
    /// Someone else changed it after the session's last edit.
    pub changed_since: bool,
    /// It is gone now.
    pub deleted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChanges {
    pub by: String,
    /// The session's title when it last edited.
    pub who: Option<String>,
    pub files: Vec<SessionFileOut>,
}

/// `GET …/files/history/session?by=<terminal id>`: what an agent session changed
/// (Review Changes). Sensitive files are left out.
pub async fn session(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Query(q): Query<SessionQuery>) -> ApiResult<Json<SessionChanges>> {
    if q.by.is_empty() || q.by.len() > 128 {
        return Err(ApiError::bad_request("by names a session's terminal id"));
    }
    let project = state.projects.require(&pid)?;
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    let st = state.clone();
    let by = q.by.clone();
    let files = blocking(move || st.files.history.read(&st, &project.id, |s| s.session_files(&by))).await?;
    let who = files.iter().rev().find_map(|f| f.last.who.clone());
    let files = files
        .into_iter()
        .filter(|f| !sensitive.matches(&f.path))
        .map(|f| SessionFileOut {
            changed_since: f.latest.id != f.last.id,
            deleted: f.latest.kind == Kind::Deleted,
            path: f.path,
            before: f.before.as_ref().map(EntryOut::from),
            first: EntryOut::from(&f.first),
            last: EntryOut::from(&f.last),
            edits: f.edits,
            latest: EntryOut::from(&f.latest),
        })
        .collect();
    Ok(Json(SessionChanges { by: q.by, who, files }))
}

#[derive(Deserialize)]
pub struct LabelBody {
    #[serde(default)]
    path: String,
    label: String,
}

pub async fn label(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Json(b): Json<LabelBody>) -> ApiResult<Json<EntryOut>> {
    let r = resolve(&state, &pid, &b.path)?;
    let e = super::put_label(&state, &pid, &r.rel, &b.label, Kind::Label).await?;
    Ok(Json(EntryOut::from(&e)))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub entries: usize,
    pub files: usize,
    /// Compressed bytes of the kept versions.
    pub bytes: u64,
    pub max_bytes: u64,
    pub retention_days: i64,
    pub max_versions: usize,
    pub max_file_bytes: u64,
}

pub async fn stats(State(state): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<Stats>> {
    state.projects.require(&pid)?;
    let st = state.clone();
    let (entries, files, bytes) = blocking(move || st.files.history.read(&st, &pid, |s| (s.len(), s.file_count(), s.stored_bytes()))).await?;
    let p = policy();
    Ok(Json(Stats {
        entries,
        files,
        bytes,
        max_bytes: p.max_bytes,
        retention_days: p.max_age_ms / (24 * 3600 * 1000),
        max_versions: p.max_versions,
        max_file_bytes: MAX_FILE_BYTES,
    }))
}

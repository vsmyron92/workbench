//! Read-only access by absolute path (`/api/fs/{read,stat,raw}?path=/abs`): agent
//! scratchpads, screenshots, `~/.claude` notes. Allowed only inside a project root
//! or one of the configured `extra_roots`; symlinks may not leave those roots.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use axum::http::HeaderMap;
use axum::response::Response;

use super::content::{FileContent, FileStat, ReadQuery, read_file, serve_file, stat_file};
use super::{Sensitive, blocking};
use crate::app::AppState;
use crate::config::expand_tilde;
use crate::error::{ApiError, ApiResult};
use crate::util;

/// Project roots and extra roots, each in its configured and its canonical spelling.
pub fn allowed_roots(state: &AppState) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = state.projects.list().iter().map(|p| p.root.clone()).collect();
    roots.extend(state.config.read().extra_roots.iter().map(|r| expand_tilde(r)));
    let canon: Vec<PathBuf> = roots.iter().filter_map(|r| util::os::path::canonicalize(r).ok()).collect();
    roots.extend(canon);
    roots.sort();
    roots.dedup();
    roots
}

/// A resolved absolute path plus whether it is sensitive (by the owning project's
/// patterns, or the built-in ones relative to its allowed root).
fn resolve_abs(state: &AppState, abs: &str) -> ApiResult<(PathBuf, bool)> {
    let roots = allowed_roots(state);
    let path = util::paths::resolve_absolute_in(&roots, abs)?;
    let sensitive = match state.projects.find_by_path(&path) {
        Some(p) => {
            let rel = util::paths::relative_to(&p.root, &path).unwrap_or_default();
            Sensitive::new(&p.config.project.sensitive).matches(&rel)
        }
        None => {
            let root = roots.iter().filter(|r| util::os::path::starts_with(&path, r)).max_by_key(|r| r.as_os_str().len());
            let rel = root.and_then(|r| util::paths::relative_to(r, &path)).unwrap_or_default();
            Sensitive::defaults().matches(&rel)
        }
    };
    Ok((path, sensitive))
}

fn require_path(q: &ReadQuery) -> ApiResult<&str> {
    if q.path.is_empty() {
        return Err(ApiError::bad_request("path is required"));
    }
    Ok(&q.path)
}

pub async fn read(State(state): State<AppState>, Query(q): Query<ReadQuery>) -> ApiResult<Json<FileContent>> {
    let (path, sensitive) = resolve_abs(&state, require_path(&q)?)?;
    let display = path.display().to_string();
    let allow = q.allow_sensitive;
    Ok(Json(blocking(move || read_file(&path, display, sensitive, allow, true)).await?))
}

pub async fn stat(State(state): State<AppState>, Query(q): Query<ReadQuery>) -> ApiResult<Json<FileStat>> {
    let (path, sensitive) = resolve_abs(&state, require_path(&q)?)?;
    let display = path.display().to_string();
    let with_etag = !sensitive || q.allow_sensitive;
    Ok(Json(blocking(move || stat_file(&path, display, with_etag)).await?))
}

pub async fn raw(State(state): State<AppState>, Query(q): Query<ReadQuery>, headers: HeaderMap) -> ApiResult<Response> {
    let (path, sensitive) = resolve_abs(&state, require_path(&q)?)?;
    if sensitive && !q.allow_sensitive {
        return Err(ApiError::forbidden("this file is marked sensitive"));
    }
    serve_file(path, &headers, q.download).await
}

#[derive(Deserialize)]
pub struct DirsQuery {
    /// Absolute or `~/…`; empty: the home directory.
    #[serde(default)]
    path: String,
    /// Include folders whose name starts with a dot.
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirList {
    /// The folder listed, resolved.
    path: String,
    parent: Option<String>,
    home: String,
    /// Subfolder names, case-insensitively sorted.
    dirs: Vec<String>,
    truncated: bool,
}

const MAX_DIRS: usize = 5000;

/// `GET /api/fs/dirs?path=`: the subfolders of any folder this user can read, for the
/// folder picker of "Add project". Names only, no files. It answers devices only:
/// agent tokens are not valid under `/api`.
pub async fn dirs(Query(q): Query<DirsQuery>) -> ApiResult<Json<DirList>> {
    let home = expand_tilde("~");
    let want = if q.path.trim().is_empty() { home.clone() } else { expand_tilde(q.path.trim()) };
    if !util::os::path::is_absolute_str(&want.to_string_lossy()) {
        return Err(ApiError::bad_request("the path must be absolute or start with ~/"));
    }
    // UNC and WSL paths on Windows: refused before anything connects to their server.
    util::os::support::require_local_root(&want)?;
    Ok(Json(blocking(move || list_dirs(&want, &home, q.hidden)).await?))
}

fn list_dirs(want: &std::path::Path, home: &std::path::Path, hidden: bool) -> ApiResult<DirList> {
    let path = util::os::path::canonicalize(want).map_err(|_| ApiError::not_found(format!("{} does not exist", want.display())))?;
    util::os::support::require_root(&path)?;
    let rd = std::fs::read_dir(&path).map_err(|e| ApiError::forbidden(format!("cannot read {}: {e}", path.display())))?;
    let mut dirs = vec![];
    let mut truncated = false;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (!hidden && name.starts_with('.')) || !std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        if dirs.len() >= MAX_DIRS {
            truncated = true;
            break;
        }
        dirs.push(name);
    }
    dirs.sort_by_key(|n| n.to_lowercase());
    let parent = path.parent().map(|p| p.display().to_string());
    Ok(DirList { path: path.display().to_string(), parent, home: home.display().to_string(), dirs, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_subfolders_only_hides_dot_folders_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["beta", "Alpha", ".hidden", "gamma/inner"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        let l = list_dirs(dir.path(), dir.path(), false).unwrap();
        assert_eq!(l.dirs, ["Alpha", "beta", "gamma"]);
        assert!(l.parent.is_some() && !l.truncated);
        assert_eq!(list_dirs(dir.path(), dir.path(), true).unwrap().dirs, [".hidden", "Alpha", "beta", "gamma"]);
        assert_eq!(list_dirs(&dir.path().join("nope"), dir.path(), false).unwrap_err().code, "not_found");
    }
}

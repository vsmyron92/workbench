//! Read-only access by absolute path (`/api/fs/{read,stat,raw}?path=/abs`): agent
//! scratchpads, screenshots, `~/.claude` notes. Allowed only inside a project root
//! or one of the configured `extra_roots`; symlinks may not leave those roots.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Query, State};
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
    let canon: Vec<PathBuf> = roots.iter().filter_map(|r| r.canonicalize().ok()).collect();
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
            let root = roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.as_os_str().len());
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

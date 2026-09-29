//! Tree operations (`POST …/files/op`) and uploads (`POST …/files/upload`).
//!
//! Nothing here ever overwrites: renames use `RENAME_NOREPLACE`, copies and uploads
//! create files exclusively, and deletes go to the trash. The project root and
//! anything inside `.git` are off limits.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::{Resolved, basename, blocking, in_git_dir, join_rel, resolve, resolve_entry, valid_name};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::util::os::fs::{copy_symlink, rename_noreplace, trash};

/// Largest single upload.
pub const MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// Entries a recursive copy may create before it gives up.
const MAX_COPY_ENTRIES: usize = 20_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpBody {
    /// `mkdir` | `create` | `rename` | `copy` | `delete`
    pub op: String,
    pub path: String,
    /// Destination (project-relative) for `rename` and `copy`.
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    pub ok: bool,
    /// The resulting path (the new one for rename/copy).
    pub path: String,
    /// For deletes: `gio`, `trash-spec` or `recycle-bin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trashed_with: Option<&'static str>,
}

fn writable(r: &Resolved, what: &str) -> ApiResult<()> {
    if r.rel.is_empty() {
        return Err(ApiError::forbidden(format!("cannot {what} the project root")));
    }
    if in_git_dir(&r.rel) {
        return Err(ApiError::forbidden("the .git directory is managed by git"));
    }
    if !valid_name(basename(&r.rel)) {
        return Err(ApiError::bad_request("invalid file name"));
    }
    Ok(())
}

pub async fn op(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Json(body): Json<OpBody>,
) -> ApiResult<Json<OpResult>> {
    // Rename, copy and delete act on the entry itself (a symlink is moved, copied or
    // trashed as a link), so its final component is not resolved; creating goes
    // through the path and must stay inside the project.
    let src = match body.op.as_str() {
        "rename" | "copy" | "delete" => resolve_entry(&state, &pid, &body.path)?,
        _ => resolve(&state, &pid, &body.path)?,
    };
    let done = |path: String| Json(OpResult { ok: true, path, trashed_with: None });
    match body.op.as_str() {
        "mkdir" => {
            writable(&src, "create")?;
            let (abs, rel) = (src.abs, src.rel);
            blocking(move || {
                if std::fs::symlink_metadata(&abs).is_ok() {
                    return Err(ApiError::conflict(format!("{rel} already exists")));
                }
                std::fs::create_dir_all(&abs)?;
                Ok(done(rel))
            })
            .await
        }
        "create" => {
            writable(&src, "create")?;
            let (abs, rel) = (src.abs, src.rel);
            blocking(move || {
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&abs) {
                    Ok(_) => Ok(done(rel)),
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(ApiError::conflict(format!("{rel} already exists"))),
                    Err(e) => Err(e.into()),
                }
            })
            .await
        }
        "rename" | "copy" => {
            writable(&src, if body.op == "rename" { "rename" } else { "copy" })?;
            let to = body.to.as_deref().ok_or_else(|| ApiError::bad_request("`to` is required"))?;
            let dst = resolve(&state, &pid, to)?;
            writable(&dst, "overwrite")?;
            if dst.abs.starts_with(&src.abs) && dst.abs != src.abs {
                return Err(ApiError::bad_request("cannot move or copy a folder into itself"));
            }
            let is_rename = body.op == "rename";
            let (from, to_abs, to_rel) = (src.abs, dst.abs, dst.rel);
            blocking(move || {
                if std::fs::symlink_metadata(&from).is_err() {
                    return Err(ApiError::not_found("source does not exist"));
                }
                let parent = to_abs.parent().ok_or_else(|| ApiError::bad_request("bad destination"))?;
                if !parent.is_dir() {
                    return Err(ApiError::bad_request("destination folder does not exist"));
                }
                if is_rename {
                    rename_noreplace(&from, &to_abs).map_err(|e| map_exists(e, &to_rel))?;
                } else {
                    if std::fs::symlink_metadata(&to_abs).is_ok() {
                        return Err(ApiError::conflict(format!("{to_rel} already exists")));
                    }
                    let mut budget = MAX_COPY_ENTRIES;
                    if let Err(e) = copy_recursive(&from, &to_abs, &mut budget) {
                        // Remove the partial copy (the destination did not exist before).
                        if e.kind() != ErrorKind::AlreadyExists {
                            let _ = match std::fs::symlink_metadata(&to_abs) {
                                Ok(m) if m.is_dir() => std::fs::remove_dir_all(&to_abs),
                                Ok(_) => std::fs::remove_file(&to_abs),
                                Err(_) => Ok(()),
                            };
                        }
                        return Err(map_exists(e, &to_rel));
                    }
                }
                Ok(done(to_rel))
            })
            .await
        }
        "delete" => {
            writable(&src, "delete")?;
            if std::fs::symlink_metadata(&src.abs).is_err() {
                return Err(ApiError::not_found(format!("{} does not exist", src.rel)));
            }
            let with = trash(&src.abs).await?;
            Ok(Json(OpResult { ok: true, path: src.rel, trashed_with: Some(with) }))
        }
        other => Err(ApiError::bad_request(format!("unknown op {other:?}"))),
    }
}

fn map_exists(e: std::io::Error, rel: &str) -> ApiError {
    if e.kind() == ErrorKind::AlreadyExists {
        ApiError::conflict(format!("{rel} already exists"))
    } else {
        e.into()
    }
}

/// Copy a file, symlink or directory tree without overwriting anything.
fn copy_recursive(from: &Path, to: &Path, budget: &mut usize) -> std::io::Result<()> {
    if *budget == 0 {
        return Err(std::io::Error::other(format!("copy stopped after {MAX_COPY_ENTRIES} entries")));
    }
    *budget -= 1;
    let md = std::fs::symlink_metadata(from)?;
    let ft = md.file_type();
    if ft.is_symlink() {
        copy_symlink(from, to)?;
    } else if ft.is_dir() {
        std::fs::create_dir(to)?;
        for ent in std::fs::read_dir(from)? {
            let ent = ent?;
            copy_recursive(&ent.path(), &to.join(ent.file_name()), budget)?;
        }
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(md.permissions().mode() & 0o7777))?;
    } else {
        let mut src = std::fs::File::open(from)?;
        let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
        if let Err(e) = std::io::copy(&mut src, &mut dst) {
            drop(dst);
            let _ = std::fs::remove_file(to);
            return Err(e);
        }
        dst.set_permissions(std::fs::Permissions::from_mode(md.permissions().mode() & 0o7777))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- upload

#[derive(Deserialize)]
pub struct UploadQuery {
    #[serde(default)]
    pub dir: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    /// Project-relative path of the created file.
    pub path: String,
    /// The name actually used (`photo (2).png` when `photo.png` existed).
    pub name: String,
    pub size: u64,
}

/// `name (n).ext` for the n-th collision (`archive (2).tar.gz` keeps double extensions).
pub fn unique_name(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    let lower = name.to_ascii_lowercase();
    let split = [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"]
        .iter()
        .find(|ext| lower.ends_with(*ext) && lower.len() > ext.len())
        .map(|ext| name.len() - ext.len())
        .or_else(|| name.rfind('.').filter(|&i| i > 0));
    match split {
        Some(i) => format!("{} ({n}){}", &name[..i], &name[i..]),
        None => format!("{name} ({n})"),
    }
}

/// Streams the raw request body into a new file in `dir` (≤ 200 MB).
pub async fn upload(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> ApiResult<Json<UploadResult>> {
    if !valid_name(&q.name) {
        return Err(ApiError::bad_request("invalid file name"));
    }
    let dir = resolve(&state, &pid, &q.dir)?;
    if in_git_dir(&dir.rel) {
        return Err(ApiError::forbidden("the .git directory is managed by git"));
    }
    if !tokio::fs::metadata(&dir.abs).await.map(|m| m.is_dir()).unwrap_or(false) {
        return Err(ApiError::bad_request(format!("{:?} is not a folder", dir.rel)));
    }
    let mut chosen = None;
    for n in 1..10_000u32 {
        let name = unique_name(&q.name, n);
        match tokio::fs::OpenOptions::new().write(true).create_new(true).open(dir.abs.join(&name)).await {
            Ok(f) => {
                chosen = Some((name, f));
                break;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let (name, mut file) = chosen.ok_or_else(|| ApiError::conflict("no free file name"))?;
    let path = dir.abs.join(&name);
    let result: ApiResult<u64> = async {
        let mut total = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::bad_request(format!("upload interrupted: {e}")))?;
            total += chunk.len() as u64;
            if total > MAX_UPLOAD_BYTES {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "too_large",
                    format!("uploads are limited to {} MB", MAX_UPLOAD_BYTES / 1024 / 1024),
                ));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(total)
    }
    .await;
    match result {
        Ok(size) => Ok(Json(UploadResult { path: join_rel(&dir.rel, &name), name, size })),
        Err(e) => {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        assert_eq!(unique_name("a.png", 1), "a.png");
        assert_eq!(unique_name("a.png", 2), "a (2).png");
        assert_eq!(unique_name("backup.tar.gz", 3), "backup (3).tar.gz");
        assert_eq!(unique_name(".env", 2), ".env (2)");
        assert_eq!(unique_name("Makefile", 2), "Makefile (2)");
        assert_eq!(unique_name("my.file.txt", 2), "my.file (2).txt");
    }

    #[test]
    fn copies_trees_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::write(src.join("inner/x.sh"), "echo").unwrap();
        std::fs::set_permissions(src.join("inner/x.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::util::os::fs::symlink("inner/x.sh", src.join("link")).unwrap();
        let mut budget = 100;
        copy_recursive(&src, &dir.path().join("dst"), &mut budget).unwrap();
        let x = dir.path().join("dst/inner/x.sh");
        assert_eq!(std::fs::read_to_string(&x).unwrap(), "echo");
        assert_eq!(std::fs::metadata(&x).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(std::fs::symlink_metadata(dir.path().join("dst/link")).unwrap().file_type().is_symlink());
        // Copying onto an existing destination fails.
        let mut budget = 100;
        assert_eq!(copy_recursive(&src, &dir.path().join("dst"), &mut budget).unwrap_err().kind(), ErrorKind::AlreadyExists);
        // The budget caps huge trees.
        let mut budget = 2;
        assert!(copy_recursive(&src, &dir.path().join("dst2"), &mut budget).is_err());
    }
}

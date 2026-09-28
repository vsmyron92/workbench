//! Path containment: every file API resolves client paths through here so a
//! request can never escape the project root (or an allowed extra root).

use std::path::{Component, Path, PathBuf};

use crate::error::ApiError;

/// Lexically normalize `rel` (a client-supplied path relative to `root`) and join
/// it to `root`. Rejects absolute paths and any `..` that would climb above `root`.
/// Then, for paths that exist, re-check containment after resolving symlinks
/// (a symlink inside the project pointing outside is refused).
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let rel = rel.trim_start_matches("./");
    if rel.contains('\0') {
        return Err(ApiError::bad_request("path contains NUL"));
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(ApiError::bad_request("expected a path relative to the project root"));
    }
    let mut out = PathBuf::new();
    for comp in rel_path.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(ApiError::forbidden("path escapes the project root"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ApiError::bad_request("expected a relative path"));
            }
        }
    }
    let joined = root.join(&out);
    check_contained(root, &joined)?;
    Ok(joined)
}

/// Like [`resolve_in_root`], for operations on the directory entry itself that never
/// follow it: delete (to the trash), rename, and the source of a copy. The parent
/// must resolve inside `root`; the final component is not resolved, so a symlink in
/// the project that points outside it can still be trashed or moved as a link.
/// Anything that reads or writes through the path must use [`resolve_in_root`].
pub fn resolve_entry_in_root(root: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let trimmed = rel.trim_start_matches("./");
    if trimmed.contains('\0') {
        return Err(ApiError::bad_request("path contains NUL"));
    }
    let p = Path::new(trimmed);
    // `file_name` is `None` for "", "/" and paths ending in "..": no entry of its own.
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
        return resolve_in_root(root, rel);
    };
    let dir = resolve_in_root(root, &parent.to_string_lossy())?;
    Ok(dir.join(name))
}

/// Accept an absolute path only when it lies inside one of `roots`.
pub fn resolve_absolute_in(roots: &[PathBuf], abs: &str) -> Result<PathBuf, ApiError> {
    let p = Path::new(abs);
    if !p.is_absolute() {
        return Err(ApiError::bad_request("expected an absolute path"));
    }
    for root in roots {
        if let Ok(rel) = p.strip_prefix(root) {
            return resolve_in_root(root, &rel.to_string_lossy());
        }
    }
    Err(ApiError::forbidden("path is outside every project and allowed root"))
}

/// For existing paths (or their nearest existing ancestor), the canonical path
/// must stay under the canonical root.
fn check_contained(root: &Path, joined: &Path) -> Result<(), ApiError> {
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut probe = joined.to_path_buf();
    loop {
        if let Ok(canon) = probe.canonicalize() {
            if canon.starts_with(&canon_root) {
                return Ok(());
            }
            return Err(ApiError::forbidden("path resolves outside the project root"));
        }
        if !probe.pop() {
            return Ok(());
        }
    }
}

/// Path of `abs` relative to `root`, with `/` separators, or `None` if outside.
pub fn relative_to(root: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(root).ok().map(|p| p.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escapes_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        assert!(resolve_in_root(root, "a/b/../c.txt").is_ok());
        assert!(resolve_in_root(root, "../etc/passwd").is_err());
        assert!(resolve_in_root(root, "a/../../x").is_err());
        assert!(resolve_in_root(root, "/etc/passwd").is_err());
        assert_eq!(resolve_in_root(root, "").unwrap(), root.to_path_buf());
    }

    #[test]
    fn rejects_symlinks_that_leave_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert!(resolve_in_root(dir.path(), "link/secret").is_err());
    }

    #[test]
    fn entries_are_resolved_without_following_the_final_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("a/link")).unwrap();
        // The link itself is an entry of the project…
        assert!(resolve_in_root(root, "link").is_err());
        assert_eq!(resolve_entry_in_root(root, "link").unwrap(), root.join("link"));
        assert_eq!(resolve_entry_in_root(root, "./a/../a/link").unwrap(), root.join("a/link"));
        assert_eq!(resolve_entry_in_root(root, "a/x.txt").unwrap(), root.join("a/x.txt"));
        // …but nothing below it is, and escapes are refused as before.
        assert!(resolve_entry_in_root(root, "link/secret").is_err());
        assert!(resolve_entry_in_root(root, "link/sub/secret").is_err());
        assert!(resolve_entry_in_root(root, "../x").is_err());
        assert!(resolve_entry_in_root(root, "a/../../x").is_err());
        assert!(resolve_entry_in_root(root, "/etc/passwd").is_err());
        assert!(resolve_entry_in_root(root, "/").is_err());
        assert!(resolve_entry_in_root(root, "a/\0").is_err());
        assert_eq!(resolve_entry_in_root(root, "a/..").unwrap(), root.to_path_buf());
        assert_eq!(resolve_entry_in_root(root, "").unwrap(), root.to_path_buf());
    }
}

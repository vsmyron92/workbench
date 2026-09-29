//! Path containment: every file API resolves client paths through here so a
//! request can never escape the project root (or an allowed extra root).

use std::path::{Component, Path, PathBuf};

use crate::error::ApiError;
use crate::util::os;

/// Lexically normalize `rel` (a client-supplied path relative to `root`) and join
/// it to `root`. Rejects absolute paths and any `..` that would climb above `root`.
/// Then, for paths that exist, re-check containment after resolving symlinks
/// (a symlink inside the project pointing outside is refused). On Windows `rel` also
/// keeps to `/` separators and names that mean one file (`os::path::check_relative`),
/// and UNC roots are refused.
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let rel = rel.trim_start_matches("./");
    if rel.contains('\0') {
        return Err(ApiError::bad_request("path contains NUL"));
    }
    if let Some(why) = os::path::unsupported_root(root) {
        return Err(ApiError::bad_request(why));
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(ApiError::bad_request("expected a path relative to the project root"));
    }
    os::path::check_relative(rel).map_err(ApiError::bad_request)?;
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
    // Before `Path` splits it: on Windows it would also split at `\`.
    os::path::check_relative(trimmed).map_err(ApiError::bad_request)?;
    let p = Path::new(trimmed);
    // `file_name` is `None` for "", "/" and paths ending in "..": no entry of its own.
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
        return resolve_in_root(root, rel);
    };
    let dir = resolve_in_root(root, &parent.to_string_lossy())?;
    Ok(dir.join(name))
}

/// Accept an absolute path only when it lies inside one of `roots` (on Windows
/// compared without regard to ASCII case: `c:\users\me` is `C:\Users\Me`).
pub fn resolve_absolute_in(roots: &[PathBuf], abs: &str) -> Result<PathBuf, ApiError> {
    let p = Path::new(abs);
    if !p.is_absolute() {
        return Err(ApiError::bad_request("expected an absolute path"));
    }
    for root in roots {
        if let Some(rel) = os::path::strip_prefix(p, root) {
            return resolve_in_root(root, &os::path::to_slash(rel));
        }
    }
    Err(ApiError::forbidden("path is outside every project and allowed root"))
}

/// For existing paths (or their nearest existing ancestor), the canonical path
/// must stay under the canonical root.
fn check_contained(root: &Path, joined: &Path) -> Result<(), ApiError> {
    let canon_root = os::path::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut probe = joined.to_path_buf();
    loop {
        if let Ok(canon) = os::path::canonicalize(&probe) {
            if os::path::starts_with(&canon, &canon_root) {
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
    os::path::strip_prefix(abs, root).map(|p| p.to_string_lossy().replace('\\', "/"))
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

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_leave_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        crate::util::os::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert!(resolve_in_root(dir.path(), "link/secret").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn entries_are_resolved_without_following_the_final_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a")).unwrap();
        crate::util::os::fs::symlink(outside.path(), root.join("link")).unwrap();
        crate::util::os::fs::symlink(outside.path(), root.join("a/link")).unwrap();
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

    /// Linux keeps its rules: `\`, `:`, device names and trailing dots are plain names.
    #[cfg(unix)]
    #[test]
    fn unix_names_are_not_windows_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in [r"a\..\..\x", "a:b", "NUL", "com1.txt", "x.", "x ", "GIT~1/config", r"C:\x"] {
            assert_eq!(resolve_in_root(root, name).unwrap(), root.join(name), "{name}");
        }
        assert_eq!(resolve_entry_in_root(root, r"a\b").unwrap(), root.join(r"a\b"));
        assert_eq!(resolve_absolute_in(&[root.to_path_buf()], &format!("{}/a\\b", root.display())).unwrap(), root.join(r"a\b"));
        let upper = root.display().to_string().to_uppercase();
        assert!(resolve_absolute_in(&[root.to_path_buf()], &format!("{upper}/x")).is_err());
        assert_eq!(relative_to(root, &root.join(r"a\b")).as_deref(), Some("a/b"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_cannot_leave_or_alias() {
        let dir = tempfile::tempdir().unwrap();
        let root = &crate::util::os::path::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        for bad in [
            r"..\x", r"a\..\..\x", r"a\b", r"C:\x", "C:x", "C:/x", r"\x", "/x", "a/b.txt:secret", "NUL", "a/com1.txt", "a/x.", "a/x ",
            "GIT~1/config", r"\\server\share\x",
        ] {
            assert!(resolve_in_root(root, bad).is_err(), "{bad}");
            assert!(resolve_entry_in_root(root, bad).is_err(), "{bad}");
        }
        assert_eq!(resolve_in_root(root, "a/b/../c.txt").unwrap(), root.join("a").join("c.txt"));
        assert_eq!(resolve_entry_in_root(root, "a/b").unwrap(), root.join("a").join("b"));
        // Case-insensitive roots, `/` or `\` in the absolute path, no escape through `..`.
        let lower = root.display().to_string().to_ascii_lowercase();
        let roots = [root.clone()];
        assert_eq!(resolve_absolute_in(&roots, &format!(r"{lower}\a\b")).unwrap(), root.join("a").join("b"));
        assert_eq!(resolve_absolute_in(&roots, &format!("{lower}/a/b")).unwrap(), root.join("a").join("b"));
        assert!(resolve_absolute_in(&roots, &format!(r"{lower}\..\x")).is_err());
        assert!(resolve_absolute_in(&roots, &format!(r"{lower}\a\x.txt:stream")).is_err());
        assert_eq!(relative_to(root, &root.join("a").join("b")).as_deref(), Some("a/b"));
        let unc = resolve_in_root(std::path::Path::new(r"\\wsl$\Ubuntu\home\u"), "x").unwrap_err();
        assert!(unc.message.contains("WSL"), "{}", unc.message);
    }

    /// A junction needs no privilege; canonicalize follows it, so it cannot leave the root.
    #[cfg(windows)]
    #[test]
    fn windows_junctions_that_leave_the_root_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        let link = dir.path().join("link");
        let ok = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(&link).arg(outside.path()).output().unwrap();
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        assert!(link.join("secret").is_file());
        assert!(resolve_in_root(dir.path(), "link/secret").is_err());
        assert!(resolve_in_root(dir.path(), "link").is_err());
        assert_eq!(resolve_entry_in_root(dir.path(), "link").unwrap(), dir.path().join("link"));
        assert!(resolve_entry_in_root(dir.path(), "link/secret").is_err());
    }
}

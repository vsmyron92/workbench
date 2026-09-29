//! `GET /api/projects/{pid}/files/list?path=` — one directory for the lazy tree.

use std::cmp::Ordering;
use std::path::Path;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use serde::{Deserialize, Serialize};

use super::gitignore::IgnoreChecker;
use super::{Sensitive, blocking, join_rel, mtime_ms, resolve};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Entries returned per directory; the rest is reported as `truncated`.
const MAX_ENTRIES: usize = 5000;
/// Names read before giving up on a pathological directory.
const MAX_SCAN: usize = 200_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    /// Project-relative, `/`-separated.
    pub path: String,
    /// `file` | `dir` | `symlink`
    pub kind: &'static str,
    pub size: u64,
    pub mtime: i64,
    /// Gitignored (or inside an ignored directory).
    pub ignored: bool,
    /// Dotfile.
    pub hidden: bool,
    /// Matches the project's sensitive patterns.
    pub sensitive: bool,
    /// For symlinks: what the link points at — `file`, `dir`, or `broken`
    /// (missing, or outside the project, which the file API refuses to follow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
    /// More than `MAX_ENTRIES` entries; `total` has the real count.
    pub truncated: bool,
    pub total: usize,
}

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
}

pub async fn list(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<Listing>> {
    let r = resolve(&state, &pid, &q.path)?;
    let sensitive = Sensitive::new(&r.project.config.project.sensitive);
    let root = r.project.root.clone();
    let listing = blocking(move || list_dir(&root, &r.abs, &r.rel, &sensitive)).await?;
    Ok(Json(listing))
}

struct Raw {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    target: Option<&'static str>,
}

pub fn list_dir(root: &Path, dir: &Path, rel: &str, sensitive: &Sensitive) -> ApiResult<Listing> {
    let md = std::fs::metadata(dir)?;
    if !md.is_dir() {
        return Err(ApiError::bad_request(format!("{rel:?} is not a directory")));
    }
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut raw: Vec<Raw> = vec![];
    let mut total = 0usize;
    for ent in std::fs::read_dir(dir)? {
        let Ok(ent) = ent else { continue };
        let name = ent.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        total += 1;
        if raw.len() >= MAX_SCAN {
            continue;
        }
        let Ok(ft) = ent.file_type() else { continue };
        let (is_dir, target) = if ft.is_symlink() {
            let target = match ent.path().canonicalize() {
                Ok(t) if !t.starts_with(&canon_root) => "broken",
                Ok(t) if t.is_dir() => "dir",
                Ok(_) => "file",
                Err(_) => "broken",
            };
            (target == "dir", Some(target))
        } else {
            (ft.is_dir(), None)
        };
        raw.push(Raw { name, is_dir, is_symlink: ft.is_symlink(), target });
    }
    raw.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    let truncated = total > MAX_ENTRIES;
    raw.truncate(MAX_ENTRIES);

    let checker = IgnoreChecker::for_dir(root, dir);
    let entries = raw
        .into_iter()
        .map(|r| {
            let abs = dir.join(&r.name);
            let md = std::fs::metadata(&abs).or_else(|_| std::fs::symlink_metadata(&abs)).ok();
            let path = join_rel(rel, &r.name);
            Entry {
                ignored: checker.is_ignored(&abs, r.is_dir),
                hidden: r.name.starts_with('.'),
                sensitive: sensitive.matches(&path),
                kind: if r.is_symlink {
                    "symlink"
                } else if r.is_dir {
                    "dir"
                } else {
                    "file"
                },
                size: md.as_ref().filter(|m| m.is_file()).map(|m| m.len()).unwrap_or(0),
                mtime: md.as_ref().map(mtime_ms).unwrap_or(0),
                target: r.target,
                name: r.name,
                path,
            }
        })
        .collect();
    Ok(Listing { path: rel.to_string(), entries, truncated, total })
}

/// Case-insensitive order where digit runs compare numerically (`file2` < `file10`).
/// Ties fall back to a case-sensitive comparison so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        if ab[i].is_ascii_digit() && bb[j].is_ascii_digit() {
            let si = i;
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let na = a[si..i].trim_start_matches('0');
            let nb = b[sj..j].trim_start_matches('0');
            let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            let ca = ab[i].to_ascii_lowercase();
            let cb = bb[j].to_ascii_lowercase();
            if ca != cb {
                return ca.cmp(&cb);
            }
            i += 1;
            j += 1;
        }
    }
    (ab.len() - i).cmp(&(bb.len() - j)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10.txt", "File2.txt", "file1.txt", "b", "A", "a", "file02.txt"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["A", "a", "b", "file1.txt", "File2.txt", "file02.txt", "file10.txt"]);
    }

    #[test]
    fn lists_dirs_first_hides_git_and_flags_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("README.md"), "hi").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        crate::util::os::fs::symlink(root.join("src"), root.join("src-link")).unwrap();
        crate::util::os::fs::symlink("/etc", root.join("etc-link")).unwrap();

        let l = list_dir(root, root, "", &Sensitive::defaults()).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["src", "src-link", "target", ".env", ".gitignore", "etc-link", "README.md"]);
        let get = |n: &str| l.entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(get("src").kind, "dir");
        assert!(get("target").ignored);
        assert!(!get("src").ignored);
        assert!(get(".env").sensitive && get(".env").hidden);
        assert_eq!(get("README.md").size, 2);
        assert_eq!(get("src-link").kind, "symlink");
        assert_eq!(get("src-link").target, Some("dir"));
        assert_eq!(get("etc-link").target, Some("broken"));
        assert!(!l.truncated);
        assert_eq!(l.total, 7);
    }

    #[test]
    fn caps_large_directories() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(MAX_ENTRIES + 10) {
            std::fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }
        let l = list_dir(dir.path(), dir.path(), "", &Sensitive::defaults()).unwrap();
        assert!(l.truncated);
        assert_eq!(l.entries.len(), MAX_ENTRIES);
        assert_eq!(l.total, MAX_ENTRIES + 10);
        assert_eq!(l.entries[0].name, "f0");
        assert_eq!(l.entries[1].name, "f1");
    }

    #[test]
    fn rejects_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "").unwrap();
        assert!(list_dir(dir.path(), &dir.path().join("a"), "a", &Sensitive::defaults()).is_err());
    }
}

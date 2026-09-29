//! Which repository a project is, and translating between project-relative paths
//! (what the UI and the other slices use) and repository-relative paths (what git
//! prints). They differ only when a project root is a subdirectory of a repository.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;

use super::cmd::{Git, GitOutput};
use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct Repo {
    /// Project id (for events).
    pub project_id: String,
    /// The working tree root (`git rev-parse --show-toplevel`).
    pub top: PathBuf,
    /// This worktree's git dir (MERGE_HEAD, rebase-merge… live here). For a linked
    /// worktree it is `<common>/worktrees/<name>`.
    pub git_dir: PathBuf,
    /// The shared git dir (refs, objects).
    #[allow(dead_code)]
    pub common_dir: PathBuf,
    /// Project root relative to `top`, with a trailing `/`, or empty.
    pub prefix: String,
}

pub(super) const DISCOVER_ARGS: [&str; 5] = ["rev-parse", "--show-toplevel", "--absolute-git-dir", "--git-common-dir", "--show-prefix"];

/// Why `root` is no repository Workbench can use: git's own words when it refuses a
/// folder another user owns (`cmd::unsafe_repository`, Windows), else "not a git
/// repository".
pub(super) fn discover_error(out: &GitOutput, root: &Path) -> ApiError {
    super::cmd::unsafe_repository(out).unwrap_or_else(|| {
        ApiError::new(StatusCode::NOT_FOUND, "not_a_repo", format!("{} is not a git repository", crate::config::contract_tilde(root)))
    })
}

impl Repo {
    /// Resolve the repository containing `root`.
    pub async fn discover(project_id: &str, root: &Path) -> Result<Self, ApiError> {
        let out = Git::read(root).args(DISCOVER_ARGS).run().await?;
        if !out.ok() {
            return Err(discover_error(&out, root));
        }
        let text = out.text();
        let mut lines = text.lines();
        let top = PathBuf::from(lines.next().unwrap_or_default());
        let git_dir = PathBuf::from(lines.next().unwrap_or_default());
        let common = lines.next().unwrap_or_default();
        let prefix = lines.next().unwrap_or_default().to_string();
        if top.as_os_str().is_empty() {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "not_a_repo", "bare repositories are not supported"));
        }
        // --git-common-dir is relative to the cwd when not absolute.
        let common_dir = {
            let p = PathBuf::from(common);
            if p.is_absolute() { p } else { root.join(p) }
        };
        let common_dir = crate::util::os::path::canonicalize(&common_dir).unwrap_or(common_dir);
        Ok(Self { project_id: project_id.to_string(), top, git_dir, common_dir, prefix })
    }

    pub fn arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Project-relative client path → repository-relative path. Rejects absolute
    /// paths, NULs and anything that escapes the working tree.
    pub fn to_repo(&self, rel: &str) -> Result<String, ApiError> {
        if rel.contains('\0') {
            return Err(ApiError::bad_request("path contains NUL"));
        }
        let rel = rel.trim_start_matches("./");
        if rel.is_empty() {
            return Err(ApiError::bad_request("empty path"));
        }
        let joined = format!("{}{}", self.prefix, rel);
        let p = Path::new(&joined);
        if p.is_absolute() {
            return Err(ApiError::bad_request("expected a project-relative path"));
        }
        let mut parts: Vec<String> = vec![];
        for c in p.components() {
            match c {
                Component::Normal(s) => {
                    // Windows: `a.rs:stream`, `NUL`, `GIT~1` (the short name of `.git`).
                    crate::util::os::path::check_component(&s.to_string_lossy()).map_err(ApiError::bad_request)?;
                    parts.push(s.to_string_lossy().into_owned())
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if parts.pop().is_none() {
                        return Err(ApiError::forbidden("path escapes the repository"));
                    }
                }
                _ => return Err(ApiError::bad_request("expected a relative path")),
            }
        }
        if parts.is_empty() {
            return Err(ApiError::bad_request("empty path"));
        }
        // Any `.git` component: this repository's git dir, but also a submodule's
        // gitfile or a nested repository's git dir (the files slice refuses the
        // same paths). Nothing git tracks lives there.
        if parts.iter().any(|p| crate::util::os::path::same_name(p, ".git")) {
            return Err(ApiError::forbidden("paths inside .git are not allowed"));
        }
        Ok(parts.join("/"))
    }

    /// Repository-relative path (as git prints it) → project-relative path. Paths
    /// outside a subdirectory project come back with `../` segments.
    pub fn to_project(&self, repo_rel: &str) -> String {
        if self.prefix.is_empty() {
            return repo_rel.to_string();
        }
        if let Some(rest) = repo_rel.strip_prefix(&self.prefix) {
            return rest.to_string();
        }
        let depth = self.prefix.trim_end_matches('/').split('/').count();
        let prefix_parts: Vec<&str> = self.prefix.trim_end_matches('/').split('/').collect();
        let path_parts: Vec<&str> = repo_rel.split('/').collect();
        let common = prefix_parts.iter().zip(&path_parts).take_while(|(a, b)| a == b).count();
        let ups = depth - common;
        format!("{}{}", "../".repeat(ups), path_parts[common..].join("/"))
    }

    /// Is this repository path inside the project (always true without a prefix)?
#[cfg(test)]
    pub fn in_project(&self, repo_rel: &str) -> bool {
        self.prefix.is_empty() || repo_rel.starts_with(&self.prefix)
    }

    /// The pathspec that limits whole-tree commands to the project (`.` at the top).
    pub fn scope(&self) -> String {
        if self.prefix.is_empty() { ".".into() } else { self.prefix.trim_end_matches('/').to_string() }
    }

    pub fn abs(&self, repo_rel: &str) -> PathBuf {
        self.top.join(repo_rel)
    }

    pub fn git(&self) -> Git {
        Git::read(&self.top)
    }

    pub fn git_w(&self) -> Git {
        Git::write(&self.top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(prefix: &str) -> Repo {
        Repo {
            project_id: "p".into(),
            top: "/r".into(),
            git_dir: "/r/.git".into(),
            common_dir: "/r/.git".into(),
            prefix: prefix.into(),
        }
    }

    #[test]
    fn translates_paths_for_root_projects() {
        let r = repo("");
        assert_eq!(r.to_repo("src/a.rs").unwrap(), "src/a.rs");
        assert_eq!(r.to_repo("./src/../b.rs").unwrap(), "b.rs");
        assert!(r.to_repo("../x").is_err());
        assert!(r.to_repo("/etc/passwd").is_err());
        assert!(r.to_repo(".git/config").is_err());
        assert!(r.to_repo("sub/.git").is_err());
        assert!(r.to_repo("vendor/lib/.git/config").is_err());
        assert!(r.to_repo("sub/x/../.git").is_err());
        assert_eq!(r.to_repo("a.git/x").unwrap(), "a.git/x");
        assert_eq!(r.to_project("src/a.rs"), "src/a.rs");
        // Windows spellings of `.git` and of other files.
        #[cfg(windows)]
        for bad in [".GIT/config", "GIT~1/config", r"sub\.git\config", "C:x", "a.rs:stream", "NUL"] {
            assert!(r.to_repo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn translates_paths_for_subdirectory_projects() {
        let r = repo("app/web/");
        assert_eq!(r.to_repo("src/a.ts").unwrap(), "app/web/src/a.ts");
        assert_eq!(r.to_repo("../server/x.rs").unwrap(), "app/server/x.rs");
        assert!(r.to_repo("../../../x").is_err());
        assert_eq!(r.to_project("app/web/src/a.ts"), "src/a.ts");
        assert_eq!(r.to_project("app/server/x.rs"), "../server/x.rs");
        assert_eq!(r.to_project("README.md"), "../../README.md");
        assert!(r.in_project("app/web/x"));
        assert!(!r.in_project("app/server/x"));
        assert_eq!(r.scope(), "app/web");
    }
}

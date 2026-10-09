//! Which repository of a project this is, and translating between project-relative
//! paths (what the UI and the other slices use) and repository-relative paths (what git
//! prints). They differ when a project root is a subdirectory of its repository
//! (`prefix`) and for a repository below the project root (`base`).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;

use super::cmd::{Git, GitOutput};
use crate::error::ApiError;
use crate::projects::{Project, ROOT_REPO};

#[derive(Debug, Clone)]
pub struct Repo {
    /// Project id (for events).
    pub project_id: String,
    /// Which repository of the project this is (`.`, or its directory below the root).
    pub id: String,
    /// Key of the state kept per repository (`Project::scope_key`): changelists,
    /// shelves, running operations.
    pub scope: String,
    /// The working tree root (`git rev-parse --show-toplevel`).
    pub top: PathBuf,
    /// This worktree's git dir (MERGE_HEAD, rebase-merge… live here). For a linked
    /// worktree it is `<common>/worktrees/<name>`.
    pub git_dir: PathBuf,
    /// The shared git dir (refs, objects).
    #[allow(dead_code)]
    pub common_dir: PathBuf,
    /// Project root relative to `top`, with a trailing `/`, or empty. Set for the repository
    /// the project root is in when the root is a subdirectory of it.
    pub prefix: String,
    /// `top` relative to the project root, with a trailing `/` (`services/api/`), or empty.
    /// Set for a repository below the project root.
    pub base: String,
    /// Directories (project-relative, no trailing `/`) of other repositories of the project
    /// inside this one's tree. Paths strictly inside them belong to those repositories.
    pub inner: Vec<String>,
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

/// Directories of the project's other repositories that lie inside repository `id`'s tree.
pub(super) fn inner_repos(project: &Project, id: &str) -> Vec<String> {
    project
        .repos
        .iter()
        .filter(|r| r.id != id && !r.is_root() && (id == ROOT_REPO || r.id.strip_prefix(id).is_some_and(|rest| rest.starts_with('/'))))
        .map(|r| r.id.clone())
        .collect()
}

impl Repo {
    /// Resolve the repository `project` is seen through (see `Project::scoped`): the
    /// working tree of a repository below the root, or the one containing the root.
    pub async fn discover(project: &Project) -> Result<Self, ApiError> {
        let inner = inner_repos(project, project.repo_id());
        match project.repo_entry().filter(|r| !r.is_root()) {
            None => Self::discover_at(&project.id, &project.root, ROOT_REPO, inner).await,
            Some(entry) => {
                let mut repo = Self::discover_at(&project.id, &entry.dir, &entry.id, inner).await?;
                // The entry is the top of a working tree; one that is none any more (its
                // `.git` is gone) would silently be the enclosing repository.
                use crate::util::os::path::{canonicalize, starts_with};
                let want = canonicalize(&entry.dir).unwrap_or_else(|_| entry.dir.clone());
                let top = canonicalize(&repo.top).unwrap_or_else(|_| repo.top.clone());
                if !(starts_with(&top, &want) && starts_with(&want, &top)) {
                    return Err(ApiError::new(
                        StatusCode::NOT_FOUND,
                        "not_a_repo",
                        format!("{} is not a git repository of its own any more", crate::config::contract_tilde(&entry.dir)),
                    ));
                }
                repo.base = format!("{}/", entry.id);
                repo.prefix = String::new();
                Ok(repo)
            }
        }
    }

    /// Resolve the repository containing `root` as the project `project_id`'s root
    /// repository.
    #[cfg(test)]
    pub async fn discover_root(project_id: &str, root: &Path) -> Result<Self, ApiError> {
        Self::discover_at(project_id, root, ROOT_REPO, vec![]).await
    }

    async fn discover_at(project_id: &str, root: &Path, id: &str, inner: Vec<String>) -> Result<Self, ApiError> {
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
        let scope = crate::projects::scope_key(project_id, id);
        Ok(Self { project_id: project_id.to_string(), id: id.to_string(), scope, top, git_dir, common_dir, prefix, base: String::new(), inner })
    }

    pub fn arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Project-relative client path → repository-relative path. Rejects absolute
    /// paths, NULs, anything that escapes the working tree and any path that belongs to
    /// another repository of the project.
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
        let elsewhere = |what: &str| ApiError::bad_request(format!("{rel} belongs to {what}: choose that repository first"));
        // `parts` are relative to the repository top for a repository holding the root and
        // to the project root for one below it. A path strictly inside another repository
        // of the project is that repository's (the directory itself is a plain path here:
        // a submodule's entry).
        let joined = parts.join("/");
        if self.inner.iter().any(|d| joined.strip_prefix(&format!("{}{d}", self.prefix)).is_some_and(|rest| rest.starts_with('/'))) {
            return Err(elsewhere("another repository of this project"));
        }
        if !self.base.is_empty() {
            let base: Vec<&str> = self.base.trim_end_matches('/').split('/').collect();
            if parts.len() <= base.len() || !parts.iter().zip(&base).all(|(a, b)| a == b) {
                return Err(elsewhere(&format!("another repository than {}", self.id)));
            }
            parts.drain(..base.len());
        }
        Ok(parts.join("/"))
    }

    /// Repository-relative path (as git prints it) → project-relative path. Paths
    /// outside a subdirectory project come back with `../` segments.
    pub fn to_project(&self, repo_rel: &str) -> String {
        if !self.base.is_empty() {
            return format!("{}{repo_rel}", self.base);
        }
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
            id: ".".into(),
            scope: "p".into(),
            top: "/r".into(),
            git_dir: "/r/.git".into(),
            common_dir: "/r/.git".into(),
            prefix: prefix.into(),
            base: String::new(),
            inner: vec![],
        }
    }

    /// The repository `services/api` below the project root, with a deeper one inside it.
    fn nested() -> Repo {
        Repo {
            id: "services/api".into(),
            scope: "p@services~2Fapi".into(),
            top: "/p/services/api".into(),
            git_dir: "/p/services/api/.git".into(),
            common_dir: "/p/services/api/.git".into(),
            base: "services/api/".into(),
            inner: vec!["services/api/vendor/lib".into()],
            ..repo("")
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

    #[test]
    fn translates_paths_for_repositories_below_the_root() {
        let r = nested();
        assert_eq!(r.to_repo("services/api/src/a.rs").unwrap(), "src/a.rs");
        assert_eq!(r.to_repo("./services/api/x/../y.rs").unwrap(), "y.rs");
        assert_eq!(r.to_repo("services/web/../api/z").unwrap(), "z");
        assert_eq!(r.to_project("src/a.rs"), "services/api/src/a.rs");
        assert_eq!(r.scope(), ".");
        // Another repository's path, the repository itself and anything above the root.
        for other in ["README.md", "services/web/a.rs", "services/api", "services/apix/a", "services", "../x", "services/api/../../a"] {
            assert!(r.to_repo(other).is_err(), "{other}");
        }
        assert_eq!(r.to_repo("services/web/a.rs").unwrap_err().status, StatusCode::BAD_REQUEST);
        assert!(r.to_repo("services/api/.git/config").is_err());
        // A repository nested in this one owns its paths; its directory is a plain entry.
        assert_eq!(r.to_repo("services/api/vendor/lib").unwrap(), "vendor/lib");
        let err = r.to_repo("services/api/vendor/lib/x.c").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("belongs to"), "{}", err.message);
    }

    #[test]
    fn a_root_repository_refuses_paths_inside_the_repositories_below_it() {
        let mut r = repo("");
        r.inner = vec!["services/api".into(), "web".into()];
        assert_eq!(r.to_repo("services/api").unwrap(), "services/api");
        assert_eq!(r.to_repo("services/other/a").unwrap(), "services/other/a");
        assert_eq!(r.to_repo("webby/a").unwrap(), "webby/a");
        for inside in ["services/api/a.rs", "web/x/y", "./web/../web/a"] {
            assert_eq!(r.to_repo(inside).unwrap_err().status, StatusCode::BAD_REQUEST, "{inside}");
        }
        // The project is a subfolder of its repository: inner directories are project-relative.
        let mut sub = repo("app/");
        sub.inner = vec!["web".into()];
        assert_eq!(sub.to_repo("web").unwrap(), "app/web");
        assert!(sub.to_repo("web/a.ts").is_err());
        assert_eq!(sub.to_repo("webx/a.ts").unwrap(), "app/webx/a.ts");
    }
}

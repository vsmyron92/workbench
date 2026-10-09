//! The git repositories of a project.
//!
//! A project is a directory. Usually it is one repository (its root is the working tree,
//! or sits inside one), but it may hold several: a folder of services that are cloned
//! side by side, a monorepo with a submodule, a root that is no repository at all. Each
//! repository is a [`RepoEntry`] with an id, the project-relative directory of its working
//! tree (`.` for the one the project root belongs to).
//!
//! The git, GitLab and GitHub slices work on one repository at a time. A request names it
//! (`?repo=<id>`, [`ProjectRegistry::require_repo`](super::ProjectRegistry::require_repo));
//! without one it is the project's *default* repository, so a project with a single
//! repository behaves as it always did. [`Project::scoped`] is the project seen through
//! one repository: the same id, root and secrets, but that repository's remote and forge
//! settings, so code that asks `Project::gitlab()` needs no other change.
//!
//! Repositories come from `[[repository]]` entries (`.workbench.toml`, the machine
//! overlay) and from a bounded search below the root (off with `nested_repos = false`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Project, RemoteInfo};
use crate::config::ProjectFile;
use crate::config::project::{Repo, Repository};
use crate::util;

/// The id of the repository the project root belongs to.
pub const ROOT_REPO: &str = ".";

/// How many directories below the root the search for repositories goes (a repository
/// at `a/b/c` is found, one at `a/b/c/d` only when configured).
const SEARCH_DEPTH: usize = 3;

/// The search stops after this many directory entries, so a huge tree cannot hold up
/// loading the project list.
const SEARCH_ENTRIES: usize = 4000;

/// Directories no repository is looked for in: dependencies and build output.
const SKIPPED: &[&str] = &["node_modules", "target", "dist", "build", "out", "venv", "site-packages", "__pycache__"];

/// One git repository of a project.
#[derive(Debug, Clone)]
pub struct RepoEntry {
    /// [`ROOT_REPO`], or the working tree's directory relative to the project root,
    /// separated by `/` (`services/api`). Stable, so a client can keep it.
    pub id: String,
    /// What the repository switcher shows.
    pub name: String,
    /// The directory git commands run in: the project root for [`ROOT_REPO`] (which may be
    /// a subfolder of the repository), else the working tree itself.
    pub dir: PathBuf,
    /// The remote's address, without credentials.
    pub remote: Option<RemoteInfo>,
    /// Remote name, default branch, GitLab, GitHub and CI settings: what `[repo]` holds for
    /// a single-repository project.
    pub config: Repo,
}

impl RepoEntry {
    pub fn is_root(&self) -> bool {
        self.id == ROOT_REPO
    }

    /// GitLab `(host, path)` of this repository (what `Project::gitlab` answers for a view of it).
    pub fn gitlab(&self) -> Option<(String, String)> {
        super::gitlab_of(Some(&self.config), self.remote.as_ref())
    }

    /// GitHub `(host, "owner/repo")` of this repository.
    pub fn github(&self) -> Option<(String, String)> {
        super::github_of(Some(&self.config), self.remote.as_ref())
    }
}

/// What the registry has to know about the forges config.toml names, to adopt a
/// self-hosted remote as GitLab or GitHub (see `adopt_configured_forge`).
pub struct ForgeHosts {
    pub gitlab: Option<String>,
    pub github: Option<String>,
}

/// The key of state that belongs to one repository (changelists, shelves, running
/// operations, forge caches): the project id for the root repository, so existing state
/// stays where it is, else `<id>@<repository id>` with every byte outside
/// `[A-Za-z0-9._-]` as `~XX`. `@` is no character of a project id, so keys never collide,
/// and the key is safe in a file name on every OS.
pub fn scope_key(project_id: &str, repo_id: &str) -> String {
    if repo_id == ROOT_REPO {
        return project_id.to_string();
    }
    let mut key = format!("{project_id}@");
    for b in repo_id.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => key.push(b as char),
            _ => key.push_str(&format!("~{b:02X}")),
        }
    }
    key
}

/// A `[[repository]] path` as a repository id: relative, inside the root, `/`-separated,
/// without `.`/`..` parts, and not the root itself (that one is `[repo]`).
pub fn normalize_path(path: &str) -> Result<String, String> {
    let p = path.trim().replace('\\', "/");
    if p.contains('\0') {
        return Err("the path contains a NUL".into());
    }
    let mut parts: Vec<&str> = vec![];
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("the path must stay inside the project".into()),
            _ => {
                util::os::path::check_component(part).map_err(|e| e.to_string())?;
                if util::os::path::same_name(part, ".git") {
                    return Err("the path names a .git folder".into());
                }
                parts.push(part);
            }
        }
    }
    if p.starts_with('/') || util::os::path::is_absolute_str(&p) {
        return Err("the path must be relative to the project root".into());
    }
    if parts.is_empty() {
        return Err("the project root is the repository `[repo]` describes".into());
    }
    Ok(parts.join("/"))
}

/// Is `dir` the top of a git working tree of its own (a `.git` folder, or a `.git` file
/// of a clone's submodule)? A linked worktree (`git worktree add`) is another checkout of
/// a repository, not another repository.
fn is_repo_top(dir: &Path, explicit: bool) -> bool {
    let dot = dir.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot) else { return false };
    if meta.is_dir() {
        return true;
    }
    if !meta.is_file() {
        return false;
    }
    if explicit {
        return true;
    }
    // The file says where the git directory is: below `worktrees/` for a linked worktree.
    // (The target is only compared, never opened.)
    let Ok(text) = std::fs::read_to_string(&dot) else { return false };
    text.lines()
        .find_map(|l| l.strip_prefix("gitdir:"))
        .is_some_and(|gd| !gd.trim().replace('\\', "/").split('/').any(|c| c == "worktrees"))
}

/// Repositories below `root`, as (id, directory), sorted by id. Symbolic links are not
/// followed; hidden folders and dependency or build folders are skipped; the search does
/// not go into a repository it found.
pub(super) fn find_nested(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = vec![];
    let mut seen = 0usize;
    let mut stack = vec![(root.to_path_buf(), String::new(), 0usize)];
    'walk: while let Some((dir, rel, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > SEARCH_ENTRIES {
                break 'walk;
            }
            // `DirEntry::file_type` does not follow links.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIPPED.contains(&name.as_str()) || util::os::path::check_component(&name).is_err() {
                continue;
            }
            let path = entry.path();
            // A `.git` that leads to another computer (Windows) is not opened.
            if util::os::path::leaves_machine_below(root, &path.join(".git")) {
                continue;
            }
            let id = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if is_repo_top(&path, false) {
                found.push((id, path));
            } else if depth + 1 < SEARCH_DEPTH {
                stack.push((path, id, depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// The remote of a repository's working tree, with the credentials of its URL dropped.
pub(super) async fn remote_of(dir: &Path, repo: &Repo) -> Option<RemoteInfo> {
    let name = Some(repo.remote.as_str()).filter(|r| !r.is_empty()).unwrap_or("origin");
    let url = util::git::remote_url(dir, name).await?;
    util::git::parse_remote(&url).map(|(host, path)| RemoteInfo { url: super::strip_credentials(&url), host, path })
}

/// The repositories of a project, the root one first, and warnings for what could not be
/// used. `root_remote` is the remote of the project root's own repository.
pub(super) async fn load(
    root: &Path,
    name: &str,
    config: &ProjectFile,
    root_remote: Option<RemoteInfo>,
    hosts: &ForgeHosts,
) -> (Vec<RepoEntry>, Vec<String>) {
    let mut warnings = vec![];
    let mut entries = vec![];
    if root.join(".git").exists() || util::git::in_work_tree(root).await {
        entries.push(RepoEntry {
            id: ROOT_REPO.into(),
            name: name.to_string(),
            dir: root.to_path_buf(),
            remote: root_remote,
            config: config.repo.clone().unwrap_or_default(),
        });
    }
    // id → (directory, the `[[repository]]` entry that names it)
    let mut wanted: BTreeMap<String, (PathBuf, Option<&Repository>)> = BTreeMap::new();
    for r in &config.repositories {
        let id = match normalize_path(&r.path) {
            Ok(id) => id,
            Err(e) => {
                warnings.push(format!("[[repository]] {:?} ignored: {e}", r.path));
                continue;
            }
        };
        match util::paths::resolve_in_root(root, &id) {
            Ok(dir) if is_repo_top(&dir, true) => {
                wanted.insert(id, (dir, Some(r)));
            }
            Ok(_) => warnings.push(format!("[[repository]] {id:?} ignored: it is not the top of a git repository")),
            Err(e) => warnings.push(format!("[[repository]] {id:?} ignored: {}", e.message)),
        }
    }
    if config.project.nested_repos != Some(false) {
        // Directory reads: not on an async worker.
        let search_root = root.to_path_buf();
        let found = tokio::task::spawn_blocking(move || find_nested(&search_root)).await.unwrap_or_default();
        for (id, dir) in found {
            wanted.entry(id).or_insert((dir, None));
        }
    }
    for (id, (dir, configured)) in wanted {
        entries.push(nested_entry(id, dir, configured, hosts).await);
    }
    (entries, warnings)
}

/// A repository below the root: what its checkout says (remote, GitLab/GitHub, CI),
/// under what `[[repository]]` sets.
async fn nested_entry(id: String, dir: PathBuf, configured: Option<&Repository>, hosts: &ForgeHosts) -> RepoEntry {
    // Detection reads only the repository's own files; it is never written to disk.
    let mut repo = crate::apps::detect(&dir).repo.unwrap_or_default();
    if let Some(c) = configured {
        repo.merge(c.repo.clone());
    }
    let remote = remote_of(&dir, &repo).await;
    if let Some(r) = &remote {
        let mut slot = Some(std::mem::take(&mut repo));
        super::adopt_forge(&mut slot, r, hosts.gitlab.as_deref(), hosts.github.as_deref());
        repo = slot.unwrap_or_default();
    }
    let name = configured
        .map(|c| c.name.trim())
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| id.clone());
    RepoEntry { id, name, dir, remote, config: repo }
}

impl Project {
    /// The repository this value is about: the project's default one, unless it came from
    /// [`Project::scoped`]. `None` for a folder in no repository.
    pub fn repo_entry(&self) -> Option<&RepoEntry> {
        self.repos.get(self.repo)
    }

    /// Id of [`Project::repo_entry`] ([`ROOT_REPO`] when there is none).
    pub fn repo_id(&self) -> &str {
        self.repo_entry().map_or(ROOT_REPO, |r| r.id.as_str())
    }

    /// Where git commands for this repository run: the project root, or the working tree
    /// of a repository below it.
    pub fn repo_dir(&self) -> &Path {
        self.repo_entry().map_or(self.root.as_path(), |r| r.dir.as_path())
    }

    /// Working trees of the repositories below the root, absolute: where the files slice
    /// walks on its own, since the root's ignore files do not govern a repository's content.
    pub fn nested_repo_dirs(&self) -> Vec<PathBuf> {
        self.repos.iter().filter(|r| !r.is_root()).map(|r| r.dir.clone()).collect()
    }

    /// The key of state kept per repository (see [`scope_key`]).
    pub fn scope_key(&self) -> String {
        scope_key(&self.id, self.repo_id())
    }

    /// This project seen through repository `id`: its remote and `[repo]` settings (so
    /// `gitlab()` and `github()` answer for that repository), and `repo_dir()` its
    /// working tree. `None` when the project has no such repository.
    pub fn scoped(&self, id: &str) -> Option<Project> {
        let i = self.repos.iter().position(|r| r.id == id)?;
        if i == self.repo {
            return Some(self.clone());
        }
        let entry = &self.repos[i];
        let mut view = self.clone();
        view.repo = i;
        view.config.repo = Some(entry.config.clone());
        view.remote = entry.remote.clone();
        Some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn scope_keys_are_file_safe_and_never_collide() {
        assert_eq!(scope_key("shop", ROOT_REPO), "shop");
        assert_eq!(scope_key("shop", "api"), "shop@api");
        assert_eq!(scope_key("shop", "services/api"), "shop@services~2Fapi");
        assert_ne!(scope_key("shop", "a/b"), scope_key("shop", "a~2Fb"));
        assert!(!scope_key("shop", "weird name:*?").contains(['/', ':', '*', '?', ' ']));
    }

    #[test]
    fn repository_paths_are_normalized_and_kept_inside() {
        assert_eq!(normalize_path("services/api").unwrap(), "services/api");
        assert_eq!(normalize_path("./services//api/").unwrap(), "services/api");
        assert_eq!(normalize_path(r"services\api").unwrap(), "services/api");
        for bad in ["", ".", "./", "..", "../x", "a/../b", "/etc", "a/.git", ".git"] {
            assert!(normalize_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn finds_nested_repositories_but_not_dependencies_worktrees_or_what_is_inside_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        make_repo(&root.join("web"));
        make_repo(&root.join("services/api"));
        make_repo(&root.join("web/vendored")); // inside a repository: not looked for
        make_repo(&root.join("node_modules/dep"));
        make_repo(&root.join(".hidden"));
        make_repo(&root.join("a/b/c/d/too-deep"));
        // A submodule's `.git` file is a repository; a linked worktree's is not.
        std::fs::create_dir_all(root.join("libs/sub")).unwrap();
        std::fs::write(root.join("libs/sub/.git"), "gitdir: ../../.git/modules/sub\n").unwrap();
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /r/.git/worktrees/wt\n").unwrap();
        let ids: Vec<String> = find_nested(root).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["libs/sub", "services/api", "web"]);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_when_searching() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        make_repo(&outside.path().join("elsewhere"));
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        make_repo(&dir.path().join("real"));
        let ids: Vec<String> = find_nested(dir.path()).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["real"]);
    }

    fn repo_entry(id: &str, gitlab: Option<&str>) -> RepoEntry {
        let mut config = Repo::default();
        config.gitlab = gitlab.map(|p| crate::config::project::GitLab { host: "gitlab.com".into(), path: p.into(), ..Default::default() });
        RepoEntry { id: id.into(), name: id.into(), dir: PathBuf::from("/p").join(id), remote: None, config }
    }

    fn project(entries: Vec<RepoEntry>) -> Project {
        let mut config = ProjectFile::default();
        config.repo = entries.first().map(|e| e.config.clone());
        Project {
            id: "shop".into(),
            name: "shop".into(),
            root: "/p".into(),
            config,
            remote: None,
            warnings: vec![],
            repo_secret_names: Default::default(),
            overlay_error: None,
            repos: std::sync::Arc::new(entries),
            repo: 0,
        }
    }

    #[test]
    fn a_scoped_project_answers_for_its_repository() {
        let p = project(vec![repo_entry(".", Some("acme/shop")), repo_entry("api", Some("acme/api")), repo_entry("docs", None)]);
        assert_eq!(p.gitlab().unwrap().1, "acme/shop");
        let api = p.scoped("api").unwrap();
        assert_eq!(api.gitlab().unwrap().1, "acme/api");
        assert_eq!((api.repo_id(), api.scope_key().as_str(), api.id.as_str()), ("api", "shop@api", "shop"));
        assert_eq!(api.repo_dir(), Path::new("/p/api"));
        // A repository without a forge does not inherit the root's.
        assert!(p.scoped("docs").unwrap().gitlab().is_none());
        assert!(p.scoped("nope").is_none());
        assert_eq!(p.scoped(".").unwrap().scope_key(), "shop");
    }
}

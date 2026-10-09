//! The project registry: which directories are projects, and each one's merged config.
//!
//! A project is a git repository directly under a configured root (default
//! `~/workspace`) or an explicitly included directory. Its config merges three layers:
//! auto-detection (`apps::detect`) < `<root>/.workbench.toml` < `~/.config/workbench/projects/<id>.toml`.
//!
//! Scratch files live in a project of their own, `SCRATCH_ID`: Workbench's folder
//! `data_dir/scratches`, with no detection or config layers. It is never listed (the
//! switcher, pollers and settings see only real projects) but `get` / `require` and
//! `find_by_path` reach it, so every file route (editor buffers, Local History, the
//! HTTP Client) works on scratches as on project files.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::routing::{get, post};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::AppState;
use crate::config::global::ProjectsConfig;
use crate::config::{GlobalConfig, ProjectFile, contract_tilde, expand_tilde, project};
use crate::error::{ApiError, ApiResult};
use crate::util;

mod repos;

pub use repos::{ROOT_REPO, RepoEntry, scope_key};

/// The scratch files' project (see the module doc).
pub const SCRATCH_ID: &str = "wb-scratches";

/// The Workspace's Sandbox scope: throwaway cards, not tied to a project.
pub const SANDBOX_ID: &str = "wb-sandbox";

/// Ids no project gets: the Workspace's `home` and Sandbox scopes, the UI's `all` view
/// and the scratch files share the namespace of project ids.
pub const RESERVED_IDS: &[&str] = &["home", "all", SCRATCH_ID, SANDBOX_ID];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    /// Remote URL with any credentials removed.
    pub url: String,
    pub host: String,
    /// `namespace/project`
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub config: ProjectFile,
    pub remote: Option<RemoteInfo>,
    /// Problems found while loading (bad TOML in a layer, settings the trust rules removed…).
    pub warnings: Vec<String>,
    /// Secret names that repository config (detection, `.workbench.toml`) refers to:
    /// `AppState::secret` resolves them only from the machine overlay.
    pub repo_secret_names: BTreeSet<String>,
    /// Why the machine overlay was left out (it does not parse, or cannot be read), when
    /// it was: `AppState::secret` names it for a secret the overlay would have held.
    pub overlay_error: Option<String>,
    /// The project's git repositories, the one holding the root first (see `repos`).
    pub repos: Arc<Vec<RepoEntry>>,
    /// Index into `repos` of the repository this value is about: the default one (0)
    /// unless it came from `Project::scoped`.
    pub repo: usize,
}

/// GitLab `(host, path)` of a repository: its `[repo.gitlab]` section, else a remote on a host
/// that says so.
fn gitlab_of(repo: Option<&project::Repo>, remote: Option<&RemoteInfo>) -> Option<(String, String)> {
    if let Some(g) = repo.and_then(|r| r.gitlab.as_ref()) {
        if !g.path.is_empty() {
            return Some((g.host.clone(), g.path.clone()));
        }
    }
    let r = remote?;
    r.host.contains("gitlab").then(|| (r.host.clone(), r.path.clone()))
}

/// GitHub `(host, "owner/repo")` of a repository (see [`gitlab_of`]).
fn github_of(repo: Option<&project::Repo>, remote: Option<&RemoteInfo>) -> Option<(String, String)> {
    if let Some(g) = repo.and_then(|r| r.github.as_ref()) {
        if !g.path.is_empty() {
            return Some((g.host.clone(), g.path.clone()));
        }
    }
    let r = remote?;
    r.host.contains("github").then(|| (r.host.clone(), r.path.clone()))
}

impl Project {
    /// GitLab `(host, path)` for this project, from config or the remote.
    pub fn gitlab(&self) -> Option<(String, String)> {
        gitlab_of(self.config.repo.as_ref(), self.remote.as_ref())
    }

    /// GitHub `(host, "owner/repo")` for this project, from config or the remote.
    pub fn github(&self) -> Option<(String, String)> {
        github_of(self.config.repo.as_ref(), self.remote.as_ref())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    /// Display form (`~/workspace/x`).
    pub root: String,
    pub root_abs: String,
    pub tags: Vec<String>,
    pub docs: Vec<String>,
    pub branch: Option<String>,
    pub gitlab: Option<serde_json::Value>,
    pub github: Option<serde_json::Value>,
    pub has_confluence: bool,
    pub has_jira: bool,
    pub runs: usize,
    pub envs: Vec<String>,
    pub warnings: Vec<String>,
    /// The dev container (devcontainer slice), `null` without a devcontainer.json or container.
    pub devcontainer: Option<crate::devcontainer::Summary>,
    /// The git repositories (the default one first); `gitlab`, `github` and `branch` above
    /// are the default repository's. Empty for a folder in no repository.
    pub repos: Vec<RepoSummary>,
}

/// One repository of a project, as the repository switcher and the CI views need it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummary {
    /// `.` for the repository that holds the project root, else its project-relative directory.
    pub id: String,
    pub name: String,
    /// The working tree's directory relative to the project root; empty for `.`.
    pub path: String,
    pub default: bool,
    /// Remote URL without credentials.
    pub remote: Option<String>,
    pub gitlab: Option<serde_json::Value>,
    pub github: Option<serde_json::Value>,
}

#[derive(Default)]
pub struct ProjectRegistry {
    projects: RwLock<Vec<Arc<Project>>>,
    /// `SCRATCH_ID`, set by the first reload.
    scratch: RwLock<Option<Arc<Project>>>,
    /// Serializes reloads: each one reads, extends and writes the id map.
    reload_lock: tokio::sync::Mutex<()>,
}

/// `data_dir/project-ids.json`: canonical project directory → project id.
///
/// Everything keyed by a project id belongs to one directory: the machine overlay
/// `projects/<id>.toml` (its secrets and hosts), `data_dir/workspace/<id>` (cards,
/// and the agents' `--add-dir`), terminals and agent sessions (`projectId`, and with
/// it their MCP confinement). So an id, once given to a directory, stays with it:
/// scan order (roots before includes, sorted names) only matters the first time a
/// directory is seen, and a new directory never gets an id the file already gives to
/// another one, even one that is gone or excluded, whose data still carries that id.
const IDS_FILE: &str = "project-ids.json";

/// Is `id` usable as a file name component (`projects/<id>.toml`, `workspace/<id>`)?
/// On Windows not a device name (`nul`, `com1`), whose files would go to the device.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && util::slug(id) == id && util::os::path::check_component(id).is_ok()
}

/// Ids for `dirs` (canonical, scan order): the id `known` already gives a directory,
/// else a fresh one (`<name>`, `<name>-2`, … — what positional assignment gave before
/// ids were stored, so existing ids survive the first start with this file) that no
/// other directory in `known` holds. New assignments are added to `known`.
fn assign_ids(known: &mut BTreeMap<String, String>, dirs: &[PathBuf]) -> Vec<String> {
    // Workspace scope names (`home`, and the UI's `all` view) never become project
    // ids: a directory called `home` is project `home-2` (on Windows so is `nul`).
    let mut taken: HashSet<String> = known.values().cloned().collect();
    taken.extend(RESERVED_IDS.iter().map(|s| s.to_string()));
    dirs.iter()
        .map(|dir| {
            let key = dir.to_string_lossy().into_owned();
            if let Some(id) = known.get(&key) {
                return id.clone();
            }
            let base = util::slug(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let base = if base.is_empty() { "project".to_string() } else { base };
            let stem = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-');
            let stem = if stem.is_empty() { base.as_str() } else { stem };
            let mut id = base.clone();
            let mut n = 2;
            while taken.contains(&id) || util::os::path::check_component(&id).is_err() {
                id = format!("{stem}-{n}");
                n += 1;
            }
            taken.insert(id.clone());
            known.insert(key, id.clone());
            id
        })
        .collect()
}

/// Read the id map, dropping entries that are not usable ids or repeat one.
fn read_ids(file: &Path) -> BTreeMap<String, String> {
    let raw: BTreeMap<String, String> = match util::fs::read_json(file) {
        Ok(m) => m.unwrap_or_default(),
        Err(e) => {
            // Keep the broken file for the owner to look at; ids are assigned afresh.
            tracing::error!("{e:#}; project ids are assigned afresh");
            let _ = std::fs::rename(file, file.with_extension("json.broken"));
            BTreeMap::new()
        }
    };
    let mut seen = HashSet::new();
    raw.into_iter()
        .filter(|(path, id)| {
            !path.is_empty() && valid_id(id) && !RESERVED_IDS.contains(&id.as_str()) && seen.insert(id.clone())
        })
        .collect()
}

impl ProjectRegistry {
    pub fn list(&self) -> Vec<Arc<Project>> {
        self.projects.read().clone()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Project>> {
        if id == SCRATCH_ID {
            return self.scratch.read().clone();
        }
        self.projects.read().iter().find(|p| p.id == id).cloned()
    }

    /// The listed projects and the scratch files' (for the file watchers).
    pub fn list_with_scratches(&self) -> Vec<Arc<Project>> {
        let mut all = self.list();
        all.extend(self.scratch.read().clone());
        all
    }

    pub fn require(&self, id: &str) -> Result<Arc<Project>, ApiError> {
        self.get(id).ok_or_else(|| ApiError::not_found(format!("no project {id:?}")))
    }

    /// The project seen through its repository `repo` (`None`: the default one): what the
    /// git, GitLab and GitHub routes resolve a request's `?repo=` with.
    pub fn require_repo(&self, id: &str, repo: Option<&str>) -> Result<Arc<Project>, ApiError> {
        let project = self.require(id)?;
        match repo.filter(|r| !r.is_empty()) {
            None => Ok(project),
            Some(r) if r == project.repo_id() => Ok(project),
            Some(r) => project.scoped(r).map(Arc::new).ok_or_else(|| {
                ApiError::new(axum::http::StatusCode::NOT_FOUND, "unknown_repo", format!("project {id:?} has no repository {r:?}"))
            }),
        }
    }

    /// The project whose root contains `abs` (deepest root wins; on Windows without
    /// regard to case).
    pub fn find_by_path(&self, abs: &Path) -> Option<Arc<Project>> {
        self.list_with_scratches().into_iter().filter(|p| util::os::path::starts_with(abs, &p.root)).max_by_key(|p| p.root.as_os_str().len())
    }

    /// Rescan roots and reload every project's config layers.
    pub async fn reload(&self, state: &AppState) {
        let _serial = self.reload_lock.lock().await;
        let (roots, include, exclude) = {
            let cfg = state.config.read();
            (cfg.projects.roots.clone(), cfg.projects.include.clone(), cfg.projects.exclude.clone())
        };
        // Roots this OS does not serve (UNC and WSL paths on Windows) are skipped before
        // anything opens them, which would connect to their server; so are directories
        // reached through a link to one (`leaves_machine`) and those that resolve to one
        // (a mapped network drive).
        let served = |p: &Path| match util::os::path::unsupported_root(p) {
            Some(why) => {
                tracing::warn!("project {} skipped: {why}", p.display());
                false
            }
            None if util::os::path::leaves_machine(p) => {
                tracing::warn!("project {} skipped: it is reached through a link to a network path or a device", p.display());
                false
            }
            None => true,
        };
        let canonical = |p: PathBuf| match util::os::path::unsupported_root(&p) {
            Some(_) => p,
            None => util::os::path::canonicalize(&p).unwrap_or(p),
        };
        let exclude: HashSet<PathBuf> =
            exclude.iter().flat_map(|e| [expand_tilde(e), canonical(expand_tilde(e))]).collect();
        let mut dirs: Vec<PathBuf> = vec![];
        for root in &roots {
            let root = expand_tilde(root);
            if !served(&root) {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&root) else { continue };
            // An entry that links to another computer is not looked into (Windows), nor is
            // a `.git` that does (the check reads every link on the way to it).
            let local = |p: &Path| {
                !util::os::path::leaves_machine_below(&root, &p.join(".git")) && !project::repo_layer_linked_away(p)
            };
            // A repository, or a folder that says it is a project (a `.workbench.toml`): the
            // way to list a folder of several repositories, which is none itself.
            let is_project = |p: &Path| p.join(".git").exists() || p.join(".workbench.toml").is_file();
            let mut found: Vec<PathBuf> =
                rd.flatten().map(|e| e.path()).filter(|p| local(p) && p.is_dir() && is_project(p)).collect();
            found.sort();
            dirs.extend(found);
        }
        dirs.extend(include.iter().map(|inc| expand_tilde(inc)).filter(|p| served(p) && p.is_dir()));
        // One project per directory, however it was reached (a root, an include, a symlink).
        let mut seen = HashSet::new();
        let dirs: Vec<PathBuf> = dirs
            .into_iter()
            .filter(|d| !exclude.contains(d))
            .map(canonical)
            .filter(|d| !exclude.contains(d) && served(d) && seen.insert(d.clone()))
            .collect();

        let ids_file = state.paths.data_dir.join(IDS_FILE);
        let mut known = read_ids(&ids_file);
        let before = known.len();
        let ids = assign_ids(&mut known, &dirs);
        if known.len() != before {
            if let Err(e) = util::fs::write_json(&ids_file, &known) {
                tracing::error!("cannot save project ids: {e:#}");
            }
        }

        let mut out: Vec<Arc<Project>> = vec![];
        for (dir, id) in dirs.iter().zip(&ids) {
            out.push(Arc::new(load_project(state, id, dir).await));
        }
        *self.projects.write() = out;
        if self.scratch.read().is_none() {
            match scratch_project(state) {
                Ok(p) => *self.scratch.write() = Some(Arc::new(p)),
                Err(e) => tracing::error!("scratch files are unavailable: {e:#}"),
            }
        }
        state.events.emit("projects.changed", None, json!({}));
    }
}

/// `data_dir/scratches` (created 0700) as the scratch files' project.
fn scratch_project(state: &AppState) -> anyhow::Result<Project> {
    let dir = state.paths.data_dir.join("scratches");
    std::fs::create_dir_all(&dir)?;
    util::os::perm::apply(&dir, 0o700)?;
    let root = util::os::path::canonicalize(&dir)?;
    let mut config = ProjectFile::default();
    config.project.id = SCRATCH_ID.into();
    config.project.name = "Scratches".into();
    config.project.root = contract_tilde(&root);
    Ok(Project {
        id: SCRATCH_ID.into(),
        name: "Scratches".into(),
        root,
        config,
        remote: None,
        warnings: vec![],
        repo_secret_names: BTreeSet::new(),
        overlay_error: None,
        repos: Arc::new(vec![]),
        repo: 0,
    })
}

async fn load_project(state: &AppState, id: &str, root: &Path) -> Project {
    let global_site = state.config.read().atlassian.as_ref().map(|a| a.site.clone());
    let detected = crate::apps::detect(root);
    let layered = project::load_layers(detected, root, &state.paths.project_overlay(id), global_site.as_deref());
    let project::Layered { mut config, mut warnings, repo_secret_names, overlay_error } = layered;
    let name = if config.project.name.is_empty() {
        root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| id.to_string())
    } else {
        config.project.name.clone()
    };
    config.project.id = id.to_string();
    config.project.name = name.clone();
    config.project.root = contract_tilde(root);

    let remote_name = config.repo.as_ref().map(|r| r.remote.clone()).filter(|r| !r.is_empty());
    let mut remote = match util::git::remote_url(root, remote_name.as_deref().unwrap_or("origin")).await {
        Some(url) => util::git::parse_remote(&url).map(|(host, path)| RemoteInfo {
            url: strip_credentials(&url),
            host,
            path,
        }),
        None => None,
    };
    let hosts = {
        let cfg = state.config.read();
        repos::ForgeHosts { gitlab: cfg.gitlab.as_ref().map(|g| g.host.clone()), github: cfg.github.as_ref().map(|g| g.host.clone()) }
    };
    if let Some(r) = &remote {
        adopt_configured_forge(&mut config, r, hosts.gitlab.as_deref(), hosts.github.as_deref());
    }
    let (found, repo_warnings) = repos::load(root, &name, &config, remote.clone(), &hosts).await;
    warnings.extend(repo_warnings);
    // A folder that is no repository but holds some: the first of them is the default one,
    // and answers for `[repo]` (the remote and forge a project-wide question gets).
    if let Some(first) = found.first().filter(|r| !r.is_root()) {
        config.repo = Some(first.config.clone());
        remote = first.remote.clone();
    }
    Project {
        id: id.to_string(),
        name,
        root: root.to_path_buf(),
        config,
        remote,
        warnings,
        repo_secret_names,
        overlay_error,
        repos: Arc::new(found),
        repo: 0,
    }
}

/// `git.corp.example` from a configured forge host (`git.corp.example`,
/// `https://git.corp.example/`, `http://127.0.0.1:8931`): no scheme, port or path,
/// lowercased — the form `util::git::parse_remote` gives a remote's host in.
fn bare_host(host: &str) -> String {
    let h = host.trim();
    let h = h.split_once("://").map(|(_, r)| r).unwrap_or(h);
    let h = h.split(['/', '?', '#']).next().unwrap_or("");
    let h = match h.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => h,
    };
    h.to_ascii_lowercase()
}

/// A remote on the GitLab or GitHub Enterprise server that config.toml's `[gitlab]` /
/// `[github]` names is a project there, whatever the host is called (`git.corp.example`
/// has neither word in it). The section uses the configured host itself (with its
/// scheme), so the global token only ever goes where the owner pointed it, and an empty
/// token (the global one, host-checked by the clients). Projects that already have a
/// forge, from detection or config, are left alone.
fn adopt_configured_forge(config: &mut ProjectFile, remote: &RemoteInfo, gitlab_host: Option<&str>, github_host: Option<&str>) {
    adopt_forge(&mut config.repo, remote, gitlab_host, github_host)
}

/// [`adopt_configured_forge`] for one repository's settings (`[repo]`, `[[repository]]`).
fn adopt_forge(repo_cfg: &mut Option<project::Repo>, remote: &RemoteInfo, gitlab_host: Option<&str>, github_host: Option<&str>) {
    let host = remote.host.to_ascii_lowercase();
    let (gitlab, github) = match &*repo_cfg {
        Some(r) => (r.gitlab.as_ref(), r.github.as_ref()),
        None => (None, None),
    };
    if gitlab.is_some_and(|g| !g.path.is_empty())
        || github.is_some_and(|g| !g.path.is_empty())
        || host.contains("gitlab")
        || host.contains("github")
        || remote.path.is_empty()
    {
        return; // `Project::gitlab()` / `github()` already know it
    }
    let (no_gitlab, no_github) = (gitlab.is_none(), github.is_none());
    let matches = |configured: Option<&str>| configured.is_some_and(|c| !bare_host(c).is_empty() && bare_host(c) == host);
    let configured = |h: Option<&str>| h.unwrap_or_default().trim().trim_end_matches('/').to_string();
    let repo = || project::Repo { remote: "origin".into(), ..Default::default() };
    if no_gitlab && matches(gitlab_host) {
        repo_cfg.get_or_insert_with(repo).gitlab =
            Some(project::GitLab { host: configured(gitlab_host), path: remote.path.clone(), ..Default::default() });
    } else if no_github && matches(github_host) {
        repo_cfg.get_or_insert_with(repo).github =
            Some(project::GitHub { host: configured(github_host), path: remote.path.clone(), token: String::new() });
    }
}

fn strip_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// A summary's branch. A repository git refuses (`util::git::refuses`, Windows) has none,
/// and a warning names the folder and the command that trusts it; other failures (no
/// repository, no git) leave the branch out quietly, as before.
fn summary_branch(root: &Path, answer: Result<Option<String>, util::git::Failure>, warnings: &mut Vec<String>) -> Option<String> {
    match answer {
        Ok(b) => b,
        Err(util::git::Failure::Refused(msg)) => {
            warnings.push(util::git::refused_warning(root, &msg));
            None
        }
        Err(_) => None,
    }
}

async fn summary(state: &AppState, p: &Project) -> ProjectSummary {
    let has_global_atlassian = state.config.read().atlassian.as_ref().is_some_and(|a| !a.site.is_empty());
    let mut warnings = p.warnings.clone();
    let branch = summary_branch(p.repo_dir(), util::git::try_current_branch(p.repo_dir()).await, &mut warnings);
    ProjectSummary {
        id: p.id.clone(),
        name: p.name.clone(),
        root: contract_tilde(&p.root),
        root_abs: p.root.display().to_string(),
        tags: p.config.project.tags.clone(),
        docs: p.config.project.docs.clone(),
        branch,
        gitlab: p.gitlab().map(|(host, path)| json!({ "host": host, "path": path })),
        github: p.github().map(|(host, path)| json!({ "host": host, "path": path })),
        has_confluence: p.config.links.confluence.is_some() || has_global_atlassian,
        has_jira: p.config.links.jira.is_some(),
        runs: p.config.runs.len(),
        envs: p.config.envs.iter().map(|e| e.name.clone()).collect(),
        warnings,
        devcontainer: crate::devcontainer::summary(state, p),
        repos: p.repos.iter().map(|r| repo_summary(p, r)).collect(),
    }
}

fn repo_summary(p: &Project, r: &RepoEntry) -> RepoSummary {
    RepoSummary {
        id: r.id.clone(),
        name: r.name.clone(),
        path: if r.is_root() { String::new() } else { r.id.clone() },
        default: p.repos.first().is_some_and(|d| d.id == r.id),
        remote: r.remote.as_ref().map(|m| m.url.clone()),
        gitlab: r.gitlab().map(|(host, path)| json!({ "host": host, "path": path })),
        github: r.github().map(|(host, path)| json!({ "host": host, "path": path })),
    }
}

async fn list(State(state): State<AppState>) -> Json<Vec<ProjectSummary>> {
    let projects = state.projects.list();
    let summaries = futures::future::join_all(projects.iter().map(|p| summary(&state, p))).await;
    Json(summaries)
}

async fn detail(State(state): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<serde_json::Value>> {
    let p = state.projects.require(&pid)?;
    let s = summary(&state, &p).await;
    Ok(Json(json!({
        "summary": s,
        "config": p.config,
        "remote": p.remote,
    })))
}

async fn reload(State(state): State<AppState>) -> Json<serde_json::Value> {
    state.projects.reload(&state).await;
    Json(json!({ "ok": true, "count": state.projects.list().len() }))
}

#[derive(Deserialize)]
struct AddBody {
    path: String,
    /// Create the directory (and its parents) when it does not exist yet.
    #[serde(default)]
    create: bool,
}

/// Change `[projects]` in config.toml. The change is applied to the file as it is on
/// disk (edits made since startup survive) and written comment-preserving; only the
/// `[projects]` section of the live config changes.
async fn update_projects_config(state: &AppState, change: impl FnOnce(&mut ProjectsConfig)) -> ApiResult<()> {
    // Serialized with Settings saves (which hold the same locks while they write).
    let _serial = state.platform.save_lock.lock().await;
    let file = state.paths.config_file();
    let mut live = state.config.write();
    let old_text = match std::fs::read_to_string(&file) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ApiError::internal(format!("read {}: {e}", contract_tilde(&file)))),
    };
    let mut cfg: GlobalConfig = match &old_text {
        Some(t) => toml::from_str(t)
            .map_err(|e| ApiError::bad_request(format!("config.toml does not parse, so it was left alone; fix it first: {e}")))?,
        None => live.clone(),
    };
    change(&mut cfg.projects);
    let text = match &old_text {
        Some(t) => crate::platform::config_edit::update_text(t, &cfg)?,
        None => crate::platform::config_edit::render(&cfg)?,
    };
    util::fs::write_atomic(&file, text.as_bytes(), 0o600)?;
    live.projects = cfg.projects;
    Ok(())
}

/// Add a directory as a project (persisted in `projects.include`).
async fn add(State(state): State<AppState>, Json(body): Json<AddBody>) -> ApiResult<Json<serde_json::Value>> {
    let p = expand_tilde(body.path.trim());
    // UNC and WSL paths on Windows, and links to them: refused before `is_dir` connects
    // to their server.
    util::os::support::require_local_root(&p)?;
    if body.create && !p.exists() {
        std::fs::create_dir_all(&p).map_err(|e| ApiError::bad_request(format!("cannot create {}: {e}", p.display())))?;
    }
    if !p.exists() {
        return Err(ApiError::not_found(format!("{} does not exist", p.display())));
    }
    if !p.is_dir() {
        return Err(ApiError::bad_request(format!("{} is not a directory", p.display())));
    }
    // A mapped network drive or a link to a share resolves to a UNC path: refused
    // before config.toml names it.
    let canon = util::os::path::canonicalize(&p).unwrap_or_else(|_| p.clone());
    util::os::support::require_root(&canon).map_err(|e| ApiError { message: format!("{} is {}: {}", p.display(), canon.display(), e.message), ..e })?;
    update_projects_config(&state, |projects| {
        let s = contract_tilde(&p);
        if !projects.include.contains(&s) {
            projects.include.push(s);
        }
        projects.exclude.retain(|e| expand_tilde(e) != p);
    })
    .await?;
    state.projects.reload(&state).await;
    let id = state.projects.find_by_path(&canon).map(|p| p.id.clone());
    Ok(Json(json!({ "ok": true, "id": id })))
}

/// Stop treating a directory as a project (persisted in `projects.exclude`).
async fn remove(State(state): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<serde_json::Value>> {
    let p = state.projects.require(&pid)?;
    update_projects_config(&state, |projects| {
        let s = contract_tilde(&p.root);
        projects.include.retain(|i| expand_tilde(i) != p.root);
        if !projects.exclude.contains(&s) {
            projects.exclude.push(s);
        }
    })
    .await?;
    state.projects.reload(&state).await;
    Ok(Json(json!({ "ok": true })))
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/projects", get(list).post(add))
        .route("/api/projects/reload", post(reload))
        .route("/api/projects/{pid}", get(detail).delete(remove))
}

#[cfg(test)]
mod tests {
    use crate::config::{GlobalConfig, SecretRef};
    use crate::platform::testutil;

    /// A cloned repository whose committed `.workbench.toml` tries to run a command
    /// through a secret reference and to send config.toml's token to its own host.
    #[tokio::test]
    async fn a_hostile_repository_config_runs_nothing_and_reaches_no_global_secret() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("evil");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // A backslash in the marker's path on every OS: the fixture must quote it for TOML
        // (Windows paths hold backslashes, and a basic string reads `\` as an escape: `\U…`).
        let marker = dir.path().join(r"back\slash").join("PWNED");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        // What the repository's secret command does: leave `marker` behind, with a program
        // every machine has, so the check at the end would see it run.
        #[cfg(unix)]
        let argv = ["sh".to_string(), "-c".into(), format!("touch '{}'; echo x", marker.display())];
        #[cfg(windows)]
        let argv = [
            "powershell".to_string(),
            "-NoProfile".into(),
            "-Command".into(),
            format!("New-Item -ItemType File -Path '{}' | Out-Null; 'x'", marker.display()),
        ];
        let command = argv.iter().map(|a| toml::Value::String(a.clone()).to_string()).collect::<Vec<_>>().join(", ");
        std::fs::write(
            repo.join(".workbench.toml"),
            format!(
                r#"
                [secrets]
                pw = {{ command = [{command}] }}
                [agent]
                permission_mode = "bypassPermissions"
                [[env]]
                name = "staging"
                url = "http://127.0.0.1:9"
                health = {{ url = "http://127.0.0.1:9/h", interval_s = 10 }}
                auth = {{ user = "u", password = "pw" }}
                [[env]]
                name = "steal"
                url = "http://127.0.0.1:9"
                health = {{ url = "http://127.0.0.1:9/h", interval_s = 10 }}
                auth = {{ user = "u", password = "gitlab" }}
                "#
            ),
        )
        .unwrap();
        let token = dir.path().join("gitlab_token");
        crate::util::fs::write_atomic(&token, b"glpat-global-token", 0o600).unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.projects.roots = vec![];
        cfg.projects.include = vec![repo.display().to_string()];
        cfg.notify.desktop = false;
        cfg.secrets.insert("gitlab".into(), SecretRef::File(token.display().to_string()));
        let app = testutil::app_with(cfg).await;
        let p = app.state.projects.require("evil").unwrap();
        assert!(p.config.secrets.is_empty());
        assert_eq!(p.config.agent.permission_mode, None);
        assert!(p.warnings.iter().any(|w| w.contains("[secrets] ignored")), "{:?}", p.warnings);
        // What the env health check does: resolve the env's password secret.
        let e = app.state.secret(Some(&p), "pw").unwrap_err();
        assert_eq!(e.code, "not_configured");
        let e = app.state.secret(Some(&p), "gitlab").unwrap_err();
        assert!(e.message.contains("machine overlay"), "{}", e.message);
        // The global secret still works for the owner's own uses.
        assert_eq!(app.state.secret(None, "gitlab").unwrap().expose(), "glpat-global-token");
        // Exactly what the background env poller does, and the poller itself.
        for name in ["staging", "steal"] {
            let env = p.config.envs.iter().find(|e| e.name == name).unwrap().clone();
            let h = crate::apps::envs::check(&app.state, &p, &env).await;
            assert!(h.error.is_some_and(|e| e.contains("machine overlay")), "{name}");
        }
        crate::apps::start(&app.state).await;
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a repository secret command ran");
    }

    #[tokio::test]
    async fn adding_and_removing_projects_keeps_config_comments() {
        let app = testutil::app().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let file = app.state.paths.config_file();
        // The owner's hand-written config, edited after startup.
        let text = "# my comment at top\n[server]\nbind = \"127.0.0.1:7999\" # inline note\n\n[projects]\n# where my repos live\nroots = []\n\n[unknown_section]\nkeep = true\n";
        std::fs::write(&file, text).unwrap();
        let call = |method: &str, uri: String, body: Option<serde_json::Value>| {
            let mut b = axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "127.0.0.1:7999")
                .header("authorization", format!("Bearer {}", app.state.auth.master_token()));
            let body = match body {
                Some(v) => {
                    b = b.header("content-type", "application/json");
                    axum::body::Body::from(v.to_string())
                }
                None => axum::body::Body::empty(),
            };
            let req = b.body(body).unwrap();
            let router = app.router.clone();
            async move {
                use tower::ServiceExt;
                router.oneshot(req).await.unwrap().status()
            }
        };
        let s = call("POST", "/api/projects".into(), Some(serde_json::json!({ "path": dir.path() }))).await;
        assert!(s.is_success(), "{s}");
        let after = std::fs::read_to_string(&file).unwrap();
        for kept in ["# my comment at top", "# inline note", "# where my repos live", "[unknown_section]"] {
            assert!(after.contains(kept), "lost {kept:?}:\n{after}");
        }
        let id = app.state.projects.find_by_path(&dir.path().canonicalize().unwrap()).unwrap().id.clone();
        let s = call("DELETE", format!("/api/projects/{id}"), None).await;
        assert!(s.is_success(), "{s}");
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(after.contains("# where my repos live") && after.contains("exclude"), "{after}");
        assert!(app.state.config.read().projects.exclude.len() == 1);
    }

    /// A missing directory is `not_found` until the client asks for it to be created.
    #[tokio::test]
    async fn adding_a_missing_directory_needs_create() {
        let app = testutil::app().await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a/new-project");
        let post = |body: serde_json::Value| {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("host", "127.0.0.1:7999")
                .header("authorization", format!("Bearer {}", app.state.auth.master_token()))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let router = app.router.clone();
            async move {
                use tower::ServiceExt;
                router.oneshot(req).await.unwrap().status()
            }
        };
        assert_eq!(post(serde_json::json!({ "path": &target })).await, axum::http::StatusCode::NOT_FOUND);
        assert!(!target.exists());
        assert!(post(serde_json::json!({ "path": &target, "create": true })).await.is_success());
        assert!(target.is_dir());
        assert!(app.state.projects.find_by_path(&target.canonicalize().unwrap()).is_some());
    }

    /// Windows: WSL and network paths are refused as `unsupported_platform` before
    /// anything opens them (which would connect to their server); config.toml is untouched.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_refuses_wsl_and_network_roots() {
        use tower::ServiceExt;
        let app = testutil::app().await;
        let file = app.state.paths.config_file();
        let before = std::fs::read_to_string(&file).unwrap();
        for (path, says) in [
            (r"\\wsl$\Ubuntu\home\u\proj", "WSL"),
            ("//wsl.localhost/Debian/src", "WSL"),
            (r"\\server.invalid\share\proj", "network"),
            (r"\??\UNC\server.invalid\share\proj", "network"),
        ] {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("host", "127.0.0.1:7999")
                .header("authorization", format!("Bearer {}", app.state.auth.master_token()))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::json!({ "path": path }).to_string()))
                .unwrap();
            let resp = app.router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status().as_u16(), 501, "{path}");
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!((v["error"]["code"].as_str(), v["error"]["feature"].as_str()), (Some("unsupported_platform"), Some("networkRoots")), "{v}");
            assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains(says)), "{v}");
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    }

    /// Windows: WSL and network roots already in config.toml are skipped, not loaded;
    /// local ones next to them still are.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_skips_wsl_and_network_roots_in_config() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        std::fs::create_dir_all(local.join(".git")).unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.projects.roots = vec![r"\\server.invalid\share".into(), r"\\wsl$\Ubuntu\home\u".into()];
        cfg.projects.include =
            vec![r"\\wsl$\Ubuntu\home\u\proj".into(), r"\??\UNC\server.invalid\share\proj".into(), local.display().to_string()];
        cfg.notify.desktop = false;
        let app = testutil::app_with(cfg).await;
        let roots: Vec<std::path::PathBuf> = app.state.projects.list().iter().map(|p| p.root.clone()).collect();
        assert_eq!(roots, [crate::util::os::path::canonicalize(&local).unwrap()]);
    }

    /// A repository git refuses (`util::git::Failure::Refused`, which only Windows reports)
    /// gets a warning with the command that trusts it; other failures stay quiet.
    #[test]
    fn a_refused_repository_warns_with_the_command_that_trusts_it() {
        use crate::util::git::Failure;
        let root = std::path::Path::new("/srv/shop");
        let refusal = "fatal: detected dubious ownership in repository at '/srv/shop'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory /srv/shop";
        let mut warnings = vec!["earlier".to_string()];
        assert_eq!(super::summary_branch(root, Err(Failure::Refused(refusal.into())), &mut warnings), None);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[1].contains("/srv/shop") && warnings[1].ends_with("git config --global --add safe.directory /srv/shop"), "{warnings:?}");
        for quiet in [Failure::NotInstalled, Failure::TimedOut, Failure::Failed("fatal: not a git repository".into())] {
            assert_eq!(super::summary_branch(root, Err(quiet), &mut warnings), None);
        }
        assert_eq!(super::summary_branch(root, Ok(Some("main".into())), &mut warnings).as_deref(), Some("main"));
        assert_eq!(super::summary_branch(root, Ok(None), &mut warnings), None, "detached");
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn strips_userinfo_from_remote_urls() {
        assert_eq!(super::strip_credentials("https://oauth2:tok@gitlab.com/a/b.git"), "https://gitlab.com/a/b.git");
        assert_eq!(super::strip_credentials("https://gitlab.com/a/b.git"), "https://gitlab.com/a/b.git");
        assert_eq!(super::strip_credentials("git@gitlab.com:a/b.git"), "git@gitlab.com:a/b.git");
    }

    #[test]
    fn ids_stay_with_their_directory() {
        use super::assign_ids;
        use std::collections::BTreeMap;
        use std::path::PathBuf;
        let p = |s: &str| PathBuf::from(s);
        let mut known = BTreeMap::new();
        // First start: what positional assignment always gave.
        let first = assign_ids(&mut known, &[p("/r/api"), p("/x/api"), p("/x/web2"), p("/y/web2"), p("/z/2024"), p("/w/2024")]);
        assert_eq!(first, ["api", "api-2", "web2", "web-2", "2024", "2024-2"]);
        // A root with another `api` now scans first: the existing ones keep their ids.
        let ids = assign_ids(&mut known, &[p("/new/api"), p("/r/api"), p("/x/api")]);
        assert_eq!(ids, ["api-3", "api", "api-2"]);
        // The first `api` is removed (or gone): nobody inherits its id, not even a newcomer.
        let ids = assign_ids(&mut known, &[p("/x/api"), p("/other/api")]);
        assert_eq!(ids, ["api-2", "api-4"]);
        // Removed and added back: its id again.
        assert_eq!(assign_ids(&mut known, &[p("/r/api")]), ["api"]);
        assert!(super::valid_id("api-2") && !super::valid_id("../x") && !super::valid_id("") && !super::valid_id("A"));
    }

    #[test]
    fn workspace_scope_names_are_never_project_ids() {
        use super::{assign_ids, read_ids};
        use std::collections::BTreeMap;
        use std::path::PathBuf;
        let mut known = BTreeMap::new();
        let ids = assign_ids(&mut known, &[PathBuf::from("/a/home"), PathBuf::from("/b/all")]);
        assert_eq!(ids, ["home-2", "all-2"]);
        // A stored map that hands out a reserved id (written before the rule) is not trusted.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project-ids.json");
        std::fs::write(&file, r#"{"/a/home":"home","/c/web":"web"}"#).unwrap();
        let known = read_ids(&file);
        assert_eq!(known.len(), 1);
        assert_eq!(known.get("/c/web").map(String::as_str), Some("web"));
    }

    /// On Windows `projects/nul.toml` or `workspace/com1` would be the device.
    #[cfg(windows)]
    #[test]
    fn windows_device_names_are_never_project_ids() {
        use super::{assign_ids, read_ids};
        use std::collections::BTreeMap;
        use std::path::PathBuf;
        let mut known = BTreeMap::new();
        let ids = assign_ids(&mut known, &[PathBuf::from(r"C:\a\nul_"), PathBuf::from(r"C:\b\COM1"), PathBuf::from(r"C:\c\aux-")]);
        assert_eq!(ids, ["nul-2", "com-2", "aux-2"]);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project-ids.json");
        std::fs::write(&file, r#"{"C:\\a\\con":"con","C:\\c\\web":"web"}"#).unwrap();
        assert_eq!(read_ids(&file).into_values().collect::<Vec<_>>(), ["web"]);
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git").args(args).current_dir(dir).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    /// Adding a root with a same-named repository, or removing the first of two, used
    /// to move an id (and the overlay's secrets, cards and sessions) to another repository.
    #[tokio::test]
    async fn adding_or_removing_projects_never_moves_an_id_or_its_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let (own, alt) = (dir.path().join("proj/pyapi"), dir.path().join("alt/pyapi"));
        for d in [&own, &alt, &dir.path().join("root1/pyapi")] {
            std::fs::create_dir_all(d.join(".git")).unwrap();
        }
        // alt/pyapi is somebody else's clone that names the owner's secret.
        std::fs::write(
            alt.join(".workbench.toml"),
            "[[env]]\nname = \"x\"\nurl = \"http://127.0.0.1:9\"\nauth = { user = \"x\", password = \"deploy-pw\" }\n",
        )
        .unwrap();
        let secret = dir.path().join("pw");
        crate::util::fs::write_atomic(&secret, b"hunter2", 0o600).unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.projects.roots = vec![];
        cfg.projects.include = vec![own.display().to_string()];
        cfg.notify.desktop = false;
        let app = testutil::app_with(cfg).await;
        let state = &app.state;
        std::fs::create_dir_all(state.paths.config_dir.join("projects")).unwrap();
        std::fs::write(
            state.paths.project_overlay("pyapi"),
            format!("[secrets]\ndeploy-pw = {{ file = {:?} }}\n", secret.display().to_string()),
        )
        .unwrap();
        let root_of = |id: &str| state.projects.get(id).map(|p| p.root.clone());
        let own = crate::util::os::path::canonicalize(&own).unwrap();
        state.projects.reload(state).await;
        assert_eq!(root_of("pyapi"), Some(own.clone()));

        // A root that holds another `pyapi` scans before the includes.
        state.config.write().projects.roots = vec![dir.path().join("root1").display().to_string()];
        state.projects.reload(state).await;
        assert_eq!(root_of("pyapi"), Some(own.clone()));
        assert_eq!(root_of("pyapi-2"), Some(crate::util::os::path::canonicalize(dir.path().join("root1/pyapi")).unwrap()));

        // Another clone is added, then the owner's project is removed.
        state.config.write().projects.include.push(alt.display().to_string());
        state.projects.reload(state).await;
        let alt_id = state.projects.find_by_path(&alt.canonicalize().unwrap()).unwrap().id.clone();
        assert_eq!(alt_id, "pyapi-3");
        state.config.write().projects.exclude.push(own.display().to_string());
        state.projects.reload(state).await;
        assert!(state.projects.get("pyapi").is_none(), "the removed project's id went to another directory");
        let alt_p = state.projects.require(&alt_id).unwrap();
        assert!(alt_p.config.secrets.is_empty());
        assert_eq!(state.secret(Some(&alt_p), "deploy-pw").unwrap_err().code, "not_configured");
        // The ids are stored with the data they key.
        let map: serde_json::Value = crate::util::fs::read_json(&state.paths.data_dir.join("project-ids.json")).unwrap().unwrap();
        assert_eq!(map[own.to_string_lossy().as_ref()], "pyapi");
    }

    #[test]
    fn configured_forge_hosts_are_bare_hosts() {
        assert_eq!(super::bare_host("git.corp.example"), "git.corp.example");
        assert_eq!(super::bare_host("https://Git.Corp.example/"), "git.corp.example");
        assert_eq!(super::bare_host("http://127.0.0.1:8931"), "127.0.0.1");
        assert_eq!(super::bare_host(""), "");
    }

    /// A GitHub Enterprise or self-hosted GitLab remote whose host has no "github" or
    /// "gitlab" in it is that forge's project when config.toml names its host.
    #[tokio::test]
    async fn remotes_on_the_configured_enterprise_hosts_are_forge_projects() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, url: &str| {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            git(&d, &["init", "-q", "-b", "main"]);
            git(&d, &["remote", "add", "origin", url]);
            d
        };
        let ghe = mk("ghe", "https://git.corp.example/team/app.git");
        let gle = mk("gle", "git@code.company.example:group/sub/svc.git");
        let other = mk("other", "https://elsewhere.example/team/app.git");
        let mut cfg = GlobalConfig::default();
        cfg.projects.roots = vec![dir.path().display().to_string()];
        cfg.notify.desktop = false;
        cfg.github = Some(crate::config::global::GithubConfig { host: "https://git.corp.example".into(), token: "ghe".into() });
        cfg.gitlab = Some(crate::config::global::GitlabConfig { host: "code.company.example".into(), token: "gl".into() });
        let app = testutil::app_with(cfg).await;
        let find = |d: &std::path::Path| app.state.projects.find_by_path(&d.canonicalize().unwrap()).unwrap();
        let p = find(&ghe);
        assert_eq!(p.github(), Some(("https://git.corp.example".to_string(), "team/app".to_string())));
        assert_eq!(p.gitlab(), None);
        // An empty token: the global one, which the client sends to its own host only.
        assert_eq!(p.config.repo.as_ref().unwrap().github.as_ref().unwrap().token, "");
        assert_eq!(find(&gle).gitlab(), Some(("code.company.example".to_string(), "group/sub/svc".to_string())));
        let o = find(&other);
        assert_eq!((o.github(), o.gitlab()), (None, None));
    }

    /// A project with several repositories: the root's own, ones found below it, one named by
    /// `[[repository]]`; and a folder that is no repository but a project (a `.workbench.toml`)
    /// holding some. A request's `?repo=` is resolved by `require_repo`.
    #[tokio::test]
    async fn a_project_lists_its_repositories_and_a_view_answers_for_one() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |rel: &str, url: &str| {
            let d = dir.path().join(rel);
            std::fs::create_dir_all(&d).unwrap();
            git(&d, &["init", "-q", "-b", "main"]);
            git(&d, &["remote", "add", "origin", url]);
            d
        };
        mk("shop", "https://gitlab.com/acme/shop.git");
        mk("shop/services/api", "https://github.com/acme/api.git");
        mk("shop/web", "git@gitlab.com:acme/web.git");
        std::fs::create_dir_all(dir.path().join("shop/docs")).unwrap();
        std::fs::write(
            dir.path().join("shop/.workbench.toml"),
            r#"
            [[repository]]
            path = "web"
            name = "Frontend"
            [repository.gitlab]
            host = "gitlab.example"
            path = "acme/fe"
            [[repository]]
            path = "../outside"
            [[repository]]
            path = "docs"
            "#,
        )
        .unwrap();
        // Not a repository itself: listed because of its `.workbench.toml`.
        std::fs::create_dir_all(dir.path().join("multi")).unwrap();
        std::fs::write(dir.path().join("multi/.workbench.toml"), "[project]\nname = \"Multi\"\n").unwrap();
        mk("multi/a", "https://gitlab.com/acme/a.git");
        mk("multi/b", "https://github.com/acme/b.git");
        // A folder with nothing to say is no project.
        std::fs::create_dir_all(dir.path().join("plain/inner")).unwrap();

        let mut cfg = GlobalConfig::default();
        cfg.projects.roots = vec![dir.path().display().to_string()];
        cfg.notify.desktop = false;
        let app = testutil::app_with(cfg).await;
        let reg = &app.state.projects;
        assert_eq!(reg.list().iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["multi", "shop"]);

        let shop = reg.require("shop").unwrap();
        let ids: Vec<&str> = shop.repos.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [".", "services/api", "web"]);
        assert_eq!(shop.repo_id(), ".");
        assert_eq!(shop.gitlab(), Some(("gitlab.com".to_string(), "acme/shop".to_string())));
        let names: Vec<&str> = shop.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["shop", "api", "Frontend"]);
        assert!(shop.warnings.iter().any(|w| w.contains("../outside") && w.contains("inside the project")), "{:?}", shop.warnings);
        assert!(shop.warnings.iter().any(|w| w.contains("docs") && w.contains("not the top of a git repository")), "{:?}", shop.warnings);

        // The default repository is the project itself; another is a view of it.
        assert!(std::sync::Arc::ptr_eq(&shop, &reg.require_repo("shop", None).unwrap()));
        assert!(std::sync::Arc::ptr_eq(&shop, &reg.require_repo("shop", Some("")).unwrap()));
        assert!(std::sync::Arc::ptr_eq(&shop, &reg.require_repo("shop", Some(".")).unwrap()));
        let api = reg.require_repo("shop", Some("services/api")).unwrap();
        assert_eq!((api.id.as_str(), api.repo_id(), api.github()), ("shop", "services/api", Some(("github.com".to_string(), "acme/api".to_string()))));
        assert_eq!(api.gitlab(), None, "the root's GitLab project is not inherited");
        assert_eq!(api.repo_dir(), shop.root.join("services/api"));
        let web = reg.require_repo("shop", Some("web")).unwrap();
        assert_eq!(web.gitlab(), Some(("gitlab.example".to_string(), "acme/fe".to_string())), "[[repository]] overrides the remote");
        let e = reg.require_repo("shop", Some("nope")).unwrap_err();
        assert_eq!((e.status, e.code), (axum::http::StatusCode::NOT_FOUND, "unknown_repo"));
        assert_eq!(reg.require_repo("nope", None).unwrap_err().status, axum::http::StatusCode::NOT_FOUND);

        // The summary lists them, the default first.
        let sum = super::summary(&app.state, &shop).await;
        assert_eq!(sum.repos.iter().map(|r| (r.id.as_str(), r.default)).collect::<Vec<_>>(), [(".", true), ("services/api", false), ("web", false)]);
        assert_eq!(sum.repos[1].github.as_ref().unwrap()["path"], "acme/api");
        assert_eq!(sum.repos[0].path, "");
        assert_eq!(sum.repos[2].path, "web");

        // A folder of repositories: no "." repository; the first one is the default and
        // answers for the project's remote and forge.
        let multi = reg.require("multi").unwrap();
        assert_eq!(multi.name, "Multi");
        assert_eq!(multi.repos.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!((multi.repo_id(), multi.repo_dir()), ("a", multi.root.join("a").as_path()));
        assert_eq!(multi.gitlab(), Some(("gitlab.com".to_string(), "acme/a".to_string())));
        assert_eq!(reg.require_repo("multi", Some("b")).unwrap().github(), Some(("github.com".to_string(), "acme/b".to_string())));
        assert_eq!(multi.scope_key(), "multi@a");
    }

    /// Scratch files: a project reachable by id and path but never listed, rooted in
    /// the data dir's `scratches` (0700), whose files go through the ordinary routes and
    /// cannot reach the rest of the data dir.
    #[tokio::test]
    async fn scratch_files_are_a_hidden_project_confined_to_their_folder() {
        use super::SCRATCH_ID;
        use crate::mcp::{McpCtx, call_api};
        use axum::http::Method;
        use serde_json::json;

        let t = crate::platform::testutil::app().await;
        assert!(t.state.projects.list().iter().all(|p| p.id != SCRATCH_ID));
        let p = t.state.projects.require(SCRATCH_ID).unwrap();
        assert_eq!(p.name, "Scratches");
        assert_eq!(p.root, crate::util::os::path::canonicalize(t.state.paths.data_dir.join("scratches")).unwrap());
        crate::util::os::perm::assert_mode(&p.root, 0o700);
        assert_eq!(t.state.projects.find_by_path(&p.root.join("a.md")).map(|x| x.id.clone()).as_deref(), Some(SCRATCH_ID));

        let ctx = McpCtx::default();
        let base = format!("/api/projects/{SCRATCH_ID}/files");
        call_api(&t.state, Method::PUT, &format!("{base}/write"), Some(json!({ "path": "scratch.http", "content": "GET http://127.0.0.1/\n", "etag": null })), &ctx)
            .await
            .unwrap();
        assert!(p.root.join("scratch.http").is_file());
        let v = call_api(&t.state, Method::GET, &format!("{base}/read?path=scratch.http"), None, &ctx).await.unwrap();
        assert_eq!(v["content"], "GET http://127.0.0.1/\n");
        // The master token and device keys sit next door.
        for bad in ["../token", "../auth.json"] {
            let e = call_api(&t.state, Method::GET, &format!("{base}/read?path={bad}"), None, &ctx).await.unwrap_err();
            assert!(e.status.is_client_error(), "{bad}: {e:?}");
        }
        crate::util::os::fs::symlink(t.state.paths.data_dir.join("token"), p.root.join("link")).unwrap();
        assert!(call_api(&t.state, Method::GET, &format!("{base}/read?path=link"), None, &ctx).await.is_err());
        // The list the UI shows never has it.
        let v = call_api(&t.state, Method::GET, "/api/projects", None, &ctx).await.unwrap();
        assert!(v.as_array().unwrap().iter().all(|x| x["id"] != SCRATCH_ID), "{v}");
    }
}

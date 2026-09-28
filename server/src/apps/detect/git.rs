//! Git remote → GitLab project or GitHub repository, default branch;
//! `.gitlab-ci.yml` / `.github/workflows/*.yml` → CI jobs.
//!
//! Other forges (Bitbucket, Codeberg, Gitea/Forgejo) get no forge section: the
//! project registry records the remote itself, and detection only tags them.

use std::path::{Path, PathBuf};

use super::Ctx;
use crate::config::project::{Ci, GitHub, GitLab, Repo};

/// `[remote "<name>"] url = …` entries of a git config, in file order (first URL each).
pub(crate) fn config_remotes(config: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    let mut current: Option<String> = None;
    for line in config.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            current = t
                .strip_prefix("[remote \"")
                .and_then(|r| r.strip_suffix("\"]"))
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            continue;
        }
        let Some(name) = &current else { continue };
        let Some((k, v)) = t.split_once('=') else { continue };
        if k.trim() == "url" && !out.iter().any(|(n, _)| n == name) {
            let v = v.trim().trim_matches('"');
            if !v.is_empty() {
                out.push((name.clone(), v.to_string()));
            }
        }
    }
    out
}

/// A remote host as the forge knows it: ssh config aliases (`github.com-work`,
/// `gitlab.com_personal`) and ssh-over-443 hosts (`ssh.github.com`) map back to
/// the forge's own host.
pub(crate) fn canonical_host(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    for known in ["github.com", "gitlab.com", "bitbucket.org", "codeberg.org"] {
        if h == known || h.strip_prefix(known).is_some_and(|rest| rest.starts_with(['-', '_'])) {
            return known.to_string();
        }
    }
    match h.as_str() {
        "ssh.github.com" => "github.com".into(),
        "altssh.gitlab.com" => "gitlab.com".into(),
        "altssh.bitbucket.org" => "bitbucket.org".into(),
        _ => h,
    }
}

/// GitHub.com or a GitHub Enterprise host (`github.<company>.com`).
fn is_github_host(host: &str) -> bool {
    host == "github.com" || host.starts_with("github.")
}

/// The repository's git directory: `.git/`, or the directory a `.git` file points
/// at (worktrees, submodules) — using its `commondir` for shared config and refs.
pub(super) fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    let meta = std::fs::symlink_metadata(&dot).ok()?;
    if meta.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let gd = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gd = if Path::new(gd).is_absolute() { PathBuf::from(gd) } else { root.join(gd) };
    match std::fs::read_to_string(gd.join("commondir")) {
        Ok(c) => {
            let c = c.trim();
            Some(if Path::new(c).is_absolute() { PathBuf::from(c) } else { gd.join(c) })
        }
        Err(_) => Some(gd),
    }
}

pub fn detect(cx: &mut Ctx) {
    let Some(gd) = git_dir(cx.root) else { return };
    let config = cx.read(&gd.join("config")).unwrap_or_default();
    let head = cx.read(&gd.join("refs/remotes/origin/HEAD")).unwrap_or_default();
    let default_branch = head
        .trim()
        .strip_prefix("ref: refs/remotes/origin/")
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    let mut repo = Repo { remote: "origin".into(), default_branch, ..Default::default() };
    // `origin`, else the first remote (a clone that only has `upstream`).
    let remotes = config_remotes(&config);
    let chosen = remotes.iter().find(|(n, _)| n == "origin").or_else(|| remotes.first());
    if let Some((name, url)) = chosen {
        repo.remote = name.clone();
        // Only host and path are kept: credentials in the URL never enter the config.
        if let Some((host, path)) = crate::util::git::parse_remote(url) {
            let host = canonical_host(&host);
            if host.contains("gitlab") {
                repo.gitlab = Some(GitLab {
                    registry: Some(format!("registry.{host}/{}", path.to_lowercase())),
                    host,
                    path,
                    project_id: None,
                    // No secret name: repository-derived config may not choose which
                    // secret goes where (it would only resolve from the machine overlay).
                    // Empty = the global [gitlab] token, which the GitLab client uses only
                    // when its host matches.
                    token: String::new(),
                });
            } else if is_github_host(&host) {
                // Same rule as GitLab: empty = the global [github] token, same host only.
                repo.github = Some(GitHub { host, path, token: String::new() });
            } else if host == "bitbucket.org" {
                cx.tag("bitbucket");
            } else if host == "codeberg.org" {
                cx.tag("codeberg");
            } else if host.contains("gitea") || host.contains("forgejo") {
                cx.tag("gitea");
            }
        }
    }
    cx.pf.repo = Some(repo);
}

/// `.github/workflows/*.yml` → `repo.ci` with provider `github` (job ids across all
/// workflows). A project that also has `.gitlab-ci.yml` keeps GitLab as its CI.
pub fn detect_github_workflows(cx: &mut Ctx) {
    let dir = cx.root.join(".github/workflows");
    let files: Vec<PathBuf> = cx
        .files
        .iter()
        .filter(|f| f.parent() == Some(dir.as_path()) && f.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .cloned()
        .collect();
    if files.is_empty() {
        return;
    }
    cx.tag("github-actions");
    if cx.pf.repo.as_ref().is_some_and(|r| r.ci.is_some()) {
        return;
    }
    let mut jobs: Vec<String> = vec![];
    for f in files.iter().take(50) {
        let Some(src) = cx.read(f) else { continue };
        for id in workflow_jobs(&src) {
            if jobs.len() < 200 && !jobs.contains(&id) {
                jobs.push(id);
            }
        }
    }
    let repo = cx.pf.repo.get_or_insert_with(|| Repo { remote: "origin".into(), ..Default::default() });
    repo.ci = Some(Ci { provider: "github".into(), config: ".github/workflows".into(), jobs, image_tag: None });
}

/// Job ids of one GitHub Actions workflow (the keys of its `jobs` mapping).
pub fn workflow_jobs(src: &str) -> Vec<String> {
    let Ok(y) = serde_norway::from_str::<serde_norway::Value>(src) else { return vec![] };
    let Some(jobs) = y.get("jobs").and_then(|j| j.as_mapping()) else { return vec![] };
    jobs.iter().filter(|(_, v)| v.is_mapping()).filter_map(|(k, _)| k.as_str().map(str::to_string)).collect()
}

/// Top-level keys that are not jobs.
const CI_RESERVED: &[&str] = &[
    "stages", "variables", "default", "include", "workflow", "image", "services", "cache", "before_script",
    "after_script", "spec",
];

pub fn detect_gitlab_ci(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let jobs = ci_jobs(&src);
    let image_tag = (src.contains("$CI_COMMIT_SHORT_SHA") || src.contains("${CI_COMMIT_SHORT_SHA}")).then(|| "short_sha".to_string());
    cx.tag("gitlab-ci");
    let config = cx.rel(f);
    let repo = cx.pf.repo.get_or_insert_with(|| Repo { remote: "origin".into(), ..Default::default() });
    repo.ci = Some(Ci { provider: "gitlab".into(), config, jobs, image_tag });
}

/// Job names of a GitLab CI file: top-level keys minus reserved keys and hidden (`.x`) jobs.
pub fn ci_jobs(src: &str) -> Vec<String> {
    let Ok(y) = serde_norway::from_str::<serde_norway::Value>(src) else { return vec![] };
    let Some(m) = y.as_mapping() else { return vec![] };
    m.iter()
        .filter_map(|(k, v)| {
            let k = k.as_str()?;
            // A job is a mapping; `pages` is a real (reserved-name) job when it is one.
            (!CI_RESERVED.contains(&k) && !k.starts_with('.') && v.is_mapping()).then(|| k.to_string())
        })
        .collect()
}

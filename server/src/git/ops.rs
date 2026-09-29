//! Local mutating operations. Every function here runs with the repository write
//! lock held by the caller (`routes.rs`), which also emits `git.changed`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::cmd::{COMMIT_TIMEOUT, check_ref_name, check_rev, git_error, literal, pathspec_stdin};
use super::repo::Repo;
use super::status::{detect_state, entries_for, narrow_entries};
use crate::error::ApiError;

/// Outcome of operations that can stop on conflicts (merge, rebase, cherry-pick,
/// revert, stash apply, continue). Conflicts are a normal result, not an error.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpOutcome {
    pub ok: bool,
    pub conflicts: bool,
    pub message: String,
    /// clean | merging | rebasing | … after the operation.
    pub state: &'static str,
    /// Checkout refused because local changes would be overwritten (the UI
    /// offers a smart checkout). An expected outcome, so not an HTTP error.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
}

async fn outcome(repo: &Repo, out: super::cmd::GitOutput, success_msg: &str) -> Result<OpOutcome, ApiError> {
    let (state, _) = detect_state(&repo.git_dir);
    if out.ok() {
        // `--autostash` exits 0 even when re-applying the local changes conflicts:
        // the files are left unmerged and the stash is kept. (Every operation
        // here refuses to start with unmerged files, so any now are new.)
        let n = conflicted_paths(repo).await?.len();
        if n > 0 {
            return Ok(OpOutcome { ok: false, conflicts: true, message: autostash_conflict_message(success_msg, n), state, dirty: false });
        }
        return Ok(OpOutcome { ok: true, message: success_msg.to_string(), state, ..Default::default() });
    }
    let raw = format!("{}\n{}", out.text(), out.stderr);
    let conflicts = raw.contains("CONFLICT") || raw.contains("Merge conflict") || has_conflicts(repo).await?;
    if conflicts {
        let n = conflicted_paths(repo).await?.len();
        return Ok(OpOutcome {
            ok: false,
            conflicts: true,
            message: format!("{n} file{} with conflicts. Resolve them, then continue.", if n == 1 { "" } else { "s" }),
            state,
            dirty: false,
        });
    }
    Err(git_error(&out))
}

/// "Rebased onto x, but re-applying your local changes conflicts in 2 files…"
pub(super) fn autostash_conflict_message(done: &str, n: usize) -> String {
    format!(
        "{done}, but re-applying your local changes conflicts in {n} file{}. Resolve {}; your changes are also kept in the stash (drop it once resolved).",
        if n == 1 { "" } else { "s" },
        if n == 1 { "it" } else { "them" }
    )
}

pub(super) async fn conflicted_paths(repo: &Repo) -> Result<Vec<String>, ApiError> {
    let out = repo.git().args(["diff", "--name-only", "--diff-filter=U", "-z"]).run().await?;
    Ok(super::cmd::split_z(&out.stdout))
}

async fn has_conflicts(repo: &Repo) -> Result<bool, ApiError> {
    Ok(!conflicted_paths(repo).await?.is_empty())
}

async fn has_head(repo: &Repo) -> Result<bool, ApiError> {
    Ok(repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?.ok())
}

fn repo_paths(repo: &Repo, paths: &[String]) -> Result<Vec<String>, ApiError> {
    if paths.is_empty() {
        return Err(ApiError::bad_request("no paths given"));
    }
    if paths.len() > 10_000 {
        return Err(ApiError::bad_request("too many paths"));
    }
    paths.iter().map(|p| repo.to_repo(p)).collect()
}

fn submodule_message(subs: &[String]) -> String {
    let (n, list) = (subs.len(), subs.join(", "));
    format!(
        "{list} {} a submodule: changes inside {} can only be committed or reset in the submodule itself.",
        if n == 1 { "is" } else { "are" },
        if n == 1 { "it" } else { "them" }
    )
}

// ---------------------------------------------------------------- stage / unstage / discard

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathsRequest {
    #[serde(default)]
    pub paths: Vec<String>,
    /// Apply to every change in the project instead of `paths`.
    #[serde(default)]
    pub all: bool,
}

/// Stage files. Returns the (project-relative) submodules that were skipped because
/// their only change is content inside them, which `git add` cannot stage.
pub async fn stage(repo: &Repo, req: &PathsRequest) -> Result<Vec<String>, ApiError> {
    if req.all {
        repo.git_w().args(["add".to_string(), "-A".into(), "--".into(), literal(&repo.scope())]).run_ok_retry_lock().await?;
        return Ok(vec![]);
    }
    let mut paths = repo_paths(repo, &req.paths)?;
    let subs: Vec<String> = narrow_entries(repo, &paths).await?.into_iter().filter(|f| f.submodule_content_only()).map(|f| f.path).collect();
    paths.retain(|p| !subs.contains(p));
    let skipped: Vec<String> = subs.iter().map(|p| repo.to_project(p)).collect();
    if paths.is_empty() {
        return Err(ApiError::bad_request(submodule_message(&skipped)));
    }
    repo.git_w()
        .args(["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(pathspec_stdin(&paths))
        .run_ok_retry_lock()
        .await?;
    Ok(skipped)
}

pub async fn unstage(repo: &Repo, req: &PathsRequest) -> Result<(), ApiError> {
    let head = has_head(repo).await?;
    let mut paths = if req.all { vec![repo.scope()] } else { repo_paths(repo, &req.paths)? };
    if !req.all {
        // Unstaging a rename needs both sides (else the old name's deletion stays staged).
        for f in entries_for(repo, &paths).await? {
            if f.index == 'R' || f.index == 'C' {
                for side in std::iter::once(f.path).chain(f.orig_path) {
                    if !paths.contains(&side) {
                        paths.push(side);
                    }
                }
            }
        }
    }
    let g = if head {
        repo.git_w().args(["restore", "--staged", "--pathspec-from-file=-", "--pathspec-file-nul"])
    } else {
        // Before the first commit there is nothing to restore from.
        repo.git_w().args(["rm", "--cached", "-r", "--quiet", "--ignore-unmatch", "--pathspec-from-file=-", "--pathspec-file-nul"])
    };
    g.stdin(pathspec_stdin(&paths)).run_ok_retry_lock().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardRequest {
    pub paths: Vec<String>,
    /// `all` (default): back to HEAD, staged and unstaged. `worktree`: only the
    /// unstaged changes (back to the index).
    #[serde(default)]
    pub scope: Option<String>,
    /// Also delete (to the trash) files that were added in the index.
    #[serde(default)]
    pub delete_added: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardResult {
    pub restored: usize,
    /// Untracked (or added) files moved to the trash.
    pub trashed: Vec<String>,
    /// Where they went: "trash" (desktop trash via gio, the Recycle Bin) or a Workbench folder.
    pub trash_location: Option<String>,
    /// Submodules whose changes inside them cannot be rolled back from here.
    pub skipped: Vec<String>,
}

/// Move a file to the desktop trash (`gio trash`, the Recycle Bin), else into `fallback_dir`.
async fn trash(abs: &Path, fallback_dir: &Path, rel: &str) -> Result<String, ApiError> {
    if crate::util::os::fs::desktop_trash(abs).await? {
        return Ok("trash".into());
    }
    let dest = fallback_dir.join(rel);
    let abs = abs.to_path_buf();
    let dest2 = dest.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(p) = dest2.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::rename(&abs, &dest2).or_else(|_| {
            // Across filesystems: copy, then remove.
            if abs.is_dir() {
                Err(std::io::Error::other("cannot move a directory across filesystems"))
            } else {
                std::fs::copy(&abs, &dest2)?;
                std::fs::remove_file(&abs)
            }
        })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(dest.parent().map(|p| p.display().to_string()).unwrap_or_default())
}

pub async fn discard(repo: &Repo, req: &DiscardRequest, trash_dir: &Path) -> Result<DiscardResult, ApiError> {
    let paths = repo_paths(repo, &req.paths)?;
    let worktree_only = req.scope.as_deref() == Some("worktree");
    let head = has_head(repo).await?;
    let entries = entries_for(repo, &paths).await?;
    let mut restore: Vec<String> = vec![];
    let mut unstage_only: Vec<String> = vec![];
    let mut to_trash: Vec<String> = vec![];
    let mut skipped: Vec<String> = vec![];
    let mut seen = std::collections::HashSet::new();
    for p in &paths {
        // Either side of a staged rename rolls back the whole rename.
        let Some(f) = entries
            .iter()
            .find(|f| &f.path == p)
            .or_else(|| entries.iter().find(|f| f.index == 'R' && f.orig_path.as_ref() == Some(p)))
        else {
            continue;
        };
        if !seen.insert(f.path.clone()) {
            continue;
        }
        let p = &f.path;
        if f.conflict {
            return Err(ApiError::bad_request(format!("{} has conflicts; resolve it instead of rolling back", repo.to_project(p))));
        }
        if f.index == '?' {
            to_trash.push(p.clone());
            continue;
        }
        if f.submodule {
            // Only a staged submodule change (a new commit or the gitlink itself)
            // can be undone from the superproject; `git restore` does not touch
            // the submodule's own checkout.
            if !worktree_only && f.index != ' ' {
                unstage_only.push(p.clone());
            }
            if f.worktree != ' ' {
                skipped.push(repo.to_project(p));
            }
            continue;
        }
        if worktree_only {
            if f.worktree != ' ' {
                restore.push(p.clone());
            }
            continue;
        }
        match f.index {
            'A' => {
                unstage_only.push(p.clone());
                if req.delete_added {
                    to_trash.push(p.clone());
                }
            }
            'R' | 'C' => {
                unstage_only.push(p.clone());
                if req.delete_added || f.index == 'R' {
                    // A rename rolled back: the new name goes away, the old one returns.
                    to_trash.push(p.clone());
                }
                if let Some(o) = &f.orig_path {
                    if f.index == 'R' {
                        restore.push(o.clone());
                    }
                }
            }
            _ => restore.push(p.clone()),
        }
    }
    if !skipped.is_empty() && restore.is_empty() && unstage_only.is_empty() && to_trash.is_empty() {
        return Err(ApiError::bad_request(submodule_message(&skipped)));
    }
    if !unstage_only.is_empty() {
        let g = if head {
            repo.git_w().args(["restore", "--staged", "--pathspec-from-file=-", "--pathspec-file-nul"])
        } else {
            repo.git_w().args(["rm", "--cached", "--quiet", "--ignore-unmatch", "--pathspec-from-file=-", "--pathspec-file-nul"])
        };
        g.stdin(pathspec_stdin(&unstage_only)).run_ok_retry_lock().await?;
    }
    if !restore.is_empty() {
        let g = if worktree_only || !head {
            repo.git_w().args(["restore", "--worktree", "--pathspec-from-file=-", "--pathspec-file-nul"])
        } else {
            repo.git_w().args(["restore", "--source=HEAD", "--staged", "--worktree", "--pathspec-from-file=-", "--pathspec-file-nul"])
        };
        g.stdin(pathspec_stdin(&restore)).run_ok_retry_lock().await?;
    }
    let mut trashed = vec![];
    let mut location = None;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let fallback = trash_dir.join(format!("{}-{stamp}", repo.project_id));
    for p in &to_trash {
        // The entry itself is trashed: an untracked symlink pointing outside is fine.
        let abs = crate::util::paths::resolve_entry_in_root(&repo.top, p)?;
        if tokio::fs::symlink_metadata(&abs).await.is_err() {
            continue;
        }
        location = Some(trash(&abs, &fallback, p).await?);
        trashed.push(repo.to_project(p));
    }
    Ok(DiscardResult { restored: restore.len() + unstage_only.len(), trashed, trash_location: location, skipped })
}

// ---------------------------------------------------------------- commit

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub message: String,
    #[serde(default)]
    pub amend: bool,
    #[serde(default)]
    pub signoff: bool,
    /// Commit exactly these files (their working-tree content), ignoring the
    /// rest of the index (`git commit --only`).
    pub paths: Option<Vec<String>>,
    /// Skip hooks (`--no-verify`).
    #[serde(default)]
    pub no_verify: bool,
    /// Commit only these lines of these files (CLion's partial commit): lines of the
    /// HEAD → working tree diff (`compare` mode, base HEAD), guarded by its
    /// fingerprint. Combined with `paths` (whole files); the rest of the index is
    /// left out, like `--only`.
    #[serde(default)]
    pub partial: Vec<PartialFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialFile {
    pub path: String,
    pub fingerprint: String,
    pub lines: Vec<super::lines::LineRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub sha: String,
    pub summary: String,
}

pub async fn commit(repo: &Repo, req: &CommitRequest) -> Result<CommitResult, ApiError> {
    let message = req.message.trim_end();
    let (state, detail) = detect_state(&repo.git_dir);
    // An empty message is fine when git has one prepared (amend, or MERGE_MSG /
    // the cherry-pick/revert message): `--no-edit` takes it.
    let prepared = state == "merging" || state == "cherry-picking" || state == "reverting";
    if message.trim().is_empty() && !req.amend && !prepared {
        return Err(ApiError::bad_request("the commit message is empty"));
    }
    // A rebase stopped at an `edit` step is waiting for exactly this (amend or new commits).
    if state == "rebasing" && !detail.edit {
        return Err(ApiError::bad_request("a rebase is in progress: resolve conflicts, stage the files and use Continue"));
    }
    if !req.partial.is_empty() {
        if state == "merging" {
            return Err(ApiError::bad_request("a merge commit must include the whole index; commit without selecting lines"));
        }
        return commit_selected(repo, req, message).await;
    }
    let mut g = repo.git_w().arg("commit").timeout(COMMIT_TIMEOUT);
    if message.trim().is_empty() {
        g = g.arg("--no-edit");
        if prepared && !req.amend {
            // MERGE_MSG lists conflicts as `#` comments that an editor would strip.
            g = g.arg("--cleanup=strip");
        }
    } else {
        g = g.args(["--cleanup=whitespace", "-F", "-"]).stdin(format!("{message}\n"));
    }
    if req.amend {
        g = g.arg("--amend");
    }
    if req.signoff {
        g = g.arg("--signoff");
    }
    if req.no_verify {
        g = g.arg("--no-verify");
    }
    if let Some(paths) = req.paths.as_ref().filter(|p| !p.is_empty()) {
        if state == "merging" {
            return Err(ApiError::bad_request("a merge commit must include the whole index; commit without selecting files"));
        }
        let mut paths = repo_paths(repo, paths)?;
        let entries = entries_for(repo, &paths).await?;
        // Untracked files must be known to git before --only can take them.
        let untracked: Vec<String> = entries.iter().filter(|f| f.index == '?').map(|f| f.path.clone()).collect();
        if !untracked.is_empty() {
            repo.git_w()
                .args(["add", "--intent-to-add", "--pathspec-from-file=-", "--pathspec-file-nul"])
                .stdin(pathspec_stdin(&untracked))
                .run_ok_retry_lock()
                .await?;
        }
        // A staged rename is committed whole (else the old name's deletion stays behind).
        for f in entries.iter().filter(|f| f.index == 'R') {
            for side in std::iter::once(&f.path).chain(&f.orig_path) {
                if !paths.contains(side) {
                    paths.push(side.clone());
                }
            }
        }
        g = g.args(["--only", "--"]).args(paths.iter().map(|p| literal(p)));
    }
    let out = g.run().await?;
    if !out.ok() {
        let raw = format!("{}\n{}", out.text(), out.stderr);
        if raw.contains("nothing to commit") || raw.contains("no changes added to commit") {
            return Err(ApiError::bad_request("Nothing to commit: stage changes first (or select files)"));
        }
        if raw.contains("Please tell me who you are") {
            return Err(ApiError::not_configured(
                "git does not know who you are: set user.name and user.email (git config --global user.name …)",
            ));
        }
        return Err(git_error(&out));
    }
    let sha = repo.git().args(["rev-parse", "HEAD"]).run_ok().await?.text().trim().to_string();
    let summary = out.text().lines().next().unwrap_or_default().to_string();
    Ok(CommitResult { sha, summary })
}

pub async fn last_commit_message(repo: &Repo) -> Result<String, ApiError> {
    if !has_head(repo).await? {
        return Ok(String::new());
    }
    let out = repo.git().args(["log", "-1", "--format=%B", "HEAD", "--"]).run_ok().await?;
    Ok(out.text().trim_end().to_string())
}

// ---------------------------------------------------------------- branches

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRequest {
    /// Branch, remote branch, tag or revision to check out.
    #[serde(rename = "ref")]
    pub rev: Option<String>,
    /// Create this new branch (from `startPoint`, default `ref` or HEAD).
    pub create: Option<String>,
    pub start_point: Option<String>,
    /// Set up tracking (for a remote start point).
    pub track: Option<bool>,
    /// Detach at `ref` (revisions and tags).
    #[serde(default)]
    pub detach: bool,
    /// Throw away local changes that would conflict.
    #[serde(default)]
    pub force: bool,
    /// Stash local changes, check out, unstash (CLion's "Smart Checkout").
    #[serde(default)]
    pub smart: bool,
}

async fn is_local_branch(repo: &Repo, name: &str) -> Result<bool, ApiError> {
    Ok(repo.git().args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]).run().await?.ok())
}

async fn checkout_once(repo: &Repo, req: &CheckoutRequest) -> Result<(), ApiError> {
    let mut g = repo.git_w().arg("switch");
    if req.force {
        g = g.arg("--discard-changes");
    }
    if let Some(name) = req.create.as_deref().filter(|s| !s.is_empty()) {
        check_ref_name(&repo.top, name, "branch").await?;
        g = g.args(["-c", name]);
        match req.track {
            Some(true) => g = g.arg("--track"),
            Some(false) => g = g.arg("--no-track"),
            None => {}
        }
        if let Some(sp) = req.start_point.as_deref().or(req.rev.as_deref()).filter(|s| !s.is_empty()) {
            g = g.args(["--end-of-options", check_rev(sp)?]);
        }
    } else {
        let rev = req.rev.as_deref().ok_or_else(|| ApiError::bad_request("give ref or create"))?;
        let rev = check_rev(rev)?;
        if req.detach || !is_local_branch(repo, rev).await? {
            g = g.args(["--detach", "--end-of-options", rev]);
        } else {
            g = g.args(["--end-of-options", rev]);
        }
    }
    g.run_ok_retry_lock().await?;
    Ok(())
}

pub async fn checkout(repo: &Repo, req: &CheckoutRequest) -> Result<OpOutcome, ApiError> {
    if !req.smart {
        return match checkout_once(repo, req).await {
            Ok(()) => Ok(OpOutcome { ok: true, message: "Checked out".into(), state: detect_state(&repo.git_dir).0, ..Default::default() }),
            Err(e) if e.code == "dirty_tree" => {
                Ok(OpOutcome { ok: false, message: e.message, state: detect_state(&repo.git_dir).0, dirty: true, ..Default::default() })
            }
            Err(e) => Err(e),
        };
    }
    // Smart checkout: stash (tracked changes), switch, restore.
    let before = repo.git().args(["rev-parse", "--verify", "--quiet", "refs/stash"]).run().await?.text();
    repo.git_w().args(["stash", "push", "-m", "Workbench smart checkout"]).run_ok_retry_lock().await?;
    let after = repo.git().args(["rev-parse", "--verify", "--quiet", "refs/stash"]).run().await?.text();
    let stashed = before.trim() != after.trim();
    if let Err(e) = checkout_once(repo, req).await {
        if stashed {
            let _ = repo.git_w().args(["stash", "pop", "--index"]).run().await;
        }
        return Err(e);
    }
    if !stashed {
        return Ok(OpOutcome { ok: true, message: "Checked out".into(), state: detect_state(&repo.git_dir).0, ..Default::default() });
    }
    let out = repo.git_w().args(["stash", "pop"]).run().await?;
    let mut o = outcome(repo, out, "Checked out; local changes restored").await?;
    if o.conflicts {
        o.message = format!("Checked out, but your local changes conflict with it: {} (the stash was kept)", o.message);
    }
    Ok(o)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBranchRequest {
    pub name: String,
    pub start_point: Option<String>,
    #[serde(default)]
    pub checkout: bool,
}

pub async fn create_branch(repo: &Repo, req: &CreateBranchRequest) -> Result<(), ApiError> {
    if req.checkout {
        let c = CheckoutRequest {
            rev: None,
            create: Some(req.name.clone()),
            start_point: req.start_point.clone(),
            track: None,
            detach: false,
            force: false,
            smart: false,
        };
        return checkout_once(repo, &c).await;
    }
    check_ref_name(&repo.top, &req.name, "branch").await?;
    let mut g = repo.git_w().args(["branch", "--end-of-options", &req.name]);
    if let Some(sp) = req.start_point.as_deref().filter(|s| !s.is_empty()) {
        g = g.arg(check_rev(sp)?);
    }
    g.run_ok_retry_lock().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameBranchRequest {
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub force: bool,
}

pub async fn rename_branch(repo: &Repo, req: &RenameBranchRequest) -> Result<(), ApiError> {
    check_rev(&req.old)?;
    check_ref_name(&repo.top, &req.new, "branch").await?;
    repo.git_w().args(["branch", if req.force { "-M" } else { "-m" }, "--end-of-options", &req.old, &req.new]).run_ok_retry_lock().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBranchRequest {
    pub name: String,
    #[serde(default)]
    pub force: bool,
}

/// Delete a local branch. `Ok(Some(message))` when it is not fully merged (the
/// UI then offers a force delete); that is an expected answer, not an error.
pub async fn delete_branch(repo: &Repo, req: &DeleteBranchRequest) -> Result<Option<String>, ApiError> {
    check_rev(&req.name)?;
    match repo.git_w().args(["branch", if req.force { "-D" } else { "-d" }, "--end-of-options", &req.name]).run_ok_retry_lock().await {
        Ok(_) => Ok(None),
        Err(e) if e.code == "not_merged" => Ok(Some(e.message)),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------- merge / rebase / sequencer

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    #[serde(rename = "ref")]
    pub rev: String,
    #[serde(default)]
    pub no_ff: bool,
    #[serde(default)]
    pub ff_only: bool,
    #[serde(default)]
    pub squash: bool,
    pub message: Option<String>,
}

pub async fn merge(repo: &Repo, req: &MergeRequest) -> Result<OpOutcome, ApiError> {
    let rev = check_rev(&req.rev)?;
    let mut g = repo.git_w().args(["merge", "--no-edit"]).timeout(COMMIT_TIMEOUT);
    if req.no_ff {
        g = g.arg("--no-ff");
    }
    if req.ff_only {
        g = g.arg("--ff-only");
    }
    if req.squash {
        g = g.arg("--squash");
    }
    if let Some(m) = req.message.as_deref().filter(|m| !m.trim().is_empty()) {
        g = g.args(["-m", m]);
    }
    let out = g.args(["--end-of-options", rev]).run().await?;
    let msg = if out.text().contains("Already up to date") {
        "Already up to date".to_string()
    } else if req.squash {
        format!("Squashed {rev} into the index; review and commit")
    } else {
        format!("Merged {rev}")
    };
    outcome(repo, out, &msg).await
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseRequest {
    pub onto: String,
    #[serde(default)]
    pub autostash: bool,
}

pub async fn rebase(repo: &Repo, req: &RebaseRequest) -> Result<OpOutcome, ApiError> {
    let onto = check_rev(&req.onto)?;
    let mut g = repo.git_w().arg("rebase").timeout(COMMIT_TIMEOUT);
    if req.autostash {
        g = g.arg("--autostash");
    }
    let out = g.args(["--end-of-options", onto]).run().await?;
    outcome(repo, out, &format!("Rebased onto {onto}")).await
}

/// Continue / abort / skip whatever is in progress. `pre_args` / `env`: the
/// interactive rebase's `-c core.commentChar` and message editor (see `rebase_i`).
pub async fn sequencer(repo: &Repo, action: &str, pre_args: &[String], env: &[(String, String)]) -> Result<OpOutcome, ApiError> {
    let (state, _) = detect_state(&repo.git_dir);
    let cmd = match state {
        "merging" => "merge",
        "rebasing" => "rebase",
        "cherry-picking" => "cherry-pick",
        "reverting" => "revert",
        _ => return Err(ApiError::bad_request("no merge, rebase, cherry-pick or revert is in progress")),
    };
    if action == "skip" && cmd == "merge" {
        return Err(ApiError::bad_request("a merge cannot be skipped; abort it instead"));
    }
    if action == "continue" && has_conflicts(repo).await? {
        return Err(ApiError::bad_request("there are unresolved conflicts: resolve and stage every file first"));
    }
    let mut g = repo.git_w().args(pre_args.iter().cloned()).args([cmd, &format!("--{action}")]).timeout(COMMIT_TIMEOUT);
    for (k, v) in env {
        g = g.env(k, v.clone());
    }
    let out = g.run().await?;
    let done = match action {
        "abort" => format!("{} aborted", title(cmd)),
        "skip" => "Skipped".to_string(),
        _ => format!("{} continued", title(cmd)),
    };
    let mut o = outcome(repo, out, &done).await?;
    if cmd == "rebase" && action != "abort" && o.state == "rebasing" && !o.conflicts {
        o.message = stop_message(repo).await;
    }
    Ok(o)
}

/// Why a rebase is stopped (edit step, or a step that needs attention).
pub async fn stop_message(repo: &Repo) -> String {
    let (_, d) = detect_state(&repo.git_dir);
    let at = match &d.stopped {
        Some(sha) => {
            let o = repo.git().args(["log", "-1", "--format=%h \u{201c}%s\u{201d}", sha, "--"]).run().await;
            o.ok().filter(|o| o.ok()).map(|o| o.text().trim().to_string()).unwrap_or_else(|| sha.chars().take(10).collect())
        }
        None => "a commit".into(),
    };
    if d.edit {
        format!("Stopped for editing at {at}: change it (commit with Amend, or add commits), then Continue")
    } else {
        format!("Stopped at {at}: check the working tree, then Continue, Skip or Abort")
    }
}

/// Commit whole files (`req.paths`) and selected lines (`req.partial`) through a
/// temporary index holding HEAD, so the rest of the real index stays out of the
/// commit (hooks see that index too). Afterwards the real index takes the committed
/// version of those paths; what was not selected stays as unstaged changes.
async fn commit_selected(repo: &Repo, req: &CommitRequest, message: &str) -> Result<CommitResult, ApiError> {
    use super::diff::{DiffMode, DiffQuery, file_diff};
    use super::lines::{LineRef, PatchKind, check_selection, git_apply, line_patch};
    if !has_head(repo).await? {
        return Err(ApiError::bad_request("the first commit cannot be partial: commit whole files"));
    }
    if req.partial.len() > 2000 {
        return Err(ApiError::bad_request("too many files"));
    }
    let ix = super::shelf::TempIndex::new(repo, Some("HEAD")).await?;
    let mut whole: Vec<String> = match req.paths.as_ref() {
        Some(p) if !p.is_empty() => repo_paths(repo, p)?,
        _ => vec![],
    };
    let mut patches = vec![];
    for pf in &req.partial {
        let q = DiffQuery { path: pf.path.clone(), mode: Some(DiffMode::Compare), base: Some("HEAD".into()), ..Default::default() };
        let (diff, parsed) = file_diff(repo, &q).await?;
        if diff.fingerprint != pf.fingerprint {
            return Err(ApiError::conflict(format!("{} changed since its lines were selected; review them and try again", pf.path)));
        }
        if !diff.can_select_lines {
            return Err(ApiError::bad_request(format!("{}: lines cannot be selected in this file; include the whole file", pf.path)));
        }
        let sel: std::collections::HashSet<LineRef> = pf.lines.iter().copied().collect();
        let rp = repo.to_repo(&pf.path)?;
        if check_selection(&parsed, &sel)? {
            whole.push(rp);
            continue;
        }
        let kind = if parsed.new_file { PatchKind::Create } else { PatchKind::Modify };
        let mode = parsed.new_mode.clone().filter(|m| m.len() == 6).unwrap_or_else(|| "100644".into());
        let patch = line_patch(&parsed, &sel, false, rp.as_bytes(), kind, &mode)?;
        // A renamed file: HEAD (the temporary index) has only the old name. Commit the
        // rename itself (the old content under the new name), then the selected lines.
        let old = match diff.old_path.as_deref().filter(|o| *o != pf.path) {
            Some(o) if !parsed.new_file => Some(repo.to_repo(o)?),
            _ => None,
        };
        patches.push((rp, patch, old));
    }
    if whole.is_empty() && patches.is_empty() {
        return Err(ApiError::bad_request("Nothing to commit: select files or lines"));
    }
    if !whole.is_empty() {
        // A staged rename is committed whole (else the old name's deletion stays behind).
        for f in entries_for(repo, &whole).await?.iter().filter(|f| f.index == 'R') {
            for side in std::iter::once(&f.path).chain(&f.orig_path) {
                if !whole.contains(side) {
                    whole.push(side.clone());
                }
            }
        }
        ix.git(repo).args(["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"]).stdin(pathspec_stdin(&whole)).run_ok().await?;
    }
    let mut touched = whole.clone();
    for (rp, patch, old) in patches {
        if let Some(old) = old {
            rename_in_index(repo, &ix, &old, &rp).await?;
            touched.push(old);
        }
        git_apply(repo, patch, &["--cached"], Some(&ix.path)).await?;
        touched.push(rp);
    }
    let mut g = ix.git(repo).arg("commit").timeout(COMMIT_TIMEOUT);
    g = if message.trim().is_empty() { g.arg("--no-edit") } else { g.args(["--cleanup=whitespace", "-F", "-"]).stdin(format!("{message}\n")) };
    if req.amend {
        g = g.arg("--amend");
    }
    if req.signoff {
        g = g.arg("--signoff");
    }
    if req.no_verify {
        g = g.arg("--no-verify");
    }
    let out = g.run().await?;
    if !out.ok() {
        let raw = format!("{}\n{}", out.text(), out.stderr);
        if raw.contains("nothing to commit") || raw.contains("no changes added to commit") {
            return Err(ApiError::bad_request("Nothing to commit: the selected changes are already in HEAD"));
        }
        if raw.contains("Please tell me who you are") {
            return Err(ApiError::not_configured("git does not know who you are: set user.name and user.email (git config --global user.name \u{2026})"));
        }
        return Err(git_error(&out));
    }
    drop(ix);
    // The real index: the committed version of every touched path.
    repo.git_w()
        .args(["reset", "-q", "HEAD", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(pathspec_stdin(&touched))
        .run_ok_retry_lock()
        .await?;
    let sha = repo.git().args(["rev-parse", "HEAD"]).run_ok().await?.text().trim().to_string();
    let summary = out.text().lines().next().unwrap_or_default().to_string();
    Ok(CommitResult { sha, summary })
}

/// In a temporary index holding HEAD: move HEAD's `old` entry (mode and blob) to `new`.
async fn rename_in_index(repo: &Repo, ix: &super::shelf::TempIndex, old: &str, new: &str) -> Result<(), ApiError> {
    let out = repo.git().args(["ls-tree", "-z", "--full-tree", "HEAD", "--"]).arg(literal(old)).run_ok().await?;
    // `<mode> blob <oid>\t<path>`
    let rec = out.text();
    let meta = rec.split('\t').next().unwrap_or_default();
    let f: Vec<&str> = meta.split_whitespace().collect();
    let [mode, "blob", oid] = f.as_slice() else {
        return Err(ApiError::conflict(format!("{old} is not a file in HEAD; refresh the diff and try again")));
    };
    ix.git(repo).args(["rm", "--cached", "-q", "--"]).arg(literal(old)).run_ok().await?;
    ix.git(repo).args(["update-index", "--add", "--cacheinfo", &format!("{mode},{oid},{new}")]).run_ok().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoCommitRequest {
    /// The commit the client saw as HEAD.
    pub sha: String,
}

/// CLion's "Undo Commit": move HEAD back one commit, keeping its changes staged.
/// Only for an unpublished, non-merge, non-root HEAD. Returns the undone message
/// (the UI puts it back into the commit box).
pub async fn undo_commit(repo: &Repo, req: &UndoCommitRequest) -> Result<String, ApiError> {
    let (state, _) = detect_state(&repo.git_dir);
    if state != "clean" {
        return Err(ApiError::conflict(format!("finish the {state} first")));
    }
    let head = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?.text().trim().to_string();
    let want = check_rev(&req.sha)?;
    if head.is_empty() || want.len() < 7 || !head.starts_with(want) {
        return Err(ApiError::conflict("HEAD moved: only the current HEAD commit can be undone; refresh and try again"));
    }
    let parents = repo.git().args(["rev-list", "--parents", "-n1", "HEAD"]).run_ok().await?.text().split_whitespace().count().saturating_sub(1);
    match parents {
        0 => return Err(ApiError::bad_request("the first commit of a repository cannot be undone")),
        1 => {}
        _ => return Err(ApiError::bad_request("a merge commit cannot be undone here: use Reset Current Branch to Here")),
    }
    let published = repo
        .git()
        .args(["for-each-ref", "--count=1", "--format=%(refname:short)", "--contains", "HEAD", "refs/remotes"])
        .run_ok()
        .await?;
    let on = published.text().trim().to_string();
    if !on.is_empty() {
        return Err(ApiError::conflict(format!("The commit is already on {on}: undoing it would rewrite published history. Revert it instead.")));
    }
    let message = last_commit_message(repo).await?;
    repo.git_w().args(["reset", "--soft", "HEAD~1", "--"]).run_ok_retry_lock().await?;
    Ok(message)
}

fn title(cmd: &str) -> String {
    let mut c = cmd.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShasRequest {
    pub shas: Vec<String>,
}

pub async fn cherry_pick(repo: &Repo, req: &ShasRequest) -> Result<OpOutcome, ApiError> {
    if req.shas.is_empty() {
        return Err(ApiError::bad_request("no commits given"));
    }
    let mut g = repo.git_w().args(["cherry-pick", "--end-of-options"]).timeout(COMMIT_TIMEOUT);
    for s in &req.shas {
        g = g.arg(check_rev(s)?);
    }
    let out = g.run().await?;
    outcome(repo, out, &format!("Cherry-picked {} commit(s)", req.shas.len())).await
}

pub async fn revert(repo: &Repo, req: &ShasRequest) -> Result<OpOutcome, ApiError> {
    if req.shas.is_empty() {
        return Err(ApiError::bad_request("no commits given"));
    }
    let mut g = repo.git_w().args(["revert", "--no-edit", "--end-of-options"]).timeout(COMMIT_TIMEOUT);
    for s in &req.shas {
        g = g.arg(check_rev(s)?);
    }
    let out = g.run().await?;
    outcome(repo, out, &format!("Reverted {} commit(s)", req.shas.len())).await
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetRequest {
    #[serde(rename = "ref")]
    pub rev: String,
    /// soft | mixed | hard | keep
    pub mode: String,
}

pub async fn reset(repo: &Repo, req: &ResetRequest) -> Result<(), ApiError> {
    let mode = match req.mode.as_str() {
        "soft" | "mixed" | "hard" | "keep" => req.mode.as_str(),
        _ => return Err(ApiError::bad_request("mode must be soft, mixed, hard or keep")),
    };
    repo.git_w().args(["reset", &format!("--{mode}"), "--end-of-options", check_rev(&req.rev)?, "--"]).run_ok_retry_lock().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRequest {
    pub name: String,
    #[serde(rename = "ref")]
    pub rev: Option<String>,
    /// Annotated tag message.
    pub message: Option<String>,
    #[serde(default)]
    pub force: bool,
}

pub async fn create_tag(repo: &Repo, req: &TagRequest) -> Result<(), ApiError> {
    check_ref_name(&repo.top, &req.name, "tag").await?;
    let mut g = repo.git_w().arg("tag");
    if req.force {
        g = g.arg("--force");
    }
    if let Some(m) = req.message.as_deref().filter(|m| !m.trim().is_empty()) {
        g = g.args(["--annotate", "-F", "-"]).stdin(format!("{m}\n"));
    }
    g = g.args(["--end-of-options", &req.name]);
    if let Some(r) = req.rev.as_deref().filter(|r| !r.is_empty()) {
        g = g.arg(check_rev(r)?);
    }
    g.run_ok_retry_lock().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
pub struct NameRequest {
    pub name: String,
}

pub async fn delete_tag(repo: &Repo, name: &str) -> Result<(), ApiError> {
    check_rev(name)?;
    repo.git_w().args(["tag", "-d", "--end-of-options", name]).run_ok_retry_lock().await?;
    Ok(())
}

// ---------------------------------------------------------------- stash

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashPushRequest {
    pub message: Option<String>,
    #[serde(default)]
    pub include_untracked: bool,
    #[serde(default)]
    pub keep_index: bool,
    /// Stash only these files.
    pub paths: Option<Vec<String>>,
    /// Stash only what is staged.
    #[serde(default)]
    pub staged: bool,
}

pub async fn stash_push(repo: &Repo, req: &StashPushRequest) -> Result<(), ApiError> {
    let mut g = repo.git_w().args(["stash", "push"]);
    if let Some(m) = req.message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        g = g.args(["-m", m]);
    }
    if req.include_untracked {
        g = g.arg("--include-untracked");
    }
    if req.keep_index {
        g = g.arg("--keep-index");
    }
    if req.staged {
        g = g.arg("--staged");
    }
    if let Some(p) = req.paths.as_ref().filter(|p| !p.is_empty()) {
        g = g.arg("--").args(repo_paths(repo, p)?.iter().map(|p| literal(p)));
    }
    let out = g.run_ok_retry_lock().await?;
    if out.text().contains("No local changes to save") {
        return Err(ApiError::bad_request("No local changes to stash"));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashRefRequest {
    pub index: u32,
    /// Expected sha of the entry (guards against a shifted stash list).
    pub sha: Option<String>,
    /// Also restore the index state (`--index`).
    #[serde(default)]
    pub reinstate_index: bool,
}

pub async fn stash_apply(repo: &Repo, req: &StashRefRequest, pop: bool) -> Result<OpOutcome, ApiError> {
    let r = super::stash::stash_ref(repo, req.index, req.sha.as_deref()).await?;
    let mut g = repo.git_w().args(["stash", if pop { "pop" } else { "apply" }]);
    if req.reinstate_index {
        g = g.arg("--index");
    }
    let out = g.arg(&r).run().await?;
    let mut o = outcome(repo, out, if pop { "Unstashed (popped)" } else { "Unstashed (applied)" }).await?;
    if o.conflicts && pop {
        o.message = format!("{} The stash was kept.", o.message);
    }
    Ok(o)
}

pub async fn stash_drop(repo: &Repo, req: &StashRefRequest) -> Result<(), ApiError> {
    let r = super::stash::stash_ref(repo, req.index, req.sha.as_deref()).await?;
    repo.git_w().args(["stash", "drop", &r]).run_ok_retry_lock().await?;
    Ok(())
}

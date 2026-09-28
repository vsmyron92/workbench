//! Shelf (JetBrains): set local changes aside outside git's own stash.
//!
//! A shelf lives in `data_dir/git/shelf/<pid>/<id>/`: `meta.json` (name, created,
//! base commit, branch, files) and one binary-safe patch per file under `files/`
//! (`git diff --binary --full-index`, so a new or binary file is complete and a
//! 3-way unshelve finds its base blobs).
//!
//! Shelving builds the patch from a temporary index (HEAD plus the selected files'
//! working-tree state, untracked files included), writes the shelf, checks that the
//! patch reverse-applies to the working tree (it describes exactly what is there),
//! and only then rolls the files back. Unshelving is `git apply --3way`: a clean
//! apply leaves modifications unstaged (new files staged, like `git stash apply`),
//! conflicts leave unmerged entries for the conflict panel. The shelf stays until the
//! user deletes it or asks to remove what was unshelved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cmd::{literal, pathspec_stdin};
use super::diff::empty_tree;
use super::log::parse_name_status;
use super::repo::Repo;
use super::status::entries_for;
use crate::error::ApiError;

const MAX_SHELVES: usize = 500;
const MAX_FILES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfFile {
    /// Project-relative path (the new name of a rename).
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_path: Option<String>,
    /// A M D R C T
    pub status: char,
    #[serde(default)]
    pub binary: bool,
    /// Patch file name under `files/`.
    pub patch: String,
    /// The changelist (id) the file was in when shelved; unshelving without a chosen
    /// changelist puts it back there.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub changelist: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfMeta {
    pub id: String,
    pub name: String,
    /// Unix ms.
    pub created: i64,
    /// HEAD when shelved (None before the first commit).
    pub base: Option<String>,
    pub branch: Option<String>,
    pub files: Vec<ShelfFile>,
    /// A commit object (base + the shelved changes) for showing diffs; rebuilt when gone.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_commit: Option<String>,
}

pub fn shelf_root(data_dir: &Path, pid: &str) -> PathBuf {
    data_dir.join("git").join("shelf").join(pid)
}

fn check_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(ApiError::bad_request("invalid shelf id"));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<String, ApiError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(ApiError::bad_request("the shelf needs a name"));
    }
    if n.chars().count() > 300 || n.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad_request("the shelf name is too long or has control characters"));
    }
    Ok(n.lines().next().unwrap_or(n).to_string())
}

// ---------------------------------------------------------------- storage (blocking)

pub fn load(root: &Path, id: &str) -> Result<ShelfMeta, ApiError> {
    check_id(id)?;
    match crate::util::fs::read_json::<ShelfMeta>(&root.join(id).join("meta.json")) {
        Ok(Some(m)) => Ok(m),
        Ok(None) => Err(ApiError::not_found(format!("no shelf {id}"))),
        Err(e) => Err(ApiError::internal(format!("shelf {id} is unreadable: {e:#}"))),
    }
}

fn save(root: &Path, meta: &ShelfMeta) -> Result<(), ApiError> {
    crate::util::fs::write_json(&root.join(&meta.id).join("meta.json"), meta).map_err(|e| ApiError::internal(format!("cannot save the shelf: {e:#}")))
}

/// Every shelf of a project, newest first (unreadable ones are skipped).
pub fn list(root: &Path) -> Vec<ShelfMeta> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut v: Vec<ShelfMeta> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| crate::util::fs::read_json::<ShelfMeta>(&e.path().join("meta.json")).ok().flatten())
        .collect();
    v.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    v.truncate(MAX_SHELVES);
    v
}

pub fn rename(root: &Path, id: &str, name: &str) -> Result<ShelfMeta, ApiError> {
    let mut m = load(root, id)?;
    m.name = check_name(name)?;
    save(root, &m)?;
    Ok(m)
}

/// Remember each file's changelist (project-relative path → list id).
pub fn record_changelists(root: &Path, id: &str, lists: &std::collections::BTreeMap<String, String>) -> Result<ShelfMeta, ApiError> {
    let mut m = load(root, id)?;
    for f in &mut m.files {
        f.changelist = lists.get(&f.path).or_else(|| f.old_path.as_ref().and_then(|o| lists.get(o))).cloned();
    }
    save(root, &m)?;
    Ok(m)
}

pub fn delete(root: &Path, id: &str) -> Result<(), ApiError> {
    check_id(id)?;
    let dir = root.join(id);
    if !dir.join("meta.json").exists() {
        return Err(ApiError::not_found(format!("no shelf {id}")));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| ApiError::internal(format!("cannot delete the shelf: {e}")))
}

fn read_patches(root: &Path, meta: &ShelfMeta, files: &[&ShelfFile]) -> Result<Vec<u8>, ApiError> {
    let mut out = vec![];
    for f in files {
        if f.patch.contains('/') || f.patch.starts_with('.') {
            return Err(ApiError::internal("corrupt shelf metadata"));
        }
        let p = root.join(&meta.id).join("files").join(&f.patch);
        let data = std::fs::read(&p).map_err(|e| ApiError::internal(format!("shelf patch {} is missing: {e}", f.patch)))?;
        out.extend_from_slice(&data);
    }
    Ok(out)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError::internal(e.to_string()))?
}

// ---------------------------------------------------------------- a temporary index

/// An index file of our own next to the repository's, removed on drop.
pub struct TempIndex {
    pub path: PathBuf,
}

impl TempIndex {
    /// A temporary index holding `tree` (a commit or tree), or nothing.
    pub async fn new(repo: &Repo, tree: Option<&str>) -> Result<Self, ApiError> {
        let ix = TempIndex { path: repo.git_dir.join(format!("wb-index-{}", crate::util::random_token(8))) };
        let g = ix.git(repo).arg("read-tree");
        match tree {
            Some(t) => g.args(["--end-of-options", t]),
            None => g.arg("--empty"),
        }
        .run_ok()
        .await?;
        Ok(ix)
    }

    /// A git command on this index.
    pub fn git(&self, repo: &Repo) -> super::cmd::Git {
        repo.git_w().env("GIT_INDEX_FILE", self.path.to_string_lossy().to_string())
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("lock"));
    }
}

async fn head(repo: &Repo) -> Result<Option<String>, ApiError> {
    let o = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?;
    Ok(o.ok().then(|| o.text().trim().to_string()).filter(|s| !s.is_empty()))
}

async fn commit_exists(repo: &Repo, sha: &str) -> Result<bool, ApiError> {
    Ok(repo.git().args(["cat-file", "-e", &format!("{sha}^{{commit}}")]).run().await?.ok())
}

// ---------------------------------------------------------------- shelve

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelveRequest {
    pub name: String,
    /// Project-relative paths (a changelist's files are resolved by the route).
    #[serde(default)]
    pub paths: Vec<String>,
    /// Shelve the files of this changelist.
    pub changelist: Option<String>,
    /// Keep the changes in the working tree too (a copy on the shelf).
    #[serde(default)]
    pub keep: bool,
}

/// Shelve `paths` and roll them back. The caller holds the repository write lock.
pub async fn shelve(repo: &Repo, root: &Path, name: &str, paths: &[String], keep: bool) -> Result<ShelfMeta, ApiError> {
    let name = check_name(name)?;
    if paths.is_empty() {
        return Err(ApiError::bad_request("no files to shelve"));
    }
    if paths.len() > MAX_FILES {
        return Err(ApiError::bad_request("too many files"));
    }
    let wanted: Vec<String> = paths.iter().map(|p| repo.to_repo(p)).collect::<Result<_, _>>()?;
    let entries: Vec<_> = entries_for(repo, &wanted).await?.into_iter().filter(|f| f.index != '!').collect();
    if let Some(c) = entries.iter().find(|f| f.conflict) {
        return Err(ApiError::bad_request(format!("{} has conflicts: resolve it before shelving", repo.to_project(&c.path))));
    }
    if let Some(s) = entries.iter().find(|f| f.submodule) {
        return Err(ApiError::bad_request(format!("{} is a submodule: its changes cannot be shelved", repo.to_project(&s.path))));
    }
    let mut include: Vec<String> = vec![];
    let mut seen = HashSet::new();
    for f in &entries {
        for p in std::iter::once(&f.path).chain(f.orig_path.iter()) {
            if seen.insert(p.clone()) {
                include.push(p.clone());
            }
        }
    }
    if include.is_empty() {
        return Err(ApiError::bad_request("the selected files have no changes to shelve"));
    }
    let base = head(repo).await?;
    let base_tree = match &base {
        Some(b) => b.clone(),
        None => empty_tree(repo).await?,
    };
    let ix = TempIndex::new(repo, base.as_deref()).await?;
    ix.git(repo)
        .args(["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(pathspec_stdin(&include))
        .run_ok()
        .await?;
    // The temporary index differs from the base exactly in the selected files.
    let ns = ix.git(repo).args(["diff", "--cached", "--name-status", "-z", "-M", "--no-ext-diff", "--end-of-options", &base_tree, "--"]).run_ok().await?;
    let changes = parse_name_status(&ns.stdout);
    if changes.is_empty() {
        return Err(ApiError::bad_request("the selected files have no changes to shelve"));
    }
    let id = format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), crate::util::random_token(3).replace(['-', '_'], "x"));
    let diff_args = |g: super::cmd::Git| {
        g.args([
            "diff",
            "--cached",
            "--binary",
            "--full-index",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--end-of-options",
            &base_tree,
            "--",
        ])
    };
    // One diff for everything, split per file (it lists the files in name-status
    // order); one diff per file only if that does not line up.
    let all_out = diff_args(ix.git(repo)).run_ok().await?;
    if all_out.truncated {
        return Err(ApiError::bad_request("the changes are too large to shelve"));
    }
    let mut chunks = split_patch(&all_out.stdout);
    if chunks.len() != changes.len() {
        chunks = vec![];
        for (_, path, old) in &changes {
            let mut g = diff_args(ix.git(repo)).arg(literal(path));
            if let Some(o) = old {
                g = g.arg(literal(o));
            }
            let out = g.run_ok().await?;
            if out.truncated {
                return Err(ApiError::bad_request(format!("{} is too large to shelve", repo.to_project(path))));
            }
            chunks.push(out.stdout);
        }
    }
    let mut files = vec![];
    let mut patches: Vec<(String, Vec<u8>)> = vec![];
    for (n, ((status, path, old), chunk)) in changes.iter().zip(chunks).enumerate() {
        if chunk.is_empty() {
            continue;
        }
        let binary = chunk.windows(16).any(|w| w == b"GIT binary patch") || chunk.windows(12).any(|w| w == b"Binary files");
        let patch = format!("{:04}.patch", n + 1);
        files.push(ShelfFile {
            path: repo.to_project(path),
            old_path: old.as_deref().map(|o| repo.to_project(o)),
            status: *status,
            binary,
            patch: patch.clone(),
            changelist: None,
        });
        patches.push((patch, chunk));
    }
    drop(ix);
    let meta = ShelfMeta { id: id.clone(), name, created: crate::util::now_ms(), base, branch: crate::util::git::current_branch(&repo.top).await, files, view_commit: None };

    // Write the shelf (staged under a dot-folder, then renamed into place).
    let (root2, meta2) = (root.to_path_buf(), meta.clone());
    let all: Vec<u8> = patches.iter().flat_map(|(_, p)| p.iter().copied()).collect();
    blocking(move || {
        let tmp = root2.join(format!(".{}.tmp", meta2.id));
        std::fs::create_dir_all(tmp.join("files")).map_err(|e| ApiError::internal(format!("cannot write the shelf: {e}")))?;
        crate::util::fs::set_mode(&tmp, 0o700);
        for (name, data) in &patches {
            crate::util::fs::write_atomic(&tmp.join("files").join(name), data, 0o600).map_err(|e| ApiError::internal(format!("{e:#}")))?;
        }
        crate::util::fs::write_json(&tmp.join("meta.json"), &meta2).map_err(|e| ApiError::internal(format!("{e:#}")))?;
        std::fs::rename(&tmp, root2.join(&meta2.id)).map_err(|e| ApiError::internal(format!("cannot write the shelf: {e}")))
    })
    .await?;

    // The shelf must describe exactly what is in the working tree before anything is rolled back.
    let check = repo.git().args(["apply", "--check", "-R", "--binary", "-"]).stdin(all).run().await?;
    if !check.ok() {
        let (root2, id2) = (root.to_path_buf(), id.clone());
        let _ = blocking(move || delete(&root2, &id2)).await;
        return Err(ApiError::internal(format!(
            "the shelved patch does not match the working tree ({}); nothing was changed",
            check.message()
        )));
    }
    if keep {
        return Ok(meta);
    }
    rollback_files(repo, &meta).await.map_err(|e| {
        ApiError::internal(format!("Shelved as “{}”, but rolling the files back failed: {}", meta.name, e.message))
    })?;
    Ok(meta)
}

/// Split `git diff` output into one patch per file (at each `diff --git` line: patch
/// content lines always start with ' ', '+', '-', '\\' or base85, never with that).
fn split_patch(out: &[u8]) -> Vec<Vec<u8>> {
    let mut chunks: Vec<Vec<u8>> = vec![];
    for line in out.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"diff --git ") || chunks.is_empty() {
            chunks.push(vec![]);
        }
        if let Some(c) = chunks.last_mut() {
            c.extend_from_slice(line);
        }
    }
    chunks.retain(|c| c.starts_with(b"diff --git "));
    chunks
}

/// Which of these repository paths exist in `rev` (one `ls-tree` per 1000 paths).
pub(super) async fn paths_in_tree(repo: &Repo, rev: Option<&str>, paths: &[String]) -> Result<HashSet<String>, ApiError> {
    let mut found = HashSet::new();
    let Some(rev) = rev else { return Ok(found) };
    for chunk in paths.chunks(1000) {
        let out = repo
            .git()
            .args(["ls-tree", "-r", "-z", "--name-only", "--full-tree", "--end-of-options", rev, "--"])
            .args(chunk.iter().map(|p| literal(p)))
            .run_ok()
            .await?;
        found.extend(super::cmd::split_z(&out.stdout));
    }
    Ok(found)
}

/// Put the shelved files back to the base: restore what the base has, delete the rest.
async fn rollback_files(repo: &Repo, meta: &ShelfMeta) -> Result<(), ApiError> {
    let mut all = vec![];
    for f in &meta.files {
        for p in std::iter::once(&f.path).chain(f.old_path.iter()) {
            all.push(repo.to_repo(p)?);
        }
    }
    let in_base = paths_in_tree(repo, meta.base.as_deref(), &all).await?;
    let (restore, remove): (Vec<String>, Vec<String>) = all.into_iter().partition(|p| in_base.contains(p));
    if !restore.is_empty() {
        repo.git_w()
            .args(["restore", "--source=HEAD", "--staged", "--worktree", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin(pathspec_stdin(&restore))
            .run_ok_retry_lock()
            .await?;
    }
    if !remove.is_empty() {
        repo.git_w()
            .args(["rm", "--cached", "-r", "--quiet", "--ignore-unmatch", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin(pathspec_stdin(&remove))
            .run_ok_retry_lock()
            .await?;
        for rp in &remove {
            let abs = crate::util::paths::resolve_entry_in_root(&repo.top, rp)?;
            if let Ok(m) = tokio::fs::symlink_metadata(&abs).await {
                if !m.is_dir() {
                    tokio::fs::remove_file(&abs).await?;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- show

/// A commit (parent: the shelf's base, or HEAD when that is gone) whose tree holds the
/// shelved changes, for the diff panels. Cached in the metadata while it exists.
pub async fn view_commit(repo: &Repo, root: &Path, id: &str) -> Result<ShelfMeta, ApiError> {
    let (r, i) = (root.to_path_buf(), id.to_string());
    let mut meta = blocking(move || load(&r, &i)).await?;
    if let Some(c) = &meta.view_commit {
        if commit_exists(repo, c).await? {
            return Ok(meta);
        }
    }
    let parent = match &meta.base {
        Some(b) if commit_exists(repo, b).await? => Some(b.clone()),
        _ => head(repo).await?,
    };
    let ix = TempIndex::new(repo, parent.as_deref()).await?;
    let (r, m) = (root.to_path_buf(), meta.clone());
    let patch = blocking(move || {
        let files: Vec<&ShelfFile> = m.files.iter().collect();
        read_patches(&r, &m, &files)
    })
    .await?;
    let applied = ix.git(repo).args(["apply", "--cached", "--binary", "--whitespace=nowarn", "-"]).stdin(patch).run().await?;
    if !applied.ok() {
        return Err(ApiError::conflict(format!("the shelf no longer applies to its base commit: {}", applied.message())));
    }
    let tree = ix.git(repo).arg("write-tree").run_ok().await?.text().trim().to_string();
    let mut g = repo
        .git_w()
        .args(["commit-tree", &tree, "-m", &format!("Shelf: {}", meta.name)])
        .env("GIT_AUTHOR_NAME", "Workbench shelf")
        .env("GIT_AUTHOR_EMAIL", "shelf@workbench.invalid")
        .env("GIT_COMMITTER_NAME", "Workbench shelf")
        .env("GIT_COMMITTER_EMAIL", "shelf@workbench.invalid");
    if let Some(p) = &parent {
        g = g.args(["-p", p]);
    }
    let sha = g.run_ok().await?.text().trim().to_string();
    meta.view_commit = Some(sha);
    let (r, m) = (root.to_path_buf(), meta.clone());
    blocking(move || save(&r, &m)).await?;
    Ok(meta)
}

// ---------------------------------------------------------------- unshelve

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UnshelveRequest {
    /// Only these files (project-relative); default all.
    pub paths: Option<Vec<String>>,
    /// Remove the unshelved files from the shelf (the shelf itself once empty).
    #[serde(default)]
    pub remove: bool,
    /// Put the unshelved files into this changelist.
    pub changelist: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UnshelveResult {
    pub ok: bool,
    pub conflicts: bool,
    pub message: String,
    /// Project-relative paths now changed in the working tree.
    pub applied: Vec<String>,
    pub conflicted: Vec<String>,
    /// The shelf was deleted (everything unshelved with `remove`).
    pub removed: bool,
}

/// Apply (some of) a shelf with a 3-way merge. The caller holds the repository write lock.
pub async fn unshelve(repo: &Repo, root: &Path, id: &str, req: &UnshelveRequest) -> Result<UnshelveResult, ApiError> {
    let (r, i) = (root.to_path_buf(), id.to_string());
    let meta = blocking(move || load(&r, &i)).await?;
    let chosen: Vec<ShelfFile> = match &req.paths {
        Some(p) if !p.is_empty() => {
            let want: HashSet<&str> = p.iter().map(String::as_str).collect();
            meta.files.iter().filter(|f| want.contains(f.path.as_str())).cloned().collect()
        }
        _ => meta.files.clone(),
    };
    if chosen.is_empty() {
        return Err(ApiError::bad_request("none of the given files is on this shelf"));
    }
    let (r, m, c) = (root.to_path_buf(), meta.clone(), chosen.clone());
    let patch = blocking(move || {
        let refs: Vec<&ShelfFile> = c.iter().collect();
        read_patches(&r, &m, &refs)
    })
    .await?;
    let touched: Vec<String> =
        chosen.iter().flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone())).map(|p| repo.to_repo(&p)).collect::<Result<_, _>>()?;
    let before: HashSet<String> = super::ops::conflicted_paths(repo).await?.into_iter().collect();
    let out = repo.git_w().args(["apply", "--3way", "--binary", "--whitespace=nowarn", "-"]).stdin(patch).run().await?;
    let conflicted: Vec<String> = super::ops::conflicted_paths(repo).await?.into_iter().filter(|p| !before.contains(p) && touched.contains(p)).collect();
    if !out.ok() && conflicted.is_empty() {
        let msg = out.message();
        let hint = if msg.contains("does not match index") || msg.contains("already exists in working directory") || msg.contains("does not exist in index") {
            "\n\nThe files have local changes that are in the way: commit, shelve or roll them back first."
        } else {
            ""
        };
        return Err(ApiError::conflict(format!("Unshelve failed: {msg}{hint}")));
    }
    // Like `git stash apply`: modifications and deletions unstaged, new files staged,
    // renames staged (both sides, so they stay a rename).
    let renamed: HashSet<String> = chosen
        .iter()
        .filter(|f| f.status == 'R' || f.status == 'C')
        .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
        .filter_map(|p| repo.to_repo(&p).ok())
        .collect();
    let in_head = paths_in_tree(repo, Some("HEAD"), &touched).await.unwrap_or_default();
    let unstage: Vec<String> = touched.iter().filter(|p| !conflicted.contains(p) && !renamed.contains(*p) && in_head.contains(*p)).cloned().collect();
    if !unstage.is_empty() {
        repo.git_w()
            .args(["reset", "-q", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin(pathspec_stdin(&unstage))
            .run_ok_retry_lock()
            .await?;
    }
    let applied: Vec<String> = chosen.iter().map(|f| f.path.clone()).collect();
    let conflicted_p: Vec<String> = conflicted.iter().map(|p| repo.to_project(p)).collect();
    let mut removed = false;
    if req.remove && conflicted.is_empty() {
        let (r, mut m) = (root.to_path_buf(), meta.clone());
        let done: HashSet<String> = applied.iter().cloned().collect();
        removed = blocking(move || {
            m.files.retain(|f| !done.contains(&f.path));
            if m.files.is_empty() {
                delete(&r, &m.id)?;
                Ok(true)
            } else {
                m.view_commit = None;
                save(&r, &m)?;
                Ok(false)
            }
        })
        .await?;
    }
    let n = applied.len();
    let message = if conflicted.is_empty() {
        format!("Unshelved {n} file{} from “{}”", if n == 1 { "" } else { "s" }, meta.name)
    } else {
        format!(
            "Unshelved “{}” with conflicts in {} file{}: resolve them (the shelf is kept)",
            meta.name,
            conflicted.len(),
            if conflicted.len() == 1 { "" } else { "s" }
        )
    };
    Ok(UnshelveResult { ok: conflicted.is_empty(), conflicts: !conflicted.is_empty(), message, applied, conflicted: conflicted_p, removed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_diff_per_file() {
        let d = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-diff --git a/fake b/fake\n+y\ndiff --git a/bin b/bin\nGIT binary patch\nliteral 3\nKcmZ?\n\n";
        let c = split_patch(d);
        assert_eq!(c.len(), 2);
        assert!(c[0].ends_with(b"+y\n") && c[1].starts_with(b"diff --git a/bin b/bin\n"));
        assert!(split_patch(b"").is_empty());
    }

    #[test]
    fn validates_ids_and_names() {
        assert!(check_id("20260927-101010-abcd").is_ok());
        assert!(check_id("../x").is_err());
        assert!(check_id("").is_err());
        assert_eq!(check_name("  WIP: parser\nmore").unwrap(), "WIP: parser");
        assert!(check_name("   ").is_err());
    }

    #[test]
    fn lists_newest_first_and_skips_broken_shelves() {
        let d = tempfile::tempdir().unwrap();
        let mk = |id: &str, created: i64| ShelfMeta { id: id.into(), name: id.into(), created, base: None, branch: None, files: vec![], view_commit: None };
        for (id, t) in [("a", 1), ("b", 3), ("c", 2)] {
            save(d.path(), &mk(id, t)).unwrap();
        }
        std::fs::create_dir_all(d.path().join("broken")).unwrap();
        std::fs::write(d.path().join("broken/meta.json"), "{").unwrap();
        std::fs::create_dir_all(d.path().join(".x.tmp")).unwrap();
        let ids: Vec<String> = list(d.path()).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
        assert_eq!(rename(d.path(), "a", "Renamed").unwrap().name, "Renamed");
        delete(d.path(), "a").unwrap();
        assert_eq!(load(d.path(), "a").unwrap_err().status.as_u16(), 404);
    }
}

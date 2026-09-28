//! File diffs (`GitFileDiff`, cross-slice contract) and hunk-level staging.
//!
//! Modes:
//! * `working`: index → working tree (the unstaged part). Hunks can be staged or discarded.
//! * `staged`:  HEAD → index. Hunks can be unstaged.
//! * `commit`:  first parent (or the empty tree) → `sha`. Read-only.
//! * `compare`: `base` → `head` (or the working tree when `head` is empty). Read-only.
//!
//! Hunk operations re-run the same diff, compare its fingerprint with the one the
//! client saw (409 when the file moved on — another agent may be editing it), then
//! build a patch containing only the chosen hunks and feed it to `git apply`.
//! Patches are built from the raw diff bytes, so files in any encoding round-trip.

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::cmd::{check_rev, literal, split_z};
use super::lines::{ChangedLine, changed_lines};
use super::repo::Repo;
use super::status::{StatusFile, entries_for, narrow_entries};
use crate::error::ApiError;

/// Sides larger than this are not sent (`tooLarge: true`).
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffMode {
    Working,
    Staged,
    Commit,
    Compare,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HunkInfo {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub original: String,
    pub modified: String,
    pub binary: bool,
    pub too_large: bool,
    pub hunks: Vec<HunkInfo>,
    pub fingerprint: String,
    // ---- additions beyond the contract
    pub mode: DiffMode,
    /// Hunk-level stage/unstage/discard is possible for this diff (plain content
    /// change; not binary, new, deleted, renamed or conflicted).
    pub can_stage_hunks: bool,
    /// One side is a Git LFS pointer (contents live outside git).
    pub lfs: bool,
    pub untracked: bool,
    pub conflict: bool,
    /// The file does not exist on the original / modified side.
    pub original_missing: bool,
    pub modified_missing: bool,
    /// File mode change, e.g. `100644 → 100755`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_change: Option<String>,
    /// Human description of the two sides ("Index", "Working tree", "abc1234"…).
    pub original_label: String,
    pub modified_label: String,
    /// The path is a submodule: there is no file content, `submoduleSummary`
    /// describes the change (`git diff --submodule=log`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub submodule: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodule_summary: Option<String>,
    /// Working mode: the submodule has a new commit checked out, which can be
    /// staged (changes *inside* it cannot).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub submodule_new_commit: bool,
    /// Single lines can be selected (stage/unstage/roll back lines, partial commit):
    /// a text diff with hunks that is not binary, LFS, a symlink, conflicted or huge.
    pub can_select_lines: bool,
    /// Every changed line with its number, when `canSelectLines`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<ChangedLine>,
}

// ---------------------------------------------------------------- parsing

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Text after the closing `@@` (function context), raw.
    pub suffix: Vec<u8>,
    /// Body lines including their prefix (' ', '+', '-', '\\'), without '\n'.
    pub body: Vec<Vec<u8>>,
}

impl ParsedHunk {
    pub fn header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@{}",
            self.old_start,
            self.old_lines,
            self.new_start,
            self.new_lines,
            String::from_utf8_lossy(&self.suffix)
        )
    }
    fn info(&self) -> HunkInfo {
        HunkInfo {
            header: self.header(),
            old_start: self.old_start,
            old_lines: self.old_lines,
            new_start: self.new_start,
            new_lines: self.new_lines,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedDiff {
    /// `diff --git`, `---` and `+++` lines (what a patch needs), raw.
    pub diff_line: Option<Vec<u8>>,
    pub minus_line: Option<Vec<u8>>,
    pub plus_line: Option<Vec<u8>>,
    pub hunks: Vec<ParsedHunk>,
    pub binary: bool,
    pub new_file: bool,
    pub deleted_file: bool,
    pub renamed: bool,
    pub copied: bool,
    pub submodule: bool,
    /// A symlink (mode 120000) on either side.
    pub symlink: bool,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
}

fn parse_range(s: &[u8]) -> Option<(u32, u32)> {
    let s = std::str::from_utf8(s).ok()?;
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// `@@ -a,b +c,d @@ suffix` → ranges and suffix.
fn parse_hunk_header(line: &[u8]) -> Option<(u32, u32, u32, u32, Vec<u8>)> {
    let rest = line.strip_prefix(b"@@ -")?;
    let sp = rest.iter().position(|b| *b == b' ')?;
    let (old, rest) = rest.split_at(sp);
    let rest = rest.strip_prefix(b" +")?;
    let end = rest.windows(3).position(|w| w == b" @@")?;
    let (new, suffix) = rest.split_at(end);
    let suffix = &suffix[3..];
    let (os, ol) = parse_range(old)?;
    let (ns, nl) = parse_range(new)?;
    Some((os, ol, ns, nl, suffix.to_vec()))
}

/// Parse the first file of a unified diff (as produced by `git diff`).
pub fn parse_diff(bytes: &[u8]) -> ParsedDiff {
    let mut d = ParsedDiff::default();
    let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut i = 0;
    let mut seen_file = false;
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with(b"diff --git ") || l.starts_with(b"diff --cc ") || l.starts_with(b"diff --combined ") {
            if seen_file {
                break; // only the first file
            }
            seen_file = true;
            d.diff_line = Some(l.to_vec());
        } else if l.starts_with(b"@@ ") {
            let Some((os, ol, ns, nl, suffix)) = parse_hunk_header(l) else {
                i += 1;
                continue;
            };
            let mut body = vec![];
            let (mut ro, mut rn) = (ol, nl);
            i += 1;
            while i < lines.len() && (ro > 0 || rn > 0) {
                let b = lines[i];
                match b.first() {
                    Some(b' ') | None => {
                        ro = ro.saturating_sub(1);
                        rn = rn.saturating_sub(1);
                        // `diff.suppressBlankEmpty` prints empty context lines as "".
                        body.push(if b.is_empty() { b" ".to_vec() } else { b.to_vec() });
                    }
                    Some(b'-') => {
                        ro = ro.saturating_sub(1);
                        body.push(b.to_vec());
                    }
                    Some(b'+') => {
                        rn = rn.saturating_sub(1);
                        body.push(b.to_vec());
                    }
                    Some(b'\\') => body.push(b.to_vec()),
                    _ => break,
                }
                i += 1;
            }
            while i < lines.len() && lines[i].first() == Some(&b'\\') {
                body.push(lines[i].to_vec());
                i += 1;
            }
            // A gitlink's "content" (a submodule whose only change is inside it has
            // no `index … 160000` line).
            if body.iter().any(|b| b.starts_with(b"-Subproject commit ") || b.starts_with(b"+Subproject commit ")) {
                d.submodule = true;
            }
            d.hunks.push(ParsedHunk { old_start: os, old_lines: ol, new_start: ns, new_lines: nl, suffix, body });
            continue;
        } else if l.starts_with(b"--- ") && d.hunks.is_empty() {
            d.minus_line = Some(l.to_vec());
        } else if l.starts_with(b"+++ ") && d.hunks.is_empty() {
            d.plus_line = Some(l.to_vec());
        } else if l.starts_with(b"new file mode ") {
            d.new_file = true;
            d.new_mode = Some(String::from_utf8_lossy(&l[14..]).into_owned());
            d.submodule |= l.ends_with(b" 160000");
        } else if l.starts_with(b"deleted file mode ") {
            d.deleted_file = true;
            d.submodule |= l.ends_with(b" 160000");
        } else if l.starts_with(b"old mode ") {
            d.old_mode = Some(String::from_utf8_lossy(&l[9..]).into_owned());
        } else if l.starts_with(b"new mode ") {
            d.new_mode = Some(String::from_utf8_lossy(&l[9..]).into_owned());
        } else if l.starts_with(b"rename from ") || l.starts_with(b"rename to ") {
            d.renamed = true;
        } else if l.starts_with(b"copy from ") || l.starts_with(b"copy to ") {
            d.copied = true;
        } else if l.starts_with(b"Binary files ") || l.starts_with(b"GIT binary patch") {
            d.binary = true;
        } else if l.starts_with(b"-Subproject commit ") || l.starts_with(b"+Subproject commit ") {
            d.submodule = true;
        } else if l.starts_with(b"index ") && l.ends_with(b" 160000") {
            d.submodule = true;
        }
        if (l.starts_with(b"index ") || l.starts_with(b"new file mode ") || l.starts_with(b"deleted file mode ") || l.starts_with(b"old mode ") || l.starts_with(b"new mode "))
            && d.hunks.is_empty()
            && l.ends_with(b" 120000")
        {
            d.symlink = true;
        }
        i += 1;
    }
    d
}

impl ParsedDiff {
    /// Plain content change that `git apply` can take hunk by hunk.
    pub fn hunk_staging_possible(&self) -> bool {
        !self.binary
            && !self.new_file
            && !self.deleted_file
            && !self.renamed
            && !self.copied
            && !self.submodule
            && !self.hunks.is_empty()
            && self.diff_line.is_some()
            && self.minus_line.is_some()
            && self.plus_line.is_some()
    }

    pub fn mode_change(&self) -> Option<String> {
        match (&self.old_mode, &self.new_mode) {
            (Some(a), Some(b)) if !self.new_file && a != b => Some(format!("{a} → {b}")),
            _ => None,
        }
    }
}

/// Build a patch with only `selected` hunks (indexes into `d.hunks`).
///
/// `reverse` = the patch will be applied with `git apply -R` (unstage, discard):
/// the new side is what is on disk, so new positions are kept and old positions
/// shifted. Forward application (stage) keeps old positions and shifts new ones.
/// The shift is the net line delta of the *unselected* hunks before each one, so
/// `git apply` starts its search at the exact line. Mode lines are dropped: hunk
/// operations change content only.
pub fn build_patch(d: &ParsedDiff, selected: &[usize], reverse: bool) -> Result<Vec<u8>, ApiError> {
    let (Some(dl), Some(ml), Some(pl)) = (&d.diff_line, &d.minus_line, &d.plus_line) else {
        return Err(ApiError::bad_request("this diff has no hunks that can be applied separately"));
    };
    let mut sel: Vec<usize> = selected.to_vec();
    sel.sort_unstable();
    sel.dedup();
    if sel.is_empty() {
        return Err(ApiError::bad_request("no hunks selected"));
    }
    if let Some(bad) = sel.iter().find(|i| **i >= d.hunks.len()) {
        return Err(ApiError::bad_request(format!("hunk {bad} does not exist (the diff has {})", d.hunks.len())));
    }
    let mut out = Vec::new();
    for l in [dl, ml, pl] {
        out.extend_from_slice(l);
        out.push(b'\n');
    }
    let mut unselected_delta: i64 = 0;
    for (i, h) in d.hunks.iter().enumerate() {
        if !sel.contains(&i) {
            unselected_delta += h.new_lines as i64 - h.old_lines as i64;
            continue;
        }
        let (os, ns) = if reverse {
            ((h.old_start as i64 + unselected_delta).max(0), h.new_start as i64)
        } else {
            (h.old_start as i64, (h.new_start as i64 - unselected_delta).max(0))
        };
        out.extend_from_slice(format!("@@ -{},{} +{},{} @@", os, h.old_lines, ns, h.new_lines).as_bytes());
        out.extend_from_slice(&h.suffix);
        out.push(b'\n');
        for b in &h.body {
            out.extend_from_slice(b);
            out.push(b'\n');
        }
    }
    Ok(out)
}

/// Stable id of a diff as shown: any change to either side changes it.
pub fn fingerprint(mode: DiffMode, path: &str, diff: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(format!("{mode:?}\0{path}\0").as_bytes());
    h.update(diff);
    hex::encode(&h.finalize()[..12])
}

// ---------------------------------------------------------------- contents

/// One side of a diff.
#[derive(Debug, Clone, Default)]
pub struct Side {
    pub text: String,
    pub missing: bool,
    pub binary: bool,
    pub too_large: bool,
    pub lfs: bool,
}

fn side_from_bytes(bytes: Vec<u8>) -> Side {
    let head = &bytes[..bytes.len().min(8000)];
    let lfs = bytes.starts_with(LFS_POINTER);
    if head.contains(&0) {
        return Side { binary: true, ..Default::default() };
    }
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    Side { text, lfs, ..Default::default() }
}

/// A blob by revision spec (`HEAD:path`, `:0:path`, `sha:path`).
pub async fn blob_side(repo: &Repo, spec: &str) -> Result<Side, ApiError> {
    let size = repo.git().args(["cat-file", "-s", spec]).run().await?;
    if !size.ok() {
        return Ok(Side { missing: true, ..Default::default() });
    }
    let n: u64 = size.text().trim().parse().unwrap_or(0);
    if n > MAX_TEXT_BYTES {
        return Ok(Side { too_large: true, ..Default::default() });
    }
    let out = repo.git().args(["cat-file", "blob", spec]).run().await?;
    if !out.ok() {
        // e.g. a submodule gitlink: not a blob.
        return Ok(Side { binary: true, ..Default::default() });
    }
    Ok(side_from_bytes(out.stdout))
}

/// The working-tree file (symlinks read as their target text, like git stores them).
pub async fn worktree_side(repo: &Repo, repo_rel: &str) -> Result<Side, ApiError> {
    let abs = repo.abs(repo_rel);
    // The parent must stay inside the working tree (no symlinked-directory escapes).
    if let Some(parent) = std::path::Path::new(repo_rel).parent() {
        let parent = parent.to_string_lossy();
        if !parent.is_empty() {
            crate::util::paths::resolve_in_root(&repo.top, &parent)?;
        }
    }
    tokio::task::spawn_blocking(move || {
        let meta = match std::fs::symlink_metadata(&abs) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Side { missing: true, ..Default::default() }),
            Err(e) => return Err(ApiError::from(e)),
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&abs)?;
            return Ok(Side { text: target.to_string_lossy().into_owned(), ..Default::default() });
        }
        if meta.is_dir() {
            // A submodule or nested repository.
            return Ok(Side { binary: true, ..Default::default() });
        }
        if meta.len() > MAX_TEXT_BYTES {
            return Ok(Side { too_large: true, ..Default::default() });
        }
        Ok(side_from_bytes(std::fs::read(&abs)?))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

// ---------------------------------------------------------------- computing diffs

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffQuery {
    pub path: String,
    pub mode: Option<DiffMode>,
    pub sha: Option<String>,
    pub base: Option<String>,
    pub head: Option<String>,
    /// Rename source when known (status `origPath`, commit file list).
    pub old_path: Option<String>,
    /// Lines of context for hunks (default 3). Hunk operations always use 3.
    pub context: Option<u32>,
}

fn diff_base_args(context: u32) -> Vec<String> {
    vec![
        "diff".into(),
        "--no-color".into(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
        "--src-prefix=a/".into(),
        "--dst-prefix=b/".into(),
        format!("-U{context}"),
    ]
}

/// The empty tree in this repository's hash algorithm.
pub async fn empty_tree(repo: &Repo) -> Result<String, ApiError> {
    let out = repo.git().args(["hash-object", "-t", "tree", "--stdin"]).stdin(Vec::new()).run_ok().await?;
    Ok(out.text().trim().to_string())
}

/// First parent of `sha`, or the empty tree for a root commit (`is_root = true`).
pub async fn parent_or_empty(repo: &Repo, sha: &str) -> Result<(String, bool), ApiError> {
    let out = repo.git().args(["rev-parse", "--verify", "--quiet", &format!("{sha}^1^{{commit}}")]).run().await?;
    if out.ok() {
        let p = out.text().trim().to_string();
        if !p.is_empty() {
            return Ok((p, false));
        }
    }
    Ok((empty_tree(repo).await?, true))
}

pub async fn resolve_commit(repo: &Repo, rev: &str) -> Result<String, ApiError> {
    let rev = check_rev(rev)?;
    let out = repo.git().args(["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{rev}^{{commit}}")]).run().await?;
    let s = out.text().trim().to_string();
    if !out.ok() || s.is_empty() {
        return Err(ApiError::not_found(format!("unknown revision {rev:?}")));
    }
    Ok(s)
}

/// Find the rename source of `path` between two trees (commit/compare modes).
async fn detect_old_path(repo: &Repo, a: &str, b: Option<&str>, path: &str) -> Result<Option<String>, ApiError> {
    let mut g = repo.git().args(["diff", "--no-color", "--no-ext-diff", "--name-status", "-z", "-M", "--end-of-options", a]);
    if let Some(b) = b {
        g = g.arg(b);
    }
    let out = g.run().await?;
    if !out.ok() {
        return Ok(None);
    }
    // `-z --name-status`: "R100\0old\0new\0" for renames/copies, "M\0path\0" otherwise.
    let recs = split_z(&out.stdout);
    let mut i = 0;
    while i < recs.len() {
        let st = &recs[i];
        if st.starts_with('R') || st.starts_with('C') {
            if recs.get(i + 2).is_some_and(|n| n == path) {
                return Ok(recs.get(i + 1).cloned());
            }
            i += 3;
        } else {
            i += 2;
        }
    }
    Ok(None)
}

/// Status entry for one path (working/staged modes need to know about untracked,
/// conflicted, submodule and renamed files). `pair_renames`: find the source of a
/// staged rename although only its new name is asked for (git pairs renames only
/// inside the pathspec, so this looks at the whole repository).
async fn path_status(repo: &Repo, repo_path: &str, pair_renames: bool) -> Result<Option<StatusFile>, ApiError> {
    let paths = [repo_path.to_string()];
    let narrow = narrow_entries(repo, &paths).await?.into_iter().find(|f| f.path == repo_path);
    // Only a path that looks added on its own can be the new side of a staged
    // rename; everything else is settled without the whole-repository query.
    if pair_renames && narrow.as_ref().is_some_and(|f| f.index == 'A') {
        return Ok(entries_for(repo, &paths).await?.into_iter().find(|f| f.path == repo_path));
    }
    Ok(narrow)
}

/// `git diff --submodule=log` for a submodule, plus what that leaves out.
async fn submodule_summary(repo: &Repo, range: &[&str], path: &str, st: Option<&StatusFile>) -> Result<String, ApiError> {
    let out = repo
        .git()
        .args(["diff", "--no-color", "--no-ext-diff", "--submodule=log"])
        .args(range.iter().copied())
        .args(["--".to_string(), literal(path)])
        .run()
        .await?;
    let mut s = out.text().trim_end().to_string();
    // `diff` does not report untracked files inside a submodule; status does.
    if st.is_some_and(|f| f.sub_untracked) && !s.contains("untracked content") {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(&format!("Submodule {path} contains untracked content"));
    }
    if s.is_empty() {
        s = format!("Submodule {path}: no change the superproject records");
    }
    Ok(s)
}

/// Compute the diff of one file. `path`/`old_path` are project-relative.
pub async fn file_diff(repo: &Repo, q: &DiffQuery) -> Result<(GitFileDiff, ParsedDiff), ApiError> {
    let mode = q.mode.unwrap_or(DiffMode::Working);
    let path = repo.to_repo(&q.path)?;
    let mut old_path = match q.old_path.as_deref().filter(|s| !s.is_empty()) {
        Some(o) => Some(repo.to_repo(o)?),
        None => None,
    };
    let context = q.context.unwrap_or(3).min(1000);
    let mut untracked = false;
    let mut conflict = false;
    let (orig_label, mod_label);
    let lit_path = literal(&path);
    // Working/staged: the status entry (untracked, conflicted, submodule…).
    let mut entry: Option<StatusFile> = None;
    // The two sides as `git diff` arguments (for a submodule summary).
    let mut range: Vec<String> = vec![];

    let (original, modified, diff_bytes): (Side, Side, Vec<u8>) = match mode {
        DiffMode::Working => {
            let st = path_status(repo, &path, false).await?;
            untracked = st.as_ref().is_some_and(|f| f.index == '?');
            conflict = st.as_ref().is_some_and(|f| f.conflict);
            entry = st;
            let modified = worktree_side(repo, &path).await?;
            if untracked {
                orig_label = "(new file)".to_string();
                mod_label = "Working tree".to_string();
                let out = repo
                    .git()
                    .args(diff_base_args(context))
                    .args(["--no-index", "--", "/dev/null", &path])
                    .run()
                    .await?;
                (Side { missing: true, ..Default::default() }, modified, out.stdout)
            } else if conflict {
                orig_label = "HEAD".to_string();
                mod_label = "Working tree (conflicted)".to_string();
                (blob_side(repo, &format!("HEAD:{path}")).await?, modified, Vec::new())
            } else {
                orig_label = "Index".to_string();
                mod_label = "Working tree".to_string();
                let original = blob_side(repo, &format!(":0:{path}")).await?;
                let out = repo.git().args(diff_base_args(context)).args(["--no-renames", "--", &lit_path]).run_ok().await?;
                (original, modified, out.stdout)
            }
        }
        DiffMode::Staged => {
            let st = path_status(repo, &path, old_path.is_none()).await?;
            if old_path.is_none() {
                old_path = st.as_ref().and_then(|f| if f.index == 'R' || f.index == 'C' { f.orig_path.clone() } else { None });
            }
            entry = st;
            range = vec!["--cached".into()];
            let has_head = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?.ok();
            orig_label = "HEAD".to_string();
            mod_label = "Index".to_string();
            let original = if has_head {
                blob_side(repo, &format!("HEAD:{}", old_path.as_deref().unwrap_or(&path))).await?
            } else {
                Side { missing: true, ..Default::default() }
            };
            let modified = blob_side(repo, &format!(":0:{path}")).await?;
            let mut g = repo.git().args(diff_base_args(context)).args(["--cached", "-M", "--", &lit_path]);
            if let Some(o) = &old_path {
                g = g.arg(literal(o));
            }
            let out = g.run_ok().await?;
            (original, modified, out.stdout)
        }
        DiffMode::Commit => {
            let sha = resolve_commit(repo, q.sha.as_deref().ok_or_else(|| ApiError::bad_request("mode=commit needs sha"))?).await?;
            let (parent, is_root) = parent_or_empty(repo, &sha).await?;
            if old_path.is_none() {
                old_path = detect_old_path(repo, &parent, Some(&sha), &path).await?;
            }
            orig_label = if is_root { "(no parent)".to_string() } else { parent[..10.min(parent.len())].to_string() };
            mod_label = sha[..10.min(sha.len())].to_string();
            let original = blob_side(repo, &format!("{parent}:{}", old_path.as_deref().unwrap_or(&path))).await?;
            let modified = blob_side(repo, &format!("{sha}:{path}")).await?;
            let mut g = repo.git().args(diff_base_args(context)).args(["-M", "--end-of-options", &parent, &sha, "--", &lit_path]);
            if let Some(o) = &old_path {
                g = g.arg(literal(o));
            }
            let out = g.run_ok().await?;
            range = vec![parent, sha];
            (original, modified, out.stdout)
        }
        DiffMode::Compare => {
            let base = resolve_commit(repo, q.base.as_deref().ok_or_else(|| ApiError::bad_request("mode=compare needs base"))?).await?;
            let head = match q.head.as_deref().filter(|h| !h.is_empty()) {
                Some(h) => Some(resolve_commit(repo, h).await?),
                None => None,
            };
            if old_path.is_none() {
                old_path = detect_old_path(repo, &base, head.as_deref(), &path).await?;
            }
            orig_label = q.base.clone().unwrap_or_default();
            mod_label = q.head.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| "Working tree".into());
            let original = blob_side(repo, &format!("{base}:{}", old_path.as_deref().unwrap_or(&path))).await?;
            let modified = match &head {
                Some(h) => blob_side(repo, &format!("{h}:{path}")).await?,
                None => worktree_side(repo, &path).await?,
            };
            let mut g = repo.git().args(diff_base_args(context)).args(["-M", "--end-of-options", &base]);
            if let Some(h) = &head {
                g = g.arg(h);
            }
            g = g.args(["--", &lit_path]);
            if let Some(o) = &old_path {
                g = g.arg(literal(o));
            }
            let out = g.run_ok().await?;
            range = std::iter::once(base).chain(head).collect();
            (original, modified, out.stdout)
        }
    };

    let parsed = parse_diff(&diff_bytes);
    let submodule = parsed.submodule || entry.as_ref().is_some_and(|f| f.submodule);
    if submodule {
        // A gitlink has no file content: describe the change instead of showing a
        // "binary, new file" diff (the gitlink's commit is not in this repository).
        let range: Vec<&str> = range.iter().map(String::as_str).collect();
        let summary = submodule_summary(repo, &range, &path, entry.as_ref()).await?;
        let diff = GitFileDiff {
            path: q.path.clone(),
            old_path: None,
            original: String::new(),
            modified: String::new(),
            binary: false,
            too_large: false,
            hunks: vec![],
            fingerprint: fingerprint(mode, &path, &diff_bytes),
            mode,
            can_stage_hunks: false,
            lfs: false,
            untracked,
            conflict,
            original_missing: parsed.new_file,
            modified_missing: parsed.deleted_file,
            mode_change: None,
            original_label: orig_label,
            modified_label: mod_label,
            submodule: true,
            submodule_summary: Some(summary),
            submodule_new_commit: mode == DiffMode::Working && entry.as_ref().is_some_and(|f| f.sub_commit),
            can_select_lines: false,
            lines: vec![],
        };
        return Ok((diff, parsed));
    }
    let binary = original.binary || modified.binary || parsed.binary;
    let too_large = !binary && (original.too_large || modified.too_large);
    let lfs = original.lfs || modified.lfs;
    let can_stage_hunks =
        matches!(mode, DiffMode::Working | DiffMode::Staged) && !untracked && !conflict && !too_large && !lfs && parsed.hunk_staging_possible();
    let show_text = !binary && !too_large;
    let lines = if show_text && !lfs && !conflict && !parsed.symlink && !parsed.submodule && !parsed.hunks.is_empty() {
        changed_lines(&parsed)
    } else {
        vec![]
    };
    let diff = GitFileDiff {
        path: q.path.clone(),
        old_path: old_path.as_deref().map(|o| repo.to_project(o)),
        original: if show_text { original.text } else { String::new() },
        modified: if show_text { modified.text } else { String::new() },
        binary,
        too_large,
        hunks: if binary { vec![] } else { parsed.hunks.iter().map(ParsedHunk::info).collect() },
        fingerprint: fingerprint(mode, &path, &diff_bytes),
        mode,
        can_stage_hunks,
        lfs,
        untracked,
        conflict,
        original_missing: original.missing,
        modified_missing: modified.missing,
        mode_change: parsed.mode_change(),
        original_label: orig_label,
        modified_label: mod_label,
        submodule: false,
        submodule_summary: None,
        submodule_new_commit: false,
        can_select_lines: !lines.is_empty(),
        lines,
    };
    Ok((diff, parsed))
}

// ---------------------------------------------------------------- hunk operations

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkOp {
    Stage,
    Unstage,
    Discard,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HunkRequest {
    pub path: String,
    pub hunk_indexes: Vec<usize>,
    pub fingerprint: String,
}

/// Apply `op` to the chosen hunks. The caller holds the repository write lock.
pub async fn apply_hunks(repo: &Repo, op: HunkOp, req: &HunkRequest) -> Result<(), ApiError> {
    let mode = match op {
        HunkOp::Stage | HunkOp::Discard => DiffMode::Working,
        HunkOp::Unstage => DiffMode::Staged,
    };
    let q = DiffQuery { path: req.path.clone(), mode: Some(mode), ..Default::default() };
    let (diff, parsed) = file_diff(repo, &q).await?;
    if diff.fingerprint != req.fingerprint {
        return Err(ApiError::conflict(format!(
            "{} changed since the diff was shown; review the new diff and try again",
            req.path
        )));
    }
    if !diff.can_stage_hunks {
        return Err(ApiError::bad_request(format!(
            "{} cannot be {} hunk by hunk (new, deleted, renamed, binary or conflicted file); use the file action instead",
            req.path,
            match op {
                HunkOp::Stage => "staged",
                HunkOp::Unstage => "unstaged",
                HunkOp::Discard => "rolled back",
            }
        )));
    }
    let patch = build_patch(&parsed, &req.hunk_indexes, op != HunkOp::Stage)?;
    let mut g = repo.git_w().args(["apply", "--recount", "--whitespace=nowarn"]);
    match op {
        HunkOp::Stage => g = g.arg("--cached"),
        HunkOp::Unstage => g = g.args(["--cached", "-R"]),
        HunkOp::Discard => g = g.arg("-R"),
    }
    g.arg("-").stdin(patch).run_ok_retry_lock().await.map_err(|e| {
        if e.status == StatusCode::UNPROCESSABLE_ENTITY {
            ApiError::conflict(format!("git apply refused the patch ({}); refresh the diff and try again", e.message))
        } else {
            e
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE_HUNKS: &str = "diff --git a/f.txt b/f.txt
index 1111111..2222222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,4 @@
-l1
+L1
 l2
 l3
 l4
@@ -10,3 +10,5 @@ fn ctx()
 l10
+new a
+new b
 l11
 l12
@@ -20,4 +22,3 @@
 l20
-l21
 l22
 l23
\\ No newline at end of file
";

    #[test]
    fn parses_hunks_headers_and_no_newline_markers() {
        let d = parse_diff(THREE_HUNKS.as_bytes());
        assert_eq!(d.hunks.len(), 3);
        assert!(d.hunk_staging_possible());
        let h = &d.hunks[1];
        assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (10, 3, 10, 5));
        assert_eq!(h.suffix, b" fn ctx()");
        assert_eq!(h.header(), "@@ -10,3 +10,5 @@ fn ctx()");
        assert_eq!(d.hunks[2].body.last().unwrap(), b"\\ No newline at end of file");
        assert_eq!(d.hunks[0].body.len(), 5);
    }

    #[test]
    fn single_line_ranges_default_to_one() {
        assert_eq!(parse_hunk_header(b"@@ -3 +3,2 @@"), Some((3, 1, 3, 2, vec![])));
        assert_eq!(parse_hunk_header(b"@@ -0,0 +1 @@ x"), Some((0, 0, 1, 1, b" x".to_vec())));
    }

    #[test]
    fn detects_binary_new_and_renamed_files() {
        let d = parse_diff(b"diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ\n");
        assert!(d.binary && !d.hunk_staging_possible());
        let d = parse_diff(b"diff --git a/n b/n\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+x\n");
        assert!(d.new_file && !d.hunk_staging_possible());
        let d = parse_diff(b"diff --git a/o b/n\nsimilarity index 90%\nrename from o\nrename to n\n--- a/o\n+++ b/n\n@@ -1 +1 @@\n-a\n+b\n");
        assert!(d.renamed && !d.hunk_staging_possible());
        let d = parse_diff(b"diff --git a/s b/s\nold mode 100644\nnew mode 100755\n--- a/s\n+++ b/s\n@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(d.mode_change().as_deref(), Some("100644 → 100755"));
        assert!(d.hunk_staging_possible());
    }

    #[test]
    fn forward_patch_shifts_new_positions_by_unselected_hunks() {
        let d = parse_diff(THREE_HUNKS.as_bytes());
        let p = String::from_utf8(build_patch(&d, &[2], false).unwrap()).unwrap();
        // Hunks 0 (+0) and 1 (+2) are skipped: the new start moves back by 2.
        assert!(p.starts_with("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -20,4 +20,3 @@\n"), "{p}");
        assert!(!p.contains("index "));
        assert!(p.ends_with("\\ No newline at end of file\n"));
        let p = String::from_utf8(build_patch(&d, &[0, 2], false).unwrap()).unwrap();
        assert!(p.contains("@@ -1,4 +1,4 @@\n") && p.contains("@@ -20,4 +20,3 @@\n"), "{p}");
    }

    #[test]
    fn reverse_patch_shifts_old_positions_by_unselected_hunks() {
        let d = parse_diff(THREE_HUNKS.as_bytes());
        let p = String::from_utf8(build_patch(&d, &[2], true).unwrap()).unwrap();
        assert!(p.contains("@@ -22,4 +22,3 @@\n"), "{p}");
        let p = String::from_utf8(build_patch(&d, &[1], true).unwrap()).unwrap();
        assert!(p.contains("@@ -10,3 +10,5 @@ fn ctx()\n"), "{p}");
        assert!(build_patch(&d, &[], true).is_err());
        assert!(build_patch(&d, &[7], true).is_err());
    }

    #[test]
    fn fingerprint_depends_on_mode_path_and_content() {
        let a = fingerprint(DiffMode::Working, "f", b"x");
        assert_eq!(a, fingerprint(DiffMode::Working, "f", b"x"));
        assert_ne!(a, fingerprint(DiffMode::Staged, "f", b"x"));
        assert_ne!(a, fingerprint(DiffMode::Working, "g", b"x"));
        assert_ne!(a, fingerprint(DiffMode::Working, "f", b"y"));
    }

    #[test]
    fn binary_and_lfs_detection() {
        assert!(side_from_bytes(vec![1, 0, 2]).binary);
        let s = side_from_bytes(b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 12\n".to_vec());
        assert!(s.lfs && !s.binary);
        assert_eq!(side_from_bytes(b"hi\n".to_vec()).text, "hi\n");
    }
}

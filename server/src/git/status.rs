//! `GET …/git/status`: `git status --porcelain=v2 --branch -z` plus the repository
//! state (merging, rebasing…) read from the git dir. Shape: `GitStatus` in
//! docs/ARCHITECTURE.md (cross-slice contract; fields may be added, never renamed).

use std::path::Path;

use serde::Serialize;

use super::cmd::literal;
use super::repo::Repo;
use crate::error::ApiError;

/// Status entries beyond this are dropped (`truncated: true`), e.g. an unignored
/// `node_modules` with `--untracked-files=all`.
pub const MAX_STATUS_FILES: usize = 20_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orig_path: Option<String>,
    /// Staged side: ' ' M A D R C T U ? !
    pub index: char,
    /// Unstaged side ('?' = untracked).
    pub worktree: char,
    pub conflict: bool,
    /// Rename/copy similarity (0-100) for `R`/`C` entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    /// A submodule entry.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub submodule: bool,
    /// The repository of the project the file is in (its id). Only the whole-project status
    /// (`?repo=all`, `repos::project_status`) says; a single repository's status leaves it out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Submodule whose checked-out commit differs from the index (`SC..`): that
    /// part can be staged like a file.
    #[serde(skip)]
    pub sub_commit: bool,
    /// Submodule with modified (`S.M.`) or untracked (`S..U`) files inside it:
    /// those can only be committed or reset inside the submodule.
    #[serde(skip)]
    pub sub_modified: bool,
    #[serde(skip)]
    pub sub_untracked: bool,
    /// The file's blob in HEAD (`hH`; all zeros when HEAD lacks it; None for
    /// untracked, ignored and conflicted entries). Changelists use it to tell a change
    /// that comes back (stash pop, unshelve) from a new one after a commit.
    #[serde(skip)]
    pub head_blob: Option<String>,
}

impl StatusFile {
    fn plain(path: &str, code: char) -> Self {
        StatusFile {
            path: path.to_string(),
            orig_path: None,
            index: code,
            worktree: code,
            conflict: false,
            score: None,
            submodule: false,
            repo: None,
            sub_commit: false,
            sub_modified: false,
            sub_untracked: false,
            head_blob: None,
        }
    }

    /// A submodule whose only unstaged change is content inside it, which neither
    /// `git add` nor `git restore` in the superproject can touch.
    pub fn submodule_content_only(&self) -> bool {
        self.submodule && (self.sub_modified || self.sub_untracked) && !self.sub_commit
    }
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StateDetail {
    /// Branch being rebased (`feature`) or merged in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Rebase target (sha) or the commit being picked/reverted/merged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onto: Option<String>,
    /// Rebase progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    /// `git am` in progress (shown as rebasing).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub am: bool,
    /// An interactive rebase (`rebase -i`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub interactive: bool,
    /// The rebase stopped at an `edit` step: amend the commit (or add commits), then continue.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub edit: bool,
    /// The commit the rebase stopped at (edit or conflict).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// clean | merging | rebasing | cherry-picking | reverting | bisecting
    pub state: &'static str,
    pub stashes: u32,
    pub files: Vec<StatusFile>,
    // ---- additions beyond the contract
    /// The upstream branch no longer exists on the remote.
    pub upstream_gone: bool,
    pub state_detail: StateDetail,
    pub truncated: bool,
}

/// Parse `git status --porcelain=v2 --branch --show-stash -z` output.
/// Paths stay repository-relative; the caller maps them to the project.
pub fn parse_porcelain_v2(bytes: &[u8]) -> GitStatus {
    let mut st = GitStatus {
        branch: None,
        head: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        state: "clean",
        stashes: 0,
        files: vec![],
        upstream_gone: false,
        state_detail: StateDetail::default(),
        truncated: false,
    };
    let mut saw_ab = false;
    let records = super::cmd::split_z(bytes);
    let mut it = records.into_iter();
    while let Some(rec) = it.next() {
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" => st.head = (v != "(initial)").then(|| v.to_string()),
                "branch.head" => st.branch = (v != "(detached)").then(|| v.to_string()),
                "branch.upstream" => st.upstream = Some(v.to_string()),
                "branch.ab" => {
                    saw_ab = true;
                    for part in v.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                "stash" => st.stashes = v.trim().parse().unwrap_or(0),
                _ => {}
            }
            continue;
        }
        if st.files.len() >= MAX_STATUS_FILES {
            st.truncated = true;
            // Keep consuming so a rename's second record is not misread; nothing else to do.
            continue;
        }
        let kind = rec.as_bytes().first().copied().unwrap_or(b' ');
        match kind {
            b'1' => {
                // 1 XY sub mH mI mW hH hI path
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if f.len() == 9 {
                    st.files.push(entry(f[1], f[2], f[8], None, None, false, Some(f[6])));
                }
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let f: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = it.next();
                if f.len() == 10 {
                    let score = f[8].get(1..).and_then(|s| s.parse().ok());
                    st.files.push(entry(f[1], f[2], f[9], orig, score, false, Some(f[6])));
                }
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() == 11 {
                    st.files.push(entry(f[1], f[2], f[10], None, None, true, None));
                }
            }
            b'?' => {
                if let Some(p) = rec.get(2..) {
                    st.files.push(StatusFile::plain(p, '?'));
                }
            }
            b'!' => {
                if let Some(p) = rec.get(2..) {
                    st.files.push(StatusFile::plain(p, '!'));
                }
            }
            _ => {}
        }
    }
    // An upstream without an ahead/behind line is gone from the remote.
    st.upstream_gone = st.upstream.is_some() && !saw_ab;
    st
}

fn entry(xy: &str, sub: &str, path: &str, orig: Option<String>, score: Option<u8>, conflict: bool, head_blob: Option<&str>) -> StatusFile {
    let mut c = xy.chars();
    let map = |ch: Option<char>| match ch {
        Some('.') | None => ' ',
        Some(x) => x,
    };
    // `N...` for files; `S<c><m><u>` for submodules (commit changed, tracked
    // changes inside, untracked files inside).
    let sb = sub.as_bytes();
    let submodule = sb.first() == Some(&b'S');
    StatusFile {
        path: path.to_string(),
        orig_path: orig,
        index: map(c.next()),
        worktree: map(c.next()),
        conflict,
        score,
        submodule,
        repo: None,
        sub_commit: submodule && sb.get(1) == Some(&b'C'),
        sub_modified: submodule && sb.get(2) == Some(&b'M'),
        sub_untracked: submodule && sb.get(3) == Some(&b'U'),
        head_blob: head_blob.map(str::to_string),
    }
}

/// Merge/rebase/cherry-pick/revert/bisect state from marker files in the git dir.
pub fn detect_state(git_dir: &Path) -> (&'static str, StateDetail) {
    let read = |name: &str| std::fs::read_to_string(git_dir.join(name)).ok().map(|s| s.trim().to_string());
    let num = |name: &str| read(name).and_then(|s| s.parse::<u32>().ok());
    let short_branch = |s: String| s.strip_prefix("refs/heads/").map(str::to_string).unwrap_or(s);
    if git_dir.join("rebase-merge").is_dir() {
        return (
            "rebasing",
            StateDetail {
                branch: read("rebase-merge/head-name").map(short_branch),
                onto: read("rebase-merge/onto"),
                step: num("rebase-merge/msgnum"),
                total: num("rebase-merge/end"),
                interactive: git_dir.join("rebase-merge/interactive").exists(),
                edit: git_dir.join("rebase-merge/amend").exists(),
                stopped: read("rebase-merge/stopped-sha").filter(|s| !s.is_empty()),
                ..Default::default()
            },
        );
    }
    if git_dir.join("rebase-apply").is_dir() {
        let am = git_dir.join("rebase-apply/applying").exists();
        return (
            "rebasing",
            StateDetail {
                branch: read("rebase-apply/head-name").map(short_branch),
                onto: read("rebase-apply/onto"),
                step: num("rebase-apply/next"),
                total: num("rebase-apply/last"),
                am,
                ..Default::default()
            },
        );
    }
    if git_dir.join("MERGE_HEAD").exists() {
        let msg = read("MERGE_MSG").unwrap_or_default();
        return (
            "merging",
            StateDetail {
                branch: merge_source(&msg),
                onto: read("MERGE_HEAD").and_then(|s| s.lines().next().map(str::to_string)),
                ..Default::default()
            },
        );
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return ("cherry-picking", StateDetail { onto: read("CHERRY_PICK_HEAD"), ..Default::default() });
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return ("reverting", StateDetail { onto: read("REVERT_HEAD"), ..Default::default() });
    }
    if git_dir.join("BISECT_LOG").exists() {
        return ("bisecting", StateDetail::default());
    }
    ("clean", StateDetail::default())
}

/// `Merge branch 'feature' into main` → `feature`.
fn merge_source(msg: &str) -> Option<String> {
    let first = msg.lines().next()?;
    for kw in ["Merge branch '", "Merge remote-tracking branch '", "Merge tag '", "Merge commit '"] {
        if let Some(rest) = first.strip_prefix(kw) {
            return rest.split('\'').next().map(str::to_string);
        }
    }
    None
}

/// Status entries of exactly these repository paths (untracked files included),
/// without rename pairing: git pairs a rename only when both of its sides are
/// inside the pathspec. Enough for "is it untracked / conflicted / a submodule".
pub async fn narrow_entries(repo: &Repo, repo_paths: &[String]) -> Result<Vec<StatusFile>, ApiError> {
    let mut files = vec![];
    // Paths go through argv here (status has no --pathspec-from-file); chunk them.
    for chunk in repo_paths.chunks(1000) {
        let out = repo
            .git()
            .args(["status", "--porcelain=v2", "-z", "--untracked-files=all", "--find-renames", "--"])
            .args(chunk.iter().map(|p| literal(p)))
            .run_ok()
            .await?;
        files.extend(parse_porcelain_v2(&out.stdout).files);
    }
    Ok(files)
}

/// Status entries that concern these repository paths, with renames paired: a
/// staged `git mv old new` comes back as one `R new <- old` entry whether `new`,
/// `old` or both were asked for (so unstage/rollback can treat both sides).
/// Tracked changes come from a whole-repository status (no untracked scan, so it
/// stays cheap); untracked files from a query narrowed to the paths.
pub async fn entries_for(repo: &Repo, repo_paths: &[String]) -> Result<Vec<StatusFile>, ApiError> {
    let wanted: std::collections::HashSet<&str> = repo_paths.iter().map(String::as_str).collect();
    let out = repo
        .git()
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=no", "--find-renames"])
        .run_ok()
        .await?;
    let mut files: Vec<StatusFile> = parse_porcelain_v2(&out.stdout)
        .files
        .into_iter()
        .filter(|f| wanted.contains(f.path.as_str()) || f.orig_path.as_deref().is_some_and(|o| wanted.contains(o)))
        .collect();
    let covered: std::collections::HashSet<String> =
        files.iter().flat_map(|f| std::iter::once(f.path.clone()).chain(f.orig_path.clone())).collect();
    let rest: Vec<String> = repo_paths.iter().filter(|p| !covered.contains(p.as_str())).cloned().collect();
    if !rest.is_empty() {
        // Untracked files, and submodules whose only change is untracked files
        // inside them (not reported without an untracked scan).
        files.extend(narrow_entries(repo, &rest).await?.into_iter().filter(|f| wanted.contains(f.path.as_str())));
    }
    Ok(files)
}

/// Run status for a repository and map paths to the project. The root directories of
/// the project's other repositories inside this one (git lists a nested repository as an
/// untracked or ignored directory), and the untracked or ignored directories above them,
/// are left out: they are not changes of this repository.
pub async fn status(repo: &Repo, include_ignored: bool) -> Result<GitStatus, ApiError> {
    let mut g = repo.git().args([
        "status",
        "--porcelain=v2",
        "--branch",
        "--show-stash",
        "-z",
        "--untracked-files=all",
        "--find-renames",
    ]);
    if include_ignored {
        g = g.arg("--ignored=matching");
    }
    g = g.args(["--".to_string(), literal(&repo.scope())]);
    let out = g.run_ok().await?;
    let mut st = parse_porcelain_v2(&out.stdout);
    let (state, detail) = detect_state(&repo.git_dir);
    st.state = state;
    st.state_detail = detail;
    if !repo.prefix.is_empty() || !repo.base.is_empty() {
        for f in &mut st.files {
            f.path = repo.to_project(&f.path);
            if let Some(o) = &f.orig_path {
                f.orig_path = Some(repo.to_project(o));
            }
        }
    }
    if !repo.inner.is_empty() {
        // An untracked or ignored directory that is, or holds, another repository says
        // nothing about that repository's files (`.gitignore` often lists the clones).
        st.files.retain(|f| {
            let p = f.path.trim_end_matches('/');
            !(matches!(f.index, '?' | '!') && repo.inner.iter().any(|d| p == d || d.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))))
        });
    }
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_headers_renames_conflicts_and_untracked() {
        let raw = "# branch.oid 1111111111111111111111111111111111111111\0\
# branch.head main\0\
# branch.upstream origin/main\0\
# branch.ab +2 -1\0\
# stash 3\0\
1 .M N... 100644 100644 100644 aaa aaa src/a b.rs\0\
1 M. N... 100644 100644 100644 aaa bbb staged.rs\0\
2 R. N... 100644 100644 100644 aaa aaa R100 new name.txt\0old name.txt\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
u DU N... 100644 000000 100644 100644 a b c deleted-by-us.rs\0\
? dir/untracked.txt\0";
        let st = parse_porcelain_v2(raw.as_bytes());
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.head.as_deref(), Some("1111111111111111111111111111111111111111"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind, st.stashes), (2, 1, 3));
        assert!(!st.upstream_gone);
        assert_eq!(st.files.len(), 6);
        assert_eq!(st.files[0].path, "src/a b.rs");
        assert_eq!((st.files[0].index, st.files[0].worktree), (' ', 'M'));
        assert_eq!((st.files[1].index, st.files[1].worktree), ('M', ' '));
        let r = &st.files[2];
        assert_eq!(r.path, "new name.txt");
        assert_eq!(r.orig_path.as_deref(), Some("old name.txt"));
        assert_eq!((r.index, r.score), ('R', Some(100)));
        // The HEAD blob (hH) of tracked changes; none for conflicts and untracked files.
        assert_eq!((st.files[1].head_blob.as_deref(), r.head_blob.as_deref()), (Some("aaa"), Some("aaa")));
        assert_eq!((st.files[3].head_blob.as_deref(), st.files[5].head_blob.as_deref()), (None, None));
        assert!(st.files[3].conflict);
        assert_eq!((st.files[3].index, st.files[3].worktree), ('U', 'U'));
        assert_eq!((st.files[4].index, st.files[4].worktree), ('D', 'U'));
        assert_eq!((st.files[5].index, st.files[5].worktree, st.files[5].path.as_str()), ('?', '?', "dir/untracked.txt"));
        assert!(st.files.iter().all(|f| !f.submodule && !f.submodule_content_only()));
    }

    #[test]
    fn parses_submodule_change_kinds() {
        let raw = "1 .M S.M. 160000 160000 160000 aaa aaa dirty\0\
1 .M S..U 160000 160000 160000 aaa aaa untracked-inside\0\
1 .M SC.. 160000 160000 160000 aaa aaa moved\0\
1 .M SCM. 160000 160000 160000 aaa aaa moved-and-dirty\0";
        let st = parse_porcelain_v2(raw.as_bytes());
        let by = |p: &str| st.files.iter().find(|f| f.path == p).unwrap().clone();
        assert!(by("dirty").submodule_content_only());
        assert!(by("untracked-inside").submodule_content_only());
        assert!(by("moved").submodule && by("moved").sub_commit && !by("moved").submodule_content_only());
        assert!(!by("moved-and-dirty").submodule_content_only());
    }

    #[test]
    fn parses_initial_detached_and_gone_upstream() {
        let st = parse_porcelain_v2(b"# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(st.head, None);
        assert_eq!(st.branch.as_deref(), Some("main"));
        let st = parse_porcelain_v2(b"# branch.oid abc\0# branch.head (detached)\0");
        assert_eq!(st.branch, None);
        let st = parse_porcelain_v2(b"# branch.oid abc\0# branch.head f\0# branch.upstream origin/f\0");
        assert!(st.upstream_gone);
    }

    #[test]
    fn detects_repository_state() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(detect_state(d.path()).0, "clean");
        std::fs::write(d.path().join("MERGE_HEAD"), "abc\n").unwrap();
        std::fs::write(d.path().join("MERGE_MSG"), "Merge branch 'feature' into main\n").unwrap();
        let (s, det) = detect_state(d.path());
        assert_eq!(s, "merging");
        assert_eq!(det.branch.as_deref(), Some("feature"));
        std::fs::create_dir(d.path().join("rebase-merge")).unwrap();
        std::fs::write(d.path().join("rebase-merge/head-name"), "refs/heads/topic\n").unwrap();
        std::fs::write(d.path().join("rebase-merge/msgnum"), "2\n").unwrap();
        std::fs::write(d.path().join("rebase-merge/end"), "5\n").unwrap();
        let (s, det) = detect_state(d.path());
        assert_eq!(s, "rebasing");
        assert_eq!((det.branch.as_deref(), det.step, det.total), (Some("topic"), Some(2), Some(5)));
    }
}

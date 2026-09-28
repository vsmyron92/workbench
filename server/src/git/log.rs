//! History: the log (for the graph), commit details and ref comparison.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::cmd::{check_rev, literal, split_z};
use super::diff::parent_or_empty;
use super::repo::Repo;
use crate::error::ApiError;

pub const DEFAULT_LOG_LIMIT: usize = 500;
pub const MAX_LOG_LIMIT: usize = 5000;
const MAX_COMMIT_FILES: usize = 5000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub name: String,
    /// head (the checked-out branch) | branch | remote | tag | HEAD (detached)
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, Unix milliseconds.
    pub time: i64,
    /// Committer time, Unix milliseconds.
    pub commit_time: i64,
    pub subject: String,
    pub refs: Vec<RefLabel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub commits: Vec<LogCommit>,
    pub has_more: bool,
    /// Single-file history (`--follow`): parents are not rewritten to the file's
    /// history, so draw the commits as one chain instead of a graph.
    pub linear: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    /// A branch, tag or revision; default HEAD. Ignored when `all`.
    #[serde(rename = "ref")]
    pub rev: Option<String>,
    pub all: Option<bool>,
    /// Project-relative file or directory. A single file follows renames.
    pub path: Option<String>,
    pub author: Option<String>,
    /// Message text (case-insensitive, literal).
    pub grep: Option<String>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
    /// Only first parents (a branch's own history).
    pub first_parent: Option<bool>,
    /// History of a line range of `path` (`start,end`, 1-based, inclusive): the
    /// commits that touched those lines (`git log -L`, CLion's "Show History for
    /// Selection").
    pub lines: Option<String>,
    /// `lines` are line numbers of the working-tree file (an editor selection): they are
    /// mapped onto the logged revision's version of the file first (through the
    /// revision → working tree diff), and a file renamed since is followed to its old name.
    pub worktree_lines: Option<bool>,
}

/// Map a range of working-tree lines onto the old side of zero-context hunks
/// `(old_start, old_lines, new_start, new_lines)` (sorted, as `git diff -U0` prints them).
/// A line in a replaced block maps to the block's old lines, a line in a pure addition
/// to the nearest old lines around it; None when every selected line is new.
pub fn map_to_old(hunks: &[(u32, u32, u32, u32)], a: u32, b: u32) -> Option<(u32, u32)> {
    // `start`: the first old line at or after the new line; `end`: the last at or before.
    let map = |l: u32, start: bool| -> i64 {
        let mut delta: i64 = 0;
        for &(os, ol, ns, nl) in hunks {
            let (os, ol, ns, nl) = (os as i64, ol as i64, ns as i64, nl as i64);
            let l = l as i64;
            if nl == 0 {
                // A pure deletion after new line `ns`.
                if l <= ns {
                    return l + delta;
                }
            } else if l < ns {
                return l + delta;
            } else if l < ns + nl {
                return match (ol, start) {
                    (0, true) => os + 1,
                    (0, false) => os,
                    (_, true) => os,
                    (_, false) => os + ol - 1,
                };
            }
            delta += ol - nl;
        }
        l as i64 + delta
    };
    let (s, e) = (map(a, true), map(b, false));
    (s >= 1 && e >= s).then_some((s as u32, e as u32))
}

/// `-L` arguments for working-tree lines `a..=b` of project path `path`, logged from `rev`:
/// the range on `rev`'s version and the file's name there.
async fn worktree_range(repo: &Repo, rev: &str, path: &str, a: u32, b: u32) -> Result<(u32, u32, String), ApiError> {
    use super::diff::{DiffMode, DiffQuery, file_diff};
    let q = DiffQuery { path: path.to_string(), mode: Some(DiffMode::Compare), base: Some(rev.to_string()), context: Some(0), ..Default::default() };
    let (d, parsed) = file_diff(repo, &q).await?;
    if d.original_missing {
        return Err(ApiError::bad_request(format!("{path} is not in {rev} yet: its lines have no history")));
    }
    let hunks: Vec<(u32, u32, u32, u32)> = parsed.hunks.iter().map(|h| (h.old_start, h.old_lines, h.new_start, h.new_lines)).collect();
    let (s, e) = map_to_old(&hunks, a, b).ok_or_else(|| ApiError::bad_request("the selected lines are not committed yet: they have no history"))?;
    let name = match d.old_path.as_deref() {
        Some(o) => repo.to_repo(o)?,
        None => repo.to_repo(path)?,
    };
    Ok((s, e, name))
}

/// `"12,40"` → (12, 40).
pub fn parse_line_range(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once(',')?;
    let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a >= 1 && b >= a && b <= 10_000_000).then_some((a, b))
}

/// Separator between fields of one record (`-z` separates records).
const FS: char = '\u{1f}';
const LOG_FORMAT: &str = "%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%s";

/// Parse `git log -z --format=LOG_FORMAT` output.
pub fn parse_log(bytes: &[u8]) -> Vec<LogCommit> {
    split_z(bytes)
        .into_iter()
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.splitn(7, FS).collect();
            if f.len() < 7 || f[0].len() < 7 {
                return None;
            }
            Some(LogCommit {
                sha: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                time: f[4].trim().parse::<i64>().unwrap_or(0) * 1000,
                commit_time: f[5].trim().parse::<i64>().unwrap_or(0) * 1000,
                subject: f[6].to_string(),
                refs: vec![],
            })
        })
        .collect()
}

/// Map commit sha → ref labels, from `git for-each-ref`. The checked-out branch is
/// `head`; a detached HEAD adds a `HEAD` label.
pub async fn ref_labels(repo: &Repo) -> Result<HashMap<String, Vec<RefLabel>>, ApiError> {
    let out = repo
        .git()
        .args([
            "for-each-ref",
            "--format=%(objectname)%1f%(*objectname)%1f%(refname)%1f%(HEAD)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])
        .run_ok()
        .await?;
    let head = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?;
    let head_sha = head.ok().then(|| head.text().trim().to_string());
    let text = out.text();
    let mut map = parse_ref_lines(&text);
    let on_branch = text.lines().any(|l| l.ends_with("\u{1f}*"));
    if let (Some(h), false) = (head_sha, on_branch) {
        map.entry(h).or_default().insert(0, RefLabel { name: "HEAD".into(), kind: "HEAD" });
    }
    Ok(map)
}

pub fn parse_ref_lines(text: &str) -> HashMap<String, Vec<RefLabel>> {
    let mut map: HashMap<String, Vec<RefLabel>> = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(FS).collect();
        if f.len() < 4 {
            continue;
        }
        // Annotated tags point at the tag object; the commit is the peeled id.
        let sha = if f[1].is_empty() { f[0] } else { f[1] };
        let refname = f[2];
        let current = f[3] == "*";
        let label = if let Some(n) = refname.strip_prefix("refs/heads/") {
            RefLabel { name: n.to_string(), kind: if current { "head" } else { "branch" } }
        } else if let Some(n) = refname.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue; // symbolic remote HEAD duplicates its target
            }
            RefLabel { name: n.to_string(), kind: "remote" }
        } else if let Some(n) = refname.strip_prefix("refs/tags/") {
            RefLabel { name: n.to_string(), kind: "tag" }
        } else {
            continue;
        };
        map.entry(sha.to_string()).or_default().push(label);
    }
    let rank = |k: &str| match k {
        "HEAD" => 0,
        "head" => 1,
        "branch" => 2,
        "remote" => 3,
        _ => 4,
    };
    for v in map.values_mut() {
        v.sort_by(|a, b| rank(a.kind).cmp(&rank(b.kind)).then_with(|| a.name.cmp(&b.name)));
    }
    map
}

pub async fn log(repo: &Repo, q: &LogQuery) -> Result<LogPage, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LOG_LIMIT).clamp(1, MAX_LOG_LIMIT);
    let skip = q.skip.unwrap_or(0);
    let has_head = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?.ok();
    let all = q.all.unwrap_or(false);
    if !has_head && !all {
        return Ok(LogPage { commits: vec![], has_more: false, linear: false });
    }
    let mut g = repo.git().args([
        "log".to_string(),
        "-z".to_string(),
        format!("--format={LOG_FORMAT}"),
        format!("--max-count={}", limit + 1),
        format!("--skip={skip}"),
    ]);
    let path = match q.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Some(repo.to_repo(p)?),
        None => None,
    };
    let is_file = path.as_ref().is_some_and(|p| repo.abs(p).is_file());
    let range = match q.lines.as_deref().filter(|l| !l.is_empty()) {
        Some(l) => {
            let r = parse_line_range(l).ok_or_else(|| ApiError::bad_request("lines must be start,end (1-based)"))?;
            if !is_file {
                return Err(ApiError::bad_request("a line range needs a file path"));
            }
            Some(r)
        }
        None => None,
    };
    if let (Some((a, b)), Some(p)) = (range, &path) {
        let (a, b, p) = if q.worktree_lines.unwrap_or(false) && !all {
            let rev = q.rev.as_deref().filter(|r| !r.is_empty()).unwrap_or("HEAD");
            worktree_range(repo, check_rev(rev)?, q.path.as_deref().unwrap_or_default().trim(), a, b).await?
        } else {
            (a, b, p.clone())
        };
        // The commits that changed these lines (no patch output), newest first.
        g = g.args([format!("-L{a},{b}:{p}"), "--no-patch".into()]);
    } else if is_file {
        // Single-file history follows renames (flat list, like CLion's file history).
        g = g.args(["--follow", "--date-order"]);
    } else {
        g = g.arg("--topo-order");
        if path.is_some() {
            // Keep the graph connected when limiting to a directory.
            g = g.arg("--parents");
        }
    }
    if q.first_parent.unwrap_or(false) {
        g = g.arg("--first-parent");
    }
    if let Some(a) = q.author.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        g = g.args(["-i".to_string(), "-F".into(), format!("--author={a}")]);
    }
    if let Some(t) = q.grep.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        g = g.args(["-i".to_string(), "-F".into(), format!("--grep={t}")]);
    }
    if all {
        g = g.args(["--branches", "--remotes", "--tags"]);
        if has_head {
            g = g.arg("HEAD");
        }
    } else {
        let rev = q.rev.as_deref().filter(|r| !r.is_empty()).unwrap_or("HEAD");
        g = g.args(["--end-of-options", check_rev(rev)?]);
    }
    g = g.arg("--");
    if let (Some(p), None) = (&path, range) {
        g = g.arg(literal(p));
    }
    let out = g.run().await?;
    if !out.ok() {
        let msg = out.message();
        if msg.contains("unknown revision") || msg.contains("bad revision") {
            return Err(ApiError::not_found(msg));
        }
        return Err(super::cmd::git_error(&out));
    }
    let mut commits = parse_log(&out.stdout);
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    let labels = ref_labels(repo).await?;
    for c in &mut commits {
        if let Some(l) = labels.get(&c.sha) {
            c.refs = l.clone();
        }
    }
    Ok(LogPage { commits, has_more, linear: is_file })
}

#[cfg(test)]
mod range_tests {
    #[test]
    fn parses_line_ranges() {
        assert_eq!(super::parse_line_range("3,9"), Some((3, 9)));
        assert_eq!(super::parse_line_range(" 5 , 5 "), Some((5, 5)));
        assert_eq!(super::parse_line_range("0,4"), None);
        assert_eq!(super::parse_line_range("9,3"), None);
        assert_eq!(super::parse_line_range("x"), None);
    }

    #[test]
    fn maps_working_tree_lines_onto_the_old_version() {
        use super::map_to_old;
        // One line added after old line 4 (new 5), old line 10 replaced by new 11-12,
        // old lines 15-16 deleted (after new line 16).
        let h = [(4, 0, 5, 1), (10, 1, 11, 2), (15, 2, 16, 0)];
        assert_eq!(map_to_old(&h, 1, 4), Some((1, 4)), "above every change");
        assert_eq!(map_to_old(&h, 6, 9), Some((5, 8)), "below the insertion: shifted by one");
        assert_eq!(map_to_old(&h, 5, 5), None, "only the new line");
        assert_eq!(map_to_old(&h, 4, 6), Some((4, 5)), "a range around the new line");
        assert_eq!(map_to_old(&h, 12, 12), Some((10, 10)), "a replaced line maps to the old block");
        assert_eq!(map_to_old(&h, 13, 16), Some((11, 14)));
        assert_eq!(map_to_old(&h, 17, 20), Some((17, 20)), "after the deletion the net change is zero");
        assert_eq!(map_to_old(&[(2, 3, 1, 0)], 1, 4), Some((1, 7)), "lines 2-4 deleted after new line 1");
        assert_eq!(map_to_old(&[], 3, 7), Some((3, 7)));
    }
}

// ---------------------------------------------------------------- commit details

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// A M D R C T
    pub status: char,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub subject: String,
    /// The full message.
    pub message: String,
    pub refs: Vec<RefLabel>,
    /// Files changed relative to the first parent.
    pub files: Vec<ChangedFile>,
    pub files_truncated: bool,
}

/// Parse `--name-status -z` output: `S\0path\0` or `R100\0old\0new\0`.
pub fn parse_name_status(bytes: &[u8]) -> Vec<(char, String, Option<String>)> {
    let recs = split_z(bytes);
    let mut v = vec![];
    let mut i = 0;
    while i < recs.len() {
        let st = recs[i].chars().next().unwrap_or('M');
        if st == 'R' || st == 'C' {
            if let (Some(old), Some(new)) = (recs.get(i + 1), recs.get(i + 2)) {
                v.push((st, new.clone(), Some(old.clone())));
            }
            i += 3;
        } else {
            if let Some(p) = recs.get(i + 1) {
                v.push((st, p.clone(), None));
            }
            i += 2;
        }
    }
    v
}

/// Parse `--numstat -z` output into path → (additions, deletions, binary).
/// Renames are `a\td\t\0old\0new\0`.
pub fn parse_numstat(bytes: &[u8]) -> HashMap<String, (u32, u32, bool)> {
    let recs = split_z(bytes);
    let mut m = HashMap::new();
    let mut i = 0;
    while i < recs.len() {
        let rec = &recs[i];
        let mut parts = rec.splitn(3, '\t');
        let a = parts.next().unwrap_or("");
        let d = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        let binary = a == "-" && d == "-";
        let stats = (a.parse().unwrap_or(0), d.parse().unwrap_or(0), binary);
        if path.is_empty() {
            // rename: the next two records are old and new
            if let Some(new) = recs.get(i + 2) {
                m.insert(new.clone(), stats);
            }
            i += 3;
        } else {
            m.insert(path.to_string(), stats);
            i += 1;
        }
    }
    m
}

/// Files changed between two trees/commits (`a` → `b`, or `a` → working tree).
pub async fn changed_files(repo: &Repo, a: &str, b: Option<&str>) -> Result<(Vec<ChangedFile>, bool), ApiError> {
    let base = ["diff", "--no-color", "--no-ext-diff", "-M", "-z"];
    let mut ns = repo.git().args(base).arg("--name-status").arg("--end-of-options").arg(a);
    let mut num = repo.git().args(base).arg("--numstat").arg("--end-of-options").arg(a);
    if let Some(b) = b {
        ns = ns.arg(b);
        num = num.arg(b);
    }
    let scope = literal(&repo.scope());
    ns = ns.args(["--", &scope]);
    num = num.args(["--", &scope]);
    let (ns, num) = tokio::join!(ns.run_ok(), num.run_ok());
    let (ns, num) = (ns?, num?);
    let stats = parse_numstat(&num.stdout);
    let mut files: Vec<ChangedFile> = parse_name_status(&ns.stdout)
        .into_iter()
        .map(|(status, path, old)| {
            let (additions, deletions, binary) = stats.get(&path).copied().unwrap_or((0, 0, false));
            ChangedFile {
                path: repo.to_project(&path),
                old_path: old.map(|o| repo.to_project(&o)),
                status,
                additions,
                deletions,
                binary,
            }
        })
        .collect();
    let truncated = files.len() > MAX_COMMIT_FILES;
    files.truncate(MAX_COMMIT_FILES);
    Ok((files, truncated))
}

pub async fn commit_details(repo: &Repo, rev: &str) -> Result<CommitDetails, ApiError> {
    let sha = super::diff::resolve_commit(repo, rev).await?;
    let out = repo
        .git()
        .args(["show", "-s", "-z", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s%x1f%B", &sha])
        .run_ok()
        .await?;
    let text = out.text();
    let rec = text.trim_end_matches('\0');
    let f: Vec<&str> = rec.splitn(10, FS).collect();
    if f.len() < 10 {
        return Err(ApiError::internal("unexpected git show output"));
    }
    let (parent, _) = parent_or_empty(repo, &sha).await?;
    let (files, files_truncated) = changed_files(repo, &parent, Some(&sha)).await?;
    let labels = ref_labels(repo).await?;
    Ok(CommitDetails {
        sha: f[0].to_string(),
        parents: f[1].split_whitespace().map(str::to_string).collect(),
        author: f[2].to_string(),
        email: f[3].to_string(),
        time: f[4].parse::<i64>().unwrap_or(0) * 1000,
        committer: f[5].to_string(),
        committer_email: f[6].to_string(),
        commit_time: f[7].parse::<i64>().unwrap_or(0) * 1000,
        subject: f[8].to_string(),
        message: f[9].trim_end().to_string(),
        refs: labels.get(&sha).cloned().unwrap_or_default(),
        files,
        files_truncated,
    })
}

// ---------------------------------------------------------------- compare

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub base: String,
    pub head: String,
    /// Commits in `head` that are not in `base` (`base..head`).
    pub head_only: Vec<LogCommit>,
    /// Commits in `base` that are not in `head` (`head..base`).
    pub base_only: Vec<LogCommit>,
    /// Files changed on `head` since the merge base (`base...head`).
    pub files: Vec<ChangedFile>,
    pub merge_base: Option<String>,
}

async fn range_log(repo: &Repo, range: &str) -> Result<Vec<LogCommit>, ApiError> {
    let out = repo
        .git()
        .args(["log".to_string(), "-z".into(), format!("--format={LOG_FORMAT}"), "--topo-order".into(), "--max-count=1000".into()])
        .args(["--end-of-options", range, "--"])
        .run_ok()
        .await?;
    Ok(parse_log(&out.stdout))
}

pub async fn compare(repo: &Repo, base: &str, head: &str) -> Result<Comparison, ApiError> {
    let b = super::diff::resolve_commit(repo, base).await?;
    let h = super::diff::resolve_commit(repo, head).await?;
    let (fwd, back) = (format!("{b}..{h}"), format!("{h}..{b}"));
    let (head_only, base_only) = tokio::join!(range_log(repo, &fwd), range_log(repo, &back));
    let mb = repo.git().args(["merge-base", &b, &h]).run().await?;
    let merge_base = mb.ok().then(|| mb.text().trim().to_string()).filter(|s| !s.is_empty());
    let files = match &merge_base {
        Some(m) => changed_files(repo, m, Some(&h)).await?.0,
        None => changed_files(repo, &b, Some(&h)).await?.0,
    };
    Ok(Comparison { base: base.to_string(), head: head.to_string(), head_only: head_only?, base_only: base_only?, files, merge_base })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_records() {
        let raw = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb cccccccccccccccccccccccccccccccccccccccc\u{1f}Ann\u{1f}ann@x\u{1f}1700000000\u{1f}1700000100\u{1f}Merge branch 'x'\0\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\u{1f}\u{1f}Bob\u{1f}bob@x\u{1f}1600000000\u{1f}1600000000\u{1f}root: with \u{2014} unicode\0";
        let v = parse_log(raw.as_bytes());
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].parents.len(), 2);
        assert_eq!(v[0].time, 1_700_000_000_000);
        assert_eq!(v[0].commit_time, 1_700_000_100_000);
        assert_eq!(v[1].parents.len(), 0);
        assert_eq!(v[1].subject, "root: with \u{2014} unicode");
    }

    #[test]
    fn parses_refs_with_kinds_and_peeled_tags() {
        let t = "1111\u{1f}\u{1f}refs/heads/main\u{1f}*\n\
1111\u{1f}\u{1f}refs/remotes/origin/main\u{1f} \n\
1111\u{1f}\u{1f}refs/remotes/origin/HEAD\u{1f} \n\
2222\u{1f}\u{1f}refs/heads/feature/x\u{1f} \n\
9999\u{1f}3333\u{1f}refs/tags/v1.0\u{1f} \n";
        let m = parse_ref_lines(t);
        assert_eq!(
            m["1111"],
            vec![RefLabel { name: "main".into(), kind: "head" }, RefLabel { name: "origin/main".into(), kind: "remote" }]
        );
        assert_eq!(m["2222"], vec![RefLabel { name: "feature/x".into(), kind: "branch" }]);
        assert_eq!(m["3333"], vec![RefLabel { name: "v1.0".into(), kind: "tag" }]);
        assert!(!m.contains_key("9999"));
    }

    #[test]
    fn parses_name_status_and_numstat_with_renames() {
        let ns = parse_name_status(b"M\0a.txt\0R090\0old.txt\0new.txt\0A\0b c.txt\0");
        assert_eq!(ns[0], ('M', "a.txt".into(), None));
        assert_eq!(ns[1], ('R', "new.txt".into(), Some("old.txt".into())));
        assert_eq!(ns[2], ('A', "b c.txt".into(), None));
        let st = parse_numstat(b"3\t1\ta.txt\0" as &[u8]);
        assert_eq!(st["a.txt"], (3, 1, false));
        let st = parse_numstat(b"1\t1\t\0old.txt\0new.txt\0-\t-\timg.png\0");
        assert_eq!(st["new.txt"], (1, 1, false));
        assert_eq!(st["img.png"], (0, 0, true));
    }
}

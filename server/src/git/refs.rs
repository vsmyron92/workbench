//! Branches, tags, remotes and worktrees (read side).

use serde::Serialize;

use super::cmd::split_z;
use super::repo::Repo;
use crate::error::ApiError;

const MAX_TAGS: usize = 2000;
const MAX_REMOTE_BRANCHES: usize = 5000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalBranch {
    pub name: String,
    pub sha: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// The upstream was deleted on the remote.
    pub gone: bool,
    pub subject: String,
    /// Tip commit time, Unix ms.
    pub time: i64,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBranch {
    /// `origin/feature`
    pub name: String,
    pub remote: String,
    /// `feature`
    pub branch: String,
    pub sha: String,
    pub subject: String,
    pub time: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    /// The commit (peeled).
    pub sha: String,
    pub annotated: bool,
    pub subject: String,
    pub time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branches {
    pub current: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub local: Vec<LocalBranch>,
    pub remote: Vec<RemoteBranch>,
    pub tags: Vec<TagInfo>,
    /// Recently checked-out local branches (from the reflog), most recent first.
    pub recent: Vec<String>,
    pub remotes: Vec<String>,
    pub tags_truncated: bool,
}

/// `ahead 2, behind 1` / `gone` / `` (from `%(upstream:track,nobracket)`).
pub fn parse_track(s: &str) -> (u32, u32, bool) {
    let s = s.trim();
    if s == "gone" {
        return (0, 0, true);
    }
    let (mut a, mut b) = (0, 0);
    for part in s.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            a = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            b = n.parse().unwrap_or(0);
        }
    }
    (a, b, false)
}

const FS: char = '\u{1f}';

/// Parse the `for-each-ref` lines produced by `branches()`.
pub fn parse_refs(text: &str) -> (Vec<LocalBranch>, Vec<RemoteBranch>, Vec<TagInfo>) {
    let (mut local, mut remote, mut tags) = (vec![], vec![], vec![]);
    for line in text.lines() {
        let f: Vec<&str> = line.split(FS).collect();
        if f.len() < 8 {
            continue;
        }
        let (refname, sha, peeled, upstream, track, subject, time, head) = (f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
        let time = time.trim().parse::<i64>().unwrap_or(0) * 1000;
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            let (ahead, behind, gone) = parse_track(track);
            local.push(LocalBranch {
                name: name.to_string(),
                sha: sha.to_string(),
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                ahead,
                behind,
                gone,
                subject: subject.to_string(),
                time,
                current: head == "*",
            });
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            let (r, b) = name.split_once('/').unwrap_or((name, ""));
            remote.push(RemoteBranch {
                name: name.to_string(),
                remote: r.to_string(),
                branch: b.to_string(),
                sha: sha.to_string(),
                subject: subject.to_string(),
                time,
            });
        } else if let Some(name) = refname.strip_prefix("refs/tags/") {
            tags.push(TagInfo {
                name: name.to_string(),
                sha: if peeled.is_empty() { sha.to_string() } else { peeled.to_string() },
                annotated: !peeled.is_empty(),
                subject: subject.to_string(),
                time,
            });
        }
    }
    (local, remote, tags)
}

/// Branch names from reflog subjects `checkout: moving from A to B`, most recent first.
pub fn parse_recent(reflog: &str, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in reflog.lines() {
        if let Some(rest) = line.strip_prefix("checkout: moving from ") {
            if let Some((_, to)) = rest.rsplit_once(" to ") {
                let to = to.trim().to_string();
                if !to.is_empty() && !out.contains(&to) {
                    out.push(to);
                }
            }
        }
        if out.len() >= limit * 3 {
            break;
        }
    }
    out
}

pub async fn branches(repo: &Repo) -> Result<Branches, ApiError> {
    let fmt = "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(contents:subject)%1f%(creatordate:unix)%1f%(HEAD)";
    let refs = repo.git().args(["for-each-ref", "--sort=-creatordate", fmt, "refs/heads", "refs/remotes", "refs/tags"]).run_ok();
    let reflog = repo.git().args(["reflog", "show", "--format=%gs", "-n", "300", "HEAD", "--"]).run();
    let head = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run();
    let current = repo.git().args(["symbolic-ref", "--quiet", "--short", "HEAD"]).run();
    let remotes = repo.git().arg("remote").run();
    let (refs, reflog, head, current, remotes) = tokio::join!(refs, reflog, head, current, remotes);
    let (local, mut remote, mut tags) = parse_refs(&refs?.text());
    let head = head?;
    let current = current?;
    let current = current.ok().then(|| current.text().trim().to_string()).filter(|s| !s.is_empty());
    let recent: Vec<String> = match reflog {
        Ok(o) if o.ok() => parse_recent(&o.text(), 10)
            .into_iter()
            .filter(|b| local.iter().any(|l| &l.name == b) && Some(b) != current.as_ref())
            .take(5)
            .collect(),
        _ => vec![],
    };
    let tags_truncated = tags.len() > MAX_TAGS;
    tags.truncate(MAX_TAGS);
    remote.truncate(MAX_REMOTE_BRANCHES);
    let remotes = remotes?.text().lines().map(str::to_string).filter(|s| !s.is_empty()).collect();
    Ok(Branches {
        detached: current.is_none() && head.ok(),
        head: head.ok().then(|| head.text().trim().to_string()),
        current,
        local,
        remote,
        tags,
        recent,
        remotes,
        tags_truncated,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub name: String,
    /// Credentials removed.
    pub fetch_url: String,
    pub push_url: String,
}

fn strip_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let end = rest.find('/').unwrap_or(rest.len());
    match rest[..end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// Parse `git remote -v`.
pub fn parse_remotes(text: &str) -> Vec<RemoteInfo> {
    let mut v: Vec<RemoteInfo> = vec![];
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let url = strip_userinfo(url);
        let idx = match v.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                v.push(RemoteInfo { name: name.into(), fetch_url: String::new(), push_url: String::new() });
                v.len() - 1
            }
        };
        if kind == "(fetch)" {
            v[idx].fetch_url = url;
        } else {
            v[idx].push_url = url;
        }
    }
    v
}

pub async fn remotes(repo: &Repo) -> Result<Vec<RemoteInfo>, ApiError> {
    let out = repo.git().args(["remote", "-v"]).run_ok().await?;
    Ok(parse_remotes(&out.text()))
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
    /// The worktree this project lives in.
    pub current: bool,
}

/// Parse `git worktree list --porcelain -z`.
pub fn parse_worktrees(bytes: &[u8]) -> Vec<WorktreeInfo> {
    let mut v = vec![];
    let mut cur: Option<WorktreeInfo> = None;
    for rec in split_z(bytes) {
        if rec.is_empty() {
            if let Some(w) = cur.take() {
                v.push(w);
            }
            continue;
        }
        let (k, val) = rec.split_once(' ').unwrap_or((rec.as_str(), ""));
        match k {
            "worktree" => {
                if let Some(w) = cur.take() {
                    v.push(w);
                }
                cur = Some(WorktreeInfo { path: val.to_string(), ..Default::default() });
            }
            "HEAD" => {
                if let Some(w) = cur.as_mut() {
                    w.head = Some(val.to_string());
                }
            }
            "branch" => {
                if let Some(w) = cur.as_mut() {
                    w.branch = Some(val.strip_prefix("refs/heads/").unwrap_or(val).to_string());
                }
            }
            "bare" => cur.iter_mut().for_each(|w| w.bare = true),
            "detached" => cur.iter_mut().for_each(|w| w.detached = true),
            "locked" => cur.iter_mut().for_each(|w| w.locked = true),
            "prunable" => cur.iter_mut().for_each(|w| w.prunable = true),
            _ => {}
        }
    }
    if let Some(w) = cur.take() {
        v.push(w);
    }
    v
}

pub async fn worktrees(repo: &Repo) -> Result<Vec<WorktreeInfo>, ApiError> {
    let out = repo.git().args(["worktree", "list", "--porcelain", "-z"]).run_ok().await?;
    let mut v = parse_worktrees(&out.stdout);
    for w in &mut v {
        w.current = std::path::Path::new(&w.path) == repo.top;
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracking_info() {
        assert_eq!(parse_track("ahead 2, behind 3"), (2, 3, false));
        assert_eq!(parse_track("behind 1"), (0, 1, false));
        assert_eq!(parse_track("gone"), (0, 0, true));
        assert_eq!(parse_track(""), (0, 0, false));
    }

    #[test]
    fn parses_branches_remotes_and_tags() {
        let t = "refs/heads/main\u{1f}aaa\u{1f}\u{1f}origin/main\u{1f}ahead 1\u{1f}Fix it\u{1f}1700000000\u{1f}*\n\
refs/heads/old\u{1f}bbb\u{1f}\u{1f}origin/old\u{1f}gone\u{1f}Old\u{1f}1600000000\u{1f} \n\
refs/remotes/origin/HEAD\u{1f}aaa\u{1f}\u{1f}\u{1f}\u{1f}Fix it\u{1f}1700000000\u{1f} \n\
refs/remotes/origin/feature/x\u{1f}ccc\u{1f}\u{1f}\u{1f}\u{1f}Feat\u{1f}1700000000\u{1f} \n\
refs/tags/v1\u{1f}ttt\u{1f}ddd\u{1f}\u{1f}\u{1f}Release 1\u{1f}1650000000\u{1f} \n";
        let (l, r, tg) = parse_refs(t);
        assert_eq!(l.len(), 2);
        assert!(l[0].current && l[0].ahead == 1 && l[0].upstream.as_deref() == Some("origin/main"));
        assert!(l[1].gone);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].remote.as_str(), r[0].branch.as_str()), ("origin", "feature/x"));
        assert_eq!(tg[0].sha, "ddd");
        assert!(tg[0].annotated);
        assert_eq!(l[0].time, 1_700_000_000_000);
    }

    #[test]
    fn parses_recent_checkouts() {
        let r = "commit: x\ncheckout: moving from main to feature\ncheckout: moving from feature to main\ncheckout: moving from main to feature\ncheckout: moving from dev to main\n";
        assert_eq!(parse_recent(r, 5), vec!["feature", "main"]);
    }

    #[test]
    fn parses_remotes_without_credentials() {
        let v = parse_remotes("origin\thttps://oauth2:secret@gitlab.com/a/b.git (fetch)\norigin\thttps://gitlab.com/a/b.git (push)\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].fetch_url, "https://gitlab.com/a/b.git");
        assert!(!format!("{v:?}").contains("secret"));
    }

    #[test]
    fn parses_worktree_porcelain() {
        let raw = b"worktree /r\0HEAD abc\0branch refs/heads/main\0\0worktree /r2\0HEAD def\0detached\0locked\0\0";
        let v = parse_worktrees(raw);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].branch.as_deref(), Some("main"));
        assert!(v[1].detached && v[1].locked);
    }
}

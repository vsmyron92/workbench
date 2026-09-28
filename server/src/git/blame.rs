//! `GET …/git/blame?path=&rev=` → `GitBlame` (cross-slice contract, used by the
//! editor's annotate gutter). Without `rev` the working-tree file is blamed and
//! uncommitted lines get the all-zero sha.

use std::collections::HashMap;

use serde::Serialize;

use super::cmd::check_rev;
use super::repo::Repo;
use crate::error::ApiError;

const MAX_BLAME_LINES: usize = 200_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    /// 1-based line in the blamed version.
    pub line: u32,
    pub sha: String,
    pub author: String,
    /// Author time, Unix milliseconds.
    pub time: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBlame {
    pub lines: Vec<BlameLine>,
    pub truncated: bool,
}

#[derive(Default, Clone)]
struct CommitMeta {
    author: String,
    time: i64,
    summary: String,
}

/// Parse `git blame --porcelain`. Metadata appears only on a commit's first group,
/// so it is remembered per sha.
pub fn parse_porcelain(text: &str) -> Vec<BlameLine> {
    let mut out = vec![];
    let mut meta: HashMap<String, CommitMeta> = HashMap::new();
    let mut cur_sha = String::new();
    let mut cur_line = 0u32;
    for l in text.lines() {
        if let Some(_content) = l.strip_prefix('\t') {
            let m = meta.get(&cur_sha).cloned().unwrap_or_default();
            out.push(BlameLine { line: cur_line, sha: cur_sha.clone(), author: m.author, time: m.time, summary: m.summary });
            if out.len() >= MAX_BLAME_LINES {
                break;
            }
            continue;
        }
        let mut parts = l.splitn(2, ' ');
        let first = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        if first.len() >= 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            // "<sha> <orig_line> <final_line> [<group_lines>]"
            let mut nums = rest.split(' ');
            let _orig = nums.next();
            cur_line = nums.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            cur_sha = first.to_string();
            meta.entry(cur_sha.clone()).or_default();
            continue;
        }
        let m = meta.entry(cur_sha.clone()).or_default();
        match first {
            "author" => m.author = rest.to_string(),
            "author-time" => m.time = rest.parse::<i64>().unwrap_or(0) * 1000,
            "summary" => m.summary = rest.to_string(),
            _ => {}
        }
    }
    out
}

pub async fn blame(repo: &Repo, path: &str, rev: Option<&str>) -> Result<GitBlame, ApiError> {
    let p = repo.to_repo(path)?;
    let mut g = repo.git().args(["blame", "--porcelain"]).timeout(std::time::Duration::from_secs(120));
    if let Some(r) = rev.filter(|r| !r.is_empty()) {
        // No `--end-of-options` here: git blame then takes the path for the
        // revision ("bad revision"). `check_rev` already refuses option-like
        // revisions. The path after `--` is a plain path (blame takes no pathspec).
        g = g.arg(check_rev(r)?);
    }
    let out = g.args(["--", &p]).run().await?;
    if !out.ok() {
        let msg = out.message();
        if msg.contains("no such path") || msg.contains("no such ref") {
            return Err(ApiError::not_found(msg));
        }
        return Err(super::cmd::git_error(&out));
    }
    let lines = parse_porcelain(&out.text());
    Ok(GitBlame { truncated: lines.len() >= MAX_BLAME_LINES, lines })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_with_repeated_commits() {
        let a = "a".repeat(40);
        let z = "0".repeat(40);
        let text = format!(
            "{a} 1 1 2\nauthor Ann\nauthor-mail <ann@x>\nauthor-time 1700000000\nauthor-tz +0000\nsummary First\nfilename f.rs\n\tline one\n\
{a} 2 2\n\tline two\n\
{z} 3 3 1\nauthor Not Committed Yet\nauthor-time 1800000000\nsummary Version of f.rs from f.rs\nfilename f.rs\n\tnew line\n"
        );
        let v = parse_porcelain(&text);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], BlameLine { line: 1, sha: a.clone(), author: "Ann".into(), time: 1_700_000_000_000, summary: "First".into() });
        assert_eq!(v[1].line, 2);
        assert_eq!(v[1].author, "Ann");
        assert_eq!(v[2].sha, z);
        assert_eq!(v[2].author, "Not Committed Yet");
    }
}

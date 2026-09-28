//! Stashes: list, show, and (in `ops`) push/apply/pop/drop.

use serde::Serialize;

use super::cmd::split_z;
use super::log::{ChangedFile, changed_files};
use super::repo::Repo;
use crate::error::ApiError;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub index: u32,
    /// `stash@{0}`
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
    /// Unix ms.
    pub time: i64,
    pub message: String,
    /// Branch the stash was made on (from "WIP on main: …" / "On main: …").
    pub branch: Option<String>,
}

pub fn parse_stash_list(bytes: &[u8]) -> Vec<StashEntry> {
    split_z(bytes)
        .into_iter()
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.splitn(4, '\u{1f}').collect();
            if f.len() < 4 {
                return None;
            }
            let index = f[0].strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()?;
            let subject = f[3];
            let branch = subject
                .strip_prefix("WIP on ")
                .or_else(|| subject.strip_prefix("On "))
                .and_then(|r| r.split_once(':'))
                .map(|(b, _)| b.to_string());
            // "On main: my message" → "my message"; "WIP on main: abc subject" stays.
            let message = match subject.strip_prefix("On ").and_then(|r| r.split_once(": ")) {
                Some((_, m)) => m.to_string(),
                None => subject.to_string(),
            };
            Some(StashEntry {
                index,
                reference: f[0].to_string(),
                sha: f[1].to_string(),
                time: f[2].parse::<i64>().unwrap_or(0) * 1000,
                message,
                branch,
            })
        })
        .collect()
}

pub async fn list(repo: &Repo) -> Result<Vec<StashEntry>, ApiError> {
    let out = repo.git().args(["stash", "list", "-z", "--format=%gd%x1f%H%x1f%ct%x1f%gs"]).run().await?;
    if !out.ok() {
        return Ok(vec![]);
    }
    Ok(parse_stash_list(&out.stdout))
}

/// The ref for index `n`, verifying it still is `sha` when given (indexes shift
/// when someone else stashes; never apply or drop the wrong entry).
pub async fn stash_ref(repo: &Repo, index: u32, sha: Option<&str>) -> Result<String, ApiError> {
    let r = format!("stash@{{{index}}}");
    let out = repo.git().args(["rev-parse", "--verify", "--quiet", &r]).run().await?;
    let actual = out.text().trim().to_string();
    if !out.ok() || actual.is_empty() {
        return Err(ApiError::not_found(format!("{r} does not exist")));
    }
    if let Some(s) = sha.filter(|s| !s.is_empty()) {
        if !actual.starts_with(s) {
            return Err(ApiError::conflict("the stash list changed; refresh and try again"));
        }
    }
    Ok(r)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashDetails {
    pub index: u32,
    pub sha: String,
    /// The stash's base commit (diff `base → sha` for tracked files).
    pub base: String,
    /// Commit holding untracked files (`--include-untracked`), if any; diff it in
    /// `commit` mode (it has no parent).
    pub untracked_sha: Option<String>,
    pub files: Vec<ChangedFile>,
    pub untracked: Vec<ChangedFile>,
}

pub async fn show(repo: &Repo, index: u32) -> Result<StashDetails, ApiError> {
    let r = stash_ref(repo, index, None).await?;
    let rp = |rev: String| async move {
        let o = repo.git().args(["rev-parse", "--verify", "--quiet", &rev]).run().await?;
        Ok::<_, ApiError>(o.ok().then(|| o.text().trim().to_string()).filter(|s| !s.is_empty()))
    };
    let sha = rp(r.clone()).await?.ok_or_else(|| ApiError::not_found("stash vanished"))?;
    let base = rp(format!("{sha}^1")).await?.ok_or_else(|| ApiError::internal("stash has no base"))?;
    let untracked_sha = rp(format!("{sha}^3")).await?;
    let (files, _) = changed_files(repo, &base, Some(&sha)).await?;
    let untracked = match &untracked_sha {
        Some(u) => {
            let empty = super::diff::empty_tree(repo).await?;
            changed_files(repo, &empty, Some(u)).await?.0
        }
        None => vec![],
    };
    Ok(StashDetails { index, sha, base, untracked_sha, files, untracked })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stash_list() {
        let raw = "stash@{0}\u{1f}aaa\u{1f}1700000000\u{1f}On main: my work\0stash@{1}\u{1f}bbb\u{1f}1600000000\u{1f}WIP on feature/x: 1234567 subject\0";
        let v = parse_stash_list(raw.as_bytes());
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].index, v[0].message.as_str(), v[0].branch.as_deref()), (0, "my work", Some("main")));
        assert_eq!((v[1].index, v[1].branch.as_deref()), (1, Some("feature/x")));
        assert_eq!(v[1].message, "WIP on feature/x: 1234567 subject");
        assert_eq!(v[0].time, 1_700_000_000_000);
    }
}

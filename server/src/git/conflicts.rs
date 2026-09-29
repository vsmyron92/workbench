//! Merge conflicts: the three stages of a conflicted file plus the working-tree
//! result, and resolving (with edited content, or by taking one side).

use serde::{Deserialize, Serialize};

use super::cmd::literal;
use super::diff::{Side, blob_side, worktree_bytes, worktree_side};
use super::eol;
use super::repo::Repo;
use super::status::detect_state;
use crate::error::ApiError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictVersions {
    pub path: String,
    /// Common ancestor (stage 1); null when the file was added on both sides.
    pub base: Option<String>,
    /// Our version (stage 2); null when we deleted it.
    pub ours: Option<String>,
    /// Their version (stage 3); null when they deleted it.
    pub theirs: Option<String>,
    /// The working-tree file (with conflict markers) as git reads it (LF where git turns
    /// its CRLFs into LFs); '' when deleted.
    pub merged: String,
    pub binary: bool,
    pub too_large: bool,
    pub ours_label: String,
    pub theirs_label: String,
    /// clean | merging | rebasing | cherry-picking | reverting
    pub state: &'static str,
    /// False once the file is resolved (the other fields are then empty), so a
    /// panel refreshing right after its own resolve gets data rather than an error.
    pub in_conflict: bool,
}

fn opt_text(s: &Side) -> Option<String> {
    (!s.missing).then(|| s.text.clone())
}

async fn short_desc(repo: &Repo, rev: &str) -> String {
    let out = repo.git().args(["log", "-1", "--format=%h %s", "--end-of-options", rev, "--"]).run().await;
    match out {
        Ok(o) if o.ok() => {
            let s = o.text().trim().to_string();
            if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s }
        }
        _ => rev.chars().take(10).collect(),
    }
}

pub async fn versions(repo: &Repo, path: &str) -> Result<ConflictVersions, ApiError> {
    let p = repo.to_repo(path)?;
    let (s1, s2, s3) = (format!(":1:{p}"), format!(":2:{p}"), format!(":3:{p}"));
    let (base, ours, theirs, merged) =
        tokio::join!(blob_side(repo, &s1), blob_side(repo, &s2), blob_side(repo, &s3), worktree_side(repo, &p));
    let (base, ours, theirs, merged) = (base?, ours?, theirs?, merged?);
    let (state, detail) = detect_state(&repo.git_dir);
    if base.missing && ours.missing && theirs.missing {
        return Ok(ConflictVersions {
            path: path.to_string(),
            base: None,
            ours: None,
            theirs: None,
            merged: String::new(),
            binary: false,
            too_large: false,
            ours_label: String::new(),
            theirs_label: String::new(),
            state,
            in_conflict: false,
        });
    }
    let binary = [&base, &ours, &theirs, &merged].iter().any(|s| s.binary);
    let too_large = [&base, &ours, &theirs, &merged].iter().any(|s| s.too_large);
    let branch = crate::util::git::current_branch(&repo.top).await;
    // During a rebase "ours" is the branch being rebased onto and "theirs" the
    // commit being replayed — label them so nobody has to remember that.
    let (ours_label, theirs_label) = match state {
        "rebasing" => {
            let onto = match &detail.onto {
                Some(o) => short_desc(repo, o).await,
                None => "upstream".into(),
            };
            let replay = if repo.git_dir.join("REBASE_HEAD").exists() { short_desc(repo, "REBASE_HEAD").await } else { "commit".into() };
            (format!("Upstream ({onto})"), format!("Rebasing ({replay})"))
        }
        "merging" => (
            format!("Yours ({})", branch.clone().unwrap_or_else(|| "HEAD".into())),
            format!("Theirs ({})", detail.branch.clone().unwrap_or_else(|| "MERGE_HEAD".into())),
        ),
        "cherry-picking" => ("Yours (HEAD)".into(), format!("Cherry-picked ({})", short_desc(repo, "CHERRY_PICK_HEAD").await)),
        "reverting" => ("Yours (HEAD)".into(), format!("Revert of ({})", short_desc(repo, "REVERT_HEAD").await)),
        _ => (
            format!("Yours ({})", branch.unwrap_or_else(|| "HEAD".into())),
            "Theirs (stash or other change)".into(),
        ),
    };
    let text = |s: &Side| if binary || too_large { None } else { opt_text(s) };
    Ok(ConflictVersions {
        path: path.to_string(),
        base: text(&base),
        ours: text(&ours),
        theirs: text(&theirs),
        merged: if binary || too_large { String::new() } else { merged.text.clone() },
        binary,
        too_large,
        ours_label,
        theirs_label,
        state,
        in_conflict: true,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveRequest {
    pub path: String,
    /// The resolved text to write, or
    pub content: Option<String>,
    /// take one side wholesale: `ours` | `theirs`.
    pub side: Option<String>,
}

/// Is this repository path unmerged (has conflict stages in the index)?
async fn is_unmerged(repo: &Repo, p: &str) -> Result<bool, ApiError> {
    let out = repo.git().args(["ls-files", "-u", "-z", "--"]).arg(literal(p)).run_ok().await?;
    Ok(!out.stdout.is_empty())
}

/// Resolve and stage one file. The caller holds the repository write lock.
///
/// Only an unmerged path can be resolved: this endpoint writes files and runs
/// `git rm --force` for a deleted side, so it must never act on anything else.
pub async fn resolve(repo: &Repo, req: &ResolveRequest) -> Result<(), ApiError> {
    let p = repo.to_repo(&req.path)?;
    let side = match req.side.as_deref() {
        None => None,
        Some("ours") => Some(("ours", ":2:")),
        Some("theirs") => Some(("theirs", ":3:")),
        Some(_) => return Err(ApiError::bad_request("side must be ours or theirs")),
    };
    if req.content.is_none() && side.is_none() {
        return Err(ApiError::bad_request("give content or side"));
    }
    if !is_unmerged(repo, &p).await? {
        return Err(ApiError::conflict(format!("{} is not in conflict (anymore)", req.path)));
    }
    if let Some(content) = &req.content {
        let abs = crate::util::paths::resolve_in_root(&repo.top, &p)?;
        // A file git checks out with CRLF was shown with LF (`versions`): the CRLFs go back.
        // Only a file with CRLFs now can be one (LF files write as before, with no lookup).
        let has_crlf = worktree_bytes(repo, &p).await.is_ok_and(|s| s.text.contains("\r\n"));
        let eol = if has_crlf { eol::of(repo, &p).await } else { eol::Eol::default() };
        let data = eol.write(content.clone()).into_bytes();
        tokio::task::spawn_blocking(move || crate::util::fs::write_atomic(&abs, &data, 0o644))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
        repo.git_w().args(["add".to_string(), "--".into(), literal(&p)]).run_ok_retry_lock().await?;
        return Ok(());
    }
    let Some((side, stage)) = side else { return Err(ApiError::bad_request("give content or side")) };
    let exists = repo.git().args(["cat-file", "-e", &format!("{stage}{p}")]).run().await?.ok();
    if exists {
        repo.git_w().args(["checkout".to_string(), format!("--{side}"), "--".into(), literal(&p)]).run_ok_retry_lock().await?;
        repo.git_w().args(["add".to_string(), "--".into(), literal(&p)]).run_ok_retry_lock().await?;
    } else {
        // That side deleted the file: resolve as a deletion.
        repo.git_w().args(["rm".to_string(), "--quiet".into(), "--force".into(), "--".into(), literal(&p)]).run_ok_retry_lock().await?;
    }
    Ok(())
}

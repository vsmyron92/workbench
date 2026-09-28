//! `git bisect`: start (bad = HEAD or a chosen commit, good = chosen commits), mark
//! the checked-out candidate (or any commit) good / bad / skip, and reset. The state
//! is read from the git dir: `BISECT_START`, `BISECT_TERMS`, `BISECT_LOG` and the
//! `refs/bisect/*` refs; the remaining range from `git rev-list --bisect-vars`.

use serde::{Deserialize, Serialize};

use super::cmd::{GitOutput, clean_message};
use super::diff::resolve_commit;
use super::repo::Repo;
use crate::error::ApiError;

/// Candidate shas sent for the log's markers.
const MAX_CANDIDATES: usize = 5000;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BisectState {
    pub active: bool,
    /// The words for good and bad (`git bisect start --term-old/--term-new`).
    pub term_good: String,
    pub term_bad: String,
    pub bad: Option<String>,
    pub good: Vec<String>,
    pub skipped: Vec<String>,
    /// The checked-out commit being tested.
    pub current: Option<String>,
    /// What `git bisect reset` returns to (a branch name or a sha).
    pub start: Option<String>,
    /// Revisions left to test after the current one and the rough number of steps
    /// (`--bisect-vars`: `bisect_nr`, `bisect_steps`, as git prints them).
    pub remaining: Option<u32>,
    pub steps: Option<u32>,
    /// The first bad commit, once found.
    pub result: Option<String>,
    /// Commits still in the range (`bad` minus everything good), for the log markers.
    pub candidates: Vec<String>,
    /// `BISECT_LOG` without the `git bisect` commands.
    pub log: Vec<String>,
}

/// `bisect_rev=…\nbisect_nr=3\n…` → (remaining, steps).
pub fn parse_bisect_vars(text: &str) -> (Option<u32>, Option<u32>) {
    let mut all = None;
    let mut steps = None;
    for l in text.lines() {
        if let Some((k, v)) = l.split_once('=') {
            let v = v.trim().trim_matches('\'');
            match k.trim() {
                "bisect_nr" => all = v.parse().ok(),
                "bisect_steps" => steps = v.parse().ok(),
                _ => {}
            }
        }
    }
    (all, steps)
}

/// The first bad commit recorded in `BISECT_LOG` (`# first bad commit: [sha] subject`;
/// newer git quotes the term: `# first 'bad' commit: [sha] …`), and the log's
/// comment lines (`# good: [sha] subject`, …).
pub fn parse_bisect_log(text: &str) -> (Option<String>, Vec<String>) {
    let mut result = None;
    let mut log = vec![];
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("# ") {
            if let Some(r) = rest.strip_prefix("first ") {
                if let Some(i) = r.find("commit: [") {
                    result = r[i + 9..].split(']').next().map(str::to_string).filter(|s| !s.is_empty());
                }
            }
            log.push(rest.to_string());
        }
    }
    (result, log)
}

async fn rev(repo: &Repo, r: &str) -> Result<Option<String>, ApiError> {
    let o = repo.git().args(["rev-parse", "--verify", "--quiet", r]).run().await?;
    Ok(o.ok().then(|| o.text().trim().to_string()).filter(|s| !s.is_empty()))
}

async fn refs_under(repo: &Repo, prefix: &str) -> Result<Vec<String>, ApiError> {
    let out = repo.git().args(["for-each-ref", "--format=%(objectname)", prefix]).run_ok().await?;
    Ok(out.text().lines().map(str::to_string).filter(|s| !s.is_empty()).collect())
}

fn terms(repo: &Repo) -> (String, String) {
    // BISECT_TERMS: the bad term, then the good one.
    match std::fs::read_to_string(repo.git_dir.join("BISECT_TERMS")) {
        Ok(t) => {
            let mut l = t.lines().map(str::trim);
            let bad = l.next().filter(|s| !s.is_empty()).unwrap_or("bad").to_string();
            let good = l.next().filter(|s| !s.is_empty()).unwrap_or("good").to_string();
            (good, bad)
        }
        Err(_) => ("good".into(), "bad".into()),
    }
}

pub async fn state(repo: &Repo) -> Result<BisectState, ApiError> {
    let log_text = std::fs::read_to_string(repo.git_dir.join("BISECT_LOG")).ok();
    let Some(log_text) = log_text else {
        return Ok(BisectState { term_good: "good".into(), term_bad: "bad".into(), ..Default::default() });
    };
    let (term_good, term_bad) = terms(repo);
    let start = std::fs::read_to_string(repo.git_dir.join("BISECT_START")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let bad = rev(repo, &format!("refs/bisect/{term_bad}")).await?;
    let good = refs_under(repo, &format!("refs/bisect/{term_good}-*")).await?;
    let skipped = refs_under(repo, "refs/bisect/skip-*").await?;
    let current = rev(repo, "HEAD").await?;
    let (result, log) = parse_bisect_log(&log_text);
    let (mut remaining, mut steps, mut candidates) = (None, None, vec![]);
    if let Some(b) = &bad {
        if !good.is_empty() {
            let mut args = vec![b.clone(), "--not".into()];
            args.extend(good.iter().cloned());
            let vars = repo.git().args(["rev-list", "--bisect-vars"]).args(args.iter().cloned()).run().await?;
            if vars.ok() {
                (remaining, steps) = parse_bisect_vars(&vars.text());
            }
            let list = repo.git().args(["rev-list".to_string(), format!("--max-count={MAX_CANDIDATES}")]).args(args).run().await?;
            if list.ok() {
                candidates = list.text().lines().map(str::to_string).collect();
            }
        }
    }
    Ok(BisectState {
        active: true,
        term_good,
        term_bad,
        bad,
        good,
        skipped,
        current,
        start,
        remaining,
        steps,
        result,
        candidates,
        log,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// The bad commit (default HEAD).
    pub bad: Option<String>,
    /// Known good commits (at least one).
    pub good: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BisectOutcome {
    pub message: String,
    pub state: BisectState,
}

/// "Bisecting: 3 revisions left to test after this (roughly 2 steps)" or
/// "<sha> is the first bad commit" from git's output.
fn summarize(out: &GitOutput) -> String {
    let text = out.text();
    if let Some(l) = text.lines().find(|l| l.contains(" is the first ") && l.trim_end().ends_with("commit")) {
        return l.trim().to_string();
    }
    if let Some(l) = text.lines().find(|l| l.starts_with("Bisecting:")) {
        return l.trim().to_string();
    }
    let m = clean_message(&format!("{}\n{}", text, out.stderr));
    m.lines().next().unwrap_or("").to_string()
}

pub async fn start(repo: &Repo, req: &StartRequest) -> Result<BisectOutcome, ApiError> {
    if repo.git_dir.join("BISECT_LOG").exists() {
        return Err(ApiError::conflict("a bisect is already running; reset it first"));
    }
    if req.good.is_empty() {
        return Err(ApiError::bad_request("mark at least one good commit"));
    }
    if req.good.len() > 50 {
        return Err(ApiError::bad_request("too many good commits"));
    }
    let bad = resolve_commit(repo, req.bad.as_deref().filter(|s| !s.is_empty()).unwrap_or("HEAD")).await?;
    let mut goods = vec![];
    for g in &req.good {
        let g = resolve_commit(repo, g).await?;
        if g == bad {
            return Err(ApiError::bad_request("the good and the bad commit are the same"));
        }
        let anc = repo.git().args(["merge-base", "--is-ancestor", &g, &bad]).run().await?;
        if !anc.ok() {
            return Err(ApiError::bad_request(format!(
                "{} is not an ancestor of the bad commit {}: bisect looks for the change between them",
                &g[..10], &bad[..10]
            )));
        }
        goods.push(g);
    }
    // Full shas (resolved above), so nothing can be read as an option (bisect has no --end-of-options).
    let out = repo.git_w().args(["bisect", "start", &bad]).args(goods.iter().cloned()).arg("--").run().await?;
    if !out.ok() {
        // A failed start can leave BISECT_* files behind: clean up so the state is not half-started.
        let _ = repo.git_w().args(["bisect", "reset"]).run().await;
        let e = super::cmd::git_error(&out);
        if e.code == "dirty_tree" {
            return Err(ApiError::new(
                e.status,
                "dirty_tree",
                "Bisect checks out other commits, and your local changes are in the way: commit, stash or shelve them first.",
            ));
        }
        return Err(e);
    }
    Ok(BisectOutcome { message: summarize(&out), state: state(repo).await? })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkRequest {
    /// good | bad | skip
    pub verdict: String,
    /// The commit to mark (default: the checked-out one).
    pub rev: Option<String>,
}

pub async fn mark(repo: &Repo, req: &MarkRequest) -> Result<BisectOutcome, ApiError> {
    if !repo.git_dir.join("BISECT_LOG").exists() {
        return Err(ApiError::bad_request("no bisect is running"));
    }
    let (term_good, term_bad) = terms(repo);
    let word = match req.verdict.as_str() {
        "good" => term_good,
        "bad" => term_bad,
        "skip" => "skip".to_string(),
        _ => return Err(ApiError::bad_request("verdict must be good, bad or skip")),
    };
    let mut g = repo.git_w().args(["bisect", &word]);
    if let Some(r) = req.rev.as_deref().filter(|s| !s.is_empty()) {
        g = g.arg(resolve_commit(repo, r).await?);
    }
    let out = g.run().await?;
    if !out.ok() {
        return Err(super::cmd::git_error(&out));
    }
    Ok(BisectOutcome { message: summarize(&out), state: state(repo).await? })
}

pub async fn reset(repo: &Repo) -> Result<BisectOutcome, ApiError> {
    if !repo.git_dir.join("BISECT_LOG").exists() && !repo.git_dir.join("BISECT_START").exists() {
        return Err(ApiError::bad_request("no bisect is running"));
    }
    let out = repo.git_w().args(["bisect", "reset"]).run().await?;
    if !out.ok() {
        return Err(super::cmd::git_error(&out));
    }
    Ok(BisectOutcome { message: "Bisect reset".into(), state: state(repo).await? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bisect_vars() {
        let t = "bisect_rev='abc'\nbisect_nr=3\nbisect_good=1\nbisect_bad=2\nbisect_all=7\nbisect_steps=2\n";
        assert_eq!(parse_bisect_vars(t), (Some(3), Some(2)));
        assert_eq!(parse_bisect_vars(""), (None, None));
    }

    #[test]
    fn parses_the_log_and_its_result() {
        let t = "git bisect start 'bbb' 'aaa'\n# bad: [bbbb] broke it\n# good: [aaaa] fine\ngit bisect good cccc\n# good: [cccc] also fine\n# first bad commit: [dddd] the culprit\n";
        let (r, log) = parse_bisect_log(t);
        assert_eq!(r.as_deref(), Some("dddd"));
        assert_eq!(log, vec!["bad: [bbbb] broke it", "good: [aaaa] fine", "good: [cccc] also fine", "first bad commit: [dddd] the culprit"]);
        assert_eq!(parse_bisect_log("git bisect start\n").0, None);
        let (r, _) = parse_bisect_log("# first 'bad' commit: [eeee] quoted term\n");
        assert_eq!(r.as_deref(), Some("eeee"));
    }
}

//! GitHub JSON shapes. Each struct deserializes GitHub's snake_case JSON (every
//! field optional through `#[serde(default)]`, so API additions and omissions
//! never break parsing) and serializes camelCase for the browser. Fields GitHub
//! does not send (`state`, `duration`, `commit_title`…) are filled in by the
//! operations. TypeScript mirrors live in `web/src/features/github/types.ts`.
//!
//! Status vocabulary: runs, jobs, steps and check runs carry GitHub's
//! `status` + `conclusion`; `state` is the GitLab-style word the rest of
//! Workbench uses (`success`, `failed`, `running`, `pending`, `canceled`,
//! `skipped`, `manual`), from `run_state`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One page of a list, as returned to the browser.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPage<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub next_page: Option<u32>,
    /// GitHub's `total_count` where the endpoint has one.
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct User {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
    pub html_url: String,
    #[serde(rename = "type")]
    pub user_type: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Label {
    pub id: u64,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Milestone {
    pub number: u64,
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct RepoRef {
    pub id: u64,
    pub full_name: String,
    pub fork: bool,
    pub html_url: String,
}

/// `head` / `base` of a pull request.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct PrRef {
    pub label: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
    pub repo: Option<RepoRef>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Pull {
    pub id: u64,
    pub node_id: String,
    pub number: u64,
    /// `open` | `closed` (see `merged`).
    pub state: String,
    pub title: String,
    pub body: Option<String>,
    pub draft: bool,
    pub locked: bool,
    pub user: Option<User>,
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<User>,
    pub labels: Vec<Label>,
    pub milestone: Option<Milestone>,
    /// Absent on search results.
    pub head: Option<PrRef>,
    pub base: Option<PrRef>,
    pub html_url: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub author_association: Option<String>,
    /// Filled from `merged_at` on list entries.
    pub merged: bool,
    // Detail-only fields.
    /// `null` while GitHub computes it.
    pub mergeable: Option<bool>,
    /// `clean` | `dirty` | `blocked` | `behind` | `unstable` | `has_hooks` | `draft` | `unknown`
    pub mergeable_state: Option<String>,
    pub rebaseable: Option<bool>,
    pub merged_by: Option<User>,
    pub comments: Option<u64>,
    pub review_comments: Option<u64>,
    pub commits: Option<u64>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
    pub maintainer_can_modify: Option<bool>,
    pub auto_merge: Option<Value>,
}

/// A pull request with what the detail view needs besides GitHub's object.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDetail {
    #[serde(flatten)]
    pub pull: Pull,
    /// Checks and statuses of the head commit.
    pub checks: Option<CommitChecks>,
    /// Latest review per reviewer.
    pub review_states: Vec<ReviewState>,
    /// The commit the base branch and the head share (the "original" side of diffs).
    pub merge_base_sha: Option<String>,
    /// Parts that could not be loaded.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewState {
    pub user: Option<User>,
    /// `APPROVED` | `CHANGES_REQUESTED` | `COMMENTED` | `DISMISSED`
    pub state: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct PrFile {
    pub sha: Option<String>,
    pub filename: String,
    /// `added` | `removed` | `modified` | `renamed` | `copied` | `changed` | `unchanged`
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub changes: u64,
    pub previous_filename: Option<String>,
    /// Missing for binary and very large files.
    pub patch: Option<String>,
    /// Set when `patch` was dropped because it is too large to send.
    pub too_large: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Review {
    pub id: u64,
    pub user: Option<User>,
    pub body: Option<String>,
    /// `APPROVED` | `CHANGES_REQUESTED` | `COMMENTED` | `DISMISSED` | `PENDING`
    pub state: String,
    pub submitted_at: Option<String>,
    pub commit_id: Option<String>,
    pub html_url: String,
    pub author_association: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct ReviewComment {
    pub id: u64,
    pub node_id: String,
    pub pull_request_review_id: Option<u64>,
    pub diff_hunk: String,
    pub path: String,
    pub commit_id: Option<String>,
    pub original_commit_id: Option<String>,
    pub in_reply_to_id: Option<u64>,
    pub user: Option<User>,
    pub body: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub html_url: String,
    /// Line in the current diff; `null` once the comment is outdated.
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    pub start_line: Option<u64>,
    pub original_start_line: Option<u64>,
    /// `LEFT` (old side) | `RIGHT` (new side)
    pub side: Option<String>,
    pub start_side: Option<String>,
    /// `line` | `file`
    pub subject_type: Option<String>,
    pub author_association: Option<String>,
}

/// Review comments grouped into a thread (root comment plus replies).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// GraphQL node id when known (needed to resolve), else `c<root comment id>`.
    pub id: String,
    pub root_id: u64,
    pub path: String,
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    pub start_line: Option<u64>,
    pub side: Option<String>,
    pub outdated: bool,
    /// `None`: unknown (no token, so no GraphQL).
    pub resolved: Option<bool>,
    pub resolved_by: Option<String>,
    pub can_resolve: bool,
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct IssueComment {
    pub id: u64,
    pub user: Option<User>,
    pub body: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub html_url: String,
    pub author_association: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawGitPerson {
    pub name: String,
    pub email: String,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawGitCommit {
    pub message: String,
    pub author: Option<RawGitPerson>,
    pub committer: Option<RawGitPerson>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawCommit {
    pub sha: String,
    pub commit: RawGitCommit,
    pub author: Option<User>,
    pub html_url: String,
}

/// A commit, flattened for the browser.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    pub title: String,
    pub message: String,
    pub author_name: String,
    pub author_login: Option<String>,
    pub date: Option<String>,
    pub html_url: String,
}

impl From<RawCommit> for Commit {
    fn from(c: RawCommit) -> Self {
        let a = c.commit.author.clone().unwrap_or_default();
        Commit {
            short_sha: c.sha.chars().take(7).collect(),
            title: first_line(&c.commit.message),
            message: c.commit.message,
            author_name: a.name,
            author_login: c.author.map(|u| u.login),
            date: a.date.or_else(|| c.commit.committer.and_then(|x| x.date)),
            html_url: c.html_url,
            sha: c.sha,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Issue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    /// `open` | `closed`
    pub state: String,
    /// `completed` | `not_planned` | `reopened` | null
    pub state_reason: Option<String>,
    pub labels: Vec<Label>,
    pub assignees: Vec<User>,
    pub user: Option<User>,
    pub milestone: Option<Milestone>,
    pub comments: u64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    pub html_url: String,
    pub locked: bool,
    pub author_association: Option<String>,
    /// Present when the "issue" is a pull request (the issues API lists both).
    #[serde(skip_serializing)]
    pub pull_request: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub download_count: u64,
    pub browser_download_url: String,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
    pub created_at: Option<String>,
    pub published_at: Option<String>,
    pub html_url: String,
    pub author: Option<User>,
    pub target_commitish: Option<String>,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Workflow {
    pub id: u64,
    pub name: String,
    pub path: String,
    /// `active` | `disabled_manually` | …
    pub state: String,
    pub html_url: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct HeadCommit {
    pub id: String,
    pub message: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct RunPr {
    pub number: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Run {
    pub id: u64,
    pub name: Option<String>,
    pub display_title: Option<String>,
    pub run_number: u64,
    pub run_attempt: Option<u64>,
    pub event: String,
    /// `queued` | `in_progress` | `completed` | `waiting` | `requested` | `pending`
    pub status: String,
    pub conclusion: Option<String>,
    pub workflow_id: u64,
    pub head_branch: Option<String>,
    pub head_sha: String,
    pub path: Option<String>,
    pub html_url: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub run_started_at: Option<String>,
    pub actor: Option<User>,
    pub triggering_actor: Option<User>,
    pub head_commit: Option<HeadCommit>,
    pub pull_requests: Vec<RunPr>,
    // Filled in.
    pub state: String,
    /// Seconds from start to the last update (finished runs) or to now.
    pub duration: Option<f64>,
    pub commit_title: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Step {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub number: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Job {
    pub id: u64,
    pub run_id: u64,
    pub run_attempt: Option<u64>,
    pub name: String,
    pub workflow_name: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub html_url: String,
    pub head_sha: String,
    pub head_branch: Option<String>,
    pub labels: Vec<String>,
    pub runner_name: Option<String>,
    pub steps: Vec<Step>,
    // Filled in.
    pub state: String,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct App {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CheckRun {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub html_url: Option<String>,
    pub details_url: Option<String>,
    pub app: Option<App>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CommitStatus {
    pub context: String,
    /// `success` | `pending` | `failure` | `error`
    pub state: String,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CombinedStatus {
    pub state: String,
    pub total_count: u64,
    pub statuses: Vec<CommitStatus>,
}

/// An artifact a workflow run uploaded (`actions/upload-artifact`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Artifact {
    pub id: u64,
    pub name: String,
    pub size_in_bytes: u64,
    /// Past its retention: GitHub deleted the content.
    pub expired: bool,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Annotation {
    pub path: String,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    /// `notice` | `warning` | `failure`
    pub annotation_level: String,
    pub title: Option<String>,
    pub message: String,
}

/// One check of a commit: a check run (a job, for Actions), a legacy commit
/// status, or a workflow run that has no check runs yet.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub name: String,
    /// `check` | `status` | `run`
    pub kind: String,
    pub state: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
    pub app: Option<String>,
    pub description: Option<String>,
    pub run_id: Option<u64>,
    pub job_id: Option<u64>,
    /// Workflow name and trigger of the run the check belongs to (Actions only).
    pub workflow: Option<String>,
    pub event: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// Everything that reported on one commit, combined into one state.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitChecks {
    pub sha: String,
    /// Combined state in the shared vocabulary; `None` when nothing reported.
    pub state: Option<String>,
    pub items: Vec<CheckItem>,
    /// Workflow runs of the commit (newest per workflow and event).
    pub runs: Vec<Run>,
}

// ---------------------------------------------------------------- pure helpers

/// GitHub `status` + `conclusion` → the shared vocabulary.
pub fn run_state(status: &str, conclusion: Option<&str>) -> &'static str {
    match status {
        "completed" => match conclusion.unwrap_or("") {
            "success" | "neutral" => "success",
            "failure" | "timed_out" | "startup_failure" => "failed",
            "cancelled" => "canceled",
            "action_required" => "manual",
            // skipped, stale, or a conclusion we do not know
            _ => "skipped",
        },
        "in_progress" => "running",
        "waiting" => "manual",
        // queued, requested, pending
        _ => "pending",
    }
}

/// A legacy commit status → the shared vocabulary.
pub fn status_state(state: &str) -> &'static str {
    match state {
        "success" => "success",
        "failure" | "error" => "failed",
        _ => "pending",
    }
}

/// States that can still change.
pub fn is_active(state: &str) -> bool {
    matches!(state, "running" | "pending")
}

/// Combine states: a failure wins, then anything still going, then approvals
/// waited on, cancellations, success; all-skipped stays skipped.
pub fn aggregate<'a>(states: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    const ORDER: [&str; 7] = ["failed", "running", "pending", "manual", "canceled", "success", "skipped"];
    let mut best: Option<usize> = None;
    for s in states {
        if let Some(i) = ORDER.iter().position(|o| *o == s) {
            best = Some(best.map_or(i, |b| b.min(i)));
        }
    }
    best.map(|i| ORDER[i])
}

pub fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

/// Seconds between two RFC 3339 times (`end` defaults to now).
pub fn seconds_between(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    let s = chrono::DateTime::parse_from_rfc3339(start?).ok()?;
    let e = match end {
        Some(e) => chrono::DateTime::parse_from_rfc3339(e).ok()?.with_timezone(&chrono::Utc),
        None => chrono::Utc::now(),
    };
    Some(((e - s.with_timezone(&chrono::Utc)).num_milliseconds() as f64 / 1000.0).max(0.0))
}

impl Run {
    pub fn fill(&mut self) {
        self.state = run_state(&self.status, self.conclusion.as_deref()).to_string();
        let start = self.run_started_at.as_deref().or(self.created_at.as_deref());
        self.duration = if self.status == "completed" {
            seconds_between(start, self.updated_at.as_deref())
        } else if self.status == "in_progress" {
            seconds_between(start, None)
        } else {
            None
        };
        if self.commit_title.is_none() {
            self.commit_title = self.head_commit.as_ref().map(|c| first_line(&c.message)).filter(|t| !t.is_empty());
        }
    }
}

impl Job {
    pub fn fill(&mut self) {
        self.state = run_state(&self.status, self.conclusion.as_deref()).to_string();
        self.duration = match (self.started_at.as_deref(), self.completed_at.as_deref()) {
            (Some(s), Some(e)) if self.status == "completed" => seconds_between(Some(s), Some(e)),
            (Some(s), None) if self.status == "in_progress" => seconds_between(Some(s), None),
            _ => None,
        };
        for st in &mut self.steps {
            st.state = run_state(&st.status, st.conclusion.as_deref()).to_string();
        }
    }
}

/// `(run id, job id)` from an Actions URL like
/// `https://github.com/o/r/actions/runs/123/job/456`.
pub fn actions_ids(url: &str) -> (Option<u64>, Option<u64>) {
    let Some(rest) = url.split("/actions/runs/").nth(1) else { return (None, None) };
    let mut parts = rest.split(['/', '?', '#']);
    let run = parts.next().and_then(|p| p.parse().ok());
    let job = match (parts.next(), parts.next()) {
        (Some("job" | "jobs"), Some(j)) => j.parse().ok(),
        _ => None,
    };
    (run, job)
}

/// Group review comments into threads (a reply names its root in `in_reply_to_id`),
/// oldest thread first, comments in order.
pub fn group_threads(comments: Vec<ReviewComment>) -> Vec<Thread> {
    let mut threads: Vec<Thread> = vec![];
    let mut index: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut sorted = comments;
    sorted.sort_by_key(|c| c.id);
    for c in sorted {
        let root = c.in_reply_to_id.and_then(|r| index.get(&r).copied());
        match root {
            Some(i) => {
                index.insert(c.id, i);
                threads[i].comments.push(c);
            }
            None => {
                index.insert(c.id, threads.len());
                threads.push(Thread {
                    id: format!("c{}", c.id),
                    root_id: c.id,
                    path: c.path.clone(),
                    line: c.line,
                    original_line: c.original_line,
                    start_line: c.start_line,
                    side: c.side.clone(),
                    outdated: c.line.is_none() && c.subject_type.as_deref() != Some("file"),
                    resolved: None,
                    resolved_by: None,
                    can_resolve: false,
                    comments: vec![c],
                });
            }
        }
    }
    threads
}

/// The latest meaningful review per reviewer: an approval or change request
/// replaces earlier ones; a plain comment counts only when there is nothing else.
pub fn review_states(reviews: &[Review]) -> Vec<ReviewState> {
    let mut out: Vec<ReviewState> = vec![];
    for r in reviews {
        if r.state == "PENDING" {
            continue;
        }
        let login = r.user.as_ref().map(|u| u.login.clone()).unwrap_or_default();
        let pos = out.iter().position(|s| s.user.as_ref().map(|u| u.login.as_str()).unwrap_or("") == login);
        let entry = ReviewState { user: r.user.clone(), state: r.state.clone(), submitted_at: r.submitted_at.clone() };
        match pos {
            None => out.push(entry),
            Some(i) if r.state != "COMMENTED" || out[i].state == "COMMENTED" => out[i] = entry,
            Some(_) => {}
        }
    }
    out
}

/// Count `+`/`-` lines of a unified diff hunk body (headers excluded).
#[cfg(test)]
pub fn diff_stats(diff: &str) -> (u64, u64) {
    let (mut add, mut del) = (0, 0);
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            add += 1;
        } else if line.starts_with('-') {
            del += 1;
        }
    }
    (add, del)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn states_map_to_the_shared_vocabulary() {
        assert_eq!(run_state("completed", Some("success")), "success");
        assert_eq!(run_state("completed", Some("neutral")), "success");
        assert_eq!(run_state("completed", Some("failure")), "failed");
        assert_eq!(run_state("completed", Some("timed_out")), "failed");
        assert_eq!(run_state("completed", Some("startup_failure")), "failed");
        assert_eq!(run_state("completed", Some("cancelled")), "canceled");
        assert_eq!(run_state("completed", Some("skipped")), "skipped");
        assert_eq!(run_state("completed", Some("action_required")), "manual");
        assert_eq!(run_state("completed", None), "skipped");
        assert_eq!(run_state("in_progress", None), "running");
        assert_eq!(run_state("queued", None), "pending");
        assert_eq!(run_state("waiting", None), "manual");
        assert_eq!(status_state("error"), "failed");
        assert_eq!(status_state("pending"), "pending");
    }

    #[test]
    fn aggregation_prefers_failures_then_activity() {
        assert_eq!(aggregate(["success", "failed", "running"]), Some("failed"));
        assert_eq!(aggregate(["success", "running", "pending"]), Some("running"));
        assert_eq!(aggregate(["success", "skipped"]), Some("success"));
        assert_eq!(aggregate(["skipped", "skipped"]), Some("skipped"));
        assert_eq!(aggregate(["success", "canceled"]), Some("canceled"));
        assert_eq!(aggregate(["success", "manual"]), Some("manual"));
        assert_eq!(aggregate(Vec::<&str>::new()), None);
    }

    #[test]
    fn runs_map_and_fill() {
        let mut r: Run = serde_json::from_value(json!({
            "id": 35770940606u64, "name": "CICD", "run_number": 812, "run_attempt": 1, "event": "pull_request",
            "status": "completed", "conclusion": "success", "workflow_id": 5, "head_branch": "docs/x",
            "head_sha": "60f44c03aa", "html_url": "https://github.com/o/r/actions/runs/1",
            "run_started_at": "2026-09-22T19:00:00Z", "updated_at": "2026-09-22T19:04:30Z",
            "head_commit": { "id": "60f44c03aa", "message": "Fix alias parsing\n\nLong body" },
            "pull_requests": [{ "number": 7, "head": {}, "base": {} }], "unknown": { "x": 1 }
        }))
        .unwrap();
        r.fill();
        assert_eq!(r.state, "success");
        assert_eq!(r.duration, Some(270.0));
        assert_eq!(r.commit_title.as_deref(), Some("Fix alias parsing"));
        let out = serde_json::to_value(&r).unwrap();
        assert_eq!(out["htmlUrl"], "https://github.com/o/r/actions/runs/1");
        assert_eq!(out["headBranch"], "docs/x");
        assert_eq!(out["pullRequests"][0]["number"], 7);
        assert!(out.get("html_url").is_none());
        let minimal: Run = serde_json::from_value(json!({ "id": 1 })).unwrap();
        assert_eq!(minimal.status, "");
    }

    #[test]
    fn jobs_fill_steps() {
        let mut j: Job = serde_json::from_value(json!({
            "id": 106892186679u64, "run_id": 1, "name": "build", "status": "completed", "conclusion": "failure",
            "started_at": "2026-09-22T19:00:30Z", "completed_at": "2026-09-22T19:00:40Z",
            "steps": [
                { "name": "Set up job", "status": "completed", "conclusion": "success", "number": 1 },
                { "name": "Test", "status": "completed", "conclusion": "failure", "number": 2 },
                { "name": "Post", "status": "completed", "conclusion": "skipped", "number": 3 }
            ]
        }))
        .unwrap();
        j.fill();
        assert_eq!(j.state, "failed");
        assert_eq!(j.duration, Some(10.0));
        let states: Vec<&str> = j.steps.iter().map(|s| s.state.as_str()).collect();
        assert_eq!(states, ["success", "failed", "skipped"]);
    }

    #[test]
    fn actions_urls_give_run_and_job_ids() {
        assert_eq!(actions_ids("https://github.com/o/r/actions/runs/123/job/456"), (Some(123), Some(456)));
        assert_eq!(actions_ids("https://github.com/o/r/actions/runs/123"), (Some(123), None));
        assert_eq!(actions_ids("https://ci.example.com/build/9"), (None, None));
    }

    #[test]
    fn review_comments_group_into_threads() {
        let c = |id: u64, reply: Option<u64>, line: Option<u64>| ReviewComment {
            id,
            in_reply_to_id: reply,
            path: "src/a.rs".into(),
            line,
            original_line: Some(3),
            body: format!("c{id}"),
            ..Default::default()
        };
        let threads = group_threads(vec![c(12, Some(10), Some(5)), c(10, None, Some(5)), c(11, None, None), c(13, Some(12), Some(5))]);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, "c10");
        assert_eq!(threads[0].comments.iter().map(|c| c.id).collect::<Vec<_>>(), [10, 12, 13]);
        assert!(!threads[0].outdated);
        assert!(threads[1].outdated);
    }

    #[test]
    fn review_states_keep_the_latest_decision() {
        let r = |login: &str, state: &str| Review {
            user: Some(User { login: login.into(), ..Default::default() }),
            state: state.into(),
            ..Default::default()
        };
        let s = review_states(&[r("a", "CHANGES_REQUESTED"), r("b", "COMMENTED"), r("a", "COMMENTED"), r("a", "APPROVED"), r("c", "PENDING")]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].state, "APPROVED");
        assert_eq!(s[1].state, "COMMENTED");
    }

    #[test]
    fn commits_flatten() {
        let c: RawCommit = serde_json::from_value(json!({
            "sha": "5babfd548d64a14eabeba53b847fdec5fa5f0ca9",
            "commit": { "message": "Subject\n\nBody", "author": { "name": "Dev", "email": "d@x", "date": "2026-09-01T00:00:00Z" } },
            "author": { "login": "dev", "id": 1 }, "html_url": "https://github.com/o/r/commit/5bab"
        }))
        .unwrap();
        let c = Commit::from(c);
        assert_eq!((c.short_sha.as_str(), c.title.as_str(), c.author_login.as_deref()), ("5babfd5", "Subject", Some("dev")));
        assert_eq!(diff_stats("@@ -1 +1,2 @@\n-a\n+b\n+c\n"), (2, 1));
    }
}

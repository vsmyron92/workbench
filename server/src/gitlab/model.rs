//! GitLab JSON shapes. Each struct deserializes GitLab's snake_case JSON (every
//! field optional through `#[serde(default)]`, so API additions and omissions
//! never break parsing) and serializes camelCase for the browser. Fields GitLab
//! does not send (`commit_title`, `kind`, `approvals`…) are filled in by `ops`.
//! TypeScript mirrors live in `web/src/features/gitlab/types.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One page of a list, as returned to the browser.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPage<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub next_page: Option<u32>,
    /// GitLab's `x-total` (absent for some endpoints and very large lists).
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct User {
    pub id: u64,
    pub username: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct DetailedStatus {
    pub text: String,
    pub label: String,
    pub group: String,
    pub icon: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Pipeline {
    pub id: u64,
    pub iid: Option<u64>,
    pub project_id: Option<u64>,
    pub status: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
    pub before_sha: Option<String>,
    pub tag: bool,
    pub source: Option<String>,
    pub name: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Seconds (GitLab sends an integer for pipelines, a float for jobs).
    pub duration: Option<f64>,
    pub queued_duration: Option<f64>,
    pub coverage: Option<Value>,
    pub web_url: String,
    pub user: Option<User>,
    pub detailed_status: Option<DetailedStatus>,
    pub yaml_errors: Option<String>,
    /// Commit subject, from the local repository when it has the commit.
    pub commit_title: Option<String>,
}

/// Pipeline reference embedded in jobs and bridges.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct PipelineRef {
    pub id: u64,
    pub iid: Option<u64>,
    pub project_id: Option<u64>,
    pub status: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
    pub web_url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CommitRef {
    pub id: String,
    pub short_id: String,
    pub title: String,
    pub author_name: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Runner {
    pub id: u64,
    pub description: String,
    pub name: Option<String>,
    pub is_shared: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Artifact {
    pub file_type: String,
    pub size: Option<u64>,
    pub filename: String,
    pub file_format: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct ArtifactsFile {
    pub filename: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub stage: String,
    pub status: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub tag: bool,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub erased_at: Option<String>,
    pub duration: Option<f64>,
    pub queued_duration: Option<f64>,
    pub allow_failure: bool,
    pub failure_reason: Option<String>,
    pub web_url: String,
    pub user: Option<User>,
    pub runner: Option<Runner>,
    pub artifacts: Vec<Artifact>,
    pub artifacts_file: Option<ArtifactsFile>,
    pub artifacts_expire_at: Option<String>,
    pub pipeline: Option<PipelineRef>,
    pub commit: Option<CommitRef>,
    pub coverage: Option<Value>,
    pub tag_list: Vec<String>,
    pub archived: bool,
    /// `job`, or `bridge` for trigger jobs (filled in by `ops`).
    pub kind: String,
    /// Bridges only: the pipeline they triggered.
    pub downstream_pipeline: Option<PipelineRef>,
}

impl Job {
    /// Whether the job has a downloadable artifacts archive (not just its log).
    pub fn has_archive(&self) -> bool {
        self.artifacts_file.is_some() || self.artifacts.iter().any(|a| a.file_type == "archive")
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct TestTotals {
    pub time: f64,
    pub count: u64,
    pub success: u64,
    pub failed: u64,
    pub skipped: u64,
    pub error: u64,
    pub suite_error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct TestSuite {
    pub name: String,
    pub total_time: f64,
    pub total_count: u64,
    pub success_count: u64,
    pub failed_count: u64,
    pub skipped_count: u64,
    pub error_count: u64,
    pub suite_error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct TestSummary {
    pub total: TestTotals,
    pub test_suites: Vec<TestSuite>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub name: String,
    pub status: String,
    pub jobs: Vec<Job>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineDetail {
    pub pipeline: Pipeline,
    pub stages: Vec<Stage>,
    pub test_summary: Option<TestSummary>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct DiffRefs {
    pub base_sha: String,
    pub head_sha: String,
    pub start_sha: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct References {
    pub short: String,
    pub full: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct MrUserPermissions {
    pub can_merge: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct ApprovedBy {
    pub user: User,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Approvals {
    pub approved: bool,
    pub approvals_required: u64,
    pub approvals_left: u64,
    pub approved_by: Vec<ApprovedBy>,
    pub user_can_approve: bool,
    pub user_has_approved: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Mr {
    pub id: u64,
    pub iid: u64,
    pub project_id: Option<u64>,
    pub title: String,
    pub description: Option<String>,
    pub state: String,
    pub draft: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub author: Option<User>,
    pub assignees: Vec<User>,
    pub reviewers: Vec<User>,
    pub labels: Vec<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
    pub merge_user: Option<User>,
    pub web_url: String,
    pub sha: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub squash_commit_sha: Option<String>,
    pub detailed_merge_status: Option<String>,
    pub has_conflicts: bool,
    pub user_notes_count: u64,
    pub upvotes: u64,
    pub downvotes: u64,
    pub references: Option<References>,
    pub squash: bool,
    pub squash_on_merge: Option<bool>,
    pub force_remove_source_branch: Option<bool>,
    pub should_remove_source_branch: Option<bool>,
    pub merge_when_pipeline_succeeds: bool,
    pub blocking_discussions_resolved: Option<bool>,
    pub discussion_locked: Option<bool>,
    // Detail-only fields.
    pub diff_refs: Option<DiffRefs>,
    /// GitLab sends a string ("10", or "1000+").
    pub changes_count: Option<String>,
    pub head_pipeline: Option<Pipeline>,
    pub merge_error: Option<String>,
    pub diverged_commits_count: Option<u64>,
    pub rebase_in_progress: Option<bool>,
    pub user: Option<MrUserPermissions>,
    /// Filled from `/approvals` for the detail view.
    pub approvals: Option<Approvals>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct MrDiffFile {
    pub old_path: String,
    pub new_path: String,
    pub a_mode: Option<String>,
    pub b_mode: Option<String>,
    pub new_file: bool,
    pub renamed_file: bool,
    pub deleted_file: bool,
    pub generated_file: Option<bool>,
    pub too_large: Option<bool>,
    pub collapsed: Option<bool>,
    pub diff: String,
    /// Counted from `diff` by `ops`.
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub title: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_date: Option<String>,
    pub committed_date: Option<String>,
    pub web_url: String,
    pub parent_ids: Vec<String>,
    pub last_pipeline: Option<PipelineRef>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Position {
    pub base_sha: Option<String>,
    pub start_sha: Option<String>,
    pub head_sha: Option<String>,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub position_type: Option<String>,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Note {
    pub id: u64,
    #[serde(rename = "type")]
    pub note_type: Option<String>,
    pub body: String,
    pub author: Option<User>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub system: bool,
    pub resolvable: bool,
    pub resolved: bool,
    pub resolved_by: Option<User>,
    pub position: Option<Position>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Discussion {
    pub id: String,
    pub individual_note: bool,
    pub notes: Vec<Note>,
}

impl Discussion {
    pub fn resolvable(&self) -> bool {
        self.notes.iter().any(|n| n.resolvable)
    }
    pub fn resolved(&self) -> bool {
        self.resolvable() && self.notes.iter().filter(|n| n.resolvable).all(|n| n.resolved)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Milestone {
    pub id: u64,
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Issue {
    pub id: u64,
    pub iid: u64,
    pub title: String,
    pub description: Option<String>,
    pub state: String,
    pub labels: Vec<String>,
    pub assignees: Vec<User>,
    pub author: Option<User>,
    pub milestone: Option<Milestone>,
    pub due_date: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    pub web_url: String,
    pub user_notes_count: u64,
    pub references: Option<References>,
    pub confidential: bool,
    pub issue_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Deployable {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub stage: String,
    pub pipeline: Option<PipelineRef>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct EnvironmentRef {
    pub id: u64,
    pub name: String,
    pub tier: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Deployment {
    pub id: u64,
    pub iid: Option<u64>,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
    pub status: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub finished_at: Option<String>,
    pub user: Option<User>,
    pub deployable: Option<Deployable>,
    pub environment: Option<EnvironmentRef>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct Environment {
    pub id: u64,
    pub name: String,
    pub slug: String,
    pub state: String,
    pub tier: Option<String>,
    pub external_url: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub auto_stop_at: Option<String>,
    pub description: Option<String>,
    pub last_deployment: Option<Deployment>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct RegistryRepo {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub location: String,
    pub created_at: Option<String>,
    pub tags_count: Option<u64>,
    pub status: Option<String>,
}

/// A container image tag. The list comes from GraphQL (newest first) with a
/// REST fallback (alphabetical, no dates); tag detail comes from REST.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct RegistryTag {
    pub name: String,
    pub path: Option<String>,
    pub location: String,
    pub digest: Option<String>,
    pub created_at: Option<String>,
    pub published_at: Option<String>,
    pub total_size: Option<u64>,
    pub revision: Option<String>,
}

/// Pipeline statuses that can still change.
pub fn is_active_status(s: &str) -> bool {
    matches!(
        s,
        "created" | "waiting_for_resource" | "preparing" | "pending" | "running" | "scheduled" | "waiting_for_callback"
    )
}

/// Job statuses after which the log no longer grows.
pub fn is_finished_status(s: &str) -> bool {
    matches!(s, "success" | "failed" | "canceled" | "skipped" | "manual")
}

/// Aggregate job statuses into a stage status, the way GitLab shows stages:
/// running beats pending, a (non-allowed) failure beats success, manual and
/// skipped only count when nothing else ran.
pub fn stage_status(jobs: &[Job]) -> String {
    let has = |s: &str| jobs.iter().any(|j| j.status == s);
    let hard_fail = jobs.iter().any(|j| j.status == "failed" && !j.allow_failure);
    let soft_fail = jobs.iter().any(|j| j.status == "failed" && j.allow_failure);
    let s = if has("running") {
        "running"
    } else if hard_fail {
        "failed"
    } else if has("pending") || has("preparing") || has("waiting_for_resource") {
        "pending"
    } else if has("created") && !jobs.iter().any(|j| j.status == "success") {
        "created"
    } else if has("canceled") {
        "canceled"
    } else if has("success") || soft_fail {
        if soft_fail { "success_with_warnings" } else { "success" }
    } else if has("manual") {
        "manual"
    } else if has("scheduled") {
        "scheduled"
    } else if has("skipped") {
        "skipped"
    } else if has("created") {
        "created"
    } else {
        jobs.first().map(|j| j.status.as_str()).unwrap_or("created")
    };
    s.to_string()
}

/// Group jobs into stages in pipeline order. GitLab creates jobs stage by stage,
/// so a stage's smallest job id orders the stages; jobs sort by name within one.
pub fn group_stages(mut jobs: Vec<Job>) -> Vec<Stage> {
    let mut order: Vec<(String, u64)> = vec![];
    for j in &jobs {
        match order.iter_mut().find(|(n, _)| *n == j.stage) {
            Some((_, min)) => *min = (*min).min(j.id),
            None => order.push((j.stage.clone(), j.id)),
        }
    }
    order.sort_by_key(|(_, min)| *min);
    jobs.sort_by(|a, b| natural_key(&a.name).cmp(&natural_key(&b.name)).then(a.id.cmp(&b.id)));
    order
        .into_iter()
        .map(|(name, _)| {
            let stage_jobs: Vec<Job> = jobs.iter().filter(|j| j.stage == name).cloned().collect();
            Stage { status: stage_status(&stage_jobs), name, jobs: stage_jobs }
        })
        .collect()
}

/// Sort key so `test 2/10` comes before `test 10/10`.
fn natural_key(s: &str) -> Vec<(String, u64)> {
    let mut out = vec![];
    let mut text = String::new();
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                num.clear();
            }
            text.push(c.to_ascii_lowercase());
        }
    }
    out.push((text, num.parse().unwrap_or(0)));
    out
}

/// Count `+`/`-` lines of a unified diff hunk body (headers excluded).
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

    fn job(id: u64, name: &str, stage: &str, status: &str) -> Job {
        Job { id, name: name.into(), stage: stage.into(), status: status.into(), ..Default::default() }
    }

    #[test]
    fn pipeline_maps_snake_to_camel_and_tolerates_gaps() {
        let p: Pipeline = serde_json::from_value(json!({
            "id": 2885126153u64, "iid": 831, "project_id": 85344789, "sha": "4b8e25089d41e6a3e1357adfa6731cd03a3198b4",
            "ref": "main", "status": "success", "source": "push", "created_at": "2026-09-26T10:04:59.178Z",
            "updated_at": "2026-09-26T10:10:46.088Z", "web_url": "https://gitlab.com/x/-/pipelines/1", "name": null,
            "duration": 343, "coverage": null, "detailed_status": {"icon": "status_success", "text": "Passed", "label": "passed", "group": "success", "tooltip": "passed"},
            "unknown_future_field": {"a": 1}
        }))
        .unwrap();
        assert_eq!(p.duration, Some(343.0));
        let out = serde_json::to_value(&p).unwrap();
        assert_eq!(out["ref"], "main");
        assert_eq!(out["webUrl"], "https://gitlab.com/x/-/pipelines/1");
        assert_eq!(out["detailedStatus"]["group"], "success");
        assert_eq!(out["projectId"], 85344789);
        assert!(out.get("web_url").is_none());
        // A minimal object still parses.
        let p: Pipeline = serde_json::from_value(json!({"id": 1})).unwrap();
        assert_eq!(p.status, "");
    }

    #[test]
    fn mr_detail_fields_map() {
        let m: Mr = serde_json::from_value(json!({
            "id": 532927942, "iid": 1, "title": "T", "state": "merged", "draft": false,
            "source_branch": "feature", "target_branch": "main", "changes_count": "10",
            "diff_refs": {"base_sha": "a", "head_sha": "b", "start_sha": "a"},
            "head_pipeline": {"id": 5, "status": "success", "ref": "feature", "sha": "b", "duration": 714},
            "user": {"can_merge": true}, "references": {"short": "!1", "full": "x/y!1"},
            "labels": ["bug"], "merge_user": {"id": 1, "username": "u", "name": "U", "avatar_url": "http://x"}
        }))
        .unwrap();
        let out = serde_json::to_value(&m).unwrap();
        assert_eq!(out["diffRefs"]["baseSha"], "a");
        assert_eq!(out["changesCount"], "10");
        assert_eq!(out["headPipeline"]["ref"], "feature");
        assert_eq!(out["user"]["canMerge"], true);
        assert_eq!(out["references"]["short"], "!1");
        assert!(out["mergeUser"].get("avatarUrl").is_none());
    }

    #[test]
    fn discussion_resolution_state() {
        let d: Discussion = serde_json::from_value(json!({
            "id": "abc", "individual_note": false,
            "notes": [
                {"id": 1, "type": "DiffNote", "body": "x", "resolvable": true, "resolved": true,
                 "position": {"new_path": "a.rs", "new_line": 3, "old_line": null}},
                {"id": 2, "body": "y", "resolvable": true, "resolved": false}
            ]
        }))
        .unwrap();
        assert!(d.resolvable());
        assert!(!d.resolved());
        let out = serde_json::to_value(&d).unwrap();
        assert_eq!(out["notes"][0]["type"], "DiffNote");
        assert_eq!(out["notes"][0]["position"]["newLine"], 3);
        assert_eq!(out["individualNote"], false);
    }

    #[test]
    fn stages_follow_creation_order_and_aggregate_status() {
        let jobs = vec![
            job(12, "docker-image", "package", "manual"),
            job(11, "web-build", "test", "success"),
            job(10, "server-tests", "test", "failed"),
        ];
        let stages = group_stages(jobs);
        assert_eq!(stages.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["test", "package"]);
        assert_eq!(stages[0].jobs[0].name, "server-tests");
        assert_eq!(stages[0].status, "failed");
        assert_eq!(stages[1].status, "manual");
    }

    #[test]
    fn stage_status_rules() {
        let mut j = job(1, "a", "s", "failed");
        j.allow_failure = true;
        assert_eq!(stage_status(&[j.clone(), job(2, "b", "s", "success")]), "success_with_warnings");
        assert_eq!(stage_status(&[job(1, "a", "s", "running"), job(2, "b", "s", "failed")]), "running");
        assert_eq!(stage_status(&[job(1, "a", "s", "pending"), job(2, "b", "s", "success")]), "pending");
        assert_eq!(stage_status(&[job(1, "a", "s", "skipped")]), "skipped");
        assert_eq!(stage_status(&[job(1, "a", "s", "canceled"), job(2, "b", "s", "success")]), "canceled");
        assert_eq!(stage_status(&[job(1, "a", "s", "created"), job(2, "b", "s", "created")]), "created");
    }

    #[test]
    fn natural_job_order() {
        let jobs = vec![job(3, "test 10/10", "t", "success"), job(2, "test 2/10", "t", "success"), job(1, "Lint", "t", "success")];
        let stages = group_stages(jobs);
        let names: Vec<&str> = stages[0].jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["Lint", "test 2/10", "test 10/10"]);
    }

    #[test]
    fn diff_stats_ignore_headers() {
        let d = "--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@\n a\n-b\n+c\n+d\n";
        assert_eq!(diff_stats(d), (2, 1));
    }

    #[test]
    fn status_classes() {
        assert!(is_active_status("running"));
        assert!(!is_active_status("success"));
        assert!(is_finished_status("manual"));
        assert!(!is_finished_status("pending"));
    }
}

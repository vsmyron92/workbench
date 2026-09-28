//! MCP tools for hosted Claude sessions: read CI state and logs, review merge
//! requests, and (mutating) create MRs, comment and retry jobs. The project is
//! always the calling session's; only a non-session caller (the master token
//! without a terminal) may pick another with `projectId`.

use std::fmt::Write as _;

use serde_json::{Value, json};

use super::client::{self, GlCtx};
use super::model::{self, Job, Pipeline};
use super::{mrs, pipelines};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

/// Diff text an agent gets at most from `gitlab_mr_diff`.
const MAX_DIFF_OUTPUT: usize = 200 * 1024;

fn s(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

fn n(args: &Value, k: &str) -> Option<u64> {
    match args.get(k)? {
        Value::Number(x) => x.as_u64(),
        Value::String(v) => v.trim().trim_start_matches(['#', '!']).parse().ok(),
        _ => None,
    }
}

fn b(args: &Value, k: &str) -> Option<bool> {
    args.get(k).and_then(Value::as_bool)
}

fn need(args: &Value, k: &str) -> ApiResult<u64> {
    n(args, k).ok_or_else(|| ApiError::bad_request(format!("{k} (a number) is required")))
}

/// The GitLab project of the calling session. A session is confined to its own
/// Workbench project (`McpCtx::project_for`): an agent of one project cannot read
/// or change another project's merge requests or CI, whatever `projectId` says.
async fn tool_ctx(state: &AppState, mctx: &McpCtx, args: &Value) -> ApiResult<GlCtx> {
    let pid = mctx.project_for(s(args, "projectId").as_deref())?;
    client::ctx(state, &pid).await
}

fn project_prop() -> Value {
    json!({ "type": "string", "description": "Workbench project id. Defaults to, and for a hosted session must be, the calling session's project." })
}

pub fn fmt_duration(secs: Option<f64>) -> String {
    let Some(s) = secs else { return "-".into() };
    let s = s.round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn when(ts: &Option<String>) -> String {
    ts.as_deref().map(|t| t.get(..16).unwrap_or(t).replace('T', " ")).unwrap_or_default()
}

fn pipeline_line(p: &Pipeline) -> String {
    format!(
        "#{}  id {}  {}  {}  {}  {}  {}  {}{}",
        p.iid.map(|i| i.to_string()).unwrap_or_else(|| "?".into()),
        p.id,
        p.status,
        p.git_ref,
        short(&p.sha),
        p.source.as_deref().unwrap_or("-"),
        fmt_duration(p.duration),
        when(&p.created_at),
        p.commit_title.as_deref().map(|t| format!("  \"{t}\"")).unwrap_or_default()
    )
}

fn job_line(j: &Job) -> String {
    let mut line = format!("  {}  id {}  {}  {}", j.name, j.id, j.status, fmt_duration(j.duration));
    if let Some(r) = &j.failure_reason {
        let _ = write!(line, "  ({r})");
    }
    if j.allow_failure {
        line.push_str("  [allowed to fail]");
    }
    if j.kind == "bridge" {
        if let Some(d) = &j.downstream_pipeline {
            let _ = write!(line, "  → downstream pipeline {} {}", d.id, d.status);
        }
    }
    line
}

fn excerpt(text: &str, max: usize) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max { one.chars().take(max).collect::<String>() + "…" } else { one }
}

pub fn mcp_tools() -> Vec<McpTool> {
    vec![
        tool(
            "gitlab_pipelines",
            "List recent GitLab CI pipelines of the project (newest first) with status, ref, commit, duration.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "ref": { "type": "string", "description": "Branch or tag to filter by." },
                "status": { "type": "string", "description": "running | pending | success | failed | canceled | skipped | manual" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 15 }
            }}),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let q = pipelines::PipelinesQuery {
                    git_ref: s(&args, "ref"),
                    status: s(&args, "status"),
                    per_page: Some(n(&args, "limit").unwrap_or(15).clamp(1, 50) as u32),
                    ..Default::default()
                };
                let page = pipelines::list_pipelines(&ctx, &q).await?;
                let mut out = format!(
                    "{} — {} pipeline(s){}{}\n",
                    ctx.meta.path_with_namespace,
                    page.items.len(),
                    page.total.map(|t| format!(" of {t}")).unwrap_or_default(),
                    q.git_ref.as_deref().map(|r| format!(" on {r}")).unwrap_or_default()
                );
                for p in &page.items {
                    out.push_str(&pipeline_line(p));
                    out.push('\n');
                }
                out.push_str("\nJobs of a pipeline: gitlab_pipeline_jobs {\"pipelineId\": <id>}");
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_pipeline_jobs",
            "Jobs of one pipeline grouped by stage, with status, duration, failure reason and the test report summary.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "pipelineId": { "type": "integer", "description": "Pipeline id (not the #iid)." }
            }, "required": ["pipelineId"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let d = pipelines::pipeline_detail(&ctx, need(&args, "pipelineId")?).await?;
                let p = &d.pipeline;
                let mut out = format!(
                    "Pipeline #{} (id {}): {} on {} @ {} — {}\n",
                    p.iid.unwrap_or(0),
                    p.id,
                    p.status,
                    p.git_ref,
                    short(&p.sha),
                    p.web_url
                );
                if let Some(e) = &p.yaml_errors {
                    let _ = writeln!(out, "YAML errors: {e}");
                }
                if let Some(t) = &d.test_summary {
                    let _ = writeln!(
                        out,
                        "Tests: {} total, {} failed, {} errors, {} skipped",
                        t.total.count, t.total.failed, t.total.error, t.total.skipped
                    );
                }
                for st in &d.stages {
                    let _ = writeln!(out, "\nStage {} [{}]", st.name, st.status);
                    for j in &st.jobs {
                        out.push_str(&job_line(j));
                        out.push('\n');
                    }
                }
                out.push_str("\nA job's log: gitlab_job_log {\"jobId\": <id>}");
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_test_failures",
            "The failed and errored tests of a pipeline from its JUnit test report: suite, test, file, and the failure output (message and stack trace).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "pipelineId": { "type": "integer", "description": "Pipeline id (not the #iid)." }
            }, "required": ["pipelineId"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let id = need(&args, "pipelineId")?;
                let Some(f) = pipelines::pipeline_test_failures(&ctx, id).await? else {
                    return Ok(ToolOutput::Text(format!("Pipeline {id} has no test report (its jobs publish no `artifacts:reports:junit`).")));
                };
                let t = &f.total;
                let mut out = format!("Tests: {} total, {} failed, {} errors, {} skipped\n", t.count, t.failed, t.error, t.skipped);
                for s in &f.suites {
                    if s.cases.is_empty() && s.suite_error.is_none() {
                        continue;
                    }
                    let _ = writeln!(out, "\n## {} ({} failed, {} errors of {})", s.name, s.failed_count, s.error_count, s.total_count);
                    if let Some(e) = &s.suite_error {
                        let _ = writeln!(out, "Suite error: {e}");
                    }
                    for c in &s.cases {
                        let _ = writeln!(out, "\n### {} {}{}", c.status.to_uppercase(), if c.classname.is_empty() { String::new() } else { format!("{} › ", c.classname) }, c.name);
                        if let Some(file) = &c.file {
                            let _ = writeln!(out, "File: {file}");
                        }
                        if let Some(n) = c.recent_failures {
                            let _ = writeln!(out, "Failed {n} times recently on {}", c.base_branch.as_deref().unwrap_or("the base branch"));
                        }
                        let text: String = c.output.chars().take(4000).collect();
                        if !text.is_empty() {
                            let _ = writeln!(out, "```\n{text}{}\n```", if text.len() < c.output.len() { "\n…" } else { "" });
                        }
                    }
                }
                if f.truncated {
                    out.push_str("\nMore failures exist than are listed here.");
                }
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_job_log",
            "The end of a CI job's log as plain text (ANSI colours and runner timestamps removed).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "jobId": { "type": "integer" },
                "tailLines": { "type": "integer", "minimum": 1, "maximum": 2000, "default": 200 }
            }, "required": ["jobId"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let id = need(&args, "jobId")?;
                let lines = n(&args, "tailLines").unwrap_or(200).clamp(1, 2000) as usize;
                let (job, text, total, truncated) = pipelines::log_tail(&ctx, id, lines, true).await?;
                let shown = text.lines().count();
                let mut out = format!("Job {} \"{}\" (stage {}): {}", job.id, job.name, job.stage, job.status);
                if let Some(r) = &job.failure_reason {
                    let _ = write!(out, " ({r})");
                }
                if let Some(p) = &job.pipeline {
                    let _ = write!(out, ", pipeline {} on {} @ {}", p.id, p.git_ref, short(&p.sha));
                }
                let _ = writeln!(
                    out,
                    "\nLast {shown} of {total} lines{}:\n",
                    if truncated { " (the log is larger; its start was not read)" } else { "" }
                );
                out.push_str(&text);
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_mrs",
            "List the project's merge requests (newest activity first).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "state": { "type": "string", "enum": ["opened", "merged", "closed", "all"], "default": "opened" },
                "search": { "type": "string" }
            }}),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let q = mrs::MrsQuery {
                    state: s(&args, "state"),
                    search: s(&args, "search"),
                    per_page: Some(30),
                    ..Default::default()
                };
                let page = mrs::list_mrs(&ctx, &q).await?;
                let mut out = format!(
                    "Merge requests ({}): {}{}\n",
                    q.state.as_deref().unwrap_or("opened"),
                    page.items.len(),
                    page.total.map(|t| format!(" of {t}")).unwrap_or_default()
                );
                for m in &page.items {
                    let _ = writeln!(
                        out,
                        "!{}  {}{}  {} → {}  \"{}\"  by {}  updated {}  [{}]",
                        m.iid,
                        m.state,
                        if m.draft { " (draft)" } else { "" },
                        m.source_branch,
                        m.target_branch,
                        m.title,
                        m.author.as_ref().map(|a| a.username.as_str()).unwrap_or("?"),
                        when(&m.updated_at),
                        m.detailed_merge_status.as_deref().unwrap_or("-")
                    );
                }
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_mr",
            "One merge request: status, branches, pipeline, approvals, description and a summary of its discussion threads.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "iid": { "type": "integer", "description": "The MR number (!iid)." }
            }, "required": ["iid"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let iid = need(&args, "iid")?;
                let (mr, discussions) = tokio::join!(mrs::get_mr(&ctx, iid), mrs::mr_discussions(&ctx, iid));
                let mr = mr?;
                let discussions = discussions.unwrap_or_default();
                let mut out = format!("!{} {}\n{}\n", mr.iid, mr.title, mr.web_url);
                let _ = writeln!(
                    out,
                    "State: {}{} · {} → {} · author {}",
                    mr.state,
                    if mr.draft { " (draft)" } else { "" },
                    mr.source_branch,
                    mr.target_branch,
                    mr.author.as_ref().map(|a| a.username.as_str()).unwrap_or("?")
                );
                let _ = writeln!(
                    out,
                    "Merge status: {} · conflicts: {} · changed files: {} · head {}",
                    mr.detailed_merge_status.as_deref().unwrap_or("-"),
                    if mr.has_conflicts { "yes" } else { "no" },
                    mr.changes_count.as_deref().unwrap_or("?"),
                    mr.sha.as_deref().map(short).unwrap_or("?")
                );
                if let Some(p) = &mr.head_pipeline {
                    let _ = writeln!(out, "Pipeline: id {} {} ({})", p.id, p.status, fmt_duration(p.duration));
                }
                if let Some(a) = &mr.approvals {
                    let by: Vec<&str> = a.approved_by.iter().map(|u| u.user.username.as_str()).collect();
                    let _ = writeln!(
                        out,
                        "Approvals: {} ({} required, {} left){}",
                        if a.approved { "approved" } else { "not approved" },
                        a.approvals_required,
                        a.approvals_left,
                        if by.is_empty() { String::new() } else { format!(", by {}", by.join(", ")) }
                    );
                }
                let desc = mr.description.as_deref().unwrap_or("").trim();
                if !desc.is_empty() {
                    let d: String = desc.chars().take(6000).collect();
                    let _ = writeln!(out, "\nDescription:\n{d}{}", if desc.chars().count() > 6000 { "\n…" } else { "" });
                }
                let threads: Vec<&model::Discussion> =
                    discussions.iter().filter(|d| d.notes.iter().any(|n| !n.system)).collect();
                let unresolved = threads.iter().filter(|d| d.resolvable() && !d.resolved()).count();
                let _ = writeln!(out, "\nDiscussions: {} thread(s), {} unresolved", threads.len(), unresolved);
                for d in threads.iter().take(60) {
                    let Some(first) = d.notes.first() else { continue };
                    let place = first
                        .position
                        .as_ref()
                        .map(|p| {
                            format!(
                                " {}:{}",
                                p.new_path.as_deref().or(p.old_path.as_deref()).unwrap_or("?"),
                                p.new_line.or(p.old_line).map(|l| l.to_string()).unwrap_or_default()
                            )
                        })
                        .unwrap_or_default();
                    let state = if !d.resolvable() { "note" } else if d.resolved() { "resolved" } else { "unresolved" };
                    let _ = writeln!(
                        out,
                        "- [{state}]{place} {}: \"{}\"{} (discussion {})",
                        first.author.as_ref().map(|a| a.username.as_str()).unwrap_or("?"),
                        excerpt(&first.body, 240),
                        if d.notes.len() > 1 { format!(" +{} repl.", d.notes.len() - 1) } else { String::new() },
                        d.id
                    );
                }
                out.push_str("\nThe diff: gitlab_mr_diff {\"iid\": N, \"path\"?: \"file\"}");
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_mr_diff",
            "The unified diff of a merge request (all files, or one file with `path`). Large diffs are cut at 200 KB.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "iid": { "type": "integer" },
                "path": { "type": "string", "description": "Only this file (old or new path)." }
            }, "required": ["iid"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let iid = need(&args, "iid")?;
                let path = s(&args, "path");
                let diffs = mrs::mr_diffs(&ctx, iid).await?;
                let files: Vec<&model::MrDiffFile> = diffs
                    .files
                    .iter()
                    .filter(|f| path.as_deref().is_none_or(|p| f.new_path == p || f.old_path == p))
                    .collect();
                if files.is_empty() {
                    return Err(ApiError::not_found(match path {
                        Some(p) => format!("!{iid} does not change {p}"),
                        None => format!("!{iid} has no changes"),
                    }));
                }
                let mut out = String::new();
                let mut cut = false;
                for f in files {
                    let header = format!("diff --git a/{} b/{}\n--- {}\n+++ {}\n", f.old_path, f.new_path,
                        if f.new_file { "/dev/null".to_string() } else { format!("a/{}", f.old_path) },
                        if f.deleted_file { "/dev/null".to_string() } else { format!("b/{}", f.new_path) });
                    let body = if f.diff.is_empty() && f.too_large == Some(true) {
                        "(diff too large to show)\n".to_string()
                    } else {
                        f.diff.clone()
                    };
                    if out.len() + header.len() + body.len() > MAX_DIFF_OUTPUT {
                        cut = true;
                        break;
                    }
                    out.push_str(&header);
                    out.push_str(&body);
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                if cut || diffs.truncated {
                    out.push_str("\n[diff truncated; pass `path` to see a single file]\n");
                }
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "gitlab_create_mr",
            "Create a merge request (source defaults to the project's current branch, target to the default branch). The branch must already be pushed. Opens the MR in Workbench.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "title": { "type": "string" },
                "description": { "type": "string", "description": "Markdown." },
                "sourceBranch": { "type": "string" },
                "targetBranch": { "type": "string" },
                "draft": { "type": "boolean", "default": false },
                "removeSourceBranch": { "type": "boolean" },
                "squash": { "type": "boolean" }
            }, "required": ["title"] }),
            true,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let body = mrs::CreateMr {
                    source_branch: s(&args, "sourceBranch"),
                    target_branch: s(&args, "targetBranch"),
                    title: s(&args, "title").unwrap_or_default(),
                    description: args.get("description").and_then(Value::as_str).map(str::to_string),
                    draft: b(&args, "draft").unwrap_or(false),
                    remove_source_branch: b(&args, "removeSourceBranch"),
                    squash: b(&args, "squash"),
                    ..Default::default()
                };
                let mr = mrs::create_mr(&ctx, &body).await?;
                state.events.ui_open(
                    "mr",
                    json!({ "projectId": ctx.project.id, "iid": mr.iid }),
                    Some(&format!("!{} {}", mr.iid, mr.title)),
                );
                Ok(ToolOutput::Text(format!(
                    "Created !{}: {} ({} → {})\n{}",
                    mr.iid, mr.title, mr.source_branch, mr.target_branch, mr.web_url
                )))
            },
        ),
        tool(
            "gitlab_mr_comment",
            "Comment on a merge request: a general note, a reply to a discussion (`discussionId`), or a diff-line comment (`path` plus `line` for the new side or `oldLine` for a removed line).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "iid": { "type": "integer" },
                "body": { "type": "string", "description": "Markdown." },
                "discussionId": { "type": "string" },
                "path": { "type": "string" },
                "line": { "type": "integer", "description": "Line in the new version of the file." },
                "oldLine": { "type": "integer", "description": "Line in the old version (removed lines, or with `line` for unchanged ones)." }
            }, "required": ["iid", "body"] }),
            true,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let iid = need(&args, "iid")?;
                let body = args.get("body").and_then(Value::as_str).unwrap_or_default().to_string();
                if let Some(did) = s(&args, "discussionId") {
                    let note = mrs::reply(&ctx, iid, &did, &body).await?;
                    return Ok(ToolOutput::Text(format!("Replied in discussion {did} on !{iid} (note {})", note.id)));
                }
                if let Some(path) = s(&args, "path") {
                    let pos = mrs::LinePosition {
                        old_path: None,
                        new_path: path.clone(),
                        new_line: n(&args, "line"),
                        old_line: n(&args, "oldLine"),
                    };
                    let d = mrs::add_discussion(&ctx, iid, &mrs::NewDiscussion { body, position: Some(pos) }).await?;
                    return Ok(ToolOutput::Text(format!("Commented on {path} in !{iid} (discussion {})", d.id)));
                }
                let note = mrs::add_note(&ctx, iid, &body).await?;
                Ok(ToolOutput::Text(format!("Commented on !{iid} (note {})", note.id)))
            },
        ),
        tool(
            "gitlab_retry_job",
            "Retry a CI job (creates a new job in the same pipeline).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "jobId": { "type": "integer" }
            }, "required": ["jobId"] }),
            true,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let id = need(&args, "jobId")?;
                let j = pipelines::job_action(&ctx, id, "retry", &[]).await?;
                Ok(ToolOutput::Text(format!(
                    "Retried job {id} \"{}\": new job {} is {}{}",
                    j.name,
                    j.id,
                    j.status,
                    j.pipeline.as_ref().map(|p| format!(" (pipeline {})", p.id)).unwrap_or_default()
                )))
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_format_compactly() {
        assert_eq!(fmt_duration(None), "-");
        assert_eq!(fmt_duration(Some(43.4)), "43s");
        assert_eq!(fmt_duration(Some(297.7)), "4m 58s");
        assert_eq!(fmt_duration(Some(7322.0)), "2h 2m");
    }

    #[test]
    fn args_accept_numbers_and_strings() {
        let a = json!({ "iid": "!12", "jobId": 5, "x": "  " });
        assert_eq!(n(&a, "iid"), Some(12));
        assert_eq!(n(&a, "jobId"), Some(5));
        assert_eq!(s(&a, "x"), None);
        assert!(need(&a, "missing").is_err());
    }

    #[test]
    fn tool_names_are_prefixed_and_mutations_marked() {
        let tools = mcp_tools();
        assert!(tools.iter().all(|t| t.name.starts_with("gitlab_")));
        let mutating: Vec<&str> = tools.iter().filter(|t| t.mutating).map(|t| t.name.as_str()).collect();
        assert_eq!(mutating, ["gitlab_create_mr", "gitlab_mr_comment", "gitlab_retry_job"]);
    }
}

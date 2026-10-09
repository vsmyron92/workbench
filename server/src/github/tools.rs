//! MCP tools for hosted agent sessions: read Actions runs, jobs and logs, review
//! pull requests, and (mutating) create pull requests, comment and re-run. The
//! project is always the calling session's; only a non-session caller (the
//! master token without a terminal) may pick another with `projectId`.

use std::fmt::Write as _;

use serde_json::{Value, json};

use super::client::{self, GhCtx};
use super::model::{Job, Run};
use super::{ci, pulls};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::forge::RepoParam;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

/// Diff text an agent gets at most from `github_pr_diff`.
const MAX_DIFF_OUTPUT: usize = 200 * 1024;

fn s(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

fn n(args: &Value, k: &str) -> Option<u64> {
    match args.get(k)? {
        Value::Number(x) => x.as_u64(),
        Value::String(v) => v.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    }
}

fn b(args: &Value, k: &str) -> Option<bool> {
    args.get(k).and_then(Value::as_bool)
}

fn need(args: &Value, k: &str) -> ApiResult<u64> {
    n(args, k).ok_or_else(|| ApiError::bad_request(format!("{k} (a number) is required")))
}

/// The GitHub repository of the calling session's project. A session is
/// confined to its own Workbench project (`McpCtx::project_for`).
async fn tool_ctx(state: &AppState, mctx: &McpCtx, args: &Value) -> ApiResult<GhCtx> {
    let pid = mctx.project_for(s(args, "projectId").as_deref())?;
    client::ctx(state, &pid, &RepoParam::named(s(args, "repo").as_deref())).await
}

fn repo_prop() -> Value {
    json!({ "type": "string", "description": "Repository id from the project's repositories (default repository when omitted). Repositories of one project can be on different GitLab/GitHub projects." })
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

fn run_line(r: &Run) -> String {
    format!(
        "run {}  #{}  {}  {}  {}  {}  {}  {}  {}{}",
        r.id,
        r.run_number,
        r.name.as_deref().unwrap_or("?"),
        r.state,
        r.head_branch.as_deref().unwrap_or("-"),
        short(&r.head_sha),
        r.event,
        fmt_duration(r.duration),
        when(&r.created_at),
        r.commit_title.as_deref().map(|t| format!("  \"{t}\"")).unwrap_or_default()
    )
}

fn job_line(j: &Job) -> String {
    let mut line = format!("  {}  job {}  {}  {}", j.name, j.id, j.state, fmt_duration(j.duration));
    let failed: Vec<&str> = j.steps.iter().filter(|s| s.state == "failed").map(|s| s.name.as_str()).collect();
    if !failed.is_empty() {
        let _ = write!(line, "  (failed step: {})", failed.join(", "));
    }
    line
}

fn excerpt(text: &str, max: usize) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max { one.chars().take(max).collect::<String>() + "…" } else { one }
}

fn login(u: &Option<super::model::User>) -> &str {
    u.as_ref().map(|u| u.login.as_str()).unwrap_or("?")
}

pub fn mcp_tools() -> Vec<McpTool> {
    vec![
        tool(
            "github_runs",
            "List recent GitHub Actions workflow runs of the project (newest first) with state, workflow, branch, commit, event and duration.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "branch": { "type": "string" },
                "status": { "type": "string", "description": "GitHub status or conclusion: in_progress | queued | completed | success | failure | cancelled" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 15 }
            }}),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let q = ci::RunsQuery {
                    branch: s(&args, "branch"),
                    status: s(&args, "status"),
                    per_page: Some(n(&args, "limit").unwrap_or(15).clamp(1, 50) as u32),
                    ..Default::default()
                };
                let page = ci::list_runs(&ctx, &q).await?;
                let mut out = format!(
                    "{} — {} run(s){}{}\n",
                    ctx.full_name(),
                    page.items.len(),
                    page.total.map(|t| format!(" of {t}")).unwrap_or_default(),
                    q.branch.as_deref().map(|r| format!(" on {r}")).unwrap_or_default()
                );
                for r in &page.items {
                    out.push_str(&run_line(r));
                    out.push('\n');
                }
                out.push_str("\nJobs of a run: github_run_jobs {\"runId\": <id>}");
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "github_run_jobs",
            "Jobs of one workflow run with their state, duration and the steps that failed.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "runId": { "type": "integer" }
            }, "required": ["runId"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let d = ci::run_detail(&ctx, need(&args, "runId")?).await?;
                let r = &d.run;
                let mut out = format!(
                    "Run {} (#{} of {}): {} on {} @ {} — {}\n",
                    r.id,
                    r.run_number,
                    r.name.as_deref().unwrap_or("?"),
                    r.state,
                    r.head_branch.as_deref().unwrap_or("-"),
                    short(&r.head_sha),
                    r.html_url
                );
                for j in &d.jobs {
                    out.push_str(&job_line(j));
                    out.push('\n');
                }
                if d.truncated {
                    out.push_str("(more jobs not shown)\n");
                }
                out.push_str("\nA job's log: github_job_log {\"jobId\": <id>}");
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "github_job_log",
            "The end of a GitHub Actions job's log as plain text (timestamps and ANSI colours removed). Logs exist once the job has finished.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "jobId": { "type": "integer" },
                "tailLines": { "type": "integer", "minimum": 1, "maximum": 2000, "default": 200 }
            }, "required": ["jobId"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let id = need(&args, "jobId")?;
                let lines = n(&args, "tailLines").unwrap_or(200).clamp(1, 2000) as usize;
                let (job, tail, message) = ci::log_tail(&ctx, id, lines, true).await?;
                let mut out = format!("Job {} \"{}\": {}", job.id, job.name, job.state);
                let failed: Vec<&str> = job.steps.iter().filter(|s| s.state == "failed").map(|s| s.name.as_str()).collect();
                if !failed.is_empty() {
                    let _ = write!(out, " (failed step: {})", failed.join(", "));
                }
                let _ = write!(out, ", run {} @ {}", job.run_id, short(&job.head_sha));
                match tail {
                    Some((text, total, truncated)) => {
                        let shown = text.lines().count();
                        let _ = writeln!(
                            out,
                            "\nLast {shown} of {total} lines{}:\n",
                            if truncated { " (the log is larger; its start was not read)" } else { "" }
                        );
                        out.push_str(&text);
                    }
                    None => {
                        let _ = writeln!(out, "\n{}", message.unwrap_or_else(|| "No log available.".into()));
                        if let Ok(a) = ci::annotations(&ctx, id).await {
                            for x in a.iter().take(50) {
                                let _ = writeln!(
                                    out,
                                    "{}: {}{} {}",
                                    x.annotation_level,
                                    x.path,
                                    x.start_line.map(|l| format!(":{l}")).unwrap_or_default(),
                                    excerpt(&x.message, 400)
                                );
                            }
                        }
                    }
                }
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "github_prs",
            "List the repository's pull requests (recently updated first).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "state": { "type": "string", "enum": ["open", "closed", "merged", "all"], "default": "open" },
                "search": { "type": "string" }
            }}),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let q = pulls::PullsQuery { state: s(&args, "state"), search: s(&args, "search"), per_page: Some(30), ..Default::default() };
                let page = pulls::list_pulls(&ctx, &q).await?;
                let mut out = format!("Pull requests ({}): {}\n", q.state.as_deref().unwrap_or("open"), page.items.len());
                for p in &page.items {
                    let state = if p.merged { "merged" } else { p.state.as_str() };
                    let _ = writeln!(
                        out,
                        "#{}  {}{}  {} → {}  \"{}\"  by {}  updated {}",
                        p.number,
                        state,
                        if p.draft { " (draft)" } else { "" },
                        p.head.as_ref().map(|h| h.git_ref.as_str()).unwrap_or("?"),
                        p.base.as_ref().map(|h| h.git_ref.as_str()).unwrap_or("?"),
                        p.title,
                        login(&p.user),
                        when(&p.updated_at)
                    );
                }
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "github_pr",
            "One pull request: state, branches, mergeability, checks, reviews, description and its review threads.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "number": { "type": "integer" }
            }, "required": ["number"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let num = need(&args, "number")?;
                let (d, threads) = tokio::join!(pulls::get_pull(&ctx, num), pulls::threads(&ctx, num));
                let d = d?;
                let threads = threads.unwrap_or_default();
                let p = &d.pull;
                let mut out = format!("#{} {}\n{}\n", p.number, p.title, p.html_url);
                let state = if p.merged { "merged" } else { p.state.as_str() };
                let _ = writeln!(
                    out,
                    "State: {state}{} · {} → {} · author {}",
                    if p.draft { " (draft)" } else { "" },
                    p.head.as_ref().map(|h| h.git_ref.as_str()).unwrap_or("?"),
                    p.base.as_ref().map(|h| h.git_ref.as_str()).unwrap_or("?"),
                    login(&p.user)
                );
                let _ = writeln!(
                    out,
                    "Mergeable: {} ({}) · {} files, +{} −{} · head {}",
                    match p.mergeable {
                        Some(true) => "yes",
                        Some(false) => "no",
                        None => "unknown",
                    },
                    p.mergeable_state.as_deref().unwrap_or("-"),
                    p.changed_files.unwrap_or(0),
                    p.additions.unwrap_or(0),
                    p.deletions.unwrap_or(0),
                    p.head.as_ref().map(|h| short(&h.sha)).unwrap_or("?")
                );
                if let Some(c) = &d.checks {
                    let _ = writeln!(out, "Checks: {}", c.state.as_deref().unwrap_or("none"));
                    for i in c.items.iter().filter(|i| i.state != "success" && i.state != "skipped").take(30) {
                        let _ = writeln!(
                            out,
                            "  - {} [{}]{}",
                            i.name,
                            i.state,
                            i.job_id.map(|j| format!(" job {j}")).unwrap_or_default()
                        );
                    }
                }
                if !d.review_states.is_empty() {
                    let r: Vec<String> =
                        d.review_states.iter().map(|r| format!("{} {}", login(&r.user), r.state.to_lowercase())).collect();
                    let _ = writeln!(out, "Reviews: {}", r.join(", "));
                }
                let body = p.body.as_deref().unwrap_or("").trim();
                if !body.is_empty() {
                    let t: String = body.chars().take(6000).collect();
                    let _ = writeln!(out, "\nDescription:\n{t}{}", if body.chars().count() > 6000 { "\n…" } else { "" });
                }
                let unresolved = threads.iter().filter(|t| t.resolved == Some(false)).count();
                let _ = writeln!(out, "\nReview threads: {} ({} unresolved)", threads.len(), unresolved);
                for t in threads.iter().take(60) {
                    let Some(first) = t.comments.first() else { continue };
                    let state = match t.resolved {
                        Some(true) => "resolved",
                        Some(false) => "unresolved",
                        None => "thread",
                    };
                    let _ = writeln!(
                        out,
                        "- [{state}{}] {}:{} {}: \"{}\"{} (comment {})",
                        if t.outdated { ", outdated" } else { "" },
                        t.path,
                        t.line.or(t.original_line).map(|l| l.to_string()).unwrap_or_default(),
                        login(&first.user),
                        excerpt(&first.body, 240),
                        if t.comments.len() > 1 { format!(" +{} repl.", t.comments.len() - 1) } else { String::new() },
                        t.root_id
                    );
                }
                out.push_str("\nThe diff: github_pr_diff {\"number\": N, \"path\"?: \"file\"}");
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "github_pr_diff",
            "The unified diff of a pull request (all files, or one file with `path`). Large diffs are cut at 200 KB.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "number": { "type": "integer" },
                "path": { "type": "string", "description": "Only this file (current or previous path)." }
            }, "required": ["number"] }),
            false,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let num = need(&args, "number")?;
                let path = s(&args, "path");
                let files = pulls::pr_files(&ctx, num).await?;
                let chosen: Vec<&super::model::PrFile> = files
                    .files
                    .iter()
                    .filter(|f| path.as_deref().is_none_or(|p| f.filename == p || f.previous_filename.as_deref() == Some(p)))
                    .collect();
                if chosen.is_empty() {
                    return Err(ApiError::not_found(match path {
                        Some(p) => format!("#{num} does not change {p}"),
                        None => format!("#{num} has no changes"),
                    }));
                }
                let mut out = String::new();
                let mut cut = false;
                for f in chosen {
                    let old = f.previous_filename.as_deref().unwrap_or(&f.filename);
                    let header = format!(
                        "diff --git a/{old} b/{}\n--- {}\n+++ {}\n",
                        f.filename,
                        if f.status == "added" { "/dev/null".to_string() } else { format!("a/{old}") },
                        if f.status == "removed" { "/dev/null".to_string() } else { format!("b/{}", f.filename) }
                    );
                    let body = match &f.patch {
                        Some(p) => p.clone(),
                        None if f.too_large => "(diff too large to show)\n".into(),
                        None => "(binary or empty change)\n".into(),
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
                if cut || files.truncated {
                    out.push_str("\n[diff truncated; pass `path` to see a single file]\n");
                }
                Ok(ToolOutput::Text(out))
            },
        ),
        tool(
            "github_create_pr",
            "Create a pull request (head defaults to the project's current branch, base to the default branch). The branch must already be pushed. Opens the pull request in Workbench.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "title": { "type": "string" },
                "body": { "type": "string", "description": "Markdown." },
                "head": { "type": "string" },
                "base": { "type": "string" },
                "draft": { "type": "boolean", "default": false }
            }, "required": ["title"] }),
            true,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let body = pulls::CreatePr {
                    title: s(&args, "title").unwrap_or_default(),
                    body: args.get("body").and_then(Value::as_str).map(str::to_string),
                    head: s(&args, "head"),
                    base: s(&args, "base"),
                    draft: b(&args, "draft").unwrap_or(false),
                };
                let p = pulls::create_pr(&ctx, &body).await?;
                // A repository other than the default one is part of the panel's identity.
                let mut id = format!("pr:{}:{}", ctx.project.id, p.number);
                let mut params = json!({ "projectId": ctx.project.id, "number": p.number });
                if let Some(repo) = crate::forge::panel_repo(&state, &ctx.project) {
                    id = format!("pr:{}::{repo}:{}", ctx.project.id, p.number);
                    params["repo"] = json!(repo);
                }
                state.events.ui_open_id(
                    "pr",
                    &id,
                    params,
                    Some(&format!("#{} {}", p.number, p.title)),
                );
                Ok(ToolOutput::Text(format!(
                    "Created #{}: {} ({} → {})\n{}",
                    p.number,
                    p.title,
                    p.head.as_ref().map(|h| h.git_ref.as_str()).unwrap_or("?"),
                    p.base.as_ref().map(|h| h.git_ref.as_str()).unwrap_or("?"),
                    p.html_url
                )))
            },
        ),
        tool(
            "github_pr_comment",
            "Comment on a pull request: a conversation comment, a reply to a review comment (`inReplyTo`), or a line comment (`path` plus `line`; `side` LEFT for a removed line).",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "number": { "type": "integer" },
                "body": { "type": "string", "description": "Markdown." },
                "inReplyTo": { "type": "integer", "description": "Id of the review comment that starts the thread." },
                "path": { "type": "string" },
                "line": { "type": "integer" },
                "side": { "type": "string", "enum": ["RIGHT", "LEFT"], "default": "RIGHT" }
            }, "required": ["number", "body"] }),
            true,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                let num = need(&args, "number")?;
                let body = args.get("body").and_then(Value::as_str).unwrap_or_default().to_string();
                if let Some(root) = n(&args, "inReplyTo") {
                    let c = pulls::reply(&ctx, num, root, &body).await?;
                    return Ok(ToolOutput::Text(format!("Replied to comment {root} on #{num} (comment {})", c.id)));
                }
                if let Some(path) = s(&args, "path") {
                    let c = pulls::NewReviewComment {
                        body,
                        path: path.clone(),
                        line: n(&args, "line"),
                        side: s(&args, "side"),
                        ..Default::default()
                    };
                    let rc = pulls::add_review_comment(&ctx, num, &c).await?;
                    return Ok(ToolOutput::Text(format!("Commented on {path} in #{num} (comment {})", rc.id)));
                }
                let c = pulls::add_comment(&ctx, num, &body).await?;
                Ok(ToolOutput::Text(format!("Commented on #{num} (comment {})", c.id)))
            },
        ),
        tool(
            "github_rerun",
            "Re-run a GitHub Actions workflow run (all jobs, or only the failed ones with failedOnly), or a single job with jobId.",
            json!({ "type": "object", "properties": {
                "projectId": project_prop(),
                "repo": repo_prop(),
                "runId": { "type": "integer" },
                "failedOnly": { "type": "boolean", "default": false },
                "jobId": { "type": "integer" }
            }}),
            true,
            |state, mctx, args| async move {
                let ctx = tool_ctx(&state, &mctx, &args).await?;
                if let Some(job) = n(&args, "jobId") {
                    let j = ci::rerun_job(&ctx, job).await?;
                    return Ok(ToolOutput::Text(format!("Re-running job {} \"{}\" of run {}", j.id, j.name, j.run_id)));
                }
                let id = need(&args, "runId")?;
                let failed = b(&args, "failedOnly").unwrap_or(false);
                let r = ci::run_action(&ctx, id, if failed { "rerun-failed-jobs" } else { "rerun" }).await?;
                Ok(ToolOutput::Text(format!(
                    "Re-running {}run {} ({}), now {}",
                    if failed { "the failed jobs of " } else { "" },
                    r.id,
                    r.name.as_deref().unwrap_or("?"),
                    r.state
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
        let a = json!({ "number": "#12", "jobId": 5, "x": "  " });
        assert_eq!(n(&a, "number"), Some(12));
        assert_eq!(n(&a, "jobId"), Some(5));
        assert_eq!(s(&a, "x"), None);
        assert!(need(&a, "missing").is_err());
    }

    #[test]
    fn tool_names_are_prefixed_and_mutations_marked() {
        let tools = mcp_tools();
        assert!(tools.iter().all(|t| t.name.starts_with("github_")));
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["github_runs", "github_run_jobs", "github_job_log", "github_prs", "github_pr", "github_pr_diff", "github_create_pr", "github_pr_comment", "github_rerun"]
        );
        let mutating: Vec<&str> = tools.iter().filter(|t| t.mutating).map(|t| t.name.as_str()).collect();
        assert_eq!(mutating, ["github_create_pr", "github_pr_comment", "github_rerun"]);
    }
}

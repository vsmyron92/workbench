//! MCP tools for hosted agents. Agents run git themselves; these only drive the
//! Workbench UI (show a diff/commit, hand a commit message to the commit window) and
//! read what only Workbench knows (the user's changelists and shelf). Nothing here
//! rewrites history, stages, shelves or bisects: destructive git operations stay
//! with the user (docs/ARCHITECTURE.md, MCP tools).

use axum::http::Method;
use serde_json::{Value, json};

use super::routes::{RepoSel, repo_for};
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

/// The target project: `projectId` (the name every Workbench tool uses; `project`
/// is accepted as an older alias), else the calling session's project. A session is
/// confined to its own project (`McpCtx::project_for`).
fn project_of(ctx: &McpCtx, args: &Value) -> Result<String, ApiError> {
    let requested = ["projectId", "project"].iter().find_map(|k| args.get(*k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()));
    ctx.project_for(requested)
}

fn project_prop() -> Value {
    json!({ "type": "string", "description": "Workbench project id (default: this session's project)" })
}

fn repo_prop() -> Value {
    json!({
        "type": "string",
        "description": "Repository of the project: its id as the project lists it (`.` for the repository the project root is in, \
                        else its folder such as `services/api`). Default: the project's default repository"
    })
}

/// The repository the tool is about (`repo`; absent: the default one).
fn repo_of(args: &Value) -> RepoSel {
    RepoSel(str_arg(args, "repo").map(str::to_string))
}

/// The id a panel of that repository has in the UI (`<project>` for the default repository,
/// `<project>::<repository>` for another): what the web's scope id is.
fn scope_id(project: &crate::projects::Project) -> String {
    match project.repos.first() {
        Some(d) if d.id != project.repo_id() => format!("{}::{}", project.id, project.repo_id()),
        _ => project.id.clone(),
    }
}

fn str_arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Emit `ui.open` with the panel's conventional id (docs/ARCHITECTURE.md#panels),
/// so reopening focuses the existing tab.
fn open(state: &AppState, panel: &str, id: String, params: Value, title: String) {
    state.events.emit("ui.open", None, json!({ "panel": panel, "id": id, "params": params, "title": title }));
}

async fn show_diff(state: AppState, ctx: McpCtx, args: Value) -> Result<ToolOutput, ApiError> {
    let pid = project_of(&ctx, &args)?;
    let rs = repo_of(&args);
    let project = super::routes::project_for(&state, &pid, &rs)?;
    let repo = repo_for(&state, &pid, &rs).await?;
    // Panels address the repository by its scope id; their params name the project and repository.
    let (scope, repo_id) = (scope_id(&project), repo.id.clone());
    let path = str_arg(&args, "path");
    let sha = match str_arg(&args, "sha") {
        Some(s) => Some(super::diff::resolve_commit(&repo, s).await?),
        None => None,
    };
    match (path, sha) {
        (Some(path), Some(sha)) => {
            repo.to_repo(path)?;
            let id = format!("diff:{scope}:commit:{sha}:{path}");
            let params = json!({ "projectId": pid, "repo": repo_id, "path": path, "mode": "commit", "sha": sha });
            open(&state, "diff", id, params, format!("{} @ {}", file_name(path), &sha[..8.min(sha.len())]));
            Ok(ToolOutput::Text(format!("Opened the diff of {path} in commit {} in Workbench.", &sha[..10.min(sha.len())])))
        }
        (None, Some(sha)) => {
            let id = format!("commit:{scope}:{sha}");
            open(&state, "commit", id, json!({ "projectId": pid, "repo": repo_id, "sha": sha }), format!("Commit {}", &sha[..8.min(sha.len())]));
            Ok(ToolOutput::Text(format!("Opened commit {} in Workbench.", &sha[..10.min(sha.len())])))
        }
        (Some(path), None) => {
            let rp = repo.to_repo(path)?;
            let st = super::status::status(&repo, false).await?;
            let entry = st.files.iter().find(|f| repo.to_repo(&f.path).ok().as_deref() == Some(rp.as_str()));
            let mode = match entry {
                Some(f) if f.worktree != ' ' || f.conflict => "working",
                Some(f) if f.index != ' ' => "staged",
                Some(_) => "working",
                None => return Err(ApiError::bad_request(format!("{path} has no uncommitted changes; pass sha to show a commit's diff"))),
            };
            if entry.is_some_and(|f| f.conflict) {
                let id = format!("conflict:{scope}:{path}");
                open(&state, "conflict", id, json!({ "projectId": pid, "repo": repo_id, "path": path }), format!("Conflict: {}", file_name(path)));
                return Ok(ToolOutput::Text(format!("{path} has merge conflicts; opened the conflict resolver in Workbench.")));
            }
            let id = format!("diff:{scope}:{mode}::{path}");
            open(&state, "diff", id, json!({ "projectId": pid, "repo": repo_id, "path": path, "mode": mode }), format!("{} ({mode})", file_name(path)));
            Ok(ToolOutput::Text(format!("Opened the {mode} diff of {path} in Workbench.")))
        }
        (None, None) => Err(ApiError::bad_request("pass path (uncommitted changes of a file) and/or sha (a commit)")),
    }
}

async fn set_commit_message(state: AppState, ctx: McpCtx, args: Value) -> Result<ToolOutput, ApiError> {
    let pid = project_of(&ctx, &args)?;
    let project = super::routes::project_for(&state, &pid, &repo_of(&args))?;
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| ApiError::bad_request("message is required"))?;
    if message.len() > 20_000 {
        return Err(ApiError::bad_request("message is too long"));
    }
    state.events.emit(
        "git.commitMessage",
        Some(&pid),
        json!({ "projectId": pid, "repo": project.repo_id(), "message": message, "terminalId": ctx.terminal_id }),
    );
    Ok(ToolOutput::Text(
        "The message is now in the Workbench commit window. The user reviews it and commits; do not run git commit yourself.".into(),
    ))
}

/// The user's changelists (with their files) and shelved changes, read-only.
async fn changelists(state: AppState, ctx: McpCtx, args: Value) -> Result<ToolOutput, ApiError> {
    let pid = project_of(&ctx, &args)?;
    let project = super::routes::project_for(&state, &pid, &repo_of(&args))?;
    let base = format!("/api/projects/{}/git", urlencoding::encode(&pid));
    let repo = format!("?repo={}", urlencoding::encode(project.repo_id()));
    let lists = crate::mcp::call_api(&state, Method::GET, &format!("{base}/changelists{repo}"), None, &ctx).await?;
    let shelves = crate::mcp::call_api(&state, Method::GET, &format!("{base}/shelf{repo}"), None, &ctx).await?;
    let shelves: Vec<Value> = shelves
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| {
                    json!({
                        "name": m.get("name"),
                        "created": m.get("created"),
                        "branch": m.get("branch"),
                        "files": m.get("files").and_then(Value::as_array).map(|f| f.iter().filter_map(|x| x.get("path").cloned()).collect::<Vec<_>>()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ToolOutput::Json(json!({ "changelists": lists.get("lists"), "active": lists.get("active"), "shelves": shelves })))
}

pub fn tools() -> Vec<McpTool> {
    vec![
        tool(
            "workbench_show_diff",
            "Show a diff in the Workbench UI (the user's IDE). With `path` only: the file's uncommitted changes \
             (unstaged, else staged; a conflicted file opens the merge resolver). With `sha` only: that commit's \
             details. With both: that file's change in that commit. Use it to point the user at what you changed.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the project root" },
                    "sha": { "type": "string", "description": "Commit (sha, branch, tag or revision expression)" },
                    "projectId": project_prop(),
                    "repo": repo_prop()
                }
            }),
            false,
            show_diff,
        ),
        tool(
            "workbench_set_commit_message",
            "Put a commit message into the Workbench commit window for the user to review and commit. Use this \
             when asked to write a commit message: summarize the staged changes (git diff --cached) in the \
             project's commit style (subject line under ~72 characters, blank line, body). Does not commit.",
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "The full commit message" },
                    "projectId": project_prop(),
                    "repo": repo_prop()
                },
                "required": ["message"]
            }),
            false,
            set_commit_message,
        ),
        tool(
            "workbench_changelists",
            "Read how the user grouped their uncommitted changes in Workbench: the changelists (name, whether it is \
             the active one, the changed files in it) and the shelf (changes the user set aside, with their files). \
             Read-only. Use it to respect the user's grouping, e.g. to describe or review only the files of one \
             changelist; committing, shelving and unshelving stay with the user.",
            json!({
                "type": "object",
                "properties": { "projectId": project_prop(), "repo": repo_prop() }
            }),
            false,
            changelists,
        ),
    ]
}

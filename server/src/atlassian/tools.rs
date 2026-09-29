//! MCP tools (`confluence_*`, `jira_*`) for hosted Claude sessions. They call the
//! same core functions as the REST handlers, with the calling session's project
//! choosing the site and credentials, and return compact text for the model.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::client::Product;
use super::{agile, api_for, comments, confluence, files, jira, jira_api, pages, storage};
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

fn arg_str(a: &Value, k: &str) -> Option<String> {
    match a.get(k)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn arg_u32(a: &Value, k: &str) -> Option<u32> {
    match a.get(k)? {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn required(a: &Value, k: &str) -> Result<String, ApiError> {
    arg_str(a, k).ok_or_else(|| ApiError::bad_request(format!("`{k}` is required")))
}

/// Readable page text handed to a model is capped (a few very long pages exist).
const MAX_TEXT_CHARS: usize = 120_000;

static PAGE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/pages/(?:edit-v2/)?(\d+)|[?&]pageId=(\d+)").unwrap());

/// A page id from `65601`, or a Confluence page URL.
pub fn page_id_arg(raw: &str) -> Result<String, ApiError> {
    let raw = raw.trim();
    if raw.bytes().all(|b| b.is_ascii_digit()) && !raw.is_empty() {
        return Ok(raw.to_string());
    }
    PAGE_URL
        .captures(raw)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| ApiError::bad_request(format!("{raw:?} is not a Confluence page id or URL")))
}

fn notify(state: &AppState, message: String) {
    state.events.notify("info", &message);
}

fn conf_api(state: &AppState, ctx: &McpCtx) -> Result<super::client::Api, ApiError> {
    api_for(state, ctx.project_id.as_deref(), Product::Confluence)
}

// ---------------------------------------------------------------- confluence

async fn confluence_search(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let input = confluence::SearchIn {
        q: arg_str(&a, "query"),
        cql: arg_str(&a, "cql"),
        space: arg_str(&a, "space"),
        limit: Some(arg_u32(&a, "limit").unwrap_or(15).clamp(1, 50)),
        archived: a.get("includeArchived").and_then(Value::as_bool),
        ..Default::default()
    };
    let out = confluence::search(&api, &input).await?;
    let mut text = format!("CQL: {}\n", out.cql);
    if out.results.is_empty() {
        text.push_str("No results.");
    }
    for h in &out.results {
        text.push_str(&format!(
            "\n- [{}] {}{} — {}{}\n  {}",
            h.id,
            h.title,
            if h.status == "archived" { " (archived)" } else { "" },
            h.space_key.as_deref().or(h.space_name.as_deref()).unwrap_or("?"),
            h.last_modified.as_deref().map(|m| format!(", modified {m}")).unwrap_or_default(),
            h.excerpt
        ));
    }
    if out.next_cursor.is_some() {
        text.push_str("\n\n(more results exist; narrow the query)");
    }
    Ok(ToolOutput::Text(text))
}

async fn confluence_get_page(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let id = page_id_arg(&required(&a, "pageId")?)?;
    let format = arg_str(&a, "format").unwrap_or_else(|| "text".into());
    let p = confluence::get_page(&state, &api, &id, arg_u32(&a, "version"), ctx.project_id.as_deref()).await?;
    let path = p.ancestors.iter().map(|c| c.title.as_str()).collect::<Vec<_>>().join(" / ");
    let mut head = format!(
        "Title: {}\nPage id: {}\nVersion: {} ({}{})\nSpace: {}\nURL: {}\n",
        p.title,
        p.id,
        p.version.number,
        p.version.created_at,
        p.version.author_name.as_deref().map(|n| format!(" by {n}")).unwrap_or_default(),
        p.space_key.as_deref().unwrap_or(&p.space_id),
        p.web_url,
    );
    if !path.is_empty() {
        head.push_str(&format!("Path: {path}\n"));
    }
    if p.status != "current" {
        head.push_str(&format!("Status: {}\n", p.status));
    }
    if !p.labels.is_empty() {
        head.push_str(&format!("Labels: {}\n", p.labels.join(", ")));
    }
    if p.has_inline_comment_markers {
        head.push_str(&format!(
            "Inline comments: {} marker(s). When updating, use format=storage and keep every <ac:inline-comment-marker> element.\n",
            p.inline_marker_refs.len()
        ));
    }
    head.push_str(&format!(
        "To edit: confluence_update_page with baseVersion={} (the update is refused if the page changed since).\n\n",
        p.version.number
    ));
    let body = if format == "storage" {
        // Never truncated: an edit must send back the whole document.
        p.storage
    } else {
        let text = storage::to_text(&p.storage);
        if text.chars().count() > MAX_TEXT_CHARS {
            let cut: String = text.chars().take(MAX_TEXT_CHARS).collect();
            format!("{cut}\n\n… (truncated: the page is {} characters as text; use format=storage for all of it)", text.chars().count())
        } else {
            text
        }
    };
    Ok(ToolOutput::Text(head + &body))
}

async fn tree_lines(api: &super::client::Api, nodes: Vec<confluence::TreeNode>, depth: u32, max_depth: u32, budget: &mut usize, out: &mut String) {
    for n in nodes {
        if *budget == 0 {
            out.push_str(&format!("{}…\n", "  ".repeat(depth as usize)));
            return;
        }
        *budget -= 1;
        let tag = match (n.kind.as_str(), n.status.as_str()) {
            ("page", "current") => String::new(),
            (k, "current") => format!(" ({k})"),
            (k, s) if k == "page" => format!(" ({s})"),
            (k, s) => format!(" ({k}, {s})"),
        };
        out.push_str(&format!("{}- {} [{}]{tag}\n", "  ".repeat(depth as usize), n.title, n.id));
        if depth + 1 < max_depth && n.has_children != Some(false) {
            if let Ok(kids) = confluence::children(api, &n.id, &n.kind, false).await {
                Box::pin(tree_lines(api, kids.children, depth + 1, max_depth, budget, out)).await;
            }
        }
    }
}

async fn confluence_page_tree(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let depth = arg_u32(&a, "depth").unwrap_or(2).clamp(1, 5);
    let mut out = String::new();
    let mut budget = 400usize;
    if let Some(pid) = arg_str(&a, "pageId") {
        let id = page_id_arg(&pid)?;
        let kids = confluence::children(&api, &id, "page", false).await?;
        out.push_str(&format!("Children of page {id}:\n"));
        tree_lines(&api, kids.children, 0, depth, &mut budget, &mut out).await;
    } else if let Some(key) = arg_str(&a, "spaceKey") {
        let space = confluence::space_by_key(&state, &api, &key).await?;
        let roots = confluence::space_root_pages(&api, &space.id, "current").await?;
        out.push_str(&format!("Space {} ({}):\n", space.name, space.key));
        tree_lines(&api, roots.children, 0, depth + 1, &mut budget, &mut out).await;
    } else {
        return Err(ApiError::bad_request("pass spaceKey or pageId"));
    }
    Ok(ToolOutput::Text(out))
}

async fn confluence_update_page(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let id = page_id_arg(&required(&a, "pageId")?)?;
    // Without a base version the update could replace a colleague's newer version unseen.
    let base = arg_u32(&a, "baseVersion").filter(|v| *v > 0).ok_or_else(|| {
        ApiError::bad_request(
            "`baseVersion` is required: call confluence_get_page first and pass the version it reports \
             (the update is refused if the page changed since, so nobody's newer edit is overwritten)",
        )
    })?;
    let input = confluence::UpdateIn {
        title: arg_str(&a, "title"),
        storage: arg_str(&a, "storage"),
        markdown: arg_str(&a, "markdown"),
        version: Some(base),
        message: Some(arg_str(&a, "message").unwrap_or_else(|| "Edited by an agent in Workbench".into())),
        force: false,
        minor_edit: false,
    };
    let out = confluence::update_page(&state, &api, &id, input).await?;
    if out.unchanged {
        return Ok(ToolOutput::Text(format!("No changes: page {} is still version {}.", out.title, out.version)));
    }
    notify(&state, format!("Agent updated Confluence page “{}” (v{})", out.title, out.version));
    Ok(ToolOutput::Text(format!("Updated “{}” to version {}: {}", out.title, out.version, out.web_url)))
}

async fn confluence_create_page(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let input = confluence::CreateIn {
        space_id: arg_str(&a, "spaceId"),
        space_key: arg_str(&a, "spaceKey"),
        parent_id: arg_str(&a, "parentId").map(|p| page_id_arg(&p)).transpose()?,
        title: required(&a, "title")?,
        storage: arg_str(&a, "storage"),
        markdown: arg_str(&a, "markdown"),
    };
    let out = confluence::create_page(&state, &api, input).await?;
    notify(&state, format!("Agent created Confluence page “{}”", out.title));
    Ok(ToolOutput::Text(format!("Created “{}” (page id {}): {}", out.title, out.id, out.web_url)))
}

async fn confluence_add_comment(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let id = page_id_arg(&required(&a, "pageId")?)?;
    let input = confluence::AddCommentIn {
        markdown: arg_str(&a, "markdown"),
        storage: arg_str(&a, "storage"),
        parent_comment_id: arg_str(&a, "parentCommentId"),
        parent_kind: arg_str(&a, "parentKind"),
    };
    let out = confluence::add_comment(&state, &api, &id, input).await?;
    notify(&state, format!("Agent commented on Confluence page {id}"));
    Ok(ToolOutput::Text(format!("Added comment {} to page {id}.", out.get("id").map(|v| v.to_string()).unwrap_or_default())))
}

async fn confluence_add_inline_comment(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let id = page_id_arg(&required(&a, "pageId")?)?;
    // Exactly as written: the selection must match the page text character for character.
    let selection = a.get("selection").and_then(Value::as_str).unwrap_or_default().to_string();
    let match_index = match arg_u32(&a, "occurrence") {
        Some(0) => return Err(ApiError::bad_request("`occurrence` counts from 1")),
        Some(n) => Some(n - 1),
        None => None,
    };
    let input = comments::CreateInlineIn {
        markdown: arg_str(&a, "markdown"),
        storage: arg_str(&a, "storage"),
        selection,
        match_index,
        match_count: None,
    };
    let out = comments::create_inline(&state, &api, &id, input, ctx.project_id.as_deref()).await?;
    notify(&state, format!("Agent added an inline comment to Confluence page {id}"));
    Ok(ToolOutput::Text(format!(
        "Added inline comment {} to page {id} on occurrence {} of {} of the selected text.",
        out.id,
        out.match_index + 1,
        out.match_count
    )))
}

/// Hidden files and folders (`.env`, `.git/…`, `.ssh/…`) and key or token files are
/// never uploaded, nor anything the project marks `sensitive` (gitignore-style, as the
/// files slice matches it: `secrets/` covers `config/secrets/…` too).
fn check_uploadable(rel: &str, sensitive: &crate::files::Sensitive) -> Result<(), ApiError> {
    let refuse = |why: &str| Err(ApiError::forbidden(format!("{rel} is not uploaded: {why}")));
    if rel.split('/').any(|c| c.starts_with('.')) {
        return refuse("hidden files and folders may hold credentials");
    }
    let name = rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase();
    const KEY_SUFFIXES: &[&str] = &[
        ".pem", ".key", ".p12", ".pfx", ".token", "_token", "_api_key", ".api_key", "_api_sk", ".api_sk", ".keystore", ".jks",
    ];
    if name.starts_with("id_rsa") || name.starts_with("id_ecdsa") || name.starts_with("id_ed25519") || KEY_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return refuse("it looks like a key or token file");
    }
    // The built-in credential names (`.netrc`, `*_api_sk`…) and the project's patterns.
    if sensitive.matches(rel) {
        return refuse("the project marks it sensitive");
    }
    Ok(())
}

/// The file an agent asked to upload: `raw` resolved inside the project root, checked
/// both as named and as what it really is once symlinks are followed (`docs/notes.txt ->
/// ../.env`, a folder linked into `.git/`), so a link never smuggles out a refused file.
/// Returns the canonical path to read and the project-relative name to report.
fn uploadable_file(root: &std::path::Path, raw: &str, sensitive: &crate::files::Sensitive) -> Result<(std::path::PathBuf, String), ApiError> {
    use crate::util::paths::{relative_to, resolve_absolute_in, resolve_in_root};
    let abs = if std::path::Path::new(raw).is_absolute() {
        resolve_absolute_in(std::slice::from_ref(&root.to_path_buf()), raw)?
    } else {
        resolve_in_root(root, raw)?
    };
    let rel = relative_to(root, &abs).unwrap_or_else(|| raw.to_string());
    check_uploadable(&rel, sensitive)?;
    let canon = abs.canonicalize().map_err(|_| ApiError::not_found(format!("{rel} does not exist in the project")))?;
    let canon_root = root.canonicalize().map_err(|_| ApiError::not_found("the project folder is missing"))?;
    let real_rel = relative_to(&canon_root, &canon)
        .filter(|r| !r.is_empty())
        .ok_or_else(|| ApiError::forbidden(format!("{rel} is not uploaded: it resolves outside the project")))?;
    if real_rel != rel {
        check_uploadable(&real_rel, sensitive).map_err(|_| ApiError::forbidden(format!("{rel} is not uploaded: it links to {real_rel}, which is refused")))?;
    }
    Ok((canon, rel))
}

/// Largest file an agent uploads (read into memory first).
const MAX_AGENT_UPLOAD: u64 = 50 * 1024 * 1024;

async fn confluence_upload_attachment(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let page = page_id_arg(&required(&a, "pageId")?)?;
    let project_id = ctx.project_for(arg_str(&a, "projectId").as_deref())?;
    let project = state.projects.require(&project_id)?;
    // The site and credentials of the project the file belongs to.
    let api = api_for(&state, Some(&project_id), Product::Confluence)?;
    let raw = required(&a, "path")?;
    let sensitive = crate::files::Sensitive::new(&project.config.project.sensitive);
    // `abs` is canonical: the checked file is the one read, whatever links led to it.
    let (abs, rel) = uploadable_file(&project.root, &raw, &sensitive)?;
    let meta = tokio::fs::metadata(&abs).await.map_err(|_| ApiError::not_found(format!("{rel} does not exist in project {project_id}")))?;
    if !meta.is_file() {
        return Err(ApiError::bad_request(format!("{rel} is not a file")));
    }
    if meta.len() > MAX_AGENT_UPLOAD {
        return Err(ApiError::bad_request(format!("{rel} is larger than {} MB", MAX_AGENT_UPLOAD / 1024 / 1024)));
    }
    let bytes = tokio::fs::read(&abs).await?;
    let name = arg_str(&a, "name").unwrap_or_else(|| abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into()));
    let up = files::Upload {
        media_type: mime_guess::from_path(&name).first_or_octet_stream().essence_str().to_string(),
        len: bytes.len() as u64,
        body: reqwest::Body::from(bytes),
        comment: arg_str(&a, "comment"),
        replace: a.get("replace").and_then(Value::as_bool).unwrap_or(false),
        name,
    };
    let out = files::upload(&state, &api, &page, up, Some(&project_id)).await?;
    notify(&state, format!("Agent attached “{}” to Confluence page {page}", out.title));
    let embed = if out.is_image {
        format!("<ac:image><ri:attachment ri:filename=\"{}\" /></ac:image>", super::html::escape(&out.title))
    } else {
        format!("<ac:link><ri:attachment ri:filename=\"{}\" /></ac:link>", super::html::escape(&out.title))
    };
    Ok(ToolOutput::Text(format!(
        "Attached “{}” ({} bytes, version {}) to page {page} as {}.\nTo show it in the page, put this in the storage body: {embed}",
        out.title,
        out.file_size.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
        out.version,
        out.id
    )))
}

fn string_list(a: &Value, k: &str) -> Vec<String> {
    match a.get(k) {
        Some(Value::Array(l)) => l.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => s.split([',', ' ']).filter(|x| !x.is_empty()).map(str::to_string).collect(),
        _ => vec![],
    }
}

async fn confluence_labels(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = conf_api(&state, &ctx)?;
    let id = page_id_arg(&required(&a, "pageId")?)?;
    let add = string_list(&a, "add");
    let remove = string_list(&a, "remove");
    let mut changed = vec![];
    if !add.is_empty() {
        pages::add_labels(&state, &api, &id, &add).await?;
        changed.push(format!("added {}", add.join(", ")));
    }
    for l in &remove {
        pages::remove_label(&state, &api, &id, l).await?;
    }
    if !remove.is_empty() {
        changed.push(format!("removed {}", remove.join(", ")));
    }
    let now = pages::labels(&api, &id).await?;
    if !changed.is_empty() {
        notify(&state, format!("Agent changed the labels of Confluence page {id}"));
    }
    let list = if now.is_empty() { "(none)".to_string() } else { now.join(", ") };
    Ok(ToolOutput::Text(if changed.is_empty() {
        format!("Labels of page {id}: {list}")
    } else {
        format!("Page {id}: {}. Labels now: {list}", changed.join("; "))
    }))
}

// ---------------------------------------------------------------- jira

async fn jira_boards(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let out = agile::boards(&api, arg_str(&a, "name").as_deref(), arg_str(&a, "projectKey").as_deref()).await?;
    if out.boards.is_empty() {
        return Ok(ToolOutput::Text("No boards.".into()));
    }
    let mut text = String::new();
    for b in &out.boards {
        text.push_str(&format!(
            "- [{}] {} ({}{})\n",
            b.id,
            b.name,
            b.kind,
            b.project_key.as_deref().map(|k| format!(", project {k}")).unwrap_or_default()
        ));
    }
    if out.truncated {
        text.push_str("(more boards exist; filter by name or projectKey)\n");
    }
    text.push_str("Read a board's issues with jira_board_issues.");
    Ok(ToolOutput::Text(text))
}

async fn jira_board_issues(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let board_id = arg_u32(&a, "boardId").filter(|b| *b > 0).ok_or_else(|| ApiError::bad_request("`boardId` is required (see jira_boards)"))? as u64;
    let detail = agile::board(&api, board_id).await?;
    let want = arg_str(&a, "sprint").unwrap_or_else(|| if detail.has_sprints { "active".into() } else { "board".into() });
    let (scope, label) = match want.as_str() {
        "backlog" => (agile::Scope::Backlog, "Backlog".to_string()),
        "board" | "all" => (agile::Scope::Board, "All board issues".to_string()),
        "active" => {
            let active = agile::sprints(&api, board_id, "active").await?;
            match active.first() {
                Some(s) => (agile::Scope::Sprint(s.id), format!("Sprint {} ({})", s.name, s.state)),
                None => (agile::Scope::Board, "No active sprint; all board issues".to_string()),
            }
        }
        id => {
            let n: u64 = id.parse().map_err(|_| ApiError::bad_request("`sprint` is active, backlog, board or a sprint id"))?;
            (agile::Scope::Sprint(n), format!("Sprint {n}"))
        }
    };
    let issues = agile::board_issues(&api, board_id, scope, arg_str(&a, "jql").as_deref()).await?;
    let mut text = format!("Board {} [{}] — {label}\n", detail.board.name, detail.board.id);
    let mut placed = vec![false; issues.issues.len()];
    for col in &detail.columns {
        let in_col: Vec<(usize, &crate::atlassian::jira::IssueSummary)> = issues
            .issues
            .iter()
            .enumerate()
            .filter(|(_, i)| i.status.as_ref().and_then(|s| s.id.as_ref()).is_some_and(|id| col.status_ids.contains(id)))
            .collect();
        text.push_str(&format!("\n## {} ({})\n", col.name, in_col.len()));
        for (n, i) in in_col {
            placed[n] = true;
            text.push_str(&format!(
                "- {} {} [{}] ({})\n",
                i.key,
                i.summary,
                i.status.as_ref().map(|s| s.name.as_str()).unwrap_or("?"),
                i.assignee.as_ref().map(|u| u.display_name.as_str()).unwrap_or("unassigned")
            ));
        }
    }
    let rest: Vec<&crate::atlassian::jira::IssueSummary> = issues.issues.iter().zip(&placed).filter(|(_, p)| !**p).map(|(i, _)| i).collect();
    if !rest.is_empty() {
        text.push_str(&format!("\n## Not on the board's columns ({})\n", rest.len()));
        for i in rest {
            text.push_str(&format!("- {} {} [{}]\n", i.key, i.summary, i.status.as_ref().map(|s| s.name.as_str()).unwrap_or("?")));
        }
    }
    if issues.truncated {
        text.push_str(&format!("\n(showing {} of {} issues)", issues.issues.len(), issues.total));
    }
    Ok(ToolOutput::Text(text))
}

async fn jira_search(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let input = jira::SearchIn {
        jql: required(&a, "jql")?,
        next_page_token: arg_str(&a, "nextPageToken"),
        max_results: Some(arg_u32(&a, "limit").unwrap_or(25).clamp(1, 100)),
        project_id: None,
    };
    let out = jira::search(&api, &input).await?;
    let mut text = String::new();
    if out.issues.is_empty() {
        text.push_str("No issues.");
    }
    for i in &out.issues {
        text.push_str(&format!(
            "- {} [{}] {} (assignee: {}, priority: {}, updated {})\n",
            i.key,
            i.status.as_ref().map(|s| s.name.as_str()).unwrap_or("?"),
            i.summary,
            i.assignee.as_ref().map(|u| u.display_name.as_str()).unwrap_or("unassigned"),
            i.priority.as_ref().map(|p| p.name.as_str()).unwrap_or("-"),
            i.updated.as_deref().unwrap_or("?"),
        ));
    }
    if let Some(t) = out.next_page_token.filter(|_| !out.is_last) {
        text.push_str(&format!("\nMore results: pass nextPageToken={t}"));
    }
    Ok(ToolOutput::Text(text))
}

async fn jira_get_issue(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let i = jira::get_issue(&api, &required(&a, "key")?, ctx.project_id.as_deref()).await?;
    let s = &i.summary;
    let mut text = format!(
        "{} — {}\nStatus: {}\nType: {}\nPriority: {}\nAssignee: {}\nReporter: {}\nLabels: {}\nURL: {}\n",
        s.key,
        s.summary,
        s.status.as_ref().map(|x| x.name.as_str()).unwrap_or("?"),
        s.issue_type.as_ref().map(|x| x.name.as_str()).unwrap_or("?"),
        s.priority.as_ref().map(|x| x.name.as_str()).unwrap_or("-"),
        s.assignee.as_ref().map(|u| u.display_name.as_str()).unwrap_or("unassigned"),
        i.reporter.as_ref().map(|u| u.display_name.as_str()).unwrap_or("?"),
        if s.labels.is_empty() { "-".into() } else { s.labels.join(", ") },
        i.web_url,
    );
    if !i.transitions.is_empty() {
        text.push_str(&format!(
            "Transitions: {}\n",
            i.transitions.iter().map(|t| format!("{} (id {})", t.name, t.id)).collect::<Vec<_>>().join(", ")
        ));
    }
    text.push_str("\n## Description\n\n");
    text.push_str(if i.description_markdown.is_empty() { "(none)" } else { &i.description_markdown });
    if !i.comments.is_empty() {
        text.push_str(&format!("\n\n## Comments ({})\n", i.comments_total));
        for c in i.comments.iter().rev().take(20).rev() {
            text.push_str(&format!(
                "\n**{}** ({}):\n{}\n",
                c.author.as_ref().map(|u| u.display_name.as_str()).unwrap_or("?"),
                c.created.as_deref().unwrap_or("?"),
                c.markdown
            ));
        }
    }
    Ok(ToolOutput::Text(text))
}

async fn jira_update_issue(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let key = required(&a, "key")?;
    let labels = a.get("labels").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let input = jira::UpdateIn { summary: arg_str(&a, "summary"), description: arg_str(&a, "description"), labels, priority_id: None };
    jira::update_issue(&state, &api, &key, input).await?;
    notify(&state, format!("Agent updated {key}"));
    Ok(ToolOutput::Text(format!("Updated {key}.")))
}

async fn jira_transition(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let key = required(&a, "key")?;
    let want = required(&a, "transition")?;
    let issue = jira::get_issue(&api, &key, ctx.project_id.as_deref()).await?;
    let t = issue
        .transitions
        .iter()
        .find(|t| t.id == want || t.name.eq_ignore_ascii_case(&want) || t.to.as_ref().is_some_and(|s| s.name.eq_ignore_ascii_case(&want)))
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "no transition {want:?}; available: {}",
                issue.transitions.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
            ))
        })?;
    jira::transition(&state, &api, &key, &t.id, arg_str(&a, "comment").as_deref()).await?;
    notify(&state, format!("Agent moved {key} to {}", t.to.as_ref().map(|s| s.name.as_str()).unwrap_or(&t.name)));
    Ok(ToolOutput::Text(format!("{key}: {} done.", t.name)))
}

async fn jira_comment(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let key = required(&a, "key")?;
    jira::add_comment(&state, &api, &key, &required(&a, "markdown")?).await?;
    notify(&state, format!("Agent commented on {key}"));
    Ok(ToolOutput::Text(format!("Commented on {key}.")))
}

async fn jira_create_issue(state: AppState, ctx: McpCtx, a: Value) -> Result<ToolOutput, ApiError> {
    let api = jira_api(&state, ctx.project_id.as_deref()).await?;
    let labels = a.get("labels").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let input = jira::CreateIn {
        project_key: required(&a, "projectKey")?,
        issue_type_id: None,
        issue_type: arg_str(&a, "issueType"),
        summary: required(&a, "summary")?,
        description: arg_str(&a, "description"),
        labels,
        parent_key: arg_str(&a, "parentKey"),
    };
    let out = jira::create_issue(&state, &api, input).await?;
    let key = out.get("key").and_then(Value::as_str).unwrap_or("?").to_string();
    notify(&state, format!("Agent created {key}"));
    Ok(ToolOutput::Text(format!("Created {key}: {}", out.get("webUrl").and_then(Value::as_str).unwrap_or(""))))
}

pub fn all() -> Vec<McpTool> {
    vec![
        tool(
            "confluence_search",
            "Search Confluence pages. Pass `query` (full-text) or raw `cql` (e.g. `ancestor = 65601 AND title ~ \"economy\"`). Returns page ids, titles, spaces and excerpts.",
            json!({"type":"object","properties":{
                "query":{"type":"string","description":"Full-text search terms"},
                "cql":{"type":"string","description":"Raw CQL; overrides query"},
                "space":{"type":"string","description":"Space key to restrict to, e.g. DESIGN"},
                "limit":{"type":"integer","minimum":1,"maximum":50},
                "includeArchived":{"type":"boolean"}}}),
            false,
            confluence_search,
        ),
        tool(
            "confluence_get_page",
            "Read a Confluence page by id or URL. format=text (default) gives readable markdown-like text; format=storage gives the raw storage XHTML to edit and pass back to confluence_update_page. The header includes the version to pass as baseVersion.",
            json!({"type":"object","properties":{
                "pageId":{"type":"string","description":"Page id or page URL"},
                "format":{"type":"string","enum":["text","storage"]},
                "version":{"type":"integer","description":"A historical version number"}},
                "required":["pageId"]}),
            false,
            confluence_get_page,
        ),
        tool(
            "confluence_page_tree",
            "List the page tree of a space (spaceKey) or under a page (pageId), with page ids, to `depth` levels.",
            json!({"type":"object","properties":{
                "spaceKey":{"type":"string"},
                "pageId":{"type":"string"},
                "depth":{"type":"integer","minimum":1,"maximum":5}}}),
            false,
            confluence_page_tree,
        ),
        tool(
            "confluence_update_page",
            "Replace a Confluence page's body (storage XHTML or markdown) and optionally its title. baseVersion is required: pass the version confluence_get_page reported; the update is refused if someone changed the page since (read it again and redo the edit). Refused when the edit would remove inline-comment markers; to keep them, edit the storage format and preserve every <ac:inline-comment-marker> element.",
            json!({"type":"object","properties":{
                "pageId":{"type":"string"},
                "storage":{"type":"string","description":"Full new body in Confluence storage format"},
                "markdown":{"type":"string","description":"Full new body as markdown (converted to storage)"},
                "title":{"type":"string"},
                "message":{"type":"string","description":"Version comment"},
                "baseVersion":{"type":"integer","minimum":1,"description":"The version your edit is based on (from confluence_get_page)"}},
                "required":["pageId","baseVersion"]}),
            true,
            confluence_update_page,
        ),
        tool(
            "confluence_create_page",
            "Create a Confluence page in a space (spaceKey or spaceId), optionally under parentId, from storage XHTML or markdown.",
            json!({"type":"object","properties":{
                "spaceKey":{"type":"string"},
                "spaceId":{"type":"string"},
                "parentId":{"type":"string"},
                "title":{"type":"string"},
                "storage":{"type":"string"},
                "markdown":{"type":"string"}},
                "required":["title"]}),
            true,
            confluence_create_page,
        ),
        tool(
            "confluence_add_comment",
            "Add a footer comment (markdown or storage) to a Confluence page, or reply to a comment with parentCommentId.",
            json!({"type":"object","properties":{
                "pageId":{"type":"string"},
                "markdown":{"type":"string"},
                "storage":{"type":"string"},
                "parentCommentId":{"type":"string"},
                "parentKind":{"type":"string","enum":["footer","inline"]}},
                "required":["pageId"]}),
            true,
            confluence_add_comment,
        ),
        tool(
            "confluence_add_inline_comment",
            "Add an inline comment to a Confluence page, anchored to a passage of its text (like selecting text in Confluence). `selection` must be the exact text as the page shows it, within one paragraph; when it occurs more than once, pass `occurrence` (1 = first) or choose a longer, unique passage.",
            json!({"type":"object","properties":{
                "pageId":{"type":"string","description":"Page id or page URL"},
                "selection":{"type":"string","description":"The exact page text the comment is about"},
                "occurrence":{"type":"integer","minimum":1,"description":"Which occurrence of the selection (1-based); required when it occurs more than once"},
                "markdown":{"type":"string","description":"The comment (markdown); mention someone with [@Name](mention:<accountId>)"},
                "storage":{"type":"string","description":"The comment in storage format instead of markdown"}},
                "required":["pageId","selection"]}),
            true,
            confluence_add_inline_comment,
        ),
        tool(
            "confluence_upload_attachment",
            "Attach a file from this session's project to a Confluence page (a new attachment; replace=true adds a new version of an attachment with the same name). The path is relative to the project root (or absolute inside it); hidden files, key/token files and paths the project marks sensitive are refused. Returns the storage markup that shows the file in a page.",
            json!({"type":"object","properties":{
                "pageId":{"type":"string","description":"Page id or page URL"},
                "path":{"type":"string","description":"File inside the project"},
                "name":{"type":"string","description":"Attachment name (default: the file name)"},
                "comment":{"type":"string"},
                "replace":{"type":"boolean","description":"Add a new version if an attachment of that name exists"},
                "projectId":{"type":"string","description":"Only for callers that are not a project session"}},
                "required":["pageId","path"]}),
            true,
            confluence_upload_attachment,
        ),
        tool(
            "confluence_labels",
            "List a Confluence page's labels, and add or remove labels (single lowercase words).",
            json!({"type":"object","properties":{
                "pageId":{"type":"string","description":"Page id or page URL"},
                "add":{"type":"array","items":{"type":"string"}},
                "remove":{"type":"array","items":{"type":"string"}}},
                "required":["pageId"]}),
            true,
            confluence_labels,
        ),
        tool(
            "jira_boards",
            "List Jira Software boards (scrum and kanban), optionally by name or project key.",
            json!({"type":"object","properties":{
                "name":{"type":"string","description":"Part of the board name"},
                "projectKey":{"type":"string"}}}),
            false,
            jira_boards,
        ),
        tool(
            "jira_board_issues",
            "Read a Jira board: its columns and the issues in each. `sprint` is active (default for scrum boards), backlog, board (every issue on the board; default for kanban) or a sprint id. `jql` narrows the issues.",
            json!({"type":"object","properties":{
                "boardId":{"type":"integer","minimum":1},
                "sprint":{"type":"string"},
                "jql":{"type":"string"}},
                "required":["boardId"]}),
            false,
            jira_board_issues,
        ),
        tool(
            "jira_search",
            "Search Jira issues with JQL (bounded queries, e.g. `project = ABC AND statusCategory != Done ORDER BY updated DESC`).",
            json!({"type":"object","properties":{
                "jql":{"type":"string"},
                "limit":{"type":"integer","minimum":1,"maximum":100},
                "nextPageToken":{"type":"string"}},
                "required":["jql"]}),
            false,
            jira_search,
        ),
        tool(
            "jira_get_issue",
            "Read a Jira issue: fields, description (markdown), available transitions and recent comments.",
            json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}),
            false,
            jira_get_issue,
        ),
        tool(
            "jira_update_issue",
            "Update a Jira issue's summary, description (markdown) and/or labels.",
            json!({"type":"object","properties":{
                "key":{"type":"string"},
                "summary":{"type":"string"},
                "description":{"type":"string","description":"Markdown"},
                "labels":{"type":"array","items":{"type":"string"}}},
                "required":["key"]}),
            true,
            jira_update_issue,
        ),
        tool(
            "jira_transition",
            "Move a Jira issue through its workflow: `transition` is a transition id or name, or the target status name.",
            json!({"type":"object","properties":{
                "key":{"type":"string"},
                "transition":{"type":"string"},
                "comment":{"type":"string","description":"Optional markdown comment"}},
                "required":["key","transition"]}),
            true,
            jira_transition,
        ),
        tool(
            "jira_comment",
            "Add a markdown comment to a Jira issue.",
            json!({"type":"object","properties":{"key":{"type":"string"},"markdown":{"type":"string"}},"required":["key","markdown"]}),
            true,
            jira_comment,
        ),
        tool(
            "jira_create_issue",
            "Create a Jira issue.",
            json!({"type":"object","properties":{
                "projectKey":{"type":"string"},
                "summary":{"type":"string"},
                "description":{"type":"string","description":"Markdown"},
                "issueType":{"type":"string","description":"Issue type name, default Task"},
                "labels":{"type":"array","items":{"type":"string"}},
                "parentKey":{"type":"string"}},
                "required":["projectKey","summary"]}),
            true,
            jira_create_issue,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ids_from_urls() {
        assert_eq!(page_id_arg("65601").unwrap(), "65601");
        assert_eq!(page_id_arg("https://x.atlassian.net/wiki/spaces/DESIGN/pages/65601/Design+Notes").unwrap(), "65601");
        assert_eq!(page_id_arg("https://x.atlassian.net/wiki/pages/viewpage.action?pageId=77").unwrap(), "77");
        assert_eq!(page_id_arg("https://x.atlassian.net/wiki/spaces/D/pages/edit-v2/12").unwrap(), "12");
        assert!(page_id_arg("hello").is_err());
    }

    #[test]
    fn tools_are_named_and_flagged() {
        let tools = all();
        assert_eq!(tools.len(), 17);
        for t in &tools {
            assert!(t.name.starts_with("confluence_") || t.name.starts_with("jira_"));
            assert_eq!(t.input_schema["type"], "object");
        }
        let mutating: Vec<&str> = tools.iter().filter(|t| t.mutating).map(|t| t.name.as_str()).collect();
        assert_eq!(
            mutating,
            [
                "confluence_update_page",
                "confluence_create_page",
                "confluence_add_comment",
                "confluence_add_inline_comment",
                "confluence_upload_attachment",
                "confluence_labels",
                "jira_update_issue",
                "jira_transition",
                "jira_comment",
                "jira_create_issue"
            ]
        );
    }

    #[test]
    fn uploads_refuse_secrets() {
        let none = crate::files::Sensitive::defaults();
        assert!(check_uploadable("docs/diagram.png", &none).is_ok());
        assert!(check_uploadable("reports/q3.pdf", &none).is_ok());
        assert!(check_uploadable("src/api_key.rs", &none).is_ok());
        for bad in [
            ".env",
            "config/.env.local",
            ".git/config",
            "keys/id_rsa",
            "certs/server.pem",
            "deploy/api_token",
            "a/.ssh/known_hosts",
            "deploy/openai_api_sk",
            "deploy/prod.api_sk",
            "CERTS/SERVER.PEM",
        ] {
            assert_eq!(check_uploadable(bad, &none).unwrap_err().code, "forbidden", "{bad}");
        }
        let project = ["outreach/", "*.csv", "/secrets.json", "secrets/"].map(String::from);
        let sensitive = crate::files::Sensitive::new(&project);
        assert!(check_uploadable("outreach/list.txt", &sensitive).is_err());
        assert!(check_uploadable("data/people.csv", &sensitive).is_err());
        assert!(check_uploadable("secrets.json", &sensitive).is_err());
        assert!(check_uploadable("docs/readme.md", &sensitive).is_ok());
        // Unanchored directory patterns match at any depth, as in the files slice.
        assert!(check_uploadable("team/outreach/list.txt", &sensitive).is_err());
        assert!(check_uploadable("config/secrets/db.txt", &sensitive).is_err());
        // An anchored one only at the root.
        assert!(check_uploadable("docs/secrets.json", &sensitive).is_ok());
    }

    #[test]
    fn uploads_check_what_links_point_to() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let outside = dir.path().join("outside.txt");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("config/secrets")).unwrap();
        std::fs::write(root.join(".env"), "API_KEY=x").unwrap();
        std::fs::write(root.join(".git/config"), "url = https://token@host/x").unwrap();
        std::fs::write(root.join("config/secrets/db.txt"), "pw").unwrap();
        std::fs::write(root.join("docs/real.txt"), "fine").unwrap();
        std::fs::write(&outside, "outside").unwrap();
        let link = |target: &str, at: &str| std::os::unix::fs::symlink(target, root.join(at)).unwrap();
        link("../.env", "docs/notes.txt");
        link("../.git", "docs/repo");
        link("../config/secrets", "docs/cfg");
        link("real.txt", "docs/alias.txt");
        link(outside.to_str().unwrap(), "docs/out.txt");
        let sensitive = crate::files::Sensitive::new(&["secrets/".to_string()]);
        let refused = |p: &str| uploadable_file(&root, p, &sensitive).unwrap_err();
        for p in ["docs/notes.txt", "docs/repo/config", "docs/cfg/db.txt", "docs/out.txt"] {
            assert_eq!(refused(p).code, "forbidden", "{p}");
        }
        let abs = root.join("docs/notes.txt").display().to_string();
        assert_eq!(refused(&abs).code, "forbidden");
        assert!(refused("docs/notes.txt").message.contains(".env"));
        // A link to an ordinary project file is fine; the canonical file is what is read.
        let (path, rel) = uploadable_file(&root, "docs/alias.txt", &sensitive).unwrap();
        assert_eq!(rel, "docs/alias.txt");
        assert_eq!(path, root.join("docs/real.txt").canonicalize().unwrap());
        assert_eq!(refused("docs/missing.txt").code, "not_found");
    }
}

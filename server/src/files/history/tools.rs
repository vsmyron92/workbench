//! MCP: `files_local_history` (read-only, confined to the session's project).

use std::path::Path;

use serde_json::{Value, json};

use super::routes::{dir_history_blocking, file_history_blocking};
use super::{EntryOut, describe};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::files::blocking;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};
use crate::util;

const DEFAULT_LIMIT: u64 = 30;
const MAX_LIMIT: u64 = 200;

pub fn tools() -> Vec<McpTool> {
    vec![tool(
        "files_local_history",
        "Workbench's Local History of a file or folder in your project: every version saved in Workbench, \
         every change seen on disk (including yours and other agents'), deletions and labels, newest first, \
         kept for 7 days. Use it to see what changed recently or to recover an earlier version. `path` is \
         relative to your project root (a folder, or \"\" for the whole project, lists recent changes of the \
         files in it). Pass `revision` (an entry id from the list) with `diff: true` for a unified diff of \
         what that revision changed (`against: \"current\"` compares it with the file on disk now), or \
         `content: true` for its full text. Sensitive files are never recorded.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Project-relative file or folder (\"\" = whole project)." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Entries to list (default 30)." },
                "revision": { "type": "integer", "minimum": 1, "description": "An entry id to diff or show." },
                "diff": { "type": "boolean", "description": "With `revision`: a unified diff." },
                "against": { "type": "string", "enum": ["previous", "current"], "description": "What the diff compares with (default previous)." },
                "content": { "type": "boolean", "description": "With `revision`: the full text of that version." },
                "projectId": { "type": "string", "description": "Only for callers that are not a project session." }
            },
            "required": ["path"]
        }),
        false,
        |state, ctx, args| async move { run(&state, &ctx, &args).await },
    )]
}

fn rows(entries: &[EntryOut], with_path: bool) -> Vec<Value> {
    entries
        .iter()
        .map(|e| {
            let mut v = json!({
                "id": e.id,
                "time": chrono::DateTime::from_timestamp_millis(e.ts).map(|t| t.to_rfc3339()).unwrap_or_default(),
                "kind": e.kind,
                "what": describe(e),
            });
            if with_path || e.kind.is_label() {
                v["path"] = json!(e.path);
            }
            if e.kind.has_content() {
                v["size"] = json!(e.size);
            }
            v
        })
        .collect()
}

async fn run(state: &AppState, ctx: &McpCtx, args: &Value) -> ApiResult<ToolOutput> {
    let pid = ctx.project_for(args.get("projectId").and_then(Value::as_str))?;
    let project = state.projects.require(&pid)?;
    let raw = args.get("path").and_then(Value::as_str).unwrap_or("").trim();
    // An absolute path inside the project is accepted too.
    let rel = match Path::new(raw).is_absolute() {
        true => util::paths::relative_to(&project.root, Path::new(raw)).ok_or_else(|| ApiError::forbidden("path is outside this session's project"))?,
        false => raw.trim_start_matches("./").trim_end_matches('/').to_string(),
    };
    let abs = util::paths::resolve_in_root(&project.root, &rel)?;
    let rel = util::paths::relative_to(&project.root, &abs).unwrap_or_default();
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;

    if let Some(id) = args.get("revision").and_then(Value::as_u64) {
        let want_diff = args.get("diff").and_then(Value::as_bool).unwrap_or(false);
        let want_content = args.get("content").and_then(Value::as_bool).unwrap_or(false);
        let against = args.get("against").and_then(Value::as_str).unwrap_or("previous").to_string();
        if against != "previous" && against != "current" {
            return Err(ApiError::bad_request("against is previous or current"));
        }
        let st = state.clone();
        let p = project.clone();
        let rel2 = rel.clone();
        return blocking(move || {
            let (e, content, _) = super::routes::load_revision(&st, &p, id)?;
            if !rel2.is_empty() && e.path != rel2 && !e.path.starts_with(&format!("{rel2}/")) {
                return Err(ApiError::bad_request(format!("entry {id} is not in {rel2}")));
            }
            let out = EntryOut::from(&e);
            let mut v = json!({ "revision": rows(std::slice::from_ref(&out), true)[0] });
            if want_diff || !want_content {
                let d = super::routes::diff_blocking(&st, &p, id, &against, 3)?;
                v["diff"] = json!(if d.diff.is_empty() { "(no differences)".to_string() } else { d.diff });
            }
            if want_content {
                v["content"] = json!(content.map(|b| String::from_utf8_lossy(&b).into_owned()));
            }
            Ok(ToolOutput::Json(v))
        })
        .await;
    }

    let st = state.clone();
    blocking(move || {
        let is_dir = rel.is_empty() || abs.is_dir();
        if is_dir {
            let h = dir_history_blocking(&st, &project, &rel, limit, None)?;
            Ok(ToolOutput::Json(json!({
                "path": h.path,
                "folder": true,
                "entries": rows(&h.entries, true),
                "more": h.truncated,
            })))
        } else {
            let h = file_history_blocking(&st, &project, &rel, limit, None)?;
            if h.untracked == Some("sensitive") {
                return Err(ApiError::forbidden(format!("{rel} is marked sensitive; Local History never records it")));
            }
            let mut v = json!({ "path": h.path, "entries": rows(&h.entries, false), "more": h.truncated });
            if let Some(why) = h.untracked {
                v["untracked"] = json!(why);
            }
            Ok(ToolOutput::Json(v))
        }
    })
    .await
}

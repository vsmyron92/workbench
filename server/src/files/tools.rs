//! MCP tools: let a hosted Claude session show the user a file.

use std::path::Path;

use serde_json::{Value, json};

use super::abs::allowed_roots;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};
use crate::util;

pub fn tools() -> Vec<McpTool> {
    vec![
        tool(
            "workbench_open_file",
            "Open a file in the Workbench editor that the user is looking at, optionally at a line. \
             `path` is relative to your project root, or absolute (inside a Workbench project or an \
             allowed scratch root). Use it to point the user at code you changed or want them to review. \
             For a rendered view of a Markdown file use workbench_open_markdown instead.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative or absolute path." },
                    "line": { "type": "integer", "minimum": 1, "description": "1-based line to reveal." },
                    "column": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"]
            }),
            false,
            |state, ctx, args| async move { open(&state, &ctx, &args, false).await },
        ),
        tool(
            "workbench_open_markdown",
            "Show a Markdown file rendered (live-updating preview) in Workbench, e.g. a plan or report \
             you wrote. `path` is relative to your project root, or absolute inside an allowed root.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Project-relative or absolute path." } },
                "required": ["path"]
            }),
            false,
            |state, ctx, args| async move { open(&state, &ctx, &args, true).await },
        ),
    ]
}

/// Where a tool path points: `(projectId, path)` with `path` project-relative, or
/// `(None, absolute)` for files in extra roots.
pub fn resolve_target(state: &AppState, ctx: &McpCtx, path: &str) -> ApiResult<(Option<String>, String)> {
    let path = path.trim();
    if path.is_empty() {
        return Err(ApiError::bad_request("path is required"));
    }
    let expanded = crate::config::expand_tilde(path);
    if expanded.is_absolute() {
        let abs = util::os::path::canonicalize(&expanded).map_err(|_| ApiError::not_found(format!("{} does not exist", expanded.display())))?;
        if let Some(p) = state.projects.find_by_path(&abs) {
            let rel = util::paths::relative_to(&p.root, &abs).unwrap_or_default();
            util::paths::resolve_in_root(&p.root, &rel)?;
            return Ok((Some(p.id.clone()), rel));
        }
        let roots = allowed_roots(state);
        let checked = util::paths::resolve_absolute_in(&roots, &abs.to_string_lossy())?;
        return Ok((None, checked.to_string_lossy().into_owned()));
    }
    let pid = ctx.project_id.clone().ok_or_else(|| {
        ApiError::bad_request("this session has no project; pass an absolute path")
    })?;
    let project = state.projects.require(&pid)?;
    let abs = util::paths::resolve_in_root(&project.root, path)?;
    if !abs.exists() {
        return Err(ApiError::not_found(format!("{path} does not exist in project {pid}")));
    }
    let rel = util::paths::relative_to(&project.root, &abs).unwrap_or_default();
    Ok((Some(pid), rel))
}

/// Panel id conventions (docs/ARCHITECTURE.md#panels); the web side uses the same.
pub fn panel_id(kind: &str, project_id: Option<&str>, path: &str) -> String {
    format!("{kind}:{}:{path}", project_id.unwrap_or(""))
}

async fn open(state: &AppState, ctx: &McpCtx, args: &Value, markdown: bool) -> Result<ToolOutput, ApiError> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
    let (pid, target) = resolve_target(state, ctx, path)?;
    let abs = match &pid {
        Some(id) => state.projects.require(id)?.root.join(&target),
        None => target.clone().into(),
    };
    if abs.is_dir() {
        return Err(ApiError::bad_request(format!("{path} is a directory")));
    }
    let title = Path::new(&target).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = if markdown { "markdown" } else { "editor" };
    let mut params = json!({ "projectId": pid, "path": target });
    if !markdown {
        if let Some(line) = args.get("line").and_then(Value::as_u64) {
            params["line"] = json!(line);
            params["column"] = json!(args.get("column").and_then(Value::as_u64).unwrap_or(1));
            // Lets the editor reveal the line again when the panel is already open.
            params["t"] = json!(util::now_ms());
        }
    }
    let id = panel_id(kind, pid.as_deref(), &target);
    // `ui.open` as emitted by `EventBus::ui_open`, plus the stable panel id.
    state.events.emit("ui.open", None, json!({ "panel": kind, "params": params, "title": title, "id": id }));
    let where_ = match &pid {
        Some(p) => format!("{target} (project {p})"),
        None => target.clone(),
    };
    let line = params.get("line").and_then(Value::as_u64).map(|l| format!(" at line {l}")).unwrap_or_default();
    Ok(ToolOutput::Text(format!("Opened {where_}{line} in Workbench{}.", if markdown { " as rendered Markdown" } else { "" })))
}

#[cfg(test)]
mod tests {
    use axum::http::Method;
    use serde_json::json;

    use crate::app::AppState;
    use crate::config::{GlobalConfig, Paths};
    use crate::mcp::{McpCtx, ToolOutput, call_api};

    #[test]
    fn panel_ids_follow_the_convention() {
        assert_eq!(super::panel_id("editor", Some("shop"), "src/main.rs"), "editor:shop:src/main.rs");
        assert_eq!(super::panel_id("markdown", None, "/tmp/x.md"), "markdown::/tmp/x.md");
    }

    /// A full in-process app over temp dirs: one project and one extra root.
    async fn app() -> (AppState, [tempfile::TempDir; 4], String) {
        let (cfg, data, proj, extra) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(proj.path().join("src")).unwrap();
        std::fs::write(proj.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(proj.path().join("PLAN.md"), "# Plan\n").unwrap();
        std::fs::write(extra.path().join("shot.md"), "# Notes\n").unwrap();
        let mut config = GlobalConfig::default();
        config.projects.roots = vec![];
        config.projects.include = vec![proj.path().canonicalize().unwrap().display().to_string()];
        config.extra_roots = vec![extra.path().display().to_string()];
        let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
        let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
        let _ = crate::app::build_router(state.clone());
        let pid = state.projects.list()[0].id.clone();
        (state, [cfg, data, proj, extra], pid)
    }

    async fn next_ui_open(rx: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<crate::events::Event>>) -> serde_json::Value {
        loop {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
            if ev.kind == "ui.open" {
                return ev.data.clone();
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_tools_open_panels_with_conventional_ids() {
        let (state, dirs, pid) = app().await;
        let tools = super::tools();
        let open_file = tools.iter().find(|t| t.name == "workbench_open_file").unwrap();
        let open_md = tools.iter().find(|t| t.name == "workbench_open_markdown").unwrap();
        let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: Some(pid.clone()) };
        let mut rx = state.events.subscribe();

        let out = (open_file.handler)(state.clone(), ctx.clone(), json!({ "path": "src/main.rs", "line": 1 })).await.unwrap();
        assert!(matches!(out, ToolOutput::Text(ref t) if t.contains("src/main.rs")));
        let d = next_ui_open(&mut rx).await;
        assert_eq!(d["panel"], "editor");
        assert_eq!(d["id"], format!("editor:{pid}:src/main.rs"));
        assert_eq!(d["params"]["projectId"], pid.as_str());
        assert_eq!(d["params"]["line"], 1);

        // Absolute path inside the project resolves to the project.
        let abs = dirs[2].path().canonicalize().unwrap().join("PLAN.md");
        (open_md.handler)(state.clone(), McpCtx::default(), json!({ "path": abs.display().to_string() })).await.unwrap();
        let d = next_ui_open(&mut rx).await;
        assert_eq!(d["panel"], "markdown");
        assert_eq!(d["params"]["path"], "PLAN.md");

        // Extra roots open read-only without a project.
        let scratch = dirs[3].path().join("shot.md");
        (open_file.handler)(state.clone(), McpCtx::default(), json!({ "path": scratch.display().to_string() })).await.unwrap();
        let d = next_ui_open(&mut rx).await;
        assert!(d["params"]["projectId"].is_null());

        // Refusals.
        assert!((open_file.handler)(state.clone(), McpCtx::default(), json!({ "path": "/etc/hostname" })).await.is_err());
        assert!((open_file.handler)(state.clone(), McpCtx::default(), json!({ "path": "src/main.rs" })).await.is_err());
        assert!((open_file.handler)(state.clone(), ctx.clone(), json!({ "path": "../outside.rs" })).await.is_err());
        assert!((open_file.handler)(state.clone(), ctx.clone(), json!({ "path": "missing.rs" })).await.is_err());
        assert!((open_file.handler)(state.clone(), ctx, json!({ "path": "src" })).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn routes_work_through_the_router() {
        let (state, _dirs, pid) = app().await;
        let ctx = McpCtx::default();
        let list = call_api(&state, Method::GET, &format!("/api/projects/{pid}/files/list?path=src"), None, &ctx).await.unwrap();
        assert_eq!(list["entries"][0]["path"], "src/main.rs");
        let read = call_api(&state, Method::GET, &format!("/api/projects/{pid}/files/read?path=src/main.rs"), None, &ctx).await.unwrap();
        let etag = read["etag"].as_str().unwrap().to_string();
        let body = json!({ "path": "src/main.rs", "content": "fn main() { println!(); }\n", "etag": etag });
        let w = call_api(&state, Method::PUT, &format!("/api/projects/{pid}/files/write"), Some(body.clone()), &ctx).await.unwrap();
        assert_ne!(w["etag"], etag);
        let err = call_api(&state, Method::PUT, &format!("/api/projects/{pid}/files/write"), Some(body), &ctx).await.unwrap_err();
        assert_eq!(err.status.as_u16(), 409);
        let found = call_api(&state, Method::GET, &format!("/api/projects/{pid}/files/find?q=main"), None, &ctx).await.unwrap();
        assert_eq!(found["results"][0]["path"], "src/main.rs");
        let hits = call_api(&state, Method::GET, &format!("/api/projects/{pid}/search?q=println"), None, &ctx).await.unwrap();
        assert_eq!(hits["matches"][0]["line"], 1);
        // A large write body (above axum's 2 MB default) is accepted.
        let big = "x".repeat(3 * 1024 * 1024);
        let body = json!({ "path": "big.txt", "content": big, "etag": null });
        call_api(&state, Method::PUT, &format!("/api/projects/{pid}/files/write"), Some(body), &ctx).await.unwrap();
    }

    /// A symlink pointing outside the project can be renamed and copied as a link,
    /// but nothing is ever read or written through it.
    #[tokio::test(flavor = "multi_thread")]
    async fn outside_symlinks_are_moved_as_links() {
        let (state, dirs, pid) = app().await;
        let (proj, outside) = (dirs[2].path(), dirs[3].path());
        crate::util::os::fs::symlink(outside, proj.join("out-link")).unwrap();
        let ctx = McpCtx::default();
        let op_url = format!("/api/projects/{pid}/files/op");
        let op = |body: serde_json::Value| call_api(&state, Method::POST, &op_url, Some(body), &ctx);
        op(json!({ "op": "rename", "path": "out-link", "to": "src/moved-link" })).await.unwrap();
        assert!(std::fs::symlink_metadata(proj.join("out-link")).is_err());
        assert_eq!(std::fs::read_link(proj.join("src/moved-link")).unwrap(), outside);
        op(json!({ "op": "copy", "path": "src/moved-link", "to": "copied-link" })).await.unwrap();
        assert_eq!(std::fs::read_link(proj.join("copied-link")).unwrap(), outside);
        assert_eq!(std::fs::read_to_string(outside.join("shot.md")).unwrap(), "# Notes\n");
        // Reads and writes through the link, or below it, stay refused.
        for path in ["copied-link", "copied-link/shot.md"] {
            let err = call_api(&state, Method::GET, &format!("/api/projects/{pid}/files/read?path={path}"), None, &ctx).await.unwrap_err();
            assert_eq!(err.status.as_u16(), 403, "{path}");
        }
        let err = op(json!({ "op": "rename", "path": "copied-link/shot.md", "to": "stolen.md" })).await.unwrap_err();
        assert_eq!(err.status.as_u16(), 403);
        let body = json!({ "path": "copied-link/new.md", "content": "x", "etag": null });
        let err = call_api(&state, Method::PUT, &format!("/api/projects/{pid}/files/write"), Some(body), &ctx).await.unwrap_err();
        assert_eq!(err.status.as_u16(), 403);
        assert!(!outside.join("new.md").exists());
    }
}

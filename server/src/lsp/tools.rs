//! MCP tools: read-only questions to the project's **running** language servers.
//! They never enable code intelligence and never start a server; a file an agent asks
//! about that no browser has open is opened on the server for that one request.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::manager::{ProjectLsp, TOOL_SOCKET};
use super::server::Server;
use super::uri::{self, ClientUri};
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

const NOT_RUNNING: &str = "Code intelligence is not running for this project: the user enables it in Workbench's editor, and a language server starts when a file of its language is open there. These tools never start one.";

pub fn tools() -> Vec<McpTool> {
    let project_prop = json!({ "type": "string", "description": "Workbench project id (default: this session's project)" });
    let pos_schema = json!({
        "type": "object",
        "properties": {
            "projectId": project_prop,
            "path": { "type": "string", "description": "File path relative to the project root" },
            "line": { "type": "integer", "minimum": 1, "description": "1-based line" },
            "column": { "type": "integer", "minimum": 1, "description": "1-based column (characters)" },
        },
        "required": ["path", "line", "column"],
    });
    vec![
        tool(
            "code_diagnostics",
            "Current errors and warnings from the project's running language servers (rust-analyzer, tsserver, pyright, gopls, clangd…), as the editor shows them. Optional `path` (a file or folder, project-relative) narrows the list. Read-only; never starts a language server, so it reports nothing when none runs.",
            json!({ "type": "object", "properties": { "projectId": project_prop, "path": { "type": "string", "description": "File or folder relative to the project root" } } }),
            false,
            |state, ctx, args| async move { diagnostics(&state, &ctx, &args).map(ToolOutput::Text) },
        ),
        tool(
            "code_symbols",
            "Search the project's symbols (functions, types, constants…) by name through its running language servers (workspace/symbol). Returns kind, name, container and file:line. Read-only; never starts a language server.",
            json!({ "type": "object", "properties": { "projectId": project_prop, "query": { "type": "string", "description": "Symbol name or part of it" } }, "required": ["query"] }),
            false,
            |state, ctx, args| async move { symbols(&state, &ctx, &args).await.map(ToolOutput::Text) },
        ),
        tool(
            "code_definition",
            "Where the symbol at path:line:column is defined, from the project's running language server (like Ctrl+B). Returns file:line:column with the line's text; library definitions outside the project are absolute paths. Read-only; never starts a language server.",
            pos_schema.clone(),
            false,
            |state, ctx, args| async move { at_position(&state, &ctx, &args, "textDocument/definition").await.map(ToolOutput::Text) },
        ),
        tool(
            "code_references",
            "Every usage of the symbol at path:line:column, from the project's running language server (Find Usages), as file:line:column with the line's text (at most 200). Read-only; never starts a language server.",
            pos_schema,
            false,
            |state, ctx, args| async move { at_position(&state, &ctx, &args, "textDocument/references").await.map(ToolOutput::Text) },
        ),
    ]
}

fn project_of(state: &AppState, ctx: &McpCtx, args: &Value) -> Result<(Arc<crate::projects::Project>, Option<Arc<ProjectLsp>>), ApiError> {
    let pid = ctx.project_for(args["projectId"].as_str())?;
    let p = state.projects.require(&pid)?;
    let lsp = if super::manager::enabled(state, &p) { state.lsp.get(&p.id) } else { None };
    Ok((p, lsp))
}

fn severity(d: &Value) -> &'static str {
    match d["severity"].as_u64().unwrap_or(1) {
        2 => "warning",
        3 => "info",
        4 => "hint",
        _ => "error",
    }
}

fn diagnostics(state: &AppState, ctx: &McpCtx, args: &Value) -> Result<String, ApiError> {
    let (_, lsp) = project_of(state, ctx, args)?;
    let Some(lsp) = lsp.filter(|l| !l.ready_servers().is_empty()) else {
        return Ok(NOT_RUNNING.into());
    };
    let filter = args["path"].as_str().map(|p| p.trim().trim_matches('/').to_string()).filter(|p| !p.is_empty());
    let mut rows: Vec<(u64, String, u64, u64, String)> = vec![];
    for (u, server, list) in lsp.diagnostics() {
        let Some(ClientUri::Project { rel, .. }) = uri::parse_client_uri(&u) else { continue };
        if let Some(f) = &filter {
            if rel != *f && !rel.starts_with(&format!("{f}/")) {
                continue;
            }
        }
        for d in list {
            let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
            let col = d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
            let src = [d["source"].as_str().unwrap_or(server.as_str()).to_string(), code_of(&d)].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
            let msg = d["message"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string();
            rows.push((d["severity"].as_u64().unwrap_or(1), rel.clone(), line, col, format!("{} [{src}]: {msg}", severity(&d))));
        }
    }
    if rows.is_empty() {
        return Ok(match filter {
            Some(f) => format!("No diagnostics under {f} from the running language servers."),
            None => "No diagnostics from the running language servers.".into(),
        });
    }
    rows.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
    let total = rows.len();
    let errors = rows.iter().filter(|r| r.0 <= 1).count();
    let warnings = rows.iter().filter(|r| r.0 == 2).count();
    let mut out = format!("{errors} errors, {warnings} warnings, {} other ({total} total)\n", total - errors - warnings);
    for (_, rel, line, col, text) in rows.iter().take(400) {
        out.push_str(&format!("{rel}:{line}:{col}: {text}\n"));
    }
    if total > 400 {
        out.push_str(&format!("… {} more\n", total - 400));
    }
    Ok(out)
}

fn code_of(d: &Value) -> String {
    match &d["code"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

const SYMBOL_KINDS: &[&str] = &[
    "", "File", "Module", "Namespace", "Package", "Class", "Method", "Property", "Field", "Constructor", "Enum", "Interface", "Function", "Variable",
    "Constant", "String", "Number", "Boolean", "Array", "Object", "Key", "Null", "EnumMember", "Struct", "Event", "Operator", "TypeParameter",
];

async fn symbols(state: &AppState, ctx: &McpCtx, args: &Value) -> Result<String, ApiError> {
    let (_, lsp) = project_of(state, ctx, args)?;
    let query = args["query"].as_str().unwrap_or("").trim().to_string();
    if query.is_empty() {
        return Err(ApiError::bad_request("query is required"));
    }
    let Some(lsp) = lsp else { return Ok(NOT_RUNNING.into()) };
    let servers: Vec<Arc<Server>> = lsp
        .ready_servers()
        .into_iter()
        .filter(|s| s.capabilities().get("workspaceSymbolProvider").is_some_and(|v| !v.is_null() && v != &Value::Bool(false)))
        .collect();
    if servers.is_empty() {
        return Ok(NOT_RUNNING.into());
    }
    let mut lines = vec![];
    for s in servers {
        let Ok(mut r) = s.request("workspace/symbol", json!({ "query": query }), Duration::from_secs(20)).await else { continue };
        {
            let mut allow = lsp.allow.lock();
            s.result_to_client(&mut r, &mut allow);
        }
        for sym in r.as_array().into_iter().flatten().take(200) {
            let kind = SYMBOL_KINDS.get(sym["kind"].as_u64().unwrap_or(0) as usize).copied().unwrap_or("");
            let name = sym["name"].as_str().unwrap_or("");
            let container = sym["containerName"].as_str().filter(|c| !c.is_empty()).map(|c| format!("  ({c})")).unwrap_or_default();
            let loc = &sym["location"];
            let where_ = display_location(loc["uri"].as_str().unwrap_or(""), loc["range"]["start"].as_object().map(|_| &loc["range"]["start"]));
            lines.push(format!("{kind} {name}{container}  {where_}"));
        }
    }
    if lines.is_empty() {
        return Ok(format!("No symbols match {query:?}."));
    }
    let total = lines.len();
    lines.truncate(150);
    let mut out = lines.join("\n");
    if total > 150 {
        out.push_str(&format!("\n… {} more", total - 150));
    }
    Ok(out)
}

fn display_path(u: &str) -> String {
    match uri::parse_client_uri(u) {
        Some(ClientUri::Project { rel, .. }) => rel,
        Some(ClientUri::Source { path, .. }) => path,
        None => u.to_string(),
    }
}

fn display_location(u: &str, start: Option<&Value>) -> String {
    let p = display_path(u);
    match start {
        Some(s) => format!("{p}:{}:{}", s["line"].as_u64().unwrap_or(0) + 1, s["character"].as_u64().unwrap_or(0) + 1),
        None => p,
    }
}

/// 1-based character column → 0-based UTF-16 offset on `line_text`.
pub fn utf16_col(line_text: &str, column: usize) -> usize {
    line_text.chars().take(column.saturating_sub(1)).map(char::len_utf16).sum()
}

/// 0-based UTF-16 offset → 1-based character column.
pub fn char_col(line_text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in line_text.chars().enumerate() {
        if units >= utf16 {
            return i + 1;
        }
        units += c.len_utf16();
    }
    line_text.chars().count() + 1
}

async fn at_position(state: &AppState, ctx: &McpCtx, args: &Value, method: &str) -> Result<String, ApiError> {
    let (p, lsp) = project_of(state, ctx, args)?;
    let path = args["path"].as_str().ok_or_else(|| ApiError::bad_request("path is required"))?;
    let line = args["line"].as_u64().filter(|l| *l >= 1).ok_or_else(|| ApiError::bad_request("line is 1-based"))? as usize;
    let column = args["column"].as_u64().filter(|c| *c >= 1).ok_or_else(|| ApiError::bad_request("column is 1-based"))? as usize;
    let abs = crate::util::paths::resolve_in_root(&p.root, path)?;
    let rel = crate::util::paths::relative_to(&p.root, &abs).unwrap_or_default();
    let Some(lsp) = lsp else { return Ok(NOT_RUNNING.into()) };
    let doc_uri = uri::project_uri(&p.id, &rel);
    let server_id = match lsp.doc_server(&doc_uri) {
        Some(s) => Some(s),
        None => lsp.choose(state, &rel, None).await.map(|s| s.id.clone()),
    };
    let Some(server) = server_id.as_deref().and_then(|s| lsp.ready_server(s)) else {
        return Ok(format!("No running language server handles {rel}. {NOT_RUNNING}"));
    };
    // A file no browser has open is opened for this request only.
    let temp = if lsp.doc_text(&doc_uri).is_none() {
        let abs2 = abs.clone();
        let text = tokio::task::spawn_blocking(move || std::fs::read(&abs2))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
        let text = String::from_utf8(text).map_err(|_| ApiError::bad_request("not a UTF-8 text file"))?;
        lsp.open(state, TOOL_SOCKET, &doc_uri, None, text, false).await.map_err(ApiError::bad_request)?;
        true
    } else {
        false
    };
    let result = async {
        let text = lsp.doc_text(&doc_uri).unwrap_or_default();
        let line_text = text.lines().nth(line - 1).ok_or_else(|| ApiError::bad_request(format!("{rel} has no line {line}")))?;
        let character = utf16_col(line_text, column);
        let mut params = json!({ "textDocument": { "uri": doc_uri }, "position": { "line": line - 1, "character": character } });
        if method == "textDocument/references" {
            params["context"] = json!({ "includeDeclaration": true });
        }
        {
            let allow = lsp.allow.lock();
            server.params_to_server(&mut params, &allow);
        }
        server.touch();
        let mut r = server.request(method, params, Duration::from_secs(60)).await.map_err(|e| ApiError::upstream(e.to_string()))?;
        {
            let mut allow = lsp.allow.lock();
            server.result_to_client(&mut r, &mut allow);
        }
        Ok::<_, ApiError>(format_locations(&lsp, &p.root, r, method).await)
    }
    .await;
    if temp {
        lsp.close(state, TOOL_SOCKET, &doc_uri);
    }
    result
}

/// Locations (Location | LocationLink, one or many) as `path:line:col  text`.
async fn format_locations(lsp: &ProjectLsp, root: &std::path::Path, r: Value, method: &str) -> String {
    let items: Vec<Value> = match r {
        Value::Array(a) => a,
        Value::Null => vec![],
        v => vec![v],
    };
    let mut locs: Vec<(String, u64, u64)> = items
        .iter()
        .filter_map(|l| {
            let u = l["uri"].as_str().or_else(|| l["targetUri"].as_str())?.to_string();
            let start = if l.get("targetSelectionRange").is_some() { &l["targetSelectionRange"]["start"] } else { &l["range"]["start"] };
            Some((u, start["line"].as_u64().unwrap_or(0), start["character"].as_u64().unwrap_or(0)))
        })
        .collect();
    if locs.is_empty() {
        return if method == "textDocument/references" { "No usages found.".into() } else { "No definition found.".into() };
    }
    locs.sort();
    locs.dedup();
    let total = locs.len();
    locs.truncate(200);
    // Line text: from open documents, else the file (project files and host sources).
    let mut texts: HashMap<String, Option<Arc<String>>> = HashMap::new();
    let mut out = String::new();
    if method == "textDocument/references" {
        out.push_str(&format!("{total} usages\n"));
    }
    for (u, line, ch) in &locs {
        if !texts.contains_key(u) {
            let t = match lsp.doc_text(u) {
                Some(t) => Some(t),
                None => {
                    let path = match uri::parse_client_uri(u) {
                        Some(ClientUri::Project { rel, .. }) => Some(root.join(rel)),
                        Some(ClientUri::Source { path, .. }) if matches!(lsp.allow.lock().get(&path), Some(uri::Origin::Host)) => Some(path.into()),
                        _ => None,
                    };
                    match path {
                        Some(p) => tokio::task::spawn_blocking(move || {
                            std::fs::metadata(&p).ok().filter(|m| m.len() < 5 * 1024 * 1024)?;
                            std::fs::read_to_string(&p).ok().map(Arc::new)
                        })
                        .await
                        .ok()
                        .flatten(),
                        None => None,
                    }
                }
            };
            texts.insert(u.clone(), t);
        }
        let line_text = texts.get(u).cloned().flatten().and_then(|t| t.lines().nth(*line as usize).map(str::to_string)).unwrap_or_default();
        let col = char_col(&line_text, *ch as usize);
        let snippet: String = line_text.trim().chars().take(160).collect();
        out.push_str(&format!("{}:{}:{}  {snippet}\n", display_path(u), line + 1, col));
    }
    if total > locs.len() {
        out.push_str(&format!("… {} more\n", total - locs.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_convert_between_characters_and_utf16() {
        let line = "let é = \"😀x\";";
        assert_eq!(utf16_col(line, 1), 0);
        assert_eq!(utf16_col(line, 5), 4);
        // After the emoji (two UTF-16 units).
        let x = line.chars().position(|c| c == 'x').unwrap() + 1;
        assert_eq!(utf16_col(line, x), x);
        assert_eq!(char_col(line, utf16_col(line, x)), x);
        assert_eq!(char_col(line, 0), 1);
        assert_eq!(char_col("ab", 99), 3);
    }
}

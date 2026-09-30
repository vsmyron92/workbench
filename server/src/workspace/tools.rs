//! MCP tools: agents register their deliverables as Workspace cards.
//!
//! The descriptions are the authoring guide (condensed from Mr. Mak Workspace's
//! `workspace-authoring` skill, MIT). A hosted session works in its own project's
//! scope or in `home`, never in another project's.

use std::path::PathBuf;

use base64::Engine;
use serde_json::{Value, json};

use super::routes::{self, PatchBody, StepBody};
use super::store::{self, CardOut, NewCard};
use super::{blocking, emit_changed};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

/// Text or decoded bytes written by `workspace_write_file`.
const MAX_WRITE_BYTES: usize = 20 * 1024 * 1024;

const GUIDE: &str = "Workspace cards are how you hand deliverables to the user: reports, docs, image sets and 3D comparisons, \
shown as tabs (steps) in Workbench's Workspace, next to the code. Put related iterations into ONE card as extra steps \
(\"v2\", \"v3\") instead of creating many cards; call workspace_list_cards first and reuse a fitting card.";

const AUTHORING: &str = "Authoring rules: HTML reports are sandboxed and must be self-contained: relative paths only, no CDN \
scripts, web fonts or remote images (they are blocked); inline or local CSS/JS is fine. Link ../_shared/report.css and \
../_shared/report.js and put data-wb-report=\"document\" on <html> for the standard dark report look plus a click-to-zoom \
image lightbox (classes: card, grid-2, grid-3, gallery, kpis/kpi, good/warn/bad). Give every image alt text and \
loading=\"lazy\"; keep wide tables scrollable. Markdown uses relative image paths. A folder of images is one step \
(viewer gallery). A 3D comparison is a JSON manifest step with viewer compare3d: {\"title\"?, \"tests\": [{\"id\", \"name\", \
\"kind\": \"lowpoly\"|\"highpoly\", \"defaultMode\"?: wire|solid|normals|pbr|albedo|normalMap|rough|metal, \"note\"?, \
\"input\"?: reference image, \"models\": [{\"file\": \"models/a.glb\", \"label\", \"note\"?, \"accent\"?, \"rotationY\"?, \
\"alias\"?}]}]} with paths relative to the manifest; a single .glb step also opens in the 3D viewer.";

fn scope_for(ctx: &McpCtx, requested: Option<&str>) -> ApiResult<String> {
    match requested.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s @ (store::HOME | store::SANDBOX)) => Ok(s.into()),
        Some(r) => ctx.project_for(Some(r)),
        None => Ok(ctx.project_id.clone().unwrap_or_else(|| store::HOME.into())),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn required<'a>(args: &'a Value, key: &str) -> ApiResult<&'a str> {
    str_arg(args, key).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| ApiError::bad_request(format!("{key} is required")))
}

/// What an agent needs to know about a card.
fn brief(c: &CardOut) -> Value {
    json!({
        "cardId": c.id,
        "scope": c.scope,
        "title": c.title,
        "description": c.description,
        "category": c.category,
        "status": c.status,
        "pinned": c.pinned,
        "archived": c.archived,
        "updated": c.updated.as_deref().unwrap_or(&c.created),
        "editable": c.editable,
        "folder": c.folder_path,
        "steps": c.steps.iter().map(|s| json!({ "index": s.index, "name": s.name, "path": s.path, "kind": s.kind, "exists": s.exists })).collect::<Vec<_>>(),
    })
}

/// Roots an agent may copy step files from: its own project's.
fn import_roots(state: &AppState, ctx: &McpCtx) -> Vec<PathBuf> {
    ctx.project_id.as_deref().and_then(|p| state.projects.get(p)).map(|p| vec![p.root.clone()]).unwrap_or_default()
}

pub fn tools() -> Vec<McpTool> {
    vec![
        tool(
            "workspace_list_cards",
            &format!(
                "List Workspace cards (deliverables) in a scope, pinned and freshest first. {GUIDE} Archived cards (explicitly, or \
                 untouched for 7 days) are left out unless includeArchived is true."
            ),
            json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "description": "\"home\", \"wb-sandbox\" (throwaway experiments) or a project id; default: this session's project, else home" },
                    "query": { "type": "string", "description": "Filter by words in the title, description, category or id" },
                    "includeArchived": { "type": "boolean" }
                }
            }),
            false,
            |state, ctx, args| async move {
                let scope = store::scope(&state, &scope_for(&ctx, str_arg(&args, "scope"))?)?;
                let q = str_arg(&args, "query").unwrap_or("").trim().to_lowercase();
                let archived = args.get("includeArchived").and_then(Value::as_bool).unwrap_or(false);
                let st = state.clone();
                let (cards, warnings) = blocking(move || store::scope_cards(&st, &scope)).await?;
                let list: Vec<Value> = cards
                    .iter()
                    .filter(|c| archived || !q.is_empty() || !c.archived)
                    .filter(|c| q.is_empty() || [&c.title, &c.description, &c.category, &c.id].iter().any(|s| s.to_lowercase().contains(&q)))
                    .map(brief)
                    .collect();
                Ok(ToolOutput::Json(json!({ "cards": list, "warnings": warnings })))
            },
        ),
        tool(
            "workspace_create_card",
            &format!(
                "Create a Workspace card and get its folder. {GUIDE} Returns {{cardId, folder}}: write the card's files into \
                 `folder` (an absolute path; or use workspace_write_file), then register each file the user should see as a \
                 tab with workspace_add_step. {AUTHORING}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short, readable title" },
                    "description": { "type": "string", "description": "One or two sentences: what this is and why" },
                    "category": { "type": "string", "description": "Free-form group, e.g. research, report, design, image-gen, analytics, dev" },
                    "scope": { "type": "string", "description": "\"home\", \"wb-sandbox\" (throwaway experiments) or a project id; default: this session's project, else home" }
                },
                "required": ["title", "description"]
            }),
            true,
            |state, ctx, args| async move {
                let scope = store::scope(&state, &scope_for(&ctx, str_arg(&args, "scope"))?)?;
                let input = NewCard {
                    title: required(&args, "title")?.to_string(),
                    description: str_arg(&args, "description").unwrap_or("").to_string(),
                    category: str_arg(&args, "category").unwrap_or("").to_string(),
                    icon: None,
                };
                let c = routes::create_card(&state, scope, input).await?;
                Ok(ToolOutput::Json(json!({
                    "cardId": c.id,
                    "scope": c.scope,
                    "folder": c.folder_path,
                    "sharedAssets": "../_shared/report.css and ../_shared/report.js (relative to the folder)",
                    "next": "Write files into folder, then call workspace_add_step for each tab, then workspace_open_card.",
                })))
            },
        ),
        tool(
            "workspace_add_step",
            &format!(
                "Add a tab (step) to a Workspace card. `path` is relative to the card folder (preferred), or an absolute path \
                 inside the card folder or inside this project (files elsewhere in the project are copied into the card). \
                 viewer: auto (by extension; a folder becomes a gallery) | html | markdown | image | gallery | compare3d | pdf \
                 | video | audio | text. {AUTHORING}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "cardId": { "type": "string" },
                    "name": { "type": "string", "description": "Tab label, e.g. \"Summary\" or \"v2 · tighter layout\"" },
                    "path": { "type": "string" },
                    "viewer": { "type": "string", "enum": super::model::VIEWERS },
                    "scope": { "type": "string" }
                },
                "required": ["cardId", "name", "path"]
            }),
            true,
            |state, ctx, args| async move {
                let scope = store::scope(&state, &scope_for(&ctx, str_arg(&args, "scope"))?)?;
                let body = StepBody {
                    name: required(&args, "name")?.to_string(),
                    path: required(&args, "path")?.to_string(),
                    viewer: str_arg(&args, "viewer").map(str::to_string),
                };
                let roots = import_roots(&state, &ctx);
                let (c, index) = routes::add_step_to(&state, scope, required(&args, "cardId")?.to_string(), body, roots).await?;
                let step = &c.steps[index];
                let mut out = json!({ "cardId": c.id, "step": { "index": index, "name": step.name, "path": step.path, "kind": step.kind, "exists": step.exists } });
                if !step.exists {
                    out["warning"] = json!(format!("{} does not exist in the card folder yet; write it to {}/{}", step.path, c.folder_path, step.path));
                }
                Ok(ToolOutput::Json(out))
            },
        ),
        tool(
            "workspace_update_card",
            "Update a Workspace card: status (active | done | archived), pinned, title, description or category. Any update \
             also marks the card as freshly touched (call it after changing a card's files so it rises to the top). Cards \
             from a repository's workspace/workspace.json (ids starting with \"repo:\") only take status and pinned.",
            json!({
                "type": "object",
                "properties": {
                    "cardId": { "type": "string" },
                    "status": { "type": "string", "enum": super::model::STATUSES },
                    "pinned": { "type": "boolean" },
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "category": { "type": "string" },
                    "scope": { "type": "string" }
                },
                "required": ["cardId"]
            }),
            true,
            |state, ctx, args| async move {
                let scope = store::scope(&state, &scope_for(&ctx, str_arg(&args, "scope"))?)?;
                let id = required(&args, "cardId")?.to_string();
                let patch = PatchBody {
                    title: str_arg(&args, "title").map(str::to_string),
                    description: str_arg(&args, "description").map(str::to_string),
                    category: str_arg(&args, "category").map(str::to_string),
                    status: str_arg(&args, "status").map(str::to_string),
                    pinned: args.get("pinned").and_then(Value::as_bool),
                    ..Default::default()
                };
                let nothing = patch.title.is_none() && patch.description.is_none() && patch.category.is_none() && patch.status.is_none() && patch.pinned.is_none();
                let c = if nothing {
                    // Just "touched": keep everything, bump `updated`.
                    routes::touch_card(&state, scope, id).await?
                } else {
                    routes::patch_card(&state, scope, id, patch).await?
                };
                Ok(ToolOutput::Json(brief(&c)))
            },
        ),
        tool(
            "workspace_write_file",
            "Write a file into a Workspace card's folder (creating folders as needed, replacing an existing file). Use it when \
             you cannot write to the card folder directly. content is UTF-8 text, or base64 with encoding \"base64\" (images). \
             Then register it with workspace_add_step.",
            json!({
                "type": "object",
                "properties": {
                    "cardId": { "type": "string" },
                    "path": { "type": "string", "description": "Relative to the card folder, e.g. report.html or img/chart.png" },
                    "content": { "type": "string" },
                    "encoding": { "type": "string", "enum": ["utf8", "base64"] },
                    "scope": { "type": "string" }
                },
                "required": ["cardId", "path", "content"]
            }),
            true,
            |state, ctx, args| async move {
                let scope = store::scope(&state, &scope_for(&ctx, str_arg(&args, "scope"))?)?;
                let id = required(&args, "cardId")?.to_string();
                let rel = required(&args, "path")?.to_string();
                let content = str_arg(&args, "content").unwrap_or("");
                let bytes = match str_arg(&args, "encoding").unwrap_or("utf8") {
                    "base64" => base64::engine::general_purpose::STANDARD
                        .decode(content.trim())
                        .map_err(|e| ApiError::bad_request(format!("content is not valid base64: {e}")))?,
                    _ => content.as_bytes().to_vec(),
                };
                if bytes.len() > MAX_WRITE_BYTES {
                    return Err(ApiError::bad_request(format!("files written this way are limited to {} MB", MAX_WRITE_BYTES >> 20)));
                }
                let _w = state.workspace.write_lock.lock().await;
                let project = scope.project_id().map(str::to_string);
                let scope_id = scope.id.clone();
                let (path, size, public_id) = blocking(move || {
                    let loc = store::find(&scope, &id)?;
                    if !loc.editable() {
                        return Err(ApiError::forbidden("repository cards are edited in the project itself"));
                    }
                    let dir = loc.dir()?;
                    let (abs, rel) = store::resolve_in_card(&dir, &rel)?;
                    if rel.is_empty() {
                        return Err(ApiError::bad_request("path must name a file"));
                    }
                    crate::util::fs::write_atomic(&abs, &bytes, 0o644)?;
                    store::touch(&loc)?;
                    Ok((rel, bytes.len(), loc.public_id()))
                })
                .await?;
                emit_changed(&state, &scope_id, project.as_deref(), Some(&public_id));
                Ok(ToolOutput::Json(json!({ "cardId": public_id, "path": path, "size": size })))
            },
        ),
        tool(
            "workspace_open_card",
            "Show a Workspace card to the user in Workbench (optionally at a step index). Do this once a deliverable is ready.",
            json!({
                "type": "object",
                "properties": {
                    "cardId": { "type": "string" },
                    "step": { "type": "integer", "minimum": 0 },
                    "scope": { "type": "string" }
                },
                "required": ["cardId"]
            }),
            false,
            |state, ctx, args| async move {
                let scope = store::scope(&state, &scope_for(&ctx, str_arg(&args, "scope"))?)?;
                let id = required(&args, "cardId")?.to_string();
                let step = args.get("step").and_then(Value::as_u64);
                let st = state.clone();
                let c = blocking(move || store::one_card(&st, &scope, &id)).await?;
                if let Some(s) = step {
                    if s as usize >= c.steps.len() {
                        return Err(ApiError::bad_request(format!("the card has {} steps", c.steps.len())));
                    }
                }
                let mut params = json!({ "scope": c.scope, "cardId": c.id });
                if let Some(s) = step {
                    params["step"] = json!(s);
                }
                state.events.ui_open_id("card", &format!("card:{}:{}", c.scope, c.id), params, Some(&c.title));
                Ok(ToolOutput::Text(format!("Opened \"{}\" in Workbench.", c.title)))
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_stay_in_their_scope() {
        let agent = McpCtx { terminal_id: Some("t".into()), project_id: Some("shop".into()) };
        assert_eq!(scope_for(&agent, None).unwrap(), "shop");
        assert_eq!(scope_for(&agent, Some("home")).unwrap(), "home");
        assert_eq!(scope_for(&agent, Some("wb-sandbox")).unwrap(), "wb-sandbox");
        assert_eq!(scope_for(&agent, Some("other")).unwrap_err().code, "forbidden");
        let loose = McpCtx { terminal_id: Some("t".into()), project_id: None };
        assert_eq!(scope_for(&loose, None).unwrap(), "home");
        assert_eq!(scope_for(&loose, Some("shop")).unwrap_err().code, "forbidden");
        let master = McpCtx::default();
        assert_eq!(scope_for(&master, None).unwrap(), "home");
        assert_eq!(scope_for(&master, Some("other")).unwrap(), "other");
    }
}

//! MCP: `devcontainer_status` (read-only). Agents never start, rebuild, stop or remove
//! a dev container: building one runs repository-defined code on the host's Docker.

use serde_json::{Value, json};

use crate::mcp::{McpTool, ToolOutput, tool};

pub fn tools() -> Vec<McpTool> {
    vec![tool(
        "devcontainer_status",
        "Dev container of a project: its devcontainer.json files, state (none|stopped|running|building|error), \
         the container (name, image, ports and how to reach them), whether shells and run configurations run \
         inside it, the remote user and workspace folder, and the risks the user reviewed. Read-only: only the \
         user can start, rebuild, stop or remove a dev container.",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": "Workbench project id; defaults to the calling session's project." }
        } }),
        false,
        |state, ctx, args| async move {
            let pid = ctx.project_for(args.get("projectId").and_then(Value::as_str))?;
            let p = state.projects.require(&pid)?;
            let mut v = super::ops::view(&state, &p, None).await?;
            // The plan's hash is the approval token of the UI; agents get the substance.
            if let Some(plan) = v.get_mut("plan").and_then(Value::as_object_mut) {
                plan.remove("hash");
            }
            Ok(ToolOutput::Json(v))
        },
    )]
}

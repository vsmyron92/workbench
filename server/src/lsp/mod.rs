//! Code intelligence slice (OWNER: lsp slice).
//!
//! Language servers (rust-analyzer, typescript-language-server, pyright, gopls,
//! clangd, …) per project and language, bridged to the editor: diagnostics, hover,
//! completion, signature help, go to definition / implementation / type definition,
//! find usages, document and workspace symbols, rename, formatting, code actions.
//!
//! **Trust.** A language server runs project code (build scripts, proc macros, the
//! project's own TypeScript, `go list`…). Nothing starts by itself: a project's servers
//! run only after the user enabled code intelligence for that project (remembered in
//! `data_dir/lsp/<id>.json`), and server commands come from `config.toml` presets
//! only, never from repository config. MCP tools only use servers that already run.
//!
//! Layout:
//! * `config`   — `[lsp]`, the presets, where commands are (PATH, rustup);
//! * `jsonrpc`  — `Content-Length` framing;
//! * `uri`      — browser ↔ host ↔ server (container) URIs, the `lsp-src` allow-set;
//! * `server`   — one process: spawn, initialize, requests, server requests, stop;
//! * `launch`   — host or dev container (`docker exec`, no TTY);
//! * `manager`  — per project: documents, server slots, diagnostics, sockets, crashes;
//! * `ws`, `routes` — the editor's socket and the REST routes;
//! * `watch`    — `fs.changed` → `workspace/didChangeWatchedFiles`;
//! * `tools`    — MCP tools (read-only);
//! * `trust`    — per-project enablement and settings.
//!
//! Routes: `/api/projects/{pid}/lsp/**`. Events: `lsp.state`, `lsp.diagnostics`.

mod config;
mod jsonrpc;
mod launch;
mod manager;
mod routes;
mod server;
mod tools;
mod trust;
mod uri;
mod watch;
mod ws;

#[cfg(test)]
mod tests;

use std::time::Duration;

use axum::Router;

use crate::app::AppState;
use crate::mcp::McpTool;

pub use config::LspConfig;
pub use manager::LspState;

pub fn router() -> Router<AppState> {
    routes::router()
}

/// Background work: the idle sweeper, files → servers, projects removed, settings.
pub async fn start(state: &AppState) {
    let st = state.clone();
    tokio::spawn(async move {
        let mut rx = st.events.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let idle = st.config.read().lsp.idle();
                    for p in st.lsp.all() {
                        let st2 = st.clone();
                        tokio::spawn(async move { p.sweep_idle(&st2, idle).await });
                    }
                }
                ev = rx.recv() => match ev {
                    Ok(ev) => match ev.kind.as_str() {
                        "fs.changed" => {
                            if let Some(pid) = ev.project_id.clone() {
                                if st.lsp.get(&pid).is_some() {
                                    let st2 = st.clone();
                                    tokio::spawn(async move { watch::on_fs_changed(&st2, &pid, &ev.data).await });
                                }
                            }
                        }
                        "projects.changed" => reconcile(&st).await,
                        "settings.changed" => st.lsp.avail.clear(),
                        _ => {}
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => reconcile(&st).await,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    });
}

/// Projects that went away (or moved) take their servers with them.
async fn reconcile(state: &AppState) {
    for lsp in state.lsp.all() {
        let keep = state.projects.get(&lsp.id).is_some_and(|p| p.root == lsp.root);
        if !keep {
            state.lsp.remove(&lsp.id);
            lsp.disconnect_all(true);
            let st = state.clone();
            tokio::spawn(async move { lsp.shutdown(&st).await });
        }
    }
}

/// Stop every language server (Workbench is exiting).
pub async fn shutdown(state: &AppState) {
    let all = state.lsp.all();
    let stops = all.iter().map(|p| p.shutdown(state));
    let _ = tokio::time::timeout(Duration::from_secs(8), futures::future::join_all(stops)).await;
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

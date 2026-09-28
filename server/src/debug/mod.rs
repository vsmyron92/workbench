//! Debugger slice (OWNER: debug slice).
//!
//! Debug sessions through the Debug Adapter Protocol (gdb's built-in DAP, lldb-dap,
//! CodeLLDB, debugpy, delve, any other adapter from config.toml): launch or attach,
//! breakpoints (line, conditional, hit counts, logpoints, functions, exceptions),
//! stepping, threads and call stacks, variables, watches, a debug console.
//!
//! **Trust.** Adapters come from `config.toml` presets and `[debug.adapters.<id>]`
//! only. Launch configurations may come from repository config (`[[debug]]`, like
//! run configurations: they run only on a click) and name adapters by id. Nothing
//! starts by itself; agents (MCP) can read the state of a session but never start,
//! step, evaluate in or stop one: every write route refuses in-process callers.
//!
//! Modules: `protocol` (DAP framing), `client` (requests with timeouts, events,
//! reverse requests), `process` (adapter processes: host or dev container, stdio or
//! TCP), `adapters` (presets, config, availability), `launch` (launch
//! configurations, plans, adapter dialects), `derive` (Cargo, CMake, Python, Go),
//! `breakpoints` (the per-project store), `session` (session manager and event
//! loop), `procs` (attach picker), `routes`, `tools` (MCP `debug_state`).
//!
//! Routes: `/api/projects/{pid}/debug/**` (see `routes`). Events: `debug.session`,
//! `debug.output`, `debug.breakpoints`.

pub mod adapters;
pub mod breakpoints;
pub mod client;
pub mod derive;
pub mod launch;
pub mod process;
pub mod procs;
pub mod protocol;
mod routes;
pub mod session;
#[cfg(test)]
mod tests;
mod tools;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::app::AppState;
use crate::mcp::McpTool;

pub use adapters::DebugConfig;
pub use routes::router;
use session::Session;

#[derive(Default)]
pub struct DebugState {
    sessions: RwLock<Vec<Arc<Session>>>,
    pub(crate) store: breakpoints::Store,
    pub(crate) probes: Mutex<HashMap<String, (Instant, adapters::Availability)>>,
    /// Exception filters adapters announced, by adapter id: `(label, filters)`, so the
    /// Breakpoints view can offer them before the next session starts.
    pub(crate) known_filters: Mutex<HashMap<String, (String, Value)>>,
    pub(crate) shutdown: CancellationToken,
    /// Child sessions (`startDebugging`) are started through this channel.
    pub(crate) children: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<session::ChildRequest>>,
}

impl DebugState {
    fn insert(&self, s: Arc<Session>) {
        self.sessions.write().push(s);
    }

    pub fn all(&self) -> Vec<Arc<Session>> {
        self.sessions.read().clone()
    }

    pub fn sessions_of(&self, pid: &str) -> Vec<Arc<Session>> {
        self.sessions.read().iter().filter(|s| s.project_id == pid).cloned().collect()
    }

    pub fn get(&self, pid: &str, id: &str) -> Option<Arc<Session>> {
        self.sessions.read().iter().find(|s| s.id == id && s.project_id == pid).cloned()
    }

    /// Forget ended sessions of `pid` beyond the newest `keep`; the ids forgotten
    /// (the caller tells the UI: `debug.session {removed: true}`).
    fn prune(&self, pid: &str, keep: usize) -> Vec<String> {
        let mut all = self.sessions.write();
        let ended: Vec<String> = all.iter().filter(|s| s.project_id == pid && !s.is_live()).map(|s| s.id.clone()).collect();
        if ended.len() <= keep {
            return vec![];
        }
        let drop: Vec<String> = ended[..ended.len() - keep].to_vec();
        all.retain(|s| !drop.contains(&s.id));
        drop
    }

    fn remove(&self, id: &str) {
        self.sessions.write().retain(|s| s.id != id);
    }
}

pub async fn start(state: &AppState) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    if state.debug.children.set(tx).is_ok() {
        tokio::spawn(session::children(state.clone(), rx));
    }
}

/// End every debug session (terminating debuggees Workbench launched).
pub async fn shutdown(state: &AppState) {
    session::shutdown(state).await;
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

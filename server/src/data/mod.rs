//! Data viewer slice (OWNER: data slice).
//!
//! Live measurements next to the agent session: an agent (or a script it runs, or a file its
//! capture tool writes) puts samples into named **series**, and saved **plots** (lanes of traces
//! under one time axis, like an oscilloscope or logic analyzer) draw them in the browser while the
//! capture runs. Logic analyzer captures, differential probe and scope waveforms, power
//! measurements: anything that is a value, a level or a note over time.
//!
//! Samples live in memory (rolling, bounded); the project keeps the series' definitions and its
//! plots on disk. Everything that writes goes through [`store::DataState`], so a sender's way in
//! (MCP tool, REST, the hook scripts post to, a followed file) changes nothing about the rules.
//!
//! Modules: `series` (storage, thinned windows, statistics), `plots` (lane layouts), `store`
//! (the per-project state, persistence, announcements), `formats` (CSV, JSON lines and VCD
//! decoders), `sources` (followed files), `routes`, `tools` (MCP).
//!
//! Routes: `/api/projects/{pid}/data/**`, `/api/hooks/data/{terminalId}/append`.
//! Events: `data.series`, `data.changed`, `data.plots`, `data.sources`.

pub mod formats;
pub mod plots;
mod routes;
pub mod series;
pub mod sources;
pub mod store;
#[cfg(test)]
mod tests;
mod tools;

use std::time::Duration;

use crate::app::AppState;
use crate::mcp::McpTool;

pub use routes::router;
pub use store::DataState;

/// How often changed series are announced to the browsers: a plot redraws at most this often.
const TICK: Duration = Duration::from_millis(100);

pub async fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if state.data.any_dirty() {
                state.data.flush_dirty(&state);
            }
        }
    });
}

/// Stop followed files.
pub async fn shutdown(state: &AppState) {
    state.data.sources.shutdown();
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

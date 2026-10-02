//! Platform slice (OWNER: platform slice).
//!
//! * `/mcp` — the Workbench MCP server hosted Claude sessions use to drive the
//!   workspace (`mcp_server.rs`); platform tools in `tools.rs`.
//! * Notifications — desktop (`notify-send`) and a user command for agent
//!   attention, environment outages and deploys, plus the activity log the UI
//!   shows (`notify.rs`, `activity.rs`).
//! * Settings — `config.toml` (structured and raw), secret status, per-project
//!   config layers (`settings.rs`, `config_edit.rs`).
//! * Remote access — bind/LAN addresses, allowed hosts, pairing codes with QR,
//!   devices (`remote.rs`, `netif.rs`), and optional TLS serving (`tls.rs`).
//! * Web Push to phones and other devices (`push/`), and `workbench service`:
//!   a systemd user unit and a desktop launcher (`service.rs`); on Windows a sign-in
//!   entry and a Start Menu shortcut starting `workbenchw.exe`, and the supervisor it
//!   runs (`service_windows.rs`).
//! * A read-only overview of the Claude Code MCP servers configured on disk
//!   (`claude_mcp.rs`).
//! * Updates: looking for a newer release, installing it over this binary and
//!   restarting into it (`update/`), also as `workbench update`.
//!
//! Routes: `/mcp`, `/api/platform/**`, `/api/settings/**`, `/api/push/**`.

pub mod activity;
pub mod claude_mcp;
pub mod config_edit;
pub mod mcp_server;
pub mod netif;
pub mod notify;
pub mod push;
pub mod remote;
#[cfg_attr(windows, path = "service_windows.rs")]
pub mod service;
pub mod settings;
pub mod tls;
pub mod tools;
pub mod update;

use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::global::{ServerConfig, TlsConfig};
use crate::mcp::McpTool;

/// Server settings as they were when the process started: changing these in
/// `config.toml` only takes effect after a restart.
#[derive(Debug, Clone, Default)]
pub struct BootInfo {
    pub bind: String,
    pub tls: Option<TlsConfig>,
}

#[derive(Default)]
pub struct PlatformState {
    pub activity: activity::ActivityLog,
    pub notifier: notify::Notifier,
    pub push: push::PushState,
    pub update: update::UpdateState,
    /// Serializes writes to `config.toml` and the project config layers.
    pub save_lock: tokio::sync::Mutex<()>,
    /// Hash of the last `config.toml` text edited outside Workbench that could not be
    /// applied (reported once).
    config_reported: parking_lot::Mutex<Option<String>>,
    boot: OnceLock<BootInfo>,
    tools: OnceLock<Arc<Vec<McpTool>>>,
}

impl PlatformState {
    /// Every MCP tool of every slice (built once; tools are static).
    pub fn tools(&self) -> Arc<Vec<McpTool>> {
        self.tools.get_or_init(|| Arc::new(crate::mcp::all_tools())).clone()
    }

    pub fn boot(&self) -> BootInfo {
        self.boot.get().cloned().unwrap_or_default()
    }

    /// Whether this process serves HTTPS itself (`[server.tls]` at startup).
    pub fn tls_active(&self) -> bool {
        self.boot.get().is_some_and(|b| b.tls.is_some())
    }
}

/// Settings that differ from what the running process started with.
pub fn restart_required(boot: &BootInfo, server: &ServerConfig) -> Vec<String> {
    let mut out = vec![];
    if !boot.bind.is_empty() && boot.bind != server.bind {
        out.push("server.bind".to_string());
    }
    if boot.tls != server.tls {
        out.push("server.tls".to_string());
    }
    out
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Truncate to at most `max` characters (not bytes), marking the cut.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/mcp",
            post(mcp_server::post)
                .get(mcp_server::method_not_allowed)
                .delete(mcp_server::method_not_allowed)
                // Tool arguments can carry page bodies or file contents.
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/api/platform/activity", get(activity::list))
        .route("/api/platform/tools", get(tools::list_route))
        .route("/api/platform/mcp-servers", get(claude_mcp::route))
        .route("/api/platform/notify-test", post(notify::test_route))
        .route("/api/platform/remote", get(remote::get_remote).put(remote::put_remote))
        .route("/api/platform/pair", post(remote::pair))
        .route("/api/platform/update", get(update::get_status))
        .route("/api/platform/update/check", post(update::post_check))
        .route("/api/platform/update/install", post(update::post_install))
        .route("/api/platform/restart", post(update::post_restart))
        .route("/api/settings", get(settings::get_settings).patch(settings::patch_settings))
        .route("/api/settings/raw", get(settings::get_raw).put(settings::put_raw))
        .route("/api/settings/validate", post(settings::validate))
        .route("/api/settings/secrets", get(settings::get_secrets))
        .route("/api/settings/secrets/{name}/chmod", post(settings::chmod_secret))
        .route("/api/settings/projects/{pid}", get(settings::get_project))
        .route("/api/settings/projects/{pid}/overlay", put(settings::put_overlay))
        .route("/api/settings/projects/{pid}/repo", put(settings::put_repo))
        .merge(push::routes())
}

pub async fn start(state: &AppState) {
    let server = state.config.read().server.clone();
    let _ = state.platform.boot.set(BootInfo { bind: server.bind, tls: server.tls });
    // Build the tool list now so a slice's broken tool definition shows up at startup.
    let n = state.platform.tools().len();
    tracing::debug!("mcp: {n} tools");
    push::start(state).await;
    notify::spawn_listener(state.clone());
    settings::watch_config(state);
    update::start(state);
}

pub fn mcp_tools() -> Vec<McpTool> {
    tools::platform_tools()
}

#[cfg(test)]
pub(crate) mod testutil {
    //! A real `AppState` on temporary directories for route-level tests.

    use std::net::SocketAddr;

    use crate::app::{self, AppState};
    use crate::config::{GlobalConfig, Paths};

    pub struct TestApp {
        pub state: AppState,
        pub router: axum::Router,
        _dirs: (tempfile::TempDir, tempfile::TempDir),
    }

    pub async fn app_with(cfg: GlobalConfig) -> TestApp {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let paths = Paths { config_dir: config_dir.path().to_path_buf(), data_dir: data_dir.path().to_path_buf() };
        cfg.save(&paths).unwrap();
        let addr: SocketAddr = "127.0.0.1:7999".parse().unwrap();
        let state = AppState::new(paths, cfg, addr).await.unwrap();
        let router = app::build_router(state.clone());
        super::start(&state).await;
        TestApp { state, router, _dirs: (config_dir, data_dir) }
    }

    pub async fn app() -> TestApp {
        let mut cfg = GlobalConfig::default();
        cfg.projects.roots.clear();
        cfg.notify.desktop = false;
        app_with(cfg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_required_flags_bind_and_tls_changes() {
        let boot = BootInfo { bind: "127.0.0.1:7777".into(), tls: None };
        let mut s = ServerConfig::default();
        assert!(restart_required(&boot, &s).is_empty());
        s.bind = "0.0.0.0:7777".into();
        s.tls = Some(TlsConfig { cert: "c".into(), key: "k".into() });
        assert_eq!(restart_required(&boot, &s), vec!["server.bind", "server.tls"]);
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 10), "héllo");
        assert_eq!(truncate_chars("héllo wörld", 5), "héll…");
    }
}

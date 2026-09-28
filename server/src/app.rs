//! Application state and router assembly.
//!
//! `AppState` is a cheap clone (`Arc`). Core services live directly on it; each
//! feature slice owns one field (`terminals`, `files`, `git`, …) whose type it
//! defines in its own module, plus a `router()` and a `start()` hook.

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use parking_lot::RwLock;
use serde_json::json;

use crate::auth::AuthState;
use crate::config::{GlobalConfig, Paths, SecretRef};
use crate::error::ApiError;
use crate::events::EventBus;
use crate::projects::{Project, ProjectRegistry};
use crate::secrets::{Secret, SecretStore};
use crate::{apps, atlassian, auth, db, debug, devcontainer, events, files, git, github, gitlab, lsp, platform, projects, spa, terminals, util, workspace};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

pub struct Inner {
    pub paths: Paths,
    /// The address the server is listening on.
    pub bind_addr: SocketAddr,
    pub started_at: i64,
    pub config: RwLock<GlobalConfig>,
    pub projects: ProjectRegistry,
    pub events: EventBus,
    pub auth: AuthState,
    pub secrets: SecretStore,
    /// Shared HTTP client for GitLab, Atlassian, health checks.
    pub http: reqwest::Client,
    router: OnceLock<Router>,

    // Feature slices — each type is defined (and owned) by its module.
    pub terminals: terminals::Terminals,
    pub files: files::FilesState,
    pub git: git::GitState,
    pub gitlab: gitlab::GitlabState,
    pub github: github::GithubState,
    pub workspace: workspace::WorkspaceState,
    pub atlassian: atlassian::AtlassianState,
    pub apps: apps::AppsState,
    pub devcontainer: devcontainer::DevcontainerState,
    pub platform: platform::PlatformState,
    pub lsp: lsp::LspState,
    pub debug: debug::DebugState,
    pub db: db::DbState,
}

impl AppState {
    pub async fn new(paths: Paths, config: GlobalConfig, bind_addr: SocketAddr) -> anyhow::Result<Self> {
        let auth = AuthState::load(&paths, bind_addr.port())?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("workbench/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        let state = AppState(Arc::new(Inner {
            paths,
            bind_addr,
            started_at: util::now_ms(),
            config: RwLock::new(config),
            projects: ProjectRegistry::default(),
            events: EventBus::new(),
            auth,
            secrets: SecretStore::default(),
            http,
            router: OnceLock::new(),
            terminals: terminals::Terminals::default(),
            files: files::FilesState::default(),
            git: git::GitState::default(),
            gitlab: gitlab::GitlabState::default(),
            github: github::GithubState::default(),
            workspace: workspace::WorkspaceState::default(),
            atlassian: atlassian::AtlassianState::default(),
            apps: apps::AppsState::default(),
            devcontainer: devcontainer::DevcontainerState::default(),
            platform: platform::PlatformState::default(),
            lsp: lsp::LspState::default(),
            debug: debug::DebugState::default(),
            db: db::DbState::default(),
        }));
        state.projects.reload(&state).await;
        Ok(state)
    }

    pub fn port(&self) -> u16 {
        self.bind_addr.port()
    }

    /// Base URL for loopback clients (hooks, MCP, the CLI). Something always listens
    /// there: see `loopback_listen_addr`.
    pub fn local_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    pub fn login_url(&self) -> String {
        format!("{}/auth?token={}", self.local_base_url(), self.auth.master_token())
    }

    /// The assembled router (for in-process calls). Set once in `build_router`.
    pub fn router(&self) -> Option<Router> {
        self.router.get().cloned()
    }

    /// Resolve a secret by name: the project's `[secrets]` (its machine overlay) first,
    /// then the global ones. A name that repository config refers to
    /// (`Project::repo_secret_names`) never reaches the global secrets, so a
    /// repository cannot send config.toml's tokens where it likes.
    pub fn secret(&self, project: Option<&Project>, name: &str) -> Result<Secret, ApiError> {
        if let Some(p) = project {
            if let Some(r) = p.config.secrets.get(name) {
                return self.secrets.resolve_ref(&format!("{}/{name}", p.id), r);
            }
            if p.repo_secret_names.contains(name) {
                return Err(ApiError::not_configured(format!(
                    "secret {name:?} is named by this repository's own config, so Workbench takes it only from the machine overlay: add it under [secrets] in {}",
                    crate::config::contract_tilde(&self.paths.project_overlay(&p.id))
                )));
            }
        }
        let r: Option<SecretRef> = self.config.read().secrets.get(name).cloned();
        match r {
            Some(r) => self.secrets.resolve_ref(name, &r),
            None => Err(ApiError::not_configured(format!(
                "no secret named {name:?}; add it under [secrets] in config.toml"
            ))),
        }
    }
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": "workbench",
        "version": env!("CARGO_PKG_VERSION"),
        "startedAt": state.started_at,
    }))
}

/// Where to listen *in addition* to `bind` so that `local_base_url`
/// (`127.0.0.1:<port>`) reaches this server: `None` when `bind` already covers it
/// (127.0.0.1 itself, 0.0.0.0, or a dual-stack `[::]`).
pub fn loopback_listen_addr(bind: SocketAddr) -> Option<SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr};
    let covered = match bind.ip() {
        IpAddr::V4(v4) => v4.is_unspecified() || v4 == Ipv4Addr::LOCALHOST,
        // [::] also takes IPv4 unless the kernel is set to v6-only sockets.
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                && std::fs::read_to_string("/proc/sys/net/ipv6/bindv6only").map(|s| s.trim() != "1").unwrap_or(true)
        }
    };
    (!covered).then(|| SocketAddr::from((Ipv4Addr::LOCALHOST, bind.port())))
}

/// Request spans carry the method and *path* only: query strings can hold
/// credentials (`/auth?token=…`, the WebSocket device key), and must not reach logs
/// even at `tower_http=debug`.
fn request_span(req: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    // tower_http's target, so `WORKBENCH_LOG=…,tower_http=debug` shows it as before.
    tracing::debug_span!(target: "tower_http::trace::make_span", "request", method = %req.method(), path = %req.uri().path())
}

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/events/ws", get(events::ws_handler))
        .merge(auth::routes())
        .merge(projects::routes())
        .merge(terminals::router())
        .merge(files::router())
        .merge(git::router())
        .merge(gitlab::router())
        .merge(github::router())
        .merge(workspace::router())
        .merge(atlassian::router())
        .merge(apps::router())
        .merge(devcontainer::router())
        .merge(lsp::router())
        .merge(debug::router())
        .merge(db::router())
        .merge(platform::router());

    let router = Router::new()
        .merge(api)
        .route("/auth", get(auth::token_login))
        .route("/pair", get(auth::pair_redeem))
        .fallback(spa::handler)
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::guard))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http().make_span_with(request_span))
        .with_state(state.clone());
    let _ = state.router.set(router.clone());
    router
}

/// Start background work (watchers, pollers, session restore).
pub async fn start_background(state: &AppState) {
    terminals::start(state).await;
    files::start(state).await;
    git::start(state).await;
    gitlab::start(state).await;
    github::start(state).await;
    workspace::start(state).await;
    atlassian::start(state).await;
    apps::start(state).await;
    devcontainer::start(state).await;
    lsp::start(state).await;
    debug::start(state).await;
    db::start(state).await;
    platform::start(state).await;
}

/// Graceful shutdown: persist terminal screens, stop children.
pub async fn shutdown(state: &AppState) {
    apps::shutdown(state).await;
    debug::shutdown(state).await;
    lsp::shutdown(state).await;
    devcontainer::shutdown(state).await;
    db::shutdown(state).await;
    terminals::shutdown(state).await;
    let _ = std::fs::remove_file(state.paths.data_dir.join("runtime.json"));
}

/// Records where the running server is, for `workbench open` / `workbench url`
/// and for helpers (`askpass`, `statusline`). 0600: it names the port only; the
/// token stays in `data_dir/token`.
pub fn write_runtime_file(state: &AppState) -> anyhow::Result<()> {
    util::fs::write_json(
        &state.paths.data_dir.join("runtime.json"),
        &json!({ "pid": std::process::id(), "url": state.local_base_url(), "port": state.port() }),
    )
}

#[cfg(test)]
mod tests {
    use super::loopback_listen_addr;

    #[test]
    fn helpers_always_have_a_loopback_listener() {
        let extra = |s: &str| loopback_listen_addr(s.parse().unwrap()).map(|a| a.to_string());
        assert_eq!(extra("127.0.0.1:7777"), None);
        assert_eq!(extra("0.0.0.0:7777"), None);
        // Just the Tailscale / LAN address, or another loopback address.
        assert_eq!(extra("100.101.102.103:7777").as_deref(), Some("127.0.0.1:7777"));
        assert_eq!(extra("192.168.1.5:7777").as_deref(), Some("127.0.0.1:7777"));
        assert_eq!(extra("127.0.0.2:7868").as_deref(), Some("127.0.0.1:7868"));
        assert_eq!(extra("[::1]:7777").as_deref(), Some("127.0.0.1:7777"));
    }
}

/// The running server's local URL and the master token.
fn running_server() -> anyhow::Result<(String, String)> {
    let paths = Paths::from_env()?;
    let rt: serde_json::Value = util::fs::read_json(&paths.data_dir.join("runtime.json"))?
        .ok_or_else(|| anyhow::anyhow!("Workbench is not running (no runtime.json); start it with `workbench`"))?;
    let url = rt["url"].as_str().unwrap_or("http://127.0.0.1:7777").trim_end_matches('/').to_string();
    let token = std::fs::read_to_string(paths.data_dir.join("token"))?.trim().to_string();
    Ok((url, token))
}

/// `workbench url`: a login URL carrying the master token, printed for the user.
pub fn login_url_of_running_server() -> anyhow::Result<String> {
    let (url, token) = running_server()?;
    Ok(format!("{url}/auth?token={token}"))
}

/// `workbench open` (and the desktop launcher): a one-time `/pair?code=…` URL minted
/// with the master token in a header, so the token never reaches the browser's argv
/// (`/proc/<pid>/cmdline`). The browser keeps its session when it has one.
pub fn launch_url_of_running_server() -> anyhow::Result<String> {
    let (url, token) = running_server()?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let code = rt.block_on(async {
        let resp = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?
            .post(format!("{url}/api/auth/pair"))
            .bearer_auth(&token)
            .json(&json!({ "launch": true }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("cannot reach Workbench at {url}: {}", e.without_url()))?;
        anyhow::ensure!(resp.status().is_success(), "Workbench at {url} answered {} to a sign-in request", resp.status());
        let v: serde_json::Value = resp.json().await?;
        v["code"].as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("Workbench at {url} gave no sign-in code"))
    })?;
    Ok(format!("{url}/pair?code={code}"))
}

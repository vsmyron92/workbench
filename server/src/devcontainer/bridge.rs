//! The bridge listener: how agents in a dev container reach Workbench's hooks and MCP.
//!
//! `127.0.0.1` inside a container is the container itself, so while a container
//! Workbench uses runs, Workbench also listens on its network's gateway address (the
//! host as the container sees it, e.g. `172.17.0.1`), on the server's port when that
//! is free there, else on a free port. That listener serves **only** `/api/hooks/**`
//! and `/mcp`, and only to requests carrying an agent token (`WORKBENCH_AGENT_TOKEN`);
//! everything else is 404, and the master token and device cookies are refused. Any
//! container on that network (and the host) can connect to it, which is why nothing
//! but agent-token routes answers there.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::AppState;

#[derive(Default)]
pub struct Bridge {
    listeners: Mutex<HashMap<IpAddr, (u16, CancellationToken)>>,
}

/// Paths the bridge serves.
pub fn allowed_path(path: &str) -> bool {
    path == "/mcp" || (path.starts_with("/api/hooks/") && !path.contains(".."))
}

async fn filter(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if !allowed_path(&path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Agent tokens only: never the master token, never a browser session.
    if state.auth.agent_from_headers(req.headers()).is_none() {
        return (StatusCode::UNAUTHORIZED, "agent token required").into_response();
    }
    let h = req.headers_mut();
    h.remove(header::COOKIE);
    h.remove(header::ORIGIN);
    // Host pinning accepts loopback names; the request came in on the bridge address.
    if let Ok(v) = HeaderValue::from_str(&format!("127.0.0.1:{}", state.port())) {
        h.insert(header::HOST, v);
    }
    next.run(req).await
}

impl Bridge {
    /// The URL agents in a container on `gateway` use, starting the listener if needed.
    pub async fn ensure(&self, state: &AppState, gateway: IpAddr) -> Option<String> {
        if let Some((port, _)) = self.listeners.lock().get(&gateway) {
            return Some(format!("http://{}", SocketAddr::new(gateway, *port)));
        }
        let router = state.router()?;
        let listener = match tokio::net::TcpListener::bind(SocketAddr::new(gateway, state.port())).await {
            Ok(l) => l,
            Err(_) => match tokio::net::TcpListener::bind(SocketAddr::new(gateway, 0)).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("dev containers: cannot listen on {gateway}: {e}");
                    return None;
                }
            },
        };
        let port = listener.local_addr().ok()?.port();
        let stop = CancellationToken::new();
        {
            let mut map = self.listeners.lock();
            if let Some((p, _)) = map.get(&gateway) {
                // Another caller won the race; drop ours.
                return Some(format!("http://{}", SocketAddr::new(gateway, *p)));
            }
            map.insert(gateway, (port, stop.clone()));
        }
        let app = router.layer(axum::middleware::from_fn_with_state(state.clone(), filter));
        tracing::info!("dev containers: hooks and MCP for containers on http://{gateway}:{port}");
        tokio::spawn(async move {
            let served = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(stop.cancelled_owned())
                .await;
            if let Err(e) = served {
                tracing::warn!("dev containers: bridge listener failed: {e}");
            }
        });
        Some(format!("http://{}", SocketAddr::new(gateway, port)))
    }

    /// The URL of a running listener for `gateway`.
    pub fn url(&self, gateway: IpAddr) -> Option<String> {
        self.listeners.lock().get(&gateway).map(|(p, _)| format!("http://{}", SocketAddr::new(gateway, *p)))
    }

    /// Stop listeners whose gateway no used container needs any more.
    pub fn retain(&self, needed: &[IpAddr]) {
        let mut map = self.listeners.lock();
        map.retain(|ip, (port, stop)| {
            let keep = needed.contains(ip);
            if !keep {
                tracing::info!("dev containers: closing the bridge listener on {ip}:{port}");
                stop.cancel();
            }
            keep
        });
    }

    pub fn active(&self) -> Vec<String> {
        self.listeners.lock().iter().map(|(ip, (p, _))| format!("{ip}:{p}")).collect()
    }

    pub fn shutdown(&self) {
        self.retain(&[]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_agent_routes() {
        assert!(super::allowed_path("/mcp"));
        assert!(super::allowed_path("/api/hooks/claude/abc"));
        assert!(super::allowed_path("/api/hooks/claude/abc/status"));
        for p in ["/", "/api/terminals", "/api/projects", "/auth", "/api/events/ws", "/mcp/x", "/api/hooks/../terminals", "/api/settings"] {
            assert!(!super::allowed_path(p), "{p}");
        }
    }
}

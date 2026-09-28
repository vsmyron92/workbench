//! Remote access: where Workbench listens and how other devices reach it,
//! accepted Host names, pairing codes (with a QR code) and paired devices.
//!
//! Pairing codes and device sessions are the core's (`/api/auth/pair`,
//! `/api/auth/devices`); this module adds the reachable URLs, the QR code and
//! the settings around them.

use std::net::IpAddr;

use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Method, header};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::AppState;
use crate::auth::Caller;
use crate::config::expand_tilde;
use crate::config::global::TlsConfig;
use crate::error::{ApiError, ApiResult};
use crate::mcp::{self, McpCtx};
use crate::util;

use super::{config_edit, netif, restart_required, settings};

// ---------------------------------------------------------------- Host checks (the guard's own rule)

use crate::auth::{host_accepted, host_name, is_loopback_name};

/// Whether a socket bound to `bind` accepts connections on `addr`.
fn listens_on(bind: IpAddr, addr: &IpAddr) -> bool {
    match bind {
        IpAddr::V4(v4) if v4.is_unspecified() => addr.is_ipv4(),
        // Linux dual-stack: [::] also accepts IPv4 unless bindv6only is set.
        IpAddr::V6(v6) if v6.is_unspecified() => true,
        b => b == *addr,
    }
}

// ---------------------------------------------------------------- info

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub interface: String,
    pub address: String,
    pub family: u8,
    /// lan | tailscale | virtual
    pub kind: &'static str,
    pub url: String,
    /// Workbench listens on this address.
    pub listening: bool,
    /// The Host pinning accepts this address (else add it to allowed hosts).
    pub host_allowed: bool,
}

fn scheme(state: &AppState) -> &'static str {
    if state.platform.tls_active() { "https" } else { "http" }
}

pub fn addresses(state: &AppState) -> Vec<Address> {
    let server = state.config.read().server.clone();
    let bind = state.bind_addr.ip();
    let port = state.port();
    let scheme = scheme(state);
    let mut out: Vec<Address> = netif::list()
        .into_iter()
        .filter_map(|i| {
            let kind = netif::classify(&i.name, &i.addr)?;
            let host = netif::url_host(&i.addr, port);
            Some(Address {
                listening: listens_on(bind, &i.addr),
                host_allowed: host_accepted(&server, bind, &host),
                url: format!("{scheme}://{host}"),
                interface: i.name,
                address: i.addr.to_string(),
                family: if i.addr.is_ipv4() { 4 } else { 6 },
                kind,
            })
        })
        .collect();
    // Tailscale first, then LAN, then virtual; IPv4 before IPv6.
    let rank = |a: &Address| (match a.kind { "tailscale" => 0, "lan" => 1, _ => 2 }, a.family);
    out.sort_by_key(rank);
    out.dedup_by(|a, b| a.url == b.url);
    out
}

async fn devices(state: &AppState, caller: Option<&Caller>) -> Vec<Value> {
    let current = match caller {
        Some(Caller::Device { session_id, .. }) => Some(session_id.clone()),
        _ => None,
    };
    let list = mcp::call_api(state, Method::GET, "/api/auth/devices", None, &McpCtx::default()).await.unwrap_or(Value::Null);
    list.as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut d| {
            let is_current = current.is_some() && d["id"].as_str() == current.as_deref();
            d["current"] = json!(is_current);
            d
        })
        .collect()
}

fn tls_info(state: &AppState, tls: Option<&TlsConfig>) -> Value {
    let active = state.platform.tls_active();
    match tls {
        None => json!({ "configured": false, "active": active }),
        Some(t) => json!({
            "configured": true,
            "active": active,
            "cert": t.cert,
            "key": t.key,
            "certExists": expand_tilde(&t.cert).is_file(),
            "keyExists": expand_tilde(&t.key).is_file(),
        }),
    }
}

fn request_host(headers: &HeaderMap) -> String {
    headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string()
}

fn request_secure(state: &AppState, headers: &HeaderMap) -> bool {
    state.platform.tls_active() || headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
}

async fn remote_info(state: &AppState, headers: &HeaderMap, caller: Option<&Caller>) -> Value {
    let server = state.config.read().server.clone();
    let loopback_only = state.bind_addr.ip().is_loopback();
    let devices = devices(state, caller).await;
    let remote_devices = devices.iter().filter(|d| d["remote"] == true).count();
    json!({
        "bind": state.bind_addr.to_string(),
        "configuredBind": server.bind,
        "port": state.port(),
        "loopbackOnly": loopback_only,
        // Reachable from other devices: bound beyond loopback or published through a proxy.
        "exposed": !loopback_only || server.public_url.is_some(),
        "addresses": addresses(state),
        "allowedHosts": server.allowed_hosts,
        "publicUrl": server.public_url,
        "tls": tls_info(state, server.tls.as_ref()),
        "devices": devices,
        "remoteDevices": remote_devices,
        "restartRequired": restart_required(&state.platform.boot(), &server),
        "requestHost": request_host(headers),
        "secure": request_secure(state, headers),
    })
}

/// `GET /api/platform/remote`
pub async fn get_remote(State(state): State<AppState>, headers: HeaderMap, caller: Option<Extension<Caller>>) -> Json<Value> {
    Json(remote_info(&state, &headers, caller.as_ref().map(|c| &c.0)).await)
}

/// `PUT /api/platform/remote {allowedHosts?, addAllowedHost?, publicUrl?, bind?, tls?}`.
/// Hosts and the public URL apply immediately; bind and TLS need a restart.
pub async fn put_remote(
    State(state): State<AppState>,
    headers: HeaderMap,
    caller: Option<Extension<Caller>>,
    Json(body): Json<Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    // Start from the file, so edits made to it by hand are kept.
    let base = settings::edit_base(&state)?;
    let mut cfg = base.cfg.clone();
    let s = &mut cfg.server;
    let bad = |m: &str| ApiError::bad_request(m.to_string());
    for (k, v) in &body {
        match k.as_str() {
            "allowedHosts" => {
                let list: Vec<String> = serde_json::from_value(v.clone()).map_err(|_| bad("allowedHosts must be a list of strings"))?;
                let mut clean: Vec<String> = vec![];
                for h in list.iter().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()) {
                    if !clean.contains(&h) {
                        clean.push(h);
                    }
                }
                s.allowed_hosts = clean;
            }
            "addAllowedHost" => {
                let h = v.as_str().map(str::trim).filter(|h| !h.is_empty()).ok_or_else(|| bad("addAllowedHost must be a host name"))?;
                if !s.allowed_hosts.iter().any(|a| a == h) {
                    s.allowed_hosts.push(h.to_string());
                }
            }
            "publicUrl" => {
                s.public_url = match v {
                    Value::Null => None,
                    Value::String(u) if u.trim().is_empty() => None,
                    Value::String(u) => Some(u.trim().trim_end_matches('/').to_string()),
                    _ => return Err(bad("publicUrl must be a string or null")),
                }
            }
            "bind" => {
                s.bind = v.as_str().map(str::trim).ok_or_else(|| bad("bind must be a string"))?.to_string();
            }
            "tls" => {
                s.tls = match v {
                    Value::Null => None,
                    other => Some(serde_json::from_value(other.clone()).map_err(|_| bad("tls must be {cert, key} or null"))?),
                }
            }
            other => return Err(ApiError::bad_request(format!("unknown field {other:?}"))),
        }
    }
    let (errors, warnings) = settings::check_global(&cfg);
    if !errors.is_empty() {
        return Err(ApiError::bad_request(errors.join("; ")));
    }
    let text = config_edit::update_text(&base.text, &cfg).map_err(ApiError::from)?;
    let applied = settings::apply_config(&state, cfg, text, Some(&base.hash), warnings).await?;
    let info = remote_info(&state, &headers, caller.as_ref().map(|c| &c.0)).await;
    Ok(Json(json!({ "applied": applied, "remote": info })))
}

// ---------------------------------------------------------------- pairing

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairCandidate {
    pub url: String,
    /// public | request | tailscale | lan | loopback
    pub kind: &'static str,
    pub label: String,
    pub reachable: bool,
    pub host_allowed: bool,
}

fn pair_candidates(state: &AppState, headers: &HeaderMap) -> Vec<PairCandidate> {
    let server = state.config.read().server.clone();
    let bind = state.bind_addr.ip();
    let mut out: Vec<PairCandidate> = vec![];
    if let Some(u) = &server.public_url {
        out.push(PairCandidate { url: u.trim_end_matches('/').to_string(), kind: "public", label: "Public URL".into(), reachable: true, host_allowed: true });
    }
    let host = request_host(headers);
    let scheme = if request_secure(state, headers) { "https" } else { "http" };
    let loopback_request = is_loopback_name(host_name(&host));
    if !host.is_empty() && !loopback_request {
        out.push(PairCandidate { url: format!("{scheme}://{host}"), kind: "request", label: "This address".into(), reachable: true, host_allowed: true });
    }
    for a in addresses(state).into_iter().filter(|a| a.kind != "virtual" && a.listening) {
        out.push(PairCandidate {
            label: format!("{} ({})", if a.kind == "tailscale" { "Tailscale" } else { "LAN" }, a.interface),
            url: a.url,
            kind: a.kind,
            reachable: true,
            host_allowed: a.host_allowed,
        });
    }
    if loopback_request && !host.is_empty() {
        out.push(PairCandidate {
            url: format!("{scheme}://{host}"),
            kind: "loopback",
            label: "This computer only".into(),
            reachable: false,
            host_allowed: host_accepted(&server, bind, &host),
        });
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.url.clone()));
    out
}

/// Render `data` as an SVG QR code whose dark modules use `currentColor` (the
/// page decides the colours) on a transparent background.
pub fn qr_svg(data: &str) -> ApiResult<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| ApiError::internal(format!("cannot encode QR code: {e}")))?;
    let image = code
        .render::<svg::Color>()
        .min_dimensions(232, 232)
        .dark_color(svg::Color("currentColor"))
        .light_color(svg::Color("transparent"))
        .build();
    // Drop the XML declaration so the markup can be inlined in HTML.
    Ok(match image.find("<svg") {
        Some(i) => image[i..].to_string(),
        None => image,
    })
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PairBody {
    name: Option<String>,
    base_url: Option<String>,
}

/// `POST /api/platform/pair {name?, baseUrl?}` → `{code, url, expiresAt, ttlMs, qrSvg, baseUrl, candidates, warning}`
pub async fn pair(State(state): State<AppState>, headers: HeaderMap, body: Option<Json<PairBody>>) -> ApiResult<Json<Value>> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let name = body.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| super::truncate_chars(n, 60));
    let minted = mcp::call_api(&state, Method::POST, "/api/auth/pair", Some(json!({ "name": name })), &McpCtx::default()).await?;
    let code = minted["code"].as_str().ok_or_else(|| ApiError::internal("pairing code missing"))?.to_string();
    let now = util::now_ms();
    let expires_at = minted["expiresAt"].as_i64().unwrap_or(now + 600_000);

    let candidates = pair_candidates(&state, &headers);
    let chosen = body
        .base_url
        .as_deref()
        .and_then(|b| candidates.iter().find(|c| c.url == b.trim_end_matches('/')))
        .or_else(|| candidates.iter().find(|c| c.reachable))
        .or(candidates.first())
        .cloned()
        .unwrap_or(PairCandidate {
            url: state.local_base_url(),
            kind: "loopback",
            label: "This computer only".into(),
            reachable: false,
            host_allowed: true,
        });
    let url = format!("{}/pair?code={code}", chosen.url);
    let warning = if !candidates.iter().any(|c| c.reachable) {
        Some(format!(
            "Workbench listens on {} only, so other devices cannot reach it. Bind it to a LAN or Tailscale address \
             (restart required), or publish it with `tailscale serve` and set the public URL.",
            state.bind_addr
        ))
    } else if !chosen.host_allowed {
        Some(format!("{} is not an allowed host yet; add it or the device will be refused.", host_name(chosen.url.split("://").nth(1).unwrap_or(""))))
    } else {
        None
    };
    Ok(Json(json!({
        "code": code,
        "url": url,
        "expiresAt": expires_at,
        "ttlMs": (expires_at - now).max(0),
        "qrSvg": qr_svg(&url)?,
        "baseUrl": chosen.url,
        "candidates": candidates,
        "warning": warning,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::global::ServerConfig;
    use crate::platform::testutil;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn host_acceptance_matches_the_guard() {
        let mut s = ServerConfig::default();
        let bind: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(host_accepted(&s, bind, "127.0.0.1:7777"));
        assert!(host_accepted(&s, bind, "localhost:7777"));
        assert!(!host_accepted(&s, bind, "192.168.1.5:7777"));
        s.allowed_hosts = vec!["192.168.1.5".into()];
        assert!(host_accepted(&s, bind, "192.168.1.5:7777"));
        s.public_url = Some("https://box.tailnet.ts.net".into());
        assert!(host_accepted(&s, bind, "box.tailnet.ts.net"));
        let bind: IpAddr = "100.64.1.2".parse().unwrap();
        assert!(host_accepted(&ServerConfig::default(), bind, "100.64.1.2:7777"));
    }

    #[test]
    fn listening_addresses() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(listens_on(ip("0.0.0.0"), &ip("192.168.1.5")));
        assert!(!listens_on(ip("0.0.0.0"), &ip("fd00::1")));
        assert!(listens_on(ip("::"), &ip("192.168.1.5")));
        assert!(listens_on(ip("100.64.1.2"), &ip("100.64.1.2")));
        assert!(!listens_on(ip("127.0.0.1"), &ip("192.168.1.5")));
    }

    #[test]
    fn qr_is_inline_svg_using_current_color() {
        let svg = qr_svg("http://192.168.1.5:7777/pair?code=ABCDEFGHJK").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("fill=\"currentColor\""));
        assert!(!svg.contains("<?xml"));
    }

    #[tokio::test]
    async fn pairing_returns_code_url_and_qr() {
        let app = testutil::app().await;
        let req = Request::builder()
            .method("POST")
            .uri("/api/platform/pair")
            .header("host", "127.0.0.1:7999")
            .header("authorization", format!("Bearer {}", app.state.auth.master_token()))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "name": "Phone" }).to_string()))
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        let code = v["code"].as_str().unwrap();
        assert_eq!(code.len(), 10);
        assert_eq!(v["url"], format!("http://127.0.0.1:7999/pair?code={code}"));
        assert!(v["qrSvg"].as_str().unwrap().starts_with("<svg"));
        assert!(v["ttlMs"].as_i64().unwrap() > 590_000);
        // Loopback-only instance: the UI must explain why a phone cannot connect.
        assert!(v["warning"].as_str().unwrap().contains("listens on 127.0.0.1:7999"), "{v}");
    }

    /// Behind a dual-stack `[::]` listener an IPv4 browser on this computer
    /// arrives as `::ffff:127.0.0.1`; its login is local, not a remote device.
    #[tokio::test]
    async fn ipv4_mapped_loopback_login_is_not_remote() {
        let app = testutil::app().await;
        for (peer, remote) in [("[::ffff:127.0.0.1]:5555", false), ("[::ffff:192.168.1.5]:5555", true), ("[::1]:5555", false)] {
            let mut req = Request::builder()
                .uri(format!("/auth?token={}", app.state.auth.master_token()))
                .header("host", "127.0.0.1:7999")
                .header("user-agent", format!("test {peer}"))
                .body(Body::empty())
                .unwrap();
            req.extensions_mut().insert(axum::extract::ConnectInfo(peer.parse::<std::net::SocketAddr>().unwrap()));
            let resp = app.router.clone().oneshot(req).await.unwrap();
            assert!(resp.status().is_redirection(), "{peer}: {}", resp.status());
            let list = devices(&app.state, None).await;
            let d = list.iter().find(|d| d["userAgent"] == format!("test {peer}")).unwrap_or_else(|| panic!("{list:?}"));
            assert_eq!(d["remote"], remote, "{peer}: {d}");
        }
    }

    #[tokio::test]
    async fn put_remote_applies_hosts_live() {
        let app = testutil::app().await;
        let req = Request::builder()
            .method("PUT")
            .uri("/api/platform/remote")
            .header("host", "127.0.0.1:7999")
            .header("authorization", format!("Bearer {}", app.state.auth.master_token()))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "addAllowedHost": "box.tailnet.ts.net", "publicUrl": "https://box.tailnet.ts.net/" }).to_string()))
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(v["remote"]["allowedHosts"], json!(["box.tailnet.ts.net"]));
        assert_eq!(v["remote"]["publicUrl"], "https://box.tailnet.ts.net");
        assert_eq!(v["remote"]["exposed"], true);
        assert!(v["applied"]["restartRequired"].as_array().unwrap().is_empty());
        // The guard now accepts the new Host.
        let req = Request::builder()
            .uri("/api/health")
            .header("host", "box.tailnet.ts.net")
            .body(Body::empty())
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

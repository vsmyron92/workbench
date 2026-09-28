//! `/api/push/**`: the VAPID key, this device's subscription, the list of devices
//! with push, per-device topics, the test button and presence reports.
//!
//! Every authenticated device is trusted (see the security model), so any device
//! may list or remove subscriptions; only a device session can subscribe (the
//! subscription ends with it), and in-process callers (MCP tools) cannot change
//! anything.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Engine, PushNote, SubInfo, Subscription, Topic, Topics, Urgency, b64_field, ece};
use crate::app::AppState;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/push", get(get_push))
        .route("/api/push/subscriptions", post(subscribe))
        .route("/api/push/subscriptions/{id}", patch(update).delete(remove))
        .route("/api/push/test", post(test))
        .route("/api/push/presence", post(presence))
}

fn engine(state: &AppState) -> ApiResult<&Arc<Engine>> {
    state.platform.push.engine().ok_or_else(|| {
        ApiError::internal(format!(
            "push notifications are unavailable: {}",
            state.platform.push.error.get().map(String::as_str).unwrap_or("not started")
        ))
    })
}

fn session_of(caller: Option<&Caller>) -> Option<(&str, &str)> {
    match caller {
        Some(Caller::Device { session_id, name }) => Some((session_id.as_str(), name.as_str())),
        _ => None,
    }
}

fn refuse_internal(caller: Option<&Caller>) -> ApiResult<()> {
    match caller {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("push settings are changed by the user, not by agents")),
        _ => Ok(()),
    }
}

/// `GET /api/push` — what the Settings page shows.
async fn get_push(State(state): State<AppState>, caller: Option<Extension<Caller>>) -> ApiResult<Json<Value>> {
    let e = engine(&state)?;
    let current = session_of(caller.as_deref()).map(|(s, _)| s);
    let subs = e.list(current);
    Ok(Json(json!({
        "publicKey": e.public_key(),
        "subscriptions": subs,
        "sessionId": current,
    })))
}

#[derive(Deserialize)]
struct Keys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeBody {
    /// `PushSubscription.toJSON()`: `{endpoint, keys: {p256dh, auth}}`.
    endpoint: String,
    keys: Keys,
    topics: Option<Topics>,
    quiet_when_active: Option<bool>,
}

/// The https origin the request came from (the VAPID `sub` fallback).
fn request_origin(headers: &HeaderMap) -> Option<String> {
    let o = headers.get(header::ORIGIN)?.to_str().ok()?;
    if o.len() > 300 {
        return None;
    }
    let u = reqwest::Url::parse(o).ok()?;
    (u.scheme() == "https" && u.username().is_empty() && u.host_str().is_some()).then(|| u.origin().ascii_serialization())
}

/// `POST /api/push/subscriptions` — subscribe this device (replaces its previous
/// subscription and any other session's with the same endpoint).
async fn subscribe(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    headers: HeaderMap,
    Json(body): Json<SubscribeBody>,
) -> ApiResult<Json<SubInfo>> {
    let e = engine(&state)?;
    refuse_internal(caller.as_deref())?;
    let Some((session_id, device)) = session_of(caller.as_deref()) else {
        return Err(ApiError::bad_request("only a signed-in browser can subscribe to push notifications"));
    };
    e.policy(&state).check(&body.endpoint).map_err(ApiError::bad_request)?;
    let p256dh = b64_field(&body.keys.p256dh).ok_or_else(|| ApiError::bad_request("keys.p256dh is not base64url"))?;
    ece::parse_public_key(&p256dh).map_err(|err| ApiError::bad_request(err.to_string()))?;
    let auth = b64_field(&body.keys.auth).ok_or_else(|| ApiError::bad_request("keys.auth is not base64url"))?;
    if auth.len() != 16 {
        return Err(ApiError::bad_request("keys.auth must be 16 bytes"));
    }
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    // Kept as the browser wrote it (checked again, parsed, at every send), so the
    // page can compare its own subscription with `endpointHash`.
    let endpoint = body.endpoint.trim().to_string();
    let info = {
        let mut subs = e.subs.write();
        let previous = subs.iter().find(|s| s.session_id == session_id || s.endpoint == endpoint).cloned();
        subs.retain(|s| s.session_id != session_id && s.endpoint != endpoint);
        if subs.len() >= super::MAX_SUBSCRIPTIONS {
            return Err(ApiError::conflict(format!(
                "{} devices already receive push notifications; remove one first",
                subs.len()
            )));
        }
        let sub = Subscription {
            id: util::random_token(9),
            session_id: session_id.to_string(),
            device: device.to_string(),
            endpoint,
            p256dh: base64_url(&p256dh),
            auth: base64_url(&auth),
            origin: request_origin(&headers),
            user_agent: ua.chars().take(200).collect(),
            created_at: util::now_ms(),
            // A re-subscription (new key, browser rotated it) keeps the device's choices.
            topics: body.topics.or_else(|| previous.as_ref().map(|p| p.topics.clone())).unwrap_or_default(),
            quiet_when_active: body.quiet_when_active.or_else(|| previous.as_ref().map(|p| p.quiet_when_active)).unwrap_or(true),
            last_ok_at: None,
            last_error: None,
            last_error_at: None,
            failures: 0,
        };
        let info = sub.info(Some(session_id));
        subs.push(sub);
        info
    };
    e.persist().await;
    state.events.emit("push.changed", None, json!({}));
    tracing::info!(device, service = info.service, "push: device subscribed");
    Ok(Json(info))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBody {
    topics: Option<Topics>,
    quiet_when_active: Option<bool>,
}

/// `PATCH /api/push/subscriptions/{id}` — topics and "quiet while in use elsewhere".
async fn update(
    State(state): State<AppState>,
    caller: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> ApiResult<Json<SubInfo>> {
    let e = engine(&state)?;
    refuse_internal(caller.as_deref())?;
    let current = session_of(caller.as_deref()).map(|(s, _)| s);
    let info = {
        let mut subs = e.subs.write();
        let s = subs.iter_mut().find(|s| s.id == id).ok_or_else(|| ApiError::not_found("no such push subscription"))?;
        if let Some(t) = body.topics {
            s.topics = t;
        }
        if let Some(q) = body.quiet_when_active {
            s.quiet_when_active = q;
        }
        s.info(current)
    };
    e.persist().await;
    state.events.emit("push.changed", None, json!({}));
    Ok(Json(info))
}

/// `DELETE /api/push/subscriptions/{id}`. The browser keeps its own subscription
/// object; the page unsubscribes it too when it removes its own.
async fn remove(State(state): State<AppState>, caller: Option<Extension<Caller>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let e = engine(&state)?;
    refuse_internal(caller.as_deref())?;
    let removed = {
        let mut subs = e.subs.write();
        let before = subs.len();
        subs.retain(|s| s.id != id);
        before != subs.len()
    };
    if !removed {
        return Err(ApiError::not_found("no such push subscription"));
    }
    e.persist().await;
    state.events.emit("push.changed", None, json!({}));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TestBody {
    /// Default: this device's subscription.
    subscription_id: Option<String>,
}

/// `POST /api/push/test` — send a test notification now (presence and topics
/// ignored) and report what the push service answered.
async fn test(State(state): State<AppState>, caller: Option<Extension<Caller>>, body: Option<Json<TestBody>>) -> ApiResult<Json<Value>> {
    let e = engine(&state)?;
    refuse_internal(caller.as_deref())?;
    let body = body.map(|b| b.0).unwrap_or_default();
    let target = match body.subscription_id {
        Some(id) => id,
        None => {
            let session = session_of(caller.as_deref()).map(|(s, _)| s).ok_or_else(|| ApiError::bad_request("name a subscriptionId"))?;
            e.subs
                .read()
                .iter()
                .find(|s| s.session_id == session)
                .map(|s| s.id.clone())
                .ok_or_else(|| ApiError::not_found("this device does not receive push notifications"))?
        }
    };
    let device = e.subs.read().iter().find(|s| s.id == target).map(|s| s.device.clone());
    let Some(device) = device else { return Err(ApiError::not_found("no such push subscription")) };
    {
        let mut last = e.last_test.lock();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return Err(ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limited", "wait a moment between tests"));
        }
        *last = Some(Instant::now());
    }
    let note = PushNote {
        topic: Topic::Test,
        tag: "test".into(),
        title: "Workbench".into(),
        body: format!("Test notification for {device}: push notifications work."),
        level: "info",
        urgency: Urgency::Normal,
        ttl_secs: 120,
        project_id: None,
        open: Some(super::OpenTarget { kind: "settings".into(), id: "settings".into(), params: json!({ "section": "notifications" }) }),
        agent: None,
    };
    let results = e.deliver(&state, note, Some(&target), true).await;
    Ok(Json(json!({ "results": results })))
}

#[derive(Deserialize)]
struct PresenceBody {
    visible: bool,
    #[serde(default)]
    active: bool,
    /// The reporting page's own id (random per page load): a browser's tabs share
    /// one device session, and hiding one of them must not hide the others.
    #[serde(default)]
    tab: Option<String>,
}

/// A page's tab id as the presence map keeps it: short and plain, else "".
fn tab_id(tab: Option<&str>) -> &str {
    tab.filter(|t| t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).unwrap_or("")
}

/// `POST /api/push/presence {visible, active, tab?}` — the page reports whether it
/// is shown (and was used recently): a device looking at Workbench gets no push.
async fn presence(State(state): State<AppState>, caller: Option<Extension<Caller>>, Json(body): Json<PresenceBody>) -> ApiResult<Json<Value>> {
    let e = engine(&state)?;
    if let Some((session, _)) = session_of(caller.as_deref()) {
        e.report_presence(session, tab_id(body.tab.as_deref()), body.visible, body.visible && body.active);
    }
    Ok(Json(json!({ "ok": true })))
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

//! REST and WebSocket routes: `/api/terminals/**`, `/api/agents/**`, `/api/hooks/**`.
//!
//! Terminal socket protocol (`/api/terminals/{id}/ws`):
//! * server → client: a text `{"t":"snapshot","cols","rows"}` followed by a **binary
//!   snapshot** (the client `reset()`s, resizes to cols×rows and writes it), then live
//!   binary output. `{"t":"resync","cols","rows"}` + a new binary snapshot when the client
//!   fell behind or the process was replaced; `{"t":"exit","code","signal"}` when the
//!   process exits and `{"t":"running"}` when a new one starts; `{"t":"size","cols","rows"}`
//!   when the PTY size changes (another view took it: render at that size).
//! * client → server: binary frames are input bytes; text `{"t":"resize","cols","rows"}`
//!   reports the size the view fits. The most recently active view decides the PTY size
//!   (`viewers`): reporting a size and typing make a view active.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post, put};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use super::agent::{self, AgentRequest, AskRequest, RemoteControlRequest};
use super::providers;
use super::{Entry, TerminalInfo, TerminalKind, input, transcript, valid_id};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::util;

const MAX_ATTACHMENT: usize = 25 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/terminals", get(list).post(create))
        .route("/api/terminals/{id}", get(get_one).patch(patch).delete(delete))
        .route("/api/terminals/{id}/ws", get(ws))
        .route("/api/terminals/{id}/input", post(send_input))
        .route("/api/terminals/{id}/keys", post(keys))
        .route("/api/terminals/{id}/restart", post(restart))
        .route("/api/terminals/{id}/kill", post(kill))
        .route("/api/terminals/{id}/seen", post(seen))
        .route("/api/terminals/{id}/text", get(text))
        .route("/api/terminals/{id}/attachment", post(attachment).layer(DefaultBodyLimit::max(MAX_ATTACHMENT + 1)))
        .route("/api/agents", post(create_agent))
        .route("/api/agents/ask", post(ask))
        .route("/api/agents/history", get(history))
        .route("/api/agents/external", get(external))
        .route("/api/agents/defaults", get(defaults))
        .route("/api/agents/usage", get(usage_snapshot))
        .route("/api/agents/usage/{provider}", put(set_limit))
        .route("/api/agents/local-models", post(local_models))
        .route("/api/agents/{id}/switch", post(switch_account))
        .route("/api/agents/remote-control", post(remote_control))
        .route("/api/agents/{id}/permission", post(answer_permission))
        // Tool payloads (file contents) can be large; hooks must never fail on size.
        .route("/api/hooks/claude/{id}", post(hook).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .route("/api/hooks/claude/{id}/status", post(status_line))
}

fn check_id(id: &str) -> ApiResult<()> {
    if valid_id(id) { Ok(()) } else { Err(ApiError::not_found("no such terminal")) }
}

// ---------------------------------------------------------------- terminals

async fn list(State(state): State<AppState>) -> Json<Vec<TerminalInfo>> {
    Json(state.terminals.list())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    kind: String,
    project_id: Option<String>,
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    /// `true`: in the project's dev container; `false`: on the host; absent: in the
    /// container when the project runs its terminals there.
    #[serde(default)]
    container: Option<bool>,
}

async fn create(State(state): State<AppState>, Json(b): Json<CreateBody>) -> ApiResult<Json<TerminalInfo>> {
    if b.kind != "shell" {
        return Err(ApiError::bad_request("only kind 'shell' can be created here; agents start through /api/agents"));
    }
    Ok(Json(state.terminals.create_shell(&state, b.project_id.filter(|p| !p.is_empty()), b.cwd, b.cols, b.rows, b.container).await?))
}

async fn get_one(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<TerminalInfo>> {
    check_id(&id)?;
    Ok(Json(state.terminals.require(&id)?.info()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchBody {
    title: Option<String>,
    /// A colour token name (`accent`, `success`, …), `#rrggbb`, or `null` to clear.
    #[serde(default)]
    color: Option<Value>,
    pinned: Option<bool>,
    order: Option<i64>,
    open: Option<bool>,
}

fn valid_color(c: &str) -> bool {
    let token = !c.is_empty() && c.len() <= 20 && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    let hex = c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit());
    token || hex
}

async fn patch(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<PatchBody>) -> ApiResult<Json<TerminalInfo>> {
    check_id(&id)?;
    let entry = state.terminals.require(&id)?;
    let title = match b.title.as_deref() {
        Some(t) => {
            let t = transcript::clean_line(t, 120);
            if t.is_empty() {
                return Err(ApiError::bad_request("title is empty"));
            }
            Some(t)
        }
        None => None,
    };
    let color = match &b.color {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(c)) if valid_color(c) => Some(Some(c.to_lowercase())),
        Some(_) => return Err(ApiError::bad_request("color must be a token name or #rrggbb")),
    };
    state.terminals.update(&entry, |r| {
        let before = serde_json::to_value(&r.info).ok();
        if let Some(t) = &title {
            r.info.title = t.clone();
            r.title_locked = true;
            if let Some(a) = r.info.agent.as_mut() {
                a.title = Some(t.clone());
            }
        }
        if let Some(c) = &color {
            r.info.color = c.clone();
        }
        if let Some(p) = b.pinned {
            r.info.pinned = p;
        }
        if let Some(o) = b.order {
            r.info.order = o;
        }
        if let Some(o) = b.open {
            r.info.open = o;
        }
        serde_json::to_value(&r.info).ok() != before
    });
    Ok(Json(entry.info()))
}

#[derive(Deserialize)]
struct DeleteQuery {
    forget: Option<bool>,
}

async fn delete(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<DeleteQuery>) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    state.terminals.close(&state, &id, q.forget.unwrap_or(false)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct InputBody {
    text: String,
    submit: Option<bool>,
    paste: Option<bool>,
    /// Type into an agent session even while it shows a dialog.
    #[serde(default)]
    force: bool,
}

async fn send_input(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<InputBody>) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    let entry = state.terminals.require(&id)?;
    if !b.force {
        // Its state, or a dialog of a Codex, Kimi or custom CLI on screen right now.
        if let Some(why) = state.terminals.prompt_refusal(&entry) {
            return Err(ApiError::new(axum::http::StatusCode::CONFLICT, "agent_busy", why));
        }
    }
    state.terminals.send_input(&id, &b.text, b.submit.unwrap_or(true), b.paste.unwrap_or(true)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct KeysBody {
    keys: Vec<String>,
}

async fn keys(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<KeysBody>) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    if b.keys.is_empty() || b.keys.len() > 32 {
        return Err(ApiError::bad_request("send 1–32 keys"));
    }
    let mut bytes = vec![];
    for k in &b.keys {
        bytes.extend_from_slice(input::key_bytes(k).ok_or_else(|| ApiError::bad_request(format!("unknown key {k:?}")))?);
    }
    state.terminals.write(&id, &bytes)?;
    Ok(Json(json!({ "ok": true })))
}

async fn restart(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<TerminalInfo>> {
    check_id(&id)?;
    Ok(Json(state.terminals.restart(&state, &id).await?))
}

async fn kill(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<TerminalInfo>> {
    check_id(&id)?;
    state.terminals.kill(&id).await?;
    Ok(Json(state.terminals.require(&id)?.info()))
}

async fn seen(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<TerminalInfo>> {
    check_id(&id)?;
    let entry = state.terminals.require(&id)?;
    state.terminals.update(&entry, |r| match r.info.agent.as_mut() {
        Some(a) if a.unread => {
            a.unread = false;
            true
        }
        _ => false,
    });
    Ok(Json(entry.info()))
}

#[derive(Deserialize)]
struct TextQuery {
    lines: Option<usize>,
}

async fn text(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<TextQuery>) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    let lines = q.lines.unwrap_or(200).clamp(1, 5000);
    // Walking the history holds the mirror lock: keep it off the async workers.
    let st = state.clone();
    let text = tokio::task::spawn_blocking(move || st.terminals.screen_text(&id, lines))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("no such terminal"))?;
    Ok(Json(json!({ "text": text })))
}

/// Save a pasted image and return its absolute path (the UI inserts it, quoted).
async fn attachment(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    state.terminals.require(&id)?;
    if body.len() > MAX_ATTACHMENT {
        return Err(ApiError::bad_request("attachments are limited to 25 MB"));
    }
    let (ext, mime) = input::sniff_image(&body).ok_or_else(|| ApiError::bad_request("only PNG, JPEG, GIF and WebP images can be attached"))?;
    let dir = state.terminals.attachments_dir()?.join(chrono::Local::now().format("%Y-%m-%d").to_string());
    let name = format!("{}-{}.{ext}", chrono::Local::now().format("%H%M%S"), util::random_token(4).replace(['-', '_'], "x"));
    let path = dir.join(name);
    let p = path.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new("/")))?;
        if let Some(parent) = p.parent().and_then(|d| d.parent()) {
            util::fs::set_mode(parent, 0o700);
        }
        util::fs::write_atomic(&p, &body, 0o600)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    let s = path.display().to_string();
    Ok(Json(json!({ "path": s, "quoted": input::quote_path(&s), "mime": mime })))
}

// ---------------------------------------------------------------- websocket

async fn ws(
    State(state): State<AppState>,
    Path(id): Path<String>,
    caller: Option<axum::extract::Extension<crate::auth::Caller>>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_id(&id)?;
    let entry = state.terminals.require(&id)?;
    // PTY input is code execution: the socket closes when its device is signed out.
    let session = state.auth.watch(caller.as_ref().map(|c| &c.0));
    Ok(upgrade.max_message_size(8 * 1024 * 1024).on_upgrade(move |socket| run_socket(state, entry, socket, session)))
}

/// One attached client: counted while attached (xterm.js then answers device queries)
/// and registered as a view for sizing. Leaving gives the size back and lets an exited
/// terminal's screen hibernate.
struct Attached {
    state: AppState,
    entry: Arc<Entry>,
    viewer: u64,
}

impl Attached {
    fn new(state: AppState, entry: Arc<Entry>) -> Self {
        entry.screen.attached.fetch_add(1, Ordering::AcqRel);
        let viewer = entry.screen.viewers.lock().join();
        Attached { state, entry, viewer }
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.entry.screen.attached.fetch_sub(1, Ordering::AcqRel);
        self.state.terminals.viewer_left(&self.entry, self.viewer);
        self.state.terminals.maybe_hibernate(&self.entry);
    }
}

#[derive(Deserialize)]
struct Control {
    t: String,
    cols: Option<u16>,
    rows: Option<u16>,
}

const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// Live output is coalesced into frames of up to this size.
const FRAME_MAX: usize = 256 * 1024;

async fn run_socket(state: AppState, entry: Arc<Entry>, socket: WebSocket, mut session: crate::auth::SessionWatch) {
    let (mut sink, mut stream) = socket.split();
    let attached = Attached::new(state.clone(), entry.clone());
    let viewer = attached.viewer;
    let mut gen_rx = entry.screen.generation.subscribe();
    let mut exit_rx = entry.exit_tx.subscribe();
    let mut size_rx = entry.screen.size_tx.subscribe();

    macro_rules! send {
        ($msg:expr) => {
            match tokio::time::timeout(SEND_TIMEOUT, sink.send($msg)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        };
    }
    macro_rules! snapshot {
        ($kind:expr) => {{
            // Serializing thousands of history rows is CPU work: keep it off the async workers.
            let screen = entry.screen.clone();
            let Ok((snap, rx, (cols, rows))) = tokio::task::spawn_blocking(move || screen.attach()).await else { return };
            gen_rx.borrow_and_update();
            // The snapshot carries the size.
            size_rx.borrow_and_update();
            send!(Message::Text(json!({ "t": $kind, "cols": cols, "rows": rows }).to_string().into()));
            send!(Message::Binary(snap.into()));
            rx
        }};
    }

    let mut rx = snapshot!("snapshot");
    let exited = exit_rx.borrow_and_update().clone();
    if let Some(e) = exited {
        send!(Message::Text(json!({ "t": "exit", "code": e.code, "signal": e.signal }).to_string().into()));
    }
    loop {
        tokio::select! {
            _ = session.ended(&state.auth) => {
                let _ = tokio::time::timeout(SEND_TIMEOUT, sink.send(crate::auth::session_ended_close())).await;
                return;
            }
            r = rx.recv() => match r {
                Ok(chunk) => {
                    let mut frame = chunk.to_vec();
                    let mut lagged = false;
                    while frame.len() < FRAME_MAX {
                        match rx.try_recv() {
                            Ok(more) => frame.extend_from_slice(&more),
                            Err(TryRecvError::Lagged(_)) => { lagged = true; break }
                            Err(_) => break,
                        }
                    }
                    if lagged {
                        rx = snapshot!("resync");
                    } else {
                        send!(Message::Binary(frame.into()));
                    }
                }
                Err(RecvError::Lagged(_)) => { rx = snapshot!("resync"); }
                Err(RecvError::Closed) => return,
            },
            r = gen_rx.changed() => {
                if r.is_err() { return }
                rx = snapshot!("resync");
            }
            r = exit_rx.changed() => {
                if r.is_err() { return }
                let v = exit_rx.borrow_and_update().clone();
                let msg = match v {
                    Some(e) => json!({ "t": "exit", "code": e.code, "signal": e.signal }),
                    None => json!({ "t": "running" }),
                };
                send!(Message::Text(msg.to_string().into()));
            }
            r = size_rx.changed() => {
                if r.is_err() { return }
                let (cols, rows) = *size_rx.borrow_and_update();
                send!(Message::Text(json!({ "t": "size", "cols": cols, "rows": rows }).to_string().into()));
            }
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    if let Some(p) = entry.running_pty() {
                        entry.note_input(&b);
                        // The view the user types in takes the size back.
                        if super::is_user_input(&b) {
                            state.terminals.viewer_active(&entry, viewer);
                            entry.look_for_permission_dialog();
                        }
                        let _ = p.write(b);
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Ok(c) = serde_json::from_str::<Control>(t.as_str()) {
                        match c.t.as_str() {
                            "resize" => {
                                if let (Some(cols), Some(rows)) = (c.cols, c.rows) {
                                    let _ = state.terminals.viewer_resize(&entry, viewer, cols, rows);
                                }
                            }
                            "ping" => send!(Message::Text(json!({ "t": "pong" }).to_string().into())),
                            _ => {}
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            },
        }
    }
}

// ---------------------------------------------------------------- agents

async fn create_agent(State(state): State<AppState>, Json(req): Json<AgentRequest>) -> ApiResult<Json<TerminalInfo>> {
    Ok(Json(state.terminals.spawn_agent(&state, req).await?))
}

async fn ask(State(state): State<AppState>, Json(req): Json<AskRequest>) -> ApiResult<Json<TerminalInfo>> {
    Ok(Json(state.terminals.ask(&state, req).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryQuery {
    project_id: String,
    /// A provider id (default `claude`).
    provider: Option<String>,
    limit: Option<usize>,
}

async fn history(State(state): State<AppState>, Query(q): Query<HistoryQuery>) -> ApiResult<Json<Vec<transcript::HistoryEntry>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let provider = q.provider.as_deref().map(str::trim).filter(|p| !p.is_empty());
    Ok(Json(state.terminals.history(&state, &q.project_id, provider, limit).await?))
}

async fn external(State(state): State<AppState>) -> Json<Vec<transcript::ExternalSession>> {
    Json(state.terminals.external(&state).await)
}

/// Only a signed-in device decides about accounts: an agent cannot clear its own limit or
/// move itself.
fn require_device(caller: &Option<axum::extract::Extension<crate::auth::Caller>>) -> ApiResult<()> {
    if matches!(caller.as_ref().map(|c| &c.0), Some(crate::auth::Caller::Device { .. })) {
        Ok(())
    } else {
        Err(ApiError::forbidden("accounts are managed from a signed-in device"))
    }
}

/// `GET /api/agents/usage` → `{usage: {<provider>: {windows, limited, limitedUntil, reason}}, failover}`.
async fn usage_snapshot(State(state): State<AppState>) -> Json<Value> {
    let failover = match providers::Failover::of(&state.config.read().agents) {
        providers::Failover::Off => "off",
        providers::Failover::New => "new",
        providers::Failover::Session => "session",
    };
    Json(json!({ "usage": state.terminals.usage.snapshot(util::now_ms()), "failover": failover }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitBody {
    /// Out of use until then (ms); `null`: usable again.
    limited_until: Option<i64>,
}

/// `PUT /api/agents/usage/{provider} {limitedUntil}`: say an account is at its limit (until
/// a time), or usable again, when what the CLIs reported is wrong or not enough.
async fn set_limit(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    caller: Option<axum::extract::Extension<crate::auth::Caller>>,
    Json(b): Json<LimitBody>,
) -> ApiResult<Json<Value>> {
    require_device(&caller)?;
    let p = agent::find_provider(&state.config.read().agents, Some(&provider))?;
    let now = util::now_ms();
    match b.limited_until {
        Some(t) if t > now => state.terminals.mark_limited_by_user(&p.id, t, now),
        Some(_) => return Err(ApiError::bad_request("limitedUntil must be in the future")),
        None => state.terminals.clear_limited_by_user(&p.id, now),
    }
    Ok(Json(json!({ "usage": state.terminals.usage.describe(&p.id, now) })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchBody {
    /// The account to continue on (default: the next one that is free).
    provider: Option<String>,
}

/// `POST /api/agents/{id}/switch {provider?}`: continue a session's work on another account,
/// as a new session in the same folder. Returns the new session.
async fn switch_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
    caller: Option<axum::extract::Extension<crate::auth::Caller>>,
    Json(b): Json<SwitchBody>,
) -> ApiResult<Json<TerminalInfo>> {
    require_device(&caller)?;
    check_id(&id)?;
    let to = b.provider.as_deref().map(str::trim).filter(|p| !p.is_empty());
    Ok(Json(state.terminals.switch_account(&state, &id, to).await?))
}

#[derive(Deserialize)]
struct LocalModelsBody {
    server: String,
    #[serde(default)]
    url: String,
}

/// `POST /api/agents/local-models {server, url?}` → `{models}` or `{error}`: what a model
/// server of your own serves.
async fn local_models(
    caller: Option<axum::extract::Extension<crate::auth::Caller>>,
    Json(b): Json<LocalModelsBody>,
) -> ApiResult<Json<Value>> {
    require_device(&caller)?;
    Ok(Json(match super::local::probe(&b.server, &b.url).await {
        Ok(models) => json!({ "models": models }),
        Err(error) => json!({ "models": [], "error": error }),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultsQuery {
    project_id: Option<String>,
}

/// What the "new session" form needs: effective defaults, the project's starters and
/// every configured provider (`providers[]`, with availability and what each supports).
async fn defaults(State(state): State<AppState>, Query(q): Query<DefaultsQuery>) -> ApiResult<Json<Value>> {
    let cfg = state.config.read().agents.clone();
    let project = match q.project_id.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => Some(state.projects.require(p)?),
        None => None,
    };
    let pa = project.as_ref().map(|p| p.config.agent.clone()).unwrap_or_default();
    let (list, warnings) = providers::list(&cfg);
    // PATH lookups touch the disk.
    let found = {
        let commands: Vec<String> = list.iter().map(|p| p.command.clone()).collect();
        tokio::task::spawn_blocking(move || commands.iter().map(|c| agent::resolve_command(c)).collect::<Vec<_>>())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
    };
    let described: Vec<Value> = list
        .iter()
        .zip(found.iter())
        .map(|(p, path)| {
            let mut v = providers::describe(p, path.as_ref());
            v["usage"] = state.terminals.usage.describe(&p.id, util::now_ms());
            if p.kind == providers::ProviderKind::Claude {
                // Project [agent] settings are Claude Code settings.
                let d = &mut v["defaults"];
                if let Some(m) = &pa.model {
                    d["model"] = json!(m);
                }
                if let Some(e) = &pa.effort {
                    d["effort"] = json!(e);
                }
                if let Some(m) = &pa.permission_mode {
                    d["permissionMode"] = json!(m);
                }
            }
            v
        })
        .collect();
    let command = agent::resolve_command(&cfg.command);
    let default_provider = providers::default_id(&cfg);
    let permission_wait = agent::permission_wait(&cfg);
    Ok(Json(json!({
        "providers": described,
        "defaultProvider": default_provider,
        "providerWarnings": warnings,
        "failover": match providers::Failover::of(&cfg) {
            providers::Failover::Off => "off",
            providers::Failover::New => "new",
            providers::Failover::Session => "session",
        },
        "command": cfg.command,
        "commandFound": command.is_some(),
        "model": pa.model.or(cfg.model),
        "effort": pa.effort.or(cfg.effort),
        "permissionMode": pa.permission_mode.or(cfg.permission_mode),
        "remoteControl": pa.remote_control || cfg.remote_control,
        "restoreOnStart": cfg.restore_on_start,
        "statusline": cfg.statusline,
        // Claude Code permission requests answerable from Workbench, and for how long (s).
        "answerPermissions": cfg.answer_permissions,
        "permissionWait": permission_wait,
        "addDirs": pa.add_dirs,
        "starters": pa.starters.iter().map(|s| json!({ "name": s.name, "prompt": s.command })).collect::<Vec<_>>(),
        "efforts": agent::EFFORTS,
        "permissionModes": agent::PERMISSION_MODES,
    })))
}

async fn remote_control(State(state): State<AppState>, Json(req): Json<RemoteControlRequest>) -> ApiResult<Json<TerminalInfo>> {
    Ok(Json(state.terminals.spawn_remote_control(&state, req).await?))
}

// ---------------------------------------------------------------- hooks

/// The agent token must belong to this terminal (or the master token is used).
fn authorize_hook(state: &AppState, headers: &HeaderMap, id: &str) -> ApiResult<()> {
    if state.auth.agent_from_headers(headers).as_deref() == Some(id) || state.auth.is_master_bearer(headers) {
        Ok(())
    } else {
        Err(ApiError::unauthorized("invalid agent token"))
    }
}

/// Claude Code HTTP hook. Always answers `{}`: Workbench observes, never decides.
async fn hook(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    authorize_hook(&state, &headers, &id)?;
    if let Ok(v) = serde_json::from_slice::<Value>(&body) {
        tracing::debug!(
            terminal = %id,
            event = v.get("hook_event_name").and_then(serde_json::Value::as_str).unwrap_or("?"),
            kind = v.get("notification_type").or_else(|| v.get("source")).and_then(serde_json::Value::as_str).unwrap_or(""),
            "claude hook"
        );
        let entry = state.terminals.require(&id)?;
        // Only Claude Code sessions are configured with these hooks.
        if entry.kind() == TerminalKind::Agent && entry.provider() == Some(super::ProviderKind::Claude) {
            if v.get("hook_event_name").and_then(Value::as_str) == Some("PermissionRequest") {
                // Held until a device answers or it stops being pending (`permission`).
                return Ok(Json(state.terminals.permission_request(&state, &id, &v).await?));
            }
            state.terminals.apply_hook(&id, &v)?;
            if v.get("hook_event_name").and_then(Value::as_str) == Some("StopFailure") {
                state.terminals.claude_turn_failed(&state, &id, &v);
            }
            // Local History attributes the files this session's tools wrote.
            crate::files::history::agent_hook(&state, &id, &v);
        }
    }
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionBody {
    /// `AgentInfo.pendingPermission.id`.
    id: String,
    /// `allow` or `deny`.
    decision: String,
    /// Deny: what Claude is told (it then goes on); without one the turn stops.
    message: Option<String>,
    /// Deny: stop the turn (default: when there is no message).
    interrupt: Option<bool>,
    /// Allow: `once` (default) or `session` (Claude's suggested rules, in memory only).
    scope: Option<String>,
}

/// `POST /api/agents/{terminalId}/permission`: answer a pending permission request.
/// Devices only: a session never approves itself, so agent tokens (refused on `/api/**`
/// anyway), in-process MCP calls and the master token (a file an agent could read) are
/// refused. `409 not_pending` when it is no longer pending.
async fn answer_permission(
    State(state): State<AppState>,
    Path(id): Path<String>,
    caller: Option<axum::extract::Extension<crate::auth::Caller>>,
    Json(b): Json<PermissionBody>,
) -> ApiResult<Json<TerminalInfo>> {
    if !matches!(caller.as_ref().map(|c| &c.0), Some(crate::auth::Caller::Device { .. })) {
        return Err(ApiError::forbidden("permission requests are answered from a signed-in device"));
    }
    check_id(&id)?;
    if b.id.is_empty() || b.id.len() > 64 {
        return Err(ApiError::bad_request("invalid permission request id"));
    }
    let decision = super::permission::Decision::parse(&b.decision, b.message.as_deref(), b.interrupt, b.scope.as_deref())
        .map_err(ApiError::bad_request)?;
    let entry = state.terminals.require(&id)?;
    if entry.kind() != TerminalKind::Agent {
        return Err(ApiError::bad_request("that terminal is not an agent session"));
    }
    Ok(Json(state.terminals.answer_permission(&id, &b.id, &decision)?))
}

/// Status line JSON forwarded by `workbench statusline`.
async fn status_line(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Value>> {
    check_id(&id)?;
    authorize_hook(&state, &headers, &id)?;
    if let Ok(v) = serde_json::from_slice::<Value>(&body) {
        state.terminals.apply_status(&id, &v)?;
        state.terminals.note_status_usage(&id, &v);
    }
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    #[test]
    fn colors_are_tokens_or_hex() {
        assert!(super::valid_color("accent"));
        assert!(super::valid_color("#a1B2c3"));
        assert!(!super::valid_color("red; background:url(x)"));
        assert!(!super::valid_color("#12345"));
    }
}

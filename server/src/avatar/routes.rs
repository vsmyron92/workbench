//! The avatar's routes (OWNER: avatar slice).
//!
//! **Device routes**, under `/api/avatar`: what a signed-in browser reads and does.
//!
//! * `GET /api/avatar` → `{enabled, sessions, avatars, asked}`; `GET /api/avatar/{terminalId}`
//!   → one `AvatarInfo` (404 when the session has none). These two only read, and also answer
//!   the master token and in-process calls.
//! * `POST …/{terminalId}/lines/{lineId}/claim {tab, release?}` → `{speak, leaseMs, retryMs}`;
//!   `POST …/lines/{lineId}/heard {outcome, voice?}` → `{ok: true}`.
//! * `POST …/{terminalId}/reply {askId?, text?, choice?, via}` → `{delivered}`;
//!   `POST …/asks/{askId}/dismiss {}` → `{ok: true}`.
//! * `POST …/{terminalId}/on {}`, `…/off {}`, `…/reset {}` → the `AvatarInfo`. `on` also tells
//!   a running session that was not told of its avatar (it pastes the brief, `device::tell`);
//!   when the session cannot take it now the answer carries `briefed: false`, and the session
//!   is told when it is next idle, or at the next `on`.
//!
//! Every POST here is a device's alone (`Caller::Device`: the guard then also asks for the
//! device key and the same origin). The master token and in-process callers get 403, an agent
//! token 401 from the guard: a session never answers itself, turns its own avatar on, or claims
//! to have been heard.
//!
//! **Hook routes**, under `/api/hooks/avatar/{terminalId}`: the session's own side, for CLIs
//! without Workbench's MCP tools (`workbench avatar`, or `curl` in a dev container).
//!
//! * `GET …/guide` → the guide for a session that drives its avatar with a command, then its
//!   status, as `text/markdown`; `GET …/status?look=1` → the JSON of `avatar_status`.
//! * `POST …/design`: the arguments of `avatar_design` as JSON, or the artwork alone
//!   (`Content-Type: image/svg+xml`, or a body that starts with `<`) → the design answer as
//!   `text/plain`.
//! * `POST …/picture`: nothing or `{}` (the avatar at rest), `{pose}` or `{frames, count?}` as
//!   `avatar_status` takes them → the picture as `image/png`. It counts as a status call.
//! * `POST …/say {text, gesture?, lang?, wait?}`, or the line itself as `text/plain` with
//!   `gesture`, `lang` and `wait` as query parameters → `{lineId, outcome?, voice?, note}`.
//! * `POST …/ask`: the arguments of `avatar_ask` → its answer; the request is held for the wait.
//! * `POST …/act {play?, mode?, times?, stop?}` → `{ok: true, note}`.
//!
//! These are outside the device guard, so each handler begins with the same two steps before it
//! looks at anything else: the bearer token is the agent token of `{terminalId}` (the master
//! token is refused here: 401), then the gate, which counts the attempt. Only then is the body,
//! taken as bytes, parsed: a call without the token or over its rate costs a buffer of at most
//! 256 KiB, never a document tree, and changes nothing. Nothing under these routes puts text into
//! a session.
//!
//! What is said, asked, answered and drawn travels in bodies only: request spans log paths.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::brief;
use super::device::{self, ClaimRequest, HeardRequest, ReplyRequest};
use super::session::{self, ActRequest, AskArgs, Call, Pass, PictureRequest, SayRequest, StatusRequest};
use super::store::{self, AvatarInfo, AvatarList, MAX_WAIT_HOOK_SECS};
use super::{Via, look, tools};
use crate::app::AppState;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};

/// The most a request of this slice carries: the size of a design call.
pub const BODY_LIMIT: usize = look::MAX_DESIGN_BYTES;

pub fn router() -> Router<AppState> {
    let avatar = "/api/avatar/{tid}";
    let hooks = "/api/hooks/avatar/{tid}";
    Router::new()
        .route("/api/avatar", get(list))
        .route(avatar, get(one))
        .route(&format!("{avatar}/lines/{{line}}/claim"), post(claim))
        .route(&format!("{avatar}/lines/{{line}}/heard"), post(heard))
        .route(&format!("{avatar}/reply"), post(reply))
        .route(&format!("{avatar}/asks/{{ask}}/dismiss"), post(dismiss))
        .route(&format!("{avatar}/on"), post(on))
        .route(&format!("{avatar}/off"), post(off))
        .route(&format!("{avatar}/reset"), post(reset))
        .route(&format!("{hooks}/guide"), get(hook_guide))
        .route(&format!("{hooks}/status"), get(hook_status))
        .route(&format!("{hooks}/design"), post(hook_design))
        .route(&format!("{hooks}/picture"), post(hook_picture))
        .route(&format!("{hooks}/say"), post(hook_say))
        .route(&format!("{hooks}/ask"), post(hook_ask))
        .route(&format!("{hooks}/act"), post(hook_act))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

// ---------------------------------------------------------------- reading a request

/// A body without the byte order mark some tools put before UTF-8 text.
fn unmarked(body: &Bytes) -> &[u8] {
    body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body)
}

/// A body as JSON. `form` says what the route takes, for the caller that sent something else.
fn json_of(body: &Bytes, form: &str) -> ApiResult<Value> {
    serde_json::from_slice(unmarked(body)).map_err(|e| ApiError::bad_request(format!("the body is not JSON ({e}): send {form}")))
}

/// A body as text.
fn text_of<'b>(body: &'b Bytes, what: &str) -> ApiResult<&'b str> {
    std::str::from_utf8(unmarked(body)).map_err(|_| ApiError::bad_request(format!("{what} is not UTF-8 text")))
}

/// The body of a route that takes no arguments: empty, or `{}`.
fn nothing(body: &Bytes, route: &str) -> ApiResult<()> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(o)) if o.is_empty() => Ok(()),
        _ => Err(ApiError::bad_request(format!("{route} takes no arguments: send {{}}"))),
    }
}

/// The media type of a request's body, without its parameters, in lower case.
fn media(headers: &HeaderMap) -> String {
    headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).and_then(|v| v.split(';').next()).unwrap_or_default().trim().to_ascii_lowercase()
}

/// The query parameters of a hook call: these keys and no others, each at most once. `mask` is
/// the session's secret masking, for a refusal that quotes a key.
fn query(uri: &Uri, call: &str, known: &[&str], mask: &dyn Fn(&str) -> String) -> ApiResult<Vec<(String, String)>> {
    let accepted = || if known.is_empty() { "it takes none".to_string() } else { format!("it takes {}", known.join(", ")) };
    let Query(pairs) = Query::<Vec<(String, String)>>::try_from_uri(uri).map_err(|_| ApiError::bad_request(format!("the query parameters of {call} cannot be read: {}", accepted())))?;
    for (i, (key, _)) in pairs.iter().enumerate() {
        if !known.contains(&key.as_str()) {
            return Err(ApiError::bad_request(format!("{call} has no query parameter {}: {}", session::shown(key, mask), accepted())));
        }
        if pairs.iter().take(i).any(|(earlier, _)| earlier == key) {
            return Err(ApiError::bad_request(format!("{call} has the query parameter {key} twice")));
        }
    }
    Ok(pairs)
}

fn parameter(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// A query parameter that switches something on: `1` or `true`.
fn switch(pairs: &[(String, String)], key: &str, mask: &dyn Fn(&str) -> String) -> ApiResult<bool> {
    match parameter(pairs, key).as_deref() {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(other) => Err(ApiError::bad_request(format!("{key}={}: write {key}=1", session::shown(other, mask)))),
    }
}

fn plain(text: String) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

// ---------------------------------------------------------------- device routes

/// Rule: only the user's devices answer for the user. The master token is a file a session
/// could read, and an in-process call is a session's tool: both are refused.
fn device_only(caller: &Option<Extension<Caller>>) -> ApiResult<()> {
    if matches!(caller.as_ref().map(|c| &c.0), Some(Caller::Device { .. })) { Ok(()) } else { Err(ApiError::forbidden("this is done from a signed-in device: a session does not answer for the user, and neither does the Workbench token")) }
}

async fn list(State(state): State<AppState>) -> Json<AvatarList> {
    Json(store::list(&state))
}

async fn one(State(state): State<AppState>, Path(tid): Path<String>) -> ApiResult<Json<AvatarInfo>> {
    Ok(Json(store::get(&state, &tid)?))
}

async fn claim(State(state): State<AppState>, Path((tid, line)): Path<(String, String)>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<device::Claim>> {
    device_only(&caller)?;
    let req = ClaimRequest::parse(&json_of(&body, "{tab, release?}")?)?;
    Ok(Json(device::claim(&state, &tid, &line, req, crate::util::now_ms())?))
}

async fn heard(State(state): State<AppState>, Path((tid, line)): Path<(String, String)>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<Value>> {
    device_only(&caller)?;
    let req = HeardRequest::parse(&json_of(&body, "{outcome, voice?}")?)?;
    device::heard(&state, &tid, &line, req, crate::util::now_ms())?;
    Ok(Json(json!({ "ok": true })))
}

/// The user's reply. It may take several seconds: a prompt waits for a pause in the user's own
/// typing in that session, and the request is held meanwhile.
async fn reply(State(state): State<AppState>, Path(tid): Path<String>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<device::Replied>> {
    device_only(&caller)?;
    let req = ReplyRequest::parse(&json_of(&body, "{askId?, text?, choice?, via}")?)?;
    Ok(Json(device::reply(&state, &tid, req, crate::util::now_ms()).await?))
}

async fn dismiss(State(state): State<AppState>, Path((tid, ask)): Path<(String, String)>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<Value>> {
    device_only(&caller)?;
    nothing(&body, "dismiss")?;
    device::dismiss_ask(&state, &tid, &ask, crate::util::now_ms())?;
    Ok(Json(json!({ "ok": true })))
}

/// Gives the session an avatar, or turns its avatar on again, and tells a running session that
/// does not know of it yet.
async fn on(State(state): State<AppState>, Path(tid): Path<String>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<Value>> {
    device_only(&caller)?;
    nothing(&body, "on")?;
    let now = crate::util::now_ms();
    let turned = device::turn_on(&state, &tid, now)?;
    let briefed = if turned.brief { device::tell(&state, &tid, now).await } else { None };
    // The paste may have taken seconds: the record as it is now.
    let mut answer = serde_json::to_value(store::get(&state, &tid).unwrap_or(turned.info))?;
    if let Some(briefed) = briefed {
        answer["briefed"] = json!(briefed);
    }
    Ok(Json(answer))
}

async fn off(State(state): State<AppState>, Path(tid): Path<String>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<AvatarInfo>> {
    device_only(&caller)?;
    nothing(&body, "off")?;
    Ok(Json(device::turn_off(&state, &tid, crate::util::now_ms())?))
}

async fn reset(State(state): State<AppState>, Path(tid): Path<String>, caller: Option<Extension<Caller>>, body: Bytes) -> ApiResult<Json<AvatarInfo>> {
    device_only(&caller)?;
    nothing(&body, "reset")?;
    Ok(Json(device::reset(&state, &tid, crate::util::now_ms())?))
}

// ---------------------------------------------------------------- hook routes

/// The two steps every hook handler begins with. The bearer token is the agent token of the
/// terminal in the path: no other session's, and not the master token, which may name any
/// terminal elsewhere but never speaks as one here. Then the gate, which counts the attempt and
/// refuses one over its rate before a byte of the body is looked at.
fn enter(state: &AppState, tid: &str, headers: &HeaderMap, call: Call) -> ApiResult<Pass> {
    if state.auth.agent_from_headers(headers).as_deref() != Some(tid) {
        return Err(ApiError::unauthorized("invalid agent token"));
    }
    session::gate(state, tid, call, Via::Command, crate::util::now_ms())
}

/// The guide for a session that drives its avatar with a command, in the form its place needs
/// (this program on the host, `curl` in a dev container), and what the avatar is like now: one
/// call tells a session all it needs before it draws.
async fn hook_guide(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, uri: Uri) -> ApiResult<Response> {
    let pass = enter(&state, &tid, &headers, Call::Status)?;
    query(&uri, "guide", &[], &pass.mask(&state))?;
    let status = session::status(&state, &pass, StatusRequest::default())?;
    // A guide to drawing is for a session that may draw: one without an avatar reads why.
    if !status.allowed {
        return Err(ApiError::conflict(status.note));
    }
    let in_container = state.terminals.info(&tid).is_some_and(|info| crate::terminals::in_container(&info));
    let text = format!("{}\n## This session's avatar now\n\n{}\n", brief::guide(Via::Command, in_container), status.text());
    Ok(([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], text).into_response())
}

async fn hook_status(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, uri: Uri) -> ApiResult<Response> {
    let pass = enter(&state, &tid, &headers, Call::Status)?;
    let mask = pass.mask(&state);
    let look = switch(&query(&uri, "status", &["look"], &mask)?, "look", &mask)?;
    let status = session::status(&state, &pass, StatusRequest { guide: false, look })?;
    // Written by the struct, so `note` comes first.
    Ok(([(header::CONTENT_TYPE, "application/json")], status.text()).into_response())
}

async fn hook_design(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let pass = enter(&state, &tid, &headers, Call::Design)?;
    // Artwork alone: said by the media type, or plain from its first character, which no JSON
    // of a design starts with.
    let artwork = media(&headers) == "image/svg+xml" || unmarked(&body).iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<');
    let args = if artwork { json!({ "svg": text_of(&body, "the artwork")? }) } else { json_of(&body, "the design as a JSON object, or the artwork alone (it starts with <svg)")? };
    Ok(plain(format!("{}\nTo see it, ask for a picture and open the file.", tools::design(&state, pass, args).await?.text)))
}

/// A picture of the session's avatar as a PNG: the body is empty or `{}` for the look at rest,
/// or `{pose}` or `{frames, count?}` as `avatar_status` takes them. It counts as a status call.
async fn hook_picture(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let pass = enter(&state, &tid, &headers, Call::Status)?;
    let mask = pass.mask(&state);
    let args = if body.iter().all(u8::is_ascii_whitespace) { json!({}) } else { json_of(&body, "{} for the avatar at rest, {\"pose\": {…}} or {\"frames\": \"<animation>\"}")? };
    session::fields(&args, "picture", &["pose", "frames", "count"], &mask)?;
    let wanted = PictureRequest::parse(&args, &mask)?.unwrap_or_else(PictureRequest::rest);
    let png = tools::picture(&state, &tid, wanted).await?;
    Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
}

async fn hook_say(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, uri: Uri, body: Bytes) -> ApiResult<Json<session::SayAnswer>> {
    let pass = enter(&state, &tid, &headers, Call::Say)?;
    let mask = pass.mask(&state);
    let extras = query(&uri, "say", &["gesture", "lang", "wait"], &mask)?;
    let req = if media(&headers) == "text/plain" {
        SayRequest { text: text_of(&body, "the line")?.to_string(), lang: parameter(&extras, "lang"), gesture: parameter(&extras, "gesture"), wait: switch(&extras, "wait", &mask)? }
    } else if extras.is_empty() {
        SayRequest::parse(&json_of(&body, "{\"text\": …}, or the line itself with Content-Type: text/plain")?, &mask)?
    } else {
        return Err(ApiError::bad_request("gesture, lang and wait are query parameters only beside a text/plain line: in a JSON body they are its keys"));
    };
    Ok(Json(session::say_call(&state, &pass, req, crate::util::now_ms()).await?))
}

/// The request is held while the call waits for the answer, up to `MAX_WAIT_HOOK_SECS`. A
/// caller that goes away leaves the question open: the waiter's own `Drop` sees to it.
async fn hook_ask(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<session::AskResult>> {
    let pass = enter(&state, &tid, &headers, Call::Ask)?;
    let args = AskArgs::parse(&json_of(&body, "{\"text\", \"choices\"?, \"freeText\"?, \"waitSeconds\"?}, {\"askId\"} or {\"cancel\": true}")?, &pass.mask(&state))?;
    Ok(Json(session::ask_call(&state, &pass, args, MAX_WAIT_HOOK_SECS, crate::util::now_ms()).await?))
}

/// The answer of the act route: done, and what was done in words (what the tool answers).
#[derive(serde::Serialize)]
struct Acted {
    ok: bool,
    note: String,
}

async fn hook_act(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Acted>> {
    let pass = enter(&state, &tid, &headers, Call::Act)?;
    let req = ActRequest::parse(&json_of(&body, "{\"play\", \"mode\"?, \"times\"?} or {\"stop\"}")?, &pass.mask(&state))?;
    let acted = session::act(&state, &pass, req, crate::util::now_ms())?;
    Ok(Json(Acted { ok: true, note: acted.note }))
}

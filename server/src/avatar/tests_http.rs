//! The slice through its ways in: the MCP tools (through their handlers and through `/mcp`), the
//! hook routes, the device routes over a real listener and a sign-in, and the `workbench avatar`
//! command.
//!
//! The environment is the state tests' (`super::tests`): a real `AppState` on temporary
//! directories, stand-in agent sessions, and the router. Calls of a session carry its agent
//! token (`AuthState::issue_agent_token`); a device is a browser that signed in through
//! `/auth?token=` at a listener on a loopback port of its own. Where a reply or a brief is
//! pasted into a session, the test takes the session's place (`Env::pastes`). No test prints a
//! token: assertions name routes and answers.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::brief::{self, Brief, BriefVariant};
use super::cli;
use super::device::{HeardRequest, heard, reply, turn_off};
use super::routes::BODY_LIMIT;
use super::session::{self, NOT_A_SESSION, NO_AVATAR, QUIET, SWITCHED_OFF, TURNED_OFF};
use super::preview;
use super::store::{Ask, DESIGN_PER_MINUTE, MAX_WAIT_HOOK_SECS, MAX_WAIT_TOOL_SECS, Outcome, SAY_PER_MINUTE, list};
use super::tests::{Env, Events, clicked, env, figure, kinds, pasted};
use super::{PREALLOWED, Via};
use crate::error::{ApiError, ApiResult};
use crate::mcp::{McpCtx, McpTool, ToolOutput, call_api};
use crate::terminals::{AgentState, ProviderKind, TerminalStatus};
use crate::util::now_ms;
use crate::util::os::shell::Dialect;

const NAMES: [&str; 5] = ["avatar_status", "avatar_design", "avatar_say", "avatar_ask", "avatar_act"];
const JSON: Option<&str> = Some("application/json");

// ---------------------------------------------------------------- helpers

fn master(e: &Env) -> String {
    e.state.auth.master_token().to_string()
}

/// The agent token of a session, as its environment holds it.
fn token(e: &Env, tid: &str) -> String {
    e.state.auth.issue_agent_token(tid)
}

fn tool(name: &str) -> McpTool {
    crate::mcp::all_tools().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"))
}

fn ctx(e: &Env, tid: &str) -> McpCtx {
    McpCtx { terminal_id: Some(tid.to_string()), project_id: Some(e.pid.clone()) }
}

/// A tool call of the session `tid`, through the tool's handler: what the model reads.
async fn call(e: &Env, tid: &str, name: &str, args: Value) -> ApiResult<String> {
    (tool(name).handler)(e.state.clone(), ctx(e, tid), args).await.map(|out| match out {
        ToolOutput::Text(text) => text,
        ToolOutput::Json(value) => value.to_string(),
        ToolOutput::Images { text, .. } => text,
    })
}

/// Waits until `look` finds something, for as long as a test may take.
async fn until<T>(what: &str, mut look: impl FnMut() -> Option<T>) -> T {
    let give_up = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(found) = look() {
            return found;
        }
        assert!(tokio::time::Instant::now() < give_up, "never happened: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The open question of a session, once a call waits for its answer.
async fn waited_for(e: &Env, tid: &str) -> Ask {
    until("a call waits for an answer", || e.avatar(tid).ask.filter(|ask| ask.waiting)).await
}

/// What a request through the router was answered with.
struct Sent {
    status: u16,
    media: String,
    text: String,
}

impl Sent {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|_| panic!("the answer is not JSON: {}", self.text))
    }

    /// The code of an error answer, or "" for an answer that is none.
    fn code(&self) -> String {
        serde_json::from_str::<Value>(&self.text).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string)).unwrap_or_default()
    }

    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or_default()
    }
}

/// A request through the router with any body and media type.
async fn request(e: &Env, method: Method, path: &str, bearer: Option<&str>, media: Option<&str>, body: impl Into<Body>) -> Sent {
    let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1");
    if let Some(bearer) = bearer {
        req = req.header("authorization", format!("Bearer {bearer}"));
    }
    if let Some(media) = media {
        req = req.header("content-type", media);
    }
    let resp = e.router.clone().oneshot(req.body(body.into()).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let media = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
    Sent { status, media, text: String::from_utf8_lossy(&bytes).into_owned() }
}

fn hook(tid: &str, route: &str) -> String {
    format!("/api/hooks/avatar/{tid}/{route}")
}

/// One JSON-RPC call of `/mcp`, as an MCP client makes it.
async fn mcp(e: &Env, bearer: &str, terminal: Option<&str>, name: &str, arguments: Value) -> Value {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
    let mut req = Request::builder().method(Method::POST).uri("/mcp").header("host", "127.0.0.1").header("content-type", "application/json").header("authorization", format!("Bearer {bearer}"));
    if let Some(terminal) = terminal {
        req = req.header("x-workbench-terminal", terminal);
    }
    let resp = e.router.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap()).unwrap()
}

/// The router on a loopback port of its own.
async fn serve(e: &Env) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = e.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    addr
}

/// A signed-in browser: its session cookie and its device key, as `/auth` hands them over.
#[derive(Clone)]
struct Device {
    http: reqwest::Client,
    origin: String,
    cookie: String,
    key: String,
}

impl Device {
    async fn sign_in(e: &Env, addr: SocketAddr) -> Device {
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let r = http.get(format!("http://{addr}/auth?token={}", e.state.auth.master_token())).send().await.unwrap();
        assert!(r.status().is_redirection());
        let key = r.headers()["location"].to_str().unwrap().strip_prefix("/#wbk=").expect("the key rides in the fragment").to_string();
        let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
        Device { http, origin: format!("http://{addr}"), cookie, key }
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        answer(self.http.get(format!("{}{path}", self.origin)).header("cookie", &self.cookie).send().await.unwrap()).await
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.post(format!("{}{path}", self.origin)).header("cookie", &self.cookie).header("origin", &self.origin).header(crate::auth::KEY_HEADER, &self.key).json(&body);
        answer(r.send().await.unwrap()).await
    }
}

async fn answer(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

/// A session that drives its avatar over the wire: its hook routes with its agent token.
#[derive(Clone)]
struct Wire {
    http: reqwest::Client,
    hooks: String,
    token: String,
}

impl Wire {
    fn of(e: &Env, addr: SocketAddr, tid: &str) -> Wire {
        Wire { http: reqwest::Client::new(), hooks: format!("http://{addr}/api/hooks/avatar/{tid}"), token: token(e, tid) }
    }

    async fn post(&self, route: &str, body: Value) -> (u16, Value) {
        answer(self.http.post(format!("{}/{route}", self.hooks)).bearer_auth(&self.token).json(&body).send().await.unwrap()).await
    }
}

/// A session whose CLI has no Workbench tools, started with the Avatar option: it drives its
/// avatar with the command from the start.
fn command_session(e: &Env) -> String {
    let tid = e.agent(json!({ "avatar": true }));
    e.change(&tid, |info| info.agent.as_mut().unwrap().provider = ProviderKind::Kimi);
    e.event("terminal.created", &tid, now_ms());
    assert_eq!(e.avatar(&tid).via, Via::Command);
    tid
}

/// The test takes the place of every session a prompt is pasted into: each paste succeeds, and
/// what was pasted where is kept.
fn pastes_succeed(e: &Env) -> Arc<Mutex<Vec<(String, String)>>> {
    let mut pastes = e.pastes();
    let kept = Arc::new(Mutex::new(Vec::new()));
    let seen = kept.clone();
    tokio::spawn(async move {
        while let Some(paste) = pastes.recv().await {
            seen.lock().push((paste.tid.clone(), paste.prompt.clone()));
            let _ = paste.done.send(Ok(()));
        }
    });
    kept
}

fn sorted_keys(v: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = v.as_object().expect("an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

// ---------------------------------------------------------------- the MCP tools

#[test]
fn the_five_tools_say_what_they_do_and_what_they_take() {
    let tools = super::mcp_tools();
    assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), NAMES);
    let arguments: [&[&str]; 5] = [
        &["count", "frames", "guide", "look", "picture", "pose"],
        &["animations", "drives", "gaze", "keep", "mouth", "name", "parents", "pivots", "roles", "states", "svg", "voice"],
        &["gesture", "lang", "text", "wait"],
        &["askId", "cancel", "choices", "freeText", "gesture", "lang", "text", "waitSeconds"],
        &["mode", "play", "stop", "times"],
    ];
    for (t, takes) in tools.iter().zip(arguments) {
        // None changes remote state.
        assert!(!t.mutating, "{}", t.name);
        let length = t.description.chars().count();
        assert!((80..700).contains(&length), "{}: a description of {length} characters", t.name);
        assert_eq!(t.input_schema["type"], "object", "{}", t.name);
        let properties = &t.input_schema["properties"];
        // What the schema names is what the call takes: no tool takes a terminal id.
        assert_eq!(sorted_keys(properties), takes, "{}", t.name);
        for (key, property) in properties.as_object().unwrap() {
            assert!(property["description"].as_str().is_some_and(|d| d.len() >= 20), "{}.{key} says what it is", t.name);
        }
    }
    // Reading is read-only; the rest does something for the user to see, and the two a session
    // calls all the time stay out of the Activity view.
    let traits: Vec<(&str, bool, bool)> = tools.iter().map(|t| (t.name.as_str(), t.acts, t.quiet)).collect();
    assert_eq!(traits, [("avatar_status", false, false), ("avatar_design", true, false), ("avatar_say", true, true), ("avatar_ask", true, false), ("avatar_act", true, true)]);
    assert_eq!(tools[2].input_schema["required"], json!(["text"]));
    // They are among the tools `/mcp` lists, and the ones a session with an avatar may call unasked.
    let all: Vec<String> = crate::mcp::all_tools().into_iter().map(|t| t.name).collect();
    assert!(NAMES.iter().all(|name| all.iter().filter(|t| t == name).count() == 1));
    assert_eq!(PREALLOWED.map(|rule| rule.strip_prefix("mcp__workbench__").unwrap()), NAMES);
    // The waits the descriptions promise are the limits.
    assert!(tools[3].description.contains(&format!("0 to {MAX_WAIT_TOOL_SECS}")));
}

#[tokio::test]
async fn a_tool_acts_for_the_session_that_calls_it_and_for_no_one_else() {
    let e = env().await;
    for name in NAMES {
        // The master token without a terminal: nobody's avatar.
        let err = (tool(name).handler)(e.state.clone(), McpCtx::default(), json!({})).await.unwrap_err();
        assert_eq!((err.status.as_u16(), err.code, err.message.as_str()), (400, "bad_request", NOT_A_SESSION), "{name}");
        // A terminal id that names nothing (the master token may send any text there).
        for tid in ["", "nope", "../../etc/passwd"] {
            let err = call(&e, tid, name, json!({})).await.unwrap_err();
            assert_eq!((err.status.as_u16(), err.message.as_str()), (400, NOT_A_SESSION), "{name} for {tid:?}");
        }
    }
    assert!(!e.state.paths.data_dir.join("avatar").join("nope.json").exists());

    // A session the user gave no avatar is told so by every tool that would use one, and is
    // remembered as having asked.
    let tid = e.agent(json!({}));
    for name in &NAMES[1..] {
        let err = call(&e, &tid, name, json!({})).await.unwrap_err();
        assert_eq!((err.status.as_u16(), err.code, err.message.as_str()), (409, "conflict", NO_AVATAR), "{name}");
    }
    assert_eq!(list(&e.state).asked, [tid.clone()]);
    assert!(list(&e.state).avatars.is_empty());
    // Once the session ended its tools act for nobody.
    let had = e.with_avatar();
    e.change(&had, |info| info.status = TerminalStatus::Exited);
    assert_eq!(call(&e, &had, "avatar_say", json!({ "text": "Still there?" })).await.unwrap_err().message, NOT_A_SESSION);
}

#[tokio::test]
async fn avatar_status_begins_with_what_to_do_and_brings_the_guide_when_asked() {
    let e = env().await;
    // Without an avatar: why, and no guide to drawing one.
    let plain = e.agent(json!({}));
    let text = call(&e, &plain, "avatar_status", json!({ "guide": true })).await.unwrap();
    assert!(text.starts_with("{\n  \"note\": \"This session has no avatar. "), "{text}");
    let status: Value = serde_json::from_str(&text).unwrap();
    assert_eq!((&status["allowed"], &status["avatar"]), (&json!(false), &Value::Null));
    assert!(list(&e.state).asked.is_empty(), "looking is not asking");

    let tid = e.with_avatar();
    let text = call(&e, &tid, "avatar_status", json!({})).await.unwrap();
    assert!(text.starts_with("{\n  \"note\": \"You have a live avatar: say the short version with avatar_say, ask with avatar_ask.\",\n  \"allowed\": true,"), "{text}");
    let status: Value = serde_json::from_str(&text).unwrap();
    assert_eq!((&status["avatar"]["designed"], &status["avatar"]["via"], &status["limits"]["sayChars"]), (&json!(false), &json!("tools"), &json!(400)));
    assert!(status["avatar"].get("design").is_none());

    // With the guide: the guide for a session with the tools first, then the same status.
    let with_guide = call(&e, &tid, "avatar_status", json!({ "guide": true })).await.unwrap();
    let (guide, after) = with_guide.split_once("\n## This session's avatar now\n\n").expect("the guide, then the status");
    assert_eq!(guide, brief::guide(Via::Tools, false));
    assert!(guide.starts_with("# Your avatar\n") && guide.contains("## Your tools") && guide.contains("avatar_design") && !guide.contains("curl"), "{guide}");
    assert_eq!(serde_json::from_str::<Value>(after).unwrap(), status);

    // With look: the stored design, in the form of a design call.
    e.design(&tid, figure(), now_ms()).unwrap();
    let status: Value = serde_json::from_str(&call(&e, &tid, "avatar_status", json!({ "look": true })).await.unwrap()).unwrap();
    assert_eq!((&status["avatar"]["designed"], &status["avatar"]["name"]), (&json!(true), &json!("Dot")));
    assert!(status["avatar"]["design"]["svg"].as_str().is_some_and(|svg| svg.starts_with("<svg viewBox='0 0 200 200'>")), "{status}");
    assert_eq!(status["avatar"]["animations"], json!(["blink", "fade", "hop", "spin"]));

    // What it does not take is refused by name.
    let err = call(&e, &tid, "avatar_status", json!({ "terminalId": plain })).await.unwrap_err();
    assert_eq!(err.status.as_u16(), 400);
    assert!(err.message.contains("status has no terminalId"), "{}", err.message);
}

#[tokio::test]
async fn a_session_designs_speaks_and_gestures_through_its_tools() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);

    let answer = call(&e, &tid, "avatar_design", figure()).await.unwrap();
    assert!(answer.starts_with("Saved as revision 1, named Dot.\nParts (id tag x y w h pivot x y, in root coordinates, at rest):\nbody g "), "{answer}");
    assert!(answer.contains("\nYour animations: blink, fade, hop, spin.") && answer.contains("\nUsage: svg "), "{answer}");
    assert_eq!(events.kinds(), ["avatar.changed"]);
    assert!(e.avatar(&tid).designed);
    // A refused design changes nothing and says what to fix.
    let err = call(&e, &tid, "avatar_design", json!({ "svg": "<svg viewBox='0 0 200 200'><use href='#body'/></svg>" })).await.unwrap_err();
    assert_eq!((err.status.as_u16(), err.code), (400, "bad_request"));
    assert!(err.message.contains("href refers to #body, which is not in this svg") && err.message.ends_with("\nNothing was changed."), "{}", err.message);
    assert_eq!((e.avatar(&tid).revision, events.kinds().len()), (1, 0));

    // A line is queued; the answer is one sentence.
    let said = call(&e, &tid, "avatar_say", json!({ "text": "Tests are green.", "gesture": "hop" })).await.unwrap();
    let line = e.avatar(&tid).lines[0].clone();
    assert_eq!(said, format!("Queued ({}).", line.id));
    assert_eq!((line.text.as_str(), line.gesture.as_deref(), line.outcome), ("Tests are green.", Some("hop"), None));
    assert_eq!(events.kinds(), ["avatar.say"]);
    let said = call(&e, &tid, "avatar_say", json!({ "text": "See `src/main.rs` for the rest." })).await.unwrap();
    assert!(said.ends_with("It will be read out literally: use plain words."), "{said}");
    let err = call(&e, &tid, "avatar_say", json!({ "gesture": "hop" })).await.unwrap_err();
    assert!(err.status.as_u16() == 400 && err.message.starts_with("say needs text"), "{}", err.message);
    // With wait the answer is what became of the line.
    let state = e.state.clone();
    let (waiting_tid, context) = (tid.clone(), ctx(&e, &tid));
    let waits = tokio::spawn(async move { (tool("avatar_say").handler)(state, context, json!({ "text": "Shall I wait?", "wait": true })).await });
    let asked = until("the line that is waited for", || e.avatar(&waiting_tid).lines.into_iter().find(|l| l.text == "Shall I wait?")).await;
    heard(&e.state, &tid, &asked.id, HeardRequest { outcome: Outcome::Spoken, voice: Some("Test Voice".into()) }, now_ms()).unwrap();
    let Ok(Ok(Ok(ToolOutput::Text(spoken)))) = tokio::time::timeout(Duration::from_secs(30), waits).await else { panic!("say with wait did not return") };
    assert_eq!(spoken, format!("{} spoken aloud by Test Voice.", asked.id));
    events.drain();

    // While the session needs the user in its terminal the avatar says and asks nothing.
    e.set_state(&tid, AgentState::NeedsPermission);
    for (name, args) in [("avatar_say", json!({ "text": "May I?" })), ("avatar_ask", json!({ "text": "May I?" }))] {
        let err = call(&e, &tid, name, args).await.unwrap_err();
        assert_eq!((err.status.as_u16(), err.code, err.message.as_str()), (409, "conflict", QUIET), "{name}");
    }
    e.set_state(&tid, AgentState::Idle);
    assert!(events.kinds().is_empty());

    // Gestures by name.
    assert_eq!(call(&e, &tid, "avatar_act", json!({ "play": "hop" })).await.unwrap(), "Playing hop.");
    assert_eq!(call(&e, &tid, "avatar_act", json!({ "play": "spin", "mode": "loop" })).await.unwrap(), "Looping spin until you stop it, play another or 2 minutes pass.");
    assert_eq!(e.avatar(&tid).held.map(|held| held.name), Some("spin".to_string()));
    assert_eq!(call(&e, &tid, "avatar_act", json!({ "stop": "all" })).await.unwrap(), "Stopped every gesture.");
    assert!(e.avatar(&tid).held.is_none());
    let err = call(&e, &tid, "avatar_act", json!({ "play": "wink" })).await.unwrap_err();
    assert!(err.message.starts_with("No animation wink. Yours: blink, fade, hop, spin; built in: "), "{}", err.message);
    let err = call(&e, &tid, "avatar_act", json!({ "play": "hop", "speed": 2 })).await.unwrap_err();
    assert!(err.message.contains("act has no speed"), "{}", err.message);
}

#[tokio::test]
async fn avatar_ask_waits_in_the_call_and_returns_the_answer() {
    let e = env().await;
    let tid = e.with_avatar();
    let (state, context) = (e.state.clone(), ctx(&e, &tid));
    let asking = tokio::spawn(async move { (tool("avatar_ask").handler)(state, context, json!({ "text": "Deploy now?", "choices": ["Yes", "Not yet"], "waitSeconds": 30 })).await });
    let ask = waited_for(&e, &tid).await;
    assert_eq!((ask.text.as_str(), ask.choices.len(), ask.free_text), ("Deploy now?", 2, true));
    // The user clicks the second choice while the call waits: the call has the answer.
    let replied = reply(&e.state, &tid, clicked(&ask.id, 1), now_ms()).await.unwrap();
    assert_eq!(serde_json::to_value(replied).unwrap(), json!({ "delivered": "answer" }));
    let Ok(Ok(Ok(ToolOutput::Text(answer)))) = tokio::time::timeout(Duration::from_secs(30), asking).await else { panic!("the call did not return its answer") };
    assert_eq!(answer, "{\n  \"answered\": true,\n  \"text\": \"Not yet\",\n  \"choice\": 1,\n  \"via\": \"click\"\n}");

    // A call that waits no time asks and returns: the question stays open.
    let answer = call(&e, &tid, "avatar_ask", json!({ "text": "And the migration?", "waitSeconds": 0 })).await.unwrap();
    assert!(answer.starts_with("{\n  \"answered\": false,\n  \"reason\": \"timeout\",\n  \"askId\": \"Q"), "{answer}");
    let unanswered: Value = serde_json::from_str(&answer).unwrap();
    let ask_id = unanswered["askId"].as_str().unwrap().to_string();
    assert!(unanswered["note"].as_str().unwrap().starts_with("Not answered yet. The question stays on screen. Do not ask again"), "{unanswered}");
    assert_eq!(e.avatar(&tid).ask.map(|ask| (ask.id, ask.waiting)), Some((ask_id.clone(), false)));
    // Its id waits again; cancel withdraws it.
    let again: Value = serde_json::from_str(&call(&e, &tid, "avatar_ask", json!({ "askId": ask_id, "waitSeconds": 0 })).await.unwrap()).unwrap();
    assert_eq!((&again["answered"], &again["reason"], &again["askId"]), (&json!(false), &json!("timeout"), &json!(ask_id)));
    let withdrawn: Value = serde_json::from_str(&call(&e, &tid, "avatar_ask", json!({ "cancel": true })).await.unwrap()).unwrap();
    assert_eq!((&withdrawn["reason"], &withdrawn["askId"]), (&json!("withdrawn"), &json!(ask_id)));
    assert!(e.avatar(&tid).ask.is_none());

    // The wait of a tool call has its limit, and an id that is not the open question's is told.
    let err = call(&e, &tid, "avatar_ask", json!({ "text": "Shall I?", "waitSeconds": MAX_WAIT_TOOL_SECS + 1 })).await.unwrap_err();
    assert_eq!((err.status.as_u16(), err.message.as_str()), (400, "waitSeconds 51: 0 to 50"));
    let err = call(&e, &tid, "avatar_ask", json!({ "askId": ask_id })).await.unwrap_err();
    assert_eq!((err.status.as_u16(), err.code), (409, "not_pending"));
    let err = call(&e, &tid, "avatar_ask", json!({})).await.unwrap_err();
    assert!(err.message.starts_with("ask needs text"), "{}", err.message);
}

#[tokio::test]
async fn the_tools_reach_the_calling_session_through_mcp() {
    let e = env().await;
    let (tid, other) = (e.with_avatar(), e.with_avatar());
    let own = token(&e, &tid);

    // The session's own token: the line is its avatar's, whoever else has one.
    let said = mcp(&e, &own, None, "avatar_say", json!({ "text": "Hello from the session." })).await;
    assert_eq!(said["result"]["isError"], false, "{said}");
    let line = e.avatar(&tid).lines[0].clone();
    assert_eq!(said["result"]["content"][0]["text"], format!("Queued ({}).", line.id));
    assert!(e.avatar(&other).lines.is_empty());
    // A header cannot make it another session: an agent token is bound to its terminal.
    let said = mcp(&e, &own, Some(&other), "avatar_say", json!({ "text": "Hello again." })).await;
    assert_eq!(said["result"]["isError"], false, "{said}");
    assert_eq!((e.avatar(&tid).lines.len(), e.avatar(&other).lines.len()), (2, 0));

    // A refusal reaches the model as the tool's words.
    let refused = mcp(&e, &own, None, "avatar_act", json!({ "play": "wink" })).await;
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    assert!(refused["result"]["content"][0]["text"].as_str().unwrap().starts_with("No animation wink."), "{refused}");

    // The master token is nobody's session, unless it names one, as for every Workbench tool.
    let master = master(&e);
    let nobody = mcp(&e, &master, None, "avatar_status", json!({})).await;
    assert_eq!((&nobody["result"]["isError"], &nobody["result"]["content"][0]["text"]), (&json!(true), &json!(NOT_A_SESSION)));
    let named = mcp(&e, &master, Some(&other), "avatar_say", json!({ "text": "Said for the other." })).await;
    assert_eq!(named["result"]["isError"], false, "{named}");
    assert_eq!(e.avatar(&other).lines.len(), 1);
}

#[tokio::test]
async fn a_client_that_goes_away_during_avatar_ask_leaves_the_question_open() {
    let e = env().await;
    let tid = e.with_avatar();
    let own = token(&e, &tid);
    let mut pastes = e.pastes();
    let mut events = Events::of(&e);

    // The call as an MCP client makes it, held open while it waits for the answer.
    let body = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "avatar_ask", "arguments": { "text": "Shall I go on?", "choices": ["Yes", "No"], "waitSeconds": 50 } } });
    let req = Request::builder().method(Method::POST).uri("/mcp").header("host", "127.0.0.1").header("content-type", "application/json").header("authorization", format!("Bearer {own}")).body(Body::from(body.to_string())).unwrap();
    let router = e.router.clone();
    let client = tokio::spawn(async move { router.oneshot(req).await.map(|resp| resp.status().as_u16()) });
    let ask = waited_for(&e, &tid).await;
    assert_eq!(kinds(&events.drain()), ["avatar.say", "avatar.ask"], "said, and opened with a call that waits");
    assert!(ask.waiting);

    // The client disconnects: its request is dropped, the platform aborts the tool's task, and
    // the call that waited takes itself out of the question.
    client.abort();
    assert!(client.await.unwrap_err().is_cancelled());
    let left = until("the call left the question", || e.avatar(&tid).ask.filter(|ask| !ask.waiting)).await;
    assert_eq!((left.id.as_str(), left.text.as_str()), (ask.id.as_str(), "Shall I go on?"));
    let after = events.drain();
    assert_eq!(kinds(&after), ["avatar.ask"]);
    assert_eq!(after[0].data, json!({ "terminalId": tid, "change": "waiter_left", "ask": left }));
    // Nothing of the call is in the Activity view: it never returned.
    assert!(e.state.platform.activity.calls(10).is_empty());

    // The question is still the user's to answer, and the answer reaches the session as a prompt.
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, clicked(&ask.id, 0), now_ms())).await;
    assert_eq!(serde_json::to_value(replied.unwrap()).unwrap(), json!({ "delivered": "prompt" }));
    assert_eq!(prompt, format!("[via avatar, answer with avatar_say] Answer to question {}: choice 1, \"Yes\"", ask.id));
    assert!(e.avatar(&tid).ask.is_none());
}

// ---------------------------------------------------------------- the hook routes

#[tokio::test]
async fn a_hook_route_takes_the_token_of_its_own_session_and_no_other() {
    let e = env().await;
    let (tid, other) = (e.with_avatar(), e.with_avatar());
    let (own, others, master) = (token(&e, &tid), token(&e, &other), master(&e));
    let routes = [
        (Method::GET, "guide", ""),
        (Method::GET, "status", ""),
        (Method::POST, "design", "{\"name\":\"Dot\"}"),
        (Method::POST, "say", "{\"text\":\"Hello.\"}"),
        (Method::POST, "ask", "{\"text\":\"Shall I?\",\"waitSeconds\":0}"),
        (Method::POST, "act", "{\"play\":\"nod\"}"),
    ];
    let mut events = Events::of(&e);
    for (method, route, body) in &routes {
        // No token, and a body that is not even JSON: the token is asked for before anything is read.
        let sent = request(&e, method.clone(), &hook(&tid, route), None, JSON, "{not json").await;
        assert_eq!((sent.status, sent.code().as_str(), sent.message().as_str()), (401, "unauthorized", "invalid agent token"), "{route} without a token");
        // Another session's token, the master token (which names any terminal elsewhere), a made-up one.
        for (whose, wrong) in [("another session's", others.as_str()), ("the master", master.as_str()), ("a made-up", "wba_made-up")] {
            let sent = request(&e, method.clone(), &hook(&tid, route), Some(wrong), JSON, *body).await;
            assert_eq!((sent.status, sent.code().as_str()), (401, "unauthorized"), "{route} with {whose} token");
        }
        // Its own token on another session's route.
        let sent = request(&e, method.clone(), &hook(&other, route), Some(&own), JSON, *body).await;
        assert_eq!(sent.status, 401, "{route} of another session");
    }
    // None of it did anything.
    assert!(events.kinds().is_empty());
    assert!(e.avatar(&tid).lines.is_empty() && e.avatar(&other).lines.is_empty());
    assert_eq!((e.avatar(&tid).via, e.avatar(&tid).revision), (Via::Tools, 0));

    for (method, route, body) in &routes {
        let sent = request(&e, method.clone(), &hook(&tid, route), Some(&own), JSON, *body).await;
        assert_eq!(sent.status, 200, "{route}: {}", sent.text);
    }
    assert_eq!((e.avatar(&tid).via, e.avatar(&tid).name.as_str(), e.avatar(&tid).lines.len()), (Via::Command, "Dot", 2));
    assert!(e.avatar(&other).lines.is_empty());

    // The same token opens no device route: it is no key to `/api/avatar`.
    let line = e.avatar(&tid).lines[0].id.clone();
    let ask = e.avatar(&tid).ask.expect("the question is open").id;
    for (method, path) in [
        (Method::GET, "/api/avatar".to_string()),
        (Method::GET, format!("/api/avatar/{tid}")),
        (Method::POST, format!("/api/avatar/{tid}/lines/{line}/claim")),
        (Method::POST, format!("/api/avatar/{tid}/lines/{line}/heard")),
        (Method::POST, format!("/api/avatar/{tid}/reply")),
        (Method::POST, format!("/api/avatar/{tid}/asks/{ask}/dismiss")),
        (Method::POST, format!("/api/avatar/{tid}/on")),
        (Method::POST, format!("/api/avatar/{tid}/off")),
        (Method::POST, format!("/api/avatar/{tid}/reset")),
    ] {
        let sent = request(&e, method, &path, Some(&own), JSON, "{}").await;
        assert_eq!((sent.status, sent.code().as_str()), (401, "unauthorized"), "{path}");
    }
    assert!(e.avatar(&tid).on && e.avatar(&tid).ask.is_some());
}

#[tokio::test]
async fn a_session_without_the_tools_does_the_same_through_its_hook_routes() {
    let e = env().await;
    let tid = e.with_avatar();
    let own = token(&e, &tid);
    let post = |route: &str, media: Option<&'static str>, body: Vec<u8>| {
        let (path, e, own) = (hook(&tid, route), &e, own.clone());
        async move { request(e, Method::POST, &path, Some(&own), media, body).await }
    };

    // A design as JSON: the answer is the text a tool call gets.
    let sent = post("design", JSON, figure().to_string().into_bytes()).await;
    assert_eq!((sent.status, sent.media.as_str()), (200, "text/plain; charset=utf-8"), "{}", sent.text);
    assert!(sent.text.starts_with("Saved as revision 1, named Dot.\nParts (id tag x y w h pivot x y, in root coordinates, at rest):\nbody g "), "{}", sent.text);
    // How a session drives its avatar is the interface it used last.
    assert_eq!(e.avatar(&tid).via, Via::Command);
    // A gesture of that look, and one it does not have.
    let sent = post("act", JSON, b"{\"play\":\"hop\",\"times\":2}".to_vec()).await;
    assert_eq!((sent.status, sent.json()), (200, json!({ "ok": true, "note": "Playing hop 2 times." })));
    let sent = post("act", JSON, b"{\"play\":\"wink\"}".to_vec()).await;
    assert_eq!((sent.status, sent.code().as_str()), (400, "bad_request"), "{}", sent.text);
    assert!(sent.message().starts_with("No animation wink. Yours: blink, fade, hop, spin; built in: "), "{}", sent.message());
    // A line as JSON.
    let sent = post("say", JSON, b"{\"text\":\"The build is green.\"}".to_vec()).await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    let first = e.avatar(&tid).lines[0].clone();
    assert_eq!(sent.json(), json!({ "lineId": first.id, "note": format!("Queued ({}).", first.id) }));
    // The line itself, with what goes with it in the query.
    let path = format!("{}?gesture=hop&lang=de", hook(&tid, "say"));
    let sent = request(&e, Method::POST, &path, Some(&own), Some("text/plain; charset=utf-8"), "Alles gr\u{fc}n.\n").await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    let second = e.avatar(&tid).lines[1].clone();
    assert_eq!((second.text.as_str(), second.lang.as_deref(), second.gesture.as_deref()), ("Alles gr\u{fc}n.", Some("de"), Some("hop")));
    // With wait the request is held until a device says what became of the line.
    heard(&e.state, &tid, &first.id, HeardRequest { outcome: Outcome::Shown, voice: None }, now_ms()).unwrap();
    let (router, waiting_path, bearer) = (e.router.clone(), format!("{}?wait=1", hook(&tid, "say")), format!("Bearer {own}"));
    let waits = tokio::spawn(async move {
        let req = Request::builder().method(Method::POST).uri(waiting_path).header("host", "127.0.0.1").header("authorization", bearer).header("content-type", "text/plain").body(Body::from("Do you hear me?")).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        (resp.status().as_u16(), serde_json::from_slice::<Value>(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap())
    });
    let third = until("the line that is waited for", || e.avatar(&tid).lines.into_iter().find(|l| l.text == "Do you hear me?")).await;
    heard(&e.state, &tid, &third.id, HeardRequest { outcome: Outcome::Skipped, voice: None }, now_ms()).unwrap();
    let (status, answer) = tokio::time::timeout(Duration::from_secs(30), waits).await.expect("say with wait returned").unwrap();
    assert_eq!(status, 200);
    assert_eq!(answer, json!({ "lineId": third.id, "outcome": "skipped", "note": format!("{} skipped by the user. Earlier: {} shown as a caption, not spoken.", third.id, first.id) }));

    // The artwork alone: said by the media type.
    let art = "<svg viewBox='0 0 200 200'><g id='body'><circle id='face' cx='100' cy='100' r='40' fill='#3d85c6'/></g></svg>";
    let sent = post("design", Some("image/svg+xml"), art.as_bytes().to_vec()).await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    assert!(sent.text.starts_with("Saved as revision 2, named Dot.") && sent.text.contains("\nCleared: "), "{}", sent.text);
    // Or plain from its first character, whatever the client calls its body (a byte order mark
    // before it is no part of it).
    let sent = post("design", Some("application/x-www-form-urlencoded"), format!("\u{feff}\n  {art}").into_bytes()).await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    assert!(sent.text.starts_with("Nothing to save: the avatar already has this design. It is revision 2"), "{}", sent.text);
    // A refusal says what to fix; a body that is neither says what the route takes.
    let sent = post("design", Some("image/svg+xml"), b"<svg viewBox='0 0 200 200'><use href='#face'/></svg>".to_vec()).await;
    assert_eq!((sent.status, sent.code().as_str()), (400, "bad_request"));
    assert!(sent.message().contains("href refers to #face, which is not in this svg") && sent.message().ends_with("\nNothing was changed."), "{}", sent.message());
    let sent = post("design", JSON, b"name=Dot".to_vec()).await;
    assert_eq!(sent.status, 400);
    assert!(sent.message().starts_with("the body is not JSON") && sent.message().contains("or the artwork alone"), "{}", sent.message());
    let sent = post("design", Some("image/svg+xml"), vec![0xff, 0xfe, b'<']).await;
    assert_eq!((sent.status, sent.message().as_str()), (400, "the artwork is not UTF-8 text"));
    assert_eq!(e.avatar(&tid).revision, 2);

    // A question, its second wait and its withdrawal.
    let sent = post("ask", JSON, b"{\"text\":\"Shall I deploy?\",\"choices\":[\"Yes\",\"No\"],\"waitSeconds\":0}".to_vec()).await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    assert!(sent.text.starts_with("{\"answered\":false,\"reason\":\"timeout\",\"askId\":\"Q"), "{}", sent.text);
    let ask_id = sent.json()["askId"].as_str().unwrap().to_string();
    let sent = post("ask", JSON, json!({ "askId": ask_id, "waitSeconds": 0 }).to_string().into_bytes()).await;
    assert_eq!((sent.status, &sent.json()["reason"]), (200, &json!("timeout")));
    // A hook request may be held longer than a tool call, and no longer than its own limit.
    let sent = post("ask", JSON, json!({ "askId": ask_id, "waitSeconds": MAX_WAIT_HOOK_SECS + 1 }).to_string().into_bytes()).await;
    assert_eq!((sent.status, sent.message().as_str()), (400, "waitSeconds 111: 0 to 110"));
    let sent = post("ask", JSON, b"{\"cancel\":true}".to_vec()).await;
    assert_eq!((sent.status, &sent.json()["reason"], &sent.json()["askId"]), (200, &json!("withdrawn"), &json!(ask_id)));

    // The status: the JSON of avatar_status, written so that what to do comes first.
    let sent = request(&e, Method::GET, &hook(&tid, "status"), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, sent.media.as_str()), (200, "application/json"));
    assert!(sent.text.starts_with("{\n  \"note\": \"You have a live avatar: say the short version with your avatar say command, ask with your avatar ask command.\",\n  \"allowed\": true,"), "{}", sent.text);
    assert!(sent.json()["avatar"].get("design").is_none());
    let sent = request(&e, Method::GET, &format!("{}?look=1", hook(&tid, "status")), Some(&own), None, Body::empty()).await;
    assert!(sent.json()["avatar"]["design"]["svg"].as_str().is_some_and(|svg| svg.starts_with("<svg ")), "{}", sent.text);
    let sent = request(&e, Method::GET, &format!("{}?guide=1", hook(&tid, "status")), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, sent.message().as_str()), (400, "status has no query parameter guide: it takes look"));

    // The guide: for a session on the host, the forms of this program, and the status after it.
    let sent = request(&e, Method::GET, &hook(&tid, "guide"), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, sent.media.as_str()), (200, "text/markdown; charset=utf-8"));
    let (guide, status) = sent.text.split_once("\n## This session's avatar now\n\n").expect("the guide, then the status");
    assert_eq!(guide, brief::guide(Via::Command, false));
    assert!(guide.starts_with("# Your avatar\n") && guide.contains("## Your commands") && guide.contains(" avatar say 'One short line.'") && !guide.contains("curl") && !guide.contains("{wb}"), "{guide}");
    assert_eq!(serde_json::from_str::<Value>(status).unwrap()["avatar"]["name"], "Dot");
    let sent = request(&e, Method::GET, &format!("{}?short=1", hook(&tid, "guide")), Some(&own), None, Body::empty()).await;
    assert_eq!(sent.status, 400);
}

#[tokio::test]
async fn a_hook_call_says_how_its_body_is_to_be_sent() {
    let e = env().await;
    let tid = e.with_avatar();
    let own = token(&e, &tid);
    // Query parameters go with a text/plain line, and only the three it has.
    let sent = request(&e, Method::POST, &format!("{}?wait=1", hook(&tid, "say")), Some(&own), JSON, "{\"text\":\"Hello.\"}").await;
    assert_eq!(sent.status, 400);
    assert!(sent.message().contains("query parameters only beside a text/plain line"), "{}", sent.message());
    let sent = request(&e, Method::POST, &format!("{}?volume=11", hook(&tid, "say")), Some(&own), Some("text/plain"), "Hello.").await;
    assert_eq!((sent.status, sent.message().as_str()), (400, "say has no query parameter volume: it takes gesture, lang, wait"));
    let sent = request(&e, Method::POST, &format!("{}?wait=maybe", hook(&tid, "say")), Some(&own), Some("text/plain"), "Hello.").await;
    assert_eq!((sent.status, sent.message().as_str()), (400, "wait=maybe: write wait=1"));
    let sent = request(&e, Method::POST, &format!("{}?lang=de&lang=fr", hook(&tid, "say")), Some(&own), Some("text/plain"), "Hello.").await;
    assert_eq!((sent.status, sent.message().as_str()), (400, "say has the query parameter lang twice"));
    // curl's own media type for a body is neither JSON nor a line: the answer names both forms.
    let sent = request(&e, Method::POST, &hook(&tid, "say"), Some(&own), Some("application/x-www-form-urlencoded"), "Hello there.").await;
    assert_eq!(sent.status, 400);
    assert!(sent.message().starts_with("the body is not JSON") && sent.message().contains("Content-Type: text/plain"), "{}", sent.message());
    assert!(e.avatar(&tid).lines.is_empty());
    // What a call does not take is refused by name, as for a tool.
    let sent = request(&e, Method::POST, &hook(&tid, "act"), Some(&own), JSON, "{\"play\":\"nod\",\"terminalId\":\"x\"}").await;
    assert_eq!(sent.status, 400);
    assert!(sent.message().contains("act has no terminalId"), "{}", sent.message());
    let sent = request(&e, Method::POST, &hook(&tid, "ask"), Some(&own), JSON, "[]").await;
    assert_eq!(sent.status, 400, "{}", sent.text);
}

#[tokio::test]
async fn the_guide_a_session_is_given_fits_where_it_runs() {
    let e = env().await;
    // In a dev container there is no Workbench program: the same calls with curl.
    let boxed = e.agent(json!({ "avatar": true, "inContainer": true }));
    e.event("terminal.created", &boxed, now_ms());
    let sent = request(&e, Method::GET, &hook(&boxed, "guide"), Some(&token(&e, &boxed)), None, Body::empty()).await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    assert!(sent.text.contains("## Your commands") && sent.text.contains("curl -sS --fail-with-body -H @-") && sent.text.contains("python3"), "{}", sent.text);
    assert!(!sent.text.contains(" avatar say '") && !sent.text.contains("## Your tools"), "{}", sent.text);

    // A session without an avatar gets no guide to drawing one: it reads why.
    let plain = e.agent(json!({}));
    let own = token(&e, &plain);
    let sent = request(&e, Method::GET, &hook(&plain, "guide"), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, sent.code().as_str(), sent.message().as_str()), (409, "conflict", NO_AVATAR));
    // Its status says the same, as an answer.
    let sent = request(&e, Method::GET, &hook(&plain, "status"), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, &sent.json()["allowed"], &sent.json()["avatar"]), (200, &json!(false), &Value::Null));
    assert!(list(&e.state).asked.is_empty());
    // With avatars for any session it is told how to draw the one its first call gives it.
    e.state.config.write().avatar.sessions = Some(super::Sessions::Any);
    let sent = request(&e, Method::GET, &hook(&plain, "guide"), Some(&own), None, Body::empty()).await;
    assert_eq!(sent.status, 200, "{}", sent.text);
    assert!(sent.text.contains("## Your commands") && sent.text.contains("\"avatar\": null"), "{}", sent.text);
}

#[tokio::test]
async fn the_gate_answers_a_hook_call_as_it_answers_a_tool() {
    let e = env().await;
    let calls = [("design", "{\"name\":\"Dot\"}"), ("say", "{\"text\":\"Hello.\"}"), ("ask", "{\"text\":\"Shall I?\",\"waitSeconds\":0}"), ("act", "{\"play\":\"nod\"}")];

    // A session the user gave no avatar: refused, and remembered as having asked.
    let plain = e.agent(json!({}));
    let own = token(&e, &plain);
    for (route, body) in calls {
        let sent = request(&e, Method::POST, &hook(&plain, route), Some(&own), JSON, body).await;
        assert_eq!((sent.status, sent.code().as_str(), sent.message().as_str()), (409, "conflict", NO_AVATAR), "{route}");
    }
    assert_eq!(list(&e.state).asked, [plain.clone()]);

    // An avatar the user turned off.
    let tid = e.with_avatar();
    let own = token(&e, &tid);
    turn_off(&e.state, &tid, now_ms()).unwrap();
    for (route, body) in calls {
        let sent = request(&e, Method::POST, &hook(&tid, route), Some(&own), JSON, body).await;
        assert_eq!((sent.status, sent.code().as_str(), sent.message().as_str()), (409, "conflict", TURNED_OFF), "{route}");
    }
    let sent = request(&e, Method::GET, &hook(&tid, "guide"), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, sent.message().as_str()), (409, TURNED_OFF));
    let sent = request(&e, Method::GET, &hook(&tid, "status"), Some(&own), None, Body::empty()).await;
    assert_eq!((sent.status, &sent.json()["note"], &sent.json()["allowed"]), (200, &json!(TURNED_OFF), &json!(false)));

    // A session that ended (its token outlives it only in this test).
    let ended = e.with_avatar();
    let own = token(&e, &ended);
    e.change(&ended, |info| info.status = TerminalStatus::Exited);
    let sent = request(&e, Method::POST, &hook(&ended, "say"), Some(&own), JSON, "{\"text\":\"Hello.\"}").await;
    assert_eq!((sent.status, sent.code().as_str(), sent.message().as_str()), (400, "bad_request", NOT_A_SESSION));

    // Avatars switched off in Settings: every route of a session answers 403, the reads too.
    let tid = e.with_avatar();
    let own = token(&e, &tid);
    e.state.config.write().avatar.enabled = Some(false);
    for (route, body) in calls {
        let sent = request(&e, Method::POST, &hook(&tid, route), Some(&own), JSON, body).await;
        assert_eq!((sent.status, sent.code().as_str(), sent.message().as_str()), (403, "forbidden", SWITCHED_OFF), "{route}");
    }
    for route in ["guide", "status"] {
        let sent = request(&e, Method::GET, &hook(&tid, route), Some(&own), None, Body::empty()).await;
        assert_eq!((sent.status, sent.message().as_str()), (403, SWITCHED_OFF), "{route}");
    }
    assert_eq!(call(&e, &tid, "avatar_say", json!({ "text": "Hello." })).await.unwrap_err().message, SWITCHED_OFF);
}

#[tokio::test]
async fn a_hook_call_over_its_size_or_its_rate_is_refused_before_it_is_read() {
    let e = env().await;
    let tid = command_session(&e);
    let own = token(&e, &tid);
    let mut events = Events::of(&e);

    // One byte over the size of a design: refused before the handler, whoever sends it.
    let over = vec![b' '; BODY_LIMIT + 1];
    for bearer in [Some(own.as_str()), None] {
        let sent = request(&e, Method::POST, &hook(&tid, "design"), bearer, JSON, over.clone()).await;
        assert_eq!(sent.status, 413, "{}", sent.text);
    }
    let sent = request(&e, Method::POST, &format!("/api/avatar/{tid}/reply"), Some(&master(&e)), JSON, over.clone()).await;
    assert_eq!(sent.status, 413, "a device route has the same limit");
    // At the size itself the call is read (and here refused for what it holds).
    let sent = request(&e, Method::POST, &hook(&tid, "design"), Some(&own), JSON, vec![b' '; BODY_LIMIT]).await;
    assert_eq!((sent.status, sent.code().as_str()), (400, "bad_request"), "{}", sent.text);

    // The rate counts attempts, refused ones too: that was the first of six.
    for attempt in 2..=DESIGN_PER_MINUTE {
        let sent = request(&e, Method::POST, &hook(&tid, "design"), Some(&own), JSON, "{not json").await;
        assert_eq!(sent.status, 400, "attempt {attempt}: {}", sent.text);
    }
    // Past it a call is refused for its rate, whatever its body: nothing of it was parsed.
    for body in ["{not json".to_string(), figure().to_string()] {
        let sent = request(&e, Method::POST, &hook(&tid, "design"), Some(&own), JSON, body).await;
        assert_eq!((sent.status, sent.code().as_str()), (429, "rate_limited"), "{}", sent.text);
        assert!(sent.message().contains("at most 6 design calls a minute"), "{}", sent.message());
    }
    for attempt in 1..=SAY_PER_MINUTE {
        let sent = request(&e, Method::POST, &hook(&tid, "say"), Some(&own), Some("text/plain"), vec![0xff]).await;
        assert_eq!((sent.status, sent.message().as_str()), (400, "the line is not UTF-8 text"), "attempt {attempt}");
    }
    let sent = request(&e, Method::POST, &hook(&tid, "say"), Some(&own), JSON, "{\"text\":\"One line too many.\"}").await;
    assert_eq!((sent.status, sent.code().as_str()), (429, "rate_limited"));
    assert!(sent.message().starts_with("Not said: at most 6 lines and questions a minute."), "{}", sent.message());
    // Nothing was drawn, said or announced.
    assert!(events.kinds().is_empty());
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.revision, avatar.designed, avatar.lines.len()), (0, false, 0));
}

// ---------------------------------------------------------------- the device routes

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_signed_in_device_answers_for_the_user() {
    let e = env().await;
    let tid = e.with_avatar();
    let addr = serve(&e).await;
    let device = Device::sign_in(&e, addr).await;
    let master = master(&e);
    let line = e.say(&tid, "The build is green.", now_ms()).unwrap().line;
    let ask = e.asked(&tid, "Deploy now?", &["Yes", "Not yet"], now_ms()).await;
    let posts = [
        (format!("/api/avatar/{tid}/lines/{}/claim", line.id), json!({ "tab": "tab-1" })),
        (format!("/api/avatar/{tid}/lines/{}/heard", line.id), json!({ "outcome": "spoken" })),
        (format!("/api/avatar/{tid}/reply"), json!({ "askId": ask.id, "choice": 0, "via": "click" })),
        (format!("/api/avatar/{tid}/reply"), json!({ "text": "Carry on.", "via": "typed" })),
        (format!("/api/avatar/{tid}/asks/{}/dismiss", ask.id), json!({})),
        (format!("/api/avatar/{tid}/off"), json!({})),
        (format!("/api/avatar/{tid}/reset"), json!({})),
        (format!("/api/avatar/{tid}/on"), json!({})),
    ];
    let mut events = Events::of(&e);
    for (path, body) in &posts {
        // The master token is a file a session could read: through the router and over the wire.
        let (status, v) = e.send(Method::POST, path, &master, Some(body.clone())).await;
        assert_eq!((status, &v["error"]["code"]), (403, &json!("forbidden")), "{path} with the master token");
        let r = device.http.post(format!("{}{path}", device.origin)).bearer_auth(&master).json(body).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 403, "{path} with the master token over the wire");
        // An in-process call is a session's own tool.
        let err = call_api(&e.state, Method::POST, path, Some(body.clone()), &ctx(&e, &tid)).await.unwrap_err();
        assert_eq!((err.status.as_u16(), err.code), (403, "forbidden"), "{path} in-process");
        // A device's cookie alone reaches every local port: without its key it does not act.
        let r = device.http.post(format!("{}{path}", device.origin)).header("cookie", &device.cookie).header("origin", &device.origin).json(body).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 401, "{path} with the cookie alone");
        // And not from another origin.
        let r = device.http.post(format!("{}{path}", device.origin)).header("cookie", &device.cookie).header("origin", "http://evil.example").header(crate::auth::KEY_HEADER, &device.key).json(body).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 403, "{path} from another origin");
    }
    // None of it did anything.
    assert!(events.kinds().is_empty());
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.on, avatar.ask.as_ref().map(|a| a.id.as_str()), avatar.lines.len(), avatar.lines[0].outcome), (true, Some(ask.id.as_str()), 2, None));

    // The two reads answer whoever is signed in or holds the token, in-process callers too.
    let (status, listed) = e.send(Method::GET, "/api/avatar", &master, None).await;
    assert_eq!((status, listed["avatars"].as_array().map(Vec::len)), (200, Some(1)), "{listed}");
    assert_eq!(call_api(&e.state, Method::GET, "/api/avatar", None, &ctx(&e, &tid)).await.unwrap(), listed);
    assert_eq!(device.get("/api/avatar").await, (200, listed.clone()));
    let (status, one) = device.get(&format!("/api/avatar/{tid}")).await;
    assert_eq!((status, &one), (200, &listed["avatars"][0]));
    assert_eq!(e.send(Method::GET, &format!("/api/avatar/{tid}"), &master, None).await, (200, one.clone()));
    let (status, missing) = device.get("/api/avatar/000000000000").await;
    assert_eq!((status, &missing["error"]["code"]), (404, &json!("not_found")));

    // The device does every one of them.
    let pasted = pastes_succeed(&e);
    for (path, body) in &posts {
        let (status, v) = device.post(path, body.clone()).await;
        match (path.rsplit('/').next().unwrap(), status) {
            // The question was answered by the reply before it: there is none to dismiss.
            ("dismiss", 409) => assert_eq!(v["error"]["code"], "not_pending", "{v}"),
            (_, 200) => {}
            other => panic!("{path}: {other:?} {v}"),
        }
    }
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.on, avatar.ask.is_none(), avatar.lines[0].outcome), (true, true, Some(Outcome::Spoken)));
    let prompts = pasted.lock().iter().map(|(_, prompt)| prompt.clone()).collect::<Vec<_>>();
    assert_eq!(prompts[..2], [format!("[via avatar, answer with avatar_say] Answer to question {}: choice 1, \"Yes\"", ask.id), "[via avatar, answer with avatar_say] Carry on.".to_string()]);
    // Turned off and on again by the device, the session is told that it has its avatar again.
    assert!(prompts.len() == 3 && prompts[2].starts_with("<workbench-avatar>[Workbench avatar] "), "{prompts:?}");
    // What a body may not hold is said in Workbench's own form, not the framework's.
    let (status, v) = device.post(&format!("/api/avatar/{tid}/reply"), json!({ "text": "Hello", "via": "shouted" })).await;
    assert_eq!((status, &v["error"]["code"]), (400, &json!("bad_request")), "{v}");
    let (status, v) = device.post(&format!("/api/avatar/{tid}/off"), json!({ "forever": true })).await;
    assert_eq!((status, v["error"]["message"].as_str()), (400, Some("off takes no arguments: send {}")));
    let r = device.http.post(format!("{}/api/avatar/{tid}/lines/{}/claim", device.origin, line.id)).header("cookie", &device.cookie).header("origin", &device.origin).header(crate::auth::KEY_HEADER, &device.key).body("tab-1").send().await.unwrap();
    let (status, v) = answer(r).await;
    assert_eq!((status, &v["error"]["code"]), (400, &json!("bad_request")), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_exchange_over_http() {
    let e = env().await;
    let tid = e.with_avatar();
    let addr = serve(&e).await;
    let device = Device::sign_in(&e, addr).await;
    let session = Wire::of(&e, addr, &tid);
    let pasted = pastes_succeed(&e);
    let of = |rest: &str| format!("/api/avatar/{tid}{rest}");
    let shown = |line: String| {
        let (device, path) = (device.clone(), of(&format!("/lines/{line}/heard")));
        async move { assert_eq!(device.post(&path, json!({ "outcome": "shown" })).await, (200, json!({ "ok": true }))) }
    };

    // The session draws its character; the browser reads the record.
    let (status, designed) = session.post("design", figure()).await;
    assert_eq!(status, 200, "{designed}");
    assert!(designed.as_str().is_some_and(|text| text.starts_with("Saved as revision 1, named Dot.")), "{designed}");
    let (status, listed) = device.get("/api/avatar").await;
    assert_eq!(status, 200);
    assert_eq!(sorted_keys(&listed), ["asked", "avatars", "enabled", "sessions"]);
    assert_eq!((&listed["enabled"], &listed["sessions"], &listed["asked"]), (&json!(true), &json!("asked"), &json!([])));
    let avatar = &listed["avatars"][0];
    // Field for field what the web's `AvatarInfo` declares.
    assert_eq!(sorted_keys(avatar), ["ask", "createdAt", "designed", "held", "lines", "look", "name", "on", "projectId", "revision", "terminalId", "updatedAt", "via", "voice"]);
    assert_eq!((&avatar["terminalId"], &avatar["projectId"], &avatar["on"], &avatar["name"], &avatar["designed"], &avatar["revision"], &avatar["via"]), (&json!(tid), &json!(e.pid), &json!(true), &json!("Dot"), &json!(true), &json!(1), &json!("command")));
    assert_eq!((&avatar["look"]["viewBox"], &avatar["voice"], &avatar["held"], &avatar["ask"], &avatar["lines"]), (&json!([0, 0, 200, 200]), &json!({ "lang": "en-GB", "pitch": 1.1 }), &Value::Null, &Value::Null, &json!([])));
    assert_eq!(sorted_keys(&avatar["look"]), ["animations", "gaze", "mouth", "nodes", "pivots", "roles", "states", "viewBox"]);

    // It says a line. One tab claims it, another is told to wait; the first speaks it and says so.
    let (status, said) = session.post("say", json!({ "text": "The build is green.", "gesture": "hop" })).await;
    assert_eq!(status, 200, "{said}");
    let first = said["lineId"].as_str().unwrap().to_string();
    assert_eq!(said, json!({ "lineId": first, "note": format!("Queued ({first}).") }));
    let (status, claim) = device.post(&of(&format!("/lines/{first}/claim")), json!({ "tab": "tab-A" })).await;
    assert_eq!((status, sorted_keys(&claim), &claim["speak"], &claim["retryMs"]), (200, vec!["leaseMs", "retryMs", "speak"], &json!(true), &json!(0)));
    assert!(claim["leaseMs"].as_i64().unwrap() > 15_000, "{claim}");
    let (_, second_tab) = device.post(&of(&format!("/lines/{first}/claim")), json!({ "tab": "tab-B" })).await;
    assert_eq!((&second_tab["speak"], &second_tab["leaseMs"]), (&json!(false), &json!(0)));
    assert!(second_tab["retryMs"].as_i64().unwrap() > 0, "{second_tab}");
    assert_eq!(device.post(&of(&format!("/lines/{first}/claim")), json!({ "tab": "tab-B", "release": true })).await, (200, json!({ "speak": false, "leaseMs": 0, "retryMs": 0 })));
    assert_eq!(device.post(&of(&format!("/lines/{first}/heard")), json!({ "outcome": "spoken", "voice": "Test Voice" })).await, (200, json!({ "ok": true })));
    let (_, gone) = device.post(&of("/lines/L0000-99/heard"), json!({ "outcome": "shown" })).await;
    assert_eq!(gone["error"]["code"], "not_found");
    // Its next line tells the session what became of the first.
    let (_, said) = session.post("say", json!({ "text": "Shall I go on?" })).await;
    let second = said["lineId"].as_str().unwrap().to_string();
    assert_eq!(said["note"], format!("Queued ({second}). Earlier: {first} spoken aloud by Test Voice."));
    shown(second.clone()).await;
    // The transcript as the browser reads it: field for field the web's `Line`.
    let (_, record) = device.get(&of("")).await;
    let line = &record["lines"][0];
    assert_eq!(line, &json!({ "id": first, "at": line["at"], "from": "agent", "text": "The build is green.", "gesture": "hop", "outcome": "spoken", "voice": "Test Voice" }));
    assert_eq!(record["lines"][1], json!({ "id": second, "at": record["lines"][1]["at"], "from": "agent", "text": "Shall I go on?", "outcome": "shown" }));

    // A question, answered by a click while the session's call waits for it.
    let asking = tokio::spawn({
        let session = session.clone();
        async move { session.post("ask", json!({ "text": "Deploy now?", "choices": ["Yes", "Not yet"], "waitSeconds": 30 })).await }
    });
    let ask = waited_for(&e, &tid).await;
    let (_, record) = device.get(&of("")).await;
    assert_eq!(record["ask"], json!({ "id": ask.id, "lineId": ask.line_id, "text": "Deploy now?", "choices": ["Yes", "Not yet"], "freeText": true, "askedAt": record["ask"]["askedAt"], "waiting": true }));
    shown(ask.line_id.clone()).await;
    assert_eq!(device.post(&of("/reply"), json!({ "askId": ask.id, "choice": 1, "via": "click" })).await, (200, json!({ "delivered": "answer" })));
    let (status, answered) = tokio::time::timeout(Duration::from_secs(30), asking).await.expect("the call returned").unwrap();
    assert_eq!((status, answered), (200, json!({ "answered": true, "text": "Not yet", "choice": 1, "via": "click" })));
    assert!(pasted.lock().is_empty(), "the call had the answer: nothing was pasted");

    // A question whose call stopped waiting: the typed answer goes into the session as a prompt.
    let (_, unanswered) = session.post("ask", json!({ "text": "Which database should the tests use?", "waitSeconds": 0 })).await;
    assert_eq!((&unanswered["answered"], &unanswered["reason"]), (&json!(false), &json!("timeout")), "{unanswered}");
    let open = unanswered["askId"].as_str().unwrap().to_string();
    shown(e.avatar(&tid).ask.unwrap().line_id).await;
    // An answer to a question that is not the open one is nobody's: nothing is pasted.
    let (status, stale) = device.post(&of("/reply"), json!({ "askId": ask.id, "text": "Yes", "via": "typed" })).await;
    assert_eq!((status, &stale["error"]["code"]), (409, &json!("not_pending")), "{stale}");
    assert_eq!(device.post(&of("/reply"), json!({ "askId": open, "text": "Postgres,\nplease", "via": "typed" })).await, (200, json!({ "delivered": "prompt" })));
    // A message without a question, dictated.
    assert_eq!(device.post(&of("/reply"), json!({ "text": "Stop and show me the diff", "via": "voice" })).await, (200, json!({ "delivered": "prompt" })));
    assert_eq!(
        pasted.lock().clone(),
        [
            (tid.clone(), format!("[via avatar, answer with your avatar say command] Answer to question {open}: Postgres, please")),
            (tid.clone(), "[via avatar, dictated, answer with your avatar say command] Stop and show me the diff".to_string()),
        ]
    );
    let (_, record) = device.get(&of("")).await;
    assert_eq!(record["ask"], Value::Null);
    let user: Vec<&Value> = record["lines"].as_array().unwrap().iter().filter(|l| l["from"] == "user").collect();
    assert_eq!(user.len(), 3);
    assert_eq!(user[1], &json!({ "id": user[1]["id"], "at": user[1]["at"], "from": "user", "text": "Postgres, please", "askId": open, "via": "typed" }));

    // A question the user dismisses: the call that waits learns so.
    let asking = tokio::spawn({
        let session = session.clone();
        async move { session.post("ask", json!({ "text": "Shall I push?", "freeText": false, "choices": ["Push"], "waitSeconds": 30 })).await }
    });
    let ask = waited_for(&e, &tid).await;
    assert_eq!(device.post(&of(&format!("/asks/{}/dismiss", ask.id)), json!({})).await, (200, json!({ "ok": true })));
    let (status, dismissed) = tokio::time::timeout(Duration::from_secs(30), asking).await.expect("the call returned").unwrap();
    assert_eq!((status, &dismissed["answered"], &dismissed["reason"], &dismissed["askId"]), (200, &json!(false), &json!("dismissed"), &json!(ask.id)));
    let (status, again) = device.post(&of(&format!("/asks/{}/dismiss", ask.id)), json!({})).await;
    assert_eq!((status, &again["error"]["code"]), (409, &json!("not_pending")));

    // A gesture, then the user turns the avatar off: the session is told, and nothing is lost.
    assert_eq!(session.post("act", json!({ "play": "hop" })).await, (200, json!({ "ok": true, "note": "Playing hop." })));
    let (status, off) = device.post(&of("/off"), json!({})).await;
    assert_eq!((status, &off["on"], &off["designed"]), (200, &json!(false), &json!(true)));
    let (status, refused) = session.post("say", json!({ "text": "Anyone there?" })).await;
    assert_eq!((status, refused["error"]["message"].as_str()), (409, Some(TURNED_OFF)));
    // On again: the session read that its avatar was turned off, so it is told that it is back.
    let (status, on) = device.post(&of("/on"), json!({})).await;
    assert_eq!((status, &on["on"], on.get("briefed")), (200, &json!(true), Some(&json!(true))));
    // Back to the default look: the name and the voice stay.
    let (status, reset) = device.post(&of("/reset"), json!({})).await;
    assert_eq!((status, &reset["look"], &reset["designed"], &reset["name"], &reset["revision"]), (200, &Value::Null, &json!(false), &json!("Dot"), &json!(2)));
    // Two replies went into the session, and what it was told when its avatar was turned on again:
    // it had a look of its own then, so it was told to keep it.
    let prompts = pasted.lock().iter().map(|(_, prompt)| prompt.clone()).collect::<Vec<_>>();
    assert!(prompts.len() == 3 && prompts[2].starts_with("<workbench-avatar>[Workbench avatar] This session still has its live avatar"), "{prompts:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_is_given_an_avatar_is_told_once() {
    let e = env().await;
    let addr = serve(&e).await;
    let device = Device::sign_in(&e, addr).await;
    let turn_on = |tid: &str| {
        let (device, path) = (device.clone(), format!("/api/avatar/{tid}/on"));
        tokio::spawn(async move { device.post(&path, json!({})).await })
    };

    // The brief goes through the terminals slice's ask path, which refuses a session it cannot
    // paste into (this stand-in has no process): the avatar is on, the session untold.
    let unreachable = e.agent(json!({}));
    let (status, answer) = turn_on(&unreachable).await.unwrap();
    assert_eq!((status, &answer["on"], &answer["briefed"]), (200, &json!(true), &json!(false)), "{answer}");
    assert!(e.avatar(&unreachable).on);

    // From here on the test is the session a brief is pasted into.
    let mut pastes = e.pastes();

    // A running session that was started without the option.
    let tid = e.agent(json!({}));
    let given = brief::session_brief(&Brief { variant: BriefVariant::Given, via: Via::Tools, in_container: false, shell: Dialect::HOST, whole: true });
    assert!(given.starts_with("<workbench-avatar>[Workbench avatar] The user just gave this session a live avatar: ") && given.ends_with("then go on with what you were doing.</workbench-avatar>"), "{given}");

    // It shows a dialog: the brief is refused there, the avatar is on all the same, and the
    // answer says that the session does not know yet.
    let on = turn_on(&tid);
    let paste = tokio::time::timeout(Duration::from_secs(30), pastes.recv()).await.expect("the brief is pasted").unwrap();
    assert_eq!((paste.tid.as_str(), paste.prompt.as_str()), (tid.as_str(), given.as_str()));
    let _ = paste.done.send(Err(ApiError::conflict("that session is showing a prompt (permission, question or trust); answer it first")));
    let (status, answer) = on.await.unwrap();
    assert_eq!((status, &answer["terminalId"], &answer["on"], &answer["designed"], &answer["briefed"]), (200, &json!(tid), &json!(true), &json!(false), &json!(false)), "{answer}");
    assert!(e.avatar(&tid).on);

    // Two windows turn it on at once: the session is told by one of them, once.
    let (first, second) = (turn_on(&tid), turn_on(&tid));
    let paste = tokio::time::timeout(Duration::from_secs(30), pastes.recv()).await.expect("the brief is pasted").unwrap();
    assert_eq!(paste.prompt, given);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(pastes.try_recv().is_err(), "the second request pastes nothing while the first tells the session");
    let _ = paste.done.send(Ok(()));
    let answers = [first.await.unwrap(), second.await.unwrap()];
    assert!(answers.iter().all(|(status, v)| *status == 200 && v["on"] == true), "{answers:?}");
    let mut told: Vec<Option<bool>> = answers.iter().map(|(_, v)| v.get("briefed").and_then(Value::as_bool)).collect();
    told.sort();
    assert_eq!(told, [None, Some(true)]);

    // Whatever the paste fails with, the avatar stays on and the session counts as untold.
    let troubled = e.agent(json!({}));
    let on = turn_on(&troubled);
    let paste = tokio::time::timeout(Duration::from_secs(30), pastes.recv()).await.expect("the brief is pasted").unwrap();
    let _ = paste.done.send(Err(ApiError::internal("the terminal could not be written to")));
    let (status, answer) = on.await.unwrap();
    assert_eq!((status, &answer["on"], &answer["briefed"]), (200, &json!(true), &json!(false)), "{answer}");

    // Off and on again: the session read that its avatar was turned off, so it is told that it
    // has it again (as when it was given, while it has no look of its own).
    assert_eq!(device.post(&format!("/api/avatar/{tid}/off"), json!({})).await.0, 200);
    let on = turn_on(&tid);
    let paste = tokio::time::timeout(Duration::from_secs(30), pastes.recv()).await.expect("the brief is pasted").unwrap();
    assert_eq!(paste.prompt, given);
    let _ = paste.done.send(Ok(()));
    let (status, answer) = on.await.unwrap();
    assert_eq!((status, &answer["on"], answer.get("briefed")), (200, &json!(true), Some(&json!(true))));
    // On while it is on and told pastes nothing.
    let (status, answer) = turn_on(&tid).await.unwrap();
    assert_eq!((status, &answer["on"], answer.get("briefed")), (200, &json!(true), None));
    // A session that was started with the option was told at its start.
    let started = e.with_avatar();
    let (status, answer) = turn_on(&started).await.unwrap();
    assert_eq!((status, answer.get("briefed")), (200, None));
    // One that is not running is not told now.
    let ended = e.agent(json!({}));
    e.change(&ended, |info| info.status = TerminalStatus::Exited);
    let (status, answer) = turn_on(&ended).await.unwrap();
    assert_eq!((status, &answer["on"], answer.get("briefed")), (200, &json!(true), None));
    assert!(pastes.try_recv().is_err());

    // A session without the tools, in a dev container, is given its curl form.
    let boxed = e.agent(json!({ "inContainer": true }));
    e.change(&boxed, |info| info.agent.as_mut().unwrap().provider = ProviderKind::Kimi);
    let on = turn_on(&boxed);
    let paste = tokio::time::timeout(Duration::from_secs(30), pastes.recv()).await.expect("the brief is pasted").unwrap();
    assert_eq!(paste.prompt, brief::session_brief(&Brief { variant: BriefVariant::Given, via: Via::Command, in_container: true, shell: Dialect::HOST, whole: true }));
    assert!(paste.prompt.contains("You drive it with one command.") && paste.prompt.contains("curl -sS --fail-with-body -H @- \"$WORKBENCH_URL/api/hooks/avatar/$WORKBENCH_TERMINAL_ID/guide\""), "{}", paste.prompt);
    let _ = paste.done.send(Ok(()));
    let (status, answer) = on.await.unwrap();
    assert_eq!((status, &answer["via"], &answer["briefed"]), (200, &json!("command"), &json!(true)));

    // Only an agent session has an avatar, and only one that exists.
    let (status, answer) = device.post("/api/avatar/000000000000/on", json!({})).await;
    assert_eq!((status, &answer["error"]["code"]), (404, &json!("not_found")));
}

// ---------------------------------------------------------------- the curl forms

/// Whether this computer has a curl that knows the options the forms use.
#[cfg(unix)]
fn has_curl() -> bool {
    use std::process::{Command, Stdio};
    Command::new("curl").args(["--fail-with-body", "--version"]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

/// The commands a dev-container session is given, run as they are written: the one of its brief
/// and those of its guide, in a shell, against a listener of this test. The shell has nothing of
/// the environment this test runs in but `PATH`: the three variables are the test session's.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_curl_forms_a_dev_container_session_is_given_work_as_they_are_written() {
    if !has_curl() {
        eprintln!("skipped: no curl with --fail-with-body on this computer");
        return;
    }
    let e = env().await;
    let tid = e.agent(json!({ "avatar": true, "inContainer": true }));
    e.change(&tid, |info| info.agent.as_mut().unwrap().provider = ProviderKind::Kimi);
    e.event("terminal.created", &tid, now_ms());
    let addr = serve(&e).await;
    let own = token(&e, &tid);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("look.json"), figure().to_string()).unwrap();
    std::fs::write(dir.path().join("ask.json"), "{\"text\": \"Deploy now?\", \"choices\": [\"Yes\", \"Not yet\"], \"waitSeconds\": 0}").unwrap();
    std::fs::write(dir.path().join("act.json"), "{\"play\": \"hop\"}").unwrap();
    let sh = |command: String| {
        let (cwd, url, tid, own) = (dir.path().to_path_buf(), format!("http://{addr}"), tid.clone(), own.clone());
        async move {
            tokio::task::spawn_blocking(move || {
                let cwd_tmp = cwd.clone();
                let out = std::process::Command::new("sh")
                    .args(["-c", &command])
                    .current_dir(cwd)
                    .env_clear()
                    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                    .env("WORKBENCH_URL", url)
                    .env("WORKBENCH_TERMINAL_ID", tid)
                    .env("WORKBENCH_AGENT_TOKEN", own)
                    .env("TMPDIR", &cwd_tmp)
                    .output()
                    .expect("sh runs");
                (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
            })
            .await
            .unwrap()
        }
    };

    // The command of the brief: it prints the guide and the status.
    let first = brief::session_brief(&Brief { variant: BriefVariant::First, via: Via::Command, in_container: true, shell: Dialect::Posix, whole: false });
    let command = first.split('`').nth(1).expect("the command stands between backticks").to_string();
    assert!(command.starts_with("printf ") && command.ends_with("/guide\""), "{command}");
    let (code, guide, err) = sh(command).await;
    assert_eq!((code, err.as_str()), (Some(0), ""), "{guide}");
    assert!(guide.starts_with("# Your avatar\n") && guide.contains("## This session's avatar now") && guide.contains("\"designed\": false"), "{guide}");

    // The forms of the guide, written out as it says: C is the call, U the session's routes.
    let call = guide.lines().find(|line| line.starts_with("    printf ")).expect("what C stands for").trim().to_string();
    let routes = guide.split("U for `").nth(1).and_then(|rest| rest.split('`').next()).expect("what U stands for").to_string();
    assert_eq!(routes, "$WORKBENCH_URL/api/hooks/avatar/$WORKBENCH_TERMINAL_ID");
    let forms: Vec<String> = guide.lines().filter(|line| line.starts_with("    C ")).map(|line| line.trim().replacen("C ", &format!("{call} "), 1).replace("\"U/", &format!("\"{routes}/"))).collect();
    assert_eq!(forms.len(), 6, "{forms:?}");
    std::fs::write(dir.path().join("picture.json"), "{\"frames\": \"hop\", \"count\": 3}").unwrap();
    let mut outputs = vec![guide.clone()];
    // The picture goes into its file: the call itself prints nothing.
    for (form, expected) in forms.iter().zip(["Saved as revision 1, named Dot.", "{\"lineId\":\"L", "{\"answered\":false,\"reason\":\"timeout\",\"askId\":\"Q", "{\"ok\":true,\"note\":\"Playing hop.\"}", "{\"note\":\"You have a live avatar: ", ""]) {
        let (code, out, err) = sh(form.clone()).await;
        assert_eq!((code, err.as_str()), (Some(0), ""), "{form}\n{out}");
        assert!(out.starts_with(expected), "{form}\nprinted: {out}");
        outputs.push(out);
    }
    // They did what they say.
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.designed, avatar.name.as_str(), avatar.via), (true, "Dot", Via::Command));
    assert_eq!(avatar.lines.iter().map(|l| (l.text.as_str(), l.gesture.as_deref())).collect::<Vec<_>>(), [("One short line.", Some("hop")), ("Deploy now?", None)]);
    assert!(outputs[5].contains("\"design\":{"), "status with look=1 brings the stored design: {}", outputs[5]);
    let picture = std::fs::read(dir.path().join(format!("avatar-{tid}.png"))).expect("the picture was written into TMPDIR, under the session's own name");
    assert!(picture.starts_with(b"\x89PNG\r\n\x1a\n") && outputs[6].is_empty(), "{} bytes", picture.len());

    // A refusal: curl ends with a failure and still prints what Workbench said.
    std::fs::write(dir.path().join("act.json"), "{\"play\": \"wink\"}").unwrap();
    let (code, out, _) = sh(forms[3].clone()).await;
    assert_eq!(code, Some(22), "{out}");
    assert!(out.starts_with("{\"error\":{\"code\":\"bad_request\",\"message\":\"No animation wink."), "{out}");
    outputs.push(out);
    // Nothing the commands printed shows the token, and none of them has it in its own text.
    assert!(outputs.iter().all(|out| !out.contains(&own)) && forms.iter().all(|form| !form.contains(&own) && form.contains("-H @-")));
}

// ---------------------------------------------------------------- pictures

/// Width and height of a PNG, from its header.
fn png_size(png: &[u8]) -> (u32, u32) {
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG: {} bytes", png.len());
    (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_is_shown_what_it_drew() {
    let e = env().await;
    let tid = e.with_avatar();
    let out = |name: &'static str, args: Value| {
        let (state, ctx) = (e.state.clone(), ctx(&e, &tid));
        async move { (tool(name).handler)(state, ctx, args).await }
    };
    let pictured = |output: ApiResult<ToolOutput>| match output {
        Ok(ToolOutput::Images { text, png }) => (text, png),
        Ok(_) => panic!("an answer without a picture"),
        Err(e) => panic!("refused: {}", e.message),
    };
    let refused = |output: ApiResult<ToolOutput>| output.err().expect("refused").message;
    // A status whose picture could not be drawn: the reason first, then the status all the same,
    // in the one line it has beside a picture.
    let unpictured = |output: ApiResult<ToolOutput>| match output {
        Ok(ToolOutput::Text(text)) => {
            let (why, status) = text.split_once('\n').expect("the reason, then the status");
            assert!(!status.is_empty() && !status.contains('\n') && !status.starts_with('{'), "{status}");
            why.strip_prefix("No picture: ").unwrap_or_else(|| panic!("no reason: {why}")).to_string()
        }
        Ok(_) => panic!("a picture"),
        Err(e) => panic!("refused: {}", e.message),
    };

    // Nothing of its own to see yet: a design call that draws nothing brings no picture, and a
    // status that asks for one says what to do and is a status all the same.
    assert!(matches!(out("avatar_design", json!({ "name": "Dot" })).await, Ok(ToolOutput::Text(_))));
    assert_eq!(unpictured(out("avatar_status", json!({ "picture": true })).await), session::NO_LOOK_TO_DRAW);

    // A design answer carries the look at rest, and says what the picture shows.
    let (text, png) = pictured(out("avatar_design", figure()).await);
    assert!(text.starts_with("Saved as revision ") && text.contains("\nPicture: your avatar at rest, on the light panel and on the dark one"), "{text}");
    assert_eq!(png.len(), 1);
    let sheet = png_size(&png[0]);
    assert!(sheet.0 > 2 * preview::SHEET_FIGURE_PX && sheet.1 > preview::SHEET_FIGURE_PX, "{sheet:?}");
    // A call that leaves the look as it is (a name) shows it again; nothing was drawn twice for it.
    let (text, again) = pictured(out("avatar_design", json!({ "name": "Dotty" })).await);
    assert!(text.contains("named Dotty") && again[0] == png[0], "{text}");

    // The status shows it again, in a pose, close up, or one of its animations as a row of
    // moments: what the picture shows comes first, then the status.
    let (text, png) = pictured(out("avatar_status", json!({ "picture": true })).await);
    assert!(text.starts_with("Picture: your avatar at rest"), "{text}");
    // Beside a picture the status is one line: a session looks many times while it corrects.
    let last = text.lines().last().unwrap();
    assert!(last.starts_with("Dotty, revision ") && last.ends_with(session::BRIEF_MORE) && !text.contains("\"limits\""), "{text}");
    assert_eq!(png_size(&png[0]), sheet);
    // A call that asks for the design too gets the whole status with its picture.
    let (text, _) = pictured(out("avatar_status", json!({ "picture": true, "look": true })).await);
    assert!(text.starts_with("Picture: your avatar at rest") && text.contains("\"designed\":true") && text.contains("\"limits\""), "{text}");
    // What stands open is in the line: a gesture that stays, a question.
    assert!(matches!(out("avatar_act", json!({ "play": "hop", "mode": "hold" })).await, Ok(_)));
    let (text, _) = pictured(out("avatar_status", json!({ "picture": true })).await);
    assert!(text.lines().last().unwrap().contains(" hop is held."), "{text}");
    assert!(matches!(out("avatar_act", json!({ "stop": "all" })).await, Ok(_)));
    let (text, posed) = pictured(out("avatar_status", json!({ "picture": true, "pose": { "animation": "hop", "at": 0.5, "mouth": 1, "lookX": 1 } })).await);
    assert!(text.starts_with("Picture: your avatar in that pose"), "{text}");
    assert!(png_size(&posed[0]) == sheet && posed[0] != png[0], "a pose is another picture");
    let (text, close) = pictured(out("avatar_status", json!({ "pose": { "zoom": "eyes" } })).await);
    assert!(text.starts_with("Picture: a closer look at eyes in that pose"), "{text}");
    assert!(png_size(&close[0]) == sheet && close[0] != png[0], "a closer look is another picture");
    let (text, strip) = pictured(out("avatar_status", json!({ "frames": "hop", "count": 3 })).await);
    assert!(text.starts_with("Picture: 3 moments of hop, from its start on the left to its end on the right. A track that has played holds"), "{text}");
    assert_eq!(png_size(&strip[0]).1, preview::STRIP_FRAME_PX);
    assert!(png_size(&strip[0]).0 >= 3 * preview::STRIP_FRAME_PX);
    let (_, six) = pictured(out("avatar_status", json!({ "frames": "hop" })).await);
    assert!(png_size(&six[0]).0 >= 6 * preview::STRIP_FRAME_PX);
    // A strip takes the inputs of a pose: a walk that shows only turned is seen turned.
    let (text, _) = pictured(out("avatar_status", json!({ "frames": "hop", "count": 3, "pose": { "mouth": 1, "lookX": -1 } })).await);
    assert!(text.contains("with those inputs held"), "{text}");
    // Workbench's built-in motion is drawn by name as well, made from the roles, and the caption
    // says it is Workbench's: a session sees how the built-in wave turns its arm.
    let (text, waved) = pictured(out("avatar_status", json!({ "frames": "wave" })).await);
    assert!(text.starts_with("Picture: 6 moments of wave, Workbench's built-in motion made from your roles, from its start on the left"), "{text}");
    assert!(png_size(&waved[0]).0 >= 6 * preview::STRIP_FRAME_PX && waved[0] != six[0], "another animation");
    let (text, nodded) = pictured(out("avatar_status", json!({ "pose": { "animation": "nod", "at": 0.25 } })).await);
    assert!(text.starts_with("Picture: your avatar in that pose, inside Workbench's built-in nod at 0.25, on the light panel"), "{text}");
    assert!(png_size(&nodded[0]) == sheet && nodded[0] != png[0], "a nod is another picture");
    // Without at, the moment where it has moved furthest from rest, and the caption names it.
    let (text, _) = pictured(out("avatar_status", json!({ "pose": { "animation": "nod", "zoom": "eyes" } })).await);
    assert!(text.starts_with("Picture: a closer look at eyes in that pose, inside Workbench's built-in nod at 0.25, where it has moved furthest, on the light"), "{text}");
    // Its own blink replaced the built-in one: that is the one drawn, and the caption says nothing of Workbench.
    let (text, _) = pictured(out("avatar_status", json!({ "frames": "blink", "count": 2 })).await);
    assert!(text.starts_with("Picture: 2 moments of blink, from its start"), "{text}");
    let (text, _) = pictured(out("avatar_status", json!({ "pose": { "animation": "hop" } })).await);
    assert!(text.starts_with("Picture: your avatar in that pose, inside hop at 0.5, where it has moved furthest, on the light panel"), "{text}");

    // A picture that cannot be drawn is said in a sentence, and the status comes all the same.
    for (args, words) in [
        (json!({ "frames": "think" }), "think is Workbench's built-in motion and needs the head role, or the pupils role and a gaze above 0, which this look does not name"),
        (json!({ "frames": "wink" }), "no animation \"wink\" in this look: its own animations are blink, fade, hop, spin; Workbench's built-in ones for its roles are breathe, talk, listen, alert, doze, wave, cheer, nod, shake"),
        (json!({ "pose": { "zoom": "nose" } }), "zoom names \"nose\""),
        (json!({ "frames": "hop", "pose": { "zoom": "nose" } }), "zoom names \"nose\""),
    ] {
        let said = unpictured(out("avatar_status", args.clone()).await);
        assert!(said.contains(words), "{args}: {said}");
    }
    // What is wrong with the call itself refuses it.
    for (args, words) in [
        (json!({ "frames": "hop", "count": 9 }), "count is a whole number from 2 to 8"),
        (json!({ "frames": "hop", "pose": { "animation": "hop" } }), "with frames, a pose holds inputs and zoom only"),
        (json!({ "frames": "hop", "pose": { "at": 0.5 } }), "with frames, a pose holds inputs and zoom only"),
        (json!({ "frames": "hop", "pose": { "zoom": 3 } }), "zoom in a pose is the id of the part to look at closely"),
        (json!({ "count": 3 }), "name the animation with frames"),
        (json!({ "pose": { "turn": 5 } }), "turn in a pose is a number from -1 to 1"),
        (json!({ "pose": { "spin": 1 } }), "pose has no spin"),
        (json!({ "picture": "yes" }), "picture is a string"),
    ] {
        let said = refused(out("avatar_status", args.clone()).await);
        assert!(said.contains(words), "{args}: {said}");
    }
    // A status without a picture is text, as before.
    assert!(matches!(out("avatar_status", json!({})).await, Ok(ToolOutput::Text(_))));

    // A look that would cost too much to draw is not stored: the browser would pay for it on
    // every frame. (The first asks the renderer for a layer larger than memory.)
    let before = e.avatar(&tid).revision;
    let heavy = json!({ "svg": "<svg viewBox='0 0 200 200'><defs><filter id='f' filterUnits='userSpaceOnUse' x='-400' y='-400' width='800' height='800'><feOffset dx='1' dy='1'/></filter></defs><g id='g' transform='translate(100 100) scale(1000 0.001) rotate(45)' filter='url(#f)'><circle cx='0' cy='0' r='0.01' fill='#3366cc'/></g></svg>" });
    let said = refused(out("avatar_design", heavy).await);
    assert!(said.starts_with("a filter sits on a part that is stretched or skewed too far to draw") && said.ends_with("\nNothing was changed."), "{said}");
    let many = json!({ "svg": format!("<svg viewBox='0 0 200 200'><defs><filter id='f' filterUnits='userSpaceOnUse' x='-200' y='-200' width='600' height='600'><feTurbulence baseFrequency='0.05' numOctaves='4'/><feMorphology operator='dilate' radius='20'/><feGaussianBlur stdDeviation='50'/></filter></defs>{}</svg>", "<circle cx='100' cy='100' r='40' fill='#3366cc' filter='url(#f)'/>".repeat(8)) });
    let said = refused(out("avatar_design", many).await);
    assert!(said.starts_with("this look's filters cost too much to draw (about ") && said.ends_with("\nNothing was changed."), "{said}");
    assert_eq!(e.avatar(&tid).revision, before);
    // What a filter costs is said beside the picture, so a session can tell before it is too much.
    let soft = json!({ "svg": "<svg viewBox='0 0 200 200'><defs><filter id='f'><feDropShadow dx='2' dy='3' stdDeviation='3'/></filter></defs><g id='body' filter='url(#f)'><circle id='face' cx='100' cy='100' r='60' fill='#3366cc'/></g></svg>" });
    let (text, _) = pictured(out("avatar_design", soft).await);
    assert!(text.contains("\nDrawing cost: about ") && text.contains(" of 60 for filters and "), "{text}");

    // An avatar the user turned off is not drawn for its session either.
    turn_off(&e.state, &tid, now_ms()).unwrap();
    assert_eq!(unpictured(out("avatar_status", json!({ "picture": true })).await), TURNED_OFF);
}

/// The caption says what the picture shows: one round of a loop, a close-up of each moment, the
/// moment of a pose and when that moment is where the animation rests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_caption_says_what_the_picture_shows() {
    let e = env().await;
    let tid = e.with_avatar();
    call(&e, &tid, "avatar_design", figure()).await.unwrap();
    let caption = |args: Value| {
        let (state, ctx) = (e.state.clone(), ctx(&e, &tid));
        async move {
            match (tool("avatar_status").handler)(state, ctx, args).await {
                Ok(ToolOutput::Images { text, .. }) => text,
                other => panic!("no picture: {:?}", other.map(|_| ()).map_err(|e| e.message)),
            }
        }
    };
    // A loop's moments are one round, evenly spaced: the last is not its end.
    let text = caption(json!({ "frames": "spin" })).await;
    assert!(text.starts_with("Picture: 6 moments of one round of spin, evenly spaced; after the last comes the first again.\n"), "{text}");
    let text = caption(json!({ "frames": "spin", "count": 4, "pose": { "lookX": 1 } })).await;
    assert!(text.starts_with("Picture: 4 moments of one round of spin, evenly spaced, with those inputs held; after the last comes the first again.\n"), "{text}");
    // A one-shot runs from its start to its end; with a zoom each moment is the close-up.
    let text = caption(json!({ "frames": "nod", "pose": { "zoom": "eyes" } })).await;
    assert!(
        text.starts_with("Picture: 6 moments of nod, Workbench's built-in motion made from your roles, close up on eyes, from its start on the left to its end on the right. A track that has played holds its last keyframe to the end, as in a gesture.\n"),
        "{text}"
    );
    // A moment given where the animation rests is said to be one.
    let text = caption(json!({ "pose": { "animation": "nod", "at": 0.5 } })).await;
    assert!(text.starts_with("Picture: your avatar in that pose, inside Workbench's built-in nod at 0.5, on the light panel") && text.lines().next().unwrap().ends_with(" At 0.5 nod is where it rests: give another at."), "{text}");
    let text = caption(json!({ "pose": { "animation": "nod" } })).await;
    assert!(text.contains("nod at 0.25, where it has moved furthest,") && !text.contains("where it rests"), "{text}");
}

/// What drawing costs is said of the figure in its pose, in the large view: the design answer
/// says it whole, a status picture only where the pose costs other than the look at rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cost_beside_a_picture_is_said_where_the_pose_changes_it() {
    // The sentence is true to its units and short enough to read every time.
    let most = session::cost_line(preview::Spent { filters: 60.0, layers: 60.0 }).unwrap();
    assert!(most.len() <= 240 && most.contains("60 of 60 for filters and 60 of 60 for layers, both in areas of the viewBox"), "{} {most}", most.len());
    assert_eq!(session::cost_line(preview::Spent { filters: 0.5, layers: 0.9 }), None);

    let e = env().await;
    let tid = e.with_avatar();
    let shadowed = json!({
        "name": "Puff",
        "svg": "<svg viewBox='0 0 200 200'><defs><filter id='f'><feDropShadow dx='2' dy='3' stdDeviation='3'/></filter></defs><g id='body' filter='url(#f)'><circle id='face' cx='100' cy='100' r='40' fill='#3366cc'/></g></svg>",
        "pivots": { "body": [100, 100] },
        "animations": { "grow": { "duration": 1, "tracks": [{ "target": "body", "prop": "scale", "values": [1, 2] }] } }
    });
    let text = call(&e, &tid, "avatar_design", shadowed).await.unwrap();
    let whole = text.lines().find(|line| line.starts_with("Drawing cost: about ")).unwrap_or_else(|| panic!("{text}"));
    assert!(whole.len() <= 240, "{whole}");
    let second = |text: &str| text.lines().nth(1).unwrap_or_default().to_string();
    // At rest, and close up on a part at rest: the design answer said it already.
    for args in [json!({ "picture": true }), json!({ "pose": { "zoom": "face" } })] {
        let text = call(&e, &tid, "avatar_status", args.clone()).await.unwrap();
        assert!(second(&text).starts_with("Puff, revision "), "{args}: {text}");
    }
    // Grown, the shadow costs more: both, in one line, the same with a close-up.
    let grown = call(&e, &tid, "avatar_status", json!({ "pose": { "animation": "grow", "at": 1 } })).await.unwrap();
    let line = second(&grown);
    assert!(line.starts_with("In this pose drawing costs about ") && line.contains(" of 60 for filters and ") && line.contains("; at rest ") && line.ends_with('.'), "{grown}");
    let close = call(&e, &tid, "avatar_status", json!({ "pose": { "animation": "grow", "at": 1, "zoom": "face" } })).await.unwrap();
    assert_eq!(second(&close), line, "the zoom changes nothing of what the pose costs");
    let strip = call(&e, &tid, "avatar_status", json!({ "frames": "grow", "count": 3 })).await.unwrap();
    assert_eq!(second(&strip), line.replace("In this pose", "In its costliest moment"), "{strip}");

    // The picture route says the same in its header: the caption, then the cost.
    let addr = serve(&e).await;
    let own = token(&e, &tid);
    let posted = reqwest::Client::new().post(format!("http://{addr}/api/hooks/avatar/{tid}/picture")).bearer_auth(&own).body("{\"pose\": {\"animation\": \"grow\", \"at\": 1}}").send().await.unwrap();
    assert_eq!(posted.status().as_u16(), 200);
    let header = posted.headers()["x-workbench-caption"].to_str().unwrap().to_string();
    let said = percent_encoding::percent_decode_str(&header).decode_utf8().unwrap().into_owned();
    assert_eq!(said, format!("{} {line}", grown.lines().next().unwrap()));
}

/// Where sessions take an avatar themselves, one that has none yet is told to draw, not that it
/// has no avatar; where the user gives them, it is told that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_picture_before_the_first_call_says_what_to_do() {
    let e = super::tests::env_with(|config| config.avatar.sessions = Some(crate::avatar::Sessions::Any)).await;
    let tid = e.agent(json!({}));
    let asked = call(&e, &tid, "avatar_status", json!({ "guide": true, "picture": true })).await.unwrap();
    assert!(asked.starts_with(&format!("No picture: {}\n# Your avatar", session::NO_LOOK_TO_DRAW)), "{}", &asked[..asked.len().min(300)]);
    let e = env().await;
    let tid = e.agent(json!({}));
    let asked = call(&e, &tid, "avatar_status", json!({ "picture": true })).await.unwrap();
    assert!(asked.starts_with(&format!("No picture: {NO_AVATAR}\n")), "{asked}");
}

/// A sentence of the renderer quotes what the session wrote: no piece of a secret is in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_picture_refusal_holds_no_piece_of_a_secret() {
    let e = env().await;
    let tid = e.with_avatar();
    let secret = "Kq9vX2mL".repeat(7);
    e.state.terminals.get(&tid).unwrap().redact.lock().push(crate::secrets::Secret::from_value(secret.clone()));
    call(&e, &tid, "avatar_design", figure()).await.unwrap();
    for args in [json!({ "frames": secret }), json!({ "pose": { "animation": secret } }), json!({ "pose": { "zoom": secret } })] {
        let said = call(&e, &tid, "avatar_status", args).await.unwrap();
        assert!(said.starts_with("No picture: ") && !said.contains(&secret[..12]), "{}", &said[..said.len().min(200)]);
    }
    let refused = call(&e, &tid, "avatar_status", json!({ "pose": { secret.as_str(): 1 } })).await.unwrap_err();
    assert!(!refused.message.contains(&secret[..12]), "{}", refused.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_picture_route_answers_a_png_to_the_session_alone() {
    let e = env().await;
    let tid = e.with_avatar();
    let other = e.with_avatar();
    call(&e, &tid, "avatar_design", figure()).await.unwrap();
    let addr = serve(&e).await;
    let own = token(&e, &tid);
    let post = |bearer: String, body: &'static str| {
        let url = format!("http://{addr}/api/hooks/avatar/{tid}/picture");
        async move { reqwest::Client::new().post(url).bearer_auth(bearer).body(body).send().await.unwrap() }
    };
    let rest = post(own.clone(), "").await;
    assert_eq!((rest.status().as_u16(), rest.headers()["content-type"].to_str().unwrap()), (200, "image/png"));
    // What it shows, in the words the tool says beside it.
    let caption = |answer: &reqwest::Response| percent_encoding::percent_decode_str(answer.headers()["x-workbench-caption"].to_str().unwrap()).decode_utf8().unwrap().into_owned();
    assert_eq!(caption(&rest), "Picture: your avatar at rest, on the light panel and on the dark one; under each, at its resting size and as a face.");
    let rest = rest.bytes().await.unwrap();
    png_size(&rest);
    let strip = post(own.clone(), "{\"frames\": \"wave\", \"count\": 2}").await;
    assert_eq!(strip.status().as_u16(), 200);
    assert!(caption(&strip).starts_with("Picture: 2 moments of wave, Workbench's built-in motion made from your roles, from its start"), "{}", caption(&strip));
    assert_eq!(png_size(&strip.bytes().await.unwrap()).1, preview::STRIP_FRAME_PX);
    // Its keys and no others; a session's token opens only its own avatar.
    let wrong = post(own.clone(), "{\"look\": true}").await;
    assert_eq!(wrong.status().as_u16(), 400);
    assert!(wrong.text().await.unwrap().contains("picture has no look"));
    assert_eq!(post(token(&e, &other), "").await.status().as_u16(), 401);
}

// ---------------------------------------------------------------- the command

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_command_drives_the_avatar_like_the_tools_do() {
    let e = env().await;
    let tid = e.with_avatar();
    let addr = serve(&e).await;
    let own = token(&e, &tid);
    // The command as a session's CLI runs it: its words, its standard input, the session's environment.
    let run = |words: &[&str], stdin: &str| {
        let (url, tid, own) = (format!("http://{addr}"), tid.clone(), own.clone());
        let (words, stdin): (Vec<String>, String) = (words.iter().map(|w| w.to_string()).collect(), stdin.to_string());
        async move {
            tokio::task::spawn_blocking(move || {
                let words: Vec<&str> = words.iter().map(String::as_str).collect();
                cli::tests::run_with(cli::tests::parsed(&words).expect("a form of the command"), [Some(&url), Some(&tid), Some(&own)], &stdin)
            })
            .await
            .unwrap()
        }
    };
    let mut outputs: Vec<(i32, String, String)> = Vec::new();
    let mut ran = |output: (i32, String, String)| {
        outputs.push(output.clone());
        output
    };

    // A design from a file.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("look.json");
    std::fs::write(&file, serde_json::to_string_pretty(&figure()).unwrap()).unwrap();
    let (code, out, err) = ran(run(&["design", file.to_str().unwrap()], "").await);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.starts_with("Saved as revision 1, named Dot.\nParts (id tag x y w h pivot x y, in root coordinates, at rest):\nbody g ") && out.ends_with('\n') && !out.ends_with("\n\n"), "{out}");
    assert_eq!((e.avatar(&tid).designed, e.avatar(&tid).via), (true, Via::Command));

    // A line: its words are joined, and the answer is the sentence a tool would give.
    let (code, out, err) = ran(run(&["say", "Tests", "are", "green.", "--gesture", "hop"], "").await);
    let line = e.avatar(&tid).lines[0].clone();
    assert_eq!((code, out, err.as_str()), (0, format!("Queued ({}).\n", line.id), ""));
    assert_eq!((line.text.as_str(), line.gesture.as_deref()), ("Tests are green.", Some("hop")));

    // A question nobody answers in time: JSON, and still exit status 0.
    let (code, out, err) = ran(run(&["ask", "Deploy now?", "--choice", "Yes", "--choice", "Not yet", "--wait", "0"], "").await);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let unanswered: Value = serde_json::from_str(&out).expect("the answer of ask is JSON");
    assert_eq!((&unanswered["answered"], &unanswered["reason"]), (&json!(false), &json!("timeout")));
    let ask_id = unanswered["askId"].as_str().unwrap().to_string();
    assert_eq!(e.avatar(&tid).ask.map(|ask| ask.choices), Some(vec!["Yes".to_string(), "Not yet".to_string()]));
    // Waiting again, the command is held until the user answers.
    let asking = tokio::spawn(run(&["ask", "--again", &ask_id, "--wait", "30"], ""));
    let ask = waited_for(&e, &tid).await;
    reply(&e.state, &tid, clicked(&ask.id, 0), now_ms()).await.unwrap();
    let (code, out, err) = ran(tokio::time::timeout(Duration::from_secs(30), asking).await.expect("the command returned").unwrap());
    assert_eq!((code, out.as_str(), err.as_str()), (0, "{\"answered\":true,\"text\":\"Yes\",\"choice\":0,\"via\":\"click\"}\n", ""));

    // A gesture, the status, the guide.
    assert_eq!(ran(run(&["act", "hop", "--times", "2"], "").await), (0, "Playing hop 2 times.\n".to_string(), String::new()));
    let (code, out, _) = ran(run(&["status"], "").await);
    assert!(code == 0 && out.starts_with("{\n  \"note\": \"You have a live avatar: say the short version with your avatar say command, ask with your avatar ask command.\","), "{out}");
    // A picture goes into the file the command names, and the command says what it shows.
    let png = dir.path().join("avatar.png");
    let (code, out, err) = ran(run(&["picture", png.to_str().unwrap(), "--frames", "hop", "--count", "2"], "").await);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.starts_with("Picture written to ") && out.contains("moments of hop") && std::fs::read(&png).unwrap().starts_with(b"\x89PNG"), "{out}");
    let (code, out, _) = ran(run(&["picture", png.to_str().unwrap(), "--pose", "-"], "{\"mouth\": 1}").await);
    assert!(code == 0 && out.contains("in that pose"), "{out}");
    // Without a name the picture goes to the temporary directory, and the command says where.
    let (code, out, _) = ran(run(&["picture"], "").await);
    let written = std::env::temp_dir().join(format!("workbench-avatar-{tid}-rest.png"));
    assert!(code == 0 && out.starts_with(&format!("Picture written to {}: your avatar at rest", written.display())), "{out}");
    assert!(std::fs::read(&written).unwrap().starts_with(b"\x89PNG"));
    let _ = std::fs::remove_file(&written);
    let (code, _, err) = ran(run(&["picture", png.to_str().unwrap(), "--frames", "wink"], "").await);
    assert!(code == 1 && err.contains("no animation \"wink\""), "{err}");
    let (code, out, _) = ran(run(&["status", "--look"], "").await);
    assert!(code == 0 && serde_json::from_str::<Value>(&out).unwrap()["avatar"]["design"]["svg"].is_string(), "{out}");
    let (code, out, err) = ran(run(&["guide"], "").await);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("# Your avatar\n") && out.contains("## Your commands") && out.contains("## This session's avatar now") && out.contains("\"designed\": true"), "{out}");

    // The artwork alone, from standard input.
    let (code, out, err) = ran(run(&["design", "-"], "<svg viewBox='0 0 200 200'><g id='body'><circle id='face' cx='100' cy='100' r='40' fill='#3d85c6'/></g></svg>\n").await);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.starts_with("Saved as revision 2, named Dot."), "{out}");

    // A refusal: exit status 1, the reason on standard error, nothing on standard output.
    let (code, out, err) = ran(run(&["act", "wink"], "").await);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.starts_with("No animation wink. Yours: none; built in: ") && err.ends_with('\n'), "{err}");
    let (code, out, err) = ran(run(&["design", "-"], "{\"svg\": \"<svg><script/></svg>\"}").await);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains("script") && err.ends_with("Nothing was changed.\n"), "{err}");
    turn_off(&e.state, &tid, now_ms()).unwrap();
    assert_eq!(ran(run(&["say", "Anyone there?"], "").await), (1, String::new(), format!("{TURNED_OFF}\n")));
    // The session ended and its token with it: refused as well, in Workbench's words.
    e.state.auth.revoke_agent_tokens(&tid);
    assert_eq!(ran(run(&["status"], "").await), (1, String::new(), "invalid agent token\n".to_string()));

    // No output of the command ever shows the token it sent.
    assert_eq!(outputs.len(), 17);
    for (code, out, err) in &outputs {
        assert!(!out.contains(&own) && !err.contains(&own), "the token is in the output of a run that ended with {code}");
    }
}

/// The command says what a picture shows in the server's words, the ones the tool says: that a
/// strip is Workbench's built-in motion, that a pose is a close-up, which moment was drawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_command_says_what_a_picture_shows_as_the_tool_does() {
    let e = env().await;
    let tid = e.with_avatar();
    call(&e, &tid, "avatar_design", figure()).await.unwrap();
    let addr = serve(&e).await;
    let own = token(&e, &tid);
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("avatar.png");
    let run = |words: Vec<String>, stdin: &'static str| {
        let (url, tid, own) = (format!("http://{addr}"), tid.clone(), own.clone());
        async move {
            tokio::task::spawn_blocking(move || {
                let words: Vec<&str> = words.iter().map(String::as_str).collect();
                cli::tests::run_with(cli::tests::parsed(&words).expect("a form of the command"), [Some(&url), Some(&tid), Some(&own)], stdin)
            })
            .await
            .unwrap()
        }
    };
    let file = png.to_str().unwrap().to_string();
    let f = file.as_str();
    for (words, stdin, args) in [
        (vec!["picture", f, "--frames", "wave"], "", json!({ "frames": "wave" })),
        (vec!["picture", f, "--pose", "-"], "{\"animation\": \"nod\", \"zoom\": \"eyes\"}", json!({ "pose": { "animation": "nod", "zoom": "eyes" } })),
        (vec!["picture", f, "--frames", "spin", "--count", "3"], "", json!({ "frames": "spin", "count": 3 })),
    ] {
        let (code, out, err) = run(words.iter().map(|w| w.to_string()).collect(), stdin).await;
        assert_eq!((code, err.as_str()), (0, ""), "{out}");
        let said = call(&e, &tid, "avatar_status", args).await.unwrap();
        let caption = said.lines().next().unwrap().strip_prefix("Picture: ").unwrap();
        assert_eq!(out, format!("Picture written to {file}: {caption} Open it and look.\n"));
        assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    }
}

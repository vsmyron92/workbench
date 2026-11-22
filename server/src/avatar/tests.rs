//! The slice through its state functions: the gate, what a session does with its avatar, what a
//! device does, what terminal events and time do, and what a restart keeps.
//!
//! The environment is a real `AppState` on temporary directories with one project. Sessions are
//! stand-in agent records (`Terminals::insert_test_agent`: running, idle, no process), terminal
//! events are fed to `on_event` as the terminals slice would emit them, emitted events are read
//! from `state.events.subscribe()`, and every clock is a value counted from [`T0`], so nothing
//! here waits for time to pass (a few tests give a wait of some milliseconds as its limit). Where
//! a reply is pasted into a session, a test takes the session's place ([`Env::pastes`]); one
//! test at the end starts a real session instead.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;
use tower::ServiceExt;

use super::device::{ClaimRequest, Delivered, HeardRequest, Replied, ReplyRequest, briefed, claim, dismiss_ask, heard, reply, reset, tell, turn_off, turn_on};
use super::session::{
    ActMode, ActRequest, AnswerWaiter, AskArgs, AskCall, AskRequest, Call, NOT_A_SESSION, NO_AVATAR, QUIET, SWITCHED_OFF, Said, SayRequest, SayWait, StatusRequest, TURNED_OFF, act, ask, ask_again, ask_call, ask_cancel,
    design, frame, gate, say, say_call, say_wait_within, status, unmasked, wait_answer_within, wait_of,
};
use super::store::{
    ACT_GAP_MS, BRIEF_REFUSALS, BRIEF_RETRY_SECS, ACT_PER_MINUTE, ASK_CALLS_PER_MINUTE, Answer, Ask, AvatarInfo, DESIGN_PER_MINUTE, HOLD_MAX_SECS, HeldMode, LINE_HELD_MAX_SECS, LINE_STALE_SECS, MAX_ASKED, MAX_AVATARS, MAX_LINES, MAX_PENDING_LINES, MAX_REPLY_CHARS, MAX_SAY_CHARS, NUDGE_AFTER_SECS,
    MAX_STORED_BYTES, NUDGES_PER_10_MIN, Outcome, Paste, Reason, ReplyVia, SAY_PER_MINUTE, STATUS_PER_MINUTE, Speaker, Window, get, list, on_event, reconcile, sweep, wake_all,
};
use super::{Carried, Sessions, Via, active, brief, carried, conversation_started, look, via_for};
use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::error::{ApiError, ApiResult};
use crate::events::Event;
use crate::secrets::Secret;
use crate::terminals::{AgentState, ProviderKind, TerminalInfo, TerminalStatus};

/// The time the tests count from: a value, never the wall clock.
pub(super) const T0: i64 = 1_800_000_000_000;
const SECOND: i64 = 1000;
const MINUTE: i64 = 60 * SECOND;

// ---------------------------------------------------------------- the environment

pub(super) struct Env {
    pub state: AppState,
    /// The one project.
    pub pid: String,
    pub root: std::path::PathBuf,
    pub router: axum::Router,
    _dirs: [tempfile::TempDir; 3],
}

pub(super) async fn env() -> Env {
    env_with(|_| {}).await
}

/// A real `AppState` on temporary directories with one project, and its router (the reply path
/// calls a route in-process). Neither terminals nor the avatar slice's background work is started.
pub(super) async fn env_with(configure: impl FnOnce(&mut GlobalConfig)) -> Env {
    let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = crate::util::os::path::canonicalize(proj.path()).unwrap();
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.display().to_string()];
    config.notify.desktop = false;
    config.agents.restore_on_start = false;
    // Most of these tests are about the mode in which the user decides who has an avatar; the
    // default (`any`: a session takes one itself) has its own, which say so.
    config.avatar.sessions = Some(Sessions::Asked);
    configure(&mut config);
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    // Nothing listens: the address only names the state.
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let router = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    Env { state, pid, root, router, _dirs: [cfg, data, proj] }
}

impl Env {
    /// A request through the router, as data's tests send one.
    pub async fn send(&self, method: Method, path: &str, bearer: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1").header("authorization", format!("Bearer {bearer}"));
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
    }

    /// A stand-in agent session of the project: running, idle, Claude Code, no process.
    pub fn agent(&self, meta: Value) -> String {
        self.state.terminals.insert_test_agent(Some(&self.pid), meta).id
    }

    /// A session that was started with the Avatar option, its record made as the lifecycle makes it.
    pub fn with_avatar(&self) -> String {
        let tid = self.agent(json!({ "avatar": true }));
        self.event("terminal.created", &tid, T0);
        tid
    }

    pub fn terminal(&self, tid: &str) -> TerminalInfo {
        self.state.terminals.info(tid).expect("the terminal")
    }

    /// Changes the stand-in session as the terminals slice would.
    pub fn change(&self, tid: &str, f: impl FnOnce(&mut TerminalInfo)) {
        let entry = self.state.terminals.get(tid).expect("the terminal");
        f(&mut entry.rec.lock().info);
    }

    pub fn set_state(&self, tid: &str, state: AgentState) {
        self.change(tid, |info| info.agent.as_mut().unwrap().state = state);
    }

    /// Feeds the slice a terminal event that carries the terminal as it is now.
    pub fn event(&self, kind: &str, tid: &str, ts: i64) {
        let info = self.terminal(tid);
        on_event(&self.state, &Event { kind: kind.into(), project_id: info.project_id.clone(), data: serde_json::to_value(&info).unwrap(), ts });
    }

    /// The session's state changes, and the slice is told.
    pub fn goes(&self, tid: &str, state: AgentState, ts: i64) {
        self.set_state(tid, state);
        self.event("terminal.updated", tid, ts);
    }

    pub fn avatar(&self, tid: &str) -> AvatarInfo {
        get(&self.state, tid).expect("an avatar")
    }

    pub fn file(&self, tid: &str) -> std::path::PathBuf {
        self.state.paths.data_dir.join("avatar").join(format!("{tid}.json"))
    }

    /// From now on the test is the session a reply is pasted into: each paste arrives here and
    /// waits for the test's verdict.
    pub fn pastes(&self) -> UnboundedReceiver<Paste> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.state.avatar.pastes.lock() = Some(tx);
        rx
    }

    pub fn say(&self, tid: &str, text: &str, now: i64) -> ApiResult<Said> {
        let pass = gate(&self.state, tid, Call::Say, Via::Tools, now)?;
        say(&self.state, &pass, SayRequest { text: text.into(), lang: None, gesture: None, wait: false }, now)
    }

    pub fn ask(&self, tid: &str, text: &str, choices: &[&str], now: i64) -> ApiResult<(Ask, AnswerWaiter)> {
        let pass = gate(&self.state, tid, Call::Ask, Via::Tools, now)?;
        ask(&self.state, &pass, question(text, choices), now)
    }

    /// A question whose call has stopped waiting: it stays open.
    pub async fn asked(&self, tid: &str, text: &str, choices: &[&str], now: i64) -> Ask {
        let (ask, waiter) = self.ask(tid, text, choices, now).unwrap();
        let left = wait_answer_within(waiter, Duration::ZERO, LONG).await;
        assert_eq!(left.reason, Some(Reason::Timeout));
        ask
    }

    pub fn design(&self, tid: &str, args: Value, now: i64) -> ApiResult<super::session::Designed> {
        let pass = gate(&self.state, tid, Call::Design, Via::Tools, now)?;
        design(&self.state, &pass, &args, now)
    }

    pub fn act(&self, tid: &str, req: ActRequest, now: i64) -> ApiResult<String> {
        let pass = gate(&self.state, tid, Call::Act, Via::Tools, now)?;
        act(&self.state, &pass, req, now).map(|acted| acted.note)
    }

    pub fn heard(&self, tid: &str, line_id: &str, outcome: Outcome, now: i64) {
        heard(&self.state, tid, line_id, HeardRequest { outcome, voice: None }, now).unwrap();
    }

    pub fn claim(&self, tid: &str, line_id: &str, tab: &str, now: i64) -> super::device::Claim {
        claim(&self.state, tid, line_id, ClaimRequest { tab: tab.into(), release: false }, now).unwrap()
    }
}

/// Long enough never to pass in a test.
pub(super) const LONG: Duration = Duration::from_secs(600);

pub(super) fn question(text: &str, choices: &[&str]) -> AskRequest {
    AskRequest { text: text.into(), choices: choices.iter().map(|c| c.to_string()).collect(), free_text: true, lang: None, gesture: None }
}

pub(super) fn typed(ask_id: Option<&str>, text: &str) -> ReplyRequest {
    ReplyRequest { ask_id: ask_id.map(str::to_string), text: Some(text.into()), choice: None, via: ReplyVia::Typed }
}

pub(super) fn clicked(ask_id: &str, index: usize) -> ReplyRequest {
    ReplyRequest { ask_id: Some(ask_id.into()), text: None, choice: Some(index), via: ReplyVia::Click }
}

pub(super) fn play(name: &str) -> ActRequest {
    ActRequest { play: Some(name.into()), ..Default::default() }
}

/// The avatar events emitted since the last look.
pub(super) struct Events(broadcast::Receiver<Arc<Event>>);

impl Events {
    pub fn of(e: &Env) -> Events {
        Events(e.state.events.subscribe())
    }

    pub fn drain(&mut self) -> Vec<Arc<Event>> {
        let mut out = Vec::new();
        while let Ok(event) = self.0.try_recv() {
            if event.kind.starts_with("avatar.") {
                out.push(event);
            }
        }
        out
    }

    /// The types of the events since the last look, in order.
    pub fn kinds(&mut self) -> Vec<String> {
        self.drain().iter().map(|event| event.kind.clone()).collect()
    }
}

pub(super) fn kinds(events: &[Arc<Event>]) -> Vec<&str> {
    events.iter().map(|event| event.kind.as_str()).collect()
}

/// Runs a reply whose prompt the test receives in the session's place, and lets the paste
/// succeed. The reply's answer, and what was pasted.
pub(super) async fn pasted(pastes: &mut UnboundedReceiver<Paste>, reply: impl Future<Output = ApiResult<Replied>>) -> (ApiResult<Replied>, String) {
    tokio::join!(reply, async {
        let paste = tokio::time::timeout(Duration::from_secs(10), pastes.recv()).await.expect("a paste").expect("a paste");
        let prompt = paste.prompt.clone();
        let _ = paste.done.send(Ok(()));
        prompt
    })
}

/// A small figure with roles, a loop, one-shots, and one animation whose paint ends elsewhere.
pub(super) fn figure() -> Value {
    json!({
        "name": "Dot",
        "svg": "<svg viewBox='0 0 200 200'><g id='body'><circle id='face' cx='100' cy='100' r='40' fill='#ffcc00' opacity='0.9'/><g id='eyes'><circle id='pupil' cx='90' cy='90' r='4' fill='#111111'/></g><rect id='arm' x='140' y='100' width='12' height='40' fill='#ffcc00'/></g></svg>",
        "pivots": { "body": [100, 140], "arm": [146, 100] },
        "roles": { "body": "body", "eyes": "eyes", "armR": "arm" },
        "animations": {
            "hop": { "duration": 0.6, "tracks": [{ "target": "body", "prop": "translateY", "values": [0, -18, 0] }] },
            "spin": { "duration": 2, "loop": true, "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 360] }] },
            "fade": { "duration": 1, "tracks": [{ "target": "arm", "prop": "translateY", "values": [0, 4] }, { "target": "face", "prop": "opacity", "values": [0.9, 0.2] }] },
            "blink": { "duration": 0.4, "tracks": [{ "target": "eyes", "prop": "scaleY", "values": [1, 0.1, 1] }] }
        },
        "voice": { "lang": "en-GB", "pitch": 1.1 }
    })
}

// ---------------------------------------------------------------- the gate

#[tokio::test]
async fn a_session_has_an_avatar_only_when_the_user_gave_it_one() {
    let e = env().await;
    let mut events = Events::of(&e);
    let tid = e.agent(json!({}));

    // By default a session cannot give itself one: it is refused, and remembered as having asked.
    let refused = gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (409, "conflict", NO_AVATAR));
    let all = list(&e.state);
    assert_eq!((all.enabled, all.sessions, all.avatars.len(), all.asked.as_slice()), (true, Sessions::Asked, 0, &[tid.clone()][..]));
    // The browsers are told once that the list changed.
    let told = events.drain();
    assert_eq!(kinds(&told), ["avatar.changed"]);
    assert_eq!((told[0].project_id.as_deref(), &told[0].data), (Some(e.pid.as_str()), &json!({ "terminalId": tid, "revision": 0, "on": false })));
    for call in [Call::Design, Call::Ask, Call::Act, Call::Say] {
        assert_eq!(gate(&e.state, &tid, call, Via::Tools, T0).unwrap_err().message, NO_AVATAR);
    }
    assert!(events.drain().is_empty() && list(&e.state).asked.len() == 1);
    assert!(!e.file(&tid).exists() && !active(&e.state, &tid));

    // Status passes the first two checks only, and reports.
    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, T0).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert_eq!((report.allowed, report.avatar.is_none(), report.note.as_str()), (false, true, NO_AVATAR));
    assert_eq!(serde_json::to_value(&report).unwrap()["avatar"], Value::Null);

    // A device gives it one. It was not started with the option and is running: it is to be told.
    let on = turn_on(&e.state, &tid, T0 + 1).unwrap();
    assert!(on.info.on && !on.info.designed && on.info.look.is_none() && on.brief);
    assert_eq!((on.info.terminal_id.as_str(), on.info.project_id.as_deref(), on.info.via, on.info.created_at), (tid.as_str(), Some(e.pid.as_str()), Via::Tools, T0 + 1));
    assert_eq!(events.kinds(), ["avatar.changed"]);
    assert!(list(&e.state).asked.is_empty() && e.file(&tid).exists() && active(&e.state, &tid));
    gate(&e.state, &tid, Call::Say, Via::Tools, T0 + 2).unwrap();

    // The user turns it off: that holds on every call, and the session is told why.
    let off = turn_off(&e.state, &tid, T0 + 3).unwrap();
    assert!(!off.on);
    assert_eq!(events.drain().last().unwrap().data, json!({ "terminalId": tid, "revision": 0, "on": false }));
    for call in [Call::Design, Call::Say, Call::Ask, Call::Act] {
        let refused = gate(&e.state, &tid, call, Via::Tools, T0 + 4).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (409, "conflict", TURNED_OFF));
    }
    assert!(!active(&e.state, &tid) && list(&e.state).asked.is_empty(), "it has one: it did not ask");
    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, T0 + 4).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert_eq!((report.allowed, report.avatar.as_ref().map(|a| a.on), report.note.as_str()), (false, Some(false), TURNED_OFF));

    // On again: still to be told, until that worked.
    assert!(turn_on(&e.state, &tid, T0 + 5).unwrap().brief);
    briefed(&e.state, &tid, T0 + 5);
    assert!(!turn_on(&e.state, &tid, T0 + 5).unwrap().brief, "told, and on already: nothing more to tell");
    // Turned off and on once more: the session read that its avatar was turned off, so it is
    // told again, once for each turning on.
    turn_off(&e.state, &tid, T0 + 6).unwrap();
    let again = turn_on(&e.state, &tid, T0 + 7).unwrap();
    assert!(again.info.on && again.brief);
    assert_eq!(again.info.updated_at, T0 + 7);
    briefed(&e.state, &tid, T0 + 7);
    assert!(!turn_on(&e.state, &tid, T0 + 8).unwrap().brief);

    // A session that was started with the option was told at its start.
    let started = e.agent(json!({ "avatar": true }));
    assert!(!turn_on(&e.state, &started, T0).unwrap().brief);
    // One that is not running is told when it runs again.
    let stopped = e.agent(json!({}));
    e.change(&stopped, |info| info.status = TerminalStatus::Exited);
    assert!(!turn_on(&e.state, &stopped, T0).unwrap().brief);
    e.change(&stopped, |info| info.status = TerminalStatus::Running);
    assert!(turn_on(&e.state, &stopped, T0).unwrap().brief);
}

#[tokio::test]
async fn with_sessions_any_a_session_gets_an_avatar_with_its_first_call() {
    let e = env_with(|cfg| cfg.avatar.sessions = Some(Sessions::Any)).await;
    let mut events = Events::of(&e);
    let tid = e.agent(json!({}));
    // Status makes none, and says that a call would.
    let pass = gate(&e.state, &tid, Call::Status, Via::Command, T0).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert!(report.allowed && report.avatar.is_none() && report.note.contains("gets one with its first"), "{}", report.note);
    assert!(list(&e.state).avatars.is_empty() && events.drain().is_empty());

    // The first call creates it: default look, on, driven the way the caller came.
    gate(&e.state, &tid, Call::Say, Via::Command, T0 + 1).unwrap();
    let all = list(&e.state);
    assert_eq!((all.sessions, all.avatars.len(), all.asked.len()), (Sessions::Any, 1, 0));
    let avatar = &all.avatars[0];
    assert!(avatar.on && avatar.look.is_none() && !avatar.designed);
    assert_eq!((avatar.via, avatar.name.as_str(), avatar.revision, avatar.created_at), (Via::Command, "", 0, T0 + 1));
    assert_eq!(events.kinds(), ["avatar.changed"]);
    let stored: Value = serde_json::from_slice(&std::fs::read(e.file(&tid)).unwrap()).unwrap();
    assert_eq!(stored, json!({ "version": 1, "on": true, "name": "", "designed": false, "look": null, "voice": {}, "via": "command" }));

    // The record's via follows the last interface used.
    gate(&e.state, &tid, Call::Status, Via::Tools, T0 + 2).unwrap();
    assert_eq!(e.avatar(&tid).via, Via::Tools);
    assert_eq!(events.kinds(), ["avatar.changed"]);
    let stored: Value = serde_json::from_slice(&std::fs::read(e.file(&tid)).unwrap()).unwrap();
    assert_eq!(stored["via"], "tools");
    gate(&e.state, &tid, Call::Act, Via::Tools, T0 + 3).unwrap();
    assert!(events.drain().is_empty(), "the same interface again changes nothing");
}

#[tokio::test]
async fn with_sessions_any_and_no_room_left_status_says_that_no_avatar_is_to_be_had() {
    let e = env_with(|cfg| cfg.avatar.sessions = Some(Sessions::Any)).await;
    let first = e.with_avatar();
    for _ in 1..MAX_AVATARS {
        e.with_avatar();
    }
    let tid = e.agent(json!({}));
    // Status says what the first call would meet, in the gate's words, and allows nothing.
    let no_room = "No avatar: 64 sessions have one already. Say it in the terminal instead.";
    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, T0).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert_eq!((report.allowed, report.avatar.is_none(), report.note.as_str()), (false, true, no_room));
    assert_eq!(gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap_err().message, no_room);
    // A session that holds one of them is told of its own as before.
    let pass = gate(&e.state, &first, Call::Status, Via::Tools, T0).unwrap();
    assert!(status(&e.state, &pass, StatusRequest::default()).unwrap().allowed);

    // A removed terminal makes room: the first call gives one again, and status says so.
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: None, data: json!({ "id": first }), ts: T0 + 1 });
    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, T0 + 2).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert!(report.allowed && report.avatar.is_none() && report.note.contains("gets one with its first"), "{}", report.note);
}

#[tokio::test]
async fn while_sessions_take_one_themselves_no_session_is_listed_as_having_asked() {
    let e = env().await;
    let tid = e.agent(json!({}));
    // Where the user decides, a session that calls for its avatar is listed as having asked.
    gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap_err();
    assert_eq!(list(&e.state).asked, [tid.clone()]);
    // Switched to the default, it may take one itself: nobody waits for the user to give one.
    e.state.config.write().avatar.sessions = Some(Sessions::Any);
    let all = list(&e.state);
    assert_eq!((all.sessions, all.asked.len()), (Sessions::Any, 0));
    assert_eq!(serde_json::to_value(&all).unwrap()["asked"], json!([]));
    // Switched back, the session that asked is listed again, until it ends or is given one.
    e.state.config.write().avatar.sessions = Some(Sessions::Asked);
    assert_eq!(list(&e.state).asked, [tid.clone()]);
}

#[tokio::test]
async fn switched_off_in_settings_every_call_is_refused_and_nothing_is_made() {
    let e = env().await;
    let had = e.with_avatar();
    e.state.config.write().avatar.enabled = Some(false);
    let mut events = Events::of(&e);

    for call in [Call::Status, Call::Design, Call::Say, Call::Ask, Call::Act] {
        let refused = gate(&e.state, &had, call, Via::Tools, T0).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (403, "forbidden", SWITCHED_OFF));
    }
    let all = list(&e.state);
    assert!(!all.enabled && all.avatars.is_empty() && all.asked.is_empty());
    assert_eq!(get(&e.state, &had).unwrap_err().status.as_u16(), 403);
    assert!(!active(&e.state, &had) && carried(&e.state, &had).is_none());
    for refused in [turn_on(&e.state, &had, T0).map(|_| ()), turn_off(&e.state, &had, T0).map(|_| ()), reset(&e.state, &had, T0).map(|_| ()), dismiss_ask(&e.state, &had, "Q", T0)] {
        assert_eq!(refused.unwrap_err().code, "forbidden");
    }
    assert_eq!(reply(&e.state, &had, typed(None, "hello"), T0).await.unwrap_err().code, "forbidden");

    // No session gets one meanwhile, whatever option it was started with.
    let new = e.agent(json!({ "avatar": true }));
    e.event("terminal.created", &new, T0);
    assert!(!e.file(&new).exists() && events.drain().is_empty());

    // Switched on again: what existed is there, what was not made is not.
    e.state.config.write().avatar.enabled = None;
    let all = list(&e.state);
    assert_eq!(all.avatars.iter().map(|a| a.terminal_id.as_str()).collect::<Vec<_>>(), [had.as_str()]);
    assert_eq!(gate(&e.state, &new, Call::Say, Via::Tools, T0).unwrap_err().message, NO_AVATAR);
}

#[tokio::test]
async fn only_a_running_agent_session_passes_the_gate() {
    let e = env().await;
    // Any text may arrive as a terminal id: nothing is made of one the terminals slice does not know.
    for unknown in ["nope", "", "../../etc/passwd", &"x".repeat(200)] {
        for call in [Call::Status, Call::Say] {
            let refused = gate(&e.state, unknown, call, Via::Tools, T0).unwrap_err();
            assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (400, "bad_request", NOT_A_SESSION));
        }
        assert_eq!(turn_on(&e.state, unknown, T0).unwrap_err().status.as_u16(), 404);
    }
    assert!(list(&e.state).asked.is_empty());
    assert_eq!(std::fs::read_dir(e.state.paths.data_dir.join("avatar")).unwrap().count(), 0);

    // A session whose process ended is not one that can call.
    let tid = e.with_avatar();
    gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap();
    e.change(&tid, |info| info.status = TerminalStatus::Exited);
    assert_eq!(gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap_err().message, NOT_A_SESSION);
    e.change(&tid, |info| info.status = TerminalStatus::Starting);
    assert_eq!(gate(&e.state, &tid, Call::Status, Via::Tools, T0).unwrap_err().message, NOT_A_SESSION);

    // How a session drives its avatar follows from its kind and where it runs.
    assert_eq!((via_for(ProviderKind::Claude, false), via_for(ProviderKind::Claude, true)), (Via::Tools, Via::Tools));
    assert_eq!((via_for(ProviderKind::Codex, false), via_for(ProviderKind::Codex, true)), (Via::Tools, Via::Command));
    for kind in [ProviderKind::Kimi, ProviderKind::Gemini, ProviderKind::Aider, ProviderKind::Custom] {
        assert_eq!((via_for(kind, false), via_for(kind, true)), (Via::Command, Via::Command));
    }
    assert_eq!(serde_json::to_value([Via::Tools, Via::Command]).unwrap(), json!(["tools", "command"]));
}

#[test]
fn a_rate_counts_every_attempt_of_the_last_minute() {
    let mut window = Window::default();
    assert!((0..3).all(|i| window.attempt(T0 + i, 3)));
    // Refused attempts count as well: hammering keeps the door shut.
    assert!((0..100).all(|i| !window.attempt(T0 + 10 + i * 500, 3)));
    assert!(!window.attempt(T0 + 59 * SECOND, 3));
    // A minute after the last attempt the count starts again.
    assert!(window.attempt(T0 + 59 * SECOND + MINUTE, 3));
    assert!(window.attempt(T0 + 59 * SECOND + MINUTE + 1, 3));
    assert!(window.attempt(T0 + 59 * SECOND + MINUTE + 2, 3));
    assert!(!window.attempt(T0 + 59 * SECOND + MINUTE + 3, 3));
}

#[tokio::test]
async fn every_rate_limit_refuses_past_it_and_emits_nothing() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);
    let too_many = |refused: ApiError, starts: &str| {
        assert_eq!((refused.status.as_u16(), refused.code), (429, "rate_limited"), "{}", refused.message);
        assert!(refused.message.starts_with(starts), "{}", refused.message);
    };

    // Lines and questions share one rate.
    for i in 0..SAY_PER_MINUTE as i64 {
        let said = e.say(&tid, "One more line.", T0 + i).unwrap();
        e.heard(&tid, &said.line.id, Outcome::Shown, T0 + i);
    }
    events.drain();
    too_many(e.say(&tid, "One too many.", T0 + 10).unwrap_err(), "Not said: at most 6 lines and questions a minute.");
    too_many(e.ask(&tid, "And a question?", &[], T0 + 11).map(|_| ()).unwrap_err(), "Not asked: at most 6 lines and questions a minute.");
    // Refused attempts count too: a minute after the first line the door is still shut.
    too_many(e.say(&tid, "Still too many.", T0 + MINUTE + 1).unwrap_err(), "Not said");
    assert!(events.drain().is_empty());
    assert_eq!(e.avatar(&tid).lines.len(), SAY_PER_MINUTE);
    let said = e.say(&tid, "A minute later.", T0 + 2 * MINUTE + 10).unwrap();
    e.heard(&tid, &said.line.id, Outcome::Shown, T0 + 2 * MINUTE + 10);
    assert_eq!(events.kinds(), ["avatar.say", "avatar.line"]);

    // Design calls, also refused ones.
    let at = T0 + 10 * MINUTE;
    for i in 0..DESIGN_PER_MINUTE as i64 {
        if i % 2 == 0 {
            e.design(&tid, json!({ "name": format!("Dot {i}") }), at + i).unwrap();
        } else {
            assert_eq!(e.design(&tid, json!({ "svg": "<svg><script/></svg>" }), at + i).unwrap_err().code, "bad_request");
        }
    }
    events.drain();
    too_many(e.design(&tid, json!({ "name": "Late" }), at + 20).unwrap_err(), "Nothing was changed: at most 6 design calls a minute.");
    assert!(events.drain().is_empty());
    assert_eq!(e.avatar(&tid).name, "Dot 4");

    // Gestures.
    for i in 0..ACT_PER_MINUTE as i64 {
        e.act(&tid, play("nod"), at + i * ACT_GAP_MS).unwrap();
    }
    assert_eq!(events.drain().len(), ACT_PER_MINUTE);
    too_many(e.act(&tid, play("nod"), at + 31 * ACT_GAP_MS).unwrap_err(), "Not played: at most 30 gestures a minute.");
    sweep(&e.state, at + 40 * ACT_GAP_MS);
    assert!(events.drain().is_empty());

    // Calls of ask, whatever they do: a session cannot poll its question faster than that.
    let nothing_open = |now: i64| gate(&e.state, &tid, Call::Ask, Via::Tools, now).and_then(|pass| ask_cancel(&e.state, &pass, now)).unwrap_err();
    for i in 0..ASK_CALLS_PER_MINUTE as i64 {
        assert_eq!(nothing_open(at + i).code, "not_pending");
    }
    too_many(nothing_open(at + 50), "At most 30 ask calls a minute");
    assert!(events.drain().is_empty());

    // A pass is for the kind of call it was given for: its rate is the one that was counted.
    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, at).unwrap();
    let misused = say(&e.state, &pass, SayRequest { text: "Hello.".into(), lang: None, gesture: None, wait: false }, at).unwrap_err();
    assert_eq!((misused.status.as_u16(), misused.code), (500, "internal"));

    // Status calls, of a session without an avatar as well.
    let bare = e.agent(json!({}));
    for who in [&tid, &bare] {
        for i in 0..STATUS_PER_MINUTE as i64 {
            gate(&e.state, who, Call::Status, Via::Tools, at + 2 * MINUTE + i).unwrap();
        }
        too_many(gate(&e.state, who, Call::Status, Via::Tools, at + 2 * MINUTE + 100).unwrap_err(), "At most 30 status calls a minute");
    }
    assert!(events.drain().is_empty());
    // One session's calls do not count against another's.
    let other = e.with_avatar();
    e.say(&other, "Hello.", T0 + 2 * MINUTE + 11).unwrap();
}

// ---------------------------------------------------------------- say

#[tokio::test]
async fn a_line_is_plain_text_with_the_sessions_secrets_masked() {
    let e = env().await;
    let tid = e.with_avatar();
    e.state.terminals.get(&tid).unwrap().redact.lock().push(Secret::from_value("hunter2hunter2".into()));
    let mut events = Events::of(&e);

    // Line breaks become spaces, what reorders or hides text goes, a secret is masked even when
    // an invisible character splits it.
    let said = e.say(&tid, "  The build\r\nis green\u{202E}: key hunter2\u{200B}hunter2\tdone ", T0).unwrap();
    assert_eq!(said.line.text, "The build is green: key •••••• done");
    assert!(said.line.id.starts_with('L') && said.line.id.ends_with("-1"), "{}", said.line.id);
    assert_eq!((said.line.at, said.line.from, said.line.outcome), (T0, Speaker::Agent, None));
    let emitted = events.drain();
    assert_eq!(kinds(&emitted), ["avatar.say"]);
    // The terminal's project is on the envelope; the line is the one the record holds.
    assert_eq!(emitted[0].project_id.as_deref(), Some(e.pid.as_str()));
    assert_eq!(emitted[0].data, json!({ "terminalId": tid, "line": { "id": said.line.id, "at": T0, "from": "agent", "text": "The build is green: key •••••• done" } }));
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.lines.as_slice(), avatar.updated_at), (&[said.line.clone()][..], T0));
    assert_eq!(said.answer().note, format!("Queued ({}).", said.line.id));

    // A language and a gesture travel with the line.
    let pass = gate(&e.state, &tid, Call::Say, Via::Tools, T0 + 1).unwrap();
    let said = say(&e.state, &pass, SayRequest { text: "Guten Tag.".into(), lang: Some(" de-DE ".into()), gesture: Some("wave".into()), wait: false }, T0 + 1).unwrap();
    assert_eq!((said.line.lang.as_deref(), said.line.gesture.as_deref()), (Some("de-DE"), Some("wave")));
    assert_eq!(events.drain()[0].data["line"], json!({ "id": said.line.id, "at": T0 + 1, "from": "agent", "text": "Guten Tag.", "lang": "de-DE", "gesture": "wave" }));

    // The whole request is checked before anything changes: each refusal says what to send.
    let pass = gate(&e.state, &tid, Call::Say, Via::Tools, T0 + 2).unwrap();
    let refuse = |req: SayRequest| say(&e.state, &pass, req, T0 + 2).unwrap_err();
    let line = |text: &str| SayRequest { text: text.into(), lang: None, gesture: None, wait: false };
    let long = refuse(line(&"a".repeat(MAX_SAY_CHARS + 1)));
    assert_eq!((long.status.as_u16(), long.message.as_str()), (400, "the line has 401 characters: at most 400"));
    assert!(refuse(line(&"a".repeat(MAX_SAY_CHARS * 20))).message.starts_with("the line is 8000 bytes long: at most 400 characters"));
    assert!(refuse(line(" \u{200B}\n ")).message.starts_with("the line is empty"));
    assert_eq!(refuse(SayRequest { lang: Some("english please".into()), ..line("Hi.") }).message, "lang \"english please\" is not a language tag: write one like en-GB or de");
    assert_eq!(
        refuse(SayRequest { gesture: Some("wink".into()), ..line("Hi.") }).message,
        "No animation wink. Yours: none; built in: breathe, blink, think, talk, listen, alert, doze, wave, cheer, nod, shake"
    );
    assert!(events.drain().is_empty());
    assert_eq!(e.avatar(&tid).lines.len(), 2);
    e.say(&tid, &"a".repeat(MAX_SAY_CHARS), T0 + 3).unwrap();

    // What will not sound like speech is accepted with a hint.
    for literal in ["Run `cargo test` now.", "See https://example.org for more.", "I changed src/main.rs.", "It is **done**.", "Look in ~/notes."] {
        let tid = e.with_avatar();
        assert!(e.say(&tid, literal, T0).unwrap().answer().note.ends_with("It will be read out literally: use plain words."), "{literal}");
    }
    for spoken in ["Tests pass and/or nothing changed.", "Three of 4 are done, 3/4 that is.", "It took 2.5 seconds: fine."] {
        let tid = e.with_avatar();
        assert_eq!(e.say(&tid, spoken, T0).unwrap().answer().note.matches("literally").count(), 0, "{spoken}");
    }
    // The arguments of a call are read strictly.
    assert_eq!(SayRequest::parse(&json!({ "text": "Hi.", "wait": true, "lang": "en" }), &unmasked).unwrap(), SayRequest { text: "Hi.".into(), lang: Some("en".into()), gesture: None, wait: true });
    assert_eq!(SayRequest::parse(&json!({ "text": "Hi.", "speed": 2 }), &unmasked).unwrap_err().message, "say has no speed: it takes text, gesture, lang, wait");
    assert_eq!(SayRequest::parse(&json!({ "text": 7 }), &unmasked).unwrap_err().message, "text is a number: write a string");
    assert!(SayRequest::parse(&json!({}), &unmasked).unwrap_err().message.starts_with("say needs text"));
    assert!(SayRequest::parse(&json!("Hi."), &unmasked).unwrap_err().message.starts_with("say takes a JSON object with text, gesture, lang, wait; this is a string"));
}

#[tokio::test]
async fn a_refusal_holds_no_piece_of_a_long_secret() {
    let e = env().await;
    let tid = e.with_avatar();
    e.design(&tid, figure(), T0).unwrap();
    // Longer than a quote: were it cut before it is masked, its first forty characters would be
    // left in the refusal, and the platform's mask would not know them any more.
    let secret = "Kq9vX2mL".repeat(7);
    e.state.terminals.get(&tid).unwrap().redact.lock().push(Secret::from_value(secret.clone()));
    let line = |lang: Option<&str>, gesture: Option<&str>| SayRequest { text: "Hi.".into(), lang: lang.map(str::to_string), gesture: gesture.map(str::to_string), wait: false };
    let say_pass = gate(&e.state, &tid, Call::Say, Via::Tools, T0 + 1).unwrap();
    let ask_pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + 1).unwrap();
    let refusals = [
        ("a language", say(&e.state, &say_pass, line(Some(&secret), None), T0 + 1).unwrap_err()),
        ("a gesture", say(&e.state, &say_pass, line(None, Some(&secret)), T0 + 1).unwrap_err()),
        ("a question's gesture", ask(&e.state, &ask_pass, AskRequest { gesture: Some(secret.clone()), ..question("Go on?", &[]) }, T0 + 1).map(|_| ()).unwrap_err()),
        ("a question's id", ask_again(&e.state, &ask_pass, &secret, T0 + 1).map(|_| ()).unwrap_err()),
        ("an act", e.act(&tid, play(&secret), T0 + 1).unwrap_err()),
        ("artwork", e.design(&tid, json!({ "svg": format!("<svg viewBox='0 0 200 200'>{secret}<circle id='c' r='5'/></svg>") }), T0 + 1).unwrap_err()),
        ("a key", SayRequest::parse(&json!({ "text": "Hi.", secret.as_str(): 1 }), &say_pass.mask(&e.state)).unwrap_err()),
        ("a mode", ActRequest::parse(&json!({ "play": "hop", "mode": secret }), &say_pass.mask(&e.state)).unwrap_err()),
    ];
    for (what, refused) in refusals {
        assert!(refused.message.contains("••••••") && !refused.message.contains(&secret[..8]), "{what}: {}", refused.message);
    }
}

/// The store lock is a leaf: a refusal that quotes what the session sent is masked, which asks the
/// terminals slice for the session's secrets, before the lock is taken.
#[tokio::test]
async fn a_refusal_is_masked_with_the_store_lock_free() {
    let e = env().await;
    let tid = e.with_avatar();
    let entry = e.state.terminals.get(&tid).unwrap();
    let ask_pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0).unwrap();
    let act_pass = gate(&e.state, &tid, Call::Act, Via::Tools, T0).unwrap();
    let stop = ActRequest { stop: Some("wink".into()), ..Default::default() };
    let calls: [(&str, Box<dyn Fn() -> ApiError + Send + Sync + '_>); 3] = [
        ("a question's id", Box::new(|| ask_again(&e.state, &ask_pass, "Q-gone", T0).map(|_| ()).unwrap_err())),
        ("a gesture to play", Box::new(|| act(&e.state, &act_pass, play("wink"), T0).map(|_| ()).unwrap_err())),
        ("a gesture to stop", Box::new(|| act(&e.state, &act_pass, stop.clone(), T0).map(|_| ()).unwrap_err())),
    ];
    for (what, call) in &calls {
        std::thread::scope(|s| {
            // The terminals slice holds the session's secrets a moment: the call waits for them,
            // and the store stays free meanwhile.
            let secrets = entry.redact.lock();
            let refusing = s.spawn(|| call());
            let mut held = false;
            for _ in 0..20 {
                held |= e.state.avatar.inner.is_locked();
                std::thread::sleep(Duration::from_millis(5));
            }
            drop(secrets);
            let refused = refusing.join().unwrap();
            assert!(!held, "{what}: the store lock was held while the refusal was masked");
            assert!(refused.message.contains("Q-gone") || refused.message.contains("wink"), "{what}: {}", refused.message);
        });
    }
}

#[tokio::test]
async fn the_avatar_is_quiet_while_the_session_needs_the_user() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);
    for needs in [AgentState::NeedsPermission, AgentState::NeedsInput] {
        e.set_state(&tid, needs);
        let refused = e.say(&tid, "Nearly done.", T0).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (409, "conflict", QUIET));
        assert_eq!(e.ask(&tid, "Go on?", &["Yes"], T0).map(|_| ()).unwrap_err().message, QUIET);
        // What says nothing is not held back.
        e.act(&tid, play("nod"), T0 + needs as i64 * SECOND).unwrap();
        gate(&e.state, &tid, Call::Status, Via::Tools, T0).unwrap();
    }
    assert_eq!(events.kinds(), ["avatar.act", "avatar.act"]);
    assert!(e.avatar(&tid).lines.is_empty());
    for (i, speaks) in [AgentState::Working, AgentState::Idle, AgentState::Starting, AgentState::Error].into_iter().enumerate() {
        e.set_state(&tid, speaks);
        let at = T0 + 2 * MINUTE + i as i64 * SECOND;
        let said = e.say(&tid, "Nearly done.", at).unwrap();
        e.heard(&tid, &said.line.id, Outcome::Shown, at);
    }
}

#[tokio::test]
async fn a_fourth_line_waits_until_one_was_presented() {
    let e = env().await;
    let tid = e.with_avatar();
    let lines: Vec<String> = (0..MAX_PENDING_LINES as i64).map(|i| e.say(&tid, "Working on it.", T0 + i).unwrap().line.id).collect();
    let mut events = Events::of(&e);
    let refused = e.say(&tid, "Still working.", T0 + 5).unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (429, "rate_limited", "Not said: 3 lines are still waiting to be spoken. Speak less often."));
    assert!(e.ask(&tid, "Shall I go on?", &[], T0 + 6).map(|_| ()).unwrap_err().message.starts_with("Not asked: 3 lines are still waiting"));
    assert!(events.drain().is_empty());

    // Any acknowledgement makes room, a caption included.
    e.heard(&tid, &lines[0], Outcome::Shown, T0 + 7);
    let said = e.say(&tid, "Done.", T0 + 8).unwrap();
    // The answer tells what became of the earlier lines, once.
    assert_eq!(said.answer().note, format!("Queued ({}). Earlier: {} shown as a caption, not spoken.", said.line.id, lines[0]));

    // Nobody listens: after a while the lines are unheard, and the next answer says to stop.
    sweep(&e.state, T0 + 8 + LINE_STALE_SECS * SECOND + 1);
    let said = e.say(&tid, "Is anyone there?", T0 + 2 * MINUTE).unwrap();
    let note = said.answer().note;
    assert!(note.contains(&format!("{} not heard: nobody had this project open in Workbench: stop speaking until the user writes.", lines[2])), "{note}");
    assert_eq!(note.matches("not heard").count(), 3, "the newest three: {note}");
    assert!(!note.contains(&lines[0]), "told before: {note}");
}

// ---------------------------------------------------------------- ask: every ending

#[tokio::test]
async fn an_answer_reaches_the_call_that_waits_for_it() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);
    let (ask, waiter) = e.ask(&tid, "  Deploy\nnow? ", &["Yes, deploy", "Not yet"], T0).unwrap();
    assert!(ask.id.starts_with('Q') && ask.id.ends_with("-1"), "{}", ask.id);
    assert_eq!((ask.text.as_str(), ask.choices.as_slice(), ask.free_text, ask.asked_at, ask.waiting), ("Deploy now?", &["Yes, deploy".to_string(), "Not yet".to_string()][..], true, T0, true));
    assert_eq!(waiter.ask_id(), ask.id);

    // Opening a question: its line with the question, then `opened`.
    let opened = events.drain();
    assert_eq!(kinds(&opened), ["avatar.say", "avatar.ask"]);
    assert_eq!(opened[0].data, json!({ "terminalId": tid, "line": { "id": ask.line_id, "at": T0, "from": "agent", "text": "Deploy now?", "askId": ask.id }, "ask": ask }));
    assert_eq!(opened[1].data, json!({ "terminalId": tid, "change": "opened", "ask": ask }));
    assert_eq!(serde_json::to_value(&ask).unwrap(), json!({ "id": ask.id, "lineId": ask.line_id, "text": "Deploy now?", "choices": ["Yes, deploy", "Not yet"], "freeText": true, "askedAt": T0, "waiting": true }));
    assert_eq!(e.avatar(&tid).ask, Some(ask.clone()));

    // A click on the first choice, while the call waits. No session is there to paste into: the
    // answer goes to the call alone.
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    let replied = reply(&e.state, &tid, clicked(&ask.id, 0), T0 + 2 * SECOND).await.unwrap();
    assert_eq!(serde_json::to_value(replied).unwrap(), json!({ "delivered": "answer" }));
    let answer = waiting.await.unwrap();
    assert_eq!(serde_json::to_value(&answer).unwrap(), json!({ "answered": true, "text": "Yes, deploy", "choice": 0, "via": "click" }));

    // The reply is a line of the user, and the question is closed.
    let closed = events.drain();
    assert_eq!(kinds(&closed), ["avatar.line", "avatar.ask"]);
    let line = &closed[0].data["line"];
    assert_eq!(line, &json!({ "id": line["id"], "at": T0 + 2 * SECOND, "from": "user", "text": "Yes, deploy", "askId": ask.id, "via": "click" }));
    assert_eq!(closed[1].data, json!({ "terminalId": tid, "change": "answered", "ask": null }));
    let avatar = e.avatar(&tid);
    assert!(avatar.ask.is_none());
    assert_eq!(avatar.lines.iter().map(|l| l.from).collect::<Vec<_>>(), [Speaker::Agent, Speaker::User]);
    // Answered is answered: the same click again finds nothing open.
    assert_eq!(reply(&e.state, &tid, clicked(&ask.id, 0), T0 + 3 * SECOND).await.unwrap_err().code, "not_pending");
}

#[tokio::test]
async fn an_answer_after_the_wait_is_pasted_as_one_line_that_holds_no_question() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut pastes = e.pastes();
    let (ask, waiter) = e.ask(&tid, "Which database should the tests use?", &["Postgres", "SQLite"], T0).unwrap();
    let mut events = Events::of(&e);

    // The call stops waiting: the question stays on screen, and its answer says what follows.
    let left = wait_answer_within(waiter, Duration::ZERO, LONG).await;
    assert_eq!(
        serde_json::to_value(&left).unwrap(),
        json!({ "answered": false, "reason": "timeout", "askId": ask.id, "note": format!("Not answered yet. The question stays on screen. Do not ask again and do not assume an answer. Go on with work that does not depend on it, or end your turn; the answer arrives as the user's next message, starting with [via avatar…] and naming {}.", ask.id) })
    );
    let still = Ask { waiting: false, ..ask.clone() };
    let left_events = events.drain();
    assert_eq!(kinds(&left_events), ["avatar.ask"]);
    assert_eq!(left_events[0].data, json!({ "terminalId": tid, "change": "waiter_left", "ask": still }));
    assert_eq!(e.avatar(&tid).ask, Some(still));

    // The user types an answer, on several lines: it goes into the session as one prompt.
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, typed(Some(&ask.id), "  Use\r\nPostgres,\tplease \u{202E}"), T0 + 5 * SECOND)).await;
    assert_eq!(replied.unwrap(), Replied { delivered: Delivered::Prompt });
    assert_eq!(prompt, format!("[via avatar, answer with avatar_say] Answer to question {}: Use Postgres, please", ask.id));
    assert!(!prompt.contains('\n') && !prompt.contains("database") && !prompt.contains("SQLite"), "{prompt}");
    let closed = events.drain();
    assert_eq!(kinds(&closed), ["avatar.line", "avatar.ask"]);
    assert_eq!(closed[0].data["line"]["text"], "Use Postgres, please");
    assert_eq!((closed[0].data["line"]["askId"].as_str(), closed[0].data["line"]["via"].as_str()), (Some(ask.id.as_str()), Some("typed")));
    assert_eq!(closed[1].data["change"], "answered");
    assert!(e.avatar(&tid).ask.is_none());

    // A clicked choice is the only text of the session that goes back in; it counts from 1.
    let ask = e.asked(&tid, "And for production?", &["Postgres", "SQLite"], T0 + MINUTE).await;
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, clicked(&ask.id, 1), T0 + MINUTE + SECOND)).await;
    assert_eq!(replied.unwrap().delivered, Delivered::Prompt);
    assert_eq!(prompt, format!("[via avatar, answer with avatar_say] Answer to question {}: choice 2, \"SQLite\"", ask.id));
    assert!(!prompt.contains("production"));
}

#[test]
fn the_frame_of_a_reply_names_how_it_was_given_and_how_to_answer() {
    let words = |text: &str, via: ReplyVia| Answer { text: text.into(), choice: None, via };
    assert_eq!(frame(Via::Tools, Some("Q1a2b-3"), &words("yes please", ReplyVia::Typed)), "[via avatar, answer with avatar_say] Answer to question Q1a2b-3: yes please");
    assert_eq!(frame(Via::Tools, Some("Q1a2b-3"), &words("yes please", ReplyVia::Voice)), "[via avatar, dictated, answer with avatar_say] Answer to question Q1a2b-3: yes please");
    assert_eq!(
        frame(Via::Tools, Some("Q1a2b-3"), &Answer { text: "Not yet".into(), choice: Some(1), via: ReplyVia::Click }),
        "[via avatar, answer with avatar_say] Answer to question Q1a2b-3: choice 2, \"Not yet\""
    );
    // A dictated answer that equals a choice is sent as that choice.
    assert_eq!(
        frame(Via::Tools, Some("Q1a2b-3"), &Answer { text: "Not yet".into(), choice: Some(1), via: ReplyVia::Voice }),
        "[via avatar, dictated, answer with avatar_say] Answer to question Q1a2b-3: choice 2, \"Not yet\""
    );
    // A message, and a session that drives its avatar with a command.
    assert_eq!(frame(Via::Tools, None, &words("Stop and show me the diff", ReplyVia::Typed)), "[via avatar, answer with avatar_say] Stop and show me the diff");
    assert_eq!(frame(Via::Command, None, &words("Stop", ReplyVia::Voice)), "[via avatar, dictated, answer with your avatar say command] Stop");
    assert_eq!(frame(Via::Command, Some("Q0000-1"), &words("ok", ReplyVia::Typed)), "[via avatar, answer with your avatar say command] Answer to question Q0000-1: ok");
}

#[tokio::test]
async fn a_question_nobody_saw_returns_at_once_and_one_that_was_seen_waits_its_time() {
    let e = env().await;
    let tid = e.with_avatar();
    let (ask, waiter) = e.ask(&tid, "Shall I rebase first?", &[], T0).unwrap();
    let mut events = Events::of(&e);

    // No device acknowledges the question: the call does not sit out its wait.
    let unseen = wait_answer_within(waiter, LONG, Duration::from_millis(20)).await;
    assert_eq!((unseen.answered, unseen.reason, unseen.ask_id.as_deref()), (false, Some(Reason::Unseen), Some(ask.id.as_str())));
    let note = unseen.note.unwrap();
    assert!(note.starts_with("Nobody saw the question") && note.contains("It stays open") && note.ends_with(&format!("naming {}.", ask.id)), "{note}");
    assert_eq!(events.kinds(), ["avatar.ask"]);
    assert_eq!(e.avatar(&tid).ask.map(|a| (a.id, a.waiting)), Some((ask.id.clone(), false)));

    // Hidden avatars have not shown it to anyone either.
    e.heard(&tid, &ask.line_id, Outcome::Hidden, T0 + 1);
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + 2).unwrap();
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, T0 + 2).unwrap();
    assert_eq!(wait_answer_within(waiter, LONG, Duration::from_millis(20)).await.reason, Some(Reason::Unseen));

    // A call that waits less than a device needs to acknowledge has only timed out.
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, T0 + 2).unwrap();
    assert_eq!(wait_answer_within(waiter, Duration::from_millis(20), LONG).await.reason, Some(Reason::Timeout));

    // Once a device showed it, the wait counts from there: it ends as a timeout.
    e.heard(&tid, &ask.line_id, Outcome::Shown, T0 + 3);
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, T0 + 4).unwrap();
    let timed_out = wait_answer_within(waiter, Duration::from_millis(20), LONG).await;
    assert_eq!(timed_out.reason, Some(Reason::Timeout));
    assert!(e.avatar(&tid).ask.is_some(), "the question stays open");

    // An acknowledgement that comes while the call waits moves it on from the first phase.
    let tid = e.with_avatar();
    let (ask, waiter) = e.ask(&tid, "And this one?", &[], T0).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, Duration::from_millis(40), LONG));
    tokio::task::yield_now().await;
    e.heard(&tid, &ask.line_id, Outcome::Spoken, T0 + 1);
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), waiting).await.unwrap().unwrap().reason, Some(Reason::Timeout));

    // How long a call waits is its own to say, within what its caller may wait.
    assert_eq!(wait_of(None, 50).unwrap(), Duration::from_secs(45));
    assert_eq!(wait_of(Some(0.0), 50).unwrap(), Duration::ZERO);
    assert_eq!(wait_of(Some(50.0), 50).unwrap(), Duration::from_secs(50));
    assert_eq!(wait_of(Some(110.0), 110).unwrap(), Duration::from_secs(110));
    assert_eq!(wait_of(Some(51.0), 50).unwrap_err().message, "waitSeconds 51: 0 to 50");
    assert!(wait_of(Some(-1.0), 50).is_err() && wait_of(Some(f64::NAN), 50).is_err());
}

#[tokio::test]
async fn a_question_ends_when_it_is_dismissed_superseded_or_withdrawn() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);

    // Dismissed by the user.
    let (ask, waiter) = e.ask(&tid, "Shall I push?", &["Yes", "No"], T0).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    events.drain();
    dismiss_ask(&e.state, &tid, &ask.id, T0 + 1).unwrap();
    let dismissed = waiting.await.unwrap();
    assert_eq!((dismissed.answered, dismissed.reason, dismissed.ask_id.as_deref()), (false, Some(Reason::Dismissed), Some(ask.id.as_str())));
    assert!(dismissed.note.unwrap().contains("Do not ask it again"));
    let closed = events.drain();
    assert_eq!(kinds(&closed), ["avatar.ask"]);
    assert_eq!(closed[0].data, json!({ "terminalId": tid, "change": "dismissed", "ask": null }));
    assert!(e.avatar(&tid).ask.is_none());
    // What is not open cannot be dismissed, by an old id or any other.
    assert_eq!(dismiss_ask(&e.state, &tid, &ask.id, T0 + 2).unwrap_err().code, "not_pending");
    assert_eq!(dismiss_ask(&e.state, &tid, "Q", T0 + 2).unwrap_err().status.as_u16(), 409);

    // Superseded by another question of the session.
    e.heard(&tid, &ask.line_id, Outcome::Shown, T0 + 2);
    let (first, waiter) = e.ask(&tid, "Shall I push?", &["Yes", "No"], T0 + 10).unwrap();
    e.heard(&tid, &first.line_id, Outcome::Shown, T0 + 10);
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    events.drain();
    let (second, second_waiter) = e.ask(&tid, "Shall I push to main?", &["Yes", "No"], T0 + 11).unwrap();
    let superseded = waiting.await.unwrap();
    assert_eq!((superseded.reason, superseded.ask_id.as_deref()), (Some(Reason::Superseded), Some(first.id.as_str())));
    assert_ne!(first.id, second.id);
    let replaced = events.drain();
    assert_eq!(kinds(&replaced), ["avatar.ask", "avatar.say", "avatar.ask"]);
    assert_eq!(replaced[0].data, json!({ "terminalId": tid, "change": "superseded", "ask": null }));
    assert_eq!((replaced[1].data["ask"]["id"].as_str(), replaced[2].data["change"].as_str()), (Some(second.id.as_str()), Some("opened")));
    assert_eq!(e.avatar(&tid).ask.map(|a| a.id), Some(second.id.clone()));

    // Withdrawn by the session: the call that waits learns so, and so does the one that cancels.
    e.heard(&tid, &second.line_id, Outcome::Shown, T0 + 11);
    let waiting = tokio::spawn(wait_answer_within(second_waiter, LONG, LONG));
    events.drain();
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + 12).unwrap();
    let cancelled = ask_cancel(&e.state, &pass, T0 + 12).unwrap();
    assert_eq!(serde_json::to_value(&cancelled).unwrap(), json!({ "answered": false, "reason": "withdrawn", "askId": second.id, "note": "The question is withdrawn." }));
    assert_eq!(waiting.await.unwrap().reason, Some(Reason::Withdrawn));
    assert_eq!(events.drain()[0].data, json!({ "terminalId": tid, "change": "withdrawn", "ask": null }));
    let nothing = ask_cancel(&e.state, &pass, T0 + 13).unwrap_err();
    assert_eq!((nothing.status.as_u16(), nothing.code, nothing.message.as_str()), (409, "not_pending", "No question is open: there is nothing to withdraw."));

    // Turning the avatar off dismisses its question.
    let (_, waiter) = e.ask(&tid, "One more?", &[], T0 + 20).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    events.drain();
    turn_off(&e.state, &tid, T0 + 21).unwrap();
    assert_eq!(waiting.await.unwrap().reason, Some(Reason::Dismissed));
    // Its question is dismissed, and what waited to be presented is unheard: nothing is shown any more.
    assert_eq!(events.kinds(), ["avatar.ask", "avatar.line", "avatar.changed"]);
    assert_eq!(e.avatar(&tid).lines.last().unwrap().outcome, Some(Outcome::Unheard));
}

#[tokio::test]
async fn the_same_question_again_is_the_open_one_and_says_nothing_twice() {
    let e = env().await;
    let tid = e.with_avatar();
    let (ask, first) = e.ask(&tid, "Shall I delete the branch?", &["Delete", "Keep"], T0).unwrap();
    let before = e.avatar(&tid);
    let mut events = Events::of(&e);

    // Asked identically while a call still waits: that call is handed off, nothing is said.
    let (again, second) = e.ask(&tid, " Shall I delete\nthe branch? ", &["Delete", "Keep"], T0 + SECOND).unwrap();
    assert_eq!(again, ask);
    assert!(events.drain().is_empty(), "no second line, no second opening");
    let after = e.avatar(&tid);
    assert_eq!((after.lines.len(), after.ask.as_ref()), (before.lines.len(), Some(&ask)));
    let handed = wait_answer_within(first, LONG, LONG).await;
    assert_eq!((handed.answered, handed.reason), (false, Some(Reason::Timeout)), "for the earlier call the answer is not there yet");
    assert_eq!(e.avatar(&tid).ask.map(|a| a.waiting), Some(true), "the newer call waits on");

    // After the wait the question is asked again: it attaches, and browsers learn that a call waits.
    assert_eq!(wait_answer_within(second, Duration::ZERO, LONG).await.reason, Some(Reason::Timeout));
    assert_eq!(events.kinds(), ["avatar.ask"]);
    let (again, third) = e.ask(&tid, "Shall I delete the branch?", &["Delete", "Keep"], T0 + 2 * SECOND).unwrap();
    assert_eq!(again, ask);
    let waiting_again = events.drain();
    assert_eq!(kinds(&waiting_again), ["avatar.ask"]);
    assert_eq!(waiting_again[0].data, json!({ "terminalId": tid, "change": "waiting", "ask": ask }));
    assert_eq!(e.avatar(&tid).lines.len(), before.lines.len());

    // Other choices make it another question.
    let (other, _waiter) = e.ask(&tid, "Shall I delete the branch?", &["Delete", "Keep", "Ask later"], T0 + 3 * SECOND).unwrap();
    assert_ne!(other.id, ask.id);
    assert_eq!(wait_answer_within(third, LONG, LONG).await.reason, Some(Reason::Superseded));
    assert_eq!(e.avatar(&tid).lines.len(), before.lines.len() + 1);
}

#[tokio::test]
async fn a_call_waits_again_on_the_open_question_by_its_id() {
    let e = env().await;
    let tid = e.with_avatar();
    let ask = e.asked(&tid, "Which port?", &[], T0).await;
    e.heard(&tid, &ask.line_id, Outcome::Shown, T0 + 1);
    let mut events = Events::of(&e);

    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + 2).unwrap();
    let (same, waiter) = ask_again(&e.state, &pass, &ask.id, T0 + 2).unwrap();
    assert_eq!(same, Ask { waiting: true, ..ask.clone() });
    assert_eq!(events.drain()[0].data["change"], "waiting");
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    let replied = reply(&e.state, &tid, typed(Some(&ask.id), "8080"), T0 + 3).await.unwrap();
    assert_eq!(replied.delivered, Delivered::Answer);
    assert_eq!(serde_json::to_value(waiting.await.unwrap()).unwrap(), json!({ "answered": true, "text": "8080", "via": "typed" }));

    // An id that is not the open question's: nothing to wait for.
    let gone = ask_again(&e.state, &pass, &ask.id, T0 + 4).map(|_| ()).unwrap_err();
    assert_eq!((gone.status.as_u16(), gone.code), (409, "not_pending"));
    assert!(gone.message.starts_with(&format!("Question {} is not open any more", ask.id)), "{}", gone.message);
    assert!(ask_again(&e.state, &pass, "Q<script>", T0 + 4).map(|_| ()).unwrap_err().message.starts_with("Question \"Q<script>\" is not open"));

    // The three forms of the call's arguments, and one call from arguments to answer.
    assert_eq!(AskArgs::parse(&json!({ "askId": "Q1", "waitSeconds": 5 }), &unmasked).unwrap(), AskArgs { call: AskCall::Again("Q1".into()), wait_seconds: Some(5.0) });
    assert_eq!(AskArgs::parse(&json!({ "cancel": true }), &unmasked).unwrap().call, AskCall::Cancel);
    let new = AskArgs::parse(&json!({ "text": "Go on?", "choices": ["Yes", "No"], "freeText": false, "lang": "en-GB", "gesture": "nod" }), &unmasked).unwrap();
    assert_eq!(new.call, AskCall::New(AskRequest { text: "Go on?".into(), choices: vec!["Yes".into(), "No".into()], free_text: false, lang: Some("en-GB".into()), gesture: Some("nod".into()) }));
    assert_eq!((new.wait(50).unwrap(), new.wait(110).unwrap()), (Duration::from_secs(45), Duration::from_secs(45)));
    for (args, starts) in [
        (json!({}), "ask needs text"),
        (json!({ "cancel": true, "text": "x" }), "cancel goes alone"),
        (json!({ "askId": "Q1", "text": "x" }), "askId and text in one call"),
        (json!({ "text": "x", "choices": ["a", "b", "c", "d", "e"] }), "5 choices: at most 4"),
        (json!({ "text": "x", "choices": "a" }), "choices is a list of up to 4 short strings"),
        (json!({ "text": "x", "choices": [1] }), "choices is a list of up to 4 short strings"),
        (json!({ "text": "x", "waitSeconds": "long" }), "waitSeconds is a string: write a number"),
        (json!({ "text": "x", "timeout": 3 }), "ask has no timeout"),
    ] {
        let refused = AskArgs::parse(&args, &unmasked).unwrap_err();
        assert!(refused.code == "bad_request" && refused.message.starts_with(starts), "{args}: {}", refused.message);
    }
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + MINUTE).unwrap();
    let asked = ask_call(&e.state, &pass, AskArgs::parse(&json!({ "text": "Ready?", "waitSeconds": 0 }), &unmasked).unwrap(), 50, T0 + MINUTE).await.unwrap();
    assert_eq!((asked.answered, asked.reason), (false, Some(Reason::Timeout)));
    assert_eq!(ask_call(&e.state, &pass, AskArgs::parse(&json!({ "text": "Ready?", "waitSeconds": 51 }), &unmasked).unwrap(), 50, T0 + MINUTE).await.unwrap_err().message, "waitSeconds 51: 0 to 50");
    let withdrawn = ask_call(&e.state, &pass, AskArgs::parse(&json!({ "cancel": true }), &unmasked).unwrap(), 50, T0 + MINUTE).await.unwrap();
    assert_eq!((withdrawn.reason, withdrawn.ask_id), (Some(Reason::Withdrawn), asked.ask_id));
}

#[tokio::test]
async fn a_question_is_checked_whole_before_it_is_asked() {
    let e = env().await;
    let tid = e.with_avatar();
    e.state.terminals.get(&tid).unwrap().redact.lock().push(Secret::from_value("hunter2hunter2".into()));
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0).unwrap();
    let mut events = Events::of(&e);
    let refuse = |req: AskRequest| ask(&e.state, &pass, req, T0).map(|_| ()).unwrap_err();
    let choices = |list: &[&str]| question("Which?", list);

    assert_eq!(refuse(question(&"a".repeat(301), &[])).message, "the question has 301 characters: at most 300");
    assert!(refuse(question("\n", &[])).message.starts_with("the question is empty"));
    assert_eq!(refuse(choices(&["a", "b", "c", "d", "e"])).message, "5 choices: at most 4");
    assert_eq!(refuse(choices(&["fine", &"b".repeat(41)])).message, "choice 2 has 41 characters: at most 40");
    assert!(refuse(choices(&["fine", " "])).message.starts_with("choice 2 is empty"));
    assert_eq!(refuse(choices(&["Yes", "No", " Yes "])).message, "choice 3 repeats choice 1: give each answer once");
    assert!(refuse(AskRequest { free_text: false, ..question("Which?", &[]) }).message.starts_with("a question without choices needs a typed answer"));
    assert!(refuse(AskRequest { gesture: Some("wink".into()), ..question("Which?", &[]) }).message.starts_with("No animation wink."));
    assert!(refuse(AskRequest { lang: Some("x".into()), ..question("Which?", &[]) }).message.starts_with("lang x is not a language tag"));
    assert!(events.drain().is_empty() && e.avatar(&tid).ask.is_none() && e.avatar(&tid).lines.is_empty());

    // The question and its choices are plain text with the session's secrets masked.
    let (asked, _waiter) = ask(&e.state, &pass, AskRequest { free_text: false, ..question("Use hunter2hunter2?", &["Use hunter2hunter2", "No\tthanks"]) }, T0).unwrap();
    assert_eq!((asked.text.as_str(), asked.choices.as_slice(), asked.free_text), ("Use ••••••?", &["Use ••••••".to_string(), "No thanks".to_string()][..], false));
    // A question that takes only its choices takes no typed answer, and no choice it does not have.
    assert_eq!(reply(&e.state, &tid, typed(Some(&asked.id), "maybe"), T0 + 1).await.unwrap_err().message, "this question takes one of its choices");
    assert_eq!(reply(&e.state, &tid, clicked(&asked.id, 2), T0 + 1).await.unwrap_err().message, "choice 2: the question has 2 choices, counted from 0");
    assert_eq!(e.avatar(&tid).ask.map(|a| a.id), Some(asked.id));
}

#[tokio::test]
async fn a_call_that_goes_away_leaves_the_question_open_and_loses_no_answer() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut pastes = e.pastes();

    // The client went away: its task is aborted, the waiter is dropped. The question stays.
    let (ask, waiter) = e.ask(&tid, "Shall I continue?", &["Yes", "No"], T0).unwrap();
    let mut events = Events::of(&e);
    drop(waiter);
    let left = events.drain();
    assert_eq!(kinds(&left), ["avatar.ask"]);
    assert_eq!(left[0].data, json!({ "terminalId": tid, "change": "waiter_left", "ask": Ask { waiting: false, ..ask.clone() } }));
    // An answer now goes in as a prompt.
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, clicked(&ask.id, 0), T0 + SECOND)).await;
    assert_eq!(replied.unwrap().delivered, Delivered::Prompt);
    assert_eq!(prompt, format!("[via avatar, answer with avatar_say] Answer to question {}: choice 1, \"Yes\"", ask.id));

    // The same for a task that is aborted while it waits.
    let (ask, waiter) = e.ask(&tid, "And now?", &[], T0 + MINUTE).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    tokio::task::yield_now().await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(e.avatar(&tid).ask.map(|a| (a.id, a.waiting)), Some((ask.id.clone(), false)));

    // The race the other way: the reply took the call's sender first and the call is dropped
    // with the answer in its hands. The device was told `answer`; the answer still arrives.
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + MINUTE + 1).unwrap();
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, T0 + MINUTE + 1).unwrap();
    let replied = reply(&e.state, &tid, typed(Some(&ask.id), "go on"), T0 + MINUTE + 2).await.unwrap();
    assert_eq!(replied.delivered, Delivered::Answer);
    assert!(pastes.try_recv().is_err(), "nothing is pasted while the call may still take the answer");
    drop(waiter);
    let paste = tokio::time::timeout(Duration::from_secs(10), pastes.recv()).await.unwrap().unwrap();
    assert_eq!((paste.tid.as_str(), paste.prompt.as_str()), (tid.as_str(), format!("[via avatar, answer with avatar_say] Answer to question {}: go on", ask.id).as_str()));
    let _ = paste.done.send(Ok(()));
    assert!(e.avatar(&tid).ask.is_none());

    // And a call whose wait ends just as the reply got in returns the answer, not a timeout.
    let (ask, waiter) = e.ask(&tid, "Last one?", &[], T0 + 2 * MINUTE).unwrap();
    assert_eq!(reply(&e.state, &tid, typed(Some(&ask.id), "yes"), T0 + 2 * MINUTE + 1).await.unwrap().delivered, Delivered::Answer);
    let answer = wait_answer_within(waiter, Duration::ZERO, LONG).await;
    assert_eq!((answer.answered, answer.text.as_deref()), (true, Some("yes")));
    assert!(pastes.try_recv().is_err(), "the call has it: no prompt as well");
}

/// The races of a reply against the call that waits for it, run on several threads: whichever
/// gets the store lock first, the answer reaches the session exactly once, through the call or
/// as a prompt, and never both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_arrives_exactly_once_whoever_wins_the_race() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut pastes = e.pastes();
    let prompts: Arc<parking_lot::Mutex<Vec<String>>> = Arc::default();
    let seen = prompts.clone();
    tokio::spawn(async move {
        while let Some(paste) = pastes.recv().await {
            seen.lock().push(paste.prompt.clone());
            let _ = paste.done.send(Ok(()));
        }
    });

    for round in 0..240_i64 {
        // A minute apart keeps to the rate of questions.
        let now = T0 + round * MINUTE;
        let (asked, waiter) = e.ask(&tid, &format!("Question {round}?"), &[], now).unwrap();
        e.heard(&tid, &asked.line_id, Outcome::Shown, now);
        // The call and the reply start at the same moment on two threads, each after spinning
        // for some microseconds of its own, so that either may reach the store lock first. The
        // call goes away, gives up at once, or gives up a few microseconds in.
        let (ready, go) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let line_up = |ready: Arc<AtomicUsize>, go: Arc<AtomicBool>, micros: i64| {
            move || {
                ready.fetch_add(1, Ordering::SeqCst);
                while !go.load(Ordering::SeqCst) {
                    std::hint::spin_loop();
                }
                let since = std::time::Instant::now();
                while since.elapsed() < Duration::from_micros(micros as u64) {
                    std::hint::spin_loop();
                }
            }
        };
        let wait_for_start = line_up(ready.clone(), go.clone(), round % 7 * 3);
        let call = tokio::spawn(async move {
            wait_for_start();
            match round % 3 {
                0 => {
                    drop(waiter);
                    None
                }
                1 => Some(wait_answer_within(waiter, Duration::ZERO, LONG).await),
                _ => Some(wait_answer_within(waiter, Duration::from_micros((round as u64 * 7919) % 400 + 1), LONG).await),
            }
        });
        let (state, session, ask_id) = (e.state.clone(), tid.clone(), asked.id.clone());
        let wait_for_start = line_up(ready.clone(), go.clone(), round % 5 * 3);
        let answer = tokio::spawn(async move {
            wait_for_start();
            reply(&state, &session, typed(Some(&ask_id), "yes"), now + 1).await
        });
        while ready.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        go.store(true, Ordering::SeqCst);
        let (call, replied) = (call.await.unwrap(), answer.await.unwrap());
        let replied = replied.unwrap_or_else(|refused| panic!("round {round}: the reply was refused: {refused}"));
        let by_call = call.as_ref().is_some_and(|result| result.answered);
        if let Some(result) = &call {
            assert!(result.answered || result.reason == Some(Reason::Timeout), "round {round}: {result:?}");
        }
        // A call that went away with the answer in its hands pastes it by itself, a moment later.
        let prompt = format!("[via avatar, answer with avatar_say] Answer to question {}: yes", asked.id);
        let pasted = || prompts.lock().iter().filter(|p| **p == prompt).count();
        let until = tokio::time::Instant::now() + Duration::from_secs(20);
        while !by_call && pasted() == 0 {
            assert!(tokio::time::Instant::now() < until, "round {round}: the answer was lost (the device was told {:?})", replied.delivered);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(by_call as usize + pasted(), 1, "round {round}: delivered {:?}, the call {call:?}", replied.delivered);
        if replied.delivered == Delivered::Prompt {
            assert!(!by_call, "round {round}: pasted although the call had it");
        }
        let avatar = e.avatar(&tid);
        assert!(avatar.ask.is_none(), "round {round}: the question stayed open");
        assert_eq!(avatar.lines.last().map(|l| (l.from, l.ask_id.clone())), Some((Speaker::User, Some(asked.id.clone()))), "round {round}");
    }
    let all = prompts.lock().clone();
    assert_eq!(all.len(), all.iter().collect::<std::collections::HashSet<_>>().len(), "no prompt twice");
}

#[tokio::test]
async fn a_question_ends_with_its_session_or_when_the_user_goes_on_in_the_terminal() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);

    // The process ends while a call waits.
    let (ask, waiter) = e.ask(&tid, "Shall I go on?", &[], T0).unwrap();
    let pending = e.say(&tid, "Working.", T0 + 1).unwrap().line.id;
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    events.drain();
    e.change(&tid, |info| {
        info.status = TerminalStatus::Exited;
        info.agent.as_mut().unwrap().state = AgentState::Exited;
    });
    e.event("terminal.exited", &tid, T0 + 2 * SECOND);
    let ended = waiting.await.unwrap();
    assert_eq!((ended.reason, ended.ask_id.as_deref()), (Some(Reason::SessionEnded), Some(ask.id.as_str())));
    // The question is over, and what it said and nobody took in is unheard.
    let after = events.drain();
    assert_eq!(kinds(&after), ["avatar.ask", "avatar.line", "avatar.line"]);
    assert_eq!(after[0].data, json!({ "terminalId": tid, "change": "session_ended", "ask": null }));
    assert_eq!((after[2].data["line"]["id"].as_str(), after[2].data["line"]["outcome"].as_str()), (Some(pending.as_str()), Some("unheard")));
    let avatar = e.avatar(&tid);
    assert!(avatar.ask.is_none() && avatar.lines.iter().all(|l| l.outcome == Some(Outcome::Unheard)));
    assert_eq!(avatar.updated_at, T0 + 2 * SECOND);

    // The user answers in the terminal: the session goes from idle to working while its
    // question is open and no call waits.
    let tid = e.with_avatar();
    let ask = e.asked(&tid, "Shall I go on?", &[], T0).await;
    events.drain();
    e.goes(&tid, AgentState::Working, T0 + 10 * SECOND);
    assert_eq!(events.drain()[0].data, json!({ "terminalId": tid, "change": "moved_on", "ask": null }));
    assert!(e.avatar(&tid).ask.is_none());
    assert_eq!(reply(&e.state, &tid, typed(Some(&ask.id), "yes"), T0 + 11 * SECOND).await.unwrap_err().code, "not_pending");

    // Not while a call still waits for the answer: its session reads as working all along.
    let tid = e.with_avatar();
    let (_, waiter) = e.ask(&tid, "Shall I go on?", &[], T0).unwrap();
    e.goes(&tid, AgentState::Working, T0 + 10 * SECOND);
    e.goes(&tid, AgentState::Idle, T0 + 11 * SECOND);
    e.goes(&tid, AgentState::Working, T0 + 12 * SECOND);
    assert!(e.avatar(&tid).ask.is_some());
    // Nor for any other change of the session.
    assert_eq!(wait_answer_within(waiter, Duration::ZERO, LONG).await.reason, Some(Reason::Timeout));
    e.goes(&tid, AgentState::Idle, T0 + 13 * SECOND);
    e.goes(&tid, AgentState::Idle, T0 + 14 * SECOND);
    e.goes(&tid, AgentState::NeedsPermission, T0 + 15 * SECOND);
    e.goes(&tid, AgentState::Working, T0 + 16 * SECOND);
    assert!(e.avatar(&tid).ask.is_some());

    // Nor when a reply went in through the avatar a moment ago: that is what set it working.
    let tid = e.with_avatar();
    let mut pastes = e.pastes();
    let ask = e.asked(&tid, "Shall I go on?", &[], T0).await;
    let (replied, _) = pasted(&mut pastes, reply(&e.state, &tid, typed(None, "wait, show me the diff first"), T0 + 20 * SECOND)).await;
    assert_eq!(replied.unwrap().delivered, Delivered::Prompt);
    e.goes(&tid, AgentState::Working, T0 + 22 * SECOND);
    assert_eq!(e.avatar(&tid).ask.map(|a| a.id), Some(ask.id.clone()), "a message leaves the question open");
    e.goes(&tid, AgentState::Idle, T0 + 30 * SECOND);
    e.goes(&tid, AgentState::Working, T0 + 31 * SECOND);
    assert!(e.avatar(&tid).ask.is_none(), "later, the user did go on in the terminal");
}

// ---------------------------------------------------------------- reply binding

#[tokio::test]
async fn a_reply_answers_exactly_the_question_it_names_or_is_a_message() {
    let e = env().await;
    let tid = e.with_avatar();
    e.state.terminals.get(&tid).unwrap().redact.lock().push(Secret::from_value("hunter2hunter2".into()));
    let mut pastes = e.pastes();
    let old = e.asked(&tid, "Shall I push?", &["Yes", "No"], T0).await;
    let open = e.asked(&tid, "Shall I push to main?", &["Yes", "No"], T0 + SECOND).await;
    let lines = e.avatar(&tid).lines.len();
    let mut events = Events::of(&e);

    // A stale id: the question on screen changed under the user. Nothing is pasted or recorded.
    for stale in [clicked(&old.id, 0), typed(Some(&old.id), "yes"), typed(Some("Q"), "yes")] {
        let refused = reply(&e.state, &tid, stale, T0 + 2 * SECOND).await.unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code), (409, "not_pending"), "{}", refused.message);
    }
    assert!(pastes.try_recv().is_err() && events.drain().is_empty());
    assert_eq!((e.avatar(&tid).lines.len(), e.avatar(&tid).ask.map(|a| a.id)), (lines, Some(open.id.clone())));

    // Without an id the text is a message, also while a question is open: it never guesses.
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, typed(None, "Yes"), T0 + 3 * SECOND)).await;
    assert_eq!(replied.unwrap().delivered, Delivered::Prompt);
    assert_eq!(prompt, "[via avatar, answer with avatar_say] Yes");
    let sent = events.drain();
    assert_eq!(kinds(&sent), ["avatar.line"]);
    assert_eq!(sent[0].data["line"], json!({ "id": sent[0].data["line"]["id"], "at": sent[0].data["line"]["at"], "from": "user", "text": "Yes", "via": "typed" }));
    assert_eq!(e.avatar(&tid).ask.map(|a| a.id), Some(open.id.clone()), "the question is still open");

    // The session gets the user's own words; what is stored and broadcast is masked.
    let dictated = ReplyRequest { via: ReplyVia::Voice, ..typed(None, "the token is hunter2hunter2,\nuse it") };
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, dictated, T0 + 20 * SECOND)).await;
    replied.unwrap();
    assert_eq!(prompt, "[via avatar, dictated, answer with avatar_say] the token is hunter2hunter2, use it");
    let sent = events.drain();
    assert_eq!((sent[0].data["line"]["text"].as_str(), sent[0].data["line"]["via"].as_str()), (Some("the token is ••••••, use it"), Some("voice")));
    assert_eq!(e.avatar(&tid).lines.last().unwrap().text, "the token is ••••••, use it");

    // A session that drives its avatar with a command is told to answer with that.
    gate(&e.state, &tid, Call::Status, Via::Command, T0 + 21 * SECOND).unwrap();
    let (_, prompt) = pasted(&mut pastes, reply(&e.state, &tid, clicked(&open.id, 1), T0 + 40 * SECOND)).await;
    assert_eq!(prompt, format!("[via avatar, answer with your avatar say command] Answer to question {}: choice 2, \"No\"", open.id));

    // A reply is checked whole before anything happens.
    let (state, session) = (&e.state, tid.as_str());
    let refuse = |req: ReplyRequest| async move { reply(state, session, req, T0 + MINUTE).await.unwrap_err().message };
    assert_eq!(refuse(ReplyRequest { ask_id: None, text: None, choice: None, via: ReplyVia::Typed }).await, "a reply needs text, or the choice of an open question");
    assert_eq!(refuse(ReplyRequest { choice: Some(0), ..typed(None, "x") }).await, "a reply is text or a choice, not both");
    assert_eq!(refuse(ReplyRequest { ask_id: None, text: None, choice: Some(0), via: ReplyVia::Click }).await, "a choice answers a question: send its askId");
    assert_eq!(refuse(typed(None, " \n\u{200B} ")).await, "the reply is empty");
    assert_eq!(refuse(typed(None, &"a".repeat(MAX_REPLY_CHARS + 1))).await, "the reply has 4001 characters: at most 4000");
    assert!(pastes.try_recv().is_err());
    let no_avatar = e.agent(json!({}));
    assert_eq!(reply(&e.state, &no_avatar, typed(None, "hello"), T0).await.unwrap_err().status.as_u16(), 404);
    assert_eq!(reply(&e.state, "nope", typed(None, "hello"), T0).await.unwrap_err().status.as_u16(), 404);
    turn_off(&e.state, &tid, T0 + 2 * MINUTE).unwrap();
    assert_eq!(reply(&e.state, &tid, typed(None, "hello"), T0 + 2 * MINUTE).await.unwrap_err().code, "conflict");

    // The body of the route, read strictly.
    assert_eq!(ReplyRequest::parse(&json!({ "askId": "Q1", "choice": 1, "via": "click" })).unwrap(), clicked("Q1", 1));
    assert_eq!(ReplyRequest::parse(&json!({ "text": "hi", "via": "voice" })).unwrap(), ReplyRequest { via: ReplyVia::Voice, ..typed(None, "hi") });
    for (body, starts) in [
        (json!({ "text": "hi" }), "a reply needs via"),
        (json!({ "text": "hi", "via": "telepathy" }), "via telepathy: click, typed or voice"),
        (json!({ "askId": "Q1", "choice": -1, "via": "click" }), "choice -1"),
        (json!({ "askId": "Q1", "choice": 1.5, "via": "click" }), "choice 1.5"),
        (json!({ "askId": "Q1", "choice": "Yes", "via": "click" }), "choice is a string"),
        (json!({ "text": "hi", "via": "typed", "terminalId": "t" }), "a reply has no terminalId"),
    ] {
        assert!(ReplyRequest::parse(&body).unwrap_err().message.starts_with(starts), "{body}");
    }
}

#[tokio::test]
async fn while_a_reply_is_being_delivered_a_second_one_is_busy() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut pastes = e.pastes();
    let open = e.asked(&tid, "Shall I merge?", &["Merge", "Wait"], T0).await;
    let mut events = Events::of(&e);

    // The first reply is with the session: the ask path may take seconds (it waits for a pause in typing).
    let mut first = Box::pin(reply(&e.state, &tid, clicked(&open.id, 0), T0 + SECOND));
    let paste = tokio::select! {
        paste = pastes.recv() => paste.unwrap(),
        early = &mut first => panic!("the reply returned before its paste: {early:?}"),
    };
    // A double click, a second tab, a message: all wait their turn.
    for second in [clicked(&open.id, 0), clicked(&open.id, 1), typed(None, "hello?")] {
        let refused = reply(&e.state, &tid, second, T0 + SECOND + 1).await.unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code), (409, "busy"), "{}", refused.message);
    }
    // The question is neither open to the session nor closed meanwhile.
    assert_eq!(dismiss_ask(&e.state, &tid, &open.id, T0 + SECOND + 2).unwrap_err().code, "busy");
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + SECOND + 3).unwrap();
    let on_its_way = format!("The user's answer to question {} is being delivered to this session now: read it as the next message.", open.id);
    assert_eq!(ask_again(&e.state, &pass, &open.id, T0 + SECOND + 3).map(|_| ()).unwrap_err().message, on_its_way);
    assert_eq!(ask_cancel(&e.state, &pass, T0 + SECOND + 3).unwrap_err().code, "busy");
    assert_eq!(ask(&e.state, &pass, question("Another question?", &[]), T0 + SECOND + 3).map(|_| ()).unwrap_err().code, "busy");
    assert!(pastes.try_recv().is_err() && events.drain().is_empty());

    // The session refuses the paste (it opened a dialog): the refusal is passed on as it is,
    // nothing is recorded and the question is open again.
    let _ = paste.done.send(Err(ApiError::conflict("that session is showing a prompt (permission, question or trust); answer it first")));
    let refused = first.await.unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (409, "conflict", "that session is showing a prompt (permission, question or trust); answer it first"));
    assert!(events.drain().is_empty());
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.ask.map(|a| a.id), avatar.lines.iter().filter(|l| l.from == Speaker::User).count()), (Some(open.id.clone()), 0));

    // The next try goes through.
    let (replied, prompt) = pasted(&mut pastes, reply(&e.state, &tid, clicked(&open.id, 1), T0 + 10 * SECOND)).await;
    assert_eq!(replied.unwrap().delivered, Delivered::Prompt);
    assert!(prompt.ends_with("choice 2, \"Wait\""));
    assert_eq!(events.kinds(), ["avatar.line", "avatar.ask"]);

    // A request that is dropped while its reply is with the session leaves nothing stuck: the
    // paste's own task ends (here the paste never reached the session) and opens the question
    // again once it runs.
    let ask = e.asked(&tid, "And release?", &[], T0 + MINUTE).await;
    let mut dropped = Box::pin(reply(&e.state, &tid, typed(Some(&ask.id), "yes"), T0 + MINUTE + 1));
    let paste = tokio::select! {
        paste = pastes.recv() => paste.unwrap(),
        early = &mut dropped => panic!("the reply returned before its paste: {early:?}"),
    };
    drop(dropped);
    drop(paste);
    tokio::task::yield_now().await;
    let (replied, _) = pasted(&mut pastes, reply(&e.state, &tid, typed(Some(&ask.id), "yes"), T0 + MINUTE + 2)).await;
    assert_eq!(replied.unwrap().delivered, Delivered::Prompt);
}

#[tokio::test]
async fn a_reply_whose_request_goes_away_during_its_paste_is_still_recorded() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut pastes = e.pastes();
    let ask = e.asked(&tid, "Shall I release?", &["Release", "Wait"], T0).await;
    let mut events = Events::of(&e);
    let mut gone = Box::pin(reply(&e.state, &tid, typed(Some(&ask.id), "yes"), T0 + SECOND));
    let paste = tokio::select! {
        paste = pastes.recv() => paste.unwrap(),
        early = &mut gone => panic!("the reply returned before its paste: {early:?}"),
    };
    // The phone is locked while the answer stands in the session's input: the request goes
    // away, and the paste goes on to its end.
    drop(gone);
    let _ = paste.done.send(Ok(()));
    for _ in 0..100 {
        if e.avatar(&tid).ask.is_none() {
            break;
        }
        tokio::task::yield_now().await;
    }
    // What went in is recorded all the same: the question is answered, once.
    let avatar = e.avatar(&tid);
    assert!(avatar.ask.is_none());
    assert_eq!(avatar.lines.iter().filter(|l| l.from == Speaker::User && l.ask_id.as_deref() == Some(ask.id.as_str())).count(), 1);
    let closed = events.drain();
    assert_eq!(kinds(&closed), ["avatar.line", "avatar.ask"]);
    assert_eq!(closed[1].data["change"], "answered");
    // A second answer from another device finds nothing open, and nothing is pasted twice.
    let refused = reply(&e.state, &tid, clicked(&ask.id, 0), T0 + 2 * SECOND).await.unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code), (409, "not_pending"));
    assert!(pastes.try_recv().is_err());
}

#[tokio::test]
async fn what_the_ask_path_refuses_is_passed_on_as_it_is() {
    // No test stands in for the session here: the prompt goes through the terminals slice's own
    // ask path, in-process, and the stand-in session has no process to paste into.
    let e = env().await;
    let tid = e.with_avatar();
    let ask = e.asked(&tid, "Shall I go on?", &["Yes", "No"], T0).await;
    let mut events = Events::of(&e);
    for req in [clicked(&ask.id, 0), typed(Some(&ask.id), "yes"), typed(None, "hello")] {
        let refused = reply(&e.state, &tid, req, T0 + SECOND).await.unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code, refused.message.as_str()), (409, "conflict", "that session is not running; resume it first"));
    }
    // Nothing was recorded, the question is open, and the next reply is not held up.
    assert!(events.drain().is_empty());
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.ask.map(|a| a.id), avatar.lines.len()), (Some(ask.id.clone()), 1));
    // With a call waiting, the same click needs no session at all.
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, T0 + 2 * SECOND).unwrap();
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, T0 + 2 * SECOND).unwrap();
    assert_eq!(reply(&e.state, &tid, clicked(&ask.id, 1), T0 + 3 * SECOND).await.unwrap().delivered, Delivered::Answer);
    assert_eq!(wait_answer_within(waiter, LONG, LONG).await.choice, Some(1));
}

// ---------------------------------------------------------------- ids

#[tokio::test]
async fn ids_never_repeat_also_after_a_restart() {
    let e = env().await;
    let tid = e.with_avatar();
    let other = e.with_avatar();
    let first = e.say(&tid, "One.", T0).unwrap().line.id;
    let second = e.say(&other, "Two.", T0).unwrap().line.id;
    let (ask, _waiter) = e.ask(&tid, "Three?", &[], T0 + 1).unwrap();
    // `L<boot>-<n>` and `Q<boot>-<n>`: four base-36 characters, then a count that all avatars share.
    let boot = &first[1..5];
    assert!(boot.len() == 4 && boot.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()), "{first}");
    assert_eq!((first.as_str(), second.as_str(), ask.line_id.as_str(), ask.id.as_str()), (format!("L{boot}-1").as_str(), format!("L{boot}-2").as_str(), format!("L{boot}-3").as_str(), format!("Q{boot}-1").as_str()));

    // A new state counts from one again, as a server does after a restart: the boot stamp differs.
    e.state.avatar.restart();
    let again = e.say(&tid, "One again.", T0 + MINUTE).unwrap().line.id;
    let (ask_again, _waiter) = e.ask(&tid, "And again?", &[], T0 + MINUTE + 1).unwrap();
    assert!(again.ends_with("-1") && ask_again.id.ends_with("-1"));
    assert_ne!(&again[1..5], boot);
    assert!(again != first && ask_again.id != ask.id && ask_again.line_id != second);
    // Each state of one process has a stamp of its own.
    let mut boots = std::collections::HashSet::from([boot.to_string(), again[1..5].to_string()]);
    for _ in 0..20 {
        e.state.avatar.restart();
        let id = e.say(&other, "Once more.", T0 + 10 * MINUTE).unwrap().line.id;
        assert!(boots.insert(id[1..5].to_string()), "{id} repeats a stamp");
    }
}

// ---------------------------------------------------------------- leases and outcomes

#[tokio::test]
async fn one_tab_at_a_time_holds_the_lease_to_say_a_line() {
    let e = env().await;
    let tid = e.with_avatar();
    let line = e.say(&tid, &"a".repeat(150), T0).unwrap().line.id;
    // Its speech time is 10 s at 15 characters a second, and the lease 15 s more.
    let lease = 25 * SECOND;

    // The first tab to ask may speak.
    let won = e.claim(&tid, &line, "tab-A", T0 + 100);
    assert_eq!(serde_json::to_value(won).unwrap(), json!({ "speak": true, "leaseMs": lease, "retryMs": 0 }));
    // Another tab is told how long the line is taken.
    assert_eq!(serde_json::to_value(e.claim(&tid, &line, "tab-B", T0 + 1100)).unwrap(), json!({ "speak": false, "leaseMs": 0, "retryMs": lease - SECOND }));
    // The holder's next claim renews the lease from then.
    assert_eq!(e.claim(&tid, &line, "tab-A", T0 + 5100).lease_ms, lease);
    assert_eq!(e.claim(&tid, &line, "tab-B", T0 + 5100).retry_ms, lease);
    // A lapsed lease can be taken.
    assert_eq!(e.claim(&tid, &line, "tab-B", T0 + 5100 + lease - 1).retry_ms, 1);
    assert!(e.claim(&tid, &line, "tab-B", T0 + 5100 + lease).speak);
    assert!(!e.claim(&tid, &line, "tab-A", T0 + 5200 + lease).speak);
    // Giving it back: only the holder can, and then the other tab gets it.
    let give_back = |tab: &str, now: i64| claim(&e.state, &tid, &line, ClaimRequest { tab: tab.into(), release: true }, now).unwrap();
    assert_eq!(serde_json::to_value(give_back("tab-A", T0 + 5300 + lease)).unwrap(), json!({ "speak": false, "leaseMs": 0, "retryMs": 0 }));
    assert!(!e.claim(&tid, &line, "tab-A", T0 + 5400 + lease).speak, "another tab's release changes nothing");
    give_back("tab-B", T0 + 5500 + lease);
    assert!(e.claim(&tid, &line, "tab-A", T0 + 5600 + lease).speak);

    // A stale line is nobody's to say: staleness is the server's decision alone.
    let old = T0 + LINE_STALE_SECS * SECOND + 1;
    assert_eq!(serde_json::to_value(e.claim(&tid, &line, "tab-A", old)).unwrap(), json!({ "speak": false, "leaseMs": 0, "retryMs": 0 }));
    assert_eq!(e.claim(&tid, &line, "tab-C", old).retry_ms, 0);

    // The tab that is saying a line keeps it past that age: a long line of a slow voice is renewed
    // chunk by chunk and is not made unheard in its middle. Nobody else gets it, and the holder
    // gets no more time once the line is LINE_HELD_MAX_SECS old.
    let tid = e.with_avatar();
    let line = e.say(&tid, &"a".repeat(390), T0).unwrap().line.id;
    let lease = 41 * SECOND;
    assert!(e.claim(&tid, &line, "tab-A", T0 + 100).speak);
    assert_eq!(e.claim(&tid, &line, "tab-A", T0 + 30 * SECOND).lease_ms, lease);
    let late = T0 + LINE_STALE_SECS * SECOND + SECOND;
    assert_eq!(serde_json::to_value(e.claim(&tid, &line, "tab-A", late)).unwrap(), json!({ "speak": true, "leaseMs": lease, "retryMs": 0 }), "the holder renews a line older than the limit");
    assert_eq!(serde_json::to_value(e.claim(&tid, &line, "tab-B", late + 1)).unwrap(), json!({ "speak": false, "leaseMs": 0, "retryMs": 0 }), "it is nobody else's");
    sweep(&e.state, late + 2);
    assert_eq!(e.avatar(&tid).lines[0].outcome, None, "a line a tab still says is not made unheard");
    // It renews as it goes on speaking, each time before its lease runs out.
    let end = T0 + LINE_HELD_MAX_SECS * SECOND + 1;
    let mut at = late;
    while at + 30 * SECOND < end {
        at += 30 * SECOND;
        assert!(e.claim(&tid, &line, "tab-A", at).speak, "renewed {} s after the line", (at - T0) / SECOND);
    }
    assert!(e.claim(&tid, &line, "tab-A", end - 2).speak, "up to the longest a line may be held");
    assert_eq!(serde_json::to_value(e.claim(&tid, &line, "tab-A", end)).unwrap(), json!({ "speak": false, "leaseMs": 0, "retryMs": 0 }), "and no longer");
    // Its last lease runs out, and then the line is unheard like any other nobody acknowledged.
    sweep(&e.state, end + lease);
    assert_eq!(e.avatar(&tid).lines[0].outcome, Some(Outcome::Unheard));

    // D2: a line that so far was only shown as a caption (or hidden) can still be claimed and
    // spoken; one with a final outcome cannot.
    let tid = e.with_avatar();
    let line = e.say(&tid, "Hello.", T0).unwrap().line.id;
    e.heard(&tid, &line, Outcome::Hidden, T0 + 5);
    e.heard(&tid, &line, Outcome::Shown, T0 + 10);
    assert_eq!(serde_json::to_value(e.claim(&tid, &line, "voice-tab", T0 + 600)).unwrap(), json!({ "speak": true, "leaseMs": 15 * SECOND + 400, "retryMs": 0 }));
    heard(&e.state, &tid, &line, HeardRequest { outcome: Outcome::Spoken, voice: Some("Vox".into()) }, T0 + 2 * SECOND).unwrap();
    for (i, done) in [Outcome::Spoken, Outcome::Skipped, Outcome::Failed].into_iter().enumerate() {
        let line = e.say(&tid, "And more.", T0 + 3 * SECOND + i as i64).unwrap().line.id;
        e.heard(&tid, &line, done, T0 + 4 * SECOND);
        assert_eq!(serde_json::to_value(e.claim(&tid, &line, "voice-tab", T0 + 5 * SECOND)).unwrap(), json!({ "speak": false, "leaseMs": 0, "retryMs": 0 }), "{done:?}");
    }
    // A final outcome ends the lease, so nobody waits for a line that is over.
    let line = e.say(&tid, "The last.", T0 + 10 * SECOND).unwrap().line.id;
    assert!(e.claim(&tid, &line, "tab-A", T0 + 10 * SECOND).speak);
    e.heard(&tid, &line, Outcome::Skipped, T0 + 11 * SECOND);
    assert_eq!(e.claim(&tid, &line, "tab-B", T0 + 11 * SECOND).retry_ms, 0);

    // What a claim is not for.
    let refuse = |line: &str, tab: &str| claim(&e.state, &tid, line, ClaimRequest { tab: tab.into(), release: false }, T0).unwrap_err();
    assert_eq!(refuse("L0000-99", "tab-A").status.as_u16(), 404);
    for tab in ["", "tab A", "tab/../x", &"t".repeat(65)] {
        assert!(refuse(&line, tab).message.starts_with("tab is the id of the browser tab that asks: 1 to 64 letters, digits, _ or -"), "{tab:?}");
    }
    assert!(claim(&e.state, &tid, &line, ClaimRequest { tab: "t".repeat(64), release: true }, T0).is_ok());
    let user_line = {
        let mut pastes = e.pastes();
        pasted(&mut pastes, reply(&e.state, &tid, typed(None, "hello"), T0)).await.0.unwrap();
        e.avatar(&tid).lines.last().unwrap().id.clone()
    };
    assert_eq!(refuse(&user_line, "tab-A").status.as_u16(), 404, "only the session's lines are spoken");
    let bare = e.agent(json!({}));
    assert_eq!(claim(&e.state, &bare, &line, ClaimRequest { tab: "tab-A".into(), release: false }, T0).unwrap_err().status.as_u16(), 404);
    // The body of the route.
    assert_eq!(ClaimRequest::parse(&json!({ "tab": "abc", "release": true })).unwrap(), ClaimRequest { tab: "abc".into(), release: true });
    assert!(!ClaimRequest::parse(&json!({ "tab": "abc" })).unwrap().release);
    assert!(ClaimRequest::parse(&json!({})).unwrap_err().message.starts_with("a claim needs tab"));
    // An avatar that is off is not spoken.
    let line = e.say(&tid, "Before it is off.", T0 + MINUTE).unwrap().line.id;
    turn_off(&e.state, &tid, T0 + MINUTE).unwrap();
    assert!(!e.claim(&tid, &line, "tab-A", T0 + MINUTE).speak);
}

#[tokio::test]
async fn a_line_keeps_the_best_outcome_it_got() {
    let e = env().await;
    let tid = e.with_avatar();
    let line = e.say(&tid, "The tests pass.", T0).unwrap().line.id;
    let mut events = Events::of(&e);
    let outcome = || e.avatar(&tid).lines[0].outcome;
    let report = |outcome: Outcome, voice: Option<&str>, now: i64| heard(&e.state, &tid, &line, HeardRequest { outcome, voice: voice.map(str::to_string) }, now).unwrap();

    // spoken > skipped > failed > shown > hidden > unheard: each better one is recorded and relayed.
    report(Outcome::Hidden, None, T0 + 1);
    assert_eq!(outcome(), Some(Outcome::Hidden));
    report(Outcome::Shown, None, T0 + 2);
    report(Outcome::Hidden, None, T0 + 3);
    report(Outcome::Shown, None, T0 + 3);
    assert_eq!(outcome(), Some(Outcome::Shown));
    report(Outcome::Failed, Some("Vox"), T0 + 4);
    assert_eq!((outcome(), e.avatar(&tid).lines[0].voice.clone()), (Some(Outcome::Failed), None), "only a voice that spoke it is named");
    report(Outcome::Skipped, None, T0 + 5);
    report(Outcome::Failed, None, T0 + 6);
    report(Outcome::Shown, None, T0 + 6);
    assert_eq!(outcome(), Some(Outcome::Skipped));
    report(Outcome::Spoken, Some("  Google UK\tEnglish Female\u{202E} "), T0 + 7);
    report(Outcome::Skipped, None, T0 + 8);
    let changed = events.drain();
    assert_eq!(kinds(&changed), ["avatar.line"; 5], "a line that did not change is not announced");
    assert_eq!(changed[4].data, json!({ "terminalId": tid, "line": { "id": line, "at": T0, "from": "agent", "text": "The tests pass.", "outcome": "spoken", "voice": "Google UK English Female" } }));
    assert_eq!(changed[4].project_id.as_deref(), Some(e.pid.as_str()));
    assert_eq!(e.avatar(&tid).updated_at, T0 + 7);

    // `unheard` is the lowest: a stale line the user asks to hear after all is recorded as spoken.
    let stale = e.say(&tid, "Nobody was here.", T0 + 10).unwrap().line.id;
    sweep(&e.state, T0 + 10 + LINE_STALE_SECS * SECOND + 1);
    assert_eq!(e.avatar(&tid).lines[1].outcome, Some(Outcome::Unheard));
    heard(&e.state, &tid, &stale, HeardRequest { outcome: Outcome::Spoken, voice: Some("Vox".into()) }, T0 + 3 * MINUTE).unwrap();
    assert_eq!((e.avatar(&tid).lines[1].outcome, e.avatar(&tid).lines[1].voice.as_deref()), (Some(Outcome::Spoken), Some("Vox")));

    // What a device may report, and of which lines.
    let refuse = |line: &str, req: HeardRequest| heard(&e.state, &tid, line, req, T0).unwrap_err();
    assert_eq!(refuse(&line, HeardRequest { outcome: Outcome::Unheard, voice: None }).status.as_u16(), 400);
    assert_eq!(refuse("L0000-99", HeardRequest { outcome: Outcome::Shown, voice: None }).status.as_u16(), 404);
    assert_eq!(refuse(&line, HeardRequest { outcome: Outcome::Spoken, voice: Some("v".repeat(81)) }).message, "the voice name has 81 characters: at most 80");
    assert_eq!(HeardRequest::parse(&json!({ "outcome": "spoken", "voice": "Vox" })).unwrap(), HeardRequest { outcome: Outcome::Spoken, voice: Some("Vox".into()) });
    for known in ["spoken", "skipped", "failed", "shown", "hidden"] {
        assert_eq!(HeardRequest::parse(&json!({ "outcome": known })).unwrap().outcome.name(), known);
    }
    assert_eq!(HeardRequest::parse(&json!({ "outcome": "unheard" })).unwrap_err().message, "outcome unheard: spoken, skipped, failed, shown or hidden");
    assert!(HeardRequest::parse(&json!({})).unwrap_err().message.starts_with("an acknowledgement needs outcome"));
    assert_eq!(serde_json::to_value(Outcome::REPORTED).unwrap(), json!(["spoken", "skipped", "failed", "shown", "hidden"]));
}

#[tokio::test]
async fn say_with_wait_returns_what_became_of_the_line() {
    let e = env().await;
    let tid = e.with_avatar();
    let quick = Duration::from_millis(30);
    let patient = SayWait { soft: LONG, most: LONG, held: LONG };

    // A device says it was spoken: the call returns at once, with the voice.
    let said = e.say(&tid, "The build is green.", T0).unwrap();
    let line = said.line.id.clone();
    let waiting = say_wait_within(&e.state, said, patient);
    let spoken = async { heard(&e.state, &tid, &line, HeardRequest { outcome: Outcome::Spoken, voice: Some("Vox".into()) }, T0 + SECOND).unwrap() };
    let (answer, ()) = tokio::join!(waiting, spoken);
    assert_eq!(serde_json::to_value(&answer).unwrap(), json!({ "lineId": line, "outcome": "spoken", "voice": "Vox", "note": format!("{line} spoken aloud by Vox.") }));

    // Only a caption, and no tab is about to speak it: a moment later the call returns.
    let said = e.say(&tid, "Tests next.", T0 + 2 * SECOND).unwrap();
    let line = said.line.id.clone();
    e.heard(&tid, &line, Outcome::Shown, T0 + 2 * SECOND);
    let answer = say_wait_within(&e.state, said, SayWait { soft: quick, most: LONG, held: LONG }).await;
    assert_eq!((answer.outcome, answer.voice.as_deref()), (Some(Outcome::Shown), None));
    // The earlier line was told in its own answer: it is not told again.
    assert_eq!(answer.note, format!("{line} shown as a caption, not spoken."));

    // A caption on one tab while another holds the line to speak it: the call waits for that.
    let said = e.say(&tid, "Now the linter.", T0 + 4 * SECOND).unwrap();
    let line = said.line.id.clone();
    assert!(e.claim(&tid, &line, "voice-tab", T0 + 4 * SECOND).speak);
    e.heard(&tid, &line, Outcome::Shown, T0 + 4 * SECOND);
    let mut waiting = Box::pin(say_wait_within(&e.state, said, SayWait { soft: quick, most: LONG, held: LONG }));
    assert!(tokio::time::timeout(Duration::from_millis(150), &mut waiting).await.is_err(), "it waits while the lease is held");
    e.heard(&tid, &line, Outcome::Skipped, T0 + 5 * SECOND);
    let answer = tokio::time::timeout(Duration::from_secs(10), waiting).await.unwrap();
    assert_eq!((answer.outcome, answer.note.as_str()), (Some(Outcome::Skipped), format!("{line} skipped by the user.").as_str()));

    // Nobody acknowledges it: at the timeout the answer is unheard, and says to stop.
    let said = e.say(&tid, "Is anyone there?", T0 + 6 * SECOND).unwrap();
    let line = said.line.id.clone();
    let answer = say_wait_within(&e.state, said, SayWait { soft: LONG, most: quick, held: LONG }).await;
    assert_eq!(answer.outcome, Some(Outcome::Unheard));
    assert_eq!(answer.note, format!("{line} not heard: nobody had this project open in Workbench: stop speaking until the user writes."));
    assert_eq!(e.avatar(&tid).lines.last().unwrap().outcome, None, "the line itself may still be presented");
    // The hidden avatars are a soft outcome like a caption; at the timeout the best known is the answer.
    let said = e.say(&tid, "Hidden?", T0 + 8 * SECOND).unwrap();
    e.heard(&tid, &said.line.id, Outcome::Hidden, T0 + 8 * SECOND);
    let answer = say_wait_within(&e.state, said, SayWait { soft: LONG, most: quick, held: LONG }).await;
    assert!(answer.outcome == Some(Outcome::Hidden) && answer.note.ends_with("not shown: the user has hidden the avatars."), "{}", answer.note);

    // A tab holds the line and has said nothing of it yet: it waits behind a long line, or is
    // being spoken. Someone is there: the call waits for it past its timeout, and says what it became.
    let at = T0 + 2 * MINUTE;
    let said = e.say(&tid, "Short.", at).unwrap();
    let line = said.line.id.clone();
    assert!(e.claim(&tid, &line, "voice-tab", at).speak);
    let mut waiting = Box::pin(say_wait_within(&e.state, said, SayWait { soft: LONG, most: quick, held: LONG }));
    assert!(tokio::time::timeout(Duration::from_millis(150), &mut waiting).await.is_err(), "it waits while a tab holds the line");
    e.heard(&tid, &line, Outcome::Spoken, at + SECOND);
    let answer = tokio::time::timeout(Duration::from_secs(10), waiting).await.unwrap();
    assert_eq!((answer.outcome, answer.note.as_str()), (Some(Outcome::Spoken), format!("{line} spoken aloud.").as_str()));
    // Still held when the longest wait is over: the line is waiting to be presented, which is no
    // reason to stop speaking.
    let said = e.say(&tid, "Still there?", at + 2 * SECOND).unwrap();
    let line = said.line.id.clone();
    assert!(e.claim(&tid, &line, "voice-tab", at + 2 * SECOND).speak);
    let answer = tokio::time::timeout(Duration::from_secs(10), say_wait_within(&e.state, said, SayWait { soft: LONG, most: quick, held: 3 * quick })).await.unwrap();
    assert_eq!((answer.outcome, answer.note.as_str()), (None, format!("{line} is waiting to be presented.").as_str()));

    // How long it waits: the line's speech time and ten seconds, within a tool call's longest wait.
    assert_eq!(SayWait::of(30), SayWait { soft: Duration::from_millis(1500), most: Duration::from_secs(12), held: Duration::from_secs(50) });
    assert_eq!(SayWait::of(MAX_SAY_CHARS).most, Duration::from_secs(36));
    assert_eq!(SayWait::of(100_000).most, Duration::from_secs(50));
    // One call from request to answer, without and with the wait.
    let tid = e.with_avatar();
    let pass = gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap();
    let queued = say_call(&e.state, &pass, SayRequest { text: "Hello.".into(), lang: None, gesture: None, wait: false }, T0).await.unwrap();
    assert_eq!(serde_json::to_value(&queued).unwrap(), json!({ "lineId": queued.line_id, "note": format!("Queued ({}).", queued.line_id) }));
}

// ---------------------------------------------------------------- design

#[tokio::test]
async fn a_session_designs_its_avatar_and_reads_what_it_drew() {
    let e = env().await;
    let tid = e.with_avatar();
    e.state.terminals.get(&tid).unwrap().redact.lock().push(Secret::from_value("hunter2hunter2".into()));
    let mut events = Events::of(&e);
    assert_eq!(carried(&e.state, &tid), Some(Carried { on: true, designed: false }));

    // A name alone is not a design: the look stays the default one.
    let named = e.design(&tid, json!({ "name": " Agent hunter2hunter2 " }), T0).unwrap();
    assert_eq!(named.text, "Saved as revision 1, named Agent ••••••.\nThe look is the default one: send svg to draw your own.");
    assert_eq!((named.revision, e.avatar(&tid).designed, e.avatar(&tid).name.as_str()), (1, false, "Agent ••••••"));
    assert_eq!(events.drain()[0].data, json!({ "terminalId": tid, "revision": 1, "on": true }));

    // The artwork with what moves it. The session cannot see it: the answer describes it.
    let drawn = e.design(&tid, figure(), T0 + 1).unwrap();
    assert_eq!(drawn.revision, 2);
    let answer: Vec<&str> = drawn.text.lines().collect();
    assert_eq!(answer[0], "Saved as revision 2, named Dot.");
    assert_eq!(answer[1], "Parts (id tag x y w h pivot x y, in root coordinates, at rest):");
    assert_eq!(&answer[2..7], ["body g 60 60 92 80 pivot 100 140", "face circle 60 60 80 80 pivot 100 100 (default)", "eyes g 86 86 8 8 pivot 90 90 (default)", "pupil circle 86 86 8 8 pivot 90 90 (default)", "arm rect 140 100 12 40 pivot 146 100"]);
    assert_eq!(answer[7], "Your animations: blink, fade, hop, spin.");
    assert!(answer[8].starts_with("Usage: svg 0.") && answer[8].ends_with(" of 32 KiB, 5 of 300 elements, 4 of 32 animations."), "{}", answer[8]);
    assert_eq!(answer[9], "Built-in motion that applies: breathe, talk, listen, alert, doze, wave, cheer, nod, shake.");
    assert_eq!(answer[10], "Warnings:");
    assert!(answer.len() > 11, "{}", drawn.text);
    let avatar = e.avatar(&tid);
    assert!(avatar.designed && avatar.look.is_some());
    assert_eq!((avatar.name.as_str(), avatar.revision, avatar.updated_at, serde_json::to_value(&avatar.voice).unwrap()), ("Dot", 2, T0 + 1, json!({ "lang": "en-GB", "pitch": 1.1 })));
    assert_eq!(events.drain()[0].data, json!({ "terminalId": tid, "revision": 2, "on": true }));
    assert_eq!((active(&e.state, &tid), carried(&e.state, &tid)), (true, Some(Carried { on: true, designed: true })));

    // The file holds what a restart keeps.
    let stored: Value = serde_json::from_slice(&std::fs::read(e.file(&tid)).unwrap()).unwrap();
    assert_eq!(stored.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["designed", "look", "name", "on", "version", "via", "voice"]);
    assert_eq!((&stored["version"], &stored["on"], &stored["name"], &stored["designed"], &stored["via"]), (&json!(1), &json!(true), &json!("Dot"), &json!(true), &json!("tools")));
    assert_eq!(stored["look"], serde_json::to_value(&avatar.look).unwrap());

    // A refused call lists what to fix and changes nothing.
    let refused = e.design(&tid, json!({ "svg": "<svg viewBox='0 0 200 200'><script>alert(1)</script><circle id='c' r='5' onclick='x()'/></svg>", "name": "Mallory" }), T0 + 2).unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code), (400, "bad_request"));
    assert!(refused.message.contains("<script>") && refused.message.ends_with("\nNothing was changed."), "{}", refused.message);
    assert!(events.drain().is_empty());
    assert_eq!(e.avatar(&tid), avatar);
    assert_eq!(e.design(&tid, json!({ "svg": "x".repeat(300 * 1024) }), T0 + 2).unwrap_err().message, "the design is 300.0 KiB of JSON: at most 256 KiB\nNothing was changed.");

    // A call without svg merges; the same design again is no change.
    let merged = e.design(&tid, json!({ "animations": { "hop": null }, "voice": { "lang": "de" } }), T0 + 3).unwrap();
    assert!(merged.text.starts_with("Saved as revision 3, named Dot.") && merged.text.contains("Your animations: blink, fade, spin."), "{}", merged.text);
    let same = e.design(&tid, json!({ "voice": { "lang": "de" } }), T0 + 4).unwrap();
    assert!(same.text.starts_with("Nothing to save: the avatar already has this design. It is revision 3, named Dot."), "{}", same.text);
    assert_eq!((same.revision, e.avatar(&tid).updated_at, events.kinds()), (3, T0 + 3, vec!["avatar.changed".to_string()]));

    // New artwork clears what moved the old one, unless it is kept: the answer says which.
    let later = T0 + 2 * MINUTE;
    let redrawn = e.design(&tid, json!({ "svg": "<svg viewBox='0 0 200 200'><g id='body'><circle id='face' cx='100' cy='100' r='50' fill='#00aaff'/></g></svg>", "keep": true }), later).unwrap();
    assert!(redrawn.text.contains("\nKept: ") && redrawn.text.contains("\nCleared: "), "{}", redrawn.text);

    // What is not drawn is named first among the warnings.
    let dropped = e.design(&tid, json!({ "svg": "<svg viewBox='0 0 200 200'><title>Dot</title><g id='body' class='shiny'><circle id='face' cx='100' cy='100' r='50' fill='#00aaff' font-family='serif'/></g></svg>" }), later).unwrap();
    let lines: Vec<&str> = dropped.text.lines().collect();
    let warnings = lines.iter().position(|line| *line == "Warnings:").expect("warnings");
    assert!(lines[warnings + 1].starts_with("Not drawn: ") && lines[warnings + 1].contains("font-family") && lines[warnings + 1].contains("title"), "{}", dropped.text);
    assert_eq!(dropped.revision, 5);

    // The user takes the look away: the default one is back, name and voice stay.
    let reset_to = reset(&e.state, &tid, later + 1).unwrap();
    assert!(reset_to.look.is_none() && !reset_to.designed && reset_to.on);
    assert_eq!((reset_to.revision, reset_to.name.as_str(), reset_to.voice.lang.as_deref(), reset_to.updated_at), (6, "Dot", Some("de"), later + 1));
    assert_eq!(events.drain().last().unwrap().data, json!({ "terminalId": tid, "revision": 6, "on": true }));
    assert_eq!(carried(&e.state, &tid), Some(Carried { on: true, designed: false }));
    let stored: Value = serde_json::from_slice(&std::fs::read(e.file(&tid)).unwrap()).unwrap();
    assert_eq!((&stored["look"], &stored["designed"], &stored["name"]), (&Value::Null, &json!(false), &json!("Dot")));
    // Again changes nothing; a session without an avatar has nothing to reset.
    assert_eq!(reset(&e.state, &tid, later + 2).unwrap().revision, 6);
    assert!(events.drain().is_empty());
    assert_eq!(reset(&e.state, &e.agent(json!({})), T0).unwrap_err().status.as_u16(), 404);
    assert_eq!(turn_off(&e.state, &e.agent(json!({})), T0).unwrap_err().status.as_u16(), 404);
}

#[tokio::test]
async fn status_begins_with_what_to_do_and_describes_the_avatar() {
    let e = env().await;
    let tid = e.with_avatar();
    e.design(&tid, figure(), T0).unwrap();
    let said = e.say(&tid, "Ready.", T0 + 1).unwrap().line.id;
    heard(&e.state, &tid, &said, HeardRequest { outcome: Outcome::Spoken, voice: Some("Vox".into()) }, T0 + 2).unwrap();
    let ask = e.asked(&tid, "Start?", &["Yes", "No"], T0 + 3).await;
    e.act(&tid, ActRequest { play: Some("spin".into()), mode: Some(ActMode::Loop), ..Default::default() }, T0 + 4).unwrap();
    let mut pastes = e.pastes();
    pasted(&mut pastes, reply(&e.state, &tid, typed(None, "one moment"), T0 + 5)).await.0.unwrap();

    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, T0 + 6).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    // The standing instructions come first in the text a session reads.
    let text = report.text();
    assert!(text.starts_with("{\n  \"note\": \"You have a live avatar: say the short version with avatar_say, ask with avatar_ask.\",\n  \"allowed\": true,\n  \"avatar\": {"), "{text}");
    let json: Value = serde_json::from_str(&text).unwrap();
    let avatar = &json["avatar"];
    assert_eq!((&avatar["on"], &avatar["name"], &avatar["designed"], &avatar["revision"], &avatar["via"]), (&json!(true), &json!("Dot"), &json!(true), &json!(1), &json!("tools")));
    assert_eq!(avatar["parts"][0], "body g 60 60 92 80 pivot 100 140");
    assert_eq!(avatar["parts"].as_array().unwrap().len(), 5);
    assert_eq!(avatar["animations"], json!(["blink", "fade", "hop", "spin"]));
    assert_eq!(avatar["builtIn"], json!(["breathe", "talk", "listen", "alert", "doze", "wave", "cheer", "nod", "shake"]));
    assert_eq!(avatar["voice"], json!({ "lang": "en-GB", "pitch": 1.1 }));
    assert_eq!(avatar["held"], json!({ "name": "spin", "mode": "loop" }));
    assert_eq!(avatar["ask"], json!({ "askId": ask.id, "text": "Start?", "choices": ["Yes", "No"], "freeText": true, "waiting": false }));
    // The last lines with what became of them.
    assert_eq!(
        avatar["lines"],
        json!([
            { "id": said, "from": "agent", "text": "Ready.", "became": "spoken aloud by Vox" },
            { "id": ask.line_id, "from": "agent", "text": "Start?", "became": "is waiting to be presented" },
            { "id": avatar["lines"][2]["id"], "from": "user", "text": "one moment", "via": "typed" },
        ])
    );
    assert!(avatar.get("design").is_none());
    assert_eq!((&json["limits"]["sayChars"], &json["limits"]["linesAndQuestionsPerMinute"], &json["limits"]["askCallsPerMinute"], &json["limits"]["waitSecondsTool"], &json["limits"]["svgBytes"]), (&json!(400), &json!(6), &json!(30), &json!(50), &json!(32768)));

    // With look the stored design comes back in the form of a design call, the artwork as SVG:
    // sent again it is the same design.
    let full = status(&e.state, &pass, StatusRequest::parse(&json!({ "look": true, "guide": true }), &unmasked).unwrap()).unwrap();
    let stored = full.avatar.unwrap().design.unwrap();
    assert!(stored["svg"].as_str().unwrap().starts_with("<svg viewBox='0 0 200 200'><g id='body'>"));
    assert_eq!(stored.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["animations", "gaze", "mouth", "name", "pivots", "roles", "states", "svg", "voice"]);
    let again = e.design(&tid, stored, T0 + 7).unwrap();
    assert!(again.text.starts_with("Nothing to save"), "{}", again.text);
    assert_eq!(StatusRequest::parse(&json!({}), &unmasked).unwrap(), StatusRequest { guide: false, look: false });
    assert!(StatusRequest::parse(&json!({ "look": "yes" }), &unmasked).unwrap_err().message.starts_with("look is a string: write true or false"));

    // A session that drives its avatar with a command reads of its command.
    let pass = gate(&e.state, &tid, Call::Status, Via::Command, T0 + 8).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert_eq!(report.note, "You have a live avatar: say the short version with your avatar say command, ask with your avatar ask command.");
    // The default look has every built-in, no parts to report and no animations of its own.
    let plain = e.with_avatar();
    let pass = gate(&e.state, &plain, Call::Status, Via::Tools, T0).unwrap();
    let report = status(&e.state, &pass, StatusRequest { guide: false, look: true }).unwrap().avatar.unwrap();
    assert!(!report.designed && report.parts.is_empty() && report.animations.is_empty() && report.built_in.len() == 11 && report.ask.is_none() && report.held.is_none());
    assert_eq!(report.design, Some(json!({ "name": "", "voice": {} })));
}

// ---------------------------------------------------------------- act

#[tokio::test]
async fn a_gesture_plays_by_name_in_one_of_three_modes() {
    let e = env().await;
    let tid = e.with_avatar();
    e.design(&tid, figure(), T0).unwrap();
    let mut events = Events::of(&e);
    // Three seconds between calls keeps to the rate of gestures.
    let clock = std::cell::Cell::new(T0);
    let go = |req: ActRequest| {
        clock.set(clock.get() + 3 * SECOND);
        e.act(&tid, req, clock.get())
    };
    let mode = |name: &str, mode: ActMode| ActRequest { play: Some(name.into()), mode: Some(mode), ..Default::default() };
    let times = |name: &str, times: u32| ActRequest { play: Some(name.into()), times: Some(times), ..Default::default() };
    let stop = |name: &str| ActRequest { stop: Some(name.into()), ..Default::default() };

    // Once is the default; an animation of the look's own or built-in motion its roles allow.
    assert_eq!(go(play("hop")).unwrap(), "Playing hop.");
    assert_eq!(go(times("wave", 3)).unwrap(), "Playing wave 3 times.");
    let played = events.drain();
    assert_eq!(kinds(&played), ["avatar.act", "avatar.act"]);
    assert_eq!(played[0].data, json!({ "terminalId": tid, "play": "hop", "mode": "once", "times": 1, "seq": 1 }));
    assert_eq!(played[1].data, json!({ "terminalId": tid, "play": "wave", "mode": "once", "times": 3, "seq": 2 }));
    assert_eq!(played[0].project_id.as_deref(), Some(e.pid.as_str()));
    assert!(e.avatar(&tid).held.is_none());

    // A name the avatar does not have: both lists come back.
    let unknown = go(play("wink")).unwrap_err();
    assert_eq!((unknown.status.as_u16(), unknown.message.as_str()), (400, "No animation wink. Yours: blink, fade, hop, spin; built in: breathe, talk, listen, alert, doze, wave, cheer, nod, shake"));
    assert!(go(play("think")).unwrap_err().message.starts_with("No animation think."), "its roles do not allow it");
    assert!(go(play("constructor")).unwrap_err().message.starts_with("No animation constructor."));
    assert!(go(stop("wink")).unwrap_err().message.starts_with("No animation wink."));
    assert!(go(play("<img src=x onerror=alert(1)>")).unwrap_err().message.starts_with("No animation \"<img src=x onerror=alert(1)>\"."));

    // Hold keeps the last frame, loop repeats an animation that loops; both are in `held`.
    assert_eq!(go(mode("hop", ActMode::Hold)).unwrap(), "Holding hop until you stop it, play another or 2 minutes pass.");
    assert_eq!(e.avatar(&tid).held.map(|h| (h.name, h.mode, h.since)), Some(("hop".to_string(), HeldMode::Hold, clock.get())));
    assert_eq!(go(mode("spin", ActMode::Loop)).unwrap(), "Looping spin until you stop it, play another or 2 minutes pass.");
    assert_eq!(serde_json::to_value(e.avatar(&tid).held).unwrap(), json!({ "name": "spin", "mode": "loop", "since": clock.get() }));
    assert_eq!(go(mode("breathe", ActMode::Loop)).unwrap(), "Looping breathe until you stop it, play another or 2 minutes pass.");
    let held = events.drain();
    assert_eq!(held[0].data, json!({ "terminalId": tid, "play": "hop", "mode": "hold", "seq": 3 }));
    assert_eq!(held[1].data, json!({ "terminalId": tid, "play": "spin", "mode": "loop", "seq": 4 }));
    // Only an animation that loops plays in mode loop; its own blink replaced the built-in one.
    assert_eq!(go(mode("hop", ActMode::Loop)).unwrap_err().message, "hop does not loop: mode loop repeats only an animation declared loop: true. Play it once, or keep its last frame with mode hold.");
    assert!(go(mode("wave", ActMode::Loop)).unwrap_err().message.starts_with("wave does not loop"));
    assert!(go(mode("blink", ActMode::Loop)).unwrap_err().message.starts_with("blink does not loop"));
    assert_eq!(e.avatar(&tid).held.map(|h| h.name), Some("breathe".to_string()), "a refused call changes nothing");

    // Held ends at the next play, at a stop of its name, or at stop all.
    go(play("nod")).unwrap();
    assert!(e.avatar(&tid).held.is_none());
    go(mode("hop", ActMode::Hold)).unwrap();
    assert_eq!(go(stop("spin")).unwrap(), "Stopped spin.");
    assert_eq!(e.avatar(&tid).held.map(|h| h.name), Some("hop".to_string()), "another name's stop leaves it");
    assert_eq!(go(stop("hop")).unwrap(), "Stopped hop.");
    assert!(e.avatar(&tid).held.is_none());
    go(mode("spin", ActMode::Loop)).unwrap();
    assert_eq!(go(stop("all")).unwrap(), "Stopped every gesture.");
    assert!(e.avatar(&tid).held.is_none());
    assert_eq!(events.drain().last().unwrap().data, json!({ "terminalId": tid, "stop": "all", "seq": 11 }));

    // D3: repeated, a paint track that ends elsewhere than it starts would flash every round.
    assert_eq!(go(times("fade", 2)).unwrap_err().message, "fade cannot repeat: its opacity ends on another value than it starts with. Play it once, or close the track.");
    assert_eq!(go(times("fade", 1)).unwrap(), "Playing fade.");
    assert_eq!(go(times("hop", 5)).unwrap(), "Playing hop 5 times.");
    assert_eq!(go(times("nod", 5)).unwrap(), "Playing nod 5 times.", "built-in motion always closes");

    // What a call may ask for.
    assert_eq!(go(times("hop", 6)).unwrap_err().message, "times 6: a whole number from 1 to 5");
    assert_eq!(go(times("hop", 0)).unwrap_err().message, "times 0: a whole number from 1 to 5");
    assert_eq!(go(ActRequest { times: Some(2), ..mode("hop", ActMode::Hold) }).unwrap_err().message, "times is for mode once: a held or looping gesture stays until it is stopped");
    assert!(go(ActRequest::default()).unwrap_err().message.starts_with("act needs play"));
    assert!(go(ActRequest { stop: Some("all".into()), ..play("hop") }).unwrap_err().message.starts_with("play and stop in one call"));
    assert_eq!(ActRequest::parse(&json!({ "play": "hop", "mode": "hold" }), &unmasked).unwrap(), mode("hop", ActMode::Hold));
    assert_eq!(ActRequest::parse(&json!({ "play": "hop", "times": 3 }), &unmasked).unwrap(), times("hop", 3));
    assert_eq!(ActRequest::parse(&json!({ "stop": "all" }), &unmasked).unwrap(), stop("all"));
    assert_eq!(ActRequest::parse(&json!({ "play": "hop", "mode": "twice" }), &unmasked).unwrap_err().message, "mode twice: once, hold or loop");
    assert_eq!(ActRequest::parse(&json!({ "play": "hop", "times": 2.5 }), &unmasked).unwrap_err().message, "times 2.5: a whole number from 1 to 5");
    assert_eq!(ActRequest::parse(&json!({ "play": "hop", "speed": 2 }), &unmasked).unwrap_err().message, "act has no speed: it takes play, mode, times, stop");

    // A design that drops the held animation ends the hold; the default look plays every built-in.
    go(mode("spin", ActMode::Loop)).unwrap();
    e.design(&tid, json!({ "animations": { "spin": null } }), clock.get()).unwrap();
    assert!(e.avatar(&tid).held.is_none());
    let plain = e.with_avatar();
    assert_eq!(e.act(&plain, mode("think", ActMode::Loop), T0).unwrap(), "Looping think until you stop it, play another or 2 minutes pass.");
    assert!(e.act(&plain, play("hop"), T0 + SECOND).unwrap_err().message.starts_with("No animation hop. Yours: none; built in: breathe, blink, think,"));
}

#[tokio::test]
async fn gestures_go_out_at_most_four_times_a_second_and_a_held_one_ends_by_itself() {
    let e = env().await;
    let tid = e.with_avatar();
    let other = e.with_avatar();
    let mut events = Events::of(&e);
    let hold = |name: &str| ActRequest { play: Some(name.into()), mode: Some(ActMode::Hold), ..Default::default() };

    // The first goes out at once. What comes sooner than a quarter second after it waits its
    // turn, and a newer one takes its place.
    e.act(&tid, play("nod"), T0).unwrap();
    e.act(&tid, play("wave"), T0 + 10).unwrap();
    e.act(&tid, hold("cheer"), T0 + 20).unwrap();
    e.act(&other, play("shake"), T0 + 20).unwrap();
    let out = events.drain();
    assert_eq!(out.iter().map(|ev| (ev.data["terminalId"].as_str().unwrap(), ev.data["play"].as_str().unwrap(), ev.data["seq"].as_u64().unwrap())).collect::<Vec<_>>(), [(tid.as_str(), "nod", 1), (other.as_str(), "shake", 2)], "each terminal has its own turn");
    // The record follows every call, also the one that was replaced and the one that waits.
    assert_eq!(e.avatar(&tid).held.map(|h| (h.name, h.since)), Some(("cheer".to_string(), T0 + 20)));
    // Not before its turn.
    sweep(&e.state, T0 + ACT_GAP_MS - 1);
    assert!(events.drain().is_empty());
    sweep(&e.state, T0 + ACT_GAP_MS);
    let turn = events.drain();
    assert_eq!(kinds(&turn), ["avatar.act"]);
    assert_eq!(turn[0].data, json!({ "terminalId": tid, "play": "cheer", "mode": "hold", "seq": 3 }));
    sweep(&e.state, T0 + 2 * ACT_GAP_MS);
    assert!(events.drain().is_empty(), "nothing waits any more");
    // Right after a turn the next one waits again; a quarter second later it does not.
    e.act(&tid, play("nod"), T0 + ACT_GAP_MS + 100).unwrap();
    assert!(events.drain().is_empty());
    sweep(&e.state, T0 + 2 * ACT_GAP_MS);
    assert_eq!(events.drain()[0].data["play"], "nod");
    e.act(&tid, hold("wave"), T0 + 3 * ACT_GAP_MS).unwrap();
    assert_eq!(events.drain()[0].data["seq"], 5);

    // A held gesture ends by itself after two minutes: the browsers are told to stop it.
    let since = T0 + 3 * ACT_GAP_MS;
    sweep(&e.state, since + HOLD_MAX_SECS * SECOND - 1);
    assert!(e.avatar(&tid).held.is_some() && events.drain().is_empty());
    sweep(&e.state, since + HOLD_MAX_SECS * SECOND);
    assert!(e.avatar(&tid).held.is_none());
    let ended = events.drain();
    assert_eq!(kinds(&ended), ["avatar.act"]);
    assert_eq!(ended[0].data, json!({ "terminalId": tid, "stop": "wave", "seq": 6 }));
    assert_eq!(e.avatar(&tid).updated_at, since + HOLD_MAX_SECS * SECOND);
}

#[tokio::test]
async fn a_gesture_that_waits_its_turn_goes_out_by_itself() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut events = Events::of(&e);
    e.act(&tid, play("nod"), T0).unwrap();
    e.act(&tid, play("wave"), T0 + 1).unwrap();
    assert_eq!(events.kinds(), ["avatar.act"]);
    // Nobody sweeps: its own timer sends it a quarter second later.
    let started = std::time::Instant::now();
    let sent = loop {
        let sent = events.drain();
        if !sent.is_empty() || started.elapsed() > Duration::from_secs(20) {
            break sent;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(sent.iter().map(|ev| ev.data["play"].as_str().unwrap()).collect::<Vec<_>>(), ["wave"]);
    assert!(started.elapsed() >= Duration::from_millis(200), "it waited its turn: {:?}", started.elapsed());
}

// ---------------------------------------------------------------- persistence

#[tokio::test]
async fn an_avatar_survives_a_restart_and_what_lives_in_memory_does_not() {
    let e = env().await;
    let tid = e.with_avatar();
    let off = e.with_avatar();
    e.design(&tid, figure(), T0).unwrap();
    e.say(&tid, "Hello.", T0 + 2).unwrap();
    let (_, _waiter) = e.ask(&tid, "Ready?", &[], T0 + 3).unwrap();
    e.act(&tid, ActRequest { play: Some("hop".into()), mode: Some(ActMode::Hold), ..Default::default() }, T0 + 4).unwrap();
    // The last interface the session used is how it drives its avatar.
    gate(&e.state, &tid, Call::Status, Via::Command, T0 + 5).unwrap();
    e.design(&off, json!({ "name": "Quiet" }), T0).unwrap();
    turn_off(&e.state, &off, T0 + 5).unwrap();
    let before = e.avatar(&tid);

    // The directory and its files are the user's alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&e.state.paths.data_dir.join("avatar")), mode(&e.file(&tid))), (0o700, 0o600));
    }

    e.state.avatar.restart();
    let after = e.avatar(&tid);
    // What the file keeps: on, name, look, voice, how it is driven.
    assert_eq!((after.on, after.name.as_str(), after.designed, &after.look, &after.voice, after.via), (true, "Dot", true, &before.look, &before.voice, Via::Command));
    assert_eq!((after.terminal_id.as_str(), after.project_id.as_deref()), (tid.as_str(), Some(e.pid.as_str())));
    // Lines, the question and a held gesture lived in memory.
    assert!(after.lines.is_empty() && after.ask.is_none() && after.held.is_none());
    assert_eq!(after.revision, 0);
    assert!(after.updated_at > 0 && after.created_at <= after.updated_at);
    let quiet = e.avatar(&off);
    assert_eq!((quiet.on, quiet.name.as_str(), quiet.designed), (false, "Quiet", false));
    assert_eq!(list(&e.state).avatars.len(), 2);
    // The user's choice holds across the restart.
    assert_eq!(gate(&e.state, &off, Call::Say, Via::Tools, T0 + MINUTE).unwrap_err().message, TURNED_OFF);
    assert!(active(&e.state, &tid) && !active(&e.state, &off));
    e.say(&tid, "Back again.", T0 + MINUTE).unwrap();

    // A terminal that is gone leaves nothing behind at the next start, a corrupt file neither.
    let dir = e.state.paths.data_dir.join("avatar");
    std::fs::write(dir.join("0123456789ab.json"), serde_json::to_vec(&json!({ "version": 1, "on": true, "name": "Ghost", "designed": false, "look": null, "voice": {}, "via": "tools" })).unwrap()).unwrap();
    std::fs::write(dir.join("0123456789ab.json.corrupt"), b"{").unwrap();
    // Names that are no terminal's avatar are not read, and not touched.
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    std::fs::write(dir.join("Not-An-Id.json"), b"{}").unwrap();
    e.state.avatar.restart();
    assert_eq!(list(&e.state).avatars.len(), 2);
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|f| f.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    let mut expected = vec!["Not-An-Id.json".to_string(), format!("{off}.json"), format!("{tid}.json"), "notes.txt".to_string()];
    expected.sort();
    assert_eq!(left, expected);
}

#[tokio::test]
async fn a_stored_file_passes_the_validator_a_design_call_passes_or_is_kept_aside() {
    let e = env().await;
    let tid = e.with_avatar();
    e.design(&tid, figure(), T0).unwrap();
    let good = e.avatar(&tid);
    let file = e.file(&tid);
    let corrupt = file.with_extension("json.corrupt");
    let stored: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let write = |value: &Value| std::fs::write(&file, serde_json::to_vec(value).unwrap()).unwrap();

    // A hand-edited file with the same design loads: what it claims about boxes is not believed.
    let mut edited = stored.clone();
    edited["look"]["nodes"][0]["box"] = json!([0, 0, 999999, 999999]);
    edited["designed"] = json!(false);
    edited["on"] = json!(false);
    write(&edited);
    e.state.avatar.restart();
    let loaded = e.avatar(&tid);
    assert_eq!((&loaded.look, loaded.designed, loaded.on, loaded.name.as_str()), (&good.look, true, false, "Dot"));
    assert!(!corrupt.exists());

    // What a design call would refuse is refused in a file as well: the avatar falls back to
    // the default look and the file is kept aside, one per terminal. What the file says of the
    // switch still holds when it can be read: an avatar the user turned off stays off. These
    // files are of an avatar that is off, unless the switch itself is what cannot be read.
    let off = {
        let mut v = stored.clone();
        v["on"] = json!(false);
        v
    };
    let hostile: Vec<(&str, Value, bool)> = vec![
        (
            "an attribute that is not in the table",
            {
                let mut v = off.clone();
                v["look"]["nodes"][0]["attrs"]["onclick"] = json!("alert(1)");
                v
            },
            false,
        ),
        (
            "an element that is not in the table",
            {
                let mut v = off.clone();
                v["look"]["nodes"][0]["tag"] = json!("script");
                v
            },
            false,
        ),
        (
            "an unknown key of the look",
            {
                let mut v = off.clone();
                v["look"]["onload"] = json!("alert(1)");
                v
            },
            false,
        ),
        (
            "a name that is too long",
            {
                let mut v = off.clone();
                v["name"] = json!("n".repeat(33));
                v
            },
            false,
        ),
        (
            "an animation that targets nothing",
            {
                let mut v = off.clone();
                v["look"]["animations"]["hop"]["tracks"][0]["target"] = json!("nobody");
                v
            },
            false,
        ),
        (
            "another version of the format",
            {
                let mut v = off.clone();
                v["version"] = json!(2);
                v
            },
            false,
        ),
        (
            "a switch that is no boolean",
            {
                let mut v = off.clone();
                v["on"] = json!("yes");
                v
            },
            true,
        ),
        (
            "a way of driving it that does not exist",
            {
                let mut v = off.clone();
                v["via"] = json!("telepathy");
                v
            },
            false,
        ),
        ("no object at all", json!([1, 2, 3]), true),
    ];
    for (what, value, on) in hostile {
        let _ = std::fs::remove_file(&corrupt);
        write(&value);
        e.state.avatar.restart();
        let fallen = e.avatar(&tid);
        assert!(fallen.look.is_none() && !fallen.designed && fallen.name.is_empty(), "{what}");
        assert_eq!(fallen.on, on, "{what}");
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&corrupt).unwrap()).unwrap(), value, "{what}: kept aside");
        // The session's file is a valid one again, so the next start reads it without a word.
        let fresh: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(fresh, json!({ "version": 1, "on": on, "name": "", "designed": false, "look": null, "voice": {}, "via": "tools" }), "{what}");
    }

    // Text that is not JSON, and a file larger than a design may be.
    for (what, bytes) in [("not JSON", b"{ \"version\": 1, ".to_vec()), ("too large", { let mut big = stored.clone(); big["padding"] = json!("x".repeat(300 * 1024)); serde_json::to_vec(&big).unwrap() })] {
        std::fs::write(&file, &bytes).unwrap();
        e.state.avatar.restart();
        assert!(e.avatar(&tid).look.is_none(), "{what}");
        assert_eq!(std::fs::read(&corrupt).unwrap(), bytes, "{what}: the newer corrupt file replaces the older");
    }
    // The session designs again; the corrupt file stays for whoever wants to look, until the
    // terminal is removed.
    e.design(&tid, figure(), T0 + MINUTE).unwrap();
    e.state.avatar.restart();
    assert!(e.avatar(&tid).designed && corrupt.exists());
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: Some(e.pid.clone()), data: json!({ "id": tid }), ts: T0 + 2 * MINUTE });
    assert!(!file.exists() && !corrupt.exists());
}

/// A design as heavy as a call may make one: sixteen parts, and sixteen animations of sixteen
/// tracks that each move all of them through sixteen keyframes.
fn heavy() -> Value {
    let ids: Vec<String> = (0..16).map(|i| format!("piece_{i:02}")).collect();
    let circles: String = ids.iter().enumerate().map(|(i, id)| format!("<circle id='{id}' cx='{}' cy='100' r='4' fill='#ffcc00'/>", 25 + i * 10)).collect();
    let values: Vec<Value> = (0..look::MAX_KEYFRAMES).map(|i| json!(0.123 + i as f64 / 100.0)).collect();
    let times: Vec<Value> = (0..look::MAX_KEYFRAMES).map(|i| json!(i as f64 / (look::MAX_KEYFRAMES - 1) as f64)).collect();
    let track = json!({ "target": ids, "prop": "opacity", "values": values, "times": times });
    let animations: serde_json::Map<String, Value> = (0..16).map(|i| (format!("heavy{i}"), json!({ "duration": 30, "tracks": vec![track.clone(); look::MAX_TRACKS] }))).collect();
    json!({ "name": "Heavy", "svg": format!("<svg viewBox='0 0 200 200'><g id='body'>{circles}</g></svg>"), "animations": animations })
}

#[tokio::test]
async fn a_heavy_design_survives_a_restart_and_stays_off() {
    let e = env().await;
    let tid = e.with_avatar();
    e.design(&tid, heavy(), T0).unwrap();
    let before = e.avatar(&tid);
    // Heavy indeed: within the limit of a stored look, and written with indentation larger than
    // a stored file may be.
    let look = before.look.clone().expect("a look");
    assert!(serde_json::to_vec(&look).unwrap().len() < look::MAX_LOOK_BYTES, "the look is accepted");
    assert!(serde_json::to_vec_pretty(&look).unwrap().len() as u64 > MAX_STORED_BYTES, "pretty, it would not be read again");

    // The user turns it off, and Workbench starts again: the look is there, and off is off.
    turn_off(&e.state, &tid, T0 + SECOND).unwrap();
    e.state.avatar.restart();
    let after = e.avatar(&tid);
    assert!(after.designed && !after.on);
    assert_eq!(after.look, before.look);
    assert!(!e.file(&tid).with_extension("json.corrupt").exists());
    assert!(std::fs::metadata(e.file(&tid)).unwrap().len() <= MAX_STORED_BYTES);
    assert_eq!(gate(&e.state, &tid, Call::Say, Via::Tools, T0 + MINUTE).unwrap_err().message, TURNED_OFF);
}

// ---------------------------------------------------------------- lifecycle

#[tokio::test]
async fn a_session_started_with_the_option_gets_its_avatar_and_a_moved_one_takes_its_along() {
    let e = env().await;
    let mut events = Events::of(&e);

    // Started with the option: a record with the default look, on.
    let tid = e.agent(json!({ "avatar": true }));
    e.event("terminal.created", &tid, T0);
    let made = events.drain();
    assert_eq!(kinds(&made), ["avatar.changed"]);
    assert_eq!((made[0].project_id.as_deref(), &made[0].data), (Some(e.pid.as_str()), &json!({ "terminalId": tid, "revision": 0, "on": true })));
    let avatar = e.avatar(&tid);
    assert!(avatar.on && avatar.look.is_none() && !avatar.designed && avatar.lines.is_empty() && avatar.ask.is_none() && avatar.held.is_none());
    assert_eq!((avatar.via, avatar.created_at, avatar.updated_at, avatar.name.as_str()), (Via::Tools, T0, T0, ""));
    assert!(e.file(&tid).exists());
    // The record and the list on the wire: the keys the browsers read, null where there is nothing.
    let wire = serde_json::to_value(&avatar).unwrap();
    assert_eq!(wire.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["ask", "createdAt", "designed", "held", "lines", "look", "name", "on", "projectId", "revision", "terminalId", "updatedAt", "via", "voice"]);
    assert_eq!(wire, json!({ "terminalId": tid, "projectId": e.pid, "on": true, "name": "", "designed": false, "revision": 0, "look": null, "voice": {}, "via": "tools", "held": null, "ask": null, "lines": [], "createdAt": T0, "updatedAt": T0 }));
    assert_eq!(serde_json::to_value(list(&e.state)).unwrap(), json!({ "enabled": true, "sessions": "asked", "avatars": [wire], "asked": [] }));
    // The same event again makes nothing new, and takes nothing away.
    e.design(&tid, figure(), T0 + 1).unwrap();
    events.drain();
    e.event("terminal.created", &tid, T0 + 2);
    assert!(events.drain().is_empty() && e.avatar(&tid).designed);

    // An event that names a terminal the terminals slice does not know makes no record and no file.
    let mut unknown = e.terminal(&tid);
    unknown.id = "ffffffffffff".into();
    on_event(&e.state, &Event { kind: "terminal.created".into(), project_id: None, data: serde_json::to_value(&unknown).unwrap(), ts: T0 + 3 });
    assert!(events.drain().is_empty() && !e.file("ffffffffffff").exists() && get(&e.state, "ffffffffffff").is_err());

    // Without the option: none. Other kinds of terminal never have one.
    let plain = e.agent(json!({}));
    e.event("terminal.created", &plain, T0);
    let shell = e.agent(json!({ "avatar": true }));
    e.change(&shell, |info| {
        info.kind = crate::terminals::TerminalKind::Shell;
        info.agent = None;
    });
    e.event("terminal.created", &shell, T0);
    assert!(events.drain().is_empty() && list(&e.state).avatars.len() == 1);
    assert_eq!(turn_on(&e.state, &shell, T0).unwrap_err().message, "only agent sessions have an avatar");

    // A session that moves to another account takes its avatar along: on, name, look, voice.
    // How the new one drives it follows from its own kind.
    let moved = e.agent(json!({ "movedFrom": tid, "inContainer": true }));
    e.change(&moved, |info| info.agent.as_mut().unwrap().provider = ProviderKind::Codex);
    e.event("terminal.created", &moved, T0 + 10);
    assert_eq!(events.kinds(), ["avatar.changed"]);
    let (old, new) = (e.avatar(&tid), e.avatar(&moved));
    assert_eq!((new.on, new.name.as_str(), &new.look, &new.voice, new.designed), (true, "Dot", &old.look, &old.voice, true));
    assert_eq!((new.via, new.revision, new.created_at, new.lines.len()), (Via::Command, 0, T0 + 10, 0));
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(e.file(&moved)).unwrap()).unwrap()["name"], "Dot");
    // Exactly once: what the user does to the new avatar afterwards holds, whatever events follow.
    reset(&e.state, &moved, T0 + 11).unwrap();
    events.drain();
    e.event("terminal.created", &moved, T0 + 12);
    e.event("terminal.updated", &moved, T0 + 13);
    assert!(events.drain().is_empty() && e.avatar(&moved).look.is_none());
    assert!(e.avatar(&tid).designed, "the session it came from keeps its own");

    // An avatar the user had turned off moves along as off; a move from a session without one
    // falls back to the option.
    turn_off(&e.state, &tid, T0 + 20).unwrap();
    let quiet = e.agent(json!({ "movedFrom": tid, "avatar": true }));
    e.event("terminal.created", &quiet, T0 + 21);
    assert!(!e.avatar(&quiet).on && e.avatar(&quiet).designed);
    assert_eq!(carried(&e.state, &tid), Some(Carried { on: false, designed: true }));
    // The one that moved with its avatar on was told of it with its first prompt; this one was
    // told nothing, and is told once the user turns its avatar on.
    assert!(!unbriefed(&e, &moved) && unbriefed(&e, &quiet));
    assert!(turn_on(&e.state, &quiet, T0 + 24).unwrap().brief);
    let fresh = e.agent(json!({ "movedFrom": plain, "avatar": true }));
    e.event("terminal.created", &fresh, T0 + 22);
    assert!(e.avatar(&fresh).on && !e.avatar(&fresh).designed);
    let none = e.agent(json!({ "movedFrom": plain }));
    e.event("terminal.created", &none, T0 + 23);
    assert_eq!((get(&e.state, &none).unwrap_err().status.as_u16(), carried(&e.state, &plain)), (404, None));
}

/// Whether the session is still to be told of its avatar (after a restart, as read from its file).
fn unbriefed(e: &Env, tid: &str) -> bool {
    super::store::ensure_loaded(&e.state);
    e.state.avatar.inner.lock().records.get(tid).expect("an avatar").unbriefed
}

/// The next paste, once the tasks that make one have run; `None` when none comes.
async fn next_paste(pastes: &mut UnboundedReceiver<Paste>) -> Option<Paste> {
    for _ in 0..100 {
        if let Ok(paste) = pastes.try_recv() {
            return Some(paste);
        }
        tokio::task::yield_now().await;
    }
    None
}

/// Lets the tasks that record a paste run.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn whether_a_session_was_told_is_kept_in_the_avatars_file() {
    let e = env().await;
    let stored = |tid: &str| serde_json::from_slice::<Value>(&std::fs::read(e.file(tid)).unwrap()).unwrap();
    // A device gives a running session an avatar: it is to be told, also after a restart.
    let tid = e.agent(json!({}));
    assert!(turn_on(&e.state, &tid, T0).unwrap().brief);
    assert_eq!(stored(&tid)["unbriefed"], json!(true));
    e.state.avatar.restart();
    assert!(unbriefed(&e, &tid) && turn_on(&e.state, &tid, T0 + 1).unwrap().brief);
    // Told is told: the file says nothing of it any more, as for every session that was told.
    briefed(&e.state, &tid, T0 + 2);
    assert_eq!(stored(&tid).get("unbriefed"), None);
    e.state.avatar.restart();
    assert!(!unbriefed(&e, &tid) && !turn_on(&e.state, &tid, T0 + 3).unwrap().brief);
    // A file with something else than true or false there holds no valid avatar.
    let mut odd = stored(&tid);
    odd["unbriefed"] = json!("yes");
    std::fs::write(e.file(&tid), serde_json::to_vec(&odd).unwrap()).unwrap();
    e.state.avatar.restart();
    assert!(!unbriefed(&e, &tid) && e.file(&tid).with_extension("json.corrupt").exists());

    // A conversation that starts anew says whether the brief went with it, on or off.
    conversation_started(&e.state, &tid, false);
    assert!(unbriefed(&e, &tid) && stored(&tid)["unbriefed"] == json!(true));
    conversation_started(&e.state, &tid, true);
    assert!(!unbriefed(&e, &tid) && stored(&tid).get("unbriefed").is_none());
    turn_off(&e.state, &tid, T0 + 4).unwrap();
    conversation_started(&e.state, &tid, false);
    assert!(unbriefed(&e, &tid));
    // Turned on again, the session is to be told.
    assert!(turn_on(&e.state, &tid, T0 + 5).unwrap().brief);
    // A session without an avatar gets none from it.
    let bare = e.agent(json!({}));
    conversation_started(&e.state, &bare, false);
    assert!(get(&e.state, &bare).is_err() && !e.file(&bare).exists());

    // A session started with the option whose first prompt was a command of its CLI was started
    // without the brief: its record says so.
    let later = e.agent(json!({ "avatar": true, "avatarBrief": "later" }));
    e.set_state(&later, AgentState::Starting);
    e.event("terminal.created", &later, T0);
    assert!(unbriefed(&e, &later) && stored(&later)["unbriefed"] == json!(true));
    let told = e.with_avatar();
    assert!(!unbriefed(&e, &told) && stored(&told).get("unbriefed").is_none());
    // The same when a device makes its record (the event made none): it is to be told now.
    let given = e.agent(json!({ "avatar": true, "avatarBrief": "later" }));
    assert!(turn_on(&e.state, &given, T0).unwrap().brief && unbriefed(&e, &given));
}

#[tokio::test]
async fn a_session_that_was_not_told_is_told_when_it_is_next_idle() {
    let e = env().await;
    let mut pastes = e.pastes();
    // Started with the option and a command as its first prompt: it is still starting.
    let tid = e.agent(json!({ "avatar": true, "avatarBrief": "later" }));
    e.set_state(&tid, AgentState::Starting);
    e.event("terminal.created", &tid, T0);
    assert!(next_paste(&mut pastes).await.is_none());
    // It runs its command, and is idle: it is told now, with the brief of a session that was
    // given an avatar while it worked.
    e.goes(&tid, AgentState::Working, T0 + SECOND);
    assert!(next_paste(&mut pastes).await.is_none());
    e.goes(&tid, AgentState::Idle, T0 + 2 * SECOND);
    let paste = next_paste(&mut pastes).await.expect("the brief is pasted");
    assert_eq!((paste.tid.as_str(), paste.prompt.as_str()), (tid.as_str(), brief::telling(false, Via::Tools, false).as_str()));
    let _ = paste.done.send(Ok(()));
    settle().await;
    assert!(!unbriefed(&e, &tid));
    // Told is told: the next idle pastes nothing.
    e.goes(&tid, AgentState::Working, T0 + MINUTE);
    e.goes(&tid, AgentState::Idle, T0 + 2 * MINUTE);
    assert!(next_paste(&mut pastes).await.is_none());

    // A session that already is idle when its record is made is told at once; one with a look of
    // its own is told it keeps it, and to go on with what it was doing.
    let idle = e.agent(json!({ "avatar": true, "avatarBrief": "later" }));
    e.event("terminal.created", &idle, T0);
    let paste = next_paste(&mut pastes).await.expect("the brief is pasted");
    assert_eq!(paste.tid, idle);
    let _ = paste.done.send(Err(ApiError::conflict("that session is showing a prompt (permission, question or trust); answer it first")));
    settle().await;
    e.design(&idle, figure(), T0 + SECOND).unwrap();
    e.goes(&idle, AgentState::Working, T0 + MINUTE);
    e.goes(&idle, AgentState::Idle, T0 + MINUTE + SECOND);
    let paste = next_paste(&mut pastes).await.expect("the brief is pasted again");
    assert_eq!(paste.prompt, brief::telling(true, Via::Tools, false));
    assert!(paste.prompt.ends_with("then go on with what you were doing.</workbench-avatar>"), "{}", paste.prompt);
    let _ = paste.done.send(Ok(()));
    settle().await;
    assert!(!unbriefed(&e, &idle));

    // An avatar that is off is not told of; nor is a session whose tab is closed.
    let off = e.agent(json!({}));
    turn_on(&e.state, &off, T0).unwrap();
    turn_off(&e.state, &off, T0).unwrap();
    e.event("terminal.updated", &off, T0 + SECOND);
    let closed = e.agent(json!({}));
    turn_on(&e.state, &closed, T0).unwrap();
    e.change(&closed, |info| info.open = false);
    e.event("terminal.updated", &closed, T0 + SECOND);
    assert!(next_paste(&mut pastes).await.is_none());
}

#[tokio::test]
async fn the_lifecycle_tells_once_per_idle_and_gives_up_after_refusals_until_the_next_on() {
    let e = env().await;
    let mut pastes = e.pastes();
    // A device gave the running session an avatar; telling it then was refused.
    let tid = e.agent(json!({}));
    assert!(turn_on(&e.state, &tid, T0).unwrap().brief);
    let refuse = |paste: Paste| {
        let _ = paste.done.send(Err(ApiError::conflict("that session is showing a prompt (permission, question or trust); answer it first")));
    };
    // The session is idle: the next word of it tells it, once in this idle.
    e.event("terminal.updated", &tid, T0 + SECOND);
    refuse(next_paste(&mut pastes).await.expect("told when idle"));
    settle().await;
    e.event("terminal.updated", &tid, T0 + 40 * SECOND);
    assert!(next_paste(&mut pastes).await.is_none(), "once per idle");
    // Idle again soon after: not yet, the last try was less than 30 seconds ago.
    let retry = BRIEF_RETRY_SECS * SECOND;
    e.goes(&tid, AgentState::Working, T0 + 2 * SECOND);
    e.goes(&tid, AgentState::Idle, T0 + retry);
    assert!(next_paste(&mut pastes).await.is_none(), "at most every 30 seconds");
    // Every later idle tries again, until the session refused it five times.
    let mut at = T0 + 2 * retry;
    for _ in 1..BRIEF_REFUSALS {
        e.goes(&tid, AgentState::Working, at - SECOND);
        e.goes(&tid, AgentState::Idle, at);
        refuse(next_paste(&mut pastes).await.expect("told at the next idle"));
        settle().await;
        at += retry;
    }
    e.goes(&tid, AgentState::Working, at - SECOND);
    e.goes(&tid, AgentState::Idle, at);
    assert!(next_paste(&mut pastes).await.is_none(), "it gave up");
    assert!(unbriefed(&e, &tid));
    // The next on tries again, and so does the lifecycle after it.
    let on = turn_on(&e.state, &tid, at + 1).unwrap();
    assert!(on.brief);
    at += retry;
    e.goes(&tid, AgentState::Working, at - SECOND);
    e.goes(&tid, AgentState::Idle, at);
    let _ = next_paste(&mut pastes).await.expect("told again").done.send(Ok(()));
    settle().await;
    assert!(!unbriefed(&e, &tid));
}

#[tokio::test]
async fn the_brief_going_in_is_not_the_user_answering_in_the_terminal() {
    let e = env().await;
    let mut pastes = e.pastes();
    let tid = e.agent(json!({}));
    turn_on(&e.state, &tid, T0).unwrap();
    let ask = e.asked(&tid, "Shall I go on?", &[], T0).await;
    // The session is told; meanwhile, and right after, it works because of the brief.
    e.event("terminal.updated", &tid, T0 + SECOND);
    let paste = next_paste(&mut pastes).await.expect("the brief is pasted");
    e.goes(&tid, AgentState::Working, T0 + 2 * SECOND);
    let _ = paste.done.send(Ok(()));
    settle().await;
    e.goes(&tid, AgentState::Idle, T0 + 3 * SECOND);
    e.goes(&tid, AgentState::Working, T0 + 4 * SECOND);
    assert_eq!(e.avatar(&tid).ask.map(|a| a.id), Some(ask.id.clone()), "the question stays open");
    // Later, going on without an answer is the user answering in the terminal again.
    e.goes(&tid, AgentState::Idle, T0 + MINUTE);
    e.goes(&tid, AgentState::Working, T0 + MINUTE + SECOND);
    assert!(e.avatar(&tid).ask.is_none());
}

#[tokio::test]
async fn a_telling_whose_request_goes_away_during_its_paste_is_still_recorded() {
    let e = env().await;
    let mut pastes = e.pastes();
    let tid = e.agent(json!({}));
    assert!(turn_on(&e.state, &tid, T0).unwrap().brief);
    let mut telling = Box::pin(tell(&e.state, &tid, T0));
    let paste = tokio::select! {
        paste = pastes.recv() => paste.unwrap(),
        early = &mut telling => panic!("the telling returned before its paste: {early:?}"),
    };
    // A second telling meanwhile pastes nothing.
    assert_eq!(tell(&e.state, &tid, T0).await, None);
    drop(telling);
    let _ = paste.done.send(Ok(()));
    settle().await;
    // The brief went in: the session counts as told, and is not told again.
    assert!(!unbriefed(&e, &tid) && !turn_on(&e.state, &tid, T0 + 1).unwrap().brief);
    assert_eq!(tell(&e.state, &tid, T0 + 2).await, None);
    assert!(next_paste(&mut pastes).await.is_none());
    // Nothing to tell either for an avatar that is off, or a session that is not running.
    let off = e.agent(json!({}));
    turn_on(&e.state, &off, T0).unwrap();
    turn_off(&e.state, &off, T0).unwrap();
    let ended = e.agent(json!({}));
    turn_on(&e.state, &ended, T0).unwrap();
    e.change(&ended, |info| info.status = TerminalStatus::Exited);
    assert_eq!((tell(&e.state, &off, T0).await, tell(&e.state, &ended, T0).await), (None, None));
}

#[tokio::test]
async fn an_avatar_turned_off_while_its_session_is_told_is_told_of_at_the_next_on() {
    let e = env().await;
    let mut pastes = e.pastes();
    let tid = e.agent(json!({}));
    assert!(turn_on(&e.state, &tid, T0).unwrap().brief);
    let refusals = |e: &Env| e.state.avatar.inner.lock().records.get(&tid).map(|rec| rec.brief_refusals);

    // Turned off after the telling looked, before its paste went out: nothing goes in, and
    // nothing counts as refused.
    let mut telling = Box::pin(tell(&e.state, &tid, T0));
    tokio::select! {
        biased;
        early = &mut telling => panic!("the telling returned before its paste: {early:?}"),
        _ = std::future::ready(()) => {}
    }
    turn_off(&e.state, &tid, T0 + 1).unwrap();
    assert!(next_paste(&mut pastes).await.is_none(), "the brief went into a session whose avatar is off");
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), telling).await.unwrap(), None);
    assert_eq!((unbriefed(&e, &tid), refusals(&e)), (true, Some(0)));

    // Turned off while its paste was on its way: it went in, but the session read "turned off"
    // since, and is told again when its avatar is next on.
    assert!(turn_on(&e.state, &tid, T0 + 2).unwrap().brief);
    let mut telling = Box::pin(tell(&e.state, &tid, T0 + 2));
    let paste = tokio::select! {
        paste = pastes.recv() => paste.unwrap(),
        early = &mut telling => panic!("the telling returned before its paste: {early:?}"),
    };
    turn_off(&e.state, &tid, T0 + 3).unwrap();
    let _ = paste.done.send(Ok(()));
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), telling).await.unwrap(), Some(true));
    assert!(unbriefed(&e, &tid));
    assert!(turn_on(&e.state, &tid, T0 + 4).unwrap().brief);
}

#[tokio::test]
async fn a_brief_left_in_the_input_box_by_a_dialog_is_not_pasted_again() {
    let e = env().await;
    let mut pastes = e.pastes();
    let tid = e.agent(json!({}));
    assert!(turn_on(&e.state, &tid, T0).unwrap().brief);
    // A dialog opened while the brief was being pasted: the Enter was withheld, and the brief
    // stands in the session's input box, where the user sees it once the dialog is answered.
    let in_the_box = || ApiError::conflict(crate::terminals::PASTED_NOT_SENT);
    let (told, ()) = tokio::join!(tell(&e.state, &tid, T0), async {
        let paste = tokio::time::timeout(Duration::from_secs(10), pastes.recv()).await.unwrap().unwrap();
        let _ = paste.done.send(Err(in_the_box()));
    });
    assert_eq!(told, Some(true));
    assert!(!unbriefed(&e, &tid));
    // No later idle pastes it a second time, to be sent with the first.
    e.goes(&tid, AgentState::Working, T0 + MINUTE);
    e.goes(&tid, AgentState::Idle, T0 + 2 * MINUTE);
    assert!(next_paste(&mut pastes).await.is_none());

    // Any other refusal leaves it to be told at a later idle.
    let other = e.agent(json!({}));
    assert!(turn_on(&e.state, &other, T0).unwrap().brief);
    e.event("terminal.updated", &other, T0 + SECOND);
    let _ = next_paste(&mut pastes).await.expect("told when idle").done.send(Err(ApiError::conflict("that session is showing a prompt (permission, question or trust); answer it first")));
    settle().await;
    assert!(unbriefed(&e, &other));
}

#[tokio::test]
async fn a_closed_tab_and_a_removed_terminal_end_what_the_avatar_had() {
    let e = env().await;
    let tid = e.with_avatar();
    let shown = e.say(&tid, "First.", T0).unwrap().line.id;
    e.heard(&tid, &shown, Outcome::Shown, T0 + 1);
    e.say(&tid, "Second.", T0 + 2).unwrap();
    let (ask, waiter) = e.ask(&tid, "Third?", &[], T0 + 3).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    let mut events = Events::of(&e);

    // The tab is closed: its question is over, and what was not presented will not be.
    e.change(&tid, |info| info.open = false);
    e.event("terminal.updated", &tid, T0 + 10);
    assert_eq!(waiting.await.unwrap().reason, Some(Reason::SessionEnded));
    // The browsers learn that those lines are over, and to read the record again.
    let closed = events.drain();
    assert_eq!(kinds(&closed), ["avatar.ask", "avatar.line", "avatar.line", "avatar.changed"]);
    assert_eq!(closed[0].data, json!({ "terminalId": tid, "change": "session_ended", "ask": null }));
    assert_eq!((closed[1].data["line"]["text"].as_str(), closed[1].data["line"]["outcome"].as_str()), (Some("Second."), Some("unheard")));
    assert_eq!((closed[2].data["line"]["id"].as_str(), closed[2].data["line"]["outcome"].as_str()), (Some(ask.line_id.as_str()), Some("unheard")));
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.lines.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), avatar.ask.is_none(), avatar.updated_at), (vec![shown.as_str()], true, T0 + 10));
    assert_ne!(ask.line_id, shown);
    // The avatar itself stays: the tab can be opened again.
    assert!(avatar.on && e.file(&tid).exists());
    e.event("terminal.updated", &tid, T0 + 11);
    assert!(events.drain().is_empty());

    // The terminal is removed: so are the avatar, its file and its corrupt file.
    e.change(&tid, |info| info.open = true);
    let (_, waiter) = e.ask(&tid, "Fourth?", &[], T0 + 20).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    let corrupt = e.file(&tid).with_extension("json.corrupt");
    std::fs::write(&corrupt, b"{").unwrap();
    events.drain();
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: Some(e.pid.clone()), data: json!({ "id": tid }), ts: T0 + 30 });
    assert_eq!(waiting.await.unwrap().reason, Some(Reason::SessionEnded));
    let removed = events.drain();
    assert_eq!(kinds(&removed), ["avatar.changed"]);
    assert_eq!((removed[0].project_id.as_deref(), &removed[0].data), (Some(e.pid.as_str()), &json!({ "terminalId": tid, "revision": 0, "on": true, "removed": true })));
    assert!(!e.file(&tid).exists() && !corrupt.exists() && list(&e.state).avatars.is_empty());
    assert_eq!(get(&e.state, &tid).unwrap_err().status.as_u16(), 404);
    // Events of terminals that have no avatar, and events this slice does not follow, do nothing.
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: None, data: json!({ "id": tid }), ts: T0 + 31 });
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: None, data: json!({ "id": "../../x" }), ts: T0 + 31 });
    on_event(&e.state, &Event { kind: "terminal.updated".into(), project_id: None, data: json!({ "id": 7 }), ts: T0 + 31 });
    on_event(&e.state, &Event { kind: "git.changed".into(), project_id: None, data: json!({}), ts: T0 + 31 });
    assert!(events.drain().is_empty());

    // When events were missed, the records are compared with the terminals that exist.
    let gone = e.with_avatar();
    let stopped = e.with_avatar();
    let running = e.with_avatar();
    let (_, gone_waiter) = e.ask(&gone, "Still there?", &[], T0).unwrap();
    let (_, stopped_waiter) = e.ask(&stopped, "Still there?", &[], T0).unwrap();
    let (_, running_waiter) = e.ask(&running, "Still there?", &[], T0).unwrap();
    // A terminal the terminals slice forgot without this slice hearing of it (it is not started
    // here, so it announces nothing).
    e.state.terminals.close(&e.state, &gone, true).await.unwrap();
    assert!(e.state.terminals.info(&gone).is_none());
    e.change(&stopped, |info| info.status = TerminalStatus::Exited);
    events.drain();
    reconcile(&e.state, T0 + MINUTE);
    assert_eq!(wait_answer_within(gone_waiter, Duration::ZERO, LONG).await.reason, Some(Reason::SessionEnded));
    assert_eq!(wait_answer_within(stopped_waiter, Duration::ZERO, LONG).await.reason, Some(Reason::SessionEnded));
    assert!(get(&e.state, &gone).is_err() && !e.file(&gone).exists());
    let stopped_now = e.avatar(&stopped);
    assert!(stopped_now.ask.is_none() && stopped_now.lines.iter().all(|l| l.outcome == Some(Outcome::Unheard)));
    assert!(e.avatar(&running).ask.is_some() && e.avatar(&running).lines[0].outcome.is_none());
    drop(running_waiter);
}

#[tokio::test]
async fn at_most_sixty_four_sessions_have_an_avatar_and_none_is_evicted() {
    let e = env_with(|cfg| cfg.avatar.sessions = Some(Sessions::Any)).await;
    let first = e.with_avatar();
    for _ in 1..MAX_AVATARS {
        e.with_avatar();
    }
    assert_eq!(list(&e.state).avatars.len(), MAX_AVATARS);
    let mut events = Events::of(&e);

    // One more is refused, whichever way it comes: the option, a device, the session's own call.
    let late = e.agent(json!({ "avatar": true }));
    e.event("terminal.created", &late, T0);
    let by_device = turn_on(&e.state, &late, T0).unwrap_err();
    assert_eq!((by_device.status.as_u16(), by_device.code), (429, "rate_limited"));
    assert!(by_device.message.starts_with("64 sessions have an avatar already"), "{}", by_device.message);
    let by_call = gate(&e.state, &late, Call::Say, Via::Tools, T0).unwrap_err();
    assert_eq!((by_call.status.as_u16(), by_call.message.as_str()), (429, "No avatar: 64 sessions have one already. Say it in the terminal instead."));
    assert!(events.drain().is_empty() && !e.file(&late).exists());
    assert_eq!(list(&e.state).avatars.len(), MAX_AVATARS);
    assert!(e.avatar(&first).on, "nothing was evicted");

    // A removed terminal makes room.
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: None, data: json!({ "id": first }), ts: T0 + 1 });
    assert!(turn_on(&e.state, &late, T0 + 2).unwrap().info.on);
}

#[tokio::test]
async fn time_ends_what_nobody_acknowledged() {
    let e = env().await;
    let tid = e.with_avatar();
    let a = e.say(&tid, "One.", T0).unwrap().line.id;
    let b = e.say(&tid, &"b".repeat(300), T0 + SECOND).unwrap().line.id;
    let c = e.say(&tid, "Three.", T0 + 2 * SECOND).unwrap().line.id;
    e.heard(&tid, &c, Outcome::Hidden, T0 + 2 * SECOND);
    // A tab holds the long line to speak it: 20 s of speech and 15 s more.
    assert!(e.claim(&tid, &b, "tab-A", T0 + 50 * SECOND).speak);
    let mut events = Events::of(&e);
    let outcomes = || e.avatar(&tid).lines.iter().map(|l| l.outcome).collect::<Vec<_>>();

    // Not yet stale.
    sweep(&e.state, T0 + LINE_STALE_SECS * SECOND);
    assert!(events.drain().is_empty());
    // A line nobody acknowledged for a minute is unheard. One a tab is saying is left alone, and
    // so is one with any outcome.
    sweep(&e.state, T0 + 62 * SECOND);
    let stale = events.drain();
    assert_eq!(kinds(&stale), ["avatar.line"]);
    assert_eq!(stale[0].data, json!({ "terminalId": tid, "line": { "id": a, "at": T0, "from": "agent", "text": "One.", "outcome": "unheard" } }));
    assert_eq!(outcomes(), [Some(Outcome::Unheard), None, Some(Outcome::Hidden)]);
    assert_eq!(e.avatar(&tid).updated_at, T0 + 62 * SECOND);
    // Its lease lapsed and still no word of it: unheard as well.
    sweep(&e.state, T0 + 86 * SECOND);
    assert_eq!(outcomes(), [Some(Outcome::Unheard), Some(Outcome::Unheard), Some(Outcome::Hidden)]);
    assert_eq!(events.kinds(), ["avatar.line"]);
    sweep(&e.state, T0 + 10 * MINUTE);
    assert!(events.drain().is_empty(), "each line once");

    // The list of who asked forgets sessions that ended, were removed or got an avatar.
    let (ended, removed, given, asks) = (e.agent(json!({})), e.agent(json!({})), e.agent(json!({})), e.agent(json!({})));
    for who in [&ended, &removed, &given, &asks] {
        gate(&e.state, who, Call::Say, Via::Tools, T0).unwrap_err();
    }
    assert_eq!(list(&e.state).asked.len(), 4);
    e.change(&ended, |info| info.status = TerminalStatus::Exited);
    on_event(&e.state, &Event { kind: "terminal.removed".into(), project_id: None, data: json!({ "id": removed }), ts: T0 });
    turn_on(&e.state, &given, T0).unwrap();
    assert_eq!(list(&e.state).asked, [ended.clone(), asks.clone()]);
    sweep(&e.state, T0 + 11 * MINUTE);
    assert_eq!(list(&e.state).asked, [asks.clone()]);
    // It is a short memory: at most 64 sessions, the oldest forgotten first.
    for _ in 0..MAX_ASKED {
        gate(&e.state, &e.agent(json!({})), Call::Say, Via::Tools, T0).unwrap_err();
    }
    let asked = list(&e.state).asked;
    assert!(asked.len() == MAX_ASKED && !asked.contains(&asks));
}

// ---------------------------------------------------------------- the nudge

#[tokio::test]
async fn the_user_is_told_once_of_a_question_nobody_saw() {
    let e = env().await;
    let tid = e.with_avatar();
    e.design(&tid, json!({ "name": "Pip" }), T0).unwrap();
    let (ask, _waiter) = e.ask(&tid, "Shall I deploy to staging?", &["Deploy", "Wait"], T0).unwrap();
    let mut events = Events::of(&e);

    // Not before the question has been open for eight seconds.
    sweep(&e.state, T0 + NUDGE_AFTER_SECS * SECOND - 1);
    assert!(events.drain().is_empty());
    sweep(&e.state, T0 + NUDGE_AFTER_SECS * SECOND);
    let nudged = events.drain();
    assert_eq!(kinds(&nudged), ["avatar.nudge"]);
    assert_eq!((nudged[0].project_id.as_deref(), &nudged[0].data), (Some(e.pid.as_str()), &json!({ "terminalId": tid, "askId": ask.id, "name": "Pip", "question": "Shall I deploy to staging?" })));
    // Once per question.
    for later in [9, 20, 59] {
        sweep(&e.state, T0 + later * SECOND);
    }
    assert!(events.drain().is_empty());

    // Not when a device showed the question to someone: hidden avatars have not.
    let seen = e.with_avatar();
    let (asked, _waiter) = e.ask(&seen, "Seen?", &[], T0).unwrap();
    e.heard(&seen, &asked.line_id, Outcome::Hidden, T0 + 1);
    let shown = e.with_avatar();
    let (asked, _waiter2) = e.ask(&shown, "Shown?", &[], T0).unwrap();
    e.heard(&shown, &asked.line_id, Outcome::Shown, T0 + 1);
    // A voice that failed may have failed in a window nobody looks at: the user is told.
    let failed = e.with_avatar();
    let (asked, _waiter3) = e.ask(&failed, "Failed?", &[], T0).unwrap();
    e.heard(&failed, &asked.line_id, Outcome::Failed, T0 + 1);
    // Nor when it is closed by then.
    let closed = e.with_avatar();
    let (asked, _waiter4) = e.ask(&closed, "Closed?", &[], T0).unwrap();
    dismiss_ask(&e.state, &closed, &asked.id, T0 + 1).unwrap();
    events.drain();
    sweep(&e.state, T0 + MINUTE);
    let mut nudged: Vec<String> = events.drain().iter().filter(|ev| ev.kind == "avatar.nudge").map(|ev| format!("{}:{}", ev.data["terminalId"].as_str().unwrap(), ev.data["name"].as_str().unwrap())).collect();
    nudged.sort();
    let mut expected = vec![format!("{seen}:"), format!("{failed}:")];
    expected.sort();
    assert_eq!(nudged, expected);

    // The budget: three per terminal in ten minutes, however many questions it asks.
    let busy = e.with_avatar();
    let mut nudges = 0;
    let mut waiters = Vec::new();
    for i in 0..5_i64 {
        let at = T0 + i * 2 * MINUTE;
        let (_, waiter) = e.ask(&busy, &format!("Question {i}?"), &[], at).unwrap();
        waiters.push(waiter);
        events.drain();
        sweep(&e.state, at + 30 * SECOND);
        nudges += events.drain().iter().filter(|ev| ev.kind == "avatar.nudge").count();
        assert_eq!(nudges, (i as usize + 1).min(NUDGES_PER_10_MIN), "question {i}");
    }
    // Ten minutes after the first, there is room again: the question that is open is announced.
    sweep(&e.state, T0 + 10 * MINUTE + 31 * SECOND);
    let nudged = events.drain();
    assert_eq!(nudged.iter().filter(|ev| ev.kind == "avatar.nudge").map(|ev| ev.data["question"].as_str().unwrap()).collect::<Vec<_>>(), ["Question 4?"]);
}

#[tokio::test]
async fn a_question_being_spoken_counts_as_seen() {
    let e = env().await;
    let tid = e.with_avatar();
    let (ask, waiter) = e.ask(&tid, "Shall I deploy the new build to staging now?", &["Deploy", "Wait"], T0).unwrap();
    // A voice tab took the question up and speaks it: it says what became of it at the end only.
    let claimed = e.claim(&tid, &ask.line_id, "voice-tab", T0);
    assert!(claimed.speak);
    let waited = tokio::time::timeout(Duration::from_secs(10), wait_answer_within(waiter, Duration::from_millis(60), Duration::from_millis(20))).await.unwrap();
    assert_eq!(waited.reason, Some(Reason::Timeout), "{:?}", waited.note);
    // Nor is the user told elsewhere of a question they are hearing.
    let mut events = Events::of(&e);
    sweep(&e.state, T0 + NUDGE_AFTER_SECS * SECOND + 1);
    assert!(events.drain().is_empty());
    // The lease lapsed and still no word of the question: now the user is told.
    sweep(&e.state, T0 + claimed.lease_ms + 1);
    assert_eq!(events.kinds(), ["avatar.nudge"]);
}

// ---------------------------------------------------------------- the transcript and shutdown

#[tokio::test]
async fn the_transcript_keeps_the_last_thirty_lines() {
    let e = env().await;
    let tid = e.with_avatar();
    let mut first = String::new();
    for i in 0..(MAX_LINES as i64 + 5) {
        // One line every eleven seconds keeps to the rate.
        let at = T0 + i * 11 * SECOND;
        let line = e.say(&tid, &format!("Line {i}."), at).unwrap().line.id;
        e.heard(&tid, &line, Outcome::Shown, at);
        if i == 0 {
            first = line;
        }
    }
    let lines = e.avatar(&tid).lines;
    assert_eq!((lines.len(), lines[0].text.as_str(), lines[MAX_LINES - 1].text.as_str()), (MAX_LINES, "Line 5.", "Line 34."));
    // A line that left the transcript is nobody's any more.
    assert_eq!(claim(&e.state, &tid, &first, ClaimRequest { tab: "tab-A".into(), release: false }, T0).unwrap_err().status.as_u16(), 404);
    assert_eq!(heard(&e.state, &tid, &first, HeardRequest { outcome: Outcome::Spoken, voice: None }, T0).unwrap_err().status.as_u16(), 404);
    // Records are listed oldest first.
    let later = e.agent(json!({}));
    turn_on(&e.state, &later, T0 + 5).unwrap();
    assert_eq!(list(&e.state).avatars.iter().map(|a| a.terminal_id.as_str()).collect::<Vec<_>>(), [tid.as_str(), later.as_str()]);
}

#[tokio::test]
async fn an_open_question_keeps_its_line_in_the_transcript() {
    let e = env().await;
    let tid = e.with_avatar();
    // The user saw the question; its call stopped waiting, and it stays open.
    let ask = e.asked(&tid, "Shall I tag the release?", &["Tag it", "Not yet"], T0).await;
    e.heard(&tid, &ask.line_id, Outcome::Shown, T0);
    // The session goes on talking through its avatar, one line every eleven seconds.
    for i in 1..=MAX_LINES as i64 {
        let at = T0 + i * 11 * SECOND;
        let line = e.say(&tid, &format!("Line {i}."), at).unwrap().line.id;
        e.heard(&tid, &line, Outcome::Shown, at);
    }
    // The oldest lines go, the question's own stays while it is open.
    let lines = e.avatar(&tid).lines;
    assert_eq!((lines.len(), lines[0].id.as_str(), lines[1].text.as_str(), lines[MAX_LINES - 1].text.as_str()), (MAX_LINES, ask.line_id.as_str(), "Line 2.", "Line 30."));

    // So it still counts as seen: nobody is told of it elsewhere, and a call that waits again
    // sits out its wait.
    let later = T0 + (MAX_LINES as i64 + 1) * 11 * SECOND;
    let mut events = Events::of(&e);
    sweep(&e.state, later);
    assert!(events.drain().is_empty());
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, later).unwrap();
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, later).unwrap();
    let waited = tokio::time::timeout(Duration::from_secs(10), wait_answer_within(waiter, Duration::from_millis(60), Duration::from_millis(20))).await.unwrap();
    assert_eq!(waited.reason, Some(Reason::Timeout), "{:?}", waited.note);
    // A device still finds its line.
    heard(&e.state, &tid, &ask.line_id, HeardRequest { outcome: Outcome::Spoken, voice: None }, later).unwrap();

    // The answer to it joins a full transcript: the question's line stays beside its answer and
    // the oldest other line goes, as a browser that hears of the answer's line first has it too.
    let pass = gate(&e.state, &tid, Call::Ask, Via::Tools, later + 1).unwrap();
    let (_, waiter) = ask_again(&e.state, &pass, &ask.id, later + 1).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    tokio::task::yield_now().await;
    let replied = reply(&e.state, &tid, clicked(&ask.id, 0), later + 2).await.unwrap();
    assert_eq!(replied.delivered, Delivered::Answer);
    assert!(waiting.await.unwrap().answered);
    let lines = e.avatar(&tid).lines;
    assert_eq!((lines.len(), lines[0].id.as_str(), lines[1].text.as_str(), lines[MAX_LINES - 1].text.as_str()), (MAX_LINES, ask.line_id.as_str(), "Line 3.", "Tag it"));
}

#[tokio::test]
async fn when_the_server_stops_every_call_that_waits_returns() {
    let e = env().await;
    let (a, b) = (e.with_avatar(), e.with_avatar());
    let (ask_a, waiter_a) = e.ask(&a, "Shall I?", &[], T0).unwrap();
    let (_, waiter_b) = e.ask(&b, "May I?", &[], T0).unwrap();
    let said = e.say(&a, "Working.", T0 + 1).unwrap();
    let waits = (tokio::spawn(wait_answer_within(waiter_a, LONG, LONG)), tokio::spawn(wait_answer_within(waiter_b, LONG, LONG)));
    let state = e.state.clone();
    let speaking = tokio::spawn(async move { say_wait_within(&state, said, SayWait { soft: LONG, most: LONG, held: LONG }).await });
    tokio::task::yield_now().await;

    wake_all(&e.state);
    let ended = tokio::time::timeout(Duration::from_secs(10), async { (waits.0.await.unwrap(), waits.1.await.unwrap(), speaking.await.unwrap()) }).await.unwrap();
    assert_eq!((ended.0.reason, ended.0.ask_id.as_deref(), ended.1.reason), (Some(Reason::SessionEnded), Some(ask_a.id.as_str()), Some(Reason::SessionEnded)));
    assert_eq!(ended.2.outcome, Some(Outcome::Unheard));
    // A call that comes now does not wait either.
    let pass = gate(&e.state, &a, Call::Ask, Via::Tools, T0 + 2).unwrap();
    let (_, late) = ask_again(&e.state, &pass, &ask_a.id, T0 + 2).unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), wait_answer_within(late, LONG, LONG)).await.unwrap().reason, Some(Reason::SessionEnded));
}

// ---------------------------------------------------------------- config

#[tokio::test]
async fn a_repository_cannot_change_who_has_an_avatar() {
    // `[avatar]` is config.toml's alone: in a `.workbench.toml` it is not a section at all. Here
    // config.toml says that the user decides; the repository's file asks for the opposite.
    let e = env().await;
    std::fs::write(e.root.join(".workbench.toml"), "[avatar]\nenabled = false\nsessions = \"any\"\n\n[agent]\nmodel = \"opus\"\navatar = true\n").unwrap();
    e.state.projects.reload(&e.state).await;
    let project = e.state.projects.require(&e.pid).unwrap();
    assert_eq!(project.config.agent.model.as_deref(), Some("opus"), "the repository's file was read");
    assert!(super::enabled(&e.state) && super::sessions(&e.state) == Sessions::Asked);
    assert_eq!(e.state.config.read().avatar, super::AvatarConfig { enabled: None, sessions: Some(Sessions::Asked) });
    let tid = e.agent(json!({}));
    assert_eq!(gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap_err().message, NO_AVATAR);
    assert!(list(&e.state).enabled && list(&e.state).avatars.is_empty());

    // config.toml's own section decides.
    e.state.config.write().avatar = super::AvatarConfig { enabled: Some(true), sessions: Some(Sessions::Any) };
    gate(&e.state, &tid, Call::Say, Via::Tools, T0).unwrap();
    assert_eq!(serde_json::to_value(list(&e.state)).unwrap()["sessions"], "any");
    assert_eq!(serde_json::to_value(super::AvatarConfig::default()).unwrap(), json!({}));
}

/// Without an `[avatar]` table avatars are on and a session takes one itself, with its first
/// call. What the user turned off stays off: the session does not turn it on again.
#[tokio::test]
async fn by_default_a_session_takes_an_avatar_itself_and_the_user_can_take_it_away() {
    let e = env_with(|cfg| cfg.avatar = super::AvatarConfig::default()).await;
    assert!(super::enabled(&e.state) && super::sessions(&e.state) == Sessions::Any);
    let tid = e.agent(json!({}));
    assert!(!super::active(&e.state, &tid));
    let pass = gate(&e.state, &tid, Call::Status, Via::Tools, T0).unwrap();
    let report = status(&e.state, &pass, StatusRequest::default()).unwrap();
    assert!(report.allowed && report.avatar.is_none() && report.note.contains("gets one with its first"), "{}", report.note);
    gate(&e.state, &tid, Call::Say, Via::Tools, T0 + 1).unwrap();
    assert!(super::active(&e.state, &tid) && list(&e.state).asked.is_empty());

    turn_off(&e.state, &tid, T0 + 2).unwrap();
    assert!(!super::active(&e.state, &tid));
    for call in [Call::Say, Call::Ask, Call::Design, Call::Act] {
        assert_eq!(gate(&e.state, &tid, call, Via::Tools, T0 + 3).unwrap_err().message, TURNED_OFF, "{call:?}");
    }
    assert!(!super::active(&e.state, &tid));
}

// ---------------------------------------------------------------- a real session

/// The one test with a process behind the session: a custom CLI that prints `ready` and echoes
/// what it reads. While it starts, the session takes no reply; once it is idle, an answer given
/// after the call stopped waiting arrives on its screen as a `[via avatar…]` prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_after_the_wait_arrives_on_the_screen_of_a_real_session() {
    let mut program = crate::util::os::exe::python();
    // It takes its time to come up, so the session is still starting when the first reply is tried.
    program.extend(["-u".to_string(), "-c".to_string(), "import sys, time\ntime.sleep(2)\nprint('ready', flush=True)\nfor line in sys.stdin:\n    print('got ' + line.strip(), flush=True)".to_string()]);
    let scripted = crate::config::global::ProviderConfig { command: Some(program.remove(0)), args: program, label: Some("Scripted".into()), ..Default::default() };
    let e = env_with(|cfg| {
        cfg.agents.providers.insert("scripted".into(), scripted);
    })
    .await;
    crate::terminals::start(&e.state).await;
    // The slice's own background work as well: real terminal events reach it over the bus.
    super::start(&e.state).await;
    let token = e.state.auth.master_token().to_string();
    let (status, started) = e.send(Method::POST, "/api/agents", &token, Some(json!({ "projectId": e.pid, "provider": "scripted" }))).await;
    assert_eq!(status, 200, "{started}");
    let tid = started["id"].as_str().unwrap().to_string();
    let now = crate::util::now_ms();

    // A device gives the session an avatar; it asks, and stops waiting.
    assert!(turn_on(&e.state, &tid, now).unwrap().brief);
    let pass = gate(&e.state, &tid, Call::Ask, Via::Command, now).unwrap();
    let (asked, waiter) = ask(&e.state, &pass, question("Shall I run the migration?", &["Run it", "Not now"]), now).unwrap();
    assert_eq!(wait_answer_within(waiter, Duration::ZERO, LONG).await.reason, Some(Reason::Timeout));

    // While the session starts it takes no prompt: the refusal of the ask path is passed on,
    // nothing is recorded and the question stays open.
    assert_eq!(e.terminal(&tid).agent.unwrap().state, AgentState::Starting);
    let refused = reply(&e.state, &tid, clicked(&asked.id, 0), now).await.unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code), (409, "conflict"), "{}", refused.message);
    assert!(refused.message.contains("answer it first"), "{}", refused.message);
    let avatar = e.avatar(&tid);
    assert_eq!((avatar.ask.map(|a| a.id), avatar.lines.iter().filter(|l| l.from == Speaker::User).count()), (Some(asked.id.clone()), 0));

    // Once it is idle the answer goes in, as one line that names the question and holds none of it.
    let idle = tokio::time::Instant::now() + Duration::from_secs(90);
    while e.terminal(&tid).agent.unwrap().state != AgentState::Idle {
        assert!(tokio::time::Instant::now() < idle, "the session never became idle");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let replied = reply(&e.state, &tid, clicked(&asked.id, 0), crate::util::now_ms()).await.unwrap();
    assert_eq!(replied.delivered, Delivered::Prompt);
    let prompt = format!("[via avatar, answer with your avatar say command] Answer to question {}: choice 1, \"Run it\"", asked.id);
    let seen = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let screen = e.state.terminals.screen_text(&tid, 40).unwrap_or_default();
        if screen.contains(&format!("got {prompt}")) {
            assert!(!screen.contains("migration"), "the question itself is never pasted: {screen}");
            break;
        }
        assert!(tokio::time::Instant::now() < seen, "the answer never arrived: {screen}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let avatar = e.avatar(&tid);
    assert!(avatar.ask.is_none());
    assert_eq!(avatar.lines.last().map(|l| (l.from, l.text.as_str(), l.ask_id.as_deref(), l.via)), Some((Speaker::User, "Run it", Some(asked.id.as_str()), Some(ReplyVia::Click))));

    // The session ends while a call waits for another answer: the terminals slice announces the
    // exit, and the call returns.
    let now = crate::util::now_ms();
    let pass = gate(&e.state, &tid, Call::Ask, Via::Command, now).unwrap();
    let (_, waiter) = ask(&e.state, &pass, question("And the next one?", &[]), now).unwrap();
    let waiting = tokio::spawn(wait_answer_within(waiter, LONG, LONG));
    e.state.terminals.kill(&tid).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(60), waiting).await.expect("the call returned").unwrap();
    assert_eq!(ended.reason, Some(Reason::SessionEnded));
    let gone = tokio::time::Instant::now() + Duration::from_secs(60);
    while e.avatar(&tid).lines.iter().any(|l| l.from == Speaker::Agent && l.outcome.is_none()) {
        assert!(tokio::time::Instant::now() < gone, "what the session said stayed pending");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(gate(&e.state, &tid, Call::Say, Via::Command, crate::util::now_ms()).unwrap_err().message, NOT_A_SESSION);
    super::shutdown(&e.state).await;
}

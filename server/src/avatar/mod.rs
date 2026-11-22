//! Avatar slice (OWNER: avatar slice).
//!
//! A hosted agent session can have a live avatar: a small animated character over the Workbench
//! window that the session draws itself, that speaks its short messages with captions, asks the
//! user questions and takes the answer, and mirrors the session's state.
//!
//! An avatar belongs to one session (a terminal of kind agent). By default (`[avatar] sessions =
//! "any"`) a session has one from its first avatar call on, or when it was started with the Avatar
//! option, by the user or by a session that starts it; with `"asked"` only when it was started
//! with the option by the user or a device gave it one. The user turns any avatar off, and a
//! session never turns one on again. Everything a session sends is untrusted: artwork becomes a
//! closed, typed tree (`look`), text one line of plain text with the session's secrets masked
//! (`text`), and every size, count, rate and wait has a named limit (`store`).
//!
//! Modules: `text` (the one function for a line of plain text), `look` (pure: the artwork parsed
//! into a closed, typed, bounded tree, its pivots, parents, roles, animations, drives, mouth,
//! gaze and voice validated and normalised, boxes, the geometry lint; the wire types of a look;
//! its limits), `preview` (a look drawn as a PNG in one pose or as moments of an animation, so a
//! session sees what it drew: the stored look read again as untrusted data and rendered here),
//! `store` (`AvatarState`: the records, their transcript, open question and leases in memory,
//! the files under `data_dir/avatar/`, what terminal events do to them, the sweep), `session`
//! (what a session may do with its own avatar: the gate, design, say, ask, act, status, and the
//! words every answer uses), `device` (what only the user's devices do: claim and acknowledge
//! speech, reply, dismiss a question, turn an avatar on or off, reset it; and telling a session
//! of its avatar, for a device and for the lifecycle), and the ways in:
//! `routes` (the device routes and the session's hook routes), `tools` (the MCP tools), `cli`
//! (the `workbench avatar` command), `brief` (what a session is told: the brief that joins its
//! first prompt, and the guide of `guide.md`, `guide_tools.md` and `guide_command.md`).
//!
//! The state functions are one implementation behind every way in: an MCP tool, a hook route and
//! the command call the same function and answer with the same words. Timed logic takes its clock
//! as a value (`now` in milliseconds, a `Duration` for a wait), so its tests need not wait for
//! time to pass.
//!
//! Who may do what. A session acts on its own avatar only: a tool takes no terminal id, and a
//! hook route accepts only the agent token of the terminal in its path (the master token is
//! refused there). What answers for the user (a reply, dismissing a question, turning an avatar
//! on or off, resetting it, claiming and acknowledging speech) is for signed-in devices only:
//! the master token and in-process calls get 403, agent tokens 401. Nothing under the hook
//! routes puts text into a session; what a device's reply pastes is a fixed frame, the
//! question's id and the user's own words or the choice that was clicked.
//!
//! Routes: `GET /api/avatar`, `GET /api/avatar/{terminalId}` (whoever is signed in or holds
//! the token); `POST /api/avatar/{terminalId}/lines/{lineId}/claim|heard`,
//! `POST /api/avatar/{terminalId}/reply|on|off|reset`,
//! `POST /api/avatar/{terminalId}/asks/{askId}/dismiss` (devices only);
//! `GET /api/hooks/avatar/{terminalId}/guide|status`,
//! `POST /api/hooks/avatar/{terminalId}/design|say|ask|act` (the session's own agent token).
//! Events: `avatar.changed`, `avatar.say`, `avatar.ask`, `avatar.line`, `avatar.act`,
//! `avatar.nudge`, each with the terminal's project on the envelope.
//! MCP tools: `avatar_status`, `avatar_design`, `avatar_say`, `avatar_ask`, `avatar_act`.

mod brief;
mod cli;
pub mod device;
pub mod look;
mod preview;
mod routes;
pub mod session;
pub mod store;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_http;
pub mod text;
mod tools;

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::mcp::McpTool;
use crate::terminals::ProviderKind;

// For the terminals slice, which builds a session's first prompt and its settings, and keeps
// the brief out of titles and History rows.
pub use brief::{BRIEF_MARK, BRIEF_TAGS, Brief, BriefVariant, PREALLOWED, session_brief};
pub use cli::{ABOUT, AvatarArgs, cli};
pub use routes::router;
pub use store::AvatarState;

/// Whether a look can be drawn within what a picture may cost, as a design call asks before it
/// stores one: for the tests that hold documented examples to what a session may send.
#[cfg(test)]
pub fn affordable(look: &look::Look) -> Result<(), String> {
    preview::affordable(&serde_json::to_value(look).map_err(|e| e.to_string())?)
}

/// The slice's MCP tools: `avatar_status`, `avatar_design`, `avatar_say`, `avatar_ask`,
/// `avatar_act`. Each acts on the calling session's own avatar.
pub fn mcp_tools() -> Vec<McpTool> {
    tools::tools()
}

/// How often lines are looked at for staleness, held gestures for their end and open questions
/// for a nudge.
const SWEEP_EVERY: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- config

/// `[avatar]` in config.toml. Repository config never reaches it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AvatarConfig {
    /// `false`: no session has an avatar; tools, hooks and the command answer 403. Default `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Which sessions have one. Default `any`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions: Option<Sessions>,
}

/// Which sessions have an avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sessions {
    /// Only a session the user gave one: started with the Avatar option, or given one from a device.
    Asked,
    /// Any session, from its first avatar call on, and a session that another session started with
    /// one (`workbench_start_session`). The default: the user can still turn any avatar off, and
    /// a session never turns one back on.
    Any,
}

/// Whether sessions may have avatars at all (`[avatar] enabled`).
pub fn enabled(state: &AppState) -> bool {
    state.config.read().avatar.enabled.unwrap_or(true)
}

/// Which sessions have an avatar (`[avatar] sessions`).
pub fn sessions(state: &AppState) -> Sessions {
    state.config.read().avatar.sessions.unwrap_or(Sessions::Any)
}

// ---------------------------------------------------------------- how a session drives its avatar

/// How a session drives its avatar: with Workbench's MCP tools, or with a command it runs
/// (`workbench avatar` on the host, `curl` against the hook routes in a dev container).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Tools,
    Command,
}

/// How a session of this kind drives its avatar: the sessions that get Workbench's MCP tools
/// (Claude Code anywhere, Codex on the host) use them, every other one runs a command. The one
/// answer for a record's first `via`, the session brief and the composer.
pub fn via_for(kind: ProviderKind, in_container: bool) -> Via {
    match kind {
        ProviderKind::Claude => Via::Tools,
        ProviderKind::Codex if !in_container => Via::Tools,
        _ => Via::Command,
    }
}

// ---------------------------------------------------------------- what the terminals slice asks

/// Whether the session has an avatar that is on, while avatars are enabled. After a session was
/// created its record is the truth, not the option it was started with: a restart with a new
/// conversation briefs the session again, and pre-allows its tools, exactly when this says yes.
/// A session started with the option whose first prompt is a command of its CLI is started
/// without the brief and with `"avatarBrief": "later"` beside `"avatar": true` in its meta: it is
/// told once it is idle.
///
/// It may read the stored avatars on first use, which asks the terminals slice for each
/// terminal: never call it with a terminal's record lock held.
pub fn active(state: &AppState, terminal_id: &str) -> bool {
    if !enabled(state) {
        return false;
    }
    store::ensure_loaded(state);
    state.avatar.inner.lock().records.get(terminal_id).is_some_and(|rec| rec.on)
}

/// A NEW conversation of the session started (a restart that does not continue the earlier
/// one): `told` says whether the brief went with it. The record, on or off, remembers whether the
/// session is still to be told of its avatar; if so, turning it on tells it, and so does the
/// lifecycle once the session is idle with its avatar on. A terminal without an avatar is left
/// as it is.
///
/// It takes the store lock only and asks no other slice, except that, like [`active`], it may
/// read the stored avatars on first use: never call it with a terminal's record lock held.
pub fn conversation_started(state: &AppState, terminal_id: &str, told: bool) {
    store::conversation_started(state, terminal_id, told);
}

/// What a session that moves to a new terminal takes along of its avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    /// False: the user turned it off. The new session is told nothing and nothing is pre-allowed.
    pub on: bool,
    /// It has a look of its own: the new session keeps it. False: it designs one.
    pub designed: bool,
}

/// The avatar of the terminal a session moves away from, for the new session's brief. `None`:
/// that terminal has none (or avatars are switched off), and the request's own option decides,
/// as for a fresh start.
///
/// Like [`active`], never call it with a terminal's record lock held.
pub fn carried(state: &AppState, old_terminal_id: &str) -> Option<Carried> {
    if !enabled(state) {
        return None;
    }
    store::ensure_loaded(state);
    state.avatar.inner.lock().records.get(old_terminal_id).map(|rec| Carried { on: rec.on, designed: rec.designed() })
}

// ---------------------------------------------------------------- start / shutdown

/// Background work, after `terminals::start`: the stored avatars are read and compared with the
/// terminals that exist, then terminal events keep the records in step and a ticker runs the sweep.
pub async fn start(state: &AppState) {
    // Subscribed before the records are read: nothing that happens meanwhile is missed.
    let mut events = state.events.subscribe();
    let st = state.clone();
    let _ = tokio::task::spawn_blocking(move || store::reconcile(&st, crate::util::now_ms())).await;

    let st = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => store::sweep(&st, crate::util::now_ms()),
                event = events.recv() => match event {
                    Ok(event) => store::on_event(&st, &event),
                    // Events were missed: look at every terminal again.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => store::reconcile(&st, crate::util::now_ms()),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    });
}

/// Before `terminals::shutdown`: every call that waits for an answer returns (`session_ended`),
/// so no held request keeps the server from stopping.
pub async fn shutdown(state: &AppState) {
    store::wake_all(state);
}

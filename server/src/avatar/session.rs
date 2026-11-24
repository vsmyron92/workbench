//! What a session may do with its own avatar (OWNER: avatar slice).
//!
//! Every way in (an MCP tool, a hook route, the `workbench avatar` command) calls the functions
//! of this file, and every answer's words come from here, so they all say the same.
//!
//! A call has two steps. [`gate`] comes first: avatars are enabled, the caller is a running
//! agent session, it has an avatar (or gets one, with `[avatar] sessions = "any"`) that is on,
//! and it keeps to the rate of its kind of call. It needs no arguments, so a hook route asks it
//! before it parses a body, and an attempt over the limit costs nothing and emits nothing. What
//! it returns is the [`Pass`] the second step takes: [`design`], [`say`], [`ask`], [`act`] or
//! [`status`], each of which validates its whole request before anything changes.
//!
//! Text a session sends is one line of plain text with the session's secrets masked
//! (`text::plain` with `Terminals::redact_text`), cleaned before the store lock is taken.

use std::time::Duration;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::look::{self, Applied, Design, Look, Prop, Voice};
use super::store::{
    ACT_GAP_MS, ACT_PER_MINUTE, ASK_CALLS_PER_MINUTE, Answer, Ask, AskEnd, Clock, DEFAULT_WAIT_SECS, DESIGN_PER_MINUTE, HOLD_MAX_SECS, Held, HeldMode, Inner, LINE_STALE_SECS, Line, MAX_ACT_TIMES, MAX_ASK_CHARS, MAX_ASKED, MAX_AVATARS,
    MAX_CHOICE_CHARS, MAX_CHOICES, MAX_LINES, MAX_NAME_CHARS, MAX_PENDING_LINES, MAX_REPLY_CHARS, MAX_SAY_CHARS, MAX_WAIT_HOOK_SECS, MAX_WAIT_TOOL_SECS, OpenAsk, Outcome, Reason, Record, ReplyVia, SAY_PER_MINUTE,
    SAY_WAIT_EXTRA_SECS, SAY_WAIT_SOFT_MS, SPEECH_CHARS_PER_SEC, STATUS_LINES, STATUS_PER_MINUTE, Speaker, UNSEEN_SECS, Windows, emit_act, emit_ask, emit_changed, emit_say, ensure_loaded, next_id, running_agent, save,
};
use super::{Sessions, Via, builtin, draw, preview, text};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::terminals::AgentState;

// ---------------------------------------------------------------- the words of a refusal

/// 403: `[avatar] enabled = false`.
pub const SWITCHED_OFF: &str = "avatars are switched off in Settings";
/// 400: the caller is not a running agent session.
pub const NOT_A_SESSION: &str = "The avatar calls act on the calling session's own avatar: make them from a running Workbench agent session.";
/// 409: the session has no avatar and may not give itself one.
pub const NO_AVATAR: &str = "This session has no avatar. The user gives a session one with the Avatar option or the command Avatar: Give This Session an Avatar. Say it in the terminal instead.";
/// 409: a session asked for an avatar for the session it starts, where the user decides who has one.
pub const NOT_YOURS_TO_GIVE: &str = "Not started: here the user decides which sessions have an avatar ([avatar] sessions = \"asked\"). Start it without avatar; the user can give it one.";
/// 409: a session whose own avatar the user turned off asked for an avatar for the session it starts.
pub const NOT_WITH_YOURS_OFF: &str = "Not started: this session's avatar is off, and only the user turns it on, so it does not give one to a session it starts. Start it without avatar.";
/// 409: the user turned the avatar off.
pub const TURNED_OFF: &str = "The user turned this session's avatar off. Say it in the terminal instead.";
/// 409: the avatar is off because its stored file could not be read at a start (nobody knows what
/// the user chose), until the user turns it on.
pub const UNREADABLE: &str = "Workbench could not read this avatar's stored file, so it is off until the user turns it on. Say it in the terminal instead.";

/// What a session is told of its avatar that is off: why it is off.
pub(super) fn off_words(rec: &Record) -> &'static str {
    if rec.unreadable { UNREADABLE } else { TURNED_OFF }
}

/// 409: the session needs a permission or an answer; its avatar says and asks nothing meanwhile.
pub const QUIET: &str = "a permission request or question is open in the session: the avatar is quiet until it is answered";

/// 429: a session would take an avatar itself while [`MAX_AVATARS`] sessions have one. A status
/// call says the same, so a session is not told it gets one that it cannot get.
fn no_room() -> String {
    format!("No avatar: {MAX_AVATARS} sessions have one already. Say it in the terminal instead.")
}

/// The standing instructions `avatar_status` begins with, by how the session drives its avatar.
const NOTE_TOOLS: &str = "You have a live avatar: say the short version with avatar_say, ask with avatar_ask.";
const NOTE_COMMAND: &str = "You have a live avatar: say the short version with your avatar say command, ask with your avatar ask command.";
const NOTE_ANY: &str = "This session has no avatar yet: it gets one with its first design, say, ask or act call.";

/// What follows a line that will not sound like speech.
const HINT_LITERAL: &str = "It will be read out literally: use plain words.";

/// The built-in motion that repeats while it plays: what mode loop accepts besides the look's
/// own animations declared `loop: true`.
const BUILTIN_LOOPS: [&str; 6] = ["breathe", "think", "talk", "listen", "alert", "doze"];

/// Bytes a text may have for each character of its limit before it is looked at: what is
/// removed (control characters, joiners) does not count against the limit, but has a bound.
pub(super) const RAW_BYTES_PER_CHAR: usize = 16;

/// The refusal for a caller that is not a session (a tool called without a terminal).
pub fn not_a_session() -> ApiError {
    ApiError::bad_request(NOT_A_SESSION)
}

fn too_many(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
}

/// The refusal for a call that would have to wait too long for something all sessions share.
pub(super) fn too_busy(message: impl Into<String>) -> ApiError {
    too_many(message)
}

pub(super) fn not_pending(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "not_pending", message)
}

pub(super) fn busy(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "busy", message)
}

fn answer_on_its_way(ask_id: &str) -> ApiError {
    busy(format!("The user's answer to question {ask_id} is being delivered to this session now: read it as the next message."))
}

// ---------------------------------------------------------------- the gate

/// The kinds of call a session makes. Each has a rate of its own; a new question also counts
/// as a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    Status,
    Design,
    Say,
    Ask,
    Act,
}

/// A call that passed the gate. Only [`gate`] makes one, for one kind of call.
#[derive(Debug)]
pub struct Pass {
    tid: String,
    call: Call,
    via: Via,
    /// The session's state when it was let in.
    session: Option<AgentState>,
    /// False only for a status call of a session without an avatar.
    avatar: bool,
}

impl Pass {
    #[allow(dead_code)] // read by tests alone so far
    pub fn terminal_id(&self) -> &str {
        &self.tid
    }

    /// The session's secret masking, for a refusal that quotes the call's arguments.
    pub(super) fn mask<'p>(&'p self, state: &'p AppState) -> impl Fn(&str) -> String + 'p {
        mask_of(state, &self.tid)
    }

    /// The pass was given for this kind of call: its rate is the one that was counted.
    fn is_for(&self, call: Call) -> ApiResult<()> {
        if self.call == call { Ok(()) } else { Err(ApiError::internal(format!("a {call:?} call was let in as a {:?} call", self.call))) }
    }

    /// Rule 6: while the session needs a permission or an answer its avatar says and asks nothing.
    fn speaks(&self) -> ApiResult<()> {
        match self.session {
            Some(AgentState::NeedsPermission | AgentState::NeedsInput) => Err(ApiError::conflict(QUIET)),
            _ => Ok(()),
        }
    }
}

/// The gate every call of a session passes first. `via` is how the caller reached Workbench: the
/// record's `via` follows the last interface used.
///
/// In this order: avatars are switched off (403); the caller is not a running agent session
/// (400); it has no avatar: with `sessions = "any"` it gets one (the one its stored file holds,
/// when it has one), otherwise it is remembered as having asked and refused (409); its avatar is
/// off (409: the user turned it off, or its stored file could not be read); it made more calls
/// of this kind in the last minute than the kind allows (429). A status call passes the first
/// two checks and the rate alone, and reports the rest.
pub fn gate(state: &AppState, tid: &str, call: Call, via: Via, now: i64) -> ApiResult<Pass> {
    ensure_loaded(state);
    if !super::enabled(state) {
        return Err(ApiError::forbidden(SWITCHED_OFF));
    }
    // Asked before the store lock is taken. `tid` may be any text: nothing is made of it before
    // the terminals slice knows it.
    let info = state.terminals.info(tid).filter(running_agent).ok_or_else(not_a_session)?;
    let session = info.agent.as_ref().map(|a| a.state);
    let any = super::sessions(state) == Sessions::Any;

    let mut guard = state.avatar.inner.lock();
    let Inner { records, asked, limits, .. } = &mut *guard;
    if !records.contains_key(tid) {
        if call == Call::Status {
            charge(limits, tid, call, now)?;
            return Ok(Pass { tid: tid.to_string(), call, via, session, avatar: false });
        }
        if !any {
            if !asked.iter().any(|id| id == tid) {
                asked.push_back(tid.to_string());
                while asked.len() > MAX_ASKED {
                    asked.pop_front();
                }
                // Who asked is part of what the browsers show: they read the list again.
                state.events.emit("avatar.changed", info.project_id.as_deref(), json!({ "terminalId": tid, "revision": 0, "on": false }));
            }
            return Err(ApiError::conflict(NO_AVATAR));
        }
        if records.len() >= MAX_AVATARS {
            return Err(too_many(no_room()));
        }
        // A stored file of its own is this session's avatar, as a start reads it: never replaced
        // by a new one. One the user turned off stays off, and this call is refused below.
        let rec = match super::store::take_over(state, &info, now) {
            Some(rec) => rec,
            None => {
                let mut rec = Record::new(&info, now);
                rec.via = via;
                save(state, tid, &rec);
                rec
            }
        };
        emit_changed(state, tid, &rec);
        asked.retain(|id| id != tid);
        records.insert(tid.to_string(), rec);
    }
    let rec = records.get_mut(tid).ok_or_else(|| ApiError::conflict(NO_AVATAR))?;
    rec.project_id = info.project_id.clone();
    if !rec.on && call != Call::Status {
        return Err(ApiError::conflict(off_words(rec)));
    }
    charge(limits, tid, call, now)?;
    if rec.on && rec.via != via {
        rec.via = via;
        rec.touch(now);
        save(state, tid, rec);
        emit_changed(state, tid, rec);
    }
    Ok(Pass { tid: tid.to_string(), call, via, session, avatar: true })
}

/// Counts an attempt of this kind of call. Over the limit: 429, with what to do.
fn charge(limits: &mut std::collections::HashMap<String, Windows>, tid: &str, call: Call, now: i64) -> ApiResult<()> {
    let windows = limits.entry(tid.to_string()).or_default();
    let (window, most) = match call {
        Call::Say => (&mut windows.say, SAY_PER_MINUTE),
        Call::Ask => (&mut windows.ask, ASK_CALLS_PER_MINUTE),
        Call::Design => (&mut windows.design, DESIGN_PER_MINUTE),
        Call::Act => (&mut windows.act, ACT_PER_MINUTE),
        Call::Status => (&mut windows.status, STATUS_PER_MINUTE),
    };
    if window.attempt(now, most) {
        return Ok(());
    }
    Err(too_many(match call {
        Call::Say => format!("Not said: at most {most} lines and questions a minute. Speak less often."),
        Call::Ask => format!("At most {most} ask calls a minute: wait longer in one call instead (waitSeconds)."),
        Call::Design => format!("Nothing was changed: at most {most} design calls a minute. Wait a minute, then send one complete call."),
        Call::Act => format!("Not played: at most {most} gestures a minute."),
        Call::Status => format!("At most {most} status calls a minute: go on with what the last answer said."),
    }))
}

/// The caller's record, for a call that passed the gate a moment ago: it may have been removed
/// or turned off since.
fn on_record<'r>(records: &'r mut std::collections::HashMap<String, Record>, tid: &str) -> ApiResult<&'r mut Record> {
    let rec = records.get_mut(tid).ok_or_else(|| ApiError::conflict(NO_AVATAR))?;
    if rec.on { Ok(rec) } else { Err(ApiError::conflict(TURNED_OFF)) }
}

// ---------------------------------------------------------------- text and names

/// `s` as one line of plain text of at most `most` characters, the session's secrets masked.
/// `what` names it in a refusal: "the line has 412 characters: at most 400".
fn clean(state: &AppState, tid: &str, what: &str, s: &str, most: usize) -> ApiResult<String> {
    if s.len() > most * RAW_BYTES_PER_CHAR {
        return Err(ApiError::bad_request(format!("{what} is {} bytes long: at most {most} characters", s.len())));
    }
    match text::plain(s, most, |t| state.terminals.redact_text(tid, t)) {
        Ok(t) if t.is_empty() => Err(ApiError::bad_request(format!("{what} is empty: write what the user should read"))),
        Ok(t) => Ok(t),
        Err(refusal) => Err(ApiError::bad_request(format!("{what} has {refusal}"))),
    }
}

/// A language tag as a browser voice takes it: `[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8}){0,2}`.
fn lang_of(lang: Option<String>, mask: &dyn Fn(&str) -> String) -> ApiResult<Option<String>> {
    let Some(lang) = lang.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()) else { return Ok(None) };
    let mut parts = lang.split('-');
    let primary = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    let ok = (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && rest.len() <= 2
        && rest.iter().all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if ok { Ok(Some(lang)) } else { Err(ApiError::bad_request(format!("lang {} is not a language tag: write one like en-GB or de", shown(&lang, mask)))) }
}

/// A name or id the caller sent, for a message: as written when it is a plain word, quoted and
/// cut otherwise. `mask` is the session's secret masking ([`mask_of`]), or [`unmasked`] for
/// what a device sent: it runs before the cut, so that no piece of a long secret is left over
/// that no mask would know any more (the platform masks a refusal again, whole).
pub(super) fn shown(s: &str, mask: &dyn Fn(&str) -> String) -> String {
    let s = mask(s);
    let plain = !s.is_empty() && s.len() <= 48 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if plain { s } else { format!("{:?}", s.chars().take(40).collect::<String>()) }
}

/// The session's secret masking, as `text::plain` and [`shown`] take it.
pub(super) fn mask_of<'s>(state: &'s AppState, tid: &'s str) -> impl Fn(&str) -> String + 's {
    move |s| state.terminals.redact_text(tid, s)
}

/// For what a device sends: the user's own words, with nothing of a session's to mask.
pub(super) fn unmasked(s: &str) -> String {
    s.to_string()
}

fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The look's own animations.
fn own_animations(look: Option<&Look>) -> Vec<String> {
    look.map(|l| l.animations.keys().cloned().collect()).unwrap_or_default()
}

/// The built-in motion a session may play by name: what its roles (and, for think, its gaze)
/// allow and it did not replace with an animation of its own. The default look has every role,
/// so all of it.
fn built_in(look: Option<&Look>) -> Vec<&'static str> {
    match look {
        Some(l) => look::builtins(l).into_iter().filter(|name| !l.animations.contains_key(*name)).collect(),
        None => look::BUILTIN.to_vec(),
    }
}

fn playable(look: Option<&Look>, name: &str) -> bool {
    look.is_some_and(|l| l.animations.contains_key(name)) || built_in(look).contains(&name)
}

/// "No animation wink. Yours: blink, hop; built in: wave, nod". For built-in motion the look does
/// not get, what it lacks first, in a picture's words: "No animation wave: wave is Workbench's
/// built-in motion and needs the armR or the armL role, which this look does not name. Yours: …".
/// `quoted` is the name as a refusal shows it ([`shown`]), made before the store lock is taken:
/// masking asks the terminals slice. A built-in's name is a plain word, so it is shown as it is.
fn no_animation(look: Option<&Look>, quoted: &str) -> ApiError {
    let list = |names: Vec<String>| if names.is_empty() { "none".to_string() } else { names.join(", ") };
    let lacks = look
        .zip(look::BUILTIN.into_iter().find(|name| *name == quoted))
        .and_then(|(l, name)| {
            let unmet = builtin::unmet(name, |role| l.roles.keys().any(|r| r.name() == role), l.gaze.map(look::Num::get));
            look::builtin_missing(name, &l.roles, l.gaze).map(|needs| format!(": {name} is Workbench's built-in motion and needs {needs}, {unmet}"))
        })
        .unwrap_or_default();
    ApiError::bad_request(format!("No animation {quoted}{lacks}. Yours: {}; built in: {}", list(own_animations(look)), list(built_in(look).into_iter().map(str::to_string).collect())))
}

/// Whether mode loop may play it: an animation of the look declared `loop: true`, or built-in
/// motion that repeats.
fn loops(look: Option<&Look>, name: &str) -> bool {
    match look.and_then(|l| l.animations.get(name)) {
        Some(animation) => animation.looping,
        None => BUILTIN_LOOPS.contains(&name),
    }
}

/// The first of the animation's `fill`, `stroke` and opacity tracks that does not end on the
/// value it starts with, by its prop. Each track is asked alone, with the rule the validator has.
fn open_paint(look: &Look, name: &str) -> Option<&'static str> {
    let animation = look.animations.get(name)?;
    animation.tracks.iter().find_map(|track| {
        let prop = match track.prop {
            Prop::Fill => "fill",
            Prop::Stroke => "stroke",
            Prop::Opacity => "opacity",
            Prop::FillOpacity => "fillOpacity",
            Prop::StrokeOpacity => "strokeOpacity",
            _ => return None,
        };
        let mut alone = look.clone();
        alone.animations.get_mut(name)?.tracks = vec![track.clone()];
        (!look::closes(&alone, name)).then_some(prop)
    })
}

/// A line with a backtick, a URL, a path or Markdown markers does not sound like speech.
fn reads_literally(text: &str) -> bool {
    if ["`", "**", "__", "~~", "](", "://", "www."].iter().any(|mark| text.contains(mark)) {
        return true;
    }
    if ["# ", "## ", "- ", "* ", "> "].iter().any(|mark| text.starts_with(mark)) {
        return true;
    }
    text.split(' ').any(|word| {
        let w = word.trim_matches(|c: char| matches!(c, ',' | '.' | ';' | ':' | '!' | '?' | '(' | ')' | '"' | '\''));
        if w.contains('\\') || w.starts_with("~/") || w.starts_with("./") || w.starts_with("../") {
            return true;
        }
        let emphasis = w.len() > 2 && ((w.starts_with('*') && w.ends_with('*')) || (w.starts_with('_') && w.ends_with('_')));
        // /usr/bin, src/main.rs, a/b/c; not and/or, 3/4, km/h.
        let slashes = w.matches('/').count();
        let rooted = w.starts_with('/') && w.chars().nth(1).is_some_and(char::is_alphanumeric);
        let file = slashes == 1 && w.rsplit('/').next().is_some_and(|last| last.len() > 2 && last.contains('.'));
        emphasis || rooted || slashes >= 2 || file
    })
}

// ---------------------------------------------------------------- design

/// An accepted design call.
#[derive(Debug, Clone, PartialEq)]
pub struct Designed {
    pub revision: u64,
    /// The answer: the name, the parts where they are, the animations, usage, the built-in
    /// motion that applies, what was kept or cleared, the warnings ("Not drawn: …" first).
    pub text: String,
    /// The avatar has a look of its own now: there is something to show the session.
    pub drawn: bool,
}

/// A design call that passed the validator and is not stored yet.
#[derive(Debug, Clone)]
pub struct Checked {
    applied: Applied,
    /// The design the call was applied to: it is stored only over this one.
    base: Design,
}

impl Checked {
    /// The look to draw before the design is stored: the new look, where it differs from the
    /// stored one. A look that cannot be drawn is not stored (the browser would pay for it on
    /// every frame, and the session could not be shown it).
    pub fn to_draw(&self) -> Option<&Look> {
        self.applied.design.look.as_ref().filter(|look| self.base.look.as_ref() != Some(*look))
    }

    /// Whether the call leaves the artwork as it is stored (it changed the rig, the motion or
    /// nothing of the look): then what is wrong with drawing it was wrong before the call.
    pub fn same_artwork(&self) -> bool {
        match (&self.applied.design.look, &self.base.look) {
            (Some(new), Some(old)) => new.nodes == old.nodes && new.view_box == old.view_box,
            _ => false,
        }
    }
}

/// The first step of a design call: the arguments of `avatar_design` validated against the
/// design the avatar has now and applied to it (the rules of `look::apply`), without storing
/// anything. A refused call lists what to fix. No lock is held when this returns: the validator
/// masks the design's text, which asks the terminals slice for the session's secrets.
pub fn design_check(state: &AppState, pass: &Pass, args: &Value) -> ApiResult<Checked> {
    pass.is_for(Call::Design)?;
    let tid = pass.tid.as_str();
    let base: Design = on_record(&mut state.avatar.inner.lock().records, tid)?.design.clone();
    let mask = |s: &str| state.terminals.redact_text(tid, s);
    let applied = look::apply_masked(Some(&base), args, &mask).map_err(|problems| ApiError::bad_request(format!("{}\nNothing was changed.", look::message(&problems))))?;
    Ok(Checked { applied, base })
}

/// The design changed between the two steps of a call (another call of the same session).
pub const CHANGED_MEANWHILE: &str = "The avatar's design was changed by another call while this one was checked: send this call again.\nNothing was changed.";

/// The second step: the checked design is stored, when the avatar still has the design the call
/// was applied to.
pub fn design_store(state: &AppState, pass: &Pass, checked: &Checked, now: i64) -> ApiResult<Designed> {
    pass.is_for(Call::Design)?;
    let tid = pass.tid.as_str();
    let applied = &checked.applied;
    let mut guard = state.avatar.inner.lock();
    let rec = on_record(&mut guard.records, tid)?;
    if rec.design != checked.base {
        return Err(ApiError::conflict(CHANGED_MEANWHILE));
    }
    let changed = rec.design != applied.design;
    if changed {
        rec.design = applied.design.clone();
        rec.revision += 1;
        // A held gesture the new look does not have is over.
        if rec.held.as_ref().is_some_and(|held| !playable(rec.design.look.as_ref(), &held.name)) {
            rec.held = None;
        }
        // Nor does a gesture that waited its turn go out against a look it was not checked for.
        rec.act_queued = None;
        rec.touch(now);
        save(state, tid, rec);
        emit_changed(state, tid, rec);
    }
    Ok(Designed { revision: rec.revision, text: design_answer(rec, applied, changed), drawn: rec.design.look.is_some() })
}

/// Both steps at once, without drawing the look in between: for tests of what a design call
/// stores. The tool and the hook route draw the look between the steps (`tools::design`).
#[cfg(test)]
pub fn design(state: &AppState, pass: &Pass, args: &Value, now: i64) -> ApiResult<Designed> {
    let checked = design_check(state, pass, args)?;
    design_store(state, pass, &checked, now)
}

fn kib(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / 1024.0)
}

/// The look's drives, one line each: "lookX moves head rotate".
fn drives(look: &Look) -> Vec<String> {
    look.drives.iter().map(ToString::to_string).collect()
}

/// What the session reads after a design call, beside the picture: the parts and where they are.
fn design_answer(rec: &Record, applied: &Applied, changed: bool) -> String {
    let named = if rec.design.name.is_empty() { "no name yet".to_string() } else { format!("named {}", rec.design.name) };
    let mut out = if changed { format!("Saved as revision {}, {named}.", rec.revision) } else { format!("Nothing to save: the avatar already has this design. It is revision {}, {named}.", rec.revision) };
    match &rec.design.look {
        None => {
            out.push_str("\nThe look is the default one: send svg to draw your own.");
            // Of what else a design holds, the default look can carry only a voice.
            if let Some(line) = stored_besides(&rec.design) {
                out.push_str(&format!("\n{line}"));
            }
        }
        Some(look) => {
            if applied.parts.is_empty() {
                out.push_str("\nNo part has an id: give every part that moves an id.");
            } else {
                out.push_str("\nParts (id tag x y w h pivot x y, in root coordinates, at rest):");
                for part in &applied.parts {
                    out.push_str(&format!("\n{part}"));
                }
            }
            if applied.animations.is_empty() {
                out.push_str("\nNo animations of your own.");
            } else {
                out.push_str(&format!("\nYour animations: {}.", applied.animations.join(", ")));
            }
            if !look.drives.is_empty() {
                out.push_str(&format!("\nDrives: {}.", drives(look).join("; ")));
            }
            // The rest of what is stored, so that a session that merged one field need not ask.
            out.push_str(&format!("\n{}", stored_besides(&rec.design).unwrap_or_else(|| STORED_NOTHING_ELSE.to_string())));
            let usage = applied.usage;
            out.push_str(&format!(
                "\nUsage: svg {} of {} KiB, {} of {} elements, {} of {} animations",
                kib(usage.svg_bytes),
                look::MAX_SVG_BYTES / 1024,
                usage.elements,
                look::MAX_ELEMENTS,
                usage.animations,
                look::MAX_ANIMATIONS
            ));
            // Parents and drives only where the look has them.
            if usage.parents > 0 {
                out.push_str(&format!(", {} of {} parents", usage.parents, look::MAX_PARENTS));
            }
            if usage.drives > 0 {
                out.push_str(&format!(", {} of {} drives", usage.drives, look::MAX_DRIVES));
            }
            out.push('.');
            let built = built_in(Some(look));
            if built.is_empty() {
                let has = |role: &str| look.roles.keys().any(|r| r.name() == role);
                out.push_str(&format!("\nNo built-in motion applies: {}.", builtin::none_applies(has, look.gaze.map(look::Num::get), &look::builtins(look))));
            } else {
                out.push_str(&format!("\nBuilt-in motion that applies: {}.", built.join(", ")));
            }
        }
    }
    if !applied.kept.is_empty() {
        out.push_str(&format!("\nKept: {}.", applied.kept.join("; ")));
    }
    if !applied.cleared.is_empty() {
        out.push_str(&format!("\nCleared: {}.", applied.cleared.join("; ")));
    }
    if !applied.warnings.is_empty() {
        out.push_str("\nWarnings:");
        for warning in &applied.warnings {
            out.push_str(&format!("\n{warning}"));
        }
    }
    out
}

/// The line of a design answer for a look that holds none of what [`stored_besides`] names.
pub const STORED_NOTHING_ELSE: &str = "Also stored: no roles, states, mouth, gaze or voice.";
/// Characters of the line [`stored_besides`] writes, at most.
pub const STORED_CHARS: usize = 160;

/// What the stored design holds besides its name, parts, pivots, parents, animations and drives
/// (which the design answer lists), in one line: the roles by name, the states it sets, the parts
/// its mouth moves, the gaze and the voice's language, only what is set. A session that merged
/// one field reads here that the others are still there. Within [`STORED_CHARS`]: the long lists
/// are counted instead. `None`: it holds none of them.
fn stored_besides(design: &Design) -> Option<String> {
    let stored = design.look.as_ref();
    let roles: Vec<&str> = stored.map(|l| l.roles.keys().map(|role| role.name()).collect()).unwrap_or_default();
    let states: Vec<&str> = stored.map(|l| l.states.keys().map(|state| state.name()).collect()).unwrap_or_default();
    let mouth: Vec<&str> = stored
        .map(|l| {
            l.mouth
                .iter()
                .map(|drive| match drive {
                    look::MouthDrive::Path { target, .. } | look::MouthDrive::Prop { target, .. } => target.as_str(),
                })
                .collect()
        })
        .unwrap_or_default();
    let gaze = stored.and_then(|l| l.gaze).map(|gaze| if gaze.get() > 0.0 { format!("gaze {}", gaze.get()) } else { "gaze off".to_string() });
    let voice = match &design.voice.lang {
        Some(lang) => Some(format!("voice {lang}")),
        None if design.voice != Voice::default() => Some("voice in the default language".to_string()),
        None => None,
    };
    // Names first; where they make the line too long, the mouth's parts and then the states are counted.
    let line = |states_named: bool, mouth_named: bool| {
        let mut said: Vec<String> = Vec::new();
        if !roles.is_empty() {
            said.push(format!("roles {}", roles.join(", ")));
        }
        if !states.is_empty() {
            said.push(if states_named { format!("states {}", states.join(", ")) } else { format!("{} states", states.len()) });
        }
        if !mouth.is_empty() {
            said.push(match (mouth_named, mouth.len()) {
                (true, _) => format!("mouth moves {}", mouth.join(", ")),
                (false, 1) => "mouth moves 1 part".to_string(),
                (false, n) => format!("mouth moves {n} parts"),
            });
        }
        said.extend(gaze.clone());
        said.extend(voice.clone());
        (!said.is_empty()).then(|| format!("Also stored: {}.", said.join("; ")))
    };
    let fits = |l: &Option<String>| l.as_ref().is_none_or(|l| l.chars().count() <= STORED_CHARS);
    [(true, true), (true, false)].into_iter().map(|(states_named, mouth_named)| line(states_named, mouth_named)).find(fits).unwrap_or_else(|| line(false, false))
}

// ---------------------------------------------------------------- say

/// One line for the user: the arguments of `avatar_say`.
#[derive(Debug, Clone, PartialEq)]
pub struct SayRequest {
    pub text: String,
    pub lang: Option<String>,
    pub gesture: Option<String>,
    /// Return only when a device said what became of the line.
    pub wait: bool,
}

impl SayRequest {
    /// `mask`: the session's secret masking ([`Pass::mask`]), for a refusal that quotes a key.
    pub fn parse(args: &Value, mask: &dyn Fn(&str) -> String) -> ApiResult<SayRequest> {
        let o = fields(args, "say", &["text", "gesture", "lang", "wait"], mask)?;
        let text = text_of(o, "text")?.ok_or_else(|| ApiError::bad_request("say needs text: the line, one or two short plain sentences"))?;
        Ok(SayRequest { text, lang: text_of(o, "lang")?, gesture: text_of(o, "gesture")?, wait: flag_of(o, "wait")?.unwrap_or(false) })
    }
}

/// A line that was queued.
#[derive(Debug)]
pub struct Said {
    pub line: Line,
    tid: String,
    /// What became of the session's earlier lines, in sentences.
    earlier: String,
    clock: Clock,
}

/// The answer of `avatar_say`: `note` is the whole answer in words.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SayAnswer {
    pub line_id: String,
    /// With `wait`: what became of the line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    pub note: String,
}

/// What became of a line, as the end of a sentence about it. It says no cause the server does
/// not know: a line shown as a caption may have been so because voice is off, or because the
/// tab that was saying it closed.
fn fate(outcome: Option<Outcome>, voice: Option<&str>) -> String {
    match (outcome, voice) {
        (None, _) => "is waiting to be presented".to_string(),
        (Some(Outcome::Spoken), Some(voice)) => format!("spoken aloud by {voice}"),
        (Some(Outcome::Spoken), None) => "spoken aloud".to_string(),
        (Some(Outcome::Shown), _) => "shown as a caption, not spoken".to_string(),
        (Some(Outcome::Skipped), _) => "skipped by the user".to_string(),
        (Some(Outcome::Failed), _) => "shown as a caption: the browser's voice failed".to_string(),
        (Some(Outcome::Hidden), _) => "not shown: the user has hidden the avatars".to_string(),
        (Some(Outcome::Unheard), _) => "not heard: nobody had this project open in Workbench: stop speaking until the user writes".to_string(),
    }
}

/// What became of the earlier lines the session has not been told about yet, newest last, at
/// most three of them. Told once: the next answer does not repeat it.
fn earlier(rec: &mut Record) -> String {
    let mut told = Vec::new();
    for l in rec.lines.iter_mut().filter(|l| l.line.from == Speaker::Agent && !l.told) {
        if l.line.outcome.is_some() {
            l.told = true;
            told.push(format!("{} {}.", l.line.id, fate(l.line.outcome, l.line.voice.as_deref())));
        }
    }
    let from = told.len().saturating_sub(3);
    told[from..].join(" ")
}

impl Said {
    fn note(&self, first: String) -> String {
        let mut note = first;
        if !self.earlier.is_empty() {
            note.push_str(&format!(" Earlier: {}", self.earlier));
        }
        if reads_literally(&self.line.text) {
            note.push_str(&format!(" {HINT_LITERAL}"));
        }
        note
    }

    /// The answer of a call that does not wait: the line is queued, and what became of earlier ones.
    pub fn answer(&self) -> SayAnswer {
        SayAnswer { line_id: self.line.id.clone(), outcome: None, voice: None, note: self.note(format!("Queued ({}).", self.line.id)) }
    }
}

/// Says one line through the session's avatar. It does not wait: the line is queued for the
/// browsers, and the answer tells what became of the earlier ones.
pub fn say(state: &AppState, pass: &Pass, req: SayRequest, now: i64) -> ApiResult<Said> {
    pass.is_for(Call::Say)?;
    pass.speaks()?;
    let tid = pass.tid.as_str();
    let text = clean(state, tid, "the line", &req.text, MAX_SAY_CHARS)?;
    let mask = mask_of(state, tid);
    let lang = lang_of(req.lang, &mask)?;
    let gesture = trimmed(req.gesture);
    // As a refusal would show it, while the store lock is free.
    let quoted = gesture.as_deref().map(|name| shown(name, &mask)).unwrap_or_default();

    let mut guard = state.avatar.inner.lock();
    let Inner { records, boot, line_count, .. } = &mut *guard;
    let rec = on_record(records, tid)?;
    if gesture.as_deref().is_some_and(|name| !playable(rec.design.look.as_ref(), name)) {
        return Err(no_animation(rec.design.look.as_ref(), &quoted));
    }
    if rec.pending() >= MAX_PENDING_LINES {
        return Err(too_many(format!("Not said: {MAX_PENDING_LINES} lines are still waiting to be spoken. Speak less often.")));
    }
    let earlier = earlier(rec);
    let line = Line { id: next_id('L', boot, line_count), at: now, from: Speaker::Agent, text, lang, gesture, ask_id: None, via: None, outcome: None, voice: None };
    rec.push_line(line.clone());
    rec.touch(now);
    emit_say(state, tid, rec, &line, None);
    Ok(Said { line, tid: tid.to_string(), earlier, clock: Clock::at(now) })
}

/// How long `say` with `wait` waits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SayWait {
    /// After a caption was shown (or the avatars are hidden) and no tab holds the line to speak it.
    pub soft: Duration,
    /// While no tab holds the line: at most this long.
    pub most: Duration,
    /// While a tab holds the line and has said nothing of it yet (it waits behind another line,
    /// or is being spoken): at most this long, from the call's start.
    pub held: Duration,
}

impl SayWait {
    /// For a line of this many characters: its estimated speech time and a margin, within the
    /// longest wait of a tool call; a line a tab holds, the longest wait of a tool call.
    pub fn of(chars: usize) -> SayWait {
        let speech = chars as u64 / SPEECH_CHARS_PER_SEC as u64;
        SayWait { soft: Duration::from_millis(SAY_WAIT_SOFT_MS), most: Duration::from_secs((speech + SAY_WAIT_EXTRA_SECS).min(MAX_WAIT_TOOL_SECS)), held: Duration::from_secs(MAX_WAIT_TOOL_SECS) }
    }
}

/// `say` with `wait`: returns when a device acknowledged the line as spoken, skipped or failed,
/// a moment after it was only shown and no tab is about to speak it, or at the timeout with the
/// best outcome known, else unheard. A line that a tab holds and has said nothing of yet is
/// waited for past the timeout, up to the longest wait of a tool call: someone is there, and
/// should it still be held then, it is waiting to be presented.
pub async fn say_wait(state: &AppState, said: Said) -> SayAnswer {
    let wait = SayWait::of(said.line.text.chars().count());
    say_wait_within(state, said, wait).await
}

pub(super) async fn say_wait_within(state: &AppState, said: Said, wait: SayWait) -> SayAnswer {
    let mut tick = state.avatar.tick.subscribe();
    let start = Instant::now();
    let deadline = start + wait.most;
    let cap = start + wait.most.max(wait.held);
    let mut soft_since: Option<Instant> = None;
    let (outcome, voice) = loop {
        tick.borrow_and_update();
        // The line as it is now: its outcome, the voice, and how long a tab still holds it.
        let seen = {
            let inner = state.avatar.inner.lock();
            let now = said.clock.now();
            let line = inner.records.get(&said.tid).and_then(|rec| rec.lines.iter().find(|l| l.line.id == said.line.id));
            // No device says anything more of it once the server stops or the avatar is off.
            let over = inner.stopping || !inner.records.get(&said.tid).is_some_and(|rec| rec.on);
            line.map(|l| (l.line.outcome, l.line.voice.clone(), l.lease.as_ref().map(|lease| lease.until - now).filter(|left| *left > 0), over))
        };
        let Some((outcome, voice, leased, over)) = seen else { break (Some(Outcome::Unheard), None) };
        let (mut wake, mut until) = (deadline, deadline);
        match outcome {
            Some(outcome) if outcome.is_final() => break (Some(outcome), voice),
            Some(outcome) => {
                let since = *soft_since.get_or_insert_with(Instant::now);
                match leased {
                    // A tab is saying it: its end is worth the wait.
                    Some(left) => wake = wake.min(Instant::now() + Duration::from_millis(left as u64 + 1)),
                    None if since.elapsed() >= wait.soft => break (Some(outcome), voice),
                    None => wake = wake.min(since + wait.soft),
                }
            }
            // A tab holds it and has said nothing of it yet: it waits behind another line, or is
            // being spoken. Its end is worth the wait, past the timeout, up to the cap.
            None => {
                if let Some(left) = leased {
                    until = cap;
                    wake = cap.min(Instant::now() + Duration::from_millis(left as u64 + 1));
                }
            }
        }
        if over || Instant::now() >= until {
            break match (outcome, leased) {
                // Still held at the cap: someone is there, and the line waits to be presented.
                (None, Some(_)) if !over => (None, None),
                // The best outcome known; none, when nobody acknowledged it and no tab holds it:
                // unheard.
                (outcome, _) => (Some(outcome.unwrap_or(Outcome::Unheard)), voice),
            };
        }
        tokio::select! {
            _ = tick.changed() => {}
            _ = tokio::time::sleep_until(wake) => {}
        }
    };
    // The session has now been told what became of this line.
    if let Some(l) = state.avatar.inner.lock().records.get_mut(&said.tid).and_then(|rec| rec.line_mut(&said.line.id)) {
        l.told = l.line.outcome.is_some();
    }
    let note = said.note(format!("{} {}.", said.line.id, fate(outcome, voice.as_deref())));
    SayAnswer { line_id: said.line.id.clone(), outcome, voice, note }
}

/// One `avatar_say` call from its request to its answer.
pub async fn say_call(state: &AppState, pass: &Pass, req: SayRequest, now: i64) -> ApiResult<SayAnswer> {
    let wait = req.wait;
    let said = say(state, pass, req, now)?;
    Ok(if wait { say_wait(state, said).await } else { said.answer() })
}

// ---------------------------------------------------------------- ask

/// A new question: the arguments of `avatar_ask` that ask one.
#[derive(Debug, Clone, PartialEq)]
pub struct AskRequest {
    pub text: String,
    /// Up to [`MAX_CHOICES`], shown as buttons.
    pub choices: Vec<String>,
    /// The user may also type or dictate an answer.
    pub free_text: bool,
    pub lang: Option<String>,
    pub gesture: Option<String>,
}

/// What an `avatar_ask` call does.
#[derive(Debug, Clone, PartialEq)]
pub enum AskCall {
    New(AskRequest),
    /// Wait again for the answer to the open question, by its id.
    Again(String),
    /// Withdraw the open question.
    Cancel,
}

/// The arguments of `avatar_ask`, in any of their three forms.
#[derive(Debug, Clone, PartialEq)]
pub struct AskArgs {
    pub call: AskCall,
    /// How long the call waits; `None`: [`DEFAULT_WAIT_SECS`].
    pub wait_seconds: Option<f64>,
}

impl AskArgs {
    /// `mask`: the session's secret masking ([`Pass::mask`]), for a refusal that quotes a key.
    pub fn parse(args: &Value, mask: &dyn Fn(&str) -> String) -> ApiResult<AskArgs> {
        let o = fields(args, "ask", &["text", "choices", "freeText", "waitSeconds", "askId", "cancel", "gesture", "lang"], mask)?;
        let wait_seconds = number_of(o, "waitSeconds")?;
        let call = match (flag_of(o, "cancel")?.unwrap_or(false), text_of(o, "askId")?, text_of(o, "text")?) {
            (true, None, None) => AskCall::Cancel,
            (true, ..) => return Err(ApiError::bad_request("cancel goes alone: it withdraws the open question")),
            (false, Some(ask_id), None) => AskCall::Again(ask_id),
            (false, Some(_), Some(_)) => return Err(ApiError::bad_request("askId and text in one call: askId waits again for the open question, text asks a new one")),
            (false, None, Some(text)) => {
                let choices = match o.get("choices") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(list)) if list.len() <= MAX_CHOICES && list.iter().all(Value::is_string) => list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                    Some(Value::Array(list)) if list.len() > MAX_CHOICES => return Err(ApiError::bad_request(format!("{} choices: at most {MAX_CHOICES}", list.len()))),
                    Some(_) => return Err(ApiError::bad_request(format!("choices is a list of up to {MAX_CHOICES} short strings"))),
                };
                AskCall::New(AskRequest { text, choices, free_text: flag_of(o, "freeText")?.unwrap_or(true), lang: text_of(o, "lang")?, gesture: text_of(o, "gesture")? })
            }
            (false, None, None) => return Err(ApiError::bad_request("ask needs text (a new question), askId (wait again for the open one) or cancel true (withdraw it)")),
        };
        Ok(AskArgs { call, wait_seconds })
    }

    /// How long this call waits, when the caller may wait `most` seconds
    /// ([`MAX_WAIT_TOOL_SECS`] for a tool, [`MAX_WAIT_HOOK_SECS`] for a hook request).
    pub fn wait(&self, most: u64) -> ApiResult<Duration> {
        wait_of(self.wait_seconds, most)
    }
}

/// `waitSeconds` as a wait: the default when none is given, refused beyond `most` seconds.
pub fn wait_of(seconds: Option<f64>, most: u64) -> ApiResult<Duration> {
    match seconds {
        None => Ok(Duration::from_secs(DEFAULT_WAIT_SECS.min(most))),
        Some(s) if s.is_finite() && (0.0..=most as f64).contains(&s) => Ok(Duration::from_secs_f64(s)),
        Some(s) => Err(ApiError::bad_request(format!("waitSeconds {s}: 0 to {most}"))),
    }
}

/// The answer of `avatar_ask`: `{answered: true, text, choice?, via}`, or
/// `{answered: false, reason, askId, note}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskResult {
    pub answered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The zero-based index into the question's choices, for a choice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<ReplyVia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl AskResult {
    fn answer(answer: Answer) -> AskResult {
        AskResult { answered: true, text: Some(answer.text), choice: answer.choice, via: Some(answer.via), reason: None, ask_id: None, note: None }
    }

    /// No answer: why, and what to do next.
    fn none(reason: Reason, ask_id: &str) -> AskResult {
        let note = match reason {
            Reason::Timeout => format!(
                "Not answered yet. The question stays on screen. Do not ask again and do not assume an answer. Go on with work that does not depend on it, or end your turn; the answer arrives as the user's next message, starting with [via avatar…] and naming {ask_id}."
            ),
            Reason::Unseen => format!(
                "Nobody saw the question: no Workbench window has this project open. It stays open. Do not ask again and do not assume an answer. Go on with work that does not depend on it, or end your turn; the answer arrives as the user's next message, starting with [via avatar…] and naming {ask_id}."
            ),
            Reason::Dismissed => "The user dismissed the question without answering. Do not ask it again: go on without an answer, or say what you need in the terminal.".to_string(),
            Reason::Superseded => "A newer question of yours replaced this one: wait for that one.".to_string(),
            Reason::Withdrawn => "The question is withdrawn.".to_string(),
            Reason::SessionEnded => "The session ended before an answer came.".to_string(),
            Reason::MovedOn => "The user went on in the terminal instead of answering through the avatar: read their message there.".to_string(),
        };
        AskResult { answered: false, text: None, choice: None, via: None, reason: Some(reason), ask_id: Some(ask_id.to_string()), note: Some(note) }
    }
}

/// A call that waits for the answer to a question.
///
/// Dropped before it returned (the client went away and its task was aborted), it leaves the
/// question: under the store lock it takes its own sender out of the record. When a reply took
/// the sender first, the answer is already in its hands, and it pastes it into the session as a
/// prompt instead, so no answer is lost with a call.
pub struct AnswerWaiter {
    state: AppState,
    tid: String,
    ask_id: String,
    /// Which of the question's waiters this is: a newer one takes its place.
    slot: u64,
    clock: Clock,
    /// Taken once the call has its end.
    rx: Option<oneshot::Receiver<AskEnd>>,
}

impl AnswerWaiter {
    #[allow(dead_code)] // read by tests alone so far
    pub fn ask_id(&self) -> &str {
        &self.ask_id
    }

    /// Whether a device took the question up: it acknowledged the question's line, or a tab holds
    /// the line to say it aloud right now. `None`: the question is closed.
    fn seen(&self) -> Option<bool> {
        let inner = self.state.avatar.inner.lock();
        if inner.stopping {
            return None;
        }
        inner.records.get(&self.tid)?.ask_taken_up(&self.ask_id, Outcome::acknowledged, self.clock.now())
    }

    /// Takes this call's own sender out of the record, when it is still there: the call leaves
    /// and the question stays open. False: someone took the sender, and has sent with it.
    fn leave(&self) -> bool {
        let mut inner = self.state.avatar.inner.lock();
        let Some(rec) = inner.records.get_mut(&self.tid) else { return false };
        let Some(open) = rec.ask.as_mut().filter(|open| open.ask.id == self.ask_id && open.waiter.as_ref().is_some_and(|(slot, _)| *slot == self.slot)) else { return false };
        open.waiter = None;
        open.ask.waiting = false;
        let ask = open.ask.clone();
        rec.touch(self.clock.now());
        emit_ask(&self.state, &self.tid, rec, "waiter_left", Some(&ask));
        true
    }

    /// The call gives up waiting. When a reply got in first, its answer is the result after all.
    fn give_up(&mut self, reason: Reason) -> AskResult {
        if self.leave() {
            self.rx = None;
            return AskResult::none(reason, &self.ask_id);
        }
        let end = self.rx.take().map(|mut rx| rx.try_recv());
        match end {
            Some(Ok(end)) => self.result(end),
            _ => AskResult::none(Reason::SessionEnded, &self.ask_id),
        }
    }

    fn result(&self, end: AskEnd) -> AskResult {
        match end {
            AskEnd::Answered { answer, .. } => AskResult::answer(answer),
            AskEnd::Closed(reason) => AskResult::none(reason, &self.ask_id),
            // Another call of the session waits for the answer now: for this one it is not there yet.
            AskEnd::Handed => AskResult::none(Reason::Timeout, &self.ask_id),
        }
    }

    fn ended(&mut self, end: Result<AskEnd, oneshot::error::RecvError>) -> AskResult {
        self.rx = None;
        match end {
            Ok(end) => self.result(end),
            Err(_) => AskResult::none(Reason::SessionEnded, &self.ask_id),
        }
    }
}

impl Drop for AnswerWaiter {
    fn drop(&mut self) {
        let Some(mut rx) = self.rx.take() else { return };
        if self.leave() {
            return;
        }
        // The sender was taken, and what it carried was sent under the store lock. An answer
        // must not be lost with this call: it goes into the session as a prompt.
        rx.close();
        if let Ok(AskEnd::Answered { prompt, .. }) = rx.try_recv() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let (state, tid) = (self.state.clone(), self.tid.clone());
                runtime.spawn(async move {
                    if let Err(e) = deliver(&state, &tid, &prompt).await {
                        tracing::warn!(terminal = %tid, "avatar: an answer could not be delivered to the session: {}", e.code);
                    }
                });
            }
        }
    }
}

/// Makes the call the one that waits for the open question's answer. A call that waited before
/// is handed off: for it the answer is not there yet.
fn attach(state: &AppState, tid: &str, rec: &mut Record, stopping: bool, now: i64) -> ApiResult<(Ask, AnswerWaiter)> {
    let Some(open) = rec.ask.as_mut() else { return Err(not_pending("No question is open.")) };
    if let Some((_, earlier)) = open.waiter.take() {
        let _ = earlier.send(AskEnd::Handed);
    }
    open.waiters += 1;
    let slot = open.waiters;
    let (tx, rx) = oneshot::channel();
    let was = open.ask.waiting;
    if stopping {
        // The server is stopping: nothing waits any more.
        let _ = tx.send(AskEnd::Closed(Reason::SessionEnded));
        open.ask.waiting = false;
    } else {
        open.waiter = Some((slot, tx));
        open.ask.waiting = true;
    }
    let ask = open.ask.clone();
    if was != ask.waiting {
        rec.touch(now);
        emit_ask(state, tid, rec, if ask.waiting { "waiting" } else { "waiter_left" }, Some(&ask));
    }
    let waiter = AnswerWaiter { state: state.clone(), tid: tid.to_string(), ask_id: ask.id.clone(), slot, clock: Clock::at(now), rx: Some(rx) };
    Ok((ask, waiter))
}

/// Asks the user one question. The call that asked waits for the answer with [`wait_answer`].
///
/// A question whose text and choices equal the open one's is that question: nothing is said or
/// announced twice, the call waits for the answer to it. Any other question takes the open
/// one's place, whose call learns that it was superseded.
pub fn ask(state: &AppState, pass: &Pass, req: AskRequest, now: i64) -> ApiResult<(Ask, AnswerWaiter)> {
    pass.is_for(Call::Ask)?;
    pass.speaks()?;
    let tid = pass.tid.as_str();
    let text = clean(state, tid, "the question", &req.text, MAX_ASK_CHARS)?;
    if req.choices.len() > MAX_CHOICES {
        return Err(ApiError::bad_request(format!("{} choices: at most {MAX_CHOICES}", req.choices.len())));
    }
    let mut choices: Vec<String> = Vec::with_capacity(req.choices.len());
    for (i, choice) in req.choices.iter().enumerate() {
        let choice = clean(state, tid, &format!("choice {}", i + 1), choice, MAX_CHOICE_CHARS)?;
        if let Some(same) = choices.iter().position(|c| *c == choice) {
            return Err(ApiError::bad_request(format!("choice {} repeats choice {}: give each answer once", i + 1, same + 1)));
        }
        choices.push(choice);
    }
    if choices.is_empty() && !req.free_text {
        return Err(ApiError::bad_request("a question without choices needs a typed answer: give choices, or leave freeText true"));
    }
    let mask = mask_of(state, tid);
    let lang = lang_of(req.lang, &mask)?;
    let gesture = trimmed(req.gesture);
    // As a refusal would show it, while the store lock is free.
    let quoted = gesture.as_deref().map(|name| shown(name, &mask)).unwrap_or_default();

    let mut guard = state.avatar.inner.lock();
    let Inner { records, limits, boot, line_count, ask_count, stopping, .. } = &mut *guard;
    let rec = on_record(records, tid)?;
    if let Some(open) = &rec.ask {
        if rec.answering(&open.ask.id) {
            return Err(answer_on_its_way(&open.ask.id));
        }
        if open.ask.text == text && open.ask.choices == choices {
            return attach(state, tid, rec, *stopping, now);
        }
    }
    // A new question is a line as well, and counts as one.
    if !limits.entry(tid.to_string()).or_default().say.attempt(now, SAY_PER_MINUTE) {
        return Err(too_many(format!("Not asked: at most {SAY_PER_MINUTE} lines and questions a minute. Ask less often.")));
    }
    if gesture.as_deref().is_some_and(|name| !playable(rec.design.look.as_ref(), name)) {
        return Err(no_animation(rec.design.look.as_ref(), &quoted));
    }
    if rec.pending() >= MAX_PENDING_LINES {
        return Err(too_many(format!("Not asked: {MAX_PENDING_LINES} lines are still waiting to be spoken. Speak less often.")));
    }
    rec.close_ask(state, tid, Reason::Superseded, now);
    let ask_id = next_id('Q', boot, ask_count);
    let line = Line { id: next_id('L', boot, line_count), at: now, from: Speaker::Agent, text: text.clone(), lang, gesture, ask_id: Some(ask_id.clone()), via: None, outcome: None, voice: None };
    let ask = Ask { id: ask_id, line_id: line.id.clone(), text, choices, free_text: req.free_text, asked_at: now, waiting: !*stopping };
    rec.push_line(line.clone());
    rec.ask = Some(OpenAsk { ask: ask.clone(), waiter: None, waiters: 0, nudged: false });
    rec.touch(now);
    emit_say(state, tid, rec, &line, Some(&ask));
    emit_ask(state, tid, rec, "opened", Some(&ask));
    attach(state, tid, rec, *stopping, now)
}

/// Waits again for the answer to the open question, by its id.
pub fn ask_again(state: &AppState, pass: &Pass, ask_id: &str, now: i64) -> ApiResult<(Ask, AnswerWaiter)> {
    pass.is_for(Call::Ask)?;
    pass.speaks()?;
    let tid = pass.tid.as_str();
    // As a refusal would show it, while the store lock is free.
    let quoted = shown(ask_id, &mask_of(state, tid));
    let mut guard = state.avatar.inner.lock();
    let stopping = guard.stopping;
    let rec = on_record(&mut guard.records, tid)?;
    if !rec.ask.as_ref().is_some_and(|open| open.ask.id == ask_id) {
        return Err(not_pending(format!(
            "Question {quoted} is not open any more: it was answered, dismissed or replaced. An answer the user gave arrives as a message that starts with [via avatar."
        )));
    }
    if rec.answering(ask_id) {
        return Err(answer_on_its_way(ask_id));
    }
    attach(state, tid, rec, stopping, now)
}

/// Withdraws the open question: it leaves the screen, and a call that waits for it learns so.
pub fn ask_cancel(state: &AppState, pass: &Pass, now: i64) -> ApiResult<AskResult> {
    pass.is_for(Call::Ask)?;
    let tid = pass.tid.as_str();
    let mut guard = state.avatar.inner.lock();
    let rec = on_record(&mut guard.records, tid)?;
    let Some(ask_id) = rec.ask.as_ref().map(|open| open.ask.id.clone()) else { return Err(not_pending("No question is open: there is nothing to withdraw.")) };
    if rec.answering(&ask_id) {
        return Err(answer_on_its_way(&ask_id));
    }
    rec.close_ask(state, tid, Reason::Withdrawn, now);
    Ok(AskResult::none(Reason::Withdrawn, &ask_id))
}

/// Waits for the answer, at most `wait` from the moment a device acknowledged the question.
/// When no device acknowledges it within [`UNSEEN_SECS`] the call returns at once (`unseen`);
/// at the end of the wait it returns `timeout` (also when the wait is shorter than that and
/// nobody acknowledged the question within it). In each case the question stays open.
pub async fn wait_answer(waiter: AnswerWaiter, wait: Duration) -> AskResult {
    wait_answer_within(waiter, wait, Duration::from_secs(UNSEEN_SECS)).await
}

pub(super) async fn wait_answer_within(mut waiter: AnswerWaiter, wait: Duration, unseen: Duration) -> AskResult {
    if wait.is_zero() {
        return waiter.give_up(Reason::Timeout);
    }
    let mut tick = waiter.state.avatar.tick.subscribe();
    // Until a device acknowledged the question, but never longer than the call wants to wait at
    // all: a call that gives up sooner than a device could answer has only timed out. A closed
    // question ends the wait through `rx`.
    let (first, why) = if wait < unseen { (wait, Reason::Timeout) } else { (unseen, Reason::Unseen) };
    let first_over = Instant::now() + first;
    loop {
        tick.borrow_and_update();
        if waiter.seen() != Some(false) {
            break;
        }
        let Some(rx) = waiter.rx.as_mut() else { break };
        tokio::select! {
            end = rx => return waiter.ended(end),
            _ = tick.changed() => {}
            _ = tokio::time::sleep_until(first_over) => return waiter.give_up(why),
        }
    }
    let Some(rx) = waiter.rx.as_mut() else { return AskResult::none(Reason::SessionEnded, &waiter.ask_id) };
    tokio::select! {
        end = rx => waiter.ended(end),
        _ = tokio::time::sleep(wait) => waiter.give_up(Reason::Timeout),
    }
}

/// One `avatar_ask` call from its arguments to its answer: a new question, another wait for the
/// open one, or its withdrawal. `most` is the longest the caller may wait, in seconds.
pub async fn ask_call(state: &AppState, pass: &Pass, args: AskArgs, most: u64, now: i64) -> ApiResult<AskResult> {
    let wait = args.wait(most)?;
    let (_, waiter) = match args.call {
        AskCall::Cancel => return ask_cancel(state, pass, now),
        AskCall::Again(ask_id) => ask_again(state, pass, &ask_id, now)?,
        AskCall::New(req) => ask(state, pass, req, now)?,
    };
    Ok(wait_answer(waiter, wait).await)
}

// ---------------------------------------------------------------- what reaches the session

/// The prompt that carries a reply of the user into the session: a fixed frame, the question's
/// id, and the user's own words or the choice that was clicked. It never holds the question.
pub(super) fn frame(session: Via, ask_id: Option<&str>, answer: &Answer) -> String {
    let with = match session {
        Via::Tools => "avatar_say",
        Via::Command => "your avatar say command",
    };
    let dictated = if answer.via == ReplyVia::Voice { ", dictated" } else { "" };
    let said = match (ask_id, answer.choice) {
        (Some(id), Some(index)) => format!("Answer to question {id}: choice {}, \"{}\"", index + 1, answer.text),
        (Some(id), None) => format!("Answer to question {id}: {}", answer.text),
        (None, _) => answer.text.clone(),
    };
    format!("[via avatar{dictated}, answer with {with}] {said}")
}

/// Pastes a prompt into the session and submits it, through the terminals slice's ask path
/// (`POST /api/agents/ask` in-process): it refuses while the session shows a dialog, is starting
/// or is not running, and waits for a pause in the user's own typing.
pub(super) async fn deliver(state: &AppState, tid: &str, prompt: &str) -> ApiResult<()> {
    #[cfg(test)]
    {
        let hook = state.avatar.pastes.lock().clone();
        if let Some(hook) = hook {
            let (done, result) = oneshot::channel();
            let _ = hook.send(super::store::Paste { tid: tid.to_string(), prompt: prompt.to_string(), done });
            return result.await.unwrap_or_else(|_| Err(ApiError::internal("the test let the paste go")));
        }
    }
    let body = json!({ "terminalId": tid, "prompt": prompt, "submit": true });
    crate::mcp::call_api(state, axum::http::Method::POST, "/api/agents/ask", Some(body), &crate::mcp::McpCtx::default()).await.map(|_| ())
}

// ---------------------------------------------------------------- act

/// How a gesture plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActMode {
    /// Once, or `times` in a row.
    Once,
    /// It keeps its last frame.
    Hold,
    /// It repeats: only an animation that loops.
    Loop,
}

/// The arguments of `avatar_act`: play one animation as a gesture, or stop one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActRequest {
    pub play: Option<String>,
    pub mode: Option<ActMode>,
    pub times: Option<u32>,
    /// A name, or `all`.
    pub stop: Option<String>,
}

impl ActRequest {
    /// `mask`: the session's secret masking ([`Pass::mask`]), for a refusal that quotes a value.
    pub fn parse(args: &Value, mask: &dyn Fn(&str) -> String) -> ApiResult<ActRequest> {
        let o = fields(args, "act", &["play", "mode", "times", "stop"], mask)?;
        let mode = match text_of(o, "mode")?.as_deref() {
            None => None,
            Some("once") => Some(ActMode::Once),
            Some("hold") => Some(ActMode::Hold),
            Some("loop") => Some(ActMode::Loop),
            Some(other) => return Err(ApiError::bad_request(format!("mode {}: once, hold or loop", shown(other, mask)))),
        };
        let times = match number_of(o, "times")? {
            None => None,
            Some(n) if n.fract() == 0.0 && (1.0..=MAX_ACT_TIMES as f64).contains(&n) => Some(n as u32),
            Some(n) => return Err(ApiError::bad_request(format!("times {n}: a whole number from 1 to {MAX_ACT_TIMES}"))),
        };
        let play = text_of(o, "play")?;
        // stop takes a name too, and its refusal says which: `{"stop": true}` reads like a switch.
        let stop = match o.get("stop") {
            None | Some(Value::Null) | Some(Value::String(_)) => text_of(o, "stop")?,
            Some(other) => return Err(ApiError::bad_request(format!("stop is {}: {STOP_TAKES}", kind_of(other)))),
        };
        Ok(ActRequest { play, mode, times, stop })
    }
}

/// What `stop` of `avatar_act` takes, as its refusal says it.
pub const STOP_TAKES: &str = "write the name of a held or looping gesture, or all";

/// An accepted `avatar_act` call, in words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Acted {
    pub note: String,
}

/// Plays one of the avatar's animations as a gesture, or stops one.
///
/// `avatar.act` goes out at most four times a second per terminal: a call that comes sooner
/// waits its turn, and a newer one takes its place. The record's `held` follows every call.
pub fn act(state: &AppState, pass: &Pass, req: ActRequest, now: i64) -> ApiResult<Acted> {
    pass.is_for(Call::Act)?;
    let tid = pass.tid.as_str();
    let (play, stop) = (trimmed(req.play), trimmed(req.stop));
    // The name as a refusal would show it, while the store lock is free.
    let quoted = play.as_deref().or(stop.as_deref()).map(|name| shown(name, &mask_of(state, tid))).unwrap_or_default();

    let mut guard = state.avatar.inner.lock();
    let Inner { records, act_count, .. } = &mut *guard;
    let rec = on_record(records, tid)?;
    let look = rec.design.look.as_ref();
    // The whole request is checked first: what is held, the event and the answer follow from it.
    let (held, body, note): (Option<Held>, Value, String) = match (play, stop) {
        (None, None) => return Err(ApiError::bad_request("act needs play (the name of an animation) or stop (a name, or all)")),
        (Some(_), Some(_)) => return Err(ApiError::bad_request("play and stop in one call: a play ends what was held, so send one of them")),
        (None, Some(name)) => {
            if name != "all" && !playable(look, &name) {
                return Err(no_animation(look, &quoted));
            }
            let still = rec.held.clone().filter(|held| name != "all" && held.name != name);
            let note = if name == "all" { "Stopped every gesture.".to_string() } else { format!("Stopped {name}.") };
            (still, json!({ "stop": name }), note)
        }
        (Some(name), None) => {
            if !playable(look, &name) {
                return Err(no_animation(look, &quoted));
            }
            let mode = req.mode.unwrap_or(ActMode::Once);
            let times = req.times.unwrap_or(1);
            if !(1..=MAX_ACT_TIMES).contains(&times) {
                return Err(ApiError::bad_request(format!("times {times}: a whole number from 1 to {MAX_ACT_TIMES}")));
            }
            match mode {
                ActMode::Once => {
                    // Played again and again, a paint track that ends elsewhere than it starts
                    // jumps there every round: that is a flash.
                    if let Some(l) = look.filter(|l| times > 1 && !look::closes(l, &name)) {
                        let prop = open_paint(l, &name).unwrap_or("paint");
                        return Err(ApiError::bad_request(format!("{name} cannot repeat: its {prop} ends on another value than it starts with. Play it once, or close the track.")));
                    }
                    let note = if times > 1 { format!("Playing {name} {times} times.") } else { format!("Playing {name}.") };
                    (None, json!({ "play": name, "mode": "once", "times": times }), note)
                }
                ActMode::Hold | ActMode::Loop => {
                    if times != 1 {
                        return Err(ApiError::bad_request("times is for mode once: a held or looping gesture stays until it is stopped"));
                    }
                    if mode == ActMode::Loop && !loops(look, &name) {
                        return Err(ApiError::bad_request(format!("{name} does not loop: mode loop repeats only an animation declared loop: true. Play it once, or keep its last frame with mode hold.")));
                    }
                    let (held_mode, word, verb) = if mode == ActMode::Hold { (HeldMode::Hold, "hold", "Holding") } else { (HeldMode::Loop, "loop", "Looping") };
                    let note = format!("{verb} {name} until you stop it, play another or {} minutes pass.", HOLD_MAX_SECS / 60);
                    (Some(Held { name: name.clone(), mode: held_mode, since: now }), json!({ "play": name, "mode": word }), note)
                }
            }
        }
    };
    if rec.held != held {
        rec.held = held;
        rec.touch(now);
    }
    // At most four events a second: one that comes sooner waits, and a newer one replaces it.
    let first_to_wait = if rec.act_queued.is_none() && now - rec.act_at >= ACT_GAP_MS {
        emit_act(state, act_count, tid, rec, body, now);
        false
    } else {
        rec.act_queued.replace(body).is_none()
    };
    let turn_in = (ACT_GAP_MS - (now - rec.act_at)).clamp(0, ACT_GAP_MS) as u64;
    drop(guard);
    if first_to_wait {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let (state, tid) = (state.clone(), tid.to_string());
            runtime.spawn(async move {
                tokio::time::sleep(Duration::from_millis(turn_in)).await;
                act_turn(&state, &tid);
            });
        }
    }
    Ok(Acted { note })
}

/// The turn of a gesture that waited: it goes out now. (The sweep sends what a lost timer left.)
fn act_turn(state: &AppState, tid: &str) {
    let mut guard = state.avatar.inner.lock();
    let Inner { records, act_count, .. } = &mut *guard;
    let Some(rec) = records.get_mut(tid) else { return };
    if let Some(body) = rec.act_queued.take() {
        let due = rec.act_at + ACT_GAP_MS;
        emit_act(state, act_count, tid, rec, body, due);
    }
}

// ---------------------------------------------------------------- status

/// The arguments of `avatar_status`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StatusRequest {
    /// The authoring guide comes first (the transport adds it).
    pub guide: bool,
    /// The stored design comes back in full, the artwork as SVG.
    pub look: bool,
}

impl StatusRequest {
    /// `mask`: the session's secret masking ([`Pass::mask`]), for a refusal that quotes a key.
    pub fn parse(args: &Value, mask: &dyn Fn(&str) -> String) -> ApiResult<StatusRequest> {
        let o = fields(args, "status", &["guide", "look", "picture", "pose", "frames", "count"], mask)?;
        Ok(StatusRequest { guide: flag_of(o, "guide")?.unwrap_or(false), look: flag_of(o, "look")?.unwrap_or(false) })
    }
}

/// The answer of `avatar_status`. Its fields are written in this order, so the standing
/// instructions come first: serialise the struct itself (a `serde_json::Value` sorts its keys).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// What to do: the standing instructions of a session with an avatar, or why it has none.
    pub note: String,
    /// The session may use its avatar: it has one that is on, or gets one with its first call.
    pub allowed: bool,
    /// `null`: the session has no avatar.
    pub avatar: Option<StatusAvatar>,
    pub limits: StatusLimits,
}

impl Status {
    /// The answer as JSON text, `note` first. With the stored design in it the JSON is written
    /// without white space: set out line by line, a large design would be several times its
    /// size and be cut off by what a tool may answer.
    pub fn text(&self) -> String {
        let whole = self.avatar.as_ref().is_some_and(|avatar| avatar.design.is_some());
        if whole { serde_json::to_string(self) } else { serde_json::to_string_pretty(self) }.unwrap_or_default()
    }

    /// The answer beside a picture, in a line: who it is, and what stands open. A session
    /// that corrects its drawing asks for one picture after another, and the parts, the last
    /// lines and the limits said again with each would cost it more than the pictures do.
    pub fn brief(&self) -> String {
        let Some(avatar) = &self.avatar else { return self.note.clone() };
        // Named as the design answer names it: a name is optional.
        let who = if avatar.name.is_empty() { BRIEF_UNNAMED } else { avatar.name.as_str() };
        let mut out = format!("{who}, revision {}{}.", avatar.revision, if avatar.on { "" } else { ", off" });
        if let Some(held) = &avatar.held {
            let mode = match held.mode {
                HeldMode::Hold => "held",
                HeldMode::Loop => "looping",
            };
            out.push_str(&format!(" {} is {mode}.", held.name));
        }
        if let Some(ask) = &avatar.ask {
            out.push_str(&format!(" Question {} is open.", ask.ask_id));
        }
        // The default look has no parts to read of: what to send instead.
        // An avatar that is off takes no design call: nothing is asked of its session.
        out.push_str(if avatar.designed { BRIEF_MORE } else if avatar.on { BRIEF_DEFAULT } else { "" });
        out
    }
}

/// Where the rest of a status is, said at the end of [`Status::brief`].
pub const BRIEF_MORE: &str = " Its parts, last lines and limits: avatar_status without a picture.";
/// The end of [`Status::brief`] for an avatar that shows the default look.
pub const BRIEF_DEFAULT: &str = " It shows Workbench's default look: send svg with a design call to draw your own.";
/// How [`Status::brief`] names an avatar without a name.
pub const BRIEF_UNNAMED: &str = "Your avatar (no name yet)";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusAvatar {
    pub on: bool,
    pub name: String,
    /// It has a look of its own. False: it shows the default look, and the session designs one.
    pub designed: bool,
    pub revision: u64,
    pub via: Via,
    /// One line per part with an id: `id tag x y w h pivot x y`, in root coordinates, at rest,
    /// then `parent id` for a part that moves with another.
    pub parts: Vec<String>,
    /// The look's own animations.
    pub animations: Vec<String>,
    /// What the live inputs move, one line each: "lookX moves head rotate".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<String>,
    /// The built-in motion a gesture may name.
    pub built_in: Vec<String>,
    pub voice: Voice,
    /// A gesture that stays until it is stopped.
    pub held: Option<StatusHeld>,
    /// The open question.
    pub ask: Option<StatusAsk>,
    /// The last lines, newest last, with what became of them.
    pub lines: Vec<StatusLine>,
    /// With `look`: the stored design as an `avatar_design` call would send it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusHeld {
    pub name: String,
    pub mode: HeldMode,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusAsk {
    pub ask_id: String,
    pub text: String,
    pub choices: Vec<String>,
    pub free_text: bool,
    /// A call of the session waits for the answer.
    pub waiting: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusLine {
    pub id: String,
    pub from: Speaker,
    pub text: String,
    /// A line of the session: what became of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub became: Option<String>,
    /// A reply of the user: the question it answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    /// A reply of the user: how it was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<ReplyVia>,
}

/// The limits a session works within.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusLimits {
    pub say_chars: usize,
    pub ask_chars: usize,
    pub choices: usize,
    pub choice_chars: usize,
    pub name_chars: usize,
    pub reply_chars: usize,
    pub lines_and_questions_per_minute: usize,
    pub designs_per_minute: usize,
    pub gestures_per_minute: usize,
    pub status_calls_per_minute: usize,
    pub ask_calls_per_minute: usize,
    pub lines_waiting: usize,
    pub transcript_lines: usize,
    pub line_unheard_after_seconds: i64,
    pub wait_seconds_default: u64,
    pub wait_seconds_tool: u64,
    pub wait_seconds_hook: u64,
    pub gesture_times: u32,
    pub held_gesture_seconds: i64,
    pub design_bytes: usize,
    pub svg_bytes: usize,
    pub elements: usize,
    pub animations: usize,
    pub tracks_per_animation: usize,
    pub keyframes_per_track: usize,
    pub filters: usize,
    pub primitives_per_filter: usize,
    pub dashes: usize,
    pub parents: usize,
    pub drives: usize,
}

fn limits() -> StatusLimits {
    StatusLimits {
        say_chars: MAX_SAY_CHARS,
        ask_chars: MAX_ASK_CHARS,
        choices: MAX_CHOICES,
        choice_chars: MAX_CHOICE_CHARS,
        name_chars: MAX_NAME_CHARS,
        reply_chars: MAX_REPLY_CHARS,
        lines_and_questions_per_minute: SAY_PER_MINUTE,
        designs_per_minute: DESIGN_PER_MINUTE,
        gestures_per_minute: ACT_PER_MINUTE,
        status_calls_per_minute: STATUS_PER_MINUTE,
        ask_calls_per_minute: ASK_CALLS_PER_MINUTE,
        lines_waiting: MAX_PENDING_LINES,
        transcript_lines: MAX_LINES,
        line_unheard_after_seconds: LINE_STALE_SECS,
        wait_seconds_default: DEFAULT_WAIT_SECS,
        wait_seconds_tool: MAX_WAIT_TOOL_SECS,
        wait_seconds_hook: MAX_WAIT_HOOK_SECS,
        gesture_times: MAX_ACT_TIMES,
        held_gesture_seconds: HOLD_MAX_SECS,
        design_bytes: look::MAX_DESIGN_BYTES,
        svg_bytes: look::MAX_SVG_BYTES,
        elements: look::MAX_ELEMENTS,
        animations: look::MAX_ANIMATIONS,
        tracks_per_animation: look::MAX_TRACKS,
        keyframes_per_track: look::MAX_KEYFRAMES,
        filters: look::MAX_FILTERS,
        primitives_per_filter: look::MAX_FILTER_PRIMITIVES,
        dashes: look::MAX_DASHES,
        parents: look::MAX_PARENTS,
        drives: look::MAX_DRIVES,
    }
}

/// The stored design in the form of a design call, the artwork as SVG.
fn design_call(design: &Design) -> Value {
    let mut call = json!({ "name": design.name, "voice": design.voice });
    if let Some(look) = &design.look {
        call["svg"] = json!(look::to_svg(look));
        call["pivots"] = json!(look.pivots);
        call["roles"] = json!(look.roles);
        call["animations"] = json!(look.animations);
        call["states"] = json!(look.states);
        call["mouth"] = json!(look.mouth);
        call["gaze"] = json!(look.gaze);
        // Written only when the look has them, as the stored look writes them.
        if !look.parents.is_empty() {
            call["parents"] = json!(look.parents);
        }
        if !look.drives.is_empty() {
            call["drives"] = json!(look.drives);
        }
    }
    call
}

/// The session's avatar as the session needs to know it: whether it has one, its name, parts,
/// animation names, the open question, the last lines with what became of them, the limits.
pub fn status(state: &AppState, pass: &Pass, req: StatusRequest) -> ApiResult<Status> {
    pass.is_for(Call::Status)?;
    let any = super::sessions(state) == Sessions::Any;
    let inner = state.avatar.inner.lock();
    let Some(rec) = inner.records.get(&pass.tid).filter(|_| pass.avatar) else {
        // Where sessions take one themselves, the first call gives it one only while there is
        // room: with MAX_AVATARS sessions holding one, that call is refused, and this says so.
        let full = !inner.records.contains_key(&pass.tid) && inner.records.len() >= MAX_AVATARS;
        let (note, allowed) = match (any, full) {
            (true, false) => (NOTE_ANY.to_string(), true),
            (true, true) => (no_room(), false),
            (false, _) => (NO_AVATAR.to_string(), false),
        };
        return Ok(Status { note, allowed, avatar: None, limits: limits() });
    };
    let look = rec.design.look.as_ref();
    let from = rec.lines.len().saturating_sub(STATUS_LINES);
    let lines = rec
        .lines
        .iter()
        .skip(from)
        .map(|l| {
            let own = l.line.from == Speaker::Agent;
            StatusLine { id: l.line.id.clone(), from: l.line.from, text: l.line.text.clone(), became: own.then(|| fate(l.line.outcome, l.line.voice.as_deref())), ask_id: l.line.ask_id.clone().filter(|_| !own), via: l.line.via }
        })
        .collect();
    let avatar = StatusAvatar {
        on: rec.on,
        name: rec.design.name.clone(),
        designed: rec.designed(),
        revision: rec.revision,
        via: rec.via,
        parts: look.map(|l| look::parts(l).iter().map(ToString::to_string).collect()).unwrap_or_default(),
        animations: own_animations(look),
        drives: look.map(drives).unwrap_or_default(),
        built_in: built_in(look).into_iter().map(str::to_string).collect(),
        voice: rec.design.voice.clone(),
        held: rec.held.as_ref().map(|held| StatusHeld { name: held.name.clone(), mode: held.mode }),
        ask: rec.ask.as_ref().map(|open| StatusAsk { ask_id: open.ask.id.clone(), text: open.ask.text.clone(), choices: open.ask.choices.clone(), free_text: open.ask.free_text, waiting: open.ask.waiting }),
        lines,
        design: req.look.then(|| design_call(&rec.design)),
    };
    let note = match (rec.on, pass.via) {
        (false, _) => off_words(rec),
        (true, Via::Tools) => NOTE_TOOLS,
        (true, Via::Command) => NOTE_COMMAND,
    };
    Ok(Status { note: note.to_string(), allowed: rec.on, avatar: Some(avatar), limits: limits() })
}

// ---------------------------------------------------------------- pictures

/// A session without a look of its own asked for a picture.
pub const NO_LOOK_TO_DRAW: &str = "There is no look of your own to draw yet: this avatar shows Workbench's default look. Send svg with a design call first.";

/// What a session asks to see of its avatar.
#[derive(Debug, Clone, PartialEq)]
pub enum PictureRequest {
    /// The figure in one pose, on the light and on the dark panel.
    Sheet(preview::Pose),
    /// Moments of one of its animations (its own, or a built-in one), in a row, with these inputs
    /// held, and close up on the part `zoom` names, where it names one.
    Strip { animation: String, frames: usize, inputs: preview::Inputs, zoom: Option<String> },
}

impl PictureRequest {
    /// The look at rest.
    pub fn rest() -> PictureRequest {
        PictureRequest::Sheet(preview::Pose::default())
    }

    /// From `{picture?, pose?, frames?, count?}` beside whatever else the call takes (the caller
    /// has checked its keys). `picture: true` is the look at rest, `pose` the look in that pose,
    /// `frames` a strip of that animation with `count` moments, and with `pose` beside it the
    /// pose's inputs held for every moment and its `zoom` the close-up of each. `None`: no
    /// picture was asked for.
    ///
    /// What the session wrote is masked before the renderer sees it, so a sentence of the
    /// renderer that quotes a name never quotes a piece of a secret.
    pub fn parse(args: &Value, mask: &dyn Fn(&str) -> String) -> ApiResult<Option<PictureRequest>> {
        let Some(o) = args.as_object() else { return Ok(None) };
        let wanted = flag_of(o, "picture")?.unwrap_or(false);
        let frames = text_of(o, "frames")?;
        let count = number_of(o, "count")?;
        let pose = match o.get("pose").filter(|v| !v.is_null()) {
            None => None,
            Some(pose) => {
                let keys = fields(pose, "pose", &preview::POSE_KEYS, mask)?;
                let mut masked = keys.clone();
                for key in ["animation", "zoom"] {
                    if let Some(Value::String(name)) = keys.get(key) {
                        masked.insert(key.to_string(), Value::String(mask(name)));
                    }
                }
                Some(Value::Object(masked))
            }
        };
        if let Some(animation) = frames {
            let frames = match count {
                None => preview::STRIP_FRAMES_DEFAULT,
                Some(n) if n.fract() == 0.0 && (preview::STRIP_FRAMES_MIN as f64..=preview::STRIP_FRAMES_MAX as f64).contains(&n) => n as usize,
                Some(_) => return Err(ApiError::bad_request(format!("count is a whole number from {} to {}: how many moments of the animation to draw", preview::STRIP_FRAMES_MIN, preview::STRIP_FRAMES_MAX))),
            };
            let (inputs, zoom) = match &pose {
                None => (preview::Inputs::default(), None),
                Some(pose) if ["animation", "at"].iter().any(|key| pose.get(*key).is_some_and(|v| !v.is_null())) => {
                    return Err(ApiError::bad_request("with frames, a pose holds inputs and zoom only (lookX, lookY, mouth, pace, turn, held, zoom): the animation is the one frames names"));
                }
                Some(pose) => {
                    let pose = preview::Pose::from_json(pose).map_err(|why| ApiError::bad_request(mask(&why)))?;
                    (pose.inputs, pose.zoom)
                }
            };
            return Ok(Some(PictureRequest::Strip { animation: mask(&animation), frames, inputs, zoom }));
        }
        if count.is_some() {
            return Err(ApiError::bad_request("count says how many moments of an animation to draw: name the animation with frames"));
        }
        match pose {
            Some(pose) => Ok(Some(PictureRequest::Sheet(preview::Pose::from_json(&pose).map_err(|why| ApiError::bad_request(mask(&why)))?))),
            None => Ok(wanted.then(PictureRequest::rest)),
        }
    }

    /// What the picture shows, for the text beside it: nothing is written in the picture itself.
    /// It is said once the picture is drawn, of what the drawing reported (`played`: whether the
    /// animation is Workbench's built-in motion, which the session did not write; whether it
    /// loops; the moment a pose shows). Where the drawing did not say, the stored look it was
    /// drawn from (`look`) tells whether an animation is Workbench's.
    pub fn caption(&self, look: Option<&Value>, played: Option<&preview::Played>) -> String {
        const SMALL: &str = "under each, the whole figure in that pose at its resting size, and its face (always at rest)";
        let workbench = |name: &str| match played {
            Some(played) => played.builtin,
            None => look.is_some_and(|look| builtin::is_workbench(look, name)),
        };
        match self {
            PictureRequest::Sheet(pose) if *pose == preview::Pose::default() => "Picture: your avatar at rest, on the light panel and on the dark one; under each, at its resting size and as a face.".to_string(),
            PictureRequest::Sheet(pose) => {
                let (inside, note) = match &pose.animation {
                    None => (String::new(), String::new()),
                    Some((name, given)) => {
                        let who = if workbench(name) { "Workbench's built-in " } else { "" };
                        let moment = played.and_then(|played| played.moment).or_else(|| given.map(|at| preview::Moment { at, chosen: preview::Chosen::Given, still: false }));
                        match moment {
                            None => (format!(", inside {who}{name}"), String::new()),
                            Some(moment) => {
                                let at = moment_text(moment.at);
                                let inside = match moment.chosen {
                                    preview::Chosen::Given => format!(", inside {who}{name} at {at}"),
                                    preview::Chosen::Furthest => format!(", inside {who}{name} at {at}, where it has moved furthest"),
                                    preview::Chosen::Unmoved => format!(", inside {who}{name} at {at}, where every moment of it looks the same"),
                                };
                                // A given moment may be the one where it rests. The moment where its
                                // values are furthest from rest drawing nothing means that nothing of
                                // it shows: what it moves is out of sight, or a drive sets the same prop.
                                let note = match (moment.still, moment.chosen) {
                                    (true, preview::Chosen::Given) => format!(" At {at} {name} is where it rests: give another at."),
                                    (true, preview::Chosen::Furthest) => format!(" Nothing of {name} shows in this picture: what it moves is out of sight here, or a drive sets the same prop."),
                                    _ => String::new(),
                                };
                                (inside, note)
                            }
                        }
                    }
                };
                match &pose.zoom {
                    Some(part) => format!("Picture: a closer look at {part} in that pose{inside}, on the light panel and on the dark one; {SMALL}.{note}"),
                    None => format!("Picture: your avatar in that pose{inside}, on the light panel and on the dark one; {SMALL}.{note}"),
                }
            }
            PictureRequest::Strip { animation, frames, inputs, zoom } => {
                let whose = if workbench(animation) { ", Workbench's built-in motion made from your roles" } else { "" };
                let close = zoom.as_ref().map(|part| format!(", close up on {part}")).unwrap_or_default();
                let held = if *inputs == preview::Inputs::default() { "" } else { ", with those inputs held" };
                // A loop's moments are at 0, 1/n, 2/n … of one round: the last is not its end.
                if played.is_some_and(|played| played.looping) {
                    format!("Picture: {frames} moments of one round of {animation}{whose}{close}, evenly spaced{held}; after the last comes the first again.")
                } else {
                    format!("Picture: {frames} moments of {animation}{whose}{close}, from its start on the left to its end on the right{held}. A track that has played holds its last keyframe to the end, as in a gesture.")
                }
            }
        }
    }

    /// What drawing costs, said beside a status picture in a line: nothing where it costs about
    /// what the look at rest costs (the design answer said that), else both. About: by less than
    /// one in each number, which the design answer does not mention either, so a hundredth of a
    /// layer that rounds up to one is not a line. Both numbers are the figure in the full view at
    /// the large size: never about the zoom or a small panel. Beyond the budget (a close-up is
    /// drawn where the whole figure is not), the line says what that means.
    pub fn cost(&self, drawn: &preview::Picture) -> Option<String> {
        let (spent, rest) = (drawn.spent, drawn.rest);
        if (spent.filters - rest.filters).abs() < 1.0 && (spent.layers - rest.layers).abs() < 1.0 {
            return None;
        }
        let when = match self {
            PictureRequest::Sheet(_) => "In this pose",
            PictureRequest::Strip { .. } => "In its costliest moment",
        };
        let over = if spent.filters > preview::FILTER_WORK || spent.layers > preview::LAYER_WORK { " Past 60 only a close-up of it can be drawn." } else { "" };
        Some(format!(
            "{when} drawing costs about {:.0} of {:.0} for filters and {:.0} of {:.0} for layers; at rest {:.0} and {:.0}.{over}",
            spent.filters.ceil(),
            preview::FILTER_WORK,
            spent.layers.ceil(),
            preview::LAYER_WORK,
            rest.filters.ceil(),
            rest.layers.ceil()
        ))
    }

    /// What the renderer is asked for.
    pub fn want(&self) -> draw::Want {
        match self {
            PictureRequest::Sheet(pose) => draw::Want::Sheet(pose.clone()),
            PictureRequest::Strip { animation, frames, inputs, zoom } => draw::Want::Strip { animation: animation.clone(), frames: *frames, inputs: *inputs, zoom: zoom.clone() },
        }
    }
}

/// A moment of an animation as a caption writes it: at most four decimals, no trailing zeros,
/// and never "-0".
fn moment_text(at: f64) -> String {
    let rounded = (at * 10_000.0).round() / 10_000.0;
    format!("{}", if rounded == 0.0 { 0.0 } else { rounded })
}

/// The look a picture of the session's avatar is drawn from, as the browser is sent it. A
/// session without a look of its own has nothing to see yet; one whose avatar the user turned
/// off is not shown it either.
pub fn picture_look(state: &AppState, tid: &str) -> ApiResult<Value> {
    // The look is copied out: nothing is drawn under the store's lock.
    let look: Option<Look> = {
        let inner = state.avatar.inner.lock();
        let Some(rec) = inner.records.get(tid) else {
            // Where sessions take an avatar themselves, one that has none yet only has to draw.
            let any = super::sessions(state) == Sessions::Any;
            return Err(ApiError::conflict(if any { NO_LOOK_TO_DRAW } else { NO_AVATAR }));
        };
        if !rec.on {
            return Err(ApiError::conflict(off_words(rec)));
        }
        rec.design.look.clone()
    };
    let look = look.ok_or_else(|| ApiError::conflict(NO_LOOK_TO_DRAW))?;
    Ok(serde_json::to_value(&look)?)
}

/// What a design answer says about what drawing the look costs, when it is worth a word: a
/// session cannot see that a filter is dear. Both numbers are areas of the viewBox, not counts
/// (`preview::Spent`), and the sentence says so; it stays within 240 characters.
pub fn cost_line(spent: preview::Spent) -> Option<String> {
    (spent.filters >= 1.0 || spent.layers >= 1.0).then(|| {
        format!(
            "Drawing cost: about {:.0} of {:.0} for filters and {:.0} of {:.0} for layers, both in areas of the viewBox: a filtered, masked, clipped or half-transparent group adds its box to layers, and to filters once per primitive (a blur 6 times, a shadow 10).",
            spent.filters.ceil(),
            preview::FILTER_WORK,
            spent.layers.ceil(),
            preview::LAYER_WORK
        )
    })
}

// ---------------------------------------------------------------- reading arguments

/// What kind of JSON value this is, for a message.
pub(super) fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// The arguments of a call as an object with these keys and no others. `mask` masks a key the
/// refusal quotes ([`shown`]).
pub(super) fn fields<'v>(args: &'v Value, call: &str, known: &[&str], mask: &dyn Fn(&str) -> String) -> ApiResult<&'v Map<String, Value>> {
    let Some(o) = args.as_object() else { return Err(ApiError::bad_request(format!("{call} takes a JSON object with {}; this is {}", known.join(", "), kind_of(args)))) };
    match o.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(ApiError::bad_request(format!("{call} has no {}: it takes {}", shown(key, mask), known.join(", ")))),
        None => Ok(o),
    }
}

pub(super) fn text_of(o: &Map<String, Value>, key: &str) -> ApiResult<Option<String>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(ApiError::bad_request(format!("{key} is {}: write a string", kind_of(other)))),
    }
}

pub(super) fn flag_of(o: &Map<String, Value>, key: &str) -> ApiResult<Option<bool>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(ApiError::bad_request(format!("{key} is {}: write true or false", kind_of(other)))),
    }
}

pub(super) fn number_of(o: &Map<String, Value>, key: &str) -> ApiResult<Option<f64>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).map(Some).ok_or_else(|| ApiError::bad_request(format!("{key} is not a number a call can use"))),
        Some(other) => Err(ApiError::bad_request(format!("{key} is {}: write a number", kind_of(other)))),
    }
}

//! What only the user's devices do with an avatar (OWNER: avatar slice).
//!
//! Claiming a line to say it aloud and acknowledging what became of it, replying to the session,
//! dismissing a question, turning an avatar on or off and resetting its look, and telling a
//! session of its avatar ([`tell`], which the lifecycle uses as well). The routes that call these
//! are for signed-in devices only: a session never answers itself, and nothing here is reachable
//! with an agent token.
//!
//! A reply and the brief are what is put into a session. What a reply pastes is a fixed frame,
//! the question's id and the user's own words (or the choice that was clicked): never the
//! question, never anything else the session wrote; the brief is Workbench's own text. Both go
//! through the terminals slice's ask path, so they are refused while the session shows a dialog
//! or is still starting, and each paste runs in a task of its own that records what became of it.

use axum::http::StatusCode;
use serde::Serialize;
use serde_json::Value;

use super::session::{RAW_BYTES_PER_CHAR, busy, deliver, fields, flag_of, frame, not_pending, number_of, shown, text_of, unmasked};
use super::store::{
    Answer, AskEnd, AvatarInfo, Clock, Inner, LEASE_EXTRA_SECS, LINE_HELD_MAX_SECS, LINE_STALE_SECS, Lease, Line, MAX_AVATARS, MAX_REPLY_CHARS, MAX_TAB_CHARS, MAX_VOICE_NAME_CHARS, Outcome, Reason, Record, ReplyVia,
    SPEECH_CHARS_PER_SEC, Speaker, emit_ask, emit_changed, emit_line, ensure_loaded, next_id, running_agent, save,
};
use super::{brief, text};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::terminals::{self, TerminalInfo, TerminalKind, TerminalStatus};

/// What every device call checks first: avatars are enabled (403) and the terminal exists (404).
fn terminal(state: &AppState, tid: &str) -> ApiResult<TerminalInfo> {
    if !super::enabled(state) {
        return Err(ApiError::forbidden(super::session::SWITCHED_OFF));
    }
    ensure_loaded(state);
    state.terminals.info(tid).ok_or_else(|| ApiError::not_found("no such session"))
}

fn no_avatar() -> ApiError {
    ApiError::not_found("this session has no avatar")
}

// ---------------------------------------------------------------- claim and heard

/// The body of a claim: the tab that asks, and whether it gives the line back.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimRequest {
    /// The id a page load made for itself: at most [`MAX_TAB_CHARS`] of `[A-Za-z0-9_-]`.
    pub tab: String,
    pub release: bool,
}

impl ClaimRequest {
    pub fn parse(body: &Value) -> ApiResult<ClaimRequest> {
        let o = fields(body, "a claim", &["tab", "release"], &unmasked)?;
        let tab = text_of(o, "tab")?.ok_or_else(|| ApiError::bad_request("a claim needs tab: the id of the browser tab that asks"))?;
        Ok(ClaimRequest { tab, release: flag_of(o, "release")?.unwrap_or(false) })
    }
}

/// The answer to a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    /// This tab may say the line aloud, for `lease_ms`.
    pub speak: bool,
    pub lease_ms: i64,
    /// Another tab holds the line for this long: worth one more try then. 0: there is nothing to
    /// try again for.
    pub retry_ms: i64,
}

const NOT_YOURS: Claim = Claim { speak: false, lease_ms: 0, retry_ms: 0 };

/// How long a tab may hold a line: its estimated speech time and a margin.
fn lease_ms(text: &str) -> i64 {
    text.chars().count() as i64 * 1000 / SPEECH_CHARS_PER_SEC + LEASE_EXTRA_SECS * 1000
}

/// A tab asks to say a line aloud. Claims are leases: the first tab to ask gets the line for its
/// speech time, the holder's next claim renews it, another tab is told when to try again, a
/// lapsed lease can be taken, and `release` gives it back.
///
/// A line that has a final outcome, or is older than [`LINE_STALE_SECS`], is nobody's to say:
/// staleness is the server's decision alone. The one exception is the tab that is saying it: the
/// holder of a live lease may renew it until the line is [`LINE_HELD_MAX_SECS`] old, so a long
/// line said by a slow voice is not cut off in its middle. A line that was only shown as a
/// caption (or hidden) can still be claimed.
pub fn claim(state: &AppState, tid: &str, line_id: &str, req: ClaimRequest, now: i64) -> ApiResult<Claim> {
    terminal(state, tid)?;
    let tab_ok = !req.tab.is_empty() && req.tab.len() <= MAX_TAB_CHARS && req.tab.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !tab_ok {
        return Err(ApiError::bad_request(format!("tab is the id of the browser tab that asks: 1 to {MAX_TAB_CHARS} letters, digits, _ or -")));
    }
    let answer = {
        let mut inner = state.avatar.inner.lock();
        let rec = inner.records.get_mut(tid).ok_or_else(no_avatar)?;
        let on = rec.on;
        let l = rec.line_mut(line_id).filter(|l| l.line.from == Speaker::Agent).ok_or_else(|| ApiError::not_found("no such line"))?;
        if req.release {
            if !l.lease.as_ref().is_some_and(|lease| lease.tab == req.tab) {
                return Ok(NOT_YOURS);
            }
            l.lease = None;
            NOT_YOURS
        } else {
            let holder = l.lease.as_ref().is_some_and(|lease| lease.tab == req.tab && lease.until > now);
            let longest = if holder { LINE_HELD_MAX_SECS } else { LINE_STALE_SECS };
            let over = l.line.outcome.is_some_and(Outcome::is_final) || now - l.line.at > longest * 1000;
            if over || !on {
                return Ok(NOT_YOURS);
            }
            if let Some(lease) = l.lease.as_ref().filter(|lease| lease.until > now && lease.tab != req.tab) {
                return Ok(Claim { speak: false, lease_ms: 0, retry_ms: lease.until - now });
            }
            let lease_ms = lease_ms(&l.line.text);
            l.lease = Some(Lease { tab: req.tab, until: now + lease_ms });
            Claim { speak: true, lease_ms, retry_ms: 0 }
        }
    };
    // Who holds the line changed: a call that waits for this line looks again.
    state.avatar.bump();
    Ok(answer)
}

/// The body of an acknowledgement: what became of the line on this device.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardRequest {
    pub outcome: Outcome,
    /// The browser voice that spoke it.
    pub voice: Option<String>,
}

impl HeardRequest {
    pub fn parse(body: &Value) -> ApiResult<HeardRequest> {
        let o = fields(body, "an acknowledgement", &["outcome", "voice"], &unmasked)?;
        let name = text_of(o, "outcome")?.ok_or_else(|| ApiError::bad_request("an acknowledgement needs outcome: spoken, skipped, failed, shown or hidden"))?;
        let outcome = Outcome::REPORTED.into_iter().find(|o| o.name() == name).ok_or_else(|| ApiError::bad_request(format!("outcome {}: spoken, skipped, failed, shown or hidden", shown(&name, &unmasked))))?;
        Ok(HeardRequest { outcome, voice: text_of(o, "voice")? })
    }
}

/// A device says what became of a line. The line keeps the best outcome it got
/// (`spoken > skipped > failed > shown > hidden > unheard`), also when it was stale already:
/// a line the user asked to hear again is recorded as spoken.
pub fn heard(state: &AppState, tid: &str, line_id: &str, req: HeardRequest, now: i64) -> ApiResult<()> {
    terminal(state, tid)?;
    if !Outcome::REPORTED.contains(&req.outcome) {
        return Err(ApiError::bad_request("outcome: spoken, skipped, failed, shown or hidden"));
    }
    let voice = match req.voice.as_deref() {
        Some(voice) if voice.len() <= MAX_VOICE_NAME_CHARS * RAW_BYTES_PER_CHAR => text::plain(voice, MAX_VOICE_NAME_CHARS, |s| s.to_string()).map_err(|refusal| ApiError::bad_request(format!("the voice name has {refusal}")))?,
        Some(voice) => return Err(ApiError::bad_request(format!("the voice name is {} bytes long: at most {MAX_VOICE_NAME_CHARS} characters", voice.len()))),
        None => String::new(),
    };
    {
        let mut inner = state.avatar.inner.lock();
        let rec = inner.records.get_mut(tid).ok_or_else(no_avatar)?;
        let l = rec.line_mut(line_id).filter(|l| l.line.from == Speaker::Agent).ok_or_else(|| ApiError::not_found("no such line"))?;
        if l.line.outcome.is_some_and(|had| had.rank() >= req.outcome.rank()) {
            return Ok(());
        }
        l.line.outcome = Some(req.outcome);
        l.line.voice = Some(voice).filter(|v| req.outcome == Outcome::Spoken && !v.is_empty());
        if req.outcome.is_final() {
            l.lease = None;
        }
        let line = l.line.clone();
        rec.touch(now);
        emit_line(state, tid, rec, &line);
    }
    state.avatar.bump();
    Ok(())
}

// ---------------------------------------------------------------- reply

/// The body of a reply: an answer to the question `ask_id`, or without it a message.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyRequest {
    pub ask_id: Option<String>,
    pub text: Option<String>,
    /// The zero-based index into the question's choices.
    pub choice: Option<usize>,
    pub via: ReplyVia,
}

impl ReplyRequest {
    pub fn parse(body: &Value) -> ApiResult<ReplyRequest> {
        let o = fields(body, "a reply", &["askId", "text", "choice", "via"], &unmasked)?;
        let via = match text_of(o, "via")?.as_deref() {
            Some("click") => ReplyVia::Click,
            Some("typed") => ReplyVia::Typed,
            Some("voice") => ReplyVia::Voice,
            Some(other) => return Err(ApiError::bad_request(format!("via {}: click, typed or voice", shown(other, &unmasked)))),
            None => return Err(ApiError::bad_request("a reply needs via: click, typed or voice")),
        };
        let choice = match number_of(o, "choice")? {
            None => None,
            Some(n) if n.fract() == 0.0 && (0.0..1024.0).contains(&n) => Some(n as usize),
            Some(n) => return Err(ApiError::bad_request(format!("choice {n}: the index of a choice, counted from 0"))),
        };
        Ok(ReplyRequest { ask_id: text_of(o, "askId")?, text: text_of(o, "text")?, choice, via })
    }
}

/// How a reply reached the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivered {
    /// To the call that waited for the answer.
    Answer,
    /// Pasted into the session as a prompt.
    Prompt,
    /// Pasted into the session's input box and not sent: a dialog opened while it went in
    /// (`terminals::PASTED_NOT_SENT`). The user sends it there once the dialog is answered.
    #[serde(rename = "inBox")]
    InBox,
}

/// The answer of the reply route: `{delivered: "answer" | "prompt" | "inBox"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Replied {
    pub delivered: Delivered,
}

/// What the user gave: their own words (and those words with the session's secrets masked, for
/// everyone else), or one of the question's choices.
enum Given {
    Words { own: String, masked: String },
    Choice(usize),
}

/// A reply on its way into the session. Should its task end before the paste did (the runtime
/// stops), the record is told that nothing is being delivered any more.
struct Delivery {
    state: AppState,
    tid: String,
    number: u64,
    over: bool,
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if self.over {
            return;
        }
        if let Some(rec) = self.state.avatar.inner.lock().records.get_mut(&self.tid) {
            if rec.delivering.as_ref().is_some_and(|(number, _)| *number == self.number) {
                rec.delivering = None;
            }
        }
    }
}

/// What a reply that is pasted brings along to be recorded once the paste went in.
struct Pasting {
    prompt: String,
    /// The text of the user's line: their words with the session's secrets masked, or the choice.
    stored: String,
    ask_id: Option<String>,
    via: ReplyVia,
}

/// The user replies to the session through its avatar. It never guesses: with `ask_id` the text
/// or choice answers exactly that question, and when it is not the open one nothing is pasted
/// or recorded (409 `not_pending`); without `ask_id` the text is a message, also while a
/// question is open.
///
/// With a call waiting for the answer, the answer goes to that call. Otherwise it is pasted into
/// the session as a prompt, which may take several seconds (the ask path waits for a pause in
/// typing); a second reply meanwhile is refused (409 `busy`), and a refusal of the session is
/// passed on with nothing recorded and the question open as before. A reply whose Enter was
/// withheld because a dialog opened while it was pasted went in all the same: it stands in the
/// session's input box, and is recorded as delivered there (`inBox`).
pub async fn reply(state: &AppState, tid: &str, req: ReplyRequest, now: i64) -> ApiResult<Replied> {
    terminal(state, tid)?;
    let clock = Clock::at(now);
    // The reply is one line. The session gets the user's own words; what is stored and
    // broadcast has the session's secrets masked.
    let given = match (req.text.as_deref(), req.choice) {
        (Some(_), Some(_)) => return Err(ApiError::bad_request("a reply is text or a choice, not both")),
        (None, None) => return Err(ApiError::bad_request("a reply needs text, or the choice of an open question")),
        (None, Some(_)) if req.ask_id.is_none() => return Err(ApiError::bad_request("a choice answers a question: send its askId")),
        (None, Some(index)) => Given::Choice(index),
        (Some(text), None) => {
            if text.len() > MAX_REPLY_CHARS * RAW_BYTES_PER_CHAR {
                return Err(ApiError::bad_request(format!("the reply is {} bytes long: at most {MAX_REPLY_CHARS} characters", text.len())));
            }
            let own = text::plain(text, MAX_REPLY_CHARS, |s| s.to_string()).map_err(|refusal| ApiError::bad_request(format!("the reply has {refusal}")))?;
            if own.is_empty() {
                return Err(ApiError::bad_request("the reply is empty"));
            }
            // A mask may be longer than what it hides: the masked text is cut, never refused.
            let masked: String = text::plain(text, usize::MAX, |s| state.terminals.redact_text(tid, s)).unwrap_or_default().chars().take(MAX_REPLY_CHARS).collect();
            Given::Words { own, masked }
        }
    };

    let (number, prompt, stored) = {
        let mut guard = state.avatar.inner.lock();
        let Inner { records, boot, line_count, delivery_count, .. } = &mut *guard;
        let rec = records.get_mut(tid).ok_or_else(no_avatar)?;
        if !rec.on {
            return Err(ApiError::conflict("this session's avatar is off: turn it on to reply through it"));
        }
        if rec.delivering.is_some() {
            return Err(busy("a reply is being delivered to this session: wait for it"));
        }
        let via = rec.via;
        let (answer, stored) = match (&req.ask_id, given) {
            (None, Given::Words { own, masked }) => (Answer { text: own, choice: None, via: req.via }, masked),
            (None, Given::Choice(_)) => return Err(ApiError::bad_request("a choice answers a question: send its askId")),
            (Some(ask_id), given) => {
                let Some(open) = rec.ask.as_mut().filter(|open| open.ask.id == *ask_id) else {
                    return Err(not_pending("that question is not open any more: nothing was sent"));
                };
                let (answer, stored) = match given {
                    Given::Choice(index) => {
                        let Some(text) = open.ask.choices.get(index).cloned() else {
                            return Err(ApiError::bad_request(format!("choice {index}: the question has {} choices, counted from 0", open.ask.choices.len())));
                        };
                        (Answer { text: text.clone(), choice: Some(index), via: req.via }, text)
                    }
                    Given::Words { .. } if !open.ask.free_text => return Err(ApiError::bad_request("this question takes one of its choices")),
                    Given::Words { own, masked } => (Answer { text: own, choice: None, via: req.via }, masked),
                };
                if let Some((_, waiter)) = open.waiter.take() {
                    open.ask.waiting = false;
                    let prompt = frame(via, Some(ask_id.as_str()), &answer);
                    if waiter.send(AskEnd::Answered { answer: answer.clone(), prompt }).is_ok() {
                        // The call that waited has it: the question is answered. Its own line stays in
                        // the transcript while the reply joins it, as a browser keeps it: a browser hears of
                        // the reply's line before it hears that the question closed, and trims by the same rule.
                        let line = Line { id: next_id('L', boot, line_count), at: now, from: Speaker::User, text: stored, lang: None, gesture: None, ask_id: Some(ask_id.clone()), via: Some(req.via), outcome: None, voice: None };
                        rec.push_line(line.clone());
                        rec.ask = None;
                        rec.touch(now);
                        emit_line(state, tid, rec, &line);
                        emit_ask(state, tid, rec, "answered", None);
                        return Ok(Replied { delivered: Delivered::Answer });
                    }
                    // That call is gone without a word: the answer goes in as a prompt.
                    let ask = open.ask.clone();
                    rec.touch(now);
                    emit_ask(state, tid, rec, "waiter_left", Some(&ask));
                }
                (answer, stored)
            }
        };
        *delivery_count += 1;
        rec.delivering = Some((*delivery_count, req.ask_id.clone()));
        (*delivery_count, frame(via, req.ask_id.as_deref(), &answer), stored)
    };

    // The paste and what records it run in a task of their own, which the request waits for. A
    // request that goes away (a phone locked, a tab closed) cannot cut in between: an answer that
    // went into the session is recorded, and the question it answers is closed.
    let delivery = Delivery { state: state.clone(), tid: tid.to_string(), number, over: false };
    let pasting = Pasting { prompt, stored, ask_id: req.ask_id, via: req.via };
    tokio::spawn(delivered(delivery, pasting, clock)).await.map_err(|_| ApiError::internal("the reply was not delivered to the end"))?
}

/// Pastes a reply into the session, then records it: a line of the user, and the question it
/// answers closed. A refusal of the session is passed on as it is: nothing is recorded, and the
/// question is open as before. A reply left in the session's input box by a dialog that opened
/// while it was pasted went in: it is recorded as any other, so a second reply does not paste
/// the same answer beside it.
async fn delivered(mut delivery: Delivery, pasting: Pasting, clock: Clock) -> ApiResult<Replied> {
    let (state, tid, number) = (delivery.state.clone(), delivery.tid.clone(), delivery.number);
    let pasted = deliver(&state, &tid, &pasting.prompt).await;
    delivery.over = true;
    let now = clock.now();
    let in_box = matches!(&pasted, Err(e) if e.message == terminals::PASTED_NOT_SENT);
    let (pasted, delivered) = if in_box { (Ok(()), Delivered::InBox) } else { (pasted, Delivered::Prompt) };

    let mut guard = state.avatar.inner.lock();
    let Inner { records, boot, line_count, .. } = &mut *guard;
    let Some(rec) = records.get_mut(&tid) else { return pasted.map(|()| Replied { delivered }) };
    if rec.delivering.as_ref().is_some_and(|(n, _)| *n == number) {
        rec.delivering = None;
    }
    pasted?;
    rec.delivered_at = now;
    let line = Line { id: next_id('L', boot, line_count), at: now, from: Speaker::User, text: pasting.stored, lang: None, gesture: None, ask_id: pasting.ask_id.clone(), via: Some(pasting.via), outcome: None, voice: None };
    rec.push_line(line.clone());
    rec.touch(now);
    emit_line(&state, &tid, rec, &line);
    if let Some(ask_id) = &pasting.ask_id {
        // Unless it closed meanwhile (the session ended, the avatar was turned off).
        if rec.ask.as_ref().is_some_and(|open| open.ask.id == *ask_id) {
            rec.ask = None;
            emit_ask(&state, &tid, rec, "answered", None);
        }
    }
    Ok(Replied { delivered })
}

/// The user dismisses the open question without answering: the call that waits learns so.
pub fn dismiss_ask(state: &AppState, tid: &str, ask_id: &str, now: i64) -> ApiResult<()> {
    terminal(state, tid)?;
    let mut inner = state.avatar.inner.lock();
    let rec = inner.records.get_mut(tid).ok_or_else(no_avatar)?;
    if !rec.ask.as_ref().is_some_and(|open| open.ask.id == ask_id) {
        return Err(not_pending("that question is not open any more"));
    }
    if rec.answering(ask_id) {
        return Err(busy("an answer to this question is being delivered: wait for it"));
    }
    rec.close_ask(state, tid, Reason::Dismissed, now);
    Ok(())
}

// ---------------------------------------------------------------- on, off, reset

/// The answer of turning an avatar on.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnedOn {
    pub info: AvatarInfo,
    /// The session does not know of its avatar yet and is running: it is to be told now
    /// ([`tell`]).
    pub brief: bool,
}

/// Gives the session an avatar, or turns its avatar on again. Only agent sessions have one, and
/// at most [`MAX_AVATARS`] open or running ones. A session without a record whose stored file is there gets
/// that avatar back, turned on, with its design (`store::take_over`).
pub fn turn_on(state: &AppState, tid: &str, now: i64) -> ApiResult<TurnedOn> {
    let info = terminal(state, tid)?;
    if info.kind != TerminalKind::Agent {
        return Err(ApiError::bad_request("only agent sessions have an avatar"));
    }
    let started_with = info.meta.get("avatar") == Some(&Value::Bool(true));
    let later = info.meta.get("avatarBrief").and_then(Value::as_str) == Some("later");
    let mut guard = state.avatar.inner.lock();
    let Inner { records, asked, held, .. } = &mut *guard;
    if !records.contains_key(tid) {
        // Only avatars of open or running sessions take room (`store::room`): closing a session
        // frees its room.
        if !super::store::room(records, held, tid) {
            return Err(ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limited", format!("{MAX_AVATARS} sessions have an avatar already: close sessions you no longer need first")));
        }
        held.remove(tid);
        // A stored file of its own is this session's avatar, as a start reads it: turned on
        // below with the design it holds, never replaced by a new one.
        let rec = match super::store::take_over(state, &info, now) {
            Some(rec) => rec,
            None => {
                let mut rec = Record::new(&info, now);
                // A session that was started with the option was told at its start, unless its
                // first prompt was a command of its CLI (`"avatarBrief": "later"`, as
                // `store::created` reads it).
                rec.unbriefed = !started_with || later;
                save(state, tid, &rec);
                rec
            }
        };
        emit_changed(state, tid, &rec);
        asked.retain(|id| id != tid);
        records.insert(tid.to_string(), rec);
    }
    let rec = records.get_mut(tid).ok_or_else(no_avatar)?;
    if !rec.on {
        rec.on = true;
        // The user chose now: whatever could not be read of the stored file before is settled.
        rec.unreadable = false;
        // Turning an avatar on always tells the session, also one that was told of it before: at
        // its last call it read that the user turned the avatar off, and it would not use it again.
        rec.unbriefed = true;
        rec.touch(now);
        save(state, tid, rec);
        emit_changed(state, tid, rec);
    }
    // The lifecycle tries again to tell a session that refused its brief before.
    rec.brief_refusals = 0;
    Ok(TurnedOn { info: rec.info(tid), brief: rec.unbriefed && info.status == TerminalStatus::Running })
}

/// The session was told of its avatar: the brief was pasted into it at `now`. Should the user
/// have turned the avatar off while it went in, the session has been refused with "turned off"
/// since, or will be at its next call: it is still to be told once the avatar is on again.
pub fn briefed(state: &AppState, tid: &str, now: i64) {
    let mut inner = state.avatar.inner.lock();
    let Some(rec) = inner.records.get_mut(tid) else { return };
    rec.brief_refusals = 0;
    // The session that starts working now does so because of the brief, not because the user
    // answered an open question in the terminal.
    rec.delivered_at = rec.delivered_at.max(now);
    if rec.unbriefed && rec.on {
        rec.unbriefed = false;
        save(state, tid, rec);
    }
}

// ---------------------------------------------------------------- telling a session of its avatar

/// A session is told of its avatar by one telling at a time: the value exists only for the one
/// that came first, and dropping it takes the session out of the set again.
struct Telling {
    state: AppState,
    tid: String,
}

impl Telling {
    fn begin(state: &AppState, tid: &str) -> Option<Telling> {
        let first = state.avatar.briefing.lock().insert(tid.to_string());
        first.then(|| Telling { state: state.clone(), tid: tid.to_string() })
    }
}

impl Drop for Telling {
    fn drop(&mut self) {
        self.state.avatar.briefing.lock().remove(&self.tid);
    }
}

/// Tells a running session of the avatar it was not told of: pastes the brief by itself through
/// the ask path, which refuses while the session shows a dialog, is starting or the user types.
/// `Given` when the avatar has no look of its own, `Again` when it has one; either ends with
/// going on with what the session was doing. `POST …/on` and the lifecycle both tell this way,
/// one telling at a time per session.
///
/// The paste and what records it run in a task of their own, which this waits for: a request
/// that goes away cannot cut in between, so a brief that went in is not pasted again. The task
/// looks once more whether the session is still to be told, right before the paste: the user may
/// have turned the avatar off in between. (The ask path may still wait a few seconds for the
/// session's first prompt or a pause in the user's typing; a brief that goes in after an off
/// leaves the session to be told at the next on, `briefed`.)
///
/// A brief that went in and whose Enter was withheld (a dialog opened meanwhile,
/// `terminals::PASTED_NOT_SENT`) stands in the session's input box, where the user finds it once
/// the dialog is answered: it counts as told, and is not pasted a second time beside it.
///
/// `Some(true)`: told. `Some(false)`: the session refused it and is still to be told; the
/// refusal counts towards [`BRIEF_REFUSALS`](super::store::BRIEF_REFUSALS). `None`: there is
/// nothing to tell (it was told, its avatar is off, it is not running), or another telling of it
/// is under way.
pub async fn tell(state: &AppState, tid: &str, now: i64) -> Option<bool> {
    // While avatars are switched off nobody is told: the session stays to be told, once they are
    // on again and it is idle.
    if !super::enabled(state) {
        return None;
    }
    let telling = Telling::begin(state, tid)?;
    // Looked at now that no other telling can be in between; the terminal is asked once the
    // store lock is free again.
    let (via, designed) = state.avatar.inner.lock().records.get(tid).filter(|rec| rec.on && rec.unbriefed).map(|rec| (rec.via, rec.designed()))?;
    let info = state.terminals.info(tid).filter(running_agent)?;
    let text = brief::telling(designed, via, terminals::in_container(&info));
    let clock = Clock::at(now);
    let told = tokio::spawn(async move {
        let Telling { state, tid } = &telling;
        let still = super::enabled(state) && state.avatar.inner.lock().records.get(tid.as_str()).is_some_and(|rec| rec.on && rec.unbriefed);
        if !still {
            return None;
        }
        match deliver(state, tid, &text).await {
            // Pasted and sent, or pasted and left in the input box by a dialog: told.
            Ok(()) => {
                briefed(state, tid, clock.now());
                Some(true)
            }
            Err(e) if e.message == terminals::PASTED_NOT_SENT => {
                briefed(state, tid, clock.now());
                Some(true)
            }
            Err(e) => {
                if e.status != StatusCode::CONFLICT {
                    tracing::warn!(terminal = %tid, "avatar: the session could not be told of its avatar: {}", e.code);
                }
                if let Some(rec) = state.avatar.inner.lock().records.get_mut(tid) {
                    rec.brief_refusals = rec.brief_refusals.saturating_add(1);
                }
                Some(false)
            }
        }
    });
    told.await.ok().flatten()
}

/// Turns the session's avatar off, on every device. The look is kept; an open question closes
/// (`dismissed`), and the session's calls are refused from now on.
pub fn turn_off(state: &AppState, tid: &str, now: i64) -> ApiResult<AvatarInfo> {
    terminal(state, tid)?;
    let mut inner = state.avatar.inner.lock();
    let rec = inner.records.get_mut(tid).ok_or_else(no_avatar)?;
    let was_on = rec.on;
    if was_on {
        rec.close_ask(state, tid, Reason::Dismissed, now);
        // Nothing is shown or spoken from now on: what waited to be presented is unheard.
        rec.pending_unheard(state, tid, now);
        rec.on = false;
        rec.held = None;
        rec.touch(now);
        save(state, tid, rec);
        emit_changed(state, tid, rec);
    }
    let info = rec.info(tid);
    drop(inner);
    if was_on {
        state.avatar.bump();
    }
    Ok(info)
}

/// Back to the default look: the session's artwork and what moves it are gone, its name and
/// voice stay.
pub fn reset(state: &AppState, tid: &str, now: i64) -> ApiResult<AvatarInfo> {
    terminal(state, tid)?;
    // Not between a design call's reading of the design and its storing of the new one.
    let _one_at_a_time = state.avatar.designing.lock();
    let mut inner = state.avatar.inner.lock();
    let rec = inner.records.get_mut(tid).ok_or_else(no_avatar)?;
    if rec.design.look.take().is_some() {
        rec.revision += 1;
        // The default look has the built-in motion and nothing else.
        rec.held = rec.held.take().filter(|held| super::look::BUILTIN.contains(&held.name.as_str()));
        rec.touch(now);
        save(state, tid, rec);
        emit_changed(state, tid, rec);
    }
    Ok(rec.info(tid))
}

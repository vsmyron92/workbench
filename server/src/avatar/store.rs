//! The avatars' records (OWNER: avatar slice).
//!
//! One record per terminal of kind agent: whether its avatar is on, what the session designed
//! (name, look, voice), how the session drives it, and, in memory only, the transcript of its
//! last lines, the open question with the call that waits for it, which tab may say a line
//! aloud, a held gesture and the rate limits.
//!
//! What a record keeps across restarts is one file, `data_dir/avatar/<terminalId>.json` (0600
//! in a 0700 directory): `{version, on, name, designed, look, voice, via, unbriefed?,
//! unreadable?}`, written compactly, so that it is never much larger than the look the validator
//! measured. It is written when the record is created and whenever one of those fields changes,
//! and removed with the terminal. A stored file is read through the same validator as a design
//! call, so a hand-written one gets no further than a session's call would; one that fails is
//! kept aside as `<terminalId>.json.corrupt` and the avatar shows the default look. Its `on`
//! still holds when the file is JSON with a boolean `on`: a design that cannot be read never
//! turns an avatar the user turned off back on. A file whose switch cannot be read either leaves
//! the avatar off until the user turns it on, and the record says why (`unreadable`). A file
//! whose terminal was not loaded at this start, while its `meta.json` is still on disk, is left
//! alone: it gets no record, and waits for the start that reads the terminal again.
//!
//! Every stored file of a loaded agent terminal gets its record at a start, however many there
//! are: [`MAX_AVATARS`] bounds new avatars only. A record made later for a terminal that has none
//! (its first call, a device's on, its `terminal.created`) never replaces a file the terminal
//! has: that file is read and taken over as a start reads it ([`take_over`]).
//!
//! Locks. [`AvatarState::inner`] is the one store lock: every change of a record, its file and
//! its events happens under it, so events of one avatar leave in the order of its changes. It is
//! never held across an await, and nothing is asked of another slice while it is held: terminal
//! infos are read before it is taken, and a design is validated (which masks text with the
//! session's secrets) under `designing` alone. A stored file that a record takes over at run time
//! is read and validated under the store lock: that masks nothing and asks no other slice, and it
//! happens once for such a record. A file name is only ever built from an id that passes
//! `terminals::valid_id`: a terminal's own, or one read from the directory and checked.
//!
//! Lifecycle: [`on_event`] follows `terminal.*` events (and tells a session that was not told of
//! its avatar once it is idle, through `device::tell`), [`reconcile`] compares the records with
//! the terminals that exist (at start, and when events were missed), [`sweep`] ends what only
//! time ends. Each takes its clock as a value.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use super::look::{self, Design, Look, Voice};
use super::{Sessions, Via};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::events::Event;
use crate::terminals::{self, AgentState, TerminalInfo, TerminalKind, TerminalStatus};

// ---------------------------------------------------------------- limits

/// Characters of one line a session says.
pub const MAX_SAY_CHARS: usize = 400;
/// Characters of one question.
pub const MAX_ASK_CHARS: usize = 300;
/// Choices of one question.
pub const MAX_CHOICES: usize = 4;
/// Characters of one choice.
pub const MAX_CHOICE_CHARS: usize = 40;
/// Characters of the avatar's name.
pub use look::MAX_NAME_CHARS;
/// Characters of one reply of the user.
pub const MAX_REPLY_CHARS: usize = 4000;
/// Lines a transcript keeps, newest last.
pub const MAX_LINES: usize = 30;
/// Lines of the session that no device has acknowledged yet: one more is refused.
pub const MAX_PENDING_LINES: usize = 3;
/// Lines and questions a session may send in a minute (a question counts as a line).
pub const SAY_PER_MINUTE: usize = 6;
/// Design calls a session may make in a minute.
pub const DESIGN_PER_MINUTE: usize = 6;
/// Gestures a session may play or stop in a minute.
pub const ACT_PER_MINUTE: usize = 30;
/// Status calls a session may make in a minute.
pub const STATUS_PER_MINUTE: usize = 30;
/// Calls of ask a session may make in a minute, whatever they do: ask, wait again, withdraw.
/// (A new question also counts as a line, against [`SAY_PER_MINUTE`].)
pub const ASK_CALLS_PER_MINUTE: usize = 30;
/// A line no device acknowledged for this long is unheard, and is no longer given out to be spoken.
pub const LINE_STALE_SECS: i64 = 60;
/// The tab that is saying a line may renew its lease past [`LINE_STALE_SECS`], but not for ever:
/// this long after the line was said even its holder gets no more time (the longest line at the
/// slowest speech a browser is given takes about two minutes).
pub const LINE_HELD_MAX_SECS: i64 = 240;
/// How long a question waits for its answer when the call names no time.
pub const DEFAULT_WAIT_SECS: u64 = 45;
/// The longest wait of an MCP tool call.
pub const MAX_WAIT_TOOL_SECS: u64 = 50;
/// The longest wait of a hook request.
pub const MAX_WAIT_HOOK_SECS: u64 = 110;
/// A question no device acknowledged within this time is unseen: its call returns at once.
pub const UNSEEN_SECS: u64 = 5;
/// A held or looping gesture ends by itself after this long.
pub const HOLD_MAX_SECS: i64 = 120;
/// An open question nobody acknowledged is announced to the user after this long.
pub const NUDGE_AFTER_SECS: i64 = 8;
/// Such announcements per terminal in ten minutes.
pub const NUDGES_PER_10_MIN: usize = 3;
/// Sessions that have an avatar. One more is refused; none is evicted, and every avatar stored
/// for a loaded terminal is read at a start, also beyond this.
pub const MAX_AVATARS: usize = 64;
/// Sessions remembered as having asked for an avatar without having one.
pub const MAX_ASKED: usize = 64;
/// How often a gesture may play in a row.
pub const MAX_ACT_TIMES: u32 = 5;
/// The least time between two `avatar.act` events of one terminal: four a second.
pub const ACT_GAP_MS: i64 = 250;
/// Characters of the id a browser tab claims lines with.
pub const MAX_TAB_CHARS: usize = 64;
/// Characters of the name of the browser voice that spoke a line.
pub const MAX_VOICE_NAME_CHARS: usize = 80;
/// How fast a browser voice speaks, for estimates: characters a second.
pub const SPEECH_CHARS_PER_SEC: i64 = 15;
/// A lease lasts the estimated speech time of its line and this much more.
pub const LEASE_EXTRA_SECS: i64 = 15;
/// `say` with `wait` gives up after the estimated speech time and this much more.
pub const SAY_WAIT_EXTRA_SECS: u64 = 10;
/// `say` with `wait` returns this long after a caption was shown when no tab speaks the line.
pub const SAY_WAIT_SOFT_MS: u64 = 1500;
/// A session that starts working this soon after a reply was delivered through its avatar did
/// so because of that reply, not because the user answered in the terminal.
pub const MOVED_ON_QUIET_SECS: i64 = 5;
/// A stored avatar larger than this is not read.
pub const MAX_STORED_BYTES: u64 = look::MAX_DESIGN_BYTES as u64;
/// Lines `avatar_status` reports.
pub const STATUS_LINES: usize = 6;
/// A session that was not told of its avatar is told when it is idle: at most this often.
pub const BRIEF_RETRY_SECS: i64 = 30;
/// Refusals of the brief after which the lifecycle stops telling the session, until its avatar is
/// turned on again.
pub const BRIEF_REFUSALS: u32 = 5;

const STORED_VERSION: u64 = 1;
const MINUTE_MS: i64 = 60_000;
const TEN_MINUTES_MS: i64 = 600_000;

// ---------------------------------------------------------------- wire types

/// What became of a line of the session, best first. A device reports the first five;
/// `unheard` is the server's own, for a line nobody acknowledged in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Spoken,
    Skipped,
    Failed,
    Shown,
    Hidden,
    Unheard,
}

impl Outcome {
    /// What a device may report.
    pub const REPORTED: [Outcome; 5] = [Outcome::Spoken, Outcome::Skipped, Outcome::Failed, Outcome::Shown, Outcome::Hidden];

    pub fn name(self) -> &'static str {
        match self {
            Outcome::Spoken => "spoken",
            Outcome::Skipped => "skipped",
            Outcome::Failed => "failed",
            Outcome::Shown => "shown",
            Outcome::Hidden => "hidden",
            Outcome::Unheard => "unheard",
        }
    }

    /// `spoken > skipped > failed > shown > hidden > unheard`: a line keeps the best it got.
    pub(super) fn rank(self) -> u8 {
        match self {
            Outcome::Spoken => 5,
            Outcome::Skipped => 4,
            Outcome::Failed => 3,
            Outcome::Shown => 2,
            Outcome::Hidden => 1,
            Outcome::Unheard => 0,
        }
    }

    /// The line is over: nobody will say it any more. `shown` and `hidden` are soft: a caption
    /// tab acknowledges within milliseconds, and a voice tab may still say the line after it.
    pub fn is_final(self) -> bool {
        matches!(self, Outcome::Spoken | Outcome::Skipped | Outcome::Failed | Outcome::Unheard)
    }

    /// A device took the line up: from then on a call that waits for an answer counts its time.
    pub(super) fn acknowledged(self) -> bool {
        matches!(self, Outcome::Spoken | Outcome::Skipped | Outcome::Failed | Outcome::Shown)
    }

    /// The line reached the user for certain, as speech or as a caption in a visible window: a
    /// question that got this far needs no announcement outside the browser.
    pub(super) fn reached(self) -> bool {
        matches!(self, Outcome::Spoken | Outcome::Shown)
    }
}

/// Whose line it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    Agent,
    User,
}

/// How the user gave a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyVia {
    Click,
    Typed,
    Voice,
}

/// One utterance of the session, or one reply of the user, in the avatar's transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// `L<boot>-<n>`: never repeats, across restarts either.
    pub id: String,
    pub at: i64,
    pub from: Speaker,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<ReplyVia>,
    /// Lines of the session only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// The browser voice that spoke it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

/// An open question.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// `Q<boot>-<n>`.
    pub id: String,
    pub line_id: String,
    pub text: String,
    pub choices: Vec<String>,
    pub free_text: bool,
    pub asked_at: i64,
    /// A tool or hook call is waiting for the answer.
    pub waiting: bool,
}

/// How a gesture stays on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeldMode {
    Hold,
    Loop,
}

/// A gesture that stays until it is stopped, the next one plays or [`HOLD_MAX_SECS`] pass.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub name: String,
    pub mode: HeldMode,
    pub since: i64,
}

/// The per-terminal record as devices see it. `look: null` is the built-in default look.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarInfo {
    pub terminal_id: String,
    pub project_id: Option<String>,
    /// False: the user turned it off. Nothing is shown or spoken, and the session's calls are refused.
    pub on: bool,
    /// The session's own name for it, or "".
    pub name: String,
    /// It has a look of its own.
    pub designed: bool,
    /// One more with every change of the design, and with nothing else.
    pub revision: u64,
    pub look: Option<Look>,
    pub voice: Voice,
    pub via: Via,
    pub held: Option<Held>,
    pub ask: Option<Ask>,
    /// Newest last, at most [`MAX_LINES`].
    pub lines: Vec<Line>,
    pub created_at: i64,
    /// A client ignores a fetched record older than the one it holds.
    pub updated_at: i64,
}

/// The answer of `GET /api/avatar`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarList {
    pub enabled: bool,
    pub sessions: Sessions,
    pub avatars: Vec<AvatarInfo>,
    /// Running sessions that called for their avatar without having one. Empty while sessions
    /// take one themselves (`sessions = "any"`).
    pub asked: Vec<String>,
}

// ---------------------------------------------------------------- what a question ends with

/// Why a call that waited for an answer returns without one. The same words are the `change` of
/// the `avatar.ask` event that closes a question (the first two leave it open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Timeout,
    Unseen,
    Dismissed,
    Superseded,
    Withdrawn,
    SessionEnded,
    MovedOn,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::Timeout => "timeout",
            Reason::Unseen => "unseen",
            Reason::Dismissed => "dismissed",
            Reason::Superseded => "superseded",
            Reason::Withdrawn => "withdrawn",
            Reason::SessionEnded => "session_ended",
            Reason::MovedOn => "moved_on",
        }
    }
}

/// The user's answer to a question.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The user's own words, or the text of the choice.
    pub text: String,
    /// The zero-based index into the question's choices, for a choice.
    pub choice: Option<usize>,
    pub via: ReplyVia,
}

/// What reaches a call that waits for an answer.
#[derive(Debug)]
pub(super) enum AskEnd {
    /// The answer, and the prompt that carries it should the call be gone by now.
    Answered { answer: Answer, prompt: String },
    /// The question closed without an answer.
    Closed(Reason),
    /// A newer call of the session waits for this answer now.
    Handed,
}

// ---------------------------------------------------------------- records

/// A line with what only the server needs to know of it.
#[derive(Debug)]
pub(super) struct LineRec {
    pub line: Line,
    /// The tab that may say it aloud, and until when.
    pub lease: Option<Lease>,
    /// The session was told what became of it.
    pub told: bool,
}

#[derive(Debug)]
pub(super) struct Lease {
    pub tab: String,
    pub until: i64,
}

/// The open question with the call that waits for it.
#[derive(Debug)]
pub(super) struct OpenAsk {
    pub ask: Ask,
    /// The waiting call: its number among this question's waiters, and the way to it.
    pub waiter: Option<(u64, oneshot::Sender<AskEnd>)>,
    /// How many calls have waited for this question.
    pub waiters: u64,
    /// The user was told of it outside the browser.
    pub nudged: bool,
}

#[derive(Debug)]
pub(super) struct Record {
    pub project_id: Option<String>,
    pub on: bool,
    pub design: Design,
    pub revision: u64,
    pub via: Via,
    pub held: Option<Held>,
    pub ask: Option<OpenAsk>,
    pub lines: VecDeque<LineRec>,
    pub created_at: i64,
    pub updated_at: i64,
    /// The session's state as the last terminal event showed it.
    pub session: Option<AgentState>,
    /// A reply is being pasted into the session: its number, and the question it answers (none:
    /// a message). Meanwhile that question is neither open to another reply nor closed.
    pub delivering: Option<(u64, Option<String>)>,
    /// When a reply, or the brief, was last pasted into the session.
    pub delivered_at: i64,
    /// The session has not been told of its avatar: a device gave it one, its first prompt was a
    /// command of its CLI, or a conversation started anew without the brief. Stored.
    pub unbriefed: bool,
    /// It is off because its stored file could not be read (nobody knows what the user chose),
    /// not because the user turned it off: until a device turns it on. Stored.
    pub unreadable: bool,
    /// When the lifecycle last tried to tell the session, and whether it did so in the session's
    /// present idle: once per idle.
    pub brief_at: i64,
    pub brief_idle: bool,
    /// Refusals of the brief since the avatar was last turned on.
    pub brief_refusals: u32,
    /// When the user was last told of open questions.
    pub nudges: VecDeque<i64>,
    /// When `avatar.act` was last emitted, and a newer one that waits its turn.
    pub act_at: i64,
    pub act_queued: Option<Value>,
}

impl Record {
    pub(super) fn new(info: &TerminalInfo, now: i64) -> Record {
        Record {
            project_id: info.project_id.clone(),
            on: true,
            design: Design::default(),
            revision: 0,
            via: via_of(info),
            held: None,
            ask: None,
            lines: VecDeque::new(),
            created_at: now,
            updated_at: now,
            session: info.agent.as_ref().map(|a| a.state),
            delivering: None,
            delivered_at: 0,
            unbriefed: false,
            unreadable: false,
            brief_at: 0,
            brief_idle: false,
            brief_refusals: 0,
            nudges: VecDeque::new(),
            act_at: 0,
            act_queued: None,
        }
    }

    /// It has a look of its own. A name or a voice alone is not a design.
    pub(super) fn designed(&self) -> bool {
        self.design.look.is_some()
    }

    pub(super) fn touch(&mut self, now: i64) {
        self.updated_at = self.updated_at.max(now);
    }

    pub(super) fn info(&self, tid: &str) -> AvatarInfo {
        AvatarInfo {
            terminal_id: tid.to_string(),
            project_id: self.project_id.clone(),
            on: self.on,
            name: self.design.name.clone(),
            designed: self.designed(),
            revision: self.revision,
            look: self.design.look.clone(),
            voice: self.design.voice.clone(),
            via: self.via,
            held: self.held.clone(),
            ask: self.ask.as_ref().map(|open| open.ask.clone()),
            lines: self.lines.iter().map(|l| l.line.clone()).collect(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    /// Lines of the session no device has acknowledged in any way.
    pub(super) fn pending(&self) -> usize {
        self.lines.iter().filter(|l| l.line.from == Speaker::Agent && l.line.outcome.is_none()).count()
    }

    pub(super) fn line_mut(&mut self, id: &str) -> Option<&mut LineRec> {
        self.lines.iter_mut().find(|l| l.line.id == id)
    }

    /// Adds a line as the newest; the oldest goes when there are more than [`MAX_LINES`]. The open
    /// question's own line stays while it is open: whether a device took the question up, and
    /// what a device acknowledges of it, are asked of that line.
    pub(super) fn push_line(&mut self, line: Line) {
        self.lines.push_back(LineRec { line, lease: None, told: false });
        let asked = self.ask.as_ref().map(|open| open.ask.line_id.clone());
        while self.lines.len() > MAX_LINES {
            let oldest = self.lines.iter().position(|l| Some(&l.line.id) != asked.as_ref()).unwrap_or(0);
            self.lines.remove(oldest);
        }
    }

    /// Whether a device took the open question `ask_id` up: its line has an outcome that `counts`,
    /// or a tab holds the line to say it aloud right now (a voice tab reports a line only once its
    /// speech has ended). `None`: that question is not open.
    pub(super) fn ask_taken_up(&self, ask_id: &str, counts: fn(Outcome) -> bool, now: i64) -> Option<bool> {
        let open = self.ask.as_ref().filter(|open| open.ask.id == ask_id)?;
        let line = self.lines.iter().find(|l| l.line.id == open.ask.line_id);
        Some(line.is_some_and(|l| l.line.outcome.is_some_and(counts) || l.lease.as_ref().is_some_and(|lease| lease.until > now)))
    }

    /// Whether a reply to the question with this id is being delivered right now.
    pub(super) fn answering(&self, ask_id: &str) -> bool {
        self.delivering.as_ref().is_some_and(|(_, of)| of.as_deref() == Some(ask_id))
    }

    /// Closes the open question, if there is one: the call that waits for it learns why, and
    /// the browsers that it is gone.
    pub(super) fn close_ask(&mut self, state: &AppState, tid: &str, reason: Reason, now: i64) -> bool {
        let Some(open) = self.ask.take() else { return false };
        if let Some((_, waiter)) = open.waiter {
            let _ = waiter.send(AskEnd::Closed(reason));
        }
        self.touch(now);
        emit_ask(state, tid, self, reason.name(), None);
        true
    }

    /// Every line of the session without an outcome becomes unheard: nobody will present it.
    pub(super) fn pending_unheard(&mut self, state: &AppState, tid: &str, now: i64) -> bool {
        let mut changed = Vec::new();
        for l in self.lines.iter_mut().filter(|l| l.line.from == Speaker::Agent && l.line.outcome.is_none()) {
            l.line.outcome = Some(Outcome::Unheard);
            l.lease = None;
            changed.push(l.line.clone());
        }
        if changed.is_empty() {
            return false;
        }
        self.touch(now);
        for line in &changed {
            emit_line(state, tid, self, line);
        }
        true
    }
}

/// How the session of this terminal drives its avatar when it has not used an interface yet.
pub(super) fn via_of(info: &TerminalInfo) -> Via {
    super::via_for(info.agent.as_ref().map(|a| a.provider).unwrap_or_default(), terminals::in_container(info))
}

/// "A running agent session": the only kind of terminal whose calls an avatar takes.
pub(super) fn running_agent(info: &TerminalInfo) -> bool {
    info.kind == TerminalKind::Agent && info.status == TerminalStatus::Running
}

// ---------------------------------------------------------------- rate limits

/// The attempts of one kind within the last minute. Refused attempts count as well.
#[derive(Debug, Default)]
pub(super) struct Window(VecDeque<i64>);

impl Window {
    /// Counts an attempt at `now`. False: `most` attempts were made within the last minute already.
    pub(super) fn attempt(&mut self, now: i64, most: usize) -> bool {
        while self.0.front().is_some_and(|at| now - *at >= MINUTE_MS) {
            self.0.pop_front();
        }
        let ok = self.0.len() < most;
        self.0.push_back(now);
        while self.0.len() > most {
            self.0.pop_front();
        }
        ok
    }
}

/// The rate limits of one terminal.
#[derive(Debug, Default)]
pub(super) struct Windows {
    /// Lines and new questions.
    pub say: Window,
    /// Calls of ask.
    pub ask: Window,
    pub design: Window,
    pub act: Window,
    pub status: Window,
}

// ---------------------------------------------------------------- the state

#[derive(Debug, Default)]
pub(super) struct Inner {
    /// Four base-36 characters of this server's start time: what keeps ids from repeating.
    pub boot: String,
    pub line_count: u64,
    pub ask_count: u64,
    pub act_count: u64,
    pub delivery_count: u64,
    /// By terminal id.
    pub records: HashMap<String, Record>,
    /// Running sessions that called for their avatar without having one, oldest first.
    pub asked: VecDeque<String>,
    pub limits: HashMap<String, Windows>,
    /// The server is stopping: no call waits for an answer any more.
    pub stopping: bool,
}

/// The next id of a line (`L<boot>-<n>`) or a question (`Q<boot>-<n>`): the boot stamp keeps it
/// from repeating across restarts, the count within one.
pub(super) fn next_id(prefix: char, boot: &str, count: &mut u64) -> String {
    *count += 1;
    format!("{prefix}{boot}-{count}")
}

/// The avatar slice's state: the records of every session that has an avatar.
pub struct AvatarState {
    pub(super) inner: Mutex<Inner>,
    loaded: AtomicBool,
    /// Held while the stored avatars are read: every entry point waits for that.
    loading: Mutex<()>,
    /// Held over a design call and a reset, so that designs are applied one after the other while
    /// the store lock stays free during validation.
    pub(super) designing: Mutex<()>,
    /// Moves on whenever a line got an outcome or its lease changed: calls that wait look again.
    pub(super) tick: watch::Sender<u64>,
    /// The sessions being told of their avatar right now (the brief is being pasted, for a
    /// device's request or for the lifecycle): a second telling of the same session does not
    /// paste it again. A leaf lock like the store lock, and never taken while that is held.
    pub(super) briefing: Mutex<HashSet<String>>,
    /// Tests take the place of the session a reply is pasted into.
    #[cfg(test)]
    pub(super) pastes: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Paste>>>,
}

impl Default for AvatarState {
    fn default() -> Self {
        Self {
            inner: Mutex::default(),
            loaded: AtomicBool::new(false),
            loading: Mutex::new(()),
            designing: Mutex::new(()),
            tick: watch::channel(0).0,
            briefing: Mutex::default(),
            #[cfg(test)]
            pastes: Mutex::new(None),
        }
    }
}

impl AvatarState {
    /// Calls that wait for something a device does look at the records again.
    pub(super) fn bump(&self) {
        self.tick.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Everything in memory is forgotten, as when the server starts again: the next entry point
    /// reads the stored avatars.
    #[cfg(test)]
    pub(super) fn restart(&self) {
        let _loading = self.loading.lock();
        *self.inner.lock() = Inner::default();
        self.loaded.store(false, Ordering::Release);
    }
}

/// A prompt on its way into a session, handed to a test instead.
#[cfg(test)]
pub(super) struct Paste {
    pub tid: String,
    pub prompt: String,
    pub done: oneshot::Sender<ApiResult<()>>,
}

/// A clock that started at a given time: what a call that waits reads the time from, so the
/// time it was given stays the measure.
#[derive(Debug, Clone, Copy)]
pub(super) struct Clock {
    base: i64,
    since: std::time::Instant,
}

impl Clock {
    pub(super) fn at(now: i64) -> Clock {
        Clock { base: now, since: std::time::Instant::now() }
    }

    pub(super) fn now(&self) -> i64 {
        self.base + self.since.elapsed().as_millis() as i64
    }
}

// ---------------------------------------------------------------- ids

/// The last start time an id prefix was made of, so two states of one process never share one.
static LAST_BOOT: AtomicI64 = AtomicI64::new(0);

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Four base-36 characters of the server's start time in milliseconds. The same four come back
/// after 28 minutes at the earliest, and then only for a start in the very same millisecond of
/// that span.
fn boot_stamp(started_at: i64) -> String {
    let stamp = match LAST_BOOT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(started_at.max(last + 1))) {
        Ok(last) | Err(last) => started_at.max(last + 1),
    };
    let mut n = stamp.rem_euclid(36_i64.pow(4)) as usize;
    let mut out = [b'0'; 4];
    for slot in out.iter_mut().rev() {
        *slot = BASE36[n % 36];
        n /= 36;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------- persistence

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Stored<'a> {
    version: u64,
    on: bool,
    name: &'a str,
    designed: bool,
    look: Option<&'a Look>,
    voice: &'a Voice,
    via: Via,
    /// Written only when it is true: absent reads as false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unbriefed: bool,
    /// Written only when it is true, as `unbriefed` is. A build that does not know it reads the
    /// file all the same: neither `strictly` nor `look::check_stored` looks at keys they do not
    /// know, and such a file is off with the default look.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unreadable: bool,
}

/// What a stored file says of its record.
struct Kept {
    on: bool,
    via: Option<Via>,
    unbriefed: bool,
    unreadable: bool,
    design: Design,
}

/// Why a stored file could not be used, and its `on` when the file is JSON with a boolean `on`.
struct Unread {
    on: Option<bool>,
    /// What is wrong with it, as the log says it after the file's path.
    why: String,
}

fn avatar_dir(state: &AppState) -> PathBuf {
    state.paths.data_dir.join("avatar")
}

/// The file of a terminal's avatar. The id is a terminal's own (records exist only for terminals
/// the terminals slice knew); its form is checked all the same before it becomes part of a path.
fn stored_file(state: &AppState, tid: &str) -> Option<PathBuf> {
    terminals::valid_id(tid).then(|| avatar_dir(state).join(format!("{tid}.json")))
}

/// Writes what a record keeps across restarts. Called with the store lock held.
///
/// Compact JSON: the validator measures a look as compact JSON, so the file is that look and a
/// few hundred bytes more, always within what [`read_stored`] reads. Indented, a look of many
/// keyframes would be several times larger, and be refused at the next start.
pub(super) fn save(state: &AppState, tid: &str, rec: &Record) {
    let Some(file) = stored_file(state, tid) else { return };
    let stored = Stored { version: STORED_VERSION, on: rec.on, name: &rec.design.name, designed: rec.designed(), look: rec.design.look.as_ref(), voice: &rec.design.voice, via: rec.via, unbriefed: rec.unbriefed, unreadable: rec.unreadable };
    let written = serde_json::to_vec(&stored).map_err(anyhow::Error::from).and_then(|bytes| crate::util::fs::write_atomic(&file, &bytes, 0o600));
    if let Err(e) = written {
        tracing::warn!(terminal = tid, "avatar: could not save {}: {e:#}", file.display());
    }
}

/// Removes a terminal's avatar file and the one kept aside as corrupt.
fn remove_stored(state: &AppState, tid: &str) {
    let Some(file) = stored_file(state, tid) else { return };
    let _ = std::fs::remove_file(file.with_extension("json.corrupt"));
    let _ = std::fs::remove_file(file);
}

/// What a stored file holds, read strictly. `Err` when it cannot be read, is too large, is not
/// JSON (a byte order mark in front, which some editors write, is no part of it, as for a
/// design's body), is not version 1 of the format, or its design does not pass the validator a
/// design call passes. The error says which, and carries the file's `on` when the file is JSON
/// with a boolean `on`, which holds all the same.
fn read_stored(file: &Path) -> Result<Kept, Unread> {
    let unread = |why: String| Unread { on: None, why };
    let not_read = |e: std::io::Error| unread(format!("could not be read ({})", e.kind()));
    let size = std::fs::metadata(file).map_err(not_read)?.len();
    if size > MAX_STORED_BYTES {
        return Err(unread(format!("is {size} bytes, larger than the {MAX_STORED_BYTES} a stored avatar may have")));
    }
    let bytes = std::fs::read(file).map_err(not_read)?;
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let value = serde_json::from_slice::<Value>(text).map_err(|_| unread("is not JSON".to_string()))?;
    let on = value.get("on").and_then(Value::as_bool);
    strictly(value).ok_or_else(|| Unread { on, why: "does not hold a valid avatar".to_string() })
}

/// The warning a stored file that could not be used is logged with: the file, what is wrong with
/// it, and what the avatar is now.
fn unread_warning(file: &Path, unread: &Unread) -> String {
    let now = match unread.on {
        None => "the avatar is off until you turn it on (with the default look)",
        Some(true) => "the avatar is on with the default look",
        Some(false) => "the avatar stays off as you left it (with the default look)",
    };
    format!("avatar: {} {}; it is kept as .corrupt, and {now}", file.display(), unread.why)
}

/// A stored file's JSON as its record, or `None` when any part of it is not what [`save`] writes.
fn strictly(value: Value) -> Option<Kept> {
    let stored = value.as_object()?;
    if stored.get("version").and_then(Value::as_u64) != Some(STORED_VERSION) {
        return None;
    }
    let on = match stored.get("on") {
        Some(Value::Bool(on)) => *on,
        _ => return None,
    };
    let via = match stored.get("via") {
        None => None,
        Some(via) => Some(serde_json::from_value::<Via>(via.clone()).ok()?),
    };
    let unbriefed = match stored.get("unbriefed") {
        None => false,
        Some(Value::Bool(unbriefed)) => *unbriefed,
        Some(_) => return None,
    };
    let unreadable = match stored.get("unreadable") {
        None => false,
        Some(Value::Bool(unreadable)) => *unreadable,
        Some(_) => return None,
    };
    if !matches!(stored.get("designed"), None | Some(Value::Bool(_))) {
        return None;
    }
    // Whether it is designed follows from the look that passes, never from the file's word.
    Some(Kept { on, via, unbriefed, unreadable, design: look::check_stored(value)? })
}

/// The record a stored file of a loaded agent terminal gives, as a start reads it: what the file
/// holds, or, for a file that does not hold a valid avatar, the default look with the file kept
/// aside as `.corrupt` and a warning that says why. The user's switch holds where it can be
/// read; where it cannot, nobody knows what the user chose, and the avatar is off until they
/// turn it on (`unreadable`).
fn from_file(state: &AppState, id: &str, info: &TerminalInfo, file: &Path, now: i64) -> Record {
    let mut rec = Record::new(info, info.created_at.min(now));
    rec.updated_at = now;
    match read_stored(file) {
        Ok(kept) => {
            rec.on = kept.on;
            rec.via = kept.via.unwrap_or(rec.via);
            rec.unbriefed = kept.unbriefed;
            rec.unreadable = kept.unreadable && !kept.on;
            rec.design = kept.design;
        }
        Err(unread) => {
            tracing::warn!(terminal = id, "{}", unread_warning(file, &unread));
            let _ = std::fs::rename(file, file.with_extension("json.corrupt"));
            rec.on = unread.on.unwrap_or(false);
            rec.unreadable = unread.on.is_none();
            save(state, id, &rec);
        }
    }
    rec
}

/// The record of a terminal's own stored file, for a terminal that has no record: `None` when it
/// has no file. A record made for a terminal is never a default one over the file it has (one a
/// start left without a record), so the user's switch and the session's design are not lost:
/// the file is read as a start reads it. Called with the store lock held, and only for a
/// terminal of kind agent the terminals slice knows.
pub(super) fn take_over(state: &AppState, info: &TerminalInfo, now: i64) -> Option<Record> {
    let file = stored_file(state, &info.id).filter(|file| stored_there(file))?;
    Some(from_file(state, &info.id, info, &file, now))
}

/// Whether something stands at a stored file's path. A path that cannot be looked at counts as
/// there: it may hold what the user chose.
fn stored_there(file: &Path) -> bool {
    !matches!(std::fs::symlink_metadata(file), Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory))
}

/// Reads the stored avatars, once: every entry point of the slice calls it first. Every file of a
/// loaded agent terminal gets its record, however many there are ([`from_file`]: one that does not
/// hold a valid avatar is kept aside and its session shows the default look). A file whose
/// terminal is gone (unknown, and without its `meta.json` on disk) is removed; one whose terminal
/// was not loaded is kept. Each file that gets no record is logged once, with why.
pub(super) fn ensure_loaded(state: &AppState) {
    let avatars = &state.avatar;
    if avatars.loaded.load(Ordering::Acquire) {
        return;
    }
    let _loading = avatars.loading.lock();
    if avatars.loaded.load(Ordering::Acquire) {
        return;
    }
    let dir = state.paths.data("avatar");
    crate::util::fs::set_mode(&dir, 0o700);
    let now = crate::util::now_ms();

    // Only names of the form `<terminal id>.json` are avatars; `<terminal id>.json.corrupt` is
    // what an earlier start kept aside.
    let mut names: Vec<String> = std::fs::read_dir(&dir).map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    let mut records = HashMap::new();
    for name in &names {
        let Some(id) = name.strip_suffix(".json").filter(|id| terminals::valid_id(id)) else { continue };
        let known = state.terminals.info(id);
        let Some(info) = known.clone().filter(|info| info.kind == TerminalKind::Agent) else {
            // A terminal that was not loaded this time (its meta.json could not be read) is not
            // gone: its avatar stays in its file, without a record, for the start that reads it.
            if known.is_none() && terminals::on_disk(state, id) {
                tracing::warn!(terminal = id, "avatar: the stored avatar of terminal {id} is kept without a record: the terminal was not loaded at this start (its meta.json could not be read), and the start that loads it reads the avatar again");
            } else {
                let why = if known.is_some() { "the terminal is not an agent session" } else { "the terminal is gone" };
                tracing::info!(terminal = id, "avatar: the stored avatar of terminal {id} is removed: {why}");
                remove_stored(state, id);
            }
            continue;
        };
        // No cap here: MAX_AVATARS bounds new avatars, and what the user and the session chose
        // for this one is in its file.
        records.insert(id.to_string(), from_file(state, id, &info, &dir.join(name), now));
    }
    for name in &names {
        let Some(id) = name.strip_suffix(".json.corrupt").filter(|id| terminals::valid_id(id)) else { continue };
        if !records.contains_key(id) && state.terminals.info(id).is_none() && !terminals::on_disk(state, id) {
            remove_stored(state, id);
        }
    }

    let mut inner = avatars.inner.lock();
    inner.boot = boot_stamp(state.started_at);
    inner.records = records;
    avatars.loaded.store(true, Ordering::Release);
}

// ---------------------------------------------------------------- events

pub(super) fn emit_changed(state: &AppState, tid: &str, rec: &Record) {
    state.events.emit("avatar.changed", rec.project_id.as_deref(), json!({ "terminalId": tid, "revision": rec.revision, "on": rec.on }));
}

/// `avatar.say`: present this line now. With `ask`, the line is that question.
pub(super) fn emit_say(state: &AppState, tid: &str, rec: &Record, line: &Line, ask: Option<&Ask>) {
    let mut data = json!({ "terminalId": tid, "line": line });
    if let Some(ask) = ask {
        data["ask"] = json!(ask);
    }
    state.events.emit("avatar.say", rec.project_id.as_deref(), data);
}

/// `avatar.ask`: `ask` is the record's question after the change, so `None` once it is closed.
pub(super) fn emit_ask(state: &AppState, tid: &str, rec: &Record, change: &str, ask: Option<&Ask>) {
    state.events.emit("avatar.ask", rec.project_id.as_deref(), json!({ "terminalId": tid, "change": change, "ask": ask }));
}

/// `avatar.line`: a line changed without being said (its outcome, a reply of the user).
pub(super) fn emit_line(state: &AppState, tid: &str, rec: &Record, line: &Line) {
    state.events.emit("avatar.line", rec.project_id.as_deref(), json!({ "terminalId": tid, "line": line }));
}

/// `avatar.act`, numbered: `body` holds `play` (with `mode` and `times`) or `stop`.
pub(super) fn emit_act(state: &AppState, count: &mut u64, tid: &str, rec: &mut Record, mut body: Value, now: i64) {
    *count += 1;
    body["terminalId"] = json!(tid);
    body["seq"] = json!(*count);
    rec.act_at = now;
    state.events.emit("avatar.act", rec.project_id.as_deref(), body);
}

// ---------------------------------------------------------------- reading

/// The answer of `GET /api/avatar`: every avatar, oldest first, and who asked for one.
pub fn list(state: &AppState) -> AvatarList {
    let (enabled, sessions) = (super::enabled(state), super::sessions(state));
    if !enabled {
        return AvatarList { enabled, sessions, avatars: Vec::new(), asked: Vec::new() };
    }
    ensure_loaded(state);
    let inner = state.avatar.inner.lock();
    let mut avatars: Vec<AvatarInfo> = inner.records.iter().map(|(tid, rec)| rec.info(tid)).collect();
    avatars.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.terminal_id.cmp(&b.terminal_id)));
    // While sessions take an avatar themselves none waits for the user to give it one, so none is
    // shown as having asked. Who asked before is still remembered, for a switch back.
    let asked = if sessions == Sessions::Any { Vec::new() } else { inner.asked.iter().cloned().collect() };
    AvatarList { enabled, sessions, avatars, asked }
}

/// The answer of `GET /api/avatar/{terminalId}`: 404 when the session has no avatar.
pub fn get(state: &AppState, tid: &str) -> ApiResult<AvatarInfo> {
    if !super::enabled(state) {
        return Err(ApiError::forbidden(super::session::SWITCHED_OFF));
    }
    ensure_loaded(state);
    let inner = state.avatar.inner.lock();
    inner.records.get(tid).map(|rec| rec.info(tid)).ok_or_else(|| ApiError::not_found("this session has no avatar"))
}

// ---------------------------------------------------------------- lifecycle

/// What a terminal event does to the avatars. The event's own time is the clock.
pub(super) fn on_event(state: &AppState, event: &Event) {
    let kind = event.kind.as_str();
    if !matches!(kind, "terminal.created" | "terminal.updated" | "terminal.exited" | "terminal.removed") {
        return;
    }
    let Some(id) = event.data.get("id").and_then(Value::as_str) else { return };
    ensure_loaded(state);
    let now = event.ts;
    if kind == "terminal.removed" {
        remove(state, id);
        return;
    }
    // Most terminals have no avatar: their updates end here.
    if kind != "terminal.created" && !state.avatar.inner.lock().records.contains_key(id) {
        return;
    }
    // These three carry the whole terminal.
    let Ok(info) = serde_json::from_value::<TerminalInfo>(event.data.clone()) else { return };
    if info.kind != TerminalKind::Agent {
        return;
    }
    match kind {
        "terminal.created" => created(state, &info, now),
        "terminal.updated" => updated(state, &info, now),
        _ => exited(state, &info, now),
    }
}

/// A new session: it takes over the avatar of the terminal it moved from, or gets one of its own
/// when it was started with the option. A terminal that has a record already keeps it, and one
/// that has a stored file of its own takes that over ([`take_over`]), whatever its meta says.
/// `reconcile` calls this as well, for a terminal whose `terminal.created` was missed.
///
/// `"avatarBrief": "later"` in its meta: the session was started without the brief (its first
/// prompt was a command of its CLI, which the brief would have become part of). It is told once
/// it is idle. A session that moved here with an avatar the user had turned off was started
/// without the brief as well (`brief_at_start` gives none): it is told once the avatar is on.
fn created(state: &AppState, info: &TerminalInfo, now: i64) {
    make_record(state, info, now, false);
}

/// [`created`], and with `made_up` what `reconcile` makes up for a `terminal.created` that was
/// missed. Made up later, two things differ. A session that is not running and whose only claim
/// is the option it was started with gets nothing: it was refused at the cap when it started (or
/// its event came while avatars were off), and room the user freed since is for sessions that
/// start now. And an avatar that is carried along on is told of once the session is idle: nobody
/// knows whether its first prompt held the brief (it did not when avatars were off at that time).
fn make_record(state: &AppState, info: &TerminalInfo, now: i64, made_up: bool) {
    if !super::enabled(state) {
        return;
    }
    let moved_from = info.meta.get("movedFrom").and_then(Value::as_str);
    let wanted = info.meta.get("avatar") == Some(&Value::Bool(true));
    let later = info.meta.get("avatarBrief").and_then(Value::as_str) == Some("later");
    // A record, and with it a file, is made only for a terminal the terminals slice knows.
    if state.terminals.info(&info.id).is_none() {
        return;
    }
    let tell = {
        let mut guard = state.avatar.inner.lock();
        let inner = &mut *guard;
        if inner.records.contains_key(&info.id) {
            return;
        }
        let carried = moved_from.and_then(|old| inner.records.get(old)).map(|old| (old.on, old.design.clone(), old.unreadable));
        let stored = stored_file(state, &info.id).is_some_and(|file| stored_there(&file));
        if carried.is_none() && !wanted && !stored {
            return;
        }
        if made_up && carried.is_none() && !stored && info.status != TerminalStatus::Running {
            return;
        }
        if inner.records.len() >= MAX_AVATARS {
            tracing::info!(terminal = %info.id, "avatar: {MAX_AVATARS} sessions have an avatar already; this one gets none");
            return;
        }
        let mut rec = match take_over(state, info, now) {
            Some(rec) => rec,
            None => {
                let mut rec = Record::new(info, now);
                // An avatar that moves along on is told of with the session's first prompt, also
                // when the session it came from was not told of it yet.
                rec.unbriefed = later || carried.as_ref().is_some_and(|(on, ..)| !on || made_up);
                if let Some((on, design, unreadable)) = carried {
                    rec.on = on;
                    rec.design = design;
                    rec.unreadable = unreadable;
                }
                save(state, &info.id, &rec);
                rec
            }
        };
        // Once, should the session be idle already.
        let tell = brief_due(&mut rec, info, now);
        emit_changed(state, &info.id, &rec);
        inner.asked.retain(|id| id != &info.id);
        inner.records.insert(info.id.clone(), rec);
        tell
    };
    if tell {
        tell_soon(state, &info.id, now);
    }
}

/// Whether the lifecycle tells the session of its avatar now, and if so it is marked as tried: a
/// session that was not told is told when it is idle, once per idle, at most every
/// [`BRIEF_RETRY_SECS`], and no more after [`BRIEF_REFUSALS`] refusals until its avatar is turned
/// on again. Called with the store lock held, after `rec.session` took the event's state.
fn brief_due(rec: &mut Record, info: &TerminalInfo, now: i64) -> bool {
    if rec.session != Some(AgentState::Idle) {
        rec.brief_idle = false;
        return false;
    }
    let due = rec.on && rec.unbriefed && running_agent(info) && info.open && !rec.brief_idle && rec.brief_refusals < BRIEF_REFUSALS && now - rec.brief_at >= BRIEF_RETRY_SECS * 1000;
    if due {
        rec.brief_idle = true;
        rec.brief_at = now;
    }
    due
}

/// Tells the session of its avatar in a task of its own: the lifecycle does not wait for a paste.
fn tell_soon(state: &AppState, tid: &str, now: i64) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let (state, tid) = (state.clone(), tid.to_string());
    runtime.spawn(async move {
        super::device::tell(&state, &tid, now).await;
    });
}

fn updated(state: &AppState, info: &TerminalInfo, now: i64) {
    // Asked before the store lock is taken: while the brief is being pasted, the session that
    // starts working does so because of it, not because the user answered in the terminal.
    let telling = state.avatar.briefing.lock().contains(&info.id);
    let mut changed = false;
    let tell = {
        let mut inner = state.avatar.inner.lock();
        let Some(rec) = inner.records.get_mut(&info.id) else { return };
        rec.project_id = info.project_id.clone();
        let was = rec.session;
        rec.session = info.agent.as_ref().map(|a| a.state);
        let tell = brief_due(rec, info, now);
        if !info.open {
            // The tab is closed: nobody answers there, and what was not presented will not be.
            // Those lines leave the transcript. The browsers learn first that they are over, then
            // that the record is to be read again: no event names a line that is gone.
            rec.close_ask(state, &info.id, Reason::SessionEnded, now);
            let pending: Vec<String> = rec.lines.iter().filter(|l| l.line.from == Speaker::Agent && l.line.outcome.is_none()).map(|l| l.line.id.clone()).collect();
            if rec.pending_unheard(state, &info.id, now) {
                rec.lines.retain(|l| !pending.contains(&l.line.id));
                emit_changed(state, &info.id, rec);
                changed = true;
            }
        } else if was == Some(AgentState::Idle) && rec.session == Some(AgentState::Working) {
            // The session went on although its question is open, nothing waits for the answer and
            // no reply went in through the avatar: the user answered in the terminal.
            let left = rec.ask.as_ref().is_some_and(|open| open.waiter.is_none());
            let quiet = rec.delivering.is_none() && !telling && now - rec.delivered_at > MOVED_ON_QUIET_SECS * 1000;
            if left && quiet {
                rec.close_ask(state, &info.id, Reason::MovedOn, now);
            }
        }
        tell
    };
    if changed {
        state.avatar.bump();
    }
    if tell {
        tell_soon(state, &info.id, now);
    }
}

/// A conversation of the session started anew (a restart): `told` says whether the brief went
/// with it. A record that exists, on or off, remembers whether the session is still to be told;
/// turning it on again tells it, and so does the lifecycle once the session is idle.
pub(super) fn conversation_started(state: &AppState, tid: &str, told: bool) {
    ensure_loaded(state);
    let mut inner = state.avatar.inner.lock();
    let Some(rec) = inner.records.get_mut(tid) else { return };
    if rec.unbriefed == told {
        rec.unbriefed = !told;
        save(state, tid, rec);
    }
}

/// The process ended (or could not start): its question is over, and what it said and nobody
/// took in is unheard.
fn exited(state: &AppState, info: &TerminalInfo, now: i64) {
    let changed = {
        let mut inner = state.avatar.inner.lock();
        let Some(rec) = inner.records.get_mut(&info.id) else { return };
        rec.project_id = info.project_id.clone();
        rec.session = info.agent.as_ref().map(|a| a.state);
        rec.close_ask(state, &info.id, Reason::SessionEnded, now);
        rec.pending_unheard(state, &info.id, now)
    };
    if changed {
        state.avatar.bump();
    }
}

/// The terminal is gone: so are its avatar, its file and its corrupt file.
fn remove(state: &AppState, tid: &str) {
    {
        let mut inner = state.avatar.inner.lock();
        inner.asked.retain(|id| id != tid);
        inner.limits.remove(tid);
        let Some(mut rec) = inner.records.remove(tid) else { return };
        if let Some((_, waiter)) = rec.ask.take().and_then(|open| open.waiter) {
            let _ = waiter.send(AskEnd::Closed(Reason::SessionEnded));
        }
        remove_stored(state, tid);
        state.events.emit("avatar.changed", rec.project_id.as_deref(), json!({ "terminalId": tid, "revision": rec.revision, "on": rec.on, "removed": true }));
    }
    state.avatar.bump();
}

/// Compares the records with the terminals that exist: at start (terminals are loaded, and may
/// have pruned, before this slice listens) and whenever events were missed. Avatars whose
/// terminal is gone are dropped; for terminals that are not running, questions close and what
/// was not presented is unheard. An agent terminal without a record gets what its missed
/// `terminal.created` would have given it ([`created`] decides, and tells).
pub(super) fn reconcile(state: &AppState, now: i64) {
    ensure_loaded(state);
    let ids: Vec<String> = state.avatar.inner.lock().records.keys().cloned().collect();
    // Asked before the lock is taken again.
    let infos: Vec<(String, Option<TerminalInfo>)> = ids.into_iter().map(|id| { let info = state.terminals.info(&id).filter(|i| i.kind == TerminalKind::Agent); (id, info) }).collect();
    let agents: Vec<TerminalInfo> = state.terminals.list().into_iter().filter(|info| info.kind == TerminalKind::Agent).collect();
    let mut changed = false;
    for (id, info) in infos {
        let Some(info) = info else {
            remove(state, &id);
            continue;
        };
        let mut inner = state.avatar.inner.lock();
        let Some(rec) = inner.records.get_mut(&id) else { continue };
        rec.project_id = info.project_id.clone();
        rec.session = info.agent.as_ref().map(|a| a.state);
        if info.status != TerminalStatus::Running {
            rec.close_ask(state, &id, Reason::SessionEnded, now);
            changed |= rec.pending_unheard(state, &id, now);
        }
    }
    // A moved or restarted session takes its avatar along, one started with the option gets
    // its own, and one that was not told is told once it is idle.
    let without: Vec<TerminalInfo> = {
        let inner = state.avatar.inner.lock();
        agents.into_iter().filter(|info| !inner.records.contains_key(&info.id)).collect()
    };
    for info in &without {
        make_record(state, info, now, true);
    }
    if changed {
        state.avatar.bump();
    }
}

/// What only time ends, run every few seconds: a line nobody acknowledged becomes unheard, a
/// held gesture ends, the user is told of a question nobody saw, a gesture that waited its turn
/// goes out, and what is remembered of sessions that are gone is forgotten.
pub(super) fn sweep(state: &AppState, now: i64) {
    ensure_loaded(state);
    // What only the terminals slice knows is asked before the lock is taken.
    let named: HashSet<String> = {
        let inner = state.avatar.inner.lock();
        inner.asked.iter().chain(inner.limits.keys()).cloned().collect()
    };
    // Per id: the terminal is gone (`None`), or whether it is a running agent session. An id that
    // joins a list after this look is not in the map, and stays.
    let known: HashMap<String, Option<bool>> = named.into_iter().map(|id| { let running = state.terminals.info(&id).map(|info| running_agent(&info)); (id, running) }).collect();

    let mut heard = false;
    {
        let mut guard = state.avatar.inner.lock();
        let Inner { records, asked, limits, act_count, .. } = &mut *guard;
        asked.retain(|id| !matches!(known.get(id), Some(None | Some(false))) && !records.contains_key(id));
        limits.retain(|id, _| known.get(id) != Some(&None));

        for (tid, rec) in records.iter_mut() {
            // A line is stale when nobody acknowledged it in time and no tab is saying it.
            let mut stale = Vec::new();
            for l in rec.lines.iter_mut() {
                let waiting = l.line.from == Speaker::Agent && l.line.outcome.is_none();
                let leased = l.lease.as_ref().is_some_and(|lease| lease.until > now);
                if waiting && !leased && now - l.line.at > LINE_STALE_SECS * 1000 {
                    l.line.outcome = Some(Outcome::Unheard);
                    l.lease = None;
                    stale.push(l.line.clone());
                }
            }
            if !stale.is_empty() {
                rec.touch(now);
                for line in &stale {
                    emit_line(state, tid, rec, line);
                }
                heard = true;
            }

            if let Some(held) = rec.held.take_if(|held| now - held.since >= HOLD_MAX_SECS * 1000) {
                rec.touch(now);
                emit_act(state, act_count, tid, rec, json!({ "stop": held.name }), now);
            }
            if rec.act_queued.is_some() && now - rec.act_at >= ACT_GAP_MS {
                if let Some(body) = rec.act_queued.take() {
                    emit_act(state, act_count, tid, rec, body, now);
                }
            }

            // The nudge: once per question, when it has been open for a while and no device spoke
            // it, showed it as a caption or is saying it right now. A session blocked in a tool
            // call reads as working, so this is how a user who is not looking learns of the
            // question.
            let due = rec.ask.as_ref().filter(|open| !open.nudged && now - open.ask.asked_at >= NUDGE_AFTER_SECS * 1000 && rec.delivering.is_none()).map(|open| (open.ask.id.clone(), open.ask.text.clone()));
            if let Some((ask_id, question)) = due {
                let seen = rec.ask_taken_up(&ask_id, Outcome::reached, now).unwrap_or(false);
                while rec.nudges.front().is_some_and(|at| now - *at >= TEN_MINUTES_MS) {
                    rec.nudges.pop_front();
                }
                if !seen && rec.nudges.len() < NUDGES_PER_10_MIN {
                    rec.nudges.push_back(now);
                    if let Some(open) = rec.ask.as_mut() {
                        open.nudged = true;
                    }
                    state.events.emit("avatar.nudge", rec.project_id.as_deref(), json!({ "terminalId": tid, "askId": ask_id, "name": rec.design.name, "question": question }));
                }
            }
        }
    }
    if heard {
        state.avatar.bump();
    }
}

/// The server stops: every call that waits for an answer returns, and none waits from now on.
pub(super) fn wake_all(state: &AppState) {
    {
        let mut inner = state.avatar.inner.lock();
        inner.stopping = true;
        for rec in inner.records.values_mut() {
            if let Some(open) = rec.ask.as_mut() {
                if let Some((_, waiter)) = open.waiter.take() {
                    open.ask.waiting = false;
                    let _ = waiter.send(AskEnd::Closed(Reason::SessionEnded));
                }
            }
        }
    }
    state.avatar.bump();
}

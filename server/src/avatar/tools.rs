//! The avatar's MCP tools (OWNER: avatar slice).
//!
//! Five tools, each a way into one state function of `session`: `avatar_status`,
//! `avatar_design`, `avatar_say`, `avatar_ask`, `avatar_act`. A tool acts on the calling
//! session's own avatar and takes no terminal id: the session is the terminal the caller's
//! agent token is bound to (`McpCtx::terminal_id`), and a caller that is not a session is
//! refused. Every call passes the gate first, which counts the attempt, and only then are its
//! arguments read.
//!
//! `avatar_design` answers with a picture of what was drawn, and `avatar_status` with one when it
//! is asked for (`ToolOutput::Images`): a session cannot see the browser. A picture is drawn in
//! a process of its own (`draw`), two at a time for all sessions and one at a time for each; a
//! design call draws its look there before it stores it.
//!
//! None of them changes remote state, so none is `mutating`. `avatar_status` only reads; the
//! other four do something for the user to see (`McpTool::acts`), and the two an agent calls
//! all the time, `avatar_say` and `avatar_act`, stay out of the Activity view when they succeed
//! (`McpTool::quiet`).
//!
//! `avatar_ask` waits inside the call, at most `MAX_WAIT_TOOL_SECS`: an MCP client gives a call
//! about a minute. When the client goes away the platform aborts the call's task, and the
//! waiter's own `Drop` leaves the question open (or pastes an answer it already holds).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::draw::{self, NotDrawn};
use super::preview::{Picture, Pose};
use super::session::{self, ActRequest, AskArgs, Call, Pass, PictureRequest, SayRequest, StatusRequest};
use super::store::MAX_WAIT_TOOL_SECS;
use super::{Via, brief};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

pub fn tools() -> Vec<McpTool> {
    vec![status_tool(), design_tool(), say_tool(), ask_tool(), act_tool()]
}

/// The gate for a tool call: the caller is a session (else 400), and what `session::gate` asks.
fn enter(state: &AppState, ctx: &McpCtx, call: Call) -> ApiResult<Pass> {
    let tid = ctx.terminal_id.as_deref().filter(|tid| !tid.is_empty()).ok_or_else(session::not_a_session)?;
    session::gate(state, tid, call, Via::Tools, crate::util::now_ms())
}

/// A design call with what drawing its look brought: the picture for the session, or why there
/// is none.
pub(super) struct DesignAnswer {
    pub designed: session::Designed,
    /// The look at rest, where the avatar has a look of its own and it could be drawn.
    pub picture: Option<Picture>,
    /// Why a look that is stored has no picture (an older look that costs too much to draw).
    pub no_picture: Option<String>,
}

/// A design call, in three steps: the arguments are validated and applied (on a thread of its
/// own: reading artwork takes time in proportion to its size); a look that changed is drawn at
/// rest, in a process of its own; only then is the design stored. A look that cannot be drawn
/// within what a picture may cost is refused and nothing changes: the browser would pay for it on
/// every frame, and the session could not be shown it.
pub(super) async fn design(state: &AppState, pass: Pass, args: Value) -> ApiResult<DesignAnswer> {
    let tid = pass.terminal_id().to_string();
    let (checked, pass) = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || session::design_check(&state, &pass, &args).map(|checked| (checked, pass))).await.map_err(|_| ApiError::internal("the design call did not finish"))??
    };
    let mut picture = None;
    if let Some(look) = checked.to_draw() {
        let look = serde_json::to_value(look)?;
        match draw_in_turn(&tid, &look, &draw::Want::Sheet(Pose::default()), false).await {
            Ok(drawn) => picture = Some(drawn),
            Err(not @ (NotDrawn::Unavailable(_) | NotDrawn::Busy(_))) => return Err(session::too_busy(format!("{}\nNothing was changed.", not.sentence()))),
            Err(not) => {
                let mut why = state.terminals.redact_text(&tid, &not.sentence());
                if checked.same_artwork() {
                    why.push_str(". The artwork this avatar already has is what costs too much: draw it again more simply (send svg)");
                }
                return Err(ApiError::bad_request(format!("{why}\nNothing was changed.")));
            }
        }
    }
    let designed = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || session::design_store(&state, &pass, &checked, crate::util::now_ms())).await.map_err(|_| ApiError::internal("the design call did not finish"))??
    };
    // A look that was not drawn for this call (the call left it as it was) is drawn for the answer.
    let mut no_picture = None;
    if designed.drawn && picture.is_none() {
        match self::picture(state, &tid, PictureRequest::rest()).await {
            Ok(drawn) => picture = Some(drawn),
            Err(e) => no_picture = Some(e.message),
        }
    }
    Ok(DesignAnswer { designed, picture, no_picture })
}

impl DesignAnswer {
    /// The answer as a session reads it: the design answer, then what the picture shows and
    /// what drawing it costs, or why there is no picture. `sees`: the picture travels with the
    /// answer (the tool); a session that drives its avatar with a command asks for it.
    pub fn text(&self, sees: bool) -> String {
        let mut text = self.designed.text.clone();
        match (&self.picture, &self.no_picture) {
            (Some(picture), _) => {
                if sees {
                    text.push_str(&format!("\n{} Look at it and correct what is off.", PictureRequest::rest().caption()));
                } else {
                    text.push_str("\nTo see it, ask for a picture and open the file.");
                }
                if let Some(cost) = session::cost_line(picture.spent) {
                    text.push_str(&format!("\n{cost}"));
                }
            }
            (None, Some(why)) => text.push_str(&format!("\nNo picture: {why}")),
            (None, None) => {}
        }
        text
    }
}

/// Pictures drawn at one time, for all sessions together: a drawing takes a core, and a session's
/// own rate says nothing about how many sessions ask at once. The others wait their turn.
static DRAWING: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(PICTURES_AT_ONCE)));
const PICTURES_AT_ONCE: usize = 2;
/// How long a picture waits for its turn before the caller is told to ask again.
const PICTURE_WAIT: Duration = Duration::from_secs(20);
/// After a drawing that died of its look, the session's next picture waits this long: one
/// session's costly poses do not keep the turns from the others.
const AFTER_A_COSTLY_ONE: Duration = Duration::from_secs(30);

/// Which sessions have a drawing under way, and which wait out a costly one.
static SESSIONS: LazyLock<Mutex<Turns>> = LazyLock::new(|| Mutex::new(Turns::default()));

#[derive(Default)]
struct Turns {
    drawing: HashSet<String>,
    resting: HashMap<String, Instant>,
}

/// A session's one drawing at a time: given back when the drawing ends or its caller leaves.
struct OwnTurn(String);

impl OwnTurn {
    /// `rests`: the pause after a costly drawing applies (to pictures a session asks for; a
    /// design call is drawn all the same, within its own rate).
    fn take(tid: &str, rests: bool, now: Instant) -> ApiResult<OwnTurn> {
        let mut turns = SESSIONS.lock();
        turns.resting.retain(|_, until| *until > now);
        if rests && turns.resting.contains_key(tid) {
            return Err(session::too_busy("The last picture of this avatar could not be drawn within what a picture may cost: change the design, or ask again in half a minute."));
        }
        if !turns.drawing.insert(tid.to_string()) {
            return Err(session::too_busy("A picture of this avatar is being drawn: wait for it, then ask for the next."));
        }
        Ok(OwnTurn(tid.to_string()))
    }
}

impl Drop for OwnTurn {
    fn drop(&mut self) {
        SESSIONS.lock().drawing.remove(&self.0);
    }
}

/// Runs `work` when one of `turns` is free, and gives the turn back when it ends or when the
/// caller goes away: dropping the future ends the work (a drawing is a process that is killed).
async fn in_turn<T>(turns: &Arc<Semaphore>, wait: Duration, work: impl Future<Output = T>) -> ApiResult<T> {
    let _turn = match tokio::time::timeout(wait, turns.clone().acquire_owned()).await {
        Ok(Ok(turn)) => turn,
        Ok(Err(_)) => return Err(ApiError::internal("the picture was not drawn")),
        Err(_) => return Err(session::too_busy("Workbench is drawing other pictures: ask for this one again in a moment.")),
    };
    Ok(work.await)
}

/// One drawing of a session's look: its own turn, then one of everybody's, then the process.
async fn draw_in_turn(tid: &str, look: &Value, want: &draw::Want, rests: bool) -> Result<Picture, NotDrawn> {
    let busy = |e: ApiError| NotDrawn::Busy(e.message);
    let _own = OwnTurn::take(tid, rests, Instant::now()).map_err(busy)?;
    let drawn = in_turn(&DRAWING, PICTURE_WAIT, draw::draw(look, want)).await.map_err(busy)?;
    // A design that could not be drawn was not stored: there is nothing left to ask a costly
    // picture of. A stored look whose pose could not be drawn is another matter.
    if rests && drawn == Err(NotDrawn::TooCostly) {
        SESSIONS.lock().resting.insert(tid.to_string(), Instant::now() + AFTER_A_COSTLY_ONE);
    }
    drawn
}

/// A picture of the session's avatar as it asked for it.
pub(super) async fn picture(state: &AppState, tid: &str, req: PictureRequest) -> ApiResult<Picture> {
    let look = session::picture_look(state, tid)?;
    draw_in_turn(tid, &look, &req.want(), true).await.map_err(|not| match not {
        NotDrawn::Unavailable(_) | NotDrawn::Busy(_) => session::too_busy(not.sentence()),
        // A sentence of the renderer may quote what the session wrote.
        other => ApiError::bad_request(state.terminals.redact_text(tid, &other.sentence())),
    })
}

// ---------------------------------------------------------------- avatar_status

fn status_tool() -> McpTool {
    tool(
        "avatar_status",
        "This session's avatar: whether it has one, its name, parts, animation names, the open question, the last \
         lines with what became of them, the limits. With guide true the authoring guide comes first: read it once \
         before avatar_design. With look true the stored design comes back in full (the artwork as SVG). To see the \
         avatar: picture true draws it at rest, pose draws it under inputs, inside an animation or close up on one \
         part, frames draws one of your own animations as a row of moments.",
        json!({ "type": "object", "properties": {
            "guide": { "type": "boolean", "description": "Put the authoring guide before the status: how to draw, rig and animate the avatar, with an example. Read it once, before the first avatar_design." },
            "look": { "type": "boolean", "description": "Include the stored design as an avatar_design call would send it: svg, pivots, parents, roles, animations, states, drives, mouth, gaze, voice, name." },
            "picture": { "type": "boolean", "description": "Add a picture of the avatar at rest: on the light and the dark panel, and under each at its resting size and as a face." },
            "pose": { "type": "object", "description": "Add a picture of the avatar in a pose: {animation?: one of your own, at?: 0 to 1 into it (default 0.5), lookX?, lookY?, turn?: -1 to 1, mouth?, pace?, held?: 0 to 1, zoom?: the id of a part to look at closely}. Shows what your drives, mouth and gaze do. With frames: its inputs are held in every moment." },
            "frames": { "type": "string", "description": "Add a picture of one of your own animations as a row of moments, start to end; with pose, under that pose's inputs (a walk seen in a side view: turn 1, pace 1)." },
            "count": { "type": "integer", "minimum": 2, "maximum": 8, "description": "With frames: how many moments, 2 to 8 (default 6)." }
        } }),
        false,
        |state, ctx, args| async move {
            let pass = enter(&state, &ctx, Call::Status)?;
            let req = StatusRequest::parse(&args, &pass.mask(&state))?;
            let wanted = PictureRequest::parse(&args, &pass.mask(&state))?;
            let status = session::status(&state, &pass, req)?;
            // The guide is for a session that may use its avatar: one that has none reads why instead.
            let text = if req.guide && status.allowed { format!("{}\n## This session's avatar now\n\n{}", brief::guide(Via::Tools, false), status.text()) } else { status.text() };
            let Some(wanted) = wanted else { return Ok(ToolOutput::Text(text)) };
            // What the picture shows comes first: a long status is cut at its end. A picture
            // that cannot be drawn does not take the status (or the guide) with it.
            let caption = wanted.caption();
            match picture(&state, pass.terminal_id(), wanted).await {
                Ok(drawn) => {
                    let cost = session::cost_line(drawn.spent).map(|line| format!("\n{line}")).unwrap_or_default();
                    Ok(ToolOutput::Images { text: format!("{caption}{cost}\n{text}"), png: vec![drawn.png] })
                }
                Err(e) => Ok(ToolOutput::Text(format!("No picture: {}\n{text}", e.message))),
            }
        },
    )
}

// ---------------------------------------------------------------- avatar_design

fn design_tool() -> McpTool {
    tool(
        "avatar_design",
        "Design or change this session's avatar: svg (the artwork, a strict SVG subset whose parts carry ids), pivots \
         and roles (so Workbench's built-in breathing, blinking, gaze and gestures drive your character), parents (a \
         part that moves with another), animations (your own keyframe tracks), states, mouth (lip sync), drives (where \
         it looks, how it walks and turns, moving your parts), voice, name. Not CSS and not SMIL: read the guide first \
         (avatar_status with guide true). A call with svg is a new look; without it the fields you send are merged. A \
         refused call changes nothing and lists what to fix.",
        json!({ "type": "object", "properties": {
            "name": { "type": "string", "description": "The avatar's own name, at most 32 characters." },
            "svg": { "type": "string", "description": "The artwork: one <svg viewBox='0 0 200 200'> of g, path, circle, ellipse, rect, line, polyline, polygon, text and use (href='#id' of a g, a shape or a text, also one in defs); in defs linearGradient, radialGradient, clipPath, mask and filter (feGaussianBlur, feDropShadow, feOffset, feFlood, feColorMatrix, feComposite, feBlend, feMerge, feMorphology, feTurbulence, feDisplacementMap), used with clip-path, mask and filter='url(#id)'; stroke-dasharray and stroke-dashoffset. Attribute values in single quotes, an id on every part that moves. At most 32 KiB and 300 elements, each use counting what it repeats." },
            "pivots": { "type": "object", "description": "Part id to [x, y]: the point the part turns, skews and scales about, in the coordinates it is drawn in. null removes one." },
            "parents": { "type": "object", "description": "Part id to the id of the part it moves with, as if it were drawn inside it, while it keeps its place in the drawing order (a wing behind the body that moves with the body). Not for a part already inside the other; at most 64, chains of 8. Merged by id; null removes one." },
            "roles": { "type": "object", "description": "Which parts are the body, head, eyes, pupils, armL and armR: role to part id. They let the built-in motion drive the character. null removes one." },
            "animations": { "type": "object", "description": "Name to {duration, loop? | every?: [min, max], ease?, tracks: [{target, prop, values, times?, ease?, phase?, delay?}]}. prop: translateX, translateY, rotate, skewX, skewY, scale, scaleX, scaleY, opacity, fillOpacity, strokeOpacity, fill, stroke, strokeWidth, strokeDashoffset, d or a shape's own number. ease: linear, in, out, inOut, inBack, outBack, inOutBack, outElastic, outBounce, step, a bezier [x1, y1, x2, y2], or a list with one per segment. A name of the built-in motion (breathe, blink, talk, nod, wave…) replaces it; walk (loop: true) and greet are played when the avatar strolls and meets another. Merged by name; null removes one." },
            "states": { "type": "object", "description": "State (idle, working, speaking, listening, waiting, attention, error, done, sleeping) to a list of up to 6 animation names, yours or built in. [] is stillness, null the default." },
            "mouth": { "description": "Lip sync: {target, open} morphs a path (open is path data in the structure of the path's own d), {target, prop, open} moves one number (scaleY, scaleX, scale, translateY, ry, r or height). One drive, or a list of up to 4." },
            "gaze": { "type": "number", "description": "How far the part with the pupils role travels when it looks at the pointer, in viewBox units, at most 5% of the width. 0 switches it off." },
            "drives": { "type": "array", "description": "Up to 16 {input, target, prop, values, at?, ease?}: the input moves the prop of the target part (or up to 16) through values placed along its range (at: rising, first 0 and last 1; evenly without it). input: lookX, lookY (-1 to 1, where it looks), mouth (0 to 1 while it speaks), pace (0 standing to 1 walking fast), turn (-1 left to 1 right), held (0 to 1 while dragged). fill, stroke and the opacities follow only pace, turn and held. Replaces the stored list; null or [] removes it." },
            "voice": { "type": "object", "description": "{lang: a tag such as en-GB, prefer: up to 5 pieces of browser voice names, pitch: 0.7 to 1.4, rate: 0.8 to 1.3}." },
            "keep": { "type": "boolean", "description": "With svg: keep the pivots, parents, roles, animations, states, mouth, gaze and drives whose parts still exist, where this call gives none." }
        } }),
        false,
        |state, ctx, args| async move {
            let pass = enter(&state, &ctx, Call::Design)?;
            let answer = design(&state, pass, args).await?;
            let text = answer.text(true);
            Ok(match answer.picture {
                Some(picture) => ToolOutput::Images { text, png: vec![picture.png] },
                None => ToolOutput::Text(text),
            })
        },
    )
    .acts()
}

// ---------------------------------------------------------------- avatar_say

fn say_tool() -> McpTool {
    tool(
        "avatar_say",
        "Say one line to the user through this session's avatar: spoken if they turned the voice on, always a caption. \
         One or two short plain sentences, at most 400 characters; no Markdown, code, paths or URLs: detail belongs in \
         the terminal. It does not wait: send it in the same step as your other tool calls, one line per step. The \
         answer tells what became of your earlier lines; when nobody listens, stop. gesture: one of your animations. \
         lang: a BCP 47 tag for a line in another language.",
        json!({ "type": "object", "properties": {
            "text": { "type": "string", "description": "The line: one or two short plain spoken sentences, at most 400 characters." },
            "gesture": { "type": "string", "description": "An animation to play with the line: one of yours or a built-in one (avatar_status lists both)." },
            "lang": { "type": "string", "description": "A BCP 47 tag such as de or pt-BR, for a line in another language than the avatar's voice." },
            "wait": { "type": "boolean", "description": "Return only when the line was spoken, skipped or shown, and say which. Default false: the line is queued and the call returns at once." }
        }, "required": ["text"] }),
        false,
        |state, ctx, args| async move {
            let pass = enter(&state, &ctx, Call::Say)?;
            let req = SayRequest::parse(&args, &pass.mask(&state))?;
            Ok(ToolOutput::Text(session::say_call(&state, &pass, req, crate::util::now_ms()).await?.note))
        },
    )
    .acts()
    .quiet()
}

// ---------------------------------------------------------------- avatar_ask

fn ask_tool() -> McpTool {
    tool(
        "avatar_ask",
        "Ask the user one short question through the avatar and wait up to waitSeconds (default 45, 0 to 50): up to 4 \
         choices as buttons and, unless freeText is false, a typed or dictated answer. Returns answered true with text \
         and choice, or answered false with a reason and askId. After a timeout the question stays on screen: do not \
         ask again; the answer arrives as a user message that starts with [via avatar…]. askId waits again on the open \
         question; cancel true withdraws it. Never for permission to run a tool.",
        json!({ "type": "object", "properties": {
            "text": { "type": "string", "description": "The question: one short plain sentence, at most 300 characters." },
            "choices": { "type": "array", "items": { "type": "string" }, "maxItems": 4, "description": "Up to 4 answers shown as buttons, each at most 40 characters." },
            "freeText": { "type": "boolean", "description": "Default true: the user may also type or dictate an answer. false: only the choices." },
            "waitSeconds": { "type": "number", "description": "How long this call waits for the answer: 0 to 50, default 45. 0 asks and returns at once." },
            "askId": { "type": "string", "description": "Instead of text: wait again for the answer to the open question with this id." },
            "cancel": { "type": "boolean", "description": "Instead of text: true withdraws the open question." },
            "gesture": { "type": "string", "description": "An animation to play with the question." },
            "lang": { "type": "string", "description": "A BCP 47 tag for a question in another language than the avatar's voice." }
        } }),
        false,
        |state, ctx, args| async move {
            let pass = enter(&state, &ctx, Call::Ask)?;
            let args = AskArgs::parse(&args, &pass.mask(&state))?;
            let answer = session::ask_call(&state, &pass, args, MAX_WAIT_TOOL_SECS, crate::util::now_ms()).await?;
            // As text, so the fields keep their order: `answered` first.
            Ok(ToolOutput::Text(serde_json::to_string_pretty(&answer)?))
        },
    )
    .acts()
}

// ---------------------------------------------------------------- avatar_act

fn act_tool() -> McpTool {
    tool(
        "avatar_act",
        "Play one of the avatar's animations as a gesture (play: a name from avatar_status; mode hold keeps the last \
         frame, loop repeats a looping animation; both end at stop, at the next play or after two minutes), or stop one \
         (stop: a name or all).",
        json!({ "type": "object", "properties": {
            "play": { "type": "string", "description": "The animation to play: one of yours or a built-in one (avatar_status lists both)." },
            "mode": { "type": "string", "enum": ["once", "hold", "loop"], "description": "once (default), hold (keep the last frame) or loop (only an animation declared loop: true)." },
            "times": { "type": "integer", "minimum": 1, "maximum": 5, "description": "With mode once: how often in a row, 1 to 5." },
            "stop": { "type": "string", "description": "Instead of play: the name of a held or looping gesture to stop, or all." }
        } }),
        false,
        |state, ctx, args| async move {
            let pass = enter(&state, &ctx, Call::Act)?;
            let req = ActRequest::parse(&args, &pass.mask(&state))?;
            Ok(ToolOutput::Text(session::act(&state, &pass, req, crate::util::now_ms())?.note))
        },
    )
    .acts()
    .quiet()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two at a time; a turn is given back when its work ends, and at once when its caller
    /// leaves, since the work ends with it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_drawings_at_a_time_and_a_caller_that_leaves_frees_its_turn() {
        let turns = Arc::new(Semaphore::new(2));
        let (began, mut has_begun) = tokio::sync::mpsc::unbounded_channel::<u8>();
        let mut callers = Vec::new();
        for n in 0..2u8 {
            let (turns, began) = (turns.clone(), began.clone());
            callers.push(tokio::spawn(async move {
                in_turn(&turns, Duration::from_secs(5), async move {
                    began.send(n).unwrap();
                    std::future::pending::<()>().await;
                })
                .await
            }));
        }
        let mut running = vec![has_begun.recv().await.unwrap(), has_begun.recv().await.unwrap()];
        running.sort();
        assert_eq!((running, turns.available_permits()), (vec![0, 1], 0));
        // A third that will not wait long is told to ask again; its work never began.
        let refused = in_turn(&turns, Duration::from_millis(100), async { 9u8 }).await.unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.code), (429, "rate_limited"));
        assert!(refused.message.contains("ask for this one again"), "{}", refused.message);
        // One caller gives up: its work is dropped with it, and the next gets the turn.
        callers[0].abort();
        assert!(callers.remove(0).await.unwrap_err().is_cancelled());
        assert_eq!(in_turn(&turns, Duration::from_secs(5), async { 9u8 }).await.unwrap(), 9);
        callers[0].abort();
        let _ = callers.remove(0).await;
        assert_eq!(turns.available_permits(), 2);
    }

    #[test]
    fn a_session_draws_one_picture_at_a_time_and_rests_after_a_costly_one() {
        let now = Instant::now();
        let first = OwnTurn::take("tools-test-a", true, now).unwrap();
        let second = OwnTurn::take("tools-test-a", true, now).err().expect("one at a time");
        assert_eq!((second.status.as_u16(), second.message.as_str()), (429, "A picture of this avatar is being drawn: wait for it, then ask for the next."));
        // Another session is not kept waiting by this one.
        drop(OwnTurn::take("tools-test-b", true, now).unwrap());
        drop(first);
        drop(OwnTurn::take("tools-test-a", true, now).unwrap());
        // After a drawing that died of its look, the session's pictures pause; its design calls do not.
        SESSIONS.lock().resting.insert("tools-test-a".to_string(), now + AFTER_A_COSTLY_ONE);
        let resting = OwnTurn::take("tools-test-a", true, now + Duration::from_secs(1)).err().expect("a pause");
        assert!(resting.message.contains("ask again in half a minute"), "{}", resting.message);
        drop(OwnTurn::take("tools-test-a", false, now + Duration::from_secs(1)).unwrap());
        drop(OwnTurn::take("tools-test-a", true, now + AFTER_A_COSTLY_ONE + Duration::from_secs(1)).unwrap());
        assert!(!SESSIONS.lock().resting.contains_key("tools-test-a"));
    }
}

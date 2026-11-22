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
//! is asked for (`ToolOutput::Images`): a session cannot see the browser. Pictures are drawn on a
//! blocking thread.
//!
//! None of them changes remote state, so none is `mutating`. `avatar_status` only reads; the
//! other four do something for the user to see (`McpTool::acts`), and the two an agent calls
//! all the time, `avatar_say` and `avatar_act`, stay out of the Activity view when they succeed
//! (`McpTool::quiet`).
//!
//! `avatar_ask` waits inside the call, at most `MAX_WAIT_TOOL_SECS`: an MCP client gives a call
//! about a minute. When the client goes away the platform aborts the call's task, and the
//! waiter's own `Drop` leaves the question open (or pastes an answer it already holds).

use serde_json::{Value, json};

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

/// A design is validated and stored on a thread of its own: reading artwork takes time in
/// proportion to its size, and the file is written before the answer.
pub(super) async fn design(state: &AppState, pass: Pass, args: Value) -> ApiResult<session::Designed> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || session::design(&state, &pass, &args, crate::util::now_ms())).await.map_err(|_| ApiError::internal("the design call did not finish"))?
}

/// Pictures drawn at one time, for all sessions together: drawing takes a core, and a session's
/// own rate says nothing about how many sessions ask at once. The others wait their turn.
static DRAWING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// A picture of the session's avatar, drawn on a thread of its own.
pub(super) async fn picture(state: &AppState, tid: &str, req: PictureRequest) -> ApiResult<Vec<u8>> {
    let (state, tid) = (state.clone(), tid.to_string());
    let turn = DRAWING.acquire().await.map_err(|_| ApiError::internal("the picture was not drawn"))?;
    let drawn = tokio::task::spawn_blocking(move || session::picture(&state, &tid, &req)).await.map_err(|_| ApiError::internal("the picture was not drawn"));
    drop(turn);
    drawn?
}

/// The picture that goes with a design answer: the look at rest. A session that drew nothing of
/// its own gets none; a look that cannot be drawn is still saved, and the answer says why there
/// is no picture.
async fn designed_picture(state: &AppState, tid: &str, text: String) -> ToolOutput {
    let rest = PictureRequest::rest();
    let caption = rest.caption();
    match picture(state, tid, rest).await {
        Ok(png) => ToolOutput::Images { text: format!("{text}\n{caption} Look at it and correct what is off."), png: vec![png] },
        Err(e) if e.message == session::NO_LOOK_TO_DRAW => ToolOutput::Text(text),
        Err(e) => ToolOutput::Text(format!("{text}\nNo picture: {}", e.message)),
    }
}

// ---------------------------------------------------------------- avatar_status

fn status_tool() -> McpTool {
    tool(
        "avatar_status",
        "This session's avatar: whether it has one, its name, parts, animation names, the open question, the last \
         lines with what became of them, the limits. With guide true the authoring guide comes first: read it once \
         before avatar_design. With look true the stored design comes back in full (the artwork as SVG). To see the \
         avatar: picture true draws it at rest, pose draws it under inputs or inside an animation, frames draws one \
         of your own animations as a row of moments.",
        json!({ "type": "object", "properties": {
            "guide": { "type": "boolean", "description": "Put the authoring guide before the status: how to draw, rig and animate the avatar, with an example. Read it once, before the first avatar_design." },
            "look": { "type": "boolean", "description": "Include the stored design as an avatar_design call would send it: svg, pivots, parents, roles, animations, states, drives, mouth, gaze, voice, name." },
            "picture": { "type": "boolean", "description": "Add a picture of the avatar at rest: on the light and the dark panel, and under each at its resting size and as a face." },
            "pose": { "type": "object", "description": "Add a picture of the avatar in a pose: {animation?: one of your own, at?: 0 to 1 into it (default 0.5), lookX?, lookY?, turn?: -1 to 1, mouth?, pace?, held?: 0 to 1}. Shows what your drives, mouth and gaze do." },
            "frames": { "type": "string", "description": "Add a picture of one of your own animations as a row of moments, start to end." },
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
            // A picture that cannot be drawn is the answer: the session asked to see something.
            let caption = wanted.caption();
            let png = picture(&state, pass.terminal_id(), wanted).await?;
            Ok(ToolOutput::Images { text: format!("{text}\n{caption}"), png: vec![png] })
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
            "animations": { "type": "object", "description": "Name to {duration, loop? | every?: [min, max], ease?, tracks: [{target, prop, values, times?, ease?, phase?, delay?}]}. prop: translateX, translateY, rotate, skewX, skewY, scale, scaleX, scaleY, opacity, fillOpacity, strokeOpacity, fill, stroke, strokeWidth, strokeDashoffset, d or a shape's own number. ease: linear, in, out, inOut, inBack, outBack, inOutBack, outElastic, outBounce, step, a bezier [x1, y1, x2, y2], or a list with one per segment. A name of the built-in motion (wave, walk, greet…) replaces it. Merged by name; null removes one." },
            "states": { "type": "object", "description": "State (idle, working, speaking, listening, waiting, attention, error, done, sleeping) to a list of up to 6 animation names, yours or built in. [] is stillness, null the default." },
            "mouth": { "description": "Lip sync: {target, open} morphs a path (open is path data in the structure of the path's own d), {target, prop, open} moves one number (scaleY, scaleX, scale, translateY, ry, r or height). One drive, or a list of up to 4." },
            "gaze": { "type": "number", "description": "How far the part with the pupils role travels when it looks at the pointer, in viewBox units, at most 5% of the width. 0 switches it off." },
            "drives": { "type": "array", "description": "Up to 16 {input, target, prop, values, at?, ease?}: the input moves the prop of the target part (or up to 16) through values placed along its range (at: 0 to 1, rising; evenly without it). input: lookX, lookY (-1 to 1, where it looks), mouth (0 to 1 while it speaks), pace (0 standing to 1 walking fast), turn (-1 left to 1 right), held (0 to 1 while dragged). fill, stroke and the opacities follow only pace, turn and held. Replaces the stored list; null or [] removes it." },
            "voice": { "type": "object", "description": "{lang: a tag such as en-GB, prefer: up to 5 pieces of browser voice names, pitch: 0.7 to 1.4, rate: 0.8 to 1.3}." },
            "keep": { "type": "boolean", "description": "With svg: keep the pivots, parents, roles, animations, states, mouth, gaze and drives whose parts still exist, where this call gives none." }
        } }),
        false,
        |state, ctx, args| async move {
            let pass = enter(&state, &ctx, Call::Design)?;
            let tid = pass.terminal_id().to_string();
            let text = design(&state, pass, args).await?.text;
            Ok(designed_picture(&state, &tid, text).await)
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

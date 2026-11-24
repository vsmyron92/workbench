//! The words a session gets back from its calls: the line beside a picture, what a design answer
//! says the stored design holds, the warnings of a design check (a role arm without a pivot, a
//! drive that changes the figure at rest), the order of the parts, and a refusal that says what
//! an argument takes.
//!
//! The environment is the state tests' (`super::tests`): a real `AppState` on temporary
//! directories with stand-in agent sessions. A tool call goes through the tool's handler, as in
//! `super::tests_http`; the validator's warnings are read from `look::apply` itself.

use serde_json::{Value, json};

use super::Via;
use super::look;
use super::session::{self, ActRequest, BRIEF_DEFAULT, BRIEF_MORE, BRIEF_UNNAMED, Call, STOP_TAKES, STORED_CHARS, STORED_NOTHING_ELSE, StatusRequest, gate, status, unmasked};
use super::tests::{Env, T0, env, figure, play};
use crate::error::ApiResult;
use crate::mcp::{McpCtx, ToolOutput};

// ---------------------------------------------------------------- helpers

/// A tool call of the session `tid`, through the tool's handler: what the model reads.
async fn call(e: &Env, tid: &str, name: &str, args: Value) -> ApiResult<String> {
    let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"));
    let ctx = McpCtx { terminal_id: Some(tid.to_string()), project_id: Some(e.pid.clone()) };
    (tool.handler)(e.state.clone(), ctx, args).await.map(|out| match out {
        ToolOutput::Text(text) => text,
        ToolOutput::Json(value) => value.to_string(),
        ToolOutput::Images { text, .. } => text,
    })
}

/// The line a session reads beside a picture.
fn brief_of(e: &Env, tid: &str, now: i64) -> String {
    let pass = gate(&e.state, tid, Call::Status, Via::Tools, now).unwrap();
    status(&e.state, &pass, StatusRequest::default()).unwrap().brief()
}

/// The warnings of a design applied to nothing.
fn warnings(design: &Value) -> Vec<String> {
    look::apply(None, design).unwrap_or_else(|problems| panic!("refused:\n{}", look::message(&problems))).warnings
}

/// `base` with the keys of `extra` set.
fn with(mut base: Value, extra: Value) -> Value {
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// The line of a design answer that says what else is stored.
fn stored_line(answer: &str) -> &str {
    answer.lines().find(|line| line.starts_with("Also stored: ")).unwrap_or_else(|| panic!("no line of what is stored:\n{answer}"))
}

// ---------------------------------------------------------------- the line beside a picture

#[tokio::test]
async fn the_line_beside_a_picture_names_an_avatar_without_a_name_and_says_what_to_send() {
    let e = env().await;
    assert_eq!((BRIEF_UNNAMED, BRIEF_DEFAULT), ("Your avatar (no name yet)", " It shows Workbench's default look: send svg with a design call to draw your own."));

    // A new avatar has no name and the default look: no parts to read of, a design to send.
    let tid = e.with_avatar();
    assert_eq!(brief_of(&e, &tid, T0), "Your avatar (no name yet), revision 0. It shows Workbench's default look: send svg with a design call to draw your own.");
    // A name alone keeps the default look.
    e.design(&tid, json!({ "name": "Dot" }), T0 + 1).unwrap();
    assert_eq!(brief_of(&e, &tid, T0 + 2), format!("Dot, revision 1.{BRIEF_DEFAULT}"));

    // A look of its own without a name (a name is optional): the rest of the status is pointed at.
    let unnamed = e.with_avatar();
    let mut design = figure();
    design.as_object_mut().unwrap().remove("name");
    assert!(e.design(&unnamed, design, T0).unwrap().text.starts_with("Saved as revision 1, no name yet."));
    assert_eq!(brief_of(&e, &unnamed, T0 + 1), format!("Your avatar (no name yet), revision 1.{BRIEF_MORE}"));
    // What stands open is said between the two, as before.
    e.act(&unnamed, ActRequest { mode: Some(session::ActMode::Hold), ..play("hop") }, T0 + 2).unwrap();
    assert_eq!(brief_of(&e, &unnamed, T0 + 3), format!("Your avatar (no name yet), revision 1. hop is held.{BRIEF_MORE}"));
    e.design(&unnamed, json!({ "name": "Pip" }), T0 + 4).unwrap();
    assert_eq!(brief_of(&e, &unnamed, T0 + 5), format!("Pip, revision 2. hop is held.{BRIEF_MORE}"));

    // As a session reads it: a new avatar's first picture request.
    let fresh = e.with_avatar();
    let said = call(&e, &fresh, "avatar_status", json!({ "picture": true })).await.unwrap();
    assert_eq!(said, format!("No picture: {}\nYour avatar (no name yet), revision 0.{BRIEF_DEFAULT}", session::NO_LOOK_TO_DRAW));
}

// ---------------------------------------------------------------- avatar_act's stop

#[tokio::test]
async fn a_stop_that_is_not_a_name_is_told_what_stop_takes() {
    assert_eq!(STOP_TAKES, "write the name of a held or looping gesture, or all");
    for (stop, kind) in [(json!(true), "true or false"), (json!(1), "a number"), (json!(["wave"]), "a list"), (json!({ "name": "wave" }), "an object")] {
        let refused = ActRequest::parse(&json!({ "stop": stop }), &unmasked).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.message), (400, format!("stop is {kind}: {STOP_TAKES}")));
    }
    // A name, all, null and nothing read as before; play keeps its own refusal.
    assert_eq!(ActRequest::parse(&json!({ "stop": "all" }), &unmasked).unwrap().stop.as_deref(), Some("all"));
    assert_eq!(ActRequest::parse(&json!({ "stop": "wave" }), &unmasked).unwrap().stop.as_deref(), Some("wave"));
    assert_eq!(ActRequest::parse(&json!({ "play": "hop", "stop": null }), &unmasked).unwrap().stop, None);
    assert_eq!(ActRequest::parse(&json!({ "play": true }), &unmasked).unwrap_err().message, "play is true or false: write a string");

    // As a session reads it.
    let e = env().await;
    let tid = e.with_avatar();
    let refused = call(&e, &tid, "avatar_act", json!({ "stop": true })).await.unwrap_err();
    assert_eq!(refused.message, "stop is true or false: write the name of a held or looping gesture, or all");
}

// ---------------------------------------------------------------- a role arm without a pivot

/// A body with two arms beside it, the arms named as roles and given no pivot.
fn arms() -> Value {
    json!({
        "svg": "<svg viewBox='0 0 200 200'><g id='body'><rect x='70' y='60' width='60' height='100' fill='#3366cc'/><g id='armL'><rect x='50' y='70' width='16' height='50' fill='#3366cc'/></g><g id='armR'><rect x='134' y='70' width='16' height='50' fill='#3366cc'/></g></g></svg>",
        "pivots": { "body": [100, 160] },
        "roles": { "body": "body", "armL": "armL", "armR": "armR" }
    })
}

#[test]
fn a_role_arm_without_a_pivot_warns_of_what_the_built_in_wave_and_cheer_do_with_it() {
    let warned = |role: &str, which: &str| format!("{role} has no pivot: built-in {which} can only wiggle it (at roles.{role}): give pivots.{role} its shoulder, in the coordinates it is drawn in");
    // Wave moves armR, cheer both: each arm is warned of once, with the built-ins that move it.
    assert_eq!(warnings(&arms()), [warned("armR", "wave and cheer"), warned("armL", "cheer")]);
    assert!(warnings(&arms()).iter().all(|w| w.chars().count() <= look::MAX_PROBLEM_CHARS));
    // Its shoulders as pivots: nothing to say.
    assert_eq!(warnings(&with(arms(), json!({ "pivots": { "body": [100, 160], "armL": [58, 72], "armR": [142, 72] } }))), [""; 0]);
    let one = warnings(&with(arms(), json!({ "pivots": { "body": [100, 160], "armR": [142, 72] } })));
    assert_eq!(one, [warned("armL", "cheer")]);
    // Without armR, armL waves; without the body and the other arm, nothing cheers.
    assert_eq!(warnings(&with(arms(), json!({ "roles": { "body": "body", "armL": "armL" } }))), [warned("armL", "wave and cheer")]);
    assert_eq!(warnings(&with(arms(), json!({ "roles": { "armR": "armR" } }))), [warned("armR", "wave")]);
    // A built-in the look replaces with its own animation moves no arm.
    let own = |names: &[&str]| -> Value { names.iter().map(|name| (name.to_string(), json!({ "duration": 1, "tracks": [{ "target": "body", "prop": "translateY", "values": [0, -4, 0] }] }))).collect::<serde_json::Map<String, Value>>().into() };
    assert_eq!(warnings(&with(arms(), json!({ "animations": own(&["wave"]) }))), [warned("armR", "cheer"), warned("armL", "cheer")]);
    assert_eq!(warnings(&with(arms(), json!({ "animations": own(&["wave", "cheer"]) }))), [""; 0]);

    // A merge that gives the pivots clears the warning; one that leaves them out says it again.
    let first = look::apply(None, &arms()).unwrap();
    let merged = look::apply(Some(&first.design), &json!({ "pivots": { "armL": [58, 72], "armR": [142, 72] } })).unwrap();
    assert_eq!(merged.warnings, [""; 0]);
    let again = look::apply(Some(&first.design), &json!({ "states": { "idle": ["breathe"] } })).unwrap();
    assert_eq!(again.warnings, [warned("armR", "wave and cheer"), warned("armL", "cheer")]);
}

// ---------------------------------------------------------------- a drive that changes the figure at rest

/// A body with a head, and a front and a side view of the torso.
fn viewed(drives: Value) -> Value {
    json!({
        "svg": "<svg viewBox='0 0 200 200'><g id='body'><circle id='head' cx='100' cy='70' r='30' fill='#ffcc00'/><rect id='front' x='80' y='105' width='40' height='60' fill='#3366cc'/><rect id='side' x='90' y='105' width='20' height='60' fill='#3366cc' opacity='0.8'/></g></svg>",
        "pivots": { "body": [100, 165] },
        "drives": drives
    })
}

/// The warnings of one drive.
fn of_drive(drive: Value) -> Vec<String> {
    warnings(&viewed(json!([drive])))
}

const MIDDLE: &str = "an odd number of values with the rest in the middle, or place them with at";

#[test]
fn a_drive_that_changes_the_figure_at_rest_is_warned_of() {
    // Two opacities along turn: at rest, facing the user, the head is half there.
    let half = of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [1, 0] }));
    assert_eq!(half, [format!("turn gives head opacity 0.5 at rest, not the artwork's 1 (at drives[0]): {MIDDLE}")]);
    assert!(half[0].chars().count() <= look::MAX_PROBLEM_CHARS, "{}", half[0]);
    // It is stored all the same, and a stored look with it loads.
    let applied = look::apply(None, &viewed(json!([{ "input": "turn", "target": "head", "prop": "opacity", "values": [1, 0] }]))).unwrap();
    assert!(look::check_stored(json!({ "look": applied.design.look })).is_some());
    // The artwork's own value is the rest: an opacity the artwork gives, and null for it.
    assert_eq!(of_drive(json!({ "input": "turn", "target": "side", "prop": "opacity", "values": [1, 1] })), [format!("turn gives side opacity 1 at rest, not the artwork's 0.8 (at drives[0]): {MIDDLE}")]);
    assert_eq!(of_drive(json!({ "input": "turn", "target": "side", "prop": "opacity", "values": [1, null, 1] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [0.4, 1, 0.4] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [1, 1, 0.35], "at": [0, 0.6, 1] })), [""; 0]);
    // A keyframe written at the rest may be meant: the cure says which one to change.
    assert_eq!(
        of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [1, 0.4, 1] })),
        ["turn gives head opacity 0.4 at rest, not the artwork's 1 (at drives[0]): make values[1] null, the artwork's own, unless that is meant"]
    );
    // A few hundredths off the rest (within a tenth of the swing) is not seen, and not warned of.
    assert_eq!(of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [0.4, 1, 0.4], "at": [0, 0.45, 1] })), [""; 0]);
    // Further off it is: the peak placed before the rest, and eased.
    assert_eq!(
        of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [0.4, 1, 0.4], "at": [0, 0.4, 1], "ease": [0.25, 0.1, 0.25, 1] })),
        [format!("turn gives head opacity 0.868 at rest, not the artwork's 1 (at drives[0]): {MIDDLE}")]
    );
    // A colour between two along turn, and one at the start of pace.
    assert_eq!(of_drive(json!({ "input": "turn", "target": "head", "prop": "fill", "values": ["#ff0000", "#00ff00"] })), [format!("turn gives head fill a blend at rest, not the artwork's #ffcc00 (at drives[0]): {MIDDLE}")]);
    assert_eq!(
        of_drive(json!({ "input": "pace", "target": "head", "prop": "fill", "values": ["red", "#00ff00"] })),
        ["pace gives head fill #ff0000 at rest, not the artwork's #ffcc00 (at drives[0]): make values[0] null, the artwork's own, unless that is meant"]
    );
    assert_eq!(of_drive(json!({ "input": "pace", "target": "head", "prop": "fill", "values": ["#FFCC00", "#00ff00"] })), [""; 0]);

    // Relative props rest at 0, scales at 1, whatever the ease does in between.
    assert_eq!(of_drive(json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [-8, 8] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [-8, 8], "ease": "inOut" })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [-8, 8], "ease": [0.42, 0, 0.58, 1] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [-8, 8], "ease": "in" })), [format!("lookX gives head rotate -4 at rest, not 0 (at drives[0]): {MIDDLE}")]);
    assert_eq!(of_drive(json!({ "input": "lookY", "target": ["head", "front"], "prop": "translateY", "values": [2, 4] })), [format!("lookY gives head, front translateY 3 at rest, not 0 (at drives[0]): {MIDDLE}")]);
    assert_eq!(of_drive(json!({ "input": "pace", "target": "head", "prop": "rotate", "values": [5, 0] })), ["pace gives head rotate 5 at rest, not 0 (at drives[0]): make values[0] 0, unless that is meant"]);
    assert_eq!(of_drive(json!({ "input": "held", "target": "head", "prop": "scaleY", "values": [1.2, 1] })), ["held gives head scaleY 1.2 at rest, not 1 (at drives[0]): make values[0] 1, unless that is meant"]);
    assert_eq!(of_drive(json!({ "input": "mouth", "target": "head", "prop": "scaleY", "values": [1, 1.2] })), [""; 0]);
    // A flip between -1 and 1 folds the part away at rest, which no keyframe asked for.
    assert_eq!(of_drive(json!({ "input": "turn", "target": "front", "prop": "scaleX", "values": [-1, 1] })), [format!("turn gives front scaleX 0 at rest, not 1 (at drives[0]): {MIDDLE}")]);

    // The guide's side view: the front view rests at 1, the side view is folded away at rest by
    // a keyframe written there, as meant. A part hidden at rest by an opacity written there too.
    let side_view = json!([
        { "input": "turn", "target": "side", "prop": "scaleX", "values": [-1, -1, 0, 1, 1], "at": [0, 0.4, 0.5, 0.6, 1] },
        { "input": "turn", "target": "front", "prop": "scaleX", "values": [0, 0, 1, 0, 0], "at": [0, 0.4, 0.5, 0.6, 1] }
    ]);
    assert_eq!(warnings(&viewed(side_view)), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "turn", "target": "side", "prop": "scaleX", "values": [-1, 0, 1] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "turn", "target": "side", "prop": "opacity", "values": [1, 0, 1] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "pace", "target": "side", "prop": "opacity", "values": [0, 1] })), [""; 0]);

    // The drives that do are one warning: the first, the others by their place, and one cure.
    let two = warnings(&viewed(json!([
        { "input": "lookX", "target": "head", "prop": "rotate", "values": [-6, 0, 6] },
        { "input": "turn", "target": "head", "prop": "opacity", "values": [1, 0] },
        { "input": "pace", "target": "front", "prop": "rotate", "values": [3, 0] }
    ])));
    assert_eq!(two, ["turn gives head opacity 0.5 at rest, not the artwork's 1 (at drives[1]; also drives[2]): make each give its prop's rest at its input's rest"]);
}

/// A face inside a body group that strokes it: `group` is the group's attributes.
fn stroked(group: &str, drive: Value) -> Vec<String> {
    warnings(&json!({
        "svg": format!("<svg viewBox='0 0 200 200'><g id='body' {group}><circle id='face' cx='100' cy='100' r='60' fill='#ff0000'/></g></svg>"),
        "drives": [drive]
    }))
}

#[test]
fn a_drive_is_measured_against_what_its_part_inherits() {
    let group = "stroke='#000000' stroke-width='3' fill-opacity='0.5'";
    // The face inherits its stroke, its width and its fill's opacity from the body: those are its
    // rest, and the cure writes them out (null is a part's own value, which it has none of).
    assert_eq!(
        stroked(group, json!({ "input": "pace", "target": "face", "prop": "strokeWidth", "values": [12, 3] })),
        ["pace gives face strokeWidth 12 at rest, not the artwork's 3 (at drives[0]): make values[0] 3, unless that is meant"]
    );
    assert_eq!(
        stroked(group, json!({ "input": "pace", "target": "face", "prop": "stroke", "values": ["#00ff00", "#000000"] })),
        ["pace gives face stroke #00ff00 at rest, not the artwork's #000000 (at drives[0]): make values[0] #000000, unless that is meant"]
    );
    assert_eq!(
        stroked(group, json!({ "input": "turn", "target": "face", "prop": "fillOpacity", "values": [0.5, 1, 0.5] })),
        ["turn gives face fillOpacity 1 at rest, not the artwork's 0.5 (at drives[0]): make values[1] 0.5, unless that is meant"]
    );
    assert_eq!(stroked(group, json!({ "input": "pace", "target": "face", "prop": "strokeWidth", "values": [3, 12] })), [""; 0]);
    assert_eq!(stroked(group, json!({ "input": "turn", "target": "face", "prop": "fillOpacity", "values": [1, 0.5, 1] })), [""; 0]);
    // From a group without an id too, and the body's own value is its own.
    let plain = warnings(&json!({
        "svg": "<svg viewBox='0 0 200 200'><g stroke='#000000' stroke-width='3'><circle id='face' cx='100' cy='100' r='60' fill='#ff0000'/></g></svg>",
        "drives": [{ "input": "pace", "target": "face", "prop": "strokeWidth", "values": [12, 3] }]
    }));
    assert_eq!(plain, ["pace gives face strokeWidth 12 at rest, not the artwork's 3 (at drives[0]): make values[0] 3, unless that is meant"]);
    assert_eq!(
        stroked(group, json!({ "input": "pace", "target": "body", "prop": "strokeWidth", "values": [12, 3] })),
        ["pace gives body strokeWidth 12 at rest, not the artwork's 3 (at drives[0]): make values[0] null, the artwork's own, unless that is meant"]
    );
    // What nothing sets is the initial value: a width of 1, a black fill, no stroke, which no
    // keyframe can write. A stroke's width shows only where a stroke is painted.
    assert_eq!(
        stroked("stroke='#000000'", json!({ "input": "pace", "target": "body", "prop": "strokeWidth", "values": [4, 1] })),
        ["pace gives body strokeWidth 4 at rest, not the artwork's 1 (at drives[0]): make values[0] 1, unless that is meant"]
    );
    assert_eq!(stroked("", json!({ "input": "pace", "target": "body", "prop": "strokeWidth", "values": [4, 1] })), [""; 0]);
    assert_eq!(stroked("", json!({ "input": "pace", "target": "face", "prop": "strokeWidth", "values": [4, 1] })), [""; 0]);
    assert_eq!(stroked("", json!({ "input": "pace", "target": "face", "prop": "fill", "values": ["#ff0000", "#00ff00"] })), [""; 0]);
    assert_eq!(
        stroked("", json!({ "input": "pace", "target": "face", "prop": "stroke", "values": ["#00ff00", "#000000"] })),
        ["pace gives face stroke #00ff00 at rest, not the artwork's none (at drives[0]): give it a stroke at stroke-opacity 0 and drive strokeOpacity instead"]
    );
    // A group's paint shows only on what takes it from the group: the face sets its own fill.
    assert_eq!(stroked("", json!({ "input": "pace", "target": "body", "prop": "fill", "values": ["#00ff00", "#ff0000"] })), [""; 0]);
    assert_eq!(stroked("", json!({ "input": "pace", "target": "body", "prop": "stroke", "values": ["#00ff00", "#000000"] })).len(), 1);
    // A line has no fill to show, nor a group whose only part is a line.
    let line = warnings(&json!({
        "svg": "<svg viewBox='0 0 200 200'><g id='arm' stroke='#000000' stroke-width='4'><line id='bone' x1='60' y1='100' x2='140' y2='100'/></g></svg>",
        "drives": [{ "input": "pace", "target": "bone", "prop": "fill", "values": ["#ff0000", "#000000"] }, { "input": "held", "target": "arm", "prop": "fill", "values": ["#ff0000", "#000000"] }]
    }));
    assert_eq!(line, [""; 0]);
    // A paint that is not plain is not compared: a gradient, currentColor.
    assert_eq!(stroked("stroke='currentColor'", json!({ "input": "pace", "target": "face", "prop": "stroke", "values": ["#00ff00", "#000000"] })), [""; 0]);
    let gradient = warnings(&json!({
        "svg": "<svg viewBox='0 0 200 200'><defs><linearGradient id='g'><stop offset='0' stop-color='#ff0000'/><stop offset='1' stop-color='#0000ff'/></linearGradient></defs><g id='body' fill='url(#g)'><circle id='face' cx='100' cy='100' r='60'/></g></svg>",
        "drives": [{ "input": "pace", "target": "face", "prop": "fill", "values": ["#00ff00", "#000000"] }]
    }));
    assert_eq!(gradient, [""; 0]);
}

#[test]
fn a_drive_warns_of_what_is_seen_at_rest_however_far_it_swings() {
    // A large swing does not hide what is plainly seen at rest: a trunk at 0.93 of its size, a
    // part turned by eight degrees.
    assert_eq!(
        of_drive(json!({ "input": "turn", "target": "head", "prop": "scale", "values": [1.25, 0.27, 0.93, 1.39, 1] })),
        ["turn gives head scale 0.93 at rest, not 1 (at drives[0]): make values[2] 1, unless that is meant"]
    );
    assert_eq!(
        of_drive(json!({ "input": "turn", "target": "head", "prop": "rotate", "values": [-90, 8, 90] })),
        ["turn gives head rotate 8 at rest, not 0 (at drives[0]): make values[1] 0, unless that is meant"]
    );
    assert_eq!(of_drive(json!({ "input": "pace", "target": "head", "prop": "translateX", "values": [3, 40] })).len(), 1);
    // A tiny swing does not make what nobody can see a warning: half a hundredth of a scale, a
    // tenth of a degree, a fifth of a unit of a 200 unit viewBox.
    assert_eq!(of_drive(json!({ "input": "lookY", "target": "head", "prop": "scaleX", "values": [0.99, 1] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "lookY", "target": "head", "prop": "rotate", "values": [0.2, 0] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "lookX", "target": "head", "prop": "translateX", "values": [0, 0.4] })), [""; 0]);
    assert_eq!(of_drive(json!({ "input": "turn", "target": "head", "prop": "opacity", "values": [0.97, 1] })), [""; 0]);
    // A length is seen through what scales the part at rest: one unit of a part drawn at a
    // quarter of its size is a quarter of a unit, and a folded part shows nothing.
    let warned = |transform: &str, prop: &str| {
        let all = warnings(&json!({
            "svg": format!("<svg viewBox='0 0 200 200'><g transform='{transform}'><circle id='dot' cx='25' cy='25' r='10' fill='#ff0000'/></g></svg>"),
            "drives": [{ "input": "lookX", "target": "dot", "prop": prop, "values": [0, 2] }]
        }));
        all.iter().any(|w| w.starts_with(&format!("lookX gives dot {prop} 1 at rest, not 0")))
    };
    assert!(warned("scale(1)", "translateX") && warned("scale(4)", "translateX"));
    assert!(!warned("scale(0.25)", "translateX") && !warned("scale(0.25 4)", "translateX") && warned("scale(0.25 4)", "translateY"));
}

#[test]
fn the_drives_that_change_the_figure_at_rest_are_one_warning_and_leave_room_for_the_others() {
    let parts: String = (0..16).map(|i| format!("<circle id='p{i}' cx='{}' cy='50' r='2' fill='#000000'/>", 5 + i * 5)).collect();
    let svg = format!(
        "<svg viewBox='0 0 100 100'><g id='body'>{parts}<rect id='armL' x='10' y='60' width='5' height='20' fill='#00aa00'/><rect id='armR' x='85' y='60' width='5' height='20' fill='#00aa00'/><circle id='far' cx='300' cy='300' r='5' fill='#000000'/><circle id='tiny' cx='50' cy='90' r='0.05' fill='#000000'/></g></svg>"
    );
    let drives: Vec<Value> = (0..16).map(|i| json!({ "input": "lookX", "target": format!("p{i}"), "prop": "translateX", "values": [0, 6] })).collect();
    let crowd = warnings(&json!({ "svg": svg, "drives": drives, "roles": { "body": "body", "armL": "armL", "armR": "armR" } }));
    // The first drive, then as many others as the line holds, then how many more.
    assert_eq!(crowd[0], "lookX gives p0 translateX 3 at rest, not 0 (at drives[0]; also drives[1], drives[2] and 13 more drives): make each give its prop's rest at its input's rest");
    assert!(crowd.iter().all(|w| w.chars().count() <= look::MAX_PROBLEM_CHARS), "{crowd:#?}");
    // The other warnings are all there.
    assert_eq!(crowd.len(), 5, "{crowd:#?}");
    assert!(crowd[1].starts_with("armR has no pivot") && crowd[2].starts_with("armL has no pivot"), "{crowd:#?}");
    assert!(crowd[3].contains("reaches outside the viewBox") && crowd[4].contains("smaller than 2 units"), "{crowd:#?}");
    // With room, the others are named.
    let turned = |n: usize| {
        let drives: Vec<Value> = (0..n).map(|i| json!({ "input": "pace", "target": format!("p{i}"), "prop": "rotate", "values": [5, 0] })).collect();
        warnings(&json!({ "svg": "<svg viewBox='0 0 100 100'><circle id='p0' cx='20' cy='30' r='9'/><circle id='p1' cx='50' cy='30' r='9'/><circle id='p2' cx='80' cy='30' r='9'/><circle id='p3' cx='50' cy='70' r='9'/></svg>", "drives": drives }))
    };
    let (first, all) = ("pace gives p0 rotate 5 at rest, not 0 (at drives[0]", "make each give its prop's rest at its input's rest");
    assert_eq!(turned(1), [format!("{first}): make values[0] 0, unless that is meant")]);
    assert_eq!(turned(3), [format!("{first}; also drives[1], drives[2]): {all}")]);
    assert_eq!(turned(4), [format!("{first}; also drives[1], drives[2] and 1 more drive): {all}")]);
    // A line too long to name them counts them.
    let fewer = warnings(&json!({
        "svg": "<svg viewBox='0 0 100 100'><circle id='leftUpperWing' cx='20' cy='30' r='9'/><circle id='rightUpperWing' cx='50' cy='30' r='9'/><circle id='p0' cx='80' cy='30' r='9'/><circle id='p1' cx='50' cy='70' r='9'/></svg>",
        "drives": [
            { "input": "lookX", "target": ["leftUpperWing", "rightUpperWing"], "prop": "translateX", "values": [0, 6] },
            { "input": "pace", "target": "p0", "prop": "rotate", "values": [5, 0] },
            { "input": "pace", "target": "p1", "prop": "rotate", "values": [5, 0] }
        ]
    }));
    assert_eq!(fewer, [format!("lookX gives leftUpperWing, rightUpperWing translateX 3 at rest, not 0 (at drives[0]; also 2 more drives): {all}")]);
}

// ---------------------------------------------------------------- the words beside a picture

#[test]
fn a_caption_says_a_moment_of_nothing_as_0() {
    use super::preview::{Chosen, Moment, Played, Pose};
    for at in [-0.0, -0.00001, 0.00001] {
        let pose = Pose { animation: Some(("glow".into(), Some(at))), ..Pose::default() };
        let played = Played { builtin: false, looping: false, moment: Some(Moment { at, chosen: Chosen::Given, still: true }) };
        let caption = session::PictureRequest::Sheet(pose).caption(None, Some(&played));
        assert!(caption.contains(", inside glow at 0, on the light panel") && caption.ends_with(" At 0 glow is where it rests: give another at."), "{caption}");
        assert!(!caption.contains("-0"), "{caption}");
    }
}

#[test]
fn the_cost_beside_a_picture_is_said_where_it_differs_from_rest_by_one_or_more() {
    use super::preview::{Inputs, Picture, Pose, Spent};
    let picture = |spent: (f64, f64), rest: (f64, f64)| Picture { png: vec![], spent: Spent { filters: spent.0, layers: spent.1 }, rest: Spent { filters: rest.0, layers: rest.1 }, played: None };
    let pose = |spent, rest| session::PictureRequest::Sheet(Pose::default()).cost(&picture(spent, rest));
    // A fade on a look with nothing layered at rest adds a hundredth of a layer, which rounds up
    // to one: no line, as the design answer has none. Nor a hundredth beside a layer.
    assert_eq!(pose((0.0, 0.0186), (0.0, 0.0)), None);
    assert_eq!(pose((1.0, 1.0186), (1.0, 1.0)), None);
    assert_eq!(pose((7.9, 2.0), (7.2, 1.5)), None);
    // One or more apart: both, rounded up as the design answer rounds.
    assert_eq!(pose((2.5, 1.2), (1.2, 1.2)).as_deref(), Some("In this pose drawing costs about 3 of 60 for filters and 2 of 60 for layers; at rest 2 and 2."));
    assert_eq!(pose((0.2, 0.0), (1.5, 0.0)).as_deref(), Some("In this pose drawing costs about 1 of 60 for filters and 0 of 60 for layers; at rest 2 and 0."));
    // Beyond the budget, which only a close-up is drawn at, the line says what that means.
    let strip = session::PictureRequest::Strip { animation: "grow".into(), frames: 8, inputs: Inputs::default(), zoom: Some("dot".into()) };
    assert_eq!(
        strip.cost(&picture((1.0, 62.3), (1.0, 4.0))).as_deref(),
        Some("In its costliest moment drawing costs about 1 of 60 for filters and 63 of 60 for layers; at rest 1 and 4. Past 60 only a close-up of it can be drawn.")
    );
}

// ---------------------------------------------------------------- what else the design answer says is stored

#[tokio::test]
async fn a_design_answer_says_what_else_the_stored_design_holds() {
    let e = env().await;
    let tid = e.with_avatar();
    // The figure names three roles and a voice, nothing else of these: one line says so, after
    // the animations and before the usage.
    let drawn = e.design(&tid, figure(), T0).unwrap().text;
    let lines: Vec<&str> = drawn.lines().collect();
    let at = lines.iter().position(|line| line.starts_with("Also stored: ")).unwrap_or_else(|| panic!("{drawn}"));
    assert_eq!(lines[at], "Also stored: roles body, eyes, armR; voice en-GB.");
    assert!(lines[at - 1].starts_with("Your animations: ") && lines[at + 1].starts_with("Usage: "), "{drawn}");

    // A merge of other fields: the line names all of them, the ones the call did not send too.
    let merged = e.design(&tid, json!({ "roles": { "pupils": "pupil" }, "states": { "working": [], "idle": ["blink"] }, "mouth": { "target": "face", "prop": "scaleY", "open": 1.2 }, "gaze": 3 }), T0 + 1).unwrap().text;
    assert_eq!(stored_line(&merged), "Also stored: roles body, eyes, pupils, armR; states idle, working; mouth moves face; gaze 3; voice en-GB.");
    let one_field = e.design(&tid, json!({ "animations": { "hop": null } }), T0 + 2).unwrap().text;
    assert_eq!(stored_line(&one_field), "Also stored: roles body, eyes, pupils, armR; states idle, working; mouth moves face; gaze 3; voice en-GB.");
    // The same design again is answered with it too.
    let same = e.design(&tid, json!({ "gaze": 3 }), T0 + 3).unwrap().text;
    assert!(same.starts_with("Nothing to save") && stored_line(&same).ends_with("gaze 3; voice en-GB."), "{same}");
    let off = e.design(&tid, json!({ "gaze": 0, "voice": { "pitch": 1.1 } }), T0 + 4).unwrap().text;
    assert_eq!(stored_line(&off), "Also stored: roles body, eyes, pupils, armR; states idle, working; mouth moves face; gaze off; voice in the default language.");

    // A look with none of them says that, and the default look says only a voice it has.
    let bare = e.with_avatar();
    let text = e.design(&bare, json!({ "svg": "<svg viewBox='0 0 200 200'><circle id='face' cx='100' cy='100' r='50' fill='#00aaff'/></svg>" }), T0).unwrap().text;
    assert_eq!(stored_line(&text), STORED_NOTHING_ELSE);
    assert_eq!(STORED_NOTHING_ELSE, "Also stored: no roles, states, mouth, gaze or voice.");
    let plain = e.with_avatar();
    assert_eq!(e.design(&plain, json!({ "name": "Dot" }), T0).unwrap().text, "Saved as revision 1, named Dot.\nThe look is the default one: send svg to draw your own.");
    assert_eq!(e.design(&plain, json!({ "voice": { "lang": "de" } }), T0 + 1).unwrap().text, "Saved as revision 2, named Dot.\nThe look is the default one: send svg to draw your own.\nAlso stored: voice de.");

    // Long lists are counted, so the line stays short.
    let long = |n: usize| format!("mouthPart{n}{}", "x".repeat(20));
    let parts: String = (0..4).map(|n| format!("<rect id='{}' x='{}' y='120' width='8' height='4' fill='#aa3333'/>", long(n), 80 + 10 * n)).collect();
    let svg = format!("<svg viewBox='0 0 200 200'><g id='body'><g id='head'><g id='eyes'><circle id='pupils' cx='100' cy='80' r='4'/></g>{parts}</g><rect id='armL' x='50' y='100' width='10' height='40'/><rect id='armR' x='140' y='100' width='10' height='40'/></g></svg>");
    let states: serde_json::Map<String, Value> = ["idle", "working", "speaking", "listening", "waiting", "attention", "error", "done", "sleeping"].iter().map(|s| (s.to_string(), json!([]))).collect();
    let mouth: Vec<Value> = (0..4).map(|n| json!({ "target": long(n), "prop": "scaleY", "open": 1.5 })).collect();
    let design = json!({
        "svg": svg,
        "pivots": { "armL": [55, 100], "armR": [145, 100] },
        "roles": { "body": "body", "head": "head", "eyes": "eyes", "pupils": "pupils", "armL": "armL", "armR": "armR" },
        "states": states, "mouth": mouth, "gaze": 2, "voice": { "lang": "en-GB" }
    });
    let full = e.with_avatar();
    let text = e.design(&full, design, T0).unwrap().text;
    let line = stored_line(&text);
    assert_eq!(line, "Also stored: roles body, head, eyes, pupils, armL, armR; 9 states; mouth moves 4 parts; gaze 2; voice en-GB.");
    assert!(line.chars().count() <= STORED_CHARS);
}

// ---------------------------------------------------------------- the order of the parts

/// The parts are listed in document order, which is the drawing order: a part moved in front of
/// another in the same group comes after it, in the design answer and in the status alike.
#[tokio::test]
async fn the_parts_are_listed_in_the_order_they_are_drawn() {
    let e = env().await;
    let tid = e.with_avatar();
    let svg = |arm_in_front: bool| {
        let (torso, arm) = ("<rect id='torso' x='70' y='60' width='60' height='100' fill='#3366cc'/>", "<rect id='arm' x='120' y='70' width='16' height='50' fill='#cc6633'/>");
        let inside = if arm_in_front { format!("{torso}{arm}") } else { format!("{arm}{torso}") };
        format!("<svg viewBox='0 0 200 200'><g id='body'>{inside}</g></svg>")
    };
    let parts = |text: &str| -> Vec<String> { text.lines().skip_while(|l| !l.starts_with("Parts (")).skip(1).take_while(|l| !l.starts_with("No animations") && !l.starts_with("Your animations")).map(|l| l.split(' ').next().unwrap().to_string()).collect() };
    let status_parts = |now: i64| -> Vec<String> {
        let pass = gate(&e.state, &tid, Call::Status, Via::Tools, now).unwrap();
        status(&e.state, &pass, StatusRequest::default()).unwrap().avatar.unwrap().parts.iter().map(|p| p.split(' ').next().unwrap().to_string()).collect()
    };
    let behind = e.design(&tid, json!({ "svg": svg(false) }), T0).unwrap().text;
    assert_eq!(parts(&behind), ["body", "arm", "torso"], "{behind}");
    assert_eq!(status_parts(T0 + 1), ["body", "arm", "torso"]);
    let in_front = e.design(&tid, json!({ "svg": svg(true) }), T0 + 2).unwrap().text;
    assert_eq!(parts(&in_front), ["body", "torso", "arm"], "{in_front}");
    assert_eq!(status_parts(T0 + 3), ["body", "torso", "arm"]);
}

// ---------------------------------------------------------------- a role part that repeats another

#[test]
fn a_role_part_that_is_a_use_of_another_role_part_warns_that_it_moves_twice() {
    // The right arm as a mirrored use of the left arm, both named as roles: a use repeats the left
    // arm with all its motion, so Workbench's cheer turns the right arm twice.
    let mirrored = json!({
        "svg": "<svg viewBox='0 0 200 200'><g id='body'><rect x='70' y='60' width='60' height='100' fill='#3366cc'/><g id='armL'><rect x='50' y='70' width='16' height='50' fill='#3366cc'/></g><use id='armR' href='#armL' transform='translate(200 0) scale(-1 1)'/></g></svg>",
        "pivots": { "body": [100, 160], "armL": [58, 72], "armR": [58, 72] },
        "roles": { "body": "body", "armL": "armL", "armR": "armR" }
    });
    let said = warnings(&mirrored);
    assert_eq!(said, ["armR is a use of armL, the armL part: it also moves whenever armL moves (at roles.armR): draw it as a part of its own, or let both be uses of a shape in defs"]);
    assert!(said.iter().all(|w| w.chars().count() <= look::MAX_PROBLEM_CHARS));
    // Both arms as uses of one shape in defs: each moves for itself, and nothing is said.
    let apart = json!({
        "svg": "<svg viewBox='0 0 200 200'><defs><rect id='limb' x='50' y='70' width='16' height='50' fill='#3366cc'/></defs><g id='body'><rect x='70' y='60' width='60' height='100' fill='#3366cc'/><use id='armL' href='#limb'/><use id='armR' href='#limb' transform='translate(200 0) scale(-1 1)'/></g></svg>",
        "pivots": { "body": [100, 160], "armL": [58, 72], "armR": [58, 72] },
        "roles": { "body": "body", "armL": "armL", "armR": "armR" }
    });
    assert_eq!(warnings(&apart), [""; 0]);
}

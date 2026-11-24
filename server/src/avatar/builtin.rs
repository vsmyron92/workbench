//! Workbench's built-in motion, made as the browser makes it (OWNER: avatar slice).
//!
//! The browser moves any look that names roles (body, head, eyes, pupils, armL, armR) with eleven
//! animations of its own: breathe, blink, think, talk, listen, alert, doze, wave, cheer, nod and
//! shake (`lib/builtin.ts`, `build`). They are written against roles, not against one drawing:
//! generated from the look's roles, boxes and pivots, with amplitudes scaled to the viewBox, in
//! the look's own animation format. This file makes the same animations from the stored look,
//! number for number, so that a picture can show a session Workbench's wave on its own character
//! (a wrong arm pivot shows only there). Which built-in motion a look gets is said by [`needs`]
//! (`look::builtins` asks it of the typed look), and a test holds that to what is made here.
//!
//! The look is read as `preview` reads it (`preview::rig`): the stored JSON, as untrusted data. The
//! example looks and the parity looks (`examples/parity/`, one for each rule the examples do not
//! reach) hold what this file makes of them as fixture files (`<name>.builtin.json`) that a test
//! here and `lib/golden.test.ts` both compare with what they make. Where the two differ, the
//! browser is right.

use std::f64::consts::PI;

use serde_json::{Value, json};

use super::look::BUILTIN;
use super::preview;

/// Pupil travel when the look gives none, and the most a look may ask for, as parts of the
/// viewBox width (`GAZE_DEFAULT` and `GAZE_MOST` of builtin.ts).
const GAZE_DEFAULT: f64 = 0.03;
const GAZE_MOST: f64 = 0.05;

/// The part that plays a role, as the built-in motion needs it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Part {
    pub id: String,
    /// Where it rests in viewBox coordinates (`rootBox` of look.ts).
    pub root_box: Option<[f64; 4]>,
    /// Its own box, in the coordinates it is drawn in (the node's `box`).
    pub own_box: Option<[f64; 4]>,
    /// The pivot the look gives it, as written.
    pub pivot: Option<[f64; 2]>,
    /// From its own coordinates (inside its own transform) to the viewBox, at rest: the matrix of
    /// `lookIndex` in look.ts, `[a, b, c, d, e, f]`.
    pub matrix: [f64; 6],
}

/// What the built-in motion is made from: the viewBox, the look's `gaze`, and the part each role
/// names where it exists and can move (`rolePart` of builtin.ts).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rig {
    pub view: [f64; 4],
    pub gaze: Option<f64>,
    pub body: Option<Part>,
    pub head: Option<Part>,
    pub eyes: Option<Part>,
    pub pupils: Option<Part>,
    pub arm_l: Option<Part>,
    pub arm_r: Option<Part>,
}

/// The built-in animations a stored look's roles give it, by name, in the order of [`BUILTIN`]
/// (the order the browser makes them in). Its own animation of the same name replaces one where
/// it plays; it is listed here all the same. Nothing for what is not a look.
pub fn animations(look: &Value) -> Vec<(&'static str, Value)> {
    preview::rig(look).map(|rig| build(&rig)).unwrap_or_default()
}

/// The names of [`animations`]: which built-in motion a look gets, as a picture makes it. The
/// status, the design answer and gestures ask `look::builtins` instead, which reads the typed
/// look without making anything; a test holds the two answers equal.
pub fn names(look: &Value) -> Vec<&'static str> {
    animations(look).into_iter().map(|(name, _)| name).collect()
}

/// Whether `name` plays Workbench's built-in motion in this look: it has no animation of its own
/// by that name, and its roles give it that one.
pub fn is_workbench(look: &Value, name: &str) -> bool {
    let own = look.get("animations").and_then(|a| a.get(name)).is_some_and(Value::is_object);
    !own && BUILTIN.contains(&name) && names(look).contains(&name)
}

/// What the built-in `name` needs of the roles and the gaze, in words, or `None` when `has`
/// (whether the look names a role, by its name) and `gaze` (the look's, `None` for the default
/// travel) give it what it needs. The rule [`build`] follows, said for a session: a test holds
/// the two together.
pub fn needs(name: &str, has: impl Fn(&str) -> bool, gaze: Option<f64>) -> Option<&'static str> {
    match name {
        "breathe" | "alert" => (!has("body")).then_some("the body role"),
        "blink" => (!has("eyes")).then_some("the eyes role"),
        // The pupils think only where they may travel: a gaze of 0 keeps them still.
        "think" => (!has("head") && !(has("pupils") && gaze.is_none_or(|g| g > 0.0))).then_some("the head role, or the pupils role and a gaze above 0"),
        "talk" | "listen" | "nod" | "shake" => (!has("head") && !has("body")).then_some("the head or the body role"),
        "doze" => (!has("eyes") || !has("body")).then_some("the eyes and the body role"),
        "wave" => (!has("armR") && !has("armL")).then_some("the armR or the armL role"),
        "cheer" => (!(has("armL") && has("armR")) && !has("body")).then_some("both arm roles, or the body role"),
        _ => Some("a name of the built-in set"),
    }
}

/// Whether a look that names the pupils role still does not think: its gaze is 0 and it has no head.
fn pupils_still(name: &str, has: &impl Fn(&str) -> bool, gaze: Option<f64>) -> bool {
    name == "think" && has("pupils") && needs(name, has, gaze).is_some()
}

/// How a refusal of the built-in `name` that the look does not get ends, after what it [`needs`]
/// (`has` and `gaze` as there): the role it does not name, or for pupils that do not think, the gaze.
pub fn unmet(name: &str, has: impl Fn(&str) -> bool, gaze: Option<f64>) -> &'static str {
    if pupils_still(name, &has, gaze) { "and this look's gaze is 0" } else { "which this look does not name" }
}

/// The advice of the warning for a state that lists the built-in `name` the look does not get
/// (`has` and `gaze` as for [`needs`]).
pub fn remedy(name: &str, has: impl Fn(&str) -> bool, gaze: Option<f64>) -> &'static str {
    if pupils_still(name, &has, gaze) { "name head, or give a gaze above 0" } else { "name the role, or it does nothing in this look" }
}

/// Why a look gets no built-in motion, for the design answer (`has` and `gaze` as for [`needs`];
/// `replaced`, those its roles give it that its own animations of the same name replace).
pub fn none_applies(has: impl Fn(&str) -> bool, gaze: Option<f64>, replaced: &[&str]) -> String {
    let mut why = Vec::new();
    if pupils_still("think", &has, gaze) {
        why.push("the pupils think only with a gaze above 0, and this look's gaze is 0".to_string());
    }
    if !replaced.is_empty() {
        why.push(format!("your own animations of the same name replace {}", replaced.join(", ")));
    }
    if why.is_empty() { "name roles (body, head, eyes, pupils, armL, armR) and Workbench moves them".to_string() } else { why.join("; ") }
}

/// `Math.round(n * 1000) / 1000` as JavaScript works it out.
fn round(n: f64) -> f64 {
    js_round(n * 1000.0) / 1000.0
}

/// `Math.round`: the nearest whole number, a half towards +∞ (-2.5 is -2), and -0 for a value from
/// -0.5 up to 0.
fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    if (-0.5..0.0).contains(&x) {
        return -0.0;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `|| 0`: plain zero for -0 (and for what is not a number).
fn or_zero(n: f64) -> f64 {
    if n == 0.0 || n.is_nan() { 0.0 } else { n }
}

/// How far the pupils travel, in viewBox units (`gazeTravel`): the look's `gaze`, at most 5% of the
/// viewBox width; 3% when it gives none; 0 switches gaze off.
fn gaze_travel(rig: &Rig) -> f64 {
    let width = rig.view[2];
    match rig.gaze {
        None => width * GAZE_DEFAULT,
        Some(own) => own.max(0.0).min(width * GAZE_MOST),
    }
}

/// What undoes the linear part of a part's matrix to the viewBox (`undoLinear`): a vector on
/// screen through it is that vector inside the part, where its motion is written. `None` where
/// nothing needs undoing (the identity) or nothing can (a determinant within 1e-9 of 0): the
/// vector is then used as it is. The gaze (`preview`'s pose), think's drift and an arm's lift go
/// through it.
pub(super) fn undo_linear(m: [f64; 6]) -> Option<[f64; 4]> {
    let [a, b, c, d, _, _] = m;
    if a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 {
        return None;
    }
    let det = a * d - b * c;
    if !(det.abs() >= 1e-9) {
        return None;
    }
    let out = [d / det, -b / det, -c / det, a / det];
    out.iter().all(|n| n.is_finite()).then_some(out)
}

/// A vector on screen inside a part, through [`undo_linear`] of its matrix (`undone`).
pub(super) fn undone(u: Option<[f64; 4]>, x: f64, y: f64) -> (f64, f64) {
    match u {
        Some(u) => (u[0] * x + u[2] * y, u[1] * x + u[3] * y),
        None => (x, y),
    }
}

/// -1 where a matrix mirrors what is drawn in it (its determinant is below 0): a turn inside it
/// shows the other way round on screen (`flipOf`).
fn flip_of(m: &[f64; 6]) -> f64 {
    if m[0] * m[3] - m[1] * m[2] < 0.0 { -1.0 } else { 1.0 }
}

/// Which way an arm lifts (`liftSign`): -1 turns it anticlockwise on screen, which raises an arm on
/// the right half of the figure; an arm on the left half is raised by +1.
fn lift_sign(arm: &Part, right: bool, middle: f64) -> f64 {
    let given = if right { -1.0 } else { 1.0 };
    let Some(b) = arm.root_box else { return given };
    let centre = b[0] + b[2] / 2.0;
    if (centre - middle).abs() < 1e-6 {
        return given;
    }
    if centre > middle { -1.0 } else { 1.0 }
}

/// How far an arm turns about its joint to point `above` degrees over the horizon, away from the
/// body (`raiseTurn`): worked out on screen from where the arm lies at rest (its box against its
/// pivot, through its matrix), then given as the turn written inside its own transform. `None`
/// when the look gives the arm no pivot or no box.
fn raise_turn(arm: &Part, sign: f64, above: f64) -> Option<f64> {
    let pivot = arm.pivot?;
    let b = arm.own_box?;
    let ox = b[0] + b[2] / 2.0 - pivot[0];
    let oy = b[1] + b[3] / 2.0 - pivot[1];
    // From the joint to the box's centre on screen: through every transform around the arm and its own.
    let m = arm.matrix;
    let dx = m[0] * ox + m[2] * oy;
    let dy = m[1] * ox + m[3] * oy;
    if dx.hypot(dy) < 1e-6 {
        return None;
    }
    // Screen angles: 0 points right, 90 down. The raised arm points up and outwards.
    let rest = dy.atan2(dx) * 180.0 / PI;
    let want = if sign < 0.0 { -above } else { above - 180.0 };
    let mut turn = want - rest;
    while turn > 180.0 {
        turn -= 360.0;
    }
    while turn <= -180.0 {
        turn += 360.0;
    }
    // The arm turns inside its own transform: where that mirrors it, the other way round.
    Some(flip_of(&m) * if turn.abs() < 15.0 { sign * 15.0 } else { round(turn) })
}

/// One arm raised to `above` degrees over the horizon and swung `back` degrees from there and up
/// again: about its joint when it has one, else a small wiggle in place (`armTracks`). Either is
/// meant on screen, so an arm inside a mirrored or turned transform moves as one outside.
fn arm_tracks(arm: &Part, right: bool, above: f64, back: f64, middle: f64, lift: f64) -> Vec<Value> {
    let sign = lift_sign(arm, right, middle);
    if let Some(turn) = raise_turn(arm, sign, above) {
        let eased = round(turn - turn.signum() * back);
        return vec![json!({ "target": arm.id, "prop": "rotate", "values": [0.0, turn, eased, turn, eased, 0.0], "ease": "inOut" })];
    }
    // The wiggle turns the other way round inside a mirror, and lifts the arm up the screen.
    let flip = flip_of(&arm.matrix);
    let (ux, uy) = undone(undo_linear(arm.matrix), 0.0, -lift);
    let (x, y) = (or_zero(round(ux)), or_zero(round(uy)));
    let mut tracks = vec![
        json!({ "target": arm.id, "prop": "rotate", "values": [0.0, flip * sign * 16.0, flip * sign * -10.0, flip * sign * 16.0, flip * sign * -10.0, 0.0], "ease": "inOut" }),
        json!({ "target": arm.id, "prop": "translateY", "values": [0.0, y, y, y, y, 0.0], "ease": "inOut" }),
    ];
    if x != 0.0 {
        tracks.push(json!({ "target": arm.id, "prop": "translateX", "values": [0.0, x, x, x, x, 0.0], "ease": "inOut" }));
    }
    tracks
}

/// The built-in animations of a rig, by name, in the order of [`BUILTIN`]: `build` of builtin.ts,
/// step by step and with its arithmetic in its order.
pub(super) fn build(rig: &Rig) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    let [vx, vy, width, height] = rig.view;
    let id = |part: &Option<Part>| part.as_ref().map(|p| p.id.clone());
    let (body, head, eyes, pupils) = (id(&rig.body), id(&rig.head), id(&rig.eyes), id(&rig.pupils));

    if let Some(body) = &body {
        out.push((
            "breathe",
            json!({ "duration": 3.6, "loop": true, "ease": "inOut", "tracks": [
                { "target": body, "prop": "scaleY", "values": [1.0, 1.025, 1.0] },
                { "target": body, "prop": "scaleX", "values": [1.0, 0.99, 1.0] }
            ] }),
        ));
    }

    if let Some(eyes) = &eyes {
        out.push(("blink", json!({ "duration": 0.18, "every": [2.4, 6.0], "tracks": [{ "target": eyes, "prop": "scaleY", "values": [1.0, 0.1, 1.0] }] })));
    }

    // The pupils think where they may travel. A look that switched its gaze off keeps them still:
    // its head tilts instead, and without a head nothing thinks.
    let travel = gaze_travel(rig);
    let times = [0.0, 0.2, 0.55, 0.8, 1.0];
    match (&pupils, &head) {
        (Some(pupils), _) if travel > 0.0 => {
            let drift = travel;
            // Up and aside on screen and as far as the gaze goes: undone through the pupils' own
            // matrix, as the gaze is.
            let u = rig.pupils.as_ref().and_then(|p| undo_linear(p.matrix));
            let (xs, ys) = ([0.0, -0.8, -0.8, 0.6, 0.0], [0.0, -0.9, -0.9, -0.7, 0.0]);
            let keys: [(f64, f64); 5] = std::array::from_fn(|i| undone(u, xs[i], ys[i]));
            out.push((
                "think",
                json!({ "duration": 2.8, "loop": true, "ease": "inOut", "times": times, "tracks": [
                    { "target": pupils, "prop": "translateX", "values": keys.map(|(x, _)| or_zero(round(x * drift))) },
                    { "target": pupils, "prop": "translateY", "values": keys.map(|(_, y)| or_zero(round(y * drift))) }
                ] }),
            ));
        }
        (_, Some(head)) => {
            out.push(("think", json!({ "duration": 2.8, "loop": true, "ease": "inOut", "times": times, "tracks": [{ "target": head, "prop": "rotate", "values": [0.0, 4.0, 4.0, -2.5, 0.0] }] })));
        }
        _ => {}
    }

    if let Some(head) = &head {
        let dip = round(height * 0.012);
        out.push((
            "talk",
            json!({ "duration": 0.9, "loop": true, "ease": "inOut", "tracks": [
                { "target": head, "prop": "translateY", "values": [0.0, dip, 0.0, round(dip * 0.6), 0.0] },
                { "target": head, "prop": "rotate", "values": [0.0, 1.2, 0.0, -1.2, 0.0] }
            ] }),
        ));
        out.push(("listen", json!({ "duration": 2.4, "loop": true, "ease": "inOut", "tracks": [{ "target": head, "prop": "rotate", "values": [-5.0, -6.2, -5.0] }] })));
    } else if let Some(body) = &body {
        out.push(("talk", json!({ "duration": 0.9, "loop": true, "ease": "inOut", "tracks": [{ "target": body, "prop": "rotate", "values": [0.0, 1.6, 0.0, -1.6, 0.0] }] })));
        out.push(("listen", json!({ "duration": 2.4, "loop": true, "ease": "inOut", "tracks": [{ "target": body, "prop": "rotate", "values": [-2.5, -3.2, -2.5] }] })));
    }

    // How high the figure may hop, and how far it may stretch while it does, before the viewBox cuts it off.
    let body_box = rig.body.as_ref().and_then(|p| p.root_box);
    let room = match body_box {
        Some(b) => (b[1] - vy - height * 0.02).max(0.0),
        None => height * 0.1,
    };
    let hop = round((height * 0.07).min(room * 0.6).max(height * 0.015));
    let reach = match body_box {
        Some(b) if b[3] > 0.0 => room * 0.4 / b[3],
        _ => 0.05,
    };
    let stretch = round(1.0 + 0.05f64.min(reach).max(0.01));

    if let Some(body) = &body {
        out.push((
            "alert",
            json!({ "duration": 1.2, "loop": true, "times": [0.0, 0.14, 0.42, 0.7, 0.84, 1.0], "tracks": [
                { "target": body, "prop": "translateY", "values": [0.0, 0.0, -hop, 0.0, 0.0, 0.0], "ease": ["linear", "out", "in", "linear", "linear"] },
                { "target": body, "prop": "scaleY", "values": [1.0, 0.93, stretch, 0.94, 1.0, 1.0], "ease": "inOut" }
            ] }),
        ));
    }

    if let (Some(eyes), Some(body)) = (&eyes, &body) {
        out.push((
            "doze",
            json!({ "duration": 5.2, "loop": true, "tracks": [
                { "target": eyes, "prop": "scaleY", "values": [0.1, 0.1] },
                { "target": body, "prop": "scaleY", "values": [1.0, 1.035, 1.0], "ease": "inOut" },
                { "target": body, "prop": "scaleX", "values": [1.0, 0.985, 1.0], "ease": "inOut" }
            ] }),
        ));
    }

    let middle = match body_box {
        Some(b) => b[0] + b[2] / 2.0,
        None => vx + width / 2.0,
    };
    let lift = round(height * 0.03);

    // The right arm waves when there is one.
    let waver = match (&rig.arm_r, &rig.arm_l) {
        (Some(arm), _) => Some((arm, true)),
        (None, Some(arm)) => Some((arm, false)),
        (None, None) => None,
    };
    if let Some((arm, right)) = waver {
        out.push(("wave", json!({ "duration": 1.4, "tracks": arm_tracks(arm, right, 42.0, 32.0, middle, lift) })));
    }

    // Both arms, or the body alone; with everything, everything joins in.
    if (rig.arm_l.is_some() && rig.arm_r.is_some()) || body.is_some() {
        let mut tracks = Vec::new();
        if let Some(arm) = &rig.arm_l {
            tracks.extend(arm_tracks(arm, false, 38.0, 12.0, middle, lift));
        }
        if let Some(arm) = &rig.arm_r {
            tracks.extend(arm_tracks(arm, true, 38.0, 12.0, middle, lift));
        }
        if let Some(body) = &body {
            tracks.push(json!({ "target": body, "prop": "translateY", "values": [0.0, -hop, 0.0, round(-hop * 0.5), 0.0, 0.0], "ease": ["out", "in", "out", "in", "linear"] }));
            tracks.push(json!({ "target": body, "prop": "scaleY", "values": [1.0, stretch, 0.95, round(1.0 + (stretch - 1.0) * 0.5), 0.98, 1.0], "ease": "inOut" }));
        }
        out.push(("cheer", json!({ "duration": 1.2, "tracks": tracks })));
    }

    if let Some(head) = &head {
        let dip = round(height * 0.03);
        let swing = round(width * 0.025);
        out.push(("nod", json!({ "duration": 0.7, "ease": "inOut", "tracks": [{ "target": head, "prop": "translateY", "values": [0.0, dip, 0.0, round(dip * 0.6), 0.0] }] })));
        out.push((
            "shake",
            json!({ "duration": 0.8, "ease": "inOut", "tracks": [{ "target": head, "prop": "translateX", "values": [0.0, -swing, swing, round(-swing * 0.7), round(swing * 0.5), 0.0] }] }),
        ));
    } else if let Some(body) = &body {
        out.push(("nod", json!({ "duration": 0.7, "ease": "inOut", "tracks": [{ "target": body, "prop": "scaleY", "values": [1.0, 0.95, 1.0, 0.97, 1.0] }] })));
        out.push(("shake", json!({ "duration": 0.8, "ease": "inOut", "tracks": [{ "target": body, "prop": "rotate", "values": [0.0, -3.0, 3.0, -2.0, 1.5, 0.0] }] })));
    }

    out
}

/// The fixture file of a look (`examples/<name>.builtin.json`): its built-in animations by name,
/// one a line, names and keys in alphabetical order.
#[cfg(test)]
pub(super) fn fixture(look: &Value) -> String {
    let mut all = animations(look);
    all.sort_by_key(|(name, _)| *name);
    let lines: Vec<String> = all.iter().map(|(name, animation)| format!("  {}: {}", Value::from(*name), animation)).collect();
    format!("{{\n{}\n}}\n", lines.join(",\n"))
}

#[cfg(test)]
#[path = "builtin_tests.rs"]
mod tests;

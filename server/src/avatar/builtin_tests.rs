//! Tests of the built-in motion (`builtin`): what each role gets, against numbers worked out by
//! hand, the one rule of which built-ins a look gets, JavaScript's rounding, arms and the gaze
//! inside mirrored and turned transforms, and the fixture files of the example looks and the
//! parity looks that the browser's golden test reads too.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::*;

const ROLES: [&str; 6] = ["body", "head", "eyes", "pupils", "armL", "armR"];

/// A look with just these roles, each a group with a box (and a pivot when asked): `withRoles` of
/// builtin.test.ts.
fn with_roles(roles: &[&str], pivots: bool) -> Value {
    let boxes = |role: &str| match role {
        "body" => json!([60, 40, 80, 130]),
        "head" => json!([70, 40, 60, 50]),
        "eyes" => json!([80, 55, 40, 12]),
        "pupils" => json!([85, 58, 30, 6]),
        "armL" => json!([40, 100, 20, 40]),
        _ => json!([140, 100, 20, 40]),
    };
    let joints = |role: &str| match role {
        "body" => json!([100, 170]),
        "head" => json!([100, 90]),
        "eyes" | "pupils" => json!([100, 61]),
        "armL" => json!([60, 100]),
        _ => json!([140, 100]),
    };
    let (mut nodes, mut named, mut joined) = (vec![], Map::new(), Map::new());
    for role in roles {
        nodes.push(json!({ "tag": "g", "id": role, "box": boxes(role) }));
        named.insert(role.to_string(), json!(role));
        if pivots {
            joined.insert(role.to_string(), joints(role));
        }
    }
    json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes, "pivots": joined, "roles": named, "animations": {}, "states": {}, "mouth": [], "gaze": null })
}

fn all(look: &Value) -> Map<String, Value> {
    animations(look).into_iter().map(|(name, a)| (name.to_string(), a)).collect()
}

/// The values of the track of `target` and `prop` in an animation.
fn values(animation: &Value, target: &str, prop: &str) -> Vec<f64> {
    let track = animation["tracks"].as_array().unwrap().iter().find(|t| t["target"] == target && t["prop"] == prop).unwrap_or_else(|| panic!("no {target} {prop} in {animation}"));
    track["values"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
}

fn targets(animation: &Value) -> Vec<String> {
    animation["tracks"].as_array().unwrap().iter().map(|t| format!("{} {}", t["target"].as_str().unwrap(), t["prop"].as_str().unwrap())).collect()
}

#[test]
fn a_look_with_all_six_roles_gets_every_built_in_with_these_numbers() {
    let look = with_roles(&ROLES, true);
    assert_eq!(names(&look), BUILTIN);
    let got = all(&look);
    // What breathes, blinks and loops, as builtin.test.ts has it.
    assert_eq!(got["breathe"], json!({ "duration": 3.6, "loop": true, "ease": "inOut", "tracks": [{ "target": "body", "prop": "scaleY", "values": [1.0, 1.025, 1.0] }, { "target": "body", "prop": "scaleX", "values": [1.0, 0.99, 1.0] }] }));
    assert_eq!(got["blink"], json!({ "duration": 0.18, "every": [2.4, 6.0], "tracks": [{ "target": "eyes", "prop": "scaleY", "values": [1.0, 0.1, 1.0] }] }));
    let looping: Vec<&str> = BUILTIN.into_iter().filter(|name| got[*name]["loop"] == true).collect();
    assert_eq!(looping, ["breathe", "think", "talk", "listen", "alert", "doze"]);
    for name in ["blink", "wave", "cheer", "nod", "shake"] {
        assert!(got[name].get("loop").is_none(), "{name}");
    }
    // The pupils drift 3% of the width (gaze not given), four fifths of it sideways.
    assert_eq!(values(&got["think"], "pupils", "translateX"), [0.0, -4.8, -4.8, 3.6, 0.0]);
    assert_eq!(values(&got["think"], "pupils", "translateY"), [0.0, -5.4, -5.4, -4.2, 0.0]);
    assert_eq!(got["think"]["times"], json!([0.0, 0.2, 0.55, 0.8, 1.0]));
    // The head talks, listens, nods and shakes, scaled to the viewBox.
    assert_eq!(values(&got["talk"], "head", "translateY"), [0.0, 2.4, 0.0, 1.44, 0.0]);
    assert_eq!(values(&got["listen"], "head", "rotate"), [-5.0, -6.2, -5.0]);
    assert_eq!(values(&got["nod"], "head", "translateY"), [0.0, 6.0, 0.0, 3.6, 0.0]);
    assert_eq!(values(&got["shake"], "head", "translateX"), [0.0, -5.0, 5.0, -3.5, 2.5, 0.0]);
    // The body starts at y 40: room for the whole hop of 7% and the whole stretch.
    assert_eq!(values(&got["alert"], "body", "translateY"), [0.0, 0.0, -14.0, 0.0, 0.0, 0.0]);
    assert_eq!(values(&got["alert"], "body", "scaleY"), [1.0, 0.93, 1.05, 0.94, 1.0, 1.0]);
    assert_eq!(got["alert"]["tracks"][0]["ease"], json!(["linear", "out", "in", "linear", "linear"]));
    assert_eq!(targets(&got["doze"]), ["eyes scaleY", "body scaleY", "body scaleX"]);
    // The right arm hangs from (140, 100) to its box centre (150, 120), 63.435 degrees under the
    // horizon, and is raised to 42 over it on the right, away from the body: about its joint.
    assert_eq!(targets(&got["wave"]), ["armR rotate"]);
    assert_eq!(values(&got["wave"], "armR", "rotate"), [0.0, -105.435, -73.435, -105.435, -73.435, 0.0]);
    // Cheering, both arms mirror each other and the body hops.
    assert_eq!(targets(&got["cheer"]), ["armL rotate", "armR rotate", "body translateY", "body scaleY"]);
    assert_eq!(values(&got["cheer"], "armL", "rotate"), [0.0, 101.435, 89.435, 101.435, 89.435, 0.0]);
    assert_eq!(values(&got["cheer"], "armR", "rotate"), [0.0, -101.435, -89.435, -101.435, -89.435, 0.0]);
    assert_eq!(values(&got["cheer"], "body", "translateY"), [0.0, -14.0, 0.0, -7.0, 0.0, 0.0]);
    assert_eq!(values(&got["cheer"], "body", "scaleY"), [1.0, 1.05, 0.95, 1.025, 0.98, 1.0]);
}

#[test]
fn a_look_without_arms_or_without_a_head_gets_what_its_roles_allow() {
    // No arms: no wave, and a cheer of the body alone.
    let armless = all(&with_roles(&["body", "head", "eyes", "pupils"], true));
    assert!(!armless.contains_key("wave"));
    assert_eq!(targets(&armless["cheer"]), ["body translateY", "body scaleY"]);
    // No head: the body talks, listens, nods and shakes, the pupils think.
    let headless = all(&with_roles(&["body", "eyes", "pupils", "armL", "armR"], true));
    assert_eq!(headless.keys().filter(|k| !BUILTIN.contains(&k.as_str())).count(), 0);
    assert_eq!(headless.len(), 11);
    assert_eq!(values(&headless["talk"], "body", "rotate"), [0.0, 1.6, 0.0, -1.6, 0.0]);
    assert_eq!(values(&headless["listen"], "body", "rotate"), [-2.5, -3.2, -2.5]);
    assert_eq!(values(&headless["nod"], "body", "scaleY"), [1.0, 0.95, 1.0, 0.97, 1.0]);
    assert_eq!(values(&headless["shake"], "body", "rotate"), [0.0, -3.0, 3.0, -2.0, 1.5, 0.0]);
    assert_eq!(targets(&headless["think"]), ["pupils translateX", "pupils translateY"]);
    // Neither head nor pupils: nothing thinks. A head alone thinks with a tilt.
    assert!(!all(&with_roles(&["body", "eyes"], true)).contains_key("think"));
    assert_eq!(values(&all(&with_roles(&["head"], true))["think"], "head", "rotate"), [0.0, 4.0, 4.0, -2.5, 0.0]);
    // Only a left arm: it waves, raised the other way round.
    let lefty = all(&with_roles(&["body", "armL"], true));
    assert!(values(&lefty["wave"], "armL", "rotate")[1] > 0.0);
    // An arm without a pivot wiggles in place instead of turning about a joint it does not have.
    let unjointed = all(&with_roles(&["body", "armR"], false));
    assert_eq!(targets(&unjointed["wave"]), ["armR rotate", "armR translateY"]);
    assert_eq!(values(&unjointed["wave"], "armR", "rotate"), [0.0, -16.0, 10.0, -16.0, 10.0, 0.0]);
    assert_eq!(values(&unjointed["wave"], "armR", "translateY"), [0.0, -6.0, -6.0, -6.0, -6.0, 0.0]);
    // Nothing for no roles, and for what is not a look.
    assert!(names(&with_roles(&[], true)).is_empty());
    assert!(names(&Value::Null).is_empty() && names(&json!({})).is_empty());
}

#[test]
fn the_gaze_and_the_room_above_the_body_change_the_numbers() {
    // Gaze switched off: the pupils stay where they are and the head thinks instead; without a
    // head nothing thinks, as "0 keeps them still" promises.
    let mut still = with_roles(&["head", "pupils"], true);
    still["gaze"] = json!(0);
    assert_eq!(targets(&all(&still)["think"]), ["head rotate"]);
    let mut headless = with_roles(&["pupils", "body"], true);
    headless["gaze"] = json!(0);
    assert!(!all(&headless).contains_key("think"));
    assert_eq!(needs("think", |role| role == "pupils" || role == "body", Some(0.0)), Some("the head role, or the pupils role and a gaze above 0"));
    // A gaze above 0, however small, and they think as far as it goes.
    headless["gaze"] = json!(0.5);
    assert_eq!(values(&all(&headless)["think"], "pupils", "translateX"), [0.0, -0.4, -0.4, 0.3, 0.0]);
    // A gaze of its own, cut at 5% of the width.
    let mut far = with_roles(&["pupils"], true);
    far["gaze"] = json!(50);
    assert_eq!(values(&all(&far)["think"], "pupils", "translateY"), [0.0, -9.0, -9.0, -7.0, 0.0]);
    // A body near the top of the viewBox hops less; one that starts at its top hops 1.5%.
    let hop = |look: &Value| -values(&all(look)["alert"], "body", "translateY")[2];
    let mut tight = with_roles(&["body"], true);
    tight["nodes"][0]["box"] = json!([60, 10, 80, 160]);
    assert_eq!(hop(&tight), 3.6);
    assert_eq!(values(&all(&tight)["alert"], "body", "scaleY")[2], 1.015);
    let mut none = with_roles(&["body"], true);
    none["nodes"][0]["box"] = json!([60, 0, 80, 180]);
    assert_eq!(hop(&none), 3.0);
    // Moved up by a transform on a parent: the room is measured where the body really is.
    let mut moved = with_roles(&["body"], true);
    moved["nodes"] = json!([{ "tag": "g", "attrs": { "transform": "translate(0 -30)" }, "children": moved["nodes"].clone() }]);
    assert_eq!(hop(&moved), 3.6);
    // Amplitudes follow the viewBox; angles and scale factors have no unit.
    let mut large = with_roles(&["body", "head"], true);
    large["viewBox"] = json!([0, 0, 2000, 2000]);
    assert_eq!(values(&all(&large)["nod"], "head", "translateY"), [0.0, 60.0, 0.0, 36.0, 0.0]);
    assert_eq!(values(&all(&large)["talk"], "head", "rotate"), [0.0, 1.2, 0.0, -1.2, 0.0]);
}

#[test]
fn which_built_ins_a_look_gets_is_one_rule() {
    // Every set of roles, with joints and without, with the default gaze, none and one: what is
    // made is what `needs` says applies.
    for mask in 0..64u32 {
        let roles: Vec<&str> = ROLES.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, r)| *r).collect();
        for gaze in [None, Some(0.0), Some(3.0)] {
            let expected: Vec<&str> = BUILTIN.into_iter().filter(|name| needs(name, |role| roles.contains(&role), gaze).is_none()).collect();
            for pivots in [true, false] {
                let mut look = with_roles(&roles, pivots);
                look["gaze"] = json!(gaze);
                assert_eq!(names(&look), expected, "{roles:?} {gaze:?}");
            }
        }
    }
    assert_eq!(needs("wave", |_| false, None), Some("the armR or the armL role"));
    assert_eq!(needs("walk", |_| true, None), Some("a name of the built-in set"));
    assert_eq!(needs("think", |role| role == "pupils", Some(0.0)), Some("the head role, or the pupils role and a gaze above 0"));
    assert_eq!(needs("think", |role| role == "pupils", None), None);
    assert_eq!(needs("think", |role| role == "head", Some(0.0)), None);
    // The answer the status and the design answer give for a stored look is this one.
    let applied = crate::avatar::look::apply(None, &serde_json::from_str(include_str!("examples/hoot.json")).unwrap()).unwrap();
    let look = applied.design.look.unwrap();
    assert_eq!(crate::avatar::look::builtins(&look), names(&serde_json::to_value(&look).unwrap()));
}

#[test]
fn a_role_that_names_nothing_that_moves_gives_nothing() {
    let mut look = with_roles(&["body", "head"], true);
    look["roles"]["eyes"] = json!("nowhere");
    look["roles"]["armR"] = json!(7);
    look["nodes"].as_array_mut().unwrap().push(json!({ "tag": "defs", "children": [{ "tag": "clipPath", "id": "c", "children": [{ "tag": "circle", "id": "inside" }] }] }));
    look["roles"]["pupils"] = json!("inside");
    look["roles"]["armL"] = json!("c");
    look["nodes"].as_array_mut().unwrap().push(json!({ "tag": "g", "id": "constructor", "box": [0, 0, 10, 10] }));
    look["roles"]["head"] = json!("constructor");
    assert_eq!(names(&look), ["breathe", "talk", "listen", "alert", "cheer", "nod", "shake"]);
}

#[test]
fn numbers_are_rounded_as_javascript_rounds_them() {
    assert_eq!((js_round(2.5), js_round(-2.5), js_round(-2.6), js_round(0.49999999999999994)), (3.0, -2.0, -3.0, 0.0));
    assert!(js_round(-0.4).is_sign_negative() && js_round(0.4).is_sign_positive());
    assert_eq!((js_round(-1800.5), js_round(1800.5)), (-1800.0, 1801.0));
    assert!(round(-0.0004).is_sign_negative() && or_zero(round(-0.0004)).is_sign_positive());
}

#[test]
fn a_name_plays_workbenchs_motion_where_the_look_has_none_of_its_own() {
    let pip: Value = serde_json::from_str(include_str!("examples/pip.look.json")).unwrap();
    // pip waves its own way, and blinks Workbench's.
    assert!(!is_workbench(&pip, "wave") && is_workbench(&pip, "blink") && is_workbench(&pip, "nod"));
    assert!(!is_workbench(&pip, "hop") && !is_workbench(&pip, "walk"));
    assert!(!is_workbench(&with_roles(&["eyes"], true), "breathe"));
}

/// What a session could have written: each example design with the built-ins as its own
/// animations is accepted.
#[test]
fn what_is_made_is_an_animation_a_session_could_have_written() {
    for design in [include_str!("examples/pip.json"), include_str!("examples/tock.json"), include_str!("examples/hoot.json")] {
        let mut design: Value = serde_json::from_str(design).unwrap();
        let look = serde_json::to_value(crate::avatar::look::apply(None, &design).unwrap().design.look.unwrap()).unwrap();
        for (name, animation) in animations(&look) {
            design["animations"][name] = animation;
        }
        if let Err(problems) = crate::avatar::look::apply(None, &design) {
            panic!("{}: {problems:#?}", design["name"]);
        }
    }
}

/// The directory of the example looks, and of the parity looks under it.
fn examples() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("avatar").join("examples")
}

/// Every `<name>.look.json` of `dir`, by name in the order of the names, with what its
/// `<name>.builtin.json` holds: checked to be what this file makes of the look, or with
/// `AVATAR_WRITE_BUILTIN=1` written so. No fixture outlives its look.
fn fixtures_in(dir: &Path) -> Vec<(String, Value, Map<String, Value>)> {
    let write = std::env::var_os("AVATAR_WRITE_BUILTIN").is_some_and(|v| v == "1");
    let (files, all) = looks_in(dir);
    let looks: Vec<&str> = all.iter().map(|(name, _)| name.as_str()).collect();
    let mut out = vec![];
    for (name, look) in all.iter().cloned() {
        let made = fixture(&look);
        let path = dir.join(format!("{name}.builtin.json"));
        if write {
            std::fs::write(&path, &made).unwrap();
        }
        let stored = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing: write it with AVATAR_WRITE_BUILTIN=1", path.display()));
        assert_eq!(stored, made, "{} is not what the server makes of {name}: write it again with AVATAR_WRITE_BUILTIN=1 and review the difference", path.display());
        out.push((name, look, serde_json::from_str(&stored).unwrap()));
    }
    for file in files.iter().filter_map(|f| f.strip_suffix(".builtin.json")) {
        assert!(looks.contains(&file), "{file}.builtin.json has no look");
    }
    out
}

/// The names of the files of `dir`, sorted, and every `<name>.look.json` there, read.
fn looks_in(dir: &Path) -> (Vec<String>, Vec<(String, Value)>) {
    let mut files: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    let looks = files
        .iter()
        .filter_map(|f| f.strip_suffix(".look.json"))
        .map(|name| (name.to_string(), serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.look.json"))).unwrap()).unwrap()))
        .collect();
    (files, looks)
}

/// `examples/<name>.builtin.json` holds what this file makes of `examples/<name>.look.json`, for
/// every example look: `lib/golden.test.ts` makes the same with the browser's code and compares.
/// After a deliberate change, write them again with `AVATAR_WRITE_BUILTIN=1 cargo test --locked
/// avatar::builtin::tests` (the parity looks' too) and review the difference.
#[test]
fn the_examples_built_in_motion_is_in_their_fixture_files() {
    let looks = fixtures_in(&examples());
    assert!(looks.len() >= 3, "{:?}", looks.iter().map(|(name, ..)| name).collect::<Vec<_>>());
    for (name, _, made) in &looks {
        // The examples name all six roles: every built-in is in it.
        assert_eq!(made.len(), BUILTIN.len(), "{name}");
    }
}

/// The parity looks (`examples/parity/`): stored looks that reach what the examples do not, each
/// with its `.builtin.json` that `lib/golden.test.ts` compares with the browser's motion too. One
/// role at a time, no joint, the gaze at 0 and small, a body at the top of its viewBox, an arm
/// near its raise, arms in the middle and beside it, arms mirrored, turned, skewed and inside a
/// flipped figure, pupils inside a figure drawn twice as large, and a number that rounds at a
/// negative half thousandth.
#[test]
fn the_parity_looks_built_in_motion_is_in_their_fixture_files() {
    let looks = fixtures_in(&examples().join("parity"));
    let named: Vec<&str> = looks.iter().map(|(name, ..)| name.as_str()).collect();
    for name in ["arms", "aside", "body", "flipped", "flipped-loose", "head", "lefty-loose", "middle", "near", "pupils-scaled", "pupils-small", "pupils-still", "skewed", "tight", "turned", "usearm"] {
        assert!(named.contains(&name), "{name} in {named:?}");
    }
    for (name, look, made) in &looks {
        // Each is a look as the validator stores it, and gets what the server says it gets.
        let typed: crate::avatar::look::Look = serde_json::from_value(look.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let again = crate::avatar::look::check_stored(json!({ "look": look })).and_then(|design| design.look);
        assert_eq!(again.as_ref(), Some(&typed), "{name} is not a look as the validator stores it");
        assert_eq!(made.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(), names(look).into_iter().collect(), "{name}");
    }
    let of = |name: &str| &looks.iter().find(|(n, ..)| n == name).unwrap().2;
    // What the rules make where the examples never go, said once more in numbers.
    assert!(of("pupils-still").is_empty(), "gaze 0 and no head: nothing thinks");
    assert_eq!(values(&of("pupils-small")["think"], "pupils", "translateY"), [0.0, -0.013, -0.013, -0.01, 0.0], "-0.0135 rounds up, as JavaScript rounds");
    assert_eq!(values(&of("tight")["alert"], "body", "scaleY")[2], 1.01, "the stretch floor");
    assert_eq!(values(&of("tight")["alert"], "body", "translateY")[2], -3.3, "the hop floor: 1.5% of 220");
    assert_eq!(values(&of("near")["wave"], "armR", "rotate")[1], -15.0, "an arm 8 degrees from its raise turns 15");
    // The right arm made as a mirrored use of the left: the turn is the control's, the other way round.
    assert_eq!(values(&of("usearm")["wave"], "armR", "rotate"), [0.0, 92.194, 60.194, 92.194, 60.194, 0.0]);
    // Inside scale(2) the pupils think half as far in their own units: 3% of the width of 400 on screen.
    assert_eq!(values(&of("pupils-scaled")["think"], "pupils", "translateX"), [0.0, -4.8, -4.8, 3.6, 0.0]);
    assert_eq!(values(&of("pupils-scaled")["think"], "pupils", "translateY"), [0.0, -5.4, -5.4, -4.2, 0.0]);
}

#[test]
fn the_pupils_think_up_and_aside_on_screen_and_as_far_as_the_gaze_whatever_transform_they_are_in() {
    // The pupils of with_roles, drawn inside a group with this transform: what think moves them by.
    let think = |transform: &str| {
        let mut look = with_roles(&["pupils"], true);
        look["nodes"] = json!([{ "tag": "g", "attrs": { "transform": transform }, "children": look["nodes"].clone() }]);
        let got = all(&look);
        let along = |prop: &str| values(&got["think"], "pupils", prop);
        (along("translateX"), along("translateY"))
    };
    // Moved only: as before, 3% of the width (6), up and to the left on screen.
    let plain = (vec![0.0, -4.8, -4.8, 3.6, 0.0], vec![0.0, -5.4, -5.4, -4.2, 0.0]);
    assert_eq!(think("translate(5 7)"), plain);
    // Twice as large: half as far inside, as far on screen. Their gaze goes as far.
    assert_eq!(think("scale(2)"), (vec![0.0, -2.4, -2.4, 1.8, 0.0], vec![0.0, -2.7, -2.7, -2.1, 0.0]));
    // Upside down, mirrored and turned a quarter: inside, the way that is up and aside on screen.
    assert_eq!(think("scale(1 -1)"), (plain.0.clone(), vec![0.0, 5.4, 5.4, 4.2, 0.0]));
    assert_eq!(think("scale(-1 1)"), (vec![0.0, 4.8, 4.8, -3.6, 0.0], plain.1.clone()));
    assert_eq!(think("rotate(90)"), (vec![0.0, -5.4, -5.4, -4.2, 0.0], vec![0.0, 4.8, 4.8, -3.6, 0.0]));
    assert_eq!(think("rotate(180)"), (vec![0.0, 4.8, 4.8, -3.6, 0.0], vec![0.0, 5.4, 5.4, 4.2, 0.0]));
    // No -0 at rest, whichever way the matrix turns.
    for transform in ["scale(1 -1)", "scale(-1 1)", "rotate(90)", "rotate(180)"] {
        let (x, y) = think(transform);
        assert!([x[0], x[4], y[0], y[4]].iter().all(|v| v.is_sign_positive()), "{transform}");
    }
}

#[test]
fn pupils_that_keep_still_are_told_so_and_other_built_ins_keep_their_words() {
    let pupils = |role: &str| role == "pupils";
    // Pupils with gaze 0 and no head: what is missing is the gaze, not a role.
    assert_eq!(unmet("think", pupils, Some(0.0)), "and this look's gaze is 0");
    assert_eq!(remedy("think", pupils, Some(0.0)), "name head, or give a gaze above 0");
    // The warning is whole at the longest place a state list has: a warning is cut at 160 characters.
    let warning = format!("think is built in and needs {} (at states.attention[5]): {}", needs("think", pupils, Some(0.0)).unwrap(), remedy("think", pupils, Some(0.0)));
    assert!(warning.chars().count() <= crate::avatar::look::MAX_PROBLEM_CHARS, "{warning}");
    assert_eq!(none_applies(pupils, Some(0.0), &[]), "the pupils think only with a gaze above 0, and this look's gaze is 0");
    // Without the pupils, and for every other built-in, the words are as they were.
    for (name, has) in [("think", "body"), ("wave", "pupils"), ("blink", "pupils"), ("cheer", "armR")] {
        assert_eq!(unmet(name, |role| role == has, Some(0.0)), "which this look does not name", "{name}");
        assert_eq!(remedy(name, |role| role == has, Some(0.0)), "name the role, or it does nothing in this look", "{name}");
    }
    // No roles at all; roles whose built-ins its own animations replace; and both reasons.
    assert_eq!(none_applies(|_| false, None, &[]), "name roles (body, head, eyes, pupils, armL, armR) and Workbench moves them");
    assert_eq!(none_applies(|role| role == "armR", None, &["wave"]), "your own animations of the same name replace wave");
    assert_eq!(
        none_applies(|role| role == "pupils" || role == "armR", Some(0.0), &["wave"]),
        "the pupils think only with a gaze above 0, and this look's gaze is 0; your own animations of the same name replace wave"
    );
}

/// What the status, the design answer and gestures say a look gets (`look::builtins`, from the
/// typed look) is what the browser and the pictures make of it as stored (`names`): for the
/// examples, the parity looks and every set of roles with the gaze not given, 0 and 3.
#[test]
fn the_built_ins_a_stored_look_gets_are_what_its_motion_holds() {
    use crate::avatar::look::{Look, apply, builtins};
    let check = |look: &Look, what: &str| assert_eq!(builtins(look), names(&serde_json::to_value(look).unwrap()), "{what}");
    for dir in [examples(), examples().join("parity")] {
        for (name, look) in looks_in(&dir).1 {
            check(&serde_json::from_value(look).unwrap(), &name);
        }
    }
    let parts = "<g id='body'><rect x='60' y='40' width='80' height='130'/></g><g id='head'><rect x='70' y='40' width='60' height='50'/></g><g id='eyes'><rect x='80' y='55' width='40' height='12'/></g><g id='pupils'><rect x='85' y='58' width='30' height='6'/></g><g id='armL'><rect x='40' y='100' width='20' height='40'/></g><g id='armR'><rect x='140' y='100' width='20' height='40'/></g>";
    for mask in 0..64u32 {
        let roles: Map<String, Value> = ROLES.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, r)| (r.to_string(), json!(r))).collect();
        for gaze in [None, Some(0), Some(3)] {
            let mut design = json!({ "svg": format!("<svg viewBox='0 0 200 200'>{parts}</svg>"), "roles": roles });
            if let Some(gaze) = gaze {
                design["gaze"] = json!(gaze);
            }
            let look = apply(None, &design).unwrap_or_else(|problems| panic!("{problems:?}")).design.look.unwrap();
            check(&look, &format!("{roles:?} {gaze:?}"));
        }
    }
}

#[test]
fn an_arm_is_raised_on_screen_whatever_transform_it_is_drawn_in() {
    // The left arm of builtin.test.ts, drawn again as the right one in a mirror about x 100.
    let mirrored = |pivots: bool| {
        let mut look = with_roles(&["body", "armL"], pivots);
        look["nodes"].as_array_mut().unwrap().push(json!({ "tag": "g", "attrs": { "transform": "translate(200 0) scale(-1 1)" }, "children": [{ "tag": "g", "id": "armR", "box": [40, 100, 20, 40] }] }));
        look["roles"]["armR"] = json!("armR");
        if pivots {
            look["pivots"]["armR"] = json!([60, 100]);
        }
        look
    };
    // On screen it is the plain right arm (with_roles), so it turns as far, the other way round.
    let plain = all(&with_roles(&["body", "armL", "armR"], true));
    let got = all(&mirrored(true));
    let turned = |a: &Value| values(a, "armR", "rotate").iter().map(|v| if *v == 0.0 { 0.0 } else { -v }).collect::<Vec<f64>>();
    assert_eq!(values(&got["wave"], "armR", "rotate"), turned(&plain["wave"]));
    assert_eq!(values(&got["cheer"], "armR", "rotate"), turned(&plain["cheer"]));
    assert_eq!(values(&got["cheer"], "armL", "rotate"), values(&plain["cheer"], "armL", "rotate"));
    // Without a joint it wiggles the other way round, and is lifted up the screen all the same.
    let loose = all(&mirrored(false));
    assert_eq!(targets(&loose["wave"]), ["armR rotate", "armR translateY"]);
    assert_eq!(values(&loose["wave"], "armR", "rotate"), [0.0, 16.0, -10.0, 16.0, -10.0, 0.0]);
    assert_eq!(values(&loose["wave"], "armR", "translateY"), [0.0, -6.0, -6.0, -6.0, -6.0, 0.0]);
    // Drawn turned a quarter, its lift is along its own x.
    let mut quarter = with_roles(&["body", "armR"], false);
    quarter["nodes"][1]["attrs"] = json!({ "transform": "rotate(90 150 120)" });
    let wave = &all(&quarter)["wave"];
    assert_eq!(targets(wave), ["armR rotate", "armR translateY", "armR translateX"]);
    assert_eq!(values(wave, "armR", "translateX"), [0.0, -6.0, -6.0, -6.0, -6.0, 0.0]);
    assert_eq!(values(wave, "armR", "translateY"), [0.0; 6]);
    assert!(values(wave, "armR", "translateY").iter().all(|v| v.is_sign_positive()));
    // An arm drawn with its own turn: worked out where it really points, so it ends 42 over the horizon.
    let mut own = with_roles(&["body", "armR"], true);
    own["nodes"][1]["attrs"] = json!({ "transform": "rotate(-40 140 100)" });
    let turn = values(&all(&own)["wave"], "armR", "rotate")[1];
    assert_eq!(turn, -65.435, "63.435 under the horizon, less the 40 it is turned, to 42 over it");
}

#[test]
fn a_vector_on_screen_is_undone_into_a_part() {
    assert_eq!(undo_linear([1.0, 0.0, 0.0, 1.0, 30.0, -4.0]), None, "a translation needs nothing");
    assert_eq!(undo_linear([-1.0, 0.0, 0.0, 1.0, 200.0, 0.0]), Some([-1.0, 0.0, 0.0, 1.0]));
    assert_eq!(undone(undo_linear([-1.0, 0.0, 0.0, 1.0, 200.0, 0.0]), 6.0, 0.0), (-6.0, 0.0));
    assert_eq!(undone(undo_linear([2.0, 0.0, 0.0, 0.5, 0.0, 0.0]), 6.0, 3.0), (3.0, 6.0));
    // A part squashed flat cannot be undone: the vector stays as it is.
    assert_eq!(undo_linear([1.0, 0.0, 0.0, 1e-12, 0.0, 0.0]), None);
    assert_eq!(undone(None, 6.0, -2.0), (6.0, -2.0));
}

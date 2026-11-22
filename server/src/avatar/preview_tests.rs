//! Tests of the picture renderer (`preview`): the SVG text it writes, the pose math against
//! numbers worked out by hand, the eases, the pictures themselves, and its refusals.

use serde_json::{Value, json};

use super::*;

fn pip() -> Value {
    serde_json::from_str(include_str!("examples/pip.look.json")).unwrap()
}

fn tock() -> Value {
    serde_json::from_str(include_str!("examples/tock.look.json")).unwrap()
}

/// The example that has what the first two lack: a `use`, a mask, a filter, dashes, parents, drives.
fn hoot() -> Value {
    serde_json::from_str(include_str!("examples/hoot.look.json")).unwrap()
}

/// A look with the parts of the newer format: `use`, a mask, filters (a drop shadow, a glow, a
/// dilation), dashes, `parents` and drives on `turn`.
fn gadget() -> Value {
    json!({
        "viewBox": [0, 0, 200, 200],
        "nodes": [
            { "tag": "defs", "children": [
                { "tag": "filter", "id": "shadow", "attrs": { "x": "-20%", "y": "-20%", "width": "140%", "height": "140%" }, "children": [
                    { "tag": "feDropShadow", "attrs": { "dx": 5, "dy": 7, "stdDeviation": 3, "floodColor": "#000000", "floodOpacity": 0.5 } }
                ] },
                { "tag": "filter", "id": "glow", "attrs": { "x": "-100%", "y": "-100%", "width": "300%", "height": "300%" }, "children": [
                    { "tag": "feGaussianBlur", "attrs": { "in": "SourceGraphic", "stdDeviation": 5, "result": "soft" } },
                    { "tag": "feMerge", "children": [
                        { "tag": "feMergeNode", "attrs": { "in": "soft" } },
                        { "tag": "feMergeNode", "attrs": { "in": "SourceGraphic" } }
                    ] }
                ] },
                { "tag": "filter", "id": "fat", "children": [
                    { "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": 2, "result": "wide" } }
                ] },
                { "tag": "mask", "id": "stripes", "children": [
                    { "tag": "rect", "attrs": { "x": 0, "y": 0, "width": 200, "height": 200, "fill": "#ffffff" } },
                    { "tag": "rect", "attrs": { "x": 0, "y": 118, "width": 200, "height": 10, "fill": "#000000" } },
                    { "tag": "rect", "attrs": { "x": 0, "y": 142, "width": 200, "height": 10, "fill": "#000000" } }
                ] },
                { "tag": "g", "id": "leaf", "box": [-12, -6, 24, 12], "children": [
                    { "tag": "ellipse", "attrs": { "cx": 0, "cy": 0, "rx": 12, "ry": 6, "fill": "#2f9e44" } }
                ] }
            ] },
            { "tag": "g", "id": "body", "attrs": { "filterRef": "shadow" }, "box": [60, 42, 80, 128], "children": [
                { "tag": "rect", "id": "torso", "attrs": { "x": 60, "y": 100, "width": 80, "height": 70, "rx": 16, "fill": "#4dabf7", "maskRef": "stripes" }, "box": [60, 100, 80, 70] },
                { "tag": "circle", "id": "head", "attrs": { "cx": 100, "cy": 70, "r": 28, "fill": "#ffd43b", "stroke": "#1e1f22", "strokeWidth": 3, "strokeDasharray": "8 5" }, "box": [72, 42, 56, 56] }
            ] },
            { "tag": "line", "id": "cord", "attrs": { "x1": 100, "y1": 26, "x2": 100, "y2": 42, "stroke": "currentColor", "strokeWidth": 2, "strokeDasharray": "3 2" }, "box": [100, 26, 0, 16] },
            { "tag": "circle", "id": "bulb", "attrs": { "cx": 100, "cy": 20, "r": 8, "fill": "#ff922b", "filterRef": "glow" }, "box": [92, 12, 16, 16] },
            { "tag": "use", "id": "leafL", "attrs": { "useRef": "leaf", "x": 46, "y": 130 }, "box": [34, 124, 24, 12] },
            { "tag": "use", "id": "leafR", "attrs": { "useRef": "leaf", "x": 154, "y": 130, "filterRef": "fat" }, "box": [142, 124, 24, 12] }
        ],
        "pivots": { "body": [100, 170] },
        "roles": { "body": "body", "head": "head" },
        "animations": {
            "bob": { "duration": 1, "loop": true, "tracks": [ { "target": "head", "prop": "translateY", "values": [0, -6, 0], "ease": "inOut" } ] },
            "pop": { "duration": 0.8, "tracks": [
                { "target": "bulb", "prop": "scale", "values": [1, 1.6, 1], "ease": [0.3, 1.6, 0.6, 1] },
                { "target": "leafR", "prop": "rotate", "values": [0, 40], "delay": 0.2 }
            ] }
        },
        "states": {},
        "mouth": [],
        "gaze": null,
        "parents": { "bulb": "head", "cord": "head" },
        "drives": [
            { "input": "turn", "target": "body", "prop": "rotate", "values": [-16, 0, 16] },
            { "input": "turn", "target": "head", "prop": "skewX", "values": [-12, 0, 12] }
        ]
    })
}

/// A small look of one part to pose: a 20 by 20 square at (40, 40) turning about (50, 40).
fn square(animation: Value) -> Value {
    json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [ { "tag": "rect", "id": "arm", "attrs": { "x": 40, "y": 40, "width": 20, "height": 20 }, "box": [40, 40, 20, 20] } ],
        "pivots": { "arm": [50, 40] }, "roles": {}, "animations": { "move": animation }, "states": {}, "mouth": [], "gaze": null
    })
}

fn posed(look: &Value, pose: &Pose) -> (Vec<Option<Matrix>>, HashMap<String, Vec<(&'static str, String)>>, HashMap<String, usize>) {
    let model = Model::read(look).unwrap();
    let p = model.pose(pose).unwrap();
    let ids: HashMap<String, usize> = model.ids.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let set = p.set.iter().filter_map(|(i, s)| model.els[*i].id.map(|id| (id.to_string(), s.clone()))).collect();
    (p.transform, set, ids)
}

fn at(name: &str, progress: f64) -> Pose {
    Pose { animation: Some((name.to_string(), progress)), inputs: Inputs::default() }
}

fn close(a: Matrix, b: Matrix) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9)
}

#[track_caller]
fn assert_matrix(got: Option<Matrix>, want: Matrix) {
    let got = got.expect("a transform");
    assert!(close(got, want), "got {got:?}, want {want:?}");
}

// ---------------------------------------------------------------- the SVG text

#[test]
fn a_small_look_is_written_from_the_tables() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [
            { "tag": "defs", "children": [
                { "tag": "linearGradient", "id": "sky", "attrs": { "x1": 0, "y1": 0, "x2": 0, "y2": 1 }, "box": [0, 0, 0, 0], "children": [
                    { "tag": "stop", "attrs": { "offset": 0, "stopColor": "#ffcc00" } },
                    { "tag": "stop", "attrs": { "offset": "100%", "stopColor": "currentColor", "stopOpacity": 0.5 } }
                ] }
            ] },
            { "tag": "g", "id": "body", "attrs": { "transform": "translate(10 0)", "opacity": "50%" }, "children": [
                { "tag": "circle", "id": "head", "attrs": { "cx": 40, "cy": 40, "r": 20, "fillRef": "sky", "stroke": "#333333", "strokeWidth": 2 }, "box": [20, 20, 40, 40] },
                { "tag": "path", "id": "smile", "attrs": { "d": "M30,50Q40 60,50 50a5 5 0 1010 0z", "fill": "none", "stroke": "currentColor", "strokeLinecap": "round" } },
                { "tag": "text", "id": "label", "attrs": { "x": 40, "y": 90, "fontSize": 10, "fontWeight": 700, "textAnchor": "middle" }, "text": "hi <&>" }
            ] }
        ],
        "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null
    });
    let model = Model::read(&look).unwrap();
    let body = model.body(&model.pose(&Pose::default()).unwrap());
    assert_eq!(
        body,
        "<defs><linearGradient id='sky' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#ffcc00'/><stop offset='100%' stop-color='currentColor' stop-opacity='0.5'/></linearGradient></defs>\
         <g id='body' transform='matrix(1 0 0 1 10 0)' opacity='50%'>\
         <circle id='head' fill='url(#sky)' stroke='#333333' stroke-width='2' cx='40' cy='40' r='20'/>\
         <path id='smile' fill='none' stroke='currentColor' stroke-linecap='round' d='M 30 50 Q 40 60 50 50 a 5 5 0 1 0 10 0 z'/>\
         <text id='label' x='40' y='90' font-size='10' font-weight='700' text-anchor='middle' font-family='sans-serif'>hi &lt;&amp;&gt;</text></g>"
    );
    assert!(model.has_text);
    // The document around it: the panel's size, the view and the colour of `currentColor`.
    assert_eq!(
        document("<g/>", [0.0, 0.0, 100.0, 100.0], 264, LIGHT.fg),
        "<svg xmlns='http://www.w3.org/2000/svg' width='264' height='264' viewBox='0 0 100 100' color='#1e1f22'><g/></svg>"
    );
}

#[test]
fn what_does_not_fit_the_tables_is_never_written() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [
            { "tag": "script", "text": "alert(1)" },
            { "tag": "foreignObject", "children": [ { "tag": "circle", "attrs": { "r": 5 } } ] },
            { "tag": "image", "attrs": { "href": "file:///etc/passwd" } },
            { "tag": "circle", "id": "x' onload='alert(1)", "attrs": { "r": 5, "onload": "alert(1)", "style": "fill:red", "href": "http://example.com/a.svg" } },
            { "tag": "circle", "id": "1st", "attrs": { "r": 5, "fill": "red; background:url(http://example.com)" } },
            { "tag": "circle", "id": "ok", "attrs": { "r": 5, "fill": "url(http://example.com/#a)", "fillRef": "x' onload='y", "strokeRef": "nowhere", "clipPathRef": "ok" } },
            { "tag": "circle", "id": "ok", "attrs": { "r": 6 } },
            { "tag": "use", "attrs": { "useRef": "http://example.com/a.svg#b", "x": 1 } },
            { "tag": "use", "attrs": { "useRef": "ok", "x": "1e400" } },
            { "tag": "text", "attrs": { "x": 1, "fontSize": "12px; font-family: x" }, "text": "</text><script>alert('x')</script>\u{0}\u{7}" },
            { "tag": "path", "attrs": { "d": "M0 0 L10 10 <script>" } },
            { "tag": "rect", "attrs": { "transform": "rotate(10) url(#x)", "width": "10", "height": 10 } },
            { "tag": "defs", "children": [
                { "tag": "filter", "id": "f", "children": [
                    { "tag": "feImage", "attrs": { "href": "file:///etc/passwd" } },
                    { "tag": "feGaussianBlur", "attrs": { "in": "BackgroundImage", "stdDeviation": "3; x", "result": "a'b" } },
                    { "tag": "feTurbulence", "attrs": { "numOctaves": 1000000, "baseFrequency": 0.05 } },
                    { "tag": "feMorphology", "attrs": { "radius": 1000, "operator": "dilate" } }
                ] }
            ] }
        ],
        "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null
    });
    let model = Model::read(&look).unwrap();
    let body = model.body(&model.pose(&Pose::default()).unwrap());
    for never in ["<script", "foreignObject", "image", "onload", "style", "http", "file:", "href='h", "1st", "red", "px", "url(#x", "BackgroundImage", "a'b", "a&apos;b", "1000000", "\u{0}", "\u{7}"] {
        assert!(!body.contains(never), "{never:?} in {body}");
    }
    assert_eq!(
        body,
        "<circle r='5'/><circle r='5'/><circle id='ok' r='5'/><circle r='6'/><use x='1'/><use href='#ok'/>\
         <text x='1' font-family='sans-serif'>&lt;/text&gt;&lt;script&gt;alert('x')&lt;/script&gt;</text><path/><rect height='10'/>\
         <defs><filter id='f'><feGaussianBlur/><feTurbulence baseFrequency='0.05'/><feOffset dx='0' dy='0'/></filter></defs>"
    );
    // A clip-path that names a circle, not a clipPath, is left out; so is a gradient that is not there.
    assert!(model.ids.contains_key("ok") && !model.ids.contains_key("1st"));
}

#[test]
fn a_use_never_repeats_what_holds_a_use_or_itself() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [
            { "tag": "g", "id": "outer", "children": [
                { "tag": "use", "id": "self", "attrs": { "useRef": "outer" } },
                { "tag": "circle", "id": "dot", "attrs": { "r": 2 } }
            ] },
            { "tag": "use", "id": "twice", "attrs": { "useRef": "outer" } },
            { "tag": "use", "id": "fine", "attrs": { "useRef": "dot", "x": 5 } },
            { "tag": "use", "attrs": { "useRef": "fine" } },
            { "tag": "defs", "children": [ { "tag": "clipPath", "id": "cp", "children": [ { "tag": "circle", "id": "inclip", "attrs": { "r": 3 } } ] } ] },
            { "tag": "use", "attrs": { "useRef": "inclip" } },
            { "tag": "use", "attrs": { "useRef": "cp" } }
        ],
        "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null
    });
    let model = Model::read(&look).unwrap();
    let body = model.body(&model.pose(&Pose::default()).unwrap());
    assert_eq!(
        body,
        "<g id='outer'><use id='self'/><circle id='dot' r='2'/></g><use id='twice'/><use id='fine' href='#dot' x='5'/><use/>\
         <defs><clipPath id='cp'><circle id='inclip' r='3'/></clipPath></defs><use/><use/>"
    );
}

#[test]
fn references_inside_a_clip_or_a_mask_and_what_the_new_format_adds() {
    let look = gadget();
    let model = Model::read(&look).unwrap();
    let body = model.body(&model.pose(&Pose::default()).unwrap());
    for want in [
        "<filter id='shadow' x='-20%' y='-20%' width='140%' height='140%'><feDropShadow dx='5' dy='7' stdDeviation='3' flood-color='#000000' flood-opacity='0.5'/></filter>",
        "<feGaussianBlur in='SourceGraphic' stdDeviation='5' result='soft'/><feMerge><feMergeNode in='soft'/><feMergeNode in='SourceGraphic'/></feMerge>",
        // A dilation in two passes, the second taking the first's result and giving the named one.
        "<filter id='fat'><feMorphology operator='dilate' radius='2 0.0001' result='_m0'/><feMorphology in='_m0' operator='dilate' radius='0.0001 2' result='wide'/></filter>",
        "<mask id='stripes'><rect fill='#ffffff' x='0' y='0' width='200' height='200'/>",
        "<g id='body' filter='url(#shadow)'>",
        "<rect id='torso' fill='#4dabf7' mask='url(#stripes)' x='60' y='100' width='80' height='70' rx='16'/>",
        "stroke-dasharray='8 5'",
        "<use id='leafL' href='#leaf' x='46' y='130'/>",
        "<use id='leafR' filter='url(#fat)' href='#leaf' x='154' y='130'/>",
    ] {
        assert!(body.contains(want), "{want} not in {body}");
    }
    // Nothing inside a mask or a clipPath is clipped, masked or filtered again.
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [ { "tag": "defs", "children": [
            { "tag": "filter", "id": "f", "children": [ { "tag": "feOffset", "attrs": { "dx": 1 } } ] },
            { "tag": "clipPath", "id": "c", "children": [ { "tag": "circle", "attrs": { "r": 5, "clipPathRef": "c", "filterRef": "f" } } ] },
            { "tag": "mask", "id": "m", "children": [ { "tag": "g", "attrs": { "maskRef": "m", "clipPathRef": "c", "filterRef": "f" }, "children": [ { "tag": "rect", "attrs": { "width": 5, "height": 5 } }, { "tag": "text", "text": "no" } ] } ] }
        ] } ],
        "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null
    });
    let model = Model::read(&look).unwrap();
    assert_eq!(
        model.body(&model.pose(&Pose::default()).unwrap()),
        "<defs><filter id='f'><feOffset dx='1'/></filter><clipPath id='c'><circle r='5'/></clipPath><mask id='m'><g><rect width='5' height='5'/></g></mask></defs>"
    );
}

#[test]
fn the_walk_is_bounded() {
    // 400 elements, and a nest deeper than a look may be: what lies beyond the limits is left out.
    let many: Vec<Value> = (0..400).map(|_| json!({ "tag": "circle", "attrs": { "r": 1 } })).collect();
    let mut deep = json!({ "tag": "circle", "attrs": { "r": 1 } });
    for _ in 0..40 {
        deep = json!({ "tag": "g", "children": [deep] });
    }
    let look = json!({ "viewBox": [0, 0, 100, 100], "nodes": [deep, { "tag": "g", "children": many }] });
    let model = Model::read(&look).unwrap();
    assert_eq!(model.els.len(), MAX_ELEMENTS);
    assert_eq!(model.els.iter().filter(|e| e.tag == Tag::G).count(), MAX_DEPTH + 1);
    // Every `use` counts with what it repeats, as the validator counts it.
    let mut nodes = vec![json!({ "tag": "g", "id": "big", "children": (0..50).map(|_| json!({ "tag": "circle", "attrs": { "r": 1 } })).collect::<Vec<_>>() })];
    nodes.extend((0..20).map(|_| json!({ "tag": "use", "attrs": { "useRef": "big" } })));
    let look = json!({ "viewBox": [0, 0, 100, 100], "nodes": nodes });
    let model = Model::read(&look).unwrap();
    let body = model.body(&model.pose(&Pose::default()).unwrap());
    // 51 for the group, 51 for each use: four of them fit.
    assert_eq!(body.matches("<use").count(), (MAX_ELEMENTS - 51) / 51);
    assert!(Model::read(&json!({ "viewBox": [0, 0, 5, 5], "nodes": [] })).is_err(), "a viewBox smaller than a look's");
    assert!(Model::read(&json!({ "viewBox": [0, 0, "100", 100], "nodes": [] })).is_err());
}

// ---------------------------------------------------------------- pose math

#[test]
fn a_rotation_a_scale_and_a_skew_about_the_pivot() {
    // rotate(90) about (50, 40): x' = -y + 90, y' = x - 10.
    let look = square(json!({ "duration": 1, "tracks": [ { "target": "arm", "prop": "rotate", "values": [0, 90] } ] }));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], [0.0, 1.0, -1.0, 0.0, 90.0, -10.0]);
    // Halfway: 45 degrees.
    let (t, _, ids) = posed(&look, &at("move", 0.5));
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert_matrix(t[ids["arm"]], [h, h, -h, h, 50.0 + 40.0 * h - 50.0 * h, 40.0 - 50.0 * h - 40.0 * h]);
    // scale(2) about (50, 40).
    let look = square(json!({ "duration": 1, "tracks": [ { "target": "arm", "prop": "scale", "values": [1, 2] } ] }));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], [2.0, 0.0, 0.0, 2.0, -50.0, -40.0]);
    // skewX(45) about (50, 40): x' = x + y - 40.
    let look = square(json!({ "duration": 1, "tracks": [ { "target": "arm", "prop": "skewX", "values": [0, 45] } ] }));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], [1.0, 0.0, 1.0, 1.0, -40.0, 0.0]);
    // skewY(45) about (50, 40): y' = x + y - 50.
    let look = square(json!({ "duration": 1, "tracks": [ { "target": "arm", "prop": "skewY", "values": [0, 45] } ] }));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], [1.0, 1.0, 0.0, 1.0, 0.0, -50.0]);
    // Both, as the engine writes them: translate(50 40) skewX(45) skewY(45) translate(-50 -40).
    // skewX·skewY = [2 1 1 1]: x' = 2x + y - 90, y' = x + y - 50 (Y first would give x' = x + y - 40).
    let look = square(json!({ "duration": 1, "tracks": [ { "target": "arm", "prop": "skewX", "values": [0, 45] }, { "target": "arm", "prop": "skewY", "values": [0, 45] } ] }));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], [2.0, 1.0, 1.0, 1.0, -90.0, -50.0]);
    // Translate, then rotate, then skew, then scale, all about the pivot, after the part's own transform.
    let mut look = square(json!({ "duration": 1, "tracks": [
        { "target": "arm", "prop": "translateX", "values": [0, 5] },
        { "target": "arm", "prop": "rotate", "values": [0, 90] },
        { "target": "arm", "prop": "skewX", "values": [0, 45] },
        { "target": "arm", "prop": "scaleY", "values": [1, 2] }
    ] }));
    look["nodes"][0]["attrs"]["transform"] = json!("translate(0 10)");
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    let want = [translate(0.0, 10.0), translate(5.0, 0.0), about(50.0, 40.0, rotate(90.0)), about(50.0, 40.0, skew_x(45.0)), about(50.0, 40.0, scale(1.0, 2.0))].into_iter().fold(IDENTITY, mul);
    assert_matrix(t[ids["arm"]], want);
    // Written out: rotate(90) about (50, 40) of the point (60, 40) is (50, 50).
    let m = want;
    let p = |x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let (x, y) = p(50.0, 40.0);
    assert!((x - 55.0).abs() < 1e-9 && (y - 50.0).abs() < 1e-9, "the pivot only moves by the translations: ({x}, {y})");
}

#[test]
fn relative_props_add_scales_multiply_absolute_props_are_set() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [ { "tag": "circle", "id": "c", "attrs": { "cx": 50, "cy": 50, "r": 10, "fill": "#ff0000", "opacity": 0.8 }, "box": [40, 40, 20, 20] } ],
        "pivots": {}, "roles": {}, "states": {}, "mouth": [], "gaze": null,
        "animations": { "a": { "duration": 2, "tracks": [
            { "target": "c", "prop": "translateX", "values": [0, 4] },
            { "target": "c", "prop": "translateX", "values": [0, 6] },
            { "target": "c", "prop": "scale", "values": [1, 2] },
            { "target": "c", "prop": "scaleX", "values": [1, 3] },
            { "target": "c", "prop": "r", "values": [null, 20] },
            { "target": "c", "prop": "fill", "values": ["#000000", "#0000ff"] },
            { "target": "c", "prop": "opacity", "values": [null, 0.2], "times": [0, 1], "ease": "in" }
        ] } }
    });
    let (t, set, ids) = posed(&look, &at("a", 0.5));
    // Pivot: the box's centre (50, 50). translate 5, scale (1.5 * 2, 1.5).
    assert_matrix(t[ids["c"]], mul(translate(5.0, 0.0), about(50.0, 50.0, scale(3.0, 1.5))));
    let set = &set["c"];
    let get = |name: &str| set.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str());
    // r: from the artwork's own 10 to 20; fill halfway from black to blue; opacity eased "in" from 0.8.
    assert_eq!(get("r"), Some("15"));
    assert_eq!(get("fill"), Some("#000080"));
    assert_eq!(get("opacity"), Some("0.65"));
}

#[test]
fn a_loop_follows_its_phase_and_a_one_shot_its_delay() {
    let look = square(json!({ "duration": 2, "loop": true, "tracks": [
        { "target": "arm", "prop": "translateX", "values": [0, 10, 0] },
        { "target": "arm", "prop": "translateY", "values": [0, 10, 0], "phase": 0.25 }
    ] }));
    let (t, _, ids) = posed(&look, &at("move", 0.25));
    assert_matrix(t[ids["arm"]], translate(5.0, 10.0));
    // 0.75 + 0.25 wraps to 0.
    let (t, _, ids) = posed(&look, &at("move", 0.75));
    assert_matrix(t[ids["arm"]], translate(5.0, 0.0));
    // A one-shot runs for its duration and its longest delay: 1 + 1 s. At 0.25 of that (0.5 s) the
    // delayed track has not started; at 0.5 (1 s) the first is at its last keyframe; at 0.75
    // (1.5 s) it has played and is at rest again, as in the browser.
    let look = square(json!({ "duration": 1, "tracks": [
        { "target": "arm", "prop": "translateX", "values": [0, 10] },
        { "target": "arm", "prop": "translateY", "values": [0, 10], "delay": 1 }
    ] }));
    let (t, _, ids) = posed(&look, &at("move", 0.25));
    assert_matrix(t[ids["arm"]], translate(5.0, 0.0));
    let (t, _, ids) = posed(&look, &at("move", 0.5));
    assert_matrix(t[ids["arm"]], translate(10.0, 0.0));
    let (t, _, ids) = posed(&look, &at("move", 0.75));
    assert_matrix(t[ids["arm"]], translate(0.0, 5.0));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], translate(0.0, 10.0));
}

/// `spin` turns a quarter about (50, 50) with `holder` in it; `hand` sits in a group moved by
/// (10, 0) and follows `holder`; `finger` follows `hand`.
fn rig(hand_moves: bool) -> Value {
    json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [
            { "tag": "g", "id": "spin", "box": [40, 40, 20, 20], "children": [
                { "tag": "rect", "id": "holder", "attrs": { "x": 40, "y": 40, "width": 20, "height": 20 }, "box": [40, 40, 20, 20] }
            ] },
            { "tag": "g", "attrs": { "transform": "translate(10 0)" }, "children": [
                { "tag": "circle", "id": "hand", "attrs": { "cx": 70, "cy": 50, "r": 4 }, "box": [66, 46, 8, 8] }
            ] },
            { "tag": "circle", "id": "finger", "attrs": { "cx": 90, "cy": 50, "r": 2 }, "box": [88, 48, 4, 4] }
        ],
        "pivots": { "spin": [50, 50] }, "roles": {}, "states": {}, "mouth": [], "gaze": null,
        "animations": { "turn": { "duration": 1, "tracks": [
            { "target": "spin", "prop": "rotate", "values": [0, 90] },
            { "target": "hand", "prop": "translateX", "values": [0, if hand_moves { 5 } else { 0 }] }
        ] } },
        "parents": { "hand": "holder", "finger": "hand" }
    })
}

#[test]
fn a_child_follows_a_parent_that_sits_in_a_rotating_group() {
    let (t, _, ids) = posed(&rig(false), &at("turn", 1.0));
    // The quarter turn about (50, 50): x' = -y + 100, y' = x.
    assert_matrix(t[ids["spin"]], [0.0, 1.0, -1.0, 0.0, 100.0, 0.0]);
    // holder itself does not move: it is carried by spin in the drawing.
    assert_matrix(t[ids["holder"]].or(Some(IDENTITY)), IDENTITY);
    // hand: C = P⁻¹·D(holder)·P with P = translate(10 0): translate(-10 0)·R·translate(10 0).
    assert_matrix(t[ids["hand"]], [0.0, 1.0, -1.0, 0.0, 90.0, 10.0]);
    // finger follows hand, which is as far from its rest as the quarter turn.
    assert_matrix(t[ids["finger"]], [0.0, 1.0, -1.0, 0.0, 100.0, 0.0]);
    // At rest nothing carries anything: C is the identity.
    let (t, _, ids) = posed(&rig(false), &Pose::default());
    assert_matrix(t[ids["hand"]], IDENTITY);
    assert_matrix(t[ids["finger"]], IDENTITY);
}

#[test]
fn a_chain_of_two_carries_the_middle_parts_own_motion() {
    let (t, _, ids) = posed(&rig(true), &at("turn", 1.0));
    // hand's own translate(5 0) is in its own coordinates, after C.
    assert_matrix(t[ids["hand"]], [0.0, 1.0, -1.0, 0.0, 90.0, 15.0]);
    // finger: D(hand) = W(hand)·R(hand)⁻¹ = [0 1 -1 0 100 15]·translate(-10 0): the turn and hand's
    // step of 5 along x, which the turn carries down along y.
    assert_matrix(t[ids["finger"]], [0.0, 1.0, -1.0, 0.0, 100.0, 5.0]);
}

#[test]
fn parents_that_go_round_in_a_loop_or_cannot_be_inverted_rest() {
    let mut look = rig(true);
    look["parents"] = json!({ "hand": "finger", "finger": "hand", "spin": "spin" });
    let (t, _, ids) = posed(&look, &at("turn", 1.0));
    assert_matrix(t[ids["spin"]], [0.0, 1.0, -1.0, 0.0, 100.0, 0.0]);
    // Both parts of the loop are drawn as they rest, hand's own step included.
    assert_eq!((t[ids["hand"]], t[ids["finger"]]), (None, None));
    // A parent squashed flat cannot be inverted against: its child rests.
    let mut look = rig(false);
    look["animations"]["turn"]["tracks"] = json!([ { "target": "spin", "prop": "scaleX", "values": [1, 0] } ]);
    look["parents"] = json!({ "hand": "holder" });
    let (t, _, ids) = posed(&look, &at("turn", 1.0));
    // D(holder) = W(holder)·R(holder)⁻¹ is the flat scale itself and inverts fine; the child follows it.
    assert_matrix(t[ids["hand"]], mul(translate(-10.0, 0.0), mul(about(50.0, 50.0, scale(0.0, 1.0)), translate(10.0, 0.0))));
    // But a part whose own group is flat has no P⁻¹: it is left as it rests.
    let mut look = rig(false);
    look["nodes"][1]["attrs"]["transform"] = json!("scale(0 1)");
    let (t, _, ids) = posed(&look, &at("turn", 1.0));
    assert_eq!(t[ids["hand"]], None);
}

/// A loop through nesting, as only a file changed by hand can hold: `a` follows `c`, `c` follows
/// `b`, and `b` is drawn inside `a`. Working it out never ends on its own: both parts of the loop
/// rest, and what is not in it moves as before.
#[test]
fn parents_that_loop_through_nesting_rest() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [
            { "tag": "g", "id": "a", "box": [10, 10, 20, 20], "children": [
                { "tag": "rect", "id": "b", "attrs": { "x": 10, "y": 10, "width": 20, "height": 20 }, "box": [10, 10, 20, 20] }
            ] },
            { "tag": "circle", "id": "c", "attrs": { "cx": 70, "cy": 70, "r": 5 }, "box": [65, 65, 10, 10] },
            { "tag": "circle", "id": "d", "attrs": { "cx": 50, "cy": 50, "r": 5 }, "box": [45, 45, 10, 10] }
        ],
        "pivots": {}, "roles": {}, "states": {}, "mouth": [], "gaze": null,
        "animations": { "go": { "duration": 1, "tracks": [
            { "target": ["a", "b", "c", "d"], "prop": "translateX", "values": [0, 10] },
            { "target": "c", "prop": "opacity", "values": [1, 0.5] }
        ] } },
        "parents": { "a": "c", "c": "b", "d": "c" }
    });
    let (t, set, ids) = posed(&look, &at("go", 1.0));
    assert_eq!((t[ids["a"]], t[ids["c"]]), (None, None), "the loop rests");
    assert!(!set.contains_key("c"), "its paint too");
    assert_matrix(t[ids["b"]], translate(10.0, 0.0));
    // d follows c, which rests where it is: d's own step alone.
    assert_matrix(t[ids["d"]], translate(10.0, 0.0));
    let model = Model::read(&look).unwrap();
    assert_eq!(model.parent_loops(&model.parents()).len(), 2);
    sheet(&look, &at("go", 1.0)).unwrap();
}

#[test]
fn a_drive_at_three_input_positions() {
    let mut look = square(json!({ "duration": 1, "tracks": [ { "target": "arm", "prop": "translateX", "values": [0, 0] } ] }));
    look["drives"] = json!([
        { "input": "turn", "target": "arm", "prop": "rotate", "values": [-30, 0, 30] },
        { "input": "pace", "target": ["arm"], "prop": "translateY", "values": [0, 10, 20], "at": [0, 0.8, 1] },
        { "input": "held", "target": "arm", "prop": "opacity", "values": [null, 0.5] },
        { "input": "lookX", "target": "arm", "prop": "scaleX", "values": [0.5, 1, 2] }
    ]);
    let pose = |turn: f64, pace: f64, held: f64, look_x: f64| Pose { animation: None, inputs: Inputs { turn, pace, held, look_x, ..Inputs::default() } };
    // turn -1 (position 0), 0 (0.5), 1 (1).
    for (turn, angle) in [(-1.0, -30.0), (0.0, 0.0), (1.0, 30.0), (0.5, 15.0)] {
        let (t, _, ids) = posed(&look, &pose(turn, 0.0, 0.0, 0.0));
        if angle == 0.0 {
            assert_matrix(t[ids["arm"]], IDENTITY);
        } else {
            assert_matrix(t[ids["arm"]], about(50.0, 40.0, rotate(angle)));
        }
    }
    // pace 0.4 is halfway into the first segment (0 … 0.8); 0.9 halfway into the second.
    for (pace, y) in [(0.0, 0.0), (0.4, 5.0), (0.9, 15.0), (1.0, 20.0)] {
        let (t, _, ids) = posed(&look, &pose(0.0, pace, 0.0, 0.0));
        assert_matrix(t[ids["arm"]], translate(0.0, y));
    }
    // An absolute prop is set, null being the artwork's own (opacity 1); a scale multiplies.
    let (t, set, ids) = posed(&look, &pose(0.0, 0.0, 0.5, -1.0));
    assert_eq!(set["arm"], vec![("opacity", "0.75".to_string())]);
    assert_matrix(t[ids["arm"]], about(50.0, 40.0, scale(0.5, 1.0)));
    // The animation comes first, the drive is added to it.
    let mut both = look.clone();
    both["animations"]["move"]["tracks"] = json!([ { "target": "arm", "prop": "rotate", "values": [0, 10] } ]);
    let (t, _, ids) = posed(&both, &Pose { animation: Some(("move".into(), 1.0)), inputs: Inputs { turn: 1.0, ..Inputs::default() } });
    assert_matrix(t[ids["arm"]], about(50.0, 40.0, rotate(40.0)));
}

#[test]
fn the_mouth_and_the_gaze_come_last() {
    let look = json!({
        "viewBox": [0, 0, 200, 200],
        "nodes": [
            { "tag": "path", "id": "mouth", "attrs": { "d": "M 80 120 Q 100 125 120 120" }, "box": [80, 120, 40, 2.5] },
            { "tag": "ellipse", "id": "jaw", "attrs": { "cx": 100, "cy": 140, "rx": 20, "ry": 4 }, "box": [80, 136, 40, 8] },
            { "tag": "circle", "id": "pupils", "attrs": { "cx": 100, "cy": 80, "r": 3 }, "box": [97, 77, 6, 6] }
        ],
        "pivots": {}, "roles": { "pupils": "pupils" }, "animations": {}, "states": {}, "gaze": null,
        "mouth": [
            { "target": "mouth", "open": "M 80 120 Q 100 145 120 120" },
            { "target": "jaw", "prop": "ry", "open": 12 },
            { "target": "pupils", "prop": "scaleY", "open": 2, "closed": 1 }
        ]
    });
    let speaking = Pose { animation: None, inputs: Inputs { mouth: Some(0.5), look_x: 1.0, look_y: 0.0, ..Inputs::default() } };
    let (t, set, ids) = posed(&look, &speaking);
    assert_eq!(set["mouth"], vec![("d", "M 80 120 Q 100 135 120 120".to_string())]);
    assert_eq!(set["jaw"], vec![("ry", "8".to_string())]);
    // The gaze: 3% of the width (6) to the right, after the mouth's scale about the box's centre.
    assert_matrix(t[ids["pupils"]], mul(translate(6.0, 0.0), about(100.0, 80.0, scale(1.0, 1.5))));
    // Without a line the mouth entries are not applied; a vector longer than 1 is cut to 1.
    let looking = Pose { animation: None, inputs: Inputs { look_x: 1.0, look_y: 1.0, ..Inputs::default() } };
    let (t, set, ids) = posed(&look, &looking);
    assert!(set.is_empty());
    let k = 6.0 / 2f64.sqrt();
    assert_matrix(t[ids["pupils"]], translate(k, k));
    // gaze 0 switches it off; a larger one is held to 5% of the width.
    let mut still = look.clone();
    still["gaze"] = json!(0);
    assert_eq!(posed(&still, &looking).0[ids["pupils"]], None);
    still["gaze"] = json!(50);
    assert_matrix(posed(&still, &Pose { animation: None, inputs: Inputs { look_x: -1.0, ..Inputs::default() } }).0[ids["pupils"]], translate(-10.0, 0.0));
}

#[test]
fn colour_and_path_keyframes_with_nulls() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [
            { "tag": "g", "attrs": { "fill": "#ff0000" }, "children": [
                { "tag": "circle", "id": "inherits", "attrs": { "r": 5 }, "box": [-5, -5, 10, 10] }
            ] },
            { "tag": "circle", "id": "none", "attrs": { "r": 5, "fill": "none" }, "box": [-5, -5, 10, 10] },
            { "tag": "path", "id": "p", "attrs": { "d": "M 0 0 L 10 0" }, "box": [0, 0, 10, 0] }
        ],
        "pivots": {}, "roles": {}, "states": {}, "mouth": [], "gaze": null,
        "animations": { "a": { "duration": 1, "tracks": [
            { "target": ["inherits", "none"], "prop": "fill", "values": [null, "#0000ff"] },
            { "target": "p", "prop": "d", "values": [null, "M 0 10 L 10 20"] }
        ] } }
    });
    let (_, set, _) = posed(&look, &at("a", 0.5));
    // From the inherited red; from nothing, the blue fades in.
    assert_eq!(set["inherits"], vec![("fill", "#800080".to_string())]);
    assert_eq!(set["none"], vec![("fill", "#0000ff80".to_string())]);
    assert_eq!(set["p"], vec![("d", "M 0 5 L 10 10".to_string())]);
}

#[test]
fn the_eases_at_known_points() {
    let e = |name: &str, t: f64| Ease::of(&json!(name)).unwrap().at(t);
    let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(near(e("linear", 0.25), 0.25));
    assert!(near(e("in", 0.5), 0.25));
    assert!(near(e("out", 0.5), 0.75));
    assert!(near(e("inOut", 0.25), 0.125) && near(e("inOut", 0.75), 0.875));
    assert!(near(e("outBack", 0.5), 1.0876975), "{}", e("outBack", 0.5));
    assert!(near(e("step", 0.5), 0.0) && near(e("step", 1.0), 1.0));
    // c3·t³ − c1·t² with c1 = 1.70158, c3 = c1 + 1.
    assert!(near(e("inBack", 0.5), -0.0876975), "{}", e("inBack", 0.5));
    // c2 = c1·1.525.
    assert!(near(e("inOutBack", 0.25), -0.09968184375), "{}", e("inOutBack", 0.25));
    assert!(near(e("inOutBack", 0.75), 1.09968184375), "{}", e("inOutBack", 0.75));
    // 2^(−5)·sin(4.25·2π/3) + 1 = 1 + 0.5/32.
    assert!(near(e("outElastic", 0.5), 1.015625), "{}", e("outElastic", 0.5));
    assert!(near(e("outElastic", 0.0), 0.0) && near(e("outElastic", 1.0), 1.0));
    // Each branch of outBounce.
    assert!(near(e("outBounce", 0.2), 0.3025));
    assert!(near(e("outBounce", 0.5), 0.765625));
    assert!(near(e("outBounce", 0.8), 0.94));
    assert!(near(e("outBounce", 0.95), 0.98453125));
    assert!(near(e("outBounce", 1.0), 1.0));
    // A bezier: CSS's `ease` at one half, by Newton; one whose slope is flat at the start by bisection.
    let bezier = |p: [f64; 4], t: f64| Ease::of(&json!(p)).unwrap().at(t);
    assert!((bezier([0.25, 0.1, 0.25, 1.0], 0.5) - 0.8024033877399112).abs() < 1e-6, "{}", bezier([0.25, 0.1, 0.25, 1.0], 0.5));
    assert!(near(bezier([0.0, 0.0, 1.0, 1.0], 0.3), 0.3) || (bezier([0.0, 0.0, 1.0, 1.0], 0.3) - 0.3).abs() < 1e-6);
    assert!(near(bezier([0.25, 0.1, 0.25, 1.0], 0.0), 0.0) && near(bezier([0.25, 0.1, 0.25, 1.0], 1.0), 1.0));
    // x(s) = s³ here, so s = ∛t and y = 1 − (1 − s)³.
    let t: f64 = 0.0005;
    let s = t.cbrt();
    assert!((bezier([0.0, 1.0, 0.0, 1.0], t) - (1.0 - (1.0 - s).powi(3))).abs() < 1e-4, "{}", bezier([0.0, 1.0, 0.0, 1.0], t));
    // Out of bounds, or not four numbers: not an ease.
    assert_eq!(Ease::of(&json!([1.5, 0, 0.5, 1])), None);
    assert_eq!(Ease::of(&json!([0.5, 4, 0.5, 1])), None);
    assert_eq!(Ease::of(&json!([0.5, 1, 0.5])), None);
    assert_eq!(Ease::of(&json!("bounce")), None);
    // Where an ease stands: one for all, four numbers for all, or one per segment.
    assert_eq!(eases(&[Some(&json!([0.1, 0.2, 0.3, 0.4]))], 3), vec![Ease::Bezier([0.1, 0.2, 0.3, 0.4]); 3]);
    assert_eq!(eases(&[Some(&json!(["in", [0.1, 0.2, 0.3, 0.4]]))], 2), vec![Ease::In, Ease::Bezier([0.1, 0.2, 0.3, 0.4])]);
    // A curve that starts flat: Newton would leave 0 … 1 there, bisection lands (x = s³, so
    // s = 0.1 and y = 0.028), and no value leaves the way between the curve's ends.
    assert!((Ease::Bezier([0.0, 0.0, 0.0, 1.0]).at(0.001) - 0.028).abs() < 1e-6);
    for curve in [[0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 1.0, 1.0], [0.0, 1.0, 0.0, 1.0], [1.0, 1.0, 1.0, 0.0]] {
        for t in [0.0, 0.001, 0.01, 0.5, 0.99, 0.999, 1.0] {
            let v = Ease::Bezier(curve).at(t);
            assert!((-1e-6..=1.0 + 1e-6).contains(&v), "{curve:?} at {t}: {v}");
        }
    }
    // A list that does not fit falls through to the animation's, then to linear.
    assert_eq!(eases(&[Some(&json!(["in"])), Some(&json!("out"))], 2), vec![Ease::Out, Ease::Out]);
    assert_eq!(eases(&[None, None], 2), vec![Ease::Linear, Ease::Linear]);
}

// ---------------------------------------------------------------- pictures

fn decode(png: &[u8]) -> Pixmap {
    Pixmap::decode_png(png).expect("a PNG")
}

/// Pixels of a rectangle that differ from `colour`.
fn drawn(p: &Pixmap, x: u32, y: u32, w: u32, h: u32, colour: [u8; 3]) -> usize {
    let mut n = 0;
    for yy in y..y + h {
        for xx in x..x + w {
            let c = p.pixel(xx, yy).unwrap();
            if [c.red(), c.green(), c.blue()] != colour {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_sheet_has_both_panels_and_the_small_views() {
    let sheet = decode(&sheet(&pip(), &Pose::default()).unwrap());
    assert_eq!((sheet.width(), sheet.height()), (532, 332));
    let big = SHEET_FIGURE_PX;
    let row = big + GAP_PX;
    for (x, theme) in [(0, LIGHT), (big + GAP_PX, DARK)] {
        // The panel's own colour in its corner, the figure in its middle.
        let corner = sheet.pixel(x + 1, 1).unwrap();
        assert_eq!([corner.red(), corner.green(), corner.blue()], theme.panel);
        let figure = drawn(&sheet, x, 0, big, big, theme.panel);
        assert!(figure > 15_000, "{figure} pixels drawn on a panel");
        // Not cut off: the panel's edges are its own colour.
        assert_eq!(drawn(&sheet, x, 0, big, 1, theme.panel) + drawn(&sheet, x, 0, 1, big, theme.panel), 0);
        // The small figure and the face.
        let left = x + (big - SMALL_FIGURE_PX - SMALL_GAP_PX - FACE_PX) / 2;
        assert!(drawn(&sheet, left, row + GAP_PX, SMALL_FIGURE_PX, SMALL_FIGURE_PX, theme.panel) > 500);
        assert!(drawn(&sheet, left + SMALL_FIGURE_PX + SMALL_GAP_PX, row + GAP_PX + 8, FACE_PX, FACE_PX, theme.panel) > 500);
    }
    // Between the panels: the gap's colour.
    let gap = sheet.pixel(big + 1, 10).unwrap();
    assert_eq!([gap.red(), gap.green(), gap.blue()], GAP_COLOUR);
}

#[test]
fn current_colour_is_each_panels_text_colour() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [ { "tag": "rect", "attrs": { "x": 0, "y": 0, "width": 100, "height": 100, "fill": "currentColor" } } ],
        "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null
    });
    let sheet = decode(&sheet(&look, &Pose::default()).unwrap());
    let rgb = |x: u32, y: u32| {
        let c = sheet.pixel(x, y).unwrap();
        format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
    };
    assert_eq!(rgb(100, 100), LIGHT.fg);
    assert_eq!(rgb(SHEET_FIGURE_PX + GAP_PX + 100, 100), DARK.fg);
}

#[test]
fn a_strip_has_its_frames_and_they_differ() {
    let png = strip(&pip(), "hop", 4).unwrap();
    let p = decode(&png);
    assert_eq!((p.width(), p.height()), (4 * STRIP_FRAME_PX + 3 * GAP_PX, STRIP_FRAME_PX));
    let frame = |i: u32| -> Vec<u8> {
        let x0 = i * (STRIP_FRAME_PX + GAP_PX);
        (0..STRIP_FRAME_PX).flat_map(|y| (x0..x0 + STRIP_FRAME_PX).map(move |x| (x, y))).flat_map(|(x, y)| {
            let c = p.pixel(x, y).unwrap();
            [c.red(), c.green(), c.blue()]
        }).collect()
    };
    let frames: Vec<Vec<u8>> = (0..4).map(frame).collect();
    // A one-shot: its first and last frames are its start and end (both at rest), the middle ones differ.
    assert_ne!(frames[1], frames[0]);
    assert_ne!(frames[2], frames[1]);
    for f in &frames {
        assert!(f.chunks(3).filter(|c| *c != LIGHT.panel).count() > 3000);
    }
    for frames in [1, 9] {
        assert_eq!(strip(&pip(), "hop", frames).unwrap_err(), "a strip has 2 to 8 frames");
    }
}

#[test]
fn the_example_looks_render() {
    // What the validator stores is what the renderer reads: turned, the third example shows its drives.
    let facing = decode(&sheet(&hoot(), &Pose::default()).unwrap());
    let turned = decode(&sheet(&hoot(), &Pose { animation: None, inputs: Inputs { turn: 1.0, pace: 1.0, ..Inputs::default() } }).unwrap());
    assert_ne!(facing.data(), turned.data(), "a turn and a pace show");
    for look in [pip(), tock(), hoot()] {
        let rest = decode(&sheet(&look, &Pose::default()).unwrap());
        let speaking = decode(&sheet(&look, &Pose { animation: None, inputs: Inputs { mouth: Some(1.0), look_x: 1.0, ..Inputs::default() } }).unwrap());
        assert_ne!(rest.data(), speaking.data(), "an open mouth and a look aside show");
        let first = look["animations"].as_object().unwrap().keys().next().unwrap().clone();
        let p = decode(&strip(&look, &first, STRIP_FRAMES_DEFAULT).unwrap());
        assert_eq!(p.width(), 6 * STRIP_FRAME_PX + 5 * GAP_PX);
        for name in look["animations"].as_object().unwrap().keys() {
            for progress in [0.0, 0.3, 0.7, 1.0] {
                sheet(&look, &at(name, progress)).unwrap();
            }
        }
    }
}

#[test]
fn the_face_is_the_square_around_the_head() {
    let look = pip();
    let model = Model::read(&look).unwrap();
    let face = model.face_view();
    let head = model.root_box(model.role("head").unwrap()).unwrap();
    assert!((face[2] - head[2].max(head[3]) * 1.2).abs() < 0.01 || face[2] == 200.0, "{face:?} {head:?}");
    assert_eq!(face[2], face[3]);
    assert!(face[0] >= 0.0 && face[1] >= 0.0 && face[0] + face[2] <= 200.0 && face[1] + face[3] <= 200.0, "{face:?}");
    // Without a head: the whole figure.
    let look = gadget();
    let model = Model::read(&look).unwrap();
    let mut headless = gadget();
    headless["roles"] = json!({});
    assert_eq!(Model::read(&headless).unwrap().face_view(), [0.0, 0.0, 200.0, 200.0]);
    // With one: 56 × 1.2 = 67.2, centred on (100, 70).
    assert_eq!(model.face_view(), [66.4, 36.4, 67.2, 67.2]);
}

#[test]
fn the_newer_format_shows_in_the_picture() {
    let look = gadget();
    let rest = decode(&sheet(&look, &Pose::default()).unwrap());
    let turned = decode(&sheet(&look, &Pose { animation: None, inputs: Inputs { turn: 1.0, ..Inputs::default() } }).unwrap());
    assert_ne!(rest.data(), turned.data());
    // The bulb follows the head (a parent) as the body leans: at rest its glow is over the cord at
    // x = 100; turned to the right it is moved right.
    let row: Vec<u32> = (0..SHEET_FIGURE_PX).filter(|x| {
        let c = turned.pixel(*x, (20.0 * 1.32) as u32).unwrap();
        c.red() > 200 && c.green() < 190 && c.blue() < 120
    }).collect();
    assert!(!row.is_empty() && row.iter().sum::<u32>() / row.len() as u32 > 140, "{row:?}");
    // The drop shadow darkens the panel below and right of the torso.
    let c = rest.pixel(((140.0 + 3.0) * 1.32) as u32, (130.0 * 1.32) as u32).unwrap();
    assert!(c.red() < LIGHT.panel[0] - 20, "{c:?}");
    // The mask cuts stripes into the torso.
    let c = rest.pixel((100.0 * 1.32) as u32, (123.0 * 1.32) as u32).unwrap();
    assert!(c.blue() < 245 && c.red() > 150, "a stripe shows the panel, darkened only by the shadow: {c:?}");
    let c = rest.pixel((100.0 * 1.32) as u32, (135.0 * 1.32) as u32).unwrap();
    assert_eq!([c.red(), c.green(), c.blue()], [0x4d, 0xab, 0xf7]);
}

#[test]
fn filters_beyond_the_budget_are_refused_with_a_sentence() {
    let mut nodes: Vec<Value> = vec![json!({ "tag": "defs", "children": [
        { "tag": "filter", "id": "heavy", "attrs": { "filterUnits": "userSpaceOnUse", "x": -200, "y": -200, "width": 600, "height": 600 }, "children":
            (0..8).map(|_| json!({ "tag": "feTurbulence", "attrs": { "baseFrequency": 0.05, "numOctaves": 4 } })).collect::<Vec<_>>() }
    ] })];
    nodes.extend((0..20).map(|i| json!({ "tag": "circle", "attrs": { "cx": i * 10, "cy": 50, "r": 4, "filterRef": "heavy" } })));
    let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes });
    let started = std::time::Instant::now();
    let err = sheet(&look, &Pose::default()).unwrap_err();
    assert!(err.starts_with("this look's filters cost too much to draw (about "), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "refused before drawing");
    assert!(err.ends_with(", at most 60): give fewer parts a filter, keep filter regions close to their part, or use fewer and smaller primitives"), "{err}");
    assert_eq!(affordable(&look).unwrap_err(), err, "a design call is told the same before the look is stored");
    // One element with two of those primitives in a region near it is well within it.
    let mut one = look.clone();
    one["nodes"].as_array_mut().unwrap().truncate(2);
    one["nodes"][0]["children"][0]["attrs"] = json!({});
    one["nodes"][0]["children"][0]["children"].as_array_mut().unwrap().truncate(2);
    sheet(&one, &Pose::default()).unwrap();
    affordable(&one).unwrap();
}

/// resvg draws a filtered group at the scale of its transform alone and turns the finished layer:
/// squeezed one way and stretched the other, or skewed both ways, that layer would be larger than
/// memory. Such a look is refused before anything is allocated.
#[test]
fn a_filter_on_a_part_stretched_out_of_shape_is_refused_before_anything_is_drawn() {
    let look = |transform: &str| {
        json!({ "viewBox": [0, 0, 200, 200], "nodes": [
            { "tag": "defs", "children": [ { "tag": "filter", "id": "f", "attrs": { "filterUnits": "userSpaceOnUse", "x": -400, "y": -400, "width": 800, "height": 800 }, "children": [ { "tag": "feOffset", "attrs": { "dx": 1, "dy": 1 } } ] } ] },
            { "tag": "g", "id": "g", "attrs": { "transform": transform, "filterRef": "f" }, "children": [ { "tag": "circle", "attrs": { "cx": 0, "cy": 0, "r": 0.01, "fill": "#3366cc" } } ] }
        ] })
    };
    for transform in ["translate(100 100) scale(1000 0.001) rotate(45)", "translate(100 100) scale(100 0.01) rotate(45)", "translate(100 100) skewX(80) skewY(80)"] {
        let started = std::time::Instant::now();
        for err in [sheet(&look(transform), &Pose::default()).unwrap_err(), affordable(&look(transform)).unwrap_err()] {
            assert!(err.starts_with("a filter sits on a part that is stretched or skewed too far to draw"), "{transform}: {err}");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{transform}: refused before drawing");
    }
    // The same filter on a part that is only turned and scaled evenly is drawn.
    let mut plain = look("translate(100 100) rotate(45) scale(2)");
    plain["nodes"][0]["children"][0]["attrs"] = json!({});
    plain["nodes"][1]["children"][0]["attrs"]["r"] = json!(20);
    sheet(&plain, &Pose::default()).unwrap();
}

/// A layer per masked, clipped or half-transparent group: a few hundred of them take seconds.
#[test]
fn hundreds_of_masked_groups_are_refused_with_a_sentence() {
    let masked = |count: usize| {
        let mut nodes: Vec<Value> = vec![json!({ "tag": "defs", "children": [ { "tag": "mask", "id": "m", "children": [ { "tag": "rect", "attrs": { "x": 0, "y": 0, "width": 200, "height": 200, "fill": "#ffffff" } } ] } ] })];
        nodes.extend((0..count).map(|_| json!({ "tag": "rect", "attrs": { "x": 10, "y": 10, "width": 180, "height": 180, "fill": "#3366cc", "maskRef": "m" } })));
        json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes })
    };
    let started = std::time::Instant::now();
    let err = affordable(&masked(200)).unwrap_err();
    assert!(err.starts_with("this look has too many masked, clipped, filtered or half-transparent groups to draw (about ") && err.contains(", at most 60): mask or clip one group"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "refused before drawing");
    sheet(&masked(6), &Pose::default()).unwrap();
}

/// A character no installed font has is left out; no other font file is searched for it.
#[test]
fn a_character_the_font_lacks_costs_no_search() {
    let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": (0..6).map(|i| json!({ "tag": "text", "attrs": { "x": 10, "y": 30 + i * 28, "fontSize": 20, "fill": "#000000" }, "text": "\u{12000}\u{13000}\u{1D800}\u{16A0}\u{A000}" })).collect::<Vec<_>>() });
    sheet(&look, &Pose::default()).unwrap();
    let started = std::time::Instant::now();
    sheet(&look, &Pose::default()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn dashes_too_fine_to_draw_are_refused_with_a_sentence() {
    let dashed = |count: usize, pattern: &str| {
        let nodes: Vec<Value> = (0..count).map(|_| json!({ "tag": "path", "attrs": { "d": "M 0 0 L 200 200 L 0 200 L 200 0", "fill": "none", "stroke": "#000000", "strokeDasharray": pattern } })).collect();
        json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes })
    };
    let started = std::time::Instant::now();
    let err = sheet(&dashed(300, "0.01"), &Pose::default()).unwrap_err();
    assert!(err.starts_with("this look's dashes are too fine to draw (about "), "{err}");
    assert!(err.ends_with(" dashes, at most 100000): make the dashes longer or dash fewer strokes"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "refused before drawing");
    // What a look draws is far within it: 765 units of a "4 2" pattern are 128 dashes.
    sheet(&dashed(20, "4 2"), &Pose::default()).unwrap();
    let one = dashed(1, "4 2");
    let model = Model::read(&one).unwrap();
    let svg = document(&model.body(&model.pose(&Pose::default()).unwrap()), model.view, 264, LIGHT.fg);
    let cost = Cost::of(&usvg::Tree::from_str(&svg, &options(false)).unwrap(), 264);
    assert!((cost.dashes - (200.0 + 400.0 * 2f64.sqrt()) / 6.0).abs() < 0.5, "{cost:?}");
}

#[test]
fn text_is_drawn_with_a_system_font() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [ { "tag": "text", "id": "t", "attrs": { "x": 50, "y": 70, "fontSize": 60, "fontWeight": "bold", "textAnchor": "middle", "fill": "currentColor" }, "text": "Hi" } ],
        "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null
    });
    let p = decode(&sheet(&look, &Pose::default()).unwrap());
    if fonts().faces().next().is_none() {
        // A system without fonts draws no text, and still a picture.
        return;
    }
    assert!(drawn(&p, 0, 0, SHEET_FIGURE_PX, SHEET_FIGURE_PX, LIGHT.panel) > 2000);
    assert!(drawn(&p, SHEET_FIGURE_PX + GAP_PX, 0, SHEET_FIGURE_PX, SHEET_FIGURE_PX, DARK.panel) > 2000);
}

// ---------------------------------------------------------------- refusals

#[test]
fn an_animation_that_is_not_the_looks_own_is_named_in_a_sentence() {
    assert_eq!(
        sheet(&pip(), &at("walk", 0.5)).unwrap_err(),
        "no animation \"walk\" in this look: its own animations are alert, breathe, cheer, doze, hop, ponder, sway, talk, wave"
    );
    // pip has a breathe of its own; blink is Workbench's.
    assert_eq!(strip(&pip(), "blink", 6).unwrap_err(), "blink is one of Workbench's built-in animations, which are not drawn here: define your own to see it");
    sheet(&pip(), &at("breathe", 0.5)).unwrap();
    let mut bare = pip();
    bare["animations"] = json!({});
    assert_eq!(strip(&bare, "walk", 6).unwrap_err(), "no animation \"walk\" in this look: it has no animations of its own");
    assert_eq!(strip(&bare, "x\"\n", 6).unwrap_err(), "no animation \"x\\\"\\n\" in this look: it has no animations of its own");
    assert_eq!(sheet(&Value::Null, &Pose::default()).unwrap_err(), "there is no look of its own to draw: this avatar shows Workbench's default look");
}

#[test]
fn a_pose_as_a_call_writes_it() {
    assert_eq!(Pose::from_json(&json!({})).unwrap(), Pose::default());
    assert_eq!(
        Pose::from_json(&json!({ "animation": "hop", "lookX": -1, "mouth": 0.4, "turn": 0.5, "pace": 1, "held": 0 })).unwrap(),
        Pose { animation: Some(("hop".into(), 0.5)), inputs: Inputs { look_x: -1.0, look_y: 0.0, mouth: Some(0.4), pace: 1.0, turn: 0.5, held: 0.0 } }
    );
    assert_eq!(Pose::from_json(&json!({ "animation": "hop", "at": 1 })).unwrap().animation, Some(("hop".into(), 1.0)));
    assert_eq!(Pose::from_json(&json!({ "lookX": 2 })).unwrap_err(), "lookX in a pose is a number from -1 to 1");
    assert_eq!(Pose::from_json(&json!({ "mouth": "open" })).unwrap_err(), "mouth in a pose is a number from 0 to 1");
    assert_eq!(Pose::from_json(&json!({ "at": 0.5 })).unwrap_err(), "at in a pose says how far into its animation: name the animation too");
    assert_eq!(Pose::from_json(&json!({ "frame": 2 })).unwrap_err(), "a pose takes animation, at, lookX, lookY, mouth, pace, turn and held, not \"frame\"");
    assert_eq!(Pose::from_json(&json!({ "animation": 3 })).unwrap_err(), "animation in a pose is the name of one of the look's own animations");
}

// ---------------------------------------------------------------- looking at the pictures

/// Writes the pictures of the example looks and of the newer format to `AVATAR_PREVIEW_OUT`, and
/// prints how long each took: `AVATAR_PREVIEW_OUT=/some/dir cargo test avatar::preview -- --ignored --nocapture`.
#[test]
#[ignore]
fn write_the_pictures() {
    let dir = std::path::PathBuf::from(std::env::var("AVATAR_PREVIEW_OUT").expect("AVATAR_PREVIEW_OUT"));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, png: Result<Vec<u8>, String>| {
        let started = std::time::Instant::now();
        let png = png.unwrap();
        std::fs::write(dir.join(format!("{name}.png")), &png).unwrap();
        eprintln!("{name}: {} bytes, written in {:?}", png.len(), started.elapsed());
    };
    let timed = |f: &dyn Fn() -> Result<Vec<u8>, String>| {
        let started = std::time::Instant::now();
        let out = f();
        eprintln!("  drawn in {:?}", started.elapsed());
        out
    };
    write("pip-rest", timed(&|| sheet(&pip(), &Pose::default())));
    write("pip-rest-again", timed(&|| sheet(&pip(), &Pose::default())));
    write("pip-hop", timed(&|| strip(&pip(), "hop", 6)));
    write("pip-speaking", timed(&|| sheet(&pip(), &Pose { animation: None, inputs: Inputs { mouth: Some(1.0), look_x: 1.0, ..Inputs::default() } })));
    write("tock-rest", timed(&|| sheet(&tock(), &Pose::default())));
    write("tock-flip", timed(&|| strip(&tock(), "flip", 6)));
    write("tock-speaking", timed(&|| sheet(&tock(), &Pose { animation: None, inputs: Inputs { mouth: Some(1.0), look_x: 1.0, ..Inputs::default() } })));
    write("hoot-rest", timed(&|| sheet(&hoot(), &Pose::default())));
    write("hoot-turned", timed(&|| sheet(&hoot(), &Pose { animation: None, inputs: Inputs { turn: 1.0, pace: 1.0, look_x: 1.0, ..Inputs::default() } })));
    write("hoot-walk", timed(&|| strip(&hoot(), "walk", 6)));
    write("gadget-rest", timed(&|| sheet(&gadget(), &Pose::default())));
    write("gadget-turned", timed(&|| sheet(&gadget(), &Pose { animation: None, inputs: Inputs { turn: 1.0, ..Inputs::default() } })));
    write("gadget-pop", timed(&|| strip(&gadget(), "pop", 5)));
}

/// Random looks with filters, masks, clips and opacity on parts under random transforms (stretched,
/// squeezed, skewed, turned, nested): whatever passes the cost check is then drawn. Run it under
/// a memory limit, so that a layer the check did not foresee ends the run instead of the machine:
/// `(ulimit -v 4000000; cargo test avatar::preview::tests::whatever_the_check -- --ignored --nocapture)`.
#[test]
#[ignore]
fn whatever_the_check_lets_through_is_drawn_within_bounds() {
    let mut seed: u64 = std::env::var("AVATAR_PREVIEW_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x9E3779B97F4A7C15);
    let mut pick = move |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    let rounds: usize = std::env::var("AVATAR_PREVIEW_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(1500);
    let scales = ["1", "2", "0.5", "10", "0.1", "100", "0.01", "1000", "0.001", "-1", "-30"];
    let (mut drawn, mut refused, mut slowest) = (0, 0, std::time::Duration::ZERO);
    for round in 0..rounds {
        let mut transform = |pick: &mut dyn FnMut(usize) -> usize| {
            let mut ops = Vec::new();
            for _ in 0..pick(4) {
                ops.push(match pick(5) {
                    0 => format!("scale({} {})", scales[pick(scales.len())], scales[pick(scales.len())]),
                    1 => format!("rotate({})", [15, 45, 90, 137, 180][pick(5)]),
                    2 => format!("skewX({})", [10, 45, 80, -80][pick(4)]),
                    3 => format!("skewY({})", [10, 45, 80, -80][pick(4)]),
                    _ => format!("translate({} {})", [0, 50, 100, 400, -400][pick(5)], [0, 50, 100, 400, -400][pick(5)]),
                });
            }
            ops.join(" ")
        };
        let primitive = |pick: &mut dyn FnMut(usize) -> usize| match pick(7) {
            0 => json!({ "tag": "feGaussianBlur", "attrs": { "stdDeviation": (["1", "5", "50"][pick(3)]) } }),
            1 => json!({ "tag": "feDropShadow", "attrs": { "dx": 2, "dy": 2, "stdDeviation": "2" } }),
            2 => json!({ "tag": "feOffset", "attrs": { "dx": 1, "dy": 1 } }),
            3 => json!({ "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": (["1", "5", "20"][pick(3)]) } }),
            4 => json!({ "tag": "feTurbulence", "attrs": { "baseFrequency": "0.05", "numOctaves": 1 + pick(4) } }),
            5 => json!({ "tag": "feFlood", "attrs": { "floodColor": "#ff0000", "floodOpacity": 0.5 } }),
            _ => json!({ "tag": "feColorMatrix", "attrs": { "type": "saturate", "values": "0.3" } }),
        };
        let region = |pick: &mut dyn FnMut(usize) -> usize| match pick(4) {
            0 => json!({}),
            1 => json!({ "filterUnits": "userSpaceOnUse", "x": -400, "y": -400, "width": 800, "height": 800 }),
            2 => json!({ "x": "-500%", "y": "-500%", "width": "1100%", "height": "1100%" }),
            _ => json!({ "filterUnits": "userSpaceOnUse", "x": 0, "y": 0, "width": 200, "height": 200 }),
        };
        let filter_attrs = region(&mut pick);
        let primitives: Vec<Value> = (0..1 + pick(3)).map(|_| primitive(&mut pick)).collect();
        let mut nodes = vec![json!({ "tag": "defs", "children": [
            { "tag": "filter", "id": "f", "attrs": filter_attrs, "children": primitives },
            { "tag": "mask", "id": "m", "children": [ { "tag": "rect", "attrs": { "x": 0, "y": 0, "width": 200, "height": 200, "fill": "#ffffff" } } ] },
            { "tag": "clipPath", "id": "c", "children": [ { "tag": "circle", "attrs": { "cx": 100, "cy": 100, "r": 80 } } ] }
        ] })];
        // A chain of groups, each with a transform and perhaps an effect, around one shape.
        let mut inner = json!({ "tag": "circle", "attrs": { "cx": ([0, 100][pick(2)]), "cy": ([0, 100][pick(2)]), "r": ([0.01, 5.0, 60.0][pick(3)]), "fill": "#3366cc" } });
        for depth in 0..1 + pick(4) {
            let mut attrs = Map::new();
            let t = transform(&mut pick);
            if !t.is_empty() {
                attrs.insert("transform".into(), json!(t));
            }
            match pick(6) {
                0 | 1 => drop(attrs.insert("filterRef".into(), json!("f"))),
                2 => drop(attrs.insert("maskRef".into(), json!("m"))),
                3 => drop(attrs.insert("clipPathRef".into(), json!("c"))),
                4 => drop(attrs.insert("opacity".into(), json!(0.5))),
                _ => {}
            }
            inner = json!({ "tag": "g", "id": format!("g{depth}"), "attrs": attrs, "children": [inner] });
        }
        nodes.push(inner);
        let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes });
        let started = std::time::Instant::now();
        match affordable(&look) {
            Err(_) => refused += 1,
            Ok(()) => {
                sheet(&look, &Pose::default()).unwrap_or_else(|why| panic!("round {round}: the check let through what a sheet refuses: {why}\n{look}"));
                drawn += 1;
                let took = started.elapsed();
                if took > slowest {
                    slowest = took;
                    eprintln!("round {round}: {took:?}");
                }
                assert!(took < std::time::Duration::from_secs(5), "round {round} took {took:?}:\n{look}");
            }
        }
    }
    eprintln!("{drawn} drawn, {refused} refused, the slowest sheet took {slowest:?}");
    assert!(drawn > rounds / 10 && refused > rounds / 10, "{drawn} drawn, {refused} refused");
}

/// How long dashes take to draw, against how many there are (`MAX_DASHES_DRAWN`):
/// `cargo test avatar::preview::tests::calibrate -- --ignored --nocapture`.
#[test]
#[ignore]
fn calibrate_dashes() {
    for (circles, dash) in [(10, 1.0), (100, 1.0), (100, 0.1), (300, 0.05)] {
        let nodes: Vec<Value> = (0..circles).map(|i| json!({ "tag": "circle", "attrs": { "cx": 100, "cy": 100, "r": 20 + (i % 80), "fill": "none", "stroke": "#000000", "strokeDasharray": format!("{dash} {dash}") } })).collect();
        let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes });
        let model = Model::read(&look).unwrap();
        let svg = document(&model.body(&model.pose(&Pose::default()).unwrap()), model.view, 264, LIGHT.fg);
        let tree = usvg::Tree::from_str(&svg, &options(false)).unwrap();
        let mut pixmap = Pixmap::new(264, 264).unwrap();
        let started = std::time::Instant::now();
        resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
        let dashes: f64 = (0..circles).map(|i| 2.0 * std::f64::consts::PI * (20 + (i % 80)) as f64 / (2.0 * dash)).sum();
        eprintln!("{circles} circles, dash {dash}: {dashes:.0} dashes, {:?}", started.elapsed());
    }
}

/// How long each primitive takes against the work `primitive_cost` estimates for it
/// (`FILTER_WORK`): `cargo test avatar::preview::tests::calibrate -- --ignored --nocapture`.
#[test]
#[ignore]
fn calibrate_filter_work() {
    let cases: Vec<(&str, Value)> = vec![
        ("blur", json!({ "tag": "feGaussianBlur", "attrs": { "stdDeviation": 6 } })),
        ("shadow", json!({ "tag": "feDropShadow", "attrs": { "dx": 3, "dy": 3, "stdDeviation": 4 } })),
        ("turb4", json!({ "tag": "feTurbulence", "attrs": { "baseFrequency": 0.05, "numOctaves": 4 } })),
        ("turb1", json!({ "tag": "feTurbulence", "attrs": { "baseFrequency": 0.05, "numOctaves": 1 } })),
        ("matrix", json!({ "tag": "feColorMatrix", "attrs": { "type": "saturate", "values": 0.3 } })),
        ("morph20", json!({ "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": 20 } })),
        ("morph3", json!({ "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": 3 } })),
        ("displace", json!({ "tag": "feDisplacementMap", "attrs": { "in2": "SourceGraphic", "scale": 10 } })),
    ];
    for (name, primitive) in cases {
        let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": [
            { "tag": "defs", "children": [ { "tag": "filter", "id": "f", "attrs": { "filterUnits": "userSpaceOnUse", "x": 0, "y": 0, "width": 200, "height": 200 }, "children": [primitive] } ] },
            { "tag": "rect", "attrs": { "x": 20, "y": 20, "width": 160, "height": 160, "fill": "#ff0000", "filterRef": "f" } }
        ] });
        let model = Model::read(&look).unwrap();
        let svg = document(&model.body(&model.pose(&Pose::default()).unwrap()), model.view, 264, LIGHT.fg);
        let tree = usvg::Tree::from_str(&svg, &options(false)).unwrap();
        let work = Cost::of(&tree, 264).filters;
        let mut pixmap = Pixmap::new(264, 264).unwrap();
        let started = std::time::Instant::now();
        for _ in 0..5 {
            resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
        }
        let per = started.elapsed() / 5;
        eprintln!("{name}: work {:.1} panels, {per:?} per render, {:.2} ms per panel of work", work / (264.0 * 264.0), per.as_secs_f64() * 1000.0 / (work / (264.0 * 264.0)));
    }
}

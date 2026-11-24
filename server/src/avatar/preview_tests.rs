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
    Pose { animation: Some((name.to_string(), Some(progress))), inputs: Inputs::default(), zoom: None }
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
         <text x='1' font-size='16' font-family='sans-serif'>&lt;/text&gt;&lt;script&gt;alert('x')&lt;/script&gt;</text><path/><rect height='10'/>\
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
    // delayed track has not started; at 0.5 (1 s) the first is at its last keyframe, and it
    // holds it to the end of the run, as a gesture does in the browser.
    let look = square(json!({ "duration": 1, "tracks": [
        { "target": "arm", "prop": "translateX", "values": [0, 10] },
        { "target": "arm", "prop": "translateY", "values": [0, 10], "delay": 1 }
    ] }));
    let (t, _, ids) = posed(&look, &at("move", 0.25));
    assert_matrix(t[ids["arm"]], translate(5.0, 0.0));
    let (t, _, ids) = posed(&look, &at("move", 0.5));
    assert_matrix(t[ids["arm"]], translate(10.0, 0.0));
    let (t, _, ids) = posed(&look, &at("move", 0.75));
    assert_matrix(t[ids["arm"]], translate(10.0, 5.0));
    let (t, _, ids) = posed(&look, &at("move", 1.0));
    assert_matrix(t[ids["arm"]], translate(10.0, 10.0));
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

/// A part follows its parent as if it were drawn inside it: what moves a group that holds them
/// both moves the part once, not twice, and a mirror of that group does not undo itself.
#[test]
fn a_part_and_its_parent_inside_one_moving_group_move_once() {
    let look = json!({
        "viewBox": [0, 0, 200, 200],
        "nodes": [ { "tag": "g", "id": "grp", "box": [60, 60, 50, 50], "children": [
            { "tag": "rect", "id": "child", "attrs": { "x": 60, "y": 60, "width": 20, "height": 20 }, "box": [60, 60, 20, 20] },
            { "tag": "rect", "id": "par", "attrs": { "x": 90, "y": 90, "width": 20, "height": 20 }, "box": [90, 90, 20, 20] }
        ] } ],
        "pivots": { "grp": [100, 100] }, "roles": {}, "states": {}, "mouth": [], "gaze": null, "animations": {},
        "parents": { "child": "par" },
        "drives": [
            { "input": "lookX", "target": "grp", "prop": "translateX", "values": [-40, 0, 40] },
            { "input": "turn", "target": "grp", "prop": "scaleX", "values": [-1, 1, 1] },
            { "input": "held", "target": "par", "prop": "translateY", "values": [0, 10] }
        ]
    });
    let with = |inputs: Inputs| posed(&look, &Pose { inputs, ..Pose::default() });
    // The group moves 40 to the right: the part goes along inside it, and is given nothing more.
    let (t, _, ids) = with(Inputs { look_x: 1.0, ..Inputs::default() });
    assert_matrix(t[ids["grp"]], translate(40.0, 0.0));
    assert_matrix(t[ids["child"]].or(Some(IDENTITY)), IDENTITY);
    // The group mirrored about x = 100: the part is mirrored with it, once.
    let (t, _, ids) = with(Inputs { turn: -1.0, ..Inputs::default() });
    assert_matrix(t[ids["grp"]], [-1.0, 0.0, 0.0, 1.0, 200.0, 0.0]);
    assert_matrix(t[ids["child"]].or(Some(IDENTITY)), IDENTITY);
    // The parent's own step is the part's too, also inside the mirrored and moved group.
    let (t, _, ids) = with(Inputs { held: 1.0, ..Inputs::default() });
    assert_matrix(t[ids["child"]], translate(0.0, 10.0));
    let (t, _, ids) = with(Inputs { held: 1.0, turn: -1.0, look_x: 1.0, ..Inputs::default() });
    assert_matrix(t[ids["child"]], translate(0.0, 10.0));
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
    let pose = |turn: f64, pace: f64, held: f64, look_x: f64| Pose { animation: None, inputs: Inputs { turn, pace, held, look_x, ..Inputs::default() }, zoom: None };
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
    let (t, _, ids) = posed(&both, &Pose { animation: Some(("move".into(), Some(1.0))), inputs: Inputs { turn: 1.0, ..Inputs::default() }, zoom: None });
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
    let speaking = Pose { animation: None, inputs: Inputs { mouth: Some(0.5), look_x: 1.0, look_y: 0.0, ..Inputs::default() }, zoom: None };
    let (t, set, ids) = posed(&look, &speaking);
    assert_eq!(set["mouth"], vec![("d", "M 80 120 Q 100 135 120 120".to_string())]);
    assert_eq!(set["jaw"], vec![("ry", "8".to_string())]);
    // The gaze: 3% of the width (6) to the right, after the mouth's scale about the box's centre.
    assert_matrix(t[ids["pupils"]], mul(translate(6.0, 0.0), about(100.0, 80.0, scale(1.0, 1.5))));
    // Without a line the mouth entries are not applied; a vector longer than 1 is cut to 1.
    let looking = Pose { animation: None, inputs: Inputs { look_x: 1.0, look_y: 1.0, ..Inputs::default() }, zoom: None };
    let (t, set, ids) = posed(&look, &looking);
    assert!(set.is_empty());
    let k = 6.0 / 2f64.sqrt();
    assert_matrix(t[ids["pupils"]], translate(k, k));
    // gaze 0 switches it off; a larger one is held to 5% of the width.
    let mut still = look.clone();
    still["gaze"] = json!(0);
    assert_eq!(posed(&still, &looking).0[ids["pupils"]], None);
    still["gaze"] = json!(50);
    assert_matrix(posed(&still, &Pose { animation: None, inputs: Inputs { look_x: -1.0, ..Inputs::default() }, zoom: None }).0[ids["pupils"]], translate(-10.0, 0.0));
}

#[test]
fn the_gaze_looks_where_it_is_asked_on_screen_whatever_the_pupils_are_drawn_in() {
    // Pupils of 3 at (100, 80), plainly or inside a group with a transform (`eyes` of gaze.test.ts).
    let eyes = |transform: Option<&str>| {
        let pupils = json!({ "tag": "circle", "id": "pupils", "attrs": { "cx": 100, "cy": 80, "r": 3 }, "box": [97, 77, 6, 6] });
        let node = match transform {
            Some(t) => json!({ "tag": "g", "attrs": { "transform": t }, "children": [pupils] }),
            None => pupils,
        };
        json!({ "viewBox": [0, 0, 200, 200], "nodes": [node], "pivots": {}, "roles": { "pupils": "pupils" }, "animations": {}, "states": {}, "mouth": [], "gaze": null })
    };
    let moved = |transform: Option<&str>, look_x: f64, look_y: f64| {
        let look = eyes(transform);
        let (t, _, ids) = posed(&look, &Pose { animation: None, inputs: Inputs { look_x, look_y, ..Inputs::default() }, zoom: None });
        t[ids["pupils"]]
    };
    // Plain, or in a group that only moves them: 3% of the width, as asked.
    assert_matrix(moved(None, 1.0, 0.0), translate(6.0, 0.0));
    assert_matrix(moved(Some("translate(10 5)"), 1.0, 0.0), translate(6.0, 0.0));
    // In a figure mirrored to face the other way their own x runs leftwards: -6 of it is 6 to the right on screen.
    assert_matrix(moved(Some("translate(200 0) scale(-1 1)"), 1.0, 0.0), translate(-6.0, 0.0));
    assert_matrix(moved(Some("translate(200 0) scale(-1 1)"), 0.0, 1.0), translate(0.0, 6.0));
    // Turned a quarter, their own -y points right on screen; drawn twice the size, half as far in their own units.
    assert_matrix(moved(Some("rotate(90 100 80)"), 1.0, 0.0), translate(0.0, -6.0));
    assert_matrix(moved(Some("translate(-100 -80) scale(2)"), 1.0, 0.0), translate(3.0, 0.0));
    // Squashed flat there is nothing to undo: the vector is used as it is.
    assert_matrix(moved(Some("scale(1 0)"), 1.0, 0.0), translate(6.0, 0.0));
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
    let turned = decode(&sheet(&hoot(), &Pose { animation: None, inputs: Inputs { turn: 1.0, pace: 1.0, ..Inputs::default() }, zoom: None }).unwrap());
    assert_ne!(facing.data(), turned.data(), "a turn and a pace show");
    // A strip holds the inputs it is given: the walk is seen in the view that shows it.
    let walking = draw_strip(&hoot(), "walk", 4, &Inputs { turn: 1.0, pace: 1.0, ..Inputs::default() }, None).unwrap();
    assert_ne!(walking.png, strip(&hoot(), "walk", 4).unwrap());
    // A closer look at one part is another picture of the same size; a part that is not there is said.
    let close = decode(&sheet(&hoot(), &Pose { zoom: Some("head".into()), ..Pose::default() }).unwrap());
    assert!(close.width() == facing.width() && close.data() != facing.data());
    assert_eq!(sheet(&hoot(), &Pose { zoom: Some("nose".into()), ..Pose::default() }).unwrap_err(), "zoom names \"nose\", which is no part of this look with a box: name the id of a part that is drawn");
    // What drawing it cost travels with the picture: the owl's shadow filter is worth a word.
    let spent = draw_sheet(&hoot(), &Pose::default()).unwrap().spent;
    assert!(spent.filters > 1.0 && spent.filters < FILTER_WORK && spent.layers > 0.0, "{spent:?}");
    for look in [pip(), tock(), hoot()] {
        let rest = decode(&sheet(&look, &Pose::default()).unwrap());
        let speaking = decode(&sheet(&look, &Pose { animation: None, inputs: Inputs { mouth: Some(1.0), look_x: 1.0, ..Inputs::default() }, zoom: None }).unwrap());
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

// ---------------------------------------------------------------- what the face leaves out

/// The directory of the example looks, and of the face looks under it (`examples/face/`).
fn examples_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("avatar").join("examples")
}

/// A design of `examples/face/` as the validator stores it.
fn face_look(name: &str) -> Value {
    let design = std::fs::read_to_string(examples_dir().join("face").join(format!("{name}.json"))).unwrap();
    stored(serde_json::from_str(&design).unwrap())
}

/// The ids of what the small face of a look leaves out, in document order.
fn left_out(look: &Value) -> Vec<&str> {
    let model = Model::read(look).unwrap();
    model.face_left_out().into_iter().map(|i| model.els[i].id.expect("only what has an id has a box")).collect()
}

/// The face fixture of a look (`<name>.face.json`): the face's square and the ids of what it
/// leaves out, in document order, as `lib/golden.test.ts` reads them.
fn face_fixture(look: &Value) -> String {
    let view = Model::read(look).unwrap().face_view();
    format!("{{\"viewBox\": {}, \"leftOut\": {}}}\n", serde_json::to_string(&view).unwrap(), serde_json::to_string(&left_out(look)).unwrap())
}

/// The names of the files of `dir`, sorted.
fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    files
}

/// Every `<name>.look.json` of `dir` by name, each with its `<name>.face.json` checked to be what
/// this renderer makes of it, or with `AVATAR_WRITE_FACE=1` written so. With `designs`, every
/// `<name>.json` of `dir` is a design whose `<name>.look.json` is checked (or written) to be what
/// the validator stores of it. No fixture outlives its look.
fn face_fixtures_in(dir: &std::path::Path, designs: bool) -> Vec<(String, Value)> {
    let write = std::env::var_os("AVATAR_WRITE_FACE").is_some_and(|v| v == "1");
    if designs {
        for name in files_in(dir).iter().filter_map(|f| f.strip_suffix(".json")).filter(|n| !n.ends_with(".look") && !n.ends_with(".face")) {
            let design: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap()).unwrap();
            let applied = crate::avatar::look::apply(None, &design).unwrap_or_else(|problems| panic!("{name} is refused: {problems:#?}"));
            let look = applied.design.look.expect("a look");
            let path = dir.join(format!("{name}.look.json"));
            if write {
                std::fs::write(&path, serde_json::to_string_pretty(&look).unwrap() + "\n").unwrap();
            }
            let stored = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing: write it with AVATAR_WRITE_FACE=1", path.display()));
            let stored: crate::avatar::look::Look = serde_json::from_str(&stored).unwrap_or_else(|e| panic!("{name}.look.json is not a look: {e}"));
            assert_eq!(stored, look, "{} is not what the validator stores of {name}.json: write it again with AVATAR_WRITE_FACE=1", path.display());
        }
    }
    let files = files_in(dir);
    let mut out = vec![];
    for name in files.iter().filter_map(|f| f.strip_suffix(".look.json")) {
        if designs {
            assert!(files.contains(&format!("{name}.json")), "{name}.look.json has no design");
        }
        let look: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.look.json"))).unwrap()).unwrap();
        let made = face_fixture(&look);
        let path = dir.join(format!("{name}.face.json"));
        if write {
            std::fs::write(&path, &made).unwrap();
        }
        let stored = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing: write it with AVATAR_WRITE_FACE=1", path.display()));
        assert_eq!(stored, made, "{} is not what the server makes of {name}: write it again with AVATAR_WRITE_FACE=1 and review the difference", path.display());
        out.push((name.to_string(), look));
    }
    for file in files.iter().filter_map(|f| f.strip_suffix(".face.json")) {
        assert!(out.iter().any(|(name, _)| name == file), "{file}.face.json has no look");
    }
    out
}

/// `examples/<name>.face.json` holds what the small face shows of `examples/<name>.look.json`:
/// its square and what it leaves out. `examples/face/` holds designs made for the face, each with
/// the look the validator stores of it and its face: a fox whose tail reaches into the square, a
/// ghost whose head is only the features on its body, a look without a head, `use`s, references,
/// parents and a sliver without an id, a flipped and turned figure, and an edge that rounds at a
/// half. `lib/golden.test.ts` makes the same faces with the browser's code and compares. After a
/// deliberate change, write them again with `AVATAR_WRITE_FACE=1 cargo test --locked
/// avatar::preview::tests::the_faces` and review the difference.
#[test]
fn the_faces_of_the_example_looks_are_in_their_fixture_files() {
    let examples = face_fixtures_in(&examples_dir(), false);
    let named: Vec<&str> = examples.iter().map(|(name, _)| name.as_str()).collect();
    for name in ["hoot", "pip", "tock"] {
        assert!(named.contains(&name), "{name} in {named:?}");
    }
    let faces = face_fixtures_in(&examples_dir().join("face"), true);
    assert_eq!(faces.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["fox", "ghost", "nested", "nohead", "refs", "ties"]);
    let of = |name: &str| examples.iter().chain(&faces).find(|(n, _)| n == name).map(|(_, look)| look).unwrap();
    // Tock keeps its hands and its core, which lie inside the square: only the jet at its lower
    // edge goes (and the shadow, which lies wholly outside it).
    assert_eq!(left_out(of("tock")), ["shadow", "jet"]);
    assert_eq!(left_out(of("hoot")), ["shadow", "feet"]);
    assert_eq!(left_out(of("pip")), ["shadow", "z1", "z2"]);
    // The tip of the fox's tail beside its head; the side view a drive hides at rest overlaps the head and stays.
    assert_eq!(left_out(of("fox")), ["tail"]);
    // The ghost's head is only its features: the body they sit on meets them and stays.
    assert_eq!(left_out(of("ghost")), ["bow", "wisp"]);
    // Without a head, the face is the whole viewBox and nothing is left out.
    assert!(left_out(of("nohead")).is_empty());
    assert_eq!(Model::read(of("nohead")).unwrap().face_view(), [0.0, 0.0, 120.0, 160.0]);
    // Boxes that touch meet; a part wholly inside the square stays; a part hung on the head
    // (`parents`) follows the same rule; a `use` is judged by its own box, whatever it repeats.
    assert_eq!(left_out(of("refs")), ["paw", "echo", "miss", "halo"]);
    // -30.0625 rounds to -30.062 as JavaScript rounds (towards +∞), and so touches the head.
    assert_eq!(left_out(of("ties")), ["past"]);
}

/// The light face panel of a sheet.
fn face_panel(sheet: &Pixmap) -> Vec<[u8; 3]> {
    let x = (SHEET_FIGURE_PX - SMALL_FIGURE_PX - SMALL_GAP_PX - FACE_PX) / 2 + SMALL_FIGURE_PX + SMALL_GAP_PX;
    let y = SHEET_FIGURE_PX + GAP_PX + GAP_PX + (SMALL_FIGURE_PX - FACE_PX) / 2;
    (y..y + FACE_PX).flat_map(|yy| (x..x + FACE_PX).map(move |xx| (xx, yy))).map(|(xx, yy)| sheet.pixel(xx, yy).map(|c| [c.red(), c.green(), c.blue()]).unwrap()).collect()
}

fn rgb_of(p: &Pixmap) -> Vec<[u8; 3]> {
    p.pixels().iter().map(|c| [c.red(), c.green(), c.blue()]).collect()
}

#[test]
fn the_face_in_a_picture_leaves_out_what_its_square_only_cuts() {
    for name in ["ghost", "fox"] {
        let look = face_look(name);
        let model = Model::read(&look).unwrap();
        let rest = model.pose(&Pose::default()).unwrap();
        let face = model.face_view();
        let whole = model.render(&model.body(&rest), face, FACE_PX, LIGHT).unwrap();
        let cut = model.render(&model.body_without(&rest, &model.face_left_out()), face, FACE_PX, LIGHT).unwrap();
        assert_ne!(rgb_of(&whole), rgb_of(&cut), "{name}: what is left out showed in the square");
        // The sheet's face is the face without them; the large views are the whole figure.
        let sheet = decode(&sheet(&look, &Pose::default()).unwrap());
        assert_eq!(face_panel(&sheet), rgb_of(&cut), "{name}");
    }
    // The ghost's pink bow reaches into the square beside its features: gone from the face. Its
    // body, which the features sit on, stays.
    let look = face_look("ghost");
    let model = Model::read(&look).unwrap();
    let rest = model.pose(&Pose::default()).unwrap();
    let pink = |p: &[[u8; 3]]| p.iter().filter(|c| c[0] > 180 && c[1] < 130 && c[2] > 80 && c[2] < 180).count();
    let sheet_colour = |p: &[[u8; 3]]| p.iter().filter(|c| **c == [0xe9, 0xec, 0xef]).count();
    let whole = rgb_of(&model.render(&model.body(&rest), model.face_view(), FACE_PX, LIGHT).unwrap());
    let face = face_panel(&decode(&sheet(&look, &Pose::default()).unwrap()));
    assert!(pink(&whole) > 0 && pink(&face) == 0, "{} pink before, {} after", pink(&whole), pink(&face));
    assert!(sheet_colour(&face) > 200, "the body behind the features: {}", sheet_colour(&face));
    // Nothing is left out of a look without a head: its face is the whole figure.
    let look = face_look("nohead");
    let model = Model::read(&look).unwrap();
    let rest = model.pose(&Pose::default()).unwrap();
    assert_eq!(model.body_without(&rest, &model.face_left_out()), model.body(&rest));
}

#[test]
fn what_the_face_leaves_out_is_out_of_sight_where_a_use_and_a_reference_still_find_it() {
    let look = face_look("refs");
    let model = Model::read(&look).unwrap();
    let rest = model.pose(&Pose::default()).unwrap();
    let body = model.body_without(&rest, &model.face_left_out());
    // Each is written inside a defs of its own, with its ids: the paw holds a gradient, a clip, a
    // mask and a filter that parts the face keeps refer to.
    assert!(body.contains("<defs><g id='paw'>") && body.contains("<defs><use id='echo'") && body.contains("<defs><rect id='miss'") && body.contains("</defs></g></defs><use id='pawNear' href='#paw'"), "{body}");
    assert_eq!(body.matches("<defs>").count(), model.body(&rest).matches("<defs>").count() + 4, "{body}");
    // A part left out is posed as any other: the halo's drive moves it at rest, out of sight.
    assert!(body.contains("<defs><circle id='halo' transform='matrix(1 0 0 1 4 0)'"), "{body}");
    // The face's square is [64, 44, 72, 72] at 40 px: a pixel is 1.8 units across. Each point
    // below lies wholly inside what it samples.
    assert_eq!(model.face_view(), [64.0, 44.0, 72.0, 72.0]);
    let face = face_panel(&decode(&sheet(&look, &Pose::default()).unwrap()));
    let at = |x: f64, y: f64| face[((y - 44.0) / 1.8) as usize * 40 + ((x - 64.0) / 1.8) as usize];
    // The use beside the head still shows the paw it repeats, painted with the paw's own gradient.
    assert_eq!(at(120.0, 108.0), [0x70, 0x48, 0xe8]);
    // The badge is clipped by the paw's clip to its left half.
    assert_eq!(at(94.0, 112.0), [0x2f, 0x9e, 0x44]);
    assert_ne!(at(106.0, 112.0), [0x2f, 0x9e, 0x44]);
    // The nose is masked by the paw's mask: a hole in its middle, black around it.
    assert_ne!(at(100.0, 88.0), [0, 0, 0]);
    assert_eq!(at(100.0, 93.5), [0, 0, 0]);
    // The cheek is blurred by the paw's filter: softened, but there.
    assert!(face.iter().any(|c| c[0] > 0xf0 && c[1] < 0xb0 && c[1] > 0x80), "the cheek");
}

#[test]
fn the_newer_format_shows_in_the_picture() {
    let look = gadget();
    let rest = decode(&sheet(&look, &Pose::default()).unwrap());
    let turned = decode(&sheet(&look, &Pose { animation: None, inputs: Inputs { turn: 1.0, ..Inputs::default() }, zoom: None }).unwrap());
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

/// What the browsers draw and the renderer's parser would change: a saturation beyond 0 … 1 is
/// written as the matrix it stands for, a morphology with one radius of 0 works along the other.
#[test]
fn a_saturation_beyond_its_range_and_a_morphology_along_one_axis_are_written_as_the_browser_draws_them() {
    let filter = |primitive: Value| {
        let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": [
            { "tag": "defs", "children": [ { "tag": "filter", "id": "f", "children": [primitive] } ] },
            { "tag": "rect", "attrs": { "x": 20, "y": 20, "width": 160, "height": 160, "fill": "#cc3366", "filterRef": "f" } }
        ] });
        let model = Model::read(&look).unwrap();
        let body = model.body(&model.pose(&Pose::default()).unwrap());
        sheet(&look, &Pose::default()).unwrap();
        body
    };
    let within = filter(json!({ "tag": "feColorMatrix", "attrs": { "type": "saturate", "values": "0.3" } }));
    assert!(within.contains("<feColorMatrix type='saturate' values='0.3'/>"), "{within}");
    let beyond = filter(json!({ "tag": "feColorMatrix", "attrs": { "type": "saturate", "values": "2" } }));
    assert!(beyond.contains("<feColorMatrix type='matrix' values='1.787 -0.715 -0.072 0 0 -0.213 1.285 -0.072 0 0 -0.213 -0.715 1.928 0 0 0 0 0 1 0'/>"), "{beyond}");
    let below = filter(json!({ "tag": "feColorMatrix", "attrs": { "type": "saturate", "values": "-1" } }));
    assert!(below.contains("type='matrix' values='-0.574 1.43 0.144 0 0"), "{below}");
    let hue = filter(json!({ "tag": "feColorMatrix", "attrs": { "type": "hueRotate", "values": "400" } }));
    assert!(hue.contains("type='hueRotate' values='400'"), "{hue}");
    // Morphology: both radii in two passes, one radius alone in one, none at all leaves the input.
    let both = filter(json!({ "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": "3 2" } }));
    assert_eq!(both.matches("<feMorphology").count(), 2, "{both}");
    let across = filter(json!({ "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": "3 0" } }));
    assert!(across.matches("<feMorphology").count() == 1 && across.contains("radius='3 0.0001'") && !across.contains("feOffset"), "{across}");
    let down = filter(json!({ "tag": "feMorphology", "attrs": { "operator": "erode", "radius": "0 2" } }));
    assert!(down.contains("operator='erode' radius='0.0001 2'"), "{down}");
    let none = filter(json!({ "tag": "feMorphology", "attrs": { "radius": "0" } }));
    assert!(none.contains("<feOffset dx='0' dy='0'/>") && !none.contains("feMorphology"), "{none}");
}

/// An ease that overshoots takes no size below nothing.
#[test]
fn a_size_never_goes_below_nothing() {
    let look = json!({
        "viewBox": [0, 0, 100, 100],
        "nodes": [ { "tag": "circle", "id": "dot", "attrs": { "cx": 50, "cy": 50, "r": 10, "stroke": "#000000", "strokeWidth": 4 }, "box": [40, 40, 20, 20] } ],
        "pivots": {}, "roles": {}, "states": {}, "mouth": [], "gaze": null,
        "animations": { "shrink": { "duration": 1, "ease": [0.3, 3, 0.7, 3], "tracks": [
            { "target": "dot", "prop": "strokeWidth", "values": [4, 0] },
            { "target": "dot", "prop": "r", "values": [10, 0] },
            { "target": "dot", "prop": "strokeDashoffset", "values": [0, -5] }
        ] } }
    });
    let (_, set, _) = posed(&look, &at("shrink", 0.5));
    let value = |name: &str| set["dot"].iter().find(|(n, _)| *n == name).map(|(_, v)| v.parse::<f64>().unwrap()).unwrap();
    assert_eq!((value("stroke-width"), value("r")), (0.0, 0.0));
    // What may be negative overshoots freely.
    assert!(value("stroke-dashoffset") < -5.0, "{}", value("stroke-dashoffset"));
}

/// A head that is a dot does not make the face a microscope, and what a look draws far beyond
/// the picture is refused in a sentence instead of being stroked for minutes.
#[test]
fn a_face_is_never_a_microscope_and_strokes_far_beyond_the_picture_are_refused() {
    let look = json!({ "viewBox": [0, 0, 4096, 4096], "nodes": [
        { "tag": "path", "id": "body", "attrs": { "d": "M400 400 C 1200 3600 2800 3600 3600 400", "fill": "none", "stroke": "#000000", "strokeWidth": 40 }, "box": [400, 400, 3200, 2400] },
        { "tag": "circle", "id": "head", "attrs": { "cx": 2048, "cy": 1000, "r": 0.001, "fill": "#000000" }, "box": [2047.999, 999.999, 0.002, 0.002] }
    ], "pivots": {}, "roles": { "body": "body", "head": "head" }, "animations": {}, "states": {}, "mouth": [], "gaze": null });
    let model = Model::read(&look).unwrap();
    assert_eq!(model.face_view()[2], 409.6, "a tenth of the viewBox at the least");
    let started = std::time::Instant::now();
    sheet(&look, &Pose::default()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());

    // A curve inside a group scaled a million times over: its stroke would be cut without end.
    let far = json!({ "viewBox": [0, 0, 16, 16], "nodes": [ { "tag": "g", "id": "g", "attrs": { "transform": "scale(1000) scale(1000) translate(-0.5 -0.5)" }, "children": [
        { "tag": "path", "attrs": { "d": "M0 0 C 0.2 0.9 0.8 0.9 1 0", "fill": "none", "stroke": "#000000", "strokeWidth": 0.01 } }
    ] } ] });
    let started = std::time::Instant::now();
    let err = sheet(&far, &Pose::default()).unwrap_err();
    assert!(err.starts_with("a stroked shape or a text reaches about ") && err.ends_with("draw it at the size of the figure"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "refused before drawing");
    // Scales that multiply beyond what can be written at all are said, not left out.
    let beyond = json!({ "viewBox": [0, 0, 200, 200], "nodes": [ { "tag": "rect", "id": "r", "attrs": { "transform": "scale(1000) scale(1000) scale(1000) translate(-0.001 -0.001)", "x": 0, "y": 0, "width": 0.001, "height": 0.001, "fill": "#000000" } } ] });
    let err = sheet(&beyond, &Pose::default()).unwrap_err();
    assert!(err.starts_with("a part is scaled beyond what can be drawn"), "{err}");
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
fn an_animation_that_the_look_does_not_have_is_named_in_a_sentence() {
    // Its own animations, then the built-in ones its roles give it that it did not replace.
    assert_eq!(
        sheet(&pip(), &at("walk", 0.5)).unwrap_err(),
        "no animation \"walk\" in this look: its own animations are alert, breathe, cheer, doze, hop, ponder, sway, talk, wave; Workbench's built-in ones for its roles are blink, think, listen, nod, shake"
    );
    let mut bare = pip();
    bare["animations"] = json!({});
    assert_eq!(strip(&bare, "walk", 6).unwrap_err(), "no animation \"walk\" in this look: it has no animations of its own; Workbench's built-in ones for its roles are breathe, blink, think, talk, listen, alert, doze, wave, cheer, nod, shake");
    bare["roles"] = json!({});
    assert_eq!(strip(&bare, "x\"\n", 6).unwrap_err(), "no animation \"x\\\"\\n\" in this look: it has no animations of its own, and its roles give it none of Workbench's built-in ones");
    // A built-in that its roles do not give it says what it needs.
    assert_eq!(strip(&bare, "wave", 6).unwrap_err(), "wave is Workbench's built-in motion and needs the armR or the armL role, which this look does not name");
    let mut armless = pip();
    armless["roles"] = json!({ "body": "frog", "head": "head" });
    assert_eq!(sheet(&armless, &at("blink", 0.5)).unwrap_err(), "blink is Workbench's built-in motion and needs the eyes role, which this look does not name");
    assert_eq!(sheet(&Value::Null, &Pose::default()).unwrap_err(), "there is no look of its own to draw: this avatar shows Workbench's default look");
}

#[test]
fn pupils_that_keep_still_are_refused_think_for_their_gaze_and_not_for_a_role() {
    // The look names the pupils, with gaze 0 and no head: what it lacks is a gaze above 0.
    let still = stored(json!({ "svg": "<svg viewBox='0 0 200 200'><g id='pupils'><circle cx='90' cy='90' r='4'/></g></svg>", "roles": { "pupils": "pupils" }, "gaze": 0 }));
    let words = "think is Workbench's built-in motion and needs the head role, or the pupils role and a gaze above 0, and this look's gaze is 0";
    assert_eq!(sheet(&still, &at("think", 0.5)).unwrap_err(), words);
    assert_eq!(strip(&still, "think", 6).unwrap_err(), words);
    // Without the pupils it lacks a role, as every other built-in does.
    let mut bare = still.clone();
    bare["roles"] = json!({});
    assert_eq!(strip(&bare, "think", 6).unwrap_err(), "think is Workbench's built-in motion and needs the head role, or the pupils role and a gaze above 0, which this look does not name");
}

#[test]
fn a_pose_and_a_strip_show_workbenchs_built_in_motion_by_name() {
    // pip blinks and nods Workbench's way: each is drawn as the browser makes it from the roles.
    let (transforms, _, ids) = posed(&pip(), &at("nod", 0.25));
    // At a quarter of the nod (the end of its first segment of four) the head has dipped by 6.
    assert_matrix(transforms[ids["head"]], [1.0, 0.0, 0.0, 1.0, 0.0, 6.0]);
    let (transforms, _, ids) = posed(&pip(), &at("blink", 0.5));
    let eyes = transforms[ids["eyes"]].expect("the eyes close");
    assert!((eyes[3] - 0.1).abs() < 1e-9, "{eyes:?}");
    // A strip of a built-in one-shot and of a built-in loop: frames that differ.
    let picture = decode(&strip(&pip(), "nod", 3).unwrap());
    assert_eq!((picture.width(), picture.height()), (3 * STRIP_FRAME_PX + 2 * GAP_PX, STRIP_FRAME_PX));
    for name in ["blink", "think", "listen", "shake"] {
        assert!(strip(&pip(), name, 4).is_ok(), "{name}");
    }
    // tock and hoot have every role: every built-in is drawn for each.
    for look in [tock(), hoot()] {
        for name in BUILTIN {
            assert!(sheet(&look, &at(name, 0.4)).is_ok(), "{name}");
        }
    }
}

#[test]
fn the_looks_own_animation_of_a_built_ins_name_is_the_one_drawn() {
    // pip waves its own way: the built-in wave turns armR about its joint, pip's own does not.
    let own = |name: &str| pip()["animations"][name]["tracks"].as_array().unwrap().iter().map(|t| t["target"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let built = builtin::animations(&pip()).into_iter().find(|(n, _)| *n == "wave").unwrap().1;
    assert_ne!(built["tracks"].as_array().unwrap().iter().map(|t| t["target"].as_str().unwrap().to_string()).collect::<Vec<_>>(), own("wave"));
    let mut without = pip();
    without["animations"].as_object_mut().unwrap().remove("wave");
    let mid = at("wave", 0.3);
    assert_ne!(posed(&pip(), &mid).0, posed(&without, &mid).0, "the own wave and the built-in one are drawn differently");
    // With its own taken away the built-in one is drawn, and the own one is drawn as written.
    let mut only = pip();
    only["animations"] = json!({ "wave": { "duration": 1, "tracks": [{ "target": "head", "prop": "translateX", "values": [0, 10] }] } });
    let (transforms, _, ids) = posed(&only, &at("wave", 1.0));
    assert_matrix(transforms[ids["head"]], [1.0, 0.0, 0.0, 1.0, 10.0, 0.0]);
    assert!(transforms[ids["armR"]].is_none(), "the built-in wave plays nowhere");
}

#[test]
fn a_pose_as_a_call_writes_it() {
    assert_eq!(Pose::from_json(&json!({})).unwrap(), Pose::default());
    assert_eq!(
        Pose::from_json(&json!({ "animation": "hop", "lookX": -1, "mouth": 0.4, "turn": 0.5, "pace": 1, "held": 0 })).unwrap(),
        Pose { animation: Some(("hop".into(), None)), inputs: Inputs { look_x: -1.0, look_y: 0.0, mouth: Some(0.4), pace: 1.0, turn: 0.5, held: 0.0 }, zoom: None }
    );
    assert_eq!(Pose::from_json(&json!({ "animation": "hop", "at": 1 })).unwrap().animation, Some(("hop".into(), Some(1.0))));
    assert_eq!(Pose::from_json(&json!({ "lookX": 2 })).unwrap_err(), "lookX in a pose is a number from -1 to 1");
    assert_eq!(Pose::from_json(&json!({ "mouth": "open" })).unwrap_err(), "mouth in a pose is a number from 0 to 1");
    assert_eq!(Pose::from_json(&json!({ "at": 0.5 })).unwrap_err(), "at in a pose says how far into its animation: name the animation too");
    assert_eq!(Pose::from_json(&json!({ "frame": 2 })).unwrap_err(), "a pose takes animation, at, lookX, lookY, mouth, pace, turn, held and zoom, not \"frame\"");
    assert_eq!(Pose::from_json(&json!({ "animation": 3 })).unwrap_err(), "animation in a pose is the name of one of the look's own animations or of a built-in one");
}

// ---------------------------------------------------------------- what a picture costs and shows

/// A design call as the validator stores it.
fn stored(design: Value) -> Value {
    let applied = crate::avatar::look::apply(None, &design).unwrap_or_else(|problems| panic!("refused: {problems:?}"));
    serde_json::to_value(applied.design.look.expect("a look")).unwrap()
}

/// 24 half-transparent strips in a group `hub` that `grow` scales tenfold about its edge, a dot
/// far from it and a body: near the budget for layers in the large view, and far within it at 56
/// pixels, where each layer's margin weighs more.
fn strips() -> Value {
    let groups: String = (0..24).map(|i| format!("<g opacity='0.5'><rect x='0' y='{i}' width='10' height='170' fill='#3366cc'/></g>")).collect();
    stored(json!({
        "name": "Strips",
        "svg": format!("<svg viewBox='0 0 200 200'><g id='hub'>{groups}</g><circle id='dot' cx='190' cy='100' r='3' fill='#cc3333'/><circle id='body' cx='100' cy='100' r='30' fill='#33aa55'/></svg>"),
        "pivots": { "hub": [10, 100] },
        "roles": { "body": "body" },
        "animations": { "grow": { "duration": 1, "tracks": [{ "target": "hub", "prop": "scaleX", "values": [1, 10] }, { "target": "hub", "prop": "scaleY", "values": [1, 10] }] } }
    }))
}

#[test]
fn the_small_panels_never_refuse_what_the_large_view_draws() {
    let look = strips();
    let grow = |at: f64, zoom: Option<&str>| Pose { animation: Some(("grow".into(), Some(at))), inputs: Inputs::default(), zoom: zoom.map(str::to_string) };
    // Near the budget the large view is drawn, and so are the small figure in the pose and the
    // face: a small panel costs fewer pixels than the large view of the same content.
    for at in [0.8, 0.85, 0.9, 0.95] {
        let picture = draw_sheet(&look, &grow(at, None)).unwrap_or_else(|why| panic!("{at}: {why}"));
        assert!(picture.spent.layers > 50.0 && picture.spent.layers <= LAYER_WORK, "{at}: {:?}", picture.spent);
        // What is reported is the figure in that pose in the large view, whatever the zoom.
        let close = draw_sheet(&look, &grow(at, Some("dot"))).unwrap_or_else(|why| panic!("{at}, close up: {why}"));
        assert_eq!((close.spent, close.rest), (picture.spent, picture.rest), "{at}");
    }
    // At 0.8 the large view costs 52 of 60, which is what is said (the small figure would have said 60).
    assert_eq!(draw_sheet(&look, &grow(0.8, None)).unwrap().spent.layers.ceil(), 52.0);
    // Past the budget the large view is refused for the look...
    let err = draw_sheet(&look, &grow(1.0, None)).unwrap_err();
    assert!(err.starts_with("this look has too many masked, clipped, filtered or half-transparent groups to draw (about 63 layers the size of the figure, at most 60)"), "{err}");
    // ...and a close-up far from what grows is drawn, and says what the pose costs: 63 of 60.
    let close = draw_sheet(&look, &grow(1.0, Some("dot"))).unwrap();
    assert!(close.spent.layers > LAYER_WORK && close.rest.layers < 5.0, "{close:?}");
    // The 132 px moments of a strip are drawn as far as the large view of each would be, and no
    // further: the strip of grow ends where the pose is refused, and is refused in its words.
    assert_eq!(draw_strip(&look, "grow", 8, &Inputs::default(), None).unwrap_err(), err);
    // Close up on the dot each moment is drawn, as the close-up pose is, and the strip says the
    // cost of its costliest moment.
    let strip = draw_strip(&look, "grow", 8, &Inputs::default(), Some("dot")).unwrap();
    assert!((strip.spent.layers - close.spent.layers).abs() < 1e-9 && strip.rest == close.rest, "{strip:?}");
}

#[test]
fn what_a_picture_costs_is_said_of_the_figure_in_its_pose_whatever_the_zoom() {
    // A look whose filter is on a part that does not move, and a square that turns.
    let mut look = square(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 90] }] }));
    look["nodes"] = json!([
        { "tag": "defs", "children": [{ "tag": "filter", "id": "soft", "children": [{ "tag": "feGaussianBlur", "attrs": { "stdDeviation": 2 } }] }] },
        { "tag": "rect", "id": "arm", "attrs": { "x": 40, "y": 40, "width": 20, "height": 20 }, "box": [40, 40, 20, 20] },
        { "tag": "circle", "id": "glow", "attrs": { "cx": 50, "cy": 80, "r": 12, "fill": "#3366cc", "filterRef": "soft" }, "box": [38, 68, 24, 24] }
    ]);
    let rest = draw_sheet(&look, &Pose::default()).unwrap();
    assert!(rest.spent.filters > 0.0 && rest.spent == rest.rest && rest.played.is_none(), "{rest:?}");
    // A pose that changes no cost reports what the rest reports; so does a close-up of it.
    for pose in [at("move", 1.0), Pose { zoom: Some("arm".into()), ..Pose::default() }, Pose { zoom: Some("glow".into()), ..at("move", 0.5) }] {
        let posed = draw_sheet(&look, &pose).unwrap();
        assert_eq!((posed.spent, posed.rest), (rest.spent, rest.spent), "{pose:?}");
        assert_ne!(posed.png, rest.png, "{pose:?}");
    }
    // gadget's turn leans its filtered body: it costs other than its rest, and the same with a
    // close-up on a part.
    let turned = Pose { inputs: Inputs { turn: 1.0, ..Inputs::default() }, ..Pose::default() };
    let whole = draw_sheet(&gadget(), &turned).unwrap();
    let close = draw_sheet(&gadget(), &Pose { zoom: Some("torso".into()), ..turned.clone() }).unwrap();
    assert!(whole.spent != whole.rest && (close.spent, close.rest) == (whole.spent, whole.rest), "{whole:?} {close:?}");
    // And what the rest reports is what a picture at rest reports.
    assert_eq!(whole.rest, draw_sheet(&gadget(), &Pose::default()).unwrap().spent);
}

#[test]
fn a_close_up_that_costs_too_much_says_it_is_the_close_up() {
    // Thirty half-transparent squares over the figure: a quarter of the budget in the large view,
    // ten times as large in a close-up of a dot.
    let mut nodes: Vec<Value> = (0..30).map(|_| json!({ "tag": "rect", "attrs": { "x": 10, "y": 10, "width": 180, "height": 180, "fill": "#3366cc", "opacity": 0.5 } })).collect();
    nodes.push(json!({ "tag": "circle", "id": "dot", "attrs": { "cx": 100, "cy": 100, "r": 2, "fill": "#000000" }, "box": [98, 98, 4, 4] }));
    let look = json!({ "viewBox": [0, 0, 200, 200], "nodes": nodes });
    draw_sheet(&look, &Pose::default()).unwrap();
    let close = Pose { zoom: Some("dot".into()), ..Pose::default() };
    let err = draw_sheet(&look, &close).unwrap_err();
    assert!(err.starts_with("the close-up of dot costs too much to draw (about ") && err.contains(" layers the size of the close-up, at most 60)"), "{err}");
    assert!(err.ends_with("look at a larger part, or at the whole figure without zoom"), "{err}");
    assert_eq!(draw_strip(&look, "none", 2, &Inputs::default(), Some("dot")).unwrap_err(), strip(&look, "none", 2).unwrap_err(), "the animation is looked for first");
}

#[test]
fn a_pose_without_a_moment_is_drawn_where_its_animation_has_moved_furthest() {
    let moment = |look: &Value, pose: &Pose| Model::read(look).unwrap().moment(pose).unwrap().1.and_then(|played| played.moment).unwrap();
    let open = |name: &str| Pose { animation: Some((name.to_string(), None)), ..Pose::default() };
    // Workbench's nod dips at a quarter and rests at the middle: without at it is drawn dipped.
    assert_eq!(moment(&pip(), &open("nod")), Moment { at: 0.25, chosen: Chosen::Furthest, still: false });
    let (transforms, _, ids) = posed(&pip(), &open("nod"));
    assert_matrix(transforms[ids["head"]], [1.0, 0.0, 0.0, 1.0, 0.0, 6.0]);
    // At a moment given where it rests, the picture is the figure without it, and says so.
    assert_eq!(moment(&pip(), &at("nod", 0.5)), Moment { at: 0.5, chosen: Chosen::Given, still: true });
    assert_eq!(moment(&pip(), &at("nod", 0.25)), Moment { at: 0.25, chosen: Chosen::Given, still: false });
    let nodded = draw_sheet(&pip(), &open("nod")).unwrap();
    assert_eq!(nodded.played, Some(Played { builtin: true, looping: false, moment: Some(Moment { at: 0.25, chosen: Chosen::Furthest, still: false }) }));
    assert_ne!(nodded.png, sheet(&pip(), &Pose::default()).unwrap());
    assert_eq!(draw_sheet(&pip(), &at("nod", 0.5)).unwrap().png, sheet(&pip(), &Pose::default()).unwrap(), "where it rests, the figure at rest");
    // A look's own: each track as a part of the furthest it goes, the earliest of the largest sums.
    let chosen = |animation: Value| moment(&square(animation), &open("move"));
    let furthest = |at: f64| Moment { at, chosen: Chosen::Furthest, still: false };
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 30, -60, 0], "times": [0, 0.25, 0.5, 1] }] })), furthest(0.5));
    // Ten units across and ninety degrees count alike: both reach 1 of their own furthest, the
    // first at the middle, the second at the end; the earlier wins.
    let two = json!({ "duration": 1, "tracks": [
        { "target": "arm", "prop": "translateX", "values": [0, 10, 0] },
        { "target": "arm", "prop": "rotate", "values": [0, 0, 90], "times": [0, 0.75, 1] }
    ] });
    assert_eq!(chosen(two), furthest(0.5));
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "scale", "values": [1, 1, 2] }] })), furthest(1.0));
    // A loop follows its phase; a one-shot's track waits for its delay and then holds its end.
    assert_eq!(chosen(json!({ "duration": 1, "loop": true, "tracks": [{ "target": "arm", "prop": "translateX", "values": [0, 10, 0], "phase": 0.25 }] })), furthest(0.25));
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "translateY", "values": [0, 10], "delay": 1 }] })), furthest(1.0));
    // An absolute number from the artwork's own (the square is 20 wide); a colour by how far its
    // channels have gone from the part's own (black: the square sets no fill).
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "width", "values": [20, 40, 30] }] })), furthest(0.5));
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "fill", "values": ["#000000", "#00ff00"] }] })), furthest(1.0));
    // An animation that never leaves its rest is drawn at its middle.
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "translateX", "values": [0, 0] }] })), Moment { at: 0.5, chosen: Chosen::Unmoved, still: true });
    assert_eq!(chosen(json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "width", "values": [30, 30] }] })), Moment { at: 0.5, chosen: Chosen::Unmoved, still: false });
}

#[test]
fn frames_close_up_show_each_moment_in_the_square_where_the_part_rests() {
    let frames = |zoom: Option<&str>| draw_strip(&pip(), "nod", 4, &Inputs::default(), zoom).unwrap();
    let (whole, close) = (frames(None), frames(Some("head")));
    let (a, b) = (decode(&whole.png), decode(&close.png));
    assert!((a.width(), a.height()) == (b.width(), b.height()) && a.data() != b.data());
    // The square stays where the head rests, so the nod shows against it: moments that differ.
    let frame = |p: &Pixmap, i: u32| -> Vec<u8> {
        let x0 = i * (STRIP_FRAME_PX + GAP_PX);
        (0..STRIP_FRAME_PX).flat_map(|y| (x0..x0 + STRIP_FRAME_PX).map(move |x| (x, y))).flat_map(|(x, y)| p.pixel(x, y).map(|c| [c.red(), c.green(), c.blue()]).unwrap()).collect()
    };
    assert_ne!(frame(&b, 0), frame(&b, 1));
    assert_eq!(frame(&b, 0), frame(&b, 3), "a nod ends where it starts");
    // What was drawn of the animation travels with it: Workbench's or the look's own, a loop or not.
    assert_eq!(close.played, Some(Played { builtin: true, looping: false, moment: None }));
    assert_eq!(draw_strip(&pip(), "hop", 2, &Inputs::default(), None).unwrap().played, Some(Played { builtin: false, looping: false, moment: None }));
    assert_eq!(draw_strip(&hoot(), "walk", 2, &Inputs::default(), Some("head")).unwrap().played, Some(Played { builtin: false, looping: true, moment: None }));
    assert_eq!(draw_strip(&pip(), "nod", 3, &Inputs::default(), Some("nose")).unwrap_err(), "zoom names \"nose\", which is no part of this look with a box: name the id of a part that is drawn");
}

/// The square's `arm` drawn as `nodes`, with `animation` as `move`.
fn drawn_as(nodes: Value, animation: Value) -> Value {
    let mut look = square(animation);
    look["nodes"] = nodes;
    look
}

/// A blue rect `arm`, 40 units wide.
fn blue() -> Value {
    json!([{ "tag": "rect", "id": "arm", "attrs": { "x": 30, "y": 30, "width": 40, "height": 40, "fill": "#3366cc" }, "box": [30, 30, 40, 40] }])
}

/// One track of `arm` as the animation `move`.
fn one_track(prop: &str, values: Value) -> Value {
    json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": prop, "values": values }] })
}

#[test]
fn a_pose_without_a_moment_is_drawn_where_it_is_furthest_from_what_its_parts_show_without_it() {
    let moment = |look: &Value| Model::read(look).unwrap().moment(&Pose { animation: Some(("move".into(), None)), ..Pose::default() }).unwrap().1.and_then(|played| played.moment).unwrap();
    let furthest = |at: f64| Moment { at, chosen: Chosen::Furthest, still: false };
    let on_blue = |prop: &str, values: Value| moment(&drawn_as(blue(), one_track(prop, values)));
    // A fade-in of a part drawn at opacity 1 is furthest at its start, where it is not there; a
    // fade-out at its end.
    assert_eq!(on_blue("opacity", json!([0, 1])), furthest(0.0));
    assert_eq!(on_blue("opacity", json!([1, 0])), furthest(1.0));
    assert_eq!(on_blue("opacity", json!([0.5, 1, 0.5])), furthest(0.0));
    // A number from the artwork's own: a rect 40 wide that grows from 10 is furthest at 10.
    assert_eq!(on_blue("width", json!([10, 40])), furthest(0.0));
    // A colour by how far its channels have gone from the part's own: a blush is furthest when
    // it is fully pink, half way, and a change of colour at its end.
    assert_eq!(on_blue("fill", json!(["#3366cc", "#ff3366", "#3366cc"])), furthest(0.5));
    assert_eq!(on_blue("fill", json!([null, "#ff3366", null])), furthest(0.5));
    assert_eq!(on_blue("fill", json!(["#3366cc", "#ff3366"])), furthest(1.0));
    let red = json!([{ "tag": "rect", "id": "arm", "attrs": { "x": 30, "y": 30, "width": 40, "height": 40, "fill": "#ff0000" }, "box": [30, 30, 40, 40] }]);
    assert_eq!(moment(&drawn_as(red, one_track("fill", json!(["#ff0000", "#00ff00"])))), furthest(1.0));
    // Path data by how far its numbers have gone: a mouth that opens and closes is open half way.
    let mouth = json!([{ "tag": "path", "id": "arm", "attrs": { "d": "M30 50 Q50 52 70 50", "fill": "none", "stroke": "#000000", "strokeWidth": 4 }, "box": [30, 50, 40, 2] }]);
    assert_eq!(moment(&drawn_as(mouth, one_track("d", json!(["M30 50 Q50 52 70 50", "M30 50 Q50 90 70 50", "M30 50 Q50 52 70 50"])))), furthest(0.5));

    // A spike between two of the even moments is found at its keyframe, not taken for stillness.
    let spike = json!({ "duration": 3, "tracks": [{ "target": "arm", "prop": "scaleY", "values": [1, 1, 0, 1, 1], "times": [0, 0.5, 0.52, 0.54, 1] }] });
    let m = moment(&drawn_as(blue(), spike));
    assert!(m.chosen == Chosen::Furthest && (m.at - 0.52).abs() < 1e-9, "{m:?}");
    // Keyframes are found through a loop's phase and a one-shot's delay.
    let looped = json!({ "duration": 1, "loop": true, "tracks": [{ "target": "arm", "prop": "translateX", "values": [0, 10, 0], "times": [0, 0.3, 1], "phase": 0.1 }] });
    assert!((moment(&drawn_as(blue(), looped)).at - 0.2).abs() < 1e-9);
    let delayed = json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "translateY", "values": [0, 10, 0], "times": [0, 0.4, 1], "delay": 1 }] });
    assert!((moment(&drawn_as(blue(), delayed)).at - 0.7).abs() < 1e-9);
    // A colour held from after its delay changes the part then: it has moved, from that moment.
    let late = json!({ "duration": 1, "tracks": [{ "target": "arm", "prop": "fill", "values": ["#ff0000", "#ff0000"], "delay": 1 }] });
    assert_eq!(moment(&drawn_as(blue(), late)), furthest(0.5));
    // Each part of a colour track counts from its own colour: one that the track paints blue
    // keeps its blue, the green one turns blue after the delay.
    let two = json!([
        { "tag": "rect", "id": "arm", "attrs": { "x": 10, "y": 30, "width": 30, "height": 40, "fill": "#3366cc" }, "box": [10, 30, 30, 40] },
        { "tag": "rect", "id": "leg", "attrs": { "x": 60, "y": 30, "width": 30, "height": 40, "fill": "#00ff00" }, "box": [60, 30, 30, 40] }
    ]);
    let both = json!({ "duration": 1, "tracks": [{ "target": ["arm", "leg"], "prop": "fill", "values": ["#3366cc", "#3366cc"], "delay": 1 }] });
    assert_eq!(moment(&drawn_as(two, both)), furthest(0.5));
    // Every moment looks the same only where no track changes what it sets.
    assert_eq!(on_blue("width", json!([30, 30])), Moment { at: 0.5, chosen: Chosen::Unmoved, still: false });
    assert_eq!(on_blue("translateX", json!([0, 0])), Moment { at: 0.5, chosen: Chosen::Unmoved, still: true });
}

#[test]
fn a_moment_drawn_as_the_figure_without_its_animation_is_said_to_rest_there() {
    let rest = |look: &Value| sheet(look, &Pose::default()).unwrap();
    let still = |look: &Value, pose: &Pose| draw_sheet(look, pose).unwrap().played.unwrap().moment.unwrap().still;
    // A track that writes the artwork's own opacity at its ends: written, the figure differs; drawn,
    // it is the figure at rest.
    let own = drawn_as(blue(), one_track("opacity", json!([1, 0.4, 1])));
    let model = Model::read(&own).unwrap();
    assert!(!model.moment(&at("move", 0.0)).unwrap().1.unwrap().moment.unwrap().still, "written, it differs");
    assert_eq!(draw_sheet(&own, &at("move", 0.0)).unwrap().png, rest(&own));
    assert!(still(&own, &at("move", 0.0)) && still(&own, &at("move", 1.0)) && !still(&own, &at("move", 0.5)));
    // So with a close-up: the figure is compared whole.
    assert!(still(&own, &Pose { zoom: Some("arm".into()), ..at("move", 0.0) }));
    assert!(!still(&own, &Pose { zoom: Some("arm".into()), ..at("move", 0.5) }));
    // A part out of sight at rest moves unseen; a part in sight does not.
    let hidden = json!([
        { "tag": "rect", "id": "arm", "attrs": { "x": 30, "y": 30, "width": 40, "height": 40, "fill": "#3366cc" }, "box": [30, 30, 40, 40] },
        { "tag": "circle", "id": "ghost", "attrs": { "cx": 50, "cy": 80, "r": 8, "fill": "#cc3366", "opacity": 0 }, "box": [42, 72, 16, 16] }
    ]);
    let unseen = json!({ "duration": 1, "tracks": [{ "target": "ghost", "prop": "translateX", "values": [0, 20] }] });
    assert!(still(&drawn_as(hidden.clone(), unseen), &at("move", 1.0)));
    let seen = json!({ "duration": 1, "tracks": [{ "target": ["ghost", "arm"], "prop": "translateX", "values": [0, 20] }] });
    assert!(!still(&drawn_as(hidden, seen), &at("move", 1.0)));
    // An example: pip's hop rests at its start and its end, which it writes out.
    assert!(still(&pip(), &at("hop", 0.0)) && still(&pip(), &at("hop", 1.0)) && !still(&pip(), &at("hop", 0.5)));
}

/// Six rects, each dilated by a radius of 20 units, that `grow` scales from 1 to `to`: the cost
/// of morphology grows with its radius in pixels, so a 132 px moment costs far less than the
/// large view of it.
fn dilated(to: f64) -> Value {
    let mut nodes = vec![json!({ "tag": "defs", "children": [{ "tag": "filter", "id": "f", "attrs": { "x": "-50%", "y": "-50%", "width": "200%", "height": "200%" }, "children": [{ "tag": "feMorphology", "attrs": { "operator": "dilate", "radius": 20 } }] }] })];
    let ids: Vec<String> = (0..6).map(|i| format!("r{i}")).collect();
    for (i, id) in ids.iter().enumerate() {
        let (x, y) = (20 + 60 * (i % 3), 40 + 80 * (i / 3));
        nodes.push(json!({ "tag": "rect", "id": id, "attrs": { "x": x, "y": y, "width": 20, "height": 20, "fill": "#3366cc", "filterRef": "f" }, "box": [x, y, 20, 20] }));
    }
    json!({
        "viewBox": [0, 0, 200, 200], "nodes": nodes, "pivots": {}, "roles": {}, "states": {}, "mouth": [], "gaze": null,
        "animations": { "grow": { "duration": 1, "tracks": [{ "target": ids, "prop": "scale", "values": [1, to] }] } }
    })
}

#[test]
fn a_strip_draws_a_moment_where_its_pose_is_drawn_and_refuses_it_in_the_same_words() {
    // Grown fourfold the large view is refused; so is a strip that ends there, in the same words
    // and numbers, although its 132 px moment would cost the drawing a sixteenth of that.
    let look = dilated(4.0);
    let end = draw_sheet(&look, &at("grow", 1.0)).unwrap_err();
    assert!(end.starts_with("this look's filters cost too much to draw (about 413 passes over the whole figure, at most 60)"), "{end}");
    assert_eq!(draw_strip(&look, "grow", 2, &Inputs::default(), None).unwrap_err(), end);
    // A longer strip is refused at its first moment the pose of which is refused, in its words.
    let first = (0..8).find_map(|i| draw_sheet(&look, &at("grow", f64::from(i) / 7.0)).err()).unwrap();
    assert_eq!(draw_strip(&look, "grow", 8, &Inputs::default(), None).unwrap_err(), first);
    // Grown twofold it is drawn, and the strip reports its costliest moment, its end, in those
    // units: what the pose of that moment reports, within the budget.
    let (strip, end) = (draw_strip(&dilated(2.0), "grow", 8, &Inputs::default(), None).unwrap(), draw_sheet(&dilated(2.0), &at("grow", 1.0)).unwrap());
    assert!(strip.spent == end.spent && strip.rest == end.rest && strip.spent.filters > 20.0 && strip.spent.filters <= FILTER_WORK, "{strip:?} {end:?}");
    // Close up, a moment is refused as the close-up of that pose is, and in its words.
    let close = draw_sheet(&dilated(2.0), &Pose { zoom: Some("r0".into()), ..at("grow", 0.0) }).unwrap_err();
    assert!(close.starts_with("the close-up of r0 costs too much to draw (about "), "{close}");
    assert_eq!(draw_strip(&dilated(2.0), "grow", 8, &Inputs::default(), Some("r0")).unwrap_err(), close);
    // A close-up drawn past the budget says the number a refusal of the whole figure says, and
    // what it means.
    let look = strips();
    let refused = draw_strip(&look, "grow", 8, &Inputs::default(), None).unwrap_err();
    let drawn = draw_strip(&look, "grow", 8, &Inputs::default(), Some("dot")).unwrap();
    let strip = crate::avatar::session::PictureRequest::Strip { animation: "grow".into(), frames: 8, inputs: Inputs::default(), zoom: Some("dot".into()) };
    let line = strip.cost(&drawn).unwrap();
    assert!(refused.contains("(about 63 layers the size of the figure, at most 60)"), "{refused}");
    assert!(line.contains(" and 63 of 60 for layers; ") && line.ends_with(" Past 60 only a close-up of it can be drawn."), "{line}");
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
    write("pip-speaking", timed(&|| sheet(&pip(), &Pose { animation: None, inputs: Inputs { mouth: Some(1.0), look_x: 1.0, ..Inputs::default() }, zoom: None })));
    write("tock-rest", timed(&|| sheet(&tock(), &Pose::default())));
    write("tock-flip", timed(&|| strip(&tock(), "flip", 6)));
    write("tock-speaking", timed(&|| sheet(&tock(), &Pose { animation: None, inputs: Inputs { mouth: Some(1.0), look_x: 1.0, ..Inputs::default() }, zoom: None })));
    write("hoot-rest", timed(&|| sheet(&hoot(), &Pose::default())));
    write("hoot-turned", timed(&|| sheet(&hoot(), &Pose { animation: None, inputs: Inputs { turn: 1.0, pace: 1.0, look_x: 1.0, ..Inputs::default() }, zoom: None })));
    write("hoot-walk", timed(&|| strip(&hoot(), "walk", 6)));
    write("gadget-rest", timed(&|| sheet(&gadget(), &Pose::default())));
    write("gadget-turned", timed(&|| sheet(&gadget(), &Pose { animation: None, inputs: Inputs { turn: 1.0, ..Inputs::default() }, zoom: None })));
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
        let transform = |pick: &mut dyn FnMut(usize) -> usize| {
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

// ---------------------------------------------------------------- what a look costs while it moves

/// The reviewer's look of the pre-release check, as a design: eight small circles, each with a
/// filter of eight primitives, each inside three nested groups that one loop scales tenfold at
/// its middle (`churn` the first five, `churn2` the rest, both in the idle state). Cheap at rest,
/// millions of passes where the scales coincide.
fn nested_heavy() -> Value {
    let filter = |i: usize| {
        format!(
            "<filter id='f{i}' x='-50%' y='-50%' width='200%' height='200%'><feTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' seed='{i}' result='t'/><feMorphology in='SourceGraphic' operator='dilate' radius='20' result='m'/><feDisplacementMap in='m' in2='t' scale='50' xChannelSelector='R' yChannelSelector='G' result='d'/><feGaussianBlur in='d' stdDeviation='50' result='b'/><feMorphology in='b' operator='erode' radius='20' result='e'/><feDropShadow in='e' dx='3' dy='3' stdDeviation='50' flood-color='#ff00ff' result='s'/><feComposite in='s' in2='t' operator='arithmetic' k1='1' k2='1' k3='1' k4='0' result='c'/><feBlend in='c' in2='SourceGraphic' mode='difference'/></filter>"
        )
    };
    let defs: String = (0..8).map(filter).collect();
    let parts: String = (0..8).map(|i| format!("<g id='a{i}'><g id='b{i}'><g id='c{i}'><circle id='p{i}' cx='{}' cy='100' r='3' fill='#3366ff' filter='url(#f{i})'/></g></g></g>", 30 + 20 * i)).collect();
    let mut tracks = vec![];
    let mut pivots = Map::new();
    for i in 0..8 {
        for level in ["a", "b", "c"] {
            tracks.push(json!({ "target": format!("{level}{i}"), "prop": "scale", "values": [1, 10, 1] }));
            pivots.insert(format!("{level}{i}"), json!([30 + 20 * i, 100]));
        }
    }
    let second = tracks.split_off(16);
    stored(json!({
        "name": "Heavier",
        "svg": format!("<svg viewBox='0 0 200 200'><defs>{defs}</defs><g id='body'>{parts}<circle id='head' cx='100' cy='40' r='20' fill='#ffcc00'/></g></svg>"),
        "pivots": pivots,
        "animations": { "churn": { "duration": 1, "loop": true, "ease": "linear", "tracks": tracks }, "churn2": { "duration": 1, "loop": true, "ease": "linear", "tracks": second } },
        "states": { "idle": ["churn", "churn2"] },
        "roles": { "body": "body", "head": "head" }
    }))
}

/// A blurred dot inside groups `a`, `b` and `c`, and the animations, states and drives given.
fn blurred(extra: Value) -> Value {
    let mut design = json!({
        "svg": "<svg viewBox='0 0 200 200'><defs><filter id='soft'><feGaussianBlur stdDeviation='2'/></filter></defs><g id='a'><g id='b'><g id='c'><circle id='dot' cx='100' cy='100' r='4' fill='#3366cc' filter='url(#soft)'/></g></g></g><circle id='head' cx='100' cy='40' r='20' fill='#ffcc00'/></svg>",
        "pivots": { "a": [100, 100], "b": [100, 100], "c": [100, 100] }
    });
    for (key, value) in extra.as_object().unwrap() {
        design[key] = value.clone();
    }
    stored(design)
}

/// What a look costs where its animations move it: the rest pose is cheap, the moment where
/// nested scales coincide is not, and the refusal says where and what to change, as a pose
/// picture of that moment would be refused.
#[test]
fn a_look_that_costs_too_much_where_it_moves_is_refused_with_where() {
    let look = nested_heavy();
    affordable(&look).expect("cheap at rest");
    let peak = moving(&look).unwrap();
    let why = peak.refused.clone().expect("refused");
    assert!(why.starts_with("this look's filters cost too much to draw in its animation churn at 0.5 (about "), "{why}");
    assert!(why.ends_with(", at most 60): animate a smaller scale, or put the filter on a part that does not grow"), "{why}");
    assert_eq!(peak.at, Moving::Animation { name: "churn".into(), builtin: false, at: 0.5 });
    assert!(peak.spent.filters > 1e6, "{peak:?}");
    // A pose picture of that moment is refused for the same number.
    let posed = sheet(&look, &at("churn", 0.5)).unwrap_err();
    let number = |s: &str| s.split("(about ").nth(1).and_then(|r| r.split(' ').next()).map(str::to_string);
    assert_eq!(number(&posed), number(&why), "{posed} / {why}");

    // The same at a tenth of the scale is within it, and its peak is that moment.
    let mild = blurred(json!({ "animations": { "grow": { "duration": 1, "loop": true, "tracks": [{ "target": "a", "prop": "scale", "values": [1, 2, 1] }] } } }));
    let peak = moving(&mild).unwrap();
    assert_eq!((peak.refused.as_deref(), &peak.at), (None, &Moving::Animation { name: "grow".into(), builtin: false, at: 0.5 }), "{peak:?}");
    let rest = model_spent(&mild);
    assert!(peak.spent.filters > 3.0 * rest.filters, "{peak:?} against {rest:?} at rest");
}

/// What the rest pose of a look costs, as a picture says it.
fn model_spent(look: &Value) -> Spent {
    let model = Model::read(look).unwrap();
    model.estimate(&model.body(&model.pose(&Pose::default()).unwrap())).unwrap()
}

/// Scales of nested groups multiply where they coincide, also when no keyframe of one lies where
/// the other peaks: two tracks that cross halfway, and an ease that overshoots between keyframes.
#[test]
fn the_peak_is_found_between_keyframes_where_tracks_cross_and_eases_overshoot() {
    // Each at most 10 times as large, together 30 times at the middle: no keyframe there.
    let crossing = blurred(json!({ "animations": { "cross": { "duration": 1, "times": [0, 1], "tracks": [
        { "target": "a", "prop": "scale", "values": [1, 10] },
        { "target": "b", "prop": "scale", "values": [10, 1] }
    ] } } }));
    let peak = moving(&crossing).unwrap();
    assert!(matches!(&peak.at, Moving::Animation { at, .. } if (at - 0.5).abs() < 0.07), "{peak:?}");
    let ends = moving(&blurred(json!({ "animations": { "end": { "duration": 1, "tracks": [{ "target": "a", "prop": "scale", "values": [1, 10] }] } } }))).unwrap();
    assert!(peak.spent.filters > 2.0 * ends.spent.filters, "{peak:?} against {ends:?}");
    // An ease that goes beyond its last keyframe before it settles there.
    let back = blurred(json!({ "animations": { "pop": { "duration": 1, "tracks": [{ "target": "a", "prop": "scale", "values": [1, 10], "ease": "outBack" }] } } }));
    let peak = moving(&back).unwrap();
    let last = Model::read(&back).unwrap().estimate(&{
        let model = Model::read(&back).unwrap();
        model.body(&model.pose(&at("pop", 1.0)).unwrap())
    }).unwrap();
    assert!(peak.spent.filters > last.filters * 1.1, "{peak:?} against its last keyframe {last:?}");
}

/// A drive is looked at along its input, at its ends and at its own keyframes, and the refusal
/// names the input and its value.
#[test]
fn a_drive_that_grows_a_filtered_part_is_refused_where_its_input_puts_it() {
    // Three drives on one input, one for each of the nested groups: they play together. (Below
    // the room a layer has, so that one value costs the most.)
    let drive = |values: Value, at: Value| {
        let drives: Vec<Value> = ["a", "b", "c"].iter().map(|t| json!({ "input": "turn", "target": t, "prop": "scale", "values": values, "at": at })).collect();
        blurred(json!({ "drives": drives }))
    };
    let why = moving(&drive(json!([1, 1, 4.5]), json!([0, 0.5, 1]))).unwrap().refused.expect("refused");
    assert!(why.starts_with("this look's filters cost too much to draw in its drives on turn at 1 (about "), "{why}");
    // Its peak in the middle of the input's range, where a keyframe of the drive lies.
    let why = moving(&drive(json!([1, 4.5, 1]), json!([0, 0.75, 1]))).unwrap().refused.expect("refused");
    assert!(why.starts_with("this look's filters cost too much to draw in its drives on turn at 0.5 (about "), "{why}");
    // The lip sync grows what it opens.
    let mouth = blurred(json!({ "mouth": [{ "target": "dot", "prop": "r", "open": 300 }] }));
    let why = moving(&mouth).unwrap().refused.expect("refused");
    assert!(why.starts_with("this look's filters cost too much to draw with its mouth open 1 (about "), "{why}");
    // Within it, its peak is said all the same.
    let peak = moving(&blurred(json!({ "drives": [{ "input": "turn", "target": "a", "prop": "scale", "values": [1, 1, 3], "at": [0, 0.5, 1] }] }))).unwrap();
    assert_eq!((peak.refused, peak.at), (None, Moving::Input { input: "turn", value: 1.0 }));
}

/// The animations a state lists play together, under the drives, so they are also taken at once,
/// each at its costliest: three that each scale one of the nested groups are cheap alone and
/// multiply together. Animations no state lists together are gestures, which replace one another.
#[test]
fn animations_that_each_cost_little_are_refused_when_they_play_together() {
    let level = |target: &str| json!({ "duration": 1, "loop": true, "tracks": [{ "target": target, "prop": "scale", "values": [1, 5, 1] }] });
    let look = blurred(json!({ "animations": { "ga": level("a"), "gb": level("b"), "gc": level("c") }, "states": { "idle": ["ga", "gb", "gc"] } }));
    for name in ["ga", "gb", "gc"] {
        assert!(sheet(&look, &at(name, 0.5)).is_ok(), "{name} alone is drawn");
    }
    let peak = moving(&look).unwrap();
    let why = peak.refused.expect("refused");
    assert!(why.starts_with("this look's filters cost too much to draw when the animations of its state idle play together, each at its costliest (about "), "{why}");
    assert_eq!(peak.at, Moving::Together { state: "idle".to_string() });
    // Two of them together are within it.
    let two = blurred(json!({ "animations": { "ga": level("a"), "gb": level("b") }, "states": { "idle": ["ga", "gb"] } }));
    assert_eq!(moving(&two).unwrap().refused, None);
    // The same three with no state that lists them together: each is a gesture of its own, and
    // gestures replace one another, so nothing is refused for a pose the browser never shows.
    let apart = blurred(json!({ "animations": { "ga": level("a"), "gb": level("b"), "gc": level("c") }, "states": { "idle": ["ga"], "working": ["gb"], "done": ["gc"] } }));
    assert_eq!(moving(&apart).unwrap().refused, None);
}

/// Workbench's built-in motion scales a part by a few hundredths at most: it never makes a look
/// that is cheap at rest dear. A look of filtered roles, at rest and at its peak.
#[test]
fn the_built_in_motion_scales_by_a_few_hundredths_at_most() {
    let look = stored(json!({
        "svg": "<svg viewBox='0 0 200 200'><defs><filter id='soft' x='-20%' y='-20%' width='140%' height='140%'><feDropShadow dx='2' dy='2' stdDeviation='3'/></filter></defs><g id='body' filter='url(#soft)'><ellipse cx='100' cy='130' rx='50' ry='45' fill='#3366cc'/><g id='head' filter='url(#soft)'><circle cx='100' cy='60' r='35' fill='#ffcc00'/><g id='eyes'><circle id='pupils' cx='100' cy='60' r='5' fill='#222222'/></g></g><rect id='armL' x='40' y='110' width='12' height='40' fill='#3366cc' filter='url(#soft)'/><rect id='armR' x='148' y='110' width='12' height='40' fill='#3366cc' filter='url(#soft)'/></g></svg>",
        "roles": { "body": "body", "head": "head", "eyes": "eyes", "pupils": "pupils", "armL": "armL", "armR": "armR" }
    }));
    let model = Model::read(&look).unwrap();
    for (name, animation) in model.builtins() {
        for track in animation["tracks"].as_array().unwrap() {
            if track["prop"].as_str().unwrap().starts_with("scale") {
                for v in track["values"].as_array().unwrap() {
                    assert!(v.as_f64().unwrap() <= 1.05, "{name}: {track}");
                }
            }
        }
    }
    // Alone, each costs at most about a tenth more than the look at rest (arms turned up widen
    // the body's filter more than its stretch does); everything at once, a third.
    let rest = model_spent(&look);
    for (name, _) in model.builtins() {
        let most = (0..=16).map(|k| model.estimate(&model.body(&model.pose(&at(name, k as f64 / 16.0)).unwrap())).unwrap().filters).fold(0.0, f64::max);
        eprintln!("{name}: {most:.2} against {:.2} at rest", rest.filters);
        assert!(most <= rest.filters * 1.15, "{name}: {most} against {rest:?} at rest");
    }
    let peak = moving(&look).unwrap();
    assert_eq!(peak.refused, None);
    assert!(peak.spent.filters <= rest.filters * 1.34, "{peak:?} against {rest:?} at rest");
}

/// The search is bounded whatever a look holds: at most 32 animations of 16 tracks of 16
/// keyframes and 16 drives give a few hundred estimates, each a parse and no drawing.
#[test]
fn the_search_for_the_costliest_moment_is_bounded() {
    let parts: String = (0..16).map(|i| format!("<g id='g{i}' opacity='0.9'><circle id='d{i}' cx='{}' cy='100' r='3' fill='#3366cc' filter='url(#soft)'/></g>", 10 + 11 * i)).collect();
    let track = |i: usize| json!({ "target": format!("g{i}"), "prop": "scale", "values": (0..16).map(|k| 1.0 + ((k * 7 + i) % 5) as f64 * 0.1).collect::<Vec<_>>() });
    let animations: Map<String, Value> = (0..32).map(|a| (format!("m{a}"), json!({ "duration": 1 + a % 3, "loop": a % 2 == 0, "tracks": (0..16).map(|i| track((i + a) % 16)).collect::<Vec<_>>() }))).collect();
    let drives: Vec<Value> = (0..16).map(|i| json!({ "input": INPUTS[i % 6], "target": format!("g{i}"), "prop": "scale", "values": [1, 1.2, 1.1, 1.3, 1], "at": [0, 0.1, 0.5, 0.7, 1] })).collect();
    let mut pivots = Map::new();
    for i in 0..16 {
        pivots.insert(format!("g{i}"), json!([10 + 11 * i, 100]));
    }
    let look = stored(json!({
        "svg": format!("<svg viewBox='0 0 200 200'><defs><filter id='soft'><feGaussianBlur stdDeviation='1'/></filter></defs><g id='body'>{parts}</g></svg>"),
        "pivots": pivots, "animations": animations, "drives": drives, "roles": { "body": "body" }
    }));
    let started = std::time::Instant::now();
    let peak = moving(&look).unwrap();
    let took = started.elapsed();
    let builtins = Model::read(&look).unwrap().builtins().len();
    let most = (32 + builtins) * (PEAK_TRIES + 1) + INPUTS.len() * PEAK_TRIES + 2;
    assert!(peak.estimated <= most, "{} estimates, at most {most}", peak.estimated);
    eprintln!("the largest look: {} estimates in {took:?}, peak {:?} at {:?}", peak.estimated, peak.spent, peak.at);
}

/// Every example the documentation and the tests hold (`examples/`, `examples/parity/`,
/// `examples/face/`) moves within what a picture may cost, with room to spare; each one's
/// costliest moment is printed (`--nocapture`).
#[test]
fn the_example_looks_move_within_what_a_picture_may_cost() {
    let dir = examples_dir();
    let mut looks: Vec<(String, Value)> = vec![];
    for sub in [dir.clone(), dir.join("parity"), dir.join("face")] {
        for file in files_in(&sub).into_iter().filter(|f| f.ends_with(".look.json")) {
            looks.push((file.clone(), serde_json::from_str(&std::fs::read_to_string(sub.join(&file)).unwrap()).unwrap()));
        }
    }
    assert!(looks.len() >= 20, "{}", looks.len());
    for (name, look) in looks {
        let peak = moving(&look).unwrap_or_else(|e| panic!("{name}: {e}"));
        let rest = model_spent(&look);
        eprintln!("{name}: rest {:.2} filters {:.2} layers; peak {:.2} filters {:.2} layers {:?}", rest.filters, rest.layers, peak.spent.filters, peak.spent.layers, peak.at);
        assert_eq!(peak.refused, None, "{name}");
        assert!(share(peak.spent) < 0.5, "{name}: {peak:?}");
    }
}

/// What a design costs at rest and at its costliest moment, for designs of a directory or files:
/// `AVATAR_PEAK_LOOKS=/a/look.json:/some/dir cargo test avatar::preview::tests::peaks_of -- --ignored --nocapture`.
#[test]
#[ignore]
fn peaks_of_the_designs_named() {
    let named = std::env::var("AVATAR_PEAK_LOOKS").expect("AVATAR_PEAK_LOOKS");
    for path in named.split(':').map(std::path::PathBuf::from) {
        let files = if path.is_dir() { files_in(&path).into_iter().filter(|f| f.ends_with(".json")).map(|f| path.join(f)).collect() } else { vec![path] };
        for file in files {
            let design: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
            let look = match crate::avatar::look::apply(None, &design) {
                Ok(applied) => serde_json::to_value(applied.design.look.unwrap()).unwrap(),
                Err(_) => design,
            };
            let started = std::time::Instant::now();
            match moving(&look) {
                Ok(peak) => {
                    let rest = Model::read(&look).ok().and_then(|model| model.estimate(&model.body(&model.pose(&Pose::default()).ok()?)).ok());
                    eprintln!("{}: rest {rest:?}; peak {:?} at {:?} ({} estimates, {:?}); {}", file.display(), peak.spent, peak.at, peak.estimated, started.elapsed(), peak.refused.as_deref().unwrap_or("accepted"));
                }
                Err(e) => eprintln!("{}: {e}", file.display()),
            }
        }
    }
}

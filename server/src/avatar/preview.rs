//! Pictures of a look (OWNER: avatar slice).
//!
//! A session cannot see the browser, so it is shown what it drew. [`sheet`] draws its look in one
//! pose at the sizes Workbench shows it: 264 px on the light panel and on the dark panel side by
//! side, and under each the figure at 56 px and its face at 40 px, both as they rest. [`strip`]
//! draws 2 to 8 moments of one of the look's own animations in a row, 132 px each, on the light
//! panel. Both answer one PNG; the caller describes the layout in words (nothing is written in the
//! picture).
//!
//! The input is the stored look as JSON, in the wire format the browser reads (`lib/types.ts`), not
//! look.rs's types. It came through the validator, but it is read here as untrusted data again:
//! the SVG text is written only from the literal tables of this file (tags, attribute names,
//! keywords), every value is parsed and written again from its numbers, text is escaped, an id or
//! a reference is printed only when it matches the id pattern and resolves to an element of the
//! right kind, and the walk is bounded (elements, `use` expansions, depth, list lengths). usvg is
//! given no resources directory and an image resolver that resolves nothing, so nothing is read
//! but the text itself and the system's fonts, which are loaded once, and only for a look with text.
//!
//! A pose follows the browser engine (`lib/engine.ts`): one of the look's own animations as a state
//! animation at a progress (relative props add, scales multiply, absolute props are set), then the
//! drives, then the look's `mouth` entries and the gaze; `parents` carry a part along with another
//! one as if it were nested in it. The built-in animations are the browser's alone: a pose names
//! the look's own animations only, and is told so for another name.
//!
//! Nothing here is called by the server itself: a picture is drawn in a process of its own
//! (`draw`), which calls [`draw_sheet`] and [`draw_strip`] under a limit on its memory and its
//! time. A sheet of the example looks takes a few tens of milliseconds. What a look costs beyond
//! its size (layers, filters, dashes, strokes far beyond the picture) is estimated from the parsed
//! tree before anything is drawn, and a look over the estimate is answered with a sentence that
//! says what to change. The estimate is a first answer, not the bound: it mirrors another
//! program's bookkeeping and has been wrong before. The bound is the process.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{Color, Paint, Pixmap, PixmapPaint, Rect, Transform};
use resvg::usvg;
use serde_json::{Map, Value};

// ---------------------------------------------------------------- sizes and colours

/// The figure on each of the two big panels of a sheet.
pub const SHEET_FIGURE_PX: u32 = 264;
/// The figure as it rests, under each big panel.
pub const SMALL_FIGURE_PX: u32 = 56;
/// The face (the square the browser crops around the head), beside the small figure.
pub const FACE_PX: u32 = 40;
/// One frame of a strip.
pub const STRIP_FRAME_PX: u32 = 132;
/// Frames of a strip: at least, at most, and what a caller asks for when it names no number.
pub const STRIP_FRAMES_MIN: usize = 2;
pub const STRIP_FRAMES_MAX: usize = 8;
pub const STRIP_FRAMES_DEFAULT: usize = 6;
/// The size of a text that names none (look.rs's box assumes it, and the browser writes it).
const DEFAULT_FONT_SIZE: &str = "16";
/// The least a face shows of the viewBox's smaller side (`faceViewBox` of look.ts has the same).
const FACE_LEAST: f64 = 0.1;
/// Pixels between two panels.
const GAP_PX: u32 = 4;
/// The row of small views under a big panel: the 56 px figure with this much room around it.
const SMALL_ROW_PX: u32 = SMALL_FIGURE_PX + 2 * GAP_PX;
/// Between the small figure and the face.
const SMALL_GAP_PX: u32 = 8;

/// A panel's colour and the colour `currentColor` takes on it.
#[derive(Debug, Clone, Copy)]
struct Theme {
    panel: [u8; 3],
    fg: &'static str,
}

// The light and the dark theme's `--bg-panel` and `--fg`, as web/src/theme/tokens.css defines them.
const LIGHT: Theme = Theme { panel: [0xf7, 0xf8, 0xfa], fg: "#1e1f22" };
const DARK: Theme = Theme { panel: [0x2b, 0x2d, 0x30], fg: "#dfe1e5" };
/// Between the panels: the light theme's `--border-strong` (web/src/theme/tokens.css), which
/// stands apart from both panels.
const GAP_COLOUR: [u8; 3] = [0xc9, 0xcc, 0xd6];

/// How much filter work a look may ask for, in passes of a cheap primitive over a whole panel
/// (half a millisecond each for a 264 px panel here; the browser pays its own share on every
/// frame a filtered part moves). A drop shadow over the whole figure is about ten, a blur six.
pub const FILTER_WORK: f64 = 60.0;
/// How many layers the size of a panel a look may ask for: every group that is filtered, masked,
/// clipped or not opaque is one, a mask or a clip one more.
pub const LAYER_WORK: f64 = 60.0;
/// How far a stroked shape or a text may reach, in widths of the panel it is drawn into. The
/// stroker works to a tolerance that shrinks with the scale: far beyond the picture, every curve
/// is cut to its recursion limit, and one picture takes minutes.
const REACH_MOST: f64 = 400.0;
/// The largest single layer, in panels. resvg keeps a layer within five panels each way, which
/// a filter on a part that is squeezed one way and stretched the other gets around: such a look
/// is not drawn.
const LAYER_MOST: f64 = 30.0;

// ---------------------------------------------------------------- limits

// The validator's limits (look.rs), repeated so that a stored file that was changed by hand is
// still bounded here: what lies beyond them is left out of the picture.
const MAX_ELEMENTS: usize = 300;
const MAX_DEPTH: usize = 10;
const MAX_TEXT_CHARS: usize = 40;
const MAX_D_CHARS: usize = 4000;
const MAX_POINTS_CHARS: usize = 2000;
const MAX_TRANSFORM_CHARS: usize = 1024;
const MAX_TRANSFORMS: usize = 8;
const MAX_ANIMATIONS: usize = 32;
const MAX_TRACKS: usize = 16;
const MAX_KEYFRAMES: usize = 16;
const MAX_TARGETS: usize = 16;
const MAX_MOUTH_DRIVES: usize = 4;
const MAX_DRIVES: usize = 16;
const MAX_PARENTS: usize = 64;
const MAX_DASHES: usize = 8;
const MAX_COLOUR_MATRIX: usize = 20;
/// No number of a look is larger than this.
const NUM_MAX: f64 = 1e6;
const VIEW_MIN: f64 = 16.0;
const VIEW_MAX: f64 = 4096.0;
/// Pupil travel when the look gives none, and the most it may ask for, as parts of the viewBox width.
const GAZE_DEFAULT: f64 = 0.03;
const GAZE_MOST: f64 = 0.05;

/// Workbench's built-in animations (look.rs `BUILTIN`), which the browser generates from roles.
const BUILTIN: [&str; 11] = ["breathe", "blink", "think", "talk", "listen", "alert", "doze", "wave", "cheer", "nod", "shake"];

// ---------------------------------------------------------------- the pose

/// Where a look's live inputs stand. `look_x`, `look_y` and `turn` run from -1 to 1, `pace` and
/// `held` from 0 to 1 (0 is at rest). `mouth`: `None` while no line is said, so the look's `mouth`
/// entries are not applied and a drive on `mouth` reads 0; `Some(0..1)` while it speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Inputs {
    pub look_x: f64,
    pub look_y: f64,
    pub mouth: Option<f64>,
    pub pace: f64,
    pub turn: f64,
    pub held: f64,
}

/// What a picture shows: nothing moving (the default), or one of the look's own animations at a
/// progress from 0 to 1, and the inputs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pose {
    pub animation: Option<(String, f64)>,
    pub inputs: Inputs,
    /// The id of a part to look at closely: the large views show the square around it.
    pub zoom: Option<String>,
}

/// The keys of a pose as a call writes it.
pub const POSE_KEYS: [&str; 9] = ["animation", "at", "lookX", "lookY", "mouth", "pace", "turn", "held", "zoom"];

impl Pose {
    /// A pose as a call writes it: `{animation?, at?, lookX?, lookY?, mouth?, pace?, turn?, held?}`.
    /// `at` is how far into `animation`, 0 to 1 (default 0.5). What is wrong is answered with a
    /// sentence the session can act on.
    pub fn from_json(value: &Value) -> Result<Pose, String> {
        let Value::Object(map) = value else {
            return Err("a pose is an object with animation, at, lookX, lookY, mouth, pace, turn, held and zoom".into());
        };
        if let Some(key) = map.keys().find(|k| !POSE_KEYS.contains(&k.as_str())) {
            return Err(format!("a pose takes animation, at, lookX, lookY, mouth, pace, turn, held and zoom, not {}", quote(key)));
        }
        let number = |key: &str, min: f64, max: f64| -> Result<Option<f64>, String> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => match v.as_f64() {
                    Some(n) if n.is_finite() && (min..=max).contains(&n) => Ok(Some(n)),
                    _ => Err(format!("{key} in a pose is a number from {min} to {max}")),
                },
            }
        };
        let at = number("at", 0.0, 1.0)?;
        let animation = match map.get("animation") {
            None | Some(Value::Null) => {
                if at.is_some() {
                    return Err("at in a pose says how far into its animation: name the animation too".into());
                }
                None
            }
            Some(Value::String(name)) => Some((name.clone(), at.unwrap_or(0.5))),
            Some(_) => return Err("animation in a pose is the name of one of the look's own animations".into()),
        };
        let inputs = Inputs {
            look_x: number("lookX", -1.0, 1.0)?.unwrap_or(0.0),
            look_y: number("lookY", -1.0, 1.0)?.unwrap_or(0.0),
            mouth: number("mouth", 0.0, 1.0)?,
            pace: number("pace", 0.0, 1.0)?.unwrap_or(0.0),
            turn: number("turn", -1.0, 1.0)?.unwrap_or(0.0),
            held: number("held", 0.0, 1.0)?.unwrap_or(0.0),
        };
        let zoom = match map.get("zoom") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) => Some(id.clone()),
            Some(_) => return Err("zoom in a pose is the id of the part to look at closely".into()),
        };
        Ok(Pose { animation, inputs, zoom })
    }

    /// The pose as a call writes it: what [`Pose::from_json`] reads back as the same pose.
    pub fn to_json(&self) -> Value {
        let mut map = self.inputs.to_json();
        if let Some((name, at)) = &self.animation {
            map.insert("animation".into(), Value::from(name.clone()));
            map.insert("at".into(), Value::from(*at));
        }
        if let Some(id) = &self.zoom {
            map.insert("zoom".into(), Value::from(id.clone()));
        }
        Value::Object(map)
    }
}

impl Inputs {
    /// The inputs as a pose writes them, those at rest left out.
    pub fn to_json(&self) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in [("lookX", self.look_x), ("lookY", self.look_y), ("pace", self.pace), ("turn", self.turn), ("held", self.held)] {
            if value != 0.0 {
                map.insert(key.into(), Value::from(value));
            }
        }
        if let Some(open) = self.mouth {
            map.insert("mouth".into(), Value::from(open));
        }
        map
    }

    /// The inputs of a pose as a call writes it: its other keys are not looked at.
    pub fn from_json(value: &Value) -> Result<Inputs, String> {
        let mut only = Map::new();
        if let Value::Object(map) = value {
            for key in ["lookX", "lookY", "mouth", "pace", "turn", "held"] {
                if let Some(v) = map.get(key) {
                    only.insert(key.into(), v.clone());
                }
            }
        }
        Pose::from_json(&Value::Object(only)).map(|pose| pose.inputs)
    }
}

// ---------------------------------------------------------------- pictures

/// A picture and what drawing it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub png: Vec<u8>,
    pub spent: Spent,
}

/// The look in `pose` at 264 px on the light panel (left) and on the dark panel (right), and under
/// each, on the same panel, the figure as it rests at 56 px and its face at 40 px. One PNG, 532 by
/// 332 pixels. With `pose.zoom` the two large views show the square around that part instead of
/// the whole figure.
pub fn draw_sheet(look: &Value, pose: &Pose) -> Result<Picture, String> {
    let model = Model::read(look)?;
    let posed = model.body(&model.pose(pose)?);
    let rest = model.body(&model.pose(&Pose::default())?);
    let view = model.view;
    let face = model.face_view();
    let close = match &pose.zoom {
        Some(id) => model.zoom_view(id)?,
        None => view,
    };

    let width = 2 * SHEET_FIGURE_PX + GAP_PX;
    let height = SHEET_FIGURE_PX + GAP_PX + SMALL_ROW_PX;
    let mut canvas = canvas(width, height)?;
    for (column, theme) in [LIGHT, DARK].into_iter().enumerate() {
        let x = column as u32 * (SHEET_FIGURE_PX + GAP_PX);
        let big = model.render(&posed, close, SHEET_FIGURE_PX, theme)?;
        place(&mut canvas, x, 0, &big);
        let row = SHEET_FIGURE_PX + GAP_PX;
        fill(&mut canvas, x, row, SHEET_FIGURE_PX, SMALL_ROW_PX, theme.panel);
        // The small figure and the face, side by side in the middle of the row.
        let left = x + (SHEET_FIGURE_PX - SMALL_FIGURE_PX - SMALL_GAP_PX - FACE_PX) / 2;
        let small = model.render(&rest, view, SMALL_FIGURE_PX, theme)?;
        place(&mut canvas, left, row + GAP_PX, &small);
        let head = model.render(&rest, face, FACE_PX, theme)?;
        place(&mut canvas, left + SMALL_FIGURE_PX + SMALL_GAP_PX, row + GAP_PX + (SMALL_FIGURE_PX - FACE_PX) / 2, &head);
    }
    let png = canvas.encode_png().map_err(|e| format!("the picture could not be encoded: {e}"))?;
    Ok(Picture { png, spent: model.spent.get() })
}

/// [`draw_sheet`], the picture alone.
#[cfg(test)]
pub fn sheet(look: &Value, pose: &Pose) -> Result<Vec<u8>, String> {
    draw_sheet(look, pose).map(|picture| picture.png)
}

/// Whether the look can be drawn at rest within what a picture may cost: `Err` is a sentence that
/// says what to change. Nothing is drawn. For the tests that hold documented examples to what a
/// design call stores; a design call itself draws the look, in a process of its own.
#[cfg(test)]
pub fn affordable(look: &Value) -> Result<(), String> {
    let model = Model::read(look)?;
    let rest = model.body(&model.pose(&Pose::default())?);
    // Every view a sheet draws: what is stored can be shown at rest.
    for (view, size) in [(model.view, SHEET_FIGURE_PX), (model.view, SMALL_FIGURE_PX), (model.face_view(), FACE_PX)] {
        model.tree(&rest, view, size, LIGHT)?;
    }
    Ok(())
}

/// `frames` moments (2 to 8) of the look's own `animation`, left to right, 132 px each on the light
/// panel, at even moments of its run: a loop at i/n of its duration (the frame after the last would
/// be the first again), a one-shot or a recurring one at i/(n-1), so the last frame is its end.
/// `inputs` hold for every moment: a walk that shows only in a side view is drawn with the turn
/// and the pace that show it.
pub fn draw_strip(look: &Value, animation: &str, frames: usize, inputs: &Inputs) -> Result<Picture, String> {
    if !(STRIP_FRAMES_MIN..=STRIP_FRAMES_MAX).contains(&frames) {
        return Err(format!("a strip has {STRIP_FRAMES_MIN} to {STRIP_FRAMES_MAX} frames"));
    }
    let model = Model::read(look)?;
    let looping = model.animation(animation)?.looping;
    let width = frames as u32 * STRIP_FRAME_PX + (frames as u32 - 1) * GAP_PX;
    let mut canvas = canvas(width, STRIP_FRAME_PX)?;
    for i in 0..frames {
        let at = if looping { i as f64 / frames as f64 } else { i as f64 / (frames - 1) as f64 };
        let pose = Pose { animation: Some((animation.to_string(), at)), inputs: *inputs, zoom: None };
        let body = model.body(&model.pose(&pose)?);
        let frame = model.render(&body, model.view, STRIP_FRAME_PX, LIGHT)?;
        place(&mut canvas, i as u32 * (STRIP_FRAME_PX + GAP_PX), 0, &frame);
    }
    let png = canvas.encode_png().map_err(|e| format!("the picture could not be encoded: {e}"))?;
    Ok(Picture { png, spent: model.spent.get() })
}

/// [`draw_strip`] with every input at rest, the picture alone.
#[cfg(test)]
pub fn strip(look: &Value, animation: &str, frames: usize) -> Result<Vec<u8>, String> {
    draw_strip(look, animation, frames, &Inputs::default()).map(|picture| picture.png)
}

fn canvas(width: u32, height: u32) -> Result<Pixmap, String> {
    let mut canvas = Pixmap::new(width, height).ok_or("the picture could not be made")?;
    canvas.fill(Color::from_rgba8(GAP_COLOUR[0], GAP_COLOUR[1], GAP_COLOUR[2], 255));
    Ok(canvas)
}

fn place(canvas: &mut Pixmap, x: u32, y: u32, picture: &Pixmap) {
    canvas.draw_pixmap(x as i32, y as i32, picture.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn fill(canvas: &mut Pixmap, x: u32, y: u32, width: u32, height: u32, colour: [u8; 3]) {
    let Some(rect) = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32) else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(colour[0], colour[1], colour[2], 255);
    paint.anti_alias = false;
    canvas.fill_rect(rect, &paint, Transform::identity(), None);
}

// ---------------------------------------------------------------- drawing one panel

/// The fonts a picture's text is drawn with, read once, for looks that have text: the system's
/// sans-serif family and a short list to fall back on. Every other font of the system is left
/// out, so looking for a character no font has costs a few files, not all of them.
static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();

/// Sans-serif families tried in turn when the system names none that is installed.
const SANS: [&str; 10] = ["Noto Sans", "DejaVu Sans", "Liberation Sans", "Arial", "Helvetica", "Segoe UI", "Ubuntu", "Cantarell", "FreeSans", "Roboto"];

/// Families a character is looked for in when the sans-serif family lacks it: symbols, arrows,
/// mathematics, monochrome emoji, and one family for Chinese, Japanese and Korean.
const FALLBACK: [&str; 12] = [
    "DejaVu Sans", "Noto Sans Symbols", "Noto Sans Symbols 2", "Noto Sans Symbols2", "Noto Sans Math", "Noto Emoji", "Symbola", "Segoe UI Symbol", "Noto Sans CJK SC", "Noto Sans CJK JP",
    "Microsoft YaHei", "Arial Unicode MS",
];

fn fonts() -> Arc<usvg::fontdb::Database> {
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            let installed = |db: &usvg::fontdb::Database, name: &str| db.faces().any(|face| face.families.iter().any(|(family, _)| family == name));
            let named = db.family_name(&usvg::fontdb::Family::SansSerif).to_string();
            let chosen = std::iter::once(named)
                .chain(SANS.iter().map(|s| s.to_string()))
                .find(|name| installed(&db, name))
                .or_else(|| db.faces().find_map(|face| face.families.first().map(|(family, _)| family.clone())));
            // Keep the chosen family whole (its weights) and one upright regular face of each
            // family to fall back on.
            let keep = |face: &usvg::fontdb::FaceInfo| {
                let of = |name: &str| face.families.iter().any(|(family, _)| family == name);
                chosen.as_deref().is_some_and(of) || (FALLBACK.iter().any(|name| of(name)) && face.style == usvg::fontdb::Style::Normal && face.weight == usvg::fontdb::Weight::NORMAL)
            };
            let leave: Vec<usvg::fontdb::ID> = db.faces().filter(|face| !keep(face)).map(|face| face.id).collect();
            for id in leave {
                db.remove_face(id);
            }
            // usvg falls back to the serif family: it is the sans-serif one here too.
            if let Some(name) = chosen {
                db.set_sans_serif_family(name.clone());
                db.set_serif_family(name);
            }
            Arc::new(db)
        })
        .clone()
}

/// usvg that reads nothing but the text it is given: no resources directory, no image of any
/// kind, and fonts only from the database (empty for a look without text).
fn options(text: bool) -> usvg::Options<'static> {
    let mut opt = usvg::Options::default();
    opt.resources_dir = None;
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(|_: &str, _: Arc<Vec<u8>>, _: &usvg::Options| None),
        resolve_string: Box::new(|_: &str, _: &usvg::Options| None),
    };
    opt.style_sheet = None;
    if text {
        opt.fontdb = fonts();
    }
    // A character the chosen font lacks is looked for in the few families kept beside it, once:
    // what none of them has is drawn as an empty box (the browser has more fonts to ask).
    let found: std::sync::Mutex<HashMap<char, Option<usvg::fontdb::ID>>> = std::sync::Mutex::new(HashMap::new());
    let ask = usvg::FontResolver::default_fallback_selector();
    opt.font_resolver.select_fallback = Box::new(move |c, used, db| {
        if let Some(known) = found.lock().ok().and_then(|found| found.get(&c).copied()) {
            return known.filter(|id| !used.contains(id));
        }
        let id = ask(c, used, db);
        if let Ok(mut found) = found.lock() {
            found.insert(c, id);
        }
        id
    });
    opt
}

impl Model<'_> {
    /// The body drawn into a square panel of `size` pixels, showing `view` of the artwork.
    fn render(&self, body: &str, view: [f64; 4], size: u32, theme: Theme) -> Result<Pixmap, String> {
        let tree = self.tree(body, view, size, theme)?;
        let mut pixmap = Pixmap::new(size, size).ok_or("the picture could not be made")?;
        pixmap.fill(Color::from_rgba8(theme.panel[0], theme.panel[1], theme.panel[2], 255));
        // The renderer is not ours: a size or a position it cannot work with must not end more
        // than this one picture.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut()))).map_err(|_| NOT_DRAWABLE.to_string())?;
        Ok(pixmap)
    }

    /// The body parsed for a panel of `size` pixels, when drawing it stays within what a picture
    /// may cost; else a sentence that says what to change. Nothing is drawn here.
    fn tree(&self, body: &str, view: [f64; 4], size: u32, theme: Theme) -> Result<usvg::Tree, String> {
        if self.unwritable.get() {
            return Err("a part is scaled beyond what can be drawn (its scales and those of what holds it multiply past a thousand million): scale it less".to_string());
        }
        let svg = document(body, view, size, theme.fg);
        let options = options(self.has_text);
        let tree = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| usvg::Tree::from_str(&svg, &options))).map_err(|_| NOT_DRAWABLE.to_string())?.map_err(|e| format!("the look could not be drawn: {e}"))?;
        let cost = Cost::of(&tree, size);
        let panel = f64::from(size) * f64::from(size);
        if cost.largest > LAYER_MOST * panel {
            return Err("a filter sits on a part that is stretched or skewed too far to draw: take the scale or skew off that part and off what holds it, or put the filter on a part that is not stretched".to_string());
        }
        if cost.reach > REACH_MOST * f64::from(size) {
            return Err(format!(
                "a stroked shape or a text reaches about {:.0} times the picture's width: it is drawn far too large (a tiny viewBox region, a head that is a dot, or scales that multiply): draw it at the size of the figure",
                cost.reach / f64::from(size)
            ));
        }
        if cost.filters > FILTER_WORK * panel {
            return Err(format!(
                "this look's filters cost too much to draw (about {:.0} passes over the whole figure, at most {FILTER_WORK:.0}): give fewer parts a filter, keep filter regions close to their part, or use fewer and smaller primitives",
                cost.filters / panel
            ));
        }
        if cost.layers > LAYER_WORK * panel {
            return Err(format!(
                "this look has too many masked, clipped, filtered or half-transparent groups to draw (about {:.0} layers the size of the figure, at most {LAYER_WORK:.0}): mask or clip one group that holds the parts instead of every part",
                cost.layers / panel
            ));
        }
        if cost.dashes > MAX_DASHES_DRAWN {
            return Err(format!(
                "this look's dashes are too fine to draw (about {:.0} dashes, at most {MAX_DASHES_DRAWN:.0}): make the dashes longer or dash fewer strokes",
                cost.dashes
            ));
        }
        let spent = self.spent.get();
        self.spent.set(Spent { filters: spent.filters.max(cost.filters / panel), layers: spent.layers.max(cost.layers / panel) });
        Ok(tree)
    }
}

/// The answer for a look the renderer itself gave up on.
const NOT_DRAWABLE: &str = "this look cannot be drawn: a size or a position in it is beyond what the renderer can work with (a mask or a filter on a part that is nearly nothing wide, far from the figure)";

/// The SVG document around a body: the panel's size, the part of the artwork shown, and the
/// colour `currentColor` takes.
fn document(body: &str, view: [f64; 4], size: u32, fg: &str) -> String {
    let mut out = String::with_capacity(body.len() + 160);
    out.push_str("<svg xmlns='http://www.w3.org/2000/svg'");
    attr(&mut out, "width", &size.to_string());
    attr(&mut out, "height", &size.to_string());
    attr(&mut out, "viewBox", &view.iter().map(|n| num(*n).unwrap_or_else(|| "0".into())).collect::<Vec<_>>().join(" "));
    attr(&mut out, "color", fg);
    out.push('>');
    out.push_str(body);
    out.push_str("</svg>");
    out
}

/// What drawing a parsed tree costs beyond the size of the look: layers, filters, dashes, and how
/// far its strokes reach.
#[derive(Debug, Default)]
struct Cost {
    /// Pixels of the largest layer resvg would allocate for one group.
    largest: f64,
    /// Pixels of all the layers: every group that is filtered, masked, clipped or not opaque is
    /// drawn into a layer of its own, and a mask or a clip into one more.
    layers: f64,
    /// Pixels a cheap primitive is run over: per group with filters, its layer times the cost of
    /// each primitive per pixel.
    filters: f64,
    /// Dashes cut from the strokes: per dashed path, its length over the dash pattern.
    dashes: f64,
    /// How far from the panel a stroked path or a text reaches, in pixels: the stroker cuts
    /// curves ever finer with the scale they are drawn at, without an end in sight.
    reach: f64,
}

/// A layer's room or box beyond this many pixels is not sized here: resvg works them out in
/// 32-bit integers, which stop at their limits, and what it then allocates follows no rule.
const LAYER_COORDINATES: f64 = 1e8;

impl Cost {
    /// What drawing `tree` into a square panel of `size` pixels costs.
    fn of(tree: &usvg::Tree, size: u32) -> Cost {
        let mut cost = Cost::default();
        let side = size as f32;
        // resvg keeps layers within twice the panel's size beyond each edge.
        if let Some(room) = Rect::from_xywh(-2.0 * side, -2.0 * side, 5.0 * side, 5.0 * side) {
            cost.add(tree.root(), Transform::identity(), room);
        }
        cost
    }

    /// Walks the tree as resvg's renderer does (`render_group`, `render_layer`) and adds up what
    /// it would allocate and run, without drawing. `transform` is what the group is drawn under,
    /// `room` what a layer is kept within.
    ///
    /// A filtered group is drawn with only the scale of its transform, and the rest (rotation,
    /// skew) is applied to the finished layer; the room is taken through the inverse of that rest.
    /// With a transform that squeezes one way and stretches the other, that inverse is enormous,
    /// and so is the layer: this is where a small look can ask for more memory than there is.
    /// What is inside a layer is drawn shifted to the layer's corner, while the room stays where
    /// it was: nested layers are sized with that shift.
    ///
    /// This is a first answer with a sentence, not the last line: the picture is drawn in a
    /// process of its own with a limit on its memory and its time (`draw.rs`).
    fn add(&mut self, group: &usvg::Group, transform: Transform, room: Rect) {
        let transform = transform.pre_concat(group.transform());
        if !group.should_isolate() {
            self.children(group, transform, room);
            return;
        }
        let filtered = !group.filters().is_empty();
        let (inner, room) = if filtered {
            let sx = transform.sx.hypot(transform.ky);
            let sy = transform.kx.hypot(transform.sy);
            let rest = Transform::from_row(transform.sx / sx, transform.ky / sx, transform.kx / sy, transform.sy / sy, 0.0, 0.0);
            // What resvg cannot invert or place it does not draw.
            let Some(back) = rest.invert() else { return };
            let Some(room) = room.transform(back) else { return };
            (back.pre_concat(transform), room)
        } else {
            (transform, room)
        };
        let Some(bbox) = group.layer_bounding_box().and_then(|b| b.transform(inner)) else { return };
        let far = |r: [f32; 4]| r.iter().any(|v| !v.is_finite() || f64::from(v.abs()) > LAYER_COORDINATES);
        if far([room.left(), room.top(), room.right(), room.bottom()]) || far([bbox.left(), bbox.top(), bbox.right(), bbox.bottom()]) {
            self.largest = f64::INFINITY;
            return;
        }
        // The layer as resvg cuts it: whole pixels, two more on each side where nothing is
        // filtered, and no further than the room.
        let grow = if filtered { 0.0 } else { 2.0 };
        let left = (f64::from(bbox.left()).floor() - grow).max(f64::from(room.left()).floor());
        let top = (f64::from(bbox.top()).floor() - grow).max(f64::from(room.top()).floor());
        let right = (f64::from(bbox.right()).ceil() + grow).min(f64::from(room.right()).ceil());
        let bottom = (f64::from(bbox.bottom()).ceil() + grow).min(f64::from(room.bottom()).ceil());
        let (width, height) = ((right - left).max(1.0), (bottom - top).max(1.0));
        let pixels = width * height;
        self.largest = self.largest.max(pixels);
        // The layer itself, and one more for a mask and for a clip.
        self.layers += pixels * (1.0 + f64::from(u8::from(group.mask().is_some())) + f64::from(u8::from(group.clip_path().is_some())));
        if filtered {
            let (sx, sy) = inner.get_scale();
            for filter in group.filters() {
                for primitive in filter.primitives() {
                    self.filters += pixels * primitive_cost(primitive.kind(), f64::from(sx), f64::from(sy));
                }
            }
        }
        // Inside the layer everything is drawn from the layer's corner.
        let inside = Transform::from_translate(-(left as f32), -(top as f32)).pre_concat(inner);
        // What masks and clips draw is drawn again for every group they apply to.
        if let Some(mask) = group.mask() {
            self.add(mask.root(), inside, room);
        }
        if let Some(clip) = group.clip_path() {
            self.add(clip.root(), inside, room);
        }
        self.children(group, inside, room);
    }

    fn children(&mut self, group: &usvg::Group, transform: Transform, room: Rect) {
        for child in group.children() {
            match child {
                usvg::Node::Group(g) => self.add(g, transform, room),
                usvg::Node::Text(t) => self.add(t.flattened(), transform, room),
                usvg::Node::Path(p) => {
                    self.dashes += dashes(p);
                    if p.stroke().is_some() {
                        self.reach = self.reach.max(reach(p, transform));
                    }
                }
                usvg::Node::Image(_) => {}
            }
        }
    }
}

/// How far a path lies from the panel's corner once it is drawn under `transform`, in pixels: the
/// largest coordinate of its box there. Not a number: beyond everything.
fn reach(path: &usvg::Path, transform: Transform) -> f64 {
    let Some(b) = path.data().bounds().transform(transform) else { return f64::INFINITY };
    let far = [b.left(), b.top(), b.right(), b.bottom()].into_iter().map(|v| f64::from(v.abs())).fold(0.0, f64::max);
    if far.is_finite() { far } else { f64::INFINITY }
}

/// The dashes a path's stroke is cut into, at most what tiny-skia cuts one path into (beyond that
/// it draws none): its length (that of its control points, which is never shorter) over the
/// length of its dash pattern, times the dashes in the pattern.
fn dashes(path: &usvg::Path) -> f64 {
    let Some(pattern) = path.stroke().and_then(|s| s.dasharray()) else { return 0.0 };
    let period: f64 = pattern.iter().map(|d| f64::from(*d)).sum();
    if !(period > 0.0) {
        return 0.0;
    }
    use resvg::tiny_skia::PathSegment;
    let (mut length, mut at, mut start) = (0.0, (0.0f64, 0.0f64), (0.0f64, 0.0f64));
    let mut step = |to: (f64, f64), at: &mut (f64, f64)| {
        length += (to.0 - at.0).hypot(to.1 - at.1);
        *at = to;
    };
    let point = |p: resvg::tiny_skia::Point| (f64::from(p.x), f64::from(p.y));
    for segment in path.data().segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                at = point(p);
                start = at;
            }
            PathSegment::LineTo(p) => step(point(p), &mut at),
            PathSegment::QuadTo(a, b) => {
                step(point(a), &mut at);
                step(point(b), &mut at);
            }
            PathSegment::CubicTo(a, b, c) => {
                step(point(a), &mut at);
                step(point(b), &mut at);
                step(point(c), &mut at);
            }
            PathSegment::Close => step(start, &mut at),
        }
    }
    (length / period * (pattern.len() / 2).max(1) as f64).min(1e6)
}

/// Dashes one picture may cut its strokes into: about 60 ms of drawing, for every panel.
const MAX_DASHES_DRAWN: f64 = 100_000.0;

/// What one primitive costs per pixel of its region, as measured: a unit is about half a
/// millisecond over a 264 px panel.
fn primitive_cost(kind: &usvg::filter::Kind, sx: f64, sy: f64) -> f64 {
    use usvg::filter::Kind;
    match kind {
        Kind::GaussianBlur(_) => 6.0,
        Kind::DropShadow(_) => 10.0,
        // Each of the two passes it is written as has one radius.
        Kind::Morphology(m) => {
            let rx = (f64::from(m.radius_x().get()) * sx).round();
            let ry = (f64::from(m.radius_y().get()) * sy).round();
            (2.0 * rx + 1.0) * (2.0 * ry + 1.0) / 4.0
        }
        Kind::Turbulence(t) => 6.0 * f64::from(t.num_octaves().max(1)),
        Kind::Merge(m) => m.inputs().len().max(1) as f64,
        Kind::DisplacementMap(_) => 2.0,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------- the model of a look

/// The elements a look may hold, with their SVG names. A literal table: no tag name ever comes
/// from a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    G,
    Path,
    Circle,
    Ellipse,
    Rect,
    Line,
    Polyline,
    Polygon,
    Text,
    Use,
    Defs,
    LinearGradient,
    RadialGradient,
    Stop,
    ClipPath,
    Mask,
    Filter,
    FeGaussianBlur,
    FeDropShadow,
    FeOffset,
    FeFlood,
    FeColorMatrix,
    FeComposite,
    FeBlend,
    FeMerge,
    FeMergeNode,
    FeMorphology,
    FeTurbulence,
    FeDisplacementMap,
}

const TAGS: [(&str, Tag); 29] = [
    ("g", Tag::G),
    ("path", Tag::Path),
    ("circle", Tag::Circle),
    ("ellipse", Tag::Ellipse),
    ("rect", Tag::Rect),
    ("line", Tag::Line),
    ("polyline", Tag::Polyline),
    ("polygon", Tag::Polygon),
    ("text", Tag::Text),
    ("use", Tag::Use),
    ("defs", Tag::Defs),
    ("linearGradient", Tag::LinearGradient),
    ("radialGradient", Tag::RadialGradient),
    ("stop", Tag::Stop),
    ("clipPath", Tag::ClipPath),
    ("mask", Tag::Mask),
    ("filter", Tag::Filter),
    ("feGaussianBlur", Tag::FeGaussianBlur),
    ("feDropShadow", Tag::FeDropShadow),
    ("feOffset", Tag::FeOffset),
    ("feFlood", Tag::FeFlood),
    ("feColorMatrix", Tag::FeColorMatrix),
    ("feComposite", Tag::FeComposite),
    ("feBlend", Tag::FeBlend),
    ("feMerge", Tag::FeMerge),
    ("feMergeNode", Tag::FeMergeNode),
    ("feMorphology", Tag::FeMorphology),
    ("feTurbulence", Tag::FeTurbulence),
    ("feDisplacementMap", Tag::FeDisplacementMap),
];

impl Tag {
    fn of(name: &str) -> Option<Tag> {
        TAGS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
    }

    fn name(self) -> &'static str {
        TAGS.iter().find(|(_, t)| *t == self).map_or("g", |(n, _)| n)
    }

    fn is_shape(self) -> bool {
        matches!(self, Tag::Path | Tag::Circle | Tag::Ellipse | Tag::Rect | Tag::Line | Tag::Polyline | Tag::Polygon)
    }

    /// What is drawn and can be a part: moved, named by a pivot, a role, a track or a drive.
    fn is_drawn(self) -> bool {
        self.is_shape() || matches!(self, Tag::G | Tag::Text | Tag::Use)
    }

    fn is_primitive(self) -> bool {
        matches!(
            self,
            Tag::FeGaussianBlur
                | Tag::FeDropShadow
                | Tag::FeOffset
                | Tag::FeFlood
                | Tag::FeColorMatrix
                | Tag::FeComposite
                | Tag::FeBlend
                | Tag::FeMerge
                | Tag::FeMorphology
                | Tag::FeTurbulence
                | Tag::FeDisplacementMap
        )
    }
}

/// What an element sits inside, as far as what it may hold and reference goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Drawn, or a thing to `use` inside `defs`.
    Art,
    Clip,
    Mask,
    Filter,
    Paint,
}

/// The content model: whether `tag` may sit in `parent` (`None`: the root).
fn allowed(parent: Option<Tag>, place: Place, tag: Tag) -> bool {
    match parent {
        None => tag.is_drawn() || tag == Tag::Defs,
        Some(Tag::G) if place == Place::Mask => tag == Tag::G || tag.is_shape(),
        Some(Tag::G) => tag.is_drawn() || tag == Tag::Defs,
        Some(Tag::Defs) => matches!(tag, Tag::LinearGradient | Tag::RadialGradient | Tag::ClipPath | Tag::Mask | Tag::Filter | Tag::G | Tag::Text) || tag.is_shape(),
        Some(Tag::LinearGradient | Tag::RadialGradient) => tag == Tag::Stop,
        Some(Tag::ClipPath) => tag.is_shape(),
        Some(Tag::Mask) => tag == Tag::G || tag.is_shape(),
        Some(Tag::Filter) => tag.is_primitive(),
        Some(Tag::FeMerge) => tag == Tag::FeMergeNode,
        Some(_) => false,
    }
}

/// One element of the artwork as read.
#[derive(Debug)]
struct El<'a> {
    tag: Tag,
    /// Printed only for the first element that carries it.
    id: Option<&'a str>,
    attrs: Option<&'a Map<String, Value>>,
    /// Its rest box in its own coordinates (its own transform excluded).
    bbox: Option<[f64; 4]>,
    text: Option<&'a str>,
    parent: Option<usize>,
    children: Vec<usize>,
    /// Its own `transform`.
    own: Option<Matrix>,
    place: Place,
    /// Inside `defs`, a `clipPath`, a `mask` or a `filter`: never moved.
    inert: bool,
    /// Elements in its subtree, itself included.
    size: usize,
    /// A `use` in its subtree, itself included.
    has_use: bool,
}

/// A stored look as read for drawing.
struct Model<'a> {
    look: &'a Map<String, Value>,
    view: [f64; 4],
    els: Vec<El<'a>>,
    roots: Vec<usize>,
    ids: HashMap<&'a str, usize>,
    /// Rest world matrix of each element: its ancestors' transforms and its own.
    rest_world: Vec<Matrix>,
    has_text: bool,
    /// A pose held a transform beyond what can be written (scales that multiply past a thousand
    /// million): set while a body is written.
    unwritable: std::cell::Cell<bool>,
    /// The most a panel of this picture cost, in panels: what a session is told beside it.
    spent: std::cell::Cell<Spent>,
}

/// What drawing cost, as parts of what a picture may cost.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spent {
    /// Passes of a cheap filter primitive over the whole figure, of [`FILTER_WORK`].
    pub filters: f64,
    /// Layers the size of the figure, of [`LAYER_WORK`].
    pub layers: f64,
}

impl<'a> Model<'a> {
    fn read(look: &'a Value) -> Result<Model<'a>, String> {
        let Value::Object(map) = look else {
            return Err("there is no look of its own to draw: this avatar shows Workbench's default look".into());
        };
        let view = map.get("viewBox").and_then(numbers4).filter(|v| (VIEW_MIN..=VIEW_MAX).contains(&v[2]) && (VIEW_MIN..=VIEW_MAX).contains(&v[3]));
        let view = view.ok_or("the stored look has no sound viewBox and cannot be drawn")?;
        let mut model = Model { look: map, view, els: vec![], roots: vec![], ids: HashMap::new(), rest_world: vec![], has_text: false, unwritable: std::cell::Cell::new(false), spent: std::cell::Cell::new(Spent::default()) };
        let mut budget = MAX_ELEMENTS;
        if let Some(nodes) = map.get("nodes").and_then(Value::as_array) {
            for node in nodes {
                if budget == 0 {
                    break;
                }
                if let Some(i) = model.add(node, None, Place::Art, false, 1, &mut budget) {
                    model.roots.push(i);
                }
            }
        }
        // Children come after their parent: sizes from the end, rest worlds from the start.
        for i in (0..model.els.len()).rev() {
            let (size, has_use) = model.els[i].children.iter().fold((1, model.els[i].tag == Tag::Use), |(s, u), &c| (s + model.els[c].size, u || model.els[c].has_use));
            model.els[i].size = size;
            model.els[i].has_use = has_use;
        }
        for i in 0..model.els.len() {
            let outer = model.els[i].parent.map_or(IDENTITY, |p| model.rest_world[p]);
            let world = model.els[i].own.map_or(outer, |own| mul(outer, own));
            model.rest_world.push(world);
        }
        Ok(model)
    }

    fn add(&mut self, node: &'a Value, parent: Option<usize>, place: Place, inert: bool, depth: usize, budget: &mut usize) -> Option<usize> {
        if depth > MAX_DEPTH || *budget == 0 {
            return None;
        }
        let obj = node.as_object()?;
        let tag = Tag::of(obj.get("tag")?.as_str()?)?;
        if !allowed(parent.map(|p| self.els[p].tag), place, tag) {
            return None;
        }
        *budget -= 1;
        let id = obj.get("id").and_then(Value::as_str).filter(|id| is_id(id) && !self.ids.contains_key(id));
        let attrs = obj.get("attrs").and_then(Value::as_object);
        let own = if tag.is_drawn() { attrs.and_then(|a| a.get("transform")).and_then(Value::as_str).and_then(parse_transform) } else { None };
        let text = if tag == Tag::Text { obj.get("text").and_then(Value::as_str) } else { None };
        self.has_text |= tag == Tag::Text;
        let index = self.els.len();
        self.els.push(El { tag, id, attrs, bbox: obj.get("box").and_then(numbers4), text, parent, children: vec![], own, place, inert, size: 1, has_use: false });
        if let Some(id) = id {
            self.ids.insert(id, index);
        }
        let (inner, inner_inert) = match tag {
            Tag::Defs => (Place::Art, true),
            Tag::ClipPath => (Place::Clip, true),
            Tag::Mask => (Place::Mask, true),
            Tag::Filter | Tag::FeMerge => (Place::Filter, true),
            Tag::LinearGradient | Tag::RadialGradient => (Place::Paint, true),
            _ => (place, inert),
        };
        if let Some(children) = obj.get("children").and_then(Value::as_array) {
            for child in children {
                if *budget == 0 {
                    break;
                }
                if let Some(c) = self.add(child, Some(index), inner, inner_inert, depth + 1, budget) {
                    self.els[index].children.push(c);
                }
            }
        }
        Some(index)
    }

    /// The element a part id names: drawn, and not inside `defs`, a `clipPath`, a `mask` or a `filter`.
    fn part(&self, id: &str) -> Option<usize> {
        let i = *self.ids.get(id)?;
        (self.els[i].tag.is_drawn() && !self.els[i].inert).then_some(i)
    }

    fn role(&self, role: &str) -> Option<usize> {
        self.look.get("roles")?.get(role)?.as_str().and_then(|id| self.part(id))
    }

    fn is_ancestor(&self, ancestor: usize, of: usize) -> bool {
        let mut at = self.els[of].parent;
        while let Some(p) = at {
            if p == ancestor {
                return true;
            }
            at = self.els[p].parent;
        }
        false
    }

    /// A part's rest box in viewBox coordinates.
    fn root_box(&self, i: usize) -> Option<[f64; 4]> {
        let b = box_through(self.rest_world[i], self.els[i].bbox?);
        b.iter().all(|n| n.is_finite()).then_some(b)
    }

    /// The square a small face shows: the head role's rest box with a tenth on each side, kept
    /// inside the viewBox; the whole viewBox for a look without a head (`faceViewBox` of look.ts).
    fn face_view(&self) -> [f64; 4] {
        let view = self.view;
        let Some(head) = self.role("head").and_then(|i| self.root_box(i)).filter(|b| b[2] > 0.0 && b[3] > 0.0) else { return view };
        // Never tighter than a tenth of the viewBox: a head that is a dot would otherwise be
        // drawn at thousands of pixels a unit.
        let side = (head[2].max(head[3]) * 1.2).max(FACE_LEAST * view[2].min(view[3])).min(view[2]).min(view[3]);
        if side.is_nan() || side <= 0.0 {
            return view;
        }
        let clamp = |centre: f64, from: f64, length: f64| (centre - side / 2.0).max(from).min(from + length - side);
        let round = |n: f64| (n * 1000.0).round() / 1000.0;
        [round(clamp(head[0] + head[2] / 2.0, view[0], view[2])), round(clamp(head[1] + head[3] / 2.0, view[1], view[3])), round(side), round(side)]
    }

    /// The square around a part, for a closer look: its rest box with a quarter added on each
    /// side, at least a tenth of the viewBox's smaller side, kept inside the viewBox.
    fn zoom_view(&self, id: &str) -> Result<[f64; 4], String> {
        let view = self.view;
        let part = self.ids.get(id).copied().filter(|i| self.els[*i].place == Place::Art);
        let Some(b) = part.and_then(|i| self.root_box(i)) else {
            return Err(format!("zoom names {}, which is no part of this look with a box: name the id of a part that is drawn", quote(id)));
        };
        let side = (b[2].max(b[3]) * 1.5).max(FACE_LEAST * view[2].min(view[3])).min(view[2]).min(view[3]);
        if side.is_nan() || side <= 0.0 {
            return Ok(view);
        }
        let clamp = |centre: f64, from: f64, length: f64| (centre - side / 2.0).max(from).min(from + length - side);
        let round = |n: f64| (n * 1000.0).round() / 1000.0;
        Ok([round(clamp(b[0] + b[2] / 2.0, view[0], view[2])), round(clamp(b[1] + b[3] / 2.0, view[1], view[3])), round(side), round(side)])
    }

    /// The point a part turns, skews and scales about, in its own coordinates: its pivot, else
    /// the centre of its box (for the body the middle of its lower edge), else the middle of the
    /// viewBox (its lower edge for the body).
    fn pivot(&self, i: usize, body: Option<usize>) -> (f64, f64) {
        let is_body = body == Some(i);
        if let Some(id) = self.els[i].id {
            if let Some(p) = self.look.get("pivots").and_then(|p| p.get(id)).and_then(Value::as_array) {
                if let (Some(x), Some(y), 2) = (p.first().and_then(finite), p.get(1).and_then(finite), p.len()) {
                    return (x, y);
                }
            }
        }
        let [vx, vy, vw, vh] = self.view;
        match self.els[i].bbox {
            Some([x, y, w, h]) => (x + w / 2.0, if is_body { y + h } else { y + h / 2.0 }),
            None => (vx + vw / 2.0, if is_body { vy + vh } else { vy + vh / 2.0 }),
        }
    }
}

// ---------------------------------------------------------------- the SVG text

/// How an attribute's value is checked and written again.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A number within the look's reach.
    Num,
    /// A number, never negative.
    Length,
    /// A number or a percentage.
    NumPct,
    /// A colour, `none` or `currentColor`.
    Paint,
    /// A colour or `currentColor`.
    Colour,
    /// References: written as `url(#id)` when `id` names an element of the right kind.
    GradientRef,
    ClipRef,
    MaskRef,
    FilterRef,
    /// The target of a `use`, written as `#id`.
    UseRef,
    Transform,
    PathData,
    Points,
    Choice(&'static [&'static str]),
    FontWeight,
    Dashes,
    /// Up to `most` numbers from `lo` to `hi` (`hi` a part of the viewBox width when `of_width`).
    List { most: usize, lo: f64, hi: f64, of_width: bool },
    /// A number whose size is at most this part of the viewBox width.
    OfWidth(f64),
    /// A filter input: `SourceGraphic`, `SourceAlpha` or the result of a primitive.
    Input,
    /// The name of a primitive's result.
    ResultName,
    /// `numOctaves`: an integer from 1 to 4.
    Octaves,
}

/// One attribute: its key in a stored node's `attrs`, the SVG name it is written as, its kind.
struct Spec {
    key: &'static str,
    svg: &'static str,
    kind: Kind,
}

const fn spec(key: &'static str, svg: &'static str, kind: Kind) -> Spec {
    Spec { key, svg, kind }
}

const UNITS: &[&str] = &["userSpaceOnUse", "objectBoundingBox"];

/// `g`, shapes, `text` and `use`. Their `transform` is written from the pose.
static PRESENTATION: [Spec; 18] = [
    spec("fill", "fill", Kind::Paint),
    spec("fillRef", "fill", Kind::GradientRef),
    spec("stroke", "stroke", Kind::Paint),
    spec("strokeRef", "stroke", Kind::GradientRef),
    spec("strokeWidth", "stroke-width", Kind::Length),
    spec("strokeLinecap", "stroke-linecap", Kind::Choice(&["butt", "round", "square"])),
    spec("strokeLinejoin", "stroke-linejoin", Kind::Choice(&["miter", "round", "bevel"])),
    spec("strokeMiterlimit", "stroke-miterlimit", Kind::Length),
    spec("strokeDasharray", "stroke-dasharray", Kind::Dashes),
    spec("strokeDashoffset", "stroke-dashoffset", Kind::Num),
    spec("fillOpacity", "fill-opacity", Kind::NumPct),
    spec("strokeOpacity", "stroke-opacity", Kind::NumPct),
    spec("opacity", "opacity", Kind::NumPct),
    spec("fillRule", "fill-rule", Kind::Choice(&["nonzero", "evenodd"])),
    spec("clipRule", "clip-rule", Kind::Choice(&["nonzero", "evenodd"])),
    spec("clipPathRef", "clip-path", Kind::ClipRef),
    spec("maskRef", "mask", Kind::MaskRef),
    spec("filterRef", "filter", Kind::FilterRef),
];
static PATH: [Spec; 1] = [spec("d", "d", Kind::PathData)];
static CIRCLE: [Spec; 3] = [spec("cx", "cx", Kind::Num), spec("cy", "cy", Kind::Num), spec("r", "r", Kind::Length)];
static ELLIPSE: [Spec; 4] = [spec("cx", "cx", Kind::Num), spec("cy", "cy", Kind::Num), spec("rx", "rx", Kind::Length), spec("ry", "ry", Kind::Length)];
static RECT: [Spec; 6] = [
    spec("x", "x", Kind::Num),
    spec("y", "y", Kind::Num),
    spec("width", "width", Kind::Length),
    spec("height", "height", Kind::Length),
    spec("rx", "rx", Kind::Length),
    spec("ry", "ry", Kind::Length),
];
static LINE: [Spec; 4] = [spec("x1", "x1", Kind::Num), spec("y1", "y1", Kind::Num), spec("x2", "x2", Kind::Num), spec("y2", "y2", Kind::Num)];
static POLY: [Spec; 1] = [spec("points", "points", Kind::Points)];
static TEXT: [Spec; 7] = [
    spec("x", "x", Kind::Num),
    spec("y", "y", Kind::Num),
    spec("fontSize", "font-size", Kind::Length),
    spec("fontWeight", "font-weight", Kind::FontWeight),
    spec("textAnchor", "text-anchor", Kind::Choice(&["start", "middle", "end"])),
    spec("dominantBaseline", "dominant-baseline", Kind::Choice(&["auto", "middle", "central", "hanging", "alphabetic"])),
    spec("letterSpacing", "letter-spacing", Kind::Num),
];
static USE: [Spec; 3] = [spec("useRef", "href", Kind::UseRef), spec("x", "x", Kind::Num), spec("y", "y", Kind::Num)];
static GRADIENT: [Spec; 3] = [
    spec("gradientUnits", "gradientUnits", Kind::Choice(UNITS)),
    spec("gradientTransform", "gradientTransform", Kind::Transform),
    spec("spreadMethod", "spreadMethod", Kind::Choice(&["pad", "reflect", "repeat"])),
];
static LINEAR: [Spec; 4] = [spec("x1", "x1", Kind::NumPct), spec("y1", "y1", Kind::NumPct), spec("x2", "x2", Kind::NumPct), spec("y2", "y2", Kind::NumPct)];
static RADIAL: [Spec; 5] = [spec("cx", "cx", Kind::NumPct), spec("cy", "cy", Kind::NumPct), spec("r", "r", Kind::NumPct), spec("fx", "fx", Kind::NumPct), spec("fy", "fy", Kind::NumPct)];
static STOP: [Spec; 3] = [spec("offset", "offset", Kind::NumPct), spec("stopColor", "stop-color", Kind::Colour), spec("stopOpacity", "stop-opacity", Kind::NumPct)];
static CLIP: [Spec; 1] = [spec("clipPathUnits", "clipPathUnits", Kind::Choice(UNITS))];
static MASK: [Spec; 7] = [
    spec("maskUnits", "maskUnits", Kind::Choice(UNITS)),
    spec("maskContentUnits", "maskContentUnits", Kind::Choice(UNITS)),
    spec("x", "x", Kind::NumPct),
    spec("y", "y", Kind::NumPct),
    spec("width", "width", Kind::NumPct),
    spec("height", "height", Kind::NumPct),
    spec("maskType", "mask-type", Kind::Choice(&["luminance", "alpha"])),
];
static FILTER: [Spec; 7] = [
    spec("x", "x", Kind::NumPct),
    spec("y", "y", Kind::NumPct),
    spec("width", "width", Kind::NumPct),
    spec("height", "height", Kind::NumPct),
    spec("filterUnits", "filterUnits", Kind::Choice(UNITS)),
    spec("primitiveUnits", "primitiveUnits", Kind::Choice(UNITS)),
    spec("colorInterpolationFilters", "color-interpolation-filters", Kind::Choice(&["sRGB", "linearRGB"])),
];
const STD_DEVIATION: Kind = Kind::List { most: 2, lo: 0.0, hi: 0.25, of_width: true };
static BLUR: [Spec; 3] = [spec("in", "in", Kind::Input), spec("stdDeviation", "stdDeviation", STD_DEVIATION), spec("result", "result", Kind::ResultName)];
static DROP_SHADOW: [Spec; 7] = [
    spec("in", "in", Kind::Input),
    spec("dx", "dx", Kind::Num),
    spec("dy", "dy", Kind::Num),
    spec("stdDeviation", "stdDeviation", STD_DEVIATION),
    spec("floodColor", "flood-color", Kind::Colour),
    spec("floodOpacity", "flood-opacity", Kind::NumPct),
    spec("result", "result", Kind::ResultName),
];
static OFFSET: [Spec; 4] = [spec("in", "in", Kind::Input), spec("dx", "dx", Kind::Num), spec("dy", "dy", Kind::Num), spec("result", "result", Kind::ResultName)];
static FLOOD: [Spec; 3] = [spec("floodColor", "flood-color", Kind::Colour), spec("floodOpacity", "flood-opacity", Kind::NumPct), spec("result", "result", Kind::ResultName)];
static COLOUR_MATRIX: [Spec; 4] = [
    spec("in", "in", Kind::Input),
    spec("type", "type", Kind::Choice(&["matrix", "saturate", "hueRotate", "luminanceToAlpha"])),
    spec("values", "values", Kind::List { most: MAX_COLOUR_MATRIX, lo: -NUM_MAX, hi: NUM_MAX, of_width: false }),
    spec("result", "result", Kind::ResultName),
];
static COMPOSITE: [Spec; 8] = [
    spec("in", "in", Kind::Input),
    spec("in2", "in2", Kind::Input),
    spec("operator", "operator", Kind::Choice(&["over", "in", "out", "atop", "xor", "arithmetic"])),
    spec("k1", "k1", Kind::Num),
    spec("k2", "k2", Kind::Num),
    spec("k3", "k3", Kind::Num),
    spec("k4", "k4", Kind::Num),
    spec("result", "result", Kind::ResultName),
];
static BLEND: [Spec; 4] = [
    spec("in", "in", Kind::Input),
    spec("in2", "in2", Kind::Input),
    spec(
        "mode",
        "mode",
        Kind::Choice(&["normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion"]),
    ),
    spec("result", "result", Kind::ResultName),
];
static MERGE: [Spec; 1] = [spec("result", "result", Kind::ResultName)];
static MERGE_NODE: [Spec; 1] = [spec("in", "in", Kind::Input)];
/// Read by [`Model::write_morphology`], which writes the primitive in two passes.
static MORPHOLOGY: [Spec; 4] = [
    spec("in", "in", Kind::Input),
    spec("operator", "operator", Kind::Choice(&["erode", "dilate"])),
    spec("radius", "radius", Kind::List { most: 2, lo: 0.0, hi: 0.1, of_width: true }),
    spec("result", "result", Kind::ResultName),
];
static TURBULENCE: [Spec; 5] = [
    spec("type", "type", Kind::Choice(&["fractalNoise", "turbulence"])),
    spec("baseFrequency", "baseFrequency", Kind::List { most: 2, lo: 0.0, hi: 1.0, of_width: false }),
    spec("numOctaves", "numOctaves", Kind::Octaves),
    spec("seed", "seed", Kind::Num),
    spec("result", "result", Kind::ResultName),
];
const CHANNELS: &[&str] = &["R", "G", "B", "A"];
static DISPLACEMENT: [Spec; 6] = [
    spec("in", "in", Kind::Input),
    spec("in2", "in2", Kind::Input),
    spec("scale", "scale", Kind::OfWidth(0.25)),
    spec("xChannelSelector", "xChannelSelector", Kind::Choice(CHANNELS)),
    spec("yChannelSelector", "yChannelSelector", Kind::Choice(CHANNELS)),
    spec("result", "result", Kind::ResultName),
];
static NONE: [Spec; 0] = [];

/// The attributes of a tag: the presentation attributes of what is drawn, then its own.
fn specs(tag: Tag) -> (&'static [Spec], &'static [Spec]) {
    match tag {
        Tag::G => (&PRESENTATION, &NONE),
        Tag::Path => (&PRESENTATION, &PATH),
        Tag::Circle => (&PRESENTATION, &CIRCLE),
        Tag::Ellipse => (&PRESENTATION, &ELLIPSE),
        Tag::Rect => (&PRESENTATION, &RECT),
        Tag::Line => (&PRESENTATION, &LINE),
        Tag::Polyline | Tag::Polygon => (&PRESENTATION, &POLY),
        Tag::Text => (&PRESENTATION, &TEXT),
        Tag::Use => (&PRESENTATION, &USE),
        Tag::Defs => (&NONE, &NONE),
        Tag::LinearGradient => (&GRADIENT, &LINEAR),
        Tag::RadialGradient => (&GRADIENT, &RADIAL),
        Tag::Stop => (&STOP, &NONE),
        Tag::ClipPath => (&CLIP, &NONE),
        Tag::Mask => (&MASK, &NONE),
        Tag::Filter => (&FILTER, &NONE),
        Tag::FeGaussianBlur => (&BLUR, &NONE),
        Tag::FeDropShadow => (&DROP_SHADOW, &NONE),
        Tag::FeOffset => (&OFFSET, &NONE),
        Tag::FeFlood => (&FLOOD, &NONE),
        Tag::FeColorMatrix => (&COLOUR_MATRIX, &NONE),
        Tag::FeComposite => (&COMPOSITE, &NONE),
        Tag::FeBlend => (&BLEND, &NONE),
        Tag::FeMerge => (&MERGE, &NONE),
        Tag::FeMergeNode => (&MERGE_NODE, &NONE),
        Tag::FeMorphology => (&MORPHOLOGY, &NONE),
        Tag::FeTurbulence => (&TURBULENCE, &NONE),
        Tag::FeDisplacementMap => (&DISPLACEMENT, &NONE),
    }
}

impl<'a> Model<'a> {
    /// The artwork in a pose, as the inside of an `<svg>` element.
    fn body(&self, posed: &Posed) -> String {
        let mut out = String::with_capacity(16 * 1024);
        // Elements written, each `use` counting with what it repeats, as the validator counts.
        let mut budget = MAX_ELEMENTS;
        let mut names = 0;
        for &root in &self.roots {
            self.write(root, posed, &mut out, &mut budget, &mut names);
        }
        out
    }

    fn write(&self, i: usize, posed: &Posed, out: &mut String, budget: &mut usize, names: &mut usize) {
        let el = &self.els[i];
        let cost = if el.tag == Tag::Use { 1 + self.use_target(i).map_or(0, |t| self.els[t].size) } else { 1 };
        if *budget < cost {
            return;
        }
        *budget -= cost;
        if el.tag == Tag::FeMorphology {
            self.write_morphology(i, out, names);
            return;
        }
        out.push('<');
        out.push_str(el.tag.name());
        if let Some(id) = el.id {
            attr(out, "id", id);
        }
        let set = posed.set.get(&i);
        let mut written: Vec<&str> = vec![];
        if el.tag.is_drawn() {
            // Nothing for the identity, as the engine writes nothing for it.
            if let Some(m) = posed.transform.get(i).copied().flatten().filter(|m| !close_to(*m, IDENTITY)) {
                match matrix_text(m) {
                    Some(text) => attr(out, "transform", &text),
                    // Left out, the part would be drawn somewhere else than the browser draws it.
                    None => self.unwritable.set(true),
                }
            }
        }
        // What the pose sets takes the place of the artwork's own value (and of a gradient).
        for (name, text) in set.into_iter().flatten() {
            attr(out, name, text);
            written.push(*name);
        }
        if el.tag == Tag::FeColorMatrix {
            if let Some(values) = el.attrs.and_then(saturate_beyond) {
                attr(out, "type", "matrix");
                attr(out, "values", &values);
                written.extend(["type", "values"]);
            }
        }
        let (common, own) = specs(el.tag);
        for spec in common.iter().chain(own) {
            if written.contains(&spec.svg) {
                continue;
            }
            let Some(value) = el.attrs.and_then(|a| a.get(spec.key)) else { continue };
            if let Some(text) = self.value_text(i, spec.kind, value) {
                attr(out, spec.svg, &text);
                written.push(spec.svg);
            }
        }
        if el.tag == Tag::Text {
            if !written.contains(&"font-size") {
                attr(out, "font-size", DEFAULT_FONT_SIZE);
            }
            attr(out, "font-family", "sans-serif");
            out.push('>');
            text_node(out, el.text.unwrap_or(""));
            out.push_str("</text>");
            return;
        }
        if el.children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        for &child in &el.children {
            self.write(child, posed, out, budget, names);
        }
        out.push_str("</");
        out.push_str(el.tag.name());
        out.push('>');
    }

    /// `feMorphology` in two passes, one along x and one along y. A square's minimum or maximum is
    /// the column's of the row's, so the picture is the same, and the work per pixel follows the
    /// radius instead of its square. Two radii of 0 leave the input as it is; with one, the
    /// browsers work along the other axis alone, and so does this.
    fn write_morphology(&self, i: usize, out: &mut String, names: &mut usize) {
        let attrs = self.els[i].attrs;
        let get = |key: &str, kind: Kind| attrs.and_then(|a| a.get(key)).and_then(|v| self.value_text(i, kind, v));
        let input = get("in", Kind::Input);
        let result = get("result", Kind::ResultName);
        let operator = get("operator", MORPHOLOGY[1].kind).unwrap_or_else(|| "erode".into());
        let radius: Vec<f64> = get("radius", MORPHOLOGY[2].kind).map(|r| r.split(' ').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
        let (rx, ry) = match radius.as_slice() {
            [r] => (*r, *r),
            [x, y] => (*x, *y),
            _ => (0.0, 0.0),
        };
        let open = |out: &mut String, tag: &str, input: Option<&str>| {
            out.push('<');
            out.push_str(tag);
            if let Some(input) = input {
                attr(out, "in", input);
            }
        };
        if rx <= 0.0 && ry <= 0.0 {
            open(out, "feOffset", input.as_deref());
            attr(out, "dx", "0");
            attr(out, "dy", "0");
        } else if rx <= 0.0 || ry <= 0.0 {
            // One radius of 0: the browsers work along the other axis alone.
            open(out, "feMorphology", input.as_deref());
            attr(out, "operator", &operator);
            let radius = if ry <= 0.0 { format!("{} 0.0001", num(rx).unwrap_or_default()) } else { format!("0.0001 {}", num(ry).unwrap_or_default()) };
            attr(out, "radius", &radius);
        } else {
            // A result name no look can give: theirs start with a letter.
            let between = format!("_m{names}");
            *names += 1;
            open(out, "feMorphology", input.as_deref());
            attr(out, "operator", &operator);
            attr(out, "radius", &format!("{} 0.0001", num(rx).unwrap_or_default()));
            attr(out, "result", &between);
            out.push_str("/>");
            open(out, "feMorphology", Some(&between));
            attr(out, "operator", &operator);
            attr(out, "radius", &format!("0.0001 {}", num(ry).unwrap_or_default()));
        }
        if let Some(result) = result {
            attr(out, "result", &result);
        }
        out.push_str("/>");
    }

    /// What a `use` repeats: a `g`, a shape or a `text` with an id, not inside a `clipPath`, a
    /// `mask` or a `filter`, not around the `use`, and with no `use` of its own inside.
    fn use_target(&self, i: usize) -> Option<usize> {
        let id = self.els[i].attrs?.get("useRef")?.as_str()?;
        let t = *self.ids.get(id)?;
        let target = &self.els[t];
        let fits = matches!(target.tag, Tag::G | Tag::Text) || target.tag.is_shape();
        (fits && target.place == Place::Art && !target.has_use && !self.is_ancestor(t, i)).then_some(t)
    }

    /// An attribute's value as written: parsed and printed again, or `None` when it does not fit.
    fn value_text(&self, i: usize, kind: Kind, value: &Value) -> Option<String> {
        let el = &self.els[i];
        let width = self.view[2];
        // Inside a clipPath or a mask nothing is clipped, masked or filtered again.
        let plain = matches!(el.place, Place::Clip | Place::Mask);
        match kind {
            Kind::Num => finite(value).and_then(num),
            Kind::Length => finite(value).filter(|v| *v >= 0.0).and_then(num),
            Kind::NumPct => match value {
                Value::String(s) if is_percent(s) => Some(s.clone()),
                _ => finite(value).and_then(num),
            },
            Kind::Paint => {
                let s = value.as_str()?;
                ["none", "currentColor"].into_iter().find(|k| *k == s).map(str::to_string).or_else(|| parse_hex(s).map(|_| s.to_string()))
            }
            Kind::Colour => {
                let s = value.as_str()?;
                if s == "currentColor" { Some("currentColor".into()) } else { parse_hex(s).map(|_| s.to_string()) }
            }
            Kind::GradientRef => self.reference(value, |t| matches!(t, Tag::LinearGradient | Tag::RadialGradient)),
            Kind::ClipRef if !plain => self.reference(value, |t| t == Tag::ClipPath),
            Kind::MaskRef if !plain => self.reference(value, |t| t == Tag::Mask),
            Kind::FilterRef if !plain => self.reference(value, |t| t == Tag::Filter),
            Kind::ClipRef | Kind::MaskRef | Kind::FilterRef => None,
            Kind::UseRef => self.use_target(i).and_then(|t| self.els[t].id).map(|id| format!("#{id}")),
            Kind::Transform => value.as_str().and_then(parse_transform).and_then(matrix_text),
            Kind::PathData => value.as_str().and_then(path_of).and_then(|p| p.text(&p.nums)),
            Kind::Points => value.as_str().filter(|s| s.len() <= MAX_POINTS_CHARS).and_then(|s| number_list(s, usize::MAX)).and_then(|l| join(&l)),
            Kind::Choice(choices) => value.as_str().and_then(|s| choices.iter().find(|c| **c == s)).map(|c| c.to_string()),
            Kind::FontWeight => {
                const WEIGHTS: [&str; 11] = ["normal", "bold", "100", "200", "300", "400", "500", "600", "700", "800", "900"];
                let s = match value {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    _ => return None,
                };
                WEIGHTS.iter().find(|w| **w == s).map(|w| w.to_string())
            }
            Kind::Dashes => match value {
                Value::String(s) if s == "none" => Some("none".into()),
                Value::String(s) => number_list(s, MAX_DASHES).filter(|l| !l.is_empty() && l.iter().all(|n| *n >= 0.0)).and_then(|l| join(&l)),
                _ => finite(value).filter(|v| *v >= 0.0).and_then(num),
            },
            Kind::List { most, lo, hi, of_width } => {
                let hi = if of_width { hi * width } else { hi };
                let list = match value {
                    Value::String(s) => number_list(s, most)?,
                    _ => vec![finite(value)?],
                };
                (!list.is_empty() && list.iter().all(|n| (lo..=hi).contains(n))).then(|| join(&list)).flatten()
            }
            Kind::OfWidth(part) => finite(value).filter(|v| v.abs() <= part * width).and_then(num),
            Kind::Input => {
                let s = value.as_str()?;
                ["SourceGraphic", "SourceAlpha"].into_iter().find(|k| *k == s).map(str::to_string).or_else(|| self.results_before(i).contains(&s).then(|| s.to_string()))
            }
            Kind::ResultName => value.as_str().filter(|s| is_result(s)).map(str::to_string),
            Kind::Octaves => value.as_u64().filter(|n| (1..=4).contains(n)).map(|n| n.to_string()),
        }
    }

    /// The results a primitive may take as an input: those of the primitives before it in its
    /// filter (for a `feMergeNode`, before its `feMerge`).
    fn results_before(&self, i: usize) -> Vec<&'a str> {
        let primitive = if self.els[i].tag == Tag::FeMergeNode { self.els[i].parent } else { Some(i) };
        let Some(primitive) = primitive else { return vec![] };
        let Some(filter) = self.els[primitive].parent.filter(|f| self.els[*f].tag == Tag::Filter) else { return vec![] };
        self.els[filter]
            .children
            .iter()
            .take_while(|c| **c != primitive)
            .filter_map(|c| self.els[*c].attrs?.get("result")?.as_str())
            .filter(|r| is_result(r))
            .collect()
    }

    /// `url(#id)` when `id` names an element this look prints whose tag fits.
    fn reference(&self, value: &Value, fits: impl Fn(Tag) -> bool) -> Option<String> {
        let id = value.as_str()?;
        let t = *self.ids.get(id)?;
        fits(self.els[t].tag).then(|| format!("url(#{id})"))
    }
}

/// One attribute, its value escaped. Every name is a literal of this file.
fn attr(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("='");
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            c if c.is_control() || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out.push('\'');
}

/// The content of a `text`: at most its 40 characters, escaped, without what XML cannot hold.
fn text_node(out: &mut String, text: &str) {
    for c in text.chars().take(MAX_TEXT_CHARS) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if c.is_control() || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
}

// ---------------------------------------------------------------- values

/// The id pattern of look.rs: a letter, then up to 31 letters, digits, `_` and `-`.
fn is_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 32 && b[0].is_ascii_alphabetic() && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

/// The name of a primitive's result: the id pattern, and none of SVG's names for an input.
fn is_result(s: &str) -> bool {
    is_id(s) && !["SourceGraphic", "SourceAlpha", "BackgroundImage", "BackgroundAlpha", "FillPaint", "StrokePaint"].contains(&s)
}

/// A name a session wrote, quoted for a sentence: cut, and with what is not plain escaped.
fn quote(s: &str) -> String {
    let cut: String = s.chars().take(32).collect();
    format!("{cut:?}")
}

fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && v.abs() <= NUM_MAX)
}

/// A `saturate` colour matrix whose amount lies outside 0 … 1, as the twenty numbers of the matrix
/// it stands for. The browsers take such an amount as it is (above 1 the colours grow stronger,
/// below 0 the hues turn over); usvg cuts it to 0 … 1, and does not touch a matrix.
fn saturate_beyond(attrs: &Map<String, Value>) -> Option<String> {
    if attrs.get("type").and_then(Value::as_str) != Some("saturate") {
        return None;
    }
    let s = match attrs.get("values")? {
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        other => finite(other)?,
    };
    if !s.is_finite() || (0.0..=1.0).contains(&s) {
        return None;
    }
    let rows = [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.0, 0.0],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.0, 0.0],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0],
    ];
    let numbers: Option<Vec<String>> = rows.iter().flatten().map(|n| num(*n)).collect();
    Some(numbers?.join(" "))
}

fn numbers4(value: &Value) -> Option<[f64; 4]> {
    let a = value.as_array().filter(|a| a.len() == 4)?;
    Some([finite(&a[0])?, finite(&a[1])?, finite(&a[2])?, finite(&a[3])?])
}

/// A number as written into the SVG text: fixed notation, at most six decimals, no exponent, no
/// "-0". `None` when it is not finite or beyond any coordinate.
fn num(v: f64) -> Option<String> {
    if !v.is_finite() || v.abs() >= 1e9 {
        return None;
    }
    let mut s = format!("{v:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    Some(if s == "-0" { "0".into() } else { s })
}

fn join(list: &[f64]) -> Option<String> {
    let parts: Option<Vec<String>> = list.iter().map(|n| num(*n)).collect();
    Some(parts?.join(" "))
}

/// `-12.5%` and the like, as look.ts accepts a percentage.
fn is_percent(s: &str) -> bool {
    let Some(body) = s.strip_suffix('%') else { return false };
    let body = body.strip_prefix('-').unwrap_or(body);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    (1..=4).contains(&int.len()) && int.bytes().all(|b| b.is_ascii_digit()) && (!body.contains('.') || (1..=3).contains(&frac.len())) && frac.bytes().all(|b| b.is_ascii_digit())
}

/// A number at `at`: `[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?`, and where it ends.
fn scan_number(b: &[u8], at: usize) -> Option<(f64, usize)> {
    let mut i = at;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int;
    if i < b.len() && b[i] == b'.' {
        let dot = i;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - dot - 1;
    }
    if digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp {
            i = j;
        }
    }
    let v: f64 = std::str::from_utf8(&b[at..i]).ok()?.parse().ok()?;
    (v.is_finite() && v.abs() <= NUM_MAX).then_some((if v == 0.0 { 0.0 } else { v }, i))
}

/// Numbers separated by white space or commas, at most `most` of them.
fn number_list(text: &str, most: usize) -> Option<Vec<f64>> {
    let mut out = vec![];
    for token in text.split(|c: char| c.is_ascii_whitespace() || c == ',').filter(|t| !t.is_empty()) {
        let (v, end) = scan_number(token.as_bytes(), 0)?;
        if end != token.len() || out.len() == most {
            return None;
        }
        out.push(v);
    }
    Some(out)
}

/// `#rrggbb` or `#rrggbbaa` as channels 0 to 255 and an alpha 0 to 1.
fn parse_hex(s: &str) -> Option<[f64; 4]> {
    let b = s.as_bytes();
    if !(b.len() == 7 || b.len() == 9) || b[0] != b'#' || !b[1..].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let n = |at: usize| f64::from(u8::from_str_radix(&s[at..at + 2], 16).unwrap_or(0));
    Some([n(1), n(3), n(5), if b.len() == 9 { n(7) / 255.0 } else { 1.0 }])
}

fn colour_text(c: [f64; 4]) -> Option<String> {
    if !c.iter().all(|v| v.is_finite()) {
        return None;
    }
    let channel = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    let alpha = (c[3].clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(if alpha == 255 {
        format!("#{:02x}{:02x}{:02x}", channel(c[0]), channel(c[1]), channel(c[2]))
    } else {
        format!("#{:02x}{:02x}{:02x}{alpha:02x}", channel(c[0]), channel(c[1]), channel(c[2]))
    })
}

// ---------------------------------------------------------------- transforms

/// An affine matrix `[a, b, c, d, e, f]`: x' = a x + c y + e, y' = b x + d y + f.
type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m` after `n`: the matrix that applies `n` first.
fn mul(m: Matrix, n: Matrix) -> Matrix {
    [m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1], m[0] * n[2] + m[2] * n[3], m[1] * n[2] + m[3] * n[3], m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5]]
}

/// The inverse, or `None` for a matrix that cannot be inverted or a number that is not finite.
fn invert(m: Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv = [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det];
    inv.iter().all(|v| v.is_finite()).then_some(inv)
}

/// Equal within 1e-9 in every entry.
fn close_to(m: Matrix, n: Matrix) -> bool {
    m.iter().zip(&n).all(|(a, b)| (a - b).abs() <= 1e-9)
}

fn translate(x: f64, y: f64) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

fn rotate(degrees: f64) -> Matrix {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [cos, sin, -sin, cos, 0.0, 0.0]
}

fn scale(sx: f64, sy: f64) -> Matrix {
    [sx, 0.0, 0.0, sy, 0.0, 0.0]
}

fn skew_x(degrees: f64) -> Matrix {
    [1.0, 0.0, degrees.to_radians().tan(), 1.0, 0.0, 0.0]
}

fn skew_y(degrees: f64) -> Matrix {
    [1.0, degrees.to_radians().tan(), 0.0, 1.0, 0.0, 0.0]
}

/// `m` about the point `(x, y)`.
fn about(x: f64, y: f64, m: Matrix) -> Matrix {
    mul(mul(translate(x, y), m), translate(-x, -y))
}

fn box_through(m: Matrix, b: [f64; 4]) -> [f64; 4] {
    let [x, y, w, h] = b;
    let corners = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)].map(|(px, py)| (m[0] * px + m[2] * py + m[4], m[1] * px + m[3] * py + m[5]));
    let x0 = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
    let y0 = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    let x1 = corners.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max);
    let y1 = corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max);
    [x0, y0, x1 - x0, y1 - y0]
}

fn matrix_text(m: Matrix) -> Option<String> {
    Some(format!("matrix({})", join(&m)?))
}

/// A transform list (`translate`, `rotate`, `scale`, `skewX`, `skewY`, `matrix` of numbers) as one
/// matrix, or `None` when it is anything else (as look.ts reads it).
fn parse_transform(text: &str) -> Option<Matrix> {
    if text.len() > MAX_TRANSFORM_CHARS {
        return None;
    }
    let b = text.as_bytes();
    let separator = |c: u8| c.is_ascii_whitespace() || c == b',';
    let mut m = IDENTITY;
    let mut at = 0;
    let mut count = 0;
    loop {
        while at < b.len() && separator(b[at]) {
            at += 1;
        }
        if at == b.len() {
            return Some(m);
        }
        let name = at;
        while at < b.len() && b[at].is_ascii_alphabetic() {
            at += 1;
        }
        let name = &text[name..at];
        while at < b.len() && b[at].is_ascii_whitespace() {
            at += 1;
        }
        if at == b.len() || b[at] != b'(' {
            return None;
        }
        let close = at + text[at..].find(')')?;
        let inner = &text[at + 1..close];
        if inner.contains('(') {
            return None;
        }
        let a = number_list(inner, 6)?;
        let op = match (name, a.as_slice()) {
            ("translate", [x]) => translate(*x, 0.0),
            ("translate", [x, y]) => translate(*x, *y),
            ("rotate", [r]) => rotate(*r),
            ("rotate", [r, x, y]) => about(*x, *y, rotate(*r)),
            ("scale", [s]) => scale(*s, *s),
            ("scale", [x, y]) => scale(*x, *y),
            ("skewX", [k]) => skew_x(*k),
            ("skewY", [k]) => skew_y(*k),
            ("matrix", [a0, a1, a2, a3, a4, a5]) => [*a0, *a1, *a2, *a3, *a4, *a5],
            _ => return None,
        };
        count += 1;
        if count > MAX_TRANSFORMS {
            return None;
        }
        m = mul(m, op);
        at = close + 1;
    }
}

// ---------------------------------------------------------------- paths

/// Path data as its tokens: a command letter, or a place where the next number goes. Arc flags are
/// numbers too, marked in `flags` and never interpolated.
#[derive(Debug, Clone, PartialEq)]
struct PathShape {
    tokens: Vec<Option<u8>>,
    nums: Vec<f64>,
    flags: Vec<bool>,
}

impl PathShape {
    /// Two paths morph into each other only with the same letters in the same order, as many
    /// numbers and equal arc flags.
    fn same(&self, other: &PathShape) -> bool {
        self.tokens == other.tokens && self.flags == other.flags && self.nums.iter().zip(&other.nums).zip(&self.flags).all(|((a, b), flag)| !flag || a == b)
    }

    /// The path written with `nums` in its places.
    fn text(&self, nums: &[f64]) -> Option<String> {
        let mut out = String::with_capacity(self.tokens.len() * 6);
        let mut n = 0;
        for token in &self.tokens {
            if !out.is_empty() {
                out.push(' ');
            }
            match token {
                Some(letter) => out.push(char::from(*letter)),
                None => {
                    let v = *nums.get(n)?;
                    if self.flags[n] {
                        out.push(if v >= 0.5 { '1' } else { '0' });
                    } else {
                        out.push_str(&num(v)?);
                    }
                    n += 1;
                }
            }
        }
        Some(out)
    }
}

/// Path data no longer than a look's, parsed.
fn path_of(text: &str) -> Option<PathShape> {
    (text.len() <= MAX_D_CHARS).then(|| parse_path(text)).flatten()
}

/// Path data split into commands and numbers, or `None` when it holds anything else.
fn parse_path(text: &str) -> Option<PathShape> {
    const COMMANDS: &[u8] = b"MmLlHhVvCcSsQqTtAaZz";
    let b = text.as_bytes();
    let mut shape = PathShape { tokens: vec![], nums: vec![], flags: vec![] };
    let (mut arc, mut in_arc, mut started) = (false, 0, false);
    let mut at = 0;
    while at < b.len() {
        let c = b[at];
        if matches!(c, b' ' | b',' | b'\t' | b'\n' | b'\r') {
            at += 1;
            continue;
        }
        if COMMANDS.contains(&c) {
            shape.tokens.push(Some(c));
            arc = c == b'A' || c == b'a';
            in_arc = 0;
            started = true;
            at += 1;
            continue;
        }
        if !started {
            return None;
        }
        // The two flags of an arc are single digits and may be written without a separator.
        if arc && (in_arc % 7 == 3 || in_arc % 7 == 4) {
            if c != b'0' && c != b'1' {
                return None;
            }
            shape.tokens.push(None);
            shape.nums.push(f64::from(c - b'0'));
            shape.flags.push(true);
            in_arc += 1;
            at += 1;
            continue;
        }
        let (v, end) = scan_number(b, at)?;
        shape.tokens.push(None);
        shape.nums.push(v);
        shape.flags.push(false);
        if arc {
            in_arc += 1;
        }
        at = end;
    }
    Some(shape)
}

// ---------------------------------------------------------------- props and eases

/// What a track or a drive moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Prop {
    TranslateX,
    TranslateY,
    Rotate,
    SkewX,
    SkewY,
    Scale,
    ScaleX,
    ScaleY,
    Opacity,
    FillOpacity,
    StrokeOpacity,
    Fill,
    Stroke,
    StrokeWidth,
    StrokeDashoffset,
    D,
    Cx,
    Cy,
    R,
    Rx,
    Ry,
    X,
    Y,
    Width,
    Height,
    X1,
    Y1,
    X2,
    Y2,
}

/// The prop's name on the wire, which is also its key in a node's `attrs`, and the SVG attribute
/// an absolute prop is written as (empty for the relative ones).
const PROPS: [(&str, Prop, &str); 29] = [
    ("translateX", Prop::TranslateX, ""),
    ("translateY", Prop::TranslateY, ""),
    ("rotate", Prop::Rotate, ""),
    ("skewX", Prop::SkewX, ""),
    ("skewY", Prop::SkewY, ""),
    ("scale", Prop::Scale, ""),
    ("scaleX", Prop::ScaleX, ""),
    ("scaleY", Prop::ScaleY, ""),
    ("opacity", Prop::Opacity, "opacity"),
    ("fillOpacity", Prop::FillOpacity, "fill-opacity"),
    ("strokeOpacity", Prop::StrokeOpacity, "stroke-opacity"),
    ("fill", Prop::Fill, "fill"),
    ("stroke", Prop::Stroke, "stroke"),
    ("strokeWidth", Prop::StrokeWidth, "stroke-width"),
    ("strokeDashoffset", Prop::StrokeDashoffset, "stroke-dashoffset"),
    ("d", Prop::D, "d"),
    ("cx", Prop::Cx, "cx"),
    ("cy", Prop::Cy, "cy"),
    ("r", Prop::R, "r"),
    ("rx", Prop::Rx, "rx"),
    ("ry", Prop::Ry, "ry"),
    ("x", Prop::X, "x"),
    ("y", Prop::Y, "y"),
    ("width", Prop::Width, "width"),
    ("height", Prop::Height, "height"),
    ("x1", Prop::X1, "x1"),
    ("y1", Prop::Y1, "y1"),
    ("x2", Prop::X2, "x2"),
    ("y2", Prop::Y2, "y2"),
];

/// The channels of a pose: translation, rotation, skew, scale.
const TX: usize = 0;
const TY: usize = 1;
const ROT: usize = 2;
const SKX: usize = 3;
const SKY: usize = 4;
const SX: usize = 5;
const SY: usize = 6;

impl Prop {
    fn of(name: &str) -> Option<Prop> {
        PROPS.iter().find(|(n, _, _)| *n == name).map(|(_, p, _)| *p)
    }

    fn key(self) -> &'static str {
        PROPS.iter().find(|(_, p, _)| *p == self).map_or("", |(n, _, _)| n)
    }

    fn svg(self) -> &'static str {
        PROPS.iter().find(|(_, p, _)| *p == self).map_or("", |(_, _, s)| s)
    }

    /// The channels a relative prop feeds; empty for an absolute one.
    fn channels(self) -> &'static [usize] {
        match self {
            Prop::TranslateX => &[TX],
            Prop::TranslateY => &[TY],
            Prop::Rotate => &[ROT],
            Prop::SkewX => &[SKX],
            Prop::SkewY => &[SKY],
            Prop::Scale => &[SX, SY],
            Prop::ScaleX => &[SX],
            Prop::ScaleY => &[SY],
            _ => &[],
        }
    }

    fn is_scale(self) -> bool {
        matches!(self, Prop::Scale | Prop::ScaleX | Prop::ScaleY)
    }

    /// A size that is never negative: a stroke's width, a radius, a width, a height.
    fn is_size(self) -> bool {
        matches!(self, Prop::StrokeWidth | Prop::R | Prop::Rx | Prop::Ry | Prop::Width | Prop::Height)
    }

    fn is_colour(self) -> bool {
        matches!(self, Prop::Fill | Prop::Stroke)
    }

    /// Whether an element of this kind has the prop.
    fn fits(self, tag: Tag) -> bool {
        match self {
            Prop::D => tag == Tag::Path,
            Prop::Cx | Prop::Cy => matches!(tag, Tag::Circle | Tag::Ellipse),
            Prop::R => tag == Tag::Circle,
            Prop::Rx | Prop::Ry => matches!(tag, Tag::Ellipse | Tag::Rect),
            Prop::X | Prop::Y => matches!(tag, Tag::Rect | Tag::Text | Tag::Use),
            Prop::Width | Prop::Height => tag == Tag::Rect,
            Prop::X1 | Prop::Y1 | Prop::X2 | Prop::Y2 => tag == Tag::Line,
            _ => tag.is_drawn(),
        }
    }

    /// What a numeric prop is where the artwork sets none.
    fn default_value(self) -> f64 {
        if matches!(self, Prop::Opacity | Prop::FillOpacity | Prop::StrokeOpacity | Prop::StrokeWidth) { 1.0 } else { 0.0 }
    }

    /// Inherited from the elements around it when the element sets none.
    fn inherits(self) -> bool {
        matches!(self, Prop::StrokeWidth | Prop::FillOpacity | Prop::StrokeOpacity | Prop::StrokeDashoffset)
    }
}

/// How one segment of keyframes eases, as lib/engine.ts eases it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ease {
    Linear,
    In,
    Out,
    InOut,
    OutBack,
    Step,
    InBack,
    InOutBack,
    OutElastic,
    OutBounce,
    Bezier([f64; 4]),
}

const EASES: [(&str, Ease); 10] = [
    ("linear", Ease::Linear),
    ("in", Ease::In),
    ("out", Ease::Out),
    ("inOut", Ease::InOut),
    ("outBack", Ease::OutBack),
    ("step", Ease::Step),
    ("inBack", Ease::InBack),
    ("inOutBack", Ease::InOutBack),
    ("outElastic", Ease::OutElastic),
    ("outBounce", Ease::OutBounce),
];

impl Ease {
    /// A name, or a cubic bezier as four numbers `[x1, y1, x2, y2]`, 0 ≤ x ≤ 1 and -2 ≤ y ≤ 3.
    fn of(value: &Value) -> Option<Ease> {
        match value {
            Value::String(name) => EASES.iter().find(|(n, _)| n == name).map(|(_, e)| *e),
            Value::Array(a) if a.len() == 4 => {
                let n: Vec<f64> = a.iter().map(finite).collect::<Option<_>>()?;
                let ok = (0.0..=1.0).contains(&n[0]) && (0.0..=1.0).contains(&n[2]) && (-2.0..=3.0).contains(&n[1]) && (-2.0..=3.0).contains(&n[3]);
                ok.then(|| Ease::Bezier([n[0], n[1], n[2], n[3]]))
            }
            _ => None,
        }
    }

    /// One eased step of a segment, 0 to 1 in.
    fn at(self, t: f64) -> f64 {
        const C1: f64 = 1.70158;
        match self {
            Ease::Linear => t,
            Ease::In => t * t,
            Ease::Out => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::InOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Ease::OutBack => {
                let u = t - 1.0;
                1.0 + (C1 + 1.0) * u * u * u + C1 * u * u
            }
            Ease::Step => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Ease::InBack => (C1 + 1.0) * t * t * t - C1 * t * t,
            Ease::InOutBack => {
                let c2 = C1 * 1.525;
                if t < 0.5 {
                    (2.0 * t).powi(2) * ((c2 + 1.0) * 2.0 * t - c2) / 2.0
                } else {
                    ((2.0 * t - 2.0).powi(2) * ((c2 + 1.0) * (2.0 * t - 2.0) + c2) + 2.0) / 2.0
                }
            }
            Ease::OutElastic => {
                if t <= 0.0 {
                    0.0
                } else if t >= 1.0 {
                    1.0
                } else {
                    2f64.powf(-10.0 * t) * ((10.0 * t - 0.75) * (2.0 * std::f64::consts::PI / 3.0)).sin() + 1.0
                }
            }
            Ease::OutBounce => {
                const N1: f64 = 7.5625;
                const D1: f64 = 2.75;
                if t < 1.0 / D1 {
                    N1 * t * t
                } else if t < 2.0 / D1 {
                    let u = t - 1.5 / D1;
                    N1 * u * u + 0.75
                } else if t < 2.5 / D1 {
                    let u = t - 2.25 / D1;
                    N1 * u * u + 0.9375
                } else {
                    let u = t - 2.625 / D1;
                    N1 * u * u + 0.984375
                }
            }
            Ease::Bezier([x1, y1, x2, y2]) => {
                let curve = |s: f64, p1: f64, p2: f64| 3.0 * (1.0 - s).powi(2) * s * p1 + 3.0 * (1.0 - s) * s * s * p2 + s * s * s;
                let slope = |s: f64| 3.0 * (1.0 - s).powi(2) * x1 + 6.0 * (1.0 - s) * s * (x2 - x1) + 3.0 * s * s * (1.0 - x2);
                // Newton from s = t; a step that leaves 0 … 1, a flat slope or a result that is
                // still off hands over to bisection, which always lands (x rises with s).
                let mut s = t;
                let mut newton = true;
                for _ in 0..8 {
                    let d = slope(s);
                    if d.abs() < 1e-6 {
                        newton = false;
                        break;
                    }
                    s -= (curve(s, x1, x2) - t) / d;
                    if !(0.0..=1.0).contains(&s) {
                        newton = false;
                        break;
                    }
                }
                if !newton || (curve(s, x1, x2) - t).abs() > 1e-5 {
                    let (mut lo, mut hi) = (0.0, 1.0);
                    for _ in 0..30 {
                        let mid = (lo + hi) / 2.0;
                        if curve(mid, x1, x2) < t {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    s = (lo + hi) / 2.0;
                }
                curve(s, y1, y2)
            }
        }
    }
}

/// The ease of each of `segments` segments: the first of `candidates` that gives them one (a name
/// or four numbers for all, or a list with one per segment), else linear.
fn eases(candidates: &[Option<&Value>], segments: usize) -> Vec<Ease> {
    for value in candidates.iter().flatten() {
        if let Some(e) = Ease::of(value) {
            return vec![e; segments];
        }
        if let Value::Array(list) = value {
            if list.len() == segments {
                if let Some(each) = list.iter().map(Ease::of).collect::<Option<Vec<_>>>() {
                    return each;
                }
            }
        }
    }
    vec![Ease::Linear; segments]
}

/// The times of `n` keyframes: the first of `candidates` that has `n` rising numbers from 0 to 1,
/// else even.
fn times(candidates: &[Option<&Value>], n: usize) -> Vec<f64> {
    for value in candidates.iter().flatten() {
        let Some(list) = value.as_array().filter(|l| l.len() == n) else { continue };
        let Some(t) = list.iter().map(finite).collect::<Option<Vec<f64>>>() else { continue };
        if t.first() == Some(&0.0) && t.last() == Some(&1.0) && t.windows(2).all(|w| w[1] > w[0]) {
            return t;
        }
    }
    (0..n).map(|i| if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 }).collect()
}

// ---------------------------------------------------------------- posing

/// An absolute prop's value in a pose.
#[derive(Debug, Clone, PartialEq)]
enum Abs {
    Num(f64),
    Colour([f64; 4]),
    D(PathShape, Vec<f64>),
}

/// The pose of one part while it is worked out: its channels (translation, rotation, skew and
/// scale) and the absolute props set on it.
#[derive(Debug, Clone)]
struct Acc {
    ch: [f64; 7],
    abs: Vec<(Prop, Abs)>,
}

impl Acc {
    fn new() -> Acc {
        Acc { ch: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0], abs: vec![] }
    }

    fn set(&mut self, prop: Prop, value: Abs) {
        match self.abs.iter_mut().find(|(p, _)| *p == prop) {
            Some(slot) => slot.1 = value,
            None => self.abs.push((prop, value)),
        }
    }

    /// A relative value: translation, rotation and skew add, scales multiply.
    fn add(&mut self, prop: Prop, v: f64) {
        for &c in prop.channels() {
            if c >= SX {
                self.ch[c] *= v;
            } else {
                self.ch[c] += v;
            }
        }
    }
}

/// One keyframe as read.
#[derive(Debug, Clone)]
enum Key {
    /// The artwork's own value.
    Null,
    Num(f64),
    Colour([f64; 4]),
    Path(PathShape),
}

/// Keyframes of one prop with their times and eases: a track's, or a drive's along its input.
struct Keys {
    prop: Prop,
    keys: Vec<Key>,
    times: Vec<f64>,
    eases: Vec<Ease>,
}

/// What sampling keyframes for one part gives.
enum Got {
    Rel(f64),
    Abs(Abs),
}

/// What a pose writes: per element the transform it carries (`None`: none), and the attributes the
/// pose sets on a part, by SVG name.
#[derive(Debug, Default)]
struct Posed {
    transform: Vec<Option<Matrix>>,
    set: HashMap<usize, Vec<(&'static str, String)>>,
}

/// One of the look's own animations, as a pose plays it.
struct Anim<'a> {
    duration: f64,
    looping: bool,
    map: &'a Map<String, Value>,
}

const INPUTS: [&str; 6] = ["lookX", "lookY", "mouth", "pace", "turn", "held"];

impl<'a> Model<'a> {
    /// One of the look's own animations by name, or the sentence that says why there is none.
    fn animation(&self, name: &str) -> Result<Anim<'a>, String> {
        let own = self.look.get("animations").and_then(Value::as_object);
        if let Some(map) = own.filter(|_| is_id(name)).and_then(|a| a.get(name)).and_then(Value::as_object) {
            let duration = map.get("duration").and_then(finite).filter(|d| (0.05..=60.0).contains(d));
            let duration = duration.ok_or_else(|| format!("the animation {name} has no sound duration and cannot be drawn"))?;
            return Ok(Anim { duration, looping: map.get("loop") == Some(&Value::Bool(true)) && map.get("every").is_none_or(Value::is_null), map });
        }
        if BUILTIN.contains(&name) {
            return Err(format!("{name} is one of Workbench's built-in animations, which are not drawn here: define your own to see it"));
        }
        let names: Vec<&str> = own.map(|a| a.keys().map(String::as_str).filter(|n| is_id(n)).take(MAX_ANIMATIONS).collect()).unwrap_or_default();
        if names.is_empty() {
            Err(format!("no animation {} in this look: it has no animations of its own", quote(name)))
        } else {
            Err(format!("no animation {} in this look: its own animations are {}", quote(name), names.join(", ")))
        }
    }

    /// Everything a pose changes, as `lib/engine.ts` samples it with this one animation playing as
    /// a state animation and these inputs.
    fn pose(&self, pose: &Pose) -> Result<Posed, String> {
        let mut accs: HashMap<usize, Acc> = HashMap::new();
        let body = self.role("body");

        if let Some((name, at)) = &pose.animation {
            let anim = self.animation(name)?;
            self.play(&anim, at.clamp(0.0, 1.0), &mut accs);
        }

        let inputs = pose.inputs;
        // Where it looks is a vector at most one long, as the browser's is.
        let far = inputs.look_x.hypot(inputs.look_y).max(1.0);
        let input = |name: &str| match name {
            "lookX" => inputs.look_x / far,
            "lookY" => inputs.look_y / far,
            "mouth" => inputs.mouth.unwrap_or(0.0),
            "pace" => inputs.pace,
            "turn" => inputs.turn,
            _ => inputs.held,
        };
        for drive in self.look.get("drives").and_then(Value::as_array).into_iter().flatten().take(MAX_DRIVES).filter_map(Value::as_object) {
            let Some(name) = drive.get("input").and_then(Value::as_str).and_then(|n| INPUTS.iter().find(|i| **i == n)) else { continue };
            // Where along the input's range the drive is: -1 … 1 maps to 0 … 1 for the gaze and
            // the turn, the others run from 0 to 1 already.
            let position = match *name {
                "lookX" | "lookY" | "turn" => (input(name).clamp(-1.0, 1.0) + 1.0) / 2.0,
                other => input(other).clamp(0.0, 1.0),
            };
            let Some(keys) = keys(drive, &[drive.get("at")], &[drive.get("ease")]) else { continue };
            for target in self.targets(drive.get("target"), keys.prop) {
                match self.sample(&keys, target, position) {
                    Some(Got::Rel(v)) => accs.entry(target).or_insert_with(Acc::new).add(keys.prop, v),
                    Some(Got::Abs(v)) => accs.entry(target).or_insert_with(Acc::new).set(keys.prop, v),
                    None => {}
                }
            }
        }

        if let Some(open) = inputs.mouth {
            self.speak(open.clamp(0.0, 1.0), &mut accs);
        }

        if let Some(pupils) = self.role("pupils") {
            let width = self.view[2];
            let travel = match self.look.get("gaze").and_then(finite) {
                Some(g) => g.clamp(0.0, width * GAZE_MOST),
                None => width * GAZE_DEFAULT,
            };
            let (x, y) = (inputs.look_x, inputs.look_y);
            if travel > 0.0 && x.is_finite() && y.is_finite() && (x != 0.0 || y != 0.0) {
                let k = travel / x.hypot(y).max(1.0);
                let acc = accs.entry(pupils).or_insert_with(Acc::new);
                acc.ch[TX] += x * k;
                acc.ch[TY] += y * k;
            }
        }

        Ok(self.settle(&accs, body))
    }

    /// The animation as a state animation at `at` of its run: a loop over its duration (each track
    /// shifted by its `phase`), a one-shot or a recurring one over its duration and its longest
    /// `delay`, a track holding its last keyframe once it has played, as in a gesture. (In a
    /// state's list the browser lets each track go back to rest when it ends.)
    fn play(&self, anim: &Anim<'_>, at: f64, accs: &mut HashMap<usize, Acc>) {
        let tracks: Vec<&Map<String, Value>> = anim.map.get("tracks").and_then(Value::as_array).into_iter().flatten().take(MAX_TRACKS).filter_map(Value::as_object).collect();
        let delay = |t: &Map<String, Value>| t.get("delay").and_then(finite).filter(|d| *d > 0.0).unwrap_or(0.0);
        let run = anim.duration + tracks.iter().map(|t| delay(t)).fold(0.0, f64::max);
        for track in tracks {
            let p = if anim.looping {
                let phase = track.get("phase").and_then(finite).unwrap_or(0.0).clamp(0.0, 1.0);
                let p = at + phase;
                p - p.floor()
            } else {
                let local = at * run - delay(track);
                // Before its delay a track changes nothing; once it has played it holds its last
                // keyframe until the whole run is over, as a gesture does (avatar_act, a line's
                // gesture, a greeting: how a session's own one-shots are mostly played).
                if local < 0.0 {
                    continue;
                }
                (local / anim.duration).min(1.0)
            };
            let Some(keys) = keys(track, &[track.get("times"), anim.map.get("times")], &[track.get("ease"), anim.map.get("ease")]) else { continue };
            for target in self.targets(track.get("target"), keys.prop) {
                match self.sample(&keys, target, p) {
                    Some(Got::Rel(v)) => accs.entry(target).or_insert_with(Acc::new).add(keys.prop, v),
                    Some(Got::Abs(v)) => accs.entry(target).or_insert_with(Acc::new).set(keys.prop, v),
                    None => {}
                }
            }
        }
    }

    /// The look's `mouth` entries at an openness: a path between its closed and its open form, a
    /// number between two values, a channel set to one between two.
    fn speak(&self, open: f64, accs: &mut HashMap<usize, Acc>) {
        for drive in self.look.get("mouth").and_then(Value::as_array).into_iter().flatten().take(MAX_MOUTH_DRIVES) {
            let Some(target) = drive.get("target").and_then(Value::as_str).and_then(|id| self.part(id)) else { continue };
            let lerp = |a: f64, b: f64| a + (b - a) * open;
            match drive.get("prop").and_then(Value::as_str) {
                Some(name) => {
                    let Some(prop) = Prop::of(name).filter(|p| matches!(p, Prop::ScaleY | Prop::ScaleX | Prop::Scale | Prop::TranslateY | Prop::Ry | Prop::R | Prop::Height)) else { continue };
                    let Some(to) = drive.get("open").and_then(finite) else { continue };
                    if !prop.fits(self.els[target].tag) {
                        continue;
                    }
                    let acc = accs.entry(target).or_insert_with(Acc::new);
                    if prop.channels().is_empty() {
                        let from = drive.get("closed").and_then(finite).unwrap_or_else(|| self.rest_num(target, prop));
                        acc.set(prop, Abs::Num(lerp(from, to)));
                    } else {
                        let from = drive.get("closed").and_then(finite).unwrap_or(if prop.is_scale() { 1.0 } else { 0.0 });
                        for &c in prop.channels() {
                            acc.ch[c] = lerp(from, to);
                        }
                    }
                }
                None => {
                    if self.els[target].tag != Tag::Path {
                        continue;
                    }
                    let Some(to) = drive.get("open").and_then(Value::as_str).and_then(path_of) else { continue };
                    let from = match drive.get("closed").and_then(Value::as_str) {
                        Some(closed) => path_of(closed),
                        None => self.rest_path(target),
                    };
                    let Some(from) = from.filter(|f| f.same(&to)) else { continue };
                    let nums = from.nums.iter().zip(&to.nums).zip(&from.flags).map(|((a, b), flag)| if *flag { *a } else { lerp(*a, *b) }).collect();
                    accs.entry(target).or_insert_with(Acc::new).set(Prop::D, Abs::D(from, nums));
                }
            }
        }
    }

    /// The parts a track or a drive names that have its prop, each once.
    fn targets(&self, named: Option<&Value>, prop: Prop) -> Vec<usize> {
        let ids: Vec<&str> = match named {
            Some(Value::String(id)) => vec![id.as_str()],
            Some(Value::Array(list)) => list.iter().take(MAX_TARGETS).filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let mut out = vec![];
        for id in ids {
            if let Some(i) = self.part(id).filter(|i| prop.fits(self.els[*i].tag)) {
                if !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out
    }

    /// Keyframes sampled for one part at `p` (0 to 1): the artwork's own value where a keyframe is
    /// null, the segment found, eased and interpolated. `None` when there is nothing to show.
    fn sample(&self, keys: &Keys, target: usize, p: f64) -> Option<Got> {
        let n = keys.keys.len();
        let mut i = 0;
        while i + 2 < n && p >= keys.times[i + 1] {
            i += 1;
        }
        let span = keys.times[i + 1] - keys.times[i];
        let f = keys.eases.get(i).copied().unwrap_or(Ease::Linear).at(if span > 0.0 { ((p - keys.times[i]) / span).clamp(0.0, 1.0) } else { 1.0 });
        let lerp = |a: f64, b: f64| a + (b - a) * f;
        if keys.prop.is_colour() {
            let rest = self.rest_colour(target, keys.prop);
            let resolve = |at: usize| -> Option<[f64; 4]> {
                match &keys.keys[at] {
                    Key::Colour(c) => Some(*c),
                    _ => match rest {
                        RestColour::Rgba(c) => Some(c),
                        // No plain colour to return to: hold the neighbouring keyframe (from nothing: transparent).
                        other => nearest(&keys.keys, at, |k| if let Key::Colour(c) = k { Some(*c) } else { None }).map(|c| if other == RestColour::Nothing { [c[0], c[1], c[2], 0.0] } else { c }),
                    },
                }
            };
            let (a, b) = (resolve(i)?, resolve(i + 1)?);
            return Some(Got::Abs(Abs::Colour([lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2]), lerp(a[3], b[3])])));
        }
        if keys.prop == Prop::D {
            let shape = keys.keys.iter().find_map(|k| if let Key::Path(s) = k { Some(s.clone()) } else { None })?;
            let rest = self.rest_path(target).filter(|r| r.same(&shape));
            let resolve = |at: usize| -> Option<Vec<f64>> {
                match &keys.keys[at] {
                    Key::Path(s) => Some(s.nums.clone()),
                    _ => rest.as_ref().map(|r| r.nums.clone()).or_else(|| nearest(&keys.keys, at, |k| if let Key::Path(s) = k { Some(s.nums.clone()) } else { None })),
                }
            };
            let (a, b) = (resolve(i)?, resolve(i + 1)?);
            let nums = a.iter().zip(&b).zip(&shape.flags).map(|((x, y), flag)| if *flag { *x } else { lerp(*x, *y) }).collect();
            return Some(Got::Abs(Abs::D(shape, nums)));
        }
        let relative = !keys.prop.channels().is_empty();
        let base = if relative {
            if keys.prop.is_scale() { 1.0 } else { 0.0 }
        } else {
            self.rest_num(target, keys.prop)
        };
        let value = |at: usize| match keys.keys[at] {
            Key::Num(v) => v,
            _ => base,
        };
        let v = lerp(value(i), value(i + 1));
        // An ease that overshoots takes no size below nothing (the browser would then draw the
        // inherited stroke, or a circle for an ellipse).
        let v = if keys.prop.is_size() { v.max(0.0) } else { v };
        Some(if relative { Got::Rel(v) } else { Got::Abs(Abs::Num(v)) })
    }

    /// The nearest element, from the part outwards, whose attributes hold one of these keys, with
    /// the key it holds and its value.
    fn inherited(&self, i: usize, keys: &[&'static str]) -> Option<(&'static str, &'a Value)> {
        let mut at = Some(i);
        while let Some(e) = at {
            if let Some(attrs) = self.els[e].attrs {
                for key in keys {
                    if let Some(v) = attrs.get(*key) {
                        return Some((key, v));
                    }
                }
            }
            at = self.els[e].parent;
        }
        None
    }

    fn rest_num(&self, i: usize, prop: Prop) -> f64 {
        let numeric = |v: &Value| match v {
            Value::String(s) if is_percent(s) => s[..s.len() - 1].parse::<f64>().ok().map(|n| n / 100.0),
            _ => finite(v),
        };
        let own = self.els[i].attrs.and_then(|a| a.get(prop.key())).and_then(numeric);
        let from = || if prop.inherits() { self.inherited(i, &[prop.key()]).and_then(|(_, v)| numeric(v)) } else { None };
        own.or_else(from).unwrap_or(prop.default_value())
    }

    fn rest_path(&self, i: usize) -> Option<PathShape> {
        self.els[i].attrs?.get("d")?.as_str().and_then(path_of)
    }

    /// The artwork's own fill or stroke of a part, as far as a colour track can return to it.
    fn rest_colour(&self, i: usize, prop: Prop) -> RestColour {
        let (key, reference) = if prop == Prop::Fill { ("fill", "fillRef") } else { ("stroke", "strokeRef") };
        match self.inherited(i, &[key, reference]) {
            Some((k, Value::String(s))) if k == key && s == "none" => RestColour::Nothing,
            Some((k, Value::String(s))) if k == key => parse_hex(s).map_or(RestColour::Unknown, RestColour::Rgba),
            Some(_) => RestColour::Unknown,
            None if prop == Prop::Fill => RestColour::Rgba([0.0, 0.0, 0.0, 1.0]),
            None => RestColour::Nothing,
        }
    }

    /// The poses turned into what is written: each moved part's matrix (its own transform, then
    /// translate, rotate, skew and scale about its pivot) and, for a part with a parent, the
    /// matrix that carries it along; the absolute props as text. A part whose numbers are not
    /// sound is drawn as it rests.
    fn settle(&self, accs: &HashMap<usize, Acc>, body: Option<usize>) -> Posed {
        let mut motion: HashMap<usize, Matrix> = HashMap::new();
        let mut unsound: HashSet<usize> = HashSet::new();
        let mut set: HashMap<usize, Vec<(&'static str, String)>> = HashMap::new();
        for (&i, acc) in accs {
            let ch = acc.ch;
            if !ch.iter().all(|v| v.is_finite() && v.abs() < 1e15) {
                unsound.insert(i);
                continue;
            }
            let (px, py) = self.pivot(i, body);
            let mut m = translate(ch[TX], ch[TY]);
            m = mul(m, about(px, py, rotate(ch[ROT])));
            m = mul(m, about(px, py, mul(skew_x(ch[SKX]), skew_y(ch[SKY]))));
            m = mul(m, about(px, py, scale(ch[SX], ch[SY])));
            let mut texts = vec![];
            for (prop, value) in &acc.abs {
                let text = match value {
                    Abs::Num(v) => num(*v),
                    Abs::Colour(c) => colour_text(*c),
                    Abs::D(shape, nums) => shape.text(nums),
                };
                match text {
                    Some(t) => texts.push((prop.svg(), t)),
                    None => {
                        unsound.insert(i);
                        break;
                    }
                }
            }
            if unsound.contains(&i) {
                continue;
            }
            motion.insert(i, m);
            if !texts.is_empty() {
                set.insert(i, texts);
            }
        }

        let parents = self.parents();
        unsound.extend(self.parent_loops(&parents));
        let mut frame = Frame { model: self, motion: &motion, parents: &parents, unsound, x: vec![None; self.els.len()], busy: vec![false; self.els.len()], world: vec![None; self.els.len()] };
        let mut transform = Vec::with_capacity(self.els.len());
        for i in 0..self.els.len() {
            let moved = motion.contains_key(&i) || parents.contains_key(&i);
            transform.push(if moved { Some(frame.x(i)) } else { self.els[i].own });
        }
        for i in &frame.unsound {
            set.remove(i);
            transform[*i] = self.els[*i].own;
        }
        Posed { transform, set }
    }

    /// `parents` of the look, part to part: what is not two parts, a part and itself, or one
    /// nested in the other is left out.
    fn parents(&self) -> HashMap<usize, usize> {
        let mut out = HashMap::new();
        let Some(map) = self.look.get("parents").and_then(Value::as_object) else { return out };
        for (child, parent) in map.iter().take(MAX_PARENTS) {
            let (Some(c), Some(p)) = (self.part(child), parent.as_str().and_then(|id| self.part(id))) else { continue };
            if c != p && !self.is_ancestor(c, p) && !self.is_ancestor(p, c) {
                out.insert(c, p);
            }
        }
        out
    }

    /// The parts whose `parents` go round in a loop, through each other or through what they are
    /// drawn in (`{a: c, c: b}` with `b` inside `a`; a file changed by hand): they rest. A part's
    /// matrix needs those of the parts with a parent around it, of its parent and of the parts with
    /// a parent around that; a part that needs itself that way is in a loop.
    fn parent_loops(&self, parents: &HashMap<usize, usize>) -> HashSet<usize> {
        let around = |i: usize| {
            let mut out = vec![];
            let mut at = self.els[i].parent;
            while let Some(p) = at {
                out.push(p);
                at = self.els[p].parent;
            }
            out
        };
        let needs: HashMap<usize, Vec<usize>> = parents
            .iter()
            .map(|(&child, &parent)| (child, around(child).into_iter().chain([parent]).chain(around(parent)).filter(|n| parents.contains_key(n)).collect()))
            .collect();
        parents
            .keys()
            .copied()
            .filter(|&start| {
                let mut seen = HashSet::new();
                let mut todo = needs[&start].clone();
                while let Some(n) = todo.pop() {
                    if n == start {
                        return true;
                    }
                    if seen.insert(n) {
                        todo.extend(needs.get(&n).into_iter().flatten());
                    }
                }
                false
            })
            .collect()
    }
}

/// The nearest keyframe on either side of `at` that `pick` takes, the earlier one first.
fn nearest<T>(keys: &[Key], at: usize, pick: impl Fn(&Key) -> Option<T>) -> Option<T> {
    for d in 1..keys.len() {
        if let Some(v) = at.checked_sub(d).and_then(|b| pick(&keys[b])) {
            return Some(v);
        }
        if let Some(v) = keys.get(at + d).and_then(&pick) {
            return Some(v);
        }
    }
    None
}

/// The artwork's own paint, as a colour track sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RestColour {
    Rgba([f64; 4]),
    /// `none`: a colour comes from transparent.
    Nothing,
    /// A gradient or `currentColor`: no colour to return to.
    Unknown,
}

/// The keyframes of a track or a drive: its prop, its values (2 to 16; colours for `fill` and
/// `stroke`, path data of one structure for `d`, numbers otherwise, null for the artwork's own),
/// its times and eases. `None` when they do not fit.
fn keys(item: &Map<String, Value>, time_candidates: &[Option<&Value>], ease_candidates: &[Option<&Value>]) -> Option<Keys> {
    let prop = Prop::of(item.get("prop")?.as_str()?)?;
    let values = item.get("values")?.as_array().filter(|v| (2..=MAX_KEYFRAMES).contains(&v.len()))?;
    let mut keys = Vec::with_capacity(values.len());
    let mut shape: Option<PathShape> = None;
    for v in values {
        let key = match v {
            Value::Null => Key::Null,
            Value::String(s) if prop.is_colour() => Key::Colour(parse_hex(s)?),
            Value::String(s) if prop == Prop::D => {
                let path = path_of(s)?;
                if shape.as_ref().is_some_and(|first| !first.same(&path)) {
                    return None;
                }
                shape.get_or_insert_with(|| path.clone());
                Key::Path(path)
            }
            _ if prop.is_colour() || prop == Prop::D => return None,
            _ => Key::Num(finite(v)?),
        };
        keys.push(key);
    }
    let n = keys.len();
    Some(Keys { prop, keys, times: times(time_candidates, n), eases: eases(ease_candidates, n - 1) })
}

/// The matrices of one frame, worked out once per element: `X(E)`, the matrix written on E, and
/// its live world `W(E)`.
struct Frame<'m, 'a> {
    model: &'m Model<'a>,
    motion: &'m HashMap<usize, Matrix>,
    parents: &'m HashMap<usize, usize>,
    unsound: HashSet<usize>,
    x: Vec<Option<Matrix>>,
    busy: Vec<bool>,
    world: Vec<Option<Matrix>>,
}

impl Frame<'_, '_> {
    fn own(&self, i: usize) -> Matrix {
        self.model.els[i].own.unwrap_or(IDENTITY)
    }

    /// `X(E)`: `T(E) = S(E)·M(E)`, and with a parent Q, `C·T(E)` where `C = P⁻¹·D(Q)·Rp`: `P` the
    /// live world of E's DOM parent, `Rp` that parent's rest world, and `D(Q) = W(Q)·R(Q)⁻¹` how
    /// far Q is from where it rests. So E's world is `D(Q)·Rp·T(E)`: E moves as if it were drawn
    /// inside Q, and what moves the groups E is really drawn in does not move it a second time.
    fn x(&mut self, i: usize) -> Matrix {
        if let Some(m) = self.x[i] {
            return m;
        }
        if self.busy[i] || self.unsound.contains(&i) {
            // A part that carries itself round a loop of parents (a file changed by hand) rests.
            self.unsound.insert(i);
            return self.own(i);
        }
        self.busy[i] = true;
        let t = mul(self.own(i), self.motion.get(&i).copied().unwrap_or(IDENTITY));
        let x = match self.parents.get(&i).copied() {
            None => t,
            Some(q) => {
                let p = self.parent_world(i);
                let rp = self.model.els[i].parent.map_or(IDENTITY, |parent| self.model.rest_world[parent]);
                let wq = self.world(q);
                match (invert(p), invert(self.model.rest_world[q])) {
                    (Some(p_inv), Some(rq_inv)) => {
                        let c = mul(p_inv, mul(mul(wq, rq_inv), rp));
                        if c.iter().all(|v| v.is_finite()) { mul(c, t) } else { self.rest(i) }
                    }
                    _ => self.rest(i),
                }
            }
        };
        let x = if self.unsound.contains(&i) { self.own(i) } else { x };
        self.busy[i] = false;
        self.x[i] = Some(x);
        x
    }

    fn rest(&mut self, i: usize) -> Matrix {
        self.unsound.insert(i);
        self.own(i)
    }

    fn world(&mut self, i: usize) -> Matrix {
        if let Some(m) = self.world[i] {
            return m;
        }
        let w = mul(self.parent_world(i), self.x(i));
        self.world[i] = Some(w);
        w
    }

    fn parent_world(&mut self, i: usize) -> Matrix {
        match self.model.els[i].parent {
            Some(p) => self.world(p),
            None => IDENTITY,
        }
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;

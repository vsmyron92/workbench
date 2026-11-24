//! The look of an avatar: the pure validator (OWNER: avatar slice).
//!
//! An agent's artwork is untrusted markup, and it never reaches a browser as markup. [`apply`]
//! reads the SVG subset with quick-xml into a closed, typed, bounded tree ([`Look`]): element
//! names and attribute names come from the tables in this file, every value is parsed and
//! written again by this file, references are bare ids that resolve inside the same document.
//! Only that tree is stored and sent; the browser rebuilds it once more from literal tables.
//!
//! Besides the artwork a look holds what moves it: pivots, roles (so Workbench's built-in motion
//! can drive any character), the agent's own animations, the state map, mouth drives, gaze,
//! parents (a part that moves with another part it is not drawn inside) and drives (the live
//! inputs, where the figure looks, how fast it walks, moving parts). All of it is checked against
//! the artwork in one pass, and a call is refused whole: the problems come back grouped by kind,
//! each with a count, the first place and what is accepted.
//!
//! Three classes of input: **kept** (the tables below: shapes, groups, text, `use` of an element
//! of the same artwork, gradients, clip paths, masks and filters of a closed set of primitives),
//! **dropped with a warning** that names and counts it (SMIL, `font-family`, `class`…), and
//! **refused** (scripts, links, images, anything that reaches outside the document, anything not
//! named here, any limit exceeded).
//!
//! Everything here is a pure function of its arguments: no state, no I/O, no clock. No input
//! makes it panic, recursion never follows the input (the tree is walked with explicit stacks and
//! its depth is limited while it is read), and counts are limited while reading, so hostile input
//! costs time in proportion to its size.
//!
//! Public surface: the wire types [`Look`], [`LookNode`], [`Animation`], [`Track`],
//! [`MouthDrive`], [`Voice`]; [`apply`] (the design call; [`apply_masked`] with the session's
//! secret masking), [`check_stored`] (the same validator for a stored file), [`to_svg`] (the
//! artwork written back as SVG), [`parts`], [`builtins`], [`closes`], [`message`], and the `MAX_*` limits.
//! The web's `lib/look.ts` mirrors the limits: its test reads the `pub const MAX_*: usize = N;`
//! lines of this file, so they stay one per line with a literal.
//!
//! Some points go beyond the letter of the contract, all on the safe side:
//!
//! - The box of an arc is its real extent, not its end points widened by the radii: a head drawn
//!   as two arcs would otherwise be reported twice as wide as it is, and as reaching outside the
//!   viewBox.
//! - A track lists at most [`MAX_TARGETS`] parts, because every count has a bound.
//! - Where path data puts its pen is a coordinate like any other: relative steps that add up to a
//!   point outside the reach of the viewBox refuse, although each step is small.
//! - A part keeps within a million units, as stored and through the transforms above it, and so
//!   does its pivot: every scale is bounded, their product is not, and the browser refuses a look
//!   with a larger number.
//! - Two `d` values are one structure only when each command carries as many numbers: the
//!   browser morphs token by token.
//! - A loop's `fill`, `stroke` and `opacity` tracks end on the value they start with: where a
//!   loop starts again its last keyframe meets its first with no time between them, which the
//!   0.17 s rule is there to prevent.
//! - The artwork keeps to [`MAX_SVG_BYTES`] in the form it is stored and given back in as well,
//!   so that what [`to_svg`] writes can always be sent again, and a stored file holds no more
//!   artwork than a call may send.
//! - A prefixed attribute refuses also on a `tspan` and behind a name that would be dropped
//!   (`data-x:onload`).
//! - A `use` that repeats what comes after it makes the artwork be read twice: the first reading
//!   learns what the element is, the second counts and places it where the `use` stands.
//! - A mask holds `g` and shapes in all its depth: no `text`, `use` or `defs`, at any level.
//! - The `values` of a colour matrix are as many as its `type` takes; a `result` is never named
//!   after an input SVG gives by name; an input of another filter is no input.
//! - A circle of `parents` may run through what a part is drawn inside, and refuses as one.
//! - A drive keeps the paint rule of a track: no `step` on fill, stroke and the opacities. Its
//!   `d` keyframes count toward [`MAX_D_KEYFRAMES_TOTAL`] with those of tracks and the mouth.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{self, Write as _};

use quick_xml::errors::{Error as XmlError, IllFormedError};
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use super::text;

// ---------------------------------------------------------------- limits

/// The arguments of one design call, as JSON.
pub const MAX_DESIGN_BYTES: usize = 262144;
/// The `svg` text of one design call.
pub const MAX_SVG_BYTES: usize = 32768;
/// A stored look, as JSON.
pub const MAX_LOOK_BYTES: usize = 131072;
/// Elements in the artwork, dropped ones included.
pub const MAX_ELEMENTS: usize = 300;
/// How deep elements nest below the `svg` root.
pub const MAX_DEPTH: usize = 10;
/// Attributes on one element.
pub const MAX_ATTRS: usize = 32;
/// Stops in one gradient.
pub const MAX_STOPS: usize = 16;
/// `text` elements in the artwork.
pub const MAX_TEXTS: usize = 6;
/// Characters in one `text` element.
pub const MAX_TEXT_CHARS: usize = 40;
/// Characters of one `d`, as written and as stored.
pub const MAX_D_CHARS: usize = 4000;
/// Characters of one `points`, as written and as stored.
pub const MAX_POINTS_CHARS: usize = 2000;
/// Functions in one `transform`.
pub const MAX_TRANSFORMS: usize = 8;
/// Characters of one `transform` as written (what is stored is shorter).
pub const MAX_TRANSFORM_CHARS: usize = 1024;
/// Characters of an id or an animation name.
pub const MAX_ID_CHARS: usize = 32;
/// Characters of the avatar's name.
pub const MAX_NAME_CHARS: usize = 32;
/// Animations of one look.
pub const MAX_ANIMATIONS: usize = 32;
/// Tracks of one animation.
pub const MAX_TRACKS: usize = 16;
/// Keyframes of one track.
pub const MAX_KEYFRAMES: usize = 16;
/// Targets one track may list.
pub const MAX_TARGETS: usize = 16;
/// Characters of one `d` keyframe (a track's value, a mouth's `open` or `closed`).
pub const MAX_D_KEYFRAME_CHARS: usize = 1000;
/// Characters of all `d` keyframes of one look together.
pub const MAX_D_KEYFRAMES_TOTAL: usize = 6000;
/// Names in one state's list.
pub const MAX_STATE_NAMES: usize = 6;
/// Mouth drives of one look.
pub const MAX_MOUTH_DRIVES: usize = 4;
/// `filter` elements in the artwork.
pub const MAX_FILTERS: usize = 8;
/// Primitives of one `filter`, its `feMergeNode` elements not counted.
pub const MAX_FILTER_PRIMITIVES: usize = 8;
/// Numbers of one `stroke-dasharray`.
pub const MAX_DASHES: usize = 8;
/// Entries of `parents`.
pub const MAX_PARENTS: usize = 64;
/// Drives of one look.
pub const MAX_DRIVES: usize = 16;
/// Fragments in `voice.prefer`.
pub const MAX_VOICE_PREFER: usize = 5;
/// Characters of one `voice.prefer` fragment.
pub const MAX_VOICE_PREFER_CHARS: usize = 48;
/// Kinds of problems a refusal lists before "and N more kinds".
pub const MAX_PROBLEM_KINDS: usize = 12;
/// Characters of one problem or warning.
pub const MAX_PROBLEM_CHARS: usize = 160;
/// Characters of quoted input inside a problem.
pub const MAX_QUOTE_CHARS: usize = 40;
/// Bytes of the joined problems ([`message`]).
pub const MAX_MESSAGE_BYTES: usize = 3000;
/// Warnings one answer carries.
pub const MAX_WARNINGS: usize = 12;

/// The viewBox when the root gives none.
const DEFAULT_VIEW: View = View { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
/// Width and height of a viewBox.
const MIN_VIEW: f64 = 16.0;
const MAX_VIEW: f64 = 4096.0;
/// Coordinates, lengths and animated numbers lie within this many times the larger side of the viewBox.
const REACH: f64 = 4.0;
/// Scale factors of a `transform`, in magnitude.
const MIN_SCALE: f64 = 0.001;
const MAX_SCALE: f64 = 1000.0;
/// Angles of `skewX` and `skewY`, in degrees: beyond this the tangent runs away.
const MAX_SKEW: f64 = 80.0;
/// Angles of `rotate`, in a transform and in a track.
const MAX_ROTATE: f64 = 3600.0;
/// `scale`, `scaleX` and `scaleY` keyframes.
const MAX_TRACK_SCALE: f64 = 10.0;
/// Seconds an animation lasts.
const MIN_DURATION: f64 = 0.1;
const MAX_DURATION: f64 = 30.0;
/// Seconds between the starts of an `every` animation.
const MIN_EVERY: f64 = 1.0;
const MAX_EVERY: f64 = 120.0;
/// Seconds between two keyframes of a `fill`, `stroke` or `opacity` track (flash safety).
const MIN_PAINT_STEP: f64 = 0.17;
/// Seconds between two keyframes of any other track.
const MIN_STEP: f64 = 0.04;
/// The smallest step of `times`.
const MIN_TIME_STEP: f64 = 0.001;
/// Pupil travel: the share of the viewBox width `gaze` may reach.
const MAX_GAZE_SHARE: f64 = 0.05;
/// `voice.pitch` and `voice.rate`.
const PITCH: (f64, f64) = (0.7, 1.4);
const RATE: (f64, f64) = (0.8, 1.3);
/// `stroke-miterlimit`.
const MITER: (f64, f64) = (1.0, 100.0);
/// A gradient coordinate in `objectBoundingBox` units, as a fraction of the box.
const MAX_BOX_FRACTION: f64 = 10.0;
/// How far a box may reach from the origin, and how large it may be: as it is stored, and at the
/// root through every transform above it. Each number a design writes is bounded by its viewBox;
/// transforms multiply and relative steps add up, so what follows from them has a bound of its
/// own. The browser refuses a look that holds a number larger than this.
const MAX_SPAN: f64 = 1_000_000.0;
/// The lint: a part smaller than this in both directions will not be seen.
const SMALL_PART: f64 = 2.0;
/// The lint: how far a box may reach over the edge of the viewBox before it is reported.
const EDGE_SLACK: f64 = 0.5;
/// A text box is estimated: this share of the font size per character.
const TEXT_ADVANCE: f64 = 0.6;
/// The share of the font size above the baseline.
const TEXT_ASCENT: f64 = 0.8;
/// The font size of a `text` without one.
const DEFAULT_FONT_SIZE: f64 = 16.0;
/// Names listed in one phrase of a problem, a warning, `kept` or `cleared`.
const LISTED: usize = 12;
/// `feMergeNode` elements of one `feMerge`.
const MAX_MERGE_NODES: usize = 8;
/// Numbers of a `feColorMatrix`'s `values`: a matrix of four rows of five.
const MATRIX_VALUES: usize = 20;
/// The links of one chain of `parents`, from a part to the part at its top.
const MAX_CHAIN: usize = 8;
/// `k1` to `k4` of `feComposite`, in magnitude.
const MAX_FACTOR: f64 = 100.0;
/// The `seed` of `feTurbulence`, in magnitude.
const MAX_SEED: f64 = 100_000.0;
/// `numOctaves` of `feTurbulence`.
const OCTAVES: (f64, f64) = (1.0, 4.0);
/// The x and y bezier control points of an ease.
const BEZIER_X: (f64, f64) = (0.0, 1.0);
const BEZIER_Y: (f64, f64) = (-2.0, 3.0);

/// Never an id and never an animation name: the browser looks parts and animations up by name,
/// and these are names every JavaScript object answers to.
pub(super) const RESERVED: [&str; 10] = ["constructor", "prototype", "toString", "toLocaleString", "valueOf", "hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable", "all", "none"];

/// Workbench's built-in motion, by name. The browser generates it for any look from its roles.
pub const BUILTIN: [&str; 11] = ["breathe", "blink", "think", "talk", "listen", "alert", "doze", "wave", "cheer", "nod", "shake"];

// ---------------------------------------------------------------- wire types

/// A number of a stored look: finite, rounded to three decimals, never negative zero. An integral
/// value is written as a JSON integer (`6`, never `6.0`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Num(f64);

impl Num {
    pub fn new(v: f64) -> Num {
        Num(round3(v))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl From<Num> for f64 {
    fn from(n: Num) -> f64 {
        n.0
    }
}

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let v = self.0;
        if v.fract() == 0.0 && v.abs() < 9.0e15 { s.serialize_i64(v as i64) } else { s.serialize_f64(v) }
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Num, D::Error> {
        let v = f64::deserialize(d)?;
        if v.is_finite() { Ok(Num::new(v)) } else { Err(serde::de::Error::custom("a finite number")) }
    }
}

/// The validated design: the artwork tree with boxes, and what moves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Look {
    /// `[x, y, width, height]`.
    pub view_box: [Num; 4],
    pub nodes: Vec<LookNode>,
    /// Per part id: the point it turns and scales about, in the coordinates its geometry is written in.
    pub pivots: BTreeMap<String, [Num; 2]>,
    pub roles: BTreeMap<Role, String>,
    pub animations: BTreeMap<String, Animation>,
    pub states: BTreeMap<LookState, Vec<String>>,
    /// 0 to [`MAX_MOUTH_DRIVES`] drives that share one openness.
    pub mouth: Vec<MouthDrive>,
    /// Pupil travel in viewBox units; `None`: 3% of the viewBox width.
    pub gaze: Option<Num>,
    /// Child part id to parent part id: the child moves with everything that moves the parent,
    /// as if it were drawn inside it, and keeps its place in the drawing order. Written only
    /// when it holds an entry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", deserialize_with = "or_default")]
    pub parents: BTreeMap<String, String>,
    /// 0 to [`MAX_DRIVES`]: the live inputs moving parts. Written only when there is one.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "or_default")]
    pub drives: Vec<Drive>,
}

/// A field that may also be `null`, which is its default.
fn or_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One element of the artwork.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LookNode {
    pub tag: Tag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Keys from the closed table only, under their React SVG prop names. References are bare ids
    /// under `fillRef`, `strokeRef`, `clipPathRef`, `maskRef`, `filterRef` and, on a `use`,
    /// `useRef`. Number lists are strings of numbers separated by single spaces.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, AttrValue>,
    /// Every node with an id: its rest-pose box `[x, y, w, h]` in the coordinates its own geometry
    /// is written in (its own transform excluded).
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[Num; 4]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<LookNode>,
    /// The content of a `text` element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The elements a look may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tag {
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

impl Tag {
    pub const ALL: [Tag; 29] = [
        Tag::G, Tag::Path, Tag::Circle, Tag::Ellipse, Tag::Rect, Tag::Line, Tag::Polyline, Tag::Polygon, Tag::Text, Tag::Use, Tag::Defs, Tag::LinearGradient, Tag::RadialGradient, Tag::Stop,
        Tag::ClipPath, Tag::Mask, Tag::Filter, Tag::FeGaussianBlur, Tag::FeDropShadow, Tag::FeOffset, Tag::FeFlood, Tag::FeColorMatrix, Tag::FeComposite, Tag::FeBlend, Tag::FeMerge,
        Tag::FeMergeNode, Tag::FeMorphology, Tag::FeTurbulence, Tag::FeDisplacementMap,
    ];

    /// The element name, as written in SVG and as stored.
    pub fn name(self) -> &'static str {
        match self {
            Tag::G => "g",
            Tag::Path => "path",
            Tag::Circle => "circle",
            Tag::Ellipse => "ellipse",
            Tag::Rect => "rect",
            Tag::Line => "line",
            Tag::Polyline => "polyline",
            Tag::Polygon => "polygon",
            Tag::Text => "text",
            Tag::Use => "use",
            Tag::Defs => "defs",
            Tag::LinearGradient => "linearGradient",
            Tag::RadialGradient => "radialGradient",
            Tag::Stop => "stop",
            Tag::ClipPath => "clipPath",
            Tag::Mask => "mask",
            Tag::Filter => "filter",
            Tag::FeGaussianBlur => "feGaussianBlur",
            Tag::FeDropShadow => "feDropShadow",
            Tag::FeOffset => "feOffset",
            Tag::FeFlood => "feFlood",
            Tag::FeColorMatrix => "feColorMatrix",
            Tag::FeComposite => "feComposite",
            Tag::FeBlend => "feBlend",
            Tag::FeMerge => "feMerge",
            Tag::FeMergeNode => "feMergeNode",
            Tag::FeMorphology => "feMorphology",
            Tag::FeTurbulence => "feTurbulence",
            Tag::FeDisplacementMap => "feDisplacementMap",
        }
    }

    fn from_name(name: &str) -> Option<Tag> {
        Tag::ALL.into_iter().find(|t| t.name() == name)
    }

    fn is_shape(self) -> bool {
        matches!(self, Tag::Path | Tag::Circle | Tag::Ellipse | Tag::Rect | Tag::Line | Tag::Polyline | Tag::Polygon)
    }

    /// What can be drawn, moved and named by a pivot, a role, a parent, a track or a drive.
    fn is_part(self) -> bool {
        matches!(self, Tag::G | Tag::Text | Tag::Use) || self.is_shape()
    }

    /// What a `use` may repeat.
    fn is_repeatable(self) -> bool {
        matches!(self, Tag::G | Tag::Text) || self.is_shape()
    }

    fn is_gradient(self) -> bool {
        matches!(self, Tag::LinearGradient | Tag::RadialGradient)
    }

    /// The children of a `filter`: every primitive, `feMerge` among them, but not `feMergeNode`.
    fn is_primitive(self) -> bool {
        matches!(
            self,
            Tag::FeGaussianBlur | Tag::FeDropShadow | Tag::FeOffset | Tag::FeFlood | Tag::FeColorMatrix | Tag::FeComposite | Tag::FeBlend | Tag::FeMerge | Tag::FeMorphology | Tag::FeTurbulence | Tag::FeDisplacementMap
        )
    }
}

/// An attribute value of the stored tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    Num(Num),
    Str(String),
}

impl AttrValue {
    fn num(&self) -> Option<f64> {
        match self {
            AttrValue::Num(n) => Some(n.get()),
            AttrValue::Str(_) => None,
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            AttrValue::Str(s) => Some(s),
            AttrValue::Num(_) => None,
        }
    }
}

/// The parts Workbench's built-in motion is written against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Body,
    Head,
    Eyes,
    Pupils,
    ArmL,
    ArmR,
}

impl Role {
    pub const ALL: [Role; 6] = [Role::Body, Role::Head, Role::Eyes, Role::Pupils, Role::ArmL, Role::ArmR];

    pub fn name(self) -> &'static str {
        match self {
            Role::Body => "body",
            Role::Head => "head",
            Role::Eyes => "eyes",
            Role::Pupils => "pupils",
            Role::ArmL => "armL",
            Role::ArmR => "armR",
        }
    }
}

/// The states an avatar shows (Workbench chooses the state, the look says what moves in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LookState {
    Idle,
    Working,
    Speaking,
    Listening,
    Waiting,
    Attention,
    Error,
    Done,
    Sleeping,
}

impl LookState {
    pub const ALL: [LookState; 9] = [LookState::Idle, LookState::Working, LookState::Speaking, LookState::Listening, LookState::Waiting, LookState::Attention, LookState::Error, LookState::Done, LookState::Sleeping];

    pub fn name(self) -> &'static str {
        match self {
            LookState::Idle => "idle",
            LookState::Working => "working",
            LookState::Speaking => "speaking",
            LookState::Listening => "listening",
            LookState::Waiting => "waiting",
            LookState::Attention => "attention",
            LookState::Error => "error",
            LookState::Done => "done",
            LookState::Sleeping => "sleeping",
        }
    }
}

/// One of the agent's own animations: keyframe tracks over a duration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Animation {
    /// Seconds.
    pub duration: Num,
    /// Repeats while its state is active, or as a gesture in mode loop.
    #[serde(rename = "loop", default, skip_serializing_if = "is_false")]
    pub looping: bool,
    /// Seconds between starts, random in `[min, max]`. Excludes `loop`; neither: a one-shot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<[Num; 2]>,
    /// The default for its tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<Vec<Num>>,
    /// The default for its tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Eases>,
    pub tracks: Vec<Track>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One property of one or several parts over the animation's time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Track {
    pub target: Targets,
    pub prop: Prop,
    /// `None`: the artwork's own value (absolute props only).
    pub values: Vec<Option<Key>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<Vec<Num>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Eases>,
    /// Loops: where in the cycle this track starts, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Num>,
    /// One-shots: seconds this track starts later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<Num>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Targets {
    One(String),
    Many(Vec<String>),
}

/// A keyframe: a number, or a string for `fill`, `stroke` (a colour) and `d` (path data).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Key {
    Num(Num),
    Str(String),
}

/// One curve for every segment, or one per segment. A list of four numbers is one bezier, so a
/// list of curves is never four numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Eases {
    One(Curve),
    Each(Vec<Curve>),
}

/// How one segment runs from one keyframe to the next: a named curve, or a cubic bezier
/// `[x1, y1, x2, y2]` from (0, 0) to (1, 1) with x in 0 … 1 and y in -2 … 3.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Curve {
    Named(Ease),
    Bezier([Num; 4]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ease {
    Linear,
    In,
    Out,
    InOut,
    OutBack,
    InBack,
    InOutBack,
    OutElastic,
    OutBounce,
    Step,
}

impl Ease {
    const ALL: [Ease; 10] = [Ease::Linear, Ease::In, Ease::Out, Ease::InOut, Ease::OutBack, Ease::InBack, Ease::InOutBack, Ease::OutElastic, Ease::OutBounce, Ease::Step];

    fn name(self) -> &'static str {
        match self {
            Ease::Linear => "linear",
            Ease::In => "in",
            Ease::Out => "out",
            Ease::InOut => "inOut",
            Ease::OutBack => "outBack",
            Ease::InBack => "inBack",
            Ease::InOutBack => "inOutBack",
            Ease::OutElastic => "outElastic",
            Ease::OutBounce => "outBounce",
            Ease::Step => "step",
        }
    }
}

impl Curve {
    /// Whether a segment eased with it swings back and forth or leaves the way between its two
    /// values far behind: elastic and bounce do, and a bezier whose handles lie outside 0 … 1.
    /// (A bezier inside rises steadily; the back eases overshoot once, by a tenth.)
    fn swings(&self) -> bool {
        match self {
            Curve::Named(ease) => matches!(ease, Ease::OutElastic | Ease::OutBounce),
            Curve::Bezier(p) => !(0.0..=1.0).contains(&p[1].get()) || !(0.0..=1.0).contains(&p[3].get()),
        }
    }
}

impl Eases {
    fn any(&self, is: impl Fn(&Curve) -> bool) -> bool {
        match self {
            Eases::One(curve) => is(curve),
            Eases::Each(each) => each.iter().any(is),
        }
    }

    /// Whether a segment jumps: the paint rule refuses that.
    fn steps(&self) -> bool {
        self.any(|curve| *curve == Curve::Named(Ease::Step))
    }

    /// Whether a segment swings: the paint rule refuses that too.
    fn swings(&self) -> bool {
        self.any(Curve::swings)
    }

    /// How many segments it names, `None` for one curve for all.
    fn count(&self) -> Option<usize> {
        match self {
            Eases::One(_) => None,
            Eases::Each(each) => Some(each.len()),
        }
    }
}

/// What a track or a drive moves. Translate, rotate, skew and scale are relative to rest; the
/// others are absolute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Prop {
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

impl Prop {
    const ALL: [Prop; 29] = [
        Prop::TranslateX, Prop::TranslateY, Prop::Rotate, Prop::SkewX, Prop::SkewY, Prop::Scale, Prop::ScaleX, Prop::ScaleY, Prop::Opacity, Prop::FillOpacity, Prop::StrokeOpacity, Prop::Fill,
        Prop::Stroke, Prop::StrokeWidth, Prop::StrokeDashoffset, Prop::D, Prop::Cx, Prop::Cy, Prop::R, Prop::Rx, Prop::Ry, Prop::X, Prop::Y, Prop::Width, Prop::Height, Prop::X1, Prop::Y1,
        Prop::X2, Prop::Y2,
    ];

    /// The name on the wire, which is also the key of the attribute in a node's `attrs`.
    fn name(self) -> &'static str {
        match self {
            Prop::TranslateX => "translateX",
            Prop::TranslateY => "translateY",
            Prop::Rotate => "rotate",
            Prop::SkewX => "skewX",
            Prop::SkewY => "skewY",
            Prop::Scale => "scale",
            Prop::ScaleX => "scaleX",
            Prop::ScaleY => "scaleY",
            Prop::Opacity => "opacity",
            Prop::FillOpacity => "fillOpacity",
            Prop::StrokeOpacity => "strokeOpacity",
            Prop::Fill => "fill",
            Prop::Stroke => "stroke",
            Prop::StrokeWidth => "strokeWidth",
            Prop::StrokeDashoffset => "strokeDashoffset",
            Prop::D => "d",
            Prop::Cx => "cx",
            Prop::Cy => "cy",
            Prop::R => "r",
            Prop::Rx => "rx",
            Prop::Ry => "ry",
            Prop::X => "x",
            Prop::Y => "y",
            Prop::Width => "width",
            Prop::Height => "height",
            Prop::X1 => "x1",
            Prop::Y1 => "y1",
            Prop::X2 => "x2",
            Prop::Y2 => "y2",
        }
    }

    fn is_translate(self) -> bool {
        matches!(self, Prop::TranslateX | Prop::TranslateY)
    }

    fn is_scale(self) -> bool {
        matches!(self, Prop::Scale | Prop::ScaleX | Prop::ScaleY)
    }

    fn is_skew(self) -> bool {
        matches!(self, Prop::SkewX | Prop::SkewY)
    }

    /// Relative to rest, composed with other animations. Every other prop is absolute.
    fn is_relative(self) -> bool {
        self.is_translate() || self.is_scale() || self.is_skew() || self == Prop::Rotate
    }

    /// What turns about the pivot.
    fn pivots(self) -> bool {
        self == Prop::Rotate || self.is_skew() || self.is_scale()
    }

    /// The props the flash rule is about.
    fn is_paint(self) -> bool {
        self.is_colour() || self.is_opacity()
    }

    fn is_colour(self) -> bool {
        matches!(self, Prop::Fill | Prop::Stroke)
    }

    fn is_opacity(self) -> bool {
        matches!(self, Prop::Opacity | Prop::FillOpacity | Prop::StrokeOpacity)
    }

    /// A geometry attribute of a shape (or `x`, `y` of a text).
    fn is_geometry(self) -> bool {
        matches!(self, Prop::Cx | Prop::Cy | Prop::R | Prop::Rx | Prop::Ry | Prop::X | Prop::Y | Prop::Width | Prop::Height | Prop::X1 | Prop::Y1 | Prop::X2 | Prop::Y2)
    }

    /// A length: never negative.
    fn is_length(self) -> bool {
        matches!(self, Prop::R | Prop::Rx | Prop::Ry | Prop::Width | Prop::Height | Prop::StrokeWidth)
    }

    /// Whether an element of this kind has the prop.
    fn fits(self, tag: Tag) -> bool {
        if !tag.is_part() {
            return false;
        }
        match self {
            Prop::D => tag == Tag::Path,
            Prop::Cx | Prop::Cy => matches!(tag, Tag::Circle | Tag::Ellipse),
            Prop::R => tag == Tag::Circle,
            Prop::Rx | Prop::Ry => matches!(tag, Tag::Ellipse | Tag::Rect),
            Prop::X | Prop::Y => matches!(tag, Tag::Rect | Tag::Text),
            Prop::Width | Prop::Height => tag == Tag::Rect,
            Prop::X1 | Prop::Y1 | Prop::X2 | Prop::Y2 => tag == Tag::Line,
            _ => true,
        }
    }

    /// The elements that have the prop, for a problem's "accepted" part.
    fn where_it_fits(self) -> &'static str {
        match self {
            Prop::D => "path",
            Prop::Cx | Prop::Cy => "circle and ellipse",
            Prop::R => "circle",
            Prop::Rx | Prop::Ry => "ellipse and rect",
            Prop::X | Prop::Y => "rect and text",
            Prop::Width | Prop::Height => "rect",
            Prop::X1 | Prop::Y1 | Prop::X2 | Prop::Y2 => "line",
            _ => "g, shapes, text and use",
        }
    }
}

/// How the mouth follows speech: a path morph, or one numeric prop. `closed` defaults to the
/// artwork's own value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum MouthDrive {
    Path {
        target: String,
        open: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed: Option<String>,
    },
    Prop {
        target: String,
        prop: MouthProp,
        open: Num,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed: Option<Num>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouthProp {
    ScaleY,
    ScaleX,
    Scale,
    TranslateY,
    Ry,
    R,
    Height,
}

impl MouthProp {
    const ALL: [MouthProp; 7] = [MouthProp::ScaleY, MouthProp::ScaleX, MouthProp::Scale, MouthProp::TranslateY, MouthProp::Ry, MouthProp::R, MouthProp::Height];

    /// The track prop it moves like.
    fn prop(self) -> Prop {
        match self {
            MouthProp::ScaleY => Prop::ScaleY,
            MouthProp::ScaleX => Prop::ScaleX,
            MouthProp::Scale => Prop::Scale,
            MouthProp::TranslateY => Prop::TranslateY,
            MouthProp::Ry => Prop::Ry,
            MouthProp::R => Prop::R,
            MouthProp::Height => Prop::Height,
        }
    }
}

/// A live input the browser feeds a look's drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Input {
    /// Where it looks, -1 … 1: x to the right, y down.
    LookX,
    LookY,
    /// 0 … 1 while it speaks.
    Mouth,
    /// 0 standing … 1 walking fast.
    Pace,
    /// -1 walking or facing left … 1 right, 0 facing the viewer.
    Turn,
    /// 0 … 1 while the user drags it.
    Held,
}

impl Input {
    const ALL: [Input; 6] = [Input::LookX, Input::LookY, Input::Mouth, Input::Pace, Input::Turn, Input::Held];

    pub fn name(self) -> &'static str {
        match self {
            Input::LookX => "lookX",
            Input::LookY => "lookY",
            Input::Mouth => "mouth",
            Input::Pace => "pace",
            Input::Turn => "turn",
            Input::Held => "held",
        }
    }

    /// The inputs the browser moves slowly (a full swing takes at least 0.25 s): the only ones
    /// paint may follow.
    fn slow(self) -> bool {
        matches!(self, Input::Pace | Input::Turn | Input::Held)
    }

    /// How far the input runs from one end of its range to the other.
    fn span(self) -> f64 {
        match self {
            Input::LookX | Input::LookY | Input::Turn => 2.0,
            Input::Mouth | Input::Pace | Input::Held => 1.0,
        }
    }
}

/// How far apart two paint values of a drive stand along a slow input, in the input's own units:
/// the browser moves such an input at most 4 units a second, so this is [`MIN_PAINT_STEP`] in time.
const PAINT_PLACE_STEP: f64 = MIN_PAINT_STEP * 4.0;

/// One live input moving a prop of one or several parts: `values` sit along the input's range at
/// `at` (evenly without it), as a track's keyframes sit along its time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Drive {
    pub input: Input,
    pub target: Targets,
    pub prop: Prop,
    /// `None`: the artwork's own value (absolute props only).
    pub values: Vec<Option<Key>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Vec<Num>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Eases>,
}

impl fmt::Display for Drive {
    /// "lookX moves head rotate", a line of the design answer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let targets = match &self.target {
            Targets::One(id) => id.clone(),
            Targets::Many(ids) => listed(ids),
        };
        write!(f, "{} moves {targets} {}", self.input.name(), self.prop.name())
    }
}

/// The voice an avatar asks for: one of the browser's voices, plus pitch and rate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    /// BCP 47; the browser's default is "en-US".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Fragments of browser voice names, tried in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<Num>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<Num>,
}

// ---------------------------------------------------------------- the design call

/// What a session designed: its name, its look and its voice. `look: None` is the built-in
/// default look.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Design {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub look: Option<Look>,
    #[serde(default)]
    pub voice: Voice,
}

/// An accepted design call.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The design after the call.
    pub design: Design,
    /// One entry per part with an id, in document order: where it is, in root coordinates.
    pub parts: Vec<Part>,
    /// The names of the look's own animations.
    pub animations: Vec<String>,
    /// A call with `svg` over an earlier look: what was kept of it ("animations wave, nod").
    pub kept: Vec<String>,
    /// A call with `svg` over an earlier look: what was cleared of it.
    pub cleared: Vec<String>,
    /// At most [`MAX_WARNINGS`]; "Not drawn: …" comes first.
    pub warnings: Vec<String>,
    /// How much of the limits the look uses.
    pub usage: Usage,
}

/// A part of the artwork as the author needs to know it: the rest pose in root coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub id: String,
    pub tag: Tag,
    /// `[x, y, w, h]` in root (viewBox) coordinates.
    pub bbox: [f64; 4],
    /// The point it turns and scales about, in root coordinates.
    pub pivot: [f64; 2],
    /// False: no pivot was given, `pivot` is the centre of the box (the bottom centre for the body).
    pub pivot_given: bool,
    /// The part it moves with, from `parents`.
    pub parent: Option<String>,
}

impl fmt::Display for Part {
    /// `id tag x y w h pivot px py`, then `parent id` when it has one: the line of the design answer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [x, y, w, h] = self.bbox.map(fmt_num);
        let [px, py] = self.pivot.map(fmt_num);
        write!(f, "{} {} {x} {y} {w} {h} pivot {px} {py}", self.id, self.tag.name())?;
        if !self.pivot_given {
            f.write_str(" (default)")?;
        }
        if let Some(parent) = &self.parent {
            write!(f, " parent {parent}")?;
        }
        Ok(())
    }
}

/// What a look uses of its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// The artwork as SVG text: what the call sent, or what [`to_svg`] writes for a call without `svg`.
    pub svg_bytes: usize,
    /// Elements, each `use` with what it repeats.
    pub elements: usize,
    pub animations: usize,
    pub parents: usize,
    pub drives: usize,
    /// The stored look as JSON.
    pub look_bytes: usize,
}

/// The keys of a design call.
const KEYS: [&str; 12] = ["name", "svg", "pivots", "parents", "roles", "animations", "states", "mouth", "gaze", "drives", "voice", "keep"];
/// Those that belong to the look and need artwork.
const LOOK_KEYS: [&str; 8] = ["pivots", "parents", "roles", "animations", "states", "mouth", "gaze", "drives"];

/// Applies one design call to the current design, by the rules of the contract's section 4.1.
///
/// A call that carries `svg` is a new look: pivots, parents, roles, animations, states, mouth,
/// gaze and drives not given in the same call are cleared, unless `keep` is true, which keeps
/// those whose targets still exist. A call without `svg` merges: `animations`, `pivots` and
/// `parents` by key and `roles` and `states` per entry (`null` removes one), while `mouth`,
/// `gaze`, `drives`, `voice` and `name` replace.
///
/// Everything is validated before anything changes: the result is either the whole new design
/// or the problems, at most [`MAX_PROBLEM_KINDS`] kinds and then "and N more kinds".
///
/// Text (the name, `text` elements, `voice.prefer`) is cleaned but not masked here; a caller that
/// knows the session's secrets uses [`apply_masked`].
#[allow(dead_code)] // part of the validator's surface; the slice itself always masks, so only tests call it
pub fn apply(current: Option<&Design>, request: &Value) -> Result<Applied, Vec<String>> {
    apply_masked(current, request, &|s| s.to_string())
}

/// [`apply`] with the session's secret masking as the masking step of [`text::plain`].
///
/// A refusal quotes what the session wrote, and at times only part of a value: path data from
/// where the reader stopped, a number as it read it. A part of a secret is no secret a mask knows.
/// So when a string of a refused call holds one of the session's secrets, the problems are those
/// of a copy of the call whose strings, keys among them, were masked first: nothing they quote
/// is left of a secret.
pub fn apply_masked(current: Option<&Design>, request: &Value, mask: &dyn Fn(&str) -> String) -> Result<Applied, Vec<String>> {
    run(current, request, mask, Mode::Call).map_err(|problems| {
        let masked = masked_strings(request, mask);
        if masked == *request {
            return problems;
        }
        match run(current, &masked, mask, Mode::Call) {
            Err(problems) => problems,
            // Masked, the call would pass (a secret was what made a text too long): its own
            // problems, each masked whole.
            Ok(_) => problems.iter().map(|p| mask(p)).collect(),
        }
    })
}

/// `v` with every string in it, and every key, masked.
fn masked_strings(v: &Value, mask: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(mask(s)),
        Value::Array(items) => Value::Array(items.iter().map(|item| masked_strings(item, mask)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, item)| (mask(k), masked_strings(item, mask))).collect()),
        other => other.clone(),
    }
}

/// A stored design through the same validator as a design call.
///
/// `stored` is an object with `name`, `look` and `voice` (other keys, such as the record's own
/// `version` and `on`, are not looked at). The look is written back as a design call and applied
/// to nothing, so a hand-written file gets no further than an agent's call would (its artwork
/// has the size limit of a call's), and the boxes are computed again instead of believed.
/// `None`: the file does not hold a valid design.
pub fn check_stored(stored: Value) -> Option<Design> {
    let obj = stored.as_object()?;
    let mut request = Map::new();
    match obj.get("name") {
        None | Some(Value::Null) => {}
        Some(Value::String(name)) => {
            request.insert("name".into(), json!(name));
        }
        Some(_) => return None,
    }
    match obj.get("voice") {
        None | Some(Value::Null) => {}
        Some(voice @ Value::Object(_)) => {
            request.insert("voice".into(), voice.clone());
        }
        Some(_) => return None,
    }
    match obj.get("look") {
        None | Some(Value::Null) => {}
        Some(look @ Value::Object(_)) => {
            if json_len(look) > MAX_LOOK_BYTES {
                return None;
            }
            let look: Look = serde_json::from_value(look.clone()).ok()?;
            request.insert("svg".into(), json!(svg_text(&look, true)?));
            request.insert("pivots".into(), serde_json::to_value(&look.pivots).ok()?);
            request.insert("roles".into(), serde_json::to_value(&look.roles).ok()?);
            request.insert("animations".into(), serde_json::to_value(&look.animations).ok()?);
            request.insert("states".into(), serde_json::to_value(&look.states).ok()?);
            request.insert("mouth".into(), serde_json::to_value(&look.mouth).ok()?);
            request.insert("gaze".into(), serde_json::to_value(look.gaze).ok()?);
            request.insert("parents".into(), serde_json::to_value(&look.parents).ok()?);
            request.insert("drives".into(), serde_json::to_value(&look.drives).ok()?);
        }
        Some(_) => return None,
    }
    if request.is_empty() {
        return Some(Design::default());
    }
    run(None, &Value::Object(request), &|s| s.to_string(), Mode::Stored).ok().map(|applied| applied.design)
}

/// The artwork of a look written back as SVG text, attribute values in single quotes. Applying it
/// again (with the look's pivots, parents, roles, animations, states, mouth, gaze and drives)
/// gives the same look:
/// for a look this file accepted the text is never longer than [`MAX_SVG_BYTES`].
pub fn to_svg(look: &Look) -> String {
    svg_text(look, false).unwrap_or_default()
}

/// The parts of a look with an id, in document order, in root coordinates.
pub fn parts(look: &Look) -> Vec<Part> {
    parts_of(look, &Index::of(&look.nodes))
}

fn parts_of(look: &Look, ix: &Index<'_>) -> Vec<Part> {
    let body = look.roles.get(&Role::Body).map(String::as_str);
    ix.order
        .iter()
        .filter_map(|id| {
            let entry = ix.by_id.get(id)?;
            let own = entry.own_box();
            let given = look.pivots.get(*id).map(|p| (p[0].get(), p[1].get()));
            let (px, py) = given.unwrap_or_else(|| default_pivot(own, body == Some(*id)));
            let (rx, ry) = point(&entry.to_root, px, py);
            let parent = look.parents.get(*id).cloned();
            Some(Part { id: id.to_string(), tag: entry.node.tag, bbox: entry.root_box().array().map(round3), pivot: [round3(rx), round3(ry)], pivot_given: given.is_some(), parent })
        })
        .collect()
}

/// The built-in motion a look's roles (and, for think, its gaze) allow, in the order of
/// [`BUILTIN`]: each one `builtin::needs` finds nothing missing for, read from the typed look alone.
/// A test holds it to what the browser and the pictures make of the look as it is stored
/// (`builtin::names`).
pub fn builtins(look: &Look) -> Vec<&'static str> {
    BUILTIN.into_iter().filter(|name| builtin_missing(name, &look.roles, look.gaze).is_none()).collect()
}

/// Whether the look's animation `name` may be played several times in a row: each of its `fill`,
/// `stroke` and `opacity` tracks ends on the value it starts with, so nothing flashes where one
/// round meets the next. Built-in motion always closes, and so does a name the look does not
/// define.
///
/// It is the rule a `loop: true` animation is held to when it is designed, asked here of any
/// animation: a one-shot may end elsewhere, and is then played once at a time.
pub fn closes(look: &Look, name: &str) -> bool {
    let Some(animation) = look.animations.get(name) else { return true };
    let ix = Index::of(&look.nodes);
    animation.tracks.iter().filter(|track| track.prop.is_paint()).all(|track| {
        let (Some(first), Some(last)) = (track.values.first(), track.values.last()) else { return true };
        let ids: &[String] = match &track.target {
            Targets::One(id) => std::slice::from_ref(id),
            Targets::Many(ids) => ids,
        };
        // null is each target's own value; one that cannot be told counts as closing, as in a loop.
        ends_as_it_starts(first, last, |key| ids.iter().all(|id| ix.by_id.get(id.as_str()).is_none_or(|entry| entry.own(track.prop).is(key))))
    })
}

/// The problems of a refusal as one text: a line each, cut to [`MAX_MESSAGE_BYTES`] on a character
/// boundary.
pub fn message(problems: &[String]) -> String {
    let mut out = problems.join("\n");
    if out.len() > MAX_MESSAGE_BYTES {
        let mut end = MAX_MESSAGE_BYTES;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// What a built-in needs that the roles and the gaze (`None`: the default travel) do not give, or
/// `None` when it applies (`builtin::needs`).
pub(super) fn builtin_missing(name: &str, roles: &BTreeMap<Role, String>, gaze: Option<Num>) -> Option<&'static str> {
    super::builtin::needs(name, |role| roles.keys().any(|r| r.name() == role), gaze.map(Num::get))
}

// ---------------------------------------------------------------- problems and warnings

/// Problems (or warnings) grouped by kind: what was found, how often, where first, what is accepted.
#[derive(Default, Clone)]
struct Notes {
    /// The first [`MAX_PROBLEM_KINDS`] kinds, in the order they were met.
    kinds: Vec<Note>,
    /// Kind key to its place in `kinds`; `usize::MAX` for a kind past the limit.
    index: HashMap<String, usize>,
    /// Kinds past the limit.
    more: usize,
}

#[derive(Clone)]
struct Note {
    what: String,
    accepted: String,
    place: String,
    count: usize,
    /// A kind whose occurrences each say something of their own ([`Notes::add_each`]).
    each: Option<Each>,
}

/// A kind of note whose occurrences each say something of their own.
#[derive(Clone)]
struct Each {
    /// What one and several of them are called: "drive", "drives".
    one: &'static str,
    many: &'static str,
    /// What to do, said of them all where the line names more than the first.
    cure: &'static str,
    /// The places of the first ones after the first.
    places: Vec<String>,
}

/// Places a kind of [`Notes::add_each`] names after its first, at most: then it counts.
const EACH_NAMED: usize = 2;

impl Notes {
    /// Records one occurrence. `key` names the kind; `what` and `accepted` are kept from the
    /// first occurrence only. Input quoted in `what` goes through [`q`].
    fn add(&mut self, key: &str, place: &str, what: String, accepted: &str) {
        if let Some(&i) = self.index.get(key) {
            if let Some(note) = self.kinds.get_mut(i) {
                note.count += 1;
            }
            return;
        }
        if self.kinds.len() >= MAX_PROBLEM_KINDS {
            self.index.insert(key.to_string(), usize::MAX);
            self.more += 1;
            return;
        }
        self.index.insert(key.to_string(), self.kinds.len());
        self.kinds.push(Note { what, accepted: accepted.to_string(), place: place.to_string(), count: 1, each: None });
    }

    /// Records one occurrence of a kind whose occurrences each say something of their own (a
    /// drive that changes the figure at rest): they are one kind, so many of them leave room for
    /// the others. The first is said in full, with `accepted` while it is the only one; the later
    /// ones are named after it as far as the line holds them, then counted, and `each.cure` is
    /// said of them all.
    fn add_each(&mut self, key: &str, place: &str, what: String, accepted: &str, each: Each) {
        if let Some(note) = self.index.get(key).and_then(|&i| self.kinds.get_mut(i)) {
            note.count += 1;
            if let Some(each) = &mut note.each {
                if each.places.len() < EACH_NAMED {
                    each.places.push(place.to_string());
                }
            }
            return;
        }
        let known = self.index.contains_key(key);
        self.add(key, place, what, accepted);
        if let Some(note) = self.index.get(key).filter(|_| !known).and_then(|&i| self.kinds.get_mut(i)) {
            note.each = Some(each);
        }
    }

    fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// How many kinds were met, the ones past the limit included.
    fn len(&self) -> usize {
        self.kinds.len() + self.more
    }

    /// Records a fault of a value parser: those are written "what was found: what is accepted",
    /// and the last ": " is where the two parts meet (the accepted part never holds one).
    fn fault(&mut self, key: &str, place: &str, lead: &str, fault: &str) {
        let (found, accepted) = fault.rsplit_once(": ").unwrap_or((fault, ""));
        self.add(key, place, format!("{lead}{found}"), accepted);
    }

    fn line(note: &Note) -> String {
        if let Some(each) = note.each.as_ref().filter(|_| note.count > 1) {
            return Notes::line_each(note, each);
        }
        let mut s = note.what.clone();
        match (note.count, note.place.is_empty()) {
            (1, true) => {}
            (1, false) => {
                let _ = write!(s, " (at {})", note.place);
            }
            (n, true) => {
                let _ = write!(s, " ({n} times)");
            }
            (n, false) => {
                let _ = write!(s, " ({n} times, first at {})", note.place);
            }
        }
        if !note.accepted.is_empty() {
            s.push_str(": ");
            s.push_str(&note.accepted);
        }
        cut(&s, MAX_PROBLEM_CHARS)
    }

    /// The line of a kind of [`Notes::add_each`] met more than once: the first, then as many of
    /// the later ones' places as the line holds and how many more, then what to do with them all.
    fn line_each(note: &Note, each: &Each) -> String {
        let later = note.count - 1;
        let mut shortest = String::new();
        for named in (0..=each.places.len().min(later)).rev() {
            let mut also = each.places[..named].join(", ");
            let more = later - named;
            if more > 0 {
                let _ = write!(also, "{}{more} more {}", if named > 0 { " and " } else { "" }, if more == 1 { each.one } else { each.many });
            }
            let s = format!("{} (at {}; also {also}): {}", note.what, note.place, each.cure);
            if s.chars().count() <= MAX_PROBLEM_CHARS {
                return s;
            }
            if shortest.is_empty() || s.chars().count() < shortest.chars().count() {
                shortest = s;
            }
        }
        cut(&shortest, MAX_PROBLEM_CHARS)
    }

    /// The problems of a refusal: one line per kind, then "and N more kinds".
    fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self.kinds.iter().map(Notes::line).collect();
        if self.more > 0 {
            out.push(format!("and {} more kind{}", self.more, if self.more == 1 { "" } else { "s" }));
        }
        out
    }

    /// One line per kind, without a cap of its own (the caller caps warnings).
    fn lines(&self) -> (Vec<String>, usize) {
        (self.kinds.iter().map(Notes::line).collect(), self.more)
    }
}

/// What was dropped from the artwork, by name, in the order it was first met.
#[derive(Default, Clone)]
struct Drops {
    names: Vec<(&'static str, usize)>,
}

impl Drops {
    fn count(&mut self, name: &'static str) {
        match self.names.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => self.names.push((name, 1)),
        }
    }

    /// "Not drawn: filter (2), font-family (3)", or nothing.
    fn line(&self) -> Option<String> {
        if self.names.is_empty() {
            return None;
        }
        // As many names as fit the line, with room kept for "and N more".
        const ROOM: usize = " and 99 more".len();
        let mut s = String::from("Not drawn: ");
        let mut shown = 0;
        for (i, (name, count)) in self.names.iter().enumerate() {
            let piece = format!("{}{name} ({count})", if i == 0 { "" } else { ", " });
            let last = i + 1 == self.names.len();
            if s.len() + piece.len() + if last { 0 } else { ROOM } > MAX_PROBLEM_CHARS {
                break;
            }
            s.push_str(&piece);
            shown += 1;
        }
        if shown < self.names.len() {
            let _ = write!(s, " and {} more", self.names.len() - shown);
        }
        Some(cut(&s, MAX_PROBLEM_CHARS))
    }
}

/// At most `max` characters; a cut is marked.
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Input quoted for a message: masked with the session's secrets, cut to [`MAX_QUOTE_CHARS`] and
/// printed with `{:?}`, so nothing an agent wrote reaches a message unescaped. The mask comes
/// first: cut first, a secret longer than a quote would leave a piece of itself that no mask
/// knows any more.
fn q(mask: &dyn Fn(&str) -> String, s: &str) -> String {
    format!("{:?}", cut(&mask(s), MAX_QUOTE_CHARS))
}

/// The first `chars` characters of what is left of an input at a place, masked first, for a
/// message.
fn quoted_at(mask: &dyn Fn(&str) -> String, rest: &str, chars: usize) -> String {
    format!("{:?}", mask(rest).chars().take(chars).collect::<String>())
}

/// For what never reaches a message, or is not a session's at all.
fn unmasked(s: &str) -> String {
    s.to_string()
}

/// Up to [`LISTED`] names, then "and N more".
fn listed<S: AsRef<str>>(names: &[S]) -> String {
    let mut s = names.iter().take(LISTED).map(|n| n.as_ref()).collect::<Vec<_>>().join(", ");
    if names.len() > LISTED {
        let _ = write!(s, " and {} more", names.len() - LISTED);
    }
    s
}

/// The size of a value as compact JSON, without building the text.
fn json_len<T: Serialize + ?Sized>(v: &T) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    match serde_json::to_writer(&mut count, v) {
        Ok(()) => count.0,
        Err(_) => usize::MAX,
    }
}

/// A size in KiB with one decimal, for messages.
fn kib(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / 1024.0)
}

/// What kind of JSON value this is, for "… not a string".
fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

// ---------------------------------------------------------------- numbers

/// Rounded to three decimals; negative zero and anything not finite become 0.
fn round3(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let r = (v * 1000.0).round() / 1000.0;
    if !r.is_finite() {
        // Too large to scale by a thousand: a whole number already.
        return v;
    }
    if r == 0.0 { 0.0 } else { r }
}

/// A number as it is written into `d`, `points`, `transform` and messages: three decimals at
/// most, no exponent, `6` for 6.0.
fn fmt_num(v: f64) -> String {
    format!("{}", round3(v))
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

/// One number of the SVG grammar at `*i`: sign, digits, fraction, exponent. Moves `*i` past it.
/// `None`: no number starts here. A number too large for a double comes back infinite, so the
/// caller's range check names it.
fn scan_number(b: &[u8], i: &mut usize) -> Option<f64> {
    let start = *i;
    let mut j = start;
    if matches!(b.get(j), Some(b'+' | b'-')) {
        j += 1;
    }
    let digits = |from: usize| b.get(from..).map_or(0, |rest| rest.iter().take_while(|c| c.is_ascii_digit()).count());
    let whole = digits(j);
    j += whole;
    let mut fraction = 0;
    if b.get(j) == Some(&b'.') {
        fraction = digits(j + 1);
        if whole == 0 && fraction == 0 {
            return None;
        }
        j += 1 + fraction;
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(b.get(j), Some(b'e' | b'E')) {
        let mut k = j + 1;
        if matches!(b.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        let exponent = digits(k);
        // "1em" is the number 1 and a unit, not an exponent.
        if exponent > 0 {
            j = k + exponent;
        }
    }
    let text = std::str::from_utf8(b.get(start..j)?).ok()?;
    let v: f64 = text.parse().ok()?;
    *i = j;
    Some(v)
}

/// A value that is one number, optionally with the unit `px` (dropped) or `%`.
enum Scalar {
    Plain(f64),
    Percent(f64),
}

fn scalar(s: &str) -> Option<Scalar> {
    let b = s.trim().as_bytes();
    let mut i = 0;
    let v = scan_number(b, &mut i)?;
    match b.get(i..)? {
        [] => Some(Scalar::Plain(v)),
        [b'%'] => Some(Scalar::Percent(v)),
        [p, x] if p.eq_ignore_ascii_case(&b'p') && x.eq_ignore_ascii_case(&b'x') => Some(Scalar::Plain(v)),
        _ => None,
    }
}

/// A plain number (a trailing `px` is dropped), not a percentage.
fn plain_number(s: &str) -> Option<f64> {
    match scalar(s)? {
        Scalar::Plain(v) => Some(v),
        Scalar::Percent(_) => None,
    }
}

/// Numbers separated by white space or commas, and nothing else. At most `max` of them are read.
fn number_list(s: &str, max: usize) -> Option<Vec<f64>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while b.get(i).is_some_and(|c| is_space(*c) || *c == b',') {
            i += 1;
        }
        if i >= b.len() {
            return Some(out);
        }
        if out.len() >= max {
            return None;
        }
        out.push(scan_number(b, &mut i)?);
    }
}

/// The viewBox and the bounds that follow from it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl View {
    fn of(look: &Look) -> View {
        let [x, y, w, h] = look.view_box.map(Num::get);
        View { x, y, w, h }
    }

    /// How far outside the viewBox a number may lie.
    ///
    /// This and the bounds below are rounded to three decimals, as every number is stored: a sum
    /// of doubles can fall just inside its three-decimal value (-4.8 - 65.6 is
    /// -70.39999999999999), and a value accepted a little inside it would be stored on it, and be
    /// out of range when the file is read again. Rounding is monotone, so a value inside a
    /// rounded bound stays inside once it is rounded itself.
    fn reach(&self) -> f64 {
        round3(REACH * self.w.max(self.h))
    }

    fn lo(&self) -> f64 {
        round3(self.x.min(self.y) - self.reach())
    }

    fn hi(&self) -> f64 {
        round3((self.x + self.w).max(self.y + self.h) + self.reach())
    }

    /// A coordinate: within the reach of the viewBox.
    fn coord(&self, v: f64) -> bool {
        v >= self.lo() && v <= self.hi()
    }

    /// Where a pen lands after steps that were added up: a coordinate, give or take what adding
    /// leaves behind the last decimal.
    fn pen(&self, v: f64) -> bool {
        v >= self.lo() - 1e-6 && v <= self.hi() + 1e-6
    }

    /// A length: not negative, at most the reach.
    fn length(&self, v: f64) -> bool {
        v >= 0.0 && v <= self.reach()
    }

    /// A distance in either direction.
    fn offset(&self, v: f64) -> bool {
        v.abs() <= self.reach()
    }

    /// Any number of path data: a coordinate, a step between two, a radius, an angle. Where the
    /// pen lands is checked besides ([`View::coord`]).
    fn path_number(&self, v: f64) -> bool {
        v.is_finite() && v.abs() <= self.hi().abs().max(self.lo().abs())
    }

    fn coord_range(&self) -> String {
        format!("{} to {}", fmt_num(self.lo()), fmt_num(self.hi()))
    }

    /// Whether a box lies inside the viewBox, give or take the slack of the lint.
    fn holds(&self, b: &Bx) -> bool {
        b.x0 >= self.x - EDGE_SLACK && b.y0 >= self.y - EDGE_SLACK && b.x1 <= self.x + self.w + EDGE_SLACK && b.y1 <= self.y + self.h + EDGE_SLACK
    }
}

// ---------------------------------------------------------------- geometry

/// `[a, b, c, d, e, f]`: x' = a x + c y + e, y' = b x + d y + f.
type Affine = [f64; 6];

const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m` after `n`: a point goes through `n` first.
fn compose(m: &Affine, n: &Affine) -> Affine {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn point(m: &Affine, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// A direction through the linear part of `m`.
fn vector(m: &Affine, dx: f64, dy: f64) -> (f64, f64) {
    (m[0] * dx + m[2] * dy, m[1] * dx + m[3] * dy)
}

/// An axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bx {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Bx {
    const ZERO: Bx = Bx { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 };

    fn at(x: f64, y: f64) -> Bx {
        Bx { x0: x, y0: y, x1: x, y1: y }
    }

    fn sized(x: f64, y: f64, w: f64, h: f64) -> Bx {
        Bx { x0: x.min(x + w), y0: y.min(y + h), x1: x.max(x + w), y1: y.max(y + h) }
    }

    fn of(b: &[Num; 4]) -> Bx {
        Bx::sized(b[0].get(), b[1].get(), b[2].get(), b[3].get())
    }

    fn add(&mut self, x: f64, y: f64) {
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
    }

    fn join(&mut self, o: &Bx) {
        self.add(o.x0, o.y0);
        self.add(o.x1, o.y1);
    }

    /// The box of this box's corners through `m`.
    fn map(&self, m: &Affine) -> Bx {
        let (x, y) = point(m, self.x0, self.y0);
        let mut out = Bx::at(x, y);
        for (cx, cy) in [(self.x1, self.y0), (self.x0, self.y1), (self.x1, self.y1)] {
            let (x, y) = point(m, cx, cy);
            out.add(x, y);
        }
        out
    }

    fn shifted(&self, dx: f64, dy: f64) -> Bx {
        Bx { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 + dx, y1: self.y1 + dy }
    }

    fn w(&self) -> f64 {
        self.x1 - self.x0
    }

    fn h(&self) -> f64 {
        self.y1 - self.y0
    }

    /// `[x, y, w, h]`.
    fn array(&self) -> [f64; 4] {
        [self.x0, self.y0, self.w(), self.h()]
    }

    /// How far it reaches from the origin, or its larger side when that is more.
    fn span(&self) -> f64 {
        [self.x0, self.y0, self.x1, self.y1, self.w(), self.h()].into_iter().fold(0.0, |far, v| if v.abs() > far || v.is_nan() { v.abs() } else { far })
    }

    fn nums(&self) -> [Num; 4] {
        self.array().map(Num::new)
    }
}

/// Adds `b` to the box of a group.
fn grow(acc: &mut Option<Bx>, b: Bx) {
    match acc {
        Some(a) => a.join(&b),
        None => *acc = Some(b),
    }
}

/// Where a part without a pivot turns and scales: the centre of its box, the bottom centre for the body.
fn default_pivot(b: Bx, body: bool) -> (f64, f64) {
    ((b.x0 + b.x1) / 2.0, if body { b.y1 } else { (b.y0 + b.y1) / 2.0 })
}

/// The box of an elliptical arc from `(x0, y0)` to `(x1, y1)`: exact where the arc is well
/// defined (the conversion to a centre of the SVG implementation notes), and otherwise the end
/// points widened by the radii in every direction.
fn arc_box(x0: f64, y0: f64, rx: f64, ry: f64, rotation: f64, large: bool, sweep: bool, x1: f64, y1: f64) -> Bx {
    let mut out = Bx::at(x0, y0);
    out.add(x1, y1);
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if (x0 == x1 && y0 == y1) || rx == 0.0 || ry == 0.0 {
        // Nothing is drawn, or a straight line.
        return out;
    }
    let wide = {
        let r = 2.0 * rx.max(ry);
        Bx { x0: out.x0 - r, y0: out.y0 - r, x1: out.x1 + r, y1: out.y1 + r }
    };
    let phi = rotation.to_radians();
    let (sin, cos) = phi.sin_cos();
    let (dx, dy) = ((x0 - x1) / 2.0, (y0 - y1) / 2.0);
    let (xp, yp) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (xp * xp) / (rx * rx) + (yp * yp) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let denominator = rx * rx * yp * yp + ry * ry * xp * xp;
    if !(denominator > 0.0) {
        return wide;
    }
    let numerator = (rx * rx * ry * ry - denominator).max(0.0);
    let coefficient = (numerator / denominator).sqrt() * if large != sweep { 1.0 } else { -1.0 };
    let (cxp, cyp) = (coefficient * rx * yp / ry, -coefficient * ry * xp / rx);
    let (cx, cy) = (cos * cxp - sin * cyp + (x0 + x1) / 2.0, sin * cxp + cos * cyp + (y0 + y1) / 2.0);
    let start = ((yp - cyp) / ry).atan2((xp - cxp) / rx);
    let end = ((-yp - cyp) / ry).atan2((-xp - cxp) / rx);
    let turn = std::f64::consts::TAU;
    let mut delta = (end - start) % turn;
    if sweep && delta < 0.0 {
        delta += turn;
    } else if !sweep && delta > 0.0 {
        delta -= turn;
    }
    if ![cx, cy, start, delta].iter().all(|v| v.is_finite()) {
        return wide;
    }
    // The angles where x or y turns around; each counts when the arc passes it.
    let extremes = [(-ry * sin).atan2(rx * cos), (ry * cos).atan2(rx * sin)];
    for base in extremes {
        for angle in [base, base + std::f64::consts::PI] {
            let along = if delta >= 0.0 { (angle - start).rem_euclid(turn) } else { (start - angle).rem_euclid(turn) };
            if along <= delta.abs() {
                let (ex, ey) = (rx * angle.cos(), ry * angle.sin());
                out.add(cx + cos * ex - sin * ey, cy + sin * ex + cos * ey);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- colours

/// Every CSS colour name, sorted.
const COLOURS: [(&str, u32); 148] = [
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4), ("azure", 0xf0ffff), ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4), ("black", 0x000000), ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e), ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c), ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b), ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc), ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b), ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3), ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969),
    ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700), ("goldenrod", 0xdaa520), ("gray", 0x808080),
    ("green", 0x008000), ("greenyellow", 0xadff2f), ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6), ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3), ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32), ("linen", 0xfaf0e6), ("magenta", 0xff00ff),
    ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa), ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371), ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23), ("orange", 0xffa500),
    ("orangered", 0xff4500), ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093), ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb), ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399), ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd), ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8), ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3), ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

const COLOUR_FORMS: &str = "use #rrggbb, rgb(), hsl() or a CSS colour name";

/// A plain colour as `[r, g, b, a]`: hex, `rgb()`/`rgba()`, `hsl()`/`hsla()`, a CSS name or
/// `transparent`.
fn colour(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    if s.len() > 64 {
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        let digits: Option<Vec<u8>> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect();
        let d = digits?;
        return match d.len() {
            3 => Some([d[0] * 17, d[1] * 17, d[2] * 17, 255]),
            4 => Some([d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17]),
            6 => Some([d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], 255]),
            8 => Some([d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], d[6] * 16 + d[7]]),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    if lower == "transparent" {
        return Some([0, 0, 0, 0]);
    }
    if let Ok(i) = COLOURS.binary_search_by(|(name, _)| name.cmp(&lower.as_str())) {
        let rgb = COLOURS[i].1;
        return Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]);
    }
    let (function, rest) = lower.split_once('(')?;
    let inside = rest.strip_suffix(')')?;
    let parts = colour_parts(inside)?;
    let alpha = match parts.get(3) {
        None => 255,
        Some((v, percent)) => byte(if *percent { v / 100.0 } else { *v } * 255.0),
    };
    match (function.trim_end(), parts.len()) {
        ("rgb" | "rgba", 3 | 4) => {
            let channel = |(v, percent): (f64, bool)| byte(if percent { v * 2.55 } else { v });
            Some([channel(parts[0]), channel(parts[1]), channel(parts[2]), alpha])
        }
        ("hsl" | "hsla", 3 | 4) => {
            let (h, s, l) = (parts[0].0.rem_euclid(360.0), (parts[1].0 / 100.0).clamp(0.0, 1.0), (parts[2].0 / 100.0).clamp(0.0, 1.0));
            let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
            let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
            let m = l - c / 2.0;
            let (r, g, b) = match (h / 60.0) as u32 {
                0 => (c, x, 0.0),
                1 => (x, c, 0.0),
                2 => (0.0, c, x),
                3 => (0.0, x, c),
                4 => (x, 0.0, c),
                _ => (c, 0.0, x),
            };
            Some([byte((r + m) * 255.0), byte((g + m) * 255.0), byte((b + m) * 255.0), alpha])
        }
        _ => None,
    }
}

/// The arguments of a colour function: numbers, each maybe with `%` (or `deg` on a hue),
/// separated by commas, white space or a slash.
fn colour_parts(inside: &str) -> Option<Vec<(f64, bool)>> {
    let b = inside.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while b.get(i).is_some_and(|c| is_space(*c) || *c == b',' || *c == b'/') {
            i += 1;
        }
        if i >= b.len() {
            return Some(out);
        }
        if out.len() >= 4 {
            return None;
        }
        let v = scan_number(b, &mut i)?;
        if !v.is_finite() {
            return None;
        }
        let percent = b.get(i) == Some(&b'%');
        if percent {
            i += 1;
        } else if b.get(i..i + 3) == Some(b"deg".as_slice()) {
            i += 3;
        }
        out.push((v, percent));
    }
}

fn byte(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// `#rrggbb`, or `#rrggbbaa` when it is not opaque.
fn hex(c: [u8; 4]) -> String {
    if c[3] == 255 { format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]) } else { format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]) }
}

/// The id inside `url(#id)`, quotes and white space allowed.
fn url_id(s: &str) -> Option<&str> {
    let s = s.trim();
    let head = s.get(..4)?;
    if !head.eq_ignore_ascii_case("url(") {
        return None;
    }
    let inner = s.get(4..)?.strip_suffix(')')?.trim();
    let inner = inner.strip_prefix('\'').and_then(|i| i.strip_suffix('\'')).or_else(|| inner.strip_prefix('"').and_then(|i| i.strip_suffix('"'))).unwrap_or(inner);
    let id = inner.trim().strip_prefix('#')?;
    valid_id(id).then_some(id)
}

/// `[A-Za-z][A-Za-z0-9_-]{0,31}`.
fn valid_id(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && s.len() <= MAX_ID_CHARS && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

const ID_FORM: &str = "a letter, then letters, digits, _ or -, at most 32 characters";

/// Why a string cannot be an id or an animation name, or `None` when it can.
fn name_fault(s: &str) -> Option<&'static str> {
    if !valid_id(s) {
        Some(ID_FORM)
    } else if RESERVED.contains(&s) {
        Some("this name is reserved, choose another")
    } else {
        None
    }
}

// ---------------------------------------------------------------- path data, points, transforms

/// Path data, read and written again.
#[derive(Debug, Clone, PartialEq)]
struct PathData {
    /// Single-space-separated tokens, each arc flag a token of its own.
    text: String,
    /// What two paths must share to be morphed into one another.
    structure: Structure,
    /// The box of all points, control points included; arcs by their real extent.
    bbox: Option<Bx>,
}

/// The command letters in order, how many numbers follow them, and the arc flags.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Structure {
    letters: String,
    numbers: usize,
    flags: String,
    /// Per command letter, how many numbers and flags stand under it. Two paths are morphed
    /// token by token, so the same letters with as many numbers in all are one structure only
    /// when each command carries as many (`L1 1 2 2 L3 3` is not `L1 1 L2 2 3 3`).
    sets: Vec<u16>,
}

impl fmt::Display for Structure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} with {} numbers", cut(&self.letters, MAX_QUOTE_CHARS), self.numbers)?;
        if !self.flags.is_empty() {
            write!(f, " and arc flags {}", cut(&self.flags, 16))?;
        }
        Ok(())
    }
}

impl Structure {
    /// "MQQZ with 10 numbers against MQLZ with 8"; where only the layout differs, "M2 L4 L2
    /// against M2 L2 L4".
    fn against(&self, other: &Structure) -> String {
        if self.alike(other) {
            return format!("{} against {}", self.layout(), other.layout());
        }
        let mut s = format!("{self} against {} with {}", cut(&other.letters, MAX_QUOTE_CHARS), other.numbers);
        if !other.flags.is_empty() {
            let _ = write!(s, " and arc flags {}", cut(&other.flags, 16));
        }
        s
    }

    /// Whether the two read alike and still differ: the same letters, numbers and flags, with
    /// the numbers under other commands.
    fn alike(&self, other: &Structure) -> bool {
        self != other && self.letters == other.letters && self.numbers == other.numbers && self.flags == other.flags
    }

    /// "M2 L4 L2": each command with how many numbers stand under it.
    fn layout(&self) -> String {
        let commands: Vec<String> = self.letters.chars().zip(&self.sets).take(MAX_QUOTE_CHARS).map(|(c, n)| if *n == 0 { c.to_string() } else { format!("{c}{n}") }).collect();
        cut(&commands.join(" "), MAX_QUOTE_CHARS)
    }

    /// This structure as a message names it next to `other`: by what tells the two apart.
    fn beside(&self, other: &Structure) -> String {
        if self.alike(other) { self.layout() } else { self.to_string() }
    }
}

/// How many numbers one command takes.
fn arity(command: u8) -> Option<usize> {
    match command.to_ascii_uppercase() {
        b'M' | b'L' | b'T' => Some(2),
        b'H' | b'V' => Some(1),
        b'C' => Some(6),
        b'S' | b'Q' => Some(4),
        b'A' => Some(7),
        b'Z' => Some(0),
        _ => None,
    }
}

const D_FORM: &str = "path data is the commands MmLlHhVvCcSsQqTtAaZz and numbers, starting with M";

/// Reads path data by the SVG grammar (`1-2` and `.5.5` are two numbers each, arc flags are
/// single digits) and writes it again. `limit`: characters, as written and as stored.
fn path_data(s: &str, view: &View, limit: usize, mask: &dyn Fn(&str) -> String) -> Result<PathData, String> {
    let written = s.chars().count();
    if written > limit {
        return Err(format!("{written} characters of path data: at most {limit}"));
    }
    let b = s.as_bytes();
    let mut text = String::with_capacity(s.len() + 16);
    let mut structure = Structure { letters: String::new(), numbers: 0, flags: String::new(), sets: Vec::new() };
    let mut bbox: Option<Bx> = None;
    // The pen, the start of the subpath, and the control point a smooth command mirrors.
    let (mut cur, mut start) = ((0.0_f64, 0.0_f64), (0.0_f64, 0.0_f64));
    let mut mirror: Option<((f64, f64), u8)> = None;
    let mut command: Option<u8> = None;
    let mut args: Vec<f64> = Vec::with_capacity(7);
    let mut taken = 0_usize;
    let mut i = 0;
    loop {
        while b.get(i).is_some_and(|c| is_space(*c) || *c == b',') {
            i += 1;
        }
        let Some(&c) = b.get(i) else { break };
        if c.is_ascii_alphabetic() && !matches!(c, b'e' | b'E') {
            let Some(n) = arity(c) else {
                return Err(format!("{} is not a path command: {D_FORM}", quoted_at(mask, s.get(i..).unwrap_or_default(), 1)));
            };
            if let Some(prev) = command {
                if !args.is_empty() || (taken == 0 && arity(prev) != Some(0)) {
                    return Err(format!("the command {} has too few numbers: {D_FORM}", prev as char));
                }
            } else if !matches!(c, b'M' | b'm') {
                return Err(format!("path data starts with {}: {D_FORM}", c as char));
            }
            command = Some(c);
            taken = 0;
            structure.letters.push(c as char);
            structure.sets.push(0);
            if !text.is_empty() {
                text.push(' ');
            }
            text.push(c as char);
            if n == 0 {
                cur = start;
                mirror = None;
            }
            i += 1;
            continue;
        }
        let Some(cmd) = command else {
            return Err(format!("path data starts with {}: {D_FORM}", quoted_at(mask, s.get(i..).unwrap_or_default(), 1)));
        };
        let n = arity(cmd).unwrap_or(0);
        if n == 0 {
            return Err(format!("a number follows Z: {D_FORM}"));
        }
        let is_flag = cmd.eq_ignore_ascii_case(&b'A') && matches!(args.len(), 3 | 4);
        let v = if is_flag {
            match c {
                b'0' => 0.0,
                b'1' => 1.0,
                _ => return Err("an arc flag in path data that is not 0 or 1: the two flags of an arc are single digits".to_string()),
            }
        } else {
            match scan_number(b, &mut i) {
                Some(v) if view.path_number(v) => round3(v),
                Some(v) => return Err(format!("the number {} in path data is out of range: {}", if v.is_finite() { fmt_num(v) } else { "beyond any size".to_string() }, view.coord_range())),
                None => return Err(format!("{} in path data: {D_FORM}", quoted_at(mask, s.get(i..).unwrap_or_default(), 8))),
            }
        };
        text.push(' ');
        if is_flag {
            i += 1;
            text.push(c as char);
            structure.flags.push(c as char);
        } else {
            text.push_str(&fmt_num(v));
            structure.numbers += 1;
        }
        if let Some(under) = structure.sets.last_mut() {
            *under = under.saturating_add(1);
        }
        args.push(v);
        if args.len() < n {
            continue;
        }
        // One whole set of numbers: move the pen and widen the box.
        taken += 1;
        let relative = cmd.is_ascii_lowercase();
        let (ox, oy) = if relative { cur } else { (0.0, 0.0) };
        // The first point this data puts outside the reach of the viewBox. Relative steps add up,
        // so the pen is checked where it lands, and not only each number as it is written. A
        // point the data does not write (a mirrored control point, the bulge of an arc) widens
        // the box and is not checked.
        let mut far: Option<(f64, f64)> = None;
        let mut seen = |x: f64, y: f64, written: bool| {
            if written && far.is_none() && !(view.pen(x) && view.pen(y)) {
                far = Some((x, y));
            }
            match &mut bbox {
                Some(bx) => bx.add(x, y),
                None => bbox = Some(Bx::at(x, y)),
            }
        };
        let upper = cmd.to_ascii_uppercase();
        let mut next_mirror = None;
        match upper {
            b'M' | b'L' | b'T' => {
                let to = (ox + args[0], oy + args[1]);
                if upper == b'M' && taken == 1 {
                    // A move: the pen is lifted, nothing is drawn from where it was.
                    start = to;
                }
                if upper == b'T' {
                    // The control point is the last one mirrored in the pen.
                    let control = match mirror {
                        Some((p, b'Q')) => (2.0 * cur.0 - p.0, 2.0 * cur.1 - p.1),
                        _ => cur,
                    };
                    seen(control.0, control.1, false);
                    next_mirror = Some((control, b'Q'));
                }
                seen(to.0, to.1, true);
                cur = to;
            }
            b'H' => {
                let to = (ox + args[0], cur.1);
                seen(to.0, to.1, true);
                cur = to;
            }
            b'V' => {
                let to = (cur.0, oy + args[0]);
                seen(to.0, to.1, true);
                cur = to;
            }
            b'C' => {
                seen(ox + args[0], oy + args[1], true);
                seen(ox + args[2], oy + args[3], true);
                seen(ox + args[4], oy + args[5], true);
                next_mirror = Some(((ox + args[2], oy + args[3]), b'C'));
                cur = (ox + args[4], oy + args[5]);
            }
            b'S' => {
                let control = match mirror {
                    Some((p, b'C')) => (2.0 * cur.0 - p.0, 2.0 * cur.1 - p.1),
                    _ => cur,
                };
                seen(control.0, control.1, false);
                seen(ox + args[0], oy + args[1], true);
                seen(ox + args[2], oy + args[3], true);
                next_mirror = Some(((ox + args[0], oy + args[1]), b'C'));
                cur = (ox + args[2], oy + args[3]);
            }
            b'Q' => {
                seen(ox + args[0], oy + args[1], true);
                seen(ox + args[2], oy + args[3], true);
                next_mirror = Some(((ox + args[0], oy + args[1]), b'Q'));
                cur = (ox + args[2], oy + args[3]);
            }
            b'A' => {
                let to = (ox + args[5], oy + args[6]);
                let arc = arc_box(cur.0, cur.1, args[0], args[1], args[2], args[3] != 0.0, args[4] != 0.0, to.0, to.1);
                seen(to.0, to.1, true);
                seen(arc.x0, arc.y0, false);
                seen(arc.x1, arc.y1, false);
                cur = to;
            }
            _ => {}
        }
        if let Some((x, y)) = far {
            return Err(format!("a point of the path lies at {} {}: every point within {}", fmt_num(x), fmt_num(y), view.coord_range()));
        }
        mirror = next_mirror;
        args.clear();
    }
    match command {
        None => return Err(format!("empty path data: {D_FORM}")),
        Some(cmd) if !args.is_empty() || (taken == 0 && arity(cmd) != Some(0)) => {
            return Err(format!("the command {} has too few numbers: {D_FORM}", cmd as char));
        }
        Some(_) => {}
    }
    let stored = text.chars().count();
    if stored > limit {
        return Err(format!("{stored} characters of path data once it is spaced out: at most {limit}"));
    }
    Ok(PathData { text, structure, bbox })
}

/// A point list read and written again: numbers in pairs, single spaces between them.
fn point_list(s: &str, view: &View) -> Result<(String, Option<Bx>), String> {
    let written = s.chars().count();
    if written > MAX_POINTS_CHARS {
        return Err(format!("{written} characters of points: at most {MAX_POINTS_CHARS}"));
    }
    let Some(numbers) = number_list(s, MAX_POINTS_CHARS) else {
        return Err("points hold more than numbers: numbers separated by spaces or commas".to_string());
    };
    if numbers.is_empty() || numbers.len() % 2 != 0 {
        return Err(format!("points hold {} numbers: x and y in pairs", numbers.len()));
    }
    // Every number of a point list is a coordinate.
    if let Some(bad) = numbers.iter().find(|v| !view.coord(**v)) {
        return Err(format!("the number {} in points is out of range: {}", if bad.is_finite() { fmt_num(*bad) } else { "beyond any size".to_string() }, view.coord_range()));
    }
    let mut bbox: Option<Bx> = None;
    for pair in numbers.chunks_exact(2) {
        grow(&mut bbox, Bx::at(round3(pair[0]), round3(pair[1])));
    }
    let text = numbers.iter().map(|v| fmt_num(*v)).collect::<Vec<_>>().join(" ");
    let stored = text.chars().count();
    if stored > MAX_POINTS_CHARS {
        return Err(format!("{stored} characters of points once they are spaced out: at most {MAX_POINTS_CHARS}"));
    }
    Ok((text, bbox))
}

const TRANSFORM_FORM: &str = "translate, rotate, scale, skewX, skewY or matrix with plain numbers";
/// The functions of a transform list.
const TRANSFORMS: [&str; 6] = ["translate", "scale", "rotate", "skewX", "skewY", "matrix"];

/// A transform list read and written again, with its matrix.
fn transform(s: &str, view: &View, mask: &dyn Fn(&str) -> String) -> Result<(String, Affine), String> {
    if s.len() > MAX_TRANSFORM_CHARS {
        return Err(format!("a transform of {} characters: at most {MAX_TRANSFORM_CHARS}", s.len()));
    }
    let b = s.as_bytes();
    let mut text = String::new();
    let mut matrix = IDENTITY;
    let mut count = 0;
    let mut i = 0;
    loop {
        while b.get(i).is_some_and(|c| is_space(*c) || *c == b',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let from = i;
        while b.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
            i += 1;
        }
        let name = std::str::from_utf8(b.get(from..i).unwrap_or_default()).unwrap_or_default();
        // A function's name for a message: as written when it is one of the six, quoted (and
        // masked) otherwise.
        let named = || if TRANSFORMS.contains(&name) { name.to_string() } else { q(mask, name) };
        while b.get(i).is_some_and(|c| is_space(*c)) {
            i += 1;
        }
        if name.is_empty() || b.get(i) != Some(&b'(') {
            return Err(format!("a transform that is not written as functions: {TRANSFORM_FORM}"));
        }
        i += 1;
        let Some(len) = b.get(i..).and_then(|rest| rest.iter().position(|c| *c == b')')) else {
            return Err(format!("the transform {}( is not closed: {TRANSFORM_FORM}", named()));
        };
        let inside = std::str::from_utf8(b.get(i..i + len).unwrap_or_default()).unwrap_or_default();
        i += len + 1;
        let Some(a) = number_list(inside, 6) else {
            return Err(format!("the transform {} does not hold plain numbers, six at most: {TRANSFORM_FORM}", q(mask, &format!("{name}({inside})"))));
        };
        if a.iter().any(|v| !v.is_finite()) {
            return Err(format!("a number in the transform {}() is beyond any size: {TRANSFORM_FORM}", named()));
        }
        count += 1;
        if count > MAX_TRANSFORMS {
            return Err(format!("a transform of more than {MAX_TRANSFORMS} functions: at most {MAX_TRANSFORMS}"));
        }
        let a: Vec<f64> = a.into_iter().map(round3).collect();
        let step: Affine = match (name, a.as_slice()) {
            ("translate", [x]) if view.offset(*x) => [1.0, 0.0, 0.0, 1.0, *x, 0.0],
            ("translate", [x, y]) if view.offset(*x) && view.offset(*y) => [1.0, 0.0, 0.0, 1.0, *x, *y],
            ("translate", [_] | [_, _]) => return Err(format!("the transform translate() moves too far: within {} either way", fmt_num(view.reach()))),
            ("scale", [sx]) if scale_ok(*sx) => [*sx, 0.0, 0.0, *sx, 0.0, 0.0],
            ("scale", [sx, sy]) if scale_ok(*sx) && scale_ok(*sy) => [*sx, 0.0, 0.0, *sy, 0.0, 0.0],
            ("scale", [_] | [_, _]) => return Err(format!("the transform scale() is out of range: factors from {MIN_SCALE} to {MAX_SCALE} in size, either sign")),
            ("rotate", [angle]) if angle.abs() <= MAX_ROTATE => rotation(*angle, 0.0, 0.0),
            ("rotate", [angle, cx, cy]) if angle.abs() <= MAX_ROTATE && view.coord(*cx) && view.coord(*cy) => rotation(*angle, *cx, *cy),
            ("rotate", [_] | [_, _, _]) => return Err(format!("the transform rotate() is out of range: an angle up to {MAX_ROTATE} degrees, a centre within {}", view.coord_range())),
            ("skewX", [angle]) if angle.abs() <= MAX_SKEW => [1.0, 0.0, angle.to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewY", [angle]) if angle.abs() <= MAX_SKEW => [1.0, angle.to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            ("skewX" | "skewY", [_]) => return Err(format!("the transform {name}() is out of range: at most {MAX_SKEW} degrees")),
            ("matrix", [p, r, s, t, e, f]) => {
                let sane = [p, r, s, t].iter().all(|v| v.abs() <= MAX_SCALE) && (p * t - r * s).abs() >= MIN_SCALE * MIN_SCALE && view.offset(*e) && view.offset(*f);
                if !sane {
                    return Err(format!("the transform matrix() would flatten the picture or move it away: factors up to {MAX_SCALE}, a determinant of at least {}", MIN_SCALE * MIN_SCALE));
                }
                [*p, *r, *s, *t, *e, *f]
            }
            ("translate" | "scale" | "rotate" | "skewX" | "skewY" | "matrix", _) => return Err(format!("the transform {name}() with {} numbers: {TRANSFORM_FORM}", a.len())),
            _ => return Err(format!("{} is not a transform function: {TRANSFORM_FORM}", q(mask, name))),
        };
        matrix = compose(&matrix, &step);
        if !text.is_empty() {
            text.push(' ');
        }
        let _ = write!(text, "{name}({})", a.iter().map(|v| fmt_num(*v)).collect::<Vec<_>>().join(" "));
    }
    // Eight functions of six bounded numbers: what is written out is always the shorter text.
    Ok((text, matrix))
}

fn scale_ok(s: f64) -> bool {
    (MIN_SCALE..=MAX_SCALE).contains(&s.abs())
}

fn rotation(degrees: f64, cx: f64, cy: f64) -> Affine {
    let (sin, cos) = degrees.to_radians().sin_cos();
    // translate(cx cy) rotate(a) translate(-cx -cy)
    [cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy]
}

/// The matrix of a stored `transform` (which this file wrote); the identity when there is none.
fn matrix_of(node: &LookNode) -> Affine {
    let Some(text) = node.attrs.get("transform").and_then(AttrValue::str) else { return IDENTITY };
    // The bounds were checked when it was stored, against the viewBox of its look.
    transform(text, &WIDE, &unmasked).map_or(IDENTITY, |(_, m)| m)
}

// ---------------------------------------------------------------- the closed tables

/// How an attribute's value is read.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Id,
    ViewBox,
    Transform,
    /// `fill` and `stroke`: a colour, `none`, `currentColor` or `url(#id)` of a gradient.
    Paint,
    StopColour,
    Coord,
    Length,
    Opacity,
    Miter,
    Choice(&'static [&'static str]),
    ClipRef,
    /// `mask` and `filter`: `url(#id)` of an element of that kind, or `none`.
    MaskRef,
    FilterRef,
    /// The `href` of a `use`: `#id`.
    Href,
    PathData,
    Points,
    FontSize,
    FontWeight,
    LetterSpacing,
    /// A gradient coordinate along x, along y, or a radius: a number or a percentage.
    GradX,
    GradY,
    GradR,
    /// The region of a mask or a filter: x, y, width, height, a number or a percentage.
    RegionX,
    RegionY,
    RegionW,
    RegionH,
    Offset,
    /// A distance either way: `dx`, `dy`, `stroke-dashoffset`.
    Distance,
    /// Up to [`MAX_DASHES`] lengths, or `none`.
    Dashes,
    /// The input of a filter primitive: `SourceGraphic`, `SourceAlpha` or an earlier result.
    In,
    /// The name of a primitive's result.
    Result,
    /// One or two numbers: `stdDeviation` (up to a quarter of the viewBox width), `radius` (a
    /// tenth), `baseFrequency` (0 to 1).
    Blur,
    Radius,
    Frequency,
    /// The numbers of a colour matrix.
    MatrixValues,
    /// `k1` to `k4`.
    Factor,
    Octaves,
    Seed,
    /// The `scale` of a displacement: up to a quarter of the viewBox width either way.
    Displace,
}

/// One attribute of the closed table.
#[derive(Debug)]
struct Spec {
    /// The SVG name.
    name: &'static str,
    /// The React SVG prop name: the camelCase alias on input and the key in a node's `attrs`.
    prop: &'static str,
    kind: Kind,
    /// Also a CSS property, so `style` may set it.
    css: bool,
}

const fn spec(name: &'static str, prop: &'static str, kind: Kind, css: bool) -> Spec {
    Spec { name, prop, kind, css }
}

/// `g`, shapes, `text` and `use`.
static PRESENTATION: [Spec; 18] = [
    spec("id", "id", Kind::Id, false),
    spec("transform", "transform", Kind::Transform, false),
    spec("fill", "fill", Kind::Paint, true),
    spec("stroke", "stroke", Kind::Paint, true),
    spec("stroke-width", "strokeWidth", Kind::Length, true),
    spec("stroke-linecap", "strokeLinecap", Kind::Choice(&["butt", "round", "square"]), true),
    spec("stroke-linejoin", "strokeLinejoin", Kind::Choice(&["miter", "round", "bevel"]), true),
    spec("stroke-miterlimit", "strokeMiterlimit", Kind::Miter, true),
    spec("stroke-dasharray", "strokeDasharray", Kind::Dashes, true),
    spec("stroke-dashoffset", "strokeDashoffset", Kind::Distance, true),
    spec("fill-opacity", "fillOpacity", Kind::Opacity, true),
    spec("stroke-opacity", "strokeOpacity", Kind::Opacity, true),
    spec("opacity", "opacity", Kind::Opacity, true),
    spec("fill-rule", "fillRule", Kind::Choice(&["nonzero", "evenodd"]), true),
    spec("clip-rule", "clipRule", Kind::Choice(&["nonzero", "evenodd"]), true),
    spec("clip-path", "clipPath", Kind::ClipRef, true),
    spec("mask", "mask", Kind::MaskRef, true),
    spec("filter", "filter", Kind::FilterRef, true),
];
static PATH: [Spec; 1] = [spec("d", "d", Kind::PathData, false)];
static CIRCLE: [Spec; 3] = [spec("cx", "cx", Kind::Coord, false), spec("cy", "cy", Kind::Coord, false), spec("r", "r", Kind::Length, false)];
static ELLIPSE: [Spec; 4] = [spec("cx", "cx", Kind::Coord, false), spec("cy", "cy", Kind::Coord, false), spec("rx", "rx", Kind::Length, false), spec("ry", "ry", Kind::Length, false)];
static RECT: [Spec; 6] = [
    spec("x", "x", Kind::Coord, false),
    spec("y", "y", Kind::Coord, false),
    spec("width", "width", Kind::Length, false),
    spec("height", "height", Kind::Length, false),
    spec("rx", "rx", Kind::Length, false),
    spec("ry", "ry", Kind::Length, false),
];
static LINE: [Spec; 4] = [spec("x1", "x1", Kind::Coord, false), spec("y1", "y1", Kind::Coord, false), spec("x2", "x2", Kind::Coord, false), spec("y2", "y2", Kind::Coord, false)];
static POLY: [Spec; 1] = [spec("points", "points", Kind::Points, false)];
static TEXT: [Spec; 7] = [
    spec("x", "x", Kind::Coord, false),
    spec("y", "y", Kind::Coord, false),
    spec("font-size", "fontSize", Kind::FontSize, true),
    spec("font-weight", "fontWeight", Kind::FontWeight, true),
    spec("text-anchor", "textAnchor", Kind::Choice(&["start", "middle", "end"]), true),
    spec("dominant-baseline", "dominantBaseline", Kind::Choice(&["auto", "middle", "central", "hanging", "alphabetic"]), true),
    spec("letter-spacing", "letterSpacing", Kind::LetterSpacing, true),
];
static GRADIENT: [Spec; 4] = [
    spec("id", "id", Kind::Id, false),
    spec("gradientUnits", "gradientUnits", Kind::Choice(&["userSpaceOnUse", "objectBoundingBox"]), false),
    spec("gradientTransform", "gradientTransform", Kind::Transform, false),
    spec("spreadMethod", "spreadMethod", Kind::Choice(&["pad", "reflect", "repeat"]), false),
];
static LINEAR: [Spec; 4] = [spec("x1", "x1", Kind::GradX, false), spec("y1", "y1", Kind::GradY, false), spec("x2", "x2", Kind::GradX, false), spec("y2", "y2", Kind::GradY, false)];
static RADIAL: [Spec; 5] = [
    spec("cx", "cx", Kind::GradX, false),
    spec("cy", "cy", Kind::GradY, false),
    spec("r", "r", Kind::GradR, false),
    spec("fx", "fx", Kind::GradX, false),
    spec("fy", "fy", Kind::GradY, false),
];
static STOP: [Spec; 3] = [spec("offset", "offset", Kind::Offset, false), spec("stop-color", "stopColor", Kind::StopColour, true), spec("stop-opacity", "stopOpacity", Kind::Opacity, true)];
static CLIP: [Spec; 2] = [spec("id", "id", Kind::Id, false), spec("clipPathUnits", "clipPathUnits", Kind::Choice(&UNITS), false)];
static ROOT: [Spec; 1] = [spec("viewBox", "viewBox", Kind::ViewBox, false)];
static NONE: [Spec; 0] = [];
/// The coordinate systems of gradients, clip paths, masks and filters.
const UNITS: [&str; 2] = ["userSpaceOnUse", "objectBoundingBox"];
/// `href` stands first: what a `use` repeats is what it is about.
static USE: [Spec; 3] = [spec("href", "href", Kind::Href, false), spec("x", "x", Kind::Coord, false), spec("y", "y", Kind::Coord, false)];
static MASK: [Spec; 8] = [
    spec("id", "id", Kind::Id, false),
    spec("maskUnits", "maskUnits", Kind::Choice(&UNITS), false),
    spec("maskContentUnits", "maskContentUnits", Kind::Choice(&UNITS), false),
    spec("x", "x", Kind::RegionX, false),
    spec("y", "y", Kind::RegionY, false),
    spec("width", "width", Kind::RegionW, false),
    spec("height", "height", Kind::RegionH, false),
    spec("mask-type", "maskType", Kind::Choice(&["luminance", "alpha"]), true),
];
static FILTER: [Spec; 8] = [
    spec("id", "id", Kind::Id, false),
    spec("x", "x", Kind::RegionX, false),
    spec("y", "y", Kind::RegionY, false),
    spec("width", "width", Kind::RegionW, false),
    spec("height", "height", Kind::RegionH, false),
    spec("filterUnits", "filterUnits", Kind::Choice(&UNITS), false),
    spec("primitiveUnits", "primitiveUnits", Kind::Choice(&UNITS), false),
    spec("color-interpolation-filters", "colorInterpolationFilters", Kind::Choice(&["sRGB", "linearRGB"]), true),
];
// The filter primitives. Their names are local to their filter: `result` names what a primitive
// gives, `in` and `in2` take it up again, and nothing else refers to them.
const IN: Spec = spec("in", "in", Kind::In, false);
const IN2: Spec = spec("in2", "in2", Kind::In, false);
const RESULT: Spec = spec("result", "result", Kind::Result, false);
const DX: Spec = spec("dx", "dx", Kind::Distance, false);
const DY: Spec = spec("dy", "dy", Kind::Distance, false);
const STD_DEVIATION: Spec = spec("stdDeviation", "stdDeviation", Kind::Blur, false);
const FLOOD_COLOR: Spec = spec("flood-color", "floodColor", Kind::StopColour, true);
const FLOOD_OPACITY: Spec = spec("flood-opacity", "floodOpacity", Kind::Opacity, true);
const CHANNELS: [&str; 4] = ["R", "G", "B", "A"];
static BLUR: [Spec; 3] = [IN, STD_DEVIATION, RESULT];
static DROP_SHADOW: [Spec; 7] = [IN, DX, DY, STD_DEVIATION, FLOOD_COLOR, FLOOD_OPACITY, RESULT];
static OFFSET: [Spec; 4] = [IN, DX, DY, RESULT];
static FLOOD: [Spec; 3] = [FLOOD_COLOR, FLOOD_OPACITY, RESULT];
static COLOR_MATRIX: [Spec; 4] = [IN, spec("type", "type", Kind::Choice(&["matrix", "saturate", "hueRotate", "luminanceToAlpha"]), false), spec("values", "values", Kind::MatrixValues, false), RESULT];
static COMPOSITE: [Spec; 8] = [
    IN,
    IN2,
    spec("operator", "operator", Kind::Choice(&["over", "in", "out", "atop", "xor", "arithmetic"]), false),
    spec("k1", "k1", Kind::Factor, false),
    spec("k2", "k2", Kind::Factor, false),
    spec("k3", "k3", Kind::Factor, false),
    spec("k4", "k4", Kind::Factor, false),
    RESULT,
];
static BLEND: [Spec; 4] = [
    IN,
    IN2,
    spec(
        "mode",
        "mode",
        Kind::Choice(&["normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion"]),
        false,
    ),
    RESULT,
];
static MERGE: [Spec; 1] = [RESULT];
static MERGE_NODE: [Spec; 1] = [IN];
static MORPHOLOGY: [Spec; 4] = [IN, spec("operator", "operator", Kind::Choice(&["erode", "dilate"]), false), spec("radius", "radius", Kind::Radius, false), RESULT];
static TURBULENCE: [Spec; 5] = [
    spec("type", "type", Kind::Choice(&["fractalNoise", "turbulence"]), false),
    spec("baseFrequency", "baseFrequency", Kind::Frequency, false),
    spec("numOctaves", "numOctaves", Kind::Octaves, false),
    spec("seed", "seed", Kind::Seed, false),
    RESULT,
];
static DISPLACEMENT: [Spec; 6] = [
    IN,
    IN2,
    spec("scale", "scale", Kind::Displace, false),
    spec("xChannelSelector", "xChannelSelector", Kind::Choice(&CHANNELS), false),
    spec("yChannelSelector", "yChannelSelector", Kind::Choice(&CHANNELS), false),
    RESULT,
];

/// Where an attribute sits: on the `svg` root or on an element of the tree.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Scope {
    Root,
    Tag(Tag),
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Scope::Root => "svg",
            Scope::Tag(tag) => tag.name(),
        }
    }

    /// The attributes accepted here.
    fn specs(self) -> impl Iterator<Item = &'static Spec> {
        let (common, own): (&'static [Spec], &'static [Spec]) = match self {
            Scope::Root => (&ROOT, &PRESENTATION),
            Scope::Tag(Tag::G) => (&PRESENTATION, &NONE),
            Scope::Tag(Tag::Path) => (&PRESENTATION, &PATH),
            Scope::Tag(Tag::Circle) => (&PRESENTATION, &CIRCLE),
            Scope::Tag(Tag::Ellipse) => (&PRESENTATION, &ELLIPSE),
            Scope::Tag(Tag::Rect) => (&PRESENTATION, &RECT),
            Scope::Tag(Tag::Line) => (&PRESENTATION, &LINE),
            Scope::Tag(Tag::Polyline | Tag::Polygon) => (&PRESENTATION, &POLY),
            Scope::Tag(Tag::Text) => (&PRESENTATION, &TEXT),
            Scope::Tag(Tag::Use) => (&PRESENTATION, &USE),
            Scope::Tag(Tag::Defs) => (&NONE, &NONE),
            Scope::Tag(Tag::LinearGradient) => (&GRADIENT, &LINEAR),
            Scope::Tag(Tag::RadialGradient) => (&GRADIENT, &RADIAL),
            Scope::Tag(Tag::Stop) => (&STOP, &NONE),
            Scope::Tag(Tag::ClipPath) => (&CLIP, &NONE),
            Scope::Tag(Tag::Mask) => (&MASK, &NONE),
            Scope::Tag(Tag::Filter) => (&FILTER, &NONE),
            Scope::Tag(Tag::FeGaussianBlur) => (&BLUR, &NONE),
            Scope::Tag(Tag::FeDropShadow) => (&DROP_SHADOW, &NONE),
            Scope::Tag(Tag::FeOffset) => (&OFFSET, &NONE),
            Scope::Tag(Tag::FeFlood) => (&FLOOD, &NONE),
            Scope::Tag(Tag::FeColorMatrix) => (&COLOR_MATRIX, &NONE),
            Scope::Tag(Tag::FeComposite) => (&COMPOSITE, &NONE),
            Scope::Tag(Tag::FeBlend) => (&BLEND, &NONE),
            Scope::Tag(Tag::FeMerge) => (&MERGE, &NONE),
            Scope::Tag(Tag::FeMergeNode) => (&MERGE_NODE, &NONE),
            Scope::Tag(Tag::FeMorphology) => (&MORPHOLOGY, &NONE),
            Scope::Tag(Tag::FeTurbulence) => (&TURBULENCE, &NONE),
            Scope::Tag(Tag::FeDisplacementMap) => (&DISPLACEMENT, &NONE),
        };
        // The root takes the presentation attributes for the group that wraps its content, not an
        // id (ignored there) and not a transform.
        let root = self == Scope::Root;
        common.iter().chain(own).filter(move |s| !(root && matches!(s.kind, Kind::Id | Kind::Transform)))
    }

    /// By the SVG name or its camelCase alias; `css`: by the CSS property name only.
    fn find(self, name: &str, css: bool) -> Option<&'static Spec> {
        self.specs().find(|s| if css { s.css && s.name == name } else { s.name == name || s.prop == name })
    }

    /// What an element takes, its own attributes first: a problem is cut to its limit, and what
    /// every shape takes is the part to lose.
    fn accepted(self) -> String {
        let common = |s: &&Spec| PRESENTATION.iter().any(|p| p.name == s.name);
        let names: Vec<&str> = self.specs().filter(|s| !common(s)).chain(self.specs().filter(common)).map(|s| s.name).collect();
        if names.is_empty() { "it takes no attributes".to_string() } else { format!("it takes {}", names.join(", ")) }
    }
}

/// The attribute of a tag that a key of `attrs` is written back as.
fn attr_name(tag: Tag, key: &str) -> Option<&'static str> {
    let scope = Scope::Tag(tag);
    match key {
        "fillRef" => scope.find("fill", false).map(|s| s.name),
        "strokeRef" => scope.find("stroke", false).map(|s| s.name),
        "clipPathRef" => scope.find("clip-path", false).map(|s| s.name),
        "maskRef" => scope.find("mask", false).map(|s| s.name),
        "filterRef" => scope.find("filter", false).map(|s| s.name),
        "useRef" => scope.find("href", false).map(|s| s.name),
        _ => scope.specs().find(|s| s.prop == key && !matches!(s.kind, Kind::Id | Kind::ClipRef | Kind::MaskRef | Kind::FilterRef | Kind::Href)).map(|s| s.name),
    }
}

/// Ignored on the root without a word.
const ROOT_IGNORED: [&str; 7] = ["xmlns", "width", "height", "version", "id", "class", "preserveAspectRatio"];

/// Attributes that are dropped with a warning, with their camelCase spellings.
const DROPPED_ATTRS: [(&str, &str); 13] = [
    ("font-family", "fontFamily"),
    ("font-style", "fontStyle"),
    ("text-decoration", "textDecoration"),
    ("paint-order", "paintOrder"),
    ("vector-effect", "vectorEffect"),
    ("shape-rendering", "shapeRendering"),
    ("text-rendering", "textRendering"),
    ("pointer-events", "pointerEvents"),
    ("cursor", "cursor"),
    ("transform-origin", "transformOrigin"),
    ("class", "className"),
    ("role", "role"),
    ("tabindex", "tabIndex"),
];

/// The prefixed names that are ignored wherever they stand: namespace declarations, and the two
/// attributes of the `xml` prefix that change nothing here. Every other prefixed name refuses.
fn ignored_prefixed(name: &str) -> bool {
    name.starts_with("xmlns:") || name == "xml:space" || name == "xml:lang"
}

fn dropped_attr(name: &str) -> Option<&'static str> {
    if name.starts_with("data-") {
        return Some("data-*");
    }
    if name.starts_with("aria-") {
        return Some("aria-*");
    }
    DROPPED_ATTRS.iter().find(|(kebab, camel)| *kebab == name || *camel == name).map(|(kebab, _)| *kebab)
}

/// Elements that are dropped with a warning, with everything inside them.
const DROPPED_ELEMENTS: [&str; 7] = ["animate", "animateTransform", "animateMotion", "set", "title", "desc", "metadata"];

/// The filter primitives there are, for one that is not.
const PRIMITIVES_HINT: &str = "use blur, shadow, colour matrix, composite, blend, morphology, turbulence or displacement";

/// Elements that refuse the call, each with what to do instead.
const REFUSED_ELEMENTS: [(&str, &str); 18] = [
    ("script", "scripts are never accepted"),
    ("style", "style sheets are not accepted, put attributes on the elements"),
    ("foreignObject", "HTML is not accepted inside the artwork"),
    ("iframe", "frames are not accepted"),
    ("image", "images are not accepted, draw with shapes"),
    ("a", "links are not accepted, use a g"),
    ("pattern", "use a flat colour or a gradient"),
    ("symbol", "put a g in defs and use it"),
    ("marker", "draw the arrow head as a shape"),
    ("switch", "keep one branch in a g"),
    ("svg", "a nested svg is not supported, use a g"),
    ("tspan", "a tspan belongs inside a text"),
    ("feImage", "images are not accepted"),
    ("feTile", PRIMITIVES_HINT),
    ("feDiffuseLighting", PRIMITIVES_HINT),
    ("feSpecularLighting", PRIMITIVES_HINT),
    ("feComponentTransfer", PRIMITIVES_HINT),
    ("feConvolveMatrix", PRIMITIVES_HINT),
];

const ELEMENTS_ACCEPTED: &str = "draw with g, path, circle, ellipse, rect, line, polyline, polygon, text and use";
/// What a filter holds, for an element it does not.
const PRIMITIVES_ACCEPTED: &str = "the primitives are feGaussianBlur, feDropShadow, feOffset, feFlood, feColorMatrix, feComposite, feBlend, feMerge, feMorphology, feTurbulence, feDisplacementMap";

/// Where an element stands: drawn, or inert inside `defs`, a `clipPath`, a `mask` or a `filter`.
/// An element takes the zone of its parent, unless it opens one of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Drawn,
    Defs,
    Clip,
    Mask,
    Filter,
}

impl Zone {
    /// The zone of an element of kind `tag` inside this one.
    fn of(self, tag: Tag) -> Zone {
        match tag {
            Tag::ClipPath => Zone::Clip,
            Tag::Mask => Zone::Mask,
            Tag::Filter => Zone::Filter,
            Tag::Defs if self == Zone::Drawn => Zone::Defs,
            _ => self,
        }
    }

    /// The element that opens it, for a message.
    fn name(self) -> &'static str {
        match self {
            Zone::Drawn => "svg",
            Zone::Defs => "defs",
            Zone::Clip => "clipPath",
            Zone::Mask => "mask",
            Zone::Filter => "filter",
        }
    }

    /// Inside a clip path or a mask, where nothing is clipped, masked or filtered itself.
    fn shapes_only(self) -> bool {
        matches!(self, Zone::Clip | Zone::Mask)
    }
}

/// Whether `child` may sit directly inside `parent` (`None`: the root), which stands in `zone`,
/// and if not, the rule.
fn placement(parent: Option<Tag>, zone: Zone, child: Tag) -> Result<(), &'static str> {
    match child {
        Tag::Stop if parent.is_some_and(Tag::is_gradient) => Ok(()),
        Tag::Stop => Err("a stop belongs inside a gradient"),
        Tag::LinearGradient | Tag::RadialGradient | Tag::ClipPath if parent == Some(Tag::Defs) => Ok(()),
        Tag::LinearGradient | Tag::RadialGradient | Tag::ClipPath => Err("gradients and clip paths belong inside defs"),
        Tag::Mask | Tag::Filter if parent == Some(Tag::Defs) => Ok(()),
        Tag::Mask | Tag::Filter => Err("masks and filters belong inside defs"),
        Tag::FeMergeNode if parent == Some(Tag::FeMerge) => Ok(()),
        Tag::FeMergeNode => Err("a feMergeNode belongs inside a feMerge"),
        t if t.is_primitive() && parent == Some(Tag::Filter) => Ok(()),
        t if t.is_primitive() => Err("a filter primitive belongs inside a filter"),
        // g, shapes, text, use and defs.
        _ if zone == Zone::Mask && !(child == Tag::G || child.is_shape()) => Err("a mask holds g and shapes only"),
        _ => match parent {
            None | Some(Tag::G) => Ok(()),
            Some(Tag::Defs) if child.is_repeatable() => Ok(()),
            Some(Tag::Defs) => Err("defs holds gradients, clip paths, masks, filters, and g, shapes and text to use"),
            Some(Tag::ClipPath) if child.is_shape() => Ok(()),
            Some(Tag::ClipPath) => Err("a clipPath holds shapes only"),
            Some(Tag::Mask) => Ok(()),
            Some(Tag::Filter) => Err("a filter holds filter primitives only"),
            Some(Tag::FeMerge) => Err("a feMerge holds feMergeNode elements only"),
            Some(p) if p.is_gradient() => Err("a gradient holds stops only"),
            Some(Tag::Use) => Err("a use has no element children"),
            Some(p) if p.is_primitive() || p == Tag::FeMergeNode => Err("a filter primitive has no element children"),
            Some(_) => Err("shapes, text and stops have no element children"),
        },
    }
}

// ---------------------------------------------------------------- the artwork: reading SVG

/// Where checks report: problems refuse the call, warnings go into the answer.
#[derive(Default, Clone)]
struct Out {
    problems: Notes,
    warnings: Notes,
}

/// What the validator gathers while it works on one call.
struct Ctx<'m> {
    out: Out,
    drops: Drops,
    /// The masking step of [`text::plain`].
    mask: &'m dyn Fn(&str) -> String,
}

impl<'m> Ctx<'m> {
    fn new(mask: &'m dyn Fn(&str) -> String) -> Ctx<'m> {
        Ctx { out: Out::default(), drops: Drops::default(), mask }
    }
}

/// The artwork as read.
struct Artwork {
    view: View,
    nodes: Vec<LookNode>,
}

#[derive(Clone, Copy, PartialEq)]
enum Doc {
    /// The root has not started.
    Before,
    In,
    /// The root has ended.
    After,
}

/// An element that is open while reading.
enum Frame {
    /// A kept element being built.
    Node(Box<Open>),
    /// A `tspan`: its text goes to the `text` below it.
    Tspan,
    /// A dropped or refused element: everything inside is passed over.
    Skip,
}

struct Open {
    node: LookNode,
    at: (usize, usize),
    /// Its own `transform`.
    matrix: Affine,
    /// Its geometry so far, in the coordinates it is written in.
    bbox: Option<Bx>,
    /// The pieces of a `text` element.
    text: String,
    /// Where it stands.
    zone: Zone,
    /// The elements it draws: itself, what is inside it, and what each `use` among them repeats.
    size: usize,
    /// A `use` stands inside it.
    holds_use: bool,
}

/// An element with an id, once it is complete: what a `use` that repeats it needs to know.
#[derive(Clone, Copy)]
struct Done {
    tag: Tag,
    zone: Zone,
    /// Its box through its own transform, in the coordinates of what it stands in.
    placed: Option<Bx>,
    size: usize,
    holds_use: bool,
}

/// What a `url(#id)` must name.
#[derive(Clone, Copy, PartialEq)]
enum Wants {
    Gradient,
    Clip,
    Mask,
    Filter,
}

/// A `url(#id)` to resolve once every id is known.
struct Reference {
    attr: &'static str,
    id: String,
    wants: Wants,
    place: String,
}

/// A `use` whose target was not complete when the `use` was: it comes later, or not at all.
struct Later {
    id: String,
    place: String,
}

/// Where text goes at this point of the document.
enum Sink {
    Skip,
    Text(usize),
    Other,
}

struct Svg<'a, 'c, 'm> {
    cx: &'c mut Ctx<'m>,
    src: &'a str,
    /// Line and column are counted forward from the last place that was asked for.
    seen: usize,
    line: usize,
    col: usize,
    view: View,
    doc: Doc,
    stack: Vec<Frame>,
    top: Vec<LookNode>,
    /// The presentation attributes of the root: they move to a `g` that wraps the content.
    wrapper: Option<LookNode>,
    ids: HashMap<String, Tag>,
    /// The elements with an id that are complete.
    done: HashMap<String, Done>,
    references: Vec<Reference>,
    later: Vec<Later>,
    elements: usize,
    texts: usize,
    filters: usize,
    /// The results of the earlier primitives of the filter being read, for the `in` of the next.
    earlier: Vec<String>,
    /// What a first reading learnt of the elements with an id, for a `use` that repeats one that
    /// comes after it. Empty in a first reading.
    ahead: HashMap<String, Done>,
    /// A problem after which reading on would only add noise.
    stop: bool,
}

/// How one reading of the artwork ended.
enum Read {
    Done(Option<Artwork>),
    /// A `use` repeats an element that comes after it: what the reading learnt of every element
    /// with an id, to read the artwork again with.
    Again(HashMap<String, Done>),
}

/// Reads the artwork by the reader rules of the contract. `None`: it could not be read to the
/// end, the problems say why. Otherwise problems may still have been recorded: the tree then
/// holds what could be kept, so that pivots, roles and animations are checked in the same pass.
fn read_svg(src: &str, limit: usize, cx: &mut Ctx<'_>) -> Option<Artwork> {
    if src.len() > limit {
        cx.out.problems.add("svg-size", "", format!("svg is {} KiB", kib(src.len())), &format!("at most {} KiB", limit / 1024));
        return None;
    }
    // A use may repeat an element that comes after it. The first reading then learns what every
    // element with an id is, and the artwork is read once more, from what was noted before it.
    // What the use repeats holds no use itself, so what was learnt of it is final.
    let (out, drops) = (cx.out.clone(), cx.drops.clone());
    match read_once(src, cx, HashMap::new()) {
        Read::Done(artwork) => artwork,
        Read::Again(ahead) => {
            (cx.out, cx.drops) = (out, drops);
            match read_once(src, cx, ahead) {
                Read::Done(artwork) => artwork,
                Read::Again(_) => None,
            }
        }
    }
}

/// One reading of the artwork, with what an earlier one learnt of its elements.
fn read_once(src: &str, cx: &mut Ctx<'_>, ahead: HashMap<String, Done>) -> Read {
    let before = cx.out.problems.len();
    let mut svg = Svg {
        cx,
        src,
        seen: 0,
        line: 1,
        col: 1,
        view: DEFAULT_VIEW,
        doc: Doc::Before,
        stack: Vec::new(),
        top: Vec::new(),
        wrapper: None,
        ids: HashMap::new(),
        done: HashMap::new(),
        references: Vec::new(),
        later: Vec::new(),
        elements: 0,
        texts: 0,
        filters: 0,
        earlier: Vec::new(),
        ahead,
        stop: false,
    };
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(false);
    loop {
        let offset = reader.buffer_position() as usize;
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(err) => {
                svg.broken(&err, reader.error_position() as usize);
                return Read::Done(None);
            }
        };
        match event {
            Event::Start(e) => svg.open(&e, false, offset),
            Event::Empty(e) => svg.open(&e, true, offset),
            Event::End(_) => svg.close(),
            Event::Text(t) => svg.text(&t, offset),
            Event::GeneralRef(r) => svg.reference(&r, offset),
            Event::CData(_) => svg.markup("cdata", "svg: a CDATA section", "write the text itself, with &lt; and &amp;", offset),
            Event::PI(_) => svg.markup("pi", "svg: a processing instruction", "only the XML declaration may stand before the svg element", offset),
            Event::DocType(_) => svg.markup("doctype", "svg: a DOCTYPE", "start with the svg element", offset),
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Eof => break,
        }
        if svg.stop {
            return Read::Done(None);
        }
    }
    svg.end(before)
}

impl Svg<'_, '_, '_> {
    /// Line and column (in characters, from 1) of a byte offset.
    fn locate(&mut self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.src.len());
        if offset < self.seen {
            (self.seen, self.line, self.col) = (0, 1, 1);
        }
        if let Some(passed) = self.src.get(self.seen..offset) {
            for c in passed.chars() {
                if c == '\n' {
                    self.line += 1;
                    self.col = 1;
                } else {
                    self.col += 1;
                }
            }
            self.seen = offset;
        }
        (self.line, self.col)
    }

    /// "line 1 column 812, in g#armL": the place and the nearest element with an id.
    fn place(&self, at: (usize, usize), own: Option<(Tag, &str)>) -> String {
        let mut s = format!("line {} column {}", at.0, at.1);
        let within = own.map(|(tag, id)| format!("{}#{id}", tag.name())).or_else(|| {
            self.stack.iter().rev().find_map(|f| match f {
                Frame::Node(open) => open.node.id.as_ref().map(|id| format!("{}#{id}", open.node.tag.name())),
                _ => None,
            })
        });
        if let Some(within) = within {
            let _ = write!(s, ", in {within}");
        }
        s
    }

    fn bad(&mut self, key: &str, place: &str, what: String, accepted: &str) {
        self.cx.out.problems.add(key, place, what, accepted);
    }

    /// Input quoted for a message, masked with the session's secrets first ([`q`]).
    fn q(&self, s: &str) -> String {
        q(self.cx.mask, s)
    }

    /// A problem that ends the reading.
    fn fatal(&mut self, key: &str, place: &str, what: String, accepted: &str) {
        self.bad(key, place, what, accepted);
        self.stop = true;
    }

    fn broken(&mut self, err: &XmlError, offset: usize) {
        let at = self.locate(offset);
        let place = self.place(at, None);
        let (what, accepted) = match err {
            XmlError::IllFormed(IllFormedError::MismatchedEndTag { expected, found }) => (format!("svg: the end tag {} closes {}", self.q(found), self.q(expected)), "every element is closed by its own end tag"),
            XmlError::IllFormed(IllFormedError::UnmatchedEndTag(name)) => (format!("svg: the end tag {} has no start tag", self.q(name)), "every element is closed by its own end tag"),
            XmlError::IllFormed(IllFormedError::UnclosedReference) => ("svg: an & without its ;".to_string(), "write &amp; for an ampersand"),
            XmlError::Syntax(_) => ("svg: a tag, comment or section is not closed".to_string(), "the artwork is well-formed XML"),
            _ => ("svg is not well-formed XML".to_string(), "one svg element with properly nested and quoted content"),
        };
        self.fatal("xml", &place, what, accepted);
    }

    fn markup(&mut self, key: &str, what: &str, accepted: &str, offset: usize) {
        let at = self.locate(offset);
        let place = self.place(at, None);
        self.bad(key, &place, format!("{what} is not accepted"), accepted);
    }

    fn sink(&self) -> Sink {
        match self.stack.last() {
            Some(Frame::Skip) => Sink::Skip,
            Some(Frame::Tspan) => self.stack.iter().rposition(|f| matches!(f, Frame::Node(_))).map_or(Sink::Skip, Sink::Text),
            Some(Frame::Node(open)) if open.node.tag == Tag::Text => Sink::Text(self.stack.len() - 1),
            _ => Sink::Other,
        }
    }

    fn push_text(&mut self, i: usize, piece: &str) {
        if let Some(Frame::Node(open)) = self.stack.get_mut(i) {
            open.text.push_str(piece);
        }
    }

    fn text(&mut self, t: &str, offset: usize) {
        match self.sink() {
            Sink::Skip => {}
            Sink::Text(i) => self.push_text(i, t),
            Sink::Other => {
                if t.bytes().all(is_space) {
                    return;
                }
                let at = self.locate(offset);
                let place = self.place(at, None);
                if self.doc == Doc::In {
                    self.bad("stray-text", &place, format!("svg: text {} outside a text element", self.q(t.trim())), "words go into a text element");
                } else {
                    self.bad("outside-text", &place, format!("svg: text {} outside the svg element", self.q(t.trim())), "only white space and comments stand around the root");
                }
            }
        }
    }

    /// `&name;` or `&#N;` in content: resolved inside a `text`, text like any other elsewhere.
    fn reference(&mut self, r: &BytesRef<'_>, offset: usize) {
        let name: &str = r;
        match self.sink() {
            Sink::Skip => {}
            Sink::Text(i) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    Ok(None) => match name {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                    Err(_) => None,
                };
                match resolved {
                    Some(c) => self.push_text(i, c.encode_utf8(&mut [0; 4])),
                    None => {
                        let at = self.locate(offset);
                        let place = self.place(at, None);
                        self.bad(&format!("entity {}", cut(name, MAX_QUOTE_CHARS)), &place, format!("svg: the reference {} is not known", self.q(name)), ENTITIES);
                    }
                }
            }
            Sink::Other => {
                let at = self.locate(offset);
                let place = self.place(at, None);
                self.bad("stray-text", &place, format!("svg: text {} outside a text element", self.q(name)), "words go into a text element");
            }
        }
    }

    fn open(&mut self, e: &BytesStart<'_>, empty: bool, offset: usize) {
        let at = self.locate(offset);
        let name = e.name().into_inner();
        match self.doc {
            Doc::Before if name == "svg" => {
                self.doc = if empty { Doc::After } else { Doc::In };
                self.root(e, at);
                return;
            }
            Doc::Before => {
                let place = self.place(at, None);
                self.fatal("root", &place, format!("svg: the root element is {}", self.q(name)), "the artwork is one svg element");
                return;
            }
            Doc::After => {
                if self.stack.is_empty() {
                    let place = self.place(at, None);
                    self.bad("second-root", &place, format!("svg: a second root element {}", self.q(name)), "the artwork is one svg element");
                }
                if !empty {
                    self.stack.push(Frame::Skip);
                }
                return;
            }
            Doc::In => {}
        }
        self.elements += 1;
        if self.elements > MAX_ELEMENTS {
            let place = self.place(at, None);
            self.fatal("elements", &place, format!("svg: more than {MAX_ELEMENTS} elements"), "draw with fewer, larger shapes");
            return;
        }
        if self.stack.len() + 1 + usize::from(self.wrapper.is_some()) > MAX_DEPTH {
            let place = self.place(at, None);
            self.fatal("depth", &place, format!("svg: elements nested deeper than {MAX_DEPTH}"), "use fewer groups inside groups");
            return;
        }
        let place = self.place(at, None);
        let frame = match self.stack.last() {
            Some(Frame::Skip) => Frame::Skip,
            Some(Frame::Tspan) => self.inside_text(e, name, &place),
            Some(Frame::Node(open)) if open.node.tag == Tag::Text => self.inside_text(e, name, &place),
            Some(Frame::Node(open)) => {
                let (parent, zone) = (open.node.tag, open.zone);
                self.element(e, name, at, &place, Some(parent), zone)
            }
            None => self.element(e, name, at, &place, None, Zone::Drawn),
        };
        match frame {
            Frame::Node(open) if empty => self.finish(*open),
            Frame::Tspan | Frame::Skip if empty => {}
            frame => self.stack.push(frame),
        }
    }

    /// An element inside a `text`: a `tspan` is flattened, anything else is passed over.
    fn inside_text(&mut self, e: &BytesStart<'_>, name: &str, place: &str) -> Frame {
        if name == "tspan" {
            // Only the words of a tspan are kept, so its attributes are not read as values. What
            // is never accepted anywhere still refuses.
            let mut given = false;
            for (n, attr) in e.attributes().enumerate() {
                let key = match attr {
                    Ok(attr) if n < MAX_ATTRS => attr.key.into_inner(),
                    _ => {
                        self.bad("attr-form tspan", place, "svg: the attributes of <tspan> are not well-formed".to_string(), "write each attribute once, as name='value', a handful at most");
                        break;
                    }
                };
                if ignored_prefixed(key) {
                    continue;
                }
                given = true;
                if key.to_ascii_lowercase().starts_with("on") {
                    self.bad("attr-on", place, format!("svg: the event attribute {}", self.q(key)), "an avatar's artwork has no scripts");
                } else if key == "href" || key == "xlink:href" {
                    self.bad("attr-href", place, format!("svg: {key} on <tspan>"), HREF_ONLY_USE);
                } else if key.contains(':') {
                    self.bad(&format!("attr-prefix {}", cut(key, MAX_QUOTE_CHARS)), place, format!("svg: the prefixed attribute {}", self.q(key)), "names are written without a prefix");
                }
            }
            self.cx.out.warnings.add("tspan", place, "a tspan is flattened into its text".to_string(), if given { "its words are kept, its own position and style are not" } else { "its words are kept" });
            return Frame::Tspan;
        }
        if let Some(dropped) = DROPPED_ELEMENTS.iter().find(|d| **d == name) {
            self.cx.drops.count(dropped);
        } else {
            self.bad(&format!("in-text {}", cut(name, MAX_QUOTE_CHARS)), place, format!("svg: {} inside a text", self.q(name)), "a text holds words, and tspans that are flattened into it");
        }
        Frame::Skip
    }

    /// An element inside the root, a `g`, `defs`, a gradient, a `clipPath`, a `mask`, a `filter`
    /// or a `feMerge`. `zone` is where the parent stands.
    fn element(&mut self, e: &BytesStart<'_>, name: &str, at: (usize, usize), place: &str, parent: Option<Tag>, zone: Zone) -> Frame {
        if let Some(tag) = Tag::from_name(name) {
            if let Err(rule) = placement(parent, zone, tag) {
                self.bad(&format!("placement {name} {}", parent.map_or("svg", Tag::name)), place, format!("svg: <{name}> inside <{}>", parent.map_or("svg", Tag::name)), rule);
                return Frame::Skip;
            }
            // How many children the parent has so far: stops, primitives, merge nodes.
            let siblings = match self.stack.last() {
                Some(Frame::Node(open)) => open.node.children.len(),
                _ => 0,
            };
            if tag == Tag::Stop && siblings >= MAX_STOPS {
                self.bad("stops", place, format!("svg: more than {MAX_STOPS} stops in a gradient"), "a few stops make a gradient");
                return Frame::Skip;
            }
            if tag.is_primitive() && siblings >= MAX_FILTER_PRIMITIVES {
                self.bad("primitives", place, format!("svg: more than {MAX_FILTER_PRIMITIVES} primitives in a filter"), "a few primitives make an effect");
                return Frame::Skip;
            }
            if tag == Tag::FeMergeNode && siblings >= MAX_MERGE_NODES {
                self.bad("merge-nodes", place, format!("svg: more than {MAX_MERGE_NODES} feMergeNode elements in a feMerge"), "a merge lays a few results over one another");
                return Frame::Skip;
            }
            if tag == Tag::Text {
                self.texts += 1;
                if self.texts > MAX_TEXTS {
                    self.bad("texts", place, format!("svg: more than {MAX_TEXTS} text elements"), "an avatar speaks in lines, not in its artwork");
                    return Frame::Skip;
                }
            }
            if tag == Tag::Filter {
                self.filters += 1;
                if self.filters > MAX_FILTERS {
                    self.bad("filters", place, format!("svg: more than {MAX_FILTERS} filters"), "a few effects, each used by the parts that need it");
                    return Frame::Skip;
                }
            }
            return Frame::Node(Box::new(self.build(e, tag, at, zone.of(tag))));
        }
        if let Some(dropped) = DROPPED_ELEMENTS.iter().find(|d| **d == name) {
            self.cx.drops.count(dropped);
        } else if let Some((refused, instead)) = REFUSED_ELEMENTS.iter().find(|(r, _)| *r == name) {
            self.bad(&format!("element {refused}"), place, format!("svg: <{refused}> is not supported"), instead);
        } else if name.contains(':') {
            self.bad(&format!("element {}", cut(name, MAX_QUOTE_CHARS)), place, format!("svg: the prefixed element {}", self.q(name)), "names are written without a prefix");
        } else {
            let accepted = if matches!(parent, Some(Tag::Filter | Tag::FeMerge)) { PRIMITIVES_ACCEPTED } else { ELEMENTS_ACCEPTED };
            self.bad(&format!("element {}", cut(name, MAX_QUOTE_CHARS)), place, format!("svg: the element {} is not supported", self.q(name)), accepted);
        }
        Frame::Skip
    }

    /// The attributes of one element, sorted into what is kept: `(spec, decoded value)` pairs as
    /// they were written, `style` already folded in. Everything else is counted as dropped or
    /// recorded as a problem here.
    fn gather(&mut self, e: &BytesStart<'_>, scope: Scope, place: &str) -> Vec<(&'static Spec, String)> {
        let on = scope.name();
        let mut found: Vec<(&'static Spec, String)> = Vec::new();
        let mut styled: Vec<(&'static Spec, String)> = Vec::new();
        // The count first, and cheaply: an element over the limit is not read at all.
        let mut counting = e.attributes();
        counting.with_checks(false);
        if counting.take(MAX_ATTRS + 1).count() > MAX_ATTRS {
            self.bad("attrs", place, format!("svg: more than {MAX_ATTRS} attributes on <{on}>"), "an element needs a handful");
            return found;
        }
        for attr in e.attributes() {
            let attr = match attr {
                Ok(attr) => attr,
                Err(err) => {
                    // The reader recovers after some of these; a refused call has no use for that.
                    let fault = match err {
                        quick_xml::events::attributes::AttrError::Duplicated(..) => "a name is given twice",
                        quick_xml::events::attributes::AttrError::UnquotedValue(_) => "a value has no quotes",
                        quick_xml::events::attributes::AttrError::ExpectedQuote(..) => "a value is not closed",
                        _ => "a name has no value",
                    };
                    self.bad(&format!("attr-form {fault}"), place, format!("svg: the attributes of <{on}> are not well-formed, {fault}"), "write each attribute once, as name='value'");
                    break;
                }
            };
            let mut key = attr.key.into_inner();
            if ignored_prefixed(key) || (scope == Scope::Root && ROOT_IGNORED.contains(&key)) {
                continue;
            }
            let lower = key.to_ascii_lowercase();
            if lower.starts_with("on") {
                self.bad("attr-on", place, format!("svg: the event attribute {}", self.q(key)), "an avatar's artwork has no scripts");
                continue;
            }
            let using = scope == Scope::Tag(Tag::Use);
            if key == "href" || key == "xlink:href" {
                if !using {
                    self.bad("attr-href", place, format!("svg: {key} on <{on}>"), HREF_ONLY_USE);
                    continue;
                }
                // Both spellings are the one href of a use, and its value says where it points.
                key = "href";
            }
            // A prefix refuses whatever follows it, a name that would be dropped as well.
            if key.contains(':') {
                self.bad(&format!("attr-prefix {}", cut(key, MAX_QUOTE_CHARS)), place, format!("svg: the prefixed attribute {}", self.q(key)), "names are written without a prefix");
                continue;
            }
            if let Some(dropped) = dropped_attr(key) {
                self.dropped(dropped, place);
                continue;
            }
            if using && matches!(key, "width" | "height") {
                self.cx.out.warnings.add("use-size", place, "width and height of a use are left out".to_string(), "a use draws what it repeats at its own size, scale it with transform");
                continue;
            }
            // The five XML entities and numeric references; any other reference refuses.
            let value = match attr.normalized_value_with(XmlVersion::Implicit1_0, 1, resolve_xml_entity) {
                Ok(value) => value,
                Err(_) => {
                    self.bad("attr-entity", place, format!("svg: {} holds a reference that is not known", self.q(key)), ENTITIES);
                    continue;
                }
            };
            if key == "style" {
                self.style(&value, scope, place, &mut styled);
                continue;
            }
            let Some(spec) = scope.find(key, false) else {
                self.bad(&format!("attr {on} {}", cut(key, MAX_QUOTE_CHARS)), place, format!("svg: {} is not an attribute of <{on}>", self.q(key)), &scope.accepted());
                continue;
            };
            if found.iter().any(|(s, _)| s.name == spec.name) {
                self.bad(&format!("attr-twice {}", spec.name), place, format!("svg: {} is given twice on <{on}>, once as {}", spec.name, spec.prop), "write each attribute once");
                continue;
            }
            found.push((spec, value.into_owned()));
        }
        // A declaration in style wins over the attribute of the same name.
        for (spec, value) in styled {
            found.retain(|(s, _)| s.name != spec.name);
            found.push((spec, value));
        }
        found
    }

    /// A `style` attribute: declarations of the presentation attributes become those attributes,
    /// anything else is dropped with a warning, and what could reach outside refuses.
    fn style(&mut self, value: &str, scope: Scope, place: &str, styled: &mut Vec<(&'static Spec, String)>) {
        let refusal = if value.contains('\\') {
            Some(("escape", "an escape"))
        } else if value.contains("/*") {
            Some(("comment", "a comment"))
        } else if value.contains('!') {
            Some(("important", "!important"))
        } else if !style_urls_ok(value) {
            Some(("url", "a url() that is not url(#id)"))
        } else {
            None
        };
        if let Some((key, what)) = refusal {
            self.bad(&format!("style-{key}"), place, format!("svg: style holds {what}"), "style is a plain list of declarations, a property and its value each");
            return;
        }
        for piece in value.split(';') {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let Some((property, value)) = piece.split_once(':') else {
                self.bad("style-form", place, format!("svg: style holds {} without a colon", self.q(piece)), "style is a plain list of declarations, a property and its value each");
                continue;
            };
            let property = property.trim().to_ascii_lowercase();
            if let Some(spec) = scope.find(&property, true) {
                styled.retain(|(s, _)| s.name != spec.name);
                styled.push((spec, value.trim().to_string()));
            } else if let Some(dropped) = dropped_attr(&property) {
                self.dropped(dropped, place);
            } else {
                self.cx.drops.count("other style properties");
            }
        }
    }

    /// An attribute or a style property that is dropped: counted for the "Not drawn" line, except
    /// `transform-origin`, whose warning says what to use instead.
    fn dropped(&mut self, name: &'static str, place: &str) {
        if name == "transform-origin" {
            self.cx.out.warnings.add("transform-origin", place, "transform-origin is not used".to_string(), "give the part a pivot in pivots instead");
        } else {
            self.cx.drops.count(name);
        }
    }

    /// Reads one attribute value into the node. Returns the box a `d` or `points` spans.
    fn set(&mut self, node: &mut LookNode, matrix: &mut Affine, spec: &Spec, raw: &str, place: &str, zone: Zone, user_space: bool) -> Option<Bx> {
        let name = spec.name;
        let key = format!("value {name} {}", cut(raw, MAX_QUOTE_CHARS));
        let lead = format!("svg: {name} ");
        let view = self.view;
        let put = |node: &mut LookNode, value: AttrValue| {
            node.attrs.insert(spec.prop.to_string(), value);
        };
        match spec.kind {
            Kind::Id | Kind::ViewBox => {}
            Kind::Transform => match transform(raw, &view, self.cx.mask) {
                Ok((text, m)) => {
                    if !text.is_empty() {
                        put(node, AttrValue::Str(text));
                    }
                    if name == "transform" {
                        *matrix = m;
                    }
                }
                Err(fault) => self.cx.out.problems.fault(&key, place, "svg: ", &fault),
            },
            Kind::Paint | Kind::StopColour => {
                let value = raw.trim();
                let paint = spec.kind == Kind::Paint;
                if paint && value.eq_ignore_ascii_case("none") {
                    put(node, AttrValue::Str("none".to_string()));
                } else if value.eq_ignore_ascii_case("currentColor") {
                    put(node, AttrValue::Str("currentColor".to_string()));
                } else if let Some(c) = colour(value) {
                    put(node, AttrValue::Str(hex(c)));
                } else if let Some(id) = url_id(value).filter(|_| paint) {
                    node.attrs.insert(format!("{}Ref", spec.prop), AttrValue::Str(id.to_string()));
                    self.references.push(Reference { attr: name, id: id.to_string(), wants: Wants::Gradient, place: place.to_string() });
                } else if value.get(..4).is_some_and(|head| head.eq_ignore_ascii_case("url(")) {
                    let local = value.find(')').and_then(|end| value.get(..=end)).is_some_and(|head| url_id(head).is_some());
                    if local && paint {
                        self.bad(&key, place, format!("{lead}{} is more than a reference", self.q(value)), "url(#id) alone, without a colour to fall back on");
                    } else {
                        self.bad(&key, place, format!("{lead}{} reaches outside the artwork", self.q(value)), if paint { "url(#id) of a gradient in this svg" } else { "a plain colour" });
                    }
                } else {
                    self.bad(&key, place, format!("{lead}{} is not a colour", self.q(value)), COLOUR_FORMS);
                }
            }
            Kind::ClipRef => {
                let value = raw.trim();
                if zone.shapes_only() {
                    let what = if zone == Zone::Clip { "a clipPath and its shapes are not clipped themselves" } else { "a mask and its shapes are not clipped themselves" };
                    self.bad("clip-in-clip", place, format!("svg: clip-path inside a {}", zone.name()), what);
                } else if let Some(id) = url_id(value) {
                    node.attrs.insert("clipPathRef".to_string(), AttrValue::Str(id.to_string()));
                    self.references.push(Reference { attr: name, id: id.to_string(), wants: Wants::Clip, place: place.to_string() });
                } else if !value.eq_ignore_ascii_case("none") {
                    self.bad(&key, place, format!("{lead}{}", self.q(value)), "url(#id) of a clipPath in this svg");
                }
            }
            Kind::MaskRef | Kind::FilterRef => {
                let value = raw.trim();
                let (wants, stored, of) = if spec.kind == Kind::MaskRef { (Wants::Mask, "maskRef", "a mask") } else { (Wants::Filter, "filterRef", "a filter") };
                if zone.shapes_only() {
                    self.bad(&format!("{name}-in-{}", zone.name()), place, format!("svg: {name} inside a {}", zone.name()), &format!("a {} and its shapes are not masked or filtered themselves", zone.name()));
                } else if let Some(id) = url_id(value) {
                    node.attrs.insert(stored.to_string(), AttrValue::Str(id.to_string()));
                    self.references.push(Reference { attr: name, id: id.to_string(), wants, place: place.to_string() });
                } else if value.eq_ignore_ascii_case("none") {
                } else if wants == Wants::Filter && value.contains('(') && !value.get(..4).is_some_and(|head| head.eq_ignore_ascii_case("url(")) {
                    self.bad(&key, place, format!("{lead}{} is a CSS filter", self.q(value)), "write a filter element in defs and refer to it with url(#id)");
                } else {
                    self.bad(&key, place, format!("{lead}{}", self.q(value)), &format!("url(#id) of {of} in this svg, or none"));
                }
            }
            Kind::Href => match raw.trim().strip_prefix('#').filter(|id| valid_id(id)) {
                Some(id) => {
                    node.attrs.insert("useRef".to_string(), AttrValue::Str(id.to_string()));
                }
                None => self.bad(&key, place, format!("{lead}{} reaches outside the artwork", self.q(raw.trim())), "#id of a g, a shape or a text in this svg"),
            },
            Kind::Dashes => {
                let value = raw.trim();
                const FORM: &str = "up to 8 lengths, dash and gap in turn, separated by spaces, or none";
                if value.eq_ignore_ascii_case("none") {
                    put(node, AttrValue::Str("none".to_string()));
                } else {
                    match number_list(value, MAX_DASHES + 1) {
                        Some(list) if list.is_empty() || list.len() > MAX_DASHES => self.bad(&key, place, format!("{lead}holds {} numbers", list.len()), FORM),
                        Some(list) if list.iter().all(|n| view.length(*n)) => put(node, AttrValue::Str(list.iter().map(|n| fmt_num(*n)).collect::<Vec<_>>().join(" "))),
                        Some(_) => self.bad(&key, place, format!("{lead}{} is out of range", self.q(value)), &format!("lengths from 0 to {}", fmt_num(view.reach()))),
                        None => self.bad(&key, place, format!("{lead}{}", self.q(value)), FORM),
                    }
                }
            }
            Kind::In => {
                let value = raw.trim();
                if matches!(value, "SourceGraphic" | "SourceAlpha") || self.earlier.iter().any(|r| r == value) {
                    put(node, AttrValue::Str(value.to_string()));
                } else if STANDARD_INPUTS.contains(&value) {
                    self.bad(&key, place, format!("{lead}{} is not given to an avatar's filters", self.q(value)), INPUT_FORM);
                } else {
                    self.bad(&key, place, format!("{lead}{} is not the result of an earlier primitive", self.q(value)), INPUT_FORM);
                }
            }
            Kind::Result => {
                let value = raw.trim();
                if let Some(fault) = name_fault(value) {
                    self.bad(&key, place, format!("{lead}{}", self.q(value)), fault);
                } else if STANDARD_INPUTS.contains(&value) {
                    self.bad(&key, place, format!("{lead}{value} would hide the input of that name"), "name a result in your own words, such as blur or glow");
                } else {
                    put(node, AttrValue::Str(value.to_string()));
                }
            }
            Kind::Blur | Kind::Radius | Kind::Frequency | Kind::MatrixValues => {
                let (most, least, at_most, form): (f64, f64, usize, &str) = match spec.kind {
                    Kind::Blur => (round3(view.w / 4.0), 0.0, 2, "one or two numbers, x and y"),
                    Kind::Radius => (round3(view.w / 10.0), 0.0, 2, "one or two numbers, x and y"),
                    Kind::Frequency => (1.0, 0.0, 2, "one or two numbers, x and y"),
                    _ => (MAX_ROTATE, -MAX_ROTATE, MATRIX_VALUES, "1 to 20 numbers separated by spaces"),
                };
                let range = if least < 0.0 { format!("each within {} either way", fmt_num(most)) } else { format!("each from 0 to {}", fmt_num(most)) };
                let accepted = format!("{form}, {range}");
                match number_list(raw, at_most + 1) {
                    Some(list) if !list.is_empty() && list.len() <= at_most && list.iter().all(|n| (least..=most).contains(n)) => put(node, AttrValue::Str(list.iter().map(|n| fmt_num(*n)).collect::<Vec<_>>().join(" "))),
                    Some(list) if !list.is_empty() && list.len() <= at_most => self.bad(&key, place, format!("{lead}{} is out of range", self.q(raw.trim())), &accepted),
                    _ => self.bad(&key, place, format!("{lead}{}", self.q(raw.trim())), &accepted),
                }
            }
            Kind::Coord | Kind::Length | Kind::FontSize | Kind::Miter | Kind::LetterSpacing | Kind::Distance | Kind::Factor | Kind::Octaves | Kind::Seed | Kind::Displace => {
                if spec.kind == Kind::LetterSpacing && raw.trim().eq_ignore_ascii_case("normal") {
                    return None;
                }
                let (ok, range): (fn(&View, f64) -> bool, String) = match spec.kind {
                    Kind::Coord => (|v, n| v.coord(n), view.coord_range()),
                    Kind::Length => (|v, n| v.length(n), format!("0 to {}", fmt_num(view.reach()))),
                    // Above 0 as it is stored: what rounds to 0 would not read back.
                    Kind::FontSize => (|v, n| round3(n) > 0.0 && v.length(n), format!("above 0, up to {}", fmt_num(view.reach()))),
                    Kind::Miter => (|_, n| n >= MITER.0 && n <= MITER.1, format!("{} to {}", MITER.0, MITER.1)),
                    Kind::Factor => (|_, n| n.abs() <= MAX_FACTOR, format!("within {MAX_FACTOR} either way")),
                    Kind::Octaves => (|_, n| n.fract() == 0.0 && n >= OCTAVES.0 && n <= OCTAVES.1, format!("a whole number from {} to {}", OCTAVES.0, OCTAVES.1)),
                    Kind::Seed => (|_, n| n.abs() <= MAX_SEED, format!("within {} either way", fmt_num(MAX_SEED))),
                    Kind::Displace => (|v, n| n.abs() <= round3(v.w / 4.0), format!("within {} either way, a quarter of the viewBox width", fmt_num(round3(view.w / 4.0)))),
                    _ => (|v, n| v.offset(n), format!("within {}", fmt_num(view.reach()))),
                };
                match plain_number(raw) {
                    Some(n) if ok(&view, n) => put(node, AttrValue::Num(Num::new(n))),
                    Some(n) if n.is_finite() => self.bad(&key, place, format!("{lead}{} is out of range", fmt_num(n)), &range),
                    Some(_) => self.bad(&key, place, format!("{lead}{} is beyond any size", self.q(raw.trim())), &range),
                    None => self.bad(&key, place, format!("{lead}{} is not a number", self.q(raw.trim())), "a plain number, px is dropped, other units and percentages are not accepted"),
                }
            }
            Kind::Opacity | Kind::Offset => match scalar(raw) {
                Some(Scalar::Plain(n)) if n.is_finite() => put(node, AttrValue::Num(Num::new(n.clamp(0.0, 1.0)))),
                Some(Scalar::Percent(n)) if n.is_finite() => put(node, AttrValue::Num(Num::new((n / 100.0).clamp(0.0, 1.0)))),
                _ => self.bad(&key, place, format!("{lead}{} is not a number", self.q(raw.trim())), "a number from 0 to 1, or a percentage"),
            },
            Kind::GradX | Kind::GradY | Kind::GradR | Kind::RegionX | Kind::RegionY | Kind::RegionW | Kind::RegionH => {
                // A percentage is a share of the box (objectBoundingBox) or of the viewBox.
                let whole = match (user_space, spec.kind) {
                    (false, _) => 1.0,
                    (true, Kind::GradX | Kind::RegionX | Kind::RegionW) => view.w,
                    (true, Kind::GradY | Kind::RegionY | Kind::RegionH) => view.h,
                    (true, _) => ((view.w * view.w + view.h * view.h) / 2.0).sqrt(),
                };
                let n = match scalar(raw) {
                    Some(Scalar::Plain(n)) => Some(n),
                    Some(Scalar::Percent(n)) => Some(n / 100.0 * whole),
                    None => None,
                };
                // A radius, a width and a height are lengths, never negative.
                let radius = matches!(spec.kind, Kind::GradR | Kind::RegionW | Kind::RegionH);
                let fits = |n: f64| match (user_space, radius) {
                    (true, true) => view.length(n),
                    (true, false) => view.coord(n),
                    (false, true) => (0.0..=MAX_BOX_FRACTION).contains(&n),
                    (false, false) => n.abs() <= MAX_BOX_FRACTION,
                };
                match n {
                    Some(n) if fits(n) => put(node, AttrValue::Num(Num::new(n))),
                    Some(_) => self.bad(&key, place, format!("{lead}{} is out of range", self.q(raw.trim())), if user_space { "a coordinate near the viewBox" } else { "a share of the painted box, such as 0.5 or 50%, at most 1000%" }),
                    None => self.bad(&key, place, format!("{lead}{} is not a number", self.q(raw.trim())), "a number or a percentage"),
                }
            }
            Kind::Choice(choices) => match choices.iter().find(|c| c.eq_ignore_ascii_case(raw.trim())) {
                Some(choice) => put(node, AttrValue::Str(choice.to_string())),
                None => self.bad(&key, place, format!("{lead}{}", self.q(raw.trim())), &format!("one of {}", choices.join(", "))),
            },
            Kind::FontWeight => {
                let value = raw.trim();
                match plain_number(value) {
                    // The nine weights a font has, as the browser checks them again.
                    Some(n) if (100.0..=900.0).contains(&n) && n % 100.0 == 0.0 => put(node, AttrValue::Num(Num::new(n))),
                    None if value.eq_ignore_ascii_case("normal") => put(node, AttrValue::Str("normal".to_string())),
                    None if value.eq_ignore_ascii_case("bold") => put(node, AttrValue::Str("bold".to_string())),
                    _ => self.bad(&key, place, format!("{lead}{}", self.q(value)), "normal, bold or one of 100, 200 and so on to 900"),
                }
            }
            Kind::PathData => match path_data(raw, &view, MAX_D_CHARS, self.cx.mask) {
                Ok(path) => {
                    put(node, AttrValue::Str(path.text));
                    return path.bbox;
                }
                Err(fault) => self.cx.out.problems.fault(&format!("value d {}", cut(&fault, MAX_QUOTE_CHARS)), place, "svg: ", &fault),
            },
            Kind::Points => match point_list(raw, &view) {
                Ok((text, bbox)) => {
                    put(node, AttrValue::Str(text));
                    return bbox;
                }
                Err(fault) => self.cx.out.problems.fault(&format!("value points {}", cut(&fault, MAX_QUOTE_CHARS)), place, "svg: ", &fault),
            },
        }
        None
    }

    /// The `svg` root: its viewBox, and its presentation attributes for the wrapping group.
    fn root(&mut self, e: &BytesStart<'_>, at: (usize, usize)) {
        let place = self.place(at, None);
        let given = self.gather(e, Scope::Root, &place);
        if let Some((_, raw)) = given.iter().find(|(s, _)| s.kind == Kind::ViewBox) {
            let numbers = number_list(raw, 4).filter(|n| n.len() == 4 && n.iter().all(|v| v.is_finite()));
            match numbers.as_deref() {
                Some([x, y, w, h]) if (MIN_VIEW..=MAX_VIEW).contains(w) && (MIN_VIEW..=MAX_VIEW).contains(h) && x.abs() <= MAX_VIEW && y.abs() <= MAX_VIEW => {
                    self.view = View { x: round3(*x), y: round3(*y), w: round3(*w), h: round3(*h) };
                }
                Some(_) => self.bad("viewbox", &place, format!("svg: viewBox {} is out of range", self.q(raw.trim())), &format!("width and height from {MIN_VIEW} to {MAX_VIEW}, x and y within {MAX_VIEW}")),
                None => self.bad("viewbox", &place, format!("svg: viewBox {}", self.q(raw.trim())), "four numbers, x y width height, such as 0 0 200 200"),
            }
        }
        let mut wrapper = LookNode { tag: Tag::G, id: None, attrs: BTreeMap::new(), bbox: None, children: Vec::new(), text: None };
        let mut matrix = IDENTITY;
        for (spec, raw) in &given {
            self.set(&mut wrapper, &mut matrix, spec, raw, &place, Zone::Drawn, false);
        }
        if !wrapper.attrs.is_empty() {
            self.wrapper = Some(wrapper);
            self.elements += 1;
        }
    }

    /// A kept element from its start tag.
    fn build(&mut self, e: &BytesStart<'_>, tag: Tag, at: (usize, usize), zone: Zone) -> Open {
        // A look ahead for the id: it names the place of everything on this element. An id that
        // would not pass is left out of the place, as nothing escapes it yet.
        let named = e.try_get_attribute("id").ok().flatten().map(|a| a.value.into_owned()).filter(|id| valid_id(id));
        let place = self.place(at, named.as_deref().map(|id| (tag, id)));
        let given = self.gather(e, Scope::Tag(tag), &place);
        let mut node = LookNode { tag, id: None, attrs: BTreeMap::new(), bbox: None, children: Vec::new(), text: None };
        if let Some((_, id)) = given.iter().find(|(s, _)| s.kind == Kind::Id) {
            if let Some(fault) = name_fault(id) {
                self.bad(&format!("id {}", cut(id, MAX_QUOTE_CHARS)), &place, format!("svg: the id {}", self.q(id)), fault);
            } else if self.ids.contains_key(id) {
                self.bad(&format!("id-twice {id}"), &place, format!("svg: the id {id} is used twice"), "an id names one element");
            } else {
                self.ids.insert(id.clone(), tag);
                node.id = Some(id.clone());
            }
        }
        let user_space = given.iter().any(|(s, v)| matches!(s.name, "gradientUnits" | "maskUnits" | "filterUnits") && v.trim().eq_ignore_ascii_case("userSpaceOnUse"));
        if tag.is_primitive() || tag == Tag::FeMergeNode {
            self.earlier = self.results();
        }
        if tag == Tag::Use && !given.iter().any(|(s, _)| s.kind == Kind::Href) {
            self.bad("use-href", &place, "svg: a use without href".to_string(), "href='#id' of a g, a shape or a text in this svg");
        }
        let mut matrix = IDENTITY;
        let mut drawn: Option<Bx> = None;
        for (spec, raw) in &given {
            if let Some(b) = self.set(&mut node, &mut matrix, spec, raw, &place, zone, user_space) {
                drawn = Some(b);
            }
        }
        if tag == Tag::FeColorMatrix {
            self.colour_matrix(&node, &place);
        }
        let n = |key: &str| node.attrs.get(key).and_then(AttrValue::num).unwrap_or(0.0);
        let bbox = match tag {
            Tag::Circle => Some(Bx::sized(n("cx") - n("r"), n("cy") - n("r"), 2.0 * n("r"), 2.0 * n("r"))),
            Tag::Ellipse => Some(Bx::sized(n("cx") - n("rx"), n("cy") - n("ry"), 2.0 * n("rx"), 2.0 * n("ry"))),
            Tag::Rect => Some(Bx::sized(n("x"), n("y"), n("width"), n("height"))),
            Tag::Line => {
                let mut b = Bx::at(n("x1"), n("y1"));
                b.add(n("x2"), n("y2"));
                Some(b)
            }
            Tag::Path | Tag::Polyline | Tag::Polygon => drawn,
            _ => None,
        };
        Open { node, at, matrix, bbox, text: String::new(), zone, size: 1, holds_use: false }
    }

    /// The results of the primitives of the filter being read, so far.
    fn results(&self) -> Vec<String> {
        let filter = self.stack.iter().rev().find_map(|f| match f {
            Frame::Node(open) if open.node.tag == Tag::Filter => Some(open),
            _ => None,
        });
        filter.into_iter().flat_map(|open| &open.node.children).filter_map(|p| p.attrs.get("result").and_then(AttrValue::str)).map(str::to_string).collect()
    }

    /// The `values` of a colour matrix are as many as its `type` takes: 20 for a matrix, one for
    /// saturate and hueRotate, none for luminanceToAlpha. Without values it is the identity.
    fn colour_matrix(&mut self, node: &LookNode, place: &str) {
        let kind = node.attrs.get("type").and_then(AttrValue::str).unwrap_or("matrix");
        let Some(count) = node.attrs.get("values").and_then(AttrValue::str).map(|v| v.split(' ').count()) else { return };
        let (wanted, form) = match kind {
            "matrix" => (MATRIX_VALUES, "20 numbers, four rows of five"),
            "saturate" => (1, "one number, 0 for grey and 1 for the colours as they are"),
            "hueRotate" => (1, "one number, the degrees the hues turn"),
            _ => (0, "no values"),
        };
        if count != wanted {
            self.bad(&format!("matrix-values {kind}"), place, format!("svg: a feColorMatrix of type {kind} with {count} values"), form);
        }
    }

    fn close(&mut self) {
        match self.stack.pop() {
            None => self.doc = Doc::After,
            Some(Frame::Node(open)) => self.finish(*open),
            Some(Frame::Tspan | Frame::Skip) => {}
        }
    }

    /// A kept element is complete: its text is cleaned, its box is known, it joins its parent.
    fn finish(&mut self, mut open: Open) {
        let tag = open.node.tag;
        if tag == Tag::Text {
            match text::plain(&open.text, MAX_TEXT_CHARS, self.cx.mask) {
                Ok(words) => {
                    open.bbox = Some(text_box(&open.node, &words));
                    open.node.text = Some(words);
                }
                Err(refusal) => {
                    let place = self.place(open.at, open.node.id.as_deref().map(|id| (tag, id)));
                    self.bad("text-long", &place, format!("svg: a text has {refusal}"), "an avatar speaks in lines, a text in the artwork is a few characters");
                    open.node.text = Some(String::new());
                }
            }
        }
        if tag == Tag::Use {
            self.repeat(&mut open);
        }
        if let Some(id) = &open.node.id {
            let own = open.bbox.unwrap_or(Bx::ZERO);
            if let Some(what) = too_far(id, &own) {
                let place = self.place(open.at, Some((tag, id)));
                self.bad("span", &place, what, &span_accepted());
            }
            open.node.bbox = Some(own.nums());
            let placed = open.bbox.map(|b| b.map(&open.matrix));
            self.done.insert(id.clone(), Done { tag, zone: open.zone, placed, size: open.size, holds_use: open.holds_use });
        }
        match self.stack.last_mut() {
            Some(Frame::Node(parent)) => {
                // A group spans its children through their transforms; so do a clipPath and a mask.
                if tag.is_part() && matches!(parent.node.tag, Tag::G | Tag::ClipPath | Tag::Mask) {
                    if let Some(b) = open.bbox {
                        grow(&mut parent.bbox, b.map(&open.matrix));
                    }
                }
                parent.size += open.size;
                parent.holds_use |= open.holds_use || tag == Tag::Use;
                parent.node.children.push(open.node);
            }
            None => self.top.push(open.node),
            Some(Frame::Tspan | Frame::Skip) => {}
        }
    }

    /// A `use` is complete. What it repeats: its box, moved by `x` and `y`, is the box of the
    /// `use`, and its elements count again. A target that is not complete yet holds the `use`
    /// (it is open), or comes after it: a first reading notes that, and the second knows it.
    fn repeat(&mut self, open: &mut Open) {
        let Some(target) = open.node.attrs.get("useRef").and_then(AttrValue::str).map(str::to_string) else { return };
        let place = self.place(open.at, open.node.id.as_deref().map(|id| (Tag::Use, id)));
        let n = |key: &str| open.node.attrs.get(key).and_then(AttrValue::num).unwrap_or(0.0);
        let (x, y) = (n("x"), n("y"));
        let known = match self.done.get(&target) {
            Some(done) => Some(*done),
            None if open.node.id.as_deref() == Some(target.as_str()) || self.ids.contains_key(&target) => None,
            None => self.ahead.get(&target).copied(),
        };
        let fault = match known {
            None if open.node.id.as_deref() == Some(target.as_str()) => Some(("use-self", "svg: a use refers to itself".to_string(), "a use repeats another element of the artwork")),
            None if self.ids.contains_key(&target) => Some(("use-holds", format!("svg: href refers to #{target}, which holds this use"), "a use repeats an element outside itself")),
            None => {
                self.later.push(Later { id: target, place });
                return;
            }
            Some(done) if done.tag == Tag::Use => Some(("use-use", format!("svg: href refers to #{target}, a use"), "a use repeats a g, a shape or a text, not another use")),
            Some(done) if !done.tag.is_repeatable() => Some(("use-kind", format!("svg: href refers to #{target}, a {}", done.tag.name()), "#id of a g, a shape or a text in this svg")),
            Some(done) if matches!(done.zone, Zone::Clip | Zone::Mask | Zone::Filter) => {
                Some(("use-zone", format!("svg: href refers to #{target}, which is inside a {}", done.zone.name()), "what a use repeats stands outside clip paths, masks and filters"))
            }
            Some(done) if done.holds_use => Some(("use-nested", format!("svg: href refers to #{target}, which holds a use"), "what a use repeats holds no use itself")),
            Some(done) => {
                open.bbox = done.placed.map(|b| b.shifted(x, y));
                open.size += done.size;
                self.elements += done.size;
                None
            }
        };
        if let Some((key, what, accepted)) = fault {
            self.bad(&format!("{key} {target}"), &place, what, accepted);
        }
        if self.elements > MAX_ELEMENTS {
            self.fatal("elements-use", &place, format!("svg: more than {MAX_ELEMENTS} elements, with what each use repeats"), "draw with fewer, larger shapes, or repeat smaller parts");
        }
    }

    /// The document has been read to its end.
    fn end(mut self, problems_before: usize) -> Read {
        let end = self.locate(self.src.len());
        if self.doc == Doc::Before {
            self.bad("root", "", "svg: no svg element".to_string(), "the artwork is one svg element, such as <svg viewBox='0 0 200 200'>…</svg>");
            return Read::Done(None);
        }
        if self.doc == Doc::In || !self.stack.is_empty() {
            let open = self.stack.iter().rev().find_map(|f| match f {
                Frame::Node(open) => Some(open.node.tag.name()),
                _ => None,
            });
            let place = self.place(end, None);
            self.bad("unclosed", &place, format!("svg ends inside <{}>", open.unwrap_or("svg")), "every element is closed, the svg element last");
            return Read::Done(None);
        }
        // A use repeats an element that comes after it: read again, knowing what it is.
        if self.ahead.is_empty() && self.later.iter().any(|later| self.done.contains_key(&later.id)) {
            return Read::Again(std::mem::take(&mut self.done));
        }
        for reference in std::mem::take(&mut self.references) {
            let (wanted, fits): (&str, fn(Tag) -> bool) = match reference.wants {
                Wants::Gradient => ("a gradient", Tag::is_gradient),
                Wants::Clip => ("a clipPath", |tag| tag == Tag::ClipPath),
                Wants::Mask => ("a mask", |tag| tag == Tag::Mask),
                Wants::Filter => ("a filter", |tag| tag == Tag::Filter),
            };
            let accepted = format!("url(#id) of {wanted} in this svg");
            match self.ids.get(&reference.id).copied() {
                Some(tag) if fits(tag) => {}
                Some(tag) => self.bad(&format!("ref-kind {} {}", reference.attr, reference.id), &reference.place, format!("svg: {} refers to #{}, a {}", reference.attr, reference.id, tag.name()), &accepted),
                None => self.bad(&format!("ref-none {} {}", reference.attr, reference.id), &reference.place, format!("svg: {} refers to #{}, which is not in this svg", reference.attr, reference.id), &accepted),
            }
        }
        // A use whose target is in no reading: one that is not in the artwork.
        for later in std::mem::take(&mut self.later) {
            self.bad(&format!("use-none {}", later.id), &later.place, format!("svg: href refers to #{}, which is not in this svg", later.id), "#id of a g, a shape or a text in this svg");
        }
        let mut nodes = std::mem::take(&mut self.top);
        if !nodes.iter().any(|n| n.tag.is_part()) && self.cx.out.problems.len() == problems_before {
            self.bad("empty", "", "svg: nothing to draw".to_string(), "shapes, groups of shapes or text inside the svg element");
        }
        if let Some(mut wrapper) = self.wrapper.take() {
            wrapper.children = nodes;
            nodes = vec![wrapper];
        }
        Read::Done(Some(Artwork { view: self.view, nodes }))
    }
}

const ENTITIES: &str = "only &amp; &lt; &gt; &quot; &apos; and numeric character references";
/// The inputs SVG gives a filter primitive by name. Only the first two are drawn for an avatar;
/// none may be the name of a result.
const STANDARD_INPUTS: [&str; 6] = ["SourceGraphic", "SourceAlpha", "BackgroundImage", "BackgroundAlpha", "FillPaint", "StrokePaint"];
const INPUT_FORM: &str = "SourceGraphic, SourceAlpha or an earlier result";

/// For an `href` anywhere but on a `use`.
const HREF_ONLY_USE: &str = "the artwork links to nothing; only a use refers to an element of it, with href='#id'";

/// Whether every `url(` of a style value is `url(#id)`.
fn style_urls_ok(value: &str) -> bool {
    // Lower-casing ASCII keeps every byte offset.
    let lower = value.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower.get(from..).and_then(|rest| rest.find("url(")) {
        let start = from + found;
        let Some(len) = lower.get(start..).and_then(|rest| rest.find(')')) else { return false };
        let Some(whole) = value.get(start..start + len + 1) else { return false };
        if url_id(whole).is_none() {
            return false;
        }
        from = start + len + 1;
    }
    true
}

fn span_accepted() -> String {
    format!("every part within {} units, drawn to the scale of the viewBox", fmt_num(MAX_SPAN))
}

/// The problem of a box beyond [`MAX_SPAN`], or `None` when it keeps within it.
fn too_far(id: &str, b: &Bx) -> Option<String> {
    let far = b.span();
    // A number that is not one counts as too far.
    (!(far <= MAX_SPAN)).then(|| format!("svg: {id} reaches {} units from the origin", if far.is_finite() { fmt_num(far) } else { "countless".to_string() }))
}

/// The estimated box of a `text`: nothing here measures a font.
fn text_box(node: &LookNode, words: &str) -> Bx {
    let n = |key: &str| node.attrs.get(key).and_then(AttrValue::num);
    let s = |key: &str| node.attrs.get(key).and_then(AttrValue::str).unwrap_or_default();
    let size = n("fontSize").unwrap_or(DEFAULT_FONT_SIZE);
    let count = words.chars().count() as f64;
    let width = (count * size * TEXT_ADVANCE + (count - 1.0).max(0.0) * n("letterSpacing").unwrap_or(0.0)).max(0.0);
    let (x, y) = (n("x").unwrap_or(0.0), n("y").unwrap_or(0.0));
    let left = match s("textAnchor") {
        "middle" => x - width / 2.0,
        "end" => x - width,
        _ => x,
    };
    let top = match s("dominantBaseline") {
        "middle" | "central" => y - size / 2.0,
        "hanging" => y,
        _ => y - size * TEXT_ASCENT,
    };
    Bx::sized(left, top, width, size)
}

// ---------------------------------------------------------------- the parts of an artwork

/// A view wide enough to read back anything this file stored, under whatever viewBox it was
/// accepted: every viewBox reaches at most from -5 to 6 times [`MAX_VIEW`], this one from -9 to 9.
const WIDE: View = View { x: -MAX_VIEW, y: -MAX_VIEW, w: 2.0 * MAX_VIEW, h: 2.0 * MAX_VIEW };

/// The elements of an artwork by id, as the checks of pivots, roles, animations and the mouth
/// need them. Built from the stored tree, so a merge works on the current look as a new design
/// works on fresh artwork.
struct Index<'n> {
    by_id: HashMap<&'n str, Entry<'n>>,
    /// The ids of the parts, in document order.
    order: Vec<&'n str>,
    /// Every element of the tree, each `use` with the elements it repeats.
    elements: usize,
}

struct Entry<'n> {
    node: &'n LookNode,
    /// A `g`, a shape, a `text` or a `use` that is drawn (not inside `defs`): what a pivot, a
    /// role, a parent, a track or a drive may name.
    part: bool,
    /// The ids of the elements it stands inside, outermost first.
    above: Vec<&'n str>,
    /// The elements it stands inside, with an id or without, outermost first: what it inherits
    /// a paint from.
    around: Vec<&'n LookNode>,
    /// From the coordinates its geometry is written in to root coordinates.
    to_root: Affine,
    /// The structure of its own `d`, when it is a path that has one: read once, asked for by
    /// every `d` track and mouth that names it.
    structure: Option<Structure>,
    /// The stroke it is drawn with at rest, its own or inherited.
    stroke: Stroke,
}

/// The stroke of an element at rest, as it inherits it and sets it: whether one is painted, its
/// width (1 by default), and whether its ends are round or square.
#[derive(Debug, Clone, Copy)]
struct Stroke {
    painted: bool,
    width: f64,
    capped: bool,
}

impl Stroke {
    /// What the root inherits: no stroke.
    const NONE: Stroke = Stroke { painted: false, width: 1.0, capped: false };

    /// The stroke of `node`, which inherits `above`.
    fn of(node: &LookNode, above: Stroke) -> Stroke {
        let text = |key: &str| node.attrs.get(key).and_then(AttrValue::str);
        let painted = if node.attrs.contains_key("strokeRef") { true } else { text("stroke").map_or(above.painted, |paint| paint != "none") };
        let width = node.attrs.get("strokeWidth").and_then(AttrValue::num).unwrap_or(above.width);
        let capped = text("strokeLinecap").map_or(above.capped, |cap| cap != "butt");
        Stroke { painted, width, capped }
    }
}

impl<'n> Index<'n> {
    /// Walks the tree with a stack of its own: the depth of the input never becomes call depth.
    fn of(nodes: &'n [LookNode]) -> Index<'n> {
        let mut ix = Index { by_id: HashMap::new(), order: Vec::new(), elements: 0 };
        let mut stack: Vec<(&'n LookNode, Affine, bool, Stroke, Vec<&'n str>, Vec<&'n LookNode>)> = nodes.iter().rev().map(|n| (n, IDENTITY, false, Stroke::NONE, Vec::new(), Vec::new())).collect();
        let mut repeated: Vec<&'n str> = Vec::new();
        while let Some((node, matrix, hidden, inherited, above, around)) = stack.pop() {
            ix.elements += 1;
            let to_root = compose(&matrix, &matrix_of(node));
            let hidden = hidden || !node.tag.is_part();
            let stroke = Stroke::of(node, inherited);
            if let Some(target) = node.attrs.get("useRef").and_then(AttrValue::str) {
                repeated.push(target);
            }
            let mut inside = above.clone();
            if let Some(id) = &node.id {
                inside.push(id);
                if !ix.by_id.contains_key(id.as_str()) {
                    if !hidden {
                        ix.order.push(id);
                    }
                    let structure = node.attrs.get("d").and_then(AttrValue::str).and_then(|d| path_data(d, &WIDE, MAX_D_CHARS, &unmasked).ok()).map(|p| p.structure);
                    ix.by_id.insert(id, Entry { node, part: !hidden, above, around: around.clone(), to_root, structure, stroke });
                }
            }
            let mut held = around;
            held.push(node);
            for child in node.children.iter().rev() {
                stack.push((child, to_root, hidden, stroke, inside.clone(), held.clone()));
            }
        }
        // What each use repeats counts again, as it counted when the artwork was read.
        for target in repeated {
            let mut count: Vec<&LookNode> = ix.by_id.get(target).map(|entry| vec![entry.node]).unwrap_or_default();
            while let Some(node) = count.pop() {
                ix.elements += 1;
                count.extend(&node.children);
            }
        }
        ix
    }

    /// "ids are armL, armR, body", for a target that was not found.
    fn ids(&self) -> String {
        if self.order.is_empty() { "the artwork has no ids, give every part that moves an id".to_string() } else { format!("ids are {}", listed(&self.order)) }
    }
}

impl Entry<'_> {
    /// Its rest-pose box in the coordinates its geometry is written in.
    fn own_box(&self) -> Bx {
        self.node.bbox.as_ref().map_or(Bx::ZERO, Bx::of)
    }

    fn root_box(&self) -> Bx {
        self.own_box().map(&self.to_root)
    }

    /// Its rest-pose box in root coordinates as it is painted: its geometry widened by half the
    /// width of its stroke. A path or a line of no length paints a dot only with round or square
    /// ends; any other shape of no size paints nothing, stroke or not.
    fn painted_box(&self) -> Bx {
        let own = self.own_box();
        let point = own.w() == 0.0 && own.h() == 0.0;
        let dot = matches!(self.node.tag, Tag::Path | Tag::Line | Tag::Polyline) && self.stroke.capped;
        if !self.stroke.painted || (point && !dot) {
            return own.map(&self.to_root);
        }
        let half = self.stroke.width / 2.0;
        Bx { x0: own.x0 - half, y0: own.y0 - half, x1: own.x1 + half, y1: own.y1 + half }.map(&self.to_root)
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.node.attrs.get(key).and_then(AttrValue::num)
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.node.attrs.get(key).and_then(AttrValue::str)
    }

    /// The artwork's own value of an absolute prop, where that can be told from the element alone.
    fn own(&self, prop: Prop) -> Own<'_> {
        match prop {
            Prop::Opacity => Own::Num(self.num("opacity").unwrap_or(1.0)),
            // These are inherited: an element that does not set one has none of its own.
            Prop::StrokeWidth | Prop::FillOpacity | Prop::StrokeOpacity | Prop::StrokeDashoffset => self.num(prop.name()).map_or(Own::Unknown, Own::Num),
            Prop::Fill | Prop::Stroke => self.str(prop.name()).filter(|c| c.starts_with('#')).map_or(Own::Unknown, Own::Text),
            Prop::D => self.str("d").map_or(Own::Unknown, Own::Text),
            p if p.is_geometry() => Own::Num(self.num(p.name()).unwrap_or(0.0)),
            _ => Own::Unknown,
        }
    }
}

/// The artwork's own value of a prop on one element. `Unknown`: it is inherited, or not plain.
enum Own<'n> {
    Num(f64),
    Text(&'n str),
    Unknown,
}

impl Own<'_> {
    fn is(&self, key: &Option<Key>) -> bool {
        match (key, self) {
            (None, _) | (_, Own::Unknown) => true,
            (Some(Key::Num(n)), Own::Num(own)) => (n.get() - own).abs() < 0.0005,
            (Some(Key::Str(s)), Own::Text(own)) => s == own,
            _ => false,
        }
    }

    fn shown(&self) -> String {
        match self {
            Own::Num(n) => format!(", {}", fmt_num(*n)),
            Own::Text(t) if t.starts_with('#') => format!(", {t}"),
            _ => String::new(),
        }
    }
}

/// Whether a track that begins with the keyframe `first` and ends with `last` ends on the value
/// it starts with. `own` answers for a `null` at one end, which is each target's own value: it is
/// asked whether every target's own value is the keyframe at the other end.
///
/// The rule a loop's paint tracks keep (where a loop starts again its last keyframe meets its
/// first), and what [`closes`] asks of an animation that is played several times in a row.
fn ends_as_it_starts(first: &Option<Key>, last: &Option<Key>, own: impl Fn(&Option<Key>) -> bool) -> bool {
    match (first, last) {
        (None, None) => true,
        (Some(Key::Num(a)), Some(Key::Num(b))) => (a.get() - b.get()).abs() < 0.0005,
        (Some(Key::Str(a)), Some(Key::Str(b))) => a == b,
        (Some(_), Some(_)) => false,
        (None, key) | (key, None) => own(key),
    }
}

// ---------------------------------------------------------------- what moves the artwork

/// A key of the arguments as a piece of a path: as written when it is a plain name, quoted (and
/// masked) otherwise.
fn seg(key: &str, mask: &dyn Fn(&str) -> String) -> String {
    if valid_id(key) { key.to_string() } else { q(mask, key) }
}

/// Unknown keys of an object are problems. False when there was one.
fn keys(o: &Map<String, Value>, known: &[&str], at: &str, out: &mut Out, mask: &dyn Fn(&str) -> String) -> bool {
    let mut ok = true;
    for key in o.keys().filter(|k| !known.contains(&k.as_str())).take(MAX_PROBLEM_KINDS) {
        let accepted = if key == "origin" {
            "there is no origin here, give the part a pivot in pivots".to_string()
        } else if known == DRIVE_KEYS && matches!(key.as_str(), "times" | "phase" | "delay") {
            "a drive follows its input, not time: at places each value along the input".to_string()
        } else {
            format!("keys are {}", known.join(", "))
        };
        out.problems.add(&format!("key {} {}", known.first().copied().unwrap_or_default(), cut(key, MAX_QUOTE_CHARS)), at, format!("{} is not a key here", q(mask, key)), &accepted);
        ok = false;
    }
    ok
}

const ANIMATION_KEYS: [&str; 6] = ["duration", "loop", "every", "times", "ease", "tracks"];
const TRACK_KEYS: [&str; 7] = ["target", "prop", "values", "times", "ease", "phase", "delay"];
const MOUTH_KEYS: [&str; 4] = ["target", "open", "closed", "prop"];
const DRIVE_KEYS: [&str; 6] = ["input", "target", "prop", "values", "at", "ease"];
const VOICE_KEYS: [&str; 4] = ["lang", "prefer", "pitch", "rate"];
const PROPS: &str = "props are translateX, translateY, rotate, skewX, skewY, scale, scaleX, scaleY, opacity, fillOpacity, strokeOpacity, fill, stroke, strokeWidth, strokeDashoffset, d, cx, cy, r, rx, ry, x, y, width, height, x1, y1, x2, y2";
const TIMES_FORM: &str = "numbers rising from 0 to 1, one per keyframe";
const EASES: &str = "linear, in, out, inOut, inBack, outBack, inOutBack, outElastic, outBounce, step or a bezier [x1, y1, x2, y2]";
const INPUTS: &str = "inputs are lookX, lookY, mouth, pace, turn, held";

/// `[a, b]` as two numbers.
fn pair(v: &Value) -> Option<[f64; 2]> {
    match v.as_array()?.as_slice() {
        [a, b] => Some([a.as_f64()?, b.as_f64()?]),
        _ => None,
    }
}

fn nums(list: Vec<f64>) -> Vec<Num> {
    list.into_iter().map(Num::new).collect()
}

/// Evenly spaced times for `n` keyframes.
fn even(n: usize) -> Vec<f64> {
    (0..n).map(|i| if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 }).collect()
}

/// `times` of an animation or a track, `at` of a drive (`what` names which).
fn check_times(v: &Value, what: &str, at: &str, out: &mut Out) -> Option<Vec<f64>> {
    let list: Option<Vec<f64>> = v.as_array().filter(|l| (2..=MAX_KEYFRAMES).contains(&l.len())).and_then(|l| l.iter().map(|t| t.as_f64().map(round3)).collect());
    match list {
        Some(t) if t.first() == Some(&0.0) && t.last() == Some(&1.0) && t.windows(2).all(|w| w[1] - w[0] >= MIN_TIME_STEP - 1e-9) => Some(t),
        Some(_) => {
            let verb = if what == "times" { "do" } else { "does" };
            out.problems.add(&format!("{what}-order"), at, format!("{what} {verb} not rise from 0 to 1"), TIMES_FORM);
            None
        }
        None => {
            out.problems.add(&format!("{what}-form"), at, format!("{what} is {}", match v {
                Value::Array(l) => format!("a list of {}", l.len()),
                other => kind_of(other).to_string(),
            }), &format!("2 to {MAX_KEYFRAMES} {TIMES_FORM}"));
            None
        }
    }
}

/// Four numbers, as a bezier is written: `None` when it is not four numbers at all.
fn four(v: &Value) -> Option<[f64; 4]> {
    match v.as_array()?.as_slice() {
        [a, b, c, d] => Some([a.as_f64()?, b.as_f64()?, c.as_f64()?, d.as_f64()?]),
        _ => None,
    }
}

/// A bezier whose control points keep to their ranges.
fn bezier(p: [f64; 4]) -> Option<Curve> {
    let x = |v: f64| (BEZIER_X.0..=BEZIER_X.1).contains(&v);
    let y = |v: f64| (BEZIER_Y.0..=BEZIER_Y.1).contains(&v);
    (x(p[0]) && y(p[1]) && x(p[2]) && y(p[3])).then(|| Curve::Bezier(p.map(Num::new)))
}

fn check_eases(v: &Value, at: &str, out: &mut Out, mask: &dyn Fn(&str) -> String) -> Option<Eases> {
    let named = |v: &Value| v.as_str().and_then(|name| Ease::ALL.into_iter().find(|e| e.name() == name)).map(Curve::Named);
    let one = |v: &Value| named(v).or_else(|| four(v).and_then(bezier));
    // Four numbers out of range: said as that, not as an unknown name.
    let wild = |v: &Value| four(v).is_some_and(|p| bezier(p).is_none());
    let eases = match v {
        Value::String(_) => named(v).map(Eases::One),
        Value::Array(_) if four(v).is_some() => four(v).and_then(bezier).map(Eases::One),
        Value::Array(list) if (1..MAX_KEYFRAMES).contains(&list.len()) => list.iter().map(one).collect::<Option<Vec<Curve>>>().map(Eases::Each),
        _ => None,
    };
    if eases.is_none() {
        if wild(v) || matches!(v, Value::Array(list) if list.iter().any(wild)) {
            let range = format!("x1 and x2 from {} to {}, y1 and y2 from {} to {}", BEZIER_X.0, BEZIER_X.1, BEZIER_Y.0, BEZIER_Y.1);
            out.problems.add("ease-bezier", at, "an ease [x1, y1, x2, y2] out of range".to_string(), &range);
            return None;
        }
        let found = match v {
            Value::String(s) => q(mask, s),
            Value::Array(list) => list.iter().find(|e| one(e).is_none()).and_then(Value::as_str).map_or_else(|| format!("a list of {}", list.len()), |s| q(mask, s)),
            other => kind_of(other).to_string(),
        };
        out.problems.add(&format!("ease {}", cut(&found, MAX_QUOTE_CHARS)), at, format!("ease {found}"), EASES);
    }
    eases
}

/// What a track takes from its animation.
struct Shared<'s> {
    duration: f64,
    looping: bool,
    times: Option<&'s [f64]>,
    ease: Option<&'s Eases>,
}

/// What a track and a drive share, read: the parts they name, the prop, the values.
struct Keyed<'v, 'a> {
    names: Vec<&'v str>,
    targets: Vec<(&'v str, &'a Entry<'a>)>,
    prop: Prop,
    /// The values that could be read.
    values: Vec<Option<Key>>,
    /// How many values were written.
    n: usize,
    ok: bool,
}

/// The pivots, roles and gaze of the look being put together, for the warnings that need them.
struct Rig<'r> {
    pivots: &'r BTreeMap<String, [Num; 2]>,
    roles: &'r BTreeMap<Role, String>,
    /// The gaze as it will be stored (think moves the pupils only with one above 0).
    gaze: Option<Num>,
}

/// The checks of what moves an artwork, against the parts of that artwork.
struct Check<'a> {
    ix: &'a Index<'a>,
    view: View,
    /// The session's secret masking, for what a problem quotes.
    mask: &'a dyn Fn(&str) -> String,
}

impl<'a> Check<'a> {
    /// The part an id names, or a problem.
    fn part(&self, id: &str, at: &str, out: &mut Out) -> Option<&'a Entry<'a>> {
        match self.ix.by_id.get(id) {
            Some(entry) if entry.part => Some(entry),
            Some(entry) => {
                // A shape that could move, were it drawn: it stands where nothing moves.
                let kept = matches!(entry.node.tag, Tag::G | Tag::Text | Tag::Use) || entry.node.tag.is_shape();
                if kept {
                    out.problems.add(&format!("not-part {id}"), at, format!("{id} stands inside defs, a mask or a clipPath, where nothing moves"), "move the masked or clipped part, or the use that repeats it");
                } else {
                    out.problems.add(&format!("not-part {id}"), at, format!("{id} is a {}, or inside defs", entry.node.tag.name()), "what moves is a g, a shape, a text or a use that is drawn");
                }
                None
            }
            None => {
                out.problems.add(&format!("no-target {}", cut(id, MAX_QUOTE_CHARS)), at, format!("target {} not found", q(self.mask, id)), &self.ix.ids());
                None
            }
        }
    }

    fn pivot(&self, id: &str, v: &Value, out: &mut Out) -> Option<[Num; 2]> {
        let at = format!("pivots.{}", seg(id, self.mask));
        let entry = self.part(id, &at, out)?;
        match pair(v) {
            Some([x, y]) if self.view.coord(x) && self.view.coord(y) => {
                // Where the transforms above the part put the point: bounded like the part itself.
                let (rx, ry) = point(&entry.to_root, x, y);
                if let Some(what) = too_far(&format!("the pivot of {id}"), &Bx::at(rx, ry)) {
                    out.problems.add("span", &at, what.replacen("svg: ", "", 1), &span_accepted());
                    return None;
                }
                Some([Num::new(x), Num::new(y)])
            }
            Some(_) => {
                out.problems.add("pivot-range", &at, "a pivot is out of range".to_string(), &self.view.coord_range());
                None
            }
            None => {
                out.problems.add("pivot-form", &at, format!("a pivot is {}", kind_of(v)), "[x, y], in the coordinates the part is drawn in");
                None
            }
        }
    }

    fn role(&self, key: &str, v: &Value, out: &mut Out) -> Option<(Role, String)> {
        let at = format!("roles.{}", seg(key, self.mask));
        let Some(role) = Role::ALL.into_iter().find(|r| r.name() == key) else {
            out.problems.add(&format!("role {}", cut(key, MAX_QUOTE_CHARS)), &at, format!("{} is not a role", q(self.mask, key)), "roles are body, head, eyes, pupils, armL, armR");
            return None;
        };
        let Some(id) = v.as_str() else {
            out.problems.add("role-form", &at, format!("a role is {}", kind_of(v)), "the id of the part that plays it");
            return None;
        };
        self.part(id, &at, out)?;
        Some((role, id.to_string()))
    }

    /// Whether a keyframe number fits a prop; the error is the range.
    fn number_fits(&self, prop: Prop, n: f64) -> Result<(), String> {
        let (ok, range) = match prop {
            Prop::TranslateX | Prop::TranslateY => (self.view.offset(n), format!("within {} either way", fmt_num(self.view.reach()))),
            Prop::Rotate => (n.abs() <= MAX_ROTATE, format!("degrees, within {MAX_ROTATE} either way")),
            Prop::SkewX | Prop::SkewY => (n.abs() <= MAX_SKEW, format!("degrees, within {MAX_SKEW} either way")),
            Prop::Scale | Prop::ScaleX | Prop::ScaleY => (n.abs() <= MAX_TRACK_SCALE, format!("-{MAX_TRACK_SCALE} to {MAX_TRACK_SCALE}")),
            p if p.is_opacity() => ((0.0..=1.0).contains(&n), "0 to 1".to_string()),
            Prop::StrokeDashoffset => (self.view.offset(n), format!("within {} either way", fmt_num(self.view.reach()))),
            p if p.is_length() => (self.view.length(n), format!("0 to {}", fmt_num(self.view.reach()))),
            _ => (self.view.coord(n), self.view.coord_range()),
        };
        if ok { Ok(()) } else { Err(range) }
    }

    /// A rotate, skew or scale on a `g` or a `path` without a pivot is accepted, with a warning
    /// that names the point it will use.
    fn pivot_warning(&self, id: &str, entry: &Entry<'_>, at: &str, rig: &Rig<'_>, out: &mut Out) {
        if !matches!(entry.node.tag, Tag::G | Tag::Path | Tag::Use) || rig.pivots.contains_key(id) {
            return;
        }
        let body = rig.roles.get(&Role::Body).is_some_and(|b| b == id);
        let (x, y) = default_pivot(entry.own_box(), body);
        let what = format!("{id} has no pivot: it turns and scales about the {} of its box, [{}, {}]", if body { "bottom centre" } else { "centre" }, fmt_num(x), fmt_num(y));
        out.warnings.add(&format!("no-pivot {id}"), at, what, "give it a pivot if its joint is elsewhere");
    }

    fn animation(&self, name: &str, v: &Value, rig: &Rig<'_>, out: &mut Out) -> Option<Animation> {
        let at = format!("animations.{}", seg(name, self.mask));
        if let Some(fault) = name_fault(name) {
            out.problems.add(&format!("animation-name {}", cut(name, MAX_QUOTE_CHARS)), &at, format!("the animation name {}", q(self.mask, name)), fault);
            return None;
        }
        let Some(o) = v.as_object() else {
            out.problems.add("animation-form", &at, format!("an animation is {}", kind_of(v)), "an object with duration and tracks");
            return None;
        };
        let mut ok = keys(o, &ANIMATION_KEYS, &at, out, self.mask);
        let duration = match o.get("duration").map(|d| (d, d.as_f64())) {
            Some((_, Some(d))) if (MIN_DURATION..=MAX_DURATION).contains(&d) => round3(d),
            Some((_, Some(d))) => {
                out.problems.add("duration-range", &at, format!("duration {}", fmt_num(d)), &format!("seconds, from {MIN_DURATION} to {MAX_DURATION}"));
                ok = false;
                1.0
            }
            Some((d, None)) => {
                out.problems.add("duration-form", &at, format!("duration is {}", kind_of(d)), &format!("seconds, from {MIN_DURATION} to {MAX_DURATION}"));
                ok = false;
                1.0
            }
            None => {
                out.problems.add("duration-none", &at, "an animation without a duration".to_string(), &format!("seconds, from {MIN_DURATION} to {MAX_DURATION}"));
                ok = false;
                1.0
            }
        };
        let looping = match o.get("loop") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(other) => {
                out.problems.add("loop-form", &at, format!("loop is {}", kind_of(other)), "true or false");
                ok = false;
                false
            }
        };
        let every = match o.get("every") {
            None | Some(Value::Null) => None,
            Some(e) => match pair(e) {
                Some([min, max]) if MIN_EVERY <= min && min <= max && max <= MAX_EVERY => Some([Num::new(min), Num::new(max)]),
                _ => {
                    out.problems.add("every-form", &at, "every is not a pair of seconds in range".to_string(), &format!("[min, max] with {MIN_EVERY} <= min <= max <= {MAX_EVERY}"));
                    ok = false;
                    None
                }
            },
        };
        if looping && every.is_some() {
            out.problems.add("loop-every", &at, "loop and every exclude each other".to_string(), "loop repeats without a pause, every starts again after a random pause, neither is a one-shot");
            ok = false;
        }
        let times = match o.get("times") {
            None | Some(Value::Null) => None,
            Some(t) => {
                let times = check_times(t, "times", &format!("{at}.times"), out);
                ok &= times.is_some();
                times
            }
        };
        let ease = match o.get("ease") {
            None | Some(Value::Null) => None,
            Some(e) => {
                let ease = check_eases(e, &format!("{at}.ease"), out, self.mask);
                ok &= ease.is_some();
                ease
            }
        };
        let mut tracks = Vec::new();
        match o.get("tracks") {
            Some(Value::Array(list)) if (1..=MAX_TRACKS).contains(&list.len()) => {
                let shared = Shared { duration, looping, times: times.as_deref(), ease: ease.as_ref() };
                for (i, track) in list.iter().enumerate() {
                    match self.track(track, &format!("{at}.tracks[{i}]"), &shared, rig, out) {
                        Some(track) => tracks.push(track),
                        None => ok = false,
                    }
                }
            }
            Some(Value::Array(list)) => {
                out.problems.add("tracks-count", &at, format!("{} tracks", list.len()), &format!("1 to {MAX_TRACKS} tracks in one animation"));
                ok = false;
            }
            Some(other) => {
                out.problems.add("tracks-form", &at, format!("tracks is {}", kind_of(other)), &format!("a list of 1 to {MAX_TRACKS} tracks"));
                ok = false;
            }
            None => {
                out.problems.add("tracks-none", &at, "an animation without tracks".to_string(), &format!("a list of 1 to {MAX_TRACKS} tracks"));
                ok = false;
            }
        }
        ok.then(|| Animation { duration: Num::new(duration), looping, every, times: times.map(nums), ease, tracks })
    }

    /// The target, the prop and the values of a track or a drive (`what` names which), with every
    /// problem of each. `None`: there is no prop, or no list of values, to read the rest against.
    fn keyed<'v>(&self, o: &'v Map<String, Value>, at: &str, what: &str, out: &mut Out) -> Option<Keyed<'v, 'a>> {
        let mut ok = true;
        let names: Vec<&'v str> = match o.get("target") {
            Some(Value::String(id)) => vec![id.as_str()],
            Some(Value::Array(list)) if (1..=MAX_TARGETS).contains(&list.len()) && list.iter().all(Value::is_string) => list.iter().filter_map(Value::as_str).collect(),
            Some(other) => {
                let found = match other {
                    Value::Array(list) => format!("a list of {}", list.len()),
                    other => kind_of(other).to_string(),
                };
                out.problems.add("target-form", at, format!("target is {found}"), &format!("an id, or a list of 1 to {MAX_TARGETS} ids"));
                ok = false;
                Vec::new()
            }
            None => {
                out.problems.add("target-none", at, format!("a {what} without a target"), "target names a g, a shape, a text or a use by its id");
                ok = false;
                Vec::new()
            }
        };
        let mut targets: Vec<(&'v str, &'a Entry<'a>)> = Vec::new();
        for (i, id) in names.iter().enumerate() {
            if names.iter().take(i).any(|earlier| earlier == id) {
                out.problems.add("target-twice", at, format!("target names {} twice", seg(id, self.mask)), "each part once");
                ok = false;
                continue;
            }
            match self.part(id, at, out) {
                Some(entry) => targets.push((*id, entry)),
                None => ok = false,
            }
        }

        let prop = match o.get("prop") {
            Some(Value::String(name)) => {
                let prop = Prop::ALL.into_iter().find(|p| p.name() == name);
                if prop.is_none() {
                    let accepted = if name == "points" { "points cannot be animated, draw a path and animate its d" } else { PROPS };
                    out.problems.add(&format!("prop {}", cut(name, MAX_QUOTE_CHARS)), at, format!("prop {}", q(self.mask, name)), accepted);
                }
                prop
            }
            Some(other) => {
                out.problems.add("prop-form", at, format!("prop is {}", kind_of(other)), PROPS);
                None
            }
            None => {
                out.problems.add("prop-none", at, format!("a {what} without a prop"), PROPS);
                None
            }
        };
        // Without a prop the values mean nothing.
        let prop = prop?;
        for (id, entry) in &targets {
            let tag = entry.node.tag;
            if !prop.fits(tag) {
                out.problems.add(&format!("fit {} {}", prop.name(), tag.name()), at, format!("{id} is a {} and has no {}", tag.name(), prop.name()), &format!("{} is a prop of {}", prop.name(), prop.where_it_fits()));
                ok = false;
            }
        }

        let list = match o.get("values") {
            Some(Value::Array(list)) if (2..=MAX_KEYFRAMES).contains(&list.len()) => list,
            Some(Value::Array(list)) => {
                out.problems.add("values-count", at, format!("{} keyframes", list.len()), &format!("2 to {MAX_KEYFRAMES} values"));
                return None;
            }
            Some(other) => {
                out.problems.add("values-form", at, format!("values is {}", kind_of(other)), &format!("a list of 2 to {MAX_KEYFRAMES} keyframes"));
                return None;
            }
            None => {
                out.problems.add("values-none", at, format!("a {what} without values"), &format!("a list of 2 to {MAX_KEYFRAMES} keyframes"));
                return None;
            }
        };
        let n = list.len();
        let mut values: Vec<Option<Key>> = Vec::with_capacity(n);
        // The structure of the first `d` keyframe: the others must share it.
        let mut shape: Option<Structure> = None;
        let mut nulls = false;
        for (i, value) in list.iter().enumerate() {
            let here = format!("{at}.values[{i}]");
            let name = prop.name();
            match value {
                Value::Null if prop.is_relative() => {
                    out.problems.add(&format!("null-relative {name}"), &here, format!("null in a {name} {what}"), "null is the artwork's own value; translate, rotate, skew and scale are relative to rest, so write 0, or 1 for a scale");
                    ok = false;
                }
                Value::Null => {
                    nulls = true;
                    values.push(None);
                }
                Value::Number(_) if prop.is_colour() || prop == Prop::D => {
                    out.problems.add(&format!("number-for {name}"), &here, format!("a number for {name}"), if prop == Prop::D { "path data as a string, or null" } else { "a colour as a string, or null" });
                    ok = false;
                }
                Value::Number(number) => match number.as_f64().filter(|f| f.is_finite()) {
                    Some(f) => match self.number_fits(prop, f) {
                        Ok(()) => values.push(Some(Key::Num(Num::new(f)))),
                        Err(range) => {
                            out.problems.add(&format!("range {name}"), &here, format!("{name} {} is out of range", fmt_num(f)), &range);
                            ok = false;
                        }
                    },
                    None => {
                        out.problems.add("number-size", &here, "a number beyond any size".to_string(), "a plain finite number");
                        ok = false;
                    }
                },
                Value::String(s) if prop.is_colour() => match colour(s) {
                    Some(c) => values.push(Some(Key::Str(hex(c)))),
                    None => {
                        out.problems.add(&format!("key-colour {}", cut(s, MAX_QUOTE_CHARS)), &here, format!("{} is not a plain colour", q(self.mask, s)), "#rrggbb, rgb(), hsl() or a CSS name, not none, currentColor or url()");
                        ok = false;
                    }
                },
                Value::String(s) if prop == Prop::D => match path_data(s, &self.view, MAX_D_KEYFRAME_CHARS, self.mask) {
                    Ok(path) => {
                        match &shape {
                            None => shape = Some(path.structure.clone()),
                            Some(first) if *first != path.structure => {
                                out.problems.add("d-structure", &here, format!("d values differ in structure: {}", first.against(&path.structure)), "same commands, same count of numbers, same arc flags");
                                ok = false;
                            }
                            Some(_) => {}
                        }
                        values.push(Some(Key::Str(path.text)));
                    }
                    Err(fault) => {
                        out.problems.fault(&format!("key-d {}", cut(&fault, MAX_QUOTE_CHARS)), &here, "a d keyframe: ", &fault);
                        ok = false;
                    }
                },
                Value::String(_) => {
                    out.problems.add(&format!("string-for {name}"), &here, format!("a string for {name}"), "a number, or null for the artwork's own value");
                    ok = false;
                }
                other => {
                    out.problems.add("key-form", &here, format!("{} as a keyframe", kind_of(other)), "a number, a string (a colour or path data) or null");
                    ok = false;
                }
            }
        }

        // null is the element's own value: it has to be there, and a d has to match the others.
        for (id, entry) in &targets {
            if nulls {
                let missing = match prop {
                    Prop::Fill | Prop::Stroke => matches!(entry.own(prop), Own::Unknown).then_some("no plain colour"),
                    Prop::StrokeWidth => matches!(entry.own(prop), Own::Unknown).then_some("no stroke-width"),
                    Prop::FillOpacity => matches!(entry.own(prop), Own::Unknown).then_some("no fill-opacity"),
                    Prop::StrokeOpacity => matches!(entry.own(prop), Own::Unknown).then_some("no stroke-opacity"),
                    Prop::StrokeDashoffset => matches!(entry.own(prop), Own::Unknown).then_some("no stroke-dashoffset"),
                    Prop::D => entry.str("d").is_none().then_some("no d"),
                    _ => None,
                };
                if let Some(missing) = missing {
                    out.problems.add(&format!("null-own {}", prop.name()), at, format!("null is the artwork's own {}, and {id} has {missing} of its own", prop.name()), "write the attribute on the element itself, or the value in the keyframe");
                    ok = false;
                }
            }
            if prop == Prop::D {
                if let (Some(own), Some(first)) = (&entry.structure, &shape) {
                    if own != first && nulls {
                        out.problems.add("d-own", at, format!("null is the d of {id}, {}, and the keyframes are {}", own.beside(first), first.beside(own)), "write them in the structure of that d");
                        ok = false;
                    } else if own != first {
                        out.warnings.add(&format!("d-differs {at} {id}"), at, format!("these d keyframes are {}, the d of {id} is {}", first.beside(own), own.beside(first)), "the shape jumps at its start and end");
                    }
                }
            }
        }

        Some(Keyed { names, targets, prop, values, n, ok })
    }

    /// One ease for all segments or one per segment, and none that jumps on paint. False when
    /// that is not so.
    fn segments(&self, ease: Option<&Eases>, n: usize, prop: Prop, at: &str, out: &mut Out) -> bool {
        let mut ok = true;
        if let Some(count) = ease.and_then(Eases::count).filter(|c| *c != n - 1) {
            out.problems.add("ease-count", at, format!("{count} eases for {} segments", n - 1), "one ease for all segments, or one per segment, which is one fewer than values");
            ok = false;
        }
        if prop.is_paint() && ease.is_some_and(Eases::steps) {
            out.problems.add("ease-step", at, format!("ease step on {}", prop.name()), "fill, stroke and opacity change smoothly so that nothing flashes, use linear, in, out or inOut");
            ok = false;
        }
        if prop.is_paint() && ease.is_some_and(Eases::swings) {
            out.problems.add("ease-swing", at, format!("an ease that swings on {}", prop.name()), "no outElastic or outBounce on fill, stroke or an opacity, and a bezier with y1 and y2 from 0 to 1");
            ok = false;
        }
        ok
    }

    /// A rotate, skew or scale of a part without a pivot warns, a translate that takes a part out
    /// of the viewBox warns.
    fn motion_warnings(&self, targets: &[(&str, &Entry<'_>)], prop: Prop, values: &[Option<Key>], at: &str, rig: &Rig<'_>, out: &mut Out) {
        if prop.pivots() {
            for (id, entry) in targets {
                self.pivot_warning(id, entry, at, rig, out);
            }
        }
        if prop.is_translate() {
            let (low, high) = values.iter().flatten().fold((0.0_f64, 0.0_f64), |(low, high), key| match key {
                Key::Num(n) => (low.min(n.get()), high.max(n.get())),
                Key::Str(_) => (low, high),
            });
            for (id, entry) in targets {
                let rest = entry.root_box();
                // A part that is outside at rest is reported once, as that.
                if !self.view.holds(&rest) {
                    continue;
                }
                for t in [low, high] {
                    let (dx, dy) = if prop == Prop::TranslateX { vector(&entry.to_root, t, 0.0) } else { vector(&entry.to_root, 0.0, t) };
                    let moved = rest.shifted(dx, dy);
                    if !self.view.holds(&moved) {
                        let what = format!("{} {} moves {id} out of the viewBox, to x {} to {}, y {} to {}", prop.name(), fmt_num(t), fmt_num(moved.x0), fmt_num(moved.x1), fmt_num(moved.y0), fmt_num(moved.y1));
                        out.warnings.add(&format!("leaves {at} {id}"), at, what, "what leaves the viewBox is cut off");
                        break;
                    }
                }
            }
        }
    }

    fn track(&self, v: &Value, at: &str, shared: &Shared<'_>, rig: &Rig<'_>, out: &mut Out) -> Option<Track> {
        let Some(o) = v.as_object() else {
            out.problems.add("track-form", at, format!("a track is {}", kind_of(v)), "an object with target, prop and values");
            return None;
        };
        let known = keys(o, &TRACK_KEYS, at, out, self.mask);
        let Keyed { names, targets, prop, values, n, ok: read } = self.keyed(o, at, "track", out)?;
        let mut ok = known && read;

        // Where a loop starts again its last keyframe meets its first with no time between them:
        // a paint that ends on another value jumps there, once a cycle, which is the flash the
        // spacing below keeps out of everything else.
        if shared.looping && prop.is_paint() && values.len() == n {
            if let (Some(first), Some(last)) = (values.first(), values.last()) {
                // null is each target's own value. One that cannot be told is reported above.
                let closes = ends_as_it_starts(first, last, |key| targets.iter().all(|(_, entry)| entry.own(prop).is(key)));
                if !closes {
                    out.problems.add(&format!("loop-ends {}", prop.name()), at, format!("the loop's {} ends on another value than it starts with", prop.name()), "end fill, stroke and opacity as they begin, so nothing flashes");
                    ok = false;
                }
            }
        }

        let times = match o.get("times") {
            None | Some(Value::Null) => None,
            Some(t) => {
                let times = check_times(t, "times", &format!("{at}.times"), out);
                ok &= times.is_some();
                times
            }
        };
        let effective: Vec<f64> = match (&times, shared.times) {
            (Some(own), _) if own.len() == n => own.clone(),
            (Some(own), _) => {
                out.problems.add("times-count", at, format!("{} times for {n} values", own.len()), "one time per keyframe");
                ok = false;
                even(n)
            }
            (None, Some(default)) if default.len() == n => default.to_vec(),
            (None, Some(default)) => {
                out.problems.add("times-default", at, format!("{n} values against the animation's {} times", default.len()), "give this track times of its own, or as many values");
                ok = false;
                even(n)
            }
            (None, None) => even(n),
        };
        let ease = match o.get("ease") {
            None | Some(Value::Null) => None,
            Some(e) => {
                let ease = check_eases(e, &format!("{at}.ease"), out, self.mask);
                ok &= ease.is_some();
                ease
            }
        };
        ok &= self.segments(ease.as_ref().or(shared.ease), n, prop, at, out);
        let paint = prop.is_paint();
        let least = if paint { MIN_PAINT_STEP } else { MIN_STEP };
        let close = effective.windows(2).enumerate().map(|(i, w)| (i, (w[1] - w[0]) * shared.duration)).find(|(_, gap)| *gap < least - 1e-9);
        if let Some((i, gap)) = close {
            let what = format!("keyframes {i} and {} of {} are {} s apart", i + 1, prop.name(), fmt_num(gap));
            if paint {
                out.problems.add("step-paint", at, what, &format!("at least {MIN_PAINT_STEP} s between keyframes of fill, stroke and opacity, so nothing flashes"));
            } else {
                out.problems.add("step", at, what, &format!("at least {MIN_STEP} s between keyframes"));
            }
            ok = false;
        }

        let phase = match o.get("phase") {
            None | Some(Value::Null) => None,
            Some(p) => match p.as_f64() {
                Some(p) if (0.0..=1.0).contains(&p) && shared.looping => Some(Num::new(p)),
                Some(p) if (0.0..=1.0).contains(&p) => {
                    out.warnings.add("phase-unused", at, "phase is for loops and is left out here".to_string(), "a one-shot starts a track later with delay");
                    None
                }
                _ => {
                    out.problems.add("phase-form", at, "phase is not a number from 0 to 1".to_string(), "the share of the cycle at which a loop's track starts");
                    ok = false;
                    None
                }
            },
        };
        let delay = match o.get("delay") {
            None | Some(Value::Null) => None,
            Some(d) => match d.as_f64() {
                Some(d) if (0.0..=shared.duration).contains(&d) && !shared.looping => Some(Num::new(d)),
                Some(d) if (0.0..=shared.duration).contains(&d) => {
                    out.warnings.add("delay-unused", at, "delay is for one-shots and is left out here".to_string(), "a loop shifts a track with phase");
                    None
                }
                _ => {
                    out.problems.add("delay-form", at, "delay is not a number of seconds within the duration".to_string(), "0 up to the animation's duration");
                    ok = false;
                    None
                }
            },
        };
        if !ok {
            return None;
        }

        // Accepted. What follows only warns.
        self.motion_warnings(&targets, prop, &values, at, rig, out);
        if !shared.looping && !prop.is_relative() {
            for (id, entry) in &targets {
                let own = entry.own(prop);
                if ![values.first(), values.last()].into_iter().flatten().all(|key| own.is(key)) {
                    let what = format!("{} of {id} does not start and end at its own value{}", prop.name(), own.shown());
                    out.warnings.add(&format!("jump {at} {id}"), at, what, "a one-shot jumps back at its end; start and end with null");
                }
            }
        }
        let target = match names.as_slice() {
            [one] => Targets::One(one.to_string()),
            many => Targets::Many(many.iter().map(|id| id.to_string()).collect()),
        };
        Some(Track { target, prop, values, times: times.map(nums), ease, phase, delay })
    }

    fn drive(&self, v: &Value, at: &str, rig: &Rig<'_>, out: &mut Out) -> Option<Drive> {
        let Some(o) = v.as_object() else {
            out.problems.add("drive-form", at, format!("a drive is {}", kind_of(v)), "an object with input, target, prop and values");
            return None;
        };
        let known = keys(o, &DRIVE_KEYS, at, out, self.mask);
        let input = match o.get("input") {
            Some(Value::String(name)) => {
                let input = Input::ALL.into_iter().find(|i| i.name() == name);
                if input.is_none() {
                    out.problems.add(&format!("input {}", cut(name, MAX_QUOTE_CHARS)), at, format!("input {}", q(self.mask, name)), INPUTS);
                }
                input
            }
            Some(other) => {
                out.problems.add("input-form", at, format!("input is {}", kind_of(other)), INPUTS);
                None
            }
            None => {
                out.problems.add("input-none", at, "a drive without an input".to_string(), INPUTS);
                None
            }
        };
        let Keyed { names, targets, prop, values, n, ok: read } = self.keyed(o, at, "drive", out)?;
        let mut ok = known && read;
        // Paint follows only what the browser moves slowly: a full swing takes a quarter second.
        if let Some(input) = input.filter(|input| prop.is_paint() && !input.slow()) {
            out.problems.add(&format!("drive-paint {}", input.name()), at, format!("{} follows {}", prop.name(), input.name()), "fill, stroke and the opacities follow only pace, turn and held, which change slowly, so nothing flashes");
            ok = false;
        }
        let places = match o.get("at") {
            None | Some(Value::Null) => None,
            Some(t) => {
                let places = check_times(t, "at", &format!("{at}.at"), out);
                ok &= places.is_some();
                places
            }
        };
        if let Some(places) = places.as_ref().filter(|places| places.len() != n) {
            out.problems.add("at-count", at, format!("{} places for {n} values", places.len()), "one place per value");
            ok = false;
        }
        // Even a slow input crosses its range in a quarter second (half a second for turn, which
        // runs from -1 to 1): paint values along it keep the distance paint keyframes keep in time.
        if let Some(input) = input.filter(|input| prop.is_paint() && input.slow() && n >= 2) {
            let least = PAINT_PLACE_STEP / input.span();
            let gaps_ok = match places.as_ref().filter(|places| places.len() == n) {
                Some(places) => places.windows(2).all(|pair| pair[1] - pair[0] >= least - MIN_TIME_STEP),
                None => 1.0 / (n - 1) as f64 >= least - MIN_TIME_STEP,
            };
            if !gaps_ok {
                out.problems.add(&format!("drive-paint-places {}", input.name()), at, format!("{n} values of {} along {} stand too close", prop.name(), input.name()), "2 values along pace or held, up to 3 along turn, at least 0.34 apart in at");
                ok = false;
            }
        }
        let ease = match o.get("ease") {
            None | Some(Value::Null) => None,
            Some(e) => {
                let ease = check_eases(e, &format!("{at}.ease"), out, self.mask);
                ok &= ease.is_some();
                ease
            }
        };
        ok &= self.segments(ease.as_ref(), n, prop, at, out);
        let input = input?;
        if !ok {
            return None;
        }
        self.motion_warnings(&targets, prop, &values, at, rig, out);
        rest_warning(input, &targets, prop, &values, places.as_deref(), ease.as_ref(), at, &self.view, out);
        let target = match names.as_slice() {
            [one] => Targets::One(one.to_string()),
            many => Targets::Many(many.iter().map(|id| id.to_string()).collect()),
        };
        Some(Drive { input, target, prop, values, at: places.map(nums), ease })
    }

    /// One entry of `parents`: `child` moves with the part `v` names, which neither holds it nor
    /// stands inside it.
    fn parent(&self, child: &str, v: &Value, out: &mut Out) -> Option<String> {
        let at = format!("parents.{}", seg(child, self.mask));
        let entry = self.part(child, &at, out);
        let Some(parent) = v.as_str() else {
            out.problems.add("parent-form", &at, format!("a parent is {}", kind_of(v)), "the id of the part it moves with");
            return None;
        };
        let other = self.part(parent, &at, out);
        let (entry, other) = (entry?, other?);
        if child == parent {
            out.problems.add("parent-self", &at, format!("{child} moves with itself"), "a part moves with another part");
            return None;
        }
        if entry.above.contains(&parent) {
            out.problems.add(&format!("parent-inside {child}"), &at, format!("{child} is inside {parent} and moves with it already"), "parents are for parts apart in the drawing order");
            return None;
        }
        if other.above.contains(&child) {
            out.problems.add(&format!("parent-holds {child}"), &at, format!("{parent} is inside {child}"), "a part cannot move with what it holds");
            return None;
        }
        Some(parent.to_string())
    }

    fn state(&self, key: &str, v: &Value, animations: &BTreeMap<String, Animation>, rig: &Rig<'_>, out: &mut Out) -> Option<(LookState, Vec<String>)> {
        let at = format!("states.{}", seg(key, self.mask));
        let Some(state) = LookState::ALL.into_iter().find(|s| s.name() == key) else {
            out.problems.add(&format!("state {}", cut(key, MAX_QUOTE_CHARS)), &at, format!("{} is not a state", q(self.mask, key)), "states are idle, working, speaking, listening, waiting, attention, error, done, sleeping");
            return None;
        };
        let names: Option<Vec<&str>> = v.as_array().filter(|l| l.len() <= MAX_STATE_NAMES).and_then(|l| l.iter().map(Value::as_str).collect());
        let Some(names) = names else {
            let found = match v {
                Value::Array(list) if list.len() > MAX_STATE_NAMES => format!("a list of {}", list.len()),
                Value::Array(_) => "a list with entries that are not names".to_string(),
                other => kind_of(other).to_string(),
            };
            out.problems.add("state-form", &at, format!("a state's list is {found}"), &format!("up to {MAX_STATE_NAMES} animation names, [] for stillness, null for the default"));
            return None;
        };
        let mut ok = true;
        for (i, name) in names.iter().enumerate() {
            let here = format!("{at}[{i}]");
            if names.iter().take(i).any(|earlier| earlier == name) {
                out.problems.add("state-twice", &here, format!("{} is listed twice", seg(name, self.mask)), "each animation once in a state");
                ok = false;
            } else if animations.contains_key(*name) {
            } else if BUILTIN.contains(name) {
                if let Some(needs) = builtin_missing(name, rig.roles, rig.gaze) {
                    let remedy = super::builtin::remedy(name, |role| rig.roles.keys().any(|r| r.name() == role), rig.gaze.map(Num::get));
                    out.warnings.add(&format!("builtin {name}"), &here, format!("{name} is built in and needs {needs}"), remedy);
                }
            } else {
                let own: Vec<&String> = animations.keys().collect();
                let accepted = if own.is_empty() { format!("built in are {}", BUILTIN.join(", ")) } else { format!("yours are {}; built in are {}", listed(&own), BUILTIN.join(", ")) };
                out.problems.add(&format!("state-name {}", cut(name, MAX_QUOTE_CHARS)), &here, format!("no animation {}", q(self.mask, name)), &accepted);
                ok = false;
            }
        }
        ok.then(|| (state, names.into_iter().map(str::to_string).collect()))
    }

    fn mouth(&self, v: &Value, at: &str, rig: &Rig<'_>, out: &mut Out) -> Option<MouthDrive> {
        const FORM: &str = "{target, open, closed?} morphs a path, {target, prop, open, closed?} moves one number";
        let Some(o) = v.as_object() else {
            out.problems.add("mouth-form", at, format!("a mouth drive is {}", kind_of(v)), FORM);
            return None;
        };
        let mut ok = keys(o, &MOUTH_KEYS, at, out, self.mask);
        let Some(id) = o.get("target").and_then(Value::as_str) else {
            out.problems.add("mouth-target", at, "a mouth drive without a target".to_string(), FORM);
            return None;
        };
        let entry = self.part(id, at, out)?;
        let tag = entry.node.tag;
        let Some(prop) = o.get("prop").filter(|p| !p.is_null()) else {
            // A path morph.
            if tag != Tag::Path {
                out.problems.add("mouth-morph", at, format!("{id} is a {}, and a mouth without prop morphs a path", tag.name()), "give it a prop, one of scaleY, scaleX, scale, translateY, ry, r or height");
                return None;
            }
            let mut read = |key: &str| match o.get(key) {
                Some(Value::String(d)) => match path_data(d, &self.view, MAX_D_KEYFRAME_CHARS, self.mask) {
                    Ok(path) => Some(path),
                    Err(fault) => {
                        out.problems.fault(&format!("mouth-d {}", cut(&fault, MAX_QUOTE_CHARS)), at, &format!("mouth {key}: "), &fault);
                        ok = false;
                        None
                    }
                },
                None | Some(Value::Null) => None,
                Some(other) => {
                    out.problems.add("mouth-d-form", at, format!("mouth {key} is {}", kind_of(other)), "path data as a string; with prop the mouth takes numbers");
                    ok = false;
                    None
                }
            };
            let (open, closed) = (read("open"), read("closed"));
            let Some(open) = open else {
                if ok {
                    out.problems.add("mouth-open", at, "a mouth drive without open".to_string(), FORM);
                }
                return None;
            };
            match (&closed, &entry.structure) {
                (Some(closed), _) if closed.structure != open.structure => {
                    out.problems.add("mouth-structure", at, format!("mouth open and closed differ in structure: {}", open.structure.against(&closed.structure)), "same commands, same count of numbers, same arc flags");
                    ok = false;
                }
                (Some(closed), Some(own)) if closed.structure != *own => {
                    out.warnings.add(&format!("mouth-differs {id}"), at, format!("the mouth's closed is {}, the d of {id} is {}", closed.structure.beside(own), own.beside(&closed.structure)), "the shape jumps when a line starts and ends");
                }
                (None, Some(own)) if open.structure != *own => {
                    out.problems.add("mouth-own", at, format!("mouth open is {}, the d of {id} is {}", open.structure.beside(own), own.beside(&open.structure)), "write open in the structure of that d, or give closed too");
                    ok = false;
                }
                (None, None) => {
                    out.problems.add("mouth-no-d", at, format!("{id} has no d to close to"), "give closed, or draw the mouth's d");
                    ok = false;
                }
                _ => {}
            }
            return ok.then(|| MouthDrive::Path { target: id.to_string(), open: open.text, closed: closed.map(|c| c.text) });
        };
        let Some(prop) = prop.as_str().and_then(|name| MouthProp::ALL.into_iter().find(|p| p.prop().name() == name)) else {
            out.problems.add("mouth-prop", at, "the mouth's prop is not one it can drive".to_string(), "scaleY, scaleX, scale, translateY, ry, r or height");
            return None;
        };
        let track = prop.prop();
        if !track.fits(tag) {
            out.problems.add(&format!("fit {} {}", track.name(), tag.name()), at, format!("{id} is a {} and has no {}", tag.name(), track.name()), &format!("{} is a prop of {}", track.name(), track.where_it_fits()));
            ok = false;
        }
        let mut number = |key: &str| match o.get(key) {
            Some(Value::Number(n)) => match n.as_f64().filter(|f| f.is_finite()).map(|f| (f, self.number_fits(track, f))) {
                Some((f, Ok(()))) => Some(Num::new(f)),
                Some((f, Err(range))) => {
                    out.problems.add(&format!("range {}", track.name()), at, format!("mouth {key} {} is out of range", fmt_num(f)), &range);
                    ok = false;
                    None
                }
                None => {
                    ok = false;
                    None
                }
            },
            None | Some(Value::Null) => None,
            Some(other) => {
                out.problems.add("mouth-number", at, format!("mouth {key} is {}", kind_of(other)), "a number; without prop the mouth morphs a path and takes path data");
                ok = false;
                None
            }
        };
        let (open, closed) = (number("open"), number("closed"));
        let Some(open) = open else {
            if ok {
                out.problems.add("mouth-open", at, "a mouth drive without open".to_string(), FORM);
            }
            return None;
        };
        if !ok {
            return None;
        }
        if track.is_scale() {
            self.pivot_warning(id, entry, at, rig, out);
        }
        Some(MouthDrive::Prop { target: id.to_string(), prop, open, closed })
    }

    fn gaze(&self, v: &Value, out: &mut Out) -> Option<Num> {
        let most = round3(MAX_GAZE_SHARE * self.view.w);
        match v.as_f64() {
            Some(g) if (0.0..=most).contains(&g) => Some(Num::new(g)),
            _ => {
                out.problems.add("gaze", "gaze", "gaze is not a number in range".to_string(), &format!("how far the pupils travel, 0 to {} here (5% of the width), 0 for none, null for the default", fmt_num(most)));
                None
            }
        }
    }
}

/// `[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8}){0,2}`.
fn valid_lang(s: &str) -> bool {
    let mut pieces = s.split('-');
    let first = pieces.next().is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()));
    let rest: Vec<&str> = pieces.collect();
    first && rest.len() <= 2 && rest.iter().all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The voice of a design call. Keys this version does not know are dropped with a warning.
fn check_voice(v: &Value, cx: &mut Ctx<'_>) -> Option<Voice> {
    let Some(o) = v.as_object() else {
        cx.out.problems.add("voice-form", "voice", format!("voice is {}", kind_of(v)), "an object with lang, prefer, pitch and rate");
        return None;
    };
    let mask = cx.mask;
    let unknown: Vec<String> = o.keys().filter(|k| !VOICE_KEYS.contains(&k.as_str())).take(LISTED).map(|k| seg(k, mask)).collect();
    if !unknown.is_empty() {
        cx.out.warnings.add("voice-keys", "", format!("voice: {} left out", unknown.join(", ")), "a voice takes lang, prefer, pitch and rate");
    }
    let mut ok = true;
    let mut voice = Voice::default();
    match o.get("lang") {
        None | Some(Value::Null) => {}
        Some(Value::String(lang)) if valid_lang(lang) => voice.lang = Some(lang.clone()),
        Some(other) => {
            let found = other.as_str().map_or_else(|| kind_of(other).to_string(), |lang| q(mask, lang));
            cx.out.problems.add("voice-lang", "voice.lang", format!("lang is {found}"), "a language tag such as en-GB, de or pt-BR");
            ok = false;
        }
    }
    match o.get("prefer") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) if list.len() <= MAX_VOICE_PREFER && list.iter().all(Value::is_string) => {
            for (i, fragment) in list.iter().filter_map(Value::as_str).enumerate() {
                match text::plain(fragment, MAX_VOICE_PREFER_CHARS, cx.mask) {
                    Ok(fragment) if !fragment.is_empty() => voice.prefer.push(fragment),
                    Ok(_) => {
                        cx.out.problems.add("voice-prefer-empty", &format!("voice.prefer[{i}]"), "an empty fragment".to_string(), "a piece of a browser voice's name, such as Female or Natural");
                        ok = false;
                    }
                    Err(refusal) => {
                        cx.out.problems.add("voice-prefer-long", &format!("voice.prefer[{i}]"), format!("a fragment has {refusal}"), "a piece of a browser voice's name");
                        ok = false;
                    }
                }
            }
        }
        Some(other) => {
            cx.out.problems.add("voice-prefer", "voice.prefer", format!("prefer is {}", match other {
                Value::Array(list) if list.len() > MAX_VOICE_PREFER => format!("a list of {}", list.len()),
                Value::Array(_) => "a list with entries that are not strings".to_string(),
                other => kind_of(other).to_string(),
            }), &format!("up to {MAX_VOICE_PREFER} pieces of browser voice names, tried in order"));
            ok = false;
        }
    }
    for (key, (low, high), slot) in [("pitch", PITCH, &mut voice.pitch), ("rate", RATE, &mut voice.rate)] {
        match o.get(key) {
            None | Some(Value::Null) => {}
            Some(n) => match n.as_f64() {
                Some(f) if (low..=high).contains(&f) => *slot = Some(Num::new(f)),
                _ => {
                    cx.out.problems.add(&format!("voice-{key}"), &format!("voice.{key}"), format!("{key} is not a number in range"), &format!("{low} to {high}"));
                    ok = false;
                }
            },
        }
    }
    ok.then_some(voice)
}

// ---------------------------------------------------------------- one call

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// A design call of a session: its arguments have a size of their own.
    Call,
    /// A stored design read back. The artwork is the text this file writes for a look, which an
    /// accepted look keeps within the limit of a call's artwork.
    Stored,
}

/// What moves a look.
#[derive(Default)]
struct Moving {
    pivots: BTreeMap<String, [Num; 2]>,
    roles: BTreeMap<Role, String>,
    animations: BTreeMap<String, Animation>,
    states: BTreeMap<LookState, Vec<String>>,
    mouth: Vec<MouthDrive>,
    gaze: Option<Num>,
    parents: BTreeMap<String, String>,
    drives: Vec<Drive>,
}

impl Moving {
    /// The look of this artwork that these move.
    fn look(self, view_box: [Num; 4], nodes: Vec<LookNode>) -> Look {
        Look { view_box, nodes, pivots: self.pivots, roles: self.roles, animations: self.animations, states: self.states, mouth: self.mouth, gaze: self.gaze, parents: self.parents, drives: self.drives }
    }
}

/// What became of the earlier look's moving parts under new artwork, as phrases for the answer.
#[derive(Default)]
struct Carried {
    kept: Vec<String>,
    cleared: Vec<String>,
}

/// One of the keyed fields of a call (`pivots`, `roles`, `animations`, `states`).
struct Asked<'r> {
    /// `null`: everything of this kind goes.
    clear: bool,
    entries: Option<&'r Map<String, Value>>,
}

impl<'r> Asked<'r> {
    fn of(req: &'r Map<String, Value>, key: &str, form: &str, out: &mut Out) -> Asked<'r> {
        match req.get(key) {
            None => Asked { clear: false, entries: None },
            Some(Value::Null) => Asked { clear: true, entries: None },
            Some(Value::Object(entries)) => Asked { clear: false, entries: Some(entries) },
            Some(other) => {
                out.problems.add(&format!("field {key}"), key, format!("{key} is {}", kind_of(other)), form);
                Asked { clear: false, entries: None }
            }
        }
    }

    /// Whether the call names this entry (to set it or, with `null`, to remove it).
    fn names(&self, key: &str) -> bool {
        self.entries.is_some_and(|entries| entries.contains_key(key))
    }

    /// The entries the call sets.
    fn given(&self) -> impl Iterator<Item = (&'r String, &'r Value)> {
        self.entries.into_iter().flatten().filter(|(_, v)| !v.is_null())
    }
}

/// "animations wave, nod" into `into`, when there are names.
fn phrase(into: &mut Vec<String>, label: &str, names: &[String]) {
    if !names.is_empty() {
        into.push(format!("{label} {}", listed(names)));
    }
}

/// A circle among the parts that move one another, or `None`: the parts of the first one found,
/// in order. A part moves with its parent in `parents` and with every part it is drawn inside,
/// so a circle may run through both: with `b` drawn inside `a`, `{a: c, c: b}` is one, although
/// no chain of `parents` comes back to where it starts. Walked with a stack of its own.
fn circle<'a>(parents: &'a BTreeMap<String, String>, ix: &'a Index<'a>) -> Option<Vec<&'a str>> {
    let next = |id: &'a str| -> Vec<&'a str> { parents.get(id).map(String::as_str).into_iter().chain(ix.by_id.get(id).into_iter().flat_map(|entry| entry.above.iter().copied())).collect() };
    // On the path being walked, or done with.
    let mut seen: HashMap<&'a str, bool> = HashMap::new();
    for start in parents.keys() {
        if seen.contains_key(start.as_str()) {
            continue;
        }
        seen.insert(start, true);
        let mut path: Vec<(&'a str, Vec<&'a str>, usize)> = vec![(start, next(start), 0)];
        loop {
            let Some(top) = path.last_mut() else { break };
            let step = top.1.get(top.2).copied();
            top.2 += 1;
            match step {
                None => {
                    if let Some((done, _, _)) = path.pop() {
                        seen.insert(done, false);
                    }
                }
                Some(id) => match seen.get(id) {
                    Some(true) => {
                        let from = path.iter().position(|(on, _, _)| *on == id).unwrap_or(0);
                        return Some(path.get(from..).unwrap_or_default().iter().map(|(on, _, _)| *on).collect());
                    }
                    Some(false) => {}
                    None => {
                        seen.insert(id, true);
                        path.push((id, next(id), 0));
                    }
                },
            }
        }
    }
    None
}

/// The first part whose chain of `parents` has more than [`MAX_CHAIN`] links, or `None`. Asked
/// once no circle is left, so every chain ends.
fn too_long(parents: &BTreeMap<String, String>) -> Option<&str> {
    parents.keys().map(String::as_str).find(|child| {
        let (mut at, mut links) = (*child, 0);
        while let Some(next) = parents.get(at) {
            links += 1;
            if links > MAX_CHAIN {
                return true;
            }
            at = next;
        }
        false
    })
}

/// What `parents` holds as a whole: no circle, no chain longer than [`MAX_CHAIN`]. Every id in it
/// is one of a part, checked before.
fn chains(parents: &BTreeMap<String, String>, ix: &Index<'_>, out: &mut Out) {
    if let Some(parts) = circle(parents, ix) {
        let at = parts.iter().find(|id| parents.contains_key(**id)).map_or_else(|| "parents".to_string(), |id| format!("parents.{id}"));
        out.problems.add("parents-circle", &at, format!("the parents of {} go round in a circle", listed(&parts)), "a part moves with no part that moves with it, through parents or what it is drawn inside");
    } else if let Some(child) = too_long(parents) {
        out.problems.add("parents-chain", &format!("parents.{child}"), format!("the chain of parents above {child} is longer than {MAX_CHAIN}"), &format!("at most {MAX_CHAIN} parts above one another"));
    }
}

/// Puts together what moves the artwork `nodes`: what is carried over from `base` (the current
/// look in a merge, the earlier look under `keep`), then what the call gives.
///
/// `keeping`: the artwork is new, so a carried entry that no longer fits it is cleared and
/// reported, not refused. Otherwise the artwork is the one the entries were accepted for, and a
/// carried entry can only fail through what this call removes; that is a problem.
fn assemble(view: View, nodes: &[LookNode], base: Option<&Look>, keeping: bool, req: &Map<String, Value>, cx: &mut Ctx<'_>) -> (Moving, Carried) {
    let ix = Index::of(nodes);
    let check = Check { ix: &ix, view, mask: cx.mask };
    let mut moving = Moving::default();
    let mut carried = Carried::default();
    let (mut kept, mut cleared): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());

    // Pivots.
    let asked = Asked::of(req, "pivots", "an object of ids with [x, y] each, null to remove one", &mut cx.out);
    for (id, p) in base.filter(|_| !asked.clear).into_iter().flat_map(|b| &b.pivots).filter(|(id, _)| !asked.names(id)) {
        let mut probe = Out::default();
        match check.pivot(id, &json!([p[0], p[1]]), if keeping { &mut probe } else { &mut cx.out }) {
            Some(p) => {
                moving.pivots.insert(id.clone(), p);
                kept.push(id.clone());
            }
            None => cleared.push(id.clone()),
        }
    }
    for (id, v) in asked.given() {
        if let Some(p) = check.pivot(id, v, &mut cx.out) {
            moving.pivots.insert(id.clone(), p);
        }
    }
    phrase(&mut carried.kept, "pivots", &kept);
    phrase(&mut carried.cleared, "pivots", &cleared);
    (kept, cleared) = (Vec::new(), Vec::new());

    // Parents, merged like pivots.
    let asked = Asked::of(req, "parents", "an object of part ids with the id of the part each moves with, null to remove one", &mut cx.out);
    for (child, parent) in base.filter(|_| !asked.clear).into_iter().flat_map(|b| &b.parents).filter(|(child, _)| !asked.names(child)) {
        let mut probe = Out::default();
        match check.parent(child, &json!(parent), if keeping { &mut probe } else { &mut cx.out }) {
            Some(parent) => {
                moving.parents.insert(child.clone(), parent);
                // Under new artwork an entry that fits alone may close a circle with the others.
                if keeping && (circle(&moving.parents, &ix).is_some() || too_long(&moving.parents).is_some()) {
                    moving.parents.remove(child);
                    cleared.push(child.clone());
                } else {
                    kept.push(child.clone());
                }
            }
            None => cleared.push(child.clone()),
        }
    }
    let count = moving.parents.len() + asked.given().filter(|(child, _)| !moving.parents.contains_key(*child)).count();
    if count > MAX_PARENTS {
        cx.out.problems.add("parents-count", "parents", format!("{count} parents"), &format!("at most {MAX_PARENTS} in one look; null removes one"));
    } else {
        for (child, v) in asked.given() {
            if let Some(parent) = check.parent(child, v, &mut cx.out) {
                moving.parents.insert(child.clone(), parent);
            }
        }
        chains(&moving.parents, &ix, &mut cx.out);
    }
    phrase(&mut carried.kept, "parents", &kept);
    phrase(&mut carried.cleared, "parents", &cleared);
    (kept, cleared) = (Vec::new(), Vec::new());

    // Roles.
    let asked = Asked::of(req, "roles", "an object of roles with the id of a part each, null to remove one", &mut cx.out);
    for (role, id) in base.filter(|_| !asked.clear).into_iter().flat_map(|b| &b.roles).filter(|(role, _)| !asked.names(role.name())) {
        let mut probe = Out::default();
        match check.role(role.name(), &json!(id), if keeping { &mut probe } else { &mut cx.out }) {
            Some((role, id)) => {
                moving.roles.insert(role, id);
                kept.push(role.name().to_string());
            }
            None => cleared.push(role.name().to_string()),
        }
    }
    for (key, v) in asked.given() {
        if let Some((role, id)) = check.role(key, v, &mut cx.out) {
            moving.roles.insert(role, id);
        }
    }
    phrase(&mut carried.kept, "roles", &kept);
    phrase(&mut carried.cleared, "roles", &cleared);
    (kept, cleared) = (Vec::new(), Vec::new());

    // Animations: against the pivots and roles as they will be. The gaze is checked below; here
    // it is only looked at, for the states' warnings.
    let gaze = match req.get("gaze") {
        None => base.and_then(|b| b.gaze).and_then(|g| check.gaze(&json!(g), &mut Out::default())),
        Some(v) if v.is_null() => None,
        Some(v) => check.gaze(v, &mut Out::default()),
    };
    let rig = Rig { pivots: &moving.pivots, roles: &moving.roles, gaze };
    let asked = Asked::of(req, "animations", "an object of names with an animation each, null to remove one", &mut cx.out);
    let mut count = asked.given().count();
    for (name, animation) in base.filter(|_| !asked.clear).into_iter().flat_map(|b| &b.animations).filter(|(name, _)| !asked.names(name)) {
        let Ok(v) = serde_json::to_value(animation) else { continue };
        // Under new artwork a carried animation is first tried on the side: only one that still
        // fits is checked for real, so that its warnings count and its problems do not.
        if keeping && check.animation(name, &v, &rig, &mut Out::default()).is_none() {
            cleared.push(name.clone());
            continue;
        }
        count += 1;
        if let Some(animation) = check.animation(name, &v, &rig, &mut cx.out) {
            moving.animations.insert(name.clone(), animation);
            kept.push(name.clone());
        }
    }
    if count > MAX_ANIMATIONS {
        cx.out.problems.add("animations-count", "animations", format!("{count} animations"), &format!("at most {MAX_ANIMATIONS} in one look; null removes one"));
    }
    // Past the limit the rest is not read: the call is refused as it is.
    for (name, v) in asked.given().take(MAX_ANIMATIONS) {
        if let Some(animation) = check.animation(name, v, &rig, &mut cx.out) {
            moving.animations.insert(name.clone(), animation);
        }
    }
    phrase(&mut carried.kept, "animations", &kept);
    phrase(&mut carried.cleared, "animations", &cleared);
    (kept, cleared) = (Vec::new(), Vec::new());

    // States: names resolve to the look's animations first, then to the built-in set.
    let asked = Asked::of(req, "states", "an object of states with a list of animation names each, null for the default", &mut cx.out);
    for (state, names) in base.filter(|_| !asked.clear).into_iter().flat_map(|b| &b.states).filter(|(state, _)| !asked.names(state.name())) {
        let mut list = names.clone();
        if keeping {
            // Names that are gone with the old artwork leave the list; a list left empty goes.
            list.retain(|name| moving.animations.contains_key(name) || BUILTIN.contains(&name.as_str()));
            if list.len() < names.len() {
                let gone: Vec<&String> = names.iter().filter(|name| !list.contains(*name)).collect();
                cleared.push(format!("{} ({})", state.name(), listed(&gone)));
            }
            if list.is_empty() && !names.is_empty() {
                continue;
            }
        }
        if let Some((state, list)) = check.state(state.name(), &json!(list), &moving.animations, &rig, &mut cx.out) {
            moving.states.insert(state, list);
            kept.push(state.name().to_string());
        }
    }
    for (key, v) in asked.given() {
        if let Some((state, list)) = check.state(key, v, &moving.animations, &rig, &mut cx.out) {
            moving.states.insert(state, list);
        }
    }
    phrase(&mut carried.kept, "states", &kept);
    phrase(&mut carried.cleared, "states", &cleared);

    // The mouth replaces.
    match req.get("mouth") {
        None => {
            let (mut stays, mut goes) = (false, false);
            for drive in base.into_iter().flat_map(|b| &b.mouth) {
                let Ok(v) = serde_json::to_value(drive) else { continue };
                if keeping && check.mouth(&v, "mouth", &rig, &mut Out::default()).is_none() {
                    goes = true;
                    continue;
                }
                if let Some(drive) = check.mouth(&v, "mouth", &rig, &mut cx.out) {
                    moving.mouth.push(drive);
                    stays = true;
                }
            }
            if stays {
                carried.kept.push("mouth".to_string());
            }
            if goes {
                carried.cleared.push("mouth".to_string());
            }
        }
        Some(Value::Null) => {}
        Some(one @ Value::Object(_)) => moving.mouth.extend(check.mouth(one, "mouth", &rig, &mut cx.out)),
        Some(Value::Array(list)) if list.len() <= MAX_MOUTH_DRIVES => {
            for (i, drive) in list.iter().enumerate() {
                moving.mouth.extend(check.mouth(drive, &format!("mouth[{i}]"), &rig, &mut cx.out));
            }
        }
        Some(other) => {
            let found = match other {
                Value::Array(list) => format!("a list of {}", list.len()),
                other => kind_of(other).to_string(),
            };
            cx.out.problems.add("mouth-field", "mouth", format!("mouth is {found}"), &format!("one drive, or a list of up to {MAX_MOUTH_DRIVES} that share one openness, or null for none"));
        }
    }

    // Drives replace, as the mouth does.
    match req.get("drives") {
        None => {
            let (mut stays, mut goes) = (false, false);
            for (i, drive) in base.into_iter().flat_map(|b| b.drives.iter().enumerate()) {
                let Ok(v) = serde_json::to_value(drive) else { continue };
                let at = format!("drives[{i}]");
                if keeping && check.drive(&v, &at, &rig, &mut Out::default()).is_none() {
                    goes = true;
                    continue;
                }
                if let Some(drive) = check.drive(&v, &at, &rig, &mut cx.out) {
                    moving.drives.push(drive);
                    stays = true;
                }
            }
            if stays {
                carried.kept.push("drives".to_string());
            }
            if goes {
                carried.cleared.push("drives".to_string());
            }
        }
        Some(Value::Null) => {}
        Some(Value::Array(list)) if list.len() <= MAX_DRIVES => {
            for (i, drive) in list.iter().enumerate() {
                moving.drives.extend(check.drive(drive, &format!("drives[{i}]"), &rig, &mut cx.out));
            }
        }
        Some(other) => {
            let found = match other {
                Value::Array(list) => format!("a list of {}", list.len()),
                other => kind_of(other).to_string(),
            };
            cx.out.problems.add("drives-field", "drives", format!("drives is {found}"), &format!("a list of up to {MAX_DRIVES} drives, [] or null for none"));
        }
    }

    // Gaze replaces.
    match req.get("gaze") {
        None => {
            if let Some(gaze) = base.and_then(|b| b.gaze) {
                let mut probe = Out::default();
                match check.gaze(&json!(gaze), if keeping { &mut probe } else { &mut cx.out }) {
                    Some(gaze) => {
                        moving.gaze = Some(gaze);
                        carried.kept.push("gaze".to_string());
                    }
                    None => carried.cleared.push("gaze".to_string()),
                }
            }
        }
        Some(Value::Null) => {}
        Some(v) => moving.gaze = check.gaze(v, &mut cx.out),
    }
    if moving.gaze.is_some_and(|g| g.get() > 0.0) && !moving.roles.contains_key(&Role::Pupils) {
        cx.out.warnings.add("gaze-pupils", "", "gaze moves the part with the pupils role, and no part has it".to_string(), "name it in roles");
    }
    arm_pivot_warnings(&moving, &mut cx.out);
    repeated_role_warnings(&moving, check.ix, &mut cx.out);
    (moving, carried)
}

/// What a call with `svg` and without `keep` cleared of the earlier look.
fn cleared_by(old: &Look, new: &Look) -> Vec<String> {
    let mut out = Vec::new();
    phrase(&mut out, "pivots", &old.pivots.keys().filter(|k| !new.pivots.contains_key(*k)).cloned().collect::<Vec<_>>());
    phrase(&mut out, "parents", &old.parents.keys().filter(|k| !new.parents.contains_key(*k)).cloned().collect::<Vec<_>>());
    phrase(&mut out, "roles", &old.roles.keys().filter(|r| !new.roles.contains_key(*r)).map(|r| r.name().to_string()).collect::<Vec<_>>());
    phrase(&mut out, "animations", &old.animations.keys().filter(|k| !new.animations.contains_key(*k)).cloned().collect::<Vec<_>>());
    phrase(&mut out, "states", &old.states.keys().filter(|s| !new.states.contains_key(*s)).map(|s| s.name().to_string()).collect::<Vec<_>>());
    if !old.mouth.is_empty() && new.mouth.is_empty() {
        out.push("mouth".to_string());
    }
    if old.gaze.is_some() && new.gaze.is_none() {
        out.push("gaze".to_string());
    }
    if !old.drives.is_empty() && new.drives.is_empty() {
        out.push("drives".to_string());
    }
    out
}

/// The characters of every `d` keyframe a look holds: tracks, drives and mouth drives.
fn d_keyframe_chars(look: &Look) -> usize {
    let tracks = look.animations.values().flat_map(|a| &a.tracks).filter(|t| t.prop == Prop::D).flat_map(|t| &t.values);
    let drives = look.drives.iter().filter(|d| d.prop == Prop::D).flat_map(|d| &d.values);
    let keyframes = tracks.chain(drives).map(|v| match v {
        Some(Key::Str(d)) => d.chars().count(),
        _ => 0,
    });
    let mouth = look.mouth.iter().map(|drive| match drive {
        MouthDrive::Path { open, closed, .. } => open.chars().count() + closed.as_ref().map_or(0, |c| c.chars().count()),
        MouthDrive::Prop { .. } => 0,
    });
    keyframes.chain(mouth).sum()
}

/// A role arm without a pivot warns, once for each: Workbench's wave and cheer raise an arm about
/// its pivot, the shoulder, and without one they can only wiggle it about the centre of its box
/// (`arm_tracks` of builtin.rs). Only the built-ins that move that arm in this look count: wave
/// moves armR, or armL where there is no armR, cheer every arm where it plays, and one the look
/// replaces with an animation of its own moves nothing.
/// A role part that is a `use` of another role's part warns: a use repeats what it names with
/// all its motion, so the repeated part moves once for itself and once more with the original
/// (a right arm that is a mirrored use of the left arm goes out of sight when both are raised).
fn repeated_role_warnings(moving: &Moving, ix: &Index, out: &mut Out) {
    for (role, id) in &moving.roles {
        let Some(entry) = ix.by_id.get(id.as_str()).filter(|entry| entry.node.tag == Tag::Use) else { continue };
        let Some(target) = entry.node.attrs.get("useRef").and_then(AttrValue::str) else { continue };
        let Some((other, _)) = moving.roles.iter().find(|(other, named)| *other != role && named.as_str() == target) else { continue };
        let what = format!("{id} is a use of {target}, the {} part: it also moves whenever {target} moves", other.name());
        out.warnings.add(&format!("role-use {id}"), &format!("roles.{}", role.name()), what, "draw it as a part of its own, or let both be uses of a shape in defs");
    }
}

fn arm_pivot_warnings(moving: &Moving, out: &mut Out) {
    let own = |name: &str| moving.animations.contains_key(name);
    let cheers = builtin_missing("cheer", &moving.roles, None).is_none() && !own("cheer");
    for (role, waved) in [(Role::ArmR, true), (Role::ArmL, !moving.roles.contains_key(&Role::ArmR))] {
        let Some(id) = moving.roles.get(&role).filter(|id| !moving.pivots.contains_key(*id)) else { continue };
        let which = match (waved && !own("wave"), cheers) {
            (true, true) => "wave and cheer",
            (true, false) => "wave",
            (false, true) => "cheer",
            (false, false) => continue,
        };
        let what = format!("{id} has no pivot: built-in {which} can only wiggle it");
        out.warnings.add(&format!("arm-pivot {id}"), &format!("roles.{}", role.name()), what, &format!("give pivots.{id} its shoulder, in the coordinates it is drawn in"));
    }
}

/// Where an input rests along its range, as a drive's places count it from 0 to 1: the middle
/// for where the figure looks and how it turns, the start for the mouth, the pace and the drag.
fn rest_place(input: Input) -> f64 {
    match input {
        Input::LookX | Input::LookY | Input::Turn => 0.5,
        Input::Mouth | Input::Pace | Input::Held => 0.0,
    }
}

/// A drive that changes the figure at rest warns. The browser applies drives on every frame, so a
/// drive whose value at its input's rest is not the prop's own rest (0 for translate, rotate and
/// skew, 1 for a scale, for the absolute props what the part shows without it, [`rest_value`])
/// redraws the figure while nothing happens: two opacities along turn leave the part half there
/// at rest. Within [`unseen`] of the rest is the rest. A keyframe written at the rest that hides
/// the part (a scale or an opacity of 0) is meant: a side view folded away while the figure faces
/// the user. Another keyframe written there may be meant, and the cure says so; a value between
/// two keyframes is not. It warns and never refuses, so stored looks keep loading, and all such
/// drives of a design are one warning, so they leave room for the others. The value at rest is
/// sampled as the browser samples a drive.
#[allow(clippy::too_many_arguments)]
fn rest_warning(input: Input, targets: &[(&str, &Entry<'_>)], prop: Prop, values: &[Option<Key>], places: Option<&[f64]>, ease: Option<&Eases>, at: &str, view: &View, out: &mut Out) {
    let n = values.len();
    if n < 2 {
        return;
    }
    let places = places.filter(|p| p.len() == n).map_or_else(|| even(n), <[f64]>::to_vec);
    let rest = rest_place(input);
    let mut i = 0;
    while i + 2 < n && rest >= places[i + 1] {
        i += 1;
    }
    let span = places[i + 1] - places[i];
    let u = if span > 0.0 { ((rest - places[i]) / span).clamp(0.0, 1.0) } else { 1.0 };
    // On a keyframe's place the ease does not matter; between two it does.
    let on_place = u <= 0.0 || u >= 1.0;
    let f = if u <= 0.0 {
        0.0
    } else if u >= 1.0 {
        1.0
    } else {
        let written = ease.and_then(|e| serde_json::to_value(e).ok());
        super::preview::eased(written.as_ref(), n - 1, i, u)
    };
    let (a, b) = (&values[i], &values[i + 1]);
    let number = |key: &Option<Key>, own: f64| match key {
        Some(Key::Num(v)) => v.get(),
        _ => own,
    };
    // A length is seen against the viewBox, in the part's own units: what scales the part at
    // rest scales what it moves, a translation along its own axis (a part folded flat shows
    // nothing).
    let side = view.w.max(view.h);
    let size = |entry: &Entry<'_>| {
        let m = &entry.to_root;
        let scale = match prop {
            Prop::TranslateX => m[0].hypot(m[1]),
            Prop::TranslateY => m[2].hypot(m[3]),
            _ => (m[0] * m[3] - m[1] * m[2]).abs().sqrt(),
        };
        if scale > 0.0 {
            side / scale
        } else if scale == 0.0 {
            f64::INFINITY
        } else {
            side
        }
    };
    let differs = |v: f64, own: f64, size: f64| {
        let swing = values.iter().map(|key| (number(key, own) - own).abs()).fold(0.0, f64::max);
        (v - own).abs() > unseen(prop, swing, size)
    };
    // A keyframe written at the rest may be meant; one between two never is. Null is the part's
    // own value; what it inherits (`inherited`) is written out, as null cannot be.
    let cure = |inherited: Option<String>| -> String {
        if on_place {
            let k = if u <= 0.0 { i } else { i + 1 };
            let rests = if prop.is_relative() { fmt_num(if prop.is_scale() { 1.0 } else { 0.0 }) } else { inherited.unwrap_or_else(|| "null, the artwork's own".to_string()) };
            format!("make values[{k}] {rests}, unless that is meant")
        } else {
            "an odd number of values with the rest in the middle, or place them with at".to_string()
        }
    };
    let gives = |id: &str, value: String, instead: String| format!("{} gives {id} {} {value} at rest, not {instead}", input.name(), prop.name());
    let what = if prop.is_relative() {
        let own = if prop.is_scale() { 1.0 } else { 0.0 };
        let v = number(a, own) + (number(b, own) - number(a, own)) * f;
        let hidden = on_place && prop.is_scale() && v.abs() < 0.0005;
        let ids: Vec<&str> = targets.iter().map(|(id, _)| *id).collect();
        // The parts move alike: the one drawn largest shows it first.
        let size = targets.iter().map(|(_, entry)| size(entry)).fold(f64::INFINITY, f64::min);
        (!hidden && differs(v, own, size)).then(|| (gives(&listed(&ids), fmt_num(v), fmt_num(own)), None))
    } else {
        targets.iter().find_map(|(id, entry)| {
            let (own, inherited) = rest_value(entry, prop);
            let written = |text: String| inherited.then(|| cure(Some(text)));
            match own {
                Own::Num(own) => {
                    let v = number(a, own) + (number(b, own) - number(a, own)) * f;
                    let v = if prop.is_length() { v.max(0.0) } else { v };
                    let hidden = on_place && prop == Prop::Opacity && v.abs() < 0.0005;
                    (!hidden && differs(v, own, size(entry))).then(|| (gives(id, fmt_num(v), format!("the artwork's {}", fmt_num(own))), written(fmt_num(own))))
                }
                Own::Text(own) => {
                    let text = |key: &Option<Key>| match key {
                        Some(Key::Str(s)) => s.clone(),
                        _ => own.to_string(),
                    };
                    let (va, vb) = (text(a), text(b));
                    let same = |x: &str, y: &str| if prop == Prop::D { x == y } else { x.eq_ignore_ascii_case(y) };
                    let at_rest = if f == 0.0 || same(&va, &vb) {
                        Some(va)
                    } else if f == 1.0 {
                        Some(vb)
                    } else {
                        None
                    };
                    let what = match (prop, at_rest) {
                        (_, Some(v)) if same(&v, own) => None,
                        (Prop::D, _) => Some(format!("{} gives {id} a d at rest, not the artwork's own", input.name())),
                        (_, Some(v)) => Some(gives(id, v, format!("the artwork's {own}"))),
                        (_, None) => Some(gives(id, "a blend".to_string(), format!("the artwork's {own}"))),
                    };
                    // No keyframe stands for none: the part is given the paint unseen, and that
                    // is driven.
                    let cure = if own == "none" { Some(format!("give it a {name} at {name}-opacity 0 and drive {name}Opacity instead", name = prop.name())) } else { written(own.to_string()) };
                    what.map(|what| (what, cure))
                }
                Own::Unknown => None,
            }
        })
    };
    if let Some((what, written)) = what {
        let each = Each { one: "drive", many: "drives", cure: "make each give its prop's rest at its input's rest", places: Vec::new() };
        out.warnings.add_each("drive-rest", at, what, &written.unwrap_or_else(|| cure(None)), each);
    }
}

/// How far from its rest a drive may leave a part at rest and the rest not be seen to change, in
/// the units of its prop: a tenth of the drive's swing (a part placed a little off its
/// keyframes), never less than what cannot be seen and never more than what plainly is. Those are
/// a hundredth and three hundredths of a scale, half a degree and two degrees of a turn or a
/// skew, two hundredths and a tenth of an opacity, and of a length (a translation, a size, a stroke)
/// 0.2 and 1 percent of `size`, the viewBox's larger side in the part's own units: half a pixel
/// and about two and a half pixels in the 264 px picture.
fn unseen(prop: Prop, swing: f64, size: f64) -> f64 {
    let (least, most) = if prop.is_scale() {
        (0.01, 0.03)
    } else if prop == Prop::Rotate || prop.is_skew() {
        (0.5, 2.0)
    } else if prop.is_opacity() {
        (0.02, 0.1)
    } else {
        (0.002 * size, 0.01 * size)
    };
    (0.1 * swing).max(least).min(most)
}

/// What a part shows of an absolute prop without the drive, as [`rest_warning`] compares it, and
/// whether it inherits that: its own value; for what it inherits (the fill, the stroke and the
/// opacities, width and offset that go with them) the nearest element around it that sets one,
/// as the renderer finds it, else the initial value (a black fill, no stroke, a width of 1,
/// opacities of 1, no offset). A plain colour, `none` or a number; unknown where it is not (a
/// gradient, `currentColor`), and where nothing the part draws shows it ([`passes_on`]: a group
/// whose parts all set their own, a stroke's width where no stroke is painted).
fn rest_value<'e>(entry: &'e Entry<'_>, prop: Prop) -> (Own<'e>, bool) {
    let (keys, initial): (&[&str], Own<'e>) = match prop {
        Prop::Fill => (&["fill", "fillRef"], Own::Text("#000000")),
        Prop::Stroke => (&["stroke", "strokeRef"], Own::Text("none")),
        Prop::StrokeWidth => (&["strokeWidth"], Own::Num(1.0)),
        Prop::FillOpacity => (&["fillOpacity"], Own::Num(1.0)),
        Prop::StrokeOpacity => (&["strokeOpacity"], Own::Num(1.0)),
        Prop::StrokeDashoffset => (&["strokeDashoffset"], Own::Num(0.0)),
        _ => return (entry.own(prop), false),
    };
    let chain = || std::iter::once(entry.node).chain(entry.around.iter().rev().copied());
    // A width, an opacity or an offset of a paint shows only where that paint is: its key, its
    // reference's key, and what nothing sets (a black fill, no stroke).
    let paint = match prop {
        Prop::StrokeWidth | Prop::StrokeOpacity | Prop::StrokeDashoffset => Some(("stroke", "strokeRef", false)),
        Prop::FillOpacity => Some(("fill", "fillRef", true)),
        _ => None,
    };
    let painted = paint.is_none_or(|(key, reference, initial)| chain().find_map(|node| painting(node, key, reference)).unwrap_or(initial));
    if !passes_on(entry.node, keys, paint.map(|(key, reference, _)| (key, reference)), painted) {
        return (Own::Unknown, false);
    }
    for (k, node) in chain().enumerate() {
        let inherited = k > 0;
        match node.attrs.get(keys[0]) {
            Some(AttrValue::Num(n)) if !prop.is_colour() => return (Own::Num(n.get()), inherited),
            Some(AttrValue::Str(s)) if prop.is_colour() && (s.starts_with('#') || s == "none") => return (Own::Text(s), inherited),
            Some(_) => return (Own::Unknown, inherited),
            None if keys.iter().any(|key| node.attrs.contains_key(*key)) => return (Own::Unknown, inherited),
            None => {}
        }
    }
    (initial, true)
}

/// Whether what `node` paints with an inherited prop (set under one of `keys`) shows: a shape, a
/// text or a use draws it (a line has no fill); a group only where something it draws takes it
/// from the group (sets none of its own, nor does a group between them). With `paint` (the
/// paint's key and its reference's key) only what is painted with it shows it; `painted`:
/// whether `node` is.
fn passes_on(node: &LookNode, keys: &[&str], paint: Option<(&str, &str)>, painted: bool) -> bool {
    let draws = |tag: Tag| (tag.is_shape() && !(tag == Tag::Line && keys[0].starts_with("fill"))) || matches!(tag, Tag::Text | Tag::Use);
    if node.tag != Tag::G {
        return painted && draws(node.tag);
    }
    node.children.iter().filter(|child| !keys.iter().any(|key| child.attrs.contains_key(*key))).any(|child| {
        let painted = paint.is_none_or(|(key, reference)| painting(child, key, reference).unwrap_or(painted));
        match child.tag {
            Tag::G => passes_on(child, keys, paint, painted),
            tag => painted && draws(tag),
        }
    })
}

/// Whether `node` sets a paint (under `key`, or a reference under `reference`) that is drawn:
/// anything but `none`. `None` where it sets none and inherits it.
fn painting(node: &LookNode, key: &str, reference: &str) -> Option<bool> {
    if node.attrs.contains_key(reference) {
        return Some(true);
    }
    node.attrs.get(key).map(|paint| paint.str() != Some("none"))
}

/// The lint of the rest pose: parts that reach outside the viewBox, parts too small to see.
fn lint_parts(look: &Look, ix: &Index<'_>, out: &mut Out) {
    let view = View::of(look);
    let (mut outside, mut small): (Vec<(&str, Bx)>, Vec<&str>) = (Vec::new(), Vec::new());
    for id in &ix.order {
        let Some(entry) = ix.by_id.get(id) else { continue };
        let b = entry.root_box();
        // Whether it is seen is a matter of what is painted: a stroke is seen as well.
        let painted = entry.painted_box();
        if painted.w().max(painted.h()) < SMALL_PART {
            small.push(*id);
        } else if !view.holds(&b) {
            outside.push((*id, b));
        }
    }
    if let Some((id, b)) = outside.first() {
        let mut what = format!("{id} reaches outside the viewBox: x {} to {}, y {} to {}", fmt_num(b.x0), fmt_num(b.x1), fmt_num(b.y0), fmt_num(b.y1));
        if outside.len() > 1 {
            let others: Vec<&str> = outside.iter().skip(1).map(|(id, _)| *id).collect();
            let _ = write!(what, "; so {} {}", if others.len() == 1 { "does" } else { "do" }, listed(&others));
        }
        out.warnings.add("outside", "", what, "what leaves the viewBox is cut off");
    }
    if !small.is_empty() {
        out.warnings.add("small", "", format!("smaller than {SMALL_PART} units: {}", listed(&small)), "it will not be seen");
    }
}

fn run(current: Option<&Design>, request: &Value, mask: &dyn Fn(&str) -> String, mode: Mode) -> Result<Applied, Vec<String>> {
    let Some(req) = request.as_object() else {
        return Err(vec![format!("a design is {}: an object with any of {}", kind_of(request), KEYS.join(", "))]);
    };
    if mode == Mode::Call {
        // Before anything is read.
        let size = json_len(request);
        if size > MAX_DESIGN_BYTES {
            return Err(vec![format!("the design is {} KiB of JSON: at most {} KiB", kib(size), MAX_DESIGN_BYTES / 1024)]);
        }
    }
    let mut cx = Ctx::new(mask);
    let known = keys(req, &KEYS, "", &mut cx.out, mask);
    if known && !req.keys().any(|k| k != "keep") {
        cx.out.problems.add("empty", "", "an empty design".to_string(), "send at least one of name, svg, pivots, parents, roles, animations, states, mouth, gaze, drives, voice");
    }
    let keep = match req.get("keep") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(keep)) => *keep,
        Some(other) => {
            cx.out.problems.add("keep-form", "keep", format!("keep is {}", kind_of(other)), "true or false");
            false
        }
    };

    // The artwork and what moves it come first: of more kinds of problems than a refusal
    // lists, the ones that decide the picture are the ones to show.
    let mut design = current.cloned().unwrap_or_default();
    let old = current.and_then(|d| d.look.as_ref());
    let touches_look = LOOK_KEYS.iter().any(|k| req.contains_key(*k));
    let mut svg_bytes = None;
    let mut carried = Carried::default();
    let look: Option<Look> = match req.get("svg") {
        Some(Value::String(svg)) => {
            svg_bytes = Some(svg.len());
            // Artwork that could not be read to its end has no parts to check the rest against:
            // its problems are reported alone.
            read_svg(svg, MAX_SVG_BYTES, &mut cx).map(|artwork| {
                // Where the transforms above a part put it: its own box was checked as it was read.
                let ix = Index::of(&artwork.nodes);
                for id in &ix.order {
                    if let Some(what) = ix.by_id.get(id).and_then(|entry| too_far(id, &entry.root_box())) {
                        cx.out.problems.add("span", "", what, &span_accepted());
                    }
                }
                let (moving, moved) = assemble(artwork.view, &artwork.nodes, old.filter(|_| keep), true, req, &mut cx);
                carried = moved;
                let view = artwork.view;
                let look = moving.look([view.x, view.y, view.w, view.h].map(Num::new), artwork.nodes);
                if let Some(old) = old.filter(|_| !keep) {
                    carried.cleared = cleared_by(old, &look);
                }
                look
            })
        }
        Some(other) => {
            cx.out.problems.add("svg-form", "svg", format!("svg is {}", kind_of(other)), "the artwork as a string of SVG markup");
            None
        }
        None => match old {
            // The look stays as it is unless the call touches it.
            Some(old) if touches_look => {
                let (moving, _) = assemble(View::of(old), &old.nodes, Some(old), false, req, &mut cx);
                Some(moving.look(old.view_box, old.nodes.clone()))
            }
            Some(_) => None,
            None => {
                if let Some(key) = LOOK_KEYS.iter().find(|k| req.contains_key(**k)) {
                    cx.out.problems.add("no-artwork", key, format!("{key} without artwork"), "this session has no design yet, send svg in the same call");
                }
                None
            }
        },
    };

    match req.get("name") {
        None => {}
        Some(Value::Null) => design.name.clear(),
        Some(Value::String(name)) => match text::plain(name, MAX_NAME_CHARS, mask) {
            Ok(name) => design.name = name,
            Err(refusal) => cx.out.problems.add("name-long", "name", format!("the name has {refusal}"), "a short name of the avatar's own"),
        },
        Some(other) => cx.out.problems.add("name-form", "name", format!("name is {}", kind_of(other)), "a string, the avatar's own name"),
    }
    match req.get("voice") {
        None => {}
        Some(Value::Null) => design.voice = Voice::default(),
        Some(voice) => {
            if let Some(voice) = check_voice(voice, &mut cx) {
                design.voice = voice;
            }
        }
    }

    let mut usage = Usage::default();
    if let Some(look) = &look {
        // The artwork is stored, and given back, in its normal form (spaced path data, colours
        // as hex), which may be longer than what was sent. It keeps to the limit in that form
        // too, so that what `to_svg` writes can always be sent again.
        let normal = if svg_bytes.is_some() { to_svg(look).len() } else { 0 };
        if normal > MAX_SVG_BYTES {
            cx.out.problems.add("svg-normal", "", format!("svg is {} KiB in the form it is stored and given back in", kib(normal)), &format!("at most {} KiB, so shorter paths or fewer shapes", MAX_SVG_BYTES / 1024));
        }
        let d_chars = d_keyframe_chars(look);
        if d_chars > MAX_D_KEYFRAMES_TOTAL {
            cx.out.problems.add("d-total", "", format!("{d_chars} characters of d keyframes in the look"), &format!("at most {MAX_D_KEYFRAMES_TOTAL} over all tracks and the mouth"));
        }
        usage.look_bytes = json_len(look);
        if usage.look_bytes > MAX_LOOK_BYTES {
            cx.out.problems.add("look-size", "", format!("the look is {} KiB once stored", kib(usage.look_bytes)), &format!("at most {} KiB, so fewer elements, shorter paths or fewer keyframes", MAX_LOOK_BYTES / 1024));
        }
    }
    if !cx.out.problems.is_empty() {
        return Err(cx.out.problems.problems());
    }

    // Accepted: nothing below refuses.
    if let Some(look) = look {
        design.look = Some(look);
    }
    let mut parts = Vec::new();
    let mut animations = Vec::new();
    if let Some(look) = &design.look {
        let ix = Index::of(&look.nodes);
        if svg_bytes.is_some() || touches_look {
            lint_parts(look, &ix, &mut cx.out);
        }
        parts = parts_of(look, &ix);
        animations = look.animations.keys().cloned().collect();
        usage = Usage {
            svg_bytes: svg_bytes.unwrap_or_else(|| to_svg(look).len()),
            elements: ix.elements,
            animations: look.animations.len(),
            parents: look.parents.len(),
            drives: look.drives.len(),
            look_bytes: if usage.look_bytes > 0 { usage.look_bytes } else { json_len(look) },
        };
    }
    let mut warnings: Vec<String> = cx.drops.line().into_iter().collect();
    let (lines, more) = cx.out.warnings.lines();
    warnings.extend(lines);
    let total = warnings.len() + more;
    if total > MAX_WARNINGS {
        warnings.truncate(MAX_WARNINGS - 1);
        warnings.push(format!("and {} more warnings", total - (MAX_WARNINGS - 1)));
    }
    Ok(Applied { design, parts, animations, kept: carried.kept, cleared: carried.cleared, warnings, usage })
}

// ---------------------------------------------------------------- the artwork written back

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The artwork as SVG text. `strict`: a key of `attrs` that is not in the closed table gives
/// `None` (a stored file); otherwise it is left out, which cannot happen to a look this file made.
fn svg_text(look: &Look, strict: bool) -> Option<String> {
    let [x, y, w, h] = look.view_box.map(|n| fmt_num(n.get()));
    let mut out = format!("<svg viewBox='{x} {y} {w} {h}'>");
    // Each element is met twice, to open it and to close it; the stack is this function's own.
    let mut stack: Vec<(&LookNode, bool)> = look.nodes.iter().rev().map(|n| (n, false)).collect();
    while let Some((node, closing)) = stack.pop() {
        let name = node.tag.name();
        if closing {
            let _ = write!(out, "</{name}>");
            continue;
        }
        let _ = write!(out, "<{name}");
        if let Some(id) = &node.id {
            let _ = write!(out, " id='{}'", escape(id));
        }
        for (key, value) in &node.attrs {
            let Some(attr) = attr_name(node.tag, key) else {
                if strict {
                    return None;
                }
                continue;
            };
            let text = match value {
                AttrValue::Str(id) if key == "useRef" => format!("#{id}"),
                AttrValue::Str(id) if key.ends_with("Ref") => format!("url(#{id})"),
                AttrValue::Str(s) => s.clone(),
                AttrValue::Num(n) => fmt_num(n.get()),
            };
            let _ = write!(out, " {attr}='{}'", escape(&text));
        }
        if node.children.is_empty() && node.text.is_none() {
            out.push_str("/>");
            continue;
        }
        out.push('>');
        if let Some(text) = &node.text {
            out.push_str(&escape(text));
        }
        stack.push((node, true));
        for child in node.children.iter().rev() {
            stack.push((child, false));
        }
    }
    out.push_str("</svg>");
    Some(out)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// A small figure with a part of every kind the tests of what moves it need: groups, a path
    /// with a `d`, shapes with and without colours of their own, a text; and what is not a part:
    /// a gradient, a clip path and the shape inside it.
    const FIGURE: &str = "<svg viewBox='0 0 200 200'><defs><linearGradient id='sky'><stop offset='0' stop-color='#fff'/><stop offset='1' stop-color='navy'/></linearGradient><clipPath id='window'><circle id='pane' cx='100' cy='100' r='50'/></clipPath></defs><g id='body'><ellipse id='belly' cx='100' cy='140' rx='40' ry='30' fill='url(#sky)'/><g id='head'><circle id='face' cx='100' cy='80' r='30' fill='#ffcc00' stroke='#333333' stroke-width='2' opacity='0.9'/><g id='eyes'><circle id='pupil' cx='90' cy='75' r='4' fill='#111111'/></g><path id='mouth' d='M84 96 Q100 104 116 96' fill='none' stroke='#333333' stroke-width='3'/></g><rect id='arm' x='140' y='100' width='12' height='40' rx='4' fill='#ffcc00'/><line id='cane' x1='60' y1='120' x2='60' y2='170' stroke='#333333' stroke-width='3'/><text id='label' x='100' y='190' font-size='10' text-anchor='middle'>hi</text></g></svg>";

    fn svg(inner: &str) -> String {
        format!("<svg viewBox='0 0 200 200'>{inner}</svg>")
    }

    #[track_caller]
    fn accepted(current: Option<&Design>, request: Value) -> Applied {
        match apply(current, &request) {
            Ok(applied) => applied,
            Err(problems) => panic!("refused: {problems:#?}"),
        }
    }

    /// The problems of a refused call, joined.
    #[track_caller]
    fn refused(current: Option<&Design>, request: Value) -> String {
        match apply(current, &request) {
            Ok(applied) => panic!("accepted, with warnings {:#?}", applied.warnings),
            Err(problems) => {
                assert!(!problems.is_empty(), "a refusal says why");
                assert!(problems.len() <= MAX_PROBLEM_KINDS + 1, "at most {MAX_PROBLEM_KINDS} kinds and one line for the rest");
                for p in &problems {
                    assert!(p.chars().count() <= MAX_PROBLEM_CHARS, "a problem of {} characters: {p}", p.chars().count());
                    assert!(!p.chars().any(|c| c.is_control()), "a control character in {p:?}");
                }
                message(&problems)
            }
        }
    }

    /// The look of artwork given as the content of an svg root.
    #[track_caller]
    fn drawn(inner: &str) -> Look {
        accepted(None, json!({ "svg": svg(inner) })).design.look.expect("a look")
    }

    /// Why artwork given as the content of an svg root is refused.
    #[track_caller]
    fn undrawn(inner: &str) -> String {
        refused(None, json!({ "svg": svg(inner) }))
    }

    /// `part` is in `text`. A part written "found: accepted" also matches a problem, where the
    /// place stands between the two.
    #[track_caller]
    fn has(text: &str, part: &str) {
        if text.contains(part) {
            return;
        }
        if let Some((found, accepted)) = part.rsplit_once(": ") {
            if text.lines().any(|line| line.contains(found) && line.contains(&format!("): {accepted}"))) {
                return;
            }
        }
        panic!("{part:?} is not in:\n{text}");
    }

    /// `FIGURE` with more keys of a design call.
    fn figure(more: Value) -> Value {
        let mut request = json!({ "svg": FIGURE });
        for (key, value) in more.as_object().expect("an object") {
            request[key.as_str()] = value.clone();
        }
        request
    }

    /// `FIGURE` with one animation `a` of one second made of these tracks.
    fn tracks(tracks: Value) -> Value {
        figure(json!({ "animations": { "a": { "duration": 1, "tracks": tracks } } }))
    }

    /// The node with this id, anywhere in the tree.
    #[track_caller]
    fn node<'l>(look: &'l Look, id: &str) -> &'l LookNode {
        let mut stack: Vec<&LookNode> = look.nodes.iter().collect();
        while let Some(n) = stack.pop() {
            if n.id.as_deref() == Some(id) {
                return n;
            }
            stack.extend(&n.children);
        }
        panic!("no node {id}");
    }

    fn attrs(node: &LookNode) -> Value {
        serde_json::to_value(&node.attrs).unwrap()
    }

    fn bbox(look: &Look, id: &str) -> [f64; 4] {
        node(look, id).bbox.expect("a box").map(Num::get)
    }

    /// Every number a stored look holds, wherever it stands.
    fn numbers(look: &Look) -> Vec<f64> {
        let mut all = Vec::new();
        let mut stack = vec![serde_json::to_value(look).unwrap()];
        while let Some(v) = stack.pop() {
            match v {
                Value::Number(n) => all.push(n.as_f64().unwrap()),
                Value::Array(a) => stack.extend(a),
                Value::Object(o) => stack.extend(o.into_iter().map(|(_, v)| v)),
                _ => {}
            }
        }
        all
    }

    // ------------------------------------------------------------ the limits the web mirrors

    #[test]
    fn the_limits_the_web_mirrors_are_plain_one_line_constants() {
        // web/src/features/avatar/lib/look.test.ts reads these lines from this file.
        let source = include_str!("look.rs");
        for (name, value) in [
            ("MAX_ELEMENTS", MAX_ELEMENTS),
            ("MAX_DEPTH", MAX_DEPTH),
            ("MAX_ANIMATIONS", MAX_ANIMATIONS),
            ("MAX_TRACKS", MAX_TRACKS),
            ("MAX_KEYFRAMES", MAX_KEYFRAMES),
            ("MAX_D_CHARS", MAX_D_CHARS),
            ("MAX_POINTS_CHARS", MAX_POINTS_CHARS),
            ("MAX_TEXTS", MAX_TEXTS),
            ("MAX_TEXT_CHARS", MAX_TEXT_CHARS),
            ("MAX_STOPS", MAX_STOPS),
            ("MAX_ATTRS", MAX_ATTRS),
            ("MAX_FILTERS", MAX_FILTERS),
            ("MAX_FILTER_PRIMITIVES", MAX_FILTER_PRIMITIVES),
            ("MAX_DASHES", MAX_DASHES),
            ("MAX_PARENTS", MAX_PARENTS),
            ("MAX_DRIVES", MAX_DRIVES),
        ] {
            has(source, &format!("pub const {name}: usize = {value};"));
        }
        assert_eq!((MAX_FILTERS, MAX_FILTER_PRIMITIVES, MAX_DASHES, MAX_PARENTS, MAX_DRIVES), (8, 8, 8, 64, 16));
        assert_eq!((MAX_SVG_BYTES, MAX_LOOK_BYTES, MAX_DESIGN_BYTES), (32 * 1024, 128 * 1024, 256 * 1024));
    }

    // ------------------------------------------------------------ kept: the allow-list

    #[test]
    fn every_element_and_attribute_of_the_allow_list_is_kept_under_its_react_name() {
        let look = drawn(
            "<defs>\
               <linearGradient id='lg' x1='0' y1='0' x2='1' y2='1' gradientUnits='objectBoundingBox' gradientTransform='rotate(45)' spreadMethod='reflect'>\
                 <stop offset='0' stop-color='red' stop-opacity='0.5'/><stop offset='100%' stop-color='#00f'/>\
               </linearGradient>\
               <radialGradient id='rg' cx='50%' cy='50%' r='50%' fx='0.3' fy='0.3'><stop offset='0.5' stop-color='white'/></radialGradient>\
               <clipPath id='cp' clipPathUnits='userSpaceOnUse'><rect x='0' y='0' width='100' height='100'/></clipPath>\
             </defs>\
             <g id='whole' transform='translate(10 20)' fill='url(#lg)' stroke='url(#rg)' stroke-width='2' stroke-linecap='round' stroke-linejoin='bevel' stroke-miterlimit='4' \
                fill-opacity='0.5' stroke-opacity='50%' opacity='0.9' fill-rule='evenodd' clip-rule='nonzero' clip-path='url(#cp)'>\
               <path id='p' d='M0 0L10 10'/><circle id='c' cx='1' cy='2' r='3'/><ellipse id='e' cx='1' cy='2' rx='3' ry='4'/>\
               <rect id='r' x='1' y='2' width='3' height='4' rx='1' ry='1'/><line id='l' x1='1' y1='2' x2='3' y2='4'/>\
               <polyline id='pl' points='0,0 10,10 20,0'/><polygon id='pg' points='0 0 10 10 20 0'/>\
               <text id='t' x='5' y='6' font-size='12' font-weight='bold' text-anchor='middle' dominant-baseline='central' letter-spacing='1'>Hi</text>\
             </g>",
        );
        let all = node(&look, "whole");
        assert_eq!(
            attrs(all),
            json!({
                "clipPathRef": "cp", "clipRule": "nonzero", "fillOpacity": 0.5, "fillRef": "lg", "fillRule": "evenodd", "opacity": 0.9, "strokeLinecap": "round",
                "strokeLinejoin": "bevel", "strokeMiterlimit": 4, "strokeOpacity": 0.5, "strokeRef": "rg", "strokeWidth": 2, "transform": "translate(10 20)"
            })
        );
        let tags: Vec<&str> = all.children.iter().map(|n| n.tag.name()).collect();
        assert_eq!(tags, ["path", "circle", "ellipse", "rect", "line", "polyline", "polygon", "text"]);
        assert_eq!(attrs(node(&look, "p")), json!({ "d": "M 0 0 L 10 10" }));
        assert_eq!(attrs(node(&look, "c")), json!({ "cx": 1, "cy": 2, "r": 3 }));
        assert_eq!(attrs(node(&look, "e")), json!({ "cx": 1, "cy": 2, "rx": 3, "ry": 4 }));
        assert_eq!(attrs(node(&look, "r")), json!({ "x": 1, "y": 2, "width": 3, "height": 4, "rx": 1, "ry": 1 }));
        assert_eq!(attrs(node(&look, "l")), json!({ "x1": 1, "y1": 2, "x2": 3, "y2": 4 }));
        assert_eq!(attrs(node(&look, "pl")), json!({ "points": "0 0 10 10 20 0" }));
        assert_eq!(attrs(node(&look, "pg")), json!({ "points": "0 0 10 10 20 0" }));
        let text = node(&look, "t");
        assert_eq!(attrs(text), json!({ "x": 5, "y": 6, "fontSize": 12, "fontWeight": "bold", "textAnchor": "middle", "dominantBaseline": "central", "letterSpacing": 1 }));
        assert_eq!(text.text.as_deref(), Some("Hi"));

        let linear = node(&look, "lg");
        assert_eq!(linear.tag, Tag::LinearGradient);
        assert_eq!(attrs(linear), json!({ "x1": 0, "y1": 0, "x2": 1, "y2": 1, "gradientUnits": "objectBoundingBox", "gradientTransform": "rotate(45)", "spreadMethod": "reflect" }));
        assert_eq!(attrs(&linear.children[0]), json!({ "offset": 0, "stopColor": "#ff0000", "stopOpacity": 0.5 }));
        assert_eq!(attrs(&linear.children[1]), json!({ "offset": 1, "stopColor": "#0000ff" }));
        // Percentages of a gradient in box units are shares of the box.
        assert_eq!(attrs(node(&look, "rg")), json!({ "cx": 0.5, "cy": 0.5, "r": 0.5, "fx": 0.3, "fy": 0.3 }));
        assert_eq!(attrs(node(&look, "cp")), json!({ "clipPathUnits": "userSpaceOnUse" }));
        assert_eq!(look.nodes[0].tag, Tag::Defs);
    }

    #[test]
    fn camel_case_spellings_are_aliases_and_both_at_once_refuse() {
        let look = drawn("<path id='p' d='M0 0L5 5' strokeWidth='2' strokeLinecap='round' fillOpacity='0.5' clipRule='evenodd'/><text id='t' textAnchor='end' fontSize='12' dominantBaseline='hanging'>a</text>");
        assert_eq!(attrs(node(&look, "p")), json!({ "d": "M 0 0 L 5 5", "strokeWidth": 2, "strokeLinecap": "round", "fillOpacity": 0.5, "clipRule": "evenodd" }));
        assert_eq!(attrs(node(&look, "t")), json!({ "textAnchor": "end", "fontSize": 12, "dominantBaseline": "hanging" }));
        has(&undrawn("<circle r='5' stroke-width='2' strokeWidth='3'/>"), "stroke-width is given twice");
    }

    #[test]
    fn the_root_gives_the_view_box_and_its_other_attributes_are_ignored_or_moved_to_a_group() {
        let plain = accepted(None, json!({ "svg": "<svg><circle r='5'/></svg>" })).design.look.unwrap();
        assert_eq!(plain.view_box.map(Num::get), [0.0, 0.0, 200.0, 200.0]);

        let request = json!({ "svg": "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' xml:space='preserve' width='24' height='24' version='1.1' id='icon' class='lucide' preserveAspectRatio='xMidYMid' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='2' style='stroke-linecap: round; background: #fff'><path d='M0 0L5 5'/></svg>" });
        let applied = accepted(None, request);
        let look = applied.design.look.unwrap();
        assert_eq!(look.view_box.map(Num::get), [0.0, 0.0, 24.0, 24.0]);
        // The presentation attributes of the root sit on a group that wraps the content.
        assert_eq!(look.nodes.len(), 1);
        assert_eq!(look.nodes[0].tag, Tag::G);
        assert_eq!(look.nodes[0].id, None);
        assert_eq!(attrs(&look.nodes[0]), json!({ "fill": "none", "stroke": "currentColor", "strokeWidth": 2, "strokeLinecap": "round" }));
        assert_eq!(look.nodes[0].children[0].tag, Tag::Path);
        assert_eq!(applied.warnings, ["Not drawn: other style properties (1)"]);

        for bad in ["0 0 8 8", "0 0 5000 100", "0 0 200", "a b c d", "0 0 200 200 5", "9999 0 200 200", "0 0 1e999 200"] {
            has(&refused(None, json!({ "svg": format!("<svg viewBox='{bad}'><circle r='5'/></svg>") })), "viewBox");
        }
        has(&refused(None, json!({ "svg": "<svg transform='scale(2)'><circle r='5'/></svg>" })), "\"transform\" is not an attribute of <svg>");
    }

    #[test]
    fn numbers_are_rounded_to_three_decimals_and_an_integral_value_is_a_json_integer() {
        let look = drawn("<circle id='c' cx='10.00049' cy='-0.0001' r='5px'/><rect id='r' x='1e1' y='.5' width='2.' height='+3.14159' rx='0.0005'/>");
        assert_eq!(serde_json::to_string(&node(&look, "c").attrs).unwrap(), r#"{"cx":10,"cy":0,"r":5}"#);
        assert_eq!(serde_json::to_string(&node(&look, "r").attrs).unwrap(), r#"{"height":3.142,"rx":0.001,"width":2,"x":10,"y":0.5}"#);
        assert_eq!(serde_json::to_string(&look.view_box).unwrap(), "[0,0,200,200]");
        assert_eq!(serde_json::to_string(&[Num::new(-0.0), Num::new(6.0), Num::new(-2.5), Num::new(0.1 + 0.2), Num::new(f64::NAN)]).unwrap(), "[0,6,-2.5,0.3,0]");
        // Both spellings of a number read back as the same value.
        let back: [Num; 3] = serde_json::from_str("[6, 6.0, 0.12345]").unwrap();
        assert_eq!(back.map(Num::get), [6.0, 6.0, 0.123]);
        assert!(serde_json::from_str::<Num>("\"6\"").is_err());
    }

    #[test]
    fn numbers_that_are_not_plain_and_finite_refuse() {
        has(&undrawn("<circle r='1e999'/>"), "beyond any size");
        has(&undrawn("<circle r='NaN'/>"), "\"NaN\" is not a number");
        has(&undrawn("<circle r='inf'/>"), "is not a number");
        has(&undrawn("<circle r='Infinity'/>"), "is not a number");
        has(&undrawn("<circle r='5em'/>"), "\"5em\" is not a number");
        has(&undrawn("<circle r='50%'/>"), "percentages are not accepted");
        has(&undrawn("<circle r=''/>"), "is not a number");
        has(&undrawn("<circle r='5 5'/>"), "is not a number");
        has(&undrawn("<circle r='0x10'/>"), "is not a number");
        has(&undrawn("<circle r='-1'/>"), "r -1 is out of range");
        has(&undrawn("<circle cx='99999' r='5'/>"), "cx 99999 is out of range: -800 to 1000");
        has(&undrawn("<path d='M 1e999 0 L 1 1'/>"), "beyond any size");
    }

    #[test]
    fn out_of_range_font_size_stroke_width_and_scale_refuse() {
        has(&undrawn("<text font-size='99999'>a</text>"), "font-size 99999 is out of range");
        has(&undrawn("<text font-size='0'>a</text>"), "font-size 0 is out of range: above 0, up to 800");
        has(&undrawn("<circle r='5' stroke-width='99999'/>"), "stroke-width 99999 is out of range: 0 to 800");
        has(&undrawn("<circle r='5' stroke-width='-1'/>"), "stroke-width -1 is out of range");
        has(&undrawn("<circle r='5' transform='scale(99999)'/>"), "the transform scale() is out of range: factors from 0.001 to 1000 in size, either sign");
        has(&undrawn("<circle r='5' transform='scale(0)'/>"), "the transform scale() is out of range");
        has(&undrawn("<circle r='5' stroke-miterlimit='0.5'/>"), "stroke-miterlimit 0.5 is out of range");
        has(&undrawn("<text font-weight='950'>a</text>"), "svg: font-weight \"950\": normal, bold or one of 100, 200 and so on to 900");
        has(&undrawn("<text font-weight='650'>a</text>"), "normal, bold or one of 100, 200 and so on to 900");
        has(&undrawn("<text font-weight='bolder'>a</text>"), "normal, bold or one of 100, 200 and so on to 900");
        for weight in ["100", "400", "900"] {
            assert_eq!(attrs(node(&drawn(&format!("<text id='t' x='9' y='50' font-weight='{weight}'>a</text>")), "t"))["fontWeight"], json!(weight.parse::<u32>().unwrap()));
        }
        has(&undrawn("<circle r='5' stroke-linecap='pointy'/>"), "one of butt, round, square");
        // An opacity outside 0 to 1 is what SVG clamps, so it is clamped.
        assert_eq!(attrs(node(&drawn("<circle id='c' r='5' opacity='1.5' fill-opacity='-2' stroke-opacity='150%'/>"), "c")), json!({ "r": 5, "opacity": 1, "fillOpacity": 0, "strokeOpacity": 1 }));
    }

    #[test]
    fn colours_are_stored_as_hex() {
        for (written, stored) in [
            ("#abc", "#aabbcc"),
            ("#abcd", "#aabbccdd"),
            ("#AABBCC", "#aabbcc"),
            ("#11223344", "#11223344"),
            ("#112233ff", "#112233"),
            ("rgb(255, 0, 0)", "#ff0000"),
            ("rgb(255 128 0)", "#ff8000"),
            ("rgba(0,0,255,0.5)", "#0000ff80"),
            ("rgb(100% 0% 0% / 50%)", "#ff000080"),
            ("RGB(300, -5, 12.6)", "#ff000d"),
            ("hsl(120, 100%, 50%)", "#00ff00"),
            ("hsl(0 100% 50%)", "#ff0000"),
            ("hsl(240deg 100% 50%)", "#0000ff"),
            ("hsla(240 100% 50% / 0.25)", "#0000ff40"),
            ("hsl(0, 0%, 50%)", "#808080"),
            ("hsl(-120, 100%, 25%)", "#000080"),
            ("Tomato", "#ff6347"),
            ("rebeccapurple", "#663399"),
            ("GREY", "#808080"),
            ("transparent", "#00000000"),
            ("  red  ", "#ff0000"),
        ] {
            assert_eq!(colour(written).map(hex).as_deref(), Some(stored), "{written}");
        }
        for not in ["blurple", "#12", "#12345", "#ggg", "rgb(1,2)", "rgb(1,2,3,4,5)", "rgb(a,b,c)", "hwb(0 0% 0%)", "url(#a)", "none", "currentColor", "inherit", "", "rgb(1e999,0,0)", "red;", "expression(alert(1))"] {
            assert_eq!(colour(not), None, "{not}");
        }
        assert_eq!(COLOURS.len(), 148);
        assert!(COLOURS.windows(2).all(|w| w[0].0 < w[1].0), "sorted, for the search");

        let look = drawn("<circle id='a' r='5' fill='none' stroke='currentcolor'/><circle id='b' r='5' fill='gold' stroke='hsl(200 50% 50%)'/>");
        assert_eq!(attrs(node(&look, "a")), json!({ "r": 5, "fill": "none", "stroke": "currentColor" }));
        assert_eq!(attrs(node(&look, "b")), json!({ "r": 5, "fill": "#ffd700", "stroke": "#4095bf" }));
        let why = undrawn("<circle r='5' fill='blurple'/>");
        has(&why, "svg: fill \"blurple\" is not a colour");
        has(&why, "use #rrggbb, rgb(), hsl() or a CSS colour name");
        has(&undrawn("<stop offset='0' stop-color='none'/>"), "a stop belongs inside a gradient");
        has(&undrawn("<defs><linearGradient id='g'><stop offset='0' stop-color='none'/></linearGradient></defs><circle r='5'/>"), "stop-color \"none\" is not a colour");
    }

    #[test]
    fn path_data_follows_the_svg_grammar_and_is_stored_as_spaced_tokens() {
        let view = DEFAULT_VIEW;
        let read = |d: &str| path_data(d, &view, MAX_D_CHARS, &unmasked).map(|p| p.text);
        // `1-2` and `.5.5` are two numbers each; arc flags are tokens of their own.
        assert_eq!(read("M1-2.5.5-3L4 5a6 6 0 016 6z").unwrap(), "M 1 -2.5 0.5 -3 L 4 5 a 6 6 0 0 1 6 6 z");
        assert_eq!(read("  M 10,20\n\tL 30,40 Z ").unwrap(), "M 10 20 L 30 40 Z");
        assert_eq!(read("M0 0C1 2 3 4 5 6S7 8 9 10Q1 2 3 4T5 6H7V8").unwrap(), "M 0 0 C 1 2 3 4 5 6 S 7 8 9 10 Q 1 2 3 4 T 5 6 H 7 V 8");
        assert_eq!(read("M0 0 1e1 2E-1 1.23456 -0").unwrap(), "M 0 0 10 0.2 1.235 0");
        assert_eq!(read("M0 0A5 5 0 1 0 10 10 5 5 0 0110 20").unwrap(), "M 0 0 A 5 5 0 1 0 10 10 5 5 0 0 1 10 20");
        let structure = |d: &str| path_data(d, &view, MAX_D_CHARS, &unmasked).unwrap().structure;
        assert_eq!(structure("M66 104 Q100 120 134 104 Q100 124 66 104 Z").to_string(), "MQQZ with 10 numbers");
        assert_eq!(structure("M0 0A5 5 0 1 0 10 10").to_string(), "MA with 7 numbers and arc flags 10");

        for (d, why) in [
            ("", "empty path data"),
            ("L0 0", "path data starts with L"),
            ("5 5", "path data starts with"),
            ("M0", "the command M has too few numbers"),
            ("M0 0 L5", "the command L has too few numbers"),
            ("M0 0 L", "the command L has too few numbers"),
            ("M L 1 1", "the command M has too few numbers"),
            ("M0 0 X 1 1", "\"X\" is not a path command"),
            ("M0 0 Z 5", "a number follows Z"),
            ("M0 0 A1 1 0 2 0 5 5", "an arc flag in path data that is not 0 or 1"),
            ("M NaN 0", "\"N\" is not a path command"),
            ("M0 0 L 1 é", "in path data"),
            ("M0 0 L 99999 0", "the number 99999 in path data is out of range"),
            ("M0 0 url(#a)", "is not a path command"),
        ] {
            has(&read(d).unwrap_err(), why);
        }
        // What was written again reads as itself: the stored form is a fixed point.
        for d in ["M1-2.5.5-3L4 5a6 6 0 016 6z", "M0 0C1 2 3 4 5 6S7 8 9 10Q1 2 3 4T5 6H7V8", "M0 0A5 5 0 1 0 10 10 5 5 0 0110 20", "m 1e1,2E-1 1.23456 -0 z"] {
            let once = read(d).unwrap();
            assert_eq!(read(&once).unwrap(), once, "{d}");
            assert_eq!(structure(&once), structure(d), "{d}");
        }
        let look = drawn("<path id='p' d='M10 10\n  L 20 20\n  Z'/>");
        assert_eq!(attrs(node(&look, "p")), json!({ "d": "M 10 10 L 20 20 Z" }));
        has(&undrawn("<path d='M0 0 L5'/>"), "svg: the command L has too few numbers: path data is the commands MmLlHhVvCcSsQqTtAaZz and numbers, starting with M");
    }

    #[test]
    fn points_and_transforms_are_read_and_written_again() {
        let look = drawn("<polygon id='p' points=' 0,0 10,10\n20,0 '/><g id='g' transform='translate(10,20) rotate(45 100 100) scale(1.5) skewX(10) skewY(-5) matrix(1 0 0 1 5 5) scale(1,-1) translate(3)'><circle r='5'/></g>");
        assert_eq!(attrs(node(&look, "p")), json!({ "points": "0 0 10 10 20 0" }));
        assert_eq!(attrs(node(&look, "g")), json!({ "transform": "translate(10 20) rotate(45 100 100) scale(1.5) skewX(10) skewY(-5) matrix(1 0 0 1 5 5) scale(1 -1) translate(3)" }));
        has(&undrawn("<polygon points='0 0 10'/>"), "points hold 3 numbers: x and y in pairs");
        has(&undrawn("<polygon points='0 0 a b'/>"), "points hold more than numbers: numbers separated by spaces or commas");
        has(&undrawn("<polygon points=''/>"), "points hold 0 numbers");
        for (written, why) in [
            ("rotate(45deg)", "the transform \"rotate(45deg)\" does not hold plain numbers, six at most"),
            ("translate(10px, 5px)", "does not hold plain numbers"),
            ("skewX(89)", "the transform skewX() is out of range: at most 80 degrees"),
            ("perspective(10)", "\"perspective\" is not a transform function"),
            ("translate(10", "the transform translate( is not closed"),
            ("translate", "a transform that is not written as functions"),
            ("rotate(1 2)", "the transform rotate() with 2 numbers"),
            ("matrix(1 2 3)", "the transform matrix() with 3 numbers"),
            ("matrix(1 1 1 1 0 0)", "the transform matrix() would flatten the picture"),
            ("translate(99999)", "the transform translate() moves too far: within 800 either way"),
            ("rotate(99999)", "the transform rotate() is out of range: an angle up to 3600 degrees, a centre within -800 to 1000"),
            ("scale(1) scale(1) scale(1) scale(1) scale(1) scale(1) scale(1) scale(1) scale(1)", "a transform of more than 8 functions: at most 8"),
            ("url(#a)", "the transform \"url(#a)\" does not hold plain numbers"),
            ("rotate 45", "a transform that is not written as functions"),
        ] {
            has(&undrawn(&format!("<circle r='5' transform='{written}'/>")), why);
        }
    }

    #[test]
    fn ids_follow_one_pattern_are_unique_and_never_a_reserved_name() {
        let look = drawn("<circle id='a' r='5'/><circle id='B-2_x' r='5'/>");
        assert_eq!(node(&look, "B-2_x").tag, Tag::Circle);
        for id in ["1a", "-a", "_a", "a b", "a.b", "é", "", "a\u{202E}b", "__proto__", "x".repeat(33).as_str()] {
            let why = undrawn(&format!("<circle id='{id}' r='5'/>"));
            has(&why, "svg: the id ");
            has(&why, "a letter, then letters, digits, _ or -, at most 32 characters");
        }
        for id in RESERVED {
            has(&undrawn(&format!("<circle id='{id}' r='5'/>")), "this name is reserved");
        }
        assert_eq!(RESERVED.len(), 10);
        has(&undrawn("<circle id='a' r='5'/><rect id='a' width='5' height='5'/>"), "svg: the id a is used twice");
        // The longest id that fits.
        drawn(&format!("<circle id='{}' r='5'/>", "x".repeat(MAX_ID_CHARS)));
    }

    // ------------------------------------------------------------ dropped with a warning

    #[test]
    fn what_is_dropped_is_named_and_counted_and_the_call_succeeds() {
        let applied = accepted(
            None,
            json!({ "svg": svg(
                "<title>Pip</title><desc>A frog</desc><metadata><rdf:RDF xmlns:rdf='x'><cc:Work rdf:about=''>text</cc:Work></rdf:RDF></metadata>\
                 <circle id='c' cx='50' cy='50' r='5' font-family='Arial' font-style='italic' text-decoration='underline' paint-order='stroke' vector-effect='non-scaling-stroke' \
                   shape-rendering='crispEdges' text-rendering='auto' pointer-events='none' cursor='pointer' class='a b' data-name='x' data-n='1' \
                   aria-label='frog' role='img' tabindex='0'><animate attributeName='r' to='9' dur='1s' xlink:href='#c'/><title>inner</title></circle>\
                 <g className='x' fontFamily='Arial'><animateTransform attributeName='transform' type='rotate'/><animateMotion path='M0 0'><mpath href='#p'/></animateMotion><set attributeName='x' to='1'/><rect width='5' height='5'/></g>"
            ) }),
        );
        let look = applied.design.look.unwrap();
        assert_eq!(attrs(node(&look, "c")), json!({ "cx": 50, "cy": 50, "r": 5 }));
        assert!(node(&look, "c").children.is_empty());
        // Nothing of a dropped element is kept, its content included.
        let stored = serde_json::to_string(&look).unwrap();
        for gone in ["Arial", "animate", "title", "Pip", "frog", "pointer", "class", "data-", "aria", "img", "tabindex", "mpath"] {
            assert!(!stored.contains(gone), "{gone} is still in {stored}");
        }
        assert_eq!(applied.warnings.len(), 1, "{:#?}", applied.warnings);
        let first = &applied.warnings[0];
        // 21 things were dropped; the line names as many as fit and counts the rest.
        assert_eq!(first, "Not drawn: title (2), desc (1), metadata (1), font-family (2), font-style (1), text-decoration (1), paint-order (1), vector-effect (1) and 13 more");
        assert!(first.chars().count() <= MAX_PROBLEM_CHARS);
    }

    #[test]
    fn a_dropped_element_is_skipped_with_its_subtree_which_still_counts_for_depth_and_size() {
        // Anything may stand inside what is dropped: it is not read.
        drawn("<metadata><script>alert(1)</script><a href='x'>y</a>&unknown;<svg:g onload='x'/></metadata><circle r='5'/>");
        // Deep content of a dropped element counts for the depth.
        let deep = format!("<metadata>{}{}</metadata><circle r='5'/>", "<x>".repeat(MAX_DEPTH), "</x>".repeat(MAX_DEPTH));
        has(&undrawn(&deep), "nested deeper than 10");
        // And its elements count.
        let many = format!("<metadata>{}</metadata><circle r='5'/>", "<x/>".repeat(MAX_ELEMENTS));
        has(&undrawn(&many), "more than 300 elements");
    }

    #[test]
    fn transform_origin_is_dropped_with_a_warning_that_points_to_pivots() {
        let applied = accepted(None, json!({ "svg": svg("<g id='arm' transform-origin='10 10'><rect width='10' height='40' transformOrigin='center' style='transform-origin: 5px 5px'/></g>") }));
        assert_eq!(applied.warnings, ["transform-origin is not used (3 times, first at line 1 column 28, in g#arm): give the part a pivot in pivots instead"]);
    }

    #[test]
    fn a_tspan_is_flattened_into_its_text() {
        let applied = accepted(None, json!({ "svg": svg("<text id='t' x='5' y='20'>one <tspan x='5' dy='12' fill='red'>two<tspan> three</tspan></tspan> four<title>tip</title></text>") }));
        let look = applied.design.look.unwrap();
        assert_eq!(node(&look, "t").text.as_deref(), Some("one two three four"));
        assert!(node(&look, "t").children.is_empty());
        has(&applied.warnings[0], "Not drawn: title (1)");
        has(&applied.warnings[1], "a tspan is flattened into its text (2 times, first at line 1 column 58, in text#t): its words are kept, its own position and style are not");
        has(&undrawn("<text>a<tspan onclick='x'>b</tspan></text>"), "the event attribute \"onclick\"");
        has(&undrawn("<text>a<a href='x'>b</a></text>"), "svg: \"a\" inside a text");
        has(&undrawn("<text>a<g/></text>"), "svg: \"g\" inside a text");
        has(&undrawn("<tspan>a</tspan>"), "<tspan> is not supported");
    }

    #[test]
    fn style_declarations_become_attributes_and_win_over_them() {
        let applied = accepted(None, json!({ "svg": svg("<defs><linearGradient id='lg'><stop offset='0' style='stop-color: red; stop-opacity: 0.5'/></linearGradient></defs><circle id='c' cx='50' cy='50' r='5' fill='red' opacity='1' style='fill: blue; STROKE-WIDTH: 2px ;opacity:50%;; mix-blend-mode: multiply; font-family: Arial; transform: rotate(45deg)'/><rect id='r' width='5' height='5' style=\"fill:url('#lg'); clip-rule: evenodd\"/><text id='t' x='50' y='50' style='font-size:12px;font-weight:700;text-anchor:middle'>a</text>") }));
        let look = applied.design.look.unwrap();
        assert_eq!(attrs(node(&look, "c")), json!({ "cx": 50, "cy": 50, "r": 5, "fill": "#0000ff", "strokeWidth": 2, "opacity": 0.5 }));
        assert_eq!(attrs(node(&look, "r")), json!({ "width": 5, "height": 5, "fillRef": "lg", "clipRule": "evenodd" }));
        assert_eq!(attrs(node(&look, "t")), json!({ "x": 50, "y": 50, "fontSize": 12, "fontWeight": 700, "textAnchor": "middle" }));
        assert_eq!(attrs(&node(&look, "lg").children[0]), json!({ "offset": 0, "stopColor": "#ff0000", "stopOpacity": 0.5 }));
        assert_eq!(applied.warnings, ["Not drawn: other style properties (2), font-family (1)"]);
        // A value that the attribute would refuse refuses in style as well.
        has(&undrawn("<circle r='5' style='fill: blurple'/>"), "fill \"blurple\" is not a colour");
    }

    #[test]
    fn style_that_could_reach_outside_refuses() {
        has(&undrawn("<circle r='5' style='fill:url(http://x/a.svg#b)'/>"), "style holds a url() that is not url(#id)");
        has(&undrawn("<circle r='5' style='background: URL( \"//x\" )'/>"), "style holds a url() that is not url(#id)");
        has(&undrawn("<circle r='5' style='fill:url(#a'/>"), "style holds a url() that is not url(#id)");
        has(&undrawn("<circle r='5' style='fill:red !important'/>"), "style holds !important");
        has(&undrawn("<circle r='5' style='fill:red;/* c */'/>"), "style holds a comment");
        has(&undrawn("<circle r='5' style='fill:\\72 ed'/>"), "style holds an escape");
        has(&undrawn("<circle r='5' style='fill red'/>"), "style holds \"fill red\" without a colon");
        // Decoded first: a numeric reference does not hide a url.
        has(&undrawn("<circle r='5' style='fill:&#117;rl(http://x)'/>"), "style holds a url() that is not url(#id)");
        has(&undrawn("<circle r='5' style='fill:&#92;75rl(x)'/>"), "style holds an escape");
    }

    // ------------------------------------------------------------ refused: the reader rules

    #[test]
    fn the_xml_declaration_comments_and_white_space_around_the_root_are_accepted() {
        let request = json!({ "svg": "\u{FEFF}<?xml version='1.0' encoding='UTF-8' standalone='no'?>\n<!-- a frog -->\n<svg viewBox='0 0 200 200'>\n  <!-- body -->\n  <circle id='c' cx='50' cy='50' r='5'/>\n</svg>\n<!-- end -->\n" });
        let applied = accepted(None, request);
        assert!(applied.warnings.is_empty());
        assert_eq!(applied.parts.len(), 1);
    }

    #[test]
    fn processing_instructions_doctype_and_cdata_refuse() {
        has(&refused(None, json!({ "svg": "<?xml-stylesheet href='a.css'?><svg><circle r='5'/></svg>" })), "svg: a processing instruction is not accepted");
        has(&undrawn("<?php echo 1 ?><circle r='5'/>"), "svg: a processing instruction is not accepted");
        has(&refused(None, json!({ "svg": "<!DOCTYPE svg PUBLIC '-//W3C//DTD SVG 1.1//EN' 'http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd'><svg><circle r='5'/></svg>" })), "svg: a DOCTYPE is not accepted");
        // An entity declared in a DOCTYPE is never expanded: the DOCTYPE refuses, and so does the reference.
        let bomb = refused(None, json!({ "svg": "<!DOCTYPE svg [<!ENTITY a 'aaaaaaaaaa'><!ENTITY b '&a;&a;&a;&a;'>]><svg><text>&b;</text></svg>" }));
        has(&bomb, "svg: a DOCTYPE is not accepted");
        has(&bomb, "the reference \"b\" is not known");
        has(&undrawn("<text><![CDATA[<b>hi</b>]]></text>"), "svg: a CDATA section is not accepted");
        has(&undrawn("<circle r='5'/><![CDATA[x]]>"), "svg: a CDATA section is not accepted");
    }

    #[test]
    fn the_document_is_one_svg_root_with_nothing_but_white_space_and_comments_around_it() {
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg><svg><circle r='5'/></svg>" })), "svg: a second root element \"svg\"");
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg><script>alert(1)</script>" })), "svg: a second root element \"script\"");
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg> trailing" })), "svg: text \"trailing\" outside the svg element");
        has(&refused(None, json!({ "svg": "leading <svg><circle r='5'/></svg>" })), "svg: text \"leading\" outside the svg element");
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg>&amp;" })), "outside a text element");
        has(&refused(None, json!({ "svg": "<div><svg><circle r='5'/></svg></div>" })), "svg: the root element is \"div\"");
        has(&refused(None, json!({ "svg": "<SVG><circle r='5'/></SVG>" })), "svg: the root element is \"SVG\"");
        has(&refused(None, json!({ "svg": "<svg:svg><circle r='5'/></svg:svg>" })), "svg: the root element is \"svg:svg\"");
        has(&refused(None, json!({ "svg": "" })), "svg: no svg element");
        has(&refused(None, json!({ "svg": "  \n <!-- nothing --> " })), "svg: no svg element");
        has(&refused(None, json!({ "svg": "<svg/>" })), "svg: nothing to draw");
        has(&refused(None, json!({ "svg": "<svg><defs/></svg>" })), "svg: nothing to draw");
    }

    #[test]
    fn a_document_that_is_not_well_formed_or_ends_inside_an_element_refuses() {
        has(&refused(None, json!({ "svg": "<svg viewBox='0 0 200 200'><g id='a'><circle r='5'/>" })), "svg ends inside <g>");
        has(&refused(None, json!({ "svg": "<svg viewBox='0 0 200 200'><circle r='5'/>" })), "svg ends inside <svg>");
        has(&refused(None, json!({ "svg": "<svg viewBox='0 0 200 200'><circle r='5'" })), "svg: a tag, comment or section is not closed");
        has(&refused(None, json!({ "svg": "<svg><!-- never closed <circle r='5'/></svg>" })), "svg: a tag, comment or section is not closed");
        has(&refused(None, json!({ "svg": "<svg><g><circle r='5'/></svg>" })), "svg: the end tag \"svg\" closes \"g\"");
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg></g>" })), "svg: the end tag \"g\" has no start tag");
        has(&refused(None, json!({ "svg": "<svg><text>a & b</text></svg>" })), "svg: an & without its ;");
        let place = refused(None, json!({ "svg": "<svg>\n  <g>\n    <circle r='5'/>\n  </svg>" }));
        has(&place, "(at line 4 column 3)");
    }

    #[test]
    fn names_are_compared_as_written_so_prefixed_names_refuse() {
        has(&undrawn("<svg:script>alert(1)</svg:script>"), "svg: the prefixed element \"svg:script\"");
        has(&undrawn("<svg:circle r='5'/>"), "svg: the prefixed element \"svg:circle\"");
        has(&undrawn("<circle r='5' x:onload='alert(1)'/>"), "svg: the prefixed attribute \"x:onload\"");
        has(&undrawn("<circle r='5' x:fill='red'/>"), "svg: the prefixed attribute \"x:fill\"");
        has(&undrawn("<circle r='5' svg:r='9'/>"), "svg: the prefixed attribute \"svg:r\"");
        has(&undrawn("<circle r='5' xlink:href='#a'/>"), "svg: xlink:href on <circle>");
        // The exceptions are ignored, wherever they stand.
        let look = drawn("<g id='g' xmlns:xlink='http://www.w3.org/1999/xlink' xml:space='preserve' xml:lang='en'><circle r='5'/></g>");
        assert!(node(&look, "g").attrs.is_empty());
        has(&undrawn("<g xmlns='http://www.w3.org/1999/xhtml'><circle r='5'/></g>"), "\"xmlns\" is not an attribute of <g>");
        has(&undrawn("<Circle r='5'/>"), "svg: the element \"Circle\" is not supported");
        has(&undrawn("<circle R='5'/>"), "svg: \"R\" is not an attribute of <circle>");
    }

    #[test]
    fn every_attribute_error_refuses() {
        has(&undrawn("<circle r='5' r='6'/>"), "the attributes of <circle> are not well-formed, a name is given twice");
        has(&undrawn("<circle r=5/>"), "the attributes of <circle> are not well-formed, a value has no quotes");
        has(&undrawn("<circle r='5' hidden/>"), "the attributes of <circle> are not well-formed, a name has no value");
        has(&undrawn("<circle r='5' fill=/>"), "the attributes of <circle> are not well-formed");
        has(&refused(None, json!({ "svg": "<svg viewBox='0 0 200 200' viewBox='0 0 1 1'><circle r='5'/></svg>" })), "the attributes of <svg> are not well-formed, a name is given twice");
        // Either quote is fine.
        drawn("<circle r=\"5\" fill='red'/>");
    }

    #[test]
    fn references_are_decoded_before_a_value_is_validated() {
        // url( spelt with a numeric reference is still a url that reaches outside.
        has(&undrawn("<circle r='5' fill='&#x75;rl(http://x)'/>"), "svg: fill \"url(http://x)\" reaches outside the artwork");
        has(&undrawn("<circle r='5' fill='&#117;&#114;&#108;(#a)'/>"), "svg: fill refers to #a, which is not in this svg");
        // The five entities and numeric references are the whole set.
        let look = drawn("<text id='t'>a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos; &#65;&#x42;</text><circle id='c' r='&#53;'/>");
        assert_eq!(node(&look, "t").text.as_deref(), Some("a & b <c> \"d\" 'e' AB"));
        assert_eq!(attrs(node(&look, "c")), json!({ "r": 5 }));
        has(&undrawn("<circle r='5' fill='&nbsp;'/>"), "svg: \"fill\" holds a reference that is not known");
        has(&undrawn("<text>&nbsp;</text>"), "svg: the reference \"nbsp\" is not known");
        has(&undrawn("<text>&#0;</text>"), "svg: the reference \"#0\" is not known");
        has(&undrawn("<text>&#xD800;</text>"), "is not known");
        // In a text, what a reference decodes to is cleaned like any other text.
        let look = drawn("<text id='a'>a&#x202E;b</text><text id='b'>a&#xE0041;b</text><text id='c'>a&#27;[31mb</text><text id='d'>a&#10;&#9;b</text><text id='e'>a\u{202E}b\u{200B}c</text>");
        assert_eq!(node(&look, "a").text.as_deref(), Some("ab"));
        assert_eq!(node(&look, "b").text.as_deref(), Some("ab"));
        assert_eq!(node(&look, "c").text.as_deref(), Some("a[31mb"));
        assert_eq!(node(&look, "d").text.as_deref(), Some("a b"));
        assert_eq!(node(&look, "e").text.as_deref(), Some("abc"));
        // In an attribute, a control character is no part of any value.
        has(&undrawn("<circle id='a&#27;b' r='5'/>"), "svg: the id \"a\\u{1b}b\"");
        has(&undrawn("<circle r='5' fill='red&#x202E;'/>"), "is not a colour");
    }

    #[test]
    fn text_outside_a_text_element_refuses_and_white_space_is_ignored() {
        drawn("\n  <g>\n    <circle r='5'/>\n  </g>\n");
        has(&undrawn("<g>hello</g>"), "svg: text \"hello\" outside a text element");
        has(&undrawn("hello<circle r='5'/>"), "svg: text \"hello\" outside a text element");
        has(&undrawn("<circle r='5'>x</circle>"), "outside a text element");
        has(&undrawn("<g>&amp;</g>"), "svg: text \"amp\" outside a text element");
    }

    // ------------------------------------------------------------ refused: the hostile corpus

    #[test]
    fn a_hostile_corpus_is_refused_with_a_message_for_each() {
        let deep = format!("{}<circle r='5'/>{}", "<g>".repeat(MAX_DEPTH + 1), "</g>".repeat(MAX_DEPTH + 1));
        let long_d = format!("<path d='M0 0{}'/>", " L1 1".repeat(900));
        for (inner, why) in [
            ("<script>alert(1)</script>", "svg: <script> is not supported"),
            ("<script href='http://x/a.js'/>", "svg: <script> is not supported"),
            ("<circle r='5' onclick='alert(1)'/>", "svg: the event attribute \"onclick\""),
            ("<circle r='5' ONLOAD='alert(1)'/>", "svg: the event attribute \"ONLOAD\""),
            ("<g onmouseover='x'><circle r='5'/></g>", "svg: the event attribute \"onmouseover\""),
            ("<circle r='5' onbegin='x'/>", "svg: the event attribute \"onbegin\""),
            ("<a href='javascript:alert(1)'><circle r='5'/></a>", "svg: <a> is not supported"),
            ("<circle r='5' href='javascript:alert(1)'/>", "svg: href on <circle>"),
            ("<use href='#a'/>", "svg: href refers to #a, which is not in this svg"),
            ("<use xlink:href='http://x/y.svg#a'/>", "svg: href \"http://x/y.svg#a\" reaches outside the artwork"),
            ("<use href='data:image/svg+xml,x'/>", "reaches outside the artwork"),
            ("<use href='javascript:alert(1)'/>", "reaches outside the artwork"),
            ("<image href='http://x/a.png' width='5' height='5'/>", "svg: <image> is not supported"),
            ("<style>* { fill: red } @import url(http://x);</style>", "svg: <style> is not supported"),
            ("<circle r='5' style='fill:url(http://x/a.svg#b)'/>", "style holds a url()"),
            ("<circle r='5' style='fill:red !important'/>", "style holds !important"),
            ("<circle r='5' style='/**/fill:red'/>", "style holds a comment"),
            ("<circle r='5' style='f\\69ll:red'/>", "style holds an escape"),
            ("<foreignObject><div xmlns='http://www.w3.org/1999/xhtml'><img src='x' onerror='alert(1)'/></div></foreignObject>", "svg: <foreignObject> is not supported"),
            ("<iframe src='http://x'/>", "svg: <iframe> is not supported"),
            ("<mask id='m'><circle r='5'/></mask>", "svg: <mask> inside <svg>: masks and filters belong inside defs"),
            ("<circle r='5' mask='url(#m)'/>", "svg: mask refers to #m, which is not in this svg"),
            ("<circle r='5' mask='url(http://x/m.svg#m)'/>", "svg: mask \"url(http://x/m.svg#m)\""),
            ("<circle r='5' filter='url(http://x/f.svg#f)'/>", "svg: filter \"url(http://x/f.svg#f)\""),
            ("<defs><filter id='f'><feImage href='http://x/a.png'/></filter></defs><circle r='5' filter='url(#f)'/>", "svg: <feImage> is not supported: images are not accepted"),
            ("<pattern id='p'/>", "svg: <pattern> is not supported"),
            ("<symbol id='s'/>", "svg: <symbol> is not supported"),
            ("<marker id='m'/>", "svg: <marker> is not supported"),
            ("<switch><circle r='5'/></switch>", "svg: <switch> is not supported"),
            ("<svg><circle r='5'/></svg>", "a nested svg is not supported"),
            ("<feImage href='http://x'/>", "svg: <feImage> is not supported: images are not accepted"),
            ("<video src='x'/>", "svg: the element \"video\" is not supported"),
            ("<animateColor/>", "svg: the element \"animateColor\" is not supported"),
            (deep.as_str(), "svg: elements nested deeper than 10"),
            ("<circle r='1e999'/>", "beyond any size"),
            ("<circle r='NaN'/>", "is not a number"),
            ("<text font-size='99999'>a</text>", "font-size 99999 is out of range"),
            ("<circle r='5' stroke-width='99999'/>", "stroke-width 99999 is out of range"),
            ("<circle r='5' transform='scale(99999)'/>", "the transform scale() is out of range"),
            (long_d.as_str(), "characters of path data: at most 4000"),
            ("<circle id='a' r='5'/><circle id='a' r='5'/>", "the id a is used twice"),
            ("<circle r='5' fill='url(#nope)'/>", "svg: fill refers to #nope, which is not in this svg"),
            ("<circle r='5' clip-path='url(#nope)'/>", "svg: clip-path refers to #nope, which is not in this svg"),
            ("<defs><clipPath id='a'><circle r='5' clip-path='url(#a)'/></clipPath></defs><circle r='5'/>", "svg: clip-path inside a clipPath"),
            ("<defs><clipPath id='a' clip-path='url(#a)'><circle r='5'/></clipPath></defs><circle r='5'/>", "\"clip-path\" is not an attribute of <clipPath>"),
            ("<circle r='5' fill='url(http://x/a.svg#g)'/>", "reaches outside the artwork"),
            ("<circle r='5' fill='url(javascript:alert(1))'/>", "reaches outside the artwork"),
            ("<circle r='5' stroke=\"url('data:image/svg+xml,x')\"/>", "reaches outside the artwork"),
            ("<circle r='5' fill='url(#a) red'/>", "svg: fill \"url(#a) red\" is more than a reference"),
            ("<circle r='5' fill='expression(alert(1))'/>", "is not a colour"),
            ("<circle r='5' src='x'/>", "\"src\" is not an attribute of <circle>"),
            ("<circle r='5' formaction='x'/>", "\"formaction\" is not an attribute of <circle>"),
            ("<circle r='5' xmlns:xlink='x' xlink:actuate='onLoad'/>", "the prefixed attribute \"xlink:actuate\""),
            ("<text>a<script>x</script></text>", "svg: \"script\" inside a text"),
        ] {
            has(&undrawn(inner), why);
        }
    }

    #[test]
    fn a_reference_resolves_to_an_element_of_the_right_kind_in_the_same_document() {
        let ok = "<defs><linearGradient id='lg'><stop offset='0' stop-color='red'/></linearGradient><clipPath id='cp'><circle r='5'/></clipPath></defs>";
        // A reference may come before what it names.
        let look = drawn(&format!("<circle id='c' r='5' fill='url(#lg)' stroke=\"url( '#lg' )\" clip-path='url(#cp)'/>{ok}"));
        assert_eq!(attrs(node(&look, "c")), json!({ "r": 5, "fillRef": "lg", "strokeRef": "lg", "clipPathRef": "cp" }));
        assert_eq!(attrs(node(&drawn("<circle id='c' r='5' clip-path='none'/>"), "c")), json!({ "r": 5 }));
        has(&undrawn(&format!("{ok}<circle r='5' fill='url(#cp)'/>")), "svg: fill refers to #cp, a clipPath: url(#id) of a gradient in this svg");
        has(&undrawn(&format!("{ok}<circle r='5' clip-path='url(#lg)'/>")), "svg: clip-path refers to #lg, a linearGradient: url(#id) of a clipPath in this svg");
        has(&undrawn("<circle id='c' r='5' fill='url(#c)'/>"), "svg: fill refers to #c, a circle");
        has(&undrawn(&format!("{ok}<circle r='5' clip-path='lg'/>")), "svg: clip-path \"lg\"");
        has(&undrawn(&format!("{ok}<circle r='5' fill='url(#1x)'/>")), "reaches outside the artwork");
    }

    #[test]
    fn the_content_model_is_enforced() {
        for (inner, why) in [
            ("<stop offset='0'/>", "svg: <stop> inside <svg>: a stop belongs inside a gradient"),
            ("<g><stop offset='0'/></g>", "svg: <stop> inside <g>"),
            ("<linearGradient id='a'/>", "svg: <linearGradient> inside <svg>: gradients and clip paths belong inside defs"),
            ("<g><clipPath id='a'/></g>", "svg: <clipPath> inside <g>"),
            ("<defs><use href='#a'/></defs><circle id='a' r='5'/>", "svg: <use> inside <defs>: defs holds gradients, clip paths, masks, filters, and g, shapes and text to use"),
            ("<defs><clipPath id='a'><g><circle r='5'/></g></clipPath></defs>", "svg: <g> inside <clipPath>: a clipPath holds shapes only"),
            ("<defs><clipPath id='a'><text>a</text></clipPath></defs>", "svg: <text> inside <clipPath>"),
            ("<defs><linearGradient id='a'><circle r='5'/></linearGradient></defs>", "svg: <circle> inside <linearGradient>: a gradient holds stops only"),
            ("<defs><linearGradient id='a'><stop offset='0'><stop offset='1'/></stop></linearGradient></defs>", "svg: <stop> inside <stop>"),
            ("<circle r='5'><g/></circle>", "svg: <g> inside <circle>: shapes, text and stops have no element children"),
            ("<path d='M0 0L1 1'><circle r='5'/></path>", "svg: <circle> inside <path>"),
            ("<defs><defs/></defs>", "svg: <defs> inside <defs>"),
        ] {
            has(&undrawn(inner), why);
        }
        // defs may stand inside a group, and a gradient may be empty.
        drawn("<g><defs><radialGradient id='a'/></defs><circle r='5' fill='url(#a)'/></g>");
        // defs holds what a use repeats, which stays inert there.
        drawn("<defs><g id='a'><circle r='5'/></g><circle id='b' r='5'/><text id='c'>a</text></defs><use href='#a'/>");
    }

    // ------------------------------------------------------------ every limit

    #[test]
    fn every_limit_of_the_artwork_is_enforced() {
        // The size of the svg text.
        let big = format!("<svg><!--{}--><circle r='5'/></svg>", "x".repeat(MAX_SVG_BYTES));
        has(&refused(None, json!({ "svg": big })), "svg is 32.0 KiB: at most 32 KiB");
        // Elements: 300 are fine, the 301st refuses.
        drawn(&"<circle r='5'/>".repeat(MAX_ELEMENTS));
        has(&undrawn(&"<circle r='5'/>".repeat(MAX_ELEMENTS + 1)), "svg: more than 300 elements");
        // Depth: 10 below the root are fine.
        let nest = |n: usize| format!("{}<circle r='5'/>{}", "<g>".repeat(n - 1), "</g>".repeat(n - 1));
        drawn(&nest(MAX_DEPTH));
        has(&undrawn(&nest(MAX_DEPTH + 1)), "svg: elements nested deeper than 10");
        // The group that wraps the content for the root's attributes is one level and one element.
        let wrapped = |inner: &str| json!({ "svg": format!("<svg fill='red'>{inner}</svg>") });
        accepted(None, wrapped(&nest(MAX_DEPTH - 1)));
        has(&refused(None, wrapped(&nest(MAX_DEPTH))), "nested deeper than 10");
        accepted(None, wrapped(&"<circle r='5'/>".repeat(MAX_ELEMENTS - 1)));
        has(&refused(None, wrapped(&"<circle r='5'/>".repeat(MAX_ELEMENTS))), "more than 300 elements");
        // Attributes on one element, dropped ones included.
        let with_attrs = |n: usize| format!("<circle {}/>", (0..n).map(|i| format!("data-a{i}='1'")).collect::<Vec<_>>().join(" "));
        drawn(&with_attrs(MAX_ATTRS));
        has(&undrawn(&with_attrs(MAX_ATTRS + 1)), "svg: more than 32 attributes on <circle>");
        // Stops in one gradient.
        let gradient = |n: usize| format!("<defs><linearGradient id='a'>{}</linearGradient></defs><circle r='5'/>", "<stop offset='0' stop-color='red'/>".repeat(n));
        drawn(&gradient(MAX_STOPS));
        has(&undrawn(&gradient(MAX_STOPS + 1)), "svg: more than 16 stops in a gradient");
        // Text elements, and the characters of one.
        drawn(&"<text>a</text>".repeat(MAX_TEXTS));
        has(&undrawn(&"<text>a</text>".repeat(MAX_TEXTS + 1)), "svg: more than 6 text elements");
        drawn(&format!("<text>{}</text>", "é".repeat(MAX_TEXT_CHARS)));
        has(&undrawn(&format!("<text>{}</text>", "é".repeat(MAX_TEXT_CHARS + 1))), "svg: a text has 41 characters: at most 40");
        // Path data and points, as written and as stored.
        let d = |n: usize| format!("<path d='M0 0{}'/>", " L1 1".repeat(n));
        drawn(&d(570));
        has(&undrawn(&d(800)), "4004 characters of path data: at most 4000");
        // Written within the limit, but over it once every token stands apart.
        has(&undrawn(&format!("<path d='M0 0{}'/>", "L1-1".repeat(900))), "characters of path data once it is spaced out: at most 4000");
        let points = |n: usize| format!("<polygon points='{}'/>", "1 1 ".repeat(n));
        drawn(&points(MAX_POINTS_CHARS / 4));
        has(&undrawn(&points(MAX_POINTS_CHARS / 4 + 1)), "2004 characters of points: at most 2000");
        // One transform: its functions, and its characters as written and as stored.
        drawn(&format!("<circle r='5' transform='{}'/>", "scale(1) ".repeat(MAX_TRANSFORMS)));
        has(&undrawn(&format!("<circle r='5' transform='scale(1){}'/>", " ".repeat(MAX_TRANSFORM_CHARS))), "a transform of 1032 characters: at most 1024");
        // About the longest transform that can be stored: well inside what the web check reads.
        let widest = format!("<svg viewBox='-4096 -4096 4096 4096'><circle r='5' transform='{}'/></svg>", "matrix(-999.999 -999.999 999.999 -999.999 -16383.999 -16383.999) ".repeat(MAX_TRANSFORMS));
        let stored = accepted(None, json!({ "svg": widest })).design.look.unwrap();
        assert_eq!(stored.nodes[0].attrs["transform"].str().unwrap().len(), 519);
        has(&undrawn(&format!("<circle r='5' transform='{}'/>", "scale(1) ".repeat(MAX_TRANSFORMS + 1))), "a transform of more than 8 functions");
    }

    // ------------------------------------------------------------ boxes

    #[test]
    fn every_shape_kind_has_its_box() {
        let look = drawn(
            "<circle id='c' cx='50' cy='60' r='10'/><ellipse id='e' cx='50' cy='60' rx='10' ry='5'/><rect id='r' x='10' y='20' width='30' height='40'/>\
             <line id='l' x1='10' y1='20' x2='40' y2='5'/><polyline id='pl' points='0,0 10,20 5,8'/><polygon id='pg' points='30 30 10 50 60 40'/>\
             <path id='p' d='M10 10 L50 30 Q80 0 60 60 Z'/><path id='rel' d='M10 10 h20 v30 l-5 -5'/><path id='cubic' d='M0 0 C10 20 30 20 40 0'/>\
             <path id='smooth' d='M0 20 Q10 30 20 20 T40 20'/><path id='ss' d='M0 50 C0 60 10 60 10 50 S20 40 20 50'/><path id='two' d='M10 10 L20 20 M50 50 L60 40 Z'/>\
             <text id='t' x='100' y='50' font-size='10'>abcd</text><text id='tm' x='100' y='50' font-size='10' text-anchor='middle' dominant-baseline='middle'>abcd</text>\
             <text id='te' x='100' y='50' font-size='20' text-anchor='end' dominant-baseline='hanging' letter-spacing='2'>ab</text><text id='plain' x='10' y='20'>a</text>",
        );
        assert_eq!(bbox(&look, "c"), [40.0, 50.0, 20.0, 20.0]);
        assert_eq!(bbox(&look, "e"), [40.0, 55.0, 20.0, 10.0]);
        assert_eq!(bbox(&look, "r"), [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(bbox(&look, "l"), [10.0, 5.0, 30.0, 15.0]);
        assert_eq!(bbox(&look, "pl"), [0.0, 0.0, 10.0, 20.0]);
        assert_eq!(bbox(&look, "pg"), [10.0, 30.0, 50.0, 20.0]);
        // A path: every point, control points included.
        assert_eq!(bbox(&look, "p"), [10.0, 0.0, 70.0, 60.0]);
        assert_eq!(bbox(&look, "rel"), [10.0, 10.0, 20.0, 30.0]);
        assert_eq!(bbox(&look, "cubic"), [0.0, 0.0, 40.0, 20.0]);
        // T mirrors the control point of the Q before it, S the second one of the C before it.
        assert_eq!(bbox(&look, "smooth"), [0.0, 10.0, 40.0, 20.0]);
        assert_eq!(bbox(&look, "ss"), [0.0, 40.0, 20.0, 20.0]);
        // A second subpath starts where its M says, not where the pen was.
        assert_eq!(bbox(&look, "two"), [10.0, 10.0, 50.0, 40.0]);
        // A text is estimated: 0.6 of the font size per character, 0.8 of it above the baseline.
        assert_eq!(bbox(&look, "t"), [100.0, 42.0, 24.0, 10.0]);
        assert_eq!(bbox(&look, "tm"), [88.0, 45.0, 24.0, 10.0]);
        assert_eq!(bbox(&look, "te"), [74.0, 50.0, 26.0, 20.0]);
        assert_eq!(bbox(&look, "plain"), [10.0, 7.2, 9.6, 16.0]);
    }

    #[test]
    fn the_box_of_an_arc_is_its_real_extent() {
        let look = drawn(
            "<path id='ring' d='M40 100 A60 60 0 1 1 160 100 A60 60 0 1 1 40 100 Z'/><path id='top' d='M40 100 A60 60 0 0 1 160 100'/>\
             <path id='quarter' d='M100 50 A50 50 0 0 1 150 100'/><path id='rest' d='M100 50 A50 50 0 1 0 150 100'/>\
             <path id='oval' d='M60 100 a40 20 0 1 0 80 0 a40 20 0 1 0 -80 0'/><path id='tilted' d='M100 80 A20 40 90 1 1 100 120 A20 40 90 1 1 100 80'/>\
             <path id='small' d='M0 100 A10 10 0 0 1 100 100'/><path id='flat' d='M10 10 A0 5 0 0 1 50 30'/><path id='still' d='M10 10 A5 5 0 0 1 10 10'/>",
        );
        assert_eq!(bbox(&look, "ring"), [40.0, 40.0, 120.0, 120.0]);
        assert_eq!(bbox(&look, "top"), [40.0, 40.0, 120.0, 60.0]);
        assert_eq!(bbox(&look, "quarter"), [100.0, 50.0, 50.0, 50.0]);
        // The long way round the same circle: everything but that quarter.
        assert_eq!(bbox(&look, "rest"), [50.0, 50.0, 100.0, 100.0]);
        assert_eq!(bbox(&look, "oval"), [60.0, 80.0, 80.0, 40.0]);
        // Turned a quarter, the long axis lies flat.
        assert_eq!(bbox(&look, "tilted"), [60.0, 80.0, 80.0, 40.0]);
        // Radii too small for the distance are scaled up, as a browser does.
        assert_eq!(bbox(&look, "small"), [0.0, 50.0, 100.0, 50.0]);
        // A radius of zero is a straight line; equal end points draw nothing.
        assert_eq!(bbox(&look, "flat"), [10.0, 10.0, 40.0, 20.0]);
        assert_eq!(bbox(&look, "still"), [10.0, 10.0, 0.0, 0.0]);
    }

    #[test]
    fn a_group_spans_its_children_through_their_transforms() {
        let look = drawn(
            "<g id='outer'><g id='inner' transform='translate(100 50)'><rect id='r' x='0' y='0' width='10' height='20'/></g><circle cx='10' cy='10' r='5'/></g>\
             <g id='turned'><rect id='tr' transform='rotate(90)' x='0' y='0' width='10' height='20'/></g>\
             <g id='scaled' transform='scale(2)'><g id='half' transform='translate(10 10) scale(0.5)'><circle id='dot' cx='20' cy='20' r='10'/></g></g>\
             <g id='empty'/><defs><clipPath id='clip'><rect x='5' y='5' width='10' height='10'/><circle cx='30' cy='30' r='5'/></clipPath><linearGradient id='grad'/></defs>",
        );
        // A box is in the coordinates the element's own geometry is written in: its own transform is not in it.
        assert_eq!(bbox(&look, "r"), [0.0, 0.0, 10.0, 20.0]);
        assert_eq!(bbox(&look, "inner"), [0.0, 0.0, 10.0, 20.0]);
        assert_eq!(bbox(&look, "outer"), [5.0, 5.0, 105.0, 65.0]);
        assert_eq!(bbox(&look, "tr"), [0.0, 0.0, 10.0, 20.0]);
        assert_eq!(bbox(&look, "turned"), [-20.0, 0.0, 20.0, 10.0]);
        assert_eq!(bbox(&look, "dot"), [10.0, 10.0, 20.0, 20.0]);
        assert_eq!(bbox(&look, "half"), [10.0, 10.0, 20.0, 20.0]);
        assert_eq!(bbox(&look, "scaled"), [15.0, 15.0, 10.0, 10.0]);
        assert_eq!(bbox(&look, "empty"), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(bbox(&look, "clip"), [5.0, 5.0, 30.0, 30.0]);
        assert_eq!(bbox(&look, "grad"), [0.0, 0.0, 0.0, 0.0]);
        // An element without an id has no box.
        assert!(node(&look, "outer").children[1].bbox.is_none());

        // For the author the parts are reported in root coordinates, through every transform.
        let all = parts(&look);
        let part = |id: &str| all.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no part {id}"));
        assert_eq!(part("dot").bbox, [30.0, 30.0, 20.0, 20.0]);
        assert_eq!(part("half").bbox, [30.0, 30.0, 20.0, 20.0]);
        assert_eq!(part("scaled").bbox, [30.0, 30.0, 20.0, 20.0]);
        assert_eq!(part("inner").bbox, [100.0, 50.0, 10.0, 20.0]);
        assert_eq!(part("tr").bbox, [-20.0, 0.0, 20.0, 10.0]);
        assert_eq!((part("dot").pivot, part("dot").pivot_given), ([40.0, 40.0], false));
        // Clip paths and gradients are not parts.
        assert_eq!(all.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["outer", "inner", "r", "turned", "tr", "scaled", "half", "dot", "empty"]);
    }

    // ------------------------------------------------------------ pivots, roles

    #[test]
    fn pivots_are_points_of_parts_and_the_answer_reports_them_in_root_coordinates() {
        let applied = accepted(None, figure(json!({ "pivots": { "head": [100, 110], "arm": [146.0004, 100] }, "roles": { "body": "body" } })));
        let look = applied.design.look.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&look.pivots).unwrap(), json!({ "arm": [146, 100], "head": [100, 110] }));
        let part = |id: &str| applied.parts.iter().find(|p| p.id == id).unwrap();
        assert_eq!((part("head").pivot, part("head").pivot_given), ([100.0, 110.0], true));
        // No pivot: the centre of the box; for the part that plays the body, the bottom centre.
        assert_eq!((part("face").pivot, part("face").pivot_given), ([100.0, 80.0], false));
        assert_eq!((part("body").bbox, part("body").pivot, part("body").pivot_given), ([60.0, 50.0, 92.0, 142.0], [106.0, 192.0], false));
        assert_eq!(part("arm").to_string(), "arm rect 140 100 12 40 pivot 146 100");
        assert_eq!(part("face").to_string(), "face circle 70 50 60 60 pivot 100 80 (default)");
        assert_eq!(applied.parts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["body", "belly", "head", "face", "eyes", "pupil", "mouth", "arm", "cane", "label"]);
        // A pivot under a transform is given in the part's own coordinates and reported at the root.
        let moved = accepted(None, json!({ "svg": svg("<g transform='translate(50 50)'><g id='arm' transform='scale(2)'><rect width='10' height='10'/></g></g>"), "pivots": { "arm": [5, 0] } }));
        assert_eq!(moved.parts[0].pivot, [60.0, 50.0]);

        has(&refused(None, figure(json!({ "pivots": { "elbow": [1, 2] } }))), "target \"elbow\" not found (at pivots.elbow): ids are body, belly, head, face, eyes, pupil, mouth, arm, cane, label");
        has(&refused(None, figure(json!({ "pivots": { "sky": [1, 2] } }))), "sky is a linearGradient, or inside defs (at pivots.sky): what moves is a g, a shape, a text or a use that is drawn");
        has(&refused(None, figure(json!({ "pivots": { "pane": [1, 2] } }))), "pane stands inside defs, a mask or a clipPath, where nothing moves (at pivots.pane): move the masked or clipped part, or the use that repeats it");
        has(&refused(None, figure(json!({ "pivots": { "arm": [1] } }))), "a pivot is a list: [x, y], in the coordinates the part is drawn in");
        has(&refused(None, figure(json!({ "pivots": { "arm": "top" } }))), "a pivot is a string");
        has(&refused(None, figure(json!({ "pivots": { "arm": [1, "2"] } }))), "a pivot is a list");
        has(&refused(None, figure(json!({ "pivots": { "arm": [99999, 0] } }))), "a pivot is out of range: -800 to 1000");
        has(&refused(None, figure(json!({ "pivots": [1, 2] }))), "pivots is a list: an object of ids with [x, y] each, null to remove one");
        has(&refused(None, json!({ "svg": svg("<circle cx='50' cy='50' r='5'/>"), "pivots": { "c": [1, 2] } })), "target \"c\" not found (at pivots.c): the artwork has no ids, give every part that moves an id");
    }

    #[test]
    fn roles_name_parts_and_decide_which_built_in_motion_applies() {
        let look = accepted(None, figure(json!({ "roles": { "body": "body", "head": "head", "eyes": "eyes", "pupils": "pupil", "armR": "arm", "armL": "cane" } }))).design.look.unwrap();
        assert_eq!(serde_json::to_string(&look.roles).unwrap(), r#"{"body":"body","head":"head","eyes":"eyes","pupils":"pupil","armL":"cane","armR":"arm"}"#);
        assert_eq!(builtins(&look), BUILTIN);
        let only = |roles: Value| builtins(&accepted(None, figure(json!({ "roles": roles }))).design.look.unwrap());
        assert_eq!(only(json!({})), [""; 0]);
        assert_eq!(only(json!({ "body": "body" })), ["breathe", "talk", "listen", "alert", "cheer", "nod", "shake"]);
        assert_eq!(only(json!({ "eyes": "eyes" })), ["blink"]);
        assert_eq!(only(json!({ "head": "head" })), ["think", "talk", "listen", "nod", "shake"]);
        assert_eq!(only(json!({ "pupils": "pupil" })), ["think"]);
        assert_eq!(only(json!({ "armL": "arm" })), ["wave"]);
        assert_eq!(only(json!({ "armL": "arm", "armR": "cane" })), ["wave", "cheer"]);
        assert_eq!(only(json!({ "eyes": "eyes", "body": "body" })), ["breathe", "blink", "talk", "listen", "alert", "doze", "cheer", "nod", "shake"]);
        // One part may play two roles.
        accepted(None, figure(json!({ "roles": { "eyes": "eyes", "pupils": "eyes" } })));

        has(&refused(None, figure(json!({ "roles": { "tail": "arm" } }))), "\"tail\" is not a role (at roles.tail): roles are body, head, eyes, pupils, armL, armR");
        has(&refused(None, figure(json!({ "roles": { "head": "nope" } }))), "target \"nope\" not found (at roles.head)");
        has(&refused(None, figure(json!({ "roles": { "head": 5 } }))), "a role is a number: the id of the part that plays it");
        has(&refused(None, figure(json!({ "roles": { "head": "sky" } }))), "sky is a linearGradient, or inside defs");
        has(&refused(None, figure(json!({ "roles": "head" }))), "roles is a string: an object of roles with the id of a part each, null to remove one");
    }

    #[test]
    fn pupils_think_only_with_a_gaze_above_0() {
        let look = |design: Value| accepted(None, figure(design)).design.look.unwrap();
        // Without a head, gaze 0 keeps the pupils still: no think, and a state that names it is warned.
        assert_eq!(builtins(&look(json!({ "roles": { "pupils": "pupil" }, "gaze": 0 }))), [""; 0]);
        assert_eq!(builtins(&look(json!({ "roles": { "pupils": "pupil" }, "gaze": 0.5 }))), ["think"]);
        assert_eq!(builtins(&look(json!({ "roles": { "pupils": "pupil", "head": "head" }, "gaze": 0 }))), ["think", "talk", "listen", "nod", "shake"]);
        let warned = |design: Value| accepted(None, figure(design)).warnings;
        // It names the pupils: the advice is the head or a gaze, not the role it has.
        let think = "think is built in and needs the head role, or the pupils role and a gaze above 0 (at states.working[0]): name head, or give a gaze above 0";
        assert_eq!(warned(json!({ "roles": { "pupils": "pupil" }, "gaze": 0, "states": { "working": ["think"] } })), [think]);
        assert_eq!(warned(json!({ "roles": { "pupils": "pupil" }, "gaze": 2, "states": { "working": ["think"] } })), [""; 0]);
        // The gaze the look will store: one carried from the design before, and one this call clears.
        let still = accepted(None, figure(json!({ "roles": { "pupils": "pupil" }, "gaze": 0 }))).design;
        assert_eq!(accepted(Some(&still), json!({ "states": { "working": ["think"] } })).warnings, [think]);
        assert_eq!(accepted(Some(&still), json!({ "gaze": null, "states": { "working": ["think"] } })).warnings, [""; 0]);
    }

    // ------------------------------------------------------------ animations

    fn rotate(target: &str) -> Value {
        json!({ "duration": 1, "tracks": [{ "target": target, "prop": "rotate", "values": [0, 10, 0] }] })
    }

    #[test]
    fn an_animation_is_stored_in_its_normal_form() {
        let look = accepted(
            None,
            figure(json!({
                "pivots": { "head": [100, 110] },
                "animations": {
                    "nod": { "duration": 0.8, "tracks": [{ "target": "head", "prop": "translateY", "values": [0, 6.00049, 0] }] },
                    "sway": { "duration": 3, "loop": true, "times": [0, 0.25, 1], "ease": "inOut", "tracks": [
                        { "target": ["arm"], "prop": "rotate", "values": [-2.5, 2.5, -2.5], "phase": 0.25 },
                        { "target": ["arm", "cane"], "prop": "opacity", "values": [null, 0.5, null], "times": [0, 0.5, 1], "ease": ["in", "out"] }
                    ] },
                    "wink": { "duration": 0.2, "every": [2, 5.5], "loop": false, "tracks": [{ "target": "pupil", "prop": "r", "values": [4, 1, 4], "delay": 0.05 }] },
                    "flash": { "duration": 2, "tracks": [
                        { "target": "face", "prop": "fill", "values": ["red", "#00F", "transparent", null] },
                        { "target": "mouth", "prop": "d", "values": ["M84,96Q100,110,116,96", null] }
                    ] }
                }
            })),
        )
        .design
        .look
        .unwrap();
        assert_eq!(
            serde_json::to_value(&look.animations).unwrap(),
            json!({
                "flash": { "duration": 2, "tracks": [
                    { "target": "face", "prop": "fill", "values": ["#ff0000", "#0000ff", "#00000000", null] },
                    { "target": "mouth", "prop": "d", "values": ["M 84 96 Q 100 110 116 96", null] }
                ] },
                "nod": { "duration": 0.8, "tracks": [{ "target": "head", "prop": "translateY", "values": [0, 6, 0] }] },
                "sway": { "duration": 3, "loop": true, "times": [0, 0.25, 1], "ease": "inOut", "tracks": [
                    { "target": "arm", "prop": "rotate", "values": [-2.5, 2.5, -2.5], "phase": 0.25 },
                    { "target": ["arm", "cane"], "prop": "opacity", "values": [null, 0.5, null], "times": [0, 0.5, 1], "ease": ["in", "out"] }
                ] },
                "wink": { "duration": 0.2, "every": [2, 5.5], "tracks": [{ "target": "pupil", "prop": "r", "values": [4, 1, 4], "delay": 0.05 }] }
            })
        );
        // The typed form reads back from that JSON.
        let back: BTreeMap<String, Animation> = serde_json::from_value(serde_json::to_value(&look.animations).unwrap()).unwrap();
        assert_eq!(back, look.animations);
        assert!(look.animations["sway"].looping && !look.animations["nod"].looping);
    }

    #[test]
    fn a_track_names_parts_that_exist_and_a_prop_they_have() {
        let bad = |track: Value| refused(None, tracks(json!([track])));
        has(&bad(json!({ "target": "arm_r", "prop": "rotate", "values": [0, 1] })), "target \"arm_r\" not found (at animations.a.tracks[0]): ids are body, belly, head, face, eyes, pupil, mouth, arm, cane, label");
        has(&bad(json!({ "target": "sky", "prop": "opacity", "values": [0, 1] })), "sky is a linearGradient, or inside defs: what moves is a g, a shape, a text or a use that is drawn");
        has(&bad(json!({ "target": "pane", "prop": "opacity", "values": [0, 1] })), "pane stands inside defs, a mask or a clipPath, where nothing moves (at animations.a.tracks[0]): move the masked or clipped part");
        has(&bad(json!({ "target": "window", "prop": "opacity", "values": [0, 1] })), "window is a clipPath, or inside defs");
        has(&bad(json!({ "target": "arm", "prop": "d", "values": ["M0 0L1 1", "M0 0L2 2"] })), "arm is a rect and has no d: d is a prop of path");
        has(&bad(json!({ "target": "arm", "prop": "r", "values": [1, 2] })), "arm is a rect and has no r: r is a prop of circle");
        has(&bad(json!({ "target": "face", "prop": "x1", "values": [1, 2] })), "face is a circle and has no x1: x1 is a prop of line");
        has(&bad(json!({ "target": "label", "prop": "width", "values": [1, 2] })), "label is a text and has no width: width is a prop of rect");
        has(&bad(json!({ "target": "head", "prop": "cx", "values": [1, 2] })), "head is a g and has no cx: cx is a prop of circle and ellipse");
        has(&bad(json!({ "target": "cane", "prop": "rx", "values": [1, 2] })), "cane is a line and has no rx: rx is a prop of ellipse and rect");
        has(&bad(json!({ "target": "arm", "prop": "points", "values": [1, 2] })), "prop \"points\": points cannot be animated, draw a path and animate its d");
        has(&bad(json!({ "target": "arm", "prop": "transform", "values": [1, 2] })), "prop \"transform\" (at animations.a.tracks[0]): props are translateX, translateY, rotate, skewX, skewY, scale, scaleX, scaleY, opacity, fillOpacity");
        has(&bad(json!({ "target": "arm", "prop": 5, "values": [1, 2] })), "prop is a number");
        has(&bad(json!({ "target": "arm", "values": [1, 2] })), "a track without a prop");
        has(&bad(json!({ "prop": "rotate", "values": [1, 2] })), "a track without a target: target names a g, a shape, a text or a use by its id");
        has(&bad(json!({ "target": 5, "prop": "rotate", "values": [1, 2] })), "target is a number: an id, or a list of 1 to 16 ids");
        has(&bad(json!("arm")), "a track is a string: an object with target, prop and values");
        // Geometry on the shapes that have it; x and y also on a text.
        let applied = accepted(
            None,
            tracks(json!([
                { "target": "label", "prop": "y", "values": [190, 180, 190] },
                { "target": "arm", "prop": "height", "values": [40, 50, 40] },
                { "target": "cane", "prop": "x2", "values": [60, 70, 60] },
                { "target": "belly", "prop": "ry", "values": [30, 32, 30] },
                { "target": "face", "prop": "cx", "values": [100, 102, 100] }
            ])),
        );
        assert_eq!(applied.warnings, [""; 0]);
    }

    #[test]
    fn a_keyframe_has_the_type_and_the_range_of_its_prop() {
        let bad = |track: Value| refused(None, tracks(json!([track])));
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": ["a", 0] })), "a string for rotate (at animations.a.tracks[0].values[0]): a number, or null for the artwork's own value");
        has(&bad(json!({ "target": "arm", "prop": "opacity", "values": ["0.5", 1] })), "a string for opacity");
        has(&bad(json!({ "target": "mouth", "prop": "d", "values": [1, 2] })), "a number for d (2 times, first at animations.a.tracks[0].values[0]): path data as a string, or null");
        has(&bad(json!({ "target": "face", "prop": "fill", "values": [1, 2] })), "a number for fill: a colour as a string, or null");
        has(&bad(json!({ "target": "face", "prop": "fill", "values": ["url(#sky)", "red"] })), "\"url(#sky)\" is not a plain colour: #rrggbb, rgb(), hsl() or a CSS name, not none, currentColor or url()");
        has(&bad(json!({ "target": "face", "prop": "stroke", "values": ["none", "red"] })), "\"none\" is not a plain colour");
        has(&bad(json!({ "target": "face", "prop": "fill", "values": ["currentColor", "red"] })), "\"currentColor\" is not a plain colour");
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": [true, 0] })), "a boolean as a keyframe: a number, a string (a colour or path data) or null");
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": [[0], 0] })), "a list as a keyframe");
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": [0, 3601] })), "rotate 3601 is out of range: degrees, within 3600 either way");
        has(&bad(json!({ "target": "arm", "prop": "scale", "values": [1, 10.5] })), "scale 10.5 is out of range: -10 to 10");
        has(&bad(json!({ "target": "arm", "prop": "opacity", "values": [1, 1.5] })), "opacity 1.5 is out of range: 0 to 1");
        has(&bad(json!({ "target": "arm", "prop": "translateX", "values": [0, 801] })), "translateX 801 is out of range: within 800 either way");
        has(&bad(json!({ "target": "arm", "prop": "width", "values": [12, -1] })), "width -1 is out of range: 0 to 800");
        has(&bad(json!({ "target": "arm", "prop": "x", "values": [140, 1001] })), "x 1001 is out of range: -800 to 1000");
        has(&bad(json!({ "target": "arm", "prop": "strokeWidth", "values": [1, 900] })), "strokeWidth 900 is out of range: 0 to 800");
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": [0] })), "1 keyframes: 2 to 16 values");
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": vec![0; MAX_KEYFRAMES + 1] })), "17 keyframes: 2 to 16 values");
        has(&bad(json!({ "target": "arm", "prop": "rotate", "values": 5 })), "values is a number");
        has(&bad(json!({ "target": "arm", "prop": "rotate" })), "a track without values");
        // The ends of the ranges.
        accepted(
            None,
            tracks(json!([
                { "target": "arm", "prop": "rotate", "values": [-3600, 3600] },
                { "target": "arm", "prop": "scale", "values": [-10, 10] },
                { "target": "arm", "prop": "translateY", "values": [-800, 800] },
                { "target": "arm", "prop": "opacity", "values": [0, 1] },
                { "target": "arm", "prop": "rotate", "values": vec![0; MAX_KEYFRAMES] }
            ])),
        );
        // Colours of every accepted form are stored as hex.
        let look = accepted(None, figure(json!({ "animations": { "a": { "duration": 3, "tracks": [{ "target": "face", "prop": "stroke", "values": ["Tomato", "rgb(0 0 255 / 50%)", "hsl(120 100% 50%)", "transparent"] }] } } }))).design.look.unwrap();
        assert_eq!(serde_json::to_value(&look.animations["a"].tracks[0].values).unwrap(), json!(["#ff6347", "#0000ff80", "#00ff00", "#00000000"]));
    }

    #[test]
    fn d_keyframes_of_one_track_share_one_structure() {
        let bad = |values: Value| refused(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": values }])));
        has(&bad(json!(["M84 96 Q100 104 116 96", "M84 96 L100 104 L116 96"])), "d values differ in structure: MQ with 6 numbers against MLL with 6 (at animations.a.tracks[0].values[1]): same commands, same count of numbers, same arc flags");
        has(&bad(json!(["M84 96 Q100 104 116 96", "M84 96 Q100 104 116 96 Z"])), "MQ with 6 numbers against MQZ with 6 (");
        has(&bad(json!(["M84 96 Q100 104 116 96", "M84 96 q100 104 116 96"])), "MQ with 6 numbers against Mq with 6 (");
        has(&bad(json!(["M0 0 L1 1 2 2", "M0 0 L1 1 L2 2"])), "ML with 6 numbers against MLL with 6 (");
        has(&bad(json!(["M0 0 L1 1", "M0 0 L1 1 2 2"])), "ML with 4 numbers against ML with 6 (");
        // Arc flags are part of the structure: they are never interpolated.
        has(&bad(json!(["M0 0 A5 5 0 0 1 10 10", "M0 0 A5 5 0 1 1 10 10"])), "MA with 7 numbers and arc flags 01 against MA with 7 and arc flags 11 (");
        has(&bad(json!(["M84 96 Q100 104 116 96", "M84 96 Q100 104"])), "a d keyframe: the command Q has too few numbers");
        has(&bad(json!(["M84 96 Q100 104 116 96", format!("M0 0{}", " L1 1".repeat(200))])), "a d keyframe: 1004 characters of path data: at most 1000");
        // Against the element's own d: accepted with a warning while no keyframe is null…
        let applied = accepted(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": ["M84 96 L100 104 L116 96", "M84 96 L100 110 L116 96", "M84 96 L100 104 L116 96"] }])));
        has(&applied.warnings.join("\n"), "these d keyframes are MLL with 6 numbers, the d of mouth is MQ with 6 numbers (at animations.a.tracks[0]): the shape jumps at its start and end");
        // …and refused when one is, because null is that d.
        has(&bad(json!([null, "M84 96 L100 104 L116 96", null])), "null is the d of mouth, MQ with 6 numbers, and the keyframes are MLL with 6 numbers (at animations.a.tracks[0]): write them in the structure of that d");
        // The same structure: nothing to say.
        let applied = accepted(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": [null, "M84 96 Q100 120 116 96", null] }])));
        assert_eq!(applied.warnings, [""; 0]);
    }

    #[test]
    fn times_rise_strictly_from_zero_to_one() {
        let bad = |times: Value| refused(None, tracks(json!([{ "target": "arm", "prop": "rotate", "values": [0, 1, 2, 3], "times": times }])));
        for times in [json!([0, 0.5, 0.5, 1]), json!([0, 0.6, 0.4, 1]), json!([0.1, 0.4, 0.6, 1]), json!([0, 0.4, 0.6, 0.9]), json!([0, 0.4, 0.4004, 1]), json!([1, 0.6, 0.4, 0]), json!([0, 0.5, 1.5, 1])] {
            has(&bad(times), "times do not rise from 0 to 1 (at animations.a.tracks[0].times): numbers rising from 0 to 1, one per keyframe");
        }
        has(&bad(json!([0, 0.5, 1])), "3 times for 4 values: one time per keyframe");
        has(&bad(json!([0, "a", 0.5, 1])), "times is a list of 4");
        has(&bad(json!("even")), "times is a string");
        has(&bad(json!([0])), "times is a list of 1: 2 to 16 numbers rising from 0 to 1, one per keyframe");
        // Uneven times are stored as given, to three decimals.
        let look = accepted(None, figure(json!({ "animations": { "a": { "duration": 30, "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 1, 2, 3], "times": [0, 0.002, 0.0044, 1] }] } } }))).design.look.unwrap();
        assert_eq!(serde_json::to_value(&look.animations["a"].tracks[0].times).unwrap(), json!([0, 0.002, 0.004, 1]));
        // The animation's times are the default of its tracks, and have to fit them.
        let shared = |values: Value| figure(json!({ "animations": { "a": { "duration": 2, "times": [0, 0.2, 1], "tracks": [{ "target": "arm", "prop": "rotate", "values": values }] } } }));
        accepted(None, shared(json!([0, 5, 0])));
        has(&refused(None, shared(json!([0, 5]))), "2 values against the animation's 3 times: give this track times of its own, or as many values");
        has(&refused(None, figure(json!({ "animations": { "a": { "duration": 2, "times": [0, 1, 0.5], "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 5, 0] }] } } }))), "times do not rise from 0 to 1 (at animations.a.times)");
    }

    #[test]
    fn paint_keyframes_are_at_least_0_17_s_apart_and_the_others_0_04_s() {
        let with = |duration: f64, track: Value| figure(json!({ "animations": { "a": { "duration": duration, "loop": true, "tracks": [track] } } }));
        // Seven even keyframes over one second are 0.167 s apart.
        let flicker = json!({ "target": "face", "prop": "opacity", "values": [0.9, 0, 0.9, 0, 0.9, 0, 0.9] });
        has(&refused(None, with(1.0, flicker.clone())), "keyframes 0 and 1 of opacity are 0.167 s apart (at animations.a.tracks[0]): at least 0.17 s between keyframes of fill, stroke and opacity, so nothing flashes");
        accepted(None, with(1.02, flicker));
        let colours = json!({ "target": "face", "prop": "fill", "values": ["#ff0000", "#0000ff", "#ff0000"], "times": [0, 0.1, 1] });
        has(&refused(None, with(1.0, colours.clone())), "keyframes 0 and 1 of fill are 0.1 s apart");
        accepted(None, with(1.7, colours));
        has(&refused(None, with(0.3, json!({ "target": "face", "prop": "stroke", "values": ["#ff0000", "#0000ff", "#ff0000"] }))), "keyframes 0 and 1 of stroke are 0.15 s apart");
        // The last gap counts like the first.
        has(&refused(None, with(1.0, json!({ "target": "face", "prop": "opacity", "values": [0.9, 0.5, 0.9], "times": [0, 0.9, 1] }))), "keyframes 1 and 2 of opacity are 0.1 s apart");
        // Any other track may move faster, down to 0.04 s.
        let shiver = json!({ "target": "arm", "prop": "rotate", "values": [0, 1, 0, 1] });
        has(&refused(None, with(0.1, shiver.clone())), "keyframes 0 and 1 of rotate are 0.033 s apart (at animations.a.tracks[0]): at least 0.04 s between keyframes");
        accepted(None, with(0.12, shiver));
        accepted(None, with(0.12, json!({ "target": "face", "prop": "strokeWidth", "values": [2, 3, 2, 3] })));
        accepted(None, with(0.12, json!({ "target": "mouth", "prop": "d", "values": ["M84 96 Q100 104 116 96", "M84 96 Q100 110 116 96", "M84 96 Q100 104 116 96", "M84 96 Q100 110 116 96"] })));
        // The animation's default times count like a track's own.
        has(&refused(None, figure(json!({ "animations": { "a": { "duration": 1, "times": [0, 0.1, 1], "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0, 0.9] }] } } }))), "keyframes 0 and 1 of opacity are 0.1 s apart");
    }

    #[test]
    fn ease_step_is_refused_on_paint_and_opacity() {
        let with = |ease: Value, prop: &str, values: Value| tracks(json!([{ "target": "face", "prop": prop, "values": values, "ease": ease }]));
        has(&refused(None, with(json!("step"), "opacity", json!([0.9, 0.2, 0.9]))), "ease step on opacity (at animations.a.tracks[0]): fill, stroke and opacity change smoothly so that nothing flashes, use linear, in, out or inOut");
        has(&refused(None, with(json!(["linear", "step"]), "fill", json!(["#ffcc00", "#00ff00", "#ffcc00"]))), "ease step on fill");
        has(&refused(None, with(json!("step"), "stroke", json!(["#333333", "#00ff00", "#333333"]))), "ease step on stroke");
        // Taken from the animation, it is refused all the same.
        has(&refused(None, figure(json!({ "animations": { "a": { "duration": 1, "ease": "step", "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9] }] } } }))), "ease step on opacity");
        // Elsewhere a step is fine.
        // Nor may paint swing: elastic, bounce and a bezier that leaves 0 … 1 cross a value more than once.
        for swing in [json!("outElastic"), json!("outBounce"), json!([0, 3, 1, -2]), json!(["linear", [0.3, -0.5, 0.7, 1]])] {
            has(&refused(None, with(swing.clone(), "opacity", json!([0.9, 0.2, 0.9]))), "an ease that swings on opacity (at animations.a.tracks[0])");
            accepted(None, with(swing, "scale", json!([1, 1.1, 1])));
        }
        for steady in [json!("outBack"), json!("inBack"), json!("inOutBack"), json!([0, 0, 0, 1]), json!([0.25, 0.1, 0.25, 1])] {
            accepted(None, with(steady, "opacity", json!([0.9, 0.2, 0.9])));
        }
        accepted(None, with(json!("step"), "scale", json!([1, 1.1, 1])));
        accepted(None, with(json!("step"), "strokeWidth", json!([2, 4, 2])));
    }

    #[test]
    fn eases_are_one_for_all_segments_or_one_per_segment() {
        let with = |ease: Value| tracks(json!([{ "target": "arm", "prop": "rotate", "values": [0, 10, 0], "ease": ease }]));
        for ease in Ease::ALL {
            let look = accepted(None, with(json!(ease.name()))).design.look.unwrap();
            assert_eq!(look.animations["a"].tracks[0].ease, Some(Eases::One(Curve::Named(ease))));
        }
        let look = accepted(None, with(json!(["out", "inOut"]))).design.look.unwrap();
        assert_eq!(look.animations["a"].tracks[0].ease, Some(Eases::Each(vec![Curve::Named(Ease::Out), Curve::Named(Ease::InOut)])));
        has(&refused(None, with(json!(["out"]))), "1 eases for 2 segments: one ease for all segments, or one per segment, which is one fewer than values");
        has(&refused(None, with(json!(["out", "in", "out"]))), "3 eases for 2 segments");
        has(&refused(None, with(json!("bounce"))), "ease \"bounce\" (at animations.a.tracks[0].ease): linear, in, out, inOut, inBack, outBack, inOutBack, outElastic, outBounce, step or a bezier [x1, y1, x2, y2]");
        has(&refused(None, with(json!(["out", "ease-in"]))), "ease \"ease-in\"");
        has(&refused(None, with(json!("InOut"))), "ease \"InOut\"");
        has(&refused(None, with(json!(5))), "ease a number");
        has(&refused(None, with(json!([]))), "ease a list of 0");
        // A list on the animation has to fit every track that takes it.
        has(&refused(None, figure(json!({ "animations": { "a": { "duration": 1, "ease": ["in", "out", "in"], "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 10, 0] }] } } }))), "3 eases for 2 segments");
    }

    #[test]
    fn a_null_keyframe_is_the_artworks_own_value_of_an_absolute_prop() {
        let with = |target: &str, prop: &str, values: Value| tracks(json!([{ "target": target, "prop": prop, "values": values }]));
        let look = accepted(None, with("face", "opacity", json!([null, 0.2, null]))).design.look.unwrap();
        assert_eq!(look.animations["a"].tracks[0].values, [None, Some(Key::Num(Num::new(0.2))), None]);
        accepted(None, with("face", "fill", json!([null, "#ff0000", null])));
        accepted(None, with("face", "stroke", json!([null, "#ff0000", null])));
        accepted(None, with("face", "strokeWidth", json!([null, 4, null])));
        accepted(None, with("face", "r", json!([null, 32, null])));
        accepted(None, with("mouth", "d", json!([null, "M84 96 Q100 120 116 96", null])));
        accepted(None, with("arm", "opacity", json!([null, 0.5, null])));
        accepted(None, with("arm", "x", json!([null, 150, null])));
        // Relative props have no value of their own: rest is 0, or 1 for a scale.
        for prop in ["translateX", "translateY", "rotate", "scale", "scaleX", "scaleY"] {
            has(&refused(None, with("arm", prop, json!([null, 1, null]))), &format!("null in a {prop} track (2 times, first at animations.a.tracks[0].values[0])"));
        }
        // A colour or a stroke width that the element does not carry itself cannot be told.
        has(&refused(None, with("belly", "fill", json!([null, "#ff0000", null]))), "null is the artwork's own fill, and belly has no plain colour of its own");
        has(&refused(None, with("pupil", "stroke", json!([null, "#ff0000", null]))), "null is the artwork's own stroke, and pupil has no plain colour of its own");
        has(&refused(None, with("mouth", "fill", json!([null, "#ff0000", null]))), "mouth has no plain colour of its own");
        has(&refused(None, with("pupil", "strokeWidth", json!([null, 2, null]))), "null is the artwork's own strokeWidth, and pupil has no stroke-width of its own");
    }

    #[test]
    fn a_track_may_move_several_parts() {
        let with = |target: Value| tracks(json!([{ "target": target, "prop": "opacity", "values": [null, 0.5, null] }]));
        let look = accepted(None, with(json!(["arm", "cane", "label"]))).design.look.unwrap();
        assert_eq!(look.animations["a"].tracks[0].target, Targets::Many(vec!["arm".into(), "cane".into(), "label".into()]));
        // A list of one is stored as the id.
        let look = accepted(None, with(json!(["arm"]))).design.look.unwrap();
        assert_eq!(look.animations["a"].tracks[0].target, Targets::One("arm".into()));
        has(&refused(None, with(json!(["arm", "cane", "arm"]))), "target names arm twice: each part once");
        has(&refused(None, with(json!([]))), "target is a list of 0: an id, or a list of 1 to 16 ids");
        has(&refused(None, with(json!(["arm", 5]))), "target is a list of 2");
        has(&refused(None, with(json!(["arm", "nope"]))), "target \"nope\" not found");
        let many: Vec<String> = (0..=MAX_TARGETS).map(|i| format!("p{i}")).collect();
        has(&refused(None, with(json!(many))), "target is a list of 17");
        // The prop has to fit every one of them.
        has(&refused(None, tracks(json!([{ "target": ["face", "arm"], "prop": "r", "values": [1, 2] }]))), "arm is a rect and has no r");
    }

    #[test]
    fn phase_is_for_loops_and_delay_for_one_shots() {
        let with = |looping: bool, key: &str, value: Value| figure(json!({ "animations": { "a": { "duration": 2, "loop": looping, "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 10, 0], key: value }] } } }));
        let track = |applied: &Applied| applied.design.look.as_ref().unwrap().animations["a"].tracks[0].clone();
        let applied = accepted(None, with(true, "phase", json!(0.25)));
        assert_eq!((track(&applied).phase, track(&applied).delay), (Some(Num::new(0.25)), None));
        assert_eq!(applied.warnings, [""; 0]);
        let applied = accepted(None, with(false, "delay", json!(0.5)));
        assert_eq!((track(&applied).phase, track(&applied).delay), (None, Some(Num::new(0.5))));
        assert_eq!(applied.warnings, [""; 0]);
        // The other way round each is left out, with a warning.
        let applied = accepted(None, with(false, "phase", json!(0.25)));
        assert_eq!(track(&applied).phase, None);
        assert_eq!(applied.warnings, ["phase is for loops and is left out here (at animations.a.tracks[0]): a one-shot starts a track later with delay"]);
        let applied = accepted(None, with(true, "delay", json!(0.5)));
        assert_eq!(track(&applied).delay, None);
        assert_eq!(applied.warnings, ["delay is for one-shots and is left out here (at animations.a.tracks[0]): a loop shifts a track with phase"]);
        has(&refused(None, with(true, "phase", json!(1.5))), "phase is not a number from 0 to 1: the share of the cycle at which a loop's track starts");
        has(&refused(None, with(true, "phase", json!("half"))), "phase is not a number from 0 to 1");
        has(&refused(None, with(false, "delay", json!(2.5))), "delay is not a number of seconds within the duration: 0 up to the animation's duration");
        has(&refused(None, with(false, "delay", json!(-1))), "delay is not a number of seconds within the duration");
        // The ends of both ranges.
        accepted(None, with(true, "phase", json!(1)));
        accepted(None, with(true, "phase", json!(0)));
        accepted(None, with(false, "delay", json!(2)));
    }

    #[test]
    fn an_animation_has_a_duration_and_is_a_loop_a_recurring_one_or_a_one_shot() {
        let with = |key: &str, value: Value| {
            let mut animation = rotate("arm");
            if value.is_null() {
                animation.as_object_mut().unwrap().remove(key);
            } else {
                animation[key] = value;
            }
            figure(json!({ "animations": { "a": animation } }))
        };
        has(&refused(None, with("duration", json!(300))), "duration 300 (at animations.a): seconds, from 0.1 to 30");
        has(&refused(None, with("duration", json!(0.05))), "duration 0.05: seconds, from 0.1 to 30");
        has(&refused(None, with("duration", json!("1s"))), "duration is a string: seconds, from 0.1 to 30");
        has(&refused(None, with("duration", json!(null))), "an animation without a duration");
        accepted(None, with("duration", json!(0.1)));
        accepted(None, with("duration", json!(30)));
        has(&refused(None, with("loop", json!("yes"))), "loop is a string: true or false");
        for every in [json!([0.5, 2]), json!([3, 2]), json!([2, 121]), json!([2]), json!(3), json!(["2", "5"])] {
            has(&refused(None, with("every", every)), "every is not a pair of seconds in range: [min, max] with 1 <= min <= max <= 120");
        }
        accepted(None, with("every", json!([1, 120])));
        let look = accepted(None, with("every", json!([3, 3.0004]))).design.look.unwrap();
        assert_eq!(look.animations["a"].every, Some([Num::new(3.0), Num::new(3.0)]));
        let mut both = rotate("arm");
        both["loop"] = json!(true);
        both["every"] = json!([2, 5]);
        has(&refused(None, figure(json!({ "animations": { "a": both } }))), "loop and every exclude each other");
        has(&refused(None, with("tracks", json!([]))), "0 tracks: 1 to 16 tracks in one animation");
        has(&refused(None, with("tracks", json!(null))), "an animation without tracks");
        has(&refused(None, with("tracks", json!({}))), "tracks is an object");
        has(&refused(None, with("speed", json!(2))), "\"speed\" is not a key here (at animations.a): keys are duration, loop, every, times, ease, tracks");
        has(&refused(None, figure(json!({ "animations": { "a": [1] } }))), "an animation is a list: an object with duration and tracks");
        has(&refused(None, figure(json!({ "animations": [rotate("arm")] }))), "animations is a list: an object of names with an animation each, null to remove one");
        // The key of an earlier draft of this grammar gets a hint of its own.
        has(&refused(None, tracks(json!([{ "target": "arm", "prop": "rotate", "values": [0, 1], "origin": [1, 2] }]))), "\"origin\" is not a key here (at animations.a.tracks[0]): there is no origin here, give the part a pivot in pivots");
    }

    #[test]
    fn animation_names_follow_the_rule_of_ids_and_a_look_holds_at_most_32() {
        let named = |name: &str| figure(json!({ "animations": { name: rotate("arm") } }));
        accepted(None, named("wave-2_b"));
        for name in ["1st", "my wave", "", "é", "__proto__", "a.b"] {
            has(&refused(None, named(name)), "the animation name ");
        }
        for name in RESERVED {
            has(&refused(None, named(name)), &format!("the animation name \"{name}\" (at animations.{name}): this name is reserved, choose another"));
        }
        // The name of a built-in is allowed: the animation takes its place.
        accepted(None, named("blink"));
        let many = |n: usize| {
            let animations: Map<String, Value> = (0..n).map(|i| (format!("a{i}"), rotate("arm"))).collect();
            figure(json!({ "animations": animations }))
        };
        let full = accepted(None, many(MAX_ANIMATIONS));
        assert_eq!((full.usage.animations, full.animations.len()), (32, 32));
        has(&refused(None, many(MAX_ANIMATIONS + 1)), "33 animations (at animations): at most 32 in one look; null removes one");
        // The limit holds after a merge as well.
        has(&refused(Some(&full.design), json!({ "animations": { "extra": rotate("arm") } })), "33 animations");
        // Replacing one, or removing one while adding another, stays within it.
        accepted(Some(&full.design), json!({ "animations": { "a0": null, "extra": rotate("cane") } }));
        accepted(Some(&full.design), json!({ "animations": { "a1": rotate("cane") } }));
    }

    #[test]
    fn the_limits_of_animations_are_enforced_also_after_a_merge() {
        let track = json!({ "target": "arm", "prop": "rotate", "values": [0, 10, 0] });
        accepted(None, tracks(json!(vec![track.clone(); MAX_TRACKS])));
        has(&refused(None, tracks(json!(vec![track; MAX_TRACKS + 1]))), "17 tracks: 1 to 16 tracks in one animation");
        // d keyframes: 1000 characters each as stored, 6000 in a look, the mouth included.
        let d = |n: usize| format!("M0 0{}", " L1 1".repeat(n));
        let frames = |n: usize| tracks(json!([{ "target": "mouth", "prop": "d", "values": vec![d(165); n] }]));
        let six = accepted(None, frames(6));
        has(&refused(None, frames(7)), "6965 characters of d keyframes in the look: at most 6000 over all tracks and the mouth");
        has(&refused(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": [d(165), d(200)] }]))), "1004 characters of path data: at most 1000");
        has(&refused(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": [format!("M0 0{}", "L1-1".repeat(200)), "M0 0"] }]))), "characters of path data once it is spaced out: at most 1000");
        has(&refused(Some(&six.design), json!({ "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96", "closed": "M84 96 Q100 104 116 96" } })), "6018 characters of d keyframes in the look");
        // The stored look: 128 KiB. Two calls that each fit may not add up to more.
        let heavy = |from: usize| {
            let values: Vec<Value> = (0..MAX_KEYFRAMES).map(|i| json!(0.123 + i as f64 / 100.0)).collect();
            let times: Vec<Value> = (0..MAX_KEYFRAMES).map(|i| json!(i as f64 / (MAX_KEYFRAMES - 1) as f64)).collect();
            let track = json!({ "target": ["arm", "cane", "label", "face", "pupil", "mouth", "belly", "head", "eyes", "body"], "prop": "opacity", "values": values, "times": times });
            let animations: Map<String, Value> = (from..from + 16).map(|i| (format!("heavy{i}"), json!({ "duration": 30, "tracks": vec![track.clone(); MAX_TRACKS] }))).collect();
            json!({ "animations": animations })
        };
        let first = accepted(None, figure(heavy(0)));
        assert!(first.usage.look_bytes > MAX_LOOK_BYTES / 2 && first.usage.look_bytes < MAX_LOOK_BYTES, "{}", first.usage.look_bytes);
        let why = refused(Some(&first.design), heavy(16));
        has(&why, "KiB once stored: at most 128 KiB, so fewer elements, shorter paths or fewer keyframes");
        has(&why, "the look is 1");
    }

    // ------------------------------------------------------------ states, mouth, gaze, voice, name

    #[test]
    fn states_list_the_looks_own_animations_and_built_in_ones() {
        let base = |states: Value| {
            figure(json!({ "roles": { "body": "body", "eyes": "eyes" }, "animations": { "hop": { "duration": 1, "tracks": [{ "target": "body", "prop": "translateY", "values": [0, -10, 0] }] } }, "states": states }))
        };
        let applied = accepted(None, base(json!({ "done": ["breathe", "blink", "hop"], "idle": [], "error": ["hop"] })));
        assert_eq!(applied.warnings, [""; 0]);
        let look = applied.design.look.unwrap();
        assert_eq!(serde_json::to_string(&look.states).unwrap(), r#"{"idle":[],"error":["hop"],"done":["breathe","blink","hop"]}"#);
        for state in LookState::ALL {
            accepted(None, base(json!({ state.name(): ["hop"] })));
        }
        has(&refused(None, base(json!({ "done": ["wink"] }))), "no animation \"wink\" (at states.done[0]): yours are hop; built in are breathe, blink, think, talk, listen, alert, doze, wave, cheer, nod, shake");
        has(&refused(None, base(json!({ "resting": ["hop"] }))), "\"resting\" is not a state: states are idle, working, speaking, listening, waiting, attention, error, done, sleeping");
        has(&refused(None, base(json!({ "idle": ["hop", "hop"] }))), "hop is listed twice (at states.idle[1]): each animation once in a state");
        has(&refused(None, base(json!({ "idle": ["hop", "breathe", "blink", "talk", "listen", "nod", "shake"] }))), "a state's list is a list of 7: up to 6 animation names, [] for stillness, null for the default");
        has(&refused(None, base(json!({ "idle": "hop" }))), "a state's list is a string");
        has(&refused(None, base(json!({ "idle": ["hop", 5] }))), "a state's list is a list with entries that are not names");
        has(&refused(None, base(json!(["hop"]))), "states is a list: an object of states with a list of animation names each, null for the default");
        // A built-in that the roles do not allow is accepted, with a warning.
        let applied = accepted(None, base(json!({ "working": ["think", "wave"] })));
        assert_eq!(applied.warnings, [
            "think is built in and needs the head role, or the pupils role and a gaze above 0 (at states.working[0]): name the role, or it does nothing in this look",
            "wave is built in and needs the armR or the armL role (at states.working[1]): name the role, or it does nothing in this look",
        ]);
    }

    #[test]
    fn the_names_every_javascript_object_answers_to_are_refused_everywhere() {
        for name in RESERVED {
            // As an id.
            has(&undrawn(&format!("<circle id='{name}' r='5'/>")), "this name is reserved");
            // As an animation.
            has(&refused(None, figure(json!({ "animations": { name: rotate("arm") } }))), "this name is reserved");
            // As an entry of a state: there is no such animation, and there cannot be.
            has(&refused(None, figure(json!({ "states": { "idle": [name] } }))), &format!("no animation \"{name}\""));
            // As the target of a mouth, a track, a pivot or a role: there is no such part.
            has(&refused(None, figure(json!({ "mouth": { "target": name, "prop": "scaleY", "open": 2 } }))), &format!("target \"{name}\" not found"));
            has(&refused(None, tracks(json!([{ "target": name, "prop": "rotate", "values": [0, 1] }]))), &format!("target \"{name}\" not found"));
            has(&refused(None, figure(json!({ "pivots": { name: [1, 2] } }))), &format!("target \"{name}\" not found"));
            has(&refused(None, figure(json!({ "roles": { "head": name } }))), &format!("target \"{name}\" not found"));
        }
        // "__proto__" is not even a name.
        has(&undrawn("<circle id='__proto__' r='5'/>"), "a letter, then letters, digits, _ or -");
        has(&refused(None, figure(json!({ "animations": { "__proto__": rotate("arm") } }))), "the animation name \"__proto__\"");
        has(&refused(None, figure(json!({ "states": { "__proto__": ["blink"] } }))), "\"__proto__\" is not a state");
        has(&refused(None, figure(json!({ "states": { "constructor": ["blink"] } }))), "\"constructor\" is not a state");
        has(&refused(None, figure(json!({ "roles": { "__proto__": "arm" } }))), "\"__proto__\" is not a role");
        has(&refused(None, figure(json!({ "pivots": { "__proto__": [1, 2] } }))), "target \"__proto__\" not found");
        // A gesture is played by the name of an animation or of a built-in: neither can be one of these.
        assert!(RESERVED.iter().all(|name| !BUILTIN.contains(name) && name_fault(name).is_some()));
        assert!(BUILTIN.iter().all(|name| name_fault(name).is_none()));
    }

    #[test]
    fn the_mouth_morphs_a_path_or_drives_one_number() {
        let mouth = |m: Value| figure(json!({ "mouth": m, "pivots": { "eyes": [90, 75], "head": [100, 110] } }));
        let stored = |m: Value| serde_json::to_value(&accepted(None, mouth(m)).design.look.unwrap().mouth).unwrap();
        // A path morph: closed is the element's own d unless it is given.
        assert_eq!(stored(json!({ "target": "mouth", "open": "M84,96 Q100,120 116,96" })), json!([{ "target": "mouth", "open": "M 84 96 Q 100 120 116 96" }]));
        assert_eq!(
            stored(json!({ "target": "mouth", "open": "M84 96 Q100 120 116 96", "closed": "M84 96 Q100 100 116 96" })),
            json!([{ "target": "mouth", "open": "M 84 96 Q 100 120 116 96", "closed": "M 84 96 Q 100 100 116 96" }])
        );
        // One number: closed is the artwork's own value unless it is given.
        assert_eq!(stored(json!({ "target": "eyes", "prop": "scaleY", "open": 1.6 })), json!([{ "target": "eyes", "prop": "scaleY", "open": 1.6 }]));
        assert_eq!(stored(json!({ "target": "face", "prop": "r", "open": 34, "closed": 30 })), json!([{ "target": "face", "prop": "r", "open": 34, "closed": 30 }]));
        let four = json!([{ "target": "belly", "prop": "ry", "open": 33 }, { "target": "arm", "prop": "height", "open": 50 }, { "target": "head", "prop": "translateY", "open": 3 }, { "target": "head", "prop": "scale", "open": 1.1 }]);
        assert_eq!(stored(four.clone()), four);
        assert_eq!(stored(json!({ "target": "head", "prop": "scaleX", "open": 1.1 })), json!([{ "target": "head", "prop": "scaleX", "open": 1.1 }]));
        // None at all with null or an empty list; five are one too many.
        assert_eq!(stored(json!(null)), json!([]));
        assert_eq!(stored(json!([])), json!([]));
        has(&refused(None, mouth(json!(vec![json!({ "target": "eyes", "prop": "scaleY", "open": 1.6 }); 5]))), "mouth is a list of 5: one drive, or a list of up to 4 that share one openness, or null for none");
        has(&refused(None, mouth(json!("open"))), "mouth is a string");

        has(&refused(None, mouth(json!({ "target": "mouth", "open": "M84 96 L100 120 L116 96" }))), "mouth open is MLL with 6 numbers, the d of mouth is MQ with 6 numbers (at mouth): write open in the structure of that d, or give closed too");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": "M84 96 Q100 120 116 96", "closed": "M84 96 L100 100 L116 96" }))), "mouth open and closed differ in structure: MQ with 6 numbers against MLL with 6 (at mouth): same commands, same count of numbers, same arc flags");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": "M84 96 Q" }))), "mouth open: the command Q has too few numbers");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": 5 }))), "mouth open is a number: path data as a string; with prop the mouth takes numbers");
        has(&refused(None, mouth(json!({ "target": "mouth" }))), "a mouth drive without open");
        has(&refused(None, mouth(json!({ "target": "arm", "open": "M0 0 L1 1" }))), "arm is a rect, and a mouth without prop morphs a path: give it a prop, one of scaleY, scaleX, scale, translateY, ry, r or height");
        has(&refused(None, mouth(json!({ "target": "arm", "prop": "rotate", "open": 5 }))), "the mouth's prop is not one it can drive: scaleY, scaleX, scale, translateY, ry, r or height");
        has(&refused(None, mouth(json!({ "target": "arm", "prop": "r", "open": 5 }))), "arm is a rect and has no r: r is a prop of circle");
        has(&refused(None, mouth(json!({ "target": "eyes", "prop": "scaleY", "open": 11 }))), "mouth open 11 is out of range: -10 to 10");
        has(&refused(None, mouth(json!({ "target": "eyes", "prop": "scaleY", "open": "wide" }))), "mouth open is a string: a number; without prop the mouth morphs a path and takes path data");
        has(&refused(None, mouth(json!({ "target": "eyes", "prop": "scaleY" }))), "a mouth drive without open");
        has(&refused(None, mouth(json!({ "target": "lips", "prop": "scaleY", "open": 2 }))), "target \"lips\" not found (at mouth)");
        has(&refused(None, mouth(json!({ "target": "sky", "prop": "scaleY", "open": 2 }))), "sky is a linearGradient, or inside defs");
        has(&refused(None, mouth(json!({ "prop": "scaleY", "open": 2 }))), "a mouth drive without a target");
        has(&refused(None, mouth(json!({ "target": "eyes", "prop": "scaleY", "open": 2, "origin": [1, 2] }))), "\"origin\" is not a key here (at mouth): there is no origin here, give the part a pivot in pivots");
        has(&refused(None, mouth(json!([{ "target": "eyes", "prop": "scaleY", "open": 2 }, 5]))), "a mouth drive is a number (at mouth[1])");

        // A closed that is not the artwork's own shape, and a scale of a group without a pivot, are accepted with a warning.
        let applied = accepted(None, figure(json!({ "mouth": [{ "target": "mouth", "open": "M84 96 L100 120 L116 96", "closed": "M84 96 L100 100 L116 96" }, { "target": "eyes", "prop": "scaleY", "open": 1.5 }] })));
        assert_eq!(applied.warnings, [
            "the mouth's closed is MLL with 6 numbers, the d of mouth is MQ with 6 numbers (at mouth[0]): the shape jumps when a line starts and ends",
            "eyes has no pivot: it turns and scales about the centre of its box, [90, 75] (at mouth[1]): give it a pivot if its joint is elsewhere",
        ]);
    }

    #[test]
    fn gaze_is_the_travel_of_the_pupils() {
        let gaze = |g: Value| figure(json!({ "gaze": g, "roles": { "pupils": "pupil" } }));
        assert_eq!(accepted(None, gaze(json!(4.0004))).design.look.unwrap().gaze, Some(Num::new(4.0)));
        assert_eq!(accepted(None, gaze(json!(0))).design.look.unwrap().gaze, Some(Num::new(0.0)));
        assert_eq!(accepted(None, gaze(json!(10))).design.look.unwrap().gaze, Some(Num::new(10.0)));
        assert_eq!(accepted(None, gaze(json!(null))).design.look.unwrap().gaze, None);
        assert_eq!(accepted(None, json!({ "svg": FIGURE })).design.look.unwrap().gaze, None);
        for bad in [json!(10.5), json!(-1), json!("far"), json!([1])] {
            has(&refused(None, gaze(bad)), "gaze is not a number in range (at gaze): how far the pupils travel, 0 to 10 here (5% of the width), 0 for none, null for the default");
        }
        // Without a part that plays the pupils it moves nothing.
        assert_eq!(accepted(None, figure(json!({ "gaze": 4 }))).warnings, ["gaze moves the part with the pupils role, and no part has it: name it in roles"]);
        assert_eq!(accepted(None, figure(json!({ "gaze": 0 }))).warnings, [""; 0]);
        // The limit follows the width of the viewBox.
        let narrow = |g: f64| json!({ "svg": "<svg viewBox='0 0 100 100'><circle id='p' cx='50' cy='50' r='5'/></svg>", "roles": { "pupils": "p" }, "gaze": g });
        accepted(None, narrow(5.0));
        has(&refused(None, narrow(5.5)), "0 to 5 here");
    }

    #[test]
    fn a_voice_is_a_language_name_fragments_pitch_and_rate() {
        let voice = |v: Value| accepted(None, json!({ "voice": v }));
        let applied = voice(json!({ "lang": "en-GB", "prefer": ["Female", " Natural\u{202E} "], "pitch": 1.15004, "rate": 1.05 }));
        assert_eq!(serde_json::to_value(&applied.design.voice).unwrap(), json!({ "lang": "en-GB", "prefer": ["Female", "Natural"], "pitch": 1.15, "rate": 1.05 }));
        assert_eq!(applied.design.look, None);
        assert_eq!(serde_json::to_string(&voice(json!({})).design.voice).unwrap(), "{}");
        for lang in ["en", "de-DE", "pt-BR", "zh-Hans-CN", "yue", "es-419"] {
            assert_eq!(voice(json!({ "lang": lang })).design.voice.lang.as_deref(), Some(lang));
        }
        for lang in ["english", "e", "en_GB", "en-", "-en", "en-GB-x-y", "en GB", "", "en-G", "en-123456789", "en-GB\u{202E}"] {
            has(&refused(None, json!({ "voice": { "lang": lang } })), "a language tag such as en-GB, de or pt-BR");
        }
        has(&refused(None, json!({ "voice": { "lang": 5 } })), "lang is a number");
        for (key, low, high) in [("pitch", 0.7, 1.4), ("rate", 0.8, 1.3)] {
            voice(json!({ key: low }));
            voice(json!({ key: high }));
            for bad in [json!(low - 0.01), json!(high + 0.01), json!("fast")] {
                has(&refused(None, json!({ "voice": { key: bad } })), &format!("{key} is not a number in range (at voice.{key}): {low} to {high}"));
            }
        }
        voice(json!({ "prefer": ["a", "b", "c", "d", "e"] }));
        has(&refused(None, json!({ "voice": { "prefer": ["a", "b", "c", "d", "e", "f"] } })), "prefer is a list of 6: up to 5 pieces of browser voice names, tried in order");
        has(&refused(None, json!({ "voice": { "prefer": ["a", 5] } })), "prefer is a list with entries that are not strings");
        has(&refused(None, json!({ "voice": { "prefer": "Female" } })), "prefer is a string");
        has(&refused(None, json!({ "voice": { "prefer": ["x".repeat(MAX_VOICE_PREFER_CHARS + 1)] } })), "a fragment has 49 characters: at most 48 (at voice.prefer[0])");
        has(&refused(None, json!({ "voice": { "prefer": ["a", " \u{200B} "] } })), "an empty fragment (at voice.prefer[1])");
        has(&refused(None, json!({ "voice": "Female" })), "voice is a string: an object with lang, prefer, pitch and rate");
        // Keys of a later version are dropped with a warning.
        let applied = voice(json!({ "lang": "de", "engine": "neural", "volume": 2 }));
        assert_eq!(applied.design.voice, Voice { lang: Some("de".into()), ..Voice::default() });
        assert_eq!(applied.warnings, ["voice: engine, volume left out: a voice takes lang, prefer, pitch and rate"]);
        // A new voice replaces the old one whole; null takes it back to the default.
        let pitched = voice(json!({ "lang": "de", "pitch": 1.2 }));
        assert_eq!(accepted(Some(&pitched.design), json!({ "voice": { "rate": 0.9 } })).design.voice, Voice { rate: Some(Num::new(0.9)), ..Voice::default() });
        assert_eq!(accepted(Some(&pitched.design), json!({ "voice": null })).design.voice, Voice::default());
        assert_eq!(accepted(Some(&pitched.design), json!({ "name": "Pip" })).design.voice, pitched.design.voice);
    }

    #[test]
    fn the_name_is_one_line_of_plain_text() {
        assert_eq!(accepted(None, json!({ "name": "  Pip\u{202E} the\n frog " })).design.name, "Pip the frog");
        assert_eq!(accepted(None, json!({ "name": "é".repeat(MAX_NAME_CHARS) })).design.name.chars().count(), 32);
        has(&refused(None, json!({ "name": "x".repeat(MAX_NAME_CHARS + 1) })), "the name has 33 characters: at most 32 (at name): a short name of the avatar's own");
        has(&refused(None, json!({ "name": 5 })), "name is a number: a string, the avatar's own name");
        let named = accepted(None, json!({ "name": "Pip" }));
        assert_eq!(named.design.look, None);
        assert_eq!((named.parts.len(), named.animations.len(), named.usage), (0, 0, Usage::default()));
        assert_eq!(accepted(Some(&named.design), json!({ "voice": { "lang": "de" } })).design.name, "Pip");
        assert_eq!(accepted(Some(&named.design), json!({ "name": null })).design.name, "");
        assert_eq!(accepted(Some(&named.design), json!({ "name": "" })).design.name, "");
    }

    #[test]
    fn the_sessions_secrets_are_masked_in_every_text_of_a_design() {
        let mask = |s: &str| s.replace("hunter2hunter2", "••••••");
        let request = json!({ "name": "hunter2\u{200B}hunter2", "svg": svg("<text id='t' x='10' y='50'>hunter2hunter2</text>"), "voice": { "prefer": ["hunter2hunter2 voice"] } });
        let applied = apply_masked(None, &request, &mask).unwrap();
        assert_eq!(applied.design.name, "••••••");
        assert_eq!(node(applied.design.look.as_ref().unwrap(), "t").text.as_deref(), Some("••••••"));
        assert_eq!(applied.design.voice.prefer, ["•••••• voice"]);
        // Without a mask the text is cleaned and kept.
        assert_eq!(apply(None, &request).unwrap().design.name, "hunter2hunter2");
    }

    // ------------------------------------------------------------ the rules of a call

    #[test]
    fn a_call_without_svg_merges_into_the_current_look() {
        let first = accepted(
            None,
            figure(json!({
                "name": "Pip",
                "pivots": { "arm": [146, 100], "head": [100, 110] },
                "roles": { "body": "body", "head": "head" },
                "animations": { "swing": rotate("arm"), "nod": rotate("head") },
                "states": { "idle": ["swing"], "done": ["nod"] },
                "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96" },
                "gaze": 4,
                "voice": { "lang": "en-GB" }
            })),
        );
        let design = first.design;
        let before = design.look.clone().unwrap();
        let merged = accepted(
            Some(&design),
            json!({
                "pivots": { "arm": null, "cane": [60, 120] },
                "roles": { "head": null, "eyes": "eyes" },
                "animations": { "swing": null, "tap": rotate("cane"), "nod": { "duration": 2, "tracks": [{ "target": "head", "prop": "rotate", "values": [0, 5, 0] }] } },
                "states": { "idle": null, "waiting": [], "done": ["nod", "tap"] }
            }),
        );
        let after = merged.design.look.clone().unwrap();
        // Animations and pivots by key, roles and states by entry; null removes one.
        assert_eq!(serde_json::to_value(&after.pivots).unwrap(), json!({ "cane": [60, 120], "head": [100, 110] }));
        assert_eq!(serde_json::to_value(&after.roles).unwrap(), json!({ "body": "body", "eyes": "eyes" }));
        assert_eq!(after.animations.keys().collect::<Vec<_>>(), ["nod", "tap"]);
        assert_eq!(after.animations["nod"].duration, Num::new(2.0));
        assert_eq!(serde_json::to_value(&after.states).unwrap(), json!({ "waiting": [], "done": ["nod", "tap"] }));
        // What the call does not name stays.
        assert_eq!((&after.nodes, after.view_box, &after.mouth, after.gaze), (&before.nodes, before.view_box, &before.mouth, before.gaze));
        assert_eq!((merged.design.name.as_str(), &merged.design.voice), ("Pip", &design.voice));
        assert_eq!((merged.kept.len(), merged.cleared.len()), (0, 0));
        assert_eq!(merged.animations, ["nod", "tap"]);
        // The mouth and gaze replace; null clears a whole field.
        let replaced = accepted(Some(&design), json!({ "mouth": { "target": "eyes", "prop": "scaleY", "open": 1.5 }, "gaze": 2 })).design.look.unwrap();
        assert_eq!(replaced.mouth, [MouthDrive::Prop { target: "eyes".into(), prop: MouthProp::ScaleY, open: Num::new(1.5), closed: None }]);
        assert_eq!(replaced.gaze, Some(Num::new(2.0)));
        assert_eq!(replaced.animations, before.animations);
        let cleared = accepted(Some(&design), json!({ "mouth": null, "gaze": null, "pivots": null, "roles": null, "states": null, "animations": { "extra": rotate("arm") } })).design.look.unwrap();
        assert!(cleared.mouth.is_empty() && cleared.gaze.is_none() && cleared.pivots.is_empty() && cleared.roles.is_empty() && cleared.states.is_empty());
        assert_eq!(cleared.animations.len(), 3);
        // Clearing every animation is refused while a state still names one of them.
        has(&refused(Some(&design), json!({ "animations": null })), "no animation \"swing\" (at states.idle[0])");
        assert!(accepted(Some(&design), json!({ "animations": null, "states": null })).design.look.unwrap().animations.is_empty());
        // A call that touches nothing of the look leaves it as it is, and says nothing about it.
        let renamed = accepted(Some(&design), json!({ "name": "Pop" }));
        assert_eq!(renamed.design.look.as_ref(), Some(&before));
        assert_eq!(renamed.warnings, [""; 0]);
        assert_eq!(renamed.parts.len(), 10);
    }

    #[test]
    fn a_refused_call_changes_nothing_and_one_refused_field_refuses_the_whole_call() {
        let first = accepted(None, figure(json!({ "animations": { "swing": rotate("arm") }, "states": { "idle": ["swing"] } }))).design;
        // What this call removes is still named by what is stored.
        has(&refused(Some(&first), json!({ "animations": { "swing": null } })), "no animation \"swing\" (at states.idle[0])");
        let why = refused(Some(&first), json!({ "name": "Pip", "gaze": 3, "animations": { "nod": rotate("nowhere") } }));
        has(&why, "target \"nowhere\" not found");
        // The current design is a value the caller still holds: there is nothing to undo.
        assert_eq!((first.name.as_str(), first.look.as_ref().unwrap().animations.len(), first.look.as_ref().unwrap().gaze), ("", 1, None));
        // What moves a look needs artwork.
        has(&refused(None, json!({ "animations": { "wave": rotate("arm") } })), "animations without artwork (at animations): this session has no design yet, send svg in the same call");
        has(&refused(None, json!({ "gaze": 3 })), "gaze without artwork");
        has(&refused(None, json!({ "parents": { "arm": "body" } })), "parents without artwork");
        has(&refused(None, json!({ "drives": [] })), "drives without artwork");
        has(&refused(None, json!({})), "an empty design: send at least one of name, svg, pivots, parents, roles, animations, states, mouth, gaze, drives, voice");
        has(&refused(None, json!({ "keep": true })), "an empty design");
        has(&refused(None, json!({ "name": "Pip", "colour": "green" })), "\"colour\" is not a key here: keys are name, svg, pivots, parents, roles, animations, states, mouth, gaze, drives, voice, keep");
        has(&refused(None, json!("<svg/>")), "a design is a string: an object with any of name, svg, pivots, parents, roles, animations, states, mouth, gaze, drives, voice, keep");
        has(&refused(None, json!({ "svg": 5 })), "svg is a number: the artwork as a string of SVG markup");
        has(&refused(None, json!({ "svg": null })), "svg is null");
        has(&refused(None, json!({ "svg": FIGURE, "keep": "yes" })), "keep is a string: true or false");
        // Artwork that cannot be read to its end is reported alone: there are no parts to check the rest against.
        assert_eq!(refused(None, json!({ "svg": "<svg><g>", "animations": { "wave": rotate("arm") } })), "svg ends inside <g> (at line 1 column 9): every element is closed, the svg element last");
        // Over 256 KiB of arguments nothing is read.
        let huge = json!({ "name": "Pip", "junk": "x".repeat(MAX_DESIGN_BYTES) });
        assert_eq!(refused(None, huge), "the design is 256.0 KiB of JSON: at most 256 KiB");
        let fits = json!({ "name": "Pip", "voice": { "prefer": vec!["x".repeat(40); 5] }, "svg": format!("<svg><!--{}--><circle cx='50' cy='50' r='5'/></svg>", "x".repeat(MAX_SVG_BYTES - 100)) });
        accepted(None, fits);
    }

    #[test]
    fn a_call_with_svg_is_a_new_look_and_keep_keeps_what_still_has_its_targets() {
        let old = accepted(
            None,
            figure(json!({
                "name": "Pip",
                "voice": { "lang": "en-GB" },
                "pivots": { "arm": [146, 100], "head": [100, 110] },
                "roles": { "body": "body", "head": "head" },
                "animations": { "swing": rotate("arm"), "nod": rotate("head"), "morph": { "duration": 1, "tracks": [{ "target": "mouth", "prop": "d", "values": [null, "M84 96 Q100 120 116 96", null] }] } },
                "states": { "idle": ["swing", "blink"], "done": ["nod"], "working": ["morph"] },
                "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96" },
                "gaze": 4
            })),
        )
        .design;
        // New artwork: the arm is gone, the head is still a group, the mouth is now a circle.
        let new_svg = svg("<g id='body'><g id='head'><circle id='face' cx='100' cy='80' r='30'/><circle id='mouth' cx='100' cy='95' r='4'/></g></g>");

        // Without keep everything that moved the old look is cleared, and the answer says what.
        let fresh = accepted(Some(&old), json!({ "svg": new_svg, "animations": { "nod": rotate("head") }, "pivots": { "head": [100, 110] } }));
        let look = fresh.design.look.as_ref().unwrap();
        assert_eq!(look.animations.keys().collect::<Vec<_>>(), ["nod"]);
        assert_eq!(serde_json::to_value(&look.pivots).unwrap(), json!({ "head": [100, 110] }));
        assert!(look.roles.is_empty() && look.states.is_empty() && look.mouth.is_empty() && look.gaze.is_none());
        assert_eq!(fresh.kept, [""; 0]);
        assert_eq!(fresh.cleared, ["pivots arm", "roles body, head", "animations morph, swing", "states idle, working, done", "mouth", "gaze"]);
        // The name and the voice are not part of the look: they stay.
        assert_eq!((fresh.design.name.as_str(), fresh.design.voice.lang.as_deref()), ("Pip", Some("en-GB")));

        // With keep: what still has its targets, and still fits them, stays.
        let kept = accepted(Some(&old), json!({ "svg": new_svg, "keep": true }));
        let look = kept.design.look.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&look.pivots).unwrap(), json!({ "head": [100, 110] }));
        assert_eq!(serde_json::to_value(&look.roles).unwrap(), json!({ "body": "body", "head": "head" }));
        assert_eq!(look.animations.keys().collect::<Vec<_>>(), ["nod"]);
        assert_eq!(serde_json::to_value(&look.states).unwrap(), json!({ "idle": ["blink"], "done": ["nod"] }));
        assert!(look.mouth.is_empty());
        assert_eq!(look.gaze, Some(Num::new(4.0)));
        assert_eq!(kept.kept, ["pivots head", "roles body, head", "animations nod", "states idle, done", "gaze"]);
        assert_eq!(kept.cleared, ["pivots arm", "animations morph, swing", "states idle (swing), working (morph)", "mouth"]);

        // What the call gives itself comes on top of what is kept.
        let both = accepted(Some(&old), json!({ "svg": new_svg, "keep": true, "animations": { "nod": null, "bob": rotate("face") }, "gaze": null, "roles": { "head": "face" } }));
        let look = both.design.look.unwrap();
        assert_eq!(look.animations.keys().collect::<Vec<_>>(), ["bob"]);
        assert_eq!(look.gaze, None);
        assert_eq!(serde_json::to_value(&look.roles).unwrap(), json!({ "body": "body", "head": "face" }));
        assert_eq!(both.kept, ["pivots head", "roles body", "states idle, done"]);

        // The first design has nothing to keep or clear.
        let first = accepted(None, json!({ "svg": FIGURE, "keep": true }));
        assert!(first.kept.is_empty() && first.cleared.is_empty());
        // The same artwork again with keep keeps everything, to the last number.
        let again = accepted(Some(&old), json!({ "svg": FIGURE, "keep": true }));
        assert_eq!(again.design, old);
        assert_eq!((again.cleared.len(), again.kept.len()), (0, 6));
        // A new look over one that had nothing moving it clears nothing.
        let bare = accepted(None, json!({ "svg": FIGURE })).design;
        let over = accepted(Some(&bare), json!({ "svg": new_svg }));
        assert!(over.kept.is_empty() && over.cleared.is_empty());
    }

    // ------------------------------------------------------------ the lint

    #[test]
    fn the_lint_reports_parts_outside_the_view_box_and_parts_too_small_to_see() {
        let applied = accepted(
            None,
            json!({ "svg": svg(
                "<circle id='in' cx='100' cy='100' r='20'/><circle id='edge' cx='195' cy='100' r='20'/><g id='low'><rect x='90' y='190' width='20' height='20'/></g>\
                 <circle id='gone' cx='-50' cy='-50' r='10'/><circle id='dot' cx='50' cy='50' r='0.5'/><g id='hollow'/><line id='hair' x1='10' y1='10' x2='10' y2='40'/>\
                 <circle id='speck' cx='100' cy='5' r='0.2'/><circle id='near' cx='10' cy='190.4' r='10'/><circle cx='500' cy='500' r='1'/>"
            ) }),
        );
        assert_eq!(applied.warnings, [
            "edge reaches outside the viewBox: x 175 to 215, y 80 to 120; so do low, gone: what leaves the viewBox is cut off",
            "smaller than 2 units: dot, hollow, speck: it will not be seen",
        ]);
        // A stroke is seen: a round-capped dot drawn as a path of no length, and a tiny ring with
        // a thick stroke it inherits, are not small. A stroke too thin to make up for the size is.
        let stroked = accepted(
            None,
            json!({ "svg": svg(
                "<circle id='in' cx='100' cy='100' r='20'/><circle id='dot' cx='50' cy='50' r='0.5'/>\
                 <path id='pip' d='M100 100 h0' stroke='#000' stroke-width='8' stroke-linecap='round'/>\
                 <g stroke='#000' stroke-width='3'><circle id='ring' cx='60' cy='60' r='0.8'/></g>\
                 <g stroke='#000' stroke-linecap='square' stroke-width='4' transform='scale(0.5)'><line id='tick' x1='80' y1='80' x2='80' y2='80'/></g>\
                 <circle id='mite' cx='140' cy='140' r='0.2' stroke='#000' stroke-width='0.5'/>"
            ) }),
        );
        assert_eq!(stroked.warnings, ["smaller than 2 units: dot, mite: it will not be seen"]);
        // With the default butt cap a path of no length draws nothing, however wide its stroke;
        // so does a stroke that is none.
        let capless = accepted(None, json!({ "svg": svg("<circle id='in' cx='100' cy='100' r='20'/><path id='nub' d='M100 100 h0' stroke='#000' stroke-width='8'/><circle id='bare' cx='60' cy='60' r='0.8' stroke='none' stroke-width='3'/>") }));
        assert_eq!(capless.warnings, ["smaller than 2 units: nub, bare: it will not be seen"]);
        // Through a transform: where the part ends up is what counts.
        let moved = accepted(None, json!({ "svg": svg("<g transform='translate(150 0)'><rect id='r' x='0' y='50' width='60' height='20'/></g><rect id='s' x='250' y='50' width='60' height='20' transform='translate(-200 0)'/>") }));
        assert_eq!(moved.warnings, ["r reaches outside the viewBox: x 150 to 210, y 50 to 70: what leaves the viewBox is cut off"]);
        let two = accepted(None, json!({ "svg": svg("<circle id='a' cx='0' cy='50' r='10'/><circle id='b' cx='50' cy='200' r='10'/>") }));
        assert_eq!(two.warnings, ["a reaches outside the viewBox: x -10 to 10, y 40 to 60; so does b: what leaves the viewBox is cut off"]);
        // A call that only renames says nothing about the artwork again.
        assert!(accepted(Some(&applied.design), json!({ "name": "Pip" })).warnings.is_empty());
    }

    #[test]
    fn the_lint_reports_a_translate_that_moves_its_part_out_of_the_view_box() {
        let hop = |prop: &str, values: Value| accepted(None, figure(json!({ "animations": { "hop": { "duration": 1, "tracks": [{ "target": "body", "prop": prop, "values": values }] } } }))).warnings;
        assert_eq!(hop("translateY", json!([0, -40, 0])), [""; 0]);
        assert_eq!(hop("translateY", json!([0, -60, 0])), ["translateY -60 moves body out of the viewBox, to x 60 to 152, y -10 to 132 (at animations.hop.tracks[0]): what leaves the viewBox is cut off"]);
        assert_eq!(hop("translateX", json!([0, 30, -70, 0])), ["translateX -70 moves body out of the viewBox, to x -10 to 82, y 50 to 192 (at animations.hop.tracks[0]): what leaves the viewBox is cut off"]);
        // The move is made in the part's own coordinates: under a parent turned a quarter, its x is the root's y.
        let turned = |prop: &str, values: Value| {
            accepted(None, json!({ "svg": svg("<g transform='rotate(90 100 100)'><rect id='r' x='90' y='20' width='20' height='20'/></g>"), "animations": { "a": { "duration": 1, "tracks": [{ "target": "r", "prop": prop, "values": values }] } } })).warnings
        };
        assert_eq!(turned("translateX", json!([0, 50, 0])), [""; 0]);
        assert_eq!(turned("translateY", json!([0, -40, 0])), ["translateY -40 moves r out of the viewBox, to x 200 to 220, y 90 to 110 (at animations.a.tracks[0]): what leaves the viewBox is cut off"]);
        // A part that is outside at rest is reported as that, once.
        let outside = accepted(None, json!({ "svg": svg("<circle id='c' cx='195' cy='100' r='20'/>"), "animations": { "a": { "duration": 1, "tracks": [{ "target": "c", "prop": "translateX", "values": [0, 50, 0] }] } } })).warnings;
        assert_eq!(outside.len(), 1);
        has(&outside[0], "c reaches outside the viewBox");
    }

    #[test]
    fn the_lint_reports_a_one_shot_that_does_not_start_and_end_at_the_artworks_own_value() {
        let one = |looping: bool, target: &str, prop: &str, values: Value| accepted(None, figure(json!({ "animations": { "a": { "duration": 2, "loop": looping, "tracks": [{ "target": target, "prop": prop, "values": values }] } } }))).warnings;
        assert_eq!(one(false, "face", "opacity", json!([1, 0.2, 1])), ["opacity of face does not start and end at its own value, 0.9 (at animations.a.tracks[0]): a one-shot jumps back at its end; start and end with null"]);
        assert_eq!(one(false, "face", "opacity", json!([0.9, 0.2, 0.9])), [""; 0]);
        assert_eq!(one(false, "face", "opacity", json!([null, 0.2, null])), [""; 0]);
        assert_eq!(one(false, "face", "opacity", json!([0.9, 0.2])).len(), 1);
        // An element without the attribute has the default: opaque, and 0 for geometry.
        assert_eq!(one(false, "arm", "opacity", json!([1, 0.2, 1])), [""; 0]);
        assert_eq!(one(false, "face", "r", json!([30, 34, 30])), [""; 0]);
        has(&one(false, "face", "r", json!([28, 34, 28])).join("\n"), "r of face does not start and end at its own value, 30");
        has(&one(false, "face", "fill", json!(["#ff0000", "#00ff00", "#ff0000"])).join("\n"), "fill of face does not start and end at its own value, #ffcc00");
        assert_eq!(one(false, "face", "fill", json!(["#fc0", "#00ff00", "#FFCC00"])), [""; 0]);
        has(&one(false, "face", "strokeWidth", json!([3, 4, 3])).join("\n"), "strokeWidth of face does not start and end at its own value, 2");
        has(&one(false, "mouth", "d", json!(["M84 96 Q100 120 116 96", "M84 96 Q100 104 116 96"])).join("\n"), "d of mouth does not start and end at its own value (at");
        assert_eq!(one(false, "mouth", "d", json!(["M84 96 Q100 104 116 96", "M84 96 Q100 120 116 96", "M84,96Q100,104,116,96"])), [""; 0]);
        // What the element inherits or takes from a gradient cannot be told, so it is not reported.
        assert_eq!(one(false, "belly", "fill", json!(["#ff0000", "#00ff00", "#ff0000"])), [""; 0]);
        assert_eq!(one(false, "pupil", "strokeWidth", json!([3, 4, 3])), [""; 0]);
        // A loop and a relative prop never jump back.
        assert_eq!(one(true, "face", "opacity", json!([1, 0.2, 1])), [""; 0]);
        assert_eq!(one(false, "arm", "translateY", json!([5, 0, 5])), [""; 0]);
    }

    #[test]
    fn a_turn_or_scale_of_a_group_or_path_without_a_pivot_warns_with_the_point_it_will_use() {
        let turn = |target: &str, prop: &str, pivots: Value, roles: Value| {
            accepted(None, figure(json!({ "pivots": pivots, "roles": roles, "animations": { "a": { "duration": 1, "loop": true, "tracks": [{ "target": target, "prop": prop, "values": [1, 1.1, 1] }] } } }))).warnings
        };
        assert_eq!(turn("head", "rotate", json!({}), json!({})), ["head has no pivot: it turns and scales about the centre of its box, [100, 80] (at animations.a.tracks[0]): give it a pivot if its joint is elsewhere"]);
        assert_eq!(turn("mouth", "scaleY", json!({}), json!({})), ["mouth has no pivot: it turns and scales about the centre of its box, [100, 100] (at animations.a.tracks[0]): give it a pivot if its joint is elsewhere"]);
        // The part that plays the body: the bottom centre.
        assert_eq!(turn("body", "scale", json!({}), json!({ "body": "body" })), ["body has no pivot: it turns and scales about the bottom centre of its box, [106, 192] (at animations.a.tracks[0]): give it a pivot if its joint is elsewhere"]);
        // With a pivot, on other shapes, and for a move, nothing is said.
        assert_eq!(turn("head", "rotate", json!({ "head": [100, 110] }), json!({})), [""; 0]);
        assert_eq!(turn("arm", "rotate", json!({}), json!({})), [""; 0]);
        assert_eq!(turn("face", "scale", json!({}), json!({})), [""; 0]);
        assert_eq!(turn("head", "translateY", json!({}), json!({})), [""; 0]);
        // Once per part, however many tracks turn it.
        let many = accepted(None, tracks(json!([{ "target": "head", "prop": "rotate", "values": [0, 5, 0] }, { "target": "head", "prop": "scaleX", "values": [1, 1.1, 1] }, { "target": ["head", "eyes"], "prop": "scale", "values": [1, 1.1, 1] }]))).warnings;
        assert_eq!(many.len(), 2);
        has(&many[0], "head has no pivot: it turns and scales about the centre of its box, [100, 80] (3 times, first at animations.a.tracks[0])");
        has(&many[1], "eyes has no pivot");
        // A pivot given in a later call ends the warning.
        let first = accepted(None, figure(json!({ "animations": { "a": rotate("head") } })));
        assert_eq!(first.warnings.len(), 1);
        assert_eq!(accepted(Some(&first.design), json!({ "pivots": { "head": [100, 110] } })).warnings, [""; 0]);
    }

    // ------------------------------------------------------------ problems and warnings as a whole

    #[test]
    fn problems_are_grouped_by_kind_with_a_count_and_the_first_place() {
        let art = "<svg viewBox='0 0 200 200'><g id='armL'><pattern id='a'/><pattern id='b'/><pattern id='c'/><pattern id='d'/></g></svg>";
        assert_eq!(art.find("<pattern"), Some(40));
        assert_eq!(refused(None, json!({ "svg": art })), "svg: <pattern> is not supported (4 times, first at line 1 column 41, in g#armL): use a flat colour or a gradient");
        // Lines are counted, and columns in characters.
        let lines = "<svg viewBox='0 0 200 200'>\n  <g id='é'>\n  </g>\n  <g id='body'>\n    <text>é</text><blink/>\n  </g>\n</svg>";
        let why = refused(None, json!({ "svg": lines }));
        has(&why, "svg: the id \"é\" (at line 2 column 3): a letter, then letters, digits, _ or -, at most 32 characters");
        has(&why, "svg: the element \"blink\" is not supported (at line 5 column 19, in g#body): draw with g, path, circle, ellipse, rect, line, polyline, polygon, text and use");
        // The place of a problem with a value is its element, by its own id.
        has(&undrawn("<g id='arm'><path id='hand' d='M0 0 L5'/></g>"), "svg: the command L has too few numbers (at line 1 column 40, in path#hand)");
        has(&undrawn("<g id='arm'><path d='M0 0 L5'/></g>"), "(at line 1 column 40, in g#arm)");
        // A path into the arguments.
        let why = refused(
            None,
            figure(json!({ "animations": { "wave": { "duration": 1, "tracks": [
                { "target": "arm", "prop": "rotate", "values": [0, 1] },
                { "target": "arm", "prop": "rotate", "values": [0, 1] },
                { "target": "arm", "prop": "rotate", "values": [0, 1, 2, "x"] }
            ] } } })),
        );
        assert_eq!(why, "a string for rotate (at animations.wave.tracks[2].values[3]): a number, or null for the artwork's own value");
        // The same mistake in many places is one kind.
        let all_wrong: Vec<Value> = (0..10).map(|_| json!({ "target": "elbow", "prop": "rotate", "values": [0, 1] })).collect();
        assert_eq!(refused(None, tracks(json!(all_wrong))), "target \"elbow\" not found (10 times, first at animations.a.tracks[0]): ids are body, belly, head, face, eyes, pupil, mouth, arm, cane, label");
    }

    #[test]
    fn at_most_twelve_kinds_of_problems_are_listed_and_the_rest_counted() {
        let things = |n: usize| (0..n).map(|i| format!("<thing{i}/>")).collect::<String>();
        let problems = apply(None, &json!({ "svg": svg(&things(20)) })).unwrap_err();
        assert_eq!(problems.len(), MAX_PROBLEM_KINDS + 1);
        assert_eq!(problems[0], "svg: the element \"thing0\" is not supported (at line 1 column 28): draw with g, path, circle, ellipse, rect, line, polyline, polygon, text and use");
        assert_eq!(problems[12], "and 8 more kinds");
        assert_eq!(apply(None, &json!({ "svg": svg(&things(13)) })).unwrap_err()[12], "and 1 more kind");
        assert_eq!(apply(None, &json!({ "svg": svg(&things(12)) })).unwrap_err().len(), 12);
        // Problems of the artwork and of what moves it are found in one pass.
        let why = refused(
            None,
            json!({
                "svg": svg("<g id='body'><pattern id='a'/><circle id='head' cx='50' cy='50' r='5'/></g>"),
                "animations": { "nod": { "duration": 300, "tracks": [{ "target": "hed", "prop": "rotate", "values": [0, 1] }] } },
                "voice": { "pitch": 9 },
                "name": 5
            }),
        );
        for part in ["<pattern> is not supported", "duration 300", "target \"hed\" not found (at animations.nod.tracks[0]): ids are body, head", "pitch is not a number in range", "name is a number"] {
            has(&why, part);
        }
        // The artwork first, then what moves it, then the name and the voice: what a cut list keeps is what decides the picture.
        let order: Vec<usize> = ["<pattern>", "duration 300", "name is", "pitch is"].iter().map(|part| why.find(part).unwrap()).collect();
        assert!(order.is_sorted(), "{why}");
    }

    #[test]
    fn quoted_input_is_cut_and_escaped_and_the_joined_message_is_bounded() {
        // Forty characters of what was written, printed as a Rust string.
        let why = undrawn(&format!("<{}/>", "x".repeat(100)));
        has(&why, &format!("svg: the element \"{}…\" is not supported", "x".repeat(MAX_QUOTE_CHARS - 1)));
        let why = refused(None, figure(json!({ "pivots": { "a\u{202E}b\nc": [1, 2] } })));
        has(&why, "target \"a\\u{202e}b\\nc\" not found (at pivots.\"a\\u{202e}b\\nc\")");
        // A reference to a line feed stays one; it is printed as written in Rust.
        let why = undrawn("<circle r='5' fill='&#27;[31m&#10;red'/>");
        has(&why, "svg: fill \"\\u{1b}[31m\\nred\" is not a colour");
        // Every problem keeps to its length, whatever is written.
        let long = "é".repeat(5000);
        refused(None, json!({ "svg": svg(&format!("<circle r='5' fill='{long}' {long}='1'/><{long}/>")), "name": long, "animations": { long.as_str(): 5 }, "pivots": { long.as_str(): 5 }, "states": { long.as_str(): 5 }, "roles": { long.as_str(): 5 }, "voice": { "lang": long } }));
        // The joined message: 3000 bytes at most, cut between characters.
        let problems: Vec<String> = (0..13).map(|i| format!("{i}: {}", "é".repeat(150))).collect();
        let joined = message(&problems);
        assert!(joined.len() <= MAX_MESSAGE_BYTES && joined.len() >= MAX_MESSAGE_BYTES - 1, "{}", joined.len());
        assert!(joined.ends_with('é'));
        assert_eq!(message(&["one".to_string(), "two".to_string()]), "one\ntwo");
        assert_eq!(message(&[]), "");
    }

    #[test]
    fn a_quote_is_masked_before_it_is_cut() {
        // A secret longer than a quote: cut first and masked afterwards, its first forty
        // characters would be left, and no mask finds them any more.
        let secret = format!("_Kq9vX2mL{}", "z7".repeat(25));
        // One of letters alone, which a transform reads as the name of a function.
        let letters = format!("Kq{}", "vXmL".repeat(20));
        let mask = |s: &str| s.replace(secret.as_str(), "••••••").replace(letters.as_str(), "••••••");
        let cases = [
            ("stray text", json!({ "svg": svg(&format!("{secret}<circle id='c' r='5'/>")) })),
            ("a colour", json!({ "svg": svg(&format!("<circle id='c' r='5' fill='{secret}'/>")) })),
            ("a number", json!({ "svg": svg(&format!("<circle id='c' r='{secret}'/>")) })),
            ("path data", json!({ "svg": svg(&format!("<path id='p' d='M0 0 L {secret}'/>")) })),
            ("a transform", json!({ "svg": svg(&format!("<g id='g' transform='translate({secret})'><circle r='5'/></g>")) })),
            ("a transform function", json!({ "svg": svg(&format!("<g id='g' transform='{letters}(1'><circle r='5'/></g>")) })),
            ("a key", json!({ secret.as_str(): 1, "name": "Pip" })),
            ("an animation name", figure(json!({ "animations": { secret.as_str(): { "duration": 1, "tracks": [{ "target": "arm", "prop": "rotate", "values": [0, 1] }] } } }))),
            ("a target", tracks(json!([{ "target": secret, "prop": "rotate", "values": [0, 1] }]))),
            ("a colour keyframe", tracks(json!([{ "target": "face", "prop": "fill", "values": ["#ffffff", secret] }]))),
            ("an ease", tracks(json!([{ "target": "arm", "prop": "rotate", "values": [0, 1], "ease": secret }]))),
            ("a role", figure(json!({ "roles": { secret.as_str(): "arm" } }))),
            ("a state", figure(json!({ "states": { "idle": [secret] } }))),
        ];
        for (what, request) in cases {
            let why = message(&apply_masked(None, &request, &mask).expect_err(what));
            // What is quoted is masked. A transform whose function name is a secret is quoted not
            // at all: masked first, it is no function.
            let quoted = why.contains("••••••") || what == "a transform function";
            assert!(quoted && !why.contains(&secret[..8]) && !why.contains(&letters[..8]), "{what}: {why}");
        }
        // Where the reader stops inside a secret, what it quotes from there, or the number it
        // read, is a piece of the secret that no mask knows: none of it is shown.
        let secrets = ["20240601abcdefXYZ", "20240601.123456789", "M1 2 L.x9vX2mLQ7Z", "abcXYZdefGHI(1"];
        let mask = |s: &str| secrets.iter().fold(s.to_string(), |s, secret| s.replace(secret, "••••••"));
        let piece = |why: &str| secrets.iter().find_map(|secret| secret.as_bytes().windows(6).find(|w| why.as_bytes().windows(6).any(|x| x == *w)).map(|w| String::from_utf8_lossy(w).into_owned()));
        let cases = [
            ("path data that starts with the digits of a secret", json!({ "svg": svg("<path id='p' d='M20240601abcdefXYZ'/>") })),
            ("path data that stops inside a secret", json!({ "svg": svg("<path id='p' d='M1 2 L.x9vX2mLQ7Z'/>") })),
            ("a number with more decimals than are kept", json!({ "svg": svg("<circle id='c' r='5' cx='20240601.123456789'/>") })),
            ("points", json!({ "svg": svg("<polygon id='p' points='20240601.123456789 1 2 3'/>") })),
            ("a transform function", json!({ "svg": svg("<g id='g' transform='abcXYZdefGHI(1'><circle r='5'/></g>") })),
        ];
        for (what, request) in cases {
            let why = message(&apply_masked(None, &request, &mask).expect_err(what));
            assert_eq!(piece(&why), None, "{what}: {why}");
        }
    }

    #[test]
    fn five_thousand_unknown_attributes_cost_little_and_give_twelve_kinds_at_most() {
        // Two-letter names, 32 on an element (as many as one may carry): 157 elements, 30 KiB.
        let names: Vec<String> = (b'a'..=b'z').flat_map(|a| (b'a'..=b'z').map(move |b| format!("{}{}", a as char, b as char))).filter(|n| n != "on" && n != "id").collect();
        let mut inner = String::new();
        let mut written = 0;
        while written < 5000 {
            inner.push_str("<g");
            for _ in 0..MAX_ATTRS {
                let _ = write!(inner, " {}=''", names[written % names.len()]);
                written += 1;
            }
            inner.push_str("/>");
        }
        let art = svg(&inner);
        assert!(art.len() <= MAX_SVG_BYTES, "{} bytes", art.len());
        let started = Instant::now();
        let problems = apply(None, &json!({ "svg": art })).unwrap_err();
        let took = started.elapsed();
        assert_eq!(problems.len(), MAX_PROBLEM_KINDS + 1);
        assert!(problems[0].starts_with("svg: \"aa\" is not an attribute of <g> (8 times, first at line 1 column 28): it takes id, transform, fill, stroke"), "{}", problems[0]);
        assert_eq!(problems[12], format!("and {} more kinds", names.len() - MAX_PROBLEM_KINDS));
        assert!(problems.iter().all(|p| p.chars().count() <= MAX_PROBLEM_CHARS));
        assert!(message(&problems).len() <= MAX_MESSAGE_BYTES);
        assert!(took < Duration::from_secs(2), "{took:?}");

        // All of them on one element: the limit of attributes is met first, and nothing more is read.
        let one = format!("<g{}/>", (0..5000).map(|i| format!(" {}=''", names[i % names.len()])).collect::<String>());
        let started = Instant::now();
        let problems = apply(None, &json!({ "svg": svg(&one) })).unwrap_err();
        assert_eq!(problems, ["svg: more than 32 attributes on <g> (at line 1 column 28): an element needs a handful"]);
        assert!(started.elapsed() < Duration::from_secs(2));

        // Attributes that are dropped, as many as fit into 32 KiB: one line of warnings.
        let mut inner = String::new();
        let mut dropped = 0;
        while inner.len() < MAX_SVG_BYTES - 800 {
            inner.push_str("<circle cx='50' cy='50' r='5'");
            for i in 0..MAX_ATTRS - 3 {
                let _ = write!(inner, " data-{i}=''");
                dropped += 1;
            }
            inner.push_str("/>");
        }
        assert!(dropped > 2000);
        let started = Instant::now();
        let applied = accepted(None, json!({ "svg": svg(&inner) }));
        assert_eq!(applied.warnings, [format!("Not drawn: data-* ({dropped})")]);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn at_most_twelve_warnings_are_given() {
        // Twenty groups, each turned by an animation of its own, none with a pivot.
        let groups: String = (0..20).map(|i| format!("<g id='g{i}'><circle cx='{}' cy='50' r='4'/></g>", 10 + i * 9)).collect();
        let animations: Map<String, Value> = (0..20).map(|i| (format!("a{i:02}"), rotate(&format!("g{i}")))).collect();
        let applied = accepted(None, json!({ "svg": svg(&format!("{groups}<title>t</title>")), "animations": animations }));
        assert_eq!(applied.warnings.len(), MAX_WARNINGS);
        assert_eq!(applied.warnings[0], "Not drawn: title (1)");
        has(&applied.warnings[1], "g0 has no pivot");
        assert_eq!(applied.warnings[11], "and 10 more warnings");
        assert!(applied.warnings.iter().all(|w| w.chars().count() <= MAX_PROBLEM_CHARS));
        // Exactly twelve are all shown.
        let animations: Map<String, Value> = (0..11).map(|i| (format!("a{i:02}"), rotate(&format!("g{i}")))).collect();
        let applied = accepted(None, json!({ "svg": svg(&format!("{groups}<title>t</title>")), "animations": animations }));
        assert_eq!(applied.warnings.len(), 12);
        has(&applied.warnings[11], "g10 has no pivot");
    }

    // ------------------------------------------------------------ written back, stored, the examples

    /// The look that the look's own artwork and moving parts apply to.
    #[track_caller]
    fn round_trip(look: &Look) -> Look {
        let request = json!({ "svg": to_svg(look), "pivots": look.pivots, "parents": look.parents, "roles": look.roles, "animations": look.animations, "states": look.states, "mouth": look.mouth, "gaze": look.gaze, "drives": look.drives });
        accepted(None, request).design.look.unwrap()
    }

    #[test]
    fn the_artwork_written_back_as_svg_applies_to_the_same_look() {
        let look = accepted(
            None,
            figure(json!({ "pivots": { "head": [100, 110] }, "roles": { "head": "head" }, "animations": { "nod": rotate("head") }, "states": { "done": ["nod"] }, "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96" }, "gaze": 3 })),
        )
        .design
        .look
        .unwrap();
        let text = to_svg(&look);
        assert!(text.starts_with("<svg viewBox='0 0 200 200'><defs><linearGradient id='sky'><stop offset='0' stop-color='#ffffff'/><stop offset='1' stop-color='#000080'/></linearGradient><clipPath id='window'><circle id='pane' cx='100' cy='100' r='50'/></clipPath></defs><g id='body'>"), "{text}");
        has(&text, "<ellipse id='belly' cx='100' cy='140' fill='url(#sky)' rx='40' ry='30'/>");
        has(&text, "<text id='label' font-size='10' text-anchor='middle' x='100' y='190'>hi</text>");
        has(&text, "<path id='mouth' d='M 84 96 Q 100 104 116 96' fill='none' stroke='#333333' stroke-width='3'/>");
        assert!(text.ends_with("</g></svg>"));
        assert!(!text.contains('"'), "single quotes only: the text sits in a JSON string without a backslash");
        assert_eq!(round_trip(&look), look);

        // Everything the allow-list holds, a wrapped root, text that needs escaping, transforms, percentages.
        let rich = accepted(
            None,
            json!({ "svg": "<svg viewBox='-10 -10 120.5 100' fill='none' stroke='currentColor'><defs><radialGradient id='rg' cx='30%' cy='30%' r='70%' fx='0.2' gradientUnits='userSpaceOnUse' gradientTransform='rotate(10) scale(1 0.5)' spreadMethod='repeat'><stop offset='10%' stop-color='rgba(255,0,0,0.5)' stop-opacity='0.5'/><stop offset='1' stop-color='currentColor'/></radialGradient><clipPath id='cp' clipPathUnits='objectBoundingBox'><polygon points='0,0 1,0 1,1'/></clipPath></defs><g id='g' transform='translate(5 5) rotate(-10 50 50)' clip-path='url(#cp)' opacity='0.5'><polyline id='pl' points='0 0 10 10 20 0' stroke='url(#rg)' stroke-linejoin='round'/><text id='t' x='10' y='40' font-size='8' font-weight='600' letter-spacing='0.5'>a &lt;b&gt; &amp; 'c' \"d\"</text><path id='arc' d='M10 10 a5 5 0 1 0 10 0 z' fill-rule='evenodd'/></g></svg>" }),
        )
        .design
        .look
        .unwrap();
        assert_eq!(node(&rich, "t").text.as_deref(), Some("a <b> & 'c' \"d\""));
        // A percentage of a gradient in user space is a share of the viewBox: of its width, its height, its diagonal.
        assert_eq!(attrs(node(&rich, "rg")), json!({ "cx": 36.15, "cy": 30, "r": 77.508, "fx": 0.2, "gradientUnits": "userSpaceOnUse", "gradientTransform": "rotate(10) scale(1 0.5)", "spreadMethod": "repeat" }));
        assert_eq!(attrs(&node(&rich, "rg").children[0]), json!({ "offset": 0.1, "stopColor": "#ff000080", "stopOpacity": 0.5 }));
        assert_eq!(attrs(&node(&rich, "rg").children[1]), json!({ "offset": 1, "stopColor": "currentColor" }));
        let text = to_svg(&rich);
        has(&text, "<svg viewBox='-10 -10 120.5 100'><g fill='none' stroke='currentColor'><defs>");
        has(&text, ">a &lt;b&gt; &amp; &apos;c&apos; &quot;d&quot;</text>");
        assert_eq!(round_trip(&rich), rich);
        // Written back and applied twice is the same text: the normal form is a fixed point.
        assert_eq!(to_svg(&round_trip(&rich)), text);
    }

    #[test]
    fn a_stored_design_goes_through_the_same_validator() {
        let design = accepted(
            None,
            figure(json!({
                "name": "Pip", "voice": { "lang": "en-GB", "pitch": 1.1 }, "pivots": { "head": [100, 110] }, "roles": { "head": "head" },
                "animations": { "nod": rotate("head") }, "states": { "done": ["nod"] }, "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96" }, "gaze": 3
            })),
        )
        .design;
        // As a store writes it: the design's fields among the record's own.
        let mut stored = serde_json::to_value(&design).unwrap();
        stored["version"] = json!(1);
        stored["on"] = json!(true);
        stored["designed"] = json!(true);
        stored["via"] = json!("tools");
        assert_eq!(check_stored(stored.clone()), Some(design.clone()));
        // The same through text, as it comes from a file.
        let from_text: Value = serde_json::from_str(&serde_json::to_string_pretty(&stored).unwrap()).unwrap();
        assert_eq!(check_stored(from_text), Some(design.clone()));
        // No look: the default look.
        assert_eq!(check_stored(json!({ "version": 1, "name": "Pip", "look": null, "voice": {} })), Some(Design { name: "Pip".into(), ..Design::default() }));
        assert_eq!(check_stored(json!({})), Some(Design::default()));
        for not in [json!(null), json!([]), json!("x"), json!({ "look": 5 }), json!({ "look": [] }), json!({ "name": 5 }), json!({ "voice": [] })] {
            assert_eq!(check_stored(not.clone()), None, "{not}");
        }

        // A hand-written file gets no further than an agent's call would.
        let tamper = |edit: &dyn Fn(&mut Value)| {
            let mut v = stored.clone();
            edit(&mut v);
            check_stored(v)
        };
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["attrs"]["onclick"] = json!("alert(1)")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["attrs"]["style"] = json!("fill:red")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["attrs"]["className"] = json!("wb-modal")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["attrs"]["dangerouslySetInnerHTML"] = json!({ "__html": "<img>" })), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["attrs"]["fillToken"] = json!("accent")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["tag"] = json!("script")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["children"][0]["attrs"]["fill"] = json!("url(http://x)")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["children"][0]["attrs"]["fillRef"] = json!("nope")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["children"][0]["attrs"]["fillRef"] = json!("sky' onload='alert(1)")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["children"][0]["attrs"]["cx"] = json!("100' onload='alert(1)")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["id"] = json!("constructor")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["id"] = json!("a'><script>alert(1)</script><g id='b")), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["extra"] = json!(1)), None);
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["text"] = json!("words in a group")), None);
        assert_eq!(tamper(&|v| v["look"]["animations"]["nod"]["tracks"][0]["target"] = json!("nowhere")), None);
        assert_eq!(tamper(&|v| v["look"]["animations"]["constructor"] = v["look"]["animations"]["nod"].clone()), None);
        assert_eq!(tamper(&|v| v["look"]["animations"]["nod"]["duration"] = json!(1e9)), None);
        assert_eq!(tamper(&|v| v["look"]["states"]["done"] = json!(["toString"])), None);
        assert_eq!(tamper(&|v| v["look"]["mouth"][0]["open"] = json!("M0 0 L1 1")), None);
        assert_eq!(tamper(&|v| v["look"]["gaze"] = json!(99)), None);
        assert_eq!(tamper(&|v| v["look"]["viewBox"] = json!([0, 0, 99999, 200])), None);
        assert_eq!(tamper(&|v| v["look"]["roles"]["tail"] = json!("arm")), None);
        assert_eq!(tamper(&|v| v["look"]["pivots"]["nowhere"] = json!([1, 2])), None);
        assert_eq!(tamper(&|v| v["name"] = json!("x".repeat(40))), None);
        assert_eq!(tamper(&|v| v["voice"]["pitch"] = json!(9)), None);
        // Nothing of the file is believed: the boxes are computed again.
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["box"] = json!([0, 0, 1, 1])), Some(design.clone()));
        // Text is text: markup in it stays words.
        let words = tamper(&|v| {
            let label = v["look"]["nodes"][1]["children"].as_array_mut().unwrap().last_mut().unwrap();
            label["text"] = json!("<b onclick='x'>&amp;</b>");
        })
        .unwrap();
        assert_eq!(node(words.look.as_ref().unwrap(), "label").text.as_deref(), Some("<b onclick='x'>&amp;</b>"));
        // Over the size of a stored look nothing is read.
        assert_eq!(tamper(&|v| v["look"]["nodes"][1]["children"][0]["attrs"]["pad"] = json!("x".repeat(MAX_LOOK_BYTES))), None);
        // A tree nested far deeper than a look may be is refused, not walked into the ground.
        let mut deep = json!({ "tag": "circle", "attrs": { "r": 5 } });
        for _ in 0..100 {
            deep = json!({ "tag": "g", "children": [deep] });
        }
        assert_eq!(tamper(&|v| v["look"]["nodes"] = json!([deep.clone()])), None);
    }

    /// The example designs and the looks `apply` makes of them. `hoot` draws with what the
    /// others do not: a use, a mask, a filter, dashes, parents and drives.
    ///
    /// `examples/*.look.json` are golden files: they are compared as typed values, never as text.
    /// After a deliberate change to the normal form, write them again with
    /// `WORKBENCH_UPDATE_GOLDEN=1 cargo test --locked avatar::look::tests::the_example_designs` and
    /// review the difference. The web's tests load the same files.
    #[test]
    fn the_example_designs_validate_without_warnings_and_equal_their_golden_looks() {
        let examples = [
            ("pip", include_str!("examples/pip.json"), include_str!("examples/pip.look.json")),
            ("tock", include_str!("examples/tock.json"), include_str!("examples/tock.look.json")),
            ("hoot", include_str!("examples/hoot.json"), include_str!("examples/hoot.look.json")),
        ];
        for (name, design, golden) in examples {
            let request: Value = serde_json::from_str(design).unwrap_or_else(|e| panic!("{name}.json is not JSON: {e}"));
            let applied = apply(None, &request).unwrap_or_else(|problems| panic!("{name} is refused: {problems:#?}"));
            assert_eq!(applied.warnings, [""; 0], "{name}");
            let look = applied.design.look.clone().expect("a look");
            if std::env::var_os("WORKBENCH_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("avatar").join("examples").join(format!("{name}.look.json"));
                std::fs::write(&path, serde_json::to_string_pretty(&look).unwrap() + "\n").unwrap();
                continue;
            }
            let golden: Look = serde_json::from_str(golden).unwrap_or_else(|e| panic!("{name}.look.json is not a look: {e}"));
            assert_eq!(look, golden, "{name}: the look differs from examples/{name}.look.json");
            // The normal form is a fixed point, through SVG text and through a stored file.
            assert_eq!(round_trip(&look), look, "{name}");
            assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()), Some(applied.design.clone()), "{name}");
            // Every built-in applies to each: they name all six roles.
            assert_eq!(builtins(&look), BUILTIN, "{name}");
            assert!(applied.usage.svg_bytes < MAX_SVG_BYTES / 2 && applied.usage.elements < MAX_ELEMENTS / 2 && applied.usage.look_bytes < MAX_LOOK_BYTES / 2, "{name}: {:?}", applied.usage);
            assert!(applied.parts.iter().all(|p| p.bbox[0] >= 0.0 && p.bbox[1] >= 0.0 && p.bbox[0] + p.bbox[2] <= 200.0 && p.bbox[1] + p.bbox[3] <= 200.0), "{name}");
        }
        // What hoot holds of this round's artwork and rig, so the browser's golden test meets all of it.
        let hoot: Look = serde_json::from_str(include_str!("examples/hoot.look.json")).unwrap();
        let mut tags = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        let mut stack: Vec<&LookNode> = hoot.nodes.iter().collect();
        while let Some(n) = stack.pop() {
            tags.insert(n.tag.name());
            keys.extend(n.attrs.keys().map(String::as_str));
            stack.extend(&n.children);
        }
        for tag in ["use", "mask", "filter", "feGaussianBlur", "feOffset", "feFlood", "feComposite", "feMerge", "feMergeNode"] {
            assert!(tags.contains(tag), "hoot has no {tag}");
        }
        for key in ["useRef", "maskRef", "filterRef", "strokeDasharray", "in", "in2", "result", "stdDeviation", "floodColor", "floodOpacity", "operator"] {
            assert!(keys.contains(key), "hoot has no {key}");
        }
        assert_eq!(hoot.parents.len(), 3);
        let inputs: std::collections::BTreeSet<&str> = hoot.drives.iter().map(|d| d.input.name()).collect();
        assert_eq!(inputs, ["held", "lookX", "pace", "turn"].into());
        assert!(hoot.drives.iter().any(|d| d.input == Input::Turn && d.prop == Prop::Opacity) && hoot.drives.iter().any(|d| d.input == Input::LookX && d.prop == Prop::Rotate));
        let eases: Vec<&Eases> = hoot.animations.values().flat_map(|a| a.ease.iter().chain(a.tracks.iter().flat_map(|t| &t.ease))).chain(hoot.drives.iter().flat_map(|d| &d.ease)).collect();
        assert!(eases.iter().any(|e| matches!(e, Eases::One(Curve::Bezier(_)) | Eases::Each(_))) && eases.iter().any(|e| matches!(e, Eases::One(Curve::Named(Ease::InOutBack | Ease::OutBounce | Ease::OutBack)))));
        assert!(hoot.animations.values().flat_map(|a| &a.tracks).any(|t| t.prop == Prop::SkewX) && hoot.animations.contains_key("walk"));
    }

    #[test]
    fn the_wire_types_have_the_shapes_of_the_contract() {
        let look = accepted(None, figure(json!({ "pivots": { "head": [100, 110] }, "roles": { "head": "head" }, "animations": { "nod": rotate("head") } }))).design.look.unwrap();
        let v = serde_json::to_value(&look).unwrap();
        // Every field of a look is always there.
        assert_eq!(v.as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(), ["animations", "gaze", "mouth", "nodes", "pivots", "roles", "states", "viewBox"].into());
        assert_eq!((&v["gaze"], &v["mouth"], &v["states"], &v["viewBox"]), (&Value::Null, &json!([]), &json!({}), &json!([0, 0, 200, 200])));
        // A node carries only what it has.
        let body = &v["nodes"][1];
        assert_eq!((&body["tag"], &body["id"], &body["box"]), (&json!("g"), &json!("body"), &json!([60, 50, 92, 142])));
        assert!(body.get("attrs").is_none() && body.get("text").is_none() && body.get("bbox").is_none());
        let label = body["children"].as_array().unwrap().last().unwrap();
        assert_eq!(label, &json!({ "tag": "text", "id": "label", "attrs": { "fontSize": 10, "textAnchor": "middle", "x": 100, "y": 190 }, "box": [94, 182, 12, 10], "text": "hi" }));
        // Reading is strict: nothing unknown gets in.
        assert_eq!(serde_json::from_value::<Look>(v.clone()).unwrap(), look);
        for (path, extra) in [("", "script"), ("/nodes/1", "onclick"), ("/animations/nod", "script"), ("/animations/nod/tracks/0", "origin")] {
            let mut tampered = v.clone();
            tampered.pointer_mut(path).unwrap()[extra] = json!("x");
            assert!(serde_json::from_value::<Look>(tampered).is_err(), "{path}");
        }
        let mut wrong_tag = v.clone();
        wrong_tag["nodes"][1]["tag"] = json!("foreignObject");
        assert!(serde_json::from_value::<Look>(wrong_tag).is_err());
        // The two forms of a mouth drive.
        let drives: Vec<MouthDrive> = serde_json::from_value(json!([{ "target": "m", "open": "M 0 0" }, { "target": "m", "prop": "scaleY", "open": 2, "closed": 1 }])).unwrap();
        assert_eq!(drives, [
            MouthDrive::Path { target: "m".into(), open: "M 0 0".into(), closed: None },
            MouthDrive::Prop { target: "m".into(), prop: MouthProp::ScaleY, open: Num::new(2.0), closed: Some(Num::new(1.0)) },
        ]);
        assert_eq!(serde_json::to_value(&drives).unwrap(), json!([{ "target": "m", "open": "M 0 0" }, { "target": "m", "prop": "scaleY", "open": 2, "closed": 1 }]));
        assert!(serde_json::from_value::<MouthDrive>(json!({ "target": "m", "open": "M 0 0", "origin": [1, 2] })).is_err());
        assert!(serde_json::from_value::<MouthDrive>(json!({ "target": "m", "prop": "rotate", "open": 2 })).is_err());
        // Names on the wire.
        for tag in Tag::ALL {
            assert_eq!(serde_json::to_value(tag).unwrap(), json!(tag.name()));
        }
        for prop in Prop::ALL {
            assert_eq!(serde_json::to_value(prop).unwrap(), json!(prop.name()));
        }
        for ease in Ease::ALL {
            assert_eq!(serde_json::to_value(ease).unwrap(), json!(ease.name()));
        }
        for role in Role::ALL {
            assert_eq!(serde_json::to_value(role).unwrap(), json!(role.name()));
        }
        for state in LookState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(state.name()));
        }
        for prop in MouthProp::ALL {
            assert_eq!(serde_json::to_value(prop).unwrap(), json!(prop.prop().name()));
        }
        assert_eq!(serde_json::to_value(Design::default()).unwrap(), json!({ "name": "", "look": null, "voice": {} }));
    }

    #[test]
    fn no_input_makes_the_validator_panic() {
        let pip: Value = serde_json::from_str(include_str!("examples/pip.json")).unwrap();
        let art = pip["svg"].as_str().unwrap();
        let cuts: Vec<usize> = (0..art.len()).filter(|i| art.is_char_boundary(*i)).collect();
        // Prefixes of real artwork, cut at every eleventh character.
        for end in cuts.iter().step_by(11) {
            let _ = apply(None, &json!({ "svg": &art[..*end] }));
        }
        // What matters to XML, CSS and numbers, put in at many places.
        let pieces = ["<", ">", "&", "'", "\"", "\u{0}", "\u{202E}", "é", "%", "e", "-", ".", "/", "\\", "(", ")", "#", ";", ":", "]]>", "<!--", "&#x0;", "&#xFFFFFFFF;", "&#;", "\u{FEFF}", "<![CDATA[", "<?", "='", " x"];
        for (n, piece) in pieces.iter().enumerate() {
            for at in cuts.iter().skip(n).step_by(97) {
                let _ = apply(None, &json!({ "svg": format!("{}{piece}{}", &art[..*at], &art[*at..]) }));
            }
        }
        // A value of every JSON kind in every place of a call.
        let kinds = [json!(null), json!(true), json!(-1), json!(1e300), json!(0.5), json!(""), json!("constructor"), json!("\u{0}\u{202E}é"), json!([]), json!([[]]), json!([null, 1, "a"]), json!({}), json!({ "a": { "b": [] } })];
        for kind in &kinds {
            for key in KEYS {
                let mut request = pip.clone();
                request[key] = kind.clone();
                let _ = apply(None, &request);
            }
            let _ = check_stored(json!({ "look": kind, "name": kind, "voice": kind }));
            let _ = check_stored(kind.clone());
            let nested = [
                json!({ "animations": { "x": kind } }),
                json!({ "animations": { "x": { "duration": kind, "loop": kind, "every": kind, "times": kind, "ease": kind, "tracks": kind } } }),
                json!({ "animations": { "x": { "duration": 1, "tracks": [kind] } } }),
                json!({ "animations": { "x": { "duration": 1, "tracks": [{ "target": kind, "prop": kind, "values": kind, "times": kind, "ease": kind, "phase": kind, "delay": kind }] } } }),
                json!({ "animations": { "x": { "duration": 1, "tracks": [{ "target": "frog", "prop": "rotate", "values": [kind, kind], "times": [kind, kind], "ease": [kind] }] } } }),
                json!({ "animations": { "x": { "duration": 1, "tracks": [{ "target": [kind, "frog"], "prop": "d", "values": [kind, "M0 0"] }] } } }),
                json!({ "mouth": { "target": kind, "open": kind, "closed": kind, "prop": kind } }),
                json!({ "mouth": [kind, { "target": "mouth", "open": kind }, { "target": "frog", "prop": "scaleY", "open": kind, "closed": kind }] }),
                json!({ "pivots": { "frog": kind }, "roles": { "body": kind }, "states": { "idle": kind } }),
                json!({ "states": { "idle": [kind] }, "voice": { "lang": kind, "prefer": kind, "pitch": kind, "rate": kind } }),
                json!({ "voice": { "prefer": [kind] } }),
                json!({ "parents": { "frog": kind, kind.as_str().unwrap_or("eyes"): "frog" } }),
                json!({ "drives": [kind, { "input": kind, "target": kind, "prop": kind, "values": kind, "at": kind, "ease": kind }] }),
                json!({ "drives": [{ "input": "lookX", "target": "frog", "prop": "rotate", "values": [kind, kind], "at": [kind, kind], "ease": [kind, [kind, 0, 1, 1]] }] }),
            ];
            for more in nested {
                let mut request = pip.clone();
                for (key, value) in more.as_object().unwrap() {
                    request[key.as_str()] = value.clone();
                }
                let _ = apply(None, &request);
            }
        }
        // Numbers at the edges of what a double holds, in every kind of value.
        for n in ["1e308", "-1e308", "1e-320", "0.0000000000000001", "9007199254740993", "-0", "00012", "1e", "+", "--1", "1..2", ".", "1e+", "٣"] {
            let _ = apply(None, &json!({ "svg": svg(&format!("<circle cx='{n}' r='{n}' opacity='{n}' transform='rotate({n}) scale({n})'/><path d='M{n} {n} A{n} {n} {n} 1 1 {n} {n}'/><polygon points='{n} {n}'/><text font-size='{n}' x='{n}'>a</text>")) }));
            let _ = colour(&format!("rgb({n} {n} {n} / {n})"));
            let _ = colour(&format!("hsl({n}, {n}%, {n}%)"));
        }
    }

    /// The same numbers on every run and every platform: a sweep, not a gamble.
    fn sequence(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below.max(1) as u64) as usize
        }
    }

    /// What every answer of the validator holds to, whatever it was given.
    #[track_caller]
    fn holds_its_promises(request: &Value) -> bool {
        match apply(None, request) {
            Ok(applied) => {
                assert!(applied.warnings.len() <= MAX_WARNINGS && applied.warnings.iter().all(|w| w.chars().count() <= MAX_PROBLEM_CHARS));
                if let Some(look) = &applied.design.look {
                    assert!(json_len(look) <= MAX_LOOK_BYTES);
                    // No number is larger than the browser takes, and the artwork can be sent again as it is given back.
                    assert!(numbers(look).iter().all(|n| n.abs() <= MAX_SPAN), "{:?}", numbers(look));
                    assert!(to_svg(look).len() <= MAX_SVG_BYTES);
                    // Written back and applied again it is the same look; so is a stored copy of it.
                    assert_eq!(&round_trip(look), look);
                    assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()).as_ref(), Some(&applied.design));
                    // A merge of nothing changes nothing, and the same artwork with keep keeps everything.
                    assert_eq!(accepted(Some(&applied.design), json!({ "pivots": {} })).design, applied.design);
                    let kept = accepted(Some(&applied.design), json!({ "svg": request["svg"], "keep": true }));
                    assert_eq!((&kept.design.look, kept.cleared.len()), (&applied.design.look, 0));
                }
                true
            }
            Err(problems) => {
                assert!(!problems.is_empty() && problems.len() <= MAX_PROBLEM_KINDS + 1);
                assert!(problems.iter().all(|p| p.chars().count() <= MAX_PROBLEM_CHARS && !p.chars().any(|c| c.is_control())), "{problems:?}");
                false
            }
        }
    }

    #[test]
    fn whatever_is_accepted_is_a_fixed_point_and_whatever_is_refused_says_so_within_bounds() {
        let designs: Vec<Value> = [include_str!("examples/pip.json"), include_str!("examples/tock.json"), include_str!("examples/hoot.json")].iter().map(|d| serde_json::from_str(d).unwrap()).collect();
        let mut pick = sequence(0x9E37_79B9_7F4A_7C15);
        let mut accepted_count = 0;

        // Artwork with a few characters put in, taken out or doubled.
        let pieces = ["<", ">", "/", "=", "'", "&", ";", "#", "(", ")", " ", "-", ".", ",", "%", "e", "0", "9", "url(", "M", "A", "z", "g", "text", "style", "id", "d", "fill", "<!--", "-->", "&#x", "&amp;", "\u{202E}", "é", "1e999", "''", " on", "none", "px"];
        for round in 0..1200 {
            let mut request = designs[round % designs.len()].clone();
            let mut art = request["svg"].as_str().unwrap().to_string();
            for _ in 0..1 + pick(3) {
                let cut = |art: &str, at: usize| (0..=at.min(art.len())).rev().find(|i| art.is_char_boundary(*i)).unwrap_or(0);
                let at = cut(&art, pick(art.len() + 1));
                let end = cut(&art, at + pick(12)).max(at);
                match pick(3) {
                    0 => art.insert_str(at, pieces[pick(pieces.len())]),
                    1 => art.replace_range(at..end, ""),
                    _ => {
                        let piece = art[at..end].to_string();
                        art.insert_str(at, &piece);
                    }
                }
            }
            request["svg"] = json!(art);
            accepted_count += usize::from(holds_its_promises(&request));
        }

        // What moves the artwork, with values replaced, removed or nudged.
        let pool = [
            json!(null), json!(true), json!(0), json!(1), json!(-1), json!(0.5), json!(0.17), json!(0.04), json!(31), json!(1e9), json!(""), json!("frog"), json!("bot"), json!("eyes"), json!("mouth"), json!("skin"), json!("nope"),
            json!("constructor"), json!("rotate"), json!("d"), json!("fill"), json!("opacity"), json!("scaleY"), json!("step"), json!("inOut"), json!("#ff0000"), json!("none"), json!("M0 0 L1 1"), json!("blink"), json!("hop"),
            json!([]), json!([0, 1]), json!([0, 0.5, 1]), json!([null, null]), json!(["frog", "eyes"]), json!(["in", "out"]), json!({}), json!({ "target": "frog", "prop": "rotate", "values": [0, 1] }),
        ];
        for round in 0..500 {
            let mut request = designs[round % designs.len()].clone();
            for _ in 0..1 + pick(2) {
                // Every place of a value below the top, as a JSON pointer; the artwork stays.
                let mut places: Vec<String> = Vec::new();
                let mut walk: Vec<(String, &Value)> = request.as_object().unwrap().iter().filter(|(k, _)| *k != "svg").map(|(k, v)| (format!("/{k}"), v)).collect();
                while let Some((at, value)) = walk.pop() {
                    match value {
                        Value::Object(o) => walk.extend(o.iter().map(|(k, v)| (format!("{at}/{k}"), v))),
                        Value::Array(a) => walk.extend(a.iter().enumerate().map(|(i, v)| (format!("{at}/{i}"), v))),
                        _ => {}
                    }
                    places.push(at);
                }
                places.sort();
                let place = places[pick(places.len())].clone();
                let value = pool[pick(pool.len())].clone();
                let how = pick(4);
                let (parent, key) = place.rsplit_once('/').unwrap();
                if how == 0 {
                    if let Some(Value::Object(o)) = request.pointer_mut(parent) {
                        o.remove(key);
                    }
                } else if let Some(slot) = request.pointer_mut(&place) {
                    *slot = match (how, slot.as_f64()) {
                        (1, Some(n)) => json!(n * [0.0, 0.5, 0.999, 1.001, 2.0, -1.0, 1000.0][pick(7)]),
                        _ => value,
                    };
                }
            }
            accepted_count += usize::from(holds_its_promises(&request));
        }
        // The sweep is worth its name only while a fair share of it is accepted.
        assert!(accepted_count > 100, "{accepted_count} accepted");
    }
    // ------------------------------------------------------------ the adversary: reader and names

    #[test]
    fn a_prefixed_attribute_refuses_wherever_it_stands_and_whatever_follows_the_prefix() {
        // On a tspan, whose other attributes are not read.
        has(&undrawn("<text>a<tspan x:onload='alert(1)'>b</tspan></text>"), "svg: the prefixed attribute \"x:onload\"");
        has(&undrawn("<text>a<tspan xml:base='http://x/'>b</tspan></text>"), "svg: the prefixed attribute \"xml:base\"");
        has(&undrawn("<text>a<tspan xlink:href='#a'>b</tspan></text>"), "svg: xlink:href on <tspan>");
        has(&undrawn("<text>a<tspan><tspan ev:event='x'>b</tspan></tspan></text>"), "svg: the prefixed attribute \"ev:event\"");
        // Behind a name that would be dropped: the prefix decides.
        has(&undrawn("<circle r='5' data-a:onload='alert(1)'/>"), "svg: the prefixed attribute \"data-a:onload\"");
        has(&undrawn("<circle r='5' aria-x:href='x'/>"), "svg: the prefixed attribute \"aria-x:href\"");
        has(&undrawn("<circle r='5' class:x='y'/>"), "svg: the prefixed attribute \"class:x\"");
        // The xml prefix has two harmless names; the others refuse like any prefix.
        has(&undrawn("<circle r='5' xml:base='http://x/'/>"), "svg: the prefixed attribute \"xml:base\"");
        has(&undrawn("<circle r='5' xml:id='a'/>"), "svg: the prefixed attribute \"xml:id\"");
        has(&refused(None, json!({ "svg": "<svg xml:base='http://x/'><circle r='5'/></svg>" })), "svg: the prefixed attribute \"xml:base\"");
        has(&undrawn("<circle r='5' :r='6'/>"), "svg: the prefixed attribute \":r\"");
        has(&undrawn("<circle r='5' r:='6'/>"), "svg: the prefixed attribute \"r:\"");
        // What is ignored is ignored on a tspan too.
        let look = drawn("<text id='t' x='5' y='50'>a<tspan xml:space='preserve' xml:lang='en' xmlns:x='y'>b</tspan></text>");
        assert_eq!(node(&look, "t").text.as_deref(), Some("ab"));
        // Not well-formed attributes of a tspan refuse, as anywhere.
        has(&undrawn("<text>a<tspan x='1' x='2'>b</tspan></text>"), "the attributes of <tspan> are not well-formed");
        has(&undrawn("<text>a<tspan x=1>b</tspan></text>"), "the attributes of <tspan> are not well-formed");
        has(&undrawn(&format!("<text>a<tspan {}>b</tspan></text>", (0..=MAX_ATTRS).map(|i| format!("data-a{i}='1'")).collect::<Vec<_>>().join(" "))), "the attributes of <tspan> are not well-formed");
        for on in ["onclick", "ONLOAD", "OnBegin", "on", "onx"] {
            has(&undrawn(&format!("<text>a<tspan {on}='x'>b</tspan></text>")), "svg: the event attribute");
            has(&undrawn(&format!("<circle r='5' {on}='x'/>")), "svg: the event attribute");
            has(&refused(None, json!({ "svg": format!("<svg {on}='x'><circle r='5'/></svg>") })), "svg: the event attribute");
        }
    }

    #[test]
    fn names_that_differ_from_the_table_by_case_or_spelling_refuse() {
        for (inner, why) in [
            ("<circle ID='a' r='5'/>", "\"ID\" is not an attribute of <circle>"),
            ("<circle Id='a' r='5'/>", "\"Id\" is not an attribute of <circle>"),
            ("<circle r='5' STYLE='fill:red'/>", "\"STYLE\" is not an attribute of <circle>"),
            ("<circle r='5' Style='fill:red'/>", "\"Style\" is not an attribute of <circle>"),
            ("<circle r='5' Fill='red'/>", "\"Fill\" is not an attribute of <circle>"),
            ("<circle r='5' FILL='red'/>", "\"FILL\" is not an attribute of <circle>"),
            ("<circle r='5' strokewidth='2'/>", "\"strokewidth\" is not an attribute of <circle>"),
            ("<circle r='5' stroke_width='2'/>", "\"stroke_width\" is not an attribute of <circle>"),
            ("<circle r='5' Stroke-Width='2'/>", "\"Stroke-Width\" is not an attribute of <circle>"),
            ("<circle r='5' StrokeWidth='2'/>", "\"StrokeWidth\" is not an attribute of <circle>"),
            ("<circle r='5' HREF='x'/>", "\"HREF\" is not an attribute of <circle>"),
            ("<circle r='5' Href='x'/>", "\"Href\" is not an attribute of <circle>"),
            ("<circle r='5' xlink:Href='x'/>", "the prefixed attribute \"xlink:Href\""),
            ("<circle r='5' XLINK:HREF='x'/>", "the prefixed attribute \"XLINK:HREF\""),
            ("<circle r='5' Mask='url(#m)'/>", "\"Mask\" is not an attribute of <circle>"),
            ("<circle r='5' Transform='scale(2)'/>", "\"Transform\" is not an attribute of <circle>"),
            ("<path D='M0 0L1 1'/>", "\"D\" is not an attribute of <path>"),
            ("<defs><linearGradient id='a' gradient-units='userSpaceOnUse'/></defs><circle r='5'/>", "\"gradient-units\" is not an attribute of <linearGradient>"),
            ("<defs><linearGradient id='a' gradientunits='userSpaceOnUse'/></defs><circle r='5'/>", "\"gradientunits\" is not an attribute of <linearGradient>"),
            ("<defs><linearGradient id='a' spread-method='pad'/></defs><circle r='5'/>", "\"spread-method\" is not an attribute of <linearGradient>"),
            ("<defs><clipPath id='a' clip-path-units='userSpaceOnUse'><circle r='5'/></clipPath></defs><circle r='5'/>", "\"clip-path-units\" is not an attribute of <clipPath>"),
            ("<defs><clippath id='a'><circle r='5'/></clippath></defs><circle r='5'/>", "the element \"clippath\" is not supported"),
            ("<defs><lineargradient id='a'/></defs><circle r='5'/>", "the element \"lineargradient\" is not supported"),
            ("<defs><LinearGradient id='a'/></defs><circle r='5'/>", "the element \"LinearGradient\" is not supported"),
            ("<G><circle r='5'/></G>", "the element \"G\" is not supported"),
            ("<Path d='M0 0L1 1'/>", "the element \"Path\" is not supported"),
            ("<TEXT>a</TEXT>", "the element \"TEXT\" is not supported"),
            ("<text>a<TSPAN>b</TSPAN></text>", "\"TSPAN\" inside a text"),
            ("<SCRIPT>alert(1)</SCRIPT>", "the element \"SCRIPT\" is not supported"),
            ("<Title>t</Title><circle r='5'/>", "the element \"Title\" is not supported"),
            ("<g/onload='x'><circle r='5'/></g/onload='x'>", "is not supported"),
            ("<circle r='5' \"onload\"='x'/>", "is not an attribute of <circle>"),
            ("<circle r='5' font-size='12'/>", "\"font-size\" is not an attribute of <circle>"),
            ("<g font-size='12'><text>a</text></g>", "\"font-size\" is not an attribute of <g>"),
            ("<circle r='5' rx='2'/>", "\"rx\" is not an attribute of <circle>"),
            ("<ellipse r='5'/>", "\"r\" is not an attribute of <ellipse>"),
            ("<circle r='5' width='5'/>", "\"width\" is not an attribute of <circle>"),
            ("<g x='1'><circle r='5'/></g>", "\"x\" is not an attribute of <g>"),
            ("<polygon d='M0 0L1 1'/>", "\"d\" is not an attribute of <polygon>"),
            ("<path points='0 0 1 1'/>", "\"points\" is not an attribute of <path>"),
            ("<defs id='d'/><circle r='5'/>", "\"id\" is not an attribute of <defs>"),
            ("<defs><linearGradient id='a'><stop id='s' offset='0'/></linearGradient></defs><circle r='5'/>", "\"id\" is not an attribute of <stop>"),
            ("<defs><linearGradient id='a' fx='1'/></defs><circle r='5'/>", "\"fx\" is not an attribute of <linearGradient>"),
            ("<defs><radialGradient id='a' x1='1'/></defs><circle r='5'/>", "\"x1\" is not an attribute of <radialGradient>"),
            ("<defs><linearGradient id='a' opacity='0.5'/></defs><circle r='5'/>", "\"opacity\" is not an attribute of <linearGradient>"),
            ("<defs><linearGradient id='a' href='#b'/></defs><circle r='5'/>", "svg: href on <linearGradient>"),
            ("<defs><clipPath id='a' transform='scale(2)'><circle r='5'/></clipPath></defs><circle r='5'/>", "\"transform\" is not an attribute of <clipPath>"),
            ("<defs><linearGradient id='a'><stop offset='0' fill='red'/></linearGradient></defs><circle r='5'/>", "\"fill\" is not an attribute of <stop>"),
        ] {
            has(&undrawn(inner), why);
        }
        for (root, why) in [("viewbox='0 0 20 20'", "\"viewbox\" is not an attribute of <svg>"), ("VIEWBOX='0 0 20 20'", "\"VIEWBOX\""), ("view-box='0 0 20 20'", "\"view-box\""), ("x='0'", "\"x\" is not an attribute of <svg>"), ("Width='5'", "\"Width\"")] {
            has(&refused(None, json!({ "svg": format!("<svg {root}><circle r='5'/></svg>") })), why);
        }
        // Values of a closed choice are compared without regard to case and stored in the table's spelling.
        let look = drawn("<defs><linearGradient id='lg' gradientUnits='USERSPACEONUSE' x1='50%' spreadMethod='Reflect'/></defs><path id='p' d='M0 0L5 5' stroke-linecap=' ROUND ' fill-rule='EvenOdd' fill='NONE' stroke='CURRENTCOLOR'/>");
        assert_eq!(attrs(node(&look, "lg")), json!({ "gradientUnits": "userSpaceOnUse", "spreadMethod": "reflect", "x1": 100 }));
        assert_eq!(attrs(node(&look, "p")), json!({ "d": "M 0 0 L 5 5", "strokeLinecap": "round", "fillRule": "evenodd", "fill": "none", "stroke": "currentColor" }));
    }

    #[test]
    fn an_attribute_given_twice_in_any_spelling_refuses() {
        has(&undrawn("<circle id='a' id='b' r='5'/>"), "a name is given twice");
        has(&undrawn("<circle r='5' style='fill:red' style='fill:blue'/>"), "a name is given twice");
        has(&undrawn("<circle r='5' fill='red' fill='red'/>"), "a name is given twice");
        has(&undrawn("<circle r='5' stroke-linecap='round' strokeLinecap='round'/>"), "stroke-linecap is given twice on <circle>, once as strokeLinecap");
        has(&undrawn("<circle r='5' strokeLinecap='round' stroke-linecap='round'/>"), "stroke-linecap is given twice");
        has(&undrawn("<text font-size='12' fontSize='12'>a</text>"), "font-size is given twice");
        has(&undrawn("<circle r='5' clip-path='none' clipPath='none'/>"), "clip-path is given twice");
        has(&undrawn("<defs><linearGradient id='a'><stop offset='0' stop-color='red' stopColor='red'/></linearGradient></defs><circle r='5'/>"), "stop-color is given twice");
        has(&refused(None, json!({ "svg": "<svg fill='red' fill='blue'><circle r='5'/></svg>" })), "a name is given twice");
        has(&refused(None, json!({ "svg": "<svg stroke-width='1' strokeWidth='2'><circle r='5'/></svg>" })), "stroke-width is given twice on <svg>");
        // A declaration in style is not a second attribute: it wins, and the last declaration of a property wins.
        let look = drawn("<circle id='c' r='5' strokeWidth='1' style='stroke-width:2;stroke-width:3;fill:red;FILL:blue'/>");
        assert_eq!(attrs(node(&look, "c")), json!({ "r": 5, "strokeWidth": 3, "fill": "#0000ff" }));
        // A dropped attribute and its camelCase spelling are both just dropped.
        assert_eq!(accepted(None, json!({ "svg": svg("<circle r='5' cx='9' cy='9' class='a' className='b'/>") })).warnings, ["Not drawn: class (2)"]);
    }

    #[test]
    fn style_keeps_only_declarations_of_the_table_and_nothing_else_gets_through() {
        let applied = accepted(
            None,
            json!({ "svg": svg(
                "<defs><linearGradient id='lg'/><clipPath id='cp'><circle r='50'/></clipPath></defs>\
                 <circle id='a' cx='50' cy='50' r='5' style='fill:URL( \"#lg\" );clip-path:url(#cp)'/>\
                 <circle id='b' cx='50' cy='50' r='5' style='stroke_width:9;strokeWidth:9;-webkit-stroke-width:9;stroke-width :2; stroke : #ABC ;;;'/>\
                 <circle id='c' cx='50' cy='50' r='5' style='transform:scale(9);d:path(\"M0 0\");r:99;cx:99;id:evil;--x:url(#lg);x:url(#cp);:red;display:none;visibility:hidden'/>\
                 <circle id='d' cx='50' cy='50' r='5' style='fill:rgb(1,2,3);stroke:hsl(120,100%,50%)\n;opacity:.5'/>"
            ) }),
        );
        let look = applied.design.look.unwrap();
        assert_eq!(attrs(node(&look, "a")), json!({ "cx": 50, "cy": 50, "r": 5, "fillRef": "lg", "clipPathRef": "cp" }));
        assert_eq!(attrs(node(&look, "b")), json!({ "cx": 50, "cy": 50, "r": 5, "strokeWidth": 2, "stroke": "#aabbcc" }));
        // Geometry, the id and a transform are not CSS here: the attributes stand.
        assert_eq!(attrs(node(&look, "c")), json!({ "cx": 50, "cy": 50, "r": 5 }));
        assert_eq!(attrs(node(&look, "d")), json!({ "cx": 50, "cy": 50, "r": 5, "fill": "#010203", "stroke": "#00ff00", "opacity": 0.5 }));
        assert_eq!(applied.warnings, ["Not drawn: other style properties (13)"]);
        for (style, why) in [
            ("fill:red;background:url(//x)", "style holds a url() that is not url(#id)"),
            ("fill:url(#a);fill:url(http://x)", "style holds a url() that is not url(#id)"),
            ("fill:url(#a;b)", "style holds a url() that is not url(#id)"),
            ("behavior:url(#default#time2)", "style holds a url() that is not url(#id)"),
            ("@import url(x)", "style holds a url() that is not url(#id)"),
            ("fill:UrL(javascript:alert(1))", "style holds a url() that is not url(#id)"),
            ("fill:url(  )", "style holds a url() that is not url(#id)"),
            ("fill:url(#a", "style holds a url() that is not url(#id)"),
            ("fill:red ! important", "style holds !important"),
            ("fill:red !IMPORTANT", "style holds !important"),
            ("fill:red/**/", "style holds a comment"),
            ("fill:r\\65 d", "style holds an escape"),
            ("fill:var(--x)", "fill \"var(--x)\" is not a colour"),
            ("fill:attr(x)", "is not a colour"),
            ("fill:expression(alert(1))", "is not a colour"),
            ("fill:", "is not a colour"),
            ("fill:red/ *", "is not a colour"),
            ("font-family:'a;fill:red'", "is not a colour"),
            ("fill:image-set('http://x' 1x)", "is not a colour"),
            ("opacity:0.5;opacity:half", "opacity \"half\" is not a number"),
            ("stroke-width:2pt", "\"2pt\" is not a number"),
            ("clip-path:circle(50%)", "svg: clip-path \"circle(50%)\""),
            ("fill:url(#nope)", "fill refers to #nope, which is not in this svg"),
        ] {
            has(&undrawn(&format!("<circle r='5' style=\"{style}\"/>")), why);
        }
        // A reference decoded from an entity is the reference.
        assert_eq!(attrs(node(&drawn("<defs><linearGradient id='lg'/></defs><circle id='c' r='5' style='fill:&#117;rl(&quot;#lg&quot;)'/>"), "c")), json!({ "r": 5, "fillRef": "lg" }));
        // Thousands of declarations in one attribute cost little.
        let many = format!("<circle id='c' cx='9' cy='9' r='5' style='{}'/>", "a:b;".repeat(7000));
        let started = Instant::now();
        assert_eq!(accepted(None, json!({ "svg": svg(&many) })).warnings, ["Not drawn: other style properties (7000)"]);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn text_and_markup_in_odd_places_refuse() {
        for (inner, why) in [
            ("<defs>words</defs><circle r='5'/>", "svg: text \"words\" outside a text element"),
            ("<defs><linearGradient id='a'>words</linearGradient></defs><circle r='5'/>", "outside a text element"),
            ("<defs><linearGradient id='a'><stop offset='0'>words</stop></linearGradient></defs><circle r='5'/>", "outside a text element"),
            ("<defs><clipPath id='a'>words<circle r='5'/></clipPath></defs><circle r='5'/>", "outside a text element"),
            ("<g><circle r='5'/>words</g>", "outside a text element"),
            ("<g>&#65;<circle r='5'/></g>", "svg: text \"#65\" outside a text element"),
            ("<g>\u{00A0}<circle r='5'/></g>", "outside a text element"),
            ("<g>\u{FEFF}<circle r='5'/></g>", "outside a text element"),
            ("<path d='M0 0L1 1'>words</path>", "outside a text element"),
            // CDATA and processing instructions refuse wherever they stand, inside what is dropped as well.
            ("<metadata><![CDATA[x]]></metadata><circle r='5'/>", "svg: a CDATA section is not accepted"),
            ("<title><![CDATA[<script>alert(1)</script>]]></title><circle r='5'/>", "svg: a CDATA section is not accepted"),
            ("<g><![CDATA[ ]]><circle r='5'/></g>", "svg: a CDATA section is not accepted"),
            ("<text>a<tspan><![CDATA[b]]></tspan></text>", "svg: a CDATA section is not accepted"),
            ("<metadata><?php x ?></metadata><circle r='5'/>", "svg: a processing instruction is not accepted"),
            ("<text>a<?x y?></text>", "svg: a processing instruction is not accepted"),
            ("<!DOCTYPE svg><circle r='5'/>", "svg: a DOCTYPE is not accepted"),
        ] {
            has(&undrawn(inner), why);
        }
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg><![CDATA[x]]>" })), "svg: a CDATA section is not accepted");
        has(&refused(None, json!({ "svg": "<svg><circle r='5'/></svg><?x y?>" })), "svg: a processing instruction is not accepted");
        has(&refused(None, json!({ "svg": "<![CDATA[<svg><circle r='5'/></svg>]]>" })), "svg: a CDATA section is not accepted");
        // Comments are no content, wherever they stand; a comment inside a text is not part of its words.
        let look = drawn("<!-- a --><text id='t' x='5' y='50'>one<!-- <tspan>hidden</tspan> -->two</text><g><!----><circle r='5'/></g>");
        assert_eq!(node(&look, "t").text.as_deref(), Some("onetwo"));
        // The words of a text are words: markup written with references stays text.
        let look = drawn("<text id='t' x='5' y='50'>&#60;script&#62;x&#x3C;/script&#x3E;</text>");
        assert_eq!(node(&look, "t").text.as_deref(), Some("<script>x</script>"));
        assert_eq!(to_svg(&look), "<svg viewBox='0 0 200 200'><text id='t' x='5' y='50'>&lt;script&gt;x&lt;/script&gt;</text></svg>");
    }

    #[test]
    fn the_document_has_one_root_and_ends_closed_whatever_surrounds_it() {
        for (doc, why) in [
            ("<svg></svg><svg/>", "svg: a second root element \"svg\""),
            ("<svg/><svg/>", "svg: a second root element \"svg\""),
            ("<svg/><g><circle r='5'/></g>", "svg: a second root element \"g\""),
            ("<svg><circle r='5'/></svg><!-- --><circle r='5'/>", "svg: a second root element \"circle\""),
            ("<g><svg><circle r='5'/></svg></g>", "svg: the root element is \"g\""),
            ("<circle r='5'/>", "svg: the root element is \"circle\""),
            ("<svg", "is not closed"),
            ("<svg>", "svg ends inside <svg>"),
            ("</svg>", "has no start tag"),
            ("<svg><circle r='5'/></svg></svg>", "has no start tag"),
            ("<svg><circle r='5'></svg>", "svg: the end tag \"svg\" closes \"circle\""),
            ("<svg><text>a</svg>", "closes \"text\""),
            ("<svg><text>a<tspan>b</text></svg>", "closes \"tspan\""),
            ("<svg><metadata><x></metadata></svg>", "closes \"x\""),
            ("<?xml version='1.0'?>", "svg: no svg element"),
            ("<!-- <svg><circle r='5'/></svg> -->", "svg: no svg element"),
            ("<svg><circle r='5'/></svg>&#65;", "outside a text element"),
            ("&lt;svg&gt;", "outside"),
            ("<svg><circle r='5'/></svg>x<svg><circle r='5'/></svg>", "outside the svg element"),
            ("<svg><g><svg viewBox='0 0 1 1'><circle r='5'/></svg></g></svg>", "a nested svg is not supported"),
            ("<svg><defs><svg/></defs><circle r='5'/></svg>", "a nested svg is not supported"),
            ("<svg><text>a<svg/></text></svg>", "\"svg\" inside a text"),
        ] {
            has(&refused(None, json!({ "svg": doc })), why);
        }
        // What stands after the root is not read as artwork, however deep it nests.
        let after = format!("<svg><circle id='c' cx='50' cy='50' r='5'/></svg>{}{}", "<x>".repeat(3000), "</x>".repeat(3000));
        let started = Instant::now();
        assert_eq!(refused(None, json!({ "svg": after })), "svg: a second root element \"x\" (at line 1 column 50): the artwork is one svg element");
        assert!(started.elapsed() < Duration::from_secs(2));
        // A declaration is the only thing that may stand before the root, besides comments and white space.
        accepted(None, json!({ "svg": "<?xml version='1.0'?><?xml version='1.0'?><svg><circle id='c' cx='50' cy='50' r='5'/></svg>" }));
    }

    #[test]
    fn ids_are_compared_after_decoding_and_as_written() {
        has(&undrawn("<circle id='a&#98;' r='5'/><circle id='ab' r='5'/>"), "svg: the id ab is used twice");
        has(&undrawn("<circle id='&#97;' r='5'/><circle id='&#x61;' r='5'/>"), "svg: the id a is used twice");
        has(&undrawn("<circle id='constru&#99;tor' r='5'/>"), "this name is reserved");
        for id in [" a", "a ", "a&#9;", "a&#10;b", "&#x200B;a", "a&#xAD;", "a&amp;b", "a&#x0301;", "\u{FF41}", "a\u{200D}b"] {
            has(&undrawn(&format!("<circle id='{id}' r='5'/>")), "a letter, then letters, digits, _ or -, at most 32 characters");
        }
        // Case tells ids apart, and a reserved name is reserved as it is written.
        let look = drawn("<circle id='Body' r='5'/><circle id='body' r='6'/><circle id='BODY' r='7'/><circle id='None' r='8'/><circle id='ALL' r='9'/><circle id='Constructor' r='9'/>");
        assert_eq!(attrs(node(&look, "body")), json!({ "r": 6 }));
        assert_eq!(attrs(node(&look, "Body")), json!({ "r": 5 }));
        // The id of the root is ignored: it names nothing, and nothing can refer to it.
        drawn("<circle id='icon' r='5'/>");
        accepted(None, json!({ "svg": "<svg id='icon'><circle id='icon' cx='9' cy='9' r='5'/></svg>" }));
        has(&refused(None, json!({ "svg": "<svg id='icon'><circle r='5' fill='url(#icon)'/></svg>" })), "svg: fill refers to #icon, which is not in this svg");
        // Nor can anything refer to an id inside what was dropped, or to an element that was refused.
        has(&undrawn("<metadata><linearGradient id='g'/></metadata><circle r='5' fill='url(#g)'/>"), "svg: fill refers to #g, which is not in this svg");
        has(&undrawn("<filter id='f'/><circle r='5' clip-path='url(#f)'/>"), "svg: clip-path refers to #f, which is not in this svg");
        has(&undrawn("<title id='t'>x</title><circle r='5' fill='url(#t)'/>"), "which is not in this svg");
        // An id that only a dropped element carries is free.
        drawn("<title id='a'>x</title><circle id='a' r='5'/>");
    }

    #[test]
    fn a_reference_names_one_element_of_its_kind_and_nothing_more() {
        let defs = "<defs><linearGradient id='lg'/><radialGradient id='rg'/><clipPath id='cp'><circle id='pane' r='5'/></clipPath></defs><g id='grp'><circle r='5'/></g>";
        for (attr, why) in [
            ("fill='url(#pane)'", "fill refers to #pane, a circle"),
            ("fill='url(#grp)'", "fill refers to #grp, a g"),
            ("stroke='url(#cp)'", "stroke refers to #cp, a clipPath"),
            ("clip-path='url(#pane)'", "clip-path refers to #pane, a circle"),
            ("clip-path='url(#rg)'", "clip-path refers to #rg, a radialGradient"),
            ("clip-path='url(#grp)'", "clip-path refers to #grp, a g"),
            ("fill='url(#lg) none'", "is more than a reference"),
            ("fill='url(#lg) url(#rg)'", "is more than a reference"),
            ("fill='url(#lg)url(http://x)'", "is more than a reference"),
            ("fill='url(#lg'", "reaches outside the artwork"),
            ("fill='url(lg)'", "reaches outside the artwork"),
            ("fill='url(#)'", "reaches outside the artwork"),
            ("fill='url()'", "reaches outside the artwork"),
            ("fill='url(#lg#rg)'", "reaches outside the artwork"),
            ("fill='url(a.svg#lg)'", "reaches outside the artwork"),
            ("fill='url(#lg%20)'", "reaches outside the artwork"),
            ("fill='url(&#x23;lg&#x29; url&#x28;http://x)'", "is more than a reference"),
            ("fill='url (#lg)'", "is not a colour"),
            ("fill='#lg'", "is not a colour"),
            ("fill='lg'", "is not a colour"),
            ("clip-path='url(#cp) view-box'", "svg: clip-path \"url(#cp) view-box\""),
            ("clip-path='url(http://x#cp)'", "svg: clip-path \"url(http://x#cp)\""),
            ("clip-path='inset(10%)'", "svg: clip-path \"inset(10%)\""),
        ] {
            has(&undrawn(&format!("{defs}<circle r='5' {attr}/>")), why);
        }
        // A stop takes a colour and never a reference.
        has(&undrawn("<defs><linearGradient id='a'><stop offset='0' stop-color='url(#a)'/></linearGradient></defs><circle r='5'/>"), "stop-color \"url(#a)\" reaches outside the artwork: a plain colour");
        // The written forms that are one reference.
        let look = drawn(&format!("{defs}<circle id='c' r='5' fill='URL(#lg)' stroke=' url( \"#rg\" ) ' clip-path=\"Url('#cp')\"/>"));
        assert_eq!(attrs(node(&look, "c")), json!({ "r": 5, "fillRef": "lg", "strokeRef": "rg", "clipPathRef": "cp" }));
    }

    #[test]
    fn a_clip_path_holds_plain_shapes_that_nothing_moves() {
        let clip = "<defs><clipPath id='cp'><circle id='pane' cx='50' cy='50' r='40'/><rect id='bar' x='0' y='0' width='10' height='10' transform='rotate(10)'/></clipPath></defs><g id='g' clip-path='url(#cp)'><circle id='c' cx='50' cy='50' r='5'/></g>";
        drawn(clip);
        let with = |more: Value| {
            let mut request = json!({ "svg": svg(clip) });
            for (key, value) in more.as_object().unwrap() {
                request[key.as_str()] = value.clone();
            }
            request
        };
        // Neither the clip path nor a shape inside it is a part: nothing names it.
        for id in ["cp", "pane", "bar"] {
            // The clip path is no part at all; a shape inside it stands where nothing moves.
            let why = if id == "cp" { "or inside defs" } else { "stands inside defs, a mask or a clipPath, where nothing moves" };
            has(&refused(None, with(json!({ "pivots": { id: [1, 2] } }))), why);
            has(&refused(None, with(json!({ "roles": { "head": id } }))), why);
            has(&refused(None, with(json!({ "animations": { "a": { "duration": 1, "tracks": [{ "target": id, "prop": "opacity", "values": [0, 1] }] } } }))), why);
            has(&refused(None, with(json!({ "animations": { "a": { "duration": 1, "tracks": [{ "target": ["c", id], "prop": "translateX", "values": [0, 1] }] } } }))), why);
            has(&refused(None, with(json!({ "mouth": { "target": id, "prop": "scaleY", "open": 2 } }))), why);
        }
        for (inner, why) in [
            ("<defs><clipPath id='a'><clipPath id='b'><circle r='5'/></clipPath></clipPath></defs><circle r='5'/>", "svg: <clipPath> inside <clipPath>"),
            ("<defs><clipPath id='a'><defs/></clipPath></defs><circle r='5'/>", "svg: <defs> inside <clipPath>: a clipPath holds shapes only"),
            ("<defs><clipPath id='a'><linearGradient id='b'/></clipPath></defs><circle r='5'/>", "svg: <linearGradient> inside <clipPath>"),
            ("<defs><clipPath id='a'><use href='#c'/></clipPath></defs><circle id='c' r='5'/>", "svg: <use> inside <clipPath>: a clipPath holds shapes only"),
            ("<defs><clipPath id='a'><circle r='5' style='clip-path:url(#a)'/></clipPath></defs><circle r='5'/>", "svg: clip-path inside a clipPath"),
            ("<defs><clipPath id='a'><circle r='5' clipPath='none'/></clipPath></defs><circle r='5'/>", "svg: clip-path inside a clipPath"),
            ("<defs><clipPath id='a' clipPathUnits='pixels'><circle r='5'/></clipPath></defs><circle r='5'/>", "one of userSpaceOnUse, objectBoundingBox"),
            ("<clipPath id='a'><circle r='5'/></clipPath><circle r='5'/>", "gradients and clip paths belong inside defs"),
        ] {
            has(&undrawn(inner), why);
        }
    }

    // ------------------------------------------------------------ the adversary: values

    #[test]
    fn one_number_is_one_plain_finite_number() {
        for bad in ["NaN", "nan", "inf", "-inf", "Infinity", "-Infinity", "1e999", "-1e999", "1E400", "0x10", "1_000", "1,5", "1 5", "\u{FF15}", "\u{0663}", "1.2.3", ".", "-", "+", "e5", "1e", "1e+", "--1", "+-1", "1 px", "1pt", "1em", "1rem", "1%", "1deg", "1e5", "\u{2212}1", "calc(1+1)", "1/2", "1e2e2", "0b1", "1f", "1L", "true", "null", "''"] {
            let why = undrawn(&format!("<circle cx=\"{bad}\" cy='5' r='5'/>"));
            assert!(why.contains("svg: cx "), "{bad}: {why}");
        }
        let nines = "9".repeat(400);
        has(&undrawn(&format!("<circle cx='{nines}' r='5'/>")), "svg: cx ");
        has(&undrawn(&format!("<circle cx='1e{nines}' r='5'/>")), "svg: cx ");
        has(&undrawn(&format!("<circle cx='0.{nines}e{nines}' r='5'/>")), "svg: cx ");
        // A number of a look is finite however large it was written, in a design and in a stored file.
        for huge in [1e300, 1e306, f64::MAX, -f64::MAX, f64::MIN_POSITIVE, 9.0e15, 123456789012345680.0] {
            assert!(Num::new(huge).get().is_finite() && round3(huge).is_finite(), "{huge}");
            let back: Num = serde_json::from_str(&serde_json::to_string(&Num::new(huge)).unwrap()).unwrap();
            assert_eq!(back, Num::new(huge), "{huge}");
        }
        assert!(serde_json::from_str::<Num>("1e306").unwrap().get().is_finite());
        assert!(serde_json::from_str::<Num>("1e999").is_err());
        assert_eq!(serde_json::to_string(&[Num::new(1_000_000.0), Num::new(-999_999.5), Num::new(-0.0004), Num::new(f64::INFINITY), Num::new(f64::NAN)]).unwrap(), "[1000000,-999999.5,0,0,0]");
        // The spellings of a plain number, and the unit that is dropped.
        let look = drawn(&format!("<rect id='r' x=' 7 ' y='007' width='+7.0' height='7e0' rx='70E-1' ry='.7e1px'/><circle id='c' cx='7PX' cy='-0' r='1e-999'/><circle id='z' cx='-0.0' cy='0.{}' r='0.00049'/>", "0".repeat(300)));
        assert_eq!(serde_json::to_string(&node(&look, "r").attrs).unwrap(), r#"{"height":7,"rx":7,"ry":7,"width":7,"x":7,"y":7}"#);
        assert_eq!(serde_json::to_string(&node(&look, "c").attrs).unwrap(), r#"{"cx":7,"cy":0,"r":0}"#);
        assert_eq!(serde_json::to_string(&node(&look, "z").attrs).unwrap(), r#"{"cx":0,"cy":0,"r":0}"#);
        // A percentage only where the contract has one: gradient coordinates, offset, the opacities.
        for (inner, why) in [
            ("<circle cx='50%' r='5'/>", "percentages are not accepted"),
            ("<rect width='100%' height='5'/>", "percentages are not accepted"),
            ("<line x1='0' y1='0' x2='100%' y2='5'/>", "percentages are not accepted"),
            ("<text x='50%' y='5'>a</text>", "percentages are not accepted"),
            ("<text font-size='120%'>a</text>", "percentages are not accepted"),
            ("<text letter-spacing='5%'>a</text>", "percentages are not accepted"),
            ("<circle r='5' stroke-width='5%'/>", "percentages are not accepted"),
            ("<circle r='5' stroke-miterlimit='400%'/>", "percentages are not accepted"),
            ("<circle r='5' transform='translate(50%)'/>", "does not hold plain numbers"),
            ("<polygon points='0% 0 1 1'/>", "points hold more than numbers"),
            ("<path d='M0% 0L1 1'/>", "in path data"),
            ("<text font-weight='400%'>a</text>", "normal, bold or one of 100, 200 and so on to 900"),
        ] {
            has(&undrawn(inner), why);
        }
        has(&refused(None, json!({ "svg": "<svg viewBox='0 0 100% 100%'><circle r='5'/></svg>" })), "viewBox");
        let look = drawn("<defs><linearGradient id='lg' x1='10%' y2='200%'><stop offset='150%' stop-opacity='-5%'/><stop offset='-1'/></linearGradient></defs><circle id='c' r='5' opacity='250%' fill-opacity='5e-1' stroke-opacity='1e2%'/>");
        assert_eq!(attrs(node(&look, "lg")), json!({ "x1": 0.1, "y2": 2 }));
        assert_eq!(attrs(&node(&look, "lg").children[0]), json!({ "offset": 1, "stopOpacity": 0 }));
        assert_eq!(attrs(&node(&look, "lg").children[1]), json!({ "offset": 0 }));
        assert_eq!(attrs(node(&look, "c")), json!({ "r": 5, "opacity": 1, "fillOpacity": 0.5, "strokeOpacity": 1 }));
        for bad in ["half", "1e999", "50 %", "%", "1%%", "0.5px%", ""] {
            has(&undrawn(&format!("<circle r='5' opacity='{bad}'/>")), "svg: opacity ");
            has(&undrawn(&format!("<defs><linearGradient id='a'><stop offset='{bad}'/></linearGradient></defs><circle r='5'/>")), "svg: offset ");
        }
        // A gradient coordinate keeps to its range in either unit.
        for bad in ["11", "-11", "1100%", "1e999", "x"] {
            has(&undrawn(&format!("<defs><linearGradient id='a' x1='{bad}'/></defs><circle r='5'/>")), "svg: x1 ");
        }
        for bad in ["1001", "-801", "600%", "1e999"] {
            has(&undrawn(&format!("<defs><linearGradient id='a' gradientUnits='userSpaceOnUse' x1='{bad}'/></defs><circle r='5'/>")), "svg: x1 ");
        }
        has(&undrawn("<defs><radialGradient id='a' r='-0.5'/></defs><circle r='5'/>"), "svg: r ");
        has(&undrawn("<defs><radialGradient id='a' gradientUnits='userSpaceOnUse' r='801'/></defs><circle r='5'/>"), "svg: r ");
    }

    #[test]
    fn every_number_is_checked_against_its_range_before_it_is_rounded_into_it() {
        // One step past each bound refuses, even where rounding would bring it back.
        for (inner, why) in [
            ("<circle cx='1000.0004' r='5'/>", "cx 1000 is out of range: -800 to 1000"),
            ("<circle cx='-800.0004' r='5'/>", "cx -800 is out of range"),
            ("<circle r='800.0004'/>", "r 800 is out of range: 0 to 800"),
            ("<circle r='5' stroke-width='800.0004'/>", "stroke-width 800 is out of range"),
            ("<text font-size='800.0004'>a</text>", "font-size 800 is out of range"),
            ("<text font-size='0.0004'>a</text>", "font-size 0 is out of range: above 0, up to 800"),
            ("<text letter-spacing='800.0004'>a</text>", "letter-spacing 800 is out of range: within 800"),
            ("<circle r='5' stroke-miterlimit='100.0004'/>", "stroke-miterlimit 100 is out of range: 1 to 100"),
            ("<circle r='5' stroke-miterlimit='0.9996'/>", "stroke-miterlimit 1 is out of range"),
        ] {
            has(&undrawn(inner), why);
        }
        for bad in ["0 0 15.9996 200", "0 0 200 4096.0004", "4096.0004 0 200 200", "0 -4096.0004 200 200", "0 0 -200 200", "0 0 0 0", "0 0 200 1e999", "0 0 200", "", " ", "0 0 200 200 0", "0,0;200,200", "0 0 200 200px", "0 0 2e2% 200"] {
            has(&refused(None, json!({ "svg": format!("<svg viewBox='{bad}'><circle r='5'/></svg>") })), "viewBox");
        }
        // The bounds themselves are inside, and what is stored never lies outside them.
        let look = accepted(None, json!({ "svg": "<svg viewBox='-4096,-4096 16 4096'><circle id='c' cx='12287.9996' cy='-20480' r='16384'/><text id='t' x='0' y='0' font-size='0.0006' letter-spacing='-16384'>a</text></svg>" })).design.look.unwrap();
        assert_eq!(look.view_box.map(Num::get), [-4096.0, -4096.0, 16.0, 4096.0]);
        assert_eq!(attrs(node(&look, "c")), json!({ "cx": 12288, "cy": -20480, "r": 16384 }));
        assert_eq!(attrs(node(&look, "t")), json!({ "x": 0, "y": 0, "fontSize": 0.001, "letterSpacing": -16384 }));
        // The reach follows the viewBox: four times its larger side, around it.
        let small = |inner: &str| refused(None, json!({ "svg": format!("<svg viewBox='10 20 16 32'>{inner}</svg>") }));
        has(&small("<circle cx='180.5' r='5'/>"), "cx 180.5 is out of range: -118 to 180");
        has(&small("<circle cx='-118.5' r='5'/>"), "cx -118.5 is out of range: -118 to 180");
        has(&small("<circle r='128.5'/>"), "r 128.5 is out of range: 0 to 128");
        accepted(None, json!({ "svg": "<svg viewBox='10 20 16 32'><circle cx='180' cy='-118' r='128'/></svg>" }));
    }

    #[test]
    fn path_data_keeps_to_its_grammar() {
        let view = DEFAULT_VIEW;
        let read = |d: &str| path_data(d, &view, MAX_D_CHARS, &unmasked).map(|p| p.text);
        for (written, stored) in [
            ("M0,0L.5.5-1-2", "M 0 0 L 0.5 0.5 -1 -2"),
            ("M-0-0L-0.0001 0", "M 0 0 L 0 0"),
            ("M0 0a6 6 0 01-6.5.5", "M 0 0 a 6 6 0 0 1 -6.5 0.5"),
            ("M0 0A6,6,0,1,1,6,6", "M 0 0 A 6 6 0 1 1 6 6"),
            ("M0 0a6 6 0 1010 10", "M 0 0 a 6 6 0 1 0 10 10"),
            ("M0 0A5 5 0 1 10 10", "M 0 0 A 5 5 0 1 1 0 10"),
            ("M0 0 1 1 2 2", "M 0 0 1 1 2 2"),
            ("m1 1 1 1zZzm5 5", "m 1 1 1 1 z Z z m 5 5"),
            ("M1e2 1E+2 1e-999 0", "M 100 100 0 0"),
            ("M+1+2L+3+4", "M 1 2 L 3 4"),
            ("M1.5.5.5.5", "M 1.5 0.5 0.5 0.5"),
            ("M0 0\u{000C}L1\r\n1", "M 0 0 L 1 1"),
            ("M 007 0.50 L 1.0004 1.0006", "M 7 0.5 L 1 1.001"),
        ] {
            assert_eq!(read(written).as_deref(), Ok(stored), "{written}");
            // What is stored reads as itself.
            assert_eq!(read(stored).as_deref(), Ok(stored), "{stored}");
        }
        for (d, why) in [
            ("M0 0 B 1 1", "\"B\" is not a path command"),
            ("M0 0 e", "\"e\" in path data"),
            ("M0 0 L 1 1e", "in path data"),
            ("M0 0 L 0x10 1", "\"x\" is not a path command"),
            ("M0 0 l 1 1 NaN", "\"N\" is not a path command"),
            ("M0 0 L 1 1 <script>", "in path data"),
            ("M0 0 L 1 1 \u{00A0}", "in path data"),
            ("M0 0 L 1 \u{FF11}", "in path data"),
            ("M0 0 L 1 \u{2212}1", "in path data"),
            ("M0 0 L 1 1 //", "in path data"),
            ("M0 0 L 1 1;", "in path data"),
            ("M0 0 L 1 1 url(#a)", "is not a path command"),
            ("M0 0 A 5 5 0 1 1 10", "the command A has too few numbers"),
            ("M0 0 A 5 5 0 -1 1 5 5", "an arc flag in path data that is not 0 or 1"),
            ("M0 0 A 5 5 0 1.0 1 5 5", "an arc flag in path data that is not 0 or 1"),
            ("M0 0 A 5 5 0 + 1 5 5", "an arc flag in path data that is not 0 or 1"),
            ("M0 0 A 5 5 0 1 2 5 5", "an arc flag in path data that is not 0 or 1"),
            ("M0 0 A 5 5 0 1 1 5 5 5", "the command A has too few numbers"),
            ("M0 0 L 1 1 5", "the command L has too few numbers"),
            ("M0 0 C 1 1 2 2 3", "the command C has too few numbers"),
            ("M0 0 Q 1 1 2", "the command Q has too few numbers"),
            ("M0 0 H", "the command H has too few numbers"),
            ("M 0 0 Z 0", "a number follows Z"),
            ("Z", "path data starts with Z"),
            ("z M0 0", "path data starts with z"),
            ("0 0 M 1 1", "path data starts with"),
            (",", "empty path data"),
            ("  ", "empty path data"),
            ("M0 0 L 1e999 1", "beyond any size"),
            ("M0 0 L -1e999 1", "beyond any size"),
            ("M0 0 L 1001 0", "the number 1001 in path data is out of range: -800 to 1000"),
            ("M0 0 H 1001", "out of range"),
            ("M0 0 V -1001", "out of range"),
            ("M0 0 a 1001 5 0 0 1 5 5", "out of range"),
        ] {
            has(&read(d).unwrap_err(), why);
        }
        // The limit is of what is written and of what is stored, to the character.
        let seg = " L 100 100";
        let exact = format!("M 0 0{} h 10", seg.repeat(399));
        assert_eq!(exact.len(), MAX_D_CHARS);
        assert_eq!(read(&exact).as_deref(), Ok(exact.as_str()));
        has(&read(&format!("{exact}0")).unwrap_err(), "4001 characters of path data: at most 4000");
        let padded = format!("M 0 0{}     ", seg.repeat(399));
        assert_eq!((padded.len(), read(&padded).unwrap().len()), (MAX_D_CHARS, 3995));
        has(&read(&format!("{padded} ")).unwrap_err(), "4001 characters of path data: at most 4000");
        let dense = |tail: &str| format!("M 0 0{}{tail}", seg.repeat(398));
        assert_eq!(read(&dense("L1-1L10-1")).unwrap().len(), MAX_D_CHARS);
        has(&read(&dense("L1-1L100-1")).unwrap_err(), "4001 characters of path data once it is spaced out: at most 4000");
        // Through a design call the same limits hold.
        assert_eq!(attrs(node(&drawn(&format!("<path id='p' d='{exact}'/>")), "p"))["d"], json!(exact));
        has(&undrawn(&format!("<path d='{exact}0'/>")), "4001 characters of path data: at most 4000");
    }

    #[test]
    fn every_point_of_a_path_and_of_a_point_list_lies_within_the_reach_of_the_view_box() {
        let view = DEFAULT_VIEW;
        let read = |d: &str| path_data(d, &view, MAX_D_CHARS, &unmasked).map(|p| p.text);
        // Relative steps add up: each is small, the pen leaves all the same.
        for d in [
            "M0 0 l900 0 l900 0",
            "M0 0 h600 h600",
            "M0 0 v-500 v-500",
            "m900 0 m900 0 l1 1",
            "M0 0 l500 500 500 500 500 500",
            "M900 900 q50 50 200 200",
            "M900 900 c0 0 200 0 50 0",
            "M900 900 s200 0 50 0",
            "M900 900 t200 0",
            "M900 900 a5 5 0 0 1 200 0",
            "M0 0 L10 10 z m1000 0 m1 0",
        ] {
            has(&read(d).unwrap_err(), "every point within -800 to 1000");
        }
        // An absolute point keeps to the range of every coordinate, on its low side too.
        has(&read("M -900 0 L 0 0").unwrap_err(), "every point within -800 to 1000");
        has(&read("M 0 0 L 0 -801").unwrap_err(), "every point within -800 to 1000");
        has(&read("M 0 0 C 0 0 -801 0 5 5").unwrap_err(), "every point within -800 to 1000");
        has(&read("M 0 0 H -801").unwrap_err(), "every point within -800 to 1000");
        // Steps that add up to the edge, give or take what adding leaves behind the last decimal.
        assert!(read("M999 0 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1").is_ok());
        has(&read("M999 0 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1 h.1").unwrap_err(), "a point of the path lies at 1000.1 0");
        // The ends of the range, reached in steps.
        assert_eq!(read("M-800-800 l900 900 900 900 h-900 h-900 v-900 v-900").as_deref(), Ok("M -800 -800 l 900 900 900 900 h -900 h -900 v -900 v -900"));
        // One number is never larger than the range reaches from zero, a step no more than a coordinate.
        has(&read("M1000 1000 l-1800 -1800").unwrap_err(), "the number -1800 in path data is out of range");
        has(&undrawn("<path d='M0 0 l900 0 l900 0'/>"), "svg: a point of the path lies at 1800 0: every point within -800 to 1000");
        has(&refused(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": ["M0 0 q900 0 900 0 q900 0 900 0", "M0 0 q900 0 900 0 q900 0 900 0"] }]))), "a d keyframe: a point of the path lies at 1800 0");
        has(&refused(None, figure(json!({ "mouth": { "target": "mouth", "open": "M0 0 q900 0 900 0 q900 0 900 0" } }))), "mouth open: a point of the path lies at 1800 0");

        for points in ["-801 0 5 5", "0 0 1000.5 5", "0 -900 5 5"] {
            has(&undrawn(&format!("<polygon points='{points}'/>")), "in points is out of range: -800 to 1000");
        }
        assert_eq!(attrs(node(&drawn("<polyline id='p' points='-800,-800 1000,1000'/>"), "p")), json!({ "points": "-800 -800 1000 1000" }));
    }

    #[test]
    fn a_point_list_is_numbers_in_pairs_within_its_length() {
        let read = |points: &str| point_list(points, &DEFAULT_VIEW).map(|(text, _)| text);
        assert_eq!(read("1,5 2,5").as_deref(), Ok("1 5 2 5"));
        assert_eq!(read(" 1-2.5.5+3 ").as_deref(), Ok("1 -2.5 0.5 3"));
        assert_eq!(read("-0,-0 1e1,1E-1").as_deref(), Ok("0 0 10 0.1"));
        for (points, why) in [
            ("1", "points hold 1 numbers: x and y in pairs"),
            ("1 2 3", "points hold 3 numbers"),
            (",", "points hold 0 numbers"),
            ("1 2 NaN 4", "points hold more than numbers"),
            ("1 2; 3 4", "points hold more than numbers"),
            ("1 2 3 4px", "points hold more than numbers"),
            ("1 2 (3 4)", "points hold more than numbers"),
            ("1 2 1e999 4", "beyond any size"),
            ("M 1 2 3 4", "points hold more than numbers"),
        ] {
            has(&read(points).unwrap_err(), why);
        }
        let exact = "1 1 ".repeat(MAX_POINTS_CHARS / 4);
        assert_eq!((exact.len(), read(&exact).unwrap().len()), (MAX_POINTS_CHARS, MAX_POINTS_CHARS - 1));
        has(&read(&format!("{exact} ")).unwrap_err(), "2001 characters of points: at most 2000");
        // Written within the limit, over it once every number stands apart.
        assert_eq!((".5.5".repeat(250).len(), read(&".5.5".repeat(250)).unwrap().len()), (1000, 1999));
        has(&read(&".5.5".repeat(251)).unwrap_err(), "2007 characters of points once they are spaced out: at most 2000");
    }

    #[test]
    fn a_transform_list_keeps_to_its_functions_and_their_ranges() {
        let read = |t: &str| transform(t, &DEFAULT_VIEW, &unmasked).map(|(text, _)| text);
        for (written, stored) in [
            ("scale(1)scale(2)", "scale(1) scale(2)"),
            (" translate( 1 , 2 ) , rotate(-0) ", "translate(1 2) rotate(0)"),
            ("matrix(1,0,0,1,0,0)", "matrix(1 0 0 1 0 0)"),
            ("scale(-1)", "scale(-1)"),
            ("scale(.001)", "scale(0.001)"),
            ("scale(0.0006 1000.0004)", "scale(0.001 1000)"),
            ("skewX(80) skewY(-80)", "skewX(80) skewY(-80)"),
            ("rotate(3600 -800 1000)", "rotate(3600 -800 1000)"),
            ("rotate(1e-999)", "rotate(0)"),
            ("translate(800 -800)", "translate(800 -800)"),
            ("matrix(0.001 0 0 0.001 800 -800)", "matrix(0.001 0 0 0.001 800 -800)"),
            ("", ""),
            (" , ", ""),
        ] {
            assert_eq!(read(written).as_deref(), Ok(stored), "{written}");
            assert_eq!(read(stored).as_deref(), Ok(stored), "{stored}");
        }
        for (written, why) in [
            ("translate((1))", "does not hold plain numbers"),
            ("scale(1))", "a transform that is not written as functions"),
            ("scale(1)(2)", "a transform that is not written as functions"),
            ("scale(1) garbage", "a transform that is not written as functions"),
            ("(1)", "a transform that is not written as functions"),
            ("translate3d(1,2,3)", "a transform that is not written as functions"),
            ("scale(1", "the transform scale( is not closed"),
            ("scale(", "the transform scale( is not closed"),
            ("scale()", "the transform scale() with 0 numbers"),
            ("scale(1 2 3)", "the transform scale() with 3 numbers"),
            ("translate(1 2 3)", "the transform translate() with 3 numbers"),
            ("rotate()", "the transform rotate() with 0 numbers"),
            ("skewX(1 2)", "the transform skewX() with 2 numbers"),
            ("matrix(1 0 0 1 0 0 0)", "does not hold plain numbers, six at most"),
            ("Translate(1)", "\"Translate\" is not a transform function"),
            ("SCALE(2)", "\"SCALE\" is not a transform function"),
            ("scaleX(2)", "\"scaleX\" is not a transform function"),
            ("rotateX(2)", "\"rotateX\" is not a transform function"),
            ("skew(2)", "\"skew\" is not a transform function"),
            ("scale(1e999)", "is beyond any size"),
            ("translate(1e999)", "is beyond any size"),
            ("rotate(-1e999)", "is beyond any size"),
            ("matrix(1e999 0 0 1 0 0)", "is beyond any size"),
            ("scale(0.0004)", "the transform scale() is out of range"),
            ("scale(1 0)", "the transform scale() is out of range"),
            ("scale(-0)", "the transform scale() is out of range"),
            ("scale(1000.0006)", "the transform scale() is out of range"),
            ("matrix(0 0 0 0 0 0)", "the transform matrix() would flatten the picture or move it away"),
            ("matrix(1 2 2 4 0 0)", "the transform matrix() would flatten the picture or move it away"),
            ("matrix(1001 0 0 1 0 0)", "the transform matrix() would flatten the picture or move it away"),
            ("matrix(1 0 0 1 801 0)", "the transform matrix() would flatten the picture or move it away"),
            ("rotate(10 1001 0)", "the transform rotate() is out of range"),
            ("rotate(3600.0006)", "the transform rotate() is out of range"),
            ("skewX(80.0006)", "the transform skewX() is out of range"),
            ("skewY(-81)", "the transform skewY() is out of range"),
            ("translate(801)", "the transform translate() moves too far"),
            ("translate(0 -801)", "the transform translate() moves too far"),
            ("scale(\u{2212}1)", "does not hold plain numbers"),
            ("scale(1;2)", "does not hold plain numbers"),
            ("scale(1px)", "does not hold plain numbers"),
            ("scale(50%)", "does not hold plain numbers"),
            ("scale(1) url(#a)", "the transform \"url(#a)\" does not hold plain numbers"),
        ] {
            has(&read(written).unwrap_err(), why);
        }
        // The length of what is written, to the character.
        assert_eq!(read(&format!("scale(1){}", " ".repeat(MAX_TRANSFORM_CHARS - 8))).as_deref(), Ok("scale(1)"));
        has(&read(&format!("scale(1){}", " ".repeat(MAX_TRANSFORM_CHARS - 7))).unwrap_err(), "a transform of 1025 characters: at most 1024");
        // A name of any length is only ever part of a message, cut like every problem.
        has(&undrawn(&format!("<circle r='5' transform='{}(1)'/>", "a".repeat(900))), "is not a transform function");
        // An empty transform is no attribute.
        assert_eq!(attrs(node(&drawn("<circle id='c' r='5' transform=' '/>"), "c")), json!({ "r": 5 }));
        assert_eq!(attrs(node(&drawn("<defs><linearGradient id='g' gradientTransform=''/></defs><circle r='5'/>"), "g")), json!({}));
    }

    // ------------------------------------------------------------ the adversary: boxes

    #[test]
    fn boxes_follow_nested_rotations_scales_and_matrices() {
        let look = drawn(
            "<g id='a' transform='rotate(90)'><g id='b' transform='scale(2 3)'><rect id='r' x='1' y='2' width='10' height='20'/></g></g>\
             <g id='m' transform='matrix(0 1 -1 0 10 20)'><circle id='c' cx='5' cy='0' r='2'/></g>\
             <g id='o' transform='translate(100 100)'><g id='f' transform='scale(-1 1)'><rect id='q' x='10' y='0' width='30' height='10'/></g></g>\
             <g id='sk'><rect id='s' transform='skewX(45)' x='0' y='0' width='10' height='10'/></g>\
             <g id='top'><g transform='translate(10 0)'><g transform='scale(2)'><circle cx='5' cy='5' r='5'/></g></g></g>\
             <g id='two' transform='translate(10 10) rotate(180) scale(0.5)'><line id='ln' x1='0' y1='0' x2='40' y2='20'/></g>\
             <g id='mix'><text id='tx' x='10' y='20' font-size='10' transform='translate(100 0)'>ab</text><polygon points='0,0 4,0 4,4' transform='rotate(-90)'/></g>",
        );
        // In its own coordinates: the transforms of its children are in a group's box, its own is not.
        assert_eq!(bbox(&look, "r"), [1.0, 2.0, 10.0, 20.0]);
        assert_eq!(bbox(&look, "b"), [1.0, 2.0, 10.0, 20.0]);
        assert_eq!(bbox(&look, "a"), [2.0, 6.0, 20.0, 60.0]);
        assert_eq!(bbox(&look, "m"), [3.0, -2.0, 4.0, 4.0]);
        assert_eq!(bbox(&look, "f"), [10.0, 0.0, 30.0, 10.0]);
        assert_eq!(bbox(&look, "o"), [-40.0, 0.0, 30.0, 10.0]);
        assert_eq!(bbox(&look, "sk"), [0.0, 0.0, 20.0, 10.0]);
        assert_eq!(bbox(&look, "top"), [10.0, 0.0, 20.0, 20.0]);
        assert_eq!(bbox(&look, "two"), [0.0, 0.0, 40.0, 20.0]);
        assert_eq!(bbox(&look, "mix"), [0.0, -4.0, 122.0, 26.0]);
        // At the root: through every transform above and its own.
        let all = parts(&look);
        let part = |id: &str| all.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no part {id}")).bbox;
        assert_eq!(part("r"), [-66.0, 2.0, 60.0, 20.0]);
        assert_eq!(part("b"), [-66.0, 2.0, 60.0, 20.0]);
        assert_eq!(part("a"), [-66.0, 2.0, 60.0, 20.0]);
        assert_eq!(part("c"), [8.0, 23.0, 4.0, 4.0]);
        assert_eq!(part("m"), [8.0, 23.0, 4.0, 4.0]);
        assert_eq!(part("q"), [60.0, 100.0, 30.0, 10.0]);
        assert_eq!(part("f"), [60.0, 100.0, 30.0, 10.0]);
        assert_eq!(part("o"), [60.0, 100.0, 30.0, 10.0]);
        assert_eq!(part("s"), [0.0, 0.0, 20.0, 10.0]);
        assert_eq!(part("ln"), [-10.0, 0.0, 20.0, 10.0]);
        assert_eq!(part("two"), [-10.0, 0.0, 20.0, 10.0]);
        assert_eq!(part("tx"), [110.0, 12.0, 12.0, 10.0]);
        // The pivot of a part without one is the centre of its box, carried to the root.
        let pivot = |id: &str| all.iter().find(|p| p.id == id).unwrap().pivot;
        assert_eq!(pivot("r"), [-36.0, 12.0]);
        assert_eq!(pivot("q"), [75.0, 105.0]);
        assert_eq!(pivot("ln"), [0.0, 5.0]);
    }

    #[test]
    fn boxes_of_paths_follow_relative_commands_and_arcs() {
        let look = drawn(
            "<path id='hv' d='M10 10 h20 v30 h-40 z'/><path id='mm' d='m10 10 l10 0 l0 10 m20 20 l5 5'/><path id='abs' d='M10 10 H50 V-5 H-20'/>\
             <path id='qt' d='M10 80 q40 -60 80 0 t80 0'/><path id='cs' d='M10 10 c0 20 30 20 30 0 s30 -20 30 0'/>\
             <path id='up' d='M50 50 a20 20 0 0 1 40 0'/><path id='down' d='M50 50 a20 20 0 0 0 40 0'/>\
             <path id='zz' d='M10 10 l10 0 z l0 10 z'/><path id='im' d='M10 10 20 30 5 40'/><path id='one' d='M7 9'/>\
             <path id='st' d='M0 100 L10 100 T30 100'/><path id='ss' d='M0 100 L10 100 S20 90 30 100'/>",
        );
        assert_eq!(bbox(&look, "hv"), [-10.0, 10.0, 40.0, 30.0]);
        assert_eq!(bbox(&look, "mm"), [10.0, 10.0, 35.0, 35.0]);
        assert_eq!(bbox(&look, "abs"), [-20.0, -5.0, 70.0, 15.0]);
        assert_eq!(bbox(&look, "qt"), [10.0, 20.0, 160.0, 120.0]);
        assert_eq!(bbox(&look, "cs"), [10.0, -10.0, 60.0, 40.0]);
        // sweep 1 runs clockwise on the screen: from the left end over the top to the right end.
        assert_eq!(bbox(&look, "up"), [50.0, 30.0, 40.0, 20.0]);
        assert_eq!(bbox(&look, "down"), [50.0, 50.0, 40.0, 20.0]);
        // After z the pen is back at the start of the subpath.
        assert_eq!(bbox(&look, "zz"), [10.0, 10.0, 10.0, 10.0]);
        assert_eq!(bbox(&look, "im"), [5.0, 10.0, 15.0, 30.0]);
        assert_eq!(bbox(&look, "one"), [7.0, 9.0, 0.0, 0.0]);
        // A smooth command after a line has nothing to mirror: its control point is the pen.
        assert_eq!(bbox(&look, "st"), [0.0, 100.0, 30.0, 0.0]);
        assert_eq!(bbox(&look, "ss"), [0.0, 90.0, 30.0, 10.0]);
    }

    #[test]
    fn a_part_keeps_within_a_million_units_through_every_transform() {
        // Two scales of a thousand: each is allowed, together they send the part out of all bounds.
        let nested = |inner: &str| format!("<g id='a' transform='scale(1000)'><g id='b' transform='scale(1000)'>{inner}</g></g>");
        assert_eq!(undrawn(&nested("<circle id='c' cx='100' cy='100' r='50'/>")), "svg: a reaches 150000000 units from the origin (3 times): every part within 1000000 units, drawn to the scale of the viewBox");
        // The box a group has of its own, from the transforms of what it holds, is named where the group stands.
        assert_eq!(undrawn("<g id='a'><g id='b' transform='scale(1000)'><g transform='scale(1000)'><circle cx='100' cy='100' r='50'/></g></g></g>"), "svg: a reaches 150000000 units from the origin (3 times, first at line 1 column 28, in g#a): every part within 1000000 units, drawn to the scale of the viewBox");
        // One of them keeps a part that is near the origin within bounds.
        drawn("<g id='a' transform='scale(1000)'><circle id='c' cx='100' cy='100' r='50'/></g>");
        // A tower of them is one kind of problem, however many parts are out there.
        let mut tower = String::from("<circle cx='100' cy='100' r='50'/>");
        for i in 0..9 {
            tower = format!("<g id='g{i}' transform='scale(1000)'>{tower}</g>");
        }
        let why = undrawn(&tower);
        has(&why, "units from the origin");
        assert_eq!(why.lines().count(), 1, "{why}");
        // A matrix does it as well as a scale, and a text is as wide as its letters make it.
        has(&undrawn("<g id='a' transform='matrix(1000 0 0 1000 0 0) matrix(1000 0 0 1000 0 0)'><circle id='c' cx='100' cy='100' r='50'/></g>"), "units from the origin");
        has(&refused(None, json!({ "svg": "<svg viewBox='0 0 4096 4096'><text id='t' x='0' y='0' font-size='16384' letter-spacing='16384'>0123456789012345678901234567890123456789</text></svg>" })), "units from the origin");
        // What is far away but has no id is not stored and not reported: there is nothing to bound.
        drawn(&nested("<circle cx='100' cy='100' r='50'/>").replace(" id='a'", "").replace(" id='b'", ""));
        // At the bound itself, and a step past it.
        let look = drawn("<g id='a' transform='scale(1000)'><circle id='c' cx='900' cy='0' r='100'/></g>");
        assert_eq!(parts(&look).iter().map(|p| p.bbox).collect::<Vec<_>>(), [[800000.0, -100000.0, 200000.0, 200000.0]; 2]);
        has(&undrawn("<g id='a' transform='scale(1000)'><circle id='c' cx='900.5' cy='0' r='100'/></g>"), "a reaches 1000500 units from the origin");
        // A pivot is carried to the root like its part: a part at the origin may sit under any
        // transform, the point it turns about may not be sent out of all bounds by it.
        let still = "<g transform='scale(1000)'><g transform='scale(1000)'><g id='hub'><circle cx='0' cy='0' r='0.5'/></g></g></g>";
        assert_eq!(parts(&drawn(still))[0].bbox, [-500000.0, -500000.0, 1000000.0, 1000000.0]);
        has(&refused(None, json!({ "svg": svg(still), "pivots": { "hub": [1000, 0] } })), "the pivot of hub reaches 1000000000 units from the origin (at pivots.hub): every part within 1000000 units, drawn to the scale of the viewBox");
        let turned = accepted(None, json!({ "svg": svg(still), "pivots": { "hub": [1, -1] } }));
        assert_eq!(turned.parts[0].pivot, [1000000.0, -1000000.0]);
        has(&refused(Some(&turned.design), json!({ "pivots": { "hub": [1.5, 0] } })), "the pivot of hub reaches 1500000 units from the origin");
        // Nothing stored is ever larger: the browser refuses a look that holds a larger number.
        let wide = accepted(None, json!({ "svg": "<svg viewBox='-4096 -4096 4096 4096'><g id='a' transform='scale(20)'><polygon id='p' points='-20480,-20480 16384,16384 0,0'/></g></svg>" })).design.look.unwrap();
        assert!(numbers(&wide).iter().all(|n| n.abs() <= 1_000_000.0), "{:?}", numbers(&wide));
    }

    #[test]
    fn what_is_stored_is_read_back_under_any_view_box() {
        // A viewBox far from the origin: its coordinates reach where no other viewBox does.
        for (view, far) in [("4096 4096 4096 4096", 24576.0_f64), ("-4096 -4096 4096 4096", -20480.0)] {
            let art = format!(
                "<svg viewBox='{view}'><g id='turn' transform='rotate(90 {far} {far})'><circle id='c' cx='{far}' cy='{near}' r='10'/></g><path id='mouth' d='M {far} {far} L {near} {near}'/></svg>",
                near = far - far.signum() * 100.0
            );
            let request = json!({ "svg": art, "mouth": { "target": "mouth", "open": format!("M {far} {far} L {far} {far}") }, "animations": { "a": { "duration": 1, "tracks": [{ "target": "mouth", "prop": "d", "values": [null, format!("M {far} {far} L {far} {far}"), null] }] } } });
            let applied = accepted(None, request);
            let look = applied.design.look.as_ref().unwrap();
            // The turn about the far point is part of where the circle ends up.
            let c = applied.parts.iter().find(|p| p.id == "c").unwrap();
            assert_eq!(c.bbox, [far + far.signum() * 100.0 - 10.0, far - 10.0, 20.0, 20.0], "{view}");
            assert_eq!(round_trip(look), *look, "{view}");
            assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()).as_ref(), Some(&applied.design), "{view}");
            // And the structure of the far path is known: a morph of another structure is still refused.
            let other = json!({ "svg": art, "mouth": { "target": "mouth", "open": format!("M {far} {far} Q {far} {far} {far} {far}") } });
            has(&refused(None, other), "mouth open is MQ with 6 numbers, the d of mouth is ML with 4 numbers");
        }
    }

    #[test]
    fn a_value_accepted_at_a_bound_is_still_inside_once_it_is_stored() {
        // The bounds of this viewBox are sums that fall just inside their three decimals
        // (-4.8 - 65.6 is -70.39999999999999): a value a little inside such a bound, stored
        // rounded, lands on it, and must pass the same check when the file is read again.
        let request = json!({
            "svg": "<svg viewBox='-4.8 0 16.4 16.4'><circle id='c' cx='-70.3998' cy='8' r='3'/><circle id='d' cx='8' cy='8' r='3'/></svg>",
            "pivots": { "c": [-70.3998, 8] }
        });
        let applied = accepted(None, request);
        assert_eq!(attrs(node(applied.design.look.as_ref().unwrap(), "c"))["cx"], json!(-70.4));
        assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()).as_ref(), Some(&applied.design));
        // A later call checks the pivot again: one this call did not send is no reason to refuse it.
        accepted(Some(&applied.design), json!({ "states": { "idle": [] } }));
    }

    // ------------------------------------------------------------ the adversary: what moves the artwork

    /// A part of every kind that can be one, each with the attributes its props animate.
    const KINDS: &str = "<svg viewBox='0 0 200 200'><g id='g'><circle cx='20' cy='20' r='5'/></g><path id='path' d='M10 10 L20 20' stroke='#333333' stroke-width='2' fill='#ff0000'/><circle id='circle' cx='20' cy='20' r='5'/><ellipse id='ellipse' cx='20' cy='20' rx='5' ry='5'/><rect id='rect' x='10' y='10' width='10' height='10'/><line id='line' x1='10' y1='10' x2='20' y2='20'/><polyline id='polyline' points='10,10 20,20 30,10'/><polygon id='polygon' points='10,10 20,20 30,10'/><text id='text' x='10' y='50'>a</text><use id='use' href='#circle' x='40'/></svg>";

    #[test]
    fn every_prop_fits_exactly_the_elements_that_have_it() {
        let shapes = [Tag::Path, Tag::Circle, Tag::Ellipse, Tag::Rect, Tag::Line, Tag::Polyline, Tag::Polygon];
        let has_it = |prop: Prop, tag: Tag| match prop {
            Prop::D => tag == Tag::Path,
            Prop::Cx | Prop::Cy => matches!(tag, Tag::Circle | Tag::Ellipse),
            Prop::R => tag == Tag::Circle,
            Prop::Rx | Prop::Ry => matches!(tag, Tag::Ellipse | Tag::Rect),
            Prop::X | Prop::Y => matches!(tag, Tag::Rect | Tag::Text),
            Prop::Width | Prop::Height => tag == Tag::Rect,
            Prop::X1 | Prop::Y1 | Prop::X2 | Prop::Y2 => tag == Tag::Line,
            _ => matches!(tag, Tag::G | Tag::Text | Tag::Use) || shapes.contains(&tag),
        };
        for tag in Tag::ALL {
            for prop in Prop::ALL {
                assert_eq!(prop.fits(tag), has_it(prop, tag), "{} on {}", prop.name(), tag.name());
            }
        }
        // And through a design call, on a part of every kind.
        let parts = [Tag::G, Tag::Path, Tag::Circle, Tag::Ellipse, Tag::Rect, Tag::Line, Tag::Polyline, Tag::Polygon, Tag::Text, Tag::Use];
        for tag in parts {
            for prop in Prop::ALL {
                let values = match prop {
                    Prop::D => json!(["M10 10 L20 20", "M10 10 L20 25"]),
                    Prop::Fill | Prop::Stroke => json!(["#ff0000", "#00ff00"]),
                    _ => json!([1, 1]),
                };
                let request = json!({ "svg": KINDS, "animations": { "a": { "duration": 1, "tracks": [{ "target": tag.name(), "prop": prop.name(), "values": values }] } } });
                assert_eq!(apply(None, &request).is_ok(), has_it(prop, tag), "{} on {}", prop.name(), tag.name());
                // The mouth drives one number of the same table.
                if let Some(drive) = MouthProp::ALL.into_iter().find(|m| m.prop() == prop) {
                    let request = json!({ "svg": KINDS, "mouth": { "target": tag.name(), "prop": serde_json::to_value(drive).unwrap(), "open": 1 } });
                        assert_eq!(apply(None, &request).is_ok(), has_it(prop, tag), "mouth {} on {}", prop.name(), tag.name());
                }
            }
        }
        // points is a prop of nothing.
        has(&refused(None, json!({ "svg": KINDS, "animations": { "a": { "duration": 1, "tracks": [{ "target": "polygon", "prop": "points", "values": ["0 0 1 1", "0 0 2 2"] }] } } })), "points cannot be animated");
    }

    #[test]
    fn d_keyframes_share_their_layout_number_for_number() {
        let bad = |values: Value| refused(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": values }])));
        // The same letters and as many numbers, but the numbers stand under other commands: the
        // browser morphs token by token, and these do not line up.
        has(&bad(json!(["M0 0 L1 1 2 2 L3 3", "M0 0 L1 1 L2 2 3 3"])), "d values differ in structure: M2 L4 L2 against M2 L2 L4 (at animations.a.tracks[0].values[1]): same commands, same count of numbers, same arc flags");
        has(&bad(json!(["M0 0 1 1 L2 2", "M0 0 L1 1 2 2"])), "d values differ in structure: M4 L2 against M2 L4");
        has(&bad(json!(["M0 0 H1 2 V3", "M0 0 H1 V2 3"])), "d values differ in structure: M2 H2 V1 against M2 H1 V2");
        has(&bad(json!(["M0 0 L1 1 2 2 A5 5 0 0 1 9 9 Z", "M0 0 1 1 L2 2 A5 5 0 0 1 9 9 Z"])), "d values differ in structure: M2 L4 A7 Z against M4 L2 A7 Z");
        has(&bad(json!(["M0 0 A5 5 0 0 1 9 9 5 5 0 0 1 8 8 L1 1", "M0 0 A5 5 0 0 1 9 9 L5 5 0 0 1 1 8 8"])), "d values differ in structure");
        // The same layout written in two ways is one structure.
        let look = accepted(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": ["M84,96Q100,104,116,96", "M 84 96 Q 100 110 116 96", "M84 96Q100 104 116 96"] }]))).design.look.unwrap();
        assert_eq!(serde_json::to_value(&look.animations["a"].tracks[0].values).unwrap(), json!(["M 84 96 Q 100 104 116 96", "M 84 96 Q 100 110 116 96", "M 84 96 Q 100 104 116 96"]));
        accepted(None, tracks(json!([{ "target": "mouth", "prop": "d", "values": ["M0 0 L1 1 2 2 L3 3", "M5 5 L6 6 7 7 L8 8"] }])));
        // Against the element's own d the layout counts as well: for null, and for the mouth.
        let art = svg("<path id='m' d='M0 0 L10 10 20 20 L30 30' fill='none' stroke='#333333'/>");
        let why = refused(None, json!({ "svg": art, "animations": { "a": { "duration": 1, "tracks": [{ "target": "m", "prop": "d", "values": [null, "M0 0 L10 10 L20 25 30 30", null] }] } } }));
        has(&why, "null is the d of m, M2 L4 L2, and the keyframes are M2 L2 L4 (at animations.a.tracks[0]): write them in the structure of that d");
        let why = refused(None, json!({ "svg": art, "mouth": { "target": "m", "open": "M0 0 L10 10 L20 25 30 30" } }));
        has(&why, "mouth open is M2 L2 L4, the d of m is M2 L4 L2 (at mouth): write open in the structure of that d, or give closed too");
        let why = refused(None, json!({ "svg": art, "mouth": { "target": "m", "open": "M0 0 L10 10 20 25 L30 30", "closed": "M0 0 L10 10 L20 20 30 30" } }));
        has(&why, "mouth open and closed differ in structure: M2 L4 L2 against M2 L2 L4");
        accepted(None, json!({ "svg": art, "mouth": { "target": "m", "open": "M0 0 L10 10 20 25 L30 30" } }));
        // Where another structure is only warned about, the warning says what differs.
        let applied = accepted(None, json!({ "svg": art, "animations": { "a": { "duration": 1, "loop": true, "tracks": [{ "target": "m", "prop": "d", "values": ["M0 0 L10 10 L20 25 30 30", "M0 0 L10 10 L20 20 30 30"] }] } } }));
        assert_eq!(applied.warnings, ["these d keyframes are M2 L2 L4, the d of m is M2 L4 L2 (at animations.a.tracks[0]): the shape jumps at its start and end"]);
        let applied = accepted(None, json!({ "svg": art, "mouth": { "target": "m", "open": "M0 0 L10 10 L20 25 30 30", "closed": "M0 0 L10 10 L20 20 30 30" } }));
        assert_eq!(applied.warnings, ["the mouth's closed is M2 L2 L4, the d of m is M2 L4 L2 (at mouth): the shape jumps when a line starts and ends"]);
    }

    #[test]
    fn the_paint_of_a_loop_ends_on_the_value_it_starts_with() {
        let looped = |target: Value, prop: &str, values: Value| figure(json!({ "animations": { "a": { "duration": 2, "loop": true, "tracks": [{ "target": target, "prop": prop, "values": values }] } } }));
        // Where a loop starts again its last keyframe meets its first: no time apart at all.
        has(&refused(None, looped(json!("face"), "opacity", json!([0.9, 0]))), "the loop's opacity ends on another value than it starts with (at animations.a.tracks[0]): end fill, stroke and opacity as they begin, so nothing flashes");
        has(&refused(None, looped(json!("face"), "fill", json!(["#ffffff", "#000000"]))), "the loop's fill ends on another value than it starts with");
        has(&refused(None, looped(json!("face"), "stroke", json!(["#ffffff", "#000000", "#fffffe"]))), "the loop's stroke ends on another value than it starts with");
        has(&refused(None, looped(json!("face"), "opacity", json!([0.9, 0.2, 0.899]))), "the loop's opacity ends on another value than it starts with");
        // The shortest such loop would flash six times a second.
        has(&refused(None, figure(json!({ "animations": { "a": { "duration": 0.17, "loop": true, "tracks": [{ "target": "face", "prop": "fill", "values": ["white", "black"] }] } } }))), "the loop's fill ends on another value than it starts with");
        // Ending as it began, in any spelling of the value.
        accepted(None, looped(json!("face"), "opacity", json!([0.9, 0, 0.9])));
        accepted(None, looped(json!("face"), "opacity", json!([0.5, 0.5])));
        accepted(None, looped(json!("face"), "opacity", json!([0.5, 0.2, 0.5004])));
        accepted(None, looped(json!("face"), "fill", json!(["#f00", "blue", "red"])));
        accepted(None, looped(json!("face"), "stroke", json!(["rgb(0 0 0 / 0)", "blue", "transparent"])));
        accepted(None, looped(json!("face"), "opacity", json!([null, 0.2, null])));
        // null is the artwork's own value: it closes the loop only with that value, on every target.
        accepted(None, looped(json!("face"), "opacity", json!([null, 0.2, 0.9])));
        accepted(None, looped(json!("face"), "fill", json!(["#ffcc00", "#ff0000", null])));
        accepted(None, looped(json!("arm"), "opacity", json!([1, 0.2, null])));
        has(&refused(None, looped(json!("face"), "opacity", json!([null, 0.2, 1]))), "the loop's opacity ends on another value than it starts with");
        has(&refused(None, looped(json!("face"), "fill", json!(["#ffcc01", "#ff0000", null]))), "the loop's fill ends on another value than it starts with");
        has(&refused(None, looped(json!(["face", "arm"]), "opacity", json!([null, 0.2, 0.9]))), "the loop's opacity ends on another value than it starts with");
        // What does not flash may jump where the loop starts again: a turn, a size, a place, a shape.
        let applied = accepted(
            None,
            figure(json!({ "animations": { "a": { "duration": 2, "loop": true, "tracks": [
                { "target": "arm", "prop": "rotate", "values": [0, 360] },
                { "target": "face", "prop": "r", "values": [30, 34] },
                { "target": "face", "prop": "strokeWidth", "values": [2, 4] },
                { "target": "arm", "prop": "translateY", "values": [0, -10] },
                { "target": "mouth", "prop": "d", "values": ["M84 96 Q100 104 116 96", "M84 96 Q100 120 116 96"] }
            ] } } })),
        );
        assert_eq!(applied.warnings, [""; 0]);
        // An animation that is not a loop ends by itself: its jump back is the lint's to report.
        let one_shot = |every: Value| figure(json!({ "animations": { "a": { "duration": 2, "every": every, "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0] }] } } }));
        assert_eq!(accepted(None, one_shot(json!(null))).warnings.len(), 1);
        assert_eq!(accepted(None, one_shot(json!([2, 5]))).warnings.len(), 1);
    }

    #[test]
    fn an_animation_closes_when_its_paint_ends_as_it_starts() {
        let look = |animations: Value| accepted(None, figure(json!({ "animations": animations }))).design.look.expect("a look");
        let one = |target: Value, prop: &str, values: Value| look(json!({ "a": { "duration": 2, "tracks": [{ "target": target, "prop": prop, "values": values }] } }));
        // A one-shot may end elsewhere: it is then not for playing several times in a row.
        assert!(!closes(&one(json!("face"), "opacity", json!([0.9, 0])), "a"));
        assert!(!closes(&one(json!("face"), "fill", json!(["#ffffff", "#000000"])), "a"));
        assert!(!closes(&one(json!("face"), "stroke", json!(["#333333", "#000000", "#333334"])), "a"));
        // Ending as it began, in any spelling of the value.
        assert!(closes(&one(json!("face"), "opacity", json!([0.9, 0, 0.9])), "a"));
        assert!(closes(&one(json!("face"), "opacity", json!([0.5, 0.2, 0.5004])), "a"));
        assert!(closes(&one(json!("face"), "fill", json!(["#f00", "blue", "red"])), "a"));
        assert!(closes(&one(json!("face"), "opacity", json!([null, 0.2, null])), "a"));
        // null is each target's own value: it closes only with that value, on every target.
        assert!(closes(&one(json!("face"), "opacity", json!([null, 0.2, 0.9])), "a"));
        assert!(closes(&one(json!("face"), "fill", json!(["#ffcc00", "#ff0000", null])), "a"));
        assert!(closes(&one(json!("arm"), "opacity", json!([1, 0.2, null])), "a"));
        assert!(!closes(&one(json!("face"), "opacity", json!([null, 0.2, 1])), "a"));
        assert!(!closes(&one(json!(["face", "arm"]), "opacity", json!([null, 0.2, 0.9])), "a"));
        // What does not flash may end anywhere.
        let moving = look(json!({ "a": { "duration": 2, "tracks": [
            { "target": "arm", "prop": "rotate", "values": [0, 360] },
            { "target": "face", "prop": "r", "values": [30, 34] },
            { "target": "face", "prop": "strokeWidth", "values": [2, 4] },
            { "target": "mouth", "prop": "d", "values": ["M84 96 Q100 104 116 96", "M84 96 Q100 120 116 96"] }
        ] } }));
        assert!(closes(&moving, "a"));
        // One open paint track among closed ones is enough.
        let mixed = look(json!({ "a": { "duration": 2, "tracks": [
            { "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9] },
            { "target": "arm", "prop": "fill", "values": ["#ffcc00", "#ff0000"] }
        ] } }));
        assert!(!closes(&mixed, "a"));
        // A loop was held to the rule when it was designed.
        let looped = look(json!({ "a": { "duration": 2, "loop": true, "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9] }] } }));
        assert!(closes(&looped, "a"));
        // Built-in motion always closes, and so does a name the look does not define.
        assert!(BUILTIN.into_iter().all(|name| closes(&mixed, name)));
        assert!(closes(&mixed, "nothing"));
        // An animation of the look's own under a built-in's name is the one that is asked.
        let own_wave = look(json!({ "wave": { "duration": 2, "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0] }] } }));
        assert!(!closes(&own_wave, "wave"));
    }

    #[test]
    fn the_spacing_of_keyframes_is_the_times_step_by_the_duration() {
        let with = |duration: f64, track: Value| figure(json!({ "animations": { "a": { "duration": duration, "tracks": [track] } } }));
        let pulse = |n: usize| json!({ "target": "face", "prop": "opacity", "values": (0..n).map(|i| if i % 2 == 0 { 0.9 } else { 0.2 }).collect::<Vec<f64>>() });
        // Exactly 0.17 s apart, however the product of a step and the duration comes out.
        accepted(None, with(0.34, pulse(3)));
        accepted(None, with(1.7, pulse(11)));
        accepted(None, with(2.55, pulse(16)));
        accepted(None, with(0.51, pulse(4)));
        has(&refused(None, with(0.33, pulse(3))), "keyframes 0 and 1 of opacity are 0.165 s apart");
        has(&refused(None, with(2.5, pulse(16))), "keyframes 0 and 1 of opacity are 0.167 s apart");
        // The duration counts as it is stored, to the millisecond.
        accepted(None, with(0.3396, pulse(3)));
        has(&refused(None, with(0.3394, pulse(3))), "of opacity are");
        // The step of times that is as short as allowed.
        let timed = |prop: &str, times: Value, values: Value| json!({ "target": "face", "prop": prop, "values": values, "times": times });
        accepted(None, with(1.0, timed("opacity", json!([0, 0.17, 1]), json!([0.9, 0.2, 0.9]))));
        has(&refused(None, with(1.0, timed("opacity", json!([0, 0.169, 1]), json!([0.9, 0.2, 0.9])))), "keyframes 0 and 1 of opacity are 0.169 s apart");
        has(&refused(None, with(1.0, timed("opacity", json!([0, 0.831, 1]), json!([0.9, 0.2, 0.9])))), "keyframes 1 and 2 of opacity are 0.169 s apart");
        accepted(None, with(1.0, timed("cx", json!([0, 0.04, 1]), json!([100, 102, 100]))));
        has(&refused(None, with(1.0, timed("cx", json!([0, 0.039, 1]), json!([100, 102, 100])))), "keyframes 0 and 1 of cx are 0.039 s apart (at animations.a.tracks[0]): at least 0.04 s between keyframes");
        accepted(None, with(30.0, timed("cx", json!([0, 0.002, 1]), json!([100, 102, 100]))));
        has(&refused(None, with(30.0, timed("cx", json!([0, 0.001, 1]), json!([100, 102, 100])))), "keyframes 0 and 1 of cx are 0.03 s apart");
        has(&refused(None, with(30.0, timed("cx", json!([0, 0.0004, 1]), json!([100, 102, 100])))), "times do not rise from 0 to 1");
        // A track's own times count, not the animation's, and the animation's count where a track has none.
        let both = |own: Value| figure(json!({ "animations": { "a": { "duration": 1, "times": [0, 0.5, 1], "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9], "times": own }] } } }));
        accepted(None, both(json!(null)));
        has(&refused(None, both(json!([0, 0.1, 1]))), "keyframes 0 and 1 of opacity are 0.1 s apart");
        // A delay moves a track, it does not stretch it.
        has(&refused(None, with(0.3, json!({ "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9], "delay": 0.3 }))), "keyframes 0 and 1 of opacity are 0.15 s apart");
    }

    #[test]
    fn the_mouth_drives_only_what_the_contract_names() {
        let mouth = |m: Value| figure(json!({ "mouth": m, "pivots": { "eyes": [90, 75], "head": [100, 110], "mouth": [100, 100] } }));
        for prop in [json!("opacity"), json!("rotate"), json!("translateX"), json!("fill"), json!("stroke"), json!("strokeWidth"), json!("d"), json!("cx"), json!("x"), json!("width"), json!("rx"), json!("points"), json!("transform"), json!("constructor"), json!(""), json!("ScaleY"), json!(5), json!(["scaleY"]), json!(true), json!({})] {
            has(&refused(None, mouth(json!({ "target": "eyes", "prop": prop, "open": 0.5 }))), "the mouth's prop is not one it can drive (at mouth): scaleY, scaleX, scale, translateY, ry, r or height");
        }
        // The ends of every range.
        for (target, prop, low, high) in [("eyes", "scaleY", -10.0, 10.0), ("eyes", "scaleX", -10.0, 10.0), ("mouth", "scale", -10.0, 10.0), ("head", "translateY", -800.0, 800.0), ("belly", "ry", 0.0, 800.0), ("arm", "ry", 0.0, 800.0), ("face", "r", 0.0, 800.0), ("arm", "height", 0.0, 800.0)] {
            accepted(None, mouth(json!({ "target": target, "prop": prop, "open": high, "closed": low })));
            has(&refused(None, mouth(json!({ "target": target, "prop": prop, "open": high + 0.5 }))), &format!("mouth open {} is out of range", fmt_num(high + 0.5)));
            has(&refused(None, mouth(json!({ "target": target, "prop": prop, "open": high, "closed": low - 0.5 }))), &format!("mouth closed {} is out of range", fmt_num(low - 0.5)));
            has(&refused(None, mouth(json!({ "target": target, "prop": prop, "open": [1] }))), "mouth open is a list");
            has(&refused(None, mouth(json!({ "target": target, "prop": prop, "open": 1, "closed": "shut" }))), "mouth closed is a string");
        }
        has(&refused(None, mouth(json!({ "target": ["eyes"], "prop": "scaleY", "open": 2 }))), "a mouth drive without a target");
        has(&refused(None, mouth(json!({ "target": 5, "open": "M0 0" }))), "a mouth drive without a target");
        has(&refused(None, mouth(json!({ "target": "mouth", "closed": "M84 96 Q100 104 116 96" }))), "a mouth drive without open");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": "M0 0 A5 5 0 0 1 9 9", "closed": "M0 0 A5 5 0 1 1 9 9" }))), "mouth open and closed differ in structure: MA with 7 numbers and arc flags 01 against MA with 7 and arc flags 11");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": "M84 96 Q100 104 116 96", "closed": 5 }))), "mouth closed is a number: path data as a string");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": format!("M0 0{}", " L1 1".repeat(200)), "closed": "M0 0" }))), "mouth open: 1004 characters of path data: at most 1000");
        // prop null is no prop: a morph.
        accepted(None, mouth(json!({ "target": "mouth", "prop": null, "open": "M84 96 Q100 120 116 96" })));
        // Four drives may be four morphs, and their path data counts towards the look's.
        let d = |n: usize| format!("M 0 0{} h 10", " L 100 100".repeat(n));
        let four = |n: usize| mouth(json!(vec![json!({ "target": "mouth", "open": d(n), "closed": d(n) }); MAX_MOUTH_DRIVES]));
        assert_eq!(d(99).len(), MAX_D_KEYFRAME_CHARS);
        has(&refused(None, four(99)), "8000 characters of d keyframes in the look: at most 6000 over all tracks and the mouth");
        assert_eq!(accepted(None, four(74)).design.look.unwrap().mouth.len(), 4);
        has(&refused(None, four(75)), "6080 characters of d keyframes in the look");
        has(&refused(None, mouth(json!({ "target": "mouth", "open": format!("{}0", d(99)), "closed": d(99) }))), "mouth open: 1001 characters of path data: at most 1000");
    }

    #[test]
    fn d_keyframes_keep_to_their_limits_to_the_character() {
        // One structure, lengths that differ by the digits of the numbers.
        let frame = |long: usize, tail: &str| format!("M 0 0{}{} h {tail}", " L 100 100".repeat(long), " L 10 10".repeat(100 - long));
        assert_eq!((frame(95, "10").len(), frame(0, "1").len()), (1000, 809));
        let with = |values: Vec<String>| tracks(json!([{ "target": "mouth", "prop": "d", "values": values }]));
        accepted(None, with(vec![frame(95, "10"), frame(95, "10")]));
        has(&refused(None, with(vec![frame(95, "100"), frame(95, "10")])), "a d keyframe: 1001 characters of path data: at most 1000");
        // 6000 in a look: seven keyframes that add up to it, and to one more.
        let seven = |long: [usize; 7], last: &str| with(long.iter().enumerate().map(|(i, n)| frame(*n, if i == 6 { last } else { "10" })).collect());
        let total = |request: &Value| request["animations"]["a"]["tracks"][0]["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().len()).sum::<usize>();
        let fits = seven([95, 70, 0, 0, 0, 0, 0], "10");
        assert_eq!(total(&fits), MAX_D_KEYFRAMES_TOTAL);
        accepted(None, fits);
        let over = seven([95, 71, 0, 0, 0, 0, 0], "1");
        assert_eq!(total(&over), MAX_D_KEYFRAMES_TOTAL + 1);
        has(&refused(None, over), "6001 characters of d keyframes in the look: at most 6000 over all tracks and the mouth");
    }

    #[test]
    fn gaze_and_voice_keep_to_their_ranges() {
        let gaze = |view: &str, g: Value| json!({ "svg": format!("<svg viewBox='{view}'><circle id='p' cx='8' cy='8' r='2'/></svg>"), "roles": { "pupils": "p" }, "gaze": g });
        for (view, most) in [("0 0 16 16", 0.8), ("0 0 200 16", 10.0), ("-50 -50 100 4096", 5.0), ("0 0 4096 16", 204.8), ("0 0 33.333 16", 1.667)] {
            assert_eq!(accepted(None, gaze(view, json!(most))).design.look.unwrap().gaze, Some(Num::new(most)), "{view}");
            assert_eq!(accepted(None, gaze(view, json!(-0.0))).design.look.unwrap().gaze, Some(Num::new(0.0)));
            has(&refused(None, gaze(view, json!(most + 0.001))), &format!("0 to {} here", fmt_num(most)));
            has(&refused(None, gaze(view, json!(-0.001))), "gaze is not a number in range");
        }
        for bad in [json!("3"), json!(true), json!([3]), json!({ "max": 3 }), json!(1e300)] {
            has(&refused(None, gaze("0 0 200 200", bad)), "gaze is not a number in range");
        }

        for lang in ["EN", "zh-Hant-TW", "sr-Latn-RS", "de-1996", "en-US-POSIX", "es-419", "ast", "en-GB-oxendict"] {
            assert_eq!(accepted(None, json!({ "voice": { "lang": lang } })).design.voice.lang.as_deref(), Some(lang));
        }
        for lang in ["x", "abcd", "en-a", "en-abcdefghi", "123", "e1", "en-US-u-ca", "en_US", "en-US ", " en", "en--US", "en-US-", "*", "en-*", "en;q=0.8", "en,de", "\u{FF45}\u{FF4E}", "e\u{0301}n", "en-U\u{200D}S", "<b>", "en\n"] {
            has(&refused(None, json!({ "voice": { "lang": lang } })), "a language tag such as en-GB, de or pt-BR");
        }
        for (key, bad) in [("lang", json!(["en"])), ("lang", json!(true)), ("pitch", json!(true)), ("pitch", json!("1")), ("pitch", json!([1])), ("rate", json!({})), ("prefer", json!({})), ("prefer", json!(5)), ("prefer", json!([["a"]])), ("prefer", json!([null]))] {
            assert!(apply(None, &json!({ "voice": { key: bad.clone() } })).is_err(), "{key}: {bad}");
        }
        // Null is "not given" for every key, and an empty list is no fragments.
        assert_eq!(accepted(None, json!({ "voice": { "lang": null, "prefer": null, "pitch": null, "rate": null } })).design.voice, Voice::default());
        assert_eq!(accepted(None, json!({ "voice": { "prefer": [] } })).design.voice, Voice::default());
        // A fragment is one line of plain text of at most 48 characters, counted after it is cleaned.
        let applied = accepted(None, json!({ "voice": { "prefer": [format!(" {}\u{202E}\n", "é".repeat(MAX_VOICE_PREFER_CHARS)), "a\tb", "Google\u{200B} UK", "x", "x"] } }));
        assert_eq!(applied.design.voice.prefer, ["é".repeat(MAX_VOICE_PREFER_CHARS).as_str(), "a b", "Google UK", "x", "x"]);
        for empty in ["", " ", "\u{200B}", "\u{202E}\u{202C}", "\n"] {
            has(&refused(None, json!({ "voice": { "prefer": ["a", empty] } })), "an empty fragment (at voice.prefer[1])");
        }
        // Keys of a later version, however many: one warning of bounded length, and the call succeeds.
        let mut voice: Map<String, Value> = (0..500).map(|i| (format!("future-key-{i}\u{202E}"), json!(i))).collect();
        voice.insert("lang".into(), json!("de"));
        let applied = accepted(None, json!({ "voice": voice }));
        assert_eq!(applied.design.voice, Voice { lang: Some("de".into()), ..Voice::default() });
        assert_eq!(applied.warnings.len(), 1);
        assert!(applied.warnings[0].chars().count() <= MAX_PROBLEM_CHARS && !applied.warnings[0].chars().any(|c| c.is_control() || c == '\u{202E}'), "{}", applied.warnings[0]);
    }

    // ------------------------------------------------------------ the adversary: the rules of a call

    #[test]
    fn keep_keeps_only_what_still_fits_the_new_artwork() {
        let old = accepted(
            None,
            figure(json!({
                "pivots": { "arm": [146, 100], "head": [100, 110], "pupil": [90, 75], "cane": [600, 600] },
                "roles": { "head": "head", "pupils": "pupil", "armR": "arm", "body": "body" },
                "animations": {
                    "grow": { "duration": 1, "tracks": [{ "target": "face", "prop": "r", "values": [null, 34, null] }] },
                    "morph": { "duration": 1, "tracks": [{ "target": "mouth", "prop": "d", "values": [null, "M84 96 Q100 120 116 96", null] }] },
                    "tint": { "duration": 1, "tracks": [{ "target": "face", "prop": "fill", "values": [null, "#ff0000", null] }] },
                    "far": { "duration": 1, "tracks": [{ "target": "arm", "prop": "translateX", "values": [0, 700, 0] }] },
                    "near": { "duration": 1, "tracks": [{ "target": "arm", "prop": "translateX", "values": [0, 7, 0] }] },
                    "both": { "duration": 1, "tracks": [{ "target": ["arm", "cane"], "prop": "opacity", "values": [null, 0.5, null] }] },
                    "blink": { "duration": 0.2, "every": [2, 5], "tracks": [{ "target": "pupil", "prop": "scaleY", "values": [1, 0.1, 1] }] }
                },
                "states": { "idle": ["grow", "near", "breathe"], "working": ["far", "morph"], "done": ["blink"], "error": [] },
                "mouth": [{ "target": "mouth", "open": "M84 96 Q100 120 116 96" }, { "target": "face", "prop": "r", "open": 34 }, { "target": "arm", "prop": "height", "open": 50 }],
                "gaze": 9
            })),
        )
        .design;
        // A smaller viewBox. head is now a gradient, pupil sits in a clip path, cane is gone, face is
        // a rect, the mouth has another structure, the arm is still a rect.
        let new_svg = "<svg viewBox='0 0 100 100'><defs><linearGradient id='head'/><clipPath id='clip'><circle id='pupil' cx='50' cy='50' r='5'/></clipPath></defs><g id='body'><rect id='face' x='20' y='20' width='60' height='60' fill='#ffcc00'/><path id='mouth' d='M30 60 L50 70 L70 60' fill='none' stroke='#333333'/><rect id='arm' x='80' y='40' width='10' height='30' fill='#ffcc00'/></g></svg>";
        let kept = accepted(Some(&old), json!({ "svg": new_svg, "keep": true }));
        let look = kept.design.look.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&look.pivots).unwrap(), json!({ "arm": [146, 100] }));
        assert_eq!(serde_json::to_value(&look.roles).unwrap(), json!({ "body": "body", "armR": "arm" }));
        // grow: a rect has no r. morph: null is another shape now. far: 700 is beyond what this viewBox reaches. both: cane is gone. blink: pupil is no part.
        assert_eq!(look.animations.keys().collect::<Vec<_>>(), ["near", "tint"]);
        assert_eq!(serde_json::to_value(&look.states).unwrap(), json!({ "idle": ["near", "breathe"], "error": [], "done": ["blink"] }));
        assert_eq!(serde_json::to_value(&look.mouth).unwrap(), json!([{ "target": "arm", "prop": "height", "open": 50 }]));
        assert_eq!(look.gaze, None);
        assert_eq!(kept.kept, ["pivots arm", "roles body, armR", "animations near, tint", "states idle, error, done", "mouth"]);
        assert_eq!(kept.cleared, ["pivots cane, head, pupil", "roles head, pupils", "animations blink, both, far, grow, morph", "states idle (grow), working (far, morph)", "mouth", "gaze"]);
        // What was kept is a look like any other: it reads back, and merges go on from it.
        assert_eq!(round_trip(look), *look);
        assert_eq!(check_stored(serde_json::to_value(&kept.design).unwrap()).as_ref(), Some(&kept.design));
        // Without keep nothing of it stays, whatever would fit.
        let fresh = accepted(Some(&old), json!({ "svg": new_svg, "keep": false }));
        let look = fresh.design.look.as_ref().unwrap();
        assert!(look.pivots.is_empty() && look.roles.is_empty() && look.animations.is_empty() && look.states.is_empty() && look.mouth.is_empty() && look.gaze.is_none());
        assert_eq!(fresh.kept, [""; 0]);
        assert_eq!(fresh.cleared.len(), 6);
        assert_eq!(accepted(Some(&old), json!({ "svg": new_svg, "keep": null })).design, fresh.design);
        assert_eq!(accepted(Some(&old), json!({ "svg": new_svg })).design, fresh.design);
        // A refused call leaves the caller's design as it was: the same value, to the last field.
        let before = old.clone();
        for request in [json!({ "svg": new_svg, "keep": true, "roles": { "head": "head" } }), json!({ "svg": "<svg><g>", "keep": true }), json!({ "animations": { "grow": null } }), json!({ "gaze": 99 }), json!({ "name": "x".repeat(99) })] {
            assert!(apply(Some(&old), &request).is_err(), "{request}");
            assert_eq!(old, before);
        }
    }

    #[test]
    fn null_removes_one_entry_or_a_whole_field_and_nothing_else() {
        let first = accepted(
            None,
            figure(json!({
                "pivots": { "arm": [146, 100], "head": [100, 110] }, "roles": { "head": "head", "body": "body", "pupils": "pupil" },
                "animations": { "swing": rotate("arm"), "nod": rotate("head") }, "states": { "idle": ["swing"], "done": ["nod"] },
                "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96" }, "gaze": 4, "name": "Pip", "voice": { "lang": "de" }
            })),
        )
        .design;
        let look = |request: Value| accepted(Some(&first), request).design.look.unwrap();
        let before = first.look.clone().unwrap();
        // One entry.
        assert_eq!(look(json!({ "pivots": { "arm": null } })).pivots.keys().collect::<Vec<_>>(), ["head"]);
        assert_eq!(look(json!({ "roles": { "head": null } })).roles.keys().copied().collect::<Vec<_>>(), [Role::Body, Role::Pupils]);
        assert_eq!(look(json!({ "states": { "idle": null } })).states.keys().copied().collect::<Vec<_>>(), [LookState::Done]);
        assert_eq!(look(json!({ "animations": { "swing": null }, "states": { "idle": null } })).animations.keys().collect::<Vec<_>>(), ["nod"]);
        // An entry that is not there: nothing happens, and nothing is said.
        for request in [json!({ "pivots": { "cane": null } }), json!({ "roles": { "eyes": null } }), json!({ "states": { "error": null } }), json!({ "animations": { "wave": null } }), json!({ "pivots": {} }), json!({ "roles": {}, "states": {}, "animations": {} })] {
            let applied = accepted(Some(&first), request);
            assert_eq!((applied.design.look.as_ref(), applied.warnings.len()), (Some(&before), 0));
        }
        // A null for a name that could not be an entry is still only a removal of nothing.
        assert_eq!(look(json!({ "pivots": { "no such part": null }, "animations": { "constructor": null } })), before);
        // A whole field; each alone leaves the others.
        assert_eq!(look(json!({ "pivots": null })), Look { pivots: BTreeMap::new(), ..before.clone() });
        assert_eq!(look(json!({ "roles": null })), Look { roles: BTreeMap::new(), ..before.clone() });
        assert_eq!(look(json!({ "mouth": null })), Look { mouth: Vec::new(), ..before.clone() });
        assert_eq!(look(json!({ "mouth": [] })), Look { mouth: Vec::new(), ..before.clone() });
        assert_eq!(look(json!({ "gaze": null })), Look { gaze: None, ..before.clone() });
        assert_eq!(look(json!({ "states": null })), Look { states: BTreeMap::new(), ..before.clone() });
        assert_eq!(look(json!({ "states": null, "animations": null })), Look { states: BTreeMap::new(), animations: BTreeMap::new(), ..before.clone() });
        // An empty list is an entry: nothing moves in that state.
        assert_eq!(serde_json::to_value(&look(json!({ "states": { "idle": [] } })).states).unwrap(), json!({ "idle": [], "done": ["nod"] }));
        // The name and the voice are not the look's: null clears each, and the look stays.
        let cleared = accepted(Some(&first), json!({ "name": null, "voice": null })).design;
        assert_eq!((cleared.name.as_str(), &cleared.voice, cleared.look.as_ref()), ("", &Voice::default(), Some(&before)));
        // svg: null is not a way to remove the artwork.
        has(&refused(Some(&first), json!({ "svg": null })), "svg is null");
    }

    // ------------------------------------------------------------ the adversary: stored files, sizes, messages

    /// Sets the value at a JSON pointer, adding the last key when it is not there.
    #[track_caller]
    fn put(v: &mut Value, path: &str, value: Value) {
        let (parent, key) = path.rsplit_once('/').expect("a path");
        match v.pointer_mut(parent).unwrap_or_else(|| panic!("no {parent}")) {
            Value::Object(o) => {
                o.insert(key.to_string(), value);
            }
            Value::Array(a) => a[key.parse::<usize>().expect("an index")] = value,
            other => panic!("{parent} is {other}"),
        }
    }

    #[test]
    fn a_stored_file_is_believed_in_nothing() {
        let design = accepted(
            None,
            figure(json!({
                "name": "Pip", "voice": { "lang": "en-GB" }, "pivots": { "head": [100, 110] }, "roles": { "head": "head", "pupils": "pupil" },
                "animations": { "nod": rotate("head") }, "states": { "done": ["nod"] }, "mouth": { "target": "mouth", "open": "M84 96 Q100 120 116 96" }, "gaze": 3
            })),
        )
        .design;
        let stored = serde_json::to_value(&design).unwrap();
        let tamper = |path: &str, value: Value| {
            let mut v = stored.clone();
            put(&mut v, path, value);
            check_stored(v)
        };
        // The places the paths below name.
        assert_eq!((&stored["look"]["nodes"][1]["id"], &stored["look"]["nodes"][1]["children"][0]["id"], &stored["look"]["nodes"][1]["children"][4]["id"]), (&json!("body"), &json!("belly"), &json!("label")));
        assert_eq!(stored["look"]["nodes"][1]["children"][1]["children"][0]["id"], json!("face"));
        let (body, belly, face, label) = ("/look/nodes/1", "/look/nodes/1/children/0", "/look/nodes/1/children/1/children/0", "/look/nodes/1/children/4");
        let drive = json!({ "target": "mouth", "open": "M84 96 Q100 120 116 96" });
        let refused: Vec<(String, Value)> = vec![
            // The shape of the look itself.
            ("/look/viewBox".into(), json!([0, 0, 200])),
            ("/look/viewBox".into(), json!("0 0 200 200")),
            ("/look/viewBox".into(), json!([0, 0, 200, 200, 0])),
            ("/look/viewBox".into(), json!([0, 0, "200", 200])),
            ("/look/viewBox".into(), json!([0, 0, 8, 8])),
            ("/look/viewBox".into(), json!([0, 0, 1e300, 200])),
            ("/look/nodes".into(), json!({})),
            ("/look/nodes".into(), json!("x")),
            ("/look/nodes".into(), json!([1])),
            ("/look/nodes".into(), json!([[]])),
            ("/look/nodes".into(), json!([])),
            ("/look".into(), json!({ "viewBox": [0, 0, 200, 200] })),
            ("/look".into(), json!({})),
            ("/look".into(), json!("x")),
            ("/look/extra".into(), json!(1)),
            // A node.
            (format!("{body}/box"), json!([0, 0, 1])),
            (format!("{body}/box"), json!("big")),
            (format!("{body}/box"), json!([0, 0, 1, "1"])),
            (format!("{body}/children"), json!({})),
            (format!("{body}/attrs"), json!([])),
            (format!("{body}/attrs"), json!("x")),
            (format!("{body}/id"), json!(5)),
            (format!("{body}/id"), json!("belly")),
            (format!("{body}/id"), json!("a b")),
            (format!("{body}/id"), json!("")),
            (format!("{body}/id"), json!("toString")),
            (format!("{body}/tag"), json!("svg")),
            (format!("{body}/tag"), json!("use")),
            (format!("{body}/tag"), json!("image")),
            (format!("{body}/tag"), json!("a")),
            (format!("{body}/tag"), json!("tspan")),
            (format!("{body}/tag"), json!("G")),
            (format!("{body}/tag"), json!(5)),
            (format!("{body}/tag"), json!("defs")),
            (format!("{body}/tag"), json!("text")),
            (format!("{body}/tag"), json!("clipPath")),
            (format!("{body}/tag"), json!("circle")),
            ("/look/nodes/0/children/0/tag".into(), json!("clipPath")),
            ("/look/nodes/0/children/0/children/0/tag".into(), json!("circle")),
            ("/look/nodes/0/children/1/children/0/tag".into(), json!("g")),
            (format!("{belly}/text"), json!("words")),
            (format!("{label}/text"), json!("x".repeat(MAX_TEXT_CHARS + 1))),
            (format!("{label}/children"), json!([{ "tag": "circle", "attrs": { "r": 5 } }])),
            // An attribute: only the keys of the table, each with a value of its kind.
            (format!("{belly}/attrs/fill"), json!("red")),
            (format!("{belly}/attrs/fillRef"), json!("")),
            (format!("{belly}/attrs/fillRef"), json!(5)),
            (format!("{belly}/attrs/fillRef"), json!("window")),
            (format!("{belly}/attrs/fillRef"), json!("belly")),
            (format!("{belly}/attrs/strokeRef"), json!("window")),
            (format!("{belly}/attrs/clipPathRef"), json!("sky")),
            (format!("{belly}/attrs/clipPathRef"), json!("pane")),
            (format!("{belly}/attrs/clipPath"), json!("url(#window)")),
            (format!("{belly}/attrs/id"), json!("x")),
            (format!("{belly}/attrs/viewBox"), json!("0 0 1 1")),
            (format!("{belly}/attrs/xmlns"), json!("x")),
            (format!("{belly}/attrs/href"), json!("#sky")),
            (format!("{belly}/attrs/xlink:href"), json!("#sky")),
            (format!("{belly}/attrs/onload"), json!("alert(1)")),
            (format!("{belly}/attrs/class"), json!("x")),
            (format!("{belly}/attrs/data-x"), json!("1")),
            (format!("{belly}/attrs/transform-origin"), json!("1 1")),
            (format!("{belly}/attrs/transformOrigin"), json!("1 1")),
            (format!("{belly}/attrs/stroke-width"), json!(2)),
            (format!("{belly}/attrs/fontSize"), json!(12)),
            (format!("{belly}/attrs/d"), json!("M0 0")),
            (format!("{belly}/attrs/r"), json!(5)),
            (format!("{belly}/attrs/strokeToken"), json!("accent")),
            (format!("{belly}/attrs/stopColorToken"), json!("fg")),
            (format!("{belly}/attrs/cx"), json!(true)),
            (format!("{belly}/attrs/cx"), json!(null)),
            (format!("{belly}/attrs/cx"), json!([1])),
            (format!("{belly}/attrs/cx"), json!({ "a": 1 })),
            (format!("{belly}/attrs/cx"), json!(1e300)),
            (format!("{belly}/attrs/cx"), json!("1e999")),
            (format!("{belly}/attrs/cx"), json!("100' onload='alert(1)")),
            (format!("{belly}/attrs/opacity"), json!("url(#sky)")),
            (format!("{belly}/attrs/transform"), json!("scale(0)")),
            (format!("{belly}/attrs/transform"), json!("rotate(1) javascript:alert(1)")),
            (format!("{face}/attrs/fill"), json!("javascript:alert(1)")),
            (format!("{face}/attrs/fill"), json!("url(http://x/a.svg#b)")),
            (format!("{face}/attrs/fill"), json!("red; background: url(x)")),
            (format!("{face}/attrs/stroke"), json!("var(--accent)")),
            (format!("{face}/attrs/r"), json!(-1)),
            (format!("{face}/attrs/strokeLinecap"), json!("pointy")),
            // What moves it.
            ("/look/animations/nod/tracks/0/prop".into(), json!("points")),
            ("/look/animations/nod/tracks/0/prop".into(), json!("d")),
            ("/look/animations/nod/tracks/0/ease".into(), json!("bounce")),
            ("/look/animations/nod/tracks/0/values".into(), json!([])),
            ("/look/animations/nod/tracks/0/values".into(), json!([0])),
            ("/look/animations/nod/tracks/0/values".into(), json!([0, "a"])),
            ("/look/animations/nod/tracks/0/values".into(), json!([0, null, 0])),
            ("/look/animations/nod/tracks/0/times".into(), json!([0, 1])),
            ("/look/animations/nod/tracks/0/times".into(), json!([0, 0.6, 0.5])),
            ("/look/animations/nod/tracks/0/target".into(), json!([])),
            ("/look/animations/nod/tracks/0/target".into(), json!(["head", "head"])),
            ("/look/animations/nod/tracks/0/target".into(), json!("pane")),
            ("/look/animations/nod/tracks/0/phase".into(), json!(2)),
            ("/look/animations/nod/tracks/0/delay".into(), json!(-1)),
            ("/look/animations/nod/duration".into(), json!("1")),
            ("/look/animations/nod/duration".into(), json!(0)),
            ("/look/animations/nod/loop".into(), json!("yes")),
            ("/look/animations/nod/every".into(), json!([5, 2])),
            ("/look/animations/nod/tracks".into(), json!([])),
            ("/look/animations/nod".into(), json!(null)),
            ("/look/animations/all".into(), rotate("head")),
            ("/look/animations".into(), json!([])),
            ("/look/states/done".into(), json!("nod")),
            ("/look/states/done".into(), json!(["nod", "nod"])),
            ("/look/states/done".into(), json!(["a", "b", "c", "d", "e", "f", "g"])),
            ("/look/states/done".into(), json!(["wink"])),
            ("/look/states/resting".into(), json!(["nod"])),
            ("/look/states".into(), json!([])),
            ("/look/mouth".into(), drive.clone()),
            ("/look/mouth".into(), json!(vec![drive.clone(); MAX_MOUTH_DRIVES + 1])),
            ("/look/mouth/0/prop".into(), json!("opacity")),
            ("/look/mouth/0/open".into(), json!(5)),
            ("/look/mouth/0/target".into(), json!("sky")),
            ("/look/roles/head".into(), json!(5)),
            ("/look/roles/head".into(), json!("sky")),
            ("/look/roles/tail".into(), json!("arm")),
            ("/look/pivots/head".into(), json!([1])),
            ("/look/pivots/head".into(), json!([1, 2, 3])),
            ("/look/pivots/head".into(), json!("x")),
            ("/look/pivots/head".into(), json!([1, 99999])),
            ("/look/pivots/sky".into(), json!([1, 2])),
            ("/look/gaze".into(), json!("3")),
            ("/look/gaze".into(), json!(-1)),
            ("/look/gaze".into(), json!([3])),
            // The name and the voice.
            ("/name".into(), json!(["Pip"])),
            ("/name".into(), json!("x".repeat(MAX_NAME_CHARS + 1))),
            ("/voice".into(), json!("x")),
            ("/voice/lang".into(), json!("xx-")),
            ("/voice/pitch".into(), json!("1")),
            ("/voice/prefer".into(), json!("a")),
        ];
        for (path, value) in refused {
            assert_eq!(tamper(&path, value.clone()), None, "{path} = {value}");
        }
        // What a file may hold besides: the same design in another spelling, which is read into the one form.
        let same: Vec<(String, Value)> = vec![
            (format!("{body}/box"), json!([0, 0, 1, 1])),
            (format!("{belly}/box"), json!(null)),
            (format!("{belly}/attrs/cx"), json!("100")),
            (format!("{belly}/attrs/cx"), json!(100.0004)),
            (format!("{face}/attrs/fill"), json!("#FC0")),
            (format!("{face}/attrs/fill"), json!("rgb(255, 204, 0)")),
            (format!("{label}/text"), json!(" h\u{202E}i\n")),
            ("/name".into(), json!("  Pip\u{200B}\n")),
            ("/voice/extra".into(), json!(1)),
            ("/extra".into(), json!({ "look": "x" })),
            ("/version".into(), json!(99)),
            ("/look/animations/nod/tracks/0/values".into(), json!([0, 10.0004, 0.0])),
            ("/look/animations/nod/loop".into(), json!(false)),
        ];
        for (path, value) in same {
            assert_eq!(tamper(&path, value.clone()), Some(design.clone()), "{path} = {value}");
        }
        // And another design, when the file holds one: it is that design's one form, and a fixed point.
        for (path, value) in [(format!("{face}/attrs/fill"), json!("red")), (format!("{belly}/attrs/fillRef"), json!("sky")), ("/look/gaze".into(), json!(null)), ("/look/mouth".into(), json!([])), ("/name".into(), json!(null))] {
            let other = tamper(&path, value.clone()).unwrap_or_else(|| panic!("{path} = {value}"));
            assert_eq!(check_stored(serde_json::to_value(&other).unwrap()), Some(other), "{path} = {value}");
        }
        // More artwork than a call may send is refused in a file as well.
        let d = format!("M 0 0{}", " l 1 1".repeat(650));
        let nodes: Vec<Value> = (0..9).map(|_| json!({ "tag": "path", "attrs": { "d": d } })).collect();
        let big = json!({ "look": { "viewBox": [0, 0, 200, 200], "nodes": nodes, "pivots": {}, "roles": {}, "animations": {}, "states": {}, "mouth": [], "gaze": null } });
        assert!(json_len(&big) < MAX_LOOK_BYTES && json_len(&big) > MAX_SVG_BYTES);
        assert_eq!(check_stored(big), None);
    }

    #[test]
    fn artwork_keeps_to_its_size_in_the_form_it_is_stored_and_given_back_in() {
        // Dense path data grows by half once every number stands apart: 32 KiB as written, 47 KiB as stored.
        let path = format!("<path d='M0,0{}'/>", "l1,1".repeat(660));
        let art = svg(&path.repeat(12));
        assert!(art.len() <= MAX_SVG_BYTES, "{}", art.len());
        assert_eq!(refused(None, json!({ "svg": art })), "svg is 46.6 KiB in the form it is stored and given back in: at most 32 KiB, so shorter paths or fewer shapes");
        // What is accepted is always given back in a form that can be sent again.
        let fits = accepted(None, json!({ "svg": svg(&path.repeat(8)) }));
        let look = fits.design.look.as_ref().unwrap();
        assert!(to_svg(look).len() <= MAX_SVG_BYTES && to_svg(look).len() > MAX_SVG_BYTES - 1024, "{}", to_svg(look).len());
        assert_eq!(round_trip(look), *look);
        assert_eq!(check_stored(serde_json::to_value(&fits.design).unwrap()).as_ref(), Some(&fits.design));
        // A merge does not change the artwork, so it cannot outgrow the limit.
        assert_eq!(accepted(Some(&fits.design), json!({ "gaze": 0 })).usage.svg_bytes, to_svg(look).len());
    }

    #[test]
    fn the_sizes_of_a_call_are_checked_to_the_byte() {
        let frame = "<svg><!----><circle id='c' cx='9' cy='9' r='5'/></svg>";
        let art = |bytes: usize| frame.replace("<!---->", &format!("<!--{}-->", "x".repeat(bytes - frame.len())));
        assert_eq!(art(MAX_SVG_BYTES).len(), MAX_SVG_BYTES);
        accepted(None, json!({ "svg": art(MAX_SVG_BYTES) }));
        assert_eq!(refused(None, json!({ "svg": art(MAX_SVG_BYTES + 1) })), "svg is 32.0 KiB: at most 32 KiB");
        // Bytes, not characters.
        let wide = frame.replace("<!---->", &format!("<!--{}-->", "é".repeat(MAX_SVG_BYTES / 2)));
        assert!(wide.chars().count() < MAX_SVG_BYTES);
        assert_eq!(refused(None, json!({ "svg": wide })), "svg is 32.1 KiB: at most 32 KiB");
        // The arguments, as the JSON they are.
        let empty = json_len(&json!({ "name": "Pip", "junk": "" }));
        let call = |bytes: usize| json!({ "name": "Pip", "junk": "x".repeat(bytes - empty) });
        assert_eq!(json_len(&call(MAX_DESIGN_BYTES)), MAX_DESIGN_BYTES);
        assert_eq!(refused(None, call(MAX_DESIGN_BYTES)), "\"junk\" is not a key here: keys are name, svg, pivots, parents, roles, animations, states, mouth, gaze, drives, voice, keep");
        assert_eq!(refused(None, call(MAX_DESIGN_BYTES + 1)), "the design is 256.0 KiB of JSON: at most 256 KiB");
        // What JSON escapes counts as it is written.
        assert_eq!(refused(None, json!({ "name": "\u{1}".repeat(MAX_DESIGN_BYTES / 6) })), "the design is 256.0 KiB of JSON: at most 256 KiB");
        assert_eq!(json_len(&json!({ "a": [1, 2.5, null, true, "é\n"] })), serde_json::to_string(&json!({ "a": [1, 2.5, null, true, "é\n"] })).unwrap().len());
    }

    #[test]
    fn cuts_fall_between_characters_whatever_the_input() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "ab…");
        assert_eq!(cut("😀😀😀😀", 3), "😀😀…");
        assert_eq!(cut("e\u{0301}e\u{0301}e\u{0301}", 4), "e\u{0301}e…");
        assert_eq!(cut("", 5), "");
        assert_eq!(q(&unmasked, &"😀".repeat(100)), format!("\"{}…\"", "😀".repeat(MAX_QUOTE_CHARS - 1)));
        assert_eq!(q(&unmasked, "a\"b\\c\n\u{202E}\u{0}\u{301}"), "\"a\\\"b\\\\c\\n\\u{202e}\\0\\u{301}\"");
        // The joined message: cut at 3000 bytes or just below, never inside a character.
        for width in ["a", "é", "€", "😀"] {
            for lead in 0..4 {
                let one = format!("{}{}", "x".repeat(lead), width.repeat(MAX_MESSAGE_BYTES));
                let joined = message(&[one.clone(), one]);
                assert!(joined.len() <= MAX_MESSAGE_BYTES && joined.len() > MAX_MESSAGE_BYTES - width.len(), "{width} {lead}: {}", joined.len());
                assert!(joined.ends_with(width));
            }
        }
        // Through a refusal: thirteen lines of names of four bytes a character.
        let things: String = (0..20).map(|i| format!("<x{i}{}/>", "😀".repeat(60))).collect();
        let problems = apply(None, &json!({ "svg": svg(&things) })).unwrap_err();
        assert_eq!(problems.len(), MAX_PROBLEM_KINDS + 1);
        assert!(problems.iter().all(|p| p.chars().count() <= MAX_PROBLEM_CHARS));
        assert!(problems.iter().map(String::len).sum::<usize>() > MAX_MESSAGE_BYTES);
        let joined = message(&problems);
        assert!(joined.len() <= MAX_MESSAGE_BYTES && joined.len() > MAX_MESSAGE_BYTES - 4, "{}", joined.len());
        // A place holds what was written only as a quoted, cut piece.
        let key = "\u{202E}'><script>".repeat(40);
        for request in [figure(json!({ "pivots": { key.as_str(): [1, 2] } })), figure(json!({ "animations": { key.as_str(): rotate("arm") } })), figure(json!({ "states": { key.as_str(): [] } })), figure(json!({ "roles": { key.as_str(): "arm" } })), figure(json!({ "states": { "idle": [key.as_str()] } })), figure(json!({ "roles": { "head": key.as_str() } })), tracks(json!([{ "target": key.as_str(), "prop": key.as_str(), "values": [key.as_str(), key.as_str()] }])), figure(json!({ "mouth": { "target": key.as_str(), "prop": key.as_str(), "open": key.as_str(), key.as_str(): 1 } })), figure(json!({ "voice": { "lang": key.as_str(), key.as_str(): 1 } })), json!({ key.as_str(): 1 })] {
            let why = refused(None, request);
            assert!(!why.contains('\u{202E}') && !why.contains("<script>".repeat(2).as_str()), "{why}");
        }
    }

    #[test]
    fn hostile_sizes_cost_little() {
        fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
            let started = Instant::now();
            let out = f();
            let took = started.elapsed();
            assert!(took < Duration::from_secs(2), "{what} took {took:?}");
            out
        }
        // 300 elements with 32 attributes each do not fit the artwork: refused for the size, nothing is read.
        let full = svg(&format!("<g{}/>", (0..MAX_ATTRS).map(|i| format!(" a{i:02}=''")).collect::<String>()).repeat(MAX_ELEMENTS));
        has(&timed("300 x 32", || refused(None, json!({ "svg": full }))), "KiB: at most 32 KiB");
        // As many as fit, each with 32 attributes that are read: 17 of the table, 15 that are dropped.
        let rich = format!(
            "<circle id='ID' cx='50' cy='50' r='5' fill='red' stroke='hsl(200 50% 50%)' stroke-width='1' stroke-linecap='round' stroke-linejoin='round' stroke-miterlimit='4' fill-opacity='1' stroke-opacity='1' opacity='1' fill-rule='nonzero' clip-rule='nonzero' clip-path='none' transform='rotate(1)'{}/>",
            (0..15).map(|i| format!(" data-{i}=''")).collect::<String>()
        );
        let count = (MAX_SVG_BYTES - 64) / (rich.len() + 2);
        let art = svg(&(0..count).map(|i| rich.replace("ID", &format!("c{i}"))).collect::<String>());
        assert!(art.len() <= MAX_SVG_BYTES && count > 60, "{count} elements, {} bytes", art.len());
        let applied = timed("32 attributes each", || accepted(None, json!({ "svg": art })));
        assert_eq!((applied.parts.len(), applied.warnings.as_slice()), (count, [format!("Not drawn: data-* ({})", count * 15)].as_slice()));
        // 32 KiB of groups: nested, side by side, inside what is dropped.
        has(&timed("nested", || undrawn(&"<g>".repeat(10_000))), "svg: elements nested deeper than 10");
        has(&timed("side by side", || undrawn(&"<g/>".repeat(8_000))), "svg: more than 300 elements");
        has(&timed("in tens", || undrawn(&format!("{}{}", "<g>".repeat(10), "</g>".repeat(10)).repeat(400))), "svg: more than 300 elements");
        has(&timed("nested in a dropped element", || undrawn(&format!("<metadata>{}", "<x>".repeat(10_000)))), "svg: elements nested deeper than 10");
        // 16 tracks of 16 path keyframes: every one is read, and the look's limit of them refuses.
        let d = format!("M 0 0{}", " L 100 100".repeat(85));
        let heavy = tracks(json!(vec![json!({ "target": "mouth", "prop": "d", "values": vec![d.clone(); MAX_KEYFRAMES] }); MAX_TRACKS]));
        assert!(json_len(&heavy) < MAX_DESIGN_BYTES && json_len(&heavy) > MAX_DESIGN_BYTES * 4 / 5);
        has(&timed("16 x 16 long d keyframes", || refused(None, heavy)), "218880 characters of d keyframes in the look: at most 6000");
        // As many d keyframes as a look may hold, in as many tracks.
        let small = tracks(json!(vec![json!({ "target": "mouth", "prop": "d", "values": vec!["M84 96 Q100 104 16 96"; MAX_KEYFRAMES] }); MAX_TRACKS]));
        let applied = timed("16 x 16 short d keyframes", || accepted(None, small));
        assert_eq!(d_keyframe_chars(applied.design.look.as_ref().unwrap()), 16 * 16 * 23);
        // Tens of thousands of entries in each keyed field.
        for key in ["pivots", "parents", "roles", "states", "animations"] {
            let many: Map<String, Value> = (0..15_000).map(|i| (format!("k{i}"), json!([1, 2]))).collect();
            let mut request = json!({ "svg": FIGURE });
            request[key] = Value::Object(many);
            assert!(json_len(&request) < MAX_DESIGN_BYTES);
            let why = timed(key, || refused(None, request));
            assert!(why.lines().count() <= MAX_PROBLEM_KINDS + 1, "{why}");
        }
        // As many drives, and a chain of parents as long as the look may hold, in a circle.
        let mut request = json!({ "svg": FIGURE });
        request["drives"] = json!(vec![json!({ "input": "lookX", "target": "arm", "prop": "rotate", "values": [0, 1] }); 3_000]);
        assert!(json_len(&request) < MAX_DESIGN_BYTES);
        has(&timed("3000 drives", || refused(None, request)), "drives is a list of 3000");
        let circles: String = (0..MAX_PARENTS + 1).map(|i| format!("<circle id='c{i}' cx='100' cy='100' r='5'/>")).collect();
        let ring: Map<String, Value> = (0..MAX_PARENTS).map(|i| (format!("c{i}"), json!(format!("c{}", (i + 1) % MAX_PARENTS)))).collect();
        has(&timed("a ring of parents", || refused(None, json!({ "svg": svg(&circles), "parents": ring }))), "go round in a circle");
        // A text made of references, a document made of comments, a value made of references.
        has(&timed("references", || undrawn(&format!("<text>{}</text>", "&amp;".repeat(6000)))), "svg: a text has 6000 characters: at most 40");
        timed("comments", || drawn(&format!("{}<circle id='c' cx='9' cy='9' r='5'/>", "<!---->".repeat(4000))));
        has(&timed("a value of references", || undrawn(&format!("<path d='M0 0{}'/>", " &#49;".repeat(4000)))), "8004 characters of path data: at most 4000");
        timed("white space", || drawn(&format!("<g>{}<circle id='c' cx='9' cy='9' r='5'/></g>", " \n\t".repeat(10_000))));
        // A stored look near its limit, read back.
        let values: Vec<Value> = (0..MAX_KEYFRAMES).map(|i| json!(0.123 + i as f64 / 100.0)).collect();
        let track = json!({ "target": ["arm", "cane", "label", "face", "pupil", "mouth", "belly", "head", "eyes", "body"], "prop": "opacity", "values": values });
        let animations: Map<String, Value> = (0..30).map(|i| (format!("heavy{i}"), json!({ "duration": 30, "tracks": vec![track.clone(); MAX_TRACKS] }))).collect();
        let big = accepted(None, figure(json!({ "animations": animations }))).design;
        assert!(json_len(&big.look) > MAX_LOOK_BYTES * 3 / 4, "{}", json_len(&big.look));
        let stored = serde_json::to_value(&big).unwrap();
        assert_eq!(timed("a stored look", || check_stored(stored)), Some(big));
    }

    // ------------------------------------------------------------ the adversary: designs made from valid pieces

    /// One design put together from valid pieces by a fixed sequence: artwork with every kind of
    /// element, nested transforms and references, and what moves it.
    fn generated(pick: &mut dyn FnMut(usize) -> usize) -> Value {
        const NUMBERS: [&str; 10] = ["0", "5", "10.5", "50", "100", "150", "199.999", "-20", "250", "33.3333"];
        const SIZES: [&str; 6] = ["0", "0.5", "4", "12.25", "40", "120"];
        const PAINTS: [&str; 10] = ["red", "#abc", "#11223344", "rgb(10 20 30)", "hsl(200 50% 50%)", "none", "currentColor", "transparent", "url(#lg)", "url(#rg)"];
        const TRANSFORMS: [&str; 8] = ["translate(10 20)", "rotate(30)", "rotate(-45 100 100)", "scale(1.5 0.5)", "scale(-1 1)", "matrix(0.5 0.2 -0.2 0.5 10 10)", "skewX(20) translate(5)", "translate(100 100) rotate(90) scale(2) translate(-100 -100)"];
        const EASES: [&str; 9] = ["linear", "in", "out", "inOut", "outBack", "inBack", "inOutBack", "outElastic", "outBounce"];
        const PATHS: [[&str; 2]; 6] = [
            ["M10 10 L50 30 Q80 0 60 60 Z", "M12 8 L55 35 Q70 10 60 70 Z"],
            ["M40 100 A60 60 0 1 1 160 100 A60 60 0 1 1 40 100 Z", "M40 100 A60 50 0 1 1 160 100 A60 50 0 1 1 40 100 Z"],
            ["m20 20 h20 v30 l-5 -5 a5 9 30 0 1 8 8", "m25 20 h10 v30 l-5 5 a5 9 30 0 1 8 2"],
            ["M0 20 Q10 30 20 20 T40 20 T60 20", "M0 25 Q10 10 20 25 T40 25 T60 25"],
            ["M0 50 C0 60 10 60 10 50 S20 40 20 50 s10 10 20 0", "M0 50 C0 70 10 70 10 50 S20 30 20 50 s10 20 20 0"],
            ["M1-2.5.5-3L4 5 6 7H9V1z", "M1-2.5.5-3L4 9 6 2H5V1z"],
        ];
        fn one(pick: &mut dyn FnMut(usize) -> usize, from: &[&'static str]) -> &'static str {
            from[pick(from.len())]
        }
        /// Values of a prop: within its range, and null only where it means the element's own value.
        fn values(pick: &mut dyn FnMut(usize) -> usize, prop: Prop, path: usize, count: usize) -> Vec<Value> {
            let mut values: Vec<Value> = (0..count)
                .map(|i| match prop {
                    Prop::D => json!(PATHS[path][i % 2]),
                    Prop::Fill | Prop::Stroke => json!(["#ff0000", "gold", "rgb(0 0 255 / 50%)", "transparent"][pick(4)]),
                    p if p.is_opacity() => json!([0.0, 0.3, 1.0][pick(3)]),
                    Prop::Rotate => json!([-90.0, 0.0, 12.5, 360.0][pick(4)]),
                    p if p.is_skew() => json!([-30.0, 0.0, 8.5, 80.0][pick(4)]),
                    p if p.is_scale() => json!([-1.0, 0.0, 0.9, 1.0, 1.25][pick(5)]),
                    p if p.is_length() => json!([0.0, 2.0, 7.5, 30.0][pick(4)]),
                    _ => json!([-12.0, 0.0, 4.0, 60.0][pick(4)]),
                })
                .collect();
            // The inherited props have no value of their own on an element that does not set them.
            if !prop.is_relative() && !matches!(prop, Prop::FillOpacity | Prop::StrokeOpacity | Prop::StrokeDashoffset) && pick(4) == 0 {
                values[0] = Value::Null;
            }
            values
        }
        /// An ease; `steady`: one a colour or an opacity may have (no jump, no swing).
        fn ease(pick: &mut dyn FnMut(usize) -> usize, steady: bool) -> Value {
            if steady {
                return if pick(4) == 0 { json!([[0.25, 0.1, 0.25, 1.0], [0.0, 0.0, 1.0, 1.0]][pick(2)]) } else { json!(["linear", "in", "out", "inOut", "outBack", "inBack"][pick(6)]) };
            }
            if pick(4) == 0 { json!([[0.25, 0.1, 0.25, 1.0], [0.68, -0.6, 0.32, 1.6], [0.0, 0.0, 1.0, 1.0]][pick(3)]) } else { json!(EASES[pick(EASES.len())]) }
        }
        // Each part: its id, its kind, the path data it takes, the ids of the groups it stands in.
        let mut parts: Vec<(String, Tag, usize, Vec<String>)> = Vec::new();
        let mut art = String::from(
            "<svg viewBox='0 0 200 200'><defs><linearGradient id='lg' x1='0' y1='0' x2='1' y2='1'><stop offset='0' stop-color='gold'/><stop offset='100%' stop-color='#00f' stop-opacity='0.5'/></linearGradient><radialGradient id='rg' gradientUnits='userSpaceOnUse' cx='50%' cy='40' r='120'><stop offset='0.5' stop-color='white'/></radialGradient><clipPath id='cp'><circle cx='100' cy='100' r='90'/></clipPath>\
             <mask id='mk' maskUnits='userSpaceOnUse' x='0' y='0' width='100%' height='100%'><rect x='0' y='0' width='200' height='200' fill='url(#lg)'/></mask>\
             <filter id='fx' x='-10%' y='-10%' width='120%' height='120%'><feGaussianBlur in='SourceAlpha' stdDeviation='2 1' result='b'/><feOffset in='b' dx='1' dy='2' result='o'/><feMerge><feMergeNode in='o'/><feMergeNode in='SourceGraphic'/></feMerge></filter>\
             <g id='stamp'><circle cx='10' cy='10' r='4'/><path d='M0 0 L5 5'/></g></defs>",
        );
        // Elements in document order; a group is closed when its count of children runs out.
        let mut open: Vec<(usize, Option<String>)> = Vec::new();
        let total = 3 + pick(14);
        for n in 0..total {
            let tag = [Tag::G, Tag::Path, Tag::Circle, Tag::Ellipse, Tag::Rect, Tag::Line, Tag::Polyline, Tag::Polygon, Tag::Text, Tag::Use][if open.len() < 4 { pick(10) } else { 1 + pick(9) }];
            let _ = write!(art, "<{}", tag.name());
            let mut path = 0;
            let mut named = None;
            if pick(3) > 0 {
                let id = format!("e{n}");
                let _ = write!(art, " id='{id}'");
                path = pick(PATHS.len());
                parts.push((id.clone(), tag, path, open.iter().filter_map(|(_, id)| id.clone()).collect()));
                named = Some(id);
            }
            match tag {
                Tag::Path => {
                    let _ = write!(art, " d='{}'", PATHS[path][0]);
                }
                Tag::Circle => {
                    let _ = write!(art, " cx='{}' cy='{}' r='{}'", one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &SIZES));
                }
                Tag::Ellipse => {
                    let _ = write!(art, " cx='{}' cy='{}' rx='{}' ry='{}'", one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &SIZES), one(pick, &SIZES));
                }
                Tag::Rect => {
                    let _ = write!(art, " x='{}' y='{}' width='{}' height='{}' rx='{}'", one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &SIZES), one(pick, &SIZES), one(pick, &SIZES));
                }
                Tag::Line => {
                    let _ = write!(art, " x1='{}' y1='{}' x2='{}' y2='{}'", one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &NUMBERS));
                }
                Tag::Polyline | Tag::Polygon => {
                    let _ = write!(art, " points='{},{} {} {},{} {}'", one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &NUMBERS));
                }
                Tag::Text => {
                    let _ = write!(art, " x='{}' y='{}' font-size='{}' text-anchor='{}'", one(pick, &NUMBERS), one(pick, &NUMBERS), one(pick, &["8", "12.5", "30"]), one(pick, &["start", "middle", "end"]));
                }
                Tag::Use => {
                    let _ = write!(art, " {}='#stamp' x='{}' y='{}'", one(pick, &["href", "xlink:href"]), one(pick, &NUMBERS), one(pick, &NUMBERS));
                }
                _ => {}
            }
            if pick(2) == 0 {
                let _ = write!(art, " fill='{}'", one(pick, &PAINTS));
            }
            if pick(2) == 0 {
                let _ = write!(art, " stroke='{}' stroke-width='{}'", one(pick, &PAINTS), one(pick, &SIZES));
            }
            if pick(4) == 0 {
                let _ = write!(art, " stroke-dasharray='{}' stroke-dashoffset='{}'", one(pick, &["4 2", "1,3,5", "none", "0.5 0.5 12"]), one(pick, &["0", "-3", "2.5"]));
            }
            if pick(3) == 0 {
                let _ = write!(art, " opacity='{}'", one(pick, &["0", "0.25", "50%", "1"]));
            }
            if pick(3) == 0 {
                let _ = write!(art, " transform='{}'", one(pick, &TRANSFORMS));
            }
            if pick(6) == 0 {
                art.push_str(" clip-path='url(#cp)'");
            }
            if pick(7) == 0 {
                art.push_str(" mask='url(#mk)'");
            }
            if pick(7) == 0 {
                art.push_str(" filter='url(#fx)'");
            }
            if pick(8) == 0 {
                art.push_str(" style='fill: teal; stroke-linecap: round; mix-blend-mode: multiply'");
            }
            match tag {
                Tag::G => {
                    art.push('>');
                    open.push((1 + pick(4), named));
                    continue;
                }
                Tag::Text => {
                    let _ = write!(art, ">{}</text>", one(pick, &["hi", "a &amp; b", "Zz", "é<tspan>!</tspan>"]));
                }
                _ => art.push_str("/>"),
            }
            while let Some((left, _)) = open.last_mut() {
                *left -= 1;
                if *left > 0 {
                    break;
                }
                open.pop();
                art.push_str("</g>");
            }
        }
        // A group must not stay empty of everything, or the artwork may hold nothing to draw.
        art.push_str("<circle cx='100' cy='100' r='3'/>");
        for _ in open {
            art.push_str("</g>");
        }
        art.push_str("</svg>");
        let mut request = json!({ "svg": art });
        if parts.is_empty() {
            return request;
        }
        let part = |pick: &mut dyn FnMut(usize) -> usize| parts[pick(parts.len())].clone();
        let mut pivots = Map::new();
        for _ in 0..pick(4) {
            pivots.insert(part(pick).0, json!([NUMBERS[pick(NUMBERS.len())].parse::<f64>().unwrap(), NUMBERS[pick(NUMBERS.len())].parse::<f64>().unwrap()]));
        }
        let mut roles = Map::new();
        for role in Role::ALL {
            if pick(2) == 0 {
                roles.insert(role.name().to_string(), json!(part(pick).0));
            }
        }
        let mut animations = Map::new();
        for a in 0..pick(5) {
            let looping = pick(3) == 0;
            let duration = [0.5, 1.0, 2.4, 6.0][pick(4)];
            let mut list = Vec::new();
            for _ in 0..1 + pick(4) {
                let (id, tag, path, _) = part(pick);
                let fitting: Vec<Prop> = Prop::ALL.into_iter().filter(|p| p.fits(tag)).collect();
                let prop = fitting[pick(fitting.len())];
                let count = 2 + pick(3);
                let mut values = values(pick, prop, path, count);
                if looping && prop.is_paint() {
                    // A loop's paint ends as it begins.
                    values[count - 1] = values[0].clone();
                }
                let mut track = json!({ "target": id, "prop": prop.name(), "values": values });
                if pick(4) == 0 {
                    track["target"] = json!([track["target"], part(pick).0]);
                }
                if pick(4) == 0 {
                    track["ease"] = ease(pick, prop.is_paint());
                }
                if pick(5) == 0 {
                    track["times"] = json!(match count {
                        2 => vec![0.0, 1.0],
                        3 => vec![0.0, 0.4, 1.0],
                        _ => vec![0.0, 0.35, 0.65, 1.0],
                    });
                }
                if pick(6) == 0 {
                    track[if looping { "phase" } else { "delay" }] = json!(0.25);
                }
                list.push(track);
            }
            let mut animation = json!({ "duration": duration, "tracks": list });
            if looping {
                animation["loop"] = json!(true);
            } else if pick(3) == 0 {
                animation["every"] = json!([2, 5.5]);
            }
            animations.insert(format!("a{a}"), animation);
        }
        let mut states = Map::new();
        for state in LookState::ALL {
            if pick(4) == 0 {
                let mut names: Vec<String> = animations.keys().filter(|_| pick(2) == 0).cloned().collect();
                names.extend(["breathe", "blink", "wave"].into_iter().filter(|_| pick(3) == 0).map(str::to_string));
                states.insert(state.name().to_string(), json!(names));
            }
        }
        request["pivots"] = Value::Object(pivots);
        request["roles"] = Value::Object(roles);
        request["animations"] = Value::Object(animations);
        request["states"] = Value::Object(states);
        if pick(3) == 0 {
            let (id, tag, path, _) = part(pick);
            request["mouth"] = match tag {
                Tag::Path if pick(2) == 0 => json!({ "target": id, "open": PATHS[path][1] }),
                Tag::Circle => json!([{ "target": id, "prop": "r", "open": 9 }, { "target": id, "prop": "scaleY", "open": 1.4, "closed": 0.9 }]),
                _ => json!({ "target": id, "prop": "scaleY", "open": 1.6 }),
            };
        }
        if pick(3) == 0 {
            request["gaze"] = json!([0.0, 2.5, 10.0][pick(3)]);
        }
        // A part moves with one drawn earlier that neither holds it nor stands inside it: no circle.
        if pick(3) == 0 {
            let mut parents = Map::new();
            for _ in 0..1 + pick(2) {
                let (i, j) = (pick(parts.len()), pick(parts.len()));
                let ((child, _, _, above), (parent, _, _, over)) = (&parts[i.max(j)], &parts[i.min(j)]);
                if i != j && !above.contains(parent) && !over.contains(child) {
                    parents.insert(child.clone(), json!(parent));
                }
            }
            request["parents"] = Value::Object(parents);
        }
        if pick(3) == 0 {
            let mut drives = Vec::new();
            for _ in 0..1 + pick(2) {
                let (id, tag, path, _) = part(pick);
                let fitting: Vec<Prop> = Prop::ALL.into_iter().filter(|p| p.fits(tag)).collect();
                let prop = fitting[pick(fitting.len())];
                // Paint follows only the inputs that change slowly.
                let inputs: &[&str] = if prop.is_paint() { &["pace", "turn", "held"] } else { &["lookX", "lookY", "mouth", "pace", "turn", "held"] };
                let input = inputs[pick(inputs.len())];
                // Paint values stand far apart along their input: two, or three along turn.
                let count = if prop.is_paint() { 2 + usize::from(input == "turn" && pick(2) == 0) } else { 2 + pick(3) };
                let mut drive = json!({ "input": input, "target": id, "prop": prop.name(), "values": values(pick, prop, path, count) });
                if pick(3) == 0 {
                    drive["at"] = json!(match count {
                        2 => vec![0.0, 1.0],
                        3 if prop.is_paint() => vec![0.0, 0.4, 1.0],
                        3 => vec![0.0, 0.25, 1.0],
                        _ => vec![0.0, 0.3, 0.7, 1.0],
                    });
                }
                if pick(3) == 0 {
                    drive["ease"] = ease(pick, prop.is_paint());
                }
                drives.push(drive);
            }
            request["drives"] = json!(drives);
        }
        request
    }

    #[test]
    fn designs_made_from_valid_pieces_are_accepted_and_hold_their_promises() {
        let mut pick = sequence(0x2545_F491_4F6C_DD1D);
        let mut accepted_count = 0;
        let rounds = 400;
        for round in 0..rounds {
            let request = generated(&mut pick);
            let ok = holds_its_promises(&request);
            // The pieces are valid: what is refused is refused for how they met, and says so.
            if !ok && round < 3 {
                let _ = refused(None, request.clone());
            }
            accepted_count += usize::from(ok);
        }
        assert!(accepted_count > rounds / 2, "{accepted_count} of {rounds} accepted");
    }
    #[test]
    fn the_stored_look_keeps_to_its_size_to_the_byte() {
        // Tracks of one size; the name of an animation is a byte a character in the stored look.
        let values: Vec<Value> = (0..MAX_KEYFRAMES).map(|i| json!(0.123 + i as f64 / 100.0)).collect();
        let times: Vec<Value> = (0..MAX_KEYFRAMES).map(|i| json!(i as f64 / (MAX_KEYFRAMES - 1) as f64)).collect();
        let track = json!({ "target": ["arm", "cane", "label", "face", "pupil", "mouth", "belly", "head", "eyes", "body"], "prop": "opacity", "values": values, "times": times });
        let design = |full: usize, tail: usize, pad: usize| {
            let mut left = pad;
            let mut animations = Map::new();
            for i in 0..full {
                let more = left.min(MAX_ID_CHARS - 3);
                left -= more;
                animations.insert(format!("a{i:02}{}", "x".repeat(more)), json!({ "duration": 30, "tracks": vec![track.clone(); MAX_TRACKS] }));
            }
            assert_eq!(left, 0, "more padding than the names take");
            if tail > 0 {
                animations.insert("tail".into(), json!({ "duration": 30, "tracks": vec![track.clone(); tail] }));
            }
            figure(json!({ "animations": animations }))
        };
        let size = |full: usize, tail: usize, pad: usize| accepted(None, design(full, tail, pad)).usage.look_bytes;
        // Sizes add up: so many whole animations, so many more tracks, the rest in names.
        let (one, two) = (size(1, 0, 0), size(2, 0, 0));
        let full = 1 + (MAX_LOOK_BYTES - one) / (two - one);
        let (first, second) = (size(1, 1, 0), size(1, 2, 0));
        let (track_bytes, tail_bytes) = (second - first, first - one);
        let rest = MAX_LOOK_BYTES - size(full, 0, 0);
        let tail = if rest >= tail_bytes { 1 + (rest - tail_bytes) / track_bytes } else { 0 };
        let pad = MAX_LOOK_BYTES - size(full, tail, 0);
        assert!(full < MAX_ANIMATIONS && tail <= MAX_TRACKS && pad < tail_bytes, "{full} animations, {tail} tracks, {pad} bytes");
        let exact = accepted(None, design(full, tail, pad));
        assert_eq!(exact.usage.look_bytes, MAX_LOOK_BYTES);
        assert_eq!(json_len(exact.design.look.as_ref().unwrap()), MAX_LOOK_BYTES);
        assert_eq!(refused(None, design(full, tail, pad + 1)), "the look is 128.0 KiB once stored: at most 128 KiB, so fewer elements, shorter paths or fewer keyframes");
        // At the limit it is stored and read back; one byte more through a merge is refused, less is fine.
        assert_eq!(check_stored(serde_json::to_value(&exact.design).unwrap()).as_ref(), Some(&exact.design));
        has(&refused(Some(&exact.design), json!({ "gaze": 0.125 })), "the look is 128.0 KiB once stored: at most 128 KiB");
        has(&refused(Some(&exact.design), json!({ "pivots": { "arm": [1, 2] } })), "the look is 128.0 KiB once stored");
        assert_eq!(accepted(Some(&exact.design), json!({ "gaze": 0 })).usage.look_bytes, MAX_LOOK_BYTES - 3);
        assert!(accepted(Some(&exact.design), json!({ "animations": { "tail": null }, "pivots": { "arm": [1, 2] } })).usage.look_bytes < MAX_LOOK_BYTES);
        // The name and the voice are not the look: they do not count towards it.
        assert_eq!(accepted(Some(&exact.design), json!({ "name": "Pip", "voice": { "lang": "en-GB" } })).usage.look_bytes, MAX_LOOK_BYTES);
    }

    #[test]
    fn a_chain_of_calls_keeps_every_promise() {
        let mut pick = sequence(0xD1B5_4A32_D192_ED03);
        let (mut calls, mut taken) = (0, 0);
        for _ in 0..60 {
            let Ok(first) = apply(None, &generated(&mut pick)) else { continue };
            let mut current = first;
            for _ in 0..8 {
                let look = current.design.look.clone().expect("a look");
                let part = |pick: &mut dyn FnMut(usize) -> usize| current.parts.get(pick(current.parts.len().max(1))).map_or("nope".to_string(), |p| p.id.clone());
                let animation = look.animations.keys().nth(pick(look.animations.len().max(1))).cloned().unwrap_or_else(|| "none-such".to_string());
                let other = generated(&mut pick);
                let call = match pick(12) {
                    0 => json!({ "animations": { animation: null }, "states": null }),
                    1 => json!({ "animations": { animation: null } }),
                    2 => json!({ "pivots": { part(&mut pick): [pick(200), pick(200)] } }),
                    3 => json!({ "roles": { Role::ALL[pick(6)].name(): part(&mut pick) } }),
                    4 => json!({ "roles": { Role::ALL[pick(6)].name(): null }, "pivots": { part(&mut pick): null } }),
                    5 => json!({ "svg": other["svg"], "keep": true }),
                    6 => json!({ "svg": to_svg(&look), "keep": true }),
                    7 => json!({ "gaze": ([json!(null), json!(0), json!(7.5)][pick(3)]).clone(), "mouth": null }),
                    8 => json!({ "mouth": { "target": part(&mut pick), "prop": "scaleY", "open": 1.5 }, "name": "Pip\u{202E}" }),
                    9 => json!({ "animations": other["animations"], "states": other["states"] }),
                    10 => json!({ "states": { LookState::ALL[pick(9)].name(): [animation, "blink"] }, "voice": { "lang": "de", "rate": 1.1 } }),
                    _ => other,
                };
                calls += 1;
                match apply(Some(&current.design), &call) {
                    Ok(applied) => {
                        taken += 1;
                        let look = applied.design.look.as_ref().expect("a look stays");
                        assert!(json_len(look) <= MAX_LOOK_BYTES && to_svg(look).len() <= MAX_SVG_BYTES && d_keyframe_chars(look) <= MAX_D_KEYFRAMES_TOTAL && look.animations.len() <= MAX_ANIMATIONS);
                        assert!(numbers(look).iter().all(|n| n.abs() <= MAX_SPAN));
                        assert!(applied.warnings.len() <= MAX_WARNINGS && applied.warnings.iter().all(|w| w.chars().count() <= MAX_PROBLEM_CHARS));
                        // However it came about, the look is one an agent could have sent whole, and a file could hold.
                        assert_eq!(&round_trip(look), look, "{call}");
                        assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()).as_ref(), Some(&applied.design), "{call}");
                        // The same artwork with keep keeps all of it.
                        if call.get("svg") == Some(&json!(to_svg(&current.design.look.clone().unwrap()))) {
                            assert_eq!((&applied.design, applied.cleared.len()), (&current.design, 0), "{call}");
                        }
                        current = applied;
                    }
                    Err(problems) => {
                        assert!(!problems.is_empty() && problems.len() <= MAX_PROBLEM_KINDS + 1);
                        assert!(problems.iter().all(|p| p.chars().count() <= MAX_PROBLEM_CHARS && !p.chars().any(|c| c.is_control())), "{problems:?}");
                    }
                }
            }
        }
        assert!(calls > 200 && taken > calls / 4, "{taken} of {calls} calls accepted");
    }
    #[test]
    fn the_value_parsers_take_any_text_and_what_they_write_reads_as_itself() {
        let pieces = [
            "M", "m", "L", "l", "H", "h", "V", "v", "C", "c", "S", "s", "Q", "q", "T", "t", "A", "a", "Z", "z", "0", "1", "2", "5", "9", "-", "+", ".", "e", "E", " ", " ", ",", "\t", "\n", "(", ")", "#", "%", "px",
            "translate", "rotate", "scale", "skewX", "skewY", "matrix", "url", "rgb", "hsl", "rgba", "red", "none", "transparent", "f", "abc", "é", "\u{202E}", "\u{0}", "/", "*", "!", "\\", ";", ":", "'", "\"", "deg",
            "1e999", "0.0004", "-0", "800", "1000", "4096", "-801", "0 0 1", "1 1 ", ".5", "360",
        ];
        // What each kind of value is mostly written with; now and then a piece of anything.
        let kinds: [&[&str]; 4] = [
            &["M0 0", "m5-5", " L10 20", "l1-2", " H5", "h-5", " V7", "v.5", "Q1 2 3 4", " T5 6", "t1 1", " C1 2 3 4 5 6", "c0,0,1,1,2,2", " S1 2 3 4", " A5 5 0 0 1 9 9", "a5 5 30 1 0-9 9", " a2 8 -40 0115 5", " Z", "z", " 1 2", " 3 4 5 6", " L400 400", "l300 300", " "],
            &["scale(1)", "translate(", "rotate(", "scale(", "skewX(", "matrix(1 0 0 1 0 0)", "1", "2.5", "-3", "0.5", " ", ",", ")", ") ", "10 20)", "45)", "90 100 100)"],
            &["1", "2.5", "-3", " ", ",", "10 20", ".5", "-0", " 100 100", "1e1", " 0 0 "],
            &["#abc", "#aabbcc", "#11223344", "#ABCD", "rgb(1 2 3)", "rgb(100%, 0%, 50%)", "rgba(1,2,3,0.5)", "hsl(120 50% 50%)", "hsla(-40deg 100% 25% / 50%)", "red", "Blue", "transparent", "rebeccapurple", " ", "a", "0", "#", ")", "1"],
        ];
        let mut pick = sequence(0x0123_4567_89AB_CDEF);
        let view = DEFAULT_VIEW;
        let (mut paths, mut transforms, mut lists, mut colours) = (0, 0, 0, 0);
        for round in 0..40_000 {
            let own = kinds[round % 4];
            let lead = if round % 4 == 0 { "M1 1" } else { "" };
            let text: String = std::iter::once(lead).chain((0..1 + pick(if round % 4 == 3 { 2 } else { 14 })).map(|_| if pick(16) == 0 { pieces[pick(pieces.len())] } else { own[pick(own.len())] })).collect();
            if let Ok(path) = path_data(&text, &view, MAX_D_CHARS, &unmasked) {
                paths += 1;
                assert!(path.text.chars().count() <= MAX_D_CHARS && path.text.bytes().all(|b| b.is_ascii_alphanumeric() || b" .-".contains(&b)), "{text:?} -> {:?}", path.text);
                let again = path_data(&path.text, &view, MAX_D_CHARS, &unmasked).unwrap_or_else(|e| panic!("{text:?} -> {:?}: {e}", path.text));
                assert_eq!((&again.text, &again.structure, again.bbox), (&path.text, &path.structure, path.bbox), "{text:?}");
                // Under the view that reads back anything stored it is the same path.
                assert_eq!(path_data(&path.text, &WIDE, MAX_D_CHARS, &unmasked).map(|p| p.structure).as_ref(), Ok(&path.structure), "{text:?}");
                assert_eq!(path.structure.sets.iter().map(|n| usize::from(*n)).sum::<usize>(), path.structure.numbers + path.structure.flags.len());
                if let Some(b) = path.bbox {
                    assert!([b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite()) && b.x0 <= b.x1 && b.y0 <= b.y1, "{text:?} -> {b:?}");
                }
            }
            if let Ok((written, matrix)) = transform(&text, &view, &unmasked) {
                transforms += 1;
                assert!(written.len() <= MAX_TRANSFORM_CHARS && written.bytes().all(|b| b.is_ascii_alphanumeric() || b" .-()".contains(&b)), "{text:?} -> {written:?}");
                assert!(matrix.iter().all(|v| v.is_finite()), "{text:?} -> {matrix:?}");
                let again = transform(&written, &view, &unmasked).unwrap_or_else(|e| panic!("{text:?} -> {written:?}: {e}"));
                assert_eq!((again.0.as_str(), again.1), (written.as_str(), matrix), "{text:?}");
                assert_eq!(transform(&written, &WIDE, &unmasked).map(|t| t.1), Ok(matrix), "{text:?}");
            }
            if let Ok((written, bbox)) = point_list(&text, &view) {
                lists += 1;
                assert!(written.len() <= MAX_POINTS_CHARS && written.bytes().all(|b| b.is_ascii_digit() || b" .-".contains(&b)), "{text:?} -> {written:?}");
                assert_eq!(point_list(&written, &view), Ok((written.clone(), bbox)), "{text:?}");
            }
            if let Some(c) = colour(&text) {
                colours += 1;
                let written = hex(c);
                assert!(written.starts_with('#') && (written.len() == 7 || written.len() == 9) && written.bytes().skip(1).all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{text:?} -> {written}");
                assert_eq!(colour(&written), Some(c), "{text:?}");
            }
            if let Some(id) = url_id(&text) {
                assert!(valid_id(id), "{text:?} -> {id:?}");
            }
            if let Some(numbers) = number_list(&text, 8) {
                assert!(numbers.len() <= 8, "{text:?}");
            }
            let _ = (scalar(&text).is_some(), plain_number(&text), style_urls_ok(&text), valid_lang(&text), name_fault(&text), q(&unmasked, &text), cut(&text, 7));
        }
        // The sweep is worth its name only while every parser takes a fair share of it.
        assert!(paths > 1000 && transforms > 300 && lists > 1000 && colours > 1000, "{paths} paths, {transforms} transforms, {lists} point lists, {colours} colours");
    }
    #[test]
    fn the_edges_of_what_is_accepted_are_whole_looks() {
        // Text of every kind that one line of plain text may hold.
        let look = drawn("<text id='a' x='5' y='20'>می\u{200C}خواهم</text><text id='b' x='5' y='40'>1\u{FE0F}\u{20E3} \u{2764}\u{FE0F} \u{1F44D}\u{1F3FD}</text><text id='c' x='5' y='60'>\u{3164}x\u{034F}y</text><text id='d' x='5' y='80'>\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}</text><text id='e' x='5' y='100'>\u{0301}e\u{0301}\u{0302}\u{0303}</text><text id='f' x='5' y='120'> </text>");
        assert_eq!(node(&look, "a").text.as_deref(), Some("می\u{200C}خواهم"));
        assert_eq!(node(&look, "b").text.as_deref(), Some("1\u{FE0F}\u{20E3} \u{2764}\u{FE0F} \u{1F44D}\u{1F3FD}"));
        assert_eq!(node(&look, "c").text.as_deref(), Some("\u{3164}x\u{034F}y"));
        assert_eq!(node(&look, "d").text.as_deref(), Some("\u{1F468}\u{1F469}\u{1F467}"));
        assert_eq!(node(&look, "e").text.as_deref(), Some("e\u{0301}\u{0302}"));
        assert_eq!(node(&look, "f").text.as_deref(), Some(""));
        assert_eq!(round_trip(&look), look);
        // Forty characters of four bytes each.
        let look = drawn(&format!("<text id='t' x='5' y='20'>{}</text>", "\u{1F600}".repeat(MAX_TEXT_CHARS)));
        assert_eq!(node(&look, "t").text.as_ref().map(|t| t.chars().count()), Some(MAX_TEXT_CHARS));
        // A box at the bound, as stored.
        let look = drawn("<g id='a'><circle id='c' transform='scale(1000)' cx='900' cy='0' r='100'/></g>");
        assert_eq!(bbox(&look, "a"), [800000.0, -100000.0, 200000.0, 200000.0]);
        // The most a track and an animation may hold of everything, at the ends of every range.
        let ids: Vec<String> = (0..MAX_TARGETS).map(|i| format!("c{i}")).collect();
        let circles: String = ids.iter().map(|id| format!("<circle id='{id}' cx='100' cy='100' r='5' fill='#ff0000' stroke='#000000' stroke-width='1'/>")).collect();
        let times: Vec<f64> = (0..MAX_KEYFRAMES).map(|i| i as f64 / (MAX_KEYFRAMES - 1) as f64).collect();
        let eases: Vec<&str> = (0..MAX_KEYFRAMES - 1).map(|i| ["linear", "in", "out", "inOut", "outBack", "step"][i % 6]).collect();
        let track = |prop: &str, values: Vec<Value>| json!({ "target": ids, "prop": prop, "values": values, "times": times, "ease": eases, "phase": 1 });
        let spread = |low: f64, high: f64| (0..MAX_KEYFRAMES).map(|i| json!(if i % 2 == 0 { low } else { high })).collect::<Vec<Value>>();
        let applied = accepted(
            None,
            json!({
                "svg": svg(&circles),
                "pivots": { "c0": [-800, 1000] },
                "animations": { "full": { "duration": 30, "loop": true, "tracks": [
                    track("translateX", spread(-800.0, 800.0)), track("translateY", spread(800.0, -800.0)), track("rotate", spread(-3600.0, 3600.0)), track("scale", spread(-10.0, 10.0)),
                    track("scaleX", spread(0.0, 10.0)), track("scaleY", spread(-10.0, 0.0)), track("strokeWidth", spread(0.0, 800.0)), track("cx", spread(-800.0, 1000.0)),
                    track("cy", spread(1000.0, -800.0)), track("r", spread(0.0, 800.0)),
                    json!({ "target": ids, "prop": "opacity", "values": [0, 1, 0], "ease": ["in", "out"] }),
                    json!({ "target": ids, "prop": "fill", "values": ["#ff0000", "#00000000", "#ff0000"], "times": [0, 0.006, 1] }),
                    json!({ "target": ids, "prop": "stroke", "values": [null, "#ffffffff", null], "phase": 0 }),
                    json!({ "target": "c0", "prop": "opacity", "values": [null, null] }),
                    json!({ "target": "c1", "prop": "r", "values": [null, 0, null], "ease": "step" }),
                    json!({ "target": "c2", "prop": "rotate", "values": [0, 0] })
                ] }, "quick": { "duration": 0.1, "every": [1, 120], "tracks": [{ "target": "c3", "prop": "scaleY", "values": [1, 0.1, 1], "times": [0, 0.4, 1], "delay": 0.1 }] } },
                "states": { "idle": [], "working": ["full", "quick", "breathe", "blink", "think", "talk"] },
                "gaze": 10
            }),
        );
        let look = applied.design.look.as_ref().unwrap();
        assert_eq!((look.animations["full"].tracks.len(), look.animations["full"].tracks[0].values.len()), (MAX_TRACKS, MAX_KEYFRAMES));
        assert_eq!(round_trip(look), *look);
        assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()).as_ref(), Some(&applied.design));
    }

    // ------------------------------------------------------------ use, masks, filters, dashes

    #[test]
    fn a_use_repeats_an_element_before_it_and_its_box_is_that_elements_moved_by_x_and_y() {
        let applied = accepted(
            None,
            json!({ "svg": svg(
                "<defs><g id='eye' transform='translate(5 5)'><circle cx='10' cy='10' r='5' fill='#ffffff'/><circle id='iris' cx='10' cy='10' r='2'/></g></defs>\
                 <use id='l' href='#eye' x='50' y='60'/><use id='r' xlink:href=' #eye ' x='100' y='60' fill='red' transform='rotate(10 110 75)' opacity='0.5'/>"
            ) }),
        );
        let look = applied.design.look.as_ref().unwrap();
        assert_eq!(attrs(node(look, "l")), json!({ "useRef": "eye", "x": 50, "y": 60 }));
        assert_eq!(attrs(node(look, "r")), json!({ "useRef": "eye", "x": 100, "y": 60, "fill": "#ff0000", "transform": "rotate(10 110 75)", "opacity": 0.5 }));
        // The box of what it repeats, through that element's own transform, moved by x and y; its own transform is not in it.
        assert_eq!((bbox(look, "eye"), bbox(look, "l"), bbox(look, "r")), ([5.0, 5.0, 10.0, 10.0], [60.0, 70.0, 10.0, 10.0], [110.0, 70.0, 10.0, 10.0]));
        // A use is a part; what it repeats in defs is not.
        assert_eq!(applied.parts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["l", "r"]);
        assert_eq!(applied.parts[0].to_string(), "l use 60 70 10 10 pivot 65 75 (default)");
        // Each use counts with what it repeats: defs, g, two circles, and twice 1 + 3.
        assert_eq!(applied.usage.elements, 12);
        assert_eq!(applied.warnings, [""; 0]);
        has(&to_svg(look), "<use id='l' href='#eye' x='50' y='60'/>");
        assert_eq!(&round_trip(look), look);
        // It moves like any part, and what it repeats may be a part as well.
        let moved = accepted(None, json!({
            "svg": svg("<g id='arm'><rect x='10' y='10' width='10' height='40'/></g><use id='twin' href='#arm' x='100'/>"),
            "pivots": { "twin": [115, 10], "arm": [15, 10] }, "roles": { "armL": "arm", "armR": "twin" },
            "animations": { "wave": { "duration": 1, "tracks": [{ "target": "twin", "prop": "rotate", "values": [0, -40, 0] }, { "target": "twin", "prop": "opacity", "values": [null, 0.5, null] }] } }
        }));
        assert_eq!(bbox(moved.design.look.as_ref().unwrap(), "twin"), [110.0, 10.0, 10.0, 40.0]);
        // Both play a role, and a use repeats its part with all its motion: that is said.
        assert_eq!(moved.warnings, ["twin is a use of arm, the armL part: it also moves whenever arm moves (at roles.armR): draw it as a part of its own, or let both be uses of a shape in defs"]);
        // width and height of a use size nothing here.
        assert_eq!(accepted(None, json!({ "svg": svg("<circle id='c' cx='50' cy='50' r='5'/><use href='#c' x='10' width='20' height='20'/>") })).warnings, ["width and height of a use are left out (2 times, first at line 1 column 66): a use draws what it repeats at its own size, scale it with transform"]);
        // A g in defs may hold a use, and is then not one to repeat.
        drawn("<circle id='c' cx='50' cy='50' r='5'/><defs><g id='two'><use href='#c'/></g></defs>");

        for (inner, why) in [
            ("<use/>", "svg: a use without href (at line 1 column 28): href='#id' of a g, a shape or a text in this svg"),
            ("<use href='#nope'/>", "svg: href refers to #nope, which is not in this svg: #id of a g, a shape or a text in this svg"),
            ("<use href='a.svg#b'/>", "svg: href \"a.svg#b\" reaches outside the artwork: #id of a g, a shape or a text in this svg"),
            ("<use href='#'/>", "svg: href \"#\" reaches outside the artwork"),
            ("<use href='#1a'/>", "svg: href \"#1a\" reaches outside the artwork"),
            ("<use href='url(#a)'/>", "reaches outside the artwork"),
            ("<defs><linearGradient id='lg'/></defs><use href='#lg'/>", "svg: href refers to #lg, a linearGradient: #id of a g, a shape or a text in this svg"),
            ("<circle id='c' r='5'/><use id='u' href='#c'/><use href='#u'/>", "svg: href refers to #u, a use: a use repeats a g, a shape or a text, not another use"),
            ("<defs><clipPath id='cp'><circle id='pane' r='5'/></clipPath></defs><use href='#pane'/>", "svg: href refers to #pane, which is inside a clipPath: what a use repeats stands outside clip paths, masks and filters"),
            ("<defs><mask id='m'><g id='spot'><circle r='5'/></g></mask></defs><use href='#spot'/>", "svg: href refers to #spot, which is inside a mask"),
            ("<defs><mask id='m'><circle r='5'/></mask></defs><use href='#m'/>", "svg: href refers to #m, a mask"),
            ("<defs><filter id='f'><feOffset/></filter></defs><use href='#f'/>", "svg: href refers to #f, a filter"),
            ("<g id='a'><use href='#a'/></g>", "svg: href refers to #a, which holds this use (at line 1 column 38, in g#a): a use repeats an element outside itself"),
            ("<use id='u' href='#u'/>", "svg: a use refers to itself (at line 1 column 28, in use#u): a use repeats another element of the artwork"),
            ("<circle id='c' r='5'/><g id='two'><use href='#c'/></g><use href='#two'/>", "svg: href refers to #two, which holds a use: what a use repeats holds no use itself"),
            ("<use href='#later'/><g id='later'><use href='#x'/></g><circle id='x' r='5'/>", "svg: href refers to #later, which holds a use"),
            ("<use href='#later'/><defs><linearGradient id='later'/></defs>", "svg: href refers to #later, a linearGradient"),
            ("<use href='#nope'/><circle id='later' r='5'/>", "svg: href refers to #nope, which is not in this svg"),
            ("<circle id='c' r='5'/><use href='#c'><circle r='1'/></use>", "svg: <circle> inside <use>: a use has no element children"),
            ("<circle id='c' r='5'/><use href='#c' xlink:href='#c'/>", "svg: href is given twice on <use>"),
            ("<circle id='c' r='5'/><use href='#c' xlink:role='x'/>", "svg: the prefixed attribute \"xlink:role\""),
            ("<circle id='c' r='5'/><use href='#c' onload='x'/>", "svg: the event attribute \"onload\""),
            ("<circle id='c' r='5'/><use href='#c' d='M0 0L1 1'/>", "\"d\" is not an attribute of <use>"),
            ("<circle r='5' href='#a'/>", "svg: href on <circle> (at line 1 column 28): the artwork links to nothing; only a use refers to an element of it, with href='#id'"),
            ("<g xlink:href='#a'><circle r='5'/></g>", "svg: xlink:href on <g>"),
        ] {
            has(&undrawn(inner), why);
        }
        // What it repeats may come after it, also in defs at the end; the box is the same.
        let ahead = accepted(None, json!({ "svg": svg("<use id='u' href='#dot' x='10'/><g id='g'><use id='v' href='#dot' y='20'/></g><defs><circle id='dot' cx='5' cy='5' r='5'/></defs>") }));
        let look = ahead.design.look.as_ref().unwrap();
        assert_eq!((bbox(look, "u"), bbox(look, "v"), bbox(look, "g")), ([10.0, 0.0, 10.0, 10.0], [0.0, 20.0, 10.0, 10.0], [0.0, 20.0, 10.0, 10.0]));
        assert_eq!((ahead.usage.elements, ahead.warnings.len()), (7, 0));
        assert_eq!(&round_trip(look), look);
        // A second reading says what the first would have, once: the problems of the artwork count once.
        let twice = undrawn("<use href='#dot'/><circle r='5' onclick='x'/><defs><circle id='dot' r='5'/></defs>");
        assert_eq!(twice, "svg: the event attribute \"onclick\" (at line 1 column 46): an avatar's artwork has no scripts");
        // What a use repeats counts again for the elements, to the limit: a target of ten elements.
        let target = format!("<defs><g id='t'>{}</g></defs>", "<circle cx='50' cy='50' r='5'/>".repeat(9));
        let uses = |n: usize| format!("{target}{}", "<use href='#t'/>".repeat(n));
        assert_eq!(accepted(None, json!({ "svg": svg(&uses(26)) })).usage.elements, 297);
        has(&undrawn(&uses(27)), "svg: more than 300 elements, with what each use repeats: draw with fewer, larger shapes, or repeat smaller parts");
        // Also when what they repeat comes after them.
        let after = |n: usize| format!("{}{target}", "<use href='#t'/>".repeat(n));
        assert_eq!(accepted(None, json!({ "svg": svg(&after(26)) })).usage.elements, 297);
        has(&undrawn(&after(27)), "svg: more than 300 elements");
    }

    #[test]
    fn a_mask_in_defs_masks_what_refers_to_it_and_holds_g_and_shapes() {
        let look = drawn(
            "<defs><linearGradient id='fade'><stop offset='0' stop-color='#ffffff'/><stop offset='1' stop-color='#000000'/></linearGradient>\
             <mask id='m' maskUnits='userSpaceOnUse' maskContentUnits='userSpaceOnUse' x='0' y='0' width='50%' height='100%' mask-type='alpha'><g transform='translate(1 1)'><rect id='window' x='0' y='0' width='100' height='199' fill='url(#fade)'/></g><circle cx='50' cy='50' r='20' fill='white'/></mask>\
             <mask id='m2' x='-10%' y='-10%' width='120%' height='1.2' style='mask-type: luminance'><circle cx='0.5' cy='0.5' r='0.5' fill='#ffffff'/></mask></defs>\
             <g id='body' mask='url(#m)'><circle id='c' cx='100' cy='100' r='50'/></g><text id='t' x='10' y='20' mask=\"url('#m2')\">a</text><use id='u' href='#c' x='5' style='mask:url(#m)'/><circle id='plain' r='5' mask='none'/>",
        );
        assert_eq!(attrs(node(&look, "m")), json!({ "maskUnits": "userSpaceOnUse", "maskContentUnits": "userSpaceOnUse", "x": 0, "y": 0, "width": 100, "height": 200, "maskType": "alpha" }));
        assert_eq!(attrs(node(&look, "m2")), json!({ "x": -0.1, "y": -0.1, "width": 1.2, "height": 1.2, "maskType": "luminance" }));
        assert_eq!((attrs(node(&look, "body")), attrs(node(&look, "t"))["maskRef"].clone(), attrs(node(&look, "u"))["maskRef"].clone()), (json!({ "maskRef": "m" }), json!("m2"), json!("m")));
        assert_eq!(attrs(node(&look, "plain")), json!({ "r": 5 }));
        // A masked part keeps the box of its geometry; a mask spans its shapes as a clip path does.
        assert_eq!((bbox(&look, "body"), bbox(&look, "m")), ([50.0, 50.0, 100.0, 100.0], [1.0, 1.0, 100.0, 199.0]));
        assert_eq!(round_trip(&look), look);
        // What a mask holds is not a part.
        has(&refused(None, json!({ "svg": to_svg(&look), "pivots": { "window": [1, 1] } })), "window stands inside defs, a mask or a clipPath, where nothing moves");

        for (inner, why) in [
            ("<mask id='m'><circle r='5'/></mask><circle r='5'/>", "svg: <mask> inside <svg>: masks and filters belong inside defs"),
            ("<g><mask id='m'/></g><circle r='5'/>", "svg: <mask> inside <g>"),
            ("<defs><mask id='m'><text>a</text></mask></defs><circle r='5'/>", "svg: <text> inside <mask>: a mask holds g and shapes only"),
            ("<defs><mask id='m'><g><text>a</text></g></mask></defs><circle r='5'/>", "svg: <text> inside <g>: a mask holds g and shapes only"),
            ("<defs><mask id='m'><defs/></mask></defs><circle r='5'/>", "svg: <defs> inside <mask>"),
            ("<circle id='c' r='5'/><defs><mask id='m'><use href='#c'/></mask></defs>", "svg: <use> inside <mask>: a mask holds g and shapes only"),
            ("<defs><mask id='m'><mask id='n'/></mask></defs><circle r='5'/>", "svg: <mask> inside <mask>"),
            ("<defs><clipPath id='cp'><circle r='5'/></clipPath><mask id='m'><circle r='5' clip-path='url(#cp)'/></mask></defs><circle r='5'/>", "svg: clip-path inside a mask: a mask and its shapes are not clipped themselves"),
            ("<defs><mask id='m'><g mask='url(#m)'><circle r='5'/></g></mask></defs><circle r='5'/>", "svg: mask inside a mask: a mask and its shapes are not masked or filtered themselves"),
            ("<defs><filter id='f'><feOffset/></filter><mask id='m'><circle r='5' filter='url(#f)'/></mask></defs><circle r='5'/>", "svg: filter inside a mask"),
            ("<defs><mask id='m'><circle r='5'/></mask><clipPath id='cp'><circle r='5' mask='url(#m)'/></clipPath></defs><circle r='5'/>", "svg: mask inside a clipPath: a clipPath and its shapes are not masked or filtered themselves"),
            ("<circle id='c' r='1'/><circle r='5' mask='url(#c)'/>", "svg: mask refers to #c, a circle: url(#id) of a mask in this svg"),
            ("<circle r='5' mask='url(#nope)'/>", "svg: mask refers to #nope, which is not in this svg"),
            ("<circle r='5' mask='circle(5px)'/>", "svg: mask \"circle(5px)\": url(#id) of a mask in this svg, or none"),
            ("<circle r='5' mask='url(#m) red'/>", "svg: mask \"url(#m) red\""),
            ("<defs><mask id='m' mask-type='colour'/></defs><circle r='5'/>", "one of luminance, alpha"),
            ("<defs><mask id='m' maskUnits='pixels'/></defs><circle r='5'/>", "one of userSpaceOnUse, objectBoundingBox"),
            ("<defs><mask id='m' width='-5%'/></defs><circle r='5'/>", "svg: width \"-5%\" is out of range: a share of the painted box, such as 0.5 or 50%, at most 1000%"),
            ("<defs><mask id='m' maskUnits='userSpaceOnUse' x='99999'/></defs><circle r='5'/>", "svg: x \"99999\" is out of range: a coordinate near the viewBox"),
            ("<defs><mask id='m' fill='red'/></defs><circle r='5'/>", "\"fill\" is not an attribute of <mask>"),
            ("<defs><mask id='m' transform='scale(2)'/></defs><circle r='5'/>", "\"transform\" is not an attribute of <mask>"),
        ] {
            has(&undrawn(inner), why);
        }
    }

    #[test]
    fn a_filter_of_the_listed_primitives_is_kept_number_for_number() {
        let look = drawn(
            "<defs><filter id='every' x='-20%' y='-20%' width='140%' height='140%' filterUnits='objectBoundingBox' primitiveUnits='userSpaceOnUse' color-interpolation-filters='linearRGB'>\
               <feGaussianBlur in='SourceAlpha' stdDeviation='2,3' result='blur'/><feOffset in='blur' dx='1' dy='-2' result='moved'/>\
               <feFlood flood-color='rgba(0,0,0,0.5)' flood-opacity='50%' result='ink'/>\
               <feComposite in='ink' in2='moved' operator='arithmetic' k1='0' k2='1' k3='-0.5' k4='0.25' result='shade'/>\
               <feColorMatrix in='shade' type='saturate' values='0.4' result='grey'/><feBlend in='SourceGraphic' in2='grey' mode='multiply' result='mixed'/>\
               <feMorphology in='mixed' operator='dilate' radius='1 2' result='fat'/><feMerge result='merged'><feMergeNode in='fat'/><feMergeNode in='SourceGraphic'/></feMerge>\
             </filter>\
             <filter id='more' style='color-interpolation-filters: sRGB'><feDropShadow dx='0' dy='3' stdDeviation='2' floodColor='currentColor' floodOpacity='0.3'/>\
               <feTurbulence type='fractalNoise' baseFrequency='0.02 0.5' numOctaves='3' seed='7' result='noise'/>\
               <feDisplacementMap in='SourceGraphic' in2='noise' scale='-12' xChannelSelector='r' yChannelSelector='G'/>\
               <feColorMatrix type='matrix' values='1 0 0 0 0 0 1 0 0 0 0 0 1 0 0 0 0 0 1 0'/><feColorMatrix type='hueRotate' values='90'/><feColorMatrix type='luminanceToAlpha'/>\
             </filter></defs>\
             <g id='body' filter='url(#every)'><circle id='c' cx='100' cy='100' r='50'/></g><circle id='d' cx='20' cy='20' r='5' style='filter: url(#more)'/><circle id='e' cx='20' cy='20' r='5' filter='none'/>",
        );
        let all = node(&look, "every");
        assert_eq!(attrs(all), json!({ "x": -0.2, "y": -0.2, "width": 1.4, "height": 1.4, "filterUnits": "objectBoundingBox", "primitiveUnits": "userSpaceOnUse", "colorInterpolationFilters": "linearRGB" }));
        let kept: Vec<(Tag, Value)> = all.children.iter().map(|n| (n.tag, attrs(n))).collect();
        assert_eq!(kept, [
            (Tag::FeGaussianBlur, json!({ "in": "SourceAlpha", "stdDeviation": "2 3", "result": "blur" })),
            (Tag::FeOffset, json!({ "in": "blur", "dx": 1, "dy": -2, "result": "moved" })),
            (Tag::FeFlood, json!({ "floodColor": "#00000080", "floodOpacity": 0.5, "result": "ink" })),
            (Tag::FeComposite, json!({ "in": "ink", "in2": "moved", "operator": "arithmetic", "k1": 0, "k2": 1, "k3": -0.5, "k4": 0.25, "result": "shade" })),
            (Tag::FeColorMatrix, json!({ "in": "shade", "type": "saturate", "values": "0.4", "result": "grey" })),
            (Tag::FeBlend, json!({ "in": "SourceGraphic", "in2": "grey", "mode": "multiply", "result": "mixed" })),
            (Tag::FeMorphology, json!({ "in": "mixed", "operator": "dilate", "radius": "1 2", "result": "fat" })),
            (Tag::FeMerge, json!({ "result": "merged" })),
        ]);
        assert_eq!(all.children[7].children.iter().map(|n| (n.tag, attrs(n))).collect::<Vec<_>>(), [(Tag::FeMergeNode, json!({ "in": "fat" })), (Tag::FeMergeNode, json!({ "in": "SourceGraphic" }))]);
        let more = node(&look, "more");
        assert_eq!(attrs(more), json!({ "colorInterpolationFilters": "sRGB" }));
        assert_eq!(more.children.iter().map(attrs).collect::<Vec<_>>(), [
            json!({ "dx": 0, "dy": 3, "stdDeviation": "2", "floodColor": "currentColor", "floodOpacity": 0.3 }),
            json!({ "type": "fractalNoise", "baseFrequency": "0.02 0.5", "numOctaves": 3, "seed": 7, "result": "noise" }),
            json!({ "in": "SourceGraphic", "in2": "noise", "scale": -12, "xChannelSelector": "R", "yChannelSelector": "G" }),
            json!({ "type": "matrix", "values": "1 0 0 0 0 0 1 0 0 0 0 0 1 0 0 0 0 0 1 0" }),
            json!({ "type": "hueRotate", "values": "90" }),
            json!({ "type": "luminanceToAlpha" }),
        ]);
        assert_eq!((attrs(node(&look, "body")), attrs(node(&look, "d"))["filterRef"].clone(), attrs(node(&look, "e"))), (json!({ "filterRef": "every" }), json!("more"), json!({ "cx": 20, "cy": 20, "r": 5 })));
        // A filtered part keeps the box of its geometry; a filter has none of its own.
        assert_eq!((bbox(&look, "body"), bbox(&look, "every")), ([50.0, 50.0, 100.0, 100.0], [0.0, 0.0, 0.0, 0.0]));
        assert_eq!(round_trip(&look), look);
        has(&to_svg(&look), "<feGaussianBlur in='SourceAlpha' result='blur' stdDeviation='2 3'/>");

        let filter = |primitives: &str| undrawn(&format!("<defs><filter id='f'>{primitives}</filter></defs><circle r='5' filter='url(#f)'/>"));
        for (primitives, why) in [
            ("<feOffset in='later'/><feFlood result='later'/>", "svg: in \"later\" is not the result of an earlier primitive (at line 1 column 49, in filter#f): SourceGraphic, SourceAlpha or an earlier result"),
            ("<feOffset in='BackgroundImage'/>", "svg: in \"BackgroundImage\" is not given to an avatar's filters"),
            ("<feOffset in='sourceGraphic'/>", "svg: in \"sourceGraphic\" is not the result of an earlier primitive"),
            ("<feComposite in='SourceGraphic' in2='nope'/>", "svg: in2 \"nope\" is not the result of an earlier primitive"),
            ("<feMerge><feMergeNode in='nope'/></feMerge>", "svg: in \"nope\" is not the result of an earlier primitive"),
            ("<feMerge result='m'><feMergeNode in='m'/></feMerge>", "svg: in \"m\" is not the result of an earlier primitive"),
            ("<feFlood result='SourceGraphic'/>", "svg: result SourceGraphic would hide the input of that name: name a result in your own words, such as blur or glow"),
            ("<feFlood result='1x'/>", "svg: result \"1x\": a letter, then letters, digits, _ or -, at most 32 characters"),
            ("<feFlood result='constructor'/>", "this name is reserved"),
            ("<feGaussianBlur stdDeviation='50.001'/>", "svg: stdDeviation \"50.001\" is out of range: one or two numbers, x and y, each from 0 to 50"),
            ("<feGaussianBlur stdDeviation='-1'/>", "svg: stdDeviation \"-1\" is out of range"),
            ("<feGaussianBlur stdDeviation='1 2 3'/>", "svg: stdDeviation \"1 2 3\": one or two numbers, x and y, each from 0 to 50"),
            ("<feGaussianBlur stdDeviation='2px'/>", "svg: stdDeviation \"2px\""),
            ("<feGaussianBlur stdDeviation=''/>", "svg: stdDeviation \"\""),
            ("<feMorphology radius='20.5'/>", "svg: radius \"20.5\" is out of range: one or two numbers, x and y, each from 0 to 20"),
            ("<feTurbulence baseFrequency='1.5'/>", "svg: baseFrequency \"1.5\" is out of range"),
            ("<feTurbulence numOctaves='5'/>", "svg: numOctaves 5 is out of range: a whole number from 1 to 4"),
            ("<feTurbulence numOctaves='2.5'/>", "svg: numOctaves 2.5 is out of range"),
            ("<feTurbulence numOctaves='0'/>", "svg: numOctaves 0 is out of range"),
            ("<feTurbulence seed='100001'/>", "svg: seed 100001 is out of range: within 100000 either way"),
            ("<feTurbulence type='noise'/>", "one of fractalNoise, turbulence"),
            ("<feDisplacementMap in2='SourceAlpha' scale='51'/>", "svg: scale 51 is out of range: within 50 either way, a quarter of the viewBox width"),
            ("<feDisplacementMap xChannelSelector='X'/>", "one of R, G, B, A"),
            ("<feComposite operator='erode'/>", "one of over, in, out, atop, xor, arithmetic"),
            ("<feComposite k1='101'/>", "svg: k1 101 is out of range: within 100 either way"),
            ("<feMorphology operator='over'/>", "one of erode, dilate"),
            ("<feBlend mode='plus'/>", "one of normal, multiply, screen, overlay"),
            ("<feColorMatrix values='1 2 3'/>", "svg: a feColorMatrix of type matrix with 3 values: 20 numbers, four rows of five"),
            ("<feColorMatrix type='saturate' values='1 1'/>", "svg: a feColorMatrix of type saturate with 2 values"),
            ("<feColorMatrix type='luminanceToAlpha' values='1'/>", "svg: a feColorMatrix of type luminanceToAlpha with 1 values: no values"),
            ("<feColorMatrix type='hueRotate' values='3601'/>", "svg: values \"3601\" is out of range"),
            ("<feColorMatrix type='hueRotate' values='a'/>", "svg: values \"a\""),
            ("<feDropShadow flood-color='url(#f)'/>", "svg: flood-color \"url(#f)\" reaches outside the artwork: a plain colour"),
            ("<feDropShadow dx='99999'/>", "svg: dx 99999 is out of range: within 800"),
            ("<feFlood x='0'/>", "\"x\" is not an attribute of <feFlood>"),
            ("<feGaussianBlur edgeMode='none'/>", "\"edgeMode\" is not an attribute of <feGaussianBlur>"),
            ("<feOffset id='o'/>", "\"id\" is not an attribute of <feOffset>"),
            ("<feOffset href='#x'/>", "svg: href on <feOffset>"),
            ("<feImage href='http://x/a.png'/>", "svg: <feImage> is not supported (at line 1 column 49, in filter#f): images are not accepted"),
            ("<feTile/>", "svg: <feTile> is not supported (at line 1 column 49, in filter#f): use blur, shadow, colour matrix, composite, blend, morphology, turbulence or displacement"),
            ("<feComponentTransfer><feFuncR type='identity'/></feComponentTransfer>", "svg: <feComponentTransfer> is not supported"),
            ("<feConvolveMatrix kernelMatrix='1'/>", "svg: <feConvolveMatrix> is not supported"),
            ("<feDiffuseLighting><feDistantLight/></feDiffuseLighting>", "svg: <feDiffuseLighting> is not supported"),
            ("<feSpecularLighting/>", "svg: <feSpecularLighting> is not supported"),
            ("<feBogus/>", "svg: the element \"feBogus\" is not supported (at line 1 column 49, in filter#f): the primitives are feGaussianBlur, feDropShadow"),
            ("<circle r='5'/>", "svg: <circle> inside <filter>: a filter holds filter primitives only"),
            ("<feMergeNode in='SourceGraphic'/>", "svg: <feMergeNode> inside <filter>: a feMergeNode belongs inside a feMerge"),
            ("<feMerge><feOffset/></feMerge>", "svg: <feOffset> inside <feMerge>: a filter primitive belongs inside a filter"),
            ("<feOffset><feFlood/></feOffset>", "svg: <feFlood> inside <feOffset>: a filter primitive belongs inside a filter"),
            ("<feMerge><feMergeNode><feMergeNode/></feMergeNode></feMerge>", "svg: <feMergeNode> inside <feMergeNode>"),
            ("<feMerge><circle r='5'/></feMerge>", "svg: <circle> inside <feMerge>: a feMerge holds feMergeNode elements only"),
            ("words", "svg: text \"words\" outside a text element"),
        ] {
            has(&filter(primitives), why);
        }
        // A result is the filter's own: another filter cannot take it up.
        has(&undrawn("<defs><filter id='a'><feFlood result='x'/></filter><filter id='b'><feOffset in='x'/></filter></defs><circle r='5'/>"), "svg: in \"x\" is not the result of an earlier primitive");
        // At the edges of the ranges.
        drawn("<defs><filter id='f'><feGaussianBlur stdDeviation='50 0'/><feMorphology radius='20'/><feTurbulence baseFrequency='0 1' numOctaves='4' seed='-100000'/><feDisplacementMap scale='-50'/><feComposite k1='-100' k4='100'/></filter></defs><circle r='5'/>");
        for (inner, why) in [
            ("<filter id='f'/><circle r='5'/>", "svg: <filter> inside <svg>: masks and filters belong inside defs"),
            ("<feGaussianBlur stdDeviation='1'/>", "svg: <feGaussianBlur> inside <svg>: a filter primitive belongs inside a filter"),
            ("<g><feOffset/></g>", "svg: <feOffset> inside <g>: a filter primitive belongs inside a filter"),
            ("<defs><filter id='f' fill='red'/></defs><circle r='5'/>", "\"fill\" is not an attribute of <filter>"),
            ("<defs><filter id='f' color-interpolation-filters='auto'/></defs><circle r='5'/>", "one of sRGB, linearRGB"),
            ("<defs><filter id='f' filterUnits='pixels'/></defs><circle r='5'/>", "one of userSpaceOnUse, objectBoundingBox"),
            ("<circle r='5' filter='blur(2px)'/>", "svg: filter \"blur(2px)\" is a CSS filter: write a filter element in defs and refer to it with url(#id)"),
            ("<circle r='5' style='filter: drop-shadow(0 0 2px red)'/>", "svg: filter \"drop-shadow(0 0 2px red)\" is a CSS filter"),
            ("<circle r='5' filter='url(#f) blur(2px)'/>", "svg: filter \"url(#f) blur(2px)\": url(#id) of a filter in this svg, or none"),
            ("<circle r='5' filter='url(http://x/a.svg#f)'/>", "svg: filter \"url(http://x/a.svg#f)\": url(#id) of a filter in this svg, or none"),
            ("<circle r='5' filter='glow'/>", "svg: filter \"glow\": url(#id) of a filter in this svg, or none"),
            ("<circle id='c' r='1'/><circle r='5' filter='url(#c)'/>", "svg: filter refers to #c, a circle: url(#id) of a filter in this svg"),
            ("<defs><mask id='m'/></defs><circle r='5' filter='url(#m)'/>", "svg: filter refers to #m, a mask"),
            ("<defs><filter id='f'/></defs><circle r='5' mask='url(#f)'/>", "svg: mask refers to #f, a filter"),
            ("<defs><filter id='f'/></defs><circle r='5' clip-path='url(#f)'/>", "svg: clip-path refers to #f, a filter"),
            ("<defs><filter id='f'/></defs><circle r='5' fill='url(#f)'/>", "svg: fill refers to #f, a filter"),
        ] {
            has(&undrawn(inner), why);
        }
        // The limits: filters in a look, primitives in a filter (merge nodes not counted), nodes in a merge.
        let filters = |n: usize| format!("<defs>{}</defs><circle r='5'/>", (0..n).map(|i| format!("<filter id='f{i}'><feOffset/></filter>")).collect::<String>());
        drawn(&filters(MAX_FILTERS));
        has(&undrawn(&filters(MAX_FILTERS + 1)), "svg: more than 8 filters");
        let primitives = |n: usize, nodes: usize| format!("<defs><filter id='f'>{}<feMerge>{}</feMerge></filter></defs><circle r='5'/>", "<feOffset/>".repeat(n - 1), "<feMergeNode/>".repeat(nodes));
        drawn(&primitives(MAX_FILTER_PRIMITIVES, 8));
        has(&undrawn(&primitives(MAX_FILTER_PRIMITIVES + 1, 1)), "svg: more than 8 primitives in a filter: a few primitives make an effect");
        has(&undrawn(&primitives(2, 9)), "svg: more than 8 feMergeNode elements in a feMerge");
    }

    #[test]
    fn dashes_are_kept_as_lengths_and_their_offset_as_a_number() {
        let look = drawn(
            "<g id='g' stroke='#000000' stroke-dasharray='4,2  1' stroke-dashoffset='-3.5'><path id='p' d='M0 0 L10 0' stroke-dasharray='NONE'/><text id='t' x='5' y='50' strokeDasharray='1 1' strokeDashoffset='2'>a</text></g>\
             <circle id='c' cx='50' cy='50' r='5' style='stroke-dasharray: 3 3; stroke-dashoffset: 1'/><circle id='d' cx='50' cy='50' r='5' stroke-dasharray='1 2 3 4 5 6 7 8.5'/>",
        );
        assert_eq!(attrs(node(&look, "g")), json!({ "stroke": "#000000", "strokeDasharray": "4 2 1", "strokeDashoffset": -3.5 }));
        assert_eq!(attrs(node(&look, "p")), json!({ "d": "M 0 0 L 10 0", "strokeDasharray": "none" }));
        assert_eq!(attrs(node(&look, "t")), json!({ "x": 5, "y": 50, "strokeDasharray": "1 1", "strokeDashoffset": 2 }));
        assert_eq!(attrs(node(&look, "c")), json!({ "cx": 50, "cy": 50, "r": 5, "strokeDasharray": "3 3", "strokeDashoffset": 1 }));
        assert_eq!(attrs(node(&look, "d"))["strokeDasharray"], json!("1 2 3 4 5 6 7 8.5"));
        assert_eq!(round_trip(&look), look);
        // On the root they go to the group that wraps the content.
        let wrapped = accepted(None, json!({ "svg": "<svg viewBox='0 0 200 200' stroke-dasharray='2'><circle cx='50' cy='50' r='5'/></svg>" })).design.look.unwrap();
        assert_eq!(attrs(&wrapped.nodes[0]), json!({ "strokeDasharray": "2" }));
        for (attr, why) in [
            ("stroke-dasharray='1 1 1 1 1 1 1 1 1'", "svg: stroke-dasharray holds 9 numbers: up to 8 lengths, dash and gap in turn, separated by spaces, or none"),
            ("stroke-dasharray='1 1 1 1 1 1 1 1 1 1'", "svg: stroke-dasharray \"1 1 1 1 1 1 1 1 1 1\": up to 8 lengths"),
            ("stroke-dasharray='-1 2'", "svg: stroke-dasharray \"-1 2\" is out of range: lengths from 0 to 800"),
            ("stroke-dasharray='801'", "is out of range"),
            ("stroke-dasharray='4px 2px'", "svg: stroke-dasharray \"4px 2px\": up to 8 lengths"),
            ("stroke-dasharray='50%'", "svg: stroke-dasharray \"50%\""),
            ("stroke-dasharray=''", "svg: stroke-dasharray holds 0 numbers"),
            ("stroke-dasharray='1e999'", "svg: stroke-dasharray \"1e999\" is out of range"),
            ("stroke-dashoffset='99999'", "svg: stroke-dashoffset 99999 is out of range: within 800"),
            ("stroke-dashoffset='1 2'", "svg: stroke-dashoffset \"1 2\" is not a number"),
            ("stroke-dasharray='1' strokeDasharray='2'", "stroke-dasharray is given twice"),
        ] {
            has(&undrawn(&format!("<circle r='5' {attr}/>")), why);
        }
    }

    // ------------------------------------------------------------ new props, eases, built-in names

    #[test]
    fn skew_dash_offset_and_the_opacities_of_fill_and_stroke_are_props() {
        let with = |track: Value| figure(json!({ "pivots": { "head": [100, 110] }, "animations": { "a": { "duration": 1, "tracks": [track] } } }));
        // Skew is relative, in degrees, about the pivot.
        let look = accepted(None, with(json!({ "target": "head", "prop": "skewX", "values": [0, 80, -80, 0] }))).design.look.unwrap();
        assert_eq!(look.animations["a"].tracks[0].prop, Prop::SkewX);
        accepted(None, with(json!({ "target": "label", "prop": "skewY", "values": [0, 12.5] })));
        has(&refused(None, with(json!({ "target": "head", "prop": "skewY", "values": [0, 80.5] }))), "skewY 80.5 is out of range (at animations.a.tracks[0].values[1]): degrees, within 80 either way");
        has(&refused(None, with(json!({ "target": "head", "prop": "skewX", "values": [null, 5] }))), "null in a skewX track (at animations.a.tracks[0].values[0]): null is the artwork's own value; translate, rotate, skew and scale are relative to rest");
        let warned = accepted(None, tracks(json!([{ "target": "eyes", "prop": "skewX", "values": [0, 10, 0] }]))).warnings;
        assert_eq!(warned, ["eyes has no pivot: it turns and scales about the centre of its box, [90, 75] (at animations.a.tracks[0]): give it a pivot if its joint is elsewhere"]);
        // The dash offset is a number either way; null needs one on the element.
        accepted(None, with(json!({ "target": "mouth", "prop": "strokeDashoffset", "values": [0, -20, 0] })));
        has(&refused(None, with(json!({ "target": "mouth", "prop": "strokeDashoffset", "values": [null, 20] }))), "null is the artwork's own strokeDashoffset, and mouth has no stroke-dashoffset of its own");
        has(&refused(None, with(json!({ "target": "mouth", "prop": "strokeDashoffset", "values": [0, 900] }))), "strokeDashoffset 900 is out of range (at animations.a.tracks[0].values[1]): within 800 either way");
        // The opacities of fill and stroke: 0 to 1, under the paint rule of opacity.
        accepted(None, with(json!({ "target": "face", "prop": "fillOpacity", "values": [1, 0.2, 1] })));
        accepted(None, with(json!({ "target": "face", "prop": "strokeOpacity", "values": [1, 0.5] })));
        has(&refused(None, with(json!({ "target": "face", "prop": "fillOpacity", "values": [0, 1.5] }))), "fillOpacity 1.5 is out of range (at animations.a.tracks[0].values[1]): 0 to 1");
        has(&refused(None, with(json!({ "target": "face", "prop": "strokeOpacity", "values": [null, 0.5] }))), "null is the artwork's own strokeOpacity, and face has no stroke-opacity of its own");
        has(&refused(None, with(json!({ "target": "face", "prop": "fillOpacity", "values": [1, 0.5, 1], "ease": "step" }))), "ease step on fillOpacity");
        has(&refused(None, with(json!({ "target": "face", "prop": "strokeOpacity", "values": [1, 0.5, 1], "times": [0, 0.1, 1] }))), "keyframes 0 and 1 of strokeOpacity are 0.1 s apart (at animations.a.tracks[0]): at least 0.17 s between keyframes of fill, stroke and opacity");
        let looping = |values: Value| figure(json!({ "animations": { "a": { "duration": 1, "loop": true, "tracks": [{ "target": "face", "prop": "fillOpacity", "values": values }] } } }));
        accepted(None, looping(json!([1, 0.4, 1])));
        has(&refused(None, looping(json!([1, 0.4]))), "the loop's fillOpacity ends on another value than it starts with");
        // A one-shot that ends elsewhere is played once at a time.
        let once = accepted(None, figure(json!({ "animations": { "fade": { "duration": 1, "tracks": [{ "target": "face", "prop": "strokeOpacity", "values": [1, 0] }] } } }))).design.look.unwrap();
        assert!(!closes(&once, "fade"));
        // And with a value of its own, null is that value.
        let own = accepted(None, json!({ "svg": svg("<circle id='c' cx='50' cy='50' r='9' fill-opacity='0.5' stroke-dashoffset='3' stroke='#000000'/>"), "animations": { "a": { "duration": 1, "tracks": [{ "target": "c", "prop": "fillOpacity", "values": [null, 1, null] }, { "target": "c", "prop": "strokeDashoffset", "values": [null, 10, null] }] } } }));
        assert_eq!(own.warnings, [""; 0]);
    }

    #[test]
    fn an_ease_is_a_named_curve_or_a_bezier_for_all_segments_or_one_per_segment() {
        let with = |ease: Value| tracks(json!([{ "target": "arm", "prop": "rotate", "values": [0, 10, 0, 5, 0], "ease": ease }]));
        let ease = |ease: Value| accepted(None, with(ease)).design.look.unwrap().animations["a"].tracks[0].ease.clone();
        // Four numbers are one bezier, for every segment however many there are.
        assert_eq!(ease(json!([0.25, 0.1, 0.25, 1])), Some(Eases::One(Curve::Bezier([0.25, 0.1, 0.25, 1.0].map(Num::new)))));
        // A list of curves, one per segment: names and beziers mixed.
        let each = ease(json!(["inBack", [0, 0, 0.58, 1], "outElastic", [0.68, -0.6, 0.32, 1.6]]));
        assert_eq!(each, Some(Eases::Each(vec![Curve::Named(Ease::InBack), Curve::Bezier([0.0, 0.0, 0.58, 1.0].map(Num::new)), Curve::Named(Ease::OutElastic), Curve::Bezier([0.68, -0.6, 0.32, 1.6].map(Num::new))])));
        assert_eq!(serde_json::to_value(&each).unwrap(), json!(["inBack", [0, 0, 0.58, 1], "outElastic", [0.68, -0.6, 0.32, 1.6]]));
        // The edges of the control points.
        ease(json!([0, -2, 1, 3]));
        ease(json!([1, 3, 0, -2]));
        for wild in [json!([1.2, 0, 0, 1]), json!([0, -2.5, 1, 1]), json!([0, 0, -0.1, 1]), json!([0, 0, 1, 3.5]), json!(["in", [0, 4, 1, 1], "out", "in"])] {
            has(&refused(None, with(wild.clone())), "an ease [x1, y1, x2, y2] out of range (at animations.a.tracks[0].ease): x1 and x2 from 0 to 1, y1 and y2 from -2 to 3");
        }
        has(&refused(None, with(json!([0, 0, 1]))), "ease a list of 3");
        has(&refused(None, with(json!([0, 0, 1, 1, 1]))), "ease a list of 5");
        has(&refused(None, with(json!([[0, 0, 1], "in", "out", "in"]))), "ease a list of 4");
        has(&refused(None, with(json!(["in", "out"]))), "2 eases for 4 segments");
        // A bezier on the animation applies to its tracks; step stays off paint in a list as well.
        accepted(None, figure(json!({ "animations": { "a": { "duration": 1, "ease": [0.4, 0, 0.2, 1], "tracks": [{ "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9] }] } } })));
        has(&refused(None, tracks(json!([{ "target": "face", "prop": "opacity", "values": [0.9, 0.2, 0.9], "ease": [[0, 0, 1, 1], "step"] }]))), "ease step on opacity");
        // Every name on the wire.
        assert_eq!(Ease::ALL.map(|e| serde_json::to_value(e).unwrap()), ["linear", "in", "out", "inOut", "outBack", "inBack", "inOutBack", "outElastic", "outBounce", "step"].map(|n| json!(n)));
    }

    #[test]
    fn a_looks_own_animation_may_bear_the_name_of_built_in_motion() {
        // walk and greet replace the overlay's hop and wave; the other names the built-in set.
        let names: Vec<&str> = BUILTIN.iter().copied().chain(["walk", "greet"]).collect();
        let animations: Map<String, Value> = names.iter().map(|name| (name.to_string(), rotate("arm"))).collect();
        // No roles: built-in motion would need them, the look's own needs none.
        let request = figure(json!({ "animations": animations, "states": { "idle": ["breathe", "blink", "walk"], "done": ["greet", "wave", "cheer", "nod", "shake", "doze"] } }));
        let applied = accepted(None, request);
        assert_eq!(applied.warnings, [""; 0]);
        assert_eq!(applied.animations.len(), names.len());
        let look = applied.design.look.as_ref().unwrap();
        assert!(names.iter().all(|name| look.animations.contains_key(*name)));
        assert_eq!(look.states[&LookState::Done], ["greet", "wave", "cheer", "nod", "shake", "doze"]);
        // Merged in later by name as well, and named in a state of a look that has the roles.
        let later = accepted(Some(&applied.design), json!({ "pivots": { "body": [100, 190] }, "roles": { "body": "body", "armR": "arm" }, "animations": { "walk": { "duration": 0.6, "loop": true, "tracks": [{ "target": "body", "prop": "rotate", "values": [-3, 3, -3] }] } }, "states": { "idle": ["walk", "breathe"] } }));
        assert_eq!(later.warnings, [""; 0]);
    }

    // ------------------------------------------------------------ parents

    #[test]
    fn parents_tie_a_part_to_another_apart_in_the_drawing_order() {
        // In FIGURE body holds belly, head (face, eyes with pupil, mouth), arm, cane and label.
        let applied = accepted(None, figure(json!({ "parents": { "arm": "head", "cane": "arm" } })));
        let look = applied.design.look.as_ref().unwrap();
        assert_eq!(look.parents, BTreeMap::from([("arm".to_string(), "head".to_string()), ("cane".to_string(), "arm".to_string())]));
        assert_eq!(serde_json::to_value(look).unwrap()["parents"], json!({ "arm": "head", "cane": "arm" }));
        let arm = applied.parts.iter().find(|p| p.id == "arm").unwrap();
        assert_eq!((arm.parent.as_deref(), arm.to_string()), (Some("head"), "arm rect 140 100 12 40 pivot 146 120 (default) parent head".to_string()));
        assert_eq!(applied.usage.parents, 2);
        assert_eq!(&round_trip(look), look);

        for (parents, why) in [
            (json!({ "face": "head" }), "face is inside head and moves with it already (at parents.face): parents are for parts apart in the drawing order"),
            (json!({ "body": "head" }), "head is inside body (at parents.body): a part cannot move with what it holds"),
            (json!({ "arm": "arm" }), "arm moves with itself (at parents.arm): a part moves with another part"),
            (json!({ "arm": "sky" }), "sky is a linearGradient, or inside defs (at parents.arm)"),
            (json!({ "pane": "arm" }), "pane stands inside defs, a mask or a clipPath, where nothing moves (at parents.pane)"),
            (json!({ "nowhere": "arm" }), "target \"nowhere\" not found (at parents.nowhere)"),
            (json!({ "arm": "elsewhere" }), "target \"elsewhere\" not found (at parents.arm)"),
            (json!({ "arm": 5 }), "a parent is a number (at parents.arm): the id of the part it moves with"),
            (json!({ "arm": "cane", "cane": "arm" }), "the parents of arm, cane go round in a circle (at parents.arm): a part moves with no part that moves with it, through parents or what it is drawn inside"),
            (json!({ "arm": "cane", "cane": "label", "label": "arm" }), "go round in a circle"),
            (json!(["arm"]), "parents is a list (at parents): an object of part ids with the id of the part each moves with, null to remove one"),
        ] {
            has(&refused(None, figure(json!({ "parents": parents }))), why);
        }
        // A circle may run through what a part is drawn inside: arm moves with face, face is drawn in
        // head, and head would move with arm.
        has(&refused(None, figure(json!({ "parents": { "head": "arm", "arm": "face" } }))), "the parents of arm, face, head go round in a circle (at parents.arm)");
        has(&refused(None, figure(json!({ "parents": { "head": "cane", "cane": "pupil" } }))), "the parents of cane, pupil, head go round in a circle (at parents.cane)");
        // Moving with a part drawn inside a third is no circle.
        accepted(None, figure(json!({ "parents": { "arm": "pupil", "cane": "mouth" } })));
        // A chain of eight links is fine, nine are not; as many entries as the limit, and no more.
        let circles = |n: usize| svg(&(0..n).map(|i| format!("<circle id='c{i}' cx='100' cy='100' r='5'/>")).collect::<String>());
        let chain = |links: usize| (1..=links).map(|i| (format!("c{i}"), json!(format!("c{}", i - 1)))).collect::<Map<String, Value>>();
        accepted(None, json!({ "svg": circles(10), "parents": chain(MAX_CHAIN) }));
        has(&refused(None, json!({ "svg": circles(10), "parents": chain(MAX_CHAIN + 1) })), "the chain of parents above c9 is longer than 8 (at parents.c9): at most 8 parts above one another");
        let star = |n: usize| (1..=n).map(|i| (format!("c{i}"), json!("c0"))).collect::<Map<String, Value>>();
        assert_eq!(accepted(None, json!({ "svg": circles(MAX_PARENTS + 2), "parents": star(MAX_PARENTS) })).usage.parents, MAX_PARENTS);
        assert_eq!(refused(None, json!({ "svg": circles(MAX_PARENTS + 2), "parents": star(MAX_PARENTS + 1) })), "65 parents (at parents): at most 64 in one look; null removes one");

        // Merged like pivots: by key, null removes one, null for the field removes all.
        let first = applied.design;
        let more = accepted(Some(&first), json!({ "parents": { "label": "arm" } })).design;
        assert_eq!(more.look.as_ref().unwrap().parents.len(), 3);
        let fewer = accepted(Some(&more), json!({ "parents": { "arm": null } })).design;
        assert_eq!(fewer.look.as_ref().unwrap().parents, BTreeMap::from([("cane".to_string(), "arm".to_string()), ("label".to_string(), "arm".to_string())]));
        assert!(accepted(Some(&more), json!({ "parents": null })).design.look.unwrap().parents.is_empty());
        // A merge is checked against the parents that stay: a circle made of an old and a new entry refuses.
        has(&refused(Some(&first), json!({ "parents": { "head": "cane" } })), "go round in a circle");
        // Under new artwork an entry that would close a circle with the others is cleared, not refused.
        let looped = accepted(None, figure(json!({ "parents": { "arm": "face", "cane": "arm" } }))).design;
        let nested = "<svg viewBox='0 0 200 200'><g id='body'><g id='cane'><circle id='face' cx='100' cy='80' r='30'/></g><rect id='arm' x='140' y='100' width='12' height='40'/></g></svg>";
        let kept = accepted(Some(&looped), json!({ "svg": nested, "keep": true }));
        assert_eq!((kept.design.look.as_ref().unwrap().parents.len(), kept.kept.as_slice(), kept.cleared.as_slice()), (1, ["parents arm".to_string()].as_slice(), ["parents cane".to_string()].as_slice()));
        // New artwork: keep keeps an entry whose parts are still apart, clears the others.
        let redrawn = "<svg viewBox='0 0 200 200'><g id='body'><g id='head'><rect id='arm' x='140' y='100' width='12' height='40'/></g><line id='cane' x1='60' y1='120' x2='60' y2='170' stroke='#333333'/></g></svg>";
        let kept = accepted(Some(&first), json!({ "svg": redrawn, "keep": true }));
        assert_eq!(kept.design.look.as_ref().unwrap().parents, BTreeMap::from([("cane".to_string(), "arm".to_string())]));
        assert_eq!((kept.kept.contains(&"parents cane".to_string()), kept.cleared.contains(&"parents arm".to_string())), (true, true), "{:?} {:?}", kept.kept, kept.cleared);
        // Without keep they are cleared, and the answer says so.
        assert!(accepted(Some(&first), json!({ "svg": redrawn })).cleared.contains(&"parents arm, cane".to_string()));
    }

    // ------------------------------------------------------------ drives

    #[test]
    fn drives_move_parts_by_the_live_inputs() {
        let with = |drives: Value| figure(json!({ "pivots": { "head": [100, 110], "body": [100, 190] }, "drives": drives }));
        let drives = json!([
            { "input": "lookX", "target": "head", "prop": "rotate", "values": [-10, 0, 10] },
            { "input": "turn", "target": ["face", "label"], "prop": "opacity", "values": [0.4, 1, 0.4], "at": [0, 0.4, 1], "ease": [0.25, 0.1, 0.25, 1] },
            { "input": "pace", "target": "body", "prop": "skewX", "values": [0, -6], "ease": "outBounce" },
            { "input": "mouth", "target": "mouth", "prop": "d", "values": [null, "M84 96 Q100 120 116 96"] },
            { "input": "held", "target": "face", "prop": "fill", "values": [null, "red"] },
            { "input": "lookY", "target": "pupil", "prop": "translateY", "values": [-3, 3], "ease": ["inOutBack"] }
        ]);
        let applied = accepted(None, with(drives.clone()));
        // The opacity peaks before turn's rest: at rest the label is a little transparent.
        assert_eq!(applied.warnings, ["turn gives label opacity 0.868 at rest, not the artwork's 1 (at drives[1]): an odd number of values with the rest in the middle, or place them with at"]);
        let look = applied.design.look.as_ref().unwrap();
        // Stored in its normal form: colours as hex, path data spaced, the optional keys only where given.
        let stored = serde_json::to_value(&look.drives).unwrap();
        assert_eq!(stored[1], json!({ "input": "turn", "target": ["face", "label"], "prop": "opacity", "values": [0.4, 1, 0.4], "at": [0, 0.4, 1], "ease": [0.25, 0.1, 0.25, 1] }));
        assert_eq!(stored[3], json!({ "input": "mouth", "target": "mouth", "prop": "d", "values": [null, "M 84 96 Q 100 120 116 96"] }));
        assert_eq!(stored[4]["values"], json!([null, "#ff0000"]));
        assert_eq!(look.drives.iter().map(ToString::to_string).collect::<Vec<_>>(), ["lookX moves head rotate", "turn moves face, label opacity", "pace moves body skewX", "mouth moves mouth d", "held moves face fill", "lookY moves pupil translateY"]);
        assert_eq!(applied.usage.drives, 6);
        assert_eq!(&round_trip(look), look);
        assert_eq!(check_stored(serde_json::to_value(&applied.design).unwrap()).as_ref(), Some(&applied.design));

        let one = |drive: Value| refused(None, with(json!([drive])));
        for (drive, why) in [
            (json!({ "input": "gaze", "target": "head", "prop": "rotate", "values": [0, 1] }), "input \"gaze\" (at drives[0]): inputs are lookX, lookY, mouth, pace, turn, held"),
            (json!({ "target": "head", "prop": "rotate", "values": [0, 1] }), "a drive without an input (at drives[0]): inputs are lookX"),
            (json!({ "input": 1, "target": "head", "prop": "rotate", "values": [0, 1] }), "input is a number"),
            (json!({ "input": "lookX", "target": "face", "prop": "opacity", "values": [0, 1] }), "opacity follows lookX (at drives[0]): fill, stroke and the opacities follow only pace, turn and held, which change slowly, so nothing flashes"),
            (json!({ "input": "mouth", "target": "face", "prop": "fill", "values": [null, "red"] }), "fill follows mouth"),
            (json!({ "input": "lookY", "target": "face", "prop": "fillOpacity", "values": [0, 1] }), "fillOpacity follows lookY"),
            (json!({ "input": "turn", "target": "face", "prop": "opacity", "values": [1, 0.5], "ease": "step" }), "ease step on opacity (at drives[0])"),
            (json!({ "input": "held", "target": "face", "prop": "fill", "values": [null, "red", null], "ease": ["linear", "step"] }), "ease step on fill (at drives[0]): fill, stroke and opacity change smoothly so that nothing flashes"),
            (json!({ "input": "pace", "target": "face", "prop": "strokeOpacity", "values": [1, 0.5], "ease": "step" }), "ease step on strokeOpacity"),
            // Nor does it swing, or change more often than a slow input lets a quarter second pass.
            (json!({ "input": "pace", "target": "face", "prop": "opacity", "values": [1, 0.5], "ease": "outBounce" }), "an ease that swings on opacity (at drives[0]): no outElastic or outBounce on fill, stroke or an opacity, and a bezier with y1 and y2 from 0 to 1"),
            (json!({ "input": "held", "target": "face", "prop": "opacity", "values": [1, 0.5], "ease": [0, 3, 1, -2] }), "an ease that swings on opacity"),
            (json!({ "input": "held", "target": "face", "prop": "opacity", "values": [1, 0, 1] }), "3 values of opacity along held stand too close (at drives[0]): 2 values along pace or held, up to 3 along turn, at least 0.34 apart in at"),
            (json!({ "input": "pace", "target": "face", "prop": "fill", "values": ["red", "blue", "red", "blue"] }), "4 values of fill along pace stand too close"),
            (json!({ "input": "turn", "target": "face", "prop": "opacity", "values": [1, 0, 1, 0] }), "4 values of opacity along turn stand too close"),
            (json!({ "input": "turn", "target": "face", "prop": "opacity", "values": [1, 0, 1], "at": [0, 0.2, 1] }), "3 values of opacity along turn stand too close"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [1] }), "1 keyframes (at drives[0]): 2 to 16 values"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate" }), "a drive without values"),
            (json!({ "input": "lookX", "prop": "rotate", "values": [0, 1] }), "a drive without a target"),
            (json!({ "input": "lookX", "target": "head", "values": [0, 1] }), "a drive without a prop"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1, 2], "at": [0, 0.7, 0.5] }), "at does not rise from 0 to 1 (at drives[0].at)"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1, 2], "at": [0, 1] }), "2 places for 3 values (at drives[0]): one place per value"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1, 2], "at": "even" }), "at is a string (at drives[0].at)"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1, 2], "ease": ["in"] }), "1 eases for 2 segments"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [null, 5] }), "null in a rotate drive (at drives[0].values[0])"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 3601] }), "rotate 3601 is out of range"),
            (json!({ "input": "mouth", "target": "mouth", "prop": "d", "values": ["M0 0 L5 5", "M0 0 Q5 5 9 9"] }), "d values differ in structure"),
            (json!({ "input": "pace", "target": "arm", "prop": "cx", "values": [0, 1] }), "arm is a rect and has no cx"),
            (json!({ "input": "pace", "target": "sky", "prop": "rotate", "values": [0, 1] }), "sky is a linearGradient, or inside defs (at drives[0])"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1], "phase": 0.5 }), "\"phase\" is not a key here (at drives[0]): a drive follows its input, not time: at places each value along the input"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1], "times": [0, 1] }), "\"times\" is not a key here (at drives[0]): a drive follows its input"),
            (json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [0, 1], "speed": 2 }), "\"speed\" is not a key here (at drives[0]): keys are input, target, prop, values, at, ease"),
            (json!(5), "a drive is a number (at drives[0]): an object with input, target, prop and values"),
        ] {
            has(&one(drive), why);
        }
        has(&refused(None, with(json!({ "input": "lookX" }))), "drives is an object (at drives): a list of up to 16 drives, [] or null for none");
        // As many as the limit, and no more.
        let many = |n: usize| json!(vec![json!({ "input": "lookX", "target": "head", "prop": "rotate", "values": [-5, 5] }); n]);
        assert_eq!(accepted(None, with(many(MAX_DRIVES))).usage.drives, MAX_DRIVES);
        assert_eq!(refused(None, with(many(MAX_DRIVES + 1))), "drives is a list of 17 (at drives): a list of up to 16 drives, [] or null for none");
        // A rotate without a pivot warns as a track does.
        assert_eq!(accepted(None, figure(json!({ "drives": [{ "input": "lookX", "target": "eyes", "prop": "rotate", "values": [-5, 5] }] }))).warnings, ["eyes has no pivot: it turns and scales about the centre of its box, [90, 75] (at drives[0]): give it a pivot if its joint is elsewhere"]);

        // The list replaces; null and [] remove it; a call that does not name it keeps it.
        let first = applied.design;
        assert_eq!(accepted(Some(&first), json!({ "drives": [drives[0]] })).design.look.unwrap().drives.len(), 1);
        assert!(accepted(Some(&first), json!({ "drives": null })).design.look.unwrap().drives.is_empty());
        assert!(accepted(Some(&first), json!({ "drives": [] })).design.look.unwrap().drives.is_empty());
        assert_eq!(accepted(Some(&first), json!({ "gaze": 2 })).design.look.unwrap().drives.len(), 6);
        // New artwork: keep keeps every drive whose targets still exist and fit, clears the others.
        let redrawn = "<svg viewBox='0 0 200 200'><g id='body'><g id='head'><circle id='face' cx='100' cy='80' r='30' fill='#ffcc00'/><path id='mouth' d='M84 96 Q100 104 116 96'/></g></g></svg>";
        let kept = accepted(Some(&first), json!({ "svg": redrawn, "keep": true }));
        assert_eq!(kept.design.look.as_ref().unwrap().drives.iter().map(ToString::to_string).collect::<Vec<_>>(), ["lookX moves head rotate", "pace moves body skewX", "mouth moves mouth d", "held moves face fill"]);
        assert!(kept.kept.contains(&"drives".to_string()) && kept.cleared.contains(&"drives".to_string()), "{:?} {:?}", kept.kept, kept.cleared);
        assert!(accepted(Some(&first), json!({ "svg": redrawn })).cleared.contains(&"drives".to_string()));
    }

    #[test]
    fn the_new_wire_types_have_the_shapes_of_the_contract() {
        let look = accepted(None, figure(json!({ "parents": { "arm": "head" }, "drives": [{ "input": "lookX", "target": "arm", "prop": "rotate", "values": [-5, 5] }] }))).design.look.unwrap();
        let v = serde_json::to_value(&look).unwrap();
        assert_eq!(v.as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(), ["animations", "drives", "gaze", "mouth", "nodes", "parents", "pivots", "roles", "states", "viewBox"].into());
        assert_eq!((&v["parents"], &v["drives"]), (&json!({ "arm": "head" }), &json!([{ "input": "lookX", "target": "arm", "prop": "rotate", "values": [-5, 5] }])));
        assert_eq!(serde_json::from_value::<Look>(v.clone()).unwrap(), look);
        // Absent or null they are empty, and an empty one is not written.
        for empty in [json!(null), json!([])] {
            let mut absent = v.clone();
            absent.as_object_mut().unwrap().remove("parents");
            absent["drives"] = empty;
            let read = serde_json::from_value::<Look>(absent).unwrap();
            assert!(read.parents.is_empty() && read.drives.is_empty());
            assert!(serde_json::to_value(&read).unwrap().get("drives").is_none());
        }
        // Nothing unknown gets into a drive; an input is one of six names.
        let mut tampered = v.clone();
        tampered["drives"][0]["phase"] = json!(0.5);
        assert!(serde_json::from_value::<Look>(tampered).is_err());
        let mut tampered = v.clone();
        tampered["drives"][0]["input"] = json!("gaze");
        assert!(serde_json::from_value::<Look>(tampered).is_err());
        for input in Input::ALL {
            assert_eq!(serde_json::to_value(input).unwrap(), json!(input.name()));
        }
        assert_eq!(Input::ALL.map(Input::name), ["lookX", "lookY", "mouth", "pace", "turn", "held"]);
        // An ease: a name, four numbers, or a list of those.
        for (written, read) in [(json!("inBack"), Eases::One(Curve::Named(Ease::InBack))), (json!([0, 0.5, 1, 1]), Eases::One(Curve::Bezier([0.0, 0.5, 1.0, 1.0].map(Num::new)))), (json!([[0, 0, 1, 1], "out"]), Eases::Each(vec![Curve::Bezier([0.0, 0.0, 1.0, 1.0].map(Num::new)), Curve::Named(Ease::Out)]))] {
            assert_eq!(serde_json::from_value::<Eases>(written.clone()).unwrap(), read);
            assert_eq!(serde_json::to_value(&read).unwrap(), written);
        }
        // The new tags and props on the wire.
        for (tag, name) in [(Tag::Use, "use"), (Tag::Mask, "mask"), (Tag::Filter, "filter"), (Tag::FeGaussianBlur, "feGaussianBlur"), (Tag::FeDropShadow, "feDropShadow"), (Tag::FeMergeNode, "feMergeNode"), (Tag::FeDisplacementMap, "feDisplacementMap")] {
            assert_eq!(serde_json::to_value(tag).unwrap(), json!(name));
        }
        for (prop, name) in [(Prop::SkewX, "skewX"), (Prop::SkewY, "skewY"), (Prop::FillOpacity, "fillOpacity"), (Prop::StrokeOpacity, "strokeOpacity"), (Prop::StrokeDashoffset, "strokeDashoffset")] {
            assert_eq!(serde_json::to_value(prop).unwrap(), json!(name));
        }
    }

    #[test]
    fn a_stored_use_mask_filter_parent_or_drive_is_believed_in_nothing() {
        let design = accepted(
            None,
            json!({
                "svg": svg("<defs><g id='dot'><circle cx='5' cy='5' r='4'/></g><mask id='m' mask-type='luminance'><rect width='200' height='200' fill='#ffffff'/></mask><filter id='f'><feGaussianBlur stdDeviation='2' result='b'/><feMerge><feMergeNode in='b'/><feMergeNode in='SourceGraphic'/></feMerge></filter><clipPath id='cp'><circle r='9'/></clipPath></defs><g id='body' mask='url(#m)' filter='url(#f)' stroke='#000000' stroke-dasharray='2 1'><use id='a' href='#dot' x='50' y='50'/><use id='b' href='#dot' x='80' y='50'/></g>"),
                "parents": { "b": "a" },
                "drives": [{ "input": "turn", "target": "a", "prop": "opacity", "values": [0.5, 1, 0.5] }]
            }),
        )
        .design;
        let stored = serde_json::to_value(&design).unwrap();
        assert_eq!(check_stored(stored.clone()), Some(design.clone()));
        let (defs, body) = ("/look/nodes/0", "/look/nodes/1");
        assert_eq!((&stored["look"]["nodes"][1]["id"], &stored["look"]["nodes"][1]["children"][0]["id"]), (&json!("body"), &json!("a")));
        let tamper = |path: &str, value: Value| {
            let mut v = stored.clone();
            put(&mut v, path, value);
            check_stored(v)
        };
        for (path, value) in [
            (format!("{body}/children/0/attrs/useRef"), json!("nope")),
            (format!("{body}/children/0/attrs/useRef"), json!("m")),
            (format!("{body}/children/0/attrs/useRef"), json!("b")),
            (format!("{body}/children/0/attrs/useRef"), json!("dot' onload='alert(1)")),
            (format!("{body}/children/0/attrs/useRef"), json!("http://x/a.svg#dot")),
            (format!("{body}/children/0/attrs/href"), json!("#dot")),
            (format!("{body}/children/0/attrs/width"), json!(10)),
            (format!("{body}/children/0/children"), json!([{ "tag": "circle", "attrs": { "r": 5 } }])),
            (format!("{body}/attrs/maskRef"), json!("cp")),
            (format!("{body}/attrs/maskRef"), json!("f")),
            (format!("{body}/attrs/filterRef"), json!("m")),
            (format!("{body}/attrs/filterRef"), json!("blur(2px)")),
            (format!("{body}/attrs/filter"), json!("url(#f)")),
            (format!("{body}/attrs/strokeDasharray"), json!("-1 2")),
            (format!("{body}/attrs/strokeDasharray"), json!("1 1 1 1 1 1 1 1 1")),
            (format!("{body}/attrs/strokeDashoffset"), json!(1e9)),
            (format!("{defs}/children/2/children/0/attrs/stdDeviation"), json!("999")),
            (format!("{defs}/children/2/children/0/attrs/in"), json!("BackgroundImage")),
            (format!("{defs}/children/2/children/0/attrs/result"), json!("SourceGraphic")),
            (format!("{defs}/children/2/children/1/children/0/attrs/in"), json!("later")),
            (format!("{defs}/children/2/children/0/tag"), json!("feImage")),
            (format!("{defs}/children/2/children/0/tag"), json!("feTile")),
            (format!("{defs}/children/2/children/0/attrs/href"), json!("http://x/a.png")),
            (format!("{defs}/children/1/attrs/maskType"), json!("colour")),
            (format!("{defs}/children/1/children/0/attrs/maskRef"), json!("m")),
            (format!("{defs}/children/1/children/0/tag"), json!("text")),
            (format!("{defs}/children/0/tag"), json!("mask")),
            ("/look/parents/b".into(), json!("nowhere")),
            ("/look/parents/a".into(), json!("b")),
            ("/look/parents/body".into(), json!("a")),
            ("/look/parents".into(), json!(["a"])),
            ("/look/drives/0/input".into(), json!("lookX")),
            ("/look/drives/0/input".into(), json!("gaze")),
            ("/look/drives/0/target".into(), json!("dot")),
            ("/look/drives/0/values".into(), json!([0.5, 2])),
            ("/look/drives/0/at".into(), json!([0, 1])),
            ("/look/drives/0/ease".into(), json!([2, 0, 1, 1])),
            ("/look/drives/0/phase".into(), json!(0.5)),
            ("/look/drives".into(), json!({})),
        ] {
            assert_eq!(tamper(&path, value.clone()), None, "{path} = {value}");
        }
        // The boxes of what a use repeats are computed again as well.
        assert_eq!(tamper(&format!("{body}/children/0/box"), json!([0, 0, 1, 1])), Some(design.clone()));
        assert_eq!(tamper("/look/drives/0/ease", json!([0.25, 0.1, 0.25, 1])).map(|d| d.look.unwrap().drives[0].ease.clone()), Some(Some(Eases::One(Curve::Bezier([0.25, 0.1, 0.25, 1.0].map(Num::new))))));
    }

    /// One call with every piece of artwork and rig this file took on last: what a guide shows.
    #[test]
    fn a_compact_design_with_use_mask_filter_dashes_parents_and_drives_has_no_warning() {
        let call = json!({
            "name": "Wisp",
            "svg": "<svg viewBox='0 0 200 200'><defs><g id='eye'><circle r='7' fill='#ffffff'/><circle r='3' fill='#222222'/></g><mask id='cut'><rect x='0' y='0' width='200' height='130' fill='#ffffff'/></mask><filter id='glow'><feGaussianBlur in='SourceGraphic' stdDeviation='3' result='soft'/><feMerge><feMergeNode in='soft'/><feMergeNode in='SourceGraphic'/></feMerge></filter></defs><g id='body' filter='url(#glow)'><circle cx='100' cy='110' r='50' fill='#7cc4ff' mask='url(#cut)'/><path d='M60 130 Q100 150 140 130' fill='none' stroke='#2b6cb0' stroke-width='3' stroke-dasharray='6 4'/></g><g id='eyes'><use id='eyeL' href='#eye' x='82' y='100'/><use id='eyeR' href='#eye' x='118' y='100'/></g><path id='tail' d='M100 160 Q120 180 100 190' fill='none' stroke='#7cc4ff' stroke-width='6' stroke-linecap='round'/></svg>",
            "pivots": { "body": [100, 160], "eyes": [100, 100], "tail": [100, 160] },
            "parents": { "eyes": "body", "tail": "body" },
            "roles": { "body": "body", "eyes": "eyes" },
            "animations": { "bob": { "duration": 2, "loop": true, "ease": [0.45, 0, 0.55, 1], "tracks": [
                { "target": "body", "prop": "translateY", "values": [0, -6, 0] },
                { "target": "body", "prop": "skewX", "values": [0, 3, 0], "ease": "inOutBack" }
            ] } },
            "states": { "idle": ["bob", "blink"] },
            "drives": [
                { "input": "lookX", "target": "eyes", "prop": "rotate", "values": [-8, 0, 8] },
                { "input": "turn", "target": "tail", "prop": "opacity", "values": [0.4, 1, 0.4], "at": [0, 0.5, 1], "ease": [0.25, 0.1, 0.25, 1] }
            ]
        });
        let applied = accepted(None, call);
        assert_eq!(applied.warnings, [""; 0]);
        let look = applied.design.look.as_ref().unwrap();
        assert_eq!((look.parents.len(), look.drives.len(), bbox(look, "eyeL")), (2, 2, [75.0, 93.0, 14.0, 14.0]));
        assert_eq!(&round_trip(look), look);
    }
}

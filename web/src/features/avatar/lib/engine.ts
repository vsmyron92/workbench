// The animation engine: a look's keyframe tracks, sampled into attribute values.
//
// Three layers, each usable without the next:
// - `compile` turns every track of a look (the agent's and the built-in ones) and every
//   drive into numbers: colours into RGBA, path data into a command template plus numbers.
//   A value that does not fit its prop rejects the look.
// - `sample` is pure: clips and inputs in, the pose of every part out. All of the animation
//   rules live here (composition, weights, gestures over state animations, fades, drives,
//   mouth, gaze, and the skeleton of `parents`). It is two pure steps: `accumulate` adds
//   everything up into numbers per part, `settle` turns the numbers into what is written.
// - `createEngine` runs one shared frame loop per window and writes poses to nodes. It
//   counts visible time only, runs at most 30 frames a second, catches per figure, and
//   freezes a figure whose frames cost too much. Between the two steps of each frame it puts
//   a limiter (`REVERSAL_MS`): whatever the tracks, gestures, drives and inputs make
//   together, no channel it writes turns back more often than three flashes a second allow.
//
// The engine never measures the DOM and never writes a string from the look: everything it
// writes is formatted here from finite numbers, plus a few literal words. The one thing it
// writes that is neither: `url(#<prefix><id>)`, when a part painted with a gradient returns
// to it after a colour track. The id names a gradient in the look's index, which holds only
// ids that passed `isId`; the prefix is the one the figure was rendered with, cleaned by
// `cleanIdPrefix`.
//
// React-free; the frame scheduler, the clock and randomness are passed in.

import { builtinAnimations, gazeTravel, rolePart, stateList } from './builtin'
import {
  IDENTITY,
  LIMITS,
  MAX_CHAIN_LINKS,
  MORE_LIMITS,
  boxThrough,
  cleanIdPrefix,
  fmt,
  formatPath,
  formatTransform,
  invert,
  isColourProp,
  isEase,
  isId,
  isPaintProp,
  isTransformProp,
  lookIndex,
  multiply,
  opMatrix,
  ownTransform,
  parseHex,
  parsePath,
  parseTransform,
  propFits,
  samePathShape,
  trackEases,
  trackTimes,
  transformMatrix,
  viewBoxOf,
} from './look'
import type { IndexEntry, Matrix, PathShape, Rgba } from './look'
import { DRIVE_INPUTS } from './types'
import type { Animation, AvatarState, Bezier, Box, Drive, DriveInput, Ease, Look, LookNode, MouthDrive, Prop, Track } from './types'

// ---------------------------------------------------------------- public shapes

/** A look ready to be sampled. Made by `compile` only. */
export interface Compiled {
  readonly look: Look
}

/** One animation that is playing. */
export interface Clip {
  /** An animation of the look, or a built-in. A name that means nothing is ignored. */
  name: string
  /** When it started, on the clock `sample` is called with. */
  startMs: number
  /** 0 to 1: how much of it applies (a state animation fading in or out, a gesture being stopped). */
  weight: number
  /**
   * `state`: part of the state's list (loops loop, recurring ones play one pass from
   * `startMs`, one-shots play once). `once`, `hold`, `loop`: a gesture, whose values
   * replace the state animations' on the props it animates.
   */
  mode: 'state' | 'once' | 'hold' | 'loop'
  /** `once`: how many passes (1 to 5). */
  times: number
}

/** What one part looks like at one moment. */
export interface Pose {
  /** The whole `transform` attribute; absent: the part has none. */
  transform?: string
  /** DOM attribute name -> value; null: remove the attribute (the artwork has none). */
  attrs: Map<string, string | null>
  /**
   * DOM attribute name -> the id of the gradient the artwork paints it with. The part is at
   * rest there: the attribute is `url(#<id prefix><id>)`, as it was rendered. A paint that a
   * track is colouring right now is in `attrs` instead. Absent: the part has no such paint.
   */
  refs?: Map<string, string>
}

/**
 * The live inputs of one moment. Without them nothing but the clips applies (the rest pose
 * when nothing plays); with them the look's drives apply too, each input that is not given
 * at rest.
 */
export interface SampleInputs {
  /** How open the mouth is, 0 to 1. null: no line is presented, and the look's `mouth` entries are not applied. */
  mouth?: number | null
  /** Where it looks, a vector of length 0 to 1 (x to the right, y down): the pupils and `lookX`, `lookY`. null: ahead. */
  gaze?: readonly [number, number] | null
  /** 0 standing to 1 walking fast. */
  pace?: number
  /** -1 walking or facing left to 1 right, 0 facing the viewer. */
  turn?: number
  /** 0 to 1 while the user drags it. */
  held?: number
}

/** What the walking overlay tells a figure about its walk; what it does not say is 0. */
export interface WalkInputs {
  pace?: number
  turn?: number
  held?: number
}

export interface SampleOptions {
  /** No looping or recurring animations, no gaze, every input but the mouth at rest; gestures and one-shots show their last frame. */
  reducedMotion?: boolean
}

export interface EngineNode {
  setAttribute(name: string, value: string): void
  removeAttribute(name: string): void
}

export interface EngineHost {
  requestFrame(cb: (tMs: number) => void): number
  cancelFrame(id: number): void
  /** 0 to 1, like `Math.random`. */
  random(): number
  /** Milliseconds, like `performance.now`: only used to time a figure's own frames. */
  now(): number
}

export interface FigureHandle {
  setState(s: AvatarState): void
  /** Plays an animation as a gesture. false: the name means nothing for this look, or the figure no longer animates. */
  play(name: string, mode?: 'once' | 'hold' | 'loop', times?: number): boolean
  stop(name: string | 'all'): void
  /** `read` gives the openness wanted now, 0 to 1, or a negative number while no line of this avatar is presented. */
  setMouth(read: (() => number) | null): void
  /**
   * `read` gives where the pupils should look, as a vector of length 0 to 1 (x to the
   * right, y down), or null to let the figure glance about by itself.
   */
  setGaze(read: (() => readonly [number, number] | null) | null): void
  /** `read` gives the figure's walk (`pace`, `turn`, `held`), read every frame; null or nothing given: all at rest. */
  setInputs(read: (() => WalkInputs | null) | null): void
  setReducedMotion(on: boolean): void
  /** The figure stopped animating: its frames cost too much, or one of its nodes threw. It stays in its rest pose, with what its drives set at rest (as a still face). */
  frozen(): boolean
  /** Puts every part back to the artwork's own values and leaves the loop. */
  detach(): void
}

export interface Engine {
  /**
   * `find` is asked for each part by the look's own id, once. `idPrefix`: what the figure's
   * ids were rendered with (`lookElements(look, idPrefix)`), so a part painted with a
   * gradient can be given it back after a colour track; without it such a paint is never
   * touched.
   */
  attach(look: Look, find: (id: string) => EngineNode | null, opts: { state: AvatarState; reducedMotion: boolean; idPrefix?: string }): FigureHandle
  /** false while the tab is hidden or no figure is on screen: the loop stops and time stands still. */
  setRunning(on: boolean): void
  dispose(): void
}

// ---------------------------------------------------------------- constants

/** A state animation fades in and out over this, and so does a finished one-shot's leftover pose. */
export const STATE_FADE_MS = 200
/** A gesture's values replace the state's over this, and give way again over this. */
export const GESTURE_BLEND_MS = 150
/** The mouth follows its target with this time constant, and stays applied this long after a line. */
export const MOUTH_EASE_MS = 60
export const MOUTH_TAIL_MS = 120
/** `pace`, `turn` and `held` move at most this much a second (flash safety: a full swing of a paint takes 0.25 s or more). */
export const INPUT_RATE = 4
/** At most 30 frames a second. A little under 1000 / 30, so a 60 Hz display gives every second frame. */
const FRAME_MS = 32
/** A frame never counts for more than this: a tab that was away does not fast-forward. */
const MAX_STEP_MS = 250
/** A figure whose frames average over this many milliseconds across this many frames is frozen. */
export const BUDGET_MS = 4
export const BUDGET_FRAMES = 30
/** A held or looping gesture ends by itself after this (the server's HOLD_MAX_SECS). */
const HOLD_MAX_MS = 120_000
const MAX_GESTURES = 6
/** The pupils follow where they are told to look with this time constant. */
export const GAZE_EASE_MS = 90
/** A correction of `parents` this close to the identity is none. */
const NEAR_IDENTITY = 1e-9

// ---------------------------------------------------------------- compiled form

type Kind = 'rel' | 'num' | 'colour' | 'd'

/** The keyframes of a track for one target, nulls resolved. Exactly one of the lists is used, by kind. */
interface Frames {
  nums: readonly number[]
  colours: readonly Rgba[]
  paths: readonly (readonly number[])[]
}

/**
 * The channels a relative prop feeds: 0 tx, 1 ty, 2 rotation, 3 skew x, 4 skew y (these add),
 * 5 sx, 6 sy (these multiply).
 */
const SCALE_FROM = 5
const REST_CHANNELS: readonly number[] = [0, 0, 0, 0, 0, 1, 1]

interface CTrack {
  prop: Prop
  kind: Kind
  channels: readonly number[]
  times: readonly number[]
  eases: readonly Ease[]
  shape: PathShape | null
  phase: number
  delayMs: number
  /** Target id -> its keyframes. */
  frames: ReadonlyMap<string, Frames>
}

interface CAnim {
  durationMs: number
  loop: boolean
  /** Seconds between starts of a recurring animation, [min, max]; null: it is a loop or a one-shot. */
  every: readonly [number, number] | null
  tracks: readonly CTrack[]
  maxDelayMs: number
}

/** A drive: a track sampled at its input's position instead of at a moment. */
interface CDrive {
  input: DriveInput
  track: CTrack
}

/**
 * What an absolute prop is when nothing animates it. `text`: what to write back; null: remove
 * the attribute. `ref`: the paint is this gradient of the look, and that is what is put back.
 */
type Rest = { kind: 'num'; value: number; text: string | null } | { kind: 'colour'; rgba: Rgba | null; fromNothing: boolean; text: string | null; ref?: string } | { kind: 'd'; shape: PathShape | null; nums: readonly number[]; text: string | null }

interface CTarget {
  /** The element's own transform, written back from its numbers ("" when it has none). */
  own: string
  pivotX: string
  pivotY: string
  negX: string
  negY: string
  /** The pivot as the numbers that are written. */
  px: number
  py: number
  /** Absolute props something may animate on this part, with the artwork's own value. */
  rest: Map<Prop, Rest>
  /** Whether something may change its transform. */
  moves: boolean
}

type CMouth = { target: string; kind: 'd'; shape: PathShape; closed: readonly number[]; open: readonly number[] } | { target: string; kind: 'channel'; channels: readonly number[]; closed: number; open: number } | { target: string; kind: 'attr'; prop: Prop; closed: number; open: number }

/** The `parents` of a look: what each child moves with, and how its parent rests. */
interface Skeleton {
  /** Child -> parent. */
  parents: ReadonlyMap<string, string>
  /** Parent -> its rest world undone, R(Q)⁻¹; null when it cannot be undone (its children are left out). */
  restInverse: ReadonlyMap<string, Matrix | null>
  /** Child -> where the element it is drawn in rests, Rp = S(A1)·…·S(An) (no motion). */
  restOuter: ReadonlyMap<string, Matrix>
}

interface Full {
  anims: ReadonlyMap<string, CAnim>
  targets: ReadonlyMap<string, CTarget>
  drives: readonly CDrive[]
  /** A drive reads where it looks, or how open its mouth is: the engine keeps those up even without pupils or a `mouth` entry. */
  readsLook: boolean
  readsMouth: boolean
  skeleton: Skeleton | null
  mouth: readonly CMouth[]
  gazeTarget: string | null
  gazeTravel: number
  /** The parts that can cover much of the figure (`largeParts`): the frame loop limits every channel of theirs, not only their paint. */
  large: ReadonlySet<string>
  /** The pose with nothing playing: the artwork's own values, as the engine writes them back. */
  rest: ReadonlyMap<string, Pose>
}

const FULL = new WeakMap<Compiled, Full>()
const COMPILED = new WeakMap<Look, Compiled | null>()

/** Prop -> the DOM attribute the engine writes. A literal table: no attribute name ever comes from a look. */
const DOM_ATTR: ReadonlyMap<Prop, string> = new Map<Prop, string>([
  ['opacity', 'opacity'],
  ['fillOpacity', 'fill-opacity'],
  ['strokeOpacity', 'stroke-opacity'],
  ['strokeDashoffset', 'stroke-dashoffset'],
  ['fill', 'fill'],
  ['stroke', 'stroke'],
  ['strokeWidth', 'stroke-width'],
  ['d', 'd'],
  ['cx', 'cx'],
  ['cy', 'cy'],
  ['r', 'r'],
  ['rx', 'rx'],
  ['ry', 'ry'],
  ['x', 'x'],
  ['y', 'y'],
  ['width', 'width'],
  ['height', 'height'],
  ['x1', 'x1'],
  ['y1', 'y1'],
  ['x2', 'x2'],
  ['y2', 'y2'],
])

const CHANNELS: ReadonlyMap<Prop, readonly number[]> = new Map<Prop, readonly number[]>([
  ['translateX', [0]],
  ['translateY', [1]],
  ['rotate', [2]],
  ['skewX', [3]],
  ['skewY', [4]],
  ['scale', [5, 6]],
  ['scaleX', [5]],
  ['scaleY', [6]],
])

const INPUTS: ReadonlySet<string> = new Set(DRIVE_INPUTS)
/** The inputs the engine moves slowly: the only ones that may drive a paint. */
const SLOW_INPUTS: ReadonlySet<DriveInput> = new Set<DriveInput>(['pace', 'turn', 'held'])
/** The props whose artwork value is inherited from the elements around a part when it sets none. */
const INHERITED: ReadonlySet<Prop> = new Set<Prop>(['strokeWidth', 'fillOpacity', 'strokeOpacity', 'strokeDashoffset'])

const clamp01 = (n: number) => (n < 0 ? 0 : n > 1 ? 1 : n)
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v)

const C1 = 1.70158
const C2 = C1 * 1.525
const C3 = C1 + 1
const BOUNCE_N = 7.5625
const BOUNCE_D = 2.75

/**
 * A cubic bezier from (0, 0) to (1, 1) through `(x1, y1)` and `(x2, y2)`, at `t`: x(s) = t is
 * solved for s by 8 Newton steps from s = t, or by 20 bisection steps when the slope gets
 * under 1e-6; the value is y(s).
 */
export function bezier(b: Bezier, t: number): number {
  const [x1, y1, x2, y2] = b
  const curve = (s: number, p1: number, p2: number) => 3 * (1 - s) * (1 - s) * s * p1 + 3 * (1 - s) * s * s * p2 + s * s * s
  const slope = (s: number) => 3 * (1 - s) * (1 - s) * x1 + 6 * (1 - s) * s * (x2 - x1) + 3 * s * s * (1 - x2)
  // Newton from s = t; a step that leaves 0 … 1, a flat slope or a result that is still off
  // hands over to bisection, which always lands (x rises with s).
  let s = t
  let newton = true
  for (let i = 0; i < 8; i++) {
    const d = slope(s)
    if (Math.abs(d) < 1e-6) {
      newton = false
      break
    }
    s -= (curve(s, x1, x2) - t) / d
    if (!(s >= 0 && s <= 1)) {
      newton = false
      break
    }
  }
  if (!newton || Math.abs(curve(s, x1, x2) - t) > 1e-5) {
    let lo = 0
    let hi = 1
    for (let i = 0; i < 30; i++) {
      const mid = (lo + hi) / 2
      if (curve(mid, x1, x2) < t) lo = mid
      else hi = mid
    }
    s = (lo + hi) / 2
  }
  return curve(s, y1, y2)
}

/**
 * One eased step of a segment, 0 to 1 in. `outBack`, `inBack`, `inOutBack` and
 * `outElastic` overshoot; `outBounce` bounces; `step` holds the start until the end; four
 * numbers are a cubic bezier.
 */
export function ease(kind: Ease, t: number): number {
  if (typeof kind !== 'string') return Array.isArray(kind) ? bezier(kind, t) : t
  switch (kind) {
    case 'in':
      return t * t
    case 'out':
      return 1 - (1 - t) * (1 - t)
    case 'inOut':
      return t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2
    case 'outBack': {
      const u = t - 1
      return 1 + C3 * u * u * u + C1 * u * u
    }
    case 'inBack':
      return C3 * t * t * t - C1 * t * t
    case 'inOutBack':
      return t < 0.5 ? ((2 * t) ** 2 * ((C2 + 1) * 2 * t - C2)) / 2 : ((2 * t - 2) ** 2 * ((C2 + 1) * (2 * t - 2) + C2) + 2) / 2
    case 'outElastic':
      if (t === 0) return 0
      if (t === 1) return 1
      return 2 ** (-10 * t) * Math.sin((10 * t - 0.75) * ((2 * Math.PI) / 3)) + 1
    case 'outBounce': {
      if (t < 1 / BOUNCE_D) return BOUNCE_N * t * t
      if (t < 2 / BOUNCE_D) return BOUNCE_N * (t - 1.5 / BOUNCE_D) ** 2 + 0.75
      if (t < 2.5 / BOUNCE_D) return BOUNCE_N * (t - 2.25 / BOUNCE_D) ** 2 + 0.9375
      return BOUNCE_N * (t - 2.625 / BOUNCE_D) ** 2 + 0.984375
    }
    case 'step':
      return t >= 1 ? 1 : 0
    default:
      return t
  }
}

/** The mouth's openness one frame later: it follows its target, faster the longer the frame took. */
export function easeMouth(open: number, target: number, dtMs: number): number {
  if (!isNum(open) || !isNum(target)) return 0
  if (!isNum(dtMs) || dtMs <= 0) return open
  return open + (target - open) * (1 - Math.exp(-dtMs / MOUTH_EASE_MS))
}

/** `pace`, `turn` or `held` one frame later: towards its target, at most `INPUT_RATE` a second. */
export function easeInput(now: number, target: number, dtMs: number): number {
  if (!isNum(now) || !isNum(target)) return 0
  if (!isNum(dtMs) || dtMs <= 0) return now
  const most = (INPUT_RATE * dtMs) / 1000
  return now + Math.min(Math.max(target - now, -most), most)
}

function colourText(c: Rgba): string | null {
  const r = fmt(Math.round(Math.min(Math.max(c[0], 0), 255)), 0)
  const g = fmt(Math.round(Math.min(Math.max(c[1], 0), 255)), 0)
  const b = fmt(Math.round(Math.min(Math.max(c[2], 0), 255)), 0)
  const a = fmt(clamp01(c[3]), 3)
  if (r === null || g === null || b === null || a === null) return null
  return a === '1' ? `rgb(${r},${g},${b})` : `rgba(${r},${g},${b},${a})`
}

/** A value read from a record that came from data: own keys only. */
function own(recordLike: unknown, key: string): unknown {
  if (typeof recordLike !== 'object' || recordLike === null || !Object.hasOwn(recordLike, key)) return undefined
  return (recordLike as Record<string, unknown>)[key]
}

/** A percentage ("50%") or a number, as a number. */
function numeric(value: unknown): number | null {
  if (isNum(value)) return value
  if (typeof value === 'string' && /^-?\d{1,4}(?:\.\d{1,3})?%$/.test(value)) return Number(value.slice(0, -1)) / 100
  return null
}

/** What an absolute numeric prop is when the artwork does not set it. */
function numberDefault(prop: Prop): number {
  return prop === 'opacity' || prop === 'strokeWidth' || prop === 'fillOpacity' || prop === 'strokeOpacity' ? 1 : 0
}

/** The nearest element, from the part outwards, that sets one of these attributes. */
function inherited(entry: IndexEntry, keys: readonly string[]): { key: string; value: unknown } | null {
  const chain: LookNode[] = [entry.node, ...[...entry.ancestors].reverse()]
  for (const node of chain) {
    for (const key of keys) {
      const value = own(node.attrs, key)
      if (value !== undefined) return { key, value }
    }
  }
  return null
}

/**
 * The artwork's own value of an absolute prop on a part; null when the engine could not put
 * it back (one of the app's token paints, or a reference that names no gradient of the look).
 */
function restOf(index: ReadonlyMap<string, IndexEntry>, entry: IndexEntry, prop: Prop): Rest | null {
  if (prop === 'd') {
    const shape = parsePath(own(entry.node.attrs, 'd'))
    if (!shape) return { kind: 'd', shape: null, nums: [], text: null }
    return { kind: 'd', shape, nums: shape.nums, text: formatPath(shape, shape.nums) }
  }
  if (isColourProp(prop)) {
    const ref = prop === 'fill' ? 'fillRef' : 'strokeRef'
    const token = prop === 'fill' ? 'fillToken' : 'strokeToken'
    if (own(entry.node.attrs, token) !== undefined) return null
    const painted = own(entry.node.attrs, ref)
    if (painted !== undefined) {
      // Painted with a gradient: a track's colours play over it (there is no colour to blend from, so
      // they start and end at once), and the part is given its gradient back afterwards.
      // The index holds only ids that passed `isId`, so what names a gradient there is safe to write in a reference.
      if (typeof painted !== 'string') return null
      const tag = index.get(painted)?.tag
      return tag === 'linearGradient' || tag === 'radialGradient' ? { kind: 'colour', rgba: null, fromNothing: false, text: null, ref: painted } : null
    }
    const mine = own(entry.node.attrs, prop)
    if (mine === 'none') return { kind: 'colour', rgba: null, fromNothing: true, text: 'none' }
    if (mine === 'currentColor') return { kind: 'colour', rgba: null, fromNothing: false, text: 'currentColor' }
    const rgba = parseHex(mine)
    if (rgba) return { kind: 'colour', rgba, fromNothing: false, text: colourText(rgba) }
    // Not set here: it is inherited, and removing the attribute brings that back.
    const from = inherited(entry, [prop, ref, token])
    if (!from) return prop === 'fill' ? { kind: 'colour', rgba: [0, 0, 0, 1], fromNothing: false, text: null } : { kind: 'colour', rgba: null, fromNothing: true, text: null }
    if (from.key === prop && from.value === 'none') return { kind: 'colour', rgba: null, fromNothing: true, text: null }
    return { kind: 'colour', rgba: from.key === prop ? parseHex(from.value) : null, fromNothing: false, text: null }
  }
  const mine = numeric(own(entry.node.attrs, prop))
  if (mine !== null) return { kind: 'num', value: mine, text: fmt(mine, 3) }
  const from = INHERITED.has(prop) ? inherited(entry, [prop]) : null
  return { kind: 'num', value: (from ? numeric(from.value) : null) ?? numberDefault(prop), text: null }
}

function nearest<T>(values: readonly (T | null)[], at: number): T | null {
  for (let d = 1; d < values.length; d++) {
    const before = values[at - d]
    if (before !== undefined && before !== null) return before
    const after = values[at + d]
    if (after !== undefined && after !== null) return after
  }
  return null
}

/** An element's own transform as a matrix (the identity when it has none). Read once per node. */
const OWN_MATRIX = new WeakMap<LookNode, Matrix>()
function ownMatrix(node: LookNode): Matrix {
  let m = OWN_MATRIX.get(node)
  if (!m) {
    const ops = ownTransform(node)
    m = ops ? transformMatrix(ops) : IDENTITY
    OWN_MATRIX.set(node, m)
  }
  return m
}

/** Thrown inside `compile`; the look is rejected. */
class Misfit extends Error {}
const misfit = (): never => {
  throw new Misfit()
}

interface Builder {
  look: Look
  index: ReadonlyMap<string, IndexEntry>
  bodyId: string | null
  targets: Map<string, CTarget>
}

/** The part an animation, a drive, a parent, the mouth or the gaze names: it must be drawn (not a gradient, a clip path, a mask, a filter or what is inside one). */
function partOf(b: Builder, id: unknown): IndexEntry {
  const entry = isId(id) ? b.index.get(id) : undefined
  if (!entry || entry.inert) return misfit()
  return entry
}

/** The compiled form of a part, made the first time something is found to move or change it. */
function targetOf(b: Builder, id: string): { target: CTarget; entry: IndexEntry } {
  const entry = partOf(b, id)
  let target = b.targets.get(id)
  if (!target) {
    const view = viewBoxOf(b.look)
    const given: unknown = own(b.look.pivots, id)
    let px = view[0] + view[2] / 2
    let py = id === b.bodyId ? view[1] + view[3] : view[1] + view[3] / 2
    const box: unknown = entry.node.box
    if (Array.isArray(given) && given.length === 2 && isNum(given[0]) && isNum(given[1])) {
      px = given[0]
      py = given[1]
    } else if (Array.isArray(box) && box.length === 4 && box.every(isNum)) {
      // No pivot of its own: the centre of its box, and for the body the middle of its lower edge.
      px = box[0] + box[2] / 2
      py = id === b.bodyId ? box[1] + box[3] : box[1] + box[3] / 2
    }
    const ops = parseTransform(own(entry.node.attrs, 'transform') ?? '')
    const pivotX = fmt(px, 3) ?? misfit()
    const pivotY = fmt(py, 3) ?? misfit()
    target = {
      own: (ops ? formatTransform(ops) : null) ?? '',
      pivotX,
      pivotY,
      negX: fmt(-px, 3) ?? misfit(),
      negY: fmt(-py, 3) ?? misfit(),
      px: Number(pivotX),
      py: Number(pivotY),
      rest: new Map(),
      moves: false,
    }
    b.targets.set(id, target)
  }
  return { target, entry }
}

function compileTrack(b: Builder, track: Track, animation: Pick<Animation, 'times' | 'ease'>): CTrack | null {
  const prop = track.prop
  const kind: Kind = isTransformProp(prop) ? 'rel' : isColourProp(prop) ? 'colour' : prop === 'd' ? 'd' : 'num'
  const values: unknown = track.values
  if (!Array.isArray(values) || values.length < 2 || values.length > LIMITS.keyframes) return misfit()
  const n = values.length

  // Every keyframe as numbers; null stays null until the target is known.
  const nums: (number | null)[] = []
  const colours: (Rgba | null)[] = []
  const paths: (readonly number[] | null)[] = []
  let shape: PathShape | null = null
  for (const v of values as unknown[]) {
    if (v === null) {
      nums.push(null)
      colours.push(null)
      paths.push(null)
    } else if (kind === 'd') {
      const parsed = parsePath(v) ?? misfit()
      if (shape && !samePathShape(shape, parsed)) return misfit()
      shape = parsed
      paths.push(parsed.nums)
    } else if (kind === 'colour') {
      colours.push(parseHex(v) ?? misfit())
    } else {
      nums.push(isNum(v) ? v : misfit())
    }
  }

  const times = trackTimes(track, animation)
  if (times.length !== n || times[0] !== 0 || times[n - 1] !== 1) return misfit()
  for (let i = 0; i < n; i++) if (!isNum(times[i]) || (i > 0 && !(times[i] > times[i - 1]))) return misfit()
  const eases = trackEases(track, animation)
  if (eases.length !== n - 1 || !eases.every((e) => isEase(e))) return misfit()

  const named: unknown = track.target
  const ids = typeof named === 'string' ? [named] : Array.isArray(named) ? (named as unknown[]) : misfit()
  if (ids.length < 1 || ids.length > LIMITS.elements) return misfit()
  const frames = new Map<string, Frames>()
  let shared: Frames | null = null
  for (const id of ids) {
    const part = partOf(b, id)
    if (typeof id !== 'string' || frames.has(id)) continue
    if (!propFits(prop, part.tag)) return misfit()
    if (kind === 'rel') {
      targetOf(b, id).target.moves = true
      const base = prop === 'scale' || prop === 'scaleX' || prop === 'scaleY' ? 1 : 0
      shared ??= { nums: nums.map((v) => v ?? base), colours: [], paths: [] }
      frames.set(id, shared)
      continue
    }
    let rest = b.targets.get(id)?.rest.get(prop)
    if (!rest) {
      const found = restOf(b.index, part, prop)
      // A part painted with one of the app's tokens: its paint cannot be put back from here, so it is not animated.
      if (!found) continue
      rest = found
      targetOf(b, id).target.rest.set(prop, rest)
    }
    if (kind === 'num') {
      const base = rest.kind === 'num' ? rest.value : 0
      frames.set(id, { nums: nums.map((v) => v ?? base), colours: [], paths: [] })
    } else if (kind === 'colour') {
      const base = rest.kind === 'colour' ? rest : null
      const resolved: Rgba[] = []
      for (let i = 0; i < n; i++) {
        const c = colours[i]
        if (c) {
          resolved.push(c)
          continue
        }
        if (base?.rgba) {
          resolved.push(base.rgba)
          continue
        }
        // The artwork has no plain colour to return to: hold the neighbouring keyframe (from nothing: transparent).
        const near = nearest(colours, i)
        if (!near) break
        resolved.push(base?.fromNothing ? [near[0], near[1], near[2], 0] : near)
      }
      if (resolved.length === n) frames.set(id, { nums: [], colours: resolved, paths: [] })
    } else {
      const base = rest.kind === 'd' && rest.shape && shape && samePathShape(rest.shape, shape) ? rest.nums : null
      const resolved: (readonly number[])[] = []
      for (let i = 0; i < n; i++) {
        const p = paths[i] ?? base ?? nearest(paths, i)
        if (!p) break
        resolved.push(p)
      }
      if (resolved.length === n) frames.set(id, { nums: [], colours: [], paths: resolved })
    }
  }
  // Nothing left to animate (a paint that cannot be put back, or nothing but nulls): the track is left out.
  if (frames.size === 0 || (kind === 'd' && !shape)) return null
  const phase = isNum(track.phase) ? clamp01(track.phase) : 0
  const delayMs = isNum(track.delay) && track.delay > 0 ? track.delay * 1000 : 0
  return { prop, kind, channels: CHANNELS.get(prop) ?? [], times, eases, shape, phase, delayMs, frames }
}

function compileAnimation(b: Builder, animation: Animation): CAnim {
  if (typeof animation !== 'object' || animation === null) return misfit()
  const duration: unknown = animation.duration
  if (!isNum(duration) || duration < 0.05 || duration > 60) return misfit()
  const list: unknown = animation.tracks
  if (!Array.isArray(list) || list.length < 1 || list.length > LIMITS.tracks) return misfit()
  const tracks: CTrack[] = []
  let maxDelayMs = 0
  for (const t of list as Track[]) {
    if (typeof t !== 'object' || t === null) return misfit()
    const track = compileTrack(b, t, animation)
    if (!track) continue
    tracks.push(track)
    maxDelayMs = Math.max(maxDelayMs, track.delayMs)
  }
  const range: unknown = animation.every
  let every: readonly [number, number] | null = null
  if (Array.isArray(range) && range.length === 2 && isNum(range[0]) && isNum(range[1])) {
    const min = Math.max(range[0], 0)
    every = [min, Math.max(range[1], min)]
  }
  return { durationMs: duration * 1000, loop: animation.loop === true && !every, every, tracks, maxDelayMs }
}

function compileDrive(b: Builder, drive: Drive): CDrive | null {
  if (typeof drive !== 'object' || drive === null) return misfit()
  const input = typeof drive.input === 'string' && INPUTS.has(drive.input) ? drive.input : misfit()
  // Where it looks and its mouth move fast: they never drive a colour or an opacity.
  if (isPaintProp(drive.prop) && !SLOW_INPUTS.has(input)) return misfit()
  // Phase and delay mean nothing here: a drive has no time, only its input's range.
  const track = compileTrack(b, { target: drive.target, prop: drive.prop, values: drive.values, times: drive.at, ease: drive.ease }, {})
  return track ? { input, track } : null
}

function compileMouth(b: Builder, drive: MouthDrive): CMouth {
  if (typeof drive !== 'object' || drive === null || !isId(drive.target)) return misfit()
  const { target, entry } = targetOf(b, drive.target)
  if ('prop' in drive && drive.prop !== undefined) {
    const prop: Prop = drive.prop
    if (!propFits(prop, entry.tag) || !isNum(drive.open)) return misfit()
    const channels = CHANNELS.get(prop)
    if (channels) {
      if (prop !== 'scaleY' && prop !== 'scaleX' && prop !== 'scale' && prop !== 'translateY') return misfit()
      target.moves = true
      const base = prop === 'translateY' ? 0 : 1
      return { target: drive.target, kind: 'channel', channels, closed: isNum(drive.closed) ? drive.closed : base, open: drive.open }
    }
    if (prop !== 'ry' && prop !== 'r' && prop !== 'height') return misfit()
    let rest = target.rest.get(prop)
    if (!rest) {
      rest = restOf(b.index, entry, prop) ?? misfit()
      target.rest.set(prop, rest)
    }
    return { target: drive.target, kind: 'attr', prop, closed: isNum(drive.closed) ? drive.closed : rest.kind === 'num' ? rest.value : 0, open: drive.open }
  }
  if (entry.tag !== 'path') return misfit()
  const open = parsePath(drive.open) ?? misfit()
  let rest = target.rest.get('d')
  if (!rest) {
    rest = restOf(b.index, entry, 'd') ?? misfit()
    target.rest.set('d', rest)
  }
  const closed = drive.closed !== undefined && drive.closed !== null ? (parsePath(drive.closed) ?? misfit()) : rest.kind === 'd' && rest.shape ? rest.shape : misfit()
  if (!samePathShape(open, closed)) return misfit()
  return { target: drive.target, kind: 'd', shape: closed, closed: closed.nums, open: open.nums }
}

/** The elements around an entry and the entry itself, outermost first. */
const chainOf = (entry: IndexEntry): readonly LookNode[] => [...entry.ancestors, entry.node]

/** R(E): where a part rests in the viewBox, from the transforms of the elements around it and its own. */
function restWorld(entry: IndexEntry): Matrix {
  let m: Matrix = IDENTITY
  for (const node of chainOf(entry)) m = multiply(m, ownMatrix(node))
  return m
}

/** `parents` as the engine uses them: every child a target that moves, every chain bounded. */
function compileSkeleton(b: Builder): Skeleton | null {
  const given: unknown = b.look.parents
  if (typeof given !== 'object' || given === null) return null
  const names = Object.keys(given)
  if (names.length === 0) return null
  if (names.length > MORE_LIMITS.parents) return misfit()
  const parents = new Map<string, string>()
  const restInverse = new Map<string, Matrix | null>()
  const restOuter = new Map<string, Matrix>()
  for (const child of names) {
    const parent = own(given, child)
    if (!isId(child) || !isId(parent) || child === parent) return misfit()
    const q = partOf(b, parent)
    const { target, entry } = targetOf(b, child)
    target.moves = true
    parents.set(child, parent)
    if (!restInverse.has(parent)) restInverse.set(parent, invert(restWorld(q)))
    let outer: Matrix = IDENTITY
    for (const node of entry.ancestors) outer = multiply(outer, ownMatrix(node))
    restOuter.set(child, outer)
  }
  for (const child of parents.keys()) {
    let at: string | undefined = child
    let links = 0
    while (at !== undefined && parents.has(at)) {
      at = parents.get(at)
      if (++links > MAX_CHAIN_LINKS) return misfit()
    }
  }
  return { parents, restInverse, restOuter }
}

function build(look: Look): Full {
  if (typeof look !== 'object' || look === null) return misfit()
  const b: Builder = { look, index: lookIndex(look), bodyId: rolePart(look, 'body'), targets: new Map() }
  const anims = new Map<string, CAnim>()
  // The built-in ones first: an agent animation with a built-in's name replaces it.
  for (const [name, animation] of builtinAnimations(look)) {
    try {
      anims.set(name, compileAnimation(b, animation))
    } catch {
      // A built-in that does not fit this look is left out; the look itself is not at fault.
    }
  }
  const mine: unknown = look.animations
  if (typeof mine === 'object' && mine !== null) {
    const names = Object.keys(mine)
    if (names.length > LIMITS.animations) return misfit()
    for (const name of names) {
      if (!isId(name)) return misfit()
      anims.set(name, compileAnimation(b, (mine as Record<string, Animation>)[name]))
    }
  }
  const drives: CDrive[] = []
  const listed: unknown = look.drives
  if (listed !== undefined && listed !== null) {
    if (!Array.isArray(listed) || listed.length > MORE_LIMITS.drives) return misfit()
    for (const drive of listed as Drive[]) {
      const compiled = compileDrive(b, drive)
      if (compiled) drives.push(compiled)
    }
  }
  const skeleton = compileSkeleton(b)
  const mouth: CMouth[] = []
  const entries: unknown = look.mouth
  if (Array.isArray(entries)) {
    if (entries.length > MORE_LIMITS.mouthDrives) return misfit()
    for (const drive of entries as MouthDrive[]) mouth.push(compileMouth(b, drive))
  }
  let gazeTarget = rolePart(look, 'pupils')
  const travel = gazeTravel(look)
  if (gazeTarget && travel > 0) targetOf(b, gazeTarget).target.moves = true
  else gazeTarget = null
  return {
    anims,
    targets: b.targets,
    drives,
    readsLook: drives.some((d) => d.input === 'lookX' || d.input === 'lookY'),
    readsMouth: drives.some((d) => d.input === 'mouth'),
    skeleton,
    mouth,
    gazeTarget,
    gazeTravel: travel,
    large: largeParts(b, anims, drives, mouth, skeleton),
    rest: new Map(),
  }
}

/** A part that covers this share of the viewBox or more, at the largest scale the look gives it, is large. */
const LARGE_SHARE = 1 / 8
/** Size props whose values, in a track, a drive or a mouth entry, make a part large when they reach this far, as shares of the viewBox width. */
const FAR_REACH: ReadonlyMap<Prop, number> = new Map<Prop, number>([
  ['r', 0.2],
  ['rx', 0.2],
  ['ry', 0.2],
  ['strokeWidth', 0.2],
  ['width', 0.35],
  ['height', 0.35],
])

/**
 * A part's rest box in the viewBox as it is painted: its box widened on every side by half
 * the width of the stroke it is painted with (its own or what it inherits; none without a
 * stroke). null when it has no box.
 */
function paintedBox(entry: IndexEntry): Box | null {
  const box: unknown = entry.node.box
  if (!Array.isArray(box) || box.length !== 4 || !box.every(isNum)) return null
  const stroke = inherited(entry, ['stroke', 'strokeRef', 'strokeToken'])
  const stroked = stroke !== null && !(stroke.key === 'stroke' && stroke.value === 'none')
  const width = stroked ? (numeric(inherited(entry, ['strokeWidth'])?.value ?? 1) ?? 1) : 0
  const half = Math.max(width, 0) / 2
  const out = boxThrough(entry.matrix, [box[0] - half, box[1] - half, box[2] + 2 * half, box[3] + 2 * half])
  return out.every((n) => Number.isFinite(n)) ? out : null
}

/**
 * The parts whose motion could flash: those that cover an eighth of the viewBox or more, at
 * the largest scale anything gives them (the largest absolute value of a scale keyframe, a
 * scale drive's value or a scale mouth entry that names the part, an element it is drawn in,
 * or a part of its `parents` chain or what that is drawn in; at least 1); those whose size
 * props reach far in a track, a drive or a mouth entry (`FAR_REACH`); and those without a box.
 * Eyes, pupils, a mouth and small limbs are small, so a blink and lip sync are not held back.
 */
function largeParts(b: Builder, anims: ReadonlyMap<string, CAnim>, drives: readonly CDrive[], mouth: readonly CMouth[], skeleton: Skeleton | null): Set<string> {
  const [, , width, height] = viewBoxOf(b.look)
  const scales = new Map<string, number>()
  const far = new Set<string>()
  const scaled = (id: string, v: number) => {
    if (isNum(v)) scales.set(id, Math.max(scales.get(id) ?? 1, Math.abs(v)))
  }
  const read = (track: CTrack) => {
    const scaling = track.channels.some((ch) => ch >= SCALE_FROM)
    const reach = FAR_REACH.get(track.prop)
    for (const [id, frames] of track.frames) {
      if (scaling) for (const v of frames.nums) scaled(id, v)
      if (reach !== undefined && frames.nums.some((v) => v >= reach * width)) far.add(id)
    }
  }
  for (const anim of anims.values()) for (const track of anim.tracks) read(track)
  for (const drive of drives) read(drive.track)
  for (const m of mouth) {
    if (m.kind === 'channel' && m.channels.some((ch) => ch >= SCALE_FROM)) {
      scaled(m.target, m.closed)
      scaled(m.target, m.open)
    }
    const reach = m.kind === 'attr' ? FAR_REACH.get(m.prop) : undefined
    if (m.kind === 'attr' && reach !== undefined && Math.max(m.closed, m.open) >= reach * width) far.add(m.target)
  }
  const out = new Set<string>()
  for (const id of b.targets.keys()) {
    const entry = b.index.get(id)
    const box = entry ? paintedBox(entry) : null
    if (far.has(id) || !entry || !box) {
      out.add(id)
      continue
    }
    let most = 1
    let at: string | undefined = id
    for (let links = 0; at !== undefined && links <= MAX_CHAIN_LINKS; links++) {
      const chain = b.index.get(at)
      for (const node of chain ? chainOf(chain) : []) if (typeof node.id === 'string') most = Math.max(most, scales.get(node.id) ?? 1)
      at = skeleton?.parents.get(at)
    }
    if (box[2] * box[3] * most * most >= width * height * LARGE_SHARE) out.add(id)
  }
  return out
}

/**
 * Every track and drive of a look turned into numbers, the built-in animations its roles
 * allow included. null when a value does not fit its prop (a string where a number belongs,
 * path keyframes of different structure, a target that does not exist): the look cannot be
 * animated and the default look is shown instead. Compiled once per look object.
 */
export function compile(look: Look): Compiled | null {
  if (typeof look !== 'object' || look === null) return null
  if (COMPILED.has(look)) return COMPILED.get(look) ?? null
  let compiled: Compiled | null = null
  try {
    const full = build(look)
    compiled = Object.freeze({ look })
    FULL.set(compiled, full)
    full.rest = sample(compiled, [], 0)
  } catch {
    compiled = null
  }
  COMPILED.set(look, compiled)
  return compiled
}

// ---------------------------------------------------------------- sample

/** A value of an absolute prop while clips are being added up. */
type Abs = { kind: 'num'; v: number } | { kind: 'colour'; v: Rgba } | { kind: 'd'; v: readonly number[]; shape: PathShape }

interface Acc {
  /** tx, ty, rotation, skew x, skew y, sx, sy. */
  ch: number[]
  abs: Map<Prop, Abs> | null
}

const lerp = (a: number, b: number, t: number) => a + (b - a) * t

function lerpNums(a: readonly number[], b: readonly number[], t: number, shape: PathShape): number[] {
  const out: number[] = []
  for (let i = 0; i < a.length; i++) out.push(shape.flags[i] ? a[i] : lerp(a[i], b[i] ?? a[i], t))
  return out
}

/** Sizes that an overshooting ease must not take below 0 (a negative radius or width is an error in SVG). A dash offset may be negative. */
const NOT_NEGATIVE: ReadonlySet<Prop> = new Set<Prop>(['strokeWidth', 'r', 'rx', 'ry', 'width', 'height'])

/** A track's value at progress `p` (0 to 1) for one target. */
function valueAt(track: CTrack, frames: Frames, p: number): Abs | number {
  const times = track.times
  let i = 0
  while (i < times.length - 2 && p >= times[i + 1]) i++
  const span = times[i + 1] - times[i]
  const f = ease(track.eases[i] ?? 'linear', span > 0 ? clamp01((p - times[i]) / span) : 1)
  if (track.kind === 'colour') {
    const a = frames.colours[i]
    const c = frames.colours[i + 1]
    return { kind: 'colour', v: [lerp(a[0], c[0], f), lerp(a[1], c[1], f), lerp(a[2], c[2], f), lerp(a[3], c[3], f)] }
  }
  if (track.kind === 'd') {
    const shape = track.shape as PathShape
    return { kind: 'd', v: lerpNums(frames.paths[i], frames.paths[i + 1], f, shape), shape }
  }
  const v = lerp(frames.nums[i], frames.nums[i + 1], f)
  if (track.kind === 'rel') return v
  return { kind: 'num', v: NOT_NEGATIVE.has(track.prop) && v < 0 ? 0 : v }
}

/** `over` laid on `under` with a weight: the animation started last wins, and fades take it there smoothly. */
function mix(under: Abs | undefined, rest: Rest | undefined, over: Abs, w: number): Abs {
  if (w >= 1) return over
  if (over.kind === 'num') {
    const base = under?.kind === 'num' ? under.v : rest?.kind === 'num' ? rest.value : over.v
    return { kind: 'num', v: lerp(base, over.v, w) }
  }
  if (over.kind === 'colour') {
    let base: Rgba = over.v
    if (under?.kind === 'colour') base = under.v
    else if (rest?.kind === 'colour') base = rest.rgba ?? (rest.fromNothing ? [over.v[0], over.v[1], over.v[2], 0] : over.v)
    return { kind: 'colour', v: [lerp(base[0], over.v[0], w), lerp(base[1], over.v[1], w), lerp(base[2], over.v[2], w), lerp(base[3], over.v[3], w)] }
  }
  let base: readonly number[] = over.v
  if (under?.kind === 'd' && samePathShape(under.shape, over.shape)) base = under.v
  else if (!under && rest?.kind === 'd' && rest.shape && samePathShape(rest.shape, over.shape)) base = rest.nums
  return { kind: 'd', v: lerpNums(base, over.v, w, over.shape), shape: over.shape }
}

const frac = (n: number) => n - Math.floor(n)

function absText(prop: Prop, value: Abs): string | null {
  if (value.kind === 'num') return fmt(NOT_NEGATIVE.has(prop) && value.v < 0 ? 0 : value.v, 3)
  if (value.kind === 'colour') return colourText(value.v)
  return formatPath(value.shape, value.v)
}

/**
 * Where along a drive's range its input is now, 0 to 1. `lookX`, `lookY` and `turn` run from
 * -1 (position 0) through 0 (0.5) to 1; the others are their own position. With reduced
 * motion every input but the mouth is at rest.
 */
function position(input: DriveInput, i: SampleInputs, reduced: boolean): number {
  const within = (v: unknown, lo: number) => (isNum(v) ? Math.min(Math.max(v, lo), 1) : 0)
  switch (input) {
    case 'mouth':
      return within(i.mouth, 0)
    case 'lookX':
    case 'lookY': {
      const g = i.gaze
      if (reduced || !Array.isArray(g) || !isNum(g[0]) || !isNum(g[1])) return 0.5
      const length = Math.hypot(g[0], g[1])
      const v = (input === 'lookX' ? g[0] : g[1]) / (length > 1 ? length : 1)
      return (within(v, -1) + 1) / 2
    }
    case 'turn':
      return reduced ? 0.5 : (within(i.turn, -1) + 1) / 2
    case 'pace':
      return reduced ? 0 : within(i.pace, 0)
    case 'held':
      return reduced ? 0 : within(i.held, 0)
  }
}

/** A part's motion this frame, as the strings written and, for a look with `parents`, as a matrix M(E). */
interface Motion {
  parts: string[]
  m: Matrix
}

/** `inner` about the point (px, py). */
function about(px: number, py: number, inner: Matrix): Matrix {
  return multiply(multiply([1, 0, 0, 1, px, py], inner), [1, 0, 0, 1, -px, -py])
}

/**
 * Translate, then rotate, skew and scale about the pivot: each written only when it is not
 * the identity, and the matrix built from exactly the numbers written. null: a number that is
 * not finite.
 */
function motionOf(t: CTarget, ch: readonly number[] | undefined, withMatrix: boolean): Motion | null {
  const out: Motion = { parts: [], m: IDENTITY }
  if (!ch) return out
  const tx = fmt(ch[0], 2)
  const ty = fmt(ch[1], 2)
  const rot = fmt(ch[2], 2)
  const kx = fmt(ch[3], 2)
  const ky = fmt(ch[4], 2)
  const sx = fmt(ch[5], 4)
  const sy = fmt(ch[6], 4)
  if (tx === null || ty === null || rot === null || kx === null || ky === null || sx === null || sy === null) return null
  // Only a look with `parents` needs the matrix: the others never build one.
  const then = (m: () => Matrix) => {
    if (withMatrix) out.m = multiply(out.m, m())
  }
  if (tx !== '0' || ty !== '0') {
    out.parts.push(`translate(${tx} ${ty})`)
    then(() => [1, 0, 0, 1, Number(tx), Number(ty)])
  }
  if (rot !== '0') {
    out.parts.push(`rotate(${rot} ${t.pivotX} ${t.pivotY})`)
    then(() => opMatrix({ fn: 'rotate', args: [Number(rot), t.px, t.py] }))
  }
  if (kx !== '0' || ky !== '0') {
    const skews = [kx !== '0' ? `skewX(${kx})` : '', ky !== '0' ? `skewY(${ky})` : ''].filter(Boolean).join(' ')
    out.parts.push(`translate(${t.pivotX} ${t.pivotY}) ${skews} translate(${t.negX} ${t.negY})`)
    then(() => about(t.px, t.py, multiply(opMatrix({ fn: 'skewX', args: [Number(kx)] }), opMatrix({ fn: 'skewY', args: [Number(ky)] }))))
  }
  if (sx !== '1' || sy !== '1') {
    out.parts.push(`translate(${t.pivotX} ${t.pivotY}) scale(${sx} ${sy}) translate(${t.negX} ${t.negY})`)
    then(() => about(t.px, t.py, [Number(sx), 0, 0, Number(sy), 0, 0]))
  }
  return out
}

const nearIdentity = (m: Matrix): boolean => Math.abs(m[0] - 1) <= NEAR_IDENTITY && Math.abs(m[1]) <= NEAR_IDENTITY && Math.abs(m[2]) <= NEAR_IDENTITY && Math.abs(m[3] - 1) <= NEAR_IDENTITY && Math.abs(m[4]) <= NEAR_IDENTITY && Math.abs(m[5]) <= NEAR_IDENTITY

/**
 * The correction C of every child of `parents`, written in front of its transform so that it
 * moves with its parent as if nested in it. With A1…An the elements around E, outermost
 * first, S its own transform, M its motion, T = S·M:
 *
 * - X(E) = T(E), and for a child of Q: X(E) = C·T(E) with C = P⁻¹·D(Q)·Rp,
 *   P = X(A1)·…·X(An) (where E's DOM parent is now), Rp = S(A1)·…·S(An) (where it rests),
 *   D(Q) = W(Q)·R(Q)⁻¹;
 * - W(E) = X(A1)·…·X(An)·X(E) (where it is now), R(E) = S(A1)·…·S(An)·S(E) (where it rests).
 *
 * So W(E) = D(Q)·Rp·T(E): E moves exactly as far as Q is from its rest, as if it were nested
 * in Q. What moves E's DOM parent is in P and, when it holds Q as well, in D(Q) too; P⁻¹
 * takes it out once, so a group that moves or mirrors both is applied once.
 *
 * null for a child whose matrices cannot be inverted, whose numbers are not finite, or
 * whose rig depends on itself through what it is nested in: it is left out of the frame.
 */
function corrections(full: Full, skeleton: Skeleton, index: ReadonlyMap<string, IndexEntry>, motions: ReadonlyMap<string, Motion | null>): Map<string, Matrix | null> {
  const transformOf = (node: LookNode): Matrix | null => {
    const own = ownMatrix(node)
    const id = node.id
    if (typeof id !== 'string' || !full.targets.has(id)) return own
    const motion = motions.get(id)
    if (motion === null) return null
    return motion && motion.parts.length > 0 ? multiply(own, motion.m) : own
  }
  const live = new Map<LookNode, Matrix | null>()
  const found = new Map<string, Matrix | null>()
  const worldAt = (chain: readonly LookNode[], i: number): Matrix | null => {
    if (i < 0) return IDENTITY
    const node = chain[i]
    if (live.has(node)) return live.get(node) ?? null
    // While it is worked out it counts as unsound: a rig that leads back to itself is left out.
    live.set(node, null)
    const outer = worldAt(chain, i - 1)
    const x = outer ? written(node, outer) : null
    const world = outer && x ? multiply(outer, x) : null
    live.set(node, world)
    return world
  }
  const correction = (child: string, outer: Matrix): Matrix | null => {
    if (found.has(child)) return found.get(child) ?? null
    found.set(child, null)
    const parent = skeleton.parents.get(child)
    const entry = parent === undefined ? undefined : index.get(parent)
    const restInverse = parent === undefined ? null : (skeleton.restInverse.get(parent) ?? null)
    const pInverse = invert(outer)
    let c: Matrix | null = null
    if (entry && restInverse && pInverse) {
      const now = worldAt(chainOf(entry), entry.ancestors.length)
      if (now) c = multiply(multiply(pInverse, multiply(now, restInverse)), skeleton.restOuter.get(child) ?? IDENTITY)
    }
    if (c && !c.every((n) => Number.isFinite(n))) c = null
    found.set(child, c)
    return c
  }
  /** X(E): what is written on it, given where its DOM parent is now. */
  const written = (node: LookNode, outer: Matrix): Matrix | null => {
    const t = transformOf(node)
    if (!t) return null
    const id = node.id
    if (typeof id !== 'string' || !skeleton.parents.has(id)) return t
    const c = correction(id, outer)
    return c ? multiply(c, t) : null
  }
  const out = new Map<string, Matrix | null>()
  for (const child of skeleton.parents.keys()) {
    const entry = index.get(child)
    const outer = entry ? worldAt(entry.ancestors, entry.ancestors.length - 1) : null
    out.set(child, outer ? correction(child, outer) : null)
  }
  return out
}

/**
 * The pose of every part a look can move, at one moment. Pure: the same arguments give the
 * same poses.
 *
 * - State clips add up: translations, rotations and skews add, scales multiply, and for
 *   every other prop the clip started last wins. A weight `w` applies as `t*w`, `r*w` and
 *   `1 + (s-1)*w`.
 * - A gesture's value replaces that sum for the props it animates, blended in over 150 ms
 *   and, when a `once` gesture has played, out again over 150 ms.
 * - A one-shot in a state list leaves its relative props fading for 200 ms after its last frame.
 * - With `inputs`, the look's drives come next, each at its input's position: translations,
 *   rotations and skews add, scales multiply, every other prop is the drive's.
 * - The mouth (`inputs.mouth` 0 to 1; null: not applied) and the gaze (`inputs.gaze`: a
 *   vector of length 0 to 1; null: none) come last.
 * - `parents` put a correction in front of each child's transform (see `corrections`).
 *
 * A part whose numbers are not finite is left out of the result. A clip whose name means
 * nothing for the look is ignored.
 *
 * Two steps, each pure: `accumulate` adds everything up into numbers per part, `settle`
 * turns those numbers into what is written. The frame loop limits the numbers between them.
 */
export function sample(c: Compiled, clips: readonly Clip[], tMs: number, inputs?: SampleInputs | null, opts?: SampleOptions): Map<string, Pose> {
  const full = FULL.get(c)
  if (!full || !isNum(tMs)) return new Map()
  return settle(full, c.look, accumulate(full, clips, tMs, inputs, opts))
}

/**
 * Everything that applies at one moment, added up: per part its transform channels and the
 * values of its absolute props. A part that nothing touches is not in the map (it is at rest).
 */
function accumulate(full: Full, clips: readonly Clip[], tMs: number, inputs?: SampleInputs | null, opts?: SampleOptions): Map<string, Acc> {
  const reduced = opts?.reducedMotion === true
  const accs = new Map<string, Acc>()
  const acc = (id: string): Acc => {
    let a = accs.get(id)
    if (!a) {
      a = { ch: [...REST_CHANNELS], abs: null }
      accs.set(id, a)
    }
    return a
  }
  const setAbs = (id: string, prop: Prop, value: Abs, w: number) => {
    const a = acc(id)
    a.abs ??= new Map()
    a.abs.set(prop, mix(a.abs.get(prop), full.targets.get(id)?.rest.get(prop), value, w))
  }

  const playing: { clip: Clip; anim: CAnim; weight: number }[] = []
  if (Array.isArray(clips)) {
    for (const clip of clips) {
      if (typeof clip !== 'object' || clip === null || typeof clip.name !== 'string') continue
      const anim = full.anims.get(clip.name)
      if (!anim || !isNum(clip.startMs) || !isNum(clip.weight)) continue
      const weight = clamp01(clip.weight)
      if (weight > 0 && tMs >= clip.startMs) playing.push({ clip, anim, weight })
    }
  }
  // Started first, applied first: what started last wins. A stable sort keeps the list's order for ties.
  playing.sort((a, b) => a.clip.startMs - b.clip.startMs)

  // State animations.
  for (const { clip, anim, weight } of playing) {
    if (clip.mode !== 'state') continue
    const lt = tMs - clip.startMs
    if (reduced && (anim.loop || anim.every)) continue
    for (const track of anim.tracks) {
      let p: number
      let gain = weight
      let relativeOnly = false
      if (anim.loop) {
        p = frac(lt / anim.durationMs + track.phase)
      } else {
        const local = lt - track.delayMs
        if (local < 0) continue
        if (reduced) {
          if (local > anim.durationMs) continue
          p = 1
        } else if (local <= anim.durationMs) {
          p = local / anim.durationMs
        } else if (local <= anim.durationMs + STATE_FADE_MS) {
          p = 1
          gain = weight * (1 - (local - anim.durationMs) / STATE_FADE_MS)
          relativeOnly = true
        } else {
          continue
        }
      }
      for (const [id, frames] of track.frames) {
        const v = valueAt(track, frames, p)
        if (typeof v !== 'number') {
          if (!relativeOnly) setAbs(id, track.prop, v, gain)
          continue
        }
        const a = acc(id)
        for (const ch of track.channels) {
          if (ch < SCALE_FROM) a.ch[ch] += v * gain
          else a.ch[ch] *= 1 + (v - 1) * gain
        }
      }
    }
  }

  // Gestures: their values take the place of the state's sum.
  for (const { clip, anim, weight } of playing) {
    if (clip.mode === 'state') continue
    const lt = tMs - clip.startMs
    const passes = clip.mode === 'once' ? Math.min(Math.max(isNum(clip.times) ? Math.floor(clip.times) : 1, 1), 5) : 1
    const playMs = anim.durationMs * passes
    let out150 = 1
    if (clip.mode === 'once') {
      const end = playMs + anim.maxDelayMs
      if (lt > end) {
        if (reduced) continue
        out150 = 1 - (lt - end) / GESTURE_BLEND_MS
        if (out150 <= 0) continue
      }
    }
    const moved = new Map<string, { v: number[]; w: number[] }>()
    for (const track of anim.tracks) {
      const local = lt - track.delayMs
      if (local < 0) continue
      let p: number
      if (reduced) p = 1
      else if (clip.mode === 'loop') p = frac(local / anim.durationMs + track.phase)
      else if (local >= playMs) p = 1
      else if (clip.mode === 'hold') p = local / anim.durationMs
      else p = frac(local / anim.durationMs)
      const w = weight * out150 * (reduced ? 1 : clamp01(local / GESTURE_BLEND_MS))
      if (w <= 0) continue
      for (const [id, frames] of track.frames) {
        const v = valueAt(track, frames, p)
        if (typeof v !== 'number') {
          setAbs(id, track.prop, v, w)
          continue
        }
        let m = moved.get(id)
        if (!m) {
          m = { v: [...REST_CHANNELS], w: [0, 0, 0, 0, 0, 0, 0] }
          moved.set(id, m)
        }
        for (const ch of track.channels) {
          if (ch < SCALE_FROM) m.v[ch] += v
          else m.v[ch] *= v
          m.w[ch] = Math.max(m.w[ch], w)
        }
      }
    }
    for (const [id, m] of moved) {
      const a = acc(id)
      for (let ch = 0; ch < REST_CHANNELS.length; ch++) if (m.w[ch] > 0) a.ch[ch] = lerp(a.ch[ch], m.v[ch], m.w[ch])
    }
  }

  // The drives: on top of the animations, each at its input's position.
  if (inputs && full.drives.length > 0) {
    for (const drive of full.drives) {
      const p = position(drive.input, inputs, reduced)
      const track = drive.track
      for (const [id, frames] of track.frames) {
        const v = valueAt(track, frames, p)
        if (typeof v !== 'number') {
          setAbs(id, track.prop, v, 1)
          continue
        }
        const a = acc(id)
        for (const ch of track.channels) {
          if (ch < SCALE_FROM) a.ch[ch] += v
          else a.ch[ch] *= v
        }
      }
    }
  }

  // The mouth, then the gaze: applied last.
  const mouthOpen = inputs?.mouth
  if (isNum(mouthOpen)) {
    const m = clamp01(mouthOpen)
    for (const drive of full.mouth) {
      if (drive.kind === 'd') setAbs(drive.target, 'd', { kind: 'd', v: lerpNums(drive.closed, drive.open, m, drive.shape), shape: drive.shape }, 1)
      else if (drive.kind === 'attr') setAbs(drive.target, drive.prop, { kind: 'num', v: lerp(drive.closed, drive.open, m) }, 1)
      else for (const ch of drive.channels) acc(drive.target).ch[ch] = lerp(drive.closed, drive.open, m)
    }
  }
  const gaze = inputs?.gaze
  if (!reduced && full.gazeTarget && Array.isArray(gaze) && isNum(gaze[0]) && isNum(gaze[1])) {
    const length = Math.hypot(gaze[0], gaze[1])
    const k = full.gazeTravel / (length > 1 ? length : 1)
    const a = acc(full.gazeTarget)
    a.ch[0] += gaze[0] * k
    a.ch[1] += gaze[1] * k
  }
  return accs
}

/**
 * The numbers of one moment as what is written: every part's transform (with the
 * corrections of `parents`) and its absolute props as text. Every part the look can move
 * gets a pose; one whose numbers are not finite is left out.
 */
function settle(full: Full, look: Look, accs: ReadonlyMap<string, Acc>): Map<string, Pose> {
  const out = new Map<string, Pose>()
  // Every part's motion as it is written; with `parents` also as matrices, for the corrections of the children.
  const skeleton = full.skeleton
  const motions = new Map<string, Motion | null>()
  for (const [id, target] of full.targets) if (target.moves) motions.set(id, motionOf(target, accs.get(id)?.ch, skeleton !== null))
  const fixes = skeleton ? corrections(full, skeleton, lookIndex(look), motions) : null

  // Every part the look can move gets a pose, so one that is no longer animated returns to the artwork's own values.
  for (const [id, target] of full.targets) {
    const a = accs.get(id)
    const pose: Pose = { attrs: new Map() }
    let sound = true
    if (target.moves) {
      const motion = motions.get(id) ?? null
      const parts: string[] = []
      if (fixes?.has(id)) {
        const fix = fixes.get(id) ?? null
        if (fix === null) sound = false
        else if (!nearIdentity(fix)) {
          const numbers = fix.map((n) => fmt(n, 6))
          if (numbers.some((n) => n === null)) sound = false
          else parts.push(`matrix(${numbers.join(' ')})`)
        }
      }
      if (target.own) parts.push(target.own)
      if (motion === null) sound = false
      else parts.push(...motion.parts)
      if (parts.length > 0) pose.transform = parts.join(' ')
    } else if (target.own) {
      pose.transform = target.own
    }
    for (const [prop, rest] of target.rest) {
      const name = DOM_ATTR.get(prop)
      if (!name) continue
      const value = a?.abs?.get(prop)
      if (!value) {
        // Nothing colours it now: a part painted with a gradient is at rest in that gradient.
        if (rest.kind === 'colour' && rest.ref !== undefined) (pose.refs ??= new Map()).set(name, rest.ref)
        else pose.attrs.set(name, rest.text)
        continue
      }
      const text = absText(prop, value)
      if (text === null) sound = false
      else pose.attrs.set(name, text)
    }
    if (sound) out.set(id, pose)
  }
  return out
}

// ---------------------------------------------------------------- a still figure

/**
 * Puts a look that is drawn without an engine (a small face, a fallback) into the pose a
 * figure holds when nothing plays and every input is at rest. For most looks that is the
 * artwork as it was rendered and nothing is written. A look with drives differs: a prop a
 * drive sets belongs to the drive (a side view that shows only while it walks is hidden at
 * rest), so those values are written once. `find` and `idPrefix` as for `Engine.attach`.
 *
 * Gives back what puts the artwork's own values back on the nodes it wrote, for a figure
 * that an engine takes over (the engine starts from the artwork as it was rendered).
 */
export function writeStill(look: Look, find: (id: string) => EngineNode | null, idPrefix: string): () => void {
  const compiled = compile(look)
  const full = compiled ? FULL.get(compiled) : undefined
  if (!compiled || !full || full.drives.length === 0) return () => {}
  const prefix = cleanIdPrefix(idPrefix)
  const still = sample(compiled, [], 0, {})
  const nodes = new Map<string, EngineNode>()
  for (const id of still.keys()) {
    try {
      const node = find(id)
      if (node && typeof node.setAttribute === 'function' && typeof node.removeAttribute === 'function') nodes.set(id, node)
    } catch {
      // A part it cannot find keeps the artwork's own values.
    }
  }
  repose(nodes, full.rest, still, prefix)
  return () => repose(nodes, still, full.rest, prefix)
}

/**
 * Writes `to` on nodes that hold `from`, where the two differ. A paint that is a gradient in
 * `to` is given its reference as it was rendered. One node that throws keeps what it has; the
 * others are still posed.
 */
function repose(nodes: ReadonlyMap<string, EngineNode>, from: ReadonlyMap<string, Pose>, to: ReadonlyMap<string, Pose>, prefix: string): void {
  for (const [id, pose] of to) {
    const node = nodes.get(id)
    if (!node) continue
    const was = from.get(id)
    try {
      if (pose.transform !== was?.transform) {
        if (pose.transform === undefined) node.removeAttribute('transform')
        else node.setAttribute('transform', pose.transform)
      }
      for (const [name, value] of pose.attrs) {
        if (was?.attrs.get(name) === value && !was?.refs?.has(name)) continue
        if (value === null) node.removeAttribute(name)
        else node.setAttribute(name, value)
      }
      for (const [name, ref] of pose.refs ?? []) if (!was?.refs?.has(name)) node.setAttribute(name, `url(#${prefix}${ref})`)
    } catch {
      // One node that throws keeps what it has; the others are still posed.
    }
  }
}

// ---------------------------------------------------------------- the limiter

/**
 * The least time between two turns of one channel of what the frame loop writes.
 *
 * The validators keep each track's paint keyframes 0.17 s apart, but what flashes is what
 * everything makes together: staggered tracks of one animation, a gesture played again and
 * again, a drive whose input jitters, a mouth at syllable rate. So the frame loop limits what
 * it writes, per part and channel: a change in the direction the channel last moved passes,
 * and so does its first change; a change that turns it back passes only 150 ms or more after
 * its last accepted turn, else the channel holds what it wrote last and takes the target as it
 * is then once the time is up (one late change, not a ramp). Eight turns would need seven
 * gaps of 150 ms, longer than a second, so no second holds more than seven turns, and between
 * seven turns lie six complete changes: at most three complete back-and-forth flashes in any
 * second, the usual threshold.
 */
export const REVERSAL_MS = 150
/** A change this small is no change: rounding, not motion. */
const STILL_DELTA = 1e-9

/** Whether the frame loop limits every channel of this part of a compiled look, not only its paint (see `largeParts`). false for a part the look never moves or changes. */
export function isLarge(c: Compiled, id: string): boolean {
  return FULL.get(c)?.large.has(id) ?? false
}

/**
 * One channel the frame loop limits: a transform channel of a part (`ch` 0 … 6), or an
 * absolute prop of it (`ch` -1). Numbers are compared as a vector: one number, a colour's
 * four channels (red, green and blue as shares of 255), or every number of path data.
 */
interface Lane {
  id: string
  ch: number
  prop: Prop | null
  rest: Rest | null
  /** What was written last, as numbers; null: no value (a paint at rest in a gradient, `none`, an inherited colour). */
  last: readonly number[] | null
  /** The path data's structure along with `last`. */
  shape: PathShape | null
  /** How an absolute prop was written last; undefined: as the artwork's own value. */
  written: Abs | undefined
  /** The change accepted last; null: none yet, or the last one was to or from no value. */
  step: readonly number[] | null
  /** When it last turned back, or went to or from no value. */
  turnAt: number
}

const unitColour = (c: Rgba): number[] => [c[0] / 255, c[1] / 255, c[2] / 255, c[3]]

/** A lane's target in the numbers of one moment: as numbers, as it would be written, and its path structure. */
function laneTarget(lane: Lane, accs: ReadonlyMap<string, Acc>): { nums: readonly number[] | null; abs: Abs | undefined; shape: PathShape | null } {
  const a = accs.get(lane.id)
  if (lane.prop === null) return { nums: [a ? a.ch[lane.ch] : REST_CHANNELS[lane.ch]], abs: undefined, shape: null }
  const abs = a?.abs?.get(lane.prop)
  if (abs) return { nums: abs.kind === 'num' ? [abs.v] : abs.kind === 'colour' ? unitColour(abs.v) : abs.v, abs, shape: abs.kind === 'd' ? abs.shape : null }
  const rest = lane.rest
  if (!rest) return { nums: null, abs: undefined, shape: null }
  if (rest.kind === 'num') return { nums: [rest.value], abs: undefined, shape: null }
  if (rest.kind === 'colour') return { nums: rest.rgba ? unitColour(rest.rgba) : null, abs: undefined, shape: null }
  return { nums: rest.shape ? rest.nums : null, abs: undefined, shape: rest.shape }
}

/**
 * The limiter of one figure: called with the numbers of each frame, before they are settled,
 * it puts back what a lane held where the lane may not change yet. Its lanes: the paint
 * (opacity, fill and stroke opacity, fill, stroke) of every part, and every other channel of
 * the large ones (`largeParts`). It starts from the artwork's own values, which is what was
 * rendered.
 */
function createLimiter(full: Full): (accs: Map<string, Acc>, now: number) => void {
  const lanes: Lane[] = []
  const none = new Map<string, Acc>()
  for (const [id, target] of full.targets) {
    const large = full.large.has(id)
    if (large && target.moves) for (let ch = 0; ch < REST_CHANNELS.length; ch++) lanes.push({ id, ch, prop: null, rest: null, last: [REST_CHANNELS[ch]], shape: null, written: undefined, step: null, turnAt: Number.NEGATIVE_INFINITY })
    for (const [prop, rest] of target.rest) {
      if (!large && !isPaintProp(prop)) continue
      const lane: Lane = { id, ch: -1, prop, rest, last: null, shape: null, written: undefined, step: null, turnAt: Number.NEGATIVE_INFINITY }
      const at = laneTarget(lane, none)
      lane.last = at.nums
      lane.shape = at.shape
      lanes.push(lane)
    }
  }

  /** Whether the lane may take its target now; if so it is taken. */
  const pass = (lane: Lane, accs: ReadonlyMap<string, Acc>, now: number): boolean => {
    const next = laneTarget(lane, accs)
    const take = (step: readonly number[] | null) => {
      lane.last = next.nums
      lane.shape = next.shape
      lane.written = next.abs
      lane.step = step
    }
    const last = lane.last
    const sameShape = lane.shape === null ? next.shape === null : next.shape !== null && samePathShape(lane.shape, next.shape)
    if (last === null || next.nums === null || last.length !== next.nums.length || !sameShape) {
      if (last === null && next.nums === null) {
        take(lane.step)
        return true
      }
      // To or from no value, or to another structure of path: a change that may come once in REVERSAL_MS.
      if (now - lane.turnAt < REVERSAL_MS) return false
      lane.turnAt = now
      take(null)
      return true
    }
    const delta: number[] = []
    let most = 0
    let along = 0
    for (let i = 0; i < last.length; i++) {
      const d = next.nums[i] - last[i]
      delta.push(d)
      most = Math.max(most, Math.abs(d))
      if (lane.step) along += d * (lane.step[i] ?? 0)
    }
    if (!(most > STILL_DELTA)) {
      take(lane.step)
      return true
    }
    if (lane.step === null || along >= 0) {
      take(delta)
      return true
    }
    // It turns back.
    if (now - lane.turnAt < REVERSAL_MS) return false
    lane.turnAt = now
    take(delta)
    return true
  }

  /** The lane's last value put back in place of its target. */
  const hold = (lane: Lane, accs: Map<string, Acc>) => {
    let a = accs.get(lane.id)
    if (!a) {
      a = { ch: [...REST_CHANNELS], abs: null }
      accs.set(lane.id, a)
    }
    if (lane.prop === null) {
      a.ch[lane.ch] = lane.last?.[0] ?? REST_CHANNELS[lane.ch]
      return
    }
    if (lane.written === undefined) a.abs?.delete(lane.prop)
    else (a.abs ??= new Map()).set(lane.prop, lane.written)
  }

  return (accs, now) => {
    for (const lane of lanes) if (!pass(lane, accs, now)) hold(lane, accs)
  }
}

// ---------------------------------------------------------------- the loop

interface Member {
  name: string
  startMs: number
  /** The weight it fades from and to, and when that fade began. */
  from: number
  to: number
  fadeAt: number
  /** A one-shot that has played: it stays a member, so it is not played again until the state is entered anew. */
  spent: boolean
}

interface Gesture {
  name: string
  mode: 'once' | 'hold' | 'loop'
  times: number
  startMs: number
  /** When it was told to stop (it then gives way over 150 ms). */
  stopAt: number | null
}

interface Figure {
  frame(now: number, dt: number): void
  /** Back to the artwork's own values, and no more frames. */
  end(): void
}

const INERT: FigureHandle = Object.freeze({
  setState: () => {},
  play: () => false,
  stop: () => {},
  setMouth: () => {},
  setGaze: () => {},
  setInputs: () => {},
  setReducedMotion: () => {},
  frozen: () => false,
  detach: () => {},
})

/**
 * The shared frame loop of one window. Figures attach to it; it asks the host for frames
 * only while it runs and has figures, and requests the next frame before it does any work.
 */
export function createEngine(host: EngineHost): Engine {
  const figures = new Set<Figure>()
  let running = true
  let disposed = false
  let pending: number | null = null
  let lastFrame: number | null = null
  let clock = 0
  let ticking = false

  const random = (): number => {
    let r = 0.5
    try {
      r = host.random()
    } catch {
      r = 0.5
    }
    return isNum(r) ? Math.min(Math.max(r, 0), 0.999999) : 0.5
  }

  const request = () => {
    if (pending !== null || !running || disposed || figures.size === 0) return
    try {
      pending = host.requestFrame(tick)
    } catch {
      pending = null
    }
  }

  const halt = () => {
    if (pending !== null) {
      try {
        host.cancelFrame(pending)
      } catch {
        // A host that cannot cancel: the late frame finds nothing to do.
      }
    }
    pending = null
    lastFrame = null
  }

  function tick(tMs: number) {
    pending = null
    if (ticking || disposed || !running) return
    ticking = true
    try {
      // The next frame first: whatever a figure does below, the loop goes on.
      request()
      if (!isNum(tMs)) return
      let dt = 0
      if (lastFrame !== null) {
        const delta = tMs - lastFrame
        if (delta < 0) {
          lastFrame = tMs
          return
        }
        if (delta < FRAME_MS) return
        dt = Math.min(delta, MAX_STEP_MS)
      }
      lastFrame = tMs
      clock += dt
      // A figure may leave the set while it is being walked (it froze or threw): that is safe.
      for (const figure of figures) figure.frame(clock, dt)
      if (figures.size === 0) halt()
    } finally {
      ticking = false
    }
  }

  function attach(look: Look, find: (id: string) => EngineNode | null, opts: { state: AvatarState; reducedMotion: boolean; idPrefix?: string }): FigureHandle {
    const compiled = disposed ? null : compile(look)
    const full = compiled ? FULL.get(compiled) : undefined
    if (!compiled || !full) return INERT

    /** What the figure's ids were rendered with; null: not told, and then a paint that is a gradient at rest is left alone. */
    const prefix = typeof opts?.idPrefix === 'string' ? cleanIdPrefix(opts.idPrefix) : null
    /** The reference to a gradient as it was rendered. `id` is one of the look's own ids (see `restOf`). */
    const gradient = (id: string): string => `url(#${prefix ?? ''}${id})`

    const nodes = new Map<string, EngineNode>()
    for (const id of full.targets.keys()) {
      let node: EngineNode | null = null
      try {
        node = find(id)
      } catch {
        node = null
      }
      if (node && typeof node.setAttribute === 'function' && typeof node.removeAttribute === 'function') nodes.set(id, node)
    }

    /** What each node holds now, as far as this figure wrote it. Starts as the artwork's own pose, which is what was rendered. */
    const written = new Map<string, { transform: string | undefined; attrs: Map<string, string | null> }>()
    for (const [id, pose] of full.rest) {
      const attrs = new Map(pose.attrs)
      if (prefix !== null && pose.refs) for (const [name, ref] of pose.refs) attrs.set(name, gradient(ref))
      written.set(id, { transform: pose.transform, attrs })
    }

    /** Between what the frame adds up and what it writes: what this figure writes cannot flash. */
    const limit = createLimiter(full)

    const members = new Map<string, Member>()
    let gestures: Gesture[] = []
    let state: AvatarState | null = null
    let reduced = opts?.reducedMotion === true
    let mouthRead: (() => number) | null = null
    let gazeRead: (() => readonly [number, number] | null) | null = null
    let walkRead: (() => WalkInputs | null) | null = null
    let mouthOpen = 0
    let mouthUntil = -1
    let gazeX = 0
    let gazeY = 0
    let aimX = 0
    let aimY = 0
    let glanceAt = clock + 2000 + random() * 3000
    let glanceEnd = 0
    let pace = 0
    let turn = 0
    let held = 0
    /** No more frames: detached, frozen, or the engine is gone. */
    let stopped = false
    /** The artwork's own values were put back. */
    let restored = false
    /** It stopped by itself: its frames cost too much, or one of them threw. */
    let froze = false
    let spent = 0
    let frames = 0

    const weightOf = (m: Member, now: number) => m.from + (m.to - m.from) * clamp01((now - m.fadeAt) / STATE_FADE_MS)

    const write = (poses: ReadonlyMap<string, Pose>, careful: boolean) => {
      for (const [id, pose] of poses) {
        const node = nodes.get(id)
        const was = written.get(id)
        if (!node || !was) continue
        try {
          if (pose.transform !== was.transform) {
            if (pose.transform === undefined) node.removeAttribute('transform')
            else node.setAttribute('transform', pose.transform)
            was.transform = pose.transform
          }
          const atRest = prefix === null ? full.rest.get(id)?.refs : undefined
          for (const [name, value] of pose.attrs) {
            if (was.attrs.get(name) === value) continue
            // Without the prefix a gradient could not be put back: such a paint keeps what was rendered.
            if (atRest?.has(name)) continue
            if (value === null) node.removeAttribute(name)
            else node.setAttribute(name, value)
            was.attrs.set(name, value)
          }
          if (prefix !== null && pose.refs) {
            for (const [name, ref] of pose.refs) {
              const value = gradient(ref)
              if (was.attrs.get(name) === value) continue
              node.setAttribute(name, value)
              was.attrs.set(name, value)
            }
          }
        } catch (e) {
          // While restoring, one node that throws must not keep the others from their rest pose.
          if (!careful) throw e
        }
      }
    }

    const figure: Figure = {
      frame(now, dt) {
        if (stopped) return
        let cost = 0
        try {
          const t0 = host.now()
          const clips: Clip[] = []

          // State animations: fades, the schedule of recurring ones, one-shots that have played.
          for (const m of members.values()) {
            const w = weightOf(m, now)
            if (m.to === 0 && w <= 0) {
              members.delete(m.name)
              continue
            }
            const anim = full.anims.get(m.name)
            if (!anim || m.spent) continue
            const lasts = anim.durationMs + anim.maxDelayMs + STATE_FADE_MS
            if (anim.every && now > m.startMs + lasts) {
              // Seconds between starts, and never before the last pass has faded.
              const gap = (anim.every[0] + random() * (anim.every[1] - anim.every[0])) * 1000
              m.startMs = Math.max(m.startMs + Math.max(gap, lasts), now)
            } else if (!anim.loop && !anim.every && now > m.startMs + lasts) {
              m.spent = true
              continue
            }
            clips.push({ name: m.name, startMs: m.startMs, weight: w, mode: 'state', times: 1 })
          }

          // Gestures: a stopped one gives way over 150 ms, a played one is dropped.
          gestures = gestures.filter((g) => {
            const anim = full.anims.get(g.name)
            if (!anim) return false
            if (g.stopAt === null && g.mode !== 'once' && now - g.startMs > HOLD_MAX_MS) g.stopAt = now
            if (g.stopAt !== null) return now - g.stopAt < GESTURE_BLEND_MS && !reduced
            if (g.mode !== 'once') return true
            return now - g.startMs <= anim.durationMs * g.times + anim.maxDelayMs + (reduced ? 0 : GESTURE_BLEND_MS)
          })
          for (const g of gestures) clips.push({ name: g.name, startMs: g.startMs, weight: g.stopAt === null ? 1 : 1 - (now - g.stopAt) / GESTURE_BLEND_MS, mode: g.mode, times: g.times })

          // The mouth: only while a line is presented and for a moment after it.
          let mouth: number | null = null
          if (full.mouth.length > 0 || full.readsMouth) {
            let want = -1
            if (mouthRead) {
              try {
                const v = mouthRead()
                if (isNum(v) && v >= 0) want = Math.min(v, 1)
              } catch {
                want = -1
              }
            }
            if (want >= 0) mouthUntil = now + MOUTH_TAIL_MS
            if (now < mouthUntil) {
              mouthOpen = easeMouth(mouthOpen, want >= 0 ? want : 0, dt)
              mouth = mouthOpen
            } else {
              mouthOpen = 0
            }
          }

          // The gaze: where the app says, else a small glance now and then.
          let gaze: readonly [number, number] | null = null
          if ((full.gazeTarget || full.readsLook) && !reduced) {
            let told: readonly [number, number] | null = null
            if (gazeRead) {
              try {
                const v = gazeRead()
                if (Array.isArray(v) && isNum(v[0]) && isNum(v[1])) told = [v[0], v[1]]
              } catch {
                told = null
              }
            }
            if (told) {
              const length = Math.hypot(told[0], told[1])
              aimX = length > 1 ? told[0] / length : told[0]
              aimY = length > 1 ? told[1] / length : told[1]
              glanceAt = now + 2000 + random() * 3000
              glanceEnd = 0
            } else {
              if (now >= glanceEnd) {
                aimX = 0
                aimY = 0
              }
              if (now >= glanceAt) {
                const angle = random() * Math.PI * 2
                const far = 0.35 + random() * 0.4
                aimX = Math.cos(angle) * far
                aimY = Math.sin(angle) * far * 0.6
                glanceEnd = now + 500 + random() * 900
                glanceAt = now + 2000 + random() * 3000
              }
            }
            const k = dt > 0 ? 1 - Math.exp(-dt / GAZE_EASE_MS) : 0
            gazeX += (aimX - gazeX) * k
            gazeY += (aimY - gazeY) * k
            gaze = [gazeX, gazeY]
          }

          // The walk: towards what the overlay says, slowly; at rest with reduced motion.
          if (full.drives.length > 0) {
            let want: WalkInputs | null = null
            if (walkRead && !reduced) {
              try {
                const v = walkRead()
                if (typeof v === 'object' && v !== null) want = v
              } catch {
                want = null
              }
            }
            const within = (v: unknown, lo: number) => (isNum(v) ? Math.min(Math.max(v, lo), 1) : 0)
            if (reduced) {
              pace = 0
              turn = 0
              held = 0
            } else {
              pace = easeInput(pace, within(want?.pace, 0), dt)
              turn = easeInput(turn, within(want?.turn, -1), dt)
              held = easeInput(held, within(want?.held, 0), dt)
            }
          }

          // Added up, limited (no channel turns back sooner than REVERSAL_MS after it last turned), written.
          const numbers = accumulate(full, clips, now, { mouth, gaze, pace, turn, held }, { reducedMotion: reduced })
          limit(numbers, now)
          write(settle(full, compiled.look, numbers), false)
          cost = host.now() - t0
        } catch {
          // A figure that throws is taken out of the loop and stays static; the others go on.
          freeze()
          return
        }
        // Cost safety: a look whose frames are too dear is frozen in its rest pose.
        if (isNum(cost) && cost > 0) spent += cost
        if (++frames >= BUDGET_FRAMES) {
          if (spent / frames > BUDGET_MS) freeze()
          spent = 0
          frames = 0
        }
      },
      end() {
        stopped = true
        if (restored) return
        restored = true
        write(full.rest, true)
      },
    }

    const out = () => {
      figures.delete(figure)
      if (figures.size === 0) halt()
    }
    const leave = () => {
      figure.end()
      out()
    }
    // Frozen, it stands as a still face does: in what the drives set at rest. Detached, it is the artwork again.
    const freeze = () => {
      if (stopped) return
      froze = true
      stopped = true
      out()
      write(sample(compiled, [], 0, {}), true)
    }

    const enter = (next: AvatarState, first: boolean) => {
      const names = stateList(compiled.look, next).filter((name) => full.anims.has(name))
      for (const m of members.values()) {
        if (names.includes(m.name) || m.to === 0) continue
        m.from = weightOf(m, clock)
        m.to = 0
        m.fadeAt = clock
      }
      for (const name of names) {
        const anim = full.anims.get(name)
        if (!anim) continue
        const had = members.get(name)
        if (had) {
          // Named in both lists: the same instance goes on, same phase. One that was leaving comes back.
          if (had.to === 0) {
            had.from = weightOf(had, clock)
            had.to = 1
            had.fadeAt = clock
          }
          continue
        }
        const once = !anim.loop && !anim.every
        members.set(name, {
          name,
          // A recurring animation first starts after a random wait; a one-shot plays now, unless the page loaded into this state.
          startMs: anim.every ? clock + random() * anim.every[1] * 1000 : clock,
          from: once ? 1 : 0,
          to: 1,
          fadeAt: clock,
          spent: once && first,
        })
      }
      state = next
    }

    enter(opts?.state ?? 'idle', true)
    figures.add(figure)
    request()

    return {
      setState(s) {
        if (stopped || s === state) return
        enter(s, false)
      },
      play(name, mode = 'once', times = 1) {
        if (stopped || typeof name !== 'string') return false
        const anim = full.anims.get(name)
        if (!anim) return false
        // Only an animation declared as a loop can loop as a gesture; anything else plays once.
        const how = mode === 'hold' ? 'hold' : mode === 'loop' && anim.loop ? 'loop' : 'once'
        // A held or looping gesture ends at the next play, and so does an earlier play of the same animation.
        for (const g of gestures) if (g.stopAt === null && (g.mode !== 'once' || g.name === name)) g.stopAt = clock
        gestures.push({ name, mode: how, times: Math.min(Math.max(isNum(times) ? Math.floor(times) : 1, 1), 5), startMs: clock, stopAt: null })
        if (gestures.length > MAX_GESTURES) gestures = gestures.slice(-MAX_GESTURES)
        return true
      },
      stop(name) {
        for (const g of gestures) if (g.stopAt === null && (name === 'all' || g.name === name)) g.stopAt = clock
      },
      setMouth(read) {
        mouthRead = typeof read === 'function' ? read : null
      },
      setGaze(read) {
        gazeRead = typeof read === 'function' ? read : null
      },
      setInputs(read) {
        walkRead = typeof read === 'function' ? read : null
      },
      setReducedMotion(on) {
        reduced = on === true
      },
      frozen: () => froze,
      detach: leave,
    }
  }

  return {
    attach,
    setRunning(on) {
      if (disposed) return
      running = on === true
      if (running) request()
      else halt()
    },
    dispose() {
      if (disposed) return
      for (const figure of figures) figure.end()
      figures.clear()
      halt()
      disposed = true
    },
  }
}

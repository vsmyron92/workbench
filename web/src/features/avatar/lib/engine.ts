// The animation engine: a look's keyframe tracks, sampled into attribute values.
//
// Three layers, each usable without the next:
// - `compile` turns every track of a look (the agent's and the built-in ones) into
//   numbers: colours into RGBA, path data into a command template plus numbers. A value
//   that does not fit its prop rejects the look.
// - `sample` is pure: clips in, the pose of every part out. All of the animation rules
//   live here (composition, weights, gestures over state animations, fades, mouth, gaze).
// - `createEngine` runs one shared frame loop per window and writes poses to nodes. It
//   counts visible time only, runs at most 30 frames a second, catches per figure, and
//   freezes a figure whose frames cost too much.
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
import { LIMITS, MORE_LIMITS, cleanIdPrefix, fmt, formatPath, formatTransform, isColourProp, isId, isTransformProp, lookIndex, parseHex, parsePath, parseTransform, propFits, samePathShape, trackEases, trackTimes, viewBoxOf } from './look'
import type { IndexEntry, PathShape, Rgba } from './look'
import type { Animation, AvatarState, Ease, Look, LookNode, MouthDrive, Prop, Track } from './types'

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

export interface SampleOptions {
  /** No looping or recurring animations and no gaze; gestures and one-shots show their last frame; the mouth still moves. */
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
  setReducedMotion(on: boolean): void
  /** The figure stopped animating: its frames cost too much, or one of its nodes threw. It stays in its rest pose. */
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
const GAZE_EASE_MS = 90

// ---------------------------------------------------------------- compiled form

type Kind = 'rel' | 'num' | 'colour' | 'd'

/** The keyframes of a track for one target, nulls resolved. Exactly one of the lists is used, by kind. */
interface Frames {
  nums: readonly number[]
  colours: readonly Rgba[]
  paths: readonly (readonly number[])[]
}

interface CTrack {
  prop: Prop
  kind: Kind
  /** 0 tx, 1 ty, 2 rotation, 3 sx, 4 sy: the channels a relative prop feeds. */
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
  /** Absolute props something may animate on this part, with the artwork's own value. */
  rest: Map<Prop, Rest>
  /** Whether something may change its transform. */
  moves: boolean
}

type CMouth = { target: string; kind: 'd'; shape: PathShape; closed: readonly number[]; open: readonly number[] } | { target: string; kind: 'channel'; channels: readonly number[]; closed: number; open: number } | { target: string; kind: 'attr'; prop: Prop; closed: number; open: number }

interface Full {
  anims: ReadonlyMap<string, CAnim>
  targets: ReadonlyMap<string, CTarget>
  mouth: readonly CMouth[]
  gazeTarget: string | null
  gazeTravel: number
  /** The pose with nothing playing: the artwork's own values, as the engine writes them back. */
  rest: ReadonlyMap<string, Pose>
}

const FULL = new WeakMap<Compiled, Full>()
const COMPILED = new WeakMap<Look, Compiled | null>()

/** Prop -> the DOM attribute the engine writes. A literal table: no attribute name ever comes from a look. */
const DOM_ATTR: ReadonlyMap<Prop, string> = new Map<Prop, string>([
  ['opacity', 'opacity'],
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
  ['scale', [3, 4]],
  ['scaleX', [3]],
  ['scaleY', [4]],
])

const EASES: ReadonlySet<string> = new Set(['linear', 'in', 'out', 'inOut', 'outBack', 'step'])

const clamp01 = (n: number) => (n < 0 ? 0 : n > 1 ? 1 : n)
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v)

/** One eased step of a segment, 0 to 1 in. `outBack` overshoots; `step` holds the start until the end. */
export function ease(kind: Ease, t: number): number {
  switch (kind) {
    case 'in':
      return t * t
    case 'out':
      return 1 - (1 - t) * (1 - t)
    case 'inOut':
      return t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2
    case 'outBack': {
      const c1 = 1.70158
      const u = t - 1
      return 1 + (c1 + 1) * u * u * u + c1 * u * u
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
  return prop === 'opacity' || prop === 'strokeWidth' ? 1 : 0
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
  const from = prop === 'strokeWidth' ? inherited(entry, [prop]) : null
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

/** The part an animation, the mouth or the gaze names: it must be drawn (not a gradient, a clip path or what is inside one). */
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
    target = {
      own: (ops ? formatTransform(ops) : null) ?? '',
      pivotX: fmt(px, 3) ?? misfit(),
      pivotY: fmt(py, 3) ?? misfit(),
      negX: fmt(-px, 3) ?? misfit(),
      negY: fmt(-py, 3) ?? misfit(),
      rest: new Map(),
      moves: false,
    }
    b.targets.set(id, target)
  }
  return { target, entry }
}

function compileTrack(b: Builder, track: Track, animation: Animation): CTrack | null {
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
  if (eases.length !== n - 1 || !eases.every((e) => EASES.has(e))) return misfit()

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
  const mouth: CMouth[] = []
  const drives: unknown = look.mouth
  if (Array.isArray(drives)) {
    if (drives.length > MORE_LIMITS.mouthDrives) return misfit()
    for (const drive of drives as MouthDrive[]) mouth.push(compileMouth(b, drive))
  }
  let gazeTarget = rolePart(look, 'pupils')
  const travel = gazeTravel(look)
  if (gazeTarget && travel > 0) targetOf(b, gazeTarget).target.moves = true
  else gazeTarget = null
  return { anims, targets: b.targets, mouth, gazeTarget, gazeTravel: travel, rest: new Map() }
}

/**
 * Every track of a look turned into numbers, the built-in animations its roles allow
 * included. null when a value does not fit its prop (a string where a number belongs, path
 * keyframes of different structure, a target that does not exist): the look cannot be
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
    full.rest = sample(compiled, [], 0, null, null)
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
  /** tx, ty, rotation, sx, sy. */
  ch: [number, number, number, number, number]
  abs: Map<Prop, Abs> | null
}

const lerp = (a: number, b: number, t: number) => a + (b - a) * t

function lerpNums(a: readonly number[], b: readonly number[], t: number, shape: PathShape): number[] {
  const out: number[] = []
  for (let i = 0; i < a.length; i++) out.push(shape.flags[i] ? a[i] : lerp(a[i], b[i] ?? a[i], t))
  return out
}

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
  return track.kind === 'rel' ? v : { kind: 'num', v }
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

function absText(value: Abs): string | null {
  if (value.kind === 'num') return fmt(value.v, 3)
  if (value.kind === 'colour') return colourText(value.v)
  return formatPath(value.shape, value.v)
}

/**
 * The pose of every part a look can move, at one moment. Pure: the same arguments give the
 * same poses.
 *
 * - State clips add up: translations and rotations add, scales multiply, and for every
 *   other prop the clip started last wins. A weight `w` applies as `t*w`, `r*w` and
 *   `1 + (s-1)*w`.
 * - A gesture's value replaces that sum for the props it animates, blended in over 150 ms
 *   and, when a `once` gesture has played, out again over 150 ms.
 * - A one-shot in a state list leaves its relative props fading for 200 ms after its last frame.
 * - The mouth (`mouthOpen` 0 to 1; null: not applied) and the gaze (`gaze`: a vector of
 *   length 0 to 1; null: none) come last.
 *
 * A part whose numbers are not finite is left out of the result. A clip whose name means
 * nothing for the look is ignored.
 */
export function sample(c: Compiled, clips: readonly Clip[], tMs: number, mouthOpen: number | null, gaze: readonly [number, number] | null, opts?: SampleOptions): Map<string, Pose> {
  const out = new Map<string, Pose>()
  const full = FULL.get(c)
  if (!full || !isNum(tMs)) return out
  const reduced = opts?.reducedMotion === true
  const accs = new Map<string, Acc>()
  const acc = (id: string): Acc => {
    let a = accs.get(id)
    if (!a) {
      a = { ch: [0, 0, 0, 1, 1], abs: null }
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
          if (ch < 3) a.ch[ch] += v * gain
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
          m = { v: [0, 0, 0, 1, 1], w: [0, 0, 0, 0, 0] }
          moved.set(id, m)
        }
        for (const ch of track.channels) {
          if (ch < 3) m.v[ch] += v
          else m.v[ch] *= v
          m.w[ch] = Math.max(m.w[ch], w)
        }
      }
    }
    for (const [id, m] of moved) {
      const a = acc(id)
      for (let ch = 0; ch < 5; ch++) if (m.w[ch] > 0) a.ch[ch] = lerp(a.ch[ch], m.v[ch], m.w[ch])
    }
  }

  // The mouth, then the gaze: applied last.
  if (isNum(mouthOpen)) {
    const m = clamp01(mouthOpen)
    for (const drive of full.mouth) {
      if (drive.kind === 'd') setAbs(drive.target, 'd', { kind: 'd', v: lerpNums(drive.closed, drive.open, m, drive.shape), shape: drive.shape }, 1)
      else if (drive.kind === 'attr') setAbs(drive.target, drive.prop, { kind: 'num', v: lerp(drive.closed, drive.open, m) }, 1)
      else for (const ch of drive.channels) acc(drive.target).ch[ch] = lerp(drive.closed, drive.open, m)
    }
  }
  if (!reduced && full.gazeTarget && Array.isArray(gaze) && isNum(gaze[0]) && isNum(gaze[1])) {
    const length = Math.hypot(gaze[0], gaze[1])
    const k = full.gazeTravel / (length > 1 ? length : 1)
    const a = acc(full.gazeTarget)
    a.ch[0] += gaze[0] * k
    a.ch[1] += gaze[1] * k
  }

  // Every part the look can move gets a pose, so one that is no longer animated returns to the artwork's own values.
  for (const [id, target] of full.targets) {
    const a = accs.get(id)
    const pose: Pose = { attrs: new Map() }
    let sound = true
    if (target.moves) {
      const parts: string[] = target.own ? [target.own] : []
      if (a) {
        const tx = fmt(a.ch[0], 2)
        const ty = fmt(a.ch[1], 2)
        const rot = fmt(a.ch[2], 2)
        const sx = fmt(a.ch[3], 4)
        const sy = fmt(a.ch[4], 4)
        if (tx === null || ty === null || rot === null || sx === null || sy === null) sound = false
        else {
          if (tx !== '0' || ty !== '0') parts.push(`translate(${tx} ${ty})`)
          if (rot !== '0') parts.push(`rotate(${rot} ${target.pivotX} ${target.pivotY})`)
          if (sx !== '1' || sy !== '1') parts.push(`translate(${target.pivotX} ${target.pivotY}) scale(${sx} ${sy}) translate(${target.negX} ${target.negY})`)
        }
      }
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
      const text = absText(value)
      if (text === null) sound = false
      else pose.attrs.set(name, text)
    }
    if (sound) out.set(id, pose)
  }
  return out
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

    const members = new Map<string, Member>()
    let gestures: Gesture[] = []
    let state: AvatarState | null = null
    let reduced = opts?.reducedMotion === true
    let mouthRead: (() => number) | null = null
    let gazeRead: (() => readonly [number, number] | null) | null = null
    let mouthOpen = 0
    let mouthUntil = -1
    let gazeX = 0
    let gazeY = 0
    let aimX = 0
    let aimY = 0
    let glanceAt = clock + 2000 + random() * 3000
    let glanceEnd = 0
    /** No more frames: detached, frozen, or the engine is gone. */
    let stopped = false
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
          if (full.mouth.length > 0) {
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
          if (full.gazeTarget && !reduced) {
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

          write(sample(compiled, clips, now, mouth, gaze, { reducedMotion: reduced }), false)
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
        if (stopped) return
        stopped = true
        write(full.rest, true)
      },
    }

    const leave = () => {
      figure.end()
      figures.delete(figure)
      if (figures.size === 0) halt()
    }
    const freeze = () => {
      if (stopped) return
      froze = true
      leave()
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

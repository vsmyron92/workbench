// The look: checking it and turning it into React elements.
//
// An agent's artwork never reaches the DOM as markup. The server parses it into a closed,
// typed, bounded tree; here the browser checks that tree again (`checkLook` builds a fresh
// object from allow-listed keys, anything off rejects the whole look) and builds React
// elements from literal tag and attribute tables (`lookElements`). No `style`, no
// `className`, no `dangerouslySetInnerHTML`, no tag or attribute name chosen by data, and
// every lookup keyed by data goes through a `Map`, a `Set` or a `switch` built from literals,
// never through property access on a plain object.
//
// React-free except for `createElement`; reads no globals.

import { createElement, type ReactNode } from 'react'
import { AVATAR_STATES, ROLES } from './types'
import type { Animation, AvatarState, Box, Ease, Look, LookNode, MouthDrive, MouthProp, Prop, Role, Tag, Track } from './types'

/**
 * The server's limits (`pub const MAX_*` in server/src/avatar/look.rs), repeated so a look
 * that somehow passed them is still bounded here. A test holds the two lists together.
 */
export const LIMITS: Readonly<Record<'elements' | 'depth' | 'animations' | 'tracks' | 'keyframes' | 'dChars' | 'pointsChars' | 'texts' | 'textChars' | 'stops' | 'attrs', number>> =
  Object.freeze({ elements: 300, depth: 10, animations: 32, tracks: 16, keyframes: 16, dChars: 4000, pointsChars: 2000, texts: 6, textChars: 40, stops: 16, attrs: 32 })

/**
 * Further limits of look.rs that the list above does not name: `MAX_TARGETS`,
 * `MAX_D_KEYFRAME_CHARS`, `MAX_D_KEYFRAMES_TOTAL`, `MAX_STATE_NAMES`, `MAX_MOUTH_DRIVES`
 * and `MAX_TRANSFORMS`. The limits hold for a look as it is stored (the server measures a
 * path as it was written and again as it is stored), so they are kept exactly here.
 */
export const MORE_LIMITS: Readonly<Record<'targets' | 'dKeyframeChars' | 'dKeyframesTotal' | 'stateNames' | 'mouthDrives' | 'transforms', number>> = Object.freeze({
  targets: 16,
  dKeyframeChars: 1000,
  dKeyframesTotal: 6000,
  stateNames: 6,
  mouthDrives: 4,
  transforms: 8,
})
/** The server reads at most this much of one transform; what it writes back is shorter. */
const MAX_TRANSFORM_CHARS = 1024
/** How far the pupils may travel at most, as a part of the viewBox width. */
const GAZE_MOST = 0.05
/** No number of a look is larger than this; the server's own bound (4 x the viewBox) is far inside it. */
const NUM_MAX = 1e6
const VIEWBOX_MIN = 16
const VIEWBOX_MAX = 4096
/** Paint and opacity keyframes are at least this far apart (flash safety), every other keyframe pair this far. */
const PAINT_STEP_S = 0.17
const MOTION_STEP_S = 0.04
const TIME_STEP = 0.001

// ---------------------------------------------------------------- names

const ID = /^[A-Za-z][A-Za-z0-9_-]{0,31}$/
const RESERVED: ReadonlySet<string> = new Set(['constructor', 'prototype', 'toString', 'toLocaleString', 'valueOf', 'hasOwnProperty', 'isPrototypeOf', 'propertyIsEnumerable', 'all', 'none'])

/** An id or an animation name: the pattern, and none of the names every object inherits. */
export function isId(value: unknown): value is string {
  return typeof value === 'string' && ID.test(value) && !RESERVED.has(value)
}

const TAGS: ReadonlyMap<string, Tag> = new Map<string, Tag>([
  ['g', 'g'],
  ['path', 'path'],
  ['circle', 'circle'],
  ['ellipse', 'ellipse'],
  ['rect', 'rect'],
  ['line', 'line'],
  ['polyline', 'polyline'],
  ['polygon', 'polygon'],
  ['text', 'text'],
  ['defs', 'defs'],
  ['linearGradient', 'linearGradient'],
  ['radialGradient', 'radialGradient'],
  ['stop', 'stop'],
  ['clipPath', 'clipPath'],
])

const SHAPES: readonly Tag[] = ['path', 'circle', 'ellipse', 'rect', 'line', 'polyline', 'polygon']
const DRAWN: readonly Tag[] = ['g', ...SHAPES, 'text']
/** What an animation, a role, a pivot or the mouth may name. */
const ANIMATABLE: ReadonlySet<Tag> = new Set(DRAWN)
const NONE: ReadonlySet<Tag> = new Set()

/** The content model: which tags an element may hold (`root`: the children of the app's `<svg>`). */
const CHILDREN: ReadonlyMap<Tag | 'root', ReadonlySet<Tag>> = new Map<Tag | 'root', ReadonlySet<Tag>>([
  ['root', new Set<Tag>([...DRAWN, 'defs'])],
  ['g', new Set<Tag>([...DRAWN, 'defs'])],
  ['defs', new Set<Tag>(['linearGradient', 'radialGradient', 'clipPath'])],
  ['linearGradient', new Set<Tag>(['stop'])],
  ['radialGradient', new Set<Tag>(['stop'])],
  ['clipPath', new Set<Tag>(SHAPES)],
  ['path', NONE],
  ['circle', NONE],
  ['ellipse', NONE],
  ['rect', NONE],
  ['line', NONE],
  ['polyline', NONE],
  ['polygon', NONE],
  ['text', NONE],
  ['stop', NONE],
])

// ---------------------------------------------------------------- values

type Kind = 'num' | 'numPct' | 'paint' | 'stopColor' | 'gradRef' | 'clipRef' | 'transform' | 'd' | 'points' | 'cap' | 'join' | 'rule' | 'weight' | 'anchor' | 'baseline' | 'units' | 'spread' | 'token'

const HEX = /^#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/
const PERCENT = /^-?\d{1,4}(?:\.\d{1,3})?%$/
const NUMBER = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/

const CAPS: ReadonlySet<string> = new Set(['butt', 'round', 'square'])
const JOINS: ReadonlySet<string> = new Set(['miter', 'round', 'bevel'])
const RULES: ReadonlySet<string> = new Set(['nonzero', 'evenodd'])
const WEIGHTS: ReadonlySet<string> = new Set(['normal', 'bold', '100', '200', '300', '400', '500', '600', '700', '800', '900'])
const ANCHORS: ReadonlySet<string> = new Set(['start', 'middle', 'end'])
const BASELINES: ReadonlySet<string> = new Set(['auto', 'middle', 'central', 'hanging', 'alphabetic'])
const UNITS: ReadonlySet<string> = new Set(['userSpaceOnUse', 'objectBoundingBox'])
const SPREADS: ReadonlySet<string> = new Set(['pad', 'reflect', 'repeat'])

/**
 * The app's colour tokens a default look may use (theme/tokens.css). Printed as `var(--…)`
 * from this literal table; server data never carries them and `checkLook` rejects them.
 */
export const TOKENS: ReadonlyMap<string, string> = new Map([
  ['plot-1', 'var(--plot-1)'],
  ['plot-2', 'var(--plot-2)'],
  ['plot-3', 'var(--plot-3)'],
  ['plot-4', 'var(--plot-4)'],
  ['plot-5', 'var(--plot-5)'],
  ['plot-6', 'var(--plot-6)'],
  ['plot-7', 'var(--plot-7)'],
  ['plot-8', 'var(--plot-8)'],
  ['fg', 'var(--fg)'],
  ['fg-muted', 'var(--fg-muted)'],
  ['bg', 'var(--bg)'],
  ['bg-elevated', 'var(--bg-elevated)'],
  ['bg-panel', 'var(--bg-panel)'],
  ['accent', 'var(--accent)'],
])

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && Math.abs(value) <= NUM_MAX
}

/** Negative zero prints as "-0": store plain zero. */
const tidy = (n: number): number => (n === 0 ? 0 : n)

/** A value of one attribute kind as it may be stored, or undefined when it is anything else. */
function attrValue(kind: Kind, value: unknown): string | number | undefined {
  if (typeof value === 'number') {
    switch (kind) {
      case 'num':
      case 'numPct':
        return finite(value) ? tidy(value) : undefined
      case 'weight':
        return WEIGHTS.has(String(value)) ? value : undefined
      default:
        return undefined
    }
  }
  if (typeof value !== 'string') return undefined
  switch (kind) {
    case 'num':
      return undefined
    case 'numPct':
      return PERCENT.test(value) ? value : undefined
    case 'paint':
      return HEX.test(value) || value === 'none' || value === 'currentColor' ? value : undefined
    case 'stopColor':
      return HEX.test(value) || value === 'currentColor' ? value : undefined
    case 'gradRef':
    case 'clipRef':
      return isId(value) ? value : undefined
    case 'transform':
      return parseTransform(value) ? value : undefined
    case 'd':
      return value.length <= LIMITS.dChars && parsePath(value) ? value : undefined
    case 'points':
      return value.length <= LIMITS.pointsChars && numberList(value) ? value : undefined
    case 'cap':
      return CAPS.has(value) ? value : undefined
    case 'join':
      return JOINS.has(value) ? value : undefined
    case 'rule':
      return RULES.has(value) ? value : undefined
    case 'weight':
      return WEIGHTS.has(value) ? value : undefined
    case 'anchor':
      return ANCHORS.has(value) ? value : undefined
    case 'baseline':
      return BASELINES.has(value) ? value : undefined
    case 'units':
      return UNITS.has(value) ? value : undefined
    case 'spread':
      return SPREADS.has(value) ? value : undefined
    case 'token':
      return TOKENS.has(value) ? value : undefined
  }
}

/** Numbers separated by white space or commas, and nothing else. */
function numberList(text: string): boolean {
  const trimmed = text.trim()
  if (trimmed === '') return true
  for (const token of trimmed.split(/[\s,]+/)) {
    if (!NUMBER.test(token) || !finite(Number(token))) return false
  }
  return true
}

// ---------------------------------------------------------------- attribute tables

/** Attributes of everything that is drawn (g, shapes, text). The token attributes are the app's own. */
const PRESENTATION: readonly (readonly [string, Kind])[] = [
  ['transform', 'transform'],
  ['fill', 'paint'],
  ['fillRef', 'gradRef'],
  ['fillToken', 'token'],
  ['stroke', 'paint'],
  ['strokeRef', 'gradRef'],
  ['strokeToken', 'token'],
  ['strokeWidth', 'num'],
  ['strokeLinecap', 'cap'],
  ['strokeLinejoin', 'join'],
  ['strokeMiterlimit', 'num'],
  ['fillOpacity', 'numPct'],
  ['strokeOpacity', 'numPct'],
  ['opacity', 'numPct'],
  ['fillRule', 'rule'],
  ['clipRule', 'rule'],
  ['clipPathRef', 'clipRef'],
]

const GRADIENT: readonly (readonly [string, Kind])[] = [
  ['gradientUnits', 'units'],
  ['gradientTransform', 'transform'],
  ['spreadMethod', 'spread'],
]

const drawn = (own: readonly (readonly [string, Kind])[]): ReadonlyMap<string, Kind> => new Map<string, Kind>([...own, ...PRESENTATION])

/** Tag -> attribute key (the React SVG prop name) -> what its value must look like. */
const ATTRS: ReadonlyMap<Tag, ReadonlyMap<string, Kind>> = new Map<Tag, ReadonlyMap<string, Kind>>([
  ['g', drawn([])],
  ['path', drawn([['d', 'd']])],
  ['circle', drawn([['cx', 'num'], ['cy', 'num'], ['r', 'num']])],
  ['ellipse', drawn([['cx', 'num'], ['cy', 'num'], ['rx', 'num'], ['ry', 'num']])],
  ['rect', drawn([['x', 'num'], ['y', 'num'], ['width', 'num'], ['height', 'num'], ['rx', 'num'], ['ry', 'num']])],
  ['line', drawn([['x1', 'num'], ['y1', 'num'], ['x2', 'num'], ['y2', 'num']])],
  ['polyline', drawn([['points', 'points']])],
  ['polygon', drawn([['points', 'points']])],
  ['text', drawn([['x', 'num'], ['y', 'num'], ['fontSize', 'num'], ['fontWeight', 'weight'], ['textAnchor', 'anchor'], ['dominantBaseline', 'baseline'], ['letterSpacing', 'num']])],
  ['defs', new Map<string, Kind>()],
  ['linearGradient', new Map<string, Kind>([['x1', 'numPct'], ['y1', 'numPct'], ['x2', 'numPct'], ['y2', 'numPct'], ...GRADIENT])],
  ['radialGradient', new Map<string, Kind>([['cx', 'numPct'], ['cy', 'numPct'], ['r', 'numPct'], ['fx', 'numPct'], ['fy', 'numPct'], ...GRADIENT])],
  ['stop', new Map<string, Kind>([['offset', 'numPct'], ['stopColor', 'stopColor'], ['stopColorToken', 'token'], ['stopOpacity', 'numPct']])],
  ['clipPath', new Map<string, Kind>([['clipPathUnits', 'units']])],
])

// ---------------------------------------------------------------- transforms

export type TransformFn = 'translate' | 'rotate' | 'scale' | 'skewX' | 'skewY' | 'matrix'
export interface TransformOp {
  fn: TransformFn
  args: number[]
}

/** Function name -> the argument counts it takes. */
const TRANSFORM_FNS: ReadonlyMap<string, { fn: TransformFn; counts: readonly number[] }> = new Map([
  ['translate', { fn: 'translate' as const, counts: [1, 2] }],
  ['rotate', { fn: 'rotate' as const, counts: [1, 3] }],
  ['scale', { fn: 'scale' as const, counts: [1, 2] }],
  ['skewX', { fn: 'skewX' as const, counts: [1] }],
  ['skewY', { fn: 'skewY' as const, counts: [1] }],
  ['matrix', { fn: 'matrix' as const, counts: [6] }],
])

/** A transform list as numbers, or null when it is anything but `translate|rotate|scale|skewX|skewY|matrix(numbers)`. */
export function parseTransform(text: unknown): TransformOp[] | null {
  if (typeof text !== 'string' || text.length > MAX_TRANSFORM_CHARS) return null
  const ops: TransformOp[] = []
  const item = /[\s,]*([A-Za-z]+)\s*\(([^()]*)\)/y
  let at = 0
  for (;;) {
    item.lastIndex = at
    const m = item.exec(text)
    if (!m) break
    const known = TRANSFORM_FNS.get(m[1])
    if (!known) return null
    const inner = m[2].trim()
    const tokens = inner === '' ? [] : inner.split(/[\s,]+/)
    if (!known.counts.includes(tokens.length)) return null
    const args: number[] = []
    for (const token of tokens) {
      const n = Number(token)
      if (!NUMBER.test(token) || !finite(n)) return null
      args.push(tidy(n))
    }
    ops.push({ fn: known.fn, args })
    if (ops.length > MORE_LIMITS.transforms) return null
    at = item.lastIndex
  }
  return /^[\s,]*$/.test(text.slice(at)) ? ops : null
}

/**
 * A number as the engine writes it: fixed notation, no exponent, no "-0". null when the
 * value is not finite (the caller then writes nothing).
 */
export function fmt(value: number, digits: number): string | null {
  if (!Number.isFinite(value) || Math.abs(value) >= 1e15) return null
  let out = value.toFixed(digits)
  if (out.includes('.')) {
    let end = out.length
    while (out.charCodeAt(end - 1) === 48) end--
    if (out.charCodeAt(end - 1) === 46) end--
    out = out.slice(0, end)
  }
  return out === '-0' ? '0' : out
}

/** The list written back from its numbers ("" for an empty list); null when a number is not finite. */
export function formatTransform(ops: readonly TransformOp[]): string | null {
  const parts: string[] = []
  for (const op of ops) {
    const args: string[] = []
    for (const a of op.args) {
      const s = fmt(a, 6)
      if (s === null) return null
      args.push(s)
    }
    parts.push(`${op.fn}(${args.join(' ')})`)
  }
  return parts.join(' ')
}

/** An affine matrix `[a, b, c, d, e, f]`: x' = a x + c y + e, y' = b x + d y + f. */
export type Matrix = readonly [number, number, number, number, number, number]
export const IDENTITY: Matrix = [1, 0, 0, 1, 0, 0]

/** `m` after `n`: the matrix that applies `n` first. */
export function multiply(m: Matrix, n: Matrix): Matrix {
  return [m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1], m[0] * n[2] + m[2] * n[3], m[1] * n[2] + m[3] * n[3], m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5]]
}

const RAD = Math.PI / 180

function opMatrix(op: TransformOp): Matrix {
  const a = op.args
  switch (op.fn) {
    case 'translate':
      return [1, 0, 0, 1, a[0], a[1] ?? 0]
    case 'scale':
      return [a[0], 0, 0, a[1] ?? a[0], 0, 0]
    case 'rotate': {
      const cos = Math.cos(a[0] * RAD)
      const sin = Math.sin(a[0] * RAD)
      const cx = a[1] ?? 0
      const cy = a[2] ?? 0
      return [cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy]
    }
    case 'skewX':
      return [1, 0, Math.tan(a[0] * RAD), 1, 0, 0]
    case 'skewY':
      return [1, Math.tan(a[0] * RAD), 0, 1, 0, 0]
    case 'matrix':
      return [a[0], a[1], a[2], a[3], a[4], a[5]]
  }
}

/** The matrix of a whole transform list. */
export function transformMatrix(ops: readonly TransformOp[]): Matrix {
  let m: Matrix = IDENTITY
  for (const op of ops) m = multiply(m, opMatrix(op))
  return m
}

/** The box that holds `box` after `m` (its four corners). */
export function boxThrough(m: Matrix, box: Readonly<Box>): Box {
  const [x, y, w, h] = box
  let x0 = Infinity
  let y0 = Infinity
  let x1 = -Infinity
  let y1 = -Infinity
  for (const [px, py] of [
    [x, y],
    [x + w, y],
    [x, y + h],
    [x + w, y + h],
  ]) {
    const tx = m[0] * px + m[2] * py + m[4]
    const ty = m[1] * px + m[3] * py + m[5]
    x0 = Math.min(x0, tx)
    y0 = Math.min(y0, ty)
    x1 = Math.max(x1, tx)
    y1 = Math.max(y1, ty)
  }
  return [x0, y0, x1 - x0, y1 - y0]
}

// ---------------------------------------------------------------- paths

/** Path data as a command template plus its numbers. Arc flags are numbers too, marked in `flags`. */
export interface PathShape {
  /** The tokens in order: a command letter, or null where the next number goes. */
  template: readonly (string | null)[]
  nums: readonly number[]
  /** Per number: whether it is an arc flag (0 or 1, never interpolated). */
  flags: readonly boolean[]
  /** The command letters, in order: two paths morph into each other only when these are equal. */
  commands: string
}

const COMMANDS: ReadonlySet<string> = new Set('MmLlHhVvCcSsQqTtAaZz')

/** Path data split into commands and numbers, or null when it holds anything else. */
export function parsePath(text: unknown): PathShape | null {
  if (typeof text !== 'string') return null
  const template: (string | null)[] = []
  const nums: number[] = []
  const flags: boolean[] = []
  let commands = ''
  let arc = false
  let inArc = 0
  const n = text.length
  const number = /[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/y
  let at = 0
  while (at < n) {
    const ch = text[at]
    if (ch === ' ' || ch === ',' || ch === '\t' || ch === '\n' || ch === '\r') {
      at++
      continue
    }
    if (COMMANDS.has(ch)) {
      template.push(ch)
      commands += ch
      arc = ch === 'A' || ch === 'a'
      inArc = 0
      at++
      continue
    }
    if (commands === '') return null
    // The two flags of an arc are single digits and may be written without a separator.
    if (arc && (inArc % 7 === 3 || inArc % 7 === 4)) {
      if (ch !== '0' && ch !== '1') return null
      template.push(null)
      nums.push(ch === '1' ? 1 : 0)
      flags.push(true)
      inArc++
      at++
      continue
    }
    number.lastIndex = at
    const m = number.exec(text)
    if (!m) return null
    const value = Number(m[0])
    if (!finite(value)) return null
    template.push(null)
    nums.push(tidy(value))
    flags.push(false)
    if (arc) inArc++
    at = number.lastIndex
  }
  return { template, nums, flags, commands }
}

/** Whether two paths have one structure: the same letters in the same order, as many numbers, equal arc flags. */
export function samePathShape(a: PathShape, b: PathShape): boolean {
  if (a.commands !== b.commands || a.nums.length !== b.nums.length || a.template.length !== b.template.length) return false
  for (let i = 0; i < a.template.length; i++) if (a.template[i] !== b.template[i]) return false
  for (let i = 0; i < a.nums.length; i++) {
    if (a.flags[i] !== b.flags[i]) return false
    if (a.flags[i] && a.nums[i] !== b.nums[i]) return false
  }
  return true
}

/** A path written from a template and numbers; null when a number is not finite. */
export function formatPath(shape: PathShape, nums: readonly number[]): string | null {
  const out: string[] = []
  let i = 0
  for (const token of shape.template) {
    if (token !== null) {
      out.push(token)
      continue
    }
    const s = shape.flags[i] ? (nums[i] >= 0.5 ? '1' : '0') : fmt(nums[i], 3)
    if (s === null) return null
    out.push(s)
    i++
  }
  return out.join(' ')
}

// ---------------------------------------------------------------- colours

/** `[r, g, b, a]`, channels 0 to 255, alpha 0 to 1. */
export type Rgba = readonly [number, number, number, number]

/** `#rrggbb` or `#rrggbbaa` as numbers; null for anything else. */
export function parseHex(text: unknown): Rgba | null {
  if (typeof text !== 'string' || !HEX.test(text)) return null
  const n = (at: number) => Number.parseInt(text.slice(at, at + 2), 16)
  return [n(1), n(3), n(5), text.length === 9 ? n(7) / 255 : 1]
}

// ---------------------------------------------------------------- which prop fits which element

const TRANSFORM_PROPS: ReadonlySet<Prop> = new Set<Prop>(['translateX', 'translateY', 'rotate', 'scale', 'scaleX', 'scaleY'])
const COLOUR_PROPS: ReadonlySet<Prop> = new Set<Prop>(['fill', 'stroke'])

/** Prop -> the tags it can be animated on. */
const PROP_TAGS: ReadonlyMap<Prop, ReadonlySet<Tag>> = new Map<Prop, ReadonlySet<Tag>>([
  ['translateX', ANIMATABLE],
  ['translateY', ANIMATABLE],
  ['rotate', ANIMATABLE],
  ['scale', ANIMATABLE],
  ['scaleX', ANIMATABLE],
  ['scaleY', ANIMATABLE],
  ['opacity', ANIMATABLE],
  ['fill', ANIMATABLE],
  ['stroke', ANIMATABLE],
  ['strokeWidth', ANIMATABLE],
  ['d', new Set<Tag>(['path'])],
  ['cx', new Set<Tag>(['circle', 'ellipse'])],
  ['cy', new Set<Tag>(['circle', 'ellipse'])],
  ['r', new Set<Tag>(['circle'])],
  ['rx', new Set<Tag>(['ellipse', 'rect'])],
  ['ry', new Set<Tag>(['ellipse', 'rect'])],
  ['x', new Set<Tag>(['rect', 'text'])],
  ['y', new Set<Tag>(['rect', 'text'])],
  ['width', new Set<Tag>(['rect'])],
  ['height', new Set<Tag>(['rect'])],
  ['x1', new Set<Tag>(['line'])],
  ['y1', new Set<Tag>(['line'])],
  ['x2', new Set<Tag>(['line'])],
  ['y2', new Set<Tag>(['line'])],
])

const PROPS: ReadonlyMap<string, Prop> = new Map<string, Prop>([...PROP_TAGS.keys()].map((p) => [p, p]))

/** Whether `prop` can be animated on an element of `tag`. */
export function propFits(prop: Prop, tag: Tag): boolean {
  return PROP_TAGS.get(prop)?.has(tag) ?? false
}

export const isTransformProp = (prop: Prop): boolean => TRANSFORM_PROPS.has(prop)
export const isColourProp = (prop: Prop): boolean => COLOUR_PROPS.has(prop)

const MOUTH_PROPS: ReadonlyMap<string, MouthProp> = new Map<string, MouthProp>([
  ['scaleY', 'scaleY'],
  ['scaleX', 'scaleX'],
  ['scale', 'scale'],
  ['translateY', 'translateY'],
  ['ry', 'ry'],
  ['r', 'r'],
  ['height', 'height'],
])

const EASES: ReadonlyMap<string, Ease> = new Map<string, Ease>([
  ['linear', 'linear'],
  ['in', 'in'],
  ['out', 'out'],
  ['inOut', 'inOut'],
  ['outBack', 'outBack'],
  ['step', 'step'],
])

const STATE_NAMES: ReadonlyMap<string, AvatarState> = new Map<string, AvatarState>(AVATAR_STATES.map((s) => [s, s]))
const ROLE_NAMES: ReadonlyMap<string, Role> = new Map<string, Role>(ROLES.map((r) => [r, r]))

// ---------------------------------------------------------------- checkLook

/** Thrown inside the check; `checkLook` turns it into null. Carries no data. */
class Off extends Error {}
const off = (): never => {
  throw new Off()
}

/** A JSON object (never an array, a class instance or null). */
function plain(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return off()
  const proto: unknown = Object.getPrototypeOf(value)
  if (proto !== Object.prototype && proto !== null) return off()
  return value as Record<string, unknown>
}

function list(value: unknown, min: number, max: number): unknown[] {
  if (!Array.isArray(value) || value.length < min || value.length > max) return off()
  return value
}

/** A record with no prototype: a lookup by a name from data can never find an inherited member. */
function record<T>(): Record<string, T> {
  return Object.create(null) as Record<string, T>
}

function numberIn(value: unknown, min: number, max: number): number {
  if (!finite(value) || value < min || value > max) return off()
  return tidy(value)
}

const present = (value: unknown): boolean => value !== undefined && value !== null

// Characters that never belong in one line of plain text: controls, line and paragraph
// separators, and the marks that reorder text. The server removed them; their presence
// means the look did not come through it.
const NOT_PLAIN = /[\p{Cc}\p{Zl}\p{Zp}\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/u

interface Walk {
  elements: number
  texts: number
  /** Every id of the tree, with its tag. */
  ids: Map<string, Tag>
  /** Ids that an animation, a role, a pivot or the mouth may name. */
  parts: Set<string>
  gradRefs: string[]
  clipRefs: string[]
}

function checkAttrs(value: unknown, tag: Tag, inClip: boolean, w: Walk): Record<string, string | number> | undefined {
  if (!present(value)) return undefined
  const src = plain(value)
  const kinds = ATTRS.get(tag) ?? off()
  const keys = Object.keys(src)
  if (keys.length > LIMITS.attrs) return off()
  for (const key of keys) if (!kinds.has(key)) return off()
  const out = record<string | number>()
  let count = 0
  // The allow-list is walked, not the data: a key is only ever one of the literals above.
  for (const [key, kind] of kinds) {
    if (!Object.hasOwn(src, key)) continue
    // The token attributes are the app's own (the default look): data never carries them.
    if (kind === 'token') return off()
    const checked = attrValue(kind, src[key])
    if (checked === undefined) return off()
    if (kind === 'gradRef') w.gradRefs.push(checked as string)
    if (kind === 'clipRef') {
      if (inClip) return off()
      w.clipRefs.push(checked as string)
    }
    out[key] = checked
    count++
  }
  if (Object.hasOwn(out, 'fill') && Object.hasOwn(out, 'fillRef')) return off()
  if (Object.hasOwn(out, 'stroke') && Object.hasOwn(out, 'strokeRef')) return off()
  return count > 0 ? out : undefined
}

function checkBox(value: unknown): Box {
  const v = list(value, 4, 4)
  const x = numberIn(v[0], -NUM_MAX, NUM_MAX)
  const y = numberIn(v[1], -NUM_MAX, NUM_MAX)
  const width = numberIn(v[2], 0, NUM_MAX)
  const height = numberIn(v[3], 0, NUM_MAX)
  return [x, y, width, height]
}

function checkNode(value: unknown, parent: Tag | 'root', depth: number, inert: boolean, inClip: boolean, w: Walk): LookNode {
  const src = plain(value)
  if (depth > LIMITS.depth) return off()
  if (++w.elements > LIMITS.elements) return off()
  let tagValue: unknown
  let idValue: unknown
  let attrsValue: unknown
  let boxValue: unknown
  let childrenValue: unknown
  let textValue: unknown
  for (const key of Object.keys(src)) {
    switch (key) {
      case 'tag':
        tagValue = src[key]
        break
      case 'id':
        idValue = src[key]
        break
      case 'attrs':
        attrsValue = src[key]
        break
      case 'box':
        boxValue = src[key]
        break
      case 'children':
        childrenValue = src[key]
        break
      case 'text':
        textValue = src[key]
        break
      default:
        return off()
    }
  }
  const tag = (typeof tagValue === 'string' ? TAGS.get(tagValue) : undefined) ?? off()
  if (!(CHILDREN.get(parent) ?? NONE).has(tag)) return off()
  const node: LookNode = { tag }
  const nowInert = inert || tag === 'defs'
  const nowInClip = inClip || tag === 'clipPath'
  if (present(idValue)) {
    if (!isId(idValue) || w.ids.has(idValue)) return off()
    w.ids.set(idValue, tag)
    if (ANIMATABLE.has(tag) && !nowInert && !nowInClip) w.parts.add(idValue)
    node.id = idValue
  }
  const attrs = checkAttrs(attrsValue, tag, nowInClip, w)
  if (attrs) node.attrs = attrs
  if (present(boxValue)) node.box = checkBox(boxValue)
  if (present(textValue)) {
    if (tag !== 'text' || typeof textValue !== 'string') return off()
    if (textValue.length > LIMITS.textChars * 2 || [...textValue].length > LIMITS.textChars || NOT_PLAIN.test(textValue)) return off()
    node.text = textValue
  }
  if (tag === 'text' && ++w.texts > LIMITS.texts) return off()
  if (present(childrenValue)) {
    const allowed = CHILDREN.get(tag) ?? NONE
    const max = tag === 'linearGradient' || tag === 'radialGradient' ? LIMITS.stops : LIMITS.elements
    const items = list(childrenValue, 0, max)
    if (items.length > 0 && allowed.size === 0) return off()
    const children: LookNode[] = []
    for (let i = 0; i < items.length; i++) children.push(checkNode(items[i], tag, depth + 1, nowInert, nowInClip, w))
    if (children.length > 0) node.children = children
  }
  return node
}

function checkTimes(value: unknown, length: number | null): number[] {
  const v = list(value, 2, LIMITS.keyframes)
  if (length !== null && v.length !== length) return off()
  const out: number[] = []
  for (let i = 0; i < v.length; i++) {
    const t = numberIn(v[i], 0, 1)
    if (i > 0 && t - out[i - 1] < TIME_STEP - 1e-9) return off()
    out.push(t)
  }
  if (out[0] !== 0 || out[out.length - 1] !== 1) return off()
  return out
}

function checkEase(value: unknown, segments: number | null): Ease | Ease[] {
  if (typeof value === 'string') return EASES.get(value) ?? off()
  const v = list(value, 1, LIMITS.keyframes - 1)
  if (segments !== null && v.length !== segments) return off()
  const out: Ease[] = []
  for (let i = 0; i < v.length; i++) {
    const e: unknown = v[i]
    out.push((typeof e === 'string' ? EASES.get(e) : undefined) ?? off())
  }
  return out
}

function valueRange(prop: Prop): [number, number] {
  switch (prop) {
    case 'opacity':
      return [0, 1]
    case 'scale':
    case 'scaleX':
    case 'scaleY':
      return [-10, 10]
    case 'rotate':
      return [-3600, 3600]
    default:
      return [-NUM_MAX, NUM_MAX]
  }
}

interface AnimationContext {
  duration: number
  times: number[] | undefined
  ease: Ease | Ease[] | undefined
  w: Walk
  dChars: { total: number }
}

/**
 * The times of a track's keyframes: its own, else its animation's when they fit, else even.
 * Always as many as there are keyframes, from 0 to 1.
 */
export function trackTimes(track: Pick<Track, 'times' | 'values'>, animation: Pick<Animation, 'times'>): number[] {
  const n = track.values.length
  if (track.times && track.times.length === n) return track.times
  if (animation.times && animation.times.length === n) return animation.times
  const out: number[] = []
  for (let i = 0; i < n; i++) out.push(n > 1 ? i / (n - 1) : 0)
  return out
}

/** The ease of each segment of a track: its own, else its animation's when it fits, else linear. */
export function trackEases(track: Pick<Track, 'ease' | 'values'>, animation: Pick<Animation, 'ease'>): Ease[] {
  const segments = Math.max(track.values.length - 1, 0)
  for (const ease of [track.ease, animation.ease]) {
    if (typeof ease === 'string') return Array.from({ length: segments }, () => ease)
    if (Array.isArray(ease) && ease.length === segments) return ease
  }
  return Array.from({ length: segments }, (): Ease => 'linear')
}

function checkTrack(value: unknown, ctx: AnimationContext): Track {
  const src = plain(value)
  let targetValue: unknown
  let propValue: unknown
  let valuesValue: unknown
  let timesValue: unknown
  let easeValue: unknown
  let phaseValue: unknown
  let delayValue: unknown
  for (const key of Object.keys(src)) {
    switch (key) {
      case 'target':
        targetValue = src[key]
        break
      case 'prop':
        propValue = src[key]
        break
      case 'values':
        valuesValue = src[key]
        break
      case 'times':
        timesValue = src[key]
        break
      case 'ease':
        easeValue = src[key]
        break
      case 'phase':
        phaseValue = src[key]
        break
      case 'delay':
        delayValue = src[key]
        break
      default:
        return off()
    }
  }
  const prop = (typeof propValue === 'string' ? PROPS.get(propValue) : undefined) ?? off()
  // The same part named twice is one target.
  const targets = new Set<string>()
  const named: unknown[] = typeof targetValue === 'string' ? [targetValue] : list(targetValue, 1, MORE_LIMITS.targets)
  for (let i = 0; i < named.length; i++) {
    const id: unknown = named[i]
    if (!isId(id) || !ctx.w.parts.has(id)) return off()
    if (!propFits(prop, ctx.w.ids.get(id) ?? off())) return off()
    targets.add(id)
  }
  const raw = list(valuesValue, 2, LIMITS.keyframes)
  const values: (number | string | null)[] = []
  const [min, max] = valueRange(prop)
  let shape: PathShape | null = null
  for (let i = 0; i < raw.length; i++) {
    const v: unknown = raw[i]
    if (v === null) {
      values.push(null)
    } else if (prop === 'd') {
      if (typeof v !== 'string' || v.length > MORE_LIMITS.dKeyframeChars) return off()
      const parsed = parsePath(v) ?? off()
      if (shape && !samePathShape(shape, parsed)) return off()
      shape = parsed
      ctx.dChars.total += v.length
      if (ctx.dChars.total > MORE_LIMITS.dKeyframesTotal) return off()
      values.push(v)
    } else if (isColourProp(prop)) {
      if (typeof v !== 'string' || !HEX.test(v)) return off()
      values.push(v)
    } else {
      values.push(numberIn(v, min, max))
    }
  }
  const track: Track = { target: typeof targetValue === 'string' ? targetValue : [...targets], prop, values }
  if (present(timesValue)) track.times = checkTimes(timesValue, values.length)
  if (present(easeValue)) track.ease = checkEase(easeValue, values.length - 1)
  if (present(phaseValue)) track.phase = numberIn(phaseValue, 0, 1)
  if (present(delayValue)) track.delay = numberIn(delayValue, 0, ctx.duration)

  // Flash safety: paint and opacity keyframes are never closer than 0.17 s and never step.
  // The allowance covers times and durations that were stored rounded to three decimals.
  const paint = isColourProp(prop) || prop === 'opacity'
  const times = trackTimes(track, ctx)
  const allowance = TIME_STEP * ctx.duration + TIME_STEP
  const least = (paint ? PAINT_STEP_S : MOTION_STEP_S) - allowance
  for (let i = 1; i < times.length; i++) {
    if ((times[i] - times[i - 1]) * ctx.duration < least) return off()
  }
  if (paint && trackEases(track, ctx).includes('step')) return off()
  return track
}

function checkAnimation(value: unknown, w: Walk, dChars: { total: number }): Animation {
  const src = plain(value)
  let durationValue: unknown
  let loopValue: unknown
  let everyValue: unknown
  let timesValue: unknown
  let easeValue: unknown
  let tracksValue: unknown
  for (const key of Object.keys(src)) {
    switch (key) {
      case 'duration':
        durationValue = src[key]
        break
      case 'loop':
        loopValue = src[key]
        break
      case 'every':
        everyValue = src[key]
        break
      case 'times':
        timesValue = src[key]
        break
      case 'ease':
        easeValue = src[key]
        break
      case 'tracks':
        tracksValue = src[key]
        break
      default:
        return off()
    }
  }
  const animation: Animation = { duration: numberIn(durationValue, 0.1, 30), tracks: [] }
  if (present(loopValue)) {
    if (typeof loopValue !== 'boolean') return off()
    animation.loop = loopValue
  }
  if (present(everyValue)) {
    const pair = list(everyValue, 2, 2)
    const min = numberIn(pair[0], 1, 120)
    animation.every = [min, numberIn(pair[1], min, 120)]
  }
  if (animation.loop === true && animation.every) return off()
  if (present(timesValue)) animation.times = checkTimes(timesValue, null)
  if (present(easeValue)) animation.ease = checkEase(easeValue, null)
  const ctx: AnimationContext = { duration: animation.duration, times: animation.times, ease: animation.ease, w, dChars }
  const tracks = list(tracksValue, 1, LIMITS.tracks)
  for (let i = 0; i < tracks.length; i++) animation.tracks.push(checkTrack(tracks[i], ctx))
  return animation
}

function checkMouth(value: unknown, w: Walk): MouthDrive {
  const src = plain(value)
  let targetValue: unknown
  let propValue: unknown
  let openValue: unknown
  let closedValue: unknown
  for (const key of Object.keys(src)) {
    switch (key) {
      case 'target':
        targetValue = src[key]
        break
      case 'prop':
        propValue = src[key]
        break
      case 'open':
        openValue = src[key]
        break
      case 'closed':
        closedValue = src[key]
        break
      default:
        return off()
    }
  }
  if (!isId(targetValue) || !w.parts.has(targetValue)) return off()
  const tag = w.ids.get(targetValue) ?? off()
  if (present(propValue)) {
    const prop = (typeof propValue === 'string' ? MOUTH_PROPS.get(propValue) : undefined) ?? off()
    if (!propFits(prop, tag)) return off()
    const [min, max] = valueRange(prop)
    const drive: MouthDrive = { target: targetValue, prop, open: numberIn(openValue, min, max) }
    if (present(closedValue)) drive.closed = numberIn(closedValue, min, max)
    return drive
  }
  if (tag !== 'path') return off()
  if (typeof openValue !== 'string' || openValue.length > MORE_LIMITS.dKeyframeChars) return off()
  const open = parsePath(openValue) ?? off()
  const drive: MouthDrive = { target: targetValue, open: openValue }
  if (present(closedValue)) {
    if (typeof closedValue !== 'string' || closedValue.length > MORE_LIMITS.dKeyframeChars) return off()
    if (!samePathShape(open, parsePath(closedValue) ?? off())) return off()
    drive.closed = closedValue
  }
  return drive
}

function check(value: unknown): Look {
  const src = plain(value)
  let viewBoxValue: unknown
  let nodesValue: unknown
  let pivotsValue: unknown
  let rolesValue: unknown
  let animationsValue: unknown
  let statesValue: unknown
  let mouthValue: unknown
  let gazeValue: unknown
  for (const key of Object.keys(src)) {
    switch (key) {
      case 'viewBox':
        viewBoxValue = src[key]
        break
      case 'nodes':
        nodesValue = src[key]
        break
      case 'pivots':
        pivotsValue = src[key]
        break
      case 'roles':
        rolesValue = src[key]
        break
      case 'animations':
        animationsValue = src[key]
        break
      case 'states':
        statesValue = src[key]
        break
      case 'mouth':
        mouthValue = src[key]
        break
      case 'gaze':
        gazeValue = src[key]
        break
      default:
        return off()
    }
  }

  const vb = list(viewBoxValue, 4, 4)
  const viewBox: Box = [numberIn(vb[0], -NUM_MAX, NUM_MAX), numberIn(vb[1], -NUM_MAX, NUM_MAX), numberIn(vb[2], VIEWBOX_MIN, VIEWBOX_MAX), numberIn(vb[3], VIEWBOX_MIN, VIEWBOX_MAX)]

  const w: Walk = { elements: 0, texts: 0, ids: new Map(), parts: new Set(), gradRefs: [], clipRefs: [] }
  const nodes: LookNode[] = []
  const top = list(nodesValue, 0, LIMITS.elements)
  for (let i = 0; i < top.length; i++) nodes.push(checkNode(top[i], 'root', 1, false, false, w))
  // Every reference resolves to an element of the right kind in this look.
  for (const id of w.gradRefs) {
    const tag = w.ids.get(id)
    if (tag !== 'linearGradient' && tag !== 'radialGradient') return off()
  }
  for (const id of w.clipRefs) if (w.ids.get(id) !== 'clipPath') return off()

  const pivots = record<[number, number]>()
  if (present(pivotsValue)) {
    const p = plain(pivotsValue)
    const keys = Object.keys(p)
    if (keys.length > LIMITS.elements) return off()
    for (const key of keys) {
      if (!isId(key) || !w.parts.has(key)) return off()
      const point = list(p[key], 2, 2)
      pivots[key] = [numberIn(point[0], -NUM_MAX, NUM_MAX), numberIn(point[1], -NUM_MAX, NUM_MAX)]
    }
  }

  const roles = record<string>() as Partial<Record<Role, string>>
  if (present(rolesValue)) {
    const r = plain(rolesValue)
    for (const key of Object.keys(r)) {
      const role = ROLE_NAMES.get(key) ?? off()
      const id: unknown = r[key]
      if (!isId(id) || !w.parts.has(id)) return off()
      roles[role] = id
    }
  }

  const animations = record<Animation>()
  if (present(animationsValue)) {
    const a = plain(animationsValue)
    const keys = Object.keys(a)
    if (keys.length > LIMITS.animations) return off()
    const dChars = { total: 0 }
    for (const key of keys) {
      if (!isId(key)) return off()
      animations[key] = checkAnimation(a[key], w, dChars)
    }
  }

  const states = record<string[]>() as Partial<Record<AvatarState, string[]>>
  if (present(statesValue)) {
    const s = plain(statesValue)
    for (const key of Object.keys(s)) {
      const state = STATE_NAMES.get(key) ?? off()
      const names = list(s[key], 0, MORE_LIMITS.stateNames)
      const out: string[] = []
      for (let i = 0; i < names.length; i++) {
        const name: unknown = names[i]
        if (!isId(name)) return off()
        out.push(name)
      }
      states[state] = out
    }
  }

  const mouth: MouthDrive[] = []
  if (present(mouthValue)) {
    const m = list(mouthValue, 0, MORE_LIMITS.mouthDrives)
    for (let i = 0; i < m.length; i++) mouth.push(checkMouth(m[i], w))
  }

  // At most 5% of the width (the server rounds that bound to three decimals).
  const gaze = present(gazeValue) ? numberIn(gazeValue, 0, viewBox[2] * GAZE_MOST + TIME_STEP) : null
  return { viewBox, nodes, pivots, roles, animations, states, mouth, gaze }
}

/**
 * A look rebuilt from allow-listed keys, or null when anything about `value` is off: an
 * unknown key, a tag or attribute outside the tables, a value that does not match its
 * type, a reference that does not resolve, a limit exceeded, one of the app's own token
 * attributes. The result shares nothing with `value`.
 */
export function checkLook(value: unknown): Look | null {
  try {
    return check(value)
  } catch {
    // Also a getter that throws or a tree too deep for the stack: the look is off either way.
    return null
  }
}

// ---------------------------------------------------------------- rendering

type SvgProps = { [name: string]: string | number }

/**
 * The prefix as `lookElements` prints it: letters, digits, "_" and "-" only, at most 32
 * characters (React's `useId` gives ":r1:"). Ids and references are printed with it, so two
 * figures never share an id; a caller that looks the nodes up again uses the same cleaned prefix.
 */
export function cleanIdPrefix(idPrefix: string): string {
  return typeof idPrefix === 'string' ? idPrefix.replace(/[^A-Za-z0-9_-]/g, '').slice(0, 32) : ''
}

/**
 * The props of one element, built key by key from the tables. Every value is checked
 * again, so this is safe on a tree that never went through `checkLook`; what does not fit
 * is left out.
 */
function nodeProps(node: LookNode, tag: Tag, prefix: string, key: number): SvgProps {
  const props: SvgProps = { key }
  if (isId(node.id)) props.id = prefix + node.id
  const src: unknown = node.attrs
  if (typeof src !== 'object' || src === null || Array.isArray(src)) return props
  const attrs = src as Record<string, unknown>
  for (const [name, kind] of ATTRS.get(tag) ?? []) {
    if (!Object.hasOwn(attrs, name)) continue
    const value = attrValue(kind, attrs[name])
    if (value === undefined) continue
    switch (name) {
      case 'fillRef':
        props.fill = `url(#${prefix}${value})`
        break
      case 'strokeRef':
        props.stroke = `url(#${prefix}${value})`
        break
      case 'clipPathRef':
        props.clipPath = `url(#${prefix}${value})`
        break
      case 'fillToken':
        props.fill = TOKENS.get(String(value)) ?? 'none'
        break
      case 'strokeToken':
        props.stroke = TOKENS.get(String(value)) ?? 'none'
        break
      case 'stopColorToken':
        props.stopColor = TOKENS.get(String(value)) ?? 'none'
        break
      default:
        // `name` is one of the table's own literals here, never a string from the look.
        props[name] = value
    }
  }
  return props
}

function renderNodes(nodes: unknown, parent: Tag | 'root', prefix: string, depth: number, budget: { left: number }): ReactNode[] {
  const out: ReactNode[] = []
  if (!Array.isArray(nodes) || depth > LIMITS.depth) return out
  const allowed = CHILDREN.get(parent) ?? NONE
  for (let i = 0; i < nodes.length; i++) {
    const node = nodes[i] as LookNode | null
    if (typeof node !== 'object' || node === null) continue
    const tag = typeof node.tag === 'string' ? TAGS.get(node.tag) : undefined
    if (!tag || !allowed.has(tag)) continue
    if (budget.left-- <= 0) break
    const props = nodeProps(node, tag, prefix, i)
    if (tag === 'text') {
      // A text node, and never more of it than a checked look could hold.
      const text = typeof node.text === 'string' ? node.text : ''
      out.push(createElement(tag, props, text.length > LIMITS.textChars ? Array.from(text.slice(0, LIMITS.textChars * 2)).slice(0, LIMITS.textChars).join('') : text))
      continue
    }
    const children = renderNodes(node.children, tag, prefix, depth + 1, budget)
    out.push(children.length > 0 ? createElement(tag, props, children) : createElement(tag, props))
  }
  return out
}

/**
 * The children of the app's `<svg>` for a look. An id `X` renders as `idPrefix + X`, a
 * reference as `url(#idPrefix + X)` (the prefix as `cleanIdPrefix` gives it). Tags and
 * attribute names come from the tables in this file and from nowhere else; text is a text node.
 */
export function lookElements(look: Look, idPrefix: string): ReactNode {
  return renderNodes(look?.nodes, 'root', cleanIdPrefix(idPrefix), 1, { left: LIMITS.elements })
}

// ---------------------------------------------------------------- geometry of a look

export interface IndexEntry {
  node: LookNode
  tag: Tag
  /** From the element's own coordinates (inside its own transform) to the viewBox. */
  matrix: Matrix
  /** The elements around it, outermost first. */
  ancestors: readonly LookNode[]
  /** Inside `defs` or a `clipPath`: never animated. */
  inert: boolean
}

const INDEX = new WeakMap<Look, ReadonlyMap<string, IndexEntry>>()
const NO_PARTS: ReadonlyMap<string, IndexEntry> = new Map()

/** Every element of a look that has an id, with where it sits. Computed once per look object. */
export function lookIndex(look: Look): ReadonlyMap<string, IndexEntry> {
  if (typeof look !== 'object' || look === null) return NO_PARTS
  const cached = INDEX.get(look)
  if (cached) return cached
  const out = new Map<string, IndexEntry>()
  let left = LIMITS.elements
  const walk = (nodes: unknown, outer: Matrix, ancestors: readonly LookNode[], inert: boolean, depth: number) => {
    if (!Array.isArray(nodes) || depth > LIMITS.depth) return
    for (const item of nodes) {
      const node = item as LookNode | null
      if (typeof node !== 'object' || node === null || left-- <= 0) continue
      const tag = typeof node.tag === 'string' ? TAGS.get(node.tag) : undefined
      if (!tag) continue
      const attrs: unknown = node.attrs
      const own = typeof attrs === 'object' && attrs !== null && Object.hasOwn(attrs, 'transform') ? parseTransform((attrs as Record<string, unknown>).transform) : null
      const matrix = own ? multiply(outer, transformMatrix(own)) : outer
      const nowInert = inert || tag === 'defs' || tag === 'clipPath'
      if (isId(node.id) && !out.has(node.id)) out.set(node.id, { node, tag, matrix, ancestors, inert: nowInert })
      walk(node.children, matrix, [...ancestors, node], nowInert, depth + 1)
    }
  }
  walk(look.nodes, IDENTITY, [], false, 1)
  INDEX.set(look, out)
  return out
}

/** A part's rest-pose box in viewBox coordinates, or null when the part or its box is missing. */
export function rootBox(look: Look, id: string | undefined): Box | null {
  if (id === undefined) return null
  const entry = lookIndex(look).get(id)
  const box = entry?.node.box
  if (!entry || !Array.isArray(box) || box.length !== 4 || !box.every((n) => finite(n))) return null
  const out = boxThrough(entry.matrix, box)
  return out.every((n) => Number.isFinite(n)) ? out : null
}

/** A look's viewBox, or the default one when it is not four sound numbers. */
export function viewBoxOf(look: Look): Box {
  const v: unknown = look?.viewBox
  if (Array.isArray(v) && v.length === 4 && v.every((n) => finite(n)) && v[2] > 0 && v[3] > 0) return [v[0], v[1], v[2], v[3]]
  return [0, 0, 200, 200]
}

/**
 * What a small face shows: a square around the head role's box with a tenth added on each
 * side, kept inside the viewBox; the whole viewBox when the look names no head.
 */
export function faceViewBox(look: Look): [number, number, number, number] {
  const view = viewBoxOf(look)
  const head = rootBox(look, look?.roles?.head)
  if (!head || head[2] <= 0 || head[3] <= 0) return view
  const side = Math.min(Math.max(head[2], head[3]) * 1.2, view[2], view[3])
  if (!(side > 0)) return view
  const clamp = (centre: number, from: number, length: number) => Math.min(Math.max(centre - side / 2, from), from + length - side)
  const x = clamp(head[0] + head[2] / 2, view[0], view[2])
  const y = clamp(head[1] + head[3] / 2, view[1], view[3])
  const round = (n: number) => Math.round(n * 1000) / 1000
  return [round(x), round(y), round(side), round(side)]
}

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
// Every reference stays inside the look: a gradient, a clip path, a mask, a filter or a part
// to `use` is named by a bare id that must resolve to an element of the right kind in the same
// tree, and is printed with the figure's prefix. The names a filter's primitives pass between
// them (`in`, `in2`, `result`) are local to that filter, never DOM ids, and are printed as they are.
//
// React-free except for `createElement`; reads no globals.

import { createElement, type ReactNode } from 'react'
import { AVATAR_STATES, DRIVE_INPUTS, ROLES } from './types'
import type { Animation, AvatarState, Bezier, Box, Drive, DriveInput, Ease, EaseName, Look, LookNode, MouthDrive, MouthProp, Prop, Role, Tag, Track } from './types'

/**
 * The server's limits (`pub const MAX_*` in server/src/avatar/look.rs), repeated so a look
 * that somehow passed them is still bounded here. A test holds the two lists together.
 */
export const LIMITS: Readonly<Record<'elements' | 'depth' | 'animations' | 'tracks' | 'keyframes' | 'dChars' | 'pointsChars' | 'texts' | 'textChars' | 'stops' | 'attrs', number>> =
  Object.freeze({ elements: 300, depth: 10, animations: 32, tracks: 16, keyframes: 16, dChars: 4000, pointsChars: 2000, texts: 6, textChars: 40, stops: 16, attrs: 32 })

/**
 * Further limits of look.rs that the list above does not name: `MAX_TARGETS`,
 * `MAX_D_KEYFRAME_CHARS`, `MAX_D_KEYFRAMES_TOTAL`, `MAX_STATE_NAMES`, `MAX_MOUTH_DRIVES`,
 * `MAX_TRANSFORMS`, `MAX_FILTERS`, `MAX_FILTER_PRIMITIVES`, `MAX_DASHES`, `MAX_PARENTS` and
 * `MAX_DRIVES`. The limits hold for a look as it is stored (the server measures a path as it
 * was written and again as it is stored), so they are kept exactly here.
 */
export const MORE_LIMITS: Readonly<
  Record<'targets' | 'dKeyframeChars' | 'dKeyframesTotal' | 'stateNames' | 'mouthDrives' | 'transforms' | 'filters' | 'filterPrimitives' | 'dashes' | 'parents' | 'drives', number>
> = Object.freeze({
  targets: 16,
  dKeyframeChars: 1000,
  dKeyframesTotal: 6000,
  stateNames: 6,
  mouthDrives: 4,
  transforms: 8,
  filters: 8,
  filterPrimitives: 8,
  dashes: 8,
  parents: 64,
  drives: 16,
})
/** How many `parents` links lead from a part to the top of its chain at most. */
export const MAX_CHAIN_LINKS = 8
/** `feMergeNode`s of one `feMerge`. */
const MAX_MERGE_NODES = 8
/** The server reads at most this much of one transform; what it writes back is shorter. */
const MAX_TRANSFORM_CHARS = 1024
/** A number list as stored (a blur, a colour matrix, dashes) is never longer than this. */
const MAX_LIST_CHARS = 512
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
  ['use', 'use'],
  ['mask', 'mask'],
  ['filter', 'filter'],
  ['feGaussianBlur', 'feGaussianBlur'],
  ['feDropShadow', 'feDropShadow'],
  ['feOffset', 'feOffset'],
  ['feFlood', 'feFlood'],
  ['feColorMatrix', 'feColorMatrix'],
  ['feComposite', 'feComposite'],
  ['feBlend', 'feBlend'],
  ['feMerge', 'feMerge'],
  ['feMergeNode', 'feMergeNode'],
  ['feMorphology', 'feMorphology'],
  ['feTurbulence', 'feTurbulence'],
  ['feDisplacementMap', 'feDisplacementMap'],
])

const SHAPES: readonly Tag[] = ['path', 'circle', 'ellipse', 'rect', 'line', 'polyline', 'polygon']
const DRAWN: readonly Tag[] = ['g', ...SHAPES, 'text']
/** What an animation, a role, a pivot, a parent, a drive or the mouth may name. */
const ANIMATABLE: ReadonlySet<Tag> = new Set<Tag>([...DRAWN, 'use'])
/** What a `use` may show. */
const USABLE: ReadonlySet<Tag> = new Set(DRAWN)
/** A filter's primitives, `feMergeNode` (which only an `feMerge` holds) not among them. */
const PRIMITIVES: readonly Tag[] = ['feGaussianBlur', 'feDropShadow', 'feOffset', 'feFlood', 'feColorMatrix', 'feComposite', 'feBlend', 'feMerge', 'feMorphology', 'feTurbulence', 'feDisplacementMap']
const NONE: ReadonlySet<Tag> = new Set()

/** The content model: which tags an element may hold (`root`: the children of the app's `<svg>`). */
const CHILDREN: ReadonlyMap<Tag | 'root', ReadonlySet<Tag>> = new Map<Tag | 'root', ReadonlySet<Tag>>([
  ['root', new Set<Tag>([...DRAWN, 'use', 'defs'])],
  ['g', new Set<Tag>([...DRAWN, 'use', 'defs'])],
  ['defs', new Set<Tag>(['linearGradient', 'radialGradient', 'clipPath', 'mask', 'filter', ...DRAWN])],
  ['linearGradient', new Set<Tag>(['stop'])],
  ['radialGradient', new Set<Tag>(['stop'])],
  ['clipPath', new Set<Tag>(SHAPES)],
  ['mask', new Set<Tag>(['g', ...SHAPES])],
  ['filter', new Set<Tag>(PRIMITIVES)],
  ['feMerge', new Set<Tag>(['feMergeNode'])],
])
/** Inside a mask a group holds what the mask holds: groups and shapes. */
const IN_MASK: ReadonlySet<Tag> = new Set<Tag>(['g', ...SHAPES])

/** What an element of `parent` may hold, inside a mask or not. */
function childrenOf(parent: Tag | 'root', inMask: boolean): ReadonlySet<Tag> {
  if (inMask && parent === 'g') return IN_MASK
  return CHILDREN.get(parent) ?? NONE
}

/** How many children an element of this tag may have. */
function childLimit(tag: Tag): number {
  switch (tag) {
    case 'linearGradient':
    case 'radialGradient':
      return LIMITS.stops
    case 'filter':
      return MORE_LIMITS.filterPrimitives
    case 'feMerge':
      return MAX_MERGE_NODES
    default:
      return LIMITS.elements
  }
}

// ---------------------------------------------------------------- values

type Kind =
  | 'num'
  | 'numPct'
  | 'paint'
  | 'colour'
  | 'gradRef'
  | 'clipRef'
  | 'maskRef'
  | 'filterRef'
  | 'useRef'
  | 'transform'
  | 'd'
  | 'points'
  | 'dashes'
  | 'cap'
  | 'join'
  | 'rule'
  | 'weight'
  | 'anchor'
  | 'baseline'
  | 'units'
  | 'spread'
  | 'token'
  | 'maskType'
  | 'interpolation'
  | 'source'
  | 'result'
  | 'blur'
  | 'radius'
  | 'frequency'
  | 'matrix'
  | 'matrixType'
  | 'composite'
  | 'morphology'
  | 'blend'
  | 'noise'
  | 'octaves'
  | 'displace'
  | 'channel'

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
const MASK_TYPES: ReadonlySet<string> = new Set(['luminance', 'alpha'])
const INTERPOLATIONS: ReadonlySet<string> = new Set(['sRGB', 'linearRGB'])
const MATRIX_TYPES: ReadonlySet<string> = new Set(['matrix', 'saturate', 'hueRotate', 'luminanceToAlpha'])
const COMPOSITES: ReadonlySet<string> = new Set(['over', 'in', 'out', 'atop', 'xor', 'arithmetic'])
const MORPHOLOGIES: ReadonlySet<string> = new Set(['erode', 'dilate'])
const BLENDS: ReadonlySet<string> = new Set(['normal', 'multiply', 'screen', 'overlay', 'darken', 'lighten', 'color-dodge', 'color-burn', 'hard-light', 'soft-light', 'difference', 'exclusion'])
const NOISES: ReadonlySet<string> = new Set(['fractalNoise', 'turbulence'])
const CHANNELS: ReadonlySet<string> = new Set(['R', 'G', 'B', 'A'])
/** What a primitive's `in` or `in2` may name besides the `result` of an earlier primitive of its filter. */
const SOURCES: ReadonlySet<string> = new Set(['SourceGraphic', 'SourceAlpha'])

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

/**
 * A list of numbers as it is stored: one number as a JSON number, or numbers separated by
 * single spaces in a string; `min` to `max` of them, each passing `each`.
 */
function numberList(value: string | number, min: number, max: number, each: (n: number) => boolean): boolean {
  if (typeof value === 'number') return min <= 1 && max >= 1 && finite(value) && each(value)
  if (value.length > MAX_LIST_CHARS) return false
  const tokens = value.split(' ')
  if (tokens.length < min || tokens.length > max) return false
  for (const token of tokens) {
    if (!NUMBER.test(token)) return false
    const n = Number(token)
    if (!finite(n) || !each(n)) return false
  }
  return true
}

/**
 * A value of one attribute kind as it may be stored, or undefined when it is anything else.
 * `width` is the viewBox width, which bounds a blur, a morphology radius and a displacement.
 */
function attrValue(kind: Kind, value: unknown, width: number): string | number | undefined {
  // Values stored rounded to three decimals may pass a bound that was rounded too.
  const most = (part: number) => width * part + TIME_STEP
  if (typeof value === 'number') {
    switch (kind) {
      case 'num':
      case 'numPct':
        return finite(value) ? tidy(value) : undefined
      case 'weight':
        return WEIGHTS.has(String(value)) ? value : undefined
      case 'octaves':
        return Number.isInteger(value) && value >= 1 && value <= 4 ? value : undefined
      case 'displace':
        return finite(value) && Math.abs(value) <= most(0.25) ? tidy(value) : undefined
      case 'dashes':
      case 'blur':
      case 'radius':
      case 'frequency':
      case 'matrix':
        return attrValue(kind, String(tidy(value)), width) !== undefined ? tidy(value) : undefined
      default:
        return undefined
    }
  }
  if (typeof value !== 'string') return undefined
  switch (kind) {
    case 'num':
    case 'octaves':
    case 'displace':
      return undefined
    case 'numPct':
      return PERCENT.test(value) ? value : undefined
    case 'paint':
      return HEX.test(value) || value === 'none' || value === 'currentColor' ? value : undefined
    case 'colour':
      return HEX.test(value) || value === 'currentColor' ? value : undefined
    case 'gradRef':
    case 'clipRef':
    case 'maskRef':
    case 'filterRef':
    case 'useRef':
    case 'result':
      return isId(value) ? value : undefined
    case 'source':
      return SOURCES.has(value) || isId(value) ? value : undefined
    case 'transform':
      return parseTransform(value) ? value : undefined
    case 'd':
      return value.length <= LIMITS.dChars && parsePath(value) ? value : undefined
    case 'points':
      return value.length <= LIMITS.pointsChars && pointList(value) ? value : undefined
    case 'dashes':
      return value === 'none' || numberList(value, 1, MORE_LIMITS.dashes, (n) => n >= 0) ? value : undefined
    case 'blur':
      return numberList(value, 1, 2, (n) => n >= 0 && n <= most(0.25)) ? value : undefined
    case 'radius':
      return numberList(value, 1, 2, (n) => Math.abs(n) <= most(0.1)) ? value : undefined
    case 'frequency':
      return numberList(value, 1, 2, (n) => n >= 0 && n <= 1) ? value : undefined
    case 'matrix':
      return numberList(value, 1, 20, () => true) ? value : undefined
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
    case 'maskType':
      return MASK_TYPES.has(value) ? value : undefined
    case 'interpolation':
      return INTERPOLATIONS.has(value) ? value : undefined
    case 'matrixType':
      return MATRIX_TYPES.has(value) ? value : undefined
    case 'composite':
      return COMPOSITES.has(value) ? value : undefined
    case 'morphology':
      return MORPHOLOGIES.has(value) ? value : undefined
    case 'blend':
      return BLENDS.has(value) ? value : undefined
    case 'noise':
      return NOISES.has(value) ? value : undefined
    case 'channel':
      return CHANNELS.has(value) ? value : undefined
  }
}

/** Numbers separated by white space or commas, and nothing else. */
function pointList(text: string): boolean {
  const trimmed = text.trim()
  if (trimmed === '') return true
  for (const token of trimmed.split(/[\s,]+/)) {
    if (!NUMBER.test(token) || !finite(Number(token))) return false
  }
  return true
}

// ---------------------------------------------------------------- attribute tables

/** Attributes of everything that is drawn (g, shapes, text, use). The token attributes are the app's own. */
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
  ['strokeDasharray', 'dashes'],
  ['strokeDashoffset', 'num'],
  ['fillOpacity', 'numPct'],
  ['strokeOpacity', 'numPct'],
  ['opacity', 'numPct'],
  ['fillRule', 'rule'],
  ['clipRule', 'rule'],
  ['clipPathRef', 'clipRef'],
  ['maskRef', 'maskRef'],
  ['filterRef', 'filterRef'],
]

const GRADIENT: readonly (readonly [string, Kind])[] = [
  ['gradientUnits', 'units'],
  ['gradientTransform', 'transform'],
  ['spreadMethod', 'spread'],
]

/** The region of a mask or a filter. */
const REGION: readonly (readonly [string, Kind])[] = [
  ['x', 'numPct'],
  ['y', 'numPct'],
  ['width', 'numPct'],
  ['height', 'numPct'],
]

const drawn = (own: readonly (readonly [string, Kind])[]): ReadonlyMap<string, Kind> => new Map<string, Kind>([...own, ...PRESENTATION])
/** A filter primitive: its own attributes and the name it gives its result. */
const primitive = (own: readonly (readonly [string, Kind])[]): ReadonlyMap<string, Kind> => new Map<string, Kind>([...own, ['result', 'result']])

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
  ['use', drawn([['useRef', 'useRef'], ['x', 'num'], ['y', 'num']])],
  ['defs', new Map<string, Kind>()],
  ['linearGradient', new Map<string, Kind>([['x1', 'numPct'], ['y1', 'numPct'], ['x2', 'numPct'], ['y2', 'numPct'], ...GRADIENT])],
  ['radialGradient', new Map<string, Kind>([['cx', 'numPct'], ['cy', 'numPct'], ['r', 'numPct'], ['fx', 'numPct'], ['fy', 'numPct'], ...GRADIENT])],
  ['stop', new Map<string, Kind>([['offset', 'numPct'], ['stopColor', 'colour'], ['stopColorToken', 'token'], ['stopOpacity', 'numPct']])],
  ['clipPath', new Map<string, Kind>([['clipPathUnits', 'units']])],
  ['mask', new Map<string, Kind>([['maskUnits', 'units'], ['maskContentUnits', 'units'], ...REGION, ['maskType', 'maskType']])],
  ['filter', new Map<string, Kind>([...REGION, ['filterUnits', 'units'], ['primitiveUnits', 'units'], ['colorInterpolationFilters', 'interpolation']])],
  ['feGaussianBlur', primitive([['in', 'source'], ['stdDeviation', 'blur']])],
  ['feDropShadow', primitive([['in', 'source'], ['dx', 'num'], ['dy', 'num'], ['stdDeviation', 'blur'], ['floodColor', 'colour'], ['floodOpacity', 'numPct']])],
  ['feOffset', primitive([['in', 'source'], ['dx', 'num'], ['dy', 'num']])],
  ['feFlood', primitive([['floodColor', 'colour'], ['floodOpacity', 'numPct']])],
  ['feColorMatrix', primitive([['in', 'source'], ['type', 'matrixType'], ['values', 'matrix']])],
  ['feComposite', primitive([['in', 'source'], ['in2', 'source'], ['operator', 'composite'], ['k1', 'num'], ['k2', 'num'], ['k3', 'num'], ['k4', 'num']])],
  ['feBlend', primitive([['in', 'source'], ['in2', 'source'], ['mode', 'blend']])],
  ['feMerge', primitive([])],
  ['feMergeNode', new Map<string, Kind>([['in', 'source']])],
  ['feMorphology', primitive([['in', 'source'], ['operator', 'morphology'], ['radius', 'radius']])],
  ['feTurbulence', primitive([['type', 'noise'], ['baseFrequency', 'frequency'], ['numOctaves', 'octaves'], ['seed', 'num']])],
  ['feDisplacementMap', primitive([['in', 'source'], ['in2', 'source'], ['scale', 'displace'], ['xChannelSelector', 'channel'], ['yChannelSelector', 'channel']])],
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

/** The matrix that undoes `m`, or null when there is none (or a number is not finite). */
export function invert(m: Matrix): Matrix | null {
  const det = m[0] * m[3] - m[1] * m[2]
  if (!Number.isFinite(det) || Math.abs(det) < 1e-12) return null
  const out: Matrix = [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det]
  return out.every((n) => Number.isFinite(n)) ? out : null
}

const RAD = Math.PI / 180

/** The matrix of one transform function. */
export function opMatrix(op: TransformOp): Matrix {
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

const TRANSFORM_PROPS: ReadonlySet<Prop> = new Set<Prop>(['translateX', 'translateY', 'rotate', 'skewX', 'skewY', 'scale', 'scaleX', 'scaleY'])
const COLOUR_PROPS: ReadonlySet<Prop> = new Set<Prop>(['fill', 'stroke'])
/** What flash safety holds to: colours and opacities. */
const PAINT_PROPS: ReadonlySet<Prop> = new Set<Prop>(['fill', 'stroke', 'opacity', 'fillOpacity', 'strokeOpacity'])

/** Prop -> the tags it can be animated on. */
const PROP_TAGS: ReadonlyMap<Prop, ReadonlySet<Tag>> = new Map<Prop, ReadonlySet<Tag>>([
  ['translateX', ANIMATABLE],
  ['translateY', ANIMATABLE],
  ['rotate', ANIMATABLE],
  ['skewX', ANIMATABLE],
  ['skewY', ANIMATABLE],
  ['scale', ANIMATABLE],
  ['scaleX', ANIMATABLE],
  ['scaleY', ANIMATABLE],
  ['opacity', ANIMATABLE],
  ['fillOpacity', ANIMATABLE],
  ['strokeOpacity', ANIMATABLE],
  ['strokeDashoffset', ANIMATABLE],
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
/** A colour or an opacity: keyframes 0.17 s apart, no step, and driven only by the inputs that move slowly. */
export const isPaintProp = (prop: Prop): boolean => PAINT_PROPS.has(prop)

const MOUTH_PROPS: ReadonlyMap<string, MouthProp> = new Map<string, MouthProp>([
  ['scaleY', 'scaleY'],
  ['scaleX', 'scaleX'],
  ['scale', 'scale'],
  ['translateY', 'translateY'],
  ['ry', 'ry'],
  ['r', 'r'],
  ['height', 'height'],
])

const EASES: ReadonlyMap<string, EaseName> = new Map<string, EaseName>([
  ['linear', 'linear'],
  ['in', 'in'],
  ['out', 'out'],
  ['inOut', 'inOut'],
  ['outBack', 'outBack'],
  ['step', 'step'],
  ['inBack', 'inBack'],
  ['inOutBack', 'inOutBack'],
  ['outElastic', 'outElastic'],
  ['outBounce', 'outBounce'],
])

/** Four numbers: one cubic bezier (its ranges are checked where it is read). */
export function isBezier(value: unknown): value is Bezier {
  return Array.isArray(value) && value.length === 4 && value.every((n) => typeof n === 'number')
}

/** One ease as `checkLook` takes it: a name from the list, or a bezier with x from 0 to 1 and y from -2 to 3. */
export function isEase(value: unknown): value is Ease {
  if (typeof value === 'string') return EASES.has(value)
  if (!isBezier(value) || !value.every((n) => finite(n))) return false
  return value[0] >= 0 && value[0] <= 1 && value[2] >= 0 && value[2] <= 1 && value[1] >= -2 && value[1] <= 3 && value[3] >= -2 && value[3] <= 3
}

const STATE_NAMES: ReadonlyMap<string, AvatarState> = new Map<string, AvatarState>(AVATAR_STATES.map((s) => [s, s]))
const ROLE_NAMES: ReadonlyMap<string, Role> = new Map<string, Role>(ROLES.map((r) => [r, r]))
const INPUT_NAMES: ReadonlyMap<string, DriveInput> = new Map<string, DriveInput>(DRIVE_INPUTS.map((i) => [i, i]))
/** The inputs the engine moves slowly (a full swing takes at least 0.25 s): only they may drive a paint. */
const SLOW_INPUTS: ReadonlySet<DriveInput> = new Set<DriveInput>(['pace', 'turn', 'held'])
/** How fast the engine moves a slow input at most, in the input's units a second (`INPUT_RATE` of the engine). */
const INPUT_UNITS_PER_S = 4

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
const NOT_PLAIN = /[\p{Cc}\p{Zl}\p{Zp}؜‎‏‪-‮⁦-⁩]/u

/** Where an element with an id sits, for the rules of `use` and `parents`, and its own attributes, for the rule of a loop's paint. */
interface Placed {
  tag: Tag
  /** Inside a clip path, a mask or a filter: nothing may `use` it. */
  sealed: boolean
  /** The ids of the elements around it, outermost first. */
  ancestors: readonly string[]
  /** It and every element inside it. */
  size: number
  /** It is a `use` or holds one. */
  holdsUse: boolean
  attrs: Readonly<Record<string, string | number>> | undefined
}

interface Walk {
  /** The viewBox width: what a blur, a radius and a displacement are measured against. */
  width: number
  elements: number
  texts: number
  filters: number
  uses: number
  /** Characters of every `d` keyframe of the look's tracks and drives. */
  dChars: number
  /** Every id of the tree, with its tag. */
  ids: Map<string, Tag>
  placed: Map<string, Placed>
  /** Ids that an animation, a role, a pivot, a parent, a drive or the mouth may name. */
  parts: Set<string>
  gradRefs: string[]
  clipRefs: string[]
  maskRefs: string[]
  filterRefs: string[]
  useRefs: { target: string; ancestors: readonly string[] }[]
}

/** What surrounds an element while the tree is walked. */
interface Around {
  /** Inside `defs`: never drawn by itself, never animated. */
  inert: boolean
  inClip: boolean
  inMask: boolean
  inFilter: boolean
  /** The ids of the elements around it, outermost first. */
  ancestors: readonly string[]
}

function checkAttrs(value: unknown, tag: Tag, around: Around, w: Walk): Record<string, string | number> | undefined {
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
    const checked = attrValue(kind, src[key], w.width)
    if (checked === undefined) return off()
    switch (kind) {
      case 'gradRef':
        w.gradRefs.push(checked as string)
        break
      // What a clip path or a mask is made of is clipped, masked and filtered by nothing.
      case 'clipRef':
        if (around.inClip || around.inMask) return off()
        w.clipRefs.push(checked as string)
        break
      case 'maskRef':
        if (around.inClip || around.inMask) return off()
        w.maskRefs.push(checked as string)
        break
      case 'filterRef':
        if (around.inClip || around.inMask) return off()
        w.filterRefs.push(checked as string)
        break
      default:
        break
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

/** An attribute of a checked node, or undefined. */
function attrOf(node: LookNode, key: string): string | number | undefined {
  const attrs = node.attrs
  return attrs !== undefined && Object.hasOwn(attrs, key) ? attrs[key] : undefined
}

/**
 * A filter's primitives in order: each `in` and `in2` (and each `feMergeNode`'s `in`) names
 * the source graphic, its alpha, or the `result` of an earlier primitive of the same filter.
 */
function checkFilter(node: LookNode, w: Walk) {
  if (++w.filters > MORE_LIMITS.filters) return off()
  const results = new Set<string>()
  const known = (name: string | number | undefined): boolean => name === undefined || (typeof name === 'string' && (SOURCES.has(name) || results.has(name)))
  for (const step of node.children ?? []) {
    if (!known(attrOf(step, 'in')) || !known(attrOf(step, 'in2'))) return off()
    for (const merged of step.children ?? []) if (!known(attrOf(merged, 'in'))) return off()
    const result = attrOf(step, 'result')
    if (typeof result === 'string') results.add(result)
  }
}

function checkNode(value: unknown, parent: Tag | 'root', depth: number, around: Around, w: Walk): LookNode {
  const src = plain(value)
  if (depth > LIMITS.depth) return off()
  const first = w.elements
  const usesBefore = w.uses
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
  if (!childrenOf(parent, around.inMask).has(tag)) return off()
  const node: LookNode = { tag }
  const inner: Around = {
    inert: around.inert || tag === 'defs',
    inClip: around.inClip || tag === 'clipPath',
    inMask: around.inMask || tag === 'mask',
    inFilter: around.inFilter || tag === 'filter',
    ancestors: around.ancestors,
  }
  let id: string | null = null
  if (present(idValue)) {
    if (!isId(idValue) || w.ids.has(idValue)) return off()
    w.ids.set(idValue, tag)
    if (ANIMATABLE.has(tag) && !inner.inert && !inner.inClip) w.parts.add(idValue)
    node.id = idValue
    id = idValue
    inner.ancestors = [...around.ancestors, idValue]
  }
  const attrs = checkAttrs(attrsValue, tag, inner, w)
  if (attrs) node.attrs = attrs
  if (tag === 'use') {
    // A use names what it shows; where that is, and what it holds, is checked once the whole tree is known.
    const target = attrs !== undefined && Object.hasOwn(attrs, 'useRef') ? attrs.useRef : undefined
    if (typeof target !== 'string') return off()
    w.uses++
    w.useRefs.push({ target, ancestors: around.ancestors })
  }
  if (present(boxValue)) node.box = checkBox(boxValue)
  if (present(textValue)) {
    if (tag !== 'text' || typeof textValue !== 'string') return off()
    if (textValue.length > LIMITS.textChars * 2 || [...textValue].length > LIMITS.textChars || NOT_PLAIN.test(textValue)) return off()
    node.text = textValue
  }
  if (tag === 'text' && ++w.texts > LIMITS.texts) return off()
  if (present(childrenValue)) {
    const allowed = childrenOf(tag, inner.inMask)
    const items = list(childrenValue, 0, childLimit(tag))
    if (items.length > 0 && allowed.size === 0) return off()
    const children: LookNode[] = []
    for (let i = 0; i < items.length; i++) children.push(checkNode(items[i], tag, depth + 1, inner, w))
    if (children.length > 0) node.children = children
  }
  if (tag === 'filter') checkFilter(node, w)
  if (id !== null) w.placed.set(id, { tag, sealed: around.inClip || around.inMask || around.inFilter, ancestors: around.ancestors, size: w.elements - first, holdsUse: w.uses > usesBefore, attrs })
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

function checkBezier(value: unknown): Bezier {
  const v = list(value, 4, 4)
  return [numberIn(v[0], 0, 1), numberIn(v[1], -2, 3), numberIn(v[2], 0, 1), numberIn(v[3], -2, 3)]
}

/** A name or four numbers (one ease for every segment), or a list of those (one per segment). */
function checkEase(value: unknown, segments: number | null): Ease | Ease[] {
  if (typeof value === 'string') return EASES.get(value) ?? off()
  const v = list(value, 1, LIMITS.keyframes - 1)
  if (isBezier(v)) return checkBezier(v)
  if (segments !== null && v.length !== segments) return off()
  const out: Ease[] = []
  for (let i = 0; i < v.length; i++) {
    const e: unknown = v[i]
    if (typeof e === 'string') out.push(EASES.get(e) ?? off())
    else if (Array.isArray(e)) out.push(checkBezier(e))
    else return off()
  }
  return out
}

function valueRange(prop: Prop): [number, number] {
  switch (prop) {
    case 'opacity':
    case 'fillOpacity':
    case 'strokeOpacity':
      return [0, 1]
    case 'scale':
    case 'scaleX':
    case 'scaleY':
      return [-10, 10]
    case 'rotate':
      return [-3600, 3600]
    case 'skewX':
    case 'skewY':
      return [-80, 80]
    default:
      return [-NUM_MAX, NUM_MAX]
  }
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

/**
 * The ease of each segment of a track: its own, else its animation's when it fits, else
 * linear. A name or a bezier stands for every segment.
 */
export function trackEases(track: Pick<Track, 'ease' | 'values'>, animation: Pick<Animation, 'ease'>): Ease[] {
  const segments = Math.max(track.values.length - 1, 0)
  for (const ease of [track.ease, animation.ease]) {
    if (typeof ease === 'string' || isBezier(ease)) return Array.from({ length: segments }, (): Ease => ease)
    if (Array.isArray(ease) && ease.length === segments) return ease
  }
  return Array.from({ length: segments }, (): Ease => 'linear')
}

/** The parts a track or a drive names: one id, or a list of up to 16 (the same part twice is one). */
function checkTargets(value: unknown, prop: Prop, w: Walk): string | string[] {
  const targets = new Set<string>()
  const named: unknown[] = typeof value === 'string' ? [value] : list(value, 1, MORE_LIMITS.targets)
  for (let i = 0; i < named.length; i++) {
    const id: unknown = named[i]
    if (!isId(id) || !w.parts.has(id)) return off()
    if (!propFits(prop, w.ids.get(id) ?? off())) return off()
    targets.add(id)
  }
  return typeof value === 'string' ? value : [...targets]
}

/** The keyframes of a track or a drive, each of the prop's type and range; null: the artwork's own value. */
function checkValues(value: unknown, prop: Prop, w: Walk): (number | string | null)[] {
  const raw = list(value, 2, LIMITS.keyframes)
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
      w.dChars += v.length
      if (w.dChars > MORE_LIMITS.dKeyframesTotal) return off()
      values.push(v)
    } else if (isColourProp(prop)) {
      if (typeof v !== 'string' || !HEX.test(v)) return off()
      values.push(v)
    } else {
      values.push(numberIn(v, min, max))
    }
  }
  return values
}

interface AnimationContext {
  duration: number
  times: number[] | undefined
  ease: Ease | Ease[] | undefined
  loop: boolean
  w: Walk
}

/** Two opacities this close are one (values are stored to three decimals; the server's own tolerance). */
const SAME_OPACITY = 0.0005

/** Whether two keyframes of a paint are the same value: equal numbers, or equal colours however they are spelled. */
function samePaint(a: number | string, b: number | string): boolean {
  if (typeof a === 'number' && typeof b === 'number') return Math.abs(a - b) < SAME_OPACITY
  if (typeof a !== 'string' || typeof b !== 'string') return false
  const x = parseHex(a)
  const y = parseHex(b)
  return x !== null && y !== null && x.every((n, i) => n === y[i])
}

/**
 * A part's own value of a paint prop, where it can be told from the element alone (as the
 * server tells it): its opacity (1 when it sets none), the fill or stroke opacity or the
 * colour it sets itself. undefined: it cannot be told (it is inherited, a gradient, a token,
 * `none`, a percentage).
 */
function ownPaint(attrs: Readonly<Record<string, string | number>> | undefined, prop: Prop): number | string | undefined {
  const value = attrs !== undefined && Object.hasOwn(attrs, prop) ? attrs[prop] : undefined
  if (prop === 'opacity') return value === undefined ? 1 : typeof value === 'number' ? value : undefined
  if (prop === 'fillOpacity' || prop === 'strokeOpacity') return typeof value === 'number' ? value : undefined
  return typeof value === 'string' && value.startsWith('#') ? value : undefined
}

/**
 * Flash safety for a loop: where it starts again its last keyframe meets its first with no
 * time between them, so a paint track ends on the value it starts with. null is each
 * target's own value: against a keyframe it closes the loop when every target's own value is
 * that keyframe, or cannot be told here.
 */
function closesLoop(prop: Prop, values: readonly (number | string | null)[], targets: readonly string[], w: Walk): boolean {
  const first = values[0]
  const last = values[values.length - 1]
  if (first === null || last === null) {
    const key = first ?? last
    if (key === null) return true
    return targets.every((id) => {
      const own = ownPaint(w.placed.get(id)?.attrs, prop)
      return own === undefined || samePaint(own, key)
    })
  }
  return samePaint(first, last)
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
  const target = checkTargets(targetValue, prop, ctx.w)
  const values = checkValues(valuesValue, prop, ctx.w)
  const track: Track = { target, prop, values }
  if (present(timesValue)) track.times = checkTimes(timesValue, values.length)
  if (present(easeValue)) track.ease = checkEase(easeValue, values.length - 1)
  if (present(phaseValue)) track.phase = numberIn(phaseValue, 0, 1)
  if (present(delayValue)) track.delay = numberIn(delayValue, 0, ctx.duration)

  // Flash safety: paint and opacity keyframes are never closer than 0.17 s and never step.
  // The allowance covers times and durations that were stored rounded to three decimals.
  const paint = isPaintProp(prop)
  const times = trackTimes(track, ctx)
  const allowance = TIME_STEP * ctx.duration + TIME_STEP
  const least = (paint ? PAINT_STEP_S : MOTION_STEP_S) - allowance
  for (let i = 1; i < times.length; i++) {
    if ((times[i] - times[i - 1]) * ctx.duration < least) return off()
  }
  if (paint && !steadyEases(trackEases(track, ctx))) return off()
  if (paint && ctx.loop && !closesLoop(prop, values, typeof target === 'string' ? [target] : target, ctx.w)) return off()
  return track
}

/**
 * Flash safety for a colour or an opacity: no segment jumps (`step`), swings back and forth
 * (`outElastic`, `outBounce`) or follows a bezier whose handles lie outside 0 … 1 (inside,
 * it rises steadily).
 */
function steadyEases(eases: readonly Ease[]): boolean {
  return eases.every((ease) => (isBezier(ease) ? ease[1] >= 0 && ease[1] <= 1 && ease[3] >= 0 && ease[3] <= 1 : ease !== 'step' && ease !== 'outElastic' && ease !== 'outBounce'))
}

function checkAnimation(value: unknown, w: Walk): Animation {
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
  const ctx: AnimationContext = { duration: animation.duration, times: animation.times, ease: animation.ease, loop: animation.loop === true, w }
  const tracks = list(tracksValue, 1, LIMITS.tracks)
  for (let i = 0; i < tracks.length; i++) animation.tracks.push(checkTrack(tracks[i], ctx))
  return animation
}

function checkDrive(value: unknown, w: Walk): Drive {
  const src = plain(value)
  let inputValue: unknown
  let targetValue: unknown
  let propValue: unknown
  let valuesValue: unknown
  let atValue: unknown
  let easeValue: unknown
  for (const key of Object.keys(src)) {
    switch (key) {
      case 'input':
        inputValue = src[key]
        break
      case 'target':
        targetValue = src[key]
        break
      case 'prop':
        propValue = src[key]
        break
      case 'values':
        valuesValue = src[key]
        break
      case 'at':
        atValue = src[key]
        break
      case 'ease':
        easeValue = src[key]
        break
      default:
        return off()
    }
  }
  const input = (typeof inputValue === 'string' ? INPUT_NAMES.get(inputValue) : undefined) ?? off()
  const prop = (typeof propValue === 'string' ? PROPS.get(propValue) : undefined) ?? off()
  // Flash safety: where it looks and its mouth move fast, so they never drive a colour or an opacity.
  if (isPaintProp(prop) && !SLOW_INPUTS.has(input)) return off()
  const target = checkTargets(targetValue, prop, w)
  const values = checkValues(valuesValue, prop, w)
  const drive: Drive = { input, target, prop, values }
  if (present(atValue)) drive.at = checkTimes(atValue, values.length)
  if (present(easeValue)) drive.ease = checkEase(easeValue, values.length - 1)
  if (isPaintProp(prop)) {
    // Even a slow input crosses its range in a quarter second (turn, from -1 to 1, in half a
    // second): paint values along it keep the distance paint keyframes keep in time.
    if (!steadyEases(trackEases(drive, {}))) return off()
    const least = (PAINT_STEP_S * INPUT_UNITS_PER_S) / (input === 'turn' ? 2 : 1) - TIME_STEP
    const places = trackTimes({ times: drive.at, values }, {})
    for (let i = 1; i < places.length; i++) if (places[i] - places[i - 1] < least) return off()
  }
  return drive
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

/**
 * What each `use` shows: a group, a shape or a text of this look that no clip path, mask or
 * filter holds, that is not around the `use`, and that holds no `use` itself (so nothing
 * repeats itself). Each counts against the element limit with what it shows.
 */
function checkUses(w: Walk) {
  let shown = 0
  for (const use of w.useRefs) {
    const target = w.placed.get(use.target)
    if (!target || !USABLE.has(target.tag) || target.sealed || target.holdsUse || use.ancestors.includes(use.target)) return off()
    shown += target.size
  }
  if (w.elements + shown > LIMITS.elements) return off()
}

/**
 * `parents`: part to part, the child neither inside its parent nor around it, at most 64
 * entries, no cycle, and no chain longer than eight links.
 */
function checkParents(value: unknown, w: Walk): Record<string, string> {
  const src = plain(value)
  const keys = Object.keys(src)
  if (keys.length > MORE_LIMITS.parents) return off()
  const parents = record<string>()
  for (const child of keys) {
    const parent: unknown = src[child]
    if (!isId(child) || !w.parts.has(child) || !isId(parent) || !w.parts.has(parent) || child === parent) return off()
    const c = w.placed.get(child) ?? off()
    const p = w.placed.get(parent) ?? off()
    if (c.ancestors.includes(parent) || p.ancestors.includes(child)) return off()
    parents[child] = parent
  }
  for (const child of keys) {
    let at = child
    let links = 0
    // A cycle never reaches the top: it runs past the longest chain allowed.
    while (Object.hasOwn(parents, at)) {
      at = parents[at]
      if (++links > MAX_CHAIN_LINKS) return off()
    }
  }
  return parents
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
  let parentsValue: unknown
  let drivesValue: unknown
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
      case 'parents':
        parentsValue = src[key]
        break
      case 'drives':
        drivesValue = src[key]
        break
      default:
        return off()
    }
  }

  const vb = list(viewBoxValue, 4, 4)
  const viewBox: Box = [numberIn(vb[0], -NUM_MAX, NUM_MAX), numberIn(vb[1], -NUM_MAX, NUM_MAX), numberIn(vb[2], VIEWBOX_MIN, VIEWBOX_MAX), numberIn(vb[3], VIEWBOX_MIN, VIEWBOX_MAX)]

  const w: Walk = { width: viewBox[2], elements: 0, texts: 0, filters: 0, uses: 0, dChars: 0, ids: new Map(), placed: new Map(), parts: new Set(), gradRefs: [], clipRefs: [], maskRefs: [], filterRefs: [], useRefs: [] }
  const nodes: LookNode[] = []
  const top = list(nodesValue, 0, LIMITS.elements)
  for (let i = 0; i < top.length; i++) nodes.push(checkNode(top[i], 'root', 1, { inert: false, inClip: false, inMask: false, inFilter: false, ancestors: [] }, w))
  // Every reference resolves to an element of the right kind in this look.
  for (const id of w.gradRefs) {
    const tag = w.ids.get(id)
    if (tag !== 'linearGradient' && tag !== 'radialGradient') return off()
  }
  for (const id of w.clipRefs) if (w.ids.get(id) !== 'clipPath') return off()
  for (const id of w.maskRefs) if (w.ids.get(id) !== 'mask') return off()
  for (const id of w.filterRefs) if (w.ids.get(id) !== 'filter') return off()
  checkUses(w)

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
    for (const key of keys) {
      if (!isId(key)) return off()
      animations[key] = checkAnimation(a[key], w)
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
  const look: Look = { viewBox, nodes, pivots, roles, animations, states, mouth, gaze }

  // The rig of the session's own: kept only when it holds something, as the server writes it.
  if (present(parentsValue)) {
    const parents = checkParents(parentsValue, w)
    if (Object.keys(parents).length > 0) look.parents = parents
  }
  if (present(drivesValue)) {
    const d = list(drivesValue, 0, MORE_LIMITS.drives)
    const drives: Drive[] = []
    for (let i = 0; i < d.length; i++) drives.push(checkDrive(d[i], w))
    if (drives.length > 0) look.drives = drives
  }
  return look
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

/** Whether a look has a drive on this input (the walking overlay leaves the lean or the hop to it). */
export function drivesOn(look: Look, input: DriveInput): boolean {
  const drives: unknown = look?.drives
  if (!Array.isArray(drives)) return false
  for (const drive of drives as unknown[]) if (typeof drive === 'object' && drive !== null && (drive as { input?: unknown }).input === input) return true
  return false
}

// ---------------------------------------------------------------- rendering

type SvgProps = { [name: string]: string | number }

/** The font size of a `text` that sets none, in viewBox units: the size the server's box of a text assumes. */
const TEXT_SIZE = 16

/**
 * The prefix as `lookElements` prints it: letters, digits, "_" and "-" only, at most 32
 * characters (React's `useId` gives ":r1:"). Ids and references are printed with it, so two
 * figures never share an id; a caller that looks the nodes up again uses the same cleaned prefix.
 */
export function cleanIdPrefix(idPrefix: string): string {
  return typeof idPrefix === 'string' ? idPrefix.replace(/[^A-Za-z0-9_-]/g, '').slice(0, 32) : ''
}

/** What a `use` would show, by id: how many elements, and whether a `use` is among them. */
interface Shown {
  size: number
  holdsUse: boolean
}

interface Drawing {
  prefix: string
  /** The viewBox width, for the values measured against it. */
  width: number
  /** Elements still to draw: a `use` takes what it shows as well. */
  left: number
  shown: ReadonlyMap<string, Shown>
}

/** Every element with an id and what it holds, walked within the limits whatever the data. */
function measure(nodes: unknown, depth: number, out: Map<string, Shown>, budget: { left: number }): Shown {
  const all: Shown = { size: 0, holdsUse: false }
  if (!Array.isArray(nodes) || depth > LIMITS.depth) return all
  for (const item of nodes) {
    const node = item as LookNode | null
    if (typeof node !== 'object' || node === null) continue
    if (budget.left-- <= 0) break
    const tag = typeof node.tag === 'string' ? TAGS.get(node.tag) : undefined
    if (!tag) continue
    const inner = measure(node.children, depth + 1, out, budget)
    const mine: Shown = { size: 1 + inner.size, holdsUse: tag === 'use' || inner.holdsUse }
    if (isId(node.id) && !out.has(node.id)) out.set(node.id, mine)
    all.size += mine.size
    all.holdsUse ||= mine.holdsUse
  }
  return all
}

/**
 * The props of one element, built key by key from the tables. Every value is checked
 * again, so this is safe on a tree that never went through `checkLook`; what does not fit
 * is left out.
 */
function nodeProps(node: LookNode, tag: Tag, d: Drawing, key: number): SvgProps {
  const prefix = d.prefix
  const props: SvgProps = { key }
  if (isId(node.id)) props.id = prefix + node.id
  const src: unknown = node.attrs
  if (typeof src !== 'object' || src === null || Array.isArray(src)) return props
  const attrs = src as Record<string, unknown>
  for (const [name, kind] of ATTRS.get(tag) ?? []) {
    if (!Object.hasOwn(attrs, name)) continue
    const value = attrValue(kind, attrs[name], d.width)
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
      case 'maskRef':
        props.mask = `url(#${prefix}${value})`
        break
      case 'filterRef':
        props.filter = `url(#${prefix}${value})`
        break
      case 'useRef':
        props.href = `#${prefix}${value}`
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

/** What a `use` shows, when it shows something that may be shown: else null. */
function shownBy(node: LookNode, d: Drawing): Shown | null {
  const attrs: unknown = node.attrs
  const ref = typeof attrs === 'object' && attrs !== null && Object.hasOwn(attrs, 'useRef') ? (attrs as Record<string, unknown>).useRef : undefined
  const shown = isId(ref) ? d.shown.get(ref) : undefined
  // A use of something that holds a use could repeat itself without end: it is not drawn.
  return shown && !shown.holdsUse ? shown : null
}

function renderNodes(nodes: unknown, parent: Tag | 'root', inMask: boolean, depth: number, d: Drawing): ReactNode[] {
  const out: ReactNode[] = []
  if (!Array.isArray(nodes) || depth > LIMITS.depth) return out
  const allowed = childrenOf(parent, inMask)
  for (let i = 0; i < nodes.length; i++) {
    const node = nodes[i] as LookNode | null
    if (typeof node !== 'object' || node === null) continue
    const tag = typeof node.tag === 'string' ? TAGS.get(node.tag) : undefined
    if (!tag || !allowed.has(tag)) continue
    let cost = 1
    if (tag === 'use') {
      const shown = shownBy(node, d)
      if (!shown) continue
      cost += shown.size
    }
    if (d.left < cost) {
      if (d.left <= 0) break
      continue
    }
    d.left -= cost
    const props = nodeProps(node, tag, d, i)
    if (tag === 'text') {
      // The size the server measured its box with, written out: else it would inherit whatever the page's style sheet says.
      if (props.fontSize === undefined) props.fontSize = TEXT_SIZE
      // A text node, and never more of it than a checked look could hold.
      const text = typeof node.text === 'string' ? node.text : ''
      out.push(createElement(tag, props, text.length > LIMITS.textChars ? Array.from(text.slice(0, LIMITS.textChars * 2)).slice(0, LIMITS.textChars).join('') : text))
      continue
    }
    const children = renderNodes(node.children, tag, inMask || tag === 'mask', depth + 1, d)
    out.push(children.length > 0 ? createElement(tag, props, children) : createElement(tag, props))
  }
  return out
}

/**
 * The children of the app's `<svg>` for a look. An id `X` renders as `idPrefix + X`, a
 * reference as `url(#idPrefix + X)` (and a `use` as `href="#idPrefix + X"`), the prefix as
 * `cleanIdPrefix` gives it. Tags and attribute names come from the tables in this file and
 * from nowhere else; text is a text node.
 */
export function lookElements(look: Look, idPrefix: string): ReactNode {
  const nodes: unknown = look?.nodes
  const shown = new Map<string, Shown>()
  measure(nodes, 1, shown, { left: LIMITS.elements })
  return renderNodes(nodes, 'root', false, 1, { prefix: cleanIdPrefix(idPrefix), width: viewBoxOf(look)[2], left: LIMITS.elements, shown })
}

// ---------------------------------------------------------------- geometry of a look

export interface IndexEntry {
  node: LookNode
  tag: Tag
  /** From the element's own coordinates (inside its own transform) to the viewBox. */
  matrix: Matrix
  /** The elements around it, outermost first. */
  ancestors: readonly LookNode[]
  /** Inside `defs`, a clip path, a mask or a filter: never animated. */
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
      const own = ownTransform(node)
      const matrix = own ? multiply(outer, transformMatrix(own)) : outer
      const nowInert = inert || tag === 'defs' || tag === 'clipPath' || tag === 'mask' || tag === 'filter'
      if (isId(node.id) && !out.has(node.id)) out.set(node.id, { node, tag, matrix, ancestors, inert: nowInert })
      walk(node.children, matrix, [...ancestors, node], nowInert, depth + 1)
    }
  }
  walk(look.nodes, IDENTITY, [], false, 1)
  INDEX.set(look, out)
  return out
}

/** An element's own `transform` as numbers; null when it has none (or one that does not parse). */
export function ownTransform(node: LookNode): TransformOp[] | null {
  const attrs: unknown = node?.attrs
  return typeof attrs === 'object' && attrs !== null && Object.hasOwn(attrs, 'transform') ? parseTransform((attrs as Record<string, unknown>).transform) : null
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

/** A face is never cropped tighter than this share of the viewBox's smaller side (a tiny head would show a few pixels blown up). */
const FACE_LEAST = 0.1

/**
 * What a small face shows: a square around the head role's box with a tenth added on each
 * side, at least a tenth of the viewBox's smaller side, kept inside the viewBox; the whole
 * viewBox when the look names no head.
 */
export function faceViewBox(look: Look): [number, number, number, number] {
  const view = viewBoxOf(look)
  const head = rootBox(look, look?.roles?.head)
  if (!head || head[2] <= 0 || head[3] <= 0) return view
  const side = Math.min(Math.max(Math.max(head[2], head[3]) * 1.2, FACE_LEAST * Math.min(view[2], view[3])), view[2], view[3])
  if (!(side > 0)) return view
  const clamp = (centre: number, from: number, length: number) => Math.min(Math.max(centre - side / 2, from), from + length - side)
  const x = clamp(head[0] + head[2] / 2, view[0], view[2])
  const y = clamp(head[1] + head[3] / 2, view[1], view[3])
  const round = (n: number) => Math.round(n * 1000) / 1000
  return [round(x), round(y), round(side), round(side)]
}

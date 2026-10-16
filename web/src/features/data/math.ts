// The maths of the data viewer, pure so it can be tested: axes (linear, logarithmic, time), SI
// formatting of values with their units, the height of each lane, and the lookups behind the cursor.
// Times are seconds, as the server sends them.

import type { Kind, Lane } from './types'

/** Times above this (about 1973) are Unix seconds: a clock. Mirrors `series::WALL_CLOCK_MIN`. */
export const WALL_CLOCK_MIN = 1.0e8
export const isWall = (t: number) => t > WALL_CLOCK_MIN

// ---------------------------------------------------------------- searching

/** Index of the first element of the ascending `a` that is >= `x` (`a.length` when none is). */
export function lowerBound(a: ArrayLike<number>, x: number): number {
  let lo = 0
  let hi = a.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (a[mid] < x) lo = mid + 1
    else hi = mid
  }
  return lo
}

/** Index of the last element <= `x` (-1 when none is). */
export function atOrBefore(a: ArrayLike<number>, x: number): number {
  let lo = 0
  let hi = a.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (a[mid] <= x) lo = mid + 1
    else hi = mid
  }
  return lo - 1
}

/** Index of the element nearest to `x` (-1 for an empty array). */
export function nearestIndex(a: ArrayLike<number>, x: number): number {
  const n = a.length
  if (!n) return -1
  const i = lowerBound(a, x)
  if (i === 0) return 0
  if (i === n) return n - 1
  return x - a[i - 1] <= a[i] - x ? i - 1 : i
}

// ---------------------------------------------------------------- SI units

const PREFIXES: [number, string][] = [
  [1e-12, 'p'],
  [1e-9, 'n'],
  [1e-6, 'µ'],
  [1e-3, 'm'],
  [1, ''],
  [1e3, 'k'],
  [1e6, 'M'],
  [1e9, 'G'],
  [1e12, 'T'],
]
const PREFIX_OF: Record<string, number> = { p: 1e-12, n: 1e-9, u: 1e-6, µ: 1e-6, μ: 1e-6, m: 1e-3, k: 1e3, M: 1e6, G: 1e9, T: 1e12 }
/** Base units a prefix makes sense on. (`m` is left out: milli or metre.) */
const BASES = new Set(['A', 'V', 'W', 'Hz', 's', 'Ω', 'ohm', 'F', 'H', 'J', 'C', 'Wh', 'Ah', 'Pa', 'N', 'T', 'B'])

export interface UnitInfo {
  /** The unit without its prefix (`A` for `mA`). */
  base: string
  /** Multiply a value in the given unit by this to get it in the base unit. */
  scale: number
  /** The unit takes SI prefixes (µA, mA, A, kA). */
  si: boolean
}

/** `mA` → `{A, 1e-3}`; `°C` → no scaling. */
export function parseUnit(unit: string): UnitInfo {
  const u = unit.trim()
  if (BASES.has(u)) return { base: u, scale: 1, si: true }
  if (u.length >= 2) {
    const f = PREFIX_OF[u[0]]
    const rest = u.slice(1)
    if (f && BASES.has(rest) && rest !== 'T' + 'x') return { base: rest, scale: f, si: true }
  }
  return { base: u, scale: 1, si: false }
}

/** `x` to `sig` significant digits, no exponent unless it must. */
export function trimNumber(x: number, sig = 4): string {
  if (x === 0) return '0'
  const a = Math.abs(x)
  if (a >= 1e15 || a < 1e-6) return x.toExponential(Math.max(0, sig - 1)).replace(/\.?0+e/, 'e')
  return String(Number(x.toPrecision(sig)))
}

/** The prefix that puts `mag` (in the base unit) in 1…1000. */
function prefixFor(mag: number): [number, string] {
  if (!(mag > 0) || !Number.isFinite(mag)) return [1, '']
  let best = PREFIXES[0]
  for (const p of PREFIXES) if (mag >= p[0] * 0.9995) best = p
  return best
}

/** A value with its unit: `0.0052` `A` → "5.2 mA"; `5200` `uA` → "5.2 mA"; `3.3` `V` → "3.3 V"; `21` `°C` → "21 °C". */
export function formatValue(v: number, unit: string, sig = 4): string {
  if (!Number.isFinite(v)) return '—'
  const u = parseUnit(unit)
  if (!u.si) return unit ? `${trimNumber(v, sig)} ${unit}` : trimNumber(v, sig)
  const base = v * u.scale
  if (base === 0) return `0 ${u.base}`
  const [f, p] = prefixFor(Math.abs(base))
  return `${trimNumber(base / f, sig)} ${p}${u.base}`
}

export interface AxisFormat {
  /** The label of a tick value. */
  label: (v: number) => string
  /** The unit shown beside the numbers (with its prefix), when they do not carry it. */
  unit: string
}

/** Tick labels of one lane: all in one prefix (chosen from the largest tick), so 10, 20, 30 mA read as a scale. */
export function axisFormat(ticks: number[], step: number, unit: string, perTick = false): AxisFormat {
  const u = parseUnit(unit)
  if (!u.si) {
    const decimals = decimalsOf(step)
    return { label: (v) => fixed(v, decimals), unit }
  }
  if (perTick) return { label: (v) => formatValue(v, unit, 3), unit: '' }
  const big = Math.max(0, ...ticks.map((t) => Math.abs(t * u.scale)))
  const [f, p] = prefixFor(big)
  const decimals = decimalsOf((step * u.scale) / f)
  return { label: (v) => fixed((v * u.scale) / f, decimals), unit: `${p}${u.base}` }
}

function decimalsOf(step: number): number {
  if (!(step > 0) || !Number.isFinite(step)) return 0
  return Math.min(8, Math.max(0, -Math.floor(Math.log10(step) + 1e-9)))
}

function fixed(v: number, decimals: number): string {
  const s = v.toFixed(decimals)
  return s === '-0' || /^-0\.0*$/.test(s) ? s.slice(1) : s
}

// ---------------------------------------------------------------- axes

export interface Ticks {
  ticks: number[]
  step: number
  lo: number
  hi: number
}

/** Round tick values (1, 2 or 5 times a power of ten) covering `lo`–`hi` with about `target` steps. A flat range is widened. */
export function niceTicks(lo: number, hi: number, target = 5): Ticks {
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) [lo, hi] = [0, 1]
  if (hi < lo) [lo, hi] = [hi, lo]
  if (hi === lo) {
    const pad = Math.abs(lo) * 0.05 || 1
    lo -= pad
    hi += pad
  }
  const raw = (hi - lo) / Math.max(1, target)
  const exp = Math.floor(Math.log10(raw))
  const mag = 10 ** exp
  const f = raw / mag
  const mult = f < 1.5 ? 1 : f < 3 ? 2 : f < 7 ? 5 : 10
  const step = mult * mag
  const first = Math.floor(lo / step + 1e-9)
  const last = Math.ceil(hi / step - 1e-9)
  const ticks: number[] = []
  for (let i = first; i <= last; i++) ticks.push(Number((i * step).toPrecision(15)))
  return { ticks, step, lo: ticks[0], hi: ticks[ticks.length - 1] }
}

export interface LogTicks {
  /** Decades: 1, 10, 100… */
  major: number[]
  /** 2…9 of each decade. */
  minor: number[]
  lo: number
  hi: number
}

/** Decade ticks covering `lo`–`hi` (both above zero). */
export function logTicks(lo: number, hi: number): LogTicks {
  if (!(lo > 0) || !(hi > 0)) [lo, hi] = [1, 10]
  if (hi < lo) [lo, hi] = [hi, lo]
  let a = Math.floor(Math.log10(lo) + 1e-9)
  let b = Math.ceil(Math.log10(hi) - 1e-9)
  if (b === a) b = a + 1
  const major: number[] = []
  const minor: number[] = []
  for (let e = a; e <= b; e++) {
    major.push(Number((10 ** e).toPrecision(12)))
    if (e < b) for (let k = 2; k <= 9; k++) minor.push(Number((k * 10 ** e).toPrecision(12)))
  }
  a = 10 ** a
  return { major, minor, lo: a, hi: Number((10 ** b).toPrecision(12)) }
}

/** Steps (seconds) a time axis uses: 1-2-5 below a minute, then the clock's own. */
const TIME_STEPS_BIG = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 14_400, 21_600, 43_200, 86_400]

/** Round time ticks covering `t0`–`t1` with about `target` steps. */
export function timeTicks(t0: number, t1: number, target = 6): Ticks {
  const span = t1 - t0
  if (!(span > 0)) return { ticks: [t0], step: 1, lo: t0, hi: t0 }
  const raw = span / Math.max(1, target)
  let step: number
  if (raw >= 1) step = TIME_STEPS_BIG.find((s) => s >= raw) ?? Math.ceil(raw / 86_400) * 86_400
  else {
    const exp = Math.floor(Math.log10(raw))
    const mag = 10 ** exp
    const f = raw / mag
    step = (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * mag
  }
  const first = Math.ceil(t0 / step - 1e-9)
  const ticks: number[] = []
  for (let i = first; i * step <= t1 + step * 1e-9 && ticks.length < 200; i++) ticks.push(Number((i * step).toPrecision(15)))
  return { ticks, step, lo: t0, hi: t1 }
}

const TIME_UNITS: [number, string][] = [
  [1, 's'],
  [1e-3, 'ms'],
  [1e-6, 'µs'],
  [1e-9, 'ns'],
]

/** A duration: "1.25 ms", "3.2 s", "1 min 5 s", "2 h 4 min". */
export function formatDuration(sec: number): string {
  if (!Number.isFinite(sec)) return '—'
  const a = Math.abs(sec)
  if (a === 0) return '0 s'
  if (a >= 3600) return `${Math.floor(a / 3600)} h ${Math.round((a % 3600) / 60)} min`
  if (a >= 60) return `${Math.floor(a / 60)} min ${Math.round(a % 60)} s`
  for (const [f, u] of TIME_UNITS) if (a >= f * 0.9995) return `${trimNumber(sec / f, 4)} ${u}`
  return `${trimNumber(sec / 1e-9, 3)} ns`
}

const pad2 = (n: number) => String(n).padStart(2, '0')

/** The time-of-day of a Unix time, to the step's resolution: "14:03:07", "14:03:07.250". */
export function formatClock(t: number, step: number): string {
  const d = new Date(Math.round(t * 1000))
  const base = `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`
  if (step >= 1) return base
  const decimals = Math.min(6, Math.max(1, -Math.floor(Math.log10(step) + 1e-9)))
  const frac = (t - Math.floor(t)).toFixed(decimals).slice(1)
  return base + (frac === '.' + '0'.repeat(decimals) && decimals > 3 ? '' : frac)
}

/** A time axis label. Relative times are in the unit the step calls for: 2 ms, 0.5 s, 1:05. */
export function formatTimeTick(t: number, step: number, wall: boolean): string {
  if (wall) return formatClock(t, step)
  const a = Math.abs(t)
  if (step >= 1) {
    if (a >= 3600) return `${t < 0 ? '−' : ''}${Math.floor(a / 3600)}:${pad2(Math.floor((a % 3600) / 60))}:${pad2(Math.round(a % 60))}`
    if (a >= 60) return `${t < 0 ? '−' : ''}${Math.floor(a / 60)}:${pad2(Math.round(a % 60))}`
    return `${fixed(t, 0)} s`
  }
  // The largest unit the step is at least a tenth of: 0.5 ms reads "0.5 ms", not "500 µs".
  for (const [f, u] of TIME_UNITS) {
    if (step >= f * 0.0999 || u === 'ns') {
      const dec = Math.max(0, -Math.floor(Math.log10(step / f) + 1e-9))
      return `${fixed(t / f, dec)} ${u}`
    }
  }
  return `${t} s`
}

/** The readout of a time under the cursor: wall clock to the millisecond (with the date), else a duration. */
export function formatTimeExact(t: number, wall: boolean): string {
  if (!Number.isFinite(t)) return '—'
  if (!wall) return formatDuration(t)
  const d = new Date(Math.round(t * 1000))
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, '0')}`
}

// ---------------------------------------------------------------- lanes

/** Height a lane gets relative to the others when its weight is 0. */
export function defaultWeight(kind: Kind): number {
  return kind === 'analog' ? 3 : 1
}

/** The least height a lane may be squeezed to, px. */
export function minLaneHeight(kind: Kind): number {
  return kind === 'analog' ? 64 : kind === 'digital' ? 30 : 26
}

export interface LaneBox {
  y0: number
  y1: number
}

/**
 * Where each lane sits in `height` px: shares by weight, none below its minimum (the others give
 * up the difference). When even the minimums do not fit, `total` is what they need and the view scrolls.
 */
export function layoutLanes(height: number, lanes: { weight: number; min: number }[], gap = 6): { boxes: LaneBox[]; total: number } {
  const n = lanes.length
  if (!n) return { boxes: [], total: height }
  const avail = height - gap * (n - 1)
  const minSum = lanes.reduce((s, l) => s + l.min, 0)
  if (minSum >= avail) {
    let y = 0
    const boxes = lanes.map((l) => {
      const b = { y0: y, y1: y + l.min }
      y += l.min + gap
      return b
    })
    return { boxes, total: minSum + gap * (n - 1) }
  }
  const size = lanes.map((l) => l.min)
  const fixed = Array.from({ length: n }, () => false)
  for (let pass = 0; pass < n; pass++) {
    const free = lanes.map((_, i) => i).filter((i) => !fixed[i])
    const room = avail - lanes.reduce((s, _l, i) => s + (fixed[i] ? size[i] : 0), 0)
    const w = free.reduce((s, i) => s + lanes[i].weight, 0)
    let again = false
    for (const i of free) {
      const share = (room * lanes[i].weight) / w
      if (share < lanes[i].min) {
        size[i] = lanes[i].min
        fixed[i] = true
        again = true
      } else size[i] = share
    }
    if (!again) break
  }
  let y = 0
  const boxes = size.map((h) => {
    const b = { y0: y, y1: y + h }
    y += h + gap
    return b
  })
  return { boxes, total: height }
}

/** The kind a lane is drawn as: its first visible trace's, else its first. */
export function laneKind(lane: Lane, kindOf: (series: string) => Kind | undefined): Kind {
  const shown = lane.traces.find((t) => !t.hidden) ?? lane.traces[0]
  return (shown && kindOf(shown.series)) || 'analog'
}

// ---------------------------------------------------------------- views

/** The newest sample at or before `t` (a held value) or null. */
export function heldValue(ts: ArrayLike<number>, vs: ArrayLike<number | null>, t: number): { t: number; v: number | null } | null {
  const i = atOrBefore(ts, t)
  return i < 0 ? null : { t: ts[i], v: vs[i] }
}

/** Zoom `[t0, t1]` by `factor` (above 1 widens) keeping `anchor` where it is on screen. */
export function zoomRange(t0: number, t1: number, anchor: number, factor: number): [number, number] {
  const f = Math.min(Math.max(factor, 1e-6), 1e6)
  return [anchor - (anchor - t0) * f, anchor + (t1 - anchor) * f]
}

/** Slide `[t0, t1]` by `dt`. */
export function panRange(t0: number, t1: number, dt: number): [number, number] {
  return [t0 + dt, t1 + dt]
}

/** The smallest and largest finite value of `v` at indexes `[from, to)`, or null. */
export function extentOf(v: ArrayLike<number | null>, from = 0, to = v.length): [number, number] | null {
  let lo = Infinity
  let hi = -Infinity
  for (let i = from; i < to; i++) {
    const x = v[i]
    if (x === null || !Number.isFinite(x)) continue
    if (x < lo) lo = x
    if (x > hi) hi = x
  }
  return lo <= hi ? [lo, hi] : null
}

/** Spans a following plot offers, seconds (0: everything). */
export const SPANS = [0.001, 0.01, 0.1, 1, 5, 10, 30, 60, 300, 600, 1800, 3600, 0] as const

export function spanLabel(s: number): string {
  return s === 0 ? 'Everything' : formatDuration(s)
}

/** The span next to `s` in `SPANS` (not the "everything" one), wider (`1`) or narrower (`-1`); free spans snap to the nearest. */
export function stepSpan(s: number, dir: 1 | -1): number {
  const list = SPANS.filter((x) => x > 0)
  if (dir === 1) return list.find((x) => x > s * 1.0001) ?? list[list.length - 1]
  return [...list].reverse().find((x) => x < s * 0.9999) ?? list[0]
}

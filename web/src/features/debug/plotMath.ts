// The maths of the plot viewer, kept pure so it can be tested: time windows, axis ticks, the
// reduction of thousands of readings to what a few hundred pixels can show, and the lookups behind
// the crosshair, the legend and the table. Times are milliseconds since the epoch, as the server sends them.

/** One watched value over time: `t` ascending, `v` the reading (NaN: the reading failed, or is no number). */
export interface Track {
  t: number[]
  v: number[]
  /** Parallel to `t`, allocated when the first needs it: the digits of a whole number a double cannot hold exactly (a
   *  64-bit value), `undefined` for every other reading. Drawing uses `v`; what is written down uses these. */
  raw?: (string | undefined)[]
}

/** Categorical colours: `--plot-1` … `--plot-8` (theme/tokens.css). A series keeps its slot for good. */
export const PLOT_SLOTS = 8

/** Time spans the viewer offers. */
export const PLOT_WINDOWS = [5_000, 10_000, 30_000, 60_000, 300_000, 900_000, 1_800_000]
export const DEFAULT_WINDOW = 30_000

export function windowLabel(ms: number): string {
  return ms >= 60_000 && ms % 60_000 === 0 ? `${ms / 60_000} min` : `${ms / 1000} s`
}

/** Readings further apart than this are not joined: the line breaks (the poll stopped, or was slow). */
export function plotGapMs(intervalMs: number): number {
  return Math.max(intervalMs * 6, 1500)
}

/** The first slot (1-based) not in `used`, or null when all eight are taken. */
export function freeSlot(used: Iterable<number>): number | null {
  const taken = new Set(used)
  for (let s = 1; s <= PLOT_SLOTS; s++) if (!taken.has(s)) return s
  return null
}

// ---------------------------------------------------------------- searching a track

/** Index of the first element of the ascending `t` that is >= `x` (`t.length` when none is). */
export function lowerBound(t: ArrayLike<number>, x: number): number {
  let lo = 0
  let hi = t.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (t[mid] < x) lo = mid + 1
    else hi = mid
  }
  return lo
}

/** The readings to draw for the span `t0`–`t1`, as `[from, to)`: one before `t0`, so the line enters from the left edge. */
export function visibleRange(t: ArrayLike<number>, t0: number, t1: number): [number, number] {
  const from = Math.max(0, lowerBound(t, t0) - 1)
  let to = lowerBound(t, t1 + 1) // first reading after t1
  to = Math.min(t.length, to)
  return [from, Math.max(from, to)]
}

/** Index of the reading nearest to `x`, or -1 for an empty track. */
export function nearestIndex(t: ArrayLike<number>, x: number): number {
  const n = t.length
  if (!n) return -1
  const i = lowerBound(t, x)
  if (i === 0) return 0
  if (i === n) return n - 1
  return x - t[i - 1] <= t[i] - x ? i - 1 : i
}

export interface Reading {
  index: number
  t: number
  v: number
  /** The exact digits, for a whole number `v` cannot hold. */
  raw?: string
}

const rawAt = (tr: Track, i: number): { raw?: string } => (tr.raw?.[i] !== undefined ? { raw: tr.raw[i] } : {})

/** The reading nearest to `x` when it is a number within `tolerance` ms of it. */
export function readingAt(tr: Track | undefined, x: number, tolerance: number): Reading | null {
  if (!tr) return null
  const i = nearestIndex(tr.t, x)
  if (i < 0 || Math.abs(tr.t[i] - x) > tolerance || !Number.isFinite(tr.v[i])) return null
  return { index: i, t: tr.t[i], v: tr.v[i], ...rawAt(tr, i) }
}

/** The newest reading that is a number. */
export function lastReading(tr: Track | undefined): Reading | null {
  if (!tr) return null
  for (let i = tr.t.length - 1; i >= 0; i--) if (Number.isFinite(tr.v[i])) return { index: i, t: tr.t[i], v: tr.v[i], ...rawAt(tr, i) }
  return null
}

/** Smallest and largest number among the readings `[from, to)`. */
export function extentOf(tr: Track | undefined, from: number, to: number): [number, number] | null {
  if (!tr) return null
  let lo = Infinity
  let hi = -Infinity
  for (let i = from; i < to; i++) {
    const v = tr.v[i]
    if (v < lo) lo = v
    if (v > hi) hi = v
  }
  return lo <= hi ? [lo, hi] : null
}

/** Smallest and largest number of a track within the span `t0`–`t1`. */
export function spanExtent(tr: Track | undefined, t0: number, t1: number): [number, number] | null {
  if (!tr) return null
  const [from, to] = visibleRange(tr.t, t0, t1)
  return extentOf(tr, from, to)
}

// ---------------------------------------------------------------- scrolling

/** The span next to `ms` among the ones offered, wider (`1`) or narrower (`-1`); `ms` itself at either end. */
export function nextWindow(ms: number, dir: 1 | -1): number {
  const i = PLOT_WINDOWS.findIndex((w) => w >= ms)
  const at = i < 0 ? PLOT_WINDOWS.length - 1 : i
  return PLOT_WINDOWS[Math.min(PLOT_WINDOWS.length - 1, Math.max(0, at + dir))]
}

/**
 * Where the view should end after it is moved to `target` (the time at its right edge). Null means "follow the newest
 * reading": asked for, or the target is not before it. The view never starts before the oldest reading, and when
 * everything kept fits in one span there is nothing to scroll (null).
 */
export function panTo(target: number, oldest: number | null, newest: number | null, windowMs: number): number | null {
  if (newest === null) return null
  const lowest = oldest === null ? newest : Math.min(newest, oldest + windowMs)
  if (!(target < newest - 1)) return null
  return Math.max(lowest, target) >= newest - 1 ? null : Math.max(lowest, target)
}

/** The thumb of the scroll bar as fractions of the track (0–1): where the visible span `t0`–`t1` sits among the readings kept. */
export function scrollThumb(oldest: number | null, newest: number | null, t0: number, t1: number): { start: number; size: number } {
  if (oldest === null || newest === null || newest <= oldest) return { start: 0, size: 1 }
  const total = newest - oldest
  const size = Math.min(1, Math.max(0.04, (t1 - t0) / total)) // never too small to take hold of
  const start = Math.min(1 - size, Math.max(0, (t0 - oldest) / total))
  return { start, size }
}

/** The new view end after zooming from `windowMs` to `next` with `anchor` (a time in view) staying where it is on screen. */
export function zoomEnd(anchor: number, viewEnd: number, windowMs: number, next: number): number {
  return anchor + ((viewEnd - anchor) * next) / windowMs
}

// ---------------------------------------------------------------- axes

export interface Ticks {
  /** The axis range, widened to whole steps so it only moves when a value leaves it. */
  lo: number
  hi: number
  step: number
  ticks: number[]
  /** Digits after the point that tell the ticks apart. */
  decimals: number
}

/** Round tick values (1, 2 or 5 times a power of ten) covering `lo`–`hi` with about `target` steps. A flat range is widened. */
export function niceTicks(lo: number, hi: number, target = 5): Ticks {
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) {
    lo = 0
    hi = 1
  }
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
  const decimals = Math.max(0, -exp - (mult === 10 ? 1 : 0))
  const first = Math.floor(lo / step + 1e-9)
  const last = Math.ceil(hi / step - 1e-9)
  const ticks: number[] = []
  for (let i = first; i <= last; i++) ticks.push(Number((i * step).toPrecision(15))) // significant digits (fixed decimals would zero a tiny range), enough to keep 1e12 + 5 apart
  return { lo: ticks[0], hi: ticks[ticks.length - 1], step, ticks, decimals }
}

const groups = new Map<number, Intl.NumberFormat>()

/** An axis label: grouped thousands, the digits the step needs. */
export function formatTick(v: number, decimals: number): string {
  let f = groups.get(decimals)
  if (!f) {
    f = new Intl.NumberFormat('en-US', { minimumFractionDigits: decimals, maximumFractionDigits: decimals })
    groups.set(decimals, f)
  }
  return f.format(v)
}

const TIME_STEPS = [500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000]

/** Offsets before the right edge (ms) of the time axis labels: 0 (now), then every step. */
export function timeTicks(windowMs: number, target = 6): number[] {
  const step = TIME_STEPS.find((s) => s >= windowMs / target) ?? windowMs
  const out: number[] = []
  for (let o = 0; o <= windowMs + 1; o += step) out.push(o)
  return out
}

/** "now", "−5 s", "−0.5 s", "−1 min", "−1:30". */
export function formatAgo(ms: number): string {
  if (ms <= 0) return 'now'
  if (ms < 59_950) return `−${Number((ms / 1000).toFixed(ms % 1000 === 0 ? 0 : 1))} s`
  const total = Math.round(ms / 1000) // whole seconds first, so 119.6 s is 2 min and not 1:60
  const m = Math.floor(total / 60)
  const s = total % 60
  return s === 0 ? `−${m} min` : `−${m}:${String(s).padStart(2, '0')}`
}

/** The table's time column: "now", "−0.25 s", "−12.50 s", "−1:05.25". */
export function formatAgoExact(ms: number): string {
  if (ms <= 0) return 'now'
  const centi = Math.round(ms / 10) // rounded once, so no part can carry into the next
  if (centi < 6000) return `−${(centi / 100).toFixed(2)} s`
  const rest = centi % 6000
  return `−${Math.floor(centi / 6000)}:${String(Math.floor(rest / 100)).padStart(2, '0')}.${String(rest % 100).padStart(2, '0')}`
}

// ---------------------------------------------------------------- drawing

/**
 * What to draw for a track in the span `t0`–`t1` on a plot `cols` pixel columns wide: runs of `[t, v, t, v, …]`.
 * Each column keeps its first and last reading and its extremes, so a spike survives however many readings
 * there are; a failed reading or a gap longer than `gapMs` ends a run.
 */
export function decimateRuns(tr: Track | undefined, t0: number, t1: number, cols: number, gapMs: number): number[][] {
  if (!tr || cols < 1 || t1 <= t0) return []
  const [from, to] = visibleRange(tr.t, t0, t1)
  const runs: number[][] = []
  let run: number[] = []
  let col = Number.NaN
  // The open pixel column's readings, as indices: the first, the lowest, the highest and the last (-1: none open).
  let first = -1
  let low = -1
  let high = -1
  let last = -1
  const put = (i: number) => run.push(tr.t[i], tr.v[i])
  const flush = () => {
    if (first < 0) return
    // In time order, each once: first <= low, high <= last, so only the two in the middle need ordering.
    put(first)
    const a = Math.min(low, high)
    const b = Math.max(low, high)
    if (a !== first && a !== last) put(a)
    if (b !== a && b !== first && b !== last) put(b)
    if (last !== first) put(last)
    first = -1
  }
  const end = () => {
    flush()
    if (run.length) runs.push(run)
    run = []
    col = Number.NaN
  }
  for (let i = from; i < to; i++) {
    const v = tr.v[i]
    if (!Number.isFinite(v)) {
      end()
      continue
    }
    if (i > from && tr.t[i] - tr.t[i - 1] > gapMs) end()
    const c = Math.floor(((tr.t[i] - t0) / (t1 - t0)) * cols)
    if (c !== col) {
      flush()
      col = c
      first = low = high = last = i
    } else {
      if (v < tr.v[low]) low = i
      if (v > tr.v[high]) high = i
      last = i
    }
  }
  end()
  return runs
}

/**
 * The polylines of a sparkline over the newest `spanMs` of a track, in a `w`×`h` box (`pad` inside the edges): time runs
 * left to right to the newest reading, the largest value is on top, a constant series is a line across the middle, and a
 * failed reading or a hole longer than `gapMs` splits the line. One entry per run, as `"x,y x,y …"`; a run of one reading
 * is a single `"x,y"` (a dot).
 */
export function sparklinePoints(tr: Track | undefined, spanMs: number, w: number, h: number, gapMs: number, pad = 2): string[] {
  const newest = tr?.t.length ? tr.t[tr.t.length - 1] : null
  if (!tr || newest === null) return []
  const t0 = newest - spanMs
  const runs = decimateRuns(tr, t0, newest + 1, Math.max(1, Math.floor(w - 2 * pad)), gapMs)
  let lo = Infinity
  let hi = -Infinity
  for (const run of runs) {
    for (let i = 1; i < run.length; i += 2) {
      lo = Math.min(lo, run[i])
      hi = Math.max(hi, run[i])
    }
  }
  if (lo > hi) return []
  const x = (t: number) => pad + ((t - t0) / spanMs) * (w - 2 * pad)
  const y = (v: number) => (hi === lo ? h / 2 : h - pad - ((v - lo) * (h - 2 * pad)) / (hi - lo))
  return runs.map((run) => {
    const pts: string[] = []
    for (let i = 0; i < run.length; i += 2) pts.push(`${Math.max(pad, x(run[i])).toFixed(1)},${y(run[i + 1]).toFixed(1)}`)
    return pts.join(' ')
  })
}

// ---------------------------------------------------------------- the table and its export

export interface PlotRow {
  t: number
  /** One entry per series, in order: undefined where it has no reading at that time. */
  v: (number | undefined)[]
  /** The exact digits of the entries `v` cannot hold, where there are any. */
  raw?: (string | undefined)[]
}

/**
 * The readings of several tracks as rows, newest first: a row per distinct time within `t0`–`t1`, at most `max`.
 * A poll round gives all its readings one time, so the series line up on equal times.
 */
export function plotRows(tracks: (Track | undefined)[], t0: number, t1: number, max: number): PlotRow[] {
  const at = tracks.map((tr) => (tr ? lowerBound(tr.t, t1 + 1) - 1 : -1)) // each track's newest index within the span
  const rows: PlotRow[] = []
  while (rows.length < max) {
    let t = -Infinity
    tracks.forEach((tr, k) => {
      if (tr && at[k] >= 0 && tr.t[at[k]] > t) t = tr.t[at[k]]
    })
    if (t < t0 || t === -Infinity) break
    let raw: (string | undefined)[] | undefined
    const v = tracks.map((tr, k) => {
      if (!tr || at[k] < 0 || tr.t[at[k]] !== t) return undefined
      const i = at[k]
      at[k]--
      if (tr.raw?.[i] !== undefined) (raw ??= tracks.map(() => undefined))[k] = tr.raw[i]
      return Number.isFinite(tr.v[i]) ? tr.v[i] : undefined
    })
    rows.push(raw ? { t, v, raw } : { t, v })
  }
  return rows
}

/** CSV of table rows: the time in seconds before the newest reading, then a column per series. */
export function plotCsv(names: string[], rows: PlotRow[], newest: number): string {
  const cell = (s: string) => (/[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s)
  // A spreadsheet runs a cell that starts with = + - @ (or a tab or return) as a formula, and the names are text the
  // user typed: a quote in front turns it back into text (the readings are numbers and stay as they are).
  const safe = (s: string) => (/^[=+\-@\t\r]/.test(s) ? `'${s}` : s)
  const lines = [['seconds_ago', 'epoch_ms', ...names.map(safe)].map(cell).join(',')]
  for (const r of rows) lines.push([((newest - r.t) / 1000).toFixed(3), String(r.t), ...r.v.map((x, i) => r.raw?.[i] ?? (x === undefined ? '' : String(x)))].join(','))
  return lines.join('\n') + '\n'
}

// Draws a plot on a canvas: gridlines, axes, one line per series, the end dots, the crosshair and the
// names at the line ends. No React in here, so the layout the pointer handlers use is the one that was drawn.
//
// Marks follow the chart rules of the design: 2 px lines with round joins, 8 px end dots with a 2 px ring in
// the surface colour, hairline solid gridlines, axis and label text in text tokens (never in a series'
// colour; the colour sits in a mark beside the text). The colours are the tokens, read from the page.

import { decimateRuns, extentOf, formatAgo, formatTick, lastReading, lowerBound, nearestIndex, niceTicks, PLOT_SLOTS, plotGapMs, readingAt, timeTicks, visibleRange, type Track } from './plotMath'
import type { PlotScale } from './plotStore'

export interface PlotTheme {
  surface: string
  grid: string
  axis: string
  text: string
  muted: string
  ui: string
  mono: string
  /** `--plot-1` … `--plot-8`, index 0 is slot 1. */
  series: string[]
}

/** The tokens as they are now (theme changes alter them), read from an element in the page. */
export function readPlotTheme(el: Element): PlotTheme {
  const css = getComputedStyle(el)
  const get = (name: string, fallback = '') => css.getPropertyValue(name).trim() || fallback
  return {
    surface: get('--bg-inset', 'transparent'),
    grid: get('--border'),
    axis: get('--border-strong'),
    text: get('--fg'),
    muted: get('--fg-subtle'),
    ui: get('--font-ui', 'sans-serif'),
    mono: get('--font-mono', 'monospace'),
    series: Array.from({ length: PLOT_SLOTS }, (_, i) => get(`--plot-${i + 1}`, get('--accent'))),
  }
}

export interface DrawSeries {
  name: string
  /** 1-based colour slot. */
  slot: number
  track: Track | undefined
  hidden: boolean
}

export interface PlotModel {
  width: number
  height: number
  dpr: number
  series: DrawSeries[]
  windowMs: number
  scale: PlotScale
  /** The time at the right edge. */
  t1: number
  /** The poll interval, which sets how long a hole in the readings may be before the line breaks. */
  intervalMs: number
  /** The time the crosshair is on (a reading's time). */
  cursorT: number | null
  theme: PlotTheme
}

export interface PlotGeometry {
  x0: number
  x1: number
  y0: number
  y1: number
  t0: number
  t1: number
}

/** Names at the line ends only while there are few enough lines to keep them apart. */
export const DIRECT_LABELS_MAX = 4
const PAD = { leftMin: 48, top: 12, bottom: 26, right: 16, rightLabels: 96 }
/** Width of a character of the 11 px axis font (a monospace face is 0.6 em), to size the left margin for the widest label. */
const CHAR_PX = 6.8
/** Line ends nearer than this (px) are not named: the names would sit on each other. */
const LABEL_GAP = 15

/** The plot rectangle. `labelChars` is the length of the widest y-axis label: the left margin grows to fit it (1,000,000 is wider than 100). */
export function layoutOf(width: number, height: number, visible: number, t1: number, windowMs: number, labelChars = 7): PlotGeometry {
  const left = Math.max(PAD.leftMin, Math.ceil(labelChars * CHAR_PX) + 14)
  return { x0: left, x1: Math.max(left + 10, width - (visible > 0 && visible <= DIRECT_LABELS_MAX ? PAD.rightLabels : PAD.right)), y0: PAD.top, y1: Math.max(PAD.top + 10, height - PAD.bottom), t0: t1 - windowMs, t1 }
}

export type LayoutInput = Pick<PlotModel, 'width' | 'height' | 'series' | 'windowMs' | 'scale' | 't1'>

/** The axis and the plot rectangle, which depend on each other: the labels the axis needs decide the margin. */
function plan(m: LayoutInput): { g: PlotGeometry; y: YScale; shown: DrawSeries[] } {
  const shown = m.series.filter((s) => !s.hidden)
  const y = yScaleOf(shown, m.scale, m.t1 - m.windowMs, m.t1, Math.max(10, m.height - PAD.top - PAD.bottom))
  const chars = Math.max(...y.ticks.map((t) => y.label(t).length))
  return { g: layoutOf(m.width, m.height, shown.length, m.t1, m.windowMs, chars), y, shown }
}

/** The geometry `drawPlot` will use for this model: what pointer handlers must map positions with. */
export function layoutFor(m: LayoutInput): PlotGeometry {
  return plan(m).g
}

/** The time under a pixel column of the plot. */
export function timeAtX(g: PlotGeometry, x: number): number {
  return g.t0 + ((x - g.x0) / (g.x1 - g.x0)) * (g.t1 - g.t0)
}

/** The time of the reading nearest to `t` among the shown series: where a crosshair snaps. Null with no readings. */
export function snapTime(series: DrawSeries[], t: number): number | null {
  let best: number | null = null
  for (const s of series) {
    if (s.hidden || !s.track) continue
    const i = nearestIndex(s.track.t, t)
    if (i < 0) continue
    const at = s.track.t[i]
    if (best === null || Math.abs(at - t) < Math.abs(best - t)) best = at
  }
  return best
}

/** The reading time next to `t` among the shown series, after it (`1`) or before it (`-1`): the keyboard moves the crosshair with it. */
export function stepTime(series: DrawSeries[], t: number, dir: 1 | -1): number | null {
  let best: number | null = null
  for (const s of series) {
    if (s.hidden || !s.track) continue
    const idx = dir === 1 ? lowerBound(s.track.t, t + 1) : lowerBound(s.track.t, t) - 1
    if (idx < 0 || idx >= s.track.t.length) continue
    const at = s.track.t[idx]
    if (best === null || (dir === 1 ? at < best : at > best)) best = at
  }
  return best
}

/** The first (`-1`) or last (`1`) reading time within the window of the shown series. */
export function edgeTime(series: DrawSeries[], g: Pick<PlotGeometry, 't0' | 't1'>, end: 1 | -1): number | null {
  let best: number | null = null
  for (const s of series) {
    if (s.hidden || !s.track) continue
    const t = s.track.t
    const [from, to] = visibleRange(t, g.t0, g.t1)
    const inside = (i: number) => t[i] >= g.t0 && t[i] <= g.t1
    let i = end === 1 ? to - 1 : from
    while (i >= from && i < to && !inside(i)) i += end === 1 ? -1 : 1
    if (i < from || i >= to) continue
    if (best === null || (end === 1 ? t[i] > best : t[i] < best)) best = t[i]
  }
  return best
}

const px = (v: number) => Math.round(v) + 0.5

/** The vertical scale: the axis' own range and how a value maps onto it. */
interface YScale {
  ticks: number[]
  decimals: number
  label: (tick: number) => string
  /** 0 (bottom) … 1 (top) for series `s` and value `v`. */
  unit: (s: DrawSeries, v: number) => number
  lo: number
  hi: number
}

function yScaleOf(shown: DrawSeries[], scale: PlotScale, t0: number, t1: number, plotHeight: number): YScale {
  const target = Math.max(2, Math.floor(plotHeight / 44))
  if (scale === 'normalized') {
    const ranges = new Map<DrawSeries, [number, number]>()
    for (const s of shown) {
      const [from, to] = s.track ? visibleRange(s.track.t, t0, t1) : [0, 0]
      const e = extentOf(s.track, from, to)
      if (e) ranges.set(s, e)
    }
    return {
      ticks: [0, 25, 50, 75, 100],
      decimals: 0,
      label: (t) => `${t}%`,
      lo: 0,
      hi: 100,
      unit: (s, v) => {
        const r = ranges.get(s)
        return !r || r[1] === r[0] ? 0.5 : (v - r[0]) / (r[1] - r[0])
      },
    }
  }
  let lo = Infinity
  let hi = -Infinity
  for (const s of shown) {
    if (!s.track) continue
    const [from, to] = visibleRange(s.track.t, t0, t1)
    const e = extentOf(s.track, from, to)
    if (e) {
      lo = Math.min(lo, e[0])
      hi = Math.max(hi, e[1])
    }
  }
  const k = niceTicks(lo, hi, target)
  return { ticks: k.ticks, decimals: k.decimals, label: (t) => formatTick(t, k.decimals), lo: k.lo, hi: k.hi, unit: (_s, v) => (v - k.lo) / (k.hi - k.lo) }
}

function dot(ctx: CanvasRenderingContext2D, x: number, y: number, color: string, surface: string) {
  ctx.beginPath()
  ctx.arc(x, y, 6, 0, Math.PI * 2) // the ring: 2 px of surface around a 4 px radius
  ctx.fillStyle = surface
  ctx.fill()
  ctx.beginPath()
  ctx.arc(x, y, 4, 0, Math.PI * 2)
  ctx.fillStyle = color
  ctx.fill()
}

function fit(ctx: CanvasRenderingContext2D, text: string, width: number): string {
  if (ctx.measureText(text).width <= width) return text
  let s = text
  while (s.length > 1 && ctx.measureText(`${s}…`).width > width) s = s.slice(0, -1)
  return `${s}…`
}

/** Draw the plot; the geometry it used comes back. */
export function drawPlot(ctx: CanvasRenderingContext2D, m: PlotModel): PlotGeometry {
  const { theme } = m
  const { g, y, shown } = plan(m)
  ctx.setTransform(m.dpr, 0, 0, m.dpr, 0, 0)
  ctx.clearRect(0, 0, m.width, m.height)
  const color = (s: DrawSeries) => theme.series[(s.slot - 1) % PLOT_SLOTS]
  const xOf = (t: number) => g.x0 + ((t - g.t0) / (g.t1 - g.t0)) * (g.x1 - g.x0)
  const yOf = (s: DrawSeries, v: number) => g.y1 - y.unit(s, v) * (g.y1 - g.y0)

  // Gridlines and axis labels: hairlines, one step off the surface.
  ctx.lineWidth = 1
  ctx.font = `11px ${theme.mono}`
  ctx.fillStyle = theme.muted
  ctx.textBaseline = 'middle'
  ctx.textAlign = 'right'
  for (const tick of y.ticks) {
    const ty = g.y1 - ((tick - y.lo) / (y.hi - y.lo)) * (g.y1 - g.y0)
    ctx.strokeStyle = theme.grid
    ctx.beginPath()
    ctx.moveTo(g.x0, px(ty))
    ctx.lineTo(g.x1, px(ty))
    ctx.stroke()
    ctx.fillText(y.label(tick), g.x0 - 8, ty)
  }
  ctx.textBaseline = 'top'
  ctx.textAlign = 'center'
  for (const off of timeTicks(m.windowMs, Math.max(2, Math.floor((g.x1 - g.x0) / 90)))) {
    const tx = xOf(g.t1 - off)
    ctx.strokeStyle = theme.grid
    ctx.beginPath()
    ctx.moveTo(px(tx), g.y0)
    ctx.lineTo(px(tx), g.y1)
    ctx.stroke()
    ctx.fillStyle = theme.muted
    // The first and last label sit inside the plot's edges instead of hanging over them.
    ctx.textAlign = off === 0 ? 'right' : off >= m.windowMs ? 'left' : 'center'
    ctx.fillText(formatAgo(off), off === 0 ? g.x1 + 6 : tx, g.y1 + 8)
  }
  ctx.strokeStyle = theme.axis
  ctx.beginPath()
  ctx.moveTo(g.x0, px(g.y1))
  ctx.lineTo(g.x1, px(g.y1))
  ctx.stroke()

  // The lines.
  const gap = plotGapMs(m.intervalMs)
  const cols = Math.max(1, Math.floor(g.x1 - g.x0))
  ctx.save()
  ctx.beginPath()
  ctx.rect(g.x0, g.y0 - 2, g.x1 - g.x0 + 8, g.y1 - g.y0 + 4)
  ctx.clip()
  ctx.lineWidth = 2
  ctx.lineJoin = 'round'
  ctx.lineCap = 'round'
  for (const s of shown) {
    ctx.strokeStyle = color(s)
    ctx.fillStyle = color(s)
    for (const run of decimateRuns(s.track, g.t0, g.t1, cols, gap)) {
      if (run.length === 2) {
        ctx.beginPath()
        ctx.arc(xOf(run[0]), yOf(s, run[1]), 2, 0, Math.PI * 2)
        ctx.fill()
        continue
      }
      ctx.beginPath()
      for (let i = 0; i < run.length; i += 2) {
        const x = xOf(run[i])
        const yy = yOf(s, run[i + 1])
        if (i === 0) ctx.moveTo(x, yy)
        else ctx.lineTo(x, yy)
      }
      ctx.stroke()
    }
  }
  ctx.restore()

  // The newest reading of each line: an end dot, and (with few lines) its name.
  const ends: { s: DrawSeries; y: number }[] = []
  for (const s of shown) {
    const last = lastReading(s.track)
    if (!last || last.t < g.t0 || last.t > g.t1) continue
    const ly = yOf(s, last.v)
    dot(ctx, xOf(last.t), ly, color(s), theme.surface)
    ends.push({ s, y: ly })
  }
  if (shown.length <= DIRECT_LABELS_MAX) {
    ctx.font = `12px ${theme.ui}`
    ctx.textAlign = 'left'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = theme.text
    // Lines that end close together get no names at all: one name beside two dots would say which of them
    // it means, and moving the names apart would detach them from their lines. The legend carries those.
    const sorted = [...ends].sort((a, b) => a.y - b.y)
    sorted.forEach((e, i) => {
      if (i > 0 && e.y - sorted[i - 1].y < LABEL_GAP) return
      if (i < sorted.length - 1 && sorted[i + 1].y - e.y < LABEL_GAP) return
      ctx.fillText(fit(ctx, e.s.name, PAD.rightLabels - 20), g.x1 + 12, e.y)
    })
  }

  // The crosshair.
  if (m.cursorT !== null && m.cursorT >= g.t0 && m.cursorT <= g.t1) {
    const cx = xOf(m.cursorT)
    ctx.strokeStyle = theme.axis
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(px(cx), g.y0)
    ctx.lineTo(px(cx), g.y1)
    ctx.stroke()
    for (const s of shown) {
      const r = readingAt(s.track, m.cursorT, gap)
      if (r) dot(ctx, xOf(r.t), yOf(s, r.v), color(s), theme.surface)
    }
  }
  return g
}

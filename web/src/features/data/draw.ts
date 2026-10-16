// Draws a plot on a canvas: lanes stacked under one time axis, each with its own vertical axis,
// like an oscilloscope or a logic analyzer. No React in here, so the geometry the pointer handlers
// use is the one that was drawn.
//
// Analog traces take the chart colours (`--plot-1` … `--plot-8`); digital lanes and events are
// drawn in the text colour: their lane names them, so colour is not asked to carry identity.
// What is drawn is what the server returned for the window, already thinned to the width.

import { atOrBefore, axisFormat, defaultWeight, formatTimeTick, layoutLanes, logTicks, lowerBound, minLaneHeight, niceTicks, timeTicks, type LaneBox } from './math'
import type { Kind, SeriesView, TraceStyle } from './types'

export interface DrawTheme {
  surface: string
  panel: string
  grid: string
  gridMinor: string
  axis: string
  text: string
  muted: string
  subtle: string
  accent: string
  accentBg: string
  ui: string
  mono: string
  /** `--plot-1` … `--plot-8`, index 0 is colour 1. */
  series: string[]
}

/** The tokens as they are now (theme changes alter them), read from an element in the page. */
export function readTheme(el: Element): DrawTheme {
  const css = getComputedStyle(el)
  const get = (name: string, fallback = '') => css.getPropertyValue(name).trim() || fallback
  return {
    surface: get('--bg-inset', 'transparent'),
    panel: get('--bg-panel', 'transparent'),
    grid: get('--border'),
    gridMinor: get('--border'),
    axis: get('--border-strong'),
    text: get('--fg'),
    muted: get('--fg-muted'),
    subtle: get('--fg-subtle'),
    accent: get('--accent'),
    accentBg: get('--accent-bg'),
    ui: get('--font-ui', 'sans-serif'),
    mono: get('--font-mono', 'monospace'),
    series: Array.from({ length: 8 }, (_, i) => get(`--plot-${i + 1}`, get('--accent'))),
  }
}

export interface DrawTrace {
  name: string
  kind: Kind
  unit: string
  /** 1–8, 0: the text colour. */
  color: number
  style: TraceStyle
  hidden: boolean
  view: SeriesView | undefined
  /** The newest time the series has (a digital level holds until it). */
  end: number | null
}

export interface DrawLane {
  title: string
  kind: Kind
  unit: string
  log: boolean
  min: number | null
  max: number | null
  weight: number
  traces: DrawTrace[]
}

export interface DrawModel {
  width: number
  height: number
  dpr: number
  t0: number
  t1: number
  lanes: DrawLane[]
  /** Events series drawn as vertical lines through every lane. */
  markers: { name: string; events: [number, string][] }[]
  cursorT: number | null
  selection: [number, number] | null
  wall: boolean
  theme: DrawTheme
  /** Shown over the lanes while there is nothing to draw. */
  empty: string | null
}

export interface LaneGeom {
  box: LaneBox
  kind: Kind
  log: boolean
  lo: number
  hi: number
  /** A value's y; NaN where it cannot be drawn (not above zero on a log axis). */
  toY: (v: number) => number
  fromY: (y: number) => number
  ticks: number[]
  minor: number[]
  label: (v: number) => string
  axisUnit: string
}

export interface Geometry {
  /** The plot rectangle's left and right edges. */
  x0: number
  x1: number
  /** Top of the time axis. */
  axisY: number
  lanes: LaneGeom[]
  /** Height the drawing needs: the canvas is at least this tall. */
  total: number
  timeStep: number
  timeTicks: number[]
}

const PAD = { left: 74, right: 14, top: 6, axis: 24 }
const GAP = 6

export const geometryPad = PAD

/** Height the lanes of a model need at least (the canvas grows to it and the panel scrolls). */
export function neededHeight(lanes: { kind: Kind }[]): number {
  if (!lanes.length) return 0
  return PAD.top + PAD.axis + lanes.reduce((s, l) => s + minLaneHeight(l.kind), 0) + GAP * (lanes.length - 1)
}

const px = (v: number) => Math.round(v) + 0.5

/** The vertical axis of a lane: fixed ends, or fitted to the values of its shown traces within the window. */
function axisOf(l: DrawLane, box: LaneBox, t0: number, t1: number): LaneGeom {
  const h = Math.max(10, box.y1 - box.y0)
  const base = { box, kind: l.kind, log: l.log }
  if (l.kind !== 'analog') {
    // Levels: 0 at the bottom, 1 at the top (a bus is drawn as boxes and does not use the axis).
    const top = box.y0 + 7
    const bottom = box.y1 - 7
    return { ...base, lo: 0, hi: 1, toY: (v) => bottom - v * (bottom - top), fromY: (y) => (bottom - y) / (bottom - top), ticks: [], minor: [], label: (v) => String(v), axisUnit: '' }
  }
  // The values in view: the samples inside the window, and where the line crosses its two edges
  // (not the samples beyond them, which may be far away and far off).
  let lo = Infinity
  let hi = -Infinity
  const take = (x: number) => {
    if (!Number.isFinite(x) || (l.log && x <= 0)) return
    if (x < lo) lo = x
    if (x > hi) hi = x
  }
  for (const t of l.traces) {
    if (t.hidden || !t.view?.t || !t.view.v) continue
    const ts = t.view.t
    const vs = t.view.v
    const i0 = lowerBound(ts, t0)
    const i1 = atOrBefore(ts, t1) + 1
    for (let i = i0; i < i1; i++) {
      const x = vs[i]
      if (x !== null) take(x)
    }
    const at = (a: number, b: number, edge: number) => {
      const [va, vb] = [vs[a], vs[b]]
      if (va === null || vb === null || !Number.isFinite(va) || !Number.isFinite(vb)) return
      if (t.style === 'step') return take(va)
      take(va + ((vb - va) * (edge - ts[a])) / (ts[b] - ts[a]))
    }
    if (i0 > 0 && i0 < ts.length) at(i0 - 1, i0, t0)
    if (i1 > 0 && i1 < ts.length) at(i1 - 1, i1, t1)
  }
  if (!(lo <= hi)) {
    lo = l.log ? 1 : 0
    hi = l.log ? 10 : 1
  }
  const target = Math.max(2, Math.floor(h / 36))
  if (l.log) {
    const t = logTicks(l.min && l.min > 0 ? l.min : lo, l.max && l.max > 0 ? l.max : hi)
    const a = Math.log10(t.lo)
    const b = Math.log10(t.hi)
    const top = box.y0 + 6
    const bottom = box.y1 - 6
    const f = axisFormat(t.major, 1, l.unit, true)
    return {
      ...base,
      lo: t.lo,
      hi: t.hi,
      toY: (v) => (v > 0 ? bottom - ((Math.log10(v) - a) / (b - a)) * (bottom - top) : Number.NaN),
      fromY: (y) => 10 ** (a + ((bottom - y) / (bottom - top)) * (b - a)),
      ticks: t.major,
      minor: t.minor,
      label: f.label,
      axisUnit: f.unit,
    }
  }
  let a = l.min ?? lo
  let b = l.max ?? hi
  if (l.min === null && l.max === null) {
    // Room above and below, so a flat trace sits in the middle and a peak does not touch the edge.
    const pad = (b - a) * 0.06 || Math.abs(a) * 0.05 || 0.5
    a -= pad
    b += pad
  } else if (b <= a) {
    b = a + 1
  }
  const k = niceTicks(a, b, target)
  const [ya, yb] = l.min !== null && l.max !== null ? [a, b] : [k.lo, k.hi]
  const top = box.y0 + 6
  const bottom = box.y1 - 6
  const f = axisFormat(k.ticks, k.step, l.unit)
  return {
    ...base,
    lo: ya,
    hi: yb,
    toY: (v) => bottom - ((v - ya) / (yb - ya)) * (bottom - top),
    fromY: (y) => ya + ((bottom - y) / (bottom - top)) * (yb - ya),
    ticks: k.ticks.filter((t) => t >= ya - 1e-12 && t <= yb + 1e-12),
    minor: [],
    label: f.label,
    axisUnit: f.unit,
  }
}

/** The geometry `drawPlot` uses for this model: what pointer handlers map positions with. */
export function geometryOf(m: Pick<DrawModel, 'width' | 'height' | 'lanes' | 't0' | 't1'>): Geometry {
  const x0 = PAD.left
  const x1 = Math.max(x0 + 10, m.width - PAD.right)
  const inner = Math.max(10, m.height - PAD.top - PAD.axis)
  const { boxes, total } = layoutLanes(
    inner,
    m.lanes.map((l) => ({ weight: l.weight > 0 ? l.weight : defaultWeight(l.kind), min: minLaneHeight(l.kind) })),
    GAP,
  )
  const lanes = m.lanes.map((l, i) => axisOf(l, { y0: PAD.top + boxes[i].y0, y1: PAD.top + boxes[i].y1 }, m.t0, m.t1))
  const ticks = timeTicks(m.t0, m.t1, Math.max(2, Math.floor((x1 - x0) / 80)))
  return { x0, x1, axisY: PAD.top + (boxes.length ? boxes[boxes.length - 1].y1 : 0) + 4, lanes, total: total + PAD.top + PAD.axis, timeStep: ticks.step, timeTicks: ticks.ticks }
}

/** The time under a pixel column. */
export function timeAt(g: Pick<Geometry, 'x0' | 'x1'>, t0: number, t1: number, x: number): number {
  return t0 + ((x - g.x0) / (g.x1 - g.x0)) * (t1 - t0)
}

export function xAt(g: Pick<Geometry, 'x0' | 'x1'>, t0: number, t1: number, t: number): number {
  return g.x0 + ((t - t0) / (t1 - t0)) * (g.x1 - g.x0)
}

/** The lane under a y, or -1. */
export function laneAt(g: Geometry, y: number): number {
  return g.lanes.findIndex((l) => y >= l.box.y0 - GAP / 2 && y <= l.box.y1 + GAP / 2)
}

function fit(ctx: CanvasRenderingContext2D, text: string, width: number): string {
  if (width <= 0) return ''
  if (ctx.measureText(text).width <= width) return text
  let s = text
  while (s.length > 1 && ctx.measureText(`${s}…`).width > width) s = s.slice(0, -1)
  return s.length > 1 ? `${s}…` : ''
}

const colorOf = (t: DrawTrace, theme: DrawTheme) => (t.kind === 'analog' && t.color >= 1 ? theme.series[(t.color - 1) % 8] : theme.text)

/** Draw the plot; the geometry it used comes back. */
export function drawPlot(ctx: CanvasRenderingContext2D, m: DrawModel): Geometry {
  const { theme } = m
  const g = geometryOf(m)
  ctx.setTransform(m.dpr, 0, 0, m.dpr, 0, 0)
  ctx.clearRect(0, 0, m.width, m.height)
  const xOf = (t: number) => g.x0 + ((t - m.t0) / (m.t1 - m.t0)) * (g.x1 - g.x0)
  const plotTop = PAD.top
  const plotBottom = g.axisY - 4

  // Time gridlines run through every lane; the axis labels sit below.
  ctx.lineWidth = 1
  ctx.strokeStyle = theme.grid
  for (const t of g.timeTicks) {
    const x = px(xOf(t))
    if (x < g.x0 - 0.5 || x > g.x1 + 0.5) continue
    ctx.beginPath()
    ctx.moveTo(x, plotTop)
    ctx.lineTo(x, plotBottom)
    ctx.stroke()
  }
  ctx.font = `11px ${theme.mono}`
  ctx.fillStyle = theme.muted
  ctx.textBaseline = 'top'
  ctx.textAlign = 'center'
  let lastRight = -Infinity
  for (const t of g.timeTicks) {
    const x = xOf(t)
    if (x < g.x0 - 1 || x > g.x1 + 1) continue
    const label = formatTimeTick(t, g.timeStep, m.wall)
    const w = ctx.measureText(label).width
    if (x - w / 2 < lastRight + 6) continue
    ctx.fillText(label, Math.min(Math.max(x, g.x0 + w / 2), m.width - w / 2), g.axisY + 4)
    lastRight = x + w / 2
  }

  m.lanes.forEach((lane, li) => {
    const lg = g.lanes[li]
    const { y0, y1 } = lg.box
    // The lane's field.
    ctx.fillStyle = theme.surface
    ctx.fillRect(g.x0, y0, g.x1 - g.x0, y1 - y0)
    ctx.strokeStyle = theme.grid
    ctx.strokeRect(g.x0 + 0.5, y0 + 0.5, g.x1 - g.x0 - 1, y1 - y0 - 1)

    if (lane.kind === 'analog') {
      ctx.font = `11px ${theme.mono}`
      ctx.textBaseline = 'middle'
      ctx.textAlign = 'right'
      const draw = (v: number, major: boolean) => {
        const y = lg.toY(v)
        if (!Number.isFinite(y) || y < y0 + 1 || y > y1 - 1) return
        ctx.strokeStyle = major ? theme.grid : theme.gridMinor
        ctx.globalAlpha = major ? 1 : 0.45
        ctx.beginPath()
        ctx.moveTo(g.x0 + 1, px(y))
        ctx.lineTo(g.x1 - 1, px(y))
        ctx.stroke()
        ctx.globalAlpha = 1
        if (major) {
          ctx.fillStyle = theme.muted
          ctx.fillText(lg.label(v), g.x0 - 6, y)
        }
      }
      lg.minor.forEach((v) => draw(v, false))
      lg.ticks.forEach((v) => draw(v, true))
    } else {
      // Digital and events lanes are named at their left edge.
      ctx.font = `12px ${theme.ui}`
      ctx.textBaseline = 'middle'
      ctx.textAlign = 'right'
      ctx.fillStyle = theme.text
      const name = lane.title || lane.traces.map((t) => t.name).join(', ')
      ctx.fillText(fit(ctx, name, PAD.left - 12), g.x0 - 8, (y0 + y1) / 2)
    }

    // The traces, clipped to the lane.
    ctx.save()
    ctx.beginPath()
    ctx.rect(g.x0, y0, g.x1 - g.x0, y1 - y0)
    ctx.clip()
    for (const t of lane.traces) {
      if (t.hidden || !t.view || t.view.missing) continue
      ctx.strokeStyle = colorOf(t, theme)
      ctx.fillStyle = colorOf(t, theme)
      if (t.kind === 'events') drawEvents(ctx, t, lg, xOf, g, theme)
      else if (t.kind === 'digital') drawDigital(ctx, t, lg, xOf, g, m, theme)
      else drawAnalog(ctx, t, lg, xOf)
    }
    ctx.restore()

    // An analog lane says what it holds, top left, over a patch of the field.
    if (lane.kind === 'analog') {
      const named = lane.title || lane.traces.map((t) => t.name).join(', ')
      const title = lg.axisUnit ? `${named} [${lg.axisUnit}]` : named
      ctx.font = `11px ${theme.ui}`
      ctx.textBaseline = 'top'
      ctx.textAlign = 'left'
      const text = fit(ctx, title, Math.min(260, g.x1 - g.x0 - 16))
      if (text) {
        ctx.globalAlpha = 0.8
        ctx.fillStyle = theme.surface
        ctx.fillRect(g.x0 + 4, y0 + 3, ctx.measureText(text).width + 8, 15)
        ctx.globalAlpha = 1
        ctx.fillStyle = theme.muted
        ctx.fillText(text, g.x0 + 8, y0 + 5)
      }
    }
  })

  // Markers: events drawn through every lane.
  ctx.save()
  ctx.beginPath()
  ctx.rect(g.x0, plotTop, g.x1 - g.x0, plotBottom - plotTop)
  ctx.clip()
  ctx.font = `11px ${theme.ui}`
  ctx.textBaseline = 'top'
  ctx.textAlign = 'left'
  for (const mk of m.markers) {
    let lastLabel = -Infinity
    for (const [t, text] of mk.events) {
      if (t < m.t0 || t > m.t1) continue
      const x = px(xOf(t))
      ctx.strokeStyle = theme.accent
      ctx.setLineDash([4, 3])
      ctx.beginPath()
      ctx.moveTo(x, plotTop)
      ctx.lineTo(x, plotBottom)
      ctx.stroke()
      ctx.setLineDash([])
      if (x > lastLabel + 8) {
        const label = fit(ctx, text, Math.min(160, g.x1 - x - 4))
        if (label) {
          ctx.fillStyle = theme.accent
          ctx.fillText(label, x + 3, plotTop + 1)
          lastLabel = x + ctx.measureText(label).width + 6
        }
      }
    }
  }
  ctx.restore()

  // A selected range.
  if (m.selection) {
    const [a, b] = m.selection
    const xa = Math.min(g.x1, Math.max(g.x0, xOf(Math.min(a, b))))
    const xb = Math.min(g.x1, Math.max(g.x0, xOf(Math.max(a, b))))
    ctx.fillStyle = theme.accentBg
    ctx.fillRect(xa, plotTop, Math.max(1, xb - xa), plotBottom - plotTop)
    ctx.strokeStyle = theme.accent
    ctx.lineWidth = 1
    for (const x of [xOf(a), xOf(b)]) {
      if (x < g.x0 || x > g.x1) continue
      ctx.beginPath()
      ctx.moveTo(px(x), plotTop)
      ctx.lineTo(px(x), plotBottom)
      ctx.stroke()
    }
  }

  // The cursor, and where each analog trace is under it.
  if (m.cursorT !== null && m.cursorT >= m.t0 && m.cursorT <= m.t1) {
    const cx = xOf(m.cursorT)
    ctx.strokeStyle = theme.muted
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(px(cx), plotTop)
    ctx.lineTo(px(cx), plotBottom)
    ctx.stroke()
    m.lanes.forEach((lane, li) => {
      for (const t of lane.traces) {
        if (t.hidden || t.kind !== 'analog' || !t.view?.t || !t.view.v) continue
        const i = nearestFinite(t.view.t, t.view.v, m.cursorT!)
        if (i < 0) continue
        const y = g.lanes[li].toY(t.view.v[i] as number)
        if (!Number.isFinite(y)) continue
        const x = xOf(t.view.t[i])
        if (x < g.x0 || x > g.x1) continue
        ctx.beginPath()
        ctx.arc(x, y, 5, 0, Math.PI * 2)
        ctx.fillStyle = theme.surface
        ctx.fill()
        ctx.beginPath()
        ctx.arc(x, y, 3.5, 0, Math.PI * 2)
        ctx.fillStyle = colorOf(t, theme)
        ctx.fill()
      }
    })
  }

  if (m.empty) {
    ctx.font = `13px ${theme.ui}`
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = theme.subtle
    ctx.fillText(m.empty, (g.x0 + g.x1) / 2, (plotTop + plotBottom) / 2)
  }
  return g
}

/** Index of the sample with a number nearest to `x`, -1 when there is none. */
export function nearestFinite(ts: number[], vs: (number | null)[], x: number): number {
  const n = ts.length
  if (!n) return -1
  let lo = 0
  let hi = n
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (ts[mid] < x) lo = mid + 1
    else hi = mid
  }
  let best = -1
  for (const i of [lo - 1, lo, lo - 2, lo + 1]) {
    if (i < 0 || i >= n || vs[i] === null || !Number.isFinite(vs[i] as number)) continue
    if (best < 0 || Math.abs(ts[i] - x) < Math.abs(ts[best] - x)) best = i
  }
  return best
}

function drawAnalog(ctx: CanvasRenderingContext2D, tr: DrawTrace, lg: LaneGeom, xOf: (t: number) => number) {
  const t = tr.view!.t
  const v = tr.view!.v
  if (!t || !v || !t.length) return
  const step = tr.style === 'step'
  const dots = tr.style === 'dots'
  ctx.lineWidth = t.length > 3000 ? 1.25 : 1.75
  ctx.lineJoin = 'round'
  ctx.lineCap = 'round'
  let open = false
  let run = 0
  let px0 = 0
  let py0 = 0
  const endRun = () => {
    if (!open) return
    if (run === 1) {
      ctx.beginPath()
      ctx.arc(px0, py0, 2.25, 0, Math.PI * 2)
      ctx.fill()
    } else ctx.stroke()
    open = false
    run = 0
  }
  for (let i = 0; i < t.length; i++) {
    const val = v[i]
    const y = val === null || !Number.isFinite(val) ? Number.NaN : lg.toY(val)
    if (!Number.isFinite(y)) {
      endRun()
      continue
    }
    const x = xOf(t[i])
    if (dots) {
      ctx.beginPath()
      ctx.arc(x, y, 2.25, 0, Math.PI * 2)
      ctx.fill()
      continue
    }
    if (!open) {
      ctx.beginPath()
      ctx.moveTo(x, y)
      open = true
      run = 1
    } else {
      if (step) ctx.lineTo(x, py0)
      ctx.lineTo(x, y)
      run++
    }
    px0 = x
    py0 = y
  }
  endRun()
}

function drawDigital(ctx: CanvasRenderingContext2D, tr: DrawTrace, lg: LaneGeom, xOf: (t: number) => number, g: Geometry, m: DrawModel, theme: DrawTheme) {
  const t = tr.view!.t
  const v = tr.view!.v
  if (!t || !v || !t.length) return
  const end = Math.min(m.t1, tr.end ?? t[t.length - 1])
  const top = lg.box.y0 + 7
  const bottom = lg.box.y1 - 7
  const isBus = v.some((x) => x !== null && Number.isFinite(x) && x > 1)
  ctx.lineWidth = 1.5
  ctx.lineJoin = 'miter'
  if (isBus) {
    ctx.font = `11px ${theme.mono}`
    ctx.textBaseline = 'middle'
    ctx.textAlign = 'left'
    const mid = (top + bottom) / 2
    for (let i = 0; i < t.length; i++) {
      const next = i + 1 < t.length ? t[i + 1] : end
      const xa = Math.max(g.x0 - 4, xOf(t[i]))
      const xb = Math.min(g.x1 + 4, xOf(next))
      if (xb < g.x0 || xa > g.x1 || xb - xa < 1) continue
      const val = v[i]
      const known = val !== null && Number.isFinite(val)
      const s = Math.min(4, (xb - xa) / 2)
      ctx.beginPath()
      ctx.moveTo(xa, mid)
      ctx.lineTo(xa + s, top)
      ctx.lineTo(xb - s, top)
      ctx.lineTo(xb, mid)
      ctx.lineTo(xb - s, bottom)
      ctx.lineTo(xa + s, bottom)
      ctx.closePath()
      ctx.globalAlpha = known ? 1 : 0.35
      ctx.stroke()
      ctx.globalAlpha = 1
      if (known && xb - xa > 26) {
        ctx.fillStyle = theme.text
        const label = fit(ctx, `0x${Math.round(val as number).toString(16).toUpperCase()}`, xb - xa - s * 2 - 4)
        ctx.fillText(label, xa + s + 3, mid)
      }
    }
    return
  }
  ctx.beginPath()
  let started = false
  let lastY = 0
  for (let i = 0; i < t.length; i++) {
    const val = v[i]
    if (val === null || !Number.isFinite(val)) {
      // Unknown: the line breaks, and the stretch is shaded.
      started = false
      const next = i + 1 < t.length ? t[i + 1] : end
      ctx.stroke()
      ctx.save()
      ctx.globalAlpha = 0.18
      ctx.fillRect(xOf(t[i]), top, Math.max(1, xOf(next) - xOf(t[i])), bottom - top)
      ctx.restore()
      ctx.beginPath()
      continue
    }
    const x = xOf(t[i])
    const y = val > 0 ? top : bottom
    if (!started) {
      ctx.moveTo(x, y)
      started = true
    } else {
      ctx.lineTo(x, lastY)
      ctx.lineTo(x, y)
    }
    lastY = y
  }
  if (started) ctx.lineTo(Math.max(xOf(t[t.length - 1]), xOf(end)), lastY)
  ctx.stroke()
}

function drawEvents(ctx: CanvasRenderingContext2D, tr: DrawTrace, lg: LaneGeom, xOf: (t: number) => number, g: Geometry, theme: DrawTheme) {
  const ev = tr.view!.events
  if (!ev) return
  ctx.font = `11px ${theme.ui}`
  ctx.textBaseline = 'middle'
  ctx.textAlign = 'left'
  const mid = (lg.box.y0 + lg.box.y1) / 2
  ctx.lineWidth = 1.5
  for (let i = 0; i < ev.length; i++) {
    const x = xOf(ev[i][0])
    if (x < g.x0 - 1 || x > g.x1 + 1) continue
    ctx.beginPath()
    ctx.moveTo(px(x), lg.box.y0 + 4)
    ctx.lineTo(px(x), lg.box.y1 - 4)
    ctx.stroke()
    // The text runs to the next event.
    const nextX = i + 1 < ev.length ? xOf(ev[i + 1][0]) : g.x1
    const room = Math.min(nextX, g.x1) - x - 6
    if (room > 14) ctx.fillText(fit(ctx, ev[i][1], room), x + 4, mid)
  }
}

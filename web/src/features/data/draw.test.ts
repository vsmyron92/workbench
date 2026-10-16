import { describe, expect, it } from 'vitest'
import { drawPlot, geometryOf, laneAt, neededHeight, nearestFinite, timeAt, xAt, type DrawLane, type DrawModel, type DrawTheme, type DrawTrace } from './draw'
import { minLaneHeight } from './math'
import type { SeriesView } from './types'

const theme: DrawTheme = { surface: '#surface', panel: '#panel', grid: '#grid', gridMinor: '#gridm', axis: '#axis', text: '#text', muted: '#muted', subtle: '#subtle', accent: '#accent', accentBg: '#accentbg', ui: 'ui', mono: 'mono', series: ['#s1', '#s2', '#s3', '#s4', '#s5', '#s6', '#s7', '#s8'] }

interface Call {
  fn: string
  args: unknown[]
  strokeStyle: unknown
  fillStyle: unknown
  lineWidth: unknown
}

function recorder() {
  const calls: Call[] = []
  const state: Record<string, unknown> = {}
  const ctx = new Proxy(
    {},
    {
      get(_t, prop: string) {
        if (prop === 'measureText') return (s: string) => ({ width: s.length * 6 })
        if (prop in state) return state[prop]
        return (...args: unknown[]) => void calls.push({ fn: prop, args, strokeStyle: state.strokeStyle, fillStyle: state.fillStyle, lineWidth: state.lineWidth })
      },
      set(_t, prop: string, value) {
        state[prop] = value
        return true
      },
    },
  ) as unknown as CanvasRenderingContext2D
  return { ctx, calls }
}

const view = (t: number[], v: (number | null)[]): SeriesView => ({ t, v, n: t.length, thinned: false })

function trace(over: Partial<DrawTrace> & { name: string }): DrawTrace {
  return { kind: 'analog', unit: '', color: 1, style: '', hidden: false, view: undefined, end: null, ...over }
}

function lane(traces: DrawTrace[], over: Partial<DrawLane> = {}): DrawLane {
  return { title: '', kind: traces[0]?.kind ?? 'analog', unit: traces[0]?.unit ?? '', log: false, min: null, max: null, weight: 0, traces, ...over }
}

const model = (lanes: DrawLane[], over: Partial<DrawModel> = {}): DrawModel => ({
  width: 800,
  height: 400,
  dpr: 2,
  t0: 0,
  t1: 10,
  lanes,
  markers: [],
  cursorT: null,
  selection: null,
  wall: false,
  theme,
  empty: null,
  ...over,
})

const current = trace({ name: 'I', unit: 'A', color: 2, view: view([0, 5, 10], [0.001, 0.05, 0.002]) })
const clk = trace({ name: 'CLK', kind: 'digital', color: 0, view: view([0, 2, 4, 6], [0, 1, 0, 1]), end: 8 })

describe('the geometry', () => {
  it('stacks lanes inside the plot and keeps the time axis below them', () => {
    const g = geometryOf(model([lane([current]), lane([clk])]))
    expect(g.lanes).toHaveLength(2)
    expect(g.lanes[0].box.y1).toBeLessThanOrEqual(g.lanes[1].box.y0)
    expect(g.axisY).toBeGreaterThanOrEqual(g.lanes[1].box.y1)
    expect(g.x1).toBeGreaterThan(g.x0)
    // The analog lane is the taller one.
    expect(g.lanes[0].box.y1 - g.lanes[0].box.y0).toBeGreaterThan(g.lanes[1].box.y1 - g.lanes[1].box.y0)
  })

  it('asks for more height than there is when the minimums do not fit', () => {
    const lanes = [lane([current]), lane([clk]), lane([clk])]
    const need = neededHeight(lanes)
    expect(need).toBeGreaterThan(minLaneHeight('analog') + 2 * minLaneHeight('digital'))
    const g = geometryOf(model(lanes, { height: 100 }))
    expect(g.total).toBeGreaterThanOrEqual(need - 1)
  })

  it('maps pixels to time and back, and finds the lane under a y', () => {
    const m = model([lane([current]), lane([clk])])
    const g = geometryOf(m)
    expect(timeAt(g, 0, 10, g.x0)).toBe(0)
    expect(timeAt(g, 0, 10, g.x1)).toBe(10)
    expect(xAt(g, 0, 10, 5)).toBeCloseTo((g.x0 + g.x1) / 2)
    expect(laneAt(g, (g.lanes[1].box.y0 + g.lanes[1].box.y1) / 2)).toBe(1)
    expect(laneAt(g, g.axisY + 50)).toBe(-1)
  })

  it('fits an analog lane to the values in view, or to a fixed range', () => {
    const fitted = geometryOf(model([lane([current])])).lanes[0]
    expect(fitted.lo).toBeLessThanOrEqual(0.001)
    expect(fitted.hi).toBeGreaterThanOrEqual(0.05)
    const fixed = geometryOf(model([lane([current], { min: 0, max: 0.1 })])).lanes[0]
    expect([fixed.lo, fixed.hi]).toEqual([0, 0.1])
    // Higher values are higher on the screen.
    expect(fixed.toY(0.1)).toBeLessThan(fixed.toY(0.01))
    expect(fixed.fromY(fixed.toY(0.04))).toBeCloseTo(0.04)
  })

  it('draws a logarithmic lane in decades, and leaves out what cannot be on it', () => {
    const l = geometryOf(model([lane([current], { log: true })])).lanes[0]
    expect(l.log).toBe(true)
    expect(l.ticks).toContain(0.01)
    expect(Number.isNaN(l.toY(0))).toBe(true)
    expect(Number.isNaN(l.toY(-1))).toBe(true)
    expect(l.toY(0.1)).toBeLessThan(l.toY(0.001))
  })

  it('does not fit the lane to samples outside the window', () => {
    const wide = trace({ name: 'I', unit: 'A', view: view([-5, 0, 1, 2, 20], [900, 1, 2, 3, 800]) })
    const l = geometryOf(model([lane([wide])], { t0: 0, t1: 2.5 })).lanes[0]
    // The line is at 3 at t=2 and climbs towards 800 at t=20: at the right edge, t=2.5, it is about 25.
    // The lane fits that, not the far samples themselves (900 and 800).
    expect(l.hi).toBeGreaterThan(24)
    expect(l.hi).toBeLessThan(40)
    expect(l.lo).toBeGreaterThanOrEqual(-3)
  })
})

describe('drawing', () => {
  it('draws an analog trace in its chart colour and a digital one in the text colour', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([current]), lane([clk])]))
    const strokes = calls.filter((c) => c.fn === 'stroke')
    expect(strokes.some((c) => c.strokeStyle === '#s2' && c.lineWidth === 1.75)).toBe(true)
    expect(strokes.some((c) => c.strokeStyle === '#text' && c.lineWidth === 1.5)).toBe(true)
    expect(calls[0]).toMatchObject({ fn: 'setTransform', args: [2, 0, 0, 2, 0, 0] })
  })

  it('does not draw a hidden trace', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([{ ...current, hidden: true }])]))
    expect(calls.some((c) => c.strokeStyle === '#s2')).toBe(false)
  })

  it('names digital lanes at their edge and writes the empty message', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([clk])], { empty: 'Waiting for data…' }))
    const texts = calls.filter((c) => c.fn === 'fillText').map((c) => String(c.args[0]))
    expect(texts).toContain('CLK')
    expect(texts).toContain('Waiting for data…')
  })

  it('breaks the line at a failed reading', () => {
    const t = trace({ name: 'x', view: view([0, 1, 2, 3, 4], [1, 2, null, 3, 4]) })
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([t])]))
    // Two runs of two samples each: two strokes in the trace colour.
    expect(calls.filter((c) => c.fn === 'stroke' && c.strokeStyle === '#s1' && c.lineWidth === 1.75)).toHaveLength(2)
  })

  it('draws a bus as boxes with its value', () => {
    const bus = trace({ name: 'ADDR', kind: 'digital', color: 0, view: view([0, 4, 8], [0x10, 0xff, 0x3]), end: 10 })
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([bus])]))
    const texts = calls.filter((c) => c.fn === 'fillText').map((c) => String(c.args[0]))
    expect(texts).toContain('0x10')
    expect(texts).toContain('0xFF')
  })

  it('draws events as ticks with their text, and markers through every lane', () => {
    const ev = trace({ name: 'marks', kind: 'events', color: 0, view: { events: [[2, 'wake'], [6, 'sleep']] } })
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([current]), lane([ev])], { markers: [{ name: 'trig', events: [[5, 'trigger']] }] }))
    const texts = calls.filter((c) => c.fn === 'fillText').map((c) => String(c.args[0]))
    expect(texts).toEqual(expect.arrayContaining(['wake', 'sleep', 'trigger']))
    expect(calls.some((c) => c.fn === 'setLineDash' && JSON.stringify(c.args[0]) === '[4,3]')).toBe(true)
  })

  it('shades a selected range and puts dots under the cursor', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model([lane([current])], { selection: [2, 6], cursorT: 5 }))
    expect(calls.some((c) => c.fn === 'fillRect' && c.fillStyle === '#accentbg')).toBe(true)
    // The cursor's dot: a ring in the surface colour, then the dot in the trace colour.
    const fills = calls.filter((c) => c.fn === 'fill').map((c) => c.fillStyle)
    expect(fills.slice(-2)).toEqual(['#surface', '#s2'])
  })

  it('returns the geometry it drew with', () => {
    const m = model([lane([current])])
    const { ctx } = recorder()
    const drawn = drawPlot(ctx, m)
    const planned = geometryOf(m)
    const plain = (g: typeof drawn) => ({ x0: g.x0, x1: g.x1, axisY: g.axisY, total: g.total, boxes: g.lanes.map((l) => l.box), range: g.lanes.map((l) => [l.lo, l.hi]) })
    expect(plain(drawn)).toEqual(plain(planned))
  })
})

describe('the sample nearest the cursor', () => {
  it('skips failed readings', () => {
    expect(nearestFinite([0, 1, 2, 3], [1, null, null, 4], 1.4)).toBe(0)
    expect(nearestFinite([0, 1, 2, 3], [1, 2, 3, 4], 2.6)).toBe(3)
    expect(nearestFinite([], [], 1)).toBe(-1)
    expect(nearestFinite([0, 1], [null, null], 0.5)).toBe(-1)
  })
})

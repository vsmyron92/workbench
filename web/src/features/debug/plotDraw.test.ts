import { describe, expect, it } from 'vitest'
import { DIRECT_LABELS_MAX, drawPlot, edgeTime, layoutFor, layoutOf, snapTime, stepTime, timeAtX, type DrawSeries, type PlotModel, type PlotTheme } from './plotDraw'
import type { Track } from './plotMath'

const track = (pairs: [number, number][]): Track => ({ t: pairs.map((p) => p[0]), v: pairs.map((p) => p[1]) })
const series = (name: string, slot: number, track: Track | undefined, hidden = false): DrawSeries => ({ name, slot, track, hidden })

const a = track([[1000, 1], [1250, 2], [1500, 3], [1750, 4]])
const b = track([[1000, 10], [1500, 30], [1750, 40]])

describe('the plot\'s geometry', () => {
  it('leaves room for the names at the line ends only while there are few lines', () => {
    const few = layoutOf(800, 300, DIRECT_LABELS_MAX, 10_000, 5000)
    const many = layoutOf(800, 300, DIRECT_LABELS_MAX + 1, 10_000, 5000)
    expect(few.x1).toBeLessThan(many.x1)
    expect([few.t0, few.t1]).toEqual([5000, 10_000])
    expect(layoutOf(40, 30, 1, 10_000, 5000).x1).toBeGreaterThan(layoutOf(40, 30, 1, 10_000, 5000).x0) // never inverted
  })

  it('widens the left margin for wide axis labels, and the drawing uses the same geometry', () => {
    const input = (v1: number, v2: number) => ({ width: 800, height: 300, series: [series('a', 1, track([[1000, v1], [1750, v2]]))], windowMs: 5000, scale: 'shared' as const, t1: 1750 })
    const small = layoutFor(input(1, 5))
    const big = layoutFor(input(1_000_000, 4_294_967_295))
    expect(big.x0).toBeGreaterThan(small.x0)
    // 4,294,967,295 is 13 characters of the 11 px axis font (about 6.6 px each) and ends 8 px left of the axis.
    expect(big.x0).toBeGreaterThanOrEqual(8 + Math.ceil(13 * 6.6))
    expect(layoutFor({ ...input(1, 5), scale: 'normalized' }).x0).toBeLessThanOrEqual(small.x0 + 6) // "100%" is short
    const { ctx } = recorder()
    expect(drawPlot(ctx, model({ series: input(1_000_000, 4_294_967_295).series }))).toEqual(layoutFor({ ...input(1_000_000, 4_294_967_295), t1: 1750 }))
  })

  it('maps a pixel column to a time', () => {
    const g = layoutOf(800, 300, 5, 10_000, 5000)
    expect(timeAtX(g, g.x0)).toBe(5000)
    expect(timeAtX(g, g.x1)).toBe(10_000)
    expect(timeAtX(g, (g.x0 + g.x1) / 2)).toBe(7500)
  })
})

describe('the crosshair', () => {
  const all = [series('a', 1, a), series('b', 2, b)]

  it('snaps to the nearest reading of any shown series', () => {
    expect(snapTime(all, 1260)).toBe(1250) // only a has a reading there
    expect(snapTime(all, 1100)).toBe(1000)
    expect(snapTime(all, 99_999)).toBe(1750)
    expect(snapTime([series('a', 1, undefined)], 1000)).toBeNull()
  })

  it('does not snap to a hidden series', () => {
    expect(snapTime([series('a', 1, a, true), series('b', 2, b)], 1260)).toBe(1500) // b's nearest, not the hidden a's 1250
  })

  it('steps to the next and the previous reading', () => {
    expect(stepTime(all, 1000, 1)).toBe(1250)
    expect(stepTime(all, 1250, 1)).toBe(1500)
    expect(stepTime(all, 1750, 1)).toBeNull()
    expect(stepTime(all, 1750, -1)).toBe(1500)
    expect(stepTime(all, 1000, -1)).toBeNull()
    expect(stepTime(all, 1300, -1)).toBe(1250)
  })

  it('jumps to the ends of the window', () => {
    expect(edgeTime(all, { t0: 0, t1: 5000 }, 1)).toBe(1750)
    expect(edgeTime(all, { t0: 0, t1: 5000 }, -1)).toBe(1000)
    expect(edgeTime(all, { t0: 1200, t1: 1600 }, -1)).toBe(1250)
    expect(edgeTime(all, { t0: 1200, t1: 1600 }, 1)).toBe(1500)
    expect(edgeTime(all, { t0: 5000, t1: 6000 }, 1)).toBeNull()
  })
})

// ---------------------------------------------------------------- drawing against a recording context

const theme: PlotTheme = { surface: '#surface', grid: '#grid', axis: '#axis', text: '#text', muted: '#muted', ui: 'ui', mono: 'mono', series: ['#s1', '#s2', '#s3', '#s4', '#s5', '#s6', '#s7', '#s8'] }

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
  return { ctx, calls, state }
}

const model = (over: Partial<PlotModel> = {}): PlotModel => ({
  width: 800,
  height: 300,
  dpr: 2,
  series: [series('rpm', 1, a), series('temp', 3, b)],
  windowMs: 5000,
  scale: 'shared',
  t1: 1750,
  intervalMs: 250,
  cursorT: null,
  theme,
  ...over,
})

describe('drawing', () => {
  it('draws one 2 px line per series in its own colour, with an end dot each', () => {
    const { ctx, calls } = recorder()
    const g = drawPlot(ctx, model())
    const lines = calls.filter((c) => c.fn === 'stroke' && c.lineWidth === 2)
    expect(lines.map((c) => c.strokeStyle)).toEqual(['#s1', '#s3'])
    // The end dot: a ring in the surface colour, then the dot in the series colour.
    const fills = calls.filter((c) => c.fn === 'fill').map((c) => c.fillStyle)
    expect(fills).toEqual(['#surface', '#s1', '#surface', '#s3'])
    expect(g.x1).toBeGreaterThan(g.x0)
    expect(calls[0]).toMatchObject({ fn: 'setTransform', args: [2, 0, 0, 2, 0, 0] })
  })

  it('writes the series names at the line ends while there are few lines, in the text colour', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model())
    const names = calls.filter((c) => c.fn === 'fillText' && (c.args[0] === 'rpm' || c.args[0] === 'temp'))
    expect(names).toHaveLength(2)
    expect(names.every((c) => c.fillStyle === '#text')).toBe(true) // never the series' own colour
  })

  it('names none of the lines that end at the same place, and still names the one that ends apart', () => {
    const together = series('ticks', 2, track([[1000, 0], [1750, 40]]))
    const alsoTop = series('temp', 1, track([[1000, 40], [1750, 40]]))
    const apart = series('led', 3, track([[1000, 0], [1750, 0]]))
    const { ctx, calls } = recorder()
    drawPlot(ctx, model({ series: [alsoTop, together, apart] }))
    const named = calls.filter((c) => c.fn === 'fillText').map((c) => String(c.args[0]))
    expect(named).toContain('led')
    expect(named).not.toContain('ticks')
    expect(named).not.toContain('temp')
  })

  it('does not write names for many lines', () => {
    const many = Array.from({ length: 6 }, (_, i) => series(`s${i}`, i + 1, track([[1000, i], [1750, i + 1]])))
    const { ctx, calls } = recorder()
    drawPlot(ctx, model({ series: many }))
    expect(calls.filter((c) => c.fn === 'fillText' && /^s\d$/.test(String(c.args[0])))).toHaveLength(0)
    expect(calls.filter((c) => c.fn === 'stroke' && c.lineWidth === 2)).toHaveLength(6)
  })

  it('draws nothing for a hidden series, and survives a series without readings', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model({ series: [series('rpm', 1, a, true), series('temp', 3, undefined), series('adc', 2, track([]))] }))
    expect(calls.filter((c) => c.fn === 'stroke' && c.lineWidth === 2)).toHaveLength(0)
    expect(calls.filter((c) => c.fn === 'fill')).toHaveLength(0)
  })

  it('labels a normalized axis in percent and a shared one in the values\' own unit', () => {
    const norm = recorder()
    drawPlot(norm.ctx, model({ scale: 'normalized' }))
    const labels = norm.calls.filter((c) => c.fn === 'fillText').map((c) => String(c.args[0]))
    expect(labels).toEqual(expect.arrayContaining(['0%', '50%', '100%']))
    const shared = recorder()
    drawPlot(shared.ctx, model())
    expect(shared.calls.filter((c) => c.fn === 'fillText').map((c) => String(c.args[0]))).not.toContain('50%')
  })

  it('puts a ring-and-dot on each line at the crosshair', () => {
    const base = recorder()
    drawPlot(base.ctx, model())
    const hover = recorder()
    drawPlot(hover.ctx, model({ cursorT: 1500 }))
    const dots = (r: ReturnType<typeof recorder>) => r.calls.filter((c) => c.fn === 'fill').length
    expect(dots(hover) - dots(base)).toBe(4) // two series: a ring and a dot each
  })

  it('draws a lone reading as a dot', () => {
    const { ctx, calls } = recorder()
    drawPlot(ctx, model({ series: [series('rpm', 1, track([[1750, 5]]))] }))
    expect(calls.filter((c) => c.fn === 'arc' && c.args[2] === 2)).toHaveLength(1)
  })

  it('draws an empty plot without readings', () => {
    const { ctx, calls } = recorder()
    expect(() => drawPlot(ctx, model({ series: [] }))).not.toThrow()
    expect(calls.some((c) => c.fn === 'fillText')).toBe(true) // the axes
  })
})

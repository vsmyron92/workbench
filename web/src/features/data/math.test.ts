import { describe, expect, it } from 'vitest'
import {
  atOrBefore,
  axisFormat,
  extentOf,
  formatDuration,
  formatTimeTick,
  formatValue,
  layoutLanes,
  logTicks,
  lowerBound,
  minLaneHeight,
  nearestIndex,
  niceTicks,
  parseUnit,
  stepSpan,
  timeTicks,
  trimNumber,
  zoomRange,
} from './math'

describe('searching', () => {
  const a = [1, 2, 4, 8]
  it('finds bounds and neighbours', () => {
    expect(lowerBound(a, 4)).toBe(2)
    expect(lowerBound(a, 5)).toBe(3)
    expect(lowerBound(a, 99)).toBe(4)
    expect(atOrBefore(a, 4)).toBe(2)
    expect(atOrBefore(a, 3.9)).toBe(1)
    expect(atOrBefore(a, 0)).toBe(-1)
    expect(nearestIndex(a, 5)).toBe(2)
    expect(nearestIndex(a, 6.1)).toBe(3)
    expect(nearestIndex([], 1)).toBe(-1)
  })
})

describe('units', () => {
  it('knows which units take prefixes', () => {
    expect(parseUnit('mA')).toEqual({ base: 'A', scale: 1e-3, si: true })
    expect(parseUnit('uA')).toEqual({ base: 'A', scale: 1e-6, si: true })
    expect(parseUnit('µV').scale).toBe(1e-6)
    expect(parseUnit('V').scale).toBe(1)
    expect(parseUnit('°C').si).toBe(false)
    expect(parseUnit('%').si).toBe(false)
    expect(parseUnit('mm').si).toBe(false)
    expect(parseUnit('').si).toBe(false)
  })

  it('writes a value with the prefix that suits it', () => {
    expect(formatValue(0.0052, 'A')).toBe('5.2 mA')
    expect(formatValue(5200, 'uA')).toBe('5.2 mA')
    expect(formatValue(3.3, 'V')).toBe('3.3 V')
    expect(formatValue(0.000012, 'A')).toBe('12 µA')
    expect(formatValue(1500, 'mW')).toBe('1.5 W')
    expect(formatValue(0, 'mA')).toBe('0 A')
    expect(formatValue(21.456, '°C')).toBe('21.46 °C')
    expect(formatValue(7, '')).toBe('7')
    expect(formatValue(Number.NaN, 'A')).toBe('—')
    expect(formatValue(2.5e-9, 'A')).toBe('2.5 nA')
  })

  it('trims numbers without exponents in ordinary ranges', () => {
    expect(trimNumber(1234.5678)).toBe('1235')
    expect(trimNumber(0.00012346)).toBe('0.0001235')
    expect(trimNumber(12.5, 3)).toBe('12.5')
  })

  it('labels one lane in one prefix', () => {
    const f = axisFormat([0, 0.01, 0.02, 0.03], 0.01, 'A')
    expect(f.unit).toBe('mA')
    expect(f.label(0.02)).toBe('20')
    expect(axisFormat([0, 0.5, 1], 0.5, '°C').label(0.5)).toBe('0.5')
    expect(axisFormat([1e-6, 1e-3], 1, 'A', true).label(1e-6)).toBe('1 µA')
  })
})

describe('axes', () => {
  it('chooses round ticks', () => {
    const t = niceTicks(0, 97, 5)
    expect(t.step).toBe(20)
    expect(t.ticks).toEqual([0, 20, 40, 60, 80, 100])
    expect(niceTicks(5, 5).ticks.length).toBeGreaterThan(1)
    expect(niceTicks(0.0011, 0.0059, 4).step).toBeCloseTo(0.001)
  })

  it('draws decades on a log axis', () => {
    const t = logTicks(0.0000123, 0.052)
    expect(t.major[0]).toBeCloseTo(1e-5)
    expect(t.major[t.major.length - 1]).toBeCloseTo(0.1)
    expect(t.minor).toContain(2e-5)
    expect(logTicks(5, 5).major.length).toBe(2)
    expect(logTicks(-1, 0).major).toEqual([1, 10])
  })

  it('chooses time steps on the clock and below a second', () => {
    expect(timeTicks(0, 10, 5).step).toBe(2)
    expect(timeTicks(0, 100, 5).step).toBe(30)
    expect(timeTicks(0, 0.0012, 6).step).toBeCloseTo(0.0002)
    const t = timeTicks(1.5, 4.5, 3)
    expect(t.ticks[0]).toBeGreaterThanOrEqual(1.5)
    expect(t.ticks.at(-1)).toBeLessThanOrEqual(4.5)
  })

  it('labels times in the unit the step calls for', () => {
    expect(formatTimeTick(0.002, 0.001, false)).toBe('2 ms')
    expect(formatTimeTick(0.0025, 0.0005, false)).toBe('2.5 ms')
    expect(formatTimeTick(2e-6, 1e-6, false)).toBe('2 µs')
    expect(formatTimeTick(12, 2, false)).toBe('12 s')
    expect(formatTimeTick(65, 5, false)).toBe('1:05')
    expect(formatTimeTick(3725, 5, false)).toBe('1:02:05')
    expect(formatDuration(0.00125)).toBe('1.25 ms')
    expect(formatDuration(75)).toBe('1 min 15 s')
    expect(formatDuration(3.2)).toBe('3.2 s')
  })

  it('writes a clock to the resolution of the step', () => {
    const t = new Date(2026, 9, 15, 14, 3, 7, 250).getTime() / 1000
    expect(formatTimeTick(t, 1, true)).toBe('14:03:07')
    expect(formatTimeTick(t, 0.05, true)).toBe('14:03:07.25')
  })
})

describe('lanes', () => {
  const lane = (weight: number, min: number) => ({ weight, min })

  it('shares the height by weight and leaves the gaps', () => {
    const { boxes, total } = layoutLanes(406, [lane(3, 60), lane(1, 20), lane(1, 20)], 3)
    expect(total).toBe(406)
    expect(boxes[0].y0).toBe(0)
    expect(boxes[2].y1).toBeCloseTo(406)
    const h = boxes.map((b) => b.y1 - b.y0)
    expect(h[0] / h[1]).toBeCloseTo(3)
    expect(boxes[1].y0 - boxes[0].y1).toBe(3)
  })

  it('never squeezes a lane under its minimum', () => {
    const { boxes } = layoutLanes(200, [lane(1, 100), lane(10, 40), lane(10, 40)], 0)
    const h = boxes.map((b) => b.y1 - b.y0)
    expect(h[0]).toBeGreaterThanOrEqual(100)
    expect(h.reduce((a, b) => a + b, 0)).toBeCloseTo(200)
  })

  it('asks for scrolling when the minimums do not fit', () => {
    const lanes = [minLaneHeight('analog'), minLaneHeight('digital'), minLaneHeight('events')].map((m) => lane(1, m))
    const { boxes, total } = layoutLanes(50, lanes, 6)
    expect(total).toBe(64 + 30 + 26 + 12)
    expect(boxes[2].y1).toBe(total)
  })

  it('has no lanes, no boxes', () => {
    expect(layoutLanes(100, [])).toEqual({ boxes: [], total: 100 })
  })
})

describe('moving the view', () => {
  it('zooms around an anchor that stays where it is', () => {
    const [a, b] = zoomRange(0, 10, 2, 0.5)
    expect([a, b]).toEqual([1, 6])
    // The anchor's fraction of the view is unchanged.
    expect((2 - a) / (b - a)).toBeCloseTo(0.2)
  })

  it('steps through the spans', () => {
    expect(stepSpan(10, 1)).toBe(30)
    expect(stepSpan(10, -1)).toBe(5)
    expect(stepSpan(3600, 1)).toBe(3600)
    expect(stepSpan(0.001, -1)).toBe(0.001)
    expect(stepSpan(7, 1)).toBe(10)
  })

  it('finds the extent of values, skipping failed readings', () => {
    expect(extentOf([3, null, -1, 7, Number.NaN])).toEqual([-1, 7])
    expect(extentOf([null, null])).toBeNull()
    expect(extentOf([1, 2, 3, 4], 1, 3)).toEqual([2, 3])
  })
})

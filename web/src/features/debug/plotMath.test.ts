import { describe, expect, it } from 'vitest'
import {
  decimateRuns,
  formatAgo,
  formatAgoExact,
  formatTick,
  freeSlot,
  lastReading,
  lowerBound,
  nearestIndex,
  nextWindow,
  niceTicks,
  panTo,
  plotCsv,
  plotGapMs,
  plotRows,
  readingAt,
  scrollThumb,
  sparklinePoints,
  spanExtent,
  timeTicks,
  visibleRange,
  windowLabel,
  zoomEnd,
  type Track,
} from './plotMath'

const track = (pairs: [number, number][]): Track => ({ t: pairs.map((p) => p[0]), v: pairs.map((p) => p[1]) })
/** A reading every `dt` ms from 0, `n` of them, valued by `f`. */
const ramp = (n: number, dt: number, f: (i: number) => number): Track => track(Array.from({ length: n }, (_, i) => [i * dt, f(i)] as [number, number]))

describe('searching a track', () => {
  const t = [10, 20, 30, 40]
  it('finds the first reading at or after a time', () => {
    expect(lowerBound(t, 5)).toBe(0)
    expect(lowerBound(t, 20)).toBe(1)
    expect(lowerBound(t, 21)).toBe(2)
    expect(lowerBound(t, 99)).toBe(4)
    expect(lowerBound([], 1)).toBe(0)
  })

  it('takes one reading before the span, so a line enters from the left edge', () => {
    expect(visibleRange(t, 25, 35)).toEqual([1, 3]) // 20 (before), 30; 40 is after
    expect(visibleRange(t, 0, 100)).toEqual([0, 4])
    expect(visibleRange(t, 50, 60)).toEqual([3, 4]) // only the last reading before the span
    expect(visibleRange([], 0, 1)).toEqual([0, 0])
  })

  it('finds the nearest reading, the earlier one on a tie', () => {
    expect(nearestIndex(t, 14)).toBe(0)
    expect(nearestIndex(t, 15)).toBe(0)
    expect(nearestIndex(t, 16)).toBe(1)
    expect(nearestIndex(t, -50)).toBe(0)
    expect(nearestIndex(t, 500)).toBe(3)
    expect(nearestIndex([], 1)).toBe(-1)
  })

  it('answers a reading only when it is a number near enough', () => {
    const tr = track([[0, 1], [100, Number.NaN], [200, 3]])
    expect(readingAt(tr, 10, 50)).toEqual({ index: 0, t: 0, v: 1 })
    expect(readingAt(tr, 100, 50)).toBeNull() // a failed reading
    expect(readingAt(tr, 150, 20)).toBeNull() // too far from both
    expect(readingAt(undefined, 0, 50)).toBeNull()
    expect(lastReading(track([[0, 1], [5, 2], [9, Number.NaN]]))).toEqual({ index: 1, t: 5, v: 2 })
    expect(lastReading(track([[0, Number.NaN]]))).toBeNull()
  })

  it('measures the extent of what is in the span', () => {
    const tr = track([[0, 5], [10, -2], [20, Number.NaN], [30, 9], [40, 100]])
    expect(spanExtent(tr, 0, 30)).toEqual([-2, 9])
    expect(spanExtent(tr, 35, 50)).toEqual([9, 100]) // the reading before the span counts
    expect(spanExtent(track([[0, Number.NaN]]), 0, 10)).toBeNull()
  })
})

describe('slots and windows', () => {
  it('gives a series the first colour nobody has', () => {
    expect(freeSlot([])).toBe(1)
    expect(freeSlot([1, 2, 4])).toBe(3)
    expect(freeSlot([8, 1])).toBe(2)
    expect(freeSlot([1, 2, 3, 4, 5, 6, 7, 8])).toBeNull()
  })

  it('labels spans and sets the gap that breaks a line', () => {
    expect(windowLabel(5000)).toBe('5 s')
    expect(windowLabel(60_000)).toBe('1 min')
    expect(windowLabel(300_000)).toBe('5 min')
    expect(plotGapMs(250)).toBe(1500)
    expect(plotGapMs(1000)).toBe(6000)
  })
})

describe('scrolling', () => {
  it('steps the span through the ones offered and stops at the ends', () => {
    expect(nextWindow(30_000, 1)).toBe(60_000)
    expect(nextWindow(30_000, -1)).toBe(10_000)
    expect(nextWindow(5_000, -1)).toBe(5_000)
    expect(nextWindow(1_800_000, 1)).toBe(1_800_000)
    expect(nextWindow(45_000, 1)).toBe(300_000) // not one of them: from the next wider
    expect(windowLabel(900_000)).toBe('15 min')
    expect(windowLabel(1_800_000)).toBe('30 min')
  })

  it('moves the view end within the readings kept, and follows the newest when it reaches it', () => {
    // 100 s of readings, a 30 s span: the view can end anywhere from 30 s after the oldest to the newest.
    const oldest = 1_000_000
    const newest = 1_100_000
    expect(panTo(1_060_000, oldest, newest, 30_000)).toBe(1_060_000)
    expect(panTo(1_010_000, oldest, newest, 30_000)).toBe(1_030_000) // not before the oldest reading
    expect(panTo(1_099_999, oldest, newest, 30_000)).toBeNull() // at the newest: follow
    expect(panTo(1_200_000, oldest, newest, 30_000)).toBeNull() // beyond it
    // Everything kept fits in the span: nothing to scroll.
    expect(panTo(1_050_000, oldest, newest, 300_000)).toBeNull()
    expect(panTo(1, null, null, 30_000)).toBeNull()
    expect(panTo(5, null, 100, 30_000)).toBeNull()
  })

  it('places the scroll bar thumb among the readings, never too small to grab', () => {
    expect(scrollThumb(1000, 101_000, 71_000, 101_000)).toEqual({ start: 0.7, size: 0.3 })
    expect(scrollThumb(1000, 101_000, 1000, 31_000)).toEqual({ start: 0, size: 0.3 })
    expect(scrollThumb(1000, 101_000, -50_000, 150_000)).toEqual({ start: 0, size: 1 }) // the span covers everything
    const tiny = scrollThumb(0, 3_600_000, 3_595_000, 3_600_000)
    expect(tiny.size).toBe(0.04)
    expect(tiny.start).toBeCloseTo(0.96, 5)
    expect(scrollThumb(null, null, 0, 10)).toEqual({ start: 0, size: 1 })
    expect(scrollThumb(5, 5, 0, 10)).toEqual({ start: 0, size: 1 })
  })

  it('keeps the time under the pointer where it is when zooming', () => {
    // A 30 s view ending at 1000 s; the pointer is on 990 s. Zoom to 10 s: the point stays 10 s before the right edge, scaled.
    const end = zoomEnd(990_000, 1_000_000, 30_000, 10_000)
    expect(end).toBeCloseTo(990_000 + 10_000 / 3, 6)
    // The same fraction of the view lies left of the point before and after.
    const frac = (990_000 - (1_000_000 - 30_000)) / 30_000
    expect((990_000 - (end - 10_000)) / 10_000).toBeCloseTo(frac, 6)
    expect(zoomEnd(5, 5, 30_000, 60_000)).toBe(5) // the anchor on the right edge: the edge stays
  })
})

describe('axes', () => {
  it('picks round ticks and widens the range to whole steps', () => {
    const k = niceTicks(0, 97)
    expect(k.ticks).toEqual([0, 20, 40, 60, 80, 100])
    expect([k.lo, k.hi, k.step, k.decimals]).toEqual([0, 100, 20, 0])
    const f = niceTicks(0.12, 0.93)
    expect(f.ticks).toEqual([0, 0.2, 0.4, 0.6, 0.8, 1])
    expect(f.lo).toBeLessThanOrEqual(0.12)
    expect(f.hi).toBeGreaterThanOrEqual(0.93)
    expect(f.decimals).toBe(1)
    const big = niceTicks(1_000_000, 4_300_000_000)
    expect(big.step).toBe(1_000_000_000)
    expect(big.decimals).toBe(0)
  })

  it('has no floating-point dust in the ticks', () => {
    for (const t of niceTicks(0, 1, 10).ticks) expect(String(t).length).toBeLessThan(6)
    expect(niceTicks(-0.3, 0.3).ticks).toContain(0)
  })

  it('keeps ticks apart for very small and very large ranges', () => {
    for (const [lo, hi] of [[0, 1e-13], [1e-14, 3e-14], [0, 1e-9], [-5e-7, 5e-7], [0, 3e15], [1e12, 1e12 + 5]]) {
      const k = niceTicks(lo, hi)
      expect(k.hi).toBeGreaterThan(k.lo)
      expect(new Set(k.ticks).size).toBe(k.ticks.length)
      expect(k.ticks.every((t, i) => i === 0 || t > k.ticks[i - 1])).toBe(true)
      expect(k.lo).toBeLessThanOrEqual(lo)
      expect(k.hi).toBeGreaterThanOrEqual(hi)
    }
  })

  it('widens a flat range and survives nonsense', () => {
    const flat = niceTicks(7, 7)
    expect(flat.lo).toBeLessThan(7)
    expect(flat.hi).toBeGreaterThan(7)
    const zero = niceTicks(0, 0)
    expect(zero.lo).toBeLessThan(0)
    expect(zero.hi).toBeGreaterThan(0)
    expect(niceTicks(Number.NaN, 5).ticks.length).toBeGreaterThan(1)
    expect(niceTicks(10, 0).ticks[0]).toBe(0) // swapped
  })

  it('writes labels with grouped thousands and the digits the step needs', () => {
    expect(formatTick(12345, 0)).toBe('12,345')
    expect(formatTick(0.5, 1)).toBe('0.5')
    expect(formatTick(2, 2)).toBe('2.00')
    expect(formatTick(-1500, 0)).toBe('-1,500')
  })

  it('labels the time axis from now backwards', () => {
    expect(timeTicks(30_000)).toEqual([0, 5000, 10_000, 15_000, 20_000, 25_000, 30_000])
    expect(timeTicks(5000)).toEqual([0, 1000, 2000, 3000, 4000, 5000])
    expect(timeTicks(300_000)).toEqual([0, 60_000, 120_000, 180_000, 240_000, 300_000])
    expect(formatAgo(0)).toBe('now')
    expect(formatAgo(5000)).toBe('−5 s')
    expect(formatAgo(500)).toBe('−0.5 s')
    expect(formatAgo(60_000)).toBe('−1 min')
    expect(formatAgo(90_000)).toBe('−1:30')
    expect(formatAgoExact(0)).toBe('now')
    expect(formatAgoExact(250)).toBe('−0.25 s')
    expect(formatAgoExact(12_500)).toBe('−12.50 s')
    expect(formatAgoExact(65_250)).toBe('−1:05.25')
    expect(formatAgoExact(125_000)).toBe('−2:05.00')
  })

  it('never writes a part that should have carried into the next', () => {
    expect(formatAgo(119_600)).toBe('−2 min')
    expect(formatAgo(59_960)).toBe('−1 min')
    expect(formatAgo(59_900)).toBe('−59.9 s')
    expect(formatAgoExact(69_997)).toBe('−1:10.00')
    expect(formatAgoExact(119_998)).toBe('−2:00.00')
    expect(formatAgoExact(59_996)).toBe('−1:00.00')
    for (let ms = 0; ms < 400_000; ms += 137) {
      expect(formatAgo(ms)).not.toMatch(/:60|\b60 s/)
      expect(formatAgoExact(ms)).not.toMatch(/:60|:\d{3}|\b60\.00/)
    }
  })
})

describe('reducing readings for the screen', () => {
  it('keeps every reading when there are fewer than columns', () => {
    const runs = decimateRuns(ramp(5, 100, (i) => i), 0, 400, 100, 1500)
    expect(runs).toEqual([[0, 0, 100, 1, 200, 2, 300, 3, 400, 4]])
  })

  it('keeps the first, last and extremes of a crowded column, so a spike survives', () => {
    // 1000 readings in a 1000 ms span drawn 10 columns wide: 100 per column; one spike and one dip.
    const tr = ramp(1000, 1, (i) => (i === 555 ? 1000 : i === 321 ? -1000 : 0))
    const runs = decimateRuns(tr, 0, 1000, 10, 1500)
    expect(runs).toHaveLength(1)
    const values = runs[0].filter((_, i) => i % 2 === 1)
    expect(values).toContain(1000)
    expect(values).toContain(-1000)
    expect(runs[0].length).toBeLessThanOrEqual(10 * 4 * 2)
    // Times only go forward.
    const times = runs[0].filter((_, i) => i % 2 === 0)
    expect([...times].sort((a, b) => a - b)).toEqual(times)
  })

  it('breaks the line at a failed reading and at a long gap', () => {
    const failed = track([[0, 1], [100, 2], [200, Number.NaN], [300, 4], [400, 5]])
    expect(decimateRuns(failed, 0, 400, 400, 1500)).toEqual([[0, 1, 100, 2], [300, 4, 400, 5]])
    const gap = track([[0, 1], [100, 2], [5000, 3], [5100, 4]])
    expect(decimateRuns(gap, 0, 5100, 5100, 1500)).toEqual([[0, 1, 100, 2], [5000, 3, 5100, 4]])
  })

  it('starts the line at the reading before the span and ignores what lies after it', () => {
    const tr = track([[0, 1], [100, 2], [200, 3], [300, 4]])
    const runs = decimateRuns(tr, 150, 250, 100, 1500)
    expect(runs).toEqual([[100, 2, 200, 3]]) // 100 enters from the left; 300 is past the right edge
  })

  it('has nothing to draw without data or width', () => {
    expect(decimateRuns(undefined, 0, 10, 10, 100)).toEqual([])
    expect(decimateRuns(ramp(3, 1, () => 1), 0, 10, 0, 100)).toEqual([])
    expect(decimateRuns(ramp(3, 1, () => 1), 10, 10, 10, 100)).toEqual([])
  })
})

/** The straightforward version of `decimateRuns` (a Set and a sort per column), kept to pin the fast one to it. */
function referenceRuns(tr: Track, t0: number, t1: number, cols: number, gapMs: number): number[][] {
  const [from, to] = visibleRange(tr.t, t0, t1)
  const runs: number[][] = []
  let run: number[] = []
  let col = Number.NaN
  let ids: number[] = []
  const flush = () => {
    for (const i of [...new Set(ids)].sort((a, b) => a - b)) run.push(tr.t[i], tr.v[i])
    ids = []
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
      ids = [i, i, i, i]
    } else {
      if (v < tr.v[ids[1]]) ids[1] = i
      if (v > tr.v[ids[2]]) ids[2] = i
      ids[3] = i
    }
  }
  end()
  return runs
}

describe('reducing readings: the fast version against the plain one', () => {
  // A small deterministic generator, so a failure can be reproduced.
  const rng = (seed: number) => () => {
    seed = (seed * 1664525 + 1013904223) >>> 0
    return seed / 2 ** 32
  }

  it('gives the same points for random tracks with ties, failed readings and holes', () => {
    for (let seed = 1; seed <= 400; seed++) {
      const r = rng(seed)
      const n = Math.floor(r() * 400)
      const pairs: [number, number][] = []
      let time = Math.floor(r() * 1000)
      for (let i = 0; i < n; i++) {
        time += 1 + Math.floor(r() * (r() < 0.05 ? 3000 : 40)) // now and then a long hole
        const roll = r()
        pairs.push([time, roll < 0.05 ? Number.NaN : Math.floor(r() * 7) - 3]) // few distinct values: many ties
      }
      const tr = track(pairs)
      const last = time || 1
      const t1 = last - Math.floor(r() * 50)
      const t0 = t1 - 1 - Math.floor(r() * (last + 100))
      const cols = 1 + Math.floor(r() * 120)
      const gap = r() < 0.5 ? 1500 : 200
      expect(decimateRuns(tr, t0, t1, cols, gap), `seed ${seed}`).toEqual(referenceRuns(tr, t0, t1, cols, gap))
    }
  })
})

describe('whole numbers a double cannot hold', () => {
  const big = '18446744073709551615'
  const tr: Track = { t: [1000, 2000, 3000], v: [5, 18446744073709552000, 7], raw: [undefined, big, undefined] }

  it('hands back the exact digits with the reading', () => {
    expect(readingAt(tr, 2000, 100)).toEqual({ index: 1, t: 2000, v: 18446744073709552000, raw: big })
    expect(readingAt(tr, 1000, 100)).toEqual({ index: 0, t: 1000, v: 5 }) // no raw key for an ordinary reading
    expect(lastReading({ t: [1, 2], v: [1, 2], raw: [undefined, big] })).toEqual({ index: 1, t: 2, v: 2, raw: big })
    expect(lastReading({ t: [1], v: [1] })).toEqual({ index: 0, t: 1, v: 1 })
  })

  it('writes them into the table and the CSV instead of the rounded double', () => {
    const rows = plotRows([tr, track([[1000, 1], [2000, 2], [3000, 3]])], 0, 5000, 10)
    expect(rows[1]).toEqual({ t: 2000, v: [18446744073709552000, 2], raw: [big, undefined] })
    expect(rows[0].raw).toBeUndefined() // nothing exact to tell at 3000
    const csv = plotCsv(['x', 'y'], rows, 3000).split('\n')
    expect(csv[2]).toBe(`1.000,2000,${big},2`)
    expect(csv[1]).toBe('0.000,3000,7,3')
  })
})

describe('the sparkline over time', () => {
  const ramp = track(Array.from({ length: 11 }, (_, i) => [i * 1000, i] as [number, number])) // 0 … 10 over 10 s

  it('runs from the left edge of its span to the newest reading, low at the bottom and high on top', () => {
    const runs = sparklinePoints(ramp, 10_000, 120, 22, 1500)
    expect(runs).toHaveLength(1)
    const pts = runs[0].split(' ').map((p) => p.split(',').map(Number))
    expect(pts).toHaveLength(11)
    expect(pts[0]).toEqual([2, 20]) // the oldest and lowest: bottom left (pad 2)
    expect(pts[10][0]).toBeCloseTo(118, 0) // the newest at the right edge
    expect(pts[10][1]).toBe(2) // and the highest on top
    expect(pts.every((p, i) => i === 0 || p[0] > pts[i - 1][0])).toBe(true)
  })

  it('shows only the newest part when the span is shorter than the readings', () => {
    const pts = sparklinePoints(ramp, 4_000, 120, 22, 1500)[0].split(' ').map((p) => p.split(',').map(Number))
    expect(pts.length).toBeLessThanOrEqual(6) // 6 s … 10 s, and the one before it
    expect(pts[pts.length - 1][1]).toBe(2)
  })

  it('draws a constant value across the middle, splits at a failed reading, and a lone reading is a dot', () => {
    expect(sparklinePoints(track([[0, 5], [1000, 5], [2000, 5]]), 2000, 120, 22, 1500)[0].split(' ').every((p) => p.endsWith(',11.0'))).toBe(true)
    const split = sparklinePoints(track([[0, 1], [1000, 2], [2000, Number.NaN], [3000, 4], [4000, 5]]), 4000, 120, 22, 1500)
    expect(split).toHaveLength(2)
    expect(sparklinePoints(track([[500, 3]]), 1000, 120, 22, 1500)[0]).toMatch(/^[\d.]+,[\d.]+$/)
    expect(sparklinePoints(undefined, 1000, 120, 22, 1500)).toEqual([])
    expect(sparklinePoints(track([[0, Number.NaN]]), 1000, 120, 22, 1500)).toEqual([])
  })
})

describe('the table and its CSV', () => {
  const a = track([[1000, 1], [1250, 2], [1500, 3]])
  const b = track([[1250, 20], [1500, Number.NaN], [1750, 40]])

  it('lines series up on equal times, newest first, with holes where a series has no reading', () => {
    expect(plotRows([a, b], 0, 2000, 10)).toEqual([
      { t: 1750, v: [undefined, 40] },
      { t: 1500, v: [3, undefined] }, // b's reading failed
      { t: 1250, v: [2, 20] },
      { t: 1000, v: [1, undefined] },
    ])
  })

  it('stays within the span and the row limit', () => {
    expect(plotRows([a, b], 1200, 1600, 10).map((r) => r.t)).toEqual([1500, 1250])
    expect(plotRows([a, b], 0, 2000, 2).map((r) => r.t)).toEqual([1750, 1500])
    expect(plotRows([undefined, undefined], 0, 2000, 5)).toEqual([])
    expect(plotRows([a, undefined], 0, 2000, 5).map((r) => r.v)).toEqual([[3, undefined], [2, undefined], [1, undefined]])
  })

  it('writes a name that a spreadsheet would run as a formula as text, and leaves negative readings alone', () => {
    const csv = plotCsv(['=SUM(A1)', '+x', '-y', '@z', 'ok-name', '=1,2'], [{ t: 1000, v: [-5, 1, 2, 3, 4, -0.5] }], 1000)
    expect(csv.split('\n')[0]).toBe("seconds_ago,epoch_ms,'=SUM(A1),'+x,'-y,'@z,ok-name,\"'=1,2\"")
    expect(csv.split('\n')[1]).toBe('0.000,1000,-5,1,2,3,4,-0.5')
  })

  it('writes CSV with the seconds before the newest reading, and quotes names that need it', () => {
    const csv = plotCsv(['ticks', 'cfg,limit', 'say "hi"'], [{ t: 2000, v: [1, undefined, 2.5] }, { t: 1750, v: [0, 3, 2] }], 2000)
    expect(csv).toBe('seconds_ago,epoch_ms,ticks,"cfg,limit","say ""hi"""\n0.000,2000,1,,2.5\n0.250,1750,0,3,2\n')
  })
})

import { beforeEach, describe, expect, it } from 'vitest'
import { forgetReadings, ingest, PLOT_MAX_READINGS, resetReadings, seed, seedHistory, trackOf } from './plotBuffer'
import type { LiveHistory, LiveItem, LiveSample } from './types'

const item = (id: number): LiveItem => ({ id, expression: `v${id}`, address: 0x20000000 + id * 4, size: 4, kind: 'uint', typeName: 'uint32_t' })
const r = (id: number, t: number, v: LiveSample['v'], e?: string): LiveSample => ({ id, t, v, e })

beforeEach(() => resetReadings())

describe('the plot viewer\'s readings', () => {
  it('appends readings by session and item', () => {
    ingest({ sessionId: 's', samples: [r(1, 100, 5), r(2, 100, 7)] })
    ingest({ sessionId: 's', samples: [r(1, 350, 6), r(2, 350, 8)] })
    ingest({ sessionId: 'other', samples: [r(1, 100, 99)] })
    expect(trackOf('s', 1)).toEqual({ t: [100, 350], v: [5, 6] })
    expect(trackOf('s', 2)).toEqual({ t: [100, 350], v: [7, 8] })
    expect(trackOf('other', 1)?.v).toEqual([99])
    expect(trackOf('s', 3)).toBeUndefined()
    expect(trackOf(undefined, 1)).toBeUndefined()
    expect(trackOf('s', undefined)).toBeUndefined()
  })

  it('turns what is no number into a gap, and reads numbers sent as text', () => {
    ingest({ sessionId: 's', samples: [r(1, 1, 3), r(1, 2, undefined, 'read failed'), r(1, 3, 'abc'), r(1, 4, '12345678901234'), r(1, 5, true), r(1, 6, 2.5)] })
    const v = trackOf('s', 1)!.v
    expect(v.slice(0, 1)).toEqual([3])
    expect(Number.isNaN(v[1])).toBe(true)
    expect(Number.isNaN(v[2])).toBe(true) // bytes are text, and cannot be drawn
    expect(v.slice(3)).toEqual([12345678901234, 1, 2.5])
  })

  it('keeps one reading per time, and starts again when the clock steps back', () => {
    ingest({ sessionId: 's', samples: [r(1, 100, 1), r(1, 100, 2)] })
    expect(trackOf('s', 1)).toEqual({ t: [100], v: [2] })
    ingest({ sessionId: 's', samples: [r(1, 200, 3), r(1, 50, 9)] })
    expect(trackOf('s', 1)).toEqual({ t: [50], v: [9] })
  })

  it('caps the history, cutting the oldest', () => {
    const batch = 500
    for (let i = 0; i < (PLOT_MAX_READINGS + 3000) / batch; i++) {
      ingest({ sessionId: 's', samples: Array.from({ length: batch }, (_, k) => r(1, (i * batch + k) * 10, i * batch + k)) })
    }
    const tr = trackOf('s', 1)!
    const total = PLOT_MAX_READINGS + 3000
    expect(tr.t.length).toBeLessThanOrEqual(PLOT_MAX_READINGS + 1500)
    expect(tr.t.length).toBeGreaterThanOrEqual(PLOT_MAX_READINGS)
    expect(tr.v[tr.v.length - 1]).toBe(total - 1)
    expect(tr.t.length).toBe(tr.v.length)
    expect(tr.t.every((t, i) => i === 0 || t > tr.t[i - 1])).toBe(true)
  })

  it('drops the readings of an item taken off the watch list, and ignores one still on its way', () => {
    ingest({ sessionId: 's', items: [item(1), item(2)], samples: [r(1, 1, 1), r(2, 1, 2)] })
    ingest({ sessionId: 's', items: [item(1)] })
    expect(trackOf('s', 2)).toBeUndefined()
    ingest({ sessionId: 's', samples: [r(1, 2, 3), r(2, 2, 4)] })
    expect(trackOf('s', 2)).toBeUndefined()
    expect(trackOf('s', 1)?.v).toEqual([1, 3])
  })

  it('starts an item from the snapshot\'s last reading and does not overwrite history with it', () => {
    seed('s', { items: [item(1), item(2)], intervalMs: 250, last: { '1': r(1, 100, 5), '2': r(2, 100, undefined, 'no') } })
    expect(trackOf('s', 1)).toEqual({ t: [100], v: [5] })
    expect(Number.isNaN(trackOf('s', 2)!.v[0])).toBe(true)
    ingest({ sessionId: 's', samples: [r(1, 350, 6)] })
    seed('s', { items: [item(1)], intervalMs: 250, last: { '1': r(1, 350, 6) } })
    expect(trackOf('s', 1)).toEqual({ t: [100, 350], v: [5, 6] })
    expect(trackOf('s', 2)).toBeUndefined()
  })

  it('forgets a session, and a late event or snapshot of it does not bring it back', () => {
    ingest({ sessionId: 's', samples: [r(1, 1, 1)] })
    forgetReadings('s')
    expect(trackOf('s', 1)).toBeUndefined()
    ingest({ sessionId: 's', samples: [r(1, 2, 2)] })
    seed('s', { items: [item(1)], intervalMs: 250, last: { '1': r(1, 3, 3) } })
    expect(trackOf('s', 1)).toBeUndefined()
    ingest({ sessionId: 'other', samples: [r(1, 1, 1)] })
    expect(trackOf('other', 1)).toBeDefined()
  })

  it('keeps the digits of a whole number a double cannot hold, beside its nearest double', () => {
    ingest({ sessionId: 's', samples: [r(1, 1, 5), r(1, 2, '18446744073709551615'), r(1, 3, '12345'), r(1, 4, 7)] })
    const tr = trackOf('s', 1)!
    expect(tr.v).toEqual([5, 18446744073709552000, 12345, 7])
    expect(tr.raw).toEqual([undefined, '18446744073709551615', undefined, undefined]) // 12345 is exact as a number
    // A track that never sees one allocates nothing.
    ingest({ sessionId: 's', samples: [r(2, 1, 1), r(2, 2, 2)] })
    expect(trackOf('s', 2)!.raw).toBeUndefined()
  })

  it('keeps the digits in step through a repeated time, a clock that stepped back and the cap', () => {
    ingest({ sessionId: 's', samples: [r(1, 1, '9007199254740993'), r(1, 1, 3)] }) // the same time again, now an ordinary number
    expect(trackOf('s', 1)).toEqual({ t: [1], v: [3], raw: [undefined] })
    ingest({ sessionId: 's', samples: [r(1, 5, '9007199254740993'), r(1, 2, 8)] }) // the clock stepped back
    expect(trackOf('s', 1)).toEqual({ t: [2], v: [8], raw: undefined })
    for (let i = 0; i < PLOT_MAX_READINGS + 2000; i++) ingest({ sessionId: 's', samples: [r(3, 10 + i, i % 50 === 0 ? '9007199254740993' : i)] })
    const tr = trackOf('s', 3)!
    expect(tr.raw).toHaveLength(tr.t.length)
    tr.t.forEach((t, k) => expect(tr.raw![k], `reading at ${t}`).toBe((t - 10) % 50 === 0 ? '9007199254740993' : undefined))
  })

  it('puts the server\'s history in front of what was collected here, without doubling or losing', () => {
    const hist = (series: LiveHistory['series']): LiveHistory => ({ intervalMs: 250, items: [], series, now: 0 })
    seed('s', { items: [item(1), item(2)], intervalMs: 250, last: {} })
    ingest({ sessionId: 's', samples: [r(1, 300, 3), r(1, 400, 4)] })
    seedHistory('s', hist({
      '1': { t: [100, 200, 300, 400, 500], v: [1, null, 3, 4, 5], exact: { '1': '9007199254740993' } },
      '2': { t: [100, 200], v: [10, 20] },
      '9': { t: [100], v: [1] }, // not watched
    }))
    // Item 1: the server's readings before 300 (100, 200), then ours; its later ones are ours to receive.
    const one = trackOf('s', 1)!
    expect(one.t).toEqual([100, 200, 300, 400])
    expect(one.v.map((x) => (Number.isNaN(x) ? 'gap' : x))).toEqual([1, 'gap', 3, 4])
    expect(one.raw).toEqual([undefined, '9007199254740993', undefined, undefined])
    expect(trackOf('s', 2)).toEqual({ t: [100, 200], v: [10, 20] })
    expect(trackOf('s', 9)).toBeUndefined()
    // Seeding again changes nothing.
    seedHistory('s', hist({ '1': { t: [100, 200, 300, 400], v: [1, null, 3, 4] } }))
    expect(trackOf('s', 1)!.t).toEqual([100, 200, 300, 400])
    // A history that is not older than ours, or is empty, is left out; one that does not add up too.
    seedHistory('s', hist({ '1': { t: [], v: [] }, '2': { t: [500], v: [5] } }))
    expect(trackOf('s', 2)!.t).toEqual([100, 200]) // 500 is newer than anything held: events bring those
    seedHistory('s', hist({ '1': { t: [1, 2], v: [1] } }))
    expect(trackOf('s', 1)!.t).toEqual([100, 200, 300, 400])
    // A session that was forgotten stays so.
    forgetReadings('gone')
    seedHistory('gone', hist({ '1': { t: [1], v: [1] } }))
    expect(trackOf('gone', 1)).toBeUndefined()
  })

  it('caps the history it was given', () => {
    const t = Array.from({ length: PLOT_MAX_READINGS + 500 }, (_, i) => i)
    seedHistory('s', { intervalMs: 250, items: [], series: { '1': { t, v: t } }, now: 0 })
    const tr = trackOf('s', 1)!
    expect(tr.t).toHaveLength(PLOT_MAX_READINGS)
    expect(tr.t[tr.t.length - 1]).toBe(PLOT_MAX_READINGS + 499)
  })

  it('ignores a reading without a usable time', () => {
    ingest({ sessionId: 's', samples: [r(1, Number.NaN, 1), r(1, 5, 2), r(1, Number.POSITIVE_INFINITY, 3)] })
    expect(trackOf('s', 1)).toEqual({ t: [5], v: [2] })
  })
})

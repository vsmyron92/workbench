import { describe, expect, it } from 'vitest'
import { applyLive, emptyLive, loadSnapshot, useLive } from './liveStore'
import { LIVE_HISTORY } from './logic'
import type { LiveItem, LiveSample, LiveSnapshot } from './types'

const item = (id: number, expression = `v${id}`): LiveItem => ({ id, expression, address: 0x20000000 + id * 4, size: 4, kind: 'uint', typeName: 'uint32_t' })
const sample = (id: number, v: number, t = v): LiveSample => ({ id, t, v })
const snap = (items: LiveItem[], last: Record<string, LiveSample> = {}): LiveSnapshot => ({ items, intervalMs: 250, last })

describe('the Live tab\'s data', () => {
  it('starts an item from its last reading and keeps the history of the ones that stay', () => {
    let s = loadSnapshot(undefined, snap([item(1), item(2)], { '1': sample(1, 10), '2': sample(2, 20) }))
    expect(s.loaded).toBe(true)
    expect(s.samples[1]).toEqual([sample(1, 10)])
    s = applyLive(s, { sessionId: 'd', samples: [sample(1, 11), sample(2, 21)] })
    // A new snapshot without item 2: its history goes, item 1's stays.
    s = loadSnapshot(s, snap([item(1)], { '1': sample(1, 12) }))
    expect(Object.keys(s.samples)).toEqual(['1'])
    expect(s.samples[1]).toEqual([sample(1, 10), sample(1, 11)])
  })

  it('adds readings, caps the history and drops what is no longer watched', () => {
    let s = loadSnapshot(undefined, snap([item(1)]))
    for (let i = 0; i < LIVE_HISTORY + 20; i++) s = applyLive(s, { sessionId: 'd', samples: [sample(1, i)] })
    expect(s.samples[1]).toHaveLength(LIVE_HISTORY)
    expect(s.samples[1][LIVE_HISTORY - 1].v).toBe(LIVE_HISTORY + 19)
    // A reading of an item that was removed meanwhile is dropped once the list is known.
    s = applyLive(s, { sessionId: 'd', samples: [sample(9, 1)] })
    expect(s.samples[9]).toBeUndefined()
    s = applyLive(s, { sessionId: 'd', items: [] })
    expect(s.items).toEqual([])
    expect(s.samples).toEqual({})
  })

  it('keeps readings that arrive before the list does, and follows the interval', () => {
    let s = applyLive(undefined, { sessionId: 'd', samples: [sample(1, 5)] })
    expect(s.loaded).toBe(false)
    expect(s.samples[1]).toEqual([sample(1, 5)])
    s = applyLive(s, { sessionId: 'd', intervalMs: 1000 })
    expect(s.intervalMs).toBe(1000)
    s = loadSnapshot(s, snap([item(1)]))
    expect(s.samples[1]).toEqual([sample(1, 5)])
    expect(emptyLive().loaded).toBe(false)
  })

  it('is kept per session and forgotten with it', () => {
    const { apply, forget } = useLive.getState()
    apply({ sessionId: 'a', items: [item(1)] })
    apply({ sessionId: 'b', items: [item(2)] })
    expect(Object.keys(useLive.getState().sessions).sort()).toEqual(['a', 'b'])
    forget('a')
    forget('nope')
    expect(Object.keys(useLive.getState().sessions)).toEqual(['b'])
    forget('b')
  })
})

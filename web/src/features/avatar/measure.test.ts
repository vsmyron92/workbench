// Where the overlay takes the pointer to be: a mouse rests where it was last seen, a touch
// or a pen is somewhere only while it is pressed, so the place of the last tap never counts
// as a pointer that rests on a bubble.

import { describe, expect, it } from 'vitest'
import { counts, followPointer, type PointerAt, type PointerSeen, type PointerSources } from './measure'

/** A window and a page root that keep their listeners, and send events to them. */
function sources() {
  const win = new Map<string, Set<(e: PointerSeen) => void>>()
  const page = new Map<string, Set<() => void>>()
  const add = <F,>(m: Map<string, Set<F>>, type: string, cb: F) => void (m.get(type) ?? m.set(type, new Set()).get(type)!).add(cb)
  const src: PointerSources = {
    win: { addEventListener: (type, cb) => add(win, type, cb), removeEventListener: (type, cb) => void win.get(type)?.delete(cb) },
    page: { addEventListener: (type, cb) => add(page, type, cb), removeEventListener: (type, cb) => void page.get(type)?.delete(cb) },
  }
  const send = (type: string, pointerType: string, x = 0, y = 0, buttons = 0) => {
    for (const cb of win.get(type) ?? []) cb({ type, pointerType, buttons, clientX: x, clientY: y })
  }
  const leave = () => {
    for (const cb of page.get('pointerleave') ?? []) cb()
  }
  const listening = () => [...win.values(), ...page.values()].reduce((n, s) => n + s.size, 0)
  return { src, send, leave, listening }
}

function follow() {
  const s = sources()
  const at: { current: PointerAt | null } = { current: null }
  let changes = 0
  let clock = 0
  const stop = followPointer(s.src, at, () => changes++, () => ++clock)
  return { ...s, at, stop, changes: () => changes }
}

describe('the pointer', () => {
  it('is where a mouse was last seen, and stays there when its button is let go', () => {
    const p = follow()
    p.send('pointermove', 'mouse', 300, 200)
    expect(p.at.current).toMatchObject({ x: 300, y: 200 })
    p.send('pointerdown', 'mouse', 310, 200, 1)
    p.send('pointerup', 'mouse', 310, 200)
    expect(p.at.current).toMatchObject({ x: 310, y: 200 })
    expect(p.changes()).toBe(2)
  })

  it('has not moved when a move comes from where it already is: the page moved under it, not the pointer', () => {
    const p = follow()
    p.send('pointermove', 'mouse', 300, 200)
    const since = p.at.current?.movedAt
    // The browser tells the page of the pointer again when what is under it changes: the same place.
    p.send('pointermove', 'mouse', 300, 200)
    p.send('pointermove', 'mouse', 300, 200)
    expect(p.at.current).toEqual({ x: 300, y: 200, movedAt: since })
    // It is told all the same: what is under the pointer may be another thing now.
    expect(p.changes()).toBe(3)
    // A real move, and a press at the same place, are news.
    p.send('pointermove', 'mouse', 301, 200)
    expect(p.at.current?.movedAt).toBeGreaterThan(since as number)
    const moved = p.at.current?.movedAt
    p.send('pointerdown', 'mouse', 301, 200, 1)
    expect(p.at.current?.movedAt).toBeGreaterThan(moved as number)
  })

  it('is nowhere once a finger was lifted: the place of the last tap does not count', () => {
    const p = follow()
    p.send('pointerdown', 'touch', 120, 80, 1)
    expect(p.at.current).toMatchObject({ x: 120, y: 80 })
    p.send('pointermove', 'touch', 130, 90, 1)
    expect(p.at.current).toMatchObject({ x: 130, y: 90 })
    p.send('pointerup', 'touch', 130, 90)
    expect(p.at.current).toBeNull()
    // A scroll the browser took over ends the touch too.
    p.send('pointerdown', 'touch', 5, 5, 1)
    p.send('pointercancel', 'touch', 5, 5)
    expect(p.at.current).toBeNull()
  })

  it('does not count a pen that hovers: it is somewhere only while it touches', () => {
    const p = follow()
    p.send('pointermove', 'pen', 50, 50, 0)
    expect(p.at.current).toBeNull()
    p.send('pointerdown', 'pen', 60, 60, 1)
    expect(p.at.current).toMatchObject({ x: 60, y: 60 })
    p.send('pointerup', 'pen', 60, 60)
    expect(p.at.current).toBeNull()
    // Hovering over the place it just tapped brings nothing back.
    p.send('pointermove', 'pen', 60, 60, 0)
    expect(p.at.current).toBeNull()
  })

  it('is nowhere when it leaves the page or the window loses focus, and nothing is told twice', () => {
    const p = follow()
    p.send('pointermove', 'mouse', 1, 1)
    p.leave()
    expect(p.at.current).toBeNull()
    const told = p.changes()
    p.leave()
    p.send('blur', 'mouse')
    expect(p.changes()).toBe(told)
    p.send('pointermove', 'mouse', 2, 2)
    p.send('blur', 'mouse')
    expect(p.at.current).toBeNull()
  })

  it('stops listening when asked', () => {
    const p = follow()
    expect(p.listening()).toBe(6)
    p.stop()
    expect(p.listening()).toBe(0)
    p.send('pointermove', 'mouse', 9, 9)
    expect(p.at.current).toBeNull()
  })

  it('counts a mouse always, a touch or a pen only pressed', () => {
    expect(counts({ type: 'pointermove', pointerType: 'mouse', buttons: 0 })).toBe(true)
    expect(counts({ type: 'pointermove', pointerType: 'pen', buttons: 0 })).toBe(false)
    expect(counts({ type: 'pointermove', pointerType: 'touch', buttons: 1 })).toBe(true)
    expect(counts({ type: 'pointerdown', pointerType: 'pen', buttons: 0 })).toBe(true)
  })
})

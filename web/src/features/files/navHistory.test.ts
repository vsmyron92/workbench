import { describe, expect, it } from 'vitest'
import { NavHistory, type NavLocation } from './navHistory'

const at = (path: string, line: number, column = 1): NavLocation => ({ projectId: 'p', path, line, column })

describe('NavHistory', () => {
  it('keeps one entry per place and follows small moves', () => {
    const h = new NavHistory()
    h.record(at('a.rs', 1), 0)
    h.record(at('a.rs', 5), 1)
    h.record(at('a.rs', 9, 4), 2)
    expect(h.entries.map((e) => [e.path, e.line, e.column])).toEqual([['a.rs', 9, 4]])
    h.record(at('a.rs', 40), 3)
    h.record(at('b.rs', 2), 4)
    expect(h.entries.map((e) => `${e.path}:${e.line}`)).toEqual(['a.rs:9', 'a.rs:40', 'b.rs:2'])
  })

  it('goes back and forward without adding entries, and a new move drops the forward ones', () => {
    const h = new NavHistory()
    h.record(at('a.rs', 1), 0)
    h.record(at('b.rs', 1), 1)
    h.record(at('c.rs', 1), 2)
    expect(h.back(10)?.path).toBe('b.rs')
    // Arriving: the editor reports its old caret, then the entry's line.
    h.record(at('b.rs', 30), 11)
    h.record(at('b.rs', 1), 12)
    expect(h.back(20)?.path).toBe('a.rs')
    h.record(at('a.rs', 1), 21)
    expect(h.forward(30)?.path).toBe('b.rs')
    h.record(at('b.rs', 1), 31)
    expect(h.entries.map((e) => e.path)).toEqual(['a.rs', 'b.rs', 'c.rs'])
    h.record(at('d.rs', 1), 40)
    expect(h.entries.map((e) => e.path)).toEqual(['a.rs', 'b.rs', 'd.rs'])
    expect(h.canForward()).toBe(false)
    expect(h.back(-1)).not.toBeNull()
  })

  it('ignores other files while arriving, until the arrival times out', () => {
    const h = new NavHistory()
    h.record(at('a.rs', 1), 0)
    h.record(at('b.rs', 1), 1)
    h.back(10)
    h.record(at('b.rs', 1), 11) // the previously active editor, before the switch
    expect(h.entries.map((e) => e.path)).toEqual(['a.rs', 'b.rs'])
    h.record(at('z.rs', 1), 10_000) // long after: a real navigation
    expect(h.entries.map((e) => e.path)).toEqual(['a.rs', 'z.rs'])
  })

  it('lists distinct recent locations, newest first, and forgets deleted files', () => {
    const h = new NavHistory()
    h.record(at('a.rs', 1), 0)
    h.record(at('b.rs', 1), 1)
    h.record(at('a.rs', 50), 2)
    h.record(at('a.rs', 3), 3)
    expect(h.locations().map((e) => `${e.path}:${e.line}`)).toEqual(['a.rs:3', 'a.rs:50', 'b.rs:1'])
    h.forget('p', 'a.rs')
    expect(h.entries.map((e) => e.path)).toEqual(['b.rs'])
    expect(h.index).toBe(0)
  })
})

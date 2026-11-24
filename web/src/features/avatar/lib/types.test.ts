// The few helpers every module of lib/ shares: the figure's state, the stable hash, freezing.

import { describe, expect, it } from 'vitest'
import { avatarState, deepFreeze, stableHash } from './types'

describe('avatarState', () => {
  const base = { blocked: false, presenting: false, listening: false, askOpen: false, tone: 'idle' as const }

  it('takes the first that applies: attention, error, speaking, listening, waiting, working, done, sleeping, idle', () => {
    expect(avatarState(base)).toBe('idle')
    expect(avatarState({ ...base, tone: 'exited' })).toBe('sleeping')
    expect(avatarState({ ...base, tone: 'unread' })).toBe('done')
    expect(avatarState({ ...base, tone: 'working' })).toBe('working')
    expect(avatarState({ ...base, tone: 'starting' })).toBe('working')
    expect(avatarState({ ...base, tone: 'working', askOpen: true })).toBe('waiting')
    expect(avatarState({ ...base, tone: 'working', askOpen: true, listening: true })).toBe('listening')
    expect(avatarState({ ...base, tone: 'working', askOpen: true, listening: true, presenting: true })).toBe('speaking')
    expect(avatarState({ ...base, tone: 'error', askOpen: true, listening: true, presenting: true })).toBe('error')
    expect(avatarState({ ...base, tone: 'error', blocked: true, presenting: true })).toBe('attention')
    expect(avatarState({ ...base, tone: 'attention', presenting: true })).toBe('attention')
    expect(avatarState({ ...base, tone: 'exited', presenting: true })).toBe('speaking')
  })
})

describe('stableHash', () => {
  it('is the same for the same text, never negative, and spreads', () => {
    expect(stableHash('')).toBe(0x811c9dc5)
    expect(stableHash('a')).toBe(0xe40c292c)
    expect(stableHash('terminal-1')).toBe(stableHash('terminal-1'))
    const buckets = [0, 0, 0, 0, 0, 0, 0, 0]
    for (let i = 0; i < 800; i++) {
      const h = stableHash(`t${i}`)
      expect(h).toBeGreaterThanOrEqual(0)
      expect(Number.isInteger(h)).toBe(true)
      buckets[h % 8]++
    }
    for (const n of buckets) expect(n).toBeGreaterThan(50)
  })
})

describe('deepFreeze', () => {
  it('freezes a value and everything inside it', () => {
    const value = deepFreeze({ a: [1, { b: 2 }], c: { d: [3] }, e: 'text', f: null })
    expect(Object.isFrozen(value)).toBe(true)
    expect(Object.isFrozen(value.a)).toBe(true)
    expect(Object.isFrozen(value.a[1])).toBe(true)
    expect(Object.isFrozen(value.c.d)).toBe(true)
    expect(() => {
      ;(value.c.d as number[]).push(4)
    }).toThrow()
    // A value that holds itself does not send it round in circles.
    const loop: { self?: unknown } = {}
    loop.self = loop
    expect(Object.isFrozen(deepFreeze(loop))).toBe(true)
    expect(deepFreeze(7)).toBe(7)
    expect(deepFreeze(null)).toBeNull()
  })
})

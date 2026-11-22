// The avatar preferences against a browser made of a map: what is stored and read back,
// what another window's writing changes here (everything but the position), pruning, and
// that nothing breaks where there is no window or no storage.

import { afterEach, describe, expect, it, vi } from 'vitest'
import type { AvatarPrefs } from './store'

const NAME = 'wb.avatar.v1'

type StorageListener = (e: { key: string | null; newValue: string | null }) => void

/** A browser window as far as the store goes: localStorage in a map, and the listeners of `storage`. */
function browser(stored?: unknown) {
  const data = new Map<string, string>()
  if (stored !== undefined) data.set(NAME, typeof stored === 'string' ? stored : JSON.stringify(stored))
  const listeners: StorageListener[] = []
  const writes: string[] = []
  vi.stubGlobal('window', {
    localStorage: {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => {
        writes.push(k)
        data.set(k, v)
      },
      removeItem: (k: string) => void data.delete(k),
    },
    addEventListener: (type: string, cb: StorageListener) => {
      if (type === 'storage') listeners.push(cb)
    },
  })
  return {
    data,
    writes,
    listeners,
    stored: () => JSON.parse(data.get(NAME) ?? 'null') as { state: Record<string, unknown>; version: number } | null,
    /** Another window of the same browser wrote the preferences: the value is in storage, and this window is told. */
    otherWindowWrites(state: unknown, version = 1) {
      const value = typeof state === 'string' ? state : JSON.stringify({ state, version })
      data.set(NAME, value)
      for (const l of listeners) l({ key: NAME, newValue: value })
    },
  }
}

/** The store as a page load gets it. */
async function load() {
  vi.resetModules()
  return (await import('./store')).useAvatarPrefs
}

const data = (s: AvatarPrefs): AvatarPrefs => ({ show: s.show, voice: s.voice, online: s.online, volume: s.volume, speed: s.speed, reduceMotion: s.reduceMotion, roam: s.roam, pos: s.pos, voices: s.voices, seen: s.seen })
/** What is remembered per session, exactly: `toMatchObject` would take a part of it for the whole. */
const remembered = (s: AvatarPrefs) => ({ voices: s.voices, seen: s.seen })
const DEFAULTS: AvatarPrefs = { show: true, voice: null, online: false, volume: 1, speed: 1, reduceMotion: false, roam: true, pos: null, voices: {}, seen: {} }

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('useAvatarPrefs: what is stored', () => {
  it('starts with avatars shown, voice not chosen, online voices not allowed', async () => {
    const b = browser()
    const prefs = await load()
    expect(data(prefs.getState())).toEqual(DEFAULTS)
    // Reading stores nothing.
    expect(b.writes).toEqual([])
    expect(b.listeners).toHaveLength(1)
  })

  it('keeps a change under wb.avatar.v1 with version 1, and reads it back on the next page load', async () => {
    const b = browser()
    const prefs = await load()
    prefs.getState().setPrefs({ voice: true, speed: 1.2, pos: { x: 0.25, y: 0.5 } })
    prefs.getState().rememberVoice('ab12cd34ef56', 'Daniel')
    prefs.getState().markSeen('ab12cd34ef56', 'Lab12-7')
    expect(b.stored()).toEqual({
      state: { show: true, voice: true, online: false, volume: 1, speed: 1.2, reduceMotion: false, roam: true, pos: { x: 0.25, y: 0.5 }, voices: { ab12cd34ef56: 'Daniel' }, seen: { ab12cd34ef56: 'Lab12-7' } },
      version: 1,
    })
    const again = await load()
    expect(data(again.getState())).toEqual(b.stored()?.state)
  })

  it('cleans what it reads: a hand-edited or older entry never puts a value out of range into the app', async () => {
    browser({
      state: {
        show: 'yes',
        voice: 'loud',
        online: 1,
        volume: 7,
        speed: 0.1,
        reduceMotion: null,
        pos: { x: 2, y: -1 },
        voices: { ab12: 'Daniel', UPPER: 'x', 'a b': 'x', ['__proto__']: 'x', cd34: 7, ef56: '', gh78: 'v'.repeat(201) },
        seen: ['L1'],
        theme: 'pink',
      },
      version: 1,
    })
    const prefs = await load()
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, volume: 1, speed: 0.7, pos: { x: 1, y: 0 }, voices: { ab12: 'Daniel' } })
    expect('theme' in prefs.getState()).toBe(false)
    expect(Object.getPrototypeOf(prefs.getState().voices)).toBe(Object.prototype)
  })

  it('reads an entry of another version through the same cleaning', async () => {
    const b = browser({ state: { voice: false, volume: 0.5, speed: 9, extra: true }, version: 0 })
    const prefs = await load()
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, voice: false, volume: 0.5, speed: 1.5 })
    expect(b.stored()?.version).toBe(1)
  })

  it('starts from the defaults when the entry is not JSON, or no object', async () => {
    for (const broken of ['{not json', '7', 'null', '[]', JSON.stringify({ state: 'on', version: 1 }), JSON.stringify({ state: [1], version: 1 })]) {
      browser(broken)
      const prefs = await load()
      expect(data(prefs.getState()), broken).toEqual(DEFAULTS)
      prefs.getState().setPrefs({ show: false })
      expect(prefs.getState().show, broken).toBe(false)
    }
  })
})

describe('useAvatarPrefs: changing settings', () => {
  it('keeps numbers in their ranges and ignores what is no value', async () => {
    browser()
    const prefs = await load()
    const set = prefs.getState().setPrefs
    set({ volume: 3, speed: 0.2 })
    expect(prefs.getState()).toMatchObject({ volume: 1, speed: 0.7 })
    set({ volume: -1, speed: 9 })
    expect(prefs.getState()).toMatchObject({ volume: 0, speed: 1.5 })
    set({ volume: Number.NaN, speed: 'fast' as unknown as number, show: 'no' as unknown as boolean, voice: 1 as unknown as boolean, pos: [1, 2] as unknown as { x: number; y: number } })
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, volume: 0, speed: 1.5 })
    set({ pos: { x: 1.5, y: 0.25 } })
    expect(prefs.getState().pos).toEqual({ x: 1, y: 0.25 })
    set({ pos: { x: Infinity, y: 0 } })
    expect(prefs.getState().pos).toEqual({ x: 1, y: 0.25 })
    // Back to "not chosen" and to the default place.
    set({ voice: true })
    set({ voice: null, pos: null })
    expect(prefs.getState()).toMatchObject({ voice: null, pos: null })
    set(null as unknown as Partial<AvatarPrefs>)
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, volume: 0, speed: 1.5 })
  })

  it('writes nothing and tells nobody when nothing changes', async () => {
    const b = browser()
    const prefs = await load()
    prefs.getState().setPrefs({ voice: false, pos: { x: 0.5, y: 0.5 } })
    prefs.getState().rememberVoice('t1', 'Daniel')
    prefs.getState().markSeen('t1', 'L1')
    const writes = b.writes.length
    let told = 0
    const off = prefs.subscribe(() => told++)
    prefs.getState().setPrefs({ voice: false, show: true, volume: 1, pos: { x: 0.5, y: 0.5 } })
    prefs.getState().setPrefs({})
    prefs.getState().rememberVoice('t1', 'Daniel')
    prefs.getState().markSeen('t1', 'L1')
    prefs.getState().forget('t9')
    prefs.getState().prune(['t1', 't2'])
    expect(b.writes).toHaveLength(writes)
    expect(told).toBe(0)
    off()
  })

  it('changes only the keys it was given', async () => {
    browser()
    const prefs = await load()
    prefs.getState().setPrefs({ volume: 0.4, reduceMotion: true })
    prefs.getState().setPrefs({ online: true })
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, volume: 0.4, reduceMotion: true, online: true })
  })
})

describe('useAvatarPrefs: another window', () => {
  it('takes over every field another window wrote, and writes nothing back', async () => {
    const b = browser()
    const prefs = await load()
    prefs.getState().setPrefs({ voice: true })
    const writes = b.writes.length
    b.otherWindowWrites({ show: false, voice: false, online: true, volume: 0.3, speed: 1.4, reduceMotion: true, roam: false, pos: null, voices: { t1: 'Daniel' }, seen: { t1: 'L4' } })
    expect(data(prefs.getState())).toEqual({ show: false, voice: false, online: true, volume: 0.3, speed: 1.4, reduceMotion: true, roam: false, pos: null, voices: { t1: 'Daniel' }, seen: { t1: 'L4' } })
    // Nothing is written in answer: two windows would otherwise tell each other for ever.
    expect(b.writes).toHaveLength(writes)
    // The actions are still there after a state that came from storage.
    prefs.getState().setPrefs({ voice: true })
    expect(prefs.getState().voice).toBe(true)
  })

  it('keeps its own position: `pos` is per window while it is open', async () => {
    const b = browser()
    const prefs = await load()
    prefs.getState().setPrefs({ pos: { x: 0.2, y: 0.8 } })
    const mine = prefs.getState().pos
    b.otherWindowWrites({ ...DEFAULTS, voice: false, pos: { x: 0.9, y: 0.1 } })
    expect(prefs.getState().voice).toBe(false)
    expect(prefs.getState().pos).toBe(mine)
    b.otherWindowWrites({ ...DEFAULTS, pos: null })
    expect(prefs.getState().pos).toBe(mine)
    // A window that was never dragged stays at the default place whatever the others do.
    const c = browser()
    const fresh = await load()
    c.otherWindowWrites({ ...DEFAULTS, pos: { x: 0.9, y: 0.1 } })
    expect(fresh.getState().pos).toBeNull()
  })

  it('takes the position that is stored when the page loads', async () => {
    browser({ state: { ...DEFAULTS, pos: { x: 0.9, y: 0.1 } }, version: 1 })
    const prefs = await load()
    expect(prefs.getState().pos).toEqual({ x: 0.9, y: 0.1 })
  })

  it('cleans what another window wrote like what it reads at start', async () => {
    const b = browser()
    const prefs = await load()
    b.otherWindowWrites({ volume: 99, speed: -3, voice: 'on', voices: { t1: 'Daniel', 'no id': 'x' }, seen: 'L1' })
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, volume: 1, speed: 0.7, voices: { t1: 'Daniel' } })
  })

  it('tells nobody when the other window changed nothing this window shares', async () => {
    const b = browser()
    const prefs = await load()
    prefs.getState().setPrefs({ voice: true, pos: { x: 0.2, y: 0.8 } })
    prefs.getState().rememberVoice('t1', 'Daniel')
    const before = prefs.getState()
    let told = 0
    const off = prefs.subscribe(() => told++)
    // The other window was dragged: only its position differs.
    b.otherWindowWrites({ ...data(before), pos: { x: 0.6, y: 0.6 } })
    expect(told).toBe(0)
    expect(prefs.getState()).toBe(before)
    b.otherWindowWrites({ ...data(before), speed: 1.1, pos: { x: 0.6, y: 0.6 } })
    expect(told).toBe(1)
    // What did not change keeps its identity, so a component that reads it does not render again.
    expect(prefs.getState().voices).toBe(before.voices)
    expect(prefs.getState().pos).toBe(before.pos)
    off()
  })

  it('ignores other keys, a removed entry, what is no JSON, and another version of the app', async () => {
    const b = browser()
    const prefs = await load()
    prefs.getState().setPrefs({ voice: true })
    const before = prefs.getState()
    const writes = b.writes.length
    const tell = b.listeners[0]
    // Another store's entry changed. (Had the avatar entry changed too, its own event would say so.)
    const mine = b.data.get(NAME) ?? ''
    b.data.set(NAME, JSON.stringify({ state: { voice: false }, version: 1 }))
    b.data.set('wb.agents.v1', JSON.stringify({ state: { voice: false }, version: 1 }))
    tell({ key: 'wb.agents.v1', newValue: b.data.get('wb.agents.v1') ?? null })
    expect(prefs.getState()).toBe(before)
    b.data.set(NAME, mine)
    // Storage was cleared, or the entry removed.
    tell({ key: null, newValue: null })
    tell({ key: NAME, newValue: null })
    b.otherWindowWrites('{not json')
    b.otherWindowWrites('null')
    b.otherWindowWrites({ voice: false }, 2)
    b.otherWindowWrites({ voice: false }, 0)
    expect(prefs.getState()).toBe(before)
    expect(b.writes).toHaveLength(writes)
  })
})

describe('useAvatarPrefs: what is remembered per session', () => {
  it('remembers a voice and the last line seen per terminal, and forgets both with the terminal', async () => {
    browser()
    const prefs = await load()
    const s = prefs.getState()
    s.rememberVoice('t1', 'Daniel')
    s.rememberVoice('t2', 'Samantha')
    s.markSeen('t1', 'L3')
    s.markSeen('t2', 'L9')
    s.markSeen('t1', 'L4')
    expect(remembered(prefs.getState())).toEqual({ voices: { t1: 'Daniel', t2: 'Samantha' }, seen: { t1: 'L4', t2: 'L9' } })
    s.forget('t1')
    expect(remembered(prefs.getState())).toEqual({ voices: { t2: 'Samantha' }, seen: { t2: 'L9' } })
    expect(Object.keys(prefs.getState().voices)).toEqual(['t2'])
    // Only one of the two had it.
    s.markSeen('t3', 'L1')
    s.forget('t3')
    expect(prefs.getState().seen).toEqual({ t2: 'L9' })
  })

  it('prunes the terminals that are gone', async () => {
    const b = browser()
    const prefs = await load()
    for (const id of ['t1', 't2', 't3']) {
      prefs.getState().rememberVoice(id, 'Daniel')
      prefs.getState().markSeen(id, 'L1')
    }
    prefs.getState().markSeen('t4', 'L1')
    prefs.getState().prune(new Set(['t2', 't4', 't7']))
    expect(remembered(prefs.getState())).toEqual({ voices: { t2: 'Daniel' }, seen: { t2: 'L1', t4: 'L1' } })
    expect(Object.keys(prefs.getState().voices)).toEqual(['t2'])
    expect(b.stored()?.state).toMatchObject({ voices: { t2: 'Daniel' }, seen: { t2: 'L1', t4: 'L1' } })
    expect(Object.keys(b.stored()?.state.voices as object)).toEqual(['t2'])
    prefs.getState().prune([])
    expect(remembered(prefs.getState())).toEqual({ voices: {}, seen: {} })
  })

  it('takes only terminal ids and short texts', async () => {
    browser()
    const prefs = await load()
    const s = prefs.getState()
    for (const id of ['', 'T1', 'a b', '../x', '__proto__', 'x'.repeat(41)]) {
      s.rememberVoice(id, 'Daniel')
      s.markSeen(id, 'L1')
    }
    s.rememberVoice('t1', '')
    s.rememberVoice('t1', 'v'.repeat(201))
    s.rememberVoice('t1', 7 as unknown as string)
    s.markSeen('t1', '')
    s.markSeen('t1', null as unknown as string)
    expect(remembered(prefs.getState())).toEqual({ voices: {}, seen: {} })
    // A terminal id that is also the name of something every object has.
    s.rememberVoice('constructor', 'Daniel')
    s.markSeen('constructor', 'L1')
    expect(remembered(prefs.getState())).toEqual({ voices: { constructor: 'Daniel' }, seen: { constructor: 'L1' } })
    s.rememberVoice('constructor', 'Daniel')
    s.forget('constructor')
    expect(remembered(prefs.getState())).toEqual({ voices: {}, seen: {} })
    s.forget('constructor')
    s.forget('tostring')
  })

  it('remembers a bounded number of sessions: the ones written longest ago go first', async () => {
    browser()
    const prefs = await load()
    const id = (i: number) => `t${String(i).padStart(4, '0')}`
    for (let i = 0; i < 300; i++) {
      prefs.getState().rememberVoice(id(i), 'Daniel')
      prefs.getState().markSeen(id(i), `L${i}`)
    }
    expect(Object.keys(prefs.getState().voices)).toHaveLength(256)
    expect(Object.keys(prefs.getState().seen)).toHaveLength(256)
    expect(Object.hasOwn(prefs.getState().voices, id(43))).toBe(false)
    expect(Object.hasOwn(prefs.getState().voices, id(44))).toBe(true)
    // Written again: it is the newest now, and the next one to go is another.
    prefs.getState().markSeen(id(44), 'L999')
    prefs.getState().markSeen('tnew', 'L1')
    expect(prefs.getState().seen[id(44)]).toBe('L999')
    expect(Object.hasOwn(prefs.getState().seen, id(45))).toBe(false)
    // And an entry with more than that many sessions is cut when it is read.
    const many = Object.fromEntries(Array.from({ length: 400 }, (_, i) => [id(i), 'Daniel']))
    browser({ state: { voices: many }, version: 1 })
    const again = await load()
    expect(Object.keys(again.getState().voices)).toHaveLength(256)
    expect(Object.hasOwn(again.getState().voices, id(399))).toBe(true)
  })
})

describe('useAvatarPrefs: where there is no browser', () => {
  it('works without a window: nothing is stored, nothing throws', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const prefs = await load()
    expect(data(prefs.getState())).toEqual(DEFAULTS)
    prefs.getState().setPrefs({ voice: true, volume: 0.5 })
    prefs.getState().rememberVoice('t1', 'Daniel')
    prefs.getState().forget('t1')
    expect(data(prefs.getState())).toEqual({ ...DEFAULTS, voice: true, volume: 0.5 })
    // Not a word about storage being unavailable: it is expected there.
    expect(warn).not.toHaveBeenCalled()
    warn.mockRestore()
  })

  it('works in a browser that blocks storage', async () => {
    const listeners: StorageListener[] = []
    vi.stubGlobal('window', {
      get localStorage(): never {
        throw new Error('SecurityError: access is denied for this document')
      },
      addEventListener: (_type: string, cb: StorageListener) => void listeners.push(cb),
    })
    const prefs = await load()
    prefs.getState().setPrefs({ voice: false })
    expect(prefs.getState().voice).toBe(false)
    // Even told of a change it cannot read: nothing happens.
    listeners[0]({ key: NAME, newValue: JSON.stringify({ state: { voice: true }, version: 1 }) })
    expect(prefs.getState().voice).toBe(false)
  })

  it('works in a browser whose storage is full', async () => {
    const stored = JSON.stringify({ state: { speed: 1.3 }, version: 1 })
    vi.stubGlobal('window', {
      localStorage: {
        getItem: () => stored,
        setItem: () => {
          throw new Error('QuotaExceededError')
        },
        removeItem: () => {},
      },
    })
    const prefs = await load()
    expect(prefs.getState().speed).toBe(1.3)
    prefs.getState().setPrefs({ speed: 0.9 })
    expect(prefs.getState().speed).toBe(0.9)
  })
})
